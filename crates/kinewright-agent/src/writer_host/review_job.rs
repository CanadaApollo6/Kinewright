use std::{
    collections::BTreeSet,
    io::{self, Seek},
    sync::{
        Arc, Mutex, MutexGuard, Weak,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use kinewright_core::{ExportCancellation, ExportSettings, MediaError};
use kinewright_media::{OwnedExport, OwnedExportCleanup, OwnedExportError, OwnedExportTarget};
use kinewright_project::{
    bundle::BundleError,
    bundle_records::{ObjectRef, RecordKind, SessionId},
    review_job::{
        PreparedReviewArtifact, ReviewControl, ReviewJob, ReviewPhase, ReviewRequest, ReviewStage,
        ReviewStageCharge, ReviewStaging, ReviewStatus, ReviewToken, SealResult, UpdateReviewJob,
        empty_annotations,
    },
    studio_records::SnapshotKey,
    studio_store::FrozenContextRequest,
};

pub(super) mod review_checks;
pub(super) mod review_driver;

use super::{
    AttachmentRole, HostCommand, HostCommandOwner, HostError, HostRefusal, HostReply, HostRequest,
    review_expectations::{
        ReviewExpectationInput, derive_review_expectations, load_source_transcripts,
    },
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

#[cfg(test)]
pub(super) type ReviewCancelHook = Arc<dyn Fn(&ExportCancellation) + Send + Sync>;
/// A test hook a seal runs before it hashes; an error fails the seal.
#[cfg(test)]
pub(super) type ReviewSealHook =
    Arc<dyn Fn(&ExportCancellation) -> Result<(), HostError> + Send + Sync>;

#[derive(Default)]
pub(super) struct ReviewRuntime {
    worker: Option<Worker>,
    // At most one per project: a Save As leaves the source's Stop with the source.
    pending_stop: Vec<ReviewStatus>,
    // The artifact this host published or verified; check results bind only to it.
    verified: Option<ReviewToken>,
    // Active off-lock check / stills reads cancelled immediately on Stop or host shutdown.
    active_cancellations: Vec<(ReviewToken, ExportCancellation)>,
    // The check driver; Stop and shutdown cancel it, shutdown joins it.
    driver: Option<Driver>,
    // Drivers that exited, or that a Stop or a change of project cancelled.
    // None of them blocks a new driver; each is joined with the lock released.
    retired_drivers: Vec<Driver>,
    // The reopen thread; shutdown cancels and joins it.
    reopen: Option<Reopen>,
    // The job whose retained artifact is hashed with the lock released.
    verifying: Option<ReviewToken>,
    // Counts drivers started, so a test can tell a new driver from the old one.
    #[cfg(test)]
    driver_generation: u64,
    #[cfg(test)]
    probe: Option<Arc<dyn Fn(ReviewPoint) + Send + Sync>>,
    #[cfg(test)]
    exhaust_artifact: bool,
    #[cfg(test)]
    pub(super) overlay_verify_hook: Option<ReviewCancelHook>,
    #[cfg(test)]
    pub(super) stills_render_hook: Option<ReviewCancelHook>,
    // Production always drives a checking job; legacy fixtures bind by hand.
    #[cfg(test)]
    pub(super) auto_drive: bool,
    // Runs with the lock released, before a seal hashes the artifact.
    #[cfg(test)]
    pub(super) seal_hook: Option<ReviewSealHook>,
    // Runs with the lock released, before a reopen hashes the artifact.
    #[cfg(test)]
    pub(super) verify_hook: Option<ReviewCancelHook>,
    // Runs on a driver thread that has decided to exit, after its last lock.
    #[cfg(test)]
    pub(super) driver_exit_hook: Option<ReviewCancelHook>,
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
    charge: Arc<ReviewStageCharge>,
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
    // Released with the worker, after its stage is swept.
    _charge: Arc<ReviewStageCharge>,
}

struct Driver {
    token: ReviewToken,
    cancellation: ExportCancellation,
    // Set under the lock by the step that ends the driver; the thread then
    // returns without taking the lock again.
    exiting: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Driver {
    /// Still the job's driver: neither exiting, cancelled nor finished.
    fn live(&self) -> bool {
        !self.exiting.load(Ordering::Acquire)
            && !self.cancellation.is_cancelled()
            && self
                .thread
                .as_ref()
                .is_some_and(|thread| !thread.is_finished())
    }
}

struct Reopen {
    cancellation: ExportCancellation,
    thread: Option<JoinHandle<()>>,
}

fn join_other(thread: JoinHandle<()>) {
    if thread.thread().id() != thread::current().id() {
        let _ = thread.join();
    }
}

// The runtime drops before the editing session, so the worker is joined while
// the project lock is still held.
impl Drop for ReviewRuntime {
    fn drop(&mut self) {
        self.cancel_for_shutdown();
        for thread in self.take_threads() {
            join_other(thread);
        }
        let Some(worker) = &mut self.worker else {
            return;
        };
        if let Some(thread) = worker.thread.take()
            && thread.thread().id() != thread::current().id()
        {
            worker.cleanup = thread.join().unwrap_or(None);
        }
        if let Some(cleanup) = &mut worker.cleanup {
            let _ = cleanup.retry();
        }
    }
}

impl ReviewRuntime {
    pub(super) fn cancel_for_shutdown(&self) {
        if let Some(worker) = &self.worker {
            worker.cancellation.cancel();
        }
        for driver in self.driver.iter().chain(&self.retired_drivers) {
            driver.cancellation.cancel();
        }
        if let Some(reopen) = &self.reopen {
            reopen.cancellation.cancel();
        }
        for (_, cancel) in &self.active_cancellations {
            cancel.cancel();
        }
    }

    /// Every driver and reopen thread, for joining with the lock released.
    fn take_threads(&mut self) -> Vec<JoinHandle<()>> {
        self.driver
            .iter_mut()
            .chain(&mut self.retired_drivers)
            .filter_map(|driver| driver.thread.take())
            .chain(self.reopen.as_mut().and_then(|reopen| reopen.thread.take()))
            .collect()
    }

    /// The active project changed: no driver, verification or read of the
    /// source continues, and nothing of it blocks the new project.
    pub(super) fn leave_project(
        &mut self,
        project: &kinewright_project::bundle_records::ProjectId,
    ) {
        if let Some(driver) = self
            .driver
            .take_if(|driver| &driver.token.project == project)
        {
            driver.cancellation.cancel();
            self.retired_drivers.push(driver);
        }
        if self
            .verified
            .as_ref()
            .is_some_and(|token| &token.project == project)
        {
            self.verified = None;
        }
        for (token, cancel) in &self.active_cancellations {
            if &token.project == project {
                cancel.cancel();
            }
        }
        self.active_cancellations
            .retain(|(_, cancel)| !cancel.is_cancelled());
    }

    pub(super) fn register_review_cancellation(
        &mut self,
        token: &ReviewToken,
        external: Option<&ExportCancellation>,
    ) -> ExportCancellation {
        self.active_cancellations
            .retain(|(_, cancel)| !cancel.is_cancelled());
        if self.active_cancellations.len() >= 64 {
            let drain = self.active_cancellations.len().saturating_sub(63);
            self.active_cancellations.drain(..drain);
        }
        let cancel = external.cloned().unwrap_or_default();
        if self.fenced(token) {
            cancel.cancel();
        } else {
            self.active_cancellations
                .push((token.clone(), cancel.clone()));
        }
        cancel
    }

    pub(super) fn cancel_review_work(&mut self, token: &ReviewToken) {
        if let Some(driver) = &self.driver
            && &driver.token == token
        {
            driver.cancellation.cancel();
        }
        for (active_token, cancel) in &self.active_cancellations {
            if active_token == token {
                cancel.cancel();
            }
        }
        self.active_cancellations
            .retain(|(active_token, cancel)| active_token != token && !cancel.is_cancelled());
    }

    fn pending_for(
        &self,
        project: &kinewright_project::bundle_records::ProjectId,
    ) -> Option<&ReviewStatus> {
        self.pending_stop
            .iter()
            .find(|status| &status.token.project == project)
    }

    fn fenced(&self, token: &ReviewToken) -> bool {
        self.pending_stop
            .iter()
            .any(|status| &status.token == token)
    }
}

const DRIVER_RUNNING: &str = "The output review checks are still running.";
const REVOKED: &str = "Output review authority was revoked.";
const FROZEN: &str =
    "The project is recovering. Try the output review again when recovery finishes.";
const BUSY_WORKER: &str =
    "Another output review is still encoding or retiring; stop it or wait for it to finish first.";

/// Fixtures whose reopened host drives its review as production does.
#[cfg(test)]
pub(super) static AUTO_DRIVE_ON_OPEN: Mutex<BTreeSet<std::path::PathBuf>> =
    Mutex::new(BTreeSet::new());

/// Reopen hooks a fixture installs before its host starts.
#[cfg(test)]
pub(super) static VERIFY_HOOK_ON_OPEN: Mutex<Vec<(std::path::PathBuf, ReviewCancelHook)>> =
    Mutex::new(Vec::new());

const VERIFY_POLL: Duration = Duration::from_millis(20);
const VERIFY_CANCELLED: &str = "The output review verification was cancelled.";

/// After the endpoint serves: verify a reopened job's artifact and start its
/// driver on a tracked thread. Shutdown cancels and joins it.
pub(super) fn resume_after_open(host: &Arc<Mutex<HostCommandOwner>>) {
    let Ok(mut owner) = host.lock() else {
        return;
    };
    #[cfg(test)]
    if let Some(path) = owner.config.path.clone() {
        if AUTO_DRIVE_ON_OPEN
            .lock()
            .is_ok_and(|paths| paths.contains(&path))
        {
            owner.review_runtime.auto_drive = true;
        }
        if let Ok(mut hooks) = VERIFY_HOOK_ON_OPEN.lock()
            && let Some(index) = hooks.iter().position(|(hooked, _)| hooked == &path)
        {
            owner.review_runtime.verify_hook = Some(hooks.remove(index).1);
        }
    }
    if owner.owner.is_none() || owner.lifecycle.stopping() || owner.review_runtime.reopen.is_some()
    {
        return;
    }
    let cancellation = ExportCancellation::default();
    let weak = Arc::downgrade(host);
    let thread = {
        let cancellation = cancellation.clone();
        thread::Builder::new()
            .name("output-review-reopen".into())
            .spawn(move || reopen_review(&weak, &cancellation))
    };
    if let Ok(thread) = thread {
        owner.review_runtime.reopen = Some(Reopen {
            cancellation,
            thread: Some(thread),
        });
    }
}

/// How often the reopen thread looks again while recovery, a branch switch
/// or native recovery owns the project.
const FROZEN_POLL: Duration = Duration::from_millis(100);

/// Run `step` under the lock once the project is not frozen. `None`: the host
/// went away, stopped or cancelled the thread.
fn when_thawed<T>(
    host: &Weak<Mutex<HostCommandOwner>>,
    cancel: &ExportCancellation,
    mut step: impl FnMut(&mut HostCommandOwner) -> T,
) -> Option<T> {
    loop {
        {
            let host = host.upgrade()?;
            let mut owner = host.lock().ok()?;
            if cancel.is_cancelled() || owner.lifecycle.stopping() {
                return None;
            }
            if !owner.review_frozen() {
                return Some(step(&mut owner));
            }
        }
        thread::sleep(FROZEN_POLL);
    }
}

fn reopen_review(host: &Weak<Mutex<HostCommandOwner>>, cancel: &ExportCancellation) {
    // The open reconciles here, once recovery, a branch switch and native
    // recovery have settled, never on the open itself.
    if !when_thawed(host, cancel, |owner| {
        owner.reconcile_review_job().is_ok() && owner.auto_drive()
    })
    .unwrap_or(false)
        || verify_reopened(host, Some(cancel)).is_err()
    {
        return;
    }
    let _ = when_thawed(host, cancel, HostCommandOwner::ensure_review_driver);
}

#[allow(
    clippy::large_enum_variant,
    reason = "one value per verification, moved straight into the hash"
)]
enum Verification {
    Done,
    // Another caller is hashing the artifact.
    Busy,
    Hash {
        token: ReviewToken,
        prepared: PreparedReviewArtifact,
        cancel: ExportCancellation,
        #[cfg(test)]
        hook: Option<ReviewCancelHook>,
    },
}

/// Clears the verification mark however the hash ends.
struct Verifying<'a> {
    host: &'a Weak<Mutex<HostCommandOwner>>,
    token: ReviewToken,
}

impl Drop for Verifying<'_> {
    fn drop(&mut self) {
        if let Some(host) = self.host.upgrade()
            && let Ok(mut owner) = host.lock()
            && owner.review_runtime.verifying.as_ref() == Some(&self.token)
        {
            owner.review_runtime.verifying = None;
        }
    }
}

/// The host must publish discovery before a potentially large artifact read.
/// Review entry points verify the retained bytes before accepting a resume:
/// the artifact is hashed with the lock released, then the job is checked
/// again under it. A caller that finds a verification under way waits for it.
pub(super) fn verify_reopened(
    host: &Weak<Mutex<HostCommandOwner>>,
    external: Option<&ExportCancellation>,
) -> Result<(), HostError> {
    loop {
        let begun = {
            let host = host.upgrade().ok_or_else(unavailable)?;
            let mut owner = lock(&host)?;
            owner.begin_reopen_verification(external)?
        };
        match begun {
            Verification::Done => return Ok(()),
            Verification::Busy => {
                if external.is_some_and(ExportCancellation::is_cancelled) {
                    return Err(refused(VERIFY_CANCELLED));
                }
                thread::sleep(VERIFY_POLL);
            }
            Verification::Hash {
                token,
                prepared,
                cancel,
                #[cfg(test)]
                hook,
            } => {
                let mark = Verifying {
                    host,
                    token: token.clone(),
                };
                #[cfg(test)]
                if let Some(hook) = hook {
                    hook(&cancel);
                }
                let hashed = prepared.open_verified_cancellable(&cancel).map(drop);
                drop(prepared);
                drop(mark);
                hashed?;
                if cancel.is_cancelled() {
                    return Err(refused(VERIFY_CANCELLED));
                }
                let host = host.upgrade().ok_or_else(unavailable)?;
                let mut owner = lock(&host)?;
                return owner.finish_reopen_verification(&token);
            }
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
    let mut receipt = None;
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
            ReviewControl::Seal { token, command } => {
                {
                    let mut owner = lock(host)?;
                    owner.authorize_review(bearer, request)?;
                    owner.reconcile_review_job()?;
                }
                // Straight after a reopen: verify the retained artifact first.
                verify_reopened(&Arc::downgrade(host), None)?;
                let (expected, cancel) = {
                    let mut owner = lock(host)?;
                    owner.authorize_review(bearer, request)?;
                    // A lost reply: the job is already retired with its receipt.
                    if let Some(sealed) = owner
                        .owner_mut()?
                        .review_source_bundle_mut(&token.project)?
                        .sealed_review(token)?
                    {
                        receipt = Some(sealed.receipt);
                        return owner.review_status();
                    }
                    owner.retire_finished_driver();
                    // A driver still checking owns the job. One that seals a
                    // ready job races this seal; the loser replays the receipt.
                    if owner.review_runtime.driver.is_some()
                        && !owner.review_ready_to_seal(token)?
                    {
                        return Err(refused(DRIVER_RUNNING));
                    }
                    let expected = owner.production_review_digest(token)?;
                    let cancel = owner
                        .review_runtime
                        .register_review_cancellation(token, None);
                    (expected, cancel)
                };
                let sealed =
                    review_driver::seal_checked_review(host, token, command, expected, &cancel)?;
                return match sealed {
                    SealResult::Sealed(sealed) => {
                        receipt = Some(sealed.receipt);
                        Ok(lock(host)?.review_status()?)
                    }
                    SealResult::Paused(status) => Ok(Some(*status)),
                };
            }
            ReviewControl::Status | ReviewControl::Resume(_) | ReviewControl::Check { .. } => {
                let status = matches!(control, ReviewControl::Status);
                let reconciled = {
                    let mut owner = lock(host)?;
                    owner.authorize_review(bearer, request)?;
                    // A read while frozen: the stored job, nothing hashed or written.
                    if status && owner.review_writes_frozen() {
                        return owner.frozen_review_status();
                    }
                    owner.reconcile_review_job().is_ok()
                };
                // A reconcile failure is answered below, as before.
                if reconciled && let Err(error) = verify_reopened(&Arc::downgrade(host), None) {
                    let owner = lock(host)?;
                    if status && owner.review_writes_frozen() {
                        return owner.frozen_review_status();
                    }
                    return Err(error);
                }
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
        Ok(status) => {
            reply.review = status;
            reply.review_receipt = receipt;
        }
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
    let (project, frozen, rulebook, data_dir) = {
        let mut owner = lock(host)?;
        owner.authorize_review(bearer, request)?;
        owner.reconcile_review_job()?;
        let bundle = owner.owner()?.review_bundle()?;
        if bundle.review_job()?.is_some() {
            return Err(refused("An output review is already active."));
        }
        // One encoding worker is tracked at a time; a second would detach the
        // first from cancellation and shutdown.
        if owner.review_runtime.worker.is_some() {
            return Err(refused(BUSY_WORKER));
        }
        (
            bundle.project_id().clone(),
            bundle.read_snapshot(snapshot)?,
            owner.owner()?.approved_rulebook_ref()?.cloned(),
            owner.config.data_dir.clone(),
        )
    };
    let inputs = frozen.open_review_inputs()?;
    let (settings, byte_limit) = delivery.unwrap_or_else(|| {
        // JA-1 S6 R1: the default review always judges loudness.
        let settings = kinewright_core::output_review_default_settings(
            &inputs.document,
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
    // Source-clock transcripts come from the media store, bound to the
    // frozen version's pins. A missing one leaves its speech unchecked.
    let data_dir = review_data_root(data_dir)?;
    let sources = load_source_transcripts(
        &inputs.document,
        &inputs.manifest,
        |content, fps, frames| {
            kinewright_media::stored_source_transcript(&data_dir, content, fps, frames)
        },
    )
    .map_err(|error| refused(&error.to_string()))?;
    let transcripts = sources.review();
    let expectations = derive_review_expectations(
        &ReviewExpectationInput {
            document: &inputs.document,
            snapshot: frozen.descriptor.identity,
            input_manifest: &inputs.manifest,
            settings: &settings,
            transcripts: &transcripts,
        },
        annotations,
    )
    .map_err(|error| refused(&error.to_string()))?;
    drop(transcripts);
    drop(inputs);
    let mut owner = lock(host)?;
    owner.authorize_review(bearer, request)?;
    if owner.owner()?.project_id()? != project {
        return Err(refused("The project changed during review preparation."));
    }
    if owner
        .owner()?
        .review_bundle()?
        .read_snapshot(snapshot)?
        .descriptor
        != frozen.descriptor
    {
        return Err(refused("The version changed during review preparation."));
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
    pub(super) fn review_fixture_auto_drive(&mut self, auto: bool) {
        self.review_runtime.auto_drive = auto;
    }

    #[cfg(test)]
    pub(super) fn review_fixture_driver_done(&self) -> bool {
        self.review_runtime
            .driver
            .as_ref()
            .is_none_or(|driver| !driver.live())
    }

    /// The number of drivers started, and whether the newest still drives.
    #[cfg(test)]
    pub(super) fn review_fixture_driver_generation(&self) -> (u64, bool) {
        (
            self.review_runtime.driver_generation,
            self.review_runtime
                .driver
                .as_ref()
                .is_some_and(Driver::live),
        )
    }

    /// Driver threads not yet joined: the slot's and the retired ones.
    #[cfg(test)]
    pub(super) fn review_fixture_driver_threads(&self) -> usize {
        self.review_runtime
            .driver
            .iter()
            .chain(&self.review_runtime.retired_drivers)
            .filter(|driver| driver.thread.is_some())
            .count()
    }

    /// The reopen thread exists and has not finished.
    #[cfg(test)]
    pub(super) fn review_fixture_reopen_running(&self) -> bool {
        self.review_runtime
            .reopen
            .as_ref()
            .and_then(|reopen| reopen.thread.as_ref())
            .is_some_and(|thread| !thread.is_finished())
    }

    #[cfg(test)]
    pub(super) fn review_fixture_freeze(&mut self, frozen: bool) {
        self.switch_frozen = frozen;
    }

    #[cfg(test)]
    pub(super) fn review_fixture_native_pending(&mut self, pending: bool) {
        self.native_recovery_pending = pending;
    }

    #[cfg(test)]
    pub(super) fn review_fixture_stops_queued(&self) -> usize {
        self.review_runtime.pending_stop.len()
    }

    #[cfg(test)]
    pub(super) fn review_fixture_recovering(&mut self, recovering: bool) {
        self.recovering = recovering;
    }

    #[cfg(test)]
    pub(super) fn review_fixture_verified(&self) -> Option<ReviewToken> {
        self.review_runtime.verified.clone()
    }

    #[cfg(test)]
    pub(super) fn review_fixture_mark_verified(&mut self, token: ReviewToken) {
        self.review_runtime.verified = Some(token);
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
        // The reopen thread and every driver, cancelled by the fence.
        let threads = host.lock().map_or_else(
            |_| Vec::new(),
            |mut owner| {
                owner.review_runtime.cancel_for_shutdown();
                owner.review_runtime.take_threads()
            },
        );
        for thread in threads {
            join_other(thread);
        }
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
        // A clean shutdown retires its own stage as the next open in this
        // process would, so another process need not wait for this one to
        // exit. A failure leaves it to the next open.
        if let Ok(mut owner) = host.lock() {
            let _ = owner.retire_for_shutdown();
        }
    }

    fn retire_for_shutdown(&mut self) -> Result<(), HostError> {
        // A frozen project keeps its job for the next open to reconcile.
        if self.settlement_owned_elsewhere() || self.review_writes_frozen() {
            return Ok(());
        }
        // A retained source records its own worker's retirement; the active
        // job is reconciled as below.
        let active = self.owner()?.project_id().ok();
        if self.review_runtime.worker.as_ref().is_some_and(|worker| {
            worker.thread.is_none() && active.as_ref() != Some(&worker.token.project)
        }) {
            self.retire_finished_worker()?;
        }
        if let Some(worker) = &mut self.review_runtime.worker {
            if worker.thread.is_some() {
                return Ok(());
            }
            let stage = worker.stage.clone();
            let project = worker.token.project.clone();
            if let Some(cleanup) = &mut worker.cleanup {
                cleanup.retry()?;
            }
            self.owner()?
                .review_source_bundle(&project)?
                .store()
                .sweep_review_staging(&stage)?;
            // Releases the staging charge after the sweep.
            self.review_runtime.worker = None;
        }
        self.reconcile_review_job()
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
            || (self.settlement_owned_elsewhere()
                && !matches!(
                    request.command,
                    HostCommand::Review(ReviewControl::Stop(_) | ReviewControl::Status)
                ))
        {
            return Err(BundleError::Unsettled.into());
        }
        // Status and Stop stay open: Status reads, Stop waits behind its fence.
        if self.review_writes_frozen()
            && !matches!(
                request.command,
                HostCommand::Review(ReviewControl::Stop(_) | ReviewControl::Status)
            )
        {
            return Err(refused(FROZEN));
        }
        Ok(())
    }

    /// Status while frozen: a Stop waiting to persist, else the stored job.
    /// An unsettled root shows no phase: its owner may still roll it back.
    fn frozen_review_status(&self) -> Result<Option<ReviewStatus>, HostError> {
        if self.owner()?.review_bundle()?.review_root_unsettled() {
            return Err(BundleError::Unsettled.into());
        }
        if let Some(pending) = self
            .owner()?
            .project_id()
            .ok()
            .and_then(|project| self.review_runtime.pending_for(&project))
        {
            return Ok(Some(pending.clone()));
        }
        self.review_status()
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
                && let Some(pending) = self
                    .owner()?
                    .project_id()
                    .ok()
                    .and_then(|project| self.review_runtime.pending_for(&project))
            {
                return Ok(Some(pending.clone()));
            }
            return Err(error);
        }
        match control {
            // A reopened Checking or ReadyToSeal job resumes its driver.
            ReviewControl::Status => self.ensure_review_driver()?,
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
            ReviewControl::Resume(token) => {
                self.resume_review(token)?;
                self.ensure_review_driver()?;
            }
            ReviewControl::Check {
                token,
                reviewer_attempt,
                command,
            } => self.start_review_driver(token, reviewer_attempt, command)?,
            ReviewControl::Queue { .. }
            | ReviewControl::QueueDefault(_)
            | ReviewControl::Seal { .. }
            | ReviewControl::Stop(_) => return Err(unavailable()),
        }
        self.review_status()
    }

    fn review_status(&self) -> Result<Option<ReviewStatus>, HostError> {
        Ok(self.owner()?.review_bundle()?.review_status()?)
    }

    /// An uncertain publication whose own owner settles it: a writer hook's,
    /// or a recovery or branch switch's root. Review writes wait behind it.
    pub(super) fn settlement_owned_elsewhere(&self) -> bool {
        self.hooks.pending.is_some()
            || self
                .owner()
                .is_ok_and(kinewright_project::editing_session::Ja1EditingSession::switch_publication_unresolved)
    }

    fn stop_review_job(&mut self, token: &ReviewToken) -> Result<Option<ReviewStatus>, HostError> {
        if self
            .review_runtime
            .pending_for(&token.project)
            .is_some_and(|pending| &pending.token != token)
        {
            let _ = self.persist_pending_stops();
            if self.review_runtime.pending_for(&token.project).is_some() {
                return Err(refused("A different output review is still stopping."));
            }
        }
        if self.review_runtime.pending_for(&token.project).is_none() {
            // Arm the fence before any read that capacity or an unsettled root
            // can refuse; such a read only leaves the phase unknown. Behind
            // another owner's uncertain publication nothing is read or settled.
            // While frozen the phase is read without settling the root.
            let pending_owner = self.settlement_owned_elsewhere();
            let frozen = self.review_writes_frozen();
            // While frozen an unsettled root is neither settled nor read, so
            // no generation is armed from it.
            if frozen
                && !pending_owner
                && self.owner().is_ok_and(|session| {
                    session
                        .review_source_bundle(&token.project)
                        .is_ok_and(kinewright_project::bundle::ProjectBundle::review_root_unsettled)
                })
            {
                return Err(refused(FROZEN));
            }
            let phase = match self.owner_mut().map_err(drop).and_then(|session| {
                if pending_owner {
                    return Err(());
                }
                let bundle = session
                    .review_source_bundle_mut(&token.project)
                    .map_err(drop)?;
                if frozen {
                    bundle.review_status()
                } else {
                    bundle.review_status_for_stop()
                }
                .map_err(drop)
            }) {
                Ok(Some(status)) if &status.token == token => {
                    if status.phase == ReviewPhase::Stopped {
                        return Ok(Some(status));
                    }
                    status.phase
                }
                Ok(_) => return Err(refused("The output review generation or version changed.")),
                Err(()) => ReviewPhase::Stopping,
            };
            self.review_runtime.pending_stop.push(ReviewStatus {
                token: token.clone(),
                phase,
                stop_pending: true,
            });
        }
        let index = self
            .review_runtime
            .pending_stop
            .iter()
            .position(|status| &status.token == token)
            .ok_or_else(unavailable)?;
        // Cancel any in-flight off-lock overlay verification or review-stills read for this token
        // immediately, even if durable Stop persistence is deferred by storage backpressure.
        self.review_runtime.cancel_review_work(token);
        let pending = self.review_runtime.pending_stop.remove(index);
        match self.persist_review_stop(&pending.token) {
            Ok(true) => {}
            Ok(false) => return Err(refused("The output review generation or version changed.")),
            Err(_) => {
                // The reply is a copy; the fence stays armed.
                let reply = pending.clone();
                self.review_runtime.pending_stop.push(pending);
                // A finished worker is swept and its charge released; its
                // retirement persists the waiting Stop with the space it frees.
                if !self.settlement_owned_elsewhere()
                    && !self.review_writes_frozen()
                    && self.review_runtime.worker.is_some()
                {
                    let _ = self.retire_review_worker();
                }
                if self.review_runtime.fenced(token) {
                    return Ok(Some(reply));
                }
            }
        }
        // The durable Stopping phase already fences every writer. A cleanup error
        // leaves it in that phase for the next retry or reopen.
        let _ = self.reconcile_review_job();
        let status = self
            .owner_mut()?
            .review_source_bundle_mut(&token.project)
            .and_then(kinewright_project::bundle::ProjectBundle::review_status_for_stop);
        Ok(Some(match status {
            Ok(Some(status)) if &status.token == token => status,
            _ => ReviewStatus {
                token: token.clone(),
                phase: ReviewPhase::Stopping,
                stop_pending: false,
            },
        }))
    }

    /// `Ok(true)`: Stopping is durable. `Ok(false)`: no fence is needed, as the
    /// job is definitely not current or its source writer is gone.
    fn persist_review_stop(&mut self, token: &ReviewToken) -> Result<bool, HostError> {
        if self.settlement_owned_elsewhere() {
            return Err(BundleError::Unsettled.into());
        }
        // The Stop stays armed and persists on the first reconcile after the freeze.
        if self.review_writes_frozen() {
            return Err(refused(FROZEN));
        }
        let session = self.owner_mut()?;
        if session.review_source_bundle(&token.project).is_err() {
            // Without its source writer, no install or publication can reach the job.
            return Ok(false);
        }
        let bundle = session.review_source_bundle_mut(&token.project)?;
        if let Err(error) = bundle.update_review_job(token, UpdateReviewJob::Stop) {
            if bundle
                .review_status_for_stop()
                .is_ok_and(|status| status.is_none_or(|status| &status.token != token))
            {
                return Ok(false);
            }
            return Err(error.into());
        }
        if let Some(worker) = &self.review_runtime.worker
            && &worker.token == token
        {
            worker.cancellation.cancel();
        }
        self.review_runtime.cancel_review_work(token);
        Ok(true)
    }

    /// The active project's failure is returned; another project's pending Stop
    /// stays armed without blocking this one.
    fn persist_pending_stops(&mut self) -> Result<(), HostError> {
        let active = self.owner()?.project_id().ok();
        let mut result = Ok(());
        for pending in std::mem::take(&mut self.review_runtime.pending_stop) {
            match self.persist_review_stop(&pending.token) {
                Ok(_) => {}
                Err(error) => {
                    if active.as_ref() == Some(&pending.token.project) {
                        result = Err(error);
                    }
                    self.review_runtime.pending_stop.push(pending);
                }
            }
        }
        result
    }

    /// The first point after a freeze ends, a /host request or native
    /// recovery's exit: a Stop queued behind the freeze persists now, without
    /// waiting for the next review command.
    pub(super) fn persist_stops_after_thaw(host: &Mutex<Self>) {
        let Ok(mut owner) = host.lock() else {
            return;
        };
        if owner.review_runtime.pending_stop.is_empty()
            || owner.review_frozen()
            || owner.settlement_owned_elsewhere()
        {
            return;
        }
        let _ = owner.persist_pending_stops();
    }

    pub(super) fn reconcile_review_job(&mut self) -> Result<(), HostError> {
        if self.settlement_owned_elsewhere() {
            return Err(BundleError::Unsettled.into());
        }
        if self.review_writes_frozen() {
            return Err(refused(FROZEN));
        }
        // A finished worker is swept and its charge released even while a Stop
        // waits for space to persist.
        let retired = self.retire_review_worker();
        let stopped = self.persist_pending_stops();
        retired?;
        stopped?;
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
        // An attempt bound before this host owned the job belongs to a process
        // that is gone; it never accepts a result here.
        let unbind = matches!(&job.phase, ReviewPhase::Checking(checks) if checks.reviewer_attempt.is_some())
            && self.review_runtime.verified.as_ref() != Some(&token);
        if let Some(stage) = &job.stage {
            prove_retired(stage)?;
            bundle.store().sweep_review_staging(stage)?;
        }
        let update = match job.phase {
            ReviewPhase::Encoding => Some(UpdateReviewJob::RecoverEncoding),
            ReviewPhase::Stopping => Some(UpdateReviewJob::RetireStopped),
            _ if job.stage.is_some() => Some(UpdateReviewJob::RetireStage),
            _ => None,
        };
        drop(job);
        if unbind {
            self.owner_mut()?
                .update_review_job(&token, UpdateReviewJob::UnbindChecks)?;
        }
        if let Some(update) = update {
            self.owner_mut()?.update_review_job(&token, update)?;
        }
        Ok(())
    }

    /// Pin the reopened job's artifact for hashing with the lock released.
    fn begin_reopen_verification(
        &mut self,
        external: Option<&ExportCancellation>,
    ) -> Result<Verification, HostError> {
        if self.lifecycle.stopping() {
            return Err(refused(REVOKED));
        }
        // No artifact read while frozen; the first command after it verifies.
        if self.review_frozen() {
            return Err(refused(FROZEN));
        }
        let bundle = self.owner()?.review_bundle()?;
        let Some(job) = bundle.review_job()?.filter(|job| {
            job.artifact_ref.is_some()
                && job.phase != ReviewPhase::Stopping
                && self.review_runtime.verified.as_ref() != Some(&job.token)
        }) else {
            return Ok(Verification::Done);
        };
        let token = job.token.clone();
        drop(job);
        if self.review_runtime.verifying.as_ref() == Some(&token) {
            return Ok(Verification::Busy);
        }
        let prepared = bundle.prepare_review_artifact(&token)?;
        let cancel = self
            .review_runtime
            .register_review_cancellation(&token, external);
        self.review_runtime.verifying = Some(token.clone());
        Ok(Verification::Hash {
            token,
            prepared,
            cancel,
            #[cfg(test)]
            hook: self.review_runtime.verify_hook.clone(),
        })
    }

    /// The bytes matched: if the job is still the one hashed, an attempt bound
    /// by a former process is unbound and the artifact counts as verified.
    fn finish_reopen_verification(&mut self, token: &ReviewToken) -> Result<(), HostError> {
        let bundle = self.owner()?.review_bundle()?;
        let Some(job) = bundle.review_job()?.filter(|job| {
            &job.token == token && job.artifact_ref.is_some() && job.phase != ReviewPhase::Stopping
        }) else {
            return Ok(());
        };
        if self.review_runtime.verified.as_ref() == Some(token) {
            return Ok(());
        }
        // Recovery, a branch switch or native recovery took the project
        // during the hash: nothing is written; the next command verifies again.
        if self.review_frozen() {
            return Err(refused(FROZEN));
        }
        let bound = matches!(&job.phase, ReviewPhase::Checking(checks) if checks.reviewer_attempt.is_some());
        drop(job);
        self.review_authority(token)?;
        if bound {
            self.owner_mut()?
                .update_review_job(token, UpdateReviewJob::UnbindChecks)?;
        }
        self.review_runtime.verified = Some(token.clone());
        Ok(())
    }

    /// Recovery, a branch switch or native recovery owns the project: review
    /// threads wait and start nothing.
    pub(super) fn review_frozen(&self) -> bool {
        self.lifecycle.stopping() || self.recovery_only || self.review_writes_frozen()
    }

    /// Recovery, a branch switch or native recovery holds the project: no
    /// review write lands until it settles.
    pub(super) fn review_writes_frozen(&self) -> bool {
        self.recovering || self.switch_frozen || self.native_recovery_pending
    }

    /// A retained project's retirement failure waits for its next command or
    /// reopen; only the active project's failure is returned.
    pub(super) fn retire_review_worker(&mut self) -> Result<(), HostError> {
        let Some(project) = self
            .review_runtime
            .worker
            .as_ref()
            .map(|worker| worker.token.project.clone())
        else {
            return Ok(());
        };
        let retired = self.retire_finished_worker();
        let active = self.owner()?.project_id().ok();
        if active.as_ref() == Some(&project) {
            retired
        } else {
            Ok(())
        }
    }

    fn retire_finished_worker(&mut self) -> Result<(), HostError> {
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
        // The thread has exited. Its stage needs no reservation to sweep, and the
        // next open of a gone source bundle sweeps it after proving this retirement.
        let source = self.owner()?.review_source_bundle(&token.project).is_ok();
        if source {
            let bundle = self.owner_mut()?.review_source_bundle_mut(&token.project)?;
            bundle.store().sweep_review_staging(&stage)?;
        }
        if let Some(cleanup) = self
            .review_runtime
            .worker
            .as_mut()
            .and_then(|worker| worker.cleanup.as_mut())
        {
            let retried = cleanup.retry();
            if source {
                retried?;
            }
        }
        // Releases the staging charge with the worker.
        self.review_runtime.worker = None;
        if !source {
            self.review_runtime
                .pending_stop
                .retain(|pending| pending.token != token);
            return Ok(());
        }
        if let Some(index) = self
            .review_runtime
            .pending_stop
            .iter()
            .position(|pending| pending.token == token)
        {
            let pending = self.review_runtime.pending_stop.remove(index);
            if let Err(error) = self.persist_review_stop(&token) {
                self.review_runtime.pending_stop.push(pending);
                return Err(error);
            }
        }
        let bundle = self.owner_mut()?.review_source_bundle_mut(&token.project)?;
        bundle.settle_review_root()?;
        if let Some(job) = bundle.review_job()?
            && job.token == token
        {
            let update = match job.phase {
                ReviewPhase::Encoding => UpdateReviewJob::Pause(
                    "Encoding ended before a durable artifact was published.".into(),
                ),
                ReviewPhase::Stopping => UpdateReviewJob::RetireStopped,
                _ if job.stage.is_some() => UpdateReviewJob::RetireStage,
                _ => return Ok(()),
            };
            bundle.update_review_job(&token, update)?;
            if matches!(job.phase, ReviewPhase::Encoding) {
                bundle.update_review_job(&token, UpdateReviewJob::RetireStage)?;
            }
        }
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
        if self.review_runtime.worker.is_some() {
            return Err(refused(BUSY_WORKER));
        }
        let bundle = self.owner()?.review_bundle()?;
        let charge = Arc::new(bundle.store().reserve_review_stage(&stage)?);
        let working = bundle.store().reserve_review_working();
        let staging = Arc::new(bundle.store().create_review_staging(&stage, &charge)?);
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
            _charge: charge,
        });
        Ok(())
    }

    /// Every review write's gate, for /host controls, drivers and workers.
    fn review_authority(&self, token: &ReviewToken) -> Result<(), HostError> {
        self.review_owned(token)?;
        if self.review_writes_frozen() {
            return Err(refused(FROZEN));
        }
        Ok(())
    }

    /// This host still owns the job, frozen or not.
    pub(super) fn review_owned(&self, token: &ReviewToken) -> Result<(), HostError> {
        if self.lifecycle.stopping()
            || self.recovery_only
            || self.settlement_owned_elsewhere()
            || !self.retention_proven
            || self.review_runtime.fenced(token)
        {
            return Err(refused(REVOKED));
        }
        Ok(())
    }

    fn review_can_complete(&self, token: &ReviewToken) -> Result<(), HostError> {
        self.review_authority(token)?;
        let bundle = self.owner()?.review_source_bundle(&token.project)?;
        if !bundle
            .review_job()?
            .is_some_and(|job| job.token == *token && job.phase == ReviewPhase::Encoding)
        {
            return Err(refused("The output review generation or version changed."));
        }
        Ok(())
    }

    /// The checking variant of the fence: the artifact this host published or
    /// verified, in a durable checking phase, with no Stop pending.
    pub(super) fn review_can_record(&self, token: &ReviewToken) -> Result<(), HostError> {
        self.review_authority(token)?;
        if self.review_runtime.verified.as_ref() != Some(token) {
            return Err(refused(
                "The output review artifact has not been verified by this host.",
            ));
        }
        let bundle = self.owner()?.review_source_bundle(&token.project)?;
        if !bundle
            .review_job()?
            .is_some_and(|job| job.token == *token && matches!(job.phase, ReviewPhase::Checking(_)))
        {
            return Err(refused("The output review generation or version changed."));
        }
        Ok(())
    }

    /// The sealing variant of the fence: the verified artifact of the current
    /// job, with every row recorded or a seal still to retry.
    pub(super) fn review_can_seal(&self, token: &ReviewToken) -> Result<(), HostError> {
        self.review_authority(token)?;
        if self.review_runtime.verified.as_ref() != Some(token) {
            return Err(refused(
                "The output review artifact has not been verified by this host.",
            ));
        }
        let bundle = self.owner()?.review_source_bundle(&token.project)?;
        if !bundle.review_job()?.is_some_and(|job| {
            job.token == *token
                && matches!(
                    job.phase,
                    ReviewPhase::Checking(_) | ReviewPhase::ReadyToSeal { .. }
                )
        }) {
            return Err(refused("The output review generation or version changed."));
        }
        Ok(())
    }

    /// Host authority over a sealed job ends once its receipt is durable.
    pub(super) fn release_sealed_review(&mut self, token: &ReviewToken) {
        if self.review_runtime.verified.as_ref() == Some(token) {
            self.review_runtime.verified = None;
        }
        self.release_bound_stills(token);
        self.review_runtime
            .active_cancellations
            .retain(|(active, _)| active != token);
    }

    /// The digest the production driver binds for the job's reviewer.
    fn production_review_digest(&self, token: &ReviewToken) -> Result<[u8; 32], HostError> {
        let bundle = self.owner()?.review_source_bundle(&token.project)?;
        let job = bundle
            .review_job()?
            .filter(|job| &job.token == token)
            .ok_or_else(|| refused("The output review generation or version changed."))?;
        let attempt = match &job.phase {
            ReviewPhase::Checking(checks) | ReviewPhase::ReadyToSeal { checks, .. } => {
                checks.reviewer_attempt.clone()
            }
            _ => None,
        }
        .ok_or_else(|| refused("No reviewer is bound to this output review."))?;
        Ok(review_driver::DriverConfiguration::production(&attempt)
            .binding(&attempt)
            .configuration_sha256)
    }

    fn review_ready_to_seal(&self, token: &ReviewToken) -> Result<bool, HostError> {
        Ok(self
            .owner()?
            .review_source_bundle(&token.project)?
            .review_job()?
            .is_some_and(|job| {
                &job.token == token && matches!(job.phase, ReviewPhase::ReadyToSeal { .. })
            }))
    }

    /// Free the driver slot of a driver that is no longer live, and join
    /// retired drivers that have finished. An exiting or cancelled driver
    /// never blocks a new one; it is joined later with the lock released.
    fn retire_finished_driver(&mut self) {
        let runtime = &mut self.review_runtime;
        if let Some(driver) = runtime.driver.take_if(|driver| !driver.live()) {
            runtime.retired_drivers.push(driver);
        }
        for driver in &mut runtime.retired_drivers {
            if let Some(thread) = driver.thread.take_if(|thread| thread.is_finished()) {
                join_other(thread);
            }
        }
        runtime
            .retired_drivers
            .retain(|driver| driver.thread.is_some());
    }

    /// Join retired drivers with the lock released: every one is cancelled
    /// or exiting, so each returns promptly.
    pub(super) fn join_retired_review_drivers(host: &Mutex<Self>) {
        let threads: Vec<_> = host.lock().map_or_else(
            |_| Vec::new(),
            |mut owner| {
                let runtime = &mut owner.review_runtime;
                for driver in &runtime.retired_drivers {
                    driver.cancellation.cancel();
                }
                std::mem::take(&mut runtime.retired_drivers)
                    .into_iter()
                    .filter_map(|mut driver| driver.thread.take())
                    .collect()
            },
        );
        for thread in threads {
            join_other(thread);
        }
    }

    #[cfg_attr(
        not(test),
        expect(clippy::unused_self, reason = "only the test build reads a switch")
    )]
    fn auto_drive(&self) -> bool {
        #[cfg(test)]
        return self.review_runtime.auto_drive;
        #[cfg(not(test))]
        true
    }

    /// Start the driver for the current job when it is Checking or
    /// `ReadyToSeal` on the artifact this host verified and none runs. Called
    /// on entry to Checking, on Resume and on Status, so a reopened job
    /// continues to its seal.
    pub(super) fn ensure_review_driver(&mut self) -> Result<(), HostError> {
        if !self.auto_drive() || self.review_frozen() {
            return Ok(());
        }
        self.retire_finished_driver();
        if self.review_runtime.driver.is_some() {
            return Ok(());
        }
        let Some(job) = self.owner()?.review_bundle()?.review_job()? else {
            return Ok(());
        };
        let token = job.token.clone();
        let drivable = matches!(
            job.phase,
            ReviewPhase::Checking(_) | ReviewPhase::ReadyToSeal { .. }
        );
        drop(job);
        if !drivable
            || self.review_runtime.verified.as_ref() != Some(&token)
            || self.review_authority(&token).is_err()
        {
            return Ok(());
        }
        self.spawn_review_driver(&token, SessionId::fresh()?)
    }

    /// `Check` names the Reviewer attempt itself: bind it, then drive.
    fn start_review_driver(
        &mut self,
        token: &ReviewToken,
        reviewer_attempt: &SessionId,
        command: &SessionId,
    ) -> Result<(), HostError> {
        self.retire_finished_driver();
        let waiting = self
            .review_runtime
            .driver
            .as_ref()
            .is_some_and(|driver| &driver.token == token && driver.live());
        if self.review_runtime.driver.is_some() && !waiting {
            return Err(refused(DRIVER_RUNNING));
        }
        self.review_can_record(token)?;
        let bound = self.bound_reviewer(token)?;
        if bound
            .as_ref()
            .is_some_and(|bound| bound != reviewer_attempt)
        {
            return Err(refused(
                "Another reviewer attempt is already bound to this output review.",
            ));
        }
        if waiting && bound.is_some() {
            return Err(refused(DRIVER_RUNNING));
        }
        self.claim_review_check(token, reviewer_attempt)?;
        if waiting {
            return Ok(());
        }
        self.spawn_review_driver(token, command.clone())
    }

    fn bound_reviewer(&self, token: &ReviewToken) -> Result<Option<SessionId>, HostError> {
        let bundle = self.owner()?.review_source_bundle(&token.project)?;
        Ok(bundle
            .review_job()?
            .filter(|job| &job.token == token)
            .and_then(|job| match &job.phase {
                ReviewPhase::Checking(checks) => checks.reviewer_attempt.clone(),
                _ => None,
            }))
    }

    /// Bind `attempt`, a live Reviewer of the cut, with the production
    /// configuration. The waiting driver then runs the checks.
    fn claim_review_check(
        &mut self,
        token: &ReviewToken,
        attempt: &SessionId,
    ) -> Result<(), HostError> {
        self.require_reviewer_of_cut(token, attempt)?;
        let config = review_driver::DriverConfiguration::production(attempt);
        let binding = config.binding(attempt);
        self.record_review_check(token, UpdateReviewJob::BindChecks(binding.clone()))?;
        self.register_bound_c6_configuration(token, &binding, &config.c6)
    }

    /// The Reviewer's first stills read claims a checking job that a
    /// driver waits on and no attempt is bound to.
    pub(super) fn claim_waiting_review(&mut self, attempt: &SessionId) -> Result<(), HostError> {
        let Some(token) = self
            .review_runtime
            .driver
            .as_ref()
            .filter(|driver| driver.live())
            .map(|driver| driver.token.clone())
        else {
            return Ok(());
        };
        let unbound = self
            .owner()?
            .review_source_bundle(&token.project)?
            .review_job()?
            .is_some_and(|job| {
                job.token == token
                    && matches!(&job.phase, ReviewPhase::Checking(checks) if checks.reviewer_attempt.is_none())
            });
        if !unbound || self.review_can_record(&token).is_err() {
            return Ok(());
        }
        self.claim_review_check(&token, attempt)
    }

    fn spawn_review_driver(
        &mut self,
        token: &ReviewToken,
        command: SessionId,
    ) -> Result<(), HostError> {
        let cancellation = self
            .review_runtime
            .register_review_cancellation(token, None);
        let weak = self.runtime_owner.clone();
        let exiting = Arc::new(AtomicBool::new(false));
        #[cfg(test)]
        let exit_hook = self.review_runtime.driver_exit_hook.clone();
        let thread = {
            let token = token.clone();
            let cancellation = cancellation.clone();
            let exiting = Arc::clone(&exiting);
            thread::Builder::new()
                .name("output-review-checks".into())
                .spawn(move || {
                    review_driver::run_review_driver(
                        &weak,
                        &token,
                        &command,
                        &cancellation,
                        &exiting,
                    );
                    #[cfg(test)]
                    if let Some(hook) = exit_hook {
                        hook(&cancellation);
                    }
                })?
        };
        #[cfg(test)]
        {
            self.review_runtime.driver_generation += 1;
        }
        self.review_runtime.driver = Some(Driver {
            token: token.clone(),
            cancellation,
            exiting,
            thread: Some(thread),
        });
        Ok(())
    }

    /// The one path by which commits 5 to 7 bind a checker and record results.
    pub(super) fn record_review_check(
        &mut self,
        token: &ReviewToken,
        update: UpdateReviewJob,
    ) -> Result<ReviewStatus, HostError> {
        if !matches!(
            update,
            UpdateReviewJob::BindChecks(_)
                | UpdateReviewJob::RecordCheck { .. }
                | UpdateReviewJob::RecordUnknown { .. }
        ) {
            return Err(refused(
                "Only check bindings and results are recorded here.",
            ));
        }
        self.review_can_record(token)?;
        Ok(self
            .owner_mut()?
            .review_source_bundle_mut(&token.project)?
            .update_review_job(token, update)?)
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
        let media = super::LazyHostMedia::new(Some(review_data_root(work.data_dir)?));
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
        // The staging callback keeps its own owner of the charge; the worker
        // still marks the end of writing below.
        let charge = Arc::clone(&work.charge);
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
        // The writer has returned; no further staging byte can land.
        work.charge.finish_writing();
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
            owner.review_runtime.verified = Some(token.clone());
            // Entry to Checking: the driver waits for the Reviewer's claim. A
            // driver that cannot start yet starts on the next Status.
            let _ = owner.ensure_review_driver();
        }
        #[cfg(test)]
        probe(ReviewPoint::AfterPublication);
        drop(receipt);
        Ok(())
    })();
    (result, pending)
}

/// Every build: output review reads only the data root its host was given.
/// The app names the person's root when it launches the host; no review
/// path resolves one, so no test build can reach the person's data.
fn review_data_root(data_dir: Option<std::path::PathBuf>) -> Result<std::path::PathBuf, HostError> {
    data_dir.ok_or_else(|| refused(NO_DATA_ROOT))
}

pub(super) const NO_DATA_ROOT: &str = "Output review requires the host's data directory.";

pub(super) fn review_media_engine_for_data_dir(
    data_dir: Option<std::path::PathBuf>,
) -> Result<kinewright_media::FfmpegMediaEngine, HostError> {
    #[cfg(not(test))]
    {
        kinewright_media::FfmpegMediaEngine::new_with_data_dir(review_data_root(data_dir)?)
            .map_err(|error| refused(&error.to_string()))
    }
    #[cfg(test)]
    {
        review_fixture_engine(data_dir)
    }
}

#[cfg(test)]
fn review_fixture_engine(
    data_dir: Option<std::path::PathBuf>,
) -> Result<kinewright_media::FfmpegMediaEngine, HostError> {
    static GPU: std::sync::OnceLock<Result<kinewright_media::GpuContext, MediaError>> =
        std::sync::OnceLock::new();
    let root = review_data_root(data_dir)?;
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

pub(super) fn process_start(pid: u32) -> io::Result<Option<u64>> {
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

pub(super) fn prove_retired(stage: &ReviewStage) -> Result<(), HostError> {
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
