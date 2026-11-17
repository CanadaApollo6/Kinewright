//! Payload, document and per-operation workspace reservations for native replay.
use crate::undo::Error as BudgetError;
use crate::{Clip, ClipContent};
use serde::Serialize;
use std::mem::size_of;

fn sum(values: impl IntoIterator<Item = u64>) -> Result<u64, BudgetError> {
    values.into_iter().try_fold(0_u64, |total, bytes| {
        total.checked_add(bytes).ok_or(BudgetError::Capacity)
    })
}
fn allocation<T>(capacity: usize) -> Result<u64, BudgetError> {
    (capacity as u64)
        .checked_mul(size_of::<T>() as u64)
        .ok_or(BudgetError::Capacity)
}
fn payload(value: &impl Serialize) -> Result<u64, BudgetError> {
    let bytes = serde_json::to_vec(value).map_err(|_| BudgetError::Encoding)?;
    #[cfg(feature = "history-metrics")]
    crate::undo::metrics::payload_serialized(bytes.len() as u64);
    payload_memory_bound(&bytes)
}

// The same conservative structural costs as store preflight, for generated JSON.
// This streaming scan allocates no intermediate JSON tree during live admission.
pub(crate) fn payload_memory_bound(bytes: &[u8]) -> Result<u64, BudgetError> {
    use serde::de::{DeserializeSeed, Error, MapAccess, SeqAccess, Visitor};
    use std::{cell::Cell, fmt};
    struct Scan<'a>(&'a Cell<u64>, u32);
    impl Scan<'_> {
        fn add<E: Error>(&self, amount: u64) -> Result<(), E> {
            let next = self
                .0
                .get()
                .checked_add(amount)
                .filter(|value| *value <= crate::undo::DECODED_PAYLOAD_BYTES)
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
        .filter(|value| *value <= crate::undo::DECODED_PAYLOAD_BYTES)
        .ok_or(BudgetError::Capacity)?;
    let charge = Cell::new(initial);
    let mut parser = serde_json::Deserializer::from_slice(bytes);
    Scan(&charge, 0)
        .deserialize(&mut parser)
        .map_err(|_| BudgetError::Capacity)?;
    parser.end().map_err(|_| BudgetError::InvalidRecord)?;
    Ok(charge.get())
}

fn clip_payload(clip: &Clip) -> Result<u64, BudgetError> {
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

// Document's inline size already includes the complete colour context. A new
// owned field must be charged here instead of silently inheriting this omission.
const _: () = assert!(
    !std::mem::needs_drop::<crate::ColorContext>(),
    "colour context gained owned storage; include it in the live history bound"
);

// A decoded command is typed Rust data, not a retained serde_json::Value tree.
// Count its arrays directly; only genuinely owned command payloads use the
// conservative structural fallback. Prior-document copies remain independent.
fn inline_command_bytes<T: Copy>(_: T) -> u64 {
    0
}
pub(crate) fn operation_owned_bytes(operation: &crate::Operation) -> Result<u64, BudgetError> {
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
fn transport_clip_payload(clip: &Clip) -> Result<u64, BudgetError> {
    let mut bytes = sum([clip_payload(clip)?, transport_padding(&clip.effects)?])?;
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
) -> Result<u64, BudgetError> {
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
    kernel_peak: u64,
}
impl ReplayBudget {
    /// A streaming replay owns one scratch document, not retained boundary snapshots.
    pub(crate) fn workspace(limit: u64) -> Result<Self, BudgetError> {
        let mut budget = Self {
            limit,
            documents: 0,
            input: 0,
            identities: 0,
            dynamic_peak: 0,
            kernel_peak: 0,
        };
        budget.check(0)?;
        Ok(budget)
    }

    pub(crate) fn reserve_input(&mut self, bytes: u64) -> Result<(), BudgetError> {
        self.input = sum([self.input, bytes])?;
        self.check(0)
    }
    pub(crate) const fn identity_charge(&self) -> u64 {
        self.identities
    }
    pub(crate) fn workspace_peak_without(&self, input: u64) -> Result<u64, BudgetError> {
        let raw = sum([
            self.input.checked_sub(input).ok_or(BudgetError::Capacity)?,
            self.dynamic_peak,
        ])?;
        sum([raw, raw.div_ceil(10)])
    }
    /// Kernel scratch alone, so a later one-batch merge adds all observations
    /// exactly once instead of counting the largest step's observations twice.
    pub(crate) fn kernel_peak_with_margin(&self) -> Result<u64, BudgetError> {
        sum([self.kernel_peak, self.kernel_peak.div_ceil(10)])
    }
    /// A private, already-margined step peak is recomputed at admission/open.
    /// Add the current replay's staged documents and metadata without applying
    /// the safety margin to that step a second time.
    pub(crate) fn check_prevalidated_peak(&self, peak: u64) -> Result<(), BudgetError> {
        let held = sum([self.input, self.documents, self.identities])?;
        let held = sum([held, held.div_ceil(10)])?;
        if sum([held, peak])? > self.limit {
            return Err(BudgetError::Capacity);
        }
        Ok(())
    }
    fn check(&mut self, additional: u64) -> Result<(), BudgetError> {
        let kernel_peak = self.kernel_peak.max(sum([self.documents, additional])?);
        let dynamic_peak =
            self.dynamic_peak
                .max(sum([self.documents, self.identities, additional])?);
        let bytes = sum([self.input, dynamic_peak])?;
        let bytes = sum([bytes, bytes.div_ceil(10)])?;
        if bytes > self.limit {
            return Err(BudgetError::Capacity);
        }
        // A rejected conservative estimate must not poison a tighter retry.
        self.kernel_peak = kernel_peak;
        self.dynamic_peak = dynamic_peak;
        Ok(())
    }

    pub(crate) fn check_known(&mut self, bytes: u64) -> Result<(), BudgetError> {
        self.check(bytes)
    }
    pub(crate) fn retain_identity(
        &mut self,
        identity: &crate::OperationIdentityEffect,
    ) -> Result<(), BudgetError> {
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
    pub(crate) fn set_document_charge(&mut self, bytes: u64) -> Result<(), BudgetError> {
        self.documents = bytes;
        self.check(0)
    }
}

/// Existing per-kind workspace terms, using the caller's running document
/// bound. Native replay never measures a whole document for an operation.
#[allow(
    clippy::too_many_lines,
    reason = "exhaustive per-kind scratch reservation table"
)]
pub(crate) fn operation_workspace_bytes(
    operation: &crate::Operation,
    document_bound: u64,
) -> Result<u64, BudgetError> {
    use crate::Operation;
    match operation {
        Operation::RelinkAsset { .. }
        | Operation::SetTrackMix { .. }
        | Operation::SetTrackAutomation { .. } => Ok(document_bound),
        Operation::CopyClipAttributes { .. } => {
            document_bound.checked_mul(5).ok_or(BudgetError::Capacity)
        }
        Operation::SplitClip { .. }
        | Operation::TrimClip { .. }
        | Operation::SlipClip { .. }
        | Operation::ReplaceClip { .. }
        | Operation::FitToFill { .. }
        | Operation::SetClipSpeed { .. }
        | Operation::ConvertLegacyLook { .. }
        | Operation::SetTitleParam { .. }
        | Operation::SetEffectParam { .. }
        | Operation::SetEffectKeyframes { .. }
        | Operation::UpsertEffectKeyframe { .. }
        | Operation::RemoveEffectKeyframe { .. }
        | Operation::ThreePointEdit { .. }
        | Operation::PatchedThreePointEdit { .. }
        | Operation::RollEdit { .. }
        | Operation::SlideClip { .. }
        | Operation::RippleInsertGap { .. }
        | Operation::RippleDeleteClip { .. }
        | Operation::SetAssetColorDescription { .. } => {
            document_bound.checked_mul(4).ok_or(BudgetError::Capacity)
        }
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

fn transport_padding<T>(values: &Vec<T>) -> Result<u64, BudgetError> {
    replay_array_padding::<T>(values.len(), values.capacity())
}

fn transport_vector_bytes<T>(values: &Vec<T>) -> Result<u64, BudgetError> {
    sum([
        allocation::<T>(values.capacity())?,
        transport_padding(values)?,
    ])
}

fn replay_array_padding<T>(length: usize, retained_capacity: usize) -> Result<u64, BudgetError> {
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
        .ok_or(BudgetError::Capacity)?
        .max(minimum);
    allocation::<T>(capacity.saturating_sub(retained_capacity))
}
