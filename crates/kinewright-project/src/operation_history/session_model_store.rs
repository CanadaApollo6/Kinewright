use super::*;

pub(super) trait ModelStore: Default {
    fn save(&mut self, snapshot: &OwnerSnapshot) -> ObjectRef;
    fn load(&self, reference: &ObjectRef, pool: &HistoryPool) -> ValidatedOwnerSnapshot;
    fn publish(&mut self, root: &Root);
    fn root(&self) -> Root;
    fn cold_root(&self) -> Root {
        // Portable cold reads exercise the same root codec as the native
        // file. Warm model reads need only the committed value.
        serde_json::from_slice(&serde_json::to_vec(&self.root()).unwrap()).unwrap()
    }
    fn blob(&mut self, bytes: &[u8], kind: RecordKind) -> ObjectRef;
    fn copy_to_new(&self, roots: &[ObjectRef]) -> Self;
    fn collect(&mut self, roots: &[ObjectRef]);
    fn assert_exists(&self, reference: &ObjectRef);
    fn assert_absent(&self, reference: &ObjectRef);
    fn resource(&self, resource: &Resource) -> ObjectRef {
        match resource {
            Resource::Lut { sha256, byte_len } => ObjectRef {
                sha256: sha256.clone(),
                byte_length: *byte_len,
                record_kind: RecordKind::OwnedLut,
            },
            Resource::TitleFont => {
                reference(kinewright_core::title_font_bytes(), RecordKind::OwnedFont)
            }
            Resource::Media { .. } => unreachable!("the portable owned-byte stream uses LUTs"),
        }
    }
}
fn reference(bytes: &[u8], record_kind: RecordKind) -> ObjectRef {
    ObjectRef {
        sha256: kinewright_media::sha256_bytes(bytes),
        byte_length: bytes.len() as u64,
        record_kind,
    }
}
#[derive(Default)]
pub(super) struct MemoryStore {
    limits: StorageLimits,
    objects: BTreeMap<ObjectRef, (Vec<u8>, Vec<ObjectRef>)>,
    root: Option<Root>,
}
impl MemoryStore {
    fn put(&mut self, bytes: Vec<u8>, kind: RecordKind, dependencies: Vec<ObjectRef>) -> ObjectRef {
        let key = reference(&bytes, kind);
        self.objects.insert(key.clone(), (bytes, dependencies));
        key
    }
    fn bytes(&self, reference: &ObjectRef) -> &[u8] {
        let bytes = &self
            .objects
            .get(reference)
            .expect("REACHABLE_OBJECT_SURVIVES_COLLECTION")
            .0;
        assert_eq!(
            &self::reference(bytes, reference.record_kind),
            reference,
            "MODEL_STORE_VERIFIES_CONTENT_IDENTITY"
        );
        bytes
    }
}
impl SnapshotSource for MemoryStore {
    fn limits(&self) -> &StorageLimits {
        &self.limits
    }
    fn document(&self, reference: &ObjectRef) -> Result<kinewright_core::Document, Error> {
        if reference.record_kind != RecordKind::Document {
            return Err(Error::InvalidRecord);
        }
        serde_json::from_slice(self.bytes(reference)).map_err(|_| Error::InvalidRecord)
    }
    fn binding_matches(&self, resource: &Resource, reference: &ObjectRef) -> Result<bool, Error> {
        let _ = self.bytes(reference);
        Ok(blob_matches(resource, reference))
    }
}
impl ModelStore for MemoryStore {
    fn save(&mut self, snapshot: &OwnerSnapshot) -> ObjectRef {
        let _window = snapshot.reservation.workspace().unwrap();
        let baseline = self.put(
            serde_json::to_vec(&snapshot.data.history.baseline).unwrap(),
            RecordKind::Document,
            vec![],
        );
        let document = self.put(
            serde_json::to_vec(&snapshot.data.history.document).unwrap(),
            RecordKind::Document,
            vec![],
        );
        let bindings: Vec<_> = snapshot
            .data
            .resources
            .iter()
            .map(|r| {
                let object = self.resource(r);
                self.assert_exists(&object);
                (r.clone(), object)
            })
            .collect();
        let record = snapshot
            .stored_record(baseline, document, &bindings)
            .unwrap();
        self.put(
            serde_json::to_vec(&record).unwrap(),
            RecordKind::OperationHistory,
            record.dependencies(),
        )
    }
    fn load(&self, reference: &ObjectRef, pool: &HistoryPool) -> ValidatedOwnerSnapshot {
        assert!(reference.byte_length <= Role::Ordinary.persisted_shell_cap());
        let window = pool.workspace(Role::Ordinary).unwrap();
        let record: StoredOwnerRecord = serde_json::from_slice(self.bytes(reference)).unwrap();
        let reservation = pool.admit_sized(record.sizing).unwrap();
        let identity: [u8; 64] = record.identity.as_bytes().try_into().unwrap();
        let validated = record
            .resolve_reserved(self, Role::Ordinary, reservation, window, bounds())
            .expect("SESSION_SAVED_RECORD_CAN_BE_OPENED");
        assert_eq!(
            digest_text(&validated.identity().digest()).as_bytes(),
            &identity,
            "SESSION_ACCEPTED_INPUT_PRESERVES_ITS_SAVED_IDENTITY"
        );
        validated
    }
    fn publish(&mut self, root: &Root) {
        // This single replacement is the model's publication linearisation
        // point. History, epoch/sequence/tip and receipt index move together.
        self.root = Some(root.clone());
    }
    fn root(&self) -> Root {
        self.root
            .as_ref()
            .expect("INITIAL_ROOT_WAS_PUBLISHED")
            .clone()
    }
    fn blob(&mut self, bytes: &[u8], kind: RecordKind) -> ObjectRef {
        self.put(Vec::from(bytes), kind, vec![])
    }
    fn copy_to_new(&self, roots: &[ObjectRef]) -> Self {
        let mut copied = Self::default();
        let mut pending = Vec::from(roots);
        while let Some(key) = pending.pop() {
            if !copied.objects.contains_key(&key) {
                let (bytes, edges) = &self.objects[&key];
                // Independent destination storage survives source teardown.
                copied.put(
                    Vec::from(bytes.as_slice()),
                    key.record_kind,
                    Vec::from(edges.as_slice()),
                );
                pending.extend(edges.iter().cloned());
            }
        }
        copied.publish(&self.cold_root());
        copied
    }
    fn collect(&mut self, roots: &[ObjectRef]) {
        let mut reachable = BTreeSet::new();
        let mut pending = Vec::from(roots);
        while let Some(key) = pending.pop() {
            if reachable.insert(key.clone()) {
                pending.extend(
                    self.objects
                        .get(&key)
                        .expect("GC_ROOT_AND_ALL_EDGES_EXIST")
                        .1
                        .iter()
                        .cloned(),
                );
            }
        }
        self.objects.retain(|key, _| reachable.contains(key));
    }
    fn assert_exists(&self, reference: &ObjectRef) {
        let _ = self.bytes(reference);
    }
    fn assert_absent(&self, reference: &ObjectRef) {
        assert!(
            !self.objects.contains_key(reference),
            "UNREACHABLE_PAYLOAD_IS_COLLECTABLE_AFTER_DRAIN"
        );
    }
}

#[cfg(target_os = "linux")]
pub(super) struct NativeStore {
    admitted: Vec<SealedObject>,
    pin: Option<crate::store::StorePin>,
    session: crate::bundle_copy::SessionStore,
    temp: kinewright_media::test_support::TempDirectory,
}
#[cfg(target_os = "linux")]
impl Default for NativeStore {
    fn default() -> Self {
        let temp = kinewright_media::test_support::TempDirectory::new_in(
            &kinewright_media::test_support::workspace_target_dir(),
            "hx-session-model",
        );
        let session =
            crate::bundle_copy::SessionStore::create(temp.root(), StorageLimits::default())
                .unwrap();
        Self {
            temp,
            session,
            admitted: vec![],
            pin: None,
        }
    }
}
#[cfg(target_os = "linux")]
impl ModelStore for NativeStore {
    fn save(&mut self, snapshot: &OwnerSnapshot) -> ObjectRef {
        let object = snapshot
            .persist(self.session.store_mut(), |r| {
                Ok(Some(match r {
                    Resource::Lut { sha256, byte_len } => ObjectRef {
                        sha256: sha256.clone(),
                        byte_length: *byte_len,
                        record_kind: RecordKind::OwnedLut,
                    },
                    _ => unreachable!("the durable stream uses LUT bindings"),
                }))
            })
            .unwrap();
        let reference = object.reference.clone();
        self.admitted.push(object);
        reference
    }
    fn load(&self, reference: &ObjectRef, pool: &HistoryPool) -> ValidatedOwnerSnapshot {
        // Native ingress retains the reserve-before-read rule. Only the
        // test's lower executor cap differs from StoredOwnerRecord::read.
        assert!(reference.byte_length <= Role::Ordinary.persisted_shell_cap());
        let window = pool.workspace(Role::Ordinary).unwrap();
        let record = self
            .session
            .store()
            .read_record::<StoredOwnerRecord>(reference)
            .unwrap()
            .take();
        let reservation = pool.admit_sized(record.sizing).unwrap();
        record
            .resolve_reserved(
                self.session.store(),
                Role::Ordinary,
                reservation,
                window,
                bounds(),
            )
            .expect("DURABLE_SESSION_SAVED_RECORD_CAN_BE_OPENED")
    }
    fn publish(&mut self, root: &Root) {
        use std::io::Write as _;
        let temporary = self.temp.root().join("session-root.next");
        let mut file = std::fs::File::create(&temporary).unwrap();
        file.write_all(&serde_json::to_vec(root).unwrap()).unwrap();
        file.sync_all().unwrap();
        std::fs::rename(temporary, self.temp.root().join("session-root")).unwrap();
        std::fs::File::open(self.temp.root())
            .unwrap()
            .sync_all()
            .unwrap();
    }
    fn root(&self) -> Root {
        serde_json::from_slice(&std::fs::read(self.temp.root().join("session-root")).unwrap())
            .unwrap()
    }
    fn blob(&mut self, bytes: &[u8], kind: RecordKind) -> ObjectRef {
        let reference = reference(bytes, kind);
        let mut input = bytes;
        self.admitted.push(
            self.session
                .store_mut()
                .admit_blob(&mut input, &reference, &[])
                .unwrap(),
        );
        reference
    }
    fn copy_to_new(&self, roots: &[ObjectRef]) -> Self {
        let mut copied = Self::default();
        let bytes = self
            .session
            .store()
            .closure(roots)
            .unwrap()
            .iter()
            .map(|r| r.byte_length)
            .sum();
        let _source_hold = self.session.store().begin_copy(roots, bytes).unwrap();
        copied
            .session
            .store_mut()
            .copy_graph_from(self.session.store(), roots)
            .unwrap();
        copied.pin = Some(copied.session.store().pin(roots).unwrap());
        copied.publish(&self.root());
        copied
    }
    fn collect(&mut self, roots: &[ObjectRef]) {
        let pin = self.session.store().pin(roots).unwrap();
        self.admitted.clear();
        self.pin = Some(pin);
        self.session.store_mut().collect_unpublished().unwrap();
    }
    fn assert_exists(&self, reference: &ObjectRef) {
        assert!(
            self.session
                .store()
                .closure(std::slice::from_ref(reference))
                .is_ok(),
            "DURABLE_REACHABLE_OBJECT_SURVIVES_COLLECTION"
        );
    }
    fn assert_absent(&self, reference: &ObjectRef) {
        assert!(
            self.session
                .store()
                .closure(std::slice::from_ref(reference))
                .is_err(),
            "DURABLE_UNREACHABLE_PAYLOAD_IS_COLLECTABLE_AFTER_DRAIN"
        );
    }
}
