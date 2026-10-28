//! Activation revokes queued locator jobs and waits for in-flight leases.
#[cfg(test)]
mod tests {
    use super::super::*;
    use crate::app::in1_tests::{in1_harness_with_engine, in1_shutdown};
    use kinewright_core::{MediaKind, SceneStatus, TranscriptStatus, VisualAssetResult};
    use kinewright_media::test_support::{GeneratedMedia, TempDirectory, workspace_target_dir};

    // Keep generated sources alive until the preview/GPU worker has retired,
    // including when the security assertion unwinds under a mutation.
    struct MediaFixture {
        engine: Option<Arc<FfmpegMediaEngine>>,
        retired: crossbeam_channel::Receiver<()>,
    }
    impl MediaFixture {
        fn new(engine: FfmpegMediaEngine) -> Self {
            Self {
                retired: engine.teardown_observer(),
                engine: Some(Arc::new(engine)),
            }
        }
        fn shared(&self) -> Arc<FfmpegMediaEngine> {
            Arc::clone(self.engine.as_ref().expect("live fixture engine"))
        }
    }
    impl Drop for MediaFixture {
        fn drop(&mut self) {
            drop(self.engine.take());
            let deadline = Instant::now() + Duration::from_secs(30);
            while !matches!(
                self.retired.try_recv(),
                Err(crossbeam_channel::TryRecvError::Disconnected)
            ) {
                assert!(Instant::now() < deadline, "media worker did not retire");
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }

    #[test]
    fn c2_r3_activation_refuses_queued_legacy_transcript_scene_and_thumbnail() {
        let root = TempDirectory::new_in(&workspace_target_dir(), "c2-r3-queued");
        let generate = |color| {
            GeneratedMedia::ffmpeg(
                "c2-r3",
                &[
                    "-f",
                    "lavfi",
                    "-i",
                    color,
                    "-f",
                    "lavfi",
                    "-i",
                    "sine=frequency=440:sample_rate=16000",
                    "-t",
                    "0.2",
                    "-c:v",
                    "mpeg4",
                    "-c:a",
                    "pcm_s16le",
                    "-shortest",
                ],
                "avi",
            )
        };
        let red = generate("color=red:s=64x64:r=30");
        let blue = generate("color=blue:s=64x64:r=30");
        let data = root.path("data");
        std::fs::create_dir_all(&data).unwrap();
        // A mutation must fail locally rather than ever downloading a model.
        std::fs::write(data.join("models"), b"no model downloads in this witness").unwrap();
        let fixture = MediaFixture::new(FfmpegMediaEngine::new_with_data_dir(data).unwrap());
        let origin = kinewright_core::MediaSourceOrigin::new_project();
        let probe = fixture.shared().legacy_media(
            origin.legacy_media().unwrap(),
            Arc::new(Document::default()),
        );
        let asset = probe.probe(red.path()).unwrap();
        assert_eq!(asset.kind, MediaKind::AudioVideo);
        let mut document = Document::default();
        document.media_pool.push(asset.clone());
        let (mut app, engine) = in1_harness_with_engine(document, fixture.shared());
        let grant = app.focused().legacy_media().unwrap();
        let retained = engine.legacy_media(grant.clone(), Arc::clone(&app.focused().document));
        let hold = grant.hold_work();
        let analysis = app.focused().analysis();
        let results = analysis.visual_asset_results();
        analysis.request_transcription(asset.clone());
        analysis.request_scene_detection(asset.clone());
        assert!(analysis.request_thumbnail(asset.clone(), TimeCode::ZERO, 32, 702));
        hold.wait(3);
        app.enable_ja1();
        assert!(app.focused().ja1.is_some(), "{}", app.status);
        std::fs::copy(blue.path(), red.path()).unwrap();
        drop(hold);
        let visual = results.recv_timeout(Duration::from_secs(30)).unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        let statuses = loop {
            let statuses = retained.test_background_statuses(&asset.path);
            if matches!(
                statuses.0,
                TranscriptStatus::Failed(_)
                    | TranscriptStatus::NoSpeech
                    | TranscriptStatus::Ready(_)
            ) && matches!(statuses.1, SceneStatus::Failed(_) | SceneStatus::Ready(_))
            {
                break statuses;
            }
            assert!(
                Instant::now() < deadline,
                "background workers did not finish: {statuses:?}"
            );
            std::thread::sleep(Duration::from_millis(5));
        };
        in1_shutdown(&mut app);
        let pixel = match &visual {
            VisualAssetResult::Thumbnail(frame) => frame.image.pixels.get(..4),
            _ => None,
        };
        assert_eq!(
            grant.locator_opens(),
            0,
            "C2_R3_RETAINED_AUTHORITY_NO_LOCATOR_OPEN: pixel={pixel:?}; {statuses:?}"
        );
        let refusal = MediaError::MediaAdmissionRequired.to_string();
        assert!(
            matches!(visual, VisualAssetResult::Failed { message, .. } if message == refusal),
            "C2_R3_THUMBNAIL_NAMED_REFUSAL"
        );
        assert!(
            matches!(statuses.0, TranscriptStatus::Failed(message) if message == refusal),
            "C2_R3_TRANSCRIPT_NAMED_REFUSAL"
        );
        assert!(
            matches!(statuses.1, SceneStatus::Failed(message) if message == refusal),
            "C2_R3_SCENE_NAMED_REFUSAL"
        );
    }

    #[test]
    fn c2_r3_activation_waits_for_in_flight_legacy_work_before_publication() {
        let fixture = MediaFixture::new(FfmpegMediaEngine::new().unwrap());
        let (mut app, _) = in1_harness_with_engine(Document::default(), fixture.shared());
        app.new_project();
        app.focus_project(0);
        let grant = app.focused().legacy_media().unwrap();
        let running = grant.begin().unwrap();
        app.enable_ja1();
        assert!(grant.check().is_err());
        assert!(app.focused().ja1.is_none());
        assert!(!app.focused().is_owned());
        assert!(app.focused().ja1_activation.is_some());
        assert!(app.focused().ja1_read_only());
        assert!(
            !app.save_project(false),
            "pending activation cannot invoke a legacy save"
        );
        assert!(!app.ja1_destination_change());
        let root = TempDirectory::new_in(&workspace_target_dir(), "c2-r3-pending-save");
        let destination = root.path("pending.kinewright");
        assert!(app.write_project(&destination).is_err());
        assert!(!destination.exists());
        let ctx = egui::Context::default();
        app.poll_ja1(&ctx);
        assert!(
            app.focused().ja1.is_none(),
            "C2_R3_IN_FLIGHT_WORK_NOT_DRAINED"
        );
        app.focus_project(1);
        assert_eq!(app.focused_project, 1);
        drop(running);
        app.poll_ja1(&ctx);
        assert!(app.projects[0].ja1.is_some(), "{}", app.status);
        assert!(app.projects[0].ja1_activation.is_none());
        assert_eq!(app.focused_project, 1);
        assert!(
            !app.focused().is_owned(),
            "activation must stay with its originating session"
        );
        in1_shutdown(&mut app);
    }
}
