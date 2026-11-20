use super::*;
use kinewright_core::output_review::{
    DeliveryException, EligibilityReason, MAX_REVIEW_ELEMENT_TEXT_BYTES, MAX_REVIEW_ELEMENTS,
    MAX_REVIEW_WORD_BYTES, MAX_REVIEW_WORDS, OverlayPixels, OverlayText, OverlayUnknown,
    ReviewLane, delivery_eligibility,
};
use kinewright_core::{
    AssetId, AutomationCurve, BlendMode, CaptionPreset, ClipContent, DeliveryEncodeDepth,
    DeliveryProfile, Effect, EffectId, Keyframe, KeyframeInterpolation, MediaKind,
    MediaSourceFingerprint, ParamValue, Title, Track, TrackId, TrackKind,
};
use kinewright_project::bundle_records::{ObjectRef, RecordKind};
use kinewright_project::owned_inputs::{MediaInterpretation, StreamInterpretation};
use std::collections::BTreeMap;

const SNAPSHOT: SnapshotIdentity = SnapshotIdentity {
    core: [1; 32],
    owner: [2; 32],
};

fn asset(id: u8, kind: MediaKind) -> MediaAsset {
    MediaAsset {
        id: AssetId(u64::from(id)),
        path: format!("fixture-{id}.mov").into(),
        remote_locator: None,
        name: format!("Fixture {id}"),
        duration: TimeCode(240),
        fps: Rational::default(),
        kind,
        resolution: (kind != MediaKind::Audio).then_some((640, 360)),
        source_fingerprint: MediaSourceFingerprint {
            content_sha256: Some(format!("{id:02x}").repeat(32)),
            byte_len: Some(100),
        },
        color_description: kinewright_core::ColorDescription::default(),
        assumed_from: None,
    }
}

fn pin(asset: &MediaAsset) -> OwnedInputEntry {
    let stream = |index| {
        Some(StreamInterpretation {
            index,
            time_base: (1, 48_000),
            timestamp_origin: 0,
        })
    };
    let profile = MediaInterpretation {
        version: 1,
        video: if asset.kind == MediaKind::Audio {
            None
        } else {
            stream(0)
        },
        audio: if matches!(asset.kind, MediaKind::Audio | MediaKind::AudioVideo) {
            stream(1)
        } else {
            None
        },
        fps: asset.fps,
        duration: asset.duration,
        kind: asset.kind,
        resolution: asset.resolution,
        effective_color: asset.color_description.clone(),
        probed_color: asset.color_description.clone(),
        source_profile_assumption: (asset.kind != MediaKind::Audio)
            .then(|| kinewright_media::source_profile_assumption(&asset.color_description))
            .flatten(),
    }
    .canonical()
    .unwrap();
    let object = ObjectRef {
        sha256: asset.source_fingerprint.content_sha256.clone().unwrap(),
        byte_length: 100,
        record_kind: RecordKind::OwnedMedia,
    };
    let profile_id = kinewright_media::sha256_bytes(profile.as_bytes());
    OwnedInputEntry {
        entry_id: format!("media:{}:{}:100:{profile_id}", asset.id, object.sha256),
        object,
        dependencies: vec![ObjectRef {
            sha256: "00".repeat(32),
            byte_length: 100,
            record_kind: RecordKind::InputChunkTable,
        }],
        selected_stream: None,
        time_base: None,
        origin: None,
        profile: Some(profile),
    }
}

fn clip(id: u64, asset: u64, at: i64, start: i64, end: i64) -> Clip {
    Clip {
        id: ClipId(id),
        asset: AssetId(asset),
        timeline_start: TimeCode(at),
        source_range: TimeCode(start)..TimeCode(end),
        content: ClipContent::Media,
        effects: Vec::new(),
        transition_in: None,
        link: None,
        audio_gain_tenth_db: 0,
        audio_fade_in_frames: TimeCode::ZERO,
        audio_fade_out_frames: TimeCode::ZERO,
        audio_gain_curve: None,
        speed_percent: 100,
        enabled: true,
        enabled_curve: None,
        blend_mode: BlendMode::Normal,
    }
}

fn title(id: u64, at: i64, text: &str, caption: bool) -> Clip {
    let mut clip = clip(id, 0, at, 0, 30);
    clip.content = ClipContent::Title(if caption {
        CaptionPreset::Clean.title(text)
    } else {
        Title {
            text: text.into(),
            ..Title::default()
        }
    });
    clip
}

fn track(id: u64, clips: Vec<Clip>) -> Track {
    Track {
        id: TrackId(id),
        kind: TrackKind::Video,
        name: None,
        sync_lock: true,
        clips,
    }
}

fn doc(assets: Vec<MediaAsset>, tracks: Vec<Track>) -> Document {
    let mut doc = Document {
        media_pool: assets,
        tracks,
        ..Document::default()
    };
    doc.duration = doc
        .tracks
        .iter()
        .flat_map(|track| &track.clips)
        .map(|clip| clip_end(&doc, clip).unwrap())
        .max()
        .unwrap_or(TimeCode::ZERO);
    doc
}

fn annotations() -> ReviewAnnotations {
    ReviewAnnotations {
        snapshot: ReviewSnapshotId(SNAPSHOT.digest()),
        sourced_elements: Vec::new(),
        text_cards: Vec::new(),
        intended_pauses: Vec::new(),
    }
}

fn manifest(document: &Document) -> OwnedInputManifest {
    OwnedInputManifest {
        project_id: None,
        session_id: None,
        owner_epoch: 1,
        entries: document.media_pool.iter().map(pin).collect(),
    }
}

fn settings(document: &Document) -> ExportSettings {
    DeliveryProfile::SourceMaster.export_settings(
        document,
        DeliveryEncodeDepth::Eight,
        kinewright_core::ExportCancellation::default(),
    )
}

fn derive(
    document: &Document,
    manifest: &OwnedInputManifest,
    transcripts: &[ReviewTranscript<'_>],
    annotations: &ReviewAnnotations,
) -> Result<ReviewExpectations, ReviewExpectationError> {
    derive_review_expectations(
        &ReviewExpectationInput {
            document,
            snapshot: SNAPSHOT,
            input_manifest: manifest,
            settings: &settings(document),
            transcripts,
        },
        annotations,
    )
}

fn transcript(words: &[Word]) -> ReviewTranscript<'_> {
    ReviewTranscript {
        identity: TranscriptIdentity {
            asset: AssetId(1),
            source_sha256: [1; 32],
            source_byte_length: 100,
            audio_stream: 1,
            revision_sha256: digest(&words).unwrap(),
        },
        words,
        landmarks: &[],
    }
}

fn word(text: &str, start_micros: u64, end_micros: u64) -> Word {
    Word {
        text: text.into(),
        start_micros,
        end_micros,
    }
}

fn us(at: u64) -> ExactMicros {
    ExactMicros::integer(at)
}

#[test]
fn output_review_mapping_repeats_and_retimes_half_open_segments() {
    let mut slow = clip(3, 1, 51, 30, 60);
    slow.speed_percent = 50;
    let mut document = doc(
        vec![asset(1, MediaKind::AudioVideo)],
        vec![track(
            1,
            vec![clip(1, 1, 3, 30, 60), clip(2, 1, 27, 30, 60), slow],
        )],
    );
    document.fps = Rational::new(24_000, 1_001).unwrap();
    document.duration = TimeCode(99);
    let manifest = manifest(&document);
    let mut settings = settings(&document);
    settings.fps = Rational::new(30_000, 1_001).unwrap();
    let words = [
        word("before", 900_000, 1_000_000),
        word("kept", 1_200_000, 1_400_000),
        word("after", 2_000_000, 2_100_000),
    ];
    let record = derive_review_expectations(
        &ReviewExpectationInput {
            document: &document,
            snapshot: SNAPSHOT,
            input_manifest: &manifest,
            settings: &settings,
            transcripts: &[transcript(&words)],
        },
        &annotations(),
    )
    .unwrap();
    let kept: Vec<_> = record.kept_words.known().collect();
    assert_eq!(
        kept.iter()
            .map(|word| (
                word.word_index,
                word.segment.clip,
                word.text.as_str(),
                word.output.clone()
            ))
            .collect::<Vec<_>>(),
        [
            (1, ClipId(1), "kept", us(325_125)..us(525_125)),
            (1, ClipId(2), "kept", us(1_326_125)..us(1_526_125)),
        ]
    );
    assert_eq!(
        record.speech_windows.known().cloned().collect::<Vec<_>>(),
        [us(325_125)..us(525_125), us(1_326_125)..us(1_526_125)]
    );
    let audio: Vec<_> = record
        .segments
        .iter()
        .filter(|segment| segment.id.lane == ReviewLane::Audio)
        .collect();
    assert_eq!(audio.len(), 2);
    assert_eq!(audio[0].source, us(1_000_000)..us(2_000_000));
    assert_eq!(audio[0].output.end, us(1_125_125));
    assert_eq!(audio[1].output.start, us(1_126_125));
    assert_eq!(audio[0].project.end, audio[1].project.start);
    assert!(!audio[0].project.contains(&audio[1].project.start));
    let video = record
        .segments
        .iter()
        .find(|segment| segment.id.clip == ClipId(3))
        .unwrap();
    assert_eq!(video.id.lane, ReviewLane::Video);
    assert_eq!(video.source, us(1_000_000)..us(2_000_000));
    assert_eq!(video.project, TimeCode(51)..TimeCode(99));
    assert_eq!(
        video.output,
        ExactMicros::new(6_406_400, 3).unwrap()..ExactMicros::new(12_412_400, 3).unwrap()
    );
}

#[test]
fn output_review_absence_empty_transcript_is_not_proven_silence() {
    let mut document = doc(
        vec![asset(1, MediaKind::AudioVideo)],
        vec![track(1, vec![clip(1, 1, 0, 0, 30)])],
    );
    let manifest = manifest(&document);
    let empty = derive(&document, &manifest, &[transcript(&[])], &annotations()).unwrap();
    assert!(
        matches!(empty.kept_words, ReviewInventory::NotDetermined { known } if known.is_empty())
    );
    assert!(matches!(
        empty.overlays.titles,
        ReviewInventory::NoneProven(AbsenceProof::NoDocumentElementsOnOutput)
    ));
    document.tracks[0].clips[0].speed_percent = 50;
    document.duration = TimeCode(60);
    let muted = derive(&document, &manifest, &[], &annotations()).unwrap();
    assert_eq!(
        muted.kept_words,
        ReviewInventory::NoneProven(AbsenceProof::NoAudibleSourceSegments)
    );
    assert!(muted.overlays.is_resolved());
}

#[test]
fn output_review_refuses_unrepresentable_source_clock() {
    let mut source = asset(1, MediaKind::Audio);
    source.duration = TimeCode(i64::MAX);
    let start = i64::MAX / 2;
    let mut audio = track(1, vec![clip(1, 1, 0, start, start + 30)]);
    audio.kind = TrackKind::Audio;
    let document = doc(vec![source], vec![audio]);
    assert!(matches!(
        derive(&document, &manifest(&document), &[], &annotations()),
        Err(ReviewExpectationError::Clock(AudibleClockError))
    ));
}

#[test]
fn output_review_absence_opaque_card_retains_pixels_and_blocks_when_unpinned() {
    let document = doc(
        vec![asset(2, MediaKind::Image)],
        vec![track(1, vec![clip(4, 2, 0, 0, 30)])],
    );
    let mut manifest = manifest(&document);
    let pinned = derive(&document, &manifest, &[], &annotations()).unwrap();
    let card = pinned.overlays.cards.known().next().unwrap();
    assert_eq!(card.text, OverlayText::Unknown(OverlayUnknown::OpaqueCard));
    assert_eq!(
        card.pixels,
        OverlayPixels::PinnedImage {
            sha256: [2; 32],
            byte_length: 100
        }
    );
    assert_eq!(card.visible[0].output, us(0)..us(1_000_000));
    assert_eq!(
        card.visible[0].bounds.unwrap().right,
        i32::try_from(pinned.raster.0).unwrap()
    );
    assert!(pinned.overlays.is_resolved());
    assert_eq!(
        delivery_eligibility(&pinned),
        [DeliveryException::Element {
            element: ClipId(4),
            reason: EligibilityReason::UnknownText(OverlayUnknown::OpaqueCard)
        }]
    );
    manifest.entries.clear();
    let unpinned = derive(&document, &manifest, &[], &annotations()).unwrap();
    assert_eq!(
        unpinned.overlays.cards.known().next().unwrap().pixels,
        OverlayPixels::Unknown(OverlayUnknown::ImageNotPinned)
    );
    assert!(!unpinned.overlays.is_resolved());
}

#[test]
fn output_review_pauses_require_author_intent() {
    let document = doc(
        vec![asset(1, MediaKind::AudioVideo)],
        vec![track(1, vec![clip(1, 1, 0, 0, 30)])],
    );
    let manifest = manifest(&document);
    let words = [word("one", 100_000, 200_000), word("two", 600_000, 700_000)];
    let transcript = [transcript(&words)];
    let mut annotations = annotations();
    let record = derive(&document, &manifest, &transcript, &annotations).unwrap();
    assert_eq!(
        record.intended_pauses,
        ReviewInventory::NoneProven(AbsenceProof::NoAuthorPauseDeclarations)
    );
    annotations.intended_pauses.push(TimeCode(6)..TimeCode(18));
    let record = derive(&document, &manifest, &transcript, &annotations).unwrap();
    assert_eq!(
        record.intended_pauses.known().collect::<Vec<_>>(),
        [&(us(200_000)..us(600_000))]
    );
    assert_eq!(
        record.speech_windows.known().cloned().collect::<Vec<_>>(),
        [us(100_000)..us(200_000), us(600_000)..us(700_000)]
    );
}

#[test]
fn output_review_rejects_annotation_from_another_cut() {
    let document = doc(
        Vec::new(),
        vec![track(1, vec![title(1, 0, "Opening", false)])],
    );
    let manifest = manifest(&document);
    let mut annotations = annotations();
    annotations.snapshot = ReviewSnapshotId([9; 32]);
    assert!(matches!(
        derive(&document, &manifest, &[], &annotations),
        Err(ReviewExpectationError::AnnotationSnapshot)
    ));
}

#[test]
fn output_review_rejects_annotation_for_a_missing_element() {
    let document = doc(
        Vec::new(),
        vec![track(1, vec![title(1, 0, "Opening", false)])],
    );
    let manifest = manifest(&document);
    let mut annotations = annotations();
    annotations.sourced_elements.push(ClipId(9));
    assert!(matches!(
        derive(&document, &manifest, &[], &annotations),
        Err(ReviewExpectationError::AnnotationElement(ClipId(9)))
    ));
}

#[test]
fn output_review_rejects_relabeling_a_caption_as_a_text_card() {
    let document = doc(
        Vec::new(),
        vec![track(1, vec![title(1, 0, "Opening", true)])],
    );
    let mut annotations = annotations();
    annotations.text_cards.push(ClipId(1));
    assert!(matches!(
        derive(&document, &manifest(&document), &[], &annotations),
        Err(ReviewExpectationError::AnnotationTextCard(ClipId(1)))
    ));
}

fn caption_record(caption: &str, kept: &[Word]) -> ReviewExpectations {
    let document = doc(
        vec![asset(1, MediaKind::AudioVideo)],
        vec![
            track(1, vec![clip(1, 1, 0, 0, 30)]),
            track(2, vec![title(2, 0, caption, true)]),
        ],
    );
    derive(
        &document,
        &manifest(&document),
        &[transcript(kept)],
        &annotations(),
    )
    .unwrap()
}

#[test]
fn output_review_matching_caption_with_digits_is_eligible() {
    let kept = [
        word("Version", 100_000, 300_000),
        word("3.5", 400_000, 800_000),
    ];
    let record = caption_record("VERSION: 3.5!", &kept);
    assert!(delivery_eligibility(&record).is_empty());
    assert!(record.overlays.is_resolved());
}

#[test]
fn output_review_eligibility_reports_all_caption_title_and_card_exceptions() {
    let document = doc(
        vec![asset(1, MediaKind::AudioVideo), asset(2, MediaKind::Image)],
        vec![
            track(1, vec![clip(1, 1, 0, 0, 30)]),
            track(2, vec![title(2, 0, "wrong second", true)]),
            track(3, vec![title(3, 0, "second first", true)]),
            track(4, vec![title(4, 0, "Chapter ٢", false)]),
            track(5, vec![clip(5, 2, 0, 0, 30)]),
            track(6, vec![title(6, 0, "Source excerpt", false)]),
        ],
    );
    let kept = [
        word("first", 100_000, 300_000),
        word("second", 400_000, 800_000),
    ];
    let mut annotations = annotations();
    annotations.text_cards.push(ClipId(6));
    annotations.sourced_elements.push(ClipId(6));
    let record = derive(
        &document,
        &manifest(&document),
        &[transcript(&kept)],
        &annotations,
    )
    .unwrap();
    assert_eq!(
        delivery_eligibility(&record),
        [
            DeliveryException::Element {
                element: ClipId(2),
                reason: EligibilityReason::CaptionWordsDisagree
            },
            DeliveryException::Element {
                element: ClipId(3),
                reason: EligibilityReason::CaptionWordsDisagree
            },
            DeliveryException::Element {
                element: ClipId(4),
                reason: EligibilityReason::DecimalDigit
            },
            DeliveryException::Element {
                element: ClipId(5),
                reason: EligibilityReason::UnknownText(OverlayUnknown::OpaqueCard)
            },
            DeliveryException::Element {
                element: ClipId(6),
                reason: EligibilityReason::SourcedText
            },
        ]
    );
}

#[test]
fn output_review_numeric_normalization_preserves_decimals_and_signs() {
    for (written, spoken, agrees) in [
        ("35", "3.5", false),
        ("2", "-2", false),
        ("2", "+2", false),
        ("2", "− 2", false),
        ("−2!", "-2", true),
        ("(.5).", ".5", true),
        ("5", ".5", false),
    ] {
        let record = caption_record(written, &[word(spoken, 200_000, 300_000)]);
        let expected = if agrees {
            vec![]
        } else {
            vec![DeliveryException::Element {
                element: ClipId(2),
                reason: EligibilityReason::CaptionWordsDisagree,
            }]
        };
        assert_eq!(
            delivery_eligibility(&record),
            expected,
            "{written} vs {spoken}"
        );
    }
}

#[test]
fn output_review_determinism_ignores_media_pool_and_annotation_order() {
    let mut document = doc(
        vec![asset(1, MediaKind::AudioVideo), asset(2, MediaKind::Image)],
        vec![
            track(1, vec![clip(1, 1, 0, 0, 30)]),
            track(2, vec![title(2, 0, "Opening", false)]),
            track(3, vec![clip(3, 2, 0, 0, 30)]),
        ],
    );
    let manifest = manifest(&document);
    let mut annotations = annotations();
    annotations.sourced_elements = vec![ClipId(2), ClipId(3)];
    annotations.intended_pauses = vec![TimeCode(25)..TimeCode(29), TimeCode(2)..TimeCode(5)];
    let words = [word("one", 220_000, 340_000)];
    let sources = [transcript(&words)];
    let first = derive(&document, &manifest, &sources, &annotations).unwrap();
    let bytes = serde_json::to_vec(&first).unwrap();
    assert_eq!(
        serde_json::from_slice::<ReviewExpectations>(&bytes).unwrap(),
        first
    );
    assert_eq!(
        bytes,
        serde_json::to_vec(&derive(&document, &manifest, &sources, &annotations).unwrap()).unwrap()
    );
    document.media_pool.reverse();
    annotations.sourced_elements.reverse();
    annotations.intended_pauses.reverse();
    assert_eq!(
        bytes,
        serde_json::to_vec(&derive(&document, &manifest, &sources, &annotations).unwrap()).unwrap()
    );
    assert_eq!(
        first.identity.input_manifest_sha256,
        digest(&manifest).unwrap()
    );
    assert_eq!(
        first.identity.export_settings_sha256,
        digest(&settings(&document)).unwrap()
    );
}

fn speech_document() -> Document {
    doc(
        vec![asset(1, MediaKind::AudioVideo)],
        vec![track(1, vec![clip(1, 1, 0, 0, 30)])],
    )
}

#[test]
fn output_review_bounds_reject_word_count_by_name() {
    let document = speech_document();
    let words: Vec<_> = (0..=MAX_REVIEW_WORDS)
        .map(|index| {
            let at = u64::try_from(index).unwrap();
            word("a", at, at + 1)
        })
        .collect();
    assert_bound(
        &derive(
            &document,
            &manifest(&document),
            &[transcript(&words)],
            &annotations(),
        ),
        "source_words",
        MAX_REVIEW_WORDS,
    );
}

#[test]
fn output_review_bounds_reject_word_text_by_name() {
    let document = speech_document();
    let words = [word(
        &"a".repeat(MAX_REVIEW_WORD_BYTES + 1),
        10_000,
        100_000,
    )];
    assert_bound(
        &derive(
            &document,
            &manifest(&document),
            &[transcript(&words)],
            &annotations(),
        ),
        "word_text_bytes",
        MAX_REVIEW_WORD_BYTES,
    );
}

#[test]
fn output_review_bounds_reject_element_count_by_name() {
    let elements = (0..=MAX_REVIEW_ELEMENTS)
        .map(|index| {
            title(
                u64::try_from(index + 1).unwrap(),
                i64::try_from(index * 30).unwrap(),
                "A",
                false,
            )
        })
        .collect();
    let too_many = doc(Vec::new(), vec![track(1, elements)]);
    assert_bound(
        &derive(&too_many, &manifest(&too_many), &[], &annotations()),
        "elements",
        MAX_REVIEW_ELEMENTS,
    );
}

#[test]
fn output_review_bounds_reject_element_text_by_name() {
    let too_long = doc(
        Vec::new(),
        vec![track(
            1,
            vec![title(
                1,
                0,
                &"a".repeat(MAX_REVIEW_ELEMENT_TEXT_BYTES + 1),
                false,
            )],
        )],
    );
    assert_bound(
        &derive(&too_long, &manifest(&too_long), &[], &annotations()),
        "element_text_bytes",
        MAX_REVIEW_ELEMENT_TEXT_BYTES,
    );
}

fn assert_bound(
    outcome: &Result<ReviewExpectations, ReviewExpectationError>,
    field: &str,
    maximum: usize,
) {
    assert!(
        matches!(outcome, Err(ReviewExpectationError::Bound { field: actual, maximum: limit }) if *actual == field && *limit == maximum),
        "expected {field} limit"
    );
}

#[test]
fn output_review_bounds_follow_animated_title_at_export_raster() {
    let mut title = title(1, 0, "Caption", true);
    title.effects.push(Effect {
        id: EffectId(1),
        name: "transform".into(),
        enabled: true,
        enabled_curve: None,
        parameters: BTreeMap::from([("x_percent".to_owned(), ParamValue::Integer(0))]),
        keyframes: BTreeMap::from([(
            "x_percent".to_owned(),
            AutomationCurve {
                keyframes: vec![
                    Keyframe {
                        at: TimeCode::ZERO,
                        value: 0,
                        interpolation: KeyframeInterpolation::Hold,
                        tangent_in: 0,
                        tangent_out: 0,
                    },
                    Keyframe {
                        at: TimeCode(15),
                        value: 10,
                        interpolation: KeyframeInterpolation::Hold,
                        tangent_in: 0,
                        tangent_out: 0,
                    },
                ],
            },
        )]),
    });
    let document = doc(Vec::new(), vec![track(1, vec![title])]);
    let manifest = manifest(&document);
    let mut settings = settings(&document);
    settings.resolution = (720, 1280);
    let record = derive_review_expectations(
        &ReviewExpectationInput {
            document: &document,
            snapshot: SNAPSHOT,
            input_manifest: &manifest,
            settings: &settings,
            transcripts: &[],
        },
        &annotations(),
    )
    .unwrap();
    let caption = record.overlays.captions.known().next().unwrap();
    let bounds = &caption.visible;
    assert_eq!(bounds.len(), 2);
    assert_eq!(bounds[0].output, us(0)..us(500_000));
    assert_eq!(bounds[1].output, us(500_000)..us(1_000_000));
    let base = kinewright_core::title_layout(&CaptionPreset::Clean.title("Caption"), (720, 1280))
        .unwrap()
        .visual_bounds;
    assert_eq!(bounds[0].bounds, Ok(base));
    assert!((71..=72).contains(&(bounds[1].bounds.unwrap().left - base.left)));
    assert!(record.overlays.is_resolved());
}

#[test]
fn output_review_landmarks_keep_independent_measurement_and_clock_uncertainty() {
    let document = doc(
        vec![asset(1, MediaKind::AudioVideo)],
        vec![track(1, vec![clip(1, 1, 0, 30, 60)])],
    );
    let manifest = manifest(&document);
    let words = [word("one", 1_200_000, 1_300_000)];
    let mut transcript = transcript(&words);
    let no_measurement = derive(
        &document,
        &manifest,
        std::slice::from_ref(&transcript),
        &annotations(),
    )
    .unwrap();
    assert!(
        matches!(no_measurement.av_landmarks, ReviewInventory::NotDetermined { known } if known.is_empty())
    );
    let source = [SourceAvLandmark {
        id: 3,
        audio_at: us(1_400_000),
        video_at: us(1_405_000),
        uncertainty: us(5_000),
    }];
    transcript.landmarks = &source;
    let record = derive(&document, &manifest, &[transcript], &annotations()).unwrap();
    let anchor = record.av_landmarks.known().next().unwrap();
    assert_eq!(
        (anchor.source_landmark, anchor.audio_at, anchor.video_at),
        (3, us(400_000), us(405_000))
    );
    assert_eq!(anchor.uncertainty, ExactMicros::new(1_260_250, 12).unwrap());
}

#[test]
fn output_review_transcript_stream_must_match_the_manifest() {
    let document = doc(
        vec![asset(1, MediaKind::AudioVideo)],
        vec![track(1, vec![clip(1, 1, 0, 0, 30)])],
    );
    let words = [word("one", 200_000, 300_000)];
    let mut transcript = transcript(&words);
    transcript.identity.audio_stream = 7;
    assert!(matches!(
        derive(
            &document,
            &manifest(&document),
            &[transcript],
            &annotations()
        ),
        Err(ReviewExpectationError::TranscriptBinding(AssetId(1)))
    ));
}
