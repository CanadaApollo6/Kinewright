struct S4Child(std::process::Child, Option<std::path::PathBuf>);

impl Drop for S4Child {
    fn drop(&mut self) {
        if !matches!(self.0.try_wait(), Ok(Some(_))) {
            let _ = self.0.kill();
        }
        let deadline = Instant::now() + Duration::from_secs(30);
        while self
            .0
            .try_wait()
            .unwrap_or_else(|error| {
                eprintln!("S4 cleanup failed: {error}");
                None
            })
            .is_none()
        {
            if Instant::now() >= deadline {
                eprintln!("S4 cleanup failed: child {} was not reaped", self.0.id());
                if let Some(path) = &self.1 {
                    s4_write_record(
                        path,
                        &serde_json::json!({"pid":self.0.id(), "reaped":false, "exit":null}),
                    );
                }
                return;
            }
            thread::sleep(SAMPLE);
        }
        let status = self
            .0
            .try_wait()
            .expect("native reap status")
            .expect("reaped");
        if let Some(path) = &self.1 {
            let receipt =
                serde_json::json!({"pid": self.0.id(), "reaped": true, "exit": status.code()});
            s4_write_record(path, &receipt);
            println!("S4 native cleanup {receipt}");
        }
    }
}

trait S4Output {
    fn s4_output(&mut self) -> std::io::Result<std::process::Output>;
}

impl S4Output for std::process::Command {
    fn s4_output(&mut self) -> std::io::Result<std::process::Output> {
        let temp = TempDirectory::new("s4-child");
        let stdout = temp.path("stdout");
        let stderr = temp.path("stderr");
        let mut child = S4Child(
            self.stdout(std::fs::File::create(&stdout)?)
                .stderr(std::fs::File::create(&stderr)?)
                .spawn()?,
            self.get_envs()
                .find_map(|(key, value)| (key == "PF1_CHILD_RECEIPT").then_some(value).flatten())
                .map(std::path::PathBuf::from),
        );
        let deadline = Instant::now() + Duration::from_secs(600);
        let status = loop {
            if let Some(status) = child.0.try_wait()? {
                break status;
            }
            if Instant::now() >= deadline {
                eprintln!("S4 child stdout: {:?}", std::fs::read_to_string(&stdout));
                eprintln!("S4 child stderr: {:?}", std::fs::read_to_string(&stderr));
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "S4 child exceeded 600 s",
                ));
            }
            thread::sleep(SAMPLE);
        };
        Ok(std::process::Output {
            status,
            stdout: std::fs::read(stdout)?,
            stderr: std::fs::read(stderr)?,
        })
    }
}

fn s4_import_child(
    output: &std::process::Output,
    path: &std::path::Path,
    receipt: &std::path::Path,
    token: &str,
) -> serde_json::Value {
    assert!(
        output.status.success(),
        "failed native child cannot publish accepted evidence"
    );
    let cleanup: serde_json::Value =
        serde_json::from_slice(&std::fs::read(receipt).expect("child receipt")).unwrap();
    assert!(
        cleanup["reaped"] == true && cleanup["exit"] == 0,
        "native completion receipt"
    );
    let mut record: serde_json::Value =
        serde_json::from_slice(&std::fs::read(path).expect("child record")).unwrap();
    assert_eq!(record["publisher_pid"], cleanup["pid"], "native child PID");
    assert_eq!(record["token"], token, "native child token");
    assert!(record["metrics"].is_object(), "native child final metrics");
    record["native_cleanup"] = cleanup;
    record["valid"] = true.into();
    record
}

#[test]
fn s4_import_requires_successful_reaped_child() {
    let temp = TempDirectory::new("s4-import");
    let path = temp.path("record.json");
    let receipt = temp.path("receipt.json");
    for argument in ["--list", "--invalid-s4-child-option"] {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .arg(argument)
            .env("PF1_CHILD_RECEIPT", &receipt)
            .s4_output()
            .unwrap();
        let cleanup: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&receipt).unwrap()).unwrap();
        let raw = serde_json::json!({"token":"fresh", "publisher_pid":cleanup["pid"], "metrics":{"settled_bytes":42}});
        std::fs::write(&path, serde_json::to_vec(&raw).unwrap()).unwrap();
        let imported =
            std::panic::catch_unwind(|| s4_import_child(&output, &path, &receipt, "fresh"));
        assert_eq!(
            imported.is_ok(),
            argument == "--list",
            "reject failed child before reading samples"
        );
        if let Ok(imported) = imported {
            assert_eq!(imported["native_cleanup"], cleanup);
            assert_eq!(imported["metrics"], raw["metrics"]);
            for field in ["publisher_pid", "token", "metrics"] {
                let mut invalid = raw.clone();
                invalid[field] = serde_json::Value::Null;
                std::fs::write(&path, serde_json::to_vec(&invalid).unwrap()).unwrap();
                assert!(
                    std::panic::catch_unwind(|| s4_import_child(&output, &path, &receipt, "fresh"))
                        .is_err()
                );
            }
        }
    }
}
