//! Recovery replays only the absent original suffix and publishes one fresh
//! install epoch. Acknowledgements and that epoch survive a writer restart.
use super::{HostCommandOwner, HostError, HostRefusal, HostReply, HostSnapshot, RecoveryBarrier};
use kinewright_project::bundle::BundleError;
use kinewright_project::operation_protocol::EncodedMessage;

fn confirmation_digest(barrier: &RecoveryBarrier) -> Result<[u8; 32], HostError> {
    struct Writer(kinewright_hash::Sha256);
    impl std::io::Write for Writer {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.update(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut writer = Writer(kinewright_hash::Sha256::new());
    writer.0.update(b"kinewright/recovery-confirmation/v1\0");
    serde_json::to_writer(&mut writer, barrier)?;
    Ok(writer.0.finalize())
}

impl HostCommandOwner {
    pub(super) fn begin_recovery(
        &mut self,
        client: String,
        frozen: Option<&HostSnapshot>,
        pending: &[EncodedMessage],
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
        let request =
            kinewright_media::sha256_bytes(&serde_json::to_vec(&(&client, frozen, pending))?);
        if let Some(barrier) = &self.recovery {
            if self.recovery_request.as_ref() == Some(&request) {
                reply.recovery = Some(barrier.clone());
                return Ok(());
            }
            return Err(HostRefusal::new(
                "recovery_pending",
                "resolve the original install before starting another",
            )
            .into());
        }
        self.recovering = true;
        self.manual = None;
        // Starting the next recovery revokes the preceding confirmation's
        // lease. Neither replay cache may retain or return that old grant.
        self.confirmed_recovery = None;
        self.foreground_reply = None;
        if pending.len() > kinewright_project::operation_protocol::PENDING_COMMANDS
            || pending
                .iter()
                .try_fold(0_u64, |n, message| n.checked_add(message.byte_len()))
                .is_none_or(|n| n > kinewright_project::operation_protocol::PENDING_BYTES)
            || (frozen.is_none() && !pending.is_empty())
        {
            return Err(HostRefusal::new("manual_capacity", "invalid frozen pending chain").into());
        }
        for message in pending {
            self.owner()?.validate_manual_message_publisher(message)?;
        }
        let before = match frozen {
            Some(snapshot) => {
                snapshot.verify()?;
                snapshot.clone()
            }
            None => HostSnapshot::capture(self.core())?,
        };
        let source = before.history.identity();
        let acknowledged =
            if let Some(receipts) = self.owner()?.installed_manual_receipts(source)? {
                // The prior attempt published this same install, even if its reply
                // was lost and this is a different writer process. Never replay it.
                let mut acknowledged = Vec::with_capacity(pending.len());
                for encoded in pending {
                    let message = encoded
                        .decode()
                        .map_err(|error| HostError::Protocol(error.to_string()))?;
                    let receipt = receipts
                        .iter()
                        .find(|receipt| receipt.id == message.id)
                        .ok_or_else(|| {
                            HostRefusal::new(
                                "recovery_unresolved",
                                "original command has no durable receipt",
                            )
                        })?;
                    acknowledged.push(receipt.clone());
                }
                acknowledged
            } else {
                self.owner_mut()?
                    .reconcile_manual_chain(&client, pending, Some(source))?;
                let mut receipts = Vec::with_capacity(pending.len());
                for message in pending {
                    receipts.push(self.owner()?.manual_receipt(message)?.ok_or_else(|| {
                        HostRefusal::new(
                            "recovery_unresolved",
                            "original command has no durable receipt",
                        )
                    })?);
                }
                receipts
            };
        // Check each original acknowledgement, rather than trusting an install
        // question to grant fresh authority to an unrelated issuer or payload.
        for message in pending {
            let decoded = message
                .decode()
                .map_err(|e| HostError::Protocol(e.to_string()))?;
            if !acknowledged.iter().any(|receipt| {
                receipt.id == decoded.id
                    && receipt.sequence == decoded.sequence
                    && receipt.accepted == decoded.accepted
                    && receipt.additional_batches == decoded.folded.len() as u64
                    && receipt.tip == decoded.tip
                    && decoded.digest().ok() == Some(receipt.command)
            }) {
                return Err(HostRefusal::new(
                    "recovery_unresolved",
                    "original payload differs from its saved receipt",
                )
                .into());
            }
        }
        let checkpoint_ref = self.owner_mut()?.rotate_manual_epoch(source)?;
        let after = HostSnapshot::from_history(self.owner()?.published_manual_history()?)?;
        let barrier = RecoveryBarrier {
            client,
            switch_generation: self.owner()?.manual_switch_generation()?,
            checkpoint_ref,
            acknowledged,
            before,
            after,
        };
        self.recovery_request = Some(request);
        self.recovery = Some(barrier.clone());
        reply.recovery = Some(barrier);
        Ok(())
    }

    pub(super) fn publish_barrier(
        &mut self,
        barrier: &RecoveryBarrier,
        reply: &mut HostReply,
    ) -> Result<(), HostError> {
        self.recovering = true;
        self.manual = None;
        barrier.before.verify()?;
        barrier.after.verify()?;
        if self.recovery.is_none() {
            self.recovery = Some(self.reopen_durable_barrier(barrier)?);
        }
        let selected = self
            .recovery
            .as_ref()
            .ok_or_else(|| HostRefusal::new("history_changed", "no selected recovery"))?;
        if !barrier
            .before
            .history
            .snapshot
            .same_encoded_state(&selected.before.history.snapshot)
            || !barrier
                .after
                .history
                .snapshot
                .same_encoded_state(&selected.after.history.snapshot)
            || self.owner()?.manual_switch_generation()? != barrier.switch_generation
            || self.recovery.as_ref() != Some(barrier)
            || self.owner()?.manual_checkpoint()?.0 != barrier.checkpoint_ref
            || self.owner()?.manual_checkpoint()?.1.identity != barrier.after.history.identity()
        {
            return Err(HostRefusal::new(
                "history_changed",
                "recovery install no longer matches its durable checkpoint",
            )
            .into());
        }
        reply.history = Some(selected.after.history.clone());
        reply.manual_ack.clone_from(&selected.acknowledged);
        reply.recovery = Some(selected.clone());
        Ok(())
    }

    fn reopen_durable_barrier(
        &self,
        barrier: &RecoveryBarrier,
    ) -> Result<RecoveryBarrier, HostError> {
        let owner = self.owner()?;
        let (reference, checkpoint) = owner.manual_checkpoint()?;
        let install = checkpoint
            .install
            .as_ref()
            .ok_or_else(|| HostRefusal::new("history_changed", "no durable recovery install"))?;
        owner.validate_manual_publisher(install.publisher)?;
        let cut = owner.freeze_manual_barrier()?;
        let before = barrier
            .before
            .history
            .clone()
            .validate(self.core().history_pool())
            .map_err(BundleError::from)?;
        if reference != barrier.checkpoint_ref
            || cut.identity() != checkpoint.identity
            || cut.change().position != checkpoint.position
            || cut.change().revision != checkpoint.revision
            || owner.manual_switch_generation()? != barrier.switch_generation
            || before.identity() != install.source
            || before.position() != install.source_position
            || barrier.before.history.branch != checkpoint.branch_id
            || barrier.before.history.revision.0.checked_add(1) != Some(checkpoint.revision.0)
            || barrier.after.history.identity() != checkpoint.identity
            || barrier.after.history.branch != checkpoint.branch_id
            || barrier.after.history.revision != checkpoint.revision
            || barrier.acknowledged.iter().any(|receipt| {
                install
                    .stream
                    .as_ref()
                    .is_none_or(|stream| !stream.receipts().any(|saved| saved == receipt))
            })
        {
            return Err(HostRefusal::new(
                "history_changed",
                "recovery evidence differs from its durable install",
            )
            .into());
        }
        drop(before);
        let after = HostSnapshot::from_history(owner.published_manual_history()?)?;
        if !after
            .history
            .snapshot
            .same_encoded_state(&barrier.after.history.snapshot)
        {
            return Err(HostRefusal::new(
                "history_changed",
                "recovery payload differs from its durable install",
            )
            .into());
        }
        Ok(RecoveryBarrier {
            client: barrier.client.clone(),
            before: barrier.before.clone(),
            after,
            acknowledged: barrier.acknowledged.clone(),
            checkpoint_ref: reference,
            switch_generation: barrier.switch_generation,
        })
    }

    pub(super) fn confirm_barrier(
        &mut self,
        barrier: &RecoveryBarrier,
        installed: &HostSnapshot,
        reply: &mut HostReply,
    ) -> Result<(), HostError> {
        installed.verify()?;
        let key = confirmation_digest(barrier)?;
        if let Some((original, confirmed)) = &self.confirmed_recovery
            && original == &key
            && installed == &barrier.after
            && confirmed.history.as_ref().is_some_and(|selected| {
                installed
                    .history
                    .snapshot
                    .same_encoded_state(&selected.snapshot)
            })
            && self
                .manual
                .as_ref()
                .map(kinewright_project::editing_session::ManualAuthority::lease)
                == confirmed.manual_lease.as_ref()
            && !self.recovering
        {
            reply.manual_lease.clone_from(&confirmed.manual_lease);
            reply.manual_inputs.clone_from(&confirmed.manual_inputs);
            reply.history.clone_from(&confirmed.history);
            return Ok(());
        }
        let trusted = self.recovery.as_ref();
        if trusted != Some(barrier)
            || trusted.is_none_or(|saved| {
                !installed
                    .history
                    .snapshot
                    .same_encoded_state(&saved.after.history.snapshot)
                    || !barrier
                        .before
                        .history
                        .snapshot
                        .same_encoded_state(&saved.before.history.snapshot)
                    || !barrier
                        .after
                        .history
                        .snapshot
                        .same_encoded_state(&saved.after.history.snapshot)
            })
            || self.owner()?.manual_switch_generation()? != barrier.switch_generation
            || installed != &barrier.after
            || self.owner()?.manual_checkpoint()?.1.identity != installed.history.identity()
            || self
                .core()
                .snapshot_identity()
                .map_err(|e| HostError::Protocol(e.to_string()))?
                != installed.history.identity()
            || self
                .core()
                .creation_branch()
                .map_err(|e| HostError::Protocol(e.to_string()))?
                != installed.history.branch
            || self
                .core()
                .change_id()
                .map_err(|e| HostError::Protocol(e.to_string()))?
                .revision
                != installed.history.revision
        {
            return Err(HostRefusal::new(
                "recovery_unconfirmed",
                "both Cores must install the same durable snapshot",
            )
            .into());
        }
        let cut = self.owner()?.freeze_manual_barrier()?;
        self.owner_mut()?.resume_manual_barrier(&cut)?;
        self.recovering = false;
        let acquisition: Result<_, HostError> = (|| {
            let lease = self.acquire_manual(barrier.client.clone())?;
            Ok((lease, self.owner()?.manual_input_context()?))
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
        let selected = self
            .recovery
            .take()
            .ok_or_else(|| HostError::Protocol("confirmed recovery disappeared".into()))?;
        reply.manual_lease = Some(lease);
        reply.manual_inputs = Some(inputs);
        // The exact reply keeps the trusted selected capture's charged buffer.
        // Its fixed confirmation digest replaces the no-longer-needed original
        // source capture; duplicate confirmation never needs that full holder.
        reply.history = Some(selected.after.history);
        self.confirmed_recovery = Some((key, reply.clone()));
        if self.message.contains(super::RECOVERY_REOPEN) {
            self.message.clear();
        }
        Ok(())
    }
}
