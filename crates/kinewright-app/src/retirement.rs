//! Revoked native owners survive frames; terminal shutdown joins after persistence.
use crate::{export_ui::ExportJob, joined_thread::JoinedThread, project::ProjectSession};
use kinewright_agent::{McpServer, export_queue::ExportQueue};

#[derive(Default)]
pub(crate) struct RetirementGroup {
    servers: Vec<McpServer>,
    workers: Vec<JoinedThread<()>>,
    exports: Vec<ExportJob>,
    queues: Vec<ExportQueue>,
    unstarted: Vec<Box<dyn FnOnce() + Send>>,
    #[cfg(all(test, target_os = "linux"))]
    refuse_spawn: bool,
}

impl RetirementGroup {
    pub(crate) fn server(&mut self, mut server: McpServer) {
        server.request_shutdown();
        self.servers.push(server);
    }

    pub(crate) fn project(&mut self, mut project: ProjectSession) {
        for thread in &mut project.threads {
            if let Some(session) = thread.session.take() {
                self.retire(session);
            }
            if let Some(server) = thread.mcp_server.take() {
                self.server(server);
            }
        }
        if let Some(investigator) = &mut project.investigator {
            self.append(investigator.take_retired());
        }
        if let Some(mut source) = project.ja1.take() {
            self.retire_close(move || {
                let _ = source.close();
            });
        }
        // A finished endpoint must never be the last queue owner on a frame.
        if let Some(queue) = project.exports.take() {
            self.queues.push(queue);
        }
    }

    pub(crate) fn retire<T: Send + 'static>(&mut self, owner: T) {
        self.retire_close(move || drop(owner));
    }

    fn retire_close(&mut self, close: impl FnOnce() + Send + 'static) {
        // Spawn with only the receiving end. Failure never drops the live owner on this frame.
        let (sender, receiver) = std::sync::mpsc::sync_channel::<Box<dyn FnOnce() + Send>>(1);
        let spawn = || {
            std::thread::Builder::new()
                .name("kinewright-owner-reaper".into())
                .spawn(move || {
                    if let Ok(close) = receiver.recv() {
                        close();
                    }
                })
        };
        #[cfg(all(test, target_os = "linux"))]
        let worker = if self.refuse_spawn {
            Err(std::io::Error::other("fixture spawn refused"))
        } else {
            spawn()
        };
        #[cfg(not(all(test, target_os = "linux")))]
        let worker = spawn();
        match worker {
            Ok(worker) => {
                if let Err(error) = sender.send(Box::new(close)) {
                    self.unstarted.push(error.0);
                }
                self.workers.push(JoinedThread::new(worker));
            }
            Err(_) => self.unstarted.push(Box::new(close)),
        }
    }

    pub(crate) fn append(&mut self, mut other: Self) {
        self.servers.append(&mut other.servers);
        self.workers.append(&mut other.workers);
        self.exports.append(&mut other.exports);
        self.queues.append(&mut other.queues);
        self.unstarted.append(&mut other.unstarted);
    }

    #[cfg(all(test, target_os = "linux"))]
    pub(crate) fn worker(&mut self, worker: JoinedThread<()>) {
        self.workers.push(worker);
    }

    #[cfg(all(test, target_os = "linux"))]
    pub(crate) fn refuse_spawn_for_test(&mut self) {
        self.refuse_spawn = true;
    }

    pub(crate) fn export(&mut self, export: ExportJob) {
        self.exports.push(export);
    }

    pub(crate) fn prune(&mut self) {
        self.servers.retain(|server| !server.is_finished());
        self.workers.retain(|worker| !worker.is_finished());
        self.exports.retain(|export| !export.is_finished());
        self.queues.retain(|queue| !queue.is_finished());
    }

    pub(crate) fn join(&mut self) {
        for close in self.unstarted.drain(..) {
            close();
        }
        self.servers.clear();
        self.workers.clear();
        self.exports.clear();
        for queue in &self.queues {
            queue.join();
        }
        self.queues.clear();
    }
}

impl Drop for RetirementGroup {
    fn drop(&mut self) {
        self.join();
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::sync::{
        Arc, RwLock,
        atomic::{AtomicBool, Ordering},
    };

    #[test]
    fn spawn_failure_keeps_the_owner_for_terminal_join() {
        let closed = Arc::new(AtomicBool::new(false));
        let closing = Arc::clone(&closed);
        let mut group = RetirementGroup {
            servers: vec![],
            workers: vec![],
            exports: vec![],
            queues: vec![],
            unstarted: vec![],
            refuse_spawn: true,
        };
        group.retire_close(move || closing.store(true, Ordering::Release));
        assert!(!closed.load(Ordering::Acquire));
        group.prune();
        assert!(!closed.load(Ordering::Acquire));
        assert_eq!(group.unstarted.len(), 1);
        group.join();
        assert!(closed.load(Ordering::Acquire));
        assert!(group.unstarted.is_empty());
    }

    #[test]
    fn queue_retirement_prunes_only_after_os_completion() {
        let queue = ExportQueue::with_lut_project_path(
            Arc::new(kinewright_core::RefusedMedia),
            Arc::new(kinewright_core::RefusedMedia),
            Arc::new(RwLock::new(None)),
        )
        .unwrap();
        let mut group = RetirementGroup::default();
        group.queues.push(queue.clone());
        group.prune();
        assert_eq!(group.queues.len(), 1);
        queue.close_admission();
        queue.join();
        group.prune();
        assert!(group.queues.is_empty());
    }
}
