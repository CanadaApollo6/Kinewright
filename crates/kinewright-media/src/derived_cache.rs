use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    sync::{Arc, Mutex, RwLock},
    thread,
    time::SystemTime,
};

use crossbeam_channel::Receiver;
use kinewright_core::{ExportCancellation, MediaError};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::sha256::sha256_file;

pub(crate) fn cache_root(data_dir: &Path, family: &str, version: u32) -> PathBuf {
    data_dir.join(family).join(format!("v{version}"))
}

pub(crate) fn spawn_worker<J, F>(
    thread_name: &str,
    worker_label: &str,
    jobs: Receiver<J>,
    mut process: F,
) -> Result<(), MediaError>
where
    J: Send + 'static,
    F: FnMut(J) -> bool + Send + 'static,
{
    thread::Builder::new()
        .name(thread_name.to_owned())
        .spawn(move || {
            while let Ok(job) = jobs.recv() {
                if !process(job) {
                    break;
                }
            }
        })
        .map(|_| ())
        .map_err(|error| {
            MediaError::Backend(format!("could not start {worker_label} worker: {error}"))
        })
}

/// Derived-data state keyed by the asset's PATH, never its id: asset ids are
/// per-document, so with several projects open the same id can name
/// different files. Content identity is what derived data belongs to.
pub(crate) struct StatusReporter<S> {
    states: Arc<RwLock<HashMap<PathBuf, S>>>,
}

impl<S> Clone for StatusReporter<S> {
    fn clone(&self) -> Self {
        Self {
            states: Arc::clone(&self.states),
        }
    }
}

impl<S> StatusReporter<S> {
    pub(crate) fn new() -> Self {
        Self {
            states: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub(crate) fn update(&self, path: &Path, status: S) {
        if let Ok(mut states) = self.states.write() {
            states.insert(path.to_path_buf(), status);
        }
    }

    pub(crate) fn should_queue(&self, path: &Path, blocks_queue: impl FnOnce(&S) -> bool) -> bool {
        self.states.read().map_or(true, |states| {
            states.get(path).is_none_or(|status| !blocks_queue(status))
        })
    }
}

impl<S: Clone> StatusReporter<S> {
    pub(crate) fn get_or(&self, path: &Path, default: S) -> S {
        self.states
            .read()
            .ok()
            .and_then(|states| states.get(path).cloned())
            .unwrap_or(default)
    }
}

/// Cooperative cancellation tokens for one analysis family, keyed by media path.
#[derive(Clone, Default)]
pub(crate) struct CancellationRegistry {
    active: Arc<Mutex<HashMap<PathBuf, ExportCancellation>>>,
}

impl CancellationRegistry {
    /// Start one job unless the same asset is already queued or running.
    pub(crate) fn start(&self, path: &Path) -> Option<ExportCancellation> {
        let mut active = self.active.lock().ok()?;
        if active.contains_key(path) {
            return None;
        }
        let cancellation = ExportCancellation::default();
        active.insert(path.to_path_buf(), cancellation.clone());
        Some(cancellation)
    }

    pub(crate) fn cancel(&self, path: &Path) -> bool {
        self.active.lock().is_ok_and(|active| {
            active.get(path).is_some_and(|cancellation| {
                cancellation.cancel();
                true
            })
        })
    }

    pub(crate) fn finish(&self, path: &Path, cancellation: &ExportCancellation) {
        if let Ok(mut active) = self.active.lock()
            && active
                .get(path)
                .is_some_and(|current| current == cancellation)
        {
            active.remove(path);
        }
    }
}

/// Content hashes are deliberately recomputed for every request. File size
/// and modification time are useful diagnostics but are not a source identity:
/// a same-size replacement can preserve both values on some filesystems.
#[derive(Default)]
pub(crate) struct ContentHashes;

impl ContentHashes {
    #[allow(clippy::unused_self)]
    pub(crate) fn get(&mut self, path: &Path) -> Result<String, MediaError> {
        sha256_file(path)
    }
}

/// Counts regular files below one cache family without following symlinks.
/// Cache roots are application-owned, so a symlink is ignored rather than
/// traversed or deleted outside that root.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct CacheStats {
    pub(crate) file_count: u64,
    pub(crate) bytes: u64,
}

pub(crate) fn inventory_cache_root(root: &Path) -> Result<CacheStats, MediaError> {
    let metadata = match fs::symlink_metadata(root) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(CacheStats::default());
        }
        Err(error) => return Err(cache_error("inspect cache", &error)),
    };
    if metadata.file_type().is_symlink() {
        return Err(MediaError::Backend(format!(
            "refusing to inspect cache root symlink {}",
            root.display()
        )));
    }
    if !metadata.is_dir() {
        return Err(MediaError::Backend(format!(
            "cache root is not a directory: {}",
            root.display()
        )));
    }
    inventory_cache_dir(root)
}

fn inventory_cache_dir(root: &Path) -> Result<CacheStats, MediaError> {
    let mut stats = CacheStats::default();
    for entry in fs::read_dir(root).map_err(|error| cache_error("scan cache", &error))? {
        let entry = entry.map_err(|error| cache_error("read cache entry", &error))?;
        let file_type = entry
            .file_type()
            .map_err(|error| cache_error("inspect cache entry", &error))?;
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            let child = inventory_cache_dir(&entry.path())?;
            stats.file_count = stats.file_count.saturating_add(child.file_count);
            stats.bytes = stats.bytes.saturating_add(child.bytes);
        } else if file_type.is_file() {
            let bytes = entry
                .metadata()
                .map_err(|error| cache_error("inspect cache file", &error))?
                .len();
            stats.file_count = stats.file_count.saturating_add(1);
            stats.bytes = stats.bytes.saturating_add(bytes);
        }
    }
    Ok(stats)
}

/// Remove only regular files below the supplied application-owned cache root.
/// Symlinks are deliberately left untouched and never followed. Empty cache
/// subdirectories are removed, while the family root itself is retained so a
/// subsequent worker can recreate files without changing its ownership shape.
pub(crate) fn clear_cache_root(root: &Path) -> Result<CacheStats, MediaError> {
    let metadata = match fs::symlink_metadata(root) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(CacheStats::default());
        }
        Err(error) => return Err(cache_error("inspect cache", &error)),
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(MediaError::Backend(format!(
            "refusing to clear non-directory cache root {}",
            root.display()
        )));
    }
    clear_cache_dir(root)
}

fn clear_cache_dir(root: &Path) -> Result<CacheStats, MediaError> {
    let mut stats = CacheStats::default();
    for entry in fs::read_dir(root).map_err(|error| cache_error("scan cache", &error))? {
        let entry = entry.map_err(|error| cache_error("read cache entry", &error))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|error| cache_error("inspect cache entry", &error))?;
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            let child = clear_cache_dir(&path)?;
            stats.file_count = stats.file_count.saturating_add(child.file_count);
            stats.bytes = stats.bytes.saturating_add(child.bytes);
            let _ = fs::remove_dir(&path);
        } else if file_type.is_file() {
            let bytes = entry
                .metadata()
                .map_err(|error| cache_error("inspect cache file", &error))?
                .len();
            fs::remove_file(&path).map_err(|error| cache_error("clear cache file", &error))?;
            stats.file_count = stats.file_count.saturating_add(1);
            stats.bytes = stats.bytes.saturating_add(bytes);
        }
    }
    Ok(stats)
}

#[derive(Clone, Copy)]
pub(crate) enum TempFileStyle {
    AppendToExtension,
    ReplaceExtension,
}

#[derive(Serialize, Deserialize)]
struct Versioned<S> {
    version: u32,
    #[serde(flatten)]
    stored: S,
}

pub(crate) struct JsonCache {
    root: PathBuf,
    version: u32,
    label: &'static str,
    write_label: &'static str,
    temp_file_style: TempFileStyle,
}

impl JsonCache {
    pub(crate) fn new(root: PathBuf, version: u32, label: &'static str) -> Self {
        Self {
            root,
            version,
            label,
            write_label: label,
            temp_file_style: TempFileStyle::AppendToExtension,
        }
    }

    pub(crate) fn with_write_options(
        mut self,
        write_label: &'static str,
        temp_file_style: TempFileStyle,
    ) -> Self {
        self.write_label = write_label;
        self.temp_file_style = temp_file_style;
        self
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    pub(crate) fn path_for(&self, content_sha256: &str, config_key: &str) -> PathBuf {
        cache_path(&self.root, content_sha256, config_key, "json")
    }

    pub(crate) fn load<S, P>(
        &self,
        path: &Path,
        restore: impl FnOnce(S) -> Option<P>,
    ) -> Result<Option<P>, MediaError>
    where
        S: DeserializeOwned,
    {
        let Some(bytes) = read_cache(path, self.label)? else {
            return Ok(None);
        };
        let Ok(entry) = serde_json::from_slice::<Versioned<S>>(&bytes) else {
            return Ok(None);
        };
        if entry.version != self.version {
            return Ok(None);
        }
        Ok(restore(entry.stored))
    }

    pub(crate) fn save<S: Serialize>(&self, path: &Path, stored: &S) -> Result<(), MediaError> {
        create_cache_dir(&self.root, self.label)?;
        let bytes = serde_json::to_vec(&Versioned {
            version: self.version,
            stored,
        })
        .map_err(|error| {
            MediaError::Backend(format!("could not encode {} cache: {error}", self.label))
        })?;
        atomic_write(path, &bytes, self.write_label, self.temp_file_style)
    }
}

pub(crate) fn cache_path(
    root: &Path,
    content_sha256: &str,
    config_key: &str,
    extension: &str,
) -> PathBuf {
    root.join(format!("{content_sha256}{config_key}.{extension}"))
}

pub(crate) fn read_cache(path: &Path, label: &str) -> Result<Option<Vec<u8>>, MediaError> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(cache_error(&format!("read {label}"), &error)),
    }
}

pub(crate) fn create_cache_dir(root: &Path, label: &str) -> Result<(), MediaError> {
    fs::create_dir_all(root).map_err(|error| cache_error(&format!("create {label}"), &error))
}

pub(crate) fn atomic_write(
    path: &Path,
    bytes: &[u8],
    label: &str,
    style: TempFileStyle,
) -> Result<(), MediaError> {
    static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let extension = match style {
        TempFileStyle::AppendToExtension => path.extension().map_or_else(
            || format!("tmp-{}-{sequence}", std::process::id()),
            |extension| {
                format!(
                    "{}.tmp-{}-{sequence}",
                    extension.to_string_lossy(),
                    std::process::id()
                )
            },
        ),
        TempFileStyle::ReplaceExtension => format!("tmp-{}-{sequence}", std::process::id()),
    };
    let temporary = path.with_extension(extension);
    fs::write(&temporary, bytes).map_err(|error| cache_error(&format!("write {label}"), &error))?;
    if path.exists() {
        fs::remove_file(path).map_err(|error| cache_error(&format!("replace {label}"), &error))?;
    }
    fs::rename(&temporary, path).map_err(|error| cache_error(&format!("commit {label}"), &error))
}

pub(crate) fn trim_cache(
    root: &Path,
    maximum_files: usize,
    maximum_bytes: u64,
) -> Result<(), MediaError> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(cache_error("scan visual asset", &error)),
    };
    let mut files = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let metadata = entry.metadata().ok()?;
            metadata.is_file().then(|| {
                (
                    entry.path(),
                    metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                    metadata.len(),
                )
            })
        })
        .collect::<Vec<_>>();
    files.sort_by_key(|(_, modified, _)| *modified);
    let mut total_bytes = files.iter().map(|(_, _, bytes)| *bytes).sum::<u64>();
    let mut file_count = files.len();
    for (path, _, bytes) in files {
        if file_count <= maximum_files && total_bytes <= maximum_bytes {
            break;
        }
        if fs::remove_file(path).is_ok() {
            file_count = file_count.saturating_sub(1);
            total_bytes = total_bytes.saturating_sub(bytes);
        }
    }
    Ok(())
}

fn cache_error(action: &str, error: &std::io::Error) -> MediaError {
    MediaError::Backend(format!("could not {action} cache: {error}"))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::sha256::sha256_file;
    use crate::test_support::TempDirectory;

    #[test]
    fn content_hash_cache_invalidates_when_file_size_changes() {
        let directory = TempDirectory::new("content-hash-stale");
        let path = directory.path("source.bin");
        fs::write(&path, b"first").unwrap();
        let mut hashes = ContentHashes;
        let first = hashes.get(&path).unwrap();
        assert_eq!(first, sha256_file(&path).unwrap());

        fs::write(&path, b"replacement-with-a-different-size").unwrap();
        let second = hashes.get(&path).unwrap();
        assert_ne!(first, second);
        assert_eq!(second, sha256_file(&path).unwrap());
    }

    #[test]
    fn content_hashes_do_not_trust_same_size_same_mtime_replacement() {
        let directory = TempDirectory::new("content-hash-same-size");
        let path = directory.path("source.bin");
        fs::write(&path, b"first").unwrap();
        let original_modified = fs::metadata(&path).unwrap().modified().unwrap();
        let mut hashes = ContentHashes;
        let first = hashes.get(&path).unwrap();

        // Keep both metadata fields identical so a size+mtime memoization scheme
        // would incorrectly return `first`. The handle that rewinds the
        // modification time is opened for writing because Windows backs
        // `set_modified` with `SetFileTime`, which needs `FILE_WRITE_ATTRIBUTES`
        // and denies access on a read-only handle; unix `futimens` does not care.
        fs::write(&path, b"other").unwrap();
        fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(original_modified)
            .unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().modified().unwrap(),
            original_modified
        );

        let second = hashes.get(&path).unwrap();
        assert_ne!(first, second);
        assert_eq!(second, sha256_file(&path).unwrap());
    }

    #[test]
    fn cache_inventory_and_clear_are_scoped_and_idempotent() {
        let directory = TempDirectory::new("cache-scope");
        let root = directory.path("visual-assets/v1");
        fs::create_dir_all(root.join("thumbnails")).unwrap();
        fs::write(root.join("waveform.json"), b"1234").unwrap();
        fs::write(root.join("thumbnails/frame.rgba"), b"567890").unwrap();
        let outside = directory.path("source.mp4");
        fs::write(&outside, b"source").unwrap();

        assert_eq!(
            inventory_cache_root(&root).unwrap(),
            CacheStats {
                file_count: 2,
                bytes: 10,
            }
        );
        assert_eq!(
            clear_cache_root(&root).unwrap(),
            CacheStats {
                file_count: 2,
                bytes: 10,
            }
        );
        assert_eq!(inventory_cache_root(&root).unwrap(), CacheStats::default());
        assert_eq!(clear_cache_root(&root).unwrap(), CacheStats::default());
        assert_eq!(fs::read(&outside).unwrap(), b"source");
        assert!(root.is_dir());
    }

    #[test]
    fn missing_cache_root_is_an_empty_inventory_and_clear() {
        let directory = TempDirectory::new("cache-missing");
        let root = directory.path("does-not-exist/v1");
        assert_eq!(inventory_cache_root(&root).unwrap(), CacheStats::default());
        assert_eq!(clear_cache_root(&root).unwrap(), CacheStats::default());
    }
}
