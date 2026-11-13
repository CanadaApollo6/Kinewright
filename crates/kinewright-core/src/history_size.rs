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
    let bytes = serde_json::to_vec(value).map_err(|_| HistoryError::Encoding)?;
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
    parser.end().map_err(|_| HistoryError::CommitInvariant)?;
    Ok(charge.get())
}

fn clip_payload(clip: &Clip, cloned: bool) -> Result<u64, HistoryError> {
    let title = match &clip.content {
        ClipContent::Title(title) => {
            (if cloned {
                title.text.len()
            } else {
                title.text.capacity()
            }) as u64
        }
        ClipContent::Media
        | ClipContent::Freeze(_)
        | ClipContent::Adjustment
        | ClipContent::Solid(_) => 0,
    };
    sum([
        title,
        allocation::<crate::Effect>(if cloned {
            clip.effects.len()
        } else {
            clip.effects.capacity()
        })?,
        if clip.effects.is_empty() {
            0
        } else {
            payload(&clip.effects)?
        },
        clip.audio_gain_curve.as_ref().map_or(Ok(0), |curve| {
            allocation::<crate::Keyframe>(if cloned {
                curve.keyframes.len()
            } else {
                curve.keyframes.capacity()
            })
        })?,
        clip.enabled_curve.as_ref().map_or(Ok(0), |curve| {
            allocation::<crate::Keyframe>(if cloned {
                curve.keyframes.len()
            } else {
                curve.keyframes.capacity()
            })
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

fn transport_vector_bytes<T>(values: &Vec<T>) -> Result<u64, HistoryError> {
    sum([
        allocation::<T>(values.capacity())?,
        transport_padding(values)?,
    ])
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
// Document's inline size already includes the complete colour context. A new
// owned field must be charged here instead of silently inheriting this omission.
const _: () = assert!(
    !std::mem::needs_drop::<crate::ColorContext>(),
    "colour context gained owned storage; include it in the live history bound"
);
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
        color_context: _,
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
            bytes = sum([bytes, clip_payload(clip, cloned)?])?;
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
    with_history_margin(history_owned_bytes(history)?)
}
fn with_history_margin(bytes: u64) -> Result<u64, HistoryError> {
    bytes
        .checked_add(bytes.div_ceil(10))
        .ok_or(HistoryError::Capacity)
}
fn history_owned_bytes(history: &HistorySnapshot) -> Result<u64, HistoryError> {
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
                effect
                    .split_parent
                    .as_ref()
                    .map_or(Ok(0), |clip| clip_payload(clip, false))?,
            ])?;
            for clip in &effect.split_results {
                bytes = sum([bytes, clip_payload(clip, false)?])?;
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
    Ok(bytes)
}

/// Bound retained history after portable decoding and journal replay.
/// # Errors
/// Refuses arithmetic overflow or an excessive nested payload.
pub fn restored_history_live_bytes(history: &HistorySnapshot) -> Result<u64, HistoryError> {
    with_history_margin(restored_history_base_bytes(history)?)
}
fn restored_history_base_bytes(history: &HistorySnapshot) -> Result<u64, HistoryError> {
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
        std::mem::size_of::<crate::HistoryReplayEvidence>() as u64 + 16 + 64,
        allocation::<std::sync::Weak<Document>>(
            2 + history.cursor.undo.len() + history.cursor.redo.len(),
        )?,
        transport_document_bytes(&history.baseline)?
            .saturating_sub(document_live_bytes(&history.baseline)?),
        replay_array_padding::<crate::HistoryBatch>(
            history.batches.len(),
            history.batches.capacity(),
        )?,
        replay_array_padding::<crate::GestureBoundary>(undo_peak, history.cursor.undo.capacity())?,
        replay_array_padding::<crate::GestureBoundary>(redo_peak, history.cursor.redo.capacity())?,
    ])?;
    // Rebinding a proved prefix clones its observation vectors at exact length.
    // A fresh replay may retain the geometric allocation from ID collection.
    // Both doors charge the same canonical capacity, including those spare slots.
    for batch in &history.batches {
        padding = sum([padding, transport_padding(&batch.identity_effects)?])?;
        for effect in &batch.identity_effects {
            padding = sum([
                padding,
                transport_padding(&effect.created)?,
                transport_padding(&effect.deleted)?,
                transport_padding(&effect.split_results)?,
            ])?;
        }
    }
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
    sum([history_owned_bytes(history)?, padding])
}

/// Measure the peak of a bounded replay. Store callers reserve their role's
/// replay window before invoking the bounded resolver, rather than treating a
/// command-payload estimate as a reservation.
/// # Errors
/// Refuses an invalid journal or replay exceeding the portable window.
pub fn measure_replay_peak(
    journal: &crate::HistoryJournal,
    baseline: &Document,
    current: &Document,
) -> Result<u64, HistoryError> {
    journal
        .resolve_with_budget(Arc::new(baseline.clone()), current, TRANSPORT_LIVE_BYTES)
        .map(|(_, peak)| peak)
}

// A decoded journal is typed Rust data, not a retained serde_json::Value tree.
// Count its arrays directly; only genuinely owned command payloads use the
// conservative structural fallback. Prior-document copies remain independent.
fn inline_command_bytes<T: Copy>(_: T) -> u64 {
    0
}
fn operation_owned_bytes(operation: &crate::Operation) -> Result<u64, HistoryError> {
    use crate::Operation;
    Ok(match operation {
        Operation::SetTrackSyncLock { track, locked } => inline_command_bytes((*track, *locked)),
        Operation::SetTrackMix {
            track,
            gain_tenth_db,
            pan_percent,
            mute,
            solo,
        } => inline_command_bytes((*track, *gain_tenth_db, *pan_percent, *mute, *solo)),
        Operation::SetEffectEnabled {
            clip,
            effect,
            enabled,
        } => inline_command_bytes((*clip, *effect, *enabled)),
        Operation::SetClipEnabled { clip, enabled } => inline_command_bytes((*clip, *enabled)),
        Operation::MoveClip { clip, to_track, to } => inline_command_bytes((*clip, *to_track, *to)),
        Operation::MoveMarker { marker, to } => inline_command_bytes((*marker, *to)),
        Operation::CopyClipAttributes {
            from_clip,
            to_clip,
            names: None,
            include_keyframes,
        } => inline_command_bytes((*from_clip, *to_clip, *include_keyframes)),
        _ => payload(operation)?,
    })
}
fn transport_clip_payload(clip: &Clip) -> Result<u64, HistoryError> {
    let mut bytes = sum([
        clip_payload(clip, false)?,
        transport_padding(&clip.effects)?,
    ])?;
    for curve in [&clip.audio_gain_curve, &clip.enabled_curve]
        .into_iter()
        .flatten()
    {
        bytes = sum([bytes, transport_padding(&curve.keyframes)?])?;
    }
    Ok(bytes)
}
fn identity_owned_bytes(
    created: &Vec<crate::ClipId>,
    deleted: &Vec<crate::ClipId>,
    parent: Option<&Clip>,
    results: &Vec<Clip>,
) -> Result<u64, HistoryError> {
    let mut bytes = sum([
        transport_vector_bytes(created)?,
        transport_vector_bytes(deleted)?,
        transport_vector_bytes(results)?,
        parent.map_or(Ok(0), transport_clip_payload)?,
    ])?;
    for clip in results {
        bytes = sum([bytes, transport_clip_payload(clip)?])?;
    }
    Ok(bytes)
}
// source command heap, replay command/cursor/log heap, operation count.
fn command_metadata_bytes(
    command: &crate::JournalCommand,
) -> Result<(u64, u64, usize), HistoryError> {
    use crate::JournalCommand;
    let (operations, vector, key) = match command {
        JournalCommand::Do(operation) => (std::slice::from_ref(operation), None, None),
        JournalCommand::DoBatch(operations) => {
            (operations.as_slice(), Some(operations.capacity()), None)
        }
        JournalCommand::DoBatchCoalesced {
            operations,
            coalesce_key,
        } => (
            operations.as_slice(),
            Some(operations.capacity()),
            Some(coalesce_key),
        ),
        JournalCommand::Undo | JournalCommand::Redo => (&[][..], None, None),
    };
    let owned = operations.iter().try_fold(0, |total, operation| {
        sum([total, operation_owned_bytes(operation)?])
    })?;
    let source = sum([
        owned,
        vector.map_or(Ok(0), |capacity| {
            sum([
                allocation::<crate::Operation>(capacity)?,
                replay_array_padding::<crate::Operation>(operations.len(), capacity)?,
            ])
        })?,
        key.map_or(0, |key| key.capacity() as u64),
    ])?;
    let replay = sum([
        owned.checked_mul(3).ok_or(HistoryError::Capacity)?,
        vector.map_or(Ok(0), |_| allocation::<crate::Operation>(operations.len()))?,
        (key.map_or(0, String::len) as u64)
            .checked_mul(2)
            .ok_or(HistoryError::Capacity)?,
    ])?;
    Ok((source, replay, operations.len()))
}
fn replay_metadata_bytes(
    journal: &crate::HistoryJournal,
    baseline: &Document,
    current: &Document,
) -> Result<u64, HistoryError> {
    let mut bytes = sum([
        size_of::<crate::HistoryJournal>() as u64,
        transport_vector_bytes(&journal.batches)?,
        journal.baseline_sha256.capacity() as u64,
        journal.document_sha256.capacity() as u64,
        transport_vector_bytes(&journal.cursor.undo)?,
        transport_vector_bytes(&journal.cursor.redo)?,
        crate::history::encoded_size(&(baseline, current, journal))?
            .checked_mul(2)
            .ok_or(HistoryError::Capacity)?,
    ])?;
    for hash in journal.cursor.undo.iter().chain(&journal.cursor.redo) {
        bytes = sum([bytes, hash.capacity() as u64])?;
    }
    let mut operations = 0_usize;
    for batch in &journal.batches {
        let (source, replay, count) = command_metadata_bytes(&batch.command)?;
        operations = operations
            .checked_add(count)
            .ok_or(HistoryError::Capacity)?;
        bytes = sum([
            bytes,
            source,
            replay,
            transport_vector_bytes(&batch.identity_effects)?,
            allocation::<crate::OperationIdentityEffect>(count)?,
        ])?;
        for effect in &batch.identity_effects {
            let crate::JournalIdentityEffect {
                created,
                deleted,
                split_parent,
                split_results,
            } = effect;
            bytes = sum([
                bytes,
                identity_owned_bytes(created, deleted, split_parent.as_ref(), split_results)?,
            ])?;
        }
    }
    let batches = if journal.batches.is_empty() {
        0
    } else {
        journal
            .batches
            .len()
            .checked_next_power_of_two()
            .ok_or(HistoryError::Capacity)?
            .max(4)
    };
    let operation_slots = if operations == 0 {
        0
    } else {
        operations
            .checked_mul(2)
            .ok_or(HistoryError::Capacity)?
            .max(4)
    };
    sum([
        bytes,
        allocation::<crate::HistoryBatch>(batches)?,
        allocation::<crate::actor::HistoryEntry>(batches)?
            .checked_mul(2)
            .ok_or(HistoryError::Capacity)?,
        allocation::<crate::Operation>(operation_slots)?
            .checked_mul(2)
            .ok_or(HistoryError::Capacity)?,
    ])
}

/// The caller reserves `limit` before replay. Resident boundary documents and
/// temporary operation copies share that window; discarded states release their
/// charge while the peak still records the whole replay's high-water mark.
#[derive(Debug, Clone)]
pub(crate) struct ReplayBudget {
    limit: u64,
    documents: u64,
    input: u64,
    identities: u64,
    dynamic_peak: u64,
}
impl ReplayBudget {
    pub(crate) fn new(
        journal: &crate::HistoryJournal,
        baseline: &Document,
        current: &Document,
        limit: u64,
    ) -> Result<Self, HistoryError> {
        let overhead = sum([
            transport_document_bytes(baseline)?,
            transport_document_bytes(current)?,
            replay_metadata_bytes(journal, baseline, current)?,
            8192,
        ])?;
        let mut budget = Self {
            limit,
            documents: 0,
            input: overhead,
            identities: 0,
            dynamic_peak: 0,
        };
        budget.documents = transport_document_bytes(baseline)?;
        budget.check(0)?;
        Ok(budget)
    }
    fn check(&mut self, additional: u64) -> Result<(), HistoryError> {
        self.dynamic_peak =
            self.dynamic_peak
                .max(sum([self.documents, self.identities, additional])?);
        let bytes = sum([self.input, self.dynamic_peak])?;
        let bytes = sum([bytes, bytes.div_ceil(10)])?;
        if bytes > self.limit {
            return Err(HistoryError::Capacity);
        }
        Ok(())
    }
    pub(crate) fn extend_input(
        &self,
        journal: &crate::HistoryJournal,
        baseline: &Document,
        current: &Document,
        limit: u64,
    ) -> Result<Self, HistoryError> {
        let input = Self::new(journal, baseline, current, limit)?.input;
        let mut resumed = self.clone();
        resumed.input = input;
        resumed.limit = limit;
        // The input charge is common to every instant in a cold replay. Recheck
        // its new value against the prefix's exact maximum, not its old total.
        resumed.check(0)?;
        Ok(resumed)
    }
    pub(crate) fn check_clone(&mut self, document: &Document) -> Result<(), HistoryError> {
        self.check(document_bytes(document, true)?)
    }
    pub(crate) fn check_operation(
        &mut self,
        document: &Document,
        operation: &crate::Operation,
    ) -> Result<(), HistoryError> {
        // This runs before EACH operation, including intermediate batch states.
        // Charge the candidate once; copying operations additionally reserve the
        // actual retained source payload, rather than their small ID-only command.
        let copied = operation_copy_bytes(document, operation)?;
        let arrays = sum([
            allocation::<crate::Track>(document.tracks.len().max(4))?,
            document.tracks.iter().try_fold(0, |total, track| {
                sum([total, allocation::<Clip>(track.clips.len().max(4))?])
            })?,
            allocation::<crate::MediaAsset>(document.media_pool.len().max(4))?,
            allocation::<crate::Marker>(document.markers.len().max(4))?,
            allocation::<crate::LutAsset>(document.lut_assets.len().max(4))?,
        ])?
        .checked_mul(2)
        .ok_or(HistoryError::Capacity)?;
        self.check(sum([
            document_live_bytes(document)?,
            arrays,
            copied,
            operation_owned_bytes(operation)?
                .checked_mul(4)
                .ok_or(HistoryError::Capacity)?,
        ])?)
    }

    pub(crate) fn retain_identity(
        &mut self,
        identity: &crate::OperationIdentityEffect,
    ) -> Result<(), HistoryError> {
        // Charge actual observations, even if the untrusted journal lied about
        // the identity payload. Later operations in the same batch cannot hide
        // a large observation by deleting its source clip.
        self.identities = sum([
            self.identities,
            identity_owned_bytes(
                &identity.created,
                &identity.deleted,
                identity.split_parent.as_ref(),
                &identity.split_results,
            )?,
        ])?;
        self.check(0)
    }
    pub(crate) const fn dynamic_peak(&self) -> u64 {
        self.dynamic_peak
    }
    pub(crate) const fn document_charge(&self) -> u64 {
        self.documents
    }
    pub(crate) fn set_document_charge(&mut self, bytes: u64) -> Result<(), HistoryError> {
        self.documents = bytes;
        self.check(0)
    }
    pub(crate) fn check_adoption_metadata(
        &mut self,
        history: &HistorySnapshot,
    ) -> Result<(), HistoryError> {
        self.check(sum([
            allocation::<(&Vec<crate::Operation>, &Option<String>)>(
                history.cursor.undo.len() + history.cursor.redo.len(),
            )?,
            allocation::<std::sync::Weak<Document>>(
                2 + history.cursor.undo.len() + history.cursor.redo.len(),
            )?,
            std::mem::size_of::<crate::HistoryReplayEvidence>() as u64 + 16 + 64,
        ])?)
    }
    pub(crate) fn finish(&self, history: &HistorySnapshot) -> Result<u64, HistoryError> {
        let retained = restored_history_live_bytes(history)?;
        if retained > self.limit {
            return Err(HistoryError::Capacity);
        }
        let raw = sum([self.input, self.dynamic_peak])?;
        Ok(sum([raw, raw.div_ceil(10)])?.max(retained))
    }
}

// Keep exhaustive: every new operation must declare its prior-state copies.
#[allow(
    clippy::too_many_lines,
    reason = "every one of the 67 operation kinds declares prior-state copies in one exhaustive match"
)]
fn operation_copy_bytes(
    document: &Document,
    operation: &crate::Operation,
) -> Result<u64, HistoryError> {
    use crate::Operation;
    let clip = |id| {
        document.clip(id).map_or(Ok(0), |c| {
            sum([size_of::<Clip>() as u64, clip_payload(c, true)?])
        })
    };
    match operation {
        Operation::RelinkAsset { asset, .. } => document
            .tracks
            .iter()
            .flat_map(|track| &track.clips)
            .filter(|source| source.asset == *asset)
            .try_fold(0, |largest, source| {
                clip(source.id).map(|bytes| largest.max(bytes))
            }),
        Operation::SetTrackMix { track, .. } | Operation::SetTrackAutomation { track, .. } => {
            document
                .audio_mix
                .tracks
                .iter()
                .find(|entry| entry.track == *track)
                .map_or(Ok(0), payload)
        }
        Operation::CopyClipAttributes {
            from_clip, to_clip, ..
        } => sum([
            clip(*from_clip)?
                .checked_mul(3)
                .ok_or(HistoryError::Capacity)?,
            clip(*to_clip)?
                .checked_mul(2)
                .ok_or(HistoryError::Capacity)?,
        ]),
        Operation::SplitClip { clip: id, .. }
        | Operation::TrimClip { clip: id, .. }
        | Operation::SlipClip { clip: id, .. }
        | Operation::ReplaceClip { clip: id, .. }
        | Operation::FitToFill { clip: id, .. }
        | Operation::SetClipSpeed { clip: id, .. }
        | Operation::ConvertLegacyLook { clip: id, .. }
        | Operation::SetTitleParam { clip: id, .. }
        | Operation::SetEffectParam { clip: id, .. }
        | Operation::SetEffectKeyframes { clip: id, .. }
        | Operation::UpsertEffectKeyframe { clip: id, .. }
        | Operation::RemoveEffectKeyframe { clip: id, .. } => {
            clip(*id)?.checked_mul(4).ok_or(HistoryError::Capacity)
        }
        Operation::ThreePointEdit { .. }
        | Operation::PatchedThreePointEdit { .. }
        | Operation::RollEdit { .. }
        | Operation::SlideClip { .. }
        | Operation::RippleInsertGap { .. }
        | Operation::RippleDeleteClip { .. }
        | Operation::SetAssetColorDescription { .. } => document_live_bytes(document)?
            .checked_mul(4)
            .ok_or(HistoryError::Capacity),
        Operation::AddAsset { .. }
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
        | Operation::AddTrack { .. }
        | Operation::RemoveTrack { .. }
        | Operation::SetTrackSyncLock { .. }
        | Operation::AddClip { .. }
        | Operation::AddTitle { .. }
        | Operation::MoveClip { .. }
        | Operation::DeleteClip { .. }
        | Operation::LinkClips { .. }
        | Operation::UnlinkClips { .. }
        | Operation::AddMarker { .. }
        | Operation::RemoveMarker { .. }
        | Operation::MoveMarker { .. }
        | Operation::AddEffect { .. }
        | Operation::InsertEffect { .. }
        | Operation::RemoveEffect { .. }
        | Operation::ClearEffectKeyframes { .. }
        | Operation::SetEffectEnabled { .. }
        | Operation::SetClipEnabled { .. }
        | Operation::SetClipEnabledCurve { .. }
        | Operation::AddLutAsset { .. }
        | Operation::RemoveLutAsset { .. }
        | Operation::SetClipAudio { .. }
        | Operation::SetClipGainEnvelope { .. }
        | Operation::AddTransition { .. }
        | Operation::RemoveTransition { .. }
        | Operation::SetMarkerParam { .. }
        | Operation::AddFreezeFrame { .. }
        | Operation::SetClipBlendMode { .. }
        | Operation::AddAdjustmentClip { .. }
        | Operation::AddSolidClip { .. }
        | Operation::SetSolidColor { .. } => Ok(0),
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct RedoInputMax {
    pub(crate) document: u64,
    pub(crate) encoded: u64,
}
pub(crate) fn redo_input_max(history: &HistorySnapshot) -> Result<RedoInputMax, HistoryError> {
    let mut result = RedoInputMax {
        document: 0,
        encoded: 0,
    };
    for document in std::iter::once(&history.document).chain(
        history
            .cursor
            .undo
            .iter()
            .chain(&history.cursor.redo)
            .map(|entry| &entry.document),
    ) {
        result.document = result.document.max(transport_document_bytes(document)?);
        result.encoded = result.encoded.max(crate::history::encoded_size(document)?);
    }
    Ok(result)
}

/// A conservative reservation for every cut reached by draining Redo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RedoCapacityBound {
    pub replay_bytes: u64,
    pub retained_bytes: u64,
    pub encoded_batch_bytes: u64,
    pub max_current_document_bytes: u64,
}

fn decimal_range_bytes(start: usize, count: usize) -> Result<u64, HistoryError> {
    let end = start.checked_add(count).ok_or(HistoryError::Capacity)?;
    let mut bytes = count as u64;
    let mut power = 10_usize;
    while power < end {
        bytes = sum([bytes, end.saturating_sub(power.max(start)) as u64])?;
        power = power.checked_mul(10).ok_or(HistoryError::Capacity)?;
    }
    Ok(bytes)
}

/// Encoded size of the additional Redo batches, including array commas.
/// # Errors
/// Refuses arithmetic or the unchanged command-count bound.
pub fn redo_batch_encoded_bytes(history: &HistorySnapshot) -> Result<u64, HistoryError> {
    let count = history.cursor.redo.len();
    let batches = history.batches.len();
    if batches
        .checked_add(count)
        .is_none_or(|end| end > crate::HISTORY_COMMANDS)
    {
        return Err(HistoryError::Capacity);
    }
    let zero = crate::JournalBatch {
        order: 0,
        command: crate::JournalCommand::Redo,
        identity_effects: vec![],
        undo_entries: 0,
        redo_entries: 0,
    };
    let constant = crate::history::encoded_size(&zero)? - 3 + u64::from(batches > 0);
    sum([
        constant
            .checked_mul(count as u64)
            .ok_or(HistoryError::Capacity)?,
        decimal_range_bytes(batches, count)?,
        decimal_range_bytes(
            history
                .cursor
                .undo
                .len()
                .checked_add(1)
                .ok_or(HistoryError::Capacity)?,
            count,
        )?,
        decimal_range_bytes(0, count)?,
    ])
}

fn canonical_slots<T>(count: usize) -> Result<usize, HistoryError> {
    if count == 0 {
        return Ok(0);
    }
    let minimum = if size_of::<T>() == 1 {
        8
    } else if size_of::<T>() <= 1024 {
        4
    } else {
        1
    };
    Ok(count
        .checked_next_power_of_two()
        .ok_or(HistoryError::Capacity)?
        .max(minimum))
}

// Redo only moves the existing current/cursor document and operation payloads.
// Reserve the final batch count, both cursor-array maxima and the largest input
// document charge over the ENTIRE current cursor. Those quantities and the
// prefix's dynamic peak are invariant while Redo drains, so recomputing this
// reservation cannot newly refuse a later Redo. No future snapshot is built.
#[allow(
    clippy::too_many_lines,
    reason = "one checked derivation covers replay input, intermediate arrays and retained history"
)]
pub(crate) fn redo_capacity_bound(
    history: &HistorySnapshot,
    dynamic_peak: u64,
    journal: &crate::HistoryJournal,
    input_history: &HistorySnapshot,
    canonical_input: RedoInputMax,
) -> Result<RedoCapacityBound, HistoryError> {
    let extra = redo_batch_encoded_bytes(history)?;
    let count = history.cursor.redo.len();
    let total = history
        .cursor
        .undo
        .len()
        .checked_add(count)
        .ok_or(HistoryError::Capacity)?;
    let future = history
        .batches
        .len()
        .checked_add(count)
        .ok_or(HistoryError::Capacity)?;
    let current_encoded = crate::history::encoded_size(&history.document)?;
    let current_document = transport_document_bytes(&history.document)?;
    let mut max_encoded = current_encoded.max(canonical_input.encoded);
    let mut max_document = current_document.max(canonical_input.document);
    // Replayed and live documents can have equal values but different spare
    // allocations. Retain both charges; Redo may make any live boundary current.
    for document in std::iter::once(&input_history.document).chain(
        input_history
            .cursor
            .undo
            .iter()
            .chain(&input_history.cursor.redo)
            .chain(history.cursor.undo.iter().chain(&history.cursor.redo))
            .map(|entry| &entry.document),
    ) {
        max_encoded = max_encoded.max(crate::history::encoded_size(document)?);
        max_document = max_document.max(transport_document_bytes(document)?);
    }
    let mut input =
        ReplayBudget::new(journal, &history.baseline, &history.document, u64::MAX)?.input;
    let old_batches = canonical_slots::<crate::HistoryBatch>(history.batches.len())?;
    let new_batches = canonical_slots::<crate::HistoryBatch>(future)?;
    let journal_slots = canonical_slots::<crate::JournalBatch>(future)?;
    let cursor_slots = canonical_slots::<String>(total)?;
    let normalize_commas = u64::from(!history.cursor.undo.is_empty() && count > 0);
    let subtract = sum([
        current_document,
        current_encoded
            .checked_mul(2)
            .ok_or(HistoryError::Capacity)?,
        transport_vector_bytes(&journal.batches)?,
        transport_vector_bytes(&journal.cursor.undo)?,
        transport_vector_bytes(&journal.cursor.redo)?,
        allocation::<crate::HistoryBatch>(old_batches)?,
        allocation::<crate::actor::HistoryEntry>(old_batches)?
            .checked_mul(2)
            .ok_or(HistoryError::Capacity)?,
    ])?;
    input = input
        .checked_sub(subtract)
        .ok_or(HistoryError::CommitInvariant)?;
    input = sum([
        input,
        max_document,
        max_encoded.checked_mul(2).ok_or(HistoryError::Capacity)?,
        sum([extra, normalize_commas])?
            .checked_mul(2)
            .ok_or(HistoryError::Capacity)?,
        allocation::<crate::JournalBatch>(journal_slots)?,
        allocation::<String>(cursor_slots)?
            .checked_mul(2)
            .ok_or(HistoryError::Capacity)?,
        allocation::<crate::HistoryBatch>(new_batches)?,
        allocation::<crate::actor::HistoryEntry>(new_batches)?
            .checked_mul(2)
            .ok_or(HistoryError::Capacity)?,
    ])?;
    // Reject the portable encoded bound at admission too, before a future Redo
    // could reach the counter in ReplayBudget::new.
    let encoded = sum([
        crate::history::encoded_size(&(&history.baseline, &history.document, &journal))?,
        max_encoded - current_encoded,
        extra,
        normalize_commas,
    ])?;
    if encoded > crate::HISTORY_BYTES {
        return Err(HistoryError::Capacity);
    }
    let old_slots = history
        .batches
        .capacity()
        .max(canonical_slots::<crate::HistoryBatch>(
            history.batches.len(),
        )?);
    let new_slots = old_slots.max(new_batches);
    let old_undo_peak = history
        .batches
        .iter()
        .map(|b| b.undo_entries)
        .max()
        .unwrap_or(0)
        .max(history.cursor.undo.len());
    let undo_slots = history
        .cursor
        .undo
        .capacity()
        .max(canonical_slots::<crate::GestureBoundary>(old_undo_peak)?);
    let future_undo_slots = undo_slots.max(canonical_slots::<crate::GestureBoundary>(total)?);
    let retained = sum([
        restored_history_base_bytes(history)?,
        allocation::<crate::HistoryBatch>(new_slots - old_slots)?,
        allocation::<crate::GestureBoundary>(future_undo_slots - undo_slots)?,
        payload(&crate::JournalCommand::Redo)?
            .checked_mul(2)
            .and_then(|n| n.checked_mul(count as u64))
            .ok_or(HistoryError::Capacity)?,
    ])?;
    let retained_bytes = with_history_margin(retained)?;
    Ok(RedoCapacityBound {
        replay_bytes: with_history_margin(sum([input, dynamic_peak])?)?.max(retained_bytes),
        retained_bytes,
        encoded_batch_bytes: extra,
        max_current_document_bytes: max_encoded,
    })
}
