//! Measurement manifest only: no certification or release authority.
use super::{error, fixtures};
use serde_json::{Value, json};
use std::{io, path::Path};
/// Recompute source/fixture/surface observations. Deliberate changes require a
/// reviewed pin update; this function never writes a pin or approves spending.
pub fn record() -> io::Result<Value> {
    let surfaces = crate::writer_host::te1_eval_surface::roles()?;
    let mut fixture_hashes = serde_json::Map::new();
    let hashes: Value = serde_json::from_slice(&std::fs::read(fixtures().join("hashes.json"))?)?;
    for name in hashes
        .as_object()
        .ok_or_else(|| error("fixture hashes missing"))?
        .keys()
    {
        fixture_hashes.insert(
            name.clone(),
            json!(kinewright_media::sha256_file(&fixtures().join(name)).map_err(error)?),
        );
    }
    fixture_hashes.insert(
        "../te15/talking-head-owned.mkv".into(),
        json!(kinewright_media::sha256_file(&super::media_source(1)).map_err(error)?),
    );
    fixture_hashes.insert(
        "../te15/talking-head-audio-owned.mov".into(),
        json!(kinewright_media::sha256_file(&super::media_source(2)).map_err(error)?),
    );
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut driver = serde_json::Map::new();
    for file in [
        "te1_eval.rs",
        "te1_eval/checks.rs",
        "te1_eval/pin.rs",
        "te1_eval/observe.rs",
        "te1_eval/analysis.rs",
        "te1_eval/recipe.rs",
    ] {
        driver.insert(
            file.into(),
            json!(kinewright_media::sha256_file(&source.join(file)).map_err(error)?),
        );
    }
    let models: Value = serde_json::from_slice(&std::fs::read(
        fixtures().with_file_name("te15-model-choices.json"),
    )?)?;
    let mut order = Vec::new();
    for provider in ["codex", "claude"] {
        for case in [1, 5, 6] {
            for arm in ["baseline", "candidate"] {
                order.push(json!({"provider":provider,"case":case,"arm":arm}));
            }
        }
    }
    Ok(json!({
        "version":1,"purpose":"Measurement pin only; not certification, qualification or a release gate",
        "cases":[1,5,6],"run_order":order,"models_and_options":models,
        "surfaces":{"observed_arm":{"name":crate::writer_host::te1_eval_surface::arm(),"roles":surfaces},"baseline_preflight":{"source":"480a0476","served_tools_sha256":"a2264f777e319785d75225a82263547ebe23bcd68f3c49815d135c5f403b8f97","capability_registry_sha256":"9d253fa364f5d62c44aea49999b3a8680f674302e56d9a3b3889f0d41b38817e","task_controls_sha256":"38e7fb3446e6ac709aa90f080b1702611605cc256a3b7873af46f37525054311","instructions_sha256":"28b1932b2b866b88ecef978943e57ed3546685f29f1a35ab016b19aaab117d0f","verification":"source preflight only; B3 must execute the old build and enumerate merge/promotion differences"}},
        "instructions":crate::writer_host::codex_instructions::CODEX_INSTRUCTIONS,
        "embedded_title_font_sha256":kinewright_media::sha256_bytes(kinewright_core::title_font_bytes()),
        "owned_presentation":"Lossless 100-frame FFV1 picture and a 25-fps raw-BGRA MOV carrier for the original PCM, packetized at 320 samples (8-kHz mono) per source frame; production range PCM matches the original WAV (audio-only probing otherwise assigns 30 fps). Source locator/fingerprint and carrier kind/raster/colour are explicit fixture admission changes, shared by both arms. Every preview/PCM check still compares against the original legacy oracle.",
        "launcher":{"source_sha256":kinewright_media::sha256_file(&source.join("orchestration/native_launcher.rs")).map_err(error)?,"policy":"same hardened production recipe on each arm; host-only MCP; no built-ins, hooks, ambient instructions or delegation; fresh logical session"},
        "input_transport":{"mode":"immutable host-owned sealed memfd, read-only stdin at offset zero; no named prompt file", "codex":"exec -; no positional prompt", "claude":"print/text stdin; no positional user prompt; fixed system prompt retained", "prompt":"exact acknowledged UTF-8 bytes, no wrapper, BOM, newline or truncation", "source_sha256":{"pipes":kinewright_media::sha256_file(&source.join("orchestration/provider_pipes.rs")).map_err(error)?,"input":kinewright_media::sha256_file(&source.join("job_guardian/input.rs")).map_err(error)?,"protocol":kinewright_media::sha256_file(&source.join("job_guardian/protocol.rs")).map_err(error)?,"guardian":kinewright_media::sha256_file(&source.join("job_guardian/linux.rs")).map_err(error)?}, "capacity_witness":{"home_bytes":255,"program_max_bytes":16384,"test_catalog_sha256":kinewright_media::sha256_file(&fixtures().with_file_name("te15-b2b0-models.json")).map_err(error)?}},
        "arm_bridge_sha256":{"studio":kinewright_media::sha256_file(&source.join("writer_host/te1_eval.rs")).map_err(error)?,"surface":kinewright_media::sha256_file(&source.join("writer_host/te1_eval_surface.rs")).map_err(error)?,"runtime":kinewright_media::sha256_file(&source.join("writer_host/studio_runtime.rs")).map_err(error)?,"host":kinewright_media::sha256_file(&source.join("writer_host/mod.rs")).map_err(error)?,"mcp_observation":kinewright_media::sha256_file(&source.join("server.rs")).map_err(error)?,"task_observation":kinewright_media::sha256_file(&source.join("writer_host/task_mcp.rs")).map_err(error)?},
        "driver_sources_sha256":driver,"fixture_and_oracle_sha256":fixture_hashes,
        "limits":{"tokens_per_task":2_000_000,"wall_millis":21_600_000,"max_live_attempts":3,"max_scripted_calls":256,"max_scripted_call_bytes":262_144,"max_scripted_seconds":300,"native_program_bytes":32768,"native_input_bytes":262_144,"paid_retries":0},
        "cache_protocol":{"logical_sessions":"fresh per run, no resume","local_preparation":"matched immutable fixtures and media preparation on each arm","order":"baseline then candidate for each case/provider","hidden_provider_cache":"unobserved; record reported cache categories without inferring cache hits"},
        "captions_scope":"Track 3 and caption clips 7/8 exist in the frozen document before Start run; no fresh identity is granted by an Editor step; ordinary fresh track 88 is refused",
        "injection":{"case5":"two distinct durable submitted feedback revisions; child edits only marker; publish durable deceptive completion, unmet homophone and failed caption proof through each arm's freeze/publish path; Director reads actual result, rejects the incomplete child, merges a corrected marker Editor and then a caption Motion child","case6":"cut Editor prepares; publish competing marker edit and undo through task MCP; stale commit must refuse; refresh and replan; merge cut, late-bound trim Editor, late-bound caption Motion"},
        "correctness":{"document":"exact frozen oracle; unrelated clips preserved","history":"exact durable bundle reopen, reversible task history, unchanged manual history and retained manual redo; extra in-scope commits may end at the exact oracle","render":"production preview compared with oracle at frames 0,20,49,50,51,52,89,90,99","audio":"production export PCM compared with oracle; non-silent, same sample count, <=1/32768 sample error","acceptance":"all checks pass; observed reads/actions retained; incomplete work or usage is not cheaper success"},
        "accepted_behavior":{"case1":"exact oracle effect and reversible task history; extra in-scope commits are accepted", "case5":"Director reads the durable deceptive child result after publication and before a subsequent merge; incomplete child is never merged; no fixed number of commits or merge replays", "case6":"affected task and attempt alone, excluding fixture calls: timeline/transcript/frame/audio evidence before refused commit, then refresh via task_status, get_timeline_state or refreshed controls; nonempty preview on prepare, then successful commit with that exact plan id and revision, in order"},
        "pre_send_size":{"limit":32768,"refusal":"retain failed run row and all earlier spending/lower bounds; stop that pair for diagnosis, never a zero-token win or automatic retry", "input_limit":262_144,"input_refusal":crate::job_guardian::protocol::INPUT_TOO_LONG,"preflight":"measure and retain the actual input and recipe bytes on the same checkout with the chosen model/options before every paid run", "change":"never raise limit mid-sample; any limit or prompt change requires reviewed pin refresh and matched unpaid arm checks"},
        "analysis":{"unit":"equal weight inclusive input plus output over every task and every attempt, including failures and retries","rows":"retain all 12 runs and all six pairs; no selection or silent replacement","per_provider":"candidate mean AND median strictly below baseline, with unchanged correctness","both_improve":"declared-sample pass only, no general model recommendation","one_improves":"mixed or inconclusive","neither_improves":"fail","correctness_failure":"stop that provider","shared_safety_failure":"stop all providers","baseline_oracle_failure":"stop and diagnose","incomplete_usage":"inconclusive, never known zero or cheap success","interruption":"no automatic retry; fresh approval and grant required for any expansion"}
    }))
}
#[must_use]
pub fn first_difference(expected: &Value, actual: &Value) -> Option<String> {
    difference(expected, actual, "")
}
fn difference(a: &Value, b: &Value, path: &str) -> Option<String> {
    match (a, b) {
        (Value::Object(a), Value::Object(b)) => {
            let keys: std::collections::BTreeSet<_> = a.keys().chain(b.keys()).collect();
            for key in keys {
                let next = format!("{path}/{}", key.replace('~', "~0").replace('/', "~1"));
                match (a.get(key), b.get(key)) {
                    (Some(a), Some(b)) => {
                        if let Some(d) = difference(a, b, &next) {
                            return Some(d);
                        }
                    }
                    _ => return Some(next),
                }
            }
            None
        }
        (Value::Array(a), Value::Array(b)) => {
            if a.len() != b.len() {
                return Some(format!("{path}/length"));
            }
            a.iter()
                .zip(b)
                .enumerate()
                .find_map(|(i, (a, b))| difference(a, b, &format!("{path}/{i}")))
        }
        _ if a != b => Some(path.into()),
        _ => None,
    }
}
pub fn verify(pin: &Value) -> io::Result<()> {
    if let Some(field) = first_difference(pin, &record()?) {
        Err(error(format!("TE1 measurement drift: {field}")))
    } else {
        Ok(())
    }
}
