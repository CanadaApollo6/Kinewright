//! Separate process so the test observes library output without libtest output.
use kinewright_media::nemo::{Error, Recognizer, Status};
use std::{path::PathBuf, process::ExitCode};

fn main() -> ExitCode {
    let mut arguments = std::env::args_os().skip(1);
    let first = arguments.next().expect("model path or installer command");
    if first == "model-key" {
        println!(
            "{}",
            kinewright_media::recognizer::ENGLISH_MODEL.files[0].sha256
        );
        return ExitCode::SUCCESS;
    }
    if first == "provision" {
        let data = PathBuf::from(arguments.next().expect("isolated model data directory"));
        return match kinewright_media::recognizer::provision(
            &kinewright_media::recognizer::ENGLISH_MODEL,
            &data,
            &kinewright_core::ExportCancellation::default(),
            |_, _| {},
        ) {
            Ok(root) => {
                println!("{}", root.display());
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("{error}");
                ExitCode::FAILURE
            }
        };
    }
    let path = PathBuf::from(first);
    let expected = arguments.next().expect("error kind");
    let actual = Recognizer::load(&path).err();
    let matches = match expected.to_str() {
        Some("missing") => matches!(actual, Some(Error::MissingModel)),
        Some("empty") => matches!(actual, Some(Error::EmptyModel)),
        Some("garbage") => {
            matches!(actual, Some(Error::InvalidModel(error)) if error.status == Status::Runtime)
        }
        _ => false,
    };
    if matches {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
