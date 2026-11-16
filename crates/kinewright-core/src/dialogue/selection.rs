//! Hard selection boundaries, independent of acoustic edge overrules.
use super::{CutRequest, Dialogue, Refusal, RefusalReason, Span};
use crate::ExactMicros;
use serde::Serialize;
use thiserror::Error;

/// Maximum distance from an ASR start to its measured sound-component onset.
/// Also protects the entire forward window of every unrequested start. Ends of
/// transcript words are approximate and never establish sound ownership.
pub const WORD_ATTRIBUTION_MICROS: u64 = 300_000;

/// Fixed-size evidence: no copied word text or unbounded diagnostic string.
#[derive(Debug, PartialEq, Eq, Serialize, Error)]
#[serde(tag = "violation", rename_all = "snake_case")]
pub enum SelectionIssue {
    #[error(
        "cut would erase unrequested word start #{word}; select it explicitly or use a smaller removal"
    )]
    UnrequestedWordStart { word: usize },
    #[error(
        "sound at {component:?} may belong to kept word #{word}; use a smaller removal or audition it manually"
    )]
    UnrequestedSound { word: usize, component: Span },
    #[error(
        "sound at {component:?} cannot be attributed exclusively to requested word starts within 300 ms; no audio was removed"
    )]
    UnattributedSound { component: Span },
    #[error(
        "reported pause is {full_micros} microseconds, but this clip retains only {usable_micros}; cannot leave {leave_micros}. Read the current clip's transcript after earlier cuts"
    )]
    PauseLimited {
        full_micros: u64,
        usable_micros: u64,
        leave_micros: u64,
        available: Span,
    },
}
impl Dialogue {
    /// Validate the exact removed source extent, including after frame mapping.
    /// A sound-component onset needs a requested spoken-word start within
    /// 300 ms. All plausible owners must be requested: any kept start within
    /// that reach, or whose forward 300 ms window overlaps the component,
    /// vetoes removal. Supplied word ends never establish sound ownership.
    /// Acoustic overrules do not waive this check.
    ///
    /// # Errors
    /// Refuses unrequested starts, kept or unanchored sound, and invalid ranges.
    pub fn validate_cut(
        &self,
        request: CutRequest,
        removed: [ExactMicros; 2],
    ) -> Result<(), Refusal> {
        let requested = match request {
            CutRequest::RemoveWords { from, to, .. } if from <= to && to < self.words.len() => {
                Some(from..=to)
            }
            CutRequest::TrimPause { after_word, .. }
                if after_word
                    .checked_add(1)
                    .is_some_and(|i| i < self.words.len()) =>
            {
                None
            }
            _ => return Err(self.refuse(RefusalReason::WordRange, 0)),
        };
        let [start, end] = removed;
        if start >= end {
            return Err(self.refuse(RefusalReason::NotForward, start.floor()));
        }
        if end > ExactMicros::integer(self.profile.duration_micros()) {
            return Err(self.refuse(RefusalReason::Unmeasured, end.floor()));
        }
        let selected = |i| requested.as_ref().is_some_and(|range| range.contains(&i));
        for (word, w) in self.words.iter().enumerate() {
            let at = ExactMicros::integer(w.start_micros);
            if !selected(word) && start <= at && at < end {
                return Err(self.refuse(
                    SelectionIssue::UnrequestedWordStart { word }.into(),
                    w.start_micros,
                ));
            }
        }
        for &component in self.rms_sound.iter().filter(|run| {
            ExactMicros::integer(run.start_micros) < end
                && start < ExactMicros::integer(run.end_micros)
        }) {
            let mut owned = false;
            for (word, w) in self.words.iter().enumerate() {
                let candidate =
                    component.start_micros.abs_diff(w.start_micros) <= WORD_ATTRIBUTION_MICROS;
                let protected = w.start_micros < component.end_micros
                    && component.start_micros
                        < w.start_micros.saturating_add(WORD_ATTRIBUTION_MICROS);
                if !selected(word) && (protected || candidate) {
                    return Err(self.refuse(
                        SelectionIssue::UnrequestedSound { word, component }.into(),
                        component.start_micros.max(start.floor()),
                    ));
                }
                if selected(word) && w.is_spoken() && candidate {
                    owned = true;
                }
            }
            if !owned {
                return Err(self.refuse(
                    SelectionIssue::UnattributedSound { component }.into(),
                    component.start_micros.max(start.floor()),
                ));
            }
        }
        Ok(())
    }
}
