//! S3 C10: one start-to-finish Studio pass through the real App, writer host,
//! project store and media, host-attached. A moment and a delayed A/B; a drawn
//! note, a voice note and its transcript edit; one batch across two versions;
//! Revise cut on the scripted Director until Accepted, across lost replies;
//! the notes mapped onto an edited copy; close and reopen; then Save As.
use super::{
    State,
    tests::{VOICE_TEST, recorded, send_capture, wait_saved},
};
use crate::{
    app::{
        KinewrightApp,
        in1_tests::{in1_harness, in1_shutdown},
    },
    ja1_attachment::{Action, Ja1Attachment},
    studio::tests::{Fixtures, Retire, publish, wait_frame},
    studio_feedback::{Origin, Outcome, RunStart},
};
use eframe::egui;
use kinewright_agent::writer_host::{
    FeedbackMapping, HostCommand, MapTarget, SessionRoots, director_choices,
};
use kinewright_core::{
    Command, Core, Document, Event, Operation, Playback, TimeCode, Track, TrackId, TrackKind,
};
use kinewright_media::test_support::{SpeechClip, normalized_words, run_ffmpeg};
use kinewright_project::{
    bundle::{BundleError, LoadedBundle},
    bundle_records::ObjectRef,
    editing_session::Ja1EditingSession,
    studio_records::{CommentId, ReviseResult, SnapshotKey, TemporalStatus},
};
use std::{
    collections::BTreeMap,
    sync::Arc,
    time::{Duration, Instant},
};

/// One project with two published versions of the red fixture: A, then B with
/// its clip at double speed. Both share the clip's occurrence.
fn two_versions() -> (Fixtures, Ja1EditingSession, SnapshotKey, SnapshotKey) {
    let root = kinewright_media::test_support::TempDirectory::new_in(
        &kinewright_media::test_support::workspace_target_dir(),
        "c10-e2e",
    );
    let [red, _] = kinewright_project::c0_fixtures::build_c2_fixture_pair(root.root()).unwrap();
    let media = std::fs::read(
        red.path
            .with_extension("kinewright-assets")
            .join("studio/objects")
            .join(&red.media.entry.object.sha256),
    )
    .unwrap();
    let loaded = LoadedBundle::read(&red.path).unwrap();
    let core = Core::spawn(Document::default()).unwrap();
    loaded.restore_manual(&core).unwrap();
    let inputs = loaded
        .manual_checkpoint()
        .unwrap()
        .input_manifest_ref
        .clone();
    let mut bundle = loaded.into_writer().unwrap();
    let alias = root.path("legacy-locator.mkv");
    std::fs::write(&alias, &media).unwrap();
    let asset = core.export_history().unwrap().document.media_pool[0].clone();
    let apply = |operation| {
        assert!(matches!(
            core.request(Command::Do(operation)).unwrap(),
            Event::DocumentChanged { .. }
        ));
    };
    apply(Operation::RelinkAsset {
        asset: asset.id,
        candidate: crate::media_workflow::build_relink_candidate(alias, &asset),
        allow_unverified_source: false,
    });
    let clip = core
        .export_history()
        .unwrap()
        .document
        .tracks
        .iter()
        .find(|track| track.kind == TrackKind::Video)
        .unwrap()
        .clips[0]
        .id;
    bundle.save(&core, &inputs).unwrap();
    let a = publish(&mut bundle);
    apply(Operation::SetClipSpeed {
        clip,
        speed_percent: 200,
    });
    bundle.save(&core, &inputs).unwrap();
    let b = publish(&mut bundle);
    let owner = Ja1EditingSession::from_bundle(bundle, &core, inputs).unwrap();
    (
        Fixtures {
            root,
            owners: Vec::new(),
            contexts: Vec::new(),
        },
        owner,
        a,
        b,
    )
}
fn roots(fixture: &Fixtures) -> SessionRoots {
    SessionRoots {
        read: vec![fixture.root.root().into()],
        write: vec![fixture.root.root().into()],
    }
}
fn launch(fixture: &Fixtures, owner: Ja1EditingSession, name: &str) -> Ja1Attachment {
    let mut attachment = Ja1Attachment::new(owner).unwrap();
    attachment.launch_host(name.into(), roots(fixture)).unwrap();
    assert!(attachment.host_attached(), "E2E_HOST_ATTACHED");
    attachment
}
fn install(app: &mut KinewrightApp, fixture: &Fixtures, attachment: Ja1Attachment) {
    app.focused_mut().ja1 = Some(attachment);
    app.focused_mut().install_ja1_view();
    app.focused_mut().studio.feedback.directory_override = Some(fixture.root.path("kept-feedback"));
}
fn show(app: &mut KinewrightApp, ctx: &egui::Context, key: &SnapshotKey, at: TimeCode) {
    let context = Arc::new(
        app.focused()
            .ja1
            .as_ref()
            .unwrap()
            .studio_context(key)
            .unwrap(),
    );
    app.install_studio_context(context).unwrap();
    app.seek_to(at);
    wait_frame(app, ctx, key, at);
    app.feedback_frame();
}
fn client(app: &KinewrightApp) -> kinewright_agent::writer_host::HostClient {
    app.focused().ja1.as_ref().unwrap().host_client().unwrap()
}
fn saved(app: &mut KinewrightApp, comment: &CommentId, marker: &str) -> ObjectRef {
    let outcome = app
        .with_feedback(|feedback, port| feedback.save(port, comment))
        .unwrap();
    let Outcome::Saved(reference) = outcome else {
        panic!("{marker} {outcome:?}");
    };
    reference
}
/// Every saved note's mapping onto the picture shown now.
fn mapped(app: &mut KinewrightApp) -> BTreeMap<ObjectRef, FeedbackMapping> {
    app.map_studio_notes().unwrap();
    let mapped = app.focused().studio.feedback.mapped.as_ref().unwrap();
    mapped
        .notes
        .iter()
        .map(|(reference, mapping)| {
            (
                reference.clone(),
                mapping
                    .clone()
                    .unwrap_or_else(|why| panic!("E2E_MAPPED {why}")),
            )
        })
        .collect()
}
/// Close as the App does: a host retaining a pending run detaches to keep it,
/// and its reply arrives between frames.
fn close_host(app: &mut KinewrightApp, ctx: &egui::Context) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(attachment) = app.focused_mut().ja1.as_mut()
            && attachment.close()
        {
            return;
        }
        app.poll_background(ctx);
        assert!(
            Instant::now() < deadline,
            "E2E_CLOSE_HOST {:?}",
            app.focused()
                .ja1
                .as_ref()
                .map(|attachment| attachment.message.clone())
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}
/// End one launched host process (this test's own child), if it still runs
/// detached, and wait for it.
fn end_host(pid: u32) {
    let running = || {
        std::fs::read_to_string(format!("/proc/{pid}/stat"))
            .is_ok_and(|stat| !stat.contains(") Z "))
    };
    if !running() {
        return;
    }
    let status = std::process::Command::new("kill")
        .arg(pid.to_string())
        .status()
        .unwrap();
    assert!(status.success(), "E2E_END_HOST");
    let deadline = Instant::now() + Duration::from_secs(10);
    while running() {
        assert!(Instant::now() < deadline, "E2E_END_HOST");
        std::thread::sleep(Duration::from_millis(10));
    }
}
/// A spoken note on the bound picture, saved by the host, its transcript
/// chosen by the person. Returns the comment and the chosen words.
fn spoken_note(app: &mut KinewrightApp, fixture: &Fixtures, live: bool) -> (CommentId, String) {
    let corrected = "Make this chart warmer and keep the title.\nCafé, frame 4.  ";
    if live {
        let speech = SpeechClip::plain(
            "c10-e2e-note",
            "Please make this chart warmer.",
            "KINEWRIGHT_C10_LIVE_AUDIO",
        );
        let wav = fixture.root.path("spoken.wav");
        run_ffmpeg(
            &[
                "-i",
                speech.mp4.to_str().unwrap(),
                "-vn",
                "-ac",
                "1",
                "-ar",
                "16000",
                "-c:a",
                "pcm_s16le",
            ],
            &wav,
        );
        app.begin_spoken_note(crate::recording::FeedbackInput::Wav(wav))
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(240);
        loop {
            app.voice_frame();
            if let State::Saved { status, .. } = &app.focused().studio.voice.state
                && !status.text.is_empty()
            {
                assert!(
                    normalized_words(&status.text)
                        .iter()
                        .any(|word| word == "warmer"),
                    "E2E_LIVE_WHISPER_TEXT {}",
                    status.text
                );
                println!("E2E_LIVE_WHISPER_TEXT {}", status.text);
                break;
            }
            assert!(
                Instant::now() < deadline,
                "E2E_LIVE_DEADLINE {}",
                app.focused().studio.voice.message
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    } else {
        // CI has no Whisper model: the capture is real and the host saves it;
        // the transcript the person chooses is seeded as the CI voice tests do.
        let captured = recorded(app, fixture);
        send_capture(app, captured, false);
        wait_saved(app);
    }
    let voice = &mut app.focused_mut().studio.voice;
    voice.text = corrected.into();
    voice.edited = true;
    app.choose_spoken_transcript().unwrap();
    let comment = app.focused().studio.voice.comment.clone().unwrap();
    (comment, corrected.into())
}

#[allow(clippy::too_many_lines, reason = "one start-to-finish Studio pass")]
fn end_to_end(live: bool) {
    let _serial = VOICE_TEST
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if live {
        let model = kinewright_media::default_data_dir()
            .join("models/whisper")
            .join(kinewright_media::WHISPER_MODEL_NAME);
        assert_eq!(
            kinewright_media::sha256_file(&model).ok().as_deref(),
            Some(kinewright_media::WHISPER_MODEL_SHA256),
            "E2E_LIVE_MODEL_REQUIRED {}",
            model.display()
        );
    }
    let mut retire = Retire::default();
    let (fixture, owner, a, b) = two_versions();
    crate::ja1_attachment::SCHEDULER_SETTINGS
        .with(|folder| *folder.borrow_mut() = Some(fixture.root.path("scheduler")));
    let attachment = launch(&fixture, owner, "c10-e2e");
    let first = attachment.studio_context(&a).unwrap();
    let (mut app, engine) = in1_harness(first.document.clone());
    drop(first);
    retire.arm(&engine);
    app.frames = engine.controls().frames();
    app.media_events = engine.controls().events();
    install(&mut app, &fixture, attachment);
    let ctx = egui::Context::default();
    crate::theme::install(&ctx);
    let path = app
        .focused()
        .ja1
        .as_ref()
        .unwrap()
        .project_path()
        .unwrap()
        .clone();

    // 1. A moment on A, then a delayed A/B: B is selected while A's frame is
    // still the one shown.
    show(&mut app, &ctx, &a, TimeCode(2));
    let moment = app.presenter.bound().unwrap();
    app.presenter.suspend();
    engine.controls().request_frame(TimeCode(2));
    let deadline = Instant::now() + Duration::from_secs(10);
    let completion = loop {
        if let Ok(frame) = app.frames.try_recv() {
            break frame;
        }
        assert!(Instant::now() < deadline, "E2E_PENDING_COMPLETION");
        std::thread::sleep(Duration::from_millis(5));
    };
    app.presenter.collect(completion);
    app.finalize_preview(&ctx);
    assert_eq!(
        app.presenter.bound().unwrap().frame_id,
        moment.frame_id,
        "E2E_PENDING_FENCE"
    );
    let context_b = Arc::new(
        app.focused()
            .ja1
            .as_ref()
            .unwrap()
            .studio_context(&b)
            .unwrap(),
    );
    app.install_studio_context(Arc::clone(&context_b)).unwrap();
    app.finalize_preview(&ctx);
    let stale = app.presenter.bound().unwrap();
    assert!(stale.stale, "E2E_STALE");
    assert_eq!(
        stale.context.as_ref().unwrap().snapshot,
        a,
        "E2E_STALE_IDENTITY"
    );
    assert_eq!(
        app.focused()
            .studio
            .selection
            .as_ref()
            .unwrap()
            .snapshot
            .descriptor
            .key,
        b,
        "E2E_SELECTED_B"
    );

    // 2a. A drawn note while B is still pending: it belongs to the picture
    // shown (A at 0:02), never to the version just selected.
    app.feedback_stroke_start();
    for point in [[0.25, 0.25], [0.5, 0.4], [0.75, 0.6]] {
        assert!(app.feedback_stroke_point(point), "E2E_STROKE_POINT");
    }
    app.feedback_stroke_end();
    let drawn = app.focused().studio.feedback.active.clone().unwrap();
    {
        let draft = app.focused_mut().studio.feedback.draft_mut(&drawn).unwrap();
        draft.text = "C10 tighten this chart".into();
        let Origin::Captured { layers, layer, .. } = &mut draft.origin else {
            panic!("E2E_CAPTURED");
        };
        assert!(
            layers
                .first()
                .is_some_and(|layer| layer.occurrence.is_some()),
            "E2E_OCCURRENCE_CAPTURED {layers:?}"
        );
        *layer = Some(0);
    }
    let drawn_ref = saved(&mut app, &drawn, "E2E_DRAWN_SAVED");
    let drawn_revision = app
        .with_feedback(|_, port| port.revision(&drawn_ref))
        .unwrap()
        .unwrap();
    assert_eq!(
        (&drawn_revision.anchor.snapshot, drawn_revision.anchor.frame),
        (&a, 2),
        "E2E_DRAWN_ON_SHOWN"
    );

    // B arrives, delivered after the switch. Playback's single audio owner is
    // G8's device-lane witness (`s3_bound_ab_gpu_identity`); CI has no device.
    app.seek_to(TimeCode(4));
    wait_frame(&mut app, &ctx, &b, TimeCode(4));
    app.feedback_frame();
    assert_eq!(
        app.presenter.bound().unwrap().context.unwrap().snapshot,
        b,
        "E2E_B_DISPLAYED"
    );

    // 2b. A voice note on B and the person's transcript edit.
    let (spoken, transcript) = spoken_note(&mut app, &fixture, live);
    let spoken_ref = saved(&mut app, &spoken, "E2E_VOICE_SAVED");

    // 3. One batch across both versions. Its first send and its Revise cut
    // each lose their reply; each retry sends the exact same bytes.
    let director = director_choices()
        .into_iter()
        .next()
        .expect("E2E_DIRECTOR_OFFERED");
    app.choose_director(&director);
    for reference in [&drawn_ref, &spoken_ref] {
        app.focused_mut().studio.feedback.toggle(reference);
    }
    // One proxy loses the batch's first reply and its Revise cut's first
    // reply. It stays in the path until the host closes.
    let proxy = app
        .focused_mut()
        .ja1
        .as_mut()
        .unwrap()
        .drop_host_replies(vec!["metadata", "revise_cut"]);
    let lost = app
        .with_feedback(|feedback, port| feedback.submit(port))
        .unwrap();
    assert_eq!(
        *proxy.lost.lock().unwrap(),
        vec!["metadata"],
        "E2E_SUBMIT_REPLY_DROPPED"
    );
    assert!(
        matches!(lost, Outcome::Unknown(_)),
        "E2E_SUBMIT_UNKNOWN {lost:?}"
    );
    let outcome = app
        .with_feedback(|feedback, port| feedback.submit(port))
        .unwrap();
    assert_eq!(
        *proxy.lost.lock().unwrap(),
        vec!["metadata", "revise_cut"],
        "E2E_REVISE_REPLY_DROPPED"
    );
    let Outcome::Submitted(batch) = outcome else {
        panic!("E2E_SUBMITTED {outcome:?}");
    };
    let kept = app
        .focused()
        .studio
        .feedback
        .revise
        .clone()
        .expect("E2E_REVISE_KEPT");
    let read = client(&app)
        .request(HostCommand::ReadSubmittedBatch {
            batch: batch.clone(),
        })
        .unwrap();
    let versions = read
        .submitted_revisions
        .iter()
        .map(|revision| revision.anchor.snapshot.clone())
        .collect::<Vec<_>>();
    assert!(
        versions.len() == 2 && versions.contains(&a) && versions.contains(&b),
        "E2E_MIXED_VERSIONS {versions:?}"
    );
    let voiced = read
        .submitted_revisions
        .iter()
        .find(|revision| revision.voice_ref.is_some())
        .expect("E2E_BATCH_VOICE");
    assert_eq!(voiced.anchor.snapshot, b, "E2E_VOICE_ON_B");
    assert_eq!(
        voiced
            .transcript_revision
            .as_ref()
            .map(|r| r.sha256.clone()),
        Some(kinewright_media::sha256_bytes(transcript.as_bytes())),
        "E2E_TRANSCRIPT_BYTES"
    );

    // 4. Retry Revise cut until Notes shows the run Accepted.
    let outcome = app
        .with_feedback(|feedback, port| feedback.submit(port))
        .unwrap();
    assert_eq!(outcome, Outcome::Submitted(batch.clone()), "E2E_RETRY");
    assert!(
        app.focused().studio.feedback.revise.is_none(),
        "E2E_SETTLED"
    );
    app.focused_mut().studio.feedback.observed_at = None;
    app.feedback_frame();
    let _ = ctx.run_ui(egui::RawInput::default(), |ui| app.studio_notes(ui));
    let row = app.focused().studio.feedback.rows[0].clone();
    let run = row.run.clone().expect("E2E_LINKED_RUN");
    assert!(
        row.label().contains("Accepted"),
        "E2E_ACCEPTED {}",
        row.label()
    );
    let projection = app
        .with_feedback(|_, port| port.observe(&run))
        .unwrap()
        .unwrap();
    assert_eq!(
        projection
            .tasks
            .iter()
            .map(|task| task.brief.provider.clone())
            .collect::<Vec<_>>(),
        vec![director.clone()],
        "E2E_DIRECTOR"
    );

    // 5. The notes on B, then on an edited copy of A.
    let on_b = mapped(&mut app);
    let edit_copy = app
        .focused_mut()
        .ja1
        .as_mut()
        .unwrap()
        .apply(Action::StudioEditCopy(a.clone()));
    edit_copy.unwrap();
    app.focused_mut().install_ja1_view();
    app.sync_studio_selection();
    // The copy opens as the current edit. While the host holds the bundle no
    // host port admits the current edit's playback inputs (finding: C9 leases
    // only frozen versions), so the Studio reports that once and stops asking.
    let deadline = Instant::now() + Duration::from_secs(30);
    while app.focused().ja1.is_none() || app.focused().studio.failure.is_none() {
        app.poll_background(&ctx);
        assert!(
            Instant::now() < deadline,
            "E2E_EDIT_COPY ja1={} job={} failure={:?} status={:?}",
            app.focused().ja1.is_some(),
            app.focused().ja1_job.is_some(),
            app.focused().studio.failure,
            app.status,
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(
        app.focused()
            .studio
            .failure
            .as_deref()
            .is_some_and(|failure| failure.starts_with("Playback input unavailable")),
        "E2E_CURRENT_EDIT_PLAYBACK {:?}",
        app.focused().studio.failure
    );
    for _ in 0..20 {
        app.poll_background(&ctx);
        assert!(app.focused().ja1_job.is_none(), "E2E_NO_RESPAWN");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(app.focused().studio.selection.is_none(), "E2E_CURRENT_EDIT");
    // The person's edit goes through the host's manual gate, as the UI does.
    app.focused()
        .send_manual_command(Command::Do(Operation::AddTrack {
            track: Track {
                id: TrackId(100),
                kind: TrackKind::Audio,
                sync_lock: true,
                clips: vec![],
            },
        }))
        .expect("E2E_COPY_EDITED");
    let deadline = Instant::now() + Duration::from_secs(30);
    while !app
        .focused()
        .document
        .tracks
        .iter()
        .any(|track| track.id == TrackId(100))
    {
        app.poll_background(&ctx);
        assert!(Instant::now() < deadline, "E2E_COPY_EDITED");
        std::thread::sleep(Duration::from_millis(5));
    }
    app.poll_background(&ctx);
    app.focused_mut()
        .ja1
        .as_mut()
        .unwrap()
        .apply(Action::Save)
        .unwrap();
    app.poll_background(&ctx);
    let on_copy = mapped(&mut app);
    assert_eq!(
        app.focused().studio.feedback.mapped.as_ref().unwrap().onto,
        MapTarget::CurrentEdit
    );
    let drawn_on_copy = &on_copy[&drawn_ref];
    assert_eq!(
        drawn_on_copy.resolution.temporal,
        TemporalStatus::Exact,
        "E2E_DRAWN_ON_COPY {drawn_on_copy:?}"
    );
    assert_eq!(
        drawn_on_copy.candidates.first().map(|(_, frame)| *frame),
        Some(2),
        "E2E_DRAWN_ON_COPY_FRAME"
    );
    let drawn_on_b = &on_b[&drawn_ref];
    assert!(
        matches!(
            drawn_on_b.resolution.temporal,
            TemporalStatus::Exact | TemporalStatus::Mapped
        ) && !drawn_on_b.candidates.is_empty(),
        "E2E_DRAWN_ON_B {drawn_on_b:?}"
    );
    // The spoken note names no clip, so it stays unresolved rather than guessed.
    assert_eq!(
        on_copy[&spoken_ref].resolution.temporal,
        TemporalStatus::Unresolved,
        "E2E_UNTARGETED_UNRESOLVED"
    );
    let _ = ctx.run_ui(egui::RawInput::default(), |ui| app.studio_notes(ui));
    assert!(
        app.focused().studio.feedback.mapped.is_some(),
        "E2E_MAPPING_SHOWN"
    );
    let labels = on_copy
        .values()
        .map(|mapping| {
            crate::studio_feedback::mapping_label(&Ok(mapping.clone()), context_b.document.fps)
        })
        .collect::<Vec<_>>();

    // 6. Close and reopen the App and its host: the same versions, notes,
    // batch, run and recomputed mappings.
    let versions_before = client(&app)
        .request(HostCommand::ListVersions)
        .unwrap()
        .versions;
    let notes = |app: &KinewrightApp| {
        app.focused()
            .studio
            .feedback
            .saved
            .iter()
            .map(|note| {
                (
                    note.reference.clone(),
                    note.text.clone(),
                    note.revision.strokes_ref.clone(),
                    note.revision.voice_ref.clone(),
                )
            })
            .collect::<Vec<_>>()
    };
    app.with_feedback(|feedback, port| feedback.refresh(&*port))
        .unwrap()
        .unwrap();
    let notes_before = notes(&app);
    app.focused_mut().studio.feedback.persist().unwrap();
    // Close keeps the pending run: the host detaches and goes on holding the
    // project, so it cannot be reopened while that run continues.
    let host = app.focused().ja1.as_ref().unwrap().host_pid().unwrap();
    close_host(&mut app, &ctx);
    assert!(
        matches!(
            Ja1EditingSession::open(&path),
            Err(BundleError::Contention(_))
        ),
        "E2E_LIVE_ELSEWHERE"
    );
    drop(app.focused_mut().ja1.take());
    drop(proxy);
    // No connector is certified, so the run never ends by itself. Ending the
    // detached host stands in for that: only durable state comes back.
    end_host(host);
    let (replacement, _) = crate::app::in1_tests::in1_harness_with_engine(
        context_b.document.clone(),
        Arc::clone(&engine),
    );
    let mut old = std::mem::replace(&mut app, replacement);
    in1_shutdown(&mut old);
    drop(old);
    drop(context_b);
    app.frames = engine.controls().frames();
    app.media_events = engine.controls().events();
    let owner = Ja1EditingSession::open(&path).unwrap();
    install(
        &mut app,
        &fixture,
        launch(&fixture, owner, "c10-e2e-reopened"),
    );
    show(&mut app, &ctx, &b, TimeCode(4));
    let _ = ctx.run_ui(egui::RawInput::default(), |ui| app.studio_notes(ui));
    app.with_feedback(|feedback, port| feedback.refresh(&*port))
        .unwrap()
        .unwrap();
    let reopened_versions = client(&app)
        .request(HostCommand::ListVersions)
        .unwrap()
        .versions;
    assert_eq!(reopened_versions, versions_before, "E2E_REOPEN_VERSIONS");
    assert_eq!(notes(&app), notes_before, "E2E_REOPEN_NOTES");
    let rows = app.focused().studio.feedback.rows.clone();
    assert_eq!(rows.len(), 1, "E2E_REOPEN_BATCH");
    assert_eq!(rows[0].batch, batch, "E2E_REOPEN_BATCH");
    assert_eq!(rows[0].run.as_ref(), Some(&run), "E2E_REOPEN_RUN");
    assert_eq!(
        mapped(&mut app),
        on_b,
        "E2E_REOPEN_MAPPINGS recomputed, never stored"
    );
    let replay = app.with_feedback(|_, port| port.revise(&kept)).unwrap();
    let RunStart::Known(ReviseResult::Accepted { run_id, .. }) = &replay else {
        panic!("E2E_REPLAY {replay:?}");
    };
    assert_eq!(run_id, &run, "E2E_REPLAY_SAME_RUN");

    // C1: Save As makes a project of its own. Its notes arrive with it, and a
    // draft typed after Save As never reaches A's recovery file.
    let project_a = app
        .focused()
        .studio
        .feedback
        .keeping
        .project()
        .cloned()
        .unwrap();
    let recovery_a = fixture
        .root
        .path("kept-feedback")
        .join(format!("{}.json", project_a.as_str()));
    let copy = fixture.root.path("save-as.kinewright");
    app.focused_mut()
        .ja1
        .as_mut()
        .unwrap()
        .apply(Action::SaveAs(copy.clone()))
        .unwrap();
    app.feedback_frame();
    let project_b = app
        .focused()
        .studio
        .feedback
        .keeping
        .project()
        .cloned()
        .unwrap();
    assert_ne!(project_a, project_b, "E2E_SAVE_AS_IDENTITY");
    let a_before = std::fs::read(&recovery_a).ok();
    let typed = app.new_feedback_note().unwrap();
    app.focused_mut()
        .studio
        .feedback
        .draft_mut(&typed)
        .unwrap()
        .text = "typed after Save As".into();
    app.focused_mut().studio.feedback.changed = true;
    app.feedback_frame();
    app.with_feedback(|feedback, port| feedback.refresh(&*port))
        .unwrap()
        .unwrap();
    assert_eq!(notes(&app), notes_before, "E2E_SAVE_AS_NOTES");
    assert_eq!(
        std::fs::read(&recovery_a).ok(),
        a_before,
        "E2E_SAVE_AS_A_UNTOUCHED"
    );
    let host = app.focused().ja1.as_ref().unwrap().host_pid().unwrap();
    close_host(&mut app, &ctx);
    end_host(host);
    in1_shutdown(&mut app);
    drop(app);
    let runs = Ja1EditingSession::open(&path)
        .unwrap()
        .orchestration_runs()
        .unwrap()
        .len();
    assert_eq!(runs, 1, "E2E_ONE_RUN");
    println!(
        "S3_E2E {}",
        serde_json::json!({
            "live_voice": live,
            "displayed": {"version": a, "frame": 2, "stale_while_b_selected": true},
            "batch_versions": versions.len(),
            "transcript_sha256": kinewright_media::sha256_bytes(transcript.as_bytes()),
            "runs": runs,
            "mappings": {
                "drawn_on_b": format!("{:?}", drawn_on_b.resolution.temporal),
                "drawn_on_copy": format!("{:?}", drawn_on_copy.resolution.temporal),
                "spoken_on_copy": format!("{:?}", on_copy[&spoken_ref].resolution.temporal),
                "labels": labels,
            },
            "reopen_equal": true,
            "save_as": {"a": project_a, "b": project_b},
        })
    );
    drop(engine);
    retire.wait();
}

/// S3's start-to-finish Studio pass (G11's App leg), with the transcript the
/// person chooses seeded as the CI voice tests do.
#[test]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn s3_studio_end_to_end() {
    end_to_end(false);
}

/// The same pass with the voice note spoken by espeak-ng and transcribed by the
/// pinned Whisper small model.
#[test]
#[ignore = "needs the Whisper model and espeak-ng"]
fn s3_studio_end_to_end_live_voice() {
    end_to_end(true);
}
