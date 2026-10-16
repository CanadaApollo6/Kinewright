struct D13Probe {
    ledger: Arc<crate::s4_d13::Ledger>,
    samples: std::sync::Mutex<Vec<(u128, u64, Vec<u64>)>>,
    mark: std::sync::atomic::AtomicU64,
    done: std::sync::atomic::AtomicBool,
    epoch: Instant,
    native: bool,
}

impl D13Probe {
    fn finish(
        &self,
        session: Session,
        lane: &crate::preview::Lane,
        sampler: thread::JoinHandle<u64>,
    ) -> u64 {
        let until = Instant::now() + Duration::from_secs(30);
        self.ledger.release();
        let cleanup = teardown(session);
        let cleaned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            d13_wait(|| self.ledger.closed() || Instant::now() >= until);
            assert!(self.ledger.closed(), "D13 cleanup deadline");
            assert!(lane.s4_clean(), "D13 cleanup {cleanup}");
            assert!(cleanup.contains("teardown_complete=true"), "{cleanup}");
        }));
        self.done.store(true, Ordering::Release);
        d13_wait(|| sampler.is_finished() || Instant::now() >= until);
        assert!(sampler.is_finished(), "D13 sampler cleanup deadline");
        let peak = sampler.join().expect("D13 sampler joined");
        cleaned.expect("D13 cleanup failed");
        peak
    }
    fn covered(&self, version: u64, ack: u128, original: &[u64]) -> bool {
        let samples = self.samples.lock().expect("D13 samples");
        let covered: Vec<_> = samples
            .iter()
            .filter(|(t, v, ids)| {
                *v == version && *t >= ack + 100_000 && original.iter().all(|id| ids.contains(id))
            })
            .collect();
        covered
            .windows(2)
            .any(|pair| pair[1].0 >= ack + 200_000 && pair[1].0 - pair[0].0 >= 100_000)
    }
    fn sample(&self, idle: u64) -> u64 {
        let mut peak = idle;
        while !self.done.load(Ordering::Acquire) {
            let rss = process_memory().expect("D13 RSS");
            peak = peak.max(rss.rss);
            let tag = self.mark.load(Ordering::Acquire);
            let version = tag & u64::from(u32::MAX);
            let ids = self.ledger.retained();
            let mut samples = self.samples.lock().expect("D13 samples");
            let time = if self.native {
                self.epoch.elapsed().as_micros()
            } else {
                samples.len() as u128 * 100_000
            };
            assert!(samples.len() < 6000, "D13 sample history overflow");
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

fn d13_wait(mut ready: impl FnMut() -> bool) {
    let until = Instant::now() + Duration::from_secs(10);
    while !ready() {
        assert!(Instant::now() < until, "D13 wait exceeded 10 s");
        thread::sleep(SAMPLE);
    }
}

fn d13_run(document: &Document, d: usize, native: bool) {
    use crate::s4_d13::{Ledger, START};
    let ledger = Ledger::new(d);
    START.with_borrow_mut(|slot| *slot = Some(Arc::clone(&ledger)));
    let session = Session::new(None, Arc::default(), false);
    START.with_borrow_mut(|slot| *slot = None);
    let lane = session.engine.s4_lane();
    lane.freeze_reader_clock();
    session.engine.set_document(Arc::new(document.clone()));
    let limit = Duration::from_secs(10);
    let first = session.wait_frame(0, session.engine.stamp(), Instant::now(), limit);
    assert!(first.is_some(), "D13 initial frame");
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
                        .expect("D13 samples")
                        .last()
                        .map_or(0, |s| s.0)
                };
                d13_wait(|| probe.covered(version, ack, &original));
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
}

#[test]
#[ignore = "PF1 D13 parent: release LL development series"]
fn s4_d13_active() {
    assert_release();
    let Workload(document, _media) = perf_fixtures::seek_gop60();
    let calibration = crate::s4_d13::calibrate(&document.media_pool[0].path);
    let temp = TempDirectory::new("d13");
    let path = temp.path("document.json");
    std::fs::write(&path, serde_json::to_vec(&document).expect("document"))
        .expect("write document");
    let frozen = serde_json::to_vec(&calibration).expect("calibration");
    println!(
        "D13 calibration sha256={} {}",
        crate::sha256::sha256_bytes(&frozen),
        calibration
    );
    for child in 0..3 {
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
        assert!(output.status.success(), "D13 child={child} failed");
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
