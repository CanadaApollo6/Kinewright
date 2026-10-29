//! S3 C7 witnesses: Revise cut reaches S4's scheduler through the real writer
//! host, exactly once across a lost reply and a restart, and Notes shows the run.
use super::*;
use crate::ja1_attachment::Ja1Attachment;
use kinewright_agent::{
    orchestration::orchestration_records::ProviderSelectionRecord,
    writer_host::{DirectorSetting, SessionRoots},
};

/// A saved project with one saved note selected for Revise cut, and a Director
/// configured in a per-test scheduler folder.
struct Project {
    temp: TempDirectory,
    path: std::path::PathBuf,
    feedback: Feedback,
}
fn project(name: &str) -> Project {
    let core = Core::spawn(Document::default()).unwrap();
    let temp = TempDirectory::new_in(&workspace_target_dir(), name);
    let path = temp.path("revise.kinewright");
    let mut bundle = ProjectBundle::activate(&path, BundleWriteAdmission::new_document()).unwrap();
    let inputs = bundle.empty_inputs().unwrap().reference.clone();
    bundle.save(&core, &inputs).unwrap();
    let branch = bundle.manual_branch_id().clone();
    let mut feedback = kept_by(&temp, &bundle);
    let mut note = capture_cut(&core, &branch, 1, 0);
    let comment = note.comment.clone();
    note.text = "C7 tighten the open".into();
    feedback.drafts.push(note);
    publish(&mut bundle);
    feedback.refresh(&bundle).unwrap();
    let saved = saved_ref(feedback.save(&mut bundle, &comment), "C7_SAVE");
    feedback.toggle(&saved);
    let folder = temp.path("scheduler");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(
        folder.join("director.json"),
        serde_json::to_vec(&DirectorSetting {
            version: 1,
            director: ProviderSelectionRecord {
                connector: "scripted".into(),
                model: "fixture".into(),
                options: vec![],
            },
        })
        .unwrap(),
    )
    .unwrap();
    drop(bundle);
    Project {
        temp,
        path,
        feedback,
    }
}
/// The App's owner for `path` with its real writer host, reading this test's
/// scheduler folder.
fn launch(project: &Project) -> Ja1Attachment {
    let owner =
        kinewright_project::editing_session::Ja1EditingSession::open(&project.path).unwrap();
    let mut attachment = Ja1Attachment::new(owner).unwrap();
    crate::ja1_attachment::SCHEDULER_SETTINGS
        .with(|folder| *folder.borrow_mut() = Some(project.temp.path("scheduler")));
    attachment
        .launch_host(
            "c7-revise".into(),
            SessionRoots {
                read: vec![project.temp.root().into()],
                write: vec![project.temp.root().into()],
            },
        )
        .unwrap();
    attachment
}
fn with_host<R>(attachment: &Ja1Attachment, act: impl FnOnce(&mut HostFeedback<'_>) -> R) -> R {
    let client = attachment.host_client().expect("C7_HOST_ATTACHED");
    act(&mut HostFeedback {
        client: &client,
        reader: attachment,
    })
}
fn linked(path: &std::path::Path) -> (Vec<Option<RunId>>, usize) {
    let owner = kinewright_project::editing_session::Ja1EditingSession::open(path).unwrap();
    let batches = owner
        .feedback_index()
        .unwrap()
        .batches
        .values()
        .map(|batch| batch.consumed_by.clone())
        .collect();
    (batches, owner.orchestration_runs().unwrap().len())
}

/// Submit; the `ReviseCut` reply is lost on the wire; the App and its host
/// restart and the kept request retries to one run, one consumption and the
/// same Accepted.
#[test]
fn s3_c7_revise_cut_exactly_once_across_lost_reply_and_restart() {
    let mut project = project("c7-exactly-once");
    let mut attachment = launch(&project);
    // The notes were saved in-process; the host attachment issues its own.
    project.feedback.issuer = None;
    let proxy = attachment.drop_host_reply("revise_cut", false);
    let outcome = with_host(&attachment, |port| project.feedback.submit(port));
    assert!(
        proxy.dropped.load(std::sync::atomic::Ordering::Acquire),
        "C7_REPLY_DROPPED"
    );
    drop(proxy);
    let Outcome::Submitted(batch) = outcome else {
        panic!("C7_SUBMITTED {outcome:?}");
    };
    let kept = project.feedback.revise.clone().expect("C7_REQUEST_KEPT");
    assert_eq!(kept.batch_id, batch);
    let on_disk = std::fs::read_to_string(project.temp.path("state").join(format!(
        "{}.json",
        attachment.studio_project_id().unwrap().as_str()
    )))
    .unwrap();
    assert!(
        on_disk.contains("\"revise\""),
        "C7_REQUEST_KEPT_ON_DISK before the App restarts"
    );
    // The App and its host stop; the kept file is all that carries the request.
    let directory = project.temp.path("state");
    project.feedback = Feedback::default();
    drop(attachment);
    let attachment = launch(&project);
    project
        .feedback
        .open(attachment.studio_project_id().map_err(text), Ok(directory))
        .unwrap();
    assert_eq!(
        project.feedback.revise.as_ref(),
        Some(&kept),
        "C7_SAME_REQUEST"
    );
    assert!(project.feedback.resume, "C7_RESUME");
    with_host(&attachment, |port| project.feedback.resume(port));
    assert!(project.feedback.revise.is_none(), "C7_REQUEST_SETTLED");
    let replay = with_host(&attachment, |port| port.revise(&kept));
    let RunStart::Known(ReviseResult::Accepted { run_id, batch_id }) = &replay else {
        panic!("C7_ACCEPTED {replay:?}");
    };
    assert_eq!(batch_id, &batch);
    let rows = &project.feedback.rows;
    assert_eq!(rows.len(), 1, "C7_ROWS {:?}", project.feedback.message);
    assert_eq!(rows[0].run.as_ref(), Some(run_id), "C7_SAME_ACCEPTED");
    assert!(rows[0].label().contains("Accepted"), "{}", rows[0].label());
    assert!(
        project
            .feedback
            .message
            .as_deref()
            .is_some_and(|message| message.contains("accepted")),
        "{:?}",
        project.feedback.message
    );
    drop(attachment);
    let (batches, runs) = linked(&project.path);
    println!(
        "C7_EXACTLY_ONCE {}",
        serde_json::json!({"runs": runs, "consumed": batches})
    );
    assert_eq!(runs, 1, "C7_ONE_RUN");
    assert_eq!(batches, vec![Some(run_id.clone())], "C7_ONE_CONSUMPTION");
}

/// Revise cut in Studio: the Notes footer's submit reaches the host and Notes
/// shows the batch with its Accepted run, read from `ObserveRun`. Observing
/// never touches the selection or the drafts.
#[test]
fn s3_c7_studio_revise_cut_shows_accepted_run_in_notes() {
    let (mut app, _engine) = in1_harness(Document::default());
    let mut project = project("c7-studio");
    // Evidence only: the screenshot run's `revise` scene opens this project.
    if let Some(to) = std::env::var_os("KINEWRIGHT_C7_SHOT_PROJECT") {
        copy_tree(project.temp.root(), std::path::Path::new(&to));
    }
    let attachment = launch(&project);
    app.projects[0].recovery.stop_for_studio();
    app.projects[0].ja1 = Some(attachment);
    let directory = project.temp.path("state");
    let mut feedback = std::mem::take(&mut project.feedback);
    // The notes were saved in-process; the host attachment issues its own.
    feedback.issuer = None;
    // The project's feedback moves into the App: the same kept file and owner.
    feedback.directory_override = Some(directory);
    app.focused_mut().studio.feedback = feedback;
    app.feedback_frame();
    assert!(
        matches!(app.focused().studio.feedback.keeping, Keeping::Kept { .. }),
        "C7_KEPT"
    );
    let outcome = app
        .with_feedback(|feedback, port| feedback.submit(port))
        .unwrap();
    assert!(matches!(outcome, Outcome::Submitted(_)), "{outcome:?}");
    let feedback = &app.focused().studio.feedback;
    assert_eq!(
        feedback.message.as_deref(),
        Some("Revise cut accepted · the Director run has started."),
        "C7_NOTES_ACCEPTED"
    );
    assert!(feedback.revise.is_none());
    // A saved note is selected again while the run is observed.
    let pinned = feedback.saved[0].reference.clone();
    app.focused_mut().studio.feedback.toggle(&pinned);
    let selection = app.focused().studio.feedback.selection.clone();
    let drafts = app.focused().studio.feedback.drafts.len();
    app.focused_mut().studio.feedback.observed_at = None;
    app.feedback_frame();
    let ctx = egui::Context::default();
    let _ = ctx.run_ui(egui::RawInput::default(), |ui| app.studio_notes(ui));
    let feedback = &app.focused().studio.feedback;
    let row = &feedback.rows[0];
    let label = row.label();
    println!("C7_NOTES_ROW {label}");
    assert!(row.run.is_some(), "C7_LINKED_RUN");
    assert!(
        matches!(&row.state, Some(Ok(_))),
        "C7_OBSERVED {:?}",
        row.state
    );
    assert!(label.contains("Accepted · run"), "C7_NOTES_RUN {label}");
    assert!(!label.contains("Delivered"), "C7_NO_DELIVERED");
    assert_eq!(feedback.selection, selection, "C7_SELECTION_PINNED");
    assert_eq!(feedback.drafts.len(), drafts, "C7_DRAFTS_UNTOUCHED");
    in1_shutdown(&mut app);
}
