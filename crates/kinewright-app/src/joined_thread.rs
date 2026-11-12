use std::thread::{JoinHandle, Result};

#[derive(Debug)]
pub(crate) struct JoinedThread<T>(Option<JoinHandle<T>>);

impl<T> JoinedThread<T> {
    pub(crate) fn new(handle: JoinHandle<T>) -> Self {
        Self(Some(handle))
    }

    pub(crate) fn is_finished(&self) -> bool {
        self.0.as_ref().is_none_or(JoinHandle::is_finished)
    }

    pub(crate) fn join(mut self) -> Result<T> {
        self.0.take().expect("live worker handle").join()
    }
}

impl<T> Drop for JoinedThread<T> {
    fn drop(&mut self) {
        if let Some(handle) = self.0.take() {
            let _ = handle.join();
        }
    }
}
