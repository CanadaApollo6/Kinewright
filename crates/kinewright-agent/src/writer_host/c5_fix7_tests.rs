// C5 fix 7: the provider's stdin ends at launch, and a provider that starts
// but never speaks is interrupted at the first-progress deadline. Both run the
// real guardian, sealed image and Linux allowlist sandbox with the protocol
// fixture child; no installed provider and no model send.

#[test]
#[ignore = "on-demand: equipped Linux guardian qualification"]
fn s4b_c5_fix7_provider_stdin_reaches_eof() {
    use super::super::studio_runtime::fixture::ProtocolFixtureMode;
    let (f, run, fixture) = c5_fix3_host(ProtocolFixtureMode::ReadStdinFirst);
    let started = Instant::now();
    c4_wait(
        || {
            if let Some(refusal) = c5_fix3_refusal(&f, &run) {
                panic!("C5_FIX7_STDIN_HELD {refusal}");
            }
            fixture
                .observations()
                .first()
                .is_some_and(|o| o.scratch.join("stdin-eof").exists())
        },
        "C5_FIX7_STDIN_EOF",
    );
    let eof = started.elapsed();
    let _units = fixture
        .observations()
        .iter()
        .map(|observation| NativeUnitCleanup::new(&observation.identity))
        .collect::<Vec<_>>();
    // The fixture speaks right after EOF, so the shortened deadline passes
    // without a refusal.
    let quiet = Instant::now() + ProtocolFixtureMode::SHORT_FIRST_PROGRESS + Duration::from_secs(2);
    while Instant::now() < quiet {
        if let Some(refusal) = c5_fix3_refusal(&f, &run) {
            panic!("C5_FIX7_FIRST_EVENT_MISSING {refusal}");
        }
        std::thread::park_timeout(Duration::from_millis(20));
    }
    c5_fix3_stop(&f, &run, &fixture);
    eprintln!("C5_FIX7_STDIN_EOF_REACHED eof_ms={}", eof.as_millis());
}

#[test]
#[ignore = "on-demand: equipped Linux guardian qualification"]
fn s4b_c5_fix7_silent_provider_meets_first_progress_deadline() {
    use super::super::studio_runtime::fixture::ProtocolFixtureMode;
    let (f, run, fixture) = c5_fix3_host(ProtocolFixtureMode::SilentAlive);
    let started = Instant::now();
    let bound = started + Duration::from_secs(30);
    let refusal = loop {
        if let Some(refusal) = c5_fix3_refusal(&f, &run) {
            break refusal;
        }
        assert!(
            Instant::now() < bound,
            "C5_FIX7_SILENT_PROVIDER_WAITS projection={:?}",
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
    assert_eq!(
        refusal,
        crate::native_launcher::first_progress_failure(ProtocolFixtureMode::SHORT_FIRST_PROGRESS),
        "C5_FIX7_FIRST_PROGRESS_REASON"
    );
    assert!(
        elapsed >= ProtocolFixtureMode::SHORT_FIRST_PROGRESS,
        "C5_FIX7_DEADLINE_EARLY {elapsed:?}"
    );
    // The deadline itself retires the attempt; nobody pressed Stop.
    c4_wait(
        || {
            let projection = f.projection(&run);
            projection.attempts.len() == 1
                && projection.attempts.iter().all(|attempt| {
                    matches!(
                        attempt.state,
                        crate::orchestration::orchestration_records::AttemptState::Retired(_)
                    )
                })
        },
        "C5_FIX7_RETIRED_WITHOUT_STOP",
    );
    let observations = fixture.observations();
    assert_eq!(observations.len(), 1, "C5_FIX7_NO_RELAUNCH");
    let executions =
        std::fs::read_to_string(observations[0].scratch.join("executions")).unwrap_or_default();
    assert_eq!(executions.lines().count(), 1, "C5_FIX7_ONE_EXECUTION");
    assert_eq!(
        c5_fix3_refusal(&f, &run).as_deref(),
        Some(refusal.as_str()),
        "C5_FIX7_ONE_REFUSAL"
    );
    c5_fix3_stop(&f, &run, &fixture);
    eprintln!(
        "C5_FIX7_FIRST_PROGRESS_INTERRUPTED elapsed_ms={} reason={refusal:?}",
        elapsed.as_millis()
    );
}
