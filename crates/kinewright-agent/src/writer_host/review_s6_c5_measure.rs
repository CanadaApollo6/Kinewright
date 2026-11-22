// JA-1 S6 C5 (G20): the scripted host-level measurement workflow. Built only
// with the `g20-measure` feature and run by `scripts/measure-g20.py`, which
// samples the process tree. It changes no product path: the host's own
// entry points, the production review driver and a scripted Reviewer over its
// own task MCP, with no model run. Each workflow starts on a fresh fixture in
// its own temporary folder under `target/`; nothing reads the person's
// application data, and the seal notification goes to a private bus.
//
// Every phase is timed host side, from the request leaving this process to
// the host's answer or the bundle state that ends the phase.

/// The delivery raster of one encode.
#[derive(Clone, Copy, Debug)]
enum G20Raster {
    /// `QueueDefault`: the default 1080p delivery at the −14 LUFS target.
    DefaultDelivery,
    /// The default settings at the document's own raster.
    SourceRaster,
}

impl G20Raster {
    fn label(self) -> &'static str {
        match self {
            Self::DefaultDelivery => "default_delivery",
            Self::SourceRaster => "source_raster",
        }
    }
}

/// Durable comments saved per workflow.
const G20_COMMENTS: usize = 20;

fn g20_ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

/// Wait for `ready`, polling every millisecond; return when it held.
fn g20_until(mut ready: impl FnMut() -> bool, marker: &str) -> Instant {
    let started = Instant::now();
    while !ready() {
        assert!(started.elapsed() < S5_REVIEW_HANG, "{marker}");
        std::thread::sleep(Duration::from_millis(1));
    }
    Instant::now()
}

/// Queue `key` at `raster`; returns the queued token and the reply time.
fn g20_queue(
    f: &Fixture,
    key: &kinewright_project::studio_records::SnapshotKey,
    raster: G20Raster,
) -> ReviewToken {
    let control = match raster {
        G20Raster::DefaultDelivery => ReviewControl::QueueDefault(key.clone()),
        G20Raster::SourceRaster => {
            let document = c7f2_owner(f)
                .lock()
                .unwrap()
                .owner()
                .unwrap()
                .read_snapshot(key)
                .unwrap()
                .open_review_inputs()
                .unwrap()
                .document;
            let mut settings = kinewright_core::output_review_default_settings(
                &document,
                kinewright_core::ExportCancellation::default(),
            );
            settings.resolution = document.resolution;
            ReviewControl::Queue {
                snapshot: key.clone(),
                settings,
                byte_limit: 64 << 20,
            }
        }
    };
    let queued = s5_review_call(&f.editor, control).unwrap();
    assert_eq!(queued.phase, ReviewPhase::Queued, "G20_QUEUED");
    queued.token
}

/// One durable comment on `key`: admit its text, take an issuer, save it.
/// Returns (whole round trip, the `SaveFeedback` request alone).
fn g20_comment(
    f: &Fixture,
    key: &kinewright_project::studio_records::SnapshotKey,
    strokes: &kinewright_project::bundle_records::ObjectRef,
    index: usize,
) -> (Duration, Duration) {
    use kinewright_project::studio_records::FeedbackEvidenceKind;
    let started = Instant::now();
    let text = ok(
        f.editor
            .request(HostCommand::AdmitFeedbackEvidence {
                kind: FeedbackEvidenceKind::Text,
                bytes: format!("G20 note {index}: tighten this cut.").into_bytes(),
            })
            .unwrap(),
        "G20_COMMENT_TEXT",
    )
    .feedback_evidence
    .unwrap();
    let revision =
        crate::writer_host::ja_s3_host::feedback_revision(key.clone(), &[text.clone(), strokes.clone(), text]);
    let command = metadata_request(f, &f.editor, MetadataChange::SaveFeedback(revision));
    let saving = Instant::now();
    let reply = ok(f.editor.request(command).unwrap(), "G20_COMMENT_SAVED");
    let done = Instant::now();
    assert!(
        matches!(
            reply.metadata.map(|receipt| receipt.result),
            Some(ReceiptResult::Metadata {
                result: MetadataResult::Feedback(_)
            })
        ),
        "G20_COMMENT_IS_FEEDBACK"
    );
    (done - started, done - saving)
}

fn g20_folder_bytes(folder: &std::path::Path) -> u64 {
    let mut total = 0;
    for entry in std::fs::read_dir(folder).unwrap() {
        let entry = entry.unwrap();
        let meta = entry.metadata().unwrap();
        total += if meta.is_dir() {
            g20_folder_bytes(&entry.path())
        } else {
            meta.len()
        };
    }
    total
}

/// One complete workflow: queue, encode, check, seal and notify, save,
/// comments, detach, exit, reopen.
fn g20_workflow(run: usize, raster: G20Raster) -> serde_json::Value {
    use std::sync::Mutex as StdMutex;
    let fixture_started = Instant::now();
    let (mut f, _) = c7_talking_head_fixture(C7Transcript::Kept);
    let fixture_ms = g20_ms(fixture_started.elapsed());
    let (_bus, daemon, _server) = s6c4_notify_on_private_bus(&f, true);
    let (run_id, parent, director) = te13_director(&f);
    let (_, _, reviewer) = s5_child(&f, &director, &parent, Role::Reviewer);
    holder(&mut f);
    let key = s5_c6_publish_version(&f, None, vec![]);
    s5_accepted(&reviewer, "record_editorial_audit", s6c3_audit(&key));
    c7_auto_drive(&f, true);
    let sealing: Arc<StdMutex<Option<Instant>>> = Arc::default();
    {
        let sealing = Arc::clone(&sealing);
        c7f2_owner(&f).lock().unwrap().review_runtime.seal_hook = Some(Arc::new(
            move |_: &kinewright_core::ExportCancellation| {
                sealing.lock().unwrap().get_or_insert_with(Instant::now);
                Ok(())
            },
        ));
    }

    // Queue.
    let queue_started = Instant::now();
    let queued = g20_queue(&f, &key, raster);
    let queue = queue_started.elapsed();

    // Encode: Resume until Checking with the encode worker finished.
    let encode_started = Instant::now();
    let token = s5_review_call(&f.editor, ReviewControl::Resume(queued))
        .unwrap()
        .token;
    let owner = c7f2_owner(&f);
    let encoded = g20_until(
        || {
            matches!(c7f_phase(&f), Some(ReviewPhase::Checking(_)))
                && owner
                    .lock()
                    .unwrap()
                    .review_fixture_worker()
                    .is_none_or(|(_, done)| done)
        },
        "G20_ENCODED",
    );
    let encode = encoded - encode_started;

    // Check: the production checks and the scripted Reviewer's judgment, up
    // to the seal's start.
    c7_judge(&reviewer);
    let seal_started = g20_until(|| sealing.lock().unwrap().is_some(), "G20_CHECKED");
    let seal_started = sealing.lock().unwrap().unwrap_or(seal_started);
    let check = seal_started - encoded;

    // Seal, then its notification on the private bus.
    let sealed = g20_until(|| !c7f_receipts(&f, &token).is_empty(), "G20_SEALED");
    let seal = sealed - seal_started;
    let notified = g20_until(|| s6c4_calls(&daemon).len() == 1, "G20_NOTIFIED");
    let notify = notified - sealed;
    let receipt = c7f_receipts(&f, &token).pop().unwrap();

    // Save to a temporary folder.
    let out = f.temp.path("out");
    std::fs::create_dir(&out).unwrap();
    let save_started = Instant::now();
    ok(
        f.editor.request(s6_save_command(&receipt, &out)).unwrap(),
        "G20_SAVE_STARTED",
    );
    let save_request = save_started.elapsed();
    let saved_status = s6_finished_polling(&f);
    let save = save_started.elapsed();
    assert!(
        matches!(
            saved_status,
            kinewright_project::review_job::delivery::SealedSaveStatus::Saved { .. }
        ),
        "G20_SAVED: {saved_status:?}"
    );
    let saved_bytes = g20_folder_bytes(&out);

    // Durable comments on the sealed version, then the version list.
    let strokes = ok(
        f.editor
            .request(HostCommand::AdmitFeedbackEvidence {
                kind: kinewright_project::studio_records::FeedbackEvidenceKind::Strokes,
                bytes: b"[]".to_vec(),
            })
            .unwrap(),
        "G20_STROKES",
    )
    .feedback_evidence
    .unwrap();
    let mut comments = Vec::with_capacity(G20_COMMENTS);
    let mut comment_saves = Vec::with_capacity(G20_COMMENTS);
    for index in 0..G20_COMMENTS {
        let (whole, save) = g20_comment(&f, &key, &strokes, index);
        comments.push(g20_ms(whole));
        comment_saves.push(g20_ms(save));
    }
    let list_started = Instant::now();
    let listed = ok(
        f.editor.request(HostCommand::ListVersions).unwrap(),
        "G20_LIST_VERSIONS",
    );
    let list_versions = list_started.elapsed();
    assert_eq!(listed.sealed_versions.len(), 1, "G20_LISTED_SEALED");

    // Detach, end the run as its processes would, and wait for the exit.
    let job = S6c3Job {
        f,
        token,
        run: run_id,
        director,
        reviewer,
    };
    let (detach, exit) = s6c3_beside_wait(&job.f, |waiting| {
        let detach_started = Instant::now();
        let detached = s6c3_detach(&job.f, "G20_DETACH");
        let detach = detach_started.elapsed();
        assert_eq!(detached.retained_runs, vec![job.run.clone()]);
        s6c3_end_run(&job, "G20_END_RUN");
        let exit_started = Instant::now();
        let exited = g20_until(|| waiting.is_finished(), "G20_EXIT");
        (detach, exited - exit_started)
    });
    drop(owner);
    let S6c3Job {
        f,
        token,
        reviewer,
        director,
        ..
    } = job;
    drop((reviewer, director));

    // Reopen in a new host on the same bundle: the receipt is there.
    let reopen_started = Instant::now();
    let f = s6c3_reopen(f);
    let reopen = reopen_started.elapsed();
    assert_eq!(c7f_receipts(&f, &token).len(), 1, "G20_REOPEN_RECEIPT");
    let relist_started = Instant::now();
    let relisted = ok(
        f.editor.request(HostCommand::ListVersions).unwrap(),
        "G20_RELIST",
    );
    let list_versions_cold = relist_started.elapsed();
    assert_eq!(relisted.sealed_versions.len(), 1, "G20_RELISTED_SEALED");
    drop(f);

    serde_json::json!({
        "run": run,
        "raster": raster.label(),
        "outcome": format!("{:?}", receipt.outcome),
        "phases_ms": {
            "fixture": fixture_ms,
            "queue": g20_ms(queue),
            "encode": g20_ms(encode),
            "check": g20_ms(check),
            "seal": g20_ms(seal),
            "notify": g20_ms(notify),
            "save_request": g20_ms(save_request),
            "save": g20_ms(save),
            "list_versions_warm": g20_ms(list_versions),
            "detach": g20_ms(detach),
            "exit": g20_ms(exit),
            "reopen": g20_ms(reopen),
            "list_versions_after_reopen": g20_ms(list_versions_cold),
        },
        "saved_bytes": saved_bytes,
        "comment_round_trip_ms": comments,
        "comment_save_request_ms": comment_saves,
    })
}

/// Poll Status every 2 ms until the save leaves `Saving`.
fn s6_finished_polling(f: &Fixture) -> kinewright_project::review_job::delivery::SealedSaveStatus {
    use kinewright_project::review_job::delivery::SealedSaveStatus;
    let started = Instant::now();
    loop {
        match s6_save_status(f) {
            Some(SealedSaveStatus::Saving { .. }) => {}
            Some(done) => return done,
            None => panic!("G20_SAVE_STATUS_LOST"),
        }
        assert!(started.elapsed() < S5_REVIEW_HANG, "G20_SAVE_NEVER_FINISHED");
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// The G20 workflow, `KINEWRIGHT_G20_RUNS` times (default 5) per raster.
/// Writes one JSON report to `KINEWRIGHT_G20_REPORT`.
#[test]
fn ja1_s6_c5_g20_measure() {
    let report = std::env::var_os("KINEWRIGHT_G20_REPORT")
        .map(std::path::PathBuf::from)
        .expect("G20: set KINEWRIGHT_G20_REPORT (run through scripts/measure-g20.py)");
    let runs: usize = std::env::var("KINEWRIGHT_G20_RUNS")
        .ok()
        .map_or(5, |runs| runs.parse().expect("G20_RUNS"));
    let lane = std::env::var("KINEWRIGHT_G20_GPU").unwrap_or_else(|_| "lavapipe".into());
    let mut workflows = Vec::new();
    for run in 1..=runs {
        for raster in [G20Raster::DefaultDelivery, G20Raster::SourceRaster] {
            eprintln!("G20_WORKFLOW run {run} {}", raster.label());
            workflows.push(g20_workflow(run, raster));
        }
    }
    let value = serde_json::json!({
        "schema_version": 1,
        "lane": lane,
        "comment_delay_mutation": cfg!(feature = "g20-comment-delay"),
        "debug_assertions": cfg!(debug_assertions),
        "workflows": workflows,
    });
    std::fs::write(&report, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
}
