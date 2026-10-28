#![allow(
    clippy::too_many_lines,
    reason = "end-to-end witnesses retain one complete transition and its oracles"
)]
//! S3 C3 witnesses (G9): the displayed packet and its letterbox geometry, and
//! immutable batches across lost replies and a restart.
use super::*;
use crate::{
    app::in1_tests::{in1_harness, in1_shutdown},
    presenter::DisplayCell,
    studio::tests::{Retire, attach, fixtures, publish, wait_frame},
};
use kinewright_core::{Command, Core, Event, FrameStamp, Operation, Playback, Track, TrackId};
use kinewright_media::test_support::{TempDirectory, workspace_target_dir};
use kinewright_project::{
    bundle::{BundleWriteAdmission, LoadedBundle},
    studio_store::FrozenContext,
};

/// Independent landmark oracle: integer pixel indices through an explicit
/// clockwise rotation, then the letterboxed scale and offset, back to screen points.
fn landmark(
    area: Rect,
    [width, height]: [u32; 2],
    orientation: &Orientation,
    [px, py]: [u32; 2],
) -> (Pos2, [f32; 2]) {
    let (shown, [dx, dy]) = match orientation {
        Orientation::Upright => ([width, height], [px, py]),
        Orientation::Rotate90 => ([height, width], [height - 1 - py, px]),
        Orientation::Rotate180 => ([width, height], [width - 1 - px, height - 1 - py]),
        Orientation::Rotate270 => ([height, width], [py, width - 1 - px]),
    };
    let [sw, sh] = shown.map(f64::from);
    let scale = (f64::from(area.width()) / sw).min(f64::from(area.height()) / sh);
    let left = f64::from(area.left()) + (f64::from(area.width()) - sw * scale) / 2.0;
    let top = f64::from(area.top()) + (f64::from(area.height()) - sh * scale) / 2.0;
    #[allow(clippy::cast_possible_truncation, reason = "screen points")]
    let at = Pos2::new(
        (left + (f64::from(dx) + 0.5) * scale) as f32,
        (top + (f64::from(dy) + 0.5) * scale) as f32,
    );
    #[allow(clippy::cast_possible_truncation, reason = "normalised raster points")]
    let raster = [
        ((f64::from(px) + 0.5) / f64::from(width)) as f32,
        ((f64::from(py) + 0.5) / f64::from(height)) as f32,
    ];
    (at, raster)
}

fn check_letterbox() {
    let orientations = [
        Orientation::Upright,
        Orientation::Rotate90,
        Orientation::Rotate180,
        Orientation::Rotate270,
    ];
    // A wide and a portrait image, in a 4:3 area and the same area at 2x scale.
    for area in [
        Rect::from_min_size(Pos2::new(10.0, 20.0), Vec2::new(800.0, 600.0)),
        Rect::from_min_size(Pos2::new(20.0, 40.0), Vec2::new(1600.0, 1200.0)),
    ] {
        for texture in [[1920, 800], [720, 1280], [64, 36]] {
            for orientation in &orientations {
                let image = DisplayedImage::fit(area, texture, orientation.clone());
                let [w, h] = texture;
                for pixel in [
                    [0, 0],
                    [w - 1, h - 1],
                    [w / 3, h / 5],
                    [w / 2, h / 2],
                    [w - 1, 0],
                ] {
                    let (at, expected) = landmark(area, texture, orientation, pixel);
                    let got = image
                        .normalise(at)
                        .unwrap_or_else(|| panic!("C3_LANDMARK_INSIDE {texture:?} {pixel:?}"));
                    for axis in 0..2 {
                        assert!(
                            (got[axis] - expected[axis]).abs() < 1e-4,
                            "C3_LANDMARK_GEOMETRY {area:?} {texture:?} {orientation:?} {pixel:?}: {got:?} vs {expected:?}"
                        );
                    }
                    let back = image.screen(got);
                    assert!(
                        (back - at).length() < 0.01,
                        "C3_LANDMARK_ROUNDTRIP {back:?} vs {at:?}"
                    );
                }
                // The letterbox and pillarbox bars are outside the picture.
                let bars = [
                    Pos2::new(area.left() + 1.0, area.center().y),
                    Pos2::new(area.center().x, area.top() + 1.0),
                ];
                for bar in bars {
                    let outside = !image.rect.contains(bar);
                    assert_eq!(
                        image.normalise(bar).is_none(),
                        outside,
                        "C3_LETTERBOX_BAR {texture:?} {orientation:?} {bar:?}"
                    );
                }
            }
        }
    }
}

fn add_track(core: &Core, id: u64) {
    assert!(matches!(
        core.request(Command::Do(Operation::AddTrack {
            track: Track {
                id: TrackId(id),
                kind: TrackKind::Audio,
                sync_lock: true,
                clips: vec![]
            }
        }))
        .unwrap(),
        Event::DocumentChanged { .. }
    ));
}
fn manual_cell(frame_id: u64, at: i64) -> DisplayCell {
    DisplayCell {
        stamp: FrameStamp { epoch: 9, seq: 1 },
        at: TimeCode(at),
        context: None,
        frame_id,
        stale: false,
    }
}
/// A draft on the current edit, captured through the same `capture` the app uses.
fn capture_cut(core: &Core, branch: &BranchId, frame_id: u64, at: i64) -> Draft {
    let history = core.export_history().unwrap();
    let cut = AcceptedCut::of(branch, &history).unwrap();
    capture(&View {
        cell: &manual_cell(frame_id, at),
        pixels: None,
        selection: None,
        requested: TimeCode(0),
        manual: Some((cut, &history.document)),
        texture: [16, 9],
    })
    .unwrap()
}
fn new_project(
    name: &str,
) -> (
    TempDirectory,
    std::path::PathBuf,
    ProjectBundle,
    Core,
    ObjectRef,
) {
    let temp = TempDirectory::new_in(&workspace_target_dir(), name);
    let path = temp.path("feedback.kinewright");
    let mut bundle = ProjectBundle::activate(&path, BundleWriteAdmission::new_document()).unwrap();
    let core = Core::spawn(Document::default()).unwrap();
    let inputs = bundle.empty_inputs().unwrap().reference.clone();
    bundle.save(&core, &inputs).unwrap();
    (temp, path, bundle, core, inputs)
}

/// Feedback kept for `bundle`'s project in a per-test state folder.
fn kept_by(temp: &TempDirectory, bundle: &ProjectBundle) -> Feedback {
    let mut feedback = Feedback::default();
    feedback
        .open(Ok(bundle.project_id().clone()), Ok(temp.path("state")))
        .unwrap();
    feedback
}
fn kept_file(temp: &TempDirectory, bundle: &ProjectBundle) -> std::path::PathBuf {
    temp.path("state")
        .join(format!("{}.json", bundle.project_id().as_str()))
}

fn check_manual_cut() {
    let (temp, _path, mut bundle, core, inputs) = new_project("c3-manual-cut");
    let mut feedback = kept_by(&temp, &bundle);
    // No established edit: nothing to anchor, so no draft.
    assert!(
        capture(&View {
            cell: &manual_cell(1, 0),
            pixels: None,
            selection: None,
            requested: TimeCode(0),
            manual: None,
            texture: [16, 9],
        })
        .is_err(),
        "C3_UNESTABLISHED_CUT"
    );
    let branch = bundle.manual_branch_id().clone();
    add_track(&core, 50);
    bundle.save(&core, &inputs).unwrap();
    let x = capture_cut(&core, &branch, 1, 2);
    let x_id = x.comment.clone();
    feedback.drafts.push(x);
    add_track(&core, 51);
    bundle.save(&core, &inputs).unwrap();
    let y = capture_cut(&core, &branch, 2, 3);
    let y_id = y.comment.clone();
    feedback.drafts.push(y);
    let newer = publish(&mut bundle);
    feedback.refresh(&bundle).unwrap();
    let snapshot_of = |feedback: &Feedback, id: &CommentId| match &feedback
        .drafts
        .iter()
        .find(|draft| &draft.comment == id)
        .unwrap()
        .origin
    {
        Origin::Captured { snapshot, .. } => snapshot.clone(),
        Origin::Revises { .. } => unreachable!(),
    };
    assert_eq!(
        snapshot_of(&feedback, &y_id),
        Some(newer.clone()),
        "C3_OWN_CUT_ACK"
    );
    assert_eq!(
        snapshot_of(&feedback, &x_id),
        None,
        "C3_NEWER_HEAD_NEVER_PROMOTES"
    );
    feedback
        .drafts
        .iter_mut()
        .for_each(|draft| draft.text = "tighten".into());
    let waiting = feedback.save(&mut bundle, &x_id);
    assert!(
        matches!(waiting, Outcome::Waiting(_)),
        "C3_UNACKED_STAYS_DRAFT {waiting:?}"
    );
    assert!(
        feedback.drafts.iter().any(|draft| draft.comment == x_id)
            && bundle.feedback_index().unwrap().revisions.is_empty(),
        "C3_UNACKED_STAYS_DRAFT"
    );
    // Undo shows the same document again, but at a later journal position: a
    // newer head still never acknowledges the earlier mark.
    core.request(Command::Undo).unwrap();
    bundle.save(&core, &inputs).unwrap();
    let _same_picture = publish(&mut bundle);
    feedback.refresh(&bundle).unwrap();
    assert_eq!(
        snapshot_of(&feedback, &x_id),
        None,
        "C3_NEWER_HEAD_NEVER_PROMOTES"
    );
    assert!(matches!(
        feedback.save(&mut bundle, &x_id),
        Outcome::Waiting(_)
    ));
    let reference = saved_ref(feedback.save(&mut bundle, &y_id), "C3_ACKED_SAVES");
    assert_eq!(
        bundle
            .read_feedback(&reference)
            .unwrap()
            .revision
            .anchor
            .snapshot,
        newer,
        "C3_ACKED_SAVES"
    );
    assert!(
        feedback.drafts.iter().any(|draft| draft.comment == x_id),
        "C3_UNACKED_STAYS_DRAFT"
    );
    check_other_history();
}

/// Fix 1: two histories with the same document and journal length, and a forked
/// branch. Only the published version's own branch and history acknowledge a draft.
fn check_other_history() {
    let (_temp, _path, mut bundle, published_core, inputs) = new_project("c3f-histories");
    let branch = bundle.manual_branch_id().clone();
    add_track(&published_core, 70);
    published_core.request(Command::Undo).unwrap();
    let other_core = Core::spawn(Document::default()).unwrap();
    add_track(&other_core, 71);
    other_core.request(Command::Undo).unwrap();
    let own = AcceptedCut::of(&branch, &published_core.export_history().unwrap()).unwrap();
    let other = AcceptedCut::of(&branch, &other_core.export_history().unwrap()).unwrap();
    assert_eq!(
        (&own.document_hash, own.batches),
        (&other.document_hash, other.batches),
        "C3F_SAME_CONTENT_AND_LENGTH"
    );
    assert_ne!(own.history_sha256, other.history_sha256);
    let mine = capture_cut(&published_core, &branch, 7, 0);
    let theirs = capture_cut(&other_core, &branch, 8, 0);
    bundle.save(&published_core, &inputs).unwrap();
    let version = publish(&mut bundle);
    let forked = bundle.fork_branch(&branch).unwrap();
    let history = published_core.export_history().unwrap();
    let elsewhere = capture(&View {
        cell: &manual_cell(9, 0),
        pixels: None,
        selection: None,
        requested: TimeCode(0),
        manual: Some((
            AcceptedCut::of(&forked, &history).unwrap(),
            &history.document,
        )),
        texture: [16, 9],
    })
    .unwrap();
    let ids = [&mine, &theirs, &elsewhere].map(|draft| draft.comment.clone());
    let mut feedback = Feedback::default();
    feedback.drafts.extend([mine, theirs, elsewhere]);
    feedback.refresh(&bundle).unwrap();
    let snapshot = |id: &CommentId| match &feedback
        .drafts
        .iter()
        .find(|draft| &draft.comment == id)
        .unwrap()
        .origin
    {
        Origin::Captured { snapshot, .. } => snapshot.clone(),
        Origin::Revises { .. } => unreachable!(),
    };
    assert_eq!(snapshot(&ids[0]), Some(version), "C3F_OWN_HISTORY_ACK");
    assert_eq!(snapshot(&ids[1]), None, "C3F_OTHER_HISTORY_NEVER_PROMOTES");
    assert_eq!(snapshot(&ids[2]), None, "C3F_OTHER_BRANCH_NEVER_PROMOTES");
}

#[test]
fn s3_displayed_packet_and_letterbox() {
    check_letterbox();
    check_manual_cut();

    let mut retire = Retire::default();
    let mut fixtures = fixtures();
    let a = Arc::clone(&fixtures.contexts[0]);
    let b = Arc::clone(&fixtures.contexts[1]);
    let a_key = a.snapshot.descriptor.key.clone();
    let (mut app, engine) = in1_harness(a.document.clone());
    retire.arm(&engine);
    app.frames = engine.controls().frames();
    app.media_events = engine.controls().events();
    attach(&mut app, fixtures.owners.remove(0));
    let state = TempDirectory::new_in(&workspace_target_dir(), "c3f2-app-state");
    app.focused_mut().studio.feedback.directory_override = Some(state.path("state"));
    app.feedback_frame();
    assert!(
        matches!(app.focused().studio.feedback.keeping, Keeping::Kept { .. }),
        "C3F2_KEPT_BY_IDENTITY"
    );
    app.install_studio_context(Arc::clone(&a)).unwrap();
    let ctx = egui::Context::default();
    wait_frame(&mut app, &ctx, &a_key, TimeCode::ZERO);
    let shown = app.presenter.bound().unwrap();
    assert_eq!(app.focused().studio.cpu_frame_id, Some(shown.frame_id));
    let frame = app.focused().studio.cpu_frame.clone().unwrap();
    let digest = kinewright_media::sha256_bytes(&frame.rgba);
    // Another version and another position are requested, not yet shown.
    app.seek_to(TimeCode(4));
    app.install_studio_context(Arc::clone(&b)).unwrap();
    let comment = app.capture_feedback().unwrap();
    {
        let draft = app
            .focused_mut()
            .studio
            .feedback
            .draft_mut(&comment)
            .unwrap();
        let Origin::Captured { proof, .. } = &draft.origin else {
            panic!("C3_CAPTURE_BOUND");
        };
        assert_eq!(
            (&proof.shown, proof.frame, proof.bound.frame_id),
            (
                &Shown::Version {
                    snapshot: a_key.clone()
                },
                0,
                shown.frame_id
            ),
            "C3_CAPTURE_BOUND: the shown picture, not the pending request"
        );
    }
    // The picture moves on before the readback is collected.
    wait_frame(&mut app, &ctx, &b.snapshot.descriptor.key, TimeCode::ZERO);
    assert_ne!(app.presenter.bound().unwrap().frame_id, shown.frame_id);
    let area = Rect::from_min_size(Pos2::new(0.0, 0.0), Vec2::new(640.0, 480.0));
    let texture = [frame.width, frame.height];
    let image = DisplayedImage::fit(area, texture, Orientation::Upright);
    let strokes = vec![
        [[0, 0], [texture[0] / 2, texture[1] / 2]],
        [
            [texture[0] - 1, texture[1] - 1],
            [texture[0] / 4, texture[1] - 1],
        ],
    ]
    .into_iter()
    .map(|stroke| {
        stroke
            .into_iter()
            .map(|pixel| {
                let (at, expected) = landmark(area, texture, &Orientation::Upright, pixel);
                let point = image.normalise(at).unwrap();
                assert!(
                    (point[0] - expected[0]).abs() < 1e-4 && (point[1] - expected[1]).abs() < 1e-4
                );
                point
            })
            .collect::<Vec<_>>()
    })
    .collect::<Vec<_>>();
    let text = "Hold the red frame · ünïcode kept exactly";
    {
        let draft = app
            .focused_mut()
            .studio
            .feedback
            .draft_mut(&comment)
            .unwrap();
        draft.text = text.into();
        draft.strokes.clone_from(&strokes);
    }
    let outcome = app
        .with_feedback(|feedback, port| feedback.save(port, &comment))
        .unwrap();
    let Outcome::Saved(reference) = outcome else {
        panic!("C3_SAVE {outcome:?}");
    };
    let (revision, text_bytes, stroke_bytes, proof) = app
        .with_feedback(|_, port| {
            let revision = port.revision(&reference).unwrap();
            let proof: FrameProof =
                serde_json::from_slice(&port.evidence(&revision.frame_proof_ref).unwrap()).unwrap();
            (
                revision.clone(),
                port.evidence(&revision.original_text_ref).unwrap(),
                port.evidence(&revision.strokes_ref).unwrap(),
                proof,
            )
        })
        .unwrap();
    let anchor = &revision.anchor;
    assert_eq!(anchor.snapshot, a_key, "C3_BOUND_VERSION");
    assert_eq!(anchor.frame, 0, "C3_BOUND_FRAME");
    assert_eq!(
        (anchor.fps.numerator, anchor.fps.denominator),
        (a.document.fps.numerator(), a.document.fps.denominator()),
        "C3_RATIONAL_FPS"
    );
    assert_eq!(
        anchor.raster_size,
        [a.document.resolution.0, a.document.resolution.1],
        "C3_RASTER"
    );
    assert_eq!(anchor.orientation, Orientation::Upright);
    assert!(
        anchor.target.is_none() && anchor.source_ref.is_none() && anchor.script_beat_id.is_none(),
        "C3_UNKNOWN_FIELDS_ABSENT"
    );
    assert_eq!(text_bytes, text.as_bytes(), "C3_EXACT_TEXT");
    assert_eq!(
        stroke_bytes,
        strokes_bytes(&strokes).unwrap(),
        "C3_EXACT_STROKES"
    );
    assert_eq!(
        proof.shown,
        Shown::Version {
            snapshot: a_key.clone()
        },
        "C3_PROOF_VERSION"
    );
    assert_eq!(proof.frame, 0, "C3_PROOF_FRAME");
    assert_eq!(proof.bound.frame_id, shown.frame_id, "C3_PROOF_BOUND_CELL");
    assert_eq!(
        proof.pixels,
        Some(PixelDigest {
            width: frame.width,
            height: frame.height,
            sha256: digest
        }),
        "C3_PROOF_PIXELS"
    );
    check_one_frame_per_stroke(&mut app, &ctx, &a);
    in1_shutdown(&mut app);
}

/// Pump the app until the bound cell is a settled picture that meets `ready`.
fn settle(
    app: &mut KinewrightApp,
    ctx: &egui::Context,
    marker: &str,
    ready: impl Fn(&DisplayCell) -> bool,
) -> DisplayCell {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        app.poll_background(ctx);
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            app.finalize_preview(ui.ctx());
        });
        if let Some(cell) = app
            .presenter
            .bound()
            .filter(|cell| !cell.stale && ready(cell))
        {
            return cell;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "{marker}: {:?}",
            app.focused().studio.failure
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}
fn strokes_of(app: &KinewrightApp, comment: &CommentId) -> Vec<Stroke> {
    app.focused()
        .studio
        .feedback
        .drafts
        .iter()
        .find(|draft| &draft.comment == comment)
        .unwrap()
        .strokes
        .clone()
}
/// Start a stroke and admit two points over the picture it started on.
fn start_stroke(app: &mut KinewrightApp, marker: &str) -> (CommentId, u64) {
    app.feedback_stroke_start();
    let gesture = app
        .focused()
        .studio
        .feedback
        .gesture
        .clone()
        .unwrap_or_else(|| panic!("{marker}: the stroke starts"));
    assert_eq!(
        Some(gesture.1),
        app.presenter.bound().map(|cell| cell.frame_id)
    );
    assert!(app.feedback_stroke_point([0.1, 0.1]), "{marker}");
    assert!(app.feedback_stroke_point([0.2, 0.2]), "{marker}");
    gesture
}

/// Fix 1 invariants 3 and 1 in the running app: a stroke ends when the bound
/// picture changes (a landing seek, a version switch), a failed capture
/// takes no points, New note focuses a typed draft, and closing warns and keeps.
fn check_one_frame_per_stroke(
    app: &mut KinewrightApp,
    ctx: &egui::Context,
    a: &Arc<FrozenContext>,
) {
    let a_key = &a.snapshot.descriptor.key;
    let drawn = vec![vec![[0.1, 0.1], [0.2, 0.2]]];
    app.focused_mut().studio.feedback.drawing = true;
    app.focused_mut().studio.feedback.active = None;
    // A landing seek moves the picture on under a stroke in progress. (Playing
    // frames bind only through paint acknowledgments and need an audio device,
    // so headless playback is not driven here.)
    let (seeking, started) = start_stroke(app, "C3F_STROKE_SEEK");
    app.seek_to(TimeCode(2));
    settle(app, ctx, "C3F_SEEK_LANDS", |cell| {
        cell.frame_id != started && cell.at == TimeCode(2)
    });
    assert!(
        !app.feedback_stroke_point([0.3, 0.3]),
        "C3F_STROKE_ONE_FRAME: seek"
    );
    assert!(!app.feedback_stroke_point([0.4, 0.4]));
    assert_eq!(strokes_of(app, &seeking), drawn, "C3F_STROKE_ONE_FRAME");
    // A version switch.
    let (switching, started) = start_stroke(app, "C3F_STROKE_VERSION");
    app.install_studio_context(Arc::clone(a)).unwrap();
    settle(app, ctx, "C3F_VERSION_SHOWN", |cell| {
        cell.frame_id != started && cell.context.as_ref().is_some_and(|c| &c.snapshot == a_key)
    });
    assert!(
        !app.feedback_stroke_point([0.3, 0.3]),
        "C3F_STROKE_ONE_FRAME: version"
    );
    assert_eq!(strokes_of(app, &switching), drawn, "C3F_STROKE_ONE_FRAME");
    app.feedback_stroke_end();

    // A pending draft takes no points after a failed capture.
    let (pending, _) = start_stroke(app, "C3F_PENDING");
    app.feedback_stroke_end();
    let lost = app
        .with_feedback(|feedback, port| {
            let mut lossy = Lossy::new(port);
            lossy.before = 1;
            feedback.save(&mut lossy, &pending)
        })
        .unwrap();
    assert!(matches!(lost, Outcome::Unknown(_)), "{lost:?}");
    assert_eq!(
        app.focused().studio.feedback.active.as_ref(),
        Some(&pending)
    );
    app.presenter.clear();
    app.feedback_stroke_start();
    assert!(app.focused().studio.feedback.gesture.is_none());
    assert!(
        !app.feedback_stroke_point([0.9, 0.9]),
        "C3F_NO_POINTS_AFTER_FAILED_CAPTURE"
    );
    assert_eq!(
        strokes_of(app, &pending),
        drawn,
        "C3F_NO_POINTS_AFTER_FAILED_CAPTURE"
    );

    // New note: a typed draft on the bound picture, its field focused.
    app.seek_to(TimeCode(1));
    let shown = settle(app, ctx, "C3F_NEW_NOTE_PICTURE", |cell| {
        cell.at == TimeCode(1)
    });
    app.focused_mut().studio.tab = crate::studio::Tab::Commentary;
    let note = app.new_feedback_note().expect("C3F_NEW_NOTE");
    let feedback = &app.focused().studio.feedback;
    assert_eq!(feedback.focus.as_ref(), Some(&note), "C3F_NEW_NOTE focus");
    assert_eq!(app.focused().studio.tab, crate::studio::Tab::Notes);
    let Origin::Captured { proof, .. } = &feedback
        .drafts
        .iter()
        .find(|draft| draft.comment == note)
        .unwrap()
        .origin
    else {
        panic!("C3F_NEW_NOTE");
    };
    assert_eq!(proof.bound.frame_id, shown.frame_id, "C3F_NEW_NOTE bound");

    // Closing warns about typed and drawn notes, and they are kept on disk.
    let typed = "Typed before closing · kept";
    app.focused_mut()
        .studio
        .feedback
        .draft_mut(&note)
        .unwrap()
        .text = typed.into();
    assert!(
        app.focused().studio.feedback.unsaved() && app.focused().is_dirty(),
        "C3F_CLOSE_WARNS: unsaved notes are unsaved work"
    );
    app.feedback_frame();
    let Keeping::Kept { path: kept, .. } = &app.focused().studio.feedback.keeping else {
        panic!("C3F_DRAFT_KEPT: kept by identity");
    };
    let kept = kept.clone();
    let bytes = std::fs::read_to_string(&kept).unwrap();
    assert!(bytes.contains(typed), "C3F_DRAFT_KEPT");
    let warning = app.focused().ja1_close_warning();
    assert!(
        warning.contains("4 notes are not saved") && warning.contains("kept on this computer"),
        "C3F_CLOSE_WARNS: {warning}"
    );
    app.request_exit(ctx);
    assert!(
        app.exit_waits_for_choice(),
        "C3F_CLOSE_WARNS: exit asks first"
    );
    check_restored_draft_needs_capture(app, ctx, &note);
    std::fs::remove_file(&kept).unwrap();
}

/// Fix 2 invariant 4: frame IDs restart with each launch. A draft restored from
/// the kept file, whose proof names the very frame ID a different picture has
/// now, is never drawn on: the stroke captures that picture afresh.
fn check_restored_draft_needs_capture(
    app: &mut KinewrightApp,
    ctx: &egui::Context,
    note: &CommentId,
) {
    let directory = app.focused().studio.feedback.directory_override.clone();
    // A restart: this run's feedback is gone, the kept file is read back.
    app.focused_mut().studio.feedback = Feedback {
        directory_override: directory,
        ..Feedback::default()
    };
    app.feedback_frame();
    let before = strokes_of(app, note);
    app.seek_to(TimeCode(2));
    let now = settle(app, ctx, "C3F2_OTHER_PICTURE", |cell| {
        cell.at == TimeCode(2)
    });
    let feedback = &mut app.focused_mut().studio.feedback;
    let restored = feedback
        .draft_mut(note)
        .expect("C3F2_RESTORED: the note is back");
    assert!(!restored.live, "C3F2_RESTORED_NOT_LIVE");
    let Origin::Captured { proof, .. } = &mut restored.origin else {
        panic!("C3F2_RESTORED");
    };
    assert_ne!(proof.frame, now.at.0, "a different picture");
    // The new run numbers this other picture with the restored proof's ID.
    proof.bound.frame_id = now.frame_id;
    feedback.active = Some(note.clone());
    feedback.drawing = true;
    app.feedback_stroke_start();
    let gesture = app.focused().studio.feedback.gesture.clone();
    assert!(
        gesture.as_ref().is_some_and(|(comment, _)| comment != note),
        "C3F2_NO_STROKE_ON_OLD_PROOF: {gesture:?}"
    );
    assert!(app.feedback_stroke_point([0.7, 0.7]));
    app.feedback_stroke_end();
    assert_eq!(
        strokes_of(app, note),
        before,
        "C3F2_NO_STROKE_ON_OLD_PROOF: the restored draft is unchanged"
    );
}

/// A port whose replies can be lost before or after the owner commits.
struct Lossy<'a> {
    inner: &'a mut dyn FeedbackPort,
    /// Lose the request before it reaches the owner.
    before: usize,
    /// Commit, then lose the reply.
    after: usize,
    /// Report this many notes already in the index.
    full: Option<usize>,
    /// Report an index without its batches.
    forget_batches: bool,
    /// The kept drafts file, read at each dispatch: what a crash then would leave.
    crash_at_dispatch: Option<std::path::PathBuf>,
    seen: Vec<Option<Vec<u8>>>,
    /// Commands that reached this port.
    calls: usize,
}
impl<'a> Lossy<'a> {
    fn new(inner: &'a mut dyn FeedbackPort) -> Self {
        Self {
            inner,
            before: 0,
            after: 0,
            full: None,
            forget_batches: false,
            crash_at_dispatch: None,
            seen: Vec::new(),
            calls: 0,
        }
    }
}
impl FeedbackPort for Lossy<'_> {
    fn admit(
        &mut self,
        kind: FeedbackEvidenceKind,
        bytes: &[u8],
    ) -> Result<(ObjectRef, Option<StorePin>), String> {
        self.inner.admit(kind, bytes)
    }
    fn envelope(
        &mut self,
        issuer: &mut Option<IssuerId>,
    ) -> Result<(MetadataEnvelope, u64), String> {
        self.inner.envelope(issuer)
    }
    fn metadata(&mut self, envelope: &MetadataEnvelope, update: &MetadataUpdate) -> Reply {
        self.calls += 1;
        if let Some(path) = &self.crash_at_dispatch {
            self.seen.push(std::fs::read(path).ok());
        }
        if self.before > 0 {
            self.before -= 1;
            return Reply::Unknown("request lost".into());
        }
        let reply = self.inner.metadata(envelope, update);
        if self.after > 0 {
            self.after -= 1;
            return Reply::Unknown("reply lost".into());
        }
        reply
    }
    fn index(&self) -> Result<FeedbackIndex, String> {
        let mut index = self.inner.index()?;
        if self.forget_batches {
            index.batches.clear();
        }
        if let Some(full) = self.full {
            let filler = index.revisions.values().next().cloned().unwrap_or_default();
            while index.revisions.len() < full {
                index
                    .revisions
                    .insert(CommentId::fresh().unwrap(), filler.clone());
            }
        }
        Ok(index)
    }
    fn revision(&self, reference: &ObjectRef) -> Result<FeedbackRevision, String> {
        self.inner.revision(reference)
    }
    fn evidence(&self, reference: &ObjectRef) -> Result<Vec<u8>, String> {
        self.inner.evidence(reference)
    }
    fn batch(&self, batch: &FeedbackBatchId) -> Result<FeedbackBatch, String> {
        self.inner.batch(batch)
    }
    fn published(&self) -> Result<Vec<(SnapshotKey, AcceptedCut)>, String> {
        self.inner.published()
    }
}

fn saved_ref(outcome: Outcome, marker: &str) -> ObjectRef {
    let Outcome::Saved(reference) = outcome else {
        panic!("{marker}: {outcome:?}");
    };
    reference
}
fn draft_kept(feedback: &Feedback, id: &CommentId, text: &str, marker: &str) {
    let draft = feedback.drafts.iter().find(|draft| &draft.comment == id);
    assert!(
        draft.is_some_and(|draft| draft.text == text),
        "{marker}: the unsaved draft is kept"
    );
    assert!(
        !feedback
            .saved
            .iter()
            .any(|note| &note.comment == id && note.text == text)
            && !feedback
                .message
                .as_deref()
                .is_some_and(|message| message.starts_with("Saved")),
        "{marker}: nothing shows Saved early"
    );
}

#[test]
fn s3_batch_revision_and_lost_reply() {
    let (temp, path, mut bundle, core, inputs) = new_project("c3-batches");
    let branch = bundle.manual_branch_id().clone();
    let mut feedback = kept_by(&temp, &bundle);
    // Version A, then version B: notes on both make a mixed-version batch.
    let mut x = capture_cut(&core, &branch, 1, 3);
    let x_id = x.comment.clone();
    x.text = "X1 open on the red".into();
    x.strokes = vec![vec![[0.25, 0.5], [0.5, 0.5]]];
    feedback.drafts.push(x);
    let version_a = publish(&mut bundle);
    add_track(&core, 60);
    bundle.save(&core, &inputs).unwrap();
    let mut y = capture_cut(&core, &branch, 2, 5);
    let y_id = y.comment.clone();
    y.text = "Y1 trim the pause".into();
    feedback.drafts.push(y);
    let version_b = publish(&mut bundle);
    feedback.refresh(&bundle).unwrap();

    // Ingress caps refuse with the draft intact.
    let mut z = capture_cut(&core, &branch, 3, 7);
    let z_id = z.comment.clone();
    let long = "z".repeat((16 << 10) + 1);
    z.text.clone_from(&long);
    feedback.drafts.push(z);
    assert!(matches!(
        feedback.save(&mut bundle, &z_id),
        Outcome::Refused(_)
    ));
    draft_kept(&feedback, &z_id, &long, "C3_TEXT_CAP");
    let draft = feedback.draft_mut(&z_id).unwrap();
    draft.text = "points".into();
    draft.strokes = vec![vec![[0.5, 0.5]; 513]];
    assert!(matches!(
        feedback.save(&mut bundle, &z_id),
        Outcome::Refused(_)
    ));
    draft_kept(&feedback, &z_id, "points", "C3_POINT_CAP");
    feedback.draft_mut(&z_id).unwrap().strokes.clear();
    let mut full = Lossy::new(&mut bundle);
    full.full = Some(usize::try_from(StorageLimits::default().comments).unwrap());
    assert!(matches!(
        feedback.save(&mut full, &z_id),
        Outcome::Refused(_)
    ));
    draft_kept(&feedback, &z_id, "points", "C3_COMMENT_CAP");
    assert!(
        bundle.feedback_index().unwrap().revisions.is_empty(),
        "C3_CAPS_WRITE_NOTHING"
    );
    feedback.drafts.retain(|draft| draft.comment != z_id);

    let x1 = saved_ref(feedback.save(&mut bundle, &x_id), "C3_SAVE_X1");
    // Y's save commits but its reply is lost: still a draft, not Saved.
    let mut lossy = Lossy::new(&mut bundle);
    lossy.after = 1;
    let lost = feedback.save(&mut lossy, &y_id);
    assert!(matches!(lost, Outcome::Unknown(_)), "C3_LOST_SAVE {lost:?}");
    draft_kept(&feedback, &y_id, "Y1 trim the pause", "C3_LOST_SAVE");
    let y1 = saved_ref(feedback.save(&mut bundle, &y_id), "C3_RETRY_Y1");
    assert_eq!(
        bundle.feedback_index().unwrap().revisions[&y_id],
        vec![y1.clone()],
        "C3_ONE_REVISION_PER_RETRY"
    );
    // Saved but unsubmitted notes are private: no batch names them.
    assert!(
        bundle.feedback_index().unwrap().batches.is_empty(),
        "C3_PRIVATE"
    );

    feedback.toggle(&x1);
    feedback.toggle(&y1);
    let Outcome::Submitted(first) = feedback.submit(&mut bundle) else {
        panic!("C3_SUBMIT_FIRST");
    };
    assert_eq!(feedback.batches, vec![first.clone()]);

    // Reopen the mixed-version batch, edit X, deselect Y.
    feedback.reopen(&bundle, &first).unwrap();
    assert_eq!(
        feedback.selection,
        vec![x1.clone(), y1.clone()],
        "C3_REOPEN"
    );
    let note = feedback
        .saved
        .iter()
        .find(|note| note.comment == x_id)
        .unwrap()
        .clone();
    feedback.revise(&note);
    assert_eq!(
        feedback.draft_mut(&x_id).unwrap().strokes,
        vec![vec![[0.25, 0.5], [0.5, 0.5]]],
        "C3F_REVISION_KEEPS_EVIDENCE: editing starts from the drawn strokes"
    );
    feedback.draft_mut(&x_id).unwrap().text = "X2 hold a beat longer".into();
    let mut lossy = Lossy::new(&mut bundle);
    lossy.after = 1;
    assert!(matches!(
        feedback.save(&mut lossy, &x_id),
        Outcome::Unknown(_)
    ));
    draft_kept(&feedback, &x_id, "X2 hold a beat longer", "C3_LOST_EDIT");
    let x2 = saved_ref(feedback.save(&mut bundle, &x_id), "C3_RETRY_X2");
    let revised = bundle.read_feedback(&x2).unwrap().revision;
    let original = bundle.read_feedback(&x1).unwrap().revision;
    assert_eq!(
        (
            &revised.strokes_ref,
            &revised.voice_ref,
            &revised.transcript_revision
        ),
        (
            &original.strokes_ref,
            &original.voice_ref,
            &original.transcript_revision
        ),
        "C3F_REVISION_KEEPS_EVIDENCE"
    );
    // The selector shows the exact selection, an older revision included, and
    // offers that comment's newest revision in its place.
    let rows = feedback.selection_rows();
    assert_eq!(
        rows.iter()
            .map(|(reference, note, newer)| (
                reference.clone(),
                note.map(|note| note.text.clone()),
                newer.clone()
            ))
            .collect::<Vec<_>>(),
        vec![
            (
                x1.clone(),
                Some("X1 open on the red".into()),
                Some(x2.clone())
            ),
            (y1.clone(), Some("Y1 trim the pause".into()), None),
        ],
        "C3F_OLDER_SELECTED_SHOWN"
    );
    feedback.replace(&x1, &x2);
    feedback.toggle(&y1);
    assert_eq!(feedback.selection, vec![x2.clone()], "C3F_REPLACE");

    // The owner checks the frame-proof reference too: a missing or oversized
    // proof is refused and nothing is written.
    let before = (*bundle.feedback_index().unwrap()).clone();
    let missing = ObjectRef {
        sha256: "0".repeat(64),
        ..original.frame_proof_ref.clone()
    };
    let oversized = ObjectRef {
        byte_length: kinewright_project::studio_records::FRAME_PROOF_BYTES + 1,
        ..original.frame_proof_ref.clone()
    };
    for (bad, marker) in [(missing, "missing"), (oversized, "oversized")] {
        feedback.drafts.push(Draft {
            comment: x_id.clone(),
            origin: Origin::Revises {
                anchor: original.anchor.clone(),
                frame_proof_ref: bad,
                strokes_ref: original.strokes_ref.clone(),
                strokes: Vec::new(),
                voice_ref: None,
                transcript_revision: None,
            },
            text: "X3 bad proof".into(),
            strokes: Vec::new(),
            state: DraftState::Editing,
            live: false,
        });
        let outcome = feedback.save(&mut bundle, &x_id);
        assert!(
            matches!(outcome, Outcome::Refused(_)),
            "C3F_PROOF_REF_CHECKED {marker}: {outcome:?}"
        );
        feedback.drafts.retain(|draft| draft.comment != x_id);
    }
    assert_eq!(
        *bundle.feedback_index().unwrap(),
        before,
        "C3F_PROOF_REF_CHECKED: nothing written"
    );

    // The request is lost before it lands: agents see nothing.
    let mut lossy = Lossy::new(&mut bundle);
    lossy.before = 1;
    assert!(matches!(feedback.submit(&mut lossy), Outcome::Unknown(_)));
    let pending = feedback.submission.as_ref().unwrap().batch.batch_id.clone();
    assert!(
        bundle.read_submitted_batch(&pending).is_err(),
        "C3_AGENTS_SEE_NOTHING_EARLY"
    );
    assert_eq!(
        feedback.batches,
        vec![first.clone()],
        "C3_NOT_SUBMITTED_EARLY"
    );
    // A receipt the index does not show is not success.
    let mut lossy = Lossy::new(&mut bundle);
    lossy.forget_batches = true;
    assert!(
        matches!(feedback.submit(&mut lossy), Outcome::Unknown(_)),
        "C3F_SUBMIT_VERIFIED"
    );
    assert_eq!(
        feedback.batches,
        vec![first.clone()],
        "C3F_SUBMIT_VERIFIED: not shown as submitted"
    );
    // A retry whose reply is lost; the person changes the selection.
    let mut lossy = Lossy::new(&mut bundle);
    lossy.after = 1;
    assert!(matches!(feedback.submit(&mut lossy), Outcome::Unknown(_)));
    assert_eq!(
        feedback.batches,
        vec![first.clone()],
        "C3_NOT_SUBMITTED_EARLY"
    );
    feedback.toggle(&y1);
    let Outcome::Submitted(second) = feedback.submit(&mut bundle) else {
        panic!("C3_SUBMIT_RETRY");
    };
    assert_eq!(second, pending, "C3_ORIGINAL_BATCH_RETRIED");
    assert_eq!(feedback.selection, vec![y1.clone()], "C3_SELECTION_KEPT");
    let _ = (version_a, version_b);

    // Restart.
    drop(feedback);
    drop(bundle);
    let reopened = LoadedBundle::read(&path).unwrap().into_writer().unwrap();
    let index = reopened.feedback_index().unwrap();
    assert_eq!(index.batches.len(), 2, "C3_EXACTLY_ONE_BATCH_PER_SUBMIT");
    assert_eq!(
        reopened
            .read_submitted_batch(&first)
            .unwrap()
            .batch
            .revision_refs,
        vec![x1.clone(), y1.clone()],
        "C3_SUBMITTED_BATCH_IMMUTABLE"
    );
    assert_eq!(
        reopened
            .read_submitted_batch(&second)
            .unwrap()
            .batch
            .revision_refs,
        vec![x2.clone()],
        "C3_ORIGINAL_REVISION_REFS"
    );
    assert_eq!(index.revisions[&x_id], vec![x1.clone(), x2.clone()]);
    let first_x = reopened.read_feedback(&x1).unwrap().revision;
    let second_x = reopened.read_feedback(&x2).unwrap().revision;
    let first_y = reopened.read_feedback(&y1).unwrap().revision;
    assert_ne!(
        first_x.anchor.snapshot, first_y.anchor.snapshot,
        "C3_MIXED_VERSIONS"
    );
    assert_eq!(second_x.predecessor, Some(x1.clone()));
    assert_eq!(second_x.anchor, first_x.anchor, "C3_ORIGINAL_ANCHOR");
    assert_eq!(
        second_x.frame_proof_ref, first_x.frame_proof_ref,
        "C3_ORIGINAL_PROOF_REF"
    );
    let evidence = |reference: &ObjectRef| {
        String::from_utf8((*reopened.store().read_evidence(reference).unwrap()).clone()).unwrap()
    };
    assert_eq!(
        evidence(&first_x.original_text_ref),
        "X1 open on the red",
        "C3_ORIGINAL_EVIDENCE"
    );
    assert_eq!(
        evidence(&second_x.original_text_ref),
        "X2 hold a beat longer"
    );
    let proof: FrameProof = serde_json::from_str(&evidence(&first_x.frame_proof_ref)).unwrap();
    assert_eq!(proof.frame, 3);
    assert!(matches!(proof.shown, Shown::Cut { .. }));
}

/// Close the process at a dispatch: its kept drafts file is what the crash left.
fn crash_image(lossy: &mut Lossy<'_>, marker: &str) -> Vec<u8> {
    lossy
        .seen
        .pop()
        .flatten()
        .unwrap_or_else(|| panic!("{marker}: the command was kept before it was sent"))
}
fn restart(
    path: &std::path::Path,
    kept: &std::path::Path,
    image: &[u8],
) -> (ProjectBundle, Feedback) {
    std::fs::write(kept, image).unwrap();
    let bundle = LoadedBundle::read(path).unwrap().into_writer().unwrap();
    let mut feedback = Feedback::default();
    feedback
        .open(
            Ok(bundle.project_id().clone()),
            Ok(kept.parent().unwrap().to_path_buf()),
        )
        .unwrap();
    (bundle, feedback)
}
fn pending_command(feedback: &Feedback, id: &CommentId) -> MetadataUpdate {
    match &feedback
        .drafts
        .iter()
        .find(|draft| &draft.comment == id)
        .unwrap()
        .state
    {
        DraftState::Pending(pending) => pending.update.clone(),
        _ => panic!("C3F_SAME_COMMAND: the restored draft is pending"),
    }
}

/// Fix 1 invariant 1: a save or submit whose outcome is lost is kept on disk
/// before it is sent, and a restart retries that exact command once.
#[test]
fn s3_fix1_restart_retries_original() {
    let (temp, path, mut bundle, core, _inputs) = new_project("c3f-restart");
    let branch = bundle.manual_branch_id().clone();
    let kept = kept_file(&temp, &bundle);
    let mut feedback = kept_by(&temp, &bundle);
    let ids = [
        (1, "X typed"),
        (2, "Y typed"),
        (3, "W typed"),
        (4, "Z typed"),
    ]
    .map(|(frame, text)| {
        let mut draft = capture_cut(&core, &branch, frame, frame.cast_signed());
        draft.text = text.into();
        draft.strokes = vec![vec![[0.25, 0.5], [0.5, 0.75]]];
        let id = draft.comment.clone();
        feedback.drafts.push(draft);
        id
    });
    let [x_id, y_id, w_id, z_id] = ids.clone();
    publish(&mut bundle);
    feedback.refresh(&bundle).unwrap();
    let x1 = saved_ref(feedback.save(&mut bundle, &x_id), "C3F_SAVE_X");

    // Y's save commits, its reply is lost, and Kinewright closes. A submit of X
    // commits, its reply is lost too, and the crash happens there.
    let mut lossy = Lossy::new(&mut bundle);
    lossy.after = 1;
    lossy.crash_at_dispatch = Some(kept.clone());
    assert!(matches!(
        feedback.save(&mut lossy, &y_id),
        Outcome::Unknown(_)
    ));
    let y_sent = crash_image(&mut lossy, "C3F_SAVE_KEPT_BEFORE_SEND");
    let y_command = pending_command(&feedback, &y_id);
    feedback.toggle(&x1);
    lossy.after = 1;
    assert!(matches!(feedback.submit(&mut lossy), Outcome::Unknown(_)));
    let image = crash_image(&mut lossy, "C3F_SUBMIT_KEPT_BEFORE_SEND");
    let batch = feedback.submission.as_ref().unwrap().batch.clone();
    drop(lossy);
    let y_committed = bundle.feedback_index().unwrap().revisions[&y_id].clone();
    assert_eq!(y_committed.len(), 1);
    drop(feedback);
    drop(bundle);

    // What the save left at its dispatch already names Y's exact command.
    let (_, at_save) = restart(&path, &kept, &y_sent);
    assert_eq!(
        pending_command(&at_save, &y_id),
        y_command,
        "C3F_SAME_COMMAND"
    );
    drop(at_save);

    let (mut bundle, mut feedback) = restart(&path, &kept, &image);
    assert!(feedback.resume, "C3F_RESTART_RESUMES");
    assert_eq!(
        pending_command(&feedback, &y_id),
        y_command,
        "C3F_SAME_COMMAND"
    );
    assert_eq!(
        feedback.submission.as_ref().map(|s| s.batch.clone()),
        Some(batch.clone()),
        "C3F_SAME_BATCH"
    );
    for (id, text) in [(&w_id, "W typed"), (&z_id, "Z typed")] {
        draft_kept(&feedback, id, text, "C3F_DRAFT_KEPT");
    }
    feedback.resume(&mut bundle);
    let index = bundle.feedback_index().unwrap();
    assert_eq!(
        index.revisions[&y_id], y_committed,
        "C3F_RESTART_RETRIES_ORIGINAL: one revision, the original"
    );
    assert!(
        !feedback.drafts.iter().any(|draft| draft.comment == y_id),
        "C3F_RESTART_RETRIES_ORIGINAL: the draft is settled, so it is never saved twice"
    );
    assert_eq!(
        index.batches.keys().cloned().collect::<Vec<_>>(),
        vec![batch.batch_id.clone()],
        "C3F_RESTART_RETRIES_ORIGINAL: the original batch, once"
    );
    assert_eq!(
        bundle
            .read_submitted_batch(&batch.batch_id)
            .unwrap()
            .batch
            .revision_refs,
        vec![x1.clone()]
    );
    assert!(feedback.submission.is_none() && feedback.selection.is_empty());

    // W's save and a submit of Y are lost before they land, then a crash.
    let y1 = index.revisions[&y_id][0].clone();
    let mut lossy = Lossy::new(&mut bundle);
    lossy.before = 2;
    lossy.crash_at_dispatch = Some(kept.clone());
    assert!(matches!(
        feedback.save(&mut lossy, &w_id),
        Outcome::Unknown(_)
    ));
    feedback.toggle(&y1);
    assert!(matches!(feedback.submit(&mut lossy), Outcome::Unknown(_)));
    let image = crash_image(&mut lossy, "C3F_SUBMIT_KEPT_BEFORE_SEND");
    let batch = feedback.submission.as_ref().unwrap().batch.clone();
    drop(lossy);
    drop(feedback);
    drop(bundle);
    let (mut bundle, mut feedback) = restart(&path, &kept, &image);
    feedback.resume(&mut bundle);
    let index = bundle.feedback_index().unwrap();
    // A command that never landed lands once now, or is refused with the note
    // and selection intact. Never twice, never lost.
    let w_saved = index.revisions.get(&w_id).map_or(0, Vec::len);
    let w_draft = feedback.drafts.iter().any(|draft| draft.comment == w_id);
    assert!(
        (w_saved, w_draft) == (1, false) || (w_saved, w_draft) == (0, true),
        "C3F_NOTHING_DUPLICATED W: {w_saved} saved, draft kept {w_draft}"
    );
    let landed = index.batches.contains_key(&batch.batch_id);
    assert_eq!(index.batches.len(), if landed { 2 } else { 1 });
    assert!(
        landed || feedback.selection == vec![y1.clone()],
        "C3F_NOTHING_DUPLICATED batch: the exact selection is back"
    );
    assert!(
        feedback
            .drafts
            .iter()
            .any(|draft| draft.comment == z_id && draft.text == "Z typed"),
        "C3F_DRAFT_KEPT"
    );
    eprintln!("C3F before-commit restart: W saved {w_saved}, batch landed {landed}");
}

/// A saved project with typed draft X, and Y's save committed with its reply
/// lost: the kept file as a crash at Y's dispatch left it, Y's exact command,
/// and Y's one committed revision.
#[allow(clippy::type_complexity, reason = "one fixture, named at the call")]
fn lost_y_save(
    temp: &TempDirectory,
    bundle: &mut ProjectBundle,
    core: &Core,
) -> (
    CommentId,
    CommentId,
    Vec<u8>,
    MetadataUpdate,
    Vec<ObjectRef>,
) {
    let branch = bundle.manual_branch_id().clone();
    let kept = kept_file(temp, bundle);
    let mut feedback = kept_by(temp, bundle);
    let [x_id, y_id] = [(1, "X typed, kept"), (2, "Y1 sent")].map(|(frame, text)| {
        let mut draft = capture_cut(core, &branch, frame, frame.cast_signed());
        draft.text = text.into();
        draft.strokes = vec![vec![[0.25, 0.5], [0.5, 0.75]]];
        let id = draft.comment.clone();
        feedback.drafts.push(draft);
        id
    });
    publish(bundle);
    feedback.refresh(&*bundle).unwrap();
    let mut lossy = Lossy::new(&mut *bundle);
    lossy.after = 1;
    lossy.crash_at_dispatch = Some(kept);
    assert!(matches!(
        feedback.save(&mut lossy, &y_id),
        Outcome::Unknown(_)
    ));
    let image = crash_image(&mut lossy, "C3F2_KEPT_BEFORE_SEND");
    drop(lossy);
    let command = pending_command(&feedback, &y_id);
    let committed = bundle.feedback_index().unwrap().revisions[&y_id].clone();
    assert_eq!(committed.len(), 1);
    (x_id, y_id, image, command, committed)
}

/// Fix 2 invariant 1: kept notes follow the project's durable identity. A
/// renamed and moved project gets its draft and exact pending command back; a
/// different project at the old path sees none of it, and a file naming another
/// project is set aside whole, never merged.
#[test]
fn s3_fix2_recovery_follows_identity() {
    let (temp, path, mut bundle, core, _inputs) = new_project("c3f2-identity");
    let project = bundle.project_id().clone();
    let (x_id, y_id, image, command, committed) = lost_y_save(&temp, &mut bundle, &core);
    drop(bundle);

    // Rename and move the project, both of its parts, then reopen it there.
    let moved = temp.path("elsewhere/renamed.kinewright");
    std::fs::create_dir_all(moved.parent().unwrap()).unwrap();
    std::fs::rename(&path, &moved).unwrap();
    std::fs::rename(
        path.with_extension("kinewright-assets"),
        moved.with_extension("kinewright-assets"),
    )
    .unwrap();
    let kept = temp
        .path("state")
        .join(format!("{}.json", project.as_str()));
    let (mut bundle, mut feedback) = restart(&moved, &kept, &image);
    assert_eq!(bundle.project_id(), &project, "C3F2_SAME_IDENTITY");
    draft_kept(&feedback, &x_id, "X typed, kept", "C3F2_MOVED_DRAFT");
    assert_eq!(
        pending_command(&feedback, &y_id),
        command,
        "C3F2_MOVED_PENDING: the exact command"
    );
    assert!(feedback.resume, "C3F2_MOVED_RESUMES");
    feedback.resume(&mut bundle);
    assert_eq!(
        bundle.feedback_index().unwrap().revisions[&y_id],
        committed,
        "C3F2_MOVED_RETRY_EXACT: one revision, the original"
    );
    assert!(!feedback.drafts.iter().any(|draft| draft.comment == y_id));
    assert!(
        feedback
            .drafts
            .iter()
            .any(|draft| draft.comment == x_id && draft.text == "X typed, kept"),
        "C3F2_MOVED_DRAFT: still a draft after the retry"
    );
    drop(feedback);
    drop(bundle);
    let ours = std::fs::read(&kept).unwrap();

    // A different project at the old path: none of them.
    let other = ProjectBundle::activate(&path, BundleWriteAdmission::new_document()).unwrap();
    assert_ne!(other.project_id(), &project);
    let feedback = kept_by(&temp, &other);
    assert!(
        feedback.drafts.is_empty()
            && feedback.submission.is_none()
            && feedback.detached.is_empty()
            && !feedback.resume,
        "C3F2_OTHER_PROJECT_SEES_NONE"
    );
    drop(feedback);
    assert_eq!(std::fs::read(&kept).unwrap(), ours, "C3F2_OURS_UNTOUCHED");

    // Even a file under the other project's name that names this project is
    // not merged: it is set aside intact.
    let theirs = kept_file(&temp, &other);
    std::fs::write(&theirs, &ours).unwrap();
    let feedback = kept_by(&temp, &other);
    assert!(
        feedback.drafts.is_empty() && !feedback.resume,
        "C3F2_MISMATCH_NOT_MERGED"
    );
    let aside = std::fs::read_dir(temp.path("state"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|entry| {
            entry
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(&format!("{}.json.other-", other.project_id().as_str()))
        })
        .collect::<Vec<_>>();
    assert_eq!(aside.len(), 1, "C3F2_MISMATCH_SET_ASIDE {aside:?}");
    assert_eq!(
        std::fs::read(&aside[0]).unwrap(),
        ours,
        "C3F2_MISMATCH_INTACT"
    );

    // With LOCALAPPDATA unset, the real location is per-user state, never temp.
    let temp_dir = std::fs::canonicalize(std::env::temp_dir()).unwrap();
    match kinewright_project::feedback_state_directory() {
        Ok(directory) => {
            assert!(
                !directory.starts_with(&temp_dir),
                "C3F2_NEVER_TEMP {directory:?}"
            );
            if std::env::var_os("LOCALAPPDATA").is_none() {
                assert!(
                    directory.ends_with("kinewright/feedback"),
                    "C3F2_USER_STATE {directory:?}"
                );
            }
            eprintln!("C3F2 state folder: {}", directory.display());
        }
        Err(why) => eprintln!("C3F2 state folder refused: {why}"),
    }
}

/// Fix 2 invariant 2: kept notes that cannot be read refuse every send. Nothing
/// reaches the owner, the file is left byte for byte, and the draft stays.
#[test]
fn s3_fix2_unreadable_kept_refuses() {
    let (temp, _path, mut bundle, core, _inputs) = new_project("c3f2-unreadable");
    let branch = bundle.manual_branch_id().clone();
    let kept = kept_file(&temp, &bundle);
    std::fs::create_dir_all(kept.parent().unwrap()).unwrap();
    let garbage = b"{\"version\":2,\"project\":".to_vec();
    std::fs::write(&kept, &garbage).unwrap();
    let mut feedback = Feedback::default();
    let opened = feedback.open(Ok(bundle.project_id().clone()), Ok(temp.path("state")));
    assert!(opened.is_err(), "C3F2_UNREADABLE_NAMED");
    assert!(matches!(feedback.keeping, Keeping::Failed { .. }));
    let mut draft = capture_cut(&core, &branch, 1, 1);
    draft.text = "typed while the kept file is broken".into();
    let id = draft.comment.clone();
    feedback.drafts.push(draft);
    publish(&mut bundle);
    feedback.refresh(&bundle).unwrap();
    let before = (*bundle.feedback_index().unwrap()).clone();
    let mut counting = Lossy::new(&mut bundle);
    let outcome = feedback.save(&mut counting, &id);
    let calls = counting.calls;
    drop(counting);
    assert!(
        matches!(&outcome, Outcome::Refused(why) if why.contains("cannot be read")),
        "C3F2_UNREADABLE_REFUSES: {outcome:?}"
    );
    assert_eq!(calls, 0, "C3F2_NOTHING_REACHES_THE_OWNER");
    assert_eq!(*bundle.feedback_index().unwrap(), before);
    draft_kept(
        &feedback,
        &id,
        "typed while the kept file is broken",
        "C3F2_DRAFT_STAYS",
    );
    assert!(feedback.close_text().contains("Closing discards them"));
    feedback.changed = true;
    let _ = feedback.persist();
    assert_eq!(
        std::fs::read(&kept).unwrap(),
        garbage,
        "C3F2_FILE_UNCHANGED"
    );

    // A file this user cannot read at all: the same refusal.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::remove_file(&kept).unwrap();
        std::fs::write(&kept, b"{}").unwrap();
        std::fs::set_permissions(&kept, std::fs::Permissions::from_mode(0o000)).unwrap();
        if std::fs::read(&kept).is_err() {
            let mut feedback = Feedback::default();
            assert!(
                feedback
                    .open(Ok(bundle.project_id().clone()), Ok(temp.path("state")))
                    .is_err()
            );
            assert!(matches!(
                feedback.retain_for_dispatch(),
                Err(why) if why.contains("cannot be read")
            ));
        } else {
            eprintln!("C3F2: running as a user who reads mode 000 files; leg skipped");
        }
        std::fs::set_permissions(&kept, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
}

/// Fix 2 invariant 3: the kept draft is edited after its save was sent and the
/// reply lost. After a restart the old receipt settles the old command, and the
/// edited text survives as a draft of its own. The same holds for a batch.
#[test]
fn s3_fix2_edited_draft_survives_old_receipt() {
    let (temp, path, mut bundle, core, _inputs) = new_project("c3f2-edited");
    let (x_id, y_id, image, _command, committed) = lost_y_save(&temp, &mut bundle, &core);
    let kept = kept_file(&temp, &bundle);
    drop(bundle);
    let mut value: serde_json::Value = serde_json::from_slice(&image).unwrap();
    let edited = "Y2 edited in the kept file after the send";
    for draft in value["drafts"].as_array_mut().unwrap() {
        if draft["comment"] == serde_json::json!(y_id) {
            draft["text"] = serde_json::json!(edited);
        }
    }
    let (mut bundle, mut feedback) = restart(&path, &kept, &serde_json::to_vec(&value).unwrap());
    let restored = feedback
        .drafts
        .iter()
        .find(|draft| draft.comment == y_id)
        .expect("C3F2_EDITED_KEPT");
    assert!(
        !restored.pending() && restored.text == edited,
        "C3F2_EDITED_IS_A_DRAFT"
    );
    assert_eq!(feedback.detached.len(), 1, "C3F2_COMMAND_DETACHED");
    feedback.resume(&mut bundle);
    assert_eq!(
        bundle.feedback_index().unwrap().revisions[&y_id],
        committed,
        "C3F2_OLD_RECEIPT_SETTLES: the old command, once"
    );
    assert!(feedback.detached.is_empty(), "C3F2_OLD_RECEIPT_SETTLES");
    draft_kept(&feedback, &y_id, edited, "C3F2_EDITED_SURVIVES");
    let y2 = saved_ref(feedback.save(&mut bundle, &y_id), "C3F2_EDITED_SAVES");
    assert_eq!(
        bundle.feedback_index().unwrap().revisions[&y_id],
        vec![committed[0].clone(), y2],
        "C3F2_EDITED_IS_THE_NEXT_REVISION"
    );

    // A submit of Y1 whose reply is lost; the kept batch is edited to name X's
    // revision as well. The command's batch is what is retried, and the edited
    // batch's notes stay selected.
    let x1 = saved_ref(feedback.save(&mut bundle, &x_id), "C3F2_SAVE_X");
    feedback.toggle(&committed[0]);
    let mut lossy = Lossy::new(&mut bundle);
    lossy.after = 1;
    lossy.crash_at_dispatch = Some(kept.clone());
    assert!(matches!(feedback.submit(&mut lossy), Outcome::Unknown(_)));
    let image = crash_image(&mut lossy, "C3F2_SUBMIT_KEPT");
    drop(lossy);
    let sent = feedback.submission.as_ref().unwrap().batch.clone();
    drop(feedback);
    drop(bundle);
    let mut value: serde_json::Value = serde_json::from_slice(&image).unwrap();
    value["submission"]["batch"]["revision_refs"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::to_value(&x1).unwrap());
    let (mut bundle, mut feedback) = restart(&path, &kept, &serde_json::to_vec(&value).unwrap());
    assert_eq!(
        feedback.submission.as_ref().map(|s| s.batch.clone()),
        Some(sent.clone()),
        "C3F2_BATCH_IS_THE_COMMAND'S"
    );
    assert!(feedback.selection.contains(&x1), "C3F2_EDITED_BATCH_KEPT");
    feedback.resume(&mut bundle);
    let index = bundle.feedback_index().unwrap();
    assert_eq!(
        index.batches.keys().cloned().collect::<Vec<_>>(),
        vec![sent.batch_id.clone()],
        "C3F2_BATCH_ONCE"
    );
    assert_eq!(
        bundle
            .read_submitted_batch(&sent.batch_id)
            .unwrap()
            .batch
            .revision_refs,
        vec![committed[0].clone()]
    );
    assert_eq!(feedback.selection, vec![x1], "C3F2_EDITED_BATCH_KEPT");
}
