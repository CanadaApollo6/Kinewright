//! The ordered accepted-command stream shared by a producer and its writer.
//! Snapshot roots and this state are one publication; a receipt is an answer
//! only after that publication is known durable.
#![allow(clippy::missing_errors_doc)]
use crate::{
    bundle_records::{BranchId, ProjectId},
    operation_actor::CommandContext,
    operation_history::{Error as HistoryError, OwnerHistory, SnapshotIdentity},
    receipts::CommandId,
};
use kinewright_core::{
    Command, Operation,
    undo::{CommandKey, Effects},
};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

/// A frozen source's one-batch operations and all its admitted input origins
/// must fit the native merge envelope, independently of the manual outbox.
pub const NATIVE_COMMAND_BYTES: u64 = kinewright_core::undo::RoleBounds::TASK.operation_bytes
    + crate::operation_history::OWNER_ALLOWANCE
    + (64 << 10);
const _: () = assert!(
    NATIVE_COMMAND_BYTES
        >= kinewright_core::undo::RoleBounds::TASK.operation_bytes
            + crate::operation_history::OWNER_ALLOWANCE
            + 4096,
    "SOURCE_BATCH_AND_INPUT_ORIGINS_FIT_NATIVE_PUBLICATION"
);
pub const PENDING_COMMANDS: usize = 64;
// One admitted 1 MiB source batch plus its normalised owner/receipt context.
// The pending queue retains encoded bytes; its holder charges those bytes.
pub const PENDING_BYTES: u64 = 2 << 20;
/// All retained sender buffers and encoding scratch share one anchor's arena.
/// Each allocation keeps its source's existing full lease until its last drop.
pub const PRODUCER_BUFFER_PEAK: u64 =
    kinewright_core::undo::RoleBounds::ORDINARY.document_live_bytes;
pub const PRODUCER_HEADER_BYTES: u64 = 128 << 10;
const _: () = assert!(
    2 * PENDING_BYTES + PRODUCER_HEADER_BYTES < PRODUCER_BUFFER_PEAK,
    "FRESH_OUTBOX_ENCODING_FITS_RESERVED_ARENA"
);
const _: () = assert!(
    PRODUCER_BUFFER_PEAK <= kinewright_core::undo::RoleBounds::ORDINARY.document_live_bytes,
    "ENCODED_OUTBOX_FITS_ONE_ORDINARY_ANCHOR_ALLOWANCE"
);
#[path = "operation_producer.rs"]
mod producer;
pub use producer::{ActionRef, EncodedMessage, Lease, Producer, ProducerError};
const _: () = assert!(
    PENDING_BYTES >= kinewright_core::undo::RoleBounds::TASK.operation_bytes + (64 << 10),
    "SOURCE_STEP_AND_CONTEXT_FIT_MANUAL_WINDOW"
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", deny_unknown_fields)]
pub enum Action {
    Do {
        operations: Vec<Operation>,
        gesture: Option<String>,
    },
    Undo,
    Redo,
    Close,
}
impl Action {
    pub(crate) fn from_normalised(command: Command) -> Result<Self, Conflict> {
        Ok(match command {
            Command::Do(operation) => Self::Do {
                operations: vec![operation],
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

    pub fn apply(
        &self,
        context: &CommandContext,
        owner: &mut OwnerHistory,
    ) -> Result<Effects, HistoryError> {
        // Each replica owns the operation payload it accepts.
        self.clone().execute(context.clone(), owner)
    }
    pub fn execute(
        self,
        context: CommandContext,
        owner: &mut OwnerHistory,
    ) -> Result<Effects, HistoryError> {
        self.execute_with_inputs(context, owner, None)
    }
    pub(crate) fn execute_with_inputs(
        self,
        context: CommandContext,
        owner: &mut OwnerHistory,
        certificate: Option<&crate::operation_inputs::ManualInputs>,
    ) -> Result<Effects, HistoryError> {
        if let Some(approval) = context.transition {
            let matching = matches!(
                (&self, approval.direction),
                (Self::Undo, crate::operation_history::Direction::Undo)
                    | (Self::Redo, crate::operation_history::Direction::Redo)
            );
            if !matching || context.approval.is_some() || !context.inputs.is_empty() {
                return Err(HistoryError::InvalidLedger);
            }
            return owner.approve_transition(approval.id, approval.direction, approval.target);
        }
        let command = match self {
            Self::Do {
                operations,
                gesture: Some(coalesce_key),
            } => Command::DoBatchCoalesced {
                operations,
                coalesce_key,
            },
            Self::Do {
                operations,
                gesture: None,
            } => Command::DoBatch(operations),
            Self::Undo => Command::Undo,
            Self::Redo => Command::Redo,
            Self::Close if context.approval.is_none() && context.inputs.is_empty() => {
                return owner.close_gesture();
            }
            Self::Close => return Err(HistoryError::InvalidLedger),
        };
        owner.execute_with_inputs(
            command,
            context.creation,
            context.inputs,
            context.approval,
            certificate,
        )
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Seed {
    pub project: ProjectId,
    pub branch: BranchId,
    pub attachment_epoch: u64,
    pub installed: SnapshotIdentity,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Message {
    pub publisher: Publisher,
    pub id: CommandId,
    pub sequence: u64,
    pub predecessor: [u8; 32],
    pub tip: [u8; 32],
    pub accepted: CommandKey,
    pub action: Action,
    pub context: CommandContext,
    /// Later sub-batches of the same open gesture, in their accepted order.
    /// A single receipt covers the whole packet, including these boundaries.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub folded: Vec<FoldedBatch>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FoldedBatch {
    pub creation: crate::operation_history::CreationOrigin,
    pub operations: Vec<Operation>,
}
/// Portable models have no namespace. Durable publishers bind the native parent
/// and namespace name already granted by their lease, including after restart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Publisher {
    Portable,
    Native([u8; 32]),
}
impl Publisher {
    pub(crate) fn native(parent: (u64, u64), name: &str) -> Result<Self, Conflict> {
        hash(b"kinewright/native-publisher/v1\0", &(parent, name)).map(Self::Native)
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    pub id: CommandId,
    pub sequence: u64,
    pub accepted: CommandKey,
    pub tip: [u8; 32],
    pub command: [u8; 32],
    /// Number of extra Core command boundaries in this packet.
    #[serde(default, skip_serializing_if = "no_additional_batches")]
    pub additional_batches: u64,
}
#[allow(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde checks its field by reference"
)]
fn no_additional_batches(value: &u64) -> bool {
    *value == 0
}
impl Receipt {
    pub(crate) fn next_position(&self) -> Option<CommandKey> {
        Some(CommandKey {
            epoch: self.accepted.epoch,
            position: self
                .accepted
                .position
                .checked_add(1)?
                .checked_add(self.additional_batches)?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamState {
    seed: Seed,
    sequence: u64,
    tip: [u8; 32],
    receipts: VecDeque<Receipt>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Conflict {
    Encoding,
    Backpressure,
    CommandTooLarge,
    Unnormalised,
    Detached,
    CounterExhausted,
    Sequence,
    Chain,
    CommandIdentity,
    AcceptedKey,
    Snapshot,
    ReceiptWindow,
}
impl std::fmt::Display for Conflict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if *self == Self::Backpressure {
            return f.write_str("Saving is catching up. Retry when the pending edits are saved.");
        }
        write!(
            f,
            "Editing stream conflict ({self:?}). Edits remain available for recovery."
        )
    }
}
impl std::error::Error for Conflict {}
fn hash(domain: &[u8], value: &impl Serialize) -> Result<[u8; 32], Conflict> {
    struct Writer(kinewright_hash::Sha256);
    impl std::io::Write for Writer {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.update(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut writer = Writer(kinewright_hash::Sha256::new());
    writer.0.update(domain);
    serde_json::to_writer(&mut writer, value).map_err(|_| Conflict::Encoding)?;
    Ok(writer.0.finalize())
}
impl Message {
    pub fn digest(&self) -> Result<[u8; 32], Conflict> {
        let header = (
            self.publisher,
            &self.id,
            self.accepted,
            &self.action,
            &self.context,
        );
        if self.folded.is_empty() {
            hash(b"kinewright/accepted-command/v1\0", &header)
        } else {
            hash(
                b"kinewright/folded-manual-command/v1\0",
                &(header, &self.folded),
            )
        }
    }
    pub(crate) fn validate_fold(&self) -> Result<(), Conflict> {
        if self.folded.is_empty() {
            return Ok(());
        }
        let Action::Do {
            operations,
            gesture: Some(_),
        } = &self.action
        else {
            return Err(Conflict::Unnormalised);
        };
        if !foldable(&self.context, operations)
            || self.folded.iter().any(|part| {
                part.creation.branch != self.context.creation.branch
                    || part.operations.is_empty()
                    || crate::operation_inputs::ManualInputs::imports(&part.operations)
            })
        {
            return Err(Conflict::Unnormalised);
        }
        Ok(())
    }
    pub(crate) fn into_parts(self) -> impl Iterator<Item = (Action, CommandContext)> {
        let gesture = match &self.action {
            Action::Do { gesture, .. } if !self.folded.is_empty() => gesture.clone(),
            _ => None,
        };
        std::iter::once((self.action, self.context)).chain(self.folded.into_iter().map(
            move |part| {
                (
                    Action::Do {
                        operations: part.operations,
                        gesture: gesture.clone(),
                    },
                    CommandContext {
                        creation: part.creation,
                        inputs: vec![],
                        approval: None,
                        transition: None,
                    },
                )
            },
        ))
    }
    pub(crate) fn chained(&self) -> Result<[u8; 32], Conflict> {
        hash(
            b"kinewright/accepted-chain/v1\0",
            &(self.predecessor, self.digest()?),
        )
    }
}
pub(crate) fn foldable(context: &CommandContext, operations: &[Operation]) -> bool {
    context.approval.is_none()
        && context.transition.is_none()
        && context.inputs.is_empty()
        && !operations.is_empty()
        && !crate::operation_inputs::ManualInputs::imports(operations)
}
impl StreamState {
    pub fn installed(seed: Seed) -> Result<Self, Conflict> {
        let tip = hash(b"kinewright/installed-stream/v1\0", &seed)?;
        Ok(Self {
            seed,
            sequence: 0,
            tip,
            receipts: VecDeque::new(),
        })
    }
    #[must_use]
    pub const fn seed(&self) -> &Seed {
        &self.seed
    }
    #[must_use]
    pub const fn sequence(&self) -> u64 {
        self.sequence
    }
    #[must_use]
    pub const fn tip(&self) -> [u8; 32] {
        self.tip
    }
    pub fn verify_installed_snapshot(&self, identity: SnapshotIdentity) -> Result<(), Conflict> {
        if self.seed.installed != identity {
            return Err(Conflict::Snapshot);
        }
        Ok(())
    }
    pub fn message(
        &self,
        id: CommandId,
        accepted: CommandKey,
        action: Action,
        context: CommandContext,
    ) -> Result<Message, Conflict> {
        let mut message = Message {
            publisher: Publisher::Portable,
            id,
            accepted,
            action,
            context,
            folded: vec![],
            sequence: self
                .sequence
                .checked_add(1)
                .ok_or(Conflict::CounterExhausted)?,
            predecessor: self.tip,
            tip: [0; 32],
        };
        message.tip = message.chained()?;
        Ok(message)
    }
    /// Receipt lookup precedes fresh authority. The absent suffix still needs
    /// the current attachment's fencing proof at the publication owner.
    pub fn lookup(&self, message: &Message) -> Result<Option<&Receipt>, Conflict> {
        message
            .id
            .issuer
            .validate()
            .map_err(|_| Conflict::CommandIdentity)?;
        if message.id.sequence == 0 {
            return Err(Conflict::CommandIdentity);
        }
        if let Some(receipt) = self
            .receipts
            .iter()
            .find(|receipt| receipt.id == message.id)
        {
            if receipt.command != message.digest()?
                || receipt.sequence != message.sequence
                || receipt.accepted != message.accepted
                || receipt.additional_batches != message.folded.len() as u64
                || receipt.tip != message.tip
            {
                return Err(Conflict::CommandIdentity);
            }
            return Ok(Some(receipt));
        }
        if message.sequence
            != self
                .sequence
                .checked_add(1)
                .ok_or(Conflict::CounterExhausted)?
        {
            return Err(Conflict::Sequence);
        }
        if message.predecessor != self.tip || message.chained()? != message.tip {
            return Err(Conflict::Chain);
        }
        Ok(None)
    }
    /// Build the prospective root state, without acknowledging or changing the
    /// committed state. The caller publishes it with the new snapshot root.
    pub fn advanced(
        &self,
        message: &Message,
        accepted: CommandKey,
    ) -> Result<(Self, Receipt), Conflict> {
        if let Some(receipt) = self.lookup(message)? {
            return Ok((self.clone(), receipt.clone()));
        }
        let receipt = Self::new_receipt(message, accepted)?;
        let mut next = self.clone();
        next.record_receipt(receipt.clone());
        Ok((next, receipt))
    }

    /// The caller has checked the chain with `lookup` and still has to
    /// apply this command before recording its receipt.
    pub(crate) fn new_receipt(
        message: &Message,
        accepted: CommandKey,
    ) -> Result<Receipt, Conflict> {
        if message.accepted != accepted {
            return Err(Conflict::AcceptedKey);
        }
        Ok(Receipt {
            id: message.id.clone(),
            sequence: message.sequence,
            accepted,
            tip: message.tip,
            command: message.digest()?,
            additional_batches: message.folded.len() as u64,
        })
    }

    pub(crate) fn record_receipt(&mut self, receipt: Receipt) {
        self.sequence = receipt.sequence;
        self.tip = receipt.tip;
        self.receipts.push_back(receipt);
        // A producer has at most 64 unacknowledged messages. It cannot advance
        // past this window with an older message still pending. Other native
        // and spending receipts keep their independent retention rules.
        if self.receipts.len() > PENDING_COMMANDS {
            self.receipts.pop_front();
        }
    }
    pub(crate) fn replace_tail_receipt(&mut self, receipt: Receipt) {
        let previous = self.receipts.pop_back().expect("an unsent receipt");
        assert_eq!(previous.sequence, receipt.sequence);
        assert_eq!(previous.id, receipt.id);
        self.record_receipt(receipt);
    }
    pub fn receipts(&self) -> impl Iterator<Item = &Receipt> {
        self.receipts.iter()
    }
    pub fn validate(&self) -> Result<(), Conflict> {
        if self.receipts.len() > PENDING_COMMANDS {
            return Err(Conflict::ReceiptWindow);
        }
        if self.sequence == 0 {
            let initial = Self::installed(self.seed.clone())?;
            if self.tip != initial.tip || !self.receipts.is_empty() {
                return Err(Conflict::Chain);
            }
        } else if self
            .receipts
            .back()
            .is_none_or(|r| r.sequence != self.sequence || r.tip != self.tip)
        {
            return Err(Conflict::ReceiptWindow);
        }
        let first = self
            .sequence
            .saturating_sub(PENDING_COMMANDS as u64)
            .saturating_add(1);
        for (offset, receipt) in self.receipts.iter().enumerate() {
            receipt
                .id
                .issuer
                .validate()
                .map_err(|_| Conflict::CommandIdentity)?;
            if receipt.id.sequence == 0 {
                return Err(Conflict::CommandIdentity);
            }
            if receipt.sequence != first + offset as u64
                || self
                    .receipts
                    .iter()
                    .take(offset)
                    .any(|prior| prior.id == receipt.id)
            {
                return Err(Conflict::ReceiptWindow);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "operation_protocol_tests.rs"]
mod tests;
