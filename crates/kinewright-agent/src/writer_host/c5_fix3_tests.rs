// C5 fix 3: the contained provider can resolve its own executable, and a
// provider that dies right after exec is reported at once, not at a deadline.
// Both run the real guardian, sealed image and Linux allowlist sandbox with the
// protocol fixture child; no installed provider and no model send.
fn c5_fix3_host(
    mode: super::super::studio_runtime::fixture::ProtocolFixtureMode,
) -> (
    Fixture,
    RunId,
    super::super::studio_runtime::fixture::ProtocolFixture,
) {
    use super::super::studio_runtime::fixture::ProtocolFixture;
    let f = Fixture::new();
    let started = f.start();
    let run: RunId = started.envelope.identity("run").unwrap();
    let fixture = ProtocolFixture::new(f.temp.root(), mode);
    let Fixture {
        host,
        temp,
        brief,
        issuer_auth,
        ..
    } = f;
    drop(host);
    let roots = SessionRoots {
        read: vec![temp.root().into()],
        write: vec![temp.root().into()],
    };
    let host = WriterHost::start_inner(
        HostConfig {
            scheduler_settings: Some(temp.path("scheduler")),
            project: "hooks".into(),
            path: Some(temp.path("project.kinewright")),
            roots: roots.clone(),
            origin: HostOrigin::Headless,
        },
        HostAuth::generate().unwrap(),
        Some(issuer_auth.clone()),
        fixture.ports(),
    )
    .unwrap();
    wait_runtime_recovery(&host);
    let editor = host.client();
    let issuer = HostIssuer::new(host.endpoint.clone(), issuer_auth.clone(), roots).unwrap();
    let f = Fixture {
        host,
        editor,
        issuer,
        issuer_auth,
        brief,
        temp,
    };
    ok(
        f.editor.request(HostCommand::Status).unwrap(),
        "C5_FIX3_PRODUCTION_DRIVE",
    );
    (f, run, fixture)
}
fn c5_fix3_refusal(f: &Fixture, run: &RunId) -> Option<String> {
    let projection = f.projection(run);
    let RunWait::Provider(reason) = projection.wait else {
        return None;
    };
    if projection.phase != RunPhase::Interrupted {
        return None;
    }
    let host = f.host.owner.upgrade().unwrap();
    let owner = host.lock().unwrap();
    Some(
        String::from_utf8_lossy(
            &owner
                .owner()
                .unwrap()
                .store()
                .unwrap()
                .read_evidence(&reason.reason)
                .unwrap(),
        )
        .into_owned(),
    )
}
fn c5_fix3_stop(
    f: &Fixture,
    run: &RunId,
    fixture: &super::super::studio_runtime::fixture::ProtocolFixture,
) {
    // The fixture's turn may advance the generation between read and stop.
    let mut attempts = 0;
    loop {
        let stop = f.control(&f.editor, run, RunAction::Stop);
        let reply = f
            .editor
            .request(HostCommand::DispatchOrchestration {
                request: Box::new(stop),
            })
            .unwrap();
        attempts += 1;
        match &reply.error {
            Some(error) if error.message == "StaleGeneration" && attempts < 20 => {
                std::thread::sleep(Duration::from_millis(50));
            }
            _ => {
                ok(reply, "C5_FIX3_STOP");
                break;
            }
        }
    }
    for observation in fixture.observations() {
        let _ = std::fs::write(observation.scratch.join("exit"), b"");
    }
    c4_wait(
        || {
            let _ = f.editor.request(HostCommand::Status);
            !fixture.claim_root().join("run.json").exists()
        },
        "C5_FIX3_CLAIM_FINISHED",
    );
    fixture.cleanup().unwrap();
}

#[test]
#[ignore = "on-demand: equipped Linux guardian qualification"]
fn s4b_c5_fix3_provider_resolves_its_own_executable() {
    use super::super::studio_runtime::fixture::ProtocolFixtureMode;
    let (f, run, fixture) = c5_fix3_host(ProtocolFixtureMode::Normal);
    c4_wait(
        || {
            if let Some(refusal) = c5_fix3_refusal(&f, &run) {
                panic!("C5_FIX3_PROVIDER_CANNOT_START {refusal}");
            }
            fixture
                .observations()
                .first()
                .is_some_and(|o| o.scratch.join("execution-ready").exists())
        },
        "C5_FIX3_PROVIDER_EXECUTED",
    );
    let _units = fixture
        .observations()
        .iter()
        .map(|observation| NativeUnitCleanup::new(&observation.identity))
        .collect::<Vec<_>>();
    c5_fix3_stop(&f, &run, &fixture);
    eprintln!("C5_FIX3_CURRENT_EXE_RESOLVED executed=true");
}

#[test]
#[ignore = "on-demand: equipped Linux guardian qualification"]
fn s4b_c5_fix3_launch_death_surfaces_promptly() {
    use super::super::studio_runtime::fixture::ProtocolFixtureMode;
    let (f, run, fixture) = c5_fix3_host(ProtocolFixtureMode::ExitAtLaunch);
    let started = Instant::now();
    let deadline = started + Duration::from_secs(60);
    let refusal = loop {
        if let Some(refusal) = c5_fix3_refusal(&f, &run) {
            break refusal;
        }
        assert!(
            Instant::now() < deadline,
            "C5_FIX3_LAUNCH_DEATH_SILENT projection={:?}",
            f.projection(&run)
        );
        std::thread::park_timeout(Duration::from_millis(5));
    };
    let elapsed = started.elapsed();
    let _units = fixture
        .observations()
        .iter()
        .map(|observation| NativeUnitCleanup::new(&observation.identity))
        .collect::<Vec<_>>();
    // C5 fix 4: the stored reason carries the guardian's own status only.
    // The provider's 192 KiB stderr, its synthetic credential-looking value
    // and its forged guardian record never reach project evidence.
    assert_eq!(
        refusal, "the provider exited before its first event (status 3)",
        "C5_FIX4_LAUNCH_DEATH_REASON"
    );
    let observations = fixture.observations();
    assert_eq!(observations.len(), 1, "C5_FIX3_NO_RELAUNCH");
    let executions =
        std::fs::read_to_string(observations[0].scratch.join("executions")).unwrap_or_default();
    assert_eq!(executions.lines().count(), 1, "C5_FIX3_ONE_EXECUTION");
    c5_fix3_stop(&f, &run, &fixture);
    eprintln!(
        "C5_FIX3_LAUNCH_DEATH_SURFACED elapsed_ms={} reason={refusal:?}",
        elapsed.as_millis()
    );
}
