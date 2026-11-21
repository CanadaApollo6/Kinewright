//! Retained-speech check: streaming recognition of the decoded encode on the
//! output clock, in-order matching of every expected kept word, and the DQ
//! join audit on the output's own sound profile.
use std::collections::BTreeMap;
use std::fs::File;
use std::path::Path;

use kinewright_core::dialogue::audit::{
    self, AuditSettings, Detail, GuardIssue, Item, KeptRange, RecordingId,
};
use kinewright_core::dialogue::measure::ProfileBuilder;
use kinewright_core::dialogue::{Dialogue, Settings, SourceRange, Word};
use kinewright_core::output_review::{
    KeptWord, RequiredReviewRow, ReviewExpectations, ReviewInventory, ReviewLane, ReviewSegment,
};
use kinewright_core::{AudibleSpan, ExactMicros, ExportCancellation, MediaError};
use serde::{Deserialize, Serialize};

use super::decode::{AudioReader, Placement};
use super::{FindingKind, FindingVerdict, ReviewRowReport, config_digest};
use crate::ReviewOutputTiming;
use crate::recognizer::{self, Recognizer};

use FindingVerdict::{Failed, Undecided};

/// A recognised word on the output clock.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecognizedReviewWord {
    pub text: String,
    pub start_us: u64,
    pub end_us: u64,
}

/// Recognised words, or why recognition could not run.
pub type ReviewRecognition = Result<Vec<RecognizedReviewWord>, String>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetainedSpeechConfig {
    /// An expected word matches a recognised word starting this close.
    pub match_window_us: u64,
    /// Recognised words accepted; more leaves the row undecided.
    pub max_recognized_words: usize,
    /// Audit items listed; a full page leaves the joins undecided.
    pub max_join_items: usize,
    /// PTS placement tolerance of the 16 kHz recognition stream.
    pub placement_tolerance_samples: u64,
}

impl Default for RetainedSpeechConfig {
    fn default() -> Self {
        Self {
            match_window_us: 400_000,
            max_recognized_words: kinewright_core::output_review::MAX_REVIEW_WORDS,
            max_join_items: 1_024,
            placement_tolerance_samples: 32,
        }
    }
}

impl RetainedSpeechConfig {
    #[must_use]
    pub fn sha256(&self) -> [u8; 32] {
        config_digest(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetainedSpeechReport {
    pub configuration_sha256: [u8; 32],
    pub row: ReviewRowReport,
    pub expected_words: u64,
    pub matched_words: u64,
    pub joins_checked: u64,
}

/// Recognise the decoded encode with a loaded recogniser. Output time zero is
/// the first picture; PTS gaps become silence.
///
/// # Errors
/// Decoder, recogniser and cancellation failures.
pub fn recognize_review_audio_with(
    recognizer: &mut dyn Recognizer,
    artifact: File,
    timing: &ReviewOutputTiming,
    tolerance_samples: u64,
    cancellation: &ExportCancellation,
) -> Result<Vec<RecognizedReviewWord>, MediaError> {
    let mut stream = OutputPcm::open(
        artifact,
        timing,
        recognizer::SAMPLE_RATE,
        tolerance_samples,
        cancellation,
    )?;
    let recognition = recognizer::queue::drive(
        recognizer,
        cancellation,
        |window| {
            stream
                .next(window, cancellation)
                .map_err(recognizer::Error::from)
        },
        |_| {},
        true,
    )
    .map_err(MediaError::from)?;
    Ok(recognition
        .words
        .into_iter()
        .map(|word| RecognizedReviewWord {
            text: word.text,
            start_us: word.start_us,
            end_us: word.end_us,
        })
        .collect())
}

/// Recognise the decoded encode with the installed English streaming model.
/// A missing or changed model is `Ok(Err(reason))`: the check cannot decide.
///
/// # Errors
/// Decoder, recogniser and cancellation failures.
pub fn recognize_review_audio(
    data_dir: &Path,
    artifact: File,
    timing: &ReviewOutputTiming,
    config: &RetainedSpeechConfig,
    cancellation: &ExportCancellation,
) -> Result<ReviewRecognition, MediaError> {
    use recognizer::{ENGLISH_MODEL, Error, Identity, ModelError};
    let data_dir = crate::data_dir::ensure_root(data_dir)?;
    let root = data_dir.join("models").join(ENGLISH_MODEL.id);
    let path = root.join(ENGLISH_MODEL.files[0].name);
    match recognizer::model::verify_at(
        &data_dir,
        &ENGLISH_MODEL.files[0],
        &path,
        &root.join("verified"),
        cancellation,
    ) {
        Ok(_) => {}
        Err(Error::Model(
            reason @ (ModelError::Missing
            | ModelError::WrongLength { .. }
            | ModelError::WrongHash { .. }
            | ModelError::Changed),
        )) => return Ok(Err(format!("recognition model unavailable: {reason:?}"))),
        Err(error) => return Err(error.into()),
    }
    let timing = timing.clone();
    let tolerance = config.placement_tolerance_samples;
    let cancel = cancellation.clone();
    let recognized = recognizer::queue::queued(
        data_dir.clone(),
        path,
        root.join("verified"),
        cancellation.clone(),
        Identity::english(),
        move |recognizer| {
            let words =
                recognize_review_audio_with(recognizer, artifact, &timing, tolerance, &cancel)?;
            Ok(recognizer::Recognition {
                timing_source: recognizer::TimingSource::DecoderTokenEstimates,
                words: words
                    .into_iter()
                    .map(|word| recognizer::Word {
                        text: word.text,
                        start_us: word.start_us,
                        end_us: word.end_us,
                    })
                    .collect(),
            })
        },
    )
    .map_err(MediaError::from)?;
    Ok(Ok(recognized
        .recognition
        .words
        .into_iter()
        .map(|word| RecognizedReviewWord {
            text: word.text,
            start_us: word.start_us,
            end_us: word.end_us,
        })
        .collect()))
}

/// Mono PCM of the decoded encode on the output clock from sample zero.
struct OutputPcm {
    reader: AudioReader,
    placement: Placement,
    /// Output sample position of the next sample handed out.
    position: i64,
    chunk: Vec<f32>,
}

impl OutputPcm {
    fn open(
        artifact: File,
        timing: &ReviewOutputTiming,
        rate: u32,
        tolerance: u64,
        cancellation: &ExportCancellation,
    ) -> Result<Self, MediaError> {
        Ok(Self {
            reader: AudioReader::open(artifact, timing, Some(rate), Some(1), cancellation)?,
            placement: Placement {
                cursor: None,
                tolerance,
            },
            position: 0,
            chunk: Vec::new(),
        })
    }

    /// Append the next placed samples; `false` at the end of the stream.
    fn next(
        &mut self,
        out: &mut Vec<f32>,
        cancellation: &ExportCancellation,
    ) -> Result<bool, MediaError> {
        loop {
            let Some(chunk) = self.reader.next(&mut self.chunk, cancellation)? else {
                return Ok(false);
            };
            if chunk.frames == 0 {
                continue;
            }
            let placed = self.placement.place(&chunk);
            if placed.start > self.position {
                let silence = usize::try_from(placed.start - self.position).unwrap_or(0);
                out.resize(out.len() + silence, 0.0);
                self.position = placed.start;
            }
            let skip = usize::try_from(self.position - placed.start)
                .unwrap_or(0)
                .min(self.chunk.len());
            out.extend_from_slice(&self.chunk[skip..]);
            self.position = self
                .position
                .max(placed.start + i64::try_from(self.chunk.len()).unwrap_or(0));
            if skip < self.chunk.len() {
                return Ok(true);
            }
        }
    }
}

/// Decide the retained-speech row from the encode, its recognised words and
/// the frozen expectations.
///
/// # Errors
/// Returns `MediaError::Cancelled` when cancelled. Decoder failures are an
/// undecided row.
pub fn check_retained_speech(
    artifact: File,
    timing: &ReviewOutputTiming,
    expectations: &ReviewExpectations,
    recognized: &ReviewRecognition,
    config: &RetainedSpeechConfig,
    cancellation: &ExportCancellation,
) -> Result<RetainedSpeechReport, MediaError> {
    let mut report = RetainedSpeechReport {
        configuration_sha256: config.sha256(),
        row: ReviewRowReport::new(RequiredReviewRow::RetainedSpeechChecks),
        expected_words: 0,
        matched_words: 0,
        joins_checked: 0,
    };
    let row = &mut report.row;
    let speech_proven_absent = matches!(expectations.kept_words, ReviewInventory::NoneProven(_));
    if speech_proven_absent && expectations.uncovered_speech_segments.is_empty() {
        return Ok(report);
    }
    if !expectations.kept_words.is_determined() {
        row.push(FindingKind::SpeechNotDetermined, Undecided, [0, 0]);
    }
    for segment in &expectations.uncovered_speech_segments {
        let span = expectations
            .segments
            .iter()
            .find(|candidate| candidate.id == *segment)
            .map_or([0, 0], |candidate| {
                [candidate.output.start.floor(), candidate.output.end.ceil()]
            });
        row.push(FindingKind::SpeechNotCovered, Undecided, span);
    }
    let words = match recognized {
        Ok(words) if words.len() <= config.max_recognized_words => words,
        Ok(words) => {
            row.push(
                FindingKind::RecognitionUnavailable {
                    reason: format!("{} recognised words exceed the cap", words.len()),
                },
                Undecided,
                [0, 0],
            );
            return Ok(report);
        }
        Err(reason) => {
            row.push(
                FindingKind::RecognitionUnavailable {
                    reason: reason.chars().take(256).collect(),
                },
                Undecided,
                [0, 0],
            );
            return Ok(report);
        }
    };
    let (expected, matched) = match_words(row, expectations, words, config);
    report.expected_words = expected;
    report.matched_words = matched;
    match output_dialogue(artifact, timing, words, config, cancellation) {
        Ok(dialogue) => {
            report.joins_checked = audit_joins(row, &dialogue, expectations, config);
        }
        Err(MediaError::Cancelled) => return Err(MediaError::Cancelled),
        Err(error) => row.push(
            FindingKind::ProfileUnavailable {
                reason: error.to_string().chars().take(256).collect(),
            },
            Undecided,
            [0, 0],
        ),
    }
    Ok(report)
}

fn match_words(
    row: &mut ReviewRowReport,
    expectations: &ReviewExpectations,
    words: &[RecognizedReviewWord],
    config: &RetainedSpeechConfig,
) -> (u64, u64) {
    let keys: Vec<String> = words
        .iter()
        .map(|word| audit::comparison_key(&word.text))
        .collect();
    let mut expected: Vec<&KeptWord> = expectations.kept_words.known().collect();
    expected.sort_by(|a, b| {
        a.output
            .start
            .partial_cmp(&b.output.start)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut cursor = 0;
    let mut matched = 0;
    for word in &expected {
        let start = word.output.start.floor();
        let span = [start, word.output.end.ceil()];
        if word.clipped {
            row.push(
                FindingKind::WordClippedInCut {
                    word_index: word.word_index,
                },
                Undecided,
                span,
            );
        }
        let key = audit::comparison_key(&word.text);
        let window = start.saturating_sub(config.match_window_us)..start + config.match_window_us;
        let candidates: Vec<usize> = (cursor..words.len())
            .take_while(|&index| words[index].start_us < window.end)
            .filter(|&index| window.contains(&words[index].start_us) && keys[index] == key)
            .collect();
        let Some(&first) = candidates.first() else {
            row.push(
                FindingKind::WordMissing {
                    word_index: word.word_index,
                    text: word.text.chars().take(64).collect(),
                },
                Failed,
                span,
            );
            continue;
        };
        let distance = |index: usize| words[index].start_us.abs_diff(start);
        if candidates
            .iter()
            .skip(1)
            .any(|&other| distance(other) < distance(first))
        {
            row.push(
                FindingKind::WordAmbiguous {
                    word_index: word.word_index,
                    text: word.text.chars().take(64).collect(),
                },
                Undecided,
                span,
            );
        }
        matched += 1;
        cursor = first + 1;
    }
    (u64::try_from(expected.len()).unwrap_or(u64::MAX), matched)
}

/// The output's own dialogue: its 16 kHz sound profile with the recognised
/// word spans as voice.
fn output_dialogue(
    artifact: File,
    timing: &ReviewOutputTiming,
    words: &[RecognizedReviewWord],
    config: &RetainedSpeechConfig,
    cancellation: &ExportCancellation,
) -> Result<Dialogue, MediaError> {
    let refuse = |error: &dyn std::fmt::Display| MediaError::Backend(error.to_string());
    let mut stream = OutputPcm::open(
        artifact,
        timing,
        recognizer::SAMPLE_RATE,
        config.placement_tolerance_samples,
        cancellation,
    )?;
    let mut builder = ProfileBuilder::new(recognizer::SAMPLE_RATE, 1).map_err(|e| refuse(&e))?;
    let mut window = Vec::new();
    loop {
        window.clear();
        let more = stream.next(&mut window, cancellation)?;
        builder.push(&window).map_err(|e| refuse(&e))?;
        if !more {
            break;
        }
    }
    let duration = u64::try_from(
        u128::try_from(stream.position.max(0)).unwrap_or(0) * 1_000_000
            / u128::from(recognizer::SAMPLE_RATE),
    )
    .unwrap_or(u64::MAX);
    let mut voice: Vec<SourceRange> = Vec::new();
    let mut spoken = Vec::new();
    let mut last_start = 0;
    let mut last_end = 0;
    for word in words {
        let end = word.end_us.min(duration);
        if word.text.trim().is_empty()
            || word.start_us >= end
            || word.start_us < last_start
            || end < last_end
        {
            continue;
        }
        last_start = word.start_us;
        last_end = end;
        match voice.last_mut() {
            Some(range) if word.start_us <= range.end_micros => {
                range.end_micros = range.end_micros.max(end);
            }
            _ => voice.push(SourceRange {
                start_micros: word.start_us,
                end_micros: end,
            }),
        }
        spoken.push(Word {
            text: word.text.clone(),
            start_micros: word.start_us,
            end_micros: end,
        });
    }
    let profile = builder.finish(Some(&voice)).map_err(|e| refuse(&e))?;
    Dialogue::new(profile, spoken, Settings::default()).map_err(|e| refuse(&e))
}

/// Output instants where kept audio jumps: segment edges that are not a
/// source-contiguous continuation. Edges under other kept audio are returned
/// separately because the output sound there is a mix.
fn join_instants(expectations: &ReviewExpectations) -> (Vec<ExactMicros>, Vec<ExactMicros>) {
    let audio: Vec<&ReviewSegment> = expectations
        .segments
        .iter()
        .filter(|segment| segment.id.lane == ReviewLane::Audio)
        .collect();
    let follows = |end: &ReviewSegment, start: &ReviewSegment| {
        end.track == start.track
            && end.asset == start.asset
            && end.output.end == start.output.start
            && end.source.end == start.source.start
    };
    let mut joins = Vec::new();
    let mut mixed = Vec::new();
    for segment in &audio {
        for (at, is_start) in [(segment.output.start, true), (segment.output.end, false)] {
            let continued = audio.iter().any(|other| {
                if is_start {
                    follows(other, segment)
                } else {
                    follows(segment, other)
                }
            });
            if continued {
                continue;
            }
            let under_other = audio.iter().any(|other| {
                !std::ptr::eq(*other, *segment) && other.output.start < at && at < other.output.end
            });
            if under_other {
                mixed.push(at);
            } else {
                joins.push(at);
            }
        }
    }
    let order =
        |a: &ExactMicros, b: &ExactMicros| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal);
    joins.sort_by(order);
    joins.dedup();
    mixed.sort_by(order);
    mixed.dedup();
    (joins, mixed)
}

fn audit_joins(
    row: &mut ReviewRowReport,
    dialogue: &Dialogue,
    expectations: &ReviewExpectations,
    config: &RetainedSpeechConfig,
) -> u64 {
    let (joins, mixed) = join_instants(expectations);
    for at in &mixed {
        row.push(
            FindingKind::JoinUnderOtherAudio,
            Undecided,
            [at.floor(), at.ceil()],
        );
    }
    let duration = ExactMicros::integer(dialogue.profile().duration_micros());
    let mut edges = vec![ExactMicros::ZERO];
    edges.extend(
        joins
            .iter()
            .copied()
            .filter(|at| *at > ExactMicros::ZERO && *at < duration),
    );
    edges.push(duration);
    let cut: Vec<KeptRange> = edges
        .windows(2)
        .enumerate()
        .map(|(index, pair)| KeptRange {
            recording: RecordingId(u64::try_from(index).unwrap_or(u64::MAX)),
            source: AudibleSpan {
                start: pair[0],
                end: pair[1],
                padding: ExactMicros::ZERO,
            },
        })
        .collect();
    let recordings: BTreeMap<RecordingId, &Dialogue> = cut
        .iter()
        .map(|range| (range.recording, dialogue))
        .collect();
    let settings = AuditSettings {
        detail: Detail::Flags,
        max_items: config.max_join_items,
        ..AuditSettings::default()
    };
    let report = match audit::audit(&recordings, &cut, &settings, audit::comparison_key) {
        Ok(report) => report,
        Err(error) => {
            row.push(
                FindingKind::ProfileUnavailable {
                    reason: error.to_string(),
                },
                Undecided,
                [0, 0],
            );
            return 0;
        }
    };
    if report.items.len() >= config.max_join_items {
        row.push(
            FindingKind::JoinsOmitted {
                count: report.omitted.joins,
            },
            Undecided,
            [0, report.duration_us],
        );
    }
    for item in &report.items {
        let Item::Join(join) = item else { continue };
        for side in &join.sides {
            let Some(edge) = &side.edge_in_sound else {
                continue;
            };
            let span = [join.at_us, join.at_us];
            match &edge.reason {
                GuardIssue::Unmeasured => row.push(FindingKind::JoinUnmeasured, Undecided, span),
                issue => row.push(
                    FindingKind::JoinInSound {
                        issue: format!("{issue:?}"),
                    },
                    Failed,
                    span,
                ),
            }
        }
    }
    report.total.joins
}
