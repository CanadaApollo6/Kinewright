//! Pinned model data and verified installation. A receipt binds a full hash to
//! file identity, size, modification time and native change time, never a path alone.
use super::Error;
use kinewright_core::ExportCancellation;
use kinewright_hash::Sha256;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::Mutex,
};

#[derive(Debug)]
pub struct ModelFile {
    pub name: &'static str,
    pub url: &'static str,
    pub bytes: u64,
    pub sha256: &'static str,
}
#[derive(Debug)]
pub struct ModelSpec {
    pub id: &'static str,
    pub name: &'static str,
    pub licence: &'static str,
    pub licence_url: &'static str,
    pub notice: &'static str,
    pub files: &'static [ModelFile],
    pub streaming: bool,
}

pub const ENGLISH_MODEL: ModelSpec = ModelSpec {
    id: "nemotron-speech-streaming-en-0.6b",
    name: "NVIDIA Nemotron Speech Streaming EN 0.6B",
    licence: "NVIDIA Open Model License",
    licence_url: "https://www.nvidia.com/en-us/agreements/enterprise-software/nvidia-open-model-license/",
    notice: "Licensed by NVIDIA Corporation under the NVIDIA Open Model License",
    streaming: true,
    files: &[ModelFile {
        name: "nemotron-speech-streaming-en-0.6b.q8_0.gguf",
        url: "https://huggingface.co/nvidia/nemotron-speech-streaming-en-0.6b/resolve/ebe59e5a817142986528bbbee5dba8db7b38ed50/nemotron-speech-streaming-en-0.6b.q8_0.gguf",
        bytes: 699_872_960,
        sha256: "d9a01898d2a611c8764e23a1c2f45e70bbd5a425dc4de93692ac951dd603812d",
    }],
};
pub const PARAKEET_MODEL: ModelSpec = ModelSpec {
    id: "parakeet-tdt-0.6b-v3",
    name: "NVIDIA Parakeet TDT 0.6B v3",
    licence: "CC-BY-4.0",
    licence_url: "https://creativecommons.org/licenses/by/4.0/",
    notice: "Parakeet TDT 0.6B v3 by NVIDIA, licensed under CC BY 4.0.",
    streaming: false,
    files: &[ModelFile {
        name: "parakeet-tdt-0.6b-v3.q8_0.gguf",
        url: "https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3/resolve/541d1f99c6b0c3cd0b11a95167540bb8edefd82b/parakeet-tdt-0.6b-v3.q8_0.gguf",
        bytes: 713_975_456,
        sha256: "e3880d0aaaaf2c308ea2c35016b2b895c423eb3fda924c1b463d1c19b7f4d32e",
    }],
};

#[derive(Debug)]
pub enum ModelError {
    Missing,
    WrongLength {
        expected: u64,
        actual: u64,
    },
    WrongHash {
        expected: &'static str,
        actual: String,
    },
    Changed,
    Io(io::Error),
    Download(String),
    InvalidSpec,
    UnsafePath(PathBuf),
}
impl std::fmt::Display for ModelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing => f.write_str("recognition model is missing"),
            Self::WrongLength { expected, actual } => write!(
                f,
                "model length mismatch: expected {expected}, got {actual}"
            ),
            Self::WrongHash { expected, actual } => write!(
                f,
                "model SHA-256 mismatch: expected {expected}, got {actual}"
            ),
            Self::Changed => f.write_str("recognition model changed after verification"),
            Self::Io(error) => write!(f, "model file: {error}"),
            Self::Download(error) => write!(f, "model download: {error}"),
            Self::InvalidSpec => f.write_str("invalid recognition model specification"),
            Self::UnsafePath(path) => {
                write!(f, "refusing linked recognition path {}", path.display())
            }
        }
    }
}
impl std::error::Error for ModelError {}
impl From<io::Error> for Error {
    fn from(error: io::Error) -> Self {
        Self::Model(ModelError::Io(error))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileStamp {
    length: u64,
    identity: [u64; 2],
    modified: [i64; 2],
    changed: [i64; 2],
}
impl FileStamp {
    fn read(file: &File) -> Result<Self, Error> {
        let metadata = file.metadata()?;
        if !metadata.is_file() {
            return Err(Error::Model(ModelError::InvalidSpec));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt as _;
            Ok(Self {
                length: metadata.len(),
                identity: [metadata.dev(), metadata.ino()],
                modified: [metadata.mtime(), metadata.mtime_nsec()],
                changed: [metadata.ctime(), metadata.ctime_nsec()],
            })
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt as _;
            let info = winapi_util::file::information(file)?;
            let modified = i64::try_from(metadata.last_write_time())
                .map_err(|_| Error::Model(ModelError::Changed))?;
            Ok(Self {
                length: metadata.len(),
                identity: [info.volume_serial_number(), info.file_index()],
                modified: [modified, 0],
                changed: [crate::nemo::file_change_time(file)?, 0],
            })
        }
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    sha256: String,
    stamp: FileStamp,
}

/// Retains the verified inode and, on Windows, a write/delete exclusion handle.
pub struct VerifiedFile {
    root: PathBuf,
    path: PathBuf,
    file: File,
    stamp: FileStamp,
}
impl VerifiedFile {
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn check(&self) -> Result<(), Error> {
        if FileStamp::read(&self.file)? != self.stamp
            || FileStamp::read(&open(&self.root, &self.path)?)? != self.stamp
        {
            return Err(Error::Model(ModelError::Changed));
        }
        Ok(())
    }
}
fn open(root: &Path, path: &Path) -> Result<File, Error> {
    refuse_links(root, path)?;
    #[cfg(unix)]
    let result = rustix::fs::open(
        path,
        rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
    )
    .map(File::from)
    .map_err(io::Error::from);
    #[cfg(windows)]
    let result = {
        use std::os::windows::fs::OpenOptionsExt as _;
        fs::OpenOptions::new()
            .read(true)
            .custom_flags(0x0020_0000) // FILE_FLAG_OPEN_REPARSE_POINT
            .share_mode(1) // FILE_SHARE_READ: retain write/delete exclusion for mapped weights.
            .open(path)
    };
    result.map_err(|e| {
        if e.kind() == io::ErrorKind::NotFound {
            Error::Model(ModelError::Missing)
        } else {
            e.into()
        }
    })
}
fn receipt_path(path: &Path, receipt_dir: &Path) -> PathBuf {
    receipt_dir.join(format!(
        "{}.json",
        crate::sha256::sha256_bytes(path.as_os_str().as_encoded_bytes())
    ))
}
// Resolve only the parent of an explicitly chosen model/receipt directory.
// Its own entry remains visible to the no-follow checks below.
fn directory_path(root: &Path) -> Result<PathBuf, Error> {
    let absolute = std::path::absolute(root)?;
    let parent = absolute
        .parent()
        .ok_or(Error::Model(ModelError::InvalidSpec))?;
    let parent = fs::canonicalize(parent).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            Error::Model(ModelError::Missing)
        } else {
            error.into()
        }
    })?;
    Ok(parent.join(
        absolute
            .file_name()
            .ok_or(Error::Model(ModelError::InvalidSpec))?,
    ))
}
fn refuse_links(root: &Path, path: &Path) -> Result<(), Error> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| Error::Model(ModelError::InvalidSpec))?;
    let mut current = root.to_path_buf();
    let mut parts = relative.components().peekable();
    loop {
        let directory = parts.peek().is_some() || current == root;
        match fs::symlink_metadata(&current) {
            Ok(metadata) => {
                #[cfg(windows)]
                let linked = {
                    use std::os::windows::fs::MetadataExt as _;
                    metadata.file_attributes() & 0x400 != 0
                };
                #[cfg(not(windows))]
                let linked = metadata.file_type().is_symlink();
                if linked || (directory && !metadata.is_dir()) {
                    return Err(Error::Model(ModelError::UnsafePath(current)));
                }
                #[cfg(unix)]
                if directory {
                    use std::os::unix::fs::MetadataExt as _;
                    if metadata.uid() != rustix::process::geteuid().as_raw() {
                        return Err(Error::Model(ModelError::UnsafePath(current)));
                    }
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        let Some(std::path::Component::Normal(name)) = parts.next() else {
            return if parts.peek().is_none() && current == path {
                Ok(())
            } else {
                Err(Error::Model(ModelError::InvalidSpec))
            };
        };
        current.push(name);
    }
}
fn temporary(path: &Path) -> Result<PathBuf, Error> {
    use std::fmt::Write as _;
    let mut random = [0_u8; 16];
    getrandom::fill(&mut random).map_err(|e| io::Error::other(e.to_string()))?;
    let mut suffix = String::from("part-");
    for byte in random {
        let _ = write!(&mut suffix, "{byte:02x}");
    }
    Ok(path.with_extension(suffix))
}
fn new_partial(root: &Path, path: &Path) -> Result<(PartialFile, File), Error> {
    new_partial_with(|| {
        let path = temporary(path)?;
        refuse_links(root, &path)?;
        Ok(path)
    })
}
fn new_partial_with(
    mut candidate: impl FnMut() -> Result<PathBuf, Error>,
) -> Result<(PartialFile, File), Error> {
    loop {
        let path = candidate()?;
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        match options.open(&path) {
            Ok(file) => return Ok((PartialFile(path), file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
    }
}
struct PartialFile(PathBuf);
impl Drop for PartialFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

pub fn verify(
    spec: &'static ModelFile,
    path: &Path,
    receipts: &Path,
    cancel: &ExportCancellation,
) -> Result<VerifiedFile, Error> {
    let original_root = path.parent().ok_or(Error::Model(ModelError::InvalidSpec))?;
    let root = directory_path(original_root)?;
    let path = root.join(
        path.file_name()
            .ok_or(Error::Model(ModelError::InvalidSpec))?,
    );
    let receipts = if let Ok(relative) = receipts.strip_prefix(original_root) {
        root.join(relative)
    } else {
        directory_path(receipts)?
    };
    verify_inner(&root, spec, &path, &receipts, cancel, true)
}
/// Application-owned paths retain the data-root boundary through every recheck.
pub(crate) fn verify_at(
    root: &Path,
    spec: &'static ModelFile,
    path: &Path,
    receipts: &Path,
    cancel: &ExportCancellation,
) -> Result<VerifiedFile, Error> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| Error::Model(ModelError::InvalidSpec))?;
    let receipt_relative = receipts
        .strip_prefix(root)
        .map_err(|_| Error::Model(ModelError::InvalidSpec))?;
    let resolved = crate::data_dir::ensure_root(root)?;
    let parent = relative
        .parent()
        .ok_or(Error::Model(ModelError::InvalidSpec))?;
    let path = resolved.join(relative);
    refuse_links(&resolved, &path)?;
    // Reading a missing voice model must not start an installation.
    if resolved.join(parent).try_exists()? {
        crate::data_dir::ensure_directory(&resolved, parent)?;
    }
    verify_inner(
        &resolved,
        spec,
        &path,
        &resolved.join(receipt_relative),
        cancel,
        true,
    )
}
fn verify_inner(
    root: &Path,
    spec: &'static ModelFile,
    path: &Path,
    receipts: &Path,
    cancel: &ExportCancellation,
    save_receipt: bool,
) -> Result<VerifiedFile, Error> {
    super::cancelled(cancel)?;
    let mut file = open(root, path)?;
    let stamp = FileStamp::read(&file)?;
    if stamp.length != spec.bytes {
        return Err(Error::Model(ModelError::WrongLength {
            expected: spec.bytes,
            actual: stamp.length,
        }));
    }
    let receipt_root = if receipts.starts_with(root) {
        root.to_path_buf()
    } else {
        directory_path(receipts)?
    };
    let receipt_path = receipt_path(path, receipts);
    refuse_links(&receipt_root, &receipt_path)?;
    let receipt = open(&receipt_root, &receipt_path)
        .and_then(|mut file| {
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes)?;
            Ok(bytes)
        })
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Receipt>(&bytes).ok());
    if !receipt.is_some_and(|r| r.sha256 == spec.sha256 && r.stamp == stamp) {
        let actual = hash(&mut file, cancel)?;
        if actual != spec.sha256 {
            return Err(Error::Model(ModelError::WrongHash {
                expected: spec.sha256,
                actual,
            }));
        }
        if FileStamp::read(&file)? != stamp || FileStamp::read(&open(root, path)?)? != stamp {
            return Err(Error::Model(ModelError::Changed));
        }
        if save_receipt {
            crate::data_dir::ensure_directory(
                &receipt_root,
                receipts
                    .strip_prefix(&receipt_root)
                    .map_err(|_| Error::Model(ModelError::InvalidSpec))?,
            )?;
            refuse_links(&receipt_root, &receipt_path)?;
            let (part, mut output) = new_partial(&receipt_root, &receipt_path)?;
            serde_json::to_writer(
                &mut output,
                &Receipt {
                    sha256: actual,
                    stamp,
                },
            )
            .map_err(|error| Error::Model(ModelError::Download(error.to_string())))?;
            output.sync_all()?;
            drop(output);
            super::cancelled(cancel)?;
            refuse_links(&receipt_root, &receipt_path)?;
            fs::rename(&part.0, &receipt_path)?;
        }
    }
    let verified = VerifiedFile {
        root: root.to_path_buf(),
        path: path.to_path_buf(),
        file,
        stamp,
    };
    super::cancelled(cancel)?;
    verified.check()?;
    Ok(verified)
}
fn hash(file: &mut impl Read, cancel: &ExportCancellation) -> Result<String, Error> {
    use std::fmt::Write as _;
    let mut digest = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        super::cancelled(cancel)?;
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    let mut result = String::with_capacity(64);
    for byte in digest.finalize() {
        let _ = write!(result, "{byte:02x}");
    }
    Ok(result)
}
static INSTALL: Mutex<()> = Mutex::new(());
/// Serialises model installers, including explicit voice-model preparation.
pub fn provision(
    spec: &'static ModelSpec,
    data: &Path,
    cancel: &ExportCancellation,
    progress: impl FnMut(u64, Option<u64>),
) -> Result<PathBuf, Error> {
    provision_with(spec, data, cancel, progress, |file| {
        let response = reqwest::blocking::Client::new()
            .get(file.url)
            .send()
            .and_then(reqwest::blocking::Response::error_for_status)
            .map_err(|error| Error::Model(ModelError::Download(error.to_string())))?;
        Ok(Box::new(response) as Box<dyn Read>)
    })
}
fn provision_with(
    spec: &'static ModelSpec,
    data: &Path,
    cancel: &ExportCancellation,
    mut progress: impl FnMut(u64, Option<u64>),
    mut download: impl FnMut(&ModelFile) -> Result<Box<dyn Read>, Error>,
) -> Result<PathBuf, Error> {
    let _install = INSTALL
        .lock()
        .map_err(|_| Error::Model(ModelError::Download("installer lock poisoned".into())))?;
    super::cancelled(cancel)?;
    let normal_name = |value: &str| {
        let mut parts = Path::new(value).components();
        matches!(parts.next(), Some(std::path::Component::Normal(_))) && parts.next().is_none()
    };
    if spec.files.is_empty()
        || !normal_name(spec.id)
        || spec.files.iter().any(|file| !normal_name(file.name))
    {
        return Err(Error::Model(ModelError::InvalidSpec));
    }
    let data = crate::data_dir::ensure_root(data)?;
    let root = data.join("models").join(spec.id);
    let receipts = root.join("verified");
    crate::data_dir::ensure_directory(&data, Path::new("models").join(spec.id).as_path())?;
    for file in spec.files {
        let path = root.join(file.name);
        match verify_inner(&data, file, &path, &receipts, cancel, true) {
            Ok(_) => continue,
            Err(Error::Model(
                ModelError::Missing
                | ModelError::WrongLength { .. }
                | ModelError::WrongHash { .. }
                | ModelError::Changed,
            )) => {}
            Err(error) => return Err(error),
        }
        let (part, mut output) = new_partial(&data, &path)?;
        let mut input = download(file)?;
        let mut buffer = vec![0_u8; 64 * 1024];
        let mut written = 0;
        loop {
            super::cancelled(cancel)?;
            let count = input.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            output.write_all(&buffer[..count])?;
            written += u64::try_from(count).map_err(|_| Error::Model(ModelError::InvalidSpec))?;
            progress(written, Some(file.bytes));
            if written > file.bytes {
                return Err(Error::Model(ModelError::WrongLength {
                    expected: file.bytes,
                    actual: written,
                }));
            }
        }
        output.sync_all()?;
        drop(output);
        drop(verify_inner(
            &data, file, &part.0, &receipts, cancel, false,
        )?);
        super::cancelled(cancel)?;
        refuse_links(&data, &path)?;
        fs::rename(&part.0, &path)?;
        drop(verify_inner(&data, file, &path, &receipts, cancel, true)?);
    }
    Ok(root)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDirectory;
    use std::io::Cursor;
    const FILE: ModelFile = ModelFile {
        name: "synthetic.gguf",
        url: "https://huggingface.co/unused",
        bytes: 3,
        sha256: "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
    };
    const SPEC: ModelSpec = ModelSpec {
        id: "synthetic",
        name: "test",
        licence: "test",
        licence_url: "",
        notice: "",
        streaming: true,
        files: &[FILE],
    };
    fn input(bytes: &'static [u8]) -> Box<dyn Read> {
        Box::new(Cursor::new(bytes))
    }
    fn path(data: &Path) -> PathBuf {
        data.join("models/synthetic/synthetic.gguf")
    }
    #[test]
    fn dq1_provision_wrong_hash_refused_and_partial_removed() {
        let data = TempDirectory::new("dq1-hash");
        let error = provision_with(
            &SPEC,
            data.root(),
            &ExportCancellation::default(),
            |_, _| {},
            |_| Ok(input(b"abd")),
        )
        .unwrap_err();
        assert!(matches!(error, Error::Model(ModelError::WrongHash { .. })));
        assert!(!path(data.root()).exists());
        assert!(
            fs::read_dir(data.root().join("models/synthetic"))
                .unwrap()
                .next()
                .is_none()
        );
    }
    #[test]
    fn dq1_partial_replaced_and_verified_model_reused() {
        let data = TempDirectory::new("dq1-partial");
        let path = path(data.root());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"a").unwrap();
        let cancel = ExportCancellation::default();
        provision_with(
            &SPEC,
            data.root(),
            &cancel,
            |_, _| {},
            |_| Ok(input(b"abc")),
        )
        .unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"abc");
        provision_with(
            &SPEC,
            data.root(),
            &cancel,
            |_, _| {},
            |_| panic!("verified model must be reused"),
        )
        .unwrap();
        assert_eq!(
            fs::read_dir(path.parent().unwrap().join("verified"))
                .unwrap()
                .count(),
            1,
            "no temporary-path receipts"
        );
    }
    struct CancellingRead {
        cancel: ExportCancellation,
        sent: bool,
    }
    impl Read for CancellingRead {
        fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
            assert!(!self.sent, "cancel checked before reading another chunk");
            bytes[0] = b'a';
            self.sent = true;
            self.cancel.cancel();
            Ok(1)
        }
    }
    #[test]
    fn dq1_cancel_mid_download_cleans_partial() {
        let data = TempDirectory::new("dq1-cancel-download");
        let cancel = ExportCancellation::default();
        let result = provision_with(
            &SPEC,
            data.root(),
            &cancel,
            |_, _| {},
            |_| {
                Ok(Box::new(CancellingRead {
                    cancel: cancel.clone(),
                    sent: false,
                }))
            },
        );
        assert!(matches!(result, Err(Error::Cancelled)));
        assert!(!path(data.root()).exists());
        assert!(
            fs::read_dir(data.root().join("models/synthetic"))
                .unwrap()
                .next()
                .is_none()
        );
    }
    #[test]
    fn dq1_altered_existing_file_rehashes_even_with_restored_mtime() {
        let data = TempDirectory::new("dq1-altered");
        let path = path(data.root());
        let cancel = ExportCancellation::default();
        provision_with(
            &SPEC,
            data.root(),
            &cancel,
            |_, _| {},
            |_| Ok(input(b"abc")),
        )
        .unwrap();
        let original = fs::metadata(&path).unwrap().modified().unwrap();
        fs::write(&path, b"abd").unwrap();
        fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(original))
            .unwrap();
        let result = verify(
            &FILE,
            &path,
            &path.parent().unwrap().join("verified"),
            &cancel,
        );
        assert!(
            matches!(result, Err(Error::Model(ModelError::WrongHash { .. }))),
            "DQ1_ALTERED_FILE_MUST_HASH"
        );
        provision_with(
            &SPEC,
            data.root(),
            &cancel,
            |_, _| {},
            |_| Ok(input(b"abc")),
        )
        .unwrap();
        assert_eq!(fs::read(path).unwrap(), b"abc");
    }
    #[test]
    fn dq1_model_spec_cannot_escape_the_data_root() {
        const BAD: ModelSpec = ModelSpec {
            files: &[ModelFile {
                name: "../outside",
                ..FILE
            }],
            ..SPEC
        };
        let data = TempDirectory::new("dq1-bad-spec");
        let result = provision_with(
            &BAD,
            data.root(),
            &ExportCancellation::default(),
            |_, _| {},
            |_| panic!("invalid spec cannot download"),
        );
        assert!(matches!(result, Err(Error::Model(ModelError::InvalidSpec))));
        assert_eq!(fs::read_dir(data.root()).unwrap().count(), 0);
    }
    #[cfg(unix)]
    #[test]
    fn dq1_exclusive_partial_and_receipt_collisions_preserve_outside_files() {
        use std::os::unix::fs::symlink;
        for role in ["download", "receipt"] {
            let data = TempDirectory::new("dq1-linked-temporary");
            let outside = TempDirectory::new("dq1-outside");
            let target = outside.path("irreplaceable");
            fs::write(&target, b"irreplaceable work").unwrap();
            let planted = data.path(&format!("{role}.part-planted"));
            let fresh = data.path(&format!("{role}.part-fresh"));
            symlink(&target, &planted).unwrap();
            let mut names = [planted.as_path(), fresh.as_path()].into_iter();
            let (guard, mut file) =
                new_partial_with(|| Ok(names.next().expect("collision retry").to_path_buf()))
                    .unwrap();
            file.write_all(b"abc").unwrap();
            drop(file);
            assert_eq!(
                fs::read(&target).unwrap(),
                b"irreplaceable work",
                "DQ1_TEMPORARY_NO_FOLLOW {role}"
            );
            assert_eq!(fs::read(&fresh).unwrap(), b"abc");
            assert!(
                fs::symlink_metadata(&planted)
                    .unwrap()
                    .file_type()
                    .is_symlink()
            );
            drop(guard);
            assert!(!fresh.exists());
            assert!(
                fs::symlink_metadata(&planted).is_ok(),
                "cleanup owns only the successfully created entry"
            );
        }
    }
    #[cfg(unix)]
    #[test]
    fn dq1_final_model_and_receipt_links_are_refused() {
        use std::os::unix::fs::symlink;
        for receipt in [false, true] {
            let data = TempDirectory::new("dq1-final-link");
            let outside = TempDirectory::new("dq1-final-outside");
            let target = outside.path("work");
            fs::write(&target, b"abc").unwrap();
            let model = path(data.root());
            let receipts = model.parent().unwrap().join("verified");
            fs::create_dir_all(&receipts).unwrap();
            if receipt {
                fs::write(&model, b"abc").unwrap();
                symlink(&target, receipt_path(&model, &receipts)).unwrap();
            } else {
                symlink(&target, &model).unwrap();
            }
            let result = provision_with(
                &SPEC,
                data.root(),
                &ExportCancellation::default(),
                |_, _| {},
                |_| panic!("linked installation must not download"),
            );
            assert!(
                matches!(result, Err(Error::Model(ModelError::UnsafePath(_)))),
                "{result:?}"
            );
            assert_eq!(fs::read(&target).unwrap(), b"abc");
        }
    }
    #[cfg(unix)]
    #[test]
    fn dq1_model_directories_never_traverse_planted_links() {
        use std::os::unix::fs::symlink;
        for component in ["models", "models/synthetic", "models/synthetic/verified"] {
            let data = TempDirectory::new("dq1-directory-link");
            let outside = TempDirectory::new("dq1-directory-outside");
            let link = data.path(component);
            fs::create_dir_all(link.parent().unwrap()).unwrap();
            symlink(outside.root(), &link).unwrap();
            let result = provision_with(
                &SPEC,
                data.root(),
                &ExportCancellation::default(),
                |_, _| {},
                |_| Ok(input(b"abc")),
            );
            assert!(result.is_err(), "linked component {component}");
            assert_eq!(
                fs::read_dir(outside.root()).unwrap().count(),
                0,
                "no writes through {component}"
            );
        }
    }

    #[test]
    fn dq1_model_provisioning_accepts_a_link_above_the_data_root() {
        let temp = TempDirectory::new("dq1-install-layout");
        let real = temp.path("real");
        fs::create_dir_all(real.join("share")).unwrap();
        crate::data_dir::directory_link(&real, &temp.path("homes"));
        let data = temp.path("homes/share/kinewright");
        let root = provision_with(
            &SPEC,
            &data,
            &ExportCancellation::default(),
            |_, _| {},
            |_| Ok(input(b"abc")),
        )
        .unwrap();
        assert_eq!(
            root,
            fs::canonicalize(real.join("share/kinewright/models/synthetic")).unwrap()
        );
        assert_eq!(fs::read(root.join(FILE.name)).unwrap(), b"abc");
        provision_with(
            &SPEC,
            &data,
            &ExportCancellation::default(),
            |_, _| {},
            |_| panic!("reuse the resolved installation"),
        )
        .unwrap();
    }
    #[test]
    fn dq1_explicit_model_directory_resolves_only_its_parent() {
        let temp = TempDirectory::new("dq1-explicit-layout");
        let real = temp.path("real");
        fs::create_dir_all(real.join("share")).unwrap();
        crate::data_dir::directory_link(&real, &temp.path("homes"));
        let chosen = temp.path("homes/share/model");
        let root = crate::data_dir::ensure_root(&chosen).unwrap();
        fs::write(root.join(FILE.name), b"abc").unwrap();
        let receipts = temp.path("receipts");
        let cancel = ExportCancellation::default();
        let verified = verify(&FILE, &chosen.join(FILE.name), &receipts, &cancel).unwrap();
        assert_eq!(verified.path(), root.join(FILE.name));
        verified.check().unwrap();
        drop(verified);
        crate::data_dir::directory_link(&root, &temp.path("linked-model"));
        assert!(matches!(
            verify(
                &FILE,
                &temp.path("linked-model").join(FILE.name),
                &receipts,
                &cancel
            ),
            Err(Error::Model(ModelError::UnsafePath(_)))
        ));
        crate::data_dir::directory_link(&root, &root.join("planted"));
        assert!(matches!(
            verify(
                &FILE,
                &root.join("planted").join(FILE.name),
                &receipts,
                &cancel
            ),
            Err(Error::Model(ModelError::UnsafePath(_)))
        ));
    }
}
