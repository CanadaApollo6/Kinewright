//! Unpaid Studio E2E. There is no installed-provider or paid entry point here.
#![cfg(feature = "te1-eval")]
use kinewright_agent::te1_eval::{fixtures, pin};
use serde_json::{Value, json};
use std::path::Path;
fn committed_pin() -> Value {
    serde_json::from_slice(
        &std::fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/te15-measurement-pin.json"),
        )
        .unwrap(),
    )
    .unwrap()
}
#[test]
fn te15_b2a_measurement_pin_matches_every_observed_field() {
    pin::verify(&committed_pin()).unwrap();
}
fn leaves(value: &Value, path: &str, paths: &mut Vec<String>) {
    match value {
        Value::Object(object) => {
            for (key, value) in object {
                leaves(
                    value,
                    &format!("{path}/{}", key.replace('~', "~0").replace('/', "~1")),
                    paths,
                );
            }
        }
        Value::Array(values) => {
            for (index, value) in values.iter().enumerate() {
                leaves(value, &format!("{path}/{index}"), paths);
            }
        }
        _ => paths.push(path.into()),
    }
}
#[test]
fn te15_b2a_pin_drift_names_every_leaf_including_open_models() {
    let pin = committed_pin();
    let mut paths = Vec::new();
    leaves(&pin, "", &mut paths);
    assert!(paths.len() > 500);
    for path in &paths {
        let mut changed = pin.clone();
        *changed.pointer_mut(path).unwrap() = json!({"mutant":true});
        assert_eq!(
            pin::first_difference(&pin, &changed),
            Some(path.clone()),
            "{path}"
        );
    }
    let mut changed = pin.clone();
    changed["unexpected"] = json!(1);
    assert_eq!(
        pin::first_difference(&pin, &changed),
        Some("/unexpected".into())
    );
    eprintln!("TE15_B2A_PIN fields={} all_named=true", paths.len());
}
#[test]
fn te15_b2a_fixtures_are_the_frozen_corpus() {
    let hashes: Value =
        serde_json::from_slice(&std::fs::read(fixtures().join("hashes.json")).unwrap()).unwrap();
    for (file, expected) in hashes.as_object().unwrap() {
        assert_eq!(
            kinewright_media::sha256_file(&fixtures().join(file)).unwrap(),
            expected.as_str().unwrap(),
            "{file}"
        );
    }
}
#[cfg(target_os = "linux")]
fn te15_b2a_catalog_child(name: &str) -> bool {
    const CHILD: &str = "KINEWRIGHT_TE15_B2A_CHILD";
    if std::env::var(CHILD).as_deref() == Ok(name) {
        let home = std::path::PathBuf::from(std::env::var_os("HOME").unwrap());
        let codex = std::path::PathBuf::from(std::env::var_os("CODEX_HOME").unwrap());
        assert!(home.starts_with(kinewright_media::test_support::workspace_target_dir()));
        assert!(codex.starts_with(kinewright_media::test_support::workspace_target_dir()));
        assert_eq!(std::fs::read_dir(&home).unwrap().count(), 0);
        std::fs::create_dir(home.join(".claude")).unwrap();
        std::fs::write(codex.join("models_cache.json"),br#"{"models":[{"slug":"gpt-6.1-sol","supported_reasoning_levels":[{"effort":"high"},{"effort":"medium"}],"service_tiers":[{"id":"priority","name":"Fast"}]}]}"#).unwrap();
        if name == "te15_b2b0_home_capacity" {
            let public = include_bytes!("fixtures/te15-b2b0-models.json");
            std::fs::write(codex.join("models_cache.json"), public).unwrap();
        }
        return true;
    }
    let temp = kinewright_media::test_support::TempDirectory::new_in(
        &kinewright_media::test_support::workspace_target_dir(),
        "te15-b2a-catalog",
    );
    for dir in ["home", "codex"] {
        std::fs::create_dir(temp.path(dir)).unwrap();
    }
    let mut home = temp.path("home");
    if name == "te15_b2b0_home_capacity" {
        home = home.join("h".repeat(255 - home.as_os_str().len() - 1));
        assert_eq!(home.as_os_str().len(), 255);
        std::fs::create_dir(&home).unwrap();
    }
    let log = std::fs::File::create(temp.path("child.log")).unwrap();
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", name, "--nocapture"])
        .env(CHILD, name)
        .env("HOME", &home)
        .env("CODEX_HOME", temp.path("codex"))
        .env("CLAUDE_CONFIG_DIR", home.join(".claude"))
        .stdout(log.try_clone().unwrap())
        .stderr(log)
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_mins(15);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break Some(status);
        }
        if std::time::Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            break None;
        }
        std::thread::park_timeout(std::time::Duration::from_millis(10));
    };
    let output = std::fs::read_to_string(temp.path("child.log")).unwrap();
    assert!(
        status.is_some_and(|s| s.success()),
        "TE15_B2A_CATALOG {name}: {output}"
    );
    for line in output
        .lines()
        .filter(|line| line.starts_with("TE15_B2A_") || line.starts_with("TE15_B2AF1_"))
    {
        eprintln!("{line}");
    }
    false
}
#[cfg(target_os = "linux")]
#[test]
fn te15_b2b0_home_capacity() {
    if !te15_b2a_catalog_child("te15_b2b0_home_capacity") {
        return;
    }
    let temp = kinewright_media::test_support::TempDirectory::new_in(
        &kinewright_media::test_support::workspace_target_dir(),
        "b2b0-capacity",
    );
    for provider in ["codex", "claude"] {
        let result = kinewright_agent::te1_eval::run_scripted(
            &temp.path(provider),
            5,
            provider,
            kinewright_agent::te1_eval::ScriptedOutcome::Correct,
        )
        .unwrap();
        assert!(!result.trace.capacity_sizes.is_empty());
        let mut maximum = 0;
        for row in &result.trace.capacity_sizes {
            assert_eq!(row["home_bytes"], 255, "B2B0_LONG_HOME");
            let bytes = row["program_bytes"]
                .as_u64()
                .unwrap()
                .max(row["all_home_paths_255_bytes"].as_u64().unwrap());
            maximum = maximum.max(bytes);
            assert!(bytes <= 16384, "B2B0_HALF_LIMIT {row}");
        }
        assert_eq!(result.trace.input_hashes.len(), result.trace.prompts.len());
        for row in &result.trace.input_hashes {
            assert_eq!(row["before"], row["after"], "B2B0_ROLE_HASH {row}");
        }
        eprintln!(
            "TE15_B2A_B2B0_HOME provider={provider} home=255 maximum={maximum} roles=6 model_options={}",
            result.trace.capacity_sizes[0]["selection"]
        );
    }
}
#[cfg(target_os = "linux")]
fn scripted(case: u8) {
    use kinewright_agent::te1_eval::{ScriptedOutcome, run_scripted};
    let name = match case {
        1 => "te15_b2a_case1_scripted_studio_end_to_end",
        5 => "te15_b2a_case5_scripted_studio_end_to_end",
        _ => "te15_b2a_case6_scripted_studio_end_to_end",
    };
    if !te15_b2a_catalog_child(name) {
        return;
    }

    let temp = kinewright_media::test_support::TempDirectory::new_in(
        &kinewright_media::test_support::workspace_target_dir(),
        "te15-b2a",
    );
    for provider in ["codex", "claude"] {
        let correct = run_scripted(
            &temp.path(&format!("{case}-{provider}-green")),
            case,
            provider,
            ScriptedOutcome::Correct,
        )
        .unwrap();
        assert!(
            correct.checks.passed(),
            "TE15_B2A_ORACLE_PASS case {case} {provider}: {:?}",
            correct.checks
        );
        let accounting: kinewright_project::orchestration_accounting::AccountingRecord =
            serde_json::from_value(correct.accounting.clone()).unwrap();
        let row = kinewright_agent::te1_eval::analysis::Row::from_accounting(
            provider,
            case,
            kinewright_agent::te1_eval::analysis::Arm::Candidate,
            &accounting,
            correct.checks.passed(),
        )
        .unwrap();
        assert!(
            row.usage_complete,
            "TE15_B2A_USAGE_COMPLETE {provider}: {accounting:?}"
        );
        assert_eq!(
            row.inclusive_lower_bound,
            120 * correct.trace.scripted_sends as u64,
            "TE15_B2A_ALL_TASK_ATTEMPTS"
        );

        assert_eq!(correct.trace.scripted_sends, correct.trace.recipes);
        assert_eq!(correct.trace.recipe_bytes.len(), correct.trace.recipes);
        assert_eq!(correct.trace.scripted_sends, if case == 1 { 3 } else { 7 });
        assert_eq!(
            correct.accounting["tasks"]
                .as_array()
                .unwrap()
                .iter()
                .map(|t| t["sends"].as_array().unwrap().len())
                .sum::<usize>(),
            correct.trace.scripted_sends
        );
        assert!(!correct.trace.calls.is_empty());
        task_scopes(&correct, case);
        let wrong = run_scripted(
            &temp.path(&format!("{case}-{provider}-red")),
            case,
            provider,
            ScriptedOutcome::Wrong,
        )
        .unwrap();
        assert!(!wrong.checks.passed(), "case {case} wrong outcome escaped");
        assert!(!wrong.checks.document);
        if case == 5 {
            assert!(
                !wrong.checks.rendered_frames,
                "bad caption must change pixels"
            );
        }
        if case == 6 {
            assert!(
                !wrong.checks.audio_samples,
                "missing ripple/trim must change PCM"
            );
        }
        assert_eq!(wrong.trace.provider_launches, 0);
        eprintln!(
            "TE15_B2A_ORACLE case={case} provider={provider} wrong=RED restored=GREEN real_provider_launches={} native_program_max_bytes={} checks={:?}",
            correct.trace.provider_launches,
            correct.trace.recipe_bytes.iter().max().unwrap(),
            correct.checks
        );
    }
}
#[cfg(target_os = "linux")]
fn stdin_shape(correct: &kinewright_agent::te1_eval::Measurement, case: u8) {
    let provider = correct.provider.as_str();
    for args in &correct.trace.recipe_argv {
        if provider == "codex" {
            assert_eq!(
                args.last().map(String::as_str),
                Some("-"),
                "B2B0_CODEX_STDIN_MODE"
            );
        } else {
            assert!(!args.iter().any(|a| a == "--"), "B2B0_CLAUDE_STDIN_MODE");
        }
    }
    assert_eq!(
        correct.trace.input_hashes.len(),
        correct.trace.prompts.len()
    );
    for row in &correct.trace.input_hashes {
        assert_eq!(
            row["before"], row["after"],
            "B2B0_CASE_HASH {case} {provider}"
        );
    }
}
#[cfg(target_os = "linux")]
fn task_scopes(correct: &kinewright_agent::te1_eval::Measurement, case: u8) {
    stdin_shape(correct, case);
    assert_eq!(
        correct.trace.launch_requests, correct.trace.scripted_sends,
        "TE15_B2AF1_LAUNCH_COUNT"
    );
    assert_eq!(
        correct.trace.bound_attempts, correct.trace.scripted_sends,
        "TE15_B2AF1_BIND_COUNT"
    );
    for size in &correct.trace.recipe_sizes {
        eprintln!(
            "TE15_B2AF1_RECIPE case={case} provider={} {}",
            correct.provider,
            serde_json::to_string(size).unwrap()
        );
    }
    let attempts: std::collections::BTreeSet<_> = correct.accounting["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|task| task["sends"].as_array().unwrap())
        .map(|send| send["attempt"].as_str().unwrap())
        .collect();
    assert_eq!(
        correct.trace.launch_requests,
        attempts.len(),
        "TE15_B2AF1_ATTEMPT_LAUNCHES"
    );
    assert_eq!(correct.trace.provider_launches, 0);
    assert!(
        correct.trace.recipe_bytes.iter().all(|n| *n <= 32768),
        "TE15_B2A_NATIVE_PROGRAM_BOUND"
    );
    assert_eq!(
        correct.trace.feedback_revisions.len(),
        if case == 5 { 2 } else { 1 },
        "TE15_B2A_NOTE_COUNT"
    );
    if case == 5 {
        assert_ne!(
            correct.trace.feedback_revisions[0], correct.trace.feedback_revisions[1],
            "TE15_B2A_NOTE_IDENTITIES"
        );
    }
    let children: Vec<_> = correct
        .trace
        .sends
        .iter()
        .filter(|s| s["brief"]["role"] != "director")
        .collect();
    assert_eq!(children.len(), if case == 1 { 1 } else { 3 });
    for child in &children {
        assert_eq!(
            child["brief"]["scope"]["base"], child["accepted_candidate"],
            "TE15_B2A_LATE_BOUND_CANDIDATE"
        );
    }
    if case == 6 {
        assert!(
            correct.trace.initial_tracks.contains(&3),
            "TE15_B2A_CAPTIONS_PRESTART"
        );
        assert!(
            correct
                .trace
                .calls
                .iter()
                .any(|call| call.tool == "prepare_edit_plan"
                    && call.arguments.pointer("/operations/0/track/id") == Some(&json!(88))
                    && call.refused
                    && serde_json::to_string(&call.reply)
                        .unwrap()
                        .contains("outside scope: track:88")),
            "TE15_B2A_ORDINARY_SCOPE"
        );
    }
    if case != 1 {
        assert_eq!(
            children
                .iter()
                .map(|s| s["brief"]["role"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["editor", "editor", "motion"]
        );
        assert_ne!(
            children[1]["brief"]["scope"]["base"], children[2]["brief"]["scope"]["base"],
            "TE15_B2A_EDITOR_MOTION_ADVANCED"
        );
        if case == 6 {
            assert_ne!(
                children[0]["brief"]["scope"]["base"], children[1]["brief"]["scope"]["base"],
                "TE15_B2A_EDITOR_EDITOR_ADVANCED"
            );
        }
    }
}
#[cfg(target_os = "linux")]
#[test]
fn te15_b2a_case1_scripted_studio_end_to_end() {
    scripted(1);
}
#[cfg(target_os = "linux")]
#[test]
fn te15_b2a_case5_scripted_studio_end_to_end() {
    scripted(5);
}
#[cfg(target_os = "linux")]
#[test]
fn te15_b2a_case6_scripted_studio_end_to_end() {
    scripted(6);
}

#[test]
fn te15_b2a_analysis_retains_every_pair_and_requires_both_statistics() {
    use kinewright_agent::te1_eval::analysis::{Arm, Row, Verdict, analyze};
    let mut rows = Vec::new();
    for provider in ["codex", "claude"] {
        for case in [1, 5, 6] {
            for arm in [Arm::Baseline, Arm::Candidate] {
                rows.push(Row {
                    provider: provider.into(),
                    case,
                    arm,
                    inclusive_lower_bound: if arm == Arm::Baseline { 100 } else { 90 },
                    usage_complete: true,
                    correct: true,
                    shared_safety: true,
                });
            }
        }
    }
    for index in [7, 9, 11] {
        rows[index].inclusive_lower_bound = 100;
    }
    let split = analyze(&rows).unwrap();
    assert_eq!(split.providers[0].verdict, Verdict::DeclaredSamplePass);
    assert_eq!(
        split.providers[1].verdict,
        Verdict::Fail,
        "TE15_B2A_PROVIDER_SEPARATE"
    );
    for index in [7, 9, 11] {
        rows[index].inclusive_lower_bound = 90;
    }
    let pass = analyze(&rows).unwrap();
    assert_eq!(pass.verdict, Verdict::DeclaredSamplePass);
    assert_eq!(pass.rows.len(), 12);
    assert!(
        pass.providers
            .iter()
            .all(|p| p.mean_improves && p.median_improves)
    );
    for (index, tokens) in [1, 3, 5].into_iter().zip([110, 110, 1]) {
        rows[index].inclusive_lower_bound = tokens;
    }
    let mixed = analyze(&rows).unwrap();
    assert_eq!(mixed.verdict, Verdict::MixedInconclusive);
    assert!(mixed.providers[0].mean_improves);
    assert!(
        !mixed.providers[0].median_improves,
        "TE15_B2A_MEDIAN_REQUIRED"
    );
    for (index, tokens) in [1, 3, 5].into_iter().zip([90, 90, 200]) {
        rows[index].inclusive_lower_bound = tokens;
    }
    let mixed = analyze(&rows).unwrap();
    assert!(!mixed.providers[0].mean_improves, "TE15_B2A_MEAN_REQUIRED");
    assert!(mixed.providers[0].median_improves);
    // Neither provider passes both aggregates, but a single improved aggregate
    // remains mixed, rather than being reported as a complete token-bar failure.
    for index in [7, 9, 11] {
        rows[index].inclusive_lower_bound = 100;
    }
    assert_eq!(analyze(&rows).unwrap().verdict, Verdict::MixedInconclusive);
    for index in [1, 3, 5] {
        rows[index].inclusive_lower_bound = 100;
    }
    assert_eq!(analyze(&rows).unwrap().verdict, Verdict::Fail);
    rows[7].usage_complete = false;
    rows[7].inclusive_lower_bound = 118;
    let incomplete = analyze(&rows).unwrap();
    assert_eq!(incomplete.verdict, Verdict::Incomplete);
    assert_eq!(incomplete.rows[7].inclusive_lower_bound, 118);
    assert_eq!(analyze(&rows[..11]).unwrap().verdict, Verdict::Incomplete);
    rows[7].correct = false;
    assert_eq!(analyze(&rows).unwrap().verdict, Verdict::StopProvider);
    rows[0].correct = false;
    assert_eq!(
        analyze(&rows).unwrap().verdict,
        Verdict::BaselineOracleFailure
    );
    rows[0].shared_safety = false;
    assert_eq!(analyze(&rows).unwrap().verdict, Verdict::StopAll);
    rows.push(rows[0].clone());
    assert!(analyze(&rows).is_err());
}

mod coverage {
    use kinewright_agent::te1_eval::analysis::{Arm, Row, Verdict, analyze};
    use kinewright_project::orchestration_accounting::*;
    use kinewright_project::usage_stream::*;
    use serde_json::json;
    fn id<T: serde::de::DeserializeOwned>(n: u64) -> T {
        serde_json::from_value(json!(format!("{n:032x}"))).unwrap()
    }
    fn accounting() -> AccountingRecord {
        let mut a = AccountingRecord::new(id(1), id(2), 1000).unwrap();
        a.add_task(id(3), Some(1000)).unwrap();
        let t = &mut a.tasks[0];
        let selection = ProviderSelection {
            connector: "codex".into(),
            model: "fixture".into(),
            options: vec![],
        };
        t.select_provider(ProviderMetadata {
            attempt: id(4),
            requested: selection.clone(),
            resolved: selection,
            cli_version: "0.166.0".into(),
        })
        .unwrap();
        t.bind_stream(StreamBinding {
            attempt: id(4),
            stream: id(5),
            adapter: Adapter::Codex01660V1,
            resumed: false,
        })
        .unwrap();
        t.consume_send(SendConsumption {
            command: kinewright_project::receipts::CommandId {
                issuer: id(6),
                sequence: 1,
            },
            attempt: id(4),
            turn: 1,
            payload: kinewright_project::bundle_records::ObjectRef {
                sha256: "a".repeat(64),
                byte_length: 10,
                record_kind: kinewright_project::bundle_records::RecordKind::OwnedData,
            },
        })
        .unwrap();
        let values = UsageValues {
            input: Some(100),
            output: Some(20),
            cache_read: Some(0),
            ..Default::default()
        };
        t.usage.push(UsageSample {
            snapshot: Some(Snapshot {
                payload_sha256: "b".repeat(64),
                stream: id(5),
                adapter: Adapter::Codex01660V1,
                models: vec![ModelSnapshot {
                    model: "c".repeat(64),
                    values: values.clone(),
                }],
            }),
            attempt: id(4),
            turn: 1,
            event: "result".into(),
            ordinal: 1,
            mode: UsageMode::Cumulative,
            values,
            complete: true,
            input_includes_cache: true,
            output_includes_reasoning: true,
        });
        t.totals = normalize(&t.usage).unwrap();
        a.validate().unwrap();
        a
    }

    fn row(a: &AccountingRecord) -> Row {
        a.validate().unwrap();
        Row::from_accounting("codex", 1, Arm::Candidate, a, true).unwrap()
    }
    fn verdict(a: &AccountingRecord) -> Verdict {
        let row = row(a);
        let mut rows = vec![];
        for provider in ["codex", "claude"] {
            for case in [1, 5, 6] {
                for arm in [Arm::Baseline, Arm::Candidate] {
                    let mut r = row.clone();
                    r.provider = provider.into();
                    r.case = case;
                    r.arm = arm;
                    if arm == Arm::Baseline {
                        r.inclusive_lower_bound = 400;
                        r.usage_complete = true;
                    }
                    rows.push(r);
                }
            }
        }
        analyze(&rows).unwrap().verdict
    }
    fn later() -> AccountingRecord {
        let mut a = accounting();
        let mut send = a.tasks[0].sends[0].clone();
        send.turn = 2;
        send.command.sequence = 2;
        a.tasks[0].consume_send(send).unwrap();
        a
    }
    #[test]
    fn te15_b2af1_uncovered_turn() {
        let a = later();
        assert_eq!(row(&a).inclusive_lower_bound, 120);
        assert_eq!(verdict(&a), Verdict::Incomplete, "TE15_B2AF1_TURN");
    }
    #[test]
    fn te15_b2af1_uncovered_attempt() {
        let mut a = accounting();
        let t = &mut a.tasks[0];
        let mut provider = t.providers[0].clone();
        provider.attempt = id(7);
        t.select_provider(provider).unwrap();
        t.bind_stream(StreamBinding {
            attempt: id(7),
            stream: id(8),
            adapter: Adapter::Codex01660V1,
            resumed: false,
        })
        .unwrap();
        let mut send = t.sends[0].clone();
        send.attempt = id(7);
        send.command.sequence = 2;
        t.consume_send(send).unwrap();
        assert_eq!(row(&a).inclusive_lower_bound, 120);
        assert_eq!(verdict(&a), Verdict::Incomplete, "TE15_B2AF1_ATTEMPT");
    }
    #[test]
    fn te15_b2af1_observation_overflow() {
        let mut a = accounting();
        a.tasks[0].measurements =
            Some(kinewright_project::usage_measurement::Measurements::default());
        for _ in 0..257 {
            a.tasks[0].measurements.as_mut().unwrap().result(
                kinewright_project::usage_measurement::ToolResult {
                    attempt: id(4),
                    turn: None,
                    provider_call: None,
                    host_call: 0,
                    tool: "get_timeline_state".into(),
                    serialized_bytes: 1,
                },
            );
        }
        assert_eq!(row(&a).inclusive_lower_bound, 120);
        assert_eq!(verdict(&a), Verdict::Incomplete, "TE15_B2AF1_OVERFLOW");
    }
    #[test]
    fn te15_b2af1_later_snapshot_closes_gap() {
        let mut a = later();
        assert!(!row(&a).usage_complete);
        let mut sample = a.tasks[0].usage[0].clone();
        sample.turn = 2;
        sample.ordinal = 2;
        sample.event = "result-2".into();
        sample.values.input = Some(200);
        sample.values.output = Some(40);
        let snap = sample.snapshot.as_mut().unwrap();
        snap.payload_sha256 = "d".repeat(64);
        snap.models[0].values = sample.values.clone();
        a.tasks[0].usage.push(sample);
        a.tasks[0].totals = normalize(&a.tasks[0].usage).unwrap();
        assert_eq!(row(&a).inclusive_lower_bound, 240);
        assert!(row(&a).usage_complete, "TE15_B2AF1_RECOVER");
        assert_eq!(
            verdict(&a),
            Verdict::DeclaredSamplePass,
            "TE15_B2AF1_RECOVER"
        );
    }
}
#[cfg(target_os = "linux")]
#[test]
fn te15_b2af1_alternative_outcomes() {
    if !te15_b2a_catalog_child("te15_b2af1_alternative_outcomes") {
        return;
    }
    let temp = kinewright_media::test_support::TempDirectory::new_in(
        &kinewright_media::test_support::workspace_target_dir(),
        "b2a",
    );
    for case in [1, 5, 6] {
        let m = kinewright_agent::te1_eval::run_scripted(
            &temp.path(&format!("case-{case}")),
            case,
            "codex",
            kinewright_agent::te1_eval::ScriptedOutcome::Alternative,
        )
        .unwrap();
        assert!(
            m.checks.passed(),
            "TE15_B2AF1_ALTERNATIVE case={case} {:?}",
            m.checks
        );
    }
}
