//! Directory handles used by legacy writers. Paths passed to IO stay below
//! the admitted directory even if a caller's alias changes during a write.
use std::{
    fs::File,
    io,
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Debug, Clone)]
pub struct PinnedDirectory {
    directory: Arc<File>,
    path: PathBuf,
    #[cfg(windows)]
    _ancestors: Arc<Vec<File>>,
}

impl PinnedDirectory {
    /// Pin an existing directory without creating it.
    /// # Errors
    /// Refuses an unreadable directory or unsupported native handle profile.
    pub fn open(path: &Path) -> io::Result<Self> {
        if std::fs::symlink_metadata(path)?.file_type().is_symlink() {
            return Err(io::Error::other("writer directory is a symlink"));
        }
        let resolved = std::fs::canonicalize(path)?;
        #[cfg(target_os = "linux")]
        let directory = File::from(rustix::fs::open(
            path,
            rustix::fs::OFlags::RDONLY
                | rustix::fs::OFlags::DIRECTORY
                | rustix::fs::OFlags::NOFOLLOW
                | rustix::fs::OFlags::CLOEXEC,
            rustix::fs::Mode::empty(),
        )?);
        #[cfg(windows)]
        let (directory, ancestors) = {
            use std::os::windows::fs::{MetadataExt as _, OpenOptionsExt as _};
            let open = |p: &Path| -> io::Result<File> {
                let file = File::options()
                    .read(true)
                    .share_mode(3)
                    .custom_flags(0x0220_0000)
                    .open(p)?;
                let metadata = file.metadata()?;
                if !metadata.is_dir() || metadata.file_attributes() & 0x400 != 0 {
                    return Err(io::Error::other("reparse writer directory"));
                }
                Ok(file)
            };
            let mut ancestors = Vec::new();
            for ancestor in path.ancestors().skip(1) {
                ancestors.push(open(ancestor)?);
            }
            (open(path)?, ancestors)
        };
        #[cfg(not(any(target_os = "linux", windows)))]
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "pinned legacy IO requires Linux or Windows",
        ));
        #[cfg(any(target_os = "linux", windows))]
        let result = Self {
            directory: Arc::new(directory),
            path: resolved,
            #[cfg(windows)]
            _ancestors: Arc::new(ancestors),
        };
        #[cfg(any(target_os = "linux", windows))]
        {
            result.verify()?;
            Ok(result)
        }
    }

    /// Native path relative to the held directory, never a caller alias.
    #[must_use]
    pub fn io_path(&self) -> PathBuf {
        #[cfg(target_os = "linux")]
        {
            use std::os::fd::AsRawFd as _;
            PathBuf::from(format!("/proc/self/fd/{}", self.directory.as_raw_fd()))
        }
        #[cfg(not(target_os = "linux"))]
        {
            self.path.clone()
        }
    }

    pub(crate) fn verify(&self) -> io::Result<()> {
        if !self.directory.metadata()?.is_dir() {
            return Err(io::Error::other("writer parent is not a directory"));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt as _;
            let held = self.directory.metadata()?;
            let current = std::fs::metadata(&self.path)?;
            if held.dev() != current.dev() || held.ino() != current.ino() {
                return Err(io::Error::other("writer directory changed"));
            }
        }
        Ok(())
    }
}

/// Apply the journal file profile. Unix locks the file; Windows recorders
/// deny deletion through their open sharing flags and retain project authority.
/// # Errors
/// Refuses a live recorder or native lock failure.
pub fn lock_journal(file: &File) -> io::Result<()> {
    #[cfg(unix)]
    {
        fs2::FileExt::try_lock_exclusive(file)
    }
    #[cfg(not(unix))]
    {
        file.metadata().map(|_| ())
    }
}
