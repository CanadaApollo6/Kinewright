#![allow(
    dead_code,
    reason = "each fixture selects part of the shared operation cases"
)]
use kinewright_core::{
    Command, Document, OpError, Operation,
    undo::{History, Record, RetentionPolicy, RoleBounds},
};
pub(super) trait ApplyHistoryOracle {
    fn apply_with_history_oracle(&self, document: &mut Document) -> Result<(), OpError>;
}
impl ApplyHistoryOracle for Operation {
    fn apply_with_history_oracle(&self, document: &mut Document) -> Result<(), OpError> {
        static SEEN: std::sync::OnceLock<std::sync::Mutex<std::collections::BTreeSet<String>>> =
            std::sync::OnceLock::new();
        // Independent pre-state and direct result are deliberate oracle copies.
        let before = document.clone();
        let result = self.apply(document);
        if result.is_ok() {
            let raw = serde_json::to_value(self).unwrap();
            let name = raw.as_object().unwrap().keys().next().unwrap().to_owned();
            let mut core =
                History::new(before, RetentionPolicy::NeverFold, RoleBounds::TASK).unwrap();
            core.execute(Command::Do(self.clone())).unwrap();
            assert_eq!(core.document().as_ref(), document);
            let record = core.record();
            let encoded = record.encode().unwrap();
            let identity = record.snapshot_identity().unwrap();
            let reopened = Record::decode(&encoded, RoleBounds::TASK)
                .unwrap()
                .into_history()
                .unwrap();
            // Runtime charges include actual spare capacities. Decoding may
            // change those allocations; the complete canonical state must stay.
            assert_eq!(
                reopened.record().encode().unwrap(),
                encoded,
                "NATIVE_OPERATION_RECORD_ROUNDTRIP"
            );
            assert_eq!(
                reopened.snapshot_identity().unwrap(),
                identity,
                "NATIVE_OPERATION_IDENTITY_ROUNDTRIP"
            );
            assert_eq!(
                reopened.document().as_ref(),
                &*document,
                "NATIVE_OPERATION_DOCUMENT_ROUNDTRIP"
            );
            let bound = reopened.document_bound();
            assert!(
                bound.encoded
                    >= serde_json::to_vec(reopened.document().as_ref())
                        .unwrap()
                        .len() as u64,
                "OPENED_OPERATION_ENCODED_BOUND"
            );
            assert!(
                bound.decoded
                    >= kinewright_core::undo::document_capacity_bytes(reopened.document().as_ref())
                        .unwrap(),
                "OPENED_OPERATION_CAPACITY_BOUND"
            );
            let mut seen = SEEN.get_or_init(Default::default).lock().unwrap();
            if !seen.contains(&name) {
                // Captured by libtest, so it cannot split a result line. Read the
                // markers with `-- --show-output`.
                println!("HISTORY_OPERATION_ORACLE={name};");
                seen.insert(name);
            }
        }
        result
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct OperationCase {
    pub document: Document,
    pub operation: Operation,
}

pub(super) fn operation_cases() -> std::collections::BTreeMap<String, OperationCase> {
    let cases: std::collections::BTreeMap<String, OperationCase> =
        serde_json::from_str(include_str!("history_operation_cases.json")).unwrap();
    let schema = serde_json::to_value(schemars::schema_for!(Operation)).unwrap();
    let expected: std::collections::BTreeSet<_> = schema["oneOf"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|variant| variant["properties"].as_object().unwrap().keys().cloned())
        .collect();
    assert_eq!(expected.len(), 67, "OPERATION_KIND_COUNT");
    assert_eq!(
        cases
            .keys()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>(),
        expected,
        "EVERY_OPERATION_REQUIRES_AN_ORACLE_CASE"
    );
    cases
}
