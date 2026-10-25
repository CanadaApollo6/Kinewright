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

#[path = "ja1_host_recovery.rs"]
mod recovery;

pub(super) struct HostAttachment {
    pub(super) switch_envelope: Option<kinewright_project::receipts::CommandEnvelope>,
    prepared_switch: Option<(
        kinewright_project::editing_session::PreparedGuiSwitch,
        super::PreparedView,
    )>,
    executable: PathBuf,
    context: std::sync::Weak<std::sync::atomic::AtomicBool>,
    recovery: Option<JoinHandle<Result<recovery::RecoveredManual, BundleError>>>,
    recovery_plan:
        std::sync::Arc<std::sync::Mutex<Option<kinewright_agent::writer_host::RecoveryBarrier>>>,
    recovery_originals: Vec<kinewright_core::AcceptedHistory>,
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
    pub manual: Option<super::ManualProducerGate>,
    manual_client: String,
    manual_inflight:
        Option<JoinHandle<Result<HostReply, kinewright_agent::writer_host::HostError>>>,
}

impl HostAttachment {
    #[cfg(all(test, target_os = "linux"))]
    pub(super) fn prepared_switch_probe(&self) -> Option<kinewright_core::Core> {
        self.prepared_switch
            .as_ref()
            .map(|(actor, _)| actor.core().clone())
    }
    pub fn restore_switch(
        &mut self,
        owner: &mut Ja1EditingSession,
        committed: &kinewright_project::bundle_switch::SwitchAttachment,
    ) -> Result<(HostReply, super::PreparedView), BundleError> {
        committed.verify()?;
        let cut = self.cut.as_ref().ok_or(BundleError::HistoryChanged)?;
        let mut prepared = None;
        if self
            .prepared_switch
            .as_ref()
            .is_some_and(|(actor, _)| !actor.matches(committed))
        {
            self.prepared_switch = None;
        }
        if let Some((actor, view)) = self.prepared_switch.take() {
            owner.activate_prepared_gui_switch(cut, committed, actor)?;
            prepared = Some(view);
        } else {
            owner.activate_host_switch(cut, committed, |core| {
                prepared = Some(super::PreparedView::prepare(core)?);
                Ok(())
            })?;
        }
        // The fresh GUI actor remains Frozen while the host checks the current pointer.
        let reply = self
            .client
            .request(HostCommand::ConfirmSwitch {
                client: self.manual_client.clone(),
                installed: Box::new(committed.identity()?),
            })
            .map_err(host_error)?;
        if let Some(error) = &reply.error {
            return Err(BundleError::Identity(error.to_string()));
        }
        let permit = owner.finish_host_switch(
            reply
                .manual_lease
                .clone()
                .ok_or(BundleError::HistoryChanged)?,
            reply
                .manual_inputs
                .clone()
                .ok_or(BundleError::HistoryChanged)?,
            self.context.upgrade().ok_or(BundleError::HistoryChanged)?,
        )?;
        self.inputs = committed.current.inputs.clone();
        self.manual = Some(super::ManualProducerGate {
            actor: owner.core().actor_identity(),
            permit,
        });
        self.client = self.client.with_manual_editor(self.manual_client.clone());
        self.cut = None;
        Ok((reply, prepared.ok_or(BundleError::HistoryChanged)?))
    }
    pub fn launch(
        owner: &mut Ja1EditingSession,
        executable: &Path,
        project: String,
        roots: SessionRoots,
        context: std::sync::Weak<std::sync::atomic::AtomicBool>,
    ) -> Result<Self, BundleError> {
        let transfer = owner.prepare_host_attachment()?;
        let retained_context = context.clone();
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
            prepared_switch: None,
            switch_envelope: None,
            executable: executable.to_path_buf(),
            context: retained_context,
            recovery: None,
            recovery_plan: std::sync::Arc::default(),
            recovery_originals: vec![],
            client,
            status,
            source: transfer.source,
            inputs: transfer.inputs,
            source_server,
            child: Some(child),
            cut: None,
            heartbeat: None,
            last_heartbeat: Instant::now(),
            manual: None,
            manual_client: serde_json::to_value(
                kinewright_agent::writer_host::HostAuth::generate().map_err(host_error)?,
            )
            .map_err(|e| BundleError::Corrupt(e.to_string()))?
            .as_str()
            .ok_or(BundleError::HistoryChanged)?
            .to_owned(),
            manual_inflight: None,
        })
    }

    #[cfg(all(test, target_os = "linux"))]
    pub(super) fn connected(
        client: HostClient,
        source: Option<BorrowedSourceDescriptor>,
        inputs: ObjectRef,
        source_server: SourceServer,
        context: std::sync::Weak<std::sync::atomic::AtomicBool>,
    ) -> Result<Self, BundleError> {
        let reply = client.request(HostCommand::Attach).map_err(host_error)?;
        if let Some(error) = reply.error {
            return Err(BundleError::Identity(error.to_string()));
        }
        if !reply.status.retention_proven {
            return Err(BundleError::Identity(RETENTION_LOST_TEXT.into()));
        }
        Ok(Self {
            prepared_switch: None,
            switch_envelope: None,
            executable: PathBuf::new(),
            context,
            recovery: None,
            recovery_plan: std::sync::Arc::default(),
            recovery_originals: vec![],
            client,
            status: reply.status,
            source,
            inputs,
            source_server: Some(source_server),
            child: None,
            cut: None,
            heartbeat: None,
            last_heartbeat: Instant::now(),
            manual: None,
            manual_client: serde_json::to_value(
                kinewright_agent::writer_host::HostAuth::generate().map_err(host_error)?,
            )
            .map_err(|e| BundleError::Corrupt(e.to_string()))?
            .as_str()
            .ok_or(BundleError::HistoryChanged)?
            .to_owned(),
            manual_inflight: None,
        })
    }

    pub fn start_manual(
        &mut self,
        owner: &mut Ja1EditingSession,
    ) -> Result<Option<super::PreparedView>, BundleError> {
        if self
            .manual
            .as_ref()
            .is_some_and(|gate| gate.actor == owner.core().actor_identity())
        {
            return Ok(None);
        }
        // C9 has no absent-suffix/floor/recovery authorization. A first-save or
        // dirty bootstrap must explicitly Save before granting a durable identity.
        let digest = HostSnapshot::capture(owner.core())
            .map_err(host_error)?
            .history_sha256;
        if self.status.association.is_none() || self.status.saved_digest.as_deref() != Some(&digest)
        {
            if self.cut.is_none() {
                self.cut = Some(owner.freeze_host_cut()?);
            }
            return Ok(None);
        }
        if self.status.frozen
            && self.status.association.is_some()
            && !self.status.prepared_selection
            && !self.status.unresolved
        {
            let current = self
                .client
                .request(HostCommand::CurrentAttachment)
                .map_err(host_error)?;
            if let Some(committed) = current.switch_attachment
                && committed.switch_generation != 0
            {
                if self.cut.is_none() {
                    self.cut = Some(owner.recapture_host_recovery_cut()?);
                }
                let (reply, view) = self.restore_switch(owner, &committed)?;
                self.accept_status(reply.status);
                return Ok(Some(view));
            }
        }
        let reply = self
            .client
            .request(HostCommand::AcquireManual {
                client: self.manual_client.clone(),
            })
            .map_err(host_error)?;
        if let Some(error) = reply.error {
            return Err(BundleError::Identity(error.to_string()));
        }
        let lease = reply.manual_lease.ok_or(BundleError::HistoryChanged)?;
        let inputs = reply.manual_inputs.ok_or(BundleError::HistoryChanged)?;
        let permit = owner.bind_manual_producer(lease, inputs)?;
        self.client = self.client.with_manual_editor(self.manual_client.clone());
        self.manual = Some(super::ManualProducerGate {
            actor: owner.core().actor_identity(),
            permit,
        });
        self.accept_status(reply.status);
        Ok(None)
    }
    fn drain_manual(&mut self, owner: &Ja1EditingSession) -> Result<(), BundleError> {
        if let Some(worker) = self.manual_inflight.take() {
            let reply = worker
                .join()
                .map_err(|_| BundleError::HistoryChanged)?
                .map_err(host_error)?;
            self.accept_manual_ack(&reply, owner)?;
            self.accept_status(reply.status);
            if let Some(error) = reply.error {
                return Err(BundleError::Identity(error.to_string()));
            }
        }
        if let Some(gate) = &self.manual {
            let messages = gate
                .permit
                .pending()
                .map_err(|e| BundleError::Identity(e.to_string()))?;
            if !messages.is_empty() {
                let reply = self
                    .client
                    .request(HostCommand::CommitHistory {
                        client: self.manual_client.clone(),
                        messages,
                    })
                    .map_err(host_error)?;
                self.accept_manual_ack(&reply, owner)?;
                self.accept_status(reply.status);
                if let Some(error) = reply.error {
                    return Err(BundleError::Identity(error.to_string()));
                }
            }
        }
        Ok(())
    }
    pub fn accept_manual_ack(
        &self,
        reply: &HostReply,
        owner: &Ja1EditingSession,
    ) -> Result<(), BundleError> {
        if !reply.manual_ack.is_empty() {
            owner.acknowledge_manual(
                &self
                    .manual
                    .as_ref()
                    .ok_or(BundleError::HistoryChanged)?
                    .permit,
                reply.manual_ack.clone(),
            )?;
        }
        Ok(())
    }

    #[cfg(all(test, target_os = "linux"))]
    pub(super) fn kill_owned_child(&mut self) {
        let child = self.child.as_mut().expect("owned child");
        child.kill().unwrap();
        child.wait().unwrap();
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
        if self.recovery.is_some() {
            return Err(BundleError::CoreMutationRefused(
                kinewright_core::CoreMutationRefusal::Frozen,
            ));
        }
        let needs_cut = (matches!(&action, Action::Retry)
            && self.switch_envelope.is_some()
            && self.status.prepared_selection)
            || matches!(
                &action,
                Action::Save
                    | Action::ConfirmSuccessorSave
                    | Action::SaveAs(_)
                    | Action::FirstSave(_)
                    | Action::PreviewRescue
                    | Action::PreviewRecovery(_)
            );
        if needs_cut && self.cut.is_none() {
            self.cut = Some(owner.freeze_host_cut()?);
        }
        self.drain_manual(owner)?;
        if matches!(
            &action,
            Action::SaveAs(_) | Action::FirstSave(_) | Action::OpenRecovery | Action::RescueTo(_)
        ) {
            self.manual = None;
        }
        // A status response issued before this command cannot replace its newer metadata.
        self.heartbeat = None;
        self.last_heartbeat = Instant::now();
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
            Action::Retry if self.switch_envelope.is_some() && self.status.frozen => {
                HostCommand::Switch {
                    envelope: Box::new(
                        self.switch_envelope
                            .clone()
                            .ok_or(BundleError::HistoryChanged)?,
                    ),
                }
            }
            Action::Retry => HostCommand::Retry,
            Action::PreviewRecovery(target) => HostCommand::PreviewRecovery {
                target: target.into(),
                snapshot: snapshot()?,
            },
            Action::OpenRecovery if self.switch_envelope.is_some() => HostCommand::Switch {
                envelope: Box::new(
                    self.switch_envelope
                        .clone()
                        .ok_or(BundleError::HistoryChanged)?,
                ),
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
        let mut reply = client.request(command).map_err(host_error)?;
        if reply.activation.is_some() {
            self.switch_envelope = None;
            self.prepared_switch = None;
        }
        if let Some(envelope) = &reply.switch_envelope {
            self.switch_envelope = Some(envelope.clone());
        }
        if let Some(payload) = reply.switch_preview.take() {
            let staged = owner.prepare_gui_switch(payload).and_then(|actor| {
                let view = super::PreparedView::prepare(actor.core())?;
                Ok((actor, view))
            });
            match staged {
                Ok(staged) => self.prepared_switch = Some(staged),
                Err(error) => {
                    self.prepared_switch = None;
                    reply = client.request(HostCommand::Abandon).map_err(host_error)?;
                    reply.error = Some(kinewright_agent::writer_host::HostRefusal::new(
                        "switch_prepare",
                        &error.to_string(),
                    ));
                }
            }
        }
        Ok(reply)
    }

    pub fn pending_count(&self) -> usize {
        self.manual
            .as_ref()
            .and_then(|gate| gate.permit.pending().ok())
            .map_or(0, |messages| messages.len())
    }
    pub fn read_only(&self) -> bool {
        self.manual.is_none()
            || self.cut.is_some()
            || self.status.frozen
            || self.status.choice.is_some()
            || self.pending_count() >= kinewright_core::MANUAL_COMMANDS
    }

    pub fn manual_save_required(&self) -> bool {
        self.manual.is_none() && !self.status.frozen && self.status.choice.is_none()
    }

    pub fn accept_status(&mut self, status: HostStatus) {
        self.client = self.client.with_roots(status.admitted_roots.clone());
        self.status = status;
    }

    /// The frame thread polls only completed work or the child process handle.
    pub fn poll(
        &mut self,
        owner: &mut Ja1EditingSession,
    ) -> Result<Option<HostReply>, BundleError> {
        if self.recovery.is_some() {
            return self.poll_manual_recovery(owner);
        }
        if let Some(child) = &mut self.child
            && child.try_wait().map_err(BundleError::Io)?.is_some()
        {
            return Err(BundleError::Identity(RETENTION_LOST_TEXT.into()));
        }
        if self
            .manual_inflight
            .as_ref()
            .is_some_and(JoinHandle::is_finished)
        {
            return self
                .manual_inflight
                .take()
                .expect("finished manual stream")
                .join()
                .map_err(|_| BundleError::HistoryChanged)?
                .map(Some)
                .map_err(host_error);
        }
        if self.manual_inflight.is_none()
            && self.cut.is_none()
            && let Some(gate) = &self.manual
        {
            let messages = gate
                .permit
                .pending()
                .map_err(|e| BundleError::Identity(e.to_string()))?;
            if !messages.is_empty() {
                // Discard the stale status lane; only stream publication replies can mark Saved.
                self.heartbeat = None;
                let client = self.client.clone();
                let identity = self.manual_client.clone();
                self.manual_inflight = Some(std::thread::spawn(move || {
                    client.request(HostCommand::CommitHistory {
                        client: identity,
                        messages,
                    })
                }));
            }
        }
        if self.manual_inflight.is_some() {
            return Ok(None);
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

    pub fn recovery_running(&self) -> bool {
        self.recovery.is_some()
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
        let pending = self.pending_count();
        if pending > 0 {
            ui.label(format!("{pending} manual edits pending durable save. Preview is local; these edits are not Saved yet."));
        }
        if self.manual_save_required() {
            ui.label(super::MANUAL_SAVE_REQUIRED_TEXT);
            if self.status.association.is_some() {
                if ui.button("Save").clicked() {
                    action = Some(Action::Save);
                }
            } else if ui.button("Save As…").clicked()
                && let Some(path) = rfd::FileDialog::new()
                    .add_filter("Kinewright project", &["kinewright"])
                    .save_file()
            {
                action = Some(Action::FirstSave(path));
            }
        }
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
    #[cfg(test)]
    if let Some(path) = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent()?.parent().map(|parent| parent.join(name)))
        && path.is_file()
    {
        return path;
    }
    std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|parent| parent.join(name)))
        .filter(|path| path.is_file())
        .unwrap_or_else(|| PathBuf::from(name))
}
