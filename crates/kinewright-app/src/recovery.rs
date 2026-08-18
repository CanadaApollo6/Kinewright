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
use kinewright_core::{Core, Document, Event, JournalCommand};
use serde::{Deserialize, Serialize};

use crate::theme::{self, type_size};

const MAGIC: &[u8] = b"KINEWRIGHT-JOURNAL 1\n";
const FORMAT_VERSION: u32 = 1;
const CORE_RESPONSE_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Serialize, Deserialize)]
struct JournalHeader {
    format_version: u32,
    /// The owning project's save path; `None` for unsaved projects. Optional
    /// with a default so journals from before multi-project still parse.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    project_path: Option<PathBuf>,
    initial_document: Document,
}

struct JournalWriter {
    file: File,
    sync_each_record: bool,
}

impl JournalWriter {
    fn create(
        path: &Path,
        project_path: Option<&Path>,
        initial_document: &Document,
    ) -> Result<Self, String> {
        Self::create_with_sync(path, project_path, initial_document, true)
    }

    fn create_with_sync(
        path: &Path,
        project_path: Option<&Path>,
        initial_document: &Document,
        sync_each_record: bool,
    ) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                format!(
                    "could not create recovery directory {}: {error}",
                    parent.display()
                )
            })?;
        }
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(path)
            .map_err(|error| {
                format!(
                    "could not create recovery journal {}: {error}",
                    path.display()
                )
            })?;
        let header = serde_json::to_vec(&JournalHeader {
            format_version: FORMAT_VERSION,
            project_path: project_path.map(Path::to_path_buf),
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
            file,
            sync_each_record,
        })
    }

    fn append(&mut self, command: &JournalCommand) -> Result<(), String> {
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
    fn start(
        core: &Core,
        path: &Path,
        project_path: Option<&Path>,
        runtime_error: Arc<Mutex<Option<String>>>,
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
        let writer = JournalWriter::create(path, project_path, &initial_document)?;
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

    #[cfg(test)]
    fn flush(&self) -> Result<(), String> {
        let (acknowledge, acknowledged) = unbounded();
        self.control
            .send(RecorderControl::Flush(acknowledge))
            .map_err(|_| "recovery recorder stopped before flush".to_owned())?;
        acknowledged
            .recv_timeout(CORE_RESPONSE_TIMEOUT)
            .map_err(|error| format!("recovery recorder did not flush in time: {error}"))
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
    journal_path: PathBuf,
    project_path: Option<PathBuf>,
    state: PendingState,
}

/// A restore the user chose from the recovery dialog. The pending file
/// survives until `consume_pending` confirms the restore actually landed.
pub(crate) struct RestoreRequest {
    pub(crate) document: Document,
    pub(crate) project_path: Option<PathBuf>,
    pub(crate) journal_path: PathBuf,
}

/// App-owned recovery lifecycle. All filesystem and UI behavior stays here.
pub(crate) struct Recovery {
    directory: PathBuf,
    path: PathBuf,
    recorder: Option<Recorder>,
    pending: Vec<PendingJournal>,
    runtime_error: Arc<Mutex<Option<String>>>,
}

impl Recovery {
    pub(crate) fn start(core: &Core, project_path: Option<&Path>) -> Self {
        Self::start_in(default_recovery_directory(), core, project_path)
    }

    /// Start a per-project recorder without repeating the process-startup
    /// recovery scan. Only the startup session owns restore decisions.
    pub(crate) fn start_attached(core: &Core, project_path: Option<&Path>) -> Self {
        Self::start_attached_in(default_recovery_directory(), core, project_path)
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
            path: directory.join("uninitialized.journal"),
            directory,
            recorder: None,
            pending,
            runtime_error,
        };
        // Per-project files never collide with pending ones, so journaling
        // starts immediately even while restores await a decision.
        recovery.attach(core, project_path);
        recovery
    }

    /// Finish the previous project's journal and begin from Core's authoritative
    /// current snapshot. Core's initial subscription event makes this race-free
    /// with edits from the agent server.
    pub(crate) fn attach(&mut self, core: &Core, project_path: Option<&Path>) {
        self.stop_active();
        let reserved: Vec<&Path> = self
            .pending
            .iter()
            .map(|entry| entry.journal_path.as_path())
            .collect();
        let target = allocate_journal_path(&self.directory, project_path, &self.path, &reserved);
        if target != self.path {
            self.remove_journal();
            self.path = target;
        }
        *lock_unpoisoned(&self.runtime_error) = None;
        match Recorder::start(
            core,
            &self.path,
            project_path,
            Arc::clone(&self.runtime_error),
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
    pub(crate) fn checkpoint(&mut self, core: &Core, project_path: Option<&Path>) {
        self.attach(core, project_path);
    }

    /// Render the startup decisions - one row per found journal - and return
    /// a restore request when the user picks one. The pending file survives
    /// until `consume_pending` confirms the restore landed, so a failed
    /// restore can still be recovered at the next launch.
    pub(crate) fn show_dialog(&mut self, ctx: &egui::Context) -> Option<RestoreRequest> {
        let mut restore: Option<usize> = None;
        let mut discard: Option<usize> = None;
        if !self.pending.is_empty() {
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
                                    ui.colored_label(
                                        egui::Color32::YELLOW,
                                        damage_description(damage),
                                    );
                                }
                            }
                            PendingState::Unusable(damage) => {
                                ui.colored_label(
                                    egui::Color32::YELLOW,
                                    "This journal's initial snapshot is unavailable.",
                                );
                                ui.label(damage_description(damage));
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
            let entry = self.pending.remove(index);
            remove_file_best_effort(&entry.journal_path, &self.runtime_error);
            return None;
        }
        if let Some(index) = restore {
            let entry = &self.pending[index];
            let PendingState::Recoverable(report) = &entry.state else {
                return None;
            };
            return Some(RestoreRequest {
                document: report.document.clone(),
                project_path: report.project_path.clone(),
                journal_path: entry.journal_path.clone(),
            });
        }
        None
    }

    /// A chosen restore landed: the crash journal has served its purpose.
    pub(crate) fn consume_pending(&mut self, journal_path: &Path) {
        self.pending
            .retain(|entry| entry.journal_path != journal_path);
        remove_file_best_effort(journal_path, &self.runtime_error);
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

    fn remove_journal(&self) {
        remove_file_best_effort(&self.path, &self.runtime_error);
    }
}

impl Drop for Recovery {
    fn drop(&mut self) {
        self.stop_active();
        // A clean exit owes nothing to recovery; undecided pending journals
        // from other crashed sessions stay for the next launch.
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

fn default_recovery_directory() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map_or_else(std::env::temp_dir, PathBuf::from)
        .join("Kinewright")
        .join("recovery")
}

/// Every journal found on disk, in deterministic name order.
fn scan_directory(directory: &Path) -> Vec<PendingJournal> {
    let Ok(entries) = fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "journal")
        })
        .collect();
    paths.sort();
    paths
        .into_iter()
        .filter_map(|journal_path| match inspect_path(&journal_path) {
            Inspection::Missing => None,
            Inspection::Recoverable(report) => Some(PendingJournal {
                project_path: report.project_path.clone(),
                journal_path,
                state: PendingState::Recoverable(report),
            }),
            Inspection::Unusable(damage) => Some(PendingJournal {
                journal_path,
                project_path: None,
                state: PendingState::Unusable(damage),
            }),
        })
        .collect()
}

/// FNV-1a, chosen over the standard hasher because journal names must stay
/// stable across builds and Rust versions to find their project again.
fn fnv1a_64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// `MyVideo-1a2b3c4d5e6f7081.journal` - readable stem, collision-proof hash.
fn journal_file_name(project_path: &Path) -> String {
    let stem: String = project_path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default()
        .chars()
        .filter(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
        .take(24)
        .collect();
    let stem = if stem.is_empty() {
        "project".to_owned()
    } else {
        stem
    };
    let hash = fnv1a_64(project_path.to_string_lossy().as_bytes());
    format!("{stem}-{hash:016x}.journal")
}

/// The journal path for a project, never colliding with a reserved (pending)
/// file: undecided crash data must not be truncated by a new session. An
/// unsaved project keeps its current `unsaved-N` file across baselines.
fn allocate_journal_path(
    directory: &Path,
    project_path: Option<&Path>,
    current: &Path,
    reserved: &[&Path],
) -> PathBuf {
    let is_reserved = |candidate: &Path| reserved.contains(&candidate);
    if let Some(project_path) = project_path {
        let base = journal_file_name(project_path);
        let first = directory.join(&base);
        if !is_reserved(&first) && (first == current || !first.exists()) {
            return first;
        }
        let stem = base.trim_end_matches(".journal");
        for suffix in 2.. {
            let candidate = directory.join(format!("{stem}-{suffix}.journal"));
            if !is_reserved(&candidate) && (candidate == current || !candidate.exists()) {
                return candidate;
            }
        }
        unreachable!("an unreserved journal suffix always exists");
    }
    let keeps_current = current
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with("unsaved-"))
        && !is_reserved(current);
    if keeps_current {
        return current.to_path_buf();
    }
    for number in 1.. {
        let candidate = directory.join(format!("unsaved-{number}.journal"));
        if !is_reserved(&candidate) && !candidate.exists() {
            return candidate;
        }
    }
    unreachable!("an unreserved unsaved journal name always exists");
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

pub(crate) fn restore_status(result: Result<(), String>) -> String {
    match result {
        Ok(()) => "Recovered unsaved work".to_owned(),
        Err(error) => format!("Could not restore unsaved work: {error}"),
    }
}

#[cfg(test)]
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
                Event::RevisionConflict { expected, actual } => {
                    panic!("fixture revision conflict: expected {expected:?}, actual {actual:?}")
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
        let status = ProcessCommand::new(std::env::current_exe().unwrap())
            .arg("recovery::tests::crash_child_process")
            .arg("--exact")
            .arg("--ignored")
            .arg("--nocapture")
            .env("KINEWRIGHT_TEST_CRASH_JOURNAL", &path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert_eq!(status.code(), Some(73));

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
        // Journaling starts immediately at a non-colliding name; the stale
        // file is untouched and survives an undecided shutdown.
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

        // The journal migrated to the project's name and the unsaved file
        // is gone; a later crash only offers post-save commands, attributed
        // to the right project.
        assert_ne!(recovery.path, unsaved_path);
        assert!(!unsaved_path.exists());
        let second = JournalCommand::Do(Operation::AddAsset { asset: asset(2) });
        let _ = core.request(Command::from(second)).unwrap();
        recovery.recorder.as_ref().unwrap().flush().unwrap();
        let Inspection::Recoverable(report) = inspect_path(&recovery.path) else {
            panic!("expected checkpoint journal");
        };
        assert_eq!(report.recovered_commands, 1);
        assert_eq!(report.project_path.as_deref(), Some(project.as_path()));
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
        // The project's own crash journal is pending: allocate a suffix so
        // the undecided data is never truncated by the new session.
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
}
