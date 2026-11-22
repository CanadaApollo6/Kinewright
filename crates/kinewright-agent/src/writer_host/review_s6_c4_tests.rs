// JA-1 S6 C4: `ListVersions` reports each version's sealed outcome, which
// the Studio ready line reads. Linux only, like the C7 fixtures.

#[cfg(target_os = "linux")]
#[test]
fn ja1_s6_c4_list_versions_reports_the_sealed_outcome() {
    let (job, receipt) = s6_sealed_job("A sealed title");
    let reply = ok(
        job.f.editor.request(HostCommand::ListVersions).unwrap(),
        "S6_C4_LIST_VERSIONS",
    );
    assert!(
        reply.versions.contains(&receipt.key.snapshot),
        "S6_C4_LISTED: {:?}",
        reply.versions
    );
    assert_eq!(
        reply.sealed_versions,
        vec![kinewright_project::review_job::delivery::SealedVersion {
            key: receipt.key.snapshot.clone(),
            number: 1,
            outcome: receipt.outcome.clone(),
            key_sha256: receipt.key_sha256.clone(),
        }],
        "S6_C4_LIST_SEALED"
    );
}
