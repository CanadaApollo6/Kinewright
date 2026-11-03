//! The Director registry, as the person meets it: only connectors this build
//! can really launch are offered, the person picks one explicitly, and a saved
//! choice is kept as it is, even when it is no longer offered, instead of
//! being replaced by another provider.
use kinewright_agent::orchestration::adapters::available_connectors;
use kinewright_agent::orchestration::director_setting::{
    DirectorStatus, configured_director, director_choices, director_status, write_director_setting,
};
use kinewright_agent::orchestration::orchestration_records::ProviderSelectionRecord;

fn folder(name: &str) -> kinewright_media::test_support::TempDirectory {
    kinewright_media::test_support::TempDirectory::new_in(
        &kinewright_media::test_support::workspace_target_dir(),
        name,
    )
}

/// Every offered connector has a real adapter and launch recipe on this
/// platform; test fixtures and connectors without one are never offered.
#[test]
fn s4b_registry_offers_only_connectors_with_a_real_adapter_and_launch_recipe() {
    let specifications = available_connectors();
    assert!(
        specifications
            .iter()
            .all(|spec| !matches!(spec.provider(), "scripted" | "scripted-claude")),
        "REGISTRY_EXCLUDES_FIXTURES_ON_EVERY_FEATURE_GRAPH"
    );
    // The native launcher exists on Linux only.
    #[cfg(not(target_os = "linux"))]
    assert!(specifications.is_empty(), "NO_LAUNCHER_NO_CONNECTOR");
    #[cfg(target_os = "linux")]
    {
        use kinewright_agent::orchestration::adapters::{
            AuthenticationMode, CliVersion, McpPolicy, SandboxPolicy,
        };
        let providers: Vec<_> = specifications.iter().map(|spec| spec.provider()).collect();
        assert_eq!(providers, ["codex"], "ONLY_LAUNCHABLE_CONNECTORS");
        for spec in &specifications {
            assert_eq!(
                spec.required_authentication(),
                AuthenticationMode::Subscription
            );
            assert_eq!(spec.sandbox_policy(), SandboxPolicy::LinuxFilesystemAndIpc);
            assert_eq!(spec.mcp_policy(), McpPolicy::HostEndpointOnly);
            assert_eq!(spec.minimum_version(), CliVersion(0, 166, 0));
        }
    }
    assert_eq!(
        director_choices(),
        specifications
            .iter()
            .map(|spec| spec.selection())
            .collect::<Vec<_>>(),
        "CHOICES_ARE_THE_REGISTRY"
    );
}

/// Nothing is picked for the person: with no saved choice there is no
/// Director, even when exactly one is offered.
#[test]
fn s4b_director_requires_the_persons_explicit_pick() {
    let temp = folder("s4b-pick");
    assert_eq!(director_status(Some(temp.root())), DirectorStatus::Missing);
    assert!(
        configured_director(Some(temp.root())).is_err(),
        "NO_DEFAULT_DIRECTOR"
    );
    assert!(
        !temp.path("director.json").exists(),
        "NOTHING_WRITTEN_UNASKED"
    );
    for choice in director_choices() {
        write_director_setting(Some(temp.root()), &choice).unwrap();
        assert_eq!(
            configured_director(Some(temp.root())),
            Ok(choice.clone()),
            "EXPLICIT_PICK_IS_USED"
        );
    }
}

/// A saved choice this build does not offer stays exactly as saved, shows as
/// unavailable, and is never swapped for an offered provider.
#[test]
fn s4b_unavailable_saved_choice_is_kept_and_never_substituted() {
    let temp = folder("s4b-kept");
    let unavailable = ProviderSelectionRecord {
        connector: "claude".into(),
        model: "claude-opus-5-5".into(),
        options: vec![],
    };
    let path = temp.path("director.json");
    let saved = serde_json::to_vec_pretty(&serde_json::json!({
        "version": 1,
        "director": unavailable,
    }))
    .unwrap();
    std::fs::write(&path, &saved).unwrap();
    assert_eq!(
        director_status(Some(temp.root())),
        DirectorStatus::NoLongerOffered(unavailable.clone()),
        "UNAVAILABLE_SHOWS_AS_UNAVAILABLE"
    );
    let why = configured_director(Some(temp.root())).unwrap_err();
    assert!(
        why.contains("claude") && why.contains("no longer available"),
        "PLAIN_REASON_NAMES_THE_SAVED_CHOICE {why}"
    );
    assert!(
        write_director_setting(Some(temp.root()), &unavailable).is_err(),
        "AN_UNOFFERED_CHOICE_IS_NEVER_WRITTEN"
    );
    assert_eq!(
        std::fs::read(&path).unwrap(),
        saved,
        "SAVED_CHOICE_KEPT_BYTE_FOR_BYTE"
    );
}
