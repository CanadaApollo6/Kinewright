//! Native bounded owned-byte admission and last-reader retirement, without a model store.
use super::{
    protocol::{InputArgs, LoadedFixture},
    revised::Adapter,
};
use kinewright_project::{
    bundle::BundleError,
    bundle_copy::SessionStore,
    bundle_records::{ObjectRef, RecordKind},
    owned_inputs::InputRequest,
    storage_budget::StorageLimits,
};
use serde_json::{Value, json};
use std::io::{Cursor, Read};
impl Adapter {
    pub(super) fn budget_probe(
        &self,
        args: &InputArgs,
        fixture: &LoadedFixture,
    ) -> Result<Value, String> {
        let error = |e: BundleError| e.to_string();
        let bytes = fixture
            .files
            .get(&args.input)
            .ok_or("missing budget input")?;
        let limits = StorageLimits {
            owned_bytes: bytes.len() as u64,
            ..StorageLimits::default()
        };
        let mut session = SessionStore::create(&self.baseline.root, limits).map_err(error)?;
        let request = InputRequest {
            project_id: None,
            session_id: Some(session.session_id().clone()),
            owner_epoch: session.store().owner_epoch(),
            job: "bounded-native-fixture".into(),
            object: ObjectRef {
                sha256: kinewright_media::sha256_bytes(bytes),
                byte_length: bytes.len() as u64,
                record_kind: RecordKind::OwnedData,
            },
            source_locator_hint: None,
            dependencies: vec![],
            selected_stream: None,
            time_base: None,
            origin: None,
            profile: None,
        };
        let (input, sealed) = session
            .store_mut()
            .admit_input(&request, &mut Cursor::new(bytes))
            .map_err(error)?;
        let monitor = session.store().usage_handle();
        let boundary = monitor().map_err(error)?;
        let mut reader = session.store().lease_input(&input).map_err(error)?;
        let mut excess = bytes.clone();
        excess.push(0);
        let mut next = request.clone();
        next.object.sha256 = kinewright_media::sha256_bytes(&excess);
        next.object.byte_length = excess.len() as u64;
        let refused = matches!(
            session
                .store_mut()
                .admit_input(&next, &mut Cursor::new(&excess)),
            Err(BundleError::Capacity(_))
        );
        let after_refusal = monitor().map_err(error)?;
        drop(sealed);
        drop(session);
        let held = monitor().map_err(error)?;
        let mut actual = vec![];
        reader.read_to_end(&mut actual).map_err(|e| e.to_string())?;
        drop(reader);
        let retired = monitor().map_err(error)?;
        let again = monitor().map_err(error)?;
        Ok(
            json!({"backend":"linux-local-storage-budget","stored_bytes_equal":actual==*bytes,"boundary_bytes_equal":boundary.owned_bytes==bytes.len() as u64,"quota_beyond_refused":refused&&after_refusal.physical_bytes==boundary.physical_bytes,"live_reader_retained":held.physical_bytes==boundary.physical_bytes&&held.physical_bytes>0,"retired_exactly_once":retired.physical_bytes==0&&again.physical_bytes==0&&retired.objects==0,"accounting_sound":!retired.damaged_accounting&&retired.reserved_bytes==0,"boundary_owned_bytes":boundary.owned_bytes,"boundary_physical_bytes":boundary.physical_bytes,"held_physical_bytes":held.physical_bytes,"retired_physical_bytes":retired.physical_bytes}),
        )
    }
}
