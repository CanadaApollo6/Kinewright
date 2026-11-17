//! Actor-side bounded outbox. Encode before acceptance, retain shared immutable
//! bytes afterwards, and advance nothing when admission or the edit is refused.
use super::{
    Action, Conflict, Message, PENDING_BYTES, PENDING_COMMANDS, PRODUCER_BUFFER_PEAK,
    PRODUCER_HEADER_BYTES, Publisher, Receipt, StreamState, hash,
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
    pub fn decode(&self) -> Result<Message, Conflict> {
        let bytes = self.0.raw.get().as_bytes();
        if bytes.len() as u64 > PENDING_BYTES {
            return Err(Conflict::Backpressure);
        }
        if kinewright_core::undo::payload_memory_bound(bytes).map_err(|_| Conflict::Encoding)?
            > kinewright_core::undo::DECODED_PAYLOAD_BYTES
        {
            return Err(Conflict::Backpressure);
        }
        serde_json::from_slice(bytes).map_err(|_| Conflict::Encoding)
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
#[derive(Serialize)]
struct Wire<'a> {
    publisher: Publisher,
    id: &'a CommandId,
    sequence: u64,
    predecessor: [u8; 32],
    tip: [u8; 32],
    accepted: CommandKey,
    action: ActionRef<'a>,
    context: &'a CommandContext,
}
struct Pending {
    encoded: EncodedMessage,
    receipt: Receipt,
}
pub(crate) struct PreparedMessage {
    pending: Pending,
    next_state: StreamState,
    next_issuer: u64,
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
    pub fn pending(&self) -> Vec<EncodedMessage> {
        self.pending
            .iter()
            .map(|pending| pending.encoded.clone())
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
        let prepared = self.prepare(self.next_key, ActionRef::from(&action), &context)?;
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
    #[allow(
        clippy::too_many_lines,
        reason = "one atomic preparation reserves, hashes and encodes the exact accepted packet"
    )]
    pub(crate) fn prepare(
        &self,
        accepted: CommandKey,
        action: ActionRef<'_>,
        context: &CommandContext,
    ) -> Result<PreparedMessage, Conflict> {
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
        if self.pending.len() == PENDING_COMMANDS {
            return Err(Conflict::Backpressure);
        }
        let next_issuer = self
            .next_issuer
            .checked_add(1)
            .ok_or(Conflict::CounterExhausted)?;
        let id = CommandId {
            issuer: self.lease.issuer.clone(),
            sequence: self.next_issuer,
        };
        let sequence = self
            .state
            .sequence
            .checked_add(1)
            .ok_or(Conflict::CounterExhausted)?;
        let publisher = match (
            self.lease.parent_identity,
            self.lease.namespace_name.as_deref(),
        ) {
            (Some(parent), Some(name)) => Publisher::native(parent, name)?,
            (None, None) => Publisher::Portable,
            _ => return Err(Conflict::Detached),
        };
        let command = hash(
            b"kinewright/accepted-command/v1\0",
            &(publisher, &id, accepted, action, context),
        )?;
        let tip = hash(
            b"kinewright/accepted-chain/v1\0",
            &(self.state.tip, command),
        )?;
        let wire = Wire {
            publisher,
            id: &id,
            sequence,
            predecessor: self.state.tip,
            tip,
            accepted,
            action,
            context,
        };
        let mut counter = Counter(0);
        serde_json::to_writer(&mut counter, &wire).map_err(|_| Conflict::Encoding)?;
        if counter.0 > PENDING_BYTES {
            return Err(Conflict::CommandTooLarge);
        }
        if counter.0 > PENDING_BYTES.saturating_sub(self.bytes) {
            return Err(Conflict::Backpressure);
        }
        // Count first so encoding needs exactly one buffer, without geometric
        // spare capacity or another typed operation list.
        let length = usize::try_from(counter.0).map_err(|_| Conflict::Backpressure)?;
        let hold = self.hold.as_ref().ok_or(Conflict::Detached)?;
        let encoding = self.arena.reserve(counter.0, hold)?;
        let charge = self.arena.reserve(
            counter
                .0
                .checked_add(ENCODED_HEADER_BYTES)
                .ok_or(Conflict::Backpressure)?,
            hold,
        )?;
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(length)
            .map_err(|_| Conflict::Backpressure)?;
        serde_json::to_writer(&mut bytes, &wire).map_err(|_| Conflict::Encoding)?;
        let text = String::from_utf8(bytes).map_err(|_| Conflict::Encoding)?;
        let raw = RawValue::from_string(text).map_err(|_| Conflict::Encoding)?;
        let receipt = Receipt {
            id,
            sequence,
            accepted,
            tip,
            command,
        };
        let mut next_state = self.state.clone();
        next_state.sequence = sequence;
        next_state.tip = tip;
        if next_state.receipts.len() == PENDING_COMMANDS {
            next_state.receipts.pop_front();
        }
        next_state.receipts.push_back(receipt.clone());
        let encoded = EncodedMessage(Arc::new(EncodedPayload {
            raw,
            _charge: Some(charge),
        }));
        drop(encoding);
        Ok(PreparedMessage {
            pending: Pending { encoded, receipt },
            next_state,
            next_issuer,
        })
    }
    /// Called by the sole actor only after its already-prepared edit succeeds.
    pub(crate) fn commit(&mut self, prepared: PreparedMessage) {
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
