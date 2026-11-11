//! Typed live-history bounds, shared by portable decoding and project admission.
use crate::HistoryError;
use crate::{Clip, ClipContent, Document, HistorySnapshot};
use serde::Serialize;
use std::{collections::BTreeSet, mem::size_of, sync::Arc};

/// Standalone transports have no store ledger beside their replay reservation.
pub const TRANSPORT_LIVE_BYTES: u64 = 128 << 20;

fn sum(values: impl IntoIterator<Item = u64>) -> Result<u64, HistoryError> {
    values.into_iter().try_fold(0_u64, |total, bytes| {
        total.checked_add(bytes).ok_or(HistoryError::Capacity)
    })
}
fn allocation<T>(capacity: usize) -> Result<u64, HistoryError> {
    (capacity as u64)
        .checked_mul(size_of::<T>() as u64)
        .ok_or(HistoryError::Capacity)
}
fn payload(value: &impl Serialize) -> Result<u64, HistoryError> {
    let bytes = serde_json::to_vec(value).map_err(|_| HistoryError::Corrupt)?;
    payload_memory_bound(&bytes)
}

// The same conservative structural costs as store preflight, for generated JSON.
// This streaming scan allocates no intermediate JSON tree during live admission.
fn payload_memory_bound(bytes: &[u8]) -> Result<u64, HistoryError> {
    use serde::de::{DeserializeSeed, Error, MapAccess, SeqAccess, Visitor};
    use std::{cell::Cell, fmt};
    struct Scan<'a>(&'a Cell<u64>, u32);
    impl Scan<'_> {
        fn add<E: Error>(&self, amount: u64) -> Result<(), E> {
            let next = self
                .0
                .get()
                .checked_add(amount)
                .filter(|value| *value <= 64 << 20)
                .ok_or_else(|| E::custom("history payload capacity"))?;
            self.0.set(next);
            Ok(())
        }
    }
    impl<'de> DeserializeSeed<'de> for Scan<'_> {
        type Value = ();
        fn deserialize<D: serde::Deserializer<'de>>(self, deserializer: D) -> Result<(), D::Error> {
            if self.1 > 128 {
                return Err(D::Error::custom("history payload depth"));
            }
            self.add(64)?;
            deserializer.deserialize_any(self)
        }
    }
    impl<'de> Visitor<'de> for Scan<'_> {
        type Value = ();
        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("history JSON")
        }
        fn visit_bool<E: Error>(self, _: bool) -> Result<(), E> {
            Ok(())
        }
        fn visit_i64<E: Error>(self, _: i64) -> Result<(), E> {
            Ok(())
        }
        fn visit_u64<E: Error>(self, _: u64) -> Result<(), E> {
            Ok(())
        }
        fn visit_f64<E: Error>(self, _: f64) -> Result<(), E> {
            Ok(())
        }
        fn visit_unit<E: Error>(self) -> Result<(), E> {
            Ok(())
        }
        fn visit_str<E: Error>(self, value: &str) -> Result<(), E> {
            self.add(value.len() as u64)
        }
        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<(), A::Error> {
            while seq.next_element_seed(Scan(self.0, self.1 + 1))?.is_some() {}
            Ok(())
        }
        fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
            while let Some(key) = map.next_key::<String>()? {
                self.add(128 + key.len() as u64)?;
                map.next_value_seed(Scan(self.0, self.1 + 1))?;
            }
            Ok(())
        }
    }
    let initial = (bytes.len() as u64)
        .checked_mul(2)
        .filter(|value| *value <= 64 << 20)
        .ok_or(HistoryError::Capacity)?;
    let charge = Cell::new(initial);
    let mut parser = serde_json::Deserializer::from_slice(bytes);
    Scan(&charge, 0)
        .deserialize(&mut parser)
        .map_err(|_| HistoryError::Capacity)?;
    parser.end().map_err(|_| HistoryError::Corrupt)?;
    Ok(charge.get())
}

fn clip_payload(clip: &Clip) -> Result<u64, HistoryError> {
    let title = match &clip.content {
        ClipContent::Title(title) => title.text.capacity() as u64,
        ClipContent::Media
        | ClipContent::Freeze(_)
        | ClipContent::Adjustment
        | ClipContent::Solid(_) => 0,
    };
    sum([
        title,
        allocation::<crate::Effect>(clip.effects.capacity())?,
        if clip.effects.is_empty() {
            0
        } else {
            payload(&clip.effects)?
        },
        clip.audio_gain_curve.as_ref().map_or(Ok(0), |curve| {
            allocation::<crate::Keyframe>(curve.keyframes.capacity())
        })?,
        clip.enabled_curve.as_ref().map_or(Ok(0), |curve| {
            allocation::<crate::Keyframe>(curve.keyframes.capacity())
        })?,
    ])
}
/// Bound the owned document allocations using their typed capacities.
/// # Errors
/// Refuses arithmetic overflow or an excessive nested payload.
pub fn document_live_bytes(document: &Document) -> Result<u64, HistoryError> {
    document_bytes(document, false)
}

fn transport_padding<T>(values: &Vec<T>) -> Result<u64, HistoryError> {
    replay_array_padding::<T>(values.len(), values.capacity())
}

fn replay_array_padding<T>(length: usize, retained_capacity: usize) -> Result<u64, HistoryError> {
    if length == 0 {
        return Ok(0);
    }
    // JSON sequences have no exact size hint. Vec starts at its minimum
    // nonzero allocation and grows geometrically while serde fills it.
    let minimum = if size_of::<T>() == 1 {
        8
    } else if size_of::<T>() <= 1024 {
        4
    } else {
        1
    };
    let capacity = length
        .checked_next_power_of_two()
        .ok_or(HistoryError::Capacity)?
        .max(minimum);
    allocation::<T>(capacity.saturating_sub(retained_capacity))
}

fn transport_document_bytes(document: &Document) -> Result<u64, HistoryError> {
    let mut bytes = sum([
        document_live_bytes(document)?,
        transport_padding(&document.tracks)?,
        transport_padding(&document.media_pool)?,
        transport_padding(&document.markers)?,
        transport_padding(&document.lut_assets)?,
    ])?;
    for track in &document.tracks {
        bytes = sum([bytes, transport_padding(&track.clips)?])?;
        for clip in &track.clips {
            bytes = sum([bytes, transport_padding(&clip.effects)?])?;
            for curve in [&clip.audio_gain_curve, &clip.enabled_curve]
                .into_iter()
                .flatten()
            {
                bytes = sum([bytes, transport_padding(&curve.keyframes)?])?;
            }
        }
    }
    Ok(bytes)
}
fn document_bytes(document: &Document, cloned: bool) -> Result<u64, HistoryError> {
    // The dominating arrays use their actual typed capacities, including unused slots.
    // Nested owned payloads use the existing conservative structural estimator.
    let Document {
        fps: _,
        resolution: _,
        duration: _,
        tracks,
        media_pool,
        markers,
        catalog,
        audio_mix,
        color_context,
        lut_assets,
        investigator,
    } = document;
    let mut bytes = sum([
        size_of::<Document>() as u64 + 16,
        allocation::<crate::Track>(if cloned {
            tracks.len()
        } else {
            tracks.capacity()
        })?,
        allocation::<crate::MediaAsset>(if cloned {
            media_pool.len()
        } else {
            media_pool.capacity()
        })?,
        allocation::<crate::Marker>(if cloned {
            markers.len()
        } else {
            markers.capacity()
        })?,
        allocation::<crate::LutAsset>(if cloned {
            lut_assets.len()
        } else {
            lut_assets.capacity()
        })?,
        payload(&(
            media_pool,
            markers,
            catalog,
            audio_mix,
            color_context,
            lut_assets,
            investigator,
        ))?,
    ])?;
    for track in tracks {
        bytes = sum([
            bytes,
            allocation::<Clip>(if cloned {
                track.clips.len()
            } else {
                track.clips.capacity()
            })?,
        ])?;
        for clip in &track.clips {
            bytes = sum([bytes, clip_payload(clip)?])?;
        }
    }
    Ok(bytes)
}

/// Charge the document arrays rebuilt by cloning during replay.
/// # Errors
/// Refuses arithmetic overflow or an excessive nested payload.
pub fn replay_document_bytes(document: &Document) -> Result<u64, HistoryError> {
    document_bytes(document, true)
}

/// Bound the unique boundary documents, command payloads and actor operation log.
/// # Errors
/// Refuses arithmetic overflow or an excessive nested payload.
pub fn history_live_bytes(history: &HistorySnapshot) -> Result<u64, HistoryError> {
    let mut documents = BTreeSet::new();
    let mut bytes = 2048_u64;
    for document in [&history.baseline, &history.document].into_iter().chain(
        history
            .cursor
            .undo
            .iter()
            .chain(&history.cursor.redo)
            .map(|entry| &entry.document),
    ) {
        if documents.insert(Arc::as_ptr(document)) {
            bytes = sum([bytes, document_live_bytes(document)?])?;
        }
    }
    bytes = sum([
        bytes,
        allocation::<crate::HistoryBatch>(history.batches.capacity())?,
    ])?;
    for batch in &history.batches {
        // Also covers the actor's duplicate accepted-operation log while restored/live.
        bytes = sum([
            bytes,
            payload(&batch.command)?
                .checked_mul(2)
                .ok_or(HistoryError::Capacity)?,
            allocation::<crate::OperationIdentityEffect>(batch.identity_effects.capacity())?,
        ])?;
        for effect in &batch.identity_effects {
            bytes = sum([
                bytes,
                allocation::<crate::ClipId>(effect.created.capacity())?,
                allocation::<crate::ClipId>(effect.deleted.capacity())?,
                allocation::<Clip>(effect.split_results.capacity())?,
                effect.split_parent.as_ref().map_or(Ok(0), clip_payload)?,
            ])?;
            for clip in &effect.split_results {
                bytes = sum([bytes, clip_payload(clip)?])?;
            }
        }
    }
    for entries in [&history.cursor.undo, &history.cursor.redo] {
        bytes = sum([
            bytes,
            allocation::<crate::GestureBoundary>(entries.capacity())?,
        ])?;
        for entry in entries {
            bytes = sum([
                bytes,
                allocation::<crate::Operation>(entry.operations.capacity())?,
                payload(&entry.operations)?,
                entry
                    .coalesce_key
                    .as_ref()
                    .map_or(0, |key| key.capacity() as u64),
            ])?;
        }
    }
    // Structural payload estimates already overcharge their typed storage. The
    // additional ten percent is checked against the restored-heap witness.
    bytes
        .checked_add(bytes.div_ceil(10))
        .ok_or(HistoryError::Capacity)
}

/// Bound retained history after portable decoding and journal replay.
/// # Errors
/// Refuses arithmetic overflow or an excessive nested payload.
pub fn restored_history_live_bytes(history: &HistorySnapshot) -> Result<u64, HistoryError> {
    let undo_peak = history
        .batches
        .iter()
        .map(|batch| batch.undo_entries)
        .max()
        .unwrap_or(0)
        .max(history.cursor.undo.len());
    let redo_peak = history
        .batches
        .iter()
        .map(|batch| batch.redo_entries)
        .max()
        .unwrap_or(0)
        .max(history.cursor.redo.len());
    let mut padding = sum([
        transport_document_bytes(&history.baseline)?
            .saturating_sub(document_live_bytes(&history.baseline)?),
        replay_array_padding::<crate::HistoryBatch>(
            history.batches.len(),
            history.batches.capacity(),
        )?,
        replay_array_padding::<crate::GestureBoundary>(undo_peak, history.cursor.undo.capacity())?,
        replay_array_padding::<crate::GestureBoundary>(redo_peak, history.cursor.redo.capacity())?,
    ])?;
    for entry in history.cursor.undo.iter().chain(&history.cursor.redo) {
        // Ordinary batches clone at exact length. Coalescing can double an
        // arbitrary initial length, so a power-of-two bound is insufficient.
        if entry.operations.len() > 1 {
            let capacity = entry
                .operations
                .len()
                .checked_mul(2)
                .ok_or(HistoryError::Capacity)?;
            padding = sum([
                padding,
                allocation::<crate::Operation>(
                    capacity.saturating_sub(entry.operations.capacity()),
                )?,
            ])?;
        }
    }
    sum([history_live_bytes(history)?, padding, padding.div_ceil(10)])
}

/// Reserve materialized states before replay starts. Growing commands contribute
/// their complete admitted payload to every possible later boundary.
/// Bound the document and command allocations before a replay starts.
/// # Errors
/// Refuses arithmetic overflow or an excessive nested payload.
pub fn replay_live_bound(
    journal: &crate::HistoryJournal,
    baseline: &Document,
    current: &Document,
) -> Result<u64, HistoryError> {
    use crate::{JournalCommand, Operation};
    let mut growth = 0_u64;
    let mut arrays_may_grow = false;
    let mut operations = 0_u64;
    let mut boundary_count = 0_u64;
    for batch in &journal.batches {
        boundary_count =
            boundary_count.max(sum([batch.undo_entries as u64, batch.redo_entries as u64])?);
        let ops: &[Operation] = match &batch.command {
            JournalCommand::Do(op) => std::slice::from_ref(op),
            JournalCommand::DoBatch(ops)
            | JournalCommand::DoBatchCoalesced {
                operations: ops, ..
            } => ops,
            JournalCommand::Undo | JournalCommand::Redo => &[],
        };
        operations = sum([operations, ops.len() as u64])?;
        for op in ops {
            if !matches!(
                op,
                Operation::SetTrackSyncLock { .. }
                    | Operation::RippleDeleteClip { .. }
                    | Operation::DeleteClip { .. }
            ) {
                arrays_may_grow |= grows_timeline_arrays(op);
                growth = sum([growth, payload(op)?])?;
            }
        }
    }
    let per_document = sum([
        document_bytes(baseline, true)?.max(document_bytes(current, true)?),
        growth,
    ])?;
    // Moving into a full track, or inserting a clip, can double a Vec's
    // capacity even when the final document has almost the same length.
    // Metadata edits, flag changes and whole-clip deletion do not grow them.
    let per_document = if arrays_may_grow {
        per_document.checked_mul(2).ok_or(HistoryError::Capacity)?
    } else {
        per_document
    };
    let documents = per_document
        .checked_mul(boundary_count + 1)
        .ok_or(HistoryError::Capacity)?;
    let identities = operations
        .checked_mul(std::mem::size_of::<crate::OperationIdentityEffect>() as u64)
        .ok_or(HistoryError::Capacity)?;
    let bytes = sum([
        documents,
        transport_document_bytes(baseline)?,
        transport_document_bytes(current)?,
        identities,
        payload(journal)?
            .checked_mul(3)
            .ok_or(HistoryError::Capacity)?,
        8192,
    ])?;
    bytes
        .checked_add(bytes.div_ceil(10))
        .ok_or(HistoryError::Capacity)
}

// Keep this exhaustive: a new operation must declare whether replay can grow
// the typed track/clip arrays. Nested metadata has its separate payload bound.
fn grows_timeline_arrays(operation: &crate::Operation) -> bool {
    use crate::Operation;
    match operation {
        Operation::AddTrack { .. }
        | Operation::AddClip { .. }
        | Operation::AddTitle { .. }
        | Operation::SplitClip { .. }
        | Operation::MoveClip { .. }
        | Operation::ThreePointEdit { .. }
        | Operation::PatchedThreePointEdit { .. }
        | Operation::RollEdit { .. }
        | Operation::SlideClip { .. }
        | Operation::ReplaceClip { .. }
        | Operation::FitToFill { .. }
        | Operation::RippleInsertGap { .. }
        | Operation::AddFreezeFrame { .. }
        | Operation::SetClipSpeed { .. }
        | Operation::AddAdjustmentClip { .. }
        | Operation::AddSolidClip { .. } => true,
        Operation::AddAsset { .. }
        | Operation::RelinkAsset { .. }
        | Operation::SetAssetColorDescription { .. }
        | Operation::SetColorContext { .. }
        | Operation::UpsertBin { .. }
        | Operation::RemoveBin { .. }
        | Operation::SetAssetBin { .. }
        | Operation::UpsertStringOut { .. }
        | Operation::RemoveStringOut { .. }
        | Operation::UpsertSyncGroup { .. }
        | Operation::RemoveSyncGroup { .. }
        | Operation::UpsertAudioBus { .. }
        | Operation::RemoveAudioBus { .. }
        | Operation::SetAudioMaster { .. }
        | Operation::SetPanLaw { .. }
        | Operation::RemoveTrack { .. }
        | Operation::SetTrackSyncLock { .. }
        | Operation::SetTrackMix { .. }
        | Operation::SetTrackAutomation { .. }
        | Operation::TrimClip { .. }
        | Operation::SlipClip { .. }
        | Operation::DeleteClip { .. }
        | Operation::RippleDeleteClip { .. }
        | Operation::LinkClips { .. }
        | Operation::UnlinkClips { .. }
        | Operation::AddMarker { .. }
        | Operation::RemoveMarker { .. }
        | Operation::MoveMarker { .. }
        | Operation::AddEffect { .. }
        | Operation::InsertEffect { .. }
        | Operation::RemoveEffect { .. }
        | Operation::SetEffectParam { .. }
        | Operation::SetEffectKeyframes { .. }
        | Operation::ClearEffectKeyframes { .. }
        | Operation::UpsertEffectKeyframe { .. }
        | Operation::RemoveEffectKeyframe { .. }
        | Operation::SetEffectEnabled { .. }
        | Operation::SetClipEnabled { .. }
        | Operation::SetClipEnabledCurve { .. }
        | Operation::CopyClipAttributes { .. }
        | Operation::ConvertLegacyLook { .. }
        | Operation::AddLutAsset { .. }
        | Operation::RemoveLutAsset { .. }
        | Operation::SetTitleParam { .. }
        | Operation::SetClipAudio { .. }
        | Operation::SetClipGainEnvelope { .. }
        | Operation::AddTransition { .. }
        | Operation::RemoveTransition { .. }
        | Operation::SetMarkerParam { .. }
        | Operation::SetClipBlendMode { .. }
        | Operation::SetSolidColor { .. } => false,
    }
}
