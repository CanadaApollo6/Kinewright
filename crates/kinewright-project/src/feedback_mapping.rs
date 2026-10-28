//! Head-scoped feedback mapping. Core and the checkpoint publisher own lineage.
#![allow(clippy::missing_errors_doc)]

use crate::{
    bundle::{BundleError, ProjectBundle},
    bundle_records::{BranchCheckpoint, LineageRecord, ObjectRef, OccurrenceId},
    studio_records::{
        CommentResolution, FeedbackRevision, ResolveFeedback, SpatialStatus, TemporalStatus,
    },
};
use kinewright_core::{ClipId, Document, FrameRounding, TimeCode, map_frames_with_rounding};
use kinewright_media::{video_layers_at, visual_layers_at};

/// A script owner supplies mappings for one exact request, after validating its beat map.
/// A replacement take can map meaning while leaving spatial resolution unresolved.
pub trait FeedbackBeatOwner {
    fn candidates(
        &self,
        request: &ResolveFeedback,
        original: &FeedbackRevision,
        candidate: &BranchCheckpoint,
    ) -> Result<Vec<MappedFrame>, BundleError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MappedFrame {
    pub clip: ClipId,
    pub frame: i64,
}

pub struct MappedFeedback {
    pub resolution: CommentResolution,
    pub candidates: Vec<MappedFrame>,
    pub original_frame: i64,
    pub original_frame_proof: ObjectRef,
    /// A proposal only. The saved strokes and frame proof remain untouched.
    pub proposed_points: Option<Vec<[f64; 2]>>,
    _pin: crate::store::StorePin,
}

impl ProjectBundle {
    /// Read both immutable checkpoints, never the current Core or selected UI version.
    #[allow(
        clippy::too_many_lines,
        reason = "Temporal and spatial outcomes share one pinned pair of checkpoints"
    )]
    pub fn resolve_feedback(
        &self,
        request: &ResolveFeedback,
        beats: Option<&dyn FeedbackBeatOwner>,
    ) -> Result<MappedFeedback, BundleError> {
        let original = self.read_feedback(&request.comment_revision_ref)?;
        let revision = &original.revision;
        let snapshot = self.read_snapshot(&revision.anchor.snapshot)?;
        let pin = self.store().pin(&[
            request.comment_revision_ref.clone(),
            snapshot.descriptor.checkpoint_ref.clone(),
            request.candidate_checkpoint_ref.clone(),
        ])?;
        let checkpoint = self
            .store()
            .read_record::<BranchCheckpoint>(&request.candidate_checkpoint_ref)?;
        let origin = self
            .store()
            .read_record::<Document>(&snapshot.descriptor.document_ref)?;
        let candidate = self
            .store()
            .read_record::<Document>(&checkpoint.document_ref)?;
        let old_lineage = self
            .store()
            .read_record::<LineageRecord>(&snapshot.descriptor.lineage_ref)?;
        let new_lineage = self
            .store()
            .read_record::<LineageRecord>(&checkpoint.lineage_ref)?;
        let mut result = MappedFeedback {
            resolution: CommentResolution {
                original_ref: request.comment_revision_ref.clone(),
                candidate_head: checkpoint.head_id.clone(),
                temporal: TemporalStatus::Unresolved,
                spatial: SpatialStatus::Unresolved,
                evidence_refs: vec![
                    revision.frame_proof_ref.clone(),
                    snapshot.descriptor.checkpoint_ref.clone(),
                    request.candidate_checkpoint_ref.clone(),
                ],
            },
            candidates: vec![],
            original_frame: revision.anchor.frame,
            original_frame_proof: revision.frame_proof_ref.clone(),
            proposed_points: None,
            _pin: pin,
        };
        let Some(target) = &revision.anchor.target else {
            return Ok(result);
        };
        let Some(occurrence) = &target.occurrence_id else {
            return Ok(result);
        };
        if old_lineage.current.get(&target.local_clip_id) != Some(occurrence)
            || revision.anchor.snapshot.document_hash != snapshot.descriptor.document_ref.sha256
            || revision.anchor.fps.numerator != origin.fps.numerator()
            || revision.anchor.fps.denominator != origin.fps.denominator()
        {
            return Ok(result);
        }
        let old_layers = video_layers_at(&origin, TimeCode(revision.anchor.frame))
            .map_err(|e| BundleError::Corrupt(e.to_string()))?;
        let Some(old_layer) = old_layers
            .iter()
            .find(|l| l.source.clip == target.local_clip_id)
        else {
            return Ok(result);
        };
        if target
            .local_frame
            .is_some_and(|f| f != old_layer.source.source_at.0)
        {
            return Ok(result);
        }
        let Some(old_asset) = origin.asset(old_layer.source.asset) else {
            return Ok(result);
        };
        let old_inputs = self
            .store()
            .read_input_manifest(&snapshot.descriptor.input_manifest_ref)?;
        let new_inputs = self
            .store()
            .read_input_manifest(&checkpoint.input_manifest_ref)?;
        let old_entries: Vec<_> = old_inputs
            .entries
            .iter()
            .filter(|entry| entry.matches_media_asset(old_asset))
            .collect();
        let [old_entry] = old_entries.as_slice() else {
            return Ok(result);
        };
        if old_asset.source_fingerprint.content_sha256.is_none()
            || old_asset.source_fingerprint.byte_len.is_none()
            || revision.anchor.source_ref.as_ref().is_some_and(|r| {
                Some(&r.sha256) != old_asset.source_fingerprint.content_sha256.as_ref()
                    || Some(r.byte_length) != old_asset.source_fingerprint.byte_len
            })
        {
            return Ok(result);
        }
        // Undo can leave the occurrence only in redo; absent ancestry cannot prove removal.
        let mut removal_proven = !is_future_occurrence(occurrence, &new_lineage);
        for track in &candidate.tracks {
            for clip in &track.clips {
                let Some(current) = new_lineage.current.get(&clip.id) else {
                    removal_proven = false;
                    continue;
                };
                if !descends_from(current, occurrence, &new_lineage)
                    && !descends_from(occurrence, current, &old_lineage)
                {
                    continue;
                }
                let Some(asset) = candidate.asset(clip.asset) else {
                    continue;
                };
                if asset.source_fingerprint != old_asset.source_fingerprint
                    || !new_inputs
                        .entries
                        .iter()
                        .any(|entry| entry == *old_entry && entry.matches_media_asset(asset))
                    || !clip.source_range.contains(&old_layer.source.source_at)
                {
                    continue;
                }
                let effective = kinewright_core::clip_effective_fps(asset.fps, clip)
                    .map_err(|e| BundleError::Corrupt(e.to_string()))?;
                let offset = old_layer
                    .source
                    .source_at
                    .checked_sub(clip.source_range.start)
                    .ok_or(BundleError::Capacity("feedback source offset"))?;
                let at =
                    map_frames_with_rounding(offset, effective, candidate.fps, FrameRounding::Ceil)
                        .map_err(|e| BundleError::Corrupt(e.to_string()))?;
                let Some(frame) = clip.timeline_start.checked_add(at) else {
                    continue;
                };
                let layers = video_layers_at(&candidate, frame)
                    .map_err(|e| BundleError::Corrupt(e.to_string()))?;
                if layers.iter().any(|l| {
                    l.source.clip == clip.id && l.source.source_at == old_layer.source.source_at
                }) {
                    if result.candidates.len() >= 512 {
                        return Err(BundleError::Capacity("feedback mapping candidates"));
                    }
                    result.candidates.push(MappedFrame {
                        clip: clip.id,
                        frame: frame.0,
                    });
                }
            }
        }
        let source_mapping = !result.candidates.is_empty();
        if !source_mapping
            && request.beat_map_ref.is_some()
            && revision.anchor.script_beat_id.is_some()
            && let Some(beats) = beats
        {
            result.candidates = beats.candidates(request, revision, &checkpoint)?;
            if result.candidates.len() > 512 {
                return Err(BundleError::Capacity("feedback beat candidates"));
            }
            result.candidates.retain(|c| {
                c.frame >= 0
                    && c.frame < candidate.duration.0
                    && video_layers_at(&candidate, TimeCode(c.frame))
                        .is_ok_and(|layers| layers.iter().any(|l| l.source.clip == c.clip))
            });
        }
        result.candidates.sort_by_key(|c| (c.frame, c.clip));
        result.candidates.dedup();
        result.resolution.temporal = match result.candidates.as_slice() {
            [] if !removal_proven => TemporalStatus::Unresolved,
            [] if request.beat_map_ref.is_some() && beats.is_none() => TemporalStatus::Unresolved,
            [] => TemporalStatus::Removed,
            [one] if source_mapping && one.frame == revision.anchor.frame => TemporalStatus::Exact,
            [_] => TemporalStatus::Mapped,
            _ => TemporalStatus::Ambiguous,
        };
        if result.resolution.temporal == TemporalStatus::Removed {
            result.resolution.spatial = SpatialStatus::Removed;
        } else if source_mapping && let [one] = result.candidates.as_slice() {
            let new_layers = video_layers_at(&candidate, TimeCode(one.frame))
                .map_err(|e| BundleError::Corrupt(e.to_string()))?;
            if let [new_layer] = new_layers.as_slice()
                && old_layers.len() == 1
                && visual_layers_at(&origin, TimeCode(revision.anchor.frame))
                    .is_ok_and(|layers| layers.len() == 1)
                && visual_layers_at(&candidate, TimeCode(one.frame))
                    .is_ok_and(|layers| layers.len() == 1)
                && origin.resolution == candidate.resolution
                && revision.anchor.raster_size == [origin.resolution.0, origin.resolution.1]
                && revision.anchor.orientation == crate::studio_records::Orientation::Upright
                && old_layer.transition == new_layer.transition
                && old_layer.transition == kinewright_media::TransitionRenderParams::default()
                && old_layer.blend_mode == new_layer.blend_mode
                && old_layer.blend_mode == kinewright_core::BlendMode::Normal
                && candidate.asset(new_layer.source.asset) == Some(old_asset)
            {
                let original_points = self.store().read_evidence(&revision.strokes_ref)?;
                let points: Vec<[f64; 2]> = serde_json::from_slice(&original_points)
                    .map_err(|e| BundleError::Corrupt(e.to_string()))?;
                if let Some(mapped) = kinewright_media::map_feedback_geometry(
                    &old_layer.effects,
                    &new_layer.effects,
                    revision.anchor.raster_size,
                    &points,
                ) {
                    result.resolution.spatial = if old_layer.effects == new_layer.effects {
                        SpatialStatus::Exact
                    } else {
                        SpatialStatus::Transformed
                    };
                    result.proposed_points = Some(mapped);
                }
            }
        }
        Ok(result)
    }
}

fn descends_from(current: &OccurrenceId, origin: &OccurrenceId, lineage: &LineageRecord) -> bool {
    let mut next = current;
    // The persisted, bounded ancestry owns the edges. The bound also refuses cycles.
    for _ in 0..=lineage.split_ancestry.len() {
        if next == origin {
            return true;
        }
        let Some((_, parent, _)) = lineage
            .split_ancestry
            .iter()
            .find(|(child, _, _)| child == next)
        else {
            return false;
        };
        next = parent;
    }
    false
}

fn is_future_occurrence(occurrence: &OccurrenceId, lineage: &LineageRecord) -> bool {
    lineage
        .redo
        .iter()
        .flat_map(|map| map.values())
        .any(|known| known == occurrence)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn s3_incomplete_split_ancestry_cannot_prove_removal() {
        let branch = crate::bundle_records::BranchId::fresh().unwrap();
        let child = OccurrenceId {
            branch_origin: branch.clone(),
            creation_batch_id: crate::bundle_records::BatchId::fresh().unwrap(),
            result_ordinal: 1,
        };
        let mut lineage = LineageRecord {
            branch_origin: branch,
            baseline: std::collections::BTreeMap::default(),
            current: std::collections::BTreeMap::default(),
            undo: vec![],
            redo: vec![[(ClipId(2), child.clone())].into()],
            split_ancestry: vec![],
        };
        assert!(
            is_future_occurrence(&child, &lineage),
            "S3_UNPROVEN_ANCESTRY_IS_NOT_REMOVAL"
        );
        lineage.redo.clear();
        assert!(!is_future_occurrence(&child, &lineage));
    }
}
