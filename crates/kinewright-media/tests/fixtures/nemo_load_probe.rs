//! Separate process so the test observes library output without libtest output.
use kinewright_media::nemo::{Error, Recognizer, Status};
use std::{path::PathBuf, process::ExitCode};

fn main() -> ExitCode {
    let mut arguments = std::env::args_os().skip(1);
    let path = PathBuf::from(arguments.next().expect("model path"));
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
