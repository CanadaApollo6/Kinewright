#![allow(clippy::missing_errors_doc)]
//! Approval audits are compared with exact kept history, never a reopened actor's revision.
use crate::{
    bundle_records::{BranchId, ObjectRef, ProjectId},
    orchestration_records::{AttemptId, EndpointId, RunId, TaskId, TaskTurn},
};
use kinewright_core::{ClipId, TimelineRevision, TrackId};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EditApproval {
    pub task: TaskId,
    pub removed_clips: usize,
    pub removed_tracks: usize,
    pub applied: bool,
    pub issue: Option<String>,
    pub digest: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BatchBinding {
    pub project: ProjectId,
    pub run: RunId,
    pub task: TaskId,
    pub attempt: AttemptId,
    pub endpoint: EndpointId,
    pub owner_epoch: u64,
    pub history_epoch: EndpointId,
    pub history_position: u64,
    pub turn: TaskTurn,
    pub revision: TimelineRevision,
    pub batch_sha256: String,
    pub description: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RemovedObject {
    Clip { clip: ClipId, track: TrackId },
    Track { track: TrackId },
}
impl RemovedObject {
    #[must_use]
    pub fn label(&self) -> String {
        match self {
            Self::Clip { clip, track } => format!("clip {} on track {}", clip.0, track.0),
            Self::Track { track } => format!("track {}", track.0),
        }
    }
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovalRecord {
    pub kind: String,
    pub binding: BatchBinding,
    pub consent: ObjectRef,
    pub digest: String,
    pub removed_clips: usize,
    pub removed_tracks: usize,
    pub targets: Vec<RemovedObject>,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppliedRecord {
    pub kind: String,
    pub approval: ObjectRef,
    pub revision: TimelineRevision,
}
/// A checkpoint's exact accepted cut within a single running actor lineage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CapturedHistory {
    pub revision: TimelineRevision,
    pub position: Option<u64>,
}
impl CapturedHistory {
    #[must_use]
    pub fn from_cut(cut: &crate::operation_actor::AcceptedCut) -> Self {
        Self {
            revision: cut.change().revision,
            position: Some(cut.change().position.position),
        }
    }
}
// Every actor which can approve an edit needs an authenticated accepted work
// turn. Reopens can replace an actor within one attempt, so use the run's send
// cap, not its per-task attempt cap, for retained lineage coverage.
const EDIT_EPOCH_LIMIT: usize = crate::run_admission::SENDS;
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeptHistoryCut {
    pub epoch: EndpointId,
    pub revision: TimelineRevision,
    pub position: u64,
}
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeptEditCoverage {
    pub cuts: Vec<KeptHistoryCut>,
    pub incomplete: bool,
}
impl KeptEditCoverage {
    /// Auxiliary disclosure must never obstruct a checkpoint or Stop.
    pub fn record(&mut self, epoch: &EndpointId, saved: CapturedHistory) -> bool {
        let Some(position) = saved.position else {
            let changed = !self.incomplete;
            self.incomplete = true;
            return changed;
        };
        if let Some(previous) = self.cuts.iter_mut().find(|cut| &cut.epoch == epoch) {
            if previous.revision == saved.revision && previous.position == position {
                return false;
            }
            previous.revision = saved.revision;
            previous.position = position;
            true
        } else if self.cuts.len() < EDIT_EPOCH_LIMIT {
            self.cuts.push(KeptHistoryCut {
                epoch: epoch.clone(),
                revision: saved.revision,
                position,
            });
            true
        } else {
            let changed = !self.incomplete;
            self.incomplete = true;
            changed
        }
    }
    #[must_use]
    pub fn covers(&self, binding: &BatchBinding, revision: TimelineRevision) -> bool {
        self.cuts.iter().any(|cut| {
            cut.epoch == binding.history_epoch
                && cut.revision >= revision
                && binding
                    .history_position
                    .checked_add(1)
                    .is_some_and(|position| cut.position >= position)
        })
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MissingApprovedEdit {
    pub task: TaskId,
    pub approval: ObjectRef,
    pub digest: String,
    pub description: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UncertainApprovedEdit {
    pub task: TaskId,
    pub approval: ObjectRef,
    pub digest: String,
    pub description: String,
}
impl UncertainApprovedEdit {
    #[must_use]
    pub fn disclosure(&self) -> String {
        format!(
            "The kept version could not be confirmed to contain this approved edit: {}. Review the kept branch, then re-run the edit or continue the task.",
            self.description
        )
    }
}
#[derive(Debug, Default)]
pub struct EditAuditSummary {
    pub approvals: Vec<EditApproval>,
    pub missing: Vec<MissingApprovedEdit>,
    pub uncertain: Vec<UncertainApprovedEdit>,
}
#[must_use]
pub fn uncertain_edits_disclosure(edits: &[UncertainApprovedEdit]) -> String {
    edits
        .iter()
        .map(UncertainApprovedEdit::disclosure)
        .collect::<Vec<_>>()
        .join(" ")
}
#[must_use]
pub fn missing_edits_disclosure(edits: &[MissingApprovedEdit]) -> String {
    if edits.is_empty() {
        return String::new();
    }
    let labels = edits
        .iter()
        .map(|edit| edit.description.as_str())
        .collect::<Vec<_>>()
        .join("; ");
    format!(
        "{} approved {} not in the kept version: {labels}. These edits were approved, then undone or not kept. Re-run the missing edits, or continue the task to make them again.",
        edits.len(),
        if edits.len() == 1 {
            "edit is"
        } else {
            "edits are"
        }
    )
}
impl KeptEditCoverage {
    #[must_use]
    pub fn valid(&self) -> bool {
        self.cuts.len() <= EDIT_EPOCH_LIMIT
            && self
                .cuts
                .iter()
                .map(|cut| &cut.epoch)
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                == self.cuts.len()
    }
}

use crate::{
    bundle::BundleError,
    bundle_records::RecordKind,
    editing_session::Ja1EditingSession,
    orchestration_records::{OrchestrationPayload, RunProjection},
};
use std::collections::{BTreeMap, BTreeSet};

/// Optional live cuts are supplied only by the host which still owns those actors.
/// Retained/recovery inspection supplies none and trusts exact durable checkpoints.
#[derive(Debug)]
pub struct LiveEditCut {
    pub task: TaskId,
    pub cut: KeptHistoryCut,
    pub answers: BTreeMap<ObjectRef, crate::operation_history::ApprovalAnswer>,
}
struct KeptTaskAudit {
    coverage: KeptEditCoverage,
    branch: Option<BranchId>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RemovalProof {
    Kept,
    Missing,
    Uncertain,
}

impl KeptTaskAudit {
    fn removal_proof(
        &self,
        reference: &ObjectRef,
        bundle: &crate::bundle::ProjectBundle,
    ) -> RemovalProof {
        let Some(branch) = &self.branch else {
            return if self.coverage.incomplete {
                RemovalProof::Uncertain
            } else {
                RemovalProof::Missing
            };
        };
        // Keep no reader in the audit summary. Each answer is a fixed-size
        // ledger result; its cold reader and pool lease end before returning.
        match bundle.branch_history(branch).and_then(|reader| {
            Ok((
                reader.history().policy(),
                reader.approval_answer(reference)?,
            ))
        }) {
            Ok((_, Some(crate::operation_history::ApprovalAnswer::Kept))) => RemovalProof::Kept,
            Ok((_, Some(crate::operation_history::ApprovalAnswer::Missing))) => {
                RemovalProof::Missing
            }
            // This is the task's authenticated saved source, not a person-selected
            // sibling line. A task never folds and only reopens its own persisted
            // state, so an approval absent from that ledger was not kept there.
            Ok((kinewright_core::undo::RetentionPolicy::NeverFold, None)) => RemovalProof::Missing,
            _ => RemovalProof::Uncertain,
        }
    }
}
impl Ja1EditingSession {
    #[allow(
        clippy::too_many_lines,
        reason = "one bounded audit walk authenticates records and checkpoint lineage before reporting an edit"
    )]
    pub fn edit_audit_summary(
        &self,
        state: &RunProjection,
        live: &[LiveEditCut],
    ) -> Result<EditAuditSummary, BundleError> {
        if state.edit_approvals.is_empty() && state.edit_applications.is_empty() {
            return Ok(EditAuditSummary::default());
        }
        let _reading = crate::run_admission::reading(self.store()?);
        let project = self.project_id()?;
        let mut kept = BTreeMap::new();
        let mut integrity = false;
        for task in &state.tasks {
            let mut coverage = KeptEditCoverage::default();
            let mut branch = None;
            let inspected = (|| -> Result<(), BundleError> {
                for reference in task
                    .progress
                    .decisions
                    .iter()
                    .rev()
                    .filter(|r| r.record_kind == RecordKind::RunIndex)
                {
                    let record = self.store()?.read_run_record(reference)?;
                    if let OrchestrationPayload::TaskHistory(history) = record.payload {
                        if history.project != project
                            || history.run != state.run
                            || history.task != task.id
                            || history.base != task.brief.scope.base
                        {
                            integrity = true;
                            coverage.incomplete = true;
                        } else if self
                            .candidate_binding(&history.source.branch)
                            .is_ok_and(|source| source == history.source)
                        {
                            coverage = history.edit_coverage;
                            branch = Some(history.source.branch);
                        } else {
                            coverage.incomplete = true;
                        }
                        break;
                    }
                }
                Ok(())
            })();
            if inspected.is_err() {
                coverage.incomplete = true;
                integrity = true;
            }
            kept.insert(task.id.clone(), KeptTaskAudit { coverage, branch });
        }
        let mut applications = BTreeMap::<_, Option<TimelineRevision>>::new();
        for reference in &state.edit_applications {
            let record = (|| -> Result<AppliedRecord, BundleError> {
                serde_json::from_slice(&self.store()?.read_evidence(reference)?)
                    .map_err(|error| BundleError::Corrupt(error.to_string()))
            })();
            match record {
                Ok(record)
                    if record.kind == "studio_edit_applied_v1"
                        && state.edit_approvals.contains(&record.approval) =>
                {
                    applications
                        .entry(record.approval)
                        .and_modify(|previous| {
                            if *previous != Some(record.revision) {
                                *previous = None;
                                integrity = true;
                            }
                        })
                        .or_insert(Some(record.revision));
                }
                _ => integrity = true,
            }
        }
        let mut summary = EditAuditSummary::default();
        let mut seen = BTreeSet::new();
        for reference in &state.edit_approvals {
            if !seen.insert(reference) {
                continue;
            }
            let read = (|| -> Result<ApprovalRecord, BundleError> {
                let record: ApprovalRecord =
                    serde_json::from_slice(&self.store()?.read_evidence(reference)?)
                        .map_err(|error| BundleError::Corrupt(error.to_string()))?;
                let binding = serde_json::to_value(&record.binding)
                    .map_err(|error| BundleError::Corrupt(error.to_string()))?;
                let digest = kinewright_media::sha256_bytes(
                    &serde_json::to_vec(&binding)
                        .map_err(|error| BundleError::Corrupt(error.to_string()))?,
                );
                if record.kind != "studio_edit_approval_v1"
                    || record.binding.run != state.run
                    || record.binding.project != project
                    || !kept.contains_key(&record.binding.task)
                    || record.digest != digest
                    || record.removed_clips
                        != record
                            .targets
                            .iter()
                            .filter(|target| matches!(target, RemovedObject::Clip { .. }))
                            .count()
                    || record.removed_tracks
                        != record
                            .targets
                            .iter()
                            .filter(|target| matches!(target, RemovedObject::Track { .. }))
                            .count()
                {
                    return Err(BundleError::Corrupt("edit audit binding changed".into()));
                }
                Ok(record)
            })();
            let Ok(record) = read else {
                integrity = true;
                continue;
            };
            let application = applications.get(reference).copied().flatten();
            if application.is_some_and(|revision| {
                record.binding.revision.0.checked_add(1) != Some(revision.0)
            }) {
                integrity = true;
                continue;
            }
            let kept = kept
                .get_mut(&record.binding.task)
                .ok_or_else(|| BundleError::Corrupt("edit audit task binding".into()))?;
            let proof = application
                .map(|_revision| {
                    let mut proof = kept.removal_proof(reference, self.active_bundle()?);
                    for cut in live.iter().filter(|cut| cut.task == record.binding.task) {
                        if let Some(answer) = cut.answers.get(reference) {
                            proof = match answer {
                                crate::operation_history::ApprovalAnswer::Kept => {
                                    RemovalProof::Kept
                                }
                                crate::operation_history::ApprovalAnswer::Missing => {
                                    RemovalProof::Missing
                                }
                                crate::operation_history::ApprovalAnswer::Uncertain => {
                                    RemovalProof::Uncertain
                                }
                            };
                        }
                    }
                    Ok::<_, BundleError>(proof)
                })
                .transpose()?;
            let unkept = application.is_some() && proof != Some(RemovalProof::Kept);
            let uncertain = proof == Some(RemovalProof::Uncertain);
            let issue = if unkept {
                let targets = record
                    .targets
                    .iter()
                    .map(RemovedObject::label)
                    .collect::<Vec<_>>()
                    .join(", ");
                let description = if targets.is_empty() {
                    record.binding.description.clone()
                } else {
                    format!("{} ({targets})", record.binding.description)
                };
                let words = if uncertain {
                    let entry = UncertainApprovedEdit {
                        task: record.binding.task.clone(),
                        approval: reference.clone(),
                        digest: record.digest.clone(),
                        description,
                    };
                    let words = entry.disclosure();
                    summary.uncertain.push(entry);
                    words
                } else {
                    let entry = MissingApprovedEdit {
                        task: record.binding.task.clone(),
                        approval: reference.clone(),
                        digest: record.digest.clone(),
                        description,
                    };
                    let words = format!(
                        "This approved edit is not in the kept version: {}. It was undone or not kept. Re-run the edit, or continue the task.",
                        entry.description
                    );
                    summary.missing.push(entry);
                    words
                };
                Some(words)
            } else {
                None
            };
            summary.approvals.push(EditApproval {
                task: record.binding.task,
                removed_clips: record.removed_clips,
                removed_tracks: record.removed_tracks,
                applied: proof == Some(RemovalProof::Kept),
                issue,
                digest: record.digest,
            });
        }
        if integrity && let Some(root) = state.tasks.first() {
            summary.approvals.push(EditApproval {
                task: root.id.clone(),
                removed_clips: 0,
                removed_tracks: 0,
                applied: false,
                issue: Some("Some edit records could not be verified.".into()),
                digest: String::new(),
            });
        }
        Ok(summary)
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    fn epoch(value: char) -> EndpointId {
        serde_json::from_value(serde_json::json!(value.to_string().repeat(32))).unwrap()
    }
    #[test]
    fn c4b_coverage_is_exact_actor_scoped_and_bounded_without_blocking_checkpoint() {
        let mut coverage = KeptEditCoverage::default();
        coverage.record(
            &epoch('a'),
            CapturedHistory {
                revision: TimelineRevision(8),
                position: Some(20),
            },
        );
        coverage.record(
            &epoch('a'),
            CapturedHistory {
                revision: TimelineRevision(9),
                position: Some(18),
            },
        );
        assert_eq!(coverage.cuts.len(), 1);
        assert_eq!(
            coverage.cuts[0].position, 18,
            "C4B_REWOUND_HISTORY_NEVER_TAKES_MAXIMUM"
        );
        for value in 0..EDIT_EPOCH_LIMIT {
            let next_epoch =
                serde_json::from_value(serde_json::json!(format!("{value:032x}"))).unwrap();
            coverage.record(
                &next_epoch,
                CapturedHistory {
                    revision: TimelineRevision(2),
                    position: Some(3),
                },
            );
        }
        assert_eq!(coverage.cuts.len(), EDIT_EPOCH_LIMIT);
        assert!(
            coverage.incomplete,
            "C4B_DISCLOSURE_UNKNOWN_NEVER_BLOCKS_CHECKPOINT"
        );
        assert!(coverage.valid());
        let mut malformed = serde_json::to_value(&coverage).unwrap();
        malformed["extra"] = serde_json::json!(true);
        assert!(
            serde_json::from_value::<KeptEditCoverage>(malformed).is_err(),
            "C4B_STRICT_CURRENT_SHAPE"
        );
        assert!(
            serde_json::from_value::<KeptEditCoverage>(serde_json::json!({"cuts":[]})).is_err(),
            "C4B_NO_OLD_SHAPE_READER"
        );
    }
    #[test]
    fn c4b_missing_edit_words_count_and_identify_the_approved_changes() {
        let run: RunId = serde_json::from_value(serde_json::json!("a".repeat(32))).unwrap();
        let task = TaskId(run.0);
        let edits = [1, 2]
            .into_iter()
            .map(|clip| MissingApprovedEdit {
                task: task.clone(),
                approval: ObjectRef {
                    sha256: "a".repeat(64),
                    byte_length: 1,
                    record_kind: crate::bundle_records::RecordKind::OwnedEvidence,
                },
                digest: "b".repeat(64),
                description: format!(
                    "Remove {}",
                    RemovedObject::Clip {
                        clip: ClipId(clip),
                        track: TrackId(81)
                    }
                    .label()
                ),
            })
            .collect::<Vec<_>>();
        let words = missing_edits_disclosure(&edits);
        assert!(words.contains("2 approved edits are not in the kept version"));
        assert!(words.contains("clip 1") && words.contains("clip 2") && words.contains("track 81"));
        assert!(words.contains("Re-run") && words.contains("continue the task"));
        assert!(missing_edits_disclosure(&[]).is_empty());
    }
}

#[cfg(test)]
mod compact_position_tests {
    use super::CapturedHistory;
    use crate::operation_actor::Core;
    use kinewright_core::{Command, Document, Operation, Track, TrackId, TrackKind};
    #[test]
    fn c4b_compact_history_preserves_absolute_accepted_batch_position() {
        let core = Core::for_test(Document::default()).unwrap();
        for command in [
            Command::Undo,
            Command::Redo,
            Command::Do(Operation::AddTrack {
                track: Track {
                    id: TrackId(1),
                    kind: TrackKind::Video,
                    sync_lock: false,
                    clips: vec![],
                },
            }),
            Command::DoBatch(vec![Operation::SetTrackSyncLock {
                track: TrackId(1),
                locked: true,
            }]),
            Command::Undo,
            Command::Redo,
        ] {
            core.request(command).unwrap();
        }
        let attachment = crate::ProjectAttachment::new(&core).unwrap();
        let accepted = attachment.capture().unwrap();
        let captured = CapturedHistory::from_cut(&accepted);
        assert_eq!(
            captured.position,
            Some(6),
            "EMPTY_UNDO_REDO_KEEP_ABSOLUTE_ACCEPTED_POSITION"
        );
        let wire = core.portable_history().unwrap();
        let branch = wire.branch.clone();
        let revision = wire.revision;
        let pool = crate::operation_history::HistoryPool::new(
            crate::operation_history::PoolLimits::default(),
        );
        let restored = Core::open(wire.validate(&pool).unwrap(), branch, revision).unwrap();
        let replayed = crate::ProjectAttachment::new(&restored)
            .unwrap()
            .capture()
            .unwrap();
        assert_eq!(
            CapturedHistory::from_cut(&replayed).position,
            captured.position,
            "PORTABLE_REOPEN_KEEPS_ACCEPTED_POSITION"
        );
        assert_eq!(
            replayed.identity(),
            accepted.identity(),
            "PORTABLE_REOPEN_KEEPS_COMBINED_IDENTITY"
        );
    }
}
