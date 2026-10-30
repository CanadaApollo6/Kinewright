use kinewright_agent::orchestration::adapters::certified_connectors;

#[test]
fn s4b_c1_release_registry_has_no_unwitnessed_provider() {
    let specifications = certified_connectors();
    #[cfg(not(feature = "test-util"))]
    assert!(
        specifications.is_empty(),
        "shipping registry must remain empty at C1"
    );
    #[cfg(feature = "test-util")]
    assert!(
        specifications
            .iter()
            .all(|spec| spec.provider() == "scripted")
    );
}
