//! Bind stored source-clock transcripts to the admitted sources of a frozen version.
//!
//! A source with audio whose transcript is missing, or whose transcript does
//! not bind to the pinned content, the selected audio stream and the source
//! clock, contributes no transcript. Derivation then leaves its speech
//! uncovered, so the speech inventory is not determined (L2) and the speech
//! row cannot pass.
use super::{ReviewExpectationError, ReviewTranscript, digest, parse_digest, pin_for};
use kinewright_core::dialogue::Word;
use kinewright_core::output_review::{MAX_REVIEW_WORD_BYTES, ReviewSpan, TranscriptIdentity};
use kinewright_core::{Document, ExactMicros, MediaError, Rational, TimeCode};
use kinewright_media::StoredSourceTranscript;
use kinewright_project::owned_inputs::OwnedInputManifest;
use std::collections::BTreeSet;

/// Transcripts that bind to the frozen version, owned for derivation.
pub struct SourceTranscripts {
    bound: Vec<BoundTranscript>,
}

struct BoundTranscript {
    identity: TranscriptIdentity,
    words: Vec<Word>,
    recognized_source: ReviewSpan,
}

impl SourceTranscripts {
    /// Borrow every bound transcript as derivation input.
    #[must_use]
    pub fn review(&self) -> Vec<ReviewTranscript<'_>> {
        self.bound
            .iter()
            .map(|bound| ReviewTranscript {
                identity: bound.identity,
                words: &bound.words,
                recognized_source: Some(bound.recognized_source.clone()),
                landmarks: &[],
            })
            .collect()
    }
}

/// Read the stored transcript of each admitted source with audio that a clip
/// of `document` uses. `read` returns the store entry for a content digest
/// and source clock; this function never recognises speech.
///
/// # Errors
/// Refuses a source clock outside the review clock domain.
pub fn load_source_transcripts(
    document: &Document,
    manifest: &OwnedInputManifest,
    read: impl Fn(&str, Rational, TimeCode) -> Result<Option<StoredSourceTranscript>, MediaError>,
) -> Result<SourceTranscripts, ReviewExpectationError> {
    let used: BTreeSet<_> = document
        .tracks
        .iter()
        .flat_map(|track| &track.clips)
        .filter(|clip| clip.content.is_media())
        .map(|clip| clip.asset)
        .collect();
    let mut bound = Vec::new();
    for asset in document.media_pool.iter().filter(|a| used.contains(&a.id)) {
        let Some(entry) = pin_for(manifest, asset) else {
            continue;
        };
        let Ok(interpretation) = entry.media_interpretation() else {
            continue;
        };
        let (Some(audio), Some(source_sha256)) =
            (interpretation.audio, parse_digest(&entry.object.sha256))
        else {
            continue;
        };
        // A store failure leaves the speech of this source unchecked.
        let Ok(Some(stored)) = read(&entry.object.sha256, asset.fps, asset.duration) else {
            continue;
        };
        if stored.audio_stream != audio.index {
            continue;
        }
        let duration = ExactMicros::frames(asset.duration, asset.fps)?;
        if duration == ExactMicros::ZERO || !words_fit(&stored.words, duration) {
            continue;
        }
        let revision_sha256 = digest(&(&stored.recogniser_key, &stored.words))?;
        bound.push(BoundTranscript {
            identity: TranscriptIdentity {
                asset: asset.id,
                source_sha256,
                source_byte_length: entry.object.byte_length,
                audio_stream: audio.index,
                revision_sha256,
            },
            words: stored.words,
            recognized_source: ExactMicros::ZERO..duration,
        });
    }
    Ok(SourceTranscripts { bound })
}

/// The same word checks derivation applies, so a misfit transcript is left
/// out instead of refusing the whole review.
fn words_fit(words: &[Word], duration: ExactMicros) -> bool {
    words.iter().all(|word| {
        word.text.len() <= MAX_REVIEW_WORD_BYTES
            && !word.text.trim().is_empty()
            && word.start_micros < word.end_micros
            && word.end_micros <= duration.ceil()
    }) && words.windows(2).all(|pair| {
        pair[0].start_micros <= pair[1].start_micros && pair[0].end_micros <= pair[1].end_micros
    })
}
