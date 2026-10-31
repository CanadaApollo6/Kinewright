use kinewright_agent::orchestration::adapters::certified_connectors;
#[test]
fn s4b_c1_release_registry_has_no_unwitnessed_provider() {
    let specifications = certified_connectors();
    assert!(
        specifications
            .iter()
            .all(|spec| !matches!(spec.provider(), "scripted" | "claude")),
        "C5_PRODUCT_REGISTRY_EXCLUDES_FIXTURES_ON_EVERY_FEATURE_GRAPH"
    );
    #[cfg(not(target_os = "linux"))]
    assert!(specifications.is_empty());
    #[cfg(target_os = "linux")]
    {
        use kinewright_agent::orchestration::adapters::CliVersion;
        assert_eq!(specifications.len(), 1);
        let specification = specifications[0];
        assert_eq!(specification.provider(), "codex");
        assert_eq!(specification.versions().first, CliVersion(0, 166, 0));
        assert_eq!(specification.versions().last, CliVersion(0, 166, 0));
        assert_eq!(specification.default_token_ceiling().get(), 2_000_000);
        let bytes = include_bytes!("../../../docs/reports/ja1-s4b/c5-release.json");
        let release: serde_json::Value = serde_json::from_slice(bytes).unwrap();
        assert_eq!(
            specification.evidence().source,
            release["source"]["sha256"].as_str().unwrap()
        );
        assert_eq!(
            specification.evidence().guardian,
            release["guardian_sha256"].as_str().unwrap()
        );
        assert_eq!(
            specification.evidence().fixtures,
            kinewright_media::sha256_bytes(bytes)
        );
        assert_eq!(release["live"]["passed"], true);
        assert_eq!(release["live"]["active_at_stop"], true);
        assert_eq!(release["live"]["reopen_exact"], true);
    }
}
