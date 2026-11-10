#[test]
fn te1_media_only_memo_reuse_refuses_replaced_source_before_start() {
    use std::io::{Read as _, Seek as _, Write as _};
    let f = Fixture::with_media(true);
    f.host.runtime.stop();
    let owner = f.host.owner.upgrade().unwrap();
    let request = f.start_request(&f.editor);
    let run: RunId = request.envelope.identity("run").unwrap();
    let (reference, first) = {
        let host = owner.lock().unwrap();
        let store = host.owner().unwrap().store().unwrap();
        let manifest = store.read_input_manifest(&f.brief.inputs).unwrap();
        let reference = kinewright_project::owned_inputs::OwnedInputRef {
            manifest_ref: f.brief.inputs.clone(),
            entry: manifest.entries[0].clone(),
        };
        let first = store.input_lease_request(&reference).unwrap();
        (reference, first)
    };
    let _verified = first.open().unwrap();
    let reuse = owner
        .lock()
        .unwrap()
        .owner()
        .unwrap()
        .store()
        .unwrap()
        .input_lease_request(&reference)
        .unwrap();
    let certificate = reuse.open().unwrap().certified_source().unwrap();
    assert!(
        !certificate.certification_was_cold(),
        "the original verification memo must be retained"
    );
    let object = te1_safety_find_object(f.temp.root(), &reference.entry.object.sha256).unwrap();
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(object)
        .unwrap();
    let length = file.metadata().unwrap().len();
    file.seek(std::io::SeekFrom::End(-1)).unwrap();
    let mut byte = [0];
    file.read_exact(&mut byte).unwrap();
    file.seek(std::io::SeekFrom::End(-1)).unwrap();
    file.write_all(&[byte[0] ^ 0xff]).unwrap();
    file.sync_all().unwrap();
    assert_eq!(file.metadata().unwrap().len(), length);
    assert!(certificate.verify_unchanged().is_err());
    let reply = f
        .editor
        .request(HostCommand::DispatchOrchestration {
            request: Box::new(request),
        })
        .unwrap();
    assert!(
        reply.error.is_some(),
        "TE1_MEMO_REPLACEMENT_STARTED_RUN {reply:?}"
    );
    let host = owner.lock().unwrap();
    assert!(host.hooks.runs.is_empty());
    assert!(
        host.owner()
            .unwrap()
            .orchestration_projection(&run)
            .unwrap()
            .is_none()
    );
}

#[test]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn te1_media_only_claude_wait_reuses_read_and_stop_prevents_publication() {
    use std::sync::{
        Arc, Barrier,
        atomic::{AtomicU8, AtomicUsize, Ordering},
    };
    let (f, run, child, core, mcp, _) = te14_media_fixture();
    f.host.runtime.stop();
    let owner = f.host.owner.upgrade().unwrap();
    let gate = Arc::new((Barrier::new(2), Barrier::new(2)));
    let outcome = Arc::new(AtomicU8::new(0));
    let cold = Arc::new(AtomicUsize::new(0));
    let endpoint = {
        let mut host = owner.lock().unwrap();
        assert!(host.hooks.context_media_wait.is_none());
        host.hooks.context_media_gate = Some(gate.clone());
        host.hooks.context_media_outcome = Some(outcome.clone());
        let store = host.owner().unwrap().store().unwrap();
        store.clear_media_memo_for_fixture().unwrap();
        let cold = cold.clone();
        store
            .hold_next_cold_media_for_fixture(Arc::new(move || {
                cold.fetch_add(1, Ordering::AcqRel);
            }))
            .unwrap();
        host.hooks.runs[&run].tasks[&child].endpoint()
    };
    let request = |frame| {
        serde_json::to_value(EvidenceRead {
            revision: grant_test_revision(&core),
            evidence: EvidenceKind::Frame {
                clip: kinewright_core::ClipId(3),
                source_frame: frame,
            },
            cursor: None,
        })
        .unwrap()
    };
    let first_request = request(0);
    let before = f.projection(&run).tasks[1].progress.decisions.len();
    let started = Instant::now();
    let first = std::thread::scope(|scope| {
        let read = scope.spawn(|| {
            mcp.runtime.block_on(async {
                tokio::time::timeout(
                    Duration::from_secs(45),
                    mcp.client.call_tool(
                        rmcp::model::CallToolRequestParams::new("read_task_evidence")
                            .with_arguments(first_request.as_object().unwrap().clone()),
                    ),
                )
                .await
            })
        });
        gate.0.wait();
        let first = read.join().unwrap();
        assert!(endpoint.media_read_pending_for_fixture());
        gate.1.wait();
        first
    })
    .expect("media response exceeded Claude's stricter 45-second witness limit")
    .unwrap();
    let first_seconds = started.elapsed().as_secs_f64();
    assert!(
        first_seconds < 45.0,
        "TE1_MEDIA_FIRST_RESPONSE {first_seconds}"
    );
    assert_eq!(first.is_error, Some(true));
    assert!(serde_json::to_string(&first).unwrap().contains("Media source verification or an earlier read is still running. Retry shortly; no page is charged until a sample is returned."));
    assert_eq!(f.projection(&run).tasks[1].progress.decisions.len(), before);
    let until = Instant::now() + Duration::from_secs(5);
    while endpoint.media_read_pending_for_fixture() && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(
        outcome.load(Ordering::Acquire),
        1,
        "timed-out read must finish rather than cancel"
    );
    let unexpected = Arc::new(AtomicU8::new(0));
    owner.lock().unwrap().hooks.context_media_outcome = Some(unexpected.clone());
    let second = te14_outcome(mcp.wire("read_task_evidence", first_request.clone()));
    assert!(
        serde_json::to_string(&second)
            .unwrap()
            .contains("owned_frame_sample")
    );
    assert_eq!(
        unexpected.load(Ordering::Acquire),
        0,
        "TE1_MEDIA_RETRY_RESTARTED_READ"
    );
    assert_eq!(
        cold.load(Ordering::Acquire),
        0,
        "TE1_MEDIA_RECERTIFIED_RUN_INPUT"
    );
    assert_eq!(
        f.projection(&run).tasks[1].progress.decisions.len(),
        before + 1
    );
    let replay = te14_outcome(mcp.wire("read_task_evidence", first_request));
    assert_eq!(second, replay);
    assert_eq!(
        f.projection(&run).tasks[1].progress.decisions.len(),
        before + 1
    );

    let stop_gate = Arc::new((Barrier::new(2), Barrier::new(2)));
    let stopped_outcome = Arc::new(AtomicU8::new(0));
    {
        let mut host = owner.lock().unwrap();
        host.hooks.context_media_wait = Some(Duration::from_millis(100));
        host.hooks.context_media_gate = Some(stop_gate.clone());
        host.hooks.context_media_outcome = Some(stopped_outcome.clone());
    }
    let stopped_request = request(1);
    let (retry, stop) = std::thread::scope(|scope| {
        let read = scope.spawn(|| mcp.wire("read_task_evidence", stopped_request.clone()));
        stop_gate.0.wait();
        let retry = read.join().unwrap();
        let stop = f.control(&f.editor, &run, RunAction::Stop);
        let stopped = call(
            &f,
            &f.editor,
            HostCommand::DispatchOrchestration {
                request: Box::new(stop),
            },
            0,
        );
        stop_gate.1.wait();
        (retry, stopped)
    });
    assert!(
        serde_json::to_string(&retry)
            .unwrap()
            .contains("Retry shortly")
    );
    assert_eq!(
        stop.orchestration.unwrap().decision,
        CommandDecision::Applied
    );
    let until = Instant::now() + Duration::from_secs(5);
    while endpoint.media_read_pending_for_fixture() && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(!endpoint.media_read_pending_for_fixture());
    assert_eq!(stopped_outcome.load(Ordering::Acquire), 2);
    let refused = mcp.wire("read_task_evidence", stopped_request);
    assert_eq!(refused.is_error, Some(true));
    assert_eq!(
        f.projection(&run).tasks[1].progress.decisions.len(),
        before + 1,
        "TE1_MEDIA_PUBLISHED_AFTER_STOP"
    );
    println!(
        "TE1_MEDIA_ONLY first_response_seconds={first_seconds:.6} cold_certifications={} retry_decodes={} stopped_publications=0",
        cold.load(Ordering::Acquire),
        unexpected.load(Ordering::Acquire)
    );
}

#[test]
#[ignore = "on-demand: native maximum-size non-media cost inventory"]
fn te1_media_only_profile_non_media_steps() {
    let (f, run, _, core, _, _) = te14_media_fixture();
    f.host.runtime.stop();
    te1_media_nonmedia_estimates(&f, &run, &core);
}

fn te1_media_nonmedia_document(
    template: &kinewright_core::Document,
    count: usize,
    chain: bool,
) -> kinewright_core::Document {
    use kinewright_core::{
        AssetId, ClipId, SyncGroup, SyncGroupId, SyncGroupMember, TimeCode, TrackId,
    };
    let track = template
        .tracks
        .iter()
        .find(|track| track.kind == kinewright_core::TrackKind::Video)
        .unwrap();
    let clip = &track.clips[0];
    let asset = template.asset(clip.asset).unwrap();
    let mut document = template.clone();
    document.tracks.clear();
    document.media_pool.clear();
    document.catalog = kinewright_core::MediaCatalog::default();
    document.duration =
        TimeCode(clip.timeline_start.0 + clip.source_range.end.0 - clip.source_range.start.0);
    for index in 0..count {
        let id = 100 + index as u64;
        let mut asset = asset.clone();
        asset.id = AssetId(id);
        let mut clip = clip.clone();
        clip.id = ClipId(id);
        clip.asset = asset.id;
        let mut track = track.clone();
        track.id = TrackId(id);
        track.sync_lock = false;
        track.clips = vec![clip];
        document.media_pool.push(asset);
        document.tracks.push(track);
        if chain && index > 0 {
            document.catalog.sync_groups.push(SyncGroup {
                id: SyncGroupId(id),
                name: format!("chain-{index}"),
                members: vec![
                    SyncGroupMember {
                        asset: AssetId(id - 1),
                        offset: TimeCode(0),
                        angle_name: "a".into(),
                    },
                    SyncGroupMember {
                        asset: AssetId(id),
                        offset: TimeCode(0),
                        angle_name: "b".into(),
                    },
                ],
            });
        }
    }
    document.validate().unwrap();
    document
}

fn te1_media_nonmedia_charge(value: &serde_json::Value) -> u64 {
    64 + match value {
        serde_json::Value::String(value) => value.len() as u64,
        serde_json::Value::Array(values) => values.iter().map(te1_media_nonmedia_charge).sum(),
        serde_json::Value::Object(values) => values
            .iter()
            .map(|(key, value)| 128 + key.len() as u64 + te1_media_nonmedia_charge(value))
            .sum(),
        _ => 0,
    }
}

fn te1_media_nonmedia_estimates(f: &Fixture, run: &RunId, core: &Core) {
    let template = core.export_history().unwrap().document;
    let history_cost = |document: &kinewright_core::Document| {
        let value = serde_json::to_value(kinewright_core::HistorySnapshot {
            floor_offset: 0,
            baseline: Arc::new(document.clone()),
            document: Arc::new(document.clone()),
            batches: vec![],
            cursor: kinewright_core::HistoryCursor {
                undo: vec![],
                redo: vec![],
            },
        })
        .unwrap();
        let bytes = serde_json::to_vec(&value).unwrap();
        2 * bytes.len() as u64 + te1_media_nonmedia_charge(&value)
    };
    let owner = f.host.owner.upgrade().unwrap();
    let projection = f.projection(run);
    let host = owner.lock().unwrap();
    let mut task = projection.tasks[1].clone();
    for chain in [false, true] {
        let mut maximum = 1;
        while history_cost(&te1_media_nonmedia_document(&template, maximum + 1, chain))
            <= kinewright_project::run_admission::HISTORY_BYTES
        {
            maximum += 1;
        }
        let document = te1_media_nonmedia_document(&template, maximum, chain);
        task.brief.scope.objects = if chain {
            [format!("clip:{}", 99 + maximum)].into()
        } else {
            std::collections::BTreeSet::default()
        };
        let started = Instant::now();
        let overview = host
            .context_overview(run, &task, kinewright_core::TimelineRevision(0), &document)
            .unwrap();
        let seconds = started.elapsed().as_secs_f64();
        assert!(overview.is_object());
        println!(
            "TE1_NONMEDIA_TIMELINE chain={chain} clips={maximum} charged_history={} overview_seconds={seconds:.6}",
            history_cost(&document)
        );
        te1_media_nonmedia_policy(&document, task.brief.scope.base.clone(), chain);
    }
    drop(host);
    te1_media_nonmedia_linear();
}

fn te1_media_nonmedia_policy(
    document: &kinewright_core::Document,
    base: crate::orchestration::orchestration_records::BranchBinding,
    chain: bool,
) {
    let policy = crate::orchestration::policy::TaskPolicy {
        role: Role::Editor,
        scope: crate::orchestration::policy::director_scope(document, base).unwrap(),
    };
    let marker = &document.markers[0];
    let operations = vec![
        kinewright_core::Operation::SetMarkerParam {
            marker: marker.id,
            name: "label".into(),
            value: kinewright_core::ParamValue::Text(marker.label.clone()),
        };
        128
    ];
    let started = Instant::now();
    policy.validate_batch(document, &operations).unwrap();
    let batch_seconds = started.elapsed().as_secs_f64();
    let operation_bytes = serde_json::to_vec(&operations[0]).unwrap().len();
    let maximum_request_ops = u32::try_from(((4 << 20) - 1024) / (operation_bytes + 1)).unwrap();
    let estimated_validation_seconds = batch_seconds * f64::from(maximum_request_ops) / 128.0;
    let private = Core::spawn(document.clone()).unwrap();
    let started = Instant::now();
    private
        .restore_history(kinewright_core::HistorySnapshot {
            floor_offset: 0,
            baseline: Arc::new(document.clone()),
            document: Arc::new(document.clone()),
            batches: vec![],
            cursor: kinewright_core::HistoryCursor {
                undo: vec![],
                redo: vec![],
            },
        })
        .unwrap();
    let restore_seconds = started.elapsed().as_secs_f64();
    println!(
        "TE1_NONMEDIA_POLICY chain={chain} operations=128 validate_seconds={batch_seconds:.6} empty_journal_restore_seconds={restore_seconds:.6} operation_bytes={operation_bytes} maximum_request_ops={maximum_request_ops} estimated_validation_seconds={estimated_validation_seconds:.6}"
    );
}

fn te1_media_nonmedia_linear() {
    let sample = serde_json::json!({"rows":vec![serde_json::json!({"text":"x".repeat(128),"count":1}); 32768]});
    let started = Instant::now();
    let bytes = serde_json::to_vec(&sample).unwrap();
    let serialize = started.elapsed().as_secs_f64();
    let started = Instant::now();
    let hash = kinewright_media::sha256_bytes(&bytes);
    let hashing = started.elapsed().as_secs_f64();
    let started = Instant::now();
    let decoded: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let parse = started.elapsed().as_secs_f64();
    let started = Instant::now();
    let cloned = decoded.clone();
    let clone = started.elapsed().as_secs_f64();
    assert_eq!(cloned["rows"].as_array().unwrap().len(), 32768);
    assert_eq!(hash.len(), 64);
    println!(
        "TE1_NONMEDIA_LINEAR bytes={} serialize_seconds={serialize:.6} hash_seconds={hashing:.6} parse_seconds={parse:.6} clone_seconds={clone:.6}",
        bytes.len()
    );
}
