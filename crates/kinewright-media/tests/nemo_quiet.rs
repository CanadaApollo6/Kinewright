use kinewright_media::test_support::TempDirectory;
use std::{fs, process::Command};

#[test]
fn dq1_failed_model_load_has_empty_process_output() {
    let directory = TempDirectory::new("dq1-quiet-load");
    fs::write(directory.path("empty"), []).unwrap();
    fs::write(directory.path("garbage"), b"not a GGUF model").unwrap();
    for kind in ["missing", "empty", "garbage"] {
        let output = Command::new(env!("CARGO_BIN_EXE_nemo-load-probe"))
            .arg(directory.path(kind))
            .arg(kind)
            .output()
            .unwrap();
        assert!(output.status.success(), "{kind}: {output:?}");
        assert!(
            output.stdout.is_empty(),
            "{kind} stdout: {:?}",
            output.stdout
        );
        assert!(
            output.stderr.is_empty(),
            "{kind} stderr: {:?}",
            output.stderr
        );
    }
}
