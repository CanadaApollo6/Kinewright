//! Recovery IO stays off the frame thread; the GUI retains its actual source owner.
use super::{HostAttachment, host_error};
use kinewright_agent::writer_host::{
    HostClient, HostCommand, HostConfig, HostOrigin, HostReply, HostSnapshot, HostSourceBinding,
    SourceServer,
};
use kinewright_project::{
    bundle::{BundleError, LoadedBundle},
    editing_session::{Ja1EditingSession, ManualRecoveryEndpoint},
};
use std::process::Child;

#[derive(Clone)]
pub(super) enum RecoveryPlan {
    Publish(kinewright_agent::writer_host::RecoveryBarrier),
    Confirm(kinewright_agent::writer_host::RecoveryBarrier),
}

pub(super) struct RecoveredManual {
    issuer: Option<kinewright_agent::writer_host::HostIssuer>,
    client: HostClient,
    child: Option<Child>,
    completion: Option<Result<CompletedRecovery, RecoveryFailure>>,
}
enum RecoveryFailure {
    ReopenRequired,
    Other(BundleError),
}
impl From<BundleError> for RecoveryFailure {
    fn from(error: BundleError) -> Self {
        Self::Other(error)
    }
}
#[allow(
    clippy::needless_pass_by_value,
    reason = "map_err consumes the owned transport error at this boundary"
)]
fn barrier_error(error: kinewright_agent::writer_host::HostError) -> RecoveryFailure {
    if matches!(
        &error,
        kinewright_agent::writer_host::HostError::Refused(refusal)
            if refusal.code == "snapshot_capacity"
    ) {
        RecoveryFailure::ReopenRequired
    } else {
        RecoveryFailure::Other(host_error(error))
    }
}
struct CompletedRecovery {
    permit: kinewright_project::operation_actor::ManualPermit,
    reply: HostReply,
    digest: String,
}
impl Drop for RecoveredManual {
    fn drop(&mut self) {
        if let Some(child) = &mut self.child {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
struct RecoveryHost {
    issuer: Option<kinewright_agent::writer_host::HostIssuer>,
    client: HostClient,
    child: Option<Child>,
}
impl Drop for RecoveryHost {
    fn drop(&mut self) {
        if let Some(child) = &mut self.child {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
fn checked(reply: HostReply) -> Result<HostReply, BundleError> {
    if let Some(error) = &reply.error {
        return Err(BundleError::Identity(error.to_string()));
    }
    if !reply.status.retention_proven {
        return Err(BundleError::Identity(super::RETENTION_LOST_TEXT.into()));
    }
    Ok(reply)
}
fn barrier_checked(reply: HostReply) -> Result<HostReply, RecoveryFailure> {
    if reply
        .error
        .as_ref()
        .is_some_and(|error| error.code == "snapshot_capacity")
    {
        return Err(RecoveryFailure::ReopenRequired);
    }
    Ok(checked(reply)?)
}
#[allow(
    clippy::too_many_arguments,
    reason = "one bounded recovery transaction owns its actor, original evidence and cancellation"
)]
fn run_barrier(
    host: &mut RecoveryHost,
    mut endpoint: ManualRecoveryEndpoint,
    client: String,
    frozen: HostSnapshot,
    pending: Vec<kinewright_project::operation_protocol::EncodedMessage>,
    live: std::sync::Arc<std::sync::atomic::AtomicBool>,
    saved_plan: &std::sync::Mutex<Option<RecoveryPlan>>,
) -> Result<
    (
        kinewright_project::operation_actor::ManualPermit,
        HostReply,
        String,
    ),
    RecoveryFailure,
> {
    let retained = saved_plan
        .lock()
        .map_err(|_| BundleError::Unsettled)?
        .clone();
    let mut plan = if let Some(retained) = retained {
        // A retry uses the already authenticated barrier. Its newly captured
        // source is not sent and must not occupy a fourth lease during install.
        drop(frozen);
        drop(pending);
        retained
    } else {
        let reply = checked(
            host.client
                .request(HostCommand::BeginRecovery {
                    client,
                    frozen: Some(frozen),
                    pending,
                })
                .map_err(host_error)?,
        )?;
        RecoveryPlan::Publish(reply.recovery.ok_or(BundleError::HistoryChanged)?)
    };
    loop {
        *saved_plan.lock().map_err(|_| BundleError::Unsettled)? = Some(plan.clone());
        let barrier = match &plan {
            RecoveryPlan::Publish(barrier) | RecoveryPlan::Confirm(barrier) => barrier.clone(),
        };
        if matches!(plan, RecoveryPlan::Publish(_)) {
            let published = barrier_checked(
                host.client
                    .request(HostCommand::PublishBarrier {
                        barrier: Box::new(barrier.clone()),
                    })
                    .map_err(barrier_error)?,
            )?;
            if published.history.as_ref() != Some(&barrier.after.history) {
                return Err(BundleError::HistoryChanged.into());
            }
            endpoint.install(barrier.after.history.clone(), published.manual_ack)?;
            *saved_plan.lock().map_err(|_| BundleError::Unsettled)? =
                Some(RecoveryPlan::Confirm(barrier.clone()));
        }
        let installed = HostSnapshot::from_history(endpoint.snapshot()?).map_err(host_error)?;
        let reply = barrier_checked(
            host.client
                .request(HostCommand::ConfirmBarrier {
                    barrier: Box::new(barrier.clone()),
                    installed,
                })
                .map_err(barrier_error)?,
        )?;
        if let Some(next) = reply.recovery.clone() {
            plan = RecoveryPlan::Publish(next);
            continue;
        }
        let digest = barrier.after.history_sha256;
        let permit = endpoint.finish(
            reply
                .manual_lease
                .clone()
                .ok_or(BundleError::HistoryChanged)?,
            reply
                .manual_inputs
                .clone()
                .ok_or(BundleError::HistoryChanged)?,
            live,
        )?;
        return Ok((permit, reply, digest));
    }
}
impl HostAttachment {
    #[allow(
        clippy::too_many_lines,
        reason = "one GUI-owned freeze, source proof and background process handoff"
    )]
    pub fn begin_manual_recovery(
        &mut self,
        owner: &mut Ja1EditingSession,
    ) -> Result<(), BundleError> {
        if self.recovery.is_some() {
            return Ok(());
        }
        let confirm_pending = self
            .recovery_plan
            .lock()
            .map_err(|_| BundleError::Unsettled)?
            .as_ref()
            .is_some_and(|plan| matches!(plan, RecoveryPlan::Confirm(_)));
        // Confirm belongs to the retained owner; initial recovery still uses C10.
        if confirm_pending && self.confirmed_lost() {
            return Err(BundleError::Identity(super::RETENTION_LOST_TEXT.into()));
        }
        // This barrier precedes joining the submitted stream worker and every child operation.
        let cut = owner.recapture_host_recovery_cut()?;
        self.cut = Some(cut);
        self.status.frozen = true;
        self.edits.abandon_unaccepted();
        let gate = self.manual.as_ref().ok_or(BundleError::HistoryChanged)?;
        let retained_plan = self
            .recovery_plan
            .lock()
            .map_err(|_| BundleError::Unsettled)?
            .is_some();
        let actor_pending = match gate.permit.pending() {
            Ok(pending) => pending,
            Err(_) if retained_plan => vec![],
            Err(error) => return Err(BundleError::Identity(error.to_string())),
        };
        if !retained_plan {
            self.recovery_originals.clone_from(&actor_pending);
        }
        let pending = self.recovery_originals.clone();
        let endpoint = owner.manual_recovery_endpoint(&cut, actor_pending)?;
        // C10 now owns the exact frozen actor and original outbox. A lost Save's
        // request capture duplicates that evidence and must not keep another
        // full lease through installing and confirming the selected barrier.
        // Selection and destination requests use their own recovery protocol.
        if self
            .pending_operation
            .as_ref()
            .is_some_and(|(_, command)| !matches!(command, HostCommand::Save { .. }))
        {
            return Err(BundleError::HistoryChanged);
        }
        self.pending_operation = None;
        let saved_plan = std::sync::Arc::clone(&self.recovery_plan);
        let live = self.context.upgrade().ok_or(BundleError::HistoryChanged)?;
        let frozen = HostSnapshot::capture(owner.core()).map_err(host_error)?;
        // Borrow the same actual GUI admission. A replacement callback never owns
        // source cleanup, pins or a new accounting ledger.
        if !self.status.destinations.is_empty() || self.status.prepared_selection {
            return Err(BundleError::Identity(super::RETENTION_LOST_TEXT.into()));
        }
        owner.ensure_host_snapshot(&cut)?;
        let retained_source = self
            .source
            .as_ref()
            .map(|source| {
                owner
                    .source()
                    .ok_or(BundleError::HistoryChanged)?
                    .borrowed_source_service(source)
            })
            .transpose()?;
        let source_descriptor = self.source.clone();
        let source_inputs = self.source.as_ref().map(|_| self.inputs.clone());
        let mut prior_child = self.child.take();
        let prior_issuer = self.issuer.clone();
        let prior_client = self.client.clone();
        let inflight = self.manual_inflight.take();
        self.heartbeat = None;
        let identity = self.manual_client.clone();
        let path = self
            .status
            .association
            .clone()
            .ok_or(BundleError::HistoryChanged)?;
        let roots = self.status.admitted_roots.clone();
        let project = self.client.endpoint().project.clone();
        let executable = self.executable.clone();
        let restart = !executable.as_os_str().is_empty()
            && prior_child
                .as_mut()
                .is_some_and(|child| child.try_wait().is_ok_and(|status| status.is_some()));
        if restart && confirm_pending {
            self.child = prior_child;
            self.manual_inflight = inflight;
            return Err(BundleError::Identity(super::RETENTION_LOST_TEXT.into()));
        }
        let callback = if restart {
            let server = if let Some(reader) = &retained_source {
                SourceServer::start(reader.clone()).map_err(host_error)?
            } else {
                let context = self.context.clone();
                SourceServer::start_context(move || {
                    if context
                        .upgrade()
                        .is_some_and(|live| live.load(std::sync::atomic::Ordering::Acquire))
                    {
                        Ok(())
                    } else {
                        Err(BundleError::Identity(
                            "originating context is closed".into(),
                        ))
                    }
                })
                .map_err(host_error)?
            };
            let callback = server.bootstrap();
            self.source_server = Some(server);
            Some(callback)
        } else {
            None
        };
        let pool = owner.core().history_pool().clone();
        self.recovery = Some(std::thread::spawn(move || {
            // Joining does not consume the original queue: the successor resolves lost replies.
            if let Some(worker) = inflight {
                let _ = worker.join();
            }
            let mut host = if restart {
                if let Some(mut child) = prior_child.take() {
                    let _ = child.kill();
                    let _ = child.wait();
                }
                let mut loaded = LoadedBundle::read(&path, &pool)?;
                let root = loaded.root()?.clone();
                let checkpoint = loaded.manual_checkpoint()?.clone();
                let validated = loaded.take_manual_snapshot()?;
                let snapshot = HostSnapshot::from_history(
                    kinewright_project::operation_actor::PortableHistory {
                        snapshot: std::sync::Arc::new(
                            validated.into_reader()?.into_capture()?.into_transport()?,
                        ),
                        branch: checkpoint.branch_id,
                        revision: checkpoint.revision,
                    },
                )
                .map_err(host_error)?;
                if let Some(reader) = &retained_source {
                    reader.ensure_snapshot(
                        source_descriptor
                            .as_ref()
                            .ok_or(BundleError::HistoryChanged)?,
                        &snapshot.history,
                    )?;
                }
                let config = HostConfig {
                    #[cfg(test)]
                    data_dir: Some(path.with_extension("test-voice-data")),
                    #[cfg(not(test))]
                    data_dir: Some(super::person_data_dir()?),
                    #[cfg(test)]
                    scheduler_settings: Some(path.with_extension("test-scheduler")),
                    #[cfg(not(test))]
                    scheduler_settings: None,
                    // JA-1 S6 C4: the app notifies on every seal; its tests never do.
                    desktop_notifications: cfg!(not(test)),
                    project,
                    path: Some(path),
                    roots,
                    origin: HostOrigin::Gui {
                        retention: HostSourceBinding {
                            source: source_descriptor,
                            inputs: source_inputs,
                            callback: callback.ok_or(BundleError::HistoryChanged)?,
                        },
                        initial_snapshot: Some(snapshot),
                        expected_root_sha256: Some(kinewright_media::sha256_bytes(
                            loaded.admission().raw_bytes(),
                        )),
                        expected_project_id: Some(root.project_id.clone()),
                    },
                };
                let (client, child, issuer) =
                    HostClient::launch_with_issuer(&executable, config).map_err(host_error)?;
                RecoveryHost {
                    issuer: Some(issuer),
                    client: client.with_manual_editor(identity.clone()),
                    child: Some(child),
                }
            } else {
                RecoveryHost {
                    issuer: prior_issuer,
                    client: prior_client,
                    child: prior_child.take(),
                }
            };
            let completion = run_barrier(
                &mut host,
                endpoint,
                identity,
                frozen,
                pending,
                live,
                &saved_plan,
            )
            .map(|(permit, reply, digest)| CompletedRecovery {
                permit,
                reply,
                digest,
            });
            Ok(RecoveredManual {
                issuer: host.issuer.clone(),
                client: host.client.clone(),
                child: host.child.take(),
                completion: Some(completion),
            })
        }));
        Ok(())
    }
    pub(super) fn poll_manual_recovery(
        &mut self,
        owner: &mut Ja1EditingSession,
    ) -> Result<Option<HostReply>, BundleError> {
        if !self
            .recovery
            .as_ref()
            .is_some_and(std::thread::JoinHandle::is_finished)
        {
            return Ok(None);
        }
        let result = self
            .recovery
            .take()
            .ok_or(BundleError::HistoryChanged)?
            .join()
            .map_err(|_| BundleError::HistoryChanged)?;
        let mut recovered = match result {
            Ok(recovered) => recovered,
            Err(error) => {
                return Ok(Some(self.paused_recovery_reply(&error)));
            }
        };
        // The same owned process returns even when a barrier reply was lost.
        // Keeping it here lets Retry reconcile without killing retained authority.
        self.client = recovered.client.clone();
        self.issuer.clone_from(&recovered.issuer);
        self.child = recovered.child.take();
        let completed = match recovered
            .completion
            .take()
            .ok_or(BundleError::HistoryChanged)?
        {
            Ok(completed) => completed,
            Err(RecoveryFailure::ReopenRequired) => {
                return Ok(Some(self.reopen_required_recovery_reply()));
            }
            Err(RecoveryFailure::Other(error)) => {
                return Ok(Some(self.paused_recovery_reply(&error)));
            }
        };
        owner.finish_host_manual_recovery(&completed.digest)?;
        self.manual = Some(super::super::ManualProducerGate {
            actor: owner.core().actor_identity(),
            permit: completed.permit,
        });
        self.cut = None;
        self.recovery_pause = super::RecoveryPause::None;
        self.unknown_operation = false;
        self.pending_operation = None;
        self.recovery_originals.clear();
        *self
            .recovery_plan
            .lock()
            .map_err(|_| BundleError::Unsettled)? = None;
        self.accept_status(completed.reply.status.clone());
        Ok(Some(completed.reply))
    }

    fn reopen_required_recovery_reply(&mut self) -> HostReply {
        self.recovery_pause = super::RecoveryPause::ReopenRequired;
        self.status.frozen = true;
        self.status.message = super::RECOVERY_REOPEN_TEXT.into();
        HostReply {
            status: self.status.clone(),
            error: Some(kinewright_agent::writer_host::HostRefusal::new(
                "recovery_paused",
                super::RECOVERY_REOPEN_TEXT,
            )),
            ..HostReply::default()
        }
    }

    fn paused_recovery_reply(&mut self, error: &BundleError) -> HostReply {
        self.status.frozen = true;
        self.status.message = format!("Recovery remains paused: {error}");
        HostReply {
            status: self.status.clone(),
            error: Some(kinewright_agent::writer_host::HostRefusal::new(
                "recovery_paused",
                &error.to_string(),
            )),
            ..HostReply::default()
        }
    }
}
