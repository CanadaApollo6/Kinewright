//! Native object identity shared by namespace, legacy locks and session checks.
use std::{fs::File, io, path::Path};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FileIdentity {
    pub(crate) device: u64,
    pub(crate) inode: u64,
}

impl FileIdentity {
    #[cfg(unix)]
    pub(crate) fn of_metadata(metadata: &std::fs::Metadata) -> Self {
        use std::os::unix::fs::MetadataExt as _;
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
        }
    }

    pub(crate) fn of_file(file: &File) -> io::Result<Self> {
        #[cfg(unix)]
        {
            Ok(Self::of_metadata(&file.metadata()?))
        }
        #[cfg(windows)]
        {
            let information = winapi_util::file::information(file)?;
            Ok(Self {
                device: information.volume_serial_number(),
                inode: information.file_index(),
            })
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = file;
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "native file identity unavailable",
            ))
        }
    }
}

fn open(path: &Path) -> io::Result<File> {
    let mut options = File::options();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        // A special-file swap must not turn a metadata probe into a blocking open.
        options.custom_flags(libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt as _;
        // Also admit directories. Identity checks do not pin project roots
        // against replacement; the writer separately pins its directories.
        options.custom_flags(0x0200_0000);
    }
    options.open(path)
}

pub(crate) fn file_matches_path(file: &File, path: &Path) -> io::Result<bool> {
    let current = open(path)?;
    Ok(FileIdentity::of_file(file)? == FileIdentity::of_file(&current)?)
}

pub(crate) fn paths_match(first: &Path, second: &Path) -> io::Result<bool> {
    let first = open(first)?;
    file_matches_path(&first, second)
}
