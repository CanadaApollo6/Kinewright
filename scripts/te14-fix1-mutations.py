#!/usr/bin/env python3
"""TE1.4 fix 1: real guard mutations, exact restores and named assertions."""
from pathlib import Path
import importlib.util
import os
import sys
sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parent.parent
spec = importlib.util.spec_from_file_location('te14_fix1_harness', ROOT / 'scripts/te13-merge-mutations.py')
harness = importlib.util.module_from_spec(spec)
spec.loader.exec_module(harness)
harness.OUT = Path(os.environ.get('KINEWRIGHT_TE14_FIX1_OUT', ROOT / 'target/te1-4/fix1/pairs'))
harness.PAIRS = []
C = 'crates/kinewright-agent/src/writer_host/task_context.rs'
M = 'crates/kinewright-agent/src/writer_host/task_mcp.rs'
P = 'crates/kinewright-agent/src/writer_host/codex_prompt.rs'
U = 'crates/kinewright-agent/src/writer_host/provider_usage.rs'
H = 'writer_host::hooks_tests::tests::'
def add(name, source, before, after, witness, marker):
    harness.PAIRS.append((name, source, before, after, 'kinewright-agent', H + witness, marker))
add('original-notes', C, 'note["text"] = self.prompt_note_text(&reference)?;', 'let _ = self.prompt_note_text(&reference)?;',
    'te14_fix1_six_original_notes_are_inline_once', 'TE14_FIX1_NOTE_TEXT_ONCE')
add('note-capacity', C, 'if reference.byte_length > (crate::protocol::limits::FRAME_BYTES / 2) as u64 {',
    'if false && reference.byte_length > (crate::protocol::limits::FRAME_BYTES / 2) as u64 {',
    'te14_fix1_note_capacity_is_unchanged', 'TE14_FIX1_NOTE_CAPACITY')
add('artifact-contents', C, '("artifact", &result.artifacts)', '("artifact", &result.evidence)',
    'te14_fix1_artifact_failure_matches_full_and_expanded_wire', 'TE14_FIX1_ARTIFACT_REJECTION')
add('evidence-contents', C, '("evidence", &result.evidence)', '("evidence", &result.artifacts)',
    'te14_fix1_artifact_failure_matches_full_and_expanded_wire', 'TE14_FIX1_EVIDENCE_CONTENTS')
add('untrusted-pass', C, '"partial_coverage":true,"note_fate":"unknown","proof_status":"unknown"',
    '"partial_coverage":true,"note_fate":"unknown","proof_status":"reported_pass"',
    'te14_fix1_shaped_explanation_cannot_certify_a_pass', 'TE14_FIX1_UNTRUSTED_PASS_UNKNOWN')
add('wake-capacity', C, 'let full: Value = serde_json::from_slice(&bytes)?;',
    'if bytes.len() > PAGE_BYTES / 2 { return Err(capacity()); }\n            let full: Value = serde_json::from_slice(&bytes)?;',
    'te14_fix1_many_results_always_deliver_a_bounded_wake', 'TE14_FIX1_WAKE_DELIVERED')
add('retry-write', C, 'if audit["kind"] == "task_evidence_expansion_v1" && audit["page_id"] == page_id {',
    'if false && audit["kind"] == "task_evidence_expansion_v1" && audit["page_id"] == page_id {',
    'te14_fix1_lost_page_reply_retries_without_a_write', 'TE14_FIX1_RETRY_NO_WRITE')
add('retry-at-eight', C, 'if audit["kind"] == "task_evidence_expansion_v1" && audit["page_id"] == page_id {',
    'if self.expansion_count(task)? < EXPANSIONS && audit["kind"] == "task_evidence_expansion_v1" && audit["page_id"] == page_id {',
    'te14_page_expansion_limit_and_reconnect_are_durable', 'TE14_FIX1_RETRY_AT_CAPACITY_FREE')
add('cursor-audit', C, 'serde_json::to_vec(&(binding,page_id))?', 'serde_json::to_vec(binding)?',
    'te14_fix1_lost_page_reply_retries_without_a_write', 'TE14_FIX1_AUDIT_BINDS_CURSOR')
add('cursor-page-identity', C, 'format!("{identity}:{offset}")', 'format!("{identity}:0")',
    'te14_fix1_lost_page_reply_retries_without_a_write', 'TE14_FIX1_CURSOR_IDENTITY')
add('cold-cert-owner', C, 'let source = lease.certified_source()?;',
    'let _cold = store.lease_input(&OwnedInputRef { manifest_ref:task.brief.inputs.clone(),entry:entry.clone() })?;\n        let source = lease.certified_source()?;',
    'te15_a_fix_retained_certificate_survives_memo_eviction', 'assertion failed: !unexpected.load(Ordering::Acquire)')
add('decode-owner', M, 'drop(host);\n                            match prepared.render(&self.endpoint, &owner) {',
    'let _held_host = host;\n                            match prepared.render(&self.endpoint, &owner) {',
    'te14_fix1_decode_stop', 'TE14_FIX1_MEDIA_CONTROL_NOT_BLOCKED')
# Retired cold-switch-owner: Reads use the pre-certified source; Switch retains its separate decode/cancellation witness. See te14-fix1/cold-cert-owner and decode-switch-owner.
add('decode-switch-owner', M, 'drop(host);\n                            match prepared.render(&self.endpoint, &owner) {',
    'let _held_host = host;\n                            match prepared.render(&self.endpoint, &owner) {',
    'te14_fix1_decode_switch', 'TE14_FIX1_MEDIA_CONTROL_NOT_BLOCKED')
add('media-liveness', C, '!endpoint.media_read_live() || !self.alive.load(std::sync::atomic::Ordering::Acquire)',
    'false && (!endpoint.media_read_live() || !self.alive.load(std::sync::atomic::Ordering::Acquire))',
    'te14_fix1_decode_stop', 'TE14_FIX1_MEDIA_CANCEL_OR_DEADLINE')
add('decoder-cancel', C, 'let signal = Arc::clone(&work.cancel);',
    'let signal = Arc::new(std::sync::atomic::AtomicBool::new(false));',
    'te14_fix1_decode_stop', 'TE14_FIX1_DECODER_CONTINUES_OR_CANCELS')
add('media-deadline', C, 'if !started || std::time::Instant::now() >= self.deadline {',
    'if !started || (false && std::time::Instant::now() >= self.deadline) {',
    'te14_fix1_media_wall_clock_limit', 'TE14_FIX1_MEDIA_CANCEL_OR_DEADLINE')
add('child-proof-source', C, '1 => Ok(sources.pop().unwrap()),', '1 => Ok(json!(current)),',
    'te14_fix1_child_proof_uses_durable_source_not_parent_clock', 'TE14_FIX1_CHILD_PROOF_SOURCE')
add('missing-proof-revision', C, ') && value.get("revision")', ') && value.get("revision").is_some() && value.get("revision")',
    'te14_fix1_child_proof_uses_durable_source_not_parent_clock', 'TE14_FIX1_CHILD_PROOF_SOURCE')
add('reference-usage', M, '**accounting = super::provider_usage::provider_accounting_view(accounting)?;',
    'let _ = super::provider_usage::provider_accounting_view(accounting)?;',
    'te14_fix1_usage_reads_hide_host_observers', 'TE14_FIX1_REFERENCE_USAGE_PROJECTED')
add('legacy-limit-words', C,
    'read_task_result exceeds its one-shot 20 KiB limit; use read_child_result for paged child records',
    'Evidence capacity exhausted; replan without guessing.',
    'te14_fix1_legacy_limit_names_the_paged_route', 'TE14_FIX1_LEGACY_LIMIT_TRUE')
add('durable-page-binding', C, 'let binding = json!({"run":state.run,"task":task.id,"source":task.brief.scope.base,"document_sha256":kinewright_media::sha256_bytes(&serde_json::to_vec(&*document)?),"evidence":read.evidence});\n        let rows = match &read.evidence {', 'let binding = json!({"run":state.run,"task":task.id,"revision":revision,"document_sha256":kinewright_media::sha256_bytes(&serde_json::to_vec(&*document)?),"evidence":read.evidence});\n        let rows = match &read.evidence {',
    'te14_fix1_retry_after_host_restart_is_free', 'TE14_FIX1_RESTART_REPLY_IDENTICAL')
# Retired media-pin-release: The temporary input-lease request whose hold was leaked is no longer opened by this read path. See te14-fix1/cold-cert-owner.
add('owned-record-bytes', C, 'return Ok(self.owner()?.store()?.read_evidence(reference)?.to_vec());',
    'return Ok(serde_json::to_vec(&self.read_task_reference(reference)?)?);',
    'te14_fix1_child_expansion_preserves_original_owned_bytes', 'TE14_FIX1_RAW_RECORD_EXACT')
harness.PAIRS.append(('media-context-cancel', C,
    'if cancel.is_some_and(|flag| flag.load(std::sync::atomic::Ordering::Acquire)) {',
    'if false && cancel.is_some_and(|flag| flag.load(std::sync::atomic::Ordering::Acquire)) {',
    'kinewright-agent', 'writer_host::task_context::tests::te14_context_materialization_observes_cancellation', 'TE14_FIX1_CONTEXT_CANCELLED'))
add('full-route-reference', C, '"result_refs":routes', '"result_refs":[]',
    'te14_fix1_artifact_failure_matches_full_and_expanded_wire', 'TE14_FIX1_FULL_ROUTE')
# The accounting projection moved intact to a platform-neutral module. Rerun
# just its six retained byte-surface witnesses, without executing the complete
# TE1 accounting mutation driver at import time.
registry = {'__file__': str(ROOT / 'scripts/te1-mutations.py'), '__name__': 'te14_nearby_registry'}
exec(compile((ROOT / 'scripts/te1-mutations.py').read_text().split('proof = []')[0], str(ROOT / 'scripts/te1-mutations.py'), 'exec'), registry)
for name, source, before, after, target, witness in registry['mutations']:
    if name in {'wake-events', 'wake-snapshots', 'full-wake-reference', 'full-wake-revision', 'full-wake-totals', 'full-wake-kind'}:
        assert target == ['--lib']
        marker = 'TE1_WAKE_ACCOUNTING_SURFACE' if name in {'wake-events', 'wake-snapshots'} else 'TE14_LEGACY_FULL_RECORD_BYTES'
        harness.PAIRS.append((name, source, before, after, 'kinewright-agent', witness, marker))
if __name__ == '__main__':
    harness.main()
