//! One editor and one host shadow. No subscriber is a durable transport.
use super::{
    HostAuth, HostCommand, HostCommandOwner, HostError, HostRefusal, HostReply, SessionRoots,
};
use kinewright_project::operation_protocol::EncodedMessage;
use kinewright_project::operation_protocol::Lease as ManualLease;

#[derive(Debug, Clone, serde::Serialize)]
pub(super) struct ForegroundOwner {
    pub attachment: Option<super::auth::AttachmentId>,
    pub client: String,
}
pub(super) fn command_client(command: &HostCommand) -> Option<&str> {
    match command {
        HostCommand::AcquireManual { client }
        | HostCommand::CommitHistory { client, .. }
        | HostCommand::PrepareOwnedMedia { client, .. }
        | HostCommand::PrepareMediaInterpretation { client, .. }
        | HostCommand::BeginRecovery { client, .. }
        | HostCommand::ConfirmSwitch { client, .. } => Some(client.as_str()),
        HostCommand::PublishBarrier { barrier } | HostCommand::ConfirmBarrier { barrier, .. } => {
            Some(barrier.client.as_str())
        }
        _ => None,
    }
}

pub(super) fn connector_command(command: &HostCommand) -> bool {
    matches!(
        command,
        HostCommand::IssueMetadataIssuer { .. }
            | HostCommand::DispatchOrchestration { .. }
            | HostCommand::ObserveRun { .. }
            | HostCommand::ResolveAttention { .. }
            | HostCommand::DetachForContinuation
            | HostCommand::ReviseCut { .. }
            | HostCommand::Metadata { .. }
            | HostCommand::AdmitFeedbackEvidence { .. }
            | HostCommand::VoiceReceipt { .. }
            | HostCommand::AcknowledgeVoice { .. }
            | HostCommand::StartTranscription { .. }
            | HostCommand::VoiceStatus { .. }
            | HostCommand::ChooseTranscript { .. }
            | HostCommand::ReadVoiceRange { .. }
            | HostCommand::ListFeedback
            | HostCommand::ListVersions
            | HostCommand::ReadSnapshot { .. }
            | HostCommand::LeaseSnapshot { .. }
            | HostCommand::LeaseCurrentEdit { .. }
            | HostCommand::SnapshotLeaseStatus { .. }
            | HostCommand::ReleaseSnapshot { .. }
            | HostCommand::ReadFeedbackEvidence { .. }
            | HostCommand::ReadFeedbackView { .. }
            | HostCommand::ReadFeedback { .. }
            | HostCommand::ResolveFeedback { .. }
            | HostCommand::ReadSubmittedBatch { .. }
            | HostCommand::Attach
            | HostCommand::CurrentAttachment
            | HostCommand::SwitchReceipt { .. }
            | HostCommand::Status
            | HostCommand::ReadHistory
            | HostCommand::Detach
            | HostCommand::StartSession { .. }
            | HostCommand::StartScripted { .. }
            | HostCommand::ReopenScripted { .. }
            | HostCommand::SendMessage { .. }
            | HostCommand::InterruptSession { .. }
            | HostCommand::SessionStatus { .. }
            | HostCommand::SaveSession { .. }
            | HostCommand::CloseSession { .. }
    )
}
impl HostCommandOwner {
    pub(super) fn operation_client<'a>(
        &self,
        command: &HostCommand,
        editor: Option<&'a str>,
    ) -> Result<&'a str, HostError> {
        let client = editor
            .filter(|client| (32..=128).contains(&client.len()))
            .ok_or_else(|| {
                HostRefusal::new("manual_client", "private GUI operation identity required")
            })?;
        if self
            .manual
            .as_ref()
            .is_some_and(|lease| lease.client != client)
        {
            return Err(HostRefusal::new(
                "manual_busy",
                "foreground operation belongs to the active manual lease client",
            )
            .into());
        }
        let command_client = command_client(command);
        if command_client.is_some_and(|expected| expected != client) {
            return Err(HostRefusal::new(
                "manual_client",
                "command and foreground operation must use the same client",
            )
            .into());
        }
        Ok(client)
    }

    pub(super) fn execute_operation(
        &mut self,
        command: HostCommand,
        roots: SessionRoots,
        editor: Option<&str>,
        operation: Option<u64>,
    ) -> HostReply {
        let Some(operation) = operation else {
            return self.execute_client(command, roots, editor);
        };
        let checked = (|| -> Result<_, HostError> {
            let client = self.operation_client(&command, editor)?;
            let digest = kinewright_media::sha256_bytes(&serde_json::to_vec(&(&command, &roots))?);
            let acquiring =
                self.manual.is_none() && matches!(command, HostCommand::AcquireManual { .. });
            if !acquiring
                && let Some((previous_client, previous, payload, reply)) = &self.foreground_reply
            {
                if previous_client != client
                    || operation < *previous
                    || (operation == *previous && *payload != digest)
                {
                    return Err(HostRefusal::new(
                        "operation_changed",
                        "retry must preserve the client, operation and payload",
                    )
                    .into());
                }
                if operation == *previous {
                    let mut reply = reply.clone();
                    self.prove_retention()?;
                    reply.status = self.status();
                    return Ok((client.to_owned(), digest, Some(reply)));
                }
            }
            Ok((client.to_owned(), digest, None))
        })();
        let (client, digest, retained) = match checked {
            Ok(value) => value,
            Err(error) => {
                return HostReply {
                    status: self.status(),
                    error: Some(error.refusal()),
                    ..HostReply::default()
                };
            }
        };
        if let Some(reply) = retained {
            return reply;
        }
        // A later authenticated operation acknowledges delivery of the previous
        // result. Release its full capture before preparing another one, while
        // retaining the client/sequence/digest fence even if this operation fails.
        if let Some((_, _, _, previous)) = &mut self.foreground_reply
            && (previous.history.is_some()
                || previous.switch_attachment.is_some()
                || previous.switch_preview.is_some())
        {
            *previous = HostReply {
                error: Some(HostRefusal::new(
                    "operation_changed",
                    "a later operation acknowledged this result",
                )),
                ..HostReply::default()
            };
        }
        let reply = self.execute_client(command, roots, editor);
        if reply.error.is_none() {
            self.foreground_reply = Some((client, operation, digest, reply.clone()));
        }
        reply
    }

    pub(super) fn execute_client(
        &mut self,
        command: HostCommand,
        roots: SessionRoots,
        editor: Option<&str>,
    ) -> HostReply {
        if self
            .manual
            .as_ref()
            .is_some_and(|lease| editor != Some(lease.client.as_str()))
            && !connector_command(&command)
            && !matches!(
                &command,
                HostCommand::AcquireManual { .. } | HostCommand::CommitHistory { .. }
            )
        {
            return HostReply {
                status: super::HostStatus::default(),
                error: Some(HostRefusal::new(
                    "manual_editor",
                    "this read/comment attachment cannot control the manual editor",
                )),
                ..HostReply::default()
            };
        }
        self.execute(command, roots)
    }

    pub(super) fn acquire_manual(&mut self, client: String) -> Result<ManualLease, HostError> {
        if !(32..=128).contains(&client.len())
            || !client
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"+/=".contains(&b))
        {
            return Err(HostRefusal::new(
                "manual_client",
                "a private attachment identity is required",
            )
            .into());
        }
        if self.switch_frozen {
            return Err(HostRefusal::new(
                "switch_frozen",
                "restore and confirm the current working edit before acquiring authority",
            )
            .into());
        }
        if self.recovering {
            return Err(HostRefusal::new(
                "recovery_frozen",
                "shared recovery has not been confirmed",
            )
            .into());
        }
        if self.manual.is_some() {
            return Err(HostRefusal::new(
                "manual_busy",
                "another manual editor already holds this attachment",
            )
            .into());
        }
        let token = serde_json::to_value(HostAuth::generate()?)?
            .as_str()
            .ok_or_else(|| HostError::Protocol("manual token".into()))?
            .to_owned();
        let next = self
            .next_manual_generation
            .checked_add(1)
            .ok_or_else(|| HostError::Protocol("manual lease generation exhausted".into()))?;
        let mut lease = self.owner_mut()?.issue_manual_lease(client, token)?;
        lease.generation = self.next_manual_generation;
        self.next_manual_generation = next;
        self.manual = Some(self.owner()?.manual_authority(lease.clone())?);
        Ok(lease)
    }
    pub(super) fn commit_history(
        &mut self,
        client: &str,
        messages: &[EncodedMessage],
        reply: &mut HostReply,
    ) -> Result<(), HostError> {
        if messages.len() > kinewright_project::operation_protocol::PENDING_COMMANDS
            || messages
                .iter()
                .try_fold(0_u64, |n, message| n.checked_add(message.byte_len()))
                .is_none_or(|n| n > kinewright_project::operation_protocol::PENDING_BYTES)
        {
            return Err(HostRefusal::new(
                "manual_capacity",
                "manual ingress exceeds its reserved capacity",
            )
            .into());
        }
        self.manual
            .as_ref()
            .filter(|lease| lease.client == client)
            .ok_or_else(|| {
                HostRefusal::new("manual_lease", "this client has no manual editor lease")
            })?;
        let authority = self.manual.take().expect("verified manual client");
        let outcome: Result<(), HostError> = (|| {
            for message in messages {
                let (receipt, report) = self
                    .owner_mut()?
                    .commit_manual_history(&authority, message)?;
                if let Some(report) = report {
                    self.saved(&report, reply, false)?;
                }
                reply.manual_ack.push(receipt);
            }
            Ok(())
        })();
        self.manual = Some(authority);
        outcome?;
        if !messages.is_empty() {
            self.prepared_owned_media = None;
        }
        self.message = "Accepted manual history is durably saved.".into();
        Ok(())
    }
}
