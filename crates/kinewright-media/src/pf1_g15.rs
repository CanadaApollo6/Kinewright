/// P-rss: a fresh process per measurement (the child below).
#[test]
#[ignore = "PF1 P-rss lane (LL pinned): cargo test --release -p kinewright-media --lib pf1_rss_baseline -- --ignored --nocapture --test-threads=1"]
fn pf1_rss_baseline() {
    assert_release();
    let lane = lane().1;
    let mut all = play_workloads();
    if wanted("title_only") {
        all.push((
            "title_only",
            s4_input("title_only").unwrap_or_else(perf_fixtures::title_only),
        ));
    }
    all.retain(|(key, _)| wanted(key));
    let collecting = std::env::var_os("PF1_RECORD").is_some();
    let repeats = if collecting { 1 } else { 3 };
    if collecting {
        assert_eq!(all.len(), 1, "one frozen G15 workload per wrapper row");
    }
    let mut children = 0;
    for (key, Workload(document, _media)) in
        all.iter().flat_map(|w| std::iter::repeat_n(w, repeats))
    {
        let temp = TempDirectory::new("pf1-rss");
        let path = temp.path("document.json");
        let record = std::env::var_os("PF1_RECORD")
            .map_or_else(|| temp.path("record.json"), std::path::PathBuf::from);
        let receipt = temp.path("receipt.json");
        let token = std::env::var("PF1_TOKEN").unwrap_or_else(|_| "g15-development".into());
        std::fs::write(&path, serde_json::to_vec(document).expect("serialises")).expect("writes");
        let child = std::process::Command::new(std::env::current_exe().expect("test binary"))
            .args([
                "--exact",
                "pf1_harness::pf1_rss_child",
                "--ignored",
                "--nocapture",
            ])
            .args(["--test-threads=1"])
            .env("PF1_RSS_DOCUMENT", &path)
            .env("PF1_RECORD", &record)
            .env("PF1_TOKEN", &token)
            .env("PF1_CHILD_RECEIPT", &receipt)
            .s4_output()
            .expect("the P-rss child runs");
        let stdout = String::from_utf8_lossy(&child.stdout);
        let stderr = String::from_utf8_lossy(&child.stderr);
        println!("S4 native stdout: {stdout}\nS4 native stderr: {stderr}");
        assert!(
            child.status.success(),
            "the P-rss child failed ({}): {stdout}{stderr}",
            child.status
        );
        // libtest prints `test … ... ` on the same line before the output.
        let line = (stdout.lines()).find_map(|line| Some(&line[line.find("PF1 rss ")?..]));
        let line = line.unwrap_or_else(|| panic!("no P-rss result: {stdout}{stderr}"));
        let mut published = s4_import_child(&child, &record, &receipt, &token);
        assert!(
            published["metrics"]["settled_bytes"].is_u64(),
            "G15 settled value"
        );
        children += 1;
        published["native_children"] = children.into();
        s4_write_record(&record, &published);
        println!("{line} lane={lane} workload={key}");
        println!("PF1 G15 workload={key} record={published}");
    }
    assert_eq!(children, all.len() * repeats, "exact fresh-child count");
}

/// One P-rss measurement: before the engine, constructed (no document), first
/// render, settled idle after 6 s, and 10 s of playback (rss MiB/threads),
/// which must have advanced at least 5 s without an engine error; then the
/// engine's teardown record (`teardown`), before the process exits.
#[test]
#[ignore = "PF1 P-rss child: spawned by pf1_rss_baseline"]
fn pf1_rss_child() {
    let Some(path) = std::env::var_os("PF1_RSS_DOCUMENT") else {
        return;
    };
    assert_release();
    let document: Document = s4_input(&std::env::var("PF1_ONLY").unwrap_or_default()).map_or_else(
        || serde_json::from_slice(&std::fs::read(path).expect("reads")).expect("a document"),
        |w| w.0,
    );
    if let Ok(frozen) = std::env::var("PF1_D13_CALIBRATION") {
        let calibration: serde_json::Value =
            serde_json::from_str(&frozen).expect("D13 calibration");
        assert_eq!(
            crate::sha256::sha256_bytes(frozen.as_bytes()),
            std::env::var("PF1_D13_CALIBRATION_SHA256").expect("calibration digest")
        );
        assert_eq!(calibration["ffmpeg"], ffmpeg_next::util::version());
        assert_eq!(calibration["B"], 16);
        assert_eq!(calibration["reader_cap"], 2);
        assert_eq!(
            calibration["media_sha256"],
            crate::sha256::sha256_file(&document.media_pool[0].path).expect("fixture hash")
        );
        d13_run(
            &document,
            calibration["d_bytes"].as_u64().expect("d") as usize,
            true,
        );
        return;
    }
    let memory = || process_memory().expect("process memory");
    let show = |m: ProcessMemory| format!("{:.1}/{}", mib(m.rss), m.threads);
    let before = memory();
    let session = Session::new(Some(SimulatedAudio::paced()), Arc::default(), lane().0);
    let protocol = serde_json::json!({"name": "P-rss", "poll_ms": 5, "children": 3});
    s4_publish_metrics(&session.gpu, protocol.clone(), None);
    thread::sleep(Duration::from_millis(500));
    let constructed = memory();
    session.load(&document);
    let first = memory();
    thread::sleep(Duration::from_secs(6));
    let settled = memory();
    session.assert_no_error("G15 settled idle");
    s4_publish_metrics(
        &session.gpu,
        protocol,
        Some((true, serde_json::json!({"settled_bytes": settled.rss}))),
    );
    let peak_reset = reset_peak();
    session.engine.play(TimeCode::ZERO);
    thread::sleep(Duration::from_secs(10));
    let playing = memory();
    let played = session.engine.position().0 as f64 * frame_ms(&document);
    session.assert_no_error("P-rss playback");
    assert!(
        played >= 5_000.0,
        "P-rss playback advanced only {played} ms"
    );
    session.engine.pause();
    // E12.10 D3(a): wait for the engine's teardown record, as P-play does,
    // so the process never exits while the worker is still destroying the
    // GPU device.
    let teardown = s4_rss_finish(session);
    println!(
        "PF1 rss before={} constructed={} first_render={} settled_idle={} playing={} \
         playing_peak_mib={:.1}{} played_s={:.1} {teardown}",
        show(before),
        show(constructed),
        show(first),
        show(settled),
        show(playing),
        mib(playing.peak),
        if peak_reset { "" } else { "(lifetime)" },
        played / 1e3,
    );
}
