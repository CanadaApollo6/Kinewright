//! Editorial records retained by Studio versions and project roots.
use crate::{
    bundle::BundleError,
    bundle_records::{ObjectRef, ProjectId, RecordKind},
    orchestration_records::{ExecutionIdentity, Role},
    studio_records::{AudioRange, SnapshotKey, VersionId},
};
use serde::{Deserialize, Serialize};

pub const MAX_AUDIT_ENTRIES: usize = 32;
pub const MAX_AUDIT_TEXT: usize = 512;
pub const MAX_BEAT_TEXT: usize = 128;
pub const MAX_RULES: usize = 24;
pub const MAX_RULE_TEXT: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditKind {
    CutRationale,
    Retention,
    Hook,
    Story,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditFinding {
    MissingRequiredBeat,
    MisleadingCut,
    Taste,
    Resolved,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditDisposition {
    Blocking,
    Advisory,
    Resolved,
}

impl AuditFinding {
    #[must_use]
    pub const fn disposition(self) -> AuditDisposition {
        match self {
            Self::MissingRequiredBeat | Self::MisleadingCut => AuditDisposition::Blocking,
            Self::Taste => AuditDisposition::Advisory,
            Self::Resolved => AuditDisposition::Resolved,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuditObservation {
    pub line_or_beat: String,
    pub output: AudioRange,
    pub text: String,
    pub kind: AuditKind,
    pub finding: AuditFinding,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuditEntry {
    pub observation: AuditObservation,
    pub disposition: AuditDisposition,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuditSubmission {
    pub version: VersionId,
    pub snapshot: SnapshotKey,
    pub rulebook_ref: Option<ObjectRef>,
    pub entries: Vec<AuditObservation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuditPart {
    pub attempt: ExecutionIdentity,
    pub snapshot: SnapshotKey,
    pub rulebook_ref: Option<ObjectRef>,
    pub entries: Vec<AuditEntry>,
}

impl AuditSubmission {
    #[must_use]
    pub fn attribute(self, attempt: ExecutionIdentity) -> (VersionId, AuditPart) {
        let entries = self
            .entries
            .into_iter()
            .map(|observation| AuditEntry {
                disposition: observation.finding.disposition(),
                observation,
            })
            .collect();
        (
            self.version,
            AuditPart {
                attempt,
                snapshot: self.snapshot,
                rulebook_ref: self.rulebook_ref,
                entries,
            },
        )
    }
}

pub(crate) fn validate_part(part: &AuditPart, role: Role) -> Result<(), BundleError> {
    part.attempt
        .validate()
        .map_err(|_| BundleError::Corrupt("editorial audit attempt".into()))?;
    crate::studio_records::kind(&part.snapshot.input_manifest_ref, RecordKind::InputManifest)?;
    let entries = &part.entries;
    if entries.len() > MAX_AUDIT_ENTRIES {
        return Err(BundleError::Identity(
            "editorial audit entries exceed the limit".into(),
        ));
    }
    if let Some(rulebook) = &part.rulebook_ref {
        crate::studio_records::kind(rulebook, RecordKind::RulebookRevision)?;
    }
    for entry in entries {
        let entry = &entry.observation;
        if (entry.kind == AuditKind::CutRationale) != (role == Role::Editor) {
            return Err(BundleError::Identity("editorial audit entry role".into()));
        }
        if !valid_text(&entry.line_or_beat, MAX_BEAT_TEXT)
            || !valid_text(&entry.text, MAX_AUDIT_TEXT)
        {
            return Err(BundleError::Identity(
                "editorial audit text exceed the limit".into(),
            ));
        }
        if entry.output.start_micros >= entry.output.end_micros {
            return Err(BundleError::Corrupt("editorial audit interval".into()));
        }
    }
    if role == Role::Reviewer
        && [AuditKind::Retention, AuditKind::Hook, AuditKind::Story]
            .into_iter()
            .any(|kind| !entries.iter().any(|entry| entry.observation.kind == kind))
    {
        return Err(BundleError::Corrupt(
            "unanswered editorial checklist item".into(),
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EditorialRationale(pub AuditPart);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EditorialAudit {
    pub review: AuditPart,
    pub rationale: Option<AuditPart>,
}

impl EditorialAudit {
    #[must_use]
    pub fn snapshot(&self) -> &SnapshotKey {
        &self.review.snapshot
    }

    #[must_use]
    pub fn rulebook_ref(&self) -> Option<&ObjectRef> {
        self.review.rulebook_ref.as_ref()
    }

    pub fn entries(&self) -> impl Iterator<Item = &AuditEntry> {
        self.rationale
            .iter()
            .flat_map(|part| &part.entries)
            .chain(&self.review.entries)
    }
}

/// Parent identifies the preceding full revision. It is lineage, so the current
/// revision does not keep otherwise-unreferenced ancestors alive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RulebookRevision {
    pub parent: Option<ObjectRef>,
    pub rules: Vec<String>,
    pub proposal_ref: ObjectRef,
    pub approved_by: RulebookApprover,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RulebookApprover {
    ProjectOwner(ProjectId),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RulebookProposal {
    pub proposed_by: ExecutionIdentity,
    pub parent: Option<ObjectRef>,
    pub rule: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RulebookDecision {
    pub proposal: ObjectRef,
    pub approve: bool,
}

/// Outer Some means preparation already ran, even with no approved revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrozenRulebook {
    pub revision: Option<ObjectRef>,
}

pub(crate) fn valid_text(text: &str, limit: usize) -> bool {
    !text.trim().is_empty() && text.len() <= limit
}
