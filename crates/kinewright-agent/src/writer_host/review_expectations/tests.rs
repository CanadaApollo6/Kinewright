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
        recognized_source: (!words.is_empty()).then_some(us(0)..us(8_000_000)),
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
    let record = caption_record("Version 3.5", &kept);
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
                reason: EligibilityReason::NumericCharacter
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
        ("−2", "-2", true),
        ("(.5).", ".5", false),
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

fn caption_exception(reason: EligibilityReason) -> [DeliveryException; 1] {
    [DeliveryException::Element {
        element: ClipId(2),
        reason,
    }]
}

#[test]
fn output_review_fix1_titles_refuse_all_unicode_number_categories() {
    for text in [
        "E = mc²",
        "½ price",
        "Room Ⅻ",
        "Step ③",
        "Edition ٢",
        "Room 2\u{202e}",
    ] {
        let document = doc(Vec::new(), vec![track(2, vec![title(2, 0, text, false)])]);
        let record = derive(&document, &manifest(&document), &[], &annotations()).unwrap();
        assert_eq!(
            delivery_eligibility(&record),
            caption_exception(EligibilityReason::NumericCharacter),
            "{text:?}"
        );
    }
}

#[test]
fn output_review_fix1_numeric_forms_preserve_every_value_character() {
    for (written, kept) in [
        ("－2", "2"),
        ("−½", "½"),
        ("1/2", "1 2"),
        ("1⁄2", "1 2"),
        ("50%", "50"),
        ("50‰", "50"),
        ("1e−3", "1e3"),
        ("1–2", "1 2"),
        ("1−2", "1-2"),
        ("1,234", "1234"),
        ("1\u{202f}234", "1 234"),
        ("(2)", "2"),
        ("3!", "3"),
    ] {
        let record = caption_record(written, &[word(kept, 200_000, 300_000)]);
        assert_eq!(
            delivery_eligibility(&record),
            caption_exception(EligibilityReason::CaptionWordsDisagree),
            "{written:?} vs {kept:?}"
        );
        let matching = caption_record(written, &[word(written, 200_000, 300_000)]);
        assert!(
            delivery_eligibility(&matching).is_empty(),
            "same form {written:?}"
        );
    }
}

#[test]
fn output_review_fix1_controls_and_marks_refuse_numeric_comparison() {
    for written in ["-\u{200b}2", "1\u{20dd}2", "1\u{202e}2"] {
        let record = caption_record(written, &[word(written, 200_000, 300_000)]);
        assert_eq!(
            delivery_eligibility(&record),
            caption_exception(EligibilityReason::NumericTextUncomparable),
            "{written:?}"
        );
    }
}

#[test]
fn output_review_fix2_numeric_caption_preserves_unit_and_prose_case() {
    let kept = [word("5", 100_000, 200_000), word("mW", 300_000, 400_000)];
    assert_eq!(
        delivery_eligibility(&caption_record("5 MW", &kept)),
        caption_exception(EligibilityReason::CaptionWordsDisagree)
    );
    let kept = [word("5", 100_000, 200_000), word("MW", 300_000, 400_000)];
    assert!(delivery_eligibility(&caption_record("5 MW", &kept)).is_empty());
    let kept = [word("costs", 100_000, 200_000), word("5", 300_000, 400_000)];
    assert_eq!(
        delivery_eligibility(&caption_record("Costs 5", &kept)),
        caption_exception(EligibilityReason::CaptionWordsDisagree)
    );
}

fn test_effect(name: &str, parameter: &str, value: i64) -> Effect {
    Effect {
        id: EffectId(1),
        name: name.into(),
        enabled: true,
        enabled_curve: None,
        parameters: BTreeMap::from([(parameter.into(), ParamValue::Integer(value))]),
        keyframes: BTreeMap::new(),
    }
}

fn adjustment(id: u64, at: i64, duration: i64, effect: Effect) -> Clip {
    let mut clip = clip(id, 0, at, 0, duration);
    clip.content = ClipContent::Adjustment;
    clip.effects.push(effect);
    clip
}

#[test]
fn output_review_fix1_upper_adjustment_splits_bounds_and_color_preserves_them() {
    let lower = title(1, 0, "Lower title", false);
    let mut document = doc(
        Vec::new(),
        vec![
            track(1, vec![lower]),
            track(
                2,
                vec![adjustment(
                    2,
                    10,
                    10,
                    test_effect("transform", "x_percent", 25),
                )],
            ),
        ],
    );
    let geometry = derive(&document, &manifest(&document), &[], &annotations()).unwrap();
    let geometry_runs = &geometry.overlays.titles.known().next().unwrap().visible;
    assert_eq!(geometry_runs.len(), 3);
    let rate = document.fps;
    assert_eq!(
        geometry_runs[1].output,
        times(TimeCode(10)..TimeCode(20), rate).unwrap()
    );
    assert_eq!(
        geometry_runs[1].bounds,
        Err(OverlayUnknown::UpperLayerGeometry)
    );
    assert!(!geometry.overlays.is_resolved());
    document.tracks[1].clips[0].effects[0] = test_effect("brightness", "percent", 10);
    let graded = derive(&document, &manifest(&document), &[], &annotations()).unwrap();
    let graded_runs = &graded.overlays.titles.known().next().unwrap().visible;
    assert_eq!(graded_runs.len(), 1);
    assert_eq!(graded_runs[0].output, us(0)..us(1_000_000));
    assert_eq!(graded_runs[0].bounds, geometry_runs[0].bounds);
    assert!(graded.overlays.is_resolved());
}

#[test]
fn output_review_fix2_adjustment_slide_marks_only_its_spatial_frames() {
    let mut adjustment = adjustment(2, 5, 25, test_effect("brightness", "percent", 10));
    adjustment.transition_in = Some(kinewright_core::Transition {
        name: "slide_left".into(),
        duration: TimeCode(21),
    });
    let document = doc(
        Vec::new(),
        vec![
            track(1, vec![title(1, 0, "Lower title", false)]),
            track(2, vec![adjustment]),
        ],
    );
    let slide = derive(&document, &manifest(&document), &[], &annotations()).unwrap();
    let runs = &slide.overlays.titles.known().next().unwrap().visible;
    assert_eq!(runs.len(), 3);
    let span = |start, end| times(TimeCode(start)..TimeCode(end), document.fps).unwrap();
    assert_eq!(runs[0].output, span(0, 5));
    assert_eq!(runs[1].output, span(5, 25));
    assert_eq!(runs[1].bounds, Err(OverlayUnknown::UpperLayerGeometry));
    assert_eq!(runs[2].output, span(25, 30));
    assert!(runs[0].bounds.is_ok());
    assert_eq!(runs[2].bounds, runs[0].bounds);
    assert!(!slide.overlays.is_resolved());
}

#[test]
fn output_review_fix2_adjustment_crossfade_preserves_bounds() {
    let mut document = doc(
        Vec::new(),
        vec![
            track(1, vec![title(1, 0, "Lower title", false)]),
            track(
                2,
                vec![adjustment(
                    2,
                    5,
                    25,
                    test_effect("brightness", "percent", 10),
                )],
            ),
        ],
    );
    let baseline = derive(&document, &manifest(&document), &[], &annotations()).unwrap();
    document.tracks[1].clips[0].transition_in = Some(kinewright_core::Transition {
        name: "crossfade".into(),
        duration: TimeCode(21),
    });
    let crossfade = derive(&document, &manifest(&document), &[], &annotations()).unwrap();
    assert_eq!(crossfade.overlays.titles, baseline.overlays.titles);
    assert!(crossfade.overlays.is_resolved());
}

#[test]
fn output_review_fix1_word_elsewhere_does_not_cover_the_kept_gap() {
    let asset = asset(1, MediaKind::AudioVideo);
    let mut document = doc(vec![asset], vec![track(1, vec![clip(1, 1, 0, 30, 60)])]);
    let words = [word("spoken", 100_000, 200_000)];
    let mut transcript = transcript(&words);
    transcript.recognized_source = Some(us(0)..us(1_000_000));
    let transcript = [transcript];
    let original = derive(&document, &manifest(&document), &transcript, &annotations()).unwrap();
    assert!(!original.kept_words.is_determined());
    document.tracks[0].clips.push(clip(2, 1, 30, 0, 30));
    document.duration = TimeCode(60);
    let extended = derive(&document, &manifest(&document), &transcript, &annotations()).unwrap();
    assert!(!extended.kept_words.is_determined());
    assert!(!extended.speech_windows.is_determined());
    assert_eq!(extended.kept_words.known().count(), 1);
    assert_eq!(
        extended.uncovered_speech_segments,
        original.uncovered_speech_segments
    );
    assert_eq!(extended.uncovered_speech_segments[0].clip, ClipId(1));
}

#[test]
fn output_review_fix1_covered_wordless_range_cannot_match_a_caption() {
    let asset = asset(1, MediaKind::AudioVideo);
    let document = doc(
        vec![asset],
        vec![
            track(1, vec![clip(1, 1, 0, 30, 60)]),
            track(2, vec![title(2, 0, "Someone spoke", true)]),
        ],
    );
    let mut silent = transcript(&[]);
    silent.recognized_source = Some(us(1_000_000)..us(2_000_000));
    let record = derive(&document, &manifest(&document), &[silent], &annotations()).unwrap();
    assert_eq!(
        record.kept_words,
        ReviewInventory::NoneProven(AbsenceProof::NoRecognizedSpeechInKeptRanges)
    );
    assert!(record.uncovered_speech_segments.is_empty());
    assert_eq!(
        delivery_eligibility(&record),
        caption_exception(EligibilityReason::CaptionWordsDisagree)
    );
}

#[test]
fn output_review_fix1_constant_keys_are_charged_for_every_evaluation() {
    let mut clip = title(1, 0, "Long title", false);
    clip.source_range.end = TimeCode(108_000);
    let mut effect = test_effect("transform", "x_percent", 0);
    effect.keyframes.insert(
        "x_percent".into(),
        AutomationCurve {
            keyframes: (0..10_000).map(|at| hold(at, 0)).collect(),
        },
    );
    clip.effects.push(effect);
    let document = doc(Vec::new(), vec![track(1, vec![clip])]);
    assert_bound(
        &derive(&document, &manifest(&document), &[], &annotations()),
        "derivation_work",
        MAX_REVIEW_DERIVATION_WORK,
    );
}

fn hold(at: i64, value: i64) -> Keyframe {
    Keyframe {
        at: TimeCode(at),
        value,
        interpolation: KeyframeInterpolation::Hold,
        tangent_in: 0,
        tangent_out: 0,
    }
}

fn fading_title(frames: i64) -> Clip {
    let mut clip = title(2, 0, "Title at work limit", false);
    clip.source_range.end = TimeCode(frames);
    let ClipContent::Title(title) = &mut clip.content else {
        unreachable!()
    };
    title.fade_in_frames = TimeCode(2);
    clip
}

#[test]
fn output_review_fix1_enable_scans_and_overlay_evaluation_share_one_limit() {
    let mut asset = asset(1, MediaKind::Video);
    asset.duration = TimeCode(400_000);
    let mut video = clip(1, 1, 0, 0, 300_000);
    video.enabled_curve = Some(AutomationCurve {
        keyframes: vec![hold(0, 1)],
    });
    let document = doc(
        vec![asset],
        vec![
            track(1, vec![video]),
            track(2, vec![fading_title(1_000_000)]),
        ],
    );
    assert_bound(
        &derive(&document, &manifest(&document), &[], &annotations()),
        "derivation_work",
        MAX_REVIEW_DERIVATION_WORK,
    );
}

#[test]
fn output_review_fix1_derivation_admits_the_full_work_budget() {
    let frames = i64::try_from(MAX_REVIEW_DERIVATION_WORK - 1).unwrap();
    let document = doc(Vec::new(), vec![track(1, vec![fading_title(frames)])]);
    let manifest = manifest(&document);
    let started = std::time::Instant::now();
    let output = derive(&document, &manifest, &[], &annotations()).unwrap();
    eprintln!(
        "derivation at {} work units ({frames} output frames): {:?}",
        MAX_REVIEW_DERIVATION_WORK,
        started.elapsed()
    );
    let run = &output.overlays.titles.known().next().unwrap().visible[0];
    assert_eq!(
        run.output,
        times(TimeCode(1)..TimeCode(frames), document.fps).unwrap()
    );
    assert!(run.bounds.is_ok());
}

fn later_landmarks() -> Vec<SourceAvLandmark> {
    (0..u32::try_from(kinewright_core::output_review::MAX_REVIEW_LANDMARKS).unwrap())
        .map(|id| SourceAvLandmark {
            id,
            audio_at: us(1_100_000),
            video_at: us(1_100_000),
            uncertainty: us(0),
        })
        .collect()
}

fn repeated_first_second(count: i64) -> Track {
    track(
        1,
        (0..count)
            .map(|i| clip(100 + u64::try_from(i).unwrap(), 1, i * 30, 0, 30))
            .collect(),
    )
}

#[test]
fn output_review_fix2_out_of_range_landmark_scans_exceed_the_work_limit() {
    let document = doc(
        vec![asset(1, MediaKind::AudioVideo)],
        vec![repeated_first_second(
            i64::try_from(MAX_REVIEW_CLIPS).unwrap(),
        )],
    );
    let landmarks = later_landmarks();
    let mut evidence = transcript(&[]);
    evidence.landmarks = &landmarks;
    assert_bound(
        &derive(&document, &manifest(&document), &[evidence], &annotations()),
        "derivation_work",
        MAX_REVIEW_DERIVATION_WORK,
    );
}

#[test]
fn output_review_fix2_full_work_budget_includes_timing_and_overlays() {
    const REPEATS: i64 = 160;
    // Registration and timing for this fixture charge 1,377,350 units.
    // The fading title uses one unit per output frame.
    const FRAMES: i64 = 622_650;
    let mut document = doc(
        vec![asset(1, MediaKind::AudioVideo)],
        vec![
            repeated_first_second(REPEATS),
            track(2, vec![fading_title(FRAMES)]),
        ],
    );
    let manifest = manifest(&document);
    let landmarks = later_landmarks();
    let mut evidence = transcript(&[]);
    evidence.landmarks = &landmarks;
    evidence.recognized_source = Some(us(0)..us(1_000_000));
    let evidence = [evidence];
    let started = std::time::Instant::now();
    let result = derive(&document, &manifest, &evidence, &annotations()).unwrap();
    eprintln!(
        "mixed derivation at {MAX_REVIEW_DERIVATION_WORK} work units ({FRAMES} overlay frames, {REPEATS} repeats x {} source landmarks): {:?}",
        landmarks.len(),
        started.elapsed()
    );
    assert_eq!(result.segments.len(), usize::try_from(REPEATS * 2).unwrap());
    assert!(matches!(
        result.av_landmarks,
        ReviewInventory::NotDetermined { known } if known.is_empty()
    ));
    let run = &result.overlays.titles.known().next().unwrap().visible[0];
    assert_eq!(
        run.output,
        times(TimeCode(1)..TimeCode(FRAMES), document.fps).unwrap()
    );
    assert!(run.bounds.is_ok());

    document.duration = TimeCode(FRAMES + 1);
    document.tracks[1].clips[0].source_range.end = document.duration;
    assert_bound(
        &derive(&document, &manifest, &evidence, &annotations()),
        "derivation_work",
        MAX_REVIEW_DERIVATION_WORK,
    );
}

#[test]
fn output_review_fix1_disabled_element_retains_annotations() {
    use kinewright_core::output_review::OffOutputAnnotation;
    let mut disabled = title(1, 0, "Disabled", false);
    disabled.enabled = false;
    let document = doc(Vec::new(), vec![track(1, vec![disabled])]);
    let mut annotations = annotations();
    annotations.sourced_elements.push(ClipId(1));
    annotations.text_cards.push(ClipId(1));
    let record = derive(&document, &manifest(&document), &[], &annotations).unwrap();
    assert_eq!(
        record.overlays.cards,
        ReviewInventory::NoneProven(AbsenceProof::NoDocumentElementsOnOutput)
    );
    assert_eq!(
        record.overlays.annotated_off_output,
        [OffOutputAnnotation {
            element: ClipId(1),
            sourced: true,
            text_card: true
        },]
    );
    assert!(record.overlays.is_resolved());
}

#[test]
fn output_review_fix1_offscreen_title_blocks_overlay_verification() {
    use kinewright_core::output_review::OffOutputAnnotation;
    let mut hidden = title(2, 0, "Offscreen", false);
    hidden
        .effects
        .push(test_effect("transform", "x_percent", 100));
    let document = doc(Vec::new(), vec![track(1, vec![hidden])]);
    let mut annotations = annotations();
    annotations.sourced_elements.push(ClipId(2));
    let record = derive(&document, &manifest(&document), &[], &annotations).unwrap();
    let title = record.overlays.titles.known().next().unwrap();
    assert_eq!(title.visible[0].bounds, Err(OverlayUnknown::NotVisible));
    assert!(!record.overlays.is_resolved());
    assert_eq!(
        record.overlays.annotated_off_output,
        [OffOutputAnnotation {
            element: ClipId(2),
            sourced: true,
            text_card: false
        },]
    );
}

#[test]
fn output_review_fix1_unsampled_element_retains_annotations() {
    use kinewright_core::output_review::OffOutputAnnotation;
    let mut between_samples = title(1, 1, "Between frames", false);
    between_samples.source_range.end = TimeCode(1);
    let document = doc(Vec::new(), vec![track(1, vec![between_samples])]);
    let manifest = manifest(&document);
    let mut settings = settings(&document);
    settings.fps = Rational::new(1, 1).unwrap();
    let mut annotations = annotations();
    annotations.text_cards.push(ClipId(1));
    let record = derive_review_expectations(
        &ReviewExpectationInput {
            document: &document,
            snapshot: SNAPSHOT,
            input_manifest: &manifest,
            settings: &settings,
            transcripts: &[],
        },
        &annotations,
    )
    .unwrap();
    assert_eq!(
        record.overlays.cards,
        ReviewInventory::NoneProven(AbsenceProof::NoDocumentElementsOnOutput)
    );
    assert_eq!(
        record.overlays.annotated_off_output,
        [OffOutputAnnotation {
            element: ClipId(1),
            sourced: false,
            text_card: true
        },]
    );
}

#[test]
fn output_review_fix1_video_hold_keeps_one_source_frame_at_mixed_rates() {
    use kinewright_core::output_review::SegmentSampling;
    let mut asset = asset(1, MediaKind::AudioVideo);
    asset.fps = Rational::new(24, 1).unwrap();
    let mut hold = clip(1, 1, 3, 0, 30);
    hold.content = ClipContent::Freeze(kinewright_core::FreezeFrame {
        source_frame: TimeCode(12),
    });
    let document = doc(vec![asset], vec![track(1, vec![hold])]);
    let manifest = manifest(&document);
    let mut settings = settings(&document);
    settings.fps = Rational::new(24, 1).unwrap();
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
    assert_eq!(record.segments.len(), 1);
    let video = &record.segments[0];
    assert_eq!(
        video.sampling,
        SegmentSampling::VideoHold {
            frame: TimeCode(12),
            source_fps: settings.fps
        }
    );
    assert_eq!(video.project, TimeCode(3)..TimeCode(33));
    assert_eq!(
        video.source,
        times(TimeCode(12)..TimeCode(13), settings.fps).unwrap()
    );
    assert_eq!(
        video.output,
        times(TimeCode(3)..TimeCode(27), settings.fps).unwrap()
    );
}

fn stored_transcript(stream: u32, end_micros: u64) -> kinewright_media::StoredSourceTranscript {
    kinewright_media::StoredSourceTranscript {
        audio_stream: stream,
        recogniser_key: "fixture".into(),
        words: vec![
            word("hello", 100_000, 400_000),
            word("there", 500_000, end_micros),
        ],
    }
}

fn loaded_speech(
    document: &Document,
    read: impl Fn(
        &str,
        Rational,
        TimeCode,
    ) -> Result<Option<kinewright_media::StoredSourceTranscript>, MediaError>,
) -> ReviewExpectations {
    let manifest = manifest(document);
    let sources = load_source_transcripts(document, &manifest, read).unwrap();
    derive(document, &manifest, &sources.review(), &annotations()).unwrap()
}

/// The store's transcript binds only to its source content, the selected
/// audio stream and the source clock. Anything else is left out, so the
/// speech stays undetermined instead of the review being refused.
#[test]
fn output_review_stored_transcripts_bind_to_source_stream_and_clock() {
    let document = speech_document();
    let content = document.media_pool[0]
        .source_fingerprint
        .content_sha256
        .clone()
        .unwrap();
    let kept = loaded_speech(&document, |sha, _, _| {
        Ok((sha == content).then(|| stored_transcript(1, 800_000)))
    });
    assert!(kept.kept_words.is_determined(), "TRANSCRIPT_KEPT_BINDS");
    assert_eq!(kept.kept_words.known().count(), 2);
    assert_eq!(kept.transcripts.len(), 1);
    assert_eq!(kept.transcripts[0].audio_stream, 1);

    let missing = loaded_speech(&document, |_, _, _| Ok(None));
    assert!(
        !missing.kept_words.is_determined(),
        "TRANSCRIPT_MISSING_NOT_CHECKED"
    );
    assert!(!missing.uncovered_speech_segments.is_empty());

    let other_stream = loaded_speech(&document, |_, _, _| Ok(Some(stored_transcript(2, 800_000))));
    assert!(
        !other_stream.kept_words.is_determined(),
        "TRANSCRIPT_STREAM_REFUSED"
    );
    assert!(other_stream.transcripts.is_empty());

    let past_end = loaded_speech(&document, |_, _, _| {
        Ok(Some(stored_transcript(1, 3_600_000_000)))
    });
    assert!(
        !past_end.kept_words.is_determined(),
        "TRANSCRIPT_TIMING_REFUSED"
    );
    assert!(past_end.transcripts.is_empty());

    let failed = loaded_speech(&document, |_, _, _| Err(MediaError::Cancelled));
    assert!(
        !failed.kept_words.is_determined(),
        "TRANSCRIPT_STORE_FAILURE_NOT_CHECKED"
    );
}

/// No audible source: nothing to transcribe, and silence is proven.
#[test]
fn output_review_stored_transcripts_absent_audio_proves_no_speech() {
    let document = doc(
        vec![asset(1, MediaKind::Video)],
        vec![track(1, vec![clip(1, 1, 0, 0, 30)])],
    );
    let expectations = loaded_speech(&document, |_, _, _| Ok(None));
    assert!(
        expectations.kept_words.is_determined(),
        "NO_AUDIO_DETERMINED"
    );
    assert_eq!(expectations.kept_words.known().count(), 0);
}
