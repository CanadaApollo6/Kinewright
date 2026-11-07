#!/usr/bin/env python3
"""TE1.4 fix 2 behavioral faults, named REDs and byte-exact restored GREENs."""
from pathlib import Path
import importlib.util
import os
import sys
sys.dont_write_bytecode=True
ROOT=Path(__file__).resolve().parent.parent
spec=importlib.util.spec_from_file_location('te14_fix2_harness',ROOT/'scripts/te13-merge-mutations.py')
harness=importlib.util.module_from_spec(spec);spec.loader.exec_module(harness)
harness.OUT=Path(os.environ.get('KINEWRIGHT_TE14_FIX2_OUT',ROOT/'target/te1-4/fix2/pairs'))
harness.PAIRS=[]
C='crates/kinewright-agent/src/writer_host/task_context.rs'
I='crates/kinewright-agent/src/writer_host/codex_instructions.rs'
H='writer_host::hooks_tests::tests::'
def add(name,before,after,witness,marker,source=C):
 harness.PAIRS.append((name,source,before,after,'kinewright-agent',H+witness,marker))
add('verification-phase','if verified.load(Ordering::Acquire) {','if !verified.load(Ordering::Acquire) {','te14_fix2_verification_wait_retry_then_sample','TE14_FIX2_MEDIA_WORDS')
add('decode-phase','verified.store(true, std::sync::atomic::Ordering::Release);','verified.store(false, std::sync::atomic::Ordering::Release);','te14_fix2_decode_overrun_is_not_retryable','TE14_FIX2_MEDIA_WORDS')
add('busy-retry','.map_err(|_| refused(MEDIA_RETRY))?;','.map_err(|_| refused(MEDIA_DECODE_TIMEOUT))?;','te14_fix2_verification_wait_retry_then_sample','TE14_FIX2_BUSY_RETRY')
add('wait-deadline','if std::time::Instant::now() >= deadline {','if false && std::time::Instant::now() >= deadline {','te14_fix2_verification_wait_retry_then_sample','TE14_FIX2_MEDIA_WAIT_ENDS')
for action in ('stop','pause'):
 add(action+'-wait','!endpoint.media_read_live() || !alive.load(Ordering::Acquire)','false && (!endpoint.media_read_live() || !alive.load(Ordering::Acquire))','te14_fix2_'+action+'_ends_wait','TE14_FIX2_MEDIA_WAIT_ENDS')
s=(ROOT/C).read_text();start=s.index('pub(super) fn read_child_context');a=s.index('        for (kind, references) in [',start);b=s.index('        for (kind, reference) in [',a);end=s.index('        for chunk in bytes_rows(&serde_json::to_vec(&usage)?)',b)
add('rejection-first',s[a:end],s[b:end]+s[a:b],'te14_fix2_rejection_precedes_bulk_records','TE14_FIX2_PROOF_FIRST')
a=s.index('            return Ok(vec![',s.index('fn child_record_rows'));b=s.index('\n        }',a)
add('oversized-poison',s[a:b],'            return Err(record_too_large());','te14_fix2_oversized_record_does_not_poison_child','TE14_FIX2_OVERSIZE_CONTINUES')
add('omitted-acceptance','"cannot_accept_from_this_read"','"unverified"','te14_fix2_oversized_record_does_not_poison_child','TE14_FIX2_OMITTED_PROOF_UNKNOWN')
add('omitted-partial','rows.iter().any(|row| row["omitted"] == true)','false','te14_fix2_oversized_record_does_not_poison_child','TE14_FIX2_PARTIAL_OMISSION')
add('record-limit-words','Record exceeds the 192 KiB single-record read limit; contents unavailable on this route.','Evidence capacity exhausted; replan without guessing.','te14_fix2_oversized_record_does_not_poison_child','TE14_FIX2_RECORD_LIMIT_WORDS')
add('reference-bytes','return Ok(self.owner()?.store()?.read_evidence(reference)?.to_vec());','return Ok(serde_json::to_vec(&self.read_task_reference(reference)?)?);','te14_fix2_all_reference_routes_preserve_bytes_or_refuse','TE14_FIX2_ORIGINAL_REFERENCE_BYTES')
add('classified-duplicates','unique_classified_object(&bytes)?','serde_json::from_slice(&bytes)?','te14_fix2_all_reference_routes_preserve_bytes_or_refuse','TE14_FIX2_AMBIGUOUS_CLASSIFICATION')
add('classification-fields','unique_classified_object(&bytes)?','serde_json::from_slice(&bytes)?','te14_fix2_duplicate_classification_fields_are_ambiguous','TE14_FIX2_DUPLICATE_CLASSIFICATION')
add('proof-classification','if value["kind"] != "task_proof_v1"','if false && value["kind"] != "task_proof_v1"','te14_fix2_classification_is_host_owned','TE14_CLASSIFICATION_IS_HOST_OWNED')
add('preview-classification','EvidenceKind::Preview { .. } if value["kind"] != "task_preview_v1"','EvidenceKind::Preview { .. } if false && value["kind"] != "task_preview_v1"','te14_fix2_classification_is_host_owned','TE14_CLASSIFICATION_IS_HOST_OWNED')
add('trim-note-count','json!(result["expected_note_ids"].as_array().map_or(0, Vec::len))','json!(0)','te14_fix2_many_notes_reach_wake_trim','TE14_FIX2_TRIM_NOTE_COUNT')
add('retry-instructions','verification still running is retryable shortly','verification is never retryable','te14_fix2_retry_and_budget_are_named_in_provider_text','TE14_FIX2_INSTRUCTIONS_RETRY',I)
add('worker-failure-words', '.join()\n                    .map_err(|_| media_failed())?\n                    .map_err(|_| media_failed());', '.join()\n                    .map_err(|_| media_failed())?\n                    .map_err(|_| media_unavailable());', 'te14_fix2_decode_failure_does_not_claim_task_was_cancelled','TE14_FIX2_WORKER_FAILURE_WORDS')
add('artifact-budget-coverage', 'json!(value["omitted"] == true || rows.iter().any(|row| row["omitted"] == true))', 'json!(rows.iter().any(|row| row["omitted"] == true))', 'te14_fix2_many_artifacts_remain_unknown_at_budget', 'TE14_FIX2_UNREAD_ARTIFACTS_UNKNOWN')
# Select nearby retained witnesses; the complete set is reserved for stage close.
for script,names in [
 ('te14-mutations.py',{'sibling-detail','stale-revision','child-source','page-limit','expansion-limit','durable-expansions','capacity-replan','golden-prefix','media-owner-lock','media-revision-recheck','cursor-data-binding'}),
 ('te14-fix1-mutations.py',{'artifact-contents','evidence-contents','retry-write','retry-at-eight','cursor-audit','durable-page-binding','reference-usage','owned-record-bytes','cold-cert-owner','cold-switch-owner','decode-owner','decode-switch-owner','media-pin-release'})
]:
 spec=importlib.util.spec_from_file_location('te14_fix2_nearby_'+script.replace('-','_'),ROOT/'scripts'/script)
 registry=importlib.util.module_from_spec(spec);spec.loader.exec_module(registry)
 selected=[row for row in registry.harness.PAIRS if row[0] in names]
 assert len(selected)==len(names),(script,names-{row[0] for row in selected})
 harness.PAIRS.extend(('near-'+row[0],*row[1:]) for row in selected)
if __name__=='__main__':harness.main()
