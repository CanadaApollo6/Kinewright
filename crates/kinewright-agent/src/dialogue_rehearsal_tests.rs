/// Opt-in measurement, deliberately outside the ordinary fast and slow tiers.
mod dialogue_rehearsal_tests {
    use super::*;
    use crate::{
        dialogue_measurements::CachedDialogueMeasurements,
        dialogue_tools::{DialogueMeasurements, MediaIdentity, Refusal},
    };
    use kinewright_core::{MediaSourceOrigin, is_filler_word};
    use kinewright_media::{
        FfmpegMediaEngine, GpuContext, SessionMedia,
        rehearsal_support::{copy_pinned_english_model, write_export_mix_wav},
        test_support::workspace_target_dir,
    };
    use serde::Serialize;
    use serde_json::Value;
    use std::{
        collections::{BTreeMap, BTreeSet},
        fs,
        io::{BufWriter, Write},
        path::{Path, PathBuf},
    };

    #[derive(Serialize)]
    struct Action {
        kind: &'static str,
        index: Option<usize>,
        next: Option<usize>,
        text: String,
        source_us: [u64; 2],
        original_project_us: [u64; 2],
        nominal_source_us: [u64; 2],
    }
    struct Addressed<'a> {
        index: usize,
        address: Value,
        word: &'a Value,
    }
    fn addressed(reply: &Value) -> Vec<Addressed<'_>> {
        let mut result = Vec::new();
        for segment in reply["segments"]
            .as_array()
            .expect("bound transcript segments")
        {
            let first = usize::try_from(segment["first"].as_u64().unwrap()).unwrap();
            for (offset, word) in segment["words"].as_array().unwrap().iter().enumerate() {
                let index = first + offset;
                result.push(Addressed {
                    index,
                    address: json!({"clip":segment["clip"],"binding":segment["binding"],"index":index}),
                    word,
                });
            }
        }
        result.sort_by_key(|w| (w.word[3].as_i64().unwrap(), w.index));
        result
    }
    fn actions(read: &Value, measured: &kinewright_core::dialogue::Dialogue) -> Vec<Action> {
        let mut words = addressed(read);
        words.dedup_by_key(|w| w.index);
        let mut result = Vec::new();
        for word in &words {
            let text = word.word[0].as_str().unwrap();
            if is_filler_word(text) {
                let source_us = [
                    word.word[1].as_u64().unwrap(),
                    word.word[2].as_u64().unwrap(),
                ];
                result.push(Action {
                    kind: "filler",
                    index: Some(word.index),
                    next: None,
                    text: text.into(),
                    source_us,
                    original_project_us: source_us,
                    nominal_source_us: source_us,
                });
            }
        }
        let union = measured.gap_runs().iter().copied();
        let settings = measured.settings();
        let share = 300_000 * settings.air_after_micros
            / (settings.air_after_micros + settings.air_before_micros);
        for run in union.filter(|r| r.duration() > 700_000) {
            let before = words
                .iter()
                .rev()
                .find(|w| w.word[1].as_u64().unwrap() <= run.start_micros());
            let next = words
                .iter()
                .find(|w| w.word[1].as_u64().unwrap() > run.start_micros());
            let source_us = [run.start_micros(), run.end_micros()];
            result.push(Action {
                kind: "pause",
                index: before.map(|w| w.index),
                next: next.map(|w| w.index),
                text: format!(
                    "{} / {}",
                    before.map_or("<start>", |w| w.word[0].as_str().unwrap()),
                    next.map_or("<end>", |w| w.word[0].as_str().unwrap())
                ),
                source_us,
                original_project_us: source_us,
                nominal_source_us: [
                    run.start_micros() + share,
                    run.end_micros() - (300_000 - share),
                ],
            });
        }
        // The initial clip is the complete source at project zero, unit speed.
        result.sort_by_key(|a| (a.original_project_us[0], a.index));
        result
    }
    #[derive(Serialize)]
    struct Call {
        tool: String,
        elapsed_us: u128,
        reply_bytes: usize,
        arguments: Value,
        reply: Value,
        #[serde(skip)]
        wire: Vec<u8>,
    }
    fn invoke(service: &KinewrightMcp, name: &str, args: Value, calls: &mut Vec<Call>) -> Value {
        let start = Instant::now();
        let response = service
            .call_exposed_blocking(
                CallToolRequestParams::new("invoke_capability").with_arguments(
                    serde_json::Map::from_iter([
                        ("name".into(), json!(name)),
                        ("arguments".into(), args.clone()),
                    ]),
                ),
            )
            .unwrap();
        let wire = serde_json::to_vec(&response).unwrap();
        let elapsed_us = start.elapsed().as_micros();
        let reply = response
            .structured_content
            .expect("production structured reply");
        calls.push(Call {
            tool: name.into(),
            elapsed_us,
            reply_bytes: wire.len(),
            arguments: args,
            reply: reply.clone(),
            wire,
        });
        reply
    }
    fn cut(service: &KinewrightMcp, args: Value, calls: &mut Vec<Call>) -> Value {
        thread::scope(|scope| {
            let work = scope.spawn(|| invoke(service, "cut_dialogue", args, calls));
            let deadline = Instant::now() + Duration::from_mins(2);
            while !work.is_finished() {
                for request in service.confirmations.pending_requests() {
                    assert!(service.confirmations.approve(request.id));
                }
                assert!(Instant::now() < deadline, "rehearsal cut exceeded 120s");
                thread::yield_now();
            }
            work.join().unwrap()
        })
    }
    fn refusal(reply: &Value, index: usize) -> Value {
        if let Some(reason) = reply.get("reason") {
            return reason.clone();
        }
        let outcomes = reply["removals"].as_array().expect("refused plan outcomes");
        let candidate = &outcomes[index];
        if candidate["outcome"] == "refused" {
            candidate.clone()
        } else {
            json!({"code":"atomic_batch_refused"})
        }
    }
    fn result(action: &Action, outcome: &Value) -> Value {
        if outcome["outcome"] != "ready" {
            return json!({"action":action,"status":"refused","reason":outcome});
        }
        // Compare to the request extent before acoustic/frame adjustment.
        // The pause split uses the unchanged production air allocation.
        let nominal = action.nominal_source_us;
        let source = |edge: usize| {
            let exact = &outcome["edges"][edge][2];
            [
                i128::from(exact[0].as_u64().expect("exact source numerator")),
                i128::from(exact[1].as_u64().unwrap()),
            ]
        };
        let actual = [source(0), source(1)];
        let moves = [
            [
                actual[0][0] - i128::from(nominal[0]) * actual[0][1],
                actual[0][1],
            ],
            [
                actual[1][0] - i128::from(nominal[1]) * actual[1][1],
                actual[1][1],
            ],
        ];
        json!({"action":action,"status":if moves[0][0]==0&&moves[1][0]==0{"as_asked"}else{"moved"},"nominal_source_us":nominal,"actual_source_us":actual,"edge_moves_us":moves,"outcome":outcome})
    }
    #[derive(Default)]
    struct Addresses {
        words: BTreeMap<usize, Value>,
        revision: Value,
        neighbour_uses: usize,
        neighbours: BTreeSet<usize>,
    }
    impl Addresses {
        fn read(&mut self, reply: &Value) {
            self.revision = reply["revision"].clone();
            for w in addressed(reply) {
                if self.words.get(&w.index) != Some(&w.address) {
                    self.words.insert(w.index, w.address);
                    self.neighbours.remove(&w.index);
                }
            }
        }
        fn applied(&mut self, reply: &Value, args: &Value) {
            self.revision = reply["revision"].clone();
            let changed: BTreeSet<_> = reply["removals"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|r| r["outcome"] == "ready")
                .map(|r| {
                    let request =
                        &args["removals"][usize::try_from(r["index"].as_u64().unwrap()).unwrap()];
                    request.get("first").unwrap_or(&request["after"])["clip"]
                        .as_u64()
                        .unwrap()
                })
                .collect();
            self.words
                .retain(|_, a| !changed.contains(&a["clip"].as_u64().unwrap()));
            let n = &reply["neighbours"];
            for join in n["joins"].as_array().expect("current neighbours") {
                assert!(
                    join["not_measured"].as_array().unwrap().is_empty(),
                    "{join}"
                );
                for a in join["left"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .chain(join["right"].as_array().unwrap())
                {
                    let clip = a[0].as_u64().unwrap();
                    let index = usize::try_from(a[1].as_u64().unwrap()).unwrap();
                    self.words.insert(index, json!({"clip":clip,"index":index,"binding":n["bindings"][clip.to_string()]}));
                    self.neighbours.insert(index);
                }
            }
        }
        fn address_index(&self, action: &Action) -> Option<usize> {
            action.index.filter(|index| self.words.contains_key(index))
        }
        fn removal(&mut self, action: &Action) -> Option<Value> {
            let index = self.address_index(action)?;
            let a = self.words.get(&index)?;
            if self.neighbours.contains(&index) {
                self.neighbour_uses += 1;
            }
            Some(if action.kind == "filler" {
                json!({"kind":"words","first":a,"last":a})
            } else {
                json!({"kind":"pause","after":a,"leave_micros":300_000})
            })
        }
    }
    #[allow(clippy::too_many_lines, reason = "one opt-in scripted editor sequence")]
    fn execute(
        service: &KinewrightMcp,
        measured: &kinewright_core::dialogue::Dialogue,
        calls: &mut Vec<Call>,
    ) -> (Vec<Value>, Value, usize) {
        let (_, doc) = service.snapshot().unwrap();
        let fps = doc.fps;
        let page = fps.numerator() * 30 / fps.denominator();
        let mut book = Addresses::default();
        let mut initial = json!({"segments":[]});
        let mut start = 0;
        while start < doc.duration.0 {
            let end = (start + i64::from(page)).min(doc.duration.0);
            let read = invoke(
                service,
                "get_timeline_transcript",
                json!({"range":{"start":start,"end":end}}),
                calls,
            );
            assert!(
                read["not_measured"].as_array().unwrap().is_empty(),
                "{read}"
            );
            assert_eq!(read["word_timing"], "approximate_transcript_intervals");
            book.read(&read);
            initial["segments"]
                .as_array_mut()
                .unwrap()
                .extend(read["segments"].as_array().unwrap().iter().cloned());
            start = end;
        }
        let actions = actions(&initial, measured);
        let mut results = Vec::new();
        let mut cuts: Vec<[i64; 2]> = Vec::new();
        let mut duration = doc.duration.0;
        let source_frame = |t: u64| {
            i64::try_from(
                u128::from(t) * u128::from(fps.numerator())
                    / (1_000_000 * u128::from(fps.denominator())),
            )
            .unwrap()
        };
        let project_frame = |t: u64, cuts: &[[i64; 2]]| {
            let source = source_frame(t);
            source
                - cuts
                    .iter()
                    .map(|c| (source.min(c[1]) - c[0]).max(0))
                    .sum::<i64>()
        };
        let mut work: std::collections::VecDeque<Vec<&Action>> = actions
            .chunks(32)
            .map(|batch| batch.iter().collect())
            .collect();
        while let Some(batch) = work.pop_front() {
            if batch.iter().any(|a| book.address_index(a).is_none()) {
                let first = batch.first().unwrap().source_us[0].saturating_sub(3_000_000);
                let last = batch.last().unwrap().source_us[1].saturating_add(3_000_000);
                let from = project_frame(first, &cuts).clamp(0, duration.saturating_sub(1));
                let end = project_frame(last, &cuts).clamp(from + 1, duration);
                let read = invoke(
                    service,
                    "get_timeline_transcript",
                    json!({"range":{"start":from,"end":end}}),
                    calls,
                );
                assert!(
                    read["not_measured"].as_array().unwrap().is_empty(),
                    "{read}"
                );
                book.read(&read);
            }
            let mut pending = Vec::new();
            for action in &batch {
                if let Some(removal) = book.removal(action) {
                    pending.push((action, removal));
                } else {
                    results.push(json!({"action":action,"status":"unaddressable","reason":{"code":"no_surviving_address"}}));
                }
            }
            if pending.is_empty() {
                continue;
            }
            let args = json!({"expected_revision":book.revision,"removals":pending.iter().map(|(_,r)|r).collect::<Vec<_>>()});
            let reply = cut(service, args.clone(), calls);
            if reply["status"] == "applied" {
                assert_eq!(reply["removals"].as_array().unwrap().len(), pending.len());
                book.applied(&reply, &args);
                for ((action, request), outcome) in
                    pending.iter().zip(reply["removals"].as_array().unwrap())
                {
                    if outcome["outcome"] == "refused" && outcome["code"] == "conflict" {
                        continue;
                    }
                    let mut row = result(action, outcome);
                    row["address_used"] = request.get("first").unwrap_or(&request["after"]).clone();
                    if outcome["outcome"] == "ready" {
                        let edge = |i: usize| {
                            let x = &outcome["edges"][i][2];
                            let n =
                                u128::from(x[0].as_u64().unwrap()) * u128::from(fps.numerator());
                            let d = u128::from(x[1].as_u64().unwrap())
                                * 1_000_000
                                * u128::from(fps.denominator());
                            assert_eq!(
                                n % d,
                                0,
                                "committed edge lies on the exact source frame grid"
                            );
                            i64::try_from(n / d).unwrap()
                        };
                        cuts.push([edge(0), edge(1)]);
                        duration -= outcome["cut"][1].as_i64().unwrap()
                            - outcome["cut"][0].as_i64().unwrap();
                    }
                    results.push(row);
                }
                for ((action, _), outcome) in pending
                    .iter()
                    .zip(reply["removals"].as_array().unwrap())
                    .rev()
                {
                    if outcome["outcome"] == "refused" && outcome["code"] == "conflict" {
                        work.push_front(vec![**action]);
                    }
                }
            } else {
                for (index, (action, _)) in pending.iter().enumerate() {
                    results.push(
                        json!({"action":action,"status":"refused","reason":refusal(&reply,index)}),
                    );
                }
            }
        }
        assert_eq!(results.len(), actions.len());
        assert!(
            calls
                .iter()
                .filter(|c| c.tool == "get_timeline_transcript")
                .all(|c| c.arguments.get("range").is_some())
        );
        assert!(
            calls
                .iter()
                .filter(|c| c.tool == "cut_dialogue")
                .all(|c| c.arguments["removals"].as_array().unwrap().len() <= 32)
        );
        assert!(
            book.neighbour_uses > 0,
            "script must reuse returned neighbour addresses"
        );
        let checked = invoke(
            service,
            "check_dialogue_cut",
            json!({"track_id":1,"detail":"all"}),
            calls,
        );
        (results, checked, book.neighbour_uses)
    }
    fn environment(name: &str) -> PathBuf {
        let path = PathBuf::from(
            std::env::var_os(name).unwrap_or_else(|| panic!("set {name} explicitly")),
        );
        assert!(path.is_absolute(), "{name} must be absolute");
        path
    }
    fn handler(
        engine: &Arc<FfmpegMediaEngine>,
        doc: Arc<Document>,
    ) -> (
        KinewrightMcp,
        Arc<CachedDialogueMeasurements>,
        Arc<SessionMedia>,
    ) {
        let core = Core::for_test(Arc::clone(&doc)).unwrap();
        let media = engine.legacy_media(
            MediaSourceOrigin::new_project().legacy_media().unwrap(),
            doc,
        );
        let provider = Arc::new(CachedDialogueMeasurements::new(
            core.clone(),
            Arc::clone(&media),
        ));
        let service = KinewrightMcp::configured(
            core,
            media.clone(),
            media.clone(),
            None,
            ConfirmationBroker::default(),
            false,
            Arc::new(RwLock::new(None)),
            Arc::new(RwLock::new(IncidentLog::default())),
        );
        *service.dialogue_measurements.write().unwrap() = Some(provider.clone());
        (service, provider, media)
    }
    fn write_json(path: &Path, value: &impl Serialize) {
        fs::write(path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
    }
    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "one opt-in end-to-end recording measurement"
    )]
    #[ignore = "on-demand: explicit AMI audio, pinned EN model, private cache and output roots; no downloads"]
    fn dq_media_real_recording_rehearsal() {
        let audio = environment("KINEWRIGHT_REHEARSAL_AUDIO");
        let model = environment("KINEWRIGHT_ASR_MODEL_DIR");
        let data = environment("KINEWRIGHT_REHEARSAL_DATA");
        let output = environment("KINEWRIGHT_REHEARSAL_OUTPUT");
        assert!(output.starts_with(workspace_target_dir().join("dq-media/rehearsal")));
        assert!(
            data.starts_with(&output),
            "cache must be in the explicit owned output tree"
        );
        fs::create_dir_all(&output).unwrap();
        write_json(
            &output.join("stage.json"),
            &json!({"stage":"verify model and prepare private cache"}),
        );
        copy_pinned_english_model(&model, &data).unwrap();
        // Force the unused compositor dependency to CPU software. The audio
        // output is paced in memory; this test never plays or requests a frame.
        let gpu = GpuContext::headless(true).unwrap();
        assert!(gpu.observed_adapter().2, "hardware adapter is forbidden");
        let adapter = gpu.observed_adapter();
        let engine = Arc::new(FfmpegMediaEngine::new_with_paced_test_audio(gpu, data).unwrap());
        let probe = engine.legacy_media(
            MediaSourceOrigin::new_project().legacy_media().unwrap(),
            Arc::new(Document::default()),
        );
        let asset = probe.probe(&audio).unwrap();
        let doc=Arc::new(Document {fps:asset.fps,duration:asset.duration,media_pool:vec![asset.clone()],tracks:vec![Track{ name: None,id:TrackId(1),kind:TrackKind::Audio,sync_lock:true,clips:vec![serde_json::from_value(json!({"id":1,"asset":asset.id,"source_range":{"start":0,"end":asset.duration},"timeline_start":0,"effects":[],"transition_in":null})).unwrap()]}],..Document::default()});
        doc.validate().unwrap();
        drop(probe);
        let identity = MediaIdentity::of(&asset).unwrap();
        let mut runs = Vec::new();
        let mut wires = Vec::new();
        for run in 1..=2 {
            write_json(
                &output.join("stage.json"),
                &json!({"stage":"real transcript job and profile","run":run}),
            );
            let (service, provider, _media) = handler(&engine, Arc::clone(&doc));
            let started = Instant::now();
            let stream = provider.audio_stream(&identity).unwrap();
            let deadline = started + Duration::from_mins(20);
            let measured = loop {
                match provider.get(&identity, stream) {
                    Ok(value) => {
                        assert!(!value.dialogue().words().is_empty());
                        break value;
                    }
                    Err(Refusal::MeasurementPending {
                        status:
                            crate::dialogue_measurements::MeasurementStatus::Queued
                            | crate::dialogue_measurements::MeasurementStatus::Running { .. },
                    }) => {}
                    Err(reason) => panic!("production measurement stopped: {reason:?}"),
                }
                assert!(
                    Instant::now() < deadline,
                    "transcript/profile exceeded 20 minutes"
                );
                thread::sleep(Duration::from_millis(50));
            };
            let measurement_us = started.elapsed().as_micros();
            if run == 1 {
                write_json(
                    &output.join("measurement.json"),
                    &json!({"profile":measured.dialogue().profile(),"words":measured.dialogue().words(),"gaps":measured.dialogue().gap_runs(),"settings":measured.dialogue().settings()}),
                );
            }
            write_json(
                &output.join("stage.json"),
                &json!({"stage":"scripted editor through real dispatcher","run":run}),
            );
            let mut calls = Vec::new();
            let (results, checked, neighbour_uses) =
                execute(&service, measured.dialogue(), &mut calls);
            let (_, after) = service.snapshot().unwrap();
            write_json(&output.join(format!("run{run}-results.json")), &results);
            let mut trace = BufWriter::new(
                fs::File::create(output.join(format!("run{run}-calls.jsonl"))).unwrap(),
            );
            for call in &calls {
                serde_json::to_writer(&mut trace, call).unwrap();
                trace.write_all(b"\n").unwrap();
            }
            trace.flush().unwrap();
            let mut raw = BufWriter::new(
                fs::File::create(output.join(format!("run{run}-replies.jsonl"))).unwrap(),
            );
            for call in &calls {
                raw.write_all(&call.wire).unwrap();
                raw.write_all(b"\n").unwrap();
            }
            raw.flush().unwrap();
            write_json(&output.join(format!("run{run}-document.json")), &after);
            runs.push(json!({"run":run,"neighbour_address_uses":neighbour_uses,"measurement_us":measurement_us,"before_frames":doc.duration,"after_frames":after.duration,"fps":doc.fps,"results":results,"check":checked,"calls":calls.iter().map(|c|json!({"tool":c.tool,"elapsed_us":c.elapsed_us,"reply_bytes":c.reply_bytes})).collect::<Vec<_>>()}));
            wires.push(calls.into_iter().map(|c| c.wire).collect::<Vec<_>>());
            if run == 1 {
                write_json(
                    &output.join("stage.json"),
                    &json!({"stage":"production export audio mix"}),
                );
                write_export_mix_wav(&doc, &output.join("original.wav")).unwrap();
                write_export_mix_wav(&after, &output.join("edited.wav")).unwrap();
            }
        }
        let identical = wires[0] == wires[1];
        write_json(
            &output.join("summary.json"),
            &json!({"audio":audio,"identity":identity,"model_root":model,"adapter":adapter,"warm_byte_identical":identical,"runs":runs}),
        );
        write_json(
            &output.join("stage.json"),
            &json!({"stage":"complete","warm_byte_identical":identical}),
        );
    }
}
