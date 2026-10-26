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

#[test]
fn c9_fix1_manual_gate_controls_the_save_requirement_message() {
    let attached = manual_status_message("Studio writer host attached.", false);
    assert!(attached.contains("Save once to keep editing here"));
    assert!(attached.contains("timeline is read-only"));
    assert!(!attached.contains("Your manual edit stays"));
    assert_eq!(
        manual_status_message(&attached, true),
        "Studio writer host attached."
    );
    assert_eq!(manual_status_message(&attached, false), attached);
    assert_eq!(
        manual_status_message("Saved the current edit.", true),
        "Saved the current edit."
    );
    let refused = manual_status_message("The destination could not be saved.", false);
    assert!(refused.starts_with("The destination could not be saved."));
    assert!(refused.contains("Save once"));
}

#[cfg(target_os = "linux")]
#[test]
fn c9_fix1_unsaved_studio_activation_requires_first_save_before_manual_edit() {
    crate::app::ja_c9_bootstrap_save_witness(false);
}

#[cfg(target_os = "linux")]
#[test]
fn c9_fix1_dirty_named_studio_activation_requires_save_before_manual_edit() {
    crate::app::ja_c9_bootstrap_save_witness(true);
}

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
#[test]
fn n2_sealed_a_discloses_committed_b_and_offers_open() {
    let (temp, mut app) = named();
    let original = app.owner.core().clone();
    edit(&original, false);
    assert!(
        kinewright_project::io_observer::fail_once("root_parent_sync", || app.apply(Action::Save))
            .is_err()
    );
    std::fs::rename(temp.path("drive"), temp.path("hidden-drive")).unwrap();
    app.apply(Action::PreviewRescue).unwrap();
    let Some(Choice::Rescue {
        selection, consent, ..
    }) = app.choice.take()
    else {
        panic!("rescue choice")
    };
    let loss = rescue_preview(&selection);
    let path = temp.path("b.kinewright");
    let inspection = app.owner.inspect_rescue_destination(&path).unwrap();
    let binder = ProjectBundle::activate(
        &temp.path("cut-authority.kinewright"),
        BundleWriteAdmission::new_document(),
    )
    .unwrap();
    let outcome = app
        .owner
        .rescue_with_activation(*selection, &consent, &inspection, |core| {
            let authority = binder.attach_core(core)?;
            let cut = authority.capture().unwrap();
            authority.resume(&cut).unwrap();
            edit(core, true);
            authority.freeze().unwrap();
            Ok(())
        })
        .unwrap();
    app.finish_rescue(&outcome, None, &path, &loss);
    app.refresh();
    let text = rendered(&app);
    assert!(text.contains("A is sealed"), "{text}");
    assert!(text.contains("committed rescue copy"), "{text}");
    assert!(text.contains(&path.display().to_string()), "{text}");
    assert!(
        !text.contains("Editing continues in the original session"),
        "{text}"
    );
    assert!(!text.contains("B's save outcome is unknown"), "{text}");
    let action = click_control(&app, &format!("Open B read-only: {}", path.display()));
    assert!(matches!(action, Action::OpenCapture(ref p) if p == &path));
    assert_eq!(
        original.attachment_state().unwrap(),
        AttachmentState::Sealed
    );
    assert!(app.owner.is_capture_destination(&path));
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

#[cfg(target_os = "linux")]
fn click_control(app: &Ja1Attachment, label: &str) -> Action {
    fn locate(shape: &egui::epaint::Shape, label: &str) -> Option<egui::Pos2> {
        match shape {
            egui::epaint::Shape::Text(t) if t.galley.job.text == label => {
                Some(t.pos + t.galley.size() * 0.5)
            }
            egui::epaint::Shape::Vec(shapes) => shapes.iter().find_map(|s| locate(s, label)),
            _ => None,
        }
    }
    let ctx = egui::Context::default();
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1600.0, 3000.0),
        )),
        ..Default::default()
    };
    let _ = ctx.run_ui(input.clone(), |ui| {
        app.show(ui);
    });
    let output = ctx.run_ui(input.clone(), |ui| {
        app.show(ui);
    });
    let at = output
        .shapes
        .iter()
        .find_map(|s| locate(&s.shape, label))
        .unwrap_or_else(|| {
            panic!(
                "missing rendered control {label}: {}",
                painted_text(&output)
            )
        });
    let mut pressed = input.clone();
    pressed.events = vec![
        egui::Event::PointerMoved(at),
        egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        },
    ];
    let _ = ctx.run_ui(pressed, |ui| {
        app.show(ui);
    });
    let mut released = input;
    released.events = vec![
        egui::Event::PointerMoved(at),
        egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        },
    ];
    let mut action = None;
    let _ = ctx.run_ui(released, |ui| {
        action = app.show(ui);
    });
    action.unwrap_or_else(|| panic!("rendered control {label} did not dispatch"))
}
#[test]
#[cfg(target_os = "linux")]
fn r3_rendered_ordinary_first_and_prepared_retry_offer_preview() {
    use kinewright_project::io_observer;
    for mode in ["ordinary", "first", "prepared"] {
        let (temp, mut app) = if mode == "first" {
            let temp = temp();
            let core = Core::spawn(doc()).unwrap();
            let owner = kinewright_project::editing_session::Ja1EditingSession::new(
                temp.root(),
                &core,
                StorageLimits::default(),
            )
            .unwrap();
            (temp, Ja1Attachment::new(owner).unwrap())
        } else {
            named()
        };
        let path = temp.path("drive/a.kinewright");
        if mode == "prepared" {
            unknown_selection(&mut app, &temp, true);
            app.offer = None;
        } else {
            edit(app.owner.core(), false);
            let action = if mode == "first" {
                Action::FirstSave(path)
            } else {
                Action::Save
            };
            let result = io_observer::fail_once("root_parent_sync", || app.apply(action));
            if mode == "ordinary" {
                assert!(result.is_err());
            } else {
                result.unwrap();
            }
            std::fs::rename(temp.path("drive"), temp.path("hidden-drive")).unwrap();
            assert!(app.offer.is_none());
        }
        assert!(app.choice.is_none());
        let retry = click_control(&app, "Retry A / selection");
        let result = app.apply(retry);
        if mode != "prepared" {
            assert!(result.is_err(), "preserve the ordinary Retry error");
        }
        assert!(
            app.offer
                .as_ref()
                .is_some_and(|p| p.eligibility == RescueEligibility::Eligible),
            "failed Retry must retain an eligible offer for {mode}"
        );
        let preview = click_control(&app, "Save rescue copy");
        app.apply(preview).unwrap();
        assert!(matches!(app.choice, Some(Choice::Rescue { .. })));
        assert!(rendered(&app).contains("rescue"));
        if mode == "prepared" {
            assert_eq!(
                app.owner.core().attachment_state().unwrap(),
                AttachmentState::Frozen
            );
        }
    }
}
#[test]
#[cfg(target_os = "linux")]
fn r1_app_worker_save_suffix_warns_at_close() {
    crate::app::ja1_review_save_suffix_witness();
}
#[test]
#[cfg(target_os = "linux")]
fn r2_studio_dirty_polls_cache_authoritative_history_changes() {
    crate::app::ja1_review_dirty_poll_witness();
}
#[test]
#[cfg(target_os = "linux")]
fn r8_worker_panic_status_matches_dropped_attachment() {
    crate::app::ja1_review_worker_panic_witness();
}

#[test]
fn g04_real_gui_journal_cannot_appear_after_owner_scan() {
    let observed = crate::recovery::g04_gui_journal_observation().unwrap();
    assert_eq!(
        observed["control_appended"], true,
        "real named GUI journal control must append"
    );
    assert_eq!(observed["project_unchanged"], true);
    assert_eq!(
        observed["post_scan_excluded"], true,
        "a real GUI writer must not create any base, suffix or alias-header journal after the owner's scan"
    );
}

#[cfg(unix)]
#[test]
fn g04_gui_journal_lost_authority_refuses_create_append_and_retire() {
    crate::recovery::g04_gui_lost_authority();
}

#[cfg(unix)]
#[test]
fn g04_gui_journal_directory_remains_pinned_across_checkpoint() {
    crate::recovery::g04_gui_directory_stays_pinned();
}

#[cfg(unix)]
#[test]
fn g04_startup_journal_retirement_uses_scanned_parent_and_bytes() {
    crate::recovery::g04_startup_retirement_keeps_scanned_identity();
}

/// App-level L2 fence: a real GUI source admission, exact history and all action routes.
/// Process kill/exit/unwind transport witnesses are owned by the parent's host gate.
#[cfg(target_os = "linux")]
#[test]
fn c8_retention_loss_fences_every_action_and_keeps_actual_source_until_close() {
    use std::io::Read as _;
    let temp = temp();
    let core = Core::spawn(doc()).unwrap();
    edit(&core, false);
    core.request(Command::Undo).unwrap();
    let owner = kinewright_project::editing_session::Ja1EditingSession::new(
        temp.root(),
        &core,
        StorageLimits::default(),
    )
    .unwrap();
    let mut app = Ja1Attachment::new(owner).unwrap();
    let history = core.export_history().unwrap();
    let transfer = app.owner.prepare_host_attachment().unwrap();
    let descriptor = transfer.source.unwrap();
    let service = transfer.source_service.unwrap();
    let input = transfer.inputs;
    let source = app.owner.source().unwrap();
    let source_root = source
        .store()
        .resolved_root()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    assert!(
        source_root.is_dir(),
        "setup: the actual private session directory exists"
    );
    let source_bytes = admission::tree(&source_root);
    assert!(
        !source_bytes.is_empty(),
        "setup: retained source data exists before failure"
    );
    assert!(
        source_bytes
            .values()
            .any(|bytes| kinewright_media::sha256_bytes(bytes) == input.sha256),
        "setup: the admitted input bytes really exist"
    );
    let usage = source.store().usage_handle();
    let pins = source.store().retention_probe().unwrap();
    let charged = usage().unwrap();
    assert!(charged.physical_bytes > 0);
    assert!(charged.reserved_bytes > 0);
    assert!(!pins.1.is_empty());
    let mut reader = service.open_borrowed_source(&descriptor, &input).unwrap();
    let mut exact = Vec::new();
    reader.read_to_end(&mut exact).unwrap();
    assert_eq!(kinewright_media::sha256_bytes(&exact), input.sha256);

    app.operation_panicked();
    assert!(app.read_only());
    assert_eq!(core.attachment_state(), Ok(AttachmentState::Frozen));
    let branch = BranchId::fresh().unwrap();
    let destination = temp.path("fresh/never-created.kinewright");
    let actions = vec![
        Action::Save,
        Action::ConfirmSuccessorSave,
        Action::SaveAs(destination.clone()),
        Action::FirstSave(destination.clone()),
        Action::Retry,
        Action::PreviewRecovery(RecoverySelection::WorkingBranch(branch.clone())),
        Action::OpenRecovery,
        Action::Abandon,
        Action::PreviewRescue,
        Action::ConsentTrim,
        Action::ConsentOmission,
        Action::RescueTo(destination.clone()),
        Action::Cancel,
        Action::PreviewRetirement(branch.clone()),
        Action::RemoveBranch,
        Action::PreviewForget,
        Action::Forget,
        Action::Reconcile(destination.clone()),
        Action::PreviewRelease(destination.clone()),
        Action::Release,
        Action::SaveSession(branch.clone()),
        Action::CloseSession(branch),
    ];
    for action in actions {
        assert!(app.apply(action).is_err());
        assert!(
            !destination.parent().unwrap().exists(),
            "refused before parent creation"
        );
        assert_eq!(core.export_history().unwrap(), history);
        assert_eq!(admission::tree(&source_root), source_bytes);
        assert_eq!(
            app.owner
                .source()
                .unwrap()
                .store()
                .retention_probe()
                .unwrap(),
            pins
        );
        let now = usage().unwrap();
        assert_eq!(now.physical_bytes, charged.physical_bytes);
        assert_eq!(now.reserved_bytes, charged.reserved_bytes);
        assert_eq!(now.reserved_copy_bytes, charged.reserved_copy_bytes);
    }
    assert!(matches!(
        core.request(Command::Query(Query::Snapshot)).unwrap(),
        Event::QueryResult(_)
    ));
    assert!(matches!(
        core.request(Command::Redo).unwrap(),
        Event::MutationRefused { .. }
    ));
    assert!(rendered(&app).contains("retained data"));
    assert!(
        app.close_text()
            .contains("Close this editing context explicitly")
    );
    assert!(app.destination_change().is_err());
    let mut after = vec![0; exact.len()];
    assert_eq!(
        service
            .read_borrowed_source(&descriptor, &input, 0, &mut after)
            .unwrap(),
        exact.len()
    );
    assert_eq!(after, exact);

    assert!(!app.close());
    assert!(!app.close());
    assert!(
        service
            .read_borrowed_source(&descriptor, &input, 0, &mut [])
            .is_err()
    );
    assert!(
        source_root.exists(),
        "an actual reader still holds the source namespace"
    );
    assert_eq!(usage().unwrap().reserved_bytes, charged.reserved_bytes);
    assert_eq!(admission::tree(&source_root), source_bytes);
    drop(reader);
    assert!(
        app.close(),
        "same actual owner retries explicit Close after reader retirement"
    );
    assert!(app.close(), "completed Close is idempotent");
    assert!(!source_root.exists());
    assert_eq!(usage().unwrap().reserved_bytes, 0);
    drop(app);
}

/// A successful real loopback first Save precedes the App's host-loss refusal control.
#[cfg(target_os = "linux")]
#[test]
fn c8_app_host_first_save_then_orderly_exit_preserves_gui_source_and_history() {
    use kinewright_agent::writer_host::{HostCommand, SessionRoots};
    let temp = temp();
    let core = Core::spawn(doc()).unwrap();
    let owner = kinewright_project::editing_session::Ja1EditingSession::new(
        temp.root(),
        &core,
        StorageLimits::default(),
    )
    .unwrap();
    let mut app = Ja1Attachment::new(owner).unwrap();
    let test_binary = std::env::current_exe().unwrap();
    let executable = test_binary
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("kinewright");
    assert!(
        executable.is_file(),
        "setup: build the shipping kinewright host first"
    );
    app.launch_host_at(
        &executable,
        "app-c8-owner".into(),
        SessionRoots {
            read: vec![temp.root().to_path_buf()],
            write: vec![temp.root().to_path_buf()],
        },
    )
    .expect("the exact GUI snapshot must reach the shipping host bootstrap");
    let attached = app
        .host_client()
        .unwrap()
        .request(HostCommand::ReadHistory)
        .unwrap();
    assert!(attached.error.is_none());
    assert_eq!(attached.history.unwrap(), core.export_history().unwrap());
    let source_root = app
        .owner
        .source()
        .unwrap()
        .store()
        .resolved_root()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    assert!(
        source_root.is_dir(),
        "setup: actual GUI source directory exists before first Save"
    );
    let source_bytes = admission::tree(&source_root);
    assert!(
        !source_bytes.is_empty(),
        "setup: actual retained source bytes exist"
    );
    assert!(source_bytes.values().any(|bytes| !bytes.is_empty()));
    let path = temp.path("a.kinewright");
    // Regression: attachment precedes edits; FirstSave must grow the real owner's
    // source overlap for this exact foreground cut, not rely on the initial empty history.
    assert_eq!(core.attachment_state(), Ok(AttachmentState::Frozen));
    app.apply(Action::FirstSave(path.clone())).unwrap();
    assert!(
        !app.is_dirty(),
        "only the publisher digest marks this exact history Saved"
    );
    assert_eq!(app.project_path().as_deref(), Some(path.as_path()));
    let reopened = Core::spawn(Document::default()).unwrap();
    LoadedBundle::read(&path)
        .unwrap()
        .restore_manual(&reopened)
        .unwrap();
    assert_eq!(
        reopened.export_history().unwrap(),
        core.export_history().unwrap()
    );
    assert!(
        source_root.exists(),
        "first Save keeps the original GUI source owner"
    );
    assert_eq!(admission::tree(&source_root), source_bytes);
    let pins = app
        .owner
        .source()
        .unwrap()
        .store()
        .retention_probe()
        .unwrap();
    let usage = app.owner.source().unwrap().store().usage_handle();
    edit_attachment(&app, false);
    let unsaved = core.export_history().unwrap();
    assert!(app.is_dirty());
    // The next save must admit this larger exact cut even if transport then fails.
    // Measure retained ownership after that admission, before the host Drop path.
    let cut = app.owner.freeze_host_cut().unwrap();
    app.host.as_mut().unwrap().cut = Some(cut);
    let retained = usage().unwrap();

    let host_pid = app.host.as_ref().unwrap().child_id().unwrap();
    app.host.as_mut().unwrap().close(); // the real shipping child exits and is reaped
    assert!(
        !Path::new(&format!("/proc/{host_pid}")).exists(),
        "orderly host exit must reap its actual child, including a zombie"
    );
    let elsewhere = temp.path("fresh/b.kinewright");
    assert!(app.apply(Action::SaveAs(elsewhere.clone())).is_err());
    assert!(app.apply(Action::FirstSave(elsewhere.clone())).is_err());
    assert!(app.apply(Action::PreviewRescue).is_err());
    assert!(!elsewhere.parent().unwrap().exists());
    assert_eq!(core.export_history().unwrap(), unsaved);
    assert_eq!(core.attachment_state(), Ok(AttachmentState::Frozen));
    assert_eq!(
        app.owner
            .source()
            .unwrap()
            .store()
            .retention_probe()
            .unwrap(),
        pins
    );
    let now = usage().unwrap();
    assert_eq!(now.physical_bytes, retained.physical_bytes);
    assert_eq!(now.reserved_copy_bytes, retained.reserved_copy_bytes);
    assert_eq!(now.reserved_bytes, retained.reserved_bytes);
    assert!(source_root.exists());
    assert_eq!(admission::tree(&source_root), source_bytes);
    assert!(app.close_text().contains("Saving and rescue are paused"));
    app.close();
    drop(app);
    assert!(!source_root.exists());
}

#[cfg(target_os = "linux")]
#[test]
fn c8_failed_host_launch_keeps_source_and_restores_local_editing() {
    let temp = temp();
    let core = Core::spawn(doc()).unwrap();
    edit(&core, false);
    let owner = kinewright_project::editing_session::Ja1EditingSession::new(
        temp.root(),
        &core,
        StorageLimits::default(),
    )
    .unwrap();
    let mut app = Ja1Attachment::new(owner).unwrap();
    let virtual_root = app.owner.source().unwrap().store().resolved_root().unwrap();
    let root = virtual_root.parent().unwrap().to_path_buf();
    assert!(
        root.is_dir(),
        "setup: actual source directory exists before launch"
    );
    let source_bytes = admission::tree(&root);
    assert!(
        !source_bytes.is_empty(),
        "setup: actual source data exists before launch"
    );
    assert!(source_bytes.values().any(|bytes| !bytes.is_empty()));
    let initial_physical = app
        .owner
        .source()
        .unwrap()
        .store()
        .usage()
        .unwrap()
        .physical_bytes;
    assert!(initial_physical > 0);
    let history = core.export_history().unwrap();
    let roots = kinewright_agent::writer_host::SessionRoots {
        read: vec![temp.root().to_path_buf()],
        write: vec![temp.root().to_path_buf()],
    };
    assert!(
        app.launch_host_at(
            &temp.path("absent-host-binary"),
            "launch-failure".into(),
            roots
        )
        .is_err()
    );
    let source = app.owner.source().unwrap();
    assert_eq!(source.store().resolved_root().unwrap(), virtual_root);
    assert_eq!(admission::tree(&root), source_bytes);
    assert_eq!(
        source.store().usage().unwrap().physical_bytes,
        initial_physical
    );
    assert!(root.exists());
    assert!(source.has_borrowed_sources());
    assert!(source.store().usage().unwrap().reserved_copy_bytes > 0);
    assert!(!app.read_only());
    assert_eq!(core.attachment_state(), Ok(AttachmentState::Live));
    assert_eq!(core.export_history().unwrap(), history);
    let fresh = temp.path("elsewhere/new.kinewright");
    app.apply(Action::FirstSave(fresh.clone())).unwrap();
    let reopened = Core::spawn(Document::default()).unwrap();
    LoadedBundle::read(&fresh)
        .unwrap()
        .restore_manual(&reopened)
        .unwrap();
    assert_eq!(reopened.export_history().unwrap(), history);
    assert_eq!(admission::tree(&root), source_bytes);
    app.close();
    drop(app);
    assert!(!root.exists());
}

/// Completion adapter: exact publisher-written floors stay on the existing GUI actor;
/// an explicit committed recovery activation creates a fresh actor even for equal content.
#[cfg(target_os = "linux")]
#[test]
fn c8_app_publisher_floor_keeps_actor_and_committed_recovery_activates_fresh_actor() {
    use kinewright_agent::writer_host::{
        HostConfig, HostOrigin, HostReply, HostSaveReport, HostSourceBinding, HostStatus,
        SessionRoots, SourceServer, WriterHost,
    };
    use kinewright_core::HistoryTransition;
    let temp = temp();
    let core = Core::spawn(doc()).unwrap();
    for n in 0..10 {
        edit(&core, n % 2 == 0);
    }
    let before = core.export_history().unwrap();
    let owner = kinewright_project::editing_session::Ja1EditingSession::new(
        temp.root(),
        &core,
        StorageLimits::default(),
    )
    .unwrap();
    let mut app = Ja1Attachment::new(owner).unwrap();
    let transfer = app.owner.prepare_host_attachment().unwrap();
    let server = SourceServer::start(transfer.source_service.unwrap()).unwrap();
    let writer = WriterHost::start(HostConfig {
        project: "app-c8-floor-activation".into(),
        path: None,
        roots: SessionRoots {
            read: vec![temp.root().to_path_buf()],
            write: vec![temp.root().to_path_buf()],
        },
        origin: HostOrigin::Gui {
            initial_snapshot: kinewright_agent::writer_host::HostSnapshot::from_history(
                transfer.history.clone(),
            )
            .unwrap(),
            retention: HostSourceBinding {
                source: transfer.source.clone(),
                inputs: Some(transfer.inputs.clone()),
                callback: server.bootstrap(),
            },
            expected_root_sha256: None,
            expected_project_id: None,
        },
    })
    .unwrap();
    let initial = app.owner.freeze_host_cut().unwrap();
    app.owner.complete_host_cut(&initial, None, None).unwrap();
    app.host = Some(
        host::HostAttachment::connected(
            writer.client(),
            transfer.source,
            transfer.inputs,
            server,
            Arc::downgrade(&app.originating_context),
        )
        .unwrap(),
    );
    app.frozen = false;

    // This report is from a real constrained publisher, rather than a live GUI history hash.
    let publisher_core = Core::spawn(Document::default()).unwrap();
    publisher_core.restore_history(before.clone()).unwrap();
    let mut publisher = kinewright_project::editing_session::Ja1EditingSession::new(
        temp.root(),
        &publisher_core,
        StorageLimits::default(),
    )
    .unwrap();
    let selected = HistoryTransition::floor(before.clone(), 5).unwrap();
    let limits = StorageLimits {
        branch_history_bytes: kinewright_project::bundle_commit::history_cost_probe(
            selected.after(),
            &StorageLimits::default(),
        )
        .unwrap(),
        ..StorageLimits::default()
    };
    let path = temp.path("committed_history-floor.kinewright");
    let CopyStatus::Committed(report) = publisher.first_save(&path, limits).unwrap() else {
        panic!("real constrained publisher must commit");
    };
    let committed_history = publisher_core.export_history().unwrap();
    assert!(committed_history.floor_offset > before.floor_offset);
    assert_eq!(
        kinewright_media::sha256_bytes(&serde_json::to_vec(&committed_history).unwrap()),
        report.history_sha256
    );
    let actor = core.actor_identity();
    let cut = app.owner.freeze_host_cut().unwrap();
    app.host.as_mut().unwrap().cut = Some(cut);
    app.accept_host_reply(HostReply {
        status: HostStatus {
            retention_proven: true,
            association: Some(path),
            saved_digest: Some(report.history_sha256.clone()),
            ..HostStatus::default()
        },
        saved: Some(HostSaveReport::from(&report)),
        history: Some(committed_history.clone()),
        ..HostReply::default()
    })
    .unwrap();
    assert_eq!(app.owner.core().actor_identity(), actor);
    assert_eq!(core.export_history().unwrap(), committed_history);
    assert_eq!(app.owner.retained_host_actor_count(), 0);

    // Continue through actual App -> loopback host commands for recovery, using the original
    // host's own newly named branch. Its cut is this exact, already-floor-installed GUI history.
    app.apply(Action::FirstSave(temp.path("host-a.kinewright")))
        .unwrap();
    for value in [true, false, true] {
        edit_attachment(&app, value);
        app.apply(Action::Save).unwrap();
    }
    let target = app
        .host
        .as_ref()
        .unwrap()
        .status
        .recovery_points
        .last()
        .unwrap()
        .clone();
    app.apply(Action::PreviewRecovery(target.into())).unwrap();
    let prior = app.owner.core().clone();
    app.apply(Action::OpenRecovery).unwrap();
    assert_ne!(app.owner.core().actor_identity(), prior.actor_identity());
    assert_eq!(prior.attachment_state(), Ok(AttachmentState::Sealed));
    assert_eq!(
        app.owner.core().attachment_state(),
        Ok(AttachmentState::Live)
    );
    assert_eq!(
        app.owner.retained_host_actor_count(),
        0,
        "recovery retains no hidden old GUI actor"
    );
    assert!(
        app.owner.source().is_some(),
        "the exact originating source stays with the same owner"
    );
    assert!(!app.is_dirty());
    app.close();
    writer.shutdown().unwrap();
}

/// The existing file-picker action authorizes a destination outside initial roots;
/// forwarded roots without that exact GUI admission still refuse before mutation.
#[cfg(target_os = "linux")]
#[test]
fn c8_person_picked_external_first_save_and_save_as_keep_source_and_narrowed_authority() {
    use kinewright_agent::writer_host::{
        HostCommand, HostConfig, HostOrigin, HostSnapshot, HostSourceBinding, SessionRoots,
        SourceServer, WriterHost,
    };
    let temp = temp();
    let allowed = temp.path("initial-root");
    let external = temp.path("external-root");
    let other = temp.path("other-root");
    for directory in [&allowed, &external, &other] {
        std::fs::create_dir_all(directory).unwrap();
    }
    let core = Core::spawn(doc()).unwrap();
    let owner = kinewright_project::editing_session::Ja1EditingSession::new(
        &allowed,
        &core,
        StorageLimits::default(),
    )
    .unwrap();
    let mut app = Ja1Attachment::new(owner).unwrap();
    let source_directory = app
        .owner
        .source()
        .unwrap()
        .store()
        .resolved_root()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    assert!(source_directory.is_dir());
    let original_bytes = admission::tree(&source_directory);
    assert!(!original_bytes.is_empty());
    let transfer = app.owner.prepare_host_attachment().unwrap();
    let server = SourceServer::start(transfer.source_service.unwrap()).unwrap();
    let initial_roots = SessionRoots {
        read: vec![allowed.clone()],
        write: vec![allowed.clone()],
    };
    let writer = WriterHost::start(HostConfig {
        project: "app-person-destination".into(),
        path: None,
        roots: initial_roots,
        origin: HostOrigin::Gui {
            initial_snapshot: kinewright_agent::writer_host::HostSnapshot::from_history(
                transfer.history.clone(),
            )
            .unwrap(),
            retention: HostSourceBinding {
                source: transfer.source.clone(),
                inputs: Some(transfer.inputs.clone()),
                callback: server.bootstrap(),
            },
            expected_root_sha256: None,
            expected_project_id: None,
        },
    })
    .unwrap();
    let cut = app.owner.freeze_host_cut().unwrap();
    app.owner.complete_host_cut(&cut, None, None).unwrap();
    app.host = Some(
        host::HostAttachment::connected(
            writer.client(),
            transfer.source,
            transfer.inputs,
            server,
            Arc::downgrade(&app.originating_context),
        )
        .unwrap(),
    );
    app.frozen = false;
    edit(&core, false); // larger history than the attachment's initial snapshot
    let first = external.join("a.kinewright");
    app.apply(Action::FirstSave(first.clone())).unwrap();
    assert!(!app.is_dirty());
    assert_eq!(app.project_path().as_deref(), Some(first.as_path()));
    assert_eq!(admission::tree(&source_directory), original_bytes);

    let refused = other.join("unapproved.kinewright");
    let widened_forwarding = writer.client().with_roots(SessionRoots {
        read: vec![allowed.clone(), external.clone(), other.clone()],
        write: vec![allowed, external.clone(), other.clone()],
    });
    let reply = widened_forwarding
        .request(HostCommand::SaveAs {
            destination: refused.clone(),
            snapshot: HostSnapshot::capture(&core).unwrap(),
        })
        .unwrap();
    assert!(
        reply.error.is_some(),
        "forwarded roots alone never authorize an external destination"
    );
    assert!(!refused.exists());
    assert!(admission::tree(&other).is_empty());

    edit_attachment(&app, true);
    let second = other.join("person-picked.kinewright");
    app.apply(Action::SaveAs(second.clone())).unwrap();
    assert_eq!(app.project_path().as_deref(), Some(second.as_path()));
    assert!(!app.is_dirty());
    let reopened = Core::spawn(Document::default()).unwrap();
    LoadedBundle::read(&second)
        .unwrap()
        .restore_manual(&reopened)
        .unwrap();
    assert_eq!(
        reopened.export_history().unwrap(),
        core.export_history().unwrap()
    );
    assert_eq!(admission::tree(&source_directory), original_bytes);
    app.apply(Action::Save).unwrap(); // later IO uses only the host-admitted current scope
    let pins = app
        .owner
        .source()
        .unwrap()
        .store()
        .retention_probe()
        .unwrap();
    let usage = app.owner.source().unwrap().store().usage().unwrap();
    app.operation_panicked();
    let never = temp.path("never-admitted/new.kinewright");
    assert!(app.apply(Action::FirstSave(never.clone())).is_err());
    assert!(app.apply(Action::SaveAs(never.clone())).is_err());
    assert!(app.apply(Action::RescueTo(never.clone())).is_err());
    assert!(!never.parent().unwrap().exists());
    assert_eq!(admission::tree(&source_directory), original_bytes);
    assert_eq!(
        app.owner
            .source()
            .unwrap()
            .store()
            .retention_probe()
            .unwrap(),
        pins
    );
    let retained = app.owner.source().unwrap().store().usage().unwrap();
    assert_eq!(retained.physical_bytes, usage.physical_bytes);
    assert_eq!(retained.reserved_bytes, usage.reserved_bytes);
    assert!(app.close());
    writer.shutdown().unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn g05l_live_coalesced_preview_precedes_disk_ack() {
    use kinewright_agent::ja_harness::protocol::Mode;
    let temp = temp();
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../kinewright-agent/tests/fixtures/ja/s2-manual-stream.json");
    // This App-private bridge runs the same additive fixture through the real
    // producer, Core, media engine, presenter callback, shipping child and publisher.
    for (mode, name) in [(Mode::Baseline, "baseline"), (Mode::Revised, "revised")] {
        let output = std::env::var_os("JA_MANUAL_APP_OUTPUT")
            .map_or_else(|| temp.path(name), |root| PathBuf::from(root).join(name));
        std::fs::create_dir_all(output.parent().unwrap()).unwrap();
        assert_eq!(
            crate::ja_harness::run(&fixture, &output, mode).unwrap(),
            0,
            "{}",
            output.display()
        );
        let observation: serde_json::Value = serde_json::from_slice(
            &std::fs::read(output.join("artifacts/observation-1.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            observation["center_rgba"],
            serde_json::json!([255, 0, 0, 255])
        );
        if mode == Mode::Revised {
            assert_eq!(observation["manual_stream"]["complete"], true);
            assert_eq!(
                observation["manual_stream"]["painted_frames"][0]["center_rgba"],
                serde_json::json!([0, 255, 0, 255])
            );
            assert_eq!(
                observation["manual_stream"]["painted_frames"][1]["center_rgba"],
                serde_json::json!([255, 0, 0, 255])
            );

            assert_eq!(
                observation["manual_stream"]["accepted"]
                    .as_array()
                    .unwrap()
                    .len(),
                2
            );
        }
    }
}

#[cfg(target_os = "linux")]
fn edit_attachment(app: &Ja1Attachment, locked: bool) {
    app.send_manual(Command::Do(Operation::SetTrackSyncLock {
        track: TrackId(1),
        locked,
    }))
    .unwrap();
    assert_eq!(
        app.owner.core().export_history().unwrap().document.tracks[0].sync_lock,
        locked
    );
}

#[cfg(target_os = "linux")]
#[test]
fn g05l_w05_all_manual_producers_respect_the_actor_cut() {
    crate::app::ja_c9_manual_producer_gate_witness();
}

#[cfg(target_os = "linux")]
#[test]
fn g05l_every_producer_stays_frozen_until_equivalence() {
    for floor in [false, true] {
        observe_c10(floor);
    }
}

#[cfg(target_os = "linux")]
pub(crate) fn observe_c10(floor: bool) -> serde_json::Value {
    use kinewright_agent::writer_host::{HostCommand, SessionRoots};
    let temp = temp();
    let core = Core::spawn(doc()).unwrap();
    let mut owner = kinewright_project::editing_session::Ja1EditingSession::new(
        temp.root(),
        &core,
        StorageLimits::default(),
    )
    .unwrap();
    let path = temp.path("c10.kinewright");
    if !floor {
        owner.first_save(&path, StorageLimits::default()).unwrap();
    }
    let mut app = Ja1Attachment::new(owner).unwrap();
    app.launch_host_at(
        &host::executable(),
        "c10-gui".into(),
        SessionRoots {
            read: vec![temp.root().into()],
            write: vec![temp.root().into()],
        },
    )
    .unwrap();
    if floor {
        app.apply(Action::FirstSave(path.clone())).unwrap();
    }
    let source_usage = app.owner.source().map(|s| s.store().usage().unwrap());
    let gate = app
        .host
        .as_ref()
        .unwrap()
        .manual
        .as_ref()
        .unwrap()
        .permit
        .clone();
    let operation = Operation::SetTrackSyncLock {
        track: TrackId(1),
        locked: false,
    };
    gate.send(Command::Do(operation.clone())).unwrap();
    // The actor barrier drains the submitted send; it cannot become a post-cut edit.
    let cut = app.owner.freeze_host_cut().unwrap();
    assert_eq!(cut.history().batches.len(), 1);
    app.host.as_mut().unwrap().cut = Some(cut.clone());
    let commands = [
        Command::Do(operation.clone()),
        Command::DoBatch(vec![operation.clone()]),
        Command::DoBatchCoalesced {
            operations: vec![operation.clone()],
            coalesce_key: "live-control".into(),
        },
        Command::Undo,
        Command::Redo,
        Command::DoIfRevision {
            expected: cut.revision(),
            operation: operation.clone(),
            token: None,
        },
        Command::DoBatchIfRevision {
            expected: cut.revision(),
            operations: vec![operation],
        },
    ];
    let producer_count = commands.len();
    let mut refused = 0;
    for command in commands {
        assert!(app.read_only());
        app.send_manual(command.clone()).unwrap();
        assert!(
            matches!(
                gate.request(command.clone()).unwrap(),
                Event::MutationRefused {
                    reason: kinewright_core::CoreMutationRefusal::Frozen,
                    ..
                }
            ),
            "{command:?}"
        );
        refused += 1;
        core.send(command).unwrap();
        assert_eq!(core.export_history().unwrap(), *cut.history());
    }
    assert!(
        app.owner
            .install_host_snapshot(cut.history().clone())
            .is_err()
    );
    let client = app.host_client().unwrap();
    assert!(
        client
            .request(HostCommand::ReadHistory)
            .unwrap()
            .error
            .is_none()
    );
    app.host.as_mut().unwrap().kill_owned_child();
    app.host
        .as_mut()
        .unwrap()
        .begin_manual_recovery_with_floor(&mut app.owner, floor.then_some(1))
        .unwrap();
    assert!(app.apply(Action::Save).is_err());
    assert!(
        !app.retention_lost,
        "a refused action during recovery cannot discard proven source ownership"
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    loop {
        app.poll_host();
        if !app.read_only() {
            break;
        }
        assert!(
            app.host.as_ref().unwrap().recovery_running(),
            "recovery finished without equivalence: {}",
            app.message
        );
        assert!(std::time::Instant::now() < deadline, "{}", app.message);
        std::thread::park_timeout(std::time::Duration::from_millis(5));
    }
    assert!(matches!(
        gate.request(Command::Undo).unwrap(),
        Event::MutationRefused { .. }
    ));
    assert_eq!(
        core.export_history().unwrap().document,
        cut.history().document
    );
    assert!(!app.is_dirty());
    if let Some(before) = source_usage {
        let after = app.owner.source().unwrap().store().usage().unwrap();
        assert_eq!(after.physical_bytes, before.physical_bytes);
        assert!(after.reserved_copy_bytes >= before.reserved_copy_bytes);
    }
    let actual = core.export_history().unwrap();
    let base = if floor {
        kinewright_core::HistoryTransition::floor(cut.history().clone(), 1)
            .unwrap()
            .after()
            .clone()
    } else {
        cut.history().clone()
    };
    let expected = kinewright_core::HistoryTransition::recovery_close(base.clone(), &base).unwrap();
    let observed = serde_json::json!({"query": matches!(core.request(Command::Query(Query::Snapshot)).unwrap(), Event::QueryResult(_)), "recovery": {
        "exact": actual == *expected.after(), "frozen_producers": refused == producer_count,
        "fresh_lease": !app.read_only(), "saved": !app.is_dirty(), "floor": actual.floor_offset == u64::from(floor),
    }});
    assert!(app.close());
    observed
}

#[cfg(target_os = "linux")]
#[test]
fn g05a_first_edit_undo_and_exact_switch_back() {
    observe_c11(false);
    observe_c11(true);
}

#[cfg(target_os = "linux")]
pub(crate) fn observe_c11(reply_drop: bool) -> serde_json::Value {
    observe_c11_fault(reply_drop.then_some(("switch", false)))
}

#[cfg(target_os = "linux")]
fn observe_c11_fault(fault: Option<(&'static str, bool)>) -> serde_json::Value {
    use kinewright_agent::writer_host::{HostCommand, SessionRoots};
    let temp = temp();
    let core = Core::spawn(doc()).unwrap();
    let mut owner = kinewright_project::editing_session::Ja1EditingSession::new(
        temp.root(),
        &core,
        StorageLimits::default(),
    )
    .unwrap();
    let path = temp.path("c11.kinewright");
    owner.first_save(&path, StorageLimits::default()).unwrap();
    edit(&core, false);
    owner.save_active().unwrap();
    edit(&core, true);
    core.request(Command::Undo).unwrap();
    owner.save_active().unwrap();
    let target = owner.list_recovery_points().unwrap().remove(0);
    let old_history = core.export_history().unwrap();
    let mut app = Ja1Attachment::new(owner).unwrap();
    app.launch_host_at(
        &host::executable(),
        "c11-app".into(),
        SessionRoots {
            read: vec![temp.root().into()],
            write: vec![temp.root().into()],
        },
    )
    .unwrap();
    let old_branch = app
        .host
        .as_ref()
        .unwrap()
        .client
        .request(HostCommand::CurrentAttachment)
        .unwrap()
        .switch_attachment
        .unwrap()
        .current
        .branch;
    let old_permit = app
        .host
        .as_ref()
        .unwrap()
        .manual
        .as_ref()
        .unwrap()
        .permit
        .clone();
    app.apply(Action::PreviewRecovery(target)).unwrap();
    let staged_actor = app
        .host
        .as_ref()
        .unwrap()
        .prepared_switch_probe()
        .expect("GUI resources must be staged before the selection publication");
    assert_eq!(
        staged_actor.attachment_state().unwrap(),
        AttachmentState::Frozen
    );
    let frozen = app.read_only()
        && matches!(
            old_permit.request(Command::Undo).unwrap(),
            Event::MutationRefused { .. }
        );
    assert!(frozen);
    let proxy =
        fault.map(|(command, before)| app.host.as_mut().unwrap().drop_http_reply(command, before));
    if let Some(proxy) = &proxy {
        assert!(
            app.apply(Action::OpenRecovery).is_err(),
            "the real App must see the transport error"
        );
        assert!(proxy.dropped.load(std::sync::atomic::Ordering::Acquire));
        assert!(!app.retention_lost, "reply loss is not confirmed host loss");
        assert!(app.read_only());
        app.apply(Action::Retry).unwrap();
    } else {
        app.apply(Action::OpenRecovery).unwrap();
    }
    assert!(!app.read_only());
    let selected = app.owner.core().export_history().unwrap();
    let selected_actor = app.owner.core().actor_identity();
    assert_eq!(selected_actor, staged_actor.actor_identity());
    let fresh = app
        .host
        .as_ref()
        .unwrap()
        .manual
        .as_ref()
        .unwrap()
        .permit
        .clone();
    fresh
        .request(Command::Do(Operation::SetTrackSyncLock {
            track: TrackId(1),
            locked: !selected.document.tracks[0].sync_lock,
        }))
        .unwrap();
    fresh.request(Command::Undo).unwrap();
    let first_undo = app.owner.core().export_history().unwrap().document == selected.document;
    assert!(first_undo);
    app.apply(Action::PreviewRecovery(RecoverySelection::WorkingBranch(
        old_branch.clone(),
    )))
    .unwrap();
    app.apply(Action::OpenRecovery).unwrap();
    let exact = app.owner.core().export_history().unwrap() == old_history;
    assert!(exact);
    assert_ne!(app.owner.core().actor_identity(), selected_actor);
    let stale = matches!(
        old_permit.request(Command::Redo).unwrap(),
        Event::MutationRefused { .. }
    );
    assert!(stale);
    let attachment = app
        .host
        .as_ref()
        .unwrap()
        .client
        .request(HostCommand::CurrentAttachment)
        .unwrap()
        .switch_attachment
        .unwrap();
    assert_eq!(attachment.current.branch, old_branch);
    let saved = !app.is_dirty();
    assert!(saved);
    let query = matches!(
        app.owner
            .core()
            .request(Command::Query(Query::Snapshot))
            .unwrap(),
        Event::QueryResult(_)
    );
    assert!(app.close());
    drop(app);
    let mut reopened = Ja1Attachment::open(&path).unwrap();
    reopened
        .launch_host_at(
            &host::executable(),
            "c11-app-reopened".into(),
            SessionRoots {
                read: vec![temp.root().into()],
                write: vec![temp.root().into()],
            },
        )
        .unwrap();
    assert!(!reopened.read_only());
    assert_eq!(
        reopened.view.as_ref().unwrap().core.actor_identity(),
        reopened.owner.core().actor_identity()
    );
    reopened
        .host
        .as_ref()
        .unwrap()
        .manual
        .as_ref()
        .unwrap()
        .permit
        .request(Command::Undo)
        .unwrap();
    assert!(
        reopened
            .view
            .as_ref()
            .unwrap()
            .events
            .try_iter()
            .any(|event| matches!(event, Event::DocumentChanged { .. })),
        "the actual restored view must receive the first edit's Core event"
    );
    assert!(reopened.close());
    serde_json::json!({"query": query, "switch": { "exact": exact, "frozen": frozen, "first_undo": first_undo, "stale": stale, "saved": saved, "generation": attachment.switch_generation == 2 }})
}

#[cfg(target_os = "linux")]
#[test]
fn s2h_confirm_reply_loss_retries_installed_b_without_reinstallation() {
    observe_c11_fault(Some(("confirm_switch", false)));
    observe_c11_fault(Some(("confirm_switch", true)));
}

#[cfg(target_os = "linux")]
#[test]
fn s2h_legacy_unsaved_activation_keeps_local_save_and_exact_reopen() {
    crate::app::ja_s2h_legacy_activation_witness();
}

#[cfg(target_os = "linux")]
#[test]
fn s2h_unlaunchable_host_restores_local_writer_and_both_saves() {
    use kinewright_agent::writer_host::SessionRoots;
    for corrupt in [false, true] {
        let temp = temp();
        let path = temp.path("a.kinewright");
        let core = Core::spawn(doc()).unwrap();
        let mut owner = kinewright_project::editing_session::Ja1EditingSession::new(
            temp.root(),
            &core,
            StorageLimits::default(),
        )
        .unwrap();
        owner.first_save(&path, StorageLimits::default()).unwrap();
        edit(&core, false);
        let expected = core.export_history().unwrap();
        let executable = temp.path("broken-host");
        if corrupt {
            use std::os::unix::fs::PermissionsExt;
            std::fs::write(&executable, b"invalid executable\0").unwrap();
            std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let mut app = Ja1Attachment::new(owner).unwrap();
        assert!(
            app.launch_host_at(
                &executable,
                "s2h-launch".into(),
                SessionRoots {
                    read: vec![temp.root().into()],
                    write: vec![temp.root().into()]
                }
            )
            .is_err()
        );
        assert!(
            !app.retention_lost,
            "pre-acquisition failure must restore local authority"
        );
        assert!(!app.read_only());
        assert_eq!(core.attachment_state(), Ok(AttachmentState::Live));
        app.apply(Action::Save).unwrap();
        let copy = temp.path("b.kinewright");
        app.apply(Action::SaveAs(copy.clone())).unwrap();
        assert!(app.close());
        drop(app);
        for destination in [&path, &copy] {
            let reopened =
                kinewright_project::editing_session::Ja1EditingSession::open(destination).unwrap();
            assert_eq!(reopened.core().export_history().unwrap(), expected);
        }
    }
}

#[cfg(target_os = "linux")]
#[test]
fn s2h_manual_and_save_reply_loss_reconcile_originals_and_kill_is_l2() {
    use kinewright_agent::writer_host::SessionRoots;
    for command in ["commit_history", "save"] {
        let temp = temp();
        let path = temp.path("a.kinewright");
        let core = Core::spawn(doc()).unwrap();
        let mut owner = kinewright_project::editing_session::Ja1EditingSession::new(
            temp.root(),
            &core,
            StorageLimits::default(),
        )
        .unwrap();
        owner.first_save(&path, StorageLimits::default()).unwrap();
        let mut app = Ja1Attachment::new(owner).unwrap();
        app.launch_host_at(
            &host::executable(),
            "s2h-reply".into(),
            SessionRoots {
                read: vec![temp.root().into()],
                write: vec![temp.root().into()],
            },
        )
        .unwrap();
        let permit = app
            .host
            .as_ref()
            .unwrap()
            .manual
            .as_ref()
            .unwrap()
            .permit
            .clone();
        assert!(matches!(
            permit
                .request(Command::Do(Operation::SetTrackSyncLock {
                    track: TrackId(1),
                    locked: false
                }))
                .unwrap(),
            Event::DocumentChanged { .. }
        ));
        let accepted = permit.pending().unwrap();
        let original =
            kinewright_project::manual_history::AcceptedManualCommand::decode(&accepted[0])
                .unwrap()
                .envelope;
        let expected = core.export_history().unwrap();
        let proxy = app.host.as_mut().unwrap().drop_http_reply(command, false);
        assert!(app.apply(Action::Save).is_err());
        assert!(proxy.dropped.load(std::sync::atomic::Ordering::Acquire));
        assert!(!app.retention_lost);
        assert!(app.read_only());
        let child = app.host.as_ref().unwrap().child_id();
        let recovery_proxy = app
            .host
            .as_mut()
            .unwrap()
            .drop_http_reply("publish_barrier", false);
        app.apply(Action::Retry).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while app.host.as_ref().unwrap().recovery_running() && std::time::Instant::now() < deadline
        {
            app.poll_host();
            std::thread::park_timeout(std::time::Duration::from_millis(5));
        }
        assert!(
            recovery_proxy
                .dropped
                .load(std::sync::atomic::Ordering::Acquire)
        );
        assert!(app.read_only());
        assert!(!app.retention_lost);
        assert_eq!(
            app.host.as_ref().unwrap().child_id(),
            child,
            "recovery reply loss must retain the same owned host for another Retry"
        );
        assert!(!app.host.as_mut().unwrap().confirmed_lost());
        app.apply(Action::Retry).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while app.read_only() && std::time::Instant::now() < deadline {
            app.poll_host();
            std::thread::park_timeout(std::time::Duration::from_millis(5));
        }
        assert!(!app.read_only(), "{}", app.message);
        assert!(!app.retention_lost);
        // Recovery-close may close the gesture; it cannot add or replay an edit.
        let after = core.export_history().unwrap();
        assert_eq!(after.document, expected.document);
        assert_eq!(after.batches, expected.batches);
        assert_eq!(after.cursor.undo, expected.cursor.undo);
        app.host.as_mut().unwrap().kill_owned_child();
        app.poll_host();
        assert!(
            app.retention_lost,
            "confirmed host death must still reach L2"
        );
        assert!(
            app.apply(Action::SaveAs(temp.path("forbidden.kinewright")))
                .is_err()
        );
        assert!(app.close());
        drop(app);
        let writer = LoadedBundle::read(&path).unwrap().into_writer().unwrap();
        assert!(matches!(
            writer.receipt_lookup(&original).unwrap(),
            kinewright_project::receipts::ReceiptLookup::Committed(_)
                | kinewright_project::receipts::ReceiptLookup::Retired(_)
        ));
    }
}

#[cfg(target_os = "linux")]
#[test]
fn s2h_direct_legacy_handoff_keeps_exact_history_until_local_studio_save() {
    use kinewright_agent::writer_host::SessionRoots;
    let temp = temp();
    let path = temp.path("legacy.kinewright");
    let document = doc();
    std::fs::write(
        &path,
        kinewright_project::serialize_project_document(&document).unwrap(),
    )
    .unwrap();
    let core = Core::spawn(document).unwrap();
    edit(&core, false);
    let expected = core.export_history().unwrap();
    let admission = BundleWriteAdmission::read(&path).unwrap();
    let owner = kinewright_project::editing_session::Ja1EditingSession::activate(
        &path, admission, &core, None,
    )
    .unwrap();
    let mut app = Ja1Attachment::new(owner).unwrap();
    assert!(
        app.launch_host_at(
            &host::executable(),
            "s2h-direct-legacy".into(),
            SessionRoots {
                read: vec![temp.root().into()],
                write: vec![temp.root().into()]
            }
        )
        .is_err()
    );
    assert!(!app.retention_lost);
    assert!(!app.read_only());
    assert_eq!(core.export_history().unwrap(), expected);
    app.apply(Action::Save).unwrap();
    assert!(app.close());
    drop(app);
    let reopened = kinewright_project::editing_session::Ja1EditingSession::open(&path).unwrap();
    assert_eq!(reopened.core().export_history().unwrap(), expected);
}
