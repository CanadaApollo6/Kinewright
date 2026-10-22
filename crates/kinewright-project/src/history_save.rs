//! The project settle path is the only constructor of its Core persistence proof.
use kinewright_core::{Core, HistoryError, HistorySaveTag, HistoryTransition, PendingHistorySave};

struct ProjectHistoryProof;
pub(crate) struct FloorBarrier(kinewright_core::HistoryFloorBarrier<ProjectHistoryProof>);
impl FloorBarrier {
    pub(crate) fn retain_until_reconcile(&mut self) -> Result<(), HistoryError> {
        self.0.retain_until_reconcile(&ProjectHistoryProof)
    }
    pub(crate) fn tag_publication(&self, tag: HistorySaveTag) -> Result<(), HistoryError> {
        self.0.tag_publication(&ProjectHistoryProof, tag)
    }
    pub(crate) fn commit(&mut self) -> Result<(), HistoryError> {
        self.0.commit(&ProjectHistoryProof)
    }
    pub(crate) fn release(&mut self) -> Result<(), HistoryError> {
        self.0.release()
    }
}
pub(crate) fn bind(core: &Core) -> Result<(), HistoryError> {
    core.history_save_owner(&ProjectHistoryProof).map(|_| ())
}
pub(crate) fn begin(
    core: &Core,
    transition: HistoryTransition,
) -> Result<FloorBarrier, HistoryError> {
    core.history_save_owner(&ProjectHistoryProof)?
        .begin_history_floor(&ProjectHistoryProof, transition)
        .map(FloorBarrier)
}
pub(crate) fn settle(
    core: &Core,
    pending: &PendingHistorySave,
    commit: bool,
) -> Result<(), HistoryError> {
    core.history_save_owner(&ProjectHistoryProof)?
        .settle_history_save(&ProjectHistoryProof, pending, commit)
}
