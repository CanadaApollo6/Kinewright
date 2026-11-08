//! Both saved provider choices have the same bounded, no-follow entry contract.
use serde::de::DeserializeOwned;
use std::{io::Read, path::Path};

pub(super) const MAX_BYTES: u64 = 16 << 10;

pub(super) fn read<T: DeserializeOwned>(path: &Path) -> Result<Option<T>, ()> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(()),
    };
    if !regular(&metadata) || metadata.len() > MAX_BYTES {
        return Err(());
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(
            (rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK)
                .bits()
                .cast_signed(),
        );
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let file = options.open(path).map_err(|_| ())?;
    let opened = file.metadata().map_err(|_| ())?;
    if !regular(&opened) || opened.len() > MAX_BYTES {
        return Err(());
    }
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ())?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(());
    }
    serde_json::from_slice(&bytes).map(Some).map_err(|_| ())
}

fn regular(metadata: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return false;
        }
    }
    metadata.is_file() && !metadata.file_type().is_symlink()
}
