//! The UI-free sidecar session (AW1 §2 B2, R6): the session half of the
//! sidecar path, extracted so headless saves share it. The app keeps a thin
//! wrapper for panel notes and derefs here. One logic change: an unloaded
//! session never wipes an occupied stem with an empty flush (GUARD-B).

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
    time::Instant,
};

use kinewright_core::{
    IncidentId, IncidentLog, Operation, RestoreReport, RunningInvestigation, TimelineRevision,
    WriteReport, should_flush,
};

use crate::{
    project::write_file_atomic,
    sidecar::{
        FlushOutcome, RefuseRename, SidecarLoad, SidecarMode, SidecarWriter, build_sidecar_bytes,
        digest_bytes, load_sidecar, refuse_sidecar, refuse_sidecar_with, sidecar_matches_project,
        sidecar_path_for_project, sidecar_refused_observation,
    },
};

/// The incident log handle a session shares with its agent servers. Same
/// type as the app's and the agent crate's aliases; defined here so the
/// session's public API names it.
pub type IncidentLogHandle = Arc<RwLock<IncidentLog>>;

/// The UI-free half of a project session: everything the sidecar load and
/// flush need, with the investigator context passed per call.
pub struct SidecarSession {
    pub saved_digest: String,
    pub sidecar_writer: Arc<SidecarWriter>,
    pub carried_sidecar_records: Vec<String>,
    pub refused_by_id: BTreeMap<IncidentId, Operation>,
    pub last_written_gen: u64,
    pub confirmed_written_gen: u64,
    /// What the open-time load did; read by the IN2B gate only.
    pub last_restore_report: Option<RestoreReport>,
    /// Suspended: loads run, every flush reports `Skipped` (rule 10, H3).
    pub sidecar_suspended: bool,
    /// Stems with a baseline: loaded, flushed, or adopted (F1). GUARD-B
    /// applies only outside this set.
    pub established: BTreeSet<PathBuf>,
    pub incidents: IncidentLogHandle,
}

/// What [`SidecarSession::load`] hands back: the session plus the wall
/// stamps the app's panel keeps (`loaded_walls`, `IN2B` §3 rule 14).
pub struct LoadedSidecarSession {
    pub session: SidecarSession,
    pub loaded_walls: BTreeMap<IncidentId, Option<i64>>,
}

/// What the open-time sidecar load hands the new session (`IN2B` §2 rule 5).
struct LoadedSessionSidecar {
    saved_digest: String,
    carried: Vec<String>,
    refused: BTreeMap<IncidentId, Operation>,
    walls: BTreeMap<IncidentId, Option<i64>>,
    last_written_gen: u64,
    report: Option<RestoreReport>,
    /// A refused sidecar the load could not move aside (N6/H3): the session
    /// suspends sidecar writes, so the still-at-the-stem file is never
    /// overwritten.
    suspended: bool,
}

impl LoadedSessionSidecar {
    fn empty() -> Self {
        Self {
            saved_digest: String::new(),
            carried: Vec::new(),
            refused: BTreeMap::new(),
            walls: BTreeMap::new(),
            last_written_gen: 0,
            report: None,
            suspended: false,
        }
    }
}

/// Load a session's history at creation: parse, gate, restore or refuse.
///
/// Every arm opens the project — incidents are recoverable state, the
/// timeline is not — and no arm deletes a sidecar it cannot read (rule 8).
/// Refusals rename to first-free `.bak` and note exactly one
/// `sidecar_refused` directly into the still-private log (the router is not
/// running yet; the code is off-allowlist, so no session could start
/// anyway). Restore runs before any note (N4.1).
///
/// `Load` carries the digest `load_document` read — single read, no TOCTOU
/// (§4 rule 2). `RecoveryNoDigest` skips the gate by rule; version arms
/// still apply.
fn load_session_sidecar(
    mode: &SidecarMode,
    project_path: Option<&Path>,
    incidents: &IncidentLogHandle,
    opening: TimelineRevision,
    rename: Option<&RefuseRename>,
) -> LoadedSessionSidecar {
    let (gate_digest, seed) = match mode {
        SidecarMode::Load { project_digest } => {
            (Some(project_digest.clone()), Some(project_digest.clone()))
        }
        // N6/H1: recovery skips the gate (the recovered document is newer
        // than the last save), but the seed still comes from the file on
        // disk — the first flush pairs instead of writing `{"",""}`.
        SidecarMode::RecoveryNoDigest => (
            None,
            project_path
                .and_then(|path| fs::read(path).ok())
                .map(|bytes| digest_bytes(&bytes)),
        ),
        SidecarMode::None => return LoadedSessionSidecar::empty(),
    };
    let Some(project_path) = project_path else {
        return LoadedSessionSidecar::empty();
    };
    let Some(sidecar_path) = sidecar_path_for_project(Some(project_path)) else {
        return LoadedSessionSidecar::empty();
    };
    let mut loaded = LoadedSessionSidecar::empty();
    if let Some(digest) = &seed {
        loaded.saved_digest.clone_from(digest);
    }
    // N6/H3: a failed `.bak` rename suspends the session — the refused
    // file stays at the stem and must never be overwritten — and the note
    // carries the IO error. Returns whether the rename failed.
    let refused = |reason: String| -> bool {
        let renamed = match rename {
            Some(injected) => refuse_sidecar_with(&sidecar_path, injected),
            None => refuse_sidecar(&sidecar_path),
        };
        let (reason, failed) = match renamed {
            Ok(_) => (reason, false),
            Err(error) => (
                format!(
                    "{reason}; the refused file could not be moved aside ({error}), so the \
                     incidents file is set aside and sidecar writes are suspended for this session"
                ),
                true,
            ),
        };
        let mut log = incidents
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _ = log.observe(sidecar_refused_observation(reason, opening));
        failed
    };
    let mut refuse_failed = false;
    match load_sidecar(&sidecar_path) {
        SidecarLoad::Absent => {}
        SidecarLoad::Newer(version) => {
            refuse_failed |= refused(format!(
                "the sidecar was written by a newer Kinewright (format_version {version}); \
                 the project opens with an empty history and the file is kept beside it"
            ));
        }
        SidecarLoad::Corrupt(reason) => {
            refuse_failed |= refused(format!(
                "the sidecar could not be read ({reason}); \
                 the project opens with an empty history and the file is kept beside it"
            ));
        }
        SidecarLoad::Current(current) => {
            let gated = match &gate_digest {
                Some(digest) => sidecar_matches_project(&current, digest),
                None => true,
            };
            if !gated {
                refuse_failed |= refused(
                    "the sidecar belongs to a different project file (neither digest matches); \
                     the project opens with an empty history and the file is kept beside it"
                        .to_owned(),
                );
                loaded.suspended = refuse_failed;
                return loaded;
            }
            // N6/H1 fallback: no file on disk (an unsaved recovery target)
            // seeds from the loaded sidecar's own digest instead of `""`.
            // The `gate_digest.is_none()` guard restricts this to recovery
            // mode — a gated Load keeps whatever its read seeded, so the
            // fallback cannot mask a refusal.
            if loaded.saved_digest.is_empty() && gate_digest.is_none() {
                loaded.saved_digest.clone_from(&current.project_digest);
            }
            // The loaded wall stamps snapshot before the records move into
            // restore (`IN2B` §3 rule 14): the card's recency derives from
            // them app-side.
            loaded.walls = current
                .records
                .iter()
                .map(|record| (record.id, record.opened_wall_millis))
                .collect();
            let mut log = incidents
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let report = log.restore(current.records, current.carried, opening, current.id_floor);
            // The disk now matches memory (carried bytes included — a
            // carried-only restore moves no generation, and must not force a
            // rewrite of bytes already on disk), so the flush baseline is the
            // post-restore generation, whatever it is.
            loaded.last_written_gen = log.generation();
            loaded.carried.clone_from(&report.carried);
            loaded.refused.clone_from(&report.refused);
            loaded.report = Some(report);
        }
    }
    loaded.suspended = refuse_failed;
    loaded
}

impl SidecarSession {
    /// Load a session's history (`None` writer spawns a private one).
    #[must_use]
    pub fn load(
        mode: &SidecarMode,
        project_path: Option<&Path>,
        incidents: IncidentLogHandle,
        opening: TimelineRevision,
        writer: Option<Arc<SidecarWriter>>,
        rename: Option<&RefuseRename>,
    ) -> LoadedSidecarSession {
        let stem = (!matches!(mode, SidecarMode::None))
            .then(|| project_path.and_then(|path| sidecar_path_for_project(Some(path))))
            .flatten();
        let established = BTreeSet::from_iter(stem);
        let loaded = load_session_sidecar(mode, project_path, &incidents, opening, rename);
        LoadedSidecarSession {
            session: Self {
                saved_digest: loaded.saved_digest,
                sidecar_writer: writer.unwrap_or_else(SidecarWriter::new),
                carried_sidecar_records: loaded.carried,
                refused_by_id: loaded.refused,
                last_written_gen: loaded.last_written_gen,
                confirmed_written_gen: loaded.last_written_gen,
                last_restore_report: loaded.report,
                sidecar_suspended: loaded.suspended,
                established,
                incidents,
            },
            loaded_walls: loaded.walls,
        }
    }

    /// Build this session's sidecar bytes without writing them (`IN2B` §2
    /// rule 5).
    ///
    /// Records are built on the caller under a read lock; the running
    /// session's turns and visible counters flush into an `Investigating`
    /// entry exactly as a live end would (§3 rule 4), the restored stash
    /// rides by id (rule 13), and carried texts re-emit verbatim (rule 8b).
    /// Both the joining flush and the background submit build through here,
    /// so one builder means one bytes shape. `prepare` runs at write time;
    /// headless passes `|_| None`.
    /// # Errors
    /// Returns the envelope serialisation failure as a string.
    pub fn sidecar_bytes_for_save(
        &mut self,
        project_digest: &str,
        previous_digest: &str,
        prepare: impl FnOnce(&mut BTreeMap<IncidentId, Operation>) -> Option<RunningInvestigation>,
    ) -> Result<(Vec<u8>, WriteReport), String> {
        // The context prepares at write, never on a skip (F2): the
        // running sample leads, then the queued refused ops copy in.
        let running = prepare(&mut self.refused_by_id);
        let log = self
            .incidents
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let open: BTreeSet<IncidentId> = log.open().map(|incident| incident.id).collect();
        self.refused_by_id.retain(|id, _| open.contains(id));
        let (records, report) = log.records(running.as_ref(), &self.refused_by_id);
        let bytes = build_sidecar_bytes(
            &records,
            &self.carried_sidecar_records,
            project_digest,
            previous_digest,
        )?;
        Ok((bytes, report))
    }

    /// The one synchronous sidecar seam: build every write and land it
    /// through the writer thread, joining (`IN2B` §2 rules 4–5).
    ///
    /// `Ok` after a successful temp + rename (report populated), `Err` after
    /// a failed one — the caller notes `sidecar_write_failed` per rule 6 and
    /// carries on. Unsaved projects report `Skipped` and attempt no IO
    /// (rule 10, N-5), as do sidecar-suspended recovery restores (§2
    /// rule 10).
    /// # Errors
    /// Returns the build failure or the writer job's IO error.
    pub fn flush_incidents(
        &mut self,
        project_path: Option<&Path>,
        project_digest: &str,
        previous_digest: &str,
        prepare: impl FnOnce(&mut BTreeMap<IncidentId, Operation>) -> Option<RunningInvestigation>,
    ) -> std::io::Result<FlushOutcome> {
        if self.sidecar_suspended {
            return Ok(FlushOutcome::Skipped);
        }
        let Some(sidecar_path) = sidecar_path_for_project(project_path) else {
            return Ok(FlushOutcome::Skipped);
        };
        let (bytes, report) = self
            .sidecar_bytes_for_save(project_digest, previous_digest, prepare)
            .map_err(std::io::Error::other)?;
        // GUARD-B (R6/F1): an unestablished stem keeps its history
        // against an empty flush; fresh, established, and non-empty
        // flushes are untouched.
        if !self.established.contains(&sidecar_path)
            && report.written_open == 0
            && report.written_resolved == 0
            && self.carried_sidecar_records.is_empty()
            && sidecar_path.exists()
        {
            return Ok(FlushOutcome::Skipped);
        }
        self.sidecar_writer
            .submit_and_join(sidecar_path.clone(), bytes)?;
        let generation = self
            .incidents
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .generation();
        self.last_written_gen = generation;
        // N6/H6: a joined success confirms; a failure propagates before
        // either baseline moves, so close retries it.
        self.confirmed_written_gen = generation;
        // F1: the write landed, so the stem is established.
        self.established.insert(sidecar_path);
        Ok(FlushOutcome::Written(report))
    }

    /// [`Self::flush_incidents`] when the writer has not confirmed the
    /// current generation, `Skipped` otherwise — the close/exit shape
    /// (`IN2B` §2 rule 4).
    ///
    /// Close write-and-waits whenever `confirmed_written_gen` trails the
    /// log (N6/H6): a failed background flush advanced the submit baseline
    /// but never confirmed, so it retries here instead of reading as done.
    /// A successful background flush still lands one redundant identical
    /// rewrite at close — async jobs carry no ack, and one idempotent write
    /// per close is cheaper than success tracking. Saves do not call this —
    /// a save always rewrites the pair over new bytes.
    /// # Errors
    /// Returns the joining flush's IO error when the write was attempted.
    pub fn flush_incidents_if_changed(
        &mut self,
        project_path: Option<&Path>,
        prepare: impl FnOnce(&mut BTreeMap<IncidentId, Operation>) -> Option<RunningInvestigation>,
    ) -> std::io::Result<FlushOutcome> {
        if self.sidecar_suspended || project_path.is_none() {
            return Ok(FlushOutcome::Skipped);
        }
        let generation = self
            .incidents
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .generation();
        if generation == self.confirmed_written_gen {
            return Ok(FlushOutcome::Skipped);
        }
        let digest = self.saved_digest.clone();
        self.flush_incidents(project_path, &digest, &digest, prepare)
    }

    /// Queue a debounced background flush without joining (`IN2B` §2 rule 4).
    ///
    /// Unchanged logs and unsaved projects queue nothing. Failures surface
    /// through [`SidecarWriter::take_errors`], which the frame thread drains
    /// into `sidecar_write_failed` notes. The submit baseline advances
    /// optimistically at submit — a failed submit's incident is the retry
    /// signal, not a 2 s resubmit churn — while `confirmed_written_gen`
    /// waits for a joined success, so close retries what the background
    /// could not land (N6/H6).
    pub fn queue_incidents_flush(
        &mut self,
        project_path: Option<&Path>,
        prepare: impl FnOnce(&mut BTreeMap<IncidentId, Operation>) -> Option<RunningInvestigation>,
    ) {
        if self.sidecar_suspended {
            return;
        }
        let Some(sidecar_path) = sidecar_path_for_project(project_path) else {
            return;
        };
        let generation = self
            .incidents
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .generation();
        if !should_flush(generation, self.last_written_gen, Instant::now()) {
            return;
        }
        let digest = self.saved_digest.clone();
        if let Ok((bytes, _)) = self.sidecar_bytes_for_save(&digest, &digest, prepare) {
            self.sidecar_writer.submit(sidecar_path, bytes);
            self.last_written_gen = generation;
        }
    }
}

/// H12 rollback plan (N6.1/J3): restore, remove, or skip (never delete unreadable).
pub enum SidecarRollback {
    Restore(Vec<u8>),
    Remove,
    Skip,
}

/// Snapshot the H12 rollback (N6.1/J3, F6): shared by app and headless.
#[must_use]
pub fn snapshot_sidecar_rollback(sidecar: Option<&Path>) -> SidecarRollback {
    let Some(sidecar) = sidecar else {
        return SidecarRollback::Skip;
    };
    match fs::read(sidecar) {
        Ok(bytes) => SidecarRollback::Restore(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => SidecarRollback::Remove,
        Err(_) => SidecarRollback::Skip,
    }
}

/// Run the H12 rollback (F6); best-effort, degrades to a re-flush.
pub fn rollback_sidecar_write(sidecar: Option<PathBuf>, plan: SidecarRollback) {
    let Some(sidecar) = sidecar else {
        return;
    };
    match plan {
        SidecarRollback::Restore(bytes) => {
            let _ = write_file_atomic(&sidecar, &bytes);
        }
        SidecarRollback::Remove => {
            let _ = fs::remove_file(sidecar);
        }
        SidecarRollback::Skip => {}
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Instant, SystemTime};

    use kinewright_core::{
        Document, IncidentCode, IncidentObservation, IncidentRecord, IncidentSubject, LabelIncident,
    };
    use kinewright_media::test_support::TempDirectory;

    use crate::project::write_project_document;

    use super::*;

    fn empty_log() -> IncidentLogHandle {
        Arc::new(RwLock::new(IncidentLog::with_start(
            Instant::now(),
            Some(SystemTime::now()),
        )))
    }

    /// Two records through the production builder.
    fn two_records() -> Vec<IncidentRecord> {
        let mut log = IncidentLog::with_start(Instant::now(), Some(SystemTime::UNIX_EPOCH));
        for n in 0..2 {
            log.observe(IncidentObservation::plain(
                IncidentCode::Label(LabelIncident::Project),
                IncidentSubject::Project,
                format!("aw1 r6 record {n}"),
                TimelineRevision::default(),
            ));
        }
        let (records, _) = log.records(None, &BTreeMap::new());
        assert_eq!(records.len(), 2);
        records
    }

    /// R6: an unloaded session never wipes an occupied stem with an empty
    /// flush — the scratch probe showed `Written(0/0)` overwriting 2 stem
    /// records on unmodified code; the guard skips instead.
    #[test]
    fn r6_unloaded_empty_flush_skips_and_preserves_the_stem() {
        let dir = TempDirectory::new("aw1-r6-guard");
        let project = dir.path("edit.kinewright");
        write_project_document(&Document::default(), &project, None).expect("project writes");
        let digest = digest_bytes(&fs::read(&project).expect("the project reads"));
        let sidecar = sidecar_path_for_project(Some(&project)).expect("a saved project derives");
        let bytes =
            build_sidecar_bytes(&two_records(), &[], &digest, &digest).expect("the sidecar builds");
        fs::write(&sidecar, &bytes).expect("the occupied stem writes");
        let loaded = SidecarSession::load(
            &SidecarMode::None,
            Some(&project),
            empty_log(),
            TimelineRevision::default(),
            None,
            None,
        );
        assert!(loaded.session.established.is_empty(), "no load ran");
        let mut session = loaded.session;
        assert_eq!(
            session
                .flush_incidents(Some(&project), &digest, &digest, |_| None)
                .expect("the flush reports"),
            FlushOutcome::Skipped,
            "an empty unloaded flush skips"
        );
        assert_eq!(
            fs::read(&sidecar).expect("the stem re-reads"),
            bytes,
            "the stem history survives"
        );
    }

    /// R6: the guard is unloaded-only — a loaded session's empty flush
    /// still attempts IO, so failures note exactly as the app's
    /// write-failure gate requires.
    #[test]
    fn r6_loaded_empty_flush_still_attempts_io() {
        let dir = TempDirectory::new("aw1-r6-loaded-attempts");
        let project = dir.path("edit.kinewright");
        write_project_document(&Document::default(), &project, None).expect("project writes");
        let digest = digest_bytes(&fs::read(&project).expect("the project reads"));
        let loaded = SidecarSession::load(
            &SidecarMode::Load {
                project_digest: digest.clone(),
            },
            Some(&project),
            empty_log(),
            TimelineRevision::default(),
            None,
            None,
        );
        assert!(!loaded.session.established.is_empty(), "the load ran");
        let mut session = loaded.session;
        let sidecar = sidecar_path_for_project(Some(&project)).expect("a saved project derives");
        fs::create_dir(&sidecar).expect("the sidecar path is occupied");
        let error = session
            .flush_incidents(Some(&project), &digest, &digest, |_| None)
            .expect_err("a loaded empty flush attempts IO");
        assert!(!error.to_string().is_empty(), "the IO error reports");
    }

    /// R6: a fresh stem still gets its first sidecar — the guard only
    /// skips over occupied stems, preserving first-save behaviour.
    #[test]
    fn r6_fresh_stem_gets_its_first_sidecar() {
        let dir = TempDirectory::new("aw1-r6-fresh-stem");
        let project = dir.path("edit.kinewright");
        write_project_document(&Document::default(), &project, None).expect("project writes");
        let digest = digest_bytes(&fs::read(&project).expect("the project reads"));
        let loaded = SidecarSession::load(
            &SidecarMode::None,
            Some(&project),
            empty_log(),
            TimelineRevision::default(),
            None,
            None,
        );
        assert!(loaded.session.established.is_empty(), "no load ran");
        let mut session = loaded.session;
        let outcome = session
            .flush_incidents(Some(&project), &digest, &digest, |_| None)
            .expect("the flush reports");
        assert!(
            matches!(outcome, FlushOutcome::Written(_)),
            "a fresh stem is written, got {outcome:?}"
        );
        let sidecar = sidecar_path_for_project(Some(&project)).expect("a saved project derives");
        let SidecarLoad::Current(current) = load_sidecar(&sidecar) else {
            panic!("the first sidecar parses");
        };
        assert!(current.records.is_empty(), "empty, as flushed");
    }

    /// F1/B1: a stem this session wrote is established — the second save's
    /// empty flush pairs instead of skipping, so the reopen finds no `.bak`.
    #[test]
    fn f1_second_empty_flush_to_a_written_stem_pairs() {
        let dir = TempDirectory::new("aw1-f1-second-save");
        let project = dir.path("edit.kinewright");
        write_project_document(&Document::default(), &project, None).expect("project writes");
        let first = digest_bytes(&fs::read(&project).expect("the project reads"));
        let loaded = SidecarSession::load(
            &SidecarMode::None,
            Some(&project),
            empty_log(),
            TimelineRevision::default(),
            None,
            None,
        );
        let mut session = loaded.session;
        assert!(
            matches!(
                session
                    .flush_incidents(Some(&project), &first, &first, |_| None)
                    .expect("the first flush reports"),
                FlushOutcome::Written(_)
            ),
            "the fresh stem is written"
        );
        // The document edit advances the project digest, not the log: the
        // second flush is still empty.
        let second = digest_bytes(b"edited project bytes");
        let outcome = session
            .flush_incidents(Some(&project), &second, &first, |_| None)
            .expect("the second flush reports");
        assert!(
            matches!(outcome, FlushOutcome::Written(_)),
            "a written stem pairs, got {outcome:?}"
        );
        let sidecar = sidecar_path_for_project(Some(&project)).expect("a saved project derives");
        let SidecarLoad::Current(current) = load_sidecar(&sidecar) else {
            panic!("the paired sidecar parses");
        };
        assert_eq!(current.project_digest, second);
        assert_eq!(current.previous_digest, first);
    }

    /// F1: legitimate clearing — resolving the last incident empties the
    /// stem on the next save; establishment is not a refusal to clear.
    #[test]
    fn f1_resolving_the_last_incident_empties_the_stem() {
        let dir = TempDirectory::new("aw1-f1-clearing");
        let project = dir.path("edit.kinewright");
        write_project_document(&Document::default(), &project, None).expect("project writes");
        let digest = digest_bytes(&fs::read(&project).expect("the project reads"));
        let sidecar = sidecar_path_for_project(Some(&project)).expect("a saved project derives");
        let bytes =
            build_sidecar_bytes(&two_records(), &[], &digest, &digest).expect("the sidecar builds");
        fs::write(&sidecar, &bytes).expect("the occupied stem writes");
        let loaded = SidecarSession::load(
            &SidecarMode::Load {
                project_digest: digest.clone(),
            },
            Some(&project),
            empty_log(),
            TimelineRevision::default(),
            None,
            None,
        );
        let mut session = loaded.session;
        assert_eq!(
            session
                .incidents
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .open_count(),
            2,
            "both records restore"
        );
        session
            .incidents
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove_open_with_code(IncidentCode::Label(LabelIncident::Project));
        let outcome = session
            .flush_incidents(Some(&project), &digest, &digest, |_| None)
            .expect("the clearing flush reports");
        assert!(
            matches!(outcome, FlushOutcome::Written(_)),
            "clearing writes, got {outcome:?}"
        );
        let SidecarLoad::Current(current) = load_sidecar(&sidecar) else {
            panic!("the cleared sidecar parses");
        };
        assert!(current.records.is_empty(), "the stem is emptied");
    }
}
