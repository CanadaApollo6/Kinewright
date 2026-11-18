#[test]
fn dq_r4_overrule_cannot_erase_an_unrequested_word_start() {
    let (s, v) = fixture();
    let before = s.snapshot().unwrap();
    let history = history_identity(&s);
    let mut removal = words(&v, 1);
    removal["overrule"] = json!([650_000, 2_050_000]);
    let result = cut(&s, args(vec![removal]), true);
    assert_eq!(
        content(&result)["status"],
        "refused",
        "DQ_R4_OVERRULE_SELECTION"
    );
    assert_eq!(content(&result)["reason"]["code"], "selection_evidence");
    assert_eq!(
        content(&result)["reason"]["violation"],
        "unrequested_word_start"
    );
    assert_eq!(content(&result)["reason"]["word"], 2);
    assert_eq!(s.snapshot().unwrap(), before);
    assert_eq!(history_identity(&s), history);
}

#[test]
fn dq_r4_selection_refusal_stays_typed_at_binding_recheck() {
    fn refusal() -> Refusal {
        Refusal::SelectionEvidence {
            issue: kinewright_core::dialogue::SelectionIssue::UnrequestedSound {
                word: 2,
                component: kinewright_core::dialogue::Span::new(2_000_000, 2_300_000).unwrap(),
            },
            at_us: 2_000_000,
            quiet_us: [Some(1_900_000), Some(2_400_000)],
        }
    }
    for fail_at in [2, 3] {
        let (s, v, _) = recheck_service(fail_at, refusal);
        let before = s.snapshot().unwrap();
        let result = cut(&s, args(vec![words(&v, 1)]), fail_at == 3);
        assert_eq!(content(&result)["status"], "refused");
        assert_eq!(
            content(&result)["reason"],
            serde_json::to_value(refusal()).unwrap()
        );
        assert_eq!(s.snapshot().unwrap(), before);
    }
}
