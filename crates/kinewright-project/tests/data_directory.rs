use kinewright_core::MediaError;
use kinewright_media::test_support::TempDirectory;
use std::{fs, path::PathBuf};

#[test]
fn persistent_recovery_root_matches_data_root_without_temp_fallback() {
    const CHILD: &str = "KINEWRIGHT_RECOVERY_ROOT_CHILD";
    if let Some(mode) = std::env::var_os(CHILD) {
        let temporary = std::env::temp_dir();
        if mode == "missing" {
            assert!(
                matches!(
                    kinewright_project::default_recovery_directory(),
                    Err(MediaError::DataDirectory(_))
                ),
                "RECOVERY_ROOT_NO_TEMP_FALLBACK"
            );
        } else {
            let data = kinewright_media::default_data_dir().unwrap();
            let recovery = kinewright_project::default_recovery_directory().unwrap();
            assert_eq!(recovery, data.join("recovery"));
            let base = PathBuf::from(std::env::var_os("KINEWRIGHT_ROOT_INPUT").unwrap());
            assert_eq!(
                fs::canonicalize(data.parent().unwrap()).unwrap(),
                fs::canonicalize(base).unwrap()
            );
            assert_eq!(
                data.file_name().unwrap(),
                if cfg!(target_os = "linux") {
                    "kinewright"
                } else {
                    "Kinewright"
                }
            );
            fs::write(recovery.join("proof"), b"persistent").unwrap();
        }
        assert_eq!(
            fs::read_dir(temporary).unwrap().count(),
            0,
            "RECOVERY_ROOT_CREATED_NOTHING_IN_TEMP"
        );
        return;
    }
    let fixture = TempDirectory::new("persistent-recovery-root");
    let temporary = fixture.path("temporary");
    fs::create_dir(&temporary).unwrap();
    for mode in ["present", "missing"] {
        let mut child = std::process::Command::new(std::env::current_exe().unwrap());
        child
            .args([
                "--exact",
                "persistent_recovery_root_matches_data_root_without_temp_fallback",
                "--test-threads=1",
            ])
            .env(CHILD, mode)
            .env("KINEWRIGHT_ROOT_INPUT", fixture.path("data"))
            .env("TMPDIR", &temporary)
            .env("TEMP", &temporary)
            .env("TMP", &temporary)
            .env_remove("HOME")
            .env_remove("XDG_DATA_HOME")
            .env_remove("LOCALAPPDATA");
        if mode == "present" {
            child
                .env("XDG_DATA_HOME", fixture.path("data"))
                .env("LOCALAPPDATA", fixture.path("data"));
        }
        let output = child.output().unwrap();
        assert!(
            output.status.success()
                && String::from_utf8_lossy(&output.stdout).contains("test result: ok. 1 passed;"),
            "RECOVERY_ROOT_CHILD {mode}: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(fs::read_dir(&temporary).unwrap().count(), 0);
    }
}
