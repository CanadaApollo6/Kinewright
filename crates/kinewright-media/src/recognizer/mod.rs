//! Local recognition sessions. Word clocks use integer source microseconds.
#![allow(clippy::missing_errors_doc)]
pub(crate) mod model;
mod pause;
pub(crate) mod queue;
use kinewright_core::ExportCancellation;
pub use model::{ENGLISH_MODEL, ModelError, ModelFile, ModelSpec, PARAKEET_MODEL, provision};
use serde::{Deserialize, Serialize};
use std::{fmt, path::Path};

pub const SAMPLE_RATE: u32 = 16_000;
pub const FEED_SAMPLES: usize = 2_560;

#[derive(Debug)]
pub enum Error {
    Cancelled,
    EmptyAudio,
    InvalidAudio,
    InvalidPolicy(&'static str),
    SessionOrder(&'static str),
    Clock(&'static str),
    Model(ModelError),
    Native(crate::nemo::Error),
    Decode(kinewright_core::MediaError),
    QueueClosed,
    UnsupportedLanguage,
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cancelled => f.write_str("recognition cancelled"),
            Self::EmptyAudio => f.write_str("recognition needs audio"),
            Self::InvalidAudio => {
                f.write_str("recognition needs finite mono audio in bounded feeds")
            }
            Self::InvalidPolicy(reason) => write!(f, "recognition policy: {reason}"),
            Self::SessionOrder(reason) => write!(f, "recognition session order: {reason}"),
            Self::Clock(reason) => write!(f, "recognition source clock: {reason}"),
            Self::Model(e) => e.fmt(f),
            Self::Native(e) => e.fmt(f),
            Self::Decode(e) => e.fmt(f),
            Self::QueueClosed => f.write_str("recognition worker stopped"),
            Self::UnsupportedLanguage => f.write_str("this recognition backend supports English"),
        }
    }
}
impl std::error::Error for Error {}
impl From<crate::nemo::Error> for Error {
    fn from(e: crate::nemo::Error) -> Self {
        Self::Native(e)
    }
}
impl From<kinewright_core::MediaError> for Error {
    fn from(e: kinewright_core::MediaError) -> Self {
        if e == kinewright_core::MediaError::Cancelled {
            Self::Cancelled
        } else {
            Self::Decode(e)
        }
    }
}
impl From<Error> for kinewright_core::MediaError {
    fn from(e: Error) -> Self {
        if matches!(e, Error::Cancelled) {
            Self::Cancelled
        } else {
            Self::Backend(e.to_string())
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PiecePolicy {
    pub target_us: u64,
    pub search_us: u64,
    pub level_window_us: u64,
    pub quiet_span_us: u64,
    pub pause_revision: u8,
}
impl Default for PiecePolicy {
    fn default() -> Self {
        Self {
            target_us: 60_000_000,
            search_us: 5_000_000,
            level_window_us: 10_000,
            quiet_span_us: 300_000,
            pause_revision: 1,
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Identity {
    pub model_id: String,
    pub file_sha256: String,
    pub runtime_version: String,
    pub build_key: String,
    pub lookahead: u8,
    pub threads: u8,
    pub feed_samples: usize,
    pub punctuation: bool,
    pub verbatim: bool,
    pub piece_policy: PiecePolicy,
}
impl Identity {
    #[must_use]
    pub fn english() -> Self {
        Self {
            model_id: ENGLISH_MODEL.id.into(),
            file_sha256: ENGLISH_MODEL.files[0].sha256.into(),
            runtime_version: crate::nemo::runtime_version(),
            build_key: crate::nemo::BUILD_KEY.into(),
            lookahead: 13,
            threads: crate::nemo::CPU_THREADS,
            feed_samples: FEED_SAMPLES,
            punctuation: crate::nemo::Options::default().punctuation,
            verbatim: crate::nemo::Options::default().verbatim,
            piece_policy: PiecePolicy::default(),
        }
    }
    fn validate(&self) -> Result<(), Error> {
        if self.feed_samples == 0 || self.feed_samples > FEED_SAMPLES {
            return Err(Error::InvalidPolicy(
                "feed size must be positive and within the native step bound",
            ));
        }
        self.piece_policy.buffer_samples().map(|_| ())
    }
    #[must_use]
    /// # Panics
    /// Panics if the fixed string-and-integer identity schema cannot serialize.
    pub fn cache_key(&self) -> String {
        crate::sha256::sha256_bytes(
            serde_json::to_string(self)
                .expect("recognizer identity serializes")
                .as_bytes(),
        )
    }
}

/// Token-duration times lag sound by 30–250 ms and scatter. They identify words;
/// they do not identify safe cut edges. DQ2 supplies measured sound boundaries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TimingSource {
    TokenDurationsLagged,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Word {
    pub text: String,
    pub start_us: u64,
    pub end_us: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Recognition {
    pub timing_source: TimingSource,
    pub words: Vec<Word>,
}
#[derive(Debug, PartialEq, Eq)]
pub enum Update {
    Interim {
        text: String,
        audio_processed_us: u64,
    },
    Committed {
        words: Vec<Word>,
        audio_processed_us: u64,
    },
}
impl Update {
    #[must_use]
    pub const fn audio_processed_us(&self) -> u64 {
        match self {
            Self::Interim {
                audio_processed_us, ..
            }
            | Self::Committed {
                audio_processed_us, ..
            } => *audio_processed_us,
        }
    }
}

pub trait Session {
    fn feed(&mut self, samples: &[f32]) -> Result<(), Error>;
    fn step(&mut self) -> Result<Option<Update>, Error>;
    /// Commit after feeding exactly through this source-clock position.
    fn commit_at(&mut self, source_position_us: u64) -> Result<(), Error>;
    fn finish(&mut self) -> Result<(), Error>;
}
pub trait Recognizer {
    fn identity(&self) -> &Identity;
    fn supports_streaming(&self) -> bool;
    /// The session borrows its loaded model, which must outlive the native stream.
    fn session(
        &mut self,
        origin_us: u64,
        cancel: ExportCancellation,
    ) -> Result<Box<dyn Session + '_>, Error>;
}

// Backends report times relative to the last commit window. Negative offsets
// are allowed for a streaming encoder's retained lookahead across a commit.
struct WindowWord {
    text: String,
    start_us: i64,
    end_us: i64,
}
struct Frame {
    final_result: bool,
    text: String,
    words: Vec<WindowWord>,
    processed_us: i64,
}
trait Backend {
    fn feed(&mut self, samples: &[f32]) -> Result<(), Error>;
    fn next(&mut self) -> Result<Option<Frame>, Error>;
    fn commit(&mut self, position_us: u64) -> Result<(), Error>;
    fn finish(&mut self) -> Result<(), Error>;
    fn close(&mut self);
}
#[derive(Clone, Copy)]
enum Phase {
    Feeding,
    Committing(u64),
    Finishing { sent: bool },
    Finished,
    Cancelled,
}
struct Driver<B> {
    backend: B,
    feed_samples: usize,
    phase: Phase,
    cancel: ExportCancellation,
    origin_us: u64,
    epoch_us: u64,
    samples: u64,
    processed_us: u64,
    last_word_start: Option<u64>,
}
impl<B: Backend> Driver<B> {
    fn new(backend: B, origin_us: u64, cancel: ExportCancellation, feed_samples: usize) -> Self {
        Self {
            backend,
            feed_samples,
            phase: Phase::Feeding,
            cancel,
            origin_us,
            epoch_us: 0,
            samples: 0,
            processed_us: origin_us,
            last_word_start: None,
        }
    }
    fn check(&mut self) -> Result<(), Error> {
        if cancelled(&self.cancel).is_err() || matches!(self.phase, Phase::Cancelled) {
            self.backend.close();
            self.phase = Phase::Cancelled;
            return Err(Error::Cancelled);
        }
        Ok(())
    }
    fn fed_us(&self) -> u64 {
        samples_us(self.samples)
    }
    fn relocate(&self, relative: i64) -> Result<u64, Error> {
        // A commit never resets the source clock. This is the only relocation layer.
        let absolute =
            i128::from(self.origin_us) + i128::from(self.epoch_us) + i128::from(relative);
        u64::try_from(absolute).map_err(|_| Error::Clock("time outside source clock"))
    }
}
impl<B: Backend> Session for Driver<B> {
    fn feed(&mut self, samples: &[f32]) -> Result<(), Error> {
        self.check()?;
        if !matches!(self.phase, Phase::Feeding) {
            return Err(Error::SessionOrder("feed requires an open session"));
        }
        if samples.is_empty() {
            return Err(Error::EmptyAudio);
        }
        if samples.len() > self.feed_samples || samples.iter().any(|s| !s.is_finite()) {
            return Err(Error::InvalidAudio);
        }
        self.backend.feed(samples)?;
        self.samples = self
            .samples
            .checked_add(u64::try_from(samples.len()).map_err(|_| Error::InvalidAudio)?)
            .ok_or(Error::Clock("sample count overflow"))?;
        Ok(())
    }
    fn commit_at(&mut self, source_position_us: u64) -> Result<(), Error> {
        self.check()?;
        if !matches!(self.phase, Phase::Feeding)
            || self.samples == 0
            || self.fed_us() <= self.epoch_us
        {
            return Err(Error::SessionOrder(
                "commit requires new audio and no pending final",
            ));
        }
        if self.origin_us.checked_add(self.fed_us()) != Some(source_position_us) {
            return Err(Error::SessionOrder("commit must be at the fed position"));
        }
        self.backend.commit(self.fed_us())?;
        self.phase = Phase::Committing(self.fed_us());
        Ok(())
    }
    fn finish(&mut self) -> Result<(), Error> {
        self.check()?;
        if !matches!(self.phase, Phase::Feeding) {
            return Err(Error::SessionOrder("finish requires an open session"));
        }
        if self.samples == 0 {
            return Err(Error::EmptyAudio);
        }
        self.phase = Phase::Finishing { sent: false };
        Ok(())
    }
    fn step(&mut self) -> Result<Option<Update>, Error> {
        self.check()?;
        if matches!(self.phase, Phase::Finished) {
            return Ok(None);
        }
        if matches!(self.phase, Phase::Finishing { sent: false }) {
            self.backend.finish()?;
            self.phase = Phase::Finishing { sent: true };
        }
        let Some(frame) = self.backend.next()? else {
            if matches!(self.phase, Phase::Finishing { .. } | Phase::Committing(_)) {
                self.backend.close();
                self.phase = Phase::Finished;
                return Err(Error::SessionOrder("backend omitted requested final"));
            }
            return Ok(None);
        };
        // An offline call may take several seconds; do not publish after cancellation.
        self.check()?;
        let native_processed = self.relocate(frame.processed_us)?;
        let end = self
            .origin_us
            .checked_add(self.fed_us())
            .ok_or(Error::Clock("source duration overflow"))?;
        self.processed_us = self.processed_us.max(native_processed.min(end));
        if !frame.final_result {
            return Ok(Some(Update::Interim {
                text: frame.text,
                audio_processed_us: self.processed_us,
            }));
        }
        let mut words = Vec::with_capacity(frame.words.len());
        for word in frame.words {
            let start_us = self.relocate(word.start_us)?;
            let end_us = self.relocate(word.end_us)?;
            if word.text.trim().is_empty()
                || end_us <= start_us
                || end_us > end
                || self.last_word_start.is_some_and(|last| start_us < last)
            {
                return Err(Error::Clock("invalid or unordered word times"));
            }
            self.last_word_start = Some(start_us);
            words.push(Word {
                text: word.text,
                start_us,
                end_us,
            });
        }
        match self.phase {
            Phase::Committing(next) => {
                self.epoch_us = next;
                self.phase = Phase::Feeding;
            }
            Phase::Finishing { .. } => {
                self.processed_us = end;
                self.phase = Phase::Finished;
                self.backend.close();
            }
            _ => return Err(Error::SessionOrder("unexpected backend final")),
        }
        Ok(Some(Update::Committed {
            words,
            audio_processed_us: self.processed_us,
        }))
    }
}
pub(crate) fn cancelled(cancel: &ExportCancellation) -> Result<(), Error> {
    if cancel.is_cancelled() || crate::locator::check().is_err() {
        Err(Error::Cancelled)
    } else {
        Ok(())
    }
}
fn samples_us(samples: u64) -> u64 {
    samples.saturating_mul(1_000_000) / u64::from(SAMPLE_RATE)
}

// The native stream clock is cumulative across force endpoints. A private
// stream boundary lets fast tests exercise this production conversion.
trait NativeStream {
    fn feed(&mut self, samples: &[f32]) -> Result<(), crate::nemo::Error>;
    fn step(&mut self) -> Result<Option<crate::nemo::Frame>, crate::nemo::Error>;
    fn commit(&mut self) -> Result<(), crate::nemo::Error>;
    fn finish(&mut self) -> Result<(), crate::nemo::Error>;
}
impl NativeStream for crate::nemo::Stream<'_> {
    fn feed(&mut self, samples: &[f32]) -> Result<(), crate::nemo::Error> {
        self.feed(samples)
    }
    fn step(&mut self) -> Result<Option<crate::nemo::Frame>, crate::nemo::Error> {
        self.step()
    }
    fn commit(&mut self) -> Result<(), crate::nemo::Error> {
        self.commit()
    }
    fn finish(&mut self) -> Result<(), crate::nemo::Error> {
        self.finish()
    }
}
struct NativeBackend<S> {
    stream: Option<S>,
    epoch_us: u64,
    pending: Option<u64>,
}
impl<S: NativeStream> NativeBackend<S> {
    fn open(
        identity: &Identity,
        open: impl FnOnce(crate::nemo::Options) -> Result<S, Error>,
    ) -> Result<Self, Error> {
        identity.validate()?;
        let stream = open(crate::nemo::Options {
            punctuation: identity.punctuation,
            verbatim: identity.verbatim,
        })?;
        Ok(Self {
            stream: Some(stream),
            epoch_us: 0,
            pending: None,
        })
    }
}
impl<S: NativeStream> Backend for NativeBackend<S> {
    fn feed(&mut self, samples: &[f32]) -> Result<(), Error> {
        self.stream
            .as_mut()
            .ok_or(Error::SessionOrder("closed stream"))?
            .feed(samples)
            .map_err(Into::into)
    }
    fn next(&mut self) -> Result<Option<Frame>, Error> {
        let Some(raw) = self
            .stream
            .as_mut()
            .ok_or(Error::SessionOrder("closed stream"))?
            .step()?
        else {
            return Ok(None);
        };
        let epoch =
            i64::try_from(self.epoch_us).map_err(|_| Error::Clock("commit offset overflow"))?;
        let frame = Frame {
            final_result: raw.final_result,
            text: raw.text,
            words: raw
                .words
                .into_iter()
                .map(|w| WindowWord {
                    text: w.text,
                    start_us: w.start_us - epoch,
                    end_us: w.end_us - epoch,
                })
                .collect(),
            processed_us: raw.audio_processed_us - epoch,
        };
        if frame.final_result
            && let Some(next) = self.pending.take()
        {
            self.epoch_us = next;
        }
        Ok(Some(frame))
    }
    fn commit(&mut self, position_us: u64) -> Result<(), Error> {
        self.stream
            .as_mut()
            .ok_or(Error::SessionOrder("closed stream"))?
            .commit()?;
        self.pending = Some(position_us);
        Ok(())
    }
    fn finish(&mut self) -> Result<(), Error> {
        self.stream
            .as_mut()
            .ok_or(Error::SessionOrder("closed stream"))?
            .finish()
            .map_err(Into::into)
    }
    fn close(&mut self) {
        self.stream = None;
    }
}

// The checked file and the backend-opening operation share this boundary;
// the fast tier can prove that an invalidated file never opens a session.
fn checked_session<S>(
    verified: &model::VerifiedFile,
    open: impl FnOnce() -> Result<S, Error>,
) -> Result<S, Error> {
    verified.check()?;
    open()
}

pub struct BuiltinRecognizer {
    native: crate::nemo::Recognizer,
    identity: Identity,
    verified: model::VerifiedFile,
}
impl BuiltinRecognizer {
    pub fn load(path: &Path, receipts: &Path, cancel: &ExportCancellation) -> Result<Self, Error> {
        Self::load_identity(path, receipts, cancel, Identity::english())
    }
    fn load_identity(
        path: &Path,
        receipts: &Path,
        cancel: &ExportCancellation,
        identity: Identity,
    ) -> Result<Self, Error> {
        identity.validate()?;
        let verified = model::verify(&ENGLISH_MODEL.files[0], path, receipts, cancel)?;
        let native = crate::nemo::Recognizer::load_config(
            verified.path(),
            identity.threads,
            identity.lookahead,
        )?;
        verified.check()?;
        Ok(Self {
            native,
            identity,
            verified,
        })
    }
}
impl Recognizer for BuiltinRecognizer {
    fn identity(&self) -> &Identity {
        &self.identity
    }
    fn supports_streaming(&self) -> bool {
        true
    }
    fn session(
        &mut self,
        origin_us: u64,
        cancel: ExportCancellation,
    ) -> Result<Box<dyn Session + '_>, Error> {
        cancelled(&cancel)?;
        let feed_samples = self.identity.feed_samples;
        checked_session(&self.verified, || {
            let backend = NativeBackend::open(&self.identity, |options| {
                self.native.stream(options).map_err(Into::into)
            })?;
            Ok(
                Box::new(Driver::new(backend, origin_us, cancel, feed_samples))
                    as Box<dyn Session + '_>,
            )
        })
    }
}

#[cfg(test)]
mod real;
#[cfg(test)]
pub(crate) mod tests;
