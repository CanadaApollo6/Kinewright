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
    pub(super) bundle: Option<kinewright_project::bundle::ProjectBundle>,
}

impl Adapter {
    pub fn new(root: &Path, mode: Mode) -> Result<Self, String> {
        Ok(Self {
            baseline: Baseline::new(root)?,
            mode,
            bundle: None,
        })
    }

    pub fn finish(&mut self) -> Result<Value, String> {
        self.bundle = None;
        self.baseline.finish()
    }

    #[allow(clippy::too_many_lines, reason = "exhaustive adapter dispatch")]
    pub fn execute(
        &mut self,
        action: &super::protocol::Action,
        fixture: &LoadedFixture,
    ) -> Result<Value, String> {
        let call = &action.call;
        match call {
            Call::S2SavePhaseProbe(_) => {
                #[cfg(target_os = "linux")]
                {
                    super::s2_observer::save_phase_cleanup(&self.baseline.root)
                }
                #[cfg(not(target_os = "linux"))]
                {
                    Err("C1 durable phase observer requires Linux; NOT RUN".into())
                }
            }
            Call::CoreSend(args)
                if self.mode == Mode::Revised
                    && matches!(
                        fixture.fixture.fixture_id.as_str(),
                        "reply-retry" | "s1-edit-receipt-restart"
                    ) =>
            {
                #[cfg(target_os = "linux")]
                {
                    self.receipt_edit(action, args, fixture)
                }
                #[cfg(not(target_os = "linux"))]
                {
                    let _ = args;
                    Err("Linux native durable command prerequisite unavailable; NOT RUN".into())
                }
            }
            Call::CommentSubmit(args) => {
                #[cfg(target_os = "linux")]
                {
                    if self.mode == Mode::Revised {
                        self.comment_receipt_submit(args, fixture)
                    } else {
                        self.comment_graph_submit(args, fixture)
                    }
                }
                #[cfg(not(target_os = "linux"))]
                {
                    let _ = args;
                    Err("comment graph Linux prerequisite unavailable; NOT RUN".into())
                }
            }
            Call::CommentInspect(args) => {
                #[cfg(target_os = "linux")]
                {
                    if self.mode == Mode::Revised {
                        self.comment_receipt_inspect(args, fixture)
                    } else {
                        self.comment_graph_inspect(args, fixture)
                    }
                }
                #[cfg(not(target_os = "linux"))]
                {
                    let _ = args;
                    Err("comment graph Linux prerequisite unavailable; NOT RUN".into())
                }
            }
            Call::CommentRetire(args) | Call::CommentRotate(args) => {
                #[cfg(target_os = "linux")]
                {
                    if self.mode == Mode::Revised {
                        self.comment_receipt_maintenance(
                            args,
                            matches!(call, Call::CommentRotate(_)),
                        )
                    } else {
                        Err("later receipt scenario unavailable on the pre-receipt observer; NOT RUN".into())
                    }
                }
                #[cfg(not(target_os = "linux"))]
                {
                    let _ = args;
                    Err("Linux native receipt prerequisite unavailable; NOT RUN".into())
                }
            }

            Call::BundleFixtureProbe(args) => {
                #[cfg(target_os = "linux")]
                {
                    for (role, reference) in &fixture.fixture.inputs {
                        if matches!(
                            reference.kind.as_str(),
                            "v3-bundle-root" | "v3-bundle-object"
                        ) {
                            let path = self.baseline.root.join(&reference.relative_file);
                            fs::create_dir_all(path.parent().ok_or("bundle sample has no parent")?)
                                .map_err(|e| e.to_string())?;
                            fs::write(
                                &path,
                                fixture
                                    .files
                                    .get(role)
                                    .ok_or("missing captured graph object")?,
                            )
                            .map_err(|e| e.to_string())?;
                        }
                    }
                    let reference = fixture
                        .fixture
                        .inputs
                        .get(&args.input)
                        .ok_or("missing sample root")?;
                    if reference.kind != "v3-bundle-root" {
                        return Err("sample root role kind mismatch".into());
                    }
                    let path = self.baseline.root.join(&reference.relative_file);
                    let (result, trace) = io_observer::observe(|| {
                        kinewright_project::bundle::LoadedBundle::read(&path)
                    });
                    let loaded = result.map_err(|e| e.to_string())?;
                    Ok(
                        json!({"backend":"linux-local-bundle-reader","graph_verified":true,
                        "read_only_admission":trace.events.is_empty(),"generation":loaded.root().map_err(|e|e.to_string())?.generation}),
                    )
                }
                #[cfg(not(target_os = "linux"))]
                {
                    let _ = args;
                    Err("native bundle reader prerequisite unavailable; NOT RUN".into())
                }
            }

            Call::ProjectSave(args) if self.mode == Mode::Revised => {
                let path = self.baseline.root.join(&args.path);
                let core = self.baseline.core()?.clone();
                if self.bundle.is_none() {
                    let admission = if path.is_file() {
                        kinewright_project::bundle::BundleWriteAdmission::read(&path)
                            .map_err(|e| e.to_string())?
                    } else {
                        kinewright_project::bundle::BundleWriteAdmission::new_document()
                    };
                    self.bundle = Some(
                        kinewright_project::bundle::ProjectBundle::activate(&path, admission)
                            .map_err(|e| e.to_string())?,
                    );
                }
                let bundle = self.bundle.as_mut().ok_or("missing bundle owner")?;
                if bundle.store().resolved_root().map_err(|e| e.to_string())?
                    != kinewright_project::canonical_project_identity(&path)
                        .map_err(|e| e.to_string())?
                {
                    return Err("complete bundle Save As belongs to copy admission".into());
                }
                let inputs = bundle.empty_inputs().map_err(|e| e.to_string())?;
                let (result, trace) =
                    io_observer::observe(|| bundle.save(&core, &inputs.reference));
                let result = result.map_err(|e| e.to_string())?;
                let expected = bundle.serialized_root().map_err(|e| e.to_string())?;
                let actual = fs::read(&path).map_err(|e| e.to_string())?;
                Ok(
                    json!({"backend":"linux-local-bundle","writer_bytes_equal":actual==*expected,
                    "root_sha256":result.root_sha256,"generation":result.generation,
                    "file":super::baseline::file_value(&path)?,"directory_sync_observed":trace.events.iter().any(|e|e.operation=="directory_sync"),"io":trace}),
                )
            }
            Call::ProjectReopen(args) if self.mode == Mode::Revised => {
                let path = self.baseline.root.join(&args.path);
                let before = self
                    .baseline
                    .core
                    .as_ref()
                    .map(|_| self.baseline.snapshot().map(|(_, document)| document))
                    .transpose()?;
                self.bundle = None;
                let loaded = kinewright_project::bundle::LoadedBundle::read(&path)
                    .map_err(|e| e.to_string())?;
                let core = kinewright_core::Core::spawn(kinewright_core::Document::default())
                    .map_err(|e| e.to_string())?;
                loaded.restore_manual(&core).map_err(|e| e.to_string())?;
                self.baseline.core = Some(core);
                self.bundle = Some(loaded.into_writer().map_err(|e| e.to_string())?);
                let (_, after) = self.baseline.snapshot()?;
                Ok(
                    json!({"backend":"linux-local-bundle","same_document":before.is_some_and(|document|document==after),"history":self.baseline.history()?}),
                )
            }

            Call::BundleCopyProbe(args) => {
                #[cfg(target_os = "linux")]
                {
                    if self.mode != Mode::Revised {
                        return Err(
                            "complete first-save copy unavailable in baseline mode; NOT RUN".into(),
                        );
                    }
                    self.copy_probe(args, fixture)
                }
                #[cfg(not(target_os = "linux"))]
                {
                    let _ = args;
                    Err("Linux copy prerequisite unavailable; NOT RUN".into())
                }
            }
            Call::BundleBudgetProbe(args) => {
                #[cfg(target_os = "linux")]
                {
                    if self.mode != Mode::Revised {
                        return Err("bounded store unavailable in baseline mode; NOT RUN".into());
                    }
                    self.budget_probe(args, fixture)
                }
                #[cfg(not(target_os = "linux"))]
                {
                    let _ = args;
                    Err("Linux storage budget prerequisite unavailable; NOT RUN".into())
                }
            }
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
                            namespace.replace_root_probe(bytes)
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
    pub(super) fn open_comment_bundle(&mut self, path: &Path) -> Result<(), String> {
        if let Some(bundle) = self.bundle.as_ref() {
            if bundle.store().resolved_root().map_err(|e| e.to_string())?
                != kinewright_project::canonical_project_identity(path)
                    .map_err(|e| e.to_string())?
            {
                return Err("comment owner path changed; copy admission required".into());
            }
            return Ok(());
        }
        if path.is_file() {
            let loaded =
                kinewright_project::bundle::LoadedBundle::read(path).map_err(|e| e.to_string())?;
            let core = kinewright_core::Core::spawn(kinewright_core::Document::default())
                .map_err(|e| e.to_string())?;
            loaded.restore_manual(&core).map_err(|e| e.to_string())?;
            self.baseline.core = Some(core);
            self.bundle = Some(loaded.into_writer().map_err(|e| e.to_string())?);
        } else {
            if self.baseline.core.is_none() {
                self.baseline.core = Some(
                    kinewright_core::Core::spawn(kinewright_core::Document::default())
                        .map_err(|e| e.to_string())?,
                );
            }
            self.bundle = Some(
                kinewright_project::bundle::ProjectBundle::activate(
                    path,
                    kinewright_project::bundle::BundleWriteAdmission::new_document(),
                )
                .map_err(|e| e.to_string())?,
            );
        }
        Ok(())
    }
    #[cfg(target_os = "linux")]
    pub(super) fn comment_request_path(&self, key: &str) -> Result<std::path::PathBuf, String> {
        if key.is_empty()
            || key.len() > 128
            || !key
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
        {
            return Err("invalid fixed comment request key".into());
        }
        Ok(self
            .baseline
            .root
            .join("requests")
            .join(format!("{key}.json")))
    }
    #[cfg(target_os = "linux")]
    #[allow(clippy::too_many_lines)]
    #[allow(clippy::items_after_statements)]
    fn comment_graph_submit(
        &mut self,
        args: &super::protocol::CommentArgs,
        fixture: &LoadedFixture,
    ) -> Result<Value, String> {
        use kinewright_project::bundle_records::{ObjectRef, RecordKind, SessionId};
        let path = self.baseline.root.join(&args.path);
        self.open_comment_bundle(&path)?;
        let text = fixture
            .files
            .get(&args.text)
            .ok_or("missing exact comment text")?;
        let strokes = fixture
            .files
            .get(&args.strokes)
            .ok_or("missing exact comment strokes")?;
        let reference = |role: &str| -> Result<ObjectRef, String> {
            let declared = fixture
                .fixture
                .inputs
                .get(role)
                .ok_or("missing comment role identity")?;
            Ok(ObjectRef {
                sha256: declared.sha256.clone(),
                byte_length: declared.byte_length,
                record_kind: RecordKind::OwnedEvidence,
            })
        };
        let text_ref = reference(&args.text)?;
        let strokes_ref = reference(&args.strokes)?;
        let key = args.original.as_deref().unwrap_or(&args.request);
        let request_path = self.comment_request_path(key)?;
        let request = if request_path.is_file() {
            super::protocol::unique_json(&super::protocol::read_bounded(
                &request_path,
                super::protocol::CONTROL_BYTES as u64,
            )?)?
        } else {
            if args.original.is_some() {
                return Err("original command envelope is unavailable".into());
            }
            fs::create_dir_all(request_path.parent().ok_or("request parent")?)
                .map_err(|e| e.to_string())?;
            let nonce_file = self.comment_request_path("opaque-client")?;
            let nonce = if nonce_file.is_file() {
                super::protocol::unique_json(&fs::read(&nonce_file).map_err(|e| e.to_string())?)?
                    .as_str()
                    .ok_or("client nonce")?
                    .to_owned()
            } else {
                let nonce = SessionId::fresh()
                    .map_err(|e| e.to_string())?
                    .as_str()
                    .to_owned();
                fs::write(
                    &nonce_file,
                    serde_json::to_vec(&nonce).map_err(|e| e.to_string())?,
                )
                .map_err(|e| e.to_string())?;
                nonce
            };
            let original_action=fixture.fixture.actions.iter().find(|a|matches!(&a.call,Call::CommentSubmit(c) if c.request==args.request && c.original.is_none())).ok_or("original fixture comment action")?;
            let bundle = self.bundle.as_ref().ok_or("comment bundle")?;
            // This is opaque pre-receipt request data, never an issued authority or a result cache.
            let request = json!({"version":1,"action":"comment","id":{"issuer":nonce,"sequence":original_action.id},
                "project_id":bundle.project_id(),"target_branch":bundle.manual_branch_id(),"owner_epoch":bundle.store().owner_epoch(),
                "lease":null,"expected_head":bundle.manual_head(),"expected_revision":self.baseline.snapshot()?.0,
                "command":null,"comment_text":text_ref,"comment_strokes":strokes_ref,"evidence":[]});
            let bytes = serde_json::to_vec(&request).map_err(|e| e.to_string())?;
            use std::io::Write;
            let mut file = fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&request_path)
                .map_err(|e| e.to_string())?;
            file.write_all(&bytes).map_err(|e| e.to_string())?;
            file.sync_all().map_err(|e| e.to_string())?;
            request
        };
        if request["comment_text"] != serde_json::to_value(&text_ref).map_err(|e| e.to_string())?
            || request["comment_strokes"]
                != serde_json::to_value(&strokes_ref).map_err(|e| e.to_string())?
        {
            return Err(
                "changed-payload receipt scenario unavailable before receipts; NOT RUN".into(),
            );
        }
        let core = self.baseline.core()?.clone();
        let bundle = self.bundle.as_mut().ok_or("comment bundle")?;
        let text_object = bundle
            .store_mut()
            .map_err(|e| e.to_string())?
            .admit_blob(&mut text.as_slice(), &text_ref, &[])
            .map_err(|e| e.to_string())?;
        let strokes_object = bundle
            .store_mut()
            .map_err(|e| e.to_string())?
            .admit_blob(&mut strokes.as_slice(), &strokes_ref, &[])
            .map_err(|e| e.to_string())?;
        let record_id = SessionId::fresh()
            .map_err(|e| e.to_string())?
            .as_str()
            .to_owned();
        // The stored record deliberately has no command ID/digest binding. It is opaque S1 evidence.
        let record=serde_json::to_vec(&json!({"tag":"s1-observer-comment","schema":1,"record_id":record_id,"text":text_ref,"strokes":strokes_ref})).map_err(|e|e.to_string())?;
        let record_ref = ObjectRef {
            sha256: kinewright_media::sha256_bytes(&record),
            byte_length: record.len() as u64,
            record_kind: RecordKind::OwnedEvidence,
        };
        let sealed = bundle
            .store_mut()
            .map_err(|e| e.to_string())?
            .admit_blob(
                &mut record.as_slice(),
                &record_ref,
                &[text_ref, strokes_ref],
            )
            .map_err(|e| e.to_string())?;
        bundle
            .retain_evidence(&record_ref)
            .map_err(|e| e.to_string())?;
        let empty = if bundle.root().is_none() {
            Some(bundle.empty_inputs().map_err(|e| e.to_string())?)
        } else {
            None
        };
        let inputs = empty.as_ref().map_or_else(
            || {
                bundle
                    .root()
                    .expect("root or empty input")
                    .input_manifest_ref
                    .clone()
            },
            |s| s.reference.clone(),
        );
        let saved = bundle.save(&core, &inputs).map_err(|e| e.to_string())?;
        drop(sealed);
        drop(text_object);
        drop(strokes_object);
        Ok(
            json!({"backend":"linux-local-pre-receipt-graph","durable_graph_published":true,"record_id":record_id,"record_ref":record_ref,
            "generation":saved.generation,"head_id":saved.head_id,"receipt_acknowledged":false,"original_request_sha256":kinewright_media::sha256_bytes(&serde_json::to_vec(&request).map_err(|e|e.to_string())?)}),
        )
    }
    #[cfg(target_os = "linux")]
    fn comment_graph_inspect(
        &mut self,
        args: &super::protocol::CommentArgs,
        fixture: &LoadedFixture,
    ) -> Result<Value, String> {
        use kinewright_project::bundle_records::ObjectRef;
        let path = self.baseline.root.join(&args.path);
        self.open_comment_bundle(&path)?;
        let request = super::protocol::unique_json(&super::protocol::read_bounded(
            &self.comment_request_path(&args.request)?,
            super::protocol::CONTROL_BYTES as u64,
        )?)?;
        let text: ObjectRef =
            serde_json::from_value(request["comment_text"].clone()).map_err(|e| e.to_string())?;
        let strokes: ObjectRef = serde_json::from_value(request["comment_strokes"].clone())
            .map_err(|e| e.to_string())?;
        let bundle = self.bundle.as_ref().ok_or("comment bundle")?;
        let mut records = vec![];
        let mut actual_text = None;
        let mut actual_strokes = None;
        for reference in bundle.evidence_refs() {
            let bytes = bundle
                .store()
                .read_evidence(reference)
                .map_err(|e| e.to_string())?;
            let Ok(record) = serde_json::from_slice::<Value>(&bytes) else {
                continue;
            };
            if record["tag"] == "s1-observer-comment"
                && record["text"] == serde_json::to_value(&text).map_err(|e| e.to_string())?
                && record["strokes"] == serde_json::to_value(&strokes).map_err(|e| e.to_string())?
            {
                records.push(record["record_id"].clone());
                actual_text = Some(
                    bundle
                        .store()
                        .read_evidence(&text)
                        .map_err(|e| e.to_string())?,
                );
                actual_strokes = Some(
                    bundle
                        .store()
                        .read_evidence(&strokes)
                        .map_err(|e| e.to_string())?,
                );
            }
        }
        let text =
            actual_text.ok_or("persisted comment text was not reached from the actual root")?;
        let strokes = actual_strokes
            .ok_or("persisted comment strokes were not reached from the actual root")?;
        let root = bundle.root().ok_or("published comment root")?;
        if root.receipt_index_ref.is_some() || root.issuer_state_ref.is_some() {
            return Err("pre-receipt observer cannot interpret receipt-bearing root".into());
        }
        Ok(
            json!({"backend":"linux-local-pre-receipt-graph","text_utf8":std::str::from_utf8(&text).map_err(|e|e.to_string())?,
            "strokes_utf8":std::str::from_utf8(&strokes).map_err(|e|e.to_string())?,"text_exact":text.as_slice()==fixture.files.get(&args.text).ok_or("expected text")?.as_slice(),
            "strokes_exact":strokes.as_slice()==fixture.files.get(&args.strokes).ok_or("expected strokes")?.as_slice(),
            "record_ids":records,"record_count":records.len(),"binding_present":root.receipt_index_ref.is_some(),"original_request":request,"generation":root.generation}),
        )
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
