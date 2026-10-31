//! One immutable image per qualified executable while this host has native work.
use std::{
    collections::BTreeMap,
    fs::File,
    io,
    os::fd::AsRawFd,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
#[derive(Default)]
pub(crate) struct ExecutableImages(Mutex<BTreeMap<String, Arc<File>>>);
impl ExecutableImages {
    pub fn get(&self, path: &Path, digest: &str) -> io::Result<Arc<File>> {
        let mut images = self
            .0
            .lock()
            .map_err(|_| io::Error::other("image cache poisoned"))?;
        let key = digest.to_owned();
        if let Some(image) = images.get(&key) {
            return Ok(image.clone());
        }
        let image = Arc::new(kinewright_job_sys::seal_executable(path, digest)?);
        images.insert(key, image.clone());
        Ok(image)
    }
    pub fn clear(&self) {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }
}
pub(crate) fn source(image: &File) -> PathBuf {
    format!("/proc/{}/fd/{}", std::process::id(), image.as_raw_fd()).into()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn s4b_c5_fix1_images_share_and_release_when_idle() {
        let images = ExecutableImages::default();
        let measure = std::env::var_os("KINEWRIGHT_C5_IMAGE_MEASURE").map(PathBuf::from);
        let path = measure.as_deref().unwrap_or(Path::new("/usr/bin/true"));
        let rss = || {
            std::fs::read_to_string("/proc/self/status")
                .unwrap()
                .lines()
                .find_map(|l| l.strip_prefix("VmRSS:").map(str::to_owned))
                .unwrap()
        };
        let before = rss();
        let digest = kinewright_media::sha256_file(path).unwrap();
        let first = images.get(path, &digest).unwrap();
        let second = images.get(path, &digest).unwrap();
        let aliases = kinewright_media::test_support::TempDirectory::new_in(
            &kinewright_media::test_support::workspace_target_dir(),
            "image-alias",
        );
        let alias = aliases.path("other-install");
        std::os::unix::fs::symlink(path, &alias).unwrap();
        let third = images.get(&alias, &digest).unwrap();
        if measure.is_some() {
            use std::os::unix::fs::MetadataExt;
            eprintln!(
                "C5_FIX1_IMAGE_MEMORY source_bytes={} shmem_allocated_bytes={} host_rss_before={} host_rss_after={} images=1 references=3",
                first.metadata().unwrap().len(),
                first.metadata().unwrap().blocks() * 512,
                before.trim(),
                rss().trim()
            );
        }
        assert!(Arc::ptr_eq(&first, &second) && Arc::ptr_eq(&first, &third));
        let weak = Arc::downgrade(&first);
        drop((first, second, third));
        assert!(weak.upgrade().is_some());
        images.clear();
        assert!(weak.upgrade().is_none(), "C5_FIX1_IDLE_IMAGE_DROPPED");
    }
}
