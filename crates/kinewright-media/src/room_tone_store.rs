//! The project-relative room-tone store (AU5 §5.1).
//!
//! Room tone is a **real WAV in the project asset store, registered as an
//! ordinary `MediaAsset`** — never a new document type and never a
//! `ClipContent` variant. The bytes live on [`LutStore`](crate::LutStore)'s
//! digest-plus-store shape, under the same
//! `<project stem>.kinewright-assets` root and the same
//! `LUT_STORE_SUFFIX`, in a `room-tone/` sub-directory whose file names are
//! the 64-character content digest plus `.wav`. No user-supplied string ever
//! reaches a path component, so traversal through a capture is structurally
//! impossible rather than merely checked.
//!
//! **The two caps are deliberately not one number (AU5 §0 R48).**
//! [`ROOM_TONE_MAX_FILE_BYTES`] guards the **reader** against a file nobody
//! wrote — exactly what `LUT_MAX_FILE_BYTES` exists to catch — and is checked
//! from file metadata before a single byte is read.
//! [`ROOM_TONE_MAX_CAPTURE_MILLISECONDS`] guards the **writer**, and caps a
//! legal capture at 23 040 044 B. The ≈ 10.5 MB between them is the margin,
//! not an inconsistency to simplify away.

use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use kinewright_core::{ExportCancellation, MediaAsset, MediaError, MediaKind, Rational, TimeCode};

use crate::{
    audio::decode_audio_range,
    clock::frame_to_samples,
    lut_store::{LUT_STORE_SUFFIX, is_canonical_sha256, sanitize},
    sha256::sha256_bytes,
};

/// Sub-directory of the store root that holds `<sha256>.wav` captures.
pub const ROOM_TONE_STORE_DIRECTORY: &str = "room-tone";

/// Largest room-tone file the store will read or hash.
///
/// 32 MiB accepts ≈ 87.4 s of 48 kHz stereo `f32` (384 000 B/s) and matches
/// `MAX_WAVEFORM_BYTES` (analysis.rs). Like `LUT_MAX_FILE_BYTES`, the length
/// is checked from the file metadata before a single byte is read, so a
/// mistaken pick — a video file, a disk image — never reaches memory. This is
/// the **reader's** guard (AU5 §0 R48).
pub const ROOM_TONE_MAX_FILE_BYTES: u64 = 32 * 1024 * 1024;

/// The shortest capture the store will write, in milliseconds.
///
/// Below half a second a fill is a loop of one gesture rather than room tone,
/// and the floor also clears AU5 §0 R26: `probe_path` refuses a duration
/// rounding to **zero frames**, and an audio-only asset probes at
/// `Rational::default()` = 30/1, so a capture must span at least one 30 fps
/// frame to be registrable at all. 500 ms is 15 of them.
pub const ROOM_TONE_MINIMUM_CAPTURE_MILLISECONDS: u64 = 500;

/// The longest capture the store will write, in milliseconds.
///
/// The **writer's** guard (AU5 §0 R48). 60 s of 48 kHz stereo `f32` is
/// 23 040 044 B written, comfortably inside [`ROOM_TONE_MAX_FILE_BYTES`].
pub const ROOM_TONE_MAX_CAPTURE_MILLISECONDS: u64 = 60_000;

/// The sample rate every capture is decoded to and written at.
///
/// Fixed rather than an argument: AU5 §5.8 decodes with
/// `decode_audio_range(.., 48_000, 2)` and the seam pin's exactness is a
/// property of a **48 kHz PCM** fixture meeting a 48 kHz mix (§5.4 rule 100).
pub const ROOM_TONE_SAMPLE_RATE: u32 = 48_000;

/// The channel count every capture is decoded to and written at.
pub const ROOM_TONE_CHANNELS: u16 = 2;

/// The frame rate an audio-only asset probes at, `Rational::default()`.
pub const ROOM_TONE_ASSET_FRAMES_PER_SECOND: u32 = 30;

/// Sample frames in one 30 fps asset frame at 48 kHz: `48_000 / 30`.
///
/// **This is the number AU5 §5.3's arithmetic rests on.** A fill is written at
/// `speed_percent = 100`, so `clip_effective_fps(asset.fps, fill) ==
/// asset.fps` and a tile of source frames `0..k` occupies exactly
/// `k * ROOM_TONE_SAMPLE_FRAMES_PER_ASSET_FRAME` sample frames of the store
/// file. [`RoomToneStore::write_capture`] therefore writes a **whole number of
/// asset frames**: a capture ending mid-frame would make `probe_path`'s
/// `ceil` round the duration up, and the last tile of a fill would read past
/// the end of the file into silence.
pub const ROOM_TONE_SAMPLE_FRAMES_PER_ASSET_FRAME: u64 = 1_600;

/// [`ROOM_TONE_SAMPLE_RATE`] as `u64`, so none of the frame arithmetic below
/// needs a cast. Every derived constant here is written as a literal and its
/// derivation is pinned by `the_capture_constants_are_their_own_arithmetic`.
const SAMPLE_RATE: u64 = 48_000;

/// Shortest capture, in sample frames: `500 * 48_000 / 1_000`, exactly 15
/// asset frames.
const MINIMUM_CAPTURE_SAMPLE_FRAMES: u64 = 24_000;

/// Longest capture, in sample frames: `60_000 * 48_000 / 1_000`, exactly
/// 1 800 asset frames.
const MAXIMUM_CAPTURE_SAMPLE_FRAMES: u64 = 2_880_000;

/// Bytes of RIFF header before the sample data.
const WAV_HEADER_BYTES: usize = 44;

/// Machine-readable room-tone store codes (AU5 §5.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RoomToneStoreErrorCode {
    /// The project path cannot yield a store root, or the root is not a
    /// directory this process will write through.
    RoomToneStoreRootInvalid,
    /// A digest is not 64 lowercase hexadecimal characters.
    InvalidRoomToneDigest,
    /// The store file is absent or is not a regular file.
    MissingRoomTone,
    /// A store file exists but hashes to something else.
    ChangedRoomTone,
    /// The path exists but its bytes or metadata cannot be read.
    UnreadableRoomTone,
    /// The candidate file exceeds [`ROOM_TONE_MAX_FILE_BYTES`].
    RoomToneFileTooLarge,
    /// The store could not be created or written.
    RoomToneStoreWriteFailed,
    /// The capture is shorter than [`ROOM_TONE_MINIMUM_CAPTURE_MILLISECONDS`].
    RoomToneCaptureTooShort,
    /// The capture is longer than [`ROOM_TONE_MAX_CAPTURE_MILLISECONDS`].
    RoomToneCaptureTooLong,
    /// The sample buffer is not whole [`ROOM_TONE_CHANNELS`] frames.
    RoomToneCaptureMalformed,
}

impl RoomToneStoreErrorCode {
    /// The stable `snake_case` token used in errors and the UI.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RoomToneStoreRootInvalid => "room_tone_store_root_invalid",
            Self::InvalidRoomToneDigest => "invalid_room_tone_digest",
            Self::MissingRoomTone => "missing_room_tone",
            Self::ChangedRoomTone => "changed_room_tone",
            Self::UnreadableRoomTone => "unreadable_room_tone",
            Self::RoomToneFileTooLarge => "room_tone_file_too_large",
            Self::RoomToneStoreWriteFailed => "room_tone_store_write_failed",
            Self::RoomToneCaptureTooShort => "room_tone_capture_too_short",
            Self::RoomToneCaptureTooLong => "room_tone_capture_too_long",
            Self::RoomToneCaptureMalformed => "room_tone_capture_malformed",
        }
    }
}

/// One typed store failure, on [`LutStoreError`](crate::LutStoreError)'s
/// shape: room-tone errors cross to callers as `MediaError::Backend` with the
/// same stable `<code>: …; observed=…; allowed=…` prefix, and the typed error
/// stays public for surfaces that need the structure back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoomToneStoreError {
    /// The stable failure code.
    pub code: RoomToneStoreErrorCode,
    /// What went wrong, in one sentence.
    pub detail: String,
    /// What was observed, when the failure has an observation.
    pub observed: Option<String>,
    /// What would have been accepted.
    pub allowed: Option<String>,
}

impl RoomToneStoreError {
    fn new(code: RoomToneStoreErrorCode, detail: String) -> Self {
        Self {
            code,
            detail,
            observed: None,
            allowed: None,
        }
    }

    fn with_observed(mut self, observed: &str) -> Self {
        self.observed = Some(sanitize(observed));
        self
    }

    fn with_allowed(mut self, allowed: &str) -> Self {
        self.allowed = Some(sanitize(allowed));
        self
    }

    /// The stable failure code.
    #[must_use]
    pub const fn code(&self) -> RoomToneStoreErrorCode {
        self.code
    }
}

impl std::fmt::Display for RoomToneStoreError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.code.as_str(), self.detail)?;
        if let Some(observed) = &self.observed {
            write!(formatter, "; observed={observed}")?;
        }
        if let Some(allowed) = &self.allowed {
            write!(formatter, "; allowed={allowed}")?;
        }
        Ok(())
    }
}

impl std::error::Error for RoomToneStoreError {}

impl From<RoomToneStoreError> for MediaError {
    fn from(error: RoomToneStoreError) -> Self {
        Self::Backend(error.to_string())
    }
}

/// What one digest looks like on disk right now (AU5 §5.1, the LUT store's
/// availability vocabulary).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RoomToneAvailabilityKind {
    /// The store file exists and hashes to the recorded digest.
    Verified,
    /// The store file is absent, or is not a regular file.
    Missing,
    /// The store file exists and hashes to something else.
    Changed,
    /// The store file exists but cannot be inspected, read, or hashed.
    Unreadable,
}

/// One availability observation for one digest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoomToneAvailabilityStatus {
    /// What was observed.
    pub kind: RoomToneAvailabilityKind,
    /// The digest the bytes actually hash to, when they could be hashed.
    pub observed_sha256: Option<String>,
    /// The typed reason, for everything but [`RoomToneAvailabilityKind::Verified`].
    pub reason: Option<String>,
    /// The store path the observation is about.
    pub path: Option<PathBuf>,
}

/// Metadata-only result of one capture (AU5 §5.1, §5.8).
///
/// No samples cross back to the caller: the agent half of `capture_room_tone`
/// probes [`RoomToneCapture::path`] with `probe_path` like any import and
/// submits one `Operation::AddAsset`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoomToneCapture {
    /// SHA-256 over the written WAV bytes; also the file stem.
    pub sha256: String,
    /// `<root>/room-tone/<sha256>.wav`.
    pub path: PathBuf,
    /// Length of the written file in bytes.
    pub byte_len: u64,
    /// The capture's length in 30 fps asset frames — what `probe_path` will
    /// report as the asset's `duration`.
    pub frames: TimeCode,
    /// The capture's length in 48 kHz sample frames, always a whole multiple
    /// of [`ROOM_TONE_SAMPLE_FRAMES_PER_ASSET_FRAME`].
    pub sample_frames: u64,
    /// The length that was **asked** for, in 48 kHz sample frames.
    ///
    /// Differs from [`RoomToneCapture::sample_frames`] by the whole-asset-frame
    /// truncation AU5 §0 R102 records — at most 1 599 sample frames, 33.3 ms —
    /// so a surface that wants to say "33 ms were trimmed" can, without
    /// recomputing from the buffer it passed.
    pub requested_sample_frames: u64,
    /// The capture's length in milliseconds, exact by construction.
    pub milliseconds: u64,
    /// Whether the store already held a correctly hashed file at this digest.
    ///
    /// AU5 §5.8 rule 116's idempotence reads this: a re-capture of the same
    /// range writes nothing, and the caller returns the existing `asset_id`
    /// and emits **no** operation.
    pub already_present: bool,
}

/// The project-relative room-tone store rooted at
/// `<dir>/<stem>.kinewright-assets`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoomToneStore {
    root: PathBuf,
}

impl RoomToneStore {
    /// Derive the store root from a saved project path.
    ///
    /// Verbatim [`LutStore::for_project`](crate::LutStore::for_project)'s
    /// rule, and deliberately the **same root**: the project path is
    /// absolutized first, the root is `<parent>/<file stem>.kinewright-assets`
    /// taken from the file stem regardless of the project file's extension,
    /// and it is never persisted anywhere. Copying the project file plus that
    /// one directory relocates the LUTs and the room tone together.
    ///
    /// # Errors
    ///
    /// Returns `room_tone_store_root_invalid` when the absolute path has no
    /// parent component or no file stem, when the working directory cannot be
    /// read, and when the derived root already exists as a symlink or a
    /// non-directory.
    pub fn for_project(project_path: &Path) -> Result<Self, MediaError> {
        let absolute = std::path::absolute(project_path).map_err(|error| {
            RoomToneStoreError::new(
                RoomToneStoreErrorCode::RoomToneStoreRootInvalid,
                format!("the project path could not be made absolute: {error}"),
            )
            .with_observed(&project_path.display().to_string())
            .with_allowed("a project file path that resolves against a readable working directory")
        })?;
        let parent = absolute
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .ok_or_else(|| {
                RoomToneStoreError::new(
                    RoomToneStoreErrorCode::RoomToneStoreRootInvalid,
                    "the project path has no parent directory".to_owned(),
                )
                .with_observed(&absolute.display().to_string())
                .with_allowed("a saved project file path such as <dir>/<stem>.kinewright")
            })?;
        let stem = absolute.file_stem().ok_or_else(|| {
            RoomToneStoreError::new(
                RoomToneStoreErrorCode::RoomToneStoreRootInvalid,
                "the project path has no file stem".to_owned(),
            )
            .with_observed(&absolute.display().to_string())
            .with_allowed("a saved project file path such as <dir>/<stem>.kinewright")
        })?;
        let mut directory = OsString::from(stem);
        directory.push(".");
        directory.push(LUT_STORE_SUFFIX);
        let root = parent.join(directory);
        check_store_directory(&root)?;
        Ok(Self { root })
    }

    /// The store root, `<dir>/<stem>.kinewright-assets`.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The directory holding `<sha256>.wav` files.
    #[must_use]
    pub fn room_tone_dir(&self) -> PathBuf {
        self.root.join(ROOM_TONE_STORE_DIRECTORY)
    }

    /// The asset frame rate a written capture probes at, `Rational::default()`.
    #[must_use]
    pub fn asset_fps() -> Rational {
        Rational::default()
    }

    /// The store path for one digest.
    ///
    /// The digest is validated before it is interpolated, so the only file
    /// names this can produce are 64 hex characters plus `.wav`.
    ///
    /// # Errors
    ///
    /// Returns `invalid_room_tone_digest` when the digest is not exactly 64
    /// lowercase hexadecimal characters.
    pub fn path_for(&self, sha256: &str) -> Result<PathBuf, MediaError> {
        self.store_path(sha256).map_err(MediaError::from)
    }

    /// [`RoomToneStore::path_for`] with the typed failure kept, for the
    /// availability surface that reports a reason rather than returning an
    /// error.
    fn store_path(&self, sha256: &str) -> Result<PathBuf, RoomToneStoreError> {
        if !is_canonical_sha256(sha256) {
            return Err(RoomToneStoreError::new(
                RoomToneStoreErrorCode::InvalidRoomToneDigest,
                "a room-tone digest must be a canonical SHA-256 digest".to_owned(),
            )
            .with_observed(sha256)
            .with_allowed("64 lowercase hexadecimal characters"));
        }
        Ok(self.room_tone_dir().join(format!("{sha256}.wav")))
    }

    /// Write one captured range into the store (AU5 §5.1, §5.8).
    ///
    /// `samples` is interleaved [`ROOM_TONE_CHANNELS`]-channel `f32` at
    /// [`ROOM_TONE_SAMPLE_RATE`] — exactly what
    /// `decode_audio_range(.., 48_000, 2)` returns. The buffer is **truncated
    /// to a whole 30 fps asset frame** before it is written, because §5.3's
    /// fill arithmetic needs `k` source frames to be exactly
    /// `k * 1 600` sample frames of this file at `speed_percent = 100`; the
    /// truncation is at most 1 599 sample frames, or 33.3 ms, and the written
    /// length is reported back in [`RoomToneCapture`].
    ///
    /// The length guards are applied to the **requested** buffer, before the
    /// truncation, so a 60.001 s request is refused rather than silently
    /// snapped down to a legal 60.000 s. Every capture that survives them is
    /// at least 15 asset frames, because
    /// [`ROOM_TONE_MINIMUM_CAPTURE_MILLISECONDS`] is itself a whole number of
    /// asset frames and truncation cannot cross a multiple it starts on or
    /// above.
    ///
    /// An existing file that already hashes correctly is left completely
    /// untouched and reported with `already_present`, exactly as
    /// `LutStore::import_lut_asset` is idempotent; a file at the same path
    /// that hashes to something else is overwritten, because an impostor at a
    /// content-addressed path cannot be the content it claims to be.
    ///
    /// # Errors
    ///
    /// Returns `room_tone_capture_malformed` for a buffer that is not whole
    /// stereo frames, `room_tone_capture_too_short` /
    /// `room_tone_capture_too_long` for a range outside the two capture caps,
    /// and `room_tone_store_root_invalid` / `room_tone_store_write_failed` for
    /// filesystem failures.
    pub fn write_capture(&self, samples: &[f32]) -> Result<RoomToneCapture, MediaError> {
        let channels = u64::from(ROOM_TONE_CHANNELS);
        let sample_count = u64::try_from(samples.len()).unwrap_or(u64::MAX);
        if !sample_count.is_multiple_of(channels) {
            return Err(RoomToneStoreError::new(
                RoomToneStoreErrorCode::RoomToneCaptureMalformed,
                "a room-tone capture must be whole interleaved stereo frames".to_owned(),
            )
            .with_observed(&format!("{sample_count} samples"))
            .with_allowed(&format!("a multiple of {ROOM_TONE_CHANNELS}"))
            .into());
        }
        let requested_sample_frames = sample_count / channels;
        check_capture_span(requested_sample_frames)?;
        let frames = requested_sample_frames / ROOM_TONE_SAMPLE_FRAMES_PER_ASSET_FRAME;
        let sample_frames = frames * ROOM_TONE_SAMPLE_FRAMES_PER_ASSET_FRAME;
        let kept = usize::try_from(sample_frames * channels).unwrap_or(samples.len());
        let bytes = room_tone_wav_bytes(&samples[..kept]);
        let byte_len = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        let sha256 = sha256_bytes(&bytes);
        let destination = self.path_for(&sha256)?;
        // AU5 §0 R107: `already_present` comes back from the write, not from a
        // second `availability` pass. The dedup compare has already read and
        // hashed the existing file; asking twice cost two full reads and two
        // digest passes of a file that can be 23 MB.
        let already_present = self.write_store_file(&destination, &bytes, &sha256)?;
        Ok(RoomToneCapture {
            sha256,
            path: destination,
            byte_len,
            frames: TimeCode(i64::try_from(frames).unwrap_or(i64::MAX)),
            sample_frames,
            requested_sample_frames,
            milliseconds: milliseconds_of(sample_frames),
            already_present,
        })
    }

    /// AU5 §5.8's media half: decode one **source** range of one asset at
    /// 48 kHz stereo and write it into the store.
    ///
    /// The raw clip source, never the mix path (§5.1 rule 91): the fill clip is
    /// replayed **through** the same track stage, bus and master chain the
    /// dialogue goes through, so capturing post-chain would apply the
    /// denoiser, the compressor and the limiter twice.
    ///
    /// This is the entry point `capture_room_tone` is built on, and it lives
    /// here because **the store owns the caps, the digest and the file**: the
    /// two capture caps are applied to the requested range before anything is
    /// decoded, and the bytes go straight into the content-addressed path
    /// without a caller ever holding them. `decode_audio_range` is `pub` as of
    /// §0 R106, so an outside caller *could* decode a range itself; nothing on
    /// the `Analysis` trait hands one raw samples, and a caller that decoded
    /// first would only move the refusal a decode later.
    /// The agent half is the rest of §5.8 rule 115 — probe the
    /// written file, override `probe_path`'s `<sha256>.wav` name (§0 R46), and
    /// submit one `Operation::AddAsset` — and rule 116's idempotence reads
    /// [`RoomToneCapture::already_present`].
    ///
    /// **The length guards are applied to the requested range before a byte is
    /// decoded**, which is the writer's half of the metadata-first rule: a
    /// range of an hour is refused rather than decoded into 1.4 GB of `f32`
    /// and then refused.
    ///
    /// # Errors
    ///
    /// Returns `room_tone_capture_malformed` for an empty or out-of-bounds
    /// range and for an asset with no audio, the two capture-length codes for
    /// a range outside the caps, and the decoder's own failure otherwise.
    pub fn capture_room_tone(
        &self,
        asset: &MediaAsset,
        source: std::ops::Range<TimeCode>,
        cancellation: &ExportCancellation,
    ) -> Result<RoomToneCapture, MediaError> {
        if !matches!(asset.kind, MediaKind::Audio | MediaKind::AudioVideo) {
            return Err(RoomToneStoreError::new(
                RoomToneStoreErrorCode::RoomToneCaptureMalformed,
                format!("asset {} carries no audio to capture", asset.id),
            )
            .with_observed(&format!("{:?}", asset.kind))
            .with_allowed("an asset whose media kind carries audio")
            .into());
        }
        if source.start < TimeCode::ZERO || source.end <= source.start {
            return Err(RoomToneStoreError::new(
                RoomToneStoreErrorCode::RoomToneCaptureMalformed,
                "a room-tone capture range must be non-empty".to_owned(),
            )
            .with_observed(&format!("{}..{}", source.start.0, source.end.0))
            .with_allowed("0 <= start < end")
            .into());
        }
        if source.end > asset.duration {
            return Err(RoomToneStoreError::new(
                RoomToneStoreErrorCode::RoomToneCaptureMalformed,
                "a room-tone capture range must lie inside the asset".to_owned(),
            )
            .with_observed(&format!("{}..{}", source.start.0, source.end.0))
            .with_allowed(&format!("0..{}", asset.duration.0))
            .into());
        }
        let requested_sample_frames =
            frame_to_samples(source.end, ROOM_TONE_SAMPLE_RATE, asset.fps).saturating_sub(
                frame_to_samples(source.start, ROOM_TONE_SAMPLE_RATE, asset.fps),
            );
        check_capture_span(requested_sample_frames)?;

        let samples = decode_audio_range(
            &asset.path,
            asset.fps,
            source.start,
            source.end,
            ROOM_TONE_SAMPLE_RATE,
            ROOM_TONE_CHANNELS,
            cancellation,
        )?;
        self.write_capture(&samples)
    }

    /// Observe one digest's availability.
    ///
    /// `symlink_metadata` does not follow a link, so a symlinked store entry
    /// is reported as `missing` instead of being read from outside the project
    /// directory, and the file length is taken from that same metadata call —
    /// an oversized file is refused **before** a byte of it is hashed.
    #[must_use]
    pub fn availability(&self, sha256: &str) -> RoomToneAvailabilityStatus {
        let path = match self.store_path(sha256) {
            Ok(path) => path,
            Err(error) => {
                return unreadable(Some(self.room_tone_dir()), &error.to_string());
            }
        };
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return missing(&path, "the store file is absent");
            }
            Err(error) => {
                return unreadable(
                    Some(path),
                    &RoomToneStoreError::new(
                        RoomToneStoreErrorCode::UnreadableRoomTone,
                        format!("could not inspect the store file: {error}"),
                    )
                    .to_string(),
                );
            }
        };
        if !metadata.is_file() {
            return missing(&path, "the store path is not a regular file");
        }
        if metadata.len() > ROOM_TONE_MAX_FILE_BYTES {
            return unreadable(Some(path), &too_large(metadata.len()));
        }
        match fs::read(&path) {
            Ok(bytes) => {
                let observed = sha256_bytes(&bytes);
                if observed == sha256 {
                    RoomToneAvailabilityStatus {
                        kind: RoomToneAvailabilityKind::Verified,
                        observed_sha256: Some(observed),
                        reason: None,
                        path: Some(path),
                    }
                } else {
                    let reason = RoomToneStoreError::new(
                        RoomToneStoreErrorCode::ChangedRoomTone,
                        "the store file no longer hashes to the recorded content".to_owned(),
                    )
                    .with_observed(&observed)
                    .with_allowed(sha256)
                    .to_string();
                    RoomToneAvailabilityStatus {
                        kind: RoomToneAvailabilityKind::Changed,
                        observed_sha256: Some(observed),
                        reason: Some(reason),
                        path: Some(path),
                    }
                }
            }
            Err(error) => unreadable(
                Some(path),
                &RoomToneStoreError::new(
                    RoomToneStoreErrorCode::UnreadableRoomTone,
                    format!("could not read the store file: {error}"),
                )
                .to_string(),
            ),
        }
    }

    /// Whether the store already holds a correctly hashed file at `sha256`.
    ///
    /// AU5 §5.8 rule 116's idempotence check, spelled once so a caller does
    /// not have to match on [`RoomToneAvailabilityKind`] to ask a yes/no
    /// question.
    #[must_use]
    pub fn holds(&self, sha256: &str) -> bool {
        self.availability(sha256).kind == RoomToneAvailabilityKind::Verified
    }

    /// Read one stored capture's bytes back.
    ///
    /// **The length is checked from the file metadata before a single byte is
    /// read** (AU5 §5.1 rule 90), which is the whole point of
    /// [`ROOM_TONE_MAX_FILE_BYTES`]: it keeps a file nobody wrote from being
    /// read into memory at all.
    ///
    /// # Errors
    ///
    /// Returns `invalid_room_tone_digest` for a malformed digest,
    /// `missing_room_tone` when nothing regular is at the path,
    /// `room_tone_file_too_large` past the cap, and `unreadable_room_tone`
    /// when the bytes cannot be read.
    pub fn read_capture(&self, sha256: &str) -> Result<Vec<u8>, MediaError> {
        let path = self.path_for(sha256)?;
        read_store_file(&path)
    }

    /// Write `bytes` to `destination` unless a correctly hashed file is
    /// already there, in which case the existing file is left completely
    /// untouched.
    ///
    /// The dedup comparison reads the existing file only when its metadata
    /// length is within [`ROOM_TONE_MAX_FILE_BYTES`]. A store entry larger
    /// than the cap is never read into memory; it is simply not a dedup
    /// candidate, so the freshly hashed `bytes` overwrite it — the safe
    /// direction, since the incoming bytes already hash to `sha256`.
    ///
    /// Answers **`true` when the existing file was kept**, which is exactly
    /// [`RoomToneCapture::already_present`] and AU5 §5.8 rule 116's
    /// idempotence signal.
    fn write_store_file(
        &self,
        destination: &Path,
        bytes: &[u8],
        sha256: &str,
    ) -> Result<bool, MediaError> {
        let directory = self.room_tone_dir();
        check_store_directory(self.root())?;
        check_store_directory(&directory)?;
        check_store_entry(destination)?;
        fs::create_dir_all(&directory).map_err(|error| {
            MediaError::from(RoomToneStoreError::new(
                RoomToneStoreErrorCode::RoomToneStoreWriteFailed,
                format!(
                    "could not create the room-tone store directory {}: {error}",
                    directory.display()
                ),
            ))
        })?;
        if let Ok(metadata) = fs::symlink_metadata(destination)
            && metadata.is_file()
            && metadata.len() <= ROOM_TONE_MAX_FILE_BYTES
            && fs::read(destination).is_ok_and(|existing| sha256_bytes(&existing) == sha256)
        {
            return Ok(true);
        }
        atomic_write_store_file(destination, bytes).map(|()| false)
    }
}

/// Refuse a capture span outside the two writer caps (AU5 §5.1 rule 90).
///
/// The comparison is on **sample frames**, not on derived milliseconds: one
/// sample frame past the ceiling still floors to 60 000 ms, so a millisecond
/// comparison would let it through.
fn check_capture_span(sample_frames: u64) -> Result<(), RoomToneStoreError> {
    if sample_frames < MINIMUM_CAPTURE_SAMPLE_FRAMES {
        return Err(RoomToneStoreError::new(
            RoomToneStoreErrorCode::RoomToneCaptureTooShort,
            "a room-tone capture must be at least half a second".to_owned(),
        )
        .with_observed(&format!(
            "{sample_frames} sample frames ({} ms)",
            milliseconds_of(sample_frames)
        ))
        .with_allowed(&format!(
            "{MINIMUM_CAPTURE_SAMPLE_FRAMES} sample frames \
             ({ROOM_TONE_MINIMUM_CAPTURE_MILLISECONDS} ms) or more"
        )));
    }
    if sample_frames > MAXIMUM_CAPTURE_SAMPLE_FRAMES {
        return Err(RoomToneStoreError::new(
            RoomToneStoreErrorCode::RoomToneCaptureTooLong,
            "a room-tone capture must be at most a minute".to_owned(),
        )
        .with_observed(&format!(
            "{sample_frames} sample frames ({} ms)",
            milliseconds_of(sample_frames)
        ))
        .with_allowed(&format!(
            "{MAXIMUM_CAPTURE_SAMPLE_FRAMES} sample frames \
             ({ROOM_TONE_MAX_CAPTURE_MILLISECONDS} ms) or fewer"
        )));
    }
    Ok(())
}

/// The exact milliseconds `sample_frames` of 48 kHz audio last.
const fn milliseconds_of(sample_frames: u64) -> u64 {
    sample_frames * 1_000 / SAMPLE_RATE
}

/// A 32-bit float WAV, byte-for-byte what `test_support::wav_f32` writes at
/// [`ROOM_TONE_SAMPLE_RATE`] and [`ROOM_TONE_CHANNELS`].
///
/// Written by hand rather than muxed so a capture survives the round trip
/// **bit-exactly**: `pcm_f32le` in a RIFF container is the only interchange
/// form where the decoder gives back the `f32` the mixer produced, which is
/// what makes §5.4's seam deviation exactly 0.0 rather than 1e-7. The
/// equivalence with the fixture helper is pinned by
/// `the_store_wav_is_byte_identical_to_the_fixture_writer`.
#[must_use]
fn room_tone_wav_bytes(samples: &[f32]) -> Vec<u8> {
    let data_length = u32::try_from(samples.len() * 4).unwrap_or(u32::MAX);
    let channels = ROOM_TONE_CHANNELS;
    let rate = ROOM_TONE_SAMPLE_RATE;
    let mut bytes = Vec::with_capacity(samples.len() * 4 + WAV_HEADER_BYTES);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data_length).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16_u32.to_le_bytes());
    bytes.extend_from_slice(&3_u16.to_le_bytes());
    bytes.extend_from_slice(&channels.to_le_bytes());
    bytes.extend_from_slice(&rate.to_le_bytes());
    bytes.extend_from_slice(&(rate * u32::from(channels) * 4).to_le_bytes());
    bytes.extend_from_slice(&(channels * 4).to_le_bytes());
    bytes.extend_from_slice(&32_u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_length.to_le_bytes());
    for sample in samples {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    bytes
}

/// Read a regular store file with the metadata-first length guard.
fn read_store_file(path: &Path) -> Result<Vec<u8>, MediaError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        MediaError::from(
            if error.kind() == std::io::ErrorKind::NotFound {
                RoomToneStoreError::new(
                    RoomToneStoreErrorCode::MissingRoomTone,
                    format!("the store file is absent: {}", path.display()),
                )
            } else {
                RoomToneStoreError::new(
                    RoomToneStoreErrorCode::UnreadableRoomTone,
                    format!("could not inspect {}: {error}", path.display()),
                )
            }
            .with_allowed("a readable regular room-tone .wav file"),
        )
    })?;
    if !metadata.is_file() {
        return Err(RoomToneStoreError::new(
            RoomToneStoreErrorCode::MissingRoomTone,
            format!("{} is not a regular file", path.display()),
        )
        .with_allowed("a readable regular room-tone .wav file")
        .into());
    }
    // The length is checked from the metadata, so an oversized file is never
    // read into memory at all.
    if metadata.len() > ROOM_TONE_MAX_FILE_BYTES {
        return Err(MediaError::Backend(too_large(metadata.len())));
    }
    fs::read(path).map_err(|error| {
        MediaError::from(
            RoomToneStoreError::new(
                RoomToneStoreErrorCode::UnreadableRoomTone,
                format!("could not read {}: {error}", path.display()),
            )
            .with_allowed("a readable regular room-tone .wav file"),
        )
    })
}

/// The one `room_tone_file_too_large` sentence, spelled once.
fn too_large(observed: u64) -> String {
    RoomToneStoreError::new(
        RoomToneStoreErrorCode::RoomToneFileTooLarge,
        "the room-tone file is larger than the room-tone file limit".to_owned(),
    )
    .with_observed(&observed.to_string())
    .with_allowed(&ROOM_TONE_MAX_FILE_BYTES.to_string())
    .to_string()
}

/// Refuse a store root or sub-directory that is a symlink or not a directory.
///
/// The store is application-owned and always sits beside the project file, so
/// a symlinked root, `room-tone/`, or entry is refused rather than followed: a
/// write must never leave the project directory. Verbatim `lut_store.rs`'s
/// rule, with the room-tone code.
fn check_store_directory(path: &Path) -> Result<(), MediaError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            Err(root_invalid(path, "the store directory is a symlink"))
        }
        Ok(metadata) if metadata.is_dir() => Ok(()),
        Ok(_) => Err(root_invalid(
            path,
            "the store directory path exists and is not a directory",
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(root_invalid(
            path,
            &format!("could not inspect the store directory: {error}"),
        )),
    }
}

/// Refuse a store file that is a symlink or is not a regular file.
fn check_store_entry(path: &Path) -> Result<(), MediaError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            Err(root_invalid(path, "the store file is a symlink"))
        }
        Ok(metadata) if metadata.is_file() => Ok(()),
        Ok(_) => Err(root_invalid(
            path,
            "the store file exists and is not a regular file",
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(root_invalid(
            path,
            &format!("could not inspect the store file: {error}"),
        )),
    }
}

/// One `room_tone_store_root_invalid` failure naming the path and the reason.
fn root_invalid(path: &Path, reason: &str) -> MediaError {
    MediaError::from(
        RoomToneStoreError::new(
            RoomToneStoreErrorCode::RoomToneStoreRootInvalid,
            reason.to_owned(),
        )
        .with_observed(&path.display().to_string()),
    )
}

/// A `missing` observation for one store path.
fn missing(path: &Path, detail: &str) -> RoomToneAvailabilityStatus {
    RoomToneAvailabilityStatus {
        kind: RoomToneAvailabilityKind::Missing,
        observed_sha256: None,
        reason: Some(
            RoomToneStoreError::new(
                RoomToneStoreErrorCode::MissingRoomTone,
                format!("{detail}: {}", path.display()),
            )
            .to_string(),
        ),
        path: Some(path.to_owned()),
    }
}

/// An `unreadable` observation for one store path.
fn unreadable(path: Option<PathBuf>, reason: &str) -> RoomToneAvailabilityStatus {
    RoomToneAvailabilityStatus {
        kind: RoomToneAvailabilityKind::Unreadable,
        observed_sha256: None,
        reason: Some(reason.to_owned()),
        path,
    }
}

/// The next temporary-file sequence number for this process.
static TEMPORARY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Write a store file by writing a temporary file in the same directory and
/// renaming it into place.
///
/// Deliberately not `derived_cache::atomic_write` and not
/// `lut_store::atomic_write_store_file`: the store is not a cache, and its
/// failures are reported with the room-tone codes.
fn atomic_write_store_file(destination: &Path, bytes: &[u8]) -> Result<(), MediaError> {
    let sequence = TEMPORARY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temporary =
        destination.with_extension(format!("wav.tmp-{}-{sequence}", std::process::id()));
    fs::write(&temporary, bytes).map_err(|error| {
        MediaError::from(RoomToneStoreError::new(
            RoomToneStoreErrorCode::RoomToneStoreWriteFailed,
            format!(
                "could not write the room-tone store file {}: {error}",
                temporary.display()
            ),
        ))
    })?;
    fs::rename(&temporary, destination).map_err(|error| {
        let _ = fs::remove_file(&temporary);
        MediaError::from(RoomToneStoreError::new(
            RoomToneStoreErrorCode::RoomToneStoreWriteFailed,
            format!(
                "could not commit the room-tone store file {}: {error}",
                destination.display()
            ),
        ))
    })
}

#[cfg(test)]
mod tests {
    use kinewright_core::{AssetId, ExportCancellation, MediaKind};

    use super::*;
    use crate::{
        audio::decode_audio_range,
        decode::probe_path,
        test_support::{GeneratedMedia, TempDirectory, pseudo_random_amplitude, wav_f32},
    };

    fn store_for(directory: &TempDirectory, project: &str) -> RoomToneStore {
        RoomToneStore::for_project(&directory.path(project)).expect("the store root should derive")
    }

    /// `frames` sample frames of interleaved stereo room tone.
    fn capture_samples(sample_frames: u64) -> Vec<f32> {
        let count = usize::try_from(sample_frames).expect("the fixture fits") * 2;
        pseudo_random_amplitude(count, 0.010)
    }

    fn backend_message(error: MediaError) -> String {
        let MediaError::Backend(message) = error else {
            panic!("room-tone store failures cross as MediaError::Backend");
        };
        message
    }

    /// A syntactically valid digest that nothing ever wrote.
    const ABSENT_DIGEST: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    #[test]
    fn the_capture_constants_are_their_own_arithmetic() {
        assert_eq!(SAMPLE_RATE, u64::from(ROOM_TONE_SAMPLE_RATE));
        // AU5 §0 R108: not a tautology against `asset_fps()`, which *returns*
        // `Rational::default()`. This ties the constant every other figure in
        // the file is derived from to the rate `probe_path` actually gives an
        // audio-only asset (decode.rs:105-112). If `Rational::default()` ever
        // moved off 30/1, 1 600 would be silently wrong and every other lane
        // here would stay green.
        assert_eq!(
            Rational::default(),
            Rational::new(ROOM_TONE_ASSET_FRAMES_PER_SECOND, 1).unwrap(),
        );
        assert_eq!(
            ROOM_TONE_SAMPLE_FRAMES_PER_ASSET_FRAME,
            SAMPLE_RATE / u64::from(ROOM_TONE_ASSET_FRAMES_PER_SECOND),
        );
        assert_eq!(
            MINIMUM_CAPTURE_SAMPLE_FRAMES,
            ROOM_TONE_MINIMUM_CAPTURE_MILLISECONDS * SAMPLE_RATE / 1_000,
        );
        assert_eq!(
            MAXIMUM_CAPTURE_SAMPLE_FRAMES,
            ROOM_TONE_MAX_CAPTURE_MILLISECONDS * SAMPLE_RATE / 1_000,
        );
        // Both caps are whole asset frames, which is what makes the truncation
        // in `write_capture` unable to cross the floor.
        assert_eq!(
            MINIMUM_CAPTURE_SAMPLE_FRAMES % ROOM_TONE_SAMPLE_FRAMES_PER_ASSET_FRAME,
            0,
        );
        assert_eq!(
            MAXIMUM_CAPTURE_SAMPLE_FRAMES % ROOM_TONE_SAMPLE_FRAMES_PER_ASSET_FRAME,
            0,
        );
        assert_eq!(ROOM_TONE_MAX_FILE_BYTES, 32 * 1024 * 1024);

        // AU5 §5.1 rule 90's two figures, recomputed rather than quoted.
        let header = u64::try_from(WAV_HEADER_BYTES).unwrap();
        let longest_legal_capture_bytes =
            MAXIMUM_CAPTURE_SAMPLE_FRAMES * u64::from(ROOM_TONE_CHANNELS) * 4 + header;
        assert_eq!(longest_legal_capture_bytes, 23_040_044);
        assert!(
            longest_legal_capture_bytes < ROOM_TONE_MAX_FILE_BYTES,
            "the writer's cap must sit inside the reader's",
        );
        let bytes_per_second = SAMPLE_RATE * u64::from(ROOM_TONE_CHANNELS) * 4;
        assert_eq!(bytes_per_second, 384_000);
        // ≈ 87.4 s of 48 kHz stereo f32 fits the reader's cap.
        assert_eq!((ROOM_TONE_MAX_FILE_BYTES - header) / bytes_per_second, 87);
    }

    #[test]
    fn the_store_root_is_shared_with_the_lut_store_and_the_directory_is_not() {
        let temporary = TempDirectory::new("room-tone-root");
        let store = store_for(&temporary, "Demo Project.kinewright");
        let luts = crate::LutStore::for_project(&temporary.path("Demo Project.kinewright"))
            .expect("the LUT store derives from the same project path");

        assert_eq!(store.root(), luts.root(), "one root holds both stores");
        assert_eq!(
            store.root().file_name().unwrap().to_str().unwrap(),
            "Demo Project.kinewright-assets",
        );
        assert_eq!(store.room_tone_dir(), store.root().join("room-tone"));
        assert_ne!(store.room_tone_dir(), luts.luts_dir());
        assert_eq!(ROOM_TONE_STORE_DIRECTORY, "room-tone");
        assert_eq!(RoomToneStore::asset_fps(), Rational::default());
    }

    #[test]
    fn path_for_accepts_only_a_canonical_digest() {
        let temporary = TempDirectory::new("room-tone-digest");
        let store = store_for(&temporary, "project.kinewright");

        assert_eq!(
            store.path_for(ABSENT_DIGEST).unwrap(),
            store.room_tone_dir().join(format!("{ABSENT_DIGEST}.wav")),
        );
        for spelling in [
            "",
            "0123456789ABCDEF0123456789abcdef0123456789abcdef0123456789abcdef",
            "0123456789abcdef",
            "../../../etc/passwd",
        ] {
            let message = backend_message(store.path_for(spelling).unwrap_err());
            assert!(
                message.starts_with("invalid_room_tone_digest: "),
                "{message}",
            );
            assert!(
                message.contains("allowed=64 lowercase hexadecimal characters"),
                "{message}"
            );
        }
    }

    #[test]
    fn the_capture_floor_is_refused_one_sample_frame_below_and_accepted_on_it() {
        let temporary = TempDirectory::new("room-tone-floor");
        let store = store_for(&temporary, "project.kinewright");

        let message = backend_message(
            store
                .write_capture(&capture_samples(MINIMUM_CAPTURE_SAMPLE_FRAMES - 1))
                .unwrap_err(),
        );
        assert!(
            message.starts_with("room_tone_capture_too_short: "),
            "{message}"
        );
        assert!(
            message.contains("observed=23999 sample frames (499 ms)"),
            "{message}"
        );
        assert!(
            message.contains("allowed=24000 sample frames (500 ms) or more"),
            "{message}"
        );
        assert_eq!(
            fs::read_dir(store.room_tone_dir()).map_or(0, Iterator::count),
            0,
            "a refused capture writes nothing at all",
        );

        let capture = store
            .write_capture(&capture_samples(MINIMUM_CAPTURE_SAMPLE_FRAMES))
            .expect("exactly the floor is a legal capture");
        assert_eq!(capture.milliseconds, ROOM_TONE_MINIMUM_CAPTURE_MILLISECONDS);
        assert_eq!(capture.sample_frames, MINIMUM_CAPTURE_SAMPLE_FRAMES);
        assert_eq!(capture.frames, TimeCode(15));
        assert!(!capture.already_present);
        assert!(capture.path.is_file());
    }

    #[test]
    fn the_capture_ceiling_is_refused_one_sample_frame_above_and_accepted_on_it() {
        let temporary = TempDirectory::new("room-tone-ceiling");
        let store = store_for(&temporary, "project.kinewright");

        let message = backend_message(
            store
                .write_capture(&capture_samples(MAXIMUM_CAPTURE_SAMPLE_FRAMES + 1))
                .unwrap_err(),
        );
        assert!(
            message.starts_with("room_tone_capture_too_long: "),
            "{message}"
        );
        assert!(
            message.contains("observed=2880001 sample frames (60000 ms)"),
            "{message}"
        );
        assert!(
            message.contains("allowed=2880000 sample frames (60000 ms) or fewer"),
            "{message}",
        );

        // The refusal is on the sample-frame count, not on the derived
        // milliseconds: one sample frame past the cap still floors to 60 000 ms,
        // so a millisecond comparison would have let it through.
        assert_eq!(milliseconds_of(MAXIMUM_CAPTURE_SAMPLE_FRAMES + 1), 60_000);

        let capture = store
            .write_capture(&capture_samples(MAXIMUM_CAPTURE_SAMPLE_FRAMES))
            .expect("exactly the ceiling is a legal capture");
        assert_eq!(capture.milliseconds, ROOM_TONE_MAX_CAPTURE_MILLISECONDS);
        assert_eq!(capture.frames, TimeCode(1_800));
        assert_eq!(capture.byte_len, 23_040_044);
        assert!(capture.byte_len < ROOM_TONE_MAX_FILE_BYTES);
        assert_eq!(fs::metadata(&capture.path).unwrap().len(), capture.byte_len);
    }

    #[test]
    fn a_capture_is_truncated_to_a_whole_asset_frame() {
        let temporary = TempDirectory::new("room-tone-truncate");
        let store = store_for(&temporary, "project.kinewright");

        // 15 asset frames plus 1 599 sample frames: the largest truncation
        // `write_capture` can make, and it still lands on the floor.
        let requested = MINIMUM_CAPTURE_SAMPLE_FRAMES + ROOM_TONE_SAMPLE_FRAMES_PER_ASSET_FRAME - 1;
        let capture = store.write_capture(&capture_samples(requested)).unwrap();
        assert_eq!(capture.sample_frames, MINIMUM_CAPTURE_SAMPLE_FRAMES);
        assert_eq!(
            capture.requested_sample_frames, requested,
            "the requested length is reported too, so a surface can say how much was trimmed",
        );
        assert_eq!(
            capture.requested_sample_frames - capture.sample_frames,
            ROOM_TONE_SAMPLE_FRAMES_PER_ASSET_FRAME - 1,
            "the largest truncation the store can make",
        );
        assert_eq!(capture.frames, TimeCode(15));
        assert_eq!(capture.milliseconds, ROOM_TONE_MINIMUM_CAPTURE_MILLISECONDS);
        assert_eq!(
            capture.byte_len,
            MINIMUM_CAPTURE_SAMPLE_FRAMES * u64::from(ROOM_TONE_CHANNELS) * 4
                + u64::try_from(WAV_HEADER_BYTES).unwrap(),
        );

        // The truncated buffer is a prefix: the store never resamples, pads or
        // reorders, so the fill replays exactly what was captured.
        let samples = capture_samples(requested);
        let kept = usize::try_from(MINIMUM_CAPTURE_SAMPLE_FRAMES).unwrap() * 2;
        assert_eq!(
            fs::read(&capture.path).unwrap(),
            room_tone_wav_bytes(&samples[..kept]),
        );
    }

    #[test]
    fn a_malformed_capture_is_refused_by_name() {
        let temporary = TempDirectory::new("room-tone-malformed");
        let store = store_for(&temporary, "project.kinewright");
        let mut samples = capture_samples(MINIMUM_CAPTURE_SAMPLE_FRAMES);
        samples.pop();

        let message = backend_message(store.write_capture(&samples).unwrap_err());
        assert!(
            message.starts_with("room_tone_capture_malformed: "),
            "{message}"
        );
        assert!(message.contains("allowed=a multiple of 2"), "{message}");
    }

    #[test]
    fn the_store_wav_is_byte_identical_to_the_fixture_writer() {
        let samples = capture_samples(MINIMUM_CAPTURE_SAMPLE_FRAMES);
        assert_eq!(
            room_tone_wav_bytes(&samples),
            wav_f32(&samples, ROOM_TONE_SAMPLE_RATE, ROOM_TONE_CHANNELS),
            "the store and the fixture helper must write the same bytes, or the seam \
             lanes would be measuring a different file from the one the product writes",
        );
    }

    #[test]
    fn a_written_capture_probes_and_decodes_bit_exactly() {
        crate::initialize_ffmpeg().expect("FFmpeg initializes");
        let temporary = TempDirectory::new("room-tone-roundtrip");
        let store = store_for(&temporary, "project.kinewright");
        // Two seconds: §5.4's own room-tone sample, 60 asset frames.
        let samples = capture_samples(96_000);
        let capture = store.write_capture(&samples).unwrap();
        assert_eq!(capture.frames, TimeCode(60));

        let asset = probe_path(&capture.path, AssetId(1)).expect("a written capture probes");
        assert_eq!(asset.kind, MediaKind::Audio);
        assert_eq!(asset.fps, RoomToneStore::asset_fps());
        assert_eq!(
            asset.duration, capture.frames,
            "the whole file must be a whole number of asset frames, or §5.3's \
             fill arithmetic would read past its end",
        );
        assert_eq!(asset.name, format!("{}.wav", capture.sha256));

        let decoded = decode_audio_range(
            &capture.path,
            asset.fps,
            TimeCode::ZERO,
            asset.duration,
            ROOM_TONE_SAMPLE_RATE,
            ROOM_TONE_CHANNELS,
            &ExportCancellation::default(),
        )
        .expect("a written capture decodes");

        assert_eq!(decoded.len(), samples.len());
        let differing = decoded
            .iter()
            .zip(&samples)
            .filter(|(decoded, written)| decoded.to_bits() != written.to_bits())
            .count();
        assert_eq!(
            differing, 0,
            "an f32 WAV must round-trip through the decoder bit-exactly: this is \
             what makes §5.4's seam deviation exactly 0.0 rather than 1e-7",
        );
    }

    #[test]
    fn a_second_capture_of_the_same_bytes_leaves_the_store_file_untouched() {
        let temporary = TempDirectory::new("room-tone-idempotent");
        let store = store_for(&temporary, "project.kinewright");
        let samples = capture_samples(MINIMUM_CAPTURE_SAMPLE_FRAMES);

        let first = store.write_capture(&samples).unwrap();
        let before = fs::metadata(&first.path).unwrap();

        let second = store.write_capture(&samples).unwrap();
        assert_eq!(first.sha256, second.sha256);
        assert_eq!(first.path, second.path);
        assert!(!first.already_present);
        assert!(
            second.already_present,
            "AU5 §5.8 rule 116 reads this to emit no operation on a re-capture",
        );

        let after = fs::metadata(&first.path).unwrap();
        assert_eq!(before.modified().unwrap(), after.modified().unwrap());
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt as _;
            assert_eq!(
                before.ino(),
                after.ino(),
                "a correctly hashed store file must not be rewritten",
            );
        }
        assert_eq!(
            fs::read_dir(store.room_tone_dir()).unwrap().count(),
            1,
            "the store is deduplicated by content digest",
        );
    }

    #[test]
    fn availability_reports_verified_missing_and_changed_and_a_changed_file_is_overwritten() {
        let temporary = TempDirectory::new("room-tone-availability");
        let store = store_for(&temporary, "project.kinewright");
        let samples = capture_samples(MINIMUM_CAPTURE_SAMPLE_FRAMES);
        let capture = store.write_capture(&samples).unwrap();

        let verified = store.availability(&capture.sha256);
        assert_eq!(verified.kind, RoomToneAvailabilityKind::Verified);
        assert_eq!(
            verified.observed_sha256.as_deref(),
            Some(capture.sha256.as_str())
        );
        assert!(verified.reason.is_none());
        assert_eq!(verified.path.as_deref(), Some(capture.path.as_path()));
        assert!(store.holds(&capture.sha256));

        let absent = store.availability(ABSENT_DIGEST);
        assert_eq!(absent.kind, RoomToneAvailabilityKind::Missing);
        assert!(absent.reason.unwrap().starts_with("missing_room_tone: "));
        assert!(!store.holds(ABSENT_DIGEST));

        let malformed = store.availability("not-a-digest");
        assert_eq!(malformed.kind, RoomToneAvailabilityKind::Unreadable);
        assert!(
            malformed
                .reason
                .unwrap()
                .starts_with("invalid_room_tone_digest: ")
        );

        // Same length, different bytes: the digest is what catches it.
        let mut corrupted = fs::read(&capture.path).unwrap();
        let last = corrupted.len() - 1;
        corrupted[last] ^= 0xff;
        fs::write(&capture.path, &corrupted).unwrap();
        let changed = store.availability(&capture.sha256);
        assert_eq!(changed.kind, RoomToneAvailabilityKind::Changed);
        assert!(changed.reason.unwrap().starts_with("changed_room_tone: "));
        assert!(!store.holds(&capture.sha256));

        let again = store.write_capture(&samples).unwrap();
        assert_eq!(again.sha256, capture.sha256);
        assert!(!again.already_present);
        assert_eq!(
            fs::read(&capture.path).unwrap(),
            room_tone_wav_bytes(&samples)
        );
        assert_eq!(
            store.availability(&capture.sha256).kind,
            RoomToneAvailabilityKind::Verified
        );
    }

    #[test]
    fn read_capture_refuses_a_file_over_the_cap_from_its_metadata_and_accepts_it_on_the_cap() {
        let temporary = TempDirectory::new("room-tone-cap");
        let store = store_for(&temporary, "project.kinewright");
        fs::create_dir_all(store.room_tone_dir()).unwrap();
        let path = store.path_for(ABSENT_DIGEST).unwrap();

        // Sparse files: the length is read from the metadata before a byte is
        // touched, so the refusal cannot be a parse failure in disguise.
        fs::File::create(&path)
            .unwrap()
            .set_len(ROOM_TONE_MAX_FILE_BYTES + 1)
            .unwrap();
        let message = backend_message(store.read_capture(ABSENT_DIGEST).unwrap_err());
        assert!(
            message.starts_with("room_tone_file_too_large: "),
            "{message}"
        );
        assert!(
            message.contains(&format!("observed={}", ROOM_TONE_MAX_FILE_BYTES + 1)),
            "{message}",
        );
        assert!(
            message.contains(&format!("allowed={ROOM_TONE_MAX_FILE_BYTES}")),
            "{message}",
        );
        assert_eq!(
            store.availability(ABSENT_DIGEST).kind,
            RoomToneAvailabilityKind::Unreadable,
            "availability applies the same metadata-first guard",
        );

        fs::File::create(&path)
            .unwrap()
            .set_len(ROOM_TONE_MAX_FILE_BYTES)
            .unwrap();
        let bytes = store
            .read_capture(ABSENT_DIGEST)
            .expect("exactly the cap is readable");
        assert_eq!(bytes.len() as u64, ROOM_TONE_MAX_FILE_BYTES);

        fs::remove_file(&path).unwrap();
        let message = backend_message(store.read_capture(ABSENT_DIGEST).unwrap_err());
        assert!(message.starts_with("missing_room_tone: "), "{message}");
    }

    /// Three seconds of stereo noise on disk, probed as an audio-only asset.
    fn source_asset(label: &str) -> (GeneratedMedia, MediaAsset, Vec<f32>) {
        crate::initialize_ffmpeg().expect("FFmpeg initializes");
        let samples = capture_samples(144_000);
        let media = GeneratedMedia::from_bytes(
            label,
            "wav",
            &wav_f32(&samples, ROOM_TONE_SAMPLE_RATE, ROOM_TONE_CHANNELS),
        );
        let asset = probe_path(media.path(), AssetId(7)).expect("the source probes");
        assert_eq!(asset.duration, TimeCode(90));
        (media, asset, samples)
    }

    #[test]
    fn capture_room_tone_writes_the_decoded_source_range_under_its_digest() {
        let temporary = TempDirectory::new("room-tone-capture");
        let store = store_for(&temporary, "project.kinewright");
        let (_media, asset, samples) = source_asset("room-tone-capture-source");

        let capture = store
            .capture_room_tone(
                &asset,
                TimeCode(30)..TimeCode(90),
                &ExportCancellation::default(),
            )
            .expect("two seconds of the source is a legal capture");

        assert_eq!(capture.frames, TimeCode(60));
        assert_eq!(capture.sample_frames, 96_000);
        assert_eq!(capture.milliseconds, 2_000);
        assert!(!capture.already_present);

        // The **raw clip source**, sample for sample (AU5 §5.1 rule 91): a
        // capture that had gone through the mix path would not match this.
        let channels = usize::from(ROOM_TONE_CHANNELS);
        let expected = room_tone_wav_bytes(&samples[48_000 * channels..144_000 * channels]);
        assert_eq!(fs::read(&capture.path).unwrap(), expected);
        assert_eq!(capture.sha256, sha256_bytes(&expected));

        let again = store
            .capture_room_tone(
                &asset,
                TimeCode(30)..TimeCode(90),
                &ExportCancellation::default(),
            )
            .expect("a re-capture of the same range succeeds");
        assert_eq!(again.sha256, capture.sha256);
        assert!(
            again.already_present,
            "AU5 §5.8 rule 116: a re-run emits no operation",
        );
        assert_eq!(fs::read_dir(store.room_tone_dir()).unwrap().count(), 1);
    }

    #[test]
    fn capture_room_tone_refuses_a_bad_or_over_long_range_before_it_decodes_anything() {
        let temporary = TempDirectory::new("room-tone-capture-refusals");
        let store = store_for(&temporary, "project.kinewright");
        let (_media, asset, _samples) = source_asset("room-tone-capture-refusal-source");
        let cancellation = ExportCancellation::default();

        for (range, code) in [
            (TimeCode(30)..TimeCode(30), "room_tone_capture_malformed: "),
            (TimeCode(-1)..TimeCode(30), "room_tone_capture_malformed: "),
            (TimeCode(30)..TimeCode(91), "room_tone_capture_malformed: "),
            (TimeCode(0)..TimeCode(14), "room_tone_capture_too_short: "),
        ] {
            let message = backend_message(
                store
                    .capture_room_tone(&asset, range.clone(), &cancellation)
                    .unwrap_err(),
            );
            assert!(message.starts_with(code), "{range:?}: {message}");
        }
        // 14 frames is 466 ms; 15 is exactly the floor.
        let capture = store
            .capture_room_tone(&asset, TimeCode(0)..TimeCode(15), &cancellation)
            .expect("exactly the floor is a legal capture");
        assert_eq!(capture.milliseconds, ROOM_TONE_MINIMUM_CAPTURE_MILLISECONDS);

        // AU5 §0 R105's own argument, pinned: a range far longer than the cap
        // is refused **before** a byte is decoded.
        //
        // The discriminator is the **path, which does not exist**. Pointing the
        // over-long asset at the real three-second file would prove nothing:
        // `decode_audio_range` treats EOF as zero padding
        // (`samples.resize(expected, 0.0)`, audio.rs), so a decode-first
        // implementation would hand `write_capture` 288 000 000 samples and its
        // own `check_capture_span` would answer the identical
        // `room_tone_capture_too_long` message. With nothing at the path, a
        // decode-first order fails with the decoder's own open error instead,
        // so only a guard-first order can produce the refusal asserted below.
        let long_asset = MediaAsset {
            duration: TimeCode(90_000),
            path: temporary.path("no-such-room-tone.wav"),
            ..asset.clone()
        };
        assert!(
            !long_asset.path.exists(),
            "the discriminator is that nothing is there"
        );
        let requested = frame_to_samples(TimeCode(90_000), ROOM_TONE_SAMPLE_RATE, long_asset.fps);
        assert_eq!(requested, 144_000_000);
        let message = backend_message(
            store
                .capture_room_tone(&long_asset, TimeCode(0)..TimeCode(90_000), &cancellation)
                .unwrap_err(),
        );
        assert!(
            message.starts_with("room_tone_capture_too_long: "),
            "{message}",
        );
        assert!(
            message.contains("observed=144000000 sample frames"),
            "{message}"
        );
        assert_eq!(
            fs::read_dir(store.room_tone_dir()).map_or(0, Iterator::count),
            1,
            "a refused capture writes nothing; only the floor capture above is there",
        );

        let video_only = MediaAsset {
            kind: MediaKind::Video,
            ..asset.clone()
        };
        let message = backend_message(
            store
                .capture_room_tone(&video_only, TimeCode(0)..TimeCode(60), &cancellation)
                .unwrap_err(),
        );
        assert!(
            message.starts_with("room_tone_capture_malformed: "),
            "{message}"
        );
        assert!(message.contains("carries no audio"), "{message}");
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_store_entry_is_missing_and_refuses_a_write() {
        let temporary = TempDirectory::new("room-tone-symlink");
        let store = store_for(&temporary, "project.kinewright");
        fs::create_dir_all(store.room_tone_dir()).unwrap();
        let outside = temporary.path("outside.wav");
        let samples = capture_samples(MINIMUM_CAPTURE_SAMPLE_FRAMES);
        fs::write(&outside, room_tone_wav_bytes(&samples)).unwrap();
        let digest = crate::sha256::sha256_bytes(&room_tone_wav_bytes(&samples));
        std::os::unix::fs::symlink(&outside, store.path_for(&digest).unwrap()).unwrap();

        assert_eq!(
            store.availability(&digest).kind,
            RoomToneAvailabilityKind::Missing,
            "a symlink is never followed out of the project directory",
        );
        let message = backend_message(store.write_capture(&samples).unwrap_err());
        assert!(
            message.starts_with("room_tone_store_root_invalid: "),
            "{message}"
        );
        assert!(message.contains("the store file is a symlink"), "{message}");
    }
}
