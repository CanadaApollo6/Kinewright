//! Mode routing only; successor actions call production owners as they land.
#![allow(clippy::missing_errors_doc)]
use super::{
    baseline::Baseline,
    protocol::{Call, LoadedFixture, Mode},
};
use kinewright_project::{io_observer, project};
use serde_json::{Value, json};
use std::{fs, path::Path};

pub struct Adapter {
    pub baseline: Baseline,
    mode: Mode,
}

impl Adapter {
    pub fn new(root: &Path, mode: Mode) -> Result<Self, String> {
        Ok(Self {
            baseline: Baseline::new(root)?,
            mode,
        })
    }

    pub fn finish(&mut self) -> Result<Value, String> {
        self.baseline.finish()
    }

    pub fn execute(&mut self, call: &Call, fixture: &LoadedFixture) -> Result<Value, String> {
        match call {
            Call::AdapterStatus(_) => {
                Ok(json!({"mode":self.mode,"backend":"portable-model","pid":std::process::id()}))
            }
            Call::StrictSave(args) | Call::StrictRenameRefusal(args) => {
                let bytes = fixture
                    .files
                    .get(&args.input)
                    .ok_or("missing IO probe input")?;
                let path = self.baseline.root.join(&args.path);
                let before = fs::read(&path).ok();
                let refused = matches!(call, Call::StrictRenameRefusal(_));
                let (result, trace) = io_observer::observe(|| {
                    if refused {
                        project::observe_refused_rename(&path, bytes)
                    } else {
                        project::write_file_atomic(&path, bytes)
                    }
                });
                let after = fs::read(&path).map_err(|e| e.to_string())?;
                let attempted_sha = kinewright_media::sha256_bytes(bytes);
                let hook = trace
                    .events
                    .iter()
                    .find(|e| e.operation == "rename_refusal_hook");
                Ok(
                    json!({"succeeded":result.is_ok(),"writer_bytes_equal":after == *bytes,
                    "root_preserved":before.as_ref() == Some(&after),
                    "refusal_reached":hook.is_some(),
                    "refusal_injected":trace.events.iter().any(|e| e.operation == "rename_refusal_injected"),
                    "attempted_bytes_equal":hook.is_some_and(|e| e.attempted_sha256.as_ref() == Some(&attempted_sha) && e.attempted_length == Some(bytes.len())),
                    "directory_sync_observed":trace.events.iter().any(|e| e.operation == "directory_sync"),
                    "in_place_fallback_observed":trace.events.iter().any(|e| e.operation == "in_place_fallback"),"io":trace}),
                )
            }
            _ => self.baseline.execute(call, fixture),
        }
    }
}
