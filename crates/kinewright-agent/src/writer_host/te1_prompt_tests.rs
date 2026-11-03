#[test]
fn te1_full_prompt_bytes_are_frozen_at_b4e9abc8() {
    let f = Fixture::new();
    let (run, _, _) = reserve(&f);
    let mut task = f.projection(&run).tasks[0].clone();
    task.id = serde_json::from_value(serde_json::json!("1".repeat(32))).unwrap();
    let evidence = task.brief.plan.clone();
    task.brief.inputs = evidence.clone();
    task.brief.scope.base.branch =
        serde_json::from_value(serde_json::json!("2".repeat(32))).unwrap();
    task.brief.scope.base.head = serde_json::from_value(serde_json::json!("3".repeat(32))).unwrap();
    task.brief.scope.base.checkpoint = evidence;
    task.brief.scope.base.document_hash = "4".repeat(64);
    let host = f.host.owner.upgrade().unwrap();
    let owner = host.lock().unwrap();
    let bytes = owner
        .codex_prompt(&task, b"frozen application payload")
        .unwrap();
    let hash = kinewright_media::sha256_bytes(&bytes);
    println!("TE1_FULL_PROMPT {hash}");
    let expected: String =
        serde_json::from_str(include_str!("../../tests/fixtures/te1-prompt.json")).unwrap();
    assert_eq!(hash, expected);
}
