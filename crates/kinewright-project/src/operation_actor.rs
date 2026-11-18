//! Actor ownership of the operation history and its project-side records.
//! The process supplies the pool; every retained query keeps its lease.
#![allow(clippy::missing_errors_doc)]

use crate::{
    bundle_records::{BatchId, BranchId, ObjectRef},
    operation_history::{
        Approval, CreationOrigin, Error as HistoryError, HistoryPool, OwnerBoundary, OwnerHistory,
        OwnerOperations, OwnerSnapshot, Role, SnapshotIdentity, TransitionApproval,
        ValidatedOwnerSnapshot,
    },
};
use crossbeam_channel::{Receiver, Sender, bounded, unbounded};
use kinewright_core::{
    AttachmentState, Command, CommandToken, CoreMutationRefusal, Document, Event, JournalCommand,
    Query, QueryResult, TimelineRevision,
    undo::{CommandKey, Effects},
};
use std::{
    any::TypeId,
    marker::PhantomData,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
};

#[path = "operation_actor_prepare.rs"]
mod prepare;
#[path = "operation_actor_producer.rs"]
mod producer;
pub(crate) use prepare::ManualPreview;
use prepare::PrepareRequest;
pub use prepare::PreparedUpdate;
pub use producer::ManualPermit;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    History(HistoryError),
    Protocol(crate::operation_protocol::Conflict),
    Disconnected,
    Retired,
    MutationRefused(CoreMutationRefusal),
    CaptureChanged,
    CounterExhausted,
    RecorderDisabled,
    System(String),
}
impl Error {
    /// Capacity refusals leave the accepted history unchanged.
    #[must_use]
    pub const fn is_capacity(&self) -> bool {
        matches!(
            self,
            Self::History(
                HistoryError::History(
                    kinewright_core::undo::Error::StepTooLarge
                        | kinewright_core::undo::Error::HistoryFull
                        | kinewright_core::undo::Error::Capacity
                        | kinewright_core::undo::Error::DocumentCapacity
                ) | HistoryError::Capacity(_)
                    | HistoryError::PoolFull(_)
                    | HistoryError::StoreCapacity(_)
                    | HistoryError::Reservation
            )
        )
    }
}
impl From<HistoryError> for Error {
    fn from(error: HistoryError) -> Self {
        Self::History(error)
    }
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::History(error) => error.fmt(f),
            Self::MutationRefused(reason) => reason.fmt(f),
            Self::Protocol(error) => error.fmt(f),
            other => write!(f, "{other:?}"),
        }
    }
}
impl std::error::Error for Error {}

pub use kinewright_core::ActorIdentity;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChangeId {
    pub actor: ActorIdentity,
    pub generation: u64,
    pub revision: TimelineRevision,
    pub position: CommandKey,
}
/// One actor transition supplies both values. The snapshot owns its full
/// independent lease; the identifier adds no document or history holder.
pub struct CapturedHistory {
    pub change: ChangeId,
    pub branch: BranchId,
    pub snapshot: OwnerSnapshot,
}
/// One immutable protocol reply owns a captured wire state. Retrying the reply
/// shares its buffer and its original capture lease; it creates no second copy.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortableHistory {
    pub snapshot: Arc<crate::operation_history::SnapshotTransport>,
    pub branch: BranchId,
    pub revision: TimelineRevision,
}
impl PartialEq for PortableHistory {
    fn eq(&self, other: &Self) -> bool {
        self.identity() == other.identity()
            && self.branch == other.branch
            && self.revision == other.revision
    }
}
impl Eq for PortableHistory {}
impl PortableHistory {
    #[must_use]
    pub fn identity(&self) -> SnapshotIdentity {
        self.snapshot.identity()
    }
    #[must_use]
    pub fn role(&self) -> Role {
        self.snapshot.role()
    }
    pub fn encoded_bytes(&self) -> Result<u64, crate::bundle::BundleError> {
        let mut counter = crate::store::BoundedCounter {
            bytes: 0,
            limit: self.role().envelope().retained,
        };
        serde_json::to_writer(&mut counter, self).map_err(crate::bundle::encoding_error)?;
        Ok(counter.bytes)
    }
    pub fn validate(self, pool: &HistoryPool) -> Result<ValidatedOwnerSnapshot, HistoryError> {
        let role = self.role();
        self.snapshot.receive(pool, role)
    }
}
impl CapturedHistory {
    pub fn into_portable(self) -> Result<PortableHistory, HistoryError> {
        Ok(PortableHistory {
            snapshot: Arc::new(self.snapshot.into_transport()?),
            branch: self.branch,
            revision: self.change.revision,
        })
    }
}
/// A frozen attachment proof contains no independently retained history.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AcceptedCut {
    change: ChangeId,
    identity: SnapshotIdentity,
}
impl AcceptedCut {
    pub(crate) const fn committed(change: ChangeId, identity: SnapshotIdentity) -> Self {
        Self { change, identity }
    }
    #[must_use]
    pub const fn change(&self) -> ChangeId {
        self.change
    }
    #[must_use]
    pub const fn identity(&self) -> SnapshotIdentity {
        self.identity
    }
}
/// The caller supplies a verified command's immutable owner context.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandContext {
    pub creation: CreationOrigin,
    pub inputs: Vec<ObjectRef>,
    pub approval: Option<Approval>,
    pub transition: Option<TransitionApproval>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub error: Error,
    pub token: Option<CommandToken>,
}
impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.error.fmt(f)
    }
}
impl std::error::Error for Refusal {}
pub type Notification = Result<Event, Refusal>;
fn command_token(command: &Command) -> Option<CommandToken> {
    match command {
        Command::DoIfRevision { token, .. } => *token,
        _ => None,
    }
}
#[derive(Clone)]
pub struct Core {
    sender: Sender<Message>,
    identity: ActorIdentity,
    handles: Arc<ActorWorker>,
    pool: HistoryPool,
}
// Core's sender is declared before this shared worker. On the last handle,
// Rust drops that sender first, disconnecting the actor's queue before joining.
// Returning from Drop therefore releases the actor's lease synchronously; a
// following admission cannot race the scheduler's teardown of the old actor.
struct ActorWorker {
    thread: Option<thread::JoinHandle<()>>,
}
impl Drop for ActorWorker {
    fn drop(&mut self) {
        if let Some(worker) = self.thread.take() {
            let _ = worker.join();
        }
    }
}

pub struct Attachment<P> {
    core: Core,
    _proof: PhantomData<fn() -> P>,
}

struct CommandMessage {
    command: Command,
    context: Option<CommandContext>,
    permit: Option<u64>,
    reply: Option<Sender<Notification>>,
}
enum InstallMode {
    NewEpoch,
    AtEpoch(u64),
    Peer {
        epoch: u64,
        branch: BranchId,
        revision: TimelineRevision,
    },
}
#[expect(
    clippy::large_enum_variant,
    reason = "actor messages move the existing store capability without copying it"
)]
enum Message {
    Command(Box<CommandMessage>),
    Close(Option<u64>, Sender<Result<Effects, Error>>),
    Producer(Box<producer::BindRequest>),
    CurrentProducer(u64, Sender<bool>),
    Pending(
        u64,
        Sender<Result<Vec<crate::operation_protocol::EncodedMessage>, Error>>,
    ),
    PendingCount(u64, Sender<Result<usize, Error>>),
    Acknowledge(
        TypeId,
        u64,
        Vec<crate::operation_protocol::Receipt>,
        Sender<Result<(), Error>>,
    ),
    Subscribe(Sender<Notification>),
    Change(Sender<Result<ChangeId, Error>>),
    Branch(Sender<BranchId>),
    Identity(Sender<Result<SnapshotIdentity, Error>>),
    Baseline(Sender<Result<Arc<Document>, Error>>),
    AttachmentState(Sender<AttachmentState>),
    Fork(
        Role,
        Sender<Result<(OwnerHistory, TimelineRevision), Error>>,
    ),
    Capture(Sender<Result<CapturedHistory, Error>>),
    Preview(Command, Sender<Result<OwnerSnapshot, Error>>),
    BatchCapacity(kinewright_core::undo::Charge, Sender<Result<(), Error>>),
    Approval(
        ObjectRef,
        Sender<Result<Option<crate::operation_history::ApprovalAnswer>, Error>>,
    ),
    Boundary(usize, Sender<Result<OwnerBoundary, Error>>),
    Operations(bool, Sender<Result<OwnerOperations, Error>>),
    Counters(
        Sender<(
            kinewright_core::undo::PhaseCounters,
            crate::operation_history::OwnerCounters,
        )>,
    ),
    Bind(TypeId, Sender<Result<(), Error>>),
    Freeze(TypeId, Sender<Result<AcceptedCut, Error>>),
    FreezePublished(TypeId, AcceptedCut, Sender<Result<(), Error>>),
    AcknowledgeFrozen(
        TypeId,
        AcceptedCut,
        Vec<crate::operation_protocol::Receipt>,
        Sender<Result<(), Error>>,
    ),
    Cut(TypeId, Sender<Result<AcceptedCut, Error>>),
    Finish(TypeId, AcceptedCut, bool, Sender<Result<(), Error>>),
    Install(
        Option<TypeId>,
        Option<AcceptedCut>,
        Box<ValidatedOwnerSnapshot>,
        InstallMode,
        Sender<Result<ChangeId, Error>>,
    ),
    Retire(TypeId, AcceptedCut, Sender<Result<(), Error>>),
    Prepare(Box<PrepareRequest>),
    PreviewManual(TypeId, AcceptedCut, Sender<Result<ManualPreview, Error>>),
    Selection(
        TypeId,
        AcceptedCut,
        Box<ValidatedOwnerSnapshot>,
        Sender<Result<(OwnerHistory, TimelineRevision), Error>>,
    ),
    RetargetSelection(
        TypeId,
        AcceptedCut,
        BranchId,
        Sender<Result<AcceptedCut, Error>>,
    ),
    SealSelection(
        TypeId,
        AcceptedCut,
        crate::store::ProjectStore,
        crate::storage_budget::StorageReservation<
            std::collections::BTreeMap<crate::operation_history::Resource, ObjectRef>,
        >,
        Sender<Result<crate::operation_history::SealedOwner, Error>>,
    ),
    CommitPrepared(
        TypeId,
        AcceptedCut,
        Box<PreparedUpdate>,
        Sender<Result<ChangeId, Error>>,
    ),
}
impl Core {
    pub fn spawn(
        pool: &HistoryPool,
        role: Role,
        origin: CreationOrigin,
        document: impl Into<Arc<Document>>,
    ) -> Result<Self, Error> {
        let history =
            OwnerHistory::new(pool, role, role.policy(), &origin, || Ok(document.into()))?;
        Self::spawn_owner(history, origin.branch, TimelineRevision::default())
    }
    pub fn spawn_recording(
        pool: &HistoryPool,
        role: Role,
        origin: CreationOrigin,
        document: impl Into<Arc<Document>>,
    ) -> Result<Self, Error> {
        Self::spawn_recording_at_revision(pool, role, origin, document, TimelineRevision::default())
    }
    pub fn spawn_recording_at_revision(
        pool: &HistoryPool,
        role: Role,
        origin: CreationOrigin,
        document: impl Into<Arc<Document>>,
        revision: TimelineRevision,
    ) -> Result<Self, Error> {
        let history =
            OwnerHistory::new(pool, role, role.policy(), &origin, || Ok(document.into()))?
                .recording()?;
        Self::spawn_owner(history, origin.branch, revision)
    }
    #[cfg(any(test, feature = "test-util"))]
    pub fn for_test(document: impl Into<Arc<Document>>) -> Result<Self, Error> {
        Self::spawn(
            &HistoryPool::new(crate::operation_history::PoolLimits::default()),
            Role::Ordinary,
            CreationOrigin {
                branch: BranchId::fresh().map_err(|e| Error::System(e.to_string()))?,
                batch: BatchId::fresh().map_err(|e| Error::System(e.to_string()))?,
            },
            document,
        )
    }
    #[cfg(any(test, feature = "test-util"))]
    pub fn for_test_at_revision(
        document: impl Into<Arc<Document>>,
        revision: TimelineRevision,
    ) -> Result<Self, Error> {
        Self::spawn_at_revision(
            &HistoryPool::new(crate::operation_history::PoolLimits::default()),
            Role::Ordinary,
            CreationOrigin {
                branch: BranchId::fresh().map_err(|error| Error::System(error.to_string()))?,
                batch: BatchId::fresh().map_err(|error| Error::System(error.to_string()))?,
            },
            document,
            revision,
        )
    }
    /// A second-process fixture receives and validates the complete wire owner.
    #[cfg(any(test, feature = "test-util"))]
    pub fn peer_for_test(history: PortableHistory) -> Result<Self, Error> {
        let pool = HistoryPool::new(crate::operation_history::PoolLimits::default());
        let branch = history.branch.clone();
        let revision = history.revision;
        Self::open(history.validate(&pool)?, branch, revision)
    }
    /// Fixture-only downward bounds exercise folds without a production-sized stream.
    #[cfg(any(test, feature = "test-util"))]
    pub fn for_test_with_bounds(
        document: impl Into<Arc<Document>>,
        bounds: kinewright_core::undo::RoleBounds,
    ) -> Result<Self, Error> {
        let pool = HistoryPool::new(crate::operation_history::PoolLimits::default());
        let origin = CreationOrigin {
            branch: BranchId::fresh().map_err(|error| Error::System(error.to_string()))?,
            batch: BatchId::fresh().map_err(|error| Error::System(error.to_string()))?,
        };
        let history = OwnerHistory::measured(
            &pool,
            Role::Ordinary,
            Role::Ordinary.policy(),
            &origin,
            bounds,
            || Ok(document.into()),
        )?;
        Self::spawn_owner(history, origin.branch, TimelineRevision::default())
    }
    pub fn spawn_at_revision(
        pool: &HistoryPool,
        role: Role,
        origin: CreationOrigin,
        document: impl Into<Arc<Document>>,
        revision: TimelineRevision,
    ) -> Result<Self, Error> {
        let history =
            OwnerHistory::new(pool, role, role.policy(), &origin, || Ok(document.into()))?;
        Self::spawn_owner(history, origin.branch, revision)
    }
    pub fn open(
        snapshot: ValidatedOwnerSnapshot,
        branch: BranchId,
        revision: TimelineRevision,
    ) -> Result<Self, Error> {
        Self::spawn_owner(snapshot.open()?, branch, revision)
    }
    pub(crate) fn spawn_owner(
        history: OwnerHistory,
        branch: BranchId,
        revision: TimelineRevision,
    ) -> Result<Self, Error> {
        Self::spawn_inner(history, branch, revision, None, AttachmentState::Live)
    }
    pub(super) fn spawn_staged(
        history: OwnerHistory,
        branch: BranchId,
        revision: TimelineRevision,
        authority: TypeId,
    ) -> Result<Self, Error> {
        Self::spawn_inner(
            history,
            branch,
            revision,
            Some(authority),
            AttachmentState::Frozen,
        )
    }
    fn spawn_inner(
        history: OwnerHistory,
        branch: BranchId,
        revision: TimelineRevision,
        owner: Option<TypeId>,
        attachment: AttachmentState,
    ) -> Result<Self, Error> {
        let pool = history.pool();
        let identity = ActorIdentity::fresh().ok_or(Error::CounterExhausted)?;
        let (sender, receiver) = unbounded();
        let worker = thread::Builder::new()
            .name("kinewright-core".into())
            .spawn(move || {
                State {
                    history: Some(history),
                    branch,
                    identity,
                    revision,
                    generation: 0,
                    owner,
                    attachment,
                    subscribers: vec![],
                    producer: None,
                    permit_generation: 0,
                }
                .run(&receiver);
            })
            .map_err(|error| Error::System(error.to_string()))?;
        Ok(Self {
            sender,
            identity,
            handles: Arc::new(ActorWorker {
                thread: Some(worker),
            }),
            pool,
        })
    }
    fn call<T>(&self, message: impl FnOnce(Sender<T>) -> Message) -> Result<T, Error> {
        let (sender, receiver) = bounded(1);
        self.sender
            .send(message(sender))
            .map_err(|_| Error::Disconnected)?;
        receiver.recv().map_err(|_| Error::Disconnected)
    }
    #[must_use]
    pub const fn actor_identity(&self) -> ActorIdentity {
        self.identity
    }
    #[must_use]
    pub fn handle_count(&self) -> usize {
        Arc::strong_count(&self.handles)
    }
    #[must_use]
    pub const fn history_pool(&self) -> &HistoryPool {
        &self.pool
    }
    /// Share the actor's immutable baseline with a comparison view.
    /// Comparison views keep their actor alive and share its accounted allocation.
    pub fn baseline_document(&self) -> Result<Arc<Document>, Error> {
        self.call(Message::Baseline)?
    }
    /// Make a child from one atomic parent cut. Existing clips retain their
    /// original lineage, and a delta child never inherits Undo or Redo stacks.
    pub fn fork(
        &self,
        role: Role,
        origin: CreationOrigin,
        inherit_revision: bool,
    ) -> Result<(Self, TimelineRevision), Error> {
        let (owner, parent_revision) = self.call(|reply| Message::Fork(role, reply))??;
        let revision = if inherit_revision {
            parent_revision
        } else {
            TimelineRevision::default()
        };
        Ok((
            Self::spawn_owner(owner, origin.branch, revision)?,
            parent_revision,
        ))
    }
    pub fn send(&self, command: Command) -> Result<(), Error> {
        self.sender
            .send(Message::Command(Box::new(CommandMessage {
                command,
                context: None,
                permit: None,
                reply: None,
            })))
            .map_err(|_| Error::Disconnected)
    }
    pub fn request(&self, command: Command) -> Notification {
        let token = command_token(&command);
        self.call(|reply| {
            Message::Command(Box::new(CommandMessage {
                command,
                context: None,
                permit: None,
                reply: Some(reply),
            }))
        })
        .map_err(|error| Refusal { error, token })?
    }
    pub fn request_with_context(&self, command: Command, context: CommandContext) -> Notification {
        let token = command_token(&command);
        self.call(|reply| {
            Message::Command(Box::new(CommandMessage {
                command,
                context: Some(context),
                permit: None,
                reply: Some(reply),
            }))
        })
        .map_err(|error| Refusal { error, token })?
    }
    pub fn close_gesture(&self) -> Result<Effects, Error> {
        self.call(|reply| Message::Close(None, reply))?
    }
    pub fn counters(
        &self,
    ) -> Result<
        (
            kinewright_core::undo::PhaseCounters,
            crate::operation_history::OwnerCounters,
        ),
        Error,
    > {
        self.call(Message::Counters)
    }
    /// Independently admitted, read-only policy preview. Nothing is installed in
    /// the source actor and its command counter and approval ledger are unchanged.
    pub fn preview_command(&self, command: Command) -> Result<OwnerSnapshot, Error> {
        self.call(|reply| Message::Preview(command, reply))?
    }
    /// Refuse impossible plain batches before consent, without a document preview.
    /// This is advisory; the executor remains the atomic admission authority.
    pub fn check_batch_capacity(
        &self,
        operations: &[kinewright_core::Operation],
    ) -> Result<(), Error> {
        if operations.is_empty() {
            return Err(HistoryError::History(kinewright_core::undo::Error::Batch(
                kinewright_core::BatchError::Empty,
            ))
            .into());
        }
        let charge = kinewright_core::undo::batch_charge(operations).map_err(HistoryError::from)?;
        self.call(|reply| Message::BatchCapacity(charge, reply))?
    }
    pub fn operations(&self, recorded: bool) -> Result<OwnerOperations, Error> {
        self.call(|reply| Message::Operations(recorded, reply))?
    }
    pub fn attachment_state(&self) -> Result<AttachmentState, Error> {
        self.call(Message::AttachmentState)
    }
    pub fn creation_branch(&self) -> Result<BranchId, Error> {
        self.call(Message::Branch)
    }
    pub fn snapshot_identity(&self) -> Result<SnapshotIdentity, Error> {
        self.call(Message::Identity)?
    }
    pub fn change_id(&self) -> Result<ChangeId, Error> {
        self.call(Message::Change)?
    }
    pub fn export_history(&self) -> Result<OwnerSnapshot, Error> {
        Ok(self.capture_state()?.snapshot)
    }
    pub fn capture_state(&self) -> Result<CapturedHistory, Error> {
        self.call(Message::Capture)?
    }
    pub fn portable_history(&self) -> Result<PortableHistory, Error> {
        self.capture_state()?.into_portable().map_err(Error::from)
    }
    pub fn approval_answer(
        &self,
        reference: ObjectRef,
    ) -> Result<Option<crate::operation_history::ApprovalAnswer>, Error> {
        self.call(|reply| Message::Approval(reference, reply))?
    }
    pub fn boundary(&self, depth: usize) -> Result<OwnerBoundary, Error> {
        self.call(|reply| Message::Boundary(depth, reply))?
    }
    pub fn subscribe(&self) -> Result<Receiver<Notification>, Error> {
        let (sender, receiver) = unbounded();
        self.sender
            .send(Message::Subscribe(sender))
            .map_err(|_| Error::Disconnected)?;
        Ok(receiver)
    }
    pub fn attach<P: 'static>(&self, _proof: &P) -> Result<Attachment<P>, Error> {
        self.call(|reply| Message::Bind(TypeId::of::<P>(), reply))??;
        Ok(Attachment {
            core: self.clone(),
            _proof: PhantomData,
        })
    }
    /// Setup-only installation; a bound actor requires its owner's attachment.
    pub fn install_snapshot(&self, snapshot: ValidatedOwnerSnapshot) -> Result<ChangeId, Error> {
        self.call(|reply| {
            Message::Install(None, None, Box::new(snapshot), InstallMode::NewEpoch, reply)
        })?
    }
}
impl<P: 'static> Attachment<P> {
    pub fn capture(&self, _proof: &P) -> Result<AcceptedCut, Error> {
        self.core
            .call(|reply| Message::Cut(TypeId::of::<P>(), reply))?
    }
    pub fn freeze(&self, _proof: &P) -> Result<AcceptedCut, Error> {
        self.core
            .call(|reply| Message::Freeze(TypeId::of::<P>(), reply))?
    }
    pub fn resume(&self, _proof: &P, cut: AcceptedCut) -> Result<(), Error> {
        self.core
            .call(|reply| Message::Finish(TypeId::of::<P>(), cut, false, reply))?
    }
    pub fn seal(&self, _proof: &P, cut: AcceptedCut) -> Result<(), Error> {
        self.core
            .call(|reply| Message::Finish(TypeId::of::<P>(), cut, true, reply))?
    }
    pub fn install_snapshot(
        &self,
        _proof: &P,
        cut: AcceptedCut,
        snapshot: ValidatedOwnerSnapshot,
    ) -> Result<ChangeId, Error> {
        self.core.call(|reply| {
            Message::Install(
                Some(TypeId::of::<P>()),
                Some(cut),
                Box::new(snapshot),
                InstallMode::NewEpoch,
                reply,
            )
        })?
    }
    /// A peer adopts the epoch already chosen by the installing owner. Its
    /// record was replay-validated before reaching this frozen attachment.
    pub fn install_snapshot_at_epoch(
        &self,
        _proof: &P,
        cut: AcceptedCut,
        snapshot: ValidatedOwnerSnapshot,
        epoch: u64,
    ) -> Result<ChangeId, Error> {
        self.core.call(|reply| {
            Message::Install(
                Some(TypeId::of::<P>()),
                Some(cut),
                Box::new(snapshot),
                InstallMode::AtEpoch(epoch),
                reply,
            )
        })?
    }
    /// Adopt the installing peer's chosen epoch and actor metadata atomically.
    pub fn install_peer_snapshot(
        &self,
        _proof: &P,
        cut: AcceptedCut,
        snapshot: ValidatedOwnerSnapshot,
        branch: BranchId,
        revision: TimelineRevision,
    ) -> Result<ChangeId, Error> {
        let mode = InstallMode::Peer {
            epoch: snapshot.position().epoch,
            branch,
            revision,
        };
        self.core.call(|reply| {
            Message::Install(
                Some(TypeId::of::<P>()),
                Some(cut),
                Box::new(snapshot),
                mode,
                reply,
            )
        })?
    }
    /// Destruction happens before the acknowledgement, including with other stale handles alive.
    pub fn retire(&self, _proof: &P, cut: AcceptedCut) -> Result<(), Error> {
        self.core
            .call(|reply| Message::Retire(TypeId::of::<P>(), cut, reply))?
    }
}
struct State {
    history: Option<OwnerHistory>,
    branch: BranchId,
    identity: ActorIdentity,
    revision: TimelineRevision,
    generation: u64,
    owner: Option<TypeId>,
    attachment: AttachmentState,
    subscribers: Vec<Sender<Notification>>,
    producer: Option<producer::BoundProducer>,
    permit_generation: u64,
}
impl State {
    fn history(&self) -> Result<&OwnerHistory, Error> {
        self.history.as_ref().ok_or(Error::Retired)
    }
    fn change(&self) -> Result<ChangeId, Error> {
        Ok(ChangeId {
            actor: self.identity,
            generation: self.generation,
            revision: self.revision,
            position: self.history()?.history().position(),
        })
    }
    fn authorize(&self, owner: TypeId) -> Result<(), Error> {
        if self.owner != Some(owner) {
            return Err(Error::MutationRefused(CoreMutationRefusal::OwnerRequired));
        }
        if self.attachment == AttachmentState::Sealed {
            return Err(Error::MutationRefused(CoreMutationRefusal::Sealed));
        }
        Ok(())
    }
    fn check_cut(&self, owner: TypeId, cut: AcceptedCut) -> Result<(), Error> {
        self.authorize(owner)?;
        if self.attachment != AttachmentState::Frozen || self.change()? != cut.change {
            return Err(Error::CaptureChanged);
        }
        Ok(())
    }
    fn freeze(&mut self, owner: TypeId) -> Result<AcceptedCut, Error> {
        let cut = self.capture_cut(owner)?;
        self.attachment = AttachmentState::Frozen;
        Ok(cut)
    }
    fn capture_cut(&self, owner: TypeId) -> Result<AcceptedCut, Error> {
        self.authorize(owner)?;
        let change = self.change()?;
        // Hash a borrowed canonical view under scratch; the cut owns no extra
        // history lease even when the incoming replacement is already admitted.
        let identity = self.history()?.identity()?;
        Ok(AcceptedCut { change, identity })
    }
    fn install(
        &mut self,
        owner: Option<TypeId>,
        cut: Option<AcceptedCut>,
        snapshot: ValidatedOwnerSnapshot,
        mode: InstallMode,
    ) -> Result<ChangeId, Error> {
        match (owner, cut) {
            (Some(owner), Some(cut)) => self.check_cut(owner, cut)?,
            (None, None) if self.owner.is_none() && self.attachment == AttachmentState::Live => {}
            _ => return Err(Error::MutationRefused(CoreMutationRefusal::OwnerRequired)),
        }
        self.check_drained()?;
        let generation = self
            .generation
            .checked_add(1)
            .ok_or(Error::CounterExhausted)?;
        let revision = match &mode {
            InstallMode::Peer { revision, .. } => *revision,
            _ => TimelineRevision(
                self.revision
                    .0
                    .checked_add(1)
                    .ok_or(Error::CounterExhausted)?,
            ),
        };
        let (epoch, branch) = match mode {
            InstallMode::NewEpoch => (None, None),
            InstallMode::AtEpoch(epoch) => (Some(epoch), None),
            InstallMode::Peer { epoch, branch, .. } => (Some(epoch), Some(branch)),
        };
        let history = self.history.as_mut().ok_or(Error::Retired)?;
        let position = match epoch {
            Some(epoch) => history.install_snapshot_at_epoch(snapshot, epoch)?,
            None => history.install_snapshot(snapshot)?,
        };
        if let Some(branch) = branch {
            self.branch = branch;
        }
        self.generation = generation;
        self.revision = revision;
        // Installation replaces the command line. Even continuity on the same
        // actor needs a fresh stream lease for its new epoch and accepted key.
        self.producer = None;
        // No fallible action follows installation. A frozen owner obtains a new
        // cut before explicitly resuming; capture backpressure remains retryable.
        Ok(ChangeId {
            actor: self.identity,
            generation,
            revision,
            position,
        })
    }
}
impl State {
    #[expect(
        clippy::too_many_lines,
        reason = "one exhaustive actor message dispatch keeps the ownership transitions visible"
    )]
    fn run(&mut self, receiver: &Receiver<Message>) {
        while let Ok(message) = receiver.recv() {
            match message {
                Message::Command(message) => {
                    let CommandMessage {
                        command,
                        context,
                        permit,
                        reply,
                    } = *message;
                    let query = matches!(command, Command::Query(_));
                    let token = command_token(&command);
                    let result = self
                        .execute(command, context, permit)
                        .map_err(|error| Refusal { error, token });
                    if !query {
                        self.broadcast(&result);
                    }
                    if let Some(reply) = reply {
                        let _ = reply.send(result);
                    }
                }
                Message::Producer(request) => self.bind_producer(*request),
                Message::CurrentProducer(permit, reply) => {
                    let _ = reply.send(self.current_producer(permit).is_ok());
                }
                Message::Pending(permit, reply) => {
                    let result = self
                        .current_producer_mut(permit)
                        .map(|producer| producer.stream.pending());
                    let _ = reply.send(result);
                }
                Message::PendingCount(permit, reply) => {
                    let result = self
                        .current_producer(permit)
                        .map(|producer| producer.stream.pending_count());
                    let _ = reply.send(result);
                }
                Message::Acknowledge(owner, permit, receipts, reply) => {
                    let result = self.acknowledge(owner, permit, &receipts);
                    let _ = reply.send(result);
                }
                Message::Close(permit, reply) => {
                    let result = self.close(permit);
                    let _ = reply.send(result);
                }
                Message::Subscribe(subscriber) => {
                    let initial = self
                        .history()
                        .map(|history| Event::DocumentChanged {
                            doc: Arc::clone(history.history().document()),
                            revision: self.revision,
                            last_op: None,
                            journal_command: None,
                            token: None,
                        })
                        .map_err(|error| Refusal { error, token: None });
                    if subscriber.send(initial).is_ok() {
                        self.subscribers.push(subscriber);
                    }
                }
                Message::Fork(role, reply) => {
                    let result = self
                        .history()
                        .and_then(|owner| owner.fork_current(role, true).map_err(Error::from))
                        .map(|owner| (owner, self.revision));
                    let _ = reply.send(result);
                }
                Message::AttachmentState(reply) => {
                    let _ = reply.send(self.attachment);
                }
                Message::Identity(reply) => {
                    let _ = reply.send(
                        self.history()
                            .and_then(|history| history.identity().map_err(Error::from)),
                    );
                }
                Message::Baseline(reply) => {
                    let _ = reply.send(
                        self.history()
                            .map(|owner| Arc::clone(owner.history().baseline())),
                    );
                }
                Message::Branch(reply) => {
                    let _ = reply.send(self.branch.clone());
                }
                Message::Change(reply) => {
                    let _ = reply.send(self.change());
                }
                Message::Preview(command, reply) => {
                    let result = (|| {
                        let mut preview = self.history()?.private_preview()?;
                        let origin = CreationOrigin {
                            branch: self.branch.clone(),
                            batch: BatchId::fresh()
                                .map_err(|error| Error::System(error.to_string()))?,
                        };
                        preview.execute(command, origin, vec![], None)?;
                        crate::operation_history::OwnerRead::new(preview)
                            .into_capture()
                            .map_err(Error::from)
                    })();
                    let _ = reply.send(result);
                }
                Message::BatchCapacity(charge, reply) => {
                    let result = self.live().and_then(|()| {
                        self.history()?
                            .history()
                            .check_batch_capacity(charge)
                            .map_err(HistoryError::from)
                            .map_err(Error::from)
                    });
                    let _ = reply.send(result);
                }
                Message::Capture(reply) => {
                    let result = self
                        .history()
                        .and_then(|history| history.capture().map_err(Error::from))
                        .and_then(|snapshot| {
                            self.change().map(|change| CapturedHistory {
                                change,
                                branch: self.branch.clone(),
                                snapshot,
                            })
                        });
                    let _ = reply.send(result);
                }
                Message::Approval(reference, reply) => {
                    let result = self.history().and_then(|history| {
                        history.approval_answer(&reference).map_err(Error::from)
                    });
                    let _ = reply.send(result);
                }
                Message::Counters(reply) => {
                    let _ = reply.send((
                        kinewright_core::undo::PhaseCounters::read(),
                        crate::operation_history::OwnerCounters::read(),
                    ));
                }
                Message::Operations(recorded, reply) => {
                    let result = self.history().and_then(|history| {
                        if recorded && history.history().op_log().is_none() {
                            return Err(Error::RecorderDisabled);
                        }
                        history.operations(recorded).map_err(Error::from)
                    });
                    let _ = reply.send(result);
                }
                Message::Boundary(depth, reply) => {
                    let result = self
                        .history()
                        .and_then(|history| history.boundary(depth).map_err(Error::from));
                    let _ = reply.send(result);
                }
                Message::Bind(owner, reply) => {
                    let result = if self.history.is_some()
                        && (self.owner == Some(owner)
                            || (self.owner.is_none() && self.attachment == AttachmentState::Live))
                    {
                        self.owner = Some(owner);
                        Ok(())
                    } else {
                        Err(Error::MutationRefused(CoreMutationRefusal::OwnerRequired))
                    };
                    let _ = reply.send(result);
                }
                Message::Freeze(owner, reply) => {
                    let result = self.freeze(owner);
                    let _ = reply.send(result);
                }
                Message::AcknowledgeFrozen(authority, cut, receipts, reply) => {
                    let result = self.check_cut(authority, cut).and_then(|()| {
                        let bound = self.producer.as_mut().ok_or(Error::Protocol(
                            crate::operation_protocol::Conflict::Detached,
                        ))?;
                        bound.stream.acknowledge(&receipts).map_err(Error::Protocol)
                    });
                    let _ = reply.send(result);
                }
                Message::FreezePublished(owner, cut, reply) => {
                    let result = (|| {
                        if self.owner != Some(owner) || self.attachment == AttachmentState::Sealed {
                            return Err(Error::MutationRefused(CoreMutationRefusal::OwnerRequired));
                        }
                        self.check_drained()?;
                        if self.change()? != cut.change {
                            return Err(Error::CaptureChanged);
                        }
                        self.attachment = AttachmentState::Frozen;
                        Ok(())
                    })();
                    let _ = reply.send(result);
                }
                Message::Cut(owner, reply) => {
                    let _ = reply.send(self.capture_cut(owner));
                }
                Message::Finish(owner, cut, seal, reply) => {
                    let result = self.check_cut(owner, cut).and_then(|()| {
                        if !seal
                            && self
                                .producer
                                .as_ref()
                                .is_some_and(|bound| bound.stream.position() != cut.change.position)
                        {
                            return Err(Error::Protocol(
                                crate::operation_protocol::Conflict::AcceptedKey,
                            ));
                        }
                        if !seal {
                            self.history
                                .as_mut()
                                .ok_or(Error::Retired)?
                                .promote_live()?;
                        }
                        self.attachment = if seal {
                            AttachmentState::Sealed
                        } else {
                            AttachmentState::Live
                        };
                        Ok(())
                    });
                    let _ = reply.send(result);
                }
                Message::Install(owner, cut, snapshot, epoch, reply) => {
                    let result = self.install(owner, cut, *snapshot, epoch);
                    if result.is_ok() {
                        let notice = self
                            .history()
                            .map(|history| Event::DocumentChanged {
                                doc: Arc::clone(history.history().document()),
                                revision: self.revision,
                                last_op: None,
                                journal_command: None,
                                token: None,
                            })
                            .map_err(|error| Refusal { error, token: None });
                        self.broadcast(&notice);
                    }
                    let _ = reply.send(result);
                }
                Message::Prepare(request) => self.prepare_update(&request),
                Message::PreviewManual(authority, cut, reply) => {
                    let _ = reply.send(self.preview_manual(authority, cut));
                }
                Message::Selection(authority, cut, snapshot, reply) => {
                    let result = self.prepare_selection(authority, cut, *snapshot);
                    let _ = reply.send(result);
                }
                Message::RetargetSelection(authority, cut, branch, reply) => {
                    let result = self.check_cut(authority, cut).and_then(|()| {
                        self.check_drained()?;
                        if self.producer.is_some() {
                            return Err(Error::Protocol(
                                crate::operation_protocol::Conflict::Detached,
                            ));
                        }
                        let generation = self
                            .generation
                            .checked_add(1)
                            .ok_or(Error::CounterExhausted)?;
                        self.branch = branch;
                        self.generation = generation;
                        Ok(AcceptedCut::committed(self.change()?, cut.identity))
                    });
                    let _ = reply.send(result);
                }
                Message::SealSelection(authority, cut, mut store, bindings, reply) => {
                    let result = self.check_cut(authority, cut).and_then(|()| {
                        self.check_drained()?;
                        let history = self.history()?;
                        history
                            .persist_prepared(&mut store, history.creation_sizing(), |resource| {
                                Ok(bindings.get(resource).cloned())
                            })
                            .map_err(Error::from)
                    });
                    let _ = reply.send(result);
                }
                Message::CommitPrepared(owner, cut, prepared, reply) => {
                    let result = self.commit_update(owner, cut, *prepared);
                    let _ = reply.send(result);
                }
                Message::Retire(owner, cut, reply) => {
                    let result =
                        if self.owner != Some(owner) || self.attachment == AttachmentState::Live {
                            Err(Error::MutationRefused(CoreMutationRefusal::OwnerRequired))
                        } else {
                            self.change().and_then(|change| {
                                if cut.change != change {
                                    return Err(Error::CaptureChanged);
                                }
                                // Drop before replying, rather than waiting for stale client handles.
                                drop(self.history.take());
                                drop(self.producer.take());
                                self.attachment = AttachmentState::Sealed;
                                self.subscribers.clear();
                                Ok(())
                            })
                        };
                    let _ = reply.send(result);
                }
            }
        }
    }
    fn broadcast(&mut self, event: &Notification) {
        self.subscribers
            .retain(|subscriber| subscriber.send(event.clone()).is_ok());
    }
    fn live(&self) -> Result<(), Error> {
        match self.attachment {
            AttachmentState::Live => {
                self.history()?;
                Ok(())
            }
            AttachmentState::Frozen => Err(Error::MutationRefused(CoreMutationRefusal::Frozen)),
            AttachmentState::Sealed => Err(Error::MutationRefused(CoreMutationRefusal::Sealed)),
        }
    }
    fn close(&mut self, permit: Option<u64>) -> Result<Effects, Error> {
        self.live()?;
        self.check_permit(permit)?;
        let generation = self
            .generation
            .checked_add(1)
            .ok_or(Error::CounterExhausted)?;
        let context = self.default_context()?;
        let effects = self.apply_action(crate::operation_protocol::Action::Close, context)?;
        self.generation = generation;
        Ok(effects)
    }
    fn query(&self, query: &Query) -> Result<Event, Error> {
        let history = self.history()?.history();
        Ok(Event::QueryResult(match query {
            Query::Document => QueryResult::Document(Arc::clone(history.document())),
            Query::Snapshot => QueryResult::Snapshot {
                revision: self.revision,
                document: Arc::clone(history.document()),
            },
            Query::Clip(id) => QueryResult::Clip(
                history
                    .document()
                    .tracks
                    .iter()
                    .flat_map(|track| &track.clips)
                    .find(|clip| clip.id == *id)
                    .cloned(),
            ),
        }))
    }
    #[expect(
        clippy::too_many_lines,
        reason = "normalise revision preconditions, admit the atomic owner transition, then emit exactly one notification"
    )]
    fn execute(
        &mut self,
        command: Command,
        context: Option<CommandContext>,
        permit: Option<u64>,
    ) -> Result<Event, Error> {
        if let Command::Query(query) = command {
            return self.query(&query);
        }
        self.live()?;
        self.check_permit(permit)?;
        let (command, token) = match command {
            Command::DoIfRevision {
                expected,
                operation,
                token,
            } => {
                if expected != self.revision {
                    return Ok(Event::RevisionConflict {
                        expected,
                        actual: self.revision,
                        token,
                    });
                }
                (Command::Do(operation), token)
            }
            Command::DoBatchIfRevision {
                expected,
                operations,
            } => {
                if expected != self.revision {
                    return Ok(Event::RevisionConflict {
                        expected,
                        actual: self.revision,
                        token: None,
                    });
                }
                (Command::DoBatch(operations), None)
            }
            other => (other, None),
        };
        if matches!(&command, Command::DoBatch(operations) | Command::DoBatchCoalesced { operations, .. } if operations.is_empty())
        {
            return Ok(Event::BatchRejected {
                operations: vec![],
                error: kinewright_core::BatchError::Empty,
            });
        }
        // The notification and the transport are independent accepted-command readers.
        let journal = match &command {
            Command::Do(operation) => JournalCommand::Do(operation.clone()),
            Command::DoBatch(operations) => JournalCommand::DoBatch(operations.clone()),
            Command::DoBatchCoalesced {
                operations,
                coalesce_key,
            } => JournalCommand::DoBatchCoalesced {
                operations: operations.clone(),
                coalesce_key: coalesce_key.clone(),
            },
            Command::Undo => JournalCommand::Undo,
            Command::Redo => JournalCommand::Redo,
            _ => unreachable!("revision preconditions and queries were normalised above"),
        };
        let changed = match &command {
            Command::Undo => !self.history()?.history().undo().is_empty(),
            Command::Redo => !self.history()?.history().redo().is_empty(),
            _ => true,
        };
        let revision = if changed {
            TimelineRevision(
                self.revision
                    .0
                    .checked_add(1)
                    .ok_or(Error::CounterExhausted)?,
            )
        } else {
            self.revision
        };
        let generation = self
            .generation
            .checked_add(1)
            .ok_or(Error::CounterExhausted)?;
        let context = match context {
            Some(context) => context,
            None => self.default_context()?,
        };
        let action =
            crate::operation_protocol::Action::from_normalised(command).map_err(Error::Protocol)?;
        let result = self.apply_action(action, context);
        match result {
            Ok(_) => {
                self.revision = revision;
                self.generation = generation;
                let last_op = match &journal {
                    JournalCommand::Do(operation) => Some(operation.clone()),
                    _ => None,
                };
                Ok(Event::DocumentChanged {
                    doc: Arc::clone(self.history()?.history().document()),
                    revision,
                    last_op,
                    journal_command: Some(journal),
                    token,
                })
            }
            Err(Error::History(HistoryError::History(kinewright_core::undo::Error::Batch(
                error,
            )))) => match journal {
                JournalCommand::Do(operation) => match error {
                    kinewright_core::BatchError::OperationFailed { error, .. } => {
                        Ok(Event::OpRejected {
                            op: operation,
                            error,
                            token,
                        })
                    }
                    other @ kinewright_core::BatchError::Empty => Err(HistoryError::History(
                        kinewright_core::undo::Error::Batch(other),
                    )
                    .into()),
                },
                JournalCommand::DoBatch(operations)
                | JournalCommand::DoBatchCoalesced { operations, .. } => {
                    Ok(Event::BatchRejected { operations, error })
                }
                _ => Err(HistoryError::History(kinewright_core::undo::Error::Batch(error)).into()),
            },
            Err(error) => Err(error),
        }
    }
}

#[cfg(test)]
#[path = "operation_actor_tests.rs"]
mod tests;
