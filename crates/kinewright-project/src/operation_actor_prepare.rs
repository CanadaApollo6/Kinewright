//! Publication preparation holds one independent working lease. The live
//! destination stays frozen until its prospective record is durably published.
use super::{
    AcceptedCut, Arc, Attachment, ChangeId, CommandContext, CommandKey, Document, Effects, Error,
    Event, HistoryError, Message, ObjectRef, Sender, SnapshotIdentity, State, TimelineRevision,
    TypeId,
};
use crate::operation_history::{CreationSizing, OwnerHistory, PreparedOwner, Resource};
use crate::operation_protocol::Action;

pub struct PreparedUpdate {
    before: ChangeId,
    revision: TimelineRevision,
    generation: u64,
    position: CommandKey,
    owner: PreparedOwner,
    effects: Effects,
    event: Event,
    batch: Option<crate::bundle_records::BatchId>,
}

/// One creation-sized working copy for the entire unpublished manual prefix.
pub(crate) struct ManualPreview {
    owner: OwnerHistory,
    before: ChangeId,
    revision: TimelineRevision,
    generation: u64,
    sizing: CreationSizing,
    bounds: kinewright_core::undo::RoleBounds,
    effects: Option<Effects>,
}
impl ManualPreview {
    pub(crate) fn position(&self) -> CommandKey {
        self.owner.history().position()
    }

    pub(crate) fn apply(&mut self, action: Action, context: CommandContext) -> Result<(), Error> {
        let revision = next_revision(self.revision, changes_timeline(&action, &self.owner))?;
        let generation = self
            .generation
            .checked_add(1)
            .ok_or(Error::CounterExhausted)?;
        // OwnerHistory applies each command atomically, including its ledger
        // and command boundaries. A refusal leaves the previous prefix intact.
        self.effects = Some(action.execute(context, &mut self.owner)?);
        self.generation = generation;
        self.revision = revision;
        Ok(())
    }

    pub(crate) fn finish(mut self) -> Result<Option<PreparedUpdate>, Error> {
        let Some(effects) = self.effects.take() else {
            return Ok(None);
        };
        let position = self.position();
        let event = prepared_event(&self.owner, self.revision);
        Ok(Some(PreparedUpdate {
            before: self.before,
            revision: self.revision,
            generation: self.generation,
            position,
            owner: self.owner.seal_preview(self.sizing, self.bounds)?,
            effects,
            event,
            batch: None,
        }))
    }
}

fn changes_timeline(action: &Action, owner: &OwnerHistory) -> bool {
    match action {
        Action::Undo => !owner.history().undo().is_empty(),
        Action::Redo => !owner.history().redo().is_empty(),
        Action::Close => false,
        Action::Do { .. } => true,
    }
}

fn next_revision(revision: TimelineRevision, changed: bool) -> Result<TimelineRevision, Error> {
    if changed {
        Ok(TimelineRevision(
            revision.0.checked_add(1).ok_or(Error::CounterExhausted)?,
        ))
    } else {
        Ok(revision)
    }
}

fn prepared_event(owner: &OwnerHistory, revision: TimelineRevision) -> Event {
    Event::DocumentChanged {
        doc: Arc::clone(owner.history().document()),
        revision,
        last_op: None,
        journal_command: None,
        token: None,
    }
}
impl PreparedUpdate {
    pub(crate) fn accepted_batch(&self) -> Option<&crate::bundle_records::BatchId> {
        self.batch.as_ref()
    }
    pub(crate) fn input_contexts(&self) -> impl Iterator<Item = &ObjectRef> {
        self.owner.input_contexts()
    }

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
    #[must_use]
    pub const fn revision(&self) -> TimelineRevision {
        self.revision
    }
    pub fn persist_checkpoint(
        &self,
        store: &mut crate::store::ProjectStore,
        resolve: impl FnMut(&Resource) -> Result<Option<ObjectRef>, HistoryError>,
    ) -> Result<crate::operation_history::SealedOwner, Error> {
        self.owner
            .persist_checkpoint(store, resolve)
            .map_err(Error::from)
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
    kind: Preparation,
    reply: Sender<Result<PreparedUpdate, Error>>,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Preparation {
    Command,
    Merge,
    Install,
    Reinstall,
}
impl<P: 'static> Attachment<P> {
    pub(crate) fn preview_manual(
        &self,
        _proof: &P,
        cut: AcceptedCut,
    ) -> Result<ManualPreview, Error> {
        self.core
            .call(|reply| Message::PreviewManual(TypeId::of::<P>(), cut, reply))?
    }

    pub(crate) fn freeze_published(
        &self,
        _proof: &P,
        identity: SnapshotIdentity,
        position: CommandKey,
        revision: TimelineRevision,
    ) -> Result<AcceptedCut, Error> {
        let change = self.core.change_id()?;
        if change.position != position || change.revision != revision {
            return Err(Error::CaptureChanged);
        }
        let cut = AcceptedCut::committed(change, identity);
        self.core
            .call(|reply| Message::FreezePublished(TypeId::of::<P>(), cut, reply))??;
        Ok(cut)
    }
    pub(crate) fn retarget_selection(
        &self,
        _proof: &P,
        cut: AcceptedCut,
        branch: crate::bundle_records::BranchId,
    ) -> Result<AcceptedCut, Error> {
        self.core
            .call(|reply| Message::RetargetSelection(TypeId::of::<P>(), cut, branch, reply))?
    }
    pub(crate) fn seal_selection(
        &self,
        _proof: &P,
        cut: AcceptedCut,
        store: &crate::store::ProjectStore,
        bindings: crate::storage_budget::StorageReservation<
            std::collections::BTreeMap<crate::operation_history::Resource, ObjectRef>,
        >,
    ) -> Result<crate::operation_history::SealedOwner, Error> {
        let store = store.actor_object_store();
        self.core
            .call(|reply| Message::SealSelection(TypeId::of::<P>(), cut, store, bindings, reply))?
    }
    /// A person's selected saved state is prepared under the incoming capture's
    /// already admitted full lease. The frozen source is kept until publication.
    pub fn prepare_selection(
        &self,
        _proof: &P,
        cut: AcceptedCut,
        snapshot: crate::operation_history::ValidatedOwnerSnapshot,
        branch: crate::bundle_records::BranchId,
    ) -> Result<super::Core, Error> {
        let (history, revision) = self.core.call(|reply| {
            Message::Selection(TypeId::of::<P>(), cut, Box::new(snapshot), reply)
        })??;
        super::Core::spawn_staged(history, branch, revision, TypeId::of::<P>())
    }
    /// Materialise a validated state whose install epoch is already chosen.
    pub(crate) fn open_validated_selection(
        &self,
        proof: &P,
        cut: AcceptedCut,
        snapshot: crate::operation_history::ValidatedOwnerSnapshot,
        branch: crate::bundle_records::BranchId,
        revision: TimelineRevision,
    ) -> Result<super::Core, Error> {
        if self.capture(proof)? != cut {
            return Err(Error::CaptureChanged);
        }
        super::Core::spawn_staged(
            snapshot.into_reader()?.into_history(),
            branch,
            revision,
            TypeId::of::<P>(),
        )
    }
    pub fn prepare(
        &self,
        _proof: &P,
        cut: AcceptedCut,
        action: Action,
        context: CommandContext,
    ) -> Result<PreparedUpdate, Error> {
        self.prepare_inner(cut, action, context, Preparation::Command)
    }
    /// A native command whose saved state the GUI will install. Pick the
    /// common epoch and install provenance in the private holder before Save.
    pub fn prepare_install(
        &self,
        _proof: &P,
        cut: AcceptedCut,
        action: Action,
        context: CommandContext,
    ) -> Result<PreparedUpdate, Error> {
        self.prepare_inner(cut, action, context, Preparation::Install)
    }
    pub(crate) fn prepare_reinstall(
        &self,
        _proof: &P,
        cut: AcceptedCut,
        context: CommandContext,
    ) -> Result<PreparedUpdate, Error> {
        // No accepted command is minted for an installation of the same state.
        self.prepare_inner(cut, Action::Close, context, Preparation::Reinstall)
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
            Preparation::Merge,
        )
    }
    fn prepare_inner(
        &self,
        cut: AcceptedCut,
        action: Action,
        context: CommandContext,
        kind: Preparation,
    ) -> Result<PreparedUpdate, Error> {
        self.core.call(|reply| {
            Message::Prepare(Box::new(PrepareRequest {
                authority: TypeId::of::<P>(),
                cut,
                action,
                context,
                kind,
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
    pub(super) fn preview_manual(
        &self,
        authority: TypeId,
        cut: AcceptedCut,
    ) -> Result<ManualPreview, Error> {
        self.check_cut(authority, cut)?;
        self.check_drained()?;
        let current = self.history()?;
        Ok(ManualPreview {
            owner: current.private_preview()?,
            before: cut.change,
            revision: self.revision,
            generation: self.generation,
            sizing: current.creation_sizing(),
            bounds: current.history().bounds(),
            effects: None,
        })
    }

    pub(super) fn prepare_selection(
        &self,
        authority: TypeId,
        cut: AcceptedCut,
        snapshot: crate::operation_history::ValidatedOwnerSnapshot,
    ) -> Result<(crate::operation_history::OwnerHistory, TimelineRevision), Error> {
        self.check_cut(authority, cut)?;
        self.check_drained()?;
        let current = self.history()?;
        if current.creation_sizing().kind != crate::operation_history::CreationKind::Ordinary {
            return Err(HistoryError::InvalidPolicy.into());
        }
        let epoch = current
            .history()
            .position()
            .epoch
            .max(snapshot.position().epoch)
            .checked_add(1)
            .ok_or(Error::CounterExhausted)?;
        let revision = TimelineRevision(
            self.revision
                .0
                .checked_add(1)
                .ok_or(Error::CounterExhausted)?,
        );
        let selected = current.prepare_selection(snapshot, epoch)?;
        Ok((selected, revision))
    }
    pub(super) fn prepare_update(&self, request: &PrepareRequest) {
        let result = self.prepare_state(
            request.authority,
            request.cut,
            &request.action,
            &request.context,
            request.kind,
        );
        let _ = request.reply.send(result);
    }
    fn prepare_state(
        &self,
        authority: TypeId,
        cut: AcceptedCut,
        action: &Action,
        context: &CommandContext,
        kind: Preparation,
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
        let revision = next_revision(
            self.revision,
            changes_timeline(action, current)
                || matches!(kind, Preparation::Install | Preparation::Reinstall),
        )?;
        let generation = self
            .generation
            .checked_add(1)
            .ok_or(Error::CounterExhausted)?;
        let mut preview = if kind == Preparation::Merge {
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
        let effects = if kind == Preparation::Reinstall {
            Effects {
                accepted: preview.history().position(),
                to_undo: vec![],
                to_redo: vec![],
                discarded: vec![],
                folded: vec![],
                applied: vec![],
            }
        } else {
            action.apply(context, &mut preview)?
        };
        if matches!(kind, Preparation::Install | Preparation::Reinstall) {
            preview.prepare_peer_install(current)?;
        }
        let position = preview.history().position();
        let event = prepared_event(&preview, revision);
        let owner = preview.seal_preview(current.creation_sizing(), current.history().bounds())?;
        Ok(PreparedUpdate {
            before: cut.change,
            revision,
            generation,
            position,
            owner,
            effects,
            event,
            batch: matches!(action, Action::Do { .. }).then(|| context.creation.batch.clone()),
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
        if cut.change.position.epoch != prepared.position.epoch {
            self.producer = None;
        }
        self.broadcast(&Ok(prepared.event));
        Ok(ChangeId {
            actor: self.identity,
            revision: self.revision,
            generation: self.generation,
            position: prepared.position,
        })
    }
}
