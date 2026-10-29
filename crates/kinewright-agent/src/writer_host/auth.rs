//! Private attachment credentials and read-only path admission.
#![allow(clippy::missing_errors_doc)]
use super::{HostDestination, HostError, HostRefusal, SessionRoots};
use base64::Engine as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt::Write as _;
use std::{
    fmt,
    fs::File,
    path::{Path, PathBuf},
};

/// A runtime secret. Debug never renders its bytes; serialization is only for private bootstrap.
#[derive(Clone)]
pub struct HostAuth([u8; 32]);
impl HostAuth {
    pub fn generate() -> Result<Self, HostError> {
        let mut token = [0; 32];
        getrandom::fill(&mut token)
            .map_err(|_| HostError::Protocol("OS randomness unavailable".into()))?;
        Ok(Self(token))
    }
    pub(crate) fn bearer(&self) -> String {
        format!(
            "Bearer {}",
            base64::engine::general_purpose::STANDARD.encode(self.0)
        )
    }
    pub(crate) fn accepts(&self, value: Option<&str>) -> bool {
        let expected = self.bearer();
        let supplied = value.unwrap_or_default().as_bytes();
        if supplied.len() != expected.len() {
            return false;
        }
        supplied
            .iter()
            .zip(expected.bytes())
            .fold(0_u8, |diff, (a, b)| diff | (a ^ b))
            == 0
    }
}
impl fmt::Debug for HostAuth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("HostAuth([REDACTED])")
    }
}
impl Serialize for HostAuth {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&base64::engine::general_purpose::STANDARD.encode(self.0))
    }
}
impl<'de> Deserialize<'de> for HostAuth {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let encoded = String::deserialize(deserializer)?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .map_err(|_| serde::de::Error::custom("invalid private attachment credential"))?;
        Ok(Self(bytes.try_into().map_err(|_| {
            serde::de::Error::custom("attachment credential must have 256 bits")
        })?))
    }
}

pub(super) const ATTACHMENT_CAPACITY: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub(super) struct AttachmentId(u64);

pub(super) struct ReadPin {
    pub roots: Vec<kinewright_project::bundle_records::ObjectRef>,
    pub pin: kinewright_project::store::StorePin,
}
impl ReadPin {
    fn same_reference(&self, other: &Self) -> bool {
        self.pin.shares_owner_with(&other.pin) && self.roots == other.roots
    }
}

pub(super) struct Attachment {
    pub alive: std::sync::Arc<std::sync::atomic::AtomicBool>,
    pub producer_scope: Option<super::orchestration::ProducerScope>,
    pub person_facing: bool,
    pub id: AttachmentId,
    pub auth: HostAuth,
    pub incarnation: String,
    pub project: String,
    #[allow(
        dead_code,
        reason = "subject provenance is retained, never used as foreground authority"
    )]
    pub subject: String,
    pub role: super::AttachmentRole,
    pub roots: SessionRoots,
    pub bootstrap: bool,
    pub read_pins: Vec<ReadPin>,
    pub snapshot_leases: std::collections::BTreeMap<String, super::snapshot_lease::LeasePin>,
}
impl Drop for Attachment {
    fn drop(&mut self) {
        self.alive
            .store(false, std::sync::atomic::Ordering::Release);
    }
}

impl Attachment {
    #[expect(
        clippy::match_same_arms,
        reason = "Manual and switch phases have explicit authority rules and independent refusal witnesses"
    )]
    pub fn permits(&self, command: &super::HostCommand) -> bool {
        use super::{AttachmentRole, HostCommand};
        match command {
            HostCommand::IssueMetadataIssuer { .. } | HostCommand::ObserveRun { .. } => true,
            HostCommand::DispatchOrchestration { .. }
            | HostCommand::ResolveAttention { .. }
            | HostCommand::DetachForContinuation => true,
            HostCommand::Metadata { update, .. } => match &update.change {
                kinewright_project::studio_records::MetadataChange::PublishSnapshot(_) => {
                    self.role == AttachmentRole::Producer
                }
                kinewright_project::studio_records::MetadataChange::SaveFeedback(_)
                | kinewright_project::studio_records::MetadataChange::SaveVoice(_)
                | kinewright_project::studio_records::MetadataChange::SubmitBatch { .. } => {
                    matches!(self.role, AttachmentRole::Viewer | AttachmentRole::Editor)
                }
                kinewright_project::studio_records::MetadataChange::Orchestration { .. }
                | kinewright_project::studio_records::MetadataChange::OrchestrationUpdate {
                    ..
                } => false,
            },
            HostCommand::AdmitFeedbackEvidence { .. }
            | HostCommand::VoiceReceipt { .. }
            | HostCommand::AcknowledgeVoice { .. }
            | HostCommand::StartTranscription { .. }
            | HostCommand::VoiceStatus { .. }
            | HostCommand::ChooseTranscript { .. }
            | HostCommand::ReadVoiceRange { .. }
            | HostCommand::ReadFeedbackEvidence { .. }
            | HostCommand::ReadFeedbackView { .. } => {
                matches!(self.role, AttachmentRole::Viewer | AttachmentRole::Editor)
            }
            HostCommand::ListVersions
            | HostCommand::LeaseSnapshot { .. }
            | HostCommand::SnapshotLeaseStatus { .. }
            | HostCommand::ReleaseSnapshot { .. }
            | HostCommand::ReadSnapshot { .. }
            | HostCommand::ReadSubmittedBatch { .. } => true,
            HostCommand::ReadFeedback { .. } | HostCommand::ListFeedback => {
                self.role != AttachmentRole::Producer
            }
            HostCommand::Attach
            | HostCommand::Status
            | HostCommand::ReadHistory
            | HostCommand::CurrentAttachment
            | HostCommand::Detach => true,
            HostCommand::AcquireManual { .. } => self.role == AttachmentRole::Editor,
            HostCommand::CommitHistory { .. } => self.role == AttachmentRole::Editor,
            HostCommand::PrepareSwitch { .. } => self.role == AttachmentRole::Editor,
            HostCommand::Switch { .. } => self.role == AttachmentRole::Editor,
            HostCommand::SwitchReceipt { .. } => self.role == AttachmentRole::Editor,
            HostCommand::ConfirmSwitch { .. } => self.role == AttachmentRole::Editor,
            HostCommand::AdmitDestination { .. } => {
                self.role == AttachmentRole::Editor && self.bootstrap
            }
            _ => self.role == AttachmentRole::Editor,
        }
    }
}

impl super::HostCommandOwner {
    /// The local owner is the trusted issuer. No request may call this port.
    pub fn issue_attachment(
        &mut self,
        subject: String,
        role: super::AttachmentRole,
        roots: SessionRoots,
    ) -> Result<HostAuth, HostError> {
        let auth = HostAuth::generate()?;
        self.register_attachment(auth.clone(), subject, role, roots, false)?;
        Ok(auth)
    }

    #[expect(
        clippy::needless_pass_by_value,
        reason = "Issuance owns the submitted scope before retaining its canonical form"
    )]
    pub(super) fn register_attachment(
        &mut self,
        auth: HostAuth,
        subject: String,
        role: super::AttachmentRole,
        roots: SessionRoots,
        bootstrap: bool,
    ) -> Result<(), HostError> {
        if role == super::AttachmentRole::Editor
            && self
                .attachments
                .iter()
                .any(|grant| grant.role == super::AttachmentRole::Editor)
        {
            return Err(HostRefusal::new("attachment_role", "one live Editor attachment").into());
        }
        if self.attachments.len() >= ATTACHMENT_CAPACITY {
            return Err(
                HostRefusal::new("attachment_capacity", "64 active attachments maximum").into(),
            );
        }
        if subject.is_empty() || subject.len() > 256 {
            return Err(HostRefusal::new(
                "attachment_subject",
                "bounded nonempty subject required",
            )
            .into());
        }
        let roots = roots.canonicalize()?;
        Self::check_narrowed_roots(&self.config.roots, &roots)?;
        let next = self.next_attachment_id.checked_add(1).ok_or_else(|| {
            HostRefusal::new("attachment_capacity", "attachment identifiers exhausted")
        })?;
        let id = AttachmentId(self.next_attachment_id);
        self.next_attachment_id = next;
        self.attachments.push(Attachment {
            alive: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true)),
            producer_scope: None,
            person_facing: bootstrap && matches!(self.config.origin, super::HostOrigin::Gui { .. }),
            id,
            auth,
            incarnation: self.instance.clone(),
            project: self.config.project.clone(),
            subject,
            role,
            roots,
            bootstrap,
            read_pins: Vec::new(),
            snapshot_leases: std::collections::BTreeMap::new(),
        });
        Ok(())
    }

    fn detached_handoff_client(&self) -> Option<&str> {
        if self.manual.is_some() {
            return None;
        }
        let owner = self.foreground_owner.as_ref()?;
        let (_, _, _, reply) = self.foreground_reply.as_ref()?;
        (owner.attachment.is_none() && (reply.saved.is_some() || reply.switch_result.is_some()))
            .then_some(owner.client.as_str())
    }

    /// Trusted local issuance port. No HTTP or MCP request exposes this action.
    pub fn discard_detached_handoff(&mut self, expected_client: &str) -> Result<(), HostError> {
        let retained = self.detached_handoff_client().ok_or_else(|| {
            HostRefusal::new(
                "handoff_changed",
                "no detached handoff without a live lease",
            )
        })?;
        if retained != expected_client {
            return Err(
                HostRefusal::new("handoff_changed", "detached handoff client changed").into(),
            );
        }
        self.foreground_reply = None;
        self.foreground_owner = None;
        Ok(())
    }

    pub(super) fn resolve_attachment(&self, bearer: Option<&str>) -> Result<usize, HostError> {
        self.attachments
            .iter()
            .position(|grant| {
                grant.auth.accepts(bearer)
                    && grant.alive.load(std::sync::atomic::Ordering::Acquire)
                    && grant.incarnation == self.instance
                    && grant.project == self.config.project
            })
            .ok_or_else(|| {
                HostRefusal::new(
                    "attachment_refused",
                    "attachment is unknown, revoked or expired",
                )
                .into()
            })
    }

    fn check_narrowed_roots(
        admitted: &SessionRoots,
        roots: &SessionRoots,
    ) -> Result<(), HostError> {
        if roots.read.iter().any(|root| !admitted.permits(root, false))
            || roots.write.iter().any(|root| !admitted.permits(root, true))
        {
            return Err(
                HostRefusal::roots("attachment roots cannot widen their admitted scope").into(),
            );
        }
        Ok(())
    }

    /// Direct-owner ingress uses exactly the HTTP/MCP authorization boundary.
    pub fn execute_attached(
        &mut self,
        auth: &HostAuth,
        request: super::HostRequest,
    ) -> super::HostReply {
        self.execute_bearer(Some(&auth.bearer()), request)
    }

    pub(super) fn authorize_request(
        &self,
        bearer: Option<&str>,
        request: &super::HostRequest,
    ) -> Result<(usize, AttachmentId, SessionRoots), HostError> {
        (|| -> Result<_, HostError> {
            let index = self.resolve_attachment(bearer)?;
            let grant = &self.attachments[index];
            if request.project != grant.project || request.protocol != super::HOST_PROTOCOL {
                return Err(HostRefusal::new(
                    "project_refused",
                    "exact attachment project and protocol required",
                )
                .into());
            }
            if request.operation.is_some() && grant.role != super::AttachmentRole::Editor {
                return Err(HostRefusal::new(
                    "role_refused",
                    "foreground operations require an Editor attachment",
                )
                .into());
            }
            if !grant.permits(&request.command) {
                return Err(HostRefusal::new(
                    "role_refused",
                    "command is outside this attachment role",
                )
                .into());
            }
            if let Some(retained) = self.detached_handoff_client()
                && (request.operation.is_some()
                    || !super::manual::connector_command(&request.command))
                && super::manual::command_client(&request.command).or(request.editor.as_deref())
                    != Some(retained)
            {
                return Err(HostRefusal::new(
                    "handoff_pending",
                    "trusted issuer must discard the detached handoff before changing client",
                )
                .into());
            }
            if request.operation.is_some() {
                let client = self.operation_client(&request.command, request.editor.as_deref())?;
                if let Some(owner) = &self.foreground_owner {
                    let permitted = if self.manual.is_some() {
                        owner.attachment.is_none() || owner.attachment == Some(grant.id)
                    } else {
                        ((owner.attachment.is_none() || owner.attachment == Some(grant.id))
                            && owner.client == client)
                            || matches!(request.command, super::HostCommand::AcquireManual { .. })
                    };
                    if !permitted {
                        return Err(HostRefusal::new(
                            "operation_changed",
                            "foreground reply belongs to another attachment instance",
                        )
                        .into());
                    }
                }
            }
            if let super::HostCommand::Metadata { envelope, .. }
            | super::HostCommand::ChooseTranscript { envelope, .. } = &request.command
            {
                self.require_metadata_issuer(index, &envelope.id.issuer)?;
            }
            if let super::HostCommand::AcknowledgeVoice { upload }
            | super::HostCommand::VoiceReceipt { upload } = &request.command
            {
                self.require_metadata_issuer(index, &upload.envelope.id.issuer)?;
            }
            // Expiry cannot prevent an attachment from releasing its own lease.
            // Release checks the attachment-owned id and digest and grants no reads.
            if !matches!(request.command, super::HostCommand::ReleaseSnapshot { .. }) {
                self.check_producer_attachment(index, &request.command)?;
            }
            let roots = request.roots.canonicalize()?;
            let mut command_roots = grant.roots.clone();
            if grant.bootstrap
                && grant.role == super::AttachmentRole::Editor
                && matches!(self.config.origin, super::HostOrigin::Gui { .. })
                && let super::HostCommand::PrepareOwnedMedia { path, .. } = &request.command
            {
                // The private GUI's file picker admits this command's source parent only.
                // It grants no persistent root and no authority to a delegated attachment.
                let parent = path
                    .parent()
                    .ok_or_else(|| HostRefusal::roots("chosen source parent"))?;
                command_roots.read.push(parent.canonicalize()?);
            }
            Self::check_narrowed_roots(&command_roots, &roots)?;
            if grant.read_pins.len() >= 64 {
                let read_roots = match &request.command {
                    super::HostCommand::ReadSnapshot { key } => {
                        let pinned = self.owner()?.read_snapshot(key)?;
                        Some(ReadPin {
                            roots: pinned.roots,
                            pin: pinned.pin,
                        })
                    }
                    super::HostCommand::ReadFeedback { reference } => {
                        let pinned = self.owner()?.read_feedback(reference)?;
                        Some(ReadPin {
                            roots: pinned.roots,
                            pin: pinned.pin,
                        })
                    }
                    super::HostCommand::ReadSubmittedBatch { batch } => {
                        let pinned = self.owner()?.read_submitted_batch(batch)?;
                        Some(ReadPin {
                            roots: pinned.roots,
                            pin: pinned.pin,
                        })
                    }
                    super::HostCommand::AdmitFeedbackEvidence { bytes, .. } => {
                        let sha256 = kinewright_media::sha256_bytes(bytes);
                        if !grant
                            .read_pins
                            .iter()
                            .any(|pin| pin.roots.len() == 1 && pin.roots[0].sha256 == sha256)
                        {
                            return Err(HostRefusal::new(
                                "attachment_capacity",
                                "64 distinct pinned reads per attachment maximum",
                            )
                            .into());
                        }
                        None
                    }
                    _ => None,
                };
                if let Some(read_roots) = read_roots
                    && !grant
                        .read_pins
                        .iter()
                        .any(|pin| pin.same_reference(&read_roots))
                {
                    return Err(HostRefusal::new(
                        "attachment_capacity",
                        "64 distinct pinned reads per attachment maximum",
                    )
                    .into());
                }
            }
            if request.operation.is_none()
                && self.manual.is_some()
                && matches!(
                    request.command,
                    super::HostCommand::ConfirmSwitch { .. }
                        | super::HostCommand::PrepareOwnedMedia { .. }
                        | super::HostCommand::PrepareMediaInterpretation { .. }
                        | super::HostCommand::ConfirmBarrier { .. }
                )
                && self
                    .foreground_owner
                    .as_ref()
                    .is_some_and(|owner| owner.attachment.is_some_and(|id| id != grant.id))
            {
                return Err(HostRefusal::new(
                    "operation_changed",
                    "manual lease renewal belongs to another attachment instance",
                )
                .into());
            }
            Ok((index, grant.id, roots))
        })()
    }
    pub(super) fn execute_bearer(
        &mut self,
        bearer: Option<&str>,
        request: super::HostRequest,
    ) -> super::HostReply {
        let checked = self.authorize_request(bearer, &request);
        let (index, attachment, roots) = match checked {
            Ok(value) => value,
            Err(error) => {
                return super::HostReply {
                    status: self.status(),
                    error: Some(error.refusal()),
                    ..super::HostReply::default()
                };
            }
        };
        let detach = matches!(
            request.command,
            super::HostCommand::Detach | super::HostCommand::DetachForContinuation
        );
        let admit = matches!(request.command, super::HostCommand::AdmitDestination { .. });
        let operation = request.operation;
        // A published revision's evidence is durable through the feedback index; its
        // admission pin is released so it does not occupy the attachment's slots.
        let published_evidence = match &request.command {
            super::HostCommand::Metadata { update, .. } => match &update.change {
                kinewright_project::studio_records::MetadataChange::SaveFeedback(revision) => vec![
                    revision.original_text_ref.clone(),
                    revision.strokes_ref.clone(),
                    revision.frame_proof_ref.clone(),
                ],
                kinewright_project::studio_records::MetadataChange::SaveVoice(voice) => {
                    voice.chosen_transcript_revision.iter().cloned().collect()
                }
                _ => vec![],
            },
            super::HostCommand::ChooseTranscript { text_ref, .. } => vec![text_ref.clone()],
            _ => vec![],
        };
        let reply = if super::snapshot_lease::is_lease_command(&request.command) {
            self.execute_snapshot_lease(index, request.command, &roots)
        } else if super::orchestration::is_hook(&request.command) {
            self.execute_hook(index, request.command, &roots)
        } else if detach {
            super::HostReply::default()
        } else {
            self.execute_operation(request.command, roots, request.editor.as_deref(), operation)
        };
        if let Some(pin) = self.pending_read_pin.take()
            && !self.attachments[index]
                .read_pins
                .iter()
                .any(|held| held.same_reference(&pin))
        {
            self.attachments[index].read_pins.push(pin);
        }
        if reply.error.is_none()
            && (reply.metadata.is_some() || reply.voice.is_some())
            && !published_evidence.is_empty()
        {
            self.attachments[index].read_pins.retain(|pin| {
                !(pin.roots.len() == 1 && published_evidence.contains(&pin.roots[0]))
            });
        }
        if reply.error.is_none() {
            if let Some(lease) = &reply.manual_lease {
                if operation.is_none() {
                    self.foreground_reply = None;
                }
                self.foreground_owner = Some(super::manual::ForegroundOwner {
                    attachment: Some(attachment),
                    client: lease.client.clone(),
                });
            } else if operation.is_some()
                && let Some((client, _, _, _)) = &self.foreground_reply
            {
                self.foreground_owner = Some(super::manual::ForegroundOwner {
                    attachment: Some(attachment),
                    client: client.clone(),
                });
            } else if !detach
                && self.attachments[index].role == super::AttachmentRole::Editor
                && let Some(lease) = &self.manual
                && request.editor.as_deref() == Some(lease.client.as_str())
                && self
                    .foreground_owner
                    .as_ref()
                    .is_none_or(|owner| owner.attachment.is_none())
            {
                self.foreground_owner = Some(super::manual::ForegroundOwner {
                    attachment: Some(attachment),
                    client: lease.client.clone(),
                });
            }
            if admit {
                self.attachments[index].roots = reply.status.admitted_roots.clone();
            }
            if detach {
                let retained_handoff =
                    self.foreground_reply
                        .as_ref()
                        .is_some_and(|(_, _, _, reply)| {
                            reply.saved.is_some() || reply.switch_result.is_some()
                        });
                let removed = self.attachments.remove(index);
                self.revoke_task_endpoints(removed.producer_scope.as_ref());
                if let Some(owner) = &mut self.foreground_owner
                    && owner.attachment == Some(removed.id)
                {
                    if self.manual.is_some() || retained_handoff {
                        owner.attachment = None;
                    } else {
                        self.foreground_reply = None;
                        self.foreground_owner = None;
                    }
                }
            }
        }
        reply
    }
}

impl SessionRoots {
    /// JSON stays ASCII in HTTP headers; decoding restores each exact Unicode path.
    pub(crate) fn wire_header(&self) -> Result<String, HostError> {
        let json = serde_json::to_string(self)?;
        let mut encoded = String::with_capacity(json.len());
        for unit in json.encode_utf16() {
            match u8::try_from(unit) {
                // Keep connector environment expansion confined to its explicit auth template.
                Ok(byte) if byte.is_ascii() && byte != b'$' => encoded.push(char::from(byte)),
                _ => write!(&mut encoded, "\\u{unit:04x}")
                    .map_err(|_| HostError::Protocol("root header encoding failed".into()))?,
            }
        }
        Ok(encoded)
    }

    pub fn canonicalize(&self) -> Result<Self, HostError> {
        if self.read.len() + self.write.len() > 64 {
            return Err(HostRefusal::roots("too many roots").into());
        }
        let canonical = |roots: &[PathBuf]| {
            roots
                .iter()
                .map(|path| {
                    let path = path
                        .canonicalize()
                        .map_err(|_| HostRefusal::roots("root is unavailable"))?;
                    if !path.is_dir() {
                        return Err(HostRefusal::roots("root is not a directory").into());
                    }
                    Ok(path)
                })
                .collect::<Result<Vec<_>, HostError>>()
        };
        Ok(Self {
            read: canonical(&self.read)?,
            write: canonical(&self.write)?,
        })
    }
    pub(crate) fn permits(&self, path: &Path, write: bool) -> bool {
        self.write
            .iter()
            .chain(if write { [].iter() } else { self.read.iter() })
            .any(|root| path.starts_with(root))
    }
}

/// Keeps the admitted parent alive through namespace acquisition. Filesystem operations use
/// this fd-relative spelling, never the caller's mutable parent spelling.
pub(crate) struct PinnedPath {
    pub display: PathBuf,
    pub io: PathBuf,
    pub inspected: HostDestination,
    pub anchor: PathBuf,
    #[cfg_attr(
        all(not(target_os = "linux"), not(test)),
        expect(
            dead_code,
            reason = "Only the Linux namespace reads the held directory; other production profiles refuse Studio IO"
        )
    )]
    #[cfg_attr(
        all(not(target_os = "linux"), test),
        allow(
            dead_code,
            reason = "Unsupported-profile tests exercise refusal without reading native directory handles"
        )
    )]
    parent: File,
}
impl PinnedPath {
    #[cfg(target_os = "linux")]
    pub fn admit(
        path: &Path,
        configured: &SessionRoots,
        forwarded: &SessionRoots,
        write: bool,
    ) -> Result<Self, HostError> {
        use std::os::{fd::AsRawFd as _, unix::fs::MetadataExt as _};
        let name = path
            .file_name()
            .ok_or_else(|| HostRefusal::roots("project filename required"))?;
        if name == "." || name == ".." {
            return Err(HostRefusal::roots("invalid filename").into());
        }
        let absolute = if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()?.join(path)
        };
        let mut ancestor = absolute
            .parent()
            .ok_or_else(|| HostRefusal::roots("missing destination parent"))?
            .to_path_buf();
        let mut missing = Vec::new();
        loop {
            match std::fs::symlink_metadata(&ancestor) {
                Ok(_) => break,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    if missing.len() >= 32 {
                        return Err(HostRefusal::roots("destination ancestry capacity").into());
                    }
                    missing.push(
                        ancestor
                            .file_name()
                            .ok_or_else(|| HostRefusal::roots("invalid destination ancestor"))?
                            .to_os_string(),
                    );
                    ancestor = ancestor
                        .parent()
                        .ok_or_else(|| HostRefusal::roots("missing existing ancestor"))?
                        .into();
                }
                Err(error) => return Err(error.into()),
            }
        }
        let canonical = ancestor.canonicalize()?;
        let handle = File::open(&canonical)?;
        let pinned = PathBuf::from(format!(
            "/proc/{}/fd/{}",
            std::process::id(),
            handle.as_raw_fd()
        ));
        let observed = pinned.canonicalize()?;
        if observed != canonical
            || !configured.permits(&observed, write)
            || !forwarded.permits(&observed, write)
        {
            return Err(
                HostRefusal::roots("path is outside the configured and forwarded roots").into(),
            );
        }
        let mut relative = PathBuf::new();
        for component in missing.into_iter().rev() {
            relative.push(component);
        }
        relative.push(name);
        kinewright_project::namespace::PortablePath::new(&relative)?;
        let target = pinned.join(&relative);
        if std::fs::symlink_metadata(&target).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err(HostRefusal::roots(
                "project symlinks require explicit admission of their resolved path",
            )
            .into());
        }
        let identity = handle.metadata()?;
        let inspected = HostDestination {
            // Resolve from the actual held parent fd once, not from the caller alias.
            // Both destination inspectors must prove this same native parent before IO.
            path: observed.join(&relative),
            parent_device: identity.dev(),
            parent_inode: identity.ino(),
        };
        Ok(Self {
            display: path.to_path_buf(),
            io: target,
            inspected,
            anchor: observed,
            parent: handle,
        })
    }
    #[cfg(target_os = "linux")]
    pub fn duplicate(&self) -> Result<Self, HostError> {
        use std::os::fd::AsRawFd as _;
        let parent = self.parent.try_clone()?;
        let relative = self
            .inspected
            .path
            .strip_prefix(&self.anchor)
            .map_err(|_| HostRefusal::roots("destination outside held ancestor"))?;
        let io = PathBuf::from(format!(
            "/proc/{}/fd/{}",
            std::process::id(),
            parent.as_raw_fd()
        ))
        .join(relative);
        Ok(Self {
            display: self.display.clone(),
            io,
            inspected: self.inspected.clone(),
            anchor: self.anchor.clone(),
            parent,
        })
    }
    /// Walk only below the held ancestor fd. Missing descendant directories are legitimate
    /// before `prepare_destination` creates them; symlinks and substitutions are refused.
    #[cfg(target_os = "linux")]
    pub fn matches(&self, target: &HostDestination) -> Result<bool, HostError> {
        use rustix::fs::{Mode, OFlags, openat};
        use std::os::unix::fs::MetadataExt as _;
        if target.path != self.inspected.path {
            return Ok(false);
        }
        let relative = self
            .inspected
            .path
            .strip_prefix(&self.anchor)
            .map_err(|_| HostRefusal::roots("destination outside held ancestor"))?;
        let mut parent = self.parent.try_clone()?;
        for component in relative
            .parent()
            .unwrap_or_else(|| Path::new(""))
            .components()
        {
            let std::path::Component::Normal(component) = component else {
                return Err(HostRefusal::roots("destination traversal").into());
            };
            match openat(
                &parent,
                component,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            ) {
                Ok(fd) => parent = File::from(fd),
                Err(rustix::io::Errno::NOENT) => break,
                Err(error) => return Err(std::io::Error::from(error).into()),
            }
        }
        let actual = parent.metadata()?;
        Ok(actual.dev() == target.parent_device && actual.ino() == target.parent_inode)
    }
    #[cfg(not(target_os = "linux"))]
    #[allow(
        clippy::unused_self,
        reason = "Unsupported profiles preserve the instance capability API while refusing IO"
    )]
    pub fn matches(&self, _target: &HostDestination) -> Result<bool, HostError> {
        Err(HostRefusal::new("durability_unsupported", "pinned Studio IO is unsupported").into())
    }
    #[cfg(not(target_os = "linux"))]
    #[allow(
        clippy::unused_self,
        reason = "Unsupported profiles preserve the instance capability API while refusing IO"
    )]
    pub fn duplicate(&self) -> Result<Self, HostError> {
        Err(HostRefusal::new("durability_unsupported", "pinned Studio IO is unsupported").into())
    }
    #[cfg(not(target_os = "linux"))]
    pub fn admit(
        path: &Path,
        configured: &SessionRoots,
        forwarded: &SessionRoots,
        write: bool,
    ) -> Result<Self, HostError> {
        let _ = (path, configured, forwarded, write);
        // Stable std cannot certify a portable directory-relative Studio writer. This is the
        // project's existing platform refusal, before any namespace or source mutation.
        Err(HostRefusal::new(
            "durability_unsupported",
            "identity-pinned Studio IO is unsupported on this platform",
        )
        .into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn credentials_are_full_width_redacted_and_exact() {
        let auth = HostAuth::generate().unwrap();
        let other = HostAuth::generate().unwrap();
        assert!(auth.accepts(Some(&auth.bearer())));
        assert!(!auth.accepts(None));
        assert!(!auth.accepts(Some(&other.bearer())));
        assert!(!auth.accepts(Some("Bearer short")));
        assert_eq!(format!("{auth:?}"), "HostAuth([REDACTED])");
        let value = serde_json::to_string(&auth).unwrap();
        let roundtrip: HostAuth = serde_json::from_str(&value).unwrap();
        assert!(auth.accepts(Some(&roundtrip.bearer())));
        assert!(serde_json::from_str::<HostAuth>("\"YWJj\"").is_err());
    }
    #[test]
    fn forwarded_roots_can_only_narrow_the_admission() {
        let roots = SessionRoots {
            read: vec![PathBuf::from("/input")],
            write: vec![PathBuf::from("/output")],
        };
        assert!(roots.permits(Path::new("/input/source.mov"), false));
        assert!(!roots.permits(Path::new("/input/source.mov"), true));
        assert!(roots.permits(Path::new("/output/edit.kinewright"), true));
        assert!(!roots.permits(Path::new("/output-other/edit.kinewright"), true));
    }
}
