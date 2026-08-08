//! Crash recovery for the operation spine.
//!
//! The active journal lives at
//! `%LOCALAPPDATA%/OpenReel/recovery/active.journal` (with the system temporary
//! directory as a fallback when `LOCALAPPDATA` is unavailable). The file is a
//! versioned preamble, a complete initial `Document` JSON line, then one
//! `JournalCommand` JSON line per accepted Core change. A trailing newline is
//! the commit marker for each line.
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
use openreel_core::{Core, Document, Event, JournalCommand};
use serde::{Deserialize, Serialize};

const MAGIC: &[u8] = b"OPENREEL-JOURNAL 1\n";
const FORMAT_VERSION: u32 = 1;
const CORE_RESPONSE_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Serialize, Deserialize)]
struct JournalHeader {
    format_version: u32,
    initial_document: Document,
}

struct JournalWriter {
    file: File,
    sync_each_record: bool,
}

impl JournalWriter {
    fn create(path: &Path, initial_document: &Document) -> Result<Self, String> {
        Self::create_with_sync(path, initial_document, true)
    }

    fn create_with_sync(
        path: &Path,
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
                format!("could not create recovery journal {}: {error}", path.display())
            })?;
        let header = serde_json::to_vec(&JournalHeader {
            format_version: FORMAT_VERSION,
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
    initial_document: Document,
    commands: Vec<ParsedCommand>,
    #[cfg(test)]
    header_end: usize,
    damage: Option<Damage>,
    byte_len: usize,
}

#[derive(Debug, Clone)]
struct RecoveryReport {
    document: Document,
    recovered_commands: usize,
    damage: Option<Damage>,
}

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
        let writer = JournalWriter::create(path, &initial_document)?;
        let (control, controls) = unbounded();
        let worker = thread::Builder::new()
            .name("openreel-recovery".to_owned())
            .spawn(move || recorder_loop(writer, events, controls, runtime_error))
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
    events: Receiver<Event>,
    controls: Receiver<RecorderControl>,
    runtime_error: Arc<Mutex<Option<String>>>,
) {
    loop {
        select! {
            recv(controls) -> message => match message {
                #[cfg(test)]
                Ok(RecorderControl::Flush(acknowledge)) => {
                    if !drain_events(&events, &mut writer, &runtime_error) {
                        let _ = acknowledge.send(());
                        break;
                    }
                    let _ = acknowledge.send(());
                }
                Ok(RecorderControl::Stop(acknowledge)) => {
                    let _ = drain_events(&events, &mut writer, &runtime_error);
                    let _ = acknowledge.send(());
                    break;
                }
                Err(_) => break,
            },
            recv(events) -> event => match event {
                Ok(event) => {
                    if !record_event(&mut writer, event, &runtime_error) {
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
    mutex.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

enum PendingRecovery {
    Recoverable(RecoveryReport),
    Unusable(Damage),
}

/// App-owned recovery lifecycle. All filesystem and UI behavior stays here.
pub(crate) struct Recovery {
    path: PathBuf,
    recorder: Option<Recorder>,
    pending: Option<PendingRecovery>,
    retained_stale_journal: bool,
    runtime_error: Arc<Mutex<Option<String>>>,
}

impl Recovery {
    pub(crate) fn start(core: &Core) -> Self {
        Self::start_at(core, default_journal_path())
    }

    fn start_at(core: &Core, path: PathBuf) -> Self {
        let inspection = inspect_path(&path);
        let runtime_error = Arc::new(Mutex::new(None));
        let mut recovery = Self {
            path,
            recorder: None,
            pending: None,
            retained_stale_journal: false,
            runtime_error,
        };
        match inspection {
            Inspection::Missing => recovery.attach(core),
            Inspection::Recoverable(report) => {
                recovery.pending = Some(PendingRecovery::Recoverable(report));
            }
            Inspection::Unusable(damage) => {
                recovery.pending = Some(PendingRecovery::Unusable(damage));
            }
        }
        recovery
    }

    /// Finish the previous project's journal and begin from Core's authoritative
    /// current snapshot. Core's initial subscription event makes this race-free
    /// with edits from the agent server.
    pub(crate) fn attach(&mut self, core: &Core) {
        if self.pending.is_some() {
            return;
        }
        self.stop_active();
        self.remove_journal();
        self.retained_stale_journal = false;
        *lock_unpoisoned(&self.runtime_error) = None;
        match Recorder::start(core, &self.path, Arc::clone(&self.runtime_error)) {
            Ok(recorder) => {
                self.recorder = Some(recorder);
            }
            Err(error) => {
                *lock_unpoisoned(&self.runtime_error) = Some(error);
            }
        }
    }

    /// Saving establishes a new baseline, so a later crash only offers edits
    /// made after the successful save.
    pub(crate) fn checkpoint(&mut self, core: &Core) {
        self.attach(core);
    }

    /// Render the startup decision and return the recovered document only when
    /// the user explicitly chooses restore. Discard starts journaling the
    /// already-running default project immediately.
    pub(crate) fn show_dialog(
        &mut self,
        ctx: &egui::Context,
        current_core: &Core,
    ) -> Option<Document> {
        let mut restore = false;
        let mut discard = false;
        if let Some(pending) = &self.pending {
            egui::Window::new("Recover unsaved work?")
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    match pending {
                        PendingRecovery::Recoverable(report) => {
                            if report.recovered_commands == 0 {
                                ui.label(
                                    "OpenReel closed unexpectedly. The journal's initial project snapshot can be restored.",
                                );
                            } else {
                                ui.label(format!(
                                    "OpenReel closed before this session was saved. {} journaled command{} can be restored.",
                                    report.recovered_commands,
                                    if report.recovered_commands == 1 { "" } else { "s" }
                                ));
                            }
                            if let Some(damage) = &report.damage {
                                ui.colored_label(
                                    egui::Color32::YELLOW,
                                    damage_description(damage),
                                );
                            }
                        }
                        PendingRecovery::Unusable(damage) => {
                            ui.colored_label(
                                egui::Color32::YELLOW,
                                "A stale recovery journal was found, but its initial snapshot is unavailable.",
                            );
                            ui.label(damage_description(damage));
                        }
                    }
                    ui.separator();
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(
                                matches!(pending, PendingRecovery::Recoverable(_)),
                                egui::Button::new("Restore unsaved work"),
                            )
                            .clicked()
                        {
                            restore = true;
                        }
                        if ui.button("Discard recovery").clicked() {
                            discard = true;
                        }
                    });
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

        if restore {
            let Some(PendingRecovery::Recoverable(report)) = self.pending.take() else {
                return None;
            };
            // Keep the stale file until `attach` succeeds during Core replacement.
            // If replacement fails, the next launch can still recover it.
            self.retained_stale_journal = true;
            return Some(report.document);
        }
        if discard {
            self.pending = None;
            self.remove_journal();
            self.attach(current_core);
        }
        None
    }

    fn stop_active(&mut self) {
        if let Some(mut recorder) = self.recorder.take() {
            recorder.shutdown();
        }
    }

    fn remove_journal(&self) {
        if let Err(error) = fs::remove_file(&self.path)
            && error.kind() != io::ErrorKind::NotFound
        {
            *lock_unpoisoned(&self.runtime_error) = Some(format!(
                "could not remove recovery journal {}: {error}",
                self.path.display()
            ));
        }
    }
}

impl Drop for Recovery {
    fn drop(&mut self) {
        self.stop_active();
        if !self.retained_stale_journal && self.pending.is_none() {
            self.remove_journal();
        }
    }
}

fn default_journal_path() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("OpenReel")
        .join("recovery")
        .join("active.journal")
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

    use openreel_core::{
        AssetId, ClipId, Command, Effect, EffectId, Event, MediaAsset, MediaKind, Operation,
        Rational, TimeCode, Track, TrackId, TrackKind,
    };
    use proptest::prelude::*;

    use super::*;

    static NEXT_TEST_DIRECTORY: AtomicU64 = AtomicU64::new(1);

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new(label: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "openreel-recovery-{label}-{}-{}",
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
                    parameters: Default::default(),
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
                Event::QueryResult(_) => panic!("unexpected query result"),
            }
        }
        document
    }

    fn encoded_journal(initial: &Document, commands: &[JournalCommand]) -> Vec<u8> {
        let mut bytes = MAGIC.to_vec();
        bytes.extend(
            serde_json::to_vec(&JournalHeader {
                format_version: FORMAT_VERSION,
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

    #[test]
    fn journal_round_trip_preserves_agent_ops_and_undo_redo_history() {
        let directory = TestDirectory::new("round-trip");
        let initial = Document::default();
        let commands = representative_commands();
        let mut writer = JournalWriter::create(&directory.journal(), &initial).unwrap();
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
        let mut recorder =
            Recorder::start(&core, &directory.journal(), Arc::clone(&runtime_error)).unwrap();
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
        let Some(path) = std::env::var_os("OPENREEL_TEST_CRASH_JOURNAL").map(PathBuf::from)
        else {
            return;
        };
        let core = Core::spawn(Document::default()).unwrap();
        let runtime_error = Arc::new(Mutex::new(None));
        let recorder = Recorder::start(&core, &path, runtime_error).unwrap();
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
            .env("OPENREEL_TEST_CRASH_JOURNAL", &path)
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
    fn clean_drop_removes_the_active_journal() {
        let directory = TestDirectory::new("clean-drop");
        let path = directory.journal();
        let core = Core::spawn(Document::default()).unwrap();
        {
            let recovery = Recovery::start_at(&core, path.clone());
            assert!(path.is_file());
            drop(recovery);
        }
        assert!(!path.exists());
    }

    #[test]
    fn even_a_header_only_stale_journal_requires_a_user_decision() {
        let directory = TestDirectory::new("header-only");
        let path = directory.journal();
        drop(JournalWriter::create(&path, &Document::default()).unwrap());
        let core = Core::spawn(Document::default()).unwrap();

        let recovery = Recovery::start_at(&core, path.clone());
        assert!(matches!(
            &recovery.pending,
            Some(PendingRecovery::Recoverable(RecoveryReport {
                recovered_commands: 0,
                ..
            }))
        ));
        assert!(recovery.recorder.is_none());
        drop(recovery);
        assert!(path.is_file(), "unresolved recovery must survive shutdown");
    }

    #[test]
    fn successful_save_checkpoint_discards_pre_save_commands() {
        let directory = TestDirectory::new("checkpoint");
        let path = directory.journal();
        let core = Core::spawn(Document::default()).unwrap();
        let mut recovery = Recovery::start_at(&core, path.clone());
        let first = JournalCommand::Do(Operation::AddAsset { asset: asset(1) });
        let _ = core.request(Command::from(first)).unwrap();
        recovery.recorder.as_ref().unwrap().flush().unwrap();
        recovery.checkpoint(&core);

        let second = JournalCommand::Do(Operation::AddAsset { asset: asset(2) });
        let _ = core.request(Command::from(second)).unwrap();
        recovery.recorder.as_ref().unwrap().flush().unwrap();
        let Inspection::Recoverable(report) = inspect_path(&path) else {
            panic!("expected checkpoint journal");
        };
        assert_eq!(report.recovered_commands, 1);
        assert!(report.document.asset(AssetId(1)).is_some());
        assert!(report.document.asset(AssetId(2)).is_some());
    }
}
