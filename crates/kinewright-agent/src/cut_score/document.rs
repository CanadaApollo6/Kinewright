// Derived from Scarlet evals/src/timeline.ts at 2cbab542; see LICENSES/scarlet-MIT.txt.
use super::{Candidate, CannotScore, SourceRange, valid_hash, validate_ranges};
use kinewright_core::{AssetId, ClipContent, Document, MediaKind, Rational, TimeCode, TrackId};

fn source_micros(frame: TimeCode, fps: Rational) -> Result<u64, CannotScore> {
    if frame.0 < 0 || !fps.is_valid() {
        return Err(CannotScore::TimeOverflow);
    }
    let n = u128::try_from(frame.0).map_err(|_| CannotScore::TimeOverflow)?
        * 1_000_000
        * u128::from(fps.denominator());
    let d = u128::from(fps.numerator());
    u64::try_from((n + d / 2) / d).map_err(|_| CannotScore::TimeOverflow)
}

/// Snapshot one named asset's contiguous, enabled, speed-one dialogue track.
/// Does not read media or treat the project-file digest as recording identity.
/// Other tracks are outside the selected dialogue score, not acoustic evidence.
/// The caller names the dialogue track; its kind is not otherwise restricted.
///
/// # Errors
/// Refuses invalid documents, unverified sources, speed changes, mixed sources,
/// gaps, overlaps, mute/automation/processing, and inexact source/project spans.
pub fn candidate_from_document(
    document: &Document,
    asset_id: AssetId,
    track_id: TrackId,
) -> Result<Candidate, CannotScore> {
    document.validate().map_err(|e| CannotScore::Document {
        detail: e.to_string(),
    })?;
    let asset = document.asset(asset_id).ok_or(CannotScore::Selection)?;
    let track = document
        .tracks
        .iter()
        .find(|t| t.id == track_id)
        .ok_or(CannotScore::Selection)?;
    let hash = asset
        .source_fingerprint
        .content_sha256
        .as_deref()
        .filter(|h| valid_hash(h))
        .filter(|_| asset.source_fingerprint.byte_len.is_some())
        .ok_or(CannotScore::UnverifiedSource)?;
    let shape = |detail| CannotScore::TrackShape { detail };
    if !matches!(asset.kind, MediaKind::Audio | MediaKind::AudioVideo) {
        return Err(shape("selected asset does not carry dialogue audio"));
    }
    // The DTO has source ranges, not a rendered mix. Refuse processing whose
    // audible result cannot be inferred from those ranges alone.
    let mix = &document.audio_mix;
    let selected = mix.tracks.iter().find(|m| m.track == track_id);
    let solo_excludes_selected =
        mix.tracks.iter().any(|m| m.solo) && !selected.is_some_and(|m| m.solo);
    if !mix.master.is_neutral()
        || !mix.pan_law.is_balance()
        || solo_excludes_selected
        || selected.is_some_and(|m| {
            m.mute
                || m.gain_tenth_db != 0
                || m.pan_percent != 0
                || m.gain_curve.is_some()
                || m.pan_curve.is_some()
        })
        || mix.buses.iter().any(|bus| bus.tracks.contains(&track_id))
    {
        return Err(shape("audio mix processing or routing is not supported"));
    }
    let mut at = TimeCode::ZERO;
    let mut kept = Vec::new();
    // Document validation already requires timeline order.
    for clip in &track.clips {
        if clip.speed_percent != 100 {
            return Err(CannotScore::Speed { clip: clip.id.0 });
        }
        if clip.asset != asset_id || !matches!(clip.content, ClipContent::Media) {
            return Err(shape(
                "dialogue track contains another source or generated content",
            ));
        }
        if !clip.enabled
            || clip.enabled_curve.is_some()
            || clip.transition_in.is_some()
            || clip.audio_gain_curve.is_some()
            || clip.audio_gain_tenth_db != 0
            || clip.audio_fade_in_frames != TimeCode::ZERO
            || clip.audio_fade_out_frames != TimeCode::ZERO
            || clip
                .effects
                .iter()
                .any(|e| e.enabled && e.name.starts_with("audio_"))
        {
            return Err(shape("disabled, automated, faded or audio-processed clip"));
        }
        if clip.timeline_start != at {
            return Err(shape(
                "dialogue clips must be contiguous from timeline zero",
            ));
        }
        let duration = document
            .clip_duration(clip)
            .map_err(|e| CannotScore::Document {
                detail: e.to_string(),
            })?;
        let source_frames = clip.source_range.end.0 - clip.source_range.start.0;
        // Packing source ranges is exact only when project-frame rounding did
        // not add/remove playback time. Reject rather than hiding dead air.
        if i128::from(source_frames)
            * i128::from(asset.fps.denominator())
            * i128::from(document.fps.numerator())
            != i128::from(duration.0)
                * i128::from(document.fps.denominator())
                * i128::from(asset.fps.numerator())
        {
            return Err(shape(
                "source span is not exact on the project's frame grid",
            ));
        }
        at = at.checked_add(duration).ok_or(CannotScore::TimeOverflow)?;
        kept.push(SourceRange {
            start_micros: source_micros(clip.source_range.start, asset.fps)?,
            end_micros: source_micros(clip.source_range.end, asset.fps)?,
        });
    }
    if at != document.duration && !track.clips.is_empty() {
        return Err(shape("dialogue track does not cover the program duration"));
    }
    validate_ranges(&kept, source_micros(asset.duration, asset.fps)?, "document")?;
    // An owned candidate snapshot must retain its source identity after the
    // caller releases or edits the borrowed Document.
    Ok(Candidate {
        recording_sha256: hash.to_owned(),
        kept,
    })
}
