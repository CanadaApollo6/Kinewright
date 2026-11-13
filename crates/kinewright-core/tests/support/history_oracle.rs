#![allow(
    dead_code,
    reason = "each fixture binary selects part of the shared oracle support"
)]
// Shared test-only document-bearing oracle. Unit fixtures include this module
// with `use crate as kinewright_core`; integration fixtures use the real crate.
use kinewright_core::{Command, Core, Document, Event, HistorySnapshot, OpError, Operation};
use serde::Serialize;
#[derive(Serialize)]
struct BoundaryOracle<'a> {
    floor_offset: u64,
    baseline: &'a Document,
    document: &'a Document,
    batches: &'a [kinewright_core::HistoryBatch],
    cursor: &'a kinewright_core::HistoryCursor,
}
pub(super) fn document_oracle(history: &HistorySnapshot) -> serde_json::Value {
    let mut value = serde_json::to_value(BoundaryOracle {
        floor_offset: history.floor_offset,
        baseline: &history.baseline,
        document: &history.document,
        batches: &history.batches,
        cursor: &history.cursor,
    })
    .unwrap();
    // The old format also stored every surviving clip ID. Reconstruct that
    // redundant field independently for the test-only legacy value.
    let mut current: std::collections::BTreeSet<_> = history
        .baseline
        .tracks
        .iter()
        .flat_map(|track| track.clips.iter().map(|clip| clip.id))
        .collect();
    let mut undo = Vec::new();
    let mut redo = Vec::new();
    for (batch, encoded) in history
        .batches
        .iter()
        .zip(value["batches"].as_array_mut().unwrap())
    {
        match &batch.command {
            kinewright_core::JournalCommand::Undo => {
                if let Some(previous) = undo.pop() {
                    redo.push(current);
                    current = previous;
                }
            }
            kinewright_core::JournalCommand::Redo => {
                if let Some(next) = redo.pop() {
                    undo.push(current);
                    current = next;
                }
            }
            _ => {
                let before = current.clone();
                for (effect, encoded) in batch
                    .identity_effects
                    .iter()
                    .zip(encoded["identity_effects"].as_array_mut().unwrap())
                {
                    let retained: Vec<_> = current
                        .iter()
                        .filter(|id| !effect.deleted.contains(id))
                        .copied()
                        .collect();
                    encoded["retained"] = serde_json::to_value(retained).unwrap();
                    for id in &effect.deleted {
                        current.remove(id);
                    }
                    current.extend(&effect.created);
                }
                if batch.undo_entries > undo.len() {
                    undo.push(before);
                }
                redo.clear();
            }
        }
    }
    value
}

pub(super) fn assert_compact_roundtrip(original: &HistorySnapshot) {
    let bytes = serde_json::to_vec(original).unwrap();
    let restored: HistorySnapshot = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(document_oracle(&restored), document_oracle(original));
    assert_eq!(&restored, original);
    assert_eq!(restored.applied_operations(), original.applied_operations());
}

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
            let core = Core::spawn(before).unwrap();
            assert!(matches!(
                core.request(Command::Do(self.clone())).unwrap(),
                Event::DocumentChanged { .. }
            ));
            let history = core.export_history().unwrap();
            assert_eq!(&*history.document, document);
            assert_compact_roundtrip(&history);
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
