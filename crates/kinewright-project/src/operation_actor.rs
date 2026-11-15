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
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
};

#[path = "operation_actor_prepare.rs"]
mod prepare;
#[path = "operation_actor_producer.rs"]
mod producer;
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
    LeasedQueryRequired,
    RecorderDisabled,
    System(String),
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
            other => write!(f, "{other:?}"),
        }
    }
}
impl std::error::Error for Error {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActorIdentity(u64);
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
    pub snapshot: OwnerSnapshot,
}
/// A frozen attachment proof contains no independently retained history.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AcceptedCut {
    change: ChangeId,
    identity: SnapshotIdentity,
}
impl AcceptedCut {
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
    handles: Arc<()>,
    pool: HistoryPool,
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
enum Message {
    Command(Box<CommandMessage>),
    Close(Option<u64>, Sender<Result<Effects, Error>>),
    Producer(Box<producer::BindRequest>),
    Pending(
        u64,
        Sender<Result<Vec<crate::operation_protocol::EncodedMessage>, Error>>,
    ),
    Acknowledge(
        TypeId,
        u64,
        Vec<crate::operation_protocol::Receipt>,
        Sender<Result<(), Error>>,
    ),
    Subscribe(Sender<Notification>),
    Change(Sender<Result<ChangeId, Error>>),
    Capture(Sender<Result<CapturedHistory, Error>>),
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
    Finish(TypeId, AcceptedCut, bool, Sender<Result<(), Error>>),
    Install(
        Option<TypeId>,
        Option<AcceptedCut>,
        Box<ValidatedOwnerSnapshot>,
        Option<u64>,
        Sender<Result<ChangeId, Error>>,
    ),
    Retire(TypeId, AcceptedCut, Sender<Result<(), Error>>),
    Prepare(Box<PrepareRequest>),
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
        let history =
            OwnerHistory::new(pool, role, role.policy(), &origin, || Ok(document.into()))?
                .recording()?;
        Self::spawn_owner(history, origin.branch, TimelineRevision::default())
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
        static NEXT_ACTOR: AtomicU64 = AtomicU64::new(1);
        let pool = history.pool();
        #[allow(deprecated, reason = "MSRV predates AtomicU64::try_update")]
        let identity = ActorIdentity(
            NEXT_ACTOR
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |old| {
                    old.checked_add(1)
                })
                .map_err(|_| Error::CounterExhausted)?,
        );
        let (sender, receiver) = unbounded();
        thread::Builder::new()
            .name("kinewright-core".into())
            .spawn(move || {
                State {
                    history: Some(history),
                    branch,
                    identity,
                    revision,
                    generation: 0,
                    owner: None,
                    attachment: AttachmentState::Live,
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
            handles: Arc::new(()),
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
    pub fn operations(&self, recorded: bool) -> Result<OwnerOperations, Error> {
        self.call(|reply| Message::Operations(recorded, reply))?
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
        self.call(|reply| Message::Install(None, None, Box::new(snapshot), None, reply))?
    }
}
impl<P: 'static> Attachment<P> {
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
                None,
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
                Some(epoch),
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
        self.authorize(owner)?;
        let change = self.change()?;
        // Hash a borrowed canonical view under scratch; the cut owns no extra
        // history lease even when the incoming replacement is already admitted.
        let identity = self.history()?.identity()?;
        self.attachment = AttachmentState::Frozen;
        Ok(AcceptedCut { change, identity })
    }
    fn install(
        &mut self,
        owner: Option<TypeId>,
        cut: Option<AcceptedCut>,
        snapshot: ValidatedOwnerSnapshot,
        epoch: Option<u64>,
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
        let revision = TimelineRevision(
            self.revision
                .0
                .checked_add(1)
                .ok_or(Error::CounterExhausted)?,
        );
        let history = self.history.as_mut().ok_or(Error::Retired)?;
        let position = match epoch {
            Some(epoch) => history.install_snapshot_at_epoch(snapshot, epoch)?,
            None => history.install_snapshot(snapshot)?,
        };
        self.generation = generation;
        self.revision = revision;
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
                Message::Pending(permit, reply) => {
                    let result = self
                        .current_producer(permit)
                        .map(|producer| producer.stream.pending());
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
                Message::Change(reply) => {
                    let _ = reply.send(self.change());
                }
                Message::Capture(reply) => {
                    let result = self
                        .history()
                        .and_then(|history| history.capture().map_err(Error::from))
                        .and_then(|snapshot| {
                            self.change()
                                .map(|change| CapturedHistory { change, snapshot })
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
                    let result = if self.owner.is_none_or(|previous| previous == owner)
                        && self.attachment == AttachmentState::Live
                        && self.history.is_some()
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
            // Large independently retained readers use capture/boundary and keep the lease.
            Query::AppliedOperations => return Err(Error::LeasedQueryRequired),
            Query::OpLog => {
                let _ = history.op_log().ok_or(Error::RecorderDisabled)?;
                return Err(Error::LeasedQueryRequired);
            }
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
