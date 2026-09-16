//! Typed incidents, their declared policy class, and the recoveries a policy
//! class allows (IN1 §2).
//!
//! An *incident* is a failure that carries a stable code, a subject, the
//! evidence that produced it, and a typed recovery. It is deliberately much
//! narrower than "everything that went wrong": a failure with no
//! [`IncidentCode`] is not an incident and stays an ordinary error line
//! (IN1 §1 item 6, §2.3c rule 28).
//!
//! **Core owns the code and the policy class; core does not own the code's
//! trigger** (IN1 §2.1 rule 2). Nothing here performs I/O, spawns a thread,
//! reads the filesystem, or calls the colour classifier on a description it
//! did not receive as an argument. The single clock is
//! [`IncidentLog::started`], justified in IN1 §2.3 rule 17: `Instant` is not
//! serialisable and `SystemTime` is not monotonic, so a `Duration` since a
//! named session origin is the only honest session-scoped stamp.

use std::{
    collections::BTreeSet,
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize, Serializer};

use crate::{
    AssetId, COLOR_CONFIDENCE_MAX_BASIS_POINTS, ColorBitDepth, ColorDescription, ColorMatrix,
    ColorPrimaries, ColorProvenance, ColorRange, ColorSourceError, ColorSourceProfileAssumption,
    ColorTransfer, ColorWhitePoint, MediaError, Operation, TimelineRevision,
};

/// A source-colour failure that Kinewright classifies as an incident.
///
/// Mirrors [`ColorSourceError`] one-to-one **except**
/// [`ColorSourceError::UnknownWhitePoint`], which is not an incident:
/// `color_description_from_decoder` never sets a white point, so every
/// correctly BT.709-tagged source in existence fails the bare classifier with
/// that code and passes only through the decoder's explicit D65 assumption
/// (IN1 §2.2 rule 6). Twelve incident codes from thirteen classifier variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SourceColorIncident {
    /// The source declares no colour primaries.
    UnknownPrimaries,
    /// The source declares no transfer characteristic.
    UnknownTransfer,
    /// The source declares no matrix coefficients.
    UnknownMatrix,
    /// The source declares no sample range.
    UnknownRange,
    /// The source declares no sample bit depth.
    UnknownBitDepth,
    /// The declared colour primaries are outside the managed CC1 set.
    UnsupportedPrimaries,
    /// The declared transfer characteristic is outside the managed CC1 set.
    UnsupportedTransfer,
    /// The declared matrix coefficients are outside the managed CC1 set.
    UnsupportedMatrix,
    /// The declared sample range is outside the managed CC1 set.
    UnsupportedRange,
    /// The declared white point is outside the managed CC1 set.
    UnsupportedWhitePoint,
    /// The declared bit depth is outside the managed CC1 set.
    UnsupportedBitDepth,
    /// Every field is individually supported and the tuple still names no
    /// managed CC1 profile.
    UnsupportedCombination,
}

impl SourceColorIncident {
    /// The stable machine-readable code, delegated to [`ColorSourceError`] so
    /// the string is owned in exactly one place (IN1 §2.2 rule 7).
    ///
    /// Not `const`, only because [`ColorSourceError`]'s payloads carry a
    /// destructor and a `const fn` may not drop a temporary.
    #[must_use]
    pub fn code(self) -> &'static str {
        self.source_error().code()
    }

    /// The stable source-description field, delegated to [`ColorSourceError`]
    /// for the same reason [`Self::code`] is (IN1 §2.2 rule 7).
    ///
    /// Not `const`, for the same reason [`Self::code`] is not.
    #[must_use]
    pub fn field(self) -> &'static str {
        self.source_error().field()
    }

    /// The incident for a classifier failure, or `None` when the failure is
    /// not an incident in this slice.
    ///
    /// [`ColorSourceError::UnknownWhitePoint`] is the only `None` (IN1 §2.2
    /// rule 6).
    #[must_use]
    pub fn from_source_error(error: &ColorSourceError) -> Option<Self> {
        match error {
            ColorSourceError::UnknownPrimaries => Some(Self::UnknownPrimaries),
            ColorSourceError::UnknownTransfer => Some(Self::UnknownTransfer),
            ColorSourceError::UnknownMatrix => Some(Self::UnknownMatrix),
            ColorSourceError::UnknownRange => Some(Self::UnknownRange),
            ColorSourceError::UnknownBitDepth => Some(Self::UnknownBitDepth),
            ColorSourceError::UnsupportedPrimaries(_) => Some(Self::UnsupportedPrimaries),
            ColorSourceError::UnsupportedTransfer(_) => Some(Self::UnsupportedTransfer),
            ColorSourceError::UnsupportedMatrix(_) => Some(Self::UnsupportedMatrix),
            ColorSourceError::UnsupportedRange(_) => Some(Self::UnsupportedRange),
            ColorSourceError::UnsupportedWhitePoint(_) => Some(Self::UnsupportedWhitePoint),
            ColorSourceError::UnsupportedBitDepth(_) => Some(Self::UnsupportedBitDepth),
            ColorSourceError::UnsupportedCombination { .. } => Some(Self::UnsupportedCombination),
            ColorSourceError::UnknownWhitePoint => None,
        }
    }

    /// A canonical classifier failure for this incident, used only to reach
    /// [`ColorSourceError`]'s `const` accessors. The payloads are placeholders
    /// and never leave this module: `code()` and `field()` ignore them.
    ///
    /// Do **not** delegate `observed()` through this. `observed()` renders the
    /// payload, so an incident asked for its observed value this way would
    /// answer with the placeholder (`"unknown"`) rather than with the tag the
    /// probe actually carried. The observed value reaches an incident from the
    /// live [`ColorSourceError`] on [`IncidentObservation`], never from here.
    fn source_error(self) -> ColorSourceError {
        match self {
            Self::UnknownPrimaries => ColorSourceError::UnknownPrimaries,
            Self::UnknownTransfer => ColorSourceError::UnknownTransfer,
            Self::UnknownMatrix => ColorSourceError::UnknownMatrix,
            Self::UnknownRange => ColorSourceError::UnknownRange,
            Self::UnknownBitDepth => ColorSourceError::UnknownBitDepth,
            Self::UnsupportedPrimaries => {
                ColorSourceError::UnsupportedPrimaries(ColorPrimaries::Unknown)
            }
            Self::UnsupportedTransfer => {
                ColorSourceError::UnsupportedTransfer(ColorTransfer::Unknown)
            }
            Self::UnsupportedMatrix => ColorSourceError::UnsupportedMatrix(ColorMatrix::Unknown),
            Self::UnsupportedRange => ColorSourceError::UnsupportedRange(ColorRange::Unknown),
            Self::UnsupportedWhitePoint => {
                ColorSourceError::UnsupportedWhitePoint(ColorWhitePoint::Unknown)
            }
            Self::UnsupportedBitDepth => {
                ColorSourceError::UnsupportedBitDepth(ColorBitDepth::Unknown)
            }
            Self::UnsupportedCombination => ColorSourceError::UnsupportedCombination {
                primaries: ColorPrimaries::Unknown,
                transfer: ColorTransfer::Unknown,
                matrix: ColorMatrix::Unknown,
                range: ColorRange::Unknown,
            },
        }
    }

    /// This incident's row index in [`POLICY`], in IN1 §2.2's table order.
    ///
    /// Total by construction, so [`policy_class`] needs no fallible lookup and
    /// no panic. `policy_rows_are_declared_in_table_order` pins the
    /// correspondence, and §2.4 rule 35's exhaustive match pins the coverage.
    const fn table_index(self) -> usize {
        match self {
            Self::UnknownPrimaries => 0,
            Self::UnknownTransfer => 1,
            Self::UnknownMatrix => 2,
            Self::UnknownRange => 3,
            Self::UnknownBitDepth => 4,
            Self::UnsupportedPrimaries => 5,
            Self::UnsupportedTransfer => 6,
            Self::UnsupportedMatrix => 7,
            Self::UnsupportedRange => 8,
            Self::UnsupportedWhitePoint => 9,
            Self::UnsupportedBitDepth => 10,
            Self::UnsupportedCombination => 11,
        }
    }
}

/// The exhaustive set of incident codes Kinewright declares.
///
/// Part A declares source colour and nothing else: there is no catch-all, no
/// `Unclassified`, and no `Other(String)` (IN1 §2.2 rule 9). It also carries no
/// `#[non_exhaustive]`, because that would force a wildcard arm into every
/// downstream `match` and make IN1 §2.4 rule 35's exhaustiveness test — the one
/// thing that proves [`POLICY`] covers every code — pass vacuously
/// (IN1 §2.2 rule 10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum IncidentCode {
    /// A CC1 source-colour classification failure.
    SourceColor(SourceColorIncident),
}

impl IncidentCode {
    /// The stable machine-readable code, delegated to the inner incident.
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Self::SourceColor(incident) => incident.code(),
        }
    }

    /// The stable source-description field, delegated to the inner incident.
    #[must_use]
    pub fn field(self) -> &'static str {
        match self {
            Self::SourceColor(incident) => incident.field(),
        }
    }

    /// A canonical classifier failure for this code, delegated to the inner
    /// incident. Private, and subject to
    /// [`SourceColorIncident::source_error`]'s caveat: the payload is a
    /// placeholder, so this reaches `const` accessors that ignore it and never
    /// `observed()`.
    fn source_error(self) -> ColorSourceError {
        match self {
            Self::SourceColor(incident) => incident.source_error(),
        }
    }
}

/// Emits the stable `code()` string and nothing else, so the wire value is the
/// identifier every other agent surface already publishes and never a nested
/// map (IN1 §2.2 rule 5).
impl Serialize for IncidentCode {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.code())
    }
}

// There is deliberately no `Deserialize` for `IncidentCode`. Every type that
// carries one — `Incident`, `IncidentEvidence`, `IncidentObservation`,
// `RecoveryAction` — is serialise-only, and IN1 §2.3c rule 30 says an incident
// is not persisted, so no wire value ever becomes an `IncidentCode`. Adding one
// would widen §14 row A's pub surface past what the contract declares; if a
// later stage needs it, it should arrive by erratum rather than by silence.

/// A session-unique incident identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct IncidentId(pub u64);

impl std::fmt::Display for IncidentId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

/// How badly the incident affects the person's work.
///
/// Severity is data, not judgement: [`POLICY`]'s `severity` column is the
/// single source (IN1 §2.3b rule 26). Every Part A row is
/// [`IncidentSeverity::Blocks`], because a source-colour refusal stops the
/// managed decode and no frame appears; `Degrades` and `Informs` are declared
/// for the later migration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IncidentSeverity {
    /// Nothing the person asked for can happen until it is resolved.
    Blocks,
    /// The work continues with a reduced result.
    Degrades,
    /// The work is unaffected; the person should know anyway.
    Informs,
}

/// What the incident is about.
///
/// Part A declares only [`IncidentSubject::Asset`]; `Clip`, `Track`,
/// `ExportJob`, `Project` and `Agent` arrive with Part B's sources
/// (IN1 §2.3 rule 14).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IncidentSubject {
    /// One media asset in the project's pool.
    Asset(AssetId),
}

impl IncidentSubject {
    /// A short human label for a card or a log line, for example `Asset 1`.
    ///
    /// The asset's human *name* is deliberately not on the incident: an
    /// incident carries no document view (IN1 §2.3c rule 31, §13 D15).
    #[must_use]
    pub fn label(self) -> String {
        match self {
            Self::Asset(asset) => format!("Asset {asset}"),
        }
    }
}

/// The typed facts the incident was opened from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IncidentEvidence {
    /// A CC1 source-colour refusal on the managed decode path.
    SourceColor {
        /// The description the incident captured at open time. The revert
        /// restores exactly these bytes (IN1 §0.1 N1.5/B4).
        probed: ColorDescription,
        /// The explicit profile assumption in force when the decode was
        /// refused, if there was one.
        assumption: Option<ColorSourceProfileAssumption>,
    },
}

impl IncidentEvidence {
    /// The probed description this evidence captured.
    #[must_use]
    pub const fn probed(&self) -> &ColorDescription {
        match self {
            Self::SourceColor { probed, .. } => probed,
        }
    }
}

/// How an incident ended.
///
/// `Approved` and `Rejected` are not IN1 outcomes; they arrive with the typed
/// broker (IN1 §6.3 rule 17, §13 D6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IncidentOutcome {
    /// The recovery was applied.
    Applied,
    /// An applied recovery was taken back.
    Reverted,
    /// Nothing was applied; the person was told what the problem is.
    Explained,
}

/// Whether the incident is still outstanding.
///
/// Adjacently tagged so it serialises as a map and can therefore be
/// `#[serde(flatten)]`ed into [`Incident`], rendering as the two top-level keys
/// `"state":"resolved","outcome":"applied"` — and as the single key
/// `"state":"open"` while the incident is open (IN1 §2.5 rule 43).
///
/// `Investigating` is not declared in Part A: nothing constructs it while
/// there is no session (IN1 §2.3 rule 13, §13 D5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "state", content = "outcome")]
pub enum IncidentState {
    /// Outstanding.
    Open,
    /// Ended, with the outcome that ended it.
    Resolved(IncidentOutcome),
}

/// What resolving an incident cost.
///
/// Every `Option` field skips when `None`, so an unresolved incident costs one
/// wire key rather than eight keys of `null` (IN1 §8 rule 4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct IncidentTelemetry {
    /// Wall time from observation to resolution, measured at the router.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved_after: Option<Duration>,
    /// Tool calls the resolver made, when an agent resolved it.
    pub tool_calls: u32,
    /// Provider input tokens reported by the resolving session, when one
    /// reported them.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    /// Provider cached input tokens, when reported.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cached_input_tokens: Option<u64>,
    /// Provider cache-creation input tokens, when reported.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_creation_input_tokens: Option<u64>,
    /// Provider output tokens, when reported.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    /// Provider reasoning output tokens, when reported.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_output_tokens: Option<u64>,
    /// Session cost in millionths of a dollar, when reported. Core carries no
    /// `f64` in a stored record, and a dollar figure is a currency amount
    /// rather than a measurement (IN1 §8 rule 5).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_usd_millionths: Option<i64>,
}

/// One classified problem, with its evidence and its typed recoveries.
///
/// There is no `message` field. It used to be
/// [`ColorSourceError::actionable_message`], which is literally
/// `display + " (field=…, observed=…, allowed=…). " + recovery_action()`, so
/// `observed` and `allowed` travelled twice and the recovery three times. It is
/// reconstructible from `code`, `field`, `observed` and `allowed`, all of which
/// are on the record, and it is removed from the *type* rather than hidden with
/// `#[serde(skip)]`: a field nobody reads is a field that drifts
/// (IN1 §2.3 rule 12).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Incident {
    /// Session-unique identifier.
    pub id: IncidentId,
    /// Serialises as the stable `code()` string (IN1 §2.2 rule 5).
    pub code: IncidentCode,
    /// Resolved through [`policy_class`] at `observe` time and stored, so an
    /// investigator decides from one read.
    pub class: PolicyClass,
    /// From [`POLICY`]'s `severity` column.
    pub severity: IncidentSeverity,
    /// What the incident is about.
    pub subject: IncidentSubject,
    /// Always `code.field()`; assigned at exactly one site.
    pub field: &'static str,
    /// The offending value, from a core accessor.
    pub observed: String,
    /// The values that would have been accepted, from a core accessor.
    pub allowed: Option<String>,
    /// The typed facts the incident was opened from.
    pub evidence: IncidentEvidence,
    /// Everything [`policy_recovery`] allows for this code and evidence.
    pub recoveries: Vec<RecoveryAction>,
    /// The timeline revision the problem was last observed at.
    pub revision: TimelineRevision,
    /// When the problem was **first** seen, measured from
    /// [`IncidentLog::with_start`]'s origin.
    ///
    /// Session-relative and never serialised: a wire value nothing may assert
    /// is a wire value nothing should be charged for (IN1 §2.3 rule 11).
    #[serde(skip)]
    pub opened_at: Duration,
    /// How many times the problem has been observed.
    pub count: u32,
    /// Whether the incident is still outstanding.
    #[serde(flatten)]
    pub state: IncidentState,
    /// What resolving it cost.
    pub telemetry: IncidentTelemetry,
}

/// The only way an incident is created.
///
/// Nothing outside this module constructs an [`Incident`]; an observation
/// carries everything [`IncidentLog`] needs and nothing it does not
/// (IN1 §2.3b rule 21).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncidentObservation {
    /// The stable code the failure classified as.
    pub code: IncidentCode,
    /// What the failure was about.
    pub subject: IncidentSubject,
    /// The offending value, from [`ColorSourceError::observed`].
    pub observed: String,
    /// The accepted values, from [`ColorSourceError::allowed_values`].
    pub allowed: Option<String>,
    /// The typed facts the failure carried.
    pub evidence: IncidentEvidence,
    /// The timeline revision the failure was observed at.
    pub revision: TimelineRevision,
}

impl IncidentObservation {
    /// Build an observation from a typed media failure, or `None` when the
    /// failure has no incident code in this slice.
    ///
    /// The match carries **no** wildcard arm, so a new [`MediaError`] variant
    /// breaks the build and forces a decision rather than silently becoming a
    /// non-incident (IN1 §2.3b rule 22).
    #[must_use]
    pub fn from_media_error(error: &MediaError, revision: TimelineRevision) -> Option<Self> {
        match error {
            MediaError::SourceColorForAsset(refusal) => {
                let incident = SourceColorIncident::from_source_error(&refusal.error)?;
                Some(Self {
                    code: IncidentCode::SourceColor(incident),
                    subject: IncidentSubject::Asset(refusal.asset),
                    observed: refusal.error.observed(),
                    allowed: Some(refusal.error.allowed_values().to_owned()),
                    evidence: IncidentEvidence::SourceColor {
                        probed: refusal.description.clone(),
                        assumption: refusal.assumption,
                    },
                    revision,
                })
            }
            // An incident needs a subject, and the layer that supplies one is
            // `contextual_managed_decode_error`, which turns every
            // `SourceColor` into a `SourceColorForAsset` before it leaves
            // `kinewright-media`. The arm exists so the exhaustive match is
            // honest, not because the path is reachable (IN1 §2.3b rule 23).
            MediaError::SourceColor(_)
            | MediaError::NotImplemented
            | MediaError::Cancelled
            | MediaError::UnsupportedDecoderFormat { .. }
            | MediaError::DeliveryColor(_)
            | MediaError::DeliveryVerification(_)
            | MediaError::ColorQc(_)
            | MediaError::MixSpectrumRangeTooShort { .. }
            | MediaError::MixLoudnessRangeTooShort { .. }
            | MediaError::Backend(_) => None,
        }
    }
}

/// What [`IncidentLog::observe`] did with an observation.
///
/// Three states rather than `Option<IncidentId>`, because the router must do
/// different things for a newly opened incident (write one audit line, consider
/// auto-apply), a dedup hit (do neither) and a suppressed observation (do
/// nothing at all) (IN1 §2.3 rule 20).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Observed {
    /// The `(code, subject)` pair has already been auto-applied or resolved
    /// this session. No entry was created or touched.
    Suppressed,
    /// An open incident with the same dedup key absorbed the observation.
    Deduped(IncidentId),
    /// A new incident was opened.
    Opened(IncidentId),
}

/// One thing a person or an agent may do about an incident.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RecoveryAction {
    /// Button text. `&'static str` so a recovery label cannot embed a path, a
    /// file name or a measured number and therefore cannot drift per fixture.
    pub label: &'static str,
    /// What pressing it does.
    pub kind: RecoveryKind,
}

/// The two shapes a recovery takes in IN1.
///
/// **Externally tagged** — serde's default — so a recovery renders as
/// `{"label":"…","kind":{"operation":{…}}}`. An internally tagged enum with a
/// `&'static str` newtype variant compiles and then fails at run time with
/// *"cannot serialize tagged newtype variant"*, and nine of the twelve Part A
/// codes are `Explain` rows. External tagging is also the smaller of the two
/// workable forms, at 14 B of wrapper against adjacent tagging's 29 B
/// (IN1 §2.5 rule 42).
///
/// No `Capability`, `Relink` or `Transcode` variant is declared: nothing would
/// construct them and IN1 forbids executing them (IN1 §2.5 rule 44).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
// `Operation` is a large enum and is `#[allow]`ed at its own declaration for
// the same reason: boxing it here would change the operation's wire shape.
#[allow(clippy::large_enum_variant)]
pub enum RecoveryKind {
    /// An exact operation the actor may send through `Command::Do*`.
    Operation(Operation),
    /// Plain language and nothing to press. The text is a core `&'static str`,
    /// never a sentence the app composed.
    Explain(&'static str),
}

/// What Kinewright is allowed to do about an incident without being asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyClass {
    /// Apply the recovery, then say so.
    AutoApply,
    /// Propose the recovery and wait for a decision.
    ///
    /// Declared with **no** Part A row: IN1 §2.5's [`RecoveryAction`] contract
    /// and IN1 §9 clause 6's assertion are written over both classes, and the
    /// migration will use it (IN1 §2.4 rule 33).
    AskFirst,
    /// Apply nothing and say what the problem is.
    Explain,
}

/// The condition under which a row's declared class survives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyPredicate {
    /// The declared class always holds.
    Always,
    /// The declared class holds only when [`rec709_compatible`] is true of the
    /// probed description; otherwise the row falls to [`PolicyClass::Explain`].
    Rec709Compatible,
}

/// One row of [`POLICY`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PolicyEntry {
    /// The code this row governs.
    pub code: IncidentCode,
    /// The class the row declares.
    pub class: PolicyClass,
    /// The condition under which the declared class survives.
    pub predicate: PolicyPredicate,
    /// How badly the incident affects the person's work.
    pub severity: IncidentSeverity,
}

/// The declared policy for every [`IncidentCode`], one row per code, in
/// IN1 §2.2's table order.
pub const POLICY: [PolicyEntry; 12] = [
    PolicyEntry {
        code: IncidentCode::SourceColor(SourceColorIncident::UnknownPrimaries),
        class: PolicyClass::AutoApply,
        predicate: PolicyPredicate::Rec709Compatible,
        severity: IncidentSeverity::Blocks,
    },
    PolicyEntry {
        code: IncidentCode::SourceColor(SourceColorIncident::UnknownTransfer),
        class: PolicyClass::AutoApply,
        predicate: PolicyPredicate::Rec709Compatible,
        severity: IncidentSeverity::Blocks,
    },
    PolicyEntry {
        code: IncidentCode::SourceColor(SourceColorIncident::UnknownMatrix),
        class: PolicyClass::AutoApply,
        predicate: PolicyPredicate::Rec709Compatible,
        severity: IncidentSeverity::Blocks,
    },
    PolicyEntry {
        code: IncidentCode::SourceColor(SourceColorIncident::UnknownRange),
        class: PolicyClass::Explain,
        predicate: PolicyPredicate::Always,
        severity: IncidentSeverity::Blocks,
    },
    PolicyEntry {
        code: IncidentCode::SourceColor(SourceColorIncident::UnknownBitDepth),
        class: PolicyClass::Explain,
        predicate: PolicyPredicate::Always,
        severity: IncidentSeverity::Blocks,
    },
    PolicyEntry {
        code: IncidentCode::SourceColor(SourceColorIncident::UnsupportedPrimaries),
        class: PolicyClass::Explain,
        predicate: PolicyPredicate::Always,
        severity: IncidentSeverity::Blocks,
    },
    PolicyEntry {
        code: IncidentCode::SourceColor(SourceColorIncident::UnsupportedTransfer),
        class: PolicyClass::Explain,
        predicate: PolicyPredicate::Always,
        severity: IncidentSeverity::Blocks,
    },
    PolicyEntry {
        code: IncidentCode::SourceColor(SourceColorIncident::UnsupportedMatrix),
        class: PolicyClass::Explain,
        predicate: PolicyPredicate::Always,
        severity: IncidentSeverity::Blocks,
    },
    PolicyEntry {
        code: IncidentCode::SourceColor(SourceColorIncident::UnsupportedRange),
        class: PolicyClass::Explain,
        predicate: PolicyPredicate::Always,
        severity: IncidentSeverity::Blocks,
    },
    PolicyEntry {
        code: IncidentCode::SourceColor(SourceColorIncident::UnsupportedWhitePoint),
        class: PolicyClass::Explain,
        predicate: PolicyPredicate::Always,
        severity: IncidentSeverity::Blocks,
    },
    PolicyEntry {
        code: IncidentCode::SourceColor(SourceColorIncident::UnsupportedBitDepth),
        class: PolicyClass::Explain,
        predicate: PolicyPredicate::Always,
        severity: IncidentSeverity::Blocks,
    },
    PolicyEntry {
        code: IncidentCode::SourceColor(SourceColorIncident::UnsupportedCombination),
        class: PolicyClass::Explain,
        predicate: PolicyPredicate::Always,
        severity: IncidentSeverity::Blocks,
    },
];

/// The [`POLICY`] row for one code. Total by construction; see
/// [`SourceColorIncident::table_index`].
const fn policy_entry(code: IncidentCode) -> PolicyEntry {
    match code {
        IncidentCode::SourceColor(incident) => POLICY[incident.table_index()],
    }
}

/// The class a code resolves to for one probed description.
///
/// [`PolicyPredicate::Always`] returns the declared class;
/// [`PolicyPredicate::Rec709Compatible`] returns the declared class when
/// [`rec709_compatible`] is true and [`PolicyClass::Explain`] otherwise
/// (IN1 §2.4 rule 36).
#[must_use]
pub fn policy_class(code: IncidentCode, probed: &ColorDescription) -> PolicyClass {
    let entry = policy_entry(code);
    match entry.predicate {
        PolicyPredicate::Always => entry.class,
        PolicyPredicate::Rec709Compatible => {
            if rec709_compatible(probed) {
                entry.class
            } else {
                PolicyClass::Explain
            }
        }
    }
}

/// The severity [`POLICY`] declares for one code. Private because IN1 §14's
/// re-export list names no such function: a reader outside core reads
/// [`POLICY`] itself.
const fn policy_severity(code: IncidentCode) -> IncidentSeverity {
    policy_entry(code).severity
}

/// The **single** producer of recoveries.
///
/// The router, the card and `get_incidents` all read it; none of them may build
/// a [`RecoveryAction`] itself, which is what makes IN1 §9 clause 6's equality
/// assertion meaningful (IN1 §2.5 rule 45).
#[must_use]
pub fn policy_recovery(
    code: IncidentCode,
    subject: IncidentSubject,
    evidence: &IncidentEvidence,
) -> Vec<RecoveryAction> {
    let probed = evidence.probed();
    match policy_class(code, probed) {
        PolicyClass::AutoApply | PolicyClass::AskFirst => {
            let IncidentSubject::Asset(asset) = subject;
            vec![RecoveryAction {
                label: ASSUME_REC709_LABEL,
                kind: RecoveryKind::Operation(assume_rec709_operation(asset, probed)),
            }]
        }
        // The sentence is `ColorSourceError::recovery_action()`'s and is never
        // restated here (IN1 §2.5 rule 46). It is reached through *this* code's
        // own canonical failure rather than through a fixed variant, so when
        // §13 D8 gives `recovery_action` per-code bodies in IN3 each row keeps
        // answering for itself instead of silently inheriting the primaries
        // sentence. Rule 47's observation that the accessor is a `const fn`
        // with no `match` today is what makes the two spellings agree now.
        PolicyClass::Explain => vec![RecoveryAction {
            label: EXPLAIN_LABEL,
            kind: RecoveryKind::Explain(code.source_error().recovery_action()),
        }],
    }
}

/// The button text of the one recovery IN1 applies.
const ASSUME_REC709_LABEL: &str = "Assume Rec.709 for this source";

/// The button text of a recovery that explains rather than applies.
const EXPLAIN_LABEL: &str = "How to fix this";

/// The description the Rec.709 recovery writes.
///
/// Fills only `Unknown` fields except for the three tags and the white point,
/// which the shipped Media-panel builder has always written unconditionally.
/// IN1 §2.4 rule 37's definition of [`rec709_compatible`] is written against
/// this function, so the predicate and the builder cannot diverge
/// (IN1 §4.5 rule 32).
#[must_use]
pub fn recovery_description(probed: &ColorDescription) -> ColorDescription {
    let range = match &probed.range {
        ColorRange::Unknown => ColorRange::Limited,
        known => known.clone(),
    };
    let bit_depth = match &probed.bit_depth {
        ColorBitDepth::Unknown => ColorBitDepth::Eight,
        known => known.clone(),
    };
    ColorDescription {
        primaries: ColorPrimaries::Bt709,
        transfer: ColorTransfer::Bt709,
        matrix: ColorMatrix::Bt709,
        range,
        white_point: ColorWhitePoint::D65,
        bit_depth,
        confidence_basis_points: COLOR_CONFIDENCE_MAX_BASIS_POINTS,
        provenance: ColorProvenance::AgentAssumption,
    }
}

/// The operation that applies [`recovery_description`] to one asset.
#[must_use]
pub fn assume_rec709_operation(asset: AssetId, probed: &ColorDescription) -> Operation {
    Operation::SetAssetColorDescription {
        asset,
        color_description: recovery_description(probed),
    }
}

/// Whether the Rec.709 recovery is honest for this probed description.
///
/// **Normative definition** (IN1 §2.4 rule 37). `rec709_compatible(probed)` is
/// true exactly when
///
/// - **(a)** [`recovery_description`] leaves every **known** field of `probed`
///   — primaries, transfer, matrix, range, white point, bit depth — unchanged,
///   filling only the `Unknown` ones, **and**
/// - **(b)** `classify_source_with_assumption(&recovery_description(probed),
///   None) == Ok(ColorSourceProfile::Rec709Video)`.
///
/// The explicit chain below is the implementation, and
/// `rec709_compatible_definition_equals_its_implementation` asserts the two
/// agree over a constructed cross product of every named tag variant.
///
/// Why a *known* non-Rec.709 value falls to [`PolicyClass::Explain`] differs by
/// field, and the reason is worth keeping beside the code (IN1 §2.4 rule 38).
/// Primaries, transfer, matrix and white point are refused when known and not
/// Rec.709 because the recovery would have to **overwrite** them, which is the
/// silent behaviour CC1 §1 forbids. Range and bit depth are refused only when
/// the recovery cannot make them classify: the builder *preserves* a known
/// range and a known depth, and `rec709_video` admits `limited` or `full` with
/// integer depth `8..=16`, so a known `Full` range and a known `Ten` depth
/// survive untouched and are admitted. `ColorPrimaries::Srgb` is refused
/// deliberately, because the only recovery IN1 ships targets `rec709_video`.
/// `Other(_)` in any tag is matched by no arm and therefore yields `false`.
#[must_use]
pub fn rec709_compatible(probed: &ColorDescription) -> bool {
    matches!(
        probed.primaries,
        ColorPrimaries::Unknown | ColorPrimaries::Bt709
    ) && matches!(
        probed.transfer,
        ColorTransfer::Unknown | ColorTransfer::Bt709
    ) && matches!(probed.matrix, ColorMatrix::Unknown | ColorMatrix::Bt709)
        && matches!(
            probed.range,
            ColorRange::Unknown | ColorRange::Limited | ColorRange::Full
        )
        && matches!(
            probed.white_point,
            ColorWhitePoint::Unknown | ColorWhitePoint::D65
        )
        && match &probed.bit_depth {
            ColorBitDepth::Unknown => true,
            value => value
                .integer_bits()
                .is_some_and(|bits| (8..=16).contains(&bits)),
        }
}

/// The session's incidents, their dedup state and their suppression set.
///
/// Session state: it is never persisted, and the only durable residue of a
/// resolved incident is the asset's written `color_description` and
/// `MediaAsset::assumed_from` (IN1 §1 item 3, §2.3c rule 30).
#[derive(Debug)]
pub struct IncidentLog {
    started: Instant,
    next_id: u64,
    entries: Vec<Incident>,
    suppressed: BTreeSet<(IncidentCode, IncidentSubject)>,
}

impl Default for IncidentLog {
    fn default() -> Self {
        Self::with_start(Instant::now())
    }
}

impl IncidentLog {
    /// A log whose session origin is pinned, so a test can read `opened_at`
    /// deterministically (IN1 §2.3 rule 18).
    #[must_use]
    pub fn with_start(started: Instant) -> Self {
        Self {
            started,
            next_id: 1,
            entries: Vec::new(),
            suppressed: BTreeSet::new(),
        }
    }

    /// Record one observation.
    ///
    /// The dedup key is `(code, subject, observed)`, in that order, and nothing
    /// else. Descriptions are deliberately **not** compared: an untagged MP4
    /// and an untagged `WebM` differ in `range`, `confidence_basis_points` and
    /// `provenance` while sharing code, subject shape and observed, so a
    /// description-keyed dedup would split one problem into two incidents
    /// (IN1 §2.3 rule 15).
    ///
    /// A `Resolved` entry never absorbs an observation; the re-open that
    /// permits is closed by the suppression set, not by weakening the rule
    /// (IN1 §2.3 rule 16). The `state == Open` filter below is therefore
    /// belt-and-braces: rule 19's suppression already makes a resolved entry
    /// unreachable, because both [`Self::resolve`] and
    /// [`Self::note_auto_applied`] insert `(code, subject)` into `suppressed`
    /// and the check above returns first. It is kept so that the rule is
    /// enforced by this function rather than by a distant invariant.
    pub fn observe(&mut self, observation: IncidentObservation) -> Observed {
        if self
            .suppressed
            .contains(&(observation.code, observation.subject))
        {
            return Observed::Suppressed;
        }
        if let Some(existing) = self.entries.iter_mut().find(|incident| {
            incident.state == IncidentState::Open
                && incident.code == observation.code
                && incident.subject == observation.subject
                && incident.observed == observation.observed
        }) {
            existing.count = existing.count.saturating_add(1);
            existing.revision = observation.revision;
            return Observed::Deduped(existing.id);
        }

        let id = IncidentId(self.next_id);
        self.next_id = self.next_id.saturating_add(1);
        let class = policy_class(observation.code, observation.evidence.probed());
        let recoveries =
            policy_recovery(observation.code, observation.subject, &observation.evidence);
        self.entries.push(Incident {
            id,
            code: observation.code,
            class,
            severity: policy_severity(observation.code),
            subject: observation.subject,
            field: observation.code.field(),
            observed: observation.observed,
            allowed: observation.allowed,
            evidence: observation.evidence,
            recoveries,
            revision: observation.revision,
            opened_at: self.started.elapsed(),
            count: 1,
            state: IncidentState::Open,
            telemetry: IncidentTelemetry::default(),
        });
        Observed::Opened(id)
    }

    /// Every incident that is still outstanding.
    pub fn open(&self) -> impl Iterator<Item = &Incident> {
        self.entries
            .iter()
            .filter(|incident| incident.state == IncidentState::Open)
    }

    /// Every incident this session opened, resolved or not.
    pub fn all(&self) -> impl Iterator<Item = &Incident> {
        self.entries.iter()
    }

    /// One incident by id.
    #[must_use]
    pub fn get(&self, id: IncidentId) -> Option<&Incident> {
        self.entries.iter().find(|incident| incident.id == id)
    }

    /// The **single** write path for one incident's telemetry.
    ///
    /// The six `AgentEvent::Cost` mirrors and `tool_calls` are stamped here
    /// and nowhere else: by the app router when it accepts an auto-apply
    /// (IN1 §5.2 rule 12) and by the `resolve_incident` capability when it
    /// records an outcome (IN1 §6.3 rule 16). `resolved_after` is stamped by
    /// [`Self::resolve`], because only the log holds the session clock it is
    /// measured against. Everything else about an
    /// [`Incident`] is written by [`Self::observe`], [`Self::resolve`] and
    /// [`Self::refresh_revision`], so handing out one narrowly typed `&mut`
    /// keeps the record's identity, classification and state owned by this
    /// type.
    pub fn telemetry_mut(&mut self, id: IncidentId) -> Option<&mut IncidentTelemetry> {
        self.entries
            .iter_mut()
            .find(|incident| incident.id == id)
            .map(|incident| &mut incident.telemetry)
    }

    /// End an incident with an outcome, stamp `telemetry.resolved_after`
    /// from the session clock, and stop this session reporting its
    /// `(code, subject)` pair.
    ///
    /// Inserts into the suppression set for **every** outcome, `Applied`,
    /// `Reverted` and `Explained` alike: a session reports a given
    /// `(code, subject)` once, and resolving it by any route ends that
    /// session's reporting of it (IN1 §2.3 rule 19).
    pub fn resolve(&mut self, id: IncidentId, outcome: IncidentOutcome) -> bool {
        let Some(incident) = self.entries.iter_mut().find(|incident| incident.id == id) else {
            return false;
        };
        incident.state = IncidentState::Resolved(outcome);
        incident.telemetry.resolved_after = Some(self.started.elapsed());
        let key = (incident.code, incident.subject);
        self.suppressed.insert(key);
        true
    }

    /// Record that the router has **sent** an auto-apply for this incident,
    /// before any acceptance arrives.
    ///
    /// A session auto-applies at most once per `(code, subject)`; any later
    /// return of the same problem is reported, never re-fixed. This is what
    /// makes a global `Undo` stick and what closes the frame-boundary
    /// re-apply race (IN1 §2.3 rule 19).
    pub fn note_auto_applied(&mut self, id: IncidentId) -> bool {
        let Some(incident) = self.entries.iter().find(|incident| incident.id == id) else {
            return false;
        };
        let key = (incident.code, incident.subject);
        self.suppressed.insert(key);
        true
    }

    /// Point an open incident at the revision it is now stated against.
    ///
    /// The third narrowly typed writer, beside [`Self::telemetry_mut`] and
    /// [`Self::note_auto_applied`], and like them it writes exactly one thing:
    /// `revision`, and only on an `Open` entry. `count`, `suppressed`, `state`,
    /// `opened_at` and telemetry are untouched. Returns `false`, changing
    /// nothing, for an unknown id or a `Resolved` entry.
    ///
    /// It exists because [`Self::observe`] cannot do this job. IN1 §5.2 rule 10
    /// says a stale router auto-apply that core refuses on the revision leaves
    /// the incident `Open` with its `revision` refreshed and is not re-sent, and
    /// §9 clause 9 asserts that state. But by then the router has already called
    /// [`Self::note_auto_applied`], so the incident's `(code, subject)` pair is
    /// in the suppression set and rule 19's check at the top of `observe`
    /// returns [`Observed::Suppressed`] before the dedup arm that would have
    /// refreshed `revision` can run — by design, because that check is what
    /// makes a session auto-apply at most once. Refreshing therefore needs its
    /// own writer rather than a weakening of rule 19.
    pub fn refresh_revision(&mut self, id: IncidentId, revision: TimelineRevision) -> bool {
        let Some(incident) = self.entries.iter_mut().find(|incident| incident.id == id) else {
            return false;
        };
        if incident.state != IncidentState::Open {
            return false;
        }
        incident.revision = revision;
        true
    }

    /// How many incidents are still outstanding.
    ///
    /// A different quantity from the audit log's length: this counts *problems
    /// currently unresolved*, the audit log counts *lines ever written this
    /// session* (IN1 §5.2 rule 15).
    #[must_use]
    pub fn open_count(&self) -> usize {
        self.open().count()
    }

    /// How many incidents this session opened.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether this session opened no incident at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ColorSourceProfile, Document, MediaAsset, MediaKind, MediaSourceFingerprint, Rational,
        SourceColorRefusal, TimeCode, classify_source_with_assumption,
    };

    /// `in1_untagged.mp4`'s pinned probed tuple (IN1 §3 rule 5 row 1).
    fn untagged_mp4_probe() -> ColorDescription {
        ColorDescription {
            primaries: ColorPrimaries::Unknown,
            transfer: ColorTransfer::Unknown,
            matrix: ColorMatrix::Unknown,
            range: ColorRange::Unknown,
            white_point: ColorWhitePoint::Unknown,
            bit_depth: ColorBitDepth::Eight,
            confidence_basis_points: 2_000,
            provenance: ColorProvenance::Inferred,
        }
    }

    /// `in1_untagged_vp9.webm`'s pinned probed tuple (IN1 §3 rule 5 row 3): the
    /// Matroska/WebM muxer always writes a `Colour/Range` element, so the
    /// untagged `WebM` differs from the untagged MP4 in three of eight fields
    /// while sharing the code, the subject shape and the observed value.
    fn untagged_webm_probe() -> ColorDescription {
        ColorDescription {
            primaries: ColorPrimaries::Unknown,
            transfer: ColorTransfer::Unknown,
            matrix: ColorMatrix::Unknown,
            range: ColorRange::Limited,
            white_point: ColorWhitePoint::Unknown,
            bit_depth: ColorBitDepth::Eight,
            confidence_basis_points: 4_000,
            provenance: ColorProvenance::StreamMetadata,
        }
    }

    fn observation_for(error: &ColorSourceError, probed: &ColorDescription) -> IncidentObservation {
        let incident =
            SourceColorIncident::from_source_error(error).expect("the fixture code is an incident");
        IncidentObservation {
            code: IncidentCode::SourceColor(incident),
            subject: IncidentSubject::Asset(AssetId(1)),
            observed: error.observed(),
            allowed: Some(error.allowed_values().to_owned()),
            evidence: IncidentEvidence::SourceColor {
                probed: probed.clone(),
                assumption: None,
            },
            revision: TimelineRevision(1),
        }
    }

    fn unknown_primaries_observation(probed: &ColorDescription) -> IncidentObservation {
        observation_for(&ColorSourceError::UnknownPrimaries, probed)
    }

    fn document_with_probed_asset(probed: &ColorDescription) -> Document {
        Document {
            media_pool: vec![MediaAsset {
                id: AssetId(1),
                path: "in1_untagged.mp4".into(),
                name: "in1_untagged".to_owned(),
                duration: TimeCode(50),
                fps: Rational::new(25, 1).unwrap(),
                kind: MediaKind::Video,
                resolution: Some((320, 180)),
                source_fingerprint: MediaSourceFingerprint::default(),
                color_description: probed.clone(),
                assumed_from: None,
            }],
            ..Document::default()
        }
    }

    /// Every classifier variant, so a mapping test cannot quietly skip one.
    fn every_source_error() -> Vec<ColorSourceError> {
        vec![
            ColorSourceError::UnknownPrimaries,
            ColorSourceError::UnsupportedPrimaries(ColorPrimaries::Bt2020),
            ColorSourceError::UnknownTransfer,
            ColorSourceError::UnsupportedTransfer(ColorTransfer::Smpte2084),
            ColorSourceError::UnknownMatrix,
            ColorSourceError::UnsupportedMatrix(ColorMatrix::Bt2020Ncl),
            ColorSourceError::UnknownRange,
            ColorSourceError::UnsupportedRange(ColorRange::Other("weird".to_owned())),
            ColorSourceError::UnknownWhitePoint,
            ColorSourceError::UnsupportedWhitePoint(ColorWhitePoint::D50),
            ColorSourceError::UnknownBitDepth,
            ColorSourceError::UnsupportedBitDepth(ColorBitDepth::Float32),
            ColorSourceError::UnsupportedCombination {
                primaries: ColorPrimaries::Bt709,
                transfer: ColorTransfer::Srgb,
                matrix: ColorMatrix::Bt709,
                range: ColorRange::Limited,
            },
        ]
    }

    /// One arm per variant with no wildcard: a thirteenth code cannot compile
    /// until `POLICY` grows a row for it (IN1 §2.4 rule 35).
    const fn ordinal(code: IncidentCode) -> usize {
        match code {
            IncidentCode::SourceColor(incident) => match incident {
                SourceColorIncident::UnknownPrimaries => 0,
                SourceColorIncident::UnknownTransfer => 1,
                SourceColorIncident::UnknownMatrix => 2,
                SourceColorIncident::UnknownRange => 3,
                SourceColorIncident::UnknownBitDepth => 4,
                SourceColorIncident::UnsupportedPrimaries => 5,
                SourceColorIncident::UnsupportedTransfer => 6,
                SourceColorIncident::UnsupportedMatrix => 7,
                SourceColorIncident::UnsupportedRange => 8,
                SourceColorIncident::UnsupportedWhitePoint => 9,
                SourceColorIncident::UnsupportedBitDepth => 10,
                SourceColorIncident::UnsupportedCombination => 11,
            },
        }
    }

    #[test]
    fn in1_policy_covers_every_incident_code_exactly_once_and_every_row_blocks() {
        assert_eq!(POLICY.len(), 12);
        let mut seen = [0_usize; 12];
        for entry in POLICY {
            seen[ordinal(entry.code)] += 1;
            assert_eq!(
                entry.severity,
                IncidentSeverity::Blocks,
                "{} declares a severity other than Blocks",
                entry.code.code()
            );
        }
        assert!(
            seen.iter().all(|count| *count == 1),
            "every code must have exactly one POLICY row, got {seen:?}"
        );
    }

    #[test]
    fn in1_policy_rows_are_declared_in_table_order() {
        for (index, entry) in POLICY.iter().enumerate() {
            assert_eq!(
                ordinal(entry.code),
                index,
                "POLICY row {index} is out of IN1 §2.2 table order"
            );
            assert_eq!(policy_entry(entry.code), *entry);
        }
    }

    #[test]
    fn in1_source_colour_incidents_mirror_the_classifier_one_to_one() {
        let mut mapped = BTreeSet::new();
        for error in every_source_error() {
            match SourceColorIncident::from_source_error(&error) {
                Some(incident) => {
                    assert_eq!(incident.code(), error.code());
                    assert_eq!(incident.field(), error.field());
                    assert_eq!(IncidentCode::SourceColor(incident).code(), error.code());
                    assert_eq!(IncidentCode::SourceColor(incident).field(), error.field());
                    assert!(
                        mapped.insert(incident),
                        "{} maps to an incident another variant already claimed",
                        error.code()
                    );
                }
                None => assert_eq!(error, ColorSourceError::UnknownWhitePoint),
            }
        }
        assert_eq!(mapped.len(), 12);
    }

    #[test]
    fn in1_incident_codes_serialise_to_their_stable_string() {
        for entry in POLICY {
            let encoded = serde_json::to_string(&entry.code).unwrap();
            assert_eq!(encoded, format!("\"{}\"", entry.code.code()));
        }
    }

    #[test]
    fn in1_every_opened_incident_carries_its_codes_field_and_declared_severity() {
        // Two probes, because `class` is the class the predicate *resolved* at
        // `observe` time and not the class the row declares (IN1 §2.3 rule 11,
        // §2.4 rule 36). `untagged_mp4_probe` is Rec.709-compatible and makes
        // the two coincide; the `Bt2020` variant is not, and separates them on
        // the three predicated rows. The expectations below are written as
        // literals rather than read back from `POLICY`, so an implementation
        // that copied `policy_entry(code).class` into the record fails here.
        let mut bt2020 = untagged_mp4_probe();
        bt2020.primaries = ColorPrimaries::Bt2020;
        for (probed, rec709_compatible) in [(untagged_mp4_probe(), true), (bt2020, false)] {
            for entry in POLICY {
                let mut log = IncidentLog::with_start(Instant::now());
                // Built from the row's own canonical failure, so `observed` and
                // `allowed` belong to the code under test rather than to a
                // borrowed `UnknownPrimaries` fixture.
                let observation = observation_for(&entry.code.source_error(), &probed);
                assert_eq!(observation.code, entry.code);
                let Observed::Opened(id) = log.observe(observation) else {
                    panic!("a fresh log must open {}", entry.code.code());
                };
                let incident = log.get(id).unwrap();
                assert_eq!(incident.field, entry.code.field());
                assert_eq!(incident.severity, entry.severity);
                let expected = match (entry.code, rec709_compatible) {
                    (
                        IncidentCode::SourceColor(
                            SourceColorIncident::UnknownPrimaries
                            | SourceColorIncident::UnknownTransfer
                            | SourceColorIncident::UnknownMatrix,
                        ),
                        true,
                    ) => PolicyClass::AutoApply,
                    _ => PolicyClass::Explain,
                };
                assert_eq!(
                    incident.class,
                    expected,
                    "{} on a {} probe",
                    entry.code.code(),
                    if rec709_compatible {
                        "compatible"
                    } else {
                        "Bt2020"
                    }
                );
            }
        }
    }

    #[test]
    fn in1_every_auto_apply_row_applies_cleanly_and_a_bt2020_probe_explains_instead() {
        let probed = untagged_mp4_probe();
        let mut bt2020 = probed.clone();
        bt2020.primaries = ColorPrimaries::Bt2020;
        let subject = IncidentSubject::Asset(AssetId(1));

        let mut auto_apply_rows = 0;
        for entry in POLICY {
            if entry.class != PolicyClass::AutoApply {
                continue;
            }
            auto_apply_rows += 1;

            let evidence = IncidentEvidence::SourceColor {
                probed: probed.clone(),
                assumption: None,
            };
            assert_eq!(policy_class(entry.code, &probed), PolicyClass::AutoApply);
            let recoveries = policy_recovery(entry.code, subject, &evidence);
            assert_eq!(recoveries.len(), 1);
            assert_eq!(recoveries[0].label, "Assume Rec.709 for this source");
            let RecoveryKind::Operation(operation) = &recoveries[0].kind else {
                panic!("an auto-apply row must carry an operation");
            };
            assert_eq!(
                operation,
                &assume_rec709_operation(AssetId(1), &probed),
                "the recovery must come from the one core builder"
            );

            let mut document = document_with_probed_asset(&probed);
            operation.apply(&mut document).unwrap();
            let asset = &document.media_pool[0];
            assert_eq!(asset.color_description, recovery_description(&probed));
            assert_eq!(asset.assumed_from.as_ref(), Some(&probed));
            assert_eq!(
                classify_source_with_assumption(&asset.color_description, None),
                Ok(ColorSourceProfile::Rec709Video)
            );

            let bt2020_evidence = IncidentEvidence::SourceColor {
                probed: bt2020.clone(),
                assumption: None,
            };
            assert_eq!(policy_class(entry.code, &bt2020), PolicyClass::Explain);
            let explained = policy_recovery(entry.code, subject, &bt2020_evidence);
            assert_eq!(explained.len(), 1);
            assert_eq!(explained[0].label, "How to fix this");
            // The literal sentence, not the expression the implementation
            // evaluates, so the assertion can tell the two apart when §13 D8
            // gives `recovery_action` per-code bodies.
            assert_eq!(
                explained[0].kind,
                RecoveryKind::Explain(
                    "Apply an explicit supported source-colour override or relink to compatible media."
                )
            );
        }
        assert_eq!(auto_apply_rows, 3);
    }

    #[test]
    fn in1_identical_observations_dedup_and_a_new_observed_opens_a_second_incident() {
        let probed = untagged_mp4_probe();
        let mut log = IncidentLog::with_start(Instant::now());

        let Observed::Opened(first) = log.observe(unknown_primaries_observation(&probed)) else {
            panic!("the first observation must open an incident");
        };
        let opened_at = log.get(first).unwrap().opened_at;

        let mut later = unknown_primaries_observation(&probed);
        later.revision = TimelineRevision(4);
        assert_eq!(log.observe(later), Observed::Deduped(first));
        let incident = log.get(first).unwrap();
        assert_eq!(incident.count, 2);
        assert_eq!(incident.revision, TimelineRevision(4));
        assert_eq!(
            incident.opened_at, opened_at,
            "`opened_at` is when the problem was first seen, not last seen"
        );
        assert_eq!(log.open_count(), 1);
        assert_eq!(log.len(), 1);

        let mut different = unknown_primaries_observation(&probed);
        different.observed = "something else".to_owned();
        let Observed::Opened(second) = log.observe(different) else {
            panic!("a different `observed` is a different dedup key");
        };
        assert_ne!(second, first);
        assert_eq!(log.open_count(), 2);
    }

    #[test]
    fn in1_a_webm_evidence_difference_does_not_split_the_incident() {
        let mut log = IncidentLog::with_start(Instant::now());
        let Observed::Opened(first) =
            log.observe(unknown_primaries_observation(&untagged_mp4_probe()))
        else {
            panic!("the first observation must open an incident");
        };
        assert_eq!(
            log.observe(unknown_primaries_observation(&untagged_webm_probe())),
            Observed::Deduped(first),
            "the dedup key is (code, subject, observed) and never the description"
        );
        assert_eq!(log.open_count(), 1);
    }

    #[test]
    fn in1_the_fixture_incident_serialises_to_the_pinned_wire_body() {
        let mut log = IncidentLog::with_start(Instant::now());
        let probed = untagged_webm_probe();
        let Observed::Opened(id) = log.observe(unknown_primaries_observation(&probed)) else {
            panic!("the first observation must open an incident");
        };
        assert_eq!(
            log.observe(unknown_primaries_observation(&probed)),
            Observed::Deduped(id)
        );
        let incident = log.get(id).unwrap();
        let body = serde_json::to_string(incident).unwrap();
        assert_eq!(body, IN1_PINNED_WIRE_BODY);
    }

    #[test]
    fn in1_resolving_an_incident_suppresses_its_code_and_subject() {
        for outcome in [
            IncidentOutcome::Applied,
            IncidentOutcome::Reverted,
            IncidentOutcome::Explained,
        ] {
            let probed = untagged_mp4_probe();
            let mut log = IncidentLog::with_start(Instant::now());
            let Observed::Opened(id) = log.observe(unknown_primaries_observation(&probed)) else {
                panic!("the first observation must open an incident");
            };
            assert!(log.resolve(id, outcome));
            assert_eq!(log.get(id).unwrap().state, IncidentState::Resolved(outcome));
            assert_eq!(log.open_count(), 0);
            assert_eq!(
                log.observe(unknown_primaries_observation(&probed)),
                Observed::Suppressed,
                "a resolved (code, subject) is not reported again this session"
            );
            assert_eq!(log.len(), 1);
        }
        let mut log = IncidentLog::with_start(Instant::now());
        assert!(!log.resolve(IncidentId(7), IncidentOutcome::Applied));
    }

    #[test]
    fn in1_a_noted_auto_apply_suppresses_the_undo_reopen() {
        let probed = untagged_mp4_probe();
        let mut log = IncidentLog::with_start(Instant::now());
        let Observed::Opened(id) = log.observe(unknown_primaries_observation(&probed)) else {
            panic!("the first observation must open an incident");
        };
        assert_eq!(log.get(id).unwrap().class, PolicyClass::AutoApply);

        // The router notes the auto-apply at the moment it *sends* the command,
        // before any acceptance arrives (IN1 §2.3 rule 19).
        assert!(log.note_auto_applied(id));
        assert!(log.resolve(id, IncidentOutcome::Applied));

        // A global `Undo` restores the probed description, the managed decode
        // fails again, and the same refusal is observed once more. Nothing is
        // re-applied and no second incident opens (IN1 §9 clause 17).
        assert_eq!(
            log.observe(unknown_primaries_observation(&probed)),
            Observed::Suppressed
        );
        assert_eq!(log.len(), 1);
        assert_eq!(log.open_count(), 0);
        assert!(log.all().all(|incident| incident.id == id));

        // The person's revert reads as reverted rather than being re-applied.
        assert!(log.resolve(id, IncidentOutcome::Reverted));
        assert_eq!(
            log.get(id).unwrap().state,
            IncidentState::Resolved(IncidentOutcome::Reverted)
        );
        assert!(!log.note_auto_applied(IncidentId(99)));
    }

    #[test]
    fn in1_refresh_revision_moves_only_the_revision_of_an_open_incident() {
        let probed = untagged_mp4_probe();
        let mut log = IncidentLog::with_start(Instant::now());
        let Observed::Opened(id) = log.observe(unknown_primaries_observation(&probed)) else {
            panic!("the first observation must open an incident");
        };
        // The router's suppression is already in place, which is why `observe`
        // cannot do this (IN1 §2.3 rule 19, §5.2 rule 10).
        assert!(log.note_auto_applied(id));
        assert_eq!(
            log.observe(unknown_primaries_observation(&probed)),
            Observed::Suppressed
        );

        let before = log.get(id).unwrap().clone();
        assert_eq!(before.revision, TimelineRevision(1));
        assert!(log.refresh_revision(id, TimelineRevision(9)));

        let after = log.get(id).unwrap().clone();
        assert_eq!(after.revision, TimelineRevision(9));
        // Everything else about the record is byte-identical: comparing the
        // whole `Incident` with only `revision` normalised is what proves
        // `count`, `state`, `opened_at` and telemetry were not touched.
        let mut expected = before.clone();
        expected.revision = TimelineRevision(9);
        assert_eq!(after, expected);
        assert_eq!(after.count, before.count);
        assert_eq!(after.state, IncidentState::Open);
        assert_eq!(after.opened_at, before.opened_at);
        assert_eq!(after.telemetry, before.telemetry);
        assert_eq!(log.open_count(), 1);

        // A resolved entry refuses and is unchanged.
        assert!(log.resolve(id, IncidentOutcome::Applied));
        let resolved = log.get(id).unwrap().clone();
        assert!(!log.refresh_revision(id, TimelineRevision(11)));
        assert_eq!(log.get(id).unwrap(), &resolved);
        assert_eq!(log.get(id).unwrap().revision, TimelineRevision(9));

        // An unknown id refuses.
        assert!(!log.refresh_revision(IncidentId(99), TimelineRevision(11)));
        assert_eq!(log.len(), 1);
    }

    #[test]
    fn in1_telemetry_mut_is_the_only_write_path_for_the_cost_mirrors() {
        let probed = untagged_mp4_probe();
        let mut log = IncidentLog::with_start(Instant::now());
        let Observed::Opened(id) = log.observe(unknown_primaries_observation(&probed)) else {
            panic!("the first observation must open an incident");
        };
        assert_eq!(log.get(id).unwrap().telemetry, IncidentTelemetry::default());
        assert!(log.telemetry_mut(IncidentId(99)).is_none());

        let telemetry = log.telemetry_mut(id).unwrap();
        telemetry.resolved_after = Some(Duration::from_millis(12));
        telemetry.tool_calls = 2;
        telemetry.input_tokens = Some(31);

        let incident = log.get(id).unwrap();
        assert!(incident.telemetry.resolved_after.is_some());
        assert_eq!(incident.telemetry.tool_calls, 2);
        assert_eq!(incident.telemetry.input_tokens, Some(31));
        // IN1 §8 rule 4: the untouched cost mirrors stay honestly absent rather
        // than silently zero, and they skip on the wire.
        for absent in [
            incident.telemetry.cached_input_tokens,
            incident.telemetry.cache_creation_input_tokens,
            incident.telemetry.output_tokens,
            incident.telemetry.reasoning_output_tokens,
        ] {
            assert_eq!(absent, None);
        }
        assert_eq!(incident.telemetry.cost_usd_millionths, None);
        let body = serde_json::to_string(incident).unwrap();
        assert!(body.contains(r#""tool_calls":2"#));
        assert!(body.contains(r#""input_tokens":31"#));
        assert!(!body.contains("output_tokens"));

        // IN1 §2.5 rule 43: `IncidentState` is adjacently tagged under
        // `#[serde(flatten)]`, so a resolved incident renders as the two
        // top-level keys and not as a nested map. Asserted by containment
        // rather than byte for byte, because `resolved_after` is wall-clock
        // (§9 regression R4).
        assert!(log.resolve(id, IncidentOutcome::Applied));
        let resolved = serde_json::to_string(log.get(id).unwrap()).unwrap();
        assert!(resolved.contains(r#""state":"resolved""#), "{resolved}");
        assert!(resolved.contains(r#""outcome":"applied""#), "{resolved}");
    }

    #[test]
    fn in1_opened_at_is_measured_from_the_pinned_session_start() {
        let probed = untagged_mp4_probe();
        let mut log = IncidentLog::with_start(Instant::now());
        let Observed::Opened(first) = log.observe(unknown_primaries_observation(&probed)) else {
            panic!("the first observation must open an incident");
        };
        let mut second_observation = unknown_primaries_observation(&probed);
        second_observation.observed = "a second problem".to_owned();
        let Observed::Opened(second) = log.observe(second_observation) else {
            panic!("a different `observed` opens a second incident");
        };
        let first_opened_at = log.get(first).unwrap().opened_at;
        let second_opened_at = log.get(second).unwrap().opened_at;
        assert!(first_opened_at <= second_opened_at);
        // The stamp is `IncidentLog::with_start`'s clock and not a constant:
        // two `Instant::elapsed()` calls separated by a `Vec` push differ by
        // nanoseconds, so this asserts that the origin exists without
        // asserting a value (IN1 §2.3 rule 17, §9 regression R4).
        assert!(second_opened_at > Duration::ZERO);
        assert!(
            !serde_json::to_string(log.get(first).unwrap())
                .unwrap()
                .contains("opened_at")
        );
        assert_eq!(log.len(), 2);
    }

    #[test]
    fn in1_from_media_error_maps_only_the_asset_scoped_source_colour_refusal() {
        let probed = untagged_mp4_probe();
        let error = MediaError::SourceColorForAsset(Box::new(SourceColorRefusal {
            asset: AssetId(1),
            path: "in1_untagged.mp4".into(),
            error: ColorSourceError::UnknownPrimaries,
            description: probed.clone(),
            assumption: None,
        }));
        let observation =
            IncidentObservation::from_media_error(&error, TimelineRevision(1)).unwrap();
        assert_eq!(
            observation,
            unknown_primaries_observation(&probed),
            "every string field comes from a core accessor, never from the app"
        );

        // The white-point refusal is not an incident in this slice, even with a
        // subject: `color_description_from_decoder` never sets a white point.
        let white_point = MediaError::SourceColorForAsset(Box::new(SourceColorRefusal {
            asset: AssetId(1),
            path: "in1_tagged.mp4".into(),
            error: ColorSourceError::UnknownWhitePoint,
            description: probed.clone(),
            assumption: None,
        }));
        assert!(IncidentObservation::from_media_error(&white_point, TimelineRevision(1)).is_none());

        for other in [
            MediaError::SourceColor(ColorSourceError::UnknownPrimaries),
            MediaError::NotImplemented,
            MediaError::Cancelled,
            MediaError::Backend("alsa: snd_pcm_pause failed".to_owned()),
            MediaError::MixSpectrumRangeTooShort {
                sample_frames: 1,
                required: 2,
            },
            MediaError::MixLoudnessRangeTooShort {
                sample_frames: 1,
                required: 2,
            },
        ] {
            assert!(
                IncidentObservation::from_media_error(&other, TimelineRevision(1)).is_none(),
                "{other} must stay an untyped error line in Part A"
            );
        }
    }

    #[test]
    fn in1_provenance_does_not_gate_the_managed_decode() {
        let recovered = recovery_description(&untagged_mp4_probe());
        assert_eq!(recovered.provenance, ColorProvenance::AgentAssumption);
        for provenance in [
            ColorProvenance::AgentAssumption,
            ColorProvenance::UserOverride,
            ColorProvenance::StreamMetadata,
        ] {
            let mut description = recovered.clone();
            description.provenance = provenance.clone();
            assert_eq!(
                classify_source_with_assumption(&description, None),
                Ok(ColorSourceProfile::Rec709Video),
                "{provenance:?} must not gate the decode"
            );
        }
    }

    #[test]
    fn in1_the_recovery_reproduces_the_shipped_builders_semantics() {
        let mut probed = untagged_mp4_probe();
        probed.range = ColorRange::Full;
        probed.bit_depth = ColorBitDepth::Ten;
        let recovered = recovery_description(&probed);
        assert_eq!(recovered.range, ColorRange::Full, "a known range is kept");
        assert_eq!(
            recovered.bit_depth,
            ColorBitDepth::Ten,
            "a known bit depth is kept"
        );
        assert_eq!(recovered.white_point, ColorWhitePoint::D65);
        assert_eq!(
            recovered.confidence_basis_points,
            COLOR_CONFIDENCE_MAX_BASIS_POINTS
        );
        assert_eq!(
            assume_rec709_operation(AssetId(3), &probed),
            Operation::SetAssetColorDescription {
                asset: AssetId(3),
                color_description: recovered,
            }
        );
    }

    #[test]
    // The cross product is the point: six enumerated tag lists in one place, so
    // a reader can see exactly what the equivalence is asserted over.
    #[allow(clippy::too_many_lines)]
    fn in1_rec709_compatible_definition_equals_its_implementation() {
        /// IN1 §2.4 rule 37's two-clause definition, written out.
        fn by_definition(probed: &ColorDescription) -> bool {
            let recovered = recovery_description(probed);
            let keeps_known_fields = (probed.primaries == ColorPrimaries::Unknown
                || recovered.primaries == probed.primaries)
                && (probed.transfer == ColorTransfer::Unknown
                    || recovered.transfer == probed.transfer)
                && (probed.matrix == ColorMatrix::Unknown || recovered.matrix == probed.matrix)
                && (probed.range == ColorRange::Unknown || recovered.range == probed.range)
                && (probed.white_point == ColorWhitePoint::Unknown
                    || recovered.white_point == probed.white_point)
                && (probed.bit_depth == ColorBitDepth::Unknown
                    || recovered.bit_depth == probed.bit_depth);
            keeps_known_fields
                && classify_source_with_assumption(&recovered, None)
                    == Ok(ColorSourceProfile::Rec709Video)
        }

        let primaries = [
            ColorPrimaries::Unknown,
            ColorPrimaries::Srgb,
            ColorPrimaries::Bt709,
            ColorPrimaries::Bt2020,
            ColorPrimaries::DisplayP3,
            ColorPrimaries::DciP3,
            ColorPrimaries::Smpte170M,
            ColorPrimaries::Smpte240M,
            ColorPrimaries::Bt470M,
            ColorPrimaries::Bt470Bg,
            ColorPrimaries::Film,
            ColorPrimaries::Other("x".to_owned()),
        ];
        let transfers = [
            ColorTransfer::Unknown,
            ColorTransfer::Srgb,
            ColorTransfer::Bt709,
            ColorTransfer::Bt1886,
            ColorTransfer::Linear,
            ColorTransfer::Gamma22,
            ColorTransfer::Gamma28,
            ColorTransfer::Smpte170M,
            ColorTransfer::Smpte2084,
            ColorTransfer::AribStdB67,
            ColorTransfer::Log,
            ColorTransfer::LogC,
            ColorTransfer::Log3G10,
            ColorTransfer::Other("x".to_owned()),
        ];
        let matrices = [
            ColorMatrix::Unknown,
            ColorMatrix::Identity,
            ColorMatrix::Rgb,
            ColorMatrix::Bt709,
            ColorMatrix::Bt2020Ncl,
            ColorMatrix::Bt2020Cl,
            ColorMatrix::Smpte170M,
            ColorMatrix::Smpte240M,
            ColorMatrix::Ycgco,
            ColorMatrix::ChromaDerivedNcl,
            ColorMatrix::ChromaDerivedCl,
            ColorMatrix::Ictcp,
            ColorMatrix::Other("x".to_owned()),
        ];
        let ranges = [
            ColorRange::Unknown,
            ColorRange::Full,
            ColorRange::Limited,
            ColorRange::Other("x".to_owned()),
        ];
        let white_points = [
            ColorWhitePoint::Unknown,
            ColorWhitePoint::D50,
            ColorWhitePoint::D55,
            ColorWhitePoint::D60,
            ColorWhitePoint::D65,
            ColorWhitePoint::Dci,
            ColorWhitePoint::Other("x".to_owned()),
        ];
        let bit_depths = [
            ColorBitDepth::Unknown,
            ColorBitDepth::Eight,
            ColorBitDepth::Ten,
            ColorBitDepth::Twelve,
            ColorBitDepth::Sixteen,
            ColorBitDepth::Float16,
            ColorBitDepth::Float32,
            ColorBitDepth::Integer(1),
            ColorBitDepth::Integer(7),
            ColorBitDepth::Integer(8),
            ColorBitDepth::Integer(16),
            ColorBitDepth::Integer(17),
            ColorBitDepth::Other("x".to_owned()),
        ];

        let mut checked = 0_u32;
        let mut agreed_true = 0_u32;
        let mut probe = ColorDescription::unknown();
        probe.confidence_basis_points = 2_000;
        for primary in &primaries {
            probe.primaries = primary.clone();
            for transfer in &transfers {
                probe.transfer = transfer.clone();
                for matrix in &matrices {
                    probe.matrix = matrix.clone();
                    for range in &ranges {
                        probe.range = range.clone();
                        for white_point in &white_points {
                            probe.white_point = white_point.clone();
                            for bit_depth in &bit_depths {
                                probe.bit_depth = bit_depth.clone();
                                let implemented = rec709_compatible(&probe);
                                assert_eq!(
                                    implemented,
                                    by_definition(&probe),
                                    "definition and implementation disagree on {probe:?}"
                                );
                                checked += 1;
                                if implemented {
                                    agreed_true += 1;
                                }
                            }
                        }
                    }
                }
            }
        }
        assert_eq!(
            checked,
            u32::try_from(
                primaries.len()
                    * transfers.len()
                    * matrices.len()
                    * ranges.len()
                    * white_points.len()
                    * bit_depths.len()
            )
            .unwrap()
        );
        assert!(
            agreed_true > 0,
            "a predicate that is never true proves nothing"
        );
    }

    /// IN1 §6.2 rule 10's body, normative as generated.
    const IN1_PINNED_WIRE_BODY: &str = concat!(
        r#"{"id":1,"code":"unknown_source_primaries","class":"auto_apply","severity":"blocks","#,
        r#""subject":{"asset":1},"field":"primaries","observed":"unknown","#,
        r#""allowed":"bt709 or srgb in a supported CC1 profile","#,
        r#""evidence":{"source_color":{"probed":{"primaries":"unknown","transfer":"unknown","#,
        r#""matrix":"unknown","range":"limited","white_point":"unknown","bit_depth":8,"#,
        r#""confidence_basis_points":4000,"provenance":"stream_metadata"},"assumption":null}},"#,
        r#""recoveries":[{"label":"Assume Rec.709 for this source","kind":{"operation":"#,
        r#"{"SetAssetColorDescription":{"asset":1,"color_description":{"primaries":"bt709","#,
        r#""transfer":"bt709","matrix":"bt709","range":"limited","white_point":"d65","#,
        r#""bit_depth":8,"confidence_basis_points":10000,"provenance":"agent_assumption"}}}}}],"#,
        r#""revision":1,"count":2,"state":"open","telemetry":{"tool_calls":0}}"#,
    );
}
