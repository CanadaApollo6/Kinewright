use std::{
    collections::BTreeSet,
    io::{self, Seek},
    sync::{Arc, Mutex, MutexGuard, Weak},
    thread::{self, JoinHandle},
};

use kinewright_core::{ExportCancellation, ExportSettings, MediaError};
use kinewright_media::{OwnedExport, OwnedExportCleanup, OwnedExportError, OwnedExportTarget};
use kinewright_project::{
    bundle::BundleError,
    bundle_records::{ObjectRef, RecordKind, SessionId},
    review_job::{
        ReviewControl, ReviewJob, ReviewPhase, ReviewRequest, ReviewStage, ReviewStaging,
        ReviewStatus, ReviewToken, UpdateReviewJob, empty_annotations,
    },
    storage_budget::StorageReservation,
    studio_records::SnapshotKey,
    studio_store::FrozenContextRequest,
};

use super::{
    AttachmentRole, HostCommand, HostCommandOwner, HostError, HostRefusal, HostReply, HostRequest,
    review_expectations::{ReviewExpectationInput, derive_review_expectations},
};

static LIVE_STAGES: Mutex<BTreeSet<SessionId>> = Mutex::new(BTreeSet::new());

struct LiveStage(SessionId);

impl LiveStage {
    fn register(id: SessionId) -> Result<Self, HostError> {
        if !LIVE_STAGES
            .lock()
            .map_err(|_| unavailable())?
            .insert(id.clone())
        {
            return Err(unavailable());
        }
        Ok(Self(id))
    }
}

impl Drop for LiveStage {
    fn drop(&mut self) {
        if let Ok(mut stages) = LIVE_STAGES.lock() {
            stages.remove(&self.0);
        }
    }
}

#[derive(Default)]
pub(super) struct ReviewRuntime {
    worker: Option<Worker>,
    pending_stop: Option<ReviewStatus>,
    verified: Option<ReviewToken>,
    #[cfg(test)]
    probe: Option<Arc<dyn Fn(ReviewPoint) + Send + Sync>>,
    #[cfg(test)]
    exhaust_artifact: bool,
}

#[cfg(test)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ReviewPoint {
    BeforeEncode,
    BeforeInstall,
    AfterInstall,
    BeforePublication,
    AfterPublication,
}

struct EncodeWork {
    context: FrozenContextRequest,
    staging: Arc<ReviewStaging>,
    charge: Arc<StorageReservation>,
    working: Arc<kinewright_media::owned_io::CursorReservation>,
    settings: ExportSettings,
    byte_limit: u64,
    data_dir: Option<std::path::PathBuf>,
    #[cfg(test)]
    probe: Option<Arc<dyn Fn(ReviewPoint) + Send + Sync>>,
    #[cfg(test)]
    exhaust_artifact: bool,
}

struct Worker {
    token: ReviewToken,
    stage: ReviewStage,
    cancellation: ExportCancellation,
    thread: Option<JoinHandle<Option<OwnedExportCleanup>>>,
    cleanup: Option<OwnedExportCleanup>,
    // Keep the directory descriptor alive through the receipt's last cleanup retry.
    _staging: Arc<ReviewStaging>,
    charge: Option<Arc<StorageReservation>>,
}

impl Drop for ReviewRuntime {
    fn drop(&mut self) {
        self.cancel_for_shutdown();
    }
}

impl ReviewRuntime {
    pub(super) fn cancel_for_shutdown(&self) {
        if let Some(worker) = &self.worker {
            worker.cancellation.cancel();
        }
    }
}

fn unavailable() -> HostError {
    HostRefusal::new("review_unavailable", "Output review owner is unavailable.").into()
}

fn refused(message: &str) -> HostError {
    HostRefusal::new("review_refused", message).into()
}

fn lock(host: &Mutex<HostCommandOwner>) -> Result<MutexGuard<'_, HostCommandOwner>, HostError> {
    host.lock().map_err(|_| unavailable())
}

pub(super) fn execute(
    host: &Arc<Mutex<HostCommandOwner>>,
    bearer: &str,
    request: &HostRequest,
    control: &ReviewControl,
) -> HostReply {
    let result = (|| {
        match control {
            ReviewControl::Queue {
                snapshot,
                settings,
                byte_limit,
            } => {
                return queue(
                    host,
                    bearer,
                    request,
                    snapshot,
                    Some((settings.clone(), *byte_limit)),
                );
            }
            ReviewControl::QueueDefault(snapshot) => {
                return queue(host, bearer, request, snapshot, None);
            }
            _ => {}
        }
        let mut owner = lock(host)?;
        owner.authorize_review(bearer, request)?;
        owner.apply_review_control(control)
    })();
    let mut reply = HostReply::default();
    if let Ok(owner) = host.lock() {
        reply.status = owner.status();
    }
    match result {
        Ok(status) => reply.review = status,
        Err(error) => reply.error = Some(error.refusal()),
    }
    reply
}

fn queue(
    host: &Mutex<HostCommandOwner>,
    bearer: &str,
    request: &HostRequest,
    snapshot: &SnapshotKey,
    delivery: Option<(ExportSettings, u64)>,
) -> Result<Option<ReviewStatus>, HostError> {
    let (project, frozen, rulebook) = {
        let mut owner = lock(host)?;
        owner.authorize_review(bearer, request)?;
        owner.reconcile_review_job()?;
        let bundle = owner.owner()?.review_bundle()?;
        if bundle.review_job()?.is_some() {
            return Err(refused("An output review is already active."));
        }
        (
            bundle.project_id().clone(),
            bundle.read_snapshot(snapshot)?,
            owner.owner()?.approved_rulebook_ref()?.cloned(),
        )
    };
    let inputs = frozen.open_review_inputs()?;
    let (settings, byte_limit) = delivery.unwrap_or_else(|| {
        let settings = kinewright_core::DeliveryProfile::SourceMaster.export_settings(
            &inputs.document,
            kinewright_core::DeliveryEncodeDepth::Eight,
            ExportCancellation::default(),
        );
        let fps = inputs.document.fps;
        let seconds = inputs
            .document
            .duration
            .0
            .unsigned_abs()
            .saturating_mul(u64::from(fps.denominator()))
            .div_ceil(u64::from(fps.numerator()));
        let bytes = seconds
            .saturating_mul(settings.video_bitrate + settings.audio_bitrate)
            .div_ceil(4)
            .saturating_add(16 << 20)
            .min(8 << 30);
        (settings, bytes)
    });
    let empty;
    let annotations = if let Some(annotations) = &inputs.annotations {
        &annotations.0
    } else {
        empty = empty_annotations(&frozen.descriptor);
        &empty
    };
    let expectations = derive_review_expectations(
        &ReviewExpectationInput {
            document: &inputs.document,
            snapshot: frozen.descriptor.identity,
            input_manifest: &inputs.manifest,
            settings: &settings,
            transcripts: &[],
        },
        annotations,
    )
    .map_err(|error| refused(&error.to_string()))?;
    drop(inputs);
    let mut owner = lock(host)?;
    owner.authorize_review(bearer, request)?;
    if owner.owner()?.project_id()? != project {
        return Err(refused("The project changed during review preparation."));
    }
    owner.owner_mut()?.queue_review(
        &frozen.descriptor,
        rulebook,
        settings,
        byte_limit,
        expectations,
    )?;
    owner.review_status()
}

impl HostCommandOwner {
    #[cfg(test)]
    pub(super) fn review_fixture_probe(
        &mut self,
        probe: impl Fn(ReviewPoint) + Send + Sync + 'static,
    ) {
        self.review_runtime.probe = Some(Arc::new(probe));
    }

    #[cfg(test)]
    pub(super) fn review_fixture_exhaust_artifact(&mut self, exhaust: bool) {
        self.review_runtime.exhaust_artifact = exhaust;
    }

    #[cfg(test)]
    pub(super) fn review_fixture_worker(&self) -> Option<(bool, bool)> {
        self.review_runtime.worker.as_ref().map(|worker| {
            (
                worker.cancellation.is_cancelled(),
                worker.thread.as_ref().is_none_or(JoinHandle::is_finished),
            )
        })
    }

    #[cfg(test)]
    pub(super) fn review_fixture_fail_stop(
        &mut self,
        token: &ReviewToken,
        fault: &'static str,
    ) -> ReviewStatus {
        kinewright_project::io_observer::fail_once(fault, || self.stop_review_job(token))
            .unwrap()
            .unwrap()
    }

    pub(super) fn join_review_for_shutdown(host: &Mutex<Self>) {
        let thread = host
            .lock()
            .ok()
            .and_then(|mut owner| owner.review_runtime.worker.as_mut()?.thread.take());
        if let Some(thread) = thread {
            let cleanup = thread.join().unwrap_or(None);
            if let Ok(mut owner) = host.lock()
                && let Some(worker) = &mut owner.review_runtime.worker
            {
                worker.cleanup = cleanup;
            }
        }
    }

    fn authorize_review(&mut self, bearer: &str, request: &HostRequest) -> Result<(), HostError> {
        let (index, _, roots) = self.authorize_request(Some(bearer), request)?;
        self.require_open_command(&request.command)?;
        let attachment = &self.attachments[index];
        if attachment.role != AttachmentRole::Editor || !attachment.person_facing {
            return Err(refused(
                "The person-facing Editor is required for output review.",
            ));
        }
        self.check_named_access(&roots, true)?;
        self.prove_retention()?;
        if self.recovery_only
            || (self.hooks.pending.is_some()
                && !matches!(
                    request.command,
                    HostCommand::Review(ReviewControl::Stop(_) | ReviewControl::Status)
                ))
        {
            return Err(BundleError::Unsettled.into());
        }
        Ok(())
    }

    fn apply_review_control(
        &mut self,
        control: &ReviewControl,
    ) -> Result<Option<ReviewStatus>, HostError> {
        if let ReviewControl::Stop(token) = control {
            return self.stop_review_job(token);
        }
        if let Err(error) = self.reconcile_review_job() {
            if matches!(control, ReviewControl::Status)
                && self.review_runtime.pending_stop.is_some()
            {
                return Ok(self.review_runtime.pending_stop.clone());
            }
            return Err(error);
        }
        match control {
            ReviewControl::Status => {}
            ReviewControl::Annotate { snapshot, fields } => {
                let pinned = self.owner()?.read_snapshot(snapshot)?;
                let annotations = kinewright_core::output_review::ReviewAnnotations {
                    snapshot: empty_annotations(&pinned.descriptor).snapshot,
                    sourced_elements: fields.sourced_elements.clone(),
                    text_cards: fields.text_cards.clone(),
                    intended_pauses: fields.intended_pauses.clone(),
                };
                drop(pinned);
                self.owner_mut()?
                    .set_review_annotations(snapshot, annotations)?;
            }
            ReviewControl::Resume(token) => self.resume_review(token)?,
            ReviewControl::Queue { .. }
            | ReviewControl::QueueDefault(_)
            | ReviewControl::Stop(_) => return Err(unavailable()),
        }
        self.review_status()
    }

    fn review_status(&self) -> Result<Option<ReviewStatus>, HostError> {
        Ok(self.owner()?.review_bundle()?.review_status()?)
    }

    fn stop_review_job(&mut self, token: &ReviewToken) -> Result<Option<ReviewStatus>, HostError> {
        if let Some(pending) = &self.review_runtime.pending_stop {
            if &pending.token != token {
                return Err(refused("A different output review is still stopping."));
            }
        } else {
            let Some(mut status) = self
                .review_status()?
                .filter(|status| &status.token == token)
            else {
                return Err(refused("The output review generation or version changed."));
            };
            if status.phase == ReviewPhase::Stopped {
                return Ok(Some(status));
            }
            status.stop_pending = true;
            self.review_runtime.pending_stop = Some(status);
        }
        if self.persist_review_stop().is_err() {
            return Ok(self.review_runtime.pending_stop.clone());
        }
        // The durable Stopping phase already fences every writer. A cleanup error
        // leaves it in that phase for the next retry or reopen.
        let _ = self.reconcile_review_job();
        self.review_status()
    }

    fn persist_review_stop(&mut self) -> Result<(), HostError> {
        let Some(token) = self
            .review_runtime
            .pending_stop
            .as_ref()
            .map(|status| status.token.clone())
        else {
            return Ok(());
        };
        if self.hooks.pending.is_some() {
            return Err(BundleError::Unsettled.into());
        }
        let bundle = self.owner_mut()?.review_source_bundle_mut(&token.project)?;
        bundle.update_review_job(&token, UpdateReviewJob::Stop)?;
        self.review_runtime.pending_stop = None;
        if let Some(worker) = &self.review_runtime.worker
            && worker.token == token
        {
            worker.cancellation.cancel();
        }
        Ok(())
    }

    pub(super) fn reconcile_review_job(&mut self) -> Result<(), HostError> {
        if self.hooks.pending.is_some() {
            return Err(BundleError::Unsettled.into());
        }
        self.persist_review_stop()?;
        self.retire_review_worker()?;
        if self
            .owner()?
            .review_bundle()?
            .root()
            .is_some_and(|root| root.active_review_ref.is_some())
        {
            let session = self.owner_mut()?;
            let project = session.project_id()?;
            session
                .review_source_bundle_mut(&project)?
                .settle_review_root()?;
        }
        let bundle = self.owner()?.review_bundle()?;
        let Some(job) = bundle.review_job()? else {
            return Ok(());
        };
        let token = job.token.clone();
        if self
            .review_runtime
            .worker
            .as_ref()
            .is_some_and(|worker| worker.token == token)
        {
            return Ok(());
        }
        if let Some(stage) = &job.stage {
            prove_retired(stage)?;
            bundle.store().sweep_review_staging(stage)?;
        }
        if job.artifact_ref.is_some()
            && job.phase != ReviewPhase::Stopping
            && self.review_runtime.verified.as_ref() != Some(&token)
        {
            drop(bundle.open_review_artifact(&token)?);
            let needs_fresh_attempt = matches!(&job.phase, ReviewPhase::Checking(checks) if checks.reviewer_attempt.is_some());
            if needs_fresh_attempt {
                self.owner_mut()?
                    .update_review_job(&token, UpdateReviewJob::UnbindChecks)?;
                self.review_runtime.verified = Some(token);
                return Ok(());
            }
            self.review_runtime.verified = Some(token.clone());
        }
        let update = match job.phase {
            ReviewPhase::Encoding => UpdateReviewJob::RecoverEncoding,
            ReviewPhase::Stopping => UpdateReviewJob::RetireStopped,
            _ if job.stage.is_some() => UpdateReviewJob::RetireStage,
            _ => return Ok(()),
        };
        drop(job);
        self.owner_mut()?.update_review_job(&token, update)?;
        Ok(())
    }

    fn retire_review_worker(&mut self) -> Result<(), HostError> {
        let Some(worker) = &mut self.review_runtime.worker else {
            return Ok(());
        };
        if worker
            .thread
            .as_ref()
            .is_some_and(|thread| !thread.is_finished())
        {
            return Ok(());
        }
        if let Some(thread) = worker.thread.take() {
            worker.cleanup = thread.join().unwrap_or(None);
        }
        let token = worker.token.clone();
        let stage = worker.stage.clone();
        let bundle = self.owner_mut()?.review_source_bundle_mut(&token.project)?;
        bundle.settle_review_root()?;
        bundle.store().sweep_review_staging(&stage)?;
        if let Some(cleanup) = &mut self
            .review_runtime
            .worker
            .as_mut()
            .expect("worker retained")
            .cleanup
        {
            cleanup.retry()?;
        }
        self.review_runtime
            .worker
            .as_mut()
            .expect("worker retained")
            .charge
            .take();
        let bundle = self.owner_mut()?.review_source_bundle_mut(&token.project)?;
        if let Some(job) = bundle.review_job()?
            && job.token == token
        {
            let update = match job.phase {
                ReviewPhase::Encoding => UpdateReviewJob::Pause(
                    "Encoding ended before a durable artifact was published.".into(),
                ),
                ReviewPhase::Stopping => UpdateReviewJob::RetireStopped,
                _ => UpdateReviewJob::RetireStage,
            };
            bundle.update_review_job(&token, update)?;
            if matches!(job.phase, ReviewPhase::Encoding) {
                bundle.update_review_job(&token, UpdateReviewJob::RetireStage)?;
            }
        }
        self.review_runtime.worker = None;
        Ok(())
    }

    fn resume_review(&mut self, token: &ReviewToken) -> Result<(), HostError> {
        if self.review_runtime.worker.is_some() {
            return Err(refused("An encoding worker is still retiring."));
        }
        let bundle = self.owner()?.review_bundle()?;
        let job = bundle
            .review_job()?
            .filter(|job| &job.token == token)
            .ok_or_else(|| refused("The output review generation or version changed."))?;
        let phase = if matches!(job.phase, ReviewPhase::Paused { .. }) {
            self.owner_mut()?
                .update_review_job(token, UpdateReviewJob::Retry)?
                .phase
        } else {
            job.phase.clone()
        };
        match phase {
            ReviewPhase::Checking(_) | ReviewPhase::ReadyToSeal { .. } => Ok(()),
            ReviewPhase::Queued => self.begin_review_encoding(token, &job),
            _ => Err(refused("This output review cannot be resumed.")),
        }
    }

    fn begin_review_encoding(
        &mut self,
        token: &ReviewToken,
        job: &ReviewJob,
    ) -> Result<(), HostError> {
        let bundle = self.owner()?.review_bundle()?;
        let request = bundle
            .store()
            .read_record::<ReviewRequest>(&job.request_ref)?;
        let snapshot = bundle.read_snapshot(&token.snapshot)?;
        let context = bundle.prepare_frozen_context_descriptor(&snapshot.descriptor)?;
        let stage = current_stage(request.byte_limit)?;
        let settings = request.settings.clone();
        drop(request);
        drop(snapshot);
        let status = self
            .owner_mut()?
            .update_review_job(token, UpdateReviewJob::BeginEncoding(stage.clone()))?;
        let result = self.spawn_review_worker(status.token.clone(), stage, settings, context);
        if let Err(error) = result {
            self.owner_mut()?
                .update_review_job(&status.token, UpdateReviewJob::Pause(error.to_string()))?;
        }
        Ok(())
    }

    fn spawn_review_worker(
        &mut self,
        token: ReviewToken,
        stage: ReviewStage,
        settings: ExportSettings,
        context: FrozenContextRequest,
    ) -> Result<(), HostError> {
        let bundle = self.owner()?.review_bundle()?;
        let charge = Arc::new(bundle.store().reserve_copy(stage.byte_limit)?);
        let working = bundle.store().reserve_review_working();
        let staging = Arc::new(bundle.store().create_review_staging(&stage)?);
        let live = LiveStage::register(stage.id.clone())?;
        let weak = self.runtime_owner.clone();
        let cancellation = settings.cancellation.clone();
        let work = EncodeWork {
            context,
            staging: Arc::clone(&staging),
            charge: Arc::clone(&charge),
            working,
            settings,
            byte_limit: stage.byte_limit,
            data_dir: self.config.data_dir.clone(),
            #[cfg(test)]
            probe: self.review_runtime.probe.clone(),
            #[cfg(test)]
            exhaust_artifact: self.review_runtime.exhaust_artifact,
        };
        let thread = {
            let token = token.clone();
            thread::Builder::new()
                .name("output-review-encode".into())
                .spawn(move || {
                    let _live = live;
                    let (result, cleanup) = encode_review(&weak, &token, work);
                    if let Err(error) = result
                        && let Some(host) = weak.upgrade()
                        && let Ok(mut owner) = host.lock()
                        && owner.review_can_complete(&token).is_ok()
                        && let Ok(session) = owner.owner_mut()
                        && let Ok(bundle) = session.review_source_bundle_mut(&token.project)
                    {
                        let _ = bundle
                            .update_review_job(&token, UpdateReviewJob::Pause(error.to_string()));
                    }
                    cleanup
                })?
        };
        self.review_runtime.worker = Some(Worker {
            token,
            stage,
            cancellation,
            thread: Some(thread),
            cleanup: None,
            _staging: staging,
            charge: Some(charge),
        });
        Ok(())
    }

    fn review_can_complete(&self, token: &ReviewToken) -> Result<(), HostError> {
        if self.lifecycle.stopping()
            || self.recovery_only
            || self.hooks.pending.is_some()
            || !self.retention_proven
            || self
                .review_runtime
                .pending_stop
                .as_ref()
                .is_some_and(|status| &status.token == token)
        {
            return Err(refused("Output review authority was revoked."));
        }
        let bundle = self.owner()?.review_source_bundle(&token.project)?;
        if !bundle
            .review_job()?
            .is_some_and(|job| job.token == *token && job.phase == ReviewPhase::Encoding)
        {
            return Err(refused("The output review generation or version changed."));
        }
        Ok(())
    }
}

fn encode_review(
    owner: &Weak<Mutex<HostCommandOwner>>,
    token: &ReviewToken,
    work: EncodeWork,
) -> (Result<(), HostError>, Option<OwnedExportCleanup>) {
    let mut pending = None;
    let result = (|| {
        #[cfg(test)]
        let probe = |point| {
            if let Some(probe) = &work.probe {
                probe(point);
            }
        };
        #[cfg(test)]
        probe(ReviewPoint::BeforeEncode);
        let inputs = Arc::new(work.context.open()?).playback_inputs()?;
        #[cfg(not(test))]
        let media = super::LazyHostMedia::new(work.data_dir);
        #[cfg(not(test))]
        let engine = media.get().map_err(|error| refused(&error.to_string()))?;
        #[cfg(test)]
        let engine = review_fixture_engine(work.data_dir)?;
        let (progress, receiver) = crossbeam_channel::bounded(0);
        drop(receiver);
        let gate = |install: &mut dyn FnMut() -> Result<(), MediaError>| {
            #[cfg(test)]
            probe(ReviewPoint::BeforeInstall);
            let host = owner.upgrade().ok_or(MediaError::Cancelled)?;
            let owner = host.lock().map_err(|_| MediaError::Cancelled)?;
            owner
                .review_can_complete(token)
                .map_err(|_| MediaError::Cancelled)?;
            install()?;
            #[cfg(test)]
            probe(ReviewPoint::AfterInstall);
            Ok(())
        };
        let charge = work.charge;
        let reserve_staging =
            move |_| Ok(Arc::clone(&charge) as kinewright_media::owned_io::LifetimeGuard);
        let outcome = engine.export_owned_document(
            &inputs,
            OwnedExportTarget {
                destination: &work.staging.destination(),
                staging_bytes: work.byte_limit,
                reserve_staging: &reserve_staging,
                reserve_working: &*work.working,
                commit: &gate,
            },
            &work.settings,
            &progress,
        );
        let receipt = match outcome {
            Ok(OwnedExport::Clean(receipt)) => receipt,
            Ok(OwnedExport::Cleanup { receipt, cleanup }) => {
                pending = Some(cleanup);
                receipt
            }
            Err(OwnedExportError::Cleanup { error, cleanup }) => {
                pending = Some(*cleanup);
                return Err(refused(&error.to_string()));
            }
            Err(OwnedExportError::RepeatedCommit(outcome)) => {
                if let OwnedExport::Cleanup { cleanup, .. } = *outcome {
                    pending = Some(cleanup);
                }
                return Err(refused("The review install gate was invoked twice."));
            }
            Err(error) => return Err(refused(&error.to_string())),
        };
        let mut artifact = work.staging.open_output()?;
        let byte_length = artifact.metadata()?.len();
        let (width, height) = work.settings.resolution;
        let inspection_bytes = u64::from(width)
            .saturating_mul(u64::from(height))
            .saturating_mul(512)
            .saturating_add(2 << 20);
        let inspection = (work.working)(inspection_bytes).map_err(|error| refused(&error))?;
        let digest = kinewright_hash::sha256_reader_observed(
            &mut artifact,
            |_| {
                if work.settings.cancellation.is_cancelled() {
                    return Err(io::Error::other("output review cancelled"));
                }
                Ok(())
            },
            std::convert::identity,
        )?;
        let media_ref = ObjectRef {
            sha256: digest,
            byte_length,
            record_kind: RecordKind::OwnedMedia,
        };
        artifact.rewind()?;
        let output = kinewright_media::inspect_review_output(
            artifact.try_clone()?,
            &work.settings.cancellation,
        )
        .map_err(|error| refused(&error.to_string()))?;
        artifact.rewind()?;
        drop(inspection);
        #[cfg(test)]
        probe(ReviewPoint::BeforePublication);
        {
            let host = owner.upgrade().ok_or_else(unavailable)?;
            let mut owner = lock(&host)?;
            owner.review_can_complete(token)?;
            let bundle = owner
                .owner_mut()?
                .review_source_bundle_mut(&token.project)?;
            let publish =
                || bundle.publish_review_artifact(token, &mut artifact, &media_ref, output);
            #[cfg(test)]
            if work.exhaust_artifact {
                kinewright_project::io_observer::fail_native_once(
                    "object_file_sync",
                    rustix::io::Errno::NOSPC.raw_os_error(),
                    publish,
                )?;
            } else {
                publish()?;
            }
            #[cfg(not(test))]
            publish()?;
        }
        #[cfg(test)]
        probe(ReviewPoint::AfterPublication);
        drop(receipt);
        Ok(())
    })();
    (result, pending)
}

#[cfg(test)]
fn review_fixture_engine(
    data_dir: Option<std::path::PathBuf>,
) -> Result<kinewright_media::FfmpegMediaEngine, HostError> {
    static GPU: std::sync::OnceLock<Result<kinewright_media::GpuContext, MediaError>> =
        std::sync::OnceLock::new();
    let root =
        data_dir.ok_or_else(|| refused("Review fixture requires a private data directory."))?;
    let gpu = GPU
        .get_or_init(|| kinewright_media::GpuContext::headless(true))
        .as_ref()
        .map_err(|error| refused(&error.to_string()))?;
    kinewright_media::FfmpegMediaEngine::new_with_gpu_and_data_dir(gpu.clone(), root)
        .map_err(|error| refused(&error.to_string()))
}

fn current_stage(byte_limit: u64) -> Result<ReviewStage, HostError> {
    let pid = std::process::id();
    Ok(ReviewStage {
        id: SessionId::fresh()?,
        boot: crate::guardian::boot_id()?,
        pid,
        process_start: process_start(pid)?.ok_or_else(unavailable)?,
        byte_limit,
    })
}

fn process_start(pid: u32) -> io::Result<Option<u64>> {
    let stat = match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(stat) => stat,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    stat.rsplit_once(')')
        .and_then(|(_, fields)| fields.split_whitespace().nth(19))
        .and_then(|start| start.parse().ok())
        .map(Some)
        .ok_or_else(|| io::Error::other("process birth could not be read"))
}

fn prove_retired(stage: &ReviewStage) -> Result<(), HostError> {
    if stage.boot != crate::guardian::boot_id()?
        || process_start(stage.pid)? != Some(stage.process_start)
    {
        return Ok(());
    }
    if stage.pid == std::process::id()
        && !LIVE_STAGES
            .lock()
            .map_err(|_| unavailable())?
            .contains(&stage.id)
    {
        return Ok(());
    }
    Err(refused("The former output review worker has not retired."))
}
