//! S3 C10: one start-to-finish Studio pass through the real App, writer host,
//! project store and media, host-attached. A moment and a delayed A/B; a drawn
//! note, a voice note and its transcript edit; one batch across two versions;
//! Revise cut on the scripted Director until Accepted, across lost replies;
//! the notes mapped onto an edited copy; close and reopen; then Save As.
//! S3 C11 adds Current-edit playback while the host holds the project.
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
use kinewright_agent::writer_host::{FeedbackMapping, HostCommand, MapTarget, SessionRoots};
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
/// The Current edit's picture at `at`: bound, current and painted. A
/// playback failure fails `marker` at once, unless it is temporary and the
/// Studio will ask again (S3 C11, fu1).
fn current_frame(
    app: &mut KinewrightApp,
    ctx: &egui::Context,
    at: TimeCode,
    marker: &str,
) -> kinewright_core::FrameTexture {
    let mut retried = false;
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        app.poll_background(ctx);
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            app.finalize_preview(ui.ctx());
        });
        let studio = &app.focused().studio;
        if studio.failure.is_some() && studio.manual_retry_pending() && !retried {
            retried = true;
            eprintln!(
                "{marker}: temporary failure, retrying: {:?}",
                studio.failure
            );
        }
        assert!(
            studio.failure.is_none() || studio.manual_retry_pending(),
            "{marker} {:?}",
            studio.failure
        );
        if studio.selection.is_none()
            && studio.manual_inputs.is_some()
            && let Some(cell) = app.presenter.bound()
            && !cell.stale
            && cell.at == at
            && cell.context.is_none()
            && app
                .presenter
                .context_epoch()
                .is_some_and(|epoch| cell.stamp.epoch >= epoch)
            && studio.cpu_frame_id == Some(cell.frame_id)
        {
            return studio.cpu_frame.clone().expect("bound picture");
        }
        assert!(
            Instant::now() < deadline,
            "{marker} inputs={} job={} bound={:?}",
            studio.manual_inputs.is_some(),
            app.focused().ja1_job.is_some(),
            app.presenter.bound().map(|cell| (cell.at, cell.stale)),
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}
/// The playback scope, once no job holds the attachment.
fn scope(app: &mut KinewrightApp, ctx: &egui::Context) -> (u64, ObjectRef) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        app.poll_background(ctx);
        if let Some(attachment) = app.focused().ja1.as_ref() {
            return attachment.studio_playback_scope().unwrap();
        }
        assert!(Instant::now() < deadline, "C11_SCOPE");
        std::thread::sleep(Duration::from_millis(5));
    }
}
/// Wait until the App's document satisfies `done`, after a manual command.
fn wait_document(
    app: &mut KinewrightApp,
    ctx: &egui::Context,
    marker: &str,
    done: impl Fn(&Document) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !done(&app.focused().document) {
        app.poll_background(ctx);
        assert!(Instant::now() < deadline, "{marker}");
        std::thread::sleep(Duration::from_millis(5));
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
    let director = kinewright_agent::orchestration::adapters::fixture_connectors()[0].selection();
    kinewright_agent::orchestration::director_setting::write_fixture_director_setting(
        &fixture.root.path("scheduler"),
    )
    .unwrap();
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
    // The copy opens as the current edit, and it plays through the host's
    // current-edit lease (S3 C11).
    let copy_at = app.focused().position;
    let copy_frame = current_frame(&mut app, &ctx, copy_at, "E2E_CURRENT_EDIT_PLAYBACK");
    assert!(
        copy_frame.rgba[0] > 128 && copy_frame.rgba[2] < 64,
        "E2E_CURRENT_EDIT_PICTURE {:?}",
        &copy_frame.rgba[..4]
    );
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
    wait_document(&mut app, &ctx, "E2E_COPY_EDITED", |document| {
        document.tracks.iter().any(|track| track.id == TrackId(100))
    });
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
    // A clip move and a new checkpoint, with the inputs unchanged: the shown
    // current-edit mapping follows without Refresh (S3 C11 fu1).
    let inputs_before = scope(&mut app, &ctx);
    let (drawn_clip, _) = drawn_on_copy.candidates[0];
    let (track, start) = app
        .focused()
        .document
        .tracks
        .iter()
        .find_map(|track| {
            track
                .clips
                .iter()
                .find(|clip| clip.id == drawn_clip)
                .map(|clip| (track.id, clip.timeline_start))
        })
        .unwrap();
    app.focused()
        .send_manual_command(Command::Do(Operation::MoveClip {
            clip: drawn_clip,
            to_track: track,
            to: TimeCode(start.0 + 5),
        }))
        .expect("E2E_CLIP_MOVED");
    wait_document(&mut app, &ctx, "E2E_CLIP_MOVED", |document| {
        document
            .clip(drawn_clip)
            .is_some_and(|clip| clip.timeline_start == TimeCode(start.0 + 5))
    });
    app.poll_background(&ctx);
    app.focused_mut()
        .ja1
        .as_mut()
        .unwrap()
        .apply(Action::Save)
        .unwrap();
    app.poll_background(&ctx);
    assert_eq!(
        scope(&mut app, &ctx),
        inputs_before,
        "E2E_MOVE_KEEPS_INPUTS"
    );
    let _ = ctx.run_ui(egui::RawInput::default(), |ui| app.studio_notes(ui));
    let followed = app.focused().studio.feedback.mapped.as_ref().unwrap().notes[&drawn_ref]
        .clone()
        .unwrap();
    assert_eq!(
        followed.candidates.first().map(|(_, frame)| *frame),
        drawn_on_copy.candidates.first().map(|(_, frame)| frame + 5),
        "E2E_MAPPING_FOLLOWS_CHECKPOINT {followed:?}"
    );

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

/// S3 C11: while the host holds a saved project, the Current edit plays from
/// launch, after an edit and after Save As, and Close ends its lease.
/// S3 C11 fu1: another editor imports between the App's last status and its
/// lease request. The host refuses the stale scope; the newer status lifts that
/// failure at once, and the new frame plays with no action from the person.
#[test]
fn s3_c11_fu1_newer_scope_lifts_a_stale_acquisition() {
    let mut retire = Retire::default();
    let (fixture, owner, _a, _b) = two_versions();
    let attachment = launch(&fixture, owner, "c11-fu1");
    let (mut app, engine) = in1_harness(Document::default());
    retire.arm(&engine);
    app.frames = engine.controls().frames();
    app.media_events = engine.controls().events();
    install(&mut app, &fixture, attachment);
    let ctx = egui::Context::default();
    crate::theme::install(&ctx);
    let at = app.focused().position;
    app.sync_studio_selection();
    current_frame(&mut app, &ctx, at, "C11FU1_LAUNCH_PLAYS");
    let stale = scope(&mut app, &ctx);

    let wav = fixture.root.path("c11-fu1.wav");
    std::fs::write(
        &wav,
        kinewright_media::test_support::wav_f32(&vec![0.25; 4800], 48_000, 1),
    )
    .unwrap();
    let reply = app.focused().ja1.as_ref().unwrap().import_elsewhere(&wav);
    assert!(reply.error.is_none(), "C11FU1_IMPORT {:?}", reply.error);
    // The App asks for its current edit again before any newer status. The
    // job holds the attachment, so no heartbeat lands until it returns.
    app.focused_mut().studio.manual_inputs = None;
    app.sync_studio_selection();
    assert!(app.focused().ja1_job.is_some(), "C11FU1_STALE_ATTEMPT");
    let deadline = Instant::now() + Duration::from_secs(30);
    while app.focused().ja1_job.is_some() || app.focused().ja1.is_none() {
        app.poll_background(&ctx);
        assert!(Instant::now() < deadline, "C11FU1_STALE_ATTEMPT");
        std::thread::sleep(Duration::from_millis(5));
    }
    let failed = app.focused().studio.manual_failure.as_ref();
    assert_eq!(
        failed.map(|failed| failed.scope.clone()),
        Some(Some(stale.clone())),
        "C11FU1_STALE_REFUSED {:?}",
        app.focused().studio.failure
    );
    assert!(
        app.focused().studio.failure.is_some(),
        "C11FU1_STALE_REFUSED"
    );

    // The next status names the newer scope. In that same pass the failure for
    // the stale scope is gone and a new acquisition starts.
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        app.poll_background(&ctx);
        if app.focused().ja1.as_ref().is_some_and(|attachment| {
            attachment.studio_playback_scope().ok() != Some(stale.clone())
        }) {
            break;
        }
        assert!(Instant::now() < deadline, "C11FU1_NEWER_STATUS");
        std::thread::sleep(Duration::from_millis(5));
    }
    app.sync_studio_selection();
    assert!(
        app.focused()
            .studio
            .manual_failure
            .as_ref()
            .is_none_or(|failed| failed.scope.as_ref() != Some(&stale)),
        "C11FU1_NEWER_SCOPE_LIFTS {:?}",
        app.focused().studio.manual_failure
    );
    current_frame(&mut app, &ctx, at, "C11FU1_NEW_SCOPE_PLAYS");
    assert_ne!(scope(&mut app, &ctx), stale, "C11FU1_NEW_SCOPE_PLAYS");

    let host = app.focused().ja1.as_ref().unwrap().host_pid().unwrap();
    close_host(&mut app, &ctx);
    app.focused_mut().ja1 = None;
    end_host(host);
    in1_shutdown(&mut app);
    drop(app);
    drop(engine);
    retire.wait();
}

#[test]
fn s3_c11_current_edit_plays_host_attached() {
    let mut retire = Retire::default();
    let (fixture, owner, _a, _b) = two_versions();
    let attachment = launch(&fixture, owner, "c11-current");
    let (mut app, engine) = in1_harness(Document::default());
    retire.arm(&engine);
    app.frames = engine.controls().frames();
    app.media_events = engine.controls().events();
    install(&mut app, &fixture, attachment);
    let ctx = egui::Context::default();
    crate::theme::install(&ctx);
    let at = app.focused().position;

    // Straight after launch, no copy made.
    app.sync_studio_selection();
    let launched = current_frame(&mut app, &ctx, at, "C11_LAUNCH_PLAYS");
    assert!(
        launched.rgba[0] > 128 && launched.rgba[2] < 64,
        "C11_LAUNCH_PICTURE {:?}",
        &launched.rgba[..4]
    );
    let launch_scope = scope(&mut app, &ctx);

    // The person's edit moves the head: the next picture is the new one, from
    // the same lease.
    let clip = app
        .focused()
        .document
        .tracks
        .iter()
        .find(|track| track.kind == TrackKind::Video)
        .unwrap()
        .clips[0]
        .id;
    app.focused()
        .send_manual_command(Command::Do(Operation::SetClipEnabled {
            clip,
            enabled: false,
        }))
        .expect("C11_EDIT");
    wait_document(&mut app, &ctx, "C11_EDIT", |document| {
        document.clip(clip).is_some_and(|clip| !clip.enabled)
    });
    let edited = current_frame(&mut app, &ctx, at, "C11_EDIT_PLAYS");
    assert!(
        edited.rgba[0] < 32,
        "C11_EDIT_SHOWS_NEW_FRAME {:?}",
        &edited.rgba[..4]
    );
    assert_eq!(scope(&mut app, &ctx), launch_scope, "C11_EDIT_KEEPS_LEASE");

    // Save As moves the owner: the App renews its lease and still plays.
    let b = fixture.root.path("c11-b.kinewright");
    app.focused_mut()
        .ja1
        .as_mut()
        .unwrap()
        .apply(Action::SaveAs(b.clone()))
        .unwrap();
    assert_ne!(scope(&mut app, &ctx), launch_scope, "C11_SAVE_AS_RENEWS");
    let after_save_as = current_frame(&mut app, &ctx, at, "C11_SAVE_AS_PLAYS");
    assert!(after_save_as.rgba[0] < 32, "C11_SAVE_AS_PICTURE");
    assert_eq!(
        app.focused().ja1.as_ref().unwrap().project_path(),
        Some(b.clone())
    );

    // Close ends the lease: inputs kept past Close cannot read.
    let kept = app.focused().studio.manual_inputs.clone().unwrap();
    kept.with_document(Arc::new(kept.document().clone()))
        .expect("C11_LIVE_INPUTS_READ");
    let host = app.focused().ja1.as_ref().unwrap().host_pid().unwrap();
    close_host(&mut app, &ctx);
    assert!(
        kept.with_document(Arc::new(kept.document().clone()))
            .is_err(),
        "C11_CLOSE_STOPS_READS"
    );
    drop(kept);
    app.focused_mut().ja1 = None;
    end_host(host);
    in1_shutdown(&mut app);
    drop(app);
    drop(engine);
    retire.wait();
}
