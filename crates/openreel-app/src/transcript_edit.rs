use openreel_core::{
    Clip, ClipId, Document, FrameRounding, Operation, TimeCode, TimelineTranscriptWord, TrackId,
    TranscriptCutRange, is_filler_word, map_frames_with_rounding, transcript_cut_ranges,
    transcript_cut_ranges_for_indices,
};

use crate::{app::OpenReelApp, transcript_ui::TranscriptSelection};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ProjectCutRange {
    track: TrackId,
    clip: ClipId,
    start: TimeCode,
    end: TimeCode,
}

/// Collapse the copies of a word that linked A/V clips repeat at the same
/// project position, keeping the first track's copy. The panel and the cut
/// planner must share this view: overlapping near-duplicate cut ranges from a
/// selection spanning both copies would fail to plan.
pub(crate) fn dedup_linked_timeline_words(
    words: Vec<TimelineTranscriptWord>,
) -> Vec<TimelineTranscriptWord> {
    let mut seen = std::collections::HashSet::new();
    words
        .into_iter()
        .filter(|word| {
            seen.insert((
                word.asset.0,
                word.source_start.0,
                word.source_end.0,
                word.project_start.0,
                word.project_end.0,
            ))
        })
        .collect()
}

pub(crate) fn filler_word_indices(words: &[TimelineTranscriptWord]) -> Vec<usize> {
    words
        .iter()
        .enumerate()
        .filter_map(|(index, word)| is_filler_word(&word.text).then_some(index))
        .collect()
}

impl OpenReelApp {
    pub(crate) fn cut_selected_transcript_words(&mut self) {
        let Some(selection) = self.transcript_selection else {
            return;
        };
        let words = match self.analysis.timeline_transcript(&self.document, None) {
            Ok(words) => dedup_linked_timeline_words(words),
            Err(error) => {
                self.record_error("Transcript edit", error.to_string());
                return;
            }
        };
        match selected_transcript_word_cut_operations(&self.document, &words, selection) {
            Ok(operations) if operations.is_empty() => {
                self.record_error(
                    "Transcript edit",
                    "The selected words contain no cuttable frames",
                );
            }
            Ok(operations) => self.send_operations(operations),
            Err(error) => self.record_error("Transcript edit", error),
        }
    }

    pub(crate) fn remove_filler_words(&mut self) {
        let words = match self.analysis.timeline_transcript(&self.document, None) {
            Ok(words) => dedup_linked_timeline_words(words),
            Err(error) => {
                self.record_error("Transcript edit", error.to_string());
                return;
            }
        };
        let selected_indices = filler_word_indices(&words);
        if selected_indices.is_empty() {
            self.record_error(
                "Transcript edit",
                "There are no filler words available to remove",
            );
            return;
        }
        let cuts = transcript_cut_ranges_for_indices(&self.document, &words, &selected_indices);
        match transcript_word_cut_operations(&self.document, &cuts) {
            Ok(operations) if operations.is_empty() => {
                self.record_error(
                    "Transcript edit",
                    "The filler words contain no cuttable frames",
                );
            }
            Ok(operations) => self.send_operations(operations),
            Err(error) => self.record_error("Transcript edit", error),
        }
    }
}

fn selected_transcript_word_cut_operations(
    document: &Document,
    words: &[TimelineTranscriptWord],
    selection: TranscriptSelection,
) -> Result<Vec<Operation>, String> {
    let selected = selection
        .indices(words)
        .ok_or_else(|| "The transcript selection is no longer available".to_owned())?;
    let cuts = transcript_cut_ranges(document, words, selected);
    transcript_word_cut_operations(document, &cuts)
}

pub(crate) fn transcript_word_cut_operations(
    document: &Document,
    cuts: &[TranscriptCutRange],
) -> Result<Vec<Operation>, String> {
    let mut project_cuts = cuts
        .iter()
        .map(|cut| map_cut_to_project(document, cut))
        .collect::<Result<Vec<_>, _>>()?;
    project_cuts.sort_by(|left, right| {
        right
            .start
            .cmp(&left.start)
            .then_with(|| right.end.cmp(&left.end))
            .then_with(|| left.track.cmp(&right.track))
            .then_with(|| left.clip.cmp(&right.clip))
    });
    project_cuts.dedup_by(|left, right| left.start == right.start && left.end == right.end);

    let mut working = document.clone();
    let mut operations = Vec::new();
    for cut in project_cuts {
        append_project_cut(&mut working, cut, &mut operations)?;
    }
    Ok(operations)
}

fn map_cut_to_project(
    document: &Document,
    cut: &TranscriptCutRange,
) -> Result<ProjectCutRange, String> {
    let clip = document
        .clip(cut.clip)
        .ok_or_else(|| format!("Transcript clip {} no longer exists", cut.clip))?;
    let track = document
        .tracks
        .iter()
        .find(|track| track.clips.iter().any(|candidate| candidate.id == cut.clip))
        .ok_or_else(|| format!("Transcript clip {} has no track", cut.clip))?;
    let asset = document
        .asset(clip.asset)
        .ok_or_else(|| format!("Transcript asset {} no longer exists", clip.asset))?;
    if cut.source_range.start < clip.source_range.start
        || cut.source_range.end > clip.source_range.end
        || cut.source_range.end <= cut.source_range.start
    {
        return Err(format!(
            "Transcript cut {}..{} is outside clip {} source range {}..{}",
            cut.source_range.start,
            cut.source_range.end,
            cut.clip,
            clip.source_range.start,
            clip.source_range.end
        ));
    }
    let start_offset = cut
        .source_range
        .start
        .checked_sub(clip.source_range.start)
        .ok_or_else(|| "Transcript cut start underflowed".to_owned())?;
    let end_offset = cut
        .source_range
        .end
        .checked_sub(clip.source_range.start)
        .ok_or_else(|| "Transcript cut end underflowed".to_owned())?;
    let start = clip
        .timeline_start
        .checked_add(
            map_frames_with_rounding(start_offset, asset.fps, document.fps, FrameRounding::Floor)
                .map_err(|error| error.to_string())?,
        )
        .ok_or_else(|| "Transcript cut start overflowed".to_owned())?;
    let end = clip
        .timeline_start
        .checked_add(
            map_frames_with_rounding(end_offset, asset.fps, document.fps, FrameRounding::Ceil)
                .map_err(|error| error.to_string())?,
        )
        .ok_or_else(|| "Transcript cut end overflowed".to_owned())?;
    if end <= start {
        return Err("Transcript cut maps to an empty project range".to_owned());
    }
    Ok(ProjectCutRange {
        track: track.id,
        clip: cut.clip,
        start,
        end,
    })
}

fn append_project_cut(
    working: &mut Document,
    cut: ProjectCutRange,
    operations: &mut Vec<Operation>,
) -> Result<(), String> {
    if working.clip(cut.clip).is_none() {
        return Err(format!(
            "Transcript clip {} was invalidated by an earlier cut",
            cut.clip
        ));
    }

    split_track_at(working, cut.track, cut.start, operations)?;
    split_track_at(working, cut.track, cut.end, operations)?;
    let primary_middle = clip_covering_exact_range(working, cut.track, cut.start, cut.end)?
        .ok_or_else(|| {
            format!(
                "Could not isolate transcript range {}..{} on track {}",
                cut.start, cut.end, cut.track
            )
        })?;

    let participating_tracks = working
        .tracks
        .iter()
        .filter(|track| track.id != cut.track && track.sync_lock)
        .map(|track| track.id)
        .collect::<Vec<_>>();
    for track in participating_tracks {
        split_track_at(working, track, cut.start, operations)?;
        split_track_at(working, track, cut.end, operations)?;
        let mut overlapping = track_clips_overlapping(working, track, cut.start, cut.end)?;
        overlapping.sort_by(|left, right| {
            right
                .timeline_start
                .cmp(&left.timeline_start)
                .then_with(|| right.id.cmp(&left.id))
        });
        for clip in overlapping {
            push_operation(working, operations, Operation::DeleteClip { clip: clip.id })?;
        }
    }

    push_operation(
        working,
        operations,
        Operation::RippleDeleteClip {
            clip: primary_middle,
        },
    )
}

fn split_track_at(
    working: &mut Document,
    track: TrackId,
    at: TimeCode,
    operations: &mut Vec<Operation>,
) -> Result<(), String> {
    let containing = working
        .tracks
        .iter()
        .find(|candidate| candidate.id == track)
        .ok_or_else(|| format!("Track {track} no longer exists"))?
        .clips
        .iter()
        .find_map(|clip| {
            let end = clip_end(working, clip).ok()?;
            (clip.timeline_start < at && at < end).then_some(clip.id)
        });
    if let Some(clip) = containing {
        push_operation(working, operations, Operation::SplitClip { clip, at })?;
    }
    Ok(())
}

fn clip_covering_exact_range(
    document: &Document,
    track: TrackId,
    start: TimeCode,
    end: TimeCode,
) -> Result<Option<ClipId>, String> {
    let track = document
        .tracks
        .iter()
        .find(|candidate| candidate.id == track)
        .ok_or_else(|| format!("Track {track} no longer exists"))?;
    for clip in &track.clips {
        if clip.timeline_start == start && clip_end(document, clip)? == end {
            return Ok(Some(clip.id));
        }
    }
    Ok(None)
}

fn track_clips_overlapping(
    document: &Document,
    track: TrackId,
    start: TimeCode,
    end: TimeCode,
) -> Result<Vec<Clip>, String> {
    let track = document
        .tracks
        .iter()
        .find(|candidate| candidate.id == track)
        .ok_or_else(|| format!("Track {track} no longer exists"))?;
    let mut overlapping = Vec::new();
    for clip in &track.clips {
        let clip_end = clip_end(document, clip)?;
        if clip.timeline_start < end && clip_end > start {
            if clip.timeline_start < start || clip_end > end {
                return Err(format!(
                    "Clip {} still straddles transcript cut {}..{}",
                    clip.id, start, end
                ));
            }
            overlapping.push(clip.clone());
        }
    }
    Ok(overlapping)
}

fn clip_end(document: &Document, clip: &Clip) -> Result<TimeCode, String> {
    clip.timeline_start
        .checked_add(
            document
                .clip_duration(clip)
                .map_err(|error| error.to_string())?,
        )
        .ok_or_else(|| format!("Clip {} end overflowed", clip.id))
}

fn push_operation(
    working: &mut Document,
    operations: &mut Vec<Operation>,
    operation: Operation,
) -> Result<(), String> {
    operation
        .apply(working)
        .map_err(|error| format!("Could not plan transcript edit: {error}"))?;
    operations.push(operation);
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use openreel_core::{
        AssetId, ClipContent, Command, Core, Event, LinkId, Marker, MarkerId, MediaAsset,
        MediaKind, Rational, Track, TrackKind, apply_batch,
    };

    use super::*;

    fn clip(id: u64, asset: AssetId, link: Option<LinkId>) -> Clip {
        Clip {
            id: ClipId(id),
            asset,
            source_range: TimeCode(0)..TimeCode(120),
            content: ClipContent::Media,
            timeline_start: TimeCode::ZERO,
            effects: Vec::new(),
            transition_in: None,
            link,
        }
    }

    fn fixture() -> Document {
        let fps = Rational::new(30, 1).unwrap();
        let asset = MediaAsset {
            id: AssetId(1),
            path: PathBuf::from("fixture.mp4"),
            name: "fixture.mp4".to_owned(),
            duration: TimeCode(180),
            fps,
            kind: MediaKind::AudioVideo,
            resolution: Some((1_920, 1_080)),
        };
        Document {
            tracks: vec![
                Track {
                    id: TrackId(1),
                    kind: TrackKind::Video,
                    sync_lock: true,
                    clips: vec![clip(1, asset.id, Some(LinkId(7)))],
                },
                Track {
                    id: TrackId(2),
                    kind: TrackKind::Audio,
                    sync_lock: true,
                    clips: vec![clip(2, asset.id, Some(LinkId(7)))],
                },
                Track {
                    id: TrackId(3),
                    kind: TrackKind::Audio,
                    sync_lock: true,
                    clips: vec![clip(3, asset.id, None)],
                },
                Track {
                    id: TrackId(4),
                    kind: TrackKind::Audio,
                    sync_lock: false,
                    clips: vec![clip(4, asset.id, None)],
                },
            ],
            media_pool: vec![asset],
            markers: vec![
                Marker {
                    id: MarkerId(1),
                    position: TimeCode(20),
                    label: "before".to_owned(),
                    color_token: 0,
                },
                Marker {
                    id: MarkerId(2),
                    position: TimeCode(80),
                    label: "after".to_owned(),
                    color_token: 1,
                },
            ],
            fps,
            resolution: (1_920, 1_080),
            duration: TimeCode(120),
        }
    }

    fn cut(clip: u64, start: i64, end: i64) -> TranscriptCutRange {
        TranscriptCutRange {
            track: TrackId(1),
            clip: ClipId(clip),
            source_range: TimeCode(start)..TimeCode(end),
        }
    }

    fn word_with_text(text: &str, start: i64, end: i64) -> TimelineTranscriptWord {
        TimelineTranscriptWord {
            text: text.to_owned(),
            asset: AssetId(1),
            track: TrackId(1),
            clip: ClipId(1),
            source_start: TimeCode(start),
            source_end: TimeCode(end),
            project_start: TimeCode(start),
            project_end: TimeCode(end),
        }
    }

    fn word(start: i64, end: i64) -> TimelineTranscriptWord {
        word_with_text("selected", start, end)
    }

    fn track_segments(document: &Document, track: TrackId) -> Vec<(i64, i64, i64)> {
        document
            .tracks
            .iter()
            .find(|candidate| candidate.id == track)
            .unwrap()
            .clips
            .iter()
            .map(|clip| {
                (
                    clip.source_range.start.0,
                    clip.source_range.end.0,
                    clip.timeline_start.0,
                )
            })
            .collect()
    }

    #[test]
    fn linked_pair_and_unlinked_sync_locked_clip_lose_the_same_range() {
        let mut document = fixture();
        let operations = transcript_word_cut_operations(&document, &[cut(1, 30, 60)]).unwrap();
        assert!(matches!(
            operations.last(),
            Some(Operation::RippleDeleteClip { .. })
        ));
        apply_batch(&mut document, &operations).unwrap();

        let expected = vec![(0, 30, 0), (60, 120, 30)];
        assert_eq!(track_segments(&document, TrackId(1)), expected);
        assert_eq!(track_segments(&document, TrackId(2)), expected);
        assert_eq!(track_segments(&document, TrackId(3)), expected);
    }

    #[test]
    fn duplicate_linked_av_transcript_ranges_ripple_only_once() {
        let mut document = fixture();
        document.tracks.truncate(2);
        let mut audio_cut = cut(2, 30, 60);
        audio_cut.track = TrackId(2);
        let operations =
            transcript_word_cut_operations(&document, &[cut(1, 30, 60), audio_cut]).unwrap();
        assert_eq!(
            operations
                .iter()
                .filter(|operation| matches!(operation, Operation::RippleDeleteClip { .. }))
                .count(),
            1
        );
        apply_batch(&mut document, &operations).unwrap();
        let expected = vec![(0, 30, 0), (60, 120, 30)];
        assert_eq!(track_segments(&document, TrackId(1)), expected);
        assert_eq!(track_segments(&document, TrackId(2)), expected);
    }

    #[test]
    fn unlocked_track_is_untouched_and_markers_shift_once() {
        let mut document = fixture();
        let untouched = document.tracks[3].clone();
        let operations = transcript_word_cut_operations(&document, &[cut(1, 30, 60)]).unwrap();
        apply_batch(&mut document, &operations).unwrap();

        assert_eq!(document.tracks[3], untouched);
        assert_eq!(document.markers[0].position, TimeCode(20));
        assert_eq!(document.markers[1].position, TimeCode(50));
    }

    #[test]
    fn batch_applies_cleanly_and_one_undo_restores_the_exact_snapshot() {
        let original = fixture();
        let operations = transcript_word_cut_operations(&original, &[cut(1, 30, 60)]).unwrap();
        let core = Core::spawn(original.clone()).unwrap();
        assert!(matches!(
            core.request(Command::DoBatch(operations)).unwrap(),
            Event::DocumentChanged { .. }
        ));
        let Event::DocumentChanged { doc, .. } = core.request(Command::Undo).unwrap() else {
            panic!("undo must return the restored document");
        };
        assert_eq!(*doc, original);
    }

    #[test]
    fn two_ranges_in_one_clip_are_planned_in_descending_project_order() {
        let mut document = fixture();
        document.tracks.truncate(1);
        let operations =
            transcript_word_cut_operations(&document, &[cut(1, 20, 30), cut(1, 60, 70)]).unwrap();
        assert_eq!(
            operations.first(),
            Some(&Operation::SplitClip {
                clip: ClipId(1),
                at: TimeCode(60),
            })
        );
        apply_batch(&mut document, &operations).unwrap();
        assert_eq!(
            track_segments(&document, TrackId(1)),
            vec![(0, 20, 0), (30, 60, 20), (70, 120, 50)]
        );
    }

    #[test]
    fn linked_duplicate_words_collapse_to_the_first_copy() {
        let video = word(30, 50);
        let mut audio = word(30, 50);
        audio.track = TrackId(2);
        audio.clip = ClipId(2);
        let unique = word(60, 70);
        let deduped = dedup_linked_timeline_words(vec![video.clone(), audio, unique.clone()]);
        assert_eq!(deduped, vec![video, unique]);
    }

    #[test]
    fn linked_duplicate_fillers_are_counted_once() {
        let video = word_with_text("Um,", 30, 50);
        let mut audio = video.clone();
        audio.track = TrackId(2);
        audio.clip = ClipId(2);
        let keep = word_with_text("keep", 60, 70);

        let deduped = dedup_linked_timeline_words(vec![video.clone(), audio, keep.clone()]);

        assert_eq!(deduped, vec![video, keep]);
        assert_eq!(filler_word_indices(&deduped), vec![0]);
    }

    #[test]
    fn filler_runs_apply_as_one_batch_and_one_undo_restores_the_snapshot() {
        let mut original = fixture();
        original.tracks.truncate(1);
        let words = vec![
            word_with_text("keep", 0, 5),
            word_with_text("um", 10, 15),
            word_with_text("keep", 20, 25),
            word_with_text("uh", 30, 35),
            word_with_text("UH", 40, 45),
            word_with_text("keep", 50, 55),
        ];
        let selected_indices = filler_word_indices(&words);
        assert_eq!(selected_indices, vec![1, 3, 4]);
        let cuts = transcript_cut_ranges_for_indices(&original, &words, &selected_indices);
        assert_eq!(cuts, vec![cut(1, 10, 17), cut(1, 30, 47)]);

        let operations = transcript_word_cut_operations(&original, &cuts).unwrap();
        assert_eq!(
            operations
                .iter()
                .filter(|operation| matches!(operation, Operation::RippleDeleteClip { .. }))
                .count(),
            2
        );
        let mut applied = original.clone();
        apply_batch(&mut applied, &operations).unwrap();
        assert_ne!(applied, original);

        let core = Core::spawn(original.clone()).unwrap();
        assert!(matches!(
            core.request(Command::DoBatch(operations)).unwrap(),
            Event::DocumentChanged { .. }
        ));
        let Event::DocumentChanged { doc, .. } = core.request(Command::Undo).unwrap() else {
            panic!("undo must return the restored document");
        };
        assert_eq!(*doc, original);
    }

    #[test]
    fn delete_selected_with_active_transcript_selection_emits_word_cut_batch_not_clip_delete() {
        let mut document = fixture();
        document.tracks.truncate(1);
        let words = vec![word(30, 50)];
        let selection = TranscriptSelection::single(&words[0]);
        let operations =
            selected_transcript_word_cut_operations(&document, &words, selection).unwrap();

        assert!(operations.len() > 1, "word cut must be sent as a batch");
        assert!(matches!(
            operations.last(),
            Some(Operation::RippleDeleteClip { .. })
        ));
        assert_ne!(
            operations,
            vec![Operation::DeleteClip { clip: ClipId(1) }],
            "the selected timeline clip delete must not win over transcript selection"
        );
    }
}
