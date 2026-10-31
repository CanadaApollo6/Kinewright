use kinewright_agent::orchestration::adapters::certified_connectors;
#[test]
fn s4b_c1_release_registry_has_no_unwitnessed_provider() {
    assert!(
        certified_connectors().is_empty(),
        "C5_PRODUCT_REGISTRY_EXCLUDES_FIXTURES_ON_EVERY_FEATURE_GRAPH"
    );
}
