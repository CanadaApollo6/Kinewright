//! Headless save orchestration (AW1 §2): serialize → sidecar flush →
//! project bytes → journal retire. The caller gates newer-format overwrite
//! first, as the app's save prologue does.

use std::path::Path;

use kinewright_core::{Document, TimelineRevision};
use kinewright_media::LutStore;

use crate::{
    project::{ProjectSaveError, ProjectSaveReport, serialize_project_document},
    recovery::retire_journal_for_project,
    session::SidecarSession,
    sidecar::{FlushOutcome, digest_bytes, sidecar_write_failed_observation},
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
    if !sidecar.loaded {
        return Err(ProjectSaveError::SessionNotLoaded);
    }
    let json = serialize_project_document(document)?;
    let new_digest = digest_bytes(json.as_bytes());
    let sidecar_outcome =
        match sidecar.flush_incidents(Some(path), &new_digest, previous_digest, None) {
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
    let project = crate::project::write_project_bytes(&json, document, path, previous_store)?;
    new_digest.clone_into(&mut sidecar.saved_digest);
    let _ = retire_journal_for_project(recovery_dir, path);
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
        project::{load_document, write_project_document},
        recovery::{journal_file_name, pending_journal_for_project},
        session::LoadedSidecarSession,
        sidecar::{SidecarLoad, SidecarMode, digest_bytes, load_sidecar, sidecar_path_for_project},
    };

    use super::*;

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
        assert!(session.loaded, "the headless open loads");
        session
    }

    /// R1/R2/R5/R6: a headless save round-trips — the project re-loads
    /// identical, the sidecar pairs on the new digest, the baseline advances,
    /// and the stale journal retires.
    #[test]
    fn headless_save_round_trip_pairs_and_retires() {
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
            pending_journal_for_project(&recovery, &project).is_none(),
            "the stale journal retired"
        );
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
        assert!(!session.loaded, "no load ran");
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
}
