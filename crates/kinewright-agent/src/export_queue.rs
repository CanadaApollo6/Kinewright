use std::{
    any::Any,
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, MutexGuard, RwLock,
        atomic::{AtomicU64, Ordering},
    },
    thread,
};

use crossbeam_channel::{Receiver, Sender};
use kinewright_core::{
    Analysis, AssetId, DELIVERY_VERIFICATION_FRAME_COUNT, DeliveryBudgets,
    DeliveryConformanceReport, DeliveryEncodeDepth, DeliveryProfile, DeliveryVerification,
    DeliveryVerificationRequest, Document, Export, ExportCancellation, ExportLutPreflightReport,
    ExportMediaPreflightIssue, ExportMediaPreflightReport, ExportProgress, ExportSettings,
    MediaAvailabilityKind, MediaError, MediaSourceFingerprint, delivery_conformance,
    document_for_delivery_profile, export_lut_preflight_with, export_media_preflight,
    lut_node_may_be_active,
};
use kinewright_media::LutStore;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const DEFAULT_EXPORT_QUEUE_CAPACITY: usize = 64;

/// Why a job that asked for a verification and was cancelled after its encode
/// finished carries no measurement (CC6 §6.5).
///
/// Cancellation cannot un-write a finished file, and the only thing left for it
/// to honour is skipping the measurement. That is a verification that could not
/// run, so it is recorded in the one field that carries such reasons rather
/// than left as a bare absence a caller could read as a pass nobody took. The
/// wording is the app dialog's, so one operator reads one sentence whichever
/// surface ran the export.
pub const EXPORT_CANCELLED_BEFORE_VERIFICATION: &str = "cancelled before verification";

/// Stable identifier assigned in enqueue order for the lifetime of a queue.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
pub struct ExportJobId(pub u64);

/// The exact delivery request captured when an export is queued.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct QueueExportRequest {
    pub output_path: PathBuf,
    pub profile: DeliveryProfile,
    pub focus_x_percent: u8,
    pub focus_y_percent: u8,
    /// Permission to replace a regular file already at `output_path`.
    /// Callers remain responsible for obtaining user confirmation before
    /// setting this to true.
    #[serde(default)]
    pub overwrite: bool,
    /// CC6 §6.5: decode the finished encode and compare it against a freshly
    /// rendered delivery reference. Defaults to **true**; verification reads a
    /// file the caller just asked to write, so it needs no confirmation gate.
    ///
    /// A verification is a *measurement*. It never moves, renames, deletes, or
    /// quarantines the encode it just read, and it never fails a job.
    #[serde(default = "crate::schema::default_true")]
    pub verify: bool,
    /// CC6 §4.1: the delivery encode depth this job runs at. Defaults to the
    /// 8-bit lane, so a pre-CC6 request means exactly what it used to.
    #[serde(default)]
    pub delivery_bit_depth: DeliveryEncodeDepth,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExportJobState {
    Queued,
    Running,
    Completed,
    Failed,
    Cancelled,
}

impl ExportJobState {
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExportJobProgress {
    pub completed_frames: u64,
    pub total_frames: u64,
}

/// Machine-readable status for one immutable export snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExportJobRecord {
    pub id: ExportJobId,
    pub output_path: PathBuf,
    pub profile: DeliveryProfile,
    pub focus_x_percent: u8,
    pub focus_y_percent: u8,
    pub overwrite: bool,
    pub state: ExportJobState,
    pub progress: ExportJobProgress,
    pub conformance: DeliveryConformanceReport,
    /// The delivery lane this job encoded at.
    ///
    /// Defaulted on read, so a job record written before CC6 deserializes as
    /// the 8-bit lane, which is what it meant (CC6 §9.3).
    #[serde(default)]
    pub delivery_bit_depth: DeliveryEncodeDepth,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// CC6 §6.5: the decoded, re-probed comparison of the finished encode.
    ///
    /// `None` when the job did not ask for verification, has not finished, or
    /// could not be verified — in which case
    /// [`Self::verification_unavailable_reason`] says why. A record that ran
    /// with `verify: false` carries neither verification key, so it serializes
    /// exactly as a pre-CC6 record did *apart from* `delivery_bit_depth`, which
    /// is a plain defaulted field and is always emitted: a reader must always
    /// be able to see which delivery lane a job ran at (CC6 §9.3/§9.4).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verification: Option<DeliveryVerification>,
    /// Why [`Self::verification`] is absent, when a verification was asked for
    /// and could not run. Never invents a pass.
    ///
    /// **Normative (CC6 §6.5, errata E31): this field is the sole carrier.**
    /// There is no `verification_unavailable` entry on any exception list, and
    /// `ExportJobRecord` publishes no exception list of its own — the
    /// exceptions that exist live inside [`DeliveryVerification`], which by
    /// definition is absent exactly when this reason is present. A surface that
    /// wants to render NOT VERIFIED reads this field.
    ///
    /// A job cancelled after its encode finished but before its verification
    /// started carries [`EXPORT_CANCELLED_BEFORE_VERIFICATION`] here: the file
    /// on disk is complete and nothing measured it, which is exactly what this
    /// field exists to say.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verification_unavailable_reason: Option<String>,
    /// True only while the post-encode verification is actually running.
    ///
    /// CC6 §6.5: the encode and the verification are two different waits, and
    /// [`ExportJobState::Running`] cannot tell them apart — a caller polling
    /// `get_export_jobs` would otherwise see a job stalled at 100% of its
    /// frames with no way to know whether it was still encoding. This is a
    /// progress detail, not a state: it deliberately adds no
    /// [`ExportJobState`] variant, so every existing terminal check keeps
    /// meaning what it meant.
    ///
    /// **Verification is not interruptible.** Cancelling a job whose
    /// verification has already started sets the record to
    /// [`ExportJobState::Cancelled`] at once, but the in-flight measurement
    /// runs to completion and is then discarded rather than published; this
    /// flag clears when it does.
    ///
    /// Skipped when false, so a record that never verified — and every record
    /// written before CC6 — serializes byte-identically.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub verifying: bool,
}

#[derive(Debug, Error)]
pub enum ExportQueueError {
    #[error("the export output path is empty or does not name a file")]
    InvalidOutputPath,
    #[error("delivery profile {profile:?} requires a .{required} output file: {}", .path.display())]
    InvalidOutputExtension {
        profile: DeliveryProfile,
        required: &'static str,
        path: PathBuf,
    },
    #[error("the export output directory does not exist: {}", .0.display())]
    OutputDirectoryMissing(PathBuf),
    #[error("the export output parent is not a directory: {}", .0.display())]
    OutputParentNotDirectory(PathBuf),
    #[error("the export output cannot be inspected: {path}: {source}")]
    OutputInspection {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("the export output is a directory: {}", .0.display())]
    OutputIsDirectory(PathBuf),
    #[error("refusing to export through a symbolic link: {}", .0.display())]
    OutputIsSymlink(PathBuf),
    #[error("the export output already exists; explicit overwrite permission is required: {}", .0.display())]
    OutputExists(PathBuf),
    #[error("delivery focal point ({x}, {y}) must stay inside 0..=100 percent")]
    InvalidFocus { x: u8, y: u8 },
    #[error("another queued or running export owns this output path: {}", .0.display())]
    OutputInUse(PathBuf),
    #[error("delivery profile could not be materialized: {0}")]
    InvalidDelivery(String),
    #[error("delivery conformance rejected the export")]
    Conformance(Box<DeliveryConformanceReport>),
    #[error("live source identity preflight rejected the export")]
    MediaPreflight(Box<ExportMediaPreflightReport>),
    /// A look a rendered frame could need is `missing`, `changed`, or
    /// `unreadable` in the project LUT store (CC4 §2.3).
    #[error("live LUT identity preflight rejected the export")]
    LutPreflight(Box<ExportLutPreflightReport>),
    /// The delivery document carries a LUT node that could evaluate, but the
    /// project has never been saved, so there is no store root to verify
    /// against (CC4 §2.2).
    #[error(
        "the project has no saved path, so its LUT store root cannot be derived; save the project before exporting a look"
    )]
    LutStoreNotSaved,
    /// The delivery document carries a LUT node that could evaluate and the
    /// project *is* saved, but the store root derived from its path is refused
    /// (CC4 §2.2).
    ///
    /// Distinct from [`Self::LutStoreNotSaved`] because the recovery is the
    /// opposite: there is nothing to save, and the typed refusal names what
    /// occupies the root instead.
    #[error("the project LUT store root cannot be used, so a look cannot be verified: {reason}")]
    LutStoreRootInvalid { reason: String },
    #[error("the export job id space is exhausted")]
    IdExhausted,
    #[error("export queue capacity must be greater than zero")]
    InvalidCapacity,
    #[error("the export queue already has its maximum of {capacity} pending jobs")]
    QueueFull { capacity: usize },
    #[error("the export queue worker could not start: {0}")]
    WorkerThread(#[source] std::io::Error),
    #[error("the export queue worker is unavailable")]
    WorkerUnavailable,
    #[error(
        "a source file changed between the export preflight and the finished encode, so the output cannot be trusted: {0}"
    )]
    SourceIdentityChanged(String),
}

#[derive(Clone)]
pub struct ExportQueue {
    state: Arc<QueueState>,
    work_tx: Sender<WorkItem>,
    capacity: usize,
}

struct QueueState {
    /// The saved project file path, shared with the MCP server. The CC4 LUT
    /// store root is derived from it on every use and never persisted.
    lut_project_path: Arc<RwLock<Option<PathBuf>>>,
    exporter: Arc<dyn Export>,
    analysis: Arc<dyn Analysis>,
    jobs: Mutex<BTreeMap<ExportJobId, StoredJob>>,
    next_id: AtomicU64,
}

struct StoredJob {
    record: ExportJobRecord,
    cancellation: ExportCancellation,
    output_key: String,
}

struct WorkItem {
    id: ExportJobId,
    document: Arc<Document>,
    output_path: PathBuf,
    profile: DeliveryProfile,
    overwrite: bool,
    /// CC6 §6.5: run the post-encode decoded comparison.
    verify: bool,
    /// CC6 §4.1: the delivery lane the settings are materialized at.
    depth: DeliveryEncodeDepth,
    cancellation: ExportCancellation,
    /// The live source identity observed when this job passed preflight.
    ///
    /// Verifying only before the encode is a time-of-check/time-of-use gap: a
    /// source can be replaced while the encode runs, producing an output whose
    /// contents do not match the fingerprints the job was admitted under.
    verified_sources: BTreeMap<AssetId, MediaSourceFingerprint>,
}

/// Run the export media preflight and snapshot the live source identity it
/// observed, from a single availability pass.
///
/// `export_media_preflight` already asks the backend for one
/// `media_availability` status per timeline-referenced source, and each status
/// carries the fingerprint the backend just hashed. Calling the preflight and
/// then a separate identity snapshot asked the backend to hash every source
/// twice for the same answer, so the pass is done once here and both results
/// are derived from it.
///
/// The observed fingerprint is preferred when the backend supplies one; the
/// persisted project fingerprint is the fallback so a backend that does not
/// re-hash still produces a stable, comparable value.
///
/// A core-level change that made `export_media_preflight` itself return the
/// observed fingerprints would let the worker's pre-encode preflight feed the
/// post-encode drift check too, removing the last redundant pass. That is a
/// `kinewright-core` API change and is out of scope here.
fn preflight_with_source_identities(
    document: &Document,
    analysis: &dyn Analysis,
) -> (
    ExportMediaPreflightReport,
    BTreeMap<AssetId, MediaSourceFingerprint>,
) {
    let mut checked_assets = Vec::new();
    let mut issues = Vec::new();
    let mut identities = BTreeMap::new();
    for asset in document.timeline_referenced_media_assets() {
        checked_assets.push(asset.id);
        let status = analysis.media_availability(asset);
        identities.insert(
            asset.id,
            status
                .observed_fingerprint
                .clone()
                .unwrap_or_else(|| asset.source_fingerprint.clone()),
        );
        if status.kind != MediaAvailabilityKind::OnlineVerified {
            issues.push(ExportMediaPreflightIssue {
                asset: asset.id,
                asset_name: asset.name.clone(),
                availability: status,
            });
        }
    }
    (
        ExportMediaPreflightReport {
            checked_assets,
            issues,
        },
        identities,
    )
}

/// Recheck every look a rendered frame could need against the project LUT
/// store, immediately before the export is admitted (CC4 §2.3).
///
/// Core supplies only the document-side half — which assets are referenced by
/// nodes that could evaluate on some frame — because it has no filesystem
/// concept. The store root is derived from the saved project path here and
/// injected, exactly as M41 injects media availability.
///
/// A document with no possibly-active LUT node needs no store at all, so an
/// unsaved project only blocks when a look would actually be rendered.
fn lut_preflight(
    document: &Document,
    lut_project_path: &RwLock<Option<PathBuf>>,
) -> Result<ExportLutPreflightReport, ExportQueueError> {
    let needs_store = document
        .tracks
        .iter()
        .flat_map(|track| &track.clips)
        .flat_map(|clip| &clip.effects)
        .any(lut_node_may_be_active);
    let project_path = lut_project_path.read().ok().and_then(|slot| slot.clone());
    let Some(project_path) = project_path else {
        if needs_store {
            return Err(ExportQueueError::LutStoreNotSaved);
        }
        return Ok(ExportLutPreflightReport {
            checked_lut_assets: Vec::new(),
            issues: Vec::new(),
        });
    };
    let store = match LutStore::for_project(&project_path) {
        Ok(store) => store,
        Err(error) => {
            if needs_store {
                // A path *is* published, so this is never `project_not_saved`:
                // the store's own typed refusal is the only honest reason
                // (CC4 §2.2). `MediaError` has no LUT variant, so the store
                // encodes its code behind `Backend`'s own label; the label is
                // dropped here so the typed `<code>: <detail>` survives intact.
                let rendered = error.to_string();
                return Err(ExportQueueError::LutStoreRootInvalid {
                    reason: rendered
                        .strip_prefix("media backend error: ")
                        .unwrap_or(rendered.as_str())
                        .to_owned(),
                });
            }
            return Ok(ExportLutPreflightReport {
                checked_lut_assets: Vec::new(),
                issues: Vec::new(),
            });
        }
    };
    Ok(export_lut_preflight_with(
        document,
        &store.availability_resolver(),
    ))
}

/// Return a description of the first source whose live identity no longer
/// matches the identity the job was admitted under, or `None` when every
/// source still matches.
///
/// What each half of the check actually guards:
///
/// * The `kind` test carries the check: a source that was replaced, truncated,
///   or unlinked while the encode ran no longer classifies as
///   `OnlineVerified`, because that kind means the backend just re-read the
///   file and matched the persisted fingerprint.
/// * The fingerprint comparison is a consistency assertion on top of it. It
///   catches a backend that reports `OnlineVerified` while handing back a
///   different observed fingerprint, and an asset that entered the timeline
///   after the job was admitted (`None` below), which the `kind` test alone
///   would accept.
fn source_identity_drift(
    document: &Document,
    analysis: &dyn Analysis,
    verified: &BTreeMap<AssetId, MediaSourceFingerprint>,
) -> Option<String> {
    for asset in document.timeline_referenced_media_assets() {
        let status = analysis.media_availability(asset);
        if status.kind != MediaAvailabilityKind::OnlineVerified {
            return Some(format!(
                "{} ({:?}): {}",
                asset.name,
                status.kind,
                status
                    .reason
                    .as_deref()
                    .unwrap_or("no backend reason was provided")
            ));
        }
        let observed = status
            .observed_fingerprint
            .unwrap_or_else(|| asset.source_fingerprint.clone());
        match verified.get(&asset.id) {
            Some(expected) if *expected == observed => {}
            Some(_) => {
                return Some(format!(
                    "{} no longer matches the fingerprint verified at preflight",
                    asset.name
                ));
            }
            None => {
                return Some(format!(
                    "{} was not part of the verified preflight set",
                    asset.name
                ));
            }
        }
    }
    None
}

impl ExportQueue {
    /// Start a bounded, serial export queue backed by one worker thread.
    ///
    /// # Errors
    ///
    /// Returns an error if the worker thread cannot be started.
    pub fn new(
        exporter: Arc<dyn Export>,
        analysis: Arc<dyn Analysis>,
    ) -> Result<Self, ExportQueueError> {
        Self::with_capacity(exporter, analysis, DEFAULT_EXPORT_QUEUE_CAPACITY)
    }

    /// Start a queue that resolves CC4 LUT availability against the project
    /// path published by the owning session.
    ///
    /// # Errors
    ///
    /// Returns an error if the worker thread cannot be started.
    pub fn with_lut_project_path(
        exporter: Arc<dyn Export>,
        analysis: Arc<dyn Analysis>,
        lut_project_path: Arc<RwLock<Option<PathBuf>>>,
    ) -> Result<Self, ExportQueueError> {
        Self::configured(
            exporter,
            analysis,
            DEFAULT_EXPORT_QUEUE_CAPACITY,
            lut_project_path,
        )
    }

    /// Start a serial export queue with an explicit maximum pending count.
    ///
    /// # Errors
    ///
    /// Returns an error for a zero capacity or if the worker thread cannot be
    /// started.
    pub fn with_capacity(
        exporter: Arc<dyn Export>,
        analysis: Arc<dyn Analysis>,
        capacity: usize,
    ) -> Result<Self, ExportQueueError> {
        Self::configured(exporter, analysis, capacity, Arc::new(RwLock::new(None)))
    }

    /// Start a serial export queue with an explicit capacity and LUT project
    /// path handle.
    ///
    /// # Errors
    ///
    /// Returns an error for a zero capacity or if the worker thread cannot be
    /// started.
    pub fn configured(
        exporter: Arc<dyn Export>,
        analysis: Arc<dyn Analysis>,
        capacity: usize,
        lut_project_path: Arc<RwLock<Option<PathBuf>>>,
    ) -> Result<Self, ExportQueueError> {
        if capacity == 0 {
            return Err(ExportQueueError::InvalidCapacity);
        }
        let (work_tx, work_rx) = crossbeam_channel::bounded(capacity);
        let state = Arc::new(QueueState {
            lut_project_path,
            exporter,
            analysis,
            jobs: Mutex::new(BTreeMap::new()),
            next_id: AtomicU64::new(1),
        });
        let worker_state = Arc::clone(&state);
        thread::Builder::new()
            .name("kinewright-export-queue".to_owned())
            .spawn(move || worker_loop(&worker_state, &work_rx))
            .map_err(ExportQueueError::WorkerThread)?;
        Ok(Self {
            state,
            work_tx,
            capacity,
        })
    }

    /// Capture and enqueue an immutable delivery document.
    ///
    /// Structural conformance errors reject the request before an id is
    /// allocated or any filesystem mutation can begin.
    ///
    /// # Errors
    ///
    /// Returns an error for an unsafe path, invalid delivery document,
    /// conformance failure, id exhaustion, or stopped worker.
    #[allow(clippy::too_many_lines)]
    pub fn enqueue(
        &self,
        document: &Document,
        request: QueueExportRequest,
    ) -> Result<ExportJobRecord, ExportQueueError> {
        let QueueExportRequest {
            output_path: requested_output,
            profile,
            focus_x_percent,
            focus_y_percent,
            overwrite,
            verify,
            delivery_bit_depth,
        } = request;
        if focus_x_percent > 100 || focus_y_percent > 100 {
            return Err(ExportQueueError::InvalidFocus {
                x: focus_x_percent,
                y: focus_y_percent,
            });
        }
        let required_extension = profile.container_extension();
        if requested_output
            .extension()
            .and_then(std::ffi::OsStr::to_str)
            .is_none_or(|extension| !extension.eq_ignore_ascii_case(required_extension))
        {
            return Err(ExportQueueError::InvalidOutputExtension {
                profile,
                required: required_extension,
                path: requested_output,
            });
        }
        let output_path = normalize_output_path(&requested_output, overwrite)?;
        let conformance = delivery_conformance(
            document,
            profile,
            delivery_bit_depth,
            focus_x_percent,
            focus_y_percent,
        )
        .map_err(|error| ExportQueueError::InvalidDelivery(error.to_string()))?;
        if !conformance.export_ready() {
            return Err(ExportQueueError::Conformance(Box::new(conformance)));
        }
        let delivery_document = Arc::new(
            document_for_delivery_profile(document, profile, focus_x_percent, focus_y_percent)
                .map_err(|error| ExportQueueError::InvalidDelivery(error.to_string()))?,
        );
        // One availability pass produces both the admission gate and the
        // identities the post-encode drift check compares against.
        let (media_preflight, verified_sources) =
            preflight_with_source_identities(&delivery_document, self.state.analysis.as_ref());
        if !media_preflight.export_ready() {
            return Err(ExportQueueError::MediaPreflight(Box::new(media_preflight)));
        }
        // CC4 §2.3: a look whose bytes are missing, changed, or unreadable
        // blocks delivery before the encode, rather than failing at render
        // time or silently dropping the node.
        let lut_preflight = lut_preflight(&delivery_document, &self.state.lut_project_path)?;
        if !lut_preflight.export_ready() {
            return Err(ExportQueueError::LutPreflight(Box::new(lut_preflight)));
        }
        let output_key = output_path_key(&output_path);
        let cancellation = ExportCancellation::default();
        let (id, record) = {
            let mut jobs = lock_jobs(&self.state);
            if jobs
                .values()
                .any(|job| !job.record.state.is_terminal() && job.output_key == output_key)
            {
                return Err(ExportQueueError::OutputInUse(output_path));
            }
            let id = ExportJobId(
                self.state
                    .next_id
                    .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                        value.checked_add(1)
                    })
                    .map_err(|_| ExportQueueError::IdExhausted)?,
            );
            let record = ExportJobRecord {
                id,
                output_path: output_path.clone(),
                profile,
                focus_x_percent,
                focus_y_percent,
                overwrite,
                state: ExportJobState::Queued,
                progress: ExportJobProgress::default(),
                conformance,
                delivery_bit_depth,
                error: None,
                verification: None,
                verification_unavailable_reason: None,
                verifying: false,
            };
            jobs.insert(
                id,
                StoredJob {
                    record: record.clone(),
                    cancellation: cancellation.clone(),
                    output_key,
                },
            );
            (id, record)
        };
        let work = WorkItem {
            id,
            document: delivery_document,
            output_path,
            profile,
            overwrite,
            verify,
            depth: delivery_bit_depth,
            cancellation,
            verified_sources,
        };
        if let Err(error) = self.work_tx.try_send(work) {
            lock_jobs(&self.state).remove(&id);
            return Err(match error {
                crossbeam_channel::TrySendError::Full(_) => ExportQueueError::QueueFull {
                    capacity: self.capacity,
                },
                crossbeam_channel::TrySendError::Disconnected(_) => {
                    ExportQueueError::WorkerUnavailable
                }
            });
        }
        Ok(record)
    }

    #[must_use]
    pub fn get(&self, id: ExportJobId) -> Option<ExportJobRecord> {
        lock_jobs(&self.state)
            .get(&id)
            .map(|job| job.record.clone())
    }

    /// Return every retained job in ascending id/enqueue order.
    #[must_use]
    pub fn list(&self) -> Vec<ExportJobRecord> {
        lock_jobs(&self.state)
            .values()
            .map(|job| job.record.clone())
            .collect()
    }

    /// Idempotently cancel a queued or running export.
    ///
    /// The status becomes `cancelled` immediately. A running backend may take
    /// a short time to observe its cancellation token and release resources.
    #[must_use]
    pub fn cancel(&self, id: ExportJobId) -> Option<ExportJobRecord> {
        let mut jobs = lock_jobs(&self.state);
        let job = jobs.get_mut(&id)?;
        if !job.record.state.is_terminal() {
            job.cancellation.cancel();
            job.record.state = ExportJobState::Cancelled;
            job.record.error = None;
        }
        Some(job.record.clone())
    }
}

fn worker_loop(state: &Arc<QueueState>, work_rx: &Receiver<WorkItem>) {
    while let Ok(work) = work_rx.recv() {
        run_work_item(state, work);
    }
}

fn run_work_item(state: &Arc<QueueState>, work: WorkItem) {
    if work.cancellation.is_cancelled() || !mark_running(state, work.id) {
        // Nothing was encoded, so there is no file a verification could have
        // measured and no reason to record about one.
        mark_cancelled(state, work.id, None);
        return;
    }
    let media_preflight = export_media_preflight(&work.document, state.analysis.as_ref());
    if !media_preflight.export_ready() {
        mark_failed(state, work.id, media_preflight.summary());
        return;
    }
    match lut_preflight(&work.document, &state.lut_project_path) {
        Ok(report) if !report.export_ready() => {
            mark_failed(state, work.id, report.summary());
            return;
        }
        Err(error) => {
            mark_failed(state, work.id, error.to_string());
            return;
        }
        Ok(_) => {}
    }
    if let Err(error) = validate_worker_output(&work.output_path, work.overwrite) {
        mark_failed(state, work.id, error.to_string());
        return;
    }

    let settings =
        work.profile
            .export_settings(&work.document, work.depth, work.cancellation.clone());
    // CC6 §6.5: the verification compares the written file against the exact
    // settings the encode ran under, so the settings are captured here rather
    // than re-materialized afterwards.
    let verification_settings = settings.clone();
    let (progress_tx, progress_rx) = crossbeam_channel::unbounded();
    let (stop_tx, stop_rx) = crossbeam_channel::bounded(1);
    let progress_state = Arc::clone(state);
    let progress_id = work.id;
    let progress_thread = thread::Builder::new()
        .name(format!("kinewright-export-progress-{}", work.id.0))
        .spawn(move || monitor_progress(&progress_state, progress_id, &progress_rx, &stop_rx));

    let document = Arc::clone(&work.document);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        state
            .exporter
            .export_document(work.document, &work.output_path, settings, progress_tx)
    }));
    let _ = stop_tx.send(());
    if let Ok(progress_thread) = progress_thread {
        let _ = progress_thread.join();
    }

    if work.cancellation.is_cancelled() {
        // CC6 §6.5: the state stays `Cancelled`. The operator said stop, and a
        // job whose cancellation was observed here is not one the queue may
        // report as completed. But a job that asked to be verified and was not
        // has to say so: skipping the measurement is the one thing cancellation
        // can still honour once the file is written, and an absent
        // `verification` with no reason beside it is the shape a caller reads
        // as "nothing to report".
        mark_cancelled(
            state,
            work.id,
            work.verify
                .then(|| EXPORT_CANCELLED_BEFORE_VERIFICATION.to_owned()),
        );
        return;
    }
    match result {
        // Close the time-of-check/time-of-use gap: a source that was swapped
        // while the encode ran would otherwise produce a job reported as
        // completed against fingerprints the output does not actually contain.
        Ok(Ok(())) => {
            match source_identity_drift(&document, state.analysis.as_ref(), &work.verified_sources)
            {
                None => {
                    // CC6 §6.5: verification runs only after the encode
                    // succeeded *and* the source identity re-check passed, so
                    // it never measures an output the queue has already
                    // refused to vouch for.
                    // CC6 §6.5: the encode is finished and the frame counter
                    // has stopped moving, so say which wait this is rather
                    // than leaving a caller to guess from a stalled Running.
                    set_verifying(state, work.id, work.verify);
                    let outcome = verify_output(
                        state,
                        work.verify,
                        work.depth,
                        &work.output_path,
                        Arc::clone(&document),
                        &verification_settings,
                    );
                    mark_completed(state, work.id, outcome);
                }
                Some(reason) => {
                    let quarantine = quarantine_untrusted_output(&work.output_path);
                    mark_failed(
                        state,
                        work.id,
                        format!(
                            "{}; {quarantine}",
                            ExportQueueError::SourceIdentityChanged(reason)
                        ),
                    );
                }
            }
        }
        // The exporter itself stopped: no complete file was written, so there
        // is no skipped measurement to explain.
        Ok(Err(MediaError::Cancelled)) => mark_cancelled(state, work.id, None),
        Ok(Err(error)) => mark_failed(state, work.id, error.to_string()),
        Err(payload) => mark_failed(
            state,
            work.id,
            format!(
                "export backend panicked: {}",
                panic_message(payload.as_ref())
            ),
        ),
    }
}

fn monitor_progress(
    state: &Arc<QueueState>,
    id: ExportJobId,
    progress_rx: &Receiver<ExportProgress>,
    stop_rx: &Receiver<()>,
) {
    loop {
        crossbeam_channel::select! {
            recv(progress_rx) -> progress => match progress {
                Ok(progress) => set_progress(state, id, &progress),
                Err(_) => return,
            },
            recv(stop_rx) -> _ => {
                for progress in progress_rx.try_iter() {
                    set_progress(state, id, &progress);
                }
                return;
            },
        }
    }
}

fn mark_running(state: &Arc<QueueState>, id: ExportJobId) -> bool {
    let mut jobs = lock_jobs(state);
    let Some(job) = jobs.get_mut(&id) else {
        return false;
    };
    if job.record.state != ExportJobState::Queued || job.cancellation.is_cancelled() {
        return false;
    }
    job.record.state = ExportJobState::Running;
    true
}

/// Move an untrusted encode out of the path the caller asked for.
///
/// A drift-failed job has already written a complete-looking file at the
/// requested output. Leaving it there means the next thing to read that path —
/// a person, a script, a re-queued job that reuses the name — picks up a file
/// the queue has explicitly refused to vouch for. It is renamed to
/// `<output>.untrusted`, or deleted when the rename is impossible, and the
/// final location is reported in the failure so nothing has to be guessed.
fn quarantine_untrusted_output(output_path: &Path) -> String {
    if !output_path.exists() {
        return "the encode left no output file to quarantine".to_owned();
    }
    let mut quarantined = output_path.as_os_str().to_owned();
    quarantined.push(".untrusted");
    let quarantined = PathBuf::from(quarantined);
    match fs::rename(output_path, &quarantined) {
        Ok(()) => format!(
            "the untrusted output was moved to {}",
            quarantined.display()
        ),
        Err(rename_error) => match fs::remove_file(output_path) {
            Ok(()) => format!(
                "the untrusted output at {} could not be renamed ({rename_error}) and was deleted",
                output_path.display()
            ),
            Err(remove_error) => format!(
                "the untrusted output at {} could not be renamed ({rename_error}) or deleted ({remove_error}); do not use it",
                output_path.display()
            ),
        },
    }
}

/// What one job's post-encode verification produced (CC6 §6.5).
///
/// Three outcomes, never two: "not asked for" and "asked for and could not
/// run" are different facts, and collapsing them would let an unavailable
/// verification read as a job that opted out.
#[derive(Debug)]
enum VerificationOutcome {
    /// The job ran with `verify: false`.
    NotRequested,
    /// A decoded comparison was produced. It may itself have failed its
    /// budgets or its tag check; that is reported, never acted on.
    Measured(Box<DeliveryVerification>),
    /// A verification was asked for and could not run at all.
    Unavailable(String),
}

/// Decode the finished encode and compare it against a fresh delivery
/// reference (CC6 §6.1/§6.5).
///
/// **This is a measurement.** It never moves, renames, deletes, or quarantines
/// the file it read, for any outcome: `quarantine_untrusted_output` belongs to
/// the source-identity path and stays there. A false positive in a measurement
/// must not be able to move a good file.
fn verify_output(
    state: &Arc<QueueState>,
    verify: bool,
    depth: DeliveryEncodeDepth,
    output_path: &Path,
    document: Arc<Document>,
    settings: &ExportSettings,
) -> VerificationOutcome {
    if !verify {
        return VerificationOutcome::NotRequested;
    }
    let request = DeliveryVerificationRequest {
        frame_count: DELIVERY_VERIFICATION_FRAME_COUNT,
        budgets: DeliveryBudgets::for_depth(depth),
        expected_delivery: settings.delivery_color.clone(),
    };
    // A backend that panics while verifying must not take down the worker or
    // destroy a finished encode either.
    let measured = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        state
            .analysis
            .verify_delivery_output(document, output_path, settings, request)
    }));
    match measured {
        Ok(Ok(verification)) => VerificationOutcome::Measured(Box::new(verification)),
        Ok(Err(error)) => VerificationOutcome::Unavailable(error.to_string()),
        Err(payload) => VerificationOutcome::Unavailable(format!(
            "delivery verification panicked: {}",
            panic_message(payload.as_ref())
        )),
    }
}

/// Complete a job and record its verification outcome.
///
/// **Normative (CC6 §6.5): verification never fails a job.** A budget overrun
/// and a tag mismatch both leave the job `Completed` with `error: None` and
/// `verification.technical_pass == false`; the encode succeeded and the file
/// is a valid deliverable, and failing the job over a measurement would
/// destroy work to report a number. A verification that could not run leaves
/// `verification: None` and records the reason rather than inventing a pass.
///
/// A job cancelled while its verification was in flight keeps its `Cancelled`
/// state and publishes no measurement — verification is not interruptible, so
/// the result arrives, and a measurement of an export the caller abandoned is
/// discarded rather than reported. The `verifying` flag is cleared either way,
/// because by the time this runs the verification really has stopped.
fn mark_completed(state: &Arc<QueueState>, id: ExportJobId, verification: VerificationOutcome) {
    let mut jobs = lock_jobs(state);
    let Some(job) = jobs.get_mut(&id) else {
        return;
    };
    job.record.verifying = false;
    if job.record.state != ExportJobState::Cancelled {
        job.record.state = ExportJobState::Completed;
        job.record.error = None;
        match verification {
            VerificationOutcome::NotRequested => {
                job.record.verification = None;
                job.record.verification_unavailable_reason = None;
            }
            VerificationOutcome::Measured(measured) => {
                job.record.verification = Some(*measured);
                job.record.verification_unavailable_reason = None;
            }
            VerificationOutcome::Unavailable(reason) => {
                job.record.verification = None;
                job.record.verification_unavailable_reason = Some(reason);
            }
        }
        if job.record.progress.total_frames > 0 {
            job.record.progress.completed_frames = job.record.progress.total_frames;
        }
    }
}

/// Publish whether this job's post-encode verification is running right now.
fn set_verifying(state: &Arc<QueueState>, id: ExportJobId, verifying: bool) {
    if let Some(job) = lock_jobs(state).get_mut(&id) {
        job.record.verifying = verifying;
    }
}

fn mark_failed(state: &Arc<QueueState>, id: ExportJobId, error: String) {
    if let Some(job) = lock_jobs(state).get_mut(&id)
        && job.record.state != ExportJobState::Cancelled
    {
        job.record.state = ExportJobState::Failed;
        job.record.error = Some(error);
    }
}

/// Settle a job as cancelled.
///
/// `verification_unavailable_reason` is `Some` only when the encode had already
/// finished and a verification the job asked for was skipped because of the
/// cancellation: the file exists, nothing measured it, and the record says so
/// rather than presenting an absent measurement a caller could read as a pass.
/// A job cancelled before or during its encode wrote no file to verify, so it
/// carries no such reason.
fn mark_cancelled(
    state: &Arc<QueueState>,
    id: ExportJobId,
    verification_unavailable_reason: Option<String>,
) {
    if let Some(job) = lock_jobs(state).get_mut(&id) {
        job.cancellation.cancel();
        job.record.state = ExportJobState::Cancelled;
        job.record.error = None;
        job.record.verification = None;
        job.record.verification_unavailable_reason = verification_unavailable_reason;
    }
}

fn set_progress(state: &Arc<QueueState>, id: ExportJobId, progress: &ExportProgress) {
    if let Some(job) = lock_jobs(state).get_mut(&id) {
        job.record.progress = ExportJobProgress {
            completed_frames: progress.completed_frames.min(progress.total_frames),
            total_frames: progress.total_frames,
        };
    }
}

fn lock_jobs(state: &QueueState) -> MutexGuard<'_, BTreeMap<ExportJobId, StoredJob>> {
    state
        .jobs
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn normalize_output_path(path: &Path, overwrite: bool) -> Result<PathBuf, ExportQueueError> {
    if path.as_os_str().is_empty() || path.file_name().is_none() {
        return Err(ExportQueueError::InvalidOutputPath);
    }
    let absolute = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()
            .map_err(|source| ExportQueueError::OutputInspection {
                path: path.to_owned(),
                source,
            })?
            .join(path)
    };
    let parent = absolute
        .parent()
        .ok_or(ExportQueueError::InvalidOutputPath)?;
    if !parent.exists() {
        return Err(ExportQueueError::OutputDirectoryMissing(parent.to_owned()));
    }
    if !parent.is_dir() {
        return Err(ExportQueueError::OutputParentNotDirectory(
            parent.to_owned(),
        ));
    }
    let canonical_parent =
        parent
            .canonicalize()
            .map_err(|source| ExportQueueError::OutputInspection {
                path: parent.to_owned(),
                source,
            })?;
    let output = canonical_parent.join(
        absolute
            .file_name()
            .ok_or(ExportQueueError::InvalidOutputPath)?,
    );
    inspect_existing_output(&output, overwrite)?;
    Ok(output)
}

fn validate_worker_output(path: &Path, overwrite: bool) -> Result<(), ExportQueueError> {
    let parent = path.parent().ok_or(ExportQueueError::InvalidOutputPath)?;
    if !parent.exists() {
        return Err(ExportQueueError::OutputDirectoryMissing(parent.to_owned()));
    }
    if !parent.is_dir() {
        return Err(ExportQueueError::OutputParentNotDirectory(
            parent.to_owned(),
        ));
    }
    inspect_existing_output(path, overwrite)
}

fn inspect_existing_output(path: &Path, overwrite: bool) -> Result<(), ExportQueueError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(source) => {
            return Err(ExportQueueError::OutputInspection {
                path: path.to_owned(),
                source,
            });
        }
    };
    if metadata.file_type().is_symlink() {
        return Err(ExportQueueError::OutputIsSymlink(path.to_owned()));
    }
    if metadata.is_dir() {
        return Err(ExportQueueError::OutputIsDirectory(path.to_owned()));
    }
    if !overwrite {
        return Err(ExportQueueError::OutputExists(path.to_owned()));
    }
    Ok(())
}

#[cfg(windows)]
fn output_path_key(path: &Path) -> String {
    path.to_string_lossy().to_lowercase()
}

#[cfg(not(windows))]
fn output_path_key(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn panic_message(payload: &(dyn Any + Send)) -> String {
    payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| payload.downcast_ref::<&str>().map(ToString::to_string))
        .unwrap_or_else(|| "unknown panic payload".to_owned())
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{
            Mutex,
            atomic::{AtomicUsize, Ordering},
        },
        time::{Duration, Instant},
    };

    use kinewright_core::{
        Analysis, AssetId, Clip, ClipContent, ColorContext, MediaAsset, MediaAvailabilityKind,
        MediaAvailabilityStatus, MediaError, MediaKind, MediaSourceFingerprint, Rational,
        RgbaImage, SilenceStatus, TimeCode, TimelineSceneChange, TimelineSilenceSpan,
        TimelineTranscriptWord, Title, Track, TrackId, TrackKind, TranscriptStatus,
        VisualAssetResult,
    };

    use super::*;

    /// What the CC6 verification test double returns.
    ///
    /// The three outcomes the queue must distinguish — a backend with no
    /// verification at all, a produced measurement, and a verification that
    /// could not run — plus the two failure shapes that are not returns at
    /// all: a backend that unwinds, and one that has not finished yet.
    #[derive(Default)]
    enum VerificationDouble {
        #[default]
        NotImplemented,
        Measured(Box<DeliveryVerification>),
        Refused(String),
        /// A backend that unwinds instead of returning. The worker must
        /// survive it, and the encode must survive it too.
        Panics,
        /// A backend that blocks until released, so a cancellation can be
        /// delivered while the verification is genuinely in flight.
        Blocking {
            entered: Sender<()>,
            release: Receiver<()>,
            verification: Box<DeliveryVerification>,
        },
    }

    #[derive(Default)]
    struct AvailabilityAnalysis {
        statuses: Mutex<BTreeMap<AssetId, MediaAvailabilityStatus>>,
        /// CC6 §6.5: the canned `verify_delivery_output` result.
        verification: Mutex<VerificationDouble>,
        /// Every verification call, so "was it called at all" is asserted
        /// rather than inferred from an absent field.
        verification_calls: Mutex<Vec<(PathBuf, DeliveryVerificationRequest, ExportSettings)>>,
    }

    impl AvailabilityAnalysis {
        fn with_statuses(
            statuses: impl IntoIterator<Item = (AssetId, MediaAvailabilityKind)>,
        ) -> Self {
            Self {
                statuses: Mutex::new(
                    statuses
                        .into_iter()
                        .map(|(asset, kind)| (asset, Self::status(kind)))
                        .collect(),
                ),
                ..Self::default()
            }
        }

        fn with_verification(double: VerificationDouble) -> Self {
            Self {
                verification: Mutex::new(double),
                ..Self::default()
            }
        }

        fn verification_calls(
            &self,
        ) -> Vec<(PathBuf, DeliveryVerificationRequest, ExportSettings)> {
            self.verification_calls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }

        fn status(kind: MediaAvailabilityKind) -> MediaAvailabilityStatus {
            MediaAvailabilityStatus {
                kind,
                observed_fingerprint: None,
                reason: Some("test availability".to_owned()),
            }
        }

        fn set_status(&self, asset: AssetId, kind: MediaAvailabilityKind) {
            self.statuses
                .lock()
                .unwrap()
                .insert(asset, Self::status(kind));
        }

        /// Seed the live fingerprint the backend reports for one source.
        fn set_observed_fingerprint(&self, asset: AssetId, content_sha256: &str) {
            self.statuses.lock().unwrap().insert(
                asset,
                MediaAvailabilityStatus {
                    kind: MediaAvailabilityKind::OnlineVerified,
                    observed_fingerprint: Some(MediaSourceFingerprint {
                        content_sha256: Some(content_sha256.to_owned()),
                        byte_len: Some(u64::try_from(content_sha256.len()).unwrap()),
                    }),
                    reason: Some("test availability".to_owned()),
                },
            );
        }
    }

    impl Analysis for AvailabilityAnalysis {
        fn probe(&self, _path: &Path) -> Result<MediaAsset, MediaError> {
            Err(MediaError::NotImplemented)
        }

        fn verify_delivery_output(
            &self,
            _document: Arc<Document>,
            path: &Path,
            settings: &ExportSettings,
            request: DeliveryVerificationRequest,
        ) -> Result<DeliveryVerification, MediaError> {
            self.verification_calls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push((path.to_owned(), request, settings.clone()));
            match &*self
                .verification
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
            {
                VerificationDouble::NotImplemented => Err(MediaError::NotImplemented),
                VerificationDouble::Measured(verification) => Ok((**verification).clone()),
                VerificationDouble::Refused(reason) => Err(MediaError::Backend(reason.clone())),
                VerificationDouble::Panics => {
                    panic!("this verification backend unwinds instead of returning")
                }
                VerificationDouble::Blocking {
                    entered,
                    release,
                    verification,
                } => {
                    entered.send(()).expect("the test observes the entry");
                    release
                        .recv_timeout(Duration::from_secs(5))
                        .expect("the test releases the verification");
                    Ok((**verification).clone())
                }
            }
        }

        fn media_availability(&self, asset: &MediaAsset) -> MediaAvailabilityStatus {
            self.statuses
                .lock()
                .unwrap()
                .get(&asset.id)
                .cloned()
                .unwrap_or(MediaAvailabilityStatus {
                    kind: MediaAvailabilityKind::OnlineUnverified,
                    observed_fingerprint: None,
                    reason: Some("test status was not explicitly seeded".to_owned()),
                })
        }

        fn thumbnail_at(&self, _t: TimeCode, _max_w: u32) -> Result<RgbaImage, MediaError> {
            Err(MediaError::NotImplemented)
        }

        fn request_transcription(&self, _asset: MediaAsset) {}

        fn transcript_status(&self, _asset: &MediaAsset) -> TranscriptStatus {
            TranscriptStatus::NotRequested
        }

        fn timeline_transcript(
            &self,
            _document: &Document,
            _range: Option<std::ops::Range<TimeCode>>,
        ) -> Result<Vec<TimelineTranscriptWord>, MediaError> {
            Ok(Vec::new())
        }

        fn request_silence_detection(&self, _asset: MediaAsset) {}

        fn silence_status(&self, _asset: &MediaAsset) -> SilenceStatus {
            SilenceStatus::NotRequested
        }

        fn timeline_silences(
            &self,
            _document: &Document,
            _range: Option<std::ops::Range<TimeCode>>,
            _minimum_source_frames: TimeCode,
        ) -> Result<Vec<TimelineSilenceSpan>, MediaError> {
            Ok(Vec::new())
        }

        fn request_scene_detection(&self, _asset: MediaAsset) {}

        fn scene_status(&self, _asset: &MediaAsset) -> kinewright_core::SceneStatus {
            kinewright_core::SceneStatus::NotRequested
        }

        fn timeline_scene_changes(
            &self,
            _document: &Document,
            _range: Option<std::ops::Range<TimeCode>>,
            _minimum_confidence_basis_points: u16,
        ) -> Result<Vec<TimelineSceneChange>, MediaError> {
            Ok(Vec::new())
        }

        fn request_waveform(&self, _asset: MediaAsset, _request_generation: u64) -> bool {
            false
        }

        fn request_thumbnail(
            &self,
            _asset: MediaAsset,
            _source_at: TimeCode,
            _max_width: u32,
            _request_generation: u64,
        ) -> bool {
            false
        }

        fn visual_asset_results(&self) -> Receiver<VisualAssetResult> {
            crossbeam_channel::never()
        }
    }

    fn fail_closed_analysis() -> Arc<dyn Analysis> {
        Arc::new(AvailabilityAnalysis::default())
    }

    fn queue(exporter: Arc<dyn Export>) -> ExportQueue {
        ExportQueue::new(exporter, fail_closed_analysis()).unwrap()
    }

    struct RecordingExporter {
        calls: Mutex<Vec<(i64, PathBuf)>>,
        active: AtomicUsize,
        maximum_active: AtomicUsize,
        delay: Duration,
    }

    impl RecordingExporter {
        fn new(delay: Duration) -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                active: AtomicUsize::new(0),
                maximum_active: AtomicUsize::new(0),
                delay,
            }
        }
    }

    impl Export for RecordingExporter {
        fn export(
            &self,
            _out: &Path,
            _settings: kinewright_core::ExportSettings,
            _progress: kinewright_core::ProgressSink,
        ) -> Result<(), MediaError> {
            unreachable!("tests exercise immutable document export")
        }

        fn export_document(
            &self,
            document: Arc<Document>,
            out: &Path,
            _settings: kinewright_core::ExportSettings,
            progress: kinewright_core::ProgressSink,
        ) -> Result<(), MediaError> {
            let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.maximum_active.fetch_max(active, Ordering::SeqCst);
            self.calls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push((document.duration.0, out.to_owned()));
            let _ = progress.send(ExportProgress {
                completed_frames: 1,
                total_frames: 2,
            });
            thread::sleep(self.delay);
            let _ = progress.send(ExportProgress {
                completed_frames: 2,
                total_frames: 2,
            });
            self.active.fetch_sub(1, Ordering::SeqCst);
            Ok(())
        }
    }

    struct SequencedExporter {
        call: AtomicUsize,
    }

    impl Export for SequencedExporter {
        fn export(
            &self,
            _out: &Path,
            _settings: kinewright_core::ExportSettings,
            _progress: kinewright_core::ProgressSink,
        ) -> Result<(), MediaError> {
            unreachable!("tests exercise immutable document export")
        }

        fn export_document(
            &self,
            _document: Arc<Document>,
            _out: &Path,
            _settings: kinewright_core::ExportSettings,
            _progress: kinewright_core::ProgressSink,
        ) -> Result<(), MediaError> {
            match self.call.fetch_add(1, Ordering::SeqCst) {
                0 => panic!("deliberate exporter panic"),
                1 => Err(MediaError::Backend("deliberate failure".to_owned())),
                _ => Ok(()),
            }
        }
    }

    struct BlockingExporter {
        calls: AtomicUsize,
        started_tx: Sender<()>,
        release_rx: Receiver<()>,
    }

    impl Export for BlockingExporter {
        fn export(
            &self,
            _out: &Path,
            _settings: kinewright_core::ExportSettings,
            _progress: kinewright_core::ProgressSink,
        ) -> Result<(), MediaError> {
            unreachable!("tests exercise immutable document export")
        }

        fn export_document(
            &self,
            _document: Arc<Document>,
            _out: &Path,
            _settings: kinewright_core::ExportSettings,
            _progress: kinewright_core::ProgressSink,
        ) -> Result<(), MediaError> {
            if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
                let _ = self.started_tx.send(());
                let _ = self.release_rx.recv();
            }
            Ok(())
        }
    }

    struct CancellableExporter {
        started_tx: Sender<()>,
        finished_tx: Sender<()>,
    }

    impl Export for CancellableExporter {
        fn export(
            &self,
            _out: &Path,
            _settings: kinewright_core::ExportSettings,
            _progress: kinewright_core::ProgressSink,
        ) -> Result<(), MediaError> {
            unreachable!("tests exercise immutable document export")
        }

        fn export_document(
            &self,
            _document: Arc<Document>,
            _out: &Path,
            settings: kinewright_core::ExportSettings,
            _progress: kinewright_core::ProgressSink,
        ) -> Result<(), MediaError> {
            let _ = self.started_tx.send(());
            while !settings.cancellation.is_cancelled() {
                thread::sleep(Duration::from_millis(2));
            }
            let _ = self.finished_tx.send(());
            Err(MediaError::Cancelled)
        }
    }

    #[test]
    fn queue_uses_immutable_snapshots_and_executes_serially() {
        let exporter = Arc::new(RecordingExporter::new(Duration::from_millis(30)));
        let queue = queue(exporter.clone());
        let directory = test_directory("serial");
        let mut first_document = renderable_document(10);
        let first = queue
            .enqueue(&first_document, request(directory.join("first.mp4"), false))
            .unwrap();
        first_document.duration = TimeCode(99);
        let second = queue
            .enqueue(
                &renderable_document(20),
                request(directory.join("second.mp4"), false),
            )
            .unwrap();

        let first = wait_for_terminal(&queue, first.id);
        let second = wait_for_terminal(&queue, second.id);
        assert_eq!(first.state, ExportJobState::Completed);
        assert_eq!(second.state, ExportJobState::Completed);
        assert_eq!(first.progress.completed_frames, 2);
        assert_eq!(exporter.maximum_active.load(Ordering::SeqCst), 1);
        let durations = exporter
            .calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .map(|(duration, _)| *duration)
            .collect::<Vec<_>>();
        assert_eq!(durations, [10, 20]);
        assert_eq!(
            queue.list().iter().map(|job| job.id).collect::<Vec<_>>(),
            [first.id, second.id]
        );
        cleanup_directory(&directory);
    }

    #[test]
    fn queued_cancellation_skips_the_exporter() {
        let (started_tx, started_rx) = crossbeam_channel::bounded(1);
        let (release_tx, release_rx) = crossbeam_channel::bounded(1);
        let exporter = Arc::new(BlockingExporter {
            calls: AtomicUsize::new(0),
            started_tx,
            release_rx,
        });
        let queue = queue(exporter.clone());
        let directory = test_directory("cancel");
        let first = queue
            .enqueue(
                &renderable_document(10),
                request(directory.join("first.mp4"), false),
            )
            .unwrap();
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        let second = queue
            .enqueue(
                &renderable_document(10),
                request(directory.join("second.mp4"), false),
            )
            .unwrap();
        assert_eq!(
            queue.cancel(second.id).unwrap().state,
            ExportJobState::Cancelled
        );
        release_tx.send(()).unwrap();

        assert_eq!(
            wait_for_terminal(&queue, first.id).state,
            ExportJobState::Completed
        );
        assert_eq!(
            wait_for_terminal(&queue, second.id).state,
            ExportJobState::Cancelled
        );
        assert_eq!(exporter.calls.load(Ordering::SeqCst), 1);
        cleanup_directory(&directory);
    }

    #[test]
    fn running_cancellation_reaches_the_backend_token() {
        let (started_tx, started_rx) = crossbeam_channel::bounded(1);
        let (finished_tx, finished_rx) = crossbeam_channel::bounded(1);
        let queue = queue(Arc::new(CancellableExporter {
            started_tx,
            finished_tx,
        }));
        let directory = test_directory("running-cancel");
        let job = queue
            .enqueue(
                &renderable_document(10),
                request(directory.join("cancel.mp4"), false),
            )
            .unwrap();
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();

        assert_eq!(
            queue.cancel(job.id).unwrap().state,
            ExportJobState::Cancelled
        );
        finished_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("backend must observe cancellation token");
        assert_eq!(
            wait_for_terminal(&queue, job.id).state,
            ExportJobState::Cancelled
        );
        cleanup_directory(&directory);
    }

    #[test]
    fn backend_failures_and_panics_do_not_stop_later_jobs() {
        let exporter = Arc::new(SequencedExporter {
            call: AtomicUsize::new(0),
        });
        let queue = queue(exporter);
        let directory = test_directory("recovery");
        let jobs = ["panic.mp4", "failure.mp4", "success.mp4"].map(|name| {
            queue
                .enqueue(
                    &renderable_document(10),
                    request(directory.join(name), false),
                )
                .unwrap()
        });

        let first = wait_for_terminal(&queue, jobs[0].id);
        let second = wait_for_terminal(&queue, jobs[1].id);
        let third = wait_for_terminal(&queue, jobs[2].id);
        assert_eq!(first.state, ExportJobState::Failed);
        assert!(first.error.unwrap().contains("panicked"));
        assert_eq!(second.state, ExportJobState::Failed);
        assert!(second.error.unwrap().contains("deliberate failure"));
        assert_eq!(third.state, ExportJobState::Completed);
        cleanup_directory(&directory);
    }

    #[test]
    fn conformance_and_output_guards_reject_unsafe_requests() {
        let exporter = Arc::new(RecordingExporter::new(Duration::ZERO));
        let queue = queue(exporter);
        let directory = test_directory("guards");
        let wrong_container = queue.enqueue(
            &renderable_document(10),
            request(directory.join("delivery.mov"), false),
        );
        assert!(matches!(
            wrong_container,
            Err(ExportQueueError::InvalidOutputExtension {
                required: "mp4",
                ..
            })
        ));
        let empty = queue.enqueue(
            &Document::default(),
            request(directory.join("empty.mp4"), false),
        );
        assert!(matches!(empty, Err(ExportQueueError::Conformance(_))));

        let existing = directory.join("existing.mp4");
        fs::write(&existing, b"user data").unwrap();
        let no_overwrite =
            queue.enqueue(&renderable_document(10), request(existing.clone(), false));
        assert!(
            matches!(no_overwrite, Err(ExportQueueError::OutputExists(path)) if path == existing.canonicalize().unwrap())
        );

        let overwrite = queue
            .enqueue(&renderable_document(10), request(existing.clone(), true))
            .unwrap();
        assert_eq!(
            wait_for_terminal(&queue, overwrite.id).state,
            ExportJobState::Completed
        );
        cleanup_directory(&directory);
    }

    #[test]
    fn queue_blocks_a_changed_same_path_source_before_allocating_a_job() {
        let analysis = Arc::new(AvailabilityAnalysis::with_statuses([
            (AssetId(1), MediaAvailabilityKind::Changed),
            (AssetId(2), MediaAvailabilityKind::OnlineVerified),
        ]));
        let queue =
            ExportQueue::new(Arc::new(RecordingExporter::new(Duration::ZERO)), analysis).unwrap();
        let directory = test_directory("changed-source");

        let result = queue.enqueue(
            &media_document_with_video_audio_and_unused(),
            request(directory.join("changed.mp4"), false),
        );

        assert!(matches!(
            result,
            Err(ExportQueueError::MediaPreflight(report))
                if report.issues.len() == 1
                    && report.issues[0].asset == AssetId(1)
                    && report.issues[0].availability.kind == MediaAvailabilityKind::Changed
        ));
        assert!(queue.list().is_empty());
        cleanup_directory(&directory);
    }

    #[test]
    fn queue_blocks_legacy_online_unverified_sources_until_relinked() {
        let analysis = Arc::new(AvailabilityAnalysis::with_statuses([
            (AssetId(1), MediaAvailabilityKind::OnlineUnverified),
            (AssetId(2), MediaAvailabilityKind::OnlineVerified),
        ]));
        let queue =
            ExportQueue::new(Arc::new(RecordingExporter::new(Duration::ZERO)), analysis).unwrap();
        let directory = test_directory("legacy-unverified");

        let result = queue.enqueue(
            &media_document_with_video_audio_and_unused(),
            request(directory.join("legacy.mp4"), false),
        );

        assert!(matches!(
            result,
            Err(ExportQueueError::MediaPreflight(report))
                if report.issues.len() == 1
                    && report.issues[0].availability.kind == MediaAvailabilityKind::OnlineUnverified
        ));
        cleanup_directory(&directory);
    }

    #[test]
    fn queue_ignores_unused_offline_media_but_blocks_referenced_video_and_audio() {
        let exporter = Arc::new(RecordingExporter::new(Duration::ZERO));
        let unused_only = Arc::new(AvailabilityAnalysis::with_statuses([
            (AssetId(1), MediaAvailabilityKind::OnlineVerified),
            (AssetId(2), MediaAvailabilityKind::OnlineVerified),
            (AssetId(3), MediaAvailabilityKind::OfflineMissing),
        ]));
        let queue = ExportQueue::new(exporter.clone(), unused_only).unwrap();
        let directory = test_directory("referenced-scope");
        let accepted = queue
            .enqueue(
                &media_document_with_video_audio_and_unused(),
                request(directory.join("unused-offline.mp4"), false),
            )
            .unwrap();
        assert_eq!(
            wait_for_terminal(&queue, accepted.id).state,
            ExportJobState::Completed
        );

        let blocking = Arc::new(AvailabilityAnalysis::with_statuses([
            (AssetId(1), MediaAvailabilityKind::OfflineMissing),
            (AssetId(2), MediaAvailabilityKind::Unreadable),
        ]));
        let queue =
            ExportQueue::new(Arc::new(RecordingExporter::new(Duration::ZERO)), blocking).unwrap();
        let result = queue.enqueue(
            &media_document_with_video_audio_and_unused(),
            request(directory.join("referenced-offline.mp4"), false),
        );
        assert!(matches!(
            result,
            Err(ExportQueueError::MediaPreflight(report))
                if report.issues.iter().map(|issue| issue.asset).collect::<Vec<_>>()
                    == vec![AssetId(1), AssetId(2)]
        ));
        cleanup_directory(&directory);
    }

    #[test]
    fn worker_rechecks_source_identity_when_a_queued_export_reaches_the_front() {
        let (started_tx, started_rx) = crossbeam_channel::bounded(1);
        let (release_tx, release_rx) = crossbeam_channel::bounded(1);
        let exporter = Arc::new(BlockingExporter {
            calls: AtomicUsize::new(0),
            started_tx,
            release_rx,
        });
        let analysis = Arc::new(AvailabilityAnalysis::with_statuses([
            (AssetId(1), MediaAvailabilityKind::OnlineVerified),
            (AssetId(2), MediaAvailabilityKind::OnlineVerified),
        ]));
        let queue = ExportQueue::new(exporter.clone(), analysis.clone()).unwrap();
        let directory = test_directory("source-race");
        let first = queue
            .enqueue(
                &renderable_document(30),
                request(directory.join("first.mp4"), false),
            )
            .unwrap();
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        let second = queue
            .enqueue(
                &media_document_with_video_audio_and_unused(),
                request(directory.join("second.mp4"), false),
            )
            .unwrap();
        analysis.set_status(AssetId(1), MediaAvailabilityKind::Changed);
        release_tx.send(()).unwrap();

        assert_eq!(
            wait_for_terminal(&queue, first.id).state,
            ExportJobState::Completed
        );
        let second = wait_for_terminal(&queue, second.id);
        assert_eq!(second.state, ExportJobState::Failed);
        assert!(second.error.unwrap().contains("Changed"));
        assert_eq!(exporter.calls.load(Ordering::SeqCst), 1);
        cleanup_directory(&directory);
    }

    #[test]
    fn active_jobs_reserve_their_normalized_output_path() {
        let (started_tx, started_rx) = crossbeam_channel::bounded(1);
        let (release_tx, release_rx) = crossbeam_channel::bounded(1);
        let exporter = Arc::new(BlockingExporter {
            calls: AtomicUsize::new(0),
            started_tx,
            release_rx,
        });
        let queue = queue(exporter);
        let directory = test_directory("reserve");
        let output = directory.join("same.mp4");
        let first = queue
            .enqueue(&renderable_document(10), request(output.clone(), false))
            .unwrap();
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        let duplicate = queue.enqueue(&renderable_document(10), request(output.clone(), false));
        assert!(matches!(duplicate, Err(ExportQueueError::OutputInUse(_))));
        release_tx.send(()).unwrap();
        assert_eq!(
            wait_for_terminal(&queue, first.id).state,
            ExportJobState::Completed
        );
        cleanup_directory(&directory);
    }

    #[test]
    fn worker_rechecks_a_destination_created_while_queued() {
        let (started_tx, started_rx) = crossbeam_channel::bounded(1);
        let (release_tx, release_rx) = crossbeam_channel::bounded(1);
        let exporter = Arc::new(BlockingExporter {
            calls: AtomicUsize::new(0),
            started_tx,
            release_rx,
        });
        let queue = queue(exporter.clone());
        let directory = test_directory("race");
        let first = queue
            .enqueue(
                &renderable_document(10),
                request(directory.join("first.mp4"), false),
            )
            .unwrap();
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        let raced_output = directory.join("raced.mp4");
        let second = queue
            .enqueue(
                &renderable_document(10),
                request(raced_output.clone(), false),
            )
            .unwrap();
        fs::write(&raced_output, b"created after enqueue").unwrap();
        release_tx.send(()).unwrap();

        assert_eq!(
            wait_for_terminal(&queue, first.id).state,
            ExportJobState::Completed
        );
        let second = wait_for_terminal(&queue, second.id);
        assert_eq!(second.state, ExportJobState::Failed);
        assert!(second.error.unwrap().contains("overwrite permission"));
        assert_eq!(exporter.calls.load(Ordering::SeqCst), 1);
        cleanup_directory(&directory);
    }

    #[test]
    fn pending_work_is_bounded_while_one_export_runs() {
        let (started_tx, started_rx) = crossbeam_channel::bounded(1);
        let (release_tx, release_rx) = crossbeam_channel::bounded(1);
        let exporter = Arc::new(BlockingExporter {
            calls: AtomicUsize::new(0),
            started_tx,
            release_rx,
        });
        let queue = ExportQueue::with_capacity(exporter, fail_closed_analysis(), 1).unwrap();
        let directory = test_directory("bounded");
        let first = queue
            .enqueue(
                &renderable_document(10),
                request(directory.join("first.mp4"), false),
            )
            .unwrap();
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        let second = queue
            .enqueue(
                &renderable_document(10),
                request(directory.join("second.mp4"), false),
            )
            .unwrap();
        let full = queue.enqueue(
            &renderable_document(10),
            request(directory.join("third.mp4"), false),
        );
        assert!(matches!(
            full,
            Err(ExportQueueError::QueueFull { capacity: 1 })
        ));
        release_tx.send(()).unwrap();
        assert_eq!(
            wait_for_terminal(&queue, first.id).state,
            ExportJobState::Completed
        );
        assert_eq!(
            wait_for_terminal(&queue, second.id).state,
            ExportJobState::Completed
        );
        cleanup_directory(&directory);
    }

    fn request(output_path: PathBuf, overwrite: bool) -> QueueExportRequest {
        QueueExportRequest {
            output_path,
            profile: DeliveryProfile::SourceMaster,
            focus_x_percent: 50,
            focus_y_percent: 50,
            overwrite,
            // The queue's pre-CC6 tests describe an encode, not a
            // verification, so they keep the default lane and opt out.
            verify: false,
            delivery_bit_depth: DeliveryEncodeDepth::Eight,
        }
    }

    fn renderable_document(duration: i64) -> Document {
        Document {
            tracks: vec![Track {
                id: TrackId(1),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: vec![Clip {
                    id: kinewright_core::ClipId(1),
                    asset: AssetId::default(),
                    source_range: TimeCode::ZERO..TimeCode(duration),
                    content: ClipContent::Title(Title {
                        text: "Export queue fixture".to_owned(),
                        ..Title::default()
                    }),
                    timeline_start: TimeCode::ZERO,
                    effects: Vec::new(),
                    transition_in: None,
                    link: None,
                    audio_gain_tenth_db: 0,
                    audio_fade_in_frames: TimeCode::ZERO,
                    audio_fade_out_frames: TimeCode::ZERO,
                    speed_percent: 100,
                }],
            }],
            duration: TimeCode(duration),
            ..Document::default()
        }
    }

    fn media_asset(id: u64, kind: MediaKind) -> MediaAsset {
        MediaAsset {
            id: AssetId(id),
            path: std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"),
            name: format!("source-{id}"),
            duration: TimeCode(30),
            fps: Rational::new(30, 1).unwrap(),
            kind,
            resolution: Some((1920, 1080)),
            source_fingerprint: MediaSourceFingerprint::unknown(),
            color_description: ColorContext::sdr_rec709().delivery,
        }
    }

    fn media_clip(id: u64) -> Clip {
        Clip {
            id: kinewright_core::ClipId(id),
            asset: AssetId(id),
            source_range: TimeCode::ZERO..TimeCode(30),
            content: ClipContent::Media,
            timeline_start: TimeCode::ZERO,
            effects: Vec::new(),
            transition_in: None,
            link: None,
            audio_gain_tenth_db: 0,
            audio_fade_in_frames: TimeCode::ZERO,
            audio_fade_out_frames: TimeCode::ZERO,
            speed_percent: 100,
        }
    }

    fn media_document_with_video_audio_and_unused() -> Document {
        Document {
            tracks: vec![
                Track {
                    id: TrackId(1),
                    kind: TrackKind::Video,
                    sync_lock: true,
                    clips: vec![media_clip(1)],
                },
                Track {
                    id: TrackId(2),
                    kind: TrackKind::Audio,
                    sync_lock: true,
                    clips: vec![media_clip(2)],
                },
            ],
            media_pool: vec![
                media_asset(1, MediaKind::Video),
                media_asset(2, MediaKind::Audio),
                media_asset(3, MediaKind::AudioVideo),
            ],
            duration: TimeCode(30),
            ..Document::default()
        }
    }

    fn wait_for_terminal(queue: &ExportQueue, id: ExportJobId) -> ExportJobRecord {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let job = queue.get(id).expect("queued job must remain inspectable");
            if job.state.is_terminal() {
                return job;
            }
            assert!(Instant::now() < deadline, "export job did not finish");
            thread::sleep(Duration::from_millis(5));
        }
    }

    fn test_directory(label: &str) -> PathBuf {
        static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(1);
        let path = std::env::temp_dir().join(format!(
            "kinewright-export-queue-{}-{label}-{}",
            std::process::id(),
            NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn cleanup_directory(path: &Path) {
        let _ = fs::remove_dir_all(path);
    }

    // ------------------------------------------------------------------
    // CC6 §6.5: post-export verification is a measurement, never a gate.
    // ------------------------------------------------------------------

    /// An exporter that leaves a complete-looking file behind, so "the output
    /// is still there, at its original path" is a real assertion.
    struct WritingExporter {
        bytes: &'static [u8],
    }

    impl Export for WritingExporter {
        fn export(
            &self,
            _out: &Path,
            _settings: ExportSettings,
            _progress: kinewright_core::ProgressSink,
        ) -> Result<(), MediaError> {
            unreachable!("tests exercise immutable document export")
        }

        fn export_document(
            &self,
            _document: Arc<Document>,
            out: &Path,
            _settings: ExportSettings,
            _progress: kinewright_core::ProgressSink,
        ) -> Result<(), MediaError> {
            fs::write(out, self.bytes).unwrap();
            Ok(())
        }
    }

    /// An encode that finishes, then observes the operator's Cancel.
    ///
    /// The race the queue has to survive: the file is complete on disk and the
    /// exporter returned `Ok`, but the cancellation flag is set by the time the
    /// worker reads it, so the verification never starts.
    struct CancellingExporter {
        bytes: &'static [u8],
    }

    impl Export for CancellingExporter {
        fn export(
            &self,
            _out: &Path,
            _settings: ExportSettings,
            _progress: kinewright_core::ProgressSink,
        ) -> Result<(), MediaError> {
            unreachable!("tests exercise immutable document export")
        }

        fn export_document(
            &self,
            _document: Arc<Document>,
            out: &Path,
            settings: ExportSettings,
            _progress: kinewright_core::ProgressSink,
        ) -> Result<(), MediaError> {
            fs::write(out, self.bytes).unwrap();
            settings.cancellation.cancel();
            Ok(())
        }
    }

    fn verified_request(
        output_path: PathBuf,
        verify: bool,
        delivery_bit_depth: DeliveryEncodeDepth,
    ) -> QueueExportRequest {
        QueueExportRequest {
            output_path,
            profile: DeliveryProfile::SourceMaster,
            focus_x_percent: 50,
            focus_y_percent: 50,
            overwrite: false,
            verify,
            delivery_bit_depth,
        }
    }

    fn channel_difference(maximum: u32) -> kinewright_core::DeliveryChannelDifference {
        kinewright_core::DeliveryChannelDifference {
            maximum_code_diff: maximum,
            p99_code_diff_millionths: 1_000_000,
            mean_code_diff_millionths: 250_000,
        }
    }

    fn plane_excursion() -> kinewright_core::PlaneLegalExcursion {
        kinewright_core::PlaneLegalExcursion {
            below_count: 0,
            above_count: 0,
            below_basis_points: 0,
            above_basis_points: 0,
            minimum_code_hundredths: 1_600,
            maximum_code_hundredths: 23_500,
        }
    }

    /// One canned verification. `technical_pass` and its exceptions are chosen
    /// by the caller, because the whole point of the outcome policy is that a
    /// failing measurement still completes the job.
    fn canned_verification(
        output_path: &Path,
        depth: DeliveryEncodeDepth,
        technical_pass: bool,
    ) -> DeliveryVerification {
        let expected = ColorContext::sdr_rec709().delivery;
        let exceptions = if technical_pass {
            Vec::new()
        } else {
            vec![kinewright_core::ColorQcException {
                code: "decoded_difference_over_budget".to_owned(),
                severity: kinewright_core::QaSeverity::Error,
                message: "the decoded luma plane exceeded its gated budget".to_owned(),
                field: Some("luma.maximum_code_diff".to_owned()),
                observed: Some("64".to_owned()),
                allowed: Some(DeliveryBudgets::for_depth(depth).luma_max_code.to_string()),
                clip: None,
                effect: None,
            }]
        };
        DeliveryVerification {
            output_path: output_path.to_owned(),
            delivery_bit_depth: depth,
            probed: expected.clone(),
            tags: kinewright_core::delivery_tag_check(
                &expected,
                &expected,
                kinewright_core::DeliveryTagSource::ProbedOutputFile,
            ),
            decoded_pixel_format: depth.pixel_format().to_owned(),
            comparison: kinewright_core::DeliveryComparison {
                frames: vec![0, 14, 29, 44, 59],
                luma: channel_difference(if technical_pass { 2 } else { 64 }),
                red: channel_difference(3),
                green: channel_difference(3),
                blue: channel_difference(3),
                combined: channel_difference(3),
                psnr_db_hundredths: Some(if technical_pass { 4_800 } else { 2_100 }),
                decoded_ycbcr: kinewright_core::YCbCrLegalReport {
                    bit_depth: depth.bits(),
                    luma: plane_excursion(),
                    cb: plane_excursion(),
                    cr: plane_excursion(),
                    source: kinewright_core::YCbCrLegalSource::DecodedNativePlanes,
                },
                rgb_extremes_note: kinewright_core::DELIVERY_RGB_EXTREMES_NOTE.to_owned(),
                budgets: DeliveryBudgets::for_depth(depth),
                within_budgets: technical_pass,
            },
            exceptions,
            technical_pass,
        }
    }

    fn verifying_queue(analysis: Arc<AvailabilityAnalysis>) -> ExportQueue {
        ExportQueue::new(
            Arc::new(WritingExporter {
                bytes: b"a complete-looking encode",
            }),
            analysis,
        )
        .unwrap()
    }

    /// CC6 §6.5: `verify: true` publishes the decoded comparison on the record,
    /// at the lane the job asked for, and the request the backend received
    /// carries that lane's own named budgets.
    #[test]
    fn cc6_a_verified_export_publishes_its_decoded_comparison_on_the_record() {
        let directory = test_directory("verify-pass");
        let output = directory.join("verified.mp4");
        let analysis = Arc::new(AvailabilityAnalysis::with_verification(
            VerificationDouble::Measured(Box::new(canned_verification(
                &output,
                DeliveryEncodeDepth::Ten,
                true,
            ))),
        ));
        let queue = verifying_queue(Arc::clone(&analysis));
        let job = queue
            .enqueue(
                &renderable_document(10),
                verified_request(output.clone(), true, DeliveryEncodeDepth::Ten),
            )
            .unwrap();
        assert_eq!(job.delivery_bit_depth, DeliveryEncodeDepth::Ten);

        let finished = wait_for_terminal(&queue, job.id);
        assert_eq!(finished.state, ExportJobState::Completed);
        assert_eq!(finished.error, None);
        let verification = finished
            .verification
            .expect("verify: true must publish the decoded comparison");
        assert!(verification.technical_pass);
        assert_eq!(verification.delivery_bit_depth, DeliveryEncodeDepth::Ten);
        assert_eq!(verification.decoded_pixel_format, "yuv420p10le");
        assert_eq!(finished.verification_unavailable_reason, None);

        // The backend was handed the default frame sample, the lane's own
        // budgets, and the settings' materialized delivery description - not a
        // set the queue invented.
        let calls = analysis.verification_calls();
        assert_eq!(calls.len(), 1);
        let (path, request, settings) = &calls[0];
        assert_eq!(path, &output);
        assert_eq!(request.frame_count, DELIVERY_VERIFICATION_FRAME_COUNT);
        assert_eq!(
            request.budgets,
            DeliveryBudgets::for_depth(DeliveryEncodeDepth::Ten)
        );
        assert_eq!(request.expected_delivery, settings.delivery_color);
        assert_eq!(
            settings.delivery_color.bit_depth,
            DeliveryEncodeDepth::Ten.color_bit_depth()
        );
        cleanup_directory(&directory);
    }

    /// CC6 §6.5, normative: a failing verification is a measurement, not a
    /// verdict. The job completes, `error` stays `None`, and the encode is
    /// still exactly where the caller asked for it.
    #[test]
    fn cc6_a_failing_verification_completes_the_job_and_leaves_the_output_alone() {
        let directory = test_directory("verify-fail");
        let output = directory.join("over-budget.mp4");
        let analysis = Arc::new(AvailabilityAnalysis::with_verification(
            VerificationDouble::Measured(Box::new(canned_verification(
                &output,
                DeliveryEncodeDepth::Eight,
                false,
            ))),
        ));
        let queue = verifying_queue(analysis);
        let job = queue
            .enqueue(
                &renderable_document(10),
                verified_request(output.clone(), true, DeliveryEncodeDepth::Eight),
            )
            .unwrap();

        let finished = wait_for_terminal(&queue, job.id);
        assert_eq!(finished.state, ExportJobState::Completed);
        assert_eq!(
            finished.error, None,
            "a measurement must never fail a finished encode"
        );
        let verification = finished.verification.expect("the measurement is published");
        assert!(!verification.technical_pass);
        assert!(
            verification
                .exceptions
                .iter()
                .any(|exception| exception.code == "decoded_difference_over_budget")
        );
        // `quarantine_untrusted_output` is not used by verification, for any
        // outcome: the file is present, at its original path, unrenamed.
        assert!(output.is_file(), "the encode must still be at {output:?}");
        assert_eq!(
            fs::read(&output).unwrap(),
            b"a complete-looking encode".to_vec()
        );
        let quarantined = {
            let mut path = output.clone().into_os_string();
            path.push(".untrusted");
            PathBuf::from(path)
        };
        assert!(
            !quarantined.exists(),
            "verification must never quarantine an output"
        );
        cleanup_directory(&directory);
    }

    /// CC6 §6.5: a verification that cannot run records why, and never invents
    /// a pass.
    #[test]
    fn cc6_an_unavailable_verification_records_its_reason_instead_of_a_pass() {
        let directory = test_directory("verify-unavailable");
        let output = directory.join("unverifiable.mp4");
        let analysis = Arc::new(AvailabilityAnalysis::with_verification(
            VerificationDouble::Refused("no usable GPU adapter".to_owned()),
        ));
        let queue = verifying_queue(Arc::clone(&analysis));
        let job = queue
            .enqueue(
                &renderable_document(10),
                verified_request(output.clone(), true, DeliveryEncodeDepth::Eight),
            )
            .unwrap();

        let finished = wait_for_terminal(&queue, job.id);
        assert_eq!(finished.state, ExportJobState::Completed);
        assert_eq!(finished.error, None);
        assert!(finished.verification.is_none());
        let reason = finished
            .verification_unavailable_reason
            .expect("an unavailable verification must say why");
        assert!(reason.contains("no usable GPU adapter"), "{reason}");
        assert_eq!(analysis.verification_calls().len(), 1);
        assert!(output.is_file());
        cleanup_directory(&directory);
    }

    /// CC6 §6.5: `verify: false` does not call the backend at all, and the two
    /// verification fields serialize away entirely.
    ///
    /// The name says "byte-identically" and the assertions below are narrower
    /// than that on purpose: `delivery_bit_depth` is a plain defaulted field,
    /// **always** emitted, so a `verify: false` record is not byte-identical to
    /// a pre-CC6 one. What is asserted is the claim that matters — no
    /// verification key appears — plus the lane, which must be visible on every
    /// record so a reader never has to infer which lane a job encoded at.
    #[test]
    fn cc6_verify_false_skips_the_measurement_and_serializes_byte_identically() {
        let directory = test_directory("verify-off");
        let output = directory.join("unverified.mp4");
        let analysis = Arc::new(AvailabilityAnalysis::with_verification(
            VerificationDouble::Measured(Box::new(canned_verification(
                &output,
                DeliveryEncodeDepth::Eight,
                true,
            ))),
        ));
        let queue = verifying_queue(Arc::clone(&analysis));
        let job = queue
            .enqueue(
                &renderable_document(10),
                verified_request(output.clone(), false, DeliveryEncodeDepth::Eight),
            )
            .unwrap();

        let finished = wait_for_terminal(&queue, job.id);
        assert_eq!(finished.state, ExportJobState::Completed);
        assert!(finished.verification.is_none());
        assert!(finished.verification_unavailable_reason.is_none());
        assert!(
            analysis.verification_calls().is_empty(),
            "verify: false must not reach the backend"
        );

        let serialized = serde_json::to_value(&finished).unwrap();
        let object = serialized.as_object().unwrap();
        assert!(!object.contains_key("verification"));
        assert!(!object.contains_key("verification_unavailable_reason"));
        // `verifying` is a progress detail that is false on every terminal
        // record, so it is skipped too.
        assert!(!object.contains_key("verifying"));
        // The lane is not skipped: it is a plain field with a default, so a
        // reader can always see which lane a job ran at.
        assert_eq!(object["delivery_bit_depth"], serde_json::json!("eight"));
        cleanup_directory(&directory);
    }

    /// CC6 §6.5: a verification backend that unwinds is contained. The job
    /// still completes, the reason names the panic, and — because a
    /// measurement never acts on the file it read — the encode is untouched.
    #[test]
    fn cc6_a_panicking_verification_is_contained_and_leaves_the_output_alone() {
        let directory = test_directory("verify-panic");
        let output = directory.join("panicked-verify.mp4");
        let analysis = Arc::new(AvailabilityAnalysis::with_verification(
            VerificationDouble::Panics,
        ));
        let queue = verifying_queue(Arc::clone(&analysis));
        let job = queue
            .enqueue(
                &renderable_document(10),
                verified_request(output.clone(), true, DeliveryEncodeDepth::Eight),
            )
            .unwrap();

        let finished = wait_for_terminal(&queue, job.id);
        assert_eq!(finished.state, ExportJobState::Completed);
        assert_eq!(
            finished.error, None,
            "a panicking measurement must not fail a finished encode"
        );
        assert!(finished.verification.is_none(), "no pass may be invented");
        let reason = finished
            .verification_unavailable_reason
            .expect("a panicking verification must say so");
        assert!(reason.contains("panicked"), "{reason}");
        assert!(!finished.verifying);
        assert_eq!(analysis.verification_calls().len(), 1);
        // Untouched: same path, same bytes, nothing quarantined.
        assert_eq!(
            fs::read(&output).unwrap(),
            b"a complete-looking encode".to_vec()
        );
        let quarantined = {
            let mut path = output.clone().into_os_string();
            path.push(".untrusted");
            PathBuf::from(path)
        };
        assert!(!quarantined.exists());

        // The worker survived it: the next job still runs.
        let next = directory.join("after-panic.mp4");
        let follower = queue
            .enqueue(
                &renderable_document(10),
                verified_request(next, false, DeliveryEncodeDepth::Eight),
            )
            .unwrap();
        assert_eq!(
            wait_for_terminal(&queue, follower.id).state,
            ExportJobState::Completed
        );
        cleanup_directory(&directory);
    }

    /// CC6 §6.5: a cancel observed *after* the encode finished and *before* the
    /// verification started leaves a `Cancelled` record — and says why there is
    /// no measurement on it.
    ///
    /// The state does not become `Completed`: the operator said stop and the
    /// queue does not overrule them. But the file is written and a caller
    /// asked for it to be verified, so the absence of a measurement is
    /// explained rather than left as a bare `None` that reads like "nothing to
    /// report". A job that never asked to be verified has nothing to explain.
    #[test]
    fn cc6_a_cancel_before_verification_records_why_the_file_is_unmeasured() {
        for verify in [true, false] {
            let directory = test_directory("cancel-before-verify");
            let output = directory.join("cancelled-before-verify.mp4");
            let analysis = Arc::new(AvailabilityAnalysis::with_verification(
                VerificationDouble::Measured(Box::new(canned_verification(
                    &output,
                    DeliveryEncodeDepth::Eight,
                    true,
                ))),
            ));
            let shared: Arc<dyn Analysis> = analysis.clone();
            let queue = ExportQueue::new(
                Arc::new(CancellingExporter {
                    bytes: b"a complete-looking encode",
                }),
                shared,
            )
            .unwrap();
            let job = queue
                .enqueue(
                    &renderable_document(10),
                    verified_request(output.clone(), verify, DeliveryEncodeDepth::Eight),
                )
                .unwrap();

            let finished = wait_for_terminal(&queue, job.id);
            assert_eq!(finished.state, ExportJobState::Cancelled);
            assert_eq!(finished.error, None, "cancelling is not a failure");
            assert!(
                finished.verification.is_none(),
                "no measurement ran, so none may be published"
            );
            assert_eq!(
                finished.verification_unavailable_reason.as_deref(),
                verify.then_some(EXPORT_CANCELLED_BEFORE_VERIFICATION),
                "a job that asked to be verified says why it was not; one that did not asks \
                 nothing and is owed no explanation"
            );
            assert!(!finished.verifying, "nothing is in flight");
            assert!(
                analysis.verification_calls().is_empty(),
                "skipping the measurement is the one thing cancellation can still honour"
            );
            // Cancellation cannot un-write a finished file, and never tries.
            assert_eq!(
                fs::read(&output).unwrap(),
                b"a complete-looking encode".to_vec()
            );
            let quarantined = {
                let mut path = output.clone().into_os_string();
                path.push(".untrusted");
                PathBuf::from(path)
            };
            assert!(!quarantined.exists());
            cleanup_directory(&directory);
        }
    }

    /// CC6 §6.5: `verifying` distinguishes the verification wait from the
    /// encode wait, and cancelling during it leaves a cancelled, unverified
    /// record — the in-flight measurement is discarded, never published.
    #[test]
    fn cc6_cancelling_during_a_verification_leaves_the_record_cancelled_and_unverified() {
        let directory = test_directory("verify-cancel");
        let output = directory.join("cancelled-verify.mp4");
        let (entered_tx, entered_rx) = crossbeam_channel::bounded(1);
        let (release_tx, release_rx) = crossbeam_channel::bounded(1);
        let analysis = Arc::new(AvailabilityAnalysis::with_verification(
            VerificationDouble::Blocking {
                entered: entered_tx,
                release: release_rx,
                verification: Box::new(canned_verification(
                    &output,
                    DeliveryEncodeDepth::Eight,
                    true,
                )),
            },
        ));
        let queue = verifying_queue(Arc::clone(&analysis));
        let job = queue
            .enqueue(
                &renderable_document(10),
                verified_request(output.clone(), true, DeliveryEncodeDepth::Eight),
            )
            .unwrap();
        entered_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("the verification starts");

        // The encode is done and the frame counter has stopped; the record
        // says which wait this is instead of a bare Running.
        let in_flight = queue.get(job.id).expect("the job is inspectable");
        assert_eq!(in_flight.state, ExportJobState::Running);
        assert!(
            in_flight.verifying,
            "a job whose verification is in flight must say so"
        );
        assert!(in_flight.verification.is_none());

        let cancelled = queue.cancel(job.id).expect("the job is cancellable");
        assert_eq!(cancelled.state, ExportJobState::Cancelled);
        release_tx.send(()).unwrap();

        // Verification is not interruptible: it runs to completion, and its
        // result is then dropped rather than published on a cancelled job.
        let settled = wait_for_settled_verification(&queue, job.id);
        assert_eq!(settled.state, ExportJobState::Cancelled);
        assert!(
            settled.verification.is_none(),
            "a cancelled job must publish no measurement"
        );
        assert_eq!(settled.verification_unavailable_reason, None);
        assert_eq!(analysis.verification_calls().len(), 1);
        cleanup_directory(&directory);
    }

    /// Wait for the verification wait to end, whatever the record's state.
    ///
    /// `wait_for_terminal` returns the instant a job is cancelled, which is
    /// before an in-flight verification has finished unwinding out of the
    /// worker; this waits for the flag the worker clears afterwards.
    fn wait_for_settled_verification(queue: &ExportQueue, id: ExportJobId) -> ExportJobRecord {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let job = queue.get(id).expect("queued job must remain inspectable");
            if !job.verifying {
                return job;
            }
            assert!(Instant::now() < deadline, "verification did not settle");
            thread::sleep(Duration::from_millis(5));
        }
    }

    /// CC6 §9.3/§9.4: a job record written before CC6 still reads.
    #[test]
    fn cc6_a_pre_cc6_job_record_deserializes_with_the_eight_bit_lane_and_no_verification() {
        let directory = test_directory("legacy-record");
        let output = directory.join("legacy.mp4");
        let queue = queue(Arc::new(WritingExporter { bytes: b"legacy" }));
        let job = queue
            .enqueue(
                &renderable_document(10),
                verified_request(output, false, DeliveryEncodeDepth::Eight),
            )
            .unwrap();
        let finished = wait_for_terminal(&queue, job.id);

        // Strip every key CC6 added, which is exactly what a pre-CC6 record is.
        let mut legacy = serde_json::to_value(&finished).unwrap();
        let object = legacy.as_object_mut().unwrap();
        object.remove("delivery_bit_depth");
        object.remove("verification");
        object.remove("verification_unavailable_reason");
        assert!(!object.contains_key("verifying"));

        let restored: ExportJobRecord = serde_json::from_value(legacy).unwrap();
        assert_eq!(restored.delivery_bit_depth, DeliveryEncodeDepth::Eight);
        assert_eq!(restored.verification, None);
        assert_eq!(restored.verification_unavailable_reason, None);
        assert_eq!(restored, finished);
        cleanup_directory(&directory);
    }
    /// Swap one source's live identity from inside the encode so the queue has
    /// to observe the change after the exporter returns.
    struct SourceSwappingExporter {
        analysis: Arc<AvailabilityAnalysis>,
        asset: AssetId,
        replacement_sha256: &'static str,
    }

    impl Export for SourceSwappingExporter {
        fn export(
            &self,
            _out: &Path,
            _settings: kinewright_core::ExportSettings,
            _progress: kinewright_core::ProgressSink,
        ) -> Result<(), MediaError> {
            unreachable!("tests exercise immutable document export")
        }

        fn export_document(
            &self,
            _document: Arc<Document>,
            out: &Path,
            _settings: kinewright_core::ExportSettings,
            _progress: kinewright_core::ProgressSink,
        ) -> Result<(), MediaError> {
            // A real encoder leaves a complete-looking file behind, which is
            // exactly what makes a drift failure dangerous.
            fs::write(out, b"untrusted encode").unwrap();
            self.analysis
                .set_observed_fingerprint(self.asset, self.replacement_sha256);
            Ok(())
        }
    }

    #[test]
    fn a_source_swapped_mid_encode_fails_the_job_instead_of_completing_it() {
        let analysis = Arc::new(AvailabilityAnalysis::default());
        analysis.set_observed_fingerprint(AssetId(1), "aa");
        analysis.set_observed_fingerprint(AssetId(2), "bb");
        let exporter = Arc::new(SourceSwappingExporter {
            analysis: Arc::clone(&analysis),
            asset: AssetId(1),
            replacement_sha256: "cc",
        });
        let queue = ExportQueue::new(exporter, analysis).unwrap();
        let directory = test_directory("toctou-swap");
        let output = directory.join("swapped.mp4");
        let quarantined = directory.join("swapped.mp4.untrusted");

        let job = queue
            .enqueue(
                &media_document_with_video_audio_and_unused(),
                request(output.clone(), false),
            )
            .unwrap();
        let terminal = wait_for_terminal(&queue, job.id);

        assert_eq!(terminal.state, ExportJobState::Failed);
        let error = terminal.error.unwrap();
        assert!(
            error.contains("changed between the export preflight and the finished encode"),
            "{error}"
        );
        assert!(error.contains("source-1"), "{error}");

        // The untrusted encode must not be left where the caller asked for a
        // trusted one, and the failure has to name where it went.
        assert!(
            !output.exists(),
            "a drift-failed encode must not stay at the requested output path"
        );
        assert!(
            quarantined.exists(),
            "the untrusted output must be retained"
        );
        assert!(
            error.contains(&quarantined.display().to_string()),
            "the failure must name the quarantined path: {error}"
        );
        cleanup_directory(&directory);
    }

    #[test]
    fn an_unchanged_source_still_completes_after_the_encode() {
        let analysis = Arc::new(AvailabilityAnalysis::default());
        analysis.set_observed_fingerprint(AssetId(1), "aa");
        analysis.set_observed_fingerprint(AssetId(2), "bb");
        let queue = ExportQueue::new(
            Arc::new(RecordingExporter::new(Duration::ZERO)),
            Arc::clone(&analysis) as Arc<dyn Analysis>,
        )
        .unwrap();
        let directory = test_directory("toctou-stable");

        let job = queue
            .enqueue(
                &media_document_with_video_audio_and_unused(),
                request(directory.join("stable.mp4"), false),
            )
            .unwrap();

        assert_eq!(
            wait_for_terminal(&queue, job.id).state,
            ExportJobState::Completed
        );
        cleanup_directory(&directory);
    }

    // -----------------------------------------------------------------------
    // CC4 §2.3 — the export LUT preflight
    // -----------------------------------------------------------------------

    /// A document whose one clip carries a `creative_look` bound to an
    /// imported asset whose bytes are the pinned `warm` bake.
    fn look_document() -> Document {
        let mut document = renderable_document(10);
        document.lut_assets = vec![kinewright_core::LutAsset {
            id: kinewright_core::LutAssetId(1),
            sha256: kinewright_media::BuiltinLook::Warm
                .pinned_sha256()
                .to_owned(),
            title: "Warm".to_owned(),
            kind: kinewright_core::LutAssetKind::Cube3d,
            size: 17,
            byte_len: kinewright_media::BuiltinLook::Warm.byte_len(),
            domain_min_millionths: [-1_000_000; 3],
            domain_max_millionths: [2_000_000; 3],
            source: kinewright_core::LutAssetSource::Imported {
                source_path: "/looks/warm.cube".to_owned(),
            },
        }];
        document.tracks[0].clips[0].effects = vec![kinewright_core::Effect {
            id: kinewright_core::EffectId(1),
            name: "creative_look".to_owned(),
            parameters: BTreeMap::from([(
                "lut_asset_id".to_owned(),
                kinewright_core::ParamValue::Integer(1),
            )]),
            keyframes: BTreeMap::new(),
        }];
        document
    }

    fn lut_queue(project_path: Option<PathBuf>) -> ExportQueue {
        ExportQueue::configured(
            Arc::new(RecordingExporter::new(Duration::ZERO)),
            fail_closed_analysis(),
            DEFAULT_EXPORT_QUEUE_CAPACITY,
            Arc::new(RwLock::new(project_path)),
        )
        .unwrap()
    }

    /// CC4 §2.3: a look a rendered frame could need blocks the export before
    /// the encode, naming the asset, its recorded hash, and the nodes that
    /// would have evaluated it.
    #[test]
    fn cc4_export_blocks_when_a_referenced_look_is_missing_from_the_store() {
        let directory = test_directory("cc4-lut-missing");
        let project = directory.join("edit.kinewright");
        let queue = lut_queue(Some(project));
        let error = queue
            .enqueue(&look_document(), request(directory.join("out.mp4"), false))
            .expect_err("a missing look must block delivery");
        let ExportQueueError::LutPreflight(report) = error else {
            panic!("expected a LUT preflight rejection, got {error}");
        };
        assert!(!report.export_ready());
        assert_eq!(
            report.checked_lut_assets,
            vec![kinewright_core::LutAssetId(1)]
        );
        assert_eq!(report.issues.len(), 1);
        assert_eq!(report.issues[0].title, "Warm");
        assert_eq!(
            report.issues[0].sha256,
            kinewright_media::BuiltinLook::Warm.pinned_sha256()
        );
        assert_eq!(
            report.issues[0].kind,
            kinewright_core::LutAvailabilityKind::Missing
        );
        assert_eq!(
            report.issues[0].referenced_by,
            vec![(kinewright_core::ClipId(1), kinewright_core::EffectId(1))]
        );
        cleanup_directory(&directory);
    }

    /// CC4 §2.3: the same document passes once the hashed bytes are present in
    /// the project store.
    #[test]
    fn cc4_export_passes_when_the_referenced_look_is_hash_verified() {
        let directory = test_directory("cc4-lut-verified");
        let project = directory.join("edit.kinewright");
        let luts = directory.join("edit.kinewright-assets").join("luts");
        fs::create_dir_all(&luts).unwrap();
        let warm = kinewright_media::BuiltinLook::Warm;
        fs::write(
            luts.join(format!("{}.cube", warm.pinned_sha256())),
            warm.canonical_text(),
        )
        .unwrap();

        let queue = lut_queue(Some(project));
        let record = queue
            .enqueue(&look_document(), request(directory.join("out.mp4"), false))
            .expect("a verified look must not block delivery");
        assert_eq!(
            wait_for_terminal(&queue, record.id).state,
            ExportJobState::Completed
        );
        cleanup_directory(&directory);
    }

    /// CC4 §2.2: with no saved project there is no store root at all, so a
    /// timeline carrying a look blocks with `project_not_saved` rather than
    /// exporting a frame whose look cannot be verified.
    #[test]
    fn cc4_export_blocks_a_look_when_the_project_has_never_been_saved() {
        let directory = test_directory("cc4-lut-unsaved");
        let queue = lut_queue(None);
        let error = queue
            .enqueue(&look_document(), request(directory.join("out.mp4"), false))
            .expect_err("an unsaved project cannot verify a look");
        assert!(
            matches!(error, ExportQueueError::LutStoreNotSaved),
            "expected the unsaved-project rejection, got {error:?}"
        );

        // A timeline with no possibly-active LUT node needs no store at all,
        // so an unsaved project still exports normally.
        let record = queue
            .enqueue(
                &renderable_document(10),
                request(directory.join("plain.mp4"), false),
            )
            .expect("a look-free timeline needs no LUT store");
        assert_eq!(
            wait_for_terminal(&queue, record.id).state,
            ExportJobState::Completed
        );
        cleanup_directory(&directory);
    }

    /// CC4 §2.2: a project that *is* saved but whose derived store root is
    /// refused is a different rejection from one that was never saved. It
    /// carries the store's own typed refusal, because "save the project" is a
    /// recovery that can never clear it.
    #[test]
    fn cc4_export_separates_a_refused_store_root_from_an_unsaved_project() {
        let directory = test_directory("cc4-lut-refused-root");
        let project = directory.join("edit.kinewright");
        // A regular file occupies exactly where the store directory belongs.
        fs::write(directory.join("edit.kinewright-assets"), b"not a directory").unwrap();

        let queue = lut_queue(Some(project));
        let error = queue
            .enqueue(&look_document(), request(directory.join("out.mp4"), false))
            .expect_err("a refused store root cannot verify a look");
        let ExportQueueError::LutStoreRootInvalid { reason } = &error else {
            panic!("expected the refused-root rejection, got {error:?}");
        };
        assert!(
            reason.starts_with("lut_store_root_invalid: "),
            "the typed code survives the MediaError label: {reason}"
        );
        assert!(
            !error.to_string().contains("save the project"),
            "a saved project must never be told to save itself: {error}"
        );

        // A look-free timeline still needs no store, refused root or not.
        let record = queue
            .enqueue(
                &renderable_document(10),
                request(directory.join("plain.mp4"), false),
            )
            .expect("a look-free timeline needs no LUT store");
        assert_eq!(
            wait_for_terminal(&queue, record.id).state,
            ExportJobState::Completed
        );
        cleanup_directory(&directory);
    }
}
