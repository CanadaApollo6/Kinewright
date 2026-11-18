use kinewright_media::test_support::TempDirectory;
use kinewright_project::RecoveryDirectoryRefusal;
use std::{fs, path::PathBuf};

#[test]
fn persistent_recovery_root_uses_state_without_temp_fallback() {
    const CHILD: &str = "KINEWRIGHT_RECOVERY_ROOT_CHILD";
    if let Some(mode) = std::env::var_os(CHILD) {
        let temporary = std::env::temp_dir();
        if mode == "missing" {
            assert!(
                matches!(
                    kinewright_project::default_recovery_directory(),
                    Err(RecoveryDirectoryRefusal::NoStateFolder)
                ),
                "RECOVERY_ROOT_NO_TEMP_FALLBACK"
            );
        } else {
            let recovery = kinewright_project::default_recovery_directory().unwrap();
            let base = PathBuf::from(std::env::var_os("KINEWRIGHT_ROOT_INPUT").unwrap());
            assert_eq!(
                recovery,
                base.join(if cfg!(target_os = "linux") {
                    "kinewright"
                } else {
                    "Kinewright"
                })
                .join("recovery")
            );
            assert!(!recovery.exists(), "resolving state creates nothing");
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
                "persistent_recovery_root_uses_state_without_temp_fallback",
                "--test-threads=1",
            ])
            .env(CHILD, mode)
            .env("KINEWRIGHT_ROOT_INPUT", fixture.path("state"))
            .env("TMPDIR", &temporary)
            .env("TEMP", &temporary)
            .env("TMP", &temporary)
            .env_remove("HOME")
            .env_remove("XDG_DATA_HOME")
            .env_remove("XDG_STATE_HOME")
            .env_remove("LOCALAPPDATA");
        if mode == "present" {
            child.env("XDG_DATA_HOME", fixture.path("data"));
            if cfg!(target_os = "linux") {
                child.env("XDG_STATE_HOME", fixture.path("state"));
            } else {
                child.env("LOCALAPPDATA", fixture.path("state"));
            }
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
