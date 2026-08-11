# M17 Text-Based Editing

M17 turns the timeline transcript into an editing surface. A normal click seeks to a word and starts a selection. Shift-click extends that selection across one contiguous range of the currently rendered timeline transcript. Delete, Backspace, or the inline delete action cuts the selected words from the project.

## Cut semantics

Cut calculations stay in integer source frames until the operation plan is built.

- A selected run starts at the first selected word's `source_start`. If malformed transcript timings overlap the preceding retained word, the start advances to that retained word's end.
- If another retained word follows in the same clip, the cut ends at the later of the final selected word's `source_end` and the next word's `source_start` minus the fps-aware 100 ms safety margin.
- The trailing inter-word gap therefore leaves with the selected words. This is the Descript-style behavior.
- If no retained word follows in the clip, the cut ends one safety margin after the final selected word, capped at the clip out point. Remaining room tone after that margin is not consumed.
- Every result is clamped to the clip's `source_range`. Empty or inverted results are discarded.
- Retained word intervals are never included in a source cut. The core implementation asserts this invariant in debug builds and exercises it with a generated integrity test.
- A selection crossing clip boundaries produces an independent source-frame range for each selected clip.

The 100 ms margin uses the same `silence_cut_margin_frames` calculation as silence editing. It is rounded in the asset's own source time base.

## Project mapping and multi-track ripple

Each source range is mapped from the clip in point into project frames. The start uses `Floor`; the end uses `Ceil`. This matches the source-to-project boundary convention used by transcript and silence rendering.

Ranges are planned in descending project order. Later splits and ripples therefore cannot invalidate the clip identity or project position needed by an earlier range in the same original clip.

For each project range, the planner performs these steps:

1. Split the selected word's clip at the project start and end, skipping a split already on a clip boundary.
2. On every other sync-locked track, split each overlapping clip wherever a cut boundary falls inside it. Delete every isolated piece inside the cut, whether linked to the transcript clip or not.
3. Leave tracks with `sync_lock = false` untouched.
4. Finish with one `RippleDeleteClip` for the isolated middle piece on the selected word's own track.

The non-primary pieces are deleted before the ripple operation. Because every straddling clip has already been split, the core ripple can shift clips beginning at the pre-edit range end without creating overlap. The single final ripple also shifts project markers once. Exact duplicate project ranges, such as the same asset represented by linked A/V transcript entries, are planned once.

## Selection model

The UI stores an anchor and head as `(ClipId, source_start)` identities instead of transient list indices. Each frame resolves those identities against the current ordered timeline transcript and derives the inclusive contiguous index range.

Linked A/V clips of the same asset each contribute a copy of every word to the timeline transcript. The panel and the cut planner both collapse those copies first, keeping the first track's word (`dedup_linked_timeline_words`): the transcript reads as one sentence, each word is selectable exactly once, and a selection can never straddle two near-identical copies whose overlapping cut ranges would fail to plan. The cross-track split-and-delete in the planner removes the linked copy's media anyway. The Analysis facet itself is unchanged, so agent-facing transcript renderings are unaffected.

- Click seeks and sets both anchor and head.
- Shift-click preserves the anchor and moves the head.
- Click on empty transcript space or press Escape to clear the selection.
- Project document changes and core replacement clear the transcript selection, matching the other editor selections.
- During playback, the active word uses the playhead accent. Selected words use the shared selection fill. Auto-scroll follows the playhead only while there is no transcript selection and the pointer is not hovering the panel.

## Undo

The complete split, cross-track delete, and ripple plan is sent as one `DoBatch`. Core applies the batch atomically against a cloned document. A failure exposes no partial edit, and one Undo restores the exact pre-cut document snapshot. This human-initiated edit does not use the agent confirmation broker.

## Filler-word removal (M18)

The timeline transcript identifies exactly six filler sounds: `um`, `uh`, `erm`, `hmm`, `mm`, and `mhm`. Matching trims surrounding whitespace plus leading and trailing `. , ! ? ; : ' " ( ) -` punctuation and the Unicode ellipsis, then lowercases the result. The vocabulary deliberately excludes context-dependent words and interjections such as “like,” “you know,” “ah,” “oh,” and “so.” A one-click removal action should prefer missing an ambiguous filler over deleting intended speech.

When the ready timeline transcript contains fillers, the panel shows their deduplicated count and a Remove fillers action. Filler words remain readable but use muted text with a subtle underline; selection and playhead treatments take precedence. The action can select non-contiguous word indices, merges consecutive fillers within a clip into one source run, and applies the same trailing-gap and 100 ms retained-neighbor margin rules as a normal transcript selection.

All filler ranges are planned together and sent as one `DoBatch`, so the complete cleanup is one atomic edit and one Undo restores the exact pre-removal snapshot. The count, source-range calculation, and planner all use the same `dedup_linked_timeline_words` view. Linked A/V copies are therefore counted once and do not create duplicate or overlapping cuts; sync-locked companion media is still removed by the existing cross-track planner.
