use super::{Error, RoleBounds, capacity, capacity::Heap, encoded_bytes, metrics};
use crate::Operation as O;
use crate::{Document, Operation, history_size::ReplayBudget};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) struct DocumentBound {
    pub encoded: u64,
    pub live: u64,
}
impl DocumentBound {
    pub fn measure(document: &Document, bounds: RoleBounds) -> Result<Self, Error> {
        let encoded = encoded_bytes(document)?;
        metrics::exact(encoded);
        let result = Self {
            encoded,
            live: capacity::document_bytes(document)?,
        };
        result.check(bounds)?;
        Ok(result)
    }
    pub fn check(self, bounds: RoleBounds) -> Result<(), Error> {
        if self.encoded > bounds.document_bytes || self.live > bounds.document_live_bytes {
            Err(Error::DocumentCapacity)
        } else {
            Ok(())
        }
    }
    pub fn grow(
        &mut self,
        growth: Self,
        document: &Document,
        bounds: RoleBounds,
    ) -> Result<(), Error> {
        self.encoded = self.encoded.saturating_add(growth.encoded);
        self.live = self.live.saturating_add(growth.live);
        if self.check(bounds).is_err() {
            *self = Self::measure(document, bounds)?;
        }
        Ok(())
    }
}

fn array_growth<T>(values: &Vec<T>) -> u64 {
    if values.len() < values.capacity() {
        return 0;
    }
    // The same geometric array reservation used by ReplayBudget. Capacity is
    // preserved by the atomic copy, so unused slots remain available.
    let next = values
        .capacity()
        .saturating_mul(2)
        .max(values.len().saturating_add(1))
        .max(4);
    (next.saturating_sub(values.capacity()) as u64).saturating_mul(std::mem::size_of::<T>() as u64)
}

#[allow(
    clippy::too_many_lines,
    reason = "exhaustive growth classification for all operation kinds"
)]
pub(super) fn reservation(
    document: &Document,
    operation: &Operation,
    size: DocumentBound,
    budget: &mut ReplayBudget,
) -> Result<DocumentBound, Error> {
    metrics::reservation();
    let copied =
        crate::history_size::operation_copy_bytes_with(document, operation, Some(size.live))
            .map_err(|_| Error::Capacity)?;
    let copied_encoded =
        crate::history_size::operation_copy_bytes_with(document, operation, Some(size.encoded))
            .map_err(|_| Error::Capacity)?;
    let payload = operation.heap()?;
    let encoded = encoded_bytes(operation)?;
    let growth = match operation {
        O::SetEffectParam { .. }
        | O::SetEffectKeyframes { .. }
        | O::SetClipEnabledCurve { .. }
        | O::SetClipGainEnvelope { .. }
        | O::SetTitleParam { .. }
        | O::SetMarkerParam { .. }
        | O::SetAudioMaster { .. } => DocumentBound {
            encoded: encoded.saturating_mul(4).saturating_add(256),
            // Replacements own their complete new payload. BTreeMap insertion
            // has at most the registered parameter set's small node overhead.
            live: payload.saturating_mul(4).saturating_add(4096),
        },
        O::AddAsset { .. }
        | O::AddLutAsset { .. }
        | O::AddMarker { .. }
        | O::AddTrack { .. }
        | O::UpsertBin { .. }
        | O::UpsertStringOut { .. }
        | O::UpsertSyncGroup { .. }
        | O::UpsertAudioBus { .. }
        | O::SetTrackMix { .. }
        | O::SetTrackAutomation { .. } => {
            let arrays = match operation {
                O::AddAsset { .. } => array_growth(&document.media_pool),
                O::AddLutAsset { .. } => array_growth(&document.lut_assets),
                O::AddMarker { .. } => array_growth(&document.markers),
                O::AddTrack { .. } => array_growth(&document.tracks),
                O::UpsertBin { .. } => array_growth(&document.catalog.bins),
                O::UpsertStringOut { .. } => array_growth(&document.catalog.string_outs),
                O::UpsertSyncGroup { .. } => array_growth(&document.catalog.sync_groups),
                O::UpsertAudioBus { .. } => array_growth(&document.audio_mix.buses),
                O::SetTrackMix { .. } | O::SetTrackAutomation { .. } => {
                    array_growth(&document.audio_mix.tracks)
                }
                _ => unreachable!(),
            };
            DocumentBound {
                encoded: encoded.saturating_mul(4).saturating_add(256),
                live: arrays
                    .saturating_add(payload.saturating_mul(4))
                    .saturating_add(4096),
            }
        }

        O::SetTrackSyncLock { .. } | O::SetEffectEnabled { .. } | O::SetClipEnabled { .. } => {
            DocumentBound {
                encoded: 20,
                live: 0,
            }
        }
        O::RemoveTrack { .. }
        | O::DeleteClip { .. }
        | O::RemoveMarker { .. }
        | O::RemoveEffect { .. }
        | O::RemoveBin { .. }
        | O::RemoveStringOut { .. }
        | O::RemoveSyncGroup { .. }
        | O::RemoveAudioBus { .. }
        | O::RemoveLutAsset { .. }
        | O::ClearEffectKeyframes { .. }
        | O::RemoveTransition { .. }
        | O::UnlinkClips { .. }
        | O::MoveMarker { .. }
        | O::SetClipAudio { .. }
        | O::SetClipSpeed { .. }
        | O::SetClipBlendMode { .. }
        | O::SetSolidColor { .. }
        | O::SetAssetColorDescription { .. }
        | O::SetColorContext { .. }
        | O::SetPanLaw { .. } => DocumentBound {
            encoded: encoded
                .saturating_mul(4)
                .saturating_add(copied_encoded)
                .saturating_add(256),
            live: 0,
        },
        // Keep this conservative fallback until growth observations include
        // enclosing Vec capacities and implicit clip/automation fan-out. A
        // touched-clip payload alone misses an allocation when a full track's
        // clip array doubles. Looking up every affected clip here would add a
        // document walk. The transition-capacity guards cover the fallback;
        // smaller growth terms need maintained maxima or kernel observations.
        O::RelinkAsset { .. }
        | O::SetAssetBin { .. }
        | O::AddClip { .. }
        | O::AddTitle { .. }
        | O::SplitClip { .. }
        | O::TrimClip { .. }
        | O::MoveClip { .. }
        | O::ThreePointEdit { .. }
        | O::PatchedThreePointEdit { .. }
        | O::SlipClip { .. }
        | O::RollEdit { .. }
        | O::SlideClip { .. }
        | O::ReplaceClip { .. }
        | O::FitToFill { .. }
        | O::RippleDeleteClip { .. }
        | O::RippleInsertGap { .. }
        | O::LinkClips { .. }
        | O::AddEffect { .. }
        | O::InsertEffect { .. }
        | O::UpsertEffectKeyframe { .. }
        | O::RemoveEffectKeyframe { .. }
        | O::CopyClipAttributes { .. }
        | O::ConvertLegacyLook { .. }
        | O::AddTransition { .. }
        | O::AddFreezeFrame { .. }
        | O::AddAdjustmentClip { .. }
        | O::AddSolidClip { .. } => DocumentBound {
            encoded: encoded
                .saturating_mul(4)
                .saturating_add(copied_encoded)
                .saturating_add(256),
            // The existing table reserves source copies; this also covers a
            // touched array's geometric reallocation and its new owned payload.
            live: size
                .live
                .saturating_add(payload.saturating_mul(4))
                .saturating_add(copied)
                .saturating_add(4096),
        },
    };
    let working = size
        .live
        .checked_mul(3)
        .and_then(|n| n.checked_add(copied))
        .and_then(|n| n.checked_add(payload.saturating_mul(4)))
        .ok_or(Error::Capacity)?;
    budget.check_known(working).map_err(|_| Error::Capacity)?;
    Ok(growth)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn touched_clip_payload_misses_full_track_array_growth() {
        let cases: serde_json::Value = serde_json::from_str(include_str!(
            "../../tests/support/history_operation_cases.json"
        ))
        .unwrap();
        let mut document: Document =
            serde_json::from_value(cases["SplitClip"]["document"].clone()).unwrap();
        let template = document.tracks[0].clips[0].clone();
        document.tracks[0].clips = (0..4)
            .map(|index| {
                let mut clip = template.clone();
                clip.id = crate::ClipId(index + 1);
                clip.effects.clear();
                clip.timeline_start = crate::TimeCode(i64::try_from(index).unwrap() * 100);
                clip
            })
            .collect();
        document.tracks[0].clips.shrink_to_fit();
        document.recompute_duration().unwrap();
        let before = capacity::document_bytes(&document).unwrap();
        let clip_payload = document.tracks[0].clips[0].heap().unwrap();
        let enclosing_growth = array_growth(&document.tracks[0].clips);
        O::SplitClip {
            clip: crate::ClipId(1),
            at: crate::TimeCode(20),
        }
        .apply(&mut document)
        .unwrap();
        let after = capacity::document_bytes(&document).unwrap();
        assert!(
            after - before > 2 * clip_payload,
            "TOUCHED_PAYLOAD_MISSES_ENCLOSING_ARRAY"
        );
        assert!(
            enclosing_growth >= 4 * std::mem::size_of::<crate::Clip>() as u64,
            "FULL_TRACK_ARRAY_GROWTH"
        );
    }
}
