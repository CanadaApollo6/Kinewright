//! Cross-project preview authority and activation cancellation witnesses.
#[cfg(test)]
mod tests {
    use super::super::*;
    use crate::app::in1_tests::{in1_harness_with_engine, in1_shutdown};
    use kinewright_core::{ColorContext, MediaSourceOrigin};
    use kinewright_media::test_support::{
        GeneratedMedia, TempDirectory, single_clip_document, workspace_target_dir,
    };

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
            Arc::clone(self.engine.as_ref().unwrap())
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

    fn video(color: &str) -> GeneratedMedia {
        GeneratedMedia::ffmpeg(
            "c2-r4-preview",
            &[
                "-f", "lavfi", "-i", color, "-t", "2", "-c:v", "mpeg4", "-g", "1",
            ],
            "avi",
        )
    }

    fn wait_frame(playback: &dyn Playback, at: TimeCode) -> kinewright_core::PreviewFrame {
        let frames = playback.frames();
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let frame = frames
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .expect("preview frame");
            if frame.at == at {
                return frame;
            }
        }
    }

    #[test]
    fn c2_r4_equal_sources_never_reuse_another_projects_reader_authority() {
        let red = video("color=red:s=64x64:r=30");
        let blue = video("color=blue:s=64x64:r=30");
        let fixture = MediaFixture::new(FfmpegMediaEngine::new().unwrap());
        let origin = MediaSourceOrigin::new_project();
        let probe = fixture.shared().legacy_media(
            origin.legacy_media().unwrap(),
            Arc::new(Document::default()),
        );
        let mut asset = probe.probe(red.path()).unwrap();
        asset.color_description = ColorContext::sdr_rec709().delivery;
        let document = single_clip_document(asset);
        let (mut app, engine) = in1_harness_with_engine(document.clone(), fixture.shared());
        let grant_a = app.focused().legacy_media().unwrap();
        let a = app.focused().media().playback;
        a.set_document(Arc::new(document.clone()));
        a.request_frame(TimeCode::ZERO);
        let warm = wait_frame(&*a, TimeCode::ZERO);
        assert!(
            warm.texture.rgba[0] > warm.texture.rgba[2],
            "A renders red source pixels"
        );
        app.projects.push(
            ProjectSession::create(
                2,
                "B",
                document.clone(),
                kinewright_project::bundle::BundleWriteAdmission::new_document(),
                None,
                crate::media_access::MediaBackend::Engine(fixture.shared()),
                &SidecarMode::None,
                None,
                PROJECT_FORMAT_VERSION,
                None,
            )
            .unwrap(),
        );
        app.focus_project(1);
        assert!(app.focused().project_path.is_none(), "B is unsaved");
        let grant_b = app.focused().legacy_media().unwrap();
        let b = app.focused().media().playback;
        b.set_document(Arc::new(document));
        b.request_frame(TimeCode::ZERO);
        wait_frame(&*b, TimeCode::ZERO);
        let hold = engine.test_hold_preview_decode(TimeCode(30));
        b.request_frame(TimeCode(30));
        hold.wait();
        let opens = (grant_a.locator_opens(), grant_b.locator_opens());
        app.enable_ja1();
        let waited = app.focused().ja1.is_none() && app.focused().ja1_activation.is_some();
        std::fs::copy(blue.path(), red.path()).unwrap();
        drop(hold);
        let ctx = egui::Context::default();
        let deadline = Instant::now() + Duration::from_secs(30);
        while app.focused().ja1.is_none() {
            app.poll_ja1(&ctx);
            assert!(
                Instant::now() < deadline,
                "activation did not drain readers: {}",
                app.status
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        let after = (grant_a.locator_opens(), grant_b.locator_opens());
        assert!(
            grant_b.is_quiescent(),
            "B readers have joined before publication"
        );
        app.focus_project(0);
        a.set_document(Arc::clone(&app.focused().document));
        a.request_frame(TimeCode(45));
        let control = wait_frame(&*a, TimeCode(45));
        let a_live = grant_a.check().is_ok() && control.texture.rgba[2] > control.texture.rgba[0];
        in1_shutdown(&mut app);
        assert!(
            waited,
            "C2_R4_REQUESTING_AUTHORITY_DRAINS_BEFORE_PUBLICATION: opens={opens:?}, after={after:?}"
        );
        assert_eq!(
            opens, after,
            "C2_R4_NO_LOCATOR_OPEN_UNDER_EITHER_GRANT_AFTER_REVOCATION"
        );
        assert!(
            a_live,
            "A retains legacy playback and decodes the changed blue source"
        );
    }

    #[test]
    fn c2_r4_activation_cancels_a_held_transcript_without_waiting_for_inference() {
        let root = TempDirectory::new_in(&workspace_target_dir(), "c2-r4-inference");
        let source = GeneratedMedia::ffmpeg(
            "c2-r4-transcript",
            &[
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:sample_rate=16000",
                "-t",
                "0.2",
                "-c:a",
                "pcm_s16le",
            ],
            "wav",
        );
        let fixture =
            MediaFixture::new(FfmpegMediaEngine::new_with_data_dir(root.path("data")).unwrap());
        let origin = MediaSourceOrigin::new_project();
        let probe = fixture.shared().legacy_media(
            origin.legacy_media().unwrap(),
            Arc::new(Document::default()),
        );
        let asset = probe.probe(source.path()).unwrap();
        let mut document = Document::default();
        document.media_pool.push(asset.clone());
        let (mut app, engine) = in1_harness_with_engine(document, fixture.shared());
        let grant = app.focused().legacy_media().unwrap();
        let retained = engine.legacy_media(grant.clone(), Arc::clone(&app.focused().document));
        let hold = engine.test_hold_transcript_inference();
        app.focused()
            .analysis()
            .request_transcription(asset.clone());
        hold.wait();
        assert!(
            !grant.is_quiescent(),
            "the real transcript job owns a work lease"
        );
        app.enable_ja1();
        let ctx = egui::Context::default();
        let deadline = Instant::now() + Duration::from_secs(10);
        while app.focused().ja1.is_none() {
            assert!(
                !app.save_project(false),
                "pending activation must not save legacy data"
            );
            app.poll_ja1(&ctx);
            assert!(
                Instant::now() < deadline,
                "C2_R4_ACTIVATION_MUST_CANCEL_TRANSCRIPT_INFERENCE"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(matches!(
            retained.test_background_statuses(&asset.path).0,
            kinewright_core::TranscriptStatus::Cancelled
        ));
        assert!(
            grant.is_quiescent(),
            "cancellation drained the job while the inference barrier remains held"
        );
        drop(hold);
        in1_shutdown(&mut app);
    }
}
