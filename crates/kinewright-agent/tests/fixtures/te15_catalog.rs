// Included by Agent and App tests so both exercise production catalog validation.
fn te15_catalog_child(name: &str) -> bool {
    const CHILD: &str = "KINEWRIGHT_TE15_CATALOG_CHILD";
    if std::env::var(CHILD).as_deref() == Ok(name) {
        let folder = std::path::PathBuf::from(std::env::var_os("CODEX_HOME").unwrap());
        assert!(folder.starts_with(kinewright_media::test_support::workspace_target_dir()));
        let home = std::path::PathBuf::from(std::env::var_os("HOME").unwrap());
        assert!(home.starts_with(kinewright_media::test_support::workspace_target_dir()));
        assert_eq!(std::fs::read_dir(home).unwrap().count(), 0);
        assert_eq!(std::fs::read_dir(&folder).unwrap().count(), 0);
        std::fs::write(folder.join("models_cache.json"), br#"{"models":[{"slug":"gpt-6.1-sol","supported_reasoning_levels":[{"effort":"high"}],"service_tiers":[{"id":"priority","name":"Fast"}]}]}"#).unwrap();
        return true;
    }
    let temp = kinewright_media::test_support::TempDirectory::new_in(
        &kinewright_media::test_support::workspace_target_dir(),
        "te15-catalog",
    );
    for directory in ["home", "codex"] {
        std::fs::create_dir(temp.path(directory)).unwrap();
    }
    let path = temp.path("child.log");
    let output = std::fs::File::create(&path).unwrap();
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", name, "--nocapture"])
        .env(CHILD, name)
        .env("HOME", temp.path("home"))
        .env("CODEX_HOME", temp.path("codex"))
        .stdout(output.try_clone().unwrap())
        .stderr(output)
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(45);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break Some(status);
        }
        if std::time::Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            break None;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    };
    let output = std::fs::read_to_string(path).unwrap();
    assert!(
        status.is_some_and(|s| s.success())
            && output
                .lines()
                .any(|line| line.starts_with("test result: ok. 1 passed; 0 failed; 0 ignored;")),
        "TE15_FIX2_SYNTHETIC_CATALOG {name}: {output}"
    );
    false
}
