use std::collections::BTreeSet;

use openreel_core::{
    CaptionCue, CaptionPreset, ClipContent, Document, Operation, Rational, TimelineTranscriptWord,
    TranscriptStatus, caption_cues, caption_title_operations as core_caption_title_operations,
};

use crate::{app::OpenReelApp, transcript_edit::dedup_linked_timeline_words};

impl OpenReelApp {
    pub(crate) fn timeline_caption_cues(&self) -> Result<Vec<CaptionCue>, String> {
        let timeline_assets = self
            .focused()
            .document
            .tracks
            .iter()
            .flat_map(|track| &track.clips)
            .filter(|clip| matches!(clip.content, ClipContent::Media))
            .map(|clip| clip.asset)
            .collect::<BTreeSet<_>>();
        if timeline_assets.is_empty() {
            return Err("Add media to the timeline before creating captions".to_owned());
        }

        for asset_id in timeline_assets {
            let Some(asset) = self.focused().document.asset(asset_id) else {
                continue;
            };
            let asset_name = asset.name.clone();
            match self.analysis.transcript_status(asset) {
                TranscriptStatus::Ready(_) | TranscriptStatus::NoSpeech => {}
                TranscriptStatus::Failed(error) => {
                    return Err(format!("Transcript failed for {asset_name}: {error}"));
                }
                _ => return Err(format!("Transcript is not ready for {asset_name}")),
            }
        }

        let words = self
            .analysis
            .timeline_transcript(&self.focused().document, None)
            .map_err(|error| error.to_string())?;
        let cues = caption_cues_from_words(words, self.focused().document.fps);
        if cues.is_empty() {
            Err("The timeline transcript contains no captionable words".to_owned())
        } else {
            Ok(cues)
        }
    }

    pub(crate) fn add_captions(&mut self) {
        let cues = match self.timeline_caption_cues() {
            Ok(cues) => cues,
            Err(error) => {
                self.record_error("Captions", error);
                return;
            }
        };
        match caption_title_operations(&self.focused().document, &cues) {
            Ok(operations) => self.send_operations(operations),
            Err(error) => self.record_error("Captions", error),
        }
    }
}

pub(crate) fn caption_cues_from_words(
    words: Vec<TimelineTranscriptWord>,
    fps: Rational,
) -> Vec<CaptionCue> {
    let words = dedup_linked_timeline_words(words);
    caption_cues(&words, fps)
}

pub(crate) fn caption_title_operations(
    document: &Document,
    cues: &[CaptionCue],
) -> Result<Vec<Operation>, String> {
    core_caption_title_operations(document, cues, CaptionPreset::Clean)
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use openreel_core::{
        AssetId, ClipId, Command, Core, Event, TimeCode, TitlePosition, Track, TrackId, TrackKind,
        apply_batch, srt, vtt,
    };

    use super::*;

    fn cue(start: i64, end: i64, text: &str) -> CaptionCue {
        CaptionCue {
            start: TimeCode(start),
            end: TimeCode(end),
            text: text.to_owned(),
        }
    }

    fn word(text: &str, start: i64, end: i64) -> TimelineTranscriptWord {
        TimelineTranscriptWord {
            text: text.to_owned(),
            speaker: None,
            asset: AssetId(1),
            track: TrackId(1),
            clip: ClipId(1),
            source_start: TimeCode(start),
            source_end: TimeCode(end),
            project_start: TimeCode(start),
            project_end: TimeCode(end),
        }
    }

    fn fixture() -> Document {
        Document {
            tracks: vec![
                Track {
                    id: TrackId(3),
                    kind: TrackKind::Video,
                    sync_lock: true,
                    clips: Vec::new(),
                },
                Track {
                    id: TrackId(9),
                    kind: TrackKind::Audio,
                    sync_lock: false,
                    clips: Vec::new(),
                },
            ],
            fps: Rational::new(30, 1).unwrap(),
            ..Document::default()
        }
    }

    #[test]
    fn burn_in_batch_adds_one_top_track_and_undo_restores_the_snapshot() {
        let original = fixture();
        let cues = [cue(0, 15, "First cue"), cue(15, 30, "Second cue")];
        let operations = caption_title_operations(&original, &cues).unwrap();
        assert_eq!(operations.len(), 3);
        assert_eq!(
            operations.first(),
            Some(&Operation::AddTrack {
                track: Track {
                    id: TrackId(10),
                    kind: TrackKind::Video,
                    sync_lock: true,
                    clips: Vec::new(),
                }
            })
        );

        let mut applied = original.clone();
        apply_batch(&mut applied, &operations).unwrap();
        let caption_track = applied.tracks.last().unwrap();
        assert_eq!(caption_track.id, TrackId(10));
        assert_eq!(caption_track.kind, TrackKind::Video);
        assert!(caption_track.sync_lock);
        assert_eq!(caption_track.clips.len(), cues.len());
        for (clip, expected) in caption_track.clips.iter().zip(cues.iter()) {
            let title = clip.content.title().unwrap();
            assert_eq!(clip.timeline_start, expected.start);
            assert_eq!(clip.source_range, TimeCode::ZERO..TimeCode(15));
            assert_eq!(title.text, expected.text);
            assert_eq!(title.font_size_token, 0);
            assert_eq!(title.color_token, 0);
            assert_eq!(title.position, TitlePosition::LowerThird);
            assert!(title.background_scrim);
            assert_eq!(title.fade_in_frames, TimeCode::ZERO);
            assert_eq!(title.fade_out_frames, TimeCode::ZERO);
        }

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
    fn sidecar_smoke_uses_the_shared_deduped_cue_builder() {
        let fps = Rational::new(30, 1).unwrap();
        let video_word = word("Hello.", 0, 15);
        let mut linked_audio_word = video_word.clone();
        linked_audio_word.track = TrackId(2);
        linked_audio_word.clip = ClipId(2);
        let cues = caption_cues_from_words(vec![video_word, linked_audio_word], fps);

        assert_eq!(cues, vec![cue(0, 30, "Hello.")]);
        assert_eq!(
            srt(&cues, fps),
            "1\n00:00:00,000 --> 00:00:01,000\nHello.\n"
        );
        assert_eq!(
            vtt(&cues, fps),
            "WEBVTT\n\n00:00:00.000 --> 00:00:01.000\nHello.\n"
        );
    }
}
