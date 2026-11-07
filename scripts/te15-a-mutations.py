#!/usr/bin/env python3
"""TE1.5 Part A named faults with exact restored bytes."""
from pathlib import Path
import importlib.util
import os
import sys
sys.dont_write_bytecode=True
ROOT=Path(__file__).resolve().parent.parent
spec=importlib.util.spec_from_file_location('te15_base',ROOT/'scripts/te13-merge-mutations.py')
h=importlib.util.module_from_spec(spec);spec.loader.exec_module(h)
h.OUT=Path(os.environ.get('KINEWRIGHT_TE15_A_OUT',ROOT/'target/te1-5/a/pairs'))
h.PAIRS=[]
C='crates/kinewright-agent/src/writer_host/task_context.rs'
H='writer_host::hooks_tests::tests::'
def add(name,before,after,witness,marker):
 h.PAIRS.append((name,C,before,after,'kinewright-agent',H+witness,marker))
add('phase-tail','if now.duration_since(started) >= wait','if true || now.duration_since(started) >= wait','te15_a_certification_tail_is_retryable','TE15_PHASE_TAIL_RETRY')
add('finished-deadline','if worker.is_finished() {','if worker.is_finished() && std::time::Instant::now() < deadline {','te15_a_finished_worker_wins_over_deadline','TE15_FINISHED_AT_DEADLINE')
add('begin-liveness','MediaReadStartError::Unavailable => media_unavailable()','MediaReadStartError::Unavailable => refused(MEDIA_RETRY)','te15_a_begin_media_read_distinguishes_liveness_from_busy','TE15_BEGIN_LIVENESS')
add('transcript-duplicates','if value.is_object() {','if false && value.is_object() {','te15_a_transcript_duplicate_words_are_refused_on_wire','TE15_TRANSCRIPT_DUPLICATES')
add('nested-revision','input.deserialize_any(UniqueVisitor).map(Self)','Value::deserialize(input).map(Self)','te15_a_nested_revision_duplicates_are_refused_on_wire','TE15_NESTED_REVISION_DUPLICATES')
add('empty-record','if bytes.is_empty() {','if false && bytes.is_empty() {','te15_a_empty_record_is_present_on_wire','TE15_EMPTY_CHILD_PRESENT')
# Nearby retained guards only. The complete retained collection is a Part B gate.
spec=importlib.util.spec_from_file_location('te15_nearby',ROOT/'scripts/te14-fix2-mutations.py')
registry=importlib.util.module_from_spec(spec);spec.loader.exec_module(registry)
names={'verification-phase','decode-phase','busy-retry','wait-deadline','stop-wait','pause-wait','omitted-partial','omitted-acceptance','reference-bytes','classified-duplicates','proof-classification','preview-classification','near-golden-prefix','near-reference-usage','near-owned-record-bytes','near-cursor-data-binding'}
selected=[row for row in registry.harness.PAIRS if row[0] in names]
assert len(selected)==len(names)
h.PAIRS.extend(('retained-'+row[0],*row[1:]) for row in selected)
if __name__=='__main__':h.main()
