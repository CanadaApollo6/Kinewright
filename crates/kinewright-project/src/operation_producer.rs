//! Actor-side bounded outbox. Encode before acceptance, retain shared immutable
//! bytes afterwards, and advance nothing when admission or the edit is refused.
use super::{
    Action, Conflict, FoldedBatch, Message, PENDING_BYTES, PENDING_COMMANDS, PRODUCER_BUFFER_PEAK,
    PRODUCER_HEADER_BYTES, Publisher, Receipt, StreamState, foldable, hash,
};
use crate::{
    bundle_records::{HeadId, ObjectRef},
    operation_actor::CommandContext,
    operation_history::{
        CreationKind, Error as HistoryError, LeaseHold, OwnerHistory, SnapshotIdentity,
    },
    receipts::{CommandId, IssuerId},
};
use kinewright_core::{
    Command, Operation,
    undo::{CommandKey, Effects},
};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use std::{
    collections::VecDeque,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Lease {
    pub state: StreamState,
    pub snapshot: SnapshotIdentity,
    pub next_key: CommandKey,
    pub issuer: IssuerId,
    pub next_issuer_sequence: u64,
    pub generation: u64,
    pub client: String,
    pub token: String,
    pub switch_generation: u64,
    pub owner_epoch: u64,
    pub anchor_head: HeadId,
    pub inputs: ObjectRef,
    pub parent_identity: Option<(u64, u64)>,
    pub namespace_name: Option<String>,
}
impl std::fmt::Debug for Lease {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Lease")
            .field("state", &self.state)
            .field("generation", &self.generation)
            .field("client", &self.client)
            .finish_non_exhaustive()
    }
}
const ENCODED_HEADER_BYTES: u64 = 128;
struct Arena {
    used: AtomicU64,
    limit: u64,
}
struct ByteCharge {
    arena: Arc<Arena>,
    bytes: u64,
    _hold: LeaseHold,
}
impl Drop for ByteCharge {
    fn drop(&mut self) {
        self.arena.used.fetch_sub(self.bytes, Ordering::Relaxed);
    }
}
impl Arena {
    fn reserve(self: &Arc<Self>, bytes: u64, hold: &LeaseHold) -> Result<ByteCharge, Conflict> {
        #[allow(deprecated, reason = "MSRV predates AtomicU64::try_update")]
        self.used
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |old| {
                old.checked_add(bytes).filter(|next| *next <= self.limit)
            })
            .map_err(|_| Conflict::Backpressure)?;
        Ok(ByteCharge {
            arena: Arc::clone(self),
            bytes,
            _hold: hold.clone(),
        })
    }
}
struct EncodedPayload {
    raw: Box<RawValue>,
    // Fields drop in order: free bytes before returning their reservation.
    _charge: Option<ByteCharge>,
}
#[derive(Clone)]
pub struct EncodedMessage(Arc<EncodedPayload>);
impl PartialEq for EncodedMessage {
    fn eq(&self, other: &Self) -> bool {
        self.0.raw.get() == other.0.raw.get()
    }
}
impl Eq for EncodedMessage {}
impl std::fmt::Debug for EncodedMessage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EncodedMessage")
            .field("bytes", &self.byte_len())
            .finish()
    }
}
impl Serialize for EncodedMessage {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        crate::operation_wire::serialize(&self.0.raw, serializer)
    }
}
impl<'de> Deserialize<'de> for EncodedMessage {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // Incoming transport has its own caller's ingress reservation. It can
        // never acquire or forge an outgoing actor's lease hold.
        let raw = crate::operation_wire::deserialize(deserializer)?;
        Ok(Self(Arc::new(EncodedPayload { raw, _charge: None })))
    }
}
impl EncodedMessage {
    fn decoded_bound(&self) -> Result<u64, Conflict> {
        let bytes = self.0.raw.get().as_bytes();
        if bytes.len() as u64 > PENDING_BYTES {
            return Err(Conflict::Backpressure);
        }
        let bound =
            kinewright_core::undo::payload_memory_bound(bytes).map_err(|_| Conflict::Encoding)?;
        if bound > kinewright_core::undo::DECODED_PAYLOAD_BYTES {
            return Err(Conflict::Backpressure);
        }
        Ok(bound)
    }
    pub fn decode(&self) -> Result<Message, Conflict> {
        self.decoded_bound()?;
        self.decode_checked()
    }
    fn decode_checked(&self) -> Result<Message, Conflict> {
        let message: Message =
            serde_json::from_str(self.0.raw.get()).map_err(|_| Conflict::Encoding)?;
        message.validate_fold()?;
        Ok(message)
    }
    #[must_use]
    pub fn byte_len(&self) -> u64 {
        self.0.raw.get().len() as u64
    }
}
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(tag = "action")]
pub enum ActionRef<'a> {
    Do {
        operations: &'a [Operation],
        gesture: Option<&'a str>,
    },
    Undo,
    Redo,
    Close,
}
impl<'a> ActionRef<'a> {
    pub fn from_command(command: &'a Command) -> Result<Self, Conflict> {
        Ok(match command {
            Command::Do(operation) => Self::Do {
                operations: std::slice::from_ref(operation),
                gesture: None,
            },
            Command::DoBatch(operations) => Self::Do {
                operations,
                gesture: None,
            },
            Command::DoBatchCoalesced {
                operations,
                coalesce_key,
            } => Self::Do {
                operations,
                gesture: Some(coalesce_key),
            },
            Command::Undo => Self::Undo,
            Command::Redo => Self::Redo,
            Command::Query(_)
            | Command::DoIfRevision { .. }
            | Command::DoBatchIfRevision { .. } => return Err(Conflict::Unnormalised),
        })
    }
}
impl<'a> From<&'a Action> for ActionRef<'a> {
    fn from(action: &'a Action) -> Self {
        match action {
            Action::Do {
                operations,
                gesture,
            } => Self::Do {
                operations,
                gesture: gesture.as_deref(),
            },
            Action::Undo => Self::Undo,
            Action::Redo => Self::Redo,
            Action::Close => Self::Close,
        }
    }
}
#[derive(Clone, Copy, Serialize)]
struct BatchRef<'a> {
    creation: &'a crate::operation_history::CreationOrigin,
    operations: &'a [Operation],
}

#[derive(Default)]
struct Folded<'a> {
    previous: &'a [FoldedBatch],
    last: Option<BatchRef<'a>>,
}
impl Folded<'_> {
    fn is_empty(&self) -> bool {
        self.previous.is_empty() && self.last.is_none()
    }
}
impl Serialize for Folded<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(
            self.previous
                .iter()
                .map(|part| BatchRef {
                    creation: &part.creation,
                    operations: &part.operations,
                })
                .chain(self.last),
        )
    }
}

#[derive(Serialize)]
struct Wire<'a> {
    publisher: Publisher,
    id: CommandId,
    sequence: u64,
    predecessor: [u8; 32],
    tip: [u8; 32],
    accepted: CommandKey,
    action: ActionRef<'a>,
    context: &'a CommandContext,
    #[serde(skip_serializing_if = "Folded::is_empty")]
    folded: Folded<'a>,
}
impl Wire<'_> {
    fn encoded_len(&self) -> Result<u64, Conflict> {
        struct Counter(u64);
        impl std::io::Write for Counter {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0 = self
                    .0
                    .checked_add(bytes.len() as u64)
                    .ok_or_else(|| std::io::Error::other("message size"))?;
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut counter = Counter(0);
        serde_json::to_writer(&mut counter, self).map_err(|_| Conflict::Encoding)?;
        Ok(counter.0)
    }
}
struct Pending {
    encoded: EncodedMessage,
    receipt: Receipt,
    sent: bool,
}
pub(crate) struct PreparedMessage {
    pending: Pending,
    next_state: StreamState,
    next_issuer: u64,
    replace_tail: bool,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProducerError {
    Protocol(Conflict),
    History(HistoryError),
}
impl From<Conflict> for ProducerError {
    fn from(error: Conflict) -> Self {
        Self::Protocol(error)
    }
}
impl From<HistoryError> for ProducerError {
    fn from(error: HistoryError) -> Self {
        Self::History(error)
    }
}
impl std::fmt::Display for ProducerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Protocol(error) => error.fmt(f),
            Self::History(error) => error.fmt(f),
        }
    }
}
impl std::error::Error for ProducerError {}
pub struct Producer {
    lease: Lease,
    state: StreamState,
    next_issuer: u64,
    next_key: CommandKey,
    attached: bool,
    arena: Arc<Arena>,
    hold: Option<LeaseHold>,
    pending: VecDeque<Pending>,
    bytes: u64,
    acknowledged: VecDeque<Receipt>,
}
impl Producer {
    pub fn bind(lease: Lease, owner: &mut OwnerHistory) -> Result<Self, ProducerError> {
        if owner.creation_sizing().kind != CreationKind::Ordinary {
            return Err(HistoryError::InvalidPolicy.into());
        }
        if lease.snapshot != owner.identity()? || lease.next_key != owner.history().position() {
            return Err(Conflict::Snapshot.into());
        }
        let producer = Self::new(lease, owner.retain_memory())?;
        owner.reserve_stream_buffer(PRODUCER_BUFFER_PEAK)?;
        Ok(producer)
    }
    /// An installation rotates authority and stream state. Retained senders
    /// stay inside the same byte arena rather than acquiring another allowance.
    pub(crate) fn rebind(
        &mut self,
        lease: Lease,
        owner: &OwnerHistory,
    ) -> Result<(), ProducerError> {
        if !self.pending.is_empty() {
            return Err(Conflict::Backpressure.into());
        }
        if lease.snapshot != owner.identity()? || lease.next_key != owner.history().position() {
            return Err(Conflict::Snapshot.into());
        }
        let mut replacement =
            Self::new(lease, self.hold.as_ref().ok_or(Conflict::Detached)?.clone())?;
        replacement.arena = Arc::clone(&self.arena);
        *self = replacement;
        Ok(())
    }
    fn new(lease: Lease, hold: LeaseHold) -> Result<Self, Conflict> {
        lease.state.validate()?;
        lease
            .issuer
            .validate()
            .map_err(|_| Conflict::CommandIdentity)?;
        lease
            .inputs
            .validate()
            .map_err(|_| Conflict::CommandIdentity)?;
        if lease.next_issuer_sequence == 0
            || lease.generation == 0
            || lease.client.len() > 256
            || lease.token.is_empty()
            || lease.token.len() > 256
        {
            return Err(Conflict::CommandIdentity);
        }
        // The source can also pass an in-process lease with reserved scalar
        // or deque capacity. Charge that real header shape before retaining it.
        let header = std::mem::size_of::<Lease>()
            .saturating_add(lease.client.capacity())
            .saturating_add(lease.token.capacity())
            .saturating_add(lease.namespace_name.as_ref().map_or(0, String::capacity))
            .saturating_add(lease.issuer.0.capacity())
            .saturating_add(lease.anchor_head.allocated_capacity())
            .saturating_add(lease.inputs.sha256.capacity())
            .saturating_add(
                lease
                    .state
                    .receipts
                    .capacity()
                    .saturating_mul(std::mem::size_of::<Receipt>()),
            )
            .saturating_add(
                lease
                    .state
                    .receipts
                    .iter()
                    .map(|receipt| receipt.id.issuer.0.capacity())
                    .sum::<usize>(),
            )
            .saturating_add(lease.state.seed.project.allocated_capacity())
            .saturating_add(lease.state.seed.branch.allocated_capacity());
        if header > 32 << 10 {
            return Err(Conflict::Backpressure);
        }
        let mut pending = VecDeque::new();
        let mut acknowledged = VecDeque::new();
        pending
            .try_reserve_exact(PENDING_COMMANDS)
            .map_err(|_| Conflict::Backpressure)?;
        acknowledged
            .try_reserve_exact(PENDING_COMMANDS)
            .map_err(|_| Conflict::Backpressure)?;
        Ok(Self {
            state: lease.state.clone(),
            next_issuer: lease.next_issuer_sequence,
            next_key: lease.next_key,
            attached: true,
            arena: Arc::new(Arena {
                used: AtomicU64::new(0),
                limit: PRODUCER_BUFFER_PEAK - PRODUCER_HEADER_BYTES,
            }),
            hold: Some(hold),
            lease,
            pending,
            acknowledged,
            bytes: 0,
        })
    }
    pub(crate) const fn position(&self) -> CommandKey {
        self.next_key
    }
    #[must_use]
    pub const fn lease(&self) -> &Lease {
        &self.lease
    }
    #[must_use]
    pub const fn state(&self) -> &StreamState {
        &self.state
    }
    #[must_use]
    pub const fn pending_bytes(&self) -> u64 {
        self.bytes
    }
    #[must_use]
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }
    #[must_use]
    /// Export immutable wire identities to a sender or recovery owner.
    pub fn pending(&mut self) -> Vec<EncodedMessage> {
        self.pending
            .iter_mut()
            .map(|pending| {
                pending.sent = true;
                pending.encoded.clone()
            })
            .collect()
    }
    pub fn execute(
        &mut self,
        owner: &mut OwnerHistory,
        action: Action,
        context: CommandContext,
    ) -> Result<Effects, ProducerError> {
        self.execute_with_inputs(owner, action, context, None)
    }
    pub(crate) fn execute_with_inputs(
        &mut self,
        owner: &mut OwnerHistory,
        action: Action,
        mut context: CommandContext,
        certificate: Option<&crate::operation_inputs::ManualInputs>,
    ) -> Result<Effects, ProducerError> {
        if !self.attached {
            return Err(Conflict::Detached.into());
        }
        // Core's Empty answer precedes every capacity and outbox admission.
        if matches!(&action, Action::Do { operations, .. } if operations.is_empty()) {
            return action.execute(context, owner).map_err(ProducerError::from);
        }
        if owner.history().position() != self.next_key {
            return Err(Conflict::AcceptedKey.into());
        }
        if let Action::Do { operations, .. } = &action
            && certificate.is_some()
        {
            if !context.inputs.is_empty() {
                return Err(HistoryError::InvalidOrigin.into());
            }
            if crate::operation_inputs::ManualInputs::imports(operations) {
                context.inputs.push(self.lease.inputs.clone());
            }
        }
        let wire_action = ActionRef::from(&action);
        let open_gesture = owner
            .history()
            .undo()
            .back()
            .and_then(kinewright_core::undo::Step::coalesce_key);
        let prepared = match self.prepare_fold(wire_action, &context, open_gesture)? {
            Some(prepared) => prepared,
            None => self.prepare(self.next_key, wire_action, &context)?,
        };
        let effects = action.execute_with_inputs(context, owner, certificate)?;
        self.commit(prepared);
        self.next_key = owner.history().position();
        Ok(effects)
    }
    pub fn detach(&mut self, owner: &mut OwnerHistory) -> Result<(), ProducerError> {
        if !self.pending.is_empty() || self.arena.used.load(Ordering::Relaxed) != 0 {
            return Err(Conflict::Backpressure.into());
        }
        owner.release_stream_buffer()?;
        self.attached = false;
        self.hold.take();
        Ok(())
    }
    fn prepare_fold(
        &self,
        action: ActionRef<'_>,
        context: &CommandContext,
        open_gesture: Option<&str>,
    ) -> Result<Option<PreparedMessage>, Conflict> {
        let ActionRef::Do {
            operations,
            gesture: Some(key),
        } = action
        else {
            return Ok(None);
        };
        let Some(tail) = self.pending.back().filter(|tail| !tail.sent) else {
            return Ok(None);
        };
        if open_gesture != Some(key) || !foldable(context, operations) {
            return Ok(None);
        }
        // The existing stream arena also covers temporary re-encoding. If it
        // cannot hold the decoded tail, fall back to an ordinary slot.
        let hold = self.hold.as_ref().ok_or(Conflict::Detached)?;
        let Ok(decoded_bound) = tail.encoded.decoded_bound() else {
            return Ok(None);
        };
        let Ok(_decoded) = self.arena.reserve(decoded_bound, hold) else {
            return Ok(None);
        };
        let old = tail.encoded.decode_checked()?;
        let Action::Do {
            operations: first,
            gesture: Some(old_key),
        } = &old.action
        else {
            return Ok(None);
        };
        if old_key != key
            || old.context.creation.branch != context.creation.branch
            || !foldable(&old.context, first)
        {
            return Ok(None);
        }
        let wire = Wire {
            publisher: old.publisher,
            id: old.id,
            sequence: old.sequence,
            predecessor: old.predecessor,
            tip: [0; 32],
            accepted: old.accepted,
            action: ActionRef::from(&old.action),
            context: &old.context,
            folded: Folded {
                previous: &old.folded,
                last: Some(BatchRef {
                    creation: &context.creation,
                    operations,
                }),
            },
        };
        match self.encode(wire, true) {
            Ok(prepared) => Ok(Some(prepared)),
            Err(Conflict::Backpressure | Conflict::CommandTooLarge) => Ok(None),
            Err(error) => Err(error),
        }
    }

    pub(crate) fn prepare(
        &self,
        accepted: CommandKey,
        action: ActionRef<'_>,
        context: &CommandContext,
    ) -> Result<PreparedMessage, Conflict> {
        if self.pending.len() == PENDING_COMMANDS {
            return Err(Conflict::Backpressure);
        }
        let publisher = match (
            self.lease.parent_identity,
            self.lease.namespace_name.as_deref(),
        ) {
            (Some(parent), Some(name)) => Publisher::native(parent, name)?,
            (None, None) => Publisher::Portable,
            _ => return Err(Conflict::Detached),
        };
        self.encode(
            Wire {
                publisher,
                id: CommandId {
                    issuer: self.lease.issuer.clone(),
                    sequence: self.next_issuer,
                },
                sequence: self
                    .state
                    .sequence
                    .checked_add(1)
                    .ok_or(Conflict::CounterExhausted)?,
                predecessor: self.state.tip,
                tip: [0; 32],
                accepted,
                action,
                context,
                folded: Folded::default(),
            },
            false,
        )
    }

    fn encode(&self, mut wire: Wire<'_>, replace_tail: bool) -> Result<PreparedMessage, Conflict> {
        let header = (
            wire.publisher,
            &wire.id,
            wire.accepted,
            wire.action,
            wire.context,
        );
        let command = if wire.folded.is_empty() {
            hash(b"kinewright/accepted-command/v1\0", &header)?
        } else {
            hash(
                b"kinewright/folded-manual-command/v1\0",
                &(header, &wire.folded),
            )?
        };
        wire.tip = hash(
            b"kinewright/accepted-chain/v1\0",
            &(wire.predecessor, command),
        )?;
        let encoded_len = wire.encoded_len()?;
        if encoded_len > PENDING_BYTES {
            return Err(Conflict::CommandTooLarge);
        }
        let retained = if replace_tail {
            self.bytes
                - self
                    .pending
                    .back()
                    .expect("replaced tail")
                    .encoded
                    .byte_len()
        } else {
            self.bytes
        };
        if encoded_len > PENDING_BYTES.saturating_sub(retained) {
            return Err(Conflict::Backpressure);
        }
        let length = usize::try_from(encoded_len).map_err(|_| Conflict::Backpressure)?;
        let hold = self.hold.as_ref().ok_or(Conflict::Detached)?;
        let encoding = self.arena.reserve(encoded_len, hold)?;
        let charge = self.arena.reserve(
            encoded_len
                .checked_add(ENCODED_HEADER_BYTES)
                .ok_or(Conflict::Backpressure)?,
            hold,
        )?;
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(length)
            .map_err(|_| Conflict::Backpressure)?;
        serde_json::to_writer(&mut bytes, &wire).map_err(|_| Conflict::Encoding)?;
        if replace_tail
            && kinewright_core::undo::payload_memory_bound(&bytes)
                .map_err(|_| Conflict::Encoding)?
                > kinewright_core::undo::DECODED_PAYLOAD_BYTES
        {
            return Err(Conflict::CommandTooLarge);
        }
        let text = String::from_utf8(bytes).map_err(|_| Conflict::Encoding)?;
        let raw = RawValue::from_string(text).map_err(|_| Conflict::Encoding)?;
        let receipt = Receipt {
            id: wire.id,
            sequence: wire.sequence,
            accepted: wire.accepted,
            tip: wire.tip,
            command,
            additional_batches: wire.folded.previous.len() as u64
                + u64::from(wire.folded.last.is_some()),
        };
        let next_issuer = self
            .next_issuer
            .checked_add(u64::from(!replace_tail))
            .ok_or(Conflict::CounterExhausted)?;
        let mut next_state = self.state.clone();
        if replace_tail {
            next_state.replace_tail_receipt(receipt.clone());
        } else {
            next_state.record_receipt(receipt.clone());
        }
        let encoded = EncodedMessage(Arc::new(EncodedPayload {
            raw,
            _charge: Some(charge),
        }));
        drop(encoding);
        Ok(PreparedMessage {
            pending: Pending {
                encoded,
                receipt,
                sent: false,
            },
            next_state,
            next_issuer,
            replace_tail,
        })
    }
    /// Called by the sole actor only after its already-prepared edit succeeds.
    pub(crate) fn commit(&mut self, prepared: PreparedMessage) {
        if prepared.replace_tail {
            let previous = self.pending.pop_back().expect("replaced tail");
            self.bytes -= previous.encoded.byte_len();
        }
        self.bytes += prepared.pending.encoded.byte_len();
        self.pending.push_back(prepared.pending);
        self.state = prepared.next_state;
        self.next_issuer = prepared.next_issuer;
    }
    pub fn acknowledge(&mut self, receipts: &[Receipt]) -> Result<(), Conflict> {
        let mut consume = 0;
        let mut bytes = self.bytes;
        for receipt in receipts {
            if self.acknowledged.iter().any(|old| old == receipt) {
                continue;
            }
            let next = self.pending.get(consume).ok_or(Conflict::ReceiptWindow)?;
            if next.receipt != *receipt {
                return Err(Conflict::CommandIdentity);
            }
            bytes = bytes
                .checked_sub(next.encoded.byte_len())
                .ok_or(Conflict::ReceiptWindow)?;
            consume += 1;
        }
        for pending in self.pending.drain(..consume) {
            if self.acknowledged.len() == PENDING_COMMANDS {
                self.acknowledged.pop_front();
            }
            self.acknowledged.push_back(pending.receipt);
        }
        self.bytes = bytes;
        Ok(())
    }
}

#[cfg(test)]
#[path = "operation_producer_tests.rs"]
mod tests;
