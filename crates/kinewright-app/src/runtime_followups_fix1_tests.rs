//! Real queue admission across superseded prompts and incomplete closes.
use super::*;
use kinewright_agent::export_queue::{ExportJobRecord, ExportQueueError};
use kinewright_project::{editing_session::Ja1EditingSession, storage_budget::StorageLimits};

fn next_export(export: &ExportWitness, name: &str) -> Result<ExportJobRecord, ExportQueueError> {
    export.queue.enqueue(
        &renderable_document(10),
        QueueExportRequest {
            output_path: export.directory.path(name),
            profile: DeliveryProfile::SourceMaster,
            focus_x_percent: 50,
            focus_y_percent: 50,
            overwrite: false,
            verify: false,
            delivery_bit_depth: DeliveryEncodeDepth::default(),
            normalize_loudness: false,
        },
    )
}
fn complete(export: &ExportWitness) {
    export.gate.release();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !export.queue.get(export.job).unwrap().state.is_terminal() {
        assert!(Instant::now() < deadline, "RUNTIME_FIX1_EXPORT_COMPLETES");
        std::thread::yield_now();
    }
}

#[test]
fn runtime_fix1_replaced_export_prompts_resume_every_open_project() {
    for quit in [false, true] {
        for dismiss in [false, true] {
            let (mut app, _) = in1_tests::in1_harness(Document::default());
            app.new_project();
            let first = ExportWitness::install_at(&mut app, 0);
            let second = ExportWitness::install_at(&mut app, 1);
            for project in &mut app.projects {
                project.saved_document = Some(Arc::clone(&project.document));
            }
            let ctx = egui::Context::default();
            if quit {
                app.request_exit(&ctx);
            } else {
                app.request_close_project(0);
            }
            assert!(app.pending_export_close.is_some());
            app.request_close_project(1);
            assert!(
                matches!(app.pending_export_close, Some(ProjectAction::CloseProject(id)) if id == app.projects[1].id)
            );
            if dismiss {
                complete(&second);
                app.show_export_close_confirmation(&ctx);
            } else {
                app.export_close_choice(&ctx, false);
            }
            let first_cancelled = first.cancelled();
            let second_cancelled = second.cancelled();
            let first_accepts = next_export(&first, "again-a.mp4");
            let second_accepts = next_export(&second, "again-b.mp4");
            // Always release both native gates before any mutation assertion.
            first.gate.release();
            second.gate.release();
            first.queue.join();
            second.queue.join();
            assert!(
                first_accepts.is_ok() && second_accepts.is_ok(),
                "RUNTIME_FIX1_REPLACED_SCOPE quit={quit} dismiss={dismiss}: {first_accepts:?}; {second_accepts:?}"
            );
            assert!(!first_cancelled && !second_cancelled);
            assert_eq!(app.projects.len(), 2);
            assert!(app.pending_export_close.is_none());
            assert!(!app.allow_close);
            assert_eq!(
                first.queue.get(first.job).unwrap().state,
                ExportJobState::Completed
            );
            assert_eq!(
                second.queue.get(second.job).unwrap().state,
                ExportJobState::Completed
            );
        }
    }
}

#[test]
fn runtime_fix1_pending_source_close_has_typed_admission_until_removal() {
    let (mut app, _) = in1_tests::in1_harness(Document::default());
    let export = ExportWitness::install(&mut app);
    let directory = TempDirectory::new_in(&workspace_target_dir(), "runtime-fix1-source");
    let owner = Ja1EditingSession::new(
        directory.root(),
        &app.projects[0].core,
        StorageLimits::default(),
    )
    .unwrap();
    let mut attachment = crate::ja1_attachment::Ja1Attachment::new(owner).unwrap();
    let transfer = attachment.owner.prepare_host_attachment().unwrap();
    let service = transfer.source_service.unwrap();
    let reader = service
        .open_borrowed_source(&transfer.source.unwrap(), &transfer.inputs)
        .unwrap();
    app.projects[0].ja1 = Some(attachment);
    app.new_project();
    let id = app.projects[0].id;
    let ctx = egui::Context::default();
    app.close_project(id);
    assert!(app.pending_export_close.is_some());
    app.export_close_choice(&ctx, true);
    let refused = next_export(&export, "pending.mp4").unwrap_err();
    export.queue.resume_admission(); // A different prompt cannot reopen a real close.
    let still_refused = next_export(&export, "still-pending.mp4").unwrap_err();
    let cancelled = export.cancelled();
    let kept_open = app.projects.iter().any(|project| project.id == id);
    let message = app.status.clone();
    drop(reader);
    complete(&export);
    app.close_project(id);
    export.queue.resume_admission();
    let after_removal = next_export(&export, "removed.mp4").unwrap_err();
    export.queue.join();
    assert!(
        matches!(refused, ExportQueueError::ProjectClosing),
        "RUNTIME_FIX1_PENDING_CLOSE_REASON: {refused}"
    );
    assert_eq!(
        refused.to_string(),
        "This project is closing. Finish or cancel the close to export."
    );
    assert!(matches!(still_refused, ExportQueueError::ProjectClosing));
    assert!(
        kept_open && (message.contains("readers") || message.contains("cleanup")),
        "{message}"
    );
    assert!(!cancelled, "RUNTIME_FIX1_PENDING_CLOSE_KEEPS_EXPORT");
    assert!(!app.projects.iter().any(|project| project.id == id));
    assert!(matches!(after_removal, ExportQueueError::WorkerUnavailable));
    assert_eq!(
        export.queue.get(export.job).unwrap().state,
        ExportJobState::Completed
    );
}

#[test]
fn runtime_fix1_refused_cancel_close_keeps_export_and_reopens_admission() {
    for quit in [false, true] {
        let (mut app, _) = in1_tests::in1_harness(Document::default());
        let export = ExportWitness::install(&mut app);
        app.new_project();
        for project in &mut app.projects {
            project.saved_document = Some(Arc::clone(&project.document));
        }
        let ctx = egui::Context::default();
        if quit {
            app.request_exit(&ctx);
        } else {
            app.request_close_project(0);
        }
        assert!(app.pending_export_close.is_some());
        let directory = TempDirectory::new_in(&workspace_target_dir(), "runtime-fix1-save");
        let root = directory.root().to_path_buf();
        let core = app.projects[0].core.clone();
        let gate = Arc::new(Gate::default());
        let saving = Arc::clone(&gate);
        app.projects[0].ja1_job = Some(std::thread::spawn(move || {
            saving.hold();
            let owner = Ja1EditingSession::new(&root, &core, StorageLimits::default()).unwrap();
            crate::ja1_attachment::Ja1Attachment::new(owner).unwrap()
        }));
        gate.entered();
        app.export_close_choice(&ctx, true);
        let accepts = next_export(&export, "still-open.mp4");
        let cancelled = export.cancelled();
        let open = app.projects.len() == 2 && !app.allow_close;
        let message = app.status.clone();
        gate.release();
        drop(app.projects[0].ja1_job.take().unwrap().join().unwrap());
        export.gate.release();
        export.queue.join();
        assert!(
            accepts.is_ok(),
            "RUNTIME_FIX1_REFUSED_CLOSE_REOPENS: {accepts:?}"
        );
        assert!(
            open && !cancelled,
            "RUNTIME_FIX1_REFUSED_CLOSE_NO_CANCEL quit={quit}"
        );
        assert!(
            message.contains("Wait for")
                && (message.contains("save") || message.contains("Studio")),
            "{message}"
        );
        assert_eq!(
            export.queue.get(export.job).unwrap().state,
            ExportJobState::Completed
        );
    }
}

#[test]
fn runtime_fix1_voice_refusal_precedes_export_cancellation() {
    use crate::recording::{
        FeedbackInput,
        feedback_capture::{Capture, CaptureReservation},
    };
    use kinewright_project::studio_records::FeedbackAnchor;
    for quit in [false, true] {
        let mut fixture = crate::studio::tests::fixtures();
        let context = fixture.contexts.remove(0);
        let (mut app, _) = in1_tests::in1_harness(Document::default());
        app.projects[0].ja1 =
            Some(crate::ja1_attachment::Ja1Attachment::new(fixture.owners.remove(0)).unwrap());
        app.projects[0].install_ja1_view();
        let export = ExportWitness::install(&mut app);
        app.new_project();
        for project in &mut app.projects {
            project.saved_document = Some(Arc::clone(&project.document));
        }
        let id = app.projects[0].id;
        let ctx = egui::Context::default();
        if quit {
            app.exit_discarded_projects = app.projects.iter().map(|project| project.id).collect();
            app.request_exit(&ctx);
        } else {
            app.close_project(id);
        }
        assert!(app.pending_export_close.is_some());
        let source = fixture.root.path("close-voice.wav");
        kinewright_media::test_support::run_ffmpeg(
            &[
                "-f",
                "lavfi",
                "-i",
                "anullsrc=r=16000:cl=mono",
                "-t",
                "0.05",
                "-c:a",
                "pcm_s16le",
            ],
            &source,
        );
        let anchor = FeedbackAnchor {
            snapshot: context.snapshot.descriptor.key.clone(),
            frame: 0,
            fps: FrameRate {
                numerator: context.document.fps.numerator(),
                denominator: context.document.fps.denominator(),
            },
            raster_size: [context.document.resolution.0, context.document.resolution.1],
            orientation: Orientation::Upright,
            target: None,
            source_ref: None,
            script_beat_id: None,
        };
        let capture = Capture::start_input(
            CaptureReservation::reserve_context(&context).unwrap(),
            FeedbackInput::Wav(source),
            fixture.root.path("capture"),
            anchor,
            context.pin_snapshot().unwrap(),
        )
        .unwrap();
        // Keep the real recording state until the close answer; no voice-frame polling races it.
        app.projects[0].studio.voice = crate::studio_voice::Voice::recording_fixture(
            capture,
            context.capture_project_id().unwrap(),
        );
        app.export_close_choice(&ctx, true);
        let accepts = next_export(&export, "after-voice-refusal.mp4");
        let cancelled = export.cancelled();
        let voice_kept = app.projects[0].studio.voice.unkept().is_some();
        let open = app.projects.len() == 2 && !app.allow_close;
        let message = app.status.clone();
        let voice = std::mem::take(&mut app.projects[0].studio.voice);
        drop(voice);
        export.gate.release();
        export.queue.join();
        assert!(
            accepts.is_ok() && !cancelled && voice_kept && open,
            "RUNTIME_FIX1_VOICE_BEFORE_CANCEL quit={quit}: {accepts:?}"
        );
        assert_eq!(message, crate::studio_voice::RECORDING_REFUSAL);
        assert_eq!(
            export.queue.get(export.job).unwrap().state,
            ExportJobState::Completed
        );
    }
}
