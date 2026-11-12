//! Revoked native owners survive frames; terminal shutdown joins after persistence.
use crate::{export_ui::ExportJob, joined_thread::JoinedThread, project::ProjectSession};
use kinewright_agent::{McpServer, export_queue::ExportQueue};

#[derive(Default)]
pub(crate) struct RetirementGroup {
    servers: Vec<McpServer>,
    workers: Vec<JoinedThread<()>>,
    exports: Vec<ExportJob>,
    queues: Vec<ExportQueue>,
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
            self.workers.append(&mut investigator.take_retired());
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

    fn retire<T: Send + 'static>(&mut self, owner: T) {
        self.retire_close(move || drop(owner));
    }

    fn retire_close(&mut self, close: impl FnOnce() + Send + 'static) {
        let worker = std::thread::Builder::new()
            .name("kinewright-owner-reaper".into())
            .spawn(close)
            .expect("native retirement worker must start");
        self.workers.push(JoinedThread::new(worker));
    }

    pub(crate) fn export(&mut self, export: ExportJob) {
        self.exports.push(export);
    }

    pub(crate) fn prune(&mut self) {
        self.servers.retain(|server| !server.is_finished());
        self.workers.retain(|worker| !worker.is_finished());
        self.exports.retain(|export| !export.is_finished());
    }

    pub(crate) fn join(&mut self) {
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
