use super::{
    Approval, ApprovalLedger, CommandOrigin, CreationOrigin, Error, HistoryPool,
    HistoryReservation, Lineage, RemovedTarget, Resource, Resources, Role,
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
}
thread_local! { static COUNTERS:Cell<OwnerCounters>=const { Cell::new(OwnerCounters { document_scans:0,exact_measurements:0,payload_operations:0,payload_bytes_serialised:0 }) }; }
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
        }
    }
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
    pub fn get(&self, key: &CommandKey) -> Option<&LiveOrigin> {
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
    pub const fn lineage(&self) -> &Lineage {
        &self.lineage
    }
}
struct Plan {
    origin: LiveOrigin,
    approval: Option<Approval>,
    seeds: Option<Lineage>,
    baseline_resources: Option<BTreeSet<Resource>>,
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
    created_tracks: Vec<kinewright_core::TrackId>,
    discarded: Vec<CommandKey>,
}
impl OwnerHistory {
    pub fn new(
        pool: &HistoryPool,
        role: Role,
        policy: RetentionPolicy,
        seed: &CreationOrigin,
        make_document: impl FnOnce() -> Result<Document, Error>,
    ) -> Result<Self, Error> {
        Self::with_bounds(pool, role, policy, seed, role.bounds(), make_document)
    }
    /// Measurement seam: limits may only be lowered, never raised past a role.
    #[cfg(any(test, feature = "history-metrics"))]
    pub fn measured(
        pool: &HistoryPool,
        role: Role,
        policy: RetentionPolicy,
        seed: &CreationOrigin,
        bounds: kinewright_core::undo::RoleBounds,
        make_document: impl FnOnce() -> Result<Document, Error>,
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
        Self::with_bounds(pool, role, policy, seed, bounds, make_document)
    }
    fn with_bounds(
        pool: &HistoryPool,
        role: Role,
        policy: RetentionPolicy,
        seed: &CreationOrigin,
        bounds: kinewright_core::undo::RoleBounds,
        make_document: impl FnOnce() -> Result<Document, Error>,
    ) -> Result<Self, Error> {
        if policy != role.policy() {
            return Err(Error::InvalidPolicy);
        }
        // Reserve the entire retained role envelope before the factory may
        // allocate a document. Growth stays inside that admitted envelope.
        let reservation = pool.admit(role)?;
        let _workspace = reservation.workspace()?;
        let core = History::new(make_document()?, policy, bounds)?;
        let document = core.baseline();
        scan();
        let seeds = Lineage::seed(document, seed);
        scan();
        let resources = Resources::new(document);
        let baseline_bytes = baseline_charge(&seeds, &resources.baseline)?;
        if baseline_bytes > role.envelope().owners {
            return Err(Error::Capacity("owner metadata"));
        }
        let baseline_decode = super::stored::baseline_decode_charge(
            &seeds,
            resources.baseline.len(),
            role.envelope().workspace,
        )?;
        if baseline_decode > role.envelope().workspace {
            return Err(Error::Capacity("owner decode"));
        }
        Ok(Self {
            core,
            ledger: ApprovalLedger::default(),
            folded_approvals: std::collections::BTreeMap::default(),
            seeds,
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
    #[must_use]
    pub fn history(&self) -> &History {
        &self.core
    }
    #[must_use]
    pub fn ledger(&self) -> &ApprovalLedger {
        &self.ledger
    }
    #[must_use]
    pub fn resources(&self) -> &Resources {
        &self.resources
    }
    #[must_use]
    pub fn seeds(&self) -> &Lineage {
        &self.seeds
    }
    #[must_use]
    pub const fn owner_bytes(&self) -> u64 {
        self.owner_bytes
    }
    #[allow(
        clippy::too_many_lines,
        reason = "one preparation/commit transaction keeps Core and owner refusal atomic"
    )]
    pub fn execute(
        &mut self,
        command: Command,
        creation: CreationOrigin,
        inputs: Vec<crate::bundle_records::ObjectRef>,
        approval: Option<Approval>,
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
            self.ledger.observe(&effects, &[]);
            return Ok(effects);
        }
        let open_key = match &command {
            Command::DoBatchCoalesced { coalesce_key, .. } => Some(coalesce_key.as_str()),
            _ => None,
        };
        let new_key_decode = super::stored::key_decode_charge(
            open_key,
            self.reservation.role().envelope().workspace,
        );
        let wire_charge = super::stored::command_decode_charge(
            &command,
            &creation,
            &inputs,
            self.reservation.role().envelope().workspace,
        );
        let Self {
            core,
            ledger,
            folded_approvals,
            seeds,
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
        let (effects, plan) =
            core.execute_prepared(command, reservation.role().envelope().owners * 2, |event| {
                prepare(
                    &event,
                    creation,
                    inputs,
                    approval,
                    ledger,
                    seeds,
                    origins,
                    *owner_bytes,
                    *baseline_bytes,
                    reservation.role(),
                    wire_charge,
                    *decode_bound,
                    *baseline_decode,
                    new_key_decode,
                    *fold_frontier,
                )
            })?;
        if let Some(approval) = plan.approval {
            ledger.insert(
                plan.origin.origin.key,
                plan.origin.origin.payload_digest,
                approval,
            );
        }
        // Each retained command contributes at most one reference per resource.
        // The bounded origin map makes count overflow unreachable.
        for key in &plan.discarded {
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
        if let Some(seeds_next) = plan.seeds {
            *seeds = seeds_next;
        }
        if let Some(baseline) = plan.baseline_resources {
            resources.baseline = baseline;
        }
        *owner_bytes = plan.owner_bytes;
        *baseline_bytes = plan.baseline_bytes;
        *decode_bound = plan.decode_bound;
        *baseline_decode = plan.baseline_decode;
        *fold_frontier = plan.fold_frontier;
        for (target, presence) in plan.terminal_updates {
            terminal_facts.insert(target, presence);
        }
        ledger.observe(&effects, &plan.created_tracks);
        for (key, proof) in plan.folded_approval_updates {
            folded_approvals.insert(key, proof);
        }
        Ok(effects)
    }
    pub fn close_gesture(&mut self) -> Result<Effects, Error> {
        let effects = self.core.close_gesture()?;
        self.ledger.observe(&effects, &[]);
        Ok(effects)
    }
    pub fn boundary(&self, depth: usize) -> Result<OwnerBoundary, Error> {
        let reservation = self.reservation.capture()?;
        let _workspace = self.reservation.workspace()?;
        // The query owns an independent occurrence map while the baseline
        // remains immutable. Only the returned boundary survives this replay.
        let mut lineage = self.seeds.clone();
        let document = self.core.visit_boundary(
            depth,
            self.reservation.role().envelope().owners * 2,
            |command, effects| {
                let origin = self.origins.get(&command.id).ok_or(Error::InvalidOrigin)?;
                if origin.origin.payload_digest != command.payload_digest {
                    return Err(Error::InvalidOrigin);
                }
                lineage.apply_bounded(
                    &origin.origin.creation,
                    effects,
                    self.reservation.role().envelope().owners,
                )
            },
        )?;
        lineage.prune();
        scan();
        lineage.verify(&document)?;
        Ok(OwnerBoundary {
            document,
            lineage,
            _reservation: reservation,
        })
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
    origins: &Origins,
    mut owner_bytes: u64,
    old_baseline: u64,
    role: Role,
    wire_charge: Result<u64, Error>,
    mut decode_bound: u64,
    old_baseline_decode: u64,
    new_key_decode: Result<u64, Error>,
    mut fold_frontier: Option<CommandKey>,
) -> Result<Plan, Error> {
    let key = event.effects.accepted;
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
        let released = if removed == key {
            charge
        } else {
            origins.get(&removed).ok_or(Error::InvalidOrigin)?.charge
        };
        let released_decode = if removed == key {
            decode_charge
        } else {
            origins
                .get(&removed)
                .ok_or(Error::InvalidOrigin)?
                .decode_charge
        };
        decode_bound = decode_bound
            .checked_sub(released_decode)
            .ok_or(Error::InvalidOrigin)?;
        owner_bytes = owner_bytes
            .checked_sub(released)
            .ok_or(Error::InvalidOrigin)?;
    }
    let (seeds_next, baseline_resources, baseline_bytes, baseline_decode) =
        if let Some(document) = event.folded_baseline {
            // A fold already walks and installs this long-lived baseline. Metadata
            // is staged here so refusal cannot partially advance its seeds.
            let mut next = seeds.clone();
            for command in &event.effects.folded {
                let creation = if command.key == key {
                    &origin.creation
                } else {
                    &origins
                        .get(&command.key)
                        .ok_or(Error::InvalidOrigin)?
                        .origin
                        .creation
                };
                next.apply_bounded(creation, &command.identities, role.envelope().owners)?;
            }
            next.prune();
            scan();
            next.verify(document)?;
            scan();
            let resources = Resources::document(document);
            let bytes = baseline_charge(&next, &resources)?;
            owner_bytes = owner_bytes
                .checked_sub(old_baseline)
                .and_then(|n| n.checked_add(bytes))
                .ok_or(Error::Capacity("owner metadata"))?;
            let decode = super::stored::baseline_decode_charge(
                &next,
                resources.len(),
                role.envelope().workspace,
            )?;
            decode_bound = decode_bound
                .checked_sub(old_baseline_decode)
                .and_then(|v| v.checked_add(decode))
                .ok_or(Error::Capacity("owner decode"))?;
            (Some(next), Some(resources), bytes, decode)
        } else {
            (None, None, old_baseline, old_baseline_decode)
        };
    if decode_bound > role.envelope().workspace {
        return Err(Error::Capacity("owner decode"));
    }
    if owner_bytes > role.envelope().owners {
        return Err(Error::Capacity("owner metadata"));
    }
    let mut folded_approval_updates = Vec::new();
    for folded in &event.effects.folded {
        let digest = if folded.key == key && approval.is_some() {
            Some(event.payload_digest)
        } else {
            ledger.get(folded.key).map(|entry| entry.payload_digest)
        };
        if let Some(payload_digest) = digest {
            let root = if folded.key == key {
                event.step_first
            } else {
                origins
                    .get(&folded.key)
                    .ok_or(Error::InvalidOrigin)?
                    .step_first
            };
            let start = origins
                .0
                .binary_search_by_key(&root.position, |row| row.origin.key.position)
                .unwrap_or_else(|index| index);
            let prefix =
                kinewright_core::undo::StepEvidence::from_commands(
                    origins
                        .0
                        .iter()
                        .skip(start)
                        .take_while(|row| row.step_first == root && row.origin.key <= folded.key)
                        .map(|row| (row.origin.key, row.origin.payload_digest))
                        .chain(std::iter::once((key, event.payload_digest)).filter(
                            |(current, _)| event.step_first == root && *current <= folded.key,
                        )),
                )
                .ok_or(Error::InvalidOrigin)?;
            folded_approval_updates.push((
                folded.key,
                super::ledger::FoldedApproval {
                    payload_digest,
                    prefix,
                },
            ));
        }
    }
    let mut terminal_updates = Vec::new();
    if !event.effects.folded.is_empty() || !event.effects.discarded.is_empty() {
        for entry in ledger.transition_entries() {
            let key = entry.target.first_key;
            let folded = event
                .effects
                .folded
                .iter()
                .any(|command| command.key == key);
            let discarded = event.effects.discarded.contains(&key);
            if folded || discarded {
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
    let created_tracks = event
        .operations
        .iter()
        .filter_map(|operation| match operation {
            Operation::AddTrack { track } => Some(track.id),
            _ => None,
        })
        .collect();
    // Own only the discarded keys so removal visits queue ends, including
    // multiple commands in one coalesced Redo step.
    let mut discarded = event.effects.discarded.clone();
    discarded.sort_unstable_by_key(|key| std::cmp::Reverse(key.position));
    Ok(Plan {
        origin: LiveOrigin {
            origin,
            resources: payload,
            charge,
            decode_charge,
            step_first: event.step_first,
        },
        approval,
        seeds: seeds_next,
        baseline_resources,
        owner_bytes,
        baseline_bytes,
        decode_bound,
        baseline_decode,
        fold_frontier,
        terminal_updates,
        folded_approval_updates,
        created_tracks,
        discarded,
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
        .checked_add(
            (origin.inputs.capacity() * std::mem::size_of::<crate::bundle_records::ObjectRef>())
                as u64,
        )
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
    // Baseline's fixed headers cover the two initially sparse BTree roots.
    768_u64
        .checked_add((approval.targets.capacity() * std::mem::size_of::<RemovedTarget>()) as u64)
        .and_then(|n| n.checked_add(approval.reference.sha256.capacity() as u64))
        .ok_or(Error::Capacity("approval size"))
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
