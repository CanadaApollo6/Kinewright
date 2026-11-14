//! Local file scoring only: no session, model, provider, network or audio decode.
use kinewright_agent::cut_score::{
    Candidate, CannotScore, Case, Score, ScoreSettings, candidate_from_document, score,
};
use kinewright_core::{AssetId, TrackId};
use serde::{Serialize, de::DeserializeOwned};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    path::{Path, PathBuf},
    process::ExitCode,
};

const USAGE: &str = "kinewright-cut-score --case CASE.json (--ranges CANDIDATE.json | --project EDIT.kinewright --asset-id N --track-id N)";

#[derive(Debug, Serialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
enum Failure {
    Arguments { message: String },
    Read { path: PathBuf, cause: String },
    Json { path: PathBuf, cause: String },
    Project { path: PathBuf, cause: String },
    CannotScore { refusal: CannotScore },
}

#[derive(Serialize)]
struct Success {
    status: &'static str,
    score: Score,
}
#[derive(Serialize)]
struct Refused {
    status: &'static str,
    error: Failure,
}

fn arguments(message: impl Into<String>) -> Failure {
    Failure::Arguments {
        message: message.into(),
    }
}

fn read<T: DeserializeOwned>(path: &Path) -> Result<T, Failure> {
    let bytes = std::fs::read(path).map_err(|e| Failure::Read {
        path: path.to_owned(),
        cause: e.to_string(),
    })?;
    serde_json::from_slice(&bytes).map_err(|e| Failure::Json {
        path: path.to_owned(),
        cause: e.to_string(),
    })
}

fn run(args: impl Iterator<Item = OsString>) -> Result<Score, Failure> {
    let mut args = args;
    let mut options = BTreeMap::new();
    while let Some(flag) = args.next() {
        let flag = flag.into_string().map_err(|_| arguments(USAGE))?;
        if ![
            "--case",
            "--ranges",
            "--project",
            "--asset-id",
            "--track-id",
        ]
        .contains(&flag.as_str())
        {
            return Err(arguments(format!("unknown option {flag}; {USAGE}")));
        }
        let value = args.next().ok_or_else(|| arguments(USAGE))?;
        if options.insert(flag, value).is_some() {
            return Err(arguments("duplicate option"));
        }
    }
    let case_path = PathBuf::from(options.remove("--case").ok_or_else(|| arguments(USAGE))?);
    let case: Case = read(&case_path)?;
    let candidate: Candidate = if let Some(path) = options.remove("--ranges") {
        if !options.is_empty() {
            return Err(arguments(USAGE));
        }
        read(Path::new(&path))?
    } else {
        let path = PathBuf::from(
            options
                .remove("--project")
                .ok_or_else(|| arguments(USAGE))?,
        );
        let mut id = |name| -> Result<u64, Failure> {
            options
                .remove(name)
                .and_then(|v| v.into_string().ok())
                .and_then(|v| v.parse().ok())
                .ok_or_else(|| arguments(format!("{name} must be an unsigned integer; {USAGE}")))
        };
        let asset = AssetId(id("--asset-id")?);
        let track = TrackId(id("--track-id")?);
        if !options.is_empty() {
            return Err(arguments(USAGE));
        }
        let admission = match kinewright_project::bundle::BundleWriteAdmission::read(&path) {
            Ok(admission) => admission,
            Err(error) => {
                return Err(Failure::Project {
                    path,
                    cause: error.to_string(),
                });
            }
        };
        if let Err(error) = admission.require_supported() {
            return Err(Failure::Project {
                path,
                cause: error.to_string(),
            });
        }
        let document = admission
            .inspection_document()
            .ok_or_else(|| Failure::Project {
                path,
                cause: "supported admission has no inspection document".into(),
            })?;
        candidate_from_document(document, asset, track)
            .map_err(|refusal| Failure::CannotScore { refusal })?
    };
    score(&case, &candidate, &ScoreSettings::default())
        .map_err(|refusal| Failure::CannotScore { refusal })
}

fn print_json(value: &impl Serialize) -> bool {
    match serde_json::to_string(value) {
        Ok(text) => {
            println!("{text}");
            true
        }
        Err(error) => {
            eprintln!("cannot serialize score: {error}");
            false
        }
    }
}

fn main() -> ExitCode {
    if std::env::args_os().nth(1).is_some_and(|s| s == "--help") {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    match run(std::env::args_os().skip(1)) {
        Ok(score) => {
            if print_json(&Success {
                status: "scored",
                score,
            }) {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Err(error) => {
            let code = if matches!(&error, Failure::CannotScore { .. }) {
                2
            } else {
                1
            };
            print_json(&Refused {
                status: "cannot_score",
                error,
            });
            ExitCode::from(code)
        }
    }
}
