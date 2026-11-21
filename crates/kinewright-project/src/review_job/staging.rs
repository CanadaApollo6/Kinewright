use std::{
    collections::BTreeSet,
    fs::File,
    os::fd::AsRawFd,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use rustix::fs::{self, AtFlags, FileType, Mode, OFlags};

use super::ReviewStage;
use crate::{
    bundle::BundleError, bundle_records::SessionId, storage_budget::StorageReservation,
    store::ProjectStore,
};

const OUTPUT: &str = "encoded.mp4";
const DIR: OFlags = OFlags::RDONLY
    .union(OFlags::DIRECTORY)
    .union(OFlags::NOFOLLOW)
    .union(OFlags::CLOEXEC);

/// All paths passed to owned export are relative to this pinned directory.
pub struct ReviewStaging {
    directory: File,
}

impl ReviewStaging {
    #[must_use]
    pub fn destination(&self) -> PathBuf {
        Path::new("/proc/self/fd")
            .join(self.directory.as_raw_fd().to_string())
            .join(OUTPUT)
    }

    pub fn open_output(&self) -> Result<File, BundleError> {
        let descriptor = fs::openat(
            &self.directory,
            OUTPUT,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
            Mode::empty(),
        )?;
        if FileType::from_raw_mode(fs::fstat(&descriptor)?.st_mode) != FileType::RegularFile {
            return Err(BundleError::Identity(
                "review output is not a regular file".into(),
            ));
        }
        Ok(File::from(descriptor))
    }
}

struct LiveStage {
    id: SessionId,
    bytes: u64,
    finished: bool,
    directory: Option<File>,
    /// The stage's allocated blocks are what it took from free space: no
    /// compression, no copy on write. Otherwise a stage still being written
    /// earns no credit.
    exact: bool,
}

/// The one review stage whose charge is live in this store.
#[derive(Default)]
pub(crate) struct LiveStageSlot(Arc<Mutex<Option<LiveStage>>>);

impl LiveStageSlot {
    /// Bytes the stage already holds on disk under its own charge. They lower
    /// free space and stay in the reservation, so the check credits them once.
    /// Once the writer finished, no further staging bytes can land.
    pub(crate) fn credit(&self) -> u64 {
        let Ok(slot) = self.0.lock() else {
            return 0;
        };
        let Some(live) = slot.as_ref() else {
            return 0;
        };
        if live.finished {
            return live.bytes;
        }
        if !live.exact {
            return 0;
        }
        live.directory
            .as_ref()
            .and_then(|directory| staged_bytes(directory).ok())
            .map_or(0, |staged| staged.min(live.bytes))
    }
}

impl LiveStageSlot {
    #[cfg(any(test, feature = "test-util"))]
    pub(crate) fn on_disk(&self) -> u64 {
        self.0
            .lock()
            .ok()
            .and_then(|slot| {
                slot.as_ref()?
                    .directory
                    .as_ref()
                    .and_then(|directory| staged_bytes(directory).ok())
            })
            .unwrap_or(0)
    }
}

fn staged_bytes(directory: &File) -> rustix::io::Result<u64> {
    let mut inodes = BTreeSet::new();
    let mut total = 0_u64;
    for entry in fs::Dir::read_from(directory)? {
        let entry = entry?;
        let name = entry.file_name();
        if matches!(name.to_bytes(), b"." | b"..") {
            continue;
        }
        // An entry removed between the listing and its stat holds nothing.
        let stat = match fs::statat(directory, name, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(stat) => stat,
            Err(rustix::io::Errno::NOENT) => continue,
            Err(error) => return Err(error),
        };
        if FileType::from_raw_mode(stat.st_mode) == FileType::RegularFile
            && inodes.insert(stat.st_ino)
        {
            let blocks = u64::try_from(stat.st_blocks).unwrap_or(0);
            total = total.saturating_add(blocks.saturating_mul(512));
        }
    }
    Ok(total)
}

/// Whether a file's `st_blocks` in this empty directory will equal what it
/// takes from free space. On btrfs the directory is made no-copy-on-write,
/// which also turns off compression for the files created in it; other file
/// systems that compress or copy on write, and unknown ones, are not trusted.
fn blocks_are_free_space(directory: &File) -> bool {
    const EXT4: u32 = 0xef53;
    const XFS: u32 = 0x5846_5342;
    const TMPFS: u32 = 0x0102_1994;
    const BTRFS: u32 = 0x9123_683e;
    let Ok(statfs) = fs::fstatfs(directory) else {
        return false;
    };
    // `f_type` is a signed word on some targets; magic numbers are 32 bits.
    #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
    let kind = statfs.f_type as u32;
    match kind {
        EXT4 | XFS | TMPFS => true,
        BTRFS => {
            let no_cow = || -> rustix::io::Result<bool> {
                let flags = fs::ioctl_getflags(directory)?;
                fs::ioctl_setflags(directory, flags | fs::IFlags::NOCOW)?;
                Ok(fs::ioctl_getflags(directory)?.contains(fs::IFlags::NOCOW))
            };
            no_cow().unwrap_or(false)
        }
        _ => false,
    }
}

/// The staging charge of one review stage. Dropping it withdraws the disk
/// credit before the reservation is released.
pub struct ReviewStageCharge {
    slot: Arc<Mutex<Option<LiveStage>>>,
    id: SessionId,
    _reservation: StorageReservation,
}

impl ReviewStageCharge {
    /// The writer returned: the stage cannot grow past what it holds now.
    pub fn finish_writing(&self) {
        if let Ok(mut slot) = self.slot.lock()
            && let Some(live) = slot.as_mut()
            && live.id == self.id
        {
            live.finished = true;
        }
    }
}

impl Drop for ReviewStageCharge {
    fn drop(&mut self) {
        if let Ok(mut slot) = self.slot.lock()
            && slot.as_ref().is_some_and(|live| live.id == self.id)
        {
            *slot = None;
        }
    }
}

impl ProjectStore {
    pub fn reserve_review_stage(
        &self,
        stage: &ReviewStage,
    ) -> Result<ReviewStageCharge, BundleError> {
        let mut reservation = self.reserve_copy(stage.byte_limit)?;
        reservation.charge_review_stage()?;
        let slot = &self.review_stage_slot().0;
        let mut live = slot
            .lock()
            .map_err(|_| BundleError::Capacity("review stage slot poisoned"))?;
        if live.is_some() {
            return Err(BundleError::Identity("another review stage is live".into()));
        }
        *live = Some(LiveStage {
            id: stage.id.clone(),
            bytes: stage.byte_limit,
            finished: false,
            directory: None,
            exact: false,
        });
        Ok(ReviewStageCharge {
            slot: Arc::clone(slot),
            id: stage.id.clone(),
            _reservation: reservation,
        })
    }

    pub fn create_review_staging(
        &self,
        stage: &ReviewStage,
        charge: &ReviewStageCharge,
    ) -> Result<ReviewStaging, BundleError> {
        if charge.id != stage.id || !Arc::ptr_eq(&charge.slot, &self.review_stage_slot().0) {
            return Err(BundleError::Identity(
                "review stage charge belongs to another stage".into(),
            ));
        }
        let parent = self.with_namespace(|namespace| namespace.review_staging_parent(true))?;
        fs::mkdirat(
            &parent,
            stage.id.as_str(),
            Mode::RUSR | Mode::WUSR | Mode::XUSR,
        )?;
        let directory = File::from(fs::openat(&parent, stage.id.as_str(), DIR, Mode::empty())?);
        if fs::fstat(&directory)?.st_dev != fs::fstat(&parent)?.st_dev {
            return Err(BundleError::Identity(
                "review stage crosses the admitted filesystem".into(),
            ));
        }
        parent.sync_all()?;
        let exact = blocks_are_free_space(&directory);
        // The credit reads the stage through its own descriptor.
        let observed = directory.try_clone()?;
        if let Ok(mut slot) = charge.slot.lock()
            && let Some(live) = slot.as_mut()
            && live.id == stage.id
        {
            live.directory = Some(observed);
            live.exact = exact;
        }
        Ok(ReviewStaging { directory })
    }

    /// Call only after the named stage's worker has retired.
    pub fn sweep_review_staging(&self, stage: &ReviewStage) -> Result<(), BundleError> {
        let parent = match self.with_namespace(|namespace| namespace.review_staging_parent(false)) {
            Ok(parent) => parent,
            Err(BundleError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(());
            }
            Err(error) => return Err(error),
        };
        let directory = match fs::openat(&parent, stage.id.as_str(), DIR, Mode::empty()) {
            Ok(directory) => directory,
            Err(rustix::io::Errno::NOENT) => return Ok(()),
            Err(error) => return Err(error.into()),
        };
        let stat = fs::fstat(&directory)?;
        if stat.st_dev != fs::fstat(&parent)?.st_dev {
            return Err(BundleError::Identity(
                "review stage crosses the admitted filesystem".into(),
            ));
        }
        for entry in fs::Dir::read_from(&directory)? {
            let entry = entry?;
            let name = entry.file_name();
            let bytes = name.to_bytes();
            if matches!(bytes, b"." | b"..") {
                continue;
            }
            let owned_temporary = bytes
                .strip_prefix(b".kinewright-owned-")
                .is_some_and(|suffix| {
                    suffix.len() == 32 && suffix.iter().all(u8::is_ascii_hexdigit)
                });
            if bytes != OUTPUT.as_bytes() && !owned_temporary {
                return Err(BundleError::Identity(
                    "unexpected review stage entry".into(),
                ));
            }
            fs::unlinkat(&directory, name, AtFlags::empty())?;
        }
        fs::fsync(&directory)?;
        let current = fs::statat(&parent, stage.id.as_str(), AtFlags::SYMLINK_NOFOLLOW)?;
        if (current.st_dev, current.st_ino) != (stat.st_dev, stat.st_ino) {
            return Err(BundleError::Identity(
                "review stage directory was replaced".into(),
            ));
        }
        fs::unlinkat(&parent, stage.id.as_str(), AtFlags::REMOVEDIR)?;
        parent.sync_all()?;
        Ok(())
    }

    #[must_use]
    pub fn reserve_review_working(
        &self,
    ) -> std::sync::Arc<kinewright_media::owned_io::CursorReservation> {
        self.decoder_reserver()
    }
}
