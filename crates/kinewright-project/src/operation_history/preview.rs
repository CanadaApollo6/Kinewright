//! A private working holder is admitted before any payload copy. Sealing it
//! records the original destination profile, without a snapshot-install epoch.
use super::{CreationKind, CreationSizing, Error, OwnerHistory};

pub(crate) struct PreparedOwner {
    history: OwnerHistory,
    destination: CreationSizing,
}
impl OwnerHistory {
    pub(crate) fn prepare_peer_install(&mut self, previous: &Self) -> Result<(), Error> {
        let epoch = self.core.begin_install_epoch()?.epoch;
        // Both endpoints start from the agreed cut. The preview already owns
        // every approval in that cut, including those discarded by this edit.
        // Match the install's fate normalisation and provenance before hashing
        // or publishing, including a newly admitted approval absent previously.
        for entry in self.ledger.commands.values_mut() {
            if entry.fate == super::Fate::Redone {
                entry.fate = super::Fate::Applied;
            }
            if previous
                .ledger
                .get_approval(&entry.approval.reference)
                .map(|old| old.fate)
                != Some(entry.fate)
            {
                entry.install_epoch = Some(epoch);
            }
        }
        Ok(())
    }
    pub(crate) fn private_preview(&self) -> Result<Self, Error> {
        self.preview_sized(self.reservation.sizing())
    }
    pub(crate) fn merge_preview(
        &self,
        incoming: &[kinewright_core::Operation],
    ) -> Result<Self, Error> {
        let mut sizing = self.reservation.sizing();
        if sizing.kind != CreationKind::LiveCandidate {
            return Err(Error::InvalidPolicy);
        }
        sizing.kind = CreationKind::StagingCandidate;
        // The holder briefly owns the old Redo tail too. Include its payload
        // in the measured creation need; the new Do will discard it atomically.
        sizing.initial_batch = kinewright_core::undo::batch_charge_iter(
            self.core
                .undo()
                .iter()
                .chain(self.core.redo())
                .flat_map(kinewright_core::undo::Step::operations)
                .chain(incoming),
        )?;
        self.preview_sized(sizing)
    }
    fn preview_sized(&self, sizing: CreationSizing) -> Result<Self, Error> {
        let reservation = self.reservation.pool().admit_sized(sizing)?;
        let _window = reservation.workspace()?;
        let limits = reservation.limits();
        if self.owner_bytes > limits.envelope.owners
            || self.decode_bound > limits.envelope.workspace.min(super::stored::DECODE_LIMIT)
        {
            return Err(Error::Capacity("preview owner metadata"));
        }
        let core = self.core.fork_with_bounds(limits.bounds)?;
        let seeds = self.seeds();
        let fold = self
            .fold
            .as_ref()
            .map(|_| {
                super::fold::FoldState::new(&seeds, core.baseline(), limits.envelope.workspace)
            })
            .transpose()?;
        let origins = super::owner::Origins(
            self.origins
                .0
                .iter()
                .map(|row| super::owner::LiveOrigin {
                    origin: row.origin.clone(),
                    resources: row.resources.clone(),
                    charge: row.charge,
                    decode_charge: row.decode_charge,
                    step_first: row.step_first,
                })
                .collect(),
        );
        Ok(Self {
            core,
            seeds,
            fold,
            origins,
            reservation,
            ledger: self.ledger.clone(),
            folded_approvals: self.folded_approvals.clone(),
            resources: self.resources.clone(),
            terminal_facts: self.terminal_facts.clone(),
            owner_bytes: self.owner_bytes,
            baseline_bytes: self.baseline_bytes,
            decode_bound: self.decode_bound,
            baseline_decode: self.baseline_decode,
            fold_frontier: self.fold_frontier,
        })
    }
    pub(crate) fn seal_preview(
        mut self,
        destination: CreationSizing,
    ) -> Result<PreparedOwner, Error> {
        let limits = destination.limits()?;
        if destination.kind.role() != self.reservation.role()
            || destination.baseline_live != self.reservation.sizing().baseline_live
            || destination.baseline_owners != self.reservation.sizing().baseline_owners
            || self.owner_bytes > limits.envelope.owners
            || self.decode_bound > limits.envelope.workspace.min(super::stored::DECODE_LIMIT)
        {
            return Err(Error::Capacity("prepared destination"));
        }
        self.core = self.core.into_bounds(limits.bounds)?;
        Ok(PreparedOwner {
            history: self,
            destination,
        })
    }
    pub(crate) fn commit_prepared(&mut self, prepared: PreparedOwner) -> Result<(), Error> {
        if prepared.destination != self.reservation.sizing()
            || !self.reservation.shares_pool(&prepared.history.reservation)
        {
            return Err(Error::Reservation);
        }
        // The preview constructor checked the old state against its lease too.
        // Keep the destination's immutable reservation and drop the replaced
        // state under the already admitted preview holder before acknowledging.
        let mut old = std::mem::replace(self, prepared.history);
        std::mem::swap(&mut self.reservation, &mut old.reservation);
        drop(old);
        Ok(())
    }
}
impl PreparedOwner {
    pub(crate) fn identity(&self) -> Result<super::SnapshotIdentity, Error> {
        let _window = self.history.reservation.workspace()?;
        self.history.identity_unreserved(self.destination)
    }
    pub(crate) fn document(&self) -> &kinewright_core::Document {
        self.history.core.document()
    }
    pub(crate) fn persist(
        &self,
        store: &mut crate::store::ProjectStore,
        resolve: impl FnMut(&super::Resource) -> Result<Option<crate::bundle_records::ObjectRef>, Error>,
    ) -> Result<crate::store::SealedObject, Error> {
        self.persist_checkpoint(store, resolve)
            .map(super::stored::SealedOwner::into_object)
    }
    pub(crate) fn persist_checkpoint(
        &self,
        store: &mut crate::store::ProjectStore,
        resolve: impl FnMut(&super::Resource) -> Result<Option<crate::bundle_records::ObjectRef>, Error>,
    ) -> Result<super::stored::SealedOwner, Error> {
        self.history
            .persist_prepared(store, self.destination, resolve)
    }
}
