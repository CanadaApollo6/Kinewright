mod dialogue_tool_tests {
    use super::*;
    use crate::dialogue_tools::{
        self as dq, BoundDialogue, DialogueMeasurements, MediaIdentity, Refusal,
    };
    use kinewright_core::dialogue::audit::{self, AuditSettings, Detail, KeptRange, RecordingId};
    use kinewright_core::dialogue::{AudibleSide, Hop, Settings, SoundProfile, Word};
    use serde_json::Value;

    struct Fake {
        value: Arc<BoundDialogue>,
        stream: u32,
    }
    impl DialogueMeasurements for Fake {
        fn audio_stream(&self, _: &MediaIdentity) -> Result<u32, Refusal> {
            Ok(self.stream)
        }
        fn get(&self, _: &MediaIdentity, _: u32) -> Result<Arc<BoundDialogue>, Refusal> {
            Ok(Arc::clone(&self.value))
        }
    }
    struct Missing;
    impl DialogueMeasurements for Missing {
        fn audio_stream(&self, _: &MediaIdentity) -> Result<u32, Refusal> {
            Err(Refusal::NotMeasured)
        }
        fn get(&self, _: &MediaIdentity, _: u32) -> Result<Arc<BoundDialogue>, Refusal> {
            Err(Refusal::NotMeasured)
        }
    }

    fn value(words: Vec<Word>, duration: u64, read: impl Fn(u64) -> Hop) -> Arc<BoundDialogue> {
        let mut hops: Vec<_> = (0..duration.div_ceil(10_000))
            .map(|i| read(i * 10_000 + 5_000))
            .collect();
        // Legacy synthetic ambience fixtures carry a reference speech hop.
        // Calibration and missing-population regressions build profiles directly.
        if !hops.iter().any(|h| h.voice) {
            hops[0] = Hop {
                rms_dbfs_hundredths: -2_000,
                peak_dbfs_hundredths: -2_000,
                voice: true,
            };
        }
        Arc::new(
            BoundDialogue::new(
                MediaIdentity {
                    sha256: "a".repeat(64),
                    byte_length: 100,
                },
                0,
                "literal-v1".into(),
                SoundProfile::from_hops(10_000, duration, hops).unwrap(),
                words,
                Settings::default(),
            )
            .unwrap(),
        )
    }
    fn said() -> Vec<Word> {
        [
            ("Hello", 200_000, 500_000),
            ("remove", 1_100_000, 1_400_000),
            ("next", 2_000_000, 2_300_000),
            ("last", 3_000_000, 3_300_000),
        ]
        .into_iter()
        .map(|(text, start_micros, end_micros)| Word {
            text: text.into(),
            start_micros,
            end_micros,
        })
        .collect()
    }
    fn level(quiet: bool) -> Hop {
        let db = if quiet { -6_000 } else { -1_800 };
        Hop {
            rms_dbfs_hundredths: db,
            peak_dbfs_hundredths: db,
            voice: !quiet,
        }
    }
    fn ordinary() -> Arc<BoundDialogue> {
        value(said(), 4_000_000, |t| {
            level(
                ![
                    (200_000, 500_000),
                    (1_100_000, 1_400_000),
                    (2_000_000, 2_300_000),
                    (3_000_000, 3_300_000),
                ]
                .into_iter()
                .any(|(a, b)| a <= t && t < b),
            )
        })
    }
    fn document(source_rate: Rational, project_rate: Rational, start: i64, end: i64) -> Document {
        let mut doc = Document {
            fps: project_rate,
            ..Document::default()
        };
        doc.media_pool.push(MediaAsset {
            id: AssetId(1),
            path: "never-opened.wav".into(),
            name: "talk".into(),
            duration: TimeCode(end),
            fps: source_rate,
            kind: MediaKind::Audio,
            resolution: None,
            source_fingerprint: MediaSourceFingerprint {
                content_sha256: Some("a".repeat(64)),
                byte_len: Some(100),
            },
            color_description: kinewright_core::ColorDescription::default(),
            assumed_from: None,
        });
        let clip=serde_json::from_value(json!({"id":1,"asset":1,"source_range":{"start":start,"end":end},"timeline_start":0,"effects":[],"transition_in":null})).unwrap();
        doc.tracks.push(Track {
            id: TrackId(1),
            kind: TrackKind::Audio,
            sync_lock: true,
            clips: vec![clip],
        });
        doc.duration = doc.clip_duration(&doc.tracks[0].clips[0]).unwrap();
        doc.validate().unwrap();
        doc
    }
    fn service(doc: Document, value: Arc<BoundDialogue>) -> KinewrightMcp {
        let media = Arc::new(NoopMedia::default());
        let playback = Arc::clone(&media);
        let service = KinewrightMcp::new(
            Core::spawn(doc).unwrap(),
            playback,
            media,
            ConfirmationBroker::default(),
        );
        *service.dialogue_measurements.write().unwrap() = Some(Arc::new(Fake { value, stream: 0 }));
        service
    }
    fn fixture() -> (KinewrightMcp, Arc<BoundDialogue>) {
        let v = ordinary();
        let fps = Rational::new(100, 1).unwrap();
        (service(document(fps, fps, 0, 400), Arc::clone(&v)), v)
    }
    fn invoke(s: &KinewrightMcp, name: &str, args: Value) -> CallToolResult {
        s.call_exposed_inner(
            CallToolRequestParams::new("invoke_capability").with_arguments(
                serde_json::Map::from_iter([
                    ("name".into(), Value::String(name.into())),
                    ("arguments".into(), args),
                ]),
            ),
        )
        .unwrap()
    }
    fn cut(s: &KinewrightMcp, args: Value, approve: bool) -> CallToolResult {
        thread::scope(|scope| {
            let worker = scope.spawn(|| invoke(s, "cut_dialogue", args));
            let deadline = Instant::now() + Duration::from_secs(60);
            while !worker.is_finished() {
                for request in s.confirmations.pending_requests() {
                    if approve {
                        let _ = s.confirmations.approve(request.id);
                    } else {
                        s.confirmations.reject(request.id, "person declined");
                    }
                }
                assert!(Instant::now() < deadline, "DQ_TOOL_REQUEST_BOUNDED");
                thread::yield_now();
            }
            worker.join().unwrap()
        })
    }
    fn address(v: &BoundDialogue, index: usize) -> Value {
        json!({"clip":1,"index":index,"binding":v.binding()})
    }
    fn words(v: &BoundDialogue, index: usize) -> Value {
        let mut removal = json!({"kind":"words","first":address(v,index),"last":address(v,index)});
        // Existing geometry controls keep fixed interior air. Boundary defaults
        // remain standard air; the rehearsal tests opt into natural interior air.
        if index > 0 && index < v.dialogue().words().len().saturating_sub(1) {
            removal["leave_micros"] = json!(250_000);
        }
        removal
    }
    fn args(removals: Vec<Value>) -> Value {
        Value::Object(serde_json::Map::from_iter([
            ("expected_revision".into(), json!(0)),
            ("removals".into(), Value::Array(removals)),
        ]))
    }
    fn content(r: &CallToolResult) -> &Value {
        r.structured_content.as_ref().unwrap()
    }
    fn ok(r: &CallToolResult) {
        assert_ne!(r.is_error, Some(true), "{r:?}");
    }

    #[test]
    fn dq_tools_word_cut_atomic_undo_and_fresh_readback() {
        let (s, v) = fixture();
        let (_, before) = s.snapshot().unwrap();
        let result = cut(&s, args(vec![words(&v, 1)]), true);
        ok(&result);
        let (revision, after) = s.snapshot().unwrap();
        assert_eq!(revision, TimelineRevision(1));
        let clips = &after.tracks[0].clips;
        assert_eq!(clips.len(), 2);
        assert_eq!(clips[0].source_range, TimeCode(0)..TimeCode(65));
        assert_eq!(clips[1].source_range, TimeCode(190)..TimeCode(400));
        assert_eq!(clips[1].timeline_start, TimeCode(65));
        assert_eq!(content(&result)["removals"][0]["cut"], json!([65, 190]));
        assert_eq!(content(&result)["removals"][0]["edges"][0][0], 65);
        assert_eq!(content(&result)["removals"][0]["edges"][1][0], 65);
        let checked = invoke(&s, "check_dialogue_cut", json!({"track_id":1}));
        ok(&checked);
        assert_eq!(
            content(&result)["joins"]["checked_at"],
            json!([[65, [1, [650_000, 1]], [1, [1_900_000, 1]], 0]]),
            "DQ_TOOLS_FRESH_COMMITTED_READBACK"
        );
        let kept: Vec<_> = dq::spans(&after)
            .unwrap()
            .into_iter()
            .filter(|s| s.track == TrackId(1))
            .map(|s| KeptRange {
                recording: RecordingId(s.asset.0),
                source: s.source,
            })
            .collect();
        let direct = audit::audit(
            &BTreeMap::from([(RecordingId(1), v.dialogue())]),
            &kept,
            &AuditSettings {
                detail: Detail::Flags,
                ..AuditSettings::default()
            },
            audit::comparison_key,
        )
        .unwrap();
        assert_eq!(
            content(&checked)["check"]["segments"][0]["report"],
            serde_json::to_value(direct).unwrap(),
            "DQ_TOOLS_DIRECT_CORE_AUDIT"
        );
        let bytes = serde_json::to_vec(&content(&result)["removals"][0])
            .unwrap()
            .len();
        println!("DQ_REMOVAL_BYTES {bytes}");
        assert!(bytes < 200);
        s.core.request(Command::Undo).unwrap();
        assert_eq!(*s.snapshot().unwrap().1, *before, "DQ_TOOLS_ONE_UNDO");
    }
    #[test]
    fn dq_tools_pause_air_and_whole_batch_refusal() {
        let (s, v) = fixture();
        let result = cut(
            &s,
            args(vec![
                json!({"kind":"pause","after":address(&v,0),"leave_micros":250_000}),
            ]),
            true,
        );
        ok(&result);
        let after = s.snapshot().unwrap().1;
        let removed = 400 - after.duration.0;
        assert_eq!(
            600_000 - removed * 10_000,
            250_000,
            "DQ_TOOLS_REQUESTED_AIR"
        );
        let (s, v) = fixture();
        let before = s.snapshot().unwrap();
        let history = s.core.export_history_identified().unwrap();
        let mut request = args(vec![
            words(&v, 1),
            json!({"kind":"pause","after":address(&v,3),"leave_micros":250_000}),
            words(&v, 3),
        ]);
        // This retained whole-batch guard is the explicit atomic option now.
        request["atomic"] = json!(true);
        let result = cut(&s, request, true);
        assert_eq!(result.is_error, Some(true), "DQ_TOOLS_WHOLE_BATCH_REFUSED");
        assert_eq!(content(&result)["removals"].as_array().unwrap().len(), 3);
        assert_eq!(content(&result)["removals"][0]["outcome"], "ready");
        assert!(
            content(&result)["removals"][1]["quiet_us"]
                .as_array()
                .unwrap()
                .iter()
                .any(Value::is_number)
        );
        assert_eq!(
            s.snapshot().unwrap(),
            before,
            "DQ_TOOLS_REFUSED_BATCH_UNCHANGED"
        );
        assert_eq!(s.core.export_history_identified().unwrap(), history);
    }
    #[test]
    fn dq_tools_overrule_and_no_approval() {
        let (s, v) = fixture();
        let r = json!({"kind":"words","first":address(&v,1),"last":address(&v,1),"overrule":[1_200_000,1_300_000]});
        let before = s.snapshot().unwrap();
        let refused = cut(&s, args(vec![r.clone()]), false);
        assert_eq!(refused.is_error, Some(true));
        assert_eq!(s.snapshot().unwrap(), before, "DQ_TOOLS_CONSENT_REQUIRED");
        let result = cut(&s, args(vec![r]), true);
        ok(&result);
        assert_eq!(content(&result)["removals"][0]["edges"][0][3], "overrule");
        let checked = invoke(&s, "check_dialogue_cut", json!({"track_id":1}));
        assert!(
            content(&checked)["check"]["segments"][0]["report"]["total"]["edge_in_sound"]
                .as_u64()
                .unwrap()
                > 0
        );
    }
    #[test]
    fn dq_tools_typed_identity_revision_stream_retime_and_absence() {
        let (s, v) = fixture();
        let before = s.snapshot().unwrap();
        let mut wrong = args(vec![words(&v, 1)]);
        wrong["expected_revision"] = json!(1);
        assert_eq!(content(&cut(&s, wrong, true))["reason"]["code"], "revision");
        let mut wrong = args(vec![words(&v, 1)]);
        wrong["removals"][0]["first"]["binding"] = json!("old");
        assert_eq!(
            content(&cut(&s, wrong, true))["reason"]["code"],
            "binding_changed"
        );
        let changed = value(said(), 4_000_000, |_| level(true));
        assert_ne!(changed.binding(), v.binding());
        *s.dialogue_measurements.write().unwrap() = Some(Arc::new(Fake {
            value: changed,
            stream: 0,
        }));
        assert_eq!(
            content(&cut(&s, args(vec![words(&v, 1)]), true))["reason"]["code"],
            "binding_changed"
        );
        assert_eq!(s.snapshot().unwrap(), before);
        *s.dialogue_measurements.write().unwrap() = Some(Arc::new(Fake {
            value: Arc::clone(&v),
            stream: 1,
        }));
        assert_eq!(
            content(&cut(&s, args(vec![words(&v, 1)]), true))["reason"]["code"],
            "stream"
        );
        let alien = value(said(), 4_000_000, |_| level(true));
        let mut changed = document(
            Rational::new(100, 1).unwrap(),
            Rational::new(100, 1).unwrap(),
            0,
            400,
        );
        changed.media_pool[0].source_fingerprint.content_sha256 = Some("b".repeat(64));
        let mismatch = service(changed, alien);
        assert_eq!(
            content(&cut(&mismatch, args(vec![words(&v, 1)]), true))["reason"]["code"],
            "identity"
        );
        assert_eq!(s.snapshot().unwrap(), before);
        let mut retimed = (*before.1).clone();
        retimed.tracks[0].clips[0].speed_percent = 200;
        retimed.duration = TimeCode(200);
        let t = service(retimed, Arc::clone(&v));
        let r = cut(&t, args(vec![words(&v, 1)]), true);
        assert_eq!(content(&r)["removals"][0]["code"], "retimed");
        let check = invoke(&t, "check_dialogue_cut", json!({"track_id":1}));
        assert_eq!(
            content(&check)["check"]["not_checked"][0]["code"],
            "retimed"
        );
        let short = value(said().into_iter().take(2).collect(), 2_000_000, |_| {
            level(true)
        });
        *s.dialogue_measurements.write().unwrap() = Some(Arc::new(Fake {
            value: Arc::clone(&short),
            stream: 0,
        }));
        let clipped = cut(&s, args(vec![words(&short, 1)]), true);
        assert_eq!(content(&clipped)["removals"][0]["code"], "not_measured");
        *s.dialogue_measurements.write().unwrap() = Some(Arc::new(Missing));
        assert_eq!(
            content(&cut(&s, args(vec![words(&v, 1)]), true))["reason"]["code"],
            "not_measured"
        );
        assert_eq!(s.snapshot().unwrap(), before);
    }
    fn clock_value() -> Arc<BoundDialogue> {
        let words = [
            ("kept", 700_000, 1_560_000),
            ("remove", 1_550_000, 2_000_000),
            ("next", 2_300_000, 2_500_000),
        ]
        .into_iter()
        .map(|(text, start_micros, end_micros)| Word {
            text: text.into(),
            start_micros,
            end_micros,
        })
        .collect();
        // Keep real quiet margins in the mixed-clock fixture, including beyond
        // the clip tail; the intentionally unsafe earlier frame remains refused.
        value(words, 2_640_000, |t| {
            level(
                (1_470_000..1_580_000).contains(&t)
                    || (2_000_000..2_300_000).contains(&t)
                    || t >= 2_500_000,
            )
        })
    }
    #[test]
    fn dq_tools_mixed_clock_guards_heard_edges_and_moves_safely() {
        let v = clock_value();
        let source = Rational::new(25, 1).unwrap();
        let project = Rational::new(24, 1).unwrap();
        assert!(v.dialogue().guard(1_520_000, AudibleSide::Out).is_ok());
        assert!(v.dialogue().guard(1_480_000, AudibleSide::Out).is_err());
        let s = service(document(source, project, 12, 64), Arc::clone(&v));
        let result = cut(&s, args(vec![words(&v, 1)]), true);
        assert_ne!(
            result.is_error,
            Some(true),
            "DQ_TOOLS_GUARD_AT_AUDIBLE_END: {result:?}"
        );
        let doc = s.snapshot().unwrap().1;
        for span in dq::spans(&doc).unwrap() {
            if span.project.start == TimeCode(25) {
                assert!(
                    v.dialogue()
                        .guard_exact(span.source.start, AudibleSide::In)
                        .is_ok(),
                    "DQ_TOOLS_HEARD_IN_GUARDED"
                );
            }
            if span.project.end == TimeCode(25) {
                assert!(
                    v.dialogue()
                        .guard_exact(span.source.end, AudibleSide::Out)
                        .is_ok(),
                    "DQ_TOOLS_HEARD_OUT_GUARDED"
                );
            }
        }
        assert_eq!(
            content(&result)["removals"][0]["edges"][0][0],
            25,
            "DQ_TOOLS_MOVED_FROM_UNSAFE_FRAME"
        );
        assert!(
            content(&result)["joins"]["flags"].is_null()
                || content(&result)["joins"]["flags"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|flag| flag[1]["sides"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .all(|side| side["edge_in_sound"].is_null()))
        );
    }
    #[test]
    fn dq_tools_unsafe_heard_edges_refuse_without_editing() {
        let v = clock_value();
        let s = service(
            document(
                Rational::new(25, 1).unwrap(),
                Rational::new(24, 1).unwrap(),
                14,
                64,
            ),
            Arc::clone(&v),
        );
        let before = s.snapshot().unwrap();
        let result = cut(&s, args(vec![words(&v, 1)]), true);
        assert_eq!(
            result.is_error,
            Some(true),
            "DQ_TOOLS_ACOUSTIC_GUARD_REQUIRED"
        );
        assert_eq!(
            s.snapshot().unwrap(),
            before,
            "DQ_TOOLS_UNSAFE_HEARD_UNCHANGED"
        );
    }
    #[test]
    fn dq_tools_unequal_rates_keep_both_sides_and_ten_ntsc_cuts() {
        let v = ordinary();
        let s = service(
            document(
                Rational::new(25, 1).unwrap(),
                Rational::new(24, 1).unwrap(),
                0,
                100,
            ),
            Arc::clone(&v),
        );
        let result = cut(
            &s,
            args(vec![
                json!({"kind":"words","first":address(&v,1),"last":address(&v,1),"overrule":[665_001,1_899_999]}),
            ]),
            true,
        );
        ok(&result);
        let edges = &content(&result)["removals"][0]["edges"];
        let exact = |v: &Value| {
            kinewright_core::ExactMicros::new(
                u128::from(v[0].as_u64().unwrap()),
                v[1].as_u64().unwrap(),
            )
            .unwrap()
        };
        assert!(
            exact(&edges[0][2]) >= kinewright_core::ExactMicros::integer(665_001),
            "DQ_TOOLS_ROUND_KEEP_OUT"
        );
        assert!(
            exact(&edges[1][2]) <= kinewright_core::ExactMicros::integer(1_899_999),
            "DQ_TOOLS_ROUND_KEEP_IN"
        );
        let tokens = (0..21)
            .map(|i| Word {
                text: format!("word{i}"),
                start_micros: 200_000 + i * 1_500_000,
                end_micros: 500_000 + i * 1_500_000,
            })
            .collect();
        let v = value(tokens, 40_040_000, |t| {
            level(t % 1_500_000 < 200_000 || t % 1_500_000 >= 500_000)
        });
        let s = service(
            document(
                Rational::new(30_000, 1001).unwrap(),
                Rational::new(30, 1).unwrap(),
                0,
                1200,
            ),
            Arc::clone(&v),
        );
        let r = cut(
            &s,
            args((1..20).step_by(2).map(|i| words(&v, i)).collect()),
            true,
        );
        ok(&r);
        assert_eq!(content(&r)["removals"].as_array().unwrap().len(), 10);
        for span in dq::spans(&s.snapshot().unwrap().1).unwrap() {
            assert!(
                v.dialogue()
                    .guard_exact(span.source.start, AudibleSide::In)
                    .is_ok(),
                "DQ_TOOLS_NTSC_IN"
            );
            assert!(
                v.dialogue()
                    .guard_exact(span.source.end, AudibleSide::Out)
                    .is_ok(),
                "DQ_TOOLS_NTSC_OUT"
            );
        }
    }
    #[test]
    fn dq_tools_padded_clip_and_sixty_join_default_size() {
        // Measure speech outside this clip's 40 ms audible source span.
        let v = value(vec![], 100_000, |t| level(t < 50_000));
        let s = service(
            document(
                Rational::new(25, 1).unwrap(),
                Rational::new(24, 1).unwrap(),
                0,
                1,
            ),
            v,
        );
        let r = invoke(&s, "check_dialogue_cut", json!({"track_id":1}));
        ok(&r);
        assert_eq!(
            content(&r)["check"]["segments"][0]["report"]["items"][0]["sides"][0]["padded_us"],
            json!([5000, 3]),
            "DQ_TOOLS_PADDING_REPORTED"
        );
        let fps = Rational::new(100, 1).unwrap();
        // Frequent word onsets describe the continuous delivery; one nine-second
        // token would correctly trigger the new onset-based hole warning.
        let tokens = (0..=60)
            .flat_map(|i| {
                (0..17).map(move |k| {
                    let start_micros = i * 9_800_000 + 200_000 + k * 500_000;
                    Word {
                        text: format!("w{}", i * 17 + k),
                        start_micros,
                        end_micros: start_micros + 300_000,
                    }
                })
            })
            .collect();
        let measurement = value(tokens, 600_000_000, |t| {
            let sounding = t / 9_800_000 <= 60 && (200_000..8_800_000).contains(&(t % 9_800_000));
            let db = if sounding { -2_000 } else { -6_000 };
            Hop {
                rms_dbfs_hundredths: db,
                peak_dbfs_hundredths: db + 600,
                voice: sounding,
            }
        });
        let mut doc = document(fps, fps, 0, 60_000);
        doc.tracks[0].clips.clear();
        for i in 0..=60 {
            doc.tracks[0].clips.push(serde_json::from_value(json!({"id":i+1,"asset":1,"source_range":{"start":i*980,"end":i*980+900},"timeline_start":i*900,"effects":[],"transition_in":null})).unwrap());
        }
        doc.duration = TimeCode(54_900);
        let service = service(doc, measurement);
        let flags = invoke(&service, "check_dialogue_cut", json!({"track_id":1}));
        ok(&flags);
        let all = invoke(
            &service,
            "check_dialogue_cut",
            json!({"track_id":1,"detail":"all"}),
        );
        ok(&all);
        let flag_bytes = serde_json::to_vec(content(&flags)).unwrap().len();
        let all_bytes = serde_json::to_vec(content(&all)).unwrap().len();
        let core_flags = serde_json::to_vec(&content(&flags)["check"]["segments"][0]["report"])
            .unwrap()
            .len();
        let core_all = serde_json::to_vec(&content(&all)["check"]["segments"][0]["report"])
            .unwrap()
            .len();
        println!(
            "DQ_TOOL_60_JOINS flags={flag_bytes} all={all_bytes} core_flags={core_flags} core_all={core_all}"
        );
        assert!(flag_bytes < 2048, "DQ_TOOLS_DEFAULT_SIZE");
        assert!(all_bytes < 32768, "DQ_TOOLS_ALL_SIZE");
        // Measured basis plus distinct nearest-word addresses in the onset fixture.
        assert_eq!(core_flags, 164, "DQ_TOOLS_MEASURED_CORE_FLAGS");
        assert_eq!(core_all, 31_870, "DQ_TOOLS_MEASURED_CORE_ALL");
        assert_eq!(
            content(&flags)["check"]["segments"][0]["report"]["total"]["holes"]
                .as_u64()
                .unwrap_or(0),
            0
        );
        assert!(content(&flags)["check"]["segments"][0]["report"]["items"].is_null());
        assert_eq!(
            content(&flags)["check"]["segments"][0]["report"]["total"]["joins"],
            62
        );
    }
    #[test]
    fn dq_tools_transcript_legacy_bytes_and_bound_addresses() {
        fn legacy_words() -> Vec<TimelineTranscriptWord> {
            vec![TimelineTranscriptWord {
                text: "Hello".into(),
                speaker: None,
                asset: AssetId(1),
                track: TrackId(1),
                clip: ClipId(1),
                source_start: TimeCode(20),
                source_end: TimeCode(50),
                project_start: TimeCode(20),
                project_end: TimeCode(50),
            }]
        }
        let fps = Rational::new(100, 1).unwrap();
        let doc = document(fps, fps, 0, 400);
        let media = Arc::new(NoopMedia {
            timeline_words: Some(legacy_words),
            transcript: Some(Arc::new(AssetTranscript {
                asset: AssetId(1),
                content_sha256: "a".repeat(64),
                source_fps: fps,
                words: vec![],
            })),
            ..NoopMedia::default()
        });
        let playback = Arc::clone(&media);
        let s = KinewrightMcp::new(
            Core::spawn(doc).unwrap(),
            playback,
            media,
            ConfirmationBroker::default(),
        );
        let result = invoke(&s, "get_timeline_transcript", json!({}));
        ok(&result);
        assert_eq!(
            result.content[0].as_text().unwrap().text,
            "timeline transcript range=0f/0.000s..400f/4.000s words=1 word_timing=approximate_transcript_intervals (ends are not measured speech boundaries)\nclip=1 asset=1 project=20f/0.200s..50f/0.500s source=20f/0.200s..50f/0.500s \"Hello\"",
            "DQ_TOOLS_SWITCH_OFF_BYTES"
        );
        let hidden = invoke(&s, "cut_dialogue", args(vec![]));
        assert_eq!(hidden.is_error, Some(true));
        let catalog = s.dialogue_catalog().unwrap();
        assert!(
            !catalog
                .iter()
                .any(|t| dq::TOOL_NAMES.contains(&t.name.as_ref()))
        );
        let tokens = (0..1800)
            .map(|i| Word {
                text: format!("word{i}"),
                start_micros: i * 400_000,
                end_micros: i * 400_000 + 300_000,
            })
            .collect();
        let v = value(tokens, 720_000_000, |_| level(true));
        let s = service(document(fps, fps, 0, 72_000), Arc::clone(&v));
        let r = invoke(&s, "get_timeline_transcript", json!({}));
        ok(&r);
        let full = content(&r);
        let mut no_addresses = full.clone();
        for segment in no_addresses["segments"].as_array_mut().unwrap() {
            segment.as_object_mut().unwrap().remove("first");
        }
        let overhead = serde_json::to_vec(full).unwrap().len()
            - serde_json::to_vec(&no_addresses).unwrap().len();
        let payload_bytes = serde_json::to_vec(full).unwrap().len();
        for segment in no_addresses["segments"].as_array_mut().unwrap() {
            segment.as_object_mut().unwrap().remove("binding");
        }
        let address_bytes = payload_bytes - serde_json::to_vec(&no_addresses).unwrap().len();
        println!(
            "DQ_TRANSCRIPT_ADDRESS_BYTES words=1800 segments={} index={overhead} binding={} address={address_bytes} payload={payload_bytes}",
            full["segments"].as_array().unwrap().len(),
            address_bytes - overhead
        );
        assert_eq!(address_bytes, 87);
        assert_eq!(overhead, 10);
        assert_eq!(full["segments"][0]["first"], 0);
        assert_eq!(full["segments"][0]["words"].as_array().unwrap().len(), 1800);
        let mut cut_args = args(vec![words(&v, 0)]);
        cut_args["removals"][0]["first"]["index"] = json!(1800);
        assert_eq!(content(&cut(&s, cut_args, true))["reason"]["code"], "word");
    }

    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "two recording fixtures, real consent, Undo and overlap control"
    )]
    fn dq_tools_join_between_two_recordings_names_both_and_counts_music() {
        struct Pair {
            a: Arc<BoundDialogue>,
            b: Arc<BoundDialogue>,
        }
        impl DialogueMeasurements for Pair {
            fn audio_stream(&self, _: &MediaIdentity) -> Result<u32, Refusal> {
                Ok(0)
            }
            fn get(&self, id: &MediaIdentity, _: u32) -> Result<Arc<BoundDialogue>, Refusal> {
                Ok(Arc::clone(if id.sha256.starts_with('b') {
                    &self.b
                } else {
                    &self.a
                }))
            }
        }
        let a = ordinary();
        let b = Arc::new(
            BoundDialogue::new(
                MediaIdentity {
                    sha256: "b".repeat(64),
                    byte_length: 100,
                },
                0,
                "literal-v1".into(),
                SoundProfile::from_hops(
                    10_000,
                    4_000_000,
                    (0..400).map(|i| {
                        level(
                            ![
                                (200_000, 500_000),
                                (1_100_000, 1_400_000),
                                (2_000_000, 2_300_000),
                                (3_000_000, 3_300_000),
                            ]
                            .into_iter()
                            .any(|(a, b)| a <= i * 10_000 + 5_000 && i * 10_000 + 5_000 < b),
                        )
                    }),
                )
                .unwrap(),
                said(),
                Settings::default(),
            )
            .unwrap(),
        );
        let fps = Rational::new(100, 1).unwrap();
        let mut doc = document(fps, fps, 0, 400);
        let mut other = document(fps, fps, 0, 400);
        let mut asset = other.media_pool.pop().unwrap();
        asset.id = AssetId(2);
        asset.source_fingerprint.content_sha256 = Some("b".repeat(64));
        doc.media_pool.push(asset);
        let mut clip = other.tracks.pop().unwrap().clips.pop().unwrap();
        clip.id = ClipId(2);
        clip.asset = AssetId(2);
        clip.timeline_start = TimeCode(400);
        doc.tracks[0].clips.push(clip);
        doc.duration = TimeCode(800);
        let s = service(doc, Arc::clone(&a));
        *s.dialogue_measurements.write().unwrap() = Some(Arc::new(Pair {
            a: Arc::clone(&a),
            b: Arc::clone(&b),
        }));
        let original = s.snapshot().unwrap().1;
        let second = json!({"clip":2,"index":0,"binding":b.binding()});
        let result = cut(
            &s,
            args(vec![
                words(&a, 3),
                json!({"kind":"words","first":second,"last":second}),
            ]),
            true,
        );
        ok(&result);
        let positions = content(&result)["joins"]["checked_at"].as_array().unwrap();
        assert!(
            positions.iter().all(|at| at[1][0] == 1 && at[2][0] == 2),
            "DQ_TOOLS_RECORDING_SIDES"
        );
        let read = invoke(
            &s,
            "check_dialogue_cut",
            json!({"track_id":1,"detail":"all"}),
        );
        ok(&read);
        assert!(
            content(&read)["check"]["segments"][0]["report"]["items"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item["kind"] == "join"
                    && item["sides"]
                        .as_array()
                        .is_some_and(|sides| sides.len() == 2
                            && sides[0]["source"][0] == 1
                            && sides[1]["source"][0] == 2))
        );
        s.core.request(Command::Undo).unwrap();
        assert_eq!(*s.snapshot().unwrap().1, *original);
        for (first_overruled, join_frame) in [(true, 310), (false, 245)] {
            let mut first = words(&a, 3);
            let second = json!({"clip":2,"index":0,"binding":b.binding()});
            let mut second = json!({"kind":"words","first":second,"last":second});
            if first_overruled {
                first["overrule"] = json!([3_100_000, 4_000_000]);
            } else {
                second["overrule"] = json!([0, 300_000]);
            }
            let revision = s.snapshot().unwrap().0;
            let mut request = args(vec![first, second]);
            request["expected_revision"] = json!(revision);
            let result = cut(&s, request, true);
            ok(&result);
            assert_eq!(content(&result)["from_revision"], json!(revision));
            for removal in content(&result)["removals"].as_array().unwrap() {
                assert_eq!(
                    removal["edges"][0][0], join_frame,
                    "DQ_TOOLS_ADJACENT_EDGE_CLOCK"
                );
                assert_eq!(
                    removal["edges"][1][0], join_frame,
                    "DQ_TOOLS_ADJACENT_EDGE_CLOCK"
                );
                assert_eq!(
                    removal["edges"][0][3],
                    if first_overruled { "overrule" } else { "quiet" }
                );
                assert_eq!(
                    removal["edges"][1][3],
                    if first_overruled { "quiet" } else { "overrule" }
                );
            }
            assert!(
                !content(&result)["joins"]["flags"]
                    .as_array()
                    .unwrap()
                    .is_empty()
            );
            s.core.request(Command::Undo).unwrap();
            assert_eq!(*s.snapshot().unwrap().1, *original);
        }
        // A separate audible bed is counted without measuring or judging its mix.
        let mut doc = (*original).clone();
        let mut bed = doc.tracks[0].clips[0].clone();
        bed.id = ClipId(3);
        doc.tracks.push(Track {
            id: TrackId(2),
            kind: TrackKind::Audio,
            sync_lock: false,
            clips: vec![bed],
        });
        let s = service(doc, Arc::clone(&a));
        let result = cut(&s, args(vec![words(&a, 1)]), true);
        ok(&result);
        assert_eq!(
            content(&result)["joins"]["checked_at"][0][3],
            1,
            "DQ_TOOLS_OTHER_AUDIBLE_COUNT"
        );
    }

    #[test]
    fn dq_tools_discovery_uses_the_invocable_dispatch_path() {
        let (mut service, _) = fixture();
        service.host_scoped = true;
        let result = service
            .call_exposed_inner(
                CallToolRequestParams::new("get_capability").with_arguments(
                    json!({"names":["cut_dialogue","check_dialogue_cut"]})
                        .as_object()
                        .unwrap()
                        .clone(),
                ),
            )
            .unwrap();
        ok(&result);
        for descriptor in content(&result)["capabilities"].as_array().unwrap() {
            assert_eq!(descriptor["invocation"], "invoke_capability");
        }
        let checked = invoke(&service, "check_dialogue_cut", json!({"track_id":1}));
        ok(&checked);
        let schemas = task_role_tools_uncached(&[
            "get_timeline_transcript",
            "cut_dialogue",
            "check_dialogue_cut",
        ])
        .unwrap();
        assert_eq!(schemas.len(), 3);
    }
    include!("dialogue_tools_review_tests.rs");
    include!("dialogue_tools_guide_tests.rs");
    include!("dialogue_tools_hole_tests.rs");
    include!("dialogue_tools_rehearsal_core_tests.rs");
    include!("dialogue_tools_rehearsal_batch_tests.rs");
    include!("dialogue_tools_rehearsal_read_tests.rs");
    include!("dialogue_tools_item12_tests.rs");
    include!("dialogue_tools_item13_tests.rs");
    include!("dialogue_tools_r4_tests.rs");
}
