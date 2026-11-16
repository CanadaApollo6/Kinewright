use super::{
    Approval, ApprovalLedger, CommandOrigin, CreationOrigin, Error, HistoryPool,
    HistoryReservation, Lineage, Resource, Resources, Role,
};
use kinewright_core::{
    Command, Document, Operation,
    undo::{CommandKey, EditEvent, Effects, History, RetentionPolicy},
};
use std::{
    cell::Cell,
    collections::{BTreeSet, VecDeque},
    sync::Arc,
};

/// Owner document traversals and aggregate metadata measurements. Encoding
/// the new command's own payload is counted separately; it is never a document
/// measurement. The existing kernel passes are counted by Core, unchanged.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct OwnerCounters {
    pub document_scans: u64,
    pub exact_measurements: u64,
    pub payload_operations: u64,
    pub payload_bytes_serialised: u64,
    pub owner_identity_hashes: u64,
    pub owner_identity_bytes_serialised: u64,
    pub seed_map_copies: u64,
    pub seed_capacity_scans: u64,
    pub seed_prune_scans: u64,
    pub seed_verify_scans: u64,
    pub resource_document_scans: u64,
    pub lineage_encodings: u64,
    pub payload_encodings: u64,
    pub folded_commands: u64,
    pub origin_lookups: u64,
    pub ledger_entries: u64,
    pub folded_seed_ids: u64,
    pub folded_ancestry: u64,
    pub folded_resource_ids: u64,
    pub folded_payload_references: u64,
}
thread_local! { static COUNTERS:Cell<OwnerCounters>=const { Cell::new(OwnerCounters { document_scans:0,exact_measurements:0,payload_operations:0,payload_bytes_serialised:0,owner_identity_hashes:0,owner_identity_bytes_serialised:0,seed_map_copies:0,seed_capacity_scans:0,seed_prune_scans:0,seed_verify_scans:0,resource_document_scans:0,lineage_encodings:0,payload_encodings:0,folded_commands:0,origin_lookups:0,ledger_entries:0,folded_seed_ids:0,folded_ancestry:0,folded_resource_ids:0,folded_payload_references:0 }) }; }
impl OwnerCounters {
    #[must_use]
    pub fn read() -> Self {
        COUNTERS.get()
    }
    #[must_use]
    pub const fn since(self, old: Self) -> Self {
        Self {
            document_scans: self.document_scans - old.document_scans,
            exact_measurements: self.exact_measurements - old.exact_measurements,
            payload_operations: self.payload_operations - old.payload_operations,
            payload_bytes_serialised: self.payload_bytes_serialised - old.payload_bytes_serialised,
            owner_identity_hashes: self.owner_identity_hashes - old.owner_identity_hashes,
            owner_identity_bytes_serialised: self.owner_identity_bytes_serialised
                - old.owner_identity_bytes_serialised,
            seed_map_copies: self.seed_map_copies - old.seed_map_copies,
            seed_capacity_scans: self.seed_capacity_scans - old.seed_capacity_scans,
            seed_prune_scans: self.seed_prune_scans - old.seed_prune_scans,
            seed_verify_scans: self.seed_verify_scans - old.seed_verify_scans,
            resource_document_scans: self.resource_document_scans - old.resource_document_scans,
            lineage_encodings: self.lineage_encodings - old.lineage_encodings,
            payload_encodings: self.payload_encodings - old.payload_encodings,
            folded_commands: self.folded_commands - old.folded_commands,
            origin_lookups: self.origin_lookups - old.origin_lookups,
            ledger_entries: self.ledger_entries - old.ledger_entries,
            folded_seed_ids: self.folded_seed_ids - old.folded_seed_ids,
            folded_ancestry: self.folded_ancestry - old.folded_ancestry,
            folded_resource_ids: self.folded_resource_ids - old.folded_resource_ids,
            folded_payload_references: self.folded_payload_references
                - old.folded_payload_references,
        }
    }
}
pub(super) fn seed_map_copies(amount: u64) {
    let mut c = COUNTERS.get();
    c.seed_map_copies += amount;
    COUNTERS.set(c);
}
pub(super) fn seed_capacity_scans(amount: u64) {
    let mut c = COUNTERS.get();
    c.seed_capacity_scans += amount;
    COUNTERS.set(c);
}
pub(super) fn seed_prune_scans(amount: u64) {
    let mut c = COUNTERS.get();
    c.seed_prune_scans += amount;
    COUNTERS.set(c);
}
pub(super) fn seed_verify_scans(amount: u64) {
    let mut c = COUNTERS.get();
    c.seed_verify_scans += amount;
    COUNTERS.set(c);
}
pub(super) fn resource_document_scans(amount: u64) {
    let mut c = COUNTERS.get();
    c.resource_document_scans += amount;
    COUNTERS.set(c);
}
pub(super) fn lineage_encodings(amount: u64) {
    let mut c = COUNTERS.get();
    c.lineage_encodings += amount;
    COUNTERS.set(c);
}
pub(super) fn payload_encodings(amount: u64) {
    let mut c = COUNTERS.get();
    c.payload_encodings += amount;
    COUNTERS.set(c);
}
pub(super) fn folded_commands(amount: u64) {
    let mut c = COUNTERS.get();
    c.folded_commands += amount;
    COUNTERS.set(c);
}
pub(super) fn origin_lookups(amount: u64) {
    let mut c = COUNTERS.get();
    c.origin_lookups += amount;
    COUNTERS.set(c);
}
pub(super) fn ledger_entries(amount: u64) {
    let mut c = COUNTERS.get();
    c.ledger_entries += amount;
    COUNTERS.set(c);
}
pub(super) fn folded_seed_ids(amount: u64) {
    let mut c = COUNTERS.get();
    c.folded_seed_ids += amount;
    COUNTERS.set(c);
}
pub(super) fn folded_ancestry(amount: u64) {
    let mut c = COUNTERS.get();
    c.folded_ancestry += amount;
    COUNTERS.set(c);
}
pub(super) fn folded_resource_ids(amount: u64) {
    let mut c = COUNTERS.get();
    c.folded_resource_ids += amount;
    COUNTERS.set(c);
}
pub(super) fn folded_payload_references(amount: u64) {
    let mut c = COUNTERS.get();
    c.folded_payload_references += amount;
    COUNTERS.set(c);
}

pub(super) fn identity_started() {
    let mut c = COUNTERS.get();
    c.owner_identity_hashes += 1;
    COUNTERS.set(c);
}
pub(super) fn identity_serialized(bytes: u64) {
    let mut c = COUNTERS.get();
    c.owner_identity_bytes_serialised += bytes;
    COUNTERS.set(c);
}
pub(super) fn payload_serialized(bytes: u64) {
    let mut c = COUNTERS.get();
    c.payload_bytes_serialised += bytes;
    COUNTERS.set(c);
}
pub(super) fn scan() {
    let mut c = COUNTERS.get();
    c.document_scans += 1;
    COUNTERS.set(c);
}
pub(super) fn exact_size(value: &impl serde::Serialize) -> Result<u64, Error> {
    struct Counter(u64);
    impl std::io::Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0 = self
                .0
                .checked_add(bytes.len() as u64)
                .ok_or_else(|| std::io::Error::other("size overflow"))?;
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut c = COUNTERS.get();
    c.exact_measurements += 1;
    COUNTERS.set(c);
    let mut counter = Counter(0);
    serde_json::to_writer(&mut counter, value).map_err(|_| Error::InvalidRecord)?;
    Ok(counter.0)
}
#[derive(Debug)]
pub(super) struct LiveOrigin {
    pub origin: CommandOrigin,
    pub resources: BTreeSet<Resource>,
    pub charge: u64,
    pub decode_charge: u64,
    pub step_first: CommandKey,
}
#[derive(Debug, Default)]
pub(super) struct Origins(pub VecDeque<LiveOrigin>);
impl Origins {
    fn spare_bytes(capacity: usize, len: usize) -> Result<u64, Error> {
        // Each live origin already pays for two inline slots. Charge the
        // retained array above that allowance after a large fold/discard.
        (capacity as u64)
            .saturating_sub((len as u64).saturating_mul(2))
            .checked_mul(std::mem::size_of::<LiveOrigin>() as u64)
            .ok_or(Error::Capacity("owner metadata"))
    }
    fn planned_spare(&self, effects: &Effects) -> Result<(u64, u64), Error> {
        let before_fold = self
            .0
            .len()
            .checked_sub(effects.discarded.len())
            .and_then(|n| n.checked_add(1))
            .ok_or(Error::InvalidOrigin)?;
        let after_fold = before_fold
            .checked_sub(effects.folded.len())
            .ok_or(Error::InvalidOrigin)?;
        let capacity = if after_fold == 0 {
            0
        } else if before_fold > self.0.capacity() {
            // VecDeque's amortized growth doubles; this non-byte element
            // has a minimum allocation of four slots. No compaction copy.
            self.0
                .capacity()
                .checked_mul(2)
                .ok_or(Error::Capacity("owner metadata"))?
                .max(before_fold)
                .max(4)
        } else {
            self.0.capacity()
        };
        Ok((
            Self::spare_bytes(self.0.capacity(), self.0.len())?,
            Self::spare_bytes(capacity, after_fold)?,
        ))
    }
    pub fn get(&self, key: &CommandKey) -> Option<&LiveOrigin> {
        origin_lookups(1);
        self.0
            .binary_search_by_key(&key.position, |v| v.origin.key.position)
            .ok()
            .and_then(|index| self.0.get(index))
            .filter(|v| v.origin.key == *key)
    }
    pub fn push(&mut self, origin: LiveOrigin) {
        self.0.push_back(origin);
    }
    pub fn remove(&mut self, key: &CommandKey) -> Option<LiveOrigin> {
        origin_lookups(1);
        let index = self
            .0
            .binary_search_by_key(&key.position, |v| v.origin.key.position)
            .ok()?;
        if self.0[index].origin.key != *key {
            return None;
        }
        self.0.remove(index)
    }
}
#[derive(Debug)]
pub struct OwnerHistory {
    pub(super) core: History,
    pub(super) ledger: ApprovalLedger,
    pub(super) folded_approvals: super::ledger::FoldedApprovals,
    pub(super) seeds: Lineage,
    pub(super) fold: Option<super::fold::FoldState>,
    pub(super) resources: Resources,
    pub(super) origins: Origins,
    pub(super) owner_bytes: u64,
    pub(super) baseline_bytes: u64,
    pub(super) decode_bound: u64,
    pub(super) baseline_decode: u64,
    pub(super) fold_frontier: Option<CommandKey>,
    pub(super) terminal_facts: super::transition::TerminalFacts,
    pub(super) reservation: HistoryReservation,
}
/// A boundary is an independently retained document and occurrence result.
/// Its lease remains live for exactly as long as these public query values.
#[derive(Debug)]
pub struct OwnerBoundary {
    pub(super) document: Arc<Document>,
    pub(super) lineage: Lineage,
    _reservation: HistoryReservation,
}
impl OwnerBoundary {
    #[must_use]
    pub fn document(&self) -> &Document {
        &self.document
    }
    #[must_use]
    pub fn document_arc(&self) -> Arc<Document> {
        Arc::clone(&self.document)
    }
    #[must_use]
    pub const fn lineage(&self) -> &Lineage {
        &self.lineage
    }
}
struct Plan {
    origin: LiveOrigin,
    approval: Option<Approval>,
    fold: Option<super::fold::FoldPatch>,
    owner_bytes: u64,
    baseline_bytes: u64,
    decode_bound: u64,
    baseline_decode: u64,
    fold_frontier: Option<CommandKey>,
    folded_approval_updates: Vec<(CommandKey, super::ledger::FoldedApproval)>,
    terminal_updates: Vec<(
        kinewright_core::undo::StepEvidence,
        super::transition::TerminalPresence,
    )>,
}
impl OwnerHistory {
    pub(crate) fn promote_live(&mut self) -> Result<(), Error> {
        self.reservation.promote_replacing(None)
    }

    pub fn new<D: Into<Arc<Document>>>(
        pool: &HistoryPool,
        role: Role,
        policy: RetentionPolicy,
        seed: &CreationOrigin,
        make_document: impl FnOnce() -> Result<D, Error>,
    ) -> Result<Self, Error> {
        Self::create(
            pool,
            match role {
                Role::Ordinary => super::CreationKind::Ordinary,
                Role::Task => super::CreationKind::Task,
                Role::Candidate => super::CreationKind::LiveCandidate,
            },
            policy,
            seed,
            None,
            kinewright_core::undo::Charge::default(),
            None,
            make_document,
        )
    }
    /// Measurement seam: limits may only be lowered, never raised past a role.
    #[cfg(any(test, feature = "test-util", feature = "history-metrics"))]
    pub fn measured<D: Into<Arc<Document>>>(
        pool: &HistoryPool,
        role: Role,
        policy: RetentionPolicy,
        seed: &CreationOrigin,
        bounds: kinewright_core::undo::RoleBounds,
        make_document: impl FnOnce() -> Result<D, Error>,
    ) -> Result<Self, Error> {
        let maximum = role.bounds();
        if bounds.document_bytes > maximum.document_bytes
            || bounds.document_live_bytes > maximum.document_live_bytes
            || bounds.operation_bytes > maximum.operation_bytes
            || bounds.decoded_history_bytes > maximum.decoded_history_bytes
            || bounds.step_cap > maximum.step_cap
            || bounds.checkpoint_documents > maximum.checkpoint_documents
            || bounds.replay_workspace > maximum.replay_workspace
        {
            return Err(Error::Capacity("measurement limits"));
        }
        Self::create(
            pool,
            match role {
                Role::Ordinary => super::CreationKind::Ordinary,
                Role::Task => super::CreationKind::Task,
                Role::Candidate => super::CreationKind::LiveCandidate,
            },
            policy,
            seed,
            Some(bounds),
            kinewright_core::undo::Charge::default(),
            None,
            make_document,
        )
    }
    /// A fresh merge destination preserves the source's original sizing even
    /// when a cold decoder has compacted its baseline allocations.
    pub fn live_candidate<D: Into<Arc<Document>>>(
        pool: &HistoryPool,
        seed: &CreationOrigin,
        source_sizing: super::CreationSizing,
        make_document: impl FnOnce() -> Result<D, Error>,
    ) -> Result<Self, Error> {
        if source_sizing.kind == super::CreationKind::Ordinary {
            return Err(Error::InvalidPolicy);
        }
        source_sizing.limits()?;
        Self::create(
            pool,
            super::CreationKind::LiveCandidate,
            RetentionPolicy::NeverFold,
            seed,
            None,
            kinewright_core::undo::Charge::default(),
            Some(source_sizing),
            make_document,
        )
    }
    pub fn staging<D: Into<Arc<Document>>>(
        pool: &HistoryPool,
        seed: &CreationOrigin,
        source_sizing: super::CreationSizing,
        initial_delta: &[Operation],
        make_document: impl FnOnce() -> Result<D, Error>,
    ) -> Result<Self, Error> {
        if source_sizing.kind == super::CreationKind::Ordinary {
            return Err(Error::InvalidPolicy);
        }
        source_sizing.limits()?;
        let charge = kinewright_core::undo::batch_charge(initial_delta)?;
        Self::create(
            pool,
            super::CreationKind::StagingCandidate,
            RetentionPolicy::NeverFold,
            seed,
            None,
            charge,
            Some(source_sizing),
            make_document,
        )
    }
    #[allow(
        clippy::too_many_arguments,
        reason = "one constructor for fixed creation and measurement profiles"
    )]
    fn create<D: Into<Arc<Document>>>(
        pool: &HistoryPool,
        kind: super::CreationKind,
        policy: RetentionPolicy,
        seed: &CreationOrigin,
        measured: Option<kinewright_core::undo::RoleBounds>,
        initial_batch: kinewright_core::undo::Charge,
        source_sizing: Option<super::CreationSizing>,
        make_document: impl FnOnce() -> Result<D, Error>,
    ) -> Result<Self, Error> {
        let role = kind.role();
        if policy != role.policy() {
            return Err(Error::InvalidPolicy);
        }
        // Scratch covers construction before the measured retained lease exists.
        // Admission failure drops these private values without starting a task.
        let workspace = pool.workspace(role)?;
        let document = make_document()?.into();
        scan();
        let seeds = Lineage::seed(&document, seed);
        Self::assemble(
            pool,
            kind,
            policy,
            measured,
            initial_batch,
            source_sizing,
            document,
            seeds,
            &workspace,
            true,
        )
    }
    #[allow(
        clippy::too_many_arguments,
        reason = "one creation-sized constructor with an existing shared workspace proof"
    )]
    fn assemble(
        pool: &HistoryPool,
        kind: super::CreationKind,
        policy: RetentionPolicy,
        measured: Option<kinewright_core::undo::RoleBounds>,
        initial_batch: kinewright_core::undo::Charge,
        source_sizing: Option<super::CreationSizing>,
        document: Arc<Document>,
        seeds: Lineage,
        workspace: &super::Workspace,
        live: bool,
    ) -> Result<Self, Error> {
        let role = kind.role();
        let baseline_live = kinewright_core::undo::document_capacity_bytes(&document)?
            .max(source_sizing.map_or(0, |s| s.baseline_live));
        scan();
        let resources = Resources::new(&document);
        let sizing = super::CreationSizing {
            kind,
            baseline_live,
            baseline_owners: baseline_charge(&seeds, &resources.baseline)?
                .max(source_sizing.map_or(0, |s| s.baseline_owners)),
            initial_batch,
        };
        let reservation = pool.admit_sized(sizing)?;
        let reservation = if live {
            reservation.into_core()?
        } else {
            reservation
        };
        let limits = reservation.limits();
        if !workspace.covers(pool, limits.envelope.workspace) {
            return Err(Error::Reservation);
        }
        let core = History::new(
            document,
            policy,
            measured.unwrap_or(limits.bounds).intersect(limits.bounds),
        )?;
        let core = if role != Role::Ordinary
            && core.bounds().replay_workspace == super::sizing::WORKSPACE_BYTES
        {
            core.with_merge_destination(super::sizing::MERGE_DESTINATION_INPUT)?
        } else {
            core
        };
        let fold = if role == Role::Ordinary {
            Some(super::fold::FoldState::new(
                &seeds,
                core.baseline(),
                limits.envelope.workspace,
            )?)
        } else {
            None
        };
        let (baseline_bytes, baseline_decode) = if let Some(state) = &fold {
            state.costs()?
        } else {
            (
                sizing.baseline_owners,
                super::stored::baseline_decode_charge(
                    &seeds,
                    resources.baseline.len(),
                    limits.envelope.workspace,
                )?,
            )
        };
        if baseline_bytes > limits.envelope.owners {
            return Err(Error::Capacity("owner metadata"));
        }
        if baseline_decode > limits.envelope.workspace.min(super::stored::DECODE_LIMIT) {
            return Err(Error::Capacity("owner decode"));
        }
        Ok(Self {
            core,
            ledger: ApprovalLedger::default(),
            folded_approvals: std::collections::BTreeMap::default(),
            seeds,
            fold,
            resources,
            origins: Origins::default(),
            owner_bytes: baseline_bytes,
            baseline_bytes,
            decode_bound: baseline_decode,
            baseline_decode,
            fold_frontier: None,
            terminal_facts: std::collections::BTreeMap::default(),
            reservation,
        })
    }
    /// A child is a fresh `NeverFold` delta on the parent's current state. Its
    /// original occurrence identities survive; parent history is not imported.
    pub(crate) fn fork_current(&self, role: Role, live: bool) -> Result<Self, Error> {
        let epoch = self
            .core
            .position()
            .epoch
            .checked_add(1)
            .ok_or(kinewright_core::undo::Error::CounterExhausted)?;
        let kind = match role {
            Role::Task => super::CreationKind::Task,
            Role::Candidate => super::CreationKind::LiveCandidate,
            Role::Ordinary => super::CreationKind::Ordinary,
        };
        let workspace = self.reservation.workspace()?;
        let mut seeds = self.seeds();
        let replayed = self.core.visit_boundary(
            self.core.undo().len(),
            self.reservation.limits().envelope.owners * 2,
            |command, effects| {
                let origin = self.origins.get(&command.id).ok_or(Error::InvalidOrigin)?;
                if command.payload_digest != origin.origin.payload_digest {
                    return Err(Error::InvalidOrigin);
                }
                seeds.apply_bounded(&origin.origin.creation, effects, role.envelope().owners)
            },
        )?;
        seeds.prune();
        scan();
        seeds.verify(&replayed)?;
        drop(replayed);
        // Construction runs under the existing scratch window until the full
        // child lease is admitted. There is no extra parent capture or gap in
        // accounting, and no lease is grown or transferred.
        let mut child = Self::assemble(
            &self.reservation.pool(),
            kind,
            role.policy(),
            None,
            kinewright_core::undo::Charge::default(),
            None,
            Arc::clone(self.core.document()),
            seeds,
            &workspace,
            live,
        )?;
        // A fresh delta has no parent command facts, but its baseline was
        // captured after the parent epoch. Resetting this to zero would make
        // an inherited effect look certainly absent under the install ruling.
        child.core.begin_install_epoch_at(epoch)?;
        Ok(child)
    }
    /// Evaluation endpoints opt in before spawning their actor. Product
    /// constructors leave the complete activity recorder disabled.
    pub fn recording(mut self) -> Result<Self, Error> {
        self.core = self.core.recording()?;
        Ok(self)
    }
    pub fn operations(&self, recorded: bool) -> Result<OwnerOperations, Error> {
        let reservation = self.reservation.capture()?;
        let _window = reservation.workspace()?;
        let operations = if recorded {
            // This reader independently owns the complete evaluation activity.
            self.core.op_log().ok_or(Error::InvalidPolicy)?.to_vec()
        } else {
            self.core.applied_delta().cloned().collect()
        };
        Ok(OwnerOperations {
            operations,
            _reservation: reservation,
        })
    }
    pub(crate) fn retain_memory(&self) -> super::LeaseHold {
        self.reservation.retain()
    }
    pub(crate) fn reserve_stream_buffer(&mut self, peak_bytes: u64) -> Result<(), Error> {
        self.core
            .reserve_stream_buffer(peak_bytes)
            .map_err(Error::from)
    }
    pub(crate) fn release_stream_buffer(&mut self) -> Result<(), Error> {
        self.core.release_stream_buffer().map_err(Error::from)
    }
    #[must_use]
    pub fn pool(&self) -> HistoryPool {
        self.reservation.pool()
    }
    #[must_use]
    pub const fn creation_sizing(&self) -> super::CreationSizing {
        self.reservation.sizing()
    }
    #[must_use]
    pub fn history(&self) -> &History {
        &self.core
    }
    #[must_use]
    pub fn ledger(&self) -> &ApprovalLedger {
        &self.ledger
    }
    /// Audit reports the approved command or transition, without retaining a
    /// document or making an independently owned copy of the ledger.
    pub fn approval_answer(
        &self,
        reference: &crate::bundle_records::ObjectRef,
    ) -> Result<Option<super::ApprovalAnswer>, Error> {
        reference.validate().map_err(|_| Error::InvalidLedger)?;
        if let Some(entry) = self.ledger.get_approval(reference) {
            return Ok(Some(entry.answer()));
        }
        Ok(self
            .transition_fate(reference)?
            .map(super::TransitionFate::answer))
    }
    #[must_use]
    pub fn resources(&self) -> &Resources {
        &self.resources
    }
    #[must_use]
    pub fn seeds(&self) -> Lineage {
        self.fold.as_ref().map_or_else(
            || self.seeds.clone(),
            |state| state.materialize(&self.seeds),
        )
    }
    #[must_use]
    pub const fn owner_bytes(&self) -> u64 {
        self.owner_bytes
    }
    #[allow(
        clippy::too_many_lines,
        clippy::missing_panics_doc,
        reason = "one atomic transaction; the private prepared plan guarantees its folding role exists"
    )]
    pub fn execute(
        &mut self,
        command: Command,
        creation: CreationOrigin,
        inputs: Vec<crate::bundle_records::ObjectRef>,
        approval: Option<Approval>,
    ) -> Result<Effects, Error> {
        self.execute_with_inputs(command, creation, inputs, approval, None)
    }
    #[allow(
        clippy::too_many_lines,
        reason = "one atomic metadata plan and its infallible commit"
    )]
    pub(crate) fn execute_with_inputs(
        &mut self,
        command: Command,
        creation: CreationOrigin,
        inputs: Vec<crate::bundle_records::ObjectRef>,
        approval: Option<Approval>,
        certificate: Option<&crate::operation_inputs::ManualInputs>,
    ) -> Result<Effects, Error> {
        // Preserve Core's Empty refusal before any capacity admission, even
        // when a different owner currently occupies the shared workspace.
        if matches!(&command,Command::DoBatch(ops)|Command::DoBatchCoalesced {operations:ops,..} if ops.is_empty())
        {
            return self.core.execute(command).map_err(Error::from);
        }
        let _workspace = self.reservation.workspace()?;
        if matches!(command, Command::Undo | Command::Redo) {
            if approval.is_some() || !inputs.is_empty() {
                return Err(Error::InvalidLedger);
            }
            let effects = self.core.execute(command)?;
            self.ledger.observe(&effects);
            return Ok(effects);
        }
        let open_key = match &command {
            Command::DoBatchCoalesced { coalesce_key, .. } => Some(coalesce_key.as_str()),
            _ => None,
        };
        let new_key_decode = super::stored::key_decode_charge(
            open_key,
            self.reservation.limits().envelope.workspace,
        );
        let wire_charge = super::stored::command_decode_charge(
            &command,
            &creation,
            &inputs,
            self.reservation.limits().envelope.workspace,
        );
        let Self {
            core,
            ledger,
            folded_approvals,
            seeds,
            fold,
            resources,
            origins,
            owner_bytes,
            baseline_bytes,
            decode_bound,
            baseline_decode,
            fold_frontier,
            terminal_facts,
            reservation,
        } = self;
        let (effects, plan) = core.execute_prepared(
            command,
            reservation.limits().envelope.owners * if fold.is_some() { 3 } else { 2 },
            |event| {
                if let Some(certificate) = certificate {
                    certificate.check(event.operations)?;
                }
                prepare(
                    &event,
                    creation,
                    inputs,
                    approval,
                    ledger,
                    seeds,
                    fold.as_ref(),
                    origins,
                    *owner_bytes,
                    *baseline_bytes,
                    reservation.limits().envelope,
                    wire_charge,
                    *decode_bound,
                    *baseline_decode,
                    new_key_decode,
                    *fold_frontier,
                )
            },
        )?;
        if let Some(approval) = plan.approval {
            ledger.insert(
                plan.origin.origin.key,
                plan.origin.origin.payload_digest,
                approval,
            );
        }
        // Each retained command contributes at most one reference per resource.
        // The bounded origin map makes count overflow unreachable.
        for key in &effects.discarded {
            if let Some(origin) = origins.remove(key) {
                resources.remove(&origin.resources);
            }
        }
        resources.add(&plan.origin.resources);
        origins.push(plan.origin);
        for command in &effects.folded {
            if let Some(origin) = origins.remove(&command.key) {
                resources.remove(&origin.resources);
            }
        }
        if origins.0.is_empty() {
            // Releasing an empty buffer copies no retained origin.
            origins.0 = VecDeque::new();
        }
        if let Some(patch) = plan.fold {
            patch.commit(
                fold.as_mut().expect("prepared folding role"),
                seeds,
                resources,
            );
        }
        *owner_bytes = plan.owner_bytes;
        *baseline_bytes = plan.baseline_bytes;
        *decode_bound = plan.decode_bound;
        *baseline_decode = plan.baseline_decode;
        *fold_frontier = plan.fold_frontier;
        for (target, presence) in plan.terminal_updates {
            terminal_facts.insert(target, presence);
        }
        ledger.observe(&effects);
        for (key, proof) in plan.folded_approval_updates {
            folded_approvals.insert(key, proof);
        }
        Ok(effects)
    }
    pub fn close_gesture(&mut self) -> Result<Effects, Error> {
        let effects = self.core.close_gesture()?;
        self.ledger.observe(&effects);
        Ok(effects)
    }
    pub fn boundary(&self, depth: usize) -> Result<OwnerBoundary, Error> {
        let reservation = self.reservation.capture()?;
        let _workspace = self.reservation.workspace()?;
        let (document, lineage) = self.replay_boundary(depth)?;
        Ok(OwnerBoundary {
            document,
            lineage,
            _reservation: reservation,
        })
    }
    pub(super) fn into_boundary(self, depth: usize) -> Result<OwnerBoundary, Error> {
        let _workspace = self.reservation.workspace()?;
        let (document, lineage) = self.replay_boundary(depth)?;
        Ok(OwnerBoundary {
            document,
            lineage,
            _reservation: self.reservation,
        })
    }
    fn replay_boundary(&self, depth: usize) -> Result<(Arc<Document>, Lineage), Error> {
        // The query owns an independent occurrence map while the baseline
        // remains immutable. Only the returned boundary survives this replay.
        let mut lineage = self.seeds();
        // This holder contains a document and lineage, not operation stacks or
        // cache anchors. Its full independent lease covers that query result.
        let lineage_cap = self
            .reservation
            .limits()
            .envelope
            .retained
            .checked_sub(self.core.bounds().document_live_bytes)
            .ok_or(Error::Reservation)?;
        let document = self.core.visit_boundary(
            depth,
            self.reservation.limits().envelope.owners * 2,
            |command, effects| {
                let origin = self.origins.get(&command.id).ok_or(Error::InvalidOrigin)?;
                if origin.origin.payload_digest != command.payload_digest {
                    return Err(Error::InvalidOrigin);
                }
                lineage.apply_bounded(&origin.origin.creation, effects, lineage_cap)
            },
        )?;
        lineage.prune();
        scan();
        lineage.verify(&document)?;
        Ok((document, lineage))
    }
}

#[allow(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "one atomic owner plan covers fates, origins, resources and folded seeds"
)]
fn prepare(
    event: &EditEvent<'_>,
    creation: CreationOrigin,
    inputs: Vec<crate::bundle_records::ObjectRef>,
    mut approval: Option<Approval>,
    ledger: &ApprovalLedger,
    seeds: &Lineage,
    fold: Option<&super::fold::FoldState>,
    origins: &Origins,
    mut owner_bytes: u64,
    old_baseline: u64,
    envelope: super::Envelope,
    wire_charge: Result<u64, Error>,
    mut decode_bound: u64,
    old_baseline_decode: u64,
    new_key_decode: Result<u64, Error>,
    mut fold_frontier: Option<CommandKey>,
) -> Result<Plan, Error> {
    let key = event.effects.accepted;
    folded_commands(event.effects.folded.len() as u64);
    if let Some(approval) = &mut approval {
        approval.targets.sort_unstable();
        if approval.targets.windows(2).any(|w| w[0] == w[1]) {
            return Err(Error::InvalidLedger);
        }
    }
    if let Some(approval) = &approval {
        ledger.admit(approval, key)?;
        if !super::ledger::targets_removed(approval, event.operations, |id| {
            event
                .effects
                .applied
                .iter()
                .flat_map(|command| &command.identities)
                .any(|effect| effect.deleted.contains(&id))
        }) {
            return Err(Error::InvalidLedger);
        }
        decode_bound = decode_bound
            .checked_add(super::stored::approval_decode_charge(approval)?)
            .ok_or(Error::Capacity("owner decode"))?;
        owner_bytes = owner_bytes
            .checked_add(approval_charge(approval)?)
            .ok_or(Error::Capacity("owner metadata"))?;
    }
    for reference in &inputs {
        reference.validate().map_err(|_| Error::InvalidOrigin)?;
    }
    let mut counters = COUNTERS.get();
    counters.payload_operations += event.operations.len() as u64;
    COUNTERS.set(counters);
    let payload = Resources::payload(event.operations);
    let origin = CommandOrigin {
        key,
        payload_digest: event.payload_digest,
        creation,
        inputs,
    };
    let charge = origin_charge(&origin, &payload)?;
    // Reserve each accepted gesture key with its command. A coalesced
    // extension conservatively charges the key again; closing a gesture may
    // leave that bound loose until the owning commands fold/drop or open
    // recomputes it. Older steps can still carry a key in Core's record.
    let decode_charge = wire_charge?
        .checked_add(new_key_decode?)
        .and_then(|n| n.checked_add(payload.len() as u64 * 4096))
        .ok_or(Error::Capacity("owner decode"))?;
    decode_bound = decode_bound
        .checked_add(decode_charge)
        .ok_or(Error::Capacity("owner decode"))?;
    owner_bytes = owner_bytes
        .checked_add(charge)
        .ok_or(Error::Capacity("owner metadata"))?;
    for removed in event
        .effects
        .discarded
        .iter()
        .copied()
        .chain(event.effects.folded.iter().map(|c| c.key))
    {
        let (released, released_decode) = if removed == key {
            (charge, decode_charge)
        } else {
            let prior = origins.get(&removed).ok_or(Error::InvalidOrigin)?;
            (prior.charge, prior.decode_charge)
        };
        decode_bound = decode_bound
            .checked_sub(released_decode)
            .ok_or(Error::InvalidOrigin)?;
        owner_bytes = owner_bytes
            .checked_sub(released)
            .ok_or(Error::InvalidOrigin)?;
    }
    let (old_spare, next_spare) = origins.planned_spare(event.effects)?;
    owner_bytes = owner_bytes
        .checked_sub(old_spare)
        .and_then(|n| n.checked_add(next_spare))
        .ok_or(Error::Capacity("owner metadata"))?;
    let mut folded_approval_updates = Vec::new();
    let (fold_next, baseline_bytes, baseline_decode) = if event.effects.folded.is_empty() {
        (None, old_baseline, old_baseline_decode)
    } else {
        let state = fold.ok_or(Error::InvalidPolicy)?;
        let mut patch = super::fold::FoldPatch::new(state, envelope.owners, envelope.workspace);
        let mut root = None;
        let mut prefix = kinewright_core::undo::StepEvidenceBuilder::new();
        let mut count = 0;
        for (command, effects) in event.folded_commands().zip(&event.effects.folded) {
            if command.key != effects.key {
                return Err(Error::InvalidOrigin);
            }
            let (creation, payload_refs) = if command.key == key {
                (&origin.creation, payload.len())
            } else {
                let prior = origins.get(&command.key).ok_or(Error::InvalidOrigin)?;
                if prior.origin.payload_digest != command.payload_digest {
                    return Err(Error::InvalidOrigin);
                }
                (&prior.origin.creation, prior.resources.len())
            };
            folded_payload_references(payload_refs as u64);
            patch.apply(
                state,
                seeds,
                creation,
                command.operations,
                &effects.identities,
            )?;
            if root != Some(command.step_first) {
                prefix = kinewright_core::undo::StepEvidenceBuilder::new();
                root = Some(command.step_first);
            }
            prefix
                .push(command.key, command.payload_digest)
                .ok_or(Error::InvalidOrigin)?;
            if (command.key == key && approval.is_some())
                || ledger
                    .get_exact(command.key, command.payload_digest)
                    .is_some()
            {
                folded_approval_updates.push((
                    command.key,
                    super::ledger::FoldedApproval {
                        payload_digest: command.payload_digest,
                        prefix: prefix.evidence().ok_or(Error::InvalidOrigin)?,
                    },
                ));
            }
            count += 1;
        }
        if count != event.effects.folded.len() {
            return Err(Error::InvalidOrigin);
        }
        let (bytes, decode) = patch.finish(state, seeds)?;
        owner_bytes = owner_bytes
            .checked_sub(old_baseline)
            .and_then(|n| n.checked_add(bytes))
            .ok_or(Error::Capacity("owner metadata"))?;
        decode_bound = decode_bound
            .checked_sub(old_baseline_decode)
            .and_then(|n| n.checked_add(decode))
            .ok_or(Error::Capacity("owner decode"))?;
        (Some(patch), bytes, decode)
    };
    if decode_bound > envelope.workspace.min(super::stored::DECODE_LIMIT) {
        return Err(Error::Capacity("owner decode"));
    }
    if owner_bytes > envelope.owners {
        return Err(Error::Capacity("owner metadata"));
    }
    let mut terminal_updates = Vec::new();
    if !event.effects.folded.is_empty() || !event.effects.discarded.is_empty() {
        for (key, folded) in event
            .effects
            .folded
            .iter()
            .map(|c| (c.key, true))
            .chain(event.effects.discarded.iter().map(|key| (*key, false)))
        {
            for entry in ledger.for_step(key) {
                let presence = if super::transition::prefix_matches(entry.target, origins) {
                    if folded {
                        super::transition::TerminalPresence::Folded
                    } else {
                        super::transition::TerminalPresence::Discarded
                    }
                } else {
                    super::transition::TerminalPresence::ShapeMismatch
                };
                terminal_updates.push((entry.target, presence));
            }
        }
        if let Some(command) = event.effects.folded.last() {
            let frontier = CommandKey {
                epoch: command.key.epoch,
                position: command.key.position.checked_add(1).ok_or(Error::History(
                    kinewright_core::undo::Error::CounterExhausted,
                ))?,
            };
            fold_frontier = Some(fold_frontier.map_or(frontier, |old| old.max(frontier)));
        }
    }
    Ok(Plan {
        origin: LiveOrigin {
            origin,
            resources: payload,
            charge,
            decode_charge,
            step_first: event.step_first,
        },
        approval,
        fold: fold_next,
        owner_bytes,
        baseline_bytes,
        decode_bound,
        baseline_decode,
        fold_frontier,
        terminal_updates,
        folded_approval_updates,
    })
}
pub(super) fn origin_charge(
    origin: &CommandOrigin,
    resources: &BTreeSet<Resource>,
) -> Result<u64, Error> {
    // Two inline slots cover geometric VecDeque capacity. Opaque IDs may
    // arrive with spare capacity through a typed value, so charge it directly.
    let mut bytes = (2 * std::mem::size_of::<LiveOrigin>()) as u64
        + 48
        + origin.creation.branch.allocated_capacity() as u64
        + origin.creation.batch.allocated_capacity() as u64;
    bytes = bytes
        .checked_add(vector_replay_bytes(&origin.inputs)?)
        .ok_or(Error::Capacity("origin size"))?;
    for reference in &origin.inputs {
        bytes = bytes
            .checked_add(reference.sha256.capacity() as u64 + 128)
            .ok_or(Error::Capacity("origin size"))?;
    }
    bytes
        .checked_add(resources.len() as u64 * 1536)
        .ok_or(Error::Capacity("origin size"))
}
pub(super) fn approval_charge(approval: &Approval) -> Result<u64, Error> {
    // Covers both the live ledger node and its eventual folded-proof node.
    // The key repeats its digest and consent key so sibling approvals remain distinct.
    // Baseline's fixed headers cover the two initially sparse BTree roots.
    1536_u64
        .checked_add(vector_replay_bytes(&approval.targets)?)
        .and_then(|n| n.checked_add(approval.reference.sha256.capacity() as u64))
        .ok_or(Error::Capacity("approval size"))
}
fn vector_replay_bytes<T>(values: &Vec<T>) -> Result<u64, Error> {
    // A one-element vec! can have one slot; a cold JSON sequence starts at
    // Vec's geometric minimum. Reserve both without normalising either copy.
    let replay = if values.is_empty() {
        0
    } else {
        let minimum = if std::mem::size_of::<T>() == 1 {
            8
        } else if std::mem::size_of::<T>() <= 1024 {
            4
        } else {
            1
        };
        values
            .len()
            .checked_next_power_of_two()
            .ok_or(Error::Capacity("owner metadata"))?
            .max(minimum)
    };
    (values.capacity().max(replay) as u64)
        .checked_mul(std::mem::size_of::<T>() as u64)
        .ok_or(Error::Capacity("owner metadata"))
}
pub(super) fn baseline_charge(
    lineage: &Lineage,
    resources: &BTreeSet<Resource>,
) -> Result<u64, Error> {
    let encoded = exact_size(lineage)?
        .checked_mul(4)
        .ok_or(Error::Capacity("baseline metadata"))?;
    encoded
        .max(lineage.capacity_bytes()?)
        .checked_add(resources.len() as u64 * 512 + 4096)
        .ok_or(Error::Capacity("baseline metadata"))
}

/// An independently retained operation reader pays its source's full lease.
pub struct OwnerOperations {
    pub(super) operations: Vec<Operation>,
    pub(super) _reservation: HistoryReservation,
}
impl OwnerOperations {
    #[must_use]
    pub fn operations(&self) -> &[Operation] {
        &self.operations
    }
}
impl std::fmt::Debug for OwnerOperations {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.operations.fmt(f)
    }
}
impl PartialEq for OwnerOperations {
    fn eq(&self, other: &Self) -> bool {
        self.operations == other.operations
    }
}
impl Eq for OwnerOperations {}
impl serde::Serialize for OwnerOperations {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.operations.serialize(serializer)
    }
}
impl std::ops::Deref for OwnerOperations {
    type Target = [Operation];
    fn deref(&self) -> &Self::Target {
        &self.operations
    }
}
