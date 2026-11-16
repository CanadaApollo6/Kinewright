mod dialogue_measurement_tests {
    use super::*;
    use crate::{
        dialogue_measurements::CachedDialogueMeasurements,
        dialogue_tools::{DialogueMeasurements, MediaIdentity, Refusal},
    };
    use kinewright_core::{AssetTranscript, MediaSourceOrigin, TranscriptWord};
    use kinewright_media::{
        FfmpegMediaEngine, SessionMedia,
        test_support::{TempDirectory, wav_f32, workspace_target_dir},
    };

    struct Fixture {
        provider: Arc<CachedDialogueMeasurements>,
        handler: KinewrightMcp,
        media: Arc<SessionMedia>,
        engine: Arc<FfmpegMediaEngine>,
        assets: Vec<MediaAsset>,
        root: TempDirectory,
    }
    #[allow(
        clippy::cast_precision_loss,
        reason = "fixture sample indices are below 2^24"
    )]
    fn fixture(authored: bool) -> Fixture {
        let root = TempDirectory::new_in(&workspace_target_dir(), "dq-media-provider");
        let fps = Rational::new(100, 1).unwrap();
        let mut assets = Vec::new();
        for id in 1..=3_u32 {
            let mut pcm = vec![0_f32; 4 * 48_000];
            for (start, end) in [(200, 500), (1100, 1400), (2000, 2300), (3000, 3600)] {
                for (i, sample) in pcm.iter_mut().enumerate().take(end * 48).skip(start * 48) {
                    *sample = 0.25 * (i as f32 * 280.0 * std::f32::consts::TAU / 48_000.0).sin();
                }
            }
            let path = root.path(&format!("{id}.wav"));
            // Distinct content keys with identical decoded audio make capacity
            // and LRU witnesses independent of detector scores on different tones.
            let mut wav = wav_f32(&pcm, 48_000, 1);
            wav.extend(b"JUNK");
            wav.extend(4_u32.to_le_bytes());
            wav.extend(id.to_le_bytes());
            let riff_bytes = u32::try_from(wav.len() - 8).unwrap();
            wav[4..8].copy_from_slice(&riff_bytes.to_le_bytes());
            std::fs::write(&path, wav).unwrap();
            assets.push(MediaAsset {
                id: AssetId(u64::from(id)),
                path: path.clone(),
                name: format!("recording {id}"),
                duration: TimeCode(400),
                fps,
                kind: MediaKind::Audio,
                resolution: None,
                source_fingerprint: MediaSourceFingerprint {
                    content_sha256: Some(kinewright_media::sha256_file(&path).unwrap()),
                    byte_len: Some(std::fs::metadata(&path).unwrap().len()),
                },
                color_description: kinewright_core::ColorDescription::default(),
                assumed_from: None,
            });
        }
        let doc = Arc::new(Document {
            fps, duration: TimeCode(400), media_pool: assets.clone(),
            tracks: vec![Track { id: TrackId(1), kind: TrackKind::Audio, sync_lock: true,
                clips: vec![serde_json::from_value(json!({"id":1,"asset":1,"source_range":{"start":0,"end":400},"timeline_start":0,"effects":[],"transition_in":null})).unwrap()] }],
            ..Document::default()
        });
        let core = Core::spawn(Arc::clone(&doc)).unwrap();
        let gpu = FfmpegMediaEngine::test_gpu_context().unwrap();
        let engine =
            Arc::new(FfmpegMediaEngine::new_with_paced_test_audio(gpu, root.path("data")).unwrap());
        let media = engine.legacy_media(
            MediaSourceOrigin::new_project().legacy_media().unwrap(),
            doc,
        );
        if authored {
            for asset in &assets {
                register(&media, asset);
            }
        }
        let provider = Arc::new(CachedDialogueMeasurements::test_new(
            core.clone(),
            Arc::clone(&media),
        ));
        let handler = KinewrightMcp::new(
            core,
            media.clone(),
            media.clone(),
            ConfirmationBroker::default(),
        );
        *handler.dialogue_measurements.write().unwrap() = Some(provider.clone());
        Fixture {
            provider,
            handler,
            media,
            engine,
            assets,
            root,
        }
    }
    fn register(media: &SessionMedia, asset: &MediaAsset) {
        media
            .register_transcript(
                asset,
                AssetTranscript {
                    asset: asset.id,
                    content_sha256: asset
                        .source_fingerprint
                        .content_sha256
                        .as_ref()
                        .unwrap()
                        .into(),
                    source_fps: asset.fps,
                    words: [
                        ("Hello", 20, 50),
                        ("remove", 110, 140),
                        ("next", 200, 230),
                        ("run", 300, 330),
                        ("together", 330, 360),
                    ]
                    .into_iter()
                    .map(|(text, a, b)| TranscriptWord {
                        text: text.into(),
                        source_start: TimeCode(a),
                        source_end: TimeCode(b),
                        speaker: None,
                    })
                    .collect(),
                },
            )
            .unwrap();
    }
    fn identity(asset: &MediaAsset) -> MediaIdentity {
        MediaIdentity::of(asset).unwrap()
    }
    fn invoke(s: &KinewrightMcp, name: &str, args: serde_json::Value) -> CallToolResult {
        s.call_exposed_blocking(
            CallToolRequestParams::new("invoke_capability").with_arguments(
                serde_json::Map::from_iter([
                    ("name".into(), json!(name)),
                    ("arguments".into(), args),
                ]),
            ),
        )
        .unwrap()
    }
    fn request(binding: &str, index: usize) -> serde_json::Value {
        let address = json!({"clip":1,"index":index,"binding":binding});
        json!({"expected_revision":0,"removals":[{"kind":"words","first":address,"last":address}]})
    }
    fn ready(f: &Fixture, index: usize) -> Arc<crate::dialogue_tools::BoundDialogue> {
        let id = identity(&f.assets[index]);
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            match f.provider.get(&id, 0) {
                Ok(value) => return value,
                Err(Refusal::MeasurementPending {
                    status:
                        crate::dialogue_measurements::MeasurementStatus::Queued
                        | crate::dialogue_measurements::MeasurementStatus::Running { .. },
                }) => {}
                Err(error) => panic!("unexpected provider refusal: {error:?}"),
            }
            assert!(Instant::now() < deadline, "DQ_MEDIA_PROVIDER_JOB_BOUNDED");
            thread::yield_now();
        }
    }
    fn approved(s: &KinewrightMcp, args: serde_json::Value) -> CallToolResult {
        thread::scope(|scope| {
            let worker = scope.spawn(|| invoke(s, "cut_dialogue", args));
            let deadline = Instant::now() + Duration::from_secs(30);
            while !worker.is_finished() {
                for request in s.confirmations.pending_requests() {
                    let _ = s.confirmations.approve(request.id);
                }
                assert!(Instant::now() < deadline, "DQ_MEDIA_PROVIDER_CUT_BOUNDED");
                thread::yield_now();
            }
            worker.join().unwrap()
        })
    }
    fn assert_check_report(
        service: &KinewrightMcp,
        value: &crate::dialogue_tools::BoundDialogue,
        checked: &serde_json::Value,
    ) {
        use kinewright_core::dialogue::audit::{
            self, AuditSettings, Detail, KeptRange, RecordingId,
        };
        assert_eq!(
            checked["segments"][0]["report"]["hole_basis"], "approximate_transcript_intervals",
            "DQ_WORD_END_DISPATCHED_HOLE_BASIS"
        );
        let document = service.snapshot().unwrap().1;
        let kept = crate::dialogue_tools::spans(&document)
            .unwrap()
            .into_iter()
            .map(|span| KeptRange {
                recording: RecordingId(span.asset.0),
                source: span.source,
            })
            .collect::<Vec<_>>();
        let direct = audit::audit(
            &BTreeMap::from([(RecordingId(1), value.dialogue())]),
            &kept,
            &AuditSettings {
                detail: Detail::All,
                ..AuditSettings::default()
            },
            audit::comparison_key,
        )
        .unwrap();
        assert_eq!(
            checked["segments"][0]["report"],
            serde_json::to_value(direct).unwrap(),
            "DQ_MEDIA_REAL_CHECK_CORE_REPORT"
        );
    }
    fn assert_transcript_timing(service: &KinewrightMcp) -> CallToolResult {
        let words = invoke(service, "get_timeline_transcript", json!({}));
        assert_eq!(
            words.structured_content.as_ref().unwrap()["word_timing"],
            "approximate_transcript_intervals",
            "DQ_WORD_END_DISPATCHED_TRANSCRIPT_BASIS"
        );
        let ranged = invoke(
            service,
            "get_timeline_transcript",
            json!({"word_range":{"clip":1,"first":0,"end":1}}),
        );
        assert_eq!(
            ranged.structured_content.as_ref().unwrap()["word_timing"],
            "approximate_transcript_intervals",
            "DQ_WORD_END_RANGED_TRANSCRIPT_BASIS"
        );
        words
    }
    #[test]
    fn dq_media_provider_dispatches_pending_then_measured_cut_and_check() {
        let f = fixture(true);
        let hold = f.engine.test_hold_transcript_inference();
        let mut blocker = f.assets[1].clone();
        blocker.path = f.root.path("blocked.wav");
        std::fs::copy(&f.assets[1].path, &blocker.path).unwrap();
        f.media.request_transcription(blocker);
        hold.wait();
        let cut = invoke(&f.handler, "cut_dialogue", request("unread", 1));
        assert_eq!(
            cut.structured_content.as_ref().unwrap()["reason"],
            json!({"code":"measurement_pending","status":{"state":"queued"}}),
            "DQ_MEDIA_PENDING_CUT_STATUS"
        );
        assert_eq!(
            serde_json::to_value(&cut).unwrap()["structuredContent"]["reason"]["status"],
            json!({"state":"queued"}),
            "DQ_MEDIA_PENDING_CUT_WIRE_STATUS"
        );
        let check = invoke(&f.handler, "check_dialogue_cut", json!({"track_id":1}));
        assert_eq!(
            check.structured_content.as_ref().unwrap()["check"]["not_checked"][0],
            json!({"clip":1,"range":[0,400],"code":"measurement_pending","status":{"state":"queued"}}),
            "DQ_MEDIA_PENDING_CHECK_STATUS"
        );
        assert_eq!(
            serde_json::to_value(&check).unwrap()["structuredContent"]["check"]["not_checked"][0]["status"],
            json!({"state":"queued"}),
            "DQ_MEDIA_PENDING_CHECK_WIRE_STATUS"
        );
        drop(hold);
        let value = ready(&f, 0);
        assert!(
            value
                .dialogue()
                .profile()
                .hop(330)
                .unwrap()
                .rms_dbfs_hundredths
                > -2000,
            "DQ_MEDIA_REAL_BURST_LEVEL"
        );
        assert!(
            value
                .dialogue()
                .profile()
                .hop(80)
                .unwrap()
                .rms_dbfs_hundredths
                <= -8000,
            "DQ_MEDIA_REAL_QUIET_LEVEL"
        );
        let words = assert_transcript_timing(&f.handler);
        assert!(
            words
                .structured_content
                .as_ref()
                .unwrap()
                .to_string()
                .contains(value.binding())
        );
        let unsafe_cut = invoke(&f.handler, "cut_dialogue", request(value.binding(), 3));
        assert_eq!(
            unsafe_cut.is_error,
            Some(true),
            "DQ_MEDIA_REAL_BURST_REFUSED"
        );
        assert_eq!(f.handler.snapshot().unwrap().0, TimelineRevision(0));
        let cut = approved(&f.handler, request(value.binding(), 1));
        assert_ne!(cut.is_error, Some(true), "{cut:?}");
        assert_eq!(
            cut.structured_content.as_ref().unwrap()["status"],
            "applied",
            "DQ_MEDIA_REAL_QUIET_CUT"
        );
        let check = invoke(
            &f.handler,
            "check_dialogue_cut",
            json!({"track_id":1,"detail":"all"}),
        );
        let checked = &check.structured_content.as_ref().unwrap()["check"];
        assert_ne!(check.is_error, Some(true));
        assert!(checked["not_checked"].is_null(), "{checked}");
        assert_eq!(
            checked["recordings"]["1"]["binding"],
            value.binding(),
            "DQ_MEDIA_REAL_CHECK_BINDING"
        );
        assert!(
            !checked["segments"].as_array().unwrap().is_empty(),
            "DQ_MEDIA_REAL_CHECK_REPORT"
        );
        assert_check_report(&f.handler, &value, checked);
    }
    #[test]
    fn dq_media_provider_eviction_rebuilds_binding_and_preserves_live_values() {
        let f = fixture(true);
        let one = ready(&f, 0);
        let binding = one.binding().to_owned();
        let cost = f.provider.test_resident_limit(None);
        f.provider.test_resident_limit(Some(cost));
        let id = identity(&f.assets[1]);
        let deadline = Instant::now() + Duration::from_secs(30);
        // Queue and finish a second actual profile while the first bound value is leased.
        loop {
            assert!(Instant::now() < deadline, "DQ_MEDIA_PINNED_REQUEST_BOUNDED");
            match f.provider.get(&id, 0) {
                Err(Refusal::MeasurementPending {
                    status:
                        crate::dialogue_measurements::MeasurementStatus::Queued
                        | crate::dialogue_measurements::MeasurementStatus::Running { .. },
                }) => thread::yield_now(),
                Err(Refusal::MeasurementPending {
                    status: crate::dialogue_measurements::MeasurementStatus::Failed { reason },
                }) => {
                    assert!(reason.contains("busy"), "{reason}");
                    break;
                }
                _ => panic!("DQ_MEDIA_PINNED_VALUE_MUST_NOT_BE_EVICTED"),
            }
        }
        assert_eq!(one.binding(), binding);
        drop(one);
        drop(ready(&f, 1));
        let rebuilt = ready(&f, 0);
        assert_eq!(
            rebuilt.binding(),
            binding,
            "DQ_MEDIA_EVICTED_BINDING_STABLE"
        );
        assert!(f.provider.test_resident_limit(None) <= cost);
    }
    #[test]
    fn dq_media_provider_missing_words_requests_without_inline_inference() {
        let f = fixture(false);
        let hold = f.engine.test_hold_transcript_inference();
        let result = f.provider.get(&identity(&f.assets[0]), 0);
        assert!(
            matches!(result, Err(Refusal::MeasurementPending { .. })),
            "DQ_MEDIA_MISSING_WORDS_REQUESTED"
        );
        hold.wait();
        assert!(f.media.transcript_status(&f.assets[0]).is_running());
        drop(hold);
        assert!(
            !f.engine.data_dir().join("models").exists(),
            "DQ_MEDIA_NO_MODEL_DOWNLOAD"
        );
    }
    #[test]
    fn dq_media_provider_lru_evicts_the_least_recent_unleased_value() {
        let f = fixture(true);
        let one = ready(&f, 0);
        let weak_one = Arc::downgrade(&one);
        let cost = f.provider.test_resident_limit(None);
        drop(one);
        f.provider.test_resident_limit(Some(cost * 2));
        let two = ready(&f, 1);
        let weak_two = Arc::downgrade(&two);
        drop(two);
        drop(ready(&f, 0));
        drop(ready(&f, 2));
        assert!(weak_one.upgrade().is_some(), "DQ_MEDIA_LRU_RECENT_SURVIVES");
        assert!(weak_two.upgrade().is_none(), "DQ_MEDIA_LRU_OLDEST_EVICTED");
        assert!(f.provider.test_resident_limit(None) <= cost * 2);
    }
    #[test]
    fn dq_media_provider_cached_profile_missing_words_requests_only_recognition() {
        let f = fixture(true);
        let measured = ready(&f, 0);
        let root = f.engine.data_dir();
        let key = kinewright_media::sound_profile::ProfileIdentity::new(
            &f.assets[0].source_fingerprint,
            0,
        )
        .unwrap();
        let cache = kinewright_media::sound_profile::SoundProfileCache::new(root).unwrap();
        let before = cache.load(&key).unwrap();
        // A second session has no authored words, but shares the source profile cache.
        let doc = f.handler.snapshot().unwrap().1;
        let media = f.engine.legacy_media(
            MediaSourceOrigin::new_project().legacy_media().unwrap(),
            doc,
        );
        let provider =
            CachedDialogueMeasurements::test_new(f.handler.core.clone(), Arc::clone(&media));
        let hold = f.engine.test_hold_transcript_inference();
        assert!(matches!(
            provider.get(&identity(&f.assets[0]), 0),
            Err(Refusal::MeasurementPending { .. })
        ));
        hold.wait();
        assert!(media.transcript_status(&f.assets[0]).is_running());
        assert_eq!(
            cache.load(&key).unwrap(),
            before,
            "DQ_MEDIA_PROFILE_NOT_REMEASURED_FOR_WORDS"
        );
        assert_eq!(measured.recogniser_key(), "authored-source-frames-v1");
        drop(hold);
    }
    #[test]
    fn dq_media_provider_changed_words_change_binding_without_remeasuring() {
        let f = fixture(true);
        let before = ready(&f, 0);
        let TranscriptStatus::Ready(words) = f.media.transcript_status(&f.assets[0]) else {
            panic!("authored words")
        };
        let mut replacement = (*words).clone();
        replacement.words[0].text = "Changed".into();
        f.media
            .register_transcript(&f.assets[0], replacement)
            .unwrap();
        let after = f.provider.get(&identity(&f.assets[0]), 0).unwrap();
        assert_ne!(
            after.binding(),
            before.binding(),
            "DQ_MEDIA_UPDATED_WORDS_REBIND"
        );
        assert_eq!(
            before.dialogue().words()[0].text,
            "Hello",
            "DQ_MEDIA_OLD_BINDING_IMMUTABLE"
        );
        assert_eq!(after.dialogue().profile(), before.dialogue().profile());
        assert!(matches!(
            f.provider.get(&identity(&f.assets[0]), 9),
            Err(Refusal::Stream)
        ));
        assert!(matches!(
            f.provider.get(
                &MediaIdentity {
                    sha256: "f".repeat(64),
                    byte_length: 10
                },
                0
            ),
            Err(Refusal::Identity)
        ));
    }
    #[test]
    fn dq_media_provider_endpoints_share_one_resident_value() {
        let f = fixture(true);
        drop(ready(&f, 0));
        let left = CachedDialogueMeasurements::new(f.handler.core.clone(), Arc::clone(&f.media));
        let right = CachedDialogueMeasurements::new(f.handler.core.clone(), Arc::clone(&f.media));
        let one = left.get(&identity(&f.assets[0]), 0).unwrap();
        let two = right.get(&identity(&f.assets[0]), 0).unwrap();
        assert!(
            Arc::ptr_eq(&one, &two),
            "DQ_MEDIA_ENDPOINTS_SHARE_RESIDENT_BUDGET"
        );
    }
}
