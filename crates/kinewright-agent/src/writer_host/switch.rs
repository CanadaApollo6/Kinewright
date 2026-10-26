//! Original envelopes select once. Receipt reads never confer manual authority.
use super::{HostCommandOwner, HostError, HostRefusal, HostReply};
use kinewright_project::{
    bundle::BundleError,
    bundle_switch::{SwitchAttachmentIdentity, SwitchTarget},
    receipts::{CommandEnvelope, ReceiptLookup, ReceiptResult},
};

pub(super) struct SwitchConfirmation {
    installed: SwitchAttachmentIdentity,
    current: SwitchAttachmentIdentity,
    grant: Option<(
        kinewright_project::manual_history::ManualLease,
        kinewright_project::owned_inputs::OwnedInputManifest,
    )>,
}

impl HostCommandOwner {
    pub(super) fn prepare_switch(
        &mut self,
        target: SwitchTarget,
        reply: &mut HostReply,
    ) -> Result<(), HostError> {
        if let Some(original) = &self.pending_switch {
            if original.switch.as_ref().is_none_or(|s| s.target != target) {
                return Err(BundleError::HistoryChanged.into());
            }
            reply.switch_envelope = Some(original.clone());
            reply.switch_preview = Some(self.owner()?.switch_preview()?);
            return Ok(());
        }
        let lease = self.manual.clone().ok_or_else(|| {
            HostRefusal::new(
                "manual_lease",
                "a current manual lease is required to prepare a switch",
            )
        })?;
        let envelope = self.owner()?.switch_envelope(&lease, target)?;
        if self.owner_mut()?.prepare_host_switch(&envelope)? != ReceiptLookup::Absent {
            return Err(BundleError::HistoryChanged.into());
        }
        // Freeze the old shadow only after every fallible target preparation succeeded.
        self.owner()?.freeze_manual_barrier()?;
        self.manual = None;
        self.switch_frozen = true;
        self.pending_switch = Some(envelope.clone());
        reply.switch_envelope = Some(envelope);
        reply.switch_preview = Some(self.owner()?.switch_preview()?);
        Ok(())
    }
    pub(super) fn lookup_switch(
        &self,
        envelope: &CommandEnvelope,
        reply: &mut HostReply,
    ) -> Result<(), HostError> {
        match self.owner()?.manual_transition_receipt(envelope)? {
            ReceiptLookup::Committed(receipt) | ReceiptLookup::Retired(receipt) => {
                let ReceiptResult::Switch { result } = receipt.result else {
                    return Err(BundleError::HistoryChanged.into());
                };
                reply.switch_result = Some(*result);
                reply.switch_attachment = Some(self.owner()?.current_switch_attachment()?);
                Ok(())
            }
            ReceiptLookup::IdPayloadConflict => Err(HostRefusal::new(
                "id_payload_conflict",
                "original switch ID has a different payload",
            )
            .into()),
            _ => Err(HostRefusal::new(
                "switch_unresolved",
                "original switch has no proved receipt",
            )
            .into()),
        }
    }
    pub(super) fn publish_switch(
        &mut self,
        envelope: &CommandEnvelope,
        reply: &mut HostReply,
    ) -> Result<(), HostError> {
        if self.pending_switch.as_ref() != Some(envelope) {
            // Read-before-authorize is solely for an already recorded original result.
            return self.lookup_switch(envelope, reply);
        }
        self.recovery_choice = false;
        self.manual = None;
        self.switch_frozen = true;
        let result = self.owner_mut()?.publish_host_switch(envelope);
        if let Some(report) = self.owner_mut()?.take_preliminary_save_report() {
            self.saved(&report, reply, true)?;
        }
        match result {
            Ok((ReceiptLookup::Committed(_) | ReceiptLookup::Retired(_), report)) => {
                if let Some(report) = report {
                    self.saved(&report, reply, true)?;
                }
                self.initial_core = self.owner()?.core().clone();
                self.owner()?.freeze_manual_barrier()?;
                self.switch_frozen = true;
                self.lookup_switch(envelope, reply)?;
                self.pending_switch = None;
                self.message = "Working edit selected. Editing stays read-only until the current checkpoint and resources are restored.".into();
                Ok(())
            }
            Ok(_) => Err(BundleError::Unsettled.into()),
            Err(error) => {
                self.manual = None;
                if !self.owner()?.has_unresolved_publication() {
                    self.switch_frozen = true;
                    reply.switch_attachment = Some(self.owner()?.current_switch_attachment()?);
                }
                Err(error.into())
            }
        }
    }
    pub(super) fn confirm_switch(
        &mut self,
        client: String,
        installed: &SwitchAttachmentIdentity,
        reply: &mut HostReply,
    ) -> Result<(), HostError> {
        let current = self.owner()?.current_switch_identity()?;
        let continuation = self.confirmed_switch.as_ref().is_some_and(|confirmation| {
            confirmation.installed == *installed
                && confirmation.current.project == current.project
                && confirmation.current.switch_generation == current.switch_generation
                && confirmation.current.current == current.current
        });
        if let Some(confirmation) = &self.confirmed_switch
            && continuation
            && let Some((lease, inputs)) = &confirmation.grant
            && lease.client == client
            && self.manual.as_ref() == Some(lease)
            && !self.switch_frozen
            && !self.recovering
            && !self.owner()?.has_unresolved_publication()
        {
            reply.manual_lease = Some(lease.clone());
            reply.manual_inputs = Some(inputs.clone());
            return Ok(());
        }
        if !self.switch_frozen
            || self.manual.is_some()
            || self.owner()?.has_unresolved_publication()
            || (current != *installed && !continuation)
            || kinewright_project::manual_history::history_sha256(
                &self
                    .owner()?
                    .core()
                    .export_history()
                    .map_err(|e| HostError::Protocol(e.to_string()))?,
            )? != installed.current.history.sha256
        {
            return Err(HostRefusal::new("switch_changed", "restore the exact current pointer, history, lineage and input context before acquiring authority").into());
        }
        let cut = self.owner()?.freeze_manual_barrier()?;
        self.owner_mut()?.resume_manual_barrier(&cut)?;
        self.switch_frozen = false;
        let acquisition: Result<_, HostError> = (|| {
            let lease = self.acquire_manual(client)?;
            let inputs = self.owner()?.manual_input_context()?;
            Ok((lease, inputs))
        })();
        match acquisition {
            Ok((lease, inputs)) => {
                self.confirmed_switch = Some(SwitchConfirmation {
                    installed: installed.clone(),
                    current: self.owner()?.current_switch_identity()?,
                    grant: Some((lease.clone(), inputs.clone())),
                });
                reply.manual_lease = Some(lease);
                reply.manual_inputs = Some(inputs);
                self.message =
                    "Current working edit and resources confirmed; fresh manual authority granted."
                        .into();
                Ok(())
            }
            Err(error) => {
                self.manual = None;
                self.switch_frozen = true;
                self.owner()?.freeze_manual_barrier()?;
                self.confirmed_switch =
                    self.owner()?.current_switch_identity().ok().map(|current| {
                        SwitchConfirmation {
                            installed: installed.clone(),
                            current,
                            grant: None,
                        }
                    });
                Err(error)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[cfg(target_os = "linux")]
    #[test]
    fn w13_host_same_target_pin_survives_quota_retirement_rotation_and_retry() {
        for (target_index, reply_only) in [(0, true), (0, false), (1, false)] {
            use super::super::*;
            use kinewright_core::{Command, Operation, Track, TrackId, TrackKind};
            use kinewright_project::{
                bundle::{BundleWriteAdmission, LoadedBundle, ProjectBundle},
                bundle_recovery::RecoverySelection,
                bundle_switch::SwitchTarget,
                storage_budget::StorageLimits,
            };
            let parent = kinewright_media::test_support::workspace_target_dir();
            std::fs::create_dir_all(&parent).unwrap();
            let root =
                kinewright_media::test_support::TempDirectory::new_in(&parent, "c11-host-pin");
            let path = root.path("a.kinewright");
            let doc = Document {
                tracks: (1..=8)
                    .map(|id| Track {
                        id: TrackId(id),
                        kind: TrackKind::Video,
                        sync_lock: true,
                        clips: vec![],
                    })
                    .collect(),
                ..Document::default()
            };
            let core = Core::spawn(doc).unwrap();
            let grow = |core: &Core, count| {
                for i in 0..count {
                    core.request(Command::Do(Operation::SetTrackSyncLock {
                        track: TrackId(1),
                        locked: i % 2 == 0,
                    }))
                    .unwrap();
                }
            };
            grow(&core, 12);
            let mut bundle =
                ProjectBundle::activate(&path, BundleWriteAdmission::new_document()).unwrap();
            let inputs = bundle.empty_inputs().unwrap().reference.clone();
            bundle.save(&core, &inputs).unwrap();
            let left = bundle.manual_branch_id().clone();
            let mut spare = vec![];
            for count in 16..22 {
                let branch = bundle.fork_branch(&left).unwrap();
                let other =
                    Core::spawn((*core.export_history().unwrap().document).clone()).unwrap();
                grow(&other, count);
                bundle.save_branch(&branch, &other).unwrap();
                spare.push(branch);
            }
            grow(&core, 8);
            bundle.save(&core, &inputs).unwrap();
            grow(&core, 8);
            bundle.save(&core, &inputs).unwrap();
            drop(bundle);
            let roots = SessionRoots {
                read: vec![root.root().into()],
                write: vec![root.root().into()],
            };
            let mut host = HostCommandOwner::open(HostConfig {
                project: "c11-pin-host".into(),
                path: Some(path.clone()),
                roots: roots.clone(),
                origin: HostOrigin::Headless,
            })
            .unwrap();
            let run = |host: &mut HostCommandOwner, command| host.execute(command, roots.clone());
            let identity = "a".repeat(64);
            assert!(
                run(
                    &mut host,
                    HostCommand::AcquireManual {
                        client: identity.clone()
                    }
                )
                .error
                .is_none()
            );
            // Issuer creation rotates inventory. Establish distinct P1/P2 histories
            // after lease acquisition so P1 is not merely the same-history issuer root.
            for _ in 0..2 {
                grow(host.core(), 8);
                let snapshot = HostSnapshot::capture(host.core()).unwrap();
                let saved = run(&mut host, HostCommand::Save { snapshot });
                assert!(saved.error.is_none(), "{:?}", saved.error);
            }
            let loaded = LoadedBundle::read(&path).unwrap();
            let peak = loaded.usage_probe().unwrap().peak_live_history_bytes;
            drop(loaded);
            let target = host
                .owner()
                .unwrap()
                .list_recovery_points()
                .unwrap()
                .remove(target_index);
            let RecoverySelection::Checkpoint {
                root: selected,
                branch,
            } = target.clone()
            else {
                panic!("P2")
            };
            let profile = StorageLimits {
                // The exact lane checks a refusal reply at the current root's
                // measured peak. Retry lanes also admit ordinary issuer and
                // retirement metadata publications under this fixed test profile.
                live_history_bytes: peak
                    + if reply_only {
                        0
                    } else {
                        kinewright_project::bundle_commit::history_cost_probe(
                            &host.core().export_history().unwrap(),
                            &StorageLimits::default(),
                        )
                        .unwrap()
                            / 2
                    },
                ..StorageLimits::default()
            };
            host.owner_mut()
                .unwrap()
                .restrict_limits_probe(profile.clone())
                .unwrap();
            let prepared = run(
                &mut host,
                HostCommand::PrepareSwitch {
                    target: SwitchTarget::Checkpoint {
                        root: selected.clone(),
                        branch,
                    },
                },
            );
            assert!(prepared.error.is_none(), "{:?}", prepared.error);
            let target_payload = prepared.switch_preview.unwrap();
            let request = prepared.switch_envelope.unwrap();
            let pin_count = host
                .owner()
                .unwrap()
                .store()
                .unwrap()
                .retention_probe()
                .unwrap()
                .1[&selected.sha256];
            assert!(pin_count > 0);
            let refused = run(
                &mut host,
                HostCommand::Switch {
                    envelope: Box::new(request.clone()),
                },
            );
            assert_eq!(
                refused.error.as_ref().map(|e| e.code.as_str()),
                Some("storage_backpressure"),
                "{:?}",
                refused.error
            );
            assert!(
                refused.switch_attachment.is_some(),
                "quota refusal must carry the verified saved source attachment"
            );
            if reply_only {
                assert!(LoadedBundle::read_with_limits_probe(&path, profile).is_ok());
                continue;
            }
            let confirmation = run(
                &mut host,
                HostCommand::ConfirmSwitch {
                    client: identity.clone(),
                    installed: Box::new(refused.switch_attachment.unwrap().identity().unwrap()),
                },
            );
            assert!(
                confirmation.error.is_none(),
                "P1/P2 confirmation: {:?}",
                confirmation.error
            );
            assert!(
                host.owner()
                    .unwrap()
                    .store()
                    .unwrap()
                    .retention_probe()
                    .unwrap()
                    .1
                    .get(&selected.sha256)
                    .copied()
                    .unwrap_or_default()
                    > 0
            );
            let mut activated = None;
            for branch in spare {
                let preview = run(
                    &mut host,
                    HostCommand::PreviewRetirement {
                        branch: branch.clone(),
                    },
                );
                assert!(preview.error.is_none());
                assert!(run(&mut host, HostCommand::RemoveBranch).error.is_none());
                assert!(
                    host.owner()
                        .unwrap()
                        .store()
                        .unwrap()
                        .verify(&selected)
                        .is_ok()
                );
                assert!(
                    host.owner()
                        .unwrap()
                        .store()
                        .unwrap()
                        .retention_probe()
                        .unwrap()
                        .1
                        .get(&selected.sha256)
                        .copied()
                        .unwrap_or_default()
                        > 0
                );
                let result = run(
                    &mut host,
                    HostCommand::Switch {
                        envelope: Box::new(request.clone()),
                    },
                );
                if result.error.is_none() {
                    activated = result.switch_attachment;
                    break;
                }
                assert_eq!(
                    result.error.as_ref().map(|e| e.code.as_str()),
                    Some("storage_backpressure")
                );
                assert!(
                    run(
                        &mut host,
                        HostCommand::ConfirmSwitch {
                            client: identity.clone(),
                            installed: Box::new(
                                result.switch_attachment.unwrap().identity().unwrap()
                            )
                        }
                    )
                    .error
                    .is_none()
                );
            }
            let activated = activated.expect(
                "unrelated working-branch retirement must make the same pinned target usable",
            );
            assert_eq!(activated.history, target_payload.history);
            assert!(
                !host
                    .owner()
                    .unwrap()
                    .list_recovery_points()
                    .unwrap()
                    .contains(&target)
            );
            drop(host);
            let loaded = LoadedBundle::read_with_limits_probe(&path, profile).unwrap();
            let fresh = Core::spawn(Document::default()).unwrap();
            loaded.restore_manual(&fresh).unwrap();
            assert_eq!(fresh.export_history().unwrap(), activated.history);
        }
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn w13_host_prepared_target_exclusion_and_current_head_retry() {
        use super::super::*;
        use kinewright_project::{
            bundle_recovery::RecoverySelection, bundle_switch::SwitchTarget,
            editing_session::Ja1EditingSession, io_observer, storage_budget::StorageLimits,
        };
        let parent = kinewright_media::test_support::workspace_target_dir();
        std::fs::create_dir_all(&parent).unwrap();
        for changed_head in [false, true] {
            let root =
                kinewright_media::test_support::TempDirectory::new_in(&parent, "c11-host-target");
            let path = root.path("a.kinewright");
            let core = Core::spawn(Document::default()).unwrap();
            let mut owner =
                Ja1EditingSession::new(root.root(), &core, StorageLimits::default()).unwrap();
            owner.first_save(&path, StorageLimits::default()).unwrap();
            owner.save_active().unwrap();
            drop(owner);
            let roots = SessionRoots {
                read: vec![root.root().into()],
                write: vec![root.root().into()],
            };
            let mut host = HostCommandOwner::open(HostConfig {
                project: "c11-target-host".into(),
                path: Some(path),
                roots: roots.clone(),
                origin: HostOrigin::Headless,
            })
            .unwrap();
            let run = |host: &mut HostCommandOwner, command| host.execute(command, roots.clone());
            let identity = "a".repeat(64);
            assert!(
                run(
                    &mut host,
                    HostCommand::AcquireManual {
                        client: identity.clone()
                    }
                )
                .error
                .is_none()
            );
            let RecoverySelection::Checkpoint { root, branch } = host
                .owner()
                .unwrap()
                .list_recovery_points()
                .unwrap()
                .remove(0)
            else {
                panic!("checkpoint")
            };
            let request = run(
                &mut host,
                HostCommand::PrepareSwitch {
                    target: SwitchTarget::Checkpoint { root, branch },
                },
            )
            .switch_envelope
            .unwrap();
            let switched = run(
                &mut host,
                HostCommand::Switch {
                    envelope: Box::new(request),
                },
            );
            assert!(switched.error.is_none(), "{:?}", switched.error);
            let current = switched.switch_attachment.unwrap();
            let old = switched.switch_result.unwrap().old.branch;
            assert_eq!(
                host.core().attachment_state().unwrap(),
                AttachmentState::Frozen
            );
            assert!(matches!(
                host.core().request(kinewright_core::Command::Undo).unwrap(),
                kinewright_core::Event::MutationRefused { .. }
            ));
            assert!(
                run(
                    &mut host,
                    HostCommand::ConfirmSwitch {
                        client: identity.clone(),
                        installed: Box::new(current.identity().unwrap())
                    }
                )
                .error
                .is_none()
            );
            let request = run(
                &mut host,
                HostCommand::PrepareSwitch {
                    target: SwitchTarget::WorkingBranch {
                        branch: old.clone(),
                    },
                },
            )
            .switch_envelope
            .unwrap();
            let refused = io_observer::fail_once("root_before_publish", || {
                run(
                    &mut host,
                    HostCommand::Switch {
                        envelope: Box::new(request.clone()),
                    },
                )
            });
            assert!(refused.error.is_some());
            let current = refused.switch_attachment.unwrap();
            assert!(
                run(
                    &mut host,
                    HostCommand::ConfirmSwitch {
                        client: identity.clone(),
                        installed: Box::new(current.identity().unwrap())
                    }
                )
                .error
                .is_none()
            );
            assert!(
                !host
                    .owner()
                    .unwrap()
                    .retirement_candidates()
                    .unwrap()
                    .contains(&old)
            );
            let root_before = host.owner().unwrap().root_sha256().unwrap();
            run(
                &mut host,
                HostCommand::PreviewRetirement {
                    branch: old.clone(),
                },
            );
            assert!(run(&mut host, HostCommand::RemoveBranch).error.is_some());
            assert_eq!(host.owner().unwrap().root_sha256().unwrap(), root_before);
            host.owner_mut()
                .unwrap()
                .change_prepared_target_probe(changed_head)
                .unwrap();
            let root_before = host.owner().unwrap().root_sha256().unwrap();
            let history = host.core().export_history().unwrap();
            let retry = run(
                &mut host,
                HostCommand::Switch {
                    envelope: Box::new(request),
                },
            );
            assert!(retry.error.is_some());
            assert_eq!(host.owner().unwrap().root_sha256().unwrap(), root_before);
            assert_eq!(host.core().export_history().unwrap(), history);
            assert_eq!(
                host.owner()
                    .unwrap()
                    .list_working_branches()
                    .unwrap()
                    .iter()
                    .any(|(id, _, _)| id == &old),
                changed_head
            );
        }
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn w13_host_frozen_rescue_abandons_original_switch_and_preserves_disclosures() {
        use super::super::*;
        use kinewright_core::{Command, Event, Operation, Track, TrackId, TrackKind};
        use kinewright_project::{
            bundle_recovery::RecoverySelection, bundle_switch::SwitchTarget,
            editing_session::Ja1EditingSession, io_observer, storage_budget::StorageLimits,
        };
        let parent = kinewright_media::test_support::workspace_target_dir();
        std::fs::create_dir_all(&parent).unwrap();
        for (landed, publication) in [(false, 1), (false, 2), (true, 1), (true, 2)] {
            let root = kinewright_media::test_support::TempDirectory::new_in(
                &parent,
                "c11-host-frozen-rescue",
            );
            let a_parent = root.path("a");
            std::fs::create_dir(&a_parent).unwrap();
            let path = a_parent.join("a.kinewright");
            let core = Core::spawn(Document {
                tracks: vec![Track {
                    id: TrackId(1),
                    kind: TrackKind::Video,
                    sync_lock: true,
                    clips: vec![],
                }],
                ..Document::default()
            })
            .unwrap();
            let mut owner =
                Ja1EditingSession::new(&a_parent, &core, StorageLimits::default()).unwrap();
            owner.first_save(&path, StorageLimits::default()).unwrap();
            for locked in [false, true] {
                core.request(Command::Do(Operation::SetTrackSyncLock {
                    track: TrackId(1),
                    locked,
                }))
                .unwrap();
                owner.save_active().unwrap();
            }
            drop(owner);
            let roots = SessionRoots {
                read: vec![root.root().into()],
                write: vec![root.root().into()],
            };
            let mut host = HostCommandOwner::open(HostConfig {
                project: "c11-frozen-host".into(),
                path: Some(path.clone()),
                roots: roots.clone(),
                origin: HostOrigin::Headless,
            })
            .unwrap();
            let run = |host: &mut HostCommandOwner, command| host.execute(command, roots.clone());
            assert!(
                run(
                    &mut host,
                    HostCommand::AcquireManual {
                        client: "a".repeat(64)
                    }
                )
                .error
                .is_none()
            );
            let target = host
                .owner()
                .unwrap()
                .list_recovery_points()
                .unwrap()
                .pop()
                .unwrap();
            let RecoverySelection::Checkpoint {
                root: selected,
                branch,
            } = target
            else {
                panic!("P2 checkpoint")
            };
            let old = host.core().clone();
            old.request(Command::Do(Operation::SetTrackSyncLock {
                track: TrackId(1),
                locked: false,
            }))
            .unwrap();
            let prepared = run(
                &mut host,
                HostCommand::PrepareSwitch {
                    target: SwitchTarget::Checkpoint {
                        root: selected,
                        branch,
                    },
                },
            );
            assert!(prepared.error.is_none(), "{:?}", prepared.error);
            let envelope = prepared.switch_envelope.unwrap();
            assert!(
                host.owner()
                    .unwrap()
                    .selection_availability()
                    .unwrap()
                    .contains("will remove")
            );
            let unknown = io_observer::fail_nth(
                if landed {
                    "root_parent_sync"
                } else {
                    "root_rename_io"
                },
                publication,
                || {
                    run(
                        &mut host,
                        HostCommand::Switch {
                            envelope: Box::new(envelope.clone()),
                        },
                    )
                },
            );
            assert!(unknown.error.is_some());
            assert_eq!(old.attachment_state().unwrap(), AttachmentState::Frozen);
            let frozen = old.export_history().unwrap();
            let hidden = root.path("hidden-a");
            std::fs::rename(&a_parent, &hidden).unwrap();
            for fault in [Some("root_before_publish"), Some("root_parent_sync"), None] {
                let preview = run(
                    &mut host,
                    HostCommand::PreviewRescue {
                        snapshot: HostSnapshot::from_history(frozen.clone()).unwrap(),
                    },
                );
                assert!(preview.error.is_none(), "{:?}", preview.error);
                assert!(preview.status.selection_availability.contains("Abandoning"));
                assert!(preview.status.close_disclosure.contains("Closing"));
                assert!(
                    preview
                        .status
                        .destination_change
                        .as_ref()
                        .is_some_and(|m| m.contains("Abandon"))
                );
                let destination =
                    root.path(&format!("rescue-{}.kinewright", fault.unwrap_or("success")));
                let result = if let Some(fault) = fault {
                    io_observer::fail_once(fault, || {
                        run(&mut host, HostCommand::RescueTo { destination })
                    })
                } else {
                    run(&mut host, HostCommand::RescueTo { destination })
                };
                if fault.is_some() {
                    assert!(result.activation.is_none());
                    assert!(result.status.frozen);
                    assert!(matches!(
                        old.request(Command::Undo).unwrap(),
                        Event::MutationRefused { .. }
                    ));
                    assert_eq!(old.export_history().unwrap(), frozen);
                } else {
                    assert!(result.error.is_none(), "{:?}", result.error);
                    assert!(result.activation.is_some());
                    assert!(!result.status.frozen);
                    assert!(host.pending_switch.is_none());
                    assert!(
                        result.status.message.contains("Abandon")
                            || result.status.message.contains("abandon")
                    );
                }
            }
            let b = host.core().clone();
            b.request(Command::Do(Operation::SetTrackSyncLock {
                track: TrackId(1),
                locked: true,
            }))
            .unwrap();
            let after = b.export_history().unwrap();
            std::fs::rename(hidden, a_parent).unwrap();
            let late = run(&mut host, HostCommand::Reconcile { destination: path });
            assert!(late.error.is_none(), "{:?}", late.error);
            assert_eq!(host.core().actor_identity(), b.actor_identity());
            assert_eq!(b.export_history().unwrap(), after);
            assert!(
                run(
                    &mut host,
                    HostCommand::Switch {
                        envelope: Box::new(envelope)
                    }
                )
                .error
                .is_some()
            );
            assert_eq!(b.export_history().unwrap(), after);
        }
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn w13_registered_host_create_reopen_refuses_retirement_and_preserves_unsaved_edits() {
        use super::super::*;
        use kinewright_core::{Command, Event, Operation, Track, TrackId, TrackKind};
        use kinewright_project::{
            editing_session::Ja1EditingSession, storage_budget::StorageLimits,
        };
        let parent = kinewright_media::test_support::workspace_target_dir();
        std::fs::create_dir_all(&parent).unwrap();
        let root =
            kinewright_media::test_support::TempDirectory::new_in(&parent, "c11-host-registered");
        let path = root.path("a.kinewright");
        let core = Core::spawn(Document {
            tracks: vec![Track {
                id: TrackId(1),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: vec![],
            }],
            ..Document::default()
        })
        .unwrap();
        let mut owner =
            Ja1EditingSession::new(root.root(), &core, StorageLimits::default()).unwrap();
        owner.first_save(&path, StorageLimits::default()).unwrap();
        drop(owner);
        let roots = SessionRoots {
            read: vec![root.root().into()],
            write: vec![root.root().into()],
        };
        let mut host = HostCommandOwner::open(HostConfig {
            project: "c11-registered-host".into(),
            path: Some(path.clone()),
            roots: roots.clone(),
            origin: HostOrigin::Headless,
        })
        .unwrap();
        let run = |host: &mut HostCommandOwner, command| {
            let reply = host.execute(command, roots.clone());
            assert!(reply.error.is_none(), "{:?}", reply.error);
            reply
        };
        let started = run(
            &mut host,
            HostCommand::StartScripted {
                turns: vec![crate::ScriptedTurn::new(vec![], "registered")],
                prompts: vec!["inspect".into()],
            },
        )
        .session
        .unwrap();
        let branch = started.branch.unwrap();
        let mut reopen_base = started.base;
        let source_base = host
            .owner()
            .unwrap()
            .current_switch_attachment()
            .unwrap()
            .current
            .branch;
        let identity = "b".repeat(64);
        run(
            &mut host,
            HostCommand::AcquireManual {
                client: identity.clone(),
            },
        );
        let kinewright_project::bundle_recovery::RecoverySelection::Checkpoint {
            root: target_root,
            branch: target_branch,
        } = host
            .owner()
            .unwrap()
            .list_recovery_points()
            .unwrap()
            .remove(0)
        else {
            panic!("checkpoint")
        };
        let envelope = run(
            &mut host,
            HostCommand::PrepareSwitch {
                target: kinewright_project::bundle_switch::SwitchTarget::Checkpoint {
                    root: target_root,
                    branch: target_branch,
                },
            },
        )
        .switch_envelope
        .unwrap();
        let selected = run(
            &mut host,
            HostCommand::Switch {
                envelope: Box::new(envelope),
            },
        )
        .switch_attachment
        .unwrap();
        run(
            &mut host,
            HostCommand::ConfirmSwitch {
                client: identity,
                installed: Box::new(selected.identity().unwrap()),
            },
        );
        for reopening in [false, true] {
            let id = if reopening {
                let base = reopen_base.take().unwrap();
                run(
                    &mut host,
                    HostCommand::ReopenScripted {
                        branch: branch.clone(),
                        base,
                        turns: vec![crate::ScriptedTurn::new(vec![], "reopened")],
                        prompts: vec!["inspect again".into()],
                    },
                )
                .session
                .unwrap()
                .id
            } else {
                started.id
            };
            let core = host.sessions[&id].core.clone();
            assert!(matches!(
                core.request(Command::Do(Operation::SetTrackSyncLock {
                    track: TrackId(1),
                    locked: reopening
                }))
                .unwrap(),
                Event::DocumentChanged { .. }
            ));
            let unsaved = core.export_history().unwrap();
            let root_before = host.owner().unwrap().root_sha256().unwrap();
            let preview = run(
                &mut host,
                HostCommand::PreviewRetirement {
                    branch: branch.clone(),
                },
            );
            assert!(
                host.execute(HostCommand::RemoveBranch, roots.clone())
                    .error
                    .is_some()
            );
            assert!(preview.status.message.contains("Save or close"));
            assert_eq!(host.owner().unwrap().root_sha256().unwrap(), root_before);
            assert_eq!(core.export_history().unwrap(), unsaved);
            assert!(
                !host
                    .owner()
                    .unwrap()
                    .retirement_candidates()
                    .unwrap()
                    .contains(&branch)
            );
            let manual = source_base.clone();
            let base_blockers = host
                .owner()
                .unwrap()
                .list_working_branches()
                .unwrap()
                .into_iter()
                .find(|(id, _, _)| id == &manual)
                .unwrap()
                .2;
            run(
                &mut host,
                HostCommand::PreviewRetirement {
                    branch: source_base.clone(),
                },
            );
            assert!(
                host.execute(HostCommand::RemoveBranch, roots.clone())
                    .error
                    .is_some()
            );
            assert!(base_blockers.iter().any(|s| s.contains("specialist base")));
            run(
                &mut host,
                HostCommand::SaveSession {
                    branch: branch.clone(),
                },
            );
            reopen_base = run(&mut host, HostCommand::SessionStatus { session: id })
                .session
                .unwrap()
                .base;
            run(
                &mut host,
                HostCommand::CloseSession {
                    branch: branch.clone(),
                },
            );
            assert!(
                host.owner()
                    .unwrap()
                    .retirement_candidates()
                    .unwrap()
                    .contains(&branch)
            );
        }
        run(
            &mut host,
            HostCommand::PreviewRetirement {
                branch: branch.clone(),
            },
        );
        run(&mut host, HostCommand::RemoveBranch);
        assert!(
            host.owner()
                .unwrap()
                .list_working_branches()
                .unwrap()
                .iter()
                .all(|(id, _, _)| id != &branch)
        );
    }
}
