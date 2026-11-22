//! JA-1 S6 C2: the one background save of a sealed cut.
//!
//! `SaveSealed` admits the folder and pins the sealed records under the
//! writer lock, starts this worker and answers at once. The copy, hash and
//! fsyncs run on the worker with neither the `/host` ingress permit nor the
//! writer lock held; the worker takes the lock once, at the end, to report.
#[cfg(test)]
use std::sync::Arc;
use std::{
    io,
    path::{Path, PathBuf},
    sync::{Mutex, Weak},
    thread::{self, JoinHandle},
};

use kinewright_core::{ExportCancellation, output_review::CoverageProfileId};
use kinewright_project::{
    bundle::BundleError,
    bundle_records::ProjectId,
    review_job::delivery::{SealedSaveStatus, admit_save_folder, sealed_cut_name},
    studio_records::SnapshotKey,
};

use super::{HostCommandOwner, HostError, join_other, refused};

/// A test hook each copied chunk passes through, with the lock released.
#[cfg(test)]
pub(in crate::writer_host) type SaveHook =
    Arc<dyn Fn(u64, &mut [u8]) -> io::Result<()> + Send + Sync>;

pub(in crate::writer_host) const SAVE_RUNNING: &str =
    "A sealed cut is already being saved; wait for it to finish or stop it first.";
pub(in crate::writer_host) const NO_SAVE: &str = "No sealed cut is being saved.";

pub(in crate::writer_host) struct SaveWorker {
    project: ProjectId,
    generation: u64,
    cancellation: ExportCancellation,
    thread: Option<JoinHandle<()>>,
}

impl SaveWorker {
    pub(super) fn project(&self) -> &ProjectId {
        &self.project
    }

    pub(super) fn running(&self) -> bool {
        self.thread
            .as_ref()
            .is_some_and(|thread| !thread.is_finished())
    }

    pub(super) fn cancel(&self) {
        self.cancellation.cancel();
    }

    pub(super) fn take_thread(&mut self) -> Option<JoinHandle<()>> {
        self.thread.take()
    }
}

/// Plain words for a failed save.
fn failure(error: &BundleError) -> String {
    match error {
        BundleError::Identity(message) => message.clone(),
        BundleError::Capacity(message) => format!("The cut was not saved: {message}."),
        BundleError::Unsettled => {
            "The project is still settling its last change; try the save again.".into()
        }
        BundleError::Io(error) if error.kind() == io::ErrorKind::StorageFull => {
            "The chosen folder ran out of space; nothing was saved.".into()
        }
        other => format!("The cut was not saved: {other}"),
    }
}

impl HostCommandOwner {
    /// Admit `folder`, pin the sealed cut and start the save worker.
    pub(super) fn start_sealed_save(
        &mut self,
        host: Weak<Mutex<Self>>,
        snapshot: &SnapshotKey,
        coverage: CoverageProfileId,
        key_sha256: &str,
        folder: &Path,
    ) -> Result<(), HostError> {
        if self
            .review_runtime
            .save
            .as_ref()
            .is_some_and(SaveWorker::running)
        {
            return Err(refused(SAVE_RUNNING));
        }
        // A finished worker has reported; join it before the next one.
        if let Some(thread) = self
            .review_runtime
            .save
            .take()
            .and_then(|mut worker| worker.take_thread())
        {
            join_other(thread);
        }
        let owner = self.owner()?;
        let bundle = owner.review_bundle()?;
        // The save belongs to the open project, which a change of project
        // leaves by its id. A version inherited by Save As still names the
        // source in its snapshot, so the snapshot's project is not the key.
        let open_project = bundle.project_id().clone();
        let owned_roots = bundle.owned_roots()?;
        let association = owner.association().map(|a| a.path().to_path_buf());
        let mut refused_roots: Vec<&Path> = owned_roots.iter().map(PathBuf::as_path).collect();
        if let Some(bundle) = &association {
            refused_roots.push(bundle);
        }
        if let Some(data) = &self.config.data_dir {
            refused_roots.push(data);
        }
        let destination = admit_save_folder(folder, &refused_roots)?;
        let cut = bundle.pin_sealed_cut(snapshot, coverage, key_sha256)?;
        let project = association
            .as_deref()
            .and_then(Path::file_stem)
            .and_then(|stem| stem.to_str())
            .unwrap_or("project");
        let name = sealed_cut_name(project, &snapshot.version, &cut.receipt.outcome);
        if destination.holds(&name)? {
            return Err(refused(
                kinewright_project::review_job::delivery::NAME_TAKEN,
            ));
        }
        let cancellation = ExportCancellation::default();
        let project = open_project;
        let generation = self.review_runtime.save_generation.wrapping_add(1);
        self.review_runtime.save_generation = generation;
        let worker_project = project.clone();
        #[cfg(test)]
        let hook = self.review_runtime.save_hook.clone();
        let worker_cancel = cancellation.clone();
        let worker_name = name.clone();
        let thread = thread::Builder::new()
            .name("kinewright-sealed-save".into())
            .spawn(move || {
                let mut observe = |copied: u64, chunk: &mut [u8]| -> io::Result<()> {
                    #[cfg(test)]
                    if let Some(hook) = &hook {
                        return hook(copied, chunk);
                    }
                    #[cfg(not(test))]
                    let _ = (copied, chunk);
                    Ok(())
                };
                let result = cut.save(&destination, &worker_name, &worker_cancel, &mut observe);
                // The pins go before the report.
                drop(cut);
                let status = match result {
                    Ok(path) => SealedSaveStatus::Saved { path },
                    Err(error) => SealedSaveStatus::Failed {
                        reason: failure(&error),
                    },
                };
                if let Some(host) = host.upgrade()
                    && let Ok(mut owner) = host.lock()
                {
                    // Only while this is still the host's save: a change of
                    // project retired it, and its status is not the new
                    // project's.
                    if owner
                        .review_runtime
                        .save
                        .as_ref()
                        .is_some_and(|save| save.generation == generation)
                    {
                        owner.review_runtime.save_status = Some((worker_project, status));
                    }
                    // JA-1 S6 C3: the save ended; a detached host may exit.
                    owner.exit_when_detached_work_ends();
                }
            })
            .map_err(|_| refused("The save could not be started."))?;
        self.review_runtime.save_status =
            Some((project.clone(), SealedSaveStatus::Saving { name }));
        self.review_runtime.save = Some(SaveWorker {
            project,
            generation,
            cancellation,
            thread: Some(thread),
        });
        Ok(())
    }

    /// Test seam: install the per-chunk hook the next save passes through.
    #[cfg(test)]
    pub(in crate::writer_host) fn set_sealed_save_hook(&mut self, hook: Option<SaveHook>) {
        self.review_runtime.save_hook = hook;
    }

    /// Test seam: the running save's cancellation, so a stalled hook sees a
    /// Stop or a shutdown.
    #[cfg(test)]
    pub(in crate::writer_host) fn sealed_save_cancellation(&self) -> Option<ExportCancellation> {
        self.review_runtime
            .save
            .as_ref()
            .map(|worker| worker.cancellation.clone())
    }

    pub(super) fn stop_sealed_save(&mut self) -> Result<(), HostError> {
        match &self.review_runtime.save {
            Some(worker) if worker.running() => {
                worker.cancel();
                Ok(())
            }
            _ => Err(refused(NO_SAVE)),
        }
    }
}
