//! AD1 — loudness normalization as one typed, verifiable plan.
//!
//! AD0 measures a delivered file against an [`AudioDeliveryTarget`] and
//! reports the gain that would reach it. AD1 turns that number into the
//! operation both the person and the agent apply: one `UpsertAudioBus` that
//! routes the chosen tracks through a compressor (only when the peak leaves no
//! room for plain gain), a gain stage, and a limiter clamped under the
//! target's ceiling by a lossy-codec headroom. The recipe, the validation, and
//! the predict-and-correct loop live here so `kinewright-app` and
//! `kinewright-agent` cannot drift apart; only the measurement of a candidate
//! document is injected, because core cannot render audio.
//!
//! Nothing here applies anything. The caller receives the operation and the
//! prediction and decides.

use std::collections::BTreeSet;
use std::fmt::Display;
use std::ops::RangeInclusive;

use serde::{Deserialize, Serialize};

use crate::{
    AudioBus, AudioBusId, AudioDeliveryMeasurement, AudioDeliveryTarget, AudioQcReport, Document,
    Effect, EffectId, Operation, ParamValue, TrackId, TrackKind, apply_batch, hundredths_to_string,
    measure_audio_qc,
};

/// The name of the bus every normalization plan inserts.
pub const NORMALIZATION_BUS_NAME: &str = "Delivery normalization";

/// How far under the target's peak ceiling the limiter clamps, in hundredths
/// of a dB. AAC and other lossy encoders overshoot the samples they were given
/// (M40 measured it on the first loudness recovery), so the processing ceiling
/// sits 2 dB under the delivery ceiling and the decoded file is what AD0
/// judges.
pub const LOSSY_CODEC_PEAK_HEADROOM_HUNDREDTHS: i32 = 200;

/// The bus gain a plan may request, in hundredths of a dB.
pub const NORMALIZATION_GAIN_RANGE_HUNDREDTHS: RangeInclusive<i32> = -6_000..=3_600;

/// The most predict-and-correct rounds a plan runs before it reports what it
/// reached.
pub const NORMALIZATION_MAX_ROUNDS: u8 = 4;

/// The smallest tolerance a plan converges to; a target that asks for less is
/// widened to this so the loop cannot chase measurement noise.
pub const NORMALIZATION_MIN_TOLERANCE_HUNDREDTHS: u16 = 25;

/// The makeup gain the compressor stage carries before plain gain takes over.
const COMPRESSOR_MAKEUP_LIMIT_HUNDREDTHS: i32 = 2_400;

/// Why a normalization plan could not be made. Each variant has a stable
/// `code` so the agent surface and the app can publish the same refusal.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, thiserror::Error,
)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum AudioNormalizationError {
    #[error("the target has no integrated loudness bound, so there is nothing to normalize to")]
    TargetGatesNothing,
    #[error("track_ids must contain at least one audio source track")]
    NoTracks,
    #[error("track_ids must not contain duplicates")]
    DuplicateTracks,
    #[error("track {track} does not exist")]
    MissingTrack { track: TrackId },
    #[error("track {track} is not an audio track")]
    NotAnAudioTrack { track: TrackId },
    #[error("track {track} contains no audio source clips")]
    EmptyTrack { track: TrackId },
    #[error(
        "track selection already intersects audio bus {bus} ({name}); remove or deliberately revise that mix before normalizing"
    )]
    TrackAlreadyMixed { bus: AudioBusId, name: String },
    #[error("timeline audio is silent; normalization cannot infer a programme level")]
    Silent,
    #[error("timeline audio has no measurable peak")]
    NoPeak,
    #[error(
        "required normalization gain {gain_hundredths_db} hundredths dB exceeds the supported {}..={} range",
        NORMALIZATION_GAIN_RANGE_HUNDREDTHS.start(),
        NORMALIZATION_GAIN_RANGE_HUNDREDTHS.end()
    )]
    GainOutOfRange { gain_hundredths_db: i32 },
    #[error("could not measure timeline audio: {reason}")]
    Measurement { reason: String },
    #[error("normalization processing is not applicable to this document: {reason}")]
    NotApplicable { reason: String },
    #[error(
        "normalization could not satisfy the delivery target: predicted {predicted_lufs} LUFS, predicted peak {predicted_peak_dbtp} dBTP"
    )]
    OutOfTolerance {
        predicted_lufs: String,
        predicted_peak_dbtp: String,
    },
}

impl AudioNormalizationError {
    /// The stable wire code, the same string serde writes in `code`.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::TargetGatesNothing => "target_gates_nothing",
            Self::NoTracks => "no_tracks",
            Self::DuplicateTracks => "duplicate_tracks",
            Self::MissingTrack { .. } => "missing_track",
            Self::NotAnAudioTrack { .. } => "not_an_audio_track",
            Self::EmptyTrack { .. } => "empty_track",
            Self::TrackAlreadyMixed { .. } => "track_already_mixed",
            Self::Silent => "silent",
            Self::NoPeak => "no_peak",
            Self::GainOutOfRange { .. } => "gain_out_of_range",
            Self::Measurement { .. } => "measurement",
            Self::NotApplicable { .. } => "not_applicable",
            Self::OutOfTolerance { .. } => "out_of_tolerance",
        }
    }
}

/// One normalization proposal: the operation and the evidence for it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AudioNormalizationPlan {
    /// Exactly one `UpsertAudioBus` inserting [`NORMALIZATION_BUS_NAME`].
    pub operation: Operation,
    pub bus_id: AudioBusId,
    pub tracks: Vec<TrackId>,
    pub target: AudioDeliveryTarget,
    /// The mix as measured before any change.
    pub current: AudioDeliveryMeasurement,
    /// The mix as measured with the operation applied to a scratch copy.
    pub predicted: AudioDeliveryMeasurement,
    /// The bus gain the final round requested, hundredths of a dB.
    pub gain_hundredths_db: i32,
    /// The sample ceiling the limiter clamps at: the target's true-peak
    /// ceiling minus [`LOSSY_CODEC_PEAK_HEADROOM_HUNDREDTHS`].
    pub processing_ceiling_dbfs_hundredths: i32,
    /// Predict-and-correct rounds spent, `1..=NORMALIZATION_MAX_ROUNDS`.
    pub rounds: u8,
    /// The AD0 judgement of the prediction against the target.
    pub predicted_qc: AudioQcReport,
}

/// The tracks a person-path normalization addresses when nobody chose: every
/// audio track that carries clips and is not already routed through a bus.
#[must_use]
pub fn audio_tracks_for_normalization(document: &Document) -> Vec<TrackId> {
    document
        .tracks
        .iter()
        .filter(|track| track.kind == TrackKind::Audio && !track.clips.is_empty())
        .filter(|track| {
            !document
                .audio_mix
                .buses
                .iter()
                .any(|bus| bus.tracks.contains(&track.id))
        })
        .map(|track| track.id)
        .collect()
}

struct Allocation {
    tracks: Vec<TrackId>,
    bus_id: AudioBusId,
    first_effect_id: u64,
}

fn validate(
    document: &Document,
    tracks: &[TrackId],
) -> Result<Allocation, AudioNormalizationError> {
    if tracks.is_empty() {
        return Err(AudioNormalizationError::NoTracks);
    }
    let unique = tracks.iter().copied().collect::<BTreeSet<_>>();
    if unique.len() != tracks.len() {
        return Err(AudioNormalizationError::DuplicateTracks);
    }
    for track in &unique {
        let candidate = document
            .tracks
            .iter()
            .find(|candidate| candidate.id == *track)
            .ok_or(AudioNormalizationError::MissingTrack { track: *track })?;
        if candidate.kind != TrackKind::Audio {
            return Err(AudioNormalizationError::NotAnAudioTrack { track: *track });
        }
        if candidate.clips.is_empty() {
            return Err(AudioNormalizationError::EmptyTrack { track: *track });
        }
    }
    if let Some(bus) = document
        .audio_mix
        .buses
        .iter()
        .find(|bus| bus.tracks.iter().any(|track| unique.contains(track)))
    {
        return Err(AudioNormalizationError::TrackAlreadyMixed {
            bus: bus.id,
            name: bus.name.clone(),
        });
    }
    let bus_id = AudioBusId(
        document
            .audio_mix
            .buses
            .iter()
            .map(|bus| bus.id.0)
            .max()
            .unwrap_or(0)
            .saturating_add(1),
    );
    let first_effect_id = document
        .tracks
        .iter()
        .flat_map(|track| &track.clips)
        .flat_map(|clip| &clip.effects)
        .chain(document.audio_mix.buses.iter().flat_map(|bus| &bus.effects))
        .map(|effect| effect.id.0)
        .max()
        .unwrap_or(0)
        .saturating_add(1);
    Ok(Allocation {
        tracks: unique.into_iter().collect(),
        bus_id,
        first_effect_id,
    })
}

fn round_hundredths_to_tenths(value: i32) -> i64 {
    i64::from(if value >= 0 {
        value.saturating_add(5) / 10
    } else {
        value.saturating_sub(5) / 10
    })
}

fn static_audio_effect(id: EffectId, name: &str, parameters: &[(&str, i64)]) -> Effect {
    Effect {
        id,
        name: name.to_owned(),
        parameters: parameters
            .iter()
            .map(|(name, value)| ((*name).to_owned(), ParamValue::Integer(*value)))
            .collect(),
        keyframes: std::collections::BTreeMap::new(),
    }
}

/// The bus one plan inserts. Pure: the same inputs always build the same bus.
///
/// # Errors
///
/// [`AudioNormalizationError::GainOutOfRange`] when the requested gain leaves
/// [`NORMALIZATION_GAIN_RANGE_HUNDREDTHS`].
pub fn normalization_bus(
    bus_id: AudioBusId,
    first_effect_id: u64,
    tracks: Vec<TrackId>,
    gain_hundredths: i32,
    measured_peak_hundredths: i32,
    ceiling_hundredths: i32,
) -> Result<AudioBus, AudioNormalizationError> {
    if !NORMALIZATION_GAIN_RANGE_HUNDREDTHS.contains(&gain_hundredths) {
        return Err(AudioNormalizationError::GainOutOfRange {
            gain_hundredths_db: gain_hundredths,
        });
    }
    let mut effects = Vec::new();
    let mut next_effect_id = first_effect_id;
    if gain_hundredths >= 0 {
        let makeup_hundredths = gain_hundredths.min(COMPRESSOR_MAKEUP_LIMIT_HUNDREDTHS);
        let post_gain_hundredths = gain_hundredths.saturating_sub(makeup_hundredths);
        let compression_required =
            measured_peak_hundredths.saturating_add(gain_hundredths) > ceiling_hundredths;
        let (threshold_tenth_db, ratio_hundredths) = if compression_required {
            let numerator = i64::from(ceiling_hundredths)
                .saturating_sub(i64::from(gain_hundredths))
                .saturating_sub(i64::from(measured_peak_hundredths).div_euclid(4));
            let threshold_hundredths = numerator.saturating_mul(4).div_euclid(3).clamp(-6_000, 0);
            (threshold_hundredths.div_euclid(10), 400)
        } else {
            (0, 100)
        };
        effects.push(static_audio_effect(
            EffectId(next_effect_id),
            "audio_compressor",
            &[
                ("threshold_tenth_db", threshold_tenth_db),
                ("ratio_hundredths", ratio_hundredths),
                ("attack_milliseconds", 5),
                ("release_milliseconds", 200),
                (
                    "makeup_gain_tenth_db",
                    round_hundredths_to_tenths(makeup_hundredths),
                ),
            ],
        ));
        next_effect_id = next_effect_id.saturating_add(1);
        if post_gain_hundredths > 0 {
            effects.push(static_audio_effect(
                EffectId(next_effect_id),
                "audio_gain",
                &[(
                    "gain_tenth_db",
                    round_hundredths_to_tenths(post_gain_hundredths),
                )],
            ));
            next_effect_id = next_effect_id.saturating_add(1);
        }
    } else {
        effects.push(static_audio_effect(
            EffectId(next_effect_id),
            "audio_gain",
            &[("gain_tenth_db", round_hundredths_to_tenths(gain_hundredths))],
        ));
        next_effect_id = next_effect_id.saturating_add(1);
    }
    effects.push(static_audio_effect(
        EffectId(next_effect_id),
        "audio_limiter",
        &[(
            "ceiling_tenth_db",
            i64::from(ceiling_hundredths).div_euclid(10),
        )],
    ));
    Ok(AudioBus {
        id: bus_id,
        name: NORMALIZATION_BUS_NAME.to_owned(),
        tracks,
        effects,
        ducking_sidechain_tracks: Vec::new(),
    })
}

/// The peak AD1 gates a prediction on: the true peak when the measurement has
/// one, else the sample peak.
fn peak_hundredths(measurement: &AudioDeliveryMeasurement) -> Option<i32> {
    measurement
        .true_peak_dbtp_hundredths
        .or(measurement.loudness.sample_peak_dbfs_hundredths)
}

/// Plan a normalization of `tracks` onto `target`.
///
/// `measure` renders and measures a document's mix; it is called once for the
/// current document and once per predict-and-correct round on a scratch copy
/// with the candidate operation applied. The loop requests
/// `target − current`, measures, corrects by the residual, and stops when the
/// residual is inside the tolerance or after [`NORMALIZATION_MAX_ROUNDS`].
///
/// # Errors
///
/// Every refusal is an [`AudioNormalizationError`] with a stable code; the
/// target must carry an integrated bound, the tracks must be audio tracks with
/// clips that no bus already routes, the mix must not be silent, and the
/// prediction must land inside the tolerance under the ceiling.
pub fn plan_audio_normalization<E: Display>(
    document: &Document,
    tracks: &[TrackId],
    target: AudioDeliveryTarget,
    mut measure: impl FnMut(&Document) -> Result<AudioDeliveryMeasurement, E>,
) -> Result<AudioNormalizationPlan, AudioNormalizationError> {
    let Some(target_lufs) = target.integrated_lufs_hundredths else {
        return Err(AudioNormalizationError::TargetGatesNothing);
    };
    let allocation = validate(document, tracks)?;
    let tolerance = i32::from(
        target
            .tolerance_lu_hundredths
            .max(NORMALIZATION_MIN_TOLERANCE_HUNDREDTHS),
    );
    // A target with no peak ceiling still needs a limiter ceiling for the bus;
    // −1 dBTP is every streaming preset's number and a safe default.
    let ceiling = target.maximum_true_peak_dbtp_hundredths.unwrap_or(-100);
    let processing_ceiling = ceiling.saturating_sub(LOSSY_CODEC_PEAK_HEADROOM_HUNDREDTHS);

    let current = measure(document).map_err(|error| AudioNormalizationError::Measurement {
        reason: error.to_string(),
    })?;
    let current_lufs = current
        .loudness
        .integrated_lufs_hundredths
        .ok_or(AudioNormalizationError::Silent)?;
    let current_peak = current
        .loudness
        .sample_peak_dbfs_hundredths
        .ok_or(AudioNormalizationError::NoPeak)?;

    let mut requested_gain = target_lufs.saturating_sub(current_lufs);
    let mut rounds = 0_u8;
    let (operation, predicted, gain_hundredths_db) = loop {
        rounds += 1;
        let bus = normalization_bus(
            allocation.bus_id,
            allocation.first_effect_id,
            allocation.tracks.clone(),
            requested_gain,
            current_peak,
            processing_ceiling,
        )?;
        let operation = Operation::UpsertAudioBus { bus };
        let mut candidate = document.clone();
        apply_batch(&mut candidate, std::slice::from_ref(&operation)).map_err(|error| {
            AudioNormalizationError::NotApplicable {
                reason: error.to_string(),
            }
        })?;
        let predicted =
            measure(&candidate).map_err(|error| AudioNormalizationError::Measurement {
                reason: error.to_string(),
            })?;
        let predicted_lufs = predicted
            .loudness
            .integrated_lufs_hundredths
            .ok_or(AudioNormalizationError::Silent)?;
        let correction = target_lufs.saturating_sub(predicted_lufs);
        if correction.unsigned_abs() <= tolerance.unsigned_abs()
            || rounds >= NORMALIZATION_MAX_ROUNDS
        {
            break (operation, predicted, requested_gain);
        }
        requested_gain = requested_gain.saturating_add(correction);
    };
    let predicted_lufs = predicted
        .loudness
        .integrated_lufs_hundredths
        .ok_or(AudioNormalizationError::Silent)?;
    let predicted_peak = peak_hundredths(&predicted).ok_or(AudioNormalizationError::NoPeak)?;
    if predicted_lufs.abs_diff(target_lufs) > tolerance.unsigned_abs() || predicted_peak > ceiling {
        return Err(AudioNormalizationError::OutOfTolerance {
            predicted_lufs: hundredths_to_string(predicted_lufs),
            predicted_peak_dbtp: hundredths_to_string(predicted_peak),
        });
    }
    Ok(AudioNormalizationPlan {
        operation,
        bus_id: allocation.bus_id,
        tracks: allocation.tracks,
        target,
        current,
        predicted,
        gain_hundredths_db,
        processing_ceiling_dbfs_hundredths: processing_ceiling,
        rounds,
        predicted_qc: measure_audio_qc(target, predicted),
    })
}

#[cfg(test)]
mod tests {
    use crate::{
        AssetId, AudioDeliveryPreset, AudioLoudness, Clip, ClipContent, MediaAsset, MediaKind,
        Rational, TimeCode, Track,
    };

    use super::*;

    fn document() -> Document {
        let fps = Rational::new(25, 1).unwrap();
        let asset = MediaAsset {
            id: AssetId(1),
            path: "voice.wav".into(),
            name: "voice".to_owned(),
            duration: TimeCode(250),
            fps,
            kind: MediaKind::Audio,
            resolution: None,
            source_fingerprint: crate::MediaSourceFingerprint::default(),
            color_description: crate::ColorContext::sdr_rec709().delivery,
        };
        let clip = Clip {
            id: crate::ClipId(1),
            asset: asset.id,
            source_range: TimeCode(0)..TimeCode(250),
            content: ClipContent::Media,
            timeline_start: TimeCode::ZERO,
            effects: Vec::new(),
            transition_in: None,
            link: None,
            audio_gain_tenth_db: 0,
            audio_fade_in_frames: TimeCode::ZERO,
            audio_fade_out_frames: TimeCode::ZERO,
            speed_percent: 100,
        };
        Document {
            tracks: vec![
                Track {
                    id: TrackId(1),
                    kind: TrackKind::Video,
                    sync_lock: true,
                    clips: Vec::new(),
                },
                Track {
                    id: TrackId(2),
                    kind: TrackKind::Audio,
                    sync_lock: true,
                    clips: vec![clip],
                },
                Track {
                    id: TrackId(3),
                    kind: TrackKind::Audio,
                    sync_lock: true,
                    clips: Vec::new(),
                },
            ],
            media_pool: vec![asset],
            duration: TimeCode(250),
            fps,
            ..Document::default()
        }
    }

    /// A measurement model: the mix reads `base` LUFS plus whatever bus gain
    /// the document carries, and the peak follows the gain until the limiter
    /// clamps it.
    fn linear_model(
        base_lufs: i32,
        base_peak: i32,
    ) -> impl FnMut(&Document) -> Result<AudioDeliveryMeasurement, String> {
        move |document: &Document| {
            let gain = document
                .audio_mix
                .buses
                .iter()
                .flat_map(|bus| &bus.effects)
                .map(|effect| match effect.name.as_str() {
                    "audio_gain" => effect
                        .parameters
                        .get("gain_tenth_db")
                        .and_then(|value| match value {
                            ParamValue::Integer(v) => Some(*v * 10),
                            _ => None,
                        })
                        .unwrap_or(0),
                    "audio_compressor" => effect
                        .parameters
                        .get("makeup_gain_tenth_db")
                        .and_then(|value| match value {
                            ParamValue::Integer(v) => Some(*v * 10),
                            _ => None,
                        })
                        .unwrap_or(0),
                    _ => 0,
                })
                .sum::<i64>();
            let ceiling = document
                .audio_mix
                .buses
                .iter()
                .flat_map(|bus| &bus.effects)
                .find(|effect| effect.name == "audio_limiter")
                .and_then(|effect| match effect.parameters.get("ceiling_tenth_db") {
                    Some(ParamValue::Integer(v)) => Some(*v * 10),
                    _ => None,
                });
            let gain = i32::try_from(gain).unwrap();
            let peak = base_peak + gain;
            let peak = ceiling.map_or(peak, |ceiling| peak.min(i32::try_from(ceiling).unwrap()));
            Ok(AudioDeliveryMeasurement {
                loudness: AudioLoudness {
                    integrated_lufs_hundredths: Some(base_lufs + gain),
                    sample_peak_dbfs_hundredths: Some(peak),
                    sample_rate: 48_000,
                    channels: 2,
                    sample_frames: 480_000,
                },
                true_peak_dbtp_hundredths: Some(peak + 20),
                loudness_range_lu_hundredths: None,
            })
        }
    }

    #[test]
    fn the_m40_event_cut_normalizes_to_streaming_in_one_round() {
        let document = document();
        let plan = plan_audio_normalization(
            &document,
            &[TrackId(2)],
            AudioDeliveryPreset::Streaming.target(),
            linear_model(-3_990, -2_500),
        )
        .unwrap();
        assert_eq!(plan.gain_hundredths_db, 2_590);
        assert_eq!(plan.rounds, 1);
        assert_eq!(plan.bus_id, AudioBusId(1));
        assert_eq!(plan.tracks, vec![TrackId(2)]);
        assert_eq!(plan.processing_ceiling_dbfs_hundredths, -300);
        assert_eq!(
            plan.predicted.loudness.integrated_lufs_hundredths,
            Some(-1_400)
        );
        assert!(
            plan.predicted_qc.technical_pass,
            "{:?}",
            plan.predicted_qc.exceptions
        );
        let Operation::UpsertAudioBus { bus } = &plan.operation else {
            panic!("expected UpsertAudioBus");
        };
        assert_eq!(bus.name, NORMALIZATION_BUS_NAME);
        let names = bus
            .effects
            .iter()
            .map(|e| e.name.as_str())
            .collect::<Vec<_>>();
        // +25.90 dB: 24 dB of compressor makeup, 1.90 dB of gain, then the limiter.
        assert_eq!(names, ["audio_compressor", "audio_gain", "audio_limiter"]);
        // −25 dBFS + 25.9 dB would land at +0.9 dBFS, over a −3 dBFS ceiling: compression engaged.
        assert_eq!(
            bus.effects[0].parameters.get("ratio_hundredths"),
            Some(&ParamValue::Integer(400))
        );
        assert_eq!(
            bus.effects[2].parameters.get("ceiling_tenth_db"),
            Some(&ParamValue::Integer(-30))
        );
    }

    #[test]
    fn a_loud_mix_gets_plain_negative_gain_and_the_limiter() {
        let plan = plan_audio_normalization(
            &document(),
            &[TrackId(2)],
            AudioDeliveryPreset::BroadcastEbuR128.target(),
            linear_model(-1_400, -100),
        )
        .unwrap();
        assert_eq!(plan.gain_hundredths_db, -900);
        let Operation::UpsertAudioBus { bus } = &plan.operation else {
            panic!("expected UpsertAudioBus");
        };
        let names = bus
            .effects
            .iter()
            .map(|e| e.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(names, ["audio_gain", "audio_limiter"]);
        assert_eq!(
            bus.effects[0].parameters.get("gain_tenth_db"),
            Some(&ParamValue::Integer(-90))
        );
    }

    #[test]
    fn the_loop_corrects_a_model_whose_gain_only_half_lands() {
        // A mix that responds to bus gain with half the requested change:
        // the first round undershoots, the second corrects onto the target.
        let mut measure = linear_model(-2_400, -1_200);
        let half = move |document: &Document| {
            measure(document).map(|mut m| {
                let full = m.loudness.integrated_lufs_hundredths.unwrap();
                let half = i32::midpoint(-2_400, full);
                m.loudness.integrated_lufs_hundredths = Some(half);
                m
            })
        };
        let plan = plan_audio_normalization(
            &document(),
            &[TrackId(2)],
            AudioDeliveryPreset::Podcast.target(),
            half,
        )
        .unwrap();
        assert!(plan.rounds >= 2, "rounds {}", plan.rounds);
        let predicted = plan.predicted.loudness.integrated_lufs_hundredths.unwrap();
        assert!((predicted - -1_600).abs() <= 100, "predicted {predicted}");
    }

    #[test]
    fn every_refusal_is_typed() {
        let document = document();
        let target = AudioDeliveryPreset::Streaming.target();
        let model = || linear_model(-2_000, -600);
        assert_eq!(
            plan_audio_normalization(
                &document,
                &[TrackId(2)],
                AudioDeliveryPreset::MeasureOnly.target(),
                model()
            )
            .unwrap_err()
            .code(),
            "target_gates_nothing"
        );
        assert_eq!(
            plan_audio_normalization(&document, &[], target, model())
                .unwrap_err()
                .code(),
            "no_tracks"
        );
        assert_eq!(
            plan_audio_normalization(&document, &[TrackId(2), TrackId(2)], target, model())
                .unwrap_err()
                .code(),
            "duplicate_tracks"
        );
        assert_eq!(
            plan_audio_normalization(&document, &[TrackId(9)], target, model())
                .unwrap_err()
                .code(),
            "missing_track"
        );
        assert_eq!(
            plan_audio_normalization(&document, &[TrackId(1)], target, model())
                .unwrap_err()
                .code(),
            "not_an_audio_track"
        );
        assert_eq!(
            plan_audio_normalization(&document, &[TrackId(3)], target, model())
                .unwrap_err()
                .code(),
            "empty_track"
        );
        let silent = |_: &Document| {
            Ok::<_, String>(AudioDeliveryMeasurement {
                loudness: AudioLoudness {
                    integrated_lufs_hundredths: None,
                    sample_peak_dbfs_hundredths: None,
                    sample_rate: 48_000,
                    channels: 2,
                    sample_frames: 0,
                },
                true_peak_dbtp_hundredths: None,
                loudness_range_lu_hundredths: None,
            })
        };
        assert_eq!(
            plan_audio_normalization(&document, &[TrackId(2)], target, silent)
                .unwrap_err()
                .code(),
            "silent"
        );
        let failing = |_: &Document| Err::<AudioDeliveryMeasurement, _>("no engine".to_owned());
        assert_eq!(
            plan_audio_normalization(&document, &[TrackId(2)], target, failing)
                .unwrap_err()
                .code(),
            "measurement"
        );
        // A mix 50 dB under the target asks for more gain than the bus allows.
        assert_eq!(
            plan_audio_normalization(
                &document,
                &[TrackId(2)],
                target,
                linear_model(-6_400, -5_000)
            )
            .unwrap_err()
            .code(),
            "gain_out_of_range"
        );
    }

    #[test]
    fn a_track_already_on_a_bus_is_refused_by_name() {
        let mut document = document();
        document.audio_mix.buses.push(AudioBus {
            id: AudioBusId(7),
            name: "Music".to_owned(),
            tracks: vec![TrackId(2)],
            effects: Vec::new(),
            ducking_sidechain_tracks: Vec::new(),
        });
        let error = plan_audio_normalization(
            &document,
            &[TrackId(2)],
            AudioDeliveryPreset::Streaming.target(),
            linear_model(-2_000, -600),
        )
        .unwrap_err();
        assert_eq!(
            error,
            AudioNormalizationError::TrackAlreadyMixed {
                bus: AudioBusId(7),
                name: "Music".to_owned()
            }
        );
        assert!(audio_tracks_for_normalization(&document).is_empty());
    }

    #[test]
    fn the_person_path_default_is_every_unmixed_audio_track_with_clips() {
        let document = document();
        assert_eq!(audio_tracks_for_normalization(&document), vec![TrackId(2)]);
    }

    #[test]
    fn the_plan_and_its_error_round_trip_as_json() {
        let plan = plan_audio_normalization(
            &document(),
            &[TrackId(2)],
            AudioDeliveryPreset::Streaming.target(),
            linear_model(-3_990, -2_500),
        )
        .unwrap();
        let json = serde_json::to_value(&plan).unwrap();
        assert_eq!(json["gain_hundredths_db"], 2_590);
        let back: AudioNormalizationPlan = serde_json::from_value(json).unwrap();
        assert_eq!(back, plan);
        let error = serde_json::to_value(AudioNormalizationError::EmptyTrack { track: TrackId(3) })
            .unwrap();
        assert_eq!(error["code"], "empty_track");
    }
}
