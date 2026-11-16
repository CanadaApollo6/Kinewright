//! Read-only A1 choice order and selected source closure; no destination namespace is created.
use super::{
    BundleError, FrozenRescueCapture, Ja1EditingSession, MissingOwnedUse, ObjectRef, ProjectBundle,
    ProjectStore, RetainedDestination, StorageLimits, StorageReservation,
};
use crate::{
    bundle_records::RecordKind, bundle_rescue::RescueSelection, rescue_record::RecoveryOmission,
};

impl Ja1EditingSession {
    pub(super) fn rescue_required_roots(
        bundle: &ProjectBundle,
    ) -> Result<Vec<ObjectRef>, BundleError> {
        let root = bundle.root.as_ref().ok_or(BundleError::Unsettled)?;
        let mut roots = root.references();
        roots.retain(|r| {
            r != &root.branch_index_ref
                && r != &root.store_manifest_ref
                && r != &root.input_manifest_ref
        });
        let index = bundle.store.read_copy_record::<std::collections::BTreeMap<
            crate::bundle_records::BranchId,
            ObjectRef,
        >>(&root.branch_index_ref, true)?;
        roots.extend(
            index
                .iter()
                .filter(|(id, _)| **id != bundle.manual_branch_id)
                .map(|(_, r)| r.clone()),
        );
        for reference in &bundle.acknowledged_evidence()? {
            if reference.record_kind == RecordKind::OwnedEvidence
                && let Ok(bytes) = bundle.store.read_copy_evidence(reference, true)
                && crate::rescue_record::parse(&bytes)?.is_some()
            {
                continue;
            }
            // Unreadable evidence remains a required root. Retained admission
            // reports its exact missing bytes/uses instead of losing that diagnostic
            // while trying to classify an unavailable rescue record.
            roots.push(reference.clone());
        }
        Ok(roots)
    }
    fn rescue_source_inputs(&self) -> Result<ObjectRef, BundleError> {
        if self.association.as_ref().is_some_and(|a| a.saved.is_none())
            && self.source.is_some()
            && let Some(RetainedDestination::FirstSave(copy)) = &self.destination
        {
            return copy.source_manifest(&self.inputs);
        }
        Ok(self.inputs.clone())
    }
    fn unavailable_recovery(
        &self,
        source: &ProjectStore,
        named: bool,
    ) -> Result<Vec<RecoveryOmission>, BundleError> {
        if !named {
            return Ok(vec![]);
        }
        let bundle = self.active_bundle()?;
        let mut candidates = vec![];
        for (i, root) in bundle.recovery.predecessors.iter().enumerate() {
            candidates.push((format!("predecessor:{i}"), root.clone(), vec![root.clone()]));
        }
        if let Some(root) = &bundle.recovery.pre_migration {
            candidates.push(("pre_migration".into(), root.clone(), vec![root.clone()]));
        }
        let raw = bundle.admission.raw_bytes();
        let acknowledged = ObjectRef {
            sha256: kinewright_media::sha256_bytes(raw),
            byte_length: raw.len() as u64,
            record_kind: RecordKind::RootCheckpoint,
        };
        candidates.push((
            "acknowledged_root".into(),
            acknowledged,
            bundle.root_references()?,
        ));
        let mut unavailable = vec![];
        for (role, root, refs) in candidates {
            let roots = refs
                .into_iter()
                .map(|r| {
                    (
                        r,
                        MissingOwnedUse::Recovery {
                            root_sha256: root.sha256.clone(),
                        },
                    )
                })
                .collect::<Vec<_>>();
            let missing = source.diagnose_retained_content_reads(&roots, 0, 32)?;
            if !missing.entries.is_empty() || !missing.dependencies_complete {
                unavailable.push(RecoveryOmission {
                    role,
                    root,
                    operation: "verify retained recovery closure".into(),
                    cause: missing.to_string(),
                });
            }
        }
        Ok(unavailable)
    }
    #[expect(
        clippy::too_many_lines,
        reason = "the selected full owner, cold archives, explicit omissions and destination forecasts form one closure admission"
    )]
    fn rescue_graph(
        &self,
        capture: &FrozenRescueCapture,
        omitted: &[RecoveryOmission],
        limits: &StorageLimits,
    ) -> Result<Vec<ObjectRef>, BundleError> {
        let (source, base) = self.capture_source()?;
        let mut roots = base.into_iter().map(|(r, _)| r).collect::<Vec<_>>();
        roots.push(self.rescue_source_inputs()?);
        capture.with_snapshot(|snapshot| {
            roots.extend(snapshot.record_references().cloned());
            // The current manifest alone cannot cover payloads reachable only
            // through Undo/Redo. Resolve the selected owner's entire resource
            // union through its retained certificates before offering omission.
            let current = self.rescue_source_inputs()?;
            let mut contexts = snapshot
                .record_references()
                .filter(|reference| reference.record_kind == RecordKind::InputManifest)
                .cloned()
                .collect::<std::collections::BTreeSet<_>>();
            contexts.insert(current.clone());
            if self.source.is_none() || self.association.as_ref().is_some_and(|a| a.saved.is_some())
            {
                let bundle = self.active_bundle()?;
                contexts.insert(
                    bundle
                        .checkpoint(bundle.manual_branch_id())?
                        .owner_ref
                        .clone(),
                );
            }
            let bindings =
                crate::operation_publication::retained_bindings(source, &contexts, &current)?;
            for resource in snapshot.resources() {
                if let Some(reference) = bindings.get(resource) {
                    roots.push(reference.clone());
                } else if !matches!(
                    resource,
                    crate::operation_history::Resource::Media { sha256: None, .. }
                ) {
                    return Err(crate::operation_history::Error::MissingResource.into());
                }
            }
            Ok(())
        })?;
        let named =
            self.association.as_ref().is_some_and(|a| a.saved.is_some()) || self.source.is_none();
        if named {
            let bundle = self.active_bundle()?;
            for (i, root) in bundle.recovery.predecessors.iter().enumerate() {
                if !omitted.iter().any(|o| o.role == format!("predecessor:{i}")) {
                    roots.push(root.clone());
                }
            }
            if let Some(root) = &bundle.recovery.pre_migration
                && !omitted.iter().any(|o| o.role == "pre_migration")
            {
                roots.push(root.clone());
            }
            if !omitted.iter().any(|o| o.role == "acknowledged_root") {
                roots.extend(bundle.root_references()?);
            }
        }
        // Exact cold archives replace older in-memory rescue holders, never a
        // second truth about their fates. Only this session's bounded attempts retain them.
        for attempt in &self.rescue.attempts {
            let archive = attempt
                .capture
                .archive
                .lock()
                .map_err(|_| BundleError::Unsettled)?;
            let store = attempt
                .capture
                .archive_store
                .lock()
                .map_err(|_| BundleError::Unsettled)?;
            if let (Some(reference), Some(store)) = (archive.as_ref(), store.as_ref()) {
                let closure =
                    store.closure_with_inventory(std::slice::from_ref(reference), false)?;
                for reference in closure {
                    store.admit_rescue_record_profile(&reference, limits)?;
                }
            }
        }
        roots.sort();
        roots.dedup();
        let diagnostic = source.diagnose_retained_content_reads(
            &roots
                .iter()
                .cloned()
                .map(|r| (r, MissingOwnedUse::Current))
                .collect::<Vec<_>>(),
            0,
            32,
        )?;
        if !diagnostic.entries.is_empty() || !diagnostic.dependencies_complete {
            return Err(BundleError::RescueMissingOwnedBytes(Box::new(diagnostic)));
        }
        let closure = source.closure_with_inventory(&roots, false)?;
        for reference in &closure {
            source.admit_rescue_record_profile(reference, limits)?;
        }
        let bytes = capture.with_snapshot(|snapshot| Ok(snapshot.encoded_bytes()?))?;
        let bytes = closure.iter().try_fold(bytes, |n, r| {
            n.checked_add(r.byte_length)
                .ok_or(BundleError::Capacity("rescue graph bytes"))
        })?;
        if bytes > limits.owned_bytes
            || bytes > limits.copy_overlap_bytes
            || closure.len() as u64 > limits.objects
        {
            return Err(BundleError::Capacity("rescue selected graph/overlap"));
        }
        Ok(roots)
    }
    pub(super) fn preview_rescue_inner(
        &self,
        capture: std::sync::Arc<StorageReservation<FrozenRescueCapture>>,
        limits: StorageLimits,
    ) -> Result<RescueSelection, BundleError> {
        limits.validate()?;
        if limits.objects == 0 || limits.metadata_bytes == 0 || limits.decoded_record_bytes == 0 {
            return Err(BundleError::Capacity("rescue record profile"));
        }
        if self
            .attachment
            .capture()
            .map_err(crate::bundle::operation_error)?
            != capture.cut
        {
            return Err(BundleError::HistoryChanged);
        }
        let named =
            self.association.as_ref().is_some_and(|a| a.saved.is_some()) || self.source.is_none();
        let (source, _) = self.capture_source()?;
        let unavailable = self.unavailable_recovery(source, named)?;
        let (omissions, roots) = match self.rescue_graph(&capture, &[], &limits) {
            Ok(roots) => (vec![], roots),
            Err(_) => (
                unavailable.clone(),
                self.rescue_graph(&capture, &unavailable, &limits)?,
            ),
        };
        // Capacity is a typed refusal. Only Core folds; rescue never trims the selected history.
        let removed_undo = 0;
        let selected_digest = kinewright_media::sha256_bytes(
            &serde_json::to_vec(&(
                capture.cut.identity(),
                &omissions,
                &roots,
                format!("{limits:?}"),
            ))
            .map_err(|e| BundleError::Corrupt(e.to_string()))?,
        );
        let reads = source.admit_retained_content_reads(
            &roots
                .iter()
                .cloned()
                .map(|r| (r, MissingOwnedUse::Manifest))
                .collect::<Vec<_>>(),
        )?;
        Ok(RescueSelection {
            capture,
            limits,
            selected_digest,
            removed_undo,
            omissions,
            roots,
            reads,
        })
    }
}
