#[test]
fn dq_native_task_without_admitted_audio_returns_identity_refusal() {
    let f = Fixture::new();
    let (run, execution, producer) = reserve(&f);
    let _task = task_actor(&f, &run, &execution, &producer);
    let owner = f.host.owner.upgrade().unwrap();
    let server = owner
        .lock()
        .unwrap()
        .start_task_server(&producer.bootstrap.auth, &run, &execution.task)
        .unwrap();
    let provider = server
        .test_dialogue_measurements()
        .expect("native task installs measurements");
    let identity = crate::dialogue_tools::MediaIdentity {
        sha256: "a".repeat(64),
        byte_length: 1,
    };
    assert_eq!(
        provider.audio_stream(&identity),
        Err(crate::dialogue_tools::Refusal::Identity)
    );
    assert!(matches!(
        provider.get(&identity, 0),
        Err(crate::dialogue_tools::Refusal::Identity)
    ));
}

#[test]
fn dq_native_task_cannot_measure_a_recording_outside_its_admission() {
    let f = Fixture::with_media(true);
    let (run, execution, producer) = reserve(&f);
    let _task = task_actor(&f, &run, &execution, &producer);
    let owner = f.host.owner.upgrade().unwrap();
    let mut host = owner.lock().unwrap();
    let task_core = host.hooks.runs[&run].tasks[&execution.task].core().clone();
    let asset = task_core.export_history().unwrap().document.media_pool[0].clone();
    let identity = crate::dialogue_tools::MediaIdentity::of(&asset).unwrap();
    let mut foreign = asset;
    foreign.id = kinewright_core::AssetId(999);
    foreign.path = f.temp.path("outside-task.wav");
    let bytes = kinewright_media::test_support::wav_f32(&vec![0.125; 4800], 48_000, 1);
    std::fs::write(&foreign.path, &bytes).unwrap();
    foreign.source_fingerprint.content_sha256 = Some(kinewright_media::sha256_bytes(&bytes));
    foreign.source_fingerprint.byte_len = Some(bytes.len() as u64);
    let outside = crate::dialogue_tools::MediaIdentity::of(&foreign).unwrap();
    assert!(matches!(
        task_core.request(kinewright_core::Command::Do(
            kinewright_core::Operation::AddAsset { asset: foreign }
        )),
        Ok(kinewright_core::Event::DocumentChanged { .. })
    ));
    let server = host
        .start_task_server(&producer.bootstrap.auth, &run, &execution.task)
        .unwrap();
    drop(host);
    let provider = server
        .test_dialogue_measurements()
        .expect("native task installs measurements");
    assert_eq!(
        provider.audio_stream(&identity),
        Ok(0),
        "admitted recording is measurable"
    );
    assert_eq!(
        provider.audio_stream(&outside),
        Err(crate::dialogue_tools::Refusal::Identity)
    );
    assert!(matches!(
        provider.get(&outside, 0),
        Err(crate::dialogue_tools::Refusal::Identity)
    ));
    assert_eq!(
        provider.audio_stream(&identity),
        Ok(0),
        "admission stays usable after refusal"
    );
}
