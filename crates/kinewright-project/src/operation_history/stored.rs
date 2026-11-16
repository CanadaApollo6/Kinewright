//! Compact store shell. The operation/owner body stays raw until an owner opens it.
use super::borrowed_identity::{OriginContexts, ResourceValues, SeedValues};
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

pub(super) const DECODE_LIMIT: u64 = kinewright_core::undo::DECODED_PAYLOAD_BYTES;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoredOwnerRecord {
    pub role: Role,
    pub sizing: super::CreationSizing,
    pub baseline: ObjectRef,
    pub document: ObjectRef,
    pub identity: String,
    pub edges: Vec<ObjectRef>,
    body: Box<RawValue>,
}
/// Sealed bytes and their trusted fixed-size projection. Publication keeps
/// the pin without reading or decoding its own newly written record again.
pub struct SealedOwner {
    pub(crate) object: SealedObject,
    pub(crate) document: ObjectRef,
    pub(crate) identity: super::SnapshotIdentity,
    pub(crate) role: Role,
    pub(crate) position: kinewright_core::undo::CommandKey,
}
impl SealedOwner {
    #[must_use]
    pub const fn reference(&self) -> &ObjectRef {
        &self.object.reference
    }
    #[must_use]
    pub fn into_object(self) -> SealedObject {
        self.object
    }
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
fn store_error(error: BundleError) -> Error {
    match error {
        BundleError::Capacity(reason) => Error::StoreCapacity(reason),
        BundleError::Corrupt(reason) => Error::StoreCorrupt(reason),
        BundleError::Unsettled => Error::StoreUnsettled,
        BundleError::RunLimit => Error::StoreRunLimit,
        other => Error::Store(other.to_string()),
    }
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
    seeds: SeedValues<'a>,
    origins: OriginContexts<'a>,
    resources: ResourceValues<'a>,
    bindings: &'a [(Resource, ObjectRef)],
    ledger: &'a super::ApprovalLedger,
    #[serde(with = "super::ledger::folded_wire")]
    folded_approvals: &'a super::ledger::FoldedApprovals,
}
impl OwnerSnapshot {
    pub fn persist(
        &self,
        store: &mut ProjectStore,
        resolve_resource: impl FnMut(&Resource) -> Result<Option<ObjectRef>, Error>,
    ) -> Result<SealedObject, Error> {
        self.persist_checkpoint(store, resolve_resource)
            .map(SealedOwner::into_object)
    }
    pub fn persist_checkpoint(
        &self,
        store: &mut ProjectStore,
        mut resolve_resource: impl FnMut(&Resource) -> Result<Option<ObjectRef>, Error>,
    ) -> Result<SealedOwner, Error> {
        let _window = self.reservation.workspace()?;
        let data = &self.data;
        let baseline = store
            .admit_document(&data.history.baseline)
            .map_err(store_error)?;
        let document = store
            .admit_document(&data.history.document)
            .map_err(store_error)?;
        let mut bindings = Vec::new();
        for resource in &data.resources {
            match resolve_resource(resource)? {
                Some(reference) if binding_matches(store, resource, &reference)? => {
                    bindings.push((resource.clone(), reference));
                }
                None if matches!(resource, Resource::Media { sha256: None, .. }) => {}
                _ => return Err(Error::MissingResource),
            }
        }
        let identity = self.data.identity()?;
        let document = document.reference;
        let record = self.stored_record_identified(
            baseline.reference,
            document.clone(),
            &bindings,
            identity.digest(),
        )?;
        let object = store.admit_record(&record).map_err(store_error)?;
        store
            .remember_owner(
                object.reference.clone(),
                crate::bundle_recovery::OwnerProof {
                    identity,
                    position: kinewright_core::undo::CommandKey {
                        epoch: data.history.epoch,
                        position: data.history.position,
                    },
                    sizing: data.sizing,
                    document: crate::bundle_recovery::ProjectionRef::new(&document)
                        .map_err(store_error)?,
                },
            )
            .map_err(store_error)?;
        Ok(SealedOwner {
            object,
            document,
            identity,
            role: data.role,
            position: kinewright_core::undo::CommandKey {
                epoch: data.history.epoch,
                position: data.history.position,
            },
        })
    }
    #[cfg(any(test, feature = "test-util"))]
    pub(crate) fn stored_record(
        &self,
        baseline: ObjectRef,
        document: ObjectRef,
        bindings: &[(Resource, ObjectRef)],
    ) -> Result<StoredOwnerRecord, Error> {
        self.stored_record_identified(baseline, document, bindings, self.snapshot_identity()?)
    }
    fn stored_record_identified(
        &self,
        baseline: ObjectRef,
        document: ObjectRef,
        bindings: &[(Resource, ObjectRef)],
        identity: [u8; 32],
    ) -> Result<StoredOwnerRecord, Error> {
        let data = &self.data;
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
        edges.extend(bindings.iter().map(|(_, reference)| reference.clone()));
        // This serialization borrows the live command arrays. No second typed
        // payload list is made just to manufacture the compact store body.
        let history = &data.history;
        let body = BorrowedBody {
            undo: &history.undo,
            redo: &history.redo,
            position: history.position,
            epoch: history.epoch,
            policy: history.policy,
            fold_frontier: data.fold_frontier,
            terminal_facts: &data.terminal_facts,
            seeds: SeedValues::Captured(&data.seeds),
            origins: OriginContexts::Captured(&data.origins),
            resources: ResourceValues::Captured(&data.resources),
            bindings,
            ledger: &data.ledger,
            folded_approvals: &data.folded_approvals,
        };
        encode_record(
            &body,
            data.role,
            data.sizing,
            baseline,
            document,
            edges,
            identity,
        )
    }
}
fn encode_record(
    body: &BorrowedBody<'_>,
    role: Role,
    sizing: super::CreationSizing,
    baseline: ObjectRef,
    document: ObjectRef,
    edges: std::collections::BTreeSet<ObjectRef>,
    identity: [u8; 32],
) -> Result<StoredOwnerRecord, Error> {
    let limits = sizing.limits()?;
    if super::owner::exact_size(body)? > limits.body_cap() {
        return Err(Error::Capacity("persisted owner body"));
    }
    let body = serde_json::value::to_raw_value(body).map_err(|_| Error::InvalidRecord)?;
    let record = StoredOwnerRecord {
        role,
        sizing,
        baseline,
        document,
        identity: digest_text(&identity),
        edges: edges.into_iter().collect(),
        body,
    };
    if super::owner::exact_size(&record)? > limits.shell_cap() {
        return Err(Error::Capacity("persisted owner shell"));
    }
    Ok(record)
}
impl super::OwnerHistory {
    /// Publish the sealed prospective destination view. The working state
    /// remains in its own creation-sized preview holder; this writes bytes
    /// under scratch and creates no extra retained capture or lease.
    pub(crate) fn persist_prepared(
        &self,
        store: &mut ProjectStore,
        sizing: super::CreationSizing,
        mut resolve_resource: impl FnMut(&Resource) -> Result<Option<ObjectRef>, Error>,
    ) -> Result<SealedOwner, Error> {
        let _window = self.reservation.workspace()?;
        let baseline = store
            .admit_document(self.core.baseline())
            .map_err(store_error)?;
        let document = store
            .admit_document(self.core.document())
            .map_err(store_error)?;
        let mut bindings = Vec::new();
        for resource in self.resources.values() {
            match resolve_resource(resource)? {
                Some(reference) if binding_matches(store, resource, &reference)? => {
                    bindings.push((resource.clone(), reference));
                }
                None if matches!(resource, Resource::Media { sha256: None, .. }) => {}
                _ => return Err(Error::MissingResource),
            }
        }
        let mut edges = std::collections::BTreeSet::new();
        for row in &self.origins.0 {
            edges.extend(row.origin.inputs.iter().cloned());
        }
        edges.extend(
            self.ledger
                .transition_entries()
                .map(|entry| entry.id.clone()),
        );
        edges.extend(
            self.ledger
                .entries()
                .map(|entry| entry.approval.reference.clone()),
        );
        edges.extend(bindings.iter().map(|(_, reference)| reference.clone()));
        let position = self.core.position();
        let body = BorrowedBody {
            undo: self.core.undo(),
            redo: self.core.redo(),
            position: position.position,
            epoch: position.epoch,
            policy: self.core.policy(),
            fold_frontier: self.fold_frontier,
            terminal_facts: &self.terminal_facts,
            seeds: SeedValues::Live(self),
            origins: OriginContexts::Live(&self.origins),
            resources: ResourceValues::Live(&self.resources),
            bindings: &bindings,
            ledger: &self.ledger,
            folded_approvals: &self.folded_approvals,
        };
        let identity = self.identity_unreserved(sizing)?;
        let current_document = document.reference.clone();
        let record = encode_record(
            &body,
            self.reservation.role(),
            sizing,
            baseline.reference,
            document.reference,
            edges,
            identity.digest(),
        )?;
        let object = store.admit_record(&record).map_err(store_error)?;
        store
            .remember_owner(
                object.reference.clone(),
                crate::bundle_recovery::OwnerProof {
                    identity,
                    position,
                    sizing,
                    document: crate::bundle_recovery::ProjectionRef::new(&current_document)
                        .map_err(store_error)?,
                },
            )
            .map_err(store_error)?;

        Ok(SealedOwner {
            object,
            document: current_document,
            identity,
            role: self.reservation.role(),
            position,
        })
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
        return Ok(manifest_matches(resource, &manifest));
    }
    // Media keeps its manifest and chunk/profile dependencies, not just a
    // naked byte object. LUTs and the embedded font also admit direct blobs.
    Ok(!matches!(resource, Resource::Media { .. }) && blob_matches(resource, reference))
}
fn manifest_matches(
    resource: &Resource,
    manifest: &crate::owned_inputs::OwnedInputManifest,
) -> bool {
    manifest.entries.iter().any(|entry| match resource {
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
    })
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
// The native store and the portable session model share this codec and the
// entire semantic validator. The source supplies verified document objects and
// resource bindings; it cannot supply a validated history token.
trait SnapshotSource {
    fn limits(&self) -> &StorageLimits;
    fn document(&self, reference: &ObjectRef) -> Result<kinewright_core::Document, Error>;
    fn binding_matches(&self, resource: &Resource, reference: &ObjectRef) -> Result<bool, Error>;
}
impl SnapshotSource for ProjectStore {
    fn limits(&self) -> &StorageLimits {
        self.limits()
    }
    fn document(&self, reference: &ObjectRef) -> Result<kinewright_core::Document, Error> {
        self.read_record::<kinewright_core::Document>(reference)
            .map(crate::storage_budget::StorageReservation::take)
            .map_err(store_error)
    }
    fn binding_matches(&self, resource: &Resource, reference: &ObjectRef) -> Result<bool, Error> {
        binding_matches(self, resource, reference)
    }
}
struct NamespaceSource<'a> {
    namespace: &'a crate::namespace::ProjectNamespace,
    usage: &'a std::sync::Arc<std::sync::Mutex<crate::storage_budget::StorageUsage>>,
    limits: &'a StorageLimits,
}
impl NamespaceSource<'_> {
    fn read<T: BundleRecord>(&self, reference: &ObjectRef) -> Result<T, Error> {
        crate::bundle_recovery::read_retained_record::<T>(
            self.namespace,
            reference,
            self.usage,
            self.limits,
        )
        .map(crate::storage_budget::StorageReservation::take)
        .map_err(store_error)
    }
}
impl SnapshotSource for NamespaceSource<'_> {
    fn limits(&self) -> &StorageLimits {
        self.limits
    }
    fn document(&self, reference: &ObjectRef) -> Result<kinewright_core::Document, Error> {
        self.read(reference)
    }
    fn binding_matches(&self, resource: &Resource, reference: &ObjectRef) -> Result<bool, Error> {
        if reference.record_kind == RecordKind::InputManifest {
            return self
                .read(reference)
                .map(|manifest| manifest_matches(resource, &manifest));
        }
        Ok(!matches!(resource, Resource::Media { .. }) && blob_matches(resource, reference))
    }
}
impl StoredOwnerRecord {
    pub(crate) fn binding_count(&self) -> Result<u64, BundleError> {
        struct Count(u64);
        impl<'de> Deserialize<'de> for Count {
            fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                struct Visitor;
                impl<'de> serde::de::Visitor<'de> for Visitor {
                    type Value = Count;
                    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                        f.write_str("resource bindings")
                    }
                    fn visit_seq<A: serde::de::SeqAccess<'de>>(
                        self,
                        mut seq: A,
                    ) -> Result<Count, A::Error> {
                        let mut count = 0_u64;
                        while seq.next_element::<serde::de::IgnoredAny>()?.is_some() {
                            count = count
                                .checked_add(1)
                                .ok_or_else(|| serde::de::Error::custom("binding count"))?;
                        }
                        Ok(Count(count))
                    }
                }
                d.deserialize_seq(Visitor)
            }
        }
        #[derive(Deserialize)]
        struct View {
            bindings: Count,
        }
        let view: View =
            serde_json::from_str(self.body.get()).map_err(crate::bundle::encoding_error)?;
        if view.bindings.0
            > self
                .sizing
                .limits()
                .map_err(|_| BundleError::Capacity("creation sizing"))?
                .envelope
                .owners
                / 256
        {
            return Err(BundleError::Capacity("resource bindings"));
        }
        Ok(view.bindings.0)
    }
    /// The publication caller reserves the counted reference storage first.
    pub(crate) fn binding_refs(&self) -> Result<Vec<(Resource, ObjectRef)>, BundleError> {
        #[derive(Deserialize)]
        struct View {
            bindings: Vec<(Resource, ObjectRef)>,
        }
        let view: View =
            serde_json::from_str(self.body.get()).map_err(crate::bundle::encoding_error)?;
        Ok(view.bindings)
    }
}
impl StoredOwnerRecord {
    pub(crate) fn read_namespace(
        namespace: &crate::namespace::ProjectNamespace,
        checkpoint: &crate::operation_checkpoint::Checkpoint,
        pool: &HistoryPool,
        usage: &std::sync::Arc<std::sync::Mutex<crate::storage_budget::StorageUsage>>,
        limits: &StorageLimits,
    ) -> Result<ValidatedOwnerSnapshot, Error> {
        checkpoint.validate(limits).map_err(store_error)?;
        if checkpoint.owner_ref.byte_length > checkpoint.role.persisted_shell_cap() {
            return Err(Error::Capacity("persisted owner shell"));
        }
        let window = pool.workspace(checkpoint.role)?;
        let source = NamespaceSource {
            namespace,
            usage,
            limits,
        };
        let record: Self = source.read(&checkpoint.owner_ref)?;
        if record.role != checkpoint.role || record.sizing.kind.role() != checkpoint.role {
            return Err(Error::Reservation);
        }
        if record.document != checkpoint.document_ref
            || record.identity != digest_text(&checkpoint.identity.digest())
        {
            return Err(Error::IdentityMismatch);
        }
        let reservation = pool.admit_sized(record.sizing)?;
        let bounds = reservation.limits().bounds;
        let validated =
            record.resolve_reserved(&source, checkpoint.role, reservation, window, bounds)?;
        if validated.identity() != checkpoint.identity
            || validated.position() != checkpoint.position
        {
            return Err(Error::IdentityMismatch);
        }
        Ok(validated)
    }
    /// Reserve an owner and workspace before reading or decoding its shell.
    pub fn read(
        store: &ProjectStore,
        reference: &ObjectRef,
        pool: &HistoryPool,
        expected_role: Role,
    ) -> Result<ValidatedOwnerSnapshot, Error> {
        Self::read_expected(store, reference, pool, expected_role, None)
    }
    pub(crate) fn read_projected(
        store: &ProjectStore,
        reference: &ObjectRef,
        pool: &HistoryPool,
        expected_role: Role,
        document: &ObjectRef,
        identity: super::SnapshotIdentity,
    ) -> Result<ValidatedOwnerSnapshot, Error> {
        Self::read_expected(
            store,
            reference,
            pool,
            expected_role,
            Some((document, identity)),
        )
    }
    fn read_expected(
        store: &ProjectStore,
        reference: &ObjectRef,
        pool: &HistoryPool,
        expected_role: Role,
        expected: Option<(&ObjectRef, super::SnapshotIdentity)>,
    ) -> Result<ValidatedOwnerSnapshot, Error> {
        // Bound raw ingress by the requested role before the store allocates
        // its encoded/decoded shell, including a record claiming another role.
        if reference.byte_length > expected_role.persisted_shell_cap() {
            return Err(Error::Capacity("persisted owner shell"));
        }
        let window = pool.workspace(expected_role)?;
        let record = store
            .read_record::<Self>(reference)
            .map_err(store_error)?
            .take();
        if record.role != expected_role || record.sizing.kind.role() != expected_role {
            return Err(Error::Reservation);
        }
        if expected.is_some_and(|(document, identity)| {
            record.document != *document || record.identity != digest_text(&identity.digest())
        }) {
            return Err(Error::IdentityMismatch);
        }
        let reservation = pool.admit_sized(record.sizing)?;
        let bounds = reservation.limits().bounds;
        let projection =
            crate::bundle_recovery::ProjectionRef::new(&record.document).map_err(store_error)?;
        let validated =
            record.resolve_reserved(store, expected_role, reservation, window, bounds)?;
        if expected.is_some_and(|(_, identity)| validated.identity() != identity) {
            return Err(Error::IdentityMismatch);
        }
        // Every cold query proved the same full owner invariants as open.
        // Preserve that fixed proof for later publication; retaining its raw
        // transport must not force a second reader alongside a private update.
        store
            .remember_owner(
                reference.clone(),
                crate::bundle_recovery::OwnerProof {
                    identity: validated.identity(),
                    position: validated.position(),
                    sizing: validated.creation_sizing(),
                    document: projection,
                },
            )
            .map_err(store_error)?;
        Ok(validated)
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
        let reservation = pool.admit_sized(self.sizing)?;
        let bounds = reservation.limits().bounds;
        let window = reservation.workspace()?;
        self.resolve_reserved(store, expected_role, reservation, window, bounds)
    }
    #[expect(
        clippy::too_many_lines,
        reason = "one bounded decode validates graph, owner semantics and identity before returning the install token"
    )]
    fn resolve_reserved(
        self,
        store: &impl SnapshotSource,
        expected_role: Role,
        reservation: super::HistoryReservation,
        window: super::Workspace,
        bounds: kinewright_core::undo::RoleBounds,
    ) -> Result<ValidatedOwnerSnapshot, Error> {
        if self.role != expected_role {
            return Err(Error::Reservation);
        }
        if reservation.sizing() != self.sizing {
            return Err(Error::InvalidRecord);
        }
        self.validate(store.limits()).map_err(store_error)?;
        let typed = kinewright_core::undo::payload_memory_bound(self.body.get().as_bytes())
            .map_err(|_| Error::Capacity("owner decode"))?;
        if typed
            .checked_add(self.body.get().len() as u64)
            .is_none_or(|n| n > reservation.limits().envelope.workspace)
        {
            return Err(Error::Capacity("owner decode"));
        }
        #[cfg(any(test, feature = "test-util"))]
        crate::io_observer::note_history_decode();
        let body = self.decode_body()?;
        let baseline = store.document(&self.baseline)?;
        if kinewright_core::undo::document_capacity_bytes(&baseline)? > bounds.document_live_bytes {
            return Err(Error::History(
                kinewright_core::undo::Error::DocumentCapacity,
            ));
        }
        // Check each decoded object before loading the next. Record validation
        // installs the baseline's canonical allocation shape once, after this
        // raw ingress has released its store body.
        let document = store.document(&self.document)?;
        if kinewright_core::undo::document_capacity_bytes(&document)? > bounds.document_live_bytes {
            return Err(Error::History(
                kinewright_core::undo::Error::DocumentCapacity,
            ));
        }
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
            if !store.binding_matches(resource, reference)? {
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
            baseline: Arc::new(baseline),
            document: Arc::new(document),
            undo,
            redo,
            position: body.position,
            epoch: body.epoch,
            policy: body.policy,
        };
        let data = SnapshotData {
            role: self.role,
            sizing: self.sizing,
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
        let Self {
            identity,
            body: raw_body,
            edges,
            baseline: baseline_edge,
            document: document_edge,
            ..
        } = self;
        drop((raw_body, edges, baseline_edge, document_edge));
        drop(window);
        let validated = OwnerSnapshot { data, reservation }.validate_inner(bounds)?;
        if digest_text(&validated.identity().digest()) != identity {
            return Err(Error::IdentityMismatch);
        }
        Ok(validated)
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
        let limits = self
            .sizing
            .limits()
            .map_err(|_| BundleError::Capacity("creation sizing"))?;
        if self.sizing.kind.role() != self.role {
            return Err(BundleError::Corrupt("creation sizing role".into()));
        }
        if self.baseline.byte_length > self.role.bounds().document_bytes
            || self.document.byte_length > self.role.bounds().document_bytes
        {
            return Err(BundleError::Capacity("owner document edge"));
        }
        if self.body.get().len() as u64 > limits.body_cap() {
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
        sizing: super::CreationSizing,
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
    let limits = header
        .sizing
        .limits()
        .map_err(|_| BundleError::Capacity("creation sizing"))?;
    if header.sizing.kind.role() != header.role {
        return Err(BundleError::Corrupt("creation sizing role".into()));
    }
    if bytes.len() as u64 > limits.shell_cap() {
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
pub(super) fn payload_charge(value: &impl Serialize, limit: u64) -> Result<(u64, u64), Error> {
    struct Limited {
        bytes: Vec<u8>,
        limit: u64,
    }
    impl std::io::Write for Limited {
        fn write(&mut self, value: &[u8]) -> std::io::Result<usize> {
            if (self.bytes.len() as u64)
                .checked_add(value.len() as u64)
                .is_none_or(|n| n > self.limit)
            {
                return Err(std::io::Error::other("wire capacity"));
            }
            self.bytes.extend_from_slice(value);
            Ok(value.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    // A Vec's geometric spare capacity is at most twice the admitted input.
    // limit is W/4, leaving W/2 for this temporary encoding; it is dropped
    // before Core's candidate or a retained ancestry payload is allocated.
    let mut out = Limited {
        bytes: Vec::new(),
        limit,
    };
    super::owner::payload_encodings(1);
    serde_json::to_writer(&mut out, value).map_err(|_| Error::Capacity("owner decode"))?;
    let encoded = out.bytes.len() as u64;
    super::owner::payload_serialized(encoded);
    let decoded = kinewright_core::undo::payload_memory_bound(&out.bytes)?
        .checked_add(encoded)
        .ok_or(Error::Capacity("owner decode"))?;
    Ok((encoded, decoded))
}
fn payload_decode_charge(value: &impl Serialize, limit: u64) -> Result<u64, Error> {
    payload_charge(value, limit).map(|(_, decoded)| decoded)
}
pub(super) fn baseline_decode_charge(
    lineage: &super::Lineage,
    resources: usize,
    workspace: u64,
) -> Result<u64, Error> {
    // NeverFold roles measure at construction/open. Folding roles keep cached
    // point charges instead; commands never encode the full seed map.
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
pub(super) mod codec_tests;

#[cfg(test)]
pub(super) fn assert_decode_forecast(record: &StoredOwnerRecord, bound: u64) {
    let bytes = record.body.get().as_bytes();
    let actual = kinewright_core::undo::payload_memory_bound(bytes).unwrap() + bytes.len() as u64;
    assert!(
        actual <= bound,
        "EVERY_SAVED_OWNER_DECODE_IS_RESERVED actual={actual} bound={bound}"
    );
}

#[cfg(test)]
#[path = "session_model.rs"]
mod session_model;
