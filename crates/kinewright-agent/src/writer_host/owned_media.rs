//! Input admission prepares the next manual command without changing its document.
use super::{
    HostAuth, HostCommandOwner, HostError, HostOrigin, HostRefusal, HostReply, HostSnapshot,
    HostSourceBinding, PinnedPath, RemoteSource, SessionRoots,
};
use kinewright_core::AssetId;
use kinewright_project::source_boundary::BorrowedSourceReader;
use std::path::Path;

impl HostCommandOwner {
    pub(super) fn refresh_unsaved_inputs(
        &mut self,
        binding: HostSourceBinding,
        snapshot: HostSnapshot,
    ) -> Result<(), HostError> {
        if self.owner.is_some()
            || self.manual.is_some()
            || self.recovering
            || self.switch_frozen
            || !matches!(self.config.origin, HostOrigin::Gui { .. })
        {
            return Err(HostRefusal::new(
                "unsaved_input_context",
                "Only the originating unsaved context may refresh its input namespace.",
            )
            .into());
        }
        snapshot.verify()?;
        let original = self.remote.as_ref().ok_or_else(|| {
            HostRefusal::new("unsaved_input_context", "Originating source is missing.")
        })?;
        let old = original.binding.source.as_ref().ok_or_else(|| {
            HostRefusal::new("unsaved_input_context", "Originating source is missing.")
        })?;
        let source = binding.source.as_ref().ok_or_else(|| {
            HostRefusal::new("unsaved_input_context", "Replacement source is missing.")
        })?;
        if source.session_id().is_none() || source.session_id() != old.session_id() {
            return Err(HostRefusal::new(
                "unsaved_input_context",
                "Replacement input owner differs.",
            )
            .into());
        }
        let mut remote = RemoteSource::new(binding.clone(), &self.instance)?;
        remote.prove()?;
        remote.prove_snapshot(
            source,
            &kinewright_project::source_boundary::BorrowedSnapshotBinding::from_history(
                &snapshot.history,
            )?,
        )?;
        self.install(snapshot)?;
        self.remote = Some(remote);
        if let HostOrigin::Gui { retention, .. } = &mut self.config.origin {
            *retention = binding;
        }
        self.message = "Imported media is kept in this session. Save once to keep editing.".into();
        Ok(())
    }
    fn check_input_editor(
        &mut self,
        client: &str,
        snapshot: &HostSnapshot,
    ) -> Result<(u64, u64), HostError> {
        if self.switch_frozen || self.recovering || self.choice.is_some() {
            return Err(HostRefusal::new(
                "input_frozen",
                "Resolve the current save or selection before importing media.",
            )
            .into());
        }
        let current = self
            .manual
            .as_ref()
            .filter(|lease| lease.client == client)
            .ok_or_else(|| {
                HostRefusal::new(
                    "manual_lease",
                    "Input admission requires this editor's manual lease.",
                )
            })?;
        snapshot.verify()?;
        if HostSnapshot::capture(self.core())? != *snapshot {
            return Err(HostRefusal::new(
                "history_changed",
                "Drain the exact accepted manual prefix before importing media.",
            )
            .into());
        }
        let current_generation = current.generation;
        let generation = self.next_manual_generation.max(
            current_generation
                .checked_add(1)
                .ok_or_else(|| HostError::Protocol("manual lease generation exhausted".into()))?,
        );
        let following_generation = generation
            .checked_add(1)
            .ok_or_else(|| HostError::Protocol("manual lease generation exhausted".into()))?;
        Ok((generation, following_generation))
    }

    fn pin_owned_input(&self, path: &Path, roots: &SessionRoots) -> Result<PinnedPath, HostError> {
        let pin = if matches!(&self.config.origin, HostOrigin::Gui { .. }) {
            let root = path
                .ancestors()
                .last()
                .filter(|root| root.is_absolute())
                .ok_or_else(|| HostRefusal::roots("absolute chosen input required"))?;
            let scope = SessionRoots {
                read: vec![root.into()],
                write: vec![],
            };
            let pin = PinnedPath::admit(path, &scope, &scope, false)?;
            if !roots.permits(&pin.inspected.path, false) {
                return Err(
                    HostRefusal::roots("chosen input is outside the forwarded read scope").into(),
                );
            }
            pin
        } else {
            PinnedPath::admit(path, &self.config.roots, roots, false)?
        };
        Ok(pin)
    }

    pub(super) fn prepare_owned_media(
        &mut self,
        client: &str,
        path: &Path,
        asset_id: AssetId,
        snapshot: &HostSnapshot,
        roots: &SessionRoots,
        reply: &mut HostReply,
    ) -> Result<(), HostError> {
        let generations = self.check_input_editor(client, snapshot)?;
        let pin = self.pin_owned_input(path, roots)?;
        let mut prepared = self.owner_mut()?.prepare_owned_media(&pin.io, asset_id)?;
        prepared.asset.path = path.to_path_buf();
        self.grant_owned_inputs(client, prepared, generations, reply)?;
        self.message = "Media is checked and ready to import. Saving is pending.".into();
        Ok(())
    }

    pub(super) fn prepare_media_interpretation(
        &mut self,
        client: &str,
        operation: &kinewright_core::Operation,
        snapshot: &HostSnapshot,
        reply: &mut HostReply,
    ) -> Result<(), HostError> {
        let generations = self.check_input_editor(client, snapshot)?;
        let kinewright_core::Operation::SetAssetColorDescription { asset: id, .. } = operation
        else {
            return Err(HostRefusal::new(
                "input_interpretation",
                "Only an existing asset's colour interpretation may change here.",
            )
            .into());
        };
        let mut document = (*snapshot.history.document).clone();
        operation
            .apply(&mut document)
            .map_err(|error| HostError::Protocol(error.to_string()))?;
        let asset = document
            .media_pool
            .iter()
            .find(|asset| asset.id == *id)
            .ok_or_else(|| {
                HostRefusal::new("input_interpretation", "The source asset is missing.")
            })?;
        let prepared = self.owner_mut()?.prepare_media_interpretation(asset)?;
        self.grant_owned_inputs(client, prepared, generations, reply)?;
        reply.owned_media_edit = Some(operation.clone());
        self.message = "Colour interpretation is checked. Saving is pending.".into();
        Ok(())
    }

    fn grant_owned_inputs(
        &mut self,
        client: &str,
        prepared: kinewright_project::owned_inputs::PreparedOwnedMedia,
        (generation, following_generation): (u64, u64),
        reply: &mut HostReply,
    ) -> Result<(), HostError> {
        let token = serde_json::to_value(HostAuth::generate()?)?
            .as_str()
            .ok_or_else(|| HostError::Protocol("input manual token".into()))?
            .to_owned();
        let mut lease =
            self.owner_mut()?
                .issue_manual_lease_for_inputs(client.to_owned(), token, &prepared)?;
        self.next_manual_generation = following_generation;
        lease.generation = generation;
        reply.owned_media = Some(prepared.asset.clone());
        reply.manual_inputs = Some(prepared.manifest.clone());
        reply.manual_lease = Some(lease.clone());
        self.prepared_owned_media = Some(prepared);
        self.manual = Some(lease);
        Ok(())
    }
}
