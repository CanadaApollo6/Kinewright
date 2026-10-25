//! GUI client state for the shipping writer host. Source ownership stays in the GUI.
use super::{Action, RETENTION_LOST_TEXT};
use eframe::egui;
use kinewright_agent::writer_host::{
    HostChoiceKind, HostClient, HostCommand, HostConfig, HostOrigin, HostReply, HostSnapshot,
    HostSourceBinding, HostStatus, SessionRoots, SourceServer,
};
use kinewright_core::AcceptedCut;
use kinewright_project::{
    bundle::BundleError, bundle_records::ObjectRef, editing_session::Ja1EditingSession,
    source_boundary::BorrowedSourceDescriptor,
};
use std::{
    path::{Path, PathBuf},
    process::Child,
    thread::JoinHandle,
    time::{Duration, Instant},
};

pub(super) struct HostAttachment {
    pub client: HostClient,
    pub status: HostStatus,
    source: Option<BorrowedSourceDescriptor>,
    inputs: ObjectRef,
    // Callback execution borrows the real GUI admission; it has no source Close callback.
    source_server: Option<SourceServer>,
    child: Option<Child>,
    pub cut: Option<AcceptedCut>,
    heartbeat: Option<JoinHandle<Result<HostReply, kinewright_agent::writer_host::HostError>>>,
    last_heartbeat: Instant,
}

impl HostAttachment {
    pub fn launch(
        owner: &mut Ja1EditingSession,
        executable: &Path,
        project: String,
        roots: SessionRoots,
        context: std::sync::Weak<std::sync::atomic::AtomicBool>,
    ) -> Result<Self, BundleError> {
        let transfer = owner.prepare_host_attachment()?;
        let source_server = Some(if let Some(reader) = transfer.source_service {
            SourceServer::start(reader).map_err(host_error)?
        } else {
            SourceServer::start_context(move || {
                if context
                    .upgrade()
                    .is_some_and(|live| live.load(std::sync::atomic::Ordering::Acquire))
                {
                    Ok(())
                } else {
                    Err(BundleError::Identity(
                        "the originating GUI context is closed".into(),
                    ))
                }
            })
            .map_err(host_error)?
        });
        let retention = HostSourceBinding {
            source: transfer.source.clone(),
            inputs: transfer.source.as_ref().map(|_| transfer.inputs.clone()),
            callback: source_server
                .as_ref()
                .expect("GUI source service")
                .bootstrap(),
        };
        let (client, mut child) = HostClient::launch(
            executable,
            HostConfig {
                project,
                path: transfer.named_path,
                roots,
                origin: HostOrigin::Gui {
                    retention,
                    initial_snapshot: HostSnapshot::from_history(transfer.history)
                        .map_err(host_error)?,
                    expected_root_sha256: transfer.expected_root_sha256,
                    expected_project_id: transfer.project_id,
                },
            },
        )
        .map_err(host_error)?;
        let attached = client
            .request(HostCommand::Attach)
            .map_err(host_error)
            .and_then(|reply| {
                if let Some(error) = reply.error {
                    return Err(BundleError::Identity(error.to_string()));
                }
                if !reply.status.retention_proven {
                    return Err(BundleError::Identity(RETENTION_LOST_TEXT.into()));
                }
                Ok(reply.status)
            });
        let status = match attached {
            Ok(status) => status,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
        };
        Ok(Self {
            client,
            status,
            source: transfer.source,
            inputs: transfer.inputs,
            source_server,
            child: Some(child),
            cut: None,
            heartbeat: None,
            last_heartbeat: Instant::now(),
        })
    }

    #[cfg(test)]
    pub(super) fn connected(
        client: HostClient,
        source: Option<BorrowedSourceDescriptor>,
        inputs: ObjectRef,
        source_server: SourceServer,
    ) -> Result<Self, BundleError> {
        let reply = client.request(HostCommand::Attach).map_err(host_error)?;
        if let Some(error) = reply.error {
            return Err(BundleError::Identity(error.to_string()));
        }
        if !reply.status.retention_proven {
            return Err(BundleError::Identity(RETENTION_LOST_TEXT.into()));
        }
        Ok(Self {
            client,
            status: reply.status,
            source,
            inputs,
            source_server: Some(source_server),
            child: None,
            cut: None,
            heartbeat: None,
            last_heartbeat: Instant::now(),
        })
    }

    #[cfg(all(test, target_os = "linux"))]
    pub(super) fn child_id(&self) -> Option<u32> {
        self.child.as_ref().map(Child::id)
    }

    #[allow(
        clippy::too_many_lines,
        reason = "one exhaustive mapping of App choices to host commands"
    )]
    pub fn request(
        &mut self,
        action: Action,
        owner: &mut Ja1EditingSession,
    ) -> Result<HostReply, BundleError> {
        // A status response issued before this command cannot replace its newer metadata.
        self.heartbeat = None;
        self.last_heartbeat = Instant::now();
        let needs_cut = matches!(
            &action,
            Action::Save
                | Action::ConfirmSuccessorSave
                | Action::SaveAs(_)
                | Action::FirstSave(_)
                | Action::PreviewRescue
                | Action::PreviewRecovery(_)
        );
        if !self.status.retention_proven {
            return Err(BundleError::Identity(RETENTION_LOST_TEXT.into()));
        }
        if needs_cut && self.cut.is_none() {
            self.cut = Some(owner.freeze_host_cut()?);
        }
        if needs_cut {
            let _ = owner
                .ensure_host_snapshot(self.cut.as_ref().ok_or(BundleError::HistoryChanged)?)?;
        }
        let snapshot = || -> Result<HostSnapshot, BundleError> {
            let history = self
                .cut
                .as_ref()
                .ok_or(BundleError::HistoryChanged)?
                .history()
                .clone();
            let bytes =
                serde_json::to_vec(&history).map_err(|e| BundleError::Corrupt(e.to_string()))?;
            Ok(HostSnapshot {
                history,
                history_sha256: kinewright_media::sha256_bytes(&bytes),
            })
        };
        let command = match action {
            Action::Save | Action::ConfirmSuccessorSave => HostCommand::Save {
                snapshot: snapshot()?,
            },
            Action::SaveAs(destination) => HostCommand::SaveAs {
                destination,
                snapshot: snapshot()?,
            },
            Action::FirstSave(destination) => HostCommand::FirstSave {
                destination,
                snapshot: snapshot()?,
                source: self.source.clone().ok_or(BundleError::HistoryChanged)?,
                inputs: self.inputs.clone(),
                callback: self
                    .source_server
                    .as_ref()
                    .ok_or(BundleError::HistoryChanged)?
                    .bootstrap(),
            },
            Action::Retry => HostCommand::Retry,
            Action::PreviewRecovery(target) => HostCommand::PreviewRecovery {
                target: target.into(),
                snapshot: snapshot()?,
            },
            Action::OpenRecovery => HostCommand::OpenRecovery,
            Action::Abandon => HostCommand::Abandon,
            Action::PreviewRescue => HostCommand::PreviewRescue {
                snapshot: snapshot()?,
            },
            Action::ConsentTrim => HostCommand::ConsentTrim,
            Action::ConsentOmission => HostCommand::ConsentOmission,
            Action::RescueTo(destination) => HostCommand::RescueTo { destination },
            Action::Cancel => HostCommand::Cancel,
            Action::PreviewRetirement(branch) => HostCommand::PreviewRetirement { branch },
            Action::RemoveBranch => HostCommand::RemoveBranch,
            Action::PreviewForget => HostCommand::PreviewForget,
            Action::Forget => HostCommand::Forget,
            Action::Reconcile(destination) => HostCommand::Reconcile { destination },
            Action::PreviewRelease(destination) => HostCommand::PreviewRelease { destination },
            Action::Release => HostCommand::Release,
            Action::SaveSession(branch) => HostCommand::SaveSession { branch },
            Action::CloseSession(branch) => HostCommand::CloseSession { branch },
        };
        let client = match &command {
            HostCommand::FirstSave { destination, .. }
            | HostCommand::SaveAs { destination, .. }
            | HostCommand::RescueTo { destination } => {
                // These paths came from the person's explicit file picker. The host admits
                // only this pinned destination; connectors cannot invoke this GUI-only route.
                self.client
                    .admit_destination(destination)
                    .map_err(host_error)?
            }
            _ => self.client.clone(),
        };
        client.request(command).map_err(host_error)
    }

    pub fn read_only(&self) -> bool {
        self.cut.is_some() || self.status.frozen || self.status.choice.is_some()
    }

    pub fn accept_status(&mut self, status: HostStatus) {
        self.client = self.client.with_roots(status.admitted_roots.clone());
        self.status = status;
    }

    /// The frame thread polls only completed work or the child process handle.
    pub fn poll(&mut self) -> Result<Option<HostReply>, BundleError> {
        if let Some(child) = &mut self.child
            && child.try_wait().map_err(BundleError::Io)?.is_some()
        {
            return Err(BundleError::Identity(RETENTION_LOST_TEXT.into()));
        }
        if self.heartbeat.as_ref().is_some_and(JoinHandle::is_finished) {
            let reply = self
                .heartbeat
                .take()
                .expect("completed heartbeat")
                .join()
                .map_err(|_| BundleError::Identity(RETENTION_LOST_TEXT.into()))?
                .map_err(host_error)?;
            self.last_heartbeat = Instant::now();
            return Ok(Some(reply));
        }
        if self.heartbeat.is_none() && self.last_heartbeat.elapsed() >= Duration::from_secs(1) {
            let client = self.client.clone();
            self.heartbeat = Some(std::thread::spawn(move || {
                client.request(HostCommand::Status)
            }));
        }
        Ok(None)
    }

    pub fn close(&mut self) {
        // Explicit originating Close, as distinct from closing/dropping a chat client.
        if !self
            .child
            .as_mut()
            .is_some_and(|child| child.try_wait().is_ok_and(|status| status.is_some()))
        {
            let _ = self.client.request(HostCommand::Shutdown);
        }
        self.source_server = None;
        if let Some(child) = &mut self.child {
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                match child.try_wait() {
                    Ok(Some(_)) => break,
                    Ok(None) if Instant::now() < deadline => {
                        std::thread::park_timeout(Duration::from_millis(5));
                    }
                    Ok(None) | Err(_) => {
                        let _ = child.kill();
                        let _ = child.wait();
                        break;
                    }
                }
            }
        }
    }

    #[allow(clippy::too_many_lines)]
    pub fn show(&self, ui: &mut egui::Ui) -> Option<Action> {
        let mut action = None;
        ui.label(&self.status.selection_availability);
        ui.label(&self.status.save_disclosure);
        match self.status.choice {
            Some(HostChoiceKind::Recovery) => {
                if ui
                    .button("Save current work, then open recovery point")
                    .clicked()
                {
                    action = Some(Action::OpenRecovery);
                }
            }
            Some(HostChoiceKind::Rescue) => {
                let trim = self.status.rescue_trim_accepted;
                let omission = self.status.rescue_omission_accepted;
                let needs_trim = self.status.rescue_trim_required;
                let needs_omission = self.status.rescue_omission_required;
                if needs_trim && !trim && ui.button("Remove this older Undo history in B").clicked()
                {
                    action = Some(Action::ConsentTrim);
                }
                for text in &self.status.rescue_omissions {
                    ui.label(text);
                }
                if needs_omission
                    && (!needs_trim || trim)
                    && !omission
                    && ui
                        .button("Omit these unavailable recovery points")
                        .clicked()
                {
                    action = Some(Action::ConsentOmission);
                }
                if (!needs_trim || trim)
                    && (!needs_omission || omission)
                    && ui.button("Save rescue copy to…").clicked()
                    && let Some(path) = rfd::FileDialog::new()
                        .add_filter("Kinewright project", &["kinewright"])
                        .save_file()
                {
                    action = Some(Action::RescueTo(path));
                }
            }
            Some(HostChoiceKind::Remove) => {
                if ui.button("Confirm working branch loss").clicked() {
                    action = Some(Action::RemoveBranch);
                }
            }
            Some(HostChoiceKind::Forget) => {
                if ui
                    .button("Forget the older recovery point from A")
                    .clicked()
                {
                    action = Some(Action::Forget);
                }
            }
            Some(HostChoiceKind::Release) => {
                if ui.button("Release completed destination owner").clicked() {
                    action = Some(Action::Release);
                }
            }
            None => {
                if ui.button("Retry A / selection").clicked() {
                    action = Some(Action::Retry);
                }
                if self.status.unresolved && ui.button("Save rescue copy").clicked() {
                    action = Some(Action::PreviewRescue);
                }
                if !self.status.frozen {
                    for (index, point) in self.status.recovery_points.iter().enumerate() {
                        if ui
                            .button(format!("Open rebound recovery point {}", index + 1))
                            .clicked()
                        {
                            action = Some(Action::PreviewRecovery(point.clone().into()));
                        }
                    }
                    for (branch, _, blockers) in &self.status.branches {
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
                            action = Some(Action::PreviewRecovery(kinewright_project::bundle_recovery::RecoverySelection::WorkingBranch(branch.clone())));
                        }
                        if self.status.retirement_candidates.contains(branch)
                            && ui
                                .button(format!("Remove this working branch {}", branch.as_str()))
                                .clicked()
                        {
                            action = Some(Action::PreviewRetirement(branch.clone()));
                        }
                        if !blockers.is_empty() {
                            ui.label("This branch is in use by a session or specialist base. Save or close that session first.");
                            if ui
                                .button(format!("Save this session: {}", branch.as_str()))
                                .clicked()
                            {
                                action = Some(Action::SaveSession(branch.clone()));
                            }
                            if ui
                                .button(format!("Close this session: {}", branch.as_str()))
                                .clicked()
                            {
                                action = Some(Action::CloseSession(branch.clone()));
                            }
                        }
                    }
                    if ui
                        .button("Forget the older recovery point from A…")
                        .clicked()
                    {
                        action = Some(Action::PreviewForget);
                    }
                }
                for (path, _) in &self.status.destinations {
                    if ui.button(format!("Check {}", path.display())).clicked() {
                        action = Some(Action::Reconcile(path.clone()));
                    }
                    if ui
                        .button(format!("Release completed copy {}…", path.display()))
                        .clicked()
                    {
                        action = Some(Action::PreviewRelease(path.clone()));
                    }
                }
            }
        }
        if self.status.choice.is_some() && ui.button("Cancel").clicked() {
            action = Some(Action::Cancel);
        }
        if self.status.prepared_selection && !self.status.unresolved {
            ui.label("Abandoning the selection lets a session-only point go. A crash can lose it.");
            if ui.button("Abandon this selection explicitly").clicked() {
                action = Some(Action::Abandon);
            }
        }
        action
    }
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "map_err consumes the owned transport error at this boundary"
)]
fn host_error(error: kinewright_agent::writer_host::HostError) -> BundleError {
    BundleError::Identity(error.to_string())
}

pub(super) fn executable() -> PathBuf {
    let name = if cfg!(windows) {
        "kinewright.exe"
    } else {
        "kinewright"
    };
    std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|parent| parent.join(name)))
        .filter(|path| path.is_file())
        .unwrap_or_else(|| PathBuf::from(name))
}
