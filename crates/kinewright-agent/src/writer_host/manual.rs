//! One editor and one host shadow. No subscriber is a durable transport.
use super::{
    HostAuth, HostCommand, HostCommandOwner, HostError, HostRefusal, HostReply, SessionRoots,
};
use kinewright_core::AcceptedHistory;
use kinewright_project::{
    manual_history::{AcceptedManualCommand, ManualLease},
    receipts::ReceiptLookup,
};
pub(super) fn connector_command(command: &HostCommand) -> bool {
    matches!(
        command,
        HostCommand::Attach
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
            let client = editor
                .filter(|client| (32..=128).contains(&client.len()))
                .ok_or_else(|| {
                    HostRefusal::new("manual_client", "private GUI operation identity required")
                })?;
            let digest = kinewright_media::sha256_bytes(&serde_json::to_vec(&(&command, &roots))?);
            if let Some((previous_client, previous, payload, reply)) = &self.foreground_reply {
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
                    let reply = reply.clone();
                    self.prove_retention()?;
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
        let reply = self.execute_client(command, roots, editor);
        // A single serialized GUI request is outstanding. Heartbeats and manual
        // publications cannot evict its result, including across an authority change.
        self.foreground_reply = Some((client, operation, digest, reply.clone()));
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
        self.manual = Some(lease.clone());
        Ok(lease)
    }
    pub(super) fn commit_history(
        &mut self,
        client: &str,
        messages: &[AcceptedHistory],
        reply: &mut HostReply,
    ) -> Result<(), HostError> {
        let lease = self
            .manual
            .as_ref()
            .filter(|lease| lease.client == client)
            .cloned()
            .ok_or_else(|| {
                HostRefusal::new("manual_lease", "this client has no manual editor lease")
            })?;
        if messages.len() > kinewright_core::MANUAL_COMMANDS
            || serde_json::to_vec(messages)?.len() > kinewright_core::MANUAL_BYTES + 128
        {
            return Err(HostRefusal::new(
                "manual_capacity",
                "manual ingress exceeds its reserved capacity",
            )
            .into());
        }
        for message in messages {
            let envelope = AcceptedManualCommand::decode(message)?.envelope;
            if envelope.manual.as_ref().map(|m| &m.binding) != Some(&lease) {
                return Err(HostRefusal::new("manual_lease", "stale manual lease").into());
            }
            let (result, report) = self.owner_mut()?.commit_manual_history(&lease, message)?;
            if !matches!(
                result,
                ReceiptLookup::Committed(_) | ReceiptLookup::Retired(_)
            ) {
                return Err(HostRefusal::new(
                    "manual_pending",
                    "manual publication has not been proved durable; editing remains pending",
                )
                .into());
            }
            if let Some(report) = report {
                self.saved(&report, reply, false)?;
            }
            reply.manual_ack.push(message.clone());
        }
        self.message = "Accepted manual history is durably saved.".into();
        Ok(())
    }
}
