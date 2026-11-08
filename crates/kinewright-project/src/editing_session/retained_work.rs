//! Person-owned recovery views derived from retained task records, including stopped runs.
use super::{BundleError, Ja1EditingSession};
use crate::{
    bundle_records::BranchId,
    bundle_recovery::RecoverySelection,
    orchestration_commit::CommandDecision,
    orchestration_records::{
        BranchBinding, CompletionStep, OrchestrationPayload, RunCleanup, RunId, RunOutcome, TaskId,
    },
    receipts::ReceiptLookup,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetainedWorkKind {
    Task,
    Completion,
    Merge,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "independent retained-record, cleanup and timeline facts"
)]
pub struct RetainedTaskWork {
    pub run: RunId,
    pub task: TaskId,
    pub kind: RetainedWorkKind,
    pub source: BranchBinding,
    pub frozen: bool,
    pub stopped: bool,
    pub cleanup_settled: bool,
    pub available: bool,
    pub open_on_timeline: bool,
    pub disclosure: String,
    #[serde(default)]
    pub settled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reviewed_on: Option<BranchId>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetainedWorkFailure {
    pub run: Option<RunId>,
    pub task: Option<TaskId>,
    pub branch: Option<BranchId>,
    pub disclosure: String,
}
impl RetainedWorkFailure {
    /// An unbound inspection failure prevents choosing any task source.
    #[must_use]
    pub fn blocks_branch(&self, branch: &BranchId) -> bool {
        self.branch.as_ref().is_none_or(|failed| failed == branch)
    }
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetainedTaskDiscovery {
    pub work: Vec<RetainedTaskWork>,
    pub failures: Vec<RetainedWorkFailure>,
    /// Suppress generic actions even for settled or unreadable task sources.
    pub task_branches: Vec<BranchId>,
}
impl RetainedTaskDiscovery {
    fn visible(&mut self) {
        self.work.retain(|work| {
            !work.settled && (work.stopped || work.frozen || work.kind != RetainedWorkKind::Task)
        });
    }
}
impl Ja1EditingSession {
    /// Best-effort disclosure isolates a broken history to its own task.
    /// Commands continue to use the strict source walk below.
    pub fn retained_task_discovery(&self) -> RetainedTaskDiscovery {
        let mut discovery = self.retained_task_sources().unwrap_or_else(|_| RetainedTaskDiscovery {
            failures: vec![RetainedWorkFailure {
                run: None, task: None, branch: None,
                disclosure: "Kept task work could not be inspected. Keep the working branches and try again. Opening or removing that work is unavailable until this is resolved.".into(),
            }],
            ..RetainedTaskDiscovery::default()
        });
        discovery.visible();
        discovery
    }
    pub fn retained_task_work(&self) -> Result<Vec<RetainedTaskWork>, BundleError> {
        let mut discovery = self.retained_task_sources()?;
        if !discovery.failures.is_empty() {
            return Err(BundleError::Identity(
                "retained task work could not be inspected".into(),
            ));
        }
        discovery.visible();
        Ok(discovery.work)
    }
    fn retained_task_sources(&self) -> Result<RetainedTaskDiscovery, BundleError> {
        // Tasks require a named bundle. An ordinary unsaved session has no
        // retained task records; absence here is not an inspection failure.
        if self.destination.is_none() {
            return Ok(RetainedTaskDiscovery::default());
        }
        let _reading = crate::run_admission::reading(self.store()?);
        let bundle = self.active_bundle()?;
        let index = bundle.run_index()?;
        let cuts = index
            .orchestration
            .iter()
            .filter_map(|(run, cuts)| cuts.last().map(|cut| (run, &cut.snapshot)))
            .collect::<Vec<_>>();
        // Receipt lookups also depend on pending commands and in-memory issuer
        // retirement, including transitions before the next root publication.
        let key = kinewright_media::sha256_bytes(
            &serde_json::to_vec(&(
                bundle.store.owner_epoch(),
                &bundle.root,
                &cuts,
                bundle.pending_command.as_deref(),
                &bundle.issuers,
            ))
            .map_err(|e| BundleError::Corrupt(e.to_string()))?,
        );
        if let Some((previous, discovery)) = &*self.retained_task_work_cache.borrow()
            && previous == &key
        {
            return Ok(discovery.clone());
        }
        let mut discovery = RetainedTaskDiscovery::default();
        for (run, _) in cuts {
            let state = match self.orchestration_projection(run) {
                Ok(Some(state)) => state,
                Ok(None) => continue,
                Err(_) => {
                    discovery.failures.push(RetainedWorkFailure {
                        run: Some(run.clone()), task: None, branch: None,
                        disclosure: format!("Run {} kept task work could not be inspected. Keep its working branches and try again. Opening or removing that work is unavailable until this is resolved.", run.0.as_str()),
                    });
                    continue;
                }
            };
            for task in &state.tasks {
                let mut source = None;
                match self.inspect_retained_task(&state, task, &mut source) {
                    Ok(Some(work)) => {
                        discovery.task_branches.push(work.source.branch.clone());
                        discovery.work.push(work);
                    }
                    Ok(None) => {}
                    Err(_) => {
                        if let Some(branch) = &source {
                            discovery.task_branches.push(branch.clone());
                        }
                        discovery.failures.push(RetainedWorkFailure {
                            run: Some(run.clone()), task: Some(task.id.clone()), branch: source,
                            disclosure: format!("Task {} kept work could not be inspected. Keep its working branch and try again. Opening or removing that work is unavailable until this is resolved.", task.id.0.as_str()),
                        });
                    }
                }
            }
        }
        *self.retained_task_work_cache.borrow_mut() = Some((key, discovery.clone()));
        Ok(discovery)
    }
    #[allow(
        clippy::too_many_lines,
        reason = "derive one task's authenticated source, receipt fate and person-facing state"
    )]
    fn inspect_retained_task(
        &self,
        state: &crate::orchestration_records::RunProjection,
        task: &crate::orchestration_records::TaskRecord,
        source_branch: &mut Option<BranchId>,
    ) -> Result<Option<RetainedTaskWork>, BundleError> {
        let bundle = self.active_bundle()?;
        let run = &state.run;
        let mut history = None;
        for reference in task
            .progress
            .decisions
            .iter()
            .rev()
            .filter(|r| r.record_kind == crate::bundle_records::RecordKind::RunIndex)
        {
            let record = self.store()?.read_run_record(reference)?;
            if let OrchestrationPayload::TaskHistory(saved) = &record.payload {
                // This hint can only deny an action; it never grants authority.
                *source_branch = Some(saved.source.branch.clone());
                if saved.project != self.project_id()?
                    || saved.run != *run
                    || saved.task != task.id
                    || saved.base != task.brief.scope.base
                {
                    return Err(BundleError::Identity(
                        "retained task history scope changed".into(),
                    ));
                }
                history = Some((**saved).clone());
                break;
            }
        }
        let Some(history) = history else {
            return Ok(None);
        };
        // Explicitly removed branches have no remaining recovery offer.
        if !bundle.branches.contains_key(&history.source.branch) {
            return Ok(None);
        }
        let incomplete = if let Some(intent) = task.progress.completion.as_deref() {
            let id = intent
                .step_id(CompletionStep::Publication)
                .map_err(|_| BundleError::HistoryChanged)?;
            self.orchestration_receipt(run, &id)?
                .is_none_or(|r| r.decision != CommandDecision::Applied)
        } else {
            false
        };
        let merge_receipt = history
            .merge_envelope
            .as_ref()
            .map(|envelope| self.manual_transition_receipt(envelope))
            .transpose()?;
        let merged = merge_receipt.as_ref().is_some_and(|receipt| {
            matches!(
                receipt,
                ReceiptLookup::Committed(_) | ReceiptLookup::Retired(_)
            )
        });
        let merge_pending = merge_receipt.is_some() && !merged;
        let merge_unresolved = merge_receipt.as_ref().is_some_and(|receipt| {
            matches!(
                receipt,
                ReceiptLookup::Pending
                    | ReceiptLookup::RetiredUnresolved
                    | ReceiptLookup::UnresolvedOrigin
            )
        });
        if merge_receipt == Some(ReceiptLookup::IdPayloadConflict) {
            return Err(BundleError::HistoryChanged);
        }
        let kind = if incomplete {
            RetainedWorkKind::Completion
        } else if merge_pending {
            RetainedWorkKind::Merge
        } else {
            RetainedWorkKind::Task
        };
        let settled = merged;
        let source = history.source;
        let available = bundle
            .branches
            .get(&source.branch)
            .is_some_and(|checkpoint| {
                checkpoint.head_id == source.head
                    && checkpoint.document_hash == source.document_hash
            });
        let manual = bundle.checkpoint(bundle.manual_branch_id())?;
        let open_on_timeline = bundle.manual_branch_id() == &source.branch
            || bundle
                .branches
                .get(&source.branch)
                .is_some_and(|checkpoint| checkpoint.history_ref == manual.history_ref);
        let reviewed_on = bundle.root.as_ref().and_then(|root| {
            root.retained_task_reviews
                .iter()
                .find(|(branch, original)| {
                    bundle.branches.contains_key(*branch) && **original == source
                })
                .map(|(branch, _)| branch.clone())
        });
        let stopped = state.outcome != RunOutcome::Pending;
        let cleanup_settled = state.cleanup == RunCleanup::Settled;
        let mut disclosure = if open_on_timeline {
            format!(
                "Task {} work is open on your timeline. Review it here, or keep this working branch for later.",
                task.id.0.as_str()
            )
        } else if let Some(branch) = &reviewed_on {
            format!(
                "Task {} work was opened for review on working branch {}. Later edits belong to that branch. The original task branch is kept for reference.",
                task.id.0.as_str(),
                branch.as_str()
            )
        } else if !available {
            format!(
                "Task {} retained work is bound to working branch {} at head {}. That branch has changed or was removed. Keep the original task history for review; this selection cannot replace it with another version.",
                task.id.0.as_str(),
                source.branch.as_str(),
                source.head.as_str()
            )
        } else if !stopped || !cleanup_settled {
            format!(
                "Task {} work is kept on working branch {}. Review becomes available when the run and its cleanup finish.",
                task.id.0.as_str(),
                source.branch.as_str()
            )
        } else {
            format!(
                "Task {} work is kept on working branch {}. Choose Save current edit, then open {} to review it on your timeline. Your current edit is saved first. You can also leave this branch for later review.",
                task.id.0.as_str(),
                source.branch.as_str(),
                source.branch.as_str()
            )
        };
        if merge_unresolved {
            disclosure.insert_str(0, "The earlier merge outcome could not be confirmed. ");
        }
        Ok(Some(RetainedTaskWork {
            run: run.clone(),
            task: task.id.clone(),
            kind,
            source,
            frozen: history.frozen,
            stopped,
            cleanup_settled,
            available,
            open_on_timeline,
            disclosure,
            settled,
            reviewed_on,
        }))
    }
    pub(super) fn authorize_retained_selection(
        &mut self,
        target: &RecoverySelection,
    ) -> Result<bool, BundleError> {
        let branch = match target {
            RecoverySelection::WorkingBranch(branch)
            | RecoverySelection::Checkpoint { branch, .. } => branch,
        };
        let discovery = self.retained_task_sources()?;
        if discovery
            .failures
            .iter()
            .any(|failure| failure.blocks_branch(branch))
        {
            return Err(BundleError::Identity(
                "retained task work could not be inspected".into(),
            ));
        }
        let retained = discovery
            .work
            .into_iter()
            .filter(|work| &work.source.branch == branch)
            .collect::<Vec<_>>();
        if retained.is_empty() {
            return Ok(false);
        }
        if retained
            .iter()
            .any(|work| !work.stopped || !work.cleanup_settled)
        {
            return Err(BundleError::Identity(
                "Stop the run and wait for cleanup before reviewing retained task work.".into(),
            ));
        }
        if retained.iter().any(|work| !work.available) {
            return Err(BundleError::HistoryChanged);
        }
        // Close only the already-acknowledged task registration. This preserves
        // its checkpoint and refuses any unsaved history or current-owner mismatch.
        let registrations = self
            .durable_sessions()
            .into_iter()
            .filter(|(_, guard)| guard.branch_id() == branch)
            .map(|(_, guard)| guard)
            .collect::<Vec<_>>();
        for guard in registrations {
            self.close_registered_task_checkpoint(&guard)?;
        }
        Ok(matches!(target, RecoverySelection::WorkingBranch(_)))
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use crate::{
        orchestration_commit::{CommandDecision, ValidatedRunUpdate, commit_orchestration},
        orchestration_records::*,
        receipts::{CommandEnvelope, CommandId},
        storage_budget::StorageLimits,
    };
    fn id<T: serde::de::DeserializeOwned>(n: u64) -> T {
        serde_json::from_value(serde_json::json!(format!("{n:032x}"))).unwrap()
    }
    #[test]
    fn te13_fix2_unsaved_session_has_no_history_inspection_failure() {
        let temp = kinewright_media::test_support::TempDirectory::new_in(
            &kinewright_media::test_support::workspace_target_dir(),
            "te13-fix2-unsaved",
        );
        let core = kinewright_core::Core::spawn(kinewright_core::Document::default()).unwrap();
        let owner = Ja1EditingSession::new(temp.root(), &core, StorageLimits::default()).unwrap();
        assert!(owner.active_bundle().is_err());
        let discovery = owner.retained_task_discovery();
        assert!(
            discovery.failures.is_empty(),
            "TE13_FIX2_UNSAVED_NO_INSPECTION_FAILURE {discovery:?}"
        );
        assert!(discovery.work.is_empty() && discovery.task_branches.is_empty());
    }
    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "a real retained H1 source and manual receipt lookup exercise cache invalidation"
    )]
    fn te13_fix2_retained_disclosure_tracks_in_memory_issuer_retirement() {
        let temp = kinewright_media::test_support::TempDirectory::new_in(
            &kinewright_media::test_support::workspace_target_dir(),
            "te13-fix2-cache",
        );
        let core = kinewright_core::Core::spawn(kinewright_core::Document::default()).unwrap();
        let mut owner =
            Ja1EditingSession::new(temp.root(), &core, StorageLimits::default()).unwrap();
        owner
            .first_save(&temp.path("cache.kinewright"), StorageLimits::default())
            .unwrap();
        let mut candidate = owner.fork_durable_branch("candidate").unwrap();
        let candidate_base_ref = owner.persist_durable_branch(&mut candidate).unwrap();
        let base = owner.candidate_binding(candidate.branch_id()).unwrap();
        let mut task_branch = owner.fork_durable_branch("task").unwrap();
        task_branch
            .core()
            .request(kinewright_core::Command::Do(
                kinewright_core::Operation::AddMarker {
                    marker: kinewright_core::Marker {
                        id: kinewright_core::MarkerId(1),
                        position: kinewright_core::TimeCode(1),
                        label: "task".into(),
                        color_token: 0,
                    },
                },
            ))
            .unwrap();
        let base_ref = owner.persist_durable_branch(&mut task_branch).unwrap();
        let source = owner.candidate_binding(task_branch.branch_id()).unwrap();
        let evidence = owner.admit_host_grant(b"cache fixture").unwrap();
        let (issuer, grant_ref) = owner
            .issue_metadata_authority("cache-test", None, b"cache authority")
            .unwrap();
        let brief = TaskBrief {
            plan: evidence.clone(),
            script: evidence.clone(),
            assignment: evidence,
            inputs: owner.current_inputs_ref().clone(),
            role: Role::Director,
            provider: ProviderSelectionRecord {
                connector: "scripted".into(),
                model: "fixture".into(),
                options: vec![],
            },
            scope: TaskScope {
                operations: std::collections::BTreeSet::new(),
                objects: std::collections::BTreeSet::new(),
                fields: std::collections::BTreeSet::new(),
                tracks: std::collections::BTreeSet::new(),
                bus_sidechain_closure: std::collections::BTreeSet::new(),
                asset_roots: vec![],
                base: base.clone(),
                consent_refs: vec![],
            },
        };
        let mut envelope = OrchestrationEnvelope {
            id: CommandId {
                issuer: issuer.clone(),
                sequence: 1,
            },
            digest: serde_json::from_value(serde_json::json!("0".repeat(64))).unwrap(),
            project: owner.project_id().unwrap(),
            expected_generation: owner.metadata_generation().unwrap(),
            authority: AuthorityBinding {
                grant_ref,
                owner_epoch: owner.store().unwrap().owner_epoch(),
                endpoint: None,
                attempt: None,
            },
            command: OrchestrationCommand::StartRun(Box::new(StartRun {
                director: brief.clone(),
            })),
        };
        envelope.digest = envelope.computed_digest().unwrap();
        let run: RunId = envelope.identity("run").unwrap();
        let task: TaskId = envelope.identity("task").unwrap();
        let merge = CommandEnvelope {
            switch: None,
            manual: None,
            version: 1,
            action: "edit".into(),
            id: CommandId {
                issuer: issuer.clone(),
                sequence: 100,
            },
            project_id: envelope.project.clone(),
            target_branch: base.branch.clone(),
            owner_epoch: envelope.authority.owner_epoch,
            lease: None,
            expected_head: Some(base.head.clone()),
            expected_revision: kinewright_core::TimelineRevision(0),
            command: Some(kinewright_core::JournalCommand::DoBatch(vec![])),
            comment_text: None,
            comment_strokes: None,
            evidence: vec![],
            transition: None,
        };
        let history = TaskHistoryRecord {
            project: envelope.project.clone(),
            run: run.clone(),
            task: task.clone(),
            attempt: id(4),
            base,
            source,
            base_ref,
            candidate_base_ref,
            frozen: true,
            merge_envelope: Some(merge.clone()),
            merge_authority: None,
            frozen_result: None,
        };
        let reference = owner
            .admit_scheduler_record(&OrchestrationRecord {
                version: 1,
                payload: OrchestrationPayload::TaskHistory(Box::new(history)),
            })
            .unwrap()
            .reference;
        let state = RunProjection {
            edit_consent: None,
            edit_approvals: Vec::new(),
            run: run.clone(),
            sequence: 1,
            control_revision: id(5),
            phase: RunPhase::Finished,
            outcome: RunOutcome::Cancelled(OutcomeDetail {
                code: "stopped_by_person".into(),
                detail_ref: None,
            }),
            wait: RunWait::None,
            cleanup: RunCleanup::Settled,
            tasks: vec![TaskRecord {
                id: task,
                parent: None,
                brief,
                state: TaskState::Queued,
                progress: TaskProgress {
                    decisions: vec![reference],
                    ..TaskProgress::default()
                },
            }],
            effects: vec![],
            attempts: vec![],
            deliveries: vec![],
            requests: vec![],
            results: vec![],
            usage: vec![],
        };
        let segment = EventSegment {
            run,
            first_sequence: 1,
            previous: None,
            events: vec![OrchestrationEvent {
                command: envelope.id.clone(),
                sequence: 1,
                projection: state.clone(),
            }],
        };
        let update = ValidatedRunUpdate::new(
            &envelope.project,
            envelope.expected_generation,
            &envelope,
            state,
            segment,
            CommandDecision::Applied,
        )
        .unwrap();
        commit_orchestration(&mut owner, update, None).unwrap();
        assert_eq!(
            owner.manual_transition_receipt(&merge).unwrap(),
            ReceiptLookup::Absent
        );
        let before = owner.retained_task_work().unwrap();
        assert_eq!(before.len(), 1);
        assert!(!before[0].disclosure.contains("earlier merge outcome"));
        let root = owner.root_sha256().unwrap();
        // This is the actual issuer state consumed by receipt_lookup. Root,
        // branch index and orchestration cut stay byte-identical.
        owner
            .active_bundle_mut()
            .unwrap()
            .issuers
            .get_mut(&issuer)
            .unwrap()
            .retired = true;
        assert_eq!(owner.root_sha256().unwrap(), root);
        assert_eq!(
            owner.manual_transition_receipt(&merge).unwrap(),
            ReceiptLookup::RetiredUnresolved
        );
        let after = owner.retained_task_work().unwrap();
        assert!(
            after[0]
                .disclosure
                .contains("earlier merge outcome could not be confirmed"),
            "TE13_FIX2_RECEIPT_CACHE_REFRESH"
        );
        assert_ne!(before, after);
        let original = after[0].source.clone();
        let missing = BranchId::fresh().unwrap();
        owner
            .active_bundle_mut()
            .unwrap()
            .root
            .as_mut()
            .unwrap()
            .retained_task_reviews
            .insert(missing, original);
        assert!(
            owner.retained_task_work().unwrap()[0].reviewed_on.is_none(),
            "TE13_FIX3_STALE_FORK_IGNORED"
        );
    }
}

#[cfg(all(test, not(target_os = "linux")))]
#[test]
fn te13_fix3_unsupported_session_refuses_without_io() {
    let temp = kinewright_media::test_support::TempDirectory::new("te13-unsupported");
    let core = kinewright_core::Core::spawn(kinewright_core::Document::default()).unwrap();
    let (result, trace) = crate::io_observer::observe(|| {
        Ja1EditingSession::new(
            temp.root(),
            &core,
            crate::storage_budget::StorageLimits::default(),
        )
    });
    assert!(matches!(result, Err(BundleError::DurabilityUnsupported)));
    assert!(trace.events.is_empty());
    assert_eq!(std::fs::read_dir(temp.root()).unwrap().count(), 0);
}

#[cfg(test)]
#[test]
fn te13_fix3_inspection_failure_branch_scope() {
    let first = BranchId::fresh().unwrap();
    let second = BranchId::fresh().unwrap();
    let mut failure = RetainedWorkFailure {
        run: None,
        task: None,
        branch: Some(first.clone()),
        disclosure: String::new(),
    };
    assert!(failure.blocks_branch(&first));
    assert!(!failure.blocks_branch(&second));
    failure.branch = None;
    assert!(failure.blocks_branch(&first) && failure.blocks_branch(&second));
}
