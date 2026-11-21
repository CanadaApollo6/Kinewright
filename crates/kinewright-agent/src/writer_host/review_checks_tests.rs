// JA-1 S5a commit 5: the technical checks run on a published review artifact
// and reach the job only through `record_review_check`.
use super::super::review_job::review_checks::{
    review_check_configuration, run_technical_review_checks,
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
fn s5c5_checks(host: &WriterHost) -> kinewright_project::review_job::ReviewChecks {
    let owner = host.owner.upgrade().unwrap();
    let owner = owner.lock().unwrap();
    let job = owner
        .owner()
        .unwrap()
        .review_bundle()
        .unwrap()
        .review_job()
        .unwrap()
        .unwrap();
    let ReviewPhase::Checking(checks) = &job.phase else {
        panic!("expected a checking job, got {:?}", job.phase);
    };
    checks.clone()
}

#[test]
#[cfg(target_os = "linux")]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn ja1_s5a_checks_record_each_decided_row_through_the_review_seam() {
    use kinewright_core::output_review::{RequiredReviewRow, ReviewRowResult};
    use kinewright_media::review_scan::{RetainedSpeechConfig, TechnicalScanConfig};
    let (_temp, host, _client, token) = s5c5_checking_job("review-c5-checks");
    let scan = TechnicalScanConfig::default();
    let speech = RetainedSpeechConfig::default();
    let shared = host.owner.upgrade().unwrap();
    let attempt = kinewright_project::bundle_records::SessionId::fresh().unwrap();
    let recognitions = std::cell::Cell::new(0_u32);
    let run = run_technical_review_checks(
        &shared,
        &token,
        attempt.clone(),
        (&scan, &speech),
        Box::new(|_, _, _, _| {
            recognitions.set(recognitions.get() + 1);
            Ok(Ok(Vec::new()))
        }),
        &kinewright_core::ExportCancellation::default(),
    )
    .expect("REVIEW_C5_CHECKS_RECORDED");
    assert_eq!(recognitions.get(), 1, "REVIEW_C5_RECOGNISED_ONCE");
    // The six-frame silent solid seed decodes cleanly: every row it decides
    // passes, and the rows of commits 6 and 7 keep the job in Checking.
    let decided = [
        (
            RequiredReviewRow::StructureAndAvIntegrity,
            run.technical.structure.result,
        ),
        (
            RequiredReviewRow::LoudnessAndPeak,
            run.technical.loudness.result,
        ),
        (
            RequiredReviewRow::TechnicalDefectScan,
            run.technical.defects.result,
        ),
        (
            RequiredReviewRow::RetainedSpeechChecks,
            run.speech.row.result,
        ),
    ];
    assert_eq!(
        decided.map(|(_, result)| result),
        [ReviewRowResult::Pass; 4],
        "REVIEW_C5_SEED_PASSES: {:?} {:?}",
        run.technical,
        run.speech
    );
    assert_eq!(run.technical.decoded_frames, 6, "REVIEW_C5_DECODED_FRAMES");
    assert!(matches!(run.status.phase, ReviewPhase::Checking(_)));
    let checks = s5c5_checks(&host);
    assert_eq!(
        checks.configuration_sha256,
        Some(review_check_configuration(&scan, &speech)),
        "REVIEW_C5_BINDS_ITS_CONFIGURATION"
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
    assert_eq!(recorded, decided.to_vec(), "REVIEW_C5_ROWS_RECORDED");
    // Another attempt cannot rebind the live checker or add rows.
    let refused = run_technical_review_checks(
        &shared,
        &token,
        kinewright_project::bundle_records::SessionId::fresh().unwrap(),
        (&scan, &speech),
        Box::new(|_, _, _, _| panic!("REVIEW_C5_REFUSED_BEFORE_DECODE")),
        &kinewright_core::ExportCancellation::default(),
    )
    .err()
    .expect("REVIEW_C5_SECOND_ATTEMPT_REFUSED");
    assert!(
        refused.to_string().contains("binding already established"),
        "REVIEW_C5_SECOND_ATTEMPT_REFUSED: {refused}"
    );
    assert_eq!(s5c5_checks(&host).rows.len(), 4, "REVIEW_C5_NO_EXTRA_ROWS");
    // A changed scan configuration is a different checker for this attempt.
    let louder = TechnicalScanConfig {
        integrated_lufs_hundredths: -1_600,
        ..TechnicalScanConfig::default()
    };
    let rebound = run_technical_review_checks(
        &shared,
        &token,
        attempt,
        (&louder, &speech),
        Box::new(|_, _, _, _| panic!("REVIEW_C5_REFUSED_BEFORE_DECODE")),
        &kinewright_core::ExportCancellation::default(),
    )
    .err()
    .expect("REVIEW_C5_CONFIGURATION_FIXED");
    assert!(
        rebound.to_string().contains("binding already established"),
        "REVIEW_C5_CONFIGURATION_FIXED: {rebound}"
    );
}
