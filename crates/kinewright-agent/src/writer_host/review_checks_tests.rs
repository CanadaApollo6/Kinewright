// JA-1 S5a commit 5: the technical checks run on a published review artifact
// and reach the job only through `record_review_check`.
use super::super::review_job::review_checks::{
    COMMIT_5_ROWS, commit_5_check, review_check_configuration, run_technical_review_checks,
};

#[cfg(target_os = "linux")]
fn s5c5_checking_job(
    marker: &str,
) -> (
    kinewright_media::test_support::TempDirectory,
    WriterHost,
    HostClient,
    ReviewToken,
) {
    let temp = kinewright_media::test_support::TempDirectory::new(marker);
    s5_review_seed(temp.root());
    let (host, client) = s5_review_host(temp.root());
    let key = s5_review_version(&host);
    let token = s5_review_start(&client, &key);
    s5_review_until(
        || {
            let owner = host.owner.upgrade().unwrap();
            let owner = owner.lock().unwrap();
            let bundle = owner.owner().unwrap().review_bundle().unwrap();
            matches!(
                bundle.review_job().unwrap().map(|job| job.phase.clone()),
                Some(ReviewPhase::Checking(_))
            )
        },
        marker,
    );
    (temp, host, client, token)
}

#[cfg(target_os = "linux")]
fn s5c5_checks(host: &WriterHost) -> Option<kinewright_project::review_job::ReviewChecks> {
    let owner = host.owner.upgrade().unwrap();
    let owner = owner.lock().unwrap();
    let job = owner
        .owner()
        .unwrap()
        .review_bundle()
        .unwrap()
        .review_job()
        .unwrap()?;
    match &job.phase {
        ReviewPhase::Checking(checks) => Some(checks.clone()),
        _ => None,
    }
}

#[cfg(target_os = "linux")]
fn s5c5_binding(
    attempt: &kinewright_project::bundle_records::SessionId,
) -> kinewright_project::review_job::ReviewCheckBinding {
    use kinewright_media::review_scan::{RetainedSpeechConfig, TechnicalScanConfig};
    kinewright_project::review_job::ReviewCheckBinding {
        configuration_sha256: review_check_configuration(
            &TechnicalScanConfig::default(),
            &RetainedSpeechConfig::default(),
        ),
        reviewer_attempt: attempt.clone(),
    }
}

#[test]
#[cfg(target_os = "linux")]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn ja1_s5a_checks_record_each_decided_row_through_the_review_seam() {
    use kinewright_core::output_review::ReviewRowResult;
    use kinewright_media::review_scan::{RetainedSpeechConfig, TechnicalScanConfig};
    let (_temp, host, _client, token) = s5c5_checking_job("review-c5-checks");
    let scan = TechnicalScanConfig::default();
    let speech = RetainedSpeechConfig::default();
    let cancel = kinewright_core::ExportCancellation::default();
    let shared = host.owner.upgrade().unwrap();
    let attempt = kinewright_project::bundle_records::SessionId::fresh().unwrap();
    // The caller's binding is recorded as given (L13): commit 6 combines its
    // configuration into the same digest before it calls.
    let binding = kinewright_project::review_job::ReviewCheckBinding {
        configuration_sha256: [0x5c; 32],
        reviewer_attempt: attempt.clone(),
    };
    let checks = std::cell::Cell::new(0_u32);
    let real = commit_5_check(&scan, &speech, &cancel);
    let run = run_technical_review_checks(&shared, &token, &binding, &|file, inputs| {
        // F17: the artifact is verified and checked with the lock released.
        assert!(shared.try_lock().is_ok(), "REVIEW_C5_LOCK_RELEASED");
        checks.set(checks.get() + 1);
        real(file, inputs)
    })
    .expect("REVIEW_C5_CHECKS_RECORDED");
    assert_eq!(checks.get(), 1, "REVIEW_C5_CHECKED_ONCE");
    let report = run.report.as_ref().expect("REVIEW_C5_REPORT");
    // The six-frame silent solid seed: nothing to compare is a pass only
    // because every frame is a generated solid, delegated, and no audio is
    // expected.
    assert_eq!(
        report.rows().map(|row| row.result),
        [ReviewRowResult::Pass; 4],
        "REVIEW_C5_SEED_PASSES: {report:?}"
    );
    assert!(
        report.defects.coverage.iter().any(|coverage| {
            coverage.unit == kinewright_media::review_scan::CoverageUnit::Frames
                && coverage.delegated == 6
                && coverage.checked == 0
        }),
        "REVIEW_C5_SOLID_DELEGATED: {:?}",
        report.defects.coverage
    );
    assert_eq!(report.decoded_frames, 6, "REVIEW_C5_DECODED_FRAMES");
    assert!(matches!(run.status.phase, ReviewPhase::Checking(_)));
    let checks = s5c5_checks(&host).unwrap();
    assert_eq!(
        checks.configuration_sha256,
        Some([0x5c; 32]),
        "REVIEW_C5_BINDS_THE_CALLERS_CONFIGURATION"
    );
    assert_eq!(checks.reviewer_attempt.as_ref(), Some(&attempt));
    let recorded: Vec<_> = checks
        .rows
        .iter()
        .map(|(&row, report)| {
            assert_eq!(report.attempt, attempt, "REVIEW_C5_ROW_ATTEMPT");
            (row, report.result)
        })
        .collect();
    assert_eq!(
        recorded,
        COMMIT_5_ROWS
            .map(|row| (row, ReviewRowResult::Pass))
            .to_vec(),
        "REVIEW_C5_ROWS_RECORDED"
    );
    // Another attempt cannot rebind the live checker or add rows.
    let other = kinewright_project::bundle_records::SessionId::fresh().unwrap();
    let refused = run_technical_review_checks(&shared, &token, &s5c5_binding(&other), &|_, _| {
        panic!("REVIEW_C5_REFUSED_BEFORE_DECODE")
    })
    .err()
    .expect("REVIEW_C5_SECOND_ATTEMPT_REFUSED");
    assert!(
        refused.to_string().contains("binding already established"),
        "REVIEW_C5_SECOND_ATTEMPT_REFUSED: {refused}"
    );
    assert_eq!(
        s5c5_checks(&host).unwrap().rows.len(),
        4,
        "REVIEW_C5_NO_EXTRA_ROWS"
    );
    // A changed configuration is a different checker for this attempt.
    let rebound = run_technical_review_checks(&shared, &token, &s5c5_binding(&attempt), &|_, _| {
        panic!("REVIEW_C5_REFUSED_BEFORE_DECODE")
    })
    .err()
    .expect("REVIEW_C5_CONFIGURATION_FIXED");
    assert!(
        rebound.to_string().contains("binding already established"),
        "REVIEW_C5_CONFIGURATION_FIXED: {rebound}"
    );
}

/// F18: a check that errors still records every row, as Unknown.
#[test]
#[cfg(target_os = "linux")]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn ja1_s5a_a_failed_check_records_every_row_unknown() {
    use kinewright_core::output_review::ReviewRowResult;
    let (_temp, host, _client, token) = s5c5_checking_job("review-c5-unknown");
    let shared = host.owner.upgrade().unwrap();
    let attempt = kinewright_project::bundle_records::SessionId::fresh().unwrap();
    let run = run_technical_review_checks(&shared, &token, &s5c5_binding(&attempt), &|_, _| {
        Err(kinewright_core::MediaError::Backend(
            "REVIEW_C5_DECODER_LOST".into(),
        ))
    })
    .expect("REVIEW_C5_UNKNOWN_RECORDED");
    assert!(
        run.report
            .as_ref()
            .is_err_and(|reason| reason.contains("REVIEW_C5_DECODER_LOST")),
        "REVIEW_C5_REASON_KEPT"
    );
    let recorded: Vec<_> = s5c5_checks(&host)
        .unwrap()
        .rows
        .iter()
        .map(|(&row, report)| (row, report.result))
        .collect();
    assert_eq!(
        recorded,
        COMMIT_5_ROWS
            .map(|row| (row, ReviewRowResult::Unknown))
            .to_vec(),
        "REVIEW_C5_EVERY_ROW_UNKNOWN"
    );
}

/// F30: a stop that lands while the check runs fences its results; the stop
/// completes because the lock is free during the check.
#[test]
#[cfg(target_os = "linux")]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn ja1_s5a_a_stop_during_the_check_fences_its_rows() {
    let (_temp, host, client, token) = s5c5_checking_job("review-c5-stop");
    let shared = host.owner.upgrade().unwrap();
    let attempt = kinewright_project::bundle_records::SessionId::fresh().unwrap();
    let refused = run_technical_review_checks(&shared, &token, &s5c5_binding(&attempt), &|_, _| {
        let stopped = s5_review_call(&client, ReviewControl::Stop(token.clone())).unwrap();
        assert!(
            !matches!(stopped.phase, ReviewPhase::Checking(_)),
            "REVIEW_C5_STOPPED: {:?}",
            stopped.phase
        );
        Err(kinewright_core::MediaError::Backend("unused".into()))
    })
    .err()
    .expect("REVIEW_C5_STOP_FENCES_RECORDING");
    assert!(
        !refused.to_string().is_empty(),
        "REVIEW_C5_STOP_FENCES_RECORDING: {refused}"
    );
    assert!(s5c5_checks(&host).is_none(), "REVIEW_C5_NO_ROWS_AFTER_STOP");
}
