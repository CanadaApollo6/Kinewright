use super::*;

const CHILD: &str = "KINEWRIGHT_MEDIA_EXIT_CHILD";

#[test]
fn process_exit_waits_for_native_preview_teardown() {
    if let Some(directory) = std::env::var_os(CHILD) {
        held_native_teardown(std::path::Path::new(&directory));
        return;
    }
    run_child(false);
}

#[test]
#[ignore = "on-demand: physical GPU process-exit ordering"]
fn process_exit_native_driver() {
    run_child(true);
}

fn marked(directory: &std::path::Path, name: &str) -> bool {
    fs::read(directory.join(name)).is_ok_and(|bytes| bytes == b"1")
}

fn returned(directory: &std::path::Path) -> Option<bool> {
    match fs::read(directory.join("returned")).as_deref() {
        Ok(b"1") => Some(true),
        Ok(b"0") => Some(false),
        _ => None,
    }
}

fn run_child(native: bool) {
    let gate = crate::test_support::TempDirectory::new("exit-gate");
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "engine::process_exit_tests::process_exit_waits_for_native_preview_teardown",
            "--nocapture",
        ])
        .env(CHILD, gate.root())
        .env(
            "KINEWRIGHT_MEDIA_EXIT_NATIVE",
            if native { "1" } else { "0" },
        )
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    println!("EXIT_CHILD_PID {} native={native}", child.id());
    let deadline = Instant::now() + Duration::from_secs(15);
    while !marked(gate.root(), "shutdown") && returned(gate.root()).is_none() {
        if child.try_wait().unwrap().is_some() || Instant::now() >= deadline {
            break;
        }
        thread::park_timeout(Duration::from_millis(1));
    }
    // A joined owner cannot return yet. Give a detached owner the opportunity
    // to return from its passing test and enter libc exit with native state held.
    let observe = Instant::now() + Duration::from_secs(1);
    while returned(gate.root()).is_none() && Instant::now() < observe {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        thread::park_timeout(Duration::from_millis(1));
    }
    if returned(gate.root()) == Some(false) {
        let exit = Instant::now() + Duration::from_secs(10);
        while child.try_wait().unwrap().is_none() && Instant::now() < exit {
            thread::park_timeout(Duration::from_millis(1));
        }
    }
    // Also release on failure, so a missed startup marker cannot strand a child.
    fs::write(gate.root().join("release"), b"1").unwrap();
    let output = child.wait_with_output().unwrap();
    print!("{}", String::from_utf8_lossy(&output.stdout));
    assert!(
        output.status.success()
            && returned(gate.root()) == Some(true)
            && marked(gate.root(), "native-done"),
        "EXIT_NATIVE_TEARDOWN_BEFORE_OWNER_RETURN status={}\n{}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn held_native_teardown(directory: &std::path::Path) {
    let gpu = if std::env::var("KINEWRIGHT_MEDIA_EXIT_NATIVE").as_deref() == Ok("1") {
        GpuContext::headless(false).expect("physical GPU witness")
    } else {
        crate::gpu_test_support::fixture_gpu_or_skip().expect("equipped GPU witness")
    };
    println!("EXIT_DRIVER {:?}", gpu.monitor_proof_metadata());
    let data = crate::test_support::TempDirectory::new("exit-owner");
    let faults = Arc::new(Faults::default());
    let (entered, entry) = bounded(1);
    let release = directory.join("release");
    *faults.on_preview_drop.lock().unwrap() = Some(Box::new(move || {
        entered.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        while !fs::read(&release).is_ok_and(|bytes| bytes == b"1") {
            assert!(Instant::now() < deadline, "EXIT_NATIVE_RELEASE");
            thread::park_timeout(Duration::from_millis(1));
        }
    }));
    let complete = directory.join("native-done");
    *faults.on_preview_exit.lock().unwrap() = Some(Box::new(move || {
        fs::write(complete, b"1").unwrap();
    }));
    let shutting_down = directory.join("shutdown");
    *faults.on_worker_shutdown.lock().unwrap() = Some(Box::new(move || {
        fs::write(shutting_down, b"1").unwrap();
    }));
    let engine = FfmpegMediaEngine::new_for_harness(
        gpu,
        data.root().to_path_buf(),
        Some(crate::audio::simulated::SimulatedAudio::stepped()),
        faults,
        Arc::default(),
    )
    .unwrap()
    .into_legacy_fixture();
    engine.set_document(Arc::new(crate::perf_fixtures::title_card((64, 64), 90)));
    engine.request_frame(TimeCode::ZERO);
    engine
        .frames()
        .recv_timeout(Duration::from_secs(10))
        .unwrap();
    drop(engine.engine.s4_lane().shut_down());
    entry.recv_timeout(Duration::from_secs(10)).unwrap();
    drop(engine);
    fs::write(
        directory.join("returned"),
        if marked(directory, "native-done") {
            b"1"
        } else {
            b"0"
        },
    )
    .unwrap();
    // No child controller thread or explicit engine-completion wait: an unfixed
    // child now prints its passing summary and exits with Preview still held.
}
