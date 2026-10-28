//! Bounded voice inference and commentary. The project owner retains audio and ASR.
#![allow(clippy::missing_errors_doc)]
use kinewright_core::{ExportCancellation, MediaError};
use serde::{Deserialize, Serialize};
use std::path::Path;

pub const MAX_VOICE_MICROS: u64 = 120_000_000;
pub const MAX_PLAYHEAD_ENTRIES: usize = 8192;
pub const MAX_VOICE_TEXT_BYTES: usize = 16_384;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VoiceJobKey<P, N, J> {
    pub project: P,
    pub note: N,
    pub job: J,
    pub audio_sha256: String,
    pub transcript_revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VoiceWord {
    pub text: String,
    pub start_micros: u64,
    pub end_micros: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "state",
    content = "words",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum VoiceTranscript {
    AwaitingModel,
    NoSpeech,
    Ready(Vec<VoiceWord>),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OriginalVoiceAsr<P, N, J, A> {
    pub version: u32,
    pub job: VoiceJobKey<P, N, J>,
    pub anchor: A,
    pub playhead_log_sha256: String,
    pub model_sha256: String,
    pub transcript: VoiceTranscript,
}

impl VoiceTranscript {
    pub fn validate(&self, duration_micros: u64) -> Result<(), MediaError> {
        if duration_micros == 0 || duration_micros > MAX_VOICE_MICROS {
            return Err(MediaError::Backend("invalid voice duration".into()));
        }
        if let Self::Ready(words) = self
            && (words.is_empty()
                || words.len() > 4096
                || words.iter().map(|w| w.text.len()).sum::<usize>() > MAX_VOICE_TEXT_BYTES
                || words.iter().any(|w| {
                    w.text.trim().is_empty()
                        || w.start_micros >= w.end_micros
                        || w.end_micros > duration_micros
                })
                || words
                    .windows(2)
                    .any(|w| w[0].end_micros > w[1].start_micros))
        {
            return Err(MediaError::Backend(
                "invalid bounded voice transcript".into(),
            ));
        }
        Ok(())
    }
}

pub struct VoiceJob<K> {
    key: K,
    cancellation: ExportCancellation,
    completed: bool,
}
impl<K> VoiceJob<K> {
    #[must_use]
    pub fn new(key: K) -> Self {
        Self {
            key,
            cancellation: ExportCancellation::default(),
            completed: false,
        }
    }
    #[must_use]
    pub fn cancellation(&self) -> ExportCancellation {
        self.cancellation.clone()
    }
    pub fn cancel(&self) {
        self.cancellation.cancel();
    }
    #[must_use]
    pub fn key(&self) -> &K {
        &self.key
    }
    #[must_use]
    pub fn is_active(&self) -> bool {
        !self.completed && !self.cancellation.is_cancelled()
    }
}
impl<P: PartialEq, N: PartialEq, J: PartialEq> VoiceJob<VoiceJobKey<P, N, J>> {
    /// Completion and the live note state must still match the original job.
    pub fn accept(
        &mut self,
        key: &VoiceJobKey<P, N, J>,
        current: &VoiceJobKey<P, N, J>,
        transcript: VoiceTranscript,
    ) -> Option<VoiceTranscript> {
        if self.completed
            || self.cancellation.is_cancelled()
            || self.key.project != key.project
            || self.key.note != key.note
            || self.key.job != key.job
            || self.key.audio_sha256 != key.audio_sha256
            || self.key.transcript_revision != key.transcript_revision
            || &self.key != current
        {
            return None;
        }
        self.completed = true;
        Some(transcript)
    }
}
impl<K> Drop for VoiceJob<K> {
    fn drop(&mut self) {
        self.cancellation.cancel();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlayheadEvent {
    Display,
    Play,
    Pause,
    Seek,
    VersionSwitch,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlayheadSample<A> {
    pub audio_micros: u64,
    pub displayed: A,
    pub event: PlayheadEvent,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureStatus {
    pub complete: bool,
    pub capture_error: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlayheadLog<A> {
    pub version: u32,
    pub samples: Vec<PlayheadSample<A>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capture_status: Option<CaptureStatus>,
}
impl<A> PlayheadLog<A> {
    pub fn new(displayed: A) -> Self {
        Self {
            version: 1,
            capture_status: None,
            samples: vec![PlayheadSample {
                audio_micros: 0,
                displayed,
                event: PlayheadEvent::Display,
            }],
        }
    }
    pub fn push(&mut self, sample: PlayheadSample<A>) -> Result<(), MediaError> {
        if self.samples.len() >= MAX_PLAYHEAD_ENTRIES
            || sample.audio_micros > MAX_VOICE_MICROS
            || self
                .samples
                .last()
                .is_some_and(|last| sample.audio_micros < last.audio_micros)
        {
            return Err(MediaError::Backend(
                "commentary log capacity or audio clock".into(),
            ));
        }
        self.samples.push(sample);
        Ok(())
    }
    pub fn validate(&self, duration_micros: u64) -> Result<(), MediaError> {
        if !matches!(self.version, 1 | 2)
            || (self.version == 2 && self.capture_status.is_none())
            || self
                .capture_status
                .as_ref()
                .and_then(|status| status.capture_error.as_ref())
                .is_some_and(|error| error.len() > MAX_VOICE_TEXT_BYTES)
            || duration_micros == 0
            || duration_micros > MAX_VOICE_MICROS
            || self.samples.is_empty()
            || self.samples.len() > MAX_PLAYHEAD_ENTRIES
            || self.samples[0].audio_micros != 0
            || self
                .samples
                .last()
                .is_some_and(|s| s.audio_micros > duration_micros)
            || self
                .samples
                .windows(2)
                .any(|w| w[0].audio_micros > w[1].audio_micros)
        {
            return Err(MediaError::Backend(
                "invalid commentary playhead log".into(),
            ));
        }
        Ok(())
    }
    #[must_use]
    pub fn anchor_at(&self, audio_micros: u64) -> Option<&A> {
        self.samples
            .iter()
            .rev()
            .find(|s| s.audio_micros <= audio_micros)
            .map(|s| &s.displayed)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommentaryRemark<A> {
    pub text: String,
    pub start_micros: u64,
    pub end_micros: u64,
    pub playhead_anchor: A,
    pub crossed_events: Vec<PlayheadSample<A>>,
}

pub fn commentary_remarks<A: Clone>(
    words: &[VoiceWord],
    log: &PlayheadLog<A>,
    duration_micros: u64,
) -> Result<Vec<CommentaryRemark<A>>, MediaError> {
    log.validate(duration_micros)?;
    if words.len() > 4096
        || words.iter().map(|w| w.text.len()).sum::<usize>() > MAX_VOICE_TEXT_BYTES
        || words
            .iter()
            .any(|w| w.start_micros >= w.end_micros || w.end_micros > duration_micros)
        || words
            .windows(2)
            .any(|w| w[0].end_micros > w[1].start_micros)
    {
        return Err(MediaError::Backend(
            "invalid bounded voice transcript".into(),
        ));
    }
    let mut result = Vec::new();
    let mut start = 0;
    for (i, word) in words.iter().enumerate() {
        let sentence = word.text.ends_with(['.', '!', '?']);
        let pause = words
            .get(i + 1)
            .is_none_or(|next| next.start_micros.saturating_sub(word.end_micros) >= 600_000);
        if sentence || pause {
            let first = &words[start];
            let anchor = log
                .anchor_at(first.start_micros)
                .ok_or_else(|| MediaError::Backend("missing commentary start anchor".into()))?;
            result.push(CommentaryRemark {
                text: words[start..=i]
                    .iter()
                    .map(|w| w.text.as_str())
                    .collect::<Vec<_>>()
                    .join(" "),
                start_micros: first.start_micros,
                end_micros: word.end_micros,
                playhead_anchor: anchor.clone(),
                crossed_events: log
                    .samples
                    .iter()
                    .filter(|s| {
                        s.audio_micros > first.start_micros
                            && s.audio_micros < word.end_micros
                            && s.event != PlayheadEvent::Display
                    })
                    .cloned()
                    .collect(),
            });
            start = i + 1;
        }
    }
    Ok(result)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpokenTimecode {
    Absent,
    Parsed(u64),
    Ambiguous,
    OutOfRange,
}

/// English phrases are recognized only after an explicit temporal cue.
#[must_use]
pub fn spoken_timecode(text: &str, program_duration_micros: u64) -> SpokenTimecode {
    let normalized = text.to_ascii_lowercase();
    let tokens: Vec<_> = normalized
        .split_whitespace()
        .map(|w| w.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != ':' && c != '-'))
        .collect();
    let mut found = Vec::new();
    for (i, token) in tokens.iter().enumerate() {
        if *token == "at" {
            let tail = &tokens[i + 1..];
            found.push(parse_time(tail));
        } else if matches!(*token, "minute" | "minutes")
            && tokens.get(i + 1) == Some(&"in")
            && i > 0
        {
            let last = number(tokens[i - 1]);
            let preceding = i.checked_sub(2).and_then(|j| number(tokens[j]));
            let value = match (preceding, last) {
                (Some(tens), Some(units)) if tens >= 20 && tens % 10 == 0 && units < 10 => {
                    tens.checked_add(units)
                }
                (Some(_), _) => None,
                (None, value) => value,
            };
            found.push(
                if tokens.get(i + 2).is_some_and(|token| time_suffix(token)) {
                    None
                } else {
                    value.and_then(|n| n.checked_mul(60))
                },
            );
        }
    }
    found.sort_unstable();
    found.dedup();
    if found.is_empty() {
        if tokens.windows(3).any(|part| {
            number(part[0]).is_some()
                && number(part[1]).is_some()
                && matches!(part[2], "second" | "seconds" | "minute" | "minutes")
        }) {
            return SpokenTimecode::Ambiguous;
        }
        return SpokenTimecode::Absent;
    }
    if found.len() != 1 {
        return SpokenTimecode::Ambiguous;
    }
    let Some(micros) = found[0].and_then(|seconds| seconds.checked_mul(1_000_000)) else {
        return SpokenTimecode::Ambiguous;
    };
    if micros >= program_duration_micros {
        SpokenTimecode::OutOfRange
    } else {
        SpokenTimecode::Parsed(micros)
    }
}
fn number(token: &str) -> Option<u64> {
    match token {
        "zero" => Some(0),
        "one" => Some(1),
        "two" => Some(2),
        "three" => Some(3),
        "four" => Some(4),
        "five" => Some(5),
        "six" => Some(6),
        "seven" => Some(7),
        "eight" => Some(8),
        "nine" => Some(9),
        "ten" => Some(10),
        "eleven" => Some(11),
        "twelve" => Some(12),
        "thirteen" => Some(13),
        "fourteen" => Some(14),
        "fifteen" => Some(15),
        "sixteen" => Some(16),
        "seventeen" => Some(17),
        "eighteen" => Some(18),
        "nineteen" => Some(19),
        "twenty" => Some(20),
        "thirty" => Some(30),
        "forty" => Some(40),
        "fifty" => Some(50),
        _ => token.parse().ok(),
    }
}
fn parse_time(tokens: &[&str]) -> Option<u64> {
    let (seconds, consumed) = parse_time_prefix(tokens)?;
    if tokens.get(consumed).is_some_and(|token| time_suffix(token)) {
        return None;
    }
    Some(seconds)
}
fn time_suffix(token: &str) -> bool {
    number(token).is_some()
        || token.contains(':')
        || matches!(
            token,
            "second" | "seconds" | "minute" | "minutes" | "and" | "or" | "to" | "in"
        )
}
fn parse_time_prefix(tokens: &[&str]) -> Option<(u64, usize)> {
    let first = *tokens.first()?;
    if let Some((minutes, seconds)) = first.split_once(':') {
        if seconds.len() != 2 {
            return None;
        }
        let seconds: u64 = seconds.parse().ok()?;
        return (seconds < 60).then_some(()).and_then(|()| {
            minutes
                .parse::<u64>()
                .ok()?
                .checked_mul(60)?
                .checked_add(seconds)
                .map(|value| (value, 1))
        });
    }
    let a = number(first)?;
    if let (Some(second), Some(unit)) = (tokens.get(1), tokens.get(2))
        && let Some(b) = number(second)
        && a >= 20
        && a % 10 == 0
        && b < 10
        && matches!(*unit, "second" | "seconds" | "minute" | "minutes")
    {
        let value = if unit.starts_with("minute") {
            a.checked_add(b)?.checked_mul(60)
        } else {
            a.checked_add(b)
        }?;
        return Some((value, if tokens.get(3) == Some(&"in") { 4 } else { 3 }));
    }
    match tokens.get(1).copied() {
        Some("second" | "seconds") => Some((a, 2)),
        Some("minute" | "minutes") => {
            let minutes = a.checked_mul(60)?;
            if let Some(seconds) = tokens.get(2).and_then(|t| number(t)) {
                if seconds >= 60 || !matches!(tokens.get(3).copied(), Some("second" | "seconds")) {
                    return None;
                }
                minutes.checked_add(seconds).map(|value| (value, 4))
            } else {
                Some((minutes, if tokens.get(2) == Some(&"in") { 3 } else { 2 }))
            }
        }
        Some(second) => {
            let b = number(second)?;
            if b >= 60
                || (a >= 20 && a % 10 == 0 && b < 10)
                || tokens.get(2).is_some_and(|t| number(t).is_some())
            {
                return None;
            }
            a.checked_mul(60)?.checked_add(b).map(|value| (value, 2))
        }
        None => None,
    }
}

struct InferenceSlots {
    active: bool,
    pending: bool,
}
static INFERENCE_SLOTS: std::sync::Mutex<InferenceSlots> = std::sync::Mutex::new(InferenceSlots {
    active: false,
    pending: false,
});
static INFERENCE_READY: std::sync::Condvar = std::sync::Condvar::new();
#[derive(Clone, Copy)]
enum InferenceSlot {
    Active,
    Pending,
}
struct InferencePermit {
    slot: InferenceSlot,
}
impl InferencePermit {
    fn acquire() -> Result<Self, MediaError> {
        let mut slots = INFERENCE_SLOTS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let slot = match (slots.active, slots.pending) {
            (false, false) => {
                slots.active = true;
                InferenceSlot::Active
            }
            (true, false) => {
                slots.pending = true;
                InferenceSlot::Pending
            }
            _ => {
                return Err(MediaError::Backend(
                    "voice inference and its pending slot are full; audio remains saved".into(),
                ));
            }
        };
        Ok(Self { slot })
    }
    fn wait(&mut self, cancellation: &ExportCancellation) -> Result<(), MediaError> {
        if cancellation.is_cancelled() {
            return Err(MediaError::Cancelled);
        }
        if matches!(self.slot, InferenceSlot::Pending) {
            let mut slots = INFERENCE_SLOTS
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            while slots.active {
                if cancellation.is_cancelled() {
                    return Err(MediaError::Cancelled);
                }
                slots = INFERENCE_READY
                    .wait_timeout(slots, std::time::Duration::from_millis(20))
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .0;
            }
            if cancellation.is_cancelled() {
                return Err(MediaError::Cancelled);
            }
            slots.pending = false;
            slots.active = true;
            self.slot = InferenceSlot::Active;
        }
        Ok(())
    }
}
impl Drop for InferencePermit {
    fn drop(&mut self) {
        let mut slots = INFERENCE_SLOTS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match self.slot {
            InferenceSlot::Active => slots.active = false,
            InferenceSlot::Pending => slots.pending = false,
        }
        INFERENCE_READY.notify_all();
    }
}

/// Input is the sealed note's original PCM WAV bytes, not an external locator.
pub fn transcribe_voice(
    data_dir: &Path,
    audio_sha256: &str,
    audio: &[u8],
    cancellation: &ExportCancellation,
) -> Result<VoiceTranscript, MediaError> {
    let mut permit = InferencePermit::acquire()?;
    permit.wait(cancellation)?;
    transcribe_reserved(data_dir, audio_sha256, audio, cancellation)
}
fn transcribe_reserved(
    data_dir: &Path,
    audio_sha256: &str,
    audio: &[u8],
    cancellation: &ExportCancellation,
) -> Result<VoiceTranscript, MediaError> {
    if crate::sha256_bytes(audio) != audio_sha256 {
        return Err(MediaError::Backend("voice audio identity mismatch".into()));
    }
    crate::transcript::transcribe_voice_wav(data_dir, audio_sha256, audio, cancellation)
}

pub(crate) fn voice_pcm(audio: &[u8]) -> Result<(Vec<f32>, u64), MediaError> {
    let invalid =
        || MediaError::Backend("voice capture requires a complete mono 16 kHz PCM16 WAV".into());
    if audio.len() < 44
        || audio.len() > 3_840_000 + 65_536
        || &audio[..4] != b"RIFF"
        || &audio[8..12] != b"WAVE"
    {
        return Err(invalid());
    }
    let u32_at = |at: usize| -> Option<u32> {
        Some(u32::from_le_bytes(audio.get(at..at + 4)?.try_into().ok()?))
    };
    if u32_at(4)
        .and_then(|n| usize::try_from(n).ok())
        .and_then(|n| n.checked_add(8))
        != Some(audio.len())
    {
        return Err(invalid());
    }
    let mut at: usize = 12;
    let mut format = false;
    let mut data = None;
    while at < audio.len() {
        let size = u32_at(at + 4)
            .and_then(|n| usize::try_from(n).ok())
            .ok_or_else(invalid)?;
        let start = at.checked_add(8).ok_or_else(invalid)?;
        let end = start.checked_add(size).ok_or_else(invalid)?;
        let chunk = audio.get(start..end).ok_or_else(invalid)?;
        match audio.get(at..at + 4) {
            Some(b"fmt ") => {
                if format
                    || chunk.len() < 16
                    || chunk[..2] != [1, 0]
                    || chunk[2..4] != [1, 0]
                    || chunk[4..8] != 16000_u32.to_le_bytes()
                    || chunk[8..12] != 32000_u32.to_le_bytes()
                    || chunk[12..14] != [2, 0]
                    || chunk[14..16] != [16, 0]
                {
                    return Err(invalid());
                }
                format = true;
            }
            Some(b"data") if data.is_some() => return Err(invalid()),
            Some(b"data") => data = Some(chunk),
            _ => {}
        }
        at = end.checked_add(size % 2).ok_or_else(invalid)?;
    }
    let data = data.ok_or_else(invalid)?;
    if !format
        || at != audio.len()
        || data.is_empty()
        || data.len() % 2 != 0
        || data.len() > 3_840_000
    {
        return Err(invalid());
    }
    let duration = u64::try_from(data.len() / 2).map_err(|_| invalid())? * 1_000_000 / 16_000;
    let samples = data
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| f32::from(i16::from_le_bytes([b[0], b[1]])) / 32768.0)
        .collect();
    Ok((samples, duration))
}

pub fn voice_audio_duration(audio: &[u8]) -> Result<u64, MediaError> {
    voice_pcm(audio).map(|(_, duration)| duration)
}

pub struct VoiceCompletion<K> {
    pub key: K,
    pub result: Result<VoiceTranscript, MediaError>,
}
pub struct VoiceTask<K> {
    result: std::sync::mpsc::Receiver<VoiceCompletion<K>>,
    cancellation: ExportCancellation,
    completed: bool,
}
impl<K> VoiceTask<K> {
    pub fn try_complete(&mut self) -> Result<Option<VoiceCompletion<K>>, MediaError> {
        match self.result.try_recv() {
            Ok(completion) => {
                self.completed = true;
                Ok(Some(completion))
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => Ok(None),
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Err(MediaError::Backend(
                "voice inference worker disconnected".into(),
            )),
        }
    }
}
impl<K> Drop for VoiceTask<K> {
    fn drop(&mut self) {
        if !self.completed {
            self.cancellation.cancel();
        }
    }
}

/// Moving the owner's read reservation into this worker retains its measured bytes.
pub fn spawn_voice_transcription<P, N, J, B>(
    key: VoiceJobKey<P, N, J>,
    audio: B,
    data_dir: std::path::PathBuf,
    cancellation: ExportCancellation,
) -> Result<VoiceTask<VoiceJobKey<P, N, J>>, MediaError>
where
    P: Send + 'static,
    N: Send + 'static,
    J: Send + 'static,
    B: std::ops::Deref<Target = Vec<u8>> + Send + 'static,
{
    let permit = InferencePermit::acquire()?;
    let (tx, result) = std::sync::mpsc::sync_channel(1);
    let inference_cancel = cancellation.clone();
    std::thread::Builder::new()
        .name("voice-note-whisper".into())
        .spawn(move || {
            let mut permit = permit;
            let result = permit.wait(&inference_cancel).and_then(|()| {
                transcribe_reserved(&data_dir, &key.audio_sha256, &audio, &inference_cancel)
            });
            let _ = tx.send(VoiceCompletion { key, result });
        })
        .map_err(|e| MediaError::Backend(e.to_string()))?;
    Ok(VoiceTask {
        result,
        cancellation,
        completed: false,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDirectory;
    use std::sync::{Arc, Barrier};
    static INFERENCE_TESTS: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn key() -> VoiceJobKey<u8, u8, u8> {
        VoiceJobKey {
            project: 1,
            note: 2,
            job: 3,
            audio_sha256: "a".repeat(64),
            transcript_revision: 4,
        }
    }
    fn transcript() -> VoiceTranscript {
        VoiceTranscript::Ready(vec![VoiceWord {
            text: "make this warmer".into(),
            start_micros: 100_000,
            end_micros: 500_000,
        }])
    }
    fn wav(silent: bool) -> Vec<u8> {
        let mut bytes = Vec::new();
        let data_bytes: u32 = 32_000;
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + data_bytes).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&16000u32.to_le_bytes());
        bytes.extend_from_slice(&32000u32.to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&data_bytes.to_le_bytes());
        for _ in 0..16000 {
            bytes.extend_from_slice(&(if silent { 0i16 } else { 1000i16 }).to_le_bytes());
        }
        bytes
    }
    #[test]
    fn s3_voice_job_identity() {
        let original = key();
        let mut variants = Vec::new();
        let mut other = original.clone();
        other.project += 1;
        variants.push(other);
        let mut other = original.clone();
        other.note += 1;
        variants.push(other);
        let mut other = original.clone();
        other.job += 1;
        variants.push(other);
        let mut other = original.clone();
        other.audio_sha256 = "b".repeat(64);
        variants.push(other);
        let mut other = original.clone();
        other.transcript_revision += 1;
        variants.push(other);
        for late in variants {
            let mut job = VoiceJob::new(original.clone());
            assert!(
                job.accept(&late, &original, transcript()).is_none(),
                "S3_VOICE_COMPLETION_IDENTITY"
            );
        }
        let mut job = VoiceJob::new(original.clone());
        let mut edited = original.clone();
        edited.transcript_revision += 1;
        assert!(
            job.accept(&original, &edited, transcript()).is_none(),
            "S3_VOICE_LIVE_REVISION"
        );
        let barrier = Arc::new(Barrier::new(2));
        let inference_barrier = barrier.clone();
        let inference = std::thread::spawn(move || {
            inference_barrier.wait();
            inference_barrier.wait();
            transcript()
        });
        barrier.wait();
        job.cancel();
        barrier.wait();
        assert!(
            job.accept(&original, &original, inference.join().unwrap())
                .is_none(),
            "S3_VOICE_CANCELLED_LATE"
        );
        let mut successful = VoiceJob::new(original.clone());
        assert_eq!(
            successful.accept(&original, &original, transcript()),
            Some(transcript())
        );
        assert!(
            successful
                .accept(&original, &original, transcript())
                .is_none(),
            "S3_VOICE_ONE_COMPLETION"
        );
    }
    #[test]
    fn s3_commentary_playhead_log() {
        let mut log = PlayheadLog::new((1, 10));
        for (audio_micros, displayed, event) in [
            (500_000, (1, 20), PlayheadEvent::Pause),
            (800_000, (1, 99), PlayheadEvent::Seek),
            (1_200_000, (2, 3), PlayheadEvent::VersionSwitch),
            (1_400_000, (2, 4), PlayheadEvent::Play),
            (1_800_000, (2, 15), PlayheadEvent::Display),
        ] {
            log.push(PlayheadSample {
                audio_micros,
                displayed,
                event,
            })
            .unwrap();
        }
        let words = [
            VoiceWord {
                text: "change".into(),
                start_micros: 100_000,
                end_micros: 400_000,
            },
            VoiceWord {
                text: "this.".into(),
                start_micros: 450_000,
                end_micros: 950_000,
            },
            VoiceWord {
                text: "other.".into(),
                start_micros: 1_300_000,
                end_micros: 1_600_000,
            },
        ];
        let remarks = commentary_remarks(&words, &log, 2_000_000).unwrap();
        assert_eq!(remarks.len(), 2);
        assert_eq!(remarks[0].playhead_anchor, (1, 10));
        assert_eq!(
            (remarks[0].start_micros, remarks[0].end_micros),
            (100_000, 950_000)
        );
        assert_eq!(
            remarks[0]
                .crossed_events
                .iter()
                .map(|e| e.event)
                .collect::<Vec<_>>(),
            vec![PlayheadEvent::Pause, PlayheadEvent::Seek]
        );
        assert_eq!(remarks[1].playhead_anchor, (2, 3));
        assert_eq!(log.anchor_at(800_000), Some(&(1, 99)));
        let before = log.clone();
        assert!(
            log.push(PlayheadSample {
                audio_micros: 1,
                displayed: (9, 9),
                event: PlayheadEvent::Seek
            })
            .is_err()
        );
        assert_eq!(log, before);
        assert!(
            commentary_remarks::<(i32, i32)>(&[], &log, 2_000_000)
                .unwrap()
                .is_empty()
        );
        while log.samples.len() < MAX_PLAYHEAD_ENTRIES {
            log.push(PlayheadSample {
                audio_micros: 1_900_000,
                displayed: (2, 15),
                event: PlayheadEvent::Display,
            })
            .unwrap();
        }
        assert!(
            log.push(PlayheadSample {
                audio_micros: 2_000_000,
                displayed: (2, 16),
                event: PlayheadEvent::Display
            })
            .is_err()
        );
    }
    #[test]
    fn s3_commentary_spoken_timecodes() {
        for (text, seconds) in [
            ("at 22 seconds, soften it", 22),
            ("at one ten", 70),
            ("at 1:10", 70),
            ("two minutes in", 120),
            ("at twenty two seconds", 22),
            ("at one minute ten seconds", 70),
            ("at two minutes in", 120),
            ("at thirteen seconds", 13),
        ] {
            assert_eq!(
                spoken_timecode(text, 300_000_000),
                SpokenTimecode::Parsed(seconds * 1_000_000),
                "{text}"
            );
        }
        for text in [
            "at one",
            "at 1:70",
            "at -1 seconds",
            "at one ten or one twenty",
            "at 1:10 or 1:20",
            "at 22 seconds and 35 seconds",
            "at twenty two",
            "at twenty two minutes ten seconds",
            "at one ten seconds",
            "twenty two minutes ten seconds",
            "one ten seconds",
            "two minutes in ten seconds",
            "at two minutes in ten seconds",
        ] {
            assert_eq!(
                spoken_timecode(text, 3_600_000_000),
                SpokenTimecode::Ambiguous,
                "{text}"
            );
        }
        assert_eq!(
            spoken_timecode("at 1:10", 60_000_000),
            SpokenTimecode::OutOfRange
        );
        assert_eq!(
            spoken_timecode("at 60 seconds", 60_000_000),
            SpokenTimecode::OutOfRange
        );
        assert_eq!(
            spoken_timecode("twenty two minutes in", 300_000_000),
            SpokenTimecode::OutOfRange
        );
        assert_eq!(
            spoken_timecode("soften this chart", 60_000_000),
            SpokenTimecode::Absent
        );
    }
    #[test]
    fn s3_voice_missing_model_silence_and_sealed_audio() {
        let _inference = INFERENCE_TESTS.lock().unwrap();
        let temp = TempDirectory::new("voice-no-model");
        let audio = wav(false);
        let hash = crate::sha256_bytes(&audio);
        assert_eq!(
            transcribe_voice(temp.root(), &hash, &audio, &ExportCancellation::default()).unwrap(),
            VoiceTranscript::AwaitingModel
        );
        assert!(!temp.path("models").exists(), "S3_VOICE_NO_DOWNLOAD");
        let silent = wav(true);
        assert_eq!(
            transcribe_voice(
                temp.root(),
                &crate::sha256_bytes(&silent),
                &silent,
                &ExportCancellation::default()
            )
            .unwrap(),
            VoiceTranscript::NoSpeech
        );
        assert!(
            transcribe_voice(temp.root(), &hash, &silent, &ExportCancellation::default()).is_err()
        );
        assert!(voice_audio_duration(&audio[..audio.len() - 1]).is_err());
        assert_eq!(voice_audio_duration(&audio).unwrap(), 1_000_000);
    }
    #[test]
    fn s3_voice_inference_admission_is_bounded() {
        let _inference = INFERENCE_TESTS.lock().unwrap();
        let mut active = InferencePermit::acquire().unwrap();
        active.wait(&ExportCancellation::default()).unwrap();
        let mut pending = InferencePermit::acquire().unwrap();
        assert!(
            InferencePermit::acquire().is_err(),
            "S3_VOICE_ONE_PENDING_SLOT"
        );
        let cancellation = ExportCancellation::default();
        let cancel = cancellation.clone();
        let worker = std::thread::spawn(move || pending.wait(&cancel));
        cancellation.cancel();
        assert_eq!(worker.join().unwrap(), Err(MediaError::Cancelled));
        assert!(INFERENCE_SLOTS.lock().unwrap().active);
        drop(active);
        let mut next = InferencePermit::acquire().unwrap();
        next.wait(&ExportCancellation::default()).unwrap();
    }
}
