#![cfg(target_os = "linux")]
#![allow(
    clippy::too_many_lines,
    reason = "end-to-end witnesses retain one complete transition and its oracles"
)]
use super::*;
use crate::app::in1_tests::{in1_harness, in1_shutdown};
use kinewright_core::{
    Analysis, Command, Core, Event, Operation, Playback, Track, TrackId, TrackKind,
};
use kinewright_media::test_support::{TempDirectory, workspace_target_dir};
use kinewright_project::{
    bundle::{LoadedBundle, ProjectBundle},
    bundle_records::BranchCheckpoint,
    editing_session::Ja1EditingSession,
    receipts::{CommandId, ReceiptLookup, ReceiptResult},
    storage_budget::StorageLimits,
    studio_records::*,
};
use std::time::{Duration, Instant};

fn publish(bundle: &mut ProjectBundle) -> SnapshotKey {
    let envelope = MetadataEnvelope {
        id: CommandId {
            issuer: bundle.issue_metadata_issuer().unwrap(),
            sequence: 1,
        },
        project_id: bundle.project_id().clone(),
        target_branch: bundle.manual_branch_id().clone(),
        owner_epoch: bundle.store().owner_epoch(),
        expected_head: bundle.manual_head().cloned(),
    };
    let update = MetadataUpdate {
        expected_root_generation: bundle.root().unwrap().generation,
        change: MetadataChange::PublishSnapshot(PublishSnapshot {
            version: VersionId::fresh().unwrap(),
            expected_head: bundle.manual_head().unwrap().clone(),
            checkpoint_ref: bundle
                .checkpoint_reference(bundle.manual_branch_id())
                .unwrap(),
            parent_version: None,
            run_provenance: None,
            revision_refs: vec![],
        }),
    };
    let ReceiptLookup::Committed(receipt) = bundle
        .metadata_command(&envelope, MetadataScope::Snapshot, &update)
        .unwrap()
    else {
        panic!("snapshot committed")
    };
    let ReceiptResult::Metadata {
        result: MetadataResult::Snapshot(key),
    } = receipt.result
    else {
        panic!("snapshot result")
    };
    key
}
struct Fixtures {
    root: TempDirectory,
    owners: Vec<Ja1EditingSession>,
    contexts: Vec<Arc<FrozenContext>>,
}
fn fixtures() -> Fixtures {
    let root = TempDirectory::new_in(&workspace_target_dir(), "c2-frozen");
    let pair = kinewright_project::c0_fixtures::build_c2_fixture_pair(root.root()).unwrap();
    let media_bytes = pair
        .iter()
        .map(|fixture| {
            let path = fixture
                .path
                .with_extension("kinewright-assets")
                .join("studio/objects")
                .join(&fixture.media.entry.object.sha256);
            let bytes = std::fs::read(path).unwrap();
            assert_eq!(
                kinewright_media::sha256_bytes(&bytes),
                fixture.media.entry.object.sha256
            );
            bytes
        })
        .collect::<Vec<_>>();
    let mut owners = Vec::new();
    let mut contexts = Vec::new();
    for (index, fixture) in pair.into_iter().enumerate() {
        let loaded = LoadedBundle::read(&fixture.path).unwrap();
        let core = Core::spawn(Document::default()).unwrap();
        loaded.restore_manual(&core).unwrap();
        let inputs = loaded
            .manual_checkpoint()
            .unwrap()
            .input_manifest_ref
            .clone();
        let mut bundle = loaded.into_writer().unwrap();
        let alias = root.path(&format!("legacy-locator-{index}.mkv"));
        std::fs::write(&alias, &media_bytes[index]).unwrap();
        let asset = core.export_history().unwrap().document.media_pool[0].clone();
        assert!(matches!(
            core.request(Command::Do(Operation::RelinkAsset {
                asset: asset.id,
                candidate: crate::media_workflow::build_relink_candidate(alias, &asset),
                allow_unverified_source: false
            }))
            .unwrap(),
            Event::DocumentChanged { .. }
        ));
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
        let mut parameters = std::collections::BTreeMap::new();
        parameters.insert(
            "lut_asset_id".into(),
            kinewright_core::ParamValue::Integer(1),
        );
        parameters.insert(
            "mix_basis_points".into(),
            kinewright_core::ParamValue::Integer(if index == 1 { 5000 } else { 10000 }),
        );
        assert!(matches!(
            core.request(Command::Do(Operation::AddEffect {
                clip,
                effect: kinewright_core::Effect {
                    id: kinewright_core::EffectId(1),
                    name: "creative_look".into(),
                    parameters,
                    keyframes: std::collections::BTreeMap::default(),
                    enabled: true,
                    enabled_curve: None
                }
            }))
            .unwrap(),
            Event::DocumentChanged { .. }
        ));
        if index == 1 {
            assert!(matches!(
                core.request(Command::Do(Operation::SetClipSpeed {
                    clip,
                    speed_percent: 200
                }))
                .unwrap(),
                Event::DocumentChanged { .. }
            ));
        }
        bundle.save(&core, &inputs).unwrap();
        let key = publish(&mut bundle);
        let context = Arc::new(bundle.frozen_context(&key).unwrap());
        println!(
            "C2_FIXTURE {}",
            serde_json::json!({"snapshot":key,"dependencies":context.snapshot.roots,"media":context.certified_input_leases[0].identity(),"document":kinewright_media::sha256_bytes(&serde_json::to_vec(&context.document).unwrap())})
        );
        contexts.push(context);
        owners.push(Ja1EditingSession::from_bundle(bundle, &core, inputs).unwrap());
    }
    // An external writer replaces both locators only after certification.
    // Correct playback must continue to read the pinned owned descriptors.
    for (index, context) in contexts.iter().enumerate() {
        std::fs::write(
            &context.document.media_pool[0].path,
            &media_bytes[1 - index],
        )
        .unwrap();
    }
    Fixtures {
        root,
        owners,
        contexts,
    }
}
fn attach(app: &mut KinewrightApp, owner: Ja1EditingSession) {
    app.projects[0].recovery.stop_for_studio();
    app.projects[0].ja1 = Some(crate::ja1_attachment::Ja1Attachment::new(owner).unwrap());
    app.projects[0].install_ja1_view();
}
fn wait_frame(app: &mut KinewrightApp, context: &egui::Context, key: &SnapshotKey, at: TimeCode) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        app.poll_background(context);
        let _ = context.run_ui(egui::RawInput::default(), |ui| {
            app.finalize_preview(ui.ctx());
        });
        if app.presenter.bound().is_some_and(|cell| {
            !cell.stale
                && cell.at == at
                && cell.context.as_ref().is_some_and(|c| c.snapshot == *key)
        }) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "C2_FRAME_DEADLINE: {:?}",
            app.focused().studio.failure
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}
fn center(frame: &kinewright_core::FrameTexture) -> [u8; 4] {
    let index = ((frame.height / 2 * frame.width + frame.width / 2) * 4) as usize;
    frame.rgba[index..index + 4].try_into().unwrap()
}

#[test]
fn s3_frozen_selection_survives_background_copy_and_reopen() {
    let mut retire = tests::Retire::default();
    let mut fixtures = fixtures();
    let context = Arc::clone(&fixtures.contexts[0]);
    let key = context.snapshot.descriptor.key.clone();
    let expected = serde_json::to_vec(&context.document).unwrap();
    let (mut app, engine) = in1_harness(context.document.clone());
    retire.arm(&engine);
    app.frames = engine.frames();
    app.media_events = engine.events();
    attach(&mut app, fixtures.owners.remove(0));
    app.install_studio_context(Arc::clone(&context)).unwrap();
    let ctx = egui::Context::default();
    wait_frame(&mut app, &ctx, &key, TimeCode::ZERO);
    assert!(matches!(
        app.focused()
            .core
            .request(Command::Do(Operation::AddTrack {
                track: Track {
                    id: TrackId(99),
                    kind: TrackKind::Audio,
                    sync_lock: true,
                    clips: vec![]
                }
            }))
            .unwrap(),
        Event::DocumentChanged { .. }
    ));
    app.poll_background(&ctx);
    assert_eq!(
        app.focused()
            .studio
            .selection
            .as_ref()
            .unwrap()
            .snapshot
            .descriptor
            .key,
        key,
        "C2_BACKGROUND_SELECTION"
    );
    assert_eq!(
        serde_json::to_vec(app.program_document()).unwrap(),
        expected,
        "C2_BACKGROUND_DOCUMENT"
    );
    assert_ne!(*app.focused().document, context.document);
    app.active_thread_mut().input = "draft still belongs to the original".into();
    app.focused_mut().studio.draft_snapshot = Some(key.clone());
    app.seek_to(TimeCode(4));
    wait_frame(&mut app, &ctx, &key, TimeCode(4));
    let attachment = app.focused_mut().ja1.as_mut().unwrap();
    attachment
        .apply(Action::StudioEditCopy(key.clone()))
        .unwrap();
    app.focused_mut().install_ja1_view();
    app.sync_studio_selection();
    assert!(
        app.focused().studio.selection.is_none(),
        "C2_EDIT_COPY_ACTIVATED"
    );
    assert!(app.focused().ja1_job.is_some());
    let pending = engine.stamp();
    app.play_program_from(app.program_position());
    app.toggle_playback();
    assert_eq!(engine.stamp(), pending, "C2_PENDING_AUDIO_OWNER");
    let deadline = Instant::now() + Duration::from_secs(30);
    while app.focused().ja1.is_none() || app.focused().studio.manual_inputs.is_none() {
        app.poll_background(&ctx);
        assert!(Instant::now() < deadline, "C2_MANUAL_CERTIFIED_COPY");
        std::thread::sleep(Duration::from_millis(5));
    }

    let core = app.focused().core.clone();
    assert!(matches!(
        core.request(Command::Do(Operation::AddTrack {
            track: Track {
                id: TrackId(100),
                kind: TrackKind::Audio,
                sync_lock: true,
                clips: vec![]
            }
        }))
        .unwrap(),
        Event::DocumentChanged { .. }
    ));
    core.request(Command::Undo).unwrap();
    core.request(Command::Redo).unwrap();
    let copy = fixtures.root.path("copy.kinewright");
    app.focused_mut()
        .ja1
        .as_mut()
        .unwrap()
        .apply(Action::SaveAs(copy.clone()))
        .unwrap();
    let original = app
        .focused()
        .ja1
        .as_ref()
        .unwrap()
        .studio_context(&key)
        .unwrap();
    assert_eq!(
        serde_json::to_vec(&original.document).unwrap(),
        expected,
        "C2_SUCCESSOR_ISOLATION"
    );
    assert_eq!(
        original.snapshot.descriptor, context.snapshot.descriptor,
        "C2_ORIGINAL_REFS"
    );
    drop(original);
    app.return_to_original_version();
    wait_frame(&mut app, &ctx, &key, TimeCode(4));
    assert_eq!(
        app.active_thread().input,
        "draft still belongs to the original"
    );
    in1_shutdown(&mut app);
    drop(app);
    drop(engine);
    retire.wait();
    let reopened = LoadedBundle::read(&copy).unwrap().into_writer().unwrap();
    let original = reopened.frozen_context(&key).unwrap();
    assert_eq!(
        serde_json::to_vec(&original.document).unwrap(),
        expected,
        "C2_REOPEN_ORIGINAL"
    );
    let checkpoint = reopened
        .store()
        .read_record::<BranchCheckpoint>(&original.snapshot.descriptor.checkpoint_ref)
        .unwrap();
    assert_ne!(
        reopened.manual_branch_id(),
        &checkpoint.branch_id,
        "C2_COPY_BRANCH_DISTINCT"
    );
    let prior = reopened.checkpoint(&checkpoint.branch_id).unwrap();
    let prior_document = reopened
        .store()
        .read_record::<Document>(&prior.document_ref)
        .unwrap();
    assert!(
        prior_document.tracks.iter().any(|t| t.id == TrackId(99)),
        "C2_PRIOR_WORK_RETAINED"
    );
    assert!(
        !prior_document.tracks.iter().any(|t| t.id == TrackId(100)),
        "C2_COPY_ISOLATED_FROM_PRIOR_WORK"
    );
    let copy_checkpoint = reopened.checkpoint(reopened.manual_branch_id()).unwrap();
    let copy_document = reopened
        .store()
        .read_record::<Document>(&copy_checkpoint.document_ref)
        .unwrap();
    assert!(
        copy_document.tracks.iter().any(|t| t.id == TrackId(100)),
        "C2_COPY_EDIT_REOPENED"
    );
    assert_eq!(
        original.certified_input_leases[0]
            .identity()
            .entry
            .dependencies,
        context.certified_input_leases[0]
            .identity()
            .entry
            .dependencies
    );
}

#[test]
fn s3_bound_ab_identity() {
    use crate::presenter::paint_support::{Painted, Pass, SIDE};
    let mut retire = tests::Retire::default();
    let fixtures = fixtures();
    let a = Arc::clone(&fixtures.contexts[0]);
    let b = Arc::clone(&fixtures.contexts[1]);
    let (mut app, engine) = in1_harness(a.document.clone());
    retire.arm(&engine);
    app.frames = engine.frames();
    app.media_events = engine.events();
    let ctx = egui::Context::default();
    app.install_studio_context(Arc::clone(&a)).unwrap();
    wait_frame(&mut app, &ctx, &a.snapshot.descriptor.key, TimeCode::ZERO);
    let first = app.presenter.bound().unwrap();
    let audio_a = first
        .context
        .as_ref()
        .unwrap()
        .inputs
        .test_audio_samples()
        .unwrap();
    assert!(
        (audio_frequency(&audio_a) - 440.0).abs() < 30.0,
        "C2_AUDIO_A_LANDMARK"
    );
    let a_pixels = center(app.focused().studio.cpu_frame.as_ref().unwrap());
    assert!(
        u16::from(a_pixels[0]) > u16::from(a_pixels[2]) + 80,
        "C2_RED_DECODED_LANDMARK {a_pixels:?}"
    );
    app.capture_studio_still(0, &ctx);
    // A new owner namespace is not ready yet. Even a genuine completion from
    // the old PF1 scene must stay under the prior stale display cell.
    app.presenter.suspend();
    engine.request_frame(TimeCode::ZERO);
    let deadline = Instant::now() + Duration::from_secs(10);
    let completion = loop {
        if let Ok(frame) = app.frames.try_recv() {
            break frame;
        }
        assert!(Instant::now() < deadline, "C2_PENDING_REAL_COMPLETION");
        std::thread::sleep(Duration::from_millis(5));
    };
    app.presenter.collect(completion);
    app.finalize_preview(&ctx);
    assert_eq!(
        app.presenter.bound().unwrap().frame_id,
        first.frame_id,
        "C2_PENDING_NAMESPACE_FENCE"
    );
    assert!(app.presenter.bound().unwrap().stale);
    app.install_studio_context(Arc::clone(&b)).unwrap();
    app.finalize_preview(&ctx);
    let stale = app.presenter.bound().unwrap();
    assert!(stale.stale);
    assert_eq!(
        stale.context.as_ref().unwrap().snapshot,
        a.snapshot.descriptor.key,
        "C2_STALE_IDENTITY"
    );
    app.seek_to(TimeCode(4));
    wait_frame(&mut app, &ctx, &b.snapshot.descriptor.key, TimeCode(4));
    let audio_b = app
        .presenter
        .bound()
        .unwrap()
        .context
        .unwrap()
        .inputs
        .test_audio_samples()
        .unwrap();
    assert!(
        (audio_frequency(&audio_b) - 880.0).abs() < 30.0,
        "C2_AUDIO_B_LANDMARK"
    );
    let b_pixels = center(app.focused().studio.cpu_frame.as_ref().unwrap());
    assert!(
        b_pixels[0] > 130
            && b_pixels[0] < 230
            && b_pixels[1] > 230
            && b_pixels[2] > 130
            && b_pixels[2] < 230,
        "C2_TIMED_LUT_LANDMARK {b_pixels:?}"
    );
    app.capture_studio_still(1, &ctx);
    assert_eq!(
        app.focused().studio.stills[0].as_ref().unwrap().cell,
        first,
        "C2_STILL_IDENTITY"
    );
    assert_ne!(
        app.focused().studio.stills[0]
            .as_ref()
            .unwrap()
            .image
            .image()
            .0,
        app.focused().studio.stills[1]
            .as_ref()
            .unwrap()
            .image
            .image()
            .0
    );
    let mut paint = Painted::new();
    let whole = Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(SIDE, SIDE));
    paint.pass(
        &app.presenter,
        &|| app.paint_clock(),
        whole,
        whole,
        Pass::Rendered,
    );
    let painted = app.presenter.paint_mark_for_test().unwrap();
    assert_eq!(
        painted.cell.context.as_ref().unwrap().snapshot,
        b.snapshot.descriptor.key,
        "C2_PAINTED_IDENTITY"
    );
    assert_eq!(painted.cell.at, TimeCode(4));
    let applied = engine.test_applied_scene().unwrap();
    assert_eq!(
        applied["document_sha256"], b.snapshot.descriptor.key.document_hash,
        "C2_APPLIED_DOCUMENT"
    );
    let identity = b.certified_input_leases[0]
        .certified_source()
        .unwrap()
        .identity();
    assert_eq!(
        applied["certified_sources"],
        serde_json::json!([{
            "asset": 1, "source_length": identity.source_length,
            "source_sha256": identity.source_sha256, "table_sha256": identity.table_sha256,
        }]),
        "C2_APPLIED_SOURCE_AND_TABLE"
    );
    println!(
        "C2_BOUND {}",
        serde_json::json!({"requested":b.snapshot.descriptor.key,"applied":applied,"bound":app.presenter.bound().unwrap().context.as_ref().unwrap().snapshot,"painted":painted.cell.context.as_ref().unwrap().snapshot,"frame":painted.cell.at,"A_rgba":a_pixels,"B_rgba":b_pixels,"A_audio_hz":audio_frequency(&audio_a),"B_audio_hz":audio_frequency(&audio_b)})
    );
    in1_shutdown(&mut app);
}

fn audio_frequency(samples: &[f32]) -> f64 {
    let mono = samples.iter().step_by(2).copied().collect::<Vec<_>>();
    let rising = mono
        .windows(2)
        .filter(|pair| pair[0] <= 0.0 && pair[1] > 0.0)
        .count();
    f64::from(u32::try_from(rising).unwrap()) * 48_000.0
        / f64::from(u32::try_from(mono.len()).unwrap())
}

#[test]
fn s3_layout_roundtrip() {
    let mut retire = tests::Retire::default();
    let fixtures = fixtures();
    let selected = Arc::clone(&fixtures.contexts[0]);
    let (mut app, engine) = in1_harness(selected.document.clone());
    retire.arm(&engine);
    app.frames = engine.frames();
    app.media_events = engine.events();
    app.install_studio_context(Arc::clone(&selected)).unwrap();
    let ctx = egui::Context::default();
    crate::theme::install(&ctx);
    app.seek_to(TimeCode(4));
    wait_frame(
        &mut app,
        &ctx,
        &selected.snapshot.descriptor.key,
        TimeCode(4),
    );
    app.active_thread_mut().input = "unfinished note on old snapshot".into();
    app.focused_mut().studio.draft_snapshot = Some(selected.snapshot.descriptor.key.clone());
    for (width, mode) in [
        (1232.0, Mode::Studio),
        (1600.0, Mode::Studio),
        (600.0, Mode::Studio),
        (1232.0, Mode::Editor),
        (600.0, Mode::Editor),
        (1232.0, Mode::Studio),
    ] {
        app.focused_mut().studio.mode = mode;
        let view = Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width, 1000.0));
        let layout = Layout::new(view, mode == Mode::Editor);
        let _ = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(view),
                ..Default::default()
            },
            |ui| app.studio_layout(ui),
        );
        assert!(layout.picture.bottom() <= layout.plate.top());
        assert!(layout.plate.bottom() <= layout.composer.top());
        if !layout.narrow {
            assert!(layout.picture.right() < layout.rail.left());
            assert!(layout.timeline.top() > layout.composer.bottom());
        }
        assert_eq!(app.program_position(), TimeCode(4));
        assert_eq!(app.active_thread().input, "unfinished note on old snapshot");
        assert_eq!(
            app.focused()
                .studio
                .selection
                .as_ref()
                .unwrap()
                .snapshot
                .descriptor
                .key,
            selected.snapshot.descriptor.key
        );
        assert_eq!(
            app.focused().studio.draft_snapshot,
            Some(selected.snapshot.descriptor.key.clone())
        );
    }
    in1_shutdown(&mut app);
}

#[test]
fn s3_still_import_and_manual_placement() {
    use kinewright_media::test_support::GeneratedMedia;
    let root = TempDirectory::new_in(&workspace_target_dir(), "c2-still-placement");
    for codec in ["png", "mjpeg"] {
        let mut retire = tests::Retire::default();
        let suffix = if codec == "png" { "png" } else { "jpg" };
        let media = GeneratedMedia::ffmpeg(
            "c2-still",
            &[
                "-f",
                "lavfi",
                "-i",
                "color=c=red:s=32x24",
                "-frames:v",
                "1",
                "-c:v",
                codec,
            ],
            suffix,
        );
        let (mut app, engine) = in1_harness(Document {
            tracks: vec![Track {
                id: TrackId(1),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: vec![],
            }],
            ..Document::default()
        });
        retire.arm(&engine);
        app.frames = engine.frames();
        app.media_events = engine.events();
        let mut owner =
            Ja1EditingSession::new(root.root(), &app.focused().core, StorageLimits::default())
                .unwrap();
        let path = root.path(&format!("{codec}.kinewright"));
        owner.first_save(&path, StorageLimits::default()).unwrap();
        let probed = engine.probe(media.path()).unwrap();
        let Operation::SetAssetColorDescription {
            mut color_description,
            ..
        } = kinewright_core::assume_rec709_operation(probed.id, &probed.color_description)
        else {
            unreachable!()
        };
        color_description.provenance = kinewright_core::ColorProvenance::UserOverride;
        let (asset, _) = owner
            .import_owned_media_with_color(media.path(), Some(color_description))
            .unwrap();
        let attachment = crate::ja1_attachment::Ja1Attachment::new(owner).unwrap();
        assert_eq!(
            asset.kind,
            kinewright_core::MediaKind::Image,
            "C2_STILL_ADMITTED"
        );
        app.focused_mut().ja1 = Some(attachment);
        app.focused_mut().install_ja1_view();
        app.add_asset_to_timeline(asset.id);
        let ctx = egui::Context::default();
        let deadline = Instant::now() + Duration::from_secs(10);
        while app.focused().document.tracks[0].clips.is_empty() {
            app.poll_background(&ctx);
            assert!(Instant::now() < deadline, "still placement");
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(
            app.focused().document.duration.0 > 1,
            "C2_STILL_MANUAL_DURATION"
        );
        while app.focused().ja1.is_none()
            || app.focused().studio.manual_inputs.is_none()
            || app.focused().studio.cpu_frame.is_none()
        {
            app.poll_background(&ctx);
            app.finalize_preview(&ctx);
            assert!(
                app.focused().studio.failure.is_none(),
                "C2_STILL_PLAYBACK_FAILURE {:?}",
                app.focused().studio.failure
            );
            assert!(Instant::now() < deadline, "C2_STILL_PLAYBACK_PREPARE");
            std::thread::sleep(Duration::from_millis(5));
        }
        let pixel = center(app.focused().studio.cpu_frame.as_ref().unwrap());
        assert!(
            pixel[0] > 180 && u16::from(pixel[0]) > u16::from(pixel[2]) + 80,
            "C2_STILL_PF1_PIXELS {pixel:?}"
        );

        app.focused_mut()
            .ja1
            .as_mut()
            .unwrap()
            .apply(Action::Save)
            .unwrap();
        let loaded = LoadedBundle::read(&path).unwrap();
        assert_eq!(
            loaded.admission().inspection_document().unwrap().media_pool[0].kind,
            kinewright_core::MediaKind::Image
        );
        in1_shutdown(&mut app);
        drop(app);
        drop(engine);
    }
}

#[test]
fn s3_bound_ab_gpu_identity() {
    let mut retire = tests::Retire::default();
    let fixtures = fixtures();
    let a = Arc::clone(&fixtures.contexts[0]);
    let b = Arc::clone(&fixtures.contexts[1]);
    let (mut app, engine, _gpu, ctx, _root) = crate::app::s3a_tests::display_app();
    retire.arm(&engine);
    app.install_studio_context(Arc::clone(&a)).unwrap();
    wait_frame(&mut app, &ctx, &a.snapshot.descriptor.key, TimeCode::ZERO);
    assert!(app.presenter.using_gpu());
    crate::app::s3a_tests::r71::rendered(&mut app, &ctx);
    app.capture_studio_still(0, &ctx);
    engine.play(TimeCode::ZERO);
    let deadline = Instant::now() + Duration::from_secs(10);
    while engine.test_audio_owner_counts()[2] == 0 {
        app.poll_background(&ctx);
        assert!(Instant::now() < deadline, "C2_AUDIO_A_STARTED");
        std::thread::sleep(Duration::from_millis(5));
    }
    app.install_studio_context(Arc::clone(&b)).unwrap();
    app.seek_to(TimeCode(4));
    wait_frame(&mut app, &ctx, &b.snapshot.descriptor.key, TimeCode(4));
    let pixels = crate::app::s3a_tests::r71::rendered(&mut app, &ctx);
    let offset = (32 * 64 + 32) * 4;
    let pixel = &pixels[offset..offset + 4];
    assert!(
        pixel[0] > 130 && pixel[0] < 230 && pixel[1] > 230 && pixel[2] > 130 && pixel[2] < 230,
        "C2_GPU_TIMED_LUT_LANDMARK {pixel:?}"
    );
    app.capture_studio_still(1, &ctx);
    let a_still = app.focused().studio.stills[0].as_ref().unwrap();
    let b_still = app.focused().studio.stills[1].as_ref().unwrap();
    assert_eq!(
        a_still.cell.context.as_ref().unwrap().snapshot,
        a.snapshot.descriptor.key,
        "C2_GPU_A_BOUND"
    );
    assert_eq!(
        b_still.cell.context.as_ref().unwrap().snapshot,
        b.snapshot.descriptor.key,
        "C2_GPU_B_BOUND"
    );
    let (a_image, b_image) = (a_still.image.image().0, b_still.image.image().0);
    let kept_a = crate::app::s3a_tests::r71::rendered_still(&mut app, &ctx, a_image);
    let kept_b = crate::app::s3a_tests::r71::rendered_still(&mut app, &ctx, b_image);
    let (a_pixel, b_pixel) = (&kept_a[offset..offset + 4], &kept_b[offset..offset + 4]);
    let format = app.native_display.as_ref().unwrap().renderer.target_format;
    let red = match format {
        eframe::wgpu::TextureFormat::Rgba8Unorm | eframe::wgpu::TextureFormat::Rgba8UnormSrgb => 0,
        eframe::wgpu::TextureFormat::Bgra8Unorm | eframe::wgpu::TextureFormat::Bgra8UnormSrgb => 2,
        other => panic!("unexpected native readback format {other:?}"),
    };
    assert!(
        a_pixel[red] > 230 && a_pixel[2 - red] < 30,
        "C2_GPU_ORIGINAL_STILL_PIXELS {a_pixel:?}"
    );
    assert!(
        b_pixel[0] > 130 && b_pixel[0] < 230 && b_pixel[1] > 230 && b_pixel[2] > 130,
        "C2_GPU_SUCCESSOR_STILL_PIXELS {b_pixel:?}"
    );
    println!("C2_GPU_STILLS format={format:?} A={a_pixel:?} B={b_pixel:?}");
    let painted = app.presenter.paint_mark_for_test().unwrap();
    assert_eq!(
        painted.cell.context.unwrap().snapshot,
        b.snapshot.descriptor.key,
        "C2_GPU_PAINTED"
    );
    engine.play(TimeCode::ZERO);
    let deadline = Instant::now() + Duration::from_secs(10);
    while engine.test_audio_owner_counts()[2] < 2 {
        app.poll_background(&ctx);
        assert!(Instant::now() < deadline, "C2_AUDIO_B_STARTED");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(engine.test_audio_owner_counts()[1], 1, "C2_ONE_AUDIO_OWNER");
    engine.pause();
    in1_shutdown(&mut app);
}

#[test]
fn s3_failed_edit_copy_keeps_frozen_selection() {
    let mut retire = tests::Retire::default();
    let mut fixtures = fixtures();
    let selected = Arc::clone(&fixtures.contexts[0]);
    let (mut app, engine) = in1_harness(selected.document.clone());
    retire.arm(&engine);
    app.frames = engine.frames();
    app.media_events = engine.events();
    attach(&mut app, fixtures.owners.remove(0));
    app.install_studio_context(Arc::clone(&selected)).unwrap();
    let before = app.focused().core.actor_identity();
    let mut wrong = selected.snapshot.descriptor.key.clone();
    wrong.version = VersionId::fresh().unwrap();
    assert!(
        app.focused_mut()
            .ja1
            .as_mut()
            .unwrap()
            .apply(Action::StudioEditCopy(wrong))
            .is_err()
    );
    app.sync_studio_selection();
    assert!(
        app.focused().studio.selection.is_some(),
        "C2_FAILED_COPY_FROZEN"
    );
    assert_eq!(app.focused().core.actor_identity(), before);
    assert!(
        app.focused()
            .send_manual_command(Command::Do(Operation::AddTrack {
                track: Track {
                    id: TrackId(99),
                    kind: TrackKind::Audio,
                    sync_lock: true,
                    clips: vec![]
                }
            }))
            .is_err(),
        "C2_FAILED_COPY_NO_MANUAL_AUTHORITY"
    );
    in1_shutdown(&mut app);
}

#[allow(
    clippy::module_inception,
    reason = "test boundary excludes teardown waits from production source scans"
)]
#[cfg(test)]
mod tests {
    use std::{
        sync::Arc,
        time::{Duration, Instant},
    };
    #[derive(Default)]
    pub(super) struct Retire {
        done: Option<crossbeam_channel::Receiver<()>>,
        gpu: Option<kinewright_media::GpuContext>,
    }
    impl Retire {
        pub(super) fn arm(&mut self, engine: &Arc<kinewright_media::FfmpegMediaEngine>) {
            self.done = Some(engine.teardown_observer());
            self.gpu = Some(engine.test_gpu());
        }
        pub(super) fn wait(&mut self) {
            if let Some(done) = self.done.take() {
                let retired = !matches!(
                    done.recv_timeout(Duration::from_secs(30)),
                    Err(crossbeam_channel::RecvTimeoutError::Timeout)
                );
                if !std::thread::panicking() {
                    assert!(retired, "C2 engine retirement deadline");
                }
            }
            if let Some(gpu) = self.gpu.take() {
                let deadline = Instant::now() + Duration::from_secs(30);
                while gpu.ledger().live_bytes() != 0 && Instant::now() < deadline {
                    let _ = gpu.device.poll(eframe::wgpu::PollType::Poll);
                    std::thread::sleep(Duration::from_millis(5));
                }
                if !std::thread::panicking() {
                    assert_eq!(gpu.ledger().live_bytes(), 0, "C2 released GPU allocations");
                }
            }
        }
    }
    impl Drop for Retire {
        fn drop(&mut self) {
            self.wait();
        }
    }
}
