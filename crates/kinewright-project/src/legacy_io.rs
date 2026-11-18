//! Directory handles used by legacy writers. Paths passed to IO stay below
//! the admitted directory even if a caller's alias changes during a write.
use std::{
    fs::{self, File},
    io,
    path::{Path, PathBuf},
    sync::Arc,
};

use crate::namespace::FileIdentity;

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
        if !crate::namespace::file_matches_path(&self.directory, &self.path)? {
            return Err(io::Error::other("writer directory changed"));
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

/// Identity of the recorder's journal. On Unix the duplicate keeps its inode
/// alive after the recorder unlocks, so a replacement cannot recycle the ID.
pub struct JournalIdentity {
    identity: FileIdentity,
    #[cfg(unix)]
    _original: File,
}

/// A journal under its lock. Unlock before closing the descriptor: a child
/// forked on another thread can hold a copy of its open file description
/// until exec. Closing one descriptor alone would leave the journal locked.
pub struct LockedJournal(File);

impl LockedJournal {
    /// Take the journal's non-blocking lock.
    ///
    /// # Errors
    /// Returns the native lock or metadata error.
    pub fn lock(file: File) -> io::Result<Self> {
        lock_journal(&file)?;
        Ok(Self(file))
    }

    /// Open and lock a regular journal for removal, without following a Unix link
    /// or blocking on a special file substituted before open.
    ///
    /// # Errors
    /// Returns open, file type, or lock errors. A missing file returns `NotFound`.
    pub fn open_for_removal(path: &Path) -> io::Result<Self> {
        fn require_regular(metadata: &fs::Metadata) -> io::Result<()> {
            if metadata.is_file() {
                Ok(())
            } else {
                Err(io::Error::other("journal is not a regular file"))
            }
        }

        require_regular(&fs::symlink_metadata(path)?)?;
        let mut options = File::options();
        options.read(true).write(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::custom_flags(
            &mut options,
            libc::O_NOFOLLOW | libc::O_NONBLOCK,
        );
        let file = options.open(path)?;
        require_regular(&file.metadata()?)?;
        Self::lock(file)
    }

    /// Capture the recorder's file identity before it releases the journal.
    ///
    /// # Errors
    /// Returns an identity or descriptor duplication error.
    pub fn identity(&self) -> io::Result<JournalIdentity> {
        Ok(JournalIdentity {
            identity: FileIdentity::of_file(&self.0)?,
            #[cfg(unix)]
            _original: self.0.try_clone()?,
        })
    }

    /// Check that the opened journal is the recorder's original file.
    ///
    /// # Errors
    /// Returns a native identity lookup error.
    pub fn matches_identity(&self, original: &JournalIdentity) -> io::Result<bool> {
        Ok(FileIdentity::of_file(&self.0)? == original.identity)
    }

    /// Unlink while holding the journal lock, after checking the path still
    /// names this file. Returns `false` if the name now identifies another file.
    ///
    /// # Errors
    /// Returns an identity lookup or unlink error.
    pub fn unlink(self, path: &Path) -> io::Result<bool> {
        let current = fs::symlink_metadata(path)?;
        if !current.is_file() {
            return Ok(false);
        }
        #[cfg(unix)]
        let matches = FileIdentity::of_file(&self.0)? == FileIdentity::of_metadata(&current);
        #[cfg(not(unix))]
        let matches = crate::namespace::file_matches_path(&self.0, path)?;
        if !matches {
            return Ok(false);
        }
        #[cfg(any(test, feature = "test-util"))]
        crate::test_hook("journal_before_unlink");
        fs::remove_file(path)?;
        Ok(true)
    }
}

impl std::ops::Deref for LockedJournal {
    type Target = File;

    fn deref(&self) -> &File {
        &self.0
    }
}

impl std::ops::DerefMut for LockedJournal {
    fn deref_mut(&mut self) -> &mut File {
        &mut self.0
    }
}

impl Drop for LockedJournal {
    fn drop(&mut self) {
        // A live Windows recorder denies deletion through its share mode.
        #[cfg(unix)]
        let _ = self.0.unlock();
    }
}

/// Whether another handle blocked locking or deleting a journal.
#[must_use]
pub fn journal_removal_contended(error: &io::Error) -> bool {
    #[cfg(windows)]
    if error.raw_os_error() == Some(32) {
        // ERROR_SHARING_VIOLATION
        return true;
    }
    error.kind() == io::ErrorKind::WouldBlock
}
