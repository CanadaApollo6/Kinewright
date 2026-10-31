//! Studio controls. The GUI keeps its originating source and manual actors until explicit Close.
use eframe::egui;
use kinewright_core::{
    AttachmentState, Command, Core, Document, Event, Query, QueryResult, TimelineRevision,
};
use kinewright_project::{
    bundle::{BundleError, PublicationState},
    bundle_copy::CopyStatus,
    bundle_records::{BranchId, ObjectRef},
    bundle_recovery::RecoverySelection,
    bundle_rescue::{RescueConsent, RescueEligibility, RescueProbe, RescueSelection},
    bundle_switch::ProjectSelectionResult,
    editing_session::Ja1EditingSession,
    storage_budget::StorageLimits,
};
use std::fmt::Write as _;
#[path = "ja1_host.rs"]
mod host;
#[cfg(test)]
pub(crate) use host::SCHEDULER_SETTINGS;
#[path = "host_snapshot.rs"]
mod host_snapshot;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

const MANUAL_SAVE_REQUIRED_TEXT: &str =
    "Save once to keep editing here; until then this timeline is read-only.";

fn manual_status_message(message: &str, has_manual_gate: bool) -> String {
    let message = message
        .strip_suffix(MANUAL_SAVE_REQUIRED_TEXT)
        .map_or(message, str::trim_end);
    if has_manual_gate {
        message.into()
    } else {
        format!("{message} {MANUAL_SAVE_REQUIRED_TEXT}")
    }
}

pub(crate) struct PreparedView {
    pub core: Core,
    pub events: crossbeam_channel::Receiver<Event>,
    pub document: Arc<Document>,
    pub revision: TimelineRevision,
}
impl PreparedView {
    fn prepare(core: &Core) -> Result<Self, BundleError> {
        let events = core
            .subscribe()
            .map_err(|e| BundleError::Corrupt(e.to_string()))?;
        let Event::QueryResult(QueryResult::Snapshot { document, revision }) = core
            .request(Command::Query(Query::Snapshot))
            .map_err(|e| BundleError::Corrupt(e.to_string()))?
        else {
            return Err(BundleError::HistoryBarrierInvariant);
        };
        Ok(Self {
            core: core.clone(),
            events,
            document,
            revision,
        })
    }
}

pub(crate) enum Action {
    StudioVersions,
    StudioManualPlayback(Arc<Document>),
    StudioSelect(kinewright_project::studio_records::SnapshotKey),
    StudioEditCopy(kinewright_project::studio_records::SnapshotKey),
    ImportMedia(PathBuf),
    ChangeMediaInterpretation(Box<kinewright_core::Operation>),
    Save,
    ConfirmSuccessorSave,
    SaveAs(PathBuf),
    FirstSave(PathBuf),
    Retry,
    PreviewRecovery(RecoverySelection),
    OpenRecovery,
    Abandon,
    PreviewRescue,
    ConsentTrim,
    ConsentOmission,
    RescueTo(PathBuf),
    Cancel,
    PreviewRetirement(BranchId),
    RemoveBranch,
    PreviewForget,
    Forget,
    Reconcile(PathBuf),
    PreviewRelease(PathBuf),
    Release,
    SaveSession(BranchId),
    CloseSession(BranchId),
    OpenCapture(PathBuf),
}
impl Action {
    /// Whether this action keeps the project's identity and session, so it may
    /// run while a spoken note is recording. Anything else can move the
    /// project (Save As, recovery, rescue, a retried switch, a Save that
    /// completes a confirmed successor Save As) or end a session.
    pub(crate) fn keeps_identity(&self) -> bool {
        matches!(
            self,
            Self::StudioVersions
                | Self::StudioManualPlayback(_)
                | Self::StudioSelect(_)
                | Self::ImportMedia(_)
                | Self::ChangeMediaInterpretation(_)
                | Self::PreviewRecovery(_)
                | Self::PreviewRescue
                | Self::PreviewRetirement(_)
                | Self::PreviewForget
                | Self::PreviewRelease(_)
                | Self::ConsentTrim
                | Self::ConsentOmission
                | Self::Cancel
        )
    }
}
enum Choice {
    SuccessorSave(Option<PathBuf>),
    Recovery,
    Rescue {
        selection: Box<RescueSelection>,
        consent: RescueConsent,
        trim: bool,
        omission: bool,
    },
    Remove {
        branch: BranchId,
        root: String,
    },
    Forget {
        point: ObjectRef,
        root: String,
    },
    Release(PathBuf),
}

/// Every Studio producer uses this capability; raw/stale Core clones are refused
/// inside the actor. No UI producer waits for durable IO.
pub(crate) struct ManualProducerGate {
    actor: kinewright_core::ActorIdentity,
    permit: kinewright_core::ManualPermit,
}
impl ManualProducerGate {
    pub fn send(&self, command: Command) -> Result<(), kinewright_core::CoreDisconnected> {
        self.permit.send(command)
    }
}

#[allow(
    clippy::struct_excessive_bools,
    reason = "independent attachment and UI lifecycle flags"
)]
pub(crate) struct Ja1Attachment {
    pub(crate) imported_media: Option<kinewright_core::AssetId>,
    pub owner: Ja1EditingSession,
    pub message: String,
    pub(crate) studio_ready: Option<Arc<kinewright_project::studio_store::FrozenContext>>,
    pub(crate) studio_versions_ready: Option<Vec<kinewright_project::studio_records::SnapshotKey>>,
    pub(crate) studio_edit_pending: bool,
    pub(crate) studio_error_ready: Option<String>,
    /// The scope a failed Current-edit acquisition tried, and whether the
    /// failure is temporary (S3 C11 fu1).
    pub(crate) studio_manual_failed: Option<(
        Option<(u64, kinewright_project::bundle_records::ObjectRef)>,
        bool,
    )>,
    pub(crate) studio_manual_ready: Option<(
        (u64, kinewright_project::bundle_records::ObjectRef),
        Arc<kinewright_media::owned_io::PlaybackInputs>,
    )>,
    choice: Option<Choice>,
    offer: Option<RescueProbe>,
    pub view: Option<PreparedView>,
    points: Vec<RecoverySelection>,
    branches: Vec<(BranchId, Vec<String>)>,
    candidates: Vec<BranchId>,
    destinations: Vec<(PathBuf, PublicationState)>,
    frozen: bool,
    pub panel_open: bool,
    retention_lost: bool,
    originating_context: Arc<std::sync::atomic::AtomicBool>,
    snapshot_readers: host_snapshot::SnapshotReaders,
    host: Option<host::HostAttachment>,
    host_history_digest: std::cell::RefCell<
        Option<(
            kinewright_core::ActorIdentity,
            kinewright_core::HistoryChangeId,
            String,
        )>,
    >,
    known_agent_effects: std::collections::BTreeSet<PathBuf>,
    save_disclosure: String,
    #[cfg(test)]
    last_selection: Option<ProjectSelectionResult>,
    #[cfg(test)]
    last_save: Option<kinewright_project::bundle::BundleSaveReport>,
    #[cfg(test)]
    last_rescue: Option<kinewright_project::bundle_rescue::RescueOutcome>,
    #[cfg(test)]
    last_retirement: Option<kinewright_project::bundle::BundleSaveReport>,
}
impl Ja1Attachment {
    pub fn open(path: &Path) -> Result<Self, BundleError> {
        Self::new(Ja1EditingSession::open(path)?)
    }
    pub fn new(owner: Ja1EditingSession) -> Result<Self, BundleError> {
        let view = Some(PreparedView::prepare(owner.core())?);
        let mut this = Self {
            imported_media: None,
            studio_ready: None,
            studio_versions_ready: None,
            studio_edit_pending: false,
            studio_error_ready: None,
            studio_manual_failed: None,
            studio_manual_ready: None,
            owner,
            message: "Studio editing. Saved means a verified durable save.".into(),
            choice: None,
            offer: None,
            view,
            points: vec![],
            branches: vec![],
            candidates: vec![],
            destinations: vec![],
            frozen: false,
            panel_open: true,
            retention_lost: false,
            originating_context: Arc::new(std::sync::atomic::AtomicBool::new(true)),
            snapshot_readers: host_snapshot::SnapshotReaders::new()?,
            host: None,
            host_history_digest: std::cell::RefCell::new(None),
            known_agent_effects: std::collections::BTreeSet::new(),
            save_disclosure: String::new(),
            #[cfg(test)]
            last_selection: None,
            #[cfg(test)]
            last_save: None,
            #[cfg(test)]
            last_rescue: None,
            #[cfg(test)]
            last_retirement: None,
        };
        this.refresh();
        Ok(this)
    }
    fn refresh(&mut self) {
        if self.host.is_some() || self.retention_lost {
            return;
        }
        self.frozen = self.owner.core().attachment_state() != Ok(AttachmentState::Live);
        self.points = self.owner.list_recovery_points().unwrap_or_default();
        self.branches = self
            .owner
            .list_working_branches()
            .unwrap_or_default()
            .into_iter()
            .map(|(id, _, blockers)| (id, blockers))
            .collect();
        self.candidates = self.owner.retirement_candidates().unwrap_or_default();
        self.destinations = self.owner.retained_destinations().unwrap_or_default();
    }
    pub fn is_dirty(&self) -> bool {
        if self.retention_lost {
            return true;
        }
        if let Some(host) = &self.host {
            let Some(saved) = &host.status.saved_digest else {
                return true;
            };
            let core = self.owner.core();
            let Ok(change) = core.history_change_id() else {
                return true;
            };
            if let Some((actor, cached, digest)) = &*self.host_history_digest.borrow()
                && *actor == core.actor_identity()
                && *cached == change
            {
                return digest != saved;
            }
            let Ok((identified, history)) = core.export_history_identified() else {
                return true;
            };
            let Ok(bytes) = serde_json::to_vec(&history) else {
                return true;
            };
            let digest = kinewright_media::sha256_bytes(&bytes);
            let dirty = &digest != saved;
            *self.host_history_digest.borrow_mut() =
                Some((core.actor_identity(), identified, digest));
            return dirty;
        }
        self.owner.dirty().unwrap_or(true)
    }

    pub fn project_path(&self) -> Option<PathBuf> {
        if let Some(host) = &self.host {
            return host.status.association.clone();
        }
        self.owner
            .association()
            .map(|association| association.path().to_path_buf())
    }

    pub fn named_io_root(&self) -> Option<PathBuf> {
        if self.host.is_some() {
            return None;
        }
        self.owner.store().ok()?.resolved_root().ok()
    }

    pub fn destination_change(&self) -> Result<(), BundleError> {
        self.require_retention()?;
        if let Some(host) = &self.host {
            if let Some(disclosure) = &host.status.destination_change {
                return Err(BundleError::Identity(disclosure.clone()));
            }
            return Ok(());
        }
        self.owner.check_destination_change()
    }
    pub fn close_text(&self) -> String {
        if self.retention_lost {
            return format!(
                "{RETENTION_LOST_TEXT} Closing can discard the only accessible copy of your unsaved work."
            );
        }
        if let Some(host) = &self.host {
            let mut text = host.status.close_disclosure.clone();
            if host.pending_count() > 0 {
                text.push_str(" Manual edits are still waiting for a durable acknowledgement. Closing can discard this pending work; it is not Saved.");
            }
            return text;
        }
        let mut text = self.owner.close_disclosure().unwrap_or_else(|e| format!("Could not verify this project's state: {e}. Closing can discard work still held only in this session."));
        if !self.destinations.is_empty() {
            text.push_str(" Closing ends retention of the original session, including unknown destinations; it does not prove that they never saved.");
        }
        text
    }
    pub fn read_only(&self) -> bool {
        self.retention_lost
            || self
                .host
                .as_ref()
                .is_some_and(host::HostAttachment::read_only)
            || self.frozen
            || self.choice.is_some()
    }
    #[allow(
        clippy::too_many_lines,
        reason = "one attachment action dispatch and result boundary"
    )]
    pub fn apply(&mut self, action: Action) -> Result<(), BundleError> {
        self.require_retention()?;
        match &action {
            Action::StudioManualPlayback(document) => {
                // A failure is reported once, with the scope it tried: an
                // unreported one would be retried every frame (S3 C10). While a
                // host holds the bundle, the inputs come from a host lease on its
                // current edit (S3 C11).
                let scope = self.studio_playback_scope();
                let attempted = scope.as_ref().ok().cloned();
                let inputs = scope.and_then(|scope| {
                    let inputs = if self.host.is_some() {
                        self.studio_current_reader(&scope)?
                            .playback_inputs(Arc::clone(document))?
                    } else {
                        self.owner
                            .store()?
                            .manual_playback_inputs(Arc::clone(document), &scope.1)?
                    };
                    Ok((scope, inputs))
                });
                match inputs {
                    Ok((scope, inputs)) => {
                        self.studio_manual_ready = Some((scope, inputs));
                        self.panel_open = false;
                        return Ok(());
                    }
                    Err(error) => {
                        self.message = format!("Playback input unavailable: {error}");
                        self.studio_error_ready = Some(self.message.clone());
                        let temporary = matches!(
                            error,
                            BundleError::HistoryChanged
                                | BundleError::Unsettled
                                | BundleError::SaveInProgress
                        );
                        self.studio_manual_failed = Some((attempted, temporary));
                        return Err(error);
                    }
                }
            }
            Action::StudioVersions => {
                self.studio_versions_ready = Some(self.studio_versions()?);
                return Ok(());
            }
            Action::StudioSelect(key) => match self.studio_context(key) {
                Ok(context) => {
                    self.studio_ready = Some(Arc::new(context));
                    self.panel_open = false;
                    return Ok(());
                }
                Err(error) => {
                    self.message = format!("Version damaged or unavailable: {error}");
                    self.studio_error_ready = Some(self.message.clone());
                    return Err(error);
                }
            },
            Action::StudioEditCopy(key) => {
                self.studio_edit_pending = false;
                let before = self.owner.core().actor_identity();
                let prior_view = self.view.take();
                let result = self.studio_edit_copy(key);
                self.studio_edit_pending = result.is_ok()
                    && self.view.as_ref().is_some_and(|view| {
                        view.core.actor_identity() != before
                            && serde_json::to_vec(&*view.document).is_ok_and(|bytes| {
                                kinewright_media::sha256_bytes(&bytes) == key.document_hash
                            })
                    });
                if self.view.is_none() && self.owner.core().actor_identity() == before {
                    self.view = prior_view;
                }
                if let Err(error) = &result {
                    self.message = format!(
                        "The copy was not opened. Your selected version remains unchanged. {error}"
                    );
                    self.studio_error_ready = Some(self.message.clone());
                }
                return result;
            }
            _ => {}
        }
        if let Action::ImportMedia(path) = &action
            && self.host.is_some()
            && self.owner.association().is_none()
        {
            let result = self
                .host
                .as_mut()
                .expect("attached host")
                .import_unsaved_media(&mut self.owner, path);
            return match result {
                Ok((id, reply)) => {
                    self.imported_media = Some(id);
                    self.view = Some(PreparedView::prepare(self.owner.core())?);
                    self.accept_host_reply(reply)
                }
                Err(error) => {
                    if self
                        .host
                        .as_ref()
                        .is_some_and(host::HostAttachment::unsaved_import_pending)
                    {
                        self.host_request_failed(&error);
                    } else {
                        self.message = error.to_string();
                    }
                    Err(error)
                }
            };
        }
        if self.host.is_some() {
            return self.apply_host(action);
        }
        let result = self.apply_inner(action);
        if let Err(error) = &result {
            self.message = match error {
                BundleError::RescueMissingOwnedBytes(missing) => format!(
                    "Rescue copy was not saved. Required project data is unavailable from A: {:?}. {} {} items omitted. Your edits remain in this session. Reconnect A or restore these exact objects, then retry. A's save outcome is still unknown. Source media availability is separate.",
                    missing.entries,
                    if missing.dependencies_complete {
                        ""
                    } else {
                        "The dependency list is incomplete."
                    },
                    missing.omitted
                ),
                BundleError::Capacity(_) => format!(
                    "There is not enough room to reopen the proposed project. Your current edit is {}. Remove an eligible working branch, then retry this same selection. {error}",
                    if self.owner.dirty()? {
                        "unchanged"
                    } else {
                        "saved in its working branch"
                    }
                ),
                _ => error.to_string(),
            };
            if self
                .owner
                .core()
                .attachment_state()
                .is_ok_and(|s| s == AttachmentState::Frozen)
            {
                self.message.push_str(" Rescue copy was not saved. Editing remains read-only while A's save or selection is unresolved.");
            }
        }
        self.refresh();
        result
    }
    fn require_retention(&self) -> Result<(), BundleError> {
        if self.retention_lost {
            return Err(BundleError::Identity(RETENTION_LOST_TEXT.into()));
        }
        Ok(())
    }

    pub fn operation_panicked(&mut self) {
        // The catch surrounds the borrowed operation: unwind cannot drop this owner.
        let _ = self.owner.host_lost();
        self.retention_lost = true;
        self.frozen = true;
        self.choice = None;
        self.message = RETENTION_LOST_TEXT.into();
        self.panel_open = true;
    }

    pub fn launch_host(
        &mut self,
        project: String,
        roots: kinewright_agent::writer_host::SessionRoots,
    ) -> Result<(), BundleError> {
        self.launch_host_at(&host::executable(), project, roots)
    }

    fn launch_host_at(
        &mut self,
        executable: &Path,
        project: String,
        roots: kinewright_agent::writer_host::SessionRoots,
    ) -> Result<(), BundleError> {
        self.require_retention()?;
        if self.host.is_some() {
            return Ok(());
        }
        let result = host::HostAttachment::launch(
            &mut self.owner,
            executable,
            project,
            roots,
            Arc::downgrade(&self.originating_context),
        );
        match result {
            Ok(mut host) => {
                if !host.unknown_operation {
                    let cut = self.owner.freeze_host_cut()?;
                    self.owner.complete_host_cut(&cut, None, None)?;
                    host.cut = None;
                }
                self.message.clone_from(&host.status.message);
                self.host = Some(host);
                self.start_host_manual()
            }
            Err(error) => {
                if self.owner.host_retention_lost() {
                    self.operation_panicked();
                } else {
                    self.frozen = self.owner.core().attachment_state() != Ok(AttachmentState::Live);
                    self.message =
                        "Host attachment did not start. Local editing and Save remain available. "
                            .into();
                }
                self.message.push_str(&error.to_string());
                Err(error)
            }
        }
    }

    fn start_host_manual(&mut self) -> Result<(), BundleError> {
        let host = self.host.as_mut().expect("attached host");
        if host.unknown_operation {
            self.frozen = true;
            self.panel_open = true;
            return Ok(());
        }
        let outcome = host.start_manual(&mut self.owner);
        let view = match outcome {
            Ok(view) => view,
            Err(error) => {
                if self
                    .host
                    .as_ref()
                    .is_some_and(|host| host.unknown_operation || host.pending_foreground())
                {
                    self.host_request_failed(&error);
                } else {
                    self.frozen = self.owner.core().attachment_state() != Ok(AttachmentState::Live);
                    self.message = error.to_string();
                }
                return Err(error);
            }
        };
        let host = self.host.as_mut().expect("attached host");
        if let Some(view) = view {
            self.view = Some(view);
            host.complete_switch_handoff();
        }
        self.frozen = self.owner.core().attachment_state() != Ok(AttachmentState::Live);
        self.message = manual_status_message(&self.message, host.manual.is_some());
        Ok(())
    }

    pub fn manual_save_required(&self) -> bool {
        !self.retention_lost
            && self
                .host
                .as_ref()
                .is_some_and(host::HostAttachment::manual_save_required)
    }

    pub fn manual_refusal_reason(&self) -> &str {
        if self.retention_lost {
            RETENTION_LOST_TEXT
        } else if self.manual_save_required() {
            MANUAL_SAVE_REQUIRED_TEXT
        } else {
            "Editing is read-only while the save or recovery choice is resolved."
        }
    }

    pub fn send_manual(&self, command: Command) -> Result<(), kinewright_core::CoreDisconnected> {
        if let Some(gate) = self.host.as_ref().and_then(|host| host.manual.as_ref()) {
            gate.send(command)
        } else {
            self.owner.core().send(command)
        }
    }
    #[cfg(test)]
    pub(crate) fn manual_pending_for_test(
        &self,
    ) -> Result<Vec<kinewright_core::AcceptedHistory>, String> {
        self.host
            .as_ref()
            .and_then(|h| h.manual.as_ref())
            .ok_or("manual permit missing")?
            .permit
            .pending()
            .map_err(|e| e.to_string())
    }
    /// The saved checkpoint Current-edit mappings are made on: the host's,
    /// from its status, or this session's own (S3 C11 fu1).
    pub(crate) fn studio_map_checkpoint(
        &self,
    ) -> Option<kinewright_project::bundle_records::ObjectRef> {
        match &self.host {
            Some(host) => host.status.manual_checkpoint.clone(),
            None => self.owner.manual_checkpoint_ref().ok(),
        }
    }
    /// The owner epoch and inputs Current-edit playback reads. While a host
    /// holds the bundle, only its status names them; a change renews the lease.
    pub(crate) fn studio_playback_scope(
        &self,
    ) -> Result<(u64, kinewright_project::bundle_records::ObjectRef), BundleError> {
        match &self.host {
            Some(host) => host
                .status
                .playback_scope
                .clone()
                .ok_or(BundleError::Unsettled),
            None => Ok((
                self.owner.store()?.owner_epoch(),
                self.owner.current_inputs_ref().clone(),
            )),
        }
    }
    pub(crate) fn studio_current_reader(
        &self,
        scope: &(u64, kinewright_project::bundle_records::ObjectRef),
    ) -> Result<kinewright_project::store::snapshot_reader::CurrentEditReader, BundleError> {
        let host = self.host.as_ref().ok_or(BundleError::HistoryChanged)?;
        let path = self.project_path().ok_or(BundleError::HistoryChanged)?;
        self.snapshot_readers.open_current(
            &host.client,
            &path,
            Arc::downgrade(&self.originating_context),
            scope,
        )
    }
    pub(crate) fn studio_versions(
        &self,
    ) -> Result<Vec<kinewright_project::studio_records::SnapshotKey>, BundleError> {
        if let Some(host) = &self.host {
            let reply = host
                .client
                .request(kinewright_agent::writer_host::HostCommand::ListVersions)
                .map_err(|e| BundleError::Corrupt(e.to_string()))?;
            if let Some(error) = reply.error {
                return Err(BundleError::Corrupt(error.message));
            }
            return Ok(reply.versions);
        }
        self.owner.studio_versions()
    }
    pub(crate) fn studio_project_id(
        &self,
    ) -> Result<kinewright_project::bundle_records::ProjectId, BundleError> {
        // One source: while a host holds the bundle, only its authenticated
        // status names the project, refreshed by every reply. First Save, Save
        // As and rescue change it there; a launch-time copy would file B's
        // notes under A (S3 C7 fix 1).
        match &self.host {
            Some(host) => host.status.project_id.clone().ok_or(BundleError::Unsettled),
            None => self.owner.project_id(),
        }
    }
    pub(crate) fn studio_context(
        &self,
        key: &kinewright_project::studio_records::SnapshotKey,
    ) -> Result<kinewright_project::studio_store::FrozenContext, BundleError> {
        if self.host.is_some() {
            return self.studio_snapshot_reader(key)?.frozen_context();
        }
        self.owner.frozen_context(key)
    }
    pub(crate) fn studio_snapshot_reader(
        &self,
        key: &kinewright_project::studio_records::SnapshotKey,
    ) -> Result<kinewright_project::store::snapshot_reader::HostSnapshotReader, BundleError> {
        let host = self.host.as_ref().ok_or(BundleError::HistoryChanged)?;
        let path = self.project_path().ok_or(BundleError::HistoryChanged)?;
        self.snapshot_readers.open(
            &host.client,
            &path,
            Arc::downgrade(&self.originating_context),
            key,
        )
    }
    /// The occurrence lineage of one published version, read-only.
    pub(crate) fn studio_lineage(
        &self,
        key: &kinewright_project::studio_records::SnapshotKey,
    ) -> Result<kinewright_project::bundle_records::LineageRecord, BundleError> {
        if self.host.is_some() {
            let reader = self.studio_snapshot_reader(key)?;
            let lineage = reader.descriptor().lineage_ref.clone();
            Ok((*reader
                .read_record::<kinewright_project::bundle_records::LineageRecord>(&lineage)?)
            .clone())
        } else {
            let snapshot = self.owner.read_snapshot(key)?;
            Ok((*self
                .owner
                .store()?
                .read_record::<kinewright_project::bundle_records::LineageRecord>(
                    &snapshot.descriptor.lineage_ref,
                )?)
            .clone())
        }
    }
    pub(crate) fn studio_edit_copy(
        &mut self,
        key: &kinewright_project::studio_records::SnapshotKey,
    ) -> Result<(), BundleError> {
        let context = self.studio_context(key)?;
        let descriptor = &context.snapshot.descriptor;
        let checkpoint = if self.host.is_some() {
            self.studio_snapshot_reader(key)?
                .read_record::<kinewright_project::bundle_records::BranchCheckpoint>(
                    &descriptor.checkpoint_ref,
                )?
        } else {
            self.owner
                .store()?
                .read_record::<kinewright_project::bundle_records::BranchCheckpoint>(
                    &descriptor.checkpoint_ref,
                )?
        };
        self.apply(Action::PreviewRecovery(RecoverySelection::Checkpoint {
            root: descriptor.checkpoint_ref.clone(),
            branch: checkpoint.branch_id.clone(),
        }))?;
        self.apply(Action::OpenRecovery)
    }

    pub fn host_client(&self) -> Option<kinewright_agent::writer_host::HostClient> {
        if self.retention_lost {
            return None;
        }
        self.host.as_ref().map(|host| host.client.clone())
    }

    pub fn host_attached(&self) -> bool {
        self.host.is_some()
    }

    pub fn host_managed(&self) -> bool {
        self.host.is_some() || self.retention_lost
    }

    fn apply_host(&mut self, action: Action) -> Result<(), BundleError> {
        if matches!(action, Action::Retry)
            && self
                .host
                .as_ref()
                .is_some_and(host::HostAttachment::needs_manual_recovery)
        {
            if let Err(error) = self
                .host
                .as_mut()
                .expect("attached host")
                .begin_manual_recovery(&mut self.owner)
            {
                self.host_request_failed(&error);
                return Err(error);
            }
            self.frozen = true;
            self.message = "Editing is read-only while accepted history is recovered.".into();
            return Ok(());
        }
        let result = self
            .host
            .as_mut()
            .expect("attached host")
            .request(action, &mut self.owner);
        let known_refusal = result.as_ref().is_ok_and(|reply| reply.error.is_some());
        let result = result.and_then(|reply| {
            self.accept_host_reply(reply)?;
            self.start_host_manual()
        });
        match result {
            Ok(()) => Ok(()),
            Err(error) => {
                if !known_refusal {
                    self.host_request_failed(&error);
                }
                Err(error)
            }
        }
    }

    fn host_request_failed(&mut self, error: &BundleError) {
        let host = self.host.as_mut().expect("attached host");
        if host.confirmed_lost() || self.owner.host_retention_lost() {
            self.operation_panicked();
            return;
        }
        // A transport error does not prove loss of the retained owner. Preserve
        // the original manual queue and switch envelope for receipt reconciliation.
        let Ok(cut) = self.owner.recapture_host_recovery_cut() else {
            self.operation_panicked();
            return;
        };
        host.cut = Some(cut);
        host.unknown_operation = true;
        host.status.frozen = true;
        self.frozen = true;
        self.panel_open = true;
        self.message = format!(
            "The host reply was not confirmed. Your edits remain here, read-only. Retry to reconcile the original request. {error}"
        );
    }

    #[allow(
        clippy::too_many_lines,
        reason = "The one reply transition keeps actor, publisher and source-retention decisions together"
    )]
    fn accept_host_reply(
        &mut self,
        mut reply: kinewright_agent::writer_host::HostReply,
    ) -> Result<(), BundleError> {
        if !reply.status.retention_proven
            || reply
                .error
                .as_ref()
                .is_some_and(|error| error.code == "retention_lost")
        {
            self.operation_panicked();
            return Err(BundleError::Identity(RETENTION_LOST_TEXT.into()));
        }
        let host = self.host.as_mut().expect("attached host");
        if reply.unsaved_inputs_refreshed && reply.error.is_none() {
            self.imported_media = host.accept_unsaved_inputs()?;
        }
        if let Some(asset) = reply.owned_media.take() {
            let media_edit = reply.owned_media_edit.take();
            let is_import = media_edit.is_none();
            if reply.error.is_some() || host.unknown_operation {
                return Err(BundleError::HistoryChanged);
            }
            let cut = host.cut.as_ref().ok_or(BundleError::HistoryChanged)?;
            let lease = reply
                .manual_lease
                .take()
                .ok_or(BundleError::HistoryChanged)?;
            let inputs = reply
                .manual_inputs
                .take()
                .ok_or(BundleError::HistoryChanged)?;
            let digest = lease.history_sha256.clone();
            let endpoint = self.owner.manual_recovery_endpoint(cut, vec![])?;
            let permit = endpoint.finish(lease, inputs, Arc::clone(&self.originating_context))?;
            self.owner.finish_host_manual_recovery(&digest)?;
            host.manual = Some(ManualProducerGate {
                actor: self.owner.core().actor_identity(),
                permit,
            });
            host.cut = None;
            host.accept_status(reply.status);
            host.manual
                .as_ref()
                .ok_or(BundleError::HistoryChanged)?
                .send(Command::Do(media_edit.unwrap_or_else(|| {
                    kinewright_core::Operation::AddAsset {
                        asset: asset.clone(),
                    }
                })))
                .map_err(|_| BundleError::HistoryChanged)?;
            if is_import {
                self.imported_media = Some(asset.id);
            }
            self.frozen = false;
            self.message = if is_import {
                format!("Imported {}. Saving is pending.", asset.name)
            } else {
                "Colour interpretation changed. Saving is pending.".into()
            };
            return Ok(());
        }
        if let Some(committed) = &reply.switch_attachment {
            if reply.error.is_some()
                && host.cut.as_ref().is_some_and(|cut| {
                    if committed.history == *cut.history() {
                        return false;
                    }
                    let discarded = committed
                        .history
                        .floor_offset
                        .saturating_sub(cut.history().floor_offset);
                    usize::try_from(discarded)
                        .ok()
                        .filter(|n| *n > 0)
                        .and_then(|n| {
                            kinewright_core::HistoryTransition::floor(cut.history().clone(), n).ok()
                        })
                        .is_none_or(|floor| floor.after() != &committed.history)
                })
            {
                self.frozen = true;
                return Err(BundleError::HistoryChanged);
            }
            let (confirmed, view) = host.restore_switch(&mut self.owner, committed)?;
            self.owner.set_host_association(
                confirmed
                    .status
                    .association
                    .as_deref()
                    .ok_or(BundleError::HistoryChanged)?,
                &committed.current.history.sha256,
            )?;
            self.view = Some(view);
            self.message.clone_from(&confirmed.status.message);
            self.frozen = confirmed.status.frozen;
            host.complete_switch_handoff();
            host.accept_status(confirmed.status);
            if let Some(error) = reply.error {
                return Err(BundleError::Identity(error.to_string()));
            }
            return Ok(());
        }
        host.accept_manual_ack(&reply, &self.owner)?;
        self.message = reply.error.as_ref().map_or_else(
            || reply.status.message.clone(),
            |error| error.message.clone(),
        );
        if let Some(saved) = &reply.saved
            && !saved.floor_disclosure.is_empty()
        {
            self.message.push(' ');
            self.message.push_str(&saved.floor_disclosure);
        }
        let manual_failed = reply.error.is_some() && host.pending_count() > 0;
        if manual_failed && host.cut.is_none() {
            host.cut = Some(self.owner.freeze_host_cut()?);
        }
        // A bootstrap cut stays frozen through Status/Retry, including errors.
        // Only a successful foreground publication can establish its manual base.
        let can_resume = !host.unknown_operation
            && !manual_failed
            && !reply.status.frozen
            && reply.status.choice.is_none()
            && (host.manual.is_some() || (reply.error.is_none() && reply.saved.is_some()));
        if reply.activation.is_some() && (!can_resume || host.cut.is_none()) {
            self.operation_panicked();
            return Err(BundleError::Identity(
                "Host activation arrived outside its frozen foreground cut.".into(),
            ));
        }
        if can_resume && let Some(cut) = &host.cut {
            let digest = reply
                .saved
                .as_ref()
                .map(|report| report.history_sha256.as_str());
            if reply
                .history
                .as_ref()
                .is_some_and(|history| history != cut.history())
                && digest.is_none()
            {
                self.operation_panicked();
                return Err(BundleError::Identity("The host did not return the publisher's exact history identity. Saving and rescue remain paused.".into()));
            }
            if let Some(activation) = &reply.activation {
                let Some(digest) = digest.filter(|digest| *digest == activation.history_sha256)
                else {
                    self.operation_panicked();
                    return Err(BundleError::Identity(
                        "Host activation differs from its publisher's report.".into(),
                    ));
                };
                let Some(history) = reply.history else {
                    self.operation_panicked();
                    return Err(BundleError::HistoryBarrierInvariant);
                };
                let kind = match &activation.kind {
                    kinewright_agent::writer_host::HostActivationKind::Rescue => {
                        kinewright_project::editing_session::HostActivationKind::Rescue
                    }
                    kinewright_agent::writer_host::HostActivationKind::Recovery => {
                        kinewright_project::editing_session::HostActivationKind::Recovery
                    }
                };
                let mut prepared = None;
                let result = self
                    .owner
                    .activate_host_cut(cut, history, digest, kind, |core| {
                        prepared = Some(PreparedView::prepare(core)?);
                        Ok(())
                    });
                if let Err(error) = result {
                    self.operation_panicked();
                    return Err(error);
                }
                self.view = prepared;
            } else {
                if let Err(error) = self.owner.complete_host_cut(cut, reply.history, digest) {
                    self.operation_panicked();
                    return Err(error);
                }
                self.view = Some(PreparedView::prepare(self.owner.core())?);
            }
            host.cut = None;
            if let Some(saved) = &reply.saved {
                self.owner.set_host_association(
                    reply.status.association.as_deref().unwrap_or(&saved.path),
                    &saved.history_sha256,
                )?;
            }
        }
        self.frozen = reply.status.frozen
            || self.owner.core().attachment_state() != Ok(AttachmentState::Live);
        host.accept_status(reply.status);
        if host.unknown_operation {
            self.frozen = true;
            self.message = "The host reply was not confirmed. Your edits remain here, read-only. Retry to reconcile the original request.".into();
        } else {
            self.message = manual_status_message(&self.message, !self.manual_save_required());
        }
        if let Some(error) = reply.error {
            return Err(BundleError::Identity(error.to_string()));
        }
        Ok(())
    }

    pub fn poll_host(&mut self) -> bool {
        if self.retention_lost || self.host.is_none() {
            return false;
        }
        let result = self
            .host
            .as_mut()
            .expect("attached host")
            .poll(&mut self.owner);
        match result {
            Ok(Some(reply)) => {
                let _ = self
                    .accept_host_reply(reply)
                    .and_then(|()| self.start_host_manual());
                true
            }
            Ok(None) => false,
            Err(error) => {
                self.host_request_failed(&error);
                if self.retention_lost {
                    return true;
                }
                let recovery = self
                    .host
                    .as_mut()
                    .expect("attached host")
                    .begin_manual_recovery(&mut self.owner);
                self.frozen = true;
                self.message = "Editing is read-only while the writer host and accepted history are recovered.".into();
                if let Err(cause) = recovery {
                    self.message = format!("Recovery remains paused: {error}; {cause}");
                }
                true
            }
        }
    }

    pub fn close(&mut self) -> bool {
        if let Some(host) = &mut self.host {
            if host.recovery_running() {
                let _ = host.poll(&mut self.owner);
                if host.recovery_running() {
                    self.message = "Recovery is stopping. The originating source remains retained until its worker retires.".into();
                    return false;
                }
            }
            if !host.close() {
                self.message = "The host is checking continuation readiness. This attachment and its source stay here until detach is confirmed.".into();
                return false;
            }
        }
        self.snapshot_readers.revoke_all();
        self.originating_context
            .store(false, std::sync::atomic::Ordering::Release);
        self.retention_lost = true;
        self.frozen = true;
        let _ = self.owner.host_lost();
        match self.owner.close_originating_source() {
            Ok(true) => true,
            Ok(false) => {
                self.message = "This editing context is closed to new work. Its source readers are still retiring; the same retained data and charges stay here. Close again after they finish.".into();
                false
            }
            Err(error) => {
                self.message = format!(
                    "Source cleanup is still pending: {error}. This editing context keeps its retained owner and charges. Retry Close to finish cleanup."
                );
                false
            }
        }
    }
    pub(crate) fn continuation_pending(&self) -> bool {
        self.host
            .as_ref()
            .is_some_and(host::HostAttachment::continuation_pending)
    }
    #[allow(
        dead_code,
        reason = "C6 wires these controls into Studio; the hook uses the existing completed-work poll"
    )]
    pub(crate) fn request_orchestration(
        &mut self,
        command: kinewright_agent::writer_host::HostCommand,
    ) -> Result<(), BundleError> {
        self.host
            .as_mut()
            .ok_or(BundleError::HistoryChanged)?
            .request_hook(command)
    }
    #[allow(dead_code, reason = "C6 consumes run, issuer and attention replies")]
    pub(crate) fn take_orchestration_reply(
        &mut self,
    ) -> Option<kinewright_agent::writer_host::HostReply> {
        self.host.as_mut()?.take_hook_reply()
    }
    #[allow(
        dead_code,
        reason = "C6 owns the continuation UI and retains the issuer before the project view is dropped"
    )]
    pub(crate) fn continuation_issuer(&self) -> Option<kinewright_agent::writer_host::HostIssuer> {
        self.host.as_ref()?.continuation_issuer()
    }

    #[allow(clippy::too_many_lines)]
    fn apply_inner(&mut self, action: Action) -> Result<(), BundleError> {
        match action {
            Action::StudioVersions
            | Action::StudioManualPlayback(_)
            | Action::StudioSelect(_)
            | Action::StudioEditCopy(_) => {
                return Err(BundleError::HistoryChanged);
            }
            Action::ChangeMediaInterpretation(operation) => {
                let kinewright_core::Operation::SetAssetColorDescription { asset: id, .. } =
                    operation.as_ref()
                else {
                    return Err(BundleError::Unsupported(
                        "media interpretation operation".into(),
                    ));
                };
                let mut document = self
                    .owner
                    .core()
                    .export_history()
                    .map_err(|_| BundleError::HistoryChanged)?
                    .document;
                let document = std::sync::Arc::make_mut(&mut document);
                operation
                    .apply(document)
                    .map_err(|error| BundleError::Corrupt(error.to_string()))?;
                let asset = document
                    .media_pool
                    .iter()
                    .find(|asset| asset.id == *id)
                    .ok_or(BundleError::HistoryChanged)?;
                self.owner.apply_owned_media_interpretation(asset)?;
                self.view = Some(PreparedView::prepare(self.owner.core())?);
                self.message = "Colour interpretation changed.".into();
            }
            Action::ImportMedia(path) => {
                let (asset, _) = self.owner.import_owned_media(&path)?;
                self.imported_media = Some(asset.id);
                self.view = Some(PreparedView::prepare(self.owner.core())?);
                self.message = format!("Imported {}.", asset.name);
            }
            Action::Save => {
                if self
                    .owner
                    .association()
                    .is_some_and(|a| self.known_agent_effects.contains(a.path()))
                {
                    self.message = "A contains the agent command's effect as of its proven save; your current edits are unchanged. Saving this current edit is a separate successor save and omits that effect. Confirm this choice before saving.".into();
                    self.choice = Some(Choice::SuccessorSave(None));
                    return Ok(());
                }
                self.apply_inner(Action::ConfirmSuccessorSave)?;
            }
            Action::ConfirmSuccessorSave => {
                let destination = match self.choice.take() {
                    Some(Choice::SuccessorSave(path)) => path,
                    choice => {
                        self.choice = choice;
                        None
                    }
                };
                let report = self.owner.save_active()?;
                self.message = format!("Saved {}.", report.path.display());
                self.known_agent_effects.remove(&report.path);
                self.choice = None;
                #[cfg(test)]
                {
                    self.last_save = Some(report);
                }
                if let Some(path) = destination {
                    self.apply_inner(Action::SaveAs(path))?;
                }
            }
            Action::FirstSave(path) => {
                self.message = copy_text(&self.owner.first_save(&path, StorageLimits::default())?);
            }
            Action::SaveAs(path) => {
                self.owner.check_destination_change()?;
                if self
                    .owner
                    .association()
                    .is_some_and(|a| self.known_agent_effects.contains(a.path()))
                {
                    self.message = "A contains the agent command's effect as of its proven save; your current edits are unchanged. Save As first saves this current edit at A and omits that effect. Confirm this separate successor save.".into();
                    self.choice = Some(Choice::SuccessorSave(Some(path)));
                } else {
                    self.message = copy_text(&self.owner.save_as(&path, StorageLimits::default())?);
                }
            }
            Action::PreviewRecovery(target) => {
                self.owner.prepare_recovery(target)?;
                self.save_disclosure = self.owner.recovery_save_disclosure()?;
                self.message = format!(
                    "Editing is read-only while your current work is saved, then the selected rebound recovery point is opened. Your prior edit remains an ordinary working branch. {} {}",
                    self.save_disclosure,
                    self.owner.selection_availability()?
                );
                self.choice = Some(Choice::Recovery);
            }
            Action::OpenRecovery => {
                self.choice = None;
                if self.owner.dirty()? && !self.owner.has_unresolved_publication() {
                    self.save_disclosure = self.owner.recovery_save_disclosure()?;
                }
                let mut view = None;
                let result = self.owner.select_local_with_activation(|core| {
                    view = Some(PreparedView::prepare(core)?);
                    Ok(())
                });
                self.message = match &result {
                    ProjectSelectionResult::Activated(report) => {
                        self.view = view;
                        format!(
                            "Opened rebound recovery point in {}. Your prior work remains saved in its working branch.",
                            report.path.display()
                        )
                    }
                    ProjectSelectionResult::Unknown => self.unknown_text(),
                    ProjectSelectionResult::CommittedWithError(error) => format!(
                        "The selection committed; editing remains read-only while attachment is reconciled: {error}"
                    ),
                    ProjectSelectionResult::Refused(error) => {
                        self.message = if self.owner.dirty()? {
                            "Your current edit is unchanged. The save did not complete, so the recovery point was not opened. ".into()
                        } else {
                            "Your current work was saved in its working branch. The recovery point was not opened. Editing continues there. ".into()
                        };
                        let prefix = self.message.clone();
                        self.message =
                            format!("{prefix}{error}. {}", self.owner.selection_availability()?);
                        if matches!(error, BundleError::Capacity(_)) {
                            self.message.push_str(" There is not enough room to reopen the proposed project. Remove an eligible working branch, then retry this same selected point.");
                        }
                        self.message.clone()
                    }
                };
                self.message.push(' ');
                self.message.push_str(&self.save_disclosure);
                #[cfg(test)]
                {
                    self.last_selection = Some(result);
                }
            }
            Action::Retry => {
                let destination = self.owner.association().map(|a| a.path().to_path_buf());
                let pending = self.owner.pending_agent_command();
                let reconciliation = if self.owner.has_prepared_selection() {
                    self.apply_inner(Action::OpenRecovery)?;
                    Ok(())
                } else {
                    let result = self.owner.reconcile_active().map(|_| ());
                    self.message = "A's destination was checked; current edits and their Saved baseline are unchanged.".into();
                    result
                };
                // A failed ordinary/first-save Retry must still reach the phase-aware probe.
                self.offer = self.owner.probe_active()?;
                if pending && !self.owner.has_unresolved_publication() {
                    if self.owner.active_publication_state()? == PublicationState::Committed {
                        self.message.push_str(" A contains the agent command's effect as of its proven save; your current edits are unchanged. A later Save is a separate successor and will disclose that omission.");
                        if let Some(path) = destination {
                            self.known_agent_effects.insert(path);
                        }
                    } else {
                        self.message.push_str(" A shows the older or absent root; the pending command was not proved committed. Your current edits are unchanged.");
                    }
                }
                if self
                    .offer
                    .as_ref()
                    .is_some_and(|p| p.eligibility == RescueEligibility::Eligible)
                {
                    self.message.push_str(" A's save outcome is unknown. Your edits are still in this session. Save rescue copy is available.");
                }
                reconciliation?;
            }
            Action::Abandon => {
                self.message = self.owner.abandon_recovery()?;
                self.message
                    .push_str(" Selection explicitly abandoned. You may now change destination.");
                self.choice = None;
            }
            Action::PreviewRescue => {
                self.offer = self.owner.probe_active()?;
                let offer = self.offer.as_ref().ok_or(BundleError::HistoryChanged)?;
                let capture = self
                    .owner
                    .prepare_rescue(offer)?
                    .ok_or(BundleError::HistoryChanged)?;
                let selection = self
                    .owner
                    .preview_rescue(capture, self.owner.store()?.limits().clone())?;
                let consent = selection.consent();
                self.message = rescue_preview(&selection);
                self.message
                    .push_str(&self.owner.rescue_handoff_disclosure(&selection)?);
                self.choice = Some(Choice::Rescue {
                    selection: Box::new(selection),
                    consent,
                    trim: false,
                    omission: false,
                });
            }
            Action::ConsentTrim => {
                let Some(Choice::Rescue {
                    selection,
                    consent,
                    trim,
                    ..
                }) = &mut self.choice
                else {
                    return Err(BundleError::HistoryChanged);
                };
                selection.consent_trim(consent)?;
                *trim = true;
            }
            Action::ConsentOmission => {
                let Some(Choice::Rescue {
                    selection,
                    consent,
                    omission,
                    ..
                }) = &mut self.choice
                else {
                    return Err(BundleError::HistoryChanged);
                };
                selection.consent_omission(consent)?;
                *omission = true;
            }
            Action::RescueTo(path) => {
                let inspection = self.owner.inspect_rescue_destination(&path)?;
                self.rescue_with_inspection(&inspection)?;
                self.owner.set_path_spelling(&path);
            }
            Action::Cancel => {
                if let Some(Choice::Rescue { selection, .. }) = self.choice.take() {
                    self.owner.cancel_rescue_selection(*selection)?;
                }
                self.message = if self.owner.has_unresolved_publication() {
                    "Rescue copy was not saved. Editing remains read-only while A's save or selection is unresolved.".into()
                } else if self.owner.has_prepared_selection() {
                    "Cancelled. Your current edit is unchanged; the selected point remains prepared for retry or explicit abandonment.".into()
                } else {
                    "Cancelled. Your current edit is unchanged.".into()
                };
                self.choice = None;
            }
            Action::PreviewRetirement(branch) => {
                self.message = self.owner.retirement_disclosure(&branch)?;
                self.choice = Some(Choice::Remove {
                    branch,
                    root: self.owner.root_sha256()?,
                });
            }
            Action::RemoveBranch => {
                let Some(Choice::Remove { branch, root }) = self.choice.take() else {
                    return Err(BundleError::HistoryChanged);
                };
                let report = self.owner.retire_working_branch(&branch, &root, true)?;
                #[cfg(test)]
                {
                    self.last_retirement = Some(report);
                }
                #[cfg(not(test))]
                {
                    let _ = report;
                }
                self.message = "Working branch removed. Independent recovery, evidence, readers and source holds may retain its bytes. Retry uses the same selected point.".into();
            }
            Action::PreviewForget => {
                let point = self
                    .owner
                    .acknowledged_recovery_point()?
                    .ok_or(BundleError::HistoryChanged)?;
                self.message = format!(
                    "Forget the older recovery point from A: {}. Its history will no longer be offered by this rescue record. Independent evidence, readers and recovery roots can still retain these bytes. This does not settle A.",
                    point.sha256
                );
                self.choice = Some(Choice::Forget {
                    point,
                    root: self.owner.root_sha256()?,
                });
            }
            Action::Forget => {
                let Some(Choice::Forget { point, root }) = self.choice.take() else {
                    return Err(BundleError::HistoryChanged);
                };
                let report = self
                    .owner
                    .retire_rescue_recovery_point(&point, &root, true)?;
                #[cfg(test)]
                {
                    self.last_retirement = Some(report);
                }
                #[cfg(not(test))]
                {
                    let _ = report;
                }
                self.message = "The older recovery point from A was explicitly forgotten. Independent references remain.".into();
            }
            Action::Reconcile(path) => {
                let pending = self.owner.pending_agent_command_at(&path);
                let state = self.owner.reconcile_retained_destination(&path)?;
                self.message = format!(
                    "{}: {state:?}. Current edits and their Saved baseline are unchanged.",
                    path.display()
                );
                if pending && state == PublicationState::Committed {
                    self.message.push_str(" A contains the agent command's effect as of its proven save; your current edits are unchanged.");
                    self.known_agent_effects.insert(path.clone());
                }
                if state == PublicationState::Committed && self.owner.is_capture_destination(&path)
                {
                    self.message.push_str(" This destination holds a copy as of capture. Editing continues in the current session; you can open it separately read-only. A separate editable open requires explicit release of its completed destination owner.");
                }
            }
            Action::PreviewRelease(path) => {
                self.message = format!(
                    "Release {} for a separate editable open, only after verified commit. This will not switch editing, release A's source data or refund rescue slots.",
                    path.display()
                );
                self.choice = Some(Choice::Release(path));
            }
            Action::Release => {
                let Some(Choice::Release(path)) = self.choice.take() else {
                    return Err(BundleError::HistoryChanged);
                };
                self.owner.release_completed_capture(&path)?;
                self.message = "Completed capture owner explicitly released for a separate editable open. Current editing and original source retention are unchanged.".into();
            }
            Action::SaveSession(ref branch) | Action::CloseSession(ref branch) => {
                let close = matches!(&action, Action::CloseSession(_));
                let (_, mut guard) = self
                    .owner
                    .durable_sessions()
                    .into_iter()
                    .find(|(_, guard)| guard.branch_id() == branch)
                    .ok_or(BundleError::HistoryChanged)?;
                self.owner.persist_durable_branch(&mut guard)?;
                if close {
                    self.owner.close_durable_branch(&guard)?;
                }
                self.message = "Session saved. Retirement eligibility has been rechecked; a live session or required base remains protected.".into();
            }
            Action::OpenCapture(_) => {
                return Err(BundleError::Identity(
                    "Open B is handled by the project window".into(),
                ));
            }
        }
        Ok(())
    }
    fn rescue_with_inspection(
        &mut self,
        inspection: &kinewright_project::namespace::DestinationInspection,
    ) -> Result<(), BundleError> {
        let path = inspection.path();
        let Some(Choice::Rescue {
            selection, consent, ..
        }) = self.choice.take()
        else {
            return Err(BundleError::HistoryChanged);
        };
        let loss = rescue_preview(&selection);
        let mut view = None;
        let outcome =
            self.owner
                .rescue_with_activation(*selection, &consent, inspection, |core| {
                    view = Some(PreparedView::prepare(core)?);
                    Ok(())
                })?;
        self.finish_rescue(&outcome, view, path, &loss);
        Ok(())
    }
    fn finish_rescue(
        &mut self,
        outcome: &kinewright_project::bundle_rescue::RescueOutcome,
        view: Option<PreparedView>,
        path: &Path,
        loss: &str,
    ) {
        self.message = if outcome.activated {
            self.view = view;
            format!(
                "Rescue copy saved to {}. Editing continues in B. A may also contain a copy from this session; its save outcome is unknown. {} {loss}",
                path.display(),
                outcome.disclosure
            )
        } else if matches!(outcome.status, CopyStatus::Committed(_)) {
            outcome.disclosure.clone()
        } else if self.owner.has_prepared_selection() {
            "B's save outcome is unknown. Editing remains read-only while A's save or selection is unresolved.".into()
        } else {
            "B's save outcome is unknown. Editing continues in the original session.".into()
        };
        #[cfg(test)]
        {
            self.last_rescue = Some(outcome.clone());
        }
    }
    fn unknown_text(&self) -> String {
        if self.owner.preliminary_save_pending() {
            "Your current save outcome is unknown. Editing is read-only while it is checked; the recovery point has not been opened.".into()
        } else {
            "Your current work is durable in the project at its current location. Editing is read-only while the recovery selection is checked. If this storage never comes back, Save rescue copy is the way to keep your work. Closing can discard the only accessible copy in this session.".into()
        }
    }
    #[allow(clippy::too_many_lines)]
    pub fn show(&self, ui: &mut egui::Ui) -> Option<Action> {
        let mut action = None;
        ui.label(&self.message);
        if self.retention_lost {
            ui.label("Queries and history export remain available. Close this editing context explicitly to end retention.");
            return None;
        }
        if let Some(host) = &self.host {
            return host.show(ui);
        }
        if self.owner.has_prepared_selection() {
            ui.label(self.owner.selection_availability().unwrap_or_default());
        }
        match &self.choice {
            Some(Choice::SuccessorSave(_)) => {
                if ui
                    .button("Save this current edit without that agent effect")
                    .clicked()
                {
                    action = Some(Action::ConfirmSuccessorSave);
                }
            }
            Some(Choice::Recovery) => {
                if ui
                    .button("Save current work, then open recovery point")
                    .clicked()
                {
                    action = Some(Action::OpenRecovery);
                }
            }
            Some(Choice::Rescue {
                selection,
                trim,
                omission,
                ..
            }) => {
                if selection.removed_undo() > 0
                    && !trim
                    && ui.button("Remove this older Undo history in B").clicked()
                {
                    action = Some(Action::ConsentTrim);
                }
                if !selection.omissions().is_empty()
                    && (selection.removed_undo() == 0 || *trim)
                    && !omission
                    && ui
                        .button("Omit these unavailable recovery points")
                        .clicked()
                {
                    action = Some(Action::ConsentOmission);
                }
                if (selection.removed_undo() == 0 || *trim)
                    && (selection.omissions().is_empty() || *omission)
                    && ui.button("Save rescue copy to…").clicked()
                    && let Some(path) = rfd::FileDialog::new()
                        .add_filter("Kinewright project", &["kinewright"])
                        .save_file()
                {
                    action = Some(Action::RescueTo(path));
                }
            }
            Some(Choice::Remove { .. }) => {
                if ui.button("Confirm working branch loss").clicked() {
                    action = Some(Action::RemoveBranch);
                }
            }
            Some(Choice::Forget { .. }) => {
                if ui
                    .button("Forget the older recovery point from A")
                    .clicked()
                {
                    action = Some(Action::Forget);
                }
            }
            Some(Choice::Release(_)) => {
                if ui.button("Release completed destination owner").clicked() {
                    action = Some(Action::Release);
                }
            }
            None => {
                if ui.button("Retry A / selection").clicked() {
                    action = Some(Action::Retry);
                }
                if self
                    .offer
                    .as_ref()
                    .is_some_and(|p| p.eligibility == RescueEligibility::Eligible)
                    && ui.button("Save rescue copy").clicked()
                {
                    action = Some(Action::PreviewRescue);
                }
                if !self.frozen {
                    for (index, point) in self.points.iter().enumerate() {
                        if ui
                            .button(format!("Open rebound recovery point {}", index + 1))
                            .clicked()
                        {
                            action = Some(Action::PreviewRecovery(point.clone()));
                        }
                    }
                    for (branch, blockers) in &self.branches {
                        ui.label(format!(
                            "Working branch {}: {}",
                            branch.as_str(),
                            blockers.join(" ")
                        ));
                        if blockers.is_empty()
                            && ui
                                .button(format!("Save current edit, then open {}", branch.as_str()))
                                .clicked()
                        {
                            action = Some(Action::PreviewRecovery(
                                RecoverySelection::WorkingBranch(branch.clone()),
                            ));
                        }
                        if self.candidates.contains(branch)
                            && ui
                                .button(format!("Remove this working branch {}", branch.as_str()))
                                .clicked()
                        {
                            action = Some(Action::PreviewRetirement(branch.clone()));
                        }
                        for (name, guard) in self.owner.durable_sessions() {
                            if guard.branch_id() == branch || &guard.base().target_branch == branch
                            {
                                ui.label(format!("This branch is in use by {name} / specialist base. Save or close that session first."));
                                if ui.button(format!("Save this session: {name}")).clicked() {
                                    action = Some(Action::SaveSession(guard.branch_id().clone()));
                                }
                                if ui.button(format!("Close this session: {name}")).clicked() {
                                    action = Some(Action::CloseSession(guard.branch_id().clone()));
                                }
                            }
                        }
                    }
                    if self
                        .owner
                        .acknowledged_recovery_point()
                        .ok()
                        .flatten()
                        .is_some()
                        && ui
                            .button("Forget the older recovery point from A…")
                            .clicked()
                    {
                        action = Some(Action::PreviewForget);
                    }
                }
                for (path, state) in &self.destinations {
                    if *state == PublicationState::Committed
                        && self.owner.is_capture_destination(path)
                        && ui
                            .button(format!("Open B read-only: {}", path.display()))
                            .clicked()
                    {
                        action = Some(Action::OpenCapture(path.clone()));
                    }
                    if ui
                        .button(format!("Check {} ({state:?})", path.display()))
                        .clicked()
                    {
                        action = Some(Action::Reconcile(path.clone()));
                    }
                    if *state == PublicationState::Committed
                        && self.owner.is_capture_destination(path)
                        && ui
                            .button(format!("Release completed copy {}…", path.display()))
                            .clicked()
                    {
                        action = Some(Action::PreviewRelease(path.clone()));
                    }
                }
            }
        }
        if self.choice.is_some() && ui.button("Cancel").clicked() {
            action = Some(Action::Cancel);
        }
        if self.owner.has_prepared_selection() && !self.owner.has_unresolved_publication() {
            ui.label("Abandoning the selection lets a session-only point go. A crash can lose it.");
            if ui.button("Abandon this selection explicitly").clicked() {
                action = Some(Action::Abandon);
            }
        }
        action
    }
}
const RETENTION_LOST_TEXT: &str = kinewright_agent::writer_host::RETENTION_LOST;

fn copy_text(status: &CopyStatus) -> String {
    match status {
        CopyStatus::Committed(report) => format!("Saved {}.", report.path.display()),
        CopyStatus::CleanupPending { diagnostic } => format!("Save failed; cleanup pending. {diagnostic}"),
        CopyStatus::DurabilityUnknown => "This destination's save outcome is unknown. Your edits are still in this session. Retry A to check it.".into(),
        CopyStatus::DefinitelyUnpublished => "Save did not complete. Your current edit is unchanged.".into(),
        CopyStatus::CommittedWithError { diagnostic } => format!("The root was saved, but its attachment still needs reconciliation. {diagnostic}"),
    }
}
fn rescue_preview(selection: &RescueSelection) -> String {
    let mut text = if selection.removed_undo() > 0 {
        format!(
            "Save a rescue copy with older Undo history removed in B. Undo before step {} will be unavailable there; {} oldest Undo entries are removed. Current edits, Redo, and the open gesture are kept. A is unchanged. ",
            selection.undo_floor(),
            selection.removed_undo()
        )
    } else {
        "Save a rescue copy with all Undo and Redo kept. ".into()
    };
    if !selection.omissions().is_empty() {
        text.push_str("Older recovery points from A are not in the rescue copy. ");
        for omission in selection.omissions() {
            let _ = write!(
                text,
                "{} {}: {} ({}) ",
                omission.role, omission.root.sha256, omission.cause, omission.operation
            );
        }
        if selection
            .omissions()
            .iter()
            .any(|o| o.role == "acknowledged_root")
        {
            text.push_str(
                "The recovery point for A's last acknowledged save is not in the rescue copy. ",
            );
        }
    }
    text
}

#[cfg(test)]
#[path = "ja1_attachment_tests.rs"]
pub(crate) mod tests;

#[cfg(all(test, target_os = "linux"))]
impl Ja1Attachment {
    pub(crate) fn launch_protocol_fixture(
        &mut self,
        roots: kinewright_agent::writer_host::SessionRoots,
        scheduler: PathBuf,
        fixture: &kinewright_agent::writer_host::ProtocolFixture,
    ) -> Result<kinewright_agent::writer_host::WriterHost, BundleError> {
        use kinewright_agent::writer_host::{
            HostConfig, HostOrigin, HostSnapshot, HostSourceBinding, SourceServer, WriterHost,
        };
        let transfer = self.owner.prepare_host_attachment()?;
        let context = Arc::downgrade(&self.originating_context);
        let retained_context = context.clone();
        let server = if let Some(reader) = transfer.source_service {
            SourceServer::start(reader)
        } else {
            SourceServer::start_context(move || {
                if context
                    .upgrade()
                    .is_some_and(|live| live.load(std::sync::atomic::Ordering::Acquire))
                {
                    Ok(())
                } else {
                    Err(BundleError::Identity("fixture GUI context closed".into()))
                }
            })
        }
        .map_err(|error| BundleError::Corrupt(error.to_string()))?;
        let writer = WriterHost::start_protocol_fixture(
            HostConfig {
                scheduler_settings: Some(scheduler),
                project: "c4-notes".into(),
                path: transfer.named_path,
                roots,
                origin: HostOrigin::Gui {
                    initial_snapshot: HostSnapshot::from_history(transfer.history)
                        .map_err(|error| BundleError::Corrupt(error.to_string()))?,
                    retention: HostSourceBinding {
                        inputs: transfer.source.as_ref().map(|_| transfer.inputs.clone()),
                        source: transfer.source.clone(),
                        callback: server.bootstrap(),
                    },
                    expected_root_sha256: transfer.expected_root_sha256,
                    expected_project_id: transfer.project_id,
                },
            },
            fixture,
        )
        .map_err(|error| BundleError::Corrupt(error.to_string()))?;
        let cut = self.owner.freeze_host_cut()?;
        self.owner.complete_host_cut(&cut, None, None)?;
        self.host = Some(host::HostAttachment::connected(
            writer.client(),
            transfer.source,
            transfer.inputs,
            server,
            retained_context,
        )?);
        self.start_host_manual()?;
        Ok(writer)
    }
}
