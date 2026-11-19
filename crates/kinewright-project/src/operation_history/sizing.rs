//! Immutable admission inputs. Capture and cold open derive exactly the same lease.
use super::{Envelope, Error, Role};
use kinewright_core::undo::{Charge, RoleBounds};
use serde::{Deserialize, Serialize};

pub const PRODUCT_DOCUMENT_BYTES: u64 = RoleBounds::ORDINARY.document_live_bytes;
pub const WORKSPACE_BYTES: u64 = 168 << 20;
const DOCUMENT_FLOOR: u64 = 512 << 10;
// Small baselines still need room for the fixed 2,000-removal fixture. The
// doubled-baseline floor dominates once the baseline itself is substantial.
const MINIMUM_DOCUMENT_GROWTH: u64 = 2 << 20;
pub(crate) const OWNER_ALLOWANCE: u64 = 3 << 20;
const OWNER_MAXIMUM: u64 = 32 << 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CreationKind {
    Ordinary,
    Task,
    LiveCandidate,
    StagingCandidate,
}
impl CreationKind {
    #[must_use]
    pub const fn role(self) -> Role {
        match self {
            Self::Ordinary => Role::Ordinary,
            Self::Task => Role::Task,
            Self::LiveCandidate | Self::StagingCandidate => Role::Candidate,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreationSizing {
    pub kind: CreationKind,
    pub baseline_live: u64,
    pub baseline_owners: u64,
    pub initial_batch: Charge,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FixedLimits {
    pub bounds: RoleBounds,
    pub envelope: Envelope,
}
impl FixedLimits {
    pub(super) const fn body_cap(self) -> u64 {
        self.bounds.decoded_history_bytes + self.envelope.owners
    }
    pub(super) const fn shell_cap(self) -> u64 {
        self.bounds.decoded_history_bytes + 2 * self.envelope.owners + 8192
    }
}
impl CreationSizing {
    /// Pure and checked: untrusted inputs can never raise a product maximum.
    pub fn limits(self) -> Result<FixedLimits, Error> {
        if self.baseline_live > PRODUCT_DOCUMENT_BYTES
            || self.baseline_owners > OWNER_MAXIMUM
            || (self.kind != CreationKind::Ordinary
                && self.baseline_owners > OWNER_MAXIMUM - OWNER_ALLOWANCE)
            || self.initial_batch.encoded > RoleBounds::TASK.operation_bytes
            || self.initial_batch.decoded > RoleBounds::TASK.decoded_history_bytes
            || (self.kind != CreationKind::StagingCandidate
                && self.initial_batch != Charge::default())
        {
            return Err(Error::Capacity("creation sizing"));
        }
        let role = self.kind.role();
        let mut bounds = role.bounds();
        let owners = if role == Role::Ordinary {
            OWNER_MAXIMUM
        } else {
            bounds.document_live_bytes = self
                .baseline_live
                .saturating_mul(2)
                .saturating_add(DOCUMENT_FLOOR)
                .max(self.baseline_live.saturating_add(MINIMUM_DOCUMENT_GROWTH))
                .min(PRODUCT_DOCUMENT_BYTES);
            self.baseline_owners + OWNER_ALLOWANCE
        };
        if self.kind == CreationKind::StagingCandidate {
            bounds.operation_bytes = self
                .initial_batch
                .encoded
                .saturating_add(64 << 10)
                .min(RoleBounds::TASK.operation_bytes);
            bounds.decoded_history_bytes = self
                .initial_batch
                .decoded
                .saturating_add(512 << 10)
                .min(RoleBounds::TASK.decoded_history_bytes);
        }
        Ok(FixedLimits {
            envelope: envelope(bounds, owners),
            bounds,
        })
    }
}
pub(super) const fn envelope(bounds: RoleBounds, owners: u64) -> Envelope {
    Envelope {
        retained: (2 + bounds.checkpoint_documents as u64) * bounds.document_live_bytes
            + bounds.decoded_history_bytes
            + owners,
        workspace: bounds.replay_workspace,
        owners,
    }
}
// Every fresh merge destination accepts the complete admitted source as one batch.
const _: () = {
    assert!(
        RoleBounds::TASK.operation_bytes <= RoleBounds::CANDIDATE.operation_bytes,
        "TASK_TO_CANDIDATE_ENCODED_FIT"
    );
    assert!(
        RoleBounds::TASK.decoded_history_bytes <= RoleBounds::CANDIDATE.decoded_history_bytes,
        "TASK_TO_CANDIDATE_DECODED_FIT"
    );
    assert!(
        RoleBounds::CANDIDATE.operation_bytes <= RoleBounds::ORDINARY.operation_bytes,
        "CANDIDATE_TO_ORDINARY_ENCODED_FIT"
    );
    assert!(
        RoleBounds::CANDIDATE.decoded_history_bytes <= RoleBounds::ORDINARY.decoded_history_bytes,
        "CANDIDATE_TO_ORDINARY_DECODED_FIT"
    );
    assert!(
        RoleBounds::TASK.document_bytes == PRODUCT_DOCUMENT_BYTES,
        "TASK_PRODUCT_DOCUMENT_ENCODED_FIT"
    );
    assert!(
        RoleBounds::CANDIDATE.document_bytes == PRODUCT_DOCUMENT_BYTES,
        "CANDIDATE_PRODUCT_DOCUMENT_ENCODED_FIT"
    );
    assert!(
        RoleBounds::ORDINARY.document_bytes == PRODUCT_DOCUMENT_BYTES,
        "ORDINARY_PRODUCT_DOCUMENT_ENCODED_FIT"
    );
    assert!(
        RoleBounds::TASK.document_live_bytes <= RoleBounds::CANDIDATE.document_live_bytes,
        "TASK_TO_CANDIDATE_LIVE_DOCUMENT_FIT"
    );
    assert!(
        RoleBounds::CANDIDATE.document_live_bytes <= RoleBounds::ORDINARY.document_live_bytes,
        "CANDIDATE_TO_ORDINARY_LIVE_DOCUMENT_FIT"
    );
    assert!(
        RoleBounds::TASK.replay_workspace <= RoleBounds::CANDIDATE.replay_workspace,
        "TASK_TO_CANDIDATE_WORKSPACE_FIT"
    );
    assert!(
        RoleBounds::CANDIDATE.replay_workspace <= RoleBounds::ORDINARY.replay_workspace,
        "CANDIDATE_TO_ORDINARY_WORKSPACE_FIT"
    );
    assert!(
        RoleBounds::CANDIDATE.step_cap >= 1,
        "CANDIDATE_ACCEPTS_ONE_MERGED_BATCH"
    );
    assert!(
        RoleBounds::ORDINARY.step_cap >= 1,
        "ORDINARY_ACCEPTS_ONE_MERGED_BATCH"
    );
};

/// Largest destination owner scratch, including a simultaneous fold patch.
pub(super) const MERGE_DESTINATION_INPUT: u64 = 3 * (32 << 20);
