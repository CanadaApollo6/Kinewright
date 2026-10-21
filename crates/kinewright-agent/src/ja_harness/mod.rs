//! Job A's eval-only baseline CLI and assertion runner.
#![allow(clippy::missing_errors_doc)]
pub mod baseline;
pub mod controller;
pub mod media_observer;
pub mod protocol;
pub mod recording;
pub mod report;

use std::{collections::BTreeMap, path::Path, time::Instant};

use serde_json::{Value, json};

use protocol::{Call, EvidenceClass, Lane, LoadedFixture, Mode};
use report::Report;

/// Run a fixture unchanged in either mode. Revised currently uses the same
/// production adapter; it does not invent successor behavior.
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
    let mut report = Report::create(output, mode)?;
    let loaded = match protocol::load(fixture_path) {
        Ok(loaded) => loaded,
        Err(error) => {
            report.header(None)?;
            return report.finish(None, Some(&error));
        }
    };
    let result = (|| {
        report.set_quota(loaded.fixture.limits.output_bytes)?;
        report.header(Some(&loaded))?;
        execute(&loaded, &mut report, observer)
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
) -> Result<(), String> {
    prerequisites(&loaded.fixture)?;
    let mut adapter = baseline::Baseline::new(&report.root.join("work"))?;
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
            match &action.call {
                Call::AssertObserved(assertion) => {
                    assert_observed(assertion, loaded, &observed, report)?;
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
                call => {
                    let custom = extension(action, loaded, report)?;
                    let dispatched = if let Some(value) = custom {
                        Some((value, true))
                    } else if matches!(action.call, Call::AppFixture(_)) {
                        Some((controller::app_fixture(action, loaded, report)?, true))
                    } else if let Some(value) = media.execute(&action.call, &mut adapter, report)? {
                        Some((value, true))
                    } else {
                        controller.dispatch(action, loaded, &observed, report)?
                    };
                    let (observation, delivered) = match dispatched {
                        Some(result) => result,
                        None => (adapter.execute(call, loaded)?, true),
                    };
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
        Ok(())
    })();
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
        "assertion":"actual == independently frozen expected", "actual":actual,"expected":expected,
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
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.first().is_some_and(|a| a == "--role") {
        return controller::worker_cli(&args);
    }
    if args.len() != 6 || args[0] != "--mode" || args[2] != "--fixture" || args[4] != "--output" {
        return Err("usage: kinewright-ja-baseline --mode baseline|revised --fixture FILE --output NEW_DIRECTORY".into());
    }
    let mode = match args[1].as_str() {
        "baseline" => Mode::Baseline,
        "revised" => Mode::Revised,
        _ => return Err("unknown mode".into()),
    };
    let exit = run(Path::new(&args[3]), Path::new(&args[5]), mode)?;
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

    #[test]
    fn edit_save_reopen_has_literal_history_loss_after_successful_control() {
        let temp = TempDirectory::new("ja-history");
        let out = temp.path("out");
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
        let temp = TempDirectory::new("ja-io");
        assert_eq!(
            run(
                &fixtures().join("legacy-io.json"),
                &temp.path("out"),
                Mode::Baseline
            )
            .unwrap(),
            1
        );
        let summary: Value =
            serde_json::from_slice(&std::fs::read(temp.path("out/summary.json")).unwrap()).unwrap();
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
        for name in ["edit-undo-redo", "format-corpus", "branches-current"] {
            let temp = TempDirectory::new("ja-controls");
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
        let temp = TempDirectory::new("ja-recording");
        let out = temp.path("out");
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
        let temp = TempDirectory::new("ja-missing-worker");
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
            let temp = TempDirectory::new("ja-setup");
            let mut fixture: Value = serde_json::from_slice(
                &std::fs::read(fixtures().join("history-loss.json")).unwrap(),
            )
            .unwrap();
            for (key, value) in change.as_object().unwrap() {
                fixture[key] = value.clone();
            }
            let fixture_root = temp.path("fixtures");
            std::fs::create_dir(&fixture_root).unwrap();
            copy_fixtures(&fixture_root);
            let path = fixture_root.join(format!("temporary-{index}.json"));
            std::fs::write(&path, serde_json::to_vec(&fixture).unwrap()).unwrap();
            let result = run(&path, &temp.path("out"), Mode::Baseline);
            std::fs::remove_file(&path).unwrap();
            assert_eq!(result.unwrap(), 2, "{index}");
        }
        assert!(protocol::unique_json(br#"{"a":{"b":1,"b":2}}"#).is_err());
        assert!(protocol::unique_json(br#"{"a":1} {}"#).is_err());
        let exact =
            protocol::unique_json(br#"{"frame":-9223372036854775808,"num":30000,"den":1001}"#)
                .unwrap();
        assert_eq!(exact["frame"].as_i64(), Some(i64::MIN));
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
        let temp = TempDirectory::new("ja-protocol");
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
            let out = temp.path(&format!("out-{i}"));
            assert_eq!(run(&path, &out, Mode::Baseline).unwrap(), 2);
            let summary: Value =
                serde_json::from_slice(&std::fs::read(out.join("summary.json")).unwrap()).unwrap();
            assert_eq!(summary["disposition"], "not_run");
            assert!(summary["assertions"].as_array().unwrap().is_empty());
        }
    }
}
