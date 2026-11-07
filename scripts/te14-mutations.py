#!/usr/bin/env python3
"""TE1.4 security and context witnesses with named failures and exact restores."""
from pathlib import Path
import importlib.util
import os
import sys

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parent.parent
spec = importlib.util.spec_from_file_location('te14_pair_harness', ROOT / 'scripts/te13-merge-mutations.py')
harness = importlib.util.module_from_spec(spec)
spec.loader.exec_module(harness)
harness.OUT = Path(os.environ.get('KINEWRIGHT_TE14_MUTATION_OUT', ROOT / 'target/te1-4/pairs'))
harness.PAIRS = []
A = 'kinewright-agent'
H = 'writer_host::hooks_tests::tests::'
C = 'crates/kinewright-agent/src/writer_host/task_context.rs'
P = 'crates/kinewright-agent/src/writer_host/codex_prompt.rs'
M = 'crates/kinewright-agent/src/writer_host/task_mcp.rs'
N = 'crates/kinewright-agent/src/writer_host/native_invocation.rs'

def add(name, source, before, after, witness, marker):
    harness.PAIRS.append((name, source, before, after, A, H + witness, marker))

add('dependency-closure', C, 'if linked || ripple || caption || synced || routing {',
    'if false && (linked || ripple || caption || synced || routing) {',
    'te14_scoped_context_expands_the_outside_view_dependency', 'TE14_FULL_SCOPED_SAME_REJECTION')
add('selected-track-ripple',C,'let same_track = track.id == other_track.id;','let same_track = false;',
    'te14_unlocked_selected_track_expands_ripple_and_marker_dependencies','TE14_UNLOCKED_RIPPLE_CLOSURE')
add('ripple-markers',C,'"markers":document.markers','"markers":[]',
    'te14_unlocked_selected_track_expands_ripple_and_marker_dependencies','TE14_RIPPLE_MARKER_CLOSURE')
add('neighbouring-words', C,
    'let first = start.saturating_sub(2);\n                    let last = end.saturating_add(2).min(words.len());',
    'let first = *start;\n                    let last = *end;',
    'te14_neighbouring_words_match_the_full_transcript_decision', 'TE14_NEIGHBOUR_DECISION')
add('legitimate-expansion', C,
    'let context = page(&rows,&json!({"run":run,"task":task.id,"source":task.brief.scope.base,"document_sha256":kinewright_media::sha256_bytes(&serde_json::to_vec(document)?),"evidence":{"kind":"timeline","clips":seeds}}),None,OVERVIEW_BYTES/2).ok();',
    'let context = page(&rows,&json!({"task":task.id,"revision":revision,"evidence":{"kind":"timeline","clips":seeds}}),None,OVERVIEW_BYTES/2).ok();',
    'te14_scoped_context_expands_the_outside_view_dependency', 'TE14_INITIAL_CURSOR_EXPANDS')
add('sibling-detail', C,
    '.find(|c| c.id == read.child && c.parent.as_ref() == Some(&parent.id))',
    '.find(|c| c.id == read.child)',
    'te14_child_details_bind_parent_child_run_reference_and_source', 'TE14_SIBLING_BINDING_DENIED')
add('stale-revision', C, 'if revision != read.revision {\n            return Err(refused("stale evidence revision"));\n        }\n        let binding', 'if false && revision != read.revision {\n            return Err(refused("stale evidence revision"));\n        }\n        let binding',
    'te14_stale_revision_and_cursor_are_refused', 'TE14_STALE_REVISION_DENIED')
add('child-source', C, 'if result.head != read.source {', 'if false && result.head != read.source {',
    'te14_child_details_bind_parent_child_run_reference_and_source', 'TE14_CHILD_STALE_SOURCE_DENIED')
add('page-limit', C,
    'let value = page(&rows, &binding, read.cursor.as_deref(), PAGE_BYTES - 4096)?;' + "\n" + '        self.record_expansion(\n            index,\n            state,\n            task,',
    'let value = page(&rows, &binding, read.cursor.as_deref(), PAGE_BYTES * 8)?;' + "\n" + '        self.record_expansion(\n            index,\n            state,\n            task,',
    'te14_page_expansion_limit_and_reconnect_are_durable', 'TE14_PAGE_LIMIT')
add('expansion-limit', C, 'const EXPANSIONS: usize = 8;', 'const EXPANSIONS: usize = 9;',
    'te14_page_expansion_limit_and_reconnect_are_durable', 'TE14_EIGHT_EXPANSIONS')
add('durable-expansions', C, 'decisions: vec![reference],', 'decisions: vec![],',
    'te14_page_expansion_limit_and_reconnect_are_durable', 'TE14_DURABLE_EXPANSION_COUNT')
add('capacity-replan', C, 'Evidence capacity exhausted; replan without guessing.',
    'Evidence capacity exhausted; continue with a guess.',
    'te14_page_expansion_limit_and_reconnect_are_durable', 'TE14_CAPACITY_REPLAN')
add('protocol-prefix', P,
    'let mut prefix = super::task_context::application_prefix(task.brief.role)?;',
    'let mut prefix = b"{\\\"data\\\":".to_vec();',
    'te14_fixed_prefix_is_identical_through_both_protocols', 'TE14_PROTOCOL_FIXED_PREFIX')
add('prefix-measurement', N,
    'let prefix_identity = kinewright_media::sha256_bytes(\n            &super::task_context::application_prefix(task.brief.role)?,\n        );',
    'let prefix_identity = kinewright_media::sha256_bytes(super::codex_instructions::CODEX_INSTRUCTIONS.as_bytes());',
    'te14_fixed_prefix_is_identical_through_both_protocols', 'TE14_PREFIX_MEASURED_SEPARATELY')
add('durable-merge-record', M,
    'if !state.results.contains(&merge.result)\n                || merge.target != history.base',
    'if merge.target != history.base',
    'te14_merge_uses_durable_delta_and_history_after_a_small_wake', 'TE14_SUMMARY_CANNOT_AUTHORIZE_MERGE')

# This changes a real fixed prefix byte, leaving variable task values untouched.
harness.PAIRS.append(('golden-prefix', 'crates/kinewright-agent/src/writer_host/codex_instructions.rs', 'Context is read-only.', 'Context is writable.', A,
    'writer_host::task_context::tests::te14_role_prefix_goldens_are_fixed_ordered_bytes', 'TE14_PREFIX_BYTES_STABLE'))

add('planar-audio-channels',C,'let planar = frame.format().is_planar();','let planar = false;',
    'te14_real_owned_audio_and_frame_samples_exclude_ambient_paths','TE14_PLANAR_CHANNELS_PRESERVED')
add('plain-transcript',C,'else if let Some(text) = value.as_str() {','else if let Some(text) = value.as_str().filter(|_|false) {',
    'te14_plain_text_transcript_keeps_neighbours_and_unknown_alignment','TE14_PLAIN_TEXT_TRANSCRIPT_EXPANDS')
add('media-owner-lock', M,
    'drop(host);\n                            prepared.render(&self.endpoint, &owner)?',
    'let _held_host=host;\n                            prepared.render(&self.endpoint, &owner)?',
    'te14_real_owned_audio_and_frame_samples_exclude_ambient_paths', 'TE14_DECODE_RELEASES_OWNER')
add('media-revision-recheck', C, 'if revision != read.revision\n            || binding != rendered.binding',
    'if false && revision != read.revision\n            || false && binding != rendered.binding',
    'te14_real_owned_audio_and_frame_samples_exclude_ambient_paths', 'TE14_DECODE_RECHECKS_REVISION')
harness.PAIRS.extend([
 ('frame-padding',C,'output.extend_from_slice(&row[..count]);','output.extend_from_slice(&row[..stride.min(limit-output.len())]);',A,
  'writer_host::task_context::tests::te14_visible_frame_sample_skips_padding','TE14_NO_FRAME_PADDING'),
 ('large-logical-row',C,'if bytes.len() <= 3072 {','if true || bytes.len() <= 3072 {',A,
  'writer_host::task_context::tests::te14_large_rows_round_trip_and_changed_data_refuses_cursor','TE14_LARGE_ROW_EXACT_EXPANSION'),
 ('cursor-data-binding',C,'serde_json::to_vec(&(binding, rows))?','serde_json::to_vec(binding)?',A,
  'writer_host::task_context::tests::te14_large_rows_round_trip_and_changed_data_refuses_cursor','TE14_CURSOR_BINDS_DATA'),
])

if __name__ == '__main__':
    harness.main()
