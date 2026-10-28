//! A reproducible, production-imported Studio fixture for desktop captures.
#[cfg(not(feature = "test-util"))]
fn main() {
    eprintln!("Use --features test-util to build the Studio fixture");
}
#[cfg(feature = "test-util")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use kinewright_project::{
        bundle::LoadedBundle,
        c0_fixtures::build_c0_fixture_pair,
        receipts::{CommandId, ReceiptLookup},
        studio_records::{
            MetadataChange, MetadataEnvelope, MetadataScope, MetadataUpdate, PublishSnapshot,
            VersionId,
        },
    };
    let root = std::path::PathBuf::from(
        std::env::args_os()
            .nth(1)
            .ok_or("fixture output directory required")?,
    );
    std::fs::create_dir_all(&root)?;
    let pair = build_c0_fixture_pair(&root)?;
    for fixture in pair {
        let mut bundle = LoadedBundle::read(&fixture.path)?.into_writer()?;
        for _ in 0..3 {
            let issuer = bundle.issue_metadata_issuer()?;
            let envelope = MetadataEnvelope {
                id: CommandId {
                    issuer,
                    sequence: 1,
                },
                project_id: bundle.project_id().clone(),
                target_branch: bundle.manual_branch_id().clone(),
                owner_epoch: bundle.store().owner_epoch(),
                expected_head: bundle.manual_head().cloned(),
            };
            let update = MetadataUpdate {
                expected_root_generation: bundle.root().ok_or("saved root")?.generation,
                change: MetadataChange::PublishSnapshot(PublishSnapshot {
                    version: VersionId::fresh()?,
                    expected_head: bundle.manual_head().ok_or("manual head")?.clone(),
                    checkpoint_ref: bundle.checkpoint_reference(bundle.manual_branch_id())?,
                    parent_version: None,
                    run_provenance: None,
                    revision_refs: vec![],
                }),
            };
            assert!(matches!(
                bundle.metadata_command(&envelope, MetadataScope::Snapshot, &update)?,
                ReceiptLookup::Committed(_)
            ));
        }
        println!("{}", fixture.path.display());
    }
    Ok(())
}
