//! Repeat the independent source/rescue oracles with the actual final host owner.
//! Setup and private IO faults inspect that same owner; Save, rescue and late
//! destination reconciliation execute the shipping command dispatcher.
#![allow(clippy::too_many_lines)]
use super::*;
use kinewright_project::{
    bundle::{BundleWriteAdmission, ProjectBundle, PublicationState},
    bundle_rescue::RescueOutcome,
    namespace::DestinationInspection,
};

thread_local! {
    static RESCUE_OBSERVATION: std::cell::RefCell<Option<RescueOutcome>> = const { std::cell::RefCell::new(None) };
}
pub(super) fn observe_rescue(outcome: &RescueOutcome) {
    RESCUE_OBSERVATION.with_borrow_mut(|observed| *observed = Some(outcome.clone()));
}
fn durable_test_root() -> &'static Path {
    static ROOT: OnceLock<std::path::PathBuf> = OnceLock::new();
    ROOT.get_or_init(|| {
        let root = kinewright_media::test_support::workspace_target_dir().join("c12-host-roots");
        std::fs::create_dir_all(&root).unwrap();
        root
    })
}
fn document() -> Document {
    Document {
        tracks: (1..=200)
            .map(|id| kinewright_core::Track {
                id: kinewright_core::TrackId(id),
                kind: kinewright_core::TrackKind::Video,
                sync_lock: true,
                clips: vec![],
            })
            .collect(),
        ..Document::default()
    }
}
fn apply(core: &Core, command: kinewright_core::Command) {
    assert!(matches!(
        core.request(command).unwrap(),
        kinewright_core::Event::DocumentChanged { .. }
    ));
}
fn edit(core: &Core, locked: bool) {
    apply(
        core,
        kinewright_core::Command::Do(kinewright_core::Operation::SetTrackSyncLock {
            track: kinewright_core::TrackId(1),
            locked,
        }),
    );
}
fn refusal(reply: HostReply) -> Result<HostReply, BundleError> {
    if let Some(error) = reply.error {
        return Err(match error.code.as_str() {
            "durability_unknown" => BundleError::DurabilityUnknown,
            "history_changed" => BundleError::HistoryChanged,
            "save_in_progress" => BundleError::SaveInProgress,
            _ => BundleError::Corrupt(error.to_string()),
        });
    }
    Ok(reply)
}

struct HostSession(HostCommandOwner);
type Ja1EditingSession = HostSession;
impl std::ops::Deref for HostSession {
    type Target = kinewright_project::editing_session::Ja1EditingSession;
    fn deref(&self) -> &Self::Target {
        self.0.owner.as_ref().unwrap()
    }
}
impl std::ops::DerefMut for HostSession {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.0.owner.as_mut().unwrap()
    }
}
impl HostSession {
    fn wrap(owner: kinewright_project::editing_session::Ja1EditingSession) -> Self {
        let roots = SessionRoots {
            read: vec![durable_test_root().into()],
            write: vec![durable_test_root().into()],
        };
        let mut host = HostCommandOwner::open(HostConfig {
            project: "c12-final-owning-host".into(),
            path: None,
            roots,
            origin: HostOrigin::Headless,
        })
        .unwrap();
        host.initial_core = owner.core().clone();
        host.owner = Some(owner);
        Self(host)
    }
    fn new(parent: &Path, core: &Core, limits: StorageLimits) -> Result<Self, BundleError> {
        kinewright_project::editing_session::Ja1EditingSession::new(parent, core, limits)
            .map(Self::wrap)
    }
    fn from_bundle(
        bundle: ProjectBundle,
        core: &Core,
        inputs: ObjectRef,
    ) -> Result<Self, BundleError> {
        kinewright_project::editing_session::Ja1EditingSession::from_bundle(bundle, core, inputs)
            .map(Self::wrap)
    }
    fn activate(
        path: &Path,
        admission: BundleWriteAdmission,
        core: &Core,
        input: Option<&ObjectRef>,
    ) -> Result<Self, BundleError> {
        kinewright_project::editing_session::Ja1EditingSession::activate(
            path, admission, core, input,
        )
        .map(Self::wrap)
    }
    fn open(path: &Path) -> Result<Self, BundleError> {
        kinewright_project::editing_session::Ja1EditingSession::open(path).map(Self::wrap)
    }
    fn run(&mut self, command: HostCommand) -> HostReply {
        self.0.execute(command, self.0.config.roots.clone())
    }
    fn save_active(&mut self) -> Result<(), BundleError> {
        let snapshot = HostSnapshot::capture(self.core()).unwrap();
        refusal(self.run(HostCommand::Save { snapshot })).map(|_| ())
    }
    fn reconcile_retained_destination(
        &mut self,
        path: &Path,
    ) -> Result<PublicationState, BundleError> {
        refusal(self.run(HostCommand::Reconcile {
            destination: path.into(),
        }))?;
        self.retained_destinations()?
            .into_iter()
            .find(|(p, _)| p == path)
            .map(|(_, state)| state)
            .ok_or(BundleError::HistoryChanged)
    }
    fn rescue(
        &mut self,
        selection: RescueSelection,
        consent: &RescueConsent,
        destination: &DestinationInspection,
    ) -> Result<RescueOutcome, BundleError> {
        // The same frozen preparation and exact explicit consent enter the real
        // dispatcher. This observation never supplies an outcome or a receipt.
        let choices = selection.consent_choices_probe(consent);
        let (trim, omission) = choices.as_ref().copied().unwrap_or((false, false));
        let supplied = if choices.is_ok() {
            selection.consent()
        } else {
            consent.clone()
        };
        self.0.choice = Some(Choice::Rescue {
            selection: Box::new(selection),
            consent: supplied,
            trim: false,
            omission: false,
        });
        if trim {
            refusal(self.run(HostCommand::ConsentTrim))?;
        }
        if omission {
            refusal(self.run(HostCommand::ConsentOmission))?;
        }
        RESCUE_OBSERVATION.with_borrow_mut(|observed| *observed = None);
        let reply = self.run(HostCommand::RescueTo {
            destination: destination.path().into(),
        });
        if let Some(outcome) = RESCUE_OBSERVATION.with_borrow_mut(Option::take) {
            assert_eq!(reply.activation.is_some(), outcome.activated);
            if !outcome.activated && matches!(outcome.status, CopyStatus::Committed(_)) {
                assert!(
                    reply.saved.is_none(),
                    "a capture cannot mark the active actor Saved"
                );
            }
            return Ok(outcome);
        }
        refusal(reply)?;
        Err(BundleError::HistoryBarrierInvariant)
    }
}

#[path = "../../../kinewright-project/tests/support/ja_s2_c5_rescue.rs"]
mod rescue_cases;

#[test]
fn n2_committed_capture_never_acknowledges_sealed_a() {
    use kinewright_project::io_observer;
    let root =
        kinewright_media::test_support::TempDirectory::new_in(durable_test_root(), "c12-n2-host");
    let path = root.path("drive/a.kinewright");
    let core = Core::spawn(document()).unwrap();
    let mut host = HostSession::new(root.root(), &core, StorageLimits::default()).unwrap();
    host.first_save(&path, StorageLimits::default()).unwrap();
    edit(&core, false);
    assert!(io_observer::fail_once("root_parent_sync", || host.save_active()).is_err());
    std::fs::rename(root.path("drive"), root.path("hidden")).unwrap();
    let offer = host.probe_active().unwrap().unwrap();
    let capture = host.prepare_rescue(&offer).unwrap().unwrap();
    let selection = host
        .preview_rescue(capture, StorageLimits::default())
        .unwrap();
    let consent = selection.consent();
    let b = root.path("b.kinewright");
    let destination = host.inspect_rescue_destination(&b).unwrap();
    let binder = ProjectBundle::activate(
        &root.path("authority.kinewright"),
        BundleWriteAdmission::new_document(),
    )
    .unwrap();
    let outcome = host
        .rescue_with_activation(selection, &consent, &destination, |candidate| {
            let authority = binder.attach_core(candidate)?;
            let cut = authority.capture().unwrap();
            authority.resume(&cut).unwrap();
            edit(candidate, true);
            authority.freeze().unwrap();
            Ok(())
        })
        .unwrap();
    let mut reply = HostReply::default();
    host.0
        .accept_rescue_outcome(outcome, &b, &mut reply)
        .unwrap();
    assert!(reply.saved.is_none());
    assert!(reply.activation.is_none());
    assert!(host.0.message.contains("A is sealed"));
    assert!(host.0.message.contains(&b.display().to_string()));
    assert_eq!(core.attachment_state().unwrap(), AttachmentState::Sealed);
    assert!(host.is_capture_destination(&b));
}

fn evidence_fixture(
    label: &str,
) -> (
    kinewright_media::test_support::TempDirectory,
    HostSession,
    ObjectRef,
) {
    use kinewright_project::bundle_records::RecordKind;
    let root = kinewright_media::test_support::TempDirectory::new_in(durable_test_root(), label);
    let path = root.path("drive/a.kinewright");
    std::fs::create_dir(root.path("drive")).unwrap();
    let core = Core::spawn(document()).unwrap();
    let mut bundle = ProjectBundle::activate(&path, BundleWriteAdmission::new_document()).unwrap();
    let inputs = bundle.empty_inputs().unwrap().reference.clone();
    let bytes = b"C12 independent evidence stays exact across remote rescue and compaction";
    let reference = ObjectRef {
        sha256: kinewright_media::sha256_bytes(bytes),
        byte_length: bytes.len() as u64,
        record_kind: RecordKind::OwnedEvidence,
    };
    let admitted = bundle
        .store_mut()
        .unwrap()
        .admit_blob(&mut &bytes[..], &reference, &[])
        .unwrap();
    bundle.retain_evidence(&reference).unwrap();
    bundle.save(&core, &inputs).unwrap();
    drop(admitted);
    let mut host = HostSession::from_bundle(bundle, &core, inputs).unwrap();
    host.save_active().unwrap();
    (root, host, reference)
}

#[test]
fn w02_host_missing_required_evidence_refuses_without_destination_mutation() {
    use kinewright_project::io_observer;
    let (root, mut host, reference) = evidence_fixture("c12-missing-host");
    assert!(io_observer::fail_once("root_parent_sync", || host.save_active()).is_err());
    let object = root
        .path("drive/a.kinewright-assets/studio/objects")
        .join(&reference.sha256);
    let bytes = std::fs::read(&object).unwrap();
    std::fs::remove_file(&object).unwrap();
    std::fs::rename(root.path("drive"), root.path("hidden")).unwrap();
    let before = host.core().export_history().unwrap();
    let (reply, trace) = io_observer::observe(|| {
        host.run(HostCommand::PreviewRescue {
            snapshot: HostSnapshot::from_history(before.clone()).unwrap(),
        })
    });
    let error = reply.error.unwrap();
    assert_eq!(error.code, "rescue_missing_owned_bytes");
    assert!(error.message.contains(&reference.sha256), "{error:?}");
    assert!(
        error.message.contains("OwnedEvidence") && error.message.contains("Manifest"),
        "{error:?}"
    );
    assert!(
        trace.events.iter().all(|event| matches!(
            event.operation,
            "rescue_separation_inspect" | "studio_dirty_history_hash" | "rescue_probe"
        )),
        "{trace:?}"
    );
    assert_eq!(host.rescue_slots(), 0);
    assert_eq!(host.core().export_history().unwrap(), before);
    assert!(!root.path("b.kinewright").exists());
    std::fs::write(
        root.path("hidden/a.kinewright-assets/studio/objects")
            .join(&reference.sha256),
        bytes,
    )
    .unwrap();
    refusal(host.run(HostCommand::PreviewRescue {
        snapshot: HostSnapshot::from_history(before).unwrap(),
    }))
    .unwrap();
    let reply = refusal(host.run(HostCommand::RescueTo {
        destination: root.path("b.kinewright"),
    }))
    .unwrap();
    assert!(
        reply.activation.is_some(),
        "reconnecting the exact bytes makes the real host route usable"
    );
}

#[test]
fn w11_host_recopy_compacts_provenance_and_keeps_independent_bytes() {
    use kinewright_project::{bundle::LoadedBundle, io_observer};
    let (root, mut host, reference) = evidence_fixture("c12-compaction-host");
    let mut path = root.path("drive/a.kinewright");
    for round in 0..18 {
        assert!(io_observer::fail_once("root_parent_sync", || host.save_active()).is_err());
        let hidden = root.path(&format!("hidden-{round}"));
        std::fs::rename(path.parent().unwrap(), &hidden).unwrap();
        let history = HostSnapshot::capture(host.core()).unwrap();
        let preview = refusal(host.run(HostCommand::PreviewRescue {
            snapshot: history.clone(),
        }))
        .unwrap();
        assert!(!preview.status.rescue_trim_required && !preview.status.rescue_omission_required);
        let next = root.path(&format!("round-{round}/a.kinewright"));
        let rescued = refusal(host.run(HostCommand::RescueTo {
            destination: next.clone(),
        }))
        .unwrap();
        assert!(rescued.activation.is_some());
        assert_eq!(rescued.history.as_ref(), Some(&history.history));
        std::fs::rename(hidden, path.parent().unwrap()).unwrap();
        refusal(host.run(HostCommand::Reconcile {
            destination: path.clone(),
        }))
        .unwrap();
        drop(host);
        let loaded = LoadedBundle::read(&next).unwrap();
        let writer = loaded.into_writer().unwrap();
        let records = writer.rescue_records().unwrap();
        assert_eq!(records.len(), 1);
        let record = &records[0];
        assert!(record.attempts.len() <= 16);
        assert!(serde_json::to_vec(record).unwrap().len() <= 64 * 1024);
        if round == 17 {
            assert!(record.summary.count > 0 && record.summary.unresolved);
            assert!(!record.summary.digest.is_empty());
        }
        assert_eq!(
            &*writer.store().read_evidence(&reference).unwrap(),
            b"C12 independent evidence stays exact across remote rescue and compaction"
        );
        drop(writer);
        host = HostSession::open(&next).unwrap();
        path = next;
    }
}

#[test]
fn w12_host_cleanup_only_fresh_copy_keeps_exact_charge_and_history() {
    use kinewright_project::{bundle::LoadedBundle, io_observer};
    let (root, mut host, _) = evidence_fixture("c12-cleanup-host");
    let path = root.path("drive/a.kinewright");
    let expected = host.core().export_history().unwrap();
    let usage = host.store().unwrap().usage_handle();
    let result = io_observer::fail_once("root_file_sync", || {
        io_observer::fail_path("root_cleanup_unlink", &path, || host.save_active())
    });
    assert!(result.is_err());
    let pending = host.store().unwrap().pending_control_bytes_probe().unwrap();
    assert!(pending > 0);
    assert!(!host.dirty().unwrap());
    std::fs::rename(root.path("drive"), root.path("hidden")).unwrap();
    let b = root.path("b.kinewright");
    let copied = refusal(host.run(HostCommand::SaveAs {
        destination: b.clone(),
        snapshot: HostSnapshot::from_history(expected.clone()).unwrap(),
    }))
    .unwrap();
    assert!(copied.saved.is_some());
    assert_eq!(host.rescue_slots(), 0);
    assert_eq!(host.core().export_history().unwrap(), expected);
    assert!(usage().unwrap().reserved_bytes >= pending);
    assert!(!usage().unwrap().damaged_accounting);
    drop(host);
    let loaded = LoadedBundle::read(&b).unwrap();
    let fresh = Core::spawn(Document::default()).unwrap();
    loaded.restore_manual(&fresh).unwrap();
    assert_eq!(fresh.export_history().unwrap(), expected);
}
#[path = "../../../kinewright-project/tests/support/ja_s2_c3.rs"]
mod source_cases;
