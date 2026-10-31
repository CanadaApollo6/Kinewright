//! Qualification waits outside H3, after a committed merge and on the next client control.
use crate::orchestration::orchestration_records::TaskId;
#[cfg(any(test, feature = "eval-harness"))]
use std::sync::Arc;
use std::sync::{Condvar, Mutex};

#[derive(Default)]
enum State {
    #[default]
    Watching,
    Merged(TaskId),
    Holding,
    #[cfg(any(test, feature = "eval-harness"))]
    Released,
    TimedOut,
}
#[derive(Default)]
pub(super) struct MergeStopGate {
    state: Mutex<State>,
    changed: Condvar,
}
impl MergeStopGate {
    pub fn after_merge(&self, task: TaskId) {
        let mut state = self.state.lock().unwrap();
        if matches!(*state, State::Watching) {
            *state = State::Merged(task);
        }
    }
    pub fn before_request(&self, task: &TaskId) -> Result<(), &'static str> {
        let mut state = self.state.lock().unwrap();
        if matches!(&*state, State::Merged(id) if id == task) {
            *state = State::Holding;
            self.changed.notify_all();
            let (mut next, timeout) = self
                .changed
                .wait_timeout_while(state, std::time::Duration::from_mins(1), |s| {
                    matches!(s, State::Holding)
                })
                .unwrap();
            if timeout.timed_out() && matches!(*next, State::Holding) {
                *next = State::TimedOut;
            }
            if matches!(*next, State::TimedOut) {
                return Err("qualification Stop barrier timed out");
            }
        }
        Ok(())
    }
    #[cfg(any(test, feature = "eval-harness"))]
    pub fn holding(&self) -> bool {
        matches!(*self.state.lock().unwrap(), State::Holding)
    }
    #[cfg(any(test, feature = "eval-harness"))]
    pub fn release(&self) {
        *self.state.lock().unwrap() = State::Released;
        self.changed.notify_all();
    }
    #[cfg(any(test, feature = "eval-harness"))]
    pub fn release_on_drop(self: &Arc<Self>) -> ReleaseOnDrop {
        ReleaseOnDrop(self.clone())
    }
}
#[cfg(any(test, feature = "eval-harness"))]
pub(super) struct ReleaseOnDrop(Arc<MergeStopGate>);
#[cfg(any(test, feature = "eval-harness"))]
impl Drop for ReleaseOnDrop {
    fn drop(&mut self) {
        self.0.release();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn s4b_c5d_barrier_is_after_merge_and_bounded_to_its_task() {
        let gate = Arc::new(MergeStopGate::default());
        let task: TaskId = serde_json::from_value(serde_json::json!("1".repeat(32))).unwrap();
        let other: TaskId = serde_json::from_value(serde_json::json!("2".repeat(32))).unwrap();
        gate.before_request(&task).unwrap();
        gate.after_merge(task.clone());
        gate.before_request(&other).unwrap();
        let worker_gate = gate.clone();
        let worker = std::thread::spawn(move || worker_gate.before_request(&task));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while !gate.holding() && std::time::Instant::now() < deadline {
            std::thread::yield_now();
        }
        let held = gate.holding();
        gate.release();
        worker.join().unwrap().unwrap();
        assert!(held, "C5D_POST_MERGE_BARRIER");
    }
}
