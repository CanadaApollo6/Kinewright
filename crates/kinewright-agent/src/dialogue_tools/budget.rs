//! One wire budget, including dictionaries, provenance and the MCP envelope.
use rmcp::model::CallToolResult;
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};

pub(crate) const FLAGS_BYTES: usize = 4_096;
pub(crate) const ALL_BYTES: usize = 32_768;

#[derive(Clone, Copy)]
enum Candidate {
    Item(usize, usize),
    NotChecked(usize),
    OtherAudio(usize),
}
#[derive(Default, Clone)]
struct Selection {
    items: BTreeMap<usize, Vec<usize>>,
    not_checked: Vec<usize>,
    other_audio: Vec<usize>,
}

fn list(value: &Value) -> &[Value] {
    value.as_array().map_or(&[], Vec::as_slice)
}

fn rank(item: &Value) -> usize {
    match item["kind"].as_str().expect("serialized audit item kind") {
        "join" => {
            let sides = list(&item["sides"]);
            if sides.iter().any(|s| !s["edge_in_sound"].is_null()) {
                0
            } else if sides.iter().any(|s| {
                s["short_air"] == true
                    || !s["dip_intrusion"].is_null()
                    || s.get("padded_us").is_some_and(|p| p != &json!([0, 1]))
            }) || item["room_tone"]["flagged"] == true
            {
                1
            } else {
                3
            }
        }
        "hole" if item["sound_us"] == 0 => 3,
        _ => 1,
    }
}

// Inspect only generated audit JSON. Conservative extra dictionary entries are
// harmless; every referenced recording/word is retained with its item.
fn references(value: &Value, ids: &mut BTreeSet<u64>, words: &mut BTreeSet<(u64, u64)>) {
    match value {
        Value::Array(a) => {
            if a.len() == 2
                && let Some(id) = a[0].as_u64()
            {
                ids.insert(id);
                if let Some(index) = a[1].as_u64() {
                    words.insert((id, index));
                }
            }
            for v in a {
                references(v, ids, words);
            }
        }
        Value::Object(o) => {
            if let Some(id) = o.get("recording").and_then(Value::as_u64) {
                ids.insert(id);
            }
            if let (Some(id), Some(index)) = (
                o.get("source").and_then(|s| s[0].as_u64()),
                o.get("dip_intrusion").and_then(|d| d["word"].as_u64()),
            ) {
                ids.insert(id);
                words.insert((id, index));
            }
            for v in o.values() {
                references(v, ids, words);
            }
        }
        _ => {}
    }
}
fn add_counts(total: &mut Map<String, Value>, counts: &Value) {
    for (name, n) in counts.as_object().expect("serialized audit counts") {
        let n = n.as_u64().expect("unsigned audit count");
        if n != 0 {
            let sum = total.get(name).and_then(Value::as_u64).unwrap_or(0) + n;
            total.insert(name.clone(), Value::from(sum));
        }
    }
}
struct Source {
    envelope: Map<String, Value>,
    check: Value,
    totals: Map<String, Value>,
    core_omitted: Map<String, Value>,
    classes: [usize; 4],
    word_names: usize,
    hole_groups: usize,
}
impl Source {
    fn new(mut value: Value) -> Self {
        let envelope = value.as_object_mut().expect("check envelope");
        let check = envelope.remove("check").expect("check result");
        let mut totals = Map::new();
        let mut core_omitted = Map::new();
        let mut classes = [0; 4];
        let mut word_names = 0;
        let mut hole_groups = 0;
        for segment in list(&check["segments"]) {
            let report = &segment["report"];
            add_counts(&mut totals, &report["total"]);
            add_counts(&mut core_omitted, &report["omitted"]);
            word_names += list(&report["words"]).len();
            hole_groups += list(&report["hole_groups"]).len();
            for item in list(&report["items"]) {
                classes[rank(item)] += 1;
            }
        }
        Self {
            envelope: std::mem::take(envelope),
            check,
            totals,
            core_omitted,
            classes,
            word_names,
            hole_groups,
        }
    }
    fn render(&self, selected: &Selection, limit: usize) -> Value {
        let original_segments = list(&self.check["segments"]);
        let original_records = self.check["recordings"]
            .as_object()
            .expect("recording dictionary");
        let mut ids = BTreeSet::new();
        let mut segments = Vec::new();
        let mut classes = [0; 4];
        let mut words_kept = 0;
        let mut groups_kept = 0;
        for (&segment, indexes) in &selected.items {
            let original = &original_segments[segment]["report"];
            let mut report: Map<_, _> = original
                .as_object()
                .expect("audit report")
                .iter()
                .filter(|(k, _)| !matches!(k.as_str(), "items" | "words" | "hole_groups"))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            let mut items = Vec::new();
            let mut wanted = BTreeSet::new();
            let mut group_ids = BTreeMap::new();
            let mut groups = Vec::new();
            for &index in indexes {
                // These are bounded wire items, never document/profile copies.
                let mut item = list(&original["items"])[index].clone();
                classes[rank(&item)] += 1;
                references(&item, &mut ids, &mut wanted);
                if item["kind"] == "hole"
                    && let Some(g) = item["group"].as_u64()
                {
                    let mapped = *group_ids.entry(g).or_insert_with(|| {
                        groups.push(
                            list(&original["hole_groups"])[usize::try_from(g).expect("hole index")]
                                .clone(),
                        );
                        groups.len() - 1
                    });
                    item["group"] = json!(mapped);
                }
                items.push(item);
            }
            let names: Vec<_> = list(&original["words"])
                .iter()
                .filter(|w| {
                    wanted.contains(&(
                        w["key"][0].as_u64().expect("recording id"),
                        w["key"][1].as_u64().expect("word index"),
                    ))
                })
                .cloned()
                .collect();
            words_kept += names.len();
            groups_kept += groups.len();
            if !names.is_empty() {
                report.insert("words".into(), Value::Array(names));
            }
            if !groups.is_empty() {
                report.insert("hole_groups".into(), Value::Array(groups));
            }
            report.insert("items".into(), Value::Array(items));
            segments.push(
                json!({"start_frame":original_segments[segment]["start_frame"],"report":report}),
            );
        }
        let recordings: Map<_, _> = original_records
            .iter()
            .filter(|(key, _)| key.parse::<u64>().is_ok_and(|id| ids.contains(&id)))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        let omitted = json!({
            "acoustic":self.classes[0]-classes[0],"warnings":self.classes[1]-classes[1],
            "information":self.classes[3]-classes[3],
            "not_checked":list(&self.check["not_checked"]).len()-selected.not_checked.len(),
            "recordings":original_records.len()-recordings.len(),
            "segments":original_segments.len()-segments.len(),
            "word_names":self.word_names-words_kept,"hole_groups":self.hole_groups-groups_kept,
            "other_audio":list(&self.check["other_audio"]).len()-selected.other_audio.len()
                + usize::try_from(self.check["other_audio_omitted"].as_u64().unwrap_or(0)).expect("clip count")
        });
        let mut envelope = self.envelope.clone();
        envelope.insert("check".into(),json!({
            "segments":segments,"recordings":recordings,
            "not_checked":selected.not_checked.iter().map(|&i|&list(&self.check["not_checked"])[i]).collect::<Vec<_>>(),
            "other_audio":selected.other_audio.iter().map(|&i|&list(&self.check["other_audio"])[i]).collect::<Vec<_>>(),
            "total":self.totals,"core_omitted":self.core_omitted,"omitted":omitted,
            "budget_bytes":limit,
            "more":"Narrow range:[start_frame,end_frame] to see omitted evidence. detail:all includes information with a 32768-byte reply budget."
        }));
        Value::Object(envelope)
    }
}

pub(crate) fn bound(result: &mut CallToolResult, all: bool) {
    let limit = if all { ALL_BYTES } else { FLAGS_BYTES };
    if serde_json::to_vec(result).expect("JSON check reply").len() <= limit {
        return;
    }
    let source = Source::new(
        result
            .structured_content
            .take()
            .expect("structured check reply"),
    );
    let mut candidates = Vec::new();
    for (si, segment) in list(&source.check["segments"]).iter().enumerate() {
        for (ii, item) in list(&segment["report"]["items"]).iter().enumerate() {
            candidates.push((rank(item), si, ii, Candidate::Item(si, ii)));
        }
    }
    for i in 0..list(&source.check["not_checked"]).len() {
        candidates.push((2, i, 0, Candidate::NotChecked(i)));
    }
    for i in 0..list(&source.check["other_audio"]).len() {
        candidates.push((4, i, 0, Candidate::OtherAudio(i)));
    }
    candidates.sort_by_key(|&(rank, segment, item, _)| (rank, segment, item));
    let mut selected = Selection::default();
    result.structured_content = Some(source.render(&selected, limit));
    for (_, _, _, candidate) in candidates {
        // Speculate only on small selection indexes and the bounded wire reply.
        let mut next = selected.clone();
        match candidate {
            Candidate::Item(segment, item) => next.items.entry(segment).or_default().push(item),
            Candidate::NotChecked(i) => next.not_checked.push(i),
            Candidate::OtherAudio(i) => next.other_audio.push(i),
        }
        let mut trial = result.clone();
        trial.structured_content = Some(source.render(&next, limit));
        if serde_json::to_vec(&trial).expect("JSON check reply").len() <= limit {
            *result = trial;
            selected = next;
        }
    }
    assert!(serde_json::to_vec(result).expect("JSON check reply").len() <= limit);
}
