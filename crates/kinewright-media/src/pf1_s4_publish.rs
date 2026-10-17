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

fn s4_publish(gpu: &GpuContext, measured: Option<(bool, f64)>) {
    s4_publish_metrics(
        gpu,
        serde_json::json!({"name": "native-display", "poll_ms": 5,
            "warmup_ms": 2000, "duration_s": 60}),
        measured.map(|(valid, fps)| (valid, serde_json::json!({"fps": fps}))),
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
