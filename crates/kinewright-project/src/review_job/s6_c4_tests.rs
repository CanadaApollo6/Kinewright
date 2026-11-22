// JA-1 S6 C4: versions carry their publish number, and the list reports
// each version's seal receipts.
#[test]
fn ja1_s6_c4_versions_list_in_publish_order_with_their_seals() {
    use kinewright_media::test_support::{TempDirectory, workspace_target_dir};
    let temp = TempDirectory::new_in(&workspace_target_dir(), "s6-c4-versions");
    let (_core, mut bundle, _) = saved_bundle(&temp);
    let ready = c7_checked(&mut bundle, &temp, false, None);
    let sealed_key = ready.token.snapshot.clone();
    let SealResult::Sealed(sealed) = c7_seal(&mut bundle, &ready).unwrap() else {
        panic!("S6C4_SEALED");
    };
    // Five more versions: random ids put them in publish order by chance
    // once in 720 runs.
    let mut published = vec![sealed_key.clone()];
    for _ in 0..5 {
        published.push(version(&mut bundle).key);
    }
    let listed = bundle.versions_in_publish_order().unwrap();
    assert_eq!(listed, published, "S6C4_PUBLISH_ORDER");
    for (index, key) in published.iter().enumerate() {
        let descriptor = bundle.read_snapshot(key).unwrap().descriptor;
        assert_eq!(
            descriptor.number,
            u32::try_from(index + 1).unwrap(),
            "S6C4_NUMBER"
        );
        assert_eq!(descriptor.label(), format!("v{}", index + 1));
    }
    let seals = bundle.sealed_versions().unwrap();
    assert_eq!(
        seals,
        vec![delivery::SealedVersion {
            key: sealed_key,
            number: 1,
            outcome: sealed.receipt.outcome.clone(),
            key_sha256: sealed.receipt.key_sha256.clone(),
        }],
        "S6C4_SEALED_OUTCOME"
    );
}

#[test]
fn ja1_s6_c4_notice_text_never_calls_facts_not_checked_delivered() {
    use kinewright_core::output_review::SealedReviewOutcome;
    assert_eq!(
        delivery::seal_notice_text("v3", &SealedReviewOutcome::Delivered),
        "v3 is delivered"
    );
    let facts = SealedReviewOutcome::ReviewedFactsNotChecked { exceptions: vec![] };
    let text = delivery::seal_notice_text("v3", &facts);
    assert_eq!(text, "v3 is reviewed, facts not checked");
    assert!(!text.contains("deliver"), "S6C4_NOT_DELIVERED");
}
