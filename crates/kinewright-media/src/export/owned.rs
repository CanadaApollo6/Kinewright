use std::{
    collections::BTreeMap,
    fmt,
    fs::{self, File, OpenOptions},
    io::{self, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::Arc,
};

use ffmpeg_next as ffmpeg;
use kinewright_core::{
    ClipContent, Document, ExportReport, ExportSettings, FrameRounding, MediaError, ProgressSink,
    TimeCode, TrackKind, map_frames_with_rounding, value_at_keys,
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

/// Acquire the owner's commit gate, invoke the install callback once inside
/// it, and return its result. Cancellation and revocation must use that same
/// gate. The read-authority checks in the callback must be callable under it.
pub type OwnedExportCommitGate<'a> =
    dyn Fn(&mut dyn FnMut() -> Result<(), MediaError>) -> Result<(), MediaError> + 'a;

#[derive(Debug, thiserror::Error)]
pub enum OwnedExportError {
    #[error(transparent)]
    Media(#[from] MediaError),
    #[error(
        "owned export needs {bytes} working bytes with {generated_layers} simultaneous generated layers: {reason}"
    )]
    WorkingReservation {
        generated_layers: usize,
        bytes: u64,
        reason: String,
    },
    #[error("{error}; {cleanup}")]
    Cleanup {
        error: MediaError,
        cleanup: Box<OwnedExportCleanup>,
    },
    #[error("owned export commit callback invoked more than once after installation")]
    RepeatedCommit(Box<OwnedExport>),
}

/// An unremoved staging name and its reservation. Keep it until retry succeeds.
/// If the output also installed, both names still refer to one file. Dropping
/// an obligation retries cleanup and keeps the charge for the process lifetime
/// if the path is still unremovable.
#[must_use]
#[derive(thiserror::Error)]
#[error("could not remove owned export staging {}: {error}", .path.display())]
pub struct OwnedExportCleanup {
    path: PathBuf,
    error: io::Error,
    charge: Option<LifetimeGuard>,
}

impl fmt::Debug for OwnedExportCleanup {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OwnedExportCleanup")
            .field("path", &self.path)
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

impl OwnedExportCleanup {
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Remove the outstanding staging name and release its reservation.
    ///
    /// # Errors
    ///
    /// Returns an I/O error and keeps the reservation if removal still fails.
    pub fn retry(&mut self) -> io::Result<()> {
        if self.charge.is_some() {
            remove_stage_file(&self.path)?;
            self.charge = None;
        }
        Ok(())
    }
}

impl Drop for OwnedExportCleanup {
    fn drop(&mut self) {
        if self.retry().is_err() {
            // Dropping an unsettled obligation must not free capacity still
            // occupied on disk. The owner can recover it through restart.
            std::mem::forget(self.charge.take());
        }
    }
}

/// Destination within a caller-controlled staging directory. The final name
/// must be vacant. Reservations are acquired before a worker creates a file.
#[derive(Clone, Copy)]
pub struct OwnedExportTarget<'a> {
    pub destination: &'a Path,
    pub staging_bytes: u64,
    pub reserve_staging: &'a CursorReservation,
    pub reserve_working: &'a CursorReservation,
    pub commit: &'a OwnedExportCommitGate<'a>,
}

/// The staged output, and any extra name that still needs removal.
#[must_use]
#[derive(Debug)]
pub enum OwnedExport {
    Clean(OwnedExportReceipt),
    Cleanup {
        receipt: OwnedExportReceipt,
        cleanup: OwnedExportCleanup,
    },
}

/// Keep this receipt until the installed staging name is removed.
pub struct OwnedExportReceipt {
    pub report: ExportReport,
    _staging: LifetimeGuard,
}

impl fmt::Debug for OwnedExportReceipt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OwnedExportReceipt")
            .field("report", &self.report)
            .finish_non_exhaustive()
    }
}

pub(crate) fn export_owned_document(
    inputs: &Arc<PlaybackInputs>,
    target: OwnedExportTarget<'_>,
    settings: &ExportSettings,
    progress: &ProgressSink,
    gpu: GpuContext,
) -> Result<OwnedExport, OwnedExportError> {
    export_with_cleanup(inputs, target, settings, progress, gpu, remove_stage_file)
}

fn export_with_cleanup(
    inputs: &Arc<PlaybackInputs>,
    target: OwnedExportTarget<'_>,
    settings: &ExportSettings,
    progress: &ProgressSink,
    gpu: GpuContext,
    remove: impl FnOnce(&Path) -> io::Result<()>,
) -> Result<OwnedExport, OwnedExportError> {
    let document = inputs.document();
    let checks = ExportChecks::owned(settings, inputs, target.staging_bytes);
    checks.check()?;
    validate_settings(document, target.destination, settings)?;
    document
        .validate()
        .map_err(|error| MediaError::InvalidDocument(Box::new(error)))?;
    inputs.fonts.check_export(document)?;
    refuse_path_effects(document)?;
    let _no_locators = crate::locator::DenyLocatorOpens::enter();
    let staging = (target.reserve_staging)(target.staging_bytes).map_err(MediaError::Backend)?;
    let generated_layers = peak_generated_layers(document, &checks)?;
    let pixels = u64::from(settings.resolution.0) * u64::from(settings.resolution.1);
    let generated_bytes = pixels
        .saturating_mul(8)
        .saturating_mul(generated_layers as u64);
    let bytes =
        working_bytes(document, settings, target.staging_bytes).saturating_add(generated_bytes);
    let _working =
        (target.reserve_working)(bytes).map_err(|reason| OwnedExportError::WorkingReservation {
            generated_layers,
            bytes,
            reason,
        })?;

    let (temporary, file) = Stage::create(target.destination)?;
    let mut repeated = false;
    let result = (|| {
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
                let io =
                    ffmpeg::format::context::StreamIo::from_write_seek(writer).map_err(backend)?;
                ffmpeg::format::output_to_stream(io, None, Some("mp4")).map_err(backend)
            },
        );
        checks.check()?;
        let report = encoded?;
        file.sync_all().map_err(backend)?;
        drop(file);

        let mut invoked = false;
        let mut installed = false;
        let result = (target.commit)(&mut || {
            if std::mem::replace(&mut invoked, true) {
                repeated = true;
                return Err(MediaError::Backend(
                    "owned export commit callback invoked more than once".into(),
                ));
            }
            inputs
                .sources
                .values()
                .try_for_each(CertifiedSource::check_read_authority)?;
            checks.check()?;
            fs::hard_link(&temporary.0, target.destination).map_err(backend)?;
            installed = true;
            Ok(())
        });
        if installed {
            return Ok(report);
        }
        if repeated {
            return Err(MediaError::Backend(
                "owned export commit callback invoked more than once".into(),
            ));
        }
        result?;
        Err(MediaError::Backend(
            "owned export commit gate skipped installation".into(),
        ))
    })();
    let outcome = temporary.finish(result, staging, remove)?;
    if repeated {
        Err(OwnedExportError::RepeatedCommit(Box::new(outcome)))
    } else {
        Ok(outcome)
    }
}

fn refuse_path_effects(document: &Document) -> Result<(), MediaError> {
    for clip in document.tracks.iter().flat_map(|track| &track.clips) {
        if let Some(effect) = clip.effects.iter().find(|effect| effect.name == "cube_lut") {
            return Err(MediaError::OwnedExportInputUnavailable {
                input: format!("clip {} effect {} (cube_lut)", clip.id, effect.id),
            });
        }
    }
    Ok(())
}

/// Count active generated layers on the actual output-frame grid. A validated
/// track has no overlaps, and a Boolean enable curve changes at most once
/// between adjacent keys under the renderer's monotone easing functions.
fn peak_generated_layers(
    document: &Document,
    checks: &ExportChecks<'_>,
) -> Result<usize, MediaError> {
    let mut events = BTreeMap::<TimeCode, i64>::new();
    for track in document
        .tracks
        .iter()
        .filter(|track| track.kind == TrackKind::Video)
    {
        for clip in track
            .clips
            .iter()
            .filter(|clip| matches!(clip.content, ClipContent::Title(_) | ClipContent::Solid(_)))
        {
            let frames = document.clip_duration(clip).map_err(backend)?.0;
            let keys = clip
                .enabled_curve
                .as_ref()
                .map_or(&[][..], |curve| curve.keyframes.as_slice());
            let mut start = 0;
            for end in keys
                .iter()
                .map(|key| key.at.0)
                .filter(|at| *at > 0 && *at < frames)
                .chain(std::iter::once(frames))
            {
                checks.check()?;
                let from = start;
                start = end;
                let index = keys.partition_point(|key| key.at.0 <= from);
                let span = &keys[index.saturating_sub(1)..(index + 1).min(keys.len())];
                let enabled = |at| value_at_keys(span, at).map_or(clip.enabled, |value| value == 1);
                let first = enabled(from);
                let last = enabled(end - 1);
                let (from, to) = match (first, last) {
                    (false, false) => continue,
                    (true, true) => (from, end),
                    (left, _) => {
                        let mut low = from;
                        let mut high = end - 1;
                        while high - low > 1 {
                            let mid = low + (high - low) / 2;
                            if enabled(mid) == left {
                                low = mid;
                            } else {
                                high = mid;
                            }
                        }
                        if left { (from, high) } else { (high, end) }
                    }
                };
                let output_at = |local| {
                    let project = clip
                        .timeline_start
                        .checked_add(TimeCode(local))
                        .ok_or_else(|| MediaError::Backend("export layer time overflow".into()))?;
                    map_frames_with_rounding(
                        project,
                        document.fps,
                        checks.settings.fps,
                        FrameRounding::Ceil,
                    )
                    .map_err(backend)
                };
                *events.entry(output_at(from)?).or_default() += 1;
                *events.entry(output_at(to)?).or_default() -= 1;
            }
        }
    }
    let peak = events
        .into_values()
        .scan(0_i64, |live, delta| {
            *live += delta;
            Some(*live)
        })
        .max()
        .unwrap_or(0);
    usize::try_from(peak).map_err(backend)
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

impl Stage {
    fn finish(
        self,
        result: Result<ExportReport, MediaError>,
        staging: LifetimeGuard,
        remove: impl FnOnce(&Path) -> io::Result<()>,
    ) -> Result<OwnedExport, OwnedExportError> {
        let removal = remove(&self.0);
        let obligation = |error, staging| OwnedExportCleanup {
            path: self.0,
            error,
            charge: Some(staging),
        };
        match result {
            Ok(report) => Ok(match removal {
                Ok(()) => OwnedExport::Clean(OwnedExportReceipt {
                    report,
                    _staging: staging,
                }),
                Err(error) => OwnedExport::Cleanup {
                    cleanup: obligation(error, Arc::clone(&staging)),
                    receipt: OwnedExportReceipt {
                        report,
                        _staging: staging,
                    },
                },
            }),
            Err(error) => Err(match removal {
                Ok(()) => error.into(),
                Err(remove_error) => OwnedExportError::Cleanup {
                    error,
                    cleanup: Box::new(obligation(remove_error, staging)),
                },
            }),
        }
    }
}

fn remove_stage_file(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        result => result,
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
