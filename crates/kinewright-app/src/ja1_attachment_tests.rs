#![allow(clippy::too_many_lines)]
use super::*;
#[cfg(target_os = "linux")]
use kinewright_core::{Operation, Track, TrackId, TrackKind};
#[cfg(target_os = "linux")]
use kinewright_media::test_support::TempDirectory;
#[cfg(target_os = "linux")]
use kinewright_project::{
    bundle::{BundleSaveReport, BundleWriteAdmission, LoadedBundle, ProjectBundle},
    bundle_rescue::RescueOutcome,
    namespace::DestinationInspection,
};

/// Shared C5 independent history/byte oracles, driven through the actual App actions.
#[cfg(target_os = "linux")]
struct AppSession(Ja1Attachment);
#[cfg(target_os = "linux")]
type Ja1EditingSession = AppSession;
#[cfg(target_os = "linux")]
impl std::ops::Deref for AppSession {
    type Target = kinewright_project::editing_session::Ja1EditingSession;
    fn deref(&self) -> &Self::Target {
        &self.0.owner
    }
}
#[cfg(target_os = "linux")]
impl std::ops::DerefMut for AppSession {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0.owner
    }
}
#[cfg(target_os = "linux")]
impl AppSession {
    fn new(parent: &Path, core: &Core, limits: StorageLimits) -> Result<Self, BundleError> {
        Ok(Self(Ja1Attachment::new(
            kinewright_project::editing_session::Ja1EditingSession::new(parent, core, limits)?,
        )?))
    }
    fn open(path: &Path) -> Result<Self, BundleError> {
        Ok(Self(Ja1Attachment::open(path)?))
    }
    fn from_bundle(
        bundle: ProjectBundle,
        core: &Core,
        input: ObjectRef,
    ) -> Result<Self, BundleError> {
        Ok(Self(Ja1Attachment::new(
            kinewright_project::editing_session::Ja1EditingSession::from_bundle(
                bundle, core, input,
            )?,
        )?))
    }
    fn activate(
        path: &Path,
        admission: BundleWriteAdmission,
        core: &Core,
        input: Option<&ObjectRef>,
    ) -> Result<Self, BundleError> {
        Ok(Self(Ja1Attachment::new(
            kinewright_project::editing_session::Ja1EditingSession::activate(
                path, admission, core, input,
            )?,
        )?))
    }
    fn save_active(&mut self) -> Result<BundleSaveReport, BundleError> {
        self.0.apply(Action::Save)?;
        if matches!(self.0.choice, Some(Choice::SuccessorSave(_))) {
            assert!(rendered(&self.0).contains("omits that effect"));
            self.0.apply(Action::ConfirmSuccessorSave)?;
        }
        Ok(self.0.last_save.take().unwrap())
    }
    #[allow(
        clippy::unnecessary_wraps,
        reason = "same driver signature as the project owner"
    )]
    fn reconcile_active(&mut self) -> Result<Option<BundleSaveReport>, BundleError> {
        self.0.apply(Action::Retry)?;
        Ok(None)
    }
    fn reconcile_retained_destination(
        &mut self,
        path: &Path,
    ) -> Result<PublicationState, BundleError> {
        self.0.apply(Action::Reconcile(path.to_path_buf()))?;
        if !self.0.owner.is_capture_destination(path) {
            assert!(!rendered(&self.0).contains("holds a copy as of capture"));
        }
        self.0
            .owner
            .retained_destinations()?
            .into_iter()
            .find(|(p, _)| p == path)
            .map(|(_, state)| state)
            .ok_or(BundleError::HistoryBarrierInvariant)
    }
    fn prepare_recovery(&mut self, target: RecoverySelection) -> Result<(), BundleError> {
        self.0.apply(Action::PreviewRecovery(target))
    }
    fn select_local(&mut self) -> ProjectSelectionResult {
        self.0.apply(Action::OpenRecovery).unwrap();
        if matches!(self.0.last_selection, Some(ProjectSelectionResult::Unknown)) {
            let text = rendered(&self.0);
            assert!(text.contains("read-only"));
            if self.0.owner.preliminary_save_pending() {
                assert!(
                    text.contains("Your current save outcome is unknown"),
                    "{text}"
                );
                assert!(!text.contains("Saved "), "no early Saved: {text}");
            }
        }
        self.0.last_selection.take().unwrap()
    }
    #[allow(
        clippy::unnecessary_wraps,
        reason = "same driver signature as the project owner"
    )]
    fn retirement_candidates(&self) -> Result<Vec<BranchId>, BundleError> {
        Ok(self.0.candidates.clone())
    }
    fn rescue(
        &mut self,
        selection: RescueSelection,
        consent: &RescueConsent,
        destination: &DestinationInspection,
    ) -> Result<RescueOutcome, BundleError> {
        let choices = selection.consent_choices_probe(consent);
        let fresh = if choices.is_ok() {
            selection.consent()
        } else {
            consent.clone()
        };
        let (trim, omission) = choices.unwrap_or((false, false));
        self.0.message = rescue_preview(&selection);
        let removed = selection.removed_undo();
        let omitted = !selection.omissions().is_empty();
        self.0.choice = Some(Choice::Rescue {
            selection: Box::new(selection),
            consent: fresh,
            trim: false,
            omission: false,
        });
        if removed > 0 {
            assert!(rendered(&self.0).contains("Undo before step"));
        }
        if omitted {
            assert!(
                rendered(&self.0)
                    .contains("Older recovery points from A are not in the rescue copy")
            );
        }
        if trim {
            self.0.apply(Action::ConsentTrim)?;
        }
        if omission {
            self.0.apply(Action::ConsentOmission)?;
        }
        self.0.rescue_with_inspection(destination)?;
        self.0.refresh();
        if self.0.owner.has_prepared_selection()
            && matches!(
                self.0.last_rescue.as_ref().unwrap().status,
                CopyStatus::DurabilityUnknown
            )
        {
            assert!(rendered(&self.0).contains("Editing remains read-only"));
        }
        Ok(self.0.last_rescue.take().unwrap())
    }
    fn retire_rescue_recovery_point(
        &mut self,
        point: &ObjectRef,
        root: &str,
        confirmed: bool,
    ) -> Result<BundleSaveReport, BundleError> {
        if !confirmed {
            return self
                .0
                .owner
                .retire_rescue_recovery_point(point, root, false);
        }
        self.0.apply(Action::PreviewForget)?;
        let Some(Choice::Forget {
            point: selected,
            root: expected,
        }) = &mut self.0.choice
        else {
            return Err(BundleError::HistoryChanged);
        };
        if selected != point {
            return Err(BundleError::HistoryChanged);
        }
        root.clone_into(expected);
        assert!(rendered(&self.0).contains("Forget the older recovery point from A"));
        self.0.apply(Action::Forget)?;
        Ok(self.0.last_retirement.take().unwrap())
    }
    fn retire_working_branch(
        &mut self,
        id: &BranchId,
        root: &str,
        confirmed: bool,
    ) -> Result<BundleSaveReport, BundleError> {
        if !confirmed {
            return self.0.owner.retire_working_branch(id, root, false);
        }
        self.0.apply(Action::PreviewRetirement(id.clone()))?;
        if let Some(Choice::Remove { root: expected, .. }) = &mut self.0.choice {
            root.clone_into(expected);
        }
        self.0.apply(Action::RemoveBranch)?;
        Ok(self.0.last_retirement.take().unwrap())
    }
    fn abandon_recovery(&mut self) -> Result<String, BundleError> {
        self.0.apply(Action::Abandon)?;
        Ok(self.0.message.clone())
    }
}

#[path = "../../kinewright-project/tests/support/ja_s2_c6_admission.rs"]
mod admission;
#[cfg(target_os = "linux")]
#[path = "../../kinewright-project/tests/support/ja_s2_c5_pressure.rs"]
mod pressure_cases;
#[cfg(target_os = "linux")]
#[path = "../../kinewright-project/tests/support/ja_s2_c5_recovery.rs"]
mod recovery_cases;
#[cfg(target_os = "linux")]
#[path = "../../kinewright-project/tests/support/ja_s2_c5_rescue.rs"]
mod rescue_cases;

fn durable_test_root() -> &'static Path {
    static ROOT: std::sync::LazyLock<PathBuf> = std::sync::LazyLock::new(|| {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/c6-test-roots");
        std::fs::create_dir_all(&path).unwrap();
        path.canonicalize().unwrap()
    });
    &ROOT
}
#[cfg(target_os = "linux")]
fn temp() -> TempDirectory {
    TempDirectory::new_in(durable_test_root(), "c6-app")
}
#[cfg(target_os = "linux")]
fn doc() -> Document {
    Document {
        tracks: vec![Track {
            id: TrackId(1),
            kind: TrackKind::Video,
            sync_lock: true,
            clips: vec![],
        }],
        ..Document::default()
    }
}
#[cfg(target_os = "linux")]
fn edit(core: &Core, locked: bool) {
    assert!(matches!(
        core.request(Command::Do(Operation::SetTrackSyncLock {
            track: TrackId(1),
            locked
        }))
        .unwrap(),
        Event::DocumentChanged { .. }
    ));
}
#[cfg(target_os = "linux")]
fn rendered(attachment: &Ja1Attachment) -> String {
    let context = egui::Context::default();
    let output = context.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1600.0, 3000.0),
            )),
            ..Default::default()
        },
        |ui| {
            attachment.show(ui);
        },
    );
    painted_text(&output)
}
#[cfg(target_os = "linux")]
pub(crate) fn painted_text(output: &egui::FullOutput) -> String {
    fn text(shape: &egui::epaint::Shape, out: &mut String) {
        match shape {
            egui::epaint::Shape::Text(t) => {
                out.push_str(&t.galley.job.text);
                out.push('\n');
            }
            egui::epaint::Shape::Vec(shapes) => {
                for s in shapes {
                    text(s, out);
                }
            }
            _ => {}
        }
    }
    let mut out = String::new();
    for shape in &output.shapes {
        text(&shape.shape, &mut out);
    }
    out
}
#[test]
fn w14_unsupported_rescue_refuses_before_session_mutation() {
    admission::matrix(|path| {
        let mut app = Ja1Attachment::open(path)?;
        app.apply(Action::Save)?;
        Ok(())
    });
}
#[cfg(target_os = "linux")]
pub(crate) fn named() -> (TempDirectory, Ja1Attachment) {
    let temp = temp();
    let core = Core::spawn(doc()).unwrap();
    let mut owner = kinewright_project::editing_session::Ja1EditingSession::new(
        temp.root(),
        &core,
        StorageLimits::default(),
    )
    .unwrap();
    owner
        .first_save(&temp.path("drive/a.kinewright"), StorageLimits::default())
        .unwrap();
    (temp, Ja1Attachment::new(owner).unwrap())
}
#[cfg(target_os = "linux")]
pub(crate) fn unknown_selection(app: &mut Ja1Attachment, temp: &TempDirectory, landed: bool) {
    use kinewright_project::io_observer;
    for value in [false, true, false] {
        edit(app.owner.core(), value);
        app.apply(Action::Save).unwrap();
    }
    let target = app
        .owner
        .list_recovery_points()
        .unwrap()
        .last()
        .unwrap()
        .clone();
    edit(app.owner.core(), true);
    app.apply(Action::PreviewRecovery(target)).unwrap();
    let preview = rendered(app);
    assert!(
        preview.contains("Saving your current work will remove this recovery point"),
        "{preview}"
    );
    assert!(preview.contains("oldest Undo"));
    let trigger = if landed {
        "root_parent_sync"
    } else {
        "root_rename_io"
    };
    io_observer::fail_nth(trigger, 2, || app.apply(Action::OpenRecovery)).unwrap();
    assert_eq!(
        app.owner.core().attachment_state().unwrap(),
        AttachmentState::Frozen
    );
    assert!(rendered(app).contains("read-only"));
    std::fs::rename(temp.path("drive"), temp.path("hidden-drive")).unwrap();
    app.apply(Action::Retry).unwrap();
    assert!(rendered(app).lines().any(|line| line == "Save rescue copy"));
}
#[test]
#[cfg(target_os = "linux")]
fn w13_checkpoint_recovery_preserves_dirty_b_before_activation() {
    recovery_cases::w13_checkpoint_recovery_preserves_dirty_b_before_activation();
}
#[test]
#[cfg(target_os = "linux")]
fn w01_immediate_rescue_switches_and_ordinary_saves_continue() {
    rescue_cases::w01_first_save_rescue_is_independent();
}
#[test]
#[cfg(target_os = "linux")]
fn w05_capture_races_and_infallible_handoff() {
    let (temp, mut app) = named();
    app.apply(Action::Save).unwrap();
    let target = app
        .owner
        .list_recovery_points()
        .unwrap()
        .last()
        .unwrap()
        .clone();
    let before = app.owner.core().clone();
    edit(&before, false);
    app.apply(Action::PreviewRecovery(target)).unwrap();
    app.apply(Action::OpenRecovery).unwrap();
    let prepared = app.view.take().unwrap();
    assert_ne!(before.actor_identity(), prepared.core.actor_identity());
    assert_eq!(before.attachment_state().unwrap(), AttachmentState::Sealed);
    assert_eq!(
        prepared.core.actor_identity(),
        app.owner.core().actor_identity()
    );
    assert_eq!(
        prepared.document,
        app.owner.core().export_history().unwrap().document
    );
    assert!(matches!(
        before.request(Command::Undo).unwrap(),
        Event::MutationRefused { .. }
    ));
    drop(app);
    LoadedBundle::read(&temp.path("drive/a.kinewright")).unwrap();
}
#[test]
#[cfg(target_os = "linux")]
fn w03_choices_never_supply_implicit_consent() {
    let (temp, mut app) = named();
    unknown_selection(&mut app, &temp, true);
    let before = app.owner.core().export_history().unwrap();
    let slots = app.owner.rescue_slots();
    app.apply(Action::PreviewRescue).unwrap();
    assert_eq!(app.owner.rescue_slots(), slots);
    assert!(!temp.path("b.kinewright").exists());
    app.apply(Action::Cancel).unwrap();
    assert_eq!(app.owner.core().export_history().unwrap(), before);
    assert_eq!(
        app.owner.core().attachment_state().unwrap(),
        AttachmentState::Frozen
    );
    assert_eq!(app.owner.rescue_slots(), slots);
}
#[test]
#[cfg(target_os = "linux")]
fn w03_trim_choice_requires_its_own_consent_and_keeps_a_exact() {
    use kinewright_project::io_observer;
    let temp = temp();
    let document = Document {
        tracks: (1..=200)
            .map(|id| Track {
                id: TrackId(id),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: vec![],
            })
            .collect(),
        ..Document::default()
    };
    let core = Core::spawn(document).unwrap();
    for i in 0..40 {
        edit(&core, i % 2 == 0);
    }
    let mut owner = kinewright_project::editing_session::Ja1EditingSession::new(
        temp.root(),
        &core,
        StorageLimits::default(),
    )
    .unwrap();
    let profile = StorageLimits {
        branch_history_bytes: 1 << 20,
        ..StorageLimits::default()
    };
    assert!(matches!(
        io_observer::fail_once("root_parent_sync", || owner
            .first_save(&temp.path("drive/a.kinewright"), profile))
        .unwrap(),
        CopyStatus::DurabilityUnknown
    ));
    std::fs::rename(temp.path("drive"), temp.path("hidden-drive")).unwrap();
    let expected = core.export_history().unwrap();
    let mut app = Ja1Attachment::new(owner).unwrap();
    app.apply(Action::PreviewRescue).unwrap();
    let text = rendered(&app);
    assert!(text.contains("Undo before step"), "{text}");
    assert!(text.contains("Remove this older Undo history in B"));
    assert!(
        !text.contains("Save rescue copy to…"),
        "no implicit trim consent: {text}"
    );
    assert!(
        app.apply(Action::RescueTo(temp.path("refused.kinewright")))
            .is_err()
    );
    assert!(!temp.path("refused.kinewright").exists());
    assert_eq!(core.export_history().unwrap(), expected);
    app.apply(Action::PreviewRescue).unwrap();
    app.apply(Action::ConsentTrim).unwrap();
    assert!(rendered(&app).contains("Save rescue copy to…"));
    app.apply(Action::RescueTo(temp.path("b.kinewright")))
        .unwrap();
    assert_eq!(core.export_history().unwrap(), expected);
    assert_eq!(core.attachment_state().unwrap(), AttachmentState::Sealed);
}
#[test]
#[cfg(target_os = "linux")]
fn w15_late_rescue_is_capture_only_and_release_is_explicit() {
    let (temp, mut app) = named();
    unknown_selection(&mut app, &temp, true);
    app.apply(Action::PreviewRescue).unwrap();
    let handoff = rendered(&app);
    assert!(
        handoff.contains("no longer in the project's automatic list"),
        "{handoff}"
    );
    assert!(handoff.contains("not in B"), "{handoff}");
    assert!(
        handoff.contains("A keeps it only if the selection completed"),
        "{handoff}"
    );
    app.apply(Action::RescueTo(temp.path("b.kinewright")))
        .unwrap();
    let b = app.owner.core().clone();
    edit(&b, false);
    let history = b.export_history().unwrap();
    let dirty = app.owner.dirty().unwrap();
    std::fs::rename(temp.path("hidden-drive"), temp.path("drive")).unwrap();
    app.apply(Action::Reconcile(temp.path("drive/a.kinewright")))
        .unwrap();
    assert_eq!(b.export_history().unwrap(), history);
    assert_eq!(app.owner.dirty().unwrap(), dirty);
    assert_eq!(app.owner.core().actor_identity(), b.actor_identity());
    app.apply(Action::Save).unwrap();
    drop(app);
    let reopened = Core::spawn(Document::default()).unwrap();
    LoadedBundle::read(&temp.path("b.kinewright"))
        .unwrap()
        .restore_manual(&reopened)
        .unwrap();
    assert_eq!(
        reopened.export_history().unwrap(),
        b.export_history().unwrap()
    );
}
#[test]
#[cfg(target_os = "linux")]
fn w13_preview_close_and_every_destination_disclose_session_pin_loss() {
    let (temp, mut app) = named();
    unknown_selection(&mut app, &temp, false);
    let before = app.owner.core().export_history().unwrap();
    let close = app.close_text();
    assert!(
        close.contains("no longer in the project's automatic list"),
        "{close}"
    );
    assert!(close.contains("Closing abandons the selection"));
    assert!(close.contains("A keeps it only if the selection completed"));
    assert!(close.contains("only accessible copy"));
    assert!(!close.contains("Closing is safe"));
    for attempt in [
        app.owner
            .save_as(&temp.path("else.kinewright"), StorageLimits::default())
            .map(|_| ()),
        app.owner
            .open_project(&temp.path("other.kinewright"))
            .map(|_| ()),
        app.destination_change(),
    ] {
        let text = attempt.err().unwrap().to_string();
        assert!(text.contains("Abandon this selection explicitly"), "{text}");
        assert!(
            text.contains("no longer in the project's automatic list"),
            "{text}"
        );
    }
    assert_eq!(app.owner.core().export_history().unwrap(), before);
    assert!(app.apply(Action::Abandon).is_err());
    std::fs::rename(temp.path("hidden-drive"), temp.path("drive")).unwrap();
    app.apply(Action::Retry).unwrap();
    app.apply(Action::Abandon).unwrap();
    assert!(app.message.contains("lets it go"));
    app.destination_change().unwrap();
}
#[test]
#[cfg(target_os = "linux")]
fn w13_actual_destination_controls_and_background_freeze() {
    crate::app::ja1_destination_witness();
}
#[test]
#[cfg(target_os = "linux")]
fn w13_owner_reported_blockers_render_save_and_close_routes() {
    let (_temp, mut app) = named();
    let guard = app.owner.fork_durable_branch("Color session").unwrap();
    edit(guard.core(), false);
    app.refresh();
    let text = rendered(&app);
    assert!(text.contains("Color session / specialist base"), "{text}");
    assert!(text.contains("Save this session: Color session"), "{text}");
    assert!(text.contains("Close this session: Color session"), "{text}");
    assert!(!app.candidates.contains(guard.branch_id()));
    app.apply(Action::SaveSession(guard.branch_id().clone()))
        .unwrap();
    assert!(!app.candidates.contains(guard.branch_id()));
    app.apply(Action::CloseSession(guard.branch_id().clone()))
        .unwrap();
    assert!(app.candidates.contains(guard.branch_id()));
}
#[test]
#[cfg(target_os = "linux")]
fn w08_focus_and_status_close_preserve_originating_context() {
    crate::app::ja1_focus_witness();
}

#[test]
#[cfg(target_os = "linux")]
fn w10_reconciliation_preserves_live_and_sealed_edits() {
    rescue_cases::w10_pending_command_exclusion_and_original_id_lookup();
}

#[test]
#[cfg(target_os = "linux")]
fn w15_completed_capture_release_keeps_sources_and_active_c_exact() {
    use kinewright_project::io_observer;
    for move_to_c in [false, true] {
        let (temp, mut app) = named();
        let original = app.owner.core().clone();
        edit(&original, false);
        assert!(io_observer::fail_once("root_parent_sync", || app.apply(Action::Save)).is_err());
        std::fs::rename(temp.path("drive"), temp.path("hidden-drive")).unwrap();
        app.apply(Action::PreviewRescue).unwrap();
        io_observer::fail_once("root_parent_sync", || {
            app.apply(Action::RescueTo(temp.path("b.kinewright")))
        })
        .unwrap();
        assert_eq!(original.attachment_state().unwrap(), AttachmentState::Live);
        edit(&original, true);
        if move_to_c {
            app.apply(Action::PreviewRescue).unwrap();
            app.apply(Action::RescueTo(temp.path("c.kinewright")))
                .unwrap();
        }
        let actor = app.owner.core().actor_identity();
        let history = app.owner.core().export_history().unwrap();
        let dirty = app.owner.dirty().unwrap();
        let slots = app.owner.rescue_slots();
        assert!(
            kinewright_project::editing_session::Ja1EditingSession::open(
                &temp.path("b.kinewright")
            )
            .is_err()
        );
        app.apply(Action::PreviewRelease(temp.path("b.kinewright")))
            .unwrap();
        assert!(app.apply(Action::Release).is_err());
        app.apply(Action::Reconcile(temp.path("b.kinewright")))
            .unwrap();
        let text = rendered(&app);
        assert!(text.contains("as of capture"));
        assert!(text.contains("separately read-only"));
        assert_eq!(app.owner.core().actor_identity(), actor);
        assert_eq!(app.owner.core().export_history().unwrap(), history);
        assert_eq!(app.owner.dirty().unwrap(), dirty);
        app.apply(Action::PreviewRelease(temp.path("b.kinewright")))
            .unwrap();
        app.apply(Action::Release).unwrap();
        assert_eq!(app.owner.rescue_slots(), slots);
        assert_eq!(app.owner.core().export_history().unwrap(), history);
        let mut separate = kinewright_project::editing_session::Ja1EditingSession::open(
            &temp.path("b.kinewright"),
        )
        .unwrap();
        separate.save_active().unwrap();
        drop(separate);
        std::fs::rename(temp.path("hidden-drive"), temp.path("drive")).unwrap();
        assert!(
            kinewright_project::editing_session::Ja1EditingSession::open(
                &temp.path("drive/a.kinewright")
            )
            .is_err()
        );
    }
}

#[cfg(target_os = "linux")]
pub(crate) fn observe_s2(revised: bool) -> serde_json::Value {
    let (temp, mut app) = named();
    let control = LoadedBundle::read(&temp.path("drive/a.kinewright"))
        .unwrap()
        .admission()
        .inspection_document()
        == Some(&doc());
    if !revised {
        return serde_json::json!({"control_reopened":control,"surface":"pending_baseline_witness"});
    }
    unknown_selection(&mut app, &temp, true);
    app.apply(Action::PreviewRescue).unwrap();
    let text = rendered(&app);
    let original = app.owner.core().clone();
    let captured = original.export_history().unwrap();
    app.apply(Action::RescueTo(temp.path("b.kinewright")))
        .unwrap();
    let fresh = Core::spawn(Document::default()).unwrap();
    LoadedBundle::read(&temp.path("b.kinewright"))
        .unwrap()
        .restore_manual(&fresh)
        .unwrap();
    serde_json::json!({"control_reopened":control,"app_rescue":original.attachment_state().unwrap() == AttachmentState::Sealed && original.actor_identity() != app.owner.core().actor_identity() && fresh.export_history().unwrap() == captured && text.contains("not in B") && text.contains("A keeps it only if the selection completed")})
}

#[test]
#[cfg_attr(
    not(feature = "slow-tests"),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn rescue_complete_app_lifecycle_stress() {
    #[cfg(target_os = "linux")]
    for _ in 0..4 {
        w15_late_rescue_is_capture_only_and_release_is_explicit();
        w15_completed_capture_release_keeps_sources_and_active_c_exact();
        rescue_cases::w13_one_level_rescue_recopy_and_explicit_ack_retirement();
    }
    #[cfg(not(target_os = "linux"))]
    w14_unsupported_rescue_refuses_before_session_mutation();
}
