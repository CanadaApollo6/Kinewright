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
        ..Trace::default()
    };
    trace.calls.push(call(
        "publish_result",
        json!({"command":{"value":{"result":{"explanation":{"sha256":"x"}}}}}),
        json!({}),
        false,
    ));
    for tool in ["merge_task", "read_task_result", "merge_task"] {
        let mut c = call(
            tool,
            json!({"command":{"value":{"task":"corrected"}}}),
            json!({"unmet":"correct-homophone","proof":"failed"}),
            false,
        );
        if tool == "read_task_result" {
            c.arguments = json!({"reference":{"sha256":"x"}});
        }
        c.task = json!("director");
        trace.calls.push(c);
    }
    assert!(observed(5, &trace), "TE15_B2AF1_READ_ALTERNATIVE");
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
