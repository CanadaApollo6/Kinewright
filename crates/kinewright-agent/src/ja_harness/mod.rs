//! Job A's eval-only baseline CLI and assertion runner.
#![allow(clippy::missing_errors_doc)]
pub mod baseline;
#[cfg(target_os = "linux")]
mod budget_observer;
pub mod controller;
#[cfg(target_os = "linux")]
mod copy_observer;
pub mod media_observer;
pub mod protocol;
#[cfg(target_os = "linux")]
mod receipt_observer;
pub mod recording;
pub mod report;
pub mod revised;
mod s2_observer;
mod supervision;

use std::{collections::BTreeMap, path::Path, time::Instant};

use serde_json::{Value, json};

use protocol::{Call, EvidenceClass, Lane, LoadedFixture, Mode};
use report::Report;

pub const MEDIA_TEARDOWN_MS: u64 = 30_000;
pub const WORKER_REAP_MS: u64 = 2_000;

fn cleanup_timeout_ms(fixture: &protocol::Fixture) -> u64 {
    // Every fixed participant can finish its adapter before its reap allowance.
    // The main adapter follows them; reserve setup's allowance for report sealing.
    let workers = u64::try_from(fixture.initial.participants.len()).unwrap_or(2);
    (workers + 1) * MEDIA_TEARDOWN_MS + workers * WORKER_REAP_MS + fixture.limits.setup_timeout_ms
}

/// Run a fixture unchanged through the selected production adapter.
pub fn run(fixture_path: &Path, output: &Path, mode: Mode) -> Result<i32, String> {
    run_with_observer(fixture_path, output, mode, &mut |_, _, _| Ok(None))
}

pub type Observer<'a> =
    dyn FnMut(&protocol::Action, &LoadedFixture, &mut Report) -> Result<Option<Value>, String> + 'a;

/// App's cfg(test) adapter supplies only its private current-path observations.
pub fn run_with_observer(
    fixture_path: &Path,
    output: &Path,
    mode: Mode,
    observer: &mut Observer<'_>,
) -> Result<i32, String> {
    let report = Report::create(output, mode)?;
    run_report(fixture_path, observer, report, None)
}

pub fn run_comment_baseline(fixture_path: &Path, output: &Path, mode: Mode) -> Result<i32, String> {
    let report = Report::create(output, mode)?;
    run_report(
        fixture_path,
        &mut |_, _, _| Ok(None),
        report,
        Some("comment-baseline"),
    )
}

fn run_report(
    fixture_path: &Path,
    observer: &mut Observer<'_>,
    mut report: Report,
    selection: Option<&str>,
) -> Result<i32, String> {
    if std::env::var_os("JA_HARNESS_DEADLINE_FILE").is_some() {
        supervision::validate_reserved(&report.root)?;
    }
    if let Ok(bytes) = protocol::read_bounded(fixture_path, protocol::INDEX_BYTES)
        && let Ok(value) = protocol::unique_json(&bytes)
        && let Some(ms) = value["limits"]["setup_timeout_ms"]
            .as_u64()
            .filter(|n| (1..=600_000).contains(n))
    {
        supervision::heartbeat(0, "setup", ms)?;
    }
    let loaded = match protocol::load(fixture_path) {
        Ok(loaded) => loaded,
        Err(error) => {
            report.header(None, selection)?;
            if let Ok(bytes) = protocol::read_bounded(fixture_path, protocol::INDEX_BYTES) {
                let identity = json!({"sha256":kinewright_media::sha256_bytes(&bytes),"byte_length":bytes.len()});
                if let Err(reason) = report.artifact("invalid-fixture.json", &bytes) {
                    report.event(
                        0,
                        "controller",
                        "invalid_fixture_not_retained",
                        json!({"identity":identity,"reason":reason}),
                    )?;
                }
            }
            return report.finish(None, Some(&error));
        }
    };
    let result = (|| {
        report.set_quota(loaded.fixture.limits.output_bytes)?;
        report.header(Some(&loaded), selection)?;
        if let Some(selection) = selection {
            if selection != "comment-baseline"
                || loaded.fixture.fixture_id != "comment-receipt"
                || report.mode != Mode::Baseline
            {
                return Err("invalid pre-receipt scenario selection".into());
            }
            report.event(0,"controller","scenario_selection",json!({"selection":selection,"scope":"exact-byte save, lost reply, restart and literal missing binding; later receipt scenarios pending"}))?;
        }
        execute(&loaded, &mut report, observer, selection)
    })();
    report.finish(
        Some(&loaded.fixture),
        result.as_ref().err().map(String::as_str),
    )
}

fn execute(
    loaded: &LoadedFixture,
    report: &mut Report,
    extension: &mut Observer<'_>,
    selection: Option<&str>,
) -> Result<(), String> {
    supervision::heartbeat(0, "setup", loaded.fixture.limits.setup_timeout_ms)?;
    prerequisites(&loaded.fixture)?;
    let mut adapter = revised::Adapter::new(&report.root.join("work"), report.mode)?;
    let mut controller = controller::Controller::new(loaded, report)?;
    let mut observed = BTreeMap::new();
    let mut media = media_observer::MediaObserver::default();
    let mut completed = std::collections::BTreeSet::new();
    let result = (|| {
        for action in &loaded.fixture.actions {
            if action.after.is_some_and(|id| !completed.contains(&id)) {
                return Err("action prerequisite was not observed".into());
            }
            let started = Instant::now();
            supervision::heartbeat(action.id, "action", loaded.fixture.limits.action_timeout_ms)?;
            match &action.call {
                Call::AssertObserved(assertion) => {
                    assert_observed(assertion, loaded, &observed, report)?;
                    if selection == Some("comment-baseline")
                        && assertion.id == "durable-comment-result-binding"
                    {
                        report.pending.push(json!({"disposition":"not_run","selection":"comment-baseline","later_scenarios":["old-epoch-receipt-reconciliation","changed-payload","retired-exact-result","retired-unresolved"],"reason":"pre-receipt source"}));
                        break;
                    }
                    if report
                        .assertions
                        .last()
                        .is_some_and(|a| a["evidence_class"] == "control" && a["passed"] == false)
                    {
                        break;
                    }
                }
                Call::SurfaceAbsent(args) => {
                    require_control(&args.positive_control, report)?;
                    let pending = json!({"disposition":"surface_absent","surface":args.surface,"owner":args.owner,
                    "baseline_witness":"pending_baseline_witness","positive_control":args.positive_control});
                    report.pending.push(pending.clone());
                    report.event(action.id, &action.participant, "pending", pending)?;
                }
                _ => {
                    let routed = controller.dispatch(action, loaded, &observed, report)?;
                    let dispatched = if routed.is_some() {
                        routed
                    } else if let Some(value) = extension(action, loaded, report)? {
                        Some((value, true))
                    } else if matches!(action.call, Call::AppFixture(_)) {
                        controller::app_fixture(action, loaded, report)?;
                        // The exact App entry supplied the whole fixture report.
                        break;
                    } else {
                        media
                            .execute(action, &mut adapter.baseline, report)?
                            .map(|value| (value, true))
                    };
                    let (observation, delivered) = match dispatched {
                        Some(result) => result,
                        None => (adapter.execute(action, loaded)?, true),
                    };
                    let delivered = delivered && controller.deliver_reply(action.id);
                    let adapter_ns = started.elapsed().as_nanos().to_string();
                    let observer_started = Instant::now();
                    let bytes = serde_json::to_vec(&observation).map_err(|e| e.to_string())?;
                    let artifact =
                        report.artifact(&format!("observation-{}.json", action.id), &bytes)?;
                    report.event(
                        action.id,
                        &action.participant,
                        if delivered {
                            "observation"
                        } else {
                            "reply_dropped"
                        },
                        artifact.clone(),
                    )?;
                    report.observations.push(json!({"action_id":action.id,"participant":action.participant,"reply_delivered":delivered,"artifact":artifact}));
                    report.event(action.id, &action.participant, "observer_timing", json!({"adapter_elapsed_ns":adapter_ns,"observer_elapsed_ns":observer_started.elapsed().as_nanos().to_string()}))?;
                    if delivered {
                        observed.insert(action.id, observation);
                    }
                }
            }
            if started.elapsed().as_millis() > u128::from(loaded.fixture.limits.action_timeout_ms) {
                return Err("action deadline exceeded; incomplete evidence".into());
            }
            completed.insert(action.id);
        }
        controller.sample(report)?;
        Ok(())
    })();
    supervision::heartbeat(u32::MAX, "cleanup", cleanup_timeout_ms(&loaded.fixture))?;
    let cleanup = controller.cleanup(report).and_then(|()| {
        let observation = adapter.finish()?;
        report.event(0, "main", "adapter_cleanup", observation)
    });
    match (result, cleanup) {
        (Err(action), Err(cleanup)) => Err(format!("{action}; cleanup: {cleanup}")),
        (Err(error), _) | (_, Err(error)) => Err(error),
        (Ok(()), Ok(())) => Ok(()),
    }
}

fn prerequisites(fixture: &protocol::Fixture) -> Result<(), String> {
    match fixture.lane {
        Lane::WindowsNative if !cfg!(windows) => {
            return Err("missing windows-native prerequisite".into());
        }
        Lane::LinuxNative if !cfg!(target_os = "linux") => {
            return Err("missing linux-native prerequisite".into());
        }
        Lane::Provider => {
            return Err(
                "configured connector/modalities are not certified; provider lane NOT RUN".into(),
            );
        }
        Lane::Practice => {
            return Err("private practice material/oracle hash slots pending; NOT RUN".into());
        }
        _ => {}
    }
    for requirement in &fixture.requirements {
        if requirement
            .strip_prefix("os:")
            .is_some_and(|os| os == std::env::consts::OS)
        {
            continue;
        }
        return Err(format!(
            "missing or uncertified prerequisite: {requirement}"
        ));
    }
    Ok(())
}

fn require_control(id: &str, report: &Report) -> Result<(), String> {
    if report
        .assertions
        .iter()
        .any(|a| a["id"] == id && a["evidence_class"] == "control" && a["passed"] == true)
    {
        Ok(())
    } else {
        Err(format!("missing successful positive control {id}"))
    }
}

fn assert_observed(
    a: &protocol::Assertion,
    loaded: &LoadedFixture,
    observations: &BTreeMap<u32, Value>,
    report: &mut Report,
) -> Result<(), String> {
    if report.assertions.iter().any(|old| old["id"] == a.id) {
        return Err("duplicate assertion id".into());
    }
    if a.evidence_class == EvidenceClass::BaselineProperty {
        require_control(
            a.positive_control
                .as_deref()
                .ok_or("baseline property needs a positive control")?,
            report,
        )?;
    }
    let actual = observations
        .get(&a.observation)
        .ok_or("assertion observation unavailable")?
        .pointer(&a.pointer)
        .ok_or("assertion pointer unavailable")?;
    let oracle_bytes = baseline::input(loaded, &format!("oracle:{}", a.oracle))?;
    let oracle = if loaded.fixture.oracles[&a.oracle].kind == "writer-bytes" {
        Value::String(String::from_utf8(oracle_bytes.to_vec()).map_err(|e| e.to_string())?)
    } else {
        protocol::unique_json(oracle_bytes)?
    };
    let expected = oracle
        .pointer(&a.oracle_pointer)
        .ok_or("oracle pointer unavailable")?;
    let passed = match a.comparison {
        protocol::Comparison::Equal => actual == expected,
        protocol::Comparison::AtMost => {
            actual
                .as_u64()
                .ok_or("at_most needs exact unsigned integer observation")?
                <= expected
                    .as_u64()
                    .ok_or("at_most needs exact unsigned integer oracle")?
        }
    };
    let detail = json!({"id":a.id,"evidence_class":a.evidence_class,"passed":passed,
        "assertion":match a.comparison { protocol::Comparison::Equal => "actual == independently frozen expected", protocol::Comparison::AtMost => "actual <= independent tolerance" }, "comparison":a.comparison,"actual":actual,"expected":expected,
        "observation":a.observation,"pointer":a.pointer,"oracle":a.oracle,"oracle_pointer":a.oracle_pointer,
        "positive_control":a.positive_control,"disposition":if passed {"pass_control"} else if a.evidence_class == EvidenceClass::BaselineProperty {"baseline_red"} else {"control_failed"}});
    let artifact = report.artifact(
        &format!("assertion-{}.json", report.assertions.len()),
        &serde_json::to_vec(&detail).map_err(|e| e.to_string())?,
    )?;
    report.event(a.observation, "controller", "assertion", artifact.clone())?;
    let mut summary = detail;
    summary
        .as_object_mut()
        .expect("assertion object")
        .remove("actual");
    summary
        .as_object_mut()
        .expect("assertion object")
        .remove("expected");
    summary["artifact"] = artifact;
    report.assertions.push(summary);
    Ok(())
}

/// Fixed CLI arguments only; stdout contains one protocol summary record.
pub fn cli() -> Result<i32, String> {
    let mut args = std::env::args().skip(1).collect::<Vec<_>>();
    let inner = args.starts_with(&["--role".into(), "fixture".into()]);
    if inner {
        args.drain(..2);
    }
    if args.first().is_some_and(|a| a == "--role") {
        return controller::worker_cli(&args);
    }
    let selection = if args.len() == 8 && args[6] == "--scenario" && args[7] == "comment-baseline" {
        Some("comment-baseline")
    } else {
        None
    };
    if (args.len() != 6 && selection.is_none())
        || args[0] != "--mode"
        || args[2] != "--fixture"
        || args[4] != "--output"
    {
        return Err("usage: kinewright-ja-baseline --mode baseline|revised --fixture FILE --output NEW_DIRECTORY".into());
    }
    let mode = match args[1].as_str() {
        "baseline" => Mode::Baseline,
        "revised" => Mode::Revised,
        _ => return Err("unknown mode".into()),
    };
    if inner {
        let root = Path::new(&args[5]);
        supervision::validate_reserved(root)?;
        let report = Report::create_reserved(root, mode)?;
        return run_report(
            Path::new(&args[3]),
            &mut |_, _, _| Ok(None),
            report,
            selection,
        );
    }
    let exit = supervision::run(Path::new(&args[3]), Path::new(&args[5]), mode, selection)?;
    println!(
        "{}",
        json!({"protocol":protocol::PROTOCOL,"protocol_version":1,"exit_code":exit,"output":args[5]})
    );
    Ok(exit)
}

#[cfg(test)]
mod tests {
    use super::*;
    use kinewright_media::test_support::TempDirectory;

    fn fixtures() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ja")
    }

    fn output(temp: &TempDirectory, label: &str) -> std::path::PathBuf {
        if let Some(root) = std::env::var_os("JA_HARNESS_RETAIN_OUTPUT") {
            let root = std::path::PathBuf::from(root);
            std::fs::create_dir_all(&root).unwrap();
            root.join(label)
        } else {
            temp.path(label)
        }
    }

    #[test]
    fn edit_save_reopen_has_literal_history_loss_after_successful_control() {
        let temp = TempDirectory::new_in(
            &kinewright_media::test_support::workspace_target_dir(),
            "ja-history",
        );
        let out = output(&temp, "history");
        assert_eq!(
            run(&fixtures().join("history-loss.json"), &out, Mode::Baseline).unwrap(),
            1
        );
        let summary: Value =
            serde_json::from_slice(&std::fs::read(out.join("summary.json")).unwrap()).unwrap();
        assert_eq!(summary["positive_controls"][0]["passed"], true);
        let red = summary["assertions"].as_array().unwrap().last().unwrap();
        assert_eq!(red["disposition"], "baseline_red");
        let detail: Value = serde_json::from_slice(
            &std::fs::read(out.join(red["artifact"]["relative_file"].as_str().unwrap())).unwrap(),
        )
        .unwrap();
        assert_eq!(detail["actual"], 0);
        assert_eq!(detail["expected"], 1);
    }

    #[test]
    fn current_io_control_precedes_missing_directory_sync_assertion() {
        let temp = TempDirectory::new_in(
            &kinewright_media::test_support::workspace_target_dir(),
            "ja-io",
        );
        let out = output(&temp, "io");
        assert_eq!(
            run(&fixtures().join("legacy-io.json"), &out, Mode::Baseline).unwrap(),
            1
        );
        let summary: Value =
            serde_json::from_slice(&std::fs::read(out.join("summary.json")).unwrap()).unwrap();
        assert!(
            summary["positive_controls"]
                .as_array()
                .unwrap()
                .iter()
                .all(|a| a["passed"] == true)
        );
        assert_eq!(
            summary["assertions"].as_array().unwrap().last().unwrap()["disposition"],
            "baseline_red"
        );
    }

    #[test]
    fn existing_edit_undo_redo_and_format_corpus_controls_pass() {
        for name in [
            "edit-undo-redo",
            "format-corpus",
            "branches-current",
            "clip-id-reuse",
        ] {
            let temp = TempDirectory::new_in(
                &kinewright_media::test_support::workspace_target_dir(),
                "ja-controls",
            );
            if name == "format-corpus" {
                kinewright_project::project::predecessor_format_controls(&temp.path("predecessor"))
                    .expect("actual pinned max-2 controls");
                continue;
            }
            assert_eq!(
                run(
                    &fixtures().join(format!("{name}.json")),
                    &temp.path("out"),
                    Mode::Baseline
                )
                .unwrap(),
                0
            );
        }
    }

    #[test]
    fn recording_forwards_real_loopback_sessions_and_confirmation() {
        let temp = TempDirectory::new_in(
            &kinewright_media::test_support::workspace_target_dir(),
            "ja-recording",
        );
        let out = output(&temp, "recording");
        assert_eq!(
            run(
                &fixtures().join("recording-loopback.json"),
                &out,
                Mode::Baseline
            )
            .unwrap(),
            0
        );
        let read = |id| -> Value {
            serde_json::from_slice(
                &std::fs::read(out.join(format!("artifacts/observation-{id}.json"))).unwrap(),
            )
            .unwrap()
        };
        let sessions = read(3);
        assert_eq!(sessions["server_requests"], 2);
        assert_eq!(sessions["server_results"], 2);
        let events = sessions["sessions"][0]["events"].as_array().unwrap();
        let cost = events
            .iter()
            .find(|e| e["kind"] == "pump_event" && e["event"]["kind"] == "cost")
            .unwrap();
        assert!(cost["event"]["cached_input_tokens"].is_null());
        let second = sessions["sessions"][1]["events"].as_array().unwrap();
        assert!(second.iter().any(|e| {
            e["event"]["kind"] == "tool_result"
                && e["event"]["result"]
                    .as_str()
                    .is_some_and(|s| s.contains("internal capability"))
        }));
        assert_eq!(
            read(8)["trace"]["confirmations"].as_array().unwrap().len(),
            1
        );
        assert_eq!(read(11)["trace"]["confirmations"][0]["approved"], false);
    }

    #[test]
    fn a_test_executable_cannot_silently_substitute_for_the_native_worker() {
        let temp = TempDirectory::new_in(
            &kinewright_media::test_support::workspace_target_dir(),
            "ja-missing-worker",
        );
        assert_eq!(
            run(
                &fixtures().join("reply-retry.json"),
                &temp.path("out"),
                Mode::Baseline
            )
            .unwrap(),
            2
        );
        let summary: Value =
            serde_json::from_slice(&std::fs::read(temp.path("out/summary.json")).unwrap()).unwrap();
        assert_eq!(summary["disposition"], "not_run");
        assert!(
            summary["missing_requirements"]
                .as_str()
                .unwrap()
                .contains("native baseline worker binary")
        );
    }

    #[test]
    fn pending_and_unavailable_lanes_never_become_behavioral_reds_or_gate_passes() {
        for (name, expected) in [
            ("pending-only", 3),
            ("aw2-capability-gap", 0),
            ("provider-not-run", 2),
            ("practice-not-run", 2),
            ("windows-not-run", 2),
        ] {
            let temp = TempDirectory::new_in(
                &kinewright_media::test_support::workspace_target_dir(),
                "ja-disposition",
            );
            let out = temp.path("out");
            assert_eq!(
                run(
                    &fixtures().join(format!("{name}.json")),
                    &out,
                    Mode::Baseline
                )
                .unwrap(),
                expected
            );
            let summary: Value =
                serde_json::from_slice(&std::fs::read(out.join("summary.json")).unwrap()).unwrap();
            assert!(
                summary["assertions"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|a| a["disposition"] != "baseline_red")
            );
            assert!(
                summary["gates"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|g| g["disposition"] == "pending_baseline_witness"
                        && g["owner"].is_string())
            );
            if name == "aw2-capability-gap" {
                assert_eq!(summary["pending"][0]["disposition"], "surface_absent");
            }
        }
    }

    #[test]
    fn loopback_fixture_admission_cannot_write_to_an_external_path() {
        let temp = TempDirectory::new_in(
            &kinewright_media::test_support::workspace_target_dir(),
            "ja-loopback-path",
        );
        copy_fixtures(temp.root());
        let mut fixture: Value = serde_json::from_slice(
            &std::fs::read(fixtures().join("recording-loopback.json")).unwrap(),
        )
        .unwrap();
        let script = json!({"sessions":[{"prompts":["fixture data"],"turns":[{"calls":[{"tool":"invoke_capability","arguments":{"name":"export_current_cut","arguments":{"output":"/not-owned"}}}],"message":"data","cost":null}]}]});
        let bytes = serde_json::to_vec(&script).unwrap();
        std::fs::write(temp.path("unsafe-script.json"), &bytes).unwrap();
        fixture["inputs"]["script"] = json!({"relative_file":"unsafe-script.json","sha256":kinewright_media::sha256_bytes(&bytes),"byte_length":bytes.len(),"kind":"script"});
        let path = temp.path("fixture.json");
        std::fs::write(&path, serde_json::to_vec(&fixture).unwrap()).unwrap();
        let out = temp.path("out");
        assert_eq!(run(&path, &out, Mode::Baseline).unwrap(), 2);
        let summary: Value =
            serde_json::from_slice(&std::fs::read(out.join("summary.json")).unwrap()).unwrap();
        assert!(
            summary["observations"].as_array().unwrap().is_empty(),
            "refuse fixture before starting Core or server"
        );
        assert!(
            summary["missing_requirements"]
                .as_str()
                .unwrap()
                .contains("external IO adapter pending")
        );
    }

    #[test]
    fn input_roles_cannot_alias_the_oracle_namespace() {
        let temp = TempDirectory::new_in(
            &kinewright_media::test_support::workspace_target_dir(),
            "ja-role-collision",
        );
        copy_fixtures(temp.root());
        let mut fixture: Value =
            serde_json::from_slice(&std::fs::read(fixtures().join("history-loss.json")).unwrap())
                .unwrap();
        fixture["inputs"]["oracle:edit"] = fixture["inputs"]["edit"].clone();
        let undo = std::fs::read(fixtures().join("command-undo.json")).unwrap();
        fixture["oracles"]["edit"] = json!({"relative_file":"command-undo.json","sha256":kinewright_media::sha256_bytes(&undo),"byte_length":undo.len(),"kind":"control"});
        fixture["actions"][1]["args"]["input"] = json!("oracle:edit");
        let path = temp.path("collision.json");
        std::fs::write(&path, serde_json::to_vec(&fixture).unwrap()).unwrap();
        let out = output(&temp, "role-collision");
        assert_eq!(run(&path, &out, Mode::Baseline).unwrap(), 2);
        let summary: Value =
            serde_json::from_slice(&std::fs::read(out.join("summary.json")).unwrap()).unwrap();
        assert!(summary["observations"].as_array().unwrap().is_empty());
        assert!(
            summary["missing_requirements"]
                .as_str()
                .unwrap()
                .contains("reserved oracle namespace")
        );
    }

    #[test]
    fn media_adapter_refuses_a_non_main_participant_before_execution() {
        let temp = TempDirectory::new_in(
            &kinewright_media::test_support::workspace_target_dir(),
            "ja-media-participant",
        );
        copy_fixtures(temp.root());
        let mut fixture: Value = serde_json::from_slice(
            &std::fs::read(fixtures().join("media-shared-control.json")).unwrap(),
        )
        .unwrap();
        fixture["actions"][1]["participant"] = json!("A");
        let path = temp.path("participant.json");
        std::fs::write(&path, serde_json::to_vec(&fixture).unwrap()).unwrap();
        let out = output(&temp, "participant-routing");
        assert_eq!(run(&path, &out, Mode::Baseline).unwrap(), 2);
        let summary: Value =
            serde_json::from_slice(&std::fs::read(out.join("summary.json")).unwrap()).unwrap();
        assert!(summary["observations"].as_array().unwrap().is_empty());
        assert!(
            summary["missing_requirements"]
                .as_str()
                .unwrap()
                .contains("requires main participant")
        );
    }

    #[test]
    fn main_adapter_uses_shared_reply_drop_and_retry_validation() {
        let temp = TempDirectory::new_in(
            &kinewright_media::test_support::workspace_target_dir(),
            "ja-main-dispatch",
        );
        copy_fixtures(temp.root());
        let mut fixture: Value =
            serde_json::from_slice(&std::fs::read(fixtures().join("reply-retry.json")).unwrap())
                .unwrap();
        fixture["initial"]["participants"] = json!([]);
        fixture["actions"]
            .as_array_mut()
            .unwrap()
            .retain(|a| a["action"] != "participant_exit");
        for action in fixture["actions"].as_array_mut().unwrap() {
            if action["participant"] == "A" {
                action["participant"] = json!("main");
            }
        }
        let path = temp.path("main-retry.json");
        std::fs::write(&path, serde_json::to_vec(&fixture).unwrap()).unwrap();
        let out = output(&temp, "main-retry");
        assert_eq!(run(&path, &out, Mode::Baseline).unwrap(), 1);
        let summary: Value =
            serde_json::from_slice(&std::fs::read(out.join("summary.json")).unwrap()).unwrap();
        assert!(
            summary["observations"]
                .as_array()
                .unwrap()
                .iter()
                .any(|a| a["action_id"] == 6 && a["reply_delivered"] == false)
        );
        assert!(
            summary["positive_controls"]
                .as_array()
                .unwrap()
                .iter()
                .all(|a| a["passed"] == true)
        );

        let mut fixture: Value = serde_json::from_slice(
            &std::fs::read(fixtures().join("media-shared-control.json")).unwrap(),
        )
        .unwrap();
        fixture["actions"].as_array_mut().unwrap().push(
            json!({"id":10,"participant":"main","retry_of":2,"action":"media_stats","args":{}}),
        );
        let path = temp.path("changed-retry.json");
        std::fs::write(&path, serde_json::to_vec(&fixture).unwrap()).unwrap();
        let out = output(&temp, "changed-retry");
        assert_eq!(run(&path, &out, Mode::Baseline).unwrap(), 2);
        let summary: Value =
            serde_json::from_slice(&std::fs::read(out.join("summary.json")).unwrap()).unwrap();
        assert!(
            summary["missing_requirements"]
                .as_str()
                .unwrap()
                .contains("retry changed original action bytes/participant")
        );
    }

    #[test]
    fn repeated_media_actions_retain_distinct_complete_artifacts() {
        let temp = TempDirectory::new_in(
            &kinewright_media::test_support::workspace_target_dir(),
            "ja-media-repeat",
        );
        let out = output(&temp, "repeated-media");
        assert_eq!(
            run(
                &fixtures().join("media-repeated-control.json"),
                &out,
                Mode::Baseline
            )
            .unwrap(),
            0
        );
        let mut paths = std::collections::BTreeSet::new();
        for (id, field) in [
            (2, "raw_rgba"),
            (4, "raw_rgba"),
            (5, "output"),
            (6, "raw_rgba"),
            (10, "raw_rgba"),
            (11, "raw_rgba"),
            (12, "output"),
            (13, "raw_rgba"),
        ] {
            let value: Value = serde_json::from_slice(
                &std::fs::read(out.join(format!("artifacts/observation-{id}.json"))).unwrap(),
            )
            .unwrap();
            let reference = &value[field];
            let name = reference["relative_file"].as_str().unwrap();
            assert!(
                paths.insert(name.to_owned()),
                "each action retains its own evidence"
            );
            let bytes = std::fs::read(out.join(name)).unwrap();
            assert_eq!(
                bytes.len() as u64,
                reference["byte_length"].as_u64().unwrap()
            );
            assert_eq!(kinewright_media::sha256_bytes(&bytes), reference["sha256"]);
        }
        assert_eq!(paths.len(), 8);
    }

    #[test]
    fn malformed_protocol_and_prerequisites_are_setup_not_red() {
        for (index, change) in [
            ("version", json!({"protocol_version":2})),
            (
                "escape",
                json!({"inputs":{"bad":{"relative_file":"../escape","sha256":"0","byte_length":0,"kind":"opaque"}}}),
            ),
            (
                "quota",
                json!({"limits":{"input_bytes":1,"output_bytes":1,"action_timeout_ms":1,"setup_timeout_ms":1}}),
            ),
            ("prerequisite", json!({"requirements":["device:missing"]})),
        ] {
            let temp = TempDirectory::new_in(
                &kinewright_media::test_support::workspace_target_dir(),
                "ja-setup",
            );
            let mut fixture: Value = serde_json::from_slice(
                &std::fs::read(fixtures().join("history-loss.json")).unwrap(),
            )
            .unwrap();
            for (key, value) in change.as_object().unwrap() {
                fixture[key] = value.clone();
            }
            if index == "escape" {
                fixture["inputs"] = serde_json::from_slice::<Value>(
                    &std::fs::read(fixtures().join("history-loss.json")).unwrap(),
                )
                .unwrap()["inputs"]
                    .clone();
                fixture["inputs"]["edit"]["relative_file"] = json!("../escape");
            }
            let fixture_root = temp.path("fixtures");
            std::fs::create_dir(&fixture_root).unwrap();
            copy_fixtures(&fixture_root);
            let path = fixture_root.join(format!("temporary-{index}.json"));
            std::fs::write(&path, serde_json::to_vec(&fixture).unwrap()).unwrap();
            let result = run(&path, &temp.path("out"), Mode::Baseline);
            std::fs::remove_file(&path).unwrap();
            assert_eq!(result.unwrap(), 2, "{index}");
            if index == "escape" {
                let summary: Value =
                    serde_json::from_slice(&std::fs::read(temp.path("out/summary.json")).unwrap())
                        .unwrap();
                assert!(
                    summary["missing_requirements"]
                        .as_str()
                        .unwrap()
                        .contains("path escape")
                );
            }
        }
        assert!(protocol::unique_json(br#"{"a":{"b":1,"b":2}}"#).is_err());
        assert!(protocol::unique_json(br#"{"a":1} {}"#).is_err());
        let exact =
            protocol::unique_json(br#"{"frame":-9223372036854775808,"num":30000,"den":1001}"#)
                .unwrap();
        assert_eq!(exact["frame"].as_i64(), Some(i64::MIN));
    }

    #[test]
    fn failed_positive_control_has_control_failed_summary_and_exit_one() {
        let temp = TempDirectory::new_in(
            &kinewright_media::test_support::workspace_target_dir(),
            "ja-control-failed",
        );
        let directory = temp.path("fixtures");
        std::fs::create_dir(&directory).unwrap();
        copy_fixtures(&directory);
        let path = directory.join("history-loss.json");
        let mut fixture: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        fixture["actions"][2]["args"]["oracle"] = json!("true");
        fixture["oracles"]["true"] = serde_json::from_slice::<Value>(
            &std::fs::read(directory.join("legacy-io.json")).unwrap(),
        )
        .unwrap()["oracles"]["true"]
            .clone();
        std::fs::write(&path, serde_json::to_vec(&fixture).unwrap()).unwrap();
        let out = temp.path("out");
        assert_eq!(run(&path, &out, Mode::Baseline).unwrap(), 1);
        let summary: Value =
            serde_json::from_slice(&std::fs::read(out.join("summary.json")).unwrap()).unwrap();
        assert_eq!(summary["disposition"], "control_failed");
        assert!(
            summary["assertions"]
                .as_array()
                .unwrap()
                .iter()
                .any(|a| a["disposition"] == "control_failed")
        );
        assert!(
            !summary["assertions"]
                .as_array()
                .unwrap()
                .iter()
                .any(|a| a["disposition"] == "baseline_red")
        );
    }

    #[test]
    fn every_path_argument_is_refused_at_admission_before_any_action() {
        let source: Value =
            serde_json::from_slice(&std::fs::read(fixtures().join("pending-only.json")).unwrap())
                .unwrap();
        for name in [
            "project_save",
            "project_reopen",
            "inspect_files",
            "legacy_rename_refusal",
            "strict_save",
            "strict_rename_refusal",
            "lock_acquire",
            "headless_save",
            "media_export",
            "media_decode",
        ] {
            for path in ["../escape", "/absolute"] {
                let temp = TempDirectory::new_in(
                    &kinewright_media::test_support::workspace_target_dir(),
                    "ja-path-admission",
                );
                let mut fixture = source.clone();
                let mut args = json!({"path":path});
                if name.starts_with("strict_") {
                    args["input"] = json!("unused");
                }
                if name == "media_decode" {
                    args["at"] = json!(0);
                }
                fixture["actions"] = json!([
                    {"id":1,"participant":"main","action":"core_start","args":{}},
                    {"id":2,"participant":"main","action":name,"args":args}]);
                let file = temp.path("fixture.json");
                std::fs::write(&file, serde_json::to_vec(&fixture).unwrap()).unwrap();
                let loaded = protocol::load(&file);
                assert!(
                    loaded
                        .as_ref()
                        .err()
                        .is_some_and(|e| e.contains("path escape")),
                    "{name} {path}: {:?}",
                    loaded.err()
                );
            }
        }
    }

    #[test]
    fn cleanup_deadline_outlives_all_fixed_role_teardown_allowances() {
        #[cfg(unix)]
        controller::witness_worker_teardown_window();
        let mut fixture = protocol::load(&fixtures().join("participants-lock.json"))
            .unwrap()
            .fixture;
        let teardown = 3 * MEDIA_TEARDOWN_MS + 2 * WORKER_REAP_MS;
        assert!(cleanup_timeout_ms(&fixture) > teardown);
        fixture.limits.setup_timeout_ms += 1234;
        assert_eq!(
            cleanup_timeout_ms(&fixture),
            teardown + fixture.limits.setup_timeout_ms
        );
    }

    fn copy_fixtures(destination: &Path) {
        for entry in std::fs::read_dir(fixtures()).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_file() {
                std::fs::copy(entry.path(), destination.join(entry.file_name())).unwrap();
            }
        }
    }

    #[test]
    fn duplicate_oversize_and_guarded_undo_are_not_run() {
        let temp = TempDirectory::new_in(
            &kinewright_media::test_support::workspace_target_dir(),
            "ja-protocol",
        );
        copy_fixtures(temp.root());
        let raw = std::fs::read_to_string(fixtures().join("edit-undo-redo.json")).unwrap();
        let mut oversize: Value = serde_json::from_str(&raw).unwrap();
        oversize["actions"][0] = json!({"id":1,"participant":"main","action":"surface_absent","args":{"surface":"x".repeat(protocol::CONTROL_BYTES),"owner":"future","positive_control":"control"}});
        let mut guarded: Value = serde_json::from_str(&raw).unwrap();
        guarded["actions"][3]["args"]["expected_revision"] = json!(1);
        for (i, bytes) in [
            raw.replacen(
                "\"protocol_version\": 1",
                "\"protocol_version\": 1, \"protocol_version\": 1",
                1,
            )
            .into_bytes(),
            serde_json::to_vec(&oversize).unwrap(),
            serde_json::to_vec(&guarded).unwrap(),
        ]
        .into_iter()
        .enumerate()
        {
            let path = temp.path(&format!("bad-{i}.json"));
            std::fs::write(&path, bytes).unwrap();
            let out = output(&temp, &format!("protocol-{i}"));
            assert_eq!(run(&path, &out, Mode::Baseline).unwrap(), 2);
            let summary: Value =
                serde_json::from_slice(&std::fs::read(out.join("summary.json")).unwrap()).unwrap();
            assert_eq!(summary["disposition"], "not_run");
            assert!(summary["assertions"].as_array().unwrap().is_empty());
        }
    }
}
