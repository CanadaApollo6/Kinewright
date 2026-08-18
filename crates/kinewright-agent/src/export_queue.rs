use std::{
    any::Any,
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicU64, Ordering},
    },
    thread,
};

use crossbeam_channel::{Receiver, Sender};
use kinewright_core::{
    DeliveryConformanceReport, DeliveryProfile, Document, Export, ExportCancellation,
    ExportProgress, MediaError, delivery_conformance, document_for_delivery_profile,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const DEFAULT_EXPORT_QUEUE_CAPACITY: usize = 64;

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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
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
}

#[derive(Clone)]
pub struct ExportQueue {
    state: Arc<QueueState>,
    work_tx: Sender<WorkItem>,
    capacity: usize,
}

struct QueueState {
    exporter: Arc<dyn Export>,
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
    cancellation: ExportCancellation,
}

impl ExportQueue {
    /// Start a bounded, serial export queue backed by one worker thread.
    ///
    /// # Errors
    ///
    /// Returns an error if the worker thread cannot be started.
    pub fn new(exporter: Arc<dyn Export>) -> Result<Self, ExportQueueError> {
        Self::with_capacity(exporter, DEFAULT_EXPORT_QUEUE_CAPACITY)
    }

    /// Start a serial export queue with an explicit maximum pending count.
    ///
    /// # Errors
    ///
    /// Returns an error for a zero capacity or if the worker thread cannot be
    /// started.
    pub fn with_capacity(
        exporter: Arc<dyn Export>,
        capacity: usize,
    ) -> Result<Self, ExportQueueError> {
        if capacity == 0 {
            return Err(ExportQueueError::InvalidCapacity);
        }
        let (work_tx, work_rx) = crossbeam_channel::bounded(capacity);
        let state = Arc::new(QueueState {
            exporter,
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
        let conformance = delivery_conformance(document, profile, focus_x_percent, focus_y_percent)
            .map_err(|error| ExportQueueError::InvalidDelivery(error.to_string()))?;
        if !conformance.export_ready() {
            return Err(ExportQueueError::Conformance(Box::new(conformance)));
        }
        let delivery_document = Arc::new(
            document_for_delivery_profile(document, profile, focus_x_percent, focus_y_percent)
                .map_err(|error| ExportQueueError::InvalidDelivery(error.to_string()))?,
        );
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
                error: None,
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
            cancellation,
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
        mark_cancelled(state, work.id);
        return;
    }
    if let Err(error) = validate_worker_output(&work.output_path, work.overwrite) {
        mark_failed(state, work.id, error.to_string());
        return;
    }

    let settings = work
        .profile
        .export_settings(&work.document, work.cancellation.clone());
    let (progress_tx, progress_rx) = crossbeam_channel::unbounded();
    let (stop_tx, stop_rx) = crossbeam_channel::bounded(1);
    let progress_state = Arc::clone(state);
    let progress_id = work.id;
    let progress_thread = thread::Builder::new()
        .name(format!("kinewright-export-progress-{}", work.id.0))
        .spawn(move || monitor_progress(&progress_state, progress_id, &progress_rx, &stop_rx));

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
        mark_cancelled(state, work.id);
        return;
    }
    match result {
        Ok(Ok(())) => mark_completed(state, work.id),
        Ok(Err(MediaError::Cancelled)) => mark_cancelled(state, work.id),
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

fn mark_completed(state: &Arc<QueueState>, id: ExportJobId) {
    if let Some(job) = lock_jobs(state).get_mut(&id)
        && job.record.state != ExportJobState::Cancelled
    {
        job.record.state = ExportJobState::Completed;
        job.record.error = None;
        if job.record.progress.total_frames > 0 {
            job.record.progress.completed_frames = job.record.progress.total_frames;
        }
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

fn mark_cancelled(state: &Arc<QueueState>, id: ExportJobId) {
    if let Some(job) = lock_jobs(state).get_mut(&id) {
        job.cancellation.cancel();
        job.record.state = ExportJobState::Cancelled;
        job.record.error = None;
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
        AssetId, Clip, ClipContent, MediaError, TimeCode, Title, Track, TrackId, TrackKind,
    };

    use super::*;

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
        let queue = ExportQueue::new(exporter.clone()).unwrap();
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
        let queue = ExportQueue::new(exporter.clone()).unwrap();
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
        let queue = ExportQueue::new(Arc::new(CancellableExporter {
            started_tx,
            finished_tx,
        }))
        .unwrap();
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
        let queue = ExportQueue::new(exporter).unwrap();
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
        let queue = ExportQueue::new(exporter).unwrap();
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
    fn active_jobs_reserve_their_normalized_output_path() {
        let (started_tx, started_rx) = crossbeam_channel::bounded(1);
        let (release_tx, release_rx) = crossbeam_channel::bounded(1);
        let exporter = Arc::new(BlockingExporter {
            calls: AtomicUsize::new(0),
            started_tx,
            release_rx,
        });
        let queue = ExportQueue::new(exporter).unwrap();
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
        let queue = ExportQueue::new(exporter.clone()).unwrap();
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
        let queue = ExportQueue::with_capacity(exporter, 1).unwrap();
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
}
