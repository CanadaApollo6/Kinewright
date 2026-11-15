//! Explicit-input recording fixtures; no application-data discovery or downloads.
use crate::test_support::wav_f32;
use kinewright_core::Document;
use std::{fs, path::Path};

/// Copy an explicitly supplied, hash-verified EN model into a fixture's private
/// data root. This never downloads; the real worker subsequently provisions
/// from this complete installation through its ordinary verified cache path.
///
/// # Errors
/// Returns verification or filesystem errors before a transcript can be queued.
pub fn copy_pinned_english_model(
    source: &Path,
    data: &Path,
) -> Result<(), kinewright_core::MediaError> {
    use crate::recognizer::{ENGLISH_MODEL, model};
    let data = crate::data_dir::ensure_root(data)?;
    let root =
        crate::data_dir::ensure_directory(&data, &Path::new("models").join(ENGLISH_MODEL.id))?;
    for spec in ENGLISH_MODEL.files {
        let verified = model::verify(
            spec,
            &source.join(spec.name),
            &data.join("input-model-receipts"),
            &kinewright_core::ExportCancellation::default(),
        )?;
        let destination = root.join(spec.name);
        if !destination.exists() {
            let mut input = fs::File::open(verified.path())
                .map_err(|e| kinewright_core::MediaError::Backend(e.to_string()))?;
            let mut output = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&destination)
                .map_err(|e| kinewright_core::MediaError::Backend(e.to_string()))?;
            std::io::copy(&mut input, &mut output)
                .map_err(|e| kinewright_core::MediaError::Backend(e.to_string()))?;
        }
        model::verify_at(
            &data,
            spec,
            &destination,
            &root.join("verified"),
            &kinewright_core::ExportCancellation::default(),
        )?;
    }
    Ok(())
}

/// Write the existing production export mix to a float WAV without invoking
/// the video exporter, compositor or output device.
///
/// # Errors
/// Returns ordinary mixer or output-file errors.
pub fn write_export_mix_wav(
    document: &Document,
    path: &Path,
) -> Result<(), kinewright_core::MediaError> {
    let settings = kinewright_core::DeliveryProfile::SourceMaster.export_settings(
        document,
        kinewright_core::DeliveryEncodeDepth::Eight,
        kinewright_core::ExportCancellation::default(),
    );
    let samples = crate::export::mix_audio(document, &settings)?;
    fs::write(path, wav_f32(&samples, 48_000, 2))
        .map_err(|e| kinewright_core::MediaError::Backend(e.to_string()))
}
