use std::{
    fs::{self, File, OpenOptions},
    io::{self, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::Arc,
};

use ffmpeg_next as ffmpeg;
use kinewright_core::{
    Document, ExportReport, ExportSettings, FrameRounding, MediaError, ProgressSink,
    map_frames_with_rounding,
};

use crate::{
    audio::graph_latency_frames,
    clock::frame_to_samples,
    compositor::GpuContext,
    decode::backend,
    owned_io::{CertifiedSource, CursorReservation, LifetimeGuard, PlaybackInputs},
};

use super::{
    AUDIO_RATE, ExportChecks, ExportSource, VideoPacketDuration, encode_document, validate_settings,
};

/// Destination within a caller-controlled staging directory. The final name
/// must be vacant. Reservations are acquired before a worker creates a file.
#[derive(Clone, Copy)]
pub struct OwnedExportTarget<'a> {
    pub destination: &'a Path,
    pub staging_bytes: u64,
    pub reserve_staging: &'a CursorReservation,
    pub reserve_working: &'a CursorReservation,
}

/// Completed staging and its byte reservation. Keep this receipt until the
/// caller has imported or removed the staged file.
pub struct OwnedExport {
    pub report: ExportReport,
    _staging: LifetimeGuard,
}

pub(crate) fn export_owned_document(
    inputs: &Arc<PlaybackInputs>,
    target: OwnedExportTarget<'_>,
    settings: &ExportSettings,
    progress: &ProgressSink,
    gpu: GpuContext,
) -> Result<OwnedExport, MediaError> {
    let document = inputs.document();
    let checks = ExportChecks::owned(settings, inputs, target.staging_bytes);
    checks.check()?;
    validate_settings(document, target.destination, settings)?;
    document
        .validate()
        .map_err(|error| MediaError::InvalidDocument(Box::new(error)))?;
    if let Some(input) = &inputs.unsupported_export_font {
        return Err(MediaError::OwnedExportInputUnavailable {
            input: input.clone(),
        });
    }
    let staging = (target.reserve_staging)(target.staging_bytes).map_err(MediaError::Backend)?;
    let _working =
        (target.reserve_working)(working_bytes(document, settings, target.staging_bytes))
            .map_err(MediaError::Backend)?;

    let (temporary, file) = Stage::create(target.destination)?;
    let writer = BoundedStage {
        file: file.try_clone().map_err(backend)?,
        limit: target.staging_bytes,
    };
    let encoded = encode_document(
        document,
        &checks,
        progress,
        gpu,
        ExportSource::Owned(inputs),
        VideoPacketDuration::OneFrame,
        || {
            let io = ffmpeg::format::context::StreamIo::from_write_seek(writer).map_err(backend)?;
            ffmpeg::format::output_to_stream(io, None, Some("mp4")).map_err(backend)
        },
    );
    // Check after the muxer has dropped, including on native I/O failure.
    checks.check()?;
    let report = encoded?;
    file.sync_all().map_err(backend)?;
    drop(file);
    inputs
        .sources
        .values()
        .try_for_each(CertifiedSource::check_read_authority)?;
    checks.check()?;
    fs::hard_link(&temporary.0, target.destination).map_err(backend)?;
    Ok(OwnedExport {
        report,
        _staging: staging,
    })
}

fn working_bytes(document: &Document, settings: &ExportSettings, staging: u64) -> u64 {
    use crate::render::FRAME_CACHE_BYTE_BUDGET;

    let stereo_bytes = 2 * size_of::<f32>() as u64;
    let latency = graph_latency_frames(&document.audio_mix.lookahead_milliseconds(), AUDIO_RATE);
    let master_frames = frame_to_samples(document.duration, AUDIO_RATE, document.fps)
        .saturating_add(latency as u64);
    // Each mixed track, the master and normalization's replacement buffer.
    let audio = master_frames
        .saturating_mul(stereo_bytes)
        .saturating_mul(document.tracks.len() as u64 + 3);
    // Only one decoded segment can overlap the track buffers. Bound both
    // its decode allocation and any growth when it is padded to placement.
    let decode = document
        .tracks
        .iter()
        .flat_map(|track| &track.clips)
        .filter(|clip| clip.content.is_media())
        .filter_map(|clip| document.asset(clip.asset).map(|asset| (asset, clip)))
        .filter(|(asset, _)| {
            matches!(
                asset.kind,
                kinewright_core::MediaKind::Audio | kinewright_core::MediaKind::AudioVideo
            )
        })
        .map(|(asset, clip)| {
            frame_to_samples(clip.source_range.end, AUDIO_RATE, asset.fps).saturating_sub(
                frame_to_samples(clip.source_range.start, AUDIO_RATE, asset.fps),
            )
        })
        .max()
        .unwrap_or(0)
        .saturating_mul(stereo_bytes)
        .saturating_mul(2);
    let pixels = u64::from(settings.resolution.0) * u64::from(settings.resolution.1);
    // Delivery textures, readback and conversion, and the encoder's frame
    // and lookahead pools. Source codec and AVIO reservations are separate.
    let video = pixels.saturating_mul(512);
    // The mux queue retains at most the checked total coded payload.
    // Allow ticket/sample-table bookkeeping for video frames and AAC packets.
    let mux_entries = map_frames_with_rounding(
        document.duration,
        document.fps,
        settings.fps,
        FrameRounding::Ceil,
    )
    .map_or(u64::MAX, |frames| frames.0.unsigned_abs())
    .saturating_add(master_frames.div_ceil(1_024))
    .saturating_add(8);
    let mux = staging.saturating_add(mux_entries.saturating_mul(512));
    audio
        .saturating_add(decode)
        .saturating_add(video)
        .saturating_add(mux)
        .saturating_add(FRAME_CACHE_BYTE_BUDGET as u64)
        .saturating_add(1 << 20)
}

struct Stage(PathBuf);

impl Stage {
    fn create(destination: &Path) -> Result<(Self, File), MediaError> {
        use std::fmt::Write as _;
        let mut random = [0_u8; 16];
        getrandom::fill(&mut random).map_err(backend)?;
        let mut name = String::from(".kinewright-owned-");
        for byte in random {
            let _ = write!(name, "{byte:02x}");
        }
        let path = destination.with_file_name(name);
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(backend)?;
        Ok((Self(path), file))
    }
}

impl Drop for Stage {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

struct BoundedStage {
    file: File,
    limit: u64,
}

impl Write for BoundedStage {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let start = self.file.stream_position()?;
        if start.saturating_add(bytes.len() as u64) > self.limit {
            return Err(io::Error::new(
                io::ErrorKind::FileTooLarge,
                "owned export staging limit",
            ));
        }
        self.file.write(bytes)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

impl Seek for BoundedStage {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        let offset = self.file.seek(position)?;
        if offset > self.limit {
            return Err(io::Error::new(
                io::ErrorKind::FileTooLarge,
                "owned export staging limit",
            ));
        }
        Ok(offset)
    }
}

#[cfg(test)]
#[path = "owned_tests.rs"]
mod tests;
