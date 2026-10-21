//! Mode routing only; successor actions call production owners as they land.
#![allow(clippy::missing_errors_doc)]
use super::{
    baseline::Baseline,
    protocol::{Call, LoadedFixture, Mode},
};
use kinewright_project::{io_observer, project};
use serde_json::{Value, json};
use std::{fs, path::Path};

pub struct Adapter {
    pub baseline: Baseline,
    mode: Mode,
}

impl Adapter {
    pub fn new(root: &Path, mode: Mode) -> Result<Self, String> {
        Ok(Self {
            baseline: Baseline::new(root)?,
            mode,
        })
    }

    pub fn finish(&mut self) -> Result<Value, String> {
        self.baseline.finish()
    }

    pub fn execute(&mut self, call: &Call, fixture: &LoadedFixture) -> Result<Value, String> {
        match call {
            Call::OwnedInputProbe(args) => {
                #[cfg(target_os = "linux")]
                {
                    if self.mode != Mode::Revised {
                        return Err("owned store is unavailable in baseline mode; NOT RUN".into());
                    }
                    self.owned_probe(args, fixture)
                }
                #[cfg(not(target_os = "linux"))]
                {
                    let _ = args;
                    Err("owned store Linux prerequisite unavailable; NOT RUN".into())
                }
            }
            Call::AdapterStatus(_) => {
                Ok(json!({"mode":self.mode,"backend":"portable-model","pid":std::process::id()}))
            }
            Call::StrictSave(args) | Call::StrictRenameRefusal(args) => {
                let bytes = fixture
                    .files
                    .get(&args.input)
                    .ok_or("missing IO probe input")?;
                let path = self.baseline.root.join(&args.path);
                let before = fs::read(&path).ok();
                let refused = matches!(call, Call::StrictRenameRefusal(_));
                let (result, trace) = io_observer::observe(|| {
                    if self.mode == Mode::Revised {
                        let mut namespace =
                            kinewright_project::namespace::ProjectNamespace::acquire(&path)
                                .map_err(|e| std::io::Error::other(e.to_string()))?;
                        let result = if refused {
                            namespace.replace_root_refusing_rename(bytes)
                        } else {
                            namespace.replace_root(bytes)
                        };
                        let released = namespace.release();
                        result
                            .and(released)
                            .map_err(|e| std::io::Error::other(e.to_string()))
                    } else if refused {
                        project::observe_refused_rename(&path, bytes)
                    } else {
                        project::write_file_atomic(&path, bytes)
                    }
                });
                let after = fs::read(&path).map_err(|e| e.to_string())?;
                let attempted_sha = kinewright_media::sha256_bytes(bytes);
                let hook = trace
                    .events
                    .iter()
                    .find(|e| e.operation == "rename_refusal_hook");
                Ok(
                    json!({"backend":if self.mode == Mode::Revised { "linux-local-strict" } else { "legacy-native-io" },"succeeded":result.is_ok(),"writer_bytes_equal":after == *bytes,
                    "root_preserved":before.as_ref() == Some(&after),
                    "refusal_reached":hook.is_some(),
                    "refusal_injected":trace.events.iter().any(|e| e.operation == "rename_refusal_injected"),
                    "attempted_bytes_equal":hook.is_some_and(|e| e.attempted_sha256.as_ref() == Some(&attempted_sha) && e.attempted_length == Some(bytes.len())),
                    "directory_sync_observed":trace.events.iter().any(|e| e.operation == "directory_sync"),
                    "in_place_fallback_observed":trace.events.iter().any(|e| e.operation == "in_place_fallback"),"io":trace}),
                )
            }
            _ => self.baseline.execute(call, fixture),
        }
    }
    #[cfg(target_os = "linux")]
    fn owned_probe(
        &self,
        args: &super::protocol::InputArgs,
        fixture: &LoadedFixture,
    ) -> Result<Value, String> {
        use kinewright_project::{
            bundle_copy::SessionStore,
            bundle_records::{ObjectRef, RecordKind},
            owned_inputs::InputRequest,
            storage_budget::StorageLimits,
        };
        use std::io::Read;
        let bytes = fixture
            .files
            .get(&args.input)
            .ok_or("missing owned-input bytes")?;
        let expected = fixture
            .fixture
            .inputs
            .get(&args.input)
            .ok_or("missing owned-input identity")?;
        let mut session = SessionStore::create(&self.baseline.root, StorageLimits::default())
            .map_err(|e| e.to_string())?;
        let store = session.store();
        let request = InputRequest {
            project_id: store.input_owner().0,
            session_id: store.input_owner().1,
            owner_epoch: store.owner_epoch(),
            job: "owned-fixture".into(),
            object: ObjectRef {
                sha256: expected.sha256.clone(),
                byte_length: expected.byte_length,
                record_kind: RecordKind::OwnedEvidence,
            },
            source_locator_hint: Some(self.baseline.root.join("locator-does-not-exist")),
            dependencies: vec![],
            selected_stream: None,
            time_base: None,
            origin: None,
            profile: None,
        };
        let (reference, sealed) = session
            .store_mut()
            .admit_input(&request, &mut bytes.as_slice())
            .map_err(|e| e.to_string())?;
        let measured = expected
            .byte_length
            .checked_add(reference.manifest_ref.byte_length)
            .ok_or("measurement overflow")?;
        let mut reader = session
            .store()
            .lease_input(&reference)
            .map_err(|e| e.to_string())?;
        let decoder_unavailable = reader.certify_decoder_open().is_err();
        let monitor = session.store().usage_handle();
        let before = monitor().map_err(|e| e.to_string())?;
        drop(sealed);
        drop(session);
        let retained = monitor().map_err(|e| e.to_string())?;
        let mut actual = Vec::new();
        reader.read_to_end(&mut actual).map_err(|e| e.to_string())?;
        drop(reader);
        let after = monitor().map_err(|e| e.to_string())?;
        Ok(
            json!({"backend":"linux-local-owned-store", "stored_bytes_equal":actual == *bytes,
            "retained_until_last_reader":before.physical_bytes == measured && retained.physical_bytes == measured,
            "charges_retired_once":after.physical_bytes == 0 && after.objects == 0 && after.reserved_bytes == 0
                && after.decoded_bytes == 0 && !after.damaged_accounting && !after.unsettled,
            "decoder_certification_unavailable":decoder_unavailable,
            "measured_physical_payload_bytes":measured,"peak_decoded_admission_charge":before.peak_decoded_bytes}),
        )
    }
}
