use super::super::task_context::{ChildRead, EvidenceKind, EvidenceRead};

fn te14_read(
    mcp: &C4Mcp,
    revision: kinewright_core::TimelineRevision,
    evidence: EvidenceKind,
    cursor: Option<String>,
) -> rmcp::model::CallToolResult {
    mcp.wire(
        "read_task_evidence",
        serde_json::to_value(EvidenceRead {
            revision,
            evidence,
            cursor,
        })
        .unwrap(),
    )
}
fn te14_outcome(reply: rmcp::model::CallToolResult) -> serde_json::Value {
    assert_eq!(reply.is_error, Some(false), "TE14_READ_ALLOWED {reply:?}");
    reply.structured_content.unwrap()["outcome"].clone()
}
fn te14_grant(f: &mut Fixture, data: &serde_json::Value) -> ObjectRef {
    f.host
        .owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .owner_mut()
        .unwrap()
        .admit_host_grant(&serde_json::to_vec(data).unwrap())
        .unwrap()
}
fn te14_document() -> Document {
    let mut doc = Document::default();
    let cues = (0..80)
        .map(|n| kinewright_core::CaptionCue {
            start: kinewright_core::TimeCode(n * 10),
            end: kinewright_core::TimeCode(n * 10 + 8),
            text: format!("caption {n}"),
        })
        .collect::<Vec<_>>();
    let ops = kinewright_core::caption_title_operations(
        &doc,
        &cues,
        kinewright_core::CaptionPreset::Clean,
    )
    .unwrap();
    kinewright_core::apply_batch(&mut doc, &ops).unwrap();
    for clip in &mut doc.tracks[0].clips {
        clip.content = kinewright_core::ClipContent::Solid(kinewright_core::SolidColor::default());
    }
    doc.tracks[0].clips[0].link = Some(kinewright_core::LinkId(7));
    doc.tracks[0].clips[79].link = Some(kinewright_core::LinkId(7));
    doc
}
#[test]
fn te14_scoped_context_expands_the_outside_view_dependency() {
    let doc = te14_document();
    let last = doc.tracks[0].clips[79].id;
    let mut f = Fixture::with_document(false, doc.clone());
    f.brief.scope =
        crate::orchestration::policy::director_scope(&doc, f.brief.scope.base.clone()).unwrap();
    let first_key = format!("clip:{}", doc.tracks[0].clips[0].id);
    f.brief
        .scope
        .objects
        .retain(|key| !key.starts_with("clip:") || *key == first_key);
    let (run, parent, mcp) = c4_director(&f);
    let initial = te14_outcome(mcp.wire("get_timeline_state", serde_json::json!({})));
    let initial_omitted = initial["context"]["omitted"] == true;
    assert!(
        !initial["context"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["clip"]["id"] == last.0),
        "TE14_DEPENDENCY_OUTSIDE_INITIAL_VIEW"
    );
    let next = initial["next_read"].clone();
    let has_cursor = next["cursor"].is_string();
    if has_cursor {
        let continued = mcp.wire("read_task_evidence", next);
        assert_eq!(
            continued.is_error,
            Some(false),
            "TE14_INITIAL_CURSOR_EXPANDS"
        );
    }
    let before = f.projection(&run).tasks[0].brief.scope.clone();
    let mut cursor = None;
    let mut seen = false;
    let mut pages = 0;
    loop {
        let reply = te14_read(
            &mcp,
            kinewright_core::TimelineRevision(0),
            EvidenceKind::Timeline {
                clips: vec![doc.tracks[0].clips[0].id],
            },
            cursor,
        );
        assert!(
            serde_json::to_vec(&reply).unwrap().len() <= 24 * 1024,
            "TE14_PAGE_WIRE_BOUND"
        );
        let value = te14_outcome(reply);
        pages += 1;
        seen |= value["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["clip"]["id"] == last.0);
        cursor = value["next_cursor"].as_str().map(str::to_owned);
        if cursor.is_none() {
            break;
        }
    }
    let full_reject = doc.tracks[0].clips.iter().any(|c| {
        c.id == last && c.link.is_some() && !before.objects.contains(&format!("clip:{}", c.id))
    });
    assert_eq!(seen, full_reject, "TE14_FULL_SCOPED_SAME_REJECTION");
    assert!(initial_omitted, "TE14_INITIAL_OMISSION");
    assert!(has_cursor, "TE14_INITIAL_CURSOR_AVAILABLE");
    assert!(seen && pages > 1, "TE14_LEGITIMATE_EXPANSION");
    assert_eq!(
        f.projection(&run).tasks[0].brief.scope,
        before,
        "TE14_READ_CANNOT_EXPAND_EDIT_AUTHORITY"
    );
    let core = te13_core(&f, &run, &parent.task);
    assert!(core.export_history().unwrap().cursor.undo.is_empty());
}
#[test]
fn te14_neighbouring_words_match_the_full_transcript_decision() {
    let mut f = Fixture::new();
    let transcript = serde_json::json!({"words":["we","did","not","approve","this","cut"]});
    let reference = te14_grant(&mut f, &transcript);
    f.brief.script = reference.clone();
    let (_, _, mcp) = c4_director(&f);
    let full = te14_outcome(te14_read(
        &mcp,
        kinewright_core::TimelineRevision(0),
        EvidenceKind::Reference {
            reference: reference.clone(),
        },
        None,
    ));
    let scoped = te14_outcome(te14_read(
        &mcp,
        kinewright_core::TimelineRevision(0),
        EvidenceKind::Transcript {
            reference,
            start: 3,
            end: 4,
        },
        None,
    ));
    let original = full["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["chunk"]["text"].as_str().unwrap())
        .collect::<String>();
    let original: serde_json::Value = serde_json::from_str(&original).unwrap();
    let full_reject = original["words"]
        .as_array()
        .unwrap()
        .iter()
        .any(|w| w == "not");
    let scoped_reject = scoped["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|w| w["word"] == "not" && w["neighbour"] == true);
    assert!(full_reject, "TE14_FULL_NEIGHBOUR_REJECT");
    assert_eq!(scoped_reject, full_reject, "TE14_NEIGHBOUR_DECISION");
}
#[test]
fn te14_stale_revision_and_cursor_are_refused() {
    let r = c5_fix10_run();
    let stale = te14_read(
        &r.editor,
        kinewright_core::TimelineRevision(0),
        EvidenceKind::Timeline { clips: vec![] },
        None,
    );
    assert_eq!(stale.is_error, Some(true), "TE14_STALE_REVISION_DENIED");
    let wrong = te14_read(
        &r.editor,
        grant_test_revision(&r.core),
        EvidenceKind::Timeline { clips: vec![] },
        Some(format!("{}:0", "0".repeat(64))),
    );
    assert_eq!(wrong.is_error, Some(true), "TE14_UNRELATED_CURSOR_DENIED");
}
#[test]
fn te14_page_expansion_limit_and_reconnect_are_durable() {
    let mut f = Fixture::new();
    let reference = te14_grant(&mut f, &serde_json::json!({"text":"x".repeat(180_000)}));
    f.brief.script = reference.clone();
    let (run, parent, mcp) = c4_director(&f);
    let mut cursor = None;
    for n in 0..8 {
        let reply = te14_read(
            &mcp,
            kinewright_core::TimelineRevision(0),
            EvidenceKind::Reference {
                reference: reference.clone(),
            },
            cursor.clone(),
        );
        assert!(
            serde_json::to_vec(&reply).unwrap().len() <= 24 * 1024,
            "TE14_PAGE_LIMIT {n}"
        );
        let value = te14_outcome(reply);
        cursor = value["next_cursor"].as_str().map(str::to_owned);
    }
    {
        let state = f.projection(&run);
        let host = f.host.owner.upgrade().unwrap();
        let host = host.lock().unwrap();
        assert_eq!(
            host.expansion_count(&state.tasks[0]).unwrap(),
            8,
            "TE14_DURABLE_EXPANSION_COUNT"
        );
    }
    let reconnect = C4Mcp::new(&f, &run, &parent.task);
    let reply = te14_read(
        &reconnect,
        kinewright_core::TimelineRevision(0),
        EvidenceKind::Reference {
            reference: reference.clone(),
        },
        cursor,
    );
    assert_eq!(reply.is_error, Some(true), "TE14_EIGHT_EXPANSIONS");
    let retry = te14_read(
        &reconnect,
        kinewright_core::TimelineRevision(0),
        EvidenceKind::Reference { reference },
        None,
    );
    assert_eq!(
        retry.is_error,
        Some(false),
        "TE14_FIX1_RETRY_AT_CAPACITY_FREE"
    );
    let first = te14_outcome(retry);
    assert!(first["page_id"].is_string(), "TE14_RETRY_AT_CAPACITY_FREE");
    let text = serde_json::to_string(&reply.content).unwrap();
    assert!(
        text.contains("evidence_capacity") && text.contains("replan without guessing"),
        "TE14_CAPACITY_REPLAN"
    );
    let state = f.projection(&run);
    let host = f.host.owner.upgrade().unwrap();
    let host = host.lock().unwrap();
    assert_eq!(
        host.expansion_count(&state.tasks[0]).unwrap(),
        8,
        "TE14_DURABLE_EXPANSION_COUNT"
    );
}
fn te14_published_report(
    unmet: bool,
    failed: bool,
    padding: usize,
) -> (C5Fix10Run, ObjectRef, TaskResult) {
    let mut r = c5_fix10_run();
    let mut result = c5_fix10_freeze(&r.director, &r.child);
    let report = serde_json::json!({"kind":"task_result_report_v1","partial_coverage":unmet||failed,"satisfied_note_ids":["note-1"],"unmet_note_ids":if unmet{vec!["note-2"]}else{vec![]},"conflicts":[],"proof_status":if failed{"failed"}else{"reported_pass"},"proof_failures":if failed{vec!["caption alignment"]}else{vec![]},"changed_scope":["marker:1"],"padding":"p".repeat(padding)});
    result.explanation = te14_grant(&mut r.f, &report);
    r.director.accepted(
        "publish_result",
        OrchestrationCommand::PublishResult(Box::new(PublishResult {
            task: r.child.clone(),
            result: result.clone(),
        })),
    );
    let reference = r.f.projection(&r.run).results.last().unwrap().clone();
    (r, reference, result)
}
#[test]
fn te14_child_details_bind_parent_child_run_reference_and_source() {
    let (r, reference, result) = te14_published_report(true, false, 0);
    let read = ChildRead {
        run: r.run.clone(),
        child: r.child.clone(),
        reference,
        source: result.head,
        cursor: None,
    };
    let success = r
        .director
        .wire("read_child_result", serde_json::to_value(&read).unwrap());
    assert_eq!(
        success.is_error,
        Some(false),
        "TE14_CHILD_EXPANSION_ALLOWED"
    );
    let mut wrong = read.clone();
    wrong.source.as_mut().unwrap().head = identity();
    assert_eq!(
        r.director
            .wire("read_child_result", serde_json::to_value(&wrong).unwrap())
            .is_error,
        Some(true),
        "TE14_CHILD_STALE_SOURCE_DENIED"
    );
    wrong = read.clone();
    wrong.child = r.parent.task.clone();
    assert_eq!(
        r.director
            .wire("read_child_result", serde_json::to_value(&wrong).unwrap())
            .is_error,
        Some(true),
        "TE14_UNRELATED_CHILD_DENIED"
    );
    wrong = read.clone();
    wrong.run = identity();
    assert_eq!(
        r.director
            .wire("read_child_result", serde_json::to_value(&wrong).unwrap())
            .is_error,
        Some(true),
        "TE14_CHILD_RUN_DENIED"
    );
    let other = Fixture::new();
    let (_, _, other_parent) = c4_director(&other);
    assert_eq!(
        other_parent
            .wire("read_child_result", serde_json::to_value(&read).unwrap())
            .is_error,
        Some(true),
        "TE14_FOREIGN_CHILD_DENIED"
    );
    // A sibling's authenticated endpoint gets no child-result authority.
    let mut brief = r.f.brief.clone();
    brief.role = Role::Editor;
    r.director.accepted(
        "assign_task",
        OrchestrationCommand::AssignTask(Box::new(AssignTask {
            run: r.run.clone(),
            parent: r.parent.task.clone(),
            brief,
        })),
    );
    let sibling_id = r.f.projection(&r.run).tasks.last().unwrap().id.clone();
    c4_reserve_task(&r.f, &r.run, &sibling_id);
    let sibling = C4Mcp::new(&r.f, &r.run, &sibling_id);
    assert_eq!(
        sibling
            .wire("read_child_result", serde_json::to_value(&read).unwrap())
            .is_error,
        Some(true),
        "TE14_SIBLING_DETAIL_DENIED"
    );
    assert_eq!(
        sibling
            .wire(
                "read_task_result",
                serde_json::json!({"reference":read.reference})
            )
            .is_error,
        Some(true),
        "TE14_SIBLING_LEGACY_DETAIL_DENIED"
    );
    // Also reach the production parent/child guard independently of the role
    // manifest. Use the real live sibling attachment and durable projection.
    let state = r.f.projection(&r.run);
    let owner = r.f.host.owner.upgrade().unwrap();
    let mut host = owner.lock().unwrap();
    let index = host
        .attachments
        .iter()
        .position(|attachment| {
            attachment
                .producer_scope
                .as_ref()
                .is_some_and(|scope| scope.task.as_ref() == Some(&sibling_id))
        })
        .unwrap();
    let record = state
        .tasks
        .iter()
        .find(|task| task.id == sibling_id)
        .unwrap();
    assert!(
        host.read_child_context(index, &state, record, &read)
            .is_err(),
        "TE14_SIBLING_BINDING_DENIED"
    );
}
#[test]
fn te14_merge_uses_durable_delta_and_history_after_a_small_wake() {
    let (r, reference, result) = te14_published_report(false, false, 24_000);
    let state = r.f.projection(&r.run);
    let host = r.f.host.owner.upgrade().unwrap();
    let summary = host
        .lock()
        .unwrap()
        .context_result_wake(&state, &r.parent.task, &[&reference])
        .unwrap();
    assert!(summary.len() < 24 * 1024);
    let before = r.core.export_history().unwrap();
    let mut forged = result.clone();
    forged.delta_ref = Some(r.f.brief.plan.clone());
    let publish = r.director.wire(
        "publish_result",
        c5_fix10_publish(&r.director, &r.child, forged),
    );
    assert_eq!(
        publish.is_error,
        Some(true),
        "TE14_FULL_DURABLE_RECORD_REQUIRED"
    );
    let mut plausible_summary = result;
    plausible_summary.delta_ref = None;
    let summary_ref = {
        let mut host = host.lock().unwrap();
        host.owner_mut()
            .unwrap()
            .admit_scheduler_record(&OrchestrationRecord {
                version: 1,
                payload: OrchestrationPayload::Result(Box::new(plausible_summary)),
            })
            .unwrap()
            .reference
    };
    let merge = r.director.wire(
        "merge_task",
        r.director
            .control(OrchestrationCommand::MergeTask(MergeTask {
                task: r.child.clone(),
                target: r.f.brief.scope.base.clone(),
                result: summary_ref,
            })),
    );
    assert_eq!(
        merge.is_error,
        Some(true),
        "TE14_SUMMARY_CANNOT_AUTHORIZE_MERGE"
    );
    assert_eq!(
        r.core.export_history().unwrap(),
        before,
        "TE14_MERGE_HISTORY_UNCHANGED"
    );
}
#[test]
fn te14_broader_read_never_widens_edit_authority() {
    let r = c5_fix10_run();
    let before = r.core.export_history().unwrap();
    te14_outcome(te14_read(
        &r.editor,
        grant_test_revision(&r.core),
        EvidenceKind::Timeline { clips: vec![] },
        None,
    ));
    let prepared=r.editor.wire("prepare_edit_plan",serde_json::json!({"expected_revision":grant_test_revision(&r.core),"operations":[{"op":"add_marker","marker":{"id":2,"position":2,"label":"outside authority","color_token":0}}]}));
    assert_eq!(
        prepared.is_error,
        Some(true),
        "TE14_CONTEXT_IS_NOT_EDIT_AUTHORITY"
    );
    assert_eq!(
        r.core.export_history().unwrap(),
        before,
        "TE14_CONTEXT_PRESERVES_HISTORY"
    );
}
#[test]
fn te14_fixed_prefix_is_identical_through_both_protocols() {
    let mut prefixes = Vec::new();
    for dialect in ["scripted", "scripted-claude"] {
        let mut f = Fixture::new();
        if dialect == "scripted-claude" {
            f.brief.provider = super::super::studio_runtime::fixture::claude_fixture_selection();
        }
        let (run, execution, _) = reserve(&f);
        let admitted = native_admitted(&f, &execution);
        let (profile, _, scratch) = native_profile_for(f.temp.root(), &admitted, dialect);
        let recipe = profile.clone();
        let invocation = native_charge(&f, &admitted, profile);
        let program = crate::native_launcher::program(
            &recipe,
            std::str::from_utf8(invocation.invocation().bytes()).unwrap(),
        )
        .unwrap();
        assert_eq!(
            program.arguments.last().unwrap().as_bytes(),
            invocation.invocation().bytes(),
            "TE14_PROTOCOL_FINAL_ARGUMENT"
        );
        let prefix = super::super::task_context::application_prefix(Role::Director).unwrap();
        assert!(
            invocation.invocation().bytes().starts_with(&prefix),
            "TE14_PROTOCOL_FIXED_PREFIX {dialect}"
        );
        let prompt: serde_json::Value =
            serde_json::from_slice(invocation.invocation().bytes()).unwrap();
        assert_eq!(
            prompt["manifest_sha256"],
            crate::server::task_manifest_identity(Role::Director).unwrap(),
            "TE14_PROTOCOL_MANIFEST {dialect}"
        );
        let state = f.projection(&run);
        let host = f.host.owner.upgrade().unwrap();
        let host = host.lock().unwrap();
        let accounting = host.current_accounting_reference(&state).unwrap();
        let record = host
            .owner()
            .unwrap()
            .store()
            .unwrap()
            .read_run_record(&accounting)
            .unwrap();
        let OrchestrationPayload::Accounting(accounting) = record.payload else {
            panic!("TE14_ACCOUNTING");
        };
        let measurement = &accounting.tasks[0].measurements.as_ref().unwrap().sends[0];
        assert_eq!(
            measurement.prefix_identity,
            kinewright_media::sha256_bytes(&prefix),
            "TE14_PREFIX_MEASURED_SEPARATELY"
        );
        prefixes.push(prefix);
        drop(invocation);
        drop(scratch);
    }
    assert_eq!(prefixes[0], prefixes[1], "TE14_PROTOCOL_PREFIX_EQUAL");
}

#[test]
fn te14_prefix_bytes_are_stable_across_distinct_tasks_of_each_role() {
    let f = Fixture::new();
    let (run, _, _) = reserve(&f);
    let original = f.projection(&run).tasks[0].clone();
    let host = f.host.owner.upgrade().unwrap();
    let host = host.lock().unwrap();
    for role in [
        Role::Director,
        Role::Editor,
        Role::Motion,
        Role::Audio,
        Role::Colour,
        Role::Reviewer,
    ] {
        let mut a = original.clone();
        a.brief.role = role;
        a.id = identity();
        let mut b = a.clone();
        b.id = identity();
        b.brief.provider.model = "other person choice".into();
        let prefix = super::super::task_context::application_prefix(role).unwrap();
        let first = host
            .codex_prompt(&a, b"brief A", &te12_fixed_context())
            .unwrap();
        let second = host
            .codex_prompt(&b, b"brief B", &te12_fixed_context())
            .unwrap();
        assert_ne!(first, second, "TE14_VARIABLE_TASKS_DIFFER");
        assert_eq!(
            &first[..prefix.len()],
            &second[..prefix.len()],
            "TE14_ROLE_TASK_PREFIX_STABLE"
        );
        assert!(
            first.starts_with(&prefix) && second.starts_with(&prefix),
            "TE14_PREFIX_PRECEDES_VARIABLE_TASK"
        );
    }
}
fn te14_media_fixture() -> (Fixture, RunId, TaskId, Core, C4Mcp, std::path::PathBuf) {
    use kinewright_core::{AssetId, Operation, TimeCode, TrackId};
    let mut f = Fixture::with_media(true);
    let video = kinewright_media::test_support::GeneratedMedia::ffmpeg(
        "te14-owned-video",
        &[
            "-f",
            "lavfi",
            "-i",
            "color=c=0x204060:s=16x16:d=1",
            "-c:v",
            "libx264",
            "-x264-params",
            "colorprim=bt709:transfer=bt709:colormatrix=bt709",
            "-pix_fmt",
            "yuv420p",
            "-color_primaries",
            "bt709",
            "-color_trc",
            "bt709",
            "-colorspace",
            "bt709",
            "-color_range",
            "tv",
        ],
        "mkv",
    );
    let stereo = kinewright_media::test_support::GeneratedMedia::ffmpeg(
        "te14-planar-audio",
        &[
            "-f",
            "lavfi",
            "-i",
            "aevalsrc=0|0.75*sin(880*2*PI*t):s=48000:d=0.5",
            "-c:a",
            "aac",
            "-b:a",
            "128k",
        ],
        "m4a",
    );
    let png = video.path();
    let mut document = {
        let owner = f.host.owner.upgrade().unwrap();
        let mut host = owner.lock().unwrap();
        let owner = host.owner_mut().unwrap();
        owner.import_owned_media(png).unwrap();
        owner.import_owned_media(stereo.path()).unwrap();
        owner.save_active().unwrap();
        let mut candidate = owner.fork_durable_branch("te14-media").unwrap();
        owner.persist_durable_branch(&mut candidate).unwrap();
        f.brief.scope.base = owner.candidate_binding(candidate.branch_id()).unwrap();
        f.brief.inputs = owner.current_inputs_ref().clone();
        (*owner.core().export_history().unwrap().document).clone()
    };
    let operations = vec![
        Operation::AddTrack {
            track: kinewright_core::Track {
                id: TrackId(1),
                kind: kinewright_core::TrackKind::Audio,
                sync_lock: true,
                clips: vec![],
            },
        },
        Operation::AddClip {
            track: TrackId(1),
            asset: AssetId(1),
            at: TimeCode(0),
            source: TimeCode(0)..TimeCode(1),
        },
        Operation::AddClip {
            track: TrackId(1),
            asset: AssetId(3),
            at: TimeCode(2),
            source: TimeCode(0)..TimeCode(1),
        },
        Operation::AddTrack {
            track: kinewright_core::Track {
                id: TrackId(2),
                kind: kinewright_core::TrackKind::Video,
                sync_lock: true,
                clips: vec![],
            },
        },
        Operation::AddClip {
            track: TrackId(2),
            asset: AssetId(2),
            at: TimeCode(0),
            source: TimeCode(0)..TimeCode(1),
        },
    ];
    assert_eq!(document.media_pool[0].id, AssetId(1));
    kinewright_core::apply_batch(&mut document, &operations).unwrap();
    {
        let host = f.host.owner.upgrade().unwrap();
        let mut host = host.lock().unwrap();
        let owner = host.owner_mut().unwrap();
        owner
            .core()
            .request(kinewright_core::Command::DoBatch(operations))
            .unwrap();
        owner.save_active().unwrap();
        let mut candidate = owner.fork_durable_branch("te14-media-timeline").unwrap();
        owner.persist_durable_branch(&mut candidate).unwrap();
        f.brief.scope.base = owner.candidate_binding(candidate.branch_id()).unwrap();
    }
    // Asset-bearing edits remain behind the existing host capability boundary.
    // The task's real edit is a marker; media already belongs to its saved base.
    kinewright_core::apply_batch(&mut document, &[history_marker(1)]).unwrap();
    let operations = vec![history_marker(1)];
    f.brief.scope =
        crate::orchestration::policy::director_scope(&document, f.brief.scope.base.clone())
            .unwrap();
    let (run, parent, director) = c4_director(&f);
    let mut brief = f.brief.clone();
    brief.role = Role::Editor;
    director.accepted(
        "assign_task",
        OrchestrationCommand::AssignTask(Box::new(AssignTask {
            run: run.clone(),
            parent: parent.task.clone(),
            brief,
        })),
    );
    let child = f.projection(&run).tasks[1].id.clone();
    let (execution, actor, endpoint, core) = c4_reserve_task(&f, &run, &child);
    history_turn(&f, &execution, &actor, &endpoint, 1, true);
    let mcp = C4Mcp::new(&f, &run, &child);
    let revision = grant_test_revision(&core);
    let prepared = te14_outcome(mcp.wire(
        "prepare_edit_plan",
        serde_json::json!({"expected_revision":revision,"operations":operations}),
    ));
    let committed = mcp.wire(
        "commit_edit_plan",
        serde_json::json!({"expected_revision":revision,"plan_id":prepared["plan_id"]}),
    );
    assert_eq!(
        committed.is_error,
        Some(false),
        "TE14_OWNED_MEDIA_COMMIT {committed:?}"
    );
    (f, run, child, core, mcp, png.to_path_buf())
}
#[test]
fn te14_real_owned_audio_and_frame_samples_exclude_ambient_paths() {
    use base64::Engine as _;
    use kinewright_core::{ClipId, TimeCode};
    let (f, run, child, core, mcp, png) = te14_media_fixture();
    for evidence in [
        EvidenceKind::Audio {
            clip: ClipId(1),
            source_frame: 0,
        },
        EvidenceKind::Frame {
            clip: ClipId(3),
            source_frame: 0,
        },
        EvidenceKind::Audio {
            clip: ClipId(2),
            source_frame: 4,
        },
    ] {
        let value = te14_outcome(te14_read(&mcp, grant_test_revision(&core), evidence, None));
        let text = serde_json::to_string(&value).unwrap();
        let sample = if value["items"][0]["kind"] == "json_fragment" {
            let text = value["items"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|v| v["kind"] == "json_fragment" && v["row"] == 0)
                .map(|v| v["chunk"]["text"].as_str().unwrap())
                .collect::<String>();
            serde_json::from_str::<serde_json::Value>(&text).unwrap()
        } else {
            value["items"][0].clone()
        };
        assert!(
            !text.contains(png.to_str().unwrap())
                && !text.contains("retained.wav")
                && !text.contains("/home/"),
            "TE14_NO_AMBIENT_MEDIA_PATH"
        );
        assert!(
            !sample["data_base64"].as_str().unwrap().is_empty(),
            "TE14_OWNED_SAMPLES_AVAILABLE"
        );
        if sample["kind"] == "owned_audio_sample" && sample["channels"] == 2 {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(sample["data_base64"].as_str().unwrap())
                .unwrap();
            let mut left = 0.0f32;
            let mut right = 0.0f32;
            for pair in bytes.as_chunks::<8>().0 {
                left += f32::from_ne_bytes(pair[..4].try_into().unwrap()).abs();
                right += f32::from_ne_bytes(pair[4..].try_into().unwrap()).abs();
            }
            assert!(
                right > 0.001 && right > left * 10.0,
                "TE14_PLANAR_CHANNELS_PRESERVED left={left} right={right}"
            );
        }
        if sample["kind"] == "owned_frame_sample" {
            assert_eq!(
                base64::engine::general_purpose::STANDARD
                    .decode(sample["data_base64"].as_str().unwrap())
                    .unwrap()
                    .len(),
                16 * 16,
                "TE14_FRAME_VISIBLE_BYTES_ONLY"
            );
        }
        assert_eq!(
            sample["partial_coverage"], true,
            "TE14_SAMPLE_PARTIAL_EXPLICIT"
        );
    }
    // The decode blocks at a deterministic test barrier after dropping the
    // owner. A real edit can proceed; its changed revision invalidates the read.
    let owner = f.host.owner.upgrade().unwrap();
    let gate = std::sync::Arc::new((std::sync::Barrier::new(2), std::sync::Barrier::new(2)));
    owner.lock().unwrap().hooks.context_media_gate = Some(gate.clone());
    let before = f.projection(&run);
    let before_count = owner
        .lock()
        .unwrap()
        .expansion_count(before.tasks.iter().find(|t| t.id == child).unwrap())
        .unwrap();
    let revision = grant_test_revision(&core);
    let (unlocked, stale) = std::thread::scope(|scope| {
        let read = scope.spawn(|| {
            te14_read(
                &mcp,
                revision,
                EvidenceKind::Frame {
                    clip: ClipId(3),
                    source_frame: 0,
                },
                None,
            )
        });
        gate.0.wait();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        let unlocked = loop {
            if owner.try_lock().is_ok() {
                break true;
            }
            if std::time::Instant::now() >= deadline {
                break false;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        };
        if unlocked {
            core.request(kinewright_core::Command::Undo).unwrap();
        }
        gate.1.wait();
        (unlocked, read.join().unwrap())
    });
    assert!(unlocked, "TE14_DECODE_RELEASES_OWNER");
    assert_eq!(stale.is_error, Some(true), "TE14_DECODE_RECHECKS_REVISION");
    let after = f.projection(&run);
    assert_eq!(
        owner
            .lock()
            .unwrap()
            .expansion_count(after.tasks.iter().find(|t| t.id == child).unwrap())
            .unwrap(),
        before_count,
        "TE14_STALE_DECODE_NO_AUDIT"
    );
    core.request(kinewright_core::Command::Redo).unwrap();
    let unavailable = te14_read(
        &mcp,
        grant_test_revision(&core),
        EvidenceKind::Audio {
            clip: ClipId(3),
            source_frame: TimeCode(0).0,
        },
        None,
    );
    assert_eq!(
        unavailable.is_error,
        Some(true),
        "TE14_NO_UNDECLARED_STREAM_FALLBACK"
    );
}

#[test]
fn te14_large_overview_is_bounded_and_can_expand() {
    let mut doc = Document::default();
    for id in 1..120 {
        doc.tracks.push(kinewright_core::Track {
            id: kinewright_core::TrackId(id),
            kind: kinewright_core::TrackKind::Audio,
            sync_lock: false,
            clips: vec![],
        });
    }
    let f = Fixture::with_document(false, doc);
    let (_, _, mcp) = c4_director(&f);
    let overview = te14_outcome(mcp.wire("get_timeline_state", serde_json::json!({})));
    assert!(
        serde_json::to_vec(&overview).unwrap().len() <= 8192,
        "TE14_OVERVIEW_BOUND"
    );
    assert!(
        overview["omitted"] == true || overview["context"]["omitted"] == true,
        "TE14_LARGE_OVERVIEW_OMISSION"
    );
    let expanded = mcp.wire(
        "read_task_evidence",
        serde_json::json!({"revision":0,"evidence":{"kind":"timeline","clips":[]}}),
    );
    assert_eq!(
        expanded.is_error,
        Some(false),
        "TE14_LARGE_OVERVIEW_EXPANDS"
    );
    let value = te14_outcome(expanded);
    let header = if value["items"][0]["kind"] == "json_fragment" {
        let text = value["items"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|v| v["kind"] == "json_fragment" && v["row"] == 0)
            .map(|v| v["chunk"]["text"].as_str().unwrap())
            .collect::<String>();
        serde_json::from_str::<serde_json::Value>(&text).unwrap()
    } else {
        value["items"][0].clone()
    };
    assert_eq!(
        header["tracks"].as_array().unwrap().len(),
        119,
        "TE14_LARGE_RELATIONSHIPS_PAGEABLE"
    );
}
#[test]
fn te14_plain_text_transcript_keeps_neighbours_and_unknown_alignment() {
    let mut f = Fixture::new();
    let text = "we did not approve this cut";
    let reference = f
        .host
        .owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .owner_mut()
        .unwrap()
        .admit_host_grant(text.as_bytes())
        .unwrap();
    f.brief.script = reference.clone();
    let (_, _, mcp) = c4_director(&f);
    let full = te14_outcome(te14_read(
        &mcp,
        kinewright_core::TimelineRevision(0),
        EvidenceKind::Reference {
            reference: reference.clone(),
        },
        None,
    ));
    let read = te14_read(
        &mcp,
        kinewright_core::TimelineRevision(0),
        EvidenceKind::Transcript {
            reference,
            start: 3,
            end: 4,
        },
        None,
    );
    assert_eq!(
        read.is_error,
        Some(false),
        "TE14_PLAIN_TEXT_TRANSCRIPT_EXPANDS"
    );
    let span = te14_outcome(read);
    assert_eq!(
        span["items"][0]["source_alignment"], "unobserved",
        "TE14_PLAIN_TEXT_ALIGNMENT_UNKNOWN"
    );
    let original = full["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["chunk"]["text"].as_str().unwrap())
        .collect::<String>();
    let full_reject = original.split_whitespace().any(|word| word == "not");
    let scoped_reject = span["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|word| word["word"] == "not" && word["neighbour"] == true);
    assert!(
        full_reject && scoped_reject,
        "TE14_PLAIN_TEXT_SAME_REJECTION"
    );
}

#[test]
fn te14_unlocked_selected_track_expands_ripple_and_marker_dependencies() {
    let mut doc = te14_document();
    doc.tracks[0].sync_lock = false;
    for clip in &mut doc.tracks[0].clips {
        clip.link = None;
    }
    if let kinewright_core::Operation::AddMarker { mut marker } = history_marker(1) {
        marker.position = kinewright_core::TimeCode(795);
        doc.markers.push(marker);
    }
    let first = doc.tracks[0].clips[0].id;
    let last = doc.tracks[0].clips.last().unwrap().id;
    let mut projected = doc.clone();
    kinewright_core::apply_batch(
        &mut projected,
        &[kinewright_core::Operation::RippleDeleteClip { clip: first }],
    )
    .unwrap();
    assert_ne!(
        projected.tracks[0].clips.last().unwrap().timeline_start,
        doc.tracks[0].clips.last().unwrap().timeline_start,
        "TE14_REAL_UNLOCKED_RIPPLE"
    );
    assert_ne!(
        projected.markers[0].position, doc.markers[0].position,
        "TE14_REAL_RIPPLE_MARKER"
    );
    let mut f = Fixture::with_document(false, doc.clone());
    f.brief.scope =
        crate::orchestration::policy::director_scope(&doc, f.brief.scope.base.clone()).unwrap();
    let first_key = format!("clip:{first}");
    f.brief
        .scope
        .objects
        .retain(|key| (!key.starts_with("clip:") || *key == first_key) && key != "marker:1");
    let (run, _, mcp) = c4_director(&f);
    let before = f.projection(&run).tasks[0].brief.scope.clone();
    let mut cursor = None;
    let mut follower = false;
    let mut marker = false;
    for _ in 0..8 {
        let value = te14_outcome(te14_read(
            &mcp,
            kinewright_core::TimelineRevision(0),
            EvidenceKind::Timeline { clips: vec![first] },
            cursor,
        ));
        for row in value["items"].as_array().unwrap() {
            follower |= row["clip"]["id"] == last.0;
            marker |= row["markers"].as_array().is_some_and(|markers| {
                markers.iter().any(|m| m["id"] == 1 && m["position"] == 795)
            });
        }
        cursor = value["next_cursor"].as_str().map(str::to_owned);
        if cursor.is_none() {
            break;
        }
    }
    assert!(
        follower && !before.objects.contains(&format!("clip:{last}")),
        "TE14_UNLOCKED_RIPPLE_CLOSURE"
    );
    assert!(
        marker && !before.objects.contains("marker:1"),
        "TE14_RIPPLE_MARKER_CLOSURE"
    );
    assert_eq!(
        f.projection(&run).tasks[0].brief.scope,
        before,
        "TE14_RIPPLE_READ_NEVER_GRANTS_EDIT"
    );
}
