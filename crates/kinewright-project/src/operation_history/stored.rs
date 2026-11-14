//! Compact store shell. The operation/owner body stays raw until an owner opens it.
use super::snapshot::SnapshotData;
use super::{Error, HistoryPool, OwnerSnapshot, Resource, Role, ValidatedOwnerSnapshot};
use crate::{
    bundle::BundleError,
    bundle_records::{ObjectRef, RecordKind},
    storage_budget::StorageLimits,
    store::{BundleRecord, ProjectStore, SealedObject, record_sealed},
};
use kinewright_core::undo::{Record, RetentionPolicy, Step};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use std::{collections::VecDeque, sync::Arc};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoredOwnerRecord {
    pub role: Role,
    pub baseline: ObjectRef,
    pub document: ObjectRef,
    pub identity: String,
    pub edges: Vec<ObjectRef>,
    body: Box<RawValue>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Body {
    undo: VecDeque<WireStep>,
    redo: Vec<WireStep>,
    position: u64,
    epoch: u64,
    policy: RetentionPolicy,
    fold_frontier: Option<kinewright_core::undo::CommandKey>,
    #[serde(with = "super::transition::terminal_wire")]
    terminal_facts: super::transition::TerminalFacts,
    seeds: super::Lineage,
    origins: Vec<(super::CreationOrigin, Vec<ObjectRef>)>,
    resources: std::collections::BTreeSet<Resource>,
    bindings: Vec<(Resource, ObjectRef)>,
    ledger: super::ApprovalLedger,
    #[serde(with = "super::ledger::folded_wire")]
    folded_approvals: super::ledger::FoldedApprovals,
}
impl Role {
    #[must_use]
    pub const fn persisted_body_cap(self) -> u64 {
        self.bounds().decoded_history_bytes + self.envelope().owners
    }
    #[must_use]
    pub const fn persisted_shell_cap(self) -> u64 {
        self.bounds().decoded_history_bytes + 2 * self.envelope().owners + 8192
    }
    #[must_use]
    pub const fn persisted_graph_cap(self) -> u64 {
        self.persisted_shell_cap() + 2 * self.bounds().document_bytes
    }
}
fn digest_text(bytes: &[u8; 32]) -> String {
    use std::fmt::Write as _;
    let mut text = String::with_capacity(64);
    for byte in bytes {
        let _ = write!(text, "{byte:02x}");
    }
    text
}
#[expect(
    clippy::needless_pass_by_value,
    reason = "map_err transfers the store error into the owner error boundary"
)]
fn store_error(error: BundleError) -> Error {
    Error::Store(error.to_string())
}
#[derive(Serialize)]
struct BorrowedBody<'a> {
    #[serde(serialize_with = "write_undo")]
    undo: &'a VecDeque<Step>,
    #[serde(serialize_with = "write_redo")]
    redo: &'a [Step],
    position: u64,
    epoch: u64,
    policy: RetentionPolicy,
    fold_frontier: Option<kinewright_core::undo::CommandKey>,
    #[serde(with = "super::transition::terminal_wire")]
    terminal_facts: &'a super::transition::TerminalFacts,
    seeds: &'a super::Lineage,
    #[serde(serialize_with = "write_origins")]
    origins: &'a [super::CommandOrigin],
    resources: &'a std::collections::BTreeSet<Resource>,
    bindings: &'a [(Resource, ObjectRef)],
    ledger: &'a super::ApprovalLedger,
    #[serde(with = "super::ledger::folded_wire")]
    folded_approvals: &'a super::ledger::FoldedApprovals,
}
impl OwnerSnapshot {
    pub fn persist(
        &self,
        store: &mut ProjectStore,
        mut resolve_resource: impl FnMut(&Resource) -> Result<Option<ObjectRef>, Error>,
    ) -> Result<SealedObject, Error> {
        let _window = self.reservation.workspace()?;
        let data = &self.data;
        let baseline = store
            .admit_document(&data.history.baseline)
            .map_err(store_error)?;
        let document = store
            .admit_document(&data.history.document)
            .map_err(store_error)?;
        let mut edges = std::collections::BTreeSet::new();
        for origin in &data.origins {
            edges.extend(origin.inputs.iter().cloned());
        }
        for approval in data.ledger.transition_entries() {
            edges.insert(approval.id.clone());
        }
        for approval in data.ledger.entries() {
            edges.insert(approval.approval.reference.clone());
        }
        let mut bindings = Vec::new();
        for resource in &data.resources {
            match resolve_resource(resource)? {
                Some(reference) if binding_matches(store, resource, &reference)? => {
                    bindings.push((resource.clone(), reference.clone()));
                    edges.insert(reference);
                }
                None if matches!(resource, Resource::Media { sha256: None, .. }) => {}
                _ => return Err(Error::MissingResource),
            }
        }
        // This serialization borrows the live command arrays. No second typed
        // payload list is made just to manufacture the compact store body.
        let history = &data.history;
        let size = super::owner::exact_size(&BorrowedBody {
            undo: &history.undo,
            redo: &history.redo,
            position: history.position,
            epoch: history.epoch,
            policy: history.policy,
            fold_frontier: data.fold_frontier,
            terminal_facts: &data.terminal_facts,
            seeds: &data.seeds,
            origins: &data.origins,
            resources: &data.resources,
            bindings: &bindings,
            ledger: &data.ledger,
            folded_approvals: &data.folded_approvals,
        })?;
        if size > data.role.persisted_body_cap() {
            return Err(Error::Capacity("persisted owner body"));
        }
        let body = serde_json::value::to_raw_value(&BorrowedBody {
            undo: &history.undo,
            redo: &history.redo,
            position: history.position,
            epoch: history.epoch,
            policy: history.policy,
            fold_frontier: data.fold_frontier,
            terminal_facts: &data.terminal_facts,
            seeds: &data.seeds,
            origins: &data.origins,
            resources: &data.resources,
            bindings: &bindings,
            ledger: &data.ledger,
            folded_approvals: &data.folded_approvals,
        })
        .map_err(|_| Error::InvalidRecord)?;
        let record = StoredOwnerRecord {
            role: data.role,
            baseline: baseline.reference.clone(),
            document: document.reference.clone(),
            identity: digest_text(&self.snapshot_identity()?),
            edges: edges.into_iter().collect(),
            body,
        };
        if super::owner::exact_size(&record)? > data.role.persisted_shell_cap() {
            return Err(Error::Capacity("persisted owner shell"));
        }
        store.admit_record(&record).map_err(store_error)
    }
}
fn binding_matches(
    store: &ProjectStore,
    resource: &Resource,
    reference: &ObjectRef,
) -> Result<bool, Error> {
    if reference.record_kind == RecordKind::InputManifest {
        let manifest = store
            .read_record::<crate::owned_inputs::OwnedInputManifest>(reference)
            .map_err(store_error)?;
        return Ok(manifest.entries.iter().any(|entry| match resource {
            Resource::Media {
                asset,
                sha256: Some(hash),
                byte_len: Some(len),
            } => {
                entry.object.record_kind == RecordKind::OwnedMedia
                    && &entry.object.sha256 == hash
                    && entry.object.byte_length == *len
                    && entry.entry_id.starts_with(&format!("media:{asset}:"))
                    && entry.chunk_table_ref().is_ok()
            }
            _ => blob_matches(resource, &entry.object),
        }));
    }
    // Media keeps its manifest and chunk/profile dependencies, not just a
    // naked byte object. LUTs and the embedded font also admit direct blobs.
    Ok(!matches!(resource, Resource::Media { .. }) && blob_matches(resource, reference))
}
fn blob_matches(resource: &Resource, reference: &ObjectRef) -> bool {
    match resource {
        Resource::Media {
            sha256: Some(hash),
            byte_len: Some(len),
            ..
        } => {
            reference.record_kind == RecordKind::OwnedMedia
                && &reference.sha256 == hash
                && reference.byte_length == *len
        }
        Resource::Lut { sha256, byte_len } => {
            reference.record_kind == RecordKind::OwnedLut
                && &reference.sha256 == sha256
                && reference.byte_length == *byte_len
        }
        Resource::TitleFont => {
            reference.record_kind == RecordKind::OwnedFont
                && reference.sha256
                    == kinewright_media::sha256_bytes(kinewright_core::title_font_bytes())
                && reference.byte_length == kinewright_core::title_font_bytes().len() as u64
        }
        Resource::Media { .. } => false,
    }
}
impl StoredOwnerRecord {
    /// Reserve an owner and workspace before reading or decoding its shell.
    pub fn read(
        store: &ProjectStore,
        reference: &ObjectRef,
        pool: &HistoryPool,
        expected_role: Role,
    ) -> Result<ValidatedOwnerSnapshot, Error> {
        // Bound raw ingress by the requested role before the store allocates
        // its encoded/decoded shell, including a record claiming another role.
        if reference.byte_length > expected_role.persisted_shell_cap() {
            return Err(Error::Capacity("persisted owner shell"));
        }
        let reservation = pool.admit(expected_role)?;
        let window = reservation.workspace()?;
        let record = store
            .read_record::<Self>(reference)
            .map_err(store_error)?
            .take();
        record.resolve_reserved(store, expected_role, reservation, window)
    }
    #[cfg(all(test, target_os = "linux"))]
    pub(super) fn resolve(
        self,
        store: &ProjectStore,
        pool: &HistoryPool,
        expected_role: Role,
    ) -> Result<ValidatedOwnerSnapshot, Error> {
        if self.role != expected_role {
            return Err(Error::Reservation);
        }
        let reservation = pool.admit(self.role)?;
        let window = reservation.workspace()?;
        self.resolve_reserved(store, expected_role, reservation, window)
    }
    fn resolve_reserved(
        self,
        store: &ProjectStore,
        expected_role: Role,
        reservation: super::HistoryReservation,
        window: super::Workspace,
    ) -> Result<ValidatedOwnerSnapshot, Error> {
        if self.role != expected_role {
            return Err(Error::Reservation);
        }
        self.validate(store.limits()).map_err(store_error)?;
        let typed = kinewright_core::undo::payload_memory_bound(self.body.get().as_bytes())
            .map_err(|_| Error::Capacity("owner decode"))?;
        if typed
            .checked_add(self.body.get().len() as u64)
            .is_none_or(|n| n > self.role.envelope().workspace)
        {
            return Err(Error::Capacity("owner decode"));
        }
        let body = self.decode_body()?;
        let baseline = store
            .read_record::<kinewright_core::Document>(&self.baseline)
            .map_err(store_error)?;
        let document = store
            .read_record::<kinewright_core::Document>(&self.document)
            .map_err(store_error)?;
        let mut expected = std::collections::BTreeSet::new();
        for (_, inputs) in &body.origins {
            expected.extend(inputs.iter().cloned());
        }
        for entry in body.ledger.transition_entries() {
            expected.insert(entry.id.clone());
        }
        for entry in body.ledger.entries() {
            expected.insert(entry.approval.reference.clone());
        }
        if body.bindings.windows(2).any(|w| w[0].0 >= w[1].0) {
            return Err(Error::InvalidResource);
        }
        let required = body
            .resources
            .iter()
            .filter(|r| !matches!(r, Resource::Media { sha256: None, .. }));
        if !required.eq(body.bindings.iter().map(|(resource, _)| resource)) {
            return Err(Error::MissingResource);
        }
        for (resource, reference) in &body.bindings {
            if !binding_matches(store, resource, reference)? {
                return Err(Error::MissingResource);
            }
            expected.insert(reference.clone());
        }
        if !expected.iter().eq(self.edges.iter()) {
            return Err(Error::InvalidResource);
        }
        let undo: VecDeque<_> = body.undo.into_iter().map(WireStep::into_core).collect();
        let redo: Vec<_> = body.redo.into_iter().map(WireStep::into_core).collect();
        let commands = undo
            .iter()
            .chain(redo.iter().rev())
            .flat_map(Step::commands);
        if undo
            .iter()
            .chain(&redo)
            .map(|step| step.commands().len())
            .sum::<usize>()
            != body.origins.len()
        {
            return Err(Error::InvalidOrigin);
        }
        let origins = commands
            .zip(body.origins)
            .map(|(command, (creation, inputs))| super::CommandOrigin {
                key: command.id,
                payload_digest: command.payload_digest,
                creation,
                inputs,
            })
            .collect();
        // Record transfers these documents to History; its current/baseline
        // pointers and subsequent cache entries share their immutable values.
        let history = Record {
            baseline: Arc::new(baseline.take()),
            document: Arc::new(document.take()),
            undo,
            redo,
            position: body.position,
            epoch: body.epoch,
            policy: body.policy,
        };
        if digest_text(&history.snapshot_identity()?) != self.identity {
            return Err(Error::IdentityMismatch);
        }
        let data = SnapshotData {
            role: self.role,
            fold_frontier: body.fold_frontier,
            terminal_facts: body.terminal_facts,
            history,
            seeds: body.seeds,
            origins,
            resources: body.resources,
            ledger: body.ledger,
            folded_approvals: body.folded_approvals,
        };
        // Drop the raw store body before semantic replay; only the final typed
        // stacks and bounded owner scratch coexist with the replay document.
        drop(self);
        drop(window);
        OwnerSnapshot { data, reservation }.validate()
    }
    fn decode_body(&self) -> Result<Body, Error> {
        serde_json::from_str(self.body.get()).map_err(|_| Error::InvalidRecord)
    }
}
impl record_sealed::Sealed for StoredOwnerRecord {}
impl BundleRecord for StoredOwnerRecord {
    const KIND: RecordKind = RecordKind::OperationHistory;
    fn dependencies(&self) -> Vec<ObjectRef> {
        std::iter::once(self.baseline.clone())
            .chain(std::iter::once(self.document.clone()))
            .chain(self.edges.iter().cloned())
            .collect()
    }
    fn decoded_bound(bytes: &[u8], _limits: &StorageLimits) -> Result<u64, BundleError> {
        compact_decode_charge(bytes)
    }
    fn validate(&self, _limits: &StorageLimits) -> Result<(), BundleError> {
        if self.baseline.byte_length > self.role.bounds().document_bytes
            || self.document.byte_length > self.role.bounds().document_bytes
        {
            return Err(BundleError::Capacity("owner document edge"));
        }
        if self.body.get().len() as u64 > self.role.persisted_body_cap() {
            return Err(BundleError::Capacity("persisted owner body"));
        }
        if self.baseline.record_kind != RecordKind::Document
            || self.document.record_kind != RecordKind::Document
            || self.identity.len() != 64
            || !self.identity.bytes().all(|b| b.is_ascii_hexdigit())
            || self.edges.windows(2).any(|w| w[0] >= w[1])
        {
            return Err(BundleError::Corrupt("operation history shell".into()));
        }
        for reference in self.dependencies() {
            reference.validate()?;
        }
        Ok(())
    }
}
// RawValue is one encoded allocation. Count only header edges, never operation
// tokens, so a command does not incur the old per-JSON-node store inflation.
pub(crate) fn compact_decode_charge(bytes: &[u8]) -> Result<u64, BundleError> {
    #[derive(Deserialize)]
    struct Header {
        role: Role,
        edges: Count,
        #[serde(default)]
        body: serde::de::IgnoredAny,
    }
    struct Count(u64);
    impl<'de> Deserialize<'de> for Count {
        fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
            struct V;
            impl<'de> serde::de::Visitor<'de> for V {
                type Value = Count;
                fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                    f.write_str("edge array")
                }
                fn visit_seq<A: serde::de::SeqAccess<'de>>(
                    self,
                    mut a: A,
                ) -> Result<Count, A::Error> {
                    let mut n = 0_u64;
                    while a.next_element::<serde::de::IgnoredAny>()?.is_some() {
                        n = n
                            .checked_add(1)
                            .ok_or_else(|| serde::de::Error::custom("edge count"))?;
                    }
                    Ok(Count(n))
                }
            }
            d.deserialize_seq(V)
        }
    }
    let header: Header =
        serde_json::from_slice(bytes).map_err(|e| BundleError::Corrupt(e.to_string()))?;
    let _ = header.body;
    if bytes.len() as u64 > header.role.persisted_shell_cap() {
        return Err(BundleError::Capacity("persisted owner shell"));
    }
    (bytes.len() as u64)
        .checked_add(
            header
                .edges
                .0
                .checked_mul(512)
                .ok_or(BundleError::Capacity("owner edges"))?,
        )
        .and_then(|n| n.checked_add(4096))
        .ok_or(BundleError::Capacity("owner shell"))
}

// Wire tuples store each command key and digest once. An origin is attached to
// that exact command boundary rather than duplicating its key and digest. Core
// canonical identity still uses its unchanged Record representation.
#[derive(Deserialize)]
struct WireStep(Option<Box<str>>, Vec<WireCommand>);
#[derive(Deserialize)]
struct WireCommand(u64, u64, HexDigest, Box<[kinewright_core::Operation]>);
struct HexDigest([u8; 32]);
impl<'de> Deserialize<'de> for HexDigest {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let value = String::deserialize(d)?;
        if value.len() != 64
            || !value
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(serde::de::Error::custom("command digest"));
        }
        let mut bytes = [0; 32];
        for (out, pair) in bytes.iter_mut().zip(value.as_bytes().as_chunks::<2>().0) {
            let digit = |v: u8| {
                if v.is_ascii_digit() {
                    v - b'0'
                } else {
                    v - b'a' + 10
                }
            };
            *out = digit(pair[0]) * 16 + digit(pair[1]);
        }
        Ok(Self(bytes))
    }
}
impl WireStep {
    fn into_core(self) -> Step {
        Step::unvalidated(
            self.1
                .into_iter()
                .map(|WireCommand(epoch, position, digest, operations)| {
                    kinewright_core::undo::StepCommand {
                        id: kinewright_core::undo::CommandKey { epoch, position },
                        position,
                        payload_digest: digest.0,
                        operations,
                    }
                })
                .collect(),
            self.0,
        )
    }
}
struct CommandWire<'a>(&'a kinewright_core::undo::StepCommand);
impl Serialize for CommandWire<'_> {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let c = self.0;
        (
            c.id.epoch,
            c.id.position,
            digest_text(&c.payload_digest),
            &c.operations,
        )
            .serialize(s)
    }
}
struct CommandsWire<'a>(&'a [kinewright_core::undo::StepCommand]);
impl Serialize for CommandsWire<'_> {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_seq(self.0.iter().map(CommandWire))
    }
}
struct StepWire<'a>(&'a Step);
impl Serialize for StepWire<'_> {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        (self.0.coalesce_key(), CommandsWire(self.0.commands())).serialize(s)
    }
}
fn write_undo<S: serde::Serializer>(steps: &VecDeque<Step>, s: S) -> Result<S::Ok, S::Error> {
    s.collect_seq(steps.iter().map(StepWire))
}
fn write_redo<S: serde::Serializer>(steps: &[Step], s: S) -> Result<S::Ok, S::Error> {
    s.collect_seq(steps.iter().map(StepWire))
}
fn write_origins<S: serde::Serializer>(
    origins: &[super::CommandOrigin],
    s: S,
) -> Result<S::Ok, S::Error> {
    s.collect_seq(
        origins
            .iter()
            .map(|origin| (&origin.creation, &origin.inputs)),
    )
}

/// Per-input work only. The temporary encoding is dropped before Core copies
/// its candidate; the resulting upper bound admits a future stored decode.
pub(super) fn command_decode_charge(
    command: &kinewright_core::Command,
    creation: &super::CreationOrigin,
    inputs: &[ObjectRef],
    workspace: u64,
) -> Result<u64, Error> {
    use kinewright_core::Command;
    let operations = match command {
        Command::Do(operation) => std::slice::from_ref(operation),
        Command::DoBatch(operations) | Command::DoBatchCoalesced { operations, .. } => {
            operations.as_slice()
        }
        _ => {
            return Err(Error::History(
                kinewright_core::undo::Error::UnsupportedCommand,
            ));
        }
    };
    fragment_charge(operations, creation, inputs, workspace)
}
pub(super) fn fragment_charge(
    operations: &[kinewright_core::Operation],
    creation: &super::CreationOrigin,
    inputs: &[ObjectRef],
    workspace: u64,
) -> Result<u64, Error> {
    #[derive(Serialize)]
    struct C<'a>(u64, u64, &'static str, &'a [kinewright_core::Operation]);
    // Original counters can eventually use all twenty decimal digits. The
    // digest's spelling is always 64 bytes, independent of its value.
    let commands = [C(
        u64::MAX,
        u64::MAX,
        "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
        operations,
    )];
    let value = ((None::<&str>, &commands), (creation, inputs));
    payload_decode_charge(&value, workspace / 4)
}
fn payload_decode_charge(value: &impl Serialize, limit: u64) -> Result<u64, Error> {
    struct Counter {
        bytes: u64,
        limit: u64,
    }
    impl std::io::Write for Counter {
        fn write(&mut self, value: &[u8]) -> std::io::Result<usize> {
            self.bytes = self
                .bytes
                .checked_add(value.len() as u64)
                .filter(|bytes| *bytes <= self.limit)
                .ok_or_else(|| std::io::Error::other("wire capacity"))?;
            Ok(value.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut counter = Counter { bytes: 0, limit };
    serde_json::to_writer(&mut counter, value).map_err(|_| Error::Capacity("owner decode"))?;
    let bytes = serde_json::to_vec(value).map_err(|_| Error::InvalidRecord)?;
    super::owner::payload_serialized(counter.bytes * 2);
    kinewright_core::undo::payload_memory_bound(&bytes)?
        .checked_add(bytes.len() as u64)
        .ok_or(Error::Capacity("owner decode"))
}
pub(super) fn baseline_decode_charge(
    lineage: &super::Lineage,
    resources: usize,
    workspace: u64,
) -> Result<u64, Error> {
    // One exact aggregate walk at construction/open/fold, alongside the other
    // baseline walks. Ordinary commands never visit this map.
    super::owner::exact_size(lineage)?;
    payload_decode_charge(lineage, workspace / 4)?
        .checked_add(resources as u64 * 4096 + 8192)
        .ok_or(Error::Capacity("owner decode"))
}
pub(super) fn approval_decode_charge(approval: &super::Approval) -> Result<u64, Error> {
    // Includes the largest fate/install-epoch spelling, the eventual folded
    // key/digest fact, sparse JSON headers and both copies of every target.
    16_384_u64
        .checked_add(approval.targets.len() as u64 * 1024)
        .ok_or(Error::Capacity("owner decode"))
}

pub(super) fn key_decode_charge(key: Option<&str>, workspace: u64) -> Result<u64, Error> {
    key.map(|key| payload_decode_charge(&key, workspace / 4))
        .transpose()
        .map(|v| v.unwrap_or(0))
}

#[cfg(all(test, target_os = "linux"))]
#[path = "stored_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "stored_codec_tests.rs"]
mod codec_tests;
