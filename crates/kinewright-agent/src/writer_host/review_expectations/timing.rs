use super::{
    DerivationWork, ReviewExpectationError, ReviewExpectationInput, ReviewTranscript, bound,
    clip_end, inventory, output_frames, parse_digest, pin_for, times, union_windows,
};
use kinewright_core::output_review::{
    AbsenceProof, AvLandmark, KeptWord, MAX_REVIEW_LANDMARKS, MAX_REVIEW_SEGMENTS,
    MAX_REVIEW_WORD_BYTES, MAX_REVIEW_WORDS, ReviewInventory, ReviewLane, ReviewSegment,
    ReviewSegmentId, ReviewSpan, SegmentSampling, TranscriptIdentity,
};
use kinewright_core::{
    AssetId, AudibleClockError, ClipContent, Document, ExactMicros, TimeCode, TrackKind,
    clip_effective_fps,
};
use kinewright_media::{enabled_clip_runs, frame_to_samples, timeline_audio_segments};
use std::{collections::BTreeMap, ops::Range};

const MIX_RATE: u32 = 48_000;

pub(super) struct TimingEvidence {
    pub(super) transcripts: Vec<TranscriptIdentity>,
    pub(super) segments: Vec<ReviewSegment>,
    pub(super) kept_words: ReviewInventory<KeptWord>,
    pub(super) speech_windows: ReviewInventory<ReviewSpan>,
    pub(super) uncovered_speech_segments: Vec<ReviewSegmentId>,
    pub(super) av_landmarks: ReviewInventory<AvLandmark>,
}

pub(super) fn derive(
    input: &ReviewExpectationInput<'_>,
    work: &mut DerivationWork,
) -> Result<TimingEvidence, ReviewExpectationError> {
    let transcripts = admitted_transcripts(input, work)?;
    let audio = audio_segments(input.document, work)?;
    let video = video_segments(input, audio.len(), work)?;
    let landmarks = landmarks(input, &audio, &video, &transcripts, work)?;
    let mut words = Vec::new();
    let mut uncovered_speech_segments = Vec::new();
    let mut used = BTreeMap::new();
    work.charge(
        audio
            .len()
            .saturating_mul(1 + 2 * search_work(transcripts.len())),
    )?;
    for segment in &audio {
        let transcript = transcripts.get(&segment.asset).copied();
        if let Some(transcript) = transcript {
            used.insert(segment.asset, transcript.identity);
            append_words(segment, transcript, &mut words, work)?;
        }
        if !transcript
            .and_then(|t| t.recognized_source.as_ref())
            .is_some_and(|range| {
                range.start <= segment.source.start && segment.source.end <= range.end
            })
        {
            uncovered_speech_segments.push(segment.id);
        }
    }
    work.charge(sort_work(uncovered_speech_segments.len()))?;
    uncovered_speech_segments.sort_unstable();
    // One sort for words, one for windows, and linear window extraction/union.
    work.charge(2 * sort_work(words.len()) + 2 * words.len())?;
    words.sort_unstable_by_key(|word| (word.output.start, word.segment, word.word_index));
    let windows = union_windows(words.iter().map(|word| word.output.clone()).collect());
    let complete = uncovered_speech_segments.is_empty();
    let proof = if audio.is_empty() {
        AbsenceProof::NoAudibleSourceSegments
    } else {
        AbsenceProof::NoRecognizedSpeechInKeptRanges
    };
    let kept_words = inventory(words, complete, proof);
    let speech_windows = inventory(windows, complete, proof);
    work.charge(sort_work(audio.len() + video.len()) + video.len() + used.len())?;
    let mut segments = audio;
    segments.extend(video);
    segments.sort_unstable_by_key(|segment| (segment.output.start, segment.id));
    Ok(TimingEvidence {
        transcripts: used.into_values().collect(),
        segments,
        kept_words,
        speech_windows,
        uncovered_speech_segments,
        av_landmarks: landmarks,
    })
}

// Linear scans use one unit per candidate. Weight indexed searches and sorts
// by the bit length of the input so repeated lookups and ordering are charged.
fn search_work(len: usize) -> usize {
    (usize::BITS - len.leading_zeros()) as usize
}

fn sort_work(len: usize) -> usize {
    len.saturating_mul(search_work(len))
}

fn admitted_transcripts<'a>(
    input: &ReviewExpectationInput<'a>,
    work: &mut DerivationWork,
) -> Result<BTreeMap<AssetId, &'a ReviewTranscript<'a>>, ReviewExpectationError> {
    let mut transcripts = BTreeMap::new();
    let mut words = 0_usize;
    let mut landmarks = 0_usize;
    for transcript in input.transcripts {
        let identity = transcript.identity;
        words = words.saturating_add(transcript.words.len());
        bound("source_words", words, MAX_REVIEW_WORDS)?;
        landmarks = landmarks.saturating_add(transcript.landmarks.len());
        bound("source_landmarks", landmarks, MAX_REVIEW_LANDMARKS)?;
        work.charge(
            1 + search_work(input.transcripts.len())
                + input.document.media_pool.len()
                + input.input_manifest.entries.len(),
        )?;
        let invalid = || ReviewExpectationError::TranscriptBinding(identity.asset);
        let asset = input.document.asset(identity.asset).ok_or_else(invalid)?;
        let entry = pin_for(input.input_manifest, asset).ok_or_else(invalid)?;
        let interpretation = entry.media_interpretation().map_err(|_| invalid())?;
        if parse_digest(&entry.object.sha256) != Some(identity.source_sha256)
            || entry.object.byte_length != identity.source_byte_length
            || interpretation.audio.as_ref().map(|s| s.index) != Some(identity.audio_stream)
            || transcripts.insert(identity.asset, transcript).is_some()
        {
            return Err(invalid());
        }
        let duration = ExactMicros::frames(asset.duration, asset.fps)?;
        let invalid_timing = || ReviewExpectationError::TranscriptTiming(identity.asset);
        if transcript
            .recognized_source
            .as_ref()
            .is_some_and(|range| range.start >= range.end || range.end > duration)
        {
            return Err(invalid_timing());
        }
        work.charge(2 * transcript.words.len())?;
        for word in transcript.words {
            bound("word_text_bytes", word.text.len(), MAX_REVIEW_WORD_BYTES)?;
            if word.text.trim().is_empty()
                || word.start_micros >= word.end_micros
                || word.end_micros > duration.ceil()
            {
                return Err(invalid_timing());
            }
        }
        if transcript.words.windows(2).any(|pair| {
            pair[0].start_micros > pair[1].start_micros || pair[0].end_micros > pair[1].end_micros
        }) {
            return Err(invalid_timing());
        }
        work.charge(transcript.landmarks.len() + sort_work(transcript.landmarks.len()))?;
        let mut landmark_ids = std::collections::BTreeSet::new();
        for landmark in transcript.landmarks {
            if interpretation.video.is_none()
                || landmark.audio_at >= duration
                || landmark.video_at >= duration
                || !landmark_ids.insert(landmark.id)
            {
                return Err(invalid_timing());
            }
        }
    }
    Ok(transcripts)
}

fn audio_segments(
    document: &Document,
    work: &mut DerivationWork,
) -> Result<Vec<ReviewSegment>, ReviewExpectationError> {
    if document.duration == TimeCode::ZERO {
        return Ok(Vec::new());
    }
    // Clip registration has already charged both timeline enable scans.
    // Account separately for the linear asset lookups in the shared mapper.
    let media_clips = document
        .tracks
        .iter()
        .flat_map(|track| &track.clips)
        .filter(|clip| clip.content.is_media())
        .count();
    let assets = document.media_pool.len();
    work.charge(media_clips.saturating_mul(assets))?;
    let segments = timeline_audio_segments(document, TimeCode::ZERO..document.duration)?;
    let mix_tracks = 2 * document.audio_mix.tracks.len();
    work.charge(segments.len().saturating_mul(1 + assets + mix_tracks))?;
    let mut mapped = Vec::new();
    for segment in segments {
        if !document.track_audible(segment.track) {
            continue;
        }
        let asset = document
            .asset(segment.asset)
            .ok_or(kinewright_core::OpError::MissingAsset(segment.asset))?;
        let samples = |frames: Range<TimeCode>, fps| -> Result<_, AudibleClockError> {
            // Outside the review clock domain the mix's sample conversion
            // can saturate both boundaries and look like an empty segment.
            ExactMicros::frames(frames.end, fps)?;
            Ok(frame_to_samples(frames.start, MIX_RATE, fps)
                ..frame_to_samples(frames.end, MIX_RATE, fps))
        };
        let source = samples(segment.source, asset.fps)?;
        let output = samples(segment.project.clone(), document.fps)?;
        let count = (source.end - source.start).min(output.end - output.start);
        if count == 0 {
            continue;
        }
        let time =
            |sample: u64| ExactMicros::new(u128::from(sample) * 1_000_000, u64::from(MIX_RATE));
        bound("segments", mapped.len() + 1, MAX_REVIEW_SEGMENTS)?;
        mapped.push(ReviewSegment {
            id: ReviewSegmentId {
                clip: segment.clip,
                lane: ReviewLane::Audio,
                project_start: segment.project.start,
            },
            track: segment.track,
            asset: segment.asset,
            source: time(source.start)?..time(source.start + count)?,
            project: segment.project,
            output: time(output.start)?..time(output.start + count)?,
            sampling: SegmentSampling::Audio,
        });
    }
    Ok(mapped)
}

fn video_segments(
    input: &ReviewExpectationInput<'_>,
    audio_count: usize,
    work: &mut DerivationWork,
) -> Result<Vec<ReviewSegment>, ReviewExpectationError> {
    let document = input.document;
    let mut mapped = Vec::new();
    for track in document
        .tracks
        .iter()
        .filter(|track| track.kind == TrackKind::Video)
    {
        for clip in track
            .clips
            .iter()
            .filter(|clip| matches!(clip.content, ClipContent::Media | ClipContent::Freeze(_)))
        {
            work.charge(1 + 2 * document.media_pool.len())?;
            let asset = document
                .asset(clip.asset)
                .ok_or(kinewright_core::OpError::MissingAsset(clip.asset))?;
            let (sampling, trim) = match &clip.content {
                ClipContent::Freeze(freeze) => {
                    let frame = freeze.source_frame;
                    let end = frame.checked_add(TimeCode(1)).ok_or(AudibleClockError)?;
                    (
                        SegmentSampling::VideoHold {
                            frame,
                            source_fps: asset.fps,
                        },
                        times(frame..end, asset.fps)?,
                    )
                }
                _ => (
                    SegmentSampling::Video {
                        clip_start: clip.timeline_start,
                        source_in: clip.source_range.start,
                        source_end: clip.source_range.end,
                        source_fps: asset.fps,
                        effective_fps: clip_effective_fps(asset.fps, clip)?,
                    },
                    times(clip.source_range.clone(), asset.fps)?,
                ),
            };
            let end = clip_end(document, clip)?;
            let duration = TimeCode(end.0 - clip.timeline_start.0);
            let runs = enabled_clip_runs(clip, TimeCode::ZERO, duration);
            work.charge(runs.len())?;
            for (start, end) in runs {
                let project = TimeCode(clip.timeline_start.0 + start.0)
                    ..TimeCode(clip.timeline_start.0 + end.0);
                let output = output_frames(project.clone(), document.fps, input.settings.fps)?;
                if output.is_empty() {
                    continue;
                }
                let source_at = |local| {
                    let elapsed = ExactMicros::frames(local, document.fps)?;
                    trim.start
                        .checked_add(scale(elapsed, clip.speed_percent, 100)?)
                        .map(|at| at.min(trim.end))
                };
                bound(
                    "segments",
                    audio_count + mapped.len() + 1,
                    MAX_REVIEW_SEGMENTS,
                )?;
                mapped.push(ReviewSegment {
                    id: ReviewSegmentId {
                        clip: clip.id,
                        lane: ReviewLane::Video,
                        project_start: project.start,
                    },
                    track: track.id,
                    asset: asset.id,
                    source: if matches!(sampling, SegmentSampling::VideoHold { .. }) {
                        trim.clone()
                    } else {
                        source_at(start)?..source_at(end)?
                    },
                    project,
                    output: times(output, input.settings.fps)?,
                    sampling,
                });
            }
        }
    }
    Ok(mapped)
}

fn scale(
    time: ExactMicros,
    numerator: u32,
    denominator: u32,
) -> Result<ExactMicros, AudibleClockError> {
    let numerator = time
        .numerator()
        .checked_mul(u128::from(numerator))
        .ok_or(AudibleClockError)?;
    let denominator = time
        .denominator()
        .checked_mul(u64::from(denominator))
        .ok_or(AudibleClockError)?;
    ExactMicros::new(numerator, denominator)
}

fn append_words(
    segment: &ReviewSegment,
    transcript: &ReviewTranscript<'_>,
    kept: &mut Vec<KeptWord>,
    work: &mut DerivationWork,
) -> Result<(), ReviewExpectationError> {
    let words = transcript.words;
    work.charge(2 * search_work(words.len()))?;
    let first =
        words.partition_point(|word| ExactMicros::integer(word.end_micros) <= segment.source.start);
    let count = words[first..]
        .partition_point(|word| ExactMicros::integer(word.start_micros) < segment.source.end);
    work.charge(count)?;
    for (offset, word) in words[first..first + count].iter().enumerate() {
        let start = ExactMicros::integer(word.start_micros);
        if !word.is_spoken() {
            continue;
        }
        let end = ExactMicros::integer(word.end_micros);
        let intersection = start.max(segment.source.start)..end.min(segment.source.end);
        let to_output = |source: ExactMicros| {
            segment
                .output
                .start
                .checked_add(source.checked_sub(segment.source.start)?)
        };
        bound("kept_words", kept.len() + 1, MAX_REVIEW_WORDS)?;
        kept.push(KeptWord {
            transcript: transcript.identity,
            word_index: u32::try_from(first + offset).expect("source word count validated"),
            segment: segment.id,
            text: word.text.clone(),
            output: to_output(intersection.start)?..to_output(intersection.end)?,
            clipped: intersection != (start..end),
        });
    }
    Ok(())
}

fn landmarks(
    input: &ReviewExpectationInput<'_>,
    audio: &[ReviewSegment],
    video: &[ReviewSegment],
    transcripts: &BTreeMap<AssetId, &ReviewTranscript<'_>>,
    work: &mut DerivationWork,
) -> Result<ReviewInventory<AvLandmark>, ReviewExpectationError> {
    let mut mapped = Vec::new();
    let mut complete = true;
    let project_frame = ExactMicros::frames(TimeCode(1), input.document.fps)?;
    let output_frame = ExactMicros::frames(TimeCode(1), input.settings.fps)?;
    let mix_sample = ExactMicros::new(1_000_000, u64::from(MIX_RATE))?;
    work.charge(sort_work(video.len()))?;
    let mut video_runs: BTreeMap<_, Vec<_>> = BTreeMap::new();
    for picture in video {
        video_runs.entry(picture.id.clip).or_default().push(picture);
    }
    let lookup = search_work(transcripts.len()) + search_work(video_runs.len());
    work.charge(audio.len().saturating_mul(1 + 2 * lookup))?;
    for sound in audio {
        if let (Some(transcript), Some(pictures)) = (
            transcripts.get(&sound.asset),
            video_runs.get(&sound.id.clip),
        ) {
            // Charge the full join before any landmark walk. Out-of-range
            // landmarks still cost a source comparison.
            work.charge(input.document.media_pool.len() + transcript.landmarks.len())?;
            // Each in-range landmark can search every picture run below.
            let candidates = transcript.landmarks.len().saturating_mul(pictures.len());
            work.charge(candidates)?;
        }
    }
    for sound in audio {
        let mut mapped_for_run = false;
        if let (Some(transcript), Some(pictures)) = (
            transcripts.get(&sound.asset),
            video_runs.get(&sound.id.clip),
        ) {
            let asset = input
                .document
                .asset(sound.asset)
                .ok_or(kinewright_core::OpError::MissingAsset(sound.asset))?;
            let source_frame = ExactMicros::frames(TimeCode(1), asset.fps)?;
            let clock_uncertainty = source_frame
                .checked_add(project_frame)?
                .checked_add(output_frame)?
                .checked_add(mix_sample)?;
            for landmark in transcript
                .landmarks
                .iter()
                .filter(|landmark| sound.source.contains(&landmark.audio_at))
            {
                let Some(picture) = pictures
                    .iter()
                    .find(|picture| picture.source.contains(&landmark.video_at))
                else {
                    continue;
                };
                // Audio and video from the same clip can only coexist at unit speed.
                let video_at = ExactMicros::frames(picture.project.start, input.document.fps)?
                    .checked_add(landmark.video_at.checked_sub(picture.source.start)?)?;
                let audio_at = sound
                    .output
                    .start
                    .checked_add(landmark.audio_at.checked_sub(sound.source.start)?)?;
                bound("mapped_landmarks", mapped.len() + 1, MAX_REVIEW_LANDMARKS)?;
                mapped.push(AvLandmark {
                    asset: sound.asset,
                    source_landmark: landmark.id,
                    audio_segment: sound.id,
                    video_segment: picture.id,
                    audio_at,
                    video_at,
                    uncertainty: landmark.uncertainty.checked_add(clock_uncertainty)?,
                });
                mapped_for_run = true;
            }
        }
        complete &= mapped_for_run;
    }
    work.charge(sort_work(mapped.len()))?;
    mapped.sort_unstable_by_key(|landmark| {
        (
            landmark.audio_at,
            landmark.audio_segment,
            landmark.source_landmark,
        )
    });
    Ok(inventory(
        mapped,
        audio.is_empty() || video.is_empty() || complete,
        AbsenceProof::NoPairedAudioVideo,
    ))
}
