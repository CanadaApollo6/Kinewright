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

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct OwnerCounters {
    pub document_scans: u64,
    pub exact_measurements: u64,
    pub payload_operations: u64,
}
thread_local! { static COUNTERS:Cell<OwnerCounters>=const { Cell::new(OwnerCounters { document_scans:0,exact_measurements:0,payload_operations:0 }) }; }
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
        }
    }
}
fn scan() {
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
    pub(super) seeds: Lineage,
    pub(super) resources: Resources,
    pub(super) origins: Origins,
    pub(super) owner_bytes: u64,
    pub(super) baseline_bytes: u64,
    pub(super) reservation: HistoryReservation,
}
struct Plan {
    origin: LiveOrigin,
    approval: Option<Approval>,
    seeds: Option<Lineage>,
    baseline_resources: Option<BTreeSet<Resource>>,
    owner_bytes: u64,
    baseline_bytes: u64,
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
        // Reserve the entire retained role envelope before the factory may
        // allocate a document. Growth stays inside that admitted envelope.
        let reservation = pool.admit(role)?;
        let _workspace = reservation.workspace()?;
        let core = History::new(make_document()?, policy, role.bounds())?;
        let document = core.baseline();
        scan();
        let seeds = Lineage::seed(document, seed);
        scan();
        let resources = Resources::new(document);
        let baseline_bytes = baseline_charge(&seeds, &resources.baseline)?;
        if baseline_bytes > role.envelope().owners {
            return Err(Error::Capacity("owner metadata"));
        }
        Ok(Self {
            core,
            ledger: ApprovalLedger::default(),
            seeds,
            resources,
            origins: Origins::default(),
            owner_bytes: baseline_bytes,
            baseline_bytes,
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
    pub fn execute(
        &mut self,
        command: Command,
        creation: CreationOrigin,
        inputs: Vec<crate::bundle_records::ObjectRef>,
        approval: Option<Approval>,
    ) -> Result<Effects, Error> {
        let _workspace = self.reservation.workspace()?;
        if matches!(command, Command::Undo | Command::Redo) {
            if approval.is_some() || !inputs.is_empty() {
                return Err(Error::InvalidLedger);
            }
            let effects = self.core.execute(command)?;
            self.ledger.observe(&effects, &[]);
            return Ok(effects);
        }
        let Self {
            core,
            ledger,
            seeds,
            resources,
            origins,
            owner_bytes,
            baseline_bytes,
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
        ledger.observe(&effects, &plan.created_tracks);
        Ok(effects)
    }
    pub fn close_gesture(&mut self) -> Result<Effects, Error> {
        let effects = self.core.close_gesture()?;
        self.ledger.observe(&effects, &[]);
        Ok(effects)
    }
    pub fn boundary(&self, depth: usize) -> Result<(Arc<Document>, Lineage), Error> {
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
                lineage.apply(&origin.origin.creation, effects)
            },
        )?;
        lineage.prune();
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
    approval: Option<Approval>,
    ledger: &ApprovalLedger,
    seeds: &Lineage,
    origins: &Origins,
    mut owner_bytes: u64,
    old_baseline: u64,
    role: Role,
) -> Result<Plan, Error> {
    let key = event.effects.accepted;
    if let Some(approval) = &approval {
        ledger.admit(approval, key)?;
        if approval.targets.iter().any(|target| match target {
            RemovedTarget::Clip(id) => !event.effects.applied.iter().any(|command| {
                command
                    .identities
                    .iter()
                    .any(|effect| effect.deleted.contains(id))
            }),
            RemovedTarget::Track(id) => !event
                .operations
                .iter()
                .any(|op| matches!(op,Operation::RemoveTrack {track} if track==id)),
        }) {
            return Err(Error::InvalidLedger);
        }
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
        owner_bytes = owner_bytes
            .checked_sub(released)
            .ok_or(Error::InvalidOrigin)?;
    }
    let (seeds_next, baseline_resources, baseline_bytes) =
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
                next.apply(creation, &command.identities)?;
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
            (Some(next), Some(resources), bytes)
        } else {
            (None, None, old_baseline)
        };
    if owner_bytes > role.envelope().owners {
        return Err(Error::Capacity("owner metadata"));
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
        },
        approval,
        seeds: seeds_next,
        baseline_resources,
        owner_bytes,
        baseline_bytes,
        created_tracks,
        discarded,
    })
}
pub(super) fn origin_charge(
    origin: &CommandOrigin,
    resources: &BTreeSet<Resource>,
) -> Result<u64, Error> {
    // Two inline slots cover geometric VecDeque capacity, plus the two
    // independently owned 32-byte IDs and their allocation headers.
    let mut bytes = (2 * std::mem::size_of::<LiveOrigin>()) as u64 + 112;
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
    768_u64
        .checked_add((approval.targets.capacity() * std::mem::size_of::<RemovedTarget>()) as u64)
        .and_then(|n| n.checked_add(approval.reference.sha256.capacity() as u64))
        .ok_or(Error::Capacity("approval size"))
}
pub(super) fn baseline_charge(
    lineage: &Lineage,
    resources: &BTreeSet<Resource>,
) -> Result<u64, Error> {
    exact_size(lineage)?
        .checked_mul(4)
        .and_then(|n| n.checked_add(lineage.occurrences.len() as u64 * 512))
        .and_then(|n| n.checked_add(resources.len() as u64 * 512 + 4096))
        .ok_or(Error::Capacity("baseline metadata"))
}
