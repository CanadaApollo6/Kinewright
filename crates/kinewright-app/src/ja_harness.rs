//! App-private fixture adapter, compiled only by libtest.
use std::path::Path;

use kinewright_agent::ja_harness::{
    self,
    protocol::{Call, LoadedFixture, Mode},
    report::Report,
};
use kinewright_core::{Operation, TimeCode};
use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AppControl {
    batches: Vec<Vec<Operation>>,
    at: i64,
    #[serde(default)]
    rescue: bool,
    #[serde(default)]
    writer_journal: bool,
    #[serde(default)]
    manual_stream: bool,
    #[serde(default)]
    manual_recovery: Option<ManualRecoveryControl>,
    #[serde(default)]
    switch: Option<bool>,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum ManualRecoveryControl {
    Recovery,
    Floor,
}

fn observe(
    action: &ja_harness::protocol::Action,
    loaded: &LoadedFixture,
    report: &mut Report,
) -> Result<Option<Value>, String> {
    let Call::AppFixture(args) = &action.call else {
        return Ok(None);
    };
    let control: AppControl = serde_json::from_value(ja_harness::protocol::unique_json(
        ja_harness::baseline::input(loaded, &args.input)?,
    )?)
    .map_err(|e| e.to_string())?;
    if let Some(reply_drop) = control.switch {
        if report.mode == Mode::Baseline {
            let core = kinewright_core::Core::spawn(kinewright_core::Document::default())
                .map_err(|e| e.to_string())?;
            let query = matches!(
                core.request(kinewright_core::Command::Query(
                    kinewright_core::Query::Snapshot
                ))
                .map_err(|e| e.to_string())?,
                kinewright_core::Event::QueryResult(_)
            );
            return Ok(Some(serde_json::json!({"query":query,"switch":null})));
        }
        #[cfg(target_os = "linux")]
        return Ok(Some(crate::ja1_attachment::tests::observe_c11(reply_drop)));
        #[cfg(not(target_os = "linux"))]
        return Err(format!(
            "C11 App switch observer (reply_drop={reply_drop}) requires Linux; NOT RUN"
        ));
    }
    if control.writer_journal {
        return crate::recovery::g04_gui_journal_observation().map(Some);
    }
    if control.rescue {
        #[cfg(target_os = "linux")]
        return Ok(Some(crate::ja1_attachment::tests::observe_s2(
            report.mode == Mode::Revised,
        )));
        #[cfg(not(target_os = "linux"))]
        return Err("C6 App durable rescue observer requires Linux; NOT RUN".into());
    }
    if let Some(recovery) = control.manual_recovery {
        let floor = matches!(recovery, ManualRecoveryControl::Floor);
        if report.mode == Mode::Baseline {
            let core = kinewright_core::Core::spawn(kinewright_core::Document::default())
                .map_err(|e| e.to_string())?;
            let query = matches!(
                core.request(kinewright_core::Command::Query(
                    kinewright_core::Query::Snapshot
                ))
                .map_err(|e| e.to_string())?,
                kinewright_core::Event::QueryResult(_)
            );
            return Ok(Some(serde_json::json!({"query": query, "recovery": null})));
        }
        #[cfg(target_os = "linux")]
        return Ok(Some(crate::ja1_attachment::tests::observe_c10(floor)));
        #[cfg(not(target_os = "linux"))]
        return Err(format!(
            "C10 App {} observer requires Linux; NOT RUN",
            if floor { "floor" } else { "recovery" }
        ));
    }
    let document = if let Some(role) = &loaded.fixture.initial.document {
        serde_json::from_value::<kinewright_project::ProjectFile>(
            ja_harness::protocol::unique_json(ja_harness::baseline::input(loaded, role)?)?,
        )
        .map_err(|e| e.to_string())?
        .document
    } else {
        kinewright_core::Document::default()
    };
    if !document.media_pool.is_empty() || !document.lut_assets.is_empty() {
        return Err("App supplied-media fixture adapter pending; NOT RUN".into());
    }
    for batch in &control.batches {
        ja_harness::protocol::validate_local_control_operations(batch)?;
    }
    crate::app::ja_observe::ja_observe_current(
        document,
        control.batches,
        TimeCode(control.at),
        report,
        control.manual_stream,
    )
    .map(Some)
}

pub(crate) fn run(fixture: &Path, output: &Path, mode: Mode) -> Result<i32, String> {
    ja_harness::run_with_observer(fixture, output, mode, &mut observe)
}

#[cfg(test)]
mod tests {
    use super::*;
    use kinewright_media::test_support::TempDirectory;

    #[test]
    #[ignore = "on-demand fixture adapter: requires JA_HARNESS_FIXTURE and JA_HARNESS_OUTPUT"]
    fn run_fixture_from_env() {
        let fixture = std::env::var_os("JA_HARNESS_FIXTURE").expect("fixture prerequisite missing");
        let output = std::env::var_os("JA_HARNESS_OUTPUT").expect("output prerequisite missing");
        let mode = match std::env::var("JA_HARNESS_MODE").as_deref() {
            Ok("revised") => Mode::Revised,
            _ => Mode::Baseline,
        };
        let exit = run(Path::new(&fixture), Path::new(&output), mode).unwrap();
        // On-demand native bridge fault injection after the common report is
        // complete, before libtest can supply its normal 0/101 process outcome.
        if std::env::var_os("JA_HARNESS_TEST_ABNORMAL_APP_EXIT").is_some() {
            std::process::exit(86);
        }
        assert_eq!(exit, 0);
    }

    #[test]
    fn coalesced_app_input_reaches_actual_headless_paint() {
        let temp = TempDirectory::new("ja-app");
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../kinewright-agent/tests/fixtures/ja/app-coalesced-paint.json");
        let output = std::env::var_os("JA_HARNESS_RETAIN_OUTPUT").map_or_else(
            || temp.path("out"),
            |root| {
                let root = std::path::PathBuf::from(root);
                std::fs::create_dir_all(&root).unwrap();
                root.join("app")
            },
        );
        assert_eq!(run(&fixture, &output, Mode::Baseline).unwrap(), 0);
        let observation: Value = serde_json::from_slice(
            &std::fs::read(output.join("artifacts/observation-1.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            observation["center_rgba"],
            serde_json::json!([255, 0, 0, 255])
        );
        assert_eq!(
            observation["issued_stamp"],
            observation["bound_display_cell"]["stamp"]
        );
        assert_eq!(
            observation["gui_journal"]["commands"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert!(observation["durable_version_id"].is_null());
    }
    #[test]
    fn gui_journal_additive_fixture_uses_real_recorder() {
        let root = kinewright_media::test_support::workspace_target_dir();
        std::fs::create_dir_all(&root).unwrap();
        let temp = kinewright_media::test_support::TempDirectory::new_in(&root, "c7-gui-fixture");
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../kinewright-agent/tests/fixtures/ja/s2-gui-journal.json");
        for mode in [Mode::Baseline, Mode::Revised] {
            let output = temp.path(if mode == Mode::Baseline {
                "baseline"
            } else {
                "revised"
            });
            assert_eq!(run(&fixture, &output, mode).unwrap(), 0);
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
mod c10_tests {
    use super::*;
    #[test]
    fn additive_manual_recovery_and_floor_fixtures() {
        let root = kinewright_media::test_support::workspace_target_dir();
        std::fs::create_dir_all(&root).unwrap();
        let temp = kinewright_media::test_support::TempDirectory::new_in(&root, "c10-fixtures");
        for label in ["s2-manual-recovery", "s2-manual-floor"] {
            let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!(
                "../kinewright-agent/tests/fixtures/ja/{label}.json"
            ));
            for (mode, name) in [(Mode::Baseline, "baseline"), (Mode::Revised, "revised")] {
                assert_eq!(
                    run(&fixture, &temp.path(&format!("{label}-{name}")), mode).unwrap(),
                    0
                );
            }
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
mod c11_tests {
    use super::*;
    #[test]
    fn additive_switch_reply_drop_and_return_fixtures() {
        let root = kinewright_media::test_support::workspace_target_dir();
        std::fs::create_dir_all(&root).unwrap();
        let temp = kinewright_media::test_support::TempDirectory::new_in(&root, "c11-fixtures");
        for label in ["s2-switch-reply-drop", "s2-switch-return"] {
            let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!(
                "../kinewright-agent/tests/fixtures/ja/{label}.json"
            ));
            for (mode, name) in [(Mode::Baseline, "baseline"), (Mode::Revised, "revised")] {
                let output = std::env::var_os("JA_SWITCH_FIXTURE_OUTPUT").map_or_else(
                    || temp.path(&format!("{label}-{name}")),
                    |root| std::path::PathBuf::from(root).join(format!("{label}-{name}")),
                );
                std::fs::create_dir_all(output.parent().unwrap()).unwrap();
                assert_eq!(run(&fixture, &output, mode).unwrap(), 0);
                let summary: Value =
                    serde_json::from_slice(&std::fs::read(output.join("summary.json")).unwrap())
                        .unwrap();
                assert!(
                    summary["assertions"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .all(|a| a["passed"] == true)
                );
                let properties = summary["assertions"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|a| a["evidence_class"] == "revised_property")
                    .count();
                assert_eq!(properties, if mode == Mode::Revised { 6 } else { 0 });
                assert_eq!(
                    summary["pending"].as_array().unwrap().len(),
                    if mode == Mode::Baseline { 6 } else { 0 }
                );
            }
        }
    }
}
