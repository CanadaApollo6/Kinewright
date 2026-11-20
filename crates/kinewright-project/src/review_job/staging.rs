use std::{
    fs::File,
    os::fd::AsRawFd,
    path::{Path, PathBuf},
};

use rustix::fs::{self, AtFlags, FileType, Mode, OFlags};

use super::ReviewStage;
use crate::{bundle::BundleError, store::ProjectStore};

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

impl ProjectStore {
    pub fn create_review_staging(&self, stage: &ReviewStage) -> Result<ReviewStaging, BundleError> {
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
