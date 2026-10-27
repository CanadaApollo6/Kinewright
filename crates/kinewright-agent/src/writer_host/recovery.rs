//! Shared manual recovery. Runtime plans have only ordinary receipt/history durability.
use super::{HostCommandOwner, HostError, HostRefusal, HostReply, HostSnapshot, RecoveryBarrier};
use kinewright_core::{AcceptedHistory, HistoryTransition};
use kinewright_project::receipts::{HistoryRequest, ReceiptLookup};

impl HostCommandOwner {
    pub(super) fn begin_recovery(
        &mut self,
        client: String,
        frozen: Option<&HostSnapshot>,
        pending: &[AcceptedHistory],
        discard: Option<usize>,
        reply: &mut HostReply,
    ) -> Result<(), HostError> {
        if !(32..=128).contains(&client.len())
            || !client
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"+/=".contains(&b))
        {
            return Err(
                HostRefusal::new("manual_client", "private attachment identity required").into(),
            );
        }
        let request = kinewright_media::sha256_bytes(&serde_json::to_vec(&(
            &client, frozen, pending, discard,
        ))?);
        if let Some(barrier) = &self.recovery {
            if self.recovery_request.as_ref() == Some(&request) {
                reply.recovery = Some(barrier.clone());
                return Ok(());
            }
            return Err(HostRefusal::new(
                "recovery_pending",
                "resolve the original barrier before starting another",
            )
            .into());
        }
        self.recovery_request = Some(request);
        self.recovery_handoff = None;
        self.confirmed_recovery = None;
        // Fence first, including every error path. Submitted GUI sends are in its frozen cut.
        self.recovering = true;
        self.manual = None;
        if pending.len() > kinewright_core::MANUAL_COMMANDS
            || serde_json::to_vec(pending)?.len() > kinewright_core::MANUAL_BYTES + 128
            || (frozen.is_none() && !pending.is_empty())
        {
            return Err(HostRefusal::new("manual_capacity", "invalid frozen pending chain").into());
        }
        if let Some(snapshot) = &frozen {
            snapshot.verify()?;
        }
        self.owner_mut()?
            .reconcile_manual_chain(&client, pending, frozen.map(|s| &s.history))?;
        let published = HostSnapshot::from_history(self.owner()?.published_manual_history()?)?;
        if let Some(before) = frozen
            && before != &published
        {
            let cut = published
                .history
                .floor_offset
                .checked_sub(before.history.floor_offset)
                .and_then(|n| usize::try_from(n).ok())
                .filter(|n| *n > 0)
                .ok_or_else(|| {
                    HostRefusal::new(
                        "history_changed",
                        "reconciled history differs from the accepted cut",
                    )
                })?;
            let floor = HistoryTransition::floor(before.history.clone(), cut)
                .map_err(|e| HostError::Protocol(e.to_string()))?;
            if floor.after() != &published.history {
                return Err(HostRefusal::new(
                    "history_changed",
                    "shared floor differs from the frozen cut",
                )
                .into());
            }
            let original = kinewright_project::manual_history::AcceptedManualCommand::decode(
                pending.last().ok_or_else(|| {
                    HostRefusal::new(
                        "history_changed",
                        "a pending cut is required for its automatic floor",
                    )
                })?,
            )?
            .envelope;
            let (reference, _) = self.owner()?.manual_checkpoint()?;
            let shadow = self.owner()?.freeze_manual_barrier()?;
            self.owner_mut()?
                .install_manual_barrier(&shadow, published.history.clone(), vec![])?;
            let removed = before
                .history
                .cursor
                .undo
                .len()
                .saturating_sub(published.history.cursor.undo.len());
            self.floor_disclosure = format!(
                "Older Undo history removed by the shared floor: {removed} entries. Current edits, Redo and the gesture are kept."
            );
            let barrier = RecoveryBarrier {
                client,
                switch_generation: self.owner()?.manual_switch_generation()?,
                envelope: original,
                checkpoint_ref: reference,
                before: before.clone(),
                after: published,
            };
            self.recovery = Some(barrier.clone());
            reply.recovery = Some(barrier);
            return Ok(());
        }
        if HostSnapshot::capture(self.core())? != published {
            return Err(
                HostRefusal::new("history_changed", "shadow differs from the durable cut").into(),
            );
        }
        self.prepare_barrier(client, discard, reply)
    }

    fn prepare_barrier(
        &mut self,
        client: String,
        discard: Option<usize>,
        reply: &mut HostReply,
    ) -> Result<(), HostError> {
        let envelope = self.owner_mut()?.prepare_manual_transition(discard)?;
        let (checkpoint_ref, _) = self.owner()?.manual_checkpoint()?;
        let before = HostSnapshot::capture(self.core())?;
        let transition = match discard {
            Some(count) => HistoryTransition::floor(before.history.clone(), count),
            None => HistoryTransition::recovery_close(before.history.clone(), &before.history),
        }
        .map_err(|e| HostError::Protocol(e.to_string()))?;
        let after = HostSnapshot::from_history(transition.after().clone())?;
        self.owner()?.freeze_manual_barrier()?;
        let barrier = RecoveryBarrier {
            switch_generation: self.owner()?.manual_switch_generation()?,
            client,
            envelope,
            checkpoint_ref,
            before,
            after,
        };
        self.recovery = Some(barrier.clone());
        reply.recovery = Some(barrier);
        Ok(())
    }

    pub(super) fn publish_barrier(
        &mut self,
        barrier: RecoveryBarrier,
        reply: &mut HostReply,
    ) -> Result<(), HostError> {
        self.recovering = true;
        self.manual = None;
        if self.owner()?.manual_switch_generation()? != barrier.switch_generation {
            return Err(
                HostRefusal::new("history_changed", "barrier switch generation differs").into(),
            );
        }
        barrier.before.verify()?;
        barrier.after.verify()?;
        if self
            .recovery
            .as_ref()
            .is_some_and(|current| current != &barrier)
        {
            return Err(HostRefusal::new(
                "history_changed",
                "retry must preserve the original barrier",
            )
            .into());
        }
        if barrier.envelope.manual.is_some() {
            let manual = barrier
                .envelope
                .manual
                .as_ref()
                .ok_or_else(|| HostError::Protocol("manual floor identity".into()))?;
            let checkpoint = self.owner()?.manual_barrier_result(&barrier.envelope)?;
            let discard = barrier
                .after
                .history
                .floor_offset
                .checked_sub(barrier.before.history.floor_offset)
                .and_then(|n| usize::try_from(n).ok())
                .filter(|n| *n > 0)
                .ok_or_else(|| HostRefusal::new("history_changed", "manual floor cut"))?;
            let floor = HistoryTransition::floor(barrier.before.history.clone(), discard)
                .map_err(|e| HostError::Protocol(e.to_string()))?;
            if manual.binding.parent_identity != Some(self.owner()?.manual_parent_identity()?)
                || manual.binding.namespace_name.as_deref()
                    != Some(self.owner()?.manual_namespace_name()?.as_str())
                || manual.after_sha256 != barrier.before.history_sha256
                || floor.after() != &barrier.after.history
                || checkpoint.history_ref.sha256 != barrier.after.history_sha256
            {
                return Err(
                    HostRefusal::new("history_changed", "recorded manual floor differs").into(),
                );
            }
            let cut = self.owner()?.freeze_manual_barrier()?;
            self.owner_mut()?.install_manual_barrier(
                &cut,
                barrier.after.history.clone(),
                vec![],
            )?;
            self.recovery = Some(barrier.clone());
            reply.history = Some(barrier.after.history.clone());
            reply.barrier_checkpoint = Some(checkpoint);
            reply.recovery = Some(barrier);
            return Ok(());
        }
        self.owner()?
            .verify_manual_predecessor(&barrier.checkpoint_ref, &barrier.before.history)?;
        let expected = match &barrier.envelope.transition {
            Some(HistoryRequest::Floor {
                discard_batches, ..
            }) => HistoryTransition::floor(barrier.before.history.clone(), *discard_batches),
            Some(HistoryRequest::RecoveryClose { .. }) => HistoryTransition::recovery_close(
                barrier.before.history.clone(),
                &barrier.before.history,
            ),
            None => {
                return Err(
                    HostRefusal::new("history_changed", "barrier transition required").into(),
                );
            }
        }
        .map_err(|e| HostError::Protocol(e.to_string()))?;
        if expected.after() != &barrier.after.history {
            return Err(HostRefusal::new("history_changed", "barrier result differs").into());
        }
        let cut = self.owner()?.freeze_manual_barrier()?;
        let lookup = self.owner()?.manual_transition_receipt(&barrier.envelope)?;
        if lookup == ReceiptLookup::Absent {
            if cut.history() != &barrier.before.history {
                return Err(
                    HostRefusal::new("history_changed", "barrier predecessor differs").into(),
                );
            }
            self.owner_mut()?
                .publish_manual_transition(&barrier.envelope)?;
        }
        let checkpoint = self.owner()?.manual_barrier_result(&barrier.envelope)?;
        if checkpoint.history_ref.sha256 != barrier.after.history_sha256 {
            return Err(HostRefusal::new(
                "recovery_unresolved",
                "barrier has no exact durable result",
            )
            .into());
        }
        self.owner_mut()?
            .install_manual_barrier(&cut, barrier.after.history.clone(), vec![])?;
        self.recovery = Some(barrier.clone());
        reply.history = Some(barrier.after.history.clone());
        reply.barrier_checkpoint = Some(checkpoint);
        reply.recovery = Some(barrier);
        Ok(())
    }

    pub(super) fn confirm_barrier(
        &mut self,
        barrier: RecoveryBarrier,
        installed: HostSnapshot,
        reply: &mut HostReply,
    ) -> Result<(), HostError> {
        installed.verify()?;
        if let Some((original, next)) = &self.recovery_handoff
            && original == &barrier
            && installed == barrier.after
            && self.recovery.as_ref() == Some(next)
        {
            reply.recovery = Some(next.clone());
            return Ok(());
        }
        if let Some((original, confirmed)) = &self.confirmed_recovery
            && original == &barrier
            && installed == barrier.after
            && self.manual == confirmed.manual_lease
            && !self.recovering
            && !self.owner()?.has_unresolved_publication()
            && self.owner()?.manual_switch_generation()? == barrier.switch_generation
            && HostSnapshot::capture(self.core())? == installed
        {
            reply.manual_lease = confirmed.manual_lease.clone();
            reply.manual_inputs = confirmed.manual_inputs.clone();
            reply.history = confirmed.history.clone();
            return Ok(());
        }
        let checkpoint = self.owner()?.manual_barrier_result(&barrier.envelope)?;
        if self.owner()?.manual_switch_generation()? != barrier.switch_generation
            || self.recovery.as_ref() != Some(&barrier)
            || installed != barrier.after
            || HostSnapshot::capture(self.core())? != installed
            || checkpoint.history_ref.sha256 != installed.history_sha256
        {
            return Err(HostRefusal::new(
                "recovery_unconfirmed",
                "both Cores must install the same durable barrier result",
            )
            .into());
        }
        if barrier.envelope.manual.is_some()
            || matches!(
                barrier.envelope.transition,
                Some(HistoryRequest::Floor { .. })
            )
        {
            self.recovery = None;
            self.prepare_barrier(barrier.client.clone(), None, reply)?;
            self.recovery_handoff = Some((
                barrier,
                reply
                    .recovery
                    .clone()
                    .ok_or_else(|| HostError::Protocol("recovery close plan".into()))?,
            ));
        } else {
            let cut = self.owner()?.freeze_manual_barrier()?;
            self.owner_mut()?.resume_manual_barrier(&cut)?;
            self.recovering = false;
            let acquisition: Result<_, HostError> = (|| {
                let lease = self.acquire_manual(barrier.client.clone())?;
                let inputs = self.owner()?.manual_input_context()?;
                Ok((lease, inputs))
            })();
            let (lease, inputs) = match acquisition {
                Ok(grant) => grant,
                Err(error) => {
                    self.manual = None;
                    self.recovering = true;
                    self.owner()?.freeze_manual_barrier()?;
                    return Err(error);
                }
            };
            self.recovery = None;
            reply.manual_lease = Some(lease);
            reply.manual_inputs = Some(inputs);
            reply.history = Some(installed.history);
            self.confirmed_recovery = Some((barrier, reply.clone()));
        }
        Ok(())
    }
}
