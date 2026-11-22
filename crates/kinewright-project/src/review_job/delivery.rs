//! JA-1 S6 C2: save a sealed cut into a folder the person picked.
//!
//! The save copies the exact bytes a seal checked. It pins the receipt, the
//! artifact and the media under the writer lock, then copies off the lock,
//! hashing every byte it writes. Nothing appears under the final name unless
//! that hash equals the receipt's `media_ref`: the folder is staged under an
//! exclusive hidden name and renamed into place with no-replace.
use kinewright_core::output_review::{
    CoverageProfileId, DeliveryException, EligibilityReason, SealedReviewOutcome,
};
use serde::{Deserialize, Serialize};

use super::ReviewReceipt;
#[cfg(target_os = "linux")]
use crate::store::ProjectStore;
use crate::{
    bundle::{BundleError, ProjectBundle},
    bundle_records::ObjectRef,
    store::StorePin,
    studio_records::{SnapshotKey, VersionId},
};

/// What a running or finished save reports through host status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum SealedSaveStatus {
    Saving { name: String },
    Saved { path: std::path::PathBuf },
    Failed { reason: String },
}

/// The words Studio, the folder name and `receipt.txt` use for an outcome.
/// A `ReviewedFactsNotChecked` cut is never called delivered (L2).
#[must_use]
pub fn outcome_label(outcome: &SealedReviewOutcome) -> &'static str {
    match outcome {
        SealedReviewOutcome::Delivered => "Delivered",
        SealedReviewOutcome::ReviewedFactsNotChecked { .. } => "Reviewed, facts not checked",
    }
}

/// The Studio menu item for saving a cut with this outcome.
#[must_use]
pub fn save_action_label(outcome: &SealedReviewOutcome) -> &'static str {
    match outcome {
        SealedReviewOutcome::Delivered => "Save delivered cut…",
        SealedReviewOutcome::ReviewedFactsNotChecked { .. } => {
            "Save reviewed cut (facts not checked)…"
        }
    }
}

const MAX_PROJECT_NAME: usize = 64;

/// `<project>-<version>-delivered` or
/// `<project>-<version>-reviewed-facts-not-checked`. The project part keeps
/// letters, digits, `.`, `_` and `-`; anything else becomes `-`, and it never
/// starts with `.`, so the name is one visible path component.
#[must_use]
pub fn sealed_cut_name(
    project: &str,
    version: &VersionId,
    outcome: &SealedReviewOutcome,
) -> String {
    let mut stem: String = project
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '-'
            }
        })
        .take(MAX_PROJECT_NAME)
        .collect();
    while stem.starts_with('.') {
        stem.remove(0);
    }
    if stem.is_empty() {
        stem.push_str("project");
    }
    let label = match outcome {
        SealedReviewOutcome::Delivered => "delivered",
        SealedReviewOutcome::ReviewedFactsNotChecked { .. } => "reviewed-facts-not-checked",
    };
    format!("{stem}-{}-{label}", version.as_str())
}

/// One element a facts-not-checked cut did not check, in plain words.
#[must_use]
pub fn describe_exception(exception: &DeliveryException) -> String {
    match exception {
        DeliveryException::Element { element, reason } => {
            let reason = match reason {
                EligibilityReason::UnknownText(_) => "its text could not be read",
                EligibilityReason::EmptyText => "it has no text",
                EligibilityReason::CaptionSpeechNotDetermined => "its speech was not determined",
                EligibilityReason::CaptionWordsDisagree => "its words differ from the speech",
                EligibilityReason::CaptionWordClipped => "a caption word is clipped",
                EligibilityReason::SourcedText => "it is sourced",
                EligibilityReason::NumericCharacter => "it contains a number",
                EligibilityReason::NumericTextUncomparable => "its numbers could not be compared",
                EligibilityReason::CaptionTextUncomparable => "its text could not be compared",
            };
            format!("Clip {}: {reason}", element.0)
        }
        DeliveryException::OverlayInventoryNotDetermined(kind) => {
            format!("The {kind:?} overlays could not be listed")
        }
    }
}

fn hundredths(value: i32) -> String {
    let sign = if value < 0 { "-" } else { "" };
    let value = value.unsigned_abs();
    format!("{sign}{}.{:02}", value / 100, value % 100)
}

/// The receipt sheet that travels with the video, written from the receipt
/// whose `media_ref` the copy was checked against.
#[must_use]
pub fn receipt_sheet(receipt: &ReviewReceipt, video: &str) -> String {
    use std::fmt::Write as _;
    let key = &receipt.key;
    let settings = &key.settings;
    let mut sheet = String::new();
    let _ = writeln!(sheet, "Kinewright sealed cut");
    let _ = writeln!(sheet, "Label: {}", outcome_label(&receipt.outcome));
    let _ = writeln!(sheet, "Version: {}", key.snapshot.version.as_str());
    let profile = match key.profile {
        CoverageProfileId::OutputReviewV1 => "output_review/1",
    };
    let _ = writeln!(sheet, "Coverage profile: {profile}");
    let _ = writeln!(sheet, "Video: {video}");
    let _ = writeln!(sheet, "Video SHA-256: {}", key.media_ref.sha256);
    let _ = writeln!(sheet, "Video bytes: {}", key.media_ref.byte_length);
    let _ = writeln!(sheet, "Receipt key SHA-256: {}", receipt.key_sha256);
    let _ = writeln!(sheet, "Settings:");
    let _ = writeln!(
        sheet,
        "  Resolution: {}x{}",
        settings.resolution.0, settings.resolution.1
    );
    let _ = writeln!(
        sheet,
        "  Frame rate: {}/{}",
        settings.fps.numerator(),
        settings.fps.denominator()
    );
    let _ = writeln!(
        sheet,
        "  Video: {} at {} bit/s",
        settings.video_codec, settings.video_bitrate
    );
    let _ = writeln!(
        sheet,
        "  Audio: {} at {} bit/s",
        settings.audio_codec, settings.audio_bitrate
    );
    match &settings.loudness_normalization {
        Some(target) => {
            let _ = writeln!(
                sheet,
                "  Loudness target: {} LUFS integrated, within {} LU, true peak at most {} dBTP",
                hundredths(target.integrated_lufs_hundredths),
                hundredths(target.tolerance_lu_hundredths),
                hundredths(target.true_peak_ceiling_dbtp_hundredths),
            );
        }
        None => {
            let _ = writeln!(sheet, "  Loudness target: none");
        }
    }
    match &receipt.outcome {
        SealedReviewOutcome::Delivered => {
            let _ = writeln!(sheet, "Not checked: nothing");
        }
        SealedReviewOutcome::ReviewedFactsNotChecked { exceptions } => {
            let _ = writeln!(sheet, "Not checked:");
            for exception in exceptions {
                let _ = writeln!(sheet, "  - {}", describe_exception(exception));
            }
        }
    }
    sheet
}

/// A sealed cut whose receipt, artifact and media stay pinned until the save
/// worker drops it. Reading it needs no writer lock.
pub struct SealedCut {
    pub receipt_ref: ObjectRef,
    pub receipt: ReviewReceipt,
    // The copy is Linux-only, like the folder admission it writes under.
    #[cfg(target_os = "linux")]
    store: ProjectStore,
    _pin: StorePin,
}

/// The sealed receipt of `snapshot` under `coverage`, if any.
pub fn receipt_for_profile(
    receipts: impl IntoIterator<Item = ReviewReceipt>,
    coverage: CoverageProfileId,
) -> Option<ReviewReceipt> {
    receipts
        .into_iter()
        .find(|receipt| receipt.key.profile == coverage)
}

pub const NO_SEALED_RECEIPT: &str = "This version has no sealed output review to save.";
pub const OTHER_RECEIPT: &str =
    "The sealed review of this version differs from the one Studio showed. Refresh and try again.";

impl ProjectBundle {
    /// The paths a save must never land inside: the bundle and its owned
    /// store, `<dir>/<stem>.kinewright-assets`, where every object lives.
    ///
    /// # Errors
    /// The store root cannot be read.
    pub fn owned_roots(&self) -> Result<Vec<std::path::PathBuf>, BundleError> {
        let root = self.store.resolved_root()?;
        let mut assets = root
            .file_stem()
            .ok_or_else(|| BundleError::Identity("missing stem".into()))?
            .to_os_string();
        assets.push(".kinewright-assets");
        let assets = root.with_file_name(assets);
        Ok(vec![root, assets])
    }

    /// Pin the sealed cut of `snapshot` under `coverage` whose receipt key
    /// digest is `key_sha256`. Reads only sealed records, never the head,
    /// the history or the working document.
    ///
    /// # Errors
    /// The root is unsettled, the version is missing, it has no receipt for
    /// `coverage`, or the receipt's key digest differs.
    pub fn pin_sealed_cut(
        &self,
        snapshot: &SnapshotKey,
        coverage: CoverageProfileId,
        key_sha256: &str,
    ) -> Result<SealedCut, BundleError> {
        // A receipt on an unsettled root may still roll back.
        if self.review_root_unsettled() {
            return Err(BundleError::Unsettled);
        }
        let descriptor = self.review_descriptor(snapshot)?;
        let (receipt_ref, receipt) = self
            .review_receipts(&descriptor)?
            .into_iter()
            .find(|(_, receipt)| receipt.key.profile == coverage)
            .ok_or_else(|| BundleError::Identity(NO_SEALED_RECEIPT.into()))?;
        if receipt.key_sha256 != key_sha256 || &receipt.key.snapshot != snapshot {
            return Err(BundleError::Identity(OTHER_RECEIPT.into()));
        }
        let receipt = receipt.take();
        let pin = self.store.pin(&[
            receipt_ref.clone(),
            receipt.key.artifact_ref.clone(),
            receipt.key.media_ref.clone(),
        ])?;
        Ok(SealedCut {
            receipt_ref,
            receipt,
            #[cfg(target_os = "linux")]
            store: pin.read_store(),
            _pin: pin,
        })
    }
}

#[cfg(target_os = "linux")]
pub use linux::{SaveDestination, admit_save_folder};

#[cfg(target_os = "linux")]
mod linux {
    use std::{
        ffi::OsStr,
        fs::File,
        io::{self, Read, Write},
        os::{fd::AsRawFd as _, unix::fs::MetadataExt as _},
        path::{Path, PathBuf},
    };

    use rustix::fs::{self, AtFlags, Mode, OFlags, RenameFlags};

    use super::{SealedCut, receipt_sheet};
    use crate::{bundle::BundleError, bundle_records::SessionId};

    const CHUNK: usize = 1 << 20;
    /// Room kept free beyond the video and its sheet.
    const SPACE_MARGIN: u64 = 1 << 20;
    pub const INSIDE_PROJECT: &str =
        "Choose a folder outside the project and its storage to save the cut.";
    pub const NAME_TAKEN: &str =
        "A file or folder with that name already exists there; nothing was changed.";
    pub const HASH_MISMATCH: &str =
        "The copied video does not match the sealed review; nothing was saved.";
    pub const SAVE_STOPPED: &str = "The save was stopped; nothing was saved.";

    /// A folder the person picked, held open by its directory handle. Every
    /// write is relative to this handle, never to its path.
    pub struct SaveDestination {
        parent: File,
        display: PathBuf,
    }

    fn refused(message: &str) -> BundleError {
        BundleError::Identity(message.into())
    }

    fn identity(file: &File) -> io::Result<(u64, u64)> {
        let metadata = file.metadata()?;
        Ok((metadata.dev(), metadata.ino()))
    }

    /// Admit `folder` as a save destination. It must be an existing absolute
    /// directory reached without a symlink in its last component, and it must
    /// not be, or lie inside, any of `refused_roots` (the bundle, the project
    /// store, the application data). The check walks the held handle's own
    /// ancestors by device and inode, so a path spelling cannot slip past it.
    ///
    /// # Errors
    /// The folder is unusable or inside a refused root.
    pub fn admit_save_folder(
        folder: &Path,
        refused_roots: &[&Path],
    ) -> Result<SaveDestination, BundleError> {
        if !folder.is_absolute() {
            return Err(refused("Choose an absolute folder to save the cut."));
        }
        let canonical = folder.canonicalize()?;
        let parent = File::from(fs::open(
            &canonical,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )?);
        let held = Path::new("/proc/self/fd").join(parent.as_raw_fd().to_string());
        if held.canonicalize()? != canonical {
            return Err(refused("The chosen folder changed while it was opened."));
        }
        let mut refused_ids = Vec::new();
        for root in refused_roots {
            // A root that does not exist cannot contain the folder.
            if let Ok(metadata) = std::fs::metadata(root) {
                refused_ids.push((metadata.dev(), metadata.ino()));
            }
        }
        let mut current = parent.try_clone()?;
        for _ in 0..4096 {
            let id = identity(&current)?;
            if refused_ids.contains(&id) {
                return Err(refused(INSIDE_PROJECT));
            }
            let up = File::from(fs::openat(
                &current,
                "..",
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
                Mode::empty(),
            )?);
            if identity(&up)? == id {
                return Ok(SaveDestination {
                    parent,
                    display: canonical,
                });
            }
            current = up;
        }
        Err(refused("The chosen folder is nested too deeply."))
    }

    impl SaveDestination {
        #[must_use]
        pub fn display(&self) -> &Path {
            &self.display
        }

        /// Whether `name` already exists directly under the folder.
        ///
        /// # Errors
        /// The folder cannot be read.
        pub fn holds(&self, name: &str) -> Result<bool, BundleError> {
            match fs::statat(&self.parent, name, AtFlags::SYMLINK_NOFOLLOW) {
                Ok(_) => Ok(true),
                Err(rustix::io::Errno::NOENT) => Ok(false),
                Err(error) => Err(io::Error::from(error).into()),
            }
        }

        fn available(&self) -> Result<u64, BundleError> {
            let stat = fs::fstatvfs(&self.parent)?;
            Ok(stat.f_bavail.saturating_mul(stat.f_frsize))
        }
    }

    /// The hidden staging folder; removed on drop unless it was published.
    struct Staging<'a> {
        parent: &'a File,
        name: String,
        directory: File,
        files: Vec<String>,
        published: bool,
    }

    impl Drop for Staging<'_> {
        fn drop(&mut self) {
            if self.published {
                return;
            }
            // Only names this save created.
            for file in &self.files {
                let _ = fs::unlinkat(&self.directory, file.as_str(), AtFlags::empty());
            }
            let _ = fs::unlinkat(self.parent, self.name.as_str(), AtFlags::REMOVEDIR);
        }
    }

    impl Staging<'_> {
        fn create(&mut self, file: String) -> Result<File, BundleError> {
            let created = fs::openat(
                &self.directory,
                file.as_str(),
                OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::from_raw_mode(0o644),
            )?;
            self.files.push(file);
            Ok(File::from(created))
        }
    }

    fn hex(digest: &[u8; 32]) -> String {
        use std::fmt::Write as _;
        digest.iter().fold(String::with_capacity(64), |mut out, b| {
            let _ = write!(out, "{b:02x}");
            out
        })
    }

    pub const RECEIPT_FILE: &str = "receipt.txt";

    /// The video inside the saved folder carries the folder's name and label.
    #[must_use]
    pub fn video_file(name: &str) -> String {
        format!("{name}.mp4")
    }

    impl SealedCut {
        /// Copy the pinned media into `video`, hashing every byte written;
        /// the bytes must equal the receipt's `media_ref`.
        fn copy_hashed(
            &self,
            video: &mut File,
            cancel: &kinewright_core::ExportCancellation,
            observe: &mut dyn FnMut(u64, &mut [u8]) -> io::Result<()>,
        ) -> Result<(), BundleError> {
            let media = &self.receipt.key.media_ref;
            let mut source = self.store.open_owned_for_hashed_copy(media)?;
            let mut hash = kinewright_hash::Sha256::new();
            let mut buffer = vec![0_u8; CHUNK];
            let mut copied = 0_u64;
            loop {
                if cancel.is_cancelled() {
                    return Err(refused(SAVE_STOPPED));
                }
                let read = match source.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(read) => read,
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(error) => return Err(error.into()),
                };
                copied = copied.saturating_add(read as u64);
                if copied > media.byte_length {
                    return Err(refused(HASH_MISMATCH));
                }
                let chunk = &mut buffer[..read];
                observe(copied, chunk)?;
                hash.update(chunk);
                video.write_all(chunk)?;
            }
            if copied != media.byte_length || hex(&hash.finalize()) != media.sha256 {
                return Err(refused(HASH_MISMATCH));
            }
            Ok(())
        }

        /// Save the sealed bytes and their receipt sheet as `<name>` under
        /// `destination`. Runs off the writer lock. `observe` sees each chunk
        /// after it is read and before it is hashed and written; production
        /// passes a no-op, tests use it to stall, fail or alter a chunk.
        ///
        /// # Errors
        /// The name exists, space is short, the copy fails or is stopped, or
        /// the copied bytes differ from the receipt. The staging folder is
        /// removed on every error.
        pub fn save(
            &self,
            destination: &SaveDestination,
            name: &str,
            cancel: &kinewright_core::ExportCancellation,
            observe: &mut dyn FnMut(u64, &mut [u8]) -> io::Result<()>,
        ) -> Result<PathBuf, BundleError> {
            if name.is_empty()
                || name.starts_with('.')
                || name.contains('/')
                || Path::new(name).file_name() != Some(OsStr::new(name))
            {
                return Err(refused("The save name is not one folder name."));
            }
            if destination.holds(name)? {
                return Err(refused(NAME_TAKEN));
            }
            let media = &self.receipt.key.media_ref;
            let video_name = video_file(name);
            let sheet = receipt_sheet(&self.receipt, &video_name);
            let needed = media
                .byte_length
                .saturating_add(sheet.len() as u64)
                .saturating_add(SPACE_MARGIN);
            if destination.available()? < needed {
                return Err(BundleError::Capacity(
                    "not enough free space in the chosen folder for the cut",
                ));
            }
            let staging_name = format!(".{name}.partial-{}", SessionId::fresh()?.as_str());
            fs::mkdirat(
                &destination.parent,
                staging_name.as_str(),
                Mode::from_raw_mode(0o755),
            )?;
            let directory = match fs::openat(
                &destination.parent,
                staging_name.as_str(),
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            ) {
                Ok(directory) => File::from(directory),
                Err(error) => {
                    let _ = fs::unlinkat(
                        &destination.parent,
                        staging_name.as_str(),
                        AtFlags::REMOVEDIR,
                    );
                    return Err(io::Error::from(error).into());
                }
            };
            let mut staging = Staging {
                parent: &destination.parent,
                name: staging_name,
                directory,
                files: Vec::new(),
                published: false,
            };
            let mut video = staging.create(video_name)?;
            self.copy_hashed(&mut video, cancel, observe)?;
            video.sync_all()?;
            drop(video);
            let mut receipt = staging.create(RECEIPT_FILE.to_owned())?;
            receipt.write_all(sheet.as_bytes())?;
            receipt.sync_all()?;
            drop(receipt);
            fs::fsync(&staging.directory)?;
            if cancel.is_cancelled() {
                return Err(refused(SAVE_STOPPED));
            }
            match fs::renameat_with(
                &destination.parent,
                staging.name.as_str(),
                &destination.parent,
                name,
                RenameFlags::NOREPLACE,
            ) {
                Ok(()) => staging.published = true,
                Err(rustix::io::Errno::EXIST) => return Err(refused(NAME_TAKEN)),
                Err(error) => return Err(io::Error::from(error).into()),
            }
            fs::fsync(&destination.parent).map_err(|error| {
                BundleError::Identity(format!(
                    "The cut was saved as {name}, but the folder could not be flushed to disk: {error}"
                ))
            })?;
            Ok(destination.display.join(name))
        }
    }
}

#[cfg(target_os = "linux")]
pub use linux::{
    HASH_MISMATCH, INSIDE_PROJECT, NAME_TAKEN, RECEIPT_FILE, SAVE_STOPPED, video_file,
};
