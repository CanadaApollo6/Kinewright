use super::{BundleError, Ja1EditingSession};
use crate::{
    bundle::BundleSaveReport,
    bundle_records::ObjectRef,
    manual_history::ManualLease,
    owned_inputs::{OwnedInputManifest, PreparedOwnedMedia},
    store::ProjectStore,
};
use kinewright_core::{AssetId, Command, Core, Event, Operation};
use std::path::Path;

impl Ja1EditingSession {
    #[must_use]
    pub fn current_inputs_ref(&self) -> &ObjectRef {
        &self.inputs
    }

    pub fn prepare_media_interpretation(
        &mut self,
        asset: &kinewright_core::MediaAsset,
    ) -> Result<PreparedOwnedMedia, BundleError> {
        if self.destination.is_some() {
            self.require_local_writer()?;
        } else if self.host_retention_lost {
            return Err(BundleError::HistoryChanged);
        }
        if self.prepared.is_some()
            || self.pending_import_source.is_some()
            || self.recovery_save_pending
            || !self.retained_destinations.is_empty()
        {
            return Err(BundleError::Unsettled);
        }
        let history = self
            .core
            .export_history()
            .map_err(crate::bundle::history_error)?;
        let current = history
            .document
            .media_pool
            .iter()
            .find(|current| current.id == asset.id)
            .ok_or_else(|| BundleError::Identity("interpretation asset is not installed".into()))?;
        if current.source_fingerprint != asset.source_fingerprint
            || current.kind != asset.kind
            || current.fps != asset.fps
            || current.duration != asset.duration
            || current.resolution != asset.resolution
        {
            return Err(BundleError::Identity(
                "interpretation cannot relink to an unadmitted source".into(),
            ));
        }
        let previous = self.inputs.clone();
        let store = if let Some(destination) = &mut self.destination {
            destination.bundle_mut()?.store_mut()?
        } else {
            self.source
                .as_mut()
                .ok_or(BundleError::HistoryBarrierInvariant)?
                .store_mut()
        };
        store.prepare_media_interpretation(&previous, asset)
    }

    /// The local writer stages colour admission before publishing the matching Core command.
    #[allow(
        clippy::too_many_lines,
        reason = "Saved command publication and unsaved checkpoint installation retain their own atomic boundaries"
    )]
    pub fn apply_owned_media_interpretation(
        &mut self,
        asset: &kinewright_core::MediaAsset,
    ) -> Result<(), BundleError> {
        self.require_local_writer()?;
        let prepared = self.prepare_media_interpretation(asset)?;
        let retained_entry = prepared
            .manifest
            .entries
            .iter()
            .find(|entry| entry.matches_media_asset(asset))
            .ok_or(BundleError::HistoryBarrierInvariant)?
            .clone();
        let retained = self
            .store()?
            .lease_input(&crate::owned_inputs::OwnedInputRef {
                manifest_ref: prepared.inputs_ref.clone(),
                entry: retained_entry,
            })?;
        if self.destination.is_none() {
            let before = self
                .core
                .export_history()
                .map_err(crate::bundle::history_error)?;
            let candidate = Core::spawn((*before.baseline).clone())
                .map_err(|error| BundleError::Corrupt(error.to_string()))?;
            crate::ProjectAttachment::new(&candidate)
                .map_err(crate::bundle::history_error)?
                .restore_history(before.clone())
                .map_err(crate::bundle::history_error)?;
            if !matches!(
                candidate
                    .request(Command::Do(Operation::SetAssetColorDescription {
                        asset: asset.id,
                        color_description: asset.color_description.clone(),
                    }))
                    .map_err(|error| BundleError::Corrupt(error.to_string()))?,
                Event::DocumentChanged { .. }
            ) {
                return Err(BundleError::Unsupported(
                    "Core rejected colour interpretation".into(),
                ));
            }
            let after = candidate
                .export_history()
                .map_err(crate::bundle::history_error)?;
            crate::bundle_commit::validate_inputs(&after, &prepared.manifest)?;
            self.attachment
                .install_checkpoint(before, after)
                .map_err(crate::bundle::history_error)?;
            self.inputs.clone_from(&prepared.inputs_ref);
            self.input_leases.push(retained);
            return Ok(());
        }
        let core = self.core.clone();
        let revision = self
            .attachment
            .capture()
            .map_err(crate::bundle::history_error)?
            .revision();
        let bundle = self
            .destination
            .as_mut()
            .ok_or_else(|| BundleError::Identity("choose a first-save destination".into()))?
            .bundle_mut()?;
        let issuer = bundle.issue_issuer(&core, &self.inputs, 1)?;
        let envelope = crate::receipts::CommandEnvelope {
            switch: None,
            manual: None,
            transition: None,
            version: 1,
            action: "edit".into(),
            id: crate::receipts::CommandId {
                issuer,
                sequence: 1,
            },
            project_id: bundle.project_id().clone(),
            target_branch: bundle.manual_branch_id().clone(),
            owner_epoch: bundle.store().owner_epoch(),
            lease: None,
            expected_head: bundle.manual_head().cloned(),
            expected_revision: revision,
            command: Some(kinewright_core::JournalCommand::Do(
                Operation::SetAssetColorDescription {
                    asset: asset.id,
                    color_description: asset.color_description.clone(),
                },
            )),
            comment_text: None,
            comment_strokes: None,
            evidence: vec![],
        };
        crate::bundle_commit::BundleCommit::command(
            bundle,
            &envelope,
            &core,
            &prepared.inputs_ref,
            &[],
        )?;
        self.inputs.clone_from(&prepared.inputs_ref);
        self.input_leases.push(retained);
        Ok(())
    }

    pub fn prepare_owned_media(
        &mut self,
        path: &Path,
        asset_id: AssetId,
    ) -> Result<PreparedOwnedMedia, BundleError> {
        self.prepare_owned_media_with_color(path, asset_id, None)
    }
    pub fn prepare_owned_media_with_color(
        &mut self,
        path: &Path,
        asset_id: AssetId,
        color: Option<kinewright_core::ColorDescription>,
    ) -> Result<PreparedOwnedMedia, BundleError> {
        if self.destination.is_some() {
            self.require_local_writer()?;
        } else if self.host_retention_lost {
            return Err(BundleError::HistoryChanged);
        }
        if self.prepared.is_some()
            || self.pending_import_source.is_some()
            || self.recovery_save_pending
            || !self.retained_destinations.is_empty()
        {
            return Err(BundleError::Unsettled);
        }
        let before = self
            .core
            .export_history()
            .map_err(crate::bundle::history_error)?;
        if before
            .document
            .media_pool
            .iter()
            .any(|asset| asset.id == asset_id)
        {
            return Err(BundleError::Identity("asset ID already exists".into()));
        }
        let previous = self.inputs.clone();
        let store = if let Some(destination) = &mut self.destination {
            destination.bundle_mut()?.store_mut()?
        } else {
            self.source
                .as_mut()
                .ok_or(BundleError::HistoryBarrierInvariant)?
                .store_mut()
        };
        prepare(store, &previous, path, asset_id, color)
    }

    pub fn issue_manual_lease_for_inputs(
        &mut self,
        client: String,
        token: String,
        prepared: &PreparedOwnedMedia,
    ) -> Result<ManualLease, BundleError> {
        self.require_local_writer()?;
        let current = self.manual_checkpoint()?.1.input_manifest_ref;
        crate::bundle_commit::validate_manifest_extension(
            self.store()?,
            &current,
            &prepared.inputs_ref,
        )?;
        if *self.store()?.read_input_manifest(&prepared.inputs_ref)? != prepared.manifest
            || !prepared
                .manifest
                .entries
                .iter()
                .any(|entry| entry.matches_media_asset(&prepared.asset))
        {
            return Err(BundleError::Identity(
                "staged media admission differs".into(),
            ));
        }
        // Issuer publication must retain this extension before the GUI can accept it.
        let previous = std::mem::replace(&mut self.inputs, prepared.inputs_ref.clone());
        let lease = self.issue_manual_lease(client, token);
        if lease.is_err() {
            self.inputs = previous;
        }
        lease
    }

    /// Local source ownership remains in this process even while an unsaved host is attached.
    pub fn import_owned_media(
        &mut self,
        path: &Path,
    ) -> Result<(kinewright_core::MediaAsset, Option<BundleSaveReport>), BundleError> {
        self.import_owned_media_with_color(path, None)
    }
    #[allow(
        clippy::too_many_lines,
        reason = "Admission and rollback stay ordered with the document publication"
    )]
    pub fn import_owned_media_with_color(
        &mut self,
        path: &Path,
        color: Option<kinewright_core::ColorDescription>,
    ) -> Result<(kinewright_core::MediaAsset, Option<BundleSaveReport>), BundleError> {
        if self.destination.is_some() {
            self.require_local_writer()?;
        }
        let before = self
            .core
            .export_history()
            .map_err(crate::bundle::history_error)?;
        let next = before
            .document
            .media_pool
            .iter()
            .map(|asset| asset.id.0)
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or(BundleError::Capacity("asset ID overflow"))?;
        let prepared = self.prepare_owned_media_with_color(path, AssetId(next), color)?;
        let candidate = Core::spawn((*before.baseline).clone())
            .map_err(|e| BundleError::Corrupt(e.to_string()))?;
        let attachment =
            crate::ProjectAttachment::new(&candidate).map_err(crate::bundle::history_error)?;
        attachment
            .restore_history(before.clone())
            .map_err(crate::bundle::history_error)?;
        if !matches!(
            candidate
                .request(Command::Do(Operation::AddAsset {
                    asset: prepared.asset.clone()
                }))
                .map_err(|e| BundleError::Corrupt(e.to_string()))?,
            Event::DocumentChanged { .. }
        ) {
            return Err(BundleError::Unsupported(
                "Core rejected imported asset".into(),
            ));
        }
        let after = candidate
            .export_history()
            .map_err(crate::bundle::history_error)?;
        crate::bundle_commit::validate_inputs(&after, &prepared.manifest)?;
        let retained_entry = prepared
            .manifest
            .entries
            .iter()
            .find(|entry| entry.matches_media_asset(&prepared.asset))
            .ok_or(BundleError::HistoryBarrierInvariant)?
            .clone();
        let retained = self
            .store()?
            .lease_input(&crate::owned_inputs::OwnedInputRef {
                manifest_ref: prepared.inputs_ref.clone(),
                entry: retained_entry,
            })?;
        let mut transfer = if self.host_attached && self.destination.is_none() {
            let source = self
                .source
                .as_mut()
                .ok_or(BundleError::HistoryBarrierInvariant)?;
            let descriptor = source.retain_borrowed_source(
                std::slice::from_ref(&prepared.inputs_ref),
                after
                    .encoded_bytes()
                    .map_err(crate::bundle::history_error)?,
            )?;
            let admitted: Result<super::PreparedHostAttachment, BundleError> = (|| {
                source.ensure_borrowed_snapshot(descriptor.admission_id(), &after)?;
                let service = source.borrowed_source_service(&descriptor)?;
                Ok(super::PreparedHostAttachment {
                    named_path: None,
                    expected_root_sha256: None,
                    project_id: None,
                    history: after.clone(),
                    inputs: prepared.inputs_ref.clone(),
                    source: Some(descriptor.clone()),
                    source_service: Some(service),
                })
            })();
            if admitted.is_err() {
                source.release_unpublished_borrowed_source(descriptor.admission_id());
            }
            Some(admitted?)
        } else {
            None
        };
        let mut retained = Some(retained);
        let report = if let Some(destination) = &mut self.destination {
            let bundle = destination.bundle_mut()?;
            let issuer = bundle.issue_issuer(&self.core, &self.inputs, 1)?;
            let envelope = crate::receipts::CommandEnvelope {
                switch: None,
                manual: None,
                transition: None,
                version: 1,
                action: "edit".into(),
                id: crate::receipts::CommandId {
                    issuer,
                    sequence: 1,
                },
                project_id: bundle.project_id().clone(),
                target_branch: bundle.manual_branch_id().clone(),
                owner_epoch: bundle.store().owner_epoch(),
                lease: None,
                expected_head: bundle.manual_head().cloned(),
                expected_revision: self
                    .attachment
                    .capture()
                    .map_err(crate::bundle::history_error)?
                    .revision(),
                command: Some(kinewright_core::JournalCommand::Do(Operation::AddAsset {
                    asset: prepared.asset.clone(),
                })),
                comment_text: None,
                comment_strokes: None,
                evidence: vec![],
            };
            crate::bundle_commit::BundleCommit::command(
                bundle,
                &envelope,
                &self.core,
                &prepared.inputs_ref,
                &[],
            )?;
            self.inputs.clone_from(&prepared.inputs_ref);
            self.input_leases.push(
                retained
                    .take()
                    .ok_or(BundleError::HistoryBarrierInvariant)?,
            );
            let report = bundle.save(&self.core, &prepared.inputs_ref)?;
            if let Some(association) = &mut self.association {
                association.saved = Some(report.history_sha256.clone());
            }
            Some(report)
        } else {
            let frozen = self
                .core
                .attachment_state()
                .map_err(crate::bundle::history_error)?
                == kinewright_core::AttachmentState::Frozen;
            if frozen {
                let cut = self
                    .host_cut
                    .as_ref()
                    .filter(|cut| cut.history() == &before)
                    .ok_or(BundleError::HistoryChanged)?;
                self.attachment
                    .resume(cut)
                    .map_err(crate::bundle::history_error)?;
            }
            let installed = self
                .attachment
                .install_checkpoint(before, after)
                .map_err(crate::bundle::history_error);
            if frozen {
                self.host_cut = Some(
                    self.attachment
                        .freeze()
                        .map_err(crate::bundle::history_error)?,
                );
            }
            if let Err(error) = installed {
                if let Some(staged) = transfer.take()
                    && let (Some(owner), Some(descriptor)) = (&mut self.source, staged.source)
                {
                    owner.release_unpublished_borrowed_source(descriptor.admission_id());
                }
                return Err(error);
            }
            None
        };
        self.inputs.clone_from(&prepared.inputs_ref);
        self.pending_import_source = transfer;
        if let Some(retained) = retained {
            self.input_leases.push(retained);
        }
        Ok((prepared.asset, report))
    }

    /// Replace the borrowed manifest only at an explicit GUI-owned foreground boundary.
    pub fn refresh_unsaved_host_source(
        &mut self,
    ) -> Result<super::PreparedHostAttachment, BundleError> {
        if !self.host_attached
            || self.host_retention_lost
            || self.destination.is_some()
            || self.association.is_some()
        {
            return Err(BundleError::HistoryChanged);
        }
        let staged = self
            .pending_import_source
            .as_ref()
            .ok_or(BundleError::HistoryChanged)?;
        let current = self
            .core
            .export_history()
            .map_err(crate::bundle::history_error)?;
        if staged.history != current || staged.inputs != self.inputs {
            return Err(BundleError::HistoryChanged);
        }
        self.host_source_admission = staged
            .source
            .as_ref()
            .map(|source| source.admission_id().clone());
        self.pending_import_source
            .take()
            .ok_or(BundleError::HistoryBarrierInvariant)
    }
}

fn prepare(
    store: &mut ProjectStore,
    previous: &ObjectRef,
    path: &Path,
    id: AssetId,
    color: Option<kinewright_core::ColorDescription>,
) -> Result<PreparedOwnedMedia, BundleError> {
    let prior = store.read_input_manifest(previous)?;
    let charge = store.reserve(
        0,
        prior
            .decoded_charge()
            .checked_add(8192)
            .ok_or(BundleError::Capacity("staged manifest charge"))?,
        0,
        false,
    )?;
    let mut manifest: OwnedInputManifest = (*prior).clone();
    drop(prior);
    if (manifest.project_id.clone(), manifest.session_id.clone()) != store.input_owner() {
        return Err(BundleError::Identity("import manifest owner".into()));
    }
    let mut file = open_import_source(path)?;
    let metadata = file.metadata()?;
    let identity = crate::namespace::FileIdentity::of_file(&file)?;
    let modified = metadata.modified()?;
    if !metadata.is_file() {
        return Err(BundleError::Unsupported(
            "media source is not a regular file".into(),
        ));
    }
    let (asset, entry, mut pins) =
        store.import_media(&mut file, metadata.len(), id, path, color)?;
    let after = file.metadata()?;
    let current = open_import_source(path)?;
    let current_metadata = current.metadata()?;
    if crate::namespace::FileIdentity::of_file(&file)? != identity
        || crate::namespace::FileIdentity::of_file(&current)? != identity
        || after.len() != metadata.len()
        || current_metadata.len() != metadata.len()
        || after.modified()? != modified
        || current_metadata.modified()? != modified
    {
        return Err(BundleError::Corrupt(
            "source identity, size or modification time changed during import".into(),
        ));
    }
    if !manifest.entries.contains(&entry) {
        manifest.entries.push(entry);
    }
    manifest.entries.sort_by(|a, b| a.entry_id.cmp(&b.entry_id));
    let sealed = store.admit_record(&manifest)?;
    let inputs_ref = sealed.reference.clone();
    pins.push(sealed);
    Ok(PreparedOwnedMedia {
        asset,
        inputs_ref,
        manifest,
        _pins: pins,
        _charge: charge,
    })
}

fn open_import_source(path: &Path) -> Result<std::fs::File, BundleError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        Ok(std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open(path)?)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Err(BundleError::DurabilityUnsupported)
    }
}
