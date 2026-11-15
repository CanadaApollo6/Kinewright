//! Executable session specification. Both replicas use the new executor and
//! owner; both stores use the production compact codec and semantic validator.
//! The protocol harness is replaced by product calls as the switch proceeds.
use super::*;
use crate::operation_history::{
    Approval, ApprovalAnswer, CreationOrigin, Direction, OwnerHistory, PoolLimits, Resources,
    tests::{document, origin},
};
use kinewright_core::{
    Command, LutAsset, LutAssetId, LutAssetKind, LutAssetSource, Marker, MarkerId, Operation,
    TimeCode, TrackId,
    undo::{CommandKey, Effects, RoleBounds},
};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

#[path = "session_model_store.rs"]
mod storage;
use storage::{MemoryStore, ModelStore};

fn bounds() -> RoleBounds {
    RoleBounds {
        step_cap: 3,
        operation_bytes: 4096,
        ..Role::Ordinary.bounds()
    }
}
fn fresh(pool: &HistoryPool) -> OwnerHistory {
    OwnerHistory::measured(
        pool,
        Role::Ordinary,
        RetentionPolicy::Fold,
        &origin(1),
        bounds(),
        || Ok(document()),
    )
    .unwrap()
}

#[derive(Clone, Serialize, Deserialize)]
enum Action {
    Edit {
        operations: Vec<Operation>,
        gesture: Option<String>,
    },
    Undo,
    Redo,
    Close,
}
#[derive(Clone, Serialize, Deserialize)]
struct Message {
    id: u64,
    action: Action,
    creation: CreationOrigin,
    approval: Option<Approval>,
    transition: Option<ObjectRef>,
}
impl Message {
    fn apply(&self, owner: &mut OwnerHistory) -> Result<Effects, Error> {
        // Each replica owns its accepted payload. Fan-out is part of the
        // model's GUI/writer boundary, not an allocation-normalising copy.
        if let Some(reference) = &self.transition {
            let direction = match self.action {
                Action::Undo => Direction::Undo,
                Action::Redo => Direction::Redo,
                _ => unreachable!("only Undo/Redo carries transition consent"),
            };
            return owner.approve_transition(
                reference.clone(),
                direction,
                owner.transition_target(direction).unwrap(),
            );
        }
        let command = match &self.action {
            Action::Edit {
                operations,
                gesture: Some(key),
            } => Command::DoBatchCoalesced {
                operations: operations.clone(),
                coalesce_key: key.clone(),
            },
            Action::Edit {
                operations,
                gesture: None,
            } => Command::DoBatch(operations.clone()),
            Action::Undo => Command::Undo,
            Action::Redo => Command::Redo,
            Action::Close => return owner.close_gesture(),
        };
        owner.execute(
            command,
            self.creation.clone(),
            vec![],
            self.approval.clone(),
        )
    }
    fn digest(&self, previous: &[u8; 32]) -> [u8; 32] {
        // Normalised commands contain no revision precondition or transport
        // authority. Receipt ids identify retransmissions across a hand-off.
        let bytes = serde_json::to_vec(&(
            previous,
            &self.action,
            &self.creation,
            &self.approval,
            &self.transition,
        ))
        .unwrap();
        let mut hash = kinewright_hash::Sha256::new();
        hash.update(&bytes);
        hash.finalize()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct Receipt {
    id: u64,
    key: CommandKey,
    tip: [u8; 32],
    command: [u8; 32],
}
struct Pending {
    message: Message,
    authority: u64,
    sequence: u64,
    predecessor: [u8; 32],
}
#[derive(Clone, Serialize, Deserialize)]
struct Root {
    snapshot: ObjectRef,
    installed: [u8; 32],
    sequence: u64,
    tip: [u8; 32],
    receipts: Vec<Receipt>,
    authority: u64,
}
#[derive(Default)]
struct Coverage {
    edits: usize,
    folds: usize,
    undos: usize,
    redos: usize,
    closes: usize,
    approvals: usize,
    transitions: usize,
    restarts: usize,
    lost_acks: usize,
    before_publish: usize,
    handoffs: usize,
    saves: usize,
    save_as: usize,
    rescues: usize,
    opens: usize,
    collections: usize,
    baseline_lineage: usize,
    split_ancestry: usize,
    redo_payloads: usize,
    ordinary_lease_peak: usize,
}
impl Coverage {
    fn accumulate(&mut self, other: &Self) {
        self.edits += other.edits;
        self.folds += other.folds;
        self.undos += other.undos;
        self.redos += other.redos;
        self.closes += other.closes;
        self.approvals += other.approvals;
        self.transitions += other.transitions;
        self.restarts += other.restarts;
        self.lost_acks += other.lost_acks;
        self.before_publish += other.before_publish;
        self.handoffs += other.handoffs;
        self.saves += other.saves;
        self.save_as += other.save_as;
        self.rescues += other.rescues;
        self.opens += other.opens;
        self.collections += other.collections;
        self.baseline_lineage += other.baseline_lineage;
        self.split_ancestry += other.split_ancestry;
        self.redo_payloads += other.redo_payloads;
        self.ordinary_lease_peak = self.ordinary_lease_peak.max(other.ordinary_lease_peak);
    }
}
struct Session<S> {
    gui_pool: HistoryPool,
    writer_pool: HistoryPool,
    gui: OwnerHistory,
    writer: OwnerHistory,
    store: S,
    pending: VecDeque<Pending>,
    authority: u64,
    gui_sequence: u64,
    gui_tip: [u8; 32],
    acknowledged: BTreeMap<u64, Receipt>,
    accepted: BTreeMap<u64, Effects>,
    checkpoints: Vec<Root>,
    coverage: Coverage,
}
impl<S: ModelStore> Session<S> {
    fn new(mut store: S) -> Self {
        let gui_pool = HistoryPool::new(PoolLimits::default());
        let writer_pool = HistoryPool::new(PoolLimits::default());
        let gui = fresh(&gui_pool);
        let writer = fresh(&writer_pool);
        let snapshot = store.save(&writer.capture().unwrap());
        let installed = writer.capture().unwrap().snapshot_identity().unwrap();
        store.publish(&Root {
            snapshot,
            installed,
            sequence: 0,
            tip: installed,
            receipts: vec![],
            authority: 1,
        });
        Self {
            gui_pool,
            writer_pool,
            gui,
            writer,
            store,
            pending: VecDeque::new(),
            authority: 1,
            gui_sequence: 0,
            gui_tip: installed,
            acknowledged: BTreeMap::new(),
            accepted: BTreeMap::new(),
            checkpoints: vec![],
            coverage: Coverage::default(),
        }
    }
    fn send(&mut self, message: Message) {
        let before = self.gui.capture().unwrap().snapshot_identity().unwrap();
        match message.apply(&mut self.gui) {
            Ok(effects) => {
                self.coverage.folds += effects.folded.len();
                self.coverage.approvals += usize::from(message.approval.is_some());
                self.coverage.transitions += usize::from(message.transition.is_some());
                match &message.action {
                    Action::Edit { .. } => self.coverage.edits += 1,
                    Action::Undo => self.coverage.undos += 1,
                    Action::Redo => self.coverage.redos += 1,
                    Action::Close => self.coverage.closes += 1,
                }
                self.accepted.insert(message.id, effects);
                self.gui_sequence += 1;
                let next_tip = message.digest(&self.gui_tip);
                self.pending.push_back(Pending {
                    message,
                    authority: self.authority,
                    sequence: self.gui_sequence,
                    predecessor: self.gui_tip,
                });
                self.gui_tip = next_tip;
            }
            Err(_) => assert_eq!(
                self.gui.capture().unwrap().snapshot_identity().unwrap(),
                before,
                "SESSION_REFUSAL_IS_ATOMIC"
            ),
        }
    }
    fn drain(&mut self, lose_ack: bool, crash_before_publish: bool) {
        while let Some(pending) = self.pending.front() {
            let message = &pending.message;
            let mut root = self.store.root();
            if !root.receipts.iter().any(|r| r.id == message.id) {
                assert_eq!(
                    pending.authority, root.authority,
                    "ABSENT_SUFFIX_REQUIRES_EXPLICIT_CURRENT_AUTHORITY_REBIND"
                );
                assert_eq!(
                    pending.sequence,
                    root.sequence + 1,
                    "SHADOW_ACCEPTS_ONLY_THE_NEXT_SEQUENCE"
                );
                assert_eq!(
                    pending.predecessor, root.tip,
                    "SHADOW_ACCEPTS_ONLY_THE_CURRENT_CHAIN_TIP"
                );
                let effects = message
                    .apply(&mut self.writer)
                    .expect("SHADOW_ACCEPTS_GUI_TRANSITION");
                assert_eq!(
                    &effects, &self.accepted[&message.id],
                    "SHADOW_ACCOUNTS_FOR_EACH_GUI_TRANSITION"
                );
                let tip = message.digest(&root.tip);
                root.receipts.push(Receipt {
                    id: message.id,
                    key: effects.accepted,
                    tip,
                    command: message.digest(&[0; 32]),
                });
                root.sequence += 1;
                root.tip = tip;
                root.snapshot = self.store.save(&self.writer.capture().unwrap());
                if crash_before_publish {
                    self.coverage.before_publish += 1;
                    self.writer = self
                        .store
                        .load(&self.store.root().snapshot, &self.writer_pool)
                        .open()
                        .unwrap();
                    break;
                }
                self.store.publish(&root);
            }
            if lose_ack {
                self.coverage.lost_acks += 1;
                break;
            }
            let committed = self.store.root();
            let receipt = committed
                .receipts
                .iter()
                .find(|r| r.id == message.id)
                .expect("ACK_FOLLOWS_ATOMIC_ROOT_AND_RECEIPT_PUBLICATION");
            assert_eq!(
                receipt.command,
                message.digest(&[0; 32]),
                "RECEIPTED_ID_CANNOT_NAME_A_DIFFERENT_COMMAND"
            );
            self.acknowledged.insert(message.id, *receipt);
            self.pending.pop_front();
        }
        self.assert_acknowledged();
        if self.pending.is_empty() {
            let root = self.store.root();
            assert_eq!(
                (root.sequence, root.tip),
                (self.gui_sequence, self.gui_tip),
                "DRAINED_REPLICAS_HAVE_THE_SAME_SEQUENCE_AND_CHAIN"
            );
            self.assert_equal();
        }
    }
    fn assert_acknowledged(&self) {
        let root = self.store.root();
        let index: BTreeMap<_, _> = root.receipts.iter().map(|r| (r.id, r)).collect();
        for receipt in self.acknowledged.values() {
            assert_eq!(
                index.get(&receipt.id).copied(),
                Some(receipt),
                "EVERY_ACKNOWLEDGED_TRANSITION_HAS_A_DURABLE_RECEIPT"
            );
        }
        assert_eq!(
            index.len(),
            root.receipts.len(),
            "RESTART_NEVER_REAPPLIES_A_RECEIPTED_MESSAGE"
        );
    }
    fn assert_equal(&mut self) {
        let root = self.store.root();
        let verification_pool = HistoryPool::new(PoolLimits::default());
        let opened = self
            .store
            .load(&root.snapshot, &verification_pool)
            .open()
            .unwrap();
        {
            let baseline = opened.boundary(0).unwrap();
            self.coverage.baseline_lineage +=
                usize::from(!baseline.lineage().occurrences.is_empty());
            self.coverage.split_ancestry +=
                usize::from(!baseline.lineage().split_ancestry.is_empty());
        }
        for owner in [&self.gui, &self.writer] {
            assert_same_values(owner, &opened);
        }
        self.assert_acknowledged();
        for pool in [&self.gui_pool, &self.writer_pool, &verification_pool] {
            let peak = pool.lease_counts(Role::Ordinary).unwrap().1;
            self.coverage.ordinary_lease_peak = self.coverage.ordinary_lease_peak.max(peak);
            assert!(peak <= 2, "SESSION_ORDINARY_LEASE_PEAK_AT_MOST_TWO");
        }
    }
    fn cold_open(&mut self, reference: &ObjectRef) {
        let validated = self.store.load(reference, &self.writer_pool);
        let before_save = validated.identity().digest();
        // Replacing the actor releases its old lease before taking a new capture.
        self.writer = validated.open().unwrap();
        assert_eq!(
            self.writer.capture().unwrap().snapshot_identity().unwrap(),
            before_save,
            "SESSION_IDENTITY_AFTER_OPEN_EQUALS_BEFORE_SAVE"
        );
    }
    fn restart(&mut self, handoff: bool) {
        let mut root = self.store.cold_root();
        // Reopening committed history preserves the epoch. Authority rotates
        // independently: replay is only the suffix with no committed receipt.
        self.cold_open(&root.snapshot);
        self.authority += 1;
        root.authority = self.authority;
        self.store.publish(&root);
        for pending in &mut self.pending {
            if !root
                .receipts
                .iter()
                .any(|receipt| receipt.id == pending.message.id)
            {
                pending.authority = self.authority;
            }
        }
        self.coverage.restarts += 1;
        self.coverage.handoffs += usize::from(handoff);
        self.drain(false, false);
    }
    fn save(&mut self, save_as: bool) {
        self.drain(false, false);
        if save_as {
            self.copy_destination();
            self.install(&self.store.root().snapshot);
        }
        self.checkpoints.push(self.store.root());
        if self.checkpoints.len() > 2 {
            self.checkpoints.remove(0);
        }
        self.coverage.saves += 1;
        self.coverage.save_as += usize::from(save_as);
    }
    fn install_checkpoint(&mut self, selected: usize) {
        self.drain(false, false);
        if self.checkpoints.is_empty() {
            self.save(false);
        }
        let snapshot = self.checkpoints[selected % self.checkpoints.len()]
            .snapshot
            .clone();
        self.copy_destination();
        self.install(&snapshot);
        self.coverage.rescues += 1;
    }
    fn copy_destination(&mut self) {
        let mut roots: Vec<_> = self
            .checkpoints
            .iter()
            .map(|r| r.snapshot.clone())
            .collect();
        roots.push(self.store.root().snapshot);
        // The source is retired here. A shallow alias cannot pass the next
        // cold read or resource-collection check.
        self.store = self.store.copy_to_new(&roots);
        self.assert_equal();
    }
    fn install(&mut self, snapshot: &ObjectRef) {
        self.gui
            .install_snapshot(self.store.load(snapshot, &self.gui_pool))
            .unwrap();
        self.writer
            .install_snapshot(self.store.load(snapshot, &self.writer_pool))
            .unwrap();
        assert!(
            self.gui
                .ledger()
                .entries()
                .all(|e| e.answer() != ApprovalAnswer::Uncertain),
            "SAME_LINE_SESSION_INSTALL_NEVER_MAKES_DO_UNCERTAIN"
        );
        let mut root = self.store.root();
        root.snapshot = self.store.save(&self.writer.capture().unwrap());
        root.installed = self.writer.capture().unwrap().snapshot_identity().unwrap();
        root.sequence = 0;
        root.tip = root.installed;
        self.gui_sequence = 0;
        self.gui_tip = root.installed;
        self.store.publish(&root);
        assert_eq!(
            self.store.root().tip,
            self.gui.capture().unwrap().snapshot_identity().unwrap(),
            "INSTALL_RESETS_THE_PREVIOUS_EPOCH_CHAIN"
        );
        self.assert_equal();
    }
    fn collect(&mut self) {
        let mut roots: Vec<_> = self
            .checkpoints
            .iter()
            .map(|r| r.snapshot.clone())
            .collect();
        roots.push(self.store.root().snapshot);
        let mut references = self
            .gui
            .resources()
            .values()
            .map(|r| self.store.resource(r))
            .collect::<Vec<_>>();
        references.extend(
            self.pending
                .iter()
                .filter_map(|p| p.message.approval.as_ref().map(|a| a.reference.clone())),
        );
        references.extend(
            self.pending
                .iter()
                .filter_map(|p| p.message.transition.clone()),
        );
        for pending in &self.pending {
            if let Action::Edit { operations, .. } = &pending.message.action {
                references.extend(
                    Resources::payload(operations)
                        .iter()
                        .map(|r| self.store.resource(r)),
                );
            }
        }
        roots.extend(references);
        self.store.collect(&roots);
        for owner in [&self.gui, &self.writer] {
            let current = Resources::document(owner.history().document());
            for command in owner.history().redo().iter().flat_map(Step::commands) {
                self.coverage.redo_payloads += Resources::payload(&command.operations)
                    .iter()
                    .filter(|r| !current.contains(r))
                    .count();
            }
            for depth in 0..=owner.history().undo().len() + owner.history().redo().len() {
                let boundary = owner.boundary(depth).unwrap();
                for resource in &Resources::document(boundary.document()) {
                    self.store.assert_exists(&self.store.resource(resource));
                }
            }
            // A payload can be reachable only by Redo, without appearing in
            // any current document. The union and all roots must survive GC.
            for resource in owner.resources().values() {
                self.store.assert_exists(&self.store.resource(resource));
            }
        }
        for checkpoint in &self.checkpoints {
            let _ = self.store.load(&checkpoint.snapshot, &self.writer_pool);
        }
        self.coverage.collections += 1;
    }
}

fn assert_same_values(left: &OwnerHistory, right: &OwnerHistory) {
    let a = left.history();
    let b = right.history();
    assert_eq!(a.document(), b.document(), "SESSION_REOPEN_DOCUMENT");
    assert_eq!(a.baseline(), b.baseline(), "SESSION_REOPEN_BASELINE");
    assert_eq!(
        a.position(),
        b.position(),
        "SESSION_REOPEN_EPOCH_AND_COUNTER"
    );
    assert_eq!(a.policy(), b.policy(), "SESSION_REOPEN_POLICY");
    assert_eq!(a.undo().len(), b.undo().len(), "SESSION_REOPEN_UNDO_DEPTH");
    assert_eq!(a.redo().len(), b.redo().len(), "SESSION_REOPEN_REDO_DEPTH");
    for (original, reopened) in a
        .undo()
        .iter()
        .chain(a.redo())
        .zip(b.undo().iter().chain(b.redo()))
    {
        assert_eq!(
            original.commands(),
            reopened.commands(),
            "SESSION_REOPEN_COMMAND_BOUNDARIES_KEYS_DIGESTS_AND_PAYLOADS"
        );
        assert_eq!(
            original.coalesce_key(),
            reopened.coalesce_key(),
            "SESSION_REOPEN_GESTURE_KEYS"
        );
    }
    assert_eq!(
        left.reservation.role(),
        right.reservation.role(),
        "SESSION_REOPEN_OWNER_ROLE"
    );
    assert_eq!(
        left.seeds(),
        right.seeds(),
        "SESSION_REOPEN_BASELINE_LINEAGE"
    );
    assert!(
        left.origins
            .0
            .iter()
            .map(|o| &o.origin)
            .eq(right.origins.0.iter().map(|o| &o.origin)),
        "SESSION_REOPEN_COMMAND_ORIGINS_AND_INPUTS"
    );
    assert_eq!(
        left.resources(),
        right.resources(),
        "SESSION_REOPEN_REACHABLE_RESOURCES"
    );
    assert_eq!(
        left.ledger(),
        right.ledger(),
        "SESSION_REOPEN_APPROVAL_ENTRIES_AND_FATES"
    );
    assert_eq!(
        left.fold_frontier, right.fold_frontier,
        "SESSION_REOPEN_FOLD_FRONTIER"
    );
    assert_eq!(
        left.terminal_facts, right.terminal_facts,
        "SESSION_REOPEN_TRANSITION_PROVENANCE"
    );
    assert_eq!(
        left.folded_approvals, right.folded_approvals,
        "SESSION_REOPEN_FOLDED_COMMAND_PROVENANCE"
    );
}

fn marker(id: u64) -> Operation {
    Operation::AddMarker {
        marker: Marker {
            id: MarkerId(id),
            position: TimeCode(0),
            label: format!("marker {id}"),
            color_token: 0,
        },
    }
}
fn lut(reference: &ObjectRef, id: u64) -> Operation {
    Operation::AddLutAsset {
        asset: LutAsset {
            id: LutAssetId(id),
            sha256: reference.sha256.clone(),
            title: format!("LUT {id}"),
            kind: LutAssetKind::Cube3d,
            size: 2,
            byte_len: reference.byte_length,
            domain_min_millionths: [0; 3],
            domain_max_millionths: [1_000_000; 3],
            source: LutAssetSource::Imported {
                source_path: format!("lut-{id}.cube"),
            },
        },
    }
}

#[path = "session_model_stream.rs"]
mod stream;

#[test]
fn generated_sessions_preserve_acknowledgements_and_reachable_states() {
    stream::run::<MemoryStore>(8, 160);
}
#[test]
#[cfg_attr(
    not(feature = "slow-tests"),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn long_generated_sessions_seeds_01_04() {
    stream::run_range::<MemoryStore>(1, 4, 1024);
}

#[test]
#[cfg_attr(
    not(feature = "slow-tests"),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn long_generated_sessions_seeds_05_08() {
    stream::run_range::<MemoryStore>(5, 8, 1024);
}

#[test]
#[cfg_attr(
    not(feature = "slow-tests"),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn long_generated_sessions_seeds_09_12() {
    stream::run_range::<MemoryStore>(9, 12, 1024);
}

#[test]
#[cfg_attr(
    not(feature = "slow-tests"),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn long_generated_sessions_seeds_13_16() {
    stream::run_range::<MemoryStore>(13, 16, 1024);
}

#[test]
#[cfg_attr(
    not(feature = "slow-tests"),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn long_generated_sessions_seeds_17_20() {
    stream::run_range::<MemoryStore>(17, 20, 1024);
}

#[test]
#[cfg_attr(
    not(feature = "slow-tests"),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn long_generated_sessions_seeds_21_24() {
    stream::run_range::<MemoryStore>(21, 24, 1024);
}

#[test]
#[cfg_attr(
    not(feature = "slow-tests"),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn long_generated_sessions_seeds_25_28() {
    stream::run_range::<MemoryStore>(25, 28, 1024);
}

#[test]
#[cfg_attr(
    not(feature = "slow-tests"),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn long_generated_sessions_seeds_29_32() {
    stream::run_range::<MemoryStore>(29, 32, 1024);
}

#[test]
#[cfg_attr(
    not(feature = "slow-tests"),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn long_generated_sessions_seeds_33_36() {
    stream::run_range::<MemoryStore>(33, 36, 1024);
}

#[test]
#[cfg_attr(
    not(feature = "slow-tests"),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn long_generated_sessions_seeds_37_40() {
    stream::run_range::<MemoryStore>(37, 40, 1024);
}

#[test]
#[cfg_attr(
    not(feature = "slow-tests"),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn long_generated_sessions_seeds_41_44() {
    stream::run_range::<MemoryStore>(41, 44, 1024);
}

#[test]
#[cfg_attr(
    not(feature = "slow-tests"),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn long_generated_sessions_seeds_45_48() {
    stream::run_range::<MemoryStore>(45, 48, 1024);
}

#[test]
#[cfg_attr(
    not(feature = "slow-tests"),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn long_generated_sessions_seeds_49_52() {
    stream::run_range::<MemoryStore>(49, 52, 1024);
}

#[test]
#[cfg_attr(
    not(feature = "slow-tests"),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn long_generated_sessions_seeds_53_56() {
    stream::run_range::<MemoryStore>(53, 56, 1024);
}

#[test]
#[cfg_attr(
    not(feature = "slow-tests"),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn long_generated_sessions_seeds_57_60() {
    stream::run_range::<MemoryStore>(57, 60, 1024);
}

#[test]
#[cfg_attr(
    not(feature = "slow-tests"),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn long_generated_sessions_seeds_61_64() {
    stream::run_range::<MemoryStore>(61, 64, 1024);
}
#[test]
#[cfg(target_os = "linux")]
fn durable_sessions_use_the_same_codec_and_transition_stream() {
    stream::run::<storage::NativeStore>(2, 80);
}

fn queued_payload_after_gui_fold<S: ModelStore>() {
    let mut session = Session::new(S::default());
    let resource = session
        .store
        .blob(b"queued owned LUT", RecordKind::OwnedLut);
    for (index, operation) in [
        lut(&resource, 1),
        Operation::RemoveLutAsset {
            lut_asset: LutAssetId(1),
        },
        marker(10),
        marker(11),
        marker(12),
    ]
    .into_iter()
    .enumerate()
    {
        session.send(Message {
            id: index as u64,
            action: Action::Edit {
                operations: vec![operation],
                gesture: None,
            },
            creation: origin(index as u64 + 2),
            approval: None,
            transition: None,
        });
    }
    assert!(
        session.gui.resources().values().next().is_none(),
        "GUI_HAS_FOLDED_PAST_THE_QUEUED_RESOURCE"
    );
    assert!(
        session.store.root().receipts.is_empty(),
        "WRITER_HAS_NOT_SEEN_THE_PAYLOAD_YET"
    );
    session.collect();
    session.store.assert_exists(&resource);
    session.drain(false, false);
    session.collect();
    session.store.assert_absent(&resource);
}
#[test]
fn queued_payload_survives_collection_after_the_gui_folds_it_out() {
    queued_payload_after_gui_fold::<MemoryStore>();
}
#[test]
#[cfg(target_os = "linux")]
fn durable_queued_payload_survives_collection_after_the_gui_folds_it_out() {
    queued_payload_after_gui_fold::<storage::NativeStore>();
}
