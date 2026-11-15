//! Publication preparation holds one independent working lease. The live
//! destination stays frozen until its prospective record is durably published.
use super::{
    AcceptedCut, Arc, Attachment, ChangeId, CommandContext, CommandKey, Document, Effects, Error,
    Event, HistoryError, Message, ObjectRef, Sender, SnapshotIdentity, State, TimelineRevision,
    TypeId,
};
use crate::operation_history::{PreparedOwner, Resource};
use crate::operation_protocol::Action;

pub struct PreparedUpdate {
    before: ChangeId,
    revision: TimelineRevision,
    generation: u64,
    position: CommandKey,
    owner: PreparedOwner,
    effects: Effects,
    event: Event,
}
impl PreparedUpdate {
    #[must_use]
    pub const fn effects(&self) -> &Effects {
        &self.effects
    }
    #[must_use]
    pub fn document(&self) -> &Document {
        self.owner.document()
    }
    pub fn identity(&self) -> Result<SnapshotIdentity, Error> {
        self.owner.identity().map_err(Error::from)
    }
    pub fn persist(
        &self,
        store: &mut crate::store::ProjectStore,
        resolve: impl FnMut(&Resource) -> Result<Option<ObjectRef>, HistoryError>,
    ) -> Result<crate::store::SealedObject, Error> {
        self.owner.persist(store, resolve).map_err(Error::from)
    }
}
pub(super) struct PrepareRequest {
    authority: TypeId,
    cut: AcceptedCut,
    action: Action,
    context: CommandContext,
    merge: bool,
    reply: Sender<Result<PreparedUpdate, Error>>,
}
impl<P: 'static> Attachment<P> {
    pub fn prepare(
        &self,
        _proof: &P,
        cut: AcceptedCut,
        action: Action,
        context: CommandContext,
    ) -> Result<PreparedUpdate, Error> {
        self.prepare_inner(cut, action, context, false)
    }
    pub fn prepare_merge(
        &self,
        _proof: &P,
        cut: AcceptedCut,
        operations: Vec<kinewright_core::Operation>,
        context: CommandContext,
    ) -> Result<PreparedUpdate, Error> {
        self.prepare_inner(
            cut,
            Action::Do {
                operations,
                gesture: None,
            },
            context,
            true,
        )
    }
    fn prepare_inner(
        &self,
        cut: AcceptedCut,
        action: Action,
        context: CommandContext,
        merge: bool,
    ) -> Result<PreparedUpdate, Error> {
        self.core.call(|reply| {
            Message::Prepare(Box::new(PrepareRequest {
                authority: TypeId::of::<P>(),
                cut,
                action,
                context,
                merge,
                reply,
            }))
        })?
    }
    /// The owner calls this only after the prospective root and receipt are
    /// known durable. Uncertain publication keeps this holder for reconciliation.
    pub fn commit_prepared(
        &self,
        _proof: &P,
        cut: AcceptedCut,
        prepared: PreparedUpdate,
    ) -> Result<ChangeId, Error> {
        self.core.call(|reply| {
            Message::CommitPrepared(TypeId::of::<P>(), cut, Box::new(prepared), reply)
        })?
    }
}
impl State {
    pub(super) fn prepare_update(&self, request: &PrepareRequest) {
        let result = self.prepare_state(
            request.authority,
            request.cut,
            &request.action,
            &request.context,
            request.merge,
        );
        let _ = request.reply.send(result);
    }
    fn prepare_state(
        &self,
        authority: TypeId,
        cut: AcceptedCut,
        action: &Action,
        context: &CommandContext,
        merge: bool,
    ) -> Result<PreparedUpdate, Error> {
        self.check_cut(authority, cut)?;
        self.check_drained()?;
        if matches!(action, Action::Do { operations, .. } if operations.is_empty()) {
            return Err(HistoryError::History(kinewright_core::undo::Error::Batch(
                kinewright_core::BatchError::Empty,
            ))
            .into());
        }
        let current = self.history()?;
        let changed = match action {
            Action::Undo => !current.history().undo().is_empty(),
            Action::Redo => !current.history().redo().is_empty(),
            Action::Close => false,
            Action::Do { .. } => true,
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
        let mut preview = if merge {
            let Action::Do {
                operations,
                gesture: None,
            } = action
            else {
                return Err(HistoryError::InvalidPolicy.into());
            };
            current.merge_preview(operations)?
        } else {
            current.private_preview()?
        };
        let effects = action.apply(context, &mut preview)?;
        let position = preview.history().position();
        let event = Event::DocumentChanged {
            doc: Arc::clone(preview.history().document()),
            revision,
            last_op: None,
            journal_command: None,
            token: None,
        };
        let owner = preview.seal_preview(current.creation_sizing())?;
        Ok(PreparedUpdate {
            before: cut.change,
            revision,
            generation,
            position,
            owner,
            effects,
            event,
        })
    }
    pub(super) fn commit_update(
        &mut self,
        authority: TypeId,
        cut: AcceptedCut,
        prepared: PreparedUpdate,
    ) -> Result<ChangeId, Error> {
        self.check_cut(authority, cut)?;
        if prepared.before != cut.change {
            return Err(Error::CaptureChanged);
        }
        self.history
            .as_mut()
            .ok_or(Error::Retired)?
            .commit_prepared(prepared.owner)?;
        // All fallible admission precedes replacement. The original full live
        // lease is unchanged; the working lease and old state are already gone.
        self.revision = prepared.revision;
        self.generation = prepared.generation;
        self.broadcast(&Ok(prepared.event));
        Ok(ChangeId {
            actor: self.identity,
            revision: self.revision,
            generation: self.generation,
            position: prepared.position,
        })
    }
}
