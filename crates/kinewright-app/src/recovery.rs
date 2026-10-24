//! Crash recovery for the operation spine.
//!
//! Journals live under `%LOCALAPPDATA%/Kinewright/recovery/` (with the system
//! temporary directory as a fallback when `LOCALAPPDATA` is unavailable),
//! ONE FILE PER PROJECT: saved projects journal to a name derived from their
//! path (`<stem>-<fnv64>.journal`, stable across builds so a crashed project
//! finds its journal again), unsaved projects to `unsaved-N.journal`. Each
//! file is a versioned preamble, a complete initial `Document` JSON line
//! (whose header also records the owning project's path), then one
//! `JournalCommand` JSON line per accepted Core change. A trailing newline is
//! the commit marker for each line.
//!
//! Startup scans the whole directory and offers every found journal for
//! restore or discard individually - a crash with several projects open
//! loses none of them, and a journal can never replay into the wrong
//! project because its identity travels in its own header. A journal that
//! would collide with an undecided pending file allocates a suffixed name
//! instead: pending crash data is never truncated by a new session.
//!
//! Every committed command is flushed and synced before the recorder accepts
//! another event. A process crash can therefore lose one or more of the newest
//! commands accepted by Core but still waiting in the subscriber queue. Every
//! event already processed by the recorder is durable subject to the storage
//! device honoring `sync_data`.

use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    thread::{self, JoinHandle},
    time::Duration,
};

use crossbeam_channel::{Receiver, Sender, select, unbounded};
use eframe::egui;
use kinewright_core::{
    Core, Document, Event, IncidentCode, IncidentObservation, IncidentSubject, JournalCommand,
    LabelIncident, PROJECT_FORMAT_VERSION, TimelineRevision,
};
use kinewright_project::{
    JOURNAL_MAGIC as MAGIC, allocate_journal_path, canonical_project_identity,
    default_recovery_directory,
};
use serde::{Deserialize, Serialize};

use crate::theme::{self, type_size};

const FORMAT_VERSION: u32 = 1;
const CORE_RESPONSE_TIMEOUT: Duration = Duration::from_secs(2);

#[cfg(test)]
thread_local! {
    static JA_DIRECTORY: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

/// Bind the real GUI recorder to the fixture's private directory, on this
/// constructor thread only. No shipping path reads this override.
#[cfg(test)]
pub(crate) fn with_ja_directory<T>(directory: PathBuf, run: impl FnOnce() -> T) -> T {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            JA_DIRECTORY.with_borrow_mut(|p| *p = None);
        }
    }
    JA_DIRECTORY.with_borrow_mut(|p| {
        assert!(p.is_none());
        *p = Some(directory);
    });
    let _reset = Reset;
    run()
}

/// The default project version: journals written before Part B read as 1
/// (`IN2B` §4 rule 5, N-4 — a named fn, not a bare `default`).
fn v1() -> u32 {
    1
}

#[derive(Debug, Serialize, Deserialize)]
struct JournalHeader {
    format_version: u32,
    /// The owning project's save path; `None` for unsaved projects. Optional
    /// with a default so journals from before multi-project still parse.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    project_path: Option<PathBuf>,
    /// The project `format_version` the writing build stamped: its own
    /// `PROJECT_FORMAT_VERSION` const, not the session's read version — the
    /// journal describes the bytes this build wrote (`IN2B` §4 rule 5, N-4).
    /// MO2 R7: the maximum supported, never the initial snapshot's minimum —
    /// later operations may need features the snapshot lacks.
    /// The journal's own strict `format_version` gate above is unchanged:
    /// the two versions answer different questions ("can I parse this
    /// journal?" vs "can I run this project?").
    #[serde(default = "v1")]
    writer_format_version: u32,
    initial_document: Document,
}

struct JournalWriter {
    authority: Option<kinewright_project::LockfileHandle>,
    _directory: kinewright_project::legacy_io::PinnedDirectory,
    file: File,
    sync_each_record: bool,
}

impl JournalWriter {
    #[cfg(test)]
    fn create(
        path: &Path,
        project_path: Option<&Path>,
        initial_document: &Document,
    ) -> Result<Self, String> {
        Self::create_with_sync(path, project_path, initial_document, true)
    }

    #[cfg(test)]
    fn create_with_sync(
        path: &Path,
        project_path: Option<&Path>,
        initial_document: &Document,
        sync_each_record: bool,
    ) -> Result<Self, String> {
        let authority = project_path
            .map(|project| {
                kinewright_project::lockfile::acquire_gui_project_lock(
                    project,
                    path.parent().unwrap_or(Path::new(".")),
                )
                .map(|owner| owner.handle)
            })
            .transpose()
            .map_err(|e| e.to_string())?;
        Self::create_owned(
            path,
            project_path,
            initial_document,
            sync_each_record,
            authority,
            None,
        )
    }

    fn create_owned(
        path: &Path,
        project_path: Option<&Path>,
        initial_document: &Document,
        sync_each_record: bool,
        authority: Option<kinewright_project::LockfileHandle>,
        directory: Option<kinewright_project::legacy_io::PinnedDirectory>,
    ) -> Result<Self, String> {
        if let Some(writer) = &authority {
            writer.verify().map_err(|e| e.to_string())?;
        }
        if project_path.is_some() && authority.is_none() {
            return Err("named journal requires writer authority".into());
        }
        let directory = if let Some(directory) = directory {
            directory
        } else {
            let parent = path.parent().unwrap_or(Path::new("."));
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            kinewright_project::legacy_io::PinnedDirectory::open(parent)
                .map_err(|e| e.to_string())?
        };
        let owned_path = directory
            .io_path()
            .join(path.file_name().ok_or("journal filename")?);
        let mut options = OpenOptions::new();
        options.create_new(true).write(true);
        // LockFileEx would prevent read-only journal inspection on Windows.
        // Deny deletion while the recorder lives; named writes also hold the project lock.
        #[cfg(windows)]
        std::os::windows::fs::OpenOptionsExt::share_mode(&mut options, 3);
        let mut file = options.open(&owned_path).map_err(|error| {
            format!(
                "could not create recovery journal {}: {error}",
                path.display()
            )
        })?;
        kinewright_project::legacy_io::lock_journal(&file).map_err(|e| e.to_string())?;
        // G5: new headers persist the absolute canonical identity, so the
        // takeover scan never has to guess which spelling a relative header
        // meant — legacy relative headers stay readable but never claim.
        let header = serde_json::to_vec(&JournalHeader {
            format_version: FORMAT_VERSION,
            // H5: a path with no identity writes no header path (never claims).
            project_path: authority
                .as_ref()
                .map(|writer| writer.resolved_identity().to_path_buf()),
            writer_format_version: PROJECT_FORMAT_VERSION,
            initial_document: initial_document.clone(),
        })
        .map_err(|error| format!("could not serialize recovery snapshot: {error}"))?;
        file.write_all(MAGIC)
            .and_then(|()| file.write_all(&header))
            .and_then(|()| file.write_all(b"\n"))
            .and_then(|()| file.flush())
            .and_then(|()| file.sync_all())
            .map_err(|error| format!("could not initialize recovery journal: {error}"))?;
        Ok(Self {
            authority,
            _directory: directory,
            file,
            sync_each_record,
        })
    }

    fn append(&mut self, command: &JournalCommand) -> Result<(), String> {
        if let Some(writer) = &self.authority {
            writer.verify().map_err(|e| e.to_string())?;
        }
        let encoded = serde_json::to_vec(command)
            .map_err(|error| format!("could not serialize recovery command: {error}"))?;
        self.file
            .write_all(&encoded)
            .and_then(|()| self.file.write_all(b"\n"))
            .and_then(|()| self.file.flush())
            .and_then(|()| {
                if self.sync_each_record {
                    self.file.sync_data()
                } else {
                    Ok(())
                }
            })
            .map_err(|error| format!("could not persist recovery command: {error}"))
    }
}

#[derive(Debug, Clone)]
struct Damage {
    offset: usize,
    ignored_bytes: usize,
    reason: String,
}

#[derive(Debug)]
struct ParsedCommand {
    command: JournalCommand,
    start: usize,
    #[cfg(test)]
    end: usize,
}

#[derive(Debug)]
struct ParsedJournal {
    project_path: Option<PathBuf>,
    initial_document: Document,
    writer_format_version: u32,
    commands: Vec<ParsedCommand>,
    #[cfg(test)]
    header_end: usize,
    damage: Option<Damage>,
    byte_len: usize,
}

#[derive(Debug, Clone)]
struct RecoveryReport {
    project_path: Option<PathBuf>,
    document: Document,
    writer_format_version: u32,
    recovered_commands: usize,
    damage: Option<Damage>,
}

#[allow(clippy::large_enum_variant)]
enum Inspection {
    Missing,
    Recoverable(RecoveryReport),
    Unusable(Damage),
}

fn parse_journal(bytes: &[u8]) -> Result<ParsedJournal, Damage> {
    if !bytes.starts_with(MAGIC) {
        let reason = if MAGIC.starts_with(bytes) {
            "the journal preamble was truncated"
        } else {
            "the journal preamble or format version is invalid"
        };
        return Err(Damage {
            offset: 0,
            ignored_bytes: bytes.len(),
            reason: reason.to_owned(),
        });
    }

    let Some((header_line, header_end)) = complete_line(bytes, MAGIC.len()) else {
        return Err(Damage {
            offset: MAGIC.len(),
            ignored_bytes: bytes.len().saturating_sub(MAGIC.len()),
            reason: "the initial document snapshot was truncated".to_owned(),
        });
    };
    let header: JournalHeader = serde_json::from_slice(header_line).map_err(|error| Damage {
        offset: MAGIC.len(),
        ignored_bytes: bytes.len().saturating_sub(MAGIC.len()),
        reason: format!("the initial document snapshot is corrupt: {error}"),
    })?;
    if header.format_version != FORMAT_VERSION {
        return Err(Damage {
            offset: MAGIC.len(),
            ignored_bytes: bytes.len().saturating_sub(MAGIC.len()),
            reason: format!(
                "journal format {} is not supported by this build",
                header.format_version
            ),
        });
    }
    if let Err(error) = header.initial_document.validate() {
        return Err(Damage {
            offset: MAGIC.len(),
            ignored_bytes: bytes.len().saturating_sub(MAGIC.len()),
            reason: format!("the initial document snapshot is invalid: {error}"),
        });
    }

    let mut commands = Vec::new();
    let mut cursor = header_end;
    let mut damage = None;
    while cursor < bytes.len() {
        let start = cursor;
        let Some((line, end)) = complete_line(bytes, start) else {
            damage = Some(Damage {
                offset: start,
                ignored_bytes: bytes.len() - start,
                reason: "the last recovery command was truncated before its commit marker"
                    .to_owned(),
            });
            break;
        };
        match serde_json::from_slice::<JournalCommand>(line) {
            Ok(command) => commands.push(ParsedCommand {
                command,
                start,
                #[cfg(test)]
                end,
            }),
            Err(error) => {
                damage = Some(Damage {
                    offset: start,
                    ignored_bytes: bytes.len() - start,
                    reason: format!("a recovery command is corrupt: {error}"),
                });
                break;
            }
        }
        cursor = end;
    }

    Ok(ParsedJournal {
        project_path: header.project_path,
        initial_document: header.initial_document,
        writer_format_version: header.writer_format_version,
        commands,
        #[cfg(test)]
        header_end,
        damage,
        byte_len: bytes.len(),
    })
}

fn complete_line(bytes: &[u8], start: usize) -> Option<(&[u8], usize)> {
    let relative_end = bytes.get(start..)?.iter().position(|byte| *byte == b'\n')?;
    let line_end = start + relative_end;
    Some((&bytes[start..line_end], line_end + 1))
}

fn inspect_bytes(bytes: &[u8]) -> Inspection {
    let parsed = match parse_journal(bytes) {
        Ok(parsed) => parsed,
        Err(damage) => return Inspection::Unusable(damage),
    };
    replay(parsed)
}

fn inspect_path(path: &Path) -> Inspection {
    match fs::read(path) {
        Ok(bytes) => inspect_bytes(&bytes),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Inspection::Missing,
        Err(error) => Inspection::Unusable(Damage {
            offset: 0,
            ignored_bytes: fs::metadata(path)
                .ok()
                .and_then(|metadata| usize::try_from(metadata.len()).ok())
                .unwrap_or_default(),
            reason: format!("the recovery journal could not be read: {error}"),
        }),
    }
}

fn replay(parsed: ParsedJournal) -> Inspection {
    let core = match Core::spawn(parsed.initial_document.clone()) {
        Ok(core) => core,
        Err(error) => {
            return Inspection::Unusable(Damage {
                offset: MAGIC.len(),
                ignored_bytes: parsed.byte_len.saturating_sub(MAGIC.len()),
                reason: format!("the initial recovery snapshot was rejected: {error}"),
            });
        }
    };
    let project_path = parsed.project_path;
    let mut document = parsed.initial_document;
    let mut recovered_commands = 0;
    let mut damage = parsed.damage;
    for record in parsed.commands {
        let expected = record.command.clone();
        let event = match core.request(record.command.into()) {
            Ok(event) => event,
            Err(error) => {
                damage = Some(Damage {
                    offset: record.start,
                    ignored_bytes: parsed.byte_len - record.start,
                    reason: format!("Core stopped during recovery replay: {error}"),
                });
                break;
            }
        };
        match event {
            Event::DocumentChanged {
                doc,
                journal_command: Some(actual),
                ..
            } if actual == expected => {
                document = (*doc).clone();
                recovered_commands += 1;
            }
            Event::OpRejected { error, .. } => {
                damage = Some(Damage {
                    offset: record.start,
                    ignored_bytes: parsed.byte_len - record.start,
                    reason: format!("a journaled operation was rejected during replay: {error}"),
                });
                break;
            }
            Event::BatchRejected { error, .. } => {
                damage = Some(Damage {
                    offset: record.start,
                    ignored_bytes: parsed.byte_len - record.start,
                    reason: format!("a journaled edit plan was rejected during replay: {error}"),
                });
                break;
            }
            _ => {
                damage = Some(Damage {
                    offset: record.start,
                    ignored_bytes: parsed.byte_len - record.start,
                    reason: "Core returned an unexpected event during recovery replay".to_owned(),
                });
                break;
            }
        }
    }
    Inspection::Recoverable(RecoveryReport {
        project_path,
        document,
        writer_format_version: parsed.writer_format_version,
        recovered_commands,
        damage,
    })
}

enum RecorderControl {
    #[cfg(test)]
    Flush(Sender<()>),
    Stop(Sender<()>),
}

struct Recorder {
    control: Sender<RecorderControl>,
    worker: Option<JoinHandle<()>>,
}

impl Recorder {
    #[cfg(test)]
    fn start(
        core: &Core,
        path: &Path,
        project_path: Option<&Path>,
        runtime_error: Arc<Mutex<Option<String>>>,
    ) -> Result<Self, String> {
        let authority = project_path
            .map(|project| {
                kinewright_project::lockfile::acquire_gui_project_lock(
                    project,
                    path.parent().unwrap_or(Path::new(".")),
                )
                .map(|owner| owner.handle)
            })
            .transpose()
            .map_err(|e| e.to_string())?;
        Self::start_owned(core, path, project_path, runtime_error, authority, None)
    }

    fn start_owned(
        core: &Core,
        path: &Path,
        project_path: Option<&Path>,
        runtime_error: Arc<Mutex<Option<String>>>,
        authority: Option<kinewright_project::LockfileHandle>,
        directory: Option<kinewright_project::legacy_io::PinnedDirectory>,
    ) -> Result<Self, String> {
        let events = core
            .subscribe()
            .map_err(|error| format!("could not subscribe recovery recorder to Core: {error}"))?;
        let initial_document = match events.recv_timeout(CORE_RESPONSE_TIMEOUT) {
            Ok(Event::DocumentChanged {
                doc,
                journal_command: None,
                ..
            }) => (*doc).clone(),
            Ok(_) => return Err("Core returned an unexpected initial recovery event".to_owned()),
            Err(error) => {
                return Err(format!(
                    "timed out waiting for Core's initial recovery snapshot: {error}"
                ));
            }
        };
        let writer = JournalWriter::create_owned(
            path,
            project_path,
            &initial_document,
            true,
            authority,
            directory,
        )?;
        let (control, controls) = unbounded();
        let worker = thread::Builder::new()
            .name("kinewright-recovery".to_owned())
            .spawn(move || recorder_loop(writer, &events, &controls, &runtime_error))
            .map_err(|error| format!("could not start recovery recorder: {error}"))?;
        Ok(Self {
            control,
            worker: Some(worker),
        })
    }

    /// Drain every queued Core event into the journal.
    ///
    /// Tests wait for the worker's acknowledgement with no deadline: the
    /// work is already bounded by the commands the test sent, and a flat
    /// two-second timeout is what flakes on a loaded CI box.
    #[cfg(test)]
    fn flush(&self) -> Result<(), String> {
        let (acknowledge, acknowledged) = unbounded();
        self.control
            .send(RecorderControl::Flush(acknowledge))
            .map_err(|_| "recovery recorder stopped before flush".to_owned())?;
        acknowledged
            .recv()
            .map_err(|_| "recovery recorder stopped before flush completed".to_owned())
    }

    fn shutdown(&mut self) {
        let Some(worker) = self.worker.take() else {
            return;
        };
        let (acknowledge, acknowledged) = unbounded();
        if self
            .control
            .send(RecorderControl::Stop(acknowledge))
            .is_ok()
        {
            let _ = acknowledged.recv_timeout(CORE_RESPONSE_TIMEOUT);
        }
        let _ = worker.join();
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn recorder_loop(
    mut writer: JournalWriter,
    events: &Receiver<Event>,
    controls: &Receiver<RecorderControl>,
    runtime_error: &Arc<Mutex<Option<String>>>,
) {
    loop {
        select! {
            recv(controls) -> message => match message {
                #[cfg(test)]
                Ok(RecorderControl::Flush(acknowledge)) => {
                    if !drain_events(events, &mut writer, runtime_error) {
                        let _ = acknowledge.send(());
                        break;
                    }
                    let _ = acknowledge.send(());
                }
                Ok(RecorderControl::Stop(acknowledge)) => {
                    let _ = drain_events(events, &mut writer, runtime_error);
                    let _ = acknowledge.send(());
                    break;
                }
                Err(_) => break,
            },
            recv(events) -> event => match event {
                Ok(event) => {
                    if !record_event(&mut writer, event, runtime_error) {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    }
}

fn drain_events(
    events: &Receiver<Event>,
    writer: &mut JournalWriter,
    runtime_error: &Arc<Mutex<Option<String>>>,
) -> bool {
    while let Ok(event) = events.try_recv() {
        if !record_event(writer, event, runtime_error) {
            return false;
        }
    }
    true
}

fn record_event(
    writer: &mut JournalWriter,
    event: Event,
    runtime_error: &Arc<Mutex<Option<String>>>,
) -> bool {
    let Event::DocumentChanged {
        journal_command: Some(command),
        ..
    } = event
    else {
        return true;
    };
    if let Err(error) = writer.append(&command) {
        *lock_unpoisoned(runtime_error) = Some(error);
        return false;
    }
    true
}

fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[allow(clippy::large_enum_variant)]
enum PendingState {
    Recoverable(RecoveryReport),
    Unusable(Damage),
}

/// One journal found on disk at startup, awaiting a restore/discard decision.
struct PendingJournal {
    directory: Option<kinewright_project::legacy_io::PinnedDirectory>,
    sha256: Option<String>,
    journal_path: PathBuf,
    project_path: Option<PathBuf>,
    state: PendingState,
}

/// Site-modal note 1 (`IN2B` §7 rule 6): modal 1's damage label over
/// the project. Pure: the app edges first-render-per-run, this fn only
/// shapes.
pub(crate) fn recovery_damage_observation(
    damage: &str,
    revision: TimelineRevision,
) -> IncidentObservation {
    let mut observation = IncidentObservation::plain(
        IncidentCode::Label(LabelIncident::RecoveryDamage),
        IncidentSubject::Project,
        format!("Recovery: {damage}"),
        revision,
    );
    observation.transient = true;
    observation
}

/// Site-modal note 2 (`IN2B` §7 rule 6): modal 2's message over the
/// project. Pure: the app edges first-render-per-run, this fn only
/// shapes.
pub(crate) fn recovery_unavailable_observation(
    message: &str,
    revision: TimelineRevision,
) -> IncidentObservation {
    let mut observation = IncidentObservation::plain(
        IncidentCode::Label(LabelIncident::RecoveryUnavailable),
        IncidentSubject::Project,
        format!("Recovery: {message}"),
        revision,
    );
    observation.transient = true;
    observation
}

/// What one `show_dialog` frame decided and rendered.
pub(crate) struct RecoveryDialogOutcome {
    /// The restore the person chose, if any.
    pub(crate) restore: Option<RestoreRequest>,
    /// The first damage label modal 1 rendered this frame, if any
    /// (`IN2B` §7 rule 6).
    pub(crate) damage_rendered: Option<String>,
    /// The message modal 2 rendered this frame, if any (`IN2B` §7 rule 6).
    pub(crate) unavailable_rendered: Option<String>,
}

/// A restore the user chose from the recovery dialog. The pending file
/// survives until `consume_pending` confirms the restore actually landed.
pub(crate) struct RestoreRequest {
    pub(crate) document: Document,
    pub(crate) project_path: Option<PathBuf>,
    pub(crate) journal_path: PathBuf,
    /// The journal's writer version (`IN2B` §4 rule 5): the restore is
    /// refused when it exceeds this build's const, before the document is
    /// trusted.
    pub(crate) writer_format_version: u32,
}

/// App-owned recovery lifecycle. All filesystem and UI behavior stays here.
pub(crate) struct Recovery {
    authority: Option<kinewright_project::LockfileHandle>,
    directory_owner: Option<kinewright_project::legacy_io::PinnedDirectory>,
    directory: PathBuf,
    path: PathBuf,
    recorder: Option<Recorder>,
    pending: Vec<PendingJournal>,
    runtime_error: Arc<Mutex<Option<String>>>,
}

impl Recovery {
    pub(crate) fn read_only() -> Self {
        Self {
            authority: None,
            directory_owner: None,
            directory: PathBuf::new(),
            path: PathBuf::new(),
            recorder: None,
            pending: vec![],
            runtime_error: Arc::new(Mutex::new(None)),
        }
    }

    pub(crate) fn start(core: &Core, project_path: Option<&Path>) -> Self {
        #[cfg(test)]
        if let Some(directory) = JA_DIRECTORY.with_borrow(Clone::clone) {
            return Self::start_in(directory, core, project_path);
        }
        Self::start_in(default_recovery_directory(), core, project_path)
    }

    /// Start a per-project recorder without repeating the process-startup
    /// recovery scan. Only the startup session owns restore decisions.
    pub(crate) fn start_attached(core: &Core, project_path: Option<&Path>) -> Self {
        #[cfg(test)]
        if let Some(directory) = JA_DIRECTORY.with_borrow(Clone::clone) {
            return Self::start_attached_in(directory, core, project_path);
        }
        Self::start_attached_in(default_recovery_directory(), core, project_path)
    }

    #[cfg(test)]
    pub(crate) fn ja_observe(&self) -> Result<(Vec<u8>, serde_json::Value), String> {
        self.recorder
            .as_ref()
            .ok_or("GUI recorder unavailable")?
            .flush()?;
        if let Some(error) = lock_unpoisoned(&self.runtime_error).clone() {
            return Err(error);
        }
        let bytes = kinewright_agent::ja_harness::protocol::read_bounded(
            &self
                .directory_owner
                .as_ref()
                .ok_or("journal directory unavailable")?
                .io_path()
                .join(self.path.file_name().ok_or("journal filename")?),
            64 * 1024 * 1024,
        )?;
        let parsed = parse_journal(&bytes).map_err(|e| e.reason)?;
        let observed = serde_json::json!({"path":self.path,"writer_format_version":parsed.writer_format_version,
            "initial_document":parsed.initial_document,"commands":parsed.commands.iter().map(|c| &c.command).collect::<Vec<_>>(),
            "damage":parsed.damage.map(|d| d.reason),"gui_post_scan_exclusion":"pending_baseline_witness"});
        Ok((bytes, observed))
    }

    fn start_in(directory: PathBuf, core: &Core, project_path: Option<&Path>) -> Self {
        let pending = scan_directory(&directory);
        Self::start_with_pending(directory, core, project_path, pending)
    }

    fn start_attached_in(directory: PathBuf, core: &Core, project_path: Option<&Path>) -> Self {
        Self::start_with_pending(directory, core, project_path, Vec::new())
    }

    fn start_with_pending(
        directory: PathBuf,
        core: &Core,
        project_path: Option<&Path>,
        pending: Vec<PendingJournal>,
    ) -> Self {
        let runtime_error = Arc::new(Mutex::new(None));
        let mut recovery = Self {
            authority: None,
            directory_owner: None,
            path: directory.join("uninitialized.journal"),
            directory,
            recorder: None,
            pending,
            runtime_error,
        };
        recovery.attach(core, project_path);
        recovery
    }

    /// Finish the previous project's journal and begin from Core's authoritative
    /// current snapshot. Core's initial subscription event makes this race-free
    /// with edits from the agent server.
    pub(crate) fn attach(&mut self, core: &Core, project_path: Option<&Path>) {
        let writer = match self.writer_for(project_path) {
            Ok(writer) => writer,
            Err(error) => {
                *lock_unpoisoned(&self.runtime_error) = Some(error);
                return;
            }
        };
        self.attach_owned(core, project_path, writer);
    }

    pub(crate) fn writer(&self) -> Option<kinewright_project::LockfileHandle> {
        self.authority.clone()
    }

    pub(crate) fn writer_for(
        &self,
        path: Option<&Path>,
    ) -> Result<Option<kinewright_project::LockfileHandle>, String> {
        let Some(path) = path else {
            return Ok(None);
        };
        let identity = canonical_project_identity(path).map_err(|e| e.to_string())?;
        if let Some(writer) = &self.authority
            && writer.resolved_identity() == identity
        {
            writer.verify().map_err(|e| e.to_string())?;
            return Ok(Some(writer.clone()));
        }
        kinewright_project::lockfile::acquire_gui_project_lock(path, &self.directory)
            .map(|owner| Some(owner.handle))
            .map_err(|e| e.to_string())
    }

    pub(crate) fn checkpoint_owned(
        &mut self,
        core: &Core,
        project_path: Option<&Path>,
        writer: Option<kinewright_project::LockfileHandle>,
    ) {
        self.attach_owned(core, project_path, writer);
    }

    fn attach_owned(
        &mut self,
        core: &Core,
        project_path: Option<&Path>,
        writer: Option<kinewright_project::LockfileHandle>,
    ) {
        if project_path.is_some() && writer.is_none() {
            *lock_unpoisoned(&self.runtime_error) =
                Some("named journal requires writer authority".into());
            return;
        }
        if writer
            .as_ref()
            .is_some_and(|writer| writer.verify().is_err())
        {
            *lock_unpoisoned(&self.runtime_error) =
                Some("journal writer authority was lost".into());
            return;
        }
        #[cfg(test)]
        if let Some(path) = project_path {
            kinewright_project::io_observer::mutation_attempt("journal_attach_enter", path);
        }
        let identity = writer
            .as_ref()
            .map(|writer| writer.resolved_identity().to_path_buf());
        let project_path = identity.as_deref().or(project_path);
        if self.directory_owner.is_none() {
            if let Err(error) = fs::create_dir_all(&self.directory) {
                *lock_unpoisoned(&self.runtime_error) = Some(error.to_string());
                return;
            }
            match fs::canonicalize(&self.directory)
                .and_then(|path| kinewright_project::legacy_io::PinnedDirectory::open(&path))
            {
                Ok(directory) => self.directory_owner = Some(directory),
                Err(error) => {
                    *lock_unpoisoned(&self.runtime_error) = Some(error.to_string());
                    return;
                }
            }
        }
        self.stop_active();
        let directory = self
            .directory_owner
            .as_ref()
            .expect("admitted journal directory");
        let io_directory = directory.io_path();
        let reserved_paths = self
            .pending
            .iter()
            .filter_map(|entry| {
                entry
                    .journal_path
                    .file_name()
                    .map(|name| io_directory.join(name))
            })
            .collect::<Vec<_>>();
        let reserved = reserved_paths
            .iter()
            .map(PathBuf::as_path)
            .collect::<Vec<_>>();
        let current = self
            .path
            .file_name()
            .map_or_else(PathBuf::new, |name| io_directory.join(name));
        let target = allocate_journal_path(&io_directory, project_path, &current, &reserved);
        let target = self
            .directory
            .join(target.file_name().expect("allocated journal filename"));
        self.remove_journal();
        self.path = target;
        self.authority = writer;
        *lock_unpoisoned(&self.runtime_error) = None;
        match Recorder::start_owned(
            core,
            &self.path,
            project_path,
            Arc::clone(&self.runtime_error),
            self.authority.clone(),
            self.directory_owner.clone(),
        ) {
            Ok(recorder) => {
                self.recorder = Some(recorder);
            }
            Err(error) => {
                *lock_unpoisoned(&self.runtime_error) = Some(error);
            }
        }
    }

    /// Saving establishes a new baseline (and, on first save or save-as,
    /// migrates the journal to the project's name), so a later crash only
    /// offers edits made after the successful save.
    #[cfg(test)]
    pub(crate) fn checkpoint(&mut self, core: &Core, project_path: Option<&Path>) {
        self.attach(core, project_path);
    }

    /// Render the startup decisions - one row per found journal - and return
    /// a restore request when the user picks one. The pending file survives
    /// until `consume_pending` confirms the restore landed, so a failed
    /// restore can still be recovered at the next launch.
    ///
    /// One frame fn past clippy's 100 lines: the two damage/unavailable
    /// renders plus their first-render edging live here rather than split
    /// across helpers a frame would call once each.
    #[allow(clippy::too_many_lines)]
    pub(crate) fn show_dialog(&mut self, ctx: &egui::Context) -> RecoveryDialogOutcome {
        let mut outcome = RecoveryDialogOutcome {
            restore: None,
            damage_rendered: None,
            unavailable_rendered: None,
        };
        if std::env::var_os("KINEWRIGHT_SCREENSHOT_TO").is_some() {
            self.pending.clear();
            return outcome;
        }
        let mut restore: Option<usize> = None;
        let mut discard: Option<usize> = None;
        if !self.pending.is_empty() {
            let damage_rendered = &mut outcome.damage_rendered;
            egui::Window::new("Recover unsaved work?")
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    ui.label("Kinewright closed unexpectedly with unsaved work.");
                    for (index, entry) in self.pending.iter().enumerate() {
                        ui.separator();
                        ui.label(
                            egui::RichText::new(pending_project_label(
                                entry.project_path.as_deref(),
                            ))
                            .font(theme::semibold(type_size::BODY)),
                        );
                        match &entry.state {
                            PendingState::Recoverable(report) => {
                                ui.label(format!(
                                    "{} journaled command{} can be restored.",
                                    report.recovered_commands,
                                    if report.recovered_commands == 1 {
                                        ""
                                    } else {
                                        "s"
                                    }
                                ));
                                if let Some(damage) = &report.damage {
                                    let description = damage_description(damage);
                                    ui.colored_label(egui::Color32::YELLOW, &description);
                                    if damage_rendered.is_none() {
                                        *damage_rendered = Some(description);
                                    }
                                }
                            }
                            PendingState::Unusable(damage) => {
                                ui.colored_label(
                                    egui::Color32::YELLOW,
                                    "This journal's initial snapshot is unavailable.",
                                );
                                let description = damage_description(damage);
                                ui.label(&description);
                                if damage_rendered.is_none() {
                                    *damage_rendered = Some(description);
                                }
                            }
                        }
                        ui.horizontal(|ui| {
                            if ui
                                .add_enabled(
                                    matches!(entry.state, PendingState::Recoverable(_)),
                                    egui::Button::new("Restore"),
                                )
                                .clicked()
                            {
                                restore = Some(index);
                            }
                            if ui.button("Discard").clicked() {
                                discard = Some(index);
                            }
                        });
                    }
                });
        }

        let runtime_message = lock_unpoisoned(&self.runtime_error).clone();
        let mut dismiss_runtime_error = false;
        if let Some(message) = runtime_message {
            outcome.unavailable_rendered = Some(message.clone());
            egui::Window::new("Crash recovery unavailable")
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    ui.label(message);
                    ui.label("Edits made while this warning is present may not survive a crash.");
                    if ui.button("Dismiss").clicked() {
                        dismiss_runtime_error = true;
                    }
                });
        }
        if dismiss_runtime_error {
            *lock_unpoisoned(&self.runtime_error) = None;
        }

        if let Some(index) = discard {
            if self.remove_pending(&self.pending[index]) {
                self.pending.remove(index);
            }
            return outcome;
        }
        if let Some(index) = restore {
            let entry = &self.pending[index];
            let PendingState::Recoverable(report) = &entry.state else {
                return outcome;
            };
            outcome.restore = Some(RestoreRequest {
                document: report.document.clone(),
                project_path: report.project_path.clone(),
                journal_path: entry.journal_path.clone(),
                writer_format_version: report.writer_format_version,
            });
        }
        outcome
    }

    /// A chosen restore landed: the crash journal has served its purpose.
    pub(crate) fn consume_pending(&mut self, journal_path: &Path) {
        if let Some(index) = self
            .pending
            .iter()
            .position(|entry| entry.journal_path == journal_path)
            && self.remove_pending(&self.pending[index])
        {
            self.pending.remove(index);
        }
    }

    /// Execute retirement at the actual active owner, retaining the scanned source identity.
    pub(crate) fn consume_pending_by(&mut self, journal_path: &Path, owner: &Self) {
        if let Some(index) = self
            .pending
            .iter()
            .position(|entry| entry.journal_path == journal_path)
            && owner.remove_pending(&self.pending[index])
        {
            self.pending.remove(index);
        }
    }

    fn remove_pending(&self, entry: &PendingJournal) -> bool {
        let result = (|| -> Result<(), String> {
            use std::io::Read as _;
            let project = entry.project_path.as_deref();
            let _owner = self.writer_for(project)?;
            let directory = entry
                .directory
                .as_ref()
                .ok_or("journal directory identity is unavailable")?;
            let path = directory
                .io_path()
                .join(entry.journal_path.file_name().ok_or("journal filename")?);
            if !fs::symlink_metadata(&path)
                .map_err(|e| e.to_string())?
                .is_file()
            {
                return Err("journal is not a regular file".into());
            }
            let mut file = OpenOptions::new()
                .read(true)
                .write(true)
                .open(&path)
                .map_err(|e| e.to_string())?;
            kinewright_project::legacy_io::lock_journal(&file).map_err(|e| e.to_string())?;
            let mut bytes = vec![];
            file.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
            if entry.sha256.as_deref() != Some(&kinewright_media::sha256_bytes(&bytes)) {
                return Err(
                    "journal changed after the recovery scan; scan it again before discarding"
                        .into(),
                );
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt as _;
                let opened = file.metadata().map_err(|e| e.to_string())?;
                let current = fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
                if opened.dev() != current.dev() || opened.ino() != current.ino() {
                    return Err("journal file identity changed".into());
                }
            }
            fs::remove_file(&path).map_err(|e| e.to_string())
        })();
        match result {
            Ok(()) => true,
            Err(error) => {
                *lock_unpoisoned(&self.runtime_error) = Some(error);
                false
            }
        }
    }

    /// A cross-version restore was refused (`IN2B` §4 rule 5): drop the
    /// pending entry so this run stops offering it, but keep the file — a
    /// newer build restores it. Unlike `consume_pending`, nothing is
    /// deleted.
    pub(crate) fn shelve_pending(&mut self, journal_path: &Path) {
        self.pending
            .retain(|entry| entry.journal_path != journal_path);
    }

    /// Preserve process-startup restore decisions when their owning project closes.
    pub(crate) fn move_pending_to(&mut self, target: &mut Self) {
        target.pending.append(&mut self.pending);
    }

    fn stop_active(&mut self) {
        if let Some(mut recorder) = self.recorder.take() {
            recorder.shutdown();
        }
    }

    /// Close finishes journal IO before relinquishing the project's writer capability.
    pub(crate) fn close(&mut self) {
        self.stop_active();
        self.remove_journal();
        if let Some(writer) = self.authority.take() {
            let _ = writer.release();
        }
        self.path = PathBuf::new();
    }

    #[cfg(test)]
    pub(crate) fn capture_pending_for_test(&mut self, journal_path: &Path) {
        let entries = scan_directory(journal_path.parent().expect("journal parent"));
        let entry = entries
            .into_iter()
            .find(|entry| entry.journal_path == journal_path)
            .expect("the real startup scan recognizes the fixture journal");
        self.pending.push(entry);
    }
    /// Studio uses the bundle's owner. Preserve any legacy journal for explicit recovery.
    pub(crate) fn stop_for_studio(&mut self) {
        self.stop_active();
        self.authority = None;
        self.path = PathBuf::new();
    }

    fn remove_journal(&self) {
        if self.path.as_os_str().is_empty() {
            return;
        }
        if self
            .authority
            .as_ref()
            .is_some_and(|writer| writer.verify().is_err())
        {
            return;
        }
        if let Some(directory) = &self.directory_owner
            && let Some(name) = self.path.file_name()
        {
            remove_file_best_effort(&directory.io_path().join(name), &self.runtime_error);
        }
    }
}

impl Drop for Recovery {
    fn drop(&mut self) {
        self.stop_active();
        self.remove_journal();
    }
}

fn remove_file_best_effort(path: &Path, runtime_error: &Arc<Mutex<Option<String>>>) {
    if let Err(error) = fs::remove_file(path)
        && error.kind() != io::ErrorKind::NotFound
    {
        *lock_unpoisoned(runtime_error) = Some(format!(
            "could not remove recovery journal {}: {error}",
            path.display()
        ));
    }
}

/// Every journal found on disk, in deterministic name order.
fn scan_directory(directory: &Path) -> Vec<PendingJournal> {
    let Ok(resolved) = fs::canonicalize(directory) else {
        return vec![];
    };
    let Ok(owner) = kinewright_project::legacy_io::PinnedDirectory::open(&resolved) else {
        return vec![];
    };
    let Ok(entries) = fs::read_dir(owner.io_path()) else {
        return vec![];
    };
    let mut names = entries
        .filter_map(Result::ok)
        .map(|entry| entry.file_name())
        .filter(|name| {
            Path::new(name)
                .extension()
                .is_some_and(|extension| extension == "journal")
        })
        .collect::<Vec<_>>();
    names.sort();
    names
        .into_iter()
        .filter_map(|name| {
            let path = owner.io_path().join(&name);
            let (inspection, sha256) = match fs::read(&path) {
                Ok(bytes) => (
                    inspect_bytes(&bytes),
                    Some(kinewright_media::sha256_bytes(&bytes)),
                ),
                Err(_) => (inspect_path(&path), None),
            };
            let journal_path = directory.join(name);
            match inspection {
                Inspection::Missing => None,
                Inspection::Recoverable(report) => Some(PendingJournal {
                    directory: Some(owner.clone()),
                    sha256,
                    project_path: report.project_path.clone(),
                    journal_path,
                    state: PendingState::Recoverable(report),
                }),
                Inspection::Unusable(damage) => Some(PendingJournal {
                    directory: Some(owner.clone()),
                    sha256,
                    project_path: None,
                    journal_path,
                    state: PendingState::Unusable(damage),
                }),
            }
        })
        .collect()
}

fn pending_project_label(project_path: Option<&Path>) -> String {
    project_path.and_then(Path::file_name).map_or_else(
        || "Unsaved project".to_owned(),
        |name| name.to_string_lossy().into_owned(),
    )
}

fn damage_description(damage: &Damage) -> String {
    format!(
        "Recovery stopped at byte {}: {}. {} trailing byte{} were ignored.",
        damage.offset,
        damage.reason,
        damage.ignored_bytes,
        if damage.ignored_bytes == 1 { "" } else { "s" }
    )
}

#[cfg(test)]
#[allow(
    clippy::items_after_test_module,
    reason = "architecture scans require test-only waiting helpers after this module"
)]
mod tests {
    use std::{
        path::PathBuf,
        process::{Command as ProcessCommand, Stdio},
        sync::atomic::{AtomicU64, Ordering},
    };

    use kinewright_core::{
        AssetId, ClipId, Command, Effect, EffectId, Event, Marker, MarkerId, MediaAsset, MediaKind,
        Operation, ParamValue, Rational, TimeCode, Title, Track, TrackId, TrackKind,
    };
    use kinewright_project::{journal_file_name, min_required_format_version};
    use proptest::prelude::*;

    use super::*;

    static NEXT_TEST_DIRECTORY: AtomicU64 = AtomicU64::new(1);

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new(label: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "kinewright-recovery-{label}-{}-{}",
                std::process::id(),
                NEXT_TEST_DIRECTORY.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn journal(&self) -> PathBuf {
            self.0.join("active.journal")
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn asset(id: u64) -> MediaAsset {
        MediaAsset {
            id: AssetId(id),
            path: PathBuf::from(format!("agent-import-{id}.mp4")),
            name: format!("Agent import {id}"),
            duration: TimeCode(300),
            fps: Rational::new(30, 1).unwrap(),
            kind: MediaKind::Video,
            resolution: Some((1_920, 1_080)),
            source_fingerprint: kinewright_core::MediaSourceFingerprint::unknown(),
            color_description: kinewright_core::ColorDescription::default(),
            assumed_from: None,
        }
    }

    fn representative_commands() -> Vec<JournalCommand> {
        vec![
            JournalCommand::Do(Operation::AddTrack {
                track: Track {
                    id: TrackId(1),
                    kind: TrackKind::Video,
                    sync_lock: true,
                    clips: Vec::new(),
                },
            }),
            // These are the same operation shapes received from the MCP agent path.
            JournalCommand::Do(Operation::AddAsset { asset: asset(1) }),
            JournalCommand::Do(Operation::AddClip {
                track: TrackId(1),
                asset: AssetId(1),
                at: TimeCode(20),
                source: TimeCode(10)..TimeCode(100),
            }),
            JournalCommand::Do(Operation::AddEffect {
                clip: ClipId(1),
                effect: Effect {
                    enabled: true,
                    enabled_curve: None,
                    id: EffectId(1),
                    name: "brightness".to_owned(),
                    parameters: std::collections::BTreeMap::default(),
                    keyframes: std::collections::BTreeMap::default(),
                },
            }),
            JournalCommand::Undo,
            JournalCommand::Redo,
            JournalCommand::Undo,
            JournalCommand::Do(Operation::MoveClip {
                clip: ClipId(1),
                to_track: TrackId(1),
                to: TimeCode(40),
            }),
        ]
    }

    fn execute(initial: &Document, commands: &[JournalCommand]) -> Document {
        let core = Core::spawn(initial.clone()).unwrap();
        let mut document = initial.clone();
        for command in commands {
            match core.request(command.clone().into()).unwrap() {
                Event::DocumentChanged { doc, .. } => document = (*doc).clone(),
                Event::OpRejected { error, .. } => panic!("fixture command was rejected: {error}"),
                Event::BatchRejected { error, .. } => {
                    panic!("fixture edit plan was rejected: {error}")
                }
                Event::RevisionConflict {
                    expected, actual, ..
                } => {
                    panic!("fixture revision conflict: expected {expected:?}, actual {actual:?}")
                }
                Event::UndoBlockedBySave { .. } => panic!("fixture Undo blocked by a save"),
                Event::MutationRefused { reason, .. } => {
                    panic!("recovery replay refused: {reason}")
                }
                Event::QueryResult(_) => panic!("unexpected query result"),
            }
        }
        document
    }

    fn encoded_journal_for(
        project_path: Option<&Path>,
        initial: &Document,
        commands: &[JournalCommand],
    ) -> Vec<u8> {
        let mut bytes = MAGIC.to_vec();
        bytes.extend(
            serde_json::to_vec(&JournalHeader {
                format_version: FORMAT_VERSION,
                project_path: project_path.map(Path::to_path_buf),
                writer_format_version: PROJECT_FORMAT_VERSION,
                initial_document: initial.clone(),
            })
            .unwrap(),
        );
        bytes.push(b'\n');
        for command in commands {
            bytes.extend(serde_json::to_vec(command).unwrap());
            bytes.push(b'\n');
        }
        bytes
    }

    /// `None` also serializes exactly like a pre-multi-project header (the
    /// identity field is skipped), so every test through here doubles as a
    /// legacy-format compatibility check.
    fn encoded_journal(initial: &Document, commands: &[JournalCommand]) -> Vec<u8> {
        encoded_journal_for(None, initial, commands)
    }

    #[test]
    fn journal_round_trip_preserves_agent_ops_and_undo_redo_history() {
        let directory = TestDirectory::new("round-trip");
        let initial = Document::default();
        let commands = representative_commands();
        let mut writer = JournalWriter::create(&directory.journal(), None, &initial).unwrap();
        for command in &commands {
            writer.append(command).unwrap();
        }
        drop(writer);

        let Inspection::Recoverable(report) = inspect_path(&directory.journal()) else {
            panic!("expected a recoverable journal");
        };
        assert_eq!(report.recovered_commands, commands.len());
        assert!(report.damage.is_none());
        assert_eq!(report.document, execute(&initial, &commands));
    }

    #[test]
    fn replayed_batch_is_one_history_entry() {
        let initial = Document::default();
        let batch = JournalCommand::DoBatch(vec![
            Operation::AddAsset { asset: asset(1) },
            Operation::AddTrack {
                track: Track {
                    id: TrackId(1),
                    kind: TrackKind::Video,
                    sync_lock: true,
                    clips: Vec::new(),
                },
            },
        ]);
        let commands = vec![batch, JournalCommand::Undo];
        let Inspection::Recoverable(report) = inspect_bytes(&encoded_journal(&initial, &commands))
        else {
            panic!("complete batch journal must be recoverable");
        };
        assert_eq!(report.recovered_commands, 2);
        assert_eq!(report.document, initial);

        let once = execute(&Document::default(), &[commands[0].clone()]);
        assert!(once.asset(AssetId(1)).is_some());
        assert_eq!(once.tracks.len(), 1);
    }

    #[test]
    fn m13_operations_replay_to_the_exact_document() {
        let initial = Document::default();
        let commands = vec![
            JournalCommand::Do(Operation::AddTrack {
                track: Track {
                    id: TrackId(1),
                    kind: TrackKind::Video,
                    sync_lock: true,
                    clips: Vec::new(),
                },
            }),
            JournalCommand::Do(Operation::AddAsset { asset: asset(1) }),
            JournalCommand::Do(Operation::AddClip {
                track: TrackId(1),
                asset: AssetId(1),
                at: TimeCode(0),
                source: TimeCode(0)..TimeCode(30),
            }),
            JournalCommand::Do(Operation::AddClip {
                track: TrackId(1),
                asset: AssetId(1),
                at: TimeCode(60),
                source: TimeCode(30)..TimeCode(60),
            }),
            JournalCommand::Do(Operation::LinkClips {
                clips: vec![ClipId(1), ClipId(2)],
            }),
            JournalCommand::Do(Operation::AddMarker {
                marker: Marker {
                    id: MarkerId(1),
                    position: TimeCode(15),
                    label: "Review".to_owned(),
                    color_token: 0,
                },
            }),
            JournalCommand::Do(Operation::MoveMarker {
                marker: MarkerId(1),
                to: TimeCode(20),
            }),
            JournalCommand::Do(Operation::RippleInsertGap {
                track: TrackId(1),
                at: TimeCode(60),
                duration: TimeCode(10),
            }),
            JournalCommand::Do(Operation::UnlinkClips {
                clips: vec![ClipId(2)],
            }),
            JournalCommand::Do(Operation::RippleDeleteClip { clip: ClipId(1) }),
            JournalCommand::Do(Operation::RemoveMarker {
                marker: MarkerId(1),
            }),
        ];
        let expected = execute(&initial, &commands);
        let Inspection::Recoverable(report) = inspect_bytes(&encoded_journal(&initial, &commands))
        else {
            panic!("M13 journal must be recoverable");
        };
        assert_eq!(report.recovered_commands, commands.len());
        assert_eq!(report.document, expected);
        assert!(report.damage.is_none());
    }

    #[test]
    fn m14_title_and_inspector_operations_replay_to_the_exact_document() {
        let initial = Document::default();
        let commands = vec![
            JournalCommand::Do(Operation::AddTrack {
                track: Track {
                    id: TrackId(1),
                    kind: TrackKind::Video,
                    sync_lock: true,
                    clips: Vec::new(),
                },
            }),
            JournalCommand::Do(Operation::AddTitle {
                track: TrackId(1),
                at: TimeCode(30),
                duration: TimeCode(90),
                title: Title::default(),
            }),
            JournalCommand::Do(Operation::SetTitleParam {
                clip: ClipId(1),
                name: "text".to_owned(),
                value: ParamValue::Text("Recovered title".to_owned()),
            }),
            JournalCommand::Do(Operation::SetTitleParam {
                clip: ClipId(1),
                name: "fade_in_frames".to_owned(),
                value: ParamValue::Integer(12),
            }),
            JournalCommand::Do(Operation::AddMarker {
                marker: Marker {
                    id: MarkerId(1),
                    position: TimeCode(45),
                    label: "Review".to_owned(),
                    color_token: 0,
                },
            }),
            JournalCommand::Do(Operation::SetMarkerParam {
                marker: MarkerId(1),
                name: "label".to_owned(),
                value: ParamValue::Text("Approved".to_owned()),
            }),
            JournalCommand::Undo,
            JournalCommand::Redo,
        ];
        let expected = execute(&initial, &commands);
        let Inspection::Recoverable(report) = inspect_bytes(&encoded_journal(&initial, &commands))
        else {
            panic!("M14 journal must be recoverable");
        };
        assert_eq!(report.recovered_commands, commands.len());
        assert_eq!(report.document, expected);
        assert!(report.damage.is_none());
    }

    #[test]
    fn m15_sync_lock_and_cross_track_ripple_replay_to_the_exact_document() {
        let initial = Document::default();
        let commands = vec![
            JournalCommand::Do(Operation::AddTrack {
                track: Track {
                    id: TrackId(1),
                    kind: TrackKind::Video,
                    sync_lock: true,
                    clips: Vec::new(),
                },
            }),
            JournalCommand::Do(Operation::AddTrack {
                track: Track {
                    id: TrackId(2),
                    kind: TrackKind::Video,
                    sync_lock: true,
                    clips: Vec::new(),
                },
            }),
            JournalCommand::Do(Operation::AddAsset { asset: asset(1) }),
            JournalCommand::Do(Operation::AddClip {
                track: TrackId(1),
                asset: AssetId(1),
                at: TimeCode(0),
                source: TimeCode(0)..TimeCode(30),
            }),
            JournalCommand::Do(Operation::AddClip {
                track: TrackId(1),
                asset: AssetId(1),
                at: TimeCode(60),
                source: TimeCode(30)..TimeCode(60),
            }),
            JournalCommand::Do(Operation::AddClip {
                track: TrackId(2),
                asset: AssetId(1),
                at: TimeCode(60),
                source: TimeCode(60)..TimeCode(90),
            }),
            JournalCommand::Do(Operation::AddMarker {
                marker: Marker {
                    id: MarkerId(1),
                    position: TimeCode(60),
                    label: "Ripple marker".to_owned(),
                    color_token: 0,
                },
            }),
            JournalCommand::Do(Operation::SetTrackSyncLock {
                track: TrackId(2),
                locked: false,
            }),
            JournalCommand::Do(Operation::RippleInsertGap {
                track: TrackId(1),
                at: TimeCode(60),
                duration: TimeCode(10),
            }),
            JournalCommand::Do(Operation::SetTrackSyncLock {
                track: TrackId(2),
                locked: true,
            }),
            JournalCommand::Do(Operation::RippleDeleteClip { clip: ClipId(1) }),
        ];
        let expected = execute(&initial, &commands);
        let Inspection::Recoverable(report) = inspect_bytes(&encoded_journal(&initial, &commands))
        else {
            panic!("M15 journal must be recoverable");
        };
        assert_eq!(report.recovered_commands, commands.len());
        assert_eq!(report.document, expected);
        assert_eq!(
            report.document.clip(ClipId(2)).unwrap().timeline_start,
            TimeCode(40)
        );
        assert_eq!(
            report.document.clip(ClipId(3)).unwrap().timeline_start,
            TimeCode(30)
        );
        assert_eq!(
            report.document.marker(MarkerId(1)).unwrap().position,
            TimeCode(40)
        );
        assert!(report.damage.is_none());
    }

    #[test]
    fn every_truncation_offset_recovers_exactly_the_committed_prefix() {
        let initial = Document::default();
        let commands = representative_commands();
        let bytes = encoded_journal(&initial, &commands);
        let full_parse = parse_journal(&bytes).unwrap();
        let header_end = full_parse.header_end;
        let command_ends: Vec<usize> = full_parse
            .commands
            .iter()
            .map(|command| command.end)
            .collect();

        for cut in 0..=bytes.len() {
            let inspection = inspect_bytes(&bytes[..cut]);
            if cut < header_end {
                assert!(
                    matches!(inspection, Inspection::Unusable(_)),
                    "cut {cut} should not have a complete initial snapshot"
                );
                continue;
            }
            let committed = command_ends.iter().take_while(|end| **end <= cut).count();
            let Inspection::Recoverable(report) = inspection else {
                panic!("cut {cut} should recover its valid prefix");
            };
            assert_eq!(report.recovered_commands, committed, "cut {cut}");
            assert_eq!(
                report.document,
                execute(&initial, &commands[..committed]),
                "cut {cut}"
            );
        }
    }

    #[test]
    fn corruption_stops_at_the_bad_record_and_reports_ignored_bytes() {
        let initial = Document::default();
        let commands = representative_commands();
        let mut bytes = encoded_journal(&initial, &commands);
        let parsed = parse_journal(&bytes).unwrap();
        let corrupt_index = 3;
        let corrupt_offset = parsed.commands[corrupt_index].start;
        bytes[corrupt_offset] = b'!';

        let Inspection::Recoverable(report) = inspect_bytes(&bytes) else {
            panic!("a corrupt command must not hide the valid prefix");
        };
        assert_eq!(report.recovered_commands, corrupt_index);
        assert_eq!(
            report.document,
            execute(&initial, &commands[..corrupt_index])
        );
        let damage = report.damage.expect("corruption must be surfaced");
        assert_eq!(damage.offset, corrupt_offset);
        assert_eq!(damage.ignored_bytes, bytes.len() - corrupt_offset);
        assert!(damage.reason.contains("corrupt"));
    }

    #[test]
    fn recorder_subscribes_to_human_and_agent_core_events() {
        let directory = TestDirectory::new("events");
        let core = Core::spawn(Document::default()).unwrap();
        let runtime_error = Arc::new(Mutex::new(None));
        let mut recorder = Recorder::start(
            &core,
            &directory.journal(),
            None,
            Arc::clone(&runtime_error),
        )
        .unwrap();
        let commands = representative_commands();
        for command in &commands {
            let event = core.request(command.clone().into()).unwrap();
            assert!(matches!(event, Event::DocumentChanged { .. }));
        }
        recorder.flush().unwrap();
        recorder.shutdown();

        assert!(lock_unpoisoned(&runtime_error).is_none());
        let Inspection::Recoverable(report) = inspect_path(&directory.journal()) else {
            panic!("expected recorded Core events");
        };
        assert_eq!(report.recovered_commands, commands.len());
        assert_eq!(report.document, execute(&Document::default(), &commands));
    }

    #[test]
    #[ignore = "invoked as the deliberately crashing subprocess"]
    fn crash_child_process() {
        let Some(path) = std::env::var_os("KINEWRIGHT_TEST_CRASH_JOURNAL").map(PathBuf::from)
        else {
            return;
        };
        let core = Core::spawn(Document::default()).unwrap();
        let runtime_error = Arc::new(Mutex::new(None));
        let recorder = Recorder::start(&core, &path, None, runtime_error).unwrap();
        for command in representative_commands() {
            let Event::DocumentChanged { .. } = core.request(command.into()).unwrap() else {
                panic!("child command was rejected");
            };
        }
        recorder.flush().unwrap();
        std::process::exit(73);
    }

    #[test]
    fn stale_journal_from_crashed_child_is_detected_and_replayed() {
        let directory = TestDirectory::new("crash");
        let path = directory.journal();
        let output = ProcessCommand::new(std::env::current_exe().unwrap())
            .arg("recovery::tests::crash_child_process")
            .arg("--exact")
            .arg("--ignored")
            .arg("--nocapture")
            .env("KINEWRIGHT_TEST_CRASH_JOURNAL", &path)
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(73),
            "crash child failed: stderr:\n{}\nstdout:\n{}",
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout)
        );

        let Inspection::Recoverable(report) = inspect_path(&path) else {
            panic!("parent did not detect the stale journal");
        };
        let commands = representative_commands();
        assert_eq!(report.recovered_commands, commands.len());
        assert_eq!(report.document, execute(&Document::default(), &commands));
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(96))]

        #[test]
        fn arbitrary_do_undo_redo_journal_replays_to_exact_final_document(
            actions in prop::collection::vec(0_u8..10, 0..80),
        ) {
            let initial = Document::default();
            let mut commands = Vec::with_capacity(actions.len());
            let mut next_asset = 1_u64;
            let mut next_track = 1_u64;
            for action in actions {
                let command = match action {
                    0..=4 => {
                        let id = next_asset;
                        next_asset += 1;
                        JournalCommand::Do(Operation::AddAsset { asset: asset(id) })
                    }
                    5 => {
                        let id = next_track;
                        next_track += 1;
                        JournalCommand::Do(Operation::AddTrack {
                            track: Track {
                                id: TrackId(id),
                                kind: TrackKind::Video,
                                sync_lock: true,
                                clips: Vec::new(),
                            },
                        })
                    }
                    6 | 7 => JournalCommand::Undo,
                    _ => JournalCommand::Redo,
                };
                commands.push(command);
            }
            let expected = execute(&initial, &commands);
            let bytes = encoded_journal(&initial, &commands);
            let Inspection::Recoverable(report) = inspect_bytes(&bytes) else {
                prop_assert!(false, "a complete generated journal must be recoverable");
                return Ok(());
            };
            prop_assert_eq!(report.recovered_commands, commands.len());
            prop_assert!(report.damage.is_none());
            prop_assert_eq!(report.document, expected);
        }
    }

    #[test]
    fn clean_drop_removes_only_this_sessions_journal() {
        let directory = TestDirectory::new("clean-drop");
        let core = Core::spawn(Document::default()).unwrap();
        let path;
        {
            let recovery = Recovery::start_in(directory.0.clone(), &core, None);
            path = recovery.path.clone();
            assert!(path.is_file());
            drop(recovery);
        }
        assert!(!path.exists());
    }

    #[test]
    fn a_stale_journal_goes_pending_while_the_new_session_journals_elsewhere() {
        let directory = TestDirectory::new("header-only");
        let stale = directory.0.join("unsaved-1.journal");
        drop(JournalWriter::create(&stale, None, &Document::default()).unwrap());
        let core = Core::spawn(Document::default()).unwrap();

        let recovery = Recovery::start_in(directory.0.clone(), &core, None);
        assert_eq!(recovery.pending.len(), 1);
        assert!(matches!(
            &recovery.pending[0].state,
            PendingState::Recoverable(RecoveryReport {
                recovered_commands: 0,
                ..
            })
        ));
        assert!(recovery.recorder.is_some());
        assert_ne!(recovery.path, stale);
        assert!(stale.is_file());
        drop(recovery);
        assert!(stale.is_file(), "unresolved recovery must survive shutdown");
    }

    #[test]
    fn attached_session_does_not_duplicate_startup_restore_decisions() {
        let directory = TestDirectory::new("attached-no-scan");
        let stale = directory.0.join("unsaved-1.journal");
        drop(JournalWriter::create(&stale, None, &Document::default()).unwrap());
        let core = Core::spawn(Document::default()).unwrap();

        let recovery = Recovery::start_attached_in(directory.0.clone(), &core, None);
        assert!(recovery.pending.is_empty());
        assert_ne!(recovery.path, stale);
        assert!(stale.is_file());
    }

    #[test]
    fn startup_restore_decisions_can_move_to_the_next_session() {
        let directory = TestDirectory::new("pending-transfer");
        let stale = directory.0.join("unsaved-1.journal");
        drop(JournalWriter::create(&stale, None, &Document::default()).unwrap());
        let first_core = Core::spawn(Document::default()).unwrap();
        let next_core = Core::spawn(Document::default()).unwrap();
        let mut first = Recovery::start_in(directory.0.clone(), &first_core, None);
        let mut next = Recovery::start_attached_in(directory.0.clone(), &next_core, None);

        first.move_pending_to(&mut next);

        assert!(first.pending.is_empty());
        assert_eq!(next.pending.len(), 1);
        assert_eq!(next.pending[0].journal_path, stale);
    }

    #[test]
    fn successful_save_checkpoint_discards_pre_save_commands_and_adopts_the_name() {
        let directory = TestDirectory::new("checkpoint");
        let core = Core::spawn(Document::default()).unwrap();
        let mut recovery = Recovery::start_in(directory.0.clone(), &core, None);
        let unsaved_path = recovery.path.clone();
        let first = JournalCommand::Do(Operation::AddAsset { asset: asset(1) });
        let _ = core.request(Command::from(first)).unwrap();
        recovery.recorder.as_ref().unwrap().flush().unwrap();
        let project = directory.0.join("My Video.kinewright");
        recovery.checkpoint(&core, Some(&project));

        assert_ne!(recovery.path, unsaved_path);
        assert!(!unsaved_path.exists());
        let second = JournalCommand::Do(Operation::AddAsset { asset: asset(2) });
        let _ = core.request(Command::from(second)).unwrap();
        recovery.recorder.as_ref().unwrap().flush().unwrap();
        let Inspection::Recoverable(report) = inspect_path(&recovery.path) else {
            panic!("expected checkpoint journal");
        };
        assert_eq!(report.recovered_commands, 1);
        // G5: new headers persist the absolute canonical identity.
        assert_eq!(
            report.project_path.as_deref(),
            Some(
                canonical_project_identity(&project)
                    .expect("the identity resolves")
                    .as_path()
            )
        );
        assert!(report.document.asset(AssetId(1)).is_some());
        assert!(report.document.asset(AssetId(2)).is_some());
    }

    #[test]
    fn journal_names_are_stable_readable_and_collision_proof() {
        let first = journal_file_name(Path::new("C:/videos/My Video (final).kinewright"));
        let again = journal_file_name(Path::new("C:/videos/My Video (final).kinewright"));
        assert_eq!(first, again, "names must be deterministic across runs");
        assert!(first.starts_with("MyVideofinal-"));
        assert!(first.ends_with(".journal"));
        let elsewhere = journal_file_name(Path::new("D:/other/My Video (final).kinewright"));
        assert_ne!(
            first, elsewhere,
            "same stem, different path, different hash"
        );
    }

    #[test]
    fn allocation_never_truncates_pending_data_and_keeps_unsaved_names() {
        let directory = Path::new("recovery");
        let project = Path::new("C:/videos/cut.kinewright");
        let base = directory.join(journal_file_name(project));
        let placeholder = directory.join("uninitialized.journal");

        // Free name: taken directly.
        assert_eq!(
            allocate_journal_path(directory, Some(project), &placeholder, &[]),
            base
        );
        let reserved = [base.as_path()];
        let suffixed = allocate_journal_path(directory, Some(project), &placeholder, &reserved);
        assert_ne!(suffixed, base);
        assert!(suffixed.to_string_lossy().ends_with("-2.journal"));

        // An unsaved project keeps its current journal across baselines...
        let current = directory.join("unsaved-3.journal");
        assert_eq!(
            allocate_journal_path(directory, None, &current, &[]),
            current
        );
        // ...unless that name now belongs to pending crash data.
        let reserved = [current.as_path()];
        let moved = allocate_journal_path(directory, None, &current, &reserved);
        assert_ne!(moved, current);
        assert!(
            moved
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("unsaved-")
        );
    }

    #[test]
    fn saved_project_allocation_avoids_an_active_existing_journal() {
        let directory = TestDirectory::new("active-saved-collision");
        let project = directory.0.join("cut.kinewright");
        let base = directory.0.join(journal_file_name(&project));
        fs::write(&base, b"active").unwrap();
        let placeholder = directory.0.join("uninitialized.journal");

        let allocated = allocate_journal_path(&directory.0, Some(&project), &placeholder, &[]);
        assert_ne!(allocated, base);
        assert!(allocated.to_string_lossy().ends_with("-2.journal"));
    }

    #[test]
    fn scan_reads_identity_from_every_journal_and_restores_consume_them() {
        let directory = TestDirectory::new("scan");
        let project = directory.0.join("Interview.kinewright");
        let commands = representative_commands();
        fs::write(
            directory.0.join("Interview-abc.journal"),
            encoded_journal_for(Some(&project), &Document::default(), &commands),
        )
        .unwrap();
        drop(
            JournalWriter::create(
                &directory.0.join("unsaved-1.journal"),
                None,
                &Document::default(),
            )
            .unwrap(),
        );
        fs::write(directory.0.join("notes.txt"), b"not a journal").unwrap();

        let core = Core::spawn(Document::default()).unwrap();
        let mut recovery = Recovery::start_in(directory.0.clone(), &core, None);
        assert_eq!(recovery.pending.len(), 2);
        let saved = recovery
            .pending
            .iter()
            .find(|entry| entry.project_path.is_some())
            .expect("the saved project's journal carries its identity");
        assert_eq!(saved.project_path.as_deref(), Some(project.as_path()));
        let PendingState::Recoverable(report) = &saved.state else {
            panic!("the saved project's journal must be recoverable");
        };
        assert_eq!(report.recovered_commands, commands.len());

        let journal_path = saved.journal_path.clone();
        recovery.consume_pending(&journal_path);
        assert_eq!(recovery.pending.len(), 1);
        assert!(!journal_path.exists());
    }

    /// Craft header-only journal bytes with a chosen writer version line.
    fn journal_bytes_with_header_line(header_line: &[u8]) -> Vec<u8> {
        let mut bytes = MAGIC.to_vec();
        bytes.extend_from_slice(header_line);
        bytes.push(b'\n');
        bytes
    }

    /// MO2 R7: a journal whose initial snapshot needs only v1 still stamps
    /// the maximum supported version, because an appended MO2 operation can
    /// need v2 — the header is never downgraded to the snapshot's minimum.
    #[test]
    fn mo2_v1_snapshot_journal_keeps_the_maximum_writer_version() {
        let initial = Document {
            tracks: vec![Track {
                id: TrackId(1),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: Vec::new(),
            }],
            ..Document::default()
        };
        assert_eq!(min_required_format_version(&initial), 1);
        let command = JournalCommand::Do(Operation::AddSolidClip {
            track: TrackId(1),
            timeline_start: TimeCode(0),
            duration: TimeCode(30),
            color: kinewright_core::SolidColor { r: 9, g: 8, b: 7 },
        });
        let directory = TestDirectory::new("mo2-writer-version");
        let mut writer = JournalWriter::create(&directory.journal(), None, &initial).unwrap();
        writer.append(&command).unwrap();
        drop(writer);
        let Inspection::Recoverable(report) = inspect_path(&directory.journal()) else {
            panic!("expected a recoverable journal");
        };
        assert_eq!(report.document, execute(&initial, &[command]));
        assert_eq!(min_required_format_version(&report.document), 2);
        assert_eq!(report.writer_format_version, PROJECT_FORMAT_VERSION);
        assert!(report.writer_format_version >= min_required_format_version(&report.document));
    }

    /// `IN2B` §4 rule 5's parse/write halves: pre-Part-B headers (no key)
    /// read as 1, the writer stamps its own const, and a 999 writer version
    /// carries through the parse.
    #[test]
    fn journal_headers_carry_the_writer_format_version() {
        // A pre-Part-B header has no `writer_format_version` key at all.
        let initial = serde_json::to_vec(&Document::default()).expect("the snapshot serialises");
        // Splice the snapshot in as raw JSON (no re-parse games: the header
        // is bytes on a wire, built as bytes).
        let mut old_line = br#"{"format_version":1,"initial_document":"#.to_vec();
        old_line.extend_from_slice(&initial);
        old_line.push(b'}');
        let parsed =
            parse_journal(&journal_bytes_with_header_line(&old_line)).expect("old headers parse");
        assert_eq!(parsed.writer_format_version, 1);

        // The writer stamps its own const, not the session's read version.
        let directory = TestDirectory::new("writer-version");
        let path = directory.0.join("stamp.journal");
        drop(JournalWriter::create(&path, None, &Document::default()).expect("journal creates"));
        let bytes = fs::read(&path).expect("the journal reads");
        let parsed = parse_journal(&bytes).expect("the stamped journal parses");
        assert_eq!(parsed.writer_format_version, PROJECT_FORMAT_VERSION);

        // A 999 writer version carries through the parse untouched.
        let header = serde_json::to_vec(&JournalHeader {
            format_version: FORMAT_VERSION,
            project_path: None,
            writer_format_version: 999,
            initial_document: Document::default(),
        })
        .expect("the 999 header serialises");
        let parsed =
            parse_journal(&journal_bytes_with_header_line(&header)).expect("999 headers parse");
        assert_eq!(parsed.writer_format_version, 999);
    }

    /// A cross-version journal scans `Recoverable` — the refusal happens at
    /// restore time (`apply_restore_request` notes the incident), not at
    /// scan time, so the dialog still offers it and the incident has a
    /// trigger.
    #[test]
    fn cross_version_journals_scan_recoverable_for_the_dialog() {
        let directory = TestDirectory::new("cross-version-scan");
        let header = serde_json::to_vec(&JournalHeader {
            format_version: FORMAT_VERSION,
            project_path: None,
            writer_format_version: 999,
            initial_document: Document::default(),
        })
        .expect("the 999 header serialises");
        fs::write(
            directory.0.join("cross.journal"),
            journal_bytes_with_header_line(&header),
        )
        .expect("the 999 journal writes");
        let pending = scan_directory(&directory.0);
        assert_eq!(pending.len(), 1);
        let PendingState::Recoverable(report) = &pending[0].state else {
            panic!("a replayable 999 journal scans Recoverable");
        };
        assert_eq!(report.writer_format_version, 999);
        assert_eq!(report.recovered_commands, 0);
    }

    /// Item 23: both recovery modals note damage-only. The constructors
    /// shape code/subject/tag/transient; the headless dialog reports
    /// damage renders — and a clean prompt reports none, because the
    /// normal prompt has no constructor (`IN2B` §7 rule 6).
    #[test]
    fn in2b_both_recovery_modals_note_damage_only() {
        let revision = TimelineRevision(7);
        let damage = recovery_damage_observation("stopped at byte 12", revision);
        assert_eq!(
            damage.code,
            IncidentCode::Label(LabelIncident::RecoveryDamage)
        );
        assert_eq!(damage.subject, IncidentSubject::Project);
        assert_eq!(damage.observed, "Recovery: stopped at byte 12");
        assert!(damage.transient);
        let unavailable = recovery_unavailable_observation("no directory", revision);
        assert_eq!(
            unavailable.code,
            IncidentCode::Label(LabelIncident::RecoveryUnavailable)
        );
        assert_eq!(unavailable.subject, IncidentSubject::Project);
        assert_eq!(unavailable.observed, "Recovery: no directory");
        assert!(unavailable.transient);

        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        // One headless frame per dialog: the pass initialises the font
        // system the labels lay out against.
        let drive = |dialog: &mut Recovery| {
            ctx.begin_pass(egui::RawInput::default());
            let outcome = dialog.show_dialog(&ctx);
            let _ = ctx.end_pass();
            outcome
        };
        let idle = || Recovery {
            authority: None,
            directory_owner: None,
            directory: PathBuf::from("recovery"),
            path: PathBuf::from("recovery").join("active.journal"),
            recorder: None,
            pending: Vec::new(),
            runtime_error: Arc::new(Mutex::new(None)),
        };
        let pending = |state| PendingJournal {
            directory: None,
            sha256: None,
            journal_path: PathBuf::from("pending.journal"),
            project_path: None,
            state,
        };
        let report = |damage| RecoveryReport {
            project_path: None,
            document: Document::default(),
            writer_format_version: PROJECT_FORMAT_VERSION,
            recovered_commands: 3,
            damage,
        };

        // The normal prompt: recoverable, no damage — nothing to note.
        let mut clean = idle();
        clean
            .pending
            .push(pending(PendingState::Recoverable(report(None))));
        let outcome = drive(&mut clean);
        assert!(outcome.restore.is_none(), "no click restores nothing");
        assert_eq!(
            outcome.damage_rendered, None,
            "the normal prompt has no constructor"
        );
        assert_eq!(outcome.unavailable_rendered, None);

        // A damaged recovery reports its damage string.
        let mut damaged = idle();
        damaged
            .pending
            .push(pending(PendingState::Recoverable(report(Some(Damage {
                offset: 12,
                ignored_bytes: 4,
                reason: "torn".to_owned(),
            })))));
        let outcome = drive(&mut damaged);
        assert_eq!(
            outcome.damage_rendered.as_deref(),
            Some("Recovery stopped at byte 12: torn. 4 trailing bytes were ignored."),
        );

        // An unusable journal reports its damage string too.
        let mut unusable = idle();
        unusable
            .pending
            .push(pending(PendingState::Unusable(Damage {
                offset: 0,
                ignored_bytes: 1,
                reason: "no snapshot".to_owned(),
            })));
        let outcome = drive(&mut unusable);
        assert_eq!(
            outcome.damage_rendered.as_deref(),
            Some("Recovery stopped at byte 0: no snapshot. 1 trailing byte were ignored."),
        );

        // A broken runtime reports its message.
        let mut broken = idle();
        broken.runtime_error = Arc::new(Mutex::new(Some("no directory".to_owned())));
        let outcome = drive(&mut broken);
        assert_eq!(
            outcome.unavailable_rendered.as_deref(),
            Some("no directory"),
        );
    }
}

/// Actual `JournalWriter` observation, shared by the C7 witness and additive S0 fixture.
/// This helper also compiles against the predecessor's unchanged GUI writer.
#[cfg(test)]
#[allow(
    clippy::too_many_lines,
    reason = "one GUI observation with a successful recorder control and three exclusion cases"
)]
pub(crate) fn g04_gui_journal_observation() -> Result<serde_json::Value, String> {
    use kinewright_media::test_support::TempDirectory;
    use kinewright_project::{LockMode, LockfileError, acquire_project_lock_with_policy};
    let root = kinewright_media::test_support::workspace_target_dir();
    fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    let temp = TempDirectory::new_in(&root, "c7-real-gui");
    let project = temp.path("project.kinewright");
    let document = Document::default();
    let bytes = kinewright_project::serialize_project_document(&document).unwrap();
    fs::write(&project, &bytes).unwrap();
    let directory = temp.path("recovery");
    fs::create_dir(&directory).unwrap();
    let core = Core::spawn(document).unwrap();
    let control = Recovery::start_attached_in(directory.clone(), &core, Some(&project));
    core.request(kinewright_core::Command::Undo).unwrap();
    control
        .recorder
        .as_ref()
        .ok_or("control recorder unavailable")?
        .flush()?;
    let parsed = parse_journal(&fs::read(&control.path).unwrap()).map_err(|e| e.reason)?;
    let control_appended = parsed.project_path.as_deref().is_some_and(|recorded| {
        canonical_project_identity(recorded).ok() == canonical_project_identity(&project).ok()
    }) && parsed.commands.len() == 1
        && parsed.commands[0].command == JournalCommand::Undo;
    let mut startup_pending = scan_directory(&directory);
    drop(control);
    let mut excluded = true;
    for shape in ["base", "suffix", "alias"] {
        let attempted = Arc::new(Mutex::new(false));
        let observed = Arc::clone(&attempted);
        let named = if shape == "alias" {
            project
                .parent()
                .unwrap()
                .join(".")
                .join(project.file_name().unwrap())
        } else {
            project.clone()
        };
        let recovery_directory = directory.clone();
        let reserved = if shape == "suffix" {
            std::mem::take(&mut startup_pending)
        } else {
            vec![]
        };
        let pending_slot = Arc::new(Mutex::new(Some(reserved)));
        let owner = kinewright_project::io_observer::at_barrier(
            "after_flock_before_publish",
            move || {
                let named = named.clone();
                let directory = recovery_directory.clone();
                let pending_slot = Arc::clone(&pending_slot);
                let created = std::thread::spawn(move || {
                    let pending = pending_slot.lock().unwrap().take().unwrap();
                    let core = Core::spawn(Document::default()).unwrap();
                    let writer =
                        Recovery::start_with_pending(directory, &core, Some(&named), pending);
                    let created = writer.recorder.is_some();
                    drop(writer);
                    created
                })
                .join()
                .unwrap();
                *observed.lock().unwrap() = created;
            },
            || {
                acquire_project_lock_with_policy(
                    &project,
                    LockMode::Headless,
                    "",
                    &directory,
                    1,
                    Duration::ZERO,
                )
            },
        )
        .map_err(|e| e.to_string())?;
        excluded &= !*attempted.lock().unwrap();
        let discovery = fs::read(&owner.handle.discovery).unwrap();
        assert!(matches!(
            acquire_project_lock_with_policy(
                &project,
                LockMode::Headless,
                "",
                &directory,
                1,
                Duration::ZERO
            ),
            Err(LockfileError::Contention { .. })
        ));
        assert_eq!(fs::read(&owner.handle.discovery).unwrap(), discovery);
        owner.handle.release().unwrap();
    }
    Ok(
        serde_json::json!({"control_appended": control_appended, "post_scan_excluded": excluded,
        "project_unchanged": fs::read(&project).unwrap() == bytes.as_bytes()}),
    )
}

#[cfg(all(test, unix))]
pub(crate) fn g04_gui_lost_authority() {
    use kinewright_media::test_support::TempDirectory;
    let root = kinewright_media::test_support::workspace_target_dir();
    fs::create_dir_all(&root).unwrap();
    let temp = TempDirectory::new_in(&root, "c7-gui-lost");
    let project = temp.path("project.kinewright");
    fs::write(
        &project,
        kinewright_project::serialize_project_document(&Document::default()).unwrap(),
    )
    .unwrap();
    let core = Core::spawn(Document::default()).unwrap();
    let mut recovery = Recovery::start_attached_in(temp.path("recovery"), &core, Some(&project));
    recovery.recorder.as_ref().unwrap().flush().unwrap();
    let journal = recovery.path.clone();
    let before = fs::read(&journal).unwrap();
    let lost = recovery.writer().unwrap();
    fs::remove_file(&lost.path).unwrap();
    let successor =
        kinewright_project::lockfile::acquire_gui_project_lock(&project, &recovery.directory)
            .unwrap();
    let discovery = fs::read(&successor.handle.discovery).unwrap();
    core.request(kinewright_core::Command::Undo).unwrap();
    let _ = recovery.recorder.as_ref().unwrap().flush();
    assert!(lock_unpoisoned(&recovery.runtime_error).is_some());
    assert_eq!(
        fs::read(&journal).unwrap(),
        before,
        "lost journal authority cannot append or flush"
    );
    let files = |directory: &Path| {
        let mut files = fs::read_dir(directory)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.extension().is_some_and(|e| e == "journal"))
            .collect::<Vec<_>>();
        files.sort();
        files
    };
    let original = files(&recovery.directory);
    recovery.pending.push(PendingJournal {
        directory: recovery.directory_owner.clone(),
        sha256: Some(kinewright_media::sha256_bytes(&before)),
        journal_path: journal.clone(),
        project_path: Some(project.clone()),
        state: PendingState::Unusable(Damage {
            offset: 0,
            ignored_bytes: 0,
            reason: "preserved older journal".into(),
        }),
    });
    recovery.checkpoint_owned(&core, Some(&project), Some(lost));
    assert_eq!(
        files(&recovery.directory),
        original,
        "lost authority cannot create a successor journal"
    );
    recovery.consume_pending(&journal);
    assert_eq!(
        fs::read(&journal).unwrap(),
        before,
        "startup retirement requires the current project authority"
    );
    drop(recovery);
    assert_eq!(
        fs::read(&journal).unwrap(),
        before,
        "lost authority cannot retire the existing journal"
    );
    assert_eq!(fs::read(&successor.handle.discovery).unwrap(), discovery);
    successor.handle.release().unwrap();
}

#[cfg(all(test, unix))]
pub(crate) fn g04_gui_directory_stays_pinned() {
    use kinewright_media::test_support::TempDirectory;
    let root = kinewright_media::test_support::workspace_target_dir();
    fs::create_dir_all(&root).unwrap();
    let temp = TempDirectory::new_in(&root, "c7-gui-directory");
    let project = temp.path("project.kinewright");
    fs::write(
        &project,
        kinewright_project::serialize_project_document(&Document::default()).unwrap(),
    )
    .unwrap();
    let core = Core::spawn(Document::default()).unwrap();
    let directory = temp.path("recovery");
    let mut recovery = Recovery::start_attached_in(directory.clone(), &core, Some(&project));
    recovery.recorder.as_ref().unwrap().flush().unwrap();
    let held = temp.path("held-recovery");
    fs::rename(&directory, &held).unwrap();
    fs::create_dir(&directory).unwrap();
    fs::write(directory.join("sentinel"), b"unrelated directory").unwrap();
    core.request(kinewright_core::Command::Undo).unwrap();
    recovery.checkpoint(&core, Some(&project));
    recovery.recorder.as_ref().unwrap().flush().unwrap();
    let name = recovery.path.file_name().unwrap();
    assert!(
        held.join(name).exists(),
        "checkpoint remains beneath the original admitted directory"
    );
    assert!(
        !directory.join(name).exists(),
        "replacement recovery directory is never written"
    );
    drop(recovery);
    assert_eq!(
        fs::read(directory.join("sentinel")).unwrap(),
        b"unrelated directory"
    );
    assert!(fs::read_dir(&held).unwrap().next().is_none());
}

#[cfg(all(test, unix))]
pub(crate) fn g04_startup_retirement_keeps_scanned_identity() {
    use kinewright_media::test_support::TempDirectory;
    let root = kinewright_media::test_support::workspace_target_dir();
    fs::create_dir_all(&root).unwrap();
    let temp = TempDirectory::new_in(&root, "c7-startup-pin");
    let project = temp.path("project.kinewright");
    fs::write(
        &project,
        kinewright_project::serialize_project_document(&Document::default()).unwrap(),
    )
    .unwrap();
    let directory = temp.path("recovery");
    fs::create_dir(&directory).unwrap();
    let journal = directory.join(kinewright_project::journal_file_name(&project));
    let control = JournalWriter::create(&journal, Some(&project), &Document::default()).unwrap();
    drop(control);
    let core = Core::spawn(Document::default()).unwrap();
    let mut recovery =
        Recovery::start_with_pending(directory.clone(), &core, None, scan_directory(&directory));
    let captured = fs::read(&journal).unwrap();
    let held = temp.path("held-recovery");
    fs::rename(&directory, &held).unwrap();
    fs::create_dir(&directory).unwrap();
    let replacement = directory.join(journal.file_name().unwrap());
    fs::write(&replacement, &captured).unwrap();
    recovery.consume_pending(&journal);
    assert_eq!(
        fs::read(&replacement).unwrap(),
        captured,
        "startup retirement stays in the scanned native directory"
    );
    assert!(!held.join(journal.file_name().unwrap()).exists());
    drop(recovery);

    // A later writer's changed journal cannot be discarded by an earlier decision.
    fs::remove_file(&replacement).unwrap();
    fs::write(&journal, &captured).unwrap();
    let mut recovery =
        Recovery::start_with_pending(directory.clone(), &core, None, scan_directory(&directory));
    let mut changed = captured;
    changed.extend_from_slice(b"null\n");
    fs::write(&journal, &changed).unwrap();
    recovery.consume_pending(&journal);
    assert_eq!(
        fs::read(&journal).unwrap(),
        changed,
        "changed bytes are preserved for a new explicit decision"
    );
    assert!(
        recovery
            .pending
            .iter()
            .any(|entry| entry.journal_path == journal)
    );
}
