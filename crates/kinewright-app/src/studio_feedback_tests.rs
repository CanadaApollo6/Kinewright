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
use kinewright_project::bundle::{BundleWriteAdmission, LoadedBundle};

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
fn capture_cut(core: &Core, frame_id: u64, at: i64) -> Draft {
    let history = core.export_history().unwrap();
    let cut = AcceptedCut::of(&history).unwrap();
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

fn check_manual_cut() {
    let (_temp, _path, mut bundle, core, inputs) = new_project("c3-manual-cut");
    let mut feedback = Feedback::default();
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
    add_track(&core, 50);
    bundle.save(&core, &inputs).unwrap();
    let x = capture_cut(&core, 1, 2);
    let x_id = x.comment.clone();
    feedback.drafts.push(x);
    add_track(&core, 51);
    bundle.save(&core, &inputs).unwrap();
    let y = capture_cut(&core, 2, 3);
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
    in1_shutdown(&mut app);
}

/// A port whose replies can be lost before or after the owner commits.
struct Lossy<'a> {
    inner: &'a mut ProjectBundle,
    /// Lose the request before it reaches the owner.
    before: usize,
    /// Commit, then lose the reply.
    after: usize,
    /// Report this many notes already in the index.
    full: Option<usize>,
}
impl<'a> Lossy<'a> {
    fn new(inner: &'a mut ProjectBundle) -> Self {
        Self {
            inner,
            before: 0,
            after: 0,
            full: None,
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
    let (_temp, path, mut bundle, core, inputs) = new_project("c3-batches");
    let mut feedback = Feedback::default();
    // Version A, then version B: notes on both make a mixed-version batch.
    let mut x = capture_cut(&core, 1, 3);
    let x_id = x.comment.clone();
    x.text = "X1 open on the red".into();
    x.strokes = vec![vec![[0.25, 0.5], [0.5, 0.5]]];
    feedback.drafts.push(x);
    let version_a = publish(&mut bundle);
    add_track(&core, 60);
    bundle.save(&core, &inputs).unwrap();
    let mut y = capture_cut(&core, 2, 5);
    let y_id = y.comment.clone();
    y.text = "Y1 trim the pause".into();
    feedback.drafts.push(y);
    let version_b = publish(&mut bundle);
    feedback.refresh(&bundle).unwrap();

    // Ingress caps refuse with the draft intact.
    let mut z = capture_cut(&core, 3, 7);
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
    feedback.draft_mut(&x_id).unwrap().text = "X2 hold a beat longer".into();
    let mut lossy = Lossy::new(&mut bundle);
    lossy.after = 1;
    assert!(matches!(
        feedback.save(&mut lossy, &x_id),
        Outcome::Unknown(_)
    ));
    draft_kept(&feedback, &x_id, "X2 hold a beat longer", "C3_LOST_EDIT");
    let x2 = saved_ref(feedback.save(&mut bundle, &x_id), "C3_RETRY_X2");
    feedback.toggle(&y1);
    feedback.toggle(&x1);
    feedback.toggle(&x2);
    assert_eq!(feedback.selection, vec![x2.clone()]);

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
    // The retry lands but its reply is lost; the person changes the selection.
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
