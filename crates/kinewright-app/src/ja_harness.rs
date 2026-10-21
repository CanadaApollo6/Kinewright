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
    )
    .map(Some)
}

fn run(fixture: &Path, output: &Path, mode: Mode) -> Result<i32, String> {
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
}
