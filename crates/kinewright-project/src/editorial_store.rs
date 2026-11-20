#![allow(clippy::missing_errors_doc)]
use crate::{
    bundle::{BundleError, ProjectBundle},
    bundle_records::{ObjectRef, RecordKind},
    editing_session::Ja1EditingSession,
    editorial::{
        AuditPart, EditorialAudit, EditorialRationale, FrozenRulebook, MAX_AUDIT_ENTRIES,
        MAX_RULE_TEXT, MAX_RULES, RulebookApprover, RulebookDecision, RulebookProposal,
        RulebookRevision, valid_text, validate_part,
    },
    orchestration_records::Role,
    storage_budget::{StorageLimits, StorageReservation},
    store::{BundleRecord, record_sealed},
    studio_records::{MetadataChange, SnapshotDescriptor, SnapshotKey, VersionId, bounded, kind},
    studio_store::{PendingRulebookUpdate, ValidatedMetadataUpdate, record},
};

fn rulebook_edges(reference: Option<&ObjectRef>) -> Vec<ObjectRef> {
    reference.into_iter().cloned().collect()
}

fn rulebook_kind(reference: Option<&ObjectRef>) -> Result<(), BundleError> {
    if let Some(reference) = reference {
        kind(reference, RecordKind::RulebookRevision)?;
        reference.validate()?;
    }
    Ok(())
}

record!(
    EditorialRationale,
    EditorialRationale,
    |v: &EditorialRationale| rulebook_edges(v.0.rulebook_ref.as_ref()),
    |v: &EditorialRationale, _: &StorageLimits| validate_part(&v.0, Role::Editor)
);
record!(
    EditorialAudit,
    EditorialAudit,
    |v: &EditorialAudit| rulebook_edges(v.rulebook_ref()),
    |v: &EditorialAudit, _: &StorageLimits| {
        validate_part(&v.review, Role::Reviewer)?;
        if let Some(rationale) = &v.rationale {
            validate_part(rationale, Role::Editor)?;
            if &rationale.snapshot != v.snapshot()
                || rationale.rulebook_ref.as_ref() != v.rulebook_ref()
            {
                return Err(BundleError::Identity(
                    "editorial audit rationale binding".into(),
                ));
            }
        }
        if v.entries().count() > MAX_AUDIT_ENTRIES {
            return Err(BundleError::Identity(
                "editorial audit entries exceed the limit".into(),
            ));
        }
        Ok(())
    }
);
record!(
    RulebookRevision,
    RulebookRevision,
    |v: &RulebookRevision| vec![v.proposal_ref.clone()],
    |v: &RulebookRevision, _: &StorageLimits| {
        rulebook_kind(v.parent.as_ref())?;
        kind(&v.proposal_ref, RecordKind::RulebookProposal)?;
        if v.rules.is_empty()
            || v.rules.len() > MAX_RULES
            || v.rules.iter().any(|rule| !valid_text(rule, MAX_RULE_TEXT))
        {
            return Err(BundleError::Identity(
                "rulebook rules exceed the limit".into(),
            ));
        }
        Ok(())
    }
);
record!(
    RulebookProposal,
    RulebookProposal,
    |_: &RulebookProposal| vec![],
    |v: &RulebookProposal, _: &StorageLimits| {
        v.proposed_by
            .validate()
            .map_err(|_| BundleError::Corrupt("rulebook proposal attempt".into()))?;
        rulebook_kind(v.parent.as_ref())?;
        if !valid_text(&v.rule, MAX_RULE_TEXT) {
            return Err(BundleError::Identity(
                "rulebook rule text exceed the limit".into(),
            ));
        }
        Ok(())
    }
);

impl ProjectBundle {
    pub(crate) fn approved_rulebook(&self) -> Option<&ObjectRef> {
        self.root.as_ref()?.approved_rulebook_ref.as_ref()
    }

    fn pending_rulebook(&self) -> Option<&ObjectRef> {
        self.root.as_ref()?.pending_rulebook_ref.as_ref()
    }

    pub fn editorial_audit(
        &self,
        key: &SnapshotKey,
    ) -> Result<Option<StorageReservation<EditorialAudit>>, BundleError> {
        let snapshot = self.read_snapshot(key)?;
        snapshot
            .descriptor
            .audit_ref
            .as_ref()
            .map(|reference| self.store.read_record(reference))
            .transpose()
    }

    pub fn rulebook_proposal(
        &self,
    ) -> Result<Option<(ObjectRef, StorageReservation<RulebookProposal>)>, BundleError> {
        self.pending_rulebook()
            .map(|reference| Ok((reference.clone(), self.store.read_record(reference)?)))
            .transpose()
    }

    pub(super) fn stage_editorial(
        &mut self,
        change: &MetadataChange,
        pending: &mut ValidatedMetadataUpdate,
    ) -> Result<bool, BundleError> {
        match change {
            MetadataChange::RetainEditorial {
                target,
                role,
                frozen,
                part,
            } => {
                self.retain_editorial_audit(target, *role, frozen, part, pending)?;
            }
            MetadataChange::ProposeRulebook(proposal) => {
                if &proposal.proposed_by.project != self.project_id()
                    || proposal.parent.as_ref() != self.approved_rulebook()
                {
                    return Err(BundleError::Identity("rulebook proposal parent".into()));
                }
                if let Some(parent) = &proposal.parent
                    && self
                        .store
                        .read_record::<RulebookRevision>(parent)?
                        .rules
                        .len()
                        >= MAX_RULES
                {
                    return Err(BundleError::Identity(
                        "rulebook rules exceed the limit".into(),
                    ));
                }
                let sealed = self.store.admit_record(proposal)?;
                if let Some(existing) = self.pending_rulebook()
                    && existing != &sealed.reference
                {
                    return Err(BundleError::Identity(
                        "rulebook proposal already pending".into(),
                    ));
                }
                pending.pending_rulebook =
                    Some(PendingRulebookUpdate::Propose(sealed.reference.clone()));
                pending.seals.push(sealed);
            }
            MetadataChange::DecideRulebook(decision) => {
                self.approve_rulebook_revision(decision, pending)?;
            }
            _ => return Ok(false),
        }
        Ok(true)
    }

    fn retain_editorial_audit(
        &mut self,
        target: &VersionId,
        role: Role,
        frozen: &FrozenRulebook,
        part: &AuditPart,
        pending: &mut ValidatedMetadataUpdate,
    ) -> Result<(), BundleError> {
        if !matches!(role, Role::Editor | Role::Reviewer) {
            return Err(BundleError::Identity("editorial audit role".into()));
        }
        if &part.attempt.project != self.project_id() || part.rulebook_ref != frozen.revision {
            return Err(BundleError::Identity(
                "editorial audit frozen rulebook".into(),
            ));
        }
        validate_part(part, role)?;
        let original = self.version_index()?;
        let _index_charge = self.store.reserve(0, original.decoded_charge(), false)?;
        let mut index = original.take();
        let descriptor = index
            .snapshots
            .get_mut(target)
            .filter(|version| version.key == part.snapshot)
            .ok_or_else(|| BundleError::Identity("editorial audit snapshot".into()))?;
        let previous = descriptor
            .audit_ref
            .as_ref()
            .filter(|_| descriptor.rulebook_ref == frozen.revision)
            .map(|reference| self.store.read_record::<EditorialAudit>(reference))
            .transpose()?
            .map(StorageReservation::take);
        match (role, previous) {
            (Role::Editor, None) => {
                let sealed = self.store.admit_record(&EditorialRationale(part.clone()))?;
                descriptor.editorial_rationale_ref = Some(sealed.reference.clone());
                pending.seals.push(sealed);
            }
            (Role::Editor, Some(mut audit)) => {
                audit.rationale = Some(part.clone());
                self.stage_complete_audit(descriptor, &audit, pending)?;
            }
            (Role::Reviewer, previous) => {
                let staged = descriptor
                    .editorial_rationale_ref
                    .as_ref()
                    .map(|reference| self.store.read_record::<EditorialRationale>(reference))
                    .transpose()?
                    .filter(|rationale| rationale.0.rulebook_ref == frozen.revision);
                if staged.is_some() {
                    descriptor.editorial_rationale_ref = None;
                }
                let rationale = staged
                    .map(|r| r.take().0)
                    .or_else(|| previous.and_then(|audit| audit.rationale));
                self.stage_complete_audit(
                    descriptor,
                    &EditorialAudit {
                        review: part.clone(),
                        rationale,
                    },
                    pending,
                )?;
            }
            _ => return Err(BundleError::HistoryBarrierInvariant),
        }
        let sealed = self.store.admit_record(&index)?;
        pending.versions = Some(sealed.reference.clone());
        pending.seals.push(sealed);
        Ok(())
    }

    fn stage_complete_audit(
        &mut self,
        descriptor: &mut SnapshotDescriptor,
        audit: &EditorialAudit,
        pending: &mut ValidatedMetadataUpdate,
    ) -> Result<(), BundleError> {
        let sealed = self.store.admit_record(audit)?;
        descriptor.audit_ref = Some(sealed.reference.clone());
        descriptor.rulebook_ref = audit.rulebook_ref().cloned();
        pending.seals.push(sealed);
        Ok(())
    }

    fn approve_rulebook_revision(
        &mut self,
        decision: &RulebookDecision,
        pending: &mut ValidatedMetadataUpdate,
    ) -> Result<(), BundleError> {
        if self.pending_rulebook() != Some(&decision.proposal) {
            return Err(BundleError::Identity("rulebook proposal changed".into()));
        }
        let proposal = self
            .store
            .read_record::<RulebookProposal>(&decision.proposal)?
            .take();
        if &proposal.proposed_by.project != self.project_id()
            || proposal.parent.as_ref() != self.approved_rulebook()
        {
            return Err(BundleError::Identity("rulebook proposal parent".into()));
        }
        if decision.approve {
            let mut rules = proposal
                .parent
                .as_ref()
                .map(|r| self.store.read_record::<RulebookRevision>(r))
                .transpose()?
                .map_or_else(Vec::new, |r| r.take().rules);
            rules.push(proposal.rule);
            let revision = RulebookRevision {
                parent: proposal.parent,
                rules,
                proposal_ref: decision.proposal.clone(),
                approved_by: RulebookApprover::ProjectOwner(self.project_id().clone()),
            };
            let sealed = self.store.admit_record(&revision)?;
            pending.approved_rulebook = Some(sealed.reference.clone());
            pending.seals.push(sealed);
        }
        pending.pending_rulebook = Some(PendingRulebookUpdate::Clear);
        Ok(())
    }
}

impl Ja1EditingSession {
    pub fn frozen_rulebook(&self) -> Result<FrozenRulebook, BundleError> {
        Ok(FrozenRulebook {
            revision: self.active_bundle()?.approved_rulebook().cloned(),
        })
    }

    pub fn editorial_audit(
        &self,
        key: &SnapshotKey,
    ) -> Result<Option<StorageReservation<EditorialAudit>>, BundleError> {
        self.active_bundle()?.editorial_audit(key)
    }

    pub fn rulebook_proposal(
        &self,
    ) -> Result<Option<(ObjectRef, StorageReservation<RulebookProposal>)>, BundleError> {
        self.active_bundle()?.rulebook_proposal()
    }

    pub fn studio_versions_at(
        &self,
        checkpoint: &ObjectRef,
    ) -> Result<Vec<SnapshotKey>, BundleError> {
        Ok(self
            .active_bundle()?
            .version_index()?
            .snapshots
            .values()
            .filter(|d| &d.checkpoint_ref == checkpoint)
            .map(|d| d.key.clone())
            .collect())
    }

    pub fn decide_rulebook(&mut self, decision: RulebookDecision) -> Result<(), BundleError> {
        self.publish_editorial(MetadataChange::DecideRulebook(decision))
    }

    pub fn publish_editorial(&mut self, change: MetadataChange) -> Result<(), BundleError> {
        use crate::{
            receipts::{CommandId, ReceiptLookup},
            studio_records::{MetadataScope, MetadataUpdate},
        };
        self.reconcile_metadata()?;
        let issuer = self.reuse_metadata_issuer_for("studio-editorial")?;
        let sequence = self.metadata_issuer_sequence(&issuer)?;
        let envelope = self.metadata_envelope(CommandId { issuer, sequence })?;
        let update = MetadataUpdate {
            expected_root_generation: self.metadata_generation()?,
            change,
        };
        match self.metadata_command(&envelope, MetadataScope::Editorial, &update)? {
            ReceiptLookup::Committed(_) | ReceiptLookup::Retired(_) => Ok(()),
            _ => Err(BundleError::Unsettled),
        }
    }
}

pub(crate) fn validate_editorial_binding(
    descriptor: &SnapshotDescriptor,
    snapshot: &SnapshotKey,
    rulebook: Option<&ObjectRef>,
) -> Result<(), BundleError> {
    if snapshot != &descriptor.key || rulebook != descriptor.rulebook_ref.as_ref() {
        return Err(BundleError::Corrupt(
            "editorial audit version binding".into(),
        ));
    }
    Ok(())
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use crate::{
        bundle::BundleWriteAdmission,
        bundle_commit::BundleCommit,
        bundle_copy::SaveAsOutcome,
        bundle_records::SessionId,
        editorial::{AuditFinding, AuditKind, AuditObservation, AuditSubmission},
        operation_actor::Core,
        orchestration_records::ExecutionIdentity,
        receipts::{CommandId, ReceiptLookup, ReceiptResult},
        studio_records::{
            AudioRange, MetadataResult, MetadataScope, MetadataUpdate, PublishSnapshot,
        },
    };
    use kinewright_media::test_support::{TempDirectory, workspace_target_dir};

    fn s5_bundle(temp: &TempDirectory) -> ProjectBundle {
        let core = Core::for_test(kinewright_core::Document::default()).unwrap();
        let mut bundle = ProjectBundle::activate(
            &temp.path("studio.kinewright"),
            BundleWriteAdmission::new_document(),
            core.history_pool(),
        )
        .unwrap();
        let inputs = bundle.empty_inputs().unwrap();
        bundle.save(&core, &inputs.reference).unwrap();
        bundle
    }

    fn s5_publish(
        bundle: &mut ProjectBundle,
        scope: MetadataScope,
        change: MetadataChange,
    ) -> MetadataResult {
        let issuer = bundle.issue_metadata_issuer().unwrap();
        let envelope = bundle.metadata_envelope(CommandId {
            issuer,
            sequence: 1,
        });
        let update = MetadataUpdate {
            expected_root_generation: bundle.generation,
            change,
        };
        let ReceiptLookup::Committed(receipt) =
            bundle.metadata_command(&envelope, scope, &update).unwrap()
        else {
            panic!("S5_METADATA_COMMITTED");
        };
        let ReceiptResult::Metadata { result } = receipt.result else {
            panic!("S5_METADATA_RECEIPT");
        };
        result
    }

    fn s5_attempt(bundle: &ProjectBundle) -> ExecutionIdentity {
        let fresh = || SessionId::fresh().unwrap();
        ExecutionIdentity {
            version: 1,
            project: bundle.project_id().clone(),
            run: crate::orchestration_records::RunId(fresh()),
            task: crate::orchestration_records::TaskId(fresh()),
            attempt: crate::orchestration_records::AttemptId(fresh()),
            nonce: fresh(),
            owner_instance: fresh(),
        }
    }

    fn s5_version_with_audit(bundle: &mut ProjectBundle) -> (SnapshotKey, ObjectRef) {
        let checkpoint_ref = bundle
            .checkpoint_reference(bundle.manual_branch_id())
            .unwrap();
        let change = MetadataChange::PublishSnapshot(PublishSnapshot {
            version: VersionId::fresh().unwrap(),
            expected_head: bundle.manual_head().unwrap().clone(),
            checkpoint_ref,
            parent_version: None,
            run_provenance: None,
            revision_refs: vec![],
        });
        let MetadataResult::Snapshot(key) = s5_publish(bundle, MetadataScope::Snapshot, change)
        else {
            panic!("S5_SNAPSHOT_RECEIPT");
        };
        let frozen = FrozenRulebook {
            revision: bundle.approved_rulebook().cloned(),
        };
        let submission = AuditSubmission {
            version: key.version.clone(),
            snapshot: key.clone(),
            rulebook_ref: frozen.revision.clone(),
            entries: [AuditKind::Retention, AuditKind::Hook, AuditKind::Story]
                .into_iter()
                .map(|kind| AuditObservation {
                    kind,
                    line_or_beat: "Opening".into(),
                    output: AudioRange {
                        start_micros: 0,
                        end_micros: 1000,
                    },
                    text: "The opening works.".into(),
                    finding: AuditFinding::Resolved,
                })
                .collect(),
        };
        let (target, part) = submission.attribute(s5_attempt(bundle));
        s5_publish(
            bundle,
            MetadataScope::Editorial,
            MetadataChange::RetainEditorial {
                target,
                role: Role::Reviewer,
                frozen,
                part,
            },
        );
        let reference = bundle
            .read_snapshot(&key)
            .unwrap()
            .descriptor
            .audit_ref
            .unwrap();
        (key, reference)
    }

    #[test]
    fn s5_collection_reclaims_only_the_discarded_versions_audit() {
        let temp = TempDirectory::new_in(&workspace_target_dir(), "s5-editorial-collect");
        let mut bundle = s5_bundle(&temp);
        let (discarded, discarded_audit) = s5_version_with_audit(&mut bundle);
        let (kept, kept_audit) = s5_version_with_audit(&mut bundle);

        // Version removal itself has no Studio action. Publish the version index
        // that a removal leaves behind through the same durable metadata path.
        let mut index = bundle.version_index().unwrap().take();
        assert!(index.snapshots.remove(&discarded.version).is_some());
        let sealed = bundle.store.admit_record(&index).unwrap();
        bundle.pending_metadata = Some(ValidatedMetadataUpdate {
            versions: Some(sealed.reference.clone()),
            seals: vec![sealed],
            ..Default::default()
        });
        BundleCommit::publish_maintenance(&mut bundle).unwrap();
        bundle.pending_metadata = None;
        // Two more acknowledgments release the preceding recovery roots.
        for _ in 0..2 {
            BundleCommit::publish_maintenance(&mut bundle).unwrap();
        }
        assert!(bundle.collect_after_ack().unwrap() > 0);
        assert!(
            bundle.store.verify(&discarded_audit).is_err(),
            "S5_DISCARDED_AUDIT_COLLECTED"
        );
        assert_eq!(
            bundle.read_snapshot(&kept).unwrap().descriptor.audit_ref,
            Some(kept_audit)
        );
        assert_eq!(
            bundle.editorial_audit(&kept).unwrap().unwrap().snapshot(),
            &kept
        );
    }

    #[test]
    fn s5_save_as_keeps_approved_rules_and_exact_audits() {
        let temp = TempDirectory::new_in(&workspace_target_dir(), "s5-editorial-copy");
        let mut bundle = s5_bundle(&temp);
        let proposal = RulebookProposal {
            proposed_by: s5_attempt(&bundle),
            parent: None,
            rule: "Keep the promise.".into(),
        };
        s5_publish(
            &mut bundle,
            MetadataScope::Editorial,
            MetadataChange::ProposeRulebook(proposal),
        );
        let proposal = bundle.pending_rulebook().unwrap().clone();
        s5_publish(
            &mut bundle,
            MetadataScope::Editorial,
            MetadataChange::DecideRulebook(RulebookDecision {
                proposal,
                approve: true,
            }),
        );
        let approved = bundle.approved_rulebook().unwrap().clone();
        let (key, audit) = s5_version_with_audit(&mut bundle);
        let prior = bundle.editorial_audit(&key).unwrap().unwrap().take();
        let proposal = RulebookProposal {
            proposed_by: s5_attempt(&bundle),
            parent: Some(approved.clone()),
            rule: "Pending copy check.".into(),
        };
        s5_publish(
            &mut bundle,
            MetadataScope::Editorial,
            MetadataChange::ProposeRulebook(proposal),
        );
        let SaveAsOutcome::Copied(copy) = bundle.save_as(&temp.path("copy.kinewright")).unwrap()
        else {
            panic!("S5_COPY");
        };
        let copied = copy
            .into_destination()
            .unwrap_or_else(|_| panic!("S5_COPY_COMMITTED"));
        assert_ne!(copied.project_id(), bundle.project_id());
        assert_eq!(
            copied.approved_rulebook(),
            Some(&approved),
            "S5_COPY_APPROVED_RULEBOOK"
        );
        assert_eq!(
            copied
                .store
                .read_record::<RulebookRevision>(&approved)
                .unwrap()
                .rules,
            ["Keep the promise."]
        );
        assert_eq!(
            copied.read_snapshot(&key).unwrap().descriptor.audit_ref,
            Some(audit)
        );
        assert_eq!(*copied.editorial_audit(&key).unwrap().unwrap(), prior);
        assert!(
            copied.rulebook_proposal().unwrap().is_none(),
            "S5_COPY_HAS_NO_PENDING_AUTHORITY"
        );
        assert!(bundle.rulebook_proposal().unwrap().is_some());
    }
}
