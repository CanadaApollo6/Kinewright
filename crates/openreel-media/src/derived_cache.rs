use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
    thread,
    time::SystemTime,
};

use crossbeam_channel::Receiver;
use openreel_core::{AssetId, MediaError};
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

pub(crate) struct StatusReporter<S> {
    states: Arc<RwLock<HashMap<AssetId, S>>>,
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

    pub(crate) fn update(&self, asset: AssetId, status: S) {
        if let Ok(mut states) = self.states.write() {
            states.insert(asset, status);
        }
    }

    pub(crate) fn should_queue(
        &self,
        asset: AssetId,
        blocks_queue: impl FnOnce(&S) -> bool,
    ) -> bool {
        self.states.read().map_or(true, |states| {
            states
                .get(&asset)
                .is_none_or(|status| !blocks_queue(status))
        })
    }
}

impl<S: Clone> StatusReporter<S> {
    pub(crate) fn get_or(&self, asset: AssetId, default: S) -> S {
        self.states
            .read()
            .ok()
            .and_then(|states| states.get(&asset).cloned())
            .unwrap_or(default)
    }
}

#[derive(Default)]
pub(crate) struct ContentHashes {
    hashes: HashMap<PathBuf, String>,
}

impl ContentHashes {
    pub(crate) fn get(&mut self, path: &Path) -> Result<String, MediaError> {
        if let Some(hash) = self.hashes.get(path) {
            return Ok(hash.clone());
        }
        let hash = sha256_file(path)?;
        self.hashes.insert(path.to_path_buf(), hash.clone());
        Ok(hash)
    }
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
    let extension = match style {
        TempFileStyle::AppendToExtension => path.extension().map_or_else(
            || format!("tmp-{}", std::process::id()),
            |extension| format!("{}.tmp-{}", extension.to_string_lossy(), std::process::id()),
        ),
        TempFileStyle::ReplaceExtension => format!("tmp-{}", std::process::id()),
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
