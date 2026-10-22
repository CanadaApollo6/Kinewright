use std::{
    fs::{self, File, Metadata},
    path::Path,
};

use kinewright_core::{MediaError, MediaSourceFingerprint};

#[cfg(test)]
pub(crate) use kinewright_hash::Sha256;
pub use kinewright_hash::{sha256_bytes, sha256_reader};

fn open_regular_file(path: &Path) -> Result<(File, Metadata), MediaError> {
    let path_metadata = fs::metadata(path).map_err(|error| {
        MediaError::Backend(format!(
            "could not inspect {} before hashing: {error}",
            path.display()
        ))
    })?;
    if !path_metadata.is_file() {
        return Err(MediaError::Backend(format!(
            "cannot hash {}: path is not a regular file",
            path.display()
        )));
    }
    let file = File::open(path).map_err(|error| {
        MediaError::Backend(format!("could not hash {}: {error}", path.display()))
    })?;
    let file_metadata = file.metadata().map_err(|error| {
        MediaError::Backend(format!(
            "could not inspect {} after opening: {error}",
            path.display()
        ))
    })?;
    if !file_metadata.is_file() {
        return Err(MediaError::Backend(format!(
            "cannot hash {}: opened path is not a regular file",
            path.display()
        )));
    }
    Ok((file, file_metadata))
}

fn metadata_changed(before: &Metadata, after: &Metadata) -> bool {
    before.len() != after.len() || before.modified().ok() != after.modified().ok()
}

fn hash_reader(file: &mut File, path: &Path) -> Result<String, MediaError> {
    sha256_reader(file)
        .map_err(|error| MediaError::Backend(format!("could not hash {}: {error}", path.display())))
}
/// Stream a regular file through the repository's deterministic SHA-256 implementation.
///
/// # Errors
///
/// Returns a media error when the path is not a regular file, cannot be opened,
/// or cannot be read completely.
pub fn sha256_file(path: &Path) -> Result<String, MediaError> {
    let (mut file, metadata) = open_regular_file(path)?;
    let digest = hash_reader(&mut file, path)?;
    let after = file.metadata().map_err(|error| {
        MediaError::Backend(format!(
            "could not inspect {} after hashing: {error}",
            path.display()
        ))
    })?;
    if metadata_changed(&metadata, &after) {
        return Err(MediaError::Backend(format!(
            "media {} changed while hashing",
            path.display()
        )));
    }
    Ok(digest)
}

/// Compute the persisted source identity used by relink and derived-data
/// verification. The byte length and digest are obtained from one regular-file
/// handle, and hashing fails if that handle's size or modification time changes.
///
/// # Errors
///
/// Returns a media error when the source metadata or bytes cannot be read.
pub fn source_fingerprint(path: &Path) -> Result<MediaSourceFingerprint, MediaError> {
    let (mut file, metadata) = open_regular_file(path)?;
    let byte_len = metadata.len();
    let content_sha256 = hash_reader(&mut file, path)?;
    let after = file.metadata().map_err(|error| {
        MediaError::Backend(format!(
            "could not inspect {} after fingerprinting: {error}",
            path.display()
        ))
    })?;
    if metadata_changed(&metadata, &after) {
        return Err(MediaError::Backend(format!(
            "media {} changed while fingerprinting",
            path.display()
        )));
    }
    Ok(MediaSourceFingerprint {
        content_sha256: Some(content_sha256),
        byte_len: Some(byte_len),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt::Write as _;

    fn encode(bytes: [u8; 32]) -> String {
        let mut encoded = String::with_capacity(64);
        for byte in bytes {
            let _ = write!(encoded, "{byte:02x}");
        }
        encoded
    }

    #[test]
    fn matches_standard_sha256_vectors_and_streaming_boundaries() {
        let mut empty = Sha256::new();
        empty.update(b"");
        assert_eq!(
            encode(empty.finalize()),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );

        let mut abc = Sha256::new();
        for byte in b"abc" {
            abc.update(&[*byte]);
        }
        assert_eq!(
            encode(abc.finalize()),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );

        let mut long = Sha256::new();
        long.update(&vec![b'a'; 1_000_000]);
        assert_eq!(
            encode(long.finalize()),
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
        );
    }

    #[test]
    fn source_fingerprint_contains_hash_and_byte_length() {
        let directory = crate::test_support::TempDirectory::new("source-fingerprint");
        let path = directory.path("fixture.bin");
        std::fs::write(&path, b"fingerprinted").unwrap();
        let fingerprint = source_fingerprint(&path).unwrap();
        let hash = sha256_file(&path).unwrap();
        assert_eq!(fingerprint.byte_len, Some(13));
        assert_eq!(fingerprint.content_sha256.as_deref(), Some(hash.as_str()));
    }

    #[test]
    fn source_fingerprint_rejects_non_regular_paths() {
        let directory = crate::test_support::TempDirectory::new("source-fingerprint-directory");
        assert!(source_fingerprint(directory.root()).is_err());
        assert!(sha256_file(directory.root()).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn source_fingerprint_rejects_fifo_without_opening_or_blocking() {
        use std::os::unix::fs::FileTypeExt as _;
        use std::process::Command;

        let directory = crate::test_support::TempDirectory::new("source-fingerprint-fifo");
        let fifo = directory.path("fixture.pipe");
        let status = Command::new("mkfifo").arg(&fifo).status().unwrap();
        assert!(status.success());
        assert!(
            std::fs::symlink_metadata(&fifo)
                .unwrap()
                .file_type()
                .is_fifo()
        );
        assert!(source_fingerprint(&fifo).is_err());
    }
}
