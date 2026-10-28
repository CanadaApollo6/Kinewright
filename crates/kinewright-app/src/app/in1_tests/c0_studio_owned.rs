//! C0 Studio-owned colour witnesses.
//!
//! Kept out of `app.rs` because the S10/AU6 guard bans OS gates there. They are
//! Linux-only because durable Studio bundles are Linux-only for now.

#![cfg(target_os = "linux")]

use super::*;

struct StudioOwnedColour {
    _fixture: GeneratedMedia,
    _temp: TempDirectory,
    app: KinewrightApp,
    _engine: Arc<FfmpegMediaEngine>,
    path: std::path::PathBuf,
    saved: kinewright_core::HistorySnapshot,
    observation: IncidentObservation,
}

/// A saved Studio project with the untagged fixture admitted as owned
/// media through a local owner, plus the unknown-colour observation the
/// router would open for that owned asset.
fn studio_owned_colour() -> StudioOwnedColour {
    use crate::ja1_attachment::{Action, Ja1Attachment};
    use kinewright_project::{editing_session::Ja1EditingSession, storage_budget::StorageLimits};
    let probe_engine = FfmpegMediaEngine::new().expect("the test engine starts");
    let (fixture, probed) = in1_untagged(&probe_engine);
    let error = in1_playback_error(&probe_engine, &probed);
    let temp = TempDirectory::new_in(
        &kinewright_media::test_support::workspace_target_dir(),
        "c0-studio-colour",
    );
    let path = temp.path("owned-colour.kinewright");
    let (mut app, engine) = in1_harness(Document {
        tracks: vec![Track {
            id: TrackId(1),
            kind: TrackKind::Video,
            sync_lock: true,
            clips: vec![],
        }],
        ..Document::default()
    });
    let core = app.projects[0].core.clone();
    app.projects[0].recovery.stop_for_studio();
    let mut owner = Ja1EditingSession::new(temp.root(), &core, StorageLimits::default()).unwrap();
    owner.first_save(&path, StorageLimits::default()).unwrap();
    let mut attachment = Ja1Attachment::new(owner).unwrap();
    let Operation::SetAssetColorDescription {
        mut color_description,
        ..
    } = kinewright_core::assume_rec709_operation(
        probed.media_pool[0].id,
        &probed.media_pool[0].color_description,
    )
    else {
        unreachable!()
    };
    color_description.provenance = kinewright_core::ColorProvenance::UserOverride;
    let (owned_asset, _) = attachment
        .owner
        .import_owned_media_with_color(fixture.path(), Some(color_description))
        .unwrap();
    attachment.apply(Action::Save).unwrap();
    let asset = owned_asset.id;
    app.projects[0].ja1 = Some(attachment);
    in1_poll_production_until(&mut app, |app| {
        let project = &app.projects[0];
        project.document.asset(asset).is_some()
            && project.ja1_job.is_none()
            && project.ja1.is_some()
            && project.studio.manual_inputs.is_some()
    });
    let saved = core.export_history().unwrap();
    let mut observation = IncidentObservation::from_media_error(
        &error,
        IncidentSubject::Project,
        app.projects[0].revision,
    );
    observation.subject = IncidentSubject::Asset(asset);
    StudioOwnedColour {
        _fixture: fixture,
        _temp: temp,
        app,
        _engine: engine,
        path,
        saved,
        observation,
    }
}

fn studio_owned_colour_finish(mut fixture: StudioOwnedColour, incident: IncidentId) {
    use crate::ja1_attachment::Action;
    let app = &mut fixture.app;
    assert_eq!(
        in1_incident(app, incident).state,
        IncidentState::Open,
        "the refused colour fix leaves its incident open"
    );
    assert_eq!(app.status, STUDIO_SOURCE_COLOUR_REFUSAL);
    let attachment = app.projects[0].ja1.as_mut().unwrap();
    let saved = attachment.apply(Action::Save);
    assert!(
        saved.is_ok(),
        "the owned document stays saveable: {saved:?} {}",
        attachment.message
    );
    let restored = kinewright_core::Core::spawn(Document::default()).unwrap();
    kinewright_project::bundle::LoadedBundle::read(&fixture.path)
        .unwrap()
        .restore_manual(&restored)
        .unwrap();
    assert_eq!(restored.export_history().unwrap(), fixture.saved);
    assert!(app.projects[0].ja1.as_mut().unwrap().close());
    app.projects[0].ja1 = None;
    in2_cleanup(app);
    drop(fixture);
}

/// Wonder r2 C2 / Opus r2 C2, router half: a Studio-owned asset's
/// automatic colour fix never reaches Core unstaged.
#[test]
fn c0_studio_owned_auto_colour_fix_is_not_sent_unstaged() {
    let mut fixture = studio_owned_colour();
    let app = &mut fixture.app;
    app.pending_observations.push(fixture.observation.clone());
    app.route_incidents();
    let incident = {
        let handle = Arc::clone(&app.projects[0].incidents);
        let log = handle
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let incident = log.all().next().expect("the router opened the incident");
        assert_eq!(incident.class, PolicyClass::AutoApply);
        incident.id
    };
    assert_eq!(
        app.projects[0].core.export_history().unwrap(),
        fixture.saved,
        "c0-colour-auto: the automatic fix must not reach Core unstaged"
    );
    assert!(
        app.pending_router_applies.is_empty(),
        "c0-colour-auto: no router send is outstanding"
    );
    studio_owned_colour_finish(fixture, incident);
}

/// Wonder r2 C2 / Opus r2 C2, card half: pressing the incident card's
/// colour recovery in a Studio project sends nothing unstaged.
#[test]
fn c0_studio_owned_card_colour_fix_is_not_sent_unstaged() {
    let mut fixture = studio_owned_colour();
    let app = &mut fixture.app;
    let (incident, operation) = {
        let handle = Arc::clone(&app.projects[0].incidents);
        let mut log = handle
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Observed::Opened(id) = log.observe(fixture.observation.clone()) else {
            panic!("a fresh log opens the owned asset's incident");
        };
        let incident = log.get(id).expect("open");
        let RecoveryKind::Operation(operation) = &incident.recoveries[0].kind else {
            panic!("the colour recovery is an operation");
        };
        (id, operation.clone())
    };
    app.send_incident_recovery(0, incident, operation, IncidentOutcome::Applied);
    assert_eq!(
        app.projects[0].core.export_history().unwrap(),
        fixture.saved,
        "c0-colour-card: the card's fix must not reach Core unstaged"
    );
    assert!(
        app.pending_router_applies.is_empty(),
        "c0-colour-card: no card send is outstanding"
    );
    studio_owned_colour_finish(fixture, incident);
}
