use openreel_core::Analysis;
use openreel_media::FfmpegMediaEngine;

#[path = "../src/test_support.rs"]
pub mod test_support;
use test_support::{SpeechClip, normalized_words, test_engine, wait_for_transcript};

const TTS_TEXT: &str = "Hello, um, this is an Open Reel transcript test.";

#[test]
fn windows_sapi_speech_is_transcribed_by_the_real_model() {
    if std::env::var("OPENREEL_TRANSCRIPT_TEST").as_deref() != Ok("1") {
        eprintln!("skipped: set OPENREEL_TRANSCRIPT_TEST=1 to run SAPI and local Whisper");
        return;
    }

    let clip = SpeechClip::plain("transcript", TTS_TEXT, "OPENREEL_TRANSCRIPT_AUDIO");
    let engine = test_engine("OPENREEL_TRANSCRIPT_TEST_DATA_DIR");
    let asset = engine
        .probe(&clip.mp4)
        .expect("generated clip should probe");
    engine.request_transcription(asset.clone());
    let transcript = wait_for_transcript(&engine, asset.id, true);
    let output = transcript
        .words
        .iter()
        .map(|word| word.text.as_str())
        .collect::<Vec<_>>()
        .join(" ");

    println!("TTS: {TTS_TEXT}");
    println!("WHISPER: {output}");
    let normalized = normalized_words(&output);
    for expected in ["hello", "um", "this", "open", "transcript", "test"] {
        assert!(
            normalized.iter().any(|word| word == expected),
            "expected `{expected}` in Whisper output: {output}"
        );
    }
    for word in &transcript.words {
        assert!(
            word.source_start < word.source_end,
            "empty timestamp: {word:?}"
        );
        assert!(
            word.source_end <= asset.duration,
            "timestamp exceeds asset duration: {word:?}"
        );
    }
}
