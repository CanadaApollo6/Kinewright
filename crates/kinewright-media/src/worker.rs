use std::thread::JoinHandle;

use crossbeam_channel::{SendError, Sender, TrySendError};

/// Closing the unique sender must precede joining, including constructor errors.
pub(crate) struct OwnedWorker<J> {
    sender: Option<Sender<J>>,
    thread: Option<JoinHandle<()>>,
}

impl<J> OwnedWorker<J> {
    #[cfg(test)]
    pub(crate) fn inline(sender: Sender<J>) -> Self {
        Self {
            sender: Some(sender),
            thread: None,
        }
    }
    pub(crate) fn new(sender: Sender<J>, thread: JoinHandle<()>) -> Self {
        Self {
            sender: Some(sender),
            thread: Some(thread),
        }
    }

    pub(crate) fn send(&self, job: J) -> Result<(), SendError<J>> {
        self.sender.as_ref().expect("live worker sender").send(job)
    }

    pub(crate) fn try_send(&self, job: J) -> Result<(), TrySendError<J>> {
        self.sender
            .as_ref()
            .expect("live worker sender")
            .try_send(job)
    }
}

impl<J> Drop for OwnedWorker<J> {
    fn drop(&mut self) {
        self.sender.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
