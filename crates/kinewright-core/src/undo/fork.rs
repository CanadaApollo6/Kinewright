//! In-process previews start only from an already validated live History.
//! Raw Records cannot enter this path; they still need semantic validation.
use super::{Cache, DocumentBound, Error, History, RoleBounds};
impl History {
    fn profile_bounds(&self, bounds: RoleBounds) -> Result<(DocumentBound, DocumentBound), Error> {
        let steps = self
            .undo
            .len()
            .checked_add(self.redo.len())
            .ok_or(Error::Capacity)?;
        let charge = self.charge();
        if steps > bounds.step_cap
            || charge.encoded > bounds.operation_bytes
            || charge.decoded > bounds.decoded_history_bytes
        {
            return Err(Error::HistoryFull);
        }
        let baseline = if self.baseline_bound.check(bounds).is_ok() {
            self.baseline_bound
        } else {
            DocumentBound::measure(&self.baseline, bounds)?
        };
        let document = if self.document_bound.check(bounds).is_ok() {
            self.document_bound
        } else {
            DocumentBound::measure(&self.document, bounds)?
        };
        if let Some(guard) = self.merge_guard {
            // Rebuild only the fixed workspace proof. This is construction,
            // never a per-command history scan or an untrusted replay shortcut.
            super::merge::MergeGuard::from_steps(
                guard.input,
                &self.undo,
                &self.redo,
                bounds.replay_workspace,
            )?;
        }
        Ok((baseline, document))
    }
    fn preview_cache(&self, bounds: RoleBounds) -> Result<Cache, Error> {
        if bounds.checkpoint_documents == self.bounds.checkpoint_documents
            && bounds.document_live_bytes >= self.bounds.document_live_bytes
        {
            // Immutable anchor documents are shared by two separately leased
            // holders of the same validated prefix, rather than copied.
            Ok(self.cache.clone())
        } else {
            Cache::new(bounds.checkpoint_documents, bounds.decoded_history_bytes)
        }
    }
    /// Independent, prevalidated private state. The owner admits its full
    /// preview lease before calling this constructor. No document is copied.
    ///
    /// Untrusted records have no preview constructor:
    /// ```compile_fail
    /// use kinewright_core::undo::{Record, RoleBounds};
    /// fn bypass(record: Record) {
    ///     let _ = record.fork_with_bounds(RoleBounds::TASK);
    /// }
    /// ```
    pub fn fork_with_bounds(&self, bounds: RoleBounds) -> Result<Self, Error> {
        let (baseline_bound, document_bound) = self.profile_bounds(bounds)?;
        let cache = self.preview_cache(bounds)?;
        Ok(Self {
            baseline: std::sync::Arc::clone(&self.baseline),
            document: std::sync::Arc::clone(&self.document),
            baseline_bound,
            document_bound,
            // The independently retained preview owns its command payloads.
            undo: self.undo.clone(),
            redo: self.redo.clone(),
            position: self.position,
            epoch: self.epoch,
            policy: self.policy,
            bounds,
            undo_charge: self.undo_charge,
            redo_charge: self.redo_charge,
            cache,
            recorder: None,
            recorder_bytes: 0,
            merge_guard: self.merge_guard,
        })
    }
    /// Seal a prepared state for its original destination's fixed profile.
    /// Admission and cache allocation finish before the state can be published.
    pub fn into_bounds(mut self, bounds: RoleBounds) -> Result<Self, Error> {
        let (baseline_bound, document_bound) = self.profile_bounds(bounds)?;
        if bounds.checkpoint_documents != self.bounds.checkpoint_documents
            || bounds.document_live_bytes < self.bounds.document_live_bytes
        {
            self.cache = Cache::new(bounds.checkpoint_documents, bounds.decoded_history_bytes)?;
        }
        self.bounds = bounds;
        self.baseline_bound = baseline_bound;
        self.document_bound = document_bound;
        Ok(self)
    }
}
