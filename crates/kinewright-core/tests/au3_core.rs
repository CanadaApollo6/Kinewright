//! AU3 loudness and delivery — core contract tests (AU3 §7 items A1 to A4).

use std::sync::Arc;

use crossbeam_channel::Receiver;
use kinewright_core::{
    AUDIO_QC_CHANNEL_BALANCE_CLAMP_LU_HUNDREDTHS, AUDIO_QC_CHANNEL_IMBALANCE_LU_HUNDREDTHS,
    AUDIO_QC_CLIPPED_RUN_SAMPLES, AUDIO_QC_ENGINE, AUDIO_QC_SILENCE_DBFS_HUNDREDTHS,
    AUDIO_QC_SILENCE_INFO_MILLISECONDS, AUDIO_QC_SILENCE_WINDOW_MILLISECONDS, AssetId,
    AudioChannelClipping, AudioClipping, AudioLoudness, AudioMix, AudioQcException,
    AudioQcMeasurements, AudioQcProvenance, AudioQcRequest, ColorContext, ColorDescription,
    DeliveryProfile, Document, EBU_R128_PROGRAMME_TARGET, FrameTexture,
    LOSSY_CODEC_TRUE_PEAK_HEADROOM_HUNDREDTHS, LoudnessSnapshot, LoudnessTarget, MediaAsset,
    MediaCatalog, MediaError, MediaEvent, MediaKind, MediaSourceFingerprint, Operation, Playback,
    QaSeverity, Rational, STREAMING_PLATFORM_TARGET, TimeCode, Track, TrackId, TrackKind,
    audio_qc_exceptions, audio_qc_technical_pass, loudness_target_exceptions, qa_document,
};

/// The pre-AU3 five-key measurement, exactly as `AudioLoudness` serialized
/// before this slice.
const PRE_AU3_LOUDNESS_JSON: &str = r#"{"integrated_lufs_hundredths":-1600,"sample_peak_dbfs_hundredths":-99,"sample_rate":48000,"channels":2,"sample_frames":4800}"#;

const fn loudness(integrated: Option<i32>, channels: u16) -> AudioLoudness {
    AudioLoudness {
        integrated_lufs_hundredths: integrated,
        sample_peak_dbfs_hundredths: Some(-300),
        sample_rate: 48_000,
        channels,
        sample_frames: 480_000,
        momentary_max_lufs_hundredths: None,
        short_term_max_lufs_hundredths: None,
        loudness_range_lu_hundredths: None,
        true_peak_dbtp_hundredths: Some(-250),
    }
}

/// A measurement that raises nothing: on target, in balance, unclipped, and
/// with no silence run over the 1 000 ms rule.
fn quiet_measurements(target: Option<LoudnessTarget>) -> AudioQcMeasurements {
    AudioQcMeasurements {
        master: loudness(Some(-1_400), 2),
        channel_balance_lu_hundredths: Some(0),
        clipping: AudioClipping::default(),
        leading_silence_frames: TimeCode::ZERO,
        leading_silence_milliseconds: 0,
        trailing_silence_frames: TimeCode::ZERO,
        trailing_silence_milliseconds: 0,
        target,
    }
}

fn codes(exceptions: &[AudioQcException]) -> Vec<&str> {
    exceptions
        .iter()
        .map(|exception| exception.code.as_str())
        .collect()
}

fn empty_timeline(fps: Rational) -> Document {
    Document {
        catalog: MediaCatalog::default(),
        audio_mix: AudioMix::default(),
        color_context: ColorContext::default(),
        lut_assets: Vec::new(),
        tracks: vec![Track {
            id: TrackId(1),
            kind: TrackKind::Video,
            sync_lock: true,
            clips: Vec::new(),
        }],
        media_pool: Vec::new(),
        markers: Vec::new(),
        fps,
        resolution: (1_920, 1_080),
        duration: TimeCode::ZERO,
    }
}

fn asset(id: u64, fps: Rational, kind: MediaKind) -> MediaAsset {
    MediaAsset {
        id: AssetId(id),
        path: std::path::PathBuf::from(format!("asset-{id}.mp4")),
        name: format!("asset-{id}"),
        duration: TimeCode(300),
        fps,
        kind,
        resolution: Some((1_920, 1_080)),
        source_fingerprint: MediaSourceFingerprint::default(),
        color_description: ColorDescription::default(),
    }
}

/// A video track carrying a video-only clip and an audio track carrying an
/// audio-video clip: exactly one audio-bearing track.
fn document_with_one_audio_track() -> Document {
    let fps = Rational::new(30, 1).unwrap();
    let mut doc = empty_timeline(fps);
    Operation::AddTrack {
        track: Track {
            id: TrackId(2),
            kind: TrackKind::Audio,
            sync_lock: true,
            clips: Vec::new(),
        },
    }
    .apply(&mut doc)
    .unwrap();
    for (id, kind, track) in [
        (1, MediaKind::Video, TrackId(1)),
        (2, MediaKind::AudioVideo, TrackId(2)),
    ] {
        Operation::AddAsset {
            asset: asset(id, fps, kind),
        }
        .apply(&mut doc)
        .unwrap();
        Operation::AddClip {
            track,
            asset: AssetId(id),
            at: TimeCode::ZERO,
            source: TimeCode(0)..TimeCode(30),
        }
        .apply(&mut doc)
        .unwrap();
    }
    assert!(doc.duration > TimeCode::ZERO);
    doc
}

const fn set_mix(track: u64, mute: bool, solo: bool) -> Operation {
    Operation::SetTrackMix {
        track: TrackId(track),
        gain_tenth_db: 0,
        pan_percent: 0,
        mute,
        solo,
    }
}

/// The `no_audible_media` message, if raised, plus the report's error count
/// and gate: the rule is Info, so mute and solo must never move either.
fn no_audible_media_message(doc: &Document) -> (Option<String>, usize, bool) {
    let report = qa_document(doc);
    let issues = report
        .issues
        .iter()
        .filter(|issue| issue.code == "no_audible_media")
        .collect::<Vec<_>>();
    assert!(issues.len() <= 1, "the issue is raised at most once");
    let message = issues.first().map(|issue| {
        assert_eq!(issue.severity, QaSeverity::Info);
        issue.message.clone()
    });
    (
        message,
        report.count(QaSeverity::Error),
        report.export_ready(),
    )
}

/// AU3 §7 item A1.
#[test]
fn audio_loudness_keeps_its_pre_au3_wire_shape_and_stays_copy_and_eq() {
    let decoded: AudioLoudness = serde_json::from_str(PRE_AU3_LOUDNESS_JSON).unwrap();
    assert_eq!(decoded.integrated_lufs_hundredths, Some(-1_600));
    assert_eq!(decoded.sample_peak_dbfs_hundredths, Some(-99));
    assert_eq!(decoded.momentary_max_lufs_hundredths, None);
    assert_eq!(decoded.short_term_max_lufs_hundredths, None);
    assert_eq!(decoded.loudness_range_lu_hundredths, None);
    assert_eq!(decoded.true_peak_dbtp_hundredths, None);
    assert_eq!(
        serde_json::to_string(&decoded).unwrap(),
        PRE_AU3_LOUDNESS_JSON
    );

    // `Copy`: the same value is used twice after a by-value move; `Eq`: the
    // two copies compare equal.
    let copied = decoded;
    let again = decoded;
    assert_eq!(copied, again);

    let mut measured = decoded;
    measured.momentary_max_lufs_hundredths = Some(-1_200);
    measured.short_term_max_lufs_hundredths = Some(-1_400);
    measured.loudness_range_lu_hundredths = Some(650);
    measured.true_peak_dbtp_hundredths = Some(-80);
    let encoded = serde_json::to_string(&measured).unwrap();
    assert!(encoded.contains(r#""momentary_max_lufs_hundredths":-1200"#));
    assert!(encoded.contains(r#""short_term_max_lufs_hundredths":-1400"#));
    assert!(encoded.contains(r#""loudness_range_lu_hundredths":650"#));
    assert!(encoded.contains(r#""true_peak_dbtp_hundredths":-80"#));
    assert_eq!(
        serde_json::from_str::<AudioLoudness>(&encoded).unwrap(),
        measured
    );

    // `AudioQcRequest` on the wire (au1_core.rs's `MixLevelRequest` template).
    let whole = serde_json::from_str::<AudioQcRequest>("{}").unwrap();
    assert_eq!(whole, AudioQcRequest::default());
    assert_eq!(serde_json::to_string(&whole).unwrap(), "{}");
    let windowed = AudioQcRequest {
        range: Some(TimeCode(30)..TimeCode(60)),
        profile: Some(DeliveryProfile::Youtube1080p),
    };
    // `DeliveryProfile` has serialized as `snake_case` of its variant name
    // since CC6 — `youtube1080p`, not `as_str()`'s `youtube_1080p` — and the
    // existing wire wins (AU3 §0 erratum).
    let windowed_json = r#"{"range":{"start":30,"end":60},"profile":"youtube1080p"}"#;
    assert_eq!(
        serde_json::from_str::<AudioQcRequest>(windowed_json).unwrap(),
        windowed
    );
    assert_eq!(serde_json::to_string(&windowed).unwrap(), windowed_json);

    // The typed short-range refusal renders its message and carries no
    // recovery code, like its spectrum sibling.
    let refusal = MediaError::MixLoudnessRangeTooShort {
        sample_frames: 8_000,
        required: 19_200,
    };
    assert_eq!(
        refusal.to_string(),
        "mix loudness needs at least 19200 sample frames; got 8000"
    );
    assert_eq!(refusal.recovery_code(), None);
}

/// AU3 §7 item A2. `loudness_target` must be usable in a `const` context.
const SOURCE_MASTER_TARGET: LoudnessTarget = DeliveryProfile::SourceMaster.loudness_target();

/// AU3 §7 item A2.
#[test]
fn every_profile_carries_its_loudness_target_as_a_const_fact() {
    assert_eq!(
        SOURCE_MASTER_TARGET,
        LoudnessTarget {
            integrated_lufs_hundredths: -2_300,
            tolerance_lu_hundredths: 100,
            true_peak_ceiling_dbtp_hundredths: -100,
            loudness_range_max_lu_hundredths: None,
        }
    );
    assert_eq!(SOURCE_MASTER_TARGET, EBU_R128_PROGRAMME_TARGET);
    for profile in [
        DeliveryProfile::Youtube1080p,
        DeliveryProfile::VerticalShort,
        DeliveryProfile::SquareSocial,
    ] {
        assert_eq!(
            profile.loudness_target(),
            LoudnessTarget {
                integrated_lufs_hundredths: -1_400,
                tolerance_lu_hundredths: 100,
                true_peak_ceiling_dbtp_hundredths: -100,
                loudness_range_max_lu_hundredths: None,
            },
            "{profile:?}"
        );
        assert_eq!(profile.loudness_target(), STREAMING_PLATFORM_TARGET);
    }
    assert_eq!(DeliveryProfile::ALL.len(), 4);
    assert_ne!(EBU_R128_PROGRAMME_TARGET, STREAMING_PLATFORM_TARGET);
    assert_eq!(LOSSY_CODEC_TRUE_PEAK_HEADROOM_HUNDREDTHS, 200);

    // The optional range maximum stays off the wire when absent.
    assert_eq!(
        serde_json::to_string(&EBU_R128_PROGRAMME_TARGET).unwrap(),
        r#"{"integrated_lufs_hundredths":-2300,"tolerance_lu_hundredths":100,"true_peak_ceiling_dbtp_hundredths":-100}"#
    );
}

/// AU3 §7 item A3.
#[test]
#[allow(clippy::too_many_lines)]
fn audio_qc_exceptions_raise_exactly_the_contract_table_in_order() {
    assert_eq!(AUDIO_QC_SILENCE_DBFS_HUNDREDTHS, -7_000);
    assert_eq!(AUDIO_QC_SILENCE_WINDOW_MILLISECONDS, 10);
    assert_eq!(AUDIO_QC_SILENCE_INFO_MILLISECONDS, 1_000);
    assert_eq!(AUDIO_QC_CLIPPED_RUN_SAMPLES, 3);
    assert_eq!(AUDIO_QC_CHANNEL_IMBALANCE_LU_HUNDREDTHS, 300);
    assert_eq!(AUDIO_QC_CHANNEL_BALANCE_CLAMP_LU_HUNDREDTHS, 6_000);

    // Everything at once, against a target that also carries a range maximum.
    let target = LoudnessTarget {
        loudness_range_max_lu_hundredths: Some(2_000),
        ..STREAMING_PLATFORM_TARGET
    };
    let mut master = loudness(Some(-2_000), 2);
    master.true_peak_dbtp_hundredths = Some(-50);
    master.loudness_range_lu_hundredths = Some(2_500);
    let everything = AudioQcMeasurements {
        master,
        channel_balance_lu_hundredths: Some(450),
        clipping: AudioClipping {
            left: AudioChannelClipping {
                over_full_scale_samples: 12,
                clipped_runs: 2,
                basis_points: 0,
            },
            right: AudioChannelClipping {
                over_full_scale_samples: 4,
                clipped_runs: 1,
                basis_points: 0,
            },
        },
        leading_silence_frames: TimeCode(45),
        leading_silence_milliseconds: 1_500,
        trailing_silence_frames: TimeCode(60),
        trailing_silence_milliseconds: 2_000,
        target: Some(target),
    };
    let exceptions = audio_qc_exceptions(&everything);
    let expected: [(&str, QaSeverity, &str, &str, &str); 8] = [
        (
            "audio_clipping",
            QaSeverity::Error,
            "clipping.left.clipped_runs",
            "2",
            "0",
        ),
        (
            "audio_clipping",
            QaSeverity::Error,
            "clipping.right.clipped_runs",
            "1",
            "0",
        ),
        (
            "audio_true_peak_over_ceiling",
            QaSeverity::Error,
            "true_peak_dbtp_hundredths",
            "-50",
            "<= -100",
        ),
        (
            "audio_channel_imbalance",
            QaSeverity::Warning,
            "channel_balance_lu_hundredths",
            "450",
            "-300..=300",
        ),
        (
            "audio_loudness_out_of_tolerance",
            QaSeverity::Warning,
            "integrated_lufs_hundredths",
            "-2000",
            "-1500..=-1300",
        ),
        (
            "audio_loudness_range_over_maximum",
            QaSeverity::Warning,
            "loudness_range_lu_hundredths",
            "2500",
            "<= 2000",
        ),
        (
            "audio_leading_silence",
            QaSeverity::Info,
            "leading_silence_frames",
            "45 frames (1500 ms)",
            "<= 1 s",
        ),
        (
            "audio_trailing_silence",
            QaSeverity::Info,
            "trailing_silence_frames",
            "60 frames (2000 ms)",
            "<= 1 s",
        ),
    ];
    assert_eq!(exceptions.len(), expected.len());
    for (exception, (code, severity, field, observed, allowed)) in exceptions.iter().zip(expected) {
        assert_eq!(exception.code, code);
        assert_eq!(exception.severity, severity, "{code}");
        assert_eq!(exception.field.as_deref(), Some(field), "{code}");
        assert_eq!(exception.observed.as_deref(), Some(observed), "{code}");
        assert_eq!(exception.allowed.as_deref(), Some(allowed), "{code}");
        assert!(!exception.message.is_empty(), "{code}");
    }
    assert!(!audio_qc_technical_pass(&exceptions));

    // The boundaries are inclusive: exactly on tolerance, ceiling, band, and
    // the 1 000 ms rule raises nothing.
    let mut on_the_line = quiet_measurements(Some(STREAMING_PLATFORM_TARGET));
    on_the_line.master.integrated_lufs_hundredths = Some(-1_500);
    on_the_line.master.true_peak_dbtp_hundredths = Some(-100);
    on_the_line.channel_balance_lu_hundredths = Some(-300);
    on_the_line.leading_silence_frames = TimeCode(30);
    on_the_line.leading_silence_milliseconds = 1_000;
    assert_eq!(audio_qc_exceptions(&on_the_line), Vec::new());
    assert!(audio_qc_technical_pass(&[]));

    // Silent: one Warning that leaves `technical_pass` true and suppresses
    // the target, balance, and silence entries.
    let mut silent = quiet_measurements(Some(STREAMING_PLATFORM_TARGET));
    silent.master.integrated_lufs_hundredths = None;
    silent.master.true_peak_dbtp_hundredths = Some(0);
    silent.channel_balance_lu_hundredths = None;
    silent.leading_silence_frames = TimeCode(300);
    silent.leading_silence_milliseconds = 10_000;
    silent.trailing_silence_frames = TimeCode(300);
    silent.trailing_silence_milliseconds = 10_000;
    let exceptions = audio_qc_exceptions(&silent);
    assert_eq!(codes(&exceptions), ["audio_silent"]);
    assert_eq!(exceptions[0].severity, QaSeverity::Warning);
    assert_eq!(
        exceptions[0].field.as_deref(),
        Some("integrated_lufs_hundredths")
    );
    assert_eq!(exceptions[0].observed.as_deref(), Some("none"));
    assert_eq!(exceptions[0].allowed.as_deref(), Some("> -7000"));
    assert!(audio_qc_technical_pass(&exceptions));

    // Δ `None` with a non-empty gate (integrated `Some`) on a stereo master:
    // one side is silent, and that is raised.
    let mut one_side = quiet_measurements(None);
    one_side.channel_balance_lu_hundredths = None;
    let exceptions = audio_qc_exceptions(&one_side);
    assert_eq!(codes(&exceptions), ["audio_channel_imbalance"]);
    assert_eq!(exceptions[0].severity, QaSeverity::Warning);
    assert_eq!(
        exceptions[0].observed.as_deref(),
        Some("none (one channel silent)")
    );
    assert_eq!(exceptions[0].allowed.as_deref(), Some("-300..=300"));

    // Δ `None` on a mono master raises nothing.
    one_side.master = loudness(Some(-1_400), 1);
    assert_eq!(audio_qc_exceptions(&one_side), Vec::new());

    // The shared target function under the delivery prefix (AU3 §5.3).
    let delivery = loudness_target_exceptions(&master, target, "delivery");
    assert_eq!(
        codes(&delivery),
        [
            "delivery_true_peak_over_ceiling",
            "delivery_loudness_out_of_tolerance",
            "delivery_loudness_range_over_maximum",
        ]
    );
    assert_eq!(
        loudness_target_exceptions(&loudness(None, 2), target, "delivery"),
        Vec::new()
    );
}

/// A `Playback` implementing only the required methods, so the AU3 defaults
/// are what a double that predates the slice observes.
struct MinimalPlayback;

impl Playback for MinimalPlayback {
    fn set_document(&self, _doc: Arc<Document>) {}
    fn request_frame(&self, _t: TimeCode) {}
    fn frames(&self) -> Receiver<(TimeCode, FrameTexture)> {
        crossbeam_channel::unbounded().1
    }
    fn events(&self) -> Receiver<MediaEvent> {
        crossbeam_channel::unbounded().1
    }
    fn play(&self, _from: TimeCode) {}
    fn pause(&self) {}
    fn seek(&self, _to: TimeCode) {}
    fn position(&self) -> TimeCode {
        TimeCode::ZERO
    }
    fn output_peaks(&self) -> [f32; 2] {
        [0.0, 0.0]
    }
}

/// AU3 §7 item A4.
#[test]
fn no_audible_media_names_mute_and_solo_and_the_defaults_hold() {
    const SILENCED: &str =
        "Every audio-bearing track is muted or silenced by another track's solo.";

    // Audible: the issue is not raised.
    let audible = document_with_one_audio_track();
    let (message, errors, export_ready) = no_audible_media_message(&audible);
    assert_eq!(message, None);

    // Muted only.
    let mut muted = audible.clone();
    set_mix(2, true, false).apply(&mut muted).unwrap();
    assert_eq!(
        no_audible_media_message(&muted),
        (Some(SILENCED.to_owned()), errors, export_ready),
        "mute never moves the gate"
    );

    // Silenced by another track's solo.
    let mut soloed = audible.clone();
    set_mix(1, false, true).apply(&mut soloed).unwrap();
    assert!(soloed.audio_mix.any_solo());
    assert!(!soloed.track_audible(TrackId(2)));
    assert_eq!(
        no_audible_media_message(&soloed),
        (Some(SILENCED.to_owned()), errors, export_ready),
        "solo never moves the gate"
    );

    // Soloing the audio-bearing track itself keeps it audible.
    let mut self_soloed = audible;
    set_mix(2, false, true).apply(&mut self_soloed).unwrap();
    assert_eq!(no_audible_media_message(&self_soloed).0, None);

    // Media-free: the original sentence.
    let fps = Rational::new(30, 1).unwrap();
    let mut video_only = empty_timeline(fps);
    Operation::AddAsset {
        asset: asset(1, fps, MediaKind::Video),
    }
    .apply(&mut video_only)
    .unwrap();
    Operation::AddClip {
        track: TrackId(1),
        asset: AssetId(1),
        at: TimeCode::ZERO,
        source: TimeCode(0)..TimeCode(30),
    }
    .apply(&mut video_only)
    .unwrap();
    assert_eq!(
        no_audible_media_message(&video_only).0.as_deref(),
        Some("The timeline has no real-time media clip with an audio stream.")
    );

    // `Playback` defaults: a snapshot of `None`s and a reset that does nothing.
    let playback = MinimalPlayback;
    assert_eq!(playback.loudness(), LoudnessSnapshot::default());
    assert_eq!(
        playback.loudness(),
        LoudnessSnapshot {
            momentary_lufs_hundredths: None,
            short_term_lufs_hundredths: None,
            integrated_lufs_hundredths: None,
            loudness_range_lu_hundredths: None,
            true_peak_dbtp_hundredths: None,
            programme_seconds: 0,
        }
    );
    playback.reset_loudness();
    assert_eq!(playback.loudness(), LoudnessSnapshot::default());

    // The eight provenance strings.
    let provenance = AudioQcProvenance::default();
    assert_eq!(provenance.engine, AUDIO_QC_ENGINE);
    assert_eq!(provenance.engine, "kinewright_audio_qc_v1");
    assert_eq!(provenance.measurement_rate, 48_000);
    assert_eq!(provenance.k_weighting, "bs1770_4_bilinear_from_prototypes");
    assert_eq!(provenance.true_peak, "8x_polyphase_256_tap_blackman_harris");
    assert_eq!(
        provenance.true_peak_bias,
        "exact_on_grid_at_most_-0.042_db_between_grid_points"
    );
    assert_eq!(provenance.gate, "bs1770_4_two_stage_complete_blocks");
    assert_eq!(provenance.loudness_range, "ebu_tech_3342_nearest_rank");
    assert_eq!(provenance.silence, "10ms_rms_-70_dbfs_post_clamp");
    assert_eq!(
        provenance.exception_order,
        "severity_desc_code_asc_field_asc"
    );
}
