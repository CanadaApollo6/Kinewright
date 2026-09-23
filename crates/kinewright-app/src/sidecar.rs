//! The incident sidecar: `<stem>.kinewright-incidents` beside the project (`IN2B` §2).
//!
//! The sidecar carries the incident log across runs as JSON: an envelope with a
//! format version and a digest pair, plus one record per persisted incident
//! ([`kinewright_core::IncidentRecord`]). Records that do not parse — or whose
//! code this build predates — are carried verbatim and re-emitted byte-equal,
//! never pruned and never trusted.
//!
//! Writes land through the ONE [`SidecarWriter`] thread per app (N2/B-5):
//! sequence-numbered jobs keyed by path, stale jobs dropped, per-job unique
//! temp names, temp + rename. Close/exit/save submit and join; the debounced
//! background flush submits without joining and its failures surface through
//! [`SidecarWriter::take_errors`].

use std::{
    collections::HashMap,
    fs, io,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    thread::JoinHandle,
};

use crossbeam_channel::{Receiver, Sender};
use kinewright_core::{
    IncidentCode, IncidentObservation, IncidentRecord, IncidentSubject, LabelIncident,
    TimelineRevision, from_code,
};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;

/// The sidecar format version this build writes (`IN2B` §2 rule 2).
///
/// Independent of the project's counter: coupling them would force a sidecar
/// bump every time the document format moves and vice versa (d11).
pub(crate) const SIDECAR_FORMAT_VERSION: u32 = 1;

/// The sidecar suffix: `<stem>.kinewright-incidents` (d1).
pub(crate) const SIDECAR_SUFFIX: &str = "kinewright-incidents";

/// FNV-1a 64 over bytes as 16 lowercase hex chars (`IN2B` §0.4 d2).
///
/// The pairing digest: it catches file mix-ups, not adversaries. Shared with
/// the journal-name hash — one implementation, two callers.
#[must_use]
pub(crate) fn digest_bytes(bytes: &[u8]) -> String {
    format!("{:016x}", crate::recovery::fnv1a_64(bytes))
}

/// Derive a project's sidecar path, mirroring `derive_lut_store` (`IN2B` §2
/// rule 1).
///
/// `None` yields `None` and no IO is attempted (rule 10); otherwise the
/// sibling `<stem>.kinewright-incidents`. A path with no file stem (a root, a
/// parent reference) likewise yields `None` rather than inventing a name.
#[must_use]
pub(crate) fn sidecar_path_for_project(project_path: Option<&Path>) -> Option<PathBuf> {
    let path = project_path?;
    let stem = path.file_stem()?;
    let mut name = stem.to_os_string();
    name.push(".");
    name.push(SIDECAR_SUFFIX);
    Some(path.parent().unwrap_or_else(|| Path::new("")).join(name))
}

/// How `ProjectSession::create` treats the sidecar (N2/B-1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SidecarMode {
    /// Digest-gated load: startup reopen and `open_project`, carrying the
    /// digest `load_document` read — single read, no TOCTOU (`IN2B` §4
    /// rule 2).
    Load { project_digest: String },
    /// Ungated load: recovery restore, whose document is definitionally newer
    /// than the last save, so the digest cannot match. Version arms still
    /// apply (`IN2B` §4 rule 5).
    RecoveryNoDigest,
    /// No load and no IO: new projects.
    None,
}

/// A sidecar that parsed and passed the version gate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LoadedSidecar {
    /// Records that parsed, in file order — including unknown-code records,
    /// which `restore` aggregates while their raw texts ride in `carried`
    /// (N4/F3).
    pub records: Vec<IncidentRecord>,
    /// Raw record texts carried verbatim: unparseable records plus every
    /// unknown-code record's source slice, byte-equal, in file order.
    pub carried: Vec<String>,
    /// The highest `id` over all carried raw values, for `restore`'s
    /// `id_floor` (N4/F2). `None` when no carried value carries a `u64` id.
    pub id_floor: Option<u64>,
    /// The envelope's digest pair (`IN2B` §2 rule 9).
    pub project_digest: String,
    /// The digest before the save that wrote this sidecar (`""` when the
    /// sidecar's save was the path's first).
    pub previous_digest: String,
}

/// What loading a sidecar file returns (`IN2B` §2 rule 2).
///
/// Exactly the four arms probe BP7 prototyped. The digest gate (rule 9) is not
/// a fifth arm: the caller applies [`sidecar_matches_project`] to `Current`
/// and refuses on mismatch, same disposition as `Corrupt`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SidecarLoad {
    /// No file: an empty log, silently.
    Absent,
    /// Parsed and version-gated; the caller still checks the digest pair.
    Current(LoadedSidecar),
    /// A newer writer's file: refused, never rewritten.
    Newer(u32),
    /// Truncated, torn, invalid JSON, versionless, or wrongly shaped:
    /// refused, with the reason.
    Corrupt(String),
}

/// The envelope as written (`IN2B` §2 rule 2), minus the version.
///
/// `load_sidecar` gates `format_version` through the `Value` first (missing →
/// `Corrupt`, oversize → `Corrupt`, newer → `Newer`), so the typed pass reads
/// only the digests and the raw records; the version key is ignored here.
/// Writes go through [`build_sidecar_bytes`] so carried texts re-emit
/// verbatim rather than through a normalising `Value` round trip (rule 8b).
#[derive(Debug, Deserialize)]
struct SidecarFile {
    project_digest: String,
    previous_digest: String,
    records: Vec<Box<RawValue>>,
}

/// Load and parse a sidecar file: the four arms, no digest gate, no rename.
///
/// * Missing file → `Absent`. Any other IO error → `Corrupt`.
/// * Invalid JSON, a non-object envelope, or a wrongly shaped envelope →
///   `Corrupt` with the reason.
/// * Missing `format_version` → `Corrupt("missing format_version")`: the
///   sidecar has no legacy, so unlike the `Document` it defaults nothing
///   (N1.1/OQ4).
/// * A `format_version` that is not a `u32` (string, float, negative, or too
///   large) → `Corrupt` (BR40: oversize reads as `Corrupt`, not `Newer` — a
///   version past the `u32` range is malformed input to the envelope's `u32`
///   field, and `Corrupt` preserves the bytes via `.bak` exactly as `Newer`
///   does).
/// * `format_version` above [`SIDECAR_FORMAT_VERSION`] → `Newer(v)`. Version 0
///   loads: the gate is newer-side only.
/// * Records parse per element (rule 8b): unparseable elements pass into
///   `carried` untouched, and unknown-code records join them (N4/F3) while
///   still reaching `restore` for the aggregate — body #5 stays true.
#[must_use]
pub(crate) fn load_sidecar(sidecar_path: &Path) -> SidecarLoad {
    let bytes = match fs::read(sidecar_path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return SidecarLoad::Absent,
        Err(error) => {
            return SidecarLoad::Corrupt(format!("could not read the sidecar: {error}"));
        }
    };
    let value: serde_json::Value = match serde_json::from_slice(&bytes) {
        Ok(value) => value,
        Err(error) => return SidecarLoad::Corrupt(trim_json_error(&error)),
    };
    let Some(envelope) = value.as_object() else {
        return SidecarLoad::Corrupt("the sidecar is not a JSON object".to_owned());
    };
    let Some(raw_version) = envelope.get("format_version") else {
        return SidecarLoad::Corrupt("missing format_version".to_owned());
    };
    let Some(version_wide) = raw_version.as_u64() else {
        return SidecarLoad::Corrupt(format!(
            "format_version is not a version number: {raw_version}"
        ));
    };
    if version_wide > u64::from(u32::MAX) {
        return SidecarLoad::Corrupt(format!(
            "format_version {version_wide} is larger than a u32 (BR40)"
        ));
    }
    let version = u32::try_from(version_wide).unwrap_or(u32::MAX);
    if version > SIDECAR_FORMAT_VERSION {
        return SidecarLoad::Newer(version);
    }
    let file: SidecarFile = match serde_json::from_slice(&bytes) {
        Ok(file) => file,
        Err(error) => return SidecarLoad::Corrupt(trim_json_error(&error)),
    };
    let mut records = Vec::with_capacity(file.records.len());
    let mut carried = Vec::new();
    let mut id_floor: Option<u64> = None;
    for raw in &file.records {
        let text = raw.get();
        match serde_json::from_str::<IncidentRecord>(text) {
            Err(_) => {
                id_floor = id_floor.max(carried_raw_id(text));
                carried.push(text.to_owned());
            }
            Ok(record) => {
                if from_code(&record.code).is_none() {
                    // N4/F3: carried verbatim for the next write AND passed to
                    // `restore`, which skips, counts and aggregates (body #5).
                    id_floor = id_floor.max(Some(record.id.0));
                    carried.push(text.to_owned());
                }
                records.push(record);
            }
        }
    }
    SidecarLoad::Current(LoadedSidecar {
        records,
        carried,
        id_floor,
        project_digest: file.project_digest,
        previous_digest: file.previous_digest,
    })
}

/// One line of a serde error: file position without the multi-line context.
fn trim_json_error(error: &serde_json::Error) -> String {
    error
        .to_string()
        .lines()
        .next()
        .unwrap_or("invalid JSON")
        .to_owned()
}

/// The `id` a carried raw value carries, when it parses that far (N4/F2).
///
/// Anything else — a non-object, a missing `id`, a non-`u64` `id` —
/// contributes nothing to the floor: core clamps and resumes above what it
/// is given, and an unreadable id cannot collide by construction because no
/// restored entry holds it.
fn carried_raw_id(text: &str) -> Option<u64> {
    #[derive(Deserialize)]
    struct CarriedId {
        #[serde(default)]
        id: Option<u64>,
    }
    serde_json::from_str::<CarriedId>(text).ok()?.id
}

/// Whether a loaded sidecar pairs with project bytes of this digest (`IN2B` §2
/// rule 9).
///
/// Either digest accepts: a crash between the sidecar write and the project
/// write leaves the *previous* digest matching — no false `.bak`.
#[must_use]
pub(crate) fn sidecar_matches_project(loaded: &LoadedSidecar, project_digest: &str) -> bool {
    loaded.project_digest == project_digest || loaded.previous_digest == project_digest
}

/// Refuse a sidecar the load cannot take: rename to the first-free `.bak`
/// (`IN2B` §2 rules 8–8a, N2/S-16).
///
/// The target is `<sidecar>.bak`, or `.bak.1`, `.bak.2`, … — the first name
/// that does not exist, so a second refusal never overwrites the first.
/// Returns the path the bytes moved to. A newer build never reads a `.bak` on
/// its own; the person renames it back by hand (N-10).
pub(crate) fn refuse_sidecar(sidecar_path: &Path) -> io::Result<PathBuf> {
    refuse_sidecar_with(sidecar_path, &|from, to| fs::rename(from, to))
}

/// The injected `.bak` rename (N6/H3): `None` renames for real, `Some`
/// runs the test's failure.
pub(crate) type RefuseRename = dyn Fn(&Path, &Path) -> io::Result<()>;

/// [`refuse_sidecar`] with the rename injected (N6/H3): a failed `.bak`
/// rename suspends the session instead of failing silently, and no portable
/// fixture fails a real rename — the test injects the failure.
pub(crate) fn refuse_sidecar_with(
    sidecar_path: &Path,
    rename: &RefuseRename,
) -> io::Result<PathBuf> {
    let mut first = sidecar_path.as_os_str().to_owned();
    first.push(".bak");
    let mut candidate = PathBuf::from(first);
    if candidate.exists() {
        for suffix in 1u32.. {
            let mut numbered = sidecar_path.as_os_str().to_owned();
            numbered.push(format!(".bak.{suffix}"));
            candidate = PathBuf::from(numbered);
            if !candidate.exists() {
                break;
            }
        }
    }
    rename(sidecar_path, &candidate)?;
    Ok(candidate)
}

/// The observation a refused sidecar notes: exactly one `sidecar_refused`
/// (`IN2B` §2 rule 8, §5 rule 1 #6).
///
/// Transient, like every §5 note: a per-run note persisted `Open` would badge
/// a clean reopen with a false card (rule 12, N2/B-4).
#[must_use]
pub(crate) fn sidecar_refused_observation(
    reason: impl Into<String>,
    revision: TimelineRevision,
) -> IncidentObservation {
    let mut observation = IncidentObservation::plain(
        IncidentCode::Label(LabelIncident::SidecarRefused),
        IncidentSubject::Project,
        reason,
        revision,
    );
    observation.transient = true;
    observation
}

/// The observation a failed sidecar write notes: exactly one
/// `sidecar_write_failed`, and the save returns success anyway (`IN2B` §2 rule
/// 6, §5 rule 1 #7). Transient, like every §5 note.
#[must_use]
pub(crate) fn sidecar_write_failed_observation(
    reason: impl Into<String>,
    revision: TimelineRevision,
) -> IncidentObservation {
    let mut observation = IncidentObservation::plain(
        IncidentCode::Label(LabelIncident::SidecarWriteFailed),
        IncidentSubject::Project,
        reason,
        revision,
    );
    observation.transient = true;
    observation
}

/// Serialise an envelope: pretty, with carried texts re-emitted verbatim
/// (`IN2B` §2 rules 1, 8b).
///
/// Parsed records render pretty (the §2 worked shape); carried texts ride as
/// [`RawValue`], so the round trip is byte-equal, not value-equal — a
/// normalising `Value` pass would rewrite history this build cannot read.
/// Consecutive writes over an unchanged log are byte-identical.
pub(crate) fn build_sidecar_bytes(
    records: &[IncidentRecord],
    carried: &[String],
    project_digest: &str,
    previous_digest: &str,
) -> Result<Vec<u8>, String> {
    #[derive(Serialize)]
    struct WriteEnvelope<'a> {
        format_version: u32,
        project_digest: &'a str,
        previous_digest: &'a str,
        records: Vec<Box<RawValue>>,
    }
    let mut raws = Vec::with_capacity(records.len() + carried.len());
    for record in records {
        let text = serde_json::to_string_pretty(record).map_err(|error| error.to_string())?;
        raws.push(RawValue::from_string(text).map_err(|error| error.to_string())?);
    }
    for text in carried {
        raws.push(RawValue::from_string(text.clone()).map_err(|error| error.to_string())?);
    }
    let envelope = WriteEnvelope {
        format_version: SIDECAR_FORMAT_VERSION,
        project_digest,
        previous_digest,
        records: raws,
    };
    serde_json::to_vec_pretty(&envelope).map_err(|error| error.to_string())
}

/// What one synchronous sidecar flush did (`IN2B` §2 rules 4–5, N-5).
///
/// Rule 5's `io::Result<WriteReport>` refined by N-5's "the skipped job
/// reports `Skipped`": `Written` carries the report, `Skipped` covers the
/// unsaved project (no path, no IO — item 17) and the unchanged log (nothing
/// newer than the last write, no torn-write window opened for nothing).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FlushOutcome {
    /// Bytes landed via temp + rename; the report counts them.
    Written(kinewright_core::WriteReport),
    /// No IO was attempted.
    Skipped,
}

/// One sidecar write on its way to the writer thread.
struct WriterJob {
    path: PathBuf,
    seq: u64,
    bytes: Vec<u8>,
    /// Present for close/exit/save jobs, which join; absent for debounced
    /// background jobs, whose failures surface via [`SidecarWriter::take_errors`].
    ack: Option<Sender<io::Result<()>>>,
}

enum WriterMessage {
    Job(WriterJob),
    Stop,
}

/// The test hook the writer runs between the temp write and the rename
/// (item 15's S-12 box), shared with the writer thread.
type RenameHook = Arc<Mutex<Option<Arc<dyn Fn() + Send + Sync>>>>;

/// The ONE sidecar writer thread per app (N2/B-5).
///
/// Records are built on the frame thread under a read lock; the serialised
/// bytes land here. Jobs are sequence-numbered across a single channel, so
/// channel order is sequence order; the thread additionally keeps the latest
/// job per path per batch and drops anything at or below the last written
/// sequence for that path — a stale background snapshot can never overwrite a
/// save's bytes, and a save's digest pair is what the gate box asserts (item
/// 14). Every job writes a per-job unique temp name
/// (`<stem>.kinewright-incidents.<seq>.tmp`), never a shared `.tmp`, then
/// renames — a load concurrent with a write sees the prior bytes or the new
/// ones, never torn ones (item 15's S-12 box).
///
/// `Drop` stops the thread after draining jobs queued before the stop; a job
/// sent after shutdown begins is never processed, so `submit_and_join` must
/// not race the last `Arc` drop — in production every flush happens before
/// teardown, and tests join before dropping.
pub(crate) struct SidecarWriter {
    tx: Sender<WriterMessage>,
    seq: AtomicU64,
    /// Async-job failures, drained by the frame thread, which notes one
    /// `sidecar_write_failed` per entry. Sync jobs report through their ack
    /// instead and never land here — one failure, one incident, whichever
    /// path flushed.
    errors: Arc<Mutex<Vec<(PathBuf, String)>>>,
    handle: Mutex<Option<JoinHandle<()>>>,
    /// Test hook run between the temp write and the rename (item 15's S-12
    /// box). Always `None` in production.
    rename_hook: RenameHook,
}

impl SidecarWriter {
    /// Spawn the writer thread.
    ///
    /// A spawn failure degrades rather than panics: the handle is `None`, the
    /// receiver is dropped, sync jobs fail (their callers note
    /// `sidecar_write_failed`) and async jobs queue nowhere. The spawn runs
    /// once per app lifetime, where the OS is not out of threads.
    #[must_use]
    pub(crate) fn new() -> Arc<Self> {
        let (tx, rx) = crossbeam_channel::unbounded();
        let writer = Arc::new(Self {
            tx,
            seq: AtomicU64::new(0),
            errors: Arc::new(Mutex::new(Vec::new())),
            handle: Mutex::new(None),
            rename_hook: Arc::new(Mutex::new(None)),
        });
        let thread_errors = Arc::clone(&writer.errors);
        let thread_hook = Arc::clone(&writer.rename_hook);
        let handle = std::thread::Builder::new()
            .name("kinewright-sidecar-writer".to_owned())
            .spawn(move || writer_loop(&rx, &thread_errors, &thread_hook))
            .ok();
        *writer
            .handle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = handle;
        writer
    }

    /// Queue one background write without joining (the debounced flush).
    ///
    /// Best-effort: a send failure means the writer is gone, which only
    /// shutdown does — and shutdown drains before it stops.
    pub(crate) fn submit(&self, path: PathBuf, bytes: Vec<u8>) {
        let seq = self.seq.fetch_add(1, Ordering::SeqCst);
        let _ = self.tx.send(WriterMessage::Job(WriterJob {
            path,
            seq,
            bytes,
            ack: None,
        }));
    }

    /// Queue one write and block until it lands (close/exit/save).
    ///
    /// A superseded job still acks `Ok`: a newer job for the same path won,
    /// which is the outcome the caller wanted — the freshest bytes.
    pub(crate) fn submit_and_join(&self, path: PathBuf, bytes: Vec<u8>) -> io::Result<()> {
        let seq = self.seq.fetch_add(1, Ordering::SeqCst);
        let (ack_tx, ack_rx) = crossbeam_channel::bounded(1);
        let sent = self
            .tx
            .send(WriterMessage::Job(WriterJob {
                path,
                seq,
                bytes,
                ack: Some(ack_tx),
            }))
            .is_ok();
        if !sent {
            return Err(io::Error::other("the sidecar writer is gone"));
        }
        ack_rx
            .recv()
            .unwrap_or_else(|_| Err(io::Error::other("the sidecar writer died")))
    }

    /// Drain async-job failures for the frame thread to note.
    pub(crate) fn take_errors(&self) -> Vec<(PathBuf, String)> {
        std::mem::take(
            &mut self
                .errors
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
    }

    /// Run `hook` between the temp write and the rename of every job until
    /// replaced. Test-only: item 15's S-12 box pauses the writer mid-write and
    /// loads in the window.
    #[cfg(test)]
    pub(crate) fn set_rename_hook(&self, hook: Arc<dyn Fn() + Send + Sync>) {
        *self
            .rename_hook
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(hook);
    }
}

impl Drop for SidecarWriter {
    fn drop(&mut self) {
        let _ = self.tx.send(WriterMessage::Stop);
        if let Ok(mut slot) = self.handle.lock()
            && let Some(handle) = slot.take()
        {
            let _ = handle.join();
        }
    }
}

/// The writer thread: batch, keep the latest per path, land via temp + rename.
fn writer_loop(
    rx: &Receiver<WriterMessage>,
    errors: &Arc<Mutex<Vec<(PathBuf, String)>>>,
    rename_hook: &RenameHook,
) {
    let mut last_written: HashMap<PathBuf, u64> = HashMap::new();
    loop {
        let mut batch = Vec::new();
        let mut stopping = false;
        match rx.recv() {
            Ok(WriterMessage::Job(job)) => batch.push(job),
            Ok(WriterMessage::Stop) => stopping = true,
            Err(_) => break,
        }
        while let Ok(message) = rx.try_recv() {
            match message {
                WriterMessage::Job(job) => batch.push(job),
                WriterMessage::Stop => stopping = true,
            }
        }
        write_batch(batch, &mut last_written, errors, rename_hook);
        if stopping {
            // A stop was seen: land anything that arrived during the batch,
            // then exit. Jobs sent after this drain are refused by a dead
            // writer; callers never send after shutdown begins.
            let mut tail = Vec::new();
            while let Ok(message) = rx.try_recv() {
                if let WriterMessage::Job(job) = message {
                    tail.push(job);
                }
            }
            write_batch(tail, &mut last_written, errors, rename_hook);
            break;
        }
    }
}

/// Land one batch: latest job per path wins, stale jobs ack `Ok` without
/// touching the disk, async failures queue for the frame thread.
fn write_batch(
    batch: Vec<WriterJob>,
    last_written: &mut HashMap<PathBuf, u64>,
    errors: &Arc<Mutex<Vec<(PathBuf, String)>>>,
    rename_hook: &RenameHook,
) {
    let mut latest: HashMap<PathBuf, WriterJob> = HashMap::new();
    for job in batch {
        match latest.entry(job.path.clone()) {
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(job);
            }
            std::collections::hash_map::Entry::Occupied(mut entry) => {
                if job.seq > entry.get().seq {
                    ack_ok(entry.insert(job));
                } else {
                    ack_ok(job);
                }
            }
        }
    }
    // Sequence order, not map order: a sync job's ack then implies every
    // earlier job in the batch already landed or queued its error, which is
    // what makes the error drain deterministic.
    let mut winners: Vec<(PathBuf, WriterJob)> = latest.into_iter().collect();
    winners.sort_by_key(|(_, job)| job.seq);
    for (path, job) in winners {
        if last_written
            .get(&path)
            .is_some_and(|&written| job.seq <= written)
        {
            ack_ok(job);
            continue;
        }
        let result = write_one_job(&path, job.seq, &job.bytes, rename_hook);
        if result.is_ok() {
            last_written.insert(path.clone(), job.seq);
        } else if job.ack.is_none()
            && let Err(error) = &result
            && let Ok(mut slot) = errors.lock()
        {
            slot.push((path.clone(), error.to_string()));
        }
        if let Some(ack) = job.ack {
            let _ = ack.send(result);
        }
    }
}

/// Ack a superseded or stale job: the freshest bytes won, which is success.
fn ack_ok(job: WriterJob) {
    if let Some(ack) = job.ack {
        let _ = ack.send(Ok(()));
    }
}

/// Land one job: per-job unique temp file, then rename over the sidecar.
///
/// The temp lives beside the sidecar so the rename is atomic on both lanes; a
/// load concurrent with a write sees the prior bytes or the new ones, never
/// torn ones. A failed rename removes its temp, best-effort, so failures do
/// not litter the project directory.
fn write_one_job(
    sidecar_path: &Path,
    seq: u64,
    bytes: &[u8],
    rename_hook: &RenameHook,
) -> io::Result<()> {
    let mut temp = sidecar_path.as_os_str().to_owned();
    temp.push(format!(".{seq}.tmp"));
    let temp = PathBuf::from(temp);
    fs::write(&temp, bytes)?;
    if let Ok(slot) = rename_hook.lock()
        && let Some(hook) = slot.clone()
    {
        hook();
    }
    if let Err(error) = fs::rename(&temp, sidecar_path) {
        let _ = fs::remove_file(&temp);
        return Err(error);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::time::{Instant, SystemTime};

    use kinewright_core::IncidentLog;
    use kinewright_media::test_support::TempDirectory;

    use super::*;

    /// Three records through the production builder: observed, never
    /// hand-built, so the shapes under test are shapes the writer emits.
    fn three_records() -> Vec<IncidentRecord> {
        let mut log = IncidentLog::with_start(Instant::now(), Some(SystemTime::UNIX_EPOCH));
        for n in 0..3 {
            log.observe(IncidentObservation::plain(
                IncidentCode::Label(LabelIncident::Project),
                IncidentSubject::Project,
                format!("in2b sidecar record {n}"),
                TimelineRevision::default(),
            ));
        }
        let (records, report) = log.records(None, &BTreeMap::new());
        assert_eq!(records.len(), 3);
        assert_eq!(report.written_open, 3);
        records
    }

    #[test]
    fn sidecar_paths_derive_from_the_stem_beside_the_project() {
        assert_eq!(sidecar_path_for_project(None), None);
        assert_eq!(
            sidecar_path_for_project(Some(Path::new("/tmp/x/edit.kinewright"))),
            Some(PathBuf::from("/tmp/x/edit.kinewright-incidents"))
        );
        // The stem, not the extension, decides — like the LUT store.
        assert_eq!(
            sidecar_path_for_project(Some(Path::new("/tmp/x/edit.json"))),
            Some(PathBuf::from("/tmp/x/edit.kinewright-incidents"))
        );
        assert_eq!(
            sidecar_path_for_project(Some(Path::new("edit.kinewright"))),
            Some(PathBuf::from("edit.kinewright-incidents"))
        );
        assert_eq!(sidecar_path_for_project(Some(Path::new("/"))), None);
    }

    #[test]
    fn digest_is_sixteen_lowercase_hex_over_fnv() {
        // The empty input pins the offset basis — a wrong polynomial fails.
        assert_eq!(digest_bytes(b""), "cbf29ce484222325");
        let digest = digest_bytes(b"{}");
        assert_eq!(digest.len(), 16);
        assert!(
            digest
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
            "lowercase hex: {digest}"
        );
        assert_ne!(digest_bytes(b"{}"), digest_bytes(b"{ }"));
    }

    #[test]
    fn missing_sidecar_is_absent_and_versionless_is_corrupt() {
        let temp = TempDirectory::new("in2b-sidecar-arms");
        assert_eq!(
            load_sidecar(&temp.path("missing.kinewright-incidents")),
            SidecarLoad::Absent
        );
        let versionless = temp.path("versionless.kinewright-incidents");
        fs::write(
            &versionless,
            r#"{"project_digest":"aa","previous_digest":"aa","records":[]}"#,
        )
        .expect("fixture writes");
        assert_eq!(
            load_sidecar(&versionless),
            SidecarLoad::Corrupt("missing format_version".to_owned())
        );
    }

    #[test]
    fn oversize_format_version_reads_as_corrupt_br40() {
        // BR40 (N4.1): a sidecar `format_version` too large for `u32` reads
        // as `Corrupt`, not `Newer` — it is malformed input to the envelope's
        // `u32` field, and `Corrupt` preserves the bytes via `.bak` exactly
        // as `Newer` does.
        let temp = TempDirectory::new("in2b-sidecar-br40");
        let path = temp.path("oversize.kinewright-incidents");
        fs::write(
            &path,
            r#"{"format_version":99999999999,"project_digest":"aa","previous_digest":"aa","records":[]}"#,
        )
        .expect("fixture writes");
        let loaded = load_sidecar(&path);
        assert!(
            matches!(loaded, SidecarLoad::Corrupt(_)),
            "oversize reads as Corrupt, got {loaded:?}"
        );
        // A merely newer version still reads as Newer.
        fs::write(
            &path,
            r#"{"format_version":999,"project_digest":"aa","previous_digest":"aa","records":[]}"#,
        )
        .expect("fixture writes");
        assert_eq!(load_sidecar(&path), SidecarLoad::Newer(999));
    }

    #[test]
    fn unparseable_and_unknown_records_carry_verbatim_with_id_floor() {
        let temp = TempDirectory::new("in2b-sidecar-carry");
        let records = three_records();
        let good_text = serde_json::to_string(&records[0]).expect("record serialises");
        let mut unknown = records[1].clone();
        unknown.code = "no_such_code_xyz".to_owned();
        let unknown_text = serde_json::to_string(&unknown).expect("record serialises");
        // Unparseable as `IncidentRecord` (missing fields), carrying a high id.
        let bad_text = r#"{"id": 9000, "bogus": [1, 2, {"nested": true}]}"#;
        let digest = digest_bytes(b"project-bytes");
        let envelope = format!(
            "{{\n  \"format_version\": 1,\n  \"project_digest\": \"{digest}\",\n  \"previous_digest\": \"{digest}\",\n  \"records\": [\n    {good_text},\n    {bad_text},\n    {unknown_text}\n  ]\n}}"
        );
        let path = temp.path("carry.kinewright-incidents");
        fs::write(&path, &envelope).expect("fixture writes");

        let SidecarLoad::Current(loaded) = load_sidecar(&path) else {
            panic!("one bad record does not refuse the file");
        };
        // The good record and the unknown-code record both parse; the
        // unknown-code record still reaches `restore` for the aggregate (F3).
        assert_eq!(loaded.records.len(), 2);
        assert_eq!(loaded.records[1].code, "no_such_code_xyz");
        // Both carried texts are byte-equal to their source slices.
        assert_eq!(loaded.carried, vec![bad_text.to_owned(), unknown_text]);
        // The floor covers every carried id, parsed or not (F2).
        assert_eq!(loaded.id_floor, Some(9000));
        assert!(sidecar_matches_project(&loaded, &digest));
        assert!(!sidecar_matches_project(&loaded, "0000000000000000"));
    }

    #[test]
    fn carried_texts_reemit_verbatim_and_consecutive_writes_match() {
        let records = three_records();
        // Element slices as a read produces them: inner whitespace kept, no
        // padding (`from_string` trims padding, but read-side raws never
        // carry any — padding between elements belongs to no value).
        let carried = vec![
            "{\"id\": 7,\n   \"bogus\": true}".to_owned(),
            "[1,2]".to_owned(),
        ];
        let first =
            build_sidecar_bytes(&records[..1], &carried, "aabbcc", "aabbcc").expect("builds");
        let text = String::from_utf8(first.clone()).expect("sidecars are UTF-8");
        for raw in &carried {
            assert!(
                text.contains(raw),
                "carried bytes appear verbatim in the rewritten sidecar"
            );
        }
        let second =
            build_sidecar_bytes(&records[..1], &carried, "aabbcc", "aabbcc").expect("rebuilds");
        assert_eq!(first, second, "consecutive writes are byte-identical");
    }

    #[test]
    fn first_free_bak_never_overwrites() {
        let temp = TempDirectory::new("in2b-sidecar-bak");
        let path = temp.path("edit.kinewright-incidents");
        fs::write(&path, b"refused once").expect("fixture writes");
        let bak = refuse_sidecar(&path).expect("first refusal renames");
        assert_eq!(bak.extension().and_then(|e| e.to_str()), Some("bak"));
        assert!(!path.exists());
        assert_eq!(fs::read(&bak).expect("bak reads"), b"refused once");

        fs::write(&path, b"refused twice").expect("fixture writes");
        let bak1 = refuse_sidecar(&path).expect("second refusal renames");
        assert!(bak1.to_string_lossy().ends_with(".bak.1"), "{bak1:?}");
        assert_eq!(
            fs::read(&bak).expect("first bak reads"),
            b"refused once",
            "the second refusal leaves the first .bak byte-identical"
        );
        assert_eq!(fs::read(&bak1).expect("second bak reads"), b"refused twice");
    }

    #[test]
    fn writer_lands_the_latest_per_path_and_reports_async_failures() {
        let temp = TempDirectory::new("in2b-sidecar-writer");
        let writer = SidecarWriter::new();
        let path = temp.path("edit.kinewright-incidents");
        // A stale async snapshot queued ahead of a joining save: the save's
        // bytes win whatever the thread interleaving.
        writer.submit(path.clone(), b"stale".to_vec());
        writer
            .submit_and_join(path.clone(), b"fresh".to_vec())
            .expect("the joining write lands");
        assert_eq!(fs::read(&path).expect("sidecar reads"), b"fresh");

        // An async failure queues for the frame thread instead of failing a
        // caller that already returned: the sync job behind it forces the
        // ordering, so the drain below is deterministic.
        let blocked = temp.path("blocked.kinewright-incidents");
        fs::create_dir(&blocked).expect("the sidecar path is occupied");
        writer.submit(blocked.clone(), b"nope".to_vec());
        writer
            .submit_and_join(temp.path("other.kinewright-incidents"), b"ok".to_vec())
            .expect("the later job lands");
        let errors = writer.take_errors();
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].0, blocked);
        assert!(writer.take_errors().is_empty(), "the drain takes all");
        // Failed jobs remove their temps: no litter beside the project.
        let litter: Vec<_> = fs::read_dir(temp.root())
            .expect("tempdir reads")
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| path.extension().is_some_and(|e| e == "tmp"))
            .collect();
        assert!(litter.is_empty(), "no temp litter remains: {litter:?}");
    }

    #[test]
    fn refused_and_write_failed_observations_are_transient_plain_notes() {
        for observation in [
            sidecar_refused_observation("refused", TimelineRevision::default()),
            sidecar_write_failed_observation("failed", TimelineRevision::default()),
        ] {
            assert!(observation.transient, "every §5 note is transient");
            assert_eq!(observation.subject, IncidentSubject::Project);
            assert_eq!(
                observation.evidence,
                kinewright_core::IncidentEvidence::Plain
            );
        }
        assert_eq!(
            sidecar_refused_observation("r", TimelineRevision::default()).code,
            IncidentCode::Label(LabelIncident::SidecarRefused)
        );
        assert_eq!(
            sidecar_write_failed_observation("r", TimelineRevision::default()).code,
            IncidentCode::Label(LabelIncident::SidecarWriteFailed)
        );
    }
}
