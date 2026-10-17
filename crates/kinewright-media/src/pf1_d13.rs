struct D13Probe {
    ledger: Arc<crate::s4_d13::Ledger>,
    samples: std::sync::Mutex<Vec<(u128, u64, Vec<u64>)>>,
    mark: std::sync::atomic::AtomicU64,
    done: std::sync::atomic::AtomicBool,
    epoch: Instant,
    native: bool,
}

struct D13Sampler<'a>(&'a D13Probe, Option<thread::JoinHandle<u64>>, Instant);
impl Drop for D13Sampler<'_> {
    fn drop(&mut self) {
        self.0.done.store(true, Ordering::Release);
        if let Some(sampler) = self.1.take() {
            while !sampler.is_finished() && Instant::now() < self.2 {
                thread::sleep(SAMPLE);
            }
            if sampler.is_finished() {
                let _ = sampler.join();
            } else {
                eprintln!("D13 sampler did not stop by the cleanup deadline");
            }
        }
    }
}

impl D13Probe {
    fn finish(
        &self,
        session: Session,
        lane: &crate::preview::Lane,
        sampler: thread::JoinHandle<u64>,
    ) -> u64 {
        let until = Instant::now() + Duration::from_secs(30);
        let mut sampler = D13Sampler(self, Some(sampler), until);
        self.ledger.release();
        let cleanup = teardown_until(session, until);
        let cleaned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            d13_wait_until(until, || self.ledger.closed());
            assert!(self.ledger.closed(), "D13 cleanup deadline");
            assert!(lane.s4_clean(), "D13 cleanup {cleanup}");
            assert!(cleanup.contains("teardown_complete=true"), "{cleanup}");
        }));
        self.done.store(true, Ordering::Release);
        d13_wait_until(until, || sampler.1.as_ref().unwrap().is_finished());
        let peak = sampler
            .1
            .take()
            .unwrap()
            .join()
            .expect("D13 sampler joined");
        cleaned.expect("D13 cleanup failed");
        peak
    }
    fn covered(&self, version: u64, ack: u128, original: &[u64]) -> Option<[u128; 2]> {
        let samples = self.samples.lock().unwrap_or_else(PoisonError::into_inner);
        let covered: Vec<_> = samples
            .iter()
            .filter(|(t, v, ids)| {
                *v == version && *t >= ack + 100_000 && original.iter().all(|id| ids.contains(id))
            })
            .collect();
        covered
            .windows(2)
            .find(|pair| pair[1].0 >= ack + 200_000 && pair[1].0 - pair[0].0 >= 100_000)
            .map(|pair| [pair[0].0, pair[1].0])
    }
    fn sample(&self, idle: u64) -> u64 {
        let mut peak = idle;
        while !self.done.load(Ordering::Acquire) {
            let rss = process_memory().expect("D13 RSS");
            peak = peak.max(rss.rss);
            let tag = self.mark.load(Ordering::Acquire);
            let version = tag & u64::from(u32::MAX);
            let ids = self.ledger.retained();
            let mut samples = self.samples.lock().unwrap_or_else(PoisonError::into_inner);
            if self.done.load(Ordering::Acquire) {
                break;
            }
            let time = if self.native {
                self.epoch.elapsed().as_micros()
            } else {
                samples.len() as u128 * 100_000
            };
            assert!(
                samples.len() < if self.native { 6000 } else { 60_000 },
                "D13 sample history overflow"
            );
            println!(
                "D13 sample {}",
                serde_json::json!({"ordinal": samples.len(), "us": time, "version": version, "cycle": tag >> 32, "child": std::env::var("PF1_D13_CHILD").ok(), "rss": rss.rss, "high_water": rss.peak, "allocations": self.ledger.snapshot()})
            );
            samples.push((time, version, ids));
            drop(samples);
            thread::sleep(Duration::from_millis(if self.native { 100 } else { 10 }));
        }
        peak
    }
}

fn d13_wait(ready: impl FnMut() -> bool) {
    d13_wait_until(Instant::now() + Duration::from_secs(10), ready);
}

fn d13_wait_until(until: Instant, mut ready: impl FnMut() -> bool) {
    while !ready() {
        assert!(Instant::now() < until, "D13 wait deadline");
        thread::sleep(SAMPLE);
    }
}

fn d13_run(document: &Document, d: usize, native: bool) {
    use crate::s4_d13::{Ledger, START};
    let ledger = Ledger::new(d);
    START.with_borrow_mut(|slot| *slot = Some(Arc::clone(&ledger)));
    let session = Session::new(None, Arc::default(), false);
    START.with_borrow_mut(|slot| *slot = None);
    let publication = d13_start(&session.gpu, d, native);
    let lane = session.engine.s4_lane();
    lane.freeze_reader_clock();
    session.engine.set_document(Arc::new(document.clone()));
    let limit = Duration::from_secs(10);
    let first = session.wait_frame(0, session.engine.stamp(), Instant::now(), limit);
    if first.is_none() {
        let until = Instant::now() + Duration::from_secs(30);
        ledger.release();
        let cleanup = teardown_until(session, until);
        d13_wait_until(until, || ledger.closed());
        assert!(lane.s4_clean(), "D13 warm-up cleanup {cleanup}");
        assert!(cleanup.contains("teardown_complete=true"), "{cleanup}");
        panic!("D13 initial frame; cleanup: {cleanup}");
    }
    if native {
        thread::sleep(Duration::from_secs(6));
    }
    let idle = process_memory().expect("D13 idle").rss;
    let probe = Arc::new(D13Probe {
        ledger: Arc::clone(&ledger),
        samples: std::sync::Mutex::default(),
        mark: std::sync::atomic::AtomicU64::new(0),
        done: std::sync::atomic::AtomicBool::new(false),
        epoch: Instant::now(),
        native,
    });
    let observed = Arc::clone(&probe);
    let sampler = thread::spawn(move || observed.sample(idle));
    let mut intervals = Vec::new();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let (start, target, back, forward) = if native {
            (969, 957, 180, 1200)
        } else {
            (20, 19, 2, 40)
        };
        let seek = |at| {
            session.engine.seek(TimeCode(at));
            session.engine.stamp()
        };
        let shown = |at, issued| {
            assert!(
                session
                    .wait_frame(at, issued, Instant::now(), Duration::from_secs(10))
                    .is_some(),
                "D13 target {at}"
            );
        };
        for cycle in 0..if native { 30 } else { 1 } {
            println!("D13 before_reset cycle={cycle} {}", ledger.snapshot());
            session.engine.set_document(Arc::new(document.clone()));
            shown(0, session.engine.stamp());
            shown(start, seek(start));
            assert!(lane.s4_absent(target), "D13 target must be nonresident");
            ledger.arm(target - 1);
            shown(target, seek(target));
            d13_wait(|| ledger.held());
            let original = ledger.retained();
            assert!(!original.is_empty(), "D13 refill retained nothing");
            for at in [back, forward] {
                let applied = lane.s4_applied();
                seek(at);
                let (applied_at, version) = applied
                    .recv_timeout(Duration::from_secs(10))
                    .expect("D13 applied plan");
                assert_eq!(applied_at, at);
                probe.mark.store((cycle << 32) | version, Ordering::Release);
                let ack = if native {
                    probe.epoch.elapsed().as_micros()
                } else {
                    probe
                        .samples
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .last()
                        .map_or(0, |s| s.0)
                };
                d13_wait(|| probe.covered(version, ack, &original).is_some());
                intervals.push(serde_json::json!({"cycle":cycle, "target":at, "version":version,
                    "ack_us":ack, "identities":original, "sample_us":probe.covered(version, ack, &original).unwrap()}));
            }
            ledger.release();
            shown(forward, session.engine.stamp());
            d13_wait(|| ledger.retired(&original));
            session.assert_no_error("D13 cycle");
        }
    }));
    let peak = probe.finish(session, &lane, sampler);
    println!(
        "D13 figures {}",
        serde_json::json!({"U": 30*d, "d_r": d, "idle": idle, "peak": peak, "peak_minus_idle": peak-idle, "excess": i128::from(peak-idle) - (30*d) as i128})
    );
    result.unwrap_or_else(|error| std::panic::resume_unwind(error));
    d13_publish_result(publication, intervals, idle, peak, d);
}

fn d13_start(gpu: &GpuContext, d: usize, native: bool) -> Option<(GpuContext, serde_json::Value)> {
    if !native {
        return None;
    }
    let protocol = serde_json::json!({"name":"D13", "poll_ms":5, "children":3, "cycles":30, "B":16, "reader_cap":2, "d_bytes":d});
    s4_publish_metrics(gpu, protocol.clone(), None);
    Some((gpu.clone(), protocol))
}

fn d13_publish_result(
    publication: Option<(GpuContext, serde_json::Value)>,
    intervals: Vec<serde_json::Value>,
    idle: u64,
    peak: u64,
    d: usize,
) {
    let Some((gpu, protocol)) = publication else {
        return;
    };
    assert_eq!(intervals.len(), 60, "D13 complete cancellation coverage");
    s4_publish_metrics(
        &gpu,
        protocol,
        Some((
            true,
            serde_json::json!({
        "oracle_ok":true, "cleanup_ok":true, "idle_bytes":idle, "peak_bytes":peak,
        "peak_minus_idle_bytes":peak-idle, "u_bytes":30*d}),
        )),
    );
    if let Ok(path) = std::env::var("PF1_RECORD") {
        let path = std::path::Path::new(&path);
        let mut record: serde_json::Value =
            serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        record["d13_intervals"] = intervals.into();
        record["calibration_sha256"] = std::env::var("PF1_D13_CALIBRATION_SHA256")
            .expect("frozen calibration")
            .into();
        s4_write_record(path, &record);
    }
}

#[test]
#[ignore = "PF1 D13 parent: release LL development series"]
fn s4_d13_active() {
    assert_release();
    let collecting = std::env::var_os("PF1_RECORD").is_some();
    let Workload(document, _media) =
        s4_input("seek_gop60").unwrap_or_else(perf_fixtures::seek_gop60);
    let calibration = if collecting {
        serde_json::from_str(&std::env::var("PF1_D13_CALIBRATION").expect("frozen calibration"))
            .unwrap()
    } else {
        crate::s4_d13::calibrate(&document.media_pool[0].path)
    };
    let temp = TempDirectory::new("d13");
    let path = temp.path("document.json");
    std::fs::write(&path, serde_json::to_vec(&document).expect("document"))
        .expect("write document");
    let frozen = if collecting {
        std::env::var("PF1_D13_CALIBRATION").unwrap().into_bytes()
    } else {
        serde_json::to_vec(&calibration).expect("calibration")
    };
    println!(
        "D13 calibration sha256={} {}",
        crate::sha256::sha256_bytes(&frozen),
        calibration
    );
    let offset: usize = if collecting {
        std::env::var("S3B_RUN_OFFSET").unwrap().parse().unwrap()
    } else {
        0
    };
    assert!(offset < 3);
    for child in offset..if collecting { offset + 1 } else { 3 } {
        let record = std::env::var_os("PF1_RECORD")
            .map_or_else(|| temp.path("record.json"), std::path::PathBuf::from);
        let receipt = temp.path("receipt.json");
        let token = std::env::var("PF1_TOKEN").unwrap_or_else(|_| "d13-development".into());
        let output = std::process::Command::new(std::env::current_exe().expect("test binary"))
            .args([
                "--exact",
                "pf1_harness::pf1_rss_child",
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("PF1_RSS_DOCUMENT", &path)
            .env("PF1_D13_CHILD", child.to_string())
            .env("PF1_RECORD", &record)
            .env("PF1_TOKEN", &token)
            .env("PF1_CHILD_RECEIPT", &receipt)
            .env(
                "PF1_D13_CALIBRATION_SHA256",
                crate::sha256::sha256_bytes(&frozen),
            )
            .env(
                "PF1_D13_CALIBRATION",
                String::from_utf8(frozen.clone()).expect("JSON"),
            )
            .s4_output()
            .expect("D13 child watchdog");
        println!(
            "D13 child={child} {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let mut published = s4_import_child(&output, &record, &receipt, &token);
        published["native_children"] = 1.into();
        s4_write_record(&record, &published);
    }
}

macro_rules! d13_witness {
    ($name:ident) => {
        #[test]
        fn $name() {
            let workload = perf_fixtures::cuts((160, 90), 60, 1, 2, 30);
            let calibration = crate::s4_d13::calibrate(&workload.0.media_pool[0].path);
            d13_run(
                &workload.0,
                calibration["d_bytes"].as_u64().expect("d") as usize,
                false,
            );
        }
    };
}
d13_witness!(s4_d13_retention_stays_within_u);
d13_witness!(s4_d13_observes_both_applied_cancellations);
d13_witness!(s4_d13_cleanup_drops_every_retained_owner);
