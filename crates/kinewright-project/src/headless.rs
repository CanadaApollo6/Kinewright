//! Headless save orchestration (AW1 §2): serialize → sidecar flush →
//! project bytes → journal retire. The caller gates newer-format overwrite
//! first, as the app's save prologue does.

use std::path::Path;

use kinewright_core::{Document, TimelineRevision};
use kinewright_media::LutStore;

use crate::{
    project::{ProjectSaveError, ProjectSaveReport, serialize_project_document},
    session::{SidecarSession, rollback_sidecar_write, snapshot_sidecar_rollback},
    sidecar::{
        FlushOutcome, digest_bytes, refuse_sidecar, sidecar_path_for_project,
        sidecar_write_failed_observation,
    },
};

/// What one headless save did.
pub struct HeadlessSaveReport {
    pub project: ProjectSaveReport,
    /// The sidecar flush: `Err` holds a failed flush's IO error, which
    /// never fails the save (`IN2B` §2 rule 6).
    pub sidecar: Result<FlushOutcome, String>,
}

/// Save a document headless (joining flush on every save, R6). Refuses
/// fail-closed for an unloaded session.
/// # Errors
/// `SessionNotLoaded`, `Serialize`, or `Write`.
pub fn save_headless(
    document: &Document,
    path: &Path,
    previous_store: Option<&LutStore>,
    sidecar: &mut SidecarSession,
    previous_digest: &str,
    revision: TimelineRevision,
    recovery_dir: &Path,
) -> Result<HeadlessSaveReport, ProjectSaveError> {
    if sidecar.established.is_empty() {
        return Err(ProjectSaveError::SessionNotLoaded);
    }
    let json = serialize_project_document(document)?;
    let new_digest = digest_bytes(json.as_bytes());
    // N6/H12+J2 (as in `write_project`): snapshot for rollback.
    let rollback_sidecar = sidecar_path_for_project(Some(path));
    let mut rollback_snapshot = snapshot_sidecar_rollback(rollback_sidecar.as_deref(), sidecar);
    let sidecar_outcome =
        match sidecar.flush_incidents(Some(path), &new_digest, previous_digest, |_| None) {
            Ok(outcome) => Ok(outcome),
            Err(error) => {
                let mut log = sidecar
                    .incidents
                    .write()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let _ = log.observe(sidecar_write_failed_observation(
                    error.to_string(),
                    revision,
                ));
                Err(error.to_string())
            }
        };
    // G3 preserve-and-replace (as in `write_project`): an occupied stem's
    // foreign history moves aside with the reopen path's `.bak` naming,
    // then the paired sidecar flushes onto the fresh stem. Either structural
    // failure fails the save — never an unpaired success.
    let sidecar_outcome = match sidecar_outcome {
        Ok(FlushOutcome::Occupied) => {
            let Some(stem) = sidecar_path_for_project(Some(path)) else {
                return Err(ProjectSaveError::Write(
                    "an occupied flush derived no stem".to_owned(),
                ));
            };
            let moved = refuse_sidecar(&stem)
                .map_err(|error| ProjectSaveError::Write(error.to_string()))?;
            rollback_snapshot.note_moved_aside(moved);
            match sidecar.flush_incidents(Some(path), &new_digest, previous_digest, |_| None) {
                Ok(FlushOutcome::Written(report)) => Ok(FlushOutcome::Written(report)),
                Err(error) => {
                    let mut log = sidecar
                        .incidents
                        .write()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    let _ = log.observe(sidecar_write_failed_observation(
                        error.to_string(),
                        revision,
                    ));
                    Err(error.to_string())
                }
                Ok(_) => {
                    return Err(ProjectSaveError::Write(
                        "the sidecar stem was re-occupied during Save As; \
                         not writing an unpaired project"
                            .to_owned(),
                    ));
                }
            }
        }
        outcome => outcome,
    };
    let project = match crate::project::write_project_bytes(&json, document, path, previous_store) {
        Ok(report) => report,
        Err(error) => {
            rollback_sidecar_write(rollback_snapshot, sidecar);
            return Err(error);
        }
    };
    // Landed bytes establish the stem (as in `adopt_saved_path`), even past
    // a benign skipped flush — later saves pair.
    if let Some(stem) = sidecar_path_for_project(Some(path)) {
        sidecar.established.insert(stem);
    }
    new_digest.clone_into(&mut sidecar.saved_digest);
    // F5: headless retires nothing (param kept for call-shape stability).
    let _ = recovery_dir;
    Ok(HeadlessSaveReport {
        project,
        sidecar: sidecar_outcome,
    })
}

#[cfg(test)]
mod tests {
    use kinewright_core::{IncidentCode, IncidentObservation, IncidentSubject, LabelIncident};
    use kinewright_media::test_support::TempDirectory;

    use crate::{
        project::{ProjectSaveError, load_document, write_project_document},
        recovery::{journal_file_name, pending_journal_for_project},
        session::LoadedSidecarSession,
        sidecar::{
            FlushOutcome, SidecarLoad, SidecarMode, build_sidecar_bytes, digest_bytes,
            load_sidecar, sidecar_path_for_project,
        },
    };

    use super::*;

    /// Observe one plain project incident into the session log.
    fn note(session: &SidecarSession, text: &str) {
        session
            .incidents
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .observe(IncidentObservation::plain(
                IncidentCode::Label(LabelIncident::Project),
                IncidentSubject::Project,
                text,
                TimelineRevision::default(),
            ));
    }

    /// A loaded headless session over `project`: the S4 open-then-save shape.
    fn loaded_session(project: &std::path::Path) -> SidecarSession {
        let digest =
            std::fs::read(project).map_or_else(|_| String::new(), |bytes| digest_bytes(&bytes));
        let log = std::sync::Arc::new(std::sync::RwLock::new(
            kinewright_core::IncidentLog::with_start(
                std::time::Instant::now(),
                Some(std::time::SystemTime::now()),
            ),
        ));
        let LoadedSidecarSession { session, .. } = SidecarSession::load(
            &SidecarMode::Load {
                project_digest: digest,
            },
            Some(project),
            log,
            TimelineRevision::default(),
            None,
            None,
        );
        assert!(!session.established.is_empty(), "the headless open loads");
        session
    }

    /// R1/R2/R5/R6: a headless save round-trips — the project re-loads
    /// identical, the sidecar pairs on the new digest, and the baseline
    /// advances. F5: an unreplayed pending journal survives the save —
    /// headless owns no journals, so it retires none.
    #[test]
    fn headless_save_preserves_an_unreplayed_journal() {
        let dir = TempDirectory::new("aw1-headless-round-trip");
        let project = dir.path("edit.kinewright");
        let recovery = dir.path("recovery");
        std::fs::create_dir(&recovery).expect("the recovery dir creates");
        let mut session = loaded_session(&project);
        {
            let mut log = session
                .incidents
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            log.observe(IncidentObservation::plain(
                IncidentCode::Label(LabelIncident::Project),
                IncidentSubject::Project,
                "aw1 headless record",
                TimelineRevision::default(),
            ));
        }
        let stale = recovery.join(journal_file_name(&project));
        std::fs::write(&stale, b"stale crash bytes").expect("the stale journal writes");

        let mut document = Document::default();
        document.markers.push(kinewright_core::Marker {
            id: kinewright_core::MarkerId(7),
            position: kinewright_core::TimeCode::ZERO,
            label: "headless edit".to_owned(),
            color_token: 0,
        });
        let report = save_headless(
            &document,
            &project,
            None,
            &mut session,
            "",
            TimelineRevision::default(),
            &recovery,
        )
        .expect("the headless save lands");
        assert!(
            matches!(report.sidecar, Ok(FlushOutcome::Written(_))),
            "headless flushes on every save"
        );
        let (reloaded, version, digest) = load_document(&project).expect("the save re-loads");
        assert_eq!(reloaded, document, "the round trip is identical");
        assert_eq!(version, kinewright_core::PROJECT_FORMAT_VERSION);
        assert_eq!(
            report.project.digest, digest,
            "one digest serves both files"
        );
        assert_eq!(session.saved_digest, digest, "the baseline advances");
        let sidecar = sidecar_path_for_project(Some(&project)).expect("a saved project derives");
        let SidecarLoad::Current(current) = load_sidecar(&sidecar) else {
            panic!("the headless sidecar parses");
        };
        assert_eq!(current.records.len(), 1, "the live log carried");
        assert_eq!(
            current.project_digest, digest,
            "the pair leads with the new digest"
        );
        assert!(
            current.previous_digest.is_empty(),
            "a first save has no previous"
        );
        assert!(
            pending_journal_for_project(&recovery, &project)
                .expect("the lookup lands")
                .is_some(),
            "the unreplayed journal survives the save"
        );
        assert!(stale.exists(), "headless retires nothing it did not replay");
    }

    /// G3/RB1 (headless): Save As with an empty log onto an occupied stem
    /// preserves-and-replaces — the foreign history moves to `.bak`, the
    /// stem pairs on the new (differing) document, and a reopen finds no
    /// `sidecar_refused` and no further `.bak`.
    #[test]
    fn g3_headless_occupied_save_as_preserves_and_pairs() {
        let dir = TempDirectory::new("aw1-g3-headless-occupied");
        let source = dir.path("source.kinewright");
        let occupied = dir.path("occupied.kinewright");
        let recovery = dir.path("recovery");
        std::fs::create_dir(&recovery).expect("the recovery dir creates");
        write_project_document(&Document::default(), &source, None).expect("source writes");
        write_project_document(&Document::default(), &occupied, None).expect("target writes");
        let foreign_digest = digest_bytes(&std::fs::read(&occupied).expect("the target reads"));
        let mut foreign = kinewright_core::IncidentLog::with_start(std::time::Instant::now(), None);
        foreign.observe(IncidentObservation::plain(
            IncidentCode::Label(LabelIncident::Project),
            IncidentSubject::Project,
            "foreign history",
            TimelineRevision::default(),
        ));
        let (records, _) = foreign.records(None, &std::collections::BTreeMap::new());
        let foreign_bytes = build_sidecar_bytes(&records, &[], &foreign_digest, &foreign_digest)
            .expect("the foreign sidecar builds");
        let occupied_sidecar = sidecar_path_for_project(Some(&occupied)).expect("derived");
        std::fs::write(&occupied_sidecar, &foreign_bytes).expect("the foreign stem writes");
        let mut session = loaded_session(&source);
        let mut changed = Document::default();
        changed.markers.push(kinewright_core::Marker {
            id: kinewright_core::MarkerId(7),
            position: kinewright_core::TimeCode::ZERO,
            label: "g3 edit".to_owned(),
            color_token: 0,
        });
        let report = save_headless(
            &changed,
            &occupied,
            None,
            &mut session,
            "",
            TimelineRevision::default(),
            &recovery,
        )
        .expect("occupied Save As succeeds");
        assert!(
            matches!(report.sidecar, Ok(FlushOutcome::Written(_))),
            "the stem pairs, got {:?}",
            report.sidecar
        );
        let SidecarLoad::Current(current) = load_sidecar(&occupied_sidecar) else {
            panic!("the replaced sidecar parses");
        };
        assert_eq!(
            current.project_digest, report.project.digest,
            "paired on the new digest"
        );
        let mut bak = occupied_sidecar.as_os_str().to_owned();
        bak.push(".bak");
        assert_eq!(
            std::fs::read(std::path::PathBuf::from(bak)).expect("the .bak reads"),
            foreign_bytes,
            "the old history sits in the .bak"
        );
        // Reopen immediately: the pair gates clean.
        let digest = digest_bytes(&std::fs::read(&occupied).expect("the target re-reads"));
        let log = std::sync::Arc::new(std::sync::RwLock::new(
            kinewright_core::IncidentLog::with_start(
                std::time::Instant::now(),
                Some(std::time::SystemTime::now()),
            ),
        ));
        let LoadedSidecarSession { session, .. } = SidecarSession::load(
            &SidecarMode::Load {
                project_digest: digest,
            },
            Some(&occupied),
            log,
            TimelineRevision::default(),
            None,
            None,
        );
        let refused = session
            .incidents
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .all()
            .any(|incident| incident.code == IncidentCode::Label(LabelIncident::SidecarRefused));
        assert!(!refused, "no sidecar_refused on reopen");
        let baks = std::fs::read_dir(dir.root())
            .expect("the dir reads")
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().contains(".bak"))
            .count();
        assert_eq!(baks, 1, "exactly the preserve .bak, no reopen refusal");
    }

    /// R6: a headless save refuses an unloaded session fail-closed instead
    /// of flushing without a baseline.
    #[test]
    fn headless_save_refuses_an_unloaded_session() {
        let dir = TempDirectory::new("aw1-headless-unloaded");
        let project = dir.path("edit.kinewright");
        let recovery = dir.path("recovery");
        std::fs::create_dir(&recovery).expect("the recovery dir creates");
        let log = std::sync::Arc::new(std::sync::RwLock::new(
            kinewright_core::IncidentLog::with_start(
                std::time::Instant::now(),
                Some(std::time::SystemTime::now()),
            ),
        ));
        let LoadedSidecarSession { session, .. } = SidecarSession::load(
            &SidecarMode::None,
            Some(&project),
            log,
            TimelineRevision::default(),
            None,
            None,
        );
        assert!(session.established.is_empty(), "no load ran");
        let mut session = session;
        assert!(
            matches!(
                save_headless(
                    &Document::default(),
                    &project,
                    None,
                    &mut session,
                    "",
                    TimelineRevision::default(),
                    &recovery,
                ),
                Err(ProjectSaveError::SessionNotLoaded)
            ),
            "an unloaded headless save refuses"
        );
        assert!(!project.exists(), "a refused save writes nothing");
    }

    /// `IN2B` §2 rule 6, headless: a failed sidecar flush notes once and the
    /// project save carries on — the report holds the IO error.
    #[test]
    fn headless_save_notes_sidecar_failure_and_saves_anyway() {
        let dir = TempDirectory::new("aw1-headless-write-failed");
        let project = dir.path("edit.kinewright");
        write_project_document(&Document::default(), &project, None).expect("project writes");
        let recovery = dir.path("recovery");
        std::fs::create_dir(&recovery).expect("the recovery dir creates");
        let mut session = loaded_session(&project);
        let sidecar = sidecar_path_for_project(Some(&project)).expect("a saved project derives");
        std::fs::create_dir(&sidecar).expect("the sidecar path is occupied");
        let previous = session.saved_digest.clone();
        let report = save_headless(
            &Document::default(),
            &project,
            None,
            &mut session,
            &previous,
            TimelineRevision::default(),
            &recovery,
        )
        .expect("the save succeeds despite the sidecar failure");
        assert!(report.sidecar.is_err(), "the report holds the IO error");
        assert!(project.is_file(), "the project bytes landed");
        let log = session
            .incidents
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(log.open_count(), 1, "exactly one failure note");
        assert_eq!(
            log.all().next().expect("the note landed").code,
            IncidentCode::Label(LabelIncident::SidecarWriteFailed)
        );
    }

    /// F6/H12 port: a project write that fails after the sidecar write
    /// rolls the sidecar back — the prior bytes return, the digest stays,
    /// and neither half litters a temp.
    #[test]
    fn f6_failed_project_write_restores_the_prior_sidecar() {
        let dir = TempDirectory::new("aw1-f6-h12-restore");
        let project = dir.path("edit.kinewright");
        let recovery = dir.path("recovery");
        std::fs::create_dir(&recovery).expect("the recovery dir creates");
        let mut session = loaded_session(&project);
        note(&session, "aw1 f6 record one");
        let previous = session.saved_digest.clone();
        save_headless(
            &Document::default(),
            &project,
            None,
            &mut session,
            &previous,
            TimelineRevision::default(),
            &recovery,
        )
        .expect("the first save lands");
        let sidecar = sidecar_path_for_project(Some(&project)).expect("a saved project derives");
        let before_sidecar = std::fs::read(&sidecar).expect("the sidecar reads");
        let saved_digest = session.saved_digest.clone();
        note(&session, "aw1 f6 record two");
        // A directory takes the project's place, failing temp + rename on
        // both lanes (a rename onto a directory never succeeds).
        let stash = dir.path("edit.kinewright.stashed");
        std::fs::rename(&project, &stash).expect("the project file moves aside");
        std::fs::create_dir(&project).expect("a directory takes its place");
        let previous = session.saved_digest.clone();
        assert!(
            matches!(
                save_headless(
                    &Document::default(),
                    &project,
                    None,
                    &mut session,
                    &previous,
                    TimelineRevision::default(),
                    &recovery,
                ),
                Err(ProjectSaveError::Write(_))
            ),
            "the project write fails"
        );
        assert_eq!(
            std::fs::read(&sidecar).expect("the sidecar re-reads"),
            before_sidecar,
            "the sidecar rolls back to its prior bytes"
        );
        assert_eq!(
            session.saved_digest, saved_digest,
            "the session keeps its old digest"
        );
        assert!(
            session.established.contains(&sidecar),
            "an earned stem survives the rollback"
        );
        for entry in std::fs::read_dir(dir.root()).expect("the dir reads") {
            let entry = entry.expect("a readable entry");
            assert!(
                !entry
                    .path()
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("tmp")),
                "no temp litter: {}",
                entry.file_name().to_string_lossy()
            );
        }
        std::fs::remove_dir(&project).expect("cleanup");
    }

    /// F6/H12 port: when no sidecar bytes preceded the failed save, the
    /// rollback removes the flushed sidecar instead of restoring.
    #[test]
    fn f6_failed_save_as_removes_the_unpaired_sidecar() {
        let dir = TempDirectory::new("aw1-f6-h12-remove");
        let project = dir.path("edit.kinewright");
        let recovery = dir.path("recovery");
        std::fs::create_dir(&recovery).expect("the recovery dir creates");
        let mut session = loaded_session(&project);
        note(&session, "aw1 f6 record one");
        let target = dir.path("target.kinewright");
        std::fs::create_dir(&target).expect("a directory takes the target");
        let sidecar = sidecar_path_for_project(Some(&target)).expect("derived");
        assert!(!sidecar.exists(), "no sidecar precedes the save");
        assert!(
            matches!(
                save_headless(
                    &Document::default(),
                    &target,
                    None,
                    &mut session,
                    "",
                    TimelineRevision::default(),
                    &recovery,
                ),
                Err(ProjectSaveError::Write(_))
            ),
            "the project write fails"
        );
        assert!(
            !sidecar.exists(),
            "the rollback removes the flushed sidecar"
        );
        assert!(
            !session.established.contains(&sidecar),
            "the rollback removes the unearned stem"
        );
        std::fs::remove_dir(&target).expect("cleanup");
    }

    /// F6/J2 port: note, save, note, failed save, close-flush. The failed
    /// save's flush must not advance the confirmed baseline past bytes
    /// that never landed, so the close flush rewrites both records.
    #[test]
    fn f6_failed_save_restores_the_close_flush_baseline() {
        let dir = TempDirectory::new("aw1-f6-j2-baseline");
        let project = dir.path("edit.kinewright");
        let recovery = dir.path("recovery");
        std::fs::create_dir(&recovery).expect("the recovery dir creates");
        let mut session = loaded_session(&project);
        note(&session, "aw1 f6 record one");
        save_headless(
            &Document::default(),
            &project,
            None,
            &mut session,
            "",
            TimelineRevision::default(),
            &recovery,
        )
        .expect("the save lands");
        note(&session, "aw1 f6 record two");
        let stash = dir.path("edit.kinewright.stashed");
        std::fs::rename(&project, &stash).expect("the project file moves aside");
        std::fs::create_dir(&project).expect("a directory takes its place");
        let previous = session.saved_digest.clone();
        assert!(
            matches!(
                save_headless(
                    &Document::default(),
                    &project,
                    None,
                    &mut session,
                    &previous,
                    TimelineRevision::default(),
                    &recovery,
                ),
                Err(ProjectSaveError::Write(_))
            ),
            "the project write fails"
        );
        std::fs::remove_dir(&project).expect("cleanup");
        std::fs::rename(&stash, &project).expect("the project file returns");
        // The close-flush analog: `flush_incidents_if_changed` is what the
        // app's close path calls.
        let outcome = session
            .flush_incidents_if_changed(Some(&project), |_| None)
            .expect("the close flush reports");
        assert!(
            matches!(outcome, FlushOutcome::Written(_)),
            "the close flush rewrites, got {outcome:?}"
        );
        let sidecar = sidecar_path_for_project(Some(&project)).expect("a saved project derives");
        let SidecarLoad::Current(current) = load_sidecar(&sidecar) else {
            panic!("the close left a current sidecar");
        };
        assert_eq!(
            current.records.len(),
            2,
            "the close flush rewrote both records"
        );
    }

    /// G4/RB2: a failed nonempty Save As onto an occupied stem restores the
    /// foreign bytes AND the establishment it never earned — and the
    /// empty-retry after the fix preserves-and-pairs instead of wiping.
    #[allow(clippy::too_many_lines)]
    #[test]
    fn g4_failed_save_as_restores_established_and_retry_preserves() {
        let dir = TempDirectory::new("aw1-g4-save-as-rollback");
        let source = dir.path("source.kinewright");
        let target = dir.path("target.kinewright");
        let recovery = dir.path("recovery");
        std::fs::create_dir(&recovery).expect("the recovery dir creates");
        write_project_document(&Document::default(), &source, None).expect("source writes");
        write_project_document(&Document::default(), &target, None).expect("target writes");
        let foreign_digest = digest_bytes(&std::fs::read(&target).expect("the target reads"));
        let mut foreign = kinewright_core::IncidentLog::with_start(std::time::Instant::now(), None);
        foreign.observe(IncidentObservation::plain(
            IncidentCode::Label(LabelIncident::Project),
            IncidentSubject::Project,
            "foreign history",
            TimelineRevision::default(),
        ));
        let (records, _) = foreign.records(None, &std::collections::BTreeMap::new());
        let foreign_bytes = build_sidecar_bytes(&records, &[], &foreign_digest, &foreign_digest)
            .expect("the foreign sidecar builds");
        let target_sidecar = sidecar_path_for_project(Some(&target)).expect("derived");
        std::fs::write(&target_sidecar, &foreign_bytes).expect("the foreign stem writes");
        let mut session = loaded_session(&source);
        note(&session, "aw1 g4 record one");
        assert!(
            !session.established.contains(&target_sidecar),
            "the target starts unestablished"
        );
        let stash = dir.path("target.kinewright.stashed");
        std::fs::rename(&target, &stash).expect("the target file moves aside");
        std::fs::create_dir(&target).expect("a directory takes its place");
        let mut changed = Document::default();
        changed.markers.push(kinewright_core::Marker {
            id: kinewright_core::MarkerId(7),
            position: kinewright_core::TimeCode::ZERO,
            label: "g4 edit".to_owned(),
            color_token: 0,
        });
        assert!(
            matches!(
                save_headless(
                    &changed,
                    &target,
                    None,
                    &mut session,
                    "",
                    TimelineRevision::default(),
                    &recovery,
                ),
                Err(ProjectSaveError::Write(_))
            ),
            "the project write fails"
        );
        assert_eq!(
            std::fs::read(&target_sidecar).expect("the stem re-reads"),
            foreign_bytes,
            "the stem rolls back to the foreign bytes"
        );
        assert!(
            !session.established.contains(&target_sidecar),
            "the rollback removes the unearned stem"
        );
        let baks = std::fs::read_dir(dir.root())
            .expect("the dir reads")
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().contains(".bak"))
            .count();
        assert_eq!(baks, 0, "no .bak litter from the failed save");
        std::fs::remove_dir(&target).expect("cleanup");
        std::fs::rename(&stash, &target).expect("the target file returns");
        session
            .incidents
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove_open_with_code(IncidentCode::Label(LabelIncident::Project));
        let report = save_headless(
            &changed,
            &target,
            None,
            &mut session,
            "",
            TimelineRevision::default(),
            &recovery,
        )
        .expect("the retry succeeds");
        assert!(
            matches!(report.sidecar, Ok(FlushOutcome::Written(_))),
            "the retry pairs, got {:?}",
            report.sidecar
        );
        let SidecarLoad::Current(current) = load_sidecar(&target_sidecar) else {
            panic!("the retried sidecar parses");
        };
        assert_eq!(
            current.project_digest, report.project.digest,
            "the retry pairs on the new digest"
        );
        let mut bak = target_sidecar.as_os_str().to_owned();
        bak.push(".bak");
        assert_eq!(
            std::fs::read(std::path::PathBuf::from(bak)).expect("the .bak reads"),
            foreign_bytes,
            "the foreign history survives the retry in the .bak"
        );
        assert!(
            session.established.contains(&target_sidecar),
            "the landed retry establishes"
        );
    }

    /// F6/J3 port: only a missing sidecar reads as "no prior sidecar" —
    /// any other read error skips the rollback instead of deleting.
    #[cfg(unix)]
    #[test]
    fn f6_rollback_keeps_an_unreadable_sidecar() {
        use std::os::unix::fs::PermissionsExt as _;

        let dir = TempDirectory::new("aw1-f6-j3-unreadable");
        // Root reads through 000: probe first.
        let probe = dir.path("probe");
        std::fs::write(&probe, b"probe").expect("the probe writes");
        std::fs::set_permissions(&probe, std::fs::Permissions::from_mode(0o000))
            .expect("chmod 000");
        if std::fs::read(&probe).is_ok() {
            std::fs::set_permissions(&probe, std::fs::Permissions::from_mode(0o600))
                .expect("chmod back");
            return;
        }
        std::fs::remove_file(&probe).expect("the probe removes");
        let project = dir.path("edit.kinewright");
        let recovery = dir.path("recovery");
        std::fs::create_dir(&recovery).expect("the recovery dir creates");
        let mut session = loaded_session(&project);
        note(&session, "aw1 f6 record one");
        save_headless(
            &Document::default(),
            &project,
            None,
            &mut session,
            "",
            TimelineRevision::default(),
            &recovery,
        )
        .expect("the save lands");
        let sidecar = sidecar_path_for_project(Some(&project)).expect("derived");
        note(&session, "aw1 f6 record two");
        std::fs::set_permissions(&sidecar, std::fs::Permissions::from_mode(0o000))
            .expect("chmod 000");
        let stash = dir.path("edit.kinewright.stashed");
        std::fs::rename(&project, &stash).expect("the project file moves aside");
        std::fs::create_dir(&project).expect("a directory takes its place");
        let previous = session.saved_digest.clone();
        assert!(
            matches!(
                save_headless(
                    &Document::default(),
                    &project,
                    None,
                    &mut session,
                    &previous,
                    TimelineRevision::default(),
                    &recovery,
                ),
                Err(ProjectSaveError::Write(_))
            ),
            "the project write fails"
        );
        std::fs::remove_dir(&project).expect("cleanup");
        assert!(
            sidecar.exists(),
            "the rollback never deletes a sidecar it could not read"
        );
        std::fs::set_permissions(&sidecar, std::fs::Permissions::from_mode(0o600))
            .expect("chmod back");
    }
}
