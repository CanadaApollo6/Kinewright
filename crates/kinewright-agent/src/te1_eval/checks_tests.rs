use super::*;
use serde_json::json;
fn call(tool: &str, arguments: Value, reply: Value, refused: bool) -> Call {
    Call {
        task: json!("cut"),
        attempt: json!("attempt"),
        tool: tool.into(),
        arguments,
        reply,
        refused,
        fixture: false,
    }
}
fn recovery() -> Trace {
    let mut trace = Trace::default();
    for kind in ["timeline", "transcript", "frame", "audio"] {
        trace.calls.push(call(
            "read_task_evidence",
            json!({"evidence":{"kind":kind}}),
            json!({}),
            false,
        ));
    }
    let mut undo = call("undo_task_edit", json!({}), json!({}), false);
    undo.fixture = true;
    trace.calls.push(undo);
    trace.calls.push(call(
        "commit_edit_plan",
        json!({"expected_revision":1,"plan_id":1}),
        json!({}),
        true,
    ));
    trace.calls.push(call(
        "get_timeline_state",
        json!({}),
        json!({"revision":2}),
        false,
    ));
    trace.calls.push(call(
        "prepare_edit_plan",
        json!({"expected_revision":2}),
        json!({"plan_id":2,"preview":{"operation_count":1,"expected_revision":2}}),
        false,
    ));
    trace.calls.push(call(
        "commit_edit_plan",
        json!({"expected_revision":2,"plan_id":2}),
        json!({}),
        false,
    ));
    trace
}
#[test]
fn te15_b2af1_sibling_evidence() {
    let mut trace = recovery();
    assert!(observed(6, &trace));
    for call in &mut trace.calls[..4] {
        call.task = json!("sibling");
    }
    assert!(!observed(6, &trace), "TE15_B2AF1_SIBLING");
}
#[test]
fn te15_b2af1_fixture_evidence() {
    let mut trace = recovery();
    for call in &mut trace.calls[..4] {
        call.fixture = true;
    }
    assert!(!observed(6, &trace), "TE15_B2AF1_FIXTURE");
}
#[test]
fn te15_b2af1_ordered_recovery() {
    let mut trace = recovery();
    trace.calls.swap(6, 8);
    assert!(!observed(6, &trace), "TE15_B2AF1_ORDER");
}
#[test]
fn te15_b2af1_bound_preview() {
    for bad in ["null", "plan", "revision", "attempt"] {
        let mut trace = recovery();
        match bad {
            "null" => trace.calls[7].reply["preview"] = Value::Null,
            "plan" => trace.calls[8].arguments["plan_id"] = json!(3),
            "revision" => trace.calls[8].arguments["expected_revision"] = json!(3),
            _ => trace.calls[8].attempt = json!("other"),
        }
        assert!(!observed(6, &trace), "TE15_B2AF1_PREVIEW {bad}");
    }
}
#[test]
fn te15_b2af1_accepted_refresh_paths() {
    for tool in ["task_status", "get_timeline_state", "controls"] {
        let mut trace = recovery();
        if tool == "controls" {
            trace.calls[5].reply = json!({"controls":{"revision":2}});
            trace.calls.remove(6);
        } else {
            trace.calls[6].tool = tool.into();
        }
        assert!(observed(6, &trace), "TE15_B2AF1_REFRESH {tool}");
    }
}
#[test]
fn te15_b2af1_case5_read_before_merge() {
    let mut trace = Trace {
        sends: vec![json!({"brief":{"role":"director"},"identity":{"task":"director"}})],
        case5_deceptive_child: Some(json!("cut")),
        ..Trace::default()
    };
    trace.calls.push(call(
        "publish_result",
        json!({"command":{"value":{"result":{"explanation":{"sha256":"x"}}}}}),
        json!({"receipt":{"id":"publication-x"}}),
        false,
    ));
    for tool in ["merge_task", "read_task_result", "merge_task"] {
        let mut c = call(
            tool,
            json!({"command":{"value":{"task":if trace.calls.len()==1 {"earlier-independent"} else {"corrected"}}}}),
            json!({"unmet":"correct-homophone","proof":"failed","receipt":{"id":format!("merge-{}",trace.calls.len())}}),
            false,
        );
        if tool == "read_task_result" {
            c.arguments = json!({"reference":{"sha256":"x"}});
        }
        c.task = json!("director");
        trace.calls.push(c);
    }
    assert!(!observed(5, &trace), "FU1_FIRST_MERGE_BEFORE_READ_REFUSED");
    trace.calls.remove(1);
    assert!(observed(5, &trace), "FU1_READ_BEFORE_FIRST_MERGE");
    trace.calls.retain(|c| c.tool != "read_task_result");
    assert!(!observed(5, &trace), "TE15_B2AF1_READ_REQUIRED");
}
#[test]
fn te15_b2af1_original_audio_path() {
    let oracle = kinewright_project::load_document(&fixtures().join("case-6-oracle.kinewright"))
        .unwrap()
        .0;
    let resolved = absolute(oracle, true);
    assert_eq!(
        resolved
            .media_pool
            .iter()
            .find(|a| a.id.0 == 2)
            .unwrap()
            .path,
        fixtures().join("talking-head.wav"),
        "TE15_B2AF1_WAV"
    );
}
#[test]
fn te15_b2af1_native_fallback_counter() {
    let mut trace = Trace::default();
    super::super::record_launch(&mut trace, true).unwrap();
    assert_eq!(trace.launch_requests, 1);
    assert!(super::super::record_launch(&mut trace, false).is_err());
    assert_eq!(trace.provider_launches, 1, "TE15_B2AF1_NATIVE_COUNT");
}

#[test]
fn te15_b2af1_recipe_home_growth() {
    let program = json!({"working_directory":"/home/person/work","credential_root":"/home/person/auth","arguments":["/home/person/work"],"endpoint":"http://127.0.0.1:1234/task"});
    let size = crate::te1_eval::recipe::measure(
        &program,
        &json!({"endpoint":"http://127.0.0.1:1234/task"}),
    );
    assert_eq!(
        size.longer_home_bytes,
        size.total_bytes + 3 * (32 - 6),
        "TE15_B2AF1_HOME"
    );
    assert!(size.identified_run_string_bytes > "/home/person/work".len());
}

#[test]
fn te15_b2af1_carrier_seek_matches_wav() {
    for (start, end) in [(0, 40), (40, 50), (50, 100), (61, 100)] {
        let decode = |path: &std::path::Path| {
            kinewright_media::decode_audio_range(
                path,
                kinewright_core::Rational::new(25, 1).unwrap(),
                TimeCode(start),
                TimeCode(end),
                48000,
                2,
                &ExportCancellation::default(),
            )
            .unwrap()
        };
        let wav = decode(&fixtures().join("talking-head.wav"));
        let carrier = decode(&super::super::media_source(2));
        assert_eq!(
            carrier.len(),
            wav.len(),
            "TE15_B2AF1_SEEK_LENGTH {start}..{end}"
        );
        assert!(
            carrier
                .iter()
                .zip(&wav)
                .all(|(a, b)| (*a - *b).abs() <= 1.0 / 32768.0),
            "TE15_B2AF1_SEEK {start}..{end}"
        );
    }
}

#[test]
fn te15_b2af1_recovery_matches_injected_task() {
    let mut trace = recovery();
    trace.calls[4].task = json!("sibling");
    assert!(!observed(6, &trace), "TE15_B2AF1_AFFECTED");
}

#[test]
fn te15_b2b_effective_merge_order() {
    let bad = call(
        "publish_result",
        json!({"command":{"value":{"result":{"explanation":{"sha256":"x"}}}}}),
        json!({"receipt":{"id":"publication-x"}}),
        false,
    );
    let mut read = call(
        "read_task_result",
        json!({"reference":{"sha256":"x"}}),
        json!({"unmet":"correct-homophone","proof":"failed"}),
        false,
    );
    read.task = json!("director");
    let mut merge = call(
        "merge_task",
        json!({"command":{"value":{"task":"corrected"}}}),
        json!({"outcome":{"receipt":{"id":{"issuer":"real-durable","counter":1}}}}),
        false,
    );
    merge.task = json!("director");
    let mut trace = Trace {
        sends: vec![json!({"brief":{"role":"director"},"identity":{"task":"director"}})],
        case5_deceptive_child: Some(json!("cut")),
        calls: vec![bad.clone(), read.clone(), merge.clone(), merge.clone()],
        ..Trace::default()
    };
    assert!(observed(5, &trace), "B2B_READ_MERGE_REPLAY");
    trace.calls = vec![bad, merge.clone(), read, merge];
    assert!(!observed(5, &trace), "B2B_MERGE_READ_REPLAY_REFUSED");
}

#[test]
fn te15_b2b_finish_publication_order() {
    let mut bad = call(
        "finish_task",
        json!({}),
        json!({"completion_status":{"publication_receipt":{"id":"durable"},"result":{"explanation":{"sha256":"x"}}}}),
        false,
    );
    let mut read = call(
        "read_task_result",
        json!({"reference":{"sha256":"x"}}),
        json!({"unmet":"correct-homophone","proof":"failed"}),
        false,
    );
    read.task = json!("director");
    let mut merge = call(
        "merge_task",
        json!({"command":{"value":{"task":"corrected"}}}),
        json!({"outcome":{"receipt":{"id":{"issuer":"real-durable","counter":1}}}}),
        false,
    );
    merge.task = json!("director");
    let mut trace = Trace {
        sends: vec![json!({"brief":{"role":"director"},"identity":{"task":"director"}})],
        case5_deceptive_child: Some(json!("cut")),
        calls: vec![bad.clone(), read.clone(), merge.clone()],
        ..Trace::default()
    };
    assert!(observed(5, &trace), "B2B_FINISH_PUBLISHED_OBSERVED");
    bad.reply["completion_status"]["publication_receipt"] = serde_json::Value::Null;
    trace.calls = vec![bad, read, merge];
    assert!(!observed(5, &trace), "B2B_FROZEN_ONLY_IS_NOT_PUBLISHED");
}

#[test]
fn te15_b2b_fu2_publication_replay() {
    let mut publication = call(
        "finish_task",
        json!({"id":"finish-1"}),
        json!({"receipt":{"id":"finish-1"},"completion_status":{"publication_receipt":{"id":"publication-1"},"result":{"explanation":{"sha256":"x"}}}}),
        false,
    );
    publication.task = json!("deceptive");
    let mut read = call(
        "read_task_result",
        json!({"reference":{"sha256":"x"}}),
        json!({"unmet":"correct-homophone","proof":"failed"}),
        false,
    );
    read.task = json!("director");
    let merge = |id| {
        let mut c = call(
            "merge_task",
            json!({"command":{"value":{"task":"corrected"}}}),
            json!({"receipt":{"id":id}}),
            false,
        );
        c.task = json!("director");
        c
    };
    let mut trace = Trace {
        sends: vec![json!({"brief":{"role":"director"},"identity":{"task":"director"}})],
        case5_deceptive_child: Some(json!("deceptive")),
        ..Trace::default()
    };
    for outcome_wrapped in [false, true] {
        let mut replay = publication.clone();
        if outcome_wrapped {
            replay.reply = json!({"outcome":publication.reply});
        }
        trace.calls = vec![
            publication.clone(),
            merge(1),
            replay,
            read.clone(),
            merge(2),
        ];
        assert!(
            !observed(5, &trace),
            "FU2_PUBLICATION_REPLAY_CANNOT_RESET_BOUNDARY"
        );
    }
    trace.calls = vec![
        publication.clone(),
        read.clone(),
        merge(1),
        publication.clone(),
        merge(1),
    ];
    assert!(observed(5, &trace), "FU2_LEGAL_FINISH_AND_MERGE_REPLAY");
    trace.calls = vec![merge(0), publication, merge(0), read, merge(1)];
    assert!(
        observed(5, &trace),
        "FU2_EARLIER_MERGE_REPLAY_IS_NOT_EFFECTIVE"
    );
}

#[test]
fn te15_b2b_fu2_first_qualifying_publication() {
    let publication = |id: &str| {
        let mut c = call(
            "finish_task",
            json!({}),
            json!({"completion_status":{"publication_receipt":{"id":id},"result":{"explanation":{"sha256":id}}}}),
            false,
        );
        c.task = json!("first");
        c
    };
    let mut read = call(
        "read_task_result",
        json!({"reference":{"sha256":"second"}}),
        json!({"unmet":"correct-homophone","proof":"failed"}),
        false,
    );
    read.task = json!("director");
    let merge = |id| {
        let mut c = call(
            "merge_task",
            json!({"command":{"value":{"task":"corrected"}}}),
            json!({"receipt":{"id":id}}),
            false,
        );
        c.task = json!("director");
        c
    };
    let mut trace = Trace {
        sends: vec![json!({"brief":{"role":"director"},"identity":{"task":"director"}})],
        case5_deceptive_child: Some(json!("first")),
        calls: vec![
            publication("first"),
            merge(1),
            publication("second"),
            read.clone(),
            merge(2),
        ],
        ..Trace::default()
    };
    assert!(
        !observed(5, &trace),
        "FU2_LATER_PUBLICATION_CANNOT_RESET_ANCHOR"
    );
    let mut refused = publication("ignored");
    refused.refused = true;
    read.arguments = json!({"reference":{"sha256":"first"}});
    trace.calls = vec![refused, publication("first"), read, merge(1)];
    assert!(
        observed(5, &trace),
        "FU2_FIRST_SUCCESSFUL_QUALIFYING_PUBLICATION"
    );
}

#[test]
fn te15_b2b_fu3_earlier_reviewer() {
    let trace = std::sync::Arc::new(std::sync::Mutex::new(Trace {
        sends: vec![json!({"brief":{"role":"director"},"identity":{"task":"director"}})],
        ..Trace::default()
    }));
    let observer = crate::te1_eval::observe::Observations::new(5, trace.clone());
    for (task, index) in [("reviewer", 2), ("deceptive", 1)] {
        observer
            .bind(crate::te1_eval::SendContext {
                endpoint: String::new(),
                identity: json!({"task":task,"attempt":task}),
                brief: json!({}),
                prompt: String::new(),
                fixtures: Value::Null,
                child_index: index,
            })
            .unwrap();
    }
    let publish = |task: &str| {
        let mut c = call(
            "finish_task",
            json!({}),
            json!({"completion_status":{
                "publication_receipt":{"id":task},"result":{"explanation":{"sha256":task}}
            }}),
            false,
        );
        c.task = json!(task);
        c
    };
    let mut merge = call(
        "merge_task",
        json!({"command":{"value":{"task":"corrected"}}}),
        json!({"receipt":{"id":"merge"}}),
        false,
    );
    merge.task = json!("director");
    for tool in ["read_task_result", "read_child_result"] {
        let mut read = call(
            tool,
            json!({"reference":{"sha256":"deceptive"},"child":"deceptive"}),
            json!({"unmet":"correct-homophone","proof":"failed"}),
            false,
        );
        read.task = json!("director");
        let mut trace = trace.lock().unwrap();
        assert_eq!(trace.case5_deceptive_child, Some(json!("deceptive")));
        trace.calls = vec![
            publish("reviewer"),
            publish("deceptive"),
            read,
            merge.clone(),
        ];
        assert!(
            observed(5, &trace),
            "FU3_EARLIER_REVIEWER_DOES_NOT_STEAL_ANCHOR"
        );
    }
}
