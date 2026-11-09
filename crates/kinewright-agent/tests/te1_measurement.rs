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
        std::fs::write(codex.join("models_cache.json"),br#"{"models":[{"slug":"gpt-6.1-sol","supported_reasoning_levels":[{"effort":"high"}],"service_tiers":[{"id":"priority","name":"Fast"}]}]}"#).unwrap();
        return true;
    }
    let temp = kinewright_media::test_support::TempDirectory::new_in(
        &kinewright_media::test_support::workspace_target_dir(),
        "te15-b2a-catalog",
    );
    for dir in ["home", "codex"] {
        std::fs::create_dir(temp.path(dir)).unwrap();
    }
    let log = std::fs::File::create(temp.path("child.log")).unwrap();
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", name, "--nocapture"])
        .env(CHILD, name)
        .env("HOME", temp.path("home"))
        .env("CODEX_HOME", temp.path("codex"))
        .env("CLAUDE_CONFIG_DIR", temp.path("home/.claude"))
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
    for line in output.lines().filter(|line| line.starts_with("TE15_B2A_")) {
        eprintln!("{line}");
    }
    false
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
            "TE15_B2A_ORACLE case={case} provider={provider} wrong=RED restored=GREEN real_provider_launches=0 native_program_max_bytes={} checks={:?}",
            correct.trace.recipe_bytes.iter().max().unwrap(),
            correct.checks
        );
    }
}
#[cfg(target_os = "linux")]
fn task_scopes(correct: &kinewright_agent::te1_eval::Measurement, case: u8) {
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
