//! The project-relative LUT store, availability, restore, and the render-time
//! [`LutLibrary`] (CC4 §2.2, §2.3, §2.4).
//!
//! The store root is derived from the project path at runtime and never stored
//! in a document, a journal entry, or a recovery record. Store file names are
//! the 64-character validated content hash plus `.cube`, so no user-supplied
//! string ever reaches a path component and traversal through import is
//! structurally impossible rather than merely checked.

use std::{
    collections::{BTreeMap, HashMap},
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc, LazyLock, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use kinewright_core::{
    LutAsset, LutAssetId, LutAssetKind, LutAssetSource, LutAvailabilityKind, LutAvailabilityStatus,
    MediaError,
};

use crate::{
    builtin_looks::BuiltinLook,
    lut::{CubeLut, parse_cube_lut_bytes},
    sha256::{sha256_bytes, sha256_file},
};

/// Directory-name suffix appended to the project stem (CC4 §2.2).
pub const LUT_STORE_SUFFIX: &str = "kinewright-assets";
/// Sub-directory of the store root that holds `.cube` files.
pub const LUT_STORE_LUTS_DIRECTORY: &str = "luts";
/// Largest `.cube` file the store will read, hash, or write.
///
/// A 65³ vendor export is about 7.5 MB of text, so 16 MiB accepts every legal
/// LUT with headroom while keeping a mistaken pick (a video file, a disk image)
/// from being read into memory at all: the length is checked from the file
/// metadata before a single byte is read.
pub const LUT_MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;

/// The longest observed fragment a store error quotes back.
const OBSERVED_LIMIT: usize = 120;

/// Machine-readable store, availability, and recovery codes (CC4 §2.3, §10.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LutStoreErrorCode {
    /// The project path cannot yield a store root, or the root is not a
    /// directory this process will write through.
    LutStoreRootInvalid,
    /// A recorded hash is not 64 lowercase hexadecimal characters.
    InvalidLutAssetHash,
    /// The store file is absent or is not a regular file.
    MissingLutAsset,
    /// A store file exists but hashes to something else.
    ChangedLutAsset,
    /// The path exists but its bytes or metadata cannot be read.
    UnreadableLutAsset,
    /// A restore candidate does not carry the recorded bytes.
    LutRelinkHashMismatch,
    /// The record disagrees with the hash-verified bytes (CC4 §2.1).
    LutAssetMetadataMismatch,
    /// A `builtin` provenance name this binary has no bake for.
    UnknownBuiltinLook,
    /// The store could not be created or written.
    LutStoreWriteFailed,
    /// One Save As copy failed; the project still saves.
    LutStoreCopyFailed,
    /// The candidate file exceeds [`LUT_MAX_FILE_BYTES`].
    LutFileTooLarge,
}

impl LutStoreErrorCode {
    /// The stable `snake_case` token used in errors, manifests, and the UI.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::LutStoreRootInvalid => "lut_store_root_invalid",
            Self::InvalidLutAssetHash => "invalid_lut_asset_hash",
            Self::MissingLutAsset => "missing_lut_asset",
            Self::ChangedLutAsset => "changed_lut_asset",
            Self::UnreadableLutAsset => "unreadable_lut_asset",
            Self::LutRelinkHashMismatch => "lut_relink_hash_mismatch",
            Self::LutAssetMetadataMismatch => "lut_asset_metadata_mismatch",
            Self::UnknownBuiltinLook => "unknown_builtin_look",
            Self::LutStoreWriteFailed => "lut_store_write_failed",
            Self::LutStoreCopyFailed => "lut_store_copy_failed",
            Self::LutFileTooLarge => "lut_file_too_large",
        }
    }
}

/// One typed store failure.
///
/// CC4 maps LUT errors to `MediaError::Backend` with a stable
/// `<code>: …; observed=…; allowed=…` prefix (a recorded departure from §2.5's
/// `MediaError::Lut`); the typed [`LutStoreError`] remains public for callers
/// that need structure, and a recovery surface can read the fields back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LutStoreError {
    /// The stable failure code.
    pub code: LutStoreErrorCode,
    /// What went wrong, in one sentence.
    pub detail: String,
    /// What was observed, when the failure has an observation.
    pub observed: Option<String>,
    /// What would have been accepted.
    pub allowed: Option<String>,
}

impl LutStoreError {
    fn new(code: LutStoreErrorCode, detail: String) -> Self {
        Self {
            code,
            detail,
            observed: None,
            allowed: None,
        }
    }

    fn with_observed(mut self, observed: &str) -> Self {
        self.observed = Some(sanitize(observed));
        self
    }

    fn with_allowed(mut self, allowed: &str) -> Self {
        self.allowed = Some(sanitize(allowed));
        self
    }

    /// The stable failure code.
    #[must_use]
    pub const fn code(&self) -> LutStoreErrorCode {
        self.code
    }
}

impl std::fmt::Display for LutStoreError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.code.as_str(), self.detail)?;
        if let Some(observed) = &self.observed {
            write!(formatter, "; observed={observed}")?;
        }
        if let Some(allowed) = &self.allowed {
            write!(formatter, "; allowed={allowed}")?;
        }
        Ok(())
    }
}

impl std::error::Error for LutStoreError {}

impl From<LutStoreError> for MediaError {
    fn from(error: LutStoreError) -> Self {
        Self::Backend(error.to_string())
    }
}

/// Quote text back without control characters or unbounded length.
fn sanitize(text: &str) -> String {
    let mut sanitized = String::with_capacity(text.len().min(OBSERVED_LIMIT));
    for character in text.chars().take(OBSERVED_LIMIT) {
        if character.is_control() {
            sanitized.push(' ');
        } else {
            sanitized.push(character);
        }
    }
    if text.chars().nth(OBSERVED_LIMIT).is_some() {
        sanitized.push('…');
    }
    sanitized
}

/// Whether a string is exactly 64 lowercase hexadecimal characters.
///
/// The same spelling M41 requires of a media source fingerprint, repeated here
/// because it guards a path component.
fn is_canonical_sha256(hash: &str) -> bool {
    hash.len() == 64
        && hash
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

/// Metadata-only result of one import (CC4 §2.4). No samples ever cross into
/// the document, the journal, a branch, or a recovery record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LutAssetImport {
    /// SHA-256 over the original file bytes, before any BOM strip.
    pub sha256: String,
    /// The `.cube` `TITLE` when present, otherwise the source file stem.
    pub title: String,
    /// The interchange form; CC4 imports only [`LutAssetKind::Cube3d`].
    pub kind: LutAssetKind,
    /// Lattice edge length parsed from the bytes.
    pub size: u32,
    /// Length of the original file in bytes.
    pub byte_len: u64,
    /// `DOMAIN_MIN` mirror, rounded half away from zero.
    pub domain_min_millionths: [i64; 3],
    /// `DOMAIN_MAX` mirror, rounded half away from zero.
    pub domain_max_millionths: [i64; 3],
    /// The path the operator chose. Informational only: never opened by the
    /// renderer and never resolved relative to anything.
    pub source_path: String,
}

impl LutAssetImport {
    /// Turn the import into the project record the caller submits with
    /// `AddLutAsset` under a freshly allocated id.
    #[must_use]
    pub fn into_lut_asset(self, id: LutAssetId) -> LutAsset {
        LutAsset {
            id,
            sha256: self.sha256,
            title: self.title,
            kind: self.kind,
            size: self.size,
            byte_len: self.byte_len,
            domain_min_millionths: self.domain_min_millionths,
            domain_max_millionths: self.domain_max_millionths,
            source: LutAssetSource::Imported {
                source_path: self.source_path,
            },
        }
    }
}

/// The project-relative LUT store rooted at `<dir>/<stem>.kinewright-assets`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LutStore {
    root: PathBuf,
}

impl LutStore {
    /// Derive the store root from a saved project path (CC4 §2.2).
    ///
    /// The root is `<parent>/<file stem>.kinewright-assets`, taken from the
    /// file stem regardless of the project file's extension: `edit.kinewright`
    /// and `edit.json` both derive `edit.kinewright-assets`. The root is never
    /// persisted anywhere; it is recomputed from the project path every time,
    /// which is what makes copying the project file plus the
    /// `<stem>.kinewright-assets` directory sufficient to relocate a project.
    ///
    /// The project path is **absolutized first**, with
    /// [`std::path::absolute`], and the root is derived from the absolute
    /// parent. A relative path such as the `edit.kinewright` an operator types
    /// on the command line names exactly the same file as `./edit.kinewright`,
    /// so the two must derive the same store; refusing one and accepting the
    /// other would make the store depend on how the path happened to be
    /// spelled. Absolutizing once, here, is what removes the working-directory
    /// dependence CC4 §2.2 forbids: the root is pinned at derivation time and
    /// cannot move if the process working directory changes later.
    ///
    /// # Errors
    ///
    /// Returns `lut_store_root_invalid` when the absolute path has no parent
    /// component or no file stem — a root such as `/`, which names no project
    /// file — when the working directory cannot be read, and when the derived
    /// root already exists as a symlink or a non-directory.
    pub fn for_project(project_path: &Path) -> Result<Self, MediaError> {
        let absolute = std::path::absolute(project_path).map_err(|error| {
            LutStoreError::new(
                LutStoreErrorCode::LutStoreRootInvalid,
                format!("the project path could not be made absolute: {error}"),
            )
            .with_observed(&project_path.display().to_string())
            .with_allowed("a project file path that resolves against a readable working directory")
        })?;
        let parent = absolute
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .ok_or_else(|| {
                LutStoreError::new(
                    LutStoreErrorCode::LutStoreRootInvalid,
                    "the project path has no parent directory".to_owned(),
                )
                .with_observed(&absolute.display().to_string())
                .with_allowed("a saved project file path such as <dir>/<stem>.kinewright")
            })?;
        let stem = absolute.file_stem().ok_or_else(|| {
            LutStoreError::new(
                LutStoreErrorCode::LutStoreRootInvalid,
                "the project path has no file stem".to_owned(),
            )
            .with_observed(&absolute.display().to_string())
            .with_allowed("a saved project file path such as <dir>/<stem>.kinewright")
        })?;
        let mut directory = OsString::from(stem);
        directory.push(".");
        directory.push(LUT_STORE_SUFFIX);
        let root = parent.join(directory);
        check_store_directory(&root)?;
        Ok(Self { root })
    }

    /// The store root, `<dir>/<stem>.kinewright-assets`.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The directory holding `<sha256>.cube` files.
    #[must_use]
    pub fn luts_dir(&self) -> PathBuf {
        self.root.join(LUT_STORE_LUTS_DIRECTORY)
    }

    /// The store path for one content hash.
    ///
    /// The hash is validated before it is interpolated, so the only file names
    /// this can produce are 64 hex characters plus `.cube`.
    ///
    /// # Errors
    ///
    /// Returns `invalid_lut_asset_hash` when the hash is not exactly 64
    /// lowercase hexadecimal characters.
    pub fn path_for(&self, sha256: &str) -> Result<PathBuf, MediaError> {
        self.store_path(sha256).map_err(MediaError::from)
    }

    /// [`LutStore::path_for`] with the typed failure kept, for the availability
    /// surfaces that report a reason rather than returning an error.
    fn store_path(&self, sha256: &str) -> Result<PathBuf, LutStoreError> {
        if !is_canonical_sha256(sha256) {
            return Err(LutStoreError::new(
                LutStoreErrorCode::InvalidLutAssetHash,
                "a LUT asset hash must be a canonical SHA-256 digest".to_owned(),
            )
            .with_observed(sha256)
            .with_allowed("64 lowercase hexadecimal characters"));
        }
        Ok(self.luts_dir().join(format!("{sha256}.cube")))
    }

    /// Import one `.cube` file into the store (CC4 §2.4).
    ///
    /// Reads the file, parses it (stripping a leading BOM for the parse only),
    /// hashes the **original** bytes, and writes `<sha256>.cube` through a
    /// temporary file in the same directory. An existing file that already
    /// hashes correctly is left untouched, so re-importing the same LUT is
    /// idempotent and does not disturb its modification time.
    ///
    /// # Errors
    ///
    /// Returns the typed parse rejection for an unusable file, and
    /// `unreadable_lut_asset` / `lut_store_write_failed` for filesystem
    /// failures.
    pub fn import_lut_asset(&self, source: &Path) -> Result<LutAssetImport, MediaError> {
        let bytes = read_regular_file(source)?;
        let lut = parse_cube_lut_bytes(&bytes)?;
        let sha256 = sha256_bytes(&bytes);
        let destination = self.path_for(&sha256)?;
        self.write_store_file(&destination, &bytes, &sha256)?;
        let (domain_min_millionths, domain_max_millionths) = lut.domain_millionths();
        Ok(LutAssetImport {
            sha256,
            title: lut.title.clone().unwrap_or_else(|| source_title(source)),
            kind: LutAssetKind::Cube3d,
            size: lut.size,
            byte_len: bytes.len() as u64,
            domain_min_millionths,
            domain_max_millionths,
            source_path: source.display().to_string(),
        })
    }

    /// Observe one asset's availability (CC4 §2.3).
    ///
    /// A built-in never touches the filesystem: it is `verified` exactly when
    /// this binary's bake hashes to the recorded sha256, and `changed`
    /// otherwise, so an older project is never silently re-rendered by a new
    /// bake.
    ///
    /// An asset whose identity verifies is then checked *against its own
    /// record*: the lattice is parsed — through the bounded process parse
    /// cache, so a document that was just rendered costs only a hash lookup —
    /// and a `size` or domain that disagrees with the bytes is reported as
    /// `changed` with the `lut_asset_metadata_mismatch:` reason. This is the
    /// same admission rule [`LutLibrary::build`] applies, so `verified` here
    /// means "this is what the render will use" rather than merely "the bytes
    /// hash correctly": without it a hand-edited record passes every preflight
    /// and fails only once the frame is being rendered.
    #[must_use]
    pub fn availability(&self, asset: &LutAsset) -> LutAvailabilityStatus {
        let status = match &asset.source {
            LutAssetSource::Builtin { name } => builtin_availability(name, &asset.sha256),
            LutAssetSource::Imported { .. } => self.imported_availability(&asset.sha256),
        };
        if status.kind != LutAvailabilityKind::Verified {
            return status;
        }
        let Some(lut) = self.verified_lattice(asset) else {
            return unreadable(
                status.path,
                status.observed_sha256,
                &LutStoreError::new(
                    LutStoreErrorCode::UnreadableLutAsset,
                    "the LUT bytes hash to the recorded content but cannot be parsed".to_owned(),
                )
                .with_allowed("a parsable 3D .cube file")
                .to_string(),
            );
        };
        match metadata_mismatch(asset, &lut) {
            Some(mismatch) => {
                metadata_mismatch_status(status.path, status.observed_sha256, mismatch)
            }
            None => status,
        }
    }

    /// The parsed lattice behind an asset whose identity has already verified.
    ///
    /// The process parse cache is keyed by content hash and the caller has
    /// just confirmed the bytes hash to `asset.sha256`, so a cache hit is by
    /// construction the right lattice. A miss re-reads the store file under
    /// the same [`LUT_MAX_FILE_BYTES`] cap the render path uses.
    fn verified_lattice(&self, asset: &LutAsset) -> Option<Arc<CubeLut>> {
        if let Some(cached) = cached_parse(&asset.sha256) {
            return Some(cached);
        }
        match &asset.source {
            LutAssetSource::Builtin { name } => {
                BuiltinLook::from_name(name).map(BuiltinLook::cached_bake)
            }
            LutAssetSource::Imported { .. } => {
                let path = self.store_path(&asset.sha256).ok()?;
                let bytes = read_store_bytes(&path).ok()?;
                let parsed = Arc::new(parse_cube_lut_bytes(&bytes).ok()?);
                cache_parse(&asset.sha256, &parsed);
                Some(parsed)
            }
        }
    }

    /// The availability probe `kinewright_core::export_lut_preflight_with`
    /// takes, bound to this store root.
    pub fn availability_resolver(&self) -> impl Fn(&LutAsset) -> LutAvailabilityStatus + '_ {
        move |asset| self.availability(asset)
    }

    /// Observe availability for imported provenance.
    fn imported_availability(&self, sha256: &str) -> LutAvailabilityStatus {
        let path = match self.store_path(sha256) {
            Ok(path) => path,
            Err(error) => {
                return unreadable(Some(self.luts_dir()), None, &error.to_string());
            }
        };
        // `symlink_metadata` does not follow a link, so a symlinked store entry
        // is reported as `missing` instead of being read from outside the
        // project directory.
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return missing(&path, "the store file is absent");
            }
            Err(error) => {
                return unreadable(
                    Some(path),
                    None,
                    &LutStoreError::new(
                        LutStoreErrorCode::UnreadableLutAsset,
                        format!("could not inspect the store file: {error}"),
                    )
                    .to_string(),
                );
            }
        };
        if !metadata.is_file() {
            return missing(&path, "the store path is not a regular file");
        }
        match sha256_file(&path) {
            Ok(observed) if observed == sha256 => LutAvailabilityStatus {
                kind: LutAvailabilityKind::Verified,
                observed_sha256: Some(observed),
                reason: None,
                path: Some(path),
            },
            Ok(observed) => {
                let reason = LutStoreError::new(
                    LutStoreErrorCode::ChangedLutAsset,
                    "the store file no longer hashes to the recorded content".to_owned(),
                )
                .with_observed(&observed)
                .with_allowed(sha256)
                .to_string();
                LutAvailabilityStatus {
                    kind: LutAvailabilityKind::Changed,
                    observed_sha256: Some(observed),
                    reason: Some(reason),
                    path: Some(path),
                }
            }
            Err(error) => unreadable(
                Some(path),
                None,
                &LutStoreError::new(
                    LutStoreErrorCode::UnreadableLutAsset,
                    format!("could not hash the store file: {error}"),
                )
                .to_string(),
            ),
        }
    }

    /// Restore an asset's bytes from a candidate the operator located.
    ///
    /// The candidate is accepted only when it hashes exactly to the recorded
    /// content. This is a media repair action, not a Core operation: content
    /// addressing means the document stores no locator to change, so a restore
    /// changes no document state (CC4 §2.3).
    ///
    /// # Errors
    ///
    /// Returns `lut_relink_hash_mismatch` naming the expected and observed
    /// hashes when the candidate is a different file, and leaves the store
    /// untouched.
    pub fn restore(&self, asset: &LutAsset, candidate: &Path) -> Result<PathBuf, MediaError> {
        let destination = self.path_for(&asset.sha256)?;
        let bytes = read_regular_file(candidate)?;
        let observed = sha256_bytes(&bytes);
        if observed != asset.sha256 {
            return Err(LutStoreError::new(
                LutStoreErrorCode::LutRelinkHashMismatch,
                format!(
                    "{} does not carry the bytes this LUT asset records",
                    candidate.display()
                ),
            )
            .with_observed(&observed)
            .with_allowed(&asset.sha256)
            .into());
        }
        self.write_store_file(&destination, &bytes, &observed)?;
        Ok(destination)
    }

    /// Copy every referenced store file into another store root for Save As
    /// (CC4 §2.2).
    ///
    /// Reports one result per asset so a project can still be saved with an
    /// unavailable asset, which is simply `missing` at the new path. Built-ins
    /// succeed without touching the filesystem: they are generated in the
    /// binary and are never written to a store.
    #[must_use]
    pub fn copy_to(
        &self,
        other: &Self,
        assets: &[LutAsset],
    ) -> Vec<(LutAssetId, Result<(), MediaError>)> {
        assets
            .iter()
            .map(|asset| (asset.id, self.copy_one(other, asset)))
            .collect()
    }

    fn copy_one(&self, other: &Self, asset: &LutAsset) -> Result<(), MediaError> {
        self.copy_one_file(other, asset).map_err(|error| {
            let MediaError::Backend(reason) = &error else {
                return error;
            };
            LutStoreError::new(
                LutStoreErrorCode::LutStoreCopyFailed,
                format!("could not copy LUT asset {} into the new store", asset.id),
            )
            .with_observed(reason)
            .into()
        })
    }

    /// The copy itself, reporting the underlying typed failure.
    fn copy_one_file(&self, other: &Self, asset: &LutAsset) -> Result<(), MediaError> {
        if matches!(asset.source, LutAssetSource::Builtin { .. }) || self.root == other.root {
            return Ok(());
        }
        let source = self.path_for(&asset.sha256)?;
        let destination = other.path_for(&asset.sha256)?;
        let bytes = read_regular_file(&source).map_err(|error| {
            MediaError::Backend(
                LutStoreError::new(
                    LutStoreErrorCode::MissingLutAsset,
                    format!("could not read {} for Save As: {error}", source.display()),
                )
                .with_allowed(&asset.sha256)
                .to_string(),
            )
        })?;
        let observed = sha256_bytes(&bytes);
        if observed != asset.sha256 {
            return Err(LutStoreError::new(
                LutStoreErrorCode::ChangedLutAsset,
                format!("{} no longer carries the recorded bytes", source.display()),
            )
            .with_observed(&observed)
            .with_allowed(&asset.sha256)
            .into());
        }
        other.write_store_file(&destination, &bytes, &observed)
    }

    /// Write `bytes` to `destination` unless a correctly hashed file is already
    /// there, in which case the existing file is left completely untouched.
    ///
    /// The dedup comparison reads the existing file only when its metadata
    /// length is within [`LUT_MAX_FILE_BYTES`]. A store entry larger than the
    /// cap is never read into memory; it is simply not a dedup candidate, so
    /// the freshly hashed `bytes` overwrite it. That is the safe direction:
    /// the incoming bytes already hash to `sha256`, and an oversized file at a
    /// content-addressed path cannot be the content it claims to be.
    fn write_store_file(
        &self,
        destination: &Path,
        bytes: &[u8],
        sha256: &str,
    ) -> Result<(), MediaError> {
        let directory = self.luts_dir();
        check_store_directory(self.root())?;
        check_store_directory(&directory)?;
        check_store_entry(destination)?;
        fs::create_dir_all(&directory).map_err(|error| {
            MediaError::from(LutStoreError::new(
                LutStoreErrorCode::LutStoreWriteFailed,
                format!(
                    "could not create the LUT store directory {}: {error}",
                    directory.display()
                ),
            ))
        })?;
        if let Ok(metadata) = fs::symlink_metadata(destination)
            && metadata.is_file()
            && metadata.len() <= LUT_MAX_FILE_BYTES
            && fs::read(destination).is_ok_and(|existing| sha256_bytes(&existing) == sha256)
        {
            return Ok(());
        }
        atomic_write_store_file(destination, bytes)
    }
}

/// Refuse a store root or sub-directory that is a symlink or not a directory.
///
/// The store is application-owned and always sits beside the project file, so
/// a symlinked root, `luts/`, or entry is refused rather than followed: a
/// write must never leave the project directory. This mirrors the cache-root
/// symlink refusal in `derived_cache.rs`.
fn check_store_directory(path: &Path) -> Result<(), MediaError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            Err(root_invalid(path, "the store directory is a symlink"))
        }
        Ok(metadata) if metadata.is_dir() => Ok(()),
        Ok(_) => Err(root_invalid(
            path,
            "the store directory path exists and is not a directory",
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(root_invalid(
            path,
            &format!("could not inspect the store directory: {error}"),
        )),
    }
}

/// Refuse a store file that is a symlink or is not a regular file.
fn check_store_entry(path: &Path) -> Result<(), MediaError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            Err(root_invalid(path, "the store file is a symlink"))
        }
        Ok(metadata) if metadata.is_file() => Ok(()),
        Ok(_) => Err(root_invalid(
            path,
            "the store file exists and is not a regular file",
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(root_invalid(
            path,
            &format!("could not inspect the store file: {error}"),
        )),
    }
}

/// One `lut_store_root_invalid` failure naming the path and the reason.
fn root_invalid(path: &Path, reason: &str) -> MediaError {
    MediaError::from(
        LutStoreError::new(LutStoreErrorCode::LutStoreRootInvalid, reason.to_owned())
            .with_observed(&path.display().to_string()),
    )
}

/// The next temporary-file sequence number for this process.
static TEMPORARY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Write a store file by writing a temporary file in the same directory and
/// renaming it into place.
///
/// This is deliberately not `derived_cache::atomic_write`: the store is not a
/// cache (CC4 §2.2 forbids cache clearing from touching it), its failures are
/// reported with typed store codes, and the rename here replaces the
/// destination directly instead of unlinking it first.
fn atomic_write_store_file(destination: &Path, bytes: &[u8]) -> Result<(), MediaError> {
    let sequence = TEMPORARY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temporary =
        destination.with_extension(format!("cube.tmp-{}-{sequence}", std::process::id()));
    fs::write(&temporary, bytes).map_err(|error| {
        MediaError::from(LutStoreError::new(
            LutStoreErrorCode::LutStoreWriteFailed,
            format!(
                "could not write the LUT store file {}: {error}",
                temporary.display()
            ),
        ))
    })?;
    fs::rename(&temporary, destination).map_err(|error| {
        let _ = fs::remove_file(&temporary);
        MediaError::from(LutStoreError::new(
            LutStoreErrorCode::LutStoreWriteFailed,
            format!(
                "could not commit the LUT store file {}: {error}",
                destination.display()
            ),
        ))
    })
}

/// Read a regular file's bytes with a typed `unreadable_lut_asset` failure.
fn read_regular_file(path: &Path) -> Result<Vec<u8>, MediaError> {
    let metadata = fs::metadata(path).map_err(|error| {
        MediaError::from(
            LutStoreError::new(
                LutStoreErrorCode::UnreadableLutAsset,
                format!("could not inspect {}: {error}", path.display()),
            )
            .with_allowed("a readable regular .cube file"),
        )
    })?;
    if !metadata.is_file() {
        return Err(LutStoreError::new(
            LutStoreErrorCode::UnreadableLutAsset,
            format!("{} is not a regular file", path.display()),
        )
        .with_allowed("a readable regular .cube file")
        .into());
    }
    // The length is checked from the metadata, so an oversized file is never
    // read into memory at all.
    if metadata.len() > LUT_MAX_FILE_BYTES {
        return Err(LutStoreError::new(
            LutStoreErrorCode::LutFileTooLarge,
            format!("{} is larger than the LUT file limit", path.display()),
        )
        .with_observed(&metadata.len().to_string())
        .with_allowed(&LUT_MAX_FILE_BYTES.to_string())
        .into());
    }
    fs::read(path).map_err(|error| {
        MediaError::from(
            LutStoreError::new(
                LutStoreErrorCode::UnreadableLutAsset,
                format!("could not read {}: {error}", path.display()),
            )
            .with_allowed("a readable regular .cube file"),
        )
    })
}

/// The fallback title for an import whose file carries no `TITLE`.
fn source_title(source: &Path) -> String {
    source
        .file_stem()
        .map(|stem| stem.to_string_lossy().trim().to_owned())
        .filter(|title| !title.is_empty())
        .unwrap_or_else(|| "Imported LUT".to_owned())
}

/// A `missing` observation for one store path.
fn missing(path: &Path, detail: &str) -> LutAvailabilityStatus {
    LutAvailabilityStatus {
        kind: LutAvailabilityKind::Missing,
        observed_sha256: None,
        reason: Some(
            LutStoreError::new(
                LutStoreErrorCode::MissingLutAsset,
                format!("{detail}: {}", path.display()),
            )
            .to_string(),
        ),
        path: Some(path.to_owned()),
    }
}

/// An `unreadable` observation for one store path.
fn unreadable(
    path: Option<PathBuf>,
    observed_sha256: Option<String>,
    reason: &str,
) -> LutAvailabilityStatus {
    LutAvailabilityStatus {
        kind: LutAvailabilityKind::Unreadable,
        observed_sha256,
        reason: Some(reason.to_owned()),
        path,
    }
}

/// Read a store file's bytes, or the typed availability that explains why not.
///
/// The [`LUT_MAX_FILE_BYTES`] cap is enforced here as well as on the import
/// path: the store is a directory on disk that a user can write into, so a
/// file that grew past the cap after it was imported must not be read into
/// memory just because it once passed the gate.
fn read_store_bytes(path: &Path) -> Result<Vec<u8>, LutAvailabilityStatus> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() => {
            if metadata.len() > LUT_MAX_FILE_BYTES {
                return Err(unreadable(
                    Some(path.to_owned()),
                    None,
                    &LutStoreError::new(
                        LutStoreErrorCode::LutFileTooLarge,
                        format!("{} is larger than the LUT file limit", path.display()),
                    )
                    .with_observed(&metadata.len().to_string())
                    .with_allowed(&LUT_MAX_FILE_BYTES.to_string())
                    .to_string(),
                ));
            }
            fs::read(path).map_err(|error| {
                unreadable(
                    Some(path.to_owned()),
                    None,
                    &LutStoreError::new(
                        LutStoreErrorCode::UnreadableLutAsset,
                        format!("could not read {}: {error}", path.display()),
                    )
                    .to_string(),
                )
            })
        }
        Ok(_) => Err(missing(path, "the store path is not a regular file")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Err(missing(path, "the store file is absent"))
        }
        Err(error) => Err(unreadable(
            Some(path.to_owned()),
            None,
            &LutStoreError::new(
                LutStoreErrorCode::UnreadableLutAsset,
                format!("could not inspect {}: {error}", path.display()),
            )
            .to_string(),
        )),
    }
}

/// Availability of a built-in asset, computed entirely from the binary.
fn builtin_availability(name: &str, recorded: &str) -> LutAvailabilityStatus {
    let Some(look) = BuiltinLook::from_name(name) else {
        return LutAvailabilityStatus {
            kind: LutAvailabilityKind::Missing,
            observed_sha256: None,
            reason: Some(
                LutStoreError::new(
                    LutStoreErrorCode::UnknownBuiltinLook,
                    "this build has no bake for the recorded built-in look".to_owned(),
                )
                .with_observed(name)
                .to_string(),
            ),
            path: None,
        };
    };
    let observed = look.sha256();
    if observed == recorded {
        return LutAvailabilityStatus {
            kind: LutAvailabilityKind::Verified,
            observed_sha256: Some(observed.to_owned()),
            reason: None,
            path: None,
        };
    }
    LutAvailabilityStatus {
        kind: LutAvailabilityKind::Changed,
        observed_sha256: Some(observed.to_owned()),
        reason: Some(
            LutStoreError::new(
                LutStoreErrorCode::ChangedLutAsset,
                format!("this build's {name} bake differs from the recorded content"),
            )
            .with_observed(observed)
            .with_allowed(recorded)
            .to_string(),
        ),
        path: None,
    }
}

/// The retained-sample budget of the process parse cache.
///
/// A 65³ lattice is 65³ × 4 samples × 4 bytes ≈ 4.4 MiB, so 128 MiB holds
/// roughly thirty of the largest legal LUTs and far more of the usual 33³
/// ones - comfortably more than a session's working set - while keeping the
/// ceiling bounded. Eviction is never a correctness question: a miss re-parses
/// from the same hash-verified bytes and produces the same lattice.
pub const LUT_PARSE_CACHE_MAX_BYTES: u64 = 128 * 1024 * 1024;

/// The entry ceiling of the process parse cache.
///
/// The byte budget alone would admit an unbounded number of `S = 2` lattices,
/// each of which still costs a hash key and an allocation.
pub const LUT_PARSE_CACHE_MAX_ENTRIES: usize = 256;

/// Process-wide parse cache, keyed by content hash (CC4 §2.4).
///
/// Keying on the hash rather than on path plus modification time is what makes
/// a same-path replacement unable to serve stale samples.
///
/// The cache is bounded in both retained bytes and entries, and is ordered
/// most-recently-used first. Nothing downstream may depend on an entry staying
/// resident: the compositor's atlas cache retains its own `Arc<CubeLut>` for
/// every slot it holds, so an eviction here can never invalidate an atlas.
static PARSE_CACHE: LazyLock<Mutex<ParseCache>> = LazyLock::new(|| {
    Mutex::new(ParseCache::new(
        LUT_PARSE_CACHE_MAX_BYTES,
        LUT_PARSE_CACHE_MAX_ENTRIES,
    ))
});

/// An MRU-ordered, byte- and entry-bounded map from content hash to lattice.
#[derive(Debug)]
struct ParseCache {
    /// Most recently used first. Linear scan: the entry cap keeps this short
    /// and every operation already costs a 64-character hash comparison.
    entries: Vec<(String, Arc<CubeLut>)>,
    /// The retained sample bytes of `entries`, kept incrementally.
    bytes: u64,
    /// The retained-byte budget; the process cache uses
    /// [`LUT_PARSE_CACHE_MAX_BYTES`] and tests use a small one.
    max_bytes: u64,
    /// The entry ceiling; the process cache uses
    /// [`LUT_PARSE_CACHE_MAX_ENTRIES`].
    max_entries: usize,
}

impl ParseCache {
    /// An empty cache with the supplied bounds.
    const fn new(max_bytes: u64, max_entries: usize) -> Self {
        Self {
            entries: Vec::new(),
            bytes: 0,
            max_bytes,
            max_entries,
        }
    }

    /// The sample bytes one lattice retains.
    fn entry_bytes(lut: &CubeLut) -> u64 {
        u64::try_from(lut.rgba.len())
            .unwrap_or(u64::MAX)
            .saturating_mul(4)
    }

    /// Look one lattice up, promoting it to most-recently-used on a hit.
    fn get(&mut self, sha256: &str) -> Option<Arc<CubeLut>> {
        let index = self.entries.iter().position(|(key, _)| key == sha256)?;
        let entry = self.entries.remove(index);
        let lut = Arc::clone(&entry.1);
        self.entries.insert(0, entry);
        Some(lut)
    }

    /// Record one lattice as most-recently-used, then evict down to budget.
    fn insert(&mut self, sha256: &str, lut: &Arc<CubeLut>) {
        if let Some(index) = self.entries.iter().position(|(key, _)| key == sha256) {
            let (_, previous) = self.entries.remove(index);
            self.bytes = self.bytes.saturating_sub(Self::entry_bytes(&previous));
        }
        self.bytes = self.bytes.saturating_add(Self::entry_bytes(lut));
        self.entries.insert(0, (sha256.to_owned(), Arc::clone(lut)));
        // The head is never evicted, so the lattice just parsed stays resident
        // even when it alone exceeds the budget.
        while self.entries.len() > 1
            && (self.bytes > self.max_bytes || self.entries.len() > self.max_entries)
            && let Some((_, evicted)) = self.entries.pop()
        {
            self.bytes = self.bytes.saturating_sub(Self::entry_bytes(&evicted));
        }
    }
}

/// Look one verified lattice up in the process parse cache.
fn cached_parse(sha256: &str) -> Option<Arc<CubeLut>> {
    PARSE_CACHE
        .lock()
        .ok()
        .and_then(|mut cache| cache.get(sha256))
}

/// Record one verified lattice in the process parse cache.
fn cache_parse(sha256: &str, lut: &Arc<CubeLut>) {
    if let Ok(mut cache) = PARSE_CACHE.lock() {
        cache.insert(sha256, lut);
    }
}

/// Report the first record field that disagrees with the verified bytes
/// (CC4 §2.1's `lut_asset_metadata_mismatch`).
///
/// The bytes are the authority; because they are hash-verified, a mismatch can
/// only mean the project JSON was hand-edited.
#[must_use]
pub fn metadata_mismatch(
    asset: &LutAsset,
    lut: &CubeLut,
) -> Option<(&'static str, String, String)> {
    if asset.size != lut.size {
        return Some(("size", asset.size.to_string(), lut.size.to_string()));
    }
    let (minimum, maximum) = lut.domain_millionths();
    if asset.domain_min_millionths != minimum {
        return Some((
            "domain_min_millionths",
            format!("{:?}", asset.domain_min_millionths),
            format!("{minimum:?}"),
        ));
    }
    if asset.domain_max_millionths != maximum {
        return Some((
            "domain_max_millionths",
            format!("{:?}", asset.domain_max_millionths),
            format!("{maximum:?}"),
        ));
    }
    None
}

/// One admitted lattice together with the content hash it was verified
/// against.
///
/// Retaining the hash is what lets a library be *published* into a
/// content-addressed table several documents share: the id is a per-document
/// name, the hash is the identity (CC4 §2.4).
#[derive(Debug, Clone)]
struct LibraryEntry {
    sha256: String,
    lut: Arc<CubeLut>,
}

/// The verified lattices the renderer evaluates, keyed by asset id (CC4 §2.4).
///
/// The compositor and the CPU reference consume this and never open a file.
///
/// A library is always **document-local**: [`LutAssetId`]s restart at 1 in
/// every project, so a library built for one document must never be used to
/// resolve another's nodes. [`LutLibrary::entries`] and
/// [`LutLibrary::from_document_assets`] are the pair that makes crossing that
/// boundary safe — publish by content hash, rebind by content hash.
#[derive(Debug, Clone, Default)]
pub struct LutLibrary {
    entries: BTreeMap<LutAssetId, LibraryEntry>,
}

impl LutLibrary {
    /// Load and verify every document asset, reporting one status per asset.
    ///
    /// An asset is admitted only when its bytes hash to the recorded content
    /// and its record agrees with those bytes, so a node bound to a `missing`,
    /// `changed`, or hand-edited asset resolves to nothing and blocks rather
    /// than silently rendering a different look.
    #[must_use]
    pub fn build(
        document_assets: &[LutAsset],
        store: Option<&LutStore>,
    ) -> (Self, Vec<(LutAssetId, LutAvailabilityStatus)>) {
        let mut entries = BTreeMap::new();
        let mut statuses = Vec::with_capacity(document_assets.len());
        for asset in document_assets {
            let (lut, status) = load_asset(asset, store);
            if let Some(lut) = lut {
                entries.insert(
                    asset.id,
                    LibraryEntry {
                        sha256: asset.sha256.clone(),
                        lut,
                    },
                );
            }
            statuses.push((asset.id, status));
        }
        (Self { entries }, statuses)
    }

    /// Rebind one document's assets to already-published lattices (CC4 §2.4).
    ///
    /// This is the render-time half of publication. [`LutLibrary::build`]
    /// reads and hash-verifies bytes from one project's store; the engine
    /// merges the result into a table keyed by content hash that every open
    /// project shares; and each document-taking render path rebuilds a
    /// *document-local* library here, at render time, from its own
    /// `Document.lut_assets`.
    ///
    /// Resolving by id against a shared table would be an aliasing bug:
    /// `LutAssetId(1)` names a different look in every project, so whichever
    /// project published last would answer for all of them. Resolving by
    /// `sha256` cannot alias, because the hash *is* the look.
    ///
    /// Built-ins never need publication: they are generated in this binary, so
    /// they resolve straight from the pinned bake table. A recorded hash this
    /// build does not bake, and a record that disagrees with the bytes it
    /// names, are both withheld rather than silently substituted.
    ///
    /// Returns the library and the ids that could not be bound, in document
    /// order. An unbound id is not by itself a failure: only a node that can
    /// actually evaluate it fails the render, with `missing_lut_asset`.
    #[must_use]
    pub fn from_document_assets(
        document_assets: &[LutAsset],
        published: &HashMap<String, Arc<CubeLut>>,
    ) -> (Self, Vec<LutAssetId>) {
        let mut entries = BTreeMap::new();
        let mut missing = Vec::new();
        for asset in document_assets {
            let candidate = match &asset.source {
                LutAssetSource::Builtin { name } => BuiltinLook::from_name(name)
                    .filter(|look| look.sha256() == asset.sha256)
                    .map(BuiltinLook::cached_bake),
                LutAssetSource::Imported { .. } => published.get(&asset.sha256).map(Arc::clone),
            };
            match candidate.filter(|lut| metadata_mismatch(asset, lut).is_none()) {
                Some(lut) => {
                    entries.insert(
                        asset.id,
                        LibraryEntry {
                            sha256: asset.sha256.clone(),
                            lut,
                        },
                    );
                }
                None => missing.push(asset.id),
            }
        }
        (Self { entries }, missing)
    }

    /// The verified lattice for one asset id, when it was admitted.
    #[must_use]
    pub fn get(&self, id: LutAssetId) -> Option<&Arc<CubeLut>> {
        self.entries.get(&id).map(|entry| &entry.lut)
    }

    /// Every admitted entry as `(id, sha256, lattice)`, in ascending id order.
    ///
    /// The hash is the one the bytes were verified against, so a caller can
    /// merge this library into a content-addressed table without re-reading or
    /// re-hashing anything.
    pub fn entries(&self) -> impl Iterator<Item = (LutAssetId, &str, &Arc<CubeLut>)> {
        self.entries
            .iter()
            .map(|(id, entry)| (*id, entry.sha256.as_str(), &entry.lut))
    }

    /// Whether the library holds no lattice.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The number of admitted lattices.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

/// Load one asset for [`LutLibrary::build`].
fn load_asset(
    asset: &LutAsset,
    store: Option<&LutStore>,
) -> (Option<Arc<CubeLut>>, LutAvailabilityStatus) {
    match &asset.source {
        LutAssetSource::Builtin { name } => load_builtin(asset, name),
        LutAssetSource::Imported { .. } => match store {
            Some(store) => load_imported(asset, store),
            None => (
                None,
                LutAvailabilityStatus {
                    kind: LutAvailabilityKind::Missing,
                    observed_sha256: None,
                    reason: Some(
                        LutStoreError::new(
                            LutStoreErrorCode::MissingLutAsset,
                            "the project has no store root, so an imported LUT cannot be read"
                                .to_owned(),
                        )
                        .with_allowed("a saved project with a <stem>.kinewright-assets directory")
                        .to_string(),
                    ),
                    path: None,
                },
            ),
        },
    }
}

/// Admit a built-in from the pinned bake table.
fn load_builtin(asset: &LutAsset, name: &str) -> (Option<Arc<CubeLut>>, LutAvailabilityStatus) {
    let status = builtin_availability(name, &asset.sha256);
    if status.kind != LutAvailabilityKind::Verified {
        return (None, status);
    }
    let Some(look) = BuiltinLook::from_name(name) else {
        return (None, status);
    };
    let lut = look.cached_bake();
    match metadata_mismatch(asset, &lut) {
        Some(mismatch) => (
            None,
            metadata_mismatch_status(
                status.path.clone(),
                status.observed_sha256.clone(),
                mismatch,
            ),
        ),
        None => (Some(lut), status),
    }
}

/// Admit an imported asset from the store, hash-verified.
fn load_imported(
    asset: &LutAsset,
    store: &LutStore,
) -> (Option<Arc<CubeLut>>, LutAvailabilityStatus) {
    let path = match store.store_path(&asset.sha256) {
        Ok(path) => path,
        Err(error) => {
            return (
                None,
                unreadable(Some(store.luts_dir()), None, &error.to_string()),
            );
        }
    };
    let bytes = match read_store_bytes(&path) {
        Ok(bytes) => bytes,
        Err(status) => return (None, status),
    };
    let observed = sha256_bytes(&bytes);
    if observed != asset.sha256 {
        let reason = LutStoreError::new(
            LutStoreErrorCode::ChangedLutAsset,
            "the store file no longer hashes to the recorded content".to_owned(),
        )
        .with_observed(&observed)
        .with_allowed(&asset.sha256)
        .to_string();
        return (
            None,
            LutAvailabilityStatus {
                kind: LutAvailabilityKind::Changed,
                observed_sha256: Some(observed),
                reason: Some(reason),
                path: Some(path),
            },
        );
    }
    let lut = match cached_parse(&observed) {
        Some(lut) => lut,
        None => match parse_cube_lut_bytes(&bytes) {
            Ok(parsed) => {
                let parsed = Arc::new(parsed);
                cache_parse(&observed, &parsed);
                parsed
            }
            Err(error) => {
                return (
                    None,
                    unreadable(Some(path), Some(observed), &error.to_string()),
                );
            }
        },
    };
    let verified = LutAvailabilityStatus {
        kind: LutAvailabilityKind::Verified,
        observed_sha256: Some(observed),
        reason: None,
        path: Some(path),
    };
    match metadata_mismatch(asset, &lut) {
        Some(mismatch) => (
            None,
            metadata_mismatch_status(
                verified.path.clone(),
                verified.observed_sha256.clone(),
                mismatch,
            ),
        ),
        None => (Some(lut), verified),
    }
}

/// The blocking status for a record that disagrees with its verified bytes.
///
/// This is CC4 §2.3's `changed` (metadata) row, not `unreadable`: the bytes
/// were read and they hash to the recorded content, so nothing about the file
/// is unreadable - the project record is what disagrees. The `reason` carries
/// the `lut_asset_metadata_mismatch:` code with the offending field, and the
/// asset is still withheld from the library.
///
/// The recovery is deliberately *not* "restore the file". Restore accepts only
/// a candidate that hashes to the recorded content, and the file already does;
/// re-running it would change nothing. The only honest repair is to re-import
/// the LUT, which writes a record derived from the bytes, or to replace the
/// hand-edited record with one that agrees with them.
fn metadata_mismatch_status(
    path: Option<PathBuf>,
    observed_sha256: Option<String>,
    (field, observed, allowed): (&'static str, String, String),
) -> LutAvailabilityStatus {
    LutAvailabilityStatus {
        kind: LutAvailabilityKind::Changed,
        observed_sha256,
        reason: Some(
            LutStoreError::new(
                LutStoreErrorCode::LutAssetMetadataMismatch,
                format!(
                    "the project record disagrees with the hash-verified bytes at {field}; \
                     re-import the LUT or correct the record, because restoring the file \
                     cannot help when the bytes already hash to the recorded content"
                ),
            )
            .with_observed(&observed)
            .with_allowed(&allowed)
            .to_string(),
        ),
        path,
    }
}

#[cfg(test)]
mod tests {
    use std::{fmt::Write as _, time::Instant};

    use super::*;
    use crate::{builtin_looks::BuiltinLook, test_support::TempDirectory};

    /// A tiny, hand-written `.cube` document with a non-identity lattice.
    const SAMPLE_CUBE: &str = "\
TITLE \"Sample Look\"
LUT_3D_SIZE 2
DOMAIN_MIN 0 0 0
DOMAIN_MAX 1 1 1
0 0 0
0.5 0 0
0 0.5 0
0.5 0.5 0
0 0 0.5
0.5 0 0.5
0 0.5 0.5
1 1 1
";

    /// A second, different `.cube` document used for mismatch fixtures.
    const OTHER_CUBE: &str = "\
LUT_3D_SIZE 2
0 0 0
1 0 0
0 1 0
1 1 0
0 0 1
1 0 1
0 1 1
1 1 1
";

    fn write_source(directory: &TempDirectory, name: &str, contents: &str) -> PathBuf {
        let path = directory.path(name);
        fs::write(&path, contents).expect("the fixture source should be written");
        path
    }

    fn store_for(directory: &TempDirectory, project: &str) -> LutStore {
        LutStore::for_project(&directory.path(project)).expect("the store root should derive")
    }

    fn imported_asset(store: &LutStore, source: &Path, id: u64) -> LutAsset {
        store
            .import_lut_asset(source)
            .expect("the fixture LUT should import")
            .into_lut_asset(LutAssetId(id))
    }

    #[test]
    fn store_root_is_the_project_stem_plus_the_asset_suffix() {
        // Both the fixture and the expectation are composed from the same
        // absolute directory rather than spelled as a literal: a leading-slash
        // string such as "/home/riel/edits" is drive-relative on Windows, so
        // `for_project` absolutizes it onto the current drive and the literal
        // it came from is no longer the answer. Composing both sides keeps the
        // assertion about the derivation instead of about the host's drives.
        // The directory need not exist - the derivation is a pure operation on
        // the parent and the stem.
        let temporary = TempDirectory::new("lut-store-root-stem");
        let edits = temporary.path("edits");

        let saved = LutStore::for_project(&edits.join("Demo Project.kinewright"))
            .expect("a saved project path should derive a store");
        assert_eq!(saved.root(), edits.join("Demo Project.kinewright-assets"));
        assert_eq!(
            saved.luts_dir(),
            edits.join("Demo Project.kinewright-assets").join("luts")
        );

        // "Replace the project extension with `.kinewright-assets`", with the
        // stem surviving verbatim, spaces and interior dots included.
        let awkward = LutStore::for_project(&edits.join("Demo Project v2.final.kinewright"))
            .expect("an awkward but saved project path should derive a store");
        assert_eq!(
            awkward.root(),
            edits.join("Demo Project v2.final.kinewright-assets")
        );

        // A Windows path derives the same way on Windows. On a unix host the
        // very same string is a single bare file name - backslash is not a
        // separator - so it absolutizes against the working directory like
        // any other relative name, keeping its backslashes in the stem.
        let windows = LutStore::for_project(Path::new(r"C:\Users\riel\Demo Project.kinewright"))
            .expect("a windows-shaped project path derives a store on either host")
            .root()
            .to_string_lossy()
            .into_owned();
        assert!(
            windows.ends_with("Demo Project.kinewright-assets"),
            "derived root should keep the stem: {windows}"
        );
        if cfg!(windows) {
            assert!(
                windows.contains(r"C:\Users\riel\") || windows.contains("C:/Users/riel/"),
                "derived root should keep the original prefix: {windows}"
            );
        }
    }

    #[test]
    fn store_root_rejects_a_path_with_no_project_file() {
        // The root directory names no project file: it has neither a parent
        // nor a stem, and no amount of absolutizing invents one.
        let error = LutStore::for_project(Path::new("/")).unwrap_err();
        let MediaError::Backend(message) = error else {
            panic!("store failures cross as MediaError::Backend");
        };
        assert!(
            message.starts_with("lut_store_root_invalid: "),
            "message should lead with the code: {message}"
        );
    }

    #[test]
    fn a_relative_project_name_derives_the_same_store_however_it_is_spelled() {
        // CC4 §2.2 forbids the store depending on the working directory, and
        // the way to honour that is to absolutize once, here - not to refuse
        // one spelling of a path the operating system already resolved. The
        // app passes `argv[1]` through untouched, so `kinewright edit.kinewright`
        // and `kinewright ./edit.kinewright` name one file and must derive one
        // store.
        let working = std::env::current_dir().expect("the test process has a working directory");
        for (bare, dotted) in [
            ("edit.kinewright", "./edit.kinewright"),
            ("edit.json", "./edit.json"),
            ("edit", "./edit"),
        ] {
            let bare_root = LutStore::for_project(Path::new(bare))
                .expect("a bare relative project name derives a store")
                .root()
                .to_owned();
            let dotted_root = LutStore::for_project(Path::new(dotted))
                .expect("an explicitly relative project name derives a store")
                .root()
                .to_owned();
            assert_eq!(bare_root, dotted_root);
            assert_eq!(bare_root, working.join("edit.kinewright-assets"));
            assert!(
                bare_root.is_absolute(),
                "the derived root is pinned, not re-resolved later: {}",
                bare_root.display()
            );
        }

        // A nested relative path absolutizes against the same working
        // directory rather than being refused.
        let nested = LutStore::for_project(Path::new("edits/demo.kinewright"))
            .expect("a nested relative project path derives a store");
        assert_eq!(
            nested.root(),
            working.join("edits").join("demo.kinewright-assets")
        );
    }

    #[test]
    fn path_for_accepts_only_a_canonical_digest() {
        let temporary = TempDirectory::new("lut-store-path");
        let store = store_for(&temporary, "project.kinewright");
        let valid = "a".repeat(64);
        assert_eq!(
            store.path_for(&valid).unwrap(),
            store.luts_dir().join(format!("{valid}.cube"))
        );
        for rejected in [
            "A".repeat(64),
            "a".repeat(63),
            "a".repeat(65),
            "g".repeat(64),
            "../../etc/passwd".to_owned(),
            String::new(),
        ] {
            let error = store.path_for(&rejected).unwrap_err();
            let MediaError::Backend(message) = error else {
                panic!("store failures cross as MediaError::Backend");
            };
            assert!(
                message.starts_with("invalid_lut_asset_hash: "),
                "{rejected} should be rejected by code: {message}"
            );
            assert!(
                message.contains("allowed=64 lowercase hexadecimal characters"),
                "{rejected} should name what is allowed: {message}"
            );
        }
    }

    #[test]
    fn import_writes_the_original_bytes_under_the_content_hash() {
        let temporary = TempDirectory::new("lut-store-import");
        let store = store_for(&temporary, "project.kinewright");
        let source = write_source(&temporary, "sample.cube", SAMPLE_CUBE);
        let import = store.import_lut_asset(&source).unwrap();

        assert_eq!(import.sha256, sha256_bytes(SAMPLE_CUBE.as_bytes()));
        assert_eq!(import.title, "Sample Look");
        assert_eq!(import.kind, LutAssetKind::Cube3d);
        assert_eq!(import.size, 2);
        assert_eq!(import.byte_len, SAMPLE_CUBE.len() as u64);
        assert_eq!(import.domain_min_millionths, [0; 3]);
        assert_eq!(import.domain_max_millionths, [1_000_000; 3]);
        assert_eq!(import.source_path, source.display().to_string());

        let stored = store.path_for(&import.sha256).unwrap();
        assert_eq!(
            stored,
            store
                .root()
                .join("luts")
                .join(format!("{}.cube", import.sha256))
        );
        assert_eq!(fs::read(&stored).unwrap(), SAMPLE_CUBE.as_bytes());

        let asset = import.into_lut_asset(LutAssetId(1));
        assert_eq!(asset.id, LutAssetId(1));
        assert!(kinewright_core::validate_lut_asset(&asset).is_ok());
        assert!(
            matches!(&asset.source, LutAssetSource::Imported { source_path } if source_path == &source.display().to_string())
        );
    }

    #[test]
    fn import_preserves_crlf_bytes_exactly() {
        let temporary = TempDirectory::new("lut-store-crlf");
        let store = store_for(&temporary, "project.kinewright");
        let crlf = SAMPLE_CUBE.replace('\n', "\r\n");
        let source = write_source(&temporary, "crlf.cube", &crlf);
        let import = store.import_lut_asset(&source).unwrap();
        assert_eq!(import.sha256, sha256_bytes(crlf.as_bytes()));
        assert_eq!(import.byte_len, crlf.len() as u64);
        let stored = fs::read(store.path_for(&import.sha256).unwrap()).unwrap();
        assert_eq!(stored, crlf.as_bytes(), "the store keeps the source bytes");
    }

    #[test]
    fn import_hashes_the_original_bytes_including_a_byte_order_mark() {
        let temporary = TempDirectory::new("lut-store-bom");
        let store = store_for(&temporary, "project.kinewright");
        let mut bytes = "\u{feff}".as_bytes().to_vec();
        bytes.extend_from_slice(SAMPLE_CUBE.as_bytes());
        let source = temporary.path("bom.cube");
        fs::write(&source, &bytes).unwrap();
        let import = store.import_lut_asset(&source).unwrap();
        assert_eq!(
            import.sha256,
            sha256_bytes(&bytes),
            "the BOM is stripped for the parse only"
        );
        assert_eq!(
            fs::read(store.path_for(&import.sha256).unwrap()).unwrap(),
            bytes
        );
    }

    #[test]
    fn reimport_is_idempotent_and_leaves_the_store_file_untouched() {
        let temporary = TempDirectory::new("lut-store-reimport");
        let store = store_for(&temporary, "project.kinewright");
        let source = write_source(&temporary, "sample.cube", SAMPLE_CUBE);
        let first = store.import_lut_asset(&source).unwrap();
        let stored = store.path_for(&first.sha256).unwrap();
        let before = fs::metadata(&stored).unwrap();

        let second = store.import_lut_asset(&source).unwrap();
        assert_eq!(first, second);
        let after = fs::metadata(&stored).unwrap();
        assert_eq!(before.modified().unwrap(), after.modified().unwrap());
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt as _;
            assert_eq!(
                before.ino(),
                after.ino(),
                "a correctly hashed store file must not be rewritten"
            );
        }
        assert_eq!(
            fs::read_dir(store.luts_dir()).unwrap().count(),
            1,
            "the store is deduplicated by content hash"
        );
    }

    #[test]
    fn import_rejects_an_unusable_file_with_the_typed_parse_code() {
        let temporary = TempDirectory::new("lut-store-reject");
        let store = store_for(&temporary, "project.kinewright");
        let source = write_source(&temporary, "one-d.cube", "LUT_1D_SIZE 2\n0 0 0\n1 1 1\n");
        let MediaError::Backend(message) = store.import_lut_asset(&source).unwrap_err() else {
            panic!("import failures cross as MediaError::Backend");
        };
        assert!(
            message.starts_with("unsupported_lut_format: "),
            "message should lead with the parse code: {message}"
        );
        assert!(
            !store.luts_dir().exists(),
            "a rejected import writes no store file"
        );

        let MediaError::Backend(message) = store.import_lut_asset(temporary.root()).unwrap_err()
        else {
            panic!("import failures cross as MediaError::Backend");
        };
        assert!(
            message.starts_with("unreadable_lut_asset: "),
            "a directory is not importable: {message}"
        );
    }

    #[test]
    fn availability_reports_verified_missing_and_changed() {
        let temporary = TempDirectory::new("lut-store-availability");
        let store = store_for(&temporary, "project.kinewright");
        let source = write_source(&temporary, "sample.cube", SAMPLE_CUBE);
        let asset = imported_asset(&store, &source, 1);
        let stored = store.path_for(&asset.sha256).unwrap();

        let verified = store.availability(&asset);
        assert_eq!(verified.kind, LutAvailabilityKind::Verified);
        assert_eq!(
            verified.observed_sha256.as_deref(),
            Some(asset.sha256.as_str())
        );
        assert_eq!(verified.reason, None);
        assert_eq!(verified.path, Some(stored.clone()));

        // Corrupt exactly one byte: the file is still a valid `.cube`, so only
        // the hash can catch it.
        let mut bytes = fs::read(&stored).unwrap();
        let last = bytes.len() - 2;
        bytes[last] = b'0';
        fs::write(&stored, &bytes).unwrap();
        let changed = store.availability(&asset);
        assert_eq!(changed.kind, LutAvailabilityKind::Changed);
        assert_eq!(
            changed.observed_sha256.as_deref(),
            Some(sha256_bytes(&bytes).as_str())
        );
        let reason = changed.reason.unwrap();
        assert!(reason.starts_with("changed_lut_asset: "), "{reason}");
        assert!(
            reason.contains(&format!("observed={}", sha256_bytes(&bytes))),
            "{reason}"
        );
        assert!(
            reason.contains(&format!("allowed={}", asset.sha256)),
            "{reason}"
        );

        fs::remove_file(&stored).unwrap();
        let missing = store.availability(&asset);
        assert_eq!(missing.kind, LutAvailabilityKind::Missing);
        assert_eq!(missing.observed_sha256, None);
        assert!(missing.reason.unwrap().starts_with("missing_lut_asset: "));
        assert_eq!(missing.path, Some(stored));
    }

    #[test]
    fn a_directory_in_place_of_a_store_file_is_missing_not_a_regular_file() {
        let temporary = TempDirectory::new("lut-store-directory");
        let store = store_for(&temporary, "project.kinewright");
        let source = write_source(&temporary, "sample.cube", SAMPLE_CUBE);
        let asset = imported_asset(&store, &source, 1);
        let stored = store.path_for(&asset.sha256).unwrap();
        fs::remove_file(&stored).unwrap();
        fs::create_dir(&stored).unwrap();
        // CC4 §2.3: `missing` covers "absent or is not a regular file";
        // `unreadable` is reserved for bytes or metadata that cannot be read.
        let status = store.availability(&asset);
        assert_eq!(status.kind, LutAvailabilityKind::Missing);
        assert!(
            status
                .reason
                .unwrap()
                .contains("the store path is not a regular file")
        );
    }

    #[test]
    fn availability_reports_unreadable_for_a_malformed_recorded_hash() {
        let temporary = TempDirectory::new("lut-store-unreadable-hash");
        let store = store_for(&temporary, "project.kinewright");
        let source = write_source(&temporary, "sample.cube", SAMPLE_CUBE);
        let mut asset = imported_asset(&store, &source, 1);
        asset.sha256 = "NOT-A-HASH".to_owned();
        let status = store.availability(&asset);
        assert_eq!(status.kind, LutAvailabilityKind::Unreadable);
        assert!(
            status
                .reason
                .unwrap()
                .starts_with("invalid_lut_asset_hash: ")
        );
        assert_eq!(status.path, Some(store.luts_dir()));
    }

    #[cfg(unix)]
    #[test]
    fn availability_reports_unreadable_for_bytes_it_cannot_read() {
        use std::os::unix::fs::PermissionsExt as _;

        let temporary = TempDirectory::new("lut-store-unreadable");
        let store = store_for(&temporary, "project.kinewright");
        let source = write_source(&temporary, "sample.cube", SAMPLE_CUBE);
        let asset = imported_asset(&store, &source, 1);
        let stored = store.path_for(&asset.sha256).unwrap();
        fs::set_permissions(&stored, fs::Permissions::from_mode(0o000)).unwrap();
        if fs::File::open(&stored).is_ok() {
            // Running as a user that bypasses the permission bits, so this
            // fixture cannot construct an unreadable file. The malformed-hash
            // fixture still covers the `unreadable` branch.
            return;
        }
        let status = store.availability(&asset);
        assert_eq!(status.kind, LutAvailabilityKind::Unreadable);
        assert!(status.reason.unwrap().starts_with("unreadable_lut_asset: "));
        fs::set_permissions(&stored, fs::Permissions::from_mode(0o600)).unwrap();
    }

    #[test]
    fn the_availability_resolver_matches_the_direct_observation() {
        let temporary = TempDirectory::new("lut-store-resolver");
        let store = store_for(&temporary, "project.kinewright");
        let source = write_source(&temporary, "sample.cube", SAMPLE_CUBE);
        let asset = imported_asset(&store, &source, 1);
        let builtin = BuiltinLook::Warm.to_lut_asset(LutAssetId(2));
        {
            // The resolver is the probe `export_lut_preflight_with` takes, so
            // it must observe exactly what `availability` observes.
            let resolver = store.availability_resolver();
            assert_eq!(resolver(&asset), store.availability(&asset));
            assert_eq!(resolver(&builtin), store.availability(&builtin));
        }
        fs::remove_file(store.path_for(&asset.sha256).unwrap()).unwrap();
        let resolver = store.availability_resolver();
        assert_eq!(resolver(&asset).kind, LutAvailabilityKind::Missing);
        assert_eq!(resolver(&builtin).kind, LutAvailabilityKind::Verified);
    }

    #[test]
    fn builtin_availability_never_touches_the_filesystem() {
        let temporary = TempDirectory::new("lut-store-builtin");
        let store = store_for(&temporary, "project.kinewright");
        let asset = BuiltinLook::Warm.to_lut_asset(LutAssetId(1));
        let verified = store.availability(&asset);
        assert_eq!(verified.kind, LutAvailabilityKind::Verified);
        assert_eq!(
            verified.observed_sha256.as_deref(),
            Some(BuiltinLook::Warm.sha256())
        );
        assert_eq!(verified.path, None, "a built-in has no store path");
        assert!(
            !store.root().exists(),
            "a built-in must not create the store"
        );

        let mut stale = asset.clone();
        stale.sha256 = "0".repeat(64);
        let changed = store.availability(&stale);
        assert_eq!(changed.kind, LutAvailabilityKind::Changed);
        assert!(changed.reason.unwrap().starts_with("changed_lut_asset: "));

        let mut unknown = asset;
        unknown.source = LutAssetSource::Builtin {
            name: "sepia".to_owned(),
        };
        let missing = store.availability(&unknown);
        assert_eq!(missing.kind, LutAvailabilityKind::Missing);
        assert!(
            missing
                .reason
                .unwrap()
                .starts_with("unknown_builtin_look: ")
        );
    }

    #[test]
    fn restore_accepts_the_exact_bytes_and_rejects_a_different_file() {
        let temporary = TempDirectory::new("lut-store-restore");
        let store = store_for(&temporary, "project.kinewright");
        let source = write_source(&temporary, "sample.cube", SAMPLE_CUBE);
        let asset = imported_asset(&store, &source, 1);
        let stored = store.path_for(&asset.sha256).unwrap();
        fs::remove_file(&stored).unwrap();
        assert_eq!(
            store.availability(&asset).kind,
            LutAvailabilityKind::Missing
        );

        let other = write_source(&temporary, "other.cube", OTHER_CUBE);
        let MediaError::Backend(message) = store.restore(&asset, &other).unwrap_err() else {
            panic!("restore failures cross as MediaError::Backend");
        };
        assert!(
            message.starts_with("lut_relink_hash_mismatch: "),
            "message should lead with the code: {message}"
        );
        assert!(
            message.contains(&format!("observed={}", sha256_bytes(OTHER_CUBE.as_bytes()))),
            "message should name the observed hash: {message}"
        );
        assert!(
            message.contains(&format!("allowed={}", asset.sha256)),
            "message should name the expected hash: {message}"
        );
        assert!(
            !stored.exists(),
            "a rejected restore leaves the store alone"
        );

        let restored = store.restore(&asset, &source).unwrap();
        assert_eq!(restored, stored);
        assert_eq!(fs::read(&stored).unwrap(), SAMPLE_CUBE.as_bytes());
        assert_eq!(
            store.availability(&asset).kind,
            LutAvailabilityKind::Verified
        );
    }

    #[test]
    fn copy_to_reproduces_the_store_bytes_for_save_as() {
        let temporary = TempDirectory::new("lut-store-copy");
        let store = store_for(&temporary, "project.kinewright");
        let elsewhere = TempDirectory::new("lut-store-copy-target");
        let target = store_for(&elsewhere, "Renamed Project.kinewright");
        let source = write_source(&temporary, "sample.cube", SAMPLE_CUBE);
        let imported = imported_asset(&store, &source, 1);
        let builtin = BuiltinLook::Cool.to_lut_asset(LutAssetId(2));

        let results = store.copy_to(&target, &[imported.clone(), builtin]);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].0, LutAssetId(1));
        assert!(results[0].1.is_ok());
        assert_eq!(results[1].0, LutAssetId(2));
        assert!(results[1].1.is_ok(), "a built-in needs no store file");
        assert_eq!(
            fs::read(target.path_for(&imported.sha256).unwrap()).unwrap(),
            SAMPLE_CUBE.as_bytes()
        );
        assert_eq!(
            target.availability(&imported).kind,
            LutAvailabilityKind::Verified
        );

        // An asset whose bytes are gone is reported per asset; the project is
        // still saved and the asset is simply missing at the new root.
        fs::remove_file(store.path_for(&imported.sha256).unwrap()).unwrap();
        let second = TempDirectory::new("lut-store-copy-second");
        let third = store_for(&second, "third.kinewright");
        let results = store.copy_to(&third, &[imported]);
        let MediaError::Backend(message) = results[0].1.as_ref().unwrap_err() else {
            panic!("copy failures cross as MediaError::Backend");
        };
        assert!(
            message.starts_with("lut_store_copy_failed: "),
            "message should lead with the code: {message}"
        );
        assert!(
            message.contains("missing_lut_asset"),
            "message should carry the underlying failure: {message}"
        );
    }

    #[test]
    fn library_builds_from_the_store_and_the_builtin_table() {
        let temporary = TempDirectory::new("lut-library");
        let store = store_for(&temporary, "project.kinewright");
        let source = write_source(&temporary, "sample.cube", SAMPLE_CUBE);
        let imported = imported_asset(&store, &source, 1);
        let builtin = BuiltinLook::Monochrome.to_lut_asset(LutAssetId(2));
        let mut absent = imported.clone();
        absent.id = LutAssetId(3);
        absent.sha256 = "f".repeat(64);

        let (library, statuses) =
            LutLibrary::build(&[imported.clone(), builtin, absent], Some(&store));
        assert_eq!(library.len(), 2);
        assert!(!library.is_empty());
        assert_eq!(
            library.get(LutAssetId(1)).unwrap().size,
            2,
            "the imported lattice comes from the verified bytes"
        );
        assert_eq!(library.get(LutAssetId(2)).unwrap().size, 17);
        assert!(library.get(LutAssetId(3)).is_none());
        assert_eq!(statuses[0].1.kind, LutAvailabilityKind::Verified);
        assert_eq!(statuses[1].1.kind, LutAvailabilityKind::Verified);
        assert_eq!(statuses[2].1.kind, LutAvailabilityKind::Missing);

        // The parse cache is keyed by content hash, so a second build serves
        // the same lattice allocation instead of re-parsing.
        let (again, _) = LutLibrary::build(&[imported], Some(&store));
        assert!(Arc::ptr_eq(
            library.get(LutAssetId(1)).unwrap(),
            again.get(LutAssetId(1)).unwrap()
        ));

        let (empty, statuses) = LutLibrary::build(&[], Some(&store));
        assert!(empty.is_empty());
        assert!(statuses.is_empty());
    }

    #[test]
    fn library_reports_an_unsaved_project_for_imported_assets() {
        let temporary = TempDirectory::new("lut-library-unsaved");
        let store = store_for(&temporary, "project.kinewright");
        let source = write_source(&temporary, "sample.cube", SAMPLE_CUBE);
        let imported = imported_asset(&store, &source, 1);
        let builtin = BuiltinLook::Identity.to_lut_asset(LutAssetId(2));
        let (library, statuses) = LutLibrary::build(&[imported, builtin], None);
        assert_eq!(library.len(), 1, "built-ins need no store");
        assert!(library.get(LutAssetId(2)).is_some());
        assert_eq!(statuses[0].1.kind, LutAvailabilityKind::Missing);
        assert!(
            statuses[0]
                .1
                .reason
                .as_ref()
                .unwrap()
                .contains("the project has no store root")
        );
    }

    /// A distinct, well-formed 64-character hash for cache fixtures.
    fn fixture_hash(index: u32) -> String {
        format!("{index:064x}")
    }

    /// A lattice of the requested sample-byte weight, with a distinguishable
    /// first sample so a stale hit would be visible.
    fn weighted_lut(marker: u16, bytes: u64) -> Arc<CubeLut> {
        let samples = usize::try_from(bytes / 4).expect("the fixture weight fits");
        let mut rgba = vec![0.0_f32; samples.max(1)];
        rgba[0] = f32::from(marker);
        Arc::new(CubeLut {
            size: 2,
            domain_min: [0.0; 3],
            domain_max: [1.0; 3],
            rgba,
            title: None,
        })
    }

    #[test]
    fn parse_cache_bounds_match_the_documented_budget() {
        // The process cache is what the constants describe; the tests below
        // exercise the same code with small bounds so they stay cheap.
        assert_eq!(LUT_PARSE_CACHE_MAX_BYTES, 128 * 1024 * 1024);
        assert_eq!(LUT_PARSE_CACHE_MAX_ENTRIES, 256);
        let cache = PARSE_CACHE.lock().expect("the parse cache lock is healthy");
        assert_eq!(cache.max_bytes, LUT_PARSE_CACHE_MAX_BYTES);
        assert_eq!(cache.max_entries, LUT_PARSE_CACHE_MAX_ENTRIES);
        assert!(cache.bytes <= LUT_PARSE_CACHE_MAX_BYTES);
    }

    #[test]
    fn parse_cache_evicts_least_recently_used_entries_past_its_byte_budget() {
        // Three entries fit; the fourth must push exactly one out.
        let weight = 400_u64;
        let mut cache = ParseCache::new(weight * 3, 64);
        for index in 0..3_u32 {
            cache.insert(
                &fixture_hash(index),
                &weighted_lut(u16::try_from(index).unwrap(), weight),
            );
        }
        assert_eq!(cache.entries.len(), 3);
        assert_eq!(cache.bytes, weight * 3);

        // Touch the oldest so it is no longer the eviction victim.
        assert!(cache.get(&fixture_hash(0)).is_some());
        cache.insert(&fixture_hash(3), &weighted_lut(3, weight));
        assert!(cache.bytes <= cache.max_bytes, "retained {}", cache.bytes);
        assert!(cache.get(&fixture_hash(3)).is_some(), "the new entry stays");
        assert!(
            cache.get(&fixture_hash(0)).is_some(),
            "the touched entry stays"
        );
        assert!(
            cache.get(&fixture_hash(1)).is_none(),
            "the least recently used entry is evicted"
        );

        // Re-inserting the same hash replaces rather than double-counts.
        let before = cache.bytes;
        cache.insert(&fixture_hash(3), &weighted_lut(3, weight));
        assert_eq!(cache.bytes, before);

        // A single lattice larger than the whole budget is still served: the
        // head is never evicted, so a miss can never become permanent.
        let mut solo = ParseCache::new(64, 64);
        solo.insert(&fixture_hash(9), &weighted_lut(9, 4_096));
        assert_eq!(solo.entries.len(), 1);
        assert!(solo.get(&fixture_hash(9)).is_some());
    }

    #[test]
    fn parse_cache_caps_the_entry_count_for_tiny_lattices() {
        let mut cache = ParseCache::new(u64::MAX, 4);
        for index in 0..12_u32 {
            cache.insert(
                &fixture_hash(index),
                &weighted_lut(u16::try_from(index).unwrap(), 128),
            );
        }
        assert_eq!(cache.entries.len(), 4);
        assert!(
            cache.get(&fixture_hash(0)).is_none(),
            "the oldest entries are evicted"
        );
        assert!(
            cache.get(&fixture_hash(11)).is_some(),
            "the newest entry is resident"
        );
    }

    #[test]
    fn parse_cache_eviction_is_not_a_correctness_question() {
        // Evicting an entry only costs a re-parse of the same verified bytes.
        //
        // The fixture text is unique to this test: the parse cache is
        // process-wide, and dropping a hash another test relies on would make
        // the suite order-dependent.
        const EVICTION_CUBE: &str = "\
TITLE \"Eviction Probe\"
LUT_3D_SIZE 2
DOMAIN_MIN 0 0 0
DOMAIN_MAX 1 1 1
0 0 0
0.25 0 0
0 0.25 0
0.25 0.25 0
0 0 0.25
0.25 0 0.25
0 0.25 0.25
0.75 0.75 0.75
";
        let temporary = TempDirectory::new("lut-parse-cache-evict");
        let store = store_for(&temporary, "project.kinewright");
        let source = write_source(&temporary, "eviction.cube", EVICTION_CUBE);
        let asset = imported_asset(&store, &source, 1);
        let (warm, _) = LutLibrary::build(std::slice::from_ref(&asset), Some(&store));
        let first = Arc::clone(warm.get(LutAssetId(1)).expect("the fixture verifies"));

        // Drop the process cache entry the way eviction would.
        {
            let mut cache = PARSE_CACHE.lock().expect("the parse cache lock is healthy");
            if let Some(index) = cache
                .entries
                .iter()
                .position(|(key, _)| key == &asset.sha256)
            {
                let (_, evicted) = cache.entries.remove(index);
                cache.bytes = cache
                    .bytes
                    .saturating_sub(ParseCache::entry_bytes(&evicted));
            }
        }

        let (cold, statuses) = LutLibrary::build(std::slice::from_ref(&asset), Some(&store));
        assert_eq!(statuses[0].1.kind, LutAvailabilityKind::Verified);
        let second = cold.get(LutAssetId(1)).expect("the re-parse verifies");
        assert_eq!(first.size, second.size);
        assert_eq!(first.rgba, second.rgba);
        assert_eq!(
            first.domain_min.map(f32::to_bits),
            second.domain_min.map(f32::to_bits)
        );
        assert_eq!(
            first.domain_max.map(f32::to_bits),
            second.domain_max.map(f32::to_bits)
        );
    }

    #[test]
    fn library_refuses_a_store_file_that_grew_past_the_byte_cap() {
        let temporary = TempDirectory::new("lut-library-too-large");
        let store = store_for(&temporary, "project.kinewright");
        let source = write_source(&temporary, "sample.cube", SAMPLE_CUBE);
        let asset = imported_asset(&store, &source, 1);
        let stored = store.path_for(&asset.sha256).expect("a canonical digest");
        // Sparse: the point is that the cap is read from the metadata, so the
        // bytes are never faulted in.
        fs::OpenOptions::new()
            .write(true)
            .open(&stored)
            .expect("the store file should open")
            .set_len(LUT_MAX_FILE_BYTES + 1)
            .expect("the store file should extend");

        let (library, statuses) = LutLibrary::build(std::slice::from_ref(&asset), Some(&store));
        assert!(library.get(LutAssetId(1)).is_none());
        assert_eq!(statuses[0].1.kind, LutAvailabilityKind::Unreadable);
        let reason = statuses[0].1.reason.clone().unwrap();
        assert!(reason.starts_with("lut_file_too_large: "), "{reason}");
        assert!(
            reason.contains(&format!("observed={}", LUT_MAX_FILE_BYTES + 1)),
            "{reason}"
        );
        assert!(
            reason.contains(&format!("allowed={LUT_MAX_FILE_BYTES}")),
            "{reason}"
        );
    }

    #[test]
    fn library_blocks_a_hand_edited_record_with_a_metadata_mismatch() {
        let temporary = TempDirectory::new("lut-library-mismatch");
        let store = store_for(&temporary, "project.kinewright");
        let source = write_source(&temporary, "sample.cube", SAMPLE_CUBE);
        let mut asset = imported_asset(&store, &source, 1);
        asset.size = 33;

        let (library, statuses) = LutLibrary::build(std::slice::from_ref(&asset), Some(&store));
        assert!(library.get(LutAssetId(1)).is_none());
        // CC4 2.3's `changed` (metadata) row: the bytes were read and they
        // hash to the recorded content, so this is not `unreadable`.
        assert_eq!(statuses[0].1.kind, LutAvailabilityKind::Changed);
        assert_eq!(
            statuses[0].1.observed_sha256.as_deref(),
            Some(asset.sha256.as_str()),
            "the bytes still hash to the recorded content"
        );
        let reason = statuses[0].1.reason.clone().unwrap();
        assert!(
            reason.starts_with("lut_asset_metadata_mismatch: "),
            "{reason}"
        );
        assert!(reason.contains("at size"), "{reason}");
        assert!(reason.contains("observed=33"), "{reason}");
        assert!(reason.contains("allowed=2"), "{reason}");

        let lut = crate::lut::parse_cube_lut_typed(SAMPLE_CUBE).unwrap();
        assert_eq!(
            metadata_mismatch(&asset, &lut),
            Some(("size", "33".to_owned(), "2".to_owned()))
        );
        asset.size = 2;
        assert_eq!(metadata_mismatch(&asset, &lut), None);
        asset.domain_max_millionths = [2_000_000; 3];
        assert_eq!(
            metadata_mismatch(&asset, &lut),
            Some((
                "domain_max_millionths",
                "[2000000, 2000000, 2000000]".to_owned(),
                "[1000000, 1000000, 1000000]".to_owned()
            ))
        );
    }

    /// A one-clip document carrying one active `creative_look` bound to
    /// `asset`, so the CC4 §2.3 preflight has a node that could evaluate.
    fn look_gate_document(assets: Vec<LutAsset>) -> kinewright_core::Document {
        use kinewright_core::{
            AssetId, Clip, ClipContent, ClipId, Effect, EffectId, ParamValue, TimeCode, Title,
            Track, TrackId, TrackKind,
        };
        let clips = assets
            .iter()
            .enumerate()
            .map(|(index, asset)| {
                let mut look = Effect {
                    id: EffectId(1),
                    name: "creative_look".to_owned(),
                    parameters: BTreeMap::new(),
                    keyframes: BTreeMap::new(),
                };
                look.parameters.insert(
                    "lut_asset_id".to_owned(),
                    ParamValue::Integer(i64::try_from(asset.id.0).unwrap()),
                );
                Clip {
                    id: ClipId(u64::try_from(index).unwrap() + 1),
                    asset: AssetId::default(),
                    source_range: TimeCode(0)..TimeCode(4),
                    content: ClipContent::Title(Title {
                        text: "CC4".to_owned(),
                        ..Title::default()
                    }),
                    timeline_start: TimeCode::ZERO,
                    effects: vec![look],
                    transition_in: None,
                    link: None,
                    audio_gain_tenth_db: 0,
                    audio_fade_in_frames: TimeCode::ZERO,
                    audio_fade_out_frames: TimeCode::ZERO,
                    speed_percent: 100,
                }
            })
            .collect();
        kinewright_core::Document {
            resolution: (64, 36),
            duration: TimeCode(4),
            tracks: vec![Track {
                id: TrackId(1),
                kind: TrackKind::Video,
                sync_lock: true,
                clips,
            }],
            lut_assets: assets,
            ..kinewright_core::Document::default()
        }
    }

    #[test]
    fn availability_reports_a_metadata_mismatch_instead_of_deferring_it_to_the_render() {
        // CC4 §2.3: `verified` has to mean "this is what the render will
        // use". The library build has always refused a record that disagrees
        // with its hash-verified bytes; observing only the hash here made
        // every preflight - the export gate, the agent's status surface, the
        // inspector chip - report `verified` for a hand-edited `size` that
        // then failed at render time.
        let temporary = TempDirectory::new("lut-availability-mismatch");
        let store = store_for(&temporary, "project.kinewright");
        // A lattice unique to this test, so the first observation below is a
        // genuine cold parse rather than a process-parse-cache hit.
        let unique = SAMPLE_CUBE.replace("Sample Look", "Availability Mismatch Fixture");
        let source = write_source(&temporary, "unique.cube", &unique);
        let honest = imported_asset(&store, &source, 1);

        let mut edited = honest.clone();
        edited.size = 33;

        let cold = store.availability(&edited);
        assert_eq!(cold.kind, LutAvailabilityKind::Changed);
        // Not `unreadable`: the bytes were read and they hash correctly. The
        // project record is what disagrees.
        assert_eq!(
            cold.observed_sha256.as_deref(),
            Some(edited.sha256.as_str())
        );
        let reason = cold
            .reason
            .clone()
            .expect("a changed status carries a reason");
        assert!(
            reason.starts_with("lut_asset_metadata_mismatch: "),
            "{reason}"
        );
        assert!(reason.contains("at size"), "{reason}");
        assert!(reason.contains("observed=33"), "{reason}");
        assert!(reason.contains("allowed=2"), "{reason}");
        // The recovery has to be honest: `restore` only accepts a candidate
        // that hashes to the recorded content, and this file already does, so
        // restoring it would change nothing.
        assert!(reason.contains("re-import"), "{reason}");
        assert!(!reason.contains("restore the"), "{reason}");

        // Warm and cold agree, and so does the resolver the preflight takes.
        assert_eq!(store.availability(&edited), cold);
        assert_eq!(store.availability_resolver()(&edited), cold);
        assert_eq!(
            store.availability(&honest).kind,
            LutAvailabilityKind::Verified
        );

        // A hand-edited domain is caught the same way, and so is a built-in:
        // the bake is generated in this binary, but the record still has to
        // agree with it.
        let mut domain = honest.clone();
        domain.domain_max_millionths = [2_000_000; 3];
        assert_eq!(
            store.availability(&domain).kind,
            LutAvailabilityKind::Changed
        );
        let mut builtin = BuiltinLook::Warm.to_lut_asset(LutAssetId(2));
        builtin.size = builtin.size.saturating_add(1);
        let builtin_status = store.availability(&builtin);
        assert_eq!(builtin_status.kind, LutAvailabilityKind::Changed);
        assert!(
            builtin_status
                .reason
                .as_deref()
                .is_some_and(|reason| reason.starts_with("lut_asset_metadata_mismatch: ")),
            "{builtin_status:?}"
        );

        // End to end through the gate an export actually passes.
        let ready = kinewright_core::export_lut_preflight_with(
            &look_gate_document(vec![honest.clone()]),
            &store.availability_resolver(),
        );
        assert!(ready.export_ready(), "{ready:?}");
        assert_eq!(ready.checked_lut_assets, vec![LutAssetId(1)]);

        let blocked = kinewright_core::export_lut_preflight_with(
            &look_gate_document(vec![edited.clone(), builtin]),
            &store.availability_resolver(),
        );
        assert!(!blocked.export_ready());
        assert_eq!(blocked.issues.len(), 2);
        for issue in &blocked.issues {
            assert_eq!(issue.kind, LutAvailabilityKind::Changed);
            assert!(
                issue
                    .reason
                    .as_deref()
                    .is_some_and(|reason| reason.starts_with("lut_asset_metadata_mismatch: ")),
                "{issue:?}"
            );
        }
        // The library build and the preflight now agree about the same
        // record, which is the whole point.
        let (library, statuses) = LutLibrary::build(std::slice::from_ref(&edited), Some(&store));
        assert!(library.get(LutAssetId(1)).is_none());
        assert_eq!(statuses[0].1.kind, LutAvailabilityKind::Changed);
    }

    #[test]
    fn the_dedup_compare_never_reads_an_oversized_store_entry() {
        // The dedup read is a plain `fs::read` of a file in a directory the
        // user can write to, so it takes the same [`LUT_MAX_FILE_BYTES`] cap
        // every other store read takes. An entry past the cap is simply not a
        // dedup candidate: it cannot be the content its own name claims, and
        // the incoming bytes have already been hashed.
        let temporary = TempDirectory::new("lut-store-dedup-cap");
        let store = store_for(&temporary, "project.kinewright");
        let source = write_source(&temporary, "sample.cube", SAMPLE_CUBE);
        let asset = imported_asset(&store, &source, 1);
        let stored = store.path_for(&asset.sha256).expect("a canonical digest");

        // Sparse, so the test costs no disk: the cap is read from the
        // metadata and the bytes are never faulted in.
        fs::OpenOptions::new()
            .write(true)
            .open(&stored)
            .expect("the store file should open")
            .set_len(LUT_MAX_FILE_BYTES + 1)
            .expect("the store file should extend");
        // Hashing streams, so the oversized entry is observed as `changed`:
        // it no longer carries the bytes its content-addressed name claims.
        assert_eq!(
            store.availability(&asset).kind,
            LutAvailabilityKind::Changed
        );

        // Restore writes through `write_store_file`, so this is the dedup
        // comparison being asked about an oversized entry.
        let restored = store
            .restore(&asset, &source)
            .expect("an oversized entry is replaced, not compared");
        assert_eq!(restored, stored);
        assert_eq!(
            fs::metadata(&stored).expect("the store file exists").len(),
            SAMPLE_CUBE.len() as u64,
            "the oversized entry must have been overwritten"
        );
        assert_eq!(sha256_bytes(&fs::read(&stored).unwrap()), asset.sha256);
        assert_eq!(
            store.availability(&asset).kind,
            LutAvailabilityKind::Verified
        );

        // A correctly hashed entry within the cap is still left untouched.
        let before = fs::metadata(&stored)
            .and_then(|metadata| metadata.modified())
            .expect("the store file has a modification time");
        store
            .restore(&asset, &source)
            .expect("restoring identical bytes succeeds");
        let after = fs::metadata(&stored)
            .and_then(|metadata| metadata.modified())
            .expect("the store file has a modification time");
        assert_eq!(before, after, "an idempotent restore rewrites nothing");
    }

    /// Create a symlink, or report that this platform cannot.
    fn create_symlink(original: &Path, link: &Path) -> std::io::Result<()> {
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(original, link)
        }
        #[cfg(windows)]
        {
            if original.is_dir() {
                std::os::windows::fs::symlink_dir(original, link)
            } else {
                std::os::windows::fs::symlink_file(original, link)
            }
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = (original, link);
            Err(std::io::Error::other("symlinks are unsupported here"))
        }
    }

    #[test]
    fn a_symlinked_store_file_is_missing_and_refuses_a_write() {
        let temporary = TempDirectory::new("lut-store-symlink-file");
        let store = store_for(&temporary, "project.kinewright");
        let source = write_source(&temporary, "sample.cube", SAMPLE_CUBE);
        let asset = imported_asset(&store, &source, 1);
        let stored = store.path_for(&asset.sha256).unwrap();
        fs::remove_file(&stored).unwrap();
        if create_symlink(&source, &stored).is_err() {
            println!("SKIPPED: this platform cannot create symlinks");
            return;
        }

        // The link target carries exactly the right bytes, and the asset is
        // still not verified: the store owns its files, it does not follow
        // links out of the project directory.
        let status = store.availability(&asset);
        assert_eq!(status.kind, LutAvailabilityKind::Missing);
        assert!(
            status
                .reason
                .unwrap()
                .contains("the store path is not a regular file")
        );
        let (library, statuses) = LutLibrary::build(std::slice::from_ref(&asset), Some(&store));
        assert!(library.is_empty());
        assert_eq!(statuses[0].1.kind, LutAvailabilityKind::Missing);

        let MediaError::Backend(message) = store.restore(&asset, &source).unwrap_err() else {
            panic!("store failures cross as MediaError::Backend");
        };
        assert!(
            message.starts_with("lut_store_root_invalid: "),
            "a symlinked store entry must refuse the write: {message}"
        );
        assert!(message.contains("the store file is a symlink"), "{message}");
    }

    #[test]
    fn a_symlinked_store_directory_refuses_every_write() {
        let temporary = TempDirectory::new("lut-store-symlink-dir");
        let store = store_for(&temporary, "project.kinewright");
        let outside = temporary.path("outside");
        fs::create_dir_all(&outside).unwrap();
        fs::create_dir_all(store.root()).unwrap();
        if create_symlink(&outside, &store.luts_dir()).is_err() {
            println!("SKIPPED: this platform cannot create symlinks");
            return;
        }
        let source = write_source(&temporary, "sample.cube", SAMPLE_CUBE);
        let MediaError::Backend(message) = store.import_lut_asset(&source).unwrap_err() else {
            panic!("store failures cross as MediaError::Backend");
        };
        assert!(
            message.starts_with("lut_store_root_invalid: "),
            "a symlinked luts directory must refuse the write: {message}"
        );
        assert!(
            message.contains("the store directory is a symlink"),
            "{message}"
        );
        assert_eq!(
            fs::read_dir(&outside).unwrap().count(),
            0,
            "nothing may be written through the link"
        );
    }

    #[test]
    fn for_project_refuses_a_root_that_is_not_a_directory() {
        let temporary = TempDirectory::new("lut-store-root-file");
        let project = temporary.path("project.kinewright");
        fs::write(
            temporary.path("project.kinewright-assets"),
            b"not a directory",
        )
        .unwrap();
        let MediaError::Backend(message) = LutStore::for_project(&project).unwrap_err() else {
            panic!("store failures cross as MediaError::Backend");
        };
        assert!(
            message.starts_with("lut_store_root_invalid: "),
            "message should lead with the code: {message}"
        );
        assert!(
            message.contains("exists and is not a directory"),
            "{message}"
        );
    }

    #[test]
    fn the_store_root_uses_the_stem_whatever_the_extension() {
        let temporary = TempDirectory::new("lut-store-stem");
        let from_project = store_for(&temporary, "edit.kinewright");
        let from_json = store_for(&temporary, "edit.json");
        assert_eq!(from_project.root(), from_json.root());
        assert_eq!(
            from_project.root(),
            temporary.path("edit.kinewright-assets")
        );
    }

    #[test]
    fn import_and_restore_refuse_a_file_over_the_size_cap() {
        let temporary = TempDirectory::new("lut-store-too-large");
        let store = store_for(&temporary, "project.kinewright");
        let source = write_source(&temporary, "sample.cube", SAMPLE_CUBE);
        let asset = imported_asset(&store, &source, 1);

        // A sparse file: the metadata length is checked before any read, so
        // the rejection cannot be a parse failure in disguise.
        let huge = temporary.path("huge.cube");
        fs::File::create(&huge)
            .unwrap()
            .set_len(LUT_MAX_FILE_BYTES + 1)
            .unwrap();

        for result in [
            store.import_lut_asset(&huge).map(|_| ()),
            store.restore(&asset, &huge).map(|_| ()),
        ] {
            let MediaError::Backend(message) = result.unwrap_err() else {
                panic!("store failures cross as MediaError::Backend");
            };
            assert!(
                message.starts_with("lut_file_too_large: "),
                "message should lead with the code: {message}"
            );
            assert!(
                message.contains(&format!("observed={}", LUT_MAX_FILE_BYTES + 1)),
                "{message}"
            );
            assert!(
                message.contains(&format!("allowed={LUT_MAX_FILE_BYTES}")),
                "{message}"
            );
        }
        assert_eq!(
            fs::read(store.path_for(&asset.sha256).unwrap()).unwrap(),
            SAMPLE_CUBE.as_bytes(),
            "a refused oversized file leaves the store alone"
        );
        assert_eq!(LUT_MAX_FILE_BYTES, 16 * 1024 * 1024);
    }

    #[test]
    fn a_sixty_five_cubed_import_completes_and_is_timed() {
        let size = 65_u32;
        let mut source = format!("TITLE \"Vendor 65\"\nLUT_3D_SIZE {size}\n");
        let last = f64::from(size - 1);
        for blue in 0..size {
            for green in 0..size {
                for red in 0..size {
                    let _ = writeln!(
                        source,
                        "{:.6} {:.6} {:.6}",
                        f64::from(red) / last,
                        f64::from(green) / last,
                        f64::from(blue) / last
                    );
                }
            }
        }
        let temporary = TempDirectory::new("lut-store-65");
        let store = store_for(&temporary, "project.kinewright");
        let path = write_source(&temporary, "vendor65.cube", &source);

        let started = Instant::now();
        let import = store.import_lut_asset(&path).unwrap();
        let elapsed = started.elapsed();
        println!(
            "65^3 import: {} bytes in {:.1} ms",
            import.byte_len,
            elapsed.as_secs_f64() * 1000.0
        );
        assert_eq!(import.size, 65);
        assert_eq!(import.title, "Vendor 65");
        assert_eq!(import.byte_len, source.len() as u64);
        assert_eq!(
            fs::read(store.path_for(&import.sha256).unwrap()).unwrap(),
            source.as_bytes()
        );
    }
}
