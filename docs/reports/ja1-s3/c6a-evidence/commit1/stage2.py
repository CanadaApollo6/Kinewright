from pathlib import Path
p=Path('crates/kinewright-media/src/transcript.rs');s=p.read_text();pos=s.index('fn ensure_model(');s=s[:pos]+'''/// Provision the same pinned, hash-checked model used by asset transcription.
///
/// # Errors
/// Returns the normal model download, verification, installation or cancellation error.
#[cfg(any(test, feature = "test-util"))]
pub fn provision_voice_model(
    data_dir: &Path,
    cancellation: &ExportCancellation,
) -> Result<PathBuf, MediaError> {
    ensure_model(data_dir, Path::new("voice-control"), &StatusReporter::new(), cancellation)
}

'''+s[pos:];p.write_text(s)
p=Path('crates/kinewright-media/src/lib.rs');s=p.read_text().replace('pub use transcript::TranscriptInferenceHold;','pub use transcript::{TranscriptInferenceHold, provision_voice_model};',1);p.write_text(s)
p=Path('crates/kinewright-app/src/recording.rs');s=p.read_text();needle='''            if !model.is_file() || sha256_file(&model).ok().as_deref() != Some(WHISPER_MODEL_SHA256)
''';assert s.count(needle)==1
s=s.replace(needle,'''            let _inference = INFERENCE_TESTS.lock().unwrap();
            if std::env::var_os("KINEWRIGHT_S3_PROVISION_MODEL").as_deref() == Some(std::ffi::OsStr::new("1")) {
                match kinewright_media::provision_voice_model(&model_root, &kinewright_core::ExportCancellation::default()) {
                    Ok(path) => eprintln!("S3_PINNED_MODEL_READY {}", path.display()),
                    Err(error) => {
                        eprintln!("NOT RUN {control}: ensure_model failed: {error}");
                        return;
                    }
                }
            }
'''+needle,1);at=s.index('fn real_speech_control');prefix=s[:at];tail=s[at:];first=tail.index('            let _inference = INFERENCE_TESTS.lock().unwrap();');second=tail.index('            let _inference = INFERENCE_TESTS.lock().unwrap();',first+1);tail=tail[:second]+tail[second:].replace('            let _inference = INFERENCE_TESTS.lock().unwrap();\n','',1);p.write_text(prefix+tail)
