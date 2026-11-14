//! Owner captures carry an independent role lease, like any other retained history.
use super::owner::{LiveOrigin, Origins, approval_charge, baseline_charge, origin_charge};
use super::{
    ApprovalLedger, CommandOrigin, Error, Fate, Lineage, OwnerHistory, Resource, Resources, Role,
};
use kinewright_core::undo::{CommandKey, Record};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::Arc,
};

pub struct OwnerSnapshot {
    pub(super) data: SnapshotData,
    pub(super) reservation: super::HistoryReservation,
}
pub struct ValidatedOwnerSnapshot {
    core: kinewright_core::undo::ValidatedSnapshot,
    folded_approvals: super::ledger::FoldedApprovals,
    ledger: ApprovalLedger,
    seeds: Lineage,
    resources: Resources,
    origins: Origins,
    owner_bytes: u64,
    baseline_bytes: u64,
    decode_bound: u64,
    baseline_decode: u64,
    fold_frontier: Option<CommandKey>,
    terminal_facts: super::transition::TerminalFacts,
    reservation: super::HistoryReservation,
}
impl ValidatedOwnerSnapshot {
    pub fn open(self) -> Result<OwnerHistory, Error> {
        Ok(OwnerHistory {
            core: self.core.into_history()?,
            ledger: self.ledger,
            folded_approvals: self.folded_approvals,
            seeds: self.seeds,
            resources: self.resources,
            origins: self.origins,
            owner_bytes: self.owner_bytes,
            baseline_bytes: self.baseline_bytes,
            decode_bound: self.decode_bound,
            baseline_decode: self.baseline_decode,
            fold_frontier: self.fold_frontier,
            terminal_facts: self.terminal_facts,
            reservation: self.reservation,
        })
    }
}
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SnapshotData {
    pub role: Role,
    pub fold_frontier: Option<CommandKey>,
    #[serde(with = "super::transition::terminal_wire")]
    pub terminal_facts: super::transition::TerminalFacts,
    pub history: Record,
    pub seeds: Lineage,
    pub origins: Vec<CommandOrigin>,
    pub resources: BTreeSet<Resource>,
    pub ledger: ApprovalLedger,
    #[serde(with = "super::ledger::folded_wire")]
    pub folded_approvals: super::ledger::FoldedApprovals,
}
impl OwnerHistory {
    pub fn capture(&self) -> Result<OwnerSnapshot, Error> {
        let reservation = self.reservation.capture()?;
        let _workspace = reservation.workspace()?;
        // These owner values belong to an immutable capture which can outlive
        // subsequent edits, folds and installs in the source history.
        let data = SnapshotData {
            role: reservation.role(),
            fold_frontier: self.fold_frontier,
            terminal_facts: self.terminal_facts.clone(),
            history: self.core.record(),
            seeds: self.seeds.clone(),
            origins: self.origins.0.iter().map(|v| v.origin.clone()).collect(),
            resources: self.resources.values().cloned().collect(),
            ledger: self.ledger.clone(),
            folded_approvals: self.folded_approvals.clone(),
        };
        Ok(OwnerSnapshot { data, reservation })
    }
    #[expect(
        clippy::too_many_lines,
        reason = "validate all merged approval evidence before replacing either Core or owner state"
    )]
    pub fn install_snapshot(
        &mut self,
        mut snapshot: ValidatedOwnerSnapshot,
    ) -> Result<CommandKey, Error> {
        if self.reservation.role() != snapshot.reservation.role()
            || !self.reservation.shares_pool(&snapshot.reservation)
        {
            return Err(Error::Reservation);
        }
        // Installation replaces the history line. Keys are never re-keyed;
        // equal keys with different payloads cannot describe the same command.
        for old in &self.origins.0 {
            if snapshot
                .origins
                .get(&old.origin.key)
                .is_some_and(|new| new.origin.payload_digest != old.origin.payload_digest)
            {
                return Err(Error::CommandConflict);
            }
        }
        let install_epoch = self
            .core
            .position()
            .epoch
            .max(snapshot.core.epoch())
            .checked_add(1)
            .ok_or(Error::History(
                kinewright_core::undo::Error::CounterExhausted,
            ))?;
        for entry in self.ledger.entries() {
            if let Some(command) = snapshot.origins.get(&entry.key)
                && command.origin.payload_digest != entry.payload_digest
            {
                return Err(Error::CommandConflict);
            }
            if let Some(new) = snapshot.ledger.get(entry.key) {
                if new.payload_digest != entry.payload_digest {
                    return Err(Error::CommandConflict);
                }
                if new.approval != entry.approval {
                    return Err(Error::InvalidLedger);
                }
            } else {
                if snapshot.ledger.len() >= crate::orchestration_records::ITEM_LIMIT {
                    return Err(Error::Capacity("approval count"));
                }
                // This independent entry survives for its run's audit. Its
                // fate below comes from the selected stacks, not this copy.
                let mut old = entry.clone();
                old.fate = Fate::Discarded;
                old.recreated = false;
                snapshot.decode_bound = snapshot
                    .decode_bound
                    .checked_add(super::stored::approval_decode_charge(&old.approval)?)
                    .ok_or(Error::Capacity("owner decode"))?;
                snapshot.owner_bytes = snapshot
                    .owner_bytes
                    .checked_add(approval_charge(&old.approval)?)
                    .ok_or(Error::Capacity("owner metadata"))?;
                snapshot.ledger.commands.insert(old.key, old);
            }
        }
        for entry in &self.ledger.transitions {
            if let Some(existing) = snapshot
                .ledger
                .transitions
                .iter()
                .find(|other| other.id == entry.id)
            {
                if existing != entry {
                    return Err(Error::InvalidLedger);
                }
            } else {
                if snapshot.ledger.len() >= crate::orchestration_records::ITEM_LIMIT {
                    return Err(Error::Capacity("approval count"));
                }
                if snapshot
                    .ledger
                    .entries()
                    .any(|other| other.approval.reference == entry.id)
                {
                    return Err(Error::InvalidLedger);
                }
                snapshot.owner_bytes = snapshot
                    .owner_bytes
                    .checked_add(super::transition::transition_charge(&entry.id)?)
                    .ok_or(Error::Capacity("owner metadata"))?;
                snapshot.decode_bound = snapshot
                    .decode_bound
                    .checked_add(super::transition::TRANSITION_DECODE)
                    .ok_or(Error::Capacity("owner decode"))?;
                snapshot.ledger.transitions.push(entry.clone());
            }
        }
        super::transition::validate_entries(
            &snapshot.ledger,
            &snapshot.origins,
            snapshot.core.undo(),
            snapshot.core.redo(),
            &snapshot.terminal_facts,
            &snapshot.folded_approvals,
            snapshot.fold_frontier,
        )?;
        for entry in snapshot.ledger.commands.values_mut() {
            let previous = self.ledger.get(entry.key).map(|old| old.fate);
            entry.fate = if let Some(command) = snapshot.origins.get(&entry.key) {
                if entry.payload_digest != command.origin.payload_digest {
                    return Err(Error::CommandConflict);
                }
                if snapshot
                    .core
                    .command_in_undo(entry.key)
                    .ok_or(Error::InvalidLedger)?
                {
                    Fate::Applied
                } else {
                    Fate::Undone
                }
            } else if snapshot
                .folded_approvals
                .get(&entry.key)
                .is_some_and(|proof| proof.payload_digest == entry.payload_digest)
            {
                Fate::Folded
            } else {
                Fate::Discarded
            };
            if previous != Some(entry.fate) {
                entry.install_epoch = Some(install_epoch);
            }
        }
        if snapshot.decode_bound > snapshot.reservation.role().envelope().workspace {
            return Err(Error::Capacity("owner decode"));
        }
        if snapshot.owner_bytes > snapshot.reservation.role().envelope().owners {
            return Err(Error::Capacity("owner metadata"));
        }
        let position = self.core.install_snapshot(snapshot.core)?;
        self.ledger = snapshot.ledger;
        self.folded_approvals = snapshot.folded_approvals;
        self.seeds = snapshot.seeds;
        self.resources = snapshot.resources;
        self.origins = snapshot.origins;
        self.owner_bytes = snapshot.owner_bytes;
        self.baseline_bytes = snapshot.baseline_bytes;
        self.decode_bound = snapshot.decode_bound;
        self.baseline_decode = snapshot.baseline_decode;
        self.fold_frontier = snapshot.fold_frontier;
        self.terminal_facts = snapshot.terminal_facts;
        self.reservation = snapshot.reservation;
        Ok(position)
    }
}
impl OwnerSnapshot {
    pub fn snapshot_identity(&self) -> Result<[u8; 32], Error> {
        Ok(self.data.history.snapshot_identity()?)
    }
    pub fn validate(self) -> Result<ValidatedOwnerSnapshot, Error> {
        let bounds = self.reservation.role().bounds();
        self.validate_inner(bounds)
    }
    #[cfg(test)]
    pub(super) fn validate_bounded(
        self,
        bounds: kinewright_core::undo::RoleBounds,
    ) -> Result<ValidatedOwnerSnapshot, Error> {
        self.validate_inner(bounds)
    }
    #[expect(
        clippy::too_many_lines,
        reason = "one streaming replay validates origins, ledger, resources and lineage together"
    )]
    fn validate_inner(
        self,
        bounds: kinewright_core::undo::RoleBounds,
    ) -> Result<ValidatedOwnerSnapshot, Error> {
        let Self { data, reservation } = self;
        let _workspace = reservation.workspace()?;
        let SnapshotData {
            role,
            fold_frontier,
            terminal_facts,
            history,
            seeds,
            origins,
            resources: recorded_resources,
            mut ledger,
            folded_approvals,
        } = data;
        if role != reservation.role() {
            return Err(Error::Reservation);
        }
        if history.policy != role.policy() {
            return Err(Error::InvalidPolicy);
        }
        super::owner::scan();
        seeds.verify(&history.baseline)?;
        super::transition::validate_frontier(
            fold_frontier,
            history.epoch,
            history.position,
            &history.undo,
            &history.redo,
        )?;
        super::owner::scan();
        let baseline_resources = Resources::document(&history.baseline);
        if baseline_resources
            .iter()
            .any(|r| !recorded_resources.contains(r))
        {
            return Err(Error::MissingResource);
        }
        let mut resources = Resources {
            baseline: baseline_resources,
            counts: BTreeMap::default(),
        };
        let baseline_bytes = baseline_charge(&seeds, &resources.baseline)?;
        let mut owner_bytes = baseline_bytes;
        let baseline_decode = super::stored::baseline_decode_charge(
            &seeds,
            resources.baseline.len(),
            role.envelope().workspace,
        )?;
        let key_charges = history
            .undo
            .iter()
            .chain(&history.redo)
            .map(|step| {
                let first = step.commands().first().ok_or(Error::History(
                    kinewright_core::undo::Error::InvalidMetadata,
                ))?;
                Ok((
                    first.id,
                    super::stored::key_decode_charge(
                        step.coalesce_key(),
                        role.envelope().workspace,
                    )?,
                ))
            })
            .collect::<Result<BTreeMap<_, _>, Error>>()?;
        let mut decode_bound = baseline_decode;
        for entry in ledger.commands.values_mut() {
            entry
                .approval
                .reference
                .validate()
                .map_err(|_| Error::InvalidLedger)?;
            if entry.approval.targets.is_empty()
                || entry.approval.targets.windows(2).any(|w| w[0] >= w[1])
                || entry
                    .install_epoch
                    .is_some_and(|epoch| epoch > history.epoch)
            {
                return Err(Error::InvalidLedger);
            }
            if entry.approval.targets.is_empty()
                || entry.key
                    >= (CommandKey {
                        epoch: history.epoch,
                        position: history.position,
                    })
            {
                return Err(Error::InvalidLedger);
            }
            decode_bound = decode_bound
                .checked_add(super::stored::approval_decode_charge(&entry.approval)?)
                .ok_or(Error::Capacity("owner decode"))?;
            owner_bytes = owner_bytes
                .checked_add(approval_charge(&entry.approval)?)
                .ok_or(Error::Capacity("owner metadata"))?;
        }
        for entry in &ledger.transitions {
            let next = CommandKey {
                epoch: history.epoch,
                position: history.position,
            };
            if entry.target.first_key >= next
                || entry.target.first_key.position >= history.position
                || entry
                    .target
                    .first_key
                    .position
                    .checked_add(entry.target.commands)
                    .is_none_or(|end| end > history.position)
                || entry.target.commands
                    > role.bounds().decoded_history_bytes
                        / (2 * std::mem::size_of::<kinewright_core::undo::StepCommand>() as u64)
            {
                return Err(Error::InvalidLedger);
            }
            entry.id.validate().map_err(|_| Error::InvalidLedger)?;
            owner_bytes = owner_bytes
                .checked_add(super::transition::transition_charge(&entry.id)?)
                .ok_or(Error::Capacity("owner metadata"))?;
            decode_bound = decode_bound
                .checked_add(super::transition::TRANSITION_DECODE)
                .ok_or(Error::Capacity("owner decode"))?;
        }
        if owner_bytes > role.envelope().owners {
            return Err(Error::Capacity("owner metadata"));
        }
        // Working occurrences follow precisely the same semantic replay as the
        // document. Only the baseline map is retained after opening.
        let mut lineage = Lineage {
            occurrences: seeds.occurrences.clone(),
            split_ancestry: Vec::new(),
        };
        // Keep the selected current boundary while Core consumes the record
        // and streams Undo followed by Redo; the document itself is not copied.
        let expected_document = Arc::clone(&history.document);
        let first = history
            .undo
            .iter()
            .chain(history.redo.iter().rev())
            .flat_map(kinewright_core::undo::Step::commands)
            .next()
            .map(|command| command.id);
        if ledger.len() > crate::orchestration_records::ITEM_LIMIT
            || folded_approvals.iter().any(|(key, proof)| {
                first.is_some_and(|first| *key >= first)
                    || ledger.get(*key).is_none_or(|entry| {
                        !proof.valid_for(*key, entry.payload_digest)
                            || entry.fate != Fate::Folded
                            || fold_frontier.is_none_or(|frontier| {
                                *key >= frontier || key.position >= frontier.position
                            })
                    })
            })
            || ledger.entries().any(|entry| {
                entry.fate == Fate::Folded
                    && folded_approvals
                        .get(&entry.key)
                        .is_none_or(|proof| proof.payload_digest != entry.payload_digest)
            })
        {
            return Err(Error::InvalidLedger);
        }
        let history_position = history.position;
        let next = CommandKey {
            epoch: history.epoch,
            position: history.position,
        };
        let mut raw_origins: VecDeque<_> = origins.into();
        let mut step_firsts: VecDeque<_> = history
            .undo
            .iter()
            .chain(history.redo.iter().rev())
            .flat_map(|step| {
                std::iter::repeat_n(
                    step.commands().first().map(|command| command.id),
                    step.commands().len(),
                )
            })
            .collect();
        let mut live_origins = Origins::default();
        let mut seen_approvals = BTreeSet::new();
        let mut entered_redo = false;
        let mut owner_error = None;
        let validated = history.validate_observed(
            bounds,
            role.envelope().owners * 2,
            |command, effects, document, is_redo| {
                let outcome = (|| {
                    if is_redo && !entered_redo {
                        super::owner::scan();
                        lineage.verify(&expected_document)?;
                        entered_redo = true;
                    }
                    let origin = raw_origins.pop_front().ok_or(Error::InvalidOrigin)?;
                    if origin.key != command.id || origin.payload_digest != command.payload_digest {
                        return Err(Error::InvalidOrigin);
                    }
                    for reference in &origin.inputs {
                        reference.validate().map_err(|_| Error::InvalidOrigin)?;
                    }
                    let payload = Resources::payload(&command.operations);
                    if payload.iter().any(|r| !recorded_resources.contains(r)) {
                        return Err(Error::MissingResource);
                    }
                    let charge = origin_charge(&origin, &payload)?;
                    let decode_charge = super::stored::fragment_charge(
                        &command.operations,
                        &origin.creation,
                        &origin.inputs,
                        role.envelope().workspace,
                    )?
                    .checked_add(key_charges.get(&command.id).copied().unwrap_or(0))
                    .and_then(|n| n.checked_add(payload.len() as u64 * 4096))
                    .ok_or(Error::Capacity("owner decode"))?;
                    decode_bound = decode_bound
                        .checked_add(decode_charge)
                        .ok_or(Error::Capacity("owner decode"))?;
                    if decode_bound > role.envelope().workspace {
                        return Err(Error::Capacity("owner decode"));
                    }
                    owner_bytes = owner_bytes
                        .checked_add(charge)
                        .ok_or(Error::Capacity("owner metadata"))?;
                    if owner_bytes > role.envelope().owners {
                        return Err(Error::Capacity("owner metadata"));
                    }
                    if let Some(entry) = ledger.commands.get_mut(&command.id) {
                        if entry.payload_digest != command.payload_digest
                            || !super::ledger::targets_removed(
                                &entry.approval,
                                &command.operations,
                                |id| effects.iter().any(|effect| effect.deleted.contains(&id)),
                            )
                            || (is_redo && entry.fate != Fate::Undone)
                            || (!is_redo && !matches!(entry.fate, Fate::Applied | Fate::Redone))
                        {
                            return Err(Error::InvalidLedger);
                        }
                        seen_approvals.insert(entry.key);
                    }
                    resources.add(&payload);
                    lineage.apply_occurrences(&origin.creation, effects)?;
                    live_origins.push(LiveOrigin {
                        origin,
                        resources: payload,
                        charge,
                        decode_charge,
                        step_first: step_firsts
                            .pop_front()
                            .flatten()
                            .ok_or(Error::InvalidOrigin)?,
                    });
                    let _ = document;
                    Ok(())
                })();
                if let Err(error) = outcome {
                    owner_error = Some(error);
                    return Err(kinewright_core::undo::Error::InvalidMetadata);
                }
                Ok(())
            },
        );
        if let Some(error) = owner_error {
            return Err(error);
        }
        let validated = validated?;
        if !entered_redo {
            super::owner::scan();
            lineage.verify(&expected_document)?;
        }
        if !raw_origins.is_empty() {
            return Err(Error::InvalidOrigin);
        }
        for entry in ledger.entries() {
            if entry.key >= next
                || entry.key.position >= history_position
                || (matches!(entry.fate, Fate::Applied | Fate::Redone | Fate::Undone)
                    && !seen_approvals.contains(&entry.key))
            {
                return Err(Error::InvalidLedger);
            }
        }
        super::transition::validate_entries(
            &ledger,
            &live_origins,
            validated.undo(),
            validated.redo(),
            &terminal_facts,
            &folded_approvals,
            fold_frontier,
        )?;
        Ok(ValidatedOwnerSnapshot {
            core: validated,
            folded_approvals,
            ledger,
            seeds,
            resources,
            origins: live_origins,
            owner_bytes,
            baseline_bytes,
            baseline_decode,
            fold_frontier,
            terminal_facts,
            decode_bound,
            reservation,
        })
    }
}
