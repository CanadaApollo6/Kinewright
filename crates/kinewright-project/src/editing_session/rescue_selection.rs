//! Read-only A1 choice order and selected source closure; no destination namespace is created.
use super::{
    BundleError, FrozenRescueCapture, Ja1EditingSession, MissingOwnedUse, ObjectRef,
    OwnedInputManifest, ProjectBundle, ProjectStore, RetainedDestination, StorageLimits,
    StorageReservation,
};
use crate::{
    bundle_records::RecordKind, bundle_rescue::RescueSelection, rescue_record::RecoveryOmission,
};
use kinewright_core::HistorySnapshot;

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
        for reference in &bundle.evidence {
            if reference.record_kind == RecordKind::OwnedEvidence
                && crate::rescue_record::parse(&bundle.store.read_copy_evidence(reference, true)?)?
                    .is_some()
            {
                continue;
            }
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
    #[allow(
        clippy::too_many_lines,
        reason = "one selected content union, readability proof and existing floor admission"
    )]
    fn rescue_graph(
        &self,
        history: &HistorySnapshot,
        omitted: &[RecoveryOmission],
        limits: &StorageLimits,
    ) -> Result<Vec<ObjectRef>, BundleError> {
        let (source, base) = self.capture_source()?;
        let mut roots = base.into_iter().map(|(r, _)| r).collect::<Vec<_>>();
        let inputs = self.rescue_source_inputs()?;
        let mut retained = (*source.read_copy_record::<OwnedInputManifest>(&inputs, true)?).clone();
        let named =
            self.association.as_ref().is_some_and(|a| a.saved.is_some()) || self.source.is_none();
        if named {
            let bundle = self.active_bundle()?;
            let prior = bundle.checkpoint(&bundle.manual_branch_id)?;
            for reference in &prior.history_input_manifest_refs {
                retained.entries.extend(
                    source
                        .read_copy_record::<OwnedInputManifest>(reference, true)?
                        .entries
                        .clone(),
                );
            }
            let original = source.read_copy_record::<HistorySnapshot>(&prior.history_ref, true)?;
            let cut = usize::try_from(history.floor_offset.saturating_sub(original.floor_offset))
                .map_err(|_| BundleError::Capacity("rescue history cut"))?;
            let contexts = &prior.history_input_manifest_refs;
            if cut == 0 {
                roots.extend(contexts.iter().cloned());
            } else {
                roots.extend(contexts.iter().skip(cut + 1).cloned());
            }
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
        crate::bundle_commit::retain_history_inputs(history, &mut retained);
        crate::bundle_commit::validate_inputs(history, &retained)?;
        for entry in &retained.entries {
            roots.push(entry.object.clone());
            roots.extend(entry.dependencies.iter().cloned());
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
        let encoded_history =
            serde_json::to_vec(history).map_err(|e| BundleError::Corrupt(e.to_string()))?;
        let decoded_history = crate::store::preflight_record(&encoded_history, limits)?;
        let metadata = closure
            .iter()
            .filter(|r| !r.record_kind.is_owned_bytes())
            .try_fold(0_u64, |n, r| {
                n.checked_add(r.byte_length)
                    .ok_or(BundleError::Capacity("rescue metadata overflow"))
            })?;
        if decoded_history > limits.live_history_bytes
            || metadata
                .checked_add(decoded_history)
                .is_none_or(|n| n > limits.metadata_bytes)
        {
            return Err(BundleError::Capacity("rescue history/metadata admission"));
        }

        let bytes = closure.iter().try_fold(
            history
                .encoded_bytes()
                .map_err(crate::bundle::history_error)?,
            |n, r| {
                n.checked_add(r.byte_length)
                    .ok_or(BundleError::Capacity("rescue graph bytes"))
            },
        )?;
        if bytes > limits.owned_bytes
            || bytes > limits.copy_overlap_bytes
            || closure.len() as u64 > limits.objects
        {
            return Err(BundleError::Capacity("rescue selected graph/overlap"));
        }
        let ordinary = crate::bundle_commit::select_history(history, limits)?;
        if ordinary.history != *history {
            return Err(BundleError::Capacity("rescue selected branch history"));
        }
        Ok(roots)
    }
    pub(super) fn preview_rescue_inner(
        &self,
        capture: StorageReservation<FrozenRescueCapture>,
        limits: StorageLimits,
    ) -> Result<RescueSelection, BundleError> {
        limits.validate()?;
        if self
            .attachment
            .capture()
            .map_err(crate::bundle::history_error)?
            != capture.cut
        {
            return Err(BundleError::HistoryChanged);
        }
        let named =
            self.association.as_ref().is_some_and(|a| a.saved.is_some()) || self.source.is_none();
        let (source, _) = self.capture_source()?;
        let unavailable = self.unavailable_recovery(source, named)?;
        let before = capture.cut.history();
        let full = self.rescue_graph(before, &[], &limits);
        let (history, omissions, roots) = if let Ok(roots) = full {
            (before.clone(), vec![], roots)
        } else if let Ok(roots) = self.rescue_graph(before, &unavailable, &limits) {
            (before.clone(), unavailable, roots)
        } else {
            // The common selector still supplies the ordinary earliest branch floor. Additional
            // graph/readability predicates use Core's same legal oldest-first transition search.
            let ordinary = crate::bundle_commit::select_history(before, &limits)?;
            let selected = if self
                .rescue_graph(&ordinary.history, &unavailable, &limits)
                .is_ok()
            {
                ordinary.history
            } else {
                kinewright_core::HistoryTransition::find_floor(
                    before.clone(),
                    |_, _, _| true,
                    |candidate| self.rescue_graph(candidate, &unavailable, &limits).is_ok(),
                )
                .map_err(|_| {
                    self.rescue_graph(before, &unavailable, &limits)
                        .unwrap_err()
                })?
                .after()
                .clone()
            };
            let roots = self.rescue_graph(&selected, &unavailable, &limits)?;
            (selected, unavailable, roots)
        };
        let removed_undo = before
            .cursor
            .undo
            .len()
            .saturating_sub(history.cursor.undo.len()) as u64;
        let selected_digest = kinewright_media::sha256_bytes(
            &serde_json::to_vec(&(&history, &omissions, &roots, format!("{limits:?}")))
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
            history,
            limits,
            selected_digest,
            removed_undo,
            omissions,
            roots,
            _reads: reads,
        })
    }
}
