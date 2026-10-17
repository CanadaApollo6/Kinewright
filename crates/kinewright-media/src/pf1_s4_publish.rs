// Shared harness-only R75 comparison; included inside pf1_harness on both trees.
fn s4_device(gpu: &GpuContext) -> serde_json::Value {
    let info = gpu.selected_adapter();
    serde_json::json!({
        "selected": {"backend": format!("{:?}", info.backend), "name": info.name,
            "vendor_id": info.vendor, "device_id": info.device, "driver": info.driver},
        "driver_info": info.driver_info,
        "software": info.device_type == wgpu::DeviceType::Cpu
    })
}

fn s4_protocol_for(gate: &str, name: &str) -> serde_json::Value {
    let mut protocol =
        serde_json::json!({"name": name, "poll_ms": 5, "warmup_ms": 2000, "duration_s": 60});
    if matches!(gate, "G2" | "G14") {
        let mut durations = serde_json::json!({"explainer_16x9":60, "reel_9x16":60, "feed_4x5":60, "talk_recut":240});
        if gate == "G14" {
            durations["typical_1080p"] = 60.into();
            durations["blend_heavy_1080p"] = 60.into();
        }
        protocol["duration_s"] = durations;
    }
    if matches!(gate, "Q3-G1" | "Q3-G14") {
        protocol["control"] = (if gate == "Q3-G1" {
            "slowdown"
        } else {
            "freeze"
        })
        .into();
    }
    protocol
}

fn s4_protocol(name: &str) -> serde_json::Value {
    s4_protocol_for(&std::env::var("PF1_GATE").unwrap_or_default(), name)
}

fn s4_ready(name: &str) -> Option<Session> {
    std::env::var_os("PF1_RECORD")?;
    let session = Session::new(Some(SimulatedAudio::paced()), Arc::default(), lane().0);
    s4_publish_metrics(&session.gpu, s4_protocol(name), None);
    Some(session)
}

fn s4_play_values(metrics: &PlayMetrics, gate: &str) -> serde_json::Value {
    if matches!(gate, "G14" | "Q3-G14") {
        serde_json::json!({"held_max_ms": metrics.held_max})
    } else {
        serde_json::json!({"miss_pct": 100.0 * (metrics.late + metrics.early + metrics.dropped) as f64 / metrics.due as f64, "present_p95_ms": metrics.present[1]})
    }
}

fn s4_assert_duration(document: &Document, key: &str) {
    let seconds = if key == "talk_recut" { 240.0 } else { 60.0 };
    assert!(
        (document.duration.0 as f64 * frame_ms(document) - seconds * 1000.0).abs() < 1e-5,
        "complete frozen timeline required"
    );
}

fn s4_allowance(document: &Document) -> Duration {
    Duration::from_secs_f64(
        1.02 * document.duration.0 as f64 * frame_ms(document) / 1000.0 + 0.5 + 5.0 + 30.0,
    )
}

fn s4_phase_deadline(
    end: Instant,
    now: Instant,
    requested: Duration,
    reserve: Duration,
) -> Instant {
    let latest = end.checked_sub(reserve).expect("phase reserve fits");
    assert!(
        now < latest,
        "insufficient shared budget; never shorten playback"
    );
    (now + requested).min(latest)
}

#[test]
fn s4_talk_budget_reserves_full_playback_and_cleanup() {
    let start = Instant::now();
    let end = start + Duration::from_secs(560);
    let reserve = Duration::from_secs(280);
    assert_eq!(
        s4_phase_deadline(
            end,
            start + Duration::from_secs(250),
            Duration::from_secs(120),
            reserve
        ),
        start + Duration::from_secs(280)
    );
    assert!(
        std::panic::catch_unwind(|| s4_phase_deadline(
            end,
            start + Duration::from_secs(281),
            Duration::ZERO,
            reserve
        ))
        .is_err()
    );
    assert_eq!(
        s4_phase_deadline(
            end,
            end.checked_sub(Duration::from_secs(10)).unwrap(),
            Duration::from_secs(30),
            Duration::ZERO
        ),
        end
    );
}

#[test]
fn s4_playback_protocol_and_metrics_keep_talk_and_expired_outcomes() {
    for gate in ["G2", "G14"] {
        assert_eq!(
            s4_protocol_for(gate, "native-display")["duration_s"]["talk_recut"],
            240
        );
    }
    let metrics = PlayMetrics {
        due: 100,
        late: 1,
        early: 2,
        dropped: 3,
        present: [10.0, 40.0, 50.0],
        held_max: 150.0,
        ..PlayMetrics::default()
    };
    assert_eq!(
        s4_play_values(&metrics, "G2"),
        serde_json::json!({"miss_pct":6.0,"present_p95_ms":40.0})
    );
    assert_eq!(
        s4_play_values(&metrics, "G14"),
        serde_json::json!({"held_max_ms":150.0})
    );
}

fn s4_publish_metrics(
    gpu: &GpuContext,
    protocol: serde_json::Value,
    measured: Option<(bool, serde_json::Value)>,
) {
    if let Ok(path) = std::env::var("PF1_RECORD") {
        let mut record = s4_device(gpu);
        record["publisher_pid"] = std::process::id().into();
        S4_INPUT.with_borrow(|slot| {
            if let Some(consumed) = slot {
                record["consumed"] = consumed.clone();
            }
        });
        record["token"] = std::env::var("PF1_TOKEN").expect("collector token").into();
        record["protocol"] = protocol;
        record["timestamp_s"] = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs_f64()
            .into();
        if let Some((valid, metrics)) = measured {
            if !matches!(record["protocol"]["name"].as_str(), Some("P-rss" | "D13")) {
                record["valid"] = valid.into();
            }
            record["metrics"] = metrics;
        }
        s4_write_record(std::path::Path::new(&path), &record);
    }
}

fn s4_write_record(path: &std::path::Path, record: &serde_json::Value) {
    let temporary = path.with_extension("tmp");
    std::fs::write(&temporary, serde_json::to_vec(record).unwrap()).unwrap();
    std::fs::rename(temporary, path).unwrap();
}

#[test]
fn s4_a_short_talk_document_is_rejected() {
    let mut document = perf_fixtures::title_only().0;
    document.duration = TimeCode(1800);
    s4_assert_duration(&document, "typical_1080p");
    assert!(std::panic::catch_unwind(|| s4_assert_duration(&document, "talk_recut")).is_err());
    document.duration = TimeCode(7200);
    s4_assert_duration(&document, "talk_recut");
}
