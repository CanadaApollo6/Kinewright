#!/usr/bin/env python3
"""Apply one named C1 mutation, retaining exact originals for restoration."""
import hashlib
import json
from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parent.parent
PROJECT = "crates/kinewright-project/src/"


def replace(text, old, new):
    if text.count(old) != 1:
        raise SystemExit(f"Mutation anchor count {text.count(old)}: {old[:90]!r}")
    return text.replace(old, new, 1)


def mutation(case):
    changes = {}

    def change(file, old, new):
        path = PROJECT + file
        text = changes.get(path, (ROOT / path).read_text())
        changes[path] = replace(text, old, new)
    if case in ('phase', 'fixture-phase'):
        change('bundle_commit.rs', '                CommitOutcome::CommittedWithError(error)\n', '                CommitOutcome::DefinitelyUnpublished(error)\n')
    elif case == 'copy-phase':
        change('bundle_copy.rs', 'CopyStatus::CommittedWithError {\n                    diagnostic: error.to_string(),\n                }', 'CopyStatus::DurabilityUnknown')
    elif case == 'committed-proof':
        change('bundle_copy.rs', '        if matches!(self.status, CopyStatus::CommittedWithError { .. })\n            && actual == self.before.root', '        if false && matches!(self.status, CopyStatus::CommittedWithError { .. })\n            && actual == self.before.root')
    elif case == 'rename':
        change('namespace/linux.rs', '                namespace.pending_root_cleanup = Some(name.clone());\n', '')
    elif case == 'named-copy':
        change('bundle_copy.rs', '        let cleanup_only = self.cleanup_pending()', '        let cleanup_only = false && self.cleanup_pending()')
    elif case == 'charge':
        change('store.rs', '                    .pending_controls\n                    .push(reservation);', '                    .pending_controls\n                    .extend(std::iter::empty());\n                drop(reservation);')
    elif case in ('wording', 'fixture-cleanup'):
        change('bundle_copy.rs', 'CopyStatus::CleanupPending { diagnostic }', '{ let _ = diagnostic; CopyStatus::DurabilityUnknown }')
    elif case == 'owned-input':
        change('bundle_copy.rs', '        entries: original.entries.clone(),', '        entries: vec![],')
    elif case == 'unknown-phase':
        change('bundle_commit.rs', '                bundle.publication_state = crate::bundle::PublicationState::Unknown;\n', '')
    elif case == 'receipt-validation':
        change('bundle_recovery.rs', '    let result = publication::validate(namespace, bytes, limits, memo);', '    let result: Result<(), BundleError> = { let _ = (namespace, bytes, limits, memo); Ok(()) };')
    elif case in ('encoded-cost', 'memo-stale-cost', 'memo-wrong-ref', 'no-preflight'):
        if case == 'memo-stale-cost':
            change('bundle_recovery/publication.rs', '            let decoded = cost.decoded;', '            let decoded = self.records.iter().filter(|(r, _)| r.record_kind == reference.record_kind).map(|(_, c)| c.decoded).min().unwrap_or(cost.decoded);')
        elif case == 'memo-wrong-ref':
            change('bundle_recovery/publication.rs', 'if let Some(cost) = self.records.get(reference)', 'if let Some(cost) = self.records.iter().find(|(r, _)| r.sha256 == reference.sha256).map(|(_, c)| c)')
            change('bundle_recovery/publication.rs', '.retain(|reference, _| references.contains(reference));', '.retain(|reference, _| references.iter().any(|r| r.sha256 == reference.sha256));')
        elif case == 'encoded-cost':
            change('bundle_recovery/publication.rs', '            let decoded = cost.decoded;', '            let decoded = reference.byte_length;')
        else:
            change('bundle_recovery/publication.rs', '        limits.decoded_record_bytes,', '        0,')
    elif case == 'memo-invalidation':
        change('store.rs', '            self.clear_verified_memo()?;', '            // Mutation: retain identity costs after lost namespace continuity.')
    else:
        raise SystemExit(f'Unknown mutation {case}')
    return changes


def apply(case, evidence):
    evidence = Path(evidence)
    if (evidence / "originals.json").exists():
        raise SystemExit("Use a fresh mutation evidence directory")
    changes = mutation(case)
    originals = {}
    for file in changes:
        raw = (ROOT / file).read_bytes()
        saved = evidence / "originals" / file
        saved.parent.mkdir(parents=True, exist_ok=True)
        saved.write_bytes(raw)
        originals[file] = hashlib.sha256(raw).hexdigest()
    (evidence / "originals.json").write_text(json.dumps(originals, indent=2) + "\n")
    for file, value in changes.items():
        (ROOT / file).write_text(value)


def restore(case, evidence):
    evidence = Path(evidence)
    originals = json.loads((evidence / "originals.json").read_text())
    for file, expected in originals.items():
        raw = (evidence / "originals" / file).read_bytes()
        if hashlib.sha256(raw).hexdigest() != expected:
            raise SystemExit("Original source hash mismatch")
        (ROOT / file).write_bytes(raw)
    for file, expected in originals.items():
        if hashlib.sha256((ROOT / file).read_bytes()).hexdigest() != expected:
            raise SystemExit("Restored source differs")


if __name__ == "__main__":
    {"apply": apply, "restore": restore}[sys.argv[1]](*sys.argv[2:])
