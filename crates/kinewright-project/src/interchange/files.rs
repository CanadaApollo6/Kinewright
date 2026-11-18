use std::{
    fs::{self, File, Metadata, ReadDir},
    io,
    path::{Path, PathBuf},
    sync::Arc,
};

use kinewright_core::{Analysis, MediaAsset, MediaError, ProbeObserver};
use kinewright_interchange::locator::LocatorFs;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum EntryKind {
    File,
    Directory,
    Other,
}

pub(super) struct DirectoryEntry {
    pub path: PathBuf,
    pub kind: EntryKind,
}

pub(super) struct NativeLocatorFs {
    pub analysis: Option<Arc<dyn Analysis>>,
}

impl LocatorFs for NativeLocatorFs {
    type Metadata = EntryKind;
    type Probe = MediaAsset;
    type Reader = File;
    type Entry = DirectoryEntry;
    type Directory = NativeEntries;

    fn stat(&self, path: &Path) -> io::Result<EntryKind> {
        fs::metadata(path).map(|metadata| entry_kind(&metadata))
    }

    fn open(&self, path: &Path) -> io::Result<File> {
        if !fs::metadata(path)?.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "expected a regular file",
            ));
        }
        File::open(path)
    }

    fn probe(&self, path: &Path, observer: &dyn ProbeObserver) -> Result<MediaAsset, MediaError> {
        self.analysis
            .as_ref()
            .ok_or(MediaError::NotImplemented)?
            .probe_observed(path, observer)
    }

    fn read_dir(&self, path: &Path) -> io::Result<NativeEntries> {
        if entry_kind(&fs::symlink_metadata(path)?) != EntryKind::Directory {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "expected a directory without a link",
            ));
        }
        fs::read_dir(path).map(NativeEntries)
    }
}

pub(super) struct NativeEntries(ReadDir);

impl Iterator for NativeEntries {
    type Item = io::Result<DirectoryEntry>;

    fn next(&mut self) -> Option<Self::Item> {
        self.0.next().map(|result| {
            let entry = result?;
            let kind = entry_kind(&entry.metadata()?);
            Ok(DirectoryEntry {
                path: entry.path(),
                kind,
            })
        })
    }
}

fn entry_kind(metadata: &Metadata) -> EntryKind {
    // A junction and other reparse directories are not followed by a roots walk.
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt as _;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return EntryKind::Other;
        }
    }
    if metadata.is_file() {
        EntryKind::File
    } else if metadata.is_dir() {
        EntryKind::Directory
    } else {
        EntryKind::Other
    }
}
