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
    collections::{BTreeMap, BTreeSet},
    time::{Duration, Instant, SystemTime},
};

use serde::{Deserialize, Serialize, Serializer, ser};

use crate::{
    AssetId, AudioChain, BatchError, COLOR_CONFIDENCE_MAX_BASIS_POINTS, CaptionPlanError, ClipId,
    ColorBitDepth, ColorDescription, ColorMatrix, ColorPrimaries, ColorProvenance, ColorQcError,
    ColorRange, ColorSourceError, ColorSourceProfileAssumption, ColorTransfer, ColorWhitePoint,
    DeliveryColorError, DeliveryColorMismatch, DeliveryVariantError, DeliveryVerificationError,
    EffectId, IncidentFamily, LutAssetId, MediaError, OpError, Operation, TimelineRevision,
    TrackId,
};

/// A source-colour failure that Kinewright classifies as an incident.
///
/// Mirrors [`ColorSourceError`] one-to-one: **thirteen** incident codes from
/// thirteen classifier variants (`IN1b` §3.2 rule 16, erratum `IN1b`-R4).
///
/// Part A declared twelve, leaving `unknown_source_white_point` out because
/// `color_description_from_decoder` never sets a white point, so every
/// correctly BT.709-tagged source fails the *bare* classifier with that code
/// (IN1 §2.2 rule 6). That reason is about the bare classifier and is
/// unchanged; `IN1b` §3.2 rule 14 measures the residue it left — a source with
/// `primaries: Srgb` and `white_point: Unknown` receives no D65 assumption and
/// reaches `Err(UnknownWhitePoint)` after it — so the code is reachable and is
/// declared.
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
    /// The source declares no white point.
    ///
    /// Declared last rather than beside its siblings so Part A's twelve
    /// [`POLICY`] row indices do not move (`IN1b` §3.2 rule 16).
    UnknownWhitePoint,
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

    /// The incident for a classifier failure.
    ///
    /// Total: there is no `None` case any more (`IN1b` §3.2 rule 16, erratum
    /// `IN1b`-R4). The match carries no wildcard, so a fourteenth classifier
    /// variant breaks the build.
    #[must_use]
    pub const fn from_source_error(error: &ColorSourceError) -> Self {
        match error {
            ColorSourceError::UnknownPrimaries => Self::UnknownPrimaries,
            ColorSourceError::UnknownTransfer => Self::UnknownTransfer,
            ColorSourceError::UnknownMatrix => Self::UnknownMatrix,
            ColorSourceError::UnknownRange => Self::UnknownRange,
            ColorSourceError::UnknownBitDepth => Self::UnknownBitDepth,
            ColorSourceError::UnsupportedPrimaries(_) => Self::UnsupportedPrimaries,
            ColorSourceError::UnsupportedTransfer(_) => Self::UnsupportedTransfer,
            ColorSourceError::UnsupportedMatrix(_) => Self::UnsupportedMatrix,
            ColorSourceError::UnsupportedRange(_) => Self::UnsupportedRange,
            ColorSourceError::UnsupportedWhitePoint(_) => Self::UnsupportedWhitePoint,
            ColorSourceError::UnsupportedBitDepth(_) => Self::UnsupportedBitDepth,
            ColorSourceError::UnsupportedCombination { .. } => Self::UnsupportedCombination,
            ColorSourceError::UnknownWhitePoint => Self::UnknownWhitePoint,
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
            Self::UnknownWhitePoint => ColorSourceError::UnknownWhitePoint,
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
            Self::UnknownWhitePoint => 12,
        }
    }
}

/// The stable code string every code-less [`MediaError`] variant shares.
///
/// Declared once so [`MediaIncident::code`] can delegate its other arm without
/// restating a literal in the delegation's unreachable fallback.
const MEDIA_BACKEND_UNCLASSIFIED: &str = "media_backend_unclassified";

/// A media failure that is not a source-colour one (`IN1b` §3.2 rule 10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MediaIncident {
    /// The managed renderer cannot prove the decoder's native format is a
    /// supported integer source surface.
    UnsupportedDecoderFormat,
    /// The media engine refused with a reason and no code of its own: the five
    /// `MediaError` variants whose `recovery_code()` is `None`.
    BackendUnclassified,
}

impl MediaIncident {
    /// The stable machine-readable code.
    ///
    /// `unsupported_decoder_format` is delegated to
    /// [`MediaError::recovery_code`], which owns the string (`IN1b` §3.2
    /// rule 11); the `None` arm cannot be reached, because that accessor
    /// answers `Some` for the variant built below, and it yields the shared
    /// constant rather than panicking in core.
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Self::UnsupportedDecoderFormat => match self.media_error().recovery_code() {
                Some(code) => code,
                None => MEDIA_BACKEND_UNCLASSIFIED,
            },
            Self::BackendUnclassified => MEDIA_BACKEND_UNCLASSIFIED,
        }
    }

    /// The stable field associated with the failure. Minted: [`MediaError`]
    /// ships no `field()` accessor to delegate to.
    #[must_use]
    pub const fn field(self) -> &'static str {
        match self {
            Self::UnsupportedDecoderFormat => "format",
            Self::BackendUnclassified => "media",
        }
    }

    /// A canonical media failure for this incident, used only to reach
    /// [`MediaError::recovery_code`]. The payload is a placeholder and never
    /// leaves this module.
    fn media_error(self) -> MediaError {
        match self {
            Self::UnsupportedDecoderFormat => MediaError::UnsupportedDecoderFormat {
                path: std::path::PathBuf::new(),
                format: String::new(),
                declared_bit_depth: None,
                decoder_bit_depth: None,
                reason: String::new(),
            },
            Self::BackendUnclassified => MediaError::Backend(String::new()),
        }
    }

    const fn table_index(self) -> usize {
        match self {
            Self::UnsupportedDecoderFormat => 0,
            Self::BackendUnclassified => 1,
        }
    }
}

/// A managed delivery encode refused for a typed colour reason
/// (`IN1b` §3.2 rule 10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DeliveryColorIncident {
    /// The video codec cannot carry managed delivery tags.
    UnsupportedCodec,
    /// One delivery-colour field is outside the managed set.
    UnsupportedField,
    /// The negotiated pixel format does not carry the declared depth.
    PixelFormatDepthMismatch,
    /// This build's encoder does not offer the required pixel format.
    EncoderPixelFormatUnavailable,
}

impl DeliveryColorIncident {
    /// The stable machine-readable code, delegated to [`DeliveryColorError`].
    #[must_use]
    pub fn code(self) -> &'static str {
        self.delivery_error().code()
    }

    /// The stable settings field associated with the failure.
    ///
    /// **Minted, not delegated, and the reason is a type**:
    /// `DeliveryColorError::field()` returns `&str` borrowed from `&self`
    /// because [`DeliveryColorError::UnsupportedField`] answers with the
    /// mismatch's own per-instance field name, so there is no `&'static str`
    /// to delegate to. The three static arms below are asserted equal to that
    /// accessor by `in1b_every_code_delegates_its_string_to_the_accessor_that_owns_it`;
    /// `UnsupportedField` answers `delivery_color`, the name of the check
    /// rather than of the one field that failed (erratum `IN1b`-A-R4).
    #[must_use]
    pub const fn field(self) -> &'static str {
        match self {
            Self::UnsupportedCodec => "video_codec",
            Self::UnsupportedField => "delivery_color",
            Self::PixelFormatDepthMismatch | Self::EncoderPixelFormatUnavailable => "pixel_format",
        }
    }

    /// The shipped recovery sentence, delegated (`IN1b` §3.7 rule 34).
    #[must_use]
    pub fn recovery_action(self) -> &'static str {
        self.delivery_error().recovery_action()
    }

    /// A canonical delivery failure, used only to reach the `const` accessors
    /// that ignore the payload.
    /// The incident one [`DeliveryColorError`] classifies as, with no
    /// wildcard arm so a fifth variant breaks the build.
    #[must_use]
    pub const fn from_delivery_error(error: &DeliveryColorError) -> Self {
        match error {
            DeliveryColorError::UnsupportedCodec { .. } => Self::UnsupportedCodec,
            DeliveryColorError::UnsupportedField(_) => Self::UnsupportedField,
            DeliveryColorError::PixelFormatDepthMismatch { .. } => Self::PixelFormatDepthMismatch,
            DeliveryColorError::EncoderPixelFormatUnavailable { .. } => {
                Self::EncoderPixelFormatUnavailable
            }
        }
    }

    fn delivery_error(self) -> DeliveryColorError {
        match self {
            Self::UnsupportedCodec => DeliveryColorError::UnsupportedCodec {
                observed: String::new(),
                allowed: "",
            },
            Self::UnsupportedField => DeliveryColorError::UnsupportedField(DeliveryColorMismatch {
                field: String::new(),
                observed: String::new(),
                allowed: String::new(),
            }),
            Self::PixelFormatDepthMismatch => DeliveryColorError::PixelFormatDepthMismatch {
                observed: String::new(),
                allowed: String::new(),
            },
            Self::EncoderPixelFormatUnavailable => {
                DeliveryColorError::EncoderPixelFormatUnavailable {
                    observed: String::new(),
                    allowed: String::new(),
                }
            }
        }
    }

    const fn table_index(self) -> usize {
        match self {
            Self::UnsupportedCodec => 0,
            Self::UnsupportedField => 1,
            Self::PixelFormatDepthMismatch => 2,
            Self::EncoderPixelFormatUnavailable => 3,
        }
    }
}

/// Post-export delivery verification could not produce an honest measurement
/// (`IN1b` §3.2 rule 10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DeliveryVerificationIncident {
    /// The sampled reference render is not full resolution.
    NotFullResolution,
    /// A native plane sample does not fit its declared container.
    PlaneOutOfContainer,
    /// The written file holds a different number of frames than the document.
    FrameCountMismatch,
    /// The requested sample count is outside the accepted range.
    FrameCountOutOfRange,
    /// The request carries another lane's budgets.
    BudgetLaneMismatch,
}

impl DeliveryVerificationIncident {
    /// The stable machine-readable code, delegated to
    /// [`DeliveryVerificationError`].
    #[must_use]
    pub fn code(self) -> &'static str {
        self.verification_error().code()
    }

    /// The stable verification field, delegated the same way.
    #[must_use]
    pub fn field(self) -> &'static str {
        self.verification_error().field()
    }

    /// The shipped recovery sentence, delegated (`IN1b` §3.7 rule 34).
    #[must_use]
    pub fn recovery_action(self) -> &'static str {
        self.verification_error().recovery_action()
    }

    /// The incident one [`DeliveryVerificationError`] classifies as, with no
    /// wildcard arm so a sixth variant breaks the build.
    #[must_use]
    pub const fn from_verification_error(error: &DeliveryVerificationError) -> Self {
        match error {
            DeliveryVerificationError::NotFullResolution { .. } => Self::NotFullResolution,
            DeliveryVerificationError::PlaneOutOfContainer { .. } => Self::PlaneOutOfContainer,
            DeliveryVerificationError::FrameCountMismatch { .. } => Self::FrameCountMismatch,
            DeliveryVerificationError::FrameCountOutOfRange { .. } => Self::FrameCountOutOfRange,
            DeliveryVerificationError::BudgetLaneMismatch { .. } => Self::BudgetLaneMismatch,
        }
    }

    fn verification_error(self) -> DeliveryVerificationError {
        match self {
            Self::NotFullResolution => DeliveryVerificationError::NotFullResolution {
                observed: String::new(),
                allowed: "",
            },
            Self::PlaneOutOfContainer => DeliveryVerificationError::PlaneOutOfContainer {
                observed: String::new(),
                allowed: "",
            },
            Self::FrameCountMismatch => DeliveryVerificationError::FrameCountMismatch {
                observed: String::new(),
                allowed: String::new(),
            },
            Self::FrameCountOutOfRange => DeliveryVerificationError::FrameCountOutOfRange {
                observed: String::new(),
                allowed: "",
            },
            Self::BudgetLaneMismatch => DeliveryVerificationError::BudgetLaneMismatch {
                observed: String::new(),
                allowed: String::new(),
            },
        }
    }

    const fn table_index(self) -> usize {
        match self {
            Self::NotFullResolution => 0,
            Self::PlaneOutOfContainer => 1,
            Self::FrameCountMismatch => 2,
            Self::FrameCountOutOfRange => 3,
            Self::BudgetLaneMismatch => 4,
        }
    }
}

/// A colour QC measurement refused with a typed reason (`IN1b` §3.2 rule 10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ColorQcIncident {
    /// A proxy raster was offered as a delivery reference.
    ProxyProofRefused,
    /// The pixel buffer is not `width * height * 4` samples.
    RasterLengthMismatch,
    /// The resolved region of interest is empty.
    EmptyPopulation,
    /// More nodes were requested than the budget allows.
    NodeBudgetExceeded,
    /// The coverage raster does not match the region.
    MatteRegionRasterMismatch,
    /// The scratch node removal was refused by the document model.
    NodeRemovalRejected,
}

impl ColorQcIncident {
    /// The stable machine-readable code, delegated to [`ColorQcError`].
    #[must_use]
    pub fn code(self) -> &'static str {
        self.qc_error().code()
    }

    /// The stable request or proof field, delegated the same way.
    #[must_use]
    pub fn field(self) -> &'static str {
        self.qc_error().field()
    }

    /// The shipped recovery sentence, delegated (`IN1b` §3.7 rule 34).
    #[must_use]
    pub fn recovery_action(self) -> &'static str {
        self.qc_error().recovery_action()
    }

    /// The incident one [`ColorQcError`] classifies as, with no wildcard arm so
    /// a seventh variant breaks the build.
    #[must_use]
    pub const fn from_qc_error(error: &ColorQcError) -> Self {
        match error {
            ColorQcError::ProxyProofRefused { .. } => Self::ProxyProofRefused,
            ColorQcError::RasterLengthMismatch { .. } => Self::RasterLengthMismatch,
            ColorQcError::EmptyPopulation { .. } => Self::EmptyPopulation,
            ColorQcError::NodeBudgetExceeded { .. } => Self::NodeBudgetExceeded,
            ColorQcError::MatteRegionRasterMismatch { .. } => Self::MatteRegionRasterMismatch,
            ColorQcError::NodeRemovalRejected { .. } => Self::NodeRemovalRejected,
        }
    }

    fn qc_error(self) -> ColorQcError {
        match self {
            Self::ProxyProofRefused => ColorQcError::ProxyProofRefused {
                observed: String::new(),
                allowed: "",
            },
            Self::RasterLengthMismatch => ColorQcError::RasterLengthMismatch {
                observed: String::new(),
                allowed: String::new(),
            },
            Self::EmptyPopulation => ColorQcError::EmptyPopulation {
                observed: String::new(),
                allowed: "",
            },
            Self::NodeBudgetExceeded => ColorQcError::NodeBudgetExceeded {
                observed: String::new(),
                allowed: "",
            },
            Self::MatteRegionRasterMismatch => ColorQcError::MatteRegionRasterMismatch {
                observed: String::new(),
                allowed: String::new(),
            },
            Self::NodeRemovalRejected => ColorQcError::NodeRemovalRejected {
                clip: ClipId(0),
                effect: EffectId(0),
                reason: String::new(),
            },
        }
    }

    const fn table_index(self) -> usize {
        match self {
            Self::ProxyProofRefused => 0,
            Self::RasterLengthMismatch => 1,
            Self::EmptyPopulation => 2,
            Self::NodeBudgetExceeded => 3,
            Self::MatteRegionRasterMismatch => 4,
            Self::NodeRemovalRejected => 5,
        }
    }
}

/// A refusal raised by one of the seven typed rejection enums
/// (`IN1b` §3.2 rule 10).
///
/// Four of the seven live outside core — `BranchError` in the agent crate,
/// `SourceEditRejection`, the relink pair and `ProjectSaveError` in the app —
/// and each reads this enum from its own crate. Core owns the code and the
/// policy class; core does not own the code's trigger (IN1 §2.1 rule 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RejectionIncident {
    /// An edit plan was refused as a whole.
    EditPlan,
    /// A delivery variant could not be built from this project.
    DeliveryVariant,
    /// The isolated agent branch refused the request.
    AgentBranch,
    /// A Source-monitor edit was refused before anything was applied.
    SourceEdit,
    /// A relink candidate was refused.
    Relink,
    /// The project file could not be written.
    ProjectSave,
    /// A caption track could not be planned from the cues.
    CaptionPlan,
}

impl RejectionIncident {
    /// The stable machine-readable code. Minted: none of the seven enums ships
    /// a `code()` accessor (`IN1b` §3.2 rule 12).
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::EditPlan => "edit_plan_rejected",
            Self::DeliveryVariant => "delivery_variant_rejected",
            Self::AgentBranch => "agent_branch_rejected",
            Self::SourceEdit => "source_edit_rejected",
            Self::Relink => "relink_rejected",
            Self::ProjectSave => "project_save_failed",
            Self::CaptionPlan => "caption_plan_rejected",
        }
    }

    /// The stable field associated with the refusal.
    #[must_use]
    pub const fn field(self) -> &'static str {
        match self {
            Self::EditPlan => "edit_plan",
            Self::DeliveryVariant => "delivery_variant",
            Self::AgentBranch => "agent_branch",
            Self::SourceEdit => "source_edit",
            Self::Relink => "relink",
            Self::ProjectSave => "project_file",
            Self::CaptionPlan => "caption_plan",
        }
    }

    const fn table_index(self) -> usize {
        match self {
            Self::EditPlan => 0,
            Self::DeliveryVariant => 1,
            Self::AgentBranch => 2,
            Self::SourceEdit => 3,
            Self::Relink => 4,
            Self::ProjectSave => 5,
            Self::CaptionPlan => 6,
        }
    }
}

/// One code per source label the application still reports under
/// (`IN1b` §3.2 rule 12), plus the seven `IN2B` §5 rows.
///
/// Fifteen are placeholders — the label's failures have no typed payload yet —
/// and two are not: `look_incomplete` and `media_incomplete` exist because
/// their label's sites disagree about severity, and a code declares one
/// severity (`IN1b` §3.6 rule 30). Labels carry untyped `Plain` evidence —
/// the §5 rows are labels because no typed payload exists for them, not
/// because they name a source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LabelIncident {
    /// `Operations`, with no typed payload.
    Operations,
    /// `Look`, with no typed payload.
    Look,
    /// `Look`, where the save or open completed with looks missing.
    LookIncomplete,
    /// `Export`, with no typed payload.
    Export,
    /// `Source monitor`, with no typed payload.
    SourceMonitor,
    /// `Relink`, with no typed payload.
    Relink,
    /// `Agent branch`, with no typed payload.
    AgentBranch,
    /// `Transcript edit`, with no typed payload.
    TranscriptEdit,
    /// `Media`, with no typed payload.
    Media,
    /// `Media`, where the open completed with media missing.
    MediaIncomplete,
    /// `Agent`, with no typed payload.
    Agent,
    /// `Recording`, with no typed payload.
    Recording,
    /// `Project`, with no typed payload.
    Project,
    /// `Captions`, with no typed payload.
    Captions,
    /// `Mixer`, with no typed payload.
    Mixer,
    /// `Media cache`, with no typed payload.
    MediaCache,
    /// `Timeline`, reachable only through the inspector's dynamic site.
    Timeline,
    /// `Panel worker error`, with no typed payload (`IN2B` §5).
    PanelWorkerError,
    /// `Recovery damage`, with no typed payload (`IN2B` §5).
    RecoveryDamage,
    /// `Recovery unavailable`, with no typed payload (`IN2B` §5).
    RecoveryUnavailable,
    /// `Newer project format`, with no typed payload (`IN2B` §5).
    ProjectNewerFormat,
    /// `Sidecar unknown codes`, with no typed payload (`IN2B` §5).
    SidecarUnknownCodes,
    /// `Sidecar refused`, with no typed payload (`IN2B` §5).
    SidecarRefused,
    /// `Sidecar write failed`, with no typed payload (`IN2B` §5).
    SidecarWriteFailed,
}

impl LabelIncident {
    /// The stable machine-readable code. Minted: a label has no accessor.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Operations => "operations_unclassified",
            Self::Look => "look_unclassified",
            Self::LookIncomplete => "look_incomplete",
            Self::Export => "export_unclassified",
            Self::SourceMonitor => "source_monitor_unclassified",
            Self::Relink => "relink_unclassified",
            Self::AgentBranch => "agent_branch_unclassified",
            Self::TranscriptEdit => "transcript_edit_unclassified",
            Self::Media => "media_unclassified",
            Self::MediaIncomplete => "media_incomplete",
            Self::Agent => "agent_unclassified",
            Self::Recording => "recording_unclassified",
            Self::Project => "project_unclassified",
            Self::Captions => "captions_unclassified",
            Self::Mixer => "mixer_unclassified",
            Self::MediaCache => "media_cache_unclassified",
            Self::Timeline => "timeline_unclassified",
            Self::PanelWorkerError => "panel_worker_error",
            Self::RecoveryDamage => "recovery_damage",
            Self::RecoveryUnavailable => "recovery_unavailable",
            Self::ProjectNewerFormat => "project_newer_format",
            Self::SidecarUnknownCodes => "sidecar_unknown_codes",
            Self::SidecarRefused => "sidecar_refused",
            Self::SidecarWriteFailed => "sidecar_write_failed",
        }
    }

    /// The stable field associated with the label.
    #[must_use]
    pub const fn field(self) -> &'static str {
        match self {
            Self::Operations => "operations",
            Self::Look | Self::LookIncomplete => "look",
            Self::Export => "export",
            Self::SourceMonitor => "source_monitor",
            Self::Relink => "relink",
            Self::AgentBranch => "agent_branch",
            Self::TranscriptEdit => "transcript_edit",
            Self::Media | Self::MediaIncomplete => "media",
            Self::Agent => "agent",
            Self::Recording => "recording",
            Self::Project | Self::ProjectNewerFormat => "project",
            Self::Captions => "captions",
            Self::Mixer => "mixer",
            Self::MediaCache => "media_cache",
            Self::Timeline => "timeline",
            Self::PanelWorkerError => "panel",
            Self::RecoveryDamage | Self::RecoveryUnavailable => "recovery",
            Self::SidecarUnknownCodes | Self::SidecarRefused | Self::SidecarWriteFailed => {
                "sidecar"
            }
        }
    }

    const fn table_index(self) -> usize {
        match self {
            Self::Operations => 0,
            Self::Look => 1,
            Self::LookIncomplete => 2,
            Self::Export => 3,
            Self::SourceMonitor => 4,
            Self::Relink => 5,
            Self::AgentBranch => 6,
            Self::TranscriptEdit => 7,
            Self::Media => 8,
            Self::MediaIncomplete => 9,
            Self::Agent => 10,
            Self::Recording => 11,
            Self::Project => 12,
            Self::Captions => 13,
            Self::Mixer => 14,
            Self::MediaCache => 15,
            Self::Timeline => 16,
            Self::PanelWorkerError => 17,
            Self::RecoveryDamage => 18,
            Self::RecoveryUnavailable => 19,
            Self::ProjectNewerFormat => 20,
            Self::SidecarUnknownCodes => 21,
            Self::SidecarRefused => 22,
            Self::SidecarWriteFailed => 23,
        }
    }
}

/// The exhaustive set of incident codes Kinewright declares.
///
/// **74 codes** (`IN1b` §3.2 rule 12, `IN2B` E-B9): thirteen source-colour,
/// two media, four delivery-colour, five delivery-verification, six colour-QC,
/// eleven operation families, the minted LUT-asset and revision-conflict rows,
/// seven typed rejections and twenty-four source labels. All seventy-four are
/// reachable: the eleven delivery-verification and colour-QC rows through the
/// `IN2B` §6 seams and the seven §5 labels through their notes, so `IN1b` §3.2
/// rule 13's exemption is spent and the count reads 74/0.
///
/// There is no catch-all, no `Unclassified` and no `Other(String)` (IN1 §2.2
/// rule 9). It also carries no `#[non_exhaustive]`, because that would force a
/// wildcard arm into every downstream `match` and make IN1 §2.4 rule 35's
/// exhaustiveness test — the one thing that proves [`POLICY`] covers every
/// code — pass vacuously (IN1 §2.2 rule 10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum IncidentCode {
    /// A CC1 source-colour classification failure.
    SourceColor(SourceColorIncident),
    /// A media failure that is not a source-colour one.
    Media(MediaIncident),
    /// A managed delivery encode refused for a typed colour reason.
    DeliveryColor(DeliveryColorIncident),
    /// A delivery verification that could not produce an honest measurement.
    DeliveryVerification(DeliveryVerificationIncident),
    /// A refused colour QC measurement.
    ColorQc(ColorQcIncident),
    /// A rejected edit, by the family of thing that must change.
    Operation(IncidentFamily),
    /// A LUT allowlist refusal, whose recovery is the store's own import
    /// rather than "supply a different field" (`IN1b` §3.1 rule 6).
    LutAssetPolicy,
    /// A revision-gated send refused because the timeline moved.
    EditRevisionConflict,
    /// A refusal raised by one of the seven typed rejection enums.
    Rejection(RejectionIncident),
    /// A source label whose failures have no typed payload yet.
    Label(LabelIncident),
}

impl IncidentCode {
    /// The stable machine-readable code.
    ///
    /// Delegated to the accessor that owns the string wherever one exists, and
    /// minted otherwise; no string is restated (`IN1b` §3.2 rule 11).
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Self::SourceColor(incident) => incident.code(),
            Self::Media(incident) => incident.code(),
            Self::DeliveryColor(incident) => incident.code(),
            Self::DeliveryVerification(incident) => incident.code(),
            Self::ColorQc(incident) => incident.code(),
            Self::Operation(family) => family.code(),
            Self::LutAssetPolicy => "lut_asset_policy",
            Self::EditRevisionConflict => "edit_revision_conflict",
            Self::Rejection(incident) => incident.code(),
            Self::Label(incident) => incident.code(),
        }
    }

    /// The stable field the failure is about, delegated the same way.
    #[must_use]
    pub fn field(self) -> &'static str {
        match self {
            Self::SourceColor(incident) => incident.field(),
            Self::Media(incident) => incident.field(),
            Self::DeliveryColor(incident) => incident.field(),
            Self::DeliveryVerification(incident) => incident.field(),
            Self::ColorQc(incident) => incident.field(),
            // An operation rejection names its field inside the message; the
            // three variants that keep an untyped `reason` are `IN1b` §13 D-B5.
            Self::Operation(_) => "operation",
            Self::LutAssetPolicy => "lut_asset",
            Self::EditRevisionConflict => "revision",
            Self::Rejection(incident) => incident.field(),
            Self::Label(incident) => incident.field(),
        }
    }

    /// This code's row index in [`POLICY`], in the enum's declaration order.
    ///
    /// Total by construction, so [`policy_class`] needs no fallible lookup and
    /// no panic. `in1b_policy_rows_are_declared_in_table_order` pins the
    /// correspondence and `IN1b` §9 clause 2's test pins the coverage.
    const fn table_index(self) -> usize {
        match self {
            Self::SourceColor(incident) => incident.table_index(),
            Self::Media(incident) => 13 + incident.table_index(),
            Self::DeliveryColor(incident) => 15 + incident.table_index(),
            Self::DeliveryVerification(incident) => 19 + incident.table_index(),
            Self::ColorQc(incident) => 24 + incident.table_index(),
            Self::Operation(family) => 30 + family_index(family),
            Self::LutAssetPolicy => 41,
            Self::EditRevisionConflict => 42,
            Self::Rejection(incident) => 43 + incident.table_index(),
            Self::Label(incident) => 50 + incident.table_index(),
        }
    }
}

/// [`IncidentFamily`]'s row offset inside [`POLICY`].
///
/// Declared here rather than beside the enum so a twelfth family breaks this
/// file too, where the eleven rows it must gain live.
const fn family_index(family: IncidentFamily) -> usize {
    match family {
        IncidentFamily::Bounds => 0,
        IncidentFamily::Malformed => 1,
        IncidentFamily::Duplicate => 2,
        IncidentFamily::Placement => 3,
        IncidentFamily::Missing => 4,
        IncidentFamily::Structure => 5,
        IncidentFamily::Relink => 6,
        IncidentFamily::Unrepresentable => 7,
        IncidentFamily::UnknownName => 8,
        IncidentFamily::Internal => 9,
        IncidentFamily::ColorPolicy => 10,
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

// There is deliberately no `Deserialize` for `IncidentCode`, and there still
// is not: the code-string-to-code direction arrives by erratum E-B8 as
// [`from_code`], a scan over [`POLICY`] — a scan cannot disagree with the
// table it scans, where a derive would mint a second source of truth.
// `IncidentCode` itself stays serialise-only.

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

/// What the incident is about, and the axis two failures dedup on.
///
/// The dedup key is `(code, subject, observed)` (IN1 §2.3 rule 15) and
/// `suppressed` is a `BTreeSet<(IncidentCode, IncidentSubject)>`, so the
/// subject **is** the dedup axis and must be `Ord`. Each variant names the
/// thing whose repeated failure is one problem (`IN1b` §3.3 rule 17).
///
/// [`Self::LutAsset`] is the eighth variant, added by ruling N5: one LUT whose
/// import, restore or hash check keeps refusing is one problem, and the seven
/// `Look` rows of Appendix B anchor to it rather than to the project.
///
/// `ExportJob`, `Project` and `Agent` are **unit** variants and no id type is
/// minted for them (`IN1b` §3.3 rule 18): the application runs one export at a
/// time, an agent thread is addressed by a re-indexed position rather than by
/// an id, and the incident log is per-project by construction. Their dedup axis
/// is therefore `(code, observed)`.
///
/// `Subject(String)` is rejected, and the reason is the dedup key: two
/// spellings of one clip would become two problems.
///
/// The wire union is three shapes, stated rather than discovered
/// (`IN1b` §3.11 rule 42): a one-key object over a transparent `u64`
/// (`{"asset":1}`, `{"lut_asset":5}`), a one-key object over a nested union
/// (`{"chain":{"bus":3}}` or `{"chain":"master"}`), and a bare string
/// (`"export_job"`, `"project"`, `"agent"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IncidentSubject {
    /// One media file that keeps refusing.
    Asset(AssetId),
    /// One LUT in the project's look store that keeps refusing.
    ///
    /// Its own variant rather than an [`Self::Asset`], because `LutAssetId` is
    /// a separate id space from `AssetId` (`model.rs`'s `id_type!` mints both)
    /// and a look failure anchored to [`Self::Project`] loses the card's
    /// anchor: the person cannot see *which* look is broken. §3.3 rule 17's
    /// claim that `Asset(AssetId)` covers "Look (per-LUT)" is erratum
    /// `IN1b`-A-R13.
    LutAsset(LutAssetId),
    /// One clip whose trim, speed or effect keeps failing.
    Clip(ClipId),
    /// One track's capture refusal.
    Track(TrackId),
    /// One bus or the master, whose mix or learn keeps refusing.
    Chain(AudioChain),
    /// One export attempt, not one incident per variant it checks.
    ExportJob,
    /// A document-wide refusal with no narrower subject.
    Project,
    /// The chat panel's own refusals.
    Agent,
}

impl IncidentSubject {
    /// A short human label for a card or a log line, for example `Asset 1`;
    /// when the incident carries `subject_name`, the card renders it beside
    /// this label.
    ///
    /// The asset's human *name* is deliberately not on the incident: an
    /// incident carries no document view (IN1 §2.3c rule 31, §13 D15,
    /// `IN1b` §13 D-B6).
    #[must_use]
    pub fn label(self) -> String {
        match self {
            Self::Asset(asset) => format!("Asset {asset}"),
            Self::LutAsset(lut) => format!("Look {lut}"),
            Self::Clip(clip) => format!("Clip {clip}"),
            Self::Track(track) => format!("Track {track}"),
            Self::Chain(AudioChain::Bus(bus)) => format!("Bus {bus}"),
            Self::Chain(AudioChain::Master) => "Master".to_owned(),
            Self::ExportJob => "Export".to_owned(),
            Self::Project => "Project".to_owned(),
            Self::Agent => "Agent".to_owned(),
        }
    }
}

/// The typed facts the incident was opened from (`IN1b` §3.4 rule 23).
///
/// **Evidence follows the code, by rule** (`IN1b` §3.4 rule 24): where a
/// producer resolves its code at run time, the evidence variant is resolved
/// with it, and a delegating variant never falls back to a `reason` string.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IncidentEvidence {
    /// A CC1 source-colour refusal on the managed decode path.
    ///
    /// Part A's variant, byte-identical on the wire: its name, both field
    /// names and the container's `rename_all` are load-bearing and must not
    /// change (`IN1b` §3.11 rule 41).
    SourceColor {
        /// The description the incident captured at open time. The revert
        /// restores exactly these bytes (IN1 §0.1 N1.5/B4).
        probed: ColorDescription,
        /// The explicit profile assumption in force when the decode was
        /// refused, if there was one.
        assumption: Option<ColorSourceProfileAssumption>,
    },
    /// A refusal whose only typed facts are already in `observed`/`allowed`.
    Plain,
    /// A rejected edit, with the family so a consumer can branch without
    /// re-parsing the message.
    OpError {
        /// The family the inner [`crate::OpError`] belongs to.
        family: IncidentFamily,
        /// The one-based operation number, when the refusal came from a batch.
        op_number: Option<usize>,
        /// The rendered rejection.
        message: String,
    },
    /// A typed media failure that is not a source-colour one.
    MediaError {
        /// [`MediaError::recovery_code`]'s answer, or the incident's own code
        /// when the failure carries none. Owned so the evidence deserialises
        /// whole (`IN2B` §0.4 d13); wire-identical, `str` and `String`
        /// serialise the same.
        code: String,
        /// The rendered failure.
        message: String,
    },
    /// A revision gate that refused (`IN1b` §4).
    Revision {
        /// The revision the send was planned against.
        expected: TimelineRevision,
        /// The revision the document was actually on.
        actual: TimelineRevision,
    },
    /// A Source-monitor edit refused before anything was applied.
    SourceEdit {
        /// The app's own rendered reason. Owned so the evidence deserialises
        /// whole (`IN2B` §0.4 d13); wire-identical.
        reason: String,
    },
    /// A relink candidate refused.
    Relink {
        /// The app's own rendered reason.
        reason: String,
    },
    /// A project file that could not be written.
    ProjectSave {
        /// The app's own rendered reason.
        reason: String,
    },
    /// An isolated agent branch that refused.
    Branch {
        /// The agent crate's own rendered reason.
        reason: String,
    },
    /// A caption plan that could not be built.
    CaptionPlan {
        /// Core's own rendered reason.
        reason: String,
    },
    /// A delivery variant that could not be built.
    DeliveryVariant {
        /// The rendered reason.
        reason: String,
    },
}

impl IncidentEvidence {
    /// The probed description this evidence captured, when it captured one.
    ///
    /// `Option`, not a reference, because evidence is per-variant after Part B
    /// and an `operation_bounds` incident on a clip has no probed colour
    /// description at all (`IN1b` §3.4 rule 23).
    #[must_use]
    pub const fn probed(&self) -> Option<&ColorDescription> {
        match self {
            Self::SourceColor { probed, .. } => Some(probed),
            Self::Plain
            | Self::OpError { .. }
            | Self::MediaError { .. }
            | Self::Revision { .. }
            | Self::SourceEdit { .. }
            | Self::Relink { .. }
            | Self::ProjectSave { .. }
            | Self::Branch { .. }
            | Self::CaptionPlan { .. }
            | Self::DeliveryVariant { .. } => None,
        }
    }
}

/// How an incident ended.
///
/// `Approved` and `Rejected` are not IN1 outcomes; they arrive with the typed
/// broker (IN1 §6.3 rule 17, §13 D6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IncidentOutcome {
    /// The recovery was applied.
    Applied,
    /// An applied recovery was taken back.
    Reverted,
    /// Nothing was applied; the person was told what the problem is.
    Explained,
    /// The person refused the session's proposal.
    ///
    /// An approved proposal that lands is [`Self::Applied`], which the document
    /// shows (IN2 §0.1 N1/Q7). A session that ended **without** a proposal the
    /// person refused does not produce this outcome — it returns the incident
    /// to [`IncidentState::Open`] (IN2 §3.7, §4.5 rule 20).
    Rejected,
}

/// Whether the incident is still outstanding.
///
/// Adjacently tagged so it serialises as a map and can therefore be
/// `#[serde(flatten)]`ed into [`Incident`], rendering as the two top-level keys
/// `"state":"resolved","outcome":"applied"` — and as the single key
/// `"state":"open"` while the incident is open (IN1 §2.5 rule 43).
///
/// `Investigating` has the same single-key shape, `"state":"investigating"`,
/// and is declared between `Open` and `Resolved` (IN2 §3.6 rule 32).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "state", content = "outcome")]
pub enum IncidentState {
    /// Outstanding.
    Open,
    /// Outstanding, with an investigator session running against it
    /// (IN2 §3.6 rule 32).
    Investigating,
    /// Ended, with the outcome that ended it.
    Resolved(IncidentOutcome),
}

impl IncidentState {
    /// Whether the incident is still outstanding. An incident under
    /// investigation is unresolved: the work is happening, not finished
    /// (IN2 §3.6 rule 33).
    #[must_use]
    pub const fn is_open(self) -> bool {
        matches!(self, Self::Open | Self::Investigating)
    }
}

/// What resolving an incident cost.
///
/// Every `Option` field skips when `None`, so an unresolved incident costs one
/// wire key rather than eight keys of `null` (IN1 §8 rule 4).
///
/// **Not [`Copy`]** since IN2 §5.4 rule 12: [`IncidentResolver::Session`]
/// carries owned strings. Probe-2 measured the fallout at zero sites.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct IncidentTelemetry {
    /// Wall time from observation to resolution, measured at the router.
    ///
    /// Served on the wire while set (IN1 §2.5) but never in a sidecar record:
    /// it is log-origin-relative, meaningless across runs, so the record
    /// builder clears it and restore reads `None` until the incident is
    /// resolved again (`IN2B` §2 rule 3, N-9; N4 F1).
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
    /// Turns the resolving session spent, when a session resolved it.
    ///
    /// Skips when absent so `IN1_INCIDENT_SERIALIZED_BYTES` cannot move: a bare
    /// `u32` beside `tool_calls` would add `,"turns":0` to **every** incident
    /// (IN2 §5.4 rule 12).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turns: Option<u32>,
    /// Who resolved it, when it was not the router; or why a session stopped
    /// without resolving it (IN2 §0.4 p).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolver: Option<IncidentResolver>,
}

/// Who resolved an incident, or why the session working on it stopped
/// (IN2 §5.4 rule 12).
///
/// A router-resolved incident costs **zero** extra keys, which is IN1 §8
/// rule 4's shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IncidentResolver {
    /// The deterministic router applied the recovery itself.
    Router,
    /// The person pressed a card action.
    Person,
    /// An investigator session.
    Session {
        /// The `HarnessId` string the session ran on.
        harness: String,
        /// The model, when one was configured.
        model: Option<String>,
        /// The pump's stop, or the reason the session was ended. A `String`
        /// rather than a `&'static str` because `StopReason::Harness` and
        /// `StopReason::Observer` carry a message (IN2 §5.4 rule 12).
        stop: String,
    },
}

/// The ceiling, in bytes of **JSON-escaped** output, on each of
/// [`IncidentProposal`]'s two free-text fields (IN2 §4.1 rule 5).
pub const INVESTIGATOR_EXPLANATION_CEILING_BYTES: usize = 240;

/// The most operations a branch may carry and still be proposable
/// (IN2 §4.1 rule 6 code 4).
pub const INVESTIGATOR_MAX_PROPOSAL_OPERATIONS: usize = 8;

/// The JSON-escaped length of `text`, in bytes, **excluding** the surrounding
/// quotes — that is, `serde_json::to_string(text).len() - 2`.
///
/// Computed here rather than through `serde_json` because core does not depend
/// on it outside `[dev-dependencies]`. `serde_json`'s default escape table is
/// exactly `"`, `\` and the C0 controls; `\b`, `\t`, `\n`, `\f` and `\r` take
/// two bytes and every other control takes six as `\u00XX`. Nothing above
/// `0x1F` is escaped, so UTF-8 passes through byte for byte. The unit test
/// `in2_both_proposal_strings_are_capped_on_their_serialised_length` asserts
/// this function against `serde_json` itself.
const fn json_escaped_byte_len(byte: u8) -> usize {
    match byte {
        b'"' | b'\\' | 0x08 | 0x09 | 0x0a | 0x0c | 0x0d => 2,
        0x00..=0x1f => 6,
        _ => 1,
    }
}

/// The JSON-escaped length of `text`, in bytes, excluding the quotes.
#[must_use]
pub fn json_escaped_len(text: &str) -> usize {
    text.bytes().map(json_escaped_byte_len).sum()
}

/// The largest index `<= index` that is a `char` boundary of `text`.
fn floor_char_boundary(text: &str, index: usize) -> usize {
    if index >= text.len() {
        return text.len();
    }
    let mut boundary = index;
    while boundary > 0 && !text.is_char_boundary(boundary) {
        boundary -= 1;
    }
    boundary
}

/// Truncate `text` so that its **serialised** length is at most `ceiling`
/// bytes (IN2 §4.1 rule 5).
///
/// JSON escaping is not length-preserving — 240 bytes of control characters
/// serialise to about 1 440 — and `IN1_INCIDENT_SERIALIZED_CEILING_BYTES` is
/// asserted over strings a **model** supplies in tests, not production code.
/// Capping the raw length would therefore be a bet on which 240 bytes arrive;
/// capping the serialised length is a property.
///
/// Each pass shrinks proportionally and then backs off to a `char` boundary, so
/// the loop strictly shrinks and terminates; the empty string escapes to zero.
#[must_use]
pub fn truncate_to_serialized_bytes(text: &str, ceiling: usize) -> String {
    let mut candidate = text;
    loop {
        let escaped = json_escaped_len(candidate);
        if escaped <= ceiling {
            return candidate.to_owned();
        }
        let proportional = candidate.len().saturating_mul(ceiling) / escaped;
        let target = proportional.min(candidate.len().saturating_sub(1));
        candidate = &candidate[..floor_char_boundary(candidate, target)];
    }
}

/// Why [`IncidentLog::record_proposal`] refused (IN2 erratum A-R7).
///
/// Each variant maps one-to-one onto one of `propose_fix`'s six refusal codes
/// (IN2 §4.1 rule 6), which is why this is a typed error rather than the `bool`
/// rule 7 spells: a caller that must answer `incident_not_found`,
/// `incident_not_investigating` and `proposal_already_recorded` separately
/// cannot do it from one `false`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RecordProposalError {
    /// No such id in the log — `incident_not_found` (code 1).
    #[error("no incident with that id")]
    NotFound,
    /// The incident's state is not `Investigating` — the person or the router
    /// got there first (`incident_not_investigating`, code 2).
    #[error("the incident is not under investigation")]
    NotInvestigating,
    /// The incident already carries a proposal that is not stale —
    /// `proposal_already_recorded` (code 6).
    #[error("the incident already carries a proposal")]
    AlreadyRecorded,
}

/// A typed fix an investigator session proposes for one incident (IN2 §4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IncidentProposal {
    /// The branch's applied operations, in order. **Never serialised**:
    /// [`Operation`] has 57 variants and several carry unbounded payloads, so a
    /// `Vec<Operation>` on the wire would end
    /// `IN1_INCIDENT_SERIALIZED_CEILING_BYTES` as a property rather than move
    /// it (IN2 §0.1 N2/c). The app reads them through the shared log, in
    /// process, which is the only consumer there is.
    #[serde(skip)]
    pub operations: Vec<Operation>,
    /// How many there are (IN2 §0.4 l). What the card's headline reads when
    /// `summary` has been truncated.
    pub operation_count: usize,
    /// One line per operation, at most
    /// [`INVESTIGATOR_EXPLANATION_CEILING_BYTES`] bytes of JSON.
    pub summary: String,
    /// One sentence a person can act on, at most
    /// [`INVESTIGATOR_EXPLANATION_CEILING_BYTES`] bytes of JSON.
    pub explanation: String,
    /// The **live** revision the branch was seeded at by [`crate::Core::spawn_at`].
    ///
    /// The field a reader needs is *what live looked like when this was
    /// proved*, which is comparable with the live revision at approval time.
    /// The branch's own counter is private to a core nobody outside the session
    /// can query and is not put on the wire (IN2 §4.2 rule 9).
    pub base_revision: TimelineRevision,
    /// Set by the app when a merge has conflicted twice (IN2 §4.4 rule 18), and
    /// by Part B on **every** proposal it loads: `operations` is
    /// `#[serde(skip)]`, so a deserialised proposal is always stale
    /// (IN2 §4.2 rule 10).
    pub stale: bool,
}

/// The serialised-bytes ceiling for an observed subject name (`IN2B` §0.4
/// d15): `observe` truncates through [`truncate_to_serialized_bytes`], so
/// the wire arm is at most `,"subject_name":"<64>"` = 82 B.
pub const SUBJECT_NAME_CEILING_BYTES: usize = 64;

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
    /// The subject's name at observe time, truncated to
    /// [`SUBJECT_NAME_CEILING_BYTES`] serialised bytes (`IN2B` §9 rule 5).
    /// Captured at open and kept — dedup never refreshes it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject_name: Option<String>,
    /// Copied from the opening observation (`IN2B` §3 rule 12). Skipped on
    /// the wire: provenance decides the write, never the record.
    #[serde(skip)]
    pub transient: bool,
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
    /// The session's proposal, when one has been recorded (IN2 §4.2 rule 8).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proposal: Option<IncidentProposal>,
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
    /// The subject's name at observe time, when the router captured one
    /// (`IN2B` §9 rule 5, d19). Every builder sets `None`; the router sets
    /// `Some` before the funnel call, and `observe` truncates and stores.
    pub name: Option<String>,
    /// Whether the note describes run-local state rather than history
    /// (`IN2B` §9 rule 5, N2/B-3). Set at note time by the §7 drains, the
    /// recovery modals, the open-time aggregates and every §5 note;
    /// `observe` copies it onto the [`Incident`], and `records()` drops
    /// open entries with the bit set.
    pub transient: bool,
}

impl IncidentObservation {
    /// An observation whose only typed facts are the ones it already carries.
    ///
    /// The shape every label placeholder uses: a code, a subject, the message
    /// the site had, and [`IncidentEvidence::Plain`].
    #[must_use]
    pub fn plain(
        code: IncidentCode,
        subject: IncidentSubject,
        observed: impl Into<String>,
        revision: TimelineRevision,
    ) -> Self {
        Self {
            code,
            subject,
            observed: observed.into(),
            allowed: None,
            evidence: IncidentEvidence::Plain,
            revision,
            name: None,
            transient: false,
        }
    }

    /// An observation from a rejected edit (`IN1b` §5.2 rule 15).
    ///
    /// The `subject` parameter stays, because a caller that knows better than
    /// the operation keeps the right to say so; what changes is that no caller
    /// has to invent one — [`Operation::incident_subject`] derives it.
    #[must_use]
    pub fn from_op_error(
        error: &OpError,
        subject: IncidentSubject,
        revision: TimelineRevision,
    ) -> Self {
        Self {
            code: error.incident_code(),
            subject,
            observed: error.to_string(),
            allowed: None,
            evidence: IncidentEvidence::OpError {
                family: error.incident_family(),
                op_number: None,
                message: error.to_string(),
            },
            revision,
            name: None,
            transient: false,
        }
    }

    /// An observation from a refused edit plan (`IN1b` §3.4 rule 24).
    ///
    /// Evidence follows the code: [`BatchError::Empty`] is `edit_plan_rejected`
    /// with [`IncidentFamily::Malformed`] evidence, because a plan with no
    /// operations is a malformed plan; [`BatchError::OperationFailed`]
    /// delegates both its code and its family to the inner rejection and never
    /// falls back to a `reason` string.
    #[must_use]
    pub fn from_batch_error(
        error: &BatchError,
        subject: IncidentSubject,
        revision: TimelineRevision,
    ) -> Self {
        let (code, family, op_number) = match error {
            BatchError::Empty => (
                IncidentCode::Rejection(RejectionIncident::EditPlan),
                IncidentFamily::Malformed,
                None,
            ),
            BatchError::OperationFailed { op_number, error } => (
                error.incident_code(),
                error.incident_family(),
                Some(*op_number),
            ),
        };
        Self {
            code,
            subject,
            observed: error.to_string(),
            allowed: None,
            evidence: IncidentEvidence::OpError {
                family,
                op_number,
                message: error.to_string(),
            },
            revision,
            name: None,
            transient: false,
        }
    }

    /// An observation from a revision gate that refused (`IN1b` §4).
    ///
    /// The incident is stated against the revision the document is *actually*
    /// on, which is the revision the person must make the edit against now.
    #[must_use]
    pub fn revision_conflict(
        subject: IncidentSubject,
        expected: TimelineRevision,
        actual: TimelineRevision,
    ) -> Self {
        Self {
            code: IncidentCode::EditRevisionConflict,
            subject,
            observed: actual.to_string(),
            allowed: Some(expected.to_string()),
            evidence: IncidentEvidence::Revision { expected, actual },
            revision: actual,
            name: None,
            transient: false,
        }
    }

    /// Build an observation from a typed media failure.
    ///
    /// **Total** after `IN1b` §3.9: every [`MediaError`] variant has a code, so
    /// the constructor returns an observation rather than an `Option` and the
    /// app's `else` arm disappears rather than being migrated
    /// (`IN1b` §5.1 rule 12, erratum `IN1b`-R9). The match carries **no**
    /// wildcard arm, so a new variant breaks the build and forces a decision
    /// (IN1 §2.3b rule 22).
    ///
    /// `subject` is the fallback the caller supplies for a failure that names
    /// none; [`MediaError::SourceColorForAsset`] overrides it with the asset it
    /// carries, which is the only variant that knows its own subject.
    #[must_use]
    #[allow(
        clippy::too_many_lines,
        reason = "one arm per MediaError variant; IN1 §2.3b rule 22 requires \
                  the match be exhaustive with no wildcard, so the length is \
                  the contract's and splitting it would hide the \
                  exhaustiveness the type system is checking"
    )]
    pub fn from_media_error(
        error: &MediaError,
        subject: IncidentSubject,
        revision: TimelineRevision,
    ) -> Self {
        match error {
            MediaError::SourceColorForAsset(refusal) => Self {
                code: IncidentCode::SourceColor(SourceColorIncident::from_source_error(
                    &refusal.error,
                )),
                subject: IncidentSubject::Asset(refusal.asset),
                observed: refusal.error.observed(),
                allowed: Some(refusal.error.allowed_values().to_owned()),
                evidence: IncidentEvidence::SourceColor {
                    probed: refusal.description.clone(),
                    assumption: refusal.assumption,
                },
                revision,
                name: None,
                transient: false,
            },
            // The bare refusal with no subject. `contextual_managed_decode_error`
            // turns every one of these into a `SourceColorForAsset` before it
            // leaves `kinewright-media`, so the arm exists to keep the match
            // honest rather than because the path is reachable
            // (IN1 §2.3b rule 23); the caller's fallback subject is used and
            // the evidence is the media one, because there is no probed
            // description to carry.
            MediaError::SourceColor(inner) => Self {
                code: IncidentCode::SourceColor(SourceColorIncident::from_source_error(inner)),
                subject,
                observed: inner.observed(),
                allowed: Some(inner.allowed_values().to_owned()),
                evidence: media_evidence(
                    error,
                    IncidentCode::SourceColor(SourceColorIncident::from_source_error(inner)),
                ),
                revision,
                name: None,
                transient: false,
            },
            // `observed` is the whole rendered refusal and `allowed` is `None`,
            // matching the `Backend` arm below rather than half-splitting the
            // template: the `#[error]` string already names the path, the
            // format, both depths and the reason, and `reason` is a sentence
            // about the format rather than the set of formats that would have
            // been accepted, so putting it under `allowed` would mislabel it.
            MediaError::UnsupportedDecoderFormat { .. } => {
                let code = IncidentCode::Media(MediaIncident::UnsupportedDecoderFormat);
                Self {
                    code,
                    subject,
                    observed: error.to_string(),
                    allowed: None,
                    evidence: media_evidence(error, code),
                    revision,
                    name: None,
                    transient: false,
                }
            }
            MediaError::DeliveryColor(inner) => {
                let code =
                    IncidentCode::DeliveryColor(DeliveryColorIncident::from_delivery_error(inner));
                Self {
                    code,
                    subject,
                    observed: inner.observed(),
                    allowed: Some(inner.allowed_values()),
                    evidence: media_evidence(error, code),
                    revision,
                    name: None,
                    transient: false,
                }
            }
            MediaError::DeliveryVerification(inner) => {
                let code = IncidentCode::DeliveryVerification(
                    DeliveryVerificationIncident::from_verification_error(inner),
                );
                Self {
                    code,
                    subject,
                    observed: inner.observed(),
                    allowed: Some(inner.allowed_values()),
                    evidence: media_evidence(error, code),
                    revision,
                    name: None,
                    transient: false,
                }
            }
            MediaError::ColorQc(inner) => {
                let code = IncidentCode::ColorQc(ColorQcIncident::from_qc_error(inner));
                Self {
                    code,
                    subject,
                    observed: inner.observed(),
                    allowed: Some(inner.allowed_values()),
                    evidence: media_evidence(error, code),
                    revision,
                    name: None,
                    transient: false,
                }
            }
            // MO2 R8: the render entry's document rejection delegates its code
            // and evidence to the wrapped `OpError`, exactly as
            // `DeliveryVariantError::InvalidDocument` does; it mints no code.
            MediaError::InvalidDocument(inner) => Self {
                code: inner.incident_code(),
                subject,
                observed: error.to_string(),
                allowed: None,
                evidence: IncidentEvidence::OpError {
                    family: inner.incident_family(),
                    op_number: None,
                    message: inner.to_string(),
                },
                revision,
                name: None,
                transient: false,
            },
            // The two matte enums and the two stores mint **no** `IncidentCode`
            // (`IN1b` §0.2/e, §3.9 rule 37, and the code table of §3.2 rule 12,
            // which declares none of their 21 + 10 strings), so they share the
            // unclassified media code with the seven code-less variants — the
            // five `IN1b` ones, `Scope`, whose `recovery_code` is `None`
            // (`IN2B` §6 rule 2, d16), and MO2's `NonFiniteRender` (R10) — and
            // so does their evidence, because one incident carries one code. Their own code is not lost: it is the
            // first token of every one of their rendered refusals and therefore
            // the first word of `observed`.
            MediaError::MatteProof(_)
            | MediaError::MatteCoverage(_)
            | MediaError::Scope(_)
            | MediaError::Store { .. }
            | MediaError::NotImplemented
            | MediaError::Cancelled
            | MediaError::MixSpectrumRangeTooShort { .. }
            | MediaError::MixLoudnessRangeTooShort { .. }
            | MediaError::NonFiniteRender { .. }
            | MediaError::Backend(_) => {
                let code = IncidentCode::Media(MediaIncident::BackendUnclassified);
                Self {
                    code,
                    subject,
                    observed: error.to_string(),
                    allowed: None,
                    evidence: media_evidence(error, code),
                    revision,
                    name: None,
                    transient: false,
                }
            }
        }
    }
}

/// [`IncidentEvidence::MediaError`] for one failure.
///
/// **One incident carries one code.** The evidence's `code` is the incident's
/// own, never the engine's `recovery_code()`: an incident whose `code` reads
/// `media_backend_unclassified` while its evidence read `matte_proof_no_matte`
/// would give an agent two answers to one question, which is the shape IN1
/// exists to remove (review-2 S2). For every variant that declares a code of
/// its own the two strings are equal anyway — asserted by
/// `in1b_from_media_error_is_total_and_keeps_the_asset_scoped_subject` — and
/// for the two matte variants and the six code-less ones the engine's own
/// token survives as the first word of `observed`.
fn media_evidence(error: &MediaError, code: IncidentCode) -> IncidentEvidence {
    IncidentEvidence::MediaError {
        code: code.code().to_owned(),
        message: error.to_string(),
    }
}

impl CaptionPlanError {
    /// The incident code this caption-plan refusal carries
    /// (`IN1b` §3.10 rule 39).
    ///
    /// Two of the six variants take `operation_internal` instead of
    /// `caption_plan_rejected`: `TrackIdExhausted` and `ClipIdExhausted` are
    /// id-space exhaustion, and `caption_plan_rejected`'s body — *"adjust the
    /// transcript selection or the script"* — is false for them. This is the
    /// hybrid code rule used the way [`OpError::incident_code`] uses it for the
    /// two LUT-asset variants, and it mints no code.
    ///
    /// Declared here rather than in `captions.rs` so that `IN1b` §14 row A's
    /// "`src/captions.rs` — read, not edited" stays true.
    #[must_use]
    pub const fn incident_code(&self) -> IncidentCode {
        match self {
            Self::TrackIdExhausted | Self::ClipIdExhausted => {
                IncidentCode::Operation(IncidentFamily::Internal)
            }
            Self::NoCues
            | Self::EmptyAuthoredScript
            | Self::AuthoredScriptAlignment
            | Self::InvalidCueDuration => IncidentCode::Rejection(RejectionIncident::CaptionPlan),
        }
    }

    /// An observation from this refusal, with the caller's subject.
    #[must_use]
    pub fn incident_observation(
        &self,
        subject: IncidentSubject,
        revision: TimelineRevision,
    ) -> IncidentObservation {
        IncidentObservation {
            code: self.incident_code(),
            subject,
            observed: self.to_string(),
            allowed: None,
            evidence: IncidentEvidence::CaptionPlan {
                reason: self.to_string(),
            },
            revision,
            name: None,
            transient: false,
        }
    }
}

impl DeliveryVariantError {
    /// The incident code this delivery-variant refusal carries
    /// (`IN1b` §3.4 rule 24).
    ///
    /// `InvalidDocument(OpError)` **delegates** its code to the inner
    /// rejection, exactly as `BranchError::InvalidBase` does: the reason the
    /// variant could not be built is the document rejection, and telling the
    /// person "adjust the focus percentages" for a duplicate clip id would be
    /// false. The other two arms are the variant's own refusal.
    ///
    /// Declared here rather than in `delivery.rs` so that `IN1b` §14 row A's
    /// "`src/delivery.rs` — read, not edited" stays true.
    #[must_use]
    pub const fn incident_code(&self) -> IncidentCode {
        match self {
            Self::InvalidDocument(error) => error.incident_code(),
            Self::InvalidFocus { .. } | Self::EffectIdExhausted => {
                IncidentCode::Rejection(RejectionIncident::DeliveryVariant)
            }
        }
    }

    /// An observation from this refusal, with the caller's subject.
    ///
    /// Evidence follows the code (`IN1b` §3.4 rule 24): the delegating arm
    /// supplies [`IncidentEvidence::OpError`] with the inner family and
    /// **never** a `reason` string.
    #[must_use]
    pub fn incident_observation(
        &self,
        subject: IncidentSubject,
        revision: TimelineRevision,
    ) -> IncidentObservation {
        let evidence = match self {
            Self::InvalidDocument(error) => IncidentEvidence::OpError {
                family: error.incident_family(),
                op_number: None,
                message: error.to_string(),
            },
            Self::InvalidFocus { .. } | Self::EffectIdExhausted => {
                IncidentEvidence::DeliveryVariant {
                    reason: self.to_string(),
                }
            }
        };
        IncidentObservation {
            code: self.incident_code(),
            subject,
            observed: self.to_string(),
            allowed: None,
            evidence,
            revision,
            name: None,
            transient: false,
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

/// The declared policy for every [`IncidentCode`], one row per code, in the
/// enum's declaration order (`IN1b` §3.6 rule 28).
///
/// **74 rows**: 3 `AutoApply`, 0 `AskFirst`, 71 `Explain`; 57 `Blocks`,
/// 17 `Degrades`, 0 `Informs`. A row's severity answers one question — *did
/// the thing the person asked for happen?* — derived from the site's own
/// control flow and declared once per code (`IN1b` §3.6 rule 30). The seventeen
/// `Degrades` rows are the five delivery-verification rows (the export **was**
/// written), the six colour-QC rows (nothing was mutated),
/// `look_incomplete`/`media_incomplete` (the project opened or saved without
/// all of its looks or media), and the four `IN2B` §5 project/sidecar rows
/// (the project opened or saved — only the history or the save-into-downgrade
/// degraded). `Informs` has no Part B row and stays declared for IN3.
///
/// IN1 §9 clause 1's "every row `Blocks`" is superseded by erratum `IN1b`-R1a:
/// it was true of a twelve-row table in which every row stopped a managed
/// decode, and is false of a seventy-four-row table in which seventeen rows
/// report work that completed with a loss.
pub const POLICY: [PolicyEntry; 74] = [
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
    PolicyEntry {
        code: IncidentCode::SourceColor(SourceColorIncident::UnknownWhitePoint),
        class: PolicyClass::Explain,
        predicate: PolicyPredicate::Always,
        severity: IncidentSeverity::Blocks,
    },
    PolicyEntry {
        code: IncidentCode::Media(MediaIncident::UnsupportedDecoderFormat),
        class: PolicyClass::Explain,
        predicate: PolicyPredicate::Always,
        severity: IncidentSeverity::Blocks,
    },
    PolicyEntry {
        code: IncidentCode::Media(MediaIncident::BackendUnclassified),
        class: PolicyClass::Explain,
        predicate: PolicyPredicate::Always,
        severity: IncidentSeverity::Blocks,
    },
    PolicyEntry {
        code: IncidentCode::DeliveryColor(DeliveryColorIncident::UnsupportedCodec),
        class: PolicyClass::Explain,
        predicate: PolicyPredicate::Always,
        severity: IncidentSeverity::Blocks,
    },
    PolicyEntry {
        code: IncidentCode::DeliveryColor(DeliveryColorIncident::UnsupportedField),
        class: PolicyClass::Explain,
        predicate: PolicyPredicate::Always,
        severity: IncidentSeverity::Blocks,
    },
    PolicyEntry {
        code: IncidentCode::DeliveryColor(DeliveryColorIncident::PixelFormatDepthMismatch),
        class: PolicyClass::Explain,
        predicate: PolicyPredicate::Always,
        severity: IncidentSeverity::Blocks,
    },
    PolicyEntry {
        code: IncidentCode::DeliveryColor(DeliveryColorIncident::EncoderPixelFormatUnavailable),
        class: PolicyClass::Explain,
        predicate: PolicyPredicate::Always,
        severity: IncidentSeverity::Blocks,
    },
    PolicyEntry {
        code: IncidentCode::DeliveryVerification(DeliveryVerificationIncident::NotFullResolution),
        class: PolicyClass::Explain,
        predicate: PolicyPredicate::Always,
        severity: IncidentSeverity::Degrades,
    },
    PolicyEntry {
        code: IncidentCode::DeliveryVerification(DeliveryVerificationIncident::PlaneOutOfContainer),
        class: PolicyClass::Explain,
        predicate: PolicyPredicate::Always,
        severity: IncidentSeverity::Degrades,
    },
    PolicyEntry {
        code: IncidentCode::DeliveryVerification(DeliveryVerificationIncident::FrameCountMismatch),
        class: PolicyClass::Explain,
        predicate: PolicyPredicate::Always,
        severity: IncidentSeverity::Degrades,
    },
    PolicyEntry {
        code: IncidentCode::DeliveryVerification(
            DeliveryVerificationIncident::FrameCountOutOfRange,
        ),
        class: PolicyClass::Explain,
        predicate: PolicyPredicate::Always,
        severity: IncidentSeverity::Degrades,
    },
    PolicyEntry {
        code: IncidentCode::DeliveryVerification(DeliveryVerificationIncident::BudgetLaneMismatch),
        class: PolicyClass::Explain,
        predicate: PolicyPredicate::Always,
        severity: IncidentSeverity::Degrades,
    },
    PolicyEntry {
        code: IncidentCode::ColorQc(ColorQcIncident::ProxyProofRefused),
        class: PolicyClass::Explain,
        predicate: PolicyPredicate::Always,
        severity: IncidentSeverity::Degrades,
    },
    PolicyEntry {
        code: IncidentCode::ColorQc(ColorQcIncident::RasterLengthMismatch),
        class: PolicyClass::Explain,
        predicate: PolicyPredicate::Always,
        severity: IncidentSeverity::Degrades,
    },
    PolicyEntry {
        code: IncidentCode::ColorQc(ColorQcIncident::EmptyPopulation),
        class: PolicyClass::Explain,
        predicate: PolicyPredicate::Always,
        severity: IncidentSeverity::Degrades,
    },
    PolicyEntry {
        code: IncidentCode::ColorQc(ColorQcIncident::NodeBudgetExceeded),
        class: PolicyClass::Explain,
        predicate: PolicyPredicate::Always,
        severity: IncidentSeverity::Degrades,
    },
    PolicyEntry {
        code: IncidentCode::ColorQc(ColorQcIncident::MatteRegionRasterMismatch),
        class: PolicyClass::Explain,
        predicate: PolicyPredicate::Always,
        severity: IncidentSeverity::Degrades,
    },
    PolicyEntry {
        code: IncidentCode::ColorQc(ColorQcIncident::NodeRemovalRejected),
        class: PolicyClass::Explain,
        predicate: PolicyPredicate::Always,
        severity: IncidentSeverity::Degrades,
    },
    PolicyEntry {
        code: IncidentCode::Operation(IncidentFamily::Bounds),
        class: PolicyClass::Explain,
        predicate: PolicyPredicate::Always,
        severity: IncidentSeverity::Blocks,
    },
    PolicyEntry {
        code: IncidentCode::Operation(IncidentFamily::Malformed),
        class: PolicyClass::Explain,
        predicate: PolicyPredicate::Always,
        severity: IncidentSeverity::Blocks,
    },
    PolicyEntry {
        code: IncidentCode::Operation(IncidentFamily::Duplicate),
        class: PolicyClass::Explain,
        predicate: PolicyPredicate::Always,
        severity: IncidentSeverity::Blocks,
    },
    PolicyEntry {
        code: IncidentCode::Operation(IncidentFamily::Placement),
        class: PolicyClass::Explain,
        predicate: PolicyPredicate::Always,
        severity: IncidentSeverity::Blocks,
    },
    PolicyEntry {
        code: IncidentCode::Operation(IncidentFamily::Missing),
        class: PolicyClass::Explain,
        predicate: PolicyPredicate::Always,
        severity: IncidentSeverity::Blocks,
    },
    PolicyEntry {
        code: IncidentCode::Operation(IncidentFamily::Structure),
        class: PolicyClass::Explain,
        predicate: PolicyPredicate::Always,
        severity: IncidentSeverity::Blocks,
    },
    PolicyEntry {
        code: IncidentCode::Operation(IncidentFamily::Relink),
        class: PolicyClass::Explain,
        predicate: PolicyPredicate::Always,
        severity: IncidentSeverity::Blocks,
    },
    PolicyEntry {
        code: IncidentCode::Operation(IncidentFamily::Unrepresentable),
        class: PolicyClass::Explain,
        predicate: PolicyPredicate::Always,
        severity: IncidentSeverity::Blocks,
    },
    PolicyEntry {
        code: IncidentCode::Operation(IncidentFamily::UnknownName),
        class: PolicyClass::Explain,
        predicate: PolicyPredicate::Always,
        severity: IncidentSeverity::Blocks,
    },
    PolicyEntry {
        code: IncidentCode::Operation(IncidentFamily::Internal),
        class: PolicyClass::Explain,
        predicate: PolicyPredicate::Always,
        severity: IncidentSeverity::Blocks,
    },
    PolicyEntry {
        code: IncidentCode::Operation(IncidentFamily::ColorPolicy),
        class: PolicyClass::Explain,
        predicate: PolicyPredicate::Always,
        severity: IncidentSeverity::Blocks,
    },
    PolicyEntry {
        code: IncidentCode::LutAssetPolicy,
        class: PolicyClass::Explain,
        predicate: PolicyPredicate::Always,
        severity: IncidentSeverity::Blocks,
    },
    PolicyEntry {
        code: IncidentCode::EditRevisionConflict,
        class: PolicyClass::Explain,
        predicate: PolicyPredicate::Always,
        severity: IncidentSeverity::Blocks,
    },
    PolicyEntry {
        code: IncidentCode::Rejection(RejectionIncident::EditPlan),
        class: PolicyClass::Explain,
        predicate: PolicyPredicate::Always,
        severity: IncidentSeverity::Blocks,
    },
    PolicyEntry {
        code: IncidentCode::Rejection(RejectionIncident::DeliveryVariant),
        class: PolicyClass::Explain,
        predicate: PolicyPredicate::Always,
        severity: IncidentSeverity::Blocks,
    },
    PolicyEntry {
        code: IncidentCode::Rejection(RejectionIncident::AgentBranch),
        class: PolicyClass::Explain,
        predicate: PolicyPredicate::Always,
        severity: IncidentSeverity::Blocks,
    },
    PolicyEntry {
        code: IncidentCode::Rejection(RejectionIncident::SourceEdit),
        class: PolicyClass::Explain,
        predicate: PolicyPredicate::Always,
        severity: IncidentSeverity::Blocks,
    },
    PolicyEntry {
        code: IncidentCode::Rejection(RejectionIncident::Relink),
        class: PolicyClass::Explain,
        predicate: PolicyPredicate::Always,
        severity: IncidentSeverity::Blocks,
    },
    PolicyEntry {
        code: IncidentCode::Rejection(RejectionIncident::ProjectSave),
        class: PolicyClass::Explain,
        predicate: PolicyPredicate::Always,
        severity: IncidentSeverity::Blocks,
    },
    PolicyEntry {
        code: IncidentCode::Rejection(RejectionIncident::CaptionPlan),
        class: PolicyClass::Explain,
        predicate: PolicyPredicate::Always,
        severity: IncidentSeverity::Blocks,
    },
    PolicyEntry {
        code: IncidentCode::Label(LabelIncident::Operations),
        class: PolicyClass::Explain,
        predicate: PolicyPredicate::Always,
        severity: IncidentSeverity::Blocks,
    },
    PolicyEntry {
        code: IncidentCode::Label(LabelIncident::Look),
        class: PolicyClass::Explain,
        predicate: PolicyPredicate::Always,
        severity: IncidentSeverity::Blocks,
    },
    PolicyEntry {
        code: IncidentCode::Label(LabelIncident::LookIncomplete),
        class: PolicyClass::Explain,
        predicate: PolicyPredicate::Always,
        severity: IncidentSeverity::Degrades,
    },
    PolicyEntry {
        code: IncidentCode::Label(LabelIncident::Export),
        class: PolicyClass::Explain,
        predicate: PolicyPredicate::Always,
        severity: IncidentSeverity::Blocks,
    },
    PolicyEntry {
        code: IncidentCode::Label(LabelIncident::SourceMonitor),
        class: PolicyClass::Explain,
        predicate: PolicyPredicate::Always,
        severity: IncidentSeverity::Blocks,
    },
    PolicyEntry {
        code: IncidentCode::Label(LabelIncident::Relink),
        class: PolicyClass::Explain,
        predicate: PolicyPredicate::Always,
        severity: IncidentSeverity::Blocks,
    },
    PolicyEntry {
        code: IncidentCode::Label(LabelIncident::AgentBranch),
        class: PolicyClass::Explain,
        predicate: PolicyPredicate::Always,
        severity: IncidentSeverity::Blocks,
    },
    PolicyEntry {
        code: IncidentCode::Label(LabelIncident::TranscriptEdit),
        class: PolicyClass::Explain,
        predicate: PolicyPredicate::Always,
        severity: IncidentSeverity::Blocks,
    },
    PolicyEntry {
        code: IncidentCode::Label(LabelIncident::Media),
        class: PolicyClass::Explain,
        predicate: PolicyPredicate::Always,
        severity: IncidentSeverity::Blocks,
    },
    PolicyEntry {
        code: IncidentCode::Label(LabelIncident::MediaIncomplete),
        class: PolicyClass::Explain,
        predicate: PolicyPredicate::Always,
        severity: IncidentSeverity::Degrades,
    },
    PolicyEntry {
        code: IncidentCode::Label(LabelIncident::Agent),
        class: PolicyClass::Explain,
        predicate: PolicyPredicate::Always,
        severity: IncidentSeverity::Blocks,
    },
    PolicyEntry {
        code: IncidentCode::Label(LabelIncident::Recording),
        class: PolicyClass::Explain,
        predicate: PolicyPredicate::Always,
        severity: IncidentSeverity::Blocks,
    },
    PolicyEntry {
        code: IncidentCode::Label(LabelIncident::Project),
        class: PolicyClass::Explain,
        predicate: PolicyPredicate::Always,
        severity: IncidentSeverity::Blocks,
    },
    PolicyEntry {
        code: IncidentCode::Label(LabelIncident::Captions),
        class: PolicyClass::Explain,
        predicate: PolicyPredicate::Always,
        severity: IncidentSeverity::Blocks,
    },
    PolicyEntry {
        code: IncidentCode::Label(LabelIncident::Mixer),
        class: PolicyClass::Explain,
        predicate: PolicyPredicate::Always,
        severity: IncidentSeverity::Blocks,
    },
    PolicyEntry {
        code: IncidentCode::Label(LabelIncident::MediaCache),
        class: PolicyClass::Explain,
        predicate: PolicyPredicate::Always,
        severity: IncidentSeverity::Blocks,
    },
    PolicyEntry {
        code: IncidentCode::Label(LabelIncident::Timeline),
        class: PolicyClass::Explain,
        predicate: PolicyPredicate::Always,
        severity: IncidentSeverity::Blocks,
    },
    // Row 68 — `IN2B` §5 rule 1 #1: the panel asked, the worker failed.
    PolicyEntry {
        code: IncidentCode::Label(LabelIncident::PanelWorkerError),
        class: PolicyClass::Explain,
        predicate: PolicyPredicate::Always,
        severity: IncidentSeverity::Blocks,
    },
    // Row 69 — rule 1 #2: an earlier run left damage; the dialog asks what to do.
    PolicyEntry {
        code: IncidentCode::Label(LabelIncident::RecoveryDamage),
        class: PolicyClass::Explain,
        predicate: PolicyPredicate::Always,
        severity: IncidentSeverity::Blocks,
    },
    // Row 70 — rule 1 #3: recording stopped, so a future crash would lose work.
    PolicyEntry {
        code: IncidentCode::Label(LabelIncident::RecoveryUnavailable),
        class: PolicyClass::Explain,
        predicate: PolicyPredicate::Always,
        severity: IncidentSeverity::Blocks,
    },
    // Row 71 — rule 1 #4: the project opens; only overwrite-save is refused.
    PolicyEntry {
        code: IncidentCode::Label(LabelIncident::ProjectNewerFormat),
        class: PolicyClass::Explain,
        predicate: PolicyPredicate::Always,
        severity: IncidentSeverity::Degrades,
    },
    // Row 72 — rule 1 #5: some history skipped; everything else loaded.
    PolicyEntry {
        code: IncidentCode::Label(LabelIncident::SidecarUnknownCodes),
        class: PolicyClass::Explain,
        predicate: PolicyPredicate::Always,
        severity: IncidentSeverity::Degrades,
    },
    // Row 73 — rule 1 #6: the project opens with an empty history.
    PolicyEntry {
        code: IncidentCode::Label(LabelIncident::SidecarRefused),
        class: PolicyClass::Explain,
        predicate: PolicyPredicate::Always,
        severity: IncidentSeverity::Degrades,
    },
    // Row 74 — rule 1 #7: the save succeeded; the history write failed.
    PolicyEntry {
        code: IncidentCode::Label(LabelIncident::SidecarWriteFailed),
        class: PolicyClass::Explain,
        predicate: PolicyPredicate::Always,
        severity: IncidentSeverity::Degrades,
    },
];

/// The codes an investigator session may start for, and no other
/// (IN2 §3.1 rule 1).
///
/// A `const` list rather than a predicate evaluated at run time. A predicate
/// would silently enrol Part B's eleven newly reachable codes and D-B2's three
/// panel-sink groups the day they land, on a population Part A never measured;
/// a `const` makes joining it an edit somebody reviews.
///
/// **It is exactly `POLICY[13..67]` — rows 14–67, contiguous** — which is a
/// checkable property and not a coincidence: [`POLICY`] declares the three
/// `AutoApply` rows first, then the colour rows the recovery vocabulary can and
/// cannot serve, then everything else (IN2 §3.1 rule 3).
///
/// **Thirteen codes are off it, for exactly two reasons**, both of which the
/// table prints (IN2 §3.1 rule 2):
///
/// - **it has a deterministic recovery** — [`deterministic_recovery`] builds
///   one, so the card already has a button and a code with a button must never
///   spend a model. Three rows: `unknown_source_range`,
///   `unknown_source_bit_depth`, `unknown_source_white_point`.
/// - **no honest recovery exists in Part A and a session cannot invent one** —
///   the seven `unsupported_source_*` rows, whose probe carries a *known*
///   non-Rec.709 value that a recovery would have to overwrite
///   (IN2 §7 rule 5). A session would spend a model to arrive at the sentence
///   the card already shows.
///
/// Plus the three `AutoApply` rows, which the router owns. **13 off, 54 on.**
/// Everything else is on it, including every row IN2 §7 measured *unbuildable*:
/// a row with no button and no honest operation is exactly the row a session is
/// for.
pub const INVESTIGATOR_ALLOWLIST: [IncidentCode; 54] = [
    IncidentCode::Media(MediaIncident::UnsupportedDecoderFormat),
    IncidentCode::Media(MediaIncident::BackendUnclassified),
    IncidentCode::DeliveryColor(DeliveryColorIncident::UnsupportedCodec),
    IncidentCode::DeliveryColor(DeliveryColorIncident::UnsupportedField),
    IncidentCode::DeliveryColor(DeliveryColorIncident::PixelFormatDepthMismatch),
    IncidentCode::DeliveryColor(DeliveryColorIncident::EncoderPixelFormatUnavailable),
    IncidentCode::DeliveryVerification(DeliveryVerificationIncident::NotFullResolution),
    IncidentCode::DeliveryVerification(DeliveryVerificationIncident::PlaneOutOfContainer),
    IncidentCode::DeliveryVerification(DeliveryVerificationIncident::FrameCountMismatch),
    IncidentCode::DeliveryVerification(DeliveryVerificationIncident::FrameCountOutOfRange),
    IncidentCode::DeliveryVerification(DeliveryVerificationIncident::BudgetLaneMismatch),
    IncidentCode::ColorQc(ColorQcIncident::ProxyProofRefused),
    IncidentCode::ColorQc(ColorQcIncident::RasterLengthMismatch),
    IncidentCode::ColorQc(ColorQcIncident::EmptyPopulation),
    IncidentCode::ColorQc(ColorQcIncident::NodeBudgetExceeded),
    IncidentCode::ColorQc(ColorQcIncident::MatteRegionRasterMismatch),
    IncidentCode::ColorQc(ColorQcIncident::NodeRemovalRejected),
    IncidentCode::Operation(IncidentFamily::Bounds),
    IncidentCode::Operation(IncidentFamily::Malformed),
    IncidentCode::Operation(IncidentFamily::Duplicate),
    IncidentCode::Operation(IncidentFamily::Placement),
    IncidentCode::Operation(IncidentFamily::Missing),
    IncidentCode::Operation(IncidentFamily::Structure),
    IncidentCode::Operation(IncidentFamily::Relink),
    IncidentCode::Operation(IncidentFamily::Unrepresentable),
    IncidentCode::Operation(IncidentFamily::UnknownName),
    IncidentCode::Operation(IncidentFamily::Internal),
    IncidentCode::Operation(IncidentFamily::ColorPolicy),
    IncidentCode::LutAssetPolicy,
    IncidentCode::EditRevisionConflict,
    IncidentCode::Rejection(RejectionIncident::EditPlan),
    IncidentCode::Rejection(RejectionIncident::DeliveryVariant),
    IncidentCode::Rejection(RejectionIncident::AgentBranch),
    IncidentCode::Rejection(RejectionIncident::SourceEdit),
    IncidentCode::Rejection(RejectionIncident::Relink),
    IncidentCode::Rejection(RejectionIncident::ProjectSave),
    IncidentCode::Rejection(RejectionIncident::CaptionPlan),
    IncidentCode::Label(LabelIncident::Operations),
    IncidentCode::Label(LabelIncident::Look),
    IncidentCode::Label(LabelIncident::LookIncomplete),
    IncidentCode::Label(LabelIncident::Export),
    IncidentCode::Label(LabelIncident::SourceMonitor),
    IncidentCode::Label(LabelIncident::Relink),
    IncidentCode::Label(LabelIncident::AgentBranch),
    IncidentCode::Label(LabelIncident::TranscriptEdit),
    IncidentCode::Label(LabelIncident::Media),
    IncidentCode::Label(LabelIncident::MediaIncomplete),
    IncidentCode::Label(LabelIncident::Agent),
    IncidentCode::Label(LabelIncident::Recording),
    IncidentCode::Label(LabelIncident::Project),
    IncidentCode::Label(LabelIncident::Captions),
    IncidentCode::Label(LabelIncident::Mixer),
    IncidentCode::Label(LabelIncident::MediaCache),
    IncidentCode::Label(LabelIncident::Timeline),
];

/// The plain-language body every `Explain` recovery carries.
///
/// Exhaustive with **no wildcard arm**, so a seventy-fifth code breaks the
/// build. **Twenty-eight** codes delegate to a shipped `recovery_action()` and
/// no string of theirs is restated here (`IN1b` §3.7 rule 34); the delegated
/// set carries **16** distinct sentences, because
/// `ColorSourceError::recovery_action()` is a `const fn` with no `match` and
/// returns one sentence for all thirteen source-colour codes. Rewriting those
/// thirteen per code is IN1 §13 D8 and is owned by IN3. The remaining
/// **46** bodies are written — 39 in `IN1b` §5.6, 7 in `IN2B` §5 rule 1 —
/// and are copied here verbatim, so the whole table carries 62 distinct
/// sentences over 74 codes and the 61 non-colour bodies are pairwise distinct.
#[must_use]
// 74 arms carrying 46 written sentences: the bodies are the deliverable and
// they are read as prose, so splitting the table across helper functions would
// scatter the thing a reviewer is asked to read in one pass (`IN1b` §5.6, N0/Q4).
#[allow(clippy::too_many_lines)]
pub fn explain_body(code: IncidentCode) -> &'static str {
    match code {
        // Delegated, all thirteen to the one shipped sentence.
        IncidentCode::SourceColor(incident) => incident.source_error().recovery_action(),
        // Delegated, one sentence each.
        IncidentCode::DeliveryColor(incident) => incident.recovery_action(),
        IncidentCode::DeliveryVerification(incident) => incident.recovery_action(),
        IncidentCode::ColorQc(incident) => incident.recovery_action(),
        // Written (`IN1b` §5.6), one per code with no accessor behind it.
        IncidentCode::Media(MediaIncident::UnsupportedDecoderFormat) => {
            "This file's pixel format is not one the managed renderer can prove is a supported integer source surface, so it refuses to decode it rather than guess at its depth. The message names the format and the depths it found; transcode the file to an 8-, 10- or 16-bit integer format, or relink to a copy that is already in one."
        }
        IncidentCode::Media(MediaIncident::BackendUnclassified) => {
            "The media engine refused this work and gave a reason but no code Kinewright can act on. The message is the engine's own; if it names a file, check the file is where the project expects it, then try again."
        }
        IncidentCode::Operation(IncidentFamily::Bounds) => {
            "This value is outside the range the edit allows. The message names the value you gave and, where it can, the range it must be in; set it inside that range and make the edit again."
        }
        IncidentCode::Operation(IncidentFamily::Malformed) => {
            "One of the fields this edit carries is missing, empty, or the wrong kind of value. The message names the field; supply a value of the kind it asks for and make the edit again."
        }
        IncidentCode::Operation(IncidentFamily::Duplicate) => {
            "Something with this identity is already in the project, so adding it again would leave two things sharing one id. Use a fresh id, or drop the second entry, and make the edit again."
        }
        IncidentCode::Operation(IncidentFamily::Placement) => {
            "This edit is valid, but not on the thing it was aimed at — a picture effect on an audio bus, a title on an audio track. Choose a target of the right kind and make the edit again."
        }
        IncidentCode::Operation(IncidentFamily::Missing) => {
            "The clip, track, asset, marker or effect this edit names is not in the project any more, so the id it used is stale. Re-read the timeline and make the edit against what is there now."
        }
        IncidentCode::Operation(IncidentFamily::Structure) => {
            "The edit is well formed, but something else has to change first — an overlap, an ordering, a live reference, or two fields that disagree. The message names what blocks it; change that, then make the edit again."
        }
        IncidentCode::Operation(IncidentFamily::Relink) => {
            "The replacement media could not be matched to the clip it is meant to replace: its fingerprint is missing, or it does not agree with the original. Relink from the media panel, or relink again with the explicit unverified-source option if you know it is the right file."
        }
        IncidentCode::Operation(IncidentFamily::Unrepresentable) => {
            "There is no value that would work: the edit cannot be expressed on this project's whole-frame grid. Choose a different frame or a different duration, rather than a different number."
        }
        IncidentCode::Operation(IncidentFamily::UnknownName) => {
            "Kinewright does not know an effect, transition, parameter or capability by that name. Look the accepted spelling up in the capability registry and make the edit with that name."
        }
        IncidentCode::Operation(IncidentFamily::Internal) => {
            "Nothing you did caused this and nothing you can change will fix it: the part of Kinewright that applies edits has stopped, an id space has run out, or an internal time conversion overflowed. Your project is intact and still on disk. Save it if you can, restart Kinewright, and report what you were doing."
        }
        IncidentCode::Operation(IncidentFamily::ColorPolicy) => {
            "This colour override is not one Kinewright will write — it names a provenance the guard refuses, or supplies a field only the application may set. Change a source's colour through the incident card or the Media panel's own control rather than writing the description directly."
        }
        IncidentCode::LutAssetPolicy => {
            "This LUT's hash or metadata is not the one the project recorded for it, so the file on disk is not the file the project expects. Import or restore the LUT through the look browser, which writes the store entry and the document together."
        }
        IncidentCode::EditRevisionConflict => {
            "The timeline moved between the moment this edit was planned and the moment it was sent, so it was refused rather than applied to a document it was not planned against. Nothing changed; make the edit again against what the timeline shows now."
        }
        IncidentCode::Rejection(RejectionIncident::EditPlan) => {
            "The edit plan was refused as a whole, so nothing in it was applied — either it contained no operations at all, or one of them failed. Where an operation failed the message names which one and why; fix that one, or add an operation to an empty plan, and send it again."
        }
        IncidentCode::Rejection(RejectionIncident::DeliveryVariant) => {
            "The delivery variant could not be built from this project — the focal point is outside the frame, or the derived document is not valid. Adjust the focus percentages or the source timeline, then export again."
        }
        IncidentCode::Rejection(RejectionIncident::AgentBranch) => {
            "The isolated agent branch refused this request. Either the base document or the operation number named is not one it can use — read the branch's operation list and name one of the numbers it shows — or the branch's own core has stopped, in which case start a new agent thread, which builds a fresh one."
        }
        IncidentCode::Rejection(RejectionIncident::SourceEdit) => {
            "Nothing was applied: something the Source edit depended on changed while the source was being verified. The message says which — if the source asset itself is gone, relink it from the media panel; otherwise reload the source, re-mark In and Out on what is loaded now, and make the edit again."
        }
        IncidentCode::Rejection(RejectionIncident::Relink) => {
            "This file cannot stand in for the asset — either it produced no verified fingerprint, or its fingerprint does not match the original. Choose a different file, or relink with the explicit unverified-source option if you know it is the right media."
        }
        IncidentCode::Rejection(RejectionIncident::ProjectSave) => {
            "The project file could not be written, so this project is still only in memory. The message says whether it failed while serialising or while writing; free some space or choose another location, then save again."
        }
        IncidentCode::Rejection(RejectionIncident::CaptionPlan) => {
            "The caption track could not be planned from these cues: there are no cues, the authored script does not line up with them, or a cue has no duration. Adjust the transcript selection or the script, then generate the captions again."
        }
        IncidentCode::Label(LabelIncident::Operations) => {
            "This edit was not applied, and Kinewright has no code for the reason yet — the message is the only description it has. Read the message, change what it names, and make the edit again."
        }
        IncidentCode::Label(LabelIncident::Look) => {
            "The look or LUT could not be imported, restored or applied. The message names the file or the store; check that the project has been saved and that its LUT folder is a writable directory, then try again."
        }
        IncidentCode::Label(LabelIncident::LookIncomplete) => {
            "The project was saved or opened, but not every look came with it, or the LUT folder beside it cannot be used. The message names which looks or which folder; re-import or restore them from the look browser, or save the project somewhere its LUT folder can be written, and they will be available again. The rest of the project is unaffected."
        }
        IncidentCode::Label(LabelIncident::Export) => {
            "The export did not start, or did not finish. The message names the step that stopped it; fix what it names in the export dialog and export again."
        }
        IncidentCode::Label(LabelIncident::SourceMonitor) => {
            "No edit was applied: the Source monitor could not act on what is loaded. The message says which part of the source or its marks is no longer usable; reload the source, re-mark it, and make the edit again."
        }
        IncidentCode::Label(LabelIncident::Relink) => {
            "The relink could not go ahead. The message names the file or the project; choose a replacement the application can read and press Relink again."
        }
        IncidentCode::Label(LabelIncident::AgentBranch) => {
            "The isolated branch this agent thread edits in could not be created or used. The message is the only description; start a new agent thread, which builds a fresh branch."
        }
        IncidentCode::Label(LabelIncident::TranscriptEdit) => {
            "The transcript edit produced no change: either the words selected contain no removable frames, or the plan behind them was refused. Select a different range of words and try again."
        }
        IncidentCode::Label(LabelIncident::Media) => {
            "Playback, import or capture stopped, or there was nothing to play. The message is the only description Kinewright has — it may name a file that has moved, or a thing the timeline still needs, such as a clip before you can press play. Do what it names, then try again."
        }
        IncidentCode::Label(LabelIncident::MediaIncomplete) => {
            "The project opened, but some of its media is not where the project expects it, or its timeline pictures could not be built. The message names which; relink the missing files from the media panel — the rest of the project is unaffected."
        }
        IncidentCode::Label(LabelIncident::Agent) => {
            "The agent harness could not be reached, started, or spoken to. The message names the harness; check that it is installed and on your PATH, then send the message again."
        }
        IncidentCode::Label(LabelIncident::Recording) => {
            "The recording did not start, or stopped before it finished. The message names the device or the log file; choose a camera and a microphone that are connected, then start the capture again."
        }
        IncidentCode::Label(LabelIncident::Project) => {
            "The project could not be opened, created, read, or restored from the crash-recovery journal. The message names the file; check that it exists and can be read, then open it again — if it was unsaved work that could not be restored, the last saved version of the project is still intact."
        }
        IncidentCode::Label(LabelIncident::Captions) => {
            "The captions could not be generated or saved. The message names the step; adjust the transcript selection or the output path and try again."
        }
        IncidentCode::Label(LabelIncident::Mixer) => {
            "The mixer could not do what was asked — usually because the audio it needs to learn from, or the node it was learned for, is not on the bus any more. Re-select the range or the node and try again."
        }
        IncidentCode::Label(LabelIncident::MediaCache) => {
            "The media cache could not be cleared. The message names the cache and the reason; close anything using those files, then clear it again."
        }
        IncidentCode::Label(LabelIncident::Timeline) => {
            "The timeline gesture did not complete. The message is the only description Kinewright has; re-select what the gesture needed and make it again."
        }
        // Written (`IN2B` §5 rule 1), one per code with no accessor behind it.
        IncidentCode::Label(LabelIncident::PanelWorkerError) => {
            "One of the panels could not show its result because the worker behind it failed. The message is the worker's own and names what went wrong; fix what it names and the panel will show its result on its own."
        }
        IncidentCode::Label(LabelIncident::RecoveryDamage) => {
            "Kinewright found unsaved work or damage from an earlier run. The message names what was found; the dialog beside this card asks what to do with it. If you recover, the recovered document starts a new history — the incident history beside it is loaded from the last save."
        }
        IncidentCode::Label(LabelIncident::RecoveryUnavailable) => {
            "Crash recovery is not recording in this run, so a future crash would lose unsaved work. The message is the journal recorder's own error; save now — work already saved is unaffected, but edits made while this warning stands may not survive a crash."
        }
        IncidentCode::Label(LabelIncident::ProjectNewerFormat) => {
            "This project file was written by a newer Kinewright than this one. It opens so nothing is lost, but saving over it is disabled — saving would silently drop what the newer version added. Use Save As to keep working in a copy, or open the project in the newer Kinewright."
        }
        IncidentCode::Label(LabelIncident::SidecarUnknownCodes) => {
            "Some of this project's saved incident history used codes this Kinewright does not know, so those records were skipped and everything else loaded. The count is on the card; open the project in a newer Kinewright to see the full history."
        }
        IncidentCode::Label(LabelIncident::SidecarRefused) => {
            "This project's saved incident history could not be loaded. The message names why — a damaged file, a newer writer, or a history file that belongs to a different project. The project itself opens normally with an empty history, and the unreadable file is kept beside it, never deleted."
        }
        IncidentCode::Label(LabelIncident::SidecarWriteFailed) => {
            "The project saved, but its incident history could not be written beside it. The message names why. The history is still live in this run and will be written again on the next save, or when the project closes."
        }
    }
}

/// The [`POLICY`] row for one code. Total by construction; see
/// [`IncidentCode::table_index`].
const fn policy_entry(code: IncidentCode) -> PolicyEntry {
    POLICY[code.table_index()]
}

/// The class a code resolves to for one piece of evidence.
///
/// The parameter is the whole evidence rather than a probed description
/// (`IN1b` §3.5 rule 26), and the reason is measurable: an `operation_bounds`
/// incident on a clip carries no probed colour description, so under Part A's
/// signature it could not be classified at all.
///
/// [`PolicyPredicate::Always`] returns the declared class;
/// [`PolicyPredicate::Rec709Compatible`] returns the declared class when
/// [`rec709_compatible`] is true of a probed description the evidence actually
/// carries, and [`PolicyClass::Explain`] otherwise — so a predicated row
/// reached with no probed description falls to `Explain`, which is the safe
/// direction and the only honest one (IN1 §2.4 rule 36).
#[must_use]
pub fn policy_class(code: IncidentCode, evidence: &IncidentEvidence) -> PolicyClass {
    let entry = policy_entry(code);
    match entry.predicate {
        PolicyPredicate::Always => entry.class,
        PolicyPredicate::Rec709Compatible => {
            if evidence.probed().is_some_and(rec709_compatible) {
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
    match policy_class(code, evidence) {
        PolicyClass::AutoApply | PolicyClass::AskFirst => {
            // The only applying recovery IN1 ships needs an asset and a probed
            // description; a row that resolves to `AutoApply` without both is a
            // contradiction the exhaustiveness test catches, not a panic here.
            // An empty `Vec` is therefore a legal return (`IN1b` §3.7 rule 33).
            match (subject, evidence.probed()) {
                (IncidentSubject::Asset(asset), Some(probed)) => vec![RecoveryAction {
                    label: ASSUME_REC709_LABEL,
                    kind: RecoveryKind::Operation(assume_rec709_operation(asset, probed)),
                }],
                _ => Vec::new(),
            }
        }
        // The sentence is core's and is never composed by a caller
        // (IN1 §2.5 rule 46). `explain_body` supersedes Part A's direct call to
        // `code.source_error().recovery_action()`, delegating to that same
        // accessor for the thirteen colour rows so they keep the shipped
        // sentence verbatim (`IN1b` §3.7 rule 34).
        //
        // A row with a buildable operation offers **a button and the
        // sentence**, in that order, and every other row is byte-for-byte what
        // it was before IN2 (IN2 §7 rule 1). `card_actions` needs no change:
        // it already enables an action iff its kind is
        // [`RecoveryKind::Operation`], so the card's button and the no-harness
        // fallback come from this one arm.
        PolicyClass::Explain => {
            let mut actions = Vec::new();
            if let Some(action) = deterministic_recovery(code, subject, evidence) {
                actions.push(action);
            }
            actions.push(RecoveryAction {
                label: EXPLAIN_LABEL,
                kind: RecoveryKind::Explain(explain_body(code)),
            });
            actions
        }
    }
}

/// The deterministic recovery an `Explain` row offers, when one is buildable
/// from the evidence the incident already carries (IN2 §7).
///
/// An **exhaustive match with no wildcard**, so a seventy-fifth code cannot
/// silently default to "no button": every code is named, and the fifty-eight
/// that IN2 §7's table does not discuss return `None` through one explicit
/// `|`-joined arm.
///
/// **Three rows build one**, and they are the three `unknown_source_*` rows
/// whose producer — `IncidentObservation::from_media_error`'s
/// `MediaError::SourceColorForAsset` arm — emits
/// [`IncidentEvidence::SourceColor`] and sets the subject to the asset itself,
/// so every input [`assume_rec709_operation`] needs is already there
/// (IN2 §7 rule 5).
///
/// **Ten colour rows do not**, and the reason is a predicate *plus* a
/// reachability argument, because the predicate alone does not do it.
/// [`rec709_compatible`] is a predicate over the **probe**, not over the code,
/// and returns `true` for an all-`Unknown` probe on an `unsupported_*` code
/// too. What separates the two groups is that an `unsupported_*` row is only
/// *reached* when the named field carries a known non-Rec.709 value, and
/// `rec709_compatible` of such a probe is `false` — so the builder would have
/// to **overwrite** a known value, which is the silent behaviour CC1 §1
/// forbids. The gate below is the predicate, and the reachability is what makes
/// it sufficient.
///
/// **Six delivery and clamp rows do not**, for two independent reasons either
/// of which is sufficient (IN2 §7 rule 6): this function never sees `observed`
/// or `allowed` — they are fields of [`Incident`], not of
/// [`IncidentEvidence`], and `policy_recovery` is called at observe time with
/// the evidence alone — and even given them, both are prose, so building a
/// typed [`Operation`] would mean parsing `Debug` output.
#[must_use]
pub fn deterministic_recovery(
    code: IncidentCode,
    subject: IncidentSubject,
    evidence: &IncidentEvidence,
) -> Option<RecoveryAction> {
    if !has_deterministic_recovery(code) {
        return None;
    }
    // The three rows' shared producer sets the subject to the asset itself and
    // puts the whole probe on the evidence, so both destructurings hold on the
    // reachable path and neither is a silent fallback.
    let IncidentSubject::Asset(asset) = subject else {
        return None;
    };
    let probed = evidence.probed()?;
    if !rec709_compatible(probed) {
        return None;
    }
    Some(RecoveryAction {
        label: ASSUME_REC709_LABEL,
        kind: RecoveryKind::Operation(assume_rec709_operation(asset, probed)),
    })
}

/// Whether [`deterministic_recovery`] has a builder for this code at all.
///
/// The **exhaustive match with no wildcard** IN2 §7 rule 2 requires: every one
/// of the seventy-four codes is named, so a seventy-fifth cannot silently
/// default to "no button". The `false` arm's groups are named in order, and the
/// reason each group is `false` is in [`deterministic_recovery`]'s own
/// documentation.
const fn has_deterministic_recovery(code: IncidentCode) -> bool {
    match code {
        // IN2 §7 rows 1-3: buildable, under `rec709_compatible`.
        IncidentCode::SourceColor(
            SourceColorIncident::UnknownRange
            | SourceColorIncident::UnknownBitDepth
            | SourceColorIncident::UnknownWhitePoint,
        ) => true,
        // Everything else. Clippy requires one nested group per outer
        // variant, so the reasons are named beside the rows they cover.
        IncidentCode::SourceColor(
            // IN2 §7 rows 4-10: a recovery would have to overwrite a known
            // non-Rec.709 value. IN1 §13 D8, IN3.
            SourceColorIncident::UnsupportedPrimaries
            | SourceColorIncident::UnsupportedTransfer
            | SourceColorIncident::UnsupportedMatrix
            | SourceColorIncident::UnsupportedRange
            | SourceColorIncident::UnsupportedWhitePoint
            | SourceColorIncident::UnsupportedBitDepth
            | SourceColorIncident::UnsupportedCombination
            // The three `AutoApply` rows, whose recovery is `policy_recovery`'s
            // own `AutoApply` arm and never this one.
            | SourceColorIncident::UnknownPrimaries
            | SourceColorIncident::UnknownTransfer
            | SourceColorIncident::UnknownMatrix,
        )
        // IN2 §7 rows 11-16: the evidence is a code and a rendered sentence,
        // `probed()` is `None`, and `observed`/`allowed` are prose this
        // function never sees anyway. Rows 11-14 and 15 are here; row 16 is
        // the `FrameCountOutOfRange` below.
        | IncidentCode::DeliveryColor(
            DeliveryColorIncident::UnsupportedCodec
            | DeliveryColorIncident::UnsupportedField
            | DeliveryColorIncident::PixelFormatDepthMismatch
            | DeliveryColorIncident::EncoderPixelFormatUnavailable,
        )
        | IncidentCode::ColorQc(
            // IN2 §7 row 15.
            ColorQcIncident::NodeBudgetExceeded
            // The five remaining colour-QC rows: each is a session.
            | ColorQcIncident::ProxyProofRefused
            | ColorQcIncident::RasterLengthMismatch
            | ColorQcIncident::EmptyPopulation
            | ColorQcIncident::MatteRegionRasterMismatch
            | ColorQcIncident::NodeRemovalRejected,
        )
        | IncidentCode::DeliveryVerification(
            // IN2 §7 row 16.
            DeliveryVerificationIncident::FrameCountOutOfRange
            // The four remaining verification rows: each is a session.
            | DeliveryVerificationIncident::NotFullResolution
            | DeliveryVerificationIncident::PlaneOutOfContainer
            | DeliveryVerificationIncident::FrameCountMismatch
            | DeliveryVerificationIncident::BudgetLaneMismatch,
        )
        // Every remaining code: no deterministic recovery in Part A, named
        // rather than defaulted. Each is a session (IN2 §3.1 rule 2).
        | IncidentCode::Media(
            MediaIncident::UnsupportedDecoderFormat | MediaIncident::BackendUnclassified,
        )
        | IncidentCode::Operation(
            IncidentFamily::Bounds
            | IncidentFamily::Malformed
            | IncidentFamily::Duplicate
            | IncidentFamily::Placement
            | IncidentFamily::Missing
            | IncidentFamily::Structure
            | IncidentFamily::Relink
            | IncidentFamily::Unrepresentable
            | IncidentFamily::UnknownName
            | IncidentFamily::Internal
            | IncidentFamily::ColorPolicy,
        )
        | IncidentCode::LutAssetPolicy
        | IncidentCode::EditRevisionConflict
        | IncidentCode::Rejection(
            RejectionIncident::EditPlan
            | RejectionIncident::DeliveryVariant
            | RejectionIncident::AgentBranch
            | RejectionIncident::SourceEdit
            | RejectionIncident::Relink
            | RejectionIncident::ProjectSave
            | RejectionIncident::CaptionPlan,
        )
        | IncidentCode::Label(
            LabelIncident::Operations
            | LabelIncident::Look
            | LabelIncident::LookIncomplete
            | LabelIncident::Export
            | LabelIncident::SourceMonitor
            | LabelIncident::Relink
            | LabelIncident::AgentBranch
            | LabelIncident::TranscriptEdit
            | LabelIncident::Media
            | LabelIncident::MediaIncomplete
            | LabelIncident::Agent
            | LabelIncident::Recording
            | LabelIncident::Project
            | LabelIncident::Captions
            | LabelIncident::Mixer
            | LabelIncident::MediaCache
            | LabelIncident::Timeline
            // The seven `IN2B` §5 labels: `Plain` evidence carries no
            // buildable operation either — and no session (off the allowlist,
            // IN2 §3.1 reason 2).
            | LabelIncident::PanelWorkerError
            | LabelIncident::RecoveryDamage
            | LabelIncident::RecoveryUnavailable
            | LabelIncident::ProjectNewerFormat
            | LabelIncident::SidecarUnknownCodes
            | LabelIncident::SidecarRefused
            | LabelIncident::SidecarWriteFailed,
        ) => false,
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

//---------------------------------------------------------------------------
// Sidecar persistence (`IN2B` §§2–3): records, reports, `from_code`, stops.
//---------------------------------------------------------------------------

/// Stop strings that persist verbatim (`IN2B` §2 rule 14, N2/S-5).
///
/// The seventeen fixed stops an investigator session can end with: `completed`;
/// the three `budget:` stops; the confirmation, disconnect, switched-off,
/// ended-unexpectedly and could-not-start stops; `re-investigated`; the three
/// close stops (`interrupted`, `project was closed`, `Kinewright is closing`);
/// `resolved elsewhere`; and the three conflict/refusal stops. Anything else
/// a stop carries is a payload and persists as its category — see
/// [`persisted_stop`]. Order is the contract's listing order; the item-9 test
/// iterates this const so a new member is covered without a test edit.
pub const STOPS: [&str; 17] = [
    "completed",
    "budget: turns",
    "budget: wall time",
    "budget: tokens",
    "the session asked for a confirmation",
    "the agent event stream disconnected",
    "the investigator was switched off",
    "the investigator session ended unexpectedly",
    "the investigator session could not start",
    "re-investigated",
    INTERRUPTED_STOP,
    "the project was closed",
    "Kinewright is closing",
    "the incident was resolved elsewhere",
    "the proposal conflicted twice against the live timeline",
    "the live timeline refused the proposal",
    "the proposal carried no operations",
];

/// The stop a record carries for an entry that was still `Investigating` when
/// the sidecar was written (`IN2B` §3 rule 4). At orderly close the sessions
/// end through `shutdown_for_close` before the final flush, so this stop
/// appears only in background flushes taken while a session still runs.
const INTERRUPTED_STOP: &str = "interrupted: Kinewright closed";

/// Categorise a session stop for persistence (`IN2B` §2 rule 14).
///
/// Verbatim iff `stop` is in [`STOPS`]; the category for the two payload
/// prefixes the app stamps at build (`harness: `, `observer: `); the
/// defensive `"session"` otherwise. Harness and observer error text is
/// arbitrary process output (paths, auth errors) and must not travel with the
/// project file — everything else about the telemetry persists verbatim.
#[must_use]
pub fn persisted_stop(stop: &str) -> &str {
    if STOPS.contains(&stop) {
        return stop;
    }
    if stop.starts_with("harness: ") {
        return "harness";
    }
    if stop.starts_with("observer: ") {
        return "observer";
    }
    "session"
}

/// Resolve a wire code string to its code (`IN2B` §3 rule 3, E-B8).
///
/// A scan over [`POLICY`], not a generated match: a scan cannot disagree with
/// the table it scans, and at most a few hundred records per load the linear
/// scan is unmeasurable. Total over all 74 codes; unknown — including `""`
/// and wrong-case — resolves to `None` and feeds the rule-6 aggregate.
#[must_use]
pub fn from_code(code: &str) -> Option<IncidentCode> {
    POLICY
        .iter()
        .find(|entry| entry.code.code() == code)
        .map(|entry| entry.code)
}

/// A session still running while the sidecar is written (`IN2B` §3 rule 4).
///
/// Plain data only — no agent types in core. The app fills it from the running
/// session's config, counters and last cost event; the record builder flushes
/// it into the entry's telemetry exactly as a live end would. The resolver is
/// `None` while running and `SharedCounters` never reaches the log, so both
/// are absent here by construction rather than by discipline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunningInvestigation {
    /// The incident the session runs against.
    pub id: IncidentId,
    /// The harness-id string the session runs on.
    pub harness: String,
    /// The model, when one is configured.
    pub model: Option<String>,
    /// Turns spent so far.
    pub turns: u32,
    /// The six cost mirrors, exactly as a live end writes them.
    pub input_tokens: Option<u64>,
    /// Provider cached input tokens, when reported.
    pub cached_input_tokens: Option<u64>,
    /// Provider cache-creation input tokens, when reported.
    pub cache_creation_input_tokens: Option<u64>,
    /// Provider output tokens, when reported.
    pub output_tokens: Option<u64>,
    /// Provider reasoning output tokens, when reported.
    pub reasoning_output_tokens: Option<u64>,
    /// Session cost in millionths of a dollar, when reported.
    pub cost_usd_millionths: Option<i64>,
}

/// One persisted incident (`IN2B` §2 rule 3).
///
/// A core type distinct from the wire shape: it omits everything derivable
/// (`class`, `severity`, `field`, `recoveries` — restore recomputes them from
/// the *current* policy table, so a loaded incident always agrees with it)
/// and carries wall millis plus offset nanos instead of `opened_at`. What is
/// not in the record cannot drift from it. Field order is the §2 worked
/// example's; every `Option` skips when absent, as the wire does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IncidentRecord {
    /// Resumes `next_id` above the loaded maximum.
    pub id: IncidentId,
    /// The IN1 §2.2 rule 5 wire value; `from_code` at restore, unknown strings
    /// feeding the rule-6 aggregate.
    pub code: String,
    /// Dedup axis and card anchor.
    pub subject: IncidentSubject,
    /// Frozen evidence: what was seen, verbatim.
    pub observed: String,
    /// What would have been accepted, verbatim.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allowed: Option<String>,
    /// The typed payload sessions and recoveries read, verbatim.
    pub evidence: IncidentEvidence,
    /// Rebasing input: restore sets it to the opening revision.
    pub revision: TimelineRevision,
    /// Wall origin plus `opened_at` at write, carried verbatim across origins
    /// afterwards; `None` where the log has no wall origin.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub opened_wall_millis: Option<i64>,
    /// `opened_at.as_nanos()`: exact session order including same-millis ties.
    pub opened_offset_nanos: u64,
    /// Episode weight, verbatim.
    pub count: u32,
    /// The writer emits only `Open` / `Resolved`; a record-`Investigating`
    /// (hand-written only) maps to `Open` at load.
    #[serde(flatten)]
    pub state: IncidentState,
    /// Spend accounting, verbatim modulo the rule-14 stop category — and never
    /// `resolved_after`, which is log-origin-relative (the builder clears it,
    /// N-9; N4 F1).
    pub telemetry: IncidentTelemetry,
    /// The pending decision, minus operations, forced stale at load.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proposal: Option<IncidentProposal>,
    /// The name when seen.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject_name: Option<String>,
    /// The session's opening context, iff it fits the d5 ceiling.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refused_op: Option<Operation>,
}

/// What one [`IncidentLog::records`] call emitted (`IN2B` §0.4 d6).
///
/// The four fields sum to the log size minus nothing —
/// `written_open + written_resolved + dropped_transient_open + pruned_resolved
/// == log.len()` — so a miscount fails loudly instead of silently dropping
/// history.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WriteReport {
    /// Records emitted for open entries (never pruned).
    pub written_open: usize,
    /// Records emitted for resolved entries (at most 256, by `IncidentId`).
    pub written_resolved: usize,
    /// Open entries dropped for transient provenance (§2 rule 12).
    pub dropped_transient_open: usize,
    /// Resolved entries pruned oldest-first (§2 rule 11).
    pub pruned_resolved: usize,
}

/// What one [`IncidentLog::restore`] call loaded (`IN2B` §0.4 d6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoreReport {
    /// Entries restored open (counted by loaded state).
    pub restored_open: usize,
    /// Entries restored resolved (counted by loaded state).
    pub restored_resolved: usize,
    /// One entry per distinct unknown code string, in first-seen order, with
    /// its count.
    pub unknown_codes: Vec<(String, usize)>,
    /// Revision fields (record `revision` plus proposal `base_revision`) that
    /// differed from the opening revision and were rebased to it.
    pub rebased: usize,
    /// The passed-through raw record texts, untouched (§2 rule 8b).
    pub carried: Vec<String>,
    /// Every record's `refused_op`, keyed by the record's `id` (§3 rule 13).
    pub refused: BTreeMap<IncidentId, Operation>,
    /// Records skipped for an unusable id: the second and later records
    /// sharing an id (the first wins), and any record with an id past
    /// [`MAX_RESTORED_ID`] (N4 F4, N4.1). Over-ceiling records with an unknown
    /// code are counted in [`Self::unknown_codes`] instead, per F3.
    pub invalid_ids: usize,
}

/// Fixed prefix of the unknown-code aggregate's `observed`; the skipped total
/// renders once at note time and the suffix is stable thereafter (`IN2B` §3
/// rule 6, N-6). The dash is U+2014, matching the contract's string.
const UNKNOWN_CODES_OBSERVED_PREFIX: &str = "incidents used codes this build does not know — ";

/// How many resolved incidents a sidecar write keeps (`IN2B` §2 rule 11).
///
/// The most recent 256 by `IncidentId` — ids are monotonic across runs because
/// `next_id` resumes above the loaded maximum, while cross-run wall offsets
/// are incomparable once the origin changes. Open incidents are never pruned.
const MAX_PERSISTED_RESOLVED: usize = 256;

/// The highest id that can restore (N4.1).
///
/// Records past it are skipped and counted — resuming above them could
/// overflow `next_id` or saturate `observe` onto a live id — and an `id_floor`
/// past it is clamped to it, so `next_id` after restore is at most
/// `MAX_RESTORED_ID + 1` and `observe` never saturates in practice.
const MAX_RESTORED_ID: u64 = u64::MAX / 2;

/// The id-relevant outcome of scanning restore's records (N4.1).
struct RestoreIdResume {
    /// The value `next_id` resumes to; `None` when nothing restorable was
    /// seen and no floor was given, in which case `restore` leaves `next_id`
    /// put (an empty log's is 1).
    next_id: Option<u64>,
    /// How many unknown-code records were skipped: `restore` notes its
    /// rule-6 aggregate — consuming one fresh id — exactly when nonzero.
    unknown_skipped: usize,
}

/// Pure id resume for [`IncidentLog::restore`]: above every restorable id in
/// `ids` — at or below `MAX_RESTORED_ID`, known- or unknown-coded alike —
/// and the `id_floor` clamped to the ceiling, via `checked_add`.
///
/// Split out so Kani can prove the resume arithmetic without the serde and
/// heap machinery of `restore`; `restore` is its only caller. Each pair is
/// `(id, code_known)`; unknown-code records always reach the scan (they
/// count toward `unknown_skipped` whether or not their id is restorable),
/// while over-ceiling and duplicate known ids never do — the former must
/// not move the maximum, the latter cannot — exactly as the inline loop did
/// (N4 F2, F4, N4.1).
fn restore_id_resume(ids: &[(u64, bool)], id_floor: Option<u64>) -> RestoreIdResume {
    let mut max_id: Option<u64> = None;
    let mut unknown_skipped = 0_usize;
    for &(id, known) in ids {
        if !known {
            unknown_skipped += 1;
        }
        if id <= MAX_RESTORED_ID {
            max_id = Some(max_id.map_or(id, |max| max.max(id)));
        }
    }
    // Above EVERY restorable id: restored, unknown-code, and the raw carried
    // values' floor from the app, clamped to the ceiling. `checked_add`, never
    // `saturating_add` — overflow is impossible by construction now, and the
    // checked form documents that rather than trusting it (N4 F2, N4.1).
    let floor = id_floor.map(|floor| floor.min(MAX_RESTORED_ID));
    let highest = max_id.into_iter().chain(floor).max();
    RestoreIdResume {
        next_id: highest.and_then(|max| max.checked_add(1)),
        unknown_skipped,
    }
}

/// Past this many compact-JSON bytes the refused-op stash is omitted from the
/// record (`IN2B` §0.4 d5): IN2 §3.4 rule 23's elision, reused rather than
/// re-minted, so one number means "an op too big to ship" everywhere it ships
/// (the opening-message side lives in the app's sibling const).
const REFUSED_STASH_CEILING_BYTES: usize = 4_096;

/// Compact-JSON byte length of `value`, without rendering it.
///
/// Core holds no `serde_json` dependency (`IN2B` §0.4 d6), so the d5 elision
/// cannot call `to_string`; this counting serializer reproduces compact
/// `serde_json` output's length instead. Exact for nulls, bools, integers,
/// strings (via [`json_escaped_len`]), chars, options, units, sequences, maps,
/// structs and every enum shape. Floats count their Rust `Debug` length plus
/// one where `serde_json` writes a `+` after a bare `e` — and that is NOT an
/// upper bound in general (N4 F8): `serde_json` 1.0.151 renders plain decimals
/// for exponents −5..=15, so `1e-5` counts 4 (`1e-5`) against a true 7
/// (`0.00001`). Exactness holds for [`Operation`], the only caller, because it
/// carries no floats at all: it derives `Eq`, which `f32`/`f64` cannot satisfy,
/// and the `operation_has_no_float_fields` test fails to compile the day a
/// float field is added. Non-finite floats count 4, the `null` `serde_json`
/// writes. Byte slices cannot occur in an [`Operation`] either (no byte field
/// anywhere in it); that arm counts a bounded array-of-numbers fallback.
/// A serialisation failure — unreachable, every arm below is infallible —
/// degrades to `usize::MAX`, the elide side.
fn compact_json_len(value: &impl Serialize) -> usize {
    let mut counter = JsonLenCounter { len: 0 };
    match value.serialize(&mut counter) {
        Ok(()) => counter.len,
        Err(JsonLenError) => usize::MAX,
    }
}

/// The counting serializer's error: unconstructable outside this module,
/// required by the trait.
#[derive(Debug)]
struct JsonLenError;

impl core::fmt::Display for JsonLenError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("json length counting cannot fail")
    }
}

impl core::error::Error for JsonLenError {}

impl ser::Error for JsonLenError {
    fn custom<T>(message: T) -> Self
    where
        T: core::fmt::Display,
    {
        let _ = message;
        Self
    }
}

/// Counts the bytes `serde_json`'s compact output would take.
struct JsonLenCounter {
    len: usize,
}

impl JsonLenCounter {
    fn push(&mut self, bytes: usize) {
        self.len = self.len.saturating_add(bytes);
    }

    fn unsigned_len(mut value: u128) -> usize {
        let mut len = 1;
        while value >= 10 {
            value /= 10;
            len += 1;
        }
        len
    }

    fn signed_len(value: i128) -> usize {
        Self::unsigned_len(value.unsigned_abs()) + usize::from(value.is_negative())
    }

    fn float_len(value: f64) -> usize {
        if !value.is_finite() {
            return 4;
        }
        let rendered = format!("{value:?}");
        // `serde_json` writes a `+` after a bare `e` where `Debug` writes
        // none (`1e+300` vs `1e300`).
        let plus = usize::from(
            rendered.contains('e') && !rendered.contains("e+") && !rendered.contains("e-"),
        );
        rendered.len() + plus
    }

    fn quoted_len(text: &str) -> usize {
        json_escaped_len(text).saturating_add(2)
    }
}

/// Counts one `[...]` (or the sequence half of a tuple variant).
struct SeqLenCounter<'a> {
    inner: &'a mut JsonLenCounter,
    first: bool,
    /// Closing bytes beyond `]`: 1 normally, 2 for a tuple variant's `]}`.
    tail: usize,
}

impl SeqLenCounter<'_> {
    fn element(&mut self) {
        if !self.first {
            self.inner.push(1);
        }
        self.first = false;
    }

    fn end(self) {
        self.inner.push(self.tail);
    }
}

impl ser::SerializeSeq for SeqLenCounter<'_> {
    type Ok = ();
    type Error = JsonLenError;

    fn serialize_element<T>(&mut self, value: &T) -> Result<(), Self::Error>
    where
        T: ?Sized + Serialize,
    {
        self.element();
        value.serialize(&mut *self.inner)
    }

    fn end(self) -> Result<(), Self::Error> {
        self.end();
        Ok(())
    }
}

impl ser::SerializeTuple for SeqLenCounter<'_> {
    type Ok = ();
    type Error = JsonLenError;

    fn serialize_element<T>(&mut self, value: &T) -> Result<(), Self::Error>
    where
        T: ?Sized + Serialize,
    {
        self.element();
        value.serialize(&mut *self.inner)
    }

    fn end(self) -> Result<(), Self::Error> {
        self.end();
        Ok(())
    }
}

impl ser::SerializeTupleStruct for SeqLenCounter<'_> {
    type Ok = ();
    type Error = JsonLenError;

    fn serialize_field<T>(&mut self, value: &T) -> Result<(), Self::Error>
    where
        T: ?Sized + Serialize,
    {
        self.element();
        value.serialize(&mut *self.inner)
    }

    fn end(self) -> Result<(), Self::Error> {
        self.end();
        Ok(())
    }
}

impl ser::SerializeTupleVariant for SeqLenCounter<'_> {
    type Ok = ();
    type Error = JsonLenError;

    fn serialize_field<T>(&mut self, value: &T) -> Result<(), Self::Error>
    where
        T: ?Sized + Serialize,
    {
        self.element();
        value.serialize(&mut *self.inner)
    }

    fn end(self) -> Result<(), Self::Error> {
        self.end();
        Ok(())
    }
}

/// Counts one `{...}` (or the mapping half of a struct variant).
struct MapLenCounter<'a> {
    inner: &'a mut JsonLenCounter,
    first: bool,
    /// Closing bytes beyond `}`: 1 normally, 2 for a struct variant's `}}`.
    tail: usize,
}

impl MapLenCounter<'_> {
    fn key(&mut self) {
        if !self.first {
            self.inner.push(1);
        }
        self.first = false;
    }

    fn end(self) {
        self.inner.push(self.tail);
    }
}

impl ser::SerializeMap for MapLenCounter<'_> {
    type Ok = ();
    type Error = JsonLenError;

    fn serialize_key<T>(&mut self, key: &T) -> Result<(), Self::Error>
    where
        T: ?Sized + Serialize,
    {
        self.key();
        key.serialize(&mut *self.inner)
    }

    fn serialize_value<T>(&mut self, value: &T) -> Result<(), Self::Error>
    where
        T: ?Sized + Serialize,
    {
        self.inner.push(1);
        value.serialize(&mut *self.inner)
    }

    fn end(self) -> Result<(), Self::Error> {
        self.end();
        Ok(())
    }
}

impl ser::SerializeStruct for MapLenCounter<'_> {
    type Ok = ();
    type Error = JsonLenError;

    fn serialize_field<T>(&mut self, key: &'static str, value: &T) -> Result<(), Self::Error>
    where
        T: ?Sized + Serialize,
    {
        self.key();
        self.inner
            .push(JsonLenCounter::quoted_len(key).saturating_add(1));
        value.serialize(&mut *self.inner)
    }

    fn end(self) -> Result<(), Self::Error> {
        self.end();
        Ok(())
    }
}

impl ser::SerializeStructVariant for MapLenCounter<'_> {
    type Ok = ();
    type Error = JsonLenError;

    fn serialize_field<T>(&mut self, key: &'static str, value: &T) -> Result<(), Self::Error>
    where
        T: ?Sized + Serialize,
    {
        self.key();
        self.inner
            .push(JsonLenCounter::quoted_len(key).saturating_add(1));
        value.serialize(&mut *self.inner)
    }

    fn end(self) -> Result<(), Self::Error> {
        self.end();
        Ok(())
    }
}

impl<'counter> Serializer for &'counter mut JsonLenCounter {
    type Ok = ();
    type Error = JsonLenError;
    type SerializeSeq = SeqLenCounter<'counter>;
    type SerializeTuple = SeqLenCounter<'counter>;
    type SerializeTupleStruct = SeqLenCounter<'counter>;
    type SerializeTupleVariant = SeqLenCounter<'counter>;
    type SerializeMap = MapLenCounter<'counter>;
    type SerializeStruct = MapLenCounter<'counter>;
    type SerializeStructVariant = MapLenCounter<'counter>;

    fn serialize_bool(self, value: bool) -> Result<(), Self::Error> {
        self.push(usize::from(!value) + 4); // "true" = 4, "false" = 5
        Ok(())
    }

    fn serialize_i8(self, value: i8) -> Result<(), Self::Error> {
        self.push(JsonLenCounter::signed_len(i128::from(value)));
        Ok(())
    }

    fn serialize_i16(self, value: i16) -> Result<(), Self::Error> {
        self.push(JsonLenCounter::signed_len(i128::from(value)));
        Ok(())
    }

    fn serialize_i32(self, value: i32) -> Result<(), Self::Error> {
        self.push(JsonLenCounter::signed_len(i128::from(value)));
        Ok(())
    }

    fn serialize_i64(self, value: i64) -> Result<(), Self::Error> {
        self.push(JsonLenCounter::signed_len(i128::from(value)));
        Ok(())
    }

    fn serialize_i128(self, value: i128) -> Result<(), Self::Error> {
        self.push(JsonLenCounter::signed_len(value));
        Ok(())
    }

    fn serialize_u8(self, value: u8) -> Result<(), Self::Error> {
        self.push(JsonLenCounter::unsigned_len(u128::from(value)));
        Ok(())
    }

    fn serialize_u16(self, value: u16) -> Result<(), Self::Error> {
        self.push(JsonLenCounter::unsigned_len(u128::from(value)));
        Ok(())
    }

    fn serialize_u32(self, value: u32) -> Result<(), Self::Error> {
        self.push(JsonLenCounter::unsigned_len(u128::from(value)));
        Ok(())
    }

    fn serialize_u64(self, value: u64) -> Result<(), Self::Error> {
        self.push(JsonLenCounter::unsigned_len(u128::from(value)));
        Ok(())
    }

    fn serialize_u128(self, value: u128) -> Result<(), Self::Error> {
        self.push(JsonLenCounter::unsigned_len(value));
        Ok(())
    }

    fn serialize_f32(self, value: f32) -> Result<(), Self::Error> {
        self.push(JsonLenCounter::float_len(f64::from(value)));
        Ok(())
    }

    fn serialize_f64(self, value: f64) -> Result<(), Self::Error> {
        self.push(JsonLenCounter::float_len(value));
        Ok(())
    }

    fn serialize_char(self, value: char) -> Result<(), Self::Error> {
        let mut encoded = [0_u8; 4];
        self.push(JsonLenCounter::quoted_len(value.encode_utf8(&mut encoded)));
        Ok(())
    }

    fn serialize_str(self, value: &str) -> Result<(), Self::Error> {
        self.push(JsonLenCounter::quoted_len(value));
        Ok(())
    }

    fn serialize_bytes(self, value: &[u8]) -> Result<(), Self::Error> {
        // Unreachable for `Operation` (no byte field); bounded as an array of
        // numbers so the count stays an upper bound rather than a lie.
        self.push(value.len().saturating_mul(4).saturating_add(2));
        Ok(())
    }

    fn serialize_none(self) -> Result<(), Self::Error> {
        self.push(4);
        Ok(())
    }

    fn serialize_some<T>(self, value: &T) -> Result<(), Self::Error>
    where
        T: ?Sized + Serialize,
    {
        value.serialize(self)
    }

    fn serialize_unit(self) -> Result<(), Self::Error> {
        self.push(4);
        Ok(())
    }

    fn serialize_unit_struct(self, _name: &'static str) -> Result<(), Self::Error> {
        self.push(4);
        Ok(())
    }

    fn serialize_unit_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
    ) -> Result<(), Self::Error> {
        self.push(JsonLenCounter::quoted_len(variant));
        Ok(())
    }

    fn serialize_newtype_struct<T>(self, _name: &'static str, value: &T) -> Result<(), Self::Error>
    where
        T: ?Sized + Serialize,
    {
        value.serialize(self)
    }

    fn serialize_newtype_variant<T>(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        value: &T,
    ) -> Result<(), Self::Error>
    where
        T: ?Sized + Serialize,
    {
        self.push(JsonLenCounter::quoted_len(variant).saturating_add(2));
        value.serialize(&mut *self)?;
        self.push(1);
        Ok(())
    }

    fn serialize_seq(self, _len: Option<usize>) -> Result<Self::SerializeSeq, Self::Error> {
        self.push(1);
        Ok(SeqLenCounter {
            inner: self,
            first: true,
            tail: 1,
        })
    }

    fn serialize_tuple(self, _len: usize) -> Result<Self::SerializeTuple, Self::Error> {
        self.push(1);
        Ok(SeqLenCounter {
            inner: self,
            first: true,
            tail: 1,
        })
    }

    fn serialize_tuple_struct(
        self,
        _name: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeTupleStruct, Self::Error> {
        self.push(1);
        Ok(SeqLenCounter {
            inner: self,
            first: true,
            tail: 1,
        })
    }

    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeTupleVariant, Self::Error> {
        self.push(JsonLenCounter::quoted_len(variant).saturating_add(3));
        Ok(SeqLenCounter {
            inner: self,
            first: true,
            tail: 2,
        })
    }

    fn serialize_map(self, _len: Option<usize>) -> Result<Self::SerializeMap, Self::Error> {
        self.push(1);
        Ok(MapLenCounter {
            inner: self,
            first: true,
            tail: 1,
        })
    }

    fn serialize_struct(
        self,
        _name: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeStruct, Self::Error> {
        self.push(1);
        Ok(MapLenCounter {
            inner: self,
            first: true,
            tail: 1,
        })
    }

    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeStructVariant, Self::Error> {
        self.push(JsonLenCounter::quoted_len(variant).saturating_add(3));
        Ok(MapLenCounter {
            inner: self,
            first: true,
            tail: 2,
        })
    }

    fn collect_str<T>(self, value: &T) -> Result<(), Self::Error>
    where
        T: ?Sized + core::fmt::Display,
    {
        use core::fmt::Write as _;
        let mut rendered = String::new();
        let _ = write!(rendered, "{value}");
        self.push(JsonLenCounter::quoted_len(&rendered));
        Ok(())
    }
}

/// The session's incidents, their dedup state and their suppression set.
///
/// Session state: it is never persisted, and the only durable residue of a
/// resolved incident is the asset's written `color_description` and
/// `MediaAsset::assumed_from` (IN1 §1 item 3, §2.3c rule 30). Entries persist
/// as sidecar records (§2); names are captured evidence, not a document view.
#[derive(Debug)]
pub struct IncidentLog {
    started: Instant,
    next_id: u64,
    entries: Vec<Incident>,
    suppressed: BTreeSet<(IncidentCode, IncidentSubject)>,
    /// Mutations performed through this log's writers, from zero. The sidecar
    /// flusher's change signal: [`Self::generation`] reads it and the pure
    /// [`should_flush`] compares it (`IN2B` §2 rule 4).
    generation: u64,
    /// The wall-clock reading of [`Self::started`], injected by the caller
    /// (`IN2B` §3 rule 14, N1/B4). `None` for origin-less logs (tests,
    /// handles that never write a sidecar): wall stamps derive only where
    /// an origin exists, and no test reads a clock.
    wall_origin: Option<SystemTime>,
    /// Carried wall stamps by id, filled by `restore` from each record's
    /// `opened_wall_millis` (`IN2B` §3 rule 14, N2/B-2). Off the wire and
    /// consulted only at sidecar write, where the builder prefers a carried
    /// wall over re-deriving one — a new origin must never make a 3-day-old
    /// incident read as minutes old.
    loaded_wall: BTreeMap<IncidentId, Option<i64>>,
}

impl Default for IncidentLog {
    fn default() -> Self {
        Self::with_start(Instant::now(), None)
    }
}

/// Whether the log holds changes the sidecar does not (`IN2B` §2 rule 4).
///
/// Pure over generations: `last_change` is [`IncidentLog::generation`] as the
/// flusher last saw it, `last_written_gen` the generation the last flush
/// wrote. The 2 s debounce lives in the caller's timer (edge-triggered: the
/// timer fires after a change and asks here whether anything is new), so this
/// comparison reads no clock and `now` is unused — it stays in the signature
/// because the rule fixes the call shape the flush scheduler uses.
#[must_use]
pub fn should_flush(last_change: u64, last_written_gen: u64, now: Instant) -> bool {
    let _ = now;
    last_change > last_written_gen
}

impl IncidentLog {
    /// A log whose session origin is pinned, so a test can read `opened_at`
    /// deterministically (IN1 §2.3 rule 18), and whose wall reading of that
    /// origin is `wall_origin` (`IN2B` §3 rule 14, N1/B4).
    ///
    /// The origin is injected, never read: the app passes
    /// `Some(SystemTime::now())` at session creation, tests pass `None` (or
    /// a fixed origin where the wall math is under test), and wall stamps
    /// derive at sidecar write as `wall_origin + opened_at` — or `None`
    /// where no origin exists.
    #[must_use]
    pub fn with_start(started: Instant, wall_origin: Option<SystemTime>) -> Self {
        Self {
            started,
            next_id: 1,
            entries: Vec::new(),
            suppressed: BTreeSet::new(),
            generation: 0,
            wall_origin,
            loaded_wall: BTreeMap::new(),
        }
    }

    /// Mutations performed through this log's writers, from zero (`IN2B` §2
    /// rule 4, N2/S-6).
    ///
    /// Bumped by **every** `&mut` writer on the log — [`Self::observe`],
    /// [`Self::resolve`], [`Self::note_auto_applied`],
    /// [`Self::refresh_revision`], [`Self::begin_investigation`],
    /// [`Self::end_investigation`], [`Self::record_proposal`],
    /// [`Self::mark_proposal_stale`], [`Self::telemetry_mut`] and
    /// [`Self::restore`] — whenever it mutates or hands out mutation
    /// capability. Paths that change nothing (a suppressed observation, an
    /// unknown id, an already-terminal state, a restore of zero records) do
    /// not move it: the flusher asks "anything new since the last write",
    /// and a no-op is not news. The MCP threads mutate through the same
    /// handle, so they count too. `restore` bumps once per restored entry —
    /// one mutation per entry, as `observe` — and its unknown-code aggregate
    /// notes through `observe`, which bumps for itself.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// One mutation happened.
    fn bump_generation(&mut self) {
        self.generation = self.generation.saturating_add(1);
    }

    /// Whether `observe` would open for this key (`IN2B` §9 rule 1).
    ///
    /// The router peeks before reading the focused document, so subject
    /// names are captured once per open and never for dedups or
    /// suppressions. Mirrors `observe`'s two early-outs exactly —
    /// `in2b_would_open_agrees_with_observe` pins the agreement.
    #[must_use]
    pub fn would_open(&self, code: IncidentCode, subject: IncidentSubject, observed: &str) -> bool {
        if self.suppressed.contains(&(code, subject)) {
            return false;
        }
        !self.entries.iter().any(|incident| {
            incident.state.is_open()
                && incident.code == code
                && incident.subject == subject
                && incident.observed == observed
        })
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
    /// (IN1 §2.3 rule 16). An `Investigating` entry **does** absorb one: a
    /// repeat observation while a session runs must dedup, not open a second
    /// incident about the same problem, which is why the filter below reads
    /// [`IncidentState::is_open`] rather than `== Open` (IN2 §3.6 site 2). The
    /// filter is otherwise belt-and-braces: rule 19's suppression already makes a resolved entry
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
            incident.state.is_open()
                && incident.code == observation.code
                && incident.subject == observation.subject
                && incident.observed == observation.observed
        }) {
            existing.count = existing.count.saturating_add(1);
            existing.revision = observation.revision;
            // Durability absorbs: one durable observation of the key makes the
            // incident durable, whichever order the two arrive in (N4 F5).
            existing.transient &= observation.transient;
            let deduped = existing.id;
            self.bump_generation();
            return Observed::Deduped(deduped);
        }

        let id = IncidentId(self.next_id);
        self.next_id = self.next_id.saturating_add(1);
        let class = policy_class(observation.code, &observation.evidence);
        let recoveries =
            policy_recovery(observation.code, observation.subject, &observation.evidence);
        self.entries.push(Incident {
            id,
            code: observation.code,
            class,
            severity: policy_severity(observation.code),
            subject: observation.subject,
            subject_name: observation
                .name
                .as_deref()
                .map(|name| truncate_to_serialized_bytes(name, SUBJECT_NAME_CEILING_BYTES)),
            transient: observation.transient,
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
            proposal: None,
        });
        self.bump_generation();
        Observed::Opened(id)
    }

    /// Every incident that is still outstanding.
    pub fn open(&self) -> impl Iterator<Item = &Incident> {
        self.entries
            .iter()
            .filter(|incident| incident.state.is_open())
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
        if self.entries.iter().any(|incident| incident.id == id) {
            self.bump_generation();
        }
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
        self.bump_generation();
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
        self.bump_generation();
        true
    }

    /// Drop every open entry carrying `code`, returning how many went.
    ///
    /// A live-log removal, not a resolution (`IN2B` §2 rule 7, N2/B-4): Save As
    /// drops the open `project_newer_format` note because it was about the old
    /// path's bytes, and carrying it would badge a clean copy with a false
    /// card. No `Resolved` is recorded, no suppression is written, and resolved
    /// entries with the code are left alone. Carried walls for dropped ids go
    /// with them. Bumps the generation when anything was dropped, like every
    /// `&mut` writer.
    pub fn remove_open_with_code(&mut self, code: IncidentCode) -> usize {
        let before = self.entries.len();
        self.entries
            .retain(|entry| !(entry.state.is_open() && entry.code == code));
        let dropped = before - self.entries.len();
        if dropped > 0 {
            let live: BTreeSet<IncidentId> = self.entries.iter().map(|entry| entry.id).collect();
            self.loaded_wall.retain(|id, _| live.contains(id));
            self.bump_generation();
        }
        dropped
    }

    /// Point an open incident at the revision it is now stated against.
    ///
    /// The third narrowly typed writer, beside [`Self::telemetry_mut`] and
    /// [`Self::note_auto_applied`], and like them it writes exactly one thing:
    /// `revision`, and only on an entry that is still outstanding — `Open` or
    /// `Investigating`, through [`IncidentState::is_open`], so IN1 §5.2
    /// rule 10's conflict refresh keeps working for an investigated incident
    /// (IN2 §3.6 site 5). `count`, `suppressed`, `state`, `opened_at` and
    /// telemetry are untouched. Returns `false`, changing nothing, for an
    /// unknown id or a `Resolved` entry.
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
        if !incident.state.is_open() {
            return false;
        }
        incident.revision = revision;
        self.bump_generation();
        true
    }

    /// Mark an incident as being investigated.
    ///
    /// The fourth narrowly typed writer, in the shape of
    /// [`Self::note_auto_applied`] and [`Self::refresh_revision`]: it writes
    /// `state = IncidentState::Investigating` and **only** on an `Open` entry,
    /// returning `false` and changing nothing otherwise. A session that fails
    /// to begin because the incident is no longer `Open` never starts
    /// (IN2 §3.6 rule 35, §3.7 rules 41–42).
    ///
    /// **It writes one more thing, and erratum A-R7 is why.** An incident that
    /// was investigated, returned to `Open` by [`Self::end_investigation`] and
    /// is now being re-investigated still carries the previous session's
    /// proposal, proved on a branch seeded at an older live revision. Leaving
    /// it as it was would let the card offer **Approve** on a
    /// `base_revision` two sessions old, which IN1 §5.3 rule 29 forbids — a
    /// button that cannot honestly succeed. So `begin_investigation` sets
    /// `proposal.stale = true` on any proposal it finds: a new session makes a
    /// new proposal, and until it does the card offers **Re-investigate**
    /// (IN2 §4.3 rule 15). It does **not** clear `telemetry.resolver`, which is
    /// the previous session's history and the only record of why it stopped;
    /// the card gates §3.7 rule 39's *"stopped"* row on `state == Open`
    /// instead. `suppressed`, `count`, `opened_at` and `revision` are untouched.
    pub fn begin_investigation(&mut self, id: IncidentId) -> bool {
        let Some(incident) = self.entries.iter_mut().find(|incident| incident.id == id) else {
            return false;
        };
        if incident.state != IncidentState::Open {
            return false;
        }
        incident.state = IncidentState::Investigating;
        if let Some(proposal) = incident.proposal.as_mut() {
            proposal.stale = true;
        }
        self.bump_generation();
        true
    }

    /// Return an investigated incident to [`IncidentState::Open`] and record
    /// why the session stopped.
    ///
    /// **This is the writer that suppresses nothing.** Six of the seven ends a
    /// session can have are not outcomes — a budget, the off switch, a policy
    /// violation, a disconnect, a harness failure, a hand resolution elsewhere
    /// — so they must not reach [`Self::resolve`], which inserts into
    /// `suppressed` for **every** outcome (IN1 §2.3 rule 19, unamended). An
    /// end that silenced the problem it was investigating is the regression
    /// IN2 §3.7 rule 38 exists to prevent.
    ///
    /// Writes `state = IncidentState::Open` and `stopped_reason` onto
    /// `telemetry.resolver`, and **only** on an `Investigating` entry
    /// (IN2 §0.4 p, §3.6 rule 35). `suppressed` is untouched.
    pub fn end_investigation(&mut self, id: IncidentId, stopped_reason: IncidentResolver) -> bool {
        let Some(incident) = self.entries.iter_mut().find(|incident| incident.id == id) else {
            return false;
        };
        if incident.state != IncidentState::Investigating {
            return false;
        }
        incident.state = IncidentState::Open;
        incident.telemetry.resolver = Some(stopped_reason);
        self.bump_generation();
        true
    }

    /// Record one session's proposal against the incident it is for.
    ///
    /// The sixth narrowly typed writer: it writes `proposal` and **only** on an
    /// `Investigating` entry that does not already carry a live one. It does
    /// not resolve the incident and it does not change its state — the **app**
    /// applies a proposal and the **app** records every outcome
    /// (IN2 §4.1 rules 4, 7).
    ///
    /// **What this writer does not check**, because IN2 §4.1 splits the six
    /// refusals between core and the `propose_fix` handler: the
    /// `INVESTIGATOR_MAX_PROPOSAL_OPERATIONS` cap (§4.1 rule 6 code 4), the
    /// empty-proposal refusal (code 3) and the destructive-operation check
    /// (code 5, §6 rule 3) are all the handler's, run over the branch's applied
    /// list **before** anything is recorded. Core's backstop is the
    /// already-recorded refusal (code 6), because only the log knows what it is
    /// already holding; the handler maps [`RecordProposalError`] onto its own
    /// `incident_not_found`, `incident_not_investigating` and
    /// `proposal_already_recorded` codes (erratum A-R7).
    ///
    /// A **stale** proposal is not a live one: [`Self::begin_investigation`]
    /// marks the previous session's proposal stale, so a re-investigation's
    /// `propose_fix` replaces it rather than being refused.
    ///
    /// # Errors
    ///
    /// Returns [`RecordProposalError`] for an unknown id, a non-`Investigating`
    /// entry, or an entry already carrying a non-stale proposal. Nothing is
    /// written in any of the three cases.
    pub fn record_proposal(
        &mut self,
        id: IncidentId,
        proposal: IncidentProposal,
    ) -> Result<(), RecordProposalError> {
        let Some(incident) = self.entries.iter_mut().find(|incident| incident.id == id) else {
            return Err(RecordProposalError::NotFound);
        };
        if incident.state != IncidentState::Investigating {
            return Err(RecordProposalError::NotInvestigating);
        }
        if incident
            .proposal
            .as_ref()
            .is_some_and(|existing| !existing.stale)
        {
            return Err(RecordProposalError::AlreadyRecorded);
        }
        incident.proposal = Some(proposal);
        self.bump_generation();
        Ok(())
    }

    /// Mark the entry's proposal stale without touching anything else.
    ///
    /// The seventh narrow writer: the app calls it when an approval fails
    /// terminally — the proposal conflicted twice against the live timeline,
    /// or the branch comparison failed — so the card stops offering Approve
    /// and offers Re-investigate instead (IN2 §4.4 rule 19, lead ruling §8.2).
    /// True when the entry exists, is `Open` or `Investigating`, and carries
    /// a non-stale proposal; false — with nothing written — otherwise.
    pub fn mark_proposal_stale(&mut self, id: IncidentId) -> bool {
        let Some(incident) = self.entries.iter_mut().find(|incident| incident.id == id) else {
            return false;
        };
        if !matches!(
            incident.state,
            IncidentState::Open | IncidentState::Investigating
        ) {
            return false;
        }
        let Some(proposal) = incident.proposal.as_mut() else {
            return false;
        };
        if proposal.stale {
            return false;
        }
        proposal.stale = true;
        self.bump_generation();
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

    /// Build the sidecar records for this log (`IN2B` §2 rules 3, 11–12, 14).
    ///
    /// Open entries with transient provenance are dropped and counted, never
    /// persisted (§2 rule 12 — a per-run note persisted `Open` would badge a
    /// clean reopen with a false card); resolved transient entries persist
    /// normally, they are history, not state. Resolved entries beyond the most
    /// recent 256 by `IncidentId` are pruned oldest-first (§2 rule 11); open
    /// entries are never pruned. An `Investigating` entry is written as `Open`
    /// with the running session's counters flushed (§3 rule 4). Records emit
    /// in log order, so the write is deterministic over the log.
    ///
    /// `running` carries the still-running session, when the app has one;
    /// `refused` maps incident ids to their refused opening-context operation,
    /// stashed per record iff it fits the d5 ceiling (§3 rule 13).
    #[must_use]
    pub fn records(
        &self,
        running: Option<&RunningInvestigation>,
        refused: &BTreeMap<IncidentId, Operation>,
    ) -> (Vec<IncidentRecord>, WriteReport) {
        let mut resolved_ids: Vec<IncidentId> = self
            .entries
            .iter()
            .filter(|entry| matches!(entry.state, IncidentState::Resolved(_)))
            .map(|entry| entry.id)
            .collect();
        resolved_ids.sort();
        let pruned: BTreeSet<IncidentId> = resolved_ids
            .iter()
            .take(resolved_ids.len().saturating_sub(MAX_PERSISTED_RESOLVED))
            .copied()
            .collect();
        let mut records = Vec::new();
        let mut report = WriteReport {
            written_open: 0,
            written_resolved: 0,
            dropped_transient_open: 0,
            pruned_resolved: 0,
        };
        for entry in &self.entries {
            if entry.state.is_open() {
                if entry.transient {
                    report.dropped_transient_open += 1;
                    continue;
                }
                records.push(self.record_for_entry(entry, running, refused.get(&entry.id)));
                report.written_open += 1;
            } else if pruned.contains(&entry.id) {
                report.pruned_resolved += 1;
            } else {
                records.push(self.record_for_entry(entry, running, refused.get(&entry.id)));
                report.written_resolved += 1;
            }
        }
        (records, report)
    }

    /// One record for one entry: verbatim evidence, derived nothing.
    fn record_for_entry(
        &self,
        entry: &Incident,
        running: Option<&RunningInvestigation>,
        refused_op: Option<&Operation>,
    ) -> IncidentRecord {
        let (state, mut telemetry) = Self::record_state_and_telemetry(entry, running);
        // The record never carries it (N-9): log-origin-relative, meaningless
        // across runs. Cleared here, in the builder — not with `#[serde(skip)]`,
        // which would also strip it from the served wire payload (N4 F1).
        telemetry.resolved_after = None;
        IncidentRecord {
            id: entry.id,
            code: entry.code.code().to_owned(),
            subject: entry.subject,
            observed: entry.observed.clone(),
            allowed: entry.allowed.clone(),
            evidence: entry.evidence.clone(),
            revision: entry.revision,
            opened_wall_millis: self.record_wall_millis(entry),
            opened_offset_nanos: u64::try_from(entry.opened_at.as_nanos()).unwrap_or(u64::MAX),
            count: entry.count,
            state,
            telemetry,
            proposal: entry.proposal.clone().map(|mut proposal| {
                // Belt and braces with the loader: no record carries a live
                // proposal, even before the loader forces it (§3 rule 7).
                proposal.stale = true;
                proposal.operations.clear();
                proposal
            }),
            subject_name: entry.subject_name.clone(),
            refused_op: refused_op
                .filter(|op| compact_json_len(op) <= REFUSED_STASH_CEILING_BYTES)
                .cloned(),
        }
    }

    /// The record's state and telemetry for one entry (`IN2B` §2 rule 14, §3
    /// rule 4).
    ///
    /// Settled entries keep their state with the stop categorised; an
    /// `Investigating` entry becomes `Open` with the interrupted stop, the
    /// running session's turns and mirrors flushed — or the defensive
    /// empty-harness resolver when no session runs behind it.
    fn record_state_and_telemetry(
        entry: &Incident,
        running: Option<&RunningInvestigation>,
    ) -> (IncidentState, IncidentTelemetry) {
        if !matches!(entry.state, IncidentState::Investigating) {
            let mut telemetry = entry.telemetry.clone();
            telemetry.resolver = telemetry.resolver.map(Self::persisted_resolver);
            return (entry.state, telemetry);
        }
        let mut telemetry = entry.telemetry.clone();
        let (harness, model) = match running.filter(|run| run.id == entry.id) {
            Some(run) => {
                telemetry.turns = Some(run.turns);
                telemetry.input_tokens = run.input_tokens;
                telemetry.cached_input_tokens = run.cached_input_tokens;
                telemetry.cache_creation_input_tokens = run.cache_creation_input_tokens;
                telemetry.output_tokens = run.output_tokens;
                telemetry.reasoning_output_tokens = run.reasoning_output_tokens;
                telemetry.cost_usd_millionths = run.cost_usd_millionths;
                (run.harness.clone(), run.model.clone())
            }
            None => (String::new(), None),
        };
        telemetry.resolver = Some(IncidentResolver::Session {
            harness,
            model,
            stop: INTERRUPTED_STOP.to_owned(),
        });
        (IncidentState::Open, telemetry)
    }

    /// One resolver with its stop categorised for persistence.
    fn persisted_resolver(resolver: IncidentResolver) -> IncidentResolver {
        match resolver {
            IncidentResolver::Session {
                harness,
                model,
                stop,
            } => IncidentResolver::Session {
                harness,
                model,
                stop: persisted_stop(&stop).to_owned(),
            },
            other => other,
        }
    }

    /// The record's wall stamp for one entry (`IN2B` §3 rule 14).
    ///
    /// Builder-preference: an entry present in `loaded_wall` is written with
    /// its carried millis verbatim — even `None`, which a re-derive must not
    /// resurrect — so write → restore under a new origin → write leaves every
    /// wall unchanged. Otherwise wall origin plus `opened_at`; `None` where
    /// the log has no origin or the sum predates the epoch.
    fn record_wall_millis(&self, entry: &Incident) -> Option<i64> {
        if let Some(carried) = self.loaded_wall.get(&entry.id) {
            return *carried;
        }
        let origin = self.wall_origin?;
        let wall = origin.checked_add(entry.opened_at)?;
        let millis = wall
            .duration_since(SystemTime::UNIX_EPOCH)
            .ok()?
            .as_millis();
        i64::try_from(millis).ok()
    }

    /// Restore records into this log (`IN2B` §3 rules 1–10, 13–14).
    ///
    /// Writes into this log, never swapping it; requires an empty log and runs
    /// before any note. Everything derived is recomputed from the *current*
    /// policy table and nothing derived is trusted; `revision`/`base_revision`
    /// rebase to `opening`; `opened_at` is set from the stored offset and the
    /// wall filed in `loaded_wall`. Hand-written strings restore under
    /// observe's serialised-length caps (`subject_name`, proposal
    /// summary/explanation); `observed` has no cap at observe and restores
    /// verbatim (N4 F6). `next_id` resumes above every restorable id in the
    /// file — restored, unknown-code, and the `id_floor` the app computed from
    /// the raw carried values, clamped to `MAX_RESTORED_ID` — via `checked_add`,
    /// so it never overflows onto a live id (N4 F2, N4.1). Unknown code strings
    /// are skipped, counted, and reported once through a single
    /// `sidecar_unknown_codes` aggregate noted last, after the resume, so its
    /// id can neither collide with a restored id nor disturb the order.
    /// Duplicate ids keep the first record and count the rest; records past
    /// `MAX_RESTORED_ID` are skipped and counted (unknown-code ones in the
    /// unknown-codes aggregate, others in `invalid_ids`) (N4 F4, N4.1).
    /// `carried` passes through untouched. Restore enqueues nothing, resolves
    /// nothing and notifies nothing.
    ///
    /// # Panics
    ///
    /// Panics in every build, not just debug, when the log is non-empty:
    /// restore runs once, into an empty log, before any note (N4 F7).
    #[must_use]
    pub fn restore(
        &mut self,
        records: Vec<IncidentRecord>,
        carried: Vec<String>,
        opening: TimelineRevision,
        id_floor: Option<u64>,
    ) -> RestoreReport {
        assert!(
            self.entries.is_empty(),
            "restore runs once, into an empty log, before any note"
        );
        let mut report = RestoreReport {
            restored_open: 0,
            restored_resolved: 0,
            unknown_codes: Vec::new(),
            rebased: 0,
            carried,
            refused: BTreeMap::new(),
            invalid_ids: 0,
        };
        // The id-relevant scan of the records, collected for the pure
        // resume below: `(id, code_known)`. Unknown-code records always reach
        // it (they count toward the aggregate whether or not their id is
        // restorable); over-ceiling and duplicate known ids never do — the
        // first record with an id wins (N4 F4, N4.1).
        let mut id_inputs: Vec<(u64, bool)> = Vec::new();
        let mut seen: BTreeSet<u64> = BTreeSet::new();
        for stored in records {
            let id = stored.id.0;
            let over_ceiling = id > MAX_RESTORED_ID;
            let Some(code) = from_code(&stored.code) else {
                // Unknown-code records count in the aggregate whether or not
                // they are restorable — but only restorable ids count toward
                // the resume (N4.1).
                Self::count_unknown(&mut report.unknown_codes, stored.code);
                id_inputs.push((id, false));
                continue;
            };
            // Past the ceiling, or a duplicate: skipped and counted (N4 F4,
            // N4.1). The first record with an id wins.
            if over_ceiling || !seen.insert(id) {
                report.invalid_ids += 1;
                continue;
            }
            id_inputs.push((id, true));
            self.restore_one(code, stored, opening, &mut report);
        }
        let resume = restore_id_resume(&id_inputs, id_floor);
        if let Some(next) = resume.next_id {
            self.next_id = next;
        }
        if resume.unknown_skipped > 0 {
            let mut aggregate = IncidentObservation::plain(
                IncidentCode::Label(LabelIncident::SidecarUnknownCodes),
                IncidentSubject::Project,
                format!(
                    "{UNKNOWN_CODES_OBSERVED_PREFIX}{} skipped",
                    resume.unknown_skipped
                ),
                opening,
            );
            // Every note with a §5 code is transient (§5 rule 1): the load
            // re-aggregates what the next write drops.
            aggregate.transient = true;
            let _ = self.observe(aggregate);
        }
        report
    }

    /// One record into one entry: recompute, rebase, file, count.
    #[allow(
        clippy::large_types_passed_by_value,
        reason = "`restore` consumes its records by contract signature; moving \
                  each record into its entry avoids cloning evidence the caller \
                  can never reuse"
    )]
    fn restore_one(
        &mut self,
        code: IncidentCode,
        record: IncidentRecord,
        opening: TimelineRevision,
        report: &mut RestoreReport,
    ) {
        let mut telemetry = record.telemetry;
        // Rule 5: a record-`Investigating` (hand-written only — the writer
        // never emits it) becomes `Open` with the rule-4 stop, keeping a
        // hand-written harness identity when the record carries one.
        let state = match record.state {
            IncidentState::Investigating => {
                let (harness, model) = match telemetry.resolver.take() {
                    Some(IncidentResolver::Session { harness, model, .. }) => (harness, model),
                    _ => (String::new(), None),
                };
                telemetry.resolver = Some(IncidentResolver::Session {
                    harness,
                    model,
                    stop: INTERRUPTED_STOP.to_owned(),
                });
                IncidentState::Open
            }
            state => state,
        };
        // The record never carries it (N-9): `None` until re-resolved.
        telemetry.resolved_after = None;
        if state.is_open() {
            report.restored_open += 1;
        } else {
            report.restored_resolved += 1;
        }
        // Rule 9: rebase to the opening revision, counting every field that
        // moved; rule 7: every loaded proposal is stale with no operations.
        // Hand-written strings restore under the caps the live paths enforce
        // (N4 F6): the 240 B ceiling `propose_fix` truncates to.
        let mut rebased = usize::from(record.revision != opening);
        let proposal = record.proposal.map(|mut proposal| {
            rebased += usize::from(proposal.base_revision != opening);
            proposal.base_revision = opening;
            proposal.stale = true;
            proposal.operations.clear();
            proposal.summary = truncate_to_serialized_bytes(
                &proposal.summary,
                INVESTIGATOR_EXPLANATION_CEILING_BYTES,
            );
            proposal.explanation = truncate_to_serialized_bytes(
                &proposal.explanation,
                INVESTIGATOR_EXPLANATION_CEILING_BYTES,
            );
            proposal
        });
        report.rebased += rebased;
        self.loaded_wall
            .insert(record.id, record.opened_wall_millis);
        if let Some(refused_op) = record.refused_op {
            report.refused.insert(record.id, refused_op);
        }
        // Recomputed from the current policy table, exactly as `observe` does
        // (§3 rule 2) — before the record's fields move into the entry.
        let class = policy_class(code, &record.evidence);
        let recoveries = policy_recovery(code, record.subject, &record.evidence);
        self.entries.push(Incident {
            id: record.id,
            code,
            class,
            severity: policy_severity(code),
            subject: record.subject,
            // Hand-written names restore under observe's 64 B cap (N4 F6).
            subject_name: record
                .subject_name
                .map(|name| truncate_to_serialized_bytes(&name, SUBJECT_NAME_CEILING_BYTES)),
            // Consumed at write: restored entries are never transient (§2
            // rule 12 — transient opens never reach a record).
            transient: false,
            field: code.field(),
            observed: record.observed,
            allowed: record.allowed,
            evidence: record.evidence,
            recoveries,
            revision: opening,
            opened_at: Duration::from_nanos(record.opened_offset_nanos),
            count: record.count,
            state,
            telemetry,
            proposal,
        });
        self.bump_generation();
    }

    /// Count one unknown code string in first-seen order (§3 rule 6).
    fn count_unknown(unknown_codes: &mut Vec<(String, usize)>, code: String) {
        if let Some(entry) = unknown_codes.iter_mut().find(|entry| entry.0 == code) {
            entry.1 += 1;
        } else {
            unknown_codes.push((code, 1));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        BinId, ColorSourceProfile, Document, InvestigatorPreferences, MediaAsset, MediaKind,
        MediaSourceFingerprint, Rational, SourceColorRefusal, TimeCode, Track, TrackKind,
        classify_source_with_assumption,
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
        IncidentObservation {
            code: IncidentCode::SourceColor(SourceColorIncident::from_source_error(error)),
            subject: IncidentSubject::Asset(AssetId(1)),
            observed: error.observed(),
            allowed: Some(error.allowed_values().to_owned()),
            evidence: IncidentEvidence::SourceColor {
                probed: probed.clone(),
                assumption: None,
            },
            revision: TimelineRevision(1),
            name: None,
            transient: false,
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

    /// One arm per code with no wildcard in either position: a seventy-fifth
    /// code cannot compile until `POLICY` grows a row for it (IN1 §2.4 rule 35,
    /// erratum `IN1b`-R15).
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
                SourceColorIncident::UnknownWhitePoint => 12,
            },
            IncidentCode::Media(incident) => match incident {
                MediaIncident::UnsupportedDecoderFormat => 13,
                MediaIncident::BackendUnclassified => 14,
            },
            IncidentCode::DeliveryColor(incident) => match incident {
                DeliveryColorIncident::UnsupportedCodec => 15,
                DeliveryColorIncident::UnsupportedField => 16,
                DeliveryColorIncident::PixelFormatDepthMismatch => 17,
                DeliveryColorIncident::EncoderPixelFormatUnavailable => 18,
            },
            IncidentCode::DeliveryVerification(incident) => match incident {
                DeliveryVerificationIncident::NotFullResolution => 19,
                DeliveryVerificationIncident::PlaneOutOfContainer => 20,
                DeliveryVerificationIncident::FrameCountMismatch => 21,
                DeliveryVerificationIncident::FrameCountOutOfRange => 22,
                DeliveryVerificationIncident::BudgetLaneMismatch => 23,
            },
            IncidentCode::ColorQc(incident) => match incident {
                ColorQcIncident::ProxyProofRefused => 24,
                ColorQcIncident::RasterLengthMismatch => 25,
                ColorQcIncident::EmptyPopulation => 26,
                ColorQcIncident::NodeBudgetExceeded => 27,
                ColorQcIncident::MatteRegionRasterMismatch => 28,
                ColorQcIncident::NodeRemovalRejected => 29,
            },
            IncidentCode::Operation(family) => match family {
                IncidentFamily::Bounds => 30,
                IncidentFamily::Malformed => 31,
                IncidentFamily::Duplicate => 32,
                IncidentFamily::Placement => 33,
                IncidentFamily::Missing => 34,
                IncidentFamily::Structure => 35,
                IncidentFamily::Relink => 36,
                IncidentFamily::Unrepresentable => 37,
                IncidentFamily::UnknownName => 38,
                IncidentFamily::Internal => 39,
                IncidentFamily::ColorPolicy => 40,
            },
            IncidentCode::LutAssetPolicy => 41,
            IncidentCode::EditRevisionConflict => 42,
            IncidentCode::Rejection(incident) => match incident {
                RejectionIncident::EditPlan => 43,
                RejectionIncident::DeliveryVariant => 44,
                RejectionIncident::AgentBranch => 45,
                RejectionIncident::SourceEdit => 46,
                RejectionIncident::Relink => 47,
                RejectionIncident::ProjectSave => 48,
                RejectionIncident::CaptionPlan => 49,
            },
            IncidentCode::Label(incident) => match incident {
                LabelIncident::Operations => 50,
                LabelIncident::Look => 51,
                LabelIncident::LookIncomplete => 52,
                LabelIncident::Export => 53,
                LabelIncident::SourceMonitor => 54,
                LabelIncident::Relink => 55,
                LabelIncident::AgentBranch => 56,
                LabelIncident::TranscriptEdit => 57,
                LabelIncident::Media => 58,
                LabelIncident::MediaIncomplete => 59,
                LabelIncident::Agent => 60,
                LabelIncident::Recording => 61,
                LabelIncident::Project => 62,
                LabelIncident::Captions => 63,
                LabelIncident::Mixer => 64,
                LabelIncident::MediaCache => 65,
                LabelIncident::Timeline => 66,
                LabelIncident::PanelWorkerError => 67,
                LabelIncident::RecoveryDamage => 68,
                LabelIncident::RecoveryUnavailable => 69,
                LabelIncident::ProjectNewerFormat => 70,
                LabelIncident::SidecarUnknownCodes => 71,
                LabelIncident::SidecarRefused => 72,
                LabelIncident::SidecarWriteFailed => 73,
            },
        }
    }

    /// `IN1b` §9 clause 2: 74 rows, an explicit no-wildcard `ordinal()`, every
    /// **colour** row `Blocks`, and every row's severity equal to `IN1b` §3.6
    /// rule 31's table plus the four `IN2B` §5 `Degrades` rows. Replaces Part
    /// A's
    /// `in1_policy_covers_every_incident_code_exactly_once_and_every_row_blocks`,
    /// whose "every row `Blocks`" is erratum `IN1b`-R1a.
    #[test]
    fn in1b_policy_covers_every_incident_code_exactly_once_with_its_declared_severity() {
        assert_eq!(POLICY.len(), 74);
        let mut seen = [0_usize; 74];
        let mut blocks = 0_usize;
        let mut degrades = 0_usize;
        let mut informs = 0_usize;
        let mut auto_apply = 0_usize;
        let mut ask_first = 0_usize;
        let mut explain = 0_usize;
        for entry in POLICY {
            seen[ordinal(entry.code)] += 1;
            // The expectation is written from `IN1b` §3.6 rule 31's grouping
            // rather than read back from the row, so a row that declares the
            // wrong value fails here instead of agreeing with itself.
            let expected = match entry.code {
                IncidentCode::DeliveryVerification(_)
                | IncidentCode::ColorQc(_)
                | IncidentCode::Label(
                    LabelIncident::LookIncomplete
                    | LabelIncident::MediaIncomplete
                    | LabelIncident::ProjectNewerFormat
                    | LabelIncident::SidecarUnknownCodes
                    | LabelIncident::SidecarRefused
                    | LabelIncident::SidecarWriteFailed,
                ) => IncidentSeverity::Degrades,
                _ => IncidentSeverity::Blocks,
            };
            assert_eq!(
                entry.severity,
                expected,
                "{} declares the wrong severity",
                entry.code.code()
            );
            if let IncidentCode::SourceColor(_) = entry.code {
                assert_eq!(
                    entry.severity,
                    IncidentSeverity::Blocks,
                    "every colour row blocks (IN1 §2.3b rule 26)"
                );
            }
            match entry.severity {
                IncidentSeverity::Blocks => blocks += 1,
                IncidentSeverity::Degrades => degrades += 1,
                IncidentSeverity::Informs => informs += 1,
            }
            match entry.class {
                PolicyClass::AutoApply => auto_apply += 1,
                PolicyClass::AskFirst => ask_first += 1,
                PolicyClass::Explain => explain += 1,
            }
        }
        assert!(
            seen.iter().all(|count| *count == 1),
            "every code must have exactly one POLICY row, got {seen:?}"
        );
        assert_eq!((blocks, degrades, informs), (57, 17, 0));
        assert_eq!((auto_apply, ask_first, explain), (3, 0, 71));
    }

    #[test]
    fn in1_policy_rows_are_declared_in_table_order() {
        assert_eq!(POLICY.len(), 74, "erratum `IN1b`-R15");
        for (index, entry) in POLICY.iter().enumerate() {
            assert_eq!(
                ordinal(entry.code),
                index,
                "POLICY row {index} is out of IN1 §2.2 table order"
            );
            assert_eq!(policy_entry(entry.code), *entry);
        }
    }

    /// Thirteen for thirteen, with **no** `None` case (erratum `IN1b`-R4).
    #[test]
    fn in1_source_colour_incidents_mirror_the_classifier_one_to_one() {
        let mut mapped = BTreeSet::new();
        for error in every_source_error() {
            let incident = SourceColorIncident::from_source_error(&error);
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
        assert_eq!(mapped.len(), 13);
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
                // Quantified over the colour rows, because they are the rows
                // whose observation this fixture can build: the other 54 codes
                // carry no `ColorSourceError` (`IN1b` §3.2 rule 10). Their
                // `field`/`severity` agreement is asserted by
                // `in1b_every_code_delegates_its_string_to_the_accessor_that_owns_it`
                // and `in1b_policy_covers_every_incident_code_exactly_once_with_its_declared_severity`.
                let IncidentCode::SourceColor(colour) = entry.code else {
                    continue;
                };
                let mut log = IncidentLog::with_start(Instant::now(), None);
                // Built from the row's own canonical failure, so `observed` and
                // `allowed` belong to the code under test rather than to a
                // borrowed `UnknownPrimaries` fixture.
                let observation = observation_for(&colour.source_error(), &probed);
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
            assert_eq!(policy_class(entry.code, &evidence), PolicyClass::AutoApply);
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
            assert_eq!(
                policy_class(entry.code, &bt2020_evidence),
                PolicyClass::Explain
            );
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
        let mut log = IncidentLog::with_start(Instant::now(), None);

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
        let mut log = IncidentLog::with_start(Instant::now(), None);
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
        let mut log = IncidentLog::with_start(Instant::now(), None);
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
            let mut log = IncidentLog::with_start(Instant::now(), None);
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
        let mut log = IncidentLog::with_start(Instant::now(), None);
        assert!(!log.resolve(IncidentId(7), IncidentOutcome::Applied));
    }

    #[test]
    fn in1_a_noted_auto_apply_suppresses_the_undo_reopen() {
        let probed = untagged_mp4_probe();
        let mut log = IncidentLog::with_start(Instant::now(), None);
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
        let mut log = IncidentLog::with_start(Instant::now(), None);
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
        let mut log = IncidentLog::with_start(Instant::now(), None);
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
        let mut log = IncidentLog::with_start(Instant::now(), None);
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

    /// Rewritten from Part A's
    /// `in1_from_media_error_maps_only_the_asset_scoped_source_colour_refusal`:
    /// after `IN1b` §3.9 every [`MediaError`] variant carries a code, so the
    /// constructor is total and there is no `None` to assert (erratum
    /// `IN1b`-R9).
    #[test]
    // The list is the point: one arm per `MediaError` variant — fourteen of
    // fifteen (`Store` rides the shared arm unpinned since N4/CR-D1) — written
    // out so a reader can see that the constructor is total.
    #[allow(clippy::too_many_lines)]
    fn in1b_from_media_error_is_total_and_keeps_the_asset_scoped_subject() {
        let probed = untagged_mp4_probe();
        let error = MediaError::SourceColorForAsset(Box::new(SourceColorRefusal {
            asset: AssetId(1),
            path: "in1_untagged.mp4".into(),
            error: ColorSourceError::UnknownPrimaries,
            description: probed.clone(),
            assumption: None,
        }));
        // The asset-scoped refusal overrides the caller's fallback subject, and
        // every string field still comes from a core accessor.
        let observation = IncidentObservation::from_media_error(
            &error,
            IncidentSubject::Project,
            TimelineRevision(1),
        );
        assert_eq!(observation, unknown_primaries_observation(&probed));

        // The white-point refusal is the thirteenth code now, not a `None`.
        let white_point = MediaError::SourceColorForAsset(Box::new(SourceColorRefusal {
            asset: AssetId(1),
            path: "in1_tagged.mp4".into(),
            error: ColorSourceError::UnknownWhitePoint,
            description: probed.clone(),
            assumption: None,
        }));
        assert_eq!(
            IncidentObservation::from_media_error(
                &white_point,
                IncidentSubject::Project,
                TimelineRevision(1),
            )
            .code,
            IncidentCode::SourceColor(SourceColorIncident::UnknownWhitePoint)
        );

        // Every other variant takes the caller's subject and its own code; the
        // six code-less ones share `media_backend_unclassified`, and the two
        // matte enums take it too while keeping their own code on the evidence
        // (`IN1b` §3.9 rule 37, as deviated from in erratum `IN1b`-A-R2).
        for (other, expected) in [
            (
                MediaError::SourceColor(ColorSourceError::UnknownPrimaries),
                IncidentCode::SourceColor(SourceColorIncident::UnknownPrimaries),
            ),
            (
                MediaError::UnsupportedDecoderFormat {
                    path: "a.mov".into(),
                    format: "yuv422p10le".to_owned(),
                    declared_bit_depth: Some(10),
                    decoder_bit_depth: Some(10),
                    reason: "not a supported integer source surface".to_owned(),
                },
                IncidentCode::Media(MediaIncident::UnsupportedDecoderFormat),
            ),
            (
                MediaError::DeliveryColor(DeliveryColorError::UnsupportedCodec {
                    observed: "prores".to_owned(),
                    allowed: "libx264",
                }),
                IncidentCode::DeliveryColor(DeliveryColorIncident::UnsupportedCodec),
            ),
            (
                MediaError::DeliveryVerification(DeliveryVerificationError::NotFullResolution {
                    observed: "proxy".to_owned(),
                    allowed: "full",
                }),
                IncidentCode::DeliveryVerification(DeliveryVerificationIncident::NotFullResolution),
            ),
            (
                MediaError::ColorQc(ColorQcError::EmptyPopulation {
                    observed: "0 px".to_owned(),
                    allowed: "at least one pixel",
                }),
                IncidentCode::ColorQc(ColorQcIncident::EmptyPopulation),
            ),
            (
                MediaError::MatteProof(crate::MatteProofError::NoMatte),
                IncidentCode::Media(MediaIncident::BackendUnclassified),
            ),
            (
                MediaError::MatteCoverage(crate::MatteCoverageError::ArithmeticOverflow {
                    operation: "sum",
                }),
                IncidentCode::Media(MediaIncident::BackendUnclassified),
            ),
            (
                MediaError::Scope(crate::ScopeError::EmptyFrames),
                IncidentCode::Media(MediaIncident::BackendUnclassified),
            ),
            (
                MediaError::NotImplemented,
                IncidentCode::Media(MediaIncident::BackendUnclassified),
            ),
            (
                MediaError::Cancelled,
                IncidentCode::Media(MediaIncident::BackendUnclassified),
            ),
            (
                MediaError::Backend("alsa: snd_pcm_pause failed".to_owned()),
                IncidentCode::Media(MediaIncident::BackendUnclassified),
            ),
            (
                MediaError::MixSpectrumRangeTooShort {
                    sample_frames: 1,
                    required: 2,
                },
                IncidentCode::Media(MediaIncident::BackendUnclassified),
            ),
            (
                MediaError::MixLoudnessRangeTooShort {
                    sample_frames: 1,
                    required: 2,
                },
                IncidentCode::Media(MediaIncident::BackendUnclassified),
            ),
            // MO2 R10 / R29: the non-finite refusal adds no investigator code.
            (
                MediaError::NonFiniteRender {
                    layer: 1,
                    clip: Some(crate::ClipId(7)),
                    at: Some(TimeCode(3)),
                },
                IncidentCode::Media(MediaIncident::BackendUnclassified),
            ),
        ] {
            let observation = IncidentObservation::from_media_error(
                &other,
                IncidentSubject::ExportJob,
                TimelineRevision(3),
            );
            assert_eq!(observation.code, expected, "{other}");
            assert_eq!(observation.revision, TimelineRevision(3));
            if !matches!(other, MediaError::SourceColor(_)) {
                assert_eq!(observation.subject, IncidentSubject::ExportJob, "{other}");
            }
            // One incident, one code: the evidence's `code` is always the
            // incident's own (review-2 S2), and for every variant that declares
            // a code it is the engine's `recovery_code()` as well.
            let IncidentEvidence::MediaError { code, .. } = &observation.evidence else {
                panic!("a media failure carries media evidence: {other}");
            };
            assert_eq!(*code, expected.code(), "{other}");
            let matte = matches!(
                other,
                MediaError::MatteProof(_) | MediaError::MatteCoverage(_)
            );
            if matte {
                // The matte pair mints no code, so the engine's token differs
                // from the incident's — and survives in `observed`.
                assert_ne!(*code, other.recovery_code().expect("a matte code"));
                assert!(
                    observation
                        .observed
                        .starts_with(other.recovery_code().expect("a matte code")),
                    "the matte code must survive in `observed`: {}",
                    observation.observed
                );
            } else if let Some(engine) = other.recovery_code() {
                assert_eq!(*code, engine, "{other}");
            }
        }
        assert_eq!(
            MediaError::MatteProof(crate::MatteProofError::NoMatte).recovery_code(),
            Some("matte_proof_no_matte")
        );
        // MO2 R8 (B1 fix G10): `InvalidDocument` delegates its code and its
        // evidence to the document rejection it wraps, as
        // `DeliveryVariantError::InvalidDocument` does; it mints no code.
        let inner = crate::OpError::InvalidResolution;
        let delegating = MediaError::InvalidDocument(Box::new(inner.clone()));
        let observation = IncidentObservation::from_media_error(
            &delegating,
            IncidentSubject::ExportJob,
            TimelineRevision(3),
        );
        assert_eq!(observation.code, inner.incident_code());
        assert_eq!(observation.subject, IncidentSubject::ExportJob);
        assert_eq!(
            observation.evidence,
            IncidentEvidence::OpError {
                family: inner.incident_family(),
                op_number: None,
                message: inner.to_string(),
            }
        );
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

    /// Every declared code, in `POLICY`'s own order. Written out rather than
    /// read from `POLICY`, so a test that quantifies over the code set cannot
    /// be satisfied by a table that lost a row.
    const EVERY_INCIDENT_CODE: [IncidentCode; 74] = [
        IncidentCode::SourceColor(SourceColorIncident::UnknownPrimaries),
        IncidentCode::SourceColor(SourceColorIncident::UnknownTransfer),
        IncidentCode::SourceColor(SourceColorIncident::UnknownMatrix),
        IncidentCode::SourceColor(SourceColorIncident::UnknownRange),
        IncidentCode::SourceColor(SourceColorIncident::UnknownBitDepth),
        IncidentCode::SourceColor(SourceColorIncident::UnsupportedPrimaries),
        IncidentCode::SourceColor(SourceColorIncident::UnsupportedTransfer),
        IncidentCode::SourceColor(SourceColorIncident::UnsupportedMatrix),
        IncidentCode::SourceColor(SourceColorIncident::UnsupportedRange),
        IncidentCode::SourceColor(SourceColorIncident::UnsupportedWhitePoint),
        IncidentCode::SourceColor(SourceColorIncident::UnsupportedBitDepth),
        IncidentCode::SourceColor(SourceColorIncident::UnsupportedCombination),
        IncidentCode::SourceColor(SourceColorIncident::UnknownWhitePoint),
        IncidentCode::Media(MediaIncident::UnsupportedDecoderFormat),
        IncidentCode::Media(MediaIncident::BackendUnclassified),
        IncidentCode::DeliveryColor(DeliveryColorIncident::UnsupportedCodec),
        IncidentCode::DeliveryColor(DeliveryColorIncident::UnsupportedField),
        IncidentCode::DeliveryColor(DeliveryColorIncident::PixelFormatDepthMismatch),
        IncidentCode::DeliveryColor(DeliveryColorIncident::EncoderPixelFormatUnavailable),
        IncidentCode::DeliveryVerification(DeliveryVerificationIncident::NotFullResolution),
        IncidentCode::DeliveryVerification(DeliveryVerificationIncident::PlaneOutOfContainer),
        IncidentCode::DeliveryVerification(DeliveryVerificationIncident::FrameCountMismatch),
        IncidentCode::DeliveryVerification(DeliveryVerificationIncident::FrameCountOutOfRange),
        IncidentCode::DeliveryVerification(DeliveryVerificationIncident::BudgetLaneMismatch),
        IncidentCode::ColorQc(ColorQcIncident::ProxyProofRefused),
        IncidentCode::ColorQc(ColorQcIncident::RasterLengthMismatch),
        IncidentCode::ColorQc(ColorQcIncident::EmptyPopulation),
        IncidentCode::ColorQc(ColorQcIncident::NodeBudgetExceeded),
        IncidentCode::ColorQc(ColorQcIncident::MatteRegionRasterMismatch),
        IncidentCode::ColorQc(ColorQcIncident::NodeRemovalRejected),
        IncidentCode::Operation(IncidentFamily::Bounds),
        IncidentCode::Operation(IncidentFamily::Malformed),
        IncidentCode::Operation(IncidentFamily::Duplicate),
        IncidentCode::Operation(IncidentFamily::Placement),
        IncidentCode::Operation(IncidentFamily::Missing),
        IncidentCode::Operation(IncidentFamily::Structure),
        IncidentCode::Operation(IncidentFamily::Relink),
        IncidentCode::Operation(IncidentFamily::Unrepresentable),
        IncidentCode::Operation(IncidentFamily::UnknownName),
        IncidentCode::Operation(IncidentFamily::Internal),
        IncidentCode::Operation(IncidentFamily::ColorPolicy),
        IncidentCode::LutAssetPolicy,
        IncidentCode::EditRevisionConflict,
        IncidentCode::Rejection(RejectionIncident::EditPlan),
        IncidentCode::Rejection(RejectionIncident::DeliveryVariant),
        IncidentCode::Rejection(RejectionIncident::AgentBranch),
        IncidentCode::Rejection(RejectionIncident::SourceEdit),
        IncidentCode::Rejection(RejectionIncident::Relink),
        IncidentCode::Rejection(RejectionIncident::ProjectSave),
        IncidentCode::Rejection(RejectionIncident::CaptionPlan),
        IncidentCode::Label(LabelIncident::Operations),
        IncidentCode::Label(LabelIncident::Look),
        IncidentCode::Label(LabelIncident::LookIncomplete),
        IncidentCode::Label(LabelIncident::Export),
        IncidentCode::Label(LabelIncident::SourceMonitor),
        IncidentCode::Label(LabelIncident::Relink),
        IncidentCode::Label(LabelIncident::AgentBranch),
        IncidentCode::Label(LabelIncident::TranscriptEdit),
        IncidentCode::Label(LabelIncident::Media),
        IncidentCode::Label(LabelIncident::MediaIncomplete),
        IncidentCode::Label(LabelIncident::Agent),
        IncidentCode::Label(LabelIncident::Recording),
        IncidentCode::Label(LabelIncident::Project),
        IncidentCode::Label(LabelIncident::Captions),
        IncidentCode::Label(LabelIncident::Mixer),
        IncidentCode::Label(LabelIncident::MediaCache),
        IncidentCode::Label(LabelIncident::Timeline),
        IncidentCode::Label(LabelIncident::PanelWorkerError),
        IncidentCode::Label(LabelIncident::RecoveryDamage),
        IncidentCode::Label(LabelIncident::RecoveryUnavailable),
        IncidentCode::Label(LabelIncident::ProjectNewerFormat),
        IncidentCode::Label(LabelIncident::SidecarUnknownCodes),
        IncidentCode::Label(LabelIncident::SidecarRefused),
        IncidentCode::Label(LabelIncident::SidecarWriteFailed),
    ];

    /// The **eight** subject variants of `IN1b` §3.3 rule 17 as amended by
    /// erratum `IN1b`-A-R13, at their widest ids.
    ///
    /// `Chain` is taken in its `Bus` form, which is the wider of its two wire
    /// shapes; `Chain(Master)` is covered by
    /// `in1b_every_subject_variant_labels_and_serialises_in_its_declared_shape`.
    fn every_subject_shape() -> [IncidentSubject; 8] {
        [
            IncidentSubject::Asset(AssetId(u64::MAX)),
            IncidentSubject::LutAsset(LutAssetId(u64::MAX)),
            IncidentSubject::Clip(ClipId(u64::MAX)),
            IncidentSubject::Track(TrackId(u64::MAX)),
            IncidentSubject::Chain(AudioChain::Bus(crate::AudioBusId(u64::MAX))),
            IncidentSubject::ExportJob,
            IncidentSubject::Project,
            IncidentSubject::Agent,
        ]
    }

    /// `IN1b` §7 item 6 and erratum `IN1b`-R14: accessor against accessor over
    /// every delegating code, and `field` equal to `code.field()` over all 74.
    #[test]
    fn in1b_every_code_delegates_its_string_to_the_accessor_that_owns_it() {
        let mut delegating = 0_usize;
        for code in EVERY_INCIDENT_CODE {
            assert!(!code.code().is_empty());
            assert!(!code.field().is_empty());
            match code {
                IncidentCode::SourceColor(incident) => {
                    let error = incident.source_error();
                    assert_eq!(incident.code(), error.code());
                    assert_eq!(incident.field(), error.field());
                    delegating += 1;
                }
                IncidentCode::Media(MediaIncident::UnsupportedDecoderFormat) => {
                    assert_eq!(
                        code.code(),
                        MediaError::UnsupportedDecoderFormat {
                            path: std::path::PathBuf::new(),
                            format: String::new(),
                            declared_bit_depth: None,
                            decoder_bit_depth: None,
                            reason: String::new(),
                        }
                        .recovery_code()
                        .expect("the decoder-format refusal carries a recovery code")
                    );
                    delegating += 1;
                }
                IncidentCode::DeliveryColor(incident) => {
                    let error = incident.delivery_error();
                    assert_eq!(incident.code(), error.code());
                    // `field()` is minted rather than delegated, because
                    // `DeliveryColorError::field()` borrows from `&self` for
                    // `UnsupportedField` (erratum `IN1b`-A-R4). The three
                    // static arms are asserted equal to the accessor here.
                    if incident == DeliveryColorIncident::UnsupportedField {
                        assert_eq!(incident.field(), "delivery_color");
                    } else {
                        assert_eq!(incident.field(), error.field());
                    }
                    delegating += 1;
                }
                IncidentCode::DeliveryVerification(incident) => {
                    let error = incident.verification_error();
                    assert_eq!(incident.code(), error.code());
                    assert_eq!(incident.field(), error.field());
                    delegating += 1;
                }
                IncidentCode::ColorQc(incident) => {
                    let error = incident.qc_error();
                    assert_eq!(incident.code(), error.code());
                    assert_eq!(incident.field(), error.field());
                    delegating += 1;
                }
                IncidentCode::Operation(family) => assert_eq!(code.code(), family.code()),
                IncidentCode::Media(MediaIncident::BackendUnclassified)
                | IncidentCode::LutAssetPolicy
                | IncidentCode::EditRevisionConflict
                | IncidentCode::Rejection(_)
                | IncidentCode::Label(_) => {}
            }
        }
        assert_eq!(delegating, 29, "`IN1b` §3.2 rule 12's 29 existing strings");

        // Every code string is distinct, and every one opens an incident whose
        // `field` is the code's own (IN1 §2.3b rule 25, erratum `IN1b`-R14).
        let mut strings = BTreeSet::new();
        for code in EVERY_INCIDENT_CODE {
            assert!(
                strings.insert(code.code()),
                "{} is declared twice",
                code.code()
            );
            let mut log = IncidentLog::with_start(Instant::now(), None);
            let Observed::Opened(id) = log.observe(IncidentObservation::plain(
                code,
                IncidentSubject::Project,
                "observed",
                TimelineRevision(1),
            )) else {
                panic!("a fresh log must open {}", code.code());
            };
            assert_eq!(log.get(id).unwrap().field, code.field());
        }
        assert_eq!(strings.len(), 74);
    }

    /// `IN1b` §9 clause 3: a predicated row reached with no probed description
    /// falls to `Explain`.
    #[test]
    fn in1b_policy_class_falls_to_explain_without_a_probed_description() {
        let predicated = IncidentCode::SourceColor(SourceColorIncident::UnknownPrimaries);
        assert_eq!(
            policy_class(predicated, &IncidentEvidence::Plain),
            PolicyClass::Explain
        );
        // The same row with a compatible probe keeps its declared class, so the
        // fall is the evidence's doing and not the row's.
        assert_eq!(
            policy_class(
                predicated,
                &IncidentEvidence::SourceColor {
                    probed: untagged_mp4_probe(),
                    assumption: None,
                }
            ),
            PolicyClass::AutoApply
        );
        // A non-colour code classifies at all, which is the measurable reason
        // the signature changed (`IN1b` §3.5 rule 26).
        assert_eq!(
            policy_class(
                IncidentCode::Operation(IncidentFamily::Bounds),
                &IncidentEvidence::OpError {
                    family: IncidentFamily::Bounds,
                    op_number: None,
                    message: "out of range".to_owned(),
                }
            ),
            PolicyClass::Explain
        );
    }

    /// `IN1b` §3.7 rule 33: the empty-`Vec` case is legal and is what an
    /// `AutoApply` row without an asset and a probe yields.
    #[test]
    fn in1b_policy_recovery_returns_nothing_for_a_subjectless_auto_apply() {
        let code = IncidentCode::SourceColor(SourceColorIncident::UnknownPrimaries);
        let evidence = IncidentEvidence::SourceColor {
            probed: untagged_mp4_probe(),
            assumption: None,
        };
        assert_eq!(policy_class(code, &evidence), PolicyClass::AutoApply);
        assert!(policy_recovery(code, IncidentSubject::Project, &evidence).is_empty());
        assert!(policy_recovery(code, IncidentSubject::ExportJob, &evidence).is_empty());
        assert_eq!(
            policy_recovery(code, IncidentSubject::Asset(AssetId(1)), &evidence).len(),
            1
        );
    }

    /// `IN1b` §9 clause 4: every one of the 74 codes reaches a body, the 61
    /// non-colour bodies are pairwise distinct, the 13 colour bodies are each
    /// `ColorSourceError::recovery_action()`'s one sentence, and the 15
    /// non-colour delegating rows equal their own accessor's string.
    #[test]
    fn in1b_every_explain_body_covers_the_code_set_and_the_non_colour_bodies_are_distinct() {
        const SHARED_COLOUR_BODY: &str =
            "Apply an explicit supported source-colour override or relink to compatible media.";
        let mut non_colour = BTreeSet::new();
        let mut colour = 0_usize;
        let mut delegated = 0_usize;
        for code in EVERY_INCIDENT_CODE {
            let body = explain_body(code);
            assert!(!body.is_empty(), "{} has no body", code.code());
            match code {
                IncidentCode::SourceColor(_) => {
                    assert_eq!(body, SHARED_COLOUR_BODY, "{}", code.code());
                    colour += 1;
                }
                other => {
                    if let IncidentCode::DeliveryColor(incident) = other {
                        assert_eq!(body, incident.recovery_action());
                        delegated += 1;
                    } else if let IncidentCode::DeliveryVerification(incident) = other {
                        assert_eq!(body, incident.recovery_action());
                        delegated += 1;
                    } else if let IncidentCode::ColorQc(incident) = other {
                        assert_eq!(body, incident.recovery_action());
                        delegated += 1;
                    }
                    assert!(
                        non_colour.insert(body),
                        "{} shares its body with another code",
                        other.code()
                    );
                }
            }
            // The body reaches the person through the one producer, never
            // through a sentence a caller composed (IN1 §2.5 rule 45).
            let recoveries =
                policy_recovery(code, IncidentSubject::Project, &IncidentEvidence::Plain);
            assert_eq!(recoveries.len(), 1, "{}", code.code());
            assert_eq!(recoveries[0].label, "How to fix this");
            assert_eq!(recoveries[0].kind, RecoveryKind::Explain(body));
        }
        assert_eq!(colour, 13);
        assert_eq!(non_colour.len(), 61);
        assert_eq!(delegated, 15);
    }

    /// IN2B §12 item 11, §5 rules 1–3: the seven new codes carry written
    /// bodies, tail `POLICY` rows in declaration order, and the allowlist is
    /// untouched.
    ///
    /// The seven `label_headline` arms live in the app crate, so their exact
    /// strings are pinned by the re-pinned headline test there (`IN2B` §5
    /// rule 1); this test pins everything core owns — the §5 table values,
    /// the bodies, the tail rows and the allowlist slice.
    #[test]
    fn in2b_the_seven_new_codes_carry_written_bodies_headlines_and_tail_rows() {
        let seven = [
            (
                LabelIncident::PanelWorkerError,
                "panel_worker_error",
                "panel",
                IncidentSeverity::Blocks,
            ),
            (
                LabelIncident::RecoveryDamage,
                "recovery_damage",
                "recovery",
                IncidentSeverity::Blocks,
            ),
            (
                LabelIncident::RecoveryUnavailable,
                "recovery_unavailable",
                "recovery",
                IncidentSeverity::Blocks,
            ),
            (
                LabelIncident::ProjectNewerFormat,
                "project_newer_format",
                "project",
                IncidentSeverity::Degrades,
            ),
            (
                LabelIncident::SidecarUnknownCodes,
                "sidecar_unknown_codes",
                "sidecar",
                IncidentSeverity::Degrades,
            ),
            (
                LabelIncident::SidecarRefused,
                "sidecar_refused",
                "sidecar",
                IncidentSeverity::Degrades,
            ),
            (
                LabelIncident::SidecarWriteFailed,
                "sidecar_write_failed",
                "sidecar",
                IncidentSeverity::Degrades,
            ),
        ];
        let mut bodies = BTreeSet::new();
        for (index, (incident, code, field, severity)) in seven.iter().enumerate() {
            let code_value = IncidentCode::Label(*incident);
            assert_eq!(incident.code(), *code);
            assert_eq!(incident.field(), *field);
            assert_eq!(code_value.table_index(), 67 + index);
            assert_eq!(code_value.field(), *field);
            let body = explain_body(code_value);
            assert!(!body.is_empty(), "{code} has no body");
            assert!(
                bodies.insert(body),
                "{code} shares its body with another new code"
            );
            let row = &POLICY[67 + index];
            assert_eq!(
                row.code,
                code_value,
                "tail row {} names its code",
                68 + index
            );
            assert_eq!(row.class, PolicyClass::Explain);
            assert_eq!(row.predicate, PolicyPredicate::Always);
            assert_eq!(row.severity, *severity);
        }
        assert_eq!(bodies.len(), 7);
        assert_eq!(POLICY.len(), 74);
        assert_eq!(INVESTIGATOR_ALLOWLIST.len(), 54);
        let rows: Vec<IncidentCode> = POLICY[13..67].iter().map(|entry| entry.code).collect();
        assert_eq!(INVESTIGATOR_ALLOWLIST.to_vec(), rows);
        for (incident, code, _, _) in &seven {
            assert!(
                !INVESTIGATOR_ALLOWLIST.contains(&IncidentCode::Label(*incident)),
                "{code} stays off the allowlist (IN2 §3.1 reason 2)"
            );
        }
    }

    /// `IN2B` §9 rule 1: `would_open` agrees with `observe` on all three
    /// outcomes — the router's peek before the focused-document read.
    #[test]
    fn in2b_would_open_agrees_with_observe() {
        let code = IncidentCode::Label(LabelIncident::Project);
        let subject = IncidentSubject::Project;
        let revision = TimelineRevision(1);
        let mut log = IncidentLog::with_start(Instant::now(), None);
        assert!(log.would_open(code, subject, "seen"));
        let observation = IncidentObservation::plain(code, subject, "seen", revision);
        let Observed::Opened(id) = log.observe(observation) else {
            panic!("the first observation opens");
        };
        // An open match dedups: no second open.
        assert!(!log.would_open(code, subject, "seen"));
        // A different `observed` opens beside it.
        assert!(log.would_open(code, subject, "seen differently"));
        // Resolving suppresses the pair: nothing opens for it again.
        assert!(log.resolve(id, IncidentOutcome::Explained));
        assert!(!log.would_open(code, subject, "seen"));
        assert!(!log.would_open(code, subject, "seen differently"));
    }

    /// IN2B §12 item 10, §9 rule 5 (d15/d19): observed names truncate on
    /// **serialised** length at `observe`, on a `char` boundary, and every
    /// builder defaults the new fields.
    #[test]
    fn in2b_observed_names_truncate_on_serialised_length_at_observe() {
        let revision = TimelineRevision(1);
        // Every `IncidentObservation` builder sets the new fields to their
        // defaults (the one literal pass, B-3).
        for observation in [
            IncidentObservation::plain(
                IncidentCode::Label(LabelIncident::Project),
                IncidentSubject::Project,
                "observed",
                revision,
            ),
            IncidentObservation::from_op_error(
                &OpError::MissingClip(ClipId(4)),
                IncidentSubject::Clip(ClipId(4)),
                revision,
            ),
            IncidentObservation::from_batch_error(
                &BatchError::Empty,
                IncidentSubject::Project,
                revision,
            ),
            IncidentObservation::revision_conflict(
                IncidentSubject::Project,
                TimelineRevision(1),
                TimelineRevision(2),
            ),
            IncidentObservation::from_media_error(
                &MediaError::Backend("backend".to_owned()),
                IncidentSubject::ExportJob,
                revision,
            ),
            CaptionPlanError::NoCues.incident_observation(IncidentSubject::Project, revision),
            DeliveryVariantError::EffectIdExhausted
                .incident_observation(IncidentSubject::Project, revision),
        ] {
            assert_eq!(observation.name, None);
            assert!(!observation.transient);
        }

        // Truncation happens at `observe`, through the production path: one
        // log, distinct `observed` strings so every row opens its own
        // incident. Expected values are exact, not bounds.
        let rows = [
            ("n".repeat(64), "n".repeat(64)),
            ("n".repeat(65), "n".repeat(64)),
            ("n".repeat(240), "n".repeat(64)),
            ("\u{0}".repeat(240), "\u{0}".repeat(10)),
            ("é".repeat(40), "é".repeat(32)),
            ("a".repeat(63) + "é", "a".repeat(63)),
            (String::new(), String::new()),
            ("   ".to_owned(), "   ".to_owned()),
            ("a\"b".repeat(40), "a\"b".repeat(16)),
        ];
        let mut log = IncidentLog::with_start(Instant::now(), None);
        for (index, (name, expected)) in rows.iter().enumerate() {
            let mut observation = IncidentObservation::plain(
                IncidentCode::Label(LabelIncident::Project),
                IncidentSubject::Project,
                format!("row-{index}"),
                revision,
            );
            observation.name = Some(name.clone());
            let Observed::Opened(id) = log.observe(observation) else {
                panic!("row {index} must open");
            };
            let stored = log.get(id).unwrap().subject_name.clone().unwrap();
            assert_eq!(stored, *expected, "row {index}");
            assert!(
                json_escaped_len(&stored) <= SUBJECT_NAME_CEILING_BYTES,
                "row {index} stores {} serialised bytes",
                json_escaped_len(&stored)
            );
        }

        // The saturated ASCII row's wire arm is the contract's 82 B figure.
        let saturated = log
            .get(IncidentId(3))
            .unwrap()
            .subject_name
            .clone()
            .unwrap();
        assert_eq!(saturated, "n".repeat(64));
        let wire = serde_json::to_string(log.get(IncidentId(3)).unwrap()).unwrap();
        let arm = format!(r#","subject_name":"{}""#, "n".repeat(64));
        assert_eq!(arm.len(), 82);
        assert!(wire.contains(&arm), "{wire}");

        // An unnamed observation stores `None`, and the key skips the wire.
        let Observed::Opened(unnamed) = log.observe(IncidentObservation::plain(
            IncidentCode::Label(LabelIncident::Project),
            IncidentSubject::Project,
            "unnamed",
            revision,
        )) else {
            panic!("the unnamed row must open");
        };
        assert_eq!(log.get(unnamed).unwrap().subject_name, None);
        let wire = serde_json::to_string(log.get(unnamed).unwrap()).unwrap();
        assert!(!wire.contains("subject_name"), "{wire}");
    }

    /// `IN1b` §3.4 rule 24: evidence follows the code for every delegating
    /// producer core owns. `BranchError::InvalidBase` is the agent crate's half
    /// of the same rule.
    #[test]
    // One case per delegating producer core owns, written out rather than
    // folded, so the pairing each one asserts is readable beside it.
    #[allow(clippy::too_many_lines)]
    fn in1b_evidence_follows_the_code_for_every_delegating_producer() {
        let empty = IncidentObservation::from_batch_error(
            &BatchError::Empty,
            IncidentSubject::Project,
            TimelineRevision(2),
        );
        assert_eq!(
            empty.code,
            IncidentCode::Rejection(RejectionIncident::EditPlan)
        );
        assert!(matches!(
            empty.evidence,
            IncidentEvidence::OpError {
                family: IncidentFamily::Malformed,
                op_number: None,
                ..
            }
        ));

        let inner = OpError::MissingClip(ClipId(4));
        let failed = IncidentObservation::from_batch_error(
            &BatchError::OperationFailed {
                op_number: 3,
                error: inner.clone(),
            },
            IncidentSubject::Clip(ClipId(4)),
            TimelineRevision(2),
        );
        assert_eq!(
            failed.code,
            IncidentCode::Operation(IncidentFamily::Missing)
        );
        assert!(matches!(
            failed.evidence,
            IncidentEvidence::OpError {
                family: IncidentFamily::Missing,
                op_number: Some(3),
                ..
            }
        ));

        // A per-variant code override still carries its own family on the
        // evidence, so a consumer can branch without re-parsing.
        let lut = OpError::InvalidLutAssetHash {
            lut_asset: crate::LutAssetId(1),
            observed: "sha256:0".to_owned(),
            allowed: "the recorded hash",
        };
        let observation =
            IncidentObservation::from_op_error(&lut, IncidentSubject::Project, TimelineRevision(2));
        assert_eq!(observation.code, IncidentCode::LutAssetPolicy);
        assert!(matches!(
            observation.evidence,
            IncidentEvidence::OpError {
                family: IncidentFamily::Malformed,
                op_number: None,
                ..
            }
        ));

        // The two caption id-exhaustion variants take `operation_internal`
        // (`IN1b` §3.10 rule 39) and the other four take the rejection code.
        for (error, expected) in [
            (
                CaptionPlanError::TrackIdExhausted,
                IncidentCode::Operation(IncidentFamily::Internal),
            ),
            (
                CaptionPlanError::ClipIdExhausted,
                IncidentCode::Operation(IncidentFamily::Internal),
            ),
            (
                CaptionPlanError::NoCues,
                IncidentCode::Rejection(RejectionIncident::CaptionPlan),
            ),
            (
                CaptionPlanError::EmptyAuthoredScript,
                IncidentCode::Rejection(RejectionIncident::CaptionPlan),
            ),
            (
                CaptionPlanError::AuthoredScriptAlignment,
                IncidentCode::Rejection(RejectionIncident::CaptionPlan),
            ),
            (
                CaptionPlanError::InvalidCueDuration,
                IncidentCode::Rejection(RejectionIncident::CaptionPlan),
            ),
        ] {
            assert_eq!(error.incident_code(), expected, "{error}");
            let observation =
                error.incident_observation(IncidentSubject::Project, TimelineRevision(1));
            assert_eq!(observation.code, expected);
            assert!(matches!(
                observation.evidence,
                IncidentEvidence::CaptionPlan { .. }
            ));
        }

        // `DeliveryVariantError::InvalidDocument` delegates its code and its
        // family to the inner rejection and supplies `OpError` evidence, never
        // a `reason` string; its two siblings supply the rejection code and
        // `DeliveryVariant` evidence.
        let delegating = DeliveryVariantError::InvalidDocument(OpError::DuplicateBin(BinId(2)));
        assert_eq!(
            delegating.incident_code(),
            IncidentCode::Operation(IncidentFamily::Duplicate)
        );
        let observation =
            delegating.incident_observation(IncidentSubject::ExportJob, TimelineRevision(5));
        assert_eq!(observation.subject, IncidentSubject::ExportJob);
        assert!(matches!(
            observation.evidence,
            IncidentEvidence::OpError {
                family: IncidentFamily::Duplicate,
                op_number: None,
                ..
            }
        ));
        for error in [
            DeliveryVariantError::InvalidFocus { x: 200, y: 0 },
            DeliveryVariantError::EffectIdExhausted,
        ] {
            assert_eq!(
                error.incident_code(),
                IncidentCode::Rejection(RejectionIncident::DeliveryVariant),
                "{error}"
            );
            assert!(matches!(
                error
                    .incident_observation(IncidentSubject::ExportJob, TimelineRevision(5))
                    .evidence,
                IncidentEvidence::DeliveryVariant { .. }
            ));
        }

        // A revision conflict is stated against the revision the document is
        // actually on (`IN1b` §4).
        let conflict = IncidentObservation::revision_conflict(
            IncidentSubject::Agent,
            TimelineRevision(7),
            TimelineRevision(9),
        );
        assert_eq!(conflict.code, IncidentCode::EditRevisionConflict);
        assert_eq!(conflict.revision, TimelineRevision(9));
        assert_eq!(
            conflict.evidence,
            IncidentEvidence::Revision {
                expected: TimelineRevision(7),
                actual: TimelineRevision(9),
            }
        );
    }

    /// `IN1b` §9 clause 5: all seven variants label and serialise in their
    /// declared shapes, `Chain` in both of its forms.
    #[test]
    fn in1b_every_subject_variant_labels_and_serialises_in_its_declared_shape() {
        for (subject, label, wire) in [
            (
                IncidentSubject::Asset(AssetId(1)),
                "Asset 1",
                r#"{"asset":1}"#,
            ),
            (
                IncidentSubject::LutAsset(LutAssetId(5)),
                "Look 5",
                r#"{"lut_asset":5}"#,
            ),
            (IncidentSubject::Clip(ClipId(4)), "Clip 4", r#"{"clip":4}"#),
            (
                IncidentSubject::Track(TrackId(2)),
                "Track 2",
                r#"{"track":2}"#,
            ),
            (
                IncidentSubject::Chain(AudioChain::Bus(crate::AudioBusId(3))),
                "Bus 3",
                r#"{"chain":{"bus":3}}"#,
            ),
            (
                IncidentSubject::Chain(AudioChain::Master),
                "Master",
                r#"{"chain":"master"}"#,
            ),
            (IncidentSubject::ExportJob, "Export", r#""export_job""#),
            (IncidentSubject::Project, "Project", r#""project""#),
            (IncidentSubject::Agent, "Agent", r#""agent""#),
        ] {
            assert_eq!(subject.label(), label);
            assert_eq!(serde_json::to_string(&subject).unwrap(), wire);
        }
        // The dedup axis is ordered, which is what `suppressed` needs.
        let mut ordered = BTreeSet::new();
        for subject in every_subject_shape() {
            assert!(ordered.insert(subject));
        }
        assert_eq!(ordered.len(), 8);
    }

    /// `IN1b` §9 clause 15 and regression R-C: Part A's literal is byte
    /// identical after Part B, re-run here under its `IN1b` name. The
    /// unmodified Part A test
    /// `in1_the_fixture_incident_serialises_to_the_pinned_wire_body` asserts
    /// the same body; this one also pins its length at 819 B.
    #[test]
    fn in1b_the_part_a_wire_body_is_byte_identical() {
        let mut log = IncidentLog::with_start(Instant::now(), None);
        let probed = untagged_webm_probe();
        let Observed::Opened(id) = log.observe(unknown_primaries_observation(&probed)) else {
            panic!("the first observation must open an incident");
        };
        assert_eq!(
            log.observe(unknown_primaries_observation(&probed)),
            Observed::Deduped(id)
        );
        let body = serde_json::to_string(log.get(id).unwrap()).unwrap();
        assert_eq!(body, IN1_PINNED_WIRE_BODY);
        assert_eq!(body.len(), 819, "`IN1_INCIDENT_SERIALIZED_BYTES`");
    }

    /// `IN1b` §7 item 13: the second and third wire literals of §3.11 rule 42,
    /// a project-subject and a chain-subject incident, pinned byte for byte.
    #[test]
    fn in1b_a_project_and_a_chain_subject_incident_serialise_to_their_pinned_bodies() {
        let mut log = IncidentLog::with_start(Instant::now(), None);
        let Observed::Opened(project) = log.observe(IncidentObservation::plain(
            IncidentCode::Label(LabelIncident::Project),
            IncidentSubject::Project,
            "could not read the project file",
            TimelineRevision(1),
        )) else {
            panic!("a fresh log must open the project incident");
        };
        assert_eq!(
            serde_json::to_string(log.get(project).unwrap()).unwrap(),
            IN1B_PINNED_PROJECT_WIRE_BODY
        );

        let Observed::Opened(chain) = log.observe(IncidentObservation::plain(
            IncidentCode::Label(LabelIncident::Mixer),
            IncidentSubject::Chain(AudioChain::Bus(crate::AudioBusId(3))),
            "no silence was selected on the bus",
            TimelineRevision(1),
        )) else {
            panic!("a fresh log must open the chain incident");
        };
        assert_eq!(
            serde_json::to_string(log.get(chain).unwrap()).unwrap(),
            IN1B_PINNED_CHAIN_WIRE_BODY
        );

        // The eighth subject (erratum `IN1b`-A-R13): one LUT in the project's
        // look store, on its own id space.
        let Observed::Opened(look) = log.observe(IncidentObservation::plain(
            IncidentCode::Label(LabelIncident::Look),
            IncidentSubject::LutAsset(LutAssetId(5)),
            "the look could not be restored from the store",
            TimelineRevision(1),
        )) else {
            panic!("a fresh log must open the look incident");
        };
        assert_eq!(
            serde_json::to_string(log.get(look).unwrap()).unwrap(),
            IN1B_PINNED_LUT_ASSET_WIRE_BODY
        );
    }

    /// The worst serialised incident over IN2 §4.2 rule 11's product, measured
    /// against the implementation rather than a prototype: the **74** declared
    /// codes times the **eight** subject variants of `IN1b` §3.3 rule 17 as
    /// amended by erratum `IN1b`-A-R13, times **two probes**, times
    /// {no proposal, a proposal at the cap}, times {open, investigating,
    /// resolved with all telemetry, resolved without the two new fields},
    /// times `IN2B` §10 rule 5's {no name, a name at the cap} — **18 944**
    /// shapes.
    ///
    /// The probe is an axis because the recoveries array is part of the wire
    /// body and IN2 §7 rule 1 put a **second** action on three of the seventy-
    /// four rows. A Rec.709-incompatible probe demotes every predicated row to
    /// `Explain` and builds no operation at all, so a loop with that probe
    /// alone cannot see the one thing this slice changed about the record
    /// (IN2 erratum A-R9). The loop therefore measures both, and asserts a
    /// positive count of shapes that carried a `RecoveryKind::Operation`.
    ///
    /// The other inputs are probe-2b T2's: the longest rendered `OpError` and
    /// `MediaError` templates on `HEAD` with saturated ids, a seventy-byte
    /// `allowed`, saturated revision and count, and the row's own written body.
    /// The figure is a measurement over chosen realistic inputs and not a proof
    /// — `observed` is not bounded by the type system (`IN1b` §3.11 rule 44) —
    /// which is why the assertion is a ceiling and a floor rather than an
    /// equality.
    ///
    /// `crates/kinewright-agent/src/server.rs`'s
    /// `IN1_INCIDENT_SERIALIZED_CEILING_BYTES` is the constant this number
    /// sets, and implementer **D moves it from 2 048 to 4 096** with a twin of
    /// the loop below, whose probe axis it needs too (IN2 §4.2 rule 11).
    #[test]
    // The twin measures six nested axes over 18 944 shapes plus the pinned
    // fixture; splitting it would put the measurement and its assertions in
    // different functions, which is exactly what makes a pin easy to weaken
    // by accident.
    #[allow(clippy::too_many_lines)]
    fn in2b_the_ceiling_holds_with_names_saturated_and_819_unmoved_on_every_subject_shape() {
        // The core twin of `in1b_every_code_fits_the_measured_ceiling`, grown
        // over IN2 §4.2 rule 11's product: 74 codes x 8 subject shapes x
        // 2 probes x {no proposal, a proposal at the cap} x {open,
        // investigating, resolved with all telemetry, resolved without the two
        // new fields} x IN2B §10 rule 5's {no name, a name at the cap}.
        //
        // **The pin is the loop, not a remembered number** (IN2 probe-2
        // disagreement 4): nothing below asserts an intermediate byte figure.
        // `IN1_INCIDENT_SERIALIZED_CEILING_BYTES` itself lives on the agent
        // crate's `KinewrightMcp` (IN2 §14 row D); this literal is its twin and
        // moves with it, 2 048 -> 4 096.
        const CEILING: usize = 4_096;

        // N5/G4: the twin earns the `_and_819_unmoved` in its name — the same
        // named fixture the agent loop pins, built from core's own twin
        // helpers, so the two constructions cross-check each other's 819. The
        // literal twins `IN1_INCIDENT_SERIALIZED_BYTES`, as `CEILING` does.
        let probed = untagged_webm_probe();
        let mut log = IncidentLog::with_start(Instant::now(), None);
        let Observed::Opened(id) = log.observe(unknown_primaries_observation(&probed)) else {
            panic!("the first observation must open an incident");
        };
        assert_eq!(
            log.observe(unknown_primaries_observation(&probed)),
            Observed::Deduped(id)
        );
        let incident = log.get(id).unwrap();
        assert_eq!(incident.count, 2);
        assert_eq!(incident.state, IncidentState::Open);
        assert_eq!(
            serde_json::to_vec(incident).unwrap().len(),
            819,
            "the fixture incident moved off its pinned budget"
        );

        let mut worst = 0_usize;
        let mut worst_shape = String::new();
        let mut with_operation_worst = 0_usize;
        let mut with_operation_shape = String::new();
        let mut with_operation = 0_usize;
        let mut measured = 0_usize;
        // N5/G1: the name arm's own pin — a loop whose saturated arm never
        // lands (deleted, `None`, or short) must fail here, not pass on the
        // unnamed shapes alone.
        let mut named_count = 0_usize;
        let mut named_worst = 0_usize;
        let mut unnamed_worst = 0_usize;
        for code in EVERY_INCIDENT_CODE {
            for subject in every_subject_shape() {
                for probed in [worst_probe(), rec709_compatible_worst_probe()] {
                    let mut log = IncidentLog::with_start(Instant::now(), None);
                    let Observed::Opened(id) =
                        log.observe(worst_observation(code, subject, &probed))
                    else {
                        panic!("a fresh log must open {}", code.code());
                    };
                    let opened = log.get(id).unwrap().clone();
                    let carries_operation = opened
                        .recoveries
                        .iter()
                        .any(|action| matches!(action.kind, RecoveryKind::Operation(_)));
                    for proposal in [None, Some(widest_proposal())] {
                        for (shape, state, telemetry) in [
                            ("open", IncidentState::Open, IncidentTelemetry::default()),
                            (
                                "investigating",
                                IncidentState::Investigating,
                                IncidentTelemetry::default(),
                            ),
                            (
                                "resolved+turns+resolver",
                                IncidentState::Resolved(IncidentOutcome::Explained),
                                widest_telemetry(true),
                            ),
                            (
                                "resolved",
                                IncidentState::Resolved(IncidentOutcome::Explained),
                                widest_telemetry(false),
                            ),
                        ] {
                            // IN2B §10 rule 5's name arm: every shape is
                            // measured with the name absent and saturated at
                            // the d15 ceiling, so the loop itself carries the
                            // +82 B and no remembered number does.
                            for name in [None, Some("n".repeat(SUBJECT_NAME_CEILING_BYTES))] {
                                let mut widest = opened.clone();
                                widest.id = IncidentId(u64::MAX);
                                widest.count = u32::MAX;
                                widest.state = state;
                                widest.telemetry = telemetry.clone();
                                widest.proposal = proposal.clone();
                                widest.subject_name = name.clone();
                                let bytes = serde_json::to_vec(&widest).unwrap().len();
                                measured += 1;
                                // N5/G1: counted by serialised length, not by
                                // arm — a short or missing saturation scores 0.
                                if name.as_ref().is_some_and(|candidate| {
                                    json_escaped_len(candidate) == SUBJECT_NAME_CEILING_BYTES
                                }) {
                                    named_count += 1;
                                    named_worst = named_worst.max(bytes);
                                } else {
                                    unnamed_worst = unnamed_worst.max(bytes);
                                }
                                let label = format!(
                                    "{} / {subject:?} / {shape} / recoveries={} / proposal={}",
                                    code.code(),
                                    widest.recoveries.len(),
                                    proposal.is_some()
                                );
                                if bytes > worst {
                                    worst = bytes;
                                    worst_shape = label.clone();
                                }
                                if carries_operation {
                                    with_operation += 1;
                                    if bytes > with_operation_worst {
                                        with_operation_worst = bytes;
                                        with_operation_shape = label;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        println!("IN2_CEILING measured={measured} worst={worst} shape={worst_shape}");
        println!(
            "IN2_CEILING with_operation={with_operation} worst={with_operation_worst} shape={with_operation_shape}"
        );
        assert_eq!(
            measured,
            74 * 8 * 2 * 2 * 4 * 2,
            "IN2 §4.2 rule 11's shapes, with the probe axis erratum A-R9 adds and the IN2B §10 rule 5 name arm"
        );
        // N5/G1: exactly half the shapes carry a name saturated at the
        // ceiling, and the saturated arm is the worst — the arm cannot be
        // `None`, deleted, or short without failing here.
        assert_eq!(
            named_count,
            measured / 2,
            "the name arm must saturate exactly half the shapes"
        );
        assert!(
            named_worst > unnamed_worst,
            "the saturated name must reach the wire: named worst {named_worst} vs unnamed worst {unnamed_worst}"
        );
        // The one thing IN2 changes about the wire body's `recoveries` array is
        // §7 rule 1's second action, and a loop that never builds an operation
        // cannot see it (erratum A-R9).
        assert!(
            with_operation > 0,
            "the population must include shapes whose recoveries carry an Operation"
        );
        assert!(
            with_operation_worst <= CEILING,
            "the worst shape carrying an Operation ({with_operation_worst}) exceeds the ceiling ({with_operation_shape})"
        );
        assert!(
            worst <= CEILING,
            "the measured worst {worst} exceeds the declared ceiling ({worst_shape})"
        );
        assert!(
            worst > CEILING / 2,
            "a ceiling more than twice the measured worst {worst} is not a measurement"
        );
    }

    /// A proposal at `INVESTIGATOR_MAX_PROPOSAL_OPERATIONS` with both strings
    /// at the 240 B serialised cap and a saturated `base_revision`. The
    /// operations are `#[serde(skip)]` and therefore cost nothing on the wire,
    /// which is the whole point of IN2 §4.2 rule 8.
    fn widest_proposal() -> IncidentProposal {
        let filler = "a".repeat(INVESTIGATOR_EXPLANATION_CEILING_BYTES);
        assert_eq!(
            json_escaped_len(&filler),
            INVESTIGATOR_EXPLANATION_CEILING_BYTES
        );
        IncidentProposal {
            operations: vec![
                Operation::DeleteClip {
                    clip: ClipId(u64::MAX)
                };
                INVESTIGATOR_MAX_PROPOSAL_OPERATIONS
            ],
            operation_count: INVESTIGATOR_MAX_PROPOSAL_OPERATIONS,
            summary: filler.clone(),
            explanation: filler,
            base_revision: TimelineRevision(u64::MAX),
            stale: true,
        }
    }

    /// Every telemetry field filled at its widest. `turns` and `resolver` are
    /// the two IN2 adds; the `false` arm is the pre-IN2 shape, so the loop
    /// measures both columns. The resolver's three strings are the widest
    /// **real** ones: a harness id, a model id and IN2 §3.7 rule 38's longest
    /// stop sentence.
    fn widest_telemetry(with_session: bool) -> IncidentTelemetry {
        IncidentTelemetry {
            resolved_after: Some(Duration::MAX),
            tool_calls: u32::MAX,
            input_tokens: Some(u64::MAX),
            cached_input_tokens: Some(u64::MAX),
            cache_creation_input_tokens: Some(u64::MAX),
            output_tokens: Some(u64::MAX),
            reasoning_output_tokens: Some(u64::MAX),
            cost_usd_millionths: Some(i64::MIN),
            turns: with_session.then_some(u32::MAX),
            resolver: with_session.then(|| IncidentResolver::Session {
                harness: "claude-code".to_owned(),
                model: Some("claude-opus-4-1-20250805".to_owned()),
                stop: "the session asked for a confirmation".to_owned(),
            }),
        }
    }

    /// probe-2b T2's worst-case inputs, rebuilt against the real types.
    fn worst_observation(
        code: IncidentCode,
        subject: IncidentSubject,
        probed: &ColorDescription,
    ) -> IncidentObservation {
        IncidentObservation {
            code,
            subject,
            observed: worst_observed(code),
            allowed: Some("a".repeat(70)),
            evidence: worst_evidence(code, probed),
            revision: TimelineRevision(u64::MAX),
            name: None,
            transient: false,
        }
    }

    /// The longest rendered `OpError` message on `HEAD`:
    /// `ColorStageOrderViolation`'s template with saturated ids.
    fn worst_op_error() -> OpError {
        OpError::ColorStageOrderViolation {
            clip: ClipId(u64::MAX),
            effect: EffectId(u64::MAX),
            kind: "creative_look".to_owned(),
            color_stage_rank: u8::MAX,
            previous_effect: EffectId(u64::MAX),
            previous_kind: "creative_look".to_owned(),
            previous_color_stage_rank: u8::MAX,
        }
    }

    /// The longest rendered `MediaError` on `HEAD`.
    fn worst_media_error() -> MediaError {
        MediaError::UnsupportedDecoderFormat {
            path: std::path::PathBuf::from(
                "/home/editor/Projects/Feature 2026/Media/Day 12/A012C003_260915_R1AB.mov",
            ),
            format: "yuv422p10le".to_owned(),
            declared_bit_depth: Some(10),
            decoder_bit_depth: Some(10),
            reason: "the decoder's native format is not a supported integer source surface"
                .to_owned(),
        }
    }

    fn worst_probe() -> ColorDescription {
        ColorDescription {
            primaries: ColorPrimaries::Unknown,
            transfer: ColorTransfer::Unknown,
            matrix: ColorMatrix::Unknown,
            range: ColorRange::Other("full_swing_studio_limited".to_owned()),
            white_point: ColorWhitePoint::Unknown,
            bit_depth: ColorBitDepth::Eight,
            confidence_basis_points: 4_000,
            provenance: ColorProvenance::Other("stream_metadata_container".to_owned()),
        }
    }

    /// The widest **Rec.709-compatible** probe, so the loop measures the rows
    /// whose recoveries carry a `RecoveryKind::Operation` — the three
    /// `AutoApply` rows and IN2 §7 rows 1-3, which are the only rows on which
    /// this slice changed the `recoveries` array (erratum A-R9).
    ///
    /// `worst_probe`'s `ColorRange::Other(_)` is refused by `rec709_compatible`
    /// (`incident.rs:2370-2391`), which admits `range` only in
    /// `Unknown | Limited | Full`, so that probe builds no operation at all.
    fn rec709_compatible_worst_probe() -> ColorDescription {
        ColorDescription {
            primaries: ColorPrimaries::Unknown,
            transfer: ColorTransfer::Unknown,
            matrix: ColorMatrix::Unknown,
            range: ColorRange::Limited,
            white_point: ColorWhitePoint::Unknown,
            bit_depth: ColorBitDepth::Sixteen,
            confidence_basis_points: u16::MAX,
            provenance: ColorProvenance::Other("stream_metadata_container".to_owned()),
        }
    }

    fn worst_relink_reason() -> String {
        format!(
            "Cannot relink asset {}: source fingerprint mismatch (expected sha256:{}…, candidate sha256:{}…)",
            u64::MAX,
            "0".repeat(16),
            "f".repeat(16)
        )
    }

    const WORST_SOURCE_EDIT_REASON: &str =
        "Source verification did not confirm the original online source; no edit was applied";

    fn worst_evidence(code: IncidentCode, probed: &ColorDescription) -> IncidentEvidence {
        match code {
            IncidentCode::SourceColor(_) => IncidentEvidence::SourceColor {
                probed: probed.clone(),
                assumption: Some(ColorSourceProfileAssumption::D65),
            },
            IncidentCode::Media(_)
            | IncidentCode::DeliveryColor(_)
            | IncidentCode::DeliveryVerification(_)
            | IncidentCode::ColorQc(_) => IncidentEvidence::MediaError {
                code: "delivery_verification_frame_count_out_of_range".to_owned(),
                message: worst_media_error().to_string(),
            },
            IncidentCode::Operation(family) => IncidentEvidence::OpError {
                family,
                op_number: Some(usize::MAX),
                message: worst_op_error().to_string(),
            },
            IncidentCode::LutAssetPolicy | IncidentCode::Rejection(RejectionIncident::EditPlan) => {
                IncidentEvidence::OpError {
                    family: IncidentFamily::Malformed,
                    op_number: Some(usize::MAX),
                    message: worst_op_error().to_string(),
                }
            }
            IncidentCode::EditRevisionConflict => IncidentEvidence::Revision {
                expected: TimelineRevision(u64::MAX),
                actual: TimelineRevision(u64::MAX),
            },
            IncidentCode::Rejection(RejectionIncident::DeliveryVariant) => {
                IncidentEvidence::DeliveryVariant {
                    reason: format!(
                        "edit plan operation {} failed: {}",
                        usize::MAX,
                        worst_op_error()
                    ),
                }
            }
            IncidentCode::Rejection(RejectionIncident::AgentBranch) => IncidentEvidence::Branch {
                reason: format!(
                    "cherry-pick operation index {} is outside the one-based range 1..={}",
                    u64::MAX,
                    u64::MAX
                ),
            },
            IncidentCode::Rejection(RejectionIncident::SourceEdit) => {
                IncidentEvidence::SourceEdit {
                    reason: WORST_SOURCE_EDIT_REASON.to_owned(),
                }
            }
            IncidentCode::Rejection(RejectionIncident::Relink) => IncidentEvidence::Relink {
                reason: worst_relink_reason(),
            },
            IncidentCode::Rejection(RejectionIncident::ProjectSave) => {
                IncidentEvidence::ProjectSave {
                    reason:
                        "could not write the project file: No space left on device (os error 28)"
                            .to_owned(),
                }
            }
            IncidentCode::Rejection(RejectionIncident::CaptionPlan) => {
                IncidentEvidence::CaptionPlan {
                    reason: CaptionPlanError::AuthoredScriptAlignment.to_string(),
                }
            }
            IncidentCode::Label(_) => IncidentEvidence::Plain,
        }
    }

    fn worst_observed(code: IncidentCode) -> String {
        match code {
            // Every colour row's `observed` comes from
            // `ColorSourceError::observed()`; the widest is the formatted tuple.
            IncidentCode::SourceColor(_) => ColorSourceError::UnsupportedCombination {
                primaries: ColorPrimaries::Other("display_p3_container".to_owned()),
                transfer: ColorTransfer::Other("arri_logc4_wide".to_owned()),
                matrix: ColorMatrix::Other("ictcp_constant_intensity".to_owned()),
                range: ColorRange::Other("full_swing_studio_limited".to_owned()),
            }
            .observed(),
            IncidentCode::Media(_)
            | IncidentCode::DeliveryColor(_)
            | IncidentCode::DeliveryVerification(_)
            | IncidentCode::ColorQc(_) => worst_media_error().to_string(),
            IncidentCode::EditRevisionConflict => format!(
                "expected timeline revision {}, current revision is {}",
                u64::MAX,
                u64::MAX
            ),
            IncidentCode::Rejection(RejectionIncident::SourceEdit) => {
                WORST_SOURCE_EDIT_REASON.to_owned()
            }
            IncidentCode::Rejection(RejectionIncident::Relink) => worst_relink_reason(),
            // A placeholder site carries whatever the application formatted, so
            // it is measured against the same bound as a typed one.
            _ => worst_op_error().to_string(),
        }
    }

    /// `IN1b` §3.11 rule 42's project-subject body, normative as generated.
    const IN1B_PINNED_PROJECT_WIRE_BODY: &str = concat!(
        r#"{"id":1,"code":"project_unclassified","class":"explain","severity":"blocks","subject":"p"#,
        r#"roject","field":"project","observed":"could not read the project file","allowed":null,"e"#,
        r#"vidence":"plain","recoveries":[{"label":"How to fix this","kind":{"explain":"The project"#,
        r#" could not be opened, created, read, or restored from the crash-recovery journal. The me"#,
        r#"ssage names the file; check that it exists and can be read, then open it again — if it w"#,
        r#"as unsaved work that could not be restored, the last saved version of the project is sti"#,
        r#"ll intact."}}],"revision":1,"count":1,"state":"open","telemetry":{"tool_calls":0}}"#,
    );

    /// `IN1b` §3.11 rule 42's look-subject body, normative as generated (erratum `IN1b`-A-R13).
    const IN1B_PINNED_LUT_ASSET_WIRE_BODY: &str = concat!(
        r#"{"id":3,"code":"look_unclassified","class":"explain","severity":"blocks","subject":{"lut"#,
        r#"_asset":5},"field":"look","observed":"the look could not be restored from the store","al"#,
        r#"lowed":null,"evidence":"plain","recoveries":[{"label":"How to fix this","kind":{"explain"#,
        r#"":"The look or LUT could not be imported, restored or applied. The message names the fil"#,
        r#"e or the store; check that the project has been saved and that its LUT folder is a writa"#,
        r#"ble directory, then try again."}}],"revision":1,"count":1,"state":"open","telemetry":{"t"#,
        r#"ool_calls":0}}"#,
    );

    /// `IN1b` §3.11 rule 42's chain-subject body, normative as generated.
    const IN1B_PINNED_CHAIN_WIRE_BODY: &str = concat!(
        r#"{"id":2,"code":"mixer_unclassified","class":"explain","severity":"blocks","subject":{"ch"#,
        r#"ain":{"bus":3}},"field":"mixer","observed":"no silence was selected on the bus","allowed"#,
        r#"":null,"evidence":"plain","recoveries":[{"label":"How to fix this","kind":{"explain":"Th"#,
        r#"e mixer could not do what was asked — usually because the audio it needs to learn from, "#,
        r#"or the node it was learned for, is not on the bus any more. Re-select the range or the n"#,
        r#"ode and try again."}}],"revision":1,"count":1,"state":"open","telemetry":{"tool_calls":0"#,
        r#"}}"#,
    );

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

    // ---------------------------------------------------------------------
    // IN2 §9.1, core items 1-12.
    // ---------------------------------------------------------------------

    /// IN2 §7 rule 4's sixteen, with its `buildable?` column. The tests below
    /// read this column rather than a literal, so the count of buildable rows
    /// is whatever the table says and is never written twice (N2.5/B8).
    const DETERMINISTIC_SIXTEEN: [(IncidentCode, bool); 16] = [
        (
            IncidentCode::SourceColor(SourceColorIncident::UnknownRange),
            true,
        ),
        (
            IncidentCode::SourceColor(SourceColorIncident::UnknownBitDepth),
            true,
        ),
        (
            IncidentCode::SourceColor(SourceColorIncident::UnknownWhitePoint),
            true,
        ),
        (
            IncidentCode::SourceColor(SourceColorIncident::UnsupportedPrimaries),
            false,
        ),
        (
            IncidentCode::SourceColor(SourceColorIncident::UnsupportedTransfer),
            false,
        ),
        (
            IncidentCode::SourceColor(SourceColorIncident::UnsupportedMatrix),
            false,
        ),
        (
            IncidentCode::SourceColor(SourceColorIncident::UnsupportedRange),
            false,
        ),
        (
            IncidentCode::SourceColor(SourceColorIncident::UnsupportedWhitePoint),
            false,
        ),
        (
            IncidentCode::SourceColor(SourceColorIncident::UnsupportedBitDepth),
            false,
        ),
        (
            IncidentCode::SourceColor(SourceColorIncident::UnsupportedCombination),
            false,
        ),
        (
            IncidentCode::DeliveryColor(DeliveryColorIncident::UnsupportedCodec),
            false,
        ),
        (
            IncidentCode::DeliveryColor(DeliveryColorIncident::UnsupportedField),
            false,
        ),
        (
            IncidentCode::DeliveryColor(DeliveryColorIncident::PixelFormatDepthMismatch),
            false,
        ),
        (
            IncidentCode::DeliveryColor(DeliveryColorIncident::EncoderPixelFormatUnavailable),
            false,
        ),
        (
            IncidentCode::ColorQc(ColorQcIncident::NodeBudgetExceeded),
            false,
        ),
        (
            IncidentCode::DeliveryVerification(DeliveryVerificationIncident::FrameCountOutOfRange),
            false,
        ),
    ];

    /// An all-`Unknown` probe: what an `unknown_source_*` row carries, and the
    /// shape `rec709_compatible` admits.
    fn all_unknown_probe() -> ColorDescription {
        ColorDescription {
            primaries: ColorPrimaries::Unknown,
            transfer: ColorTransfer::Unknown,
            matrix: ColorMatrix::Unknown,
            range: ColorRange::Unknown,
            white_point: ColorWhitePoint::Unknown,
            bit_depth: ColorBitDepth::Unknown,
            confidence_basis_points: 0,
            provenance: ColorProvenance::Inferred,
        }
    }

    fn source_color_evidence(probed: &ColorDescription) -> IncidentEvidence {
        IncidentEvidence::SourceColor {
            probed: probed.clone(),
            assumption: None,
        }
    }

    /// Every evidence shape IN2 §7 can build an operation from, plus the two
    /// that carry nothing typed at all.
    fn buildable_evidence_shapes() -> Vec<IncidentEvidence> {
        vec![
            source_color_evidence(&all_unknown_probe()),
            source_color_evidence(&untagged_mp4_probe()),
            source_color_evidence(&untagged_webm_probe()),
            source_color_evidence(&{
                let mut probed = all_unknown_probe();
                probed.primaries = ColorPrimaries::Bt2020;
                probed
            }),
            IncidentEvidence::Plain,
        ]
    }

    /// IN2 §9.1 item 1, §9.2 clause 1.
    #[test]
    fn in2_the_allowlist_is_fifty_four_explain_codes_that_are_policy_rows_fourteen_to_sixty_seven()
    {
        assert_eq!(INVESTIGATOR_ALLOWLIST.len(), 54);

        let mut seen = BTreeSet::new();
        for code in INVESTIGATOR_ALLOWLIST {
            assert!(seen.insert(code), "{} is listed twice", code.code());
        }
        assert_eq!(seen.len(), 54);

        for code in INVESTIGATOR_ALLOWLIST {
            let entry = policy_entry(code);
            assert_eq!(
                entry.class,
                PolicyClass::Explain,
                "{} is not an Explain row",
                code.code()
            );
            assert_eq!(
                entry.predicate,
                PolicyPredicate::Always,
                "{} is predicated",
                code.code()
            );
        }

        let auto_apply: Vec<IncidentCode> = POLICY
            .iter()
            .filter(|entry| entry.class == PolicyClass::AutoApply)
            .map(|entry| entry.code)
            .collect();
        assert_eq!(auto_apply.len(), 3);
        for code in &auto_apply {
            assert!(
                !INVESTIGATOR_ALLOWLIST.contains(code),
                "the router owns {}",
                code.code()
            );
        }

        // Rows 14-67, contiguous.
        let rows: Vec<IncidentCode> = POLICY[13..67].iter().map(|entry| entry.code).collect();
        assert_eq!(INVESTIGATOR_ALLOWLIST.to_vec(), rows);

        // No member yields an `Operation` recovery under any evidence IN2 §7
        // can build one from.
        let mut checked = 0_usize;
        for code in INVESTIGATOR_ALLOWLIST {
            for subject in every_subject_shape() {
                for evidence in buildable_evidence_shapes() {
                    assert!(
                        deterministic_recovery(code, subject, &evidence).is_none(),
                        "{} must have no button",
                        code.code()
                    );
                    assert!(
                        !policy_recovery(code, subject, &evidence)
                            .iter()
                            .any(|action| matches!(action.kind, RecoveryKind::Operation(_))),
                        "{} must have no button",
                        code.code()
                    );
                    checked += 1;
                }
            }
        }
        assert_eq!(checked, 54 * 8 * 5);

        // The converse, read from IN2 §7 rule 4's own column: every row the
        // table marks `yes` yields one, and they are exactly the `Explain`
        // codes that do.
        let expected_buildable: BTreeSet<IncidentCode> = DETERMINISTIC_SIXTEEN
            .iter()
            .filter(|(_, buildable)| *buildable)
            .map(|(code, _)| *code)
            .collect();
        assert!(!expected_buildable.is_empty());

        let mut measured_buildable = BTreeSet::new();
        for code in EVERY_INCIDENT_CODE {
            if policy_entry(code).class != PolicyClass::Explain {
                continue;
            }
            for evidence in buildable_evidence_shapes() {
                if policy_recovery(code, IncidentSubject::Asset(AssetId(1)), &evidence)
                    .iter()
                    .any(|action| matches!(action.kind, RecoveryKind::Operation(_)))
                {
                    measured_buildable.insert(code);
                }
            }
        }
        assert_eq!(measured_buildable, expected_buildable);
    }

    /// IN2 §9.1 item 2, §9.2 clause 6 — the four behaviours core owns. The
    /// remaining two, `incident_panel_rows` and the Media panel's filter, plus
    /// `card_actions`' revert guard, live in `kinewright-app` and are asserted
    /// by that crate's own tests (IN2 §3.6 rule 33 sites 1, 6, 7, 8).
    #[test]
    fn in2_investigating_counts_as_open_at_every_site() {
        assert!(IncidentState::Open.is_open());
        assert!(IncidentState::Investigating.is_open());
        for outcome in [
            IncidentOutcome::Applied,
            IncidentOutcome::Reverted,
            IncidentOutcome::Explained,
            IncidentOutcome::Rejected,
        ] {
            assert!(!IncidentState::Resolved(outcome).is_open());
        }

        let probed = untagged_mp4_probe();
        let mut log = IncidentLog::with_start(Instant::now(), None);
        let Observed::Opened(id) = log.observe(unknown_primaries_observation(&probed)) else {
            panic!("the first observation must open an incident");
        };
        assert!(log.begin_investigation(id));
        assert_eq!(log.get(id).unwrap().state, IncidentState::Investigating);

        // Site 3: still in the default listing. Site 4: the badge does not fall.
        assert_eq!(log.open().count(), 1);
        assert_eq!(log.open_count(), 1);

        // Site 2: a repeat observation dedups rather than opening a second
        // incident about the same problem.
        assert_eq!(
            log.observe(unknown_primaries_observation(&probed)),
            Observed::Deduped(id)
        );
        assert_eq!(log.len(), 1);
        assert_eq!(log.get(id).unwrap().count, 2);

        // Site 5: IN1 §5.2 rule 10's conflict refresh keeps working.
        assert!(log.refresh_revision(id, TimelineRevision(9)));
        assert_eq!(log.get(id).unwrap().revision, TimelineRevision(9));
        assert_eq!(log.get(id).unwrap().state, IncidentState::Investigating);
    }

    /// IN2 §9.1 item 3, §3.6 rule 35.
    #[test]
    fn in2_begin_investigation_writes_one_field_and_only_on_an_open_entry() {
        let probed = untagged_mp4_probe();
        let mut log = IncidentLog::with_start(Instant::now(), None);
        let Observed::Opened(id) = log.observe(unknown_primaries_observation(&probed)) else {
            panic!("the first observation must open an incident");
        };
        let before = log.get(id).unwrap().clone();

        assert!(log.begin_investigation(id));
        let after = log.get(id).unwrap().clone();
        assert_eq!(after.state, IncidentState::Investigating);
        // The second field erratum A-R7 adds only exists when a previous
        // session left a proposal behind; there is none here.
        assert!(after.proposal.is_none());
        assert_eq!(after.count, before.count);
        assert_eq!(after.revision, before.revision);
        assert_eq!(after.opened_at, before.opened_at);
        assert_eq!(after.telemetry, before.telemetry);
        assert!(after.proposal.is_none());
        assert_eq!(
            log.observe(unknown_primaries_observation(&probed)),
            Observed::Deduped(id),
            "`begin_investigation` must not touch `suppressed`"
        );

        // Already investigating.
        assert!(!log.begin_investigation(id));
        // Unknown id.
        assert!(!log.begin_investigation(IncidentId(9_999)));

        let mut refused = 0_usize;
        for outcome in [
            IncidentOutcome::Applied,
            IncidentOutcome::Reverted,
            IncidentOutcome::Explained,
            IncidentOutcome::Rejected,
        ] {
            let mut log = IncidentLog::with_start(Instant::now(), None);
            let Observed::Opened(id) = log.observe(unknown_primaries_observation(&probed)) else {
                panic!("the first observation must open an incident");
            };
            assert!(log.resolve(id, outcome));
            assert!(!log.begin_investigation(id));
            assert_eq!(log.get(id).unwrap().state, IncidentState::Resolved(outcome));
            refused += 1;
        }
        assert_eq!(refused, 4);
    }

    /// IN2 §9.1 item 4, §9.2 clause 8 — N2.5/B3's rule, and the one that fails
    /// hardest at `7e85972`, because neither the state nor the writer exists.
    #[test]
    fn in2_end_investigation_returns_the_incident_to_open_and_suppresses_nothing() {
        let probed = untagged_mp4_probe();
        let stops = [
            "budget: turns",
            "budget: wall time",
            "budget: tokens",
            "the investigator was switched off",
            "the session asked for a confirmation",
            "the agent event stream disconnected",
        ];
        let mut ended = 0_usize;
        for stop in stops {
            let mut log = IncidentLog::with_start(Instant::now(), None);
            let Observed::Opened(id) = log.observe(unknown_primaries_observation(&probed)) else {
                panic!("the first observation must open an incident");
            };
            assert!(log.begin_investigation(id));
            assert!(log.end_investigation(
                id,
                IncidentResolver::Session {
                    harness: "scripted".to_owned(),
                    model: None,
                    stop: stop.to_owned(),
                },
            ));

            let incident = log.get(id).unwrap();
            assert_eq!(incident.state, IncidentState::Open);
            assert!(matches!(
                incident.telemetry.resolver,
                Some(IncidentResolver::Session { stop: ref written, .. })
                    if written == stop
            ));
            assert_eq!(log.open_count(), 1);

            // Nothing was suppressed: the next observation of the same
            // `(code, subject)` dedups into the same incident.
            assert_eq!(
                log.observe(unknown_primaries_observation(&probed)),
                Observed::Deduped(id)
            );
            assert_eq!(log.len(), 1);
            ended += 1;
        }
        assert_eq!(ended, stops.len());

        // Only on an `Investigating` entry.
        let mut log = IncidentLog::with_start(Instant::now(), None);
        let Observed::Opened(id) = log.observe(unknown_primaries_observation(&probed)) else {
            panic!("the first observation must open an incident");
        };
        assert!(!log.end_investigation(
            id,
            IncidentResolver::Session {
                harness: "scripted".to_owned(),
                model: None,
                stop: "budget: turns".to_owned(),
            },
        ));
        assert!(log.get(id).unwrap().telemetry.resolver.is_none());
        assert!(!log.end_investigation(IncidentId(9_999), IncidentResolver::Person));
    }

    /// Lead ruling §8.2, IN2 §4.4 rule 19: `mark_proposal_stale` writes the
    /// flag and nothing else, on `Open` or `Investigating` entries carrying a
    /// non-stale proposal.
    #[test]
    fn in2_mark_proposal_stale_writes_the_flag_and_nothing_else() {
        fn proposal() -> IncidentProposal {
            IncidentProposal {
                operations: Vec::new(),
                operation_count: 0,
                summary: String::new(),
                explanation: "mark it".to_owned(),
                base_revision: TimelineRevision::default(),
                stale: false,
            }
        }

        let probed = untagged_mp4_probe();
        let mut marked = 0_usize;
        for state in ["investigating", "open"] {
            let mut log = IncidentLog::with_start(Instant::now(), None);
            let Observed::Opened(id) = log.observe(unknown_primaries_observation(&probed)) else {
                panic!("the first observation must open an incident");
            };
            assert!(log.begin_investigation(id));
            log.record_proposal(id, proposal()).unwrap();
            if state == "open" {
                assert!(log.end_investigation(id, IncidentResolver::Person));
            }
            let before = log.get(id).unwrap().clone();

            assert!(log.mark_proposal_stale(id));
            let after = log.get(id).unwrap();
            assert!(after.proposal.as_ref().unwrap().stale);
            assert_eq!(after.state, before.state);
            assert_eq!(after.telemetry, before.telemetry);
            assert_eq!(after.count, before.count);
            assert_eq!(after.revision, before.revision);
            assert_eq!(
                log.observe(unknown_primaries_observation(&probed)),
                Observed::Deduped(id),
                "`mark_proposal_stale` must not suppress"
            );
            marked += 1;
        }
        assert_eq!(marked, 2);

        // Already stale, no proposal, resolved, and unknown: all false, all
        // writing nothing.
        let mut log = IncidentLog::with_start(Instant::now(), None);
        let Observed::Opened(id) = log.observe(unknown_primaries_observation(&probed)) else {
            panic!("the first observation must open an incident");
        };
        assert!(log.begin_investigation(id));
        assert!(!log.mark_proposal_stale(id), "no proposal to mark");
        log.record_proposal(id, proposal()).unwrap();
        assert!(log.mark_proposal_stale(id));
        assert!(!log.mark_proposal_stale(id), "already stale");
        assert!(log.resolve(id, IncidentOutcome::Rejected));
        assert!(!log.mark_proposal_stale(id), "resolved entries stay shut");
        assert!(!log.mark_proposal_stale(IncidentId(9_999)));
    }

    /// IN2 §9.1 item 5, §4.5 rule 22: the one end that resolves.
    #[test]
    fn in2_an_explicit_reject_resolves_and_suppresses() {
        let probed = untagged_mp4_probe();
        let mut log = IncidentLog::with_start(Instant::now(), None);
        let Observed::Opened(id) = log.observe(unknown_primaries_observation(&probed)) else {
            panic!("the first observation must open an incident");
        };
        assert!(log.begin_investigation(id));
        assert!(log.resolve(id, IncidentOutcome::Rejected));

        let incident = log.get(id).unwrap();
        assert_eq!(
            incident.state,
            IncidentState::Resolved(IncidentOutcome::Rejected)
        );
        assert!(incident.telemetry.resolved_after.is_some());
        assert_eq!(log.open_count(), 0);
        assert_eq!(
            log.observe(unknown_primaries_observation(&probed)),
            Observed::Suppressed,
            "IN1 §2.3 rule 19 is unamended: `resolve` inserts for every outcome"
        );
        assert_eq!(log.len(), 1);
    }

    /// IN2 §9.1 item 6, §9.2 clause 4, §7 rules 1 and 7. The card half —
    /// `card_actions`' first action being `enabled` — is asserted by
    /// `kinewright-app`, which owns `card_actions`.
    #[test]
    fn in2_the_three_deterministic_rows_offer_a_button_and_the_sentence() {
        let buildable: Vec<IncidentCode> = DETERMINISTIC_SIXTEEN
            .iter()
            .filter(|(_, is_buildable)| *is_buildable)
            .map(|(code, _)| *code)
            .collect();
        assert!(!buildable.is_empty());

        let probed = all_unknown_probe();
        assert!(rec709_compatible(&probed));
        let evidence = source_color_evidence(&probed);
        let subject = IncidentSubject::Asset(AssetId(4));

        for code in buildable {
            let actions = policy_recovery(code, subject, &evidence);
            assert_eq!(actions.len(), 2, "{} must offer two actions", code.code());
            assert_eq!(
                actions[0].kind,
                RecoveryKind::Operation(assume_rec709_operation(AssetId(4), &probed)),
                "{}'s first action is the button",
                code.code()
            );
            assert_eq!(actions[0].label, ASSUME_REC709_LABEL);
            assert_eq!(
                actions[1].kind,
                RecoveryKind::Explain(explain_body(code)),
                "{}'s second action is still the sentence",
                code.code()
            );
            assert_eq!(actions[1].label, EXPLAIN_LABEL);

            // The no-harness fallback is the same value, by construction: the
            // incident stores exactly what `policy_recovery` returned.
            let mut log = IncidentLog::with_start(Instant::now(), None);
            let Observed::Opened(id) = log.observe(IncidentObservation {
                code,
                subject,
                observed: "unknown".to_owned(),
                allowed: None,
                evidence: evidence.clone(),
                revision: TimelineRevision(1),
                name: None,
                transient: false,
            }) else {
                panic!("a fresh log must open {}", code.code());
            };
            assert_eq!(log.get(id).unwrap().recoveries, actions);
        }
    }

    /// IN2 §9.1 item 7, §7 rule 5 — the predicate **and** the reachability.
    #[test]
    fn in2_an_unsupported_colour_row_still_offers_only_its_sentence() {
        let reached: [(IncidentCode, ColorDescription); 7] = [
            (
                IncidentCode::SourceColor(SourceColorIncident::UnsupportedPrimaries),
                ColorDescription {
                    primaries: ColorPrimaries::Bt2020,
                    ..all_unknown_probe()
                },
            ),
            (
                IncidentCode::SourceColor(SourceColorIncident::UnsupportedTransfer),
                ColorDescription {
                    transfer: ColorTransfer::Smpte2084,
                    ..all_unknown_probe()
                },
            ),
            (
                IncidentCode::SourceColor(SourceColorIncident::UnsupportedMatrix),
                ColorDescription {
                    matrix: ColorMatrix::Bt2020Ncl,
                    ..all_unknown_probe()
                },
            ),
            (
                IncidentCode::SourceColor(SourceColorIncident::UnsupportedRange),
                ColorDescription {
                    range: ColorRange::Other("weird".to_owned()),
                    ..all_unknown_probe()
                },
            ),
            (
                IncidentCode::SourceColor(SourceColorIncident::UnsupportedWhitePoint),
                ColorDescription {
                    white_point: ColorWhitePoint::D50,
                    ..all_unknown_probe()
                },
            ),
            (
                IncidentCode::SourceColor(SourceColorIncident::UnsupportedBitDepth),
                ColorDescription {
                    bit_depth: ColorBitDepth::Float32,
                    ..all_unknown_probe()
                },
            ),
            (
                IncidentCode::SourceColor(SourceColorIncident::UnsupportedCombination),
                ColorDescription {
                    primaries: ColorPrimaries::Bt709,
                    transfer: ColorTransfer::Srgb,
                    matrix: ColorMatrix::Bt709,
                    range: ColorRange::Limited,
                    ..all_unknown_probe()
                },
            ),
        ];

        // Half two of rule 5: the predicate alone does **not** do it — it is
        // `true` for an all-`Unknown` probe on these very codes.
        let all_unknown = all_unknown_probe();
        assert!(rec709_compatible(&all_unknown));

        let mut checked = 0_usize;
        for (code, probed) in reached {
            assert!(
                !rec709_compatible(&probed),
                "{} is only reached with a known non-Rec.709 value",
                code.code()
            );
            let evidence = source_color_evidence(&probed);
            let actions = policy_recovery(code, IncidentSubject::Asset(AssetId(1)), &evidence);
            assert_eq!(actions.len(), 1, "{} offers only its sentence", code.code());
            assert_eq!(actions[0].kind, RecoveryKind::Explain(explain_body(code)));
            assert!(
                deterministic_recovery(code, IncidentSubject::Asset(AssetId(1)), &evidence)
                    .is_none()
            );
            // And not even with the admitted probe: the row has no builder.
            assert!(
                deterministic_recovery(
                    code,
                    IncidentSubject::Asset(AssetId(1)),
                    &source_color_evidence(&all_unknown),
                )
                .is_none()
            );
            checked += 1;
        }
        assert_eq!(checked, 7);
    }

    /// IN2 §9.1 item 8, §7 rule 6 — each of rows 11-16 through its **real**
    /// producer, so the test pins the producer and not a hand-built value.
    #[test]
    fn in2_the_six_delivery_and_clamp_rows_carry_no_typed_evidence() {
        let cases: Vec<(&str, MediaError)> = vec![
            (
                "unsupported_delivery_codec",
                MediaError::DeliveryColor(DeliveryColorError::UnsupportedCodec {
                    observed: "prores_ks".to_owned(),
                    allowed: "libx264",
                }),
            ),
            (
                "unsupported_delivery_color",
                MediaError::DeliveryColor(DeliveryColorError::UnsupportedField(
                    DeliveryColorMismatch {
                        field: "primaries".to_owned(),
                        observed: "Bt2020".to_owned(),
                        allowed: "Bt709".to_owned(),
                    },
                )),
            ),
            (
                "delivery_pixel_format_depth_mismatch",
                MediaError::DeliveryColor(DeliveryColorError::PixelFormatDepthMismatch {
                    observed: "yuv420p".to_owned(),
                    allowed: "yuv420p10le".to_owned(),
                }),
            ),
            (
                "delivery_encoder_pixel_format_unavailable",
                MediaError::DeliveryColor(DeliveryColorError::EncoderPixelFormatUnavailable {
                    observed: "yuv420p".to_owned(),
                    allowed: "yuv420p10le".to_owned(),
                }),
            ),
            (
                "color_qc_node_budget_exceeded",
                MediaError::ColorQc(ColorQcError::NodeBudgetExceeded {
                    observed: "17".to_owned(),
                    allowed: "1..=16",
                }),
            ),
            (
                "delivery_verification_frame_count_out_of_range",
                MediaError::DeliveryVerification(DeliveryVerificationError::FrameCountOutOfRange {
                    observed: "32".to_owned(),
                    allowed: "1..=16",
                }),
            ),
        ];
        assert_eq!(cases.len(), 6);

        let table: BTreeSet<&str> = DETERMINISTIC_SIXTEEN[10..16]
            .iter()
            .map(|(code, _)| code.code())
            .collect();
        let mut checked = 0_usize;
        for (expected, error) in cases {
            assert!(table.contains(expected));
            let observation = IncidentObservation::from_media_error(
                &error,
                IncidentSubject::ExportJob,
                TimelineRevision::default(),
            );
            assert_eq!(observation.code.code(), expected);
            assert!(
                matches!(observation.evidence, IncidentEvidence::MediaError { .. }),
                "{expected} carries only a code and a rendered sentence"
            );
            assert!(observation.evidence.probed().is_none());
            assert!(
                deterministic_recovery(
                    observation.code,
                    observation.subject,
                    &observation.evidence,
                )
                .is_none()
            );
            let actions =
                policy_recovery(observation.code, observation.subject, &observation.evidence);
            assert_eq!(actions.len(), 1);
            assert!(matches!(actions[0].kind, RecoveryKind::Explain(_)));
            checked += 1;
        }
        assert_eq!(checked, 6);
    }

    /// IN2 §9.1 item 9, §4.2 rule 8.
    #[test]
    fn in2_a_recorded_proposal_never_serialises_its_operations() {
        let probed = untagged_mp4_probe();
        let mut log = IncidentLog::with_start(Instant::now(), None);
        let Observed::Opened(id) = log.observe(unknown_primaries_observation(&probed)) else {
            panic!("the first observation must open an incident");
        };

        let proposal = IncidentProposal {
            operations: vec![
                Operation::DeleteClip { clip: ClipId(1) },
                Operation::DeleteClip { clip: ClipId(2) },
                Operation::DeleteClip { clip: ClipId(3) },
            ],
            operation_count: 3,
            summary: "remove_clip 1, remove_clip 2, remove_clip 3".to_owned(),
            explanation: "Three clips point past their source end.".to_owned(),
            base_revision: TimelineRevision(12),
            stale: false,
        };

        // Only on an `Investigating` entry.
        assert_eq!(
            log.record_proposal(id, proposal.clone()),
            Err(RecordProposalError::NotInvestigating)
        );
        assert!(log.begin_investigation(id));
        assert_eq!(log.record_proposal(id, proposal.clone()), Ok(()));
        assert_eq!(
            log.get(id).unwrap().state,
            IncidentState::Investigating,
            "`record_proposal` does not resolve and does not change state"
        );
        assert_eq!(log.get(id).unwrap().proposal.as_ref(), Some(&proposal));

        let body = serde_json::to_string(log.get(id).unwrap()).unwrap();
        assert!(body.contains(r#""operation_count":3"#));
        assert!(
            !body.contains("\"operations\""),
            "a `Vec<Operation>` on the wire would end the ceiling as a property"
        );
        assert!(body.contains(r#""base_revision":12"#));
        assert!(body.contains(r#""stale":false"#));

        // Resolved entries refuse it too.
        let mut log = IncidentLog::with_start(Instant::now(), None);
        let Observed::Opened(id) = log.observe(unknown_primaries_observation(&probed)) else {
            panic!("the first observation must open an incident");
        };
        assert!(log.resolve(id, IncidentOutcome::Explained));
        assert_eq!(
            log.record_proposal(id, proposal),
            Err(RecordProposalError::NotInvestigating)
        );
        assert!(log.get(id).unwrap().proposal.is_none());
        assert_eq!(
            log.record_proposal(IncidentId(9_999), tiny_proposal()),
            Err(RecordProposalError::NotFound)
        );
    }

    /// IN2 erratum A-R7: a second session makes a new proposal, and the
    /// previous one is stale from the moment that session begins.
    #[test]
    fn in2_a_second_session_marks_the_old_proposal_stale_and_replaces_it() {
        let probed = untagged_mp4_probe();
        let mut log = IncidentLog::with_start(Instant::now(), None);
        let Observed::Opened(id) = log.observe(unknown_primaries_observation(&probed)) else {
            panic!("the first observation must open an incident");
        };

        let first = IncidentProposal {
            explanation: "the first session's sentence".to_owned(),
            base_revision: TimelineRevision(3),
            ..tiny_proposal()
        };
        let second = IncidentProposal {
            explanation: "the second session's sentence".to_owned(),
            base_revision: TimelineRevision(11),
            ..tiny_proposal()
        };

        assert!(log.begin_investigation(id));
        assert_eq!(log.record_proposal(id, first.clone()), Ok(()));
        assert!(!log.get(id).unwrap().proposal.as_ref().unwrap().stale);

        // A second record while the first is live is refused, and nothing moves.
        assert_eq!(
            log.record_proposal(id, second.clone()),
            Err(RecordProposalError::AlreadyRecorded)
        );
        assert_eq!(log.get(id).unwrap().proposal.as_ref(), Some(&first));

        // The session stops without an outcome; the incident goes back to
        // `Open` carrying the proposal and the stop.
        assert!(log.end_investigation(
            id,
            IncidentResolver::Session {
                harness: "scripted".to_owned(),
                model: None,
                stop: "budget: turns".to_owned(),
            },
        ));
        let stopped = log.get(id).unwrap();
        assert_eq!(stopped.state, IncidentState::Open);
        assert!(!stopped.proposal.as_ref().unwrap().stale);
        assert!(stopped.telemetry.resolver.is_some());

        // Re-investigating marks it stale and leaves telemetry as history.
        assert!(log.begin_investigation(id));
        let reinvestigating = log.get(id).unwrap();
        assert_eq!(reinvestigating.state, IncidentState::Investigating);
        assert!(
            reinvestigating.proposal.as_ref().unwrap().stale,
            "a proposal proved two sessions ago must not offer Approve"
        );
        assert_eq!(
            reinvestigating.proposal.as_ref().unwrap().base_revision,
            first.base_revision,
            "only `stale` moves; the record is otherwise the first session's"
        );
        assert!(
            reinvestigating.telemetry.resolver.is_some(),
            "the previous session's stop is history and is not cleared"
        );

        // And the new session's proposal replaces the stale one.
        assert_eq!(log.record_proposal(id, second.clone()), Ok(()));
        let replaced = log.get(id).unwrap().proposal.as_ref().unwrap();
        assert_eq!(replaced, &second);
        assert!(!replaced.stale);
    }

    fn tiny_proposal() -> IncidentProposal {
        IncidentProposal {
            operations: Vec::new(),
            operation_count: 0,
            summary: String::new(),
            explanation: String::new(),
            base_revision: TimelineRevision::default(),
            stale: false,
        }
    }

    /// IN2 §9.1 item 10, §4.1 rule 5 — the cap is on the **serialised** string,
    /// and `json_escaped_len` is asserted against `serde_json` itself rather
    /// than trusted.
    #[test]
    fn in2_both_proposal_strings_are_capped_on_their_serialised_length() {
        let ceiling = INVESTIGATOR_EXPLANATION_CEILING_BYTES;
        assert_eq!(ceiling, 240);

        let cases = [
            "\u{1}".repeat(240),
            "é".repeat(240),
            "\u{1f600}".repeat(240),
            "\"\\\n\t".repeat(80),
            "a".repeat(240),
            "a".repeat(10),
            String::new(),
        ];
        let mut checked = 0_usize;
        for raw in &cases {
            // The helper agrees with `serde_json` byte for byte.
            assert_eq!(
                json_escaped_len(raw),
                serde_json::to_string(raw).unwrap().len() - 2,
                "the escaped-length model must equal serde_json's"
            );

            let capped = truncate_to_serialized_bytes(raw, ceiling);
            let serialised = serde_json::to_string(&capped).unwrap();
            assert!(
                serialised.len() <= ceiling + 2,
                "{} bytes serialise to {}",
                capped.len(),
                serialised.len()
            );
            assert!(raw.starts_with(&capped), "truncation is a prefix");
            assert!(raw.is_char_boundary(capped.len()));
            if json_escaped_len(raw) <= ceiling {
                assert_eq!(&capped, raw, "a string that fits is not touched");
            }
            checked += 1;
        }
        assert_eq!(checked, cases.len());

        // 240 control characters serialise to about 1 440 B raw, which is the
        // whole reason the cap is on the serialised string.
        assert_eq!(json_escaped_len(&"\u{1}".repeat(240)), 1_440);

        let proposal = IncidentProposal {
            operations: Vec::new(),
            operation_count: 0,
            summary: truncate_to_serialized_bytes(&"é".repeat(240), ceiling),
            explanation: truncate_to_serialized_bytes(&"\u{1}".repeat(240), ceiling),
            base_revision: TimelineRevision::default(),
            stale: false,
        };
        assert!(serde_json::to_string(&proposal.summary).unwrap().len() <= 242);
        assert!(serde_json::to_string(&proposal.explanation).unwrap().len() <= 242);
    }

    /// IN2 §9.1 item 11, §5.4 rule 12 — the three new keys skip when absent, so
    /// `IN1_INCIDENT_SERIALIZED_BYTES` cannot move.
    #[test]
    fn in2_the_proposal_and_the_new_telemetry_fields_skip_when_absent() {
        let mut log = IncidentLog::with_start(Instant::now(), None);
        let probed = untagged_webm_probe();
        let Observed::Opened(id) = log.observe(unknown_primaries_observation(&probed)) else {
            panic!("the first observation must open an incident");
        };
        assert_eq!(
            log.observe(unknown_primaries_observation(&probed)),
            Observed::Deduped(id)
        );

        let incident = log.get(id).unwrap();
        assert!(incident.proposal.is_none());
        assert!(incident.telemetry.turns.is_none());
        assert!(incident.telemetry.resolver.is_none());

        let body = serde_json::to_string(incident).unwrap();
        for key in ["\"proposal\"", "\"turns\"", "\"resolver\""] {
            assert!(!body.contains(key), "{key} must skip when absent");
        }
        assert_eq!(body, IN1_PINNED_WIRE_BODY);
        assert_eq!(body.len(), 819, "`IN1_INCIDENT_SERIALIZED_BYTES`");
    }

    /// IN2 §9.1 item 12, §9.2 clause 13, §2.4.
    #[test]
    fn in2_a_muted_code_round_trips_through_the_document_byte_identically() {
        // The pre-IN2 project has no `investigator` key and loads and re-saves
        // byte-unchanged. The same pin lives in `tests/contracts.rs`; it is
        // repeated here because clause 13 is about the field this slice adds.
        let legacy: Document = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/pre_m13_project.json"
        )))
        .unwrap();
        assert!(legacy.investigator.is_none());
        let round_tripped = serde_json::to_vec(&legacy).unwrap();
        assert!(
            !round_tripped
                .windows(12)
                .any(|window| window == b"investigator")
        );
        assert_eq!(round_tripped.len(), 1_215);
        assert_eq!(
            format!("{:016x}", fnv1a64_digest(&round_tripped)),
            "c9da3186e131e4fd"
        );

        // One muted code round-trips, and an unrecognised string is inert.
        let muted = Document {
            investigator: Some(InvestigatorPreferences {
                muted_codes: vec![
                    IncidentCode::Label(LabelIncident::Operations)
                        .code()
                        .to_owned(),
                    "a_code_this_build_has_never_heard_of".to_owned(),
                ],
            }),
            ..Document::default()
        };
        let encoded = serde_json::to_string(&muted).unwrap();
        assert!(encoded.contains(r#""investigator":{"muted_codes":["operations_unclassified""#));
        let decoded: Document = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, muted);
        let preferences = decoded.investigator.unwrap();
        assert_eq!(preferences.muted_codes.len(), 2);
        assert!(
            preferences
                .muted_codes
                .iter()
                .any(|code| code == "operations_unclassified")
        );
        assert!(
            !EVERY_INCIDENT_CODE
                .iter()
                .any(|code| code.code() == "a_code_this_build_has_never_heard_of"),
            "an unrecognised string matches no code and is therefore inert"
        );

        // An empty preference block still costs no `muted_codes` key.
        let empty = Document {
            investigator: Some(InvestigatorPreferences::default()),
            ..Document::default()
        };
        let encoded = serde_json::to_string(&empty).unwrap();
        assert!(encoded.contains(r#""investigator":{}"#));
    }

    /// IN2B §2 rule 4 (N2/S-6): `generation()` moves on every mutation and
    /// on no no-op, and the pure `should_flush` compares generations.
    ///
    /// Not a §12 item (stage A2 lands none): a unit test for the new public
    /// surface, which stage C1's flush tests build on.
    #[test]
    fn generation_moves_on_mutation_and_should_flush_compares_generations() {
        let mut log = IncidentLog::with_start(Instant::now(), None);
        assert_eq!(log.generation(), 0);
        let now = Instant::now();
        assert!(!should_flush(log.generation(), 0, now));

        let observation = || {
            IncidentObservation::plain(
                IncidentCode::Label(LabelIncident::Project),
                IncidentSubject::Project,
                "could not read the project file",
                TimelineRevision(1),
            )
        };
        let Observed::Opened(id) = log.observe(observation()) else {
            panic!("a fresh log must open");
        };
        assert_eq!(log.generation(), 1);
        assert!(should_flush(log.generation(), 0, now));
        assert!(!should_flush(log.generation(), 1, now));

        assert_eq!(log.observe(observation()), Observed::Deduped(id));
        assert_eq!(log.generation(), 2);

        assert!(log.refresh_revision(id, TimelineRevision(2)));
        assert_eq!(log.generation(), 3);
        assert!(!log.refresh_revision(IncidentId(999), TimelineRevision(2)));
        assert_eq!(log.generation(), 3, "an unknown id changes nothing");

        let telemetry = log.telemetry_mut(id);
        assert!(telemetry.is_some());
        assert_eq!(log.generation(), 4);
        assert!(log.telemetry_mut(IncidentId(999)).is_none());
        assert_eq!(log.generation(), 4, "an unknown id hands out nothing");

        assert!(log.begin_investigation(id));
        assert_eq!(log.generation(), 5);
        assert!(!log.begin_investigation(id));
        assert_eq!(log.generation(), 5, "a refused begin changes nothing");

        let proposal = || IncidentProposal {
            operations: Vec::new(),
            operation_count: 0,
            summary: "summary".to_owned(),
            explanation: "explanation".to_owned(),
            base_revision: TimelineRevision(1),
            stale: false,
        };
        assert!(log.record_proposal(id, proposal()).is_ok());
        assert_eq!(log.generation(), 6);
        assert_eq!(
            log.record_proposal(id, proposal()),
            Err(RecordProposalError::AlreadyRecorded)
        );
        assert_eq!(log.generation(), 6, "a refused record changes nothing");

        assert!(log.mark_proposal_stale(id));
        assert_eq!(log.generation(), 7);
        assert!(!log.mark_proposal_stale(id));
        assert_eq!(log.generation(), 7, "an already-stale mark changes nothing");

        assert!(log.end_investigation(id, IncidentResolver::Person));
        assert_eq!(log.generation(), 8);
        assert!(!log.end_investigation(id, IncidentResolver::Person));
        assert_eq!(log.generation(), 8, "ending a non-session changes nothing");

        assert!(log.note_auto_applied(id));
        assert_eq!(log.generation(), 9);

        assert!(log.resolve(id, IncidentOutcome::Explained));
        assert_eq!(log.generation(), 10);
        assert!(!log.resolve(IncidentId(999), IncidentOutcome::Explained));
        assert_eq!(log.generation(), 10, "an unknown id changes nothing");

        assert_eq!(log.observe(observation()), Observed::Suppressed);
        assert_eq!(log.generation(), 10, "a suppressed observation is not news");
    }

    fn assert_json_round_trip<T>(value: &T)
    where
        T: serde::Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug,
    {
        let body = serde_json::to_string(value).unwrap();
        let decoded: T = serde_json::from_str(&body).unwrap();
        assert_eq!(&decoded, value, "{body}");
    }

    /// IN2B stage A2: the new `Deserialize` derives round-trip through real
    /// JSON — the groundwork A6's record/restore items build on.
    ///
    /// Not a §12 item (stage A2 lands none): unit tests proving the derives
    /// expand to working deserialisers, not only to compiling ones.
    #[test]
    fn incident_subject_and_evidence_round_trip_through_json() {
        for subject in [
            IncidentSubject::Asset(AssetId(3)),
            IncidentSubject::LutAsset(crate::LutAssetId(5)),
            IncidentSubject::Clip(crate::ClipId(9)),
            IncidentSubject::Track(crate::TrackId(2)),
            IncidentSubject::Chain(AudioChain::Bus(crate::AudioBusId(3))),
            IncidentSubject::Chain(AudioChain::Master),
            IncidentSubject::ExportJob,
            IncidentSubject::Project,
            IncidentSubject::Agent,
        ] {
            assert_json_round_trip(&subject);
        }

        let probed = untagged_webm_probe();
        for evidence in [
            IncidentEvidence::SourceColor {
                probed: probed.clone(),
                assumption: Some(ColorSourceProfileAssumption::D65),
            },
            IncidentEvidence::Plain,
            IncidentEvidence::OpError {
                family: IncidentFamily::Bounds,
                op_number: Some(3),
                message: "message".to_owned(),
            },
            IncidentEvidence::MediaError {
                code: "media_backend_unclassified".to_owned(),
                message: "message".to_owned(),
            },
            IncidentEvidence::Revision {
                expected: TimelineRevision(7),
                actual: TimelineRevision(9),
            },
            IncidentEvidence::SourceEdit {
                reason: "reason".to_owned(),
            },
            IncidentEvidence::Relink {
                reason: "reason".to_owned(),
            },
            IncidentEvidence::ProjectSave {
                reason: "reason".to_owned(),
            },
            IncidentEvidence::Branch {
                reason: "reason".to_owned(),
            },
            IncidentEvidence::CaptionPlan {
                reason: "reason".to_owned(),
            },
            IncidentEvidence::DeliveryVariant {
                reason: "reason".to_owned(),
            },
        ] {
            assert_json_round_trip(&evidence);
        }
    }

    /// IN2B stage A2, second half: outcomes, states, resolvers, telemetry
    /// and proposals round-trip through real JSON.
    #[test]
    fn incident_state_telemetry_and_proposal_round_trip_through_json() {
        for outcome in [
            IncidentOutcome::Applied,
            IncidentOutcome::Reverted,
            IncidentOutcome::Explained,
            IncidentOutcome::Rejected,
        ] {
            assert_json_round_trip(&outcome);
            assert_json_round_trip(&IncidentState::Resolved(outcome));
        }
        assert_json_round_trip(&IncidentState::Open);
        assert_json_round_trip(&IncidentState::Investigating);

        assert_json_round_trip(&IncidentResolver::Router);
        assert_json_round_trip(&IncidentResolver::Person);
        assert_json_round_trip(&IncidentResolver::Session {
            harness: "harness".to_owned(),
            model: Some("model".to_owned()),
            stop: "stop".to_owned(),
        });

        assert_json_round_trip(&IncidentTelemetry::default());
        // `resolved_after` stays on the wire when present (IN1 §2.5) but never
        // in a record (N-9): the builder clears it (N4 F1).
        let saturated = IncidentTelemetry {
            resolved_after: Some(Duration::from_secs(9)),
            tool_calls: 3,
            input_tokens: Some(100),
            cached_input_tokens: Some(10),
            cache_creation_input_tokens: Some(11),
            output_tokens: Some(50),
            reasoning_output_tokens: Some(5),
            cost_usd_millionths: Some(42),
            turns: Some(9),
            resolver: Some(IncidentResolver::Session {
                harness: "harness".to_owned(),
                model: None,
                stop: "stop".to_owned(),
            }),
        };
        let body = serde_json::to_string(&saturated).unwrap();
        assert!(body.contains("resolved_after"), "served while set: {body}");
        let decoded: IncidentTelemetry = serde_json::from_str(&body).unwrap();
        assert_eq!(decoded, saturated);

        let mut log = IncidentLog::with_start(Instant::now(), None);
        let Observed::Opened(id) = log.observe(IncidentObservation::plain(
            IncidentCode::EditRevisionConflict,
            IncidentSubject::Project,
            "revision 9",
            TimelineRevision(9),
        )) else {
            panic!("the first observation must open");
        };
        assert!(log.resolve(id, IncidentOutcome::Applied));
        assert!(log.get(id).unwrap().telemetry.resolved_after.is_some());
        let (records, _) = log.records(None, &BTreeMap::new());
        assert_eq!(records.len(), 1);
        assert!(records[0].telemetry.resolved_after.is_none());
        let record_body = serde_json::to_string(&records[0]).unwrap();
        assert!(
            !record_body.contains("resolved_after"),
            "log-origin-relative, never in a record: {record_body}"
        );

        let proposal = IncidentProposal {
            operations: Vec::new(),
            operation_count: 2,
            summary: "summary".to_owned(),
            explanation: "explanation".to_owned(),
            base_revision: TimelineRevision(41),
            stale: false,
        };
        let body = serde_json::to_string(&proposal).unwrap();
        assert!(
            !body.contains("operations"),
            "operations never serialise: {body}"
        );
        assert_json_round_trip(&proposal);
        // `#[serde(skip)]` ignores the key even when a hand-written record
        // carries it: operations never resurrect through the derive.
        let decoded = serde_json::from_str::<IncidentProposal>(
            r#"{"operations":[1,2,3],"operation_count":2,"summary":"s","explanation":"e","base_revision":41,"stale":false}"#,
        )
        .unwrap();
        assert!(decoded.operations.is_empty());
        assert_eq!(decoded.operation_count, 2);
    }

    /// FNV-1a 64, the digest `tests/contracts.rs` pins the legacy round trip
    /// with.
    fn fnv1a64_digest(bytes: &[u8]) -> u64 {
        let mut hash = 0xcbf2_9ce4_8422_2325_u64;
        for byte in bytes {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x100_0000_01b3);
        }
        hash
    }

    /// Minimal hand-built record: the caller overrides the fields its item
    /// pins; everything else is the quietest legal value.
    fn stored_record(id: u64, code: &str) -> IncidentRecord {
        IncidentRecord {
            id: IncidentId(id),
            code: code.to_owned(),
            subject: IncidentSubject::Project,
            observed: format!("observed {id}"),
            allowed: None,
            evidence: IncidentEvidence::Plain,
            revision: TimelineRevision(7),
            opened_wall_millis: None,
            opened_offset_nanos: 1_000 * id,
            count: 1,
            state: IncidentState::Open,
            telemetry: IncidentTelemetry::default(),
            proposal: None,
            subject_name: None,
            refused_op: None,
        }
    }

    /// Through JSON and back, as the sidecar loader delivers records — which
    /// also proves the record's own derives on every item below.
    fn parsed_records(stored: Vec<IncidentRecord>) -> Vec<IncidentRecord> {
        stored
            .into_iter()
            .map(|record| {
                let body = serde_json::to_string(&record).unwrap();
                serde_json::from_str(&body).unwrap()
            })
            .collect()
    }

    /// Parse records after arming each with `resolved_after`, so item 2's
    /// `is_none` check is falsifiable rather than vacuous (N4 F10): the assert
    /// proves the field actually survived JSON this far.
    fn parsed_records_with_resolved_after(stored: Vec<IncidentRecord>) -> Vec<IncidentRecord> {
        let armed = stored
            .into_iter()
            .map(|mut record| {
                record.telemetry.resolved_after = Some(Duration::from_secs(9));
                record
            })
            .collect();
        let parsed = parsed_records(armed);
        assert!(
            parsed
                .iter()
                .all(|record| record.telemetry.resolved_after.is_some())
        );
        parsed
    }

    /// A live-looking proposal: recorded, counted, and not yet stale — the
    /// shape restore must force stale with no operations.
    fn live_looking_proposal() -> IncidentProposal {
        IncidentProposal {
            operations: Vec::new(),
            operation_count: 2,
            summary: "two operations".to_owned(),
            explanation: "apply both".to_owned(),
            base_revision: TimelineRevision(5),
            stale: false,
        }
    }

    /// One open entry with an exact `opened_at`, for the wall tests: the wall
    /// math is under test, not the session clock, and no test reads a clock.
    fn log_with_exact_opened_at(origin: Option<SystemTime>, opened_at: Duration) -> IncidentLog {
        let mut log = IncidentLog::with_start(Instant::now(), origin);
        let Observed::Opened(id) = log.observe(IncidentObservation::plain(
            IncidentCode::EditRevisionConflict,
            IncidentSubject::Project,
            "revision 9",
            TimelineRevision(9),
        )) else {
            panic!("the first observation must open");
        };
        log.entries
            .iter_mut()
            .find(|entry| entry.id == id)
            .unwrap()
            .opened_at = opened_at;
        log
    }

    /// `IN2B` §12 item 1 (`IN2B` §3 rule 3): `from_code` round-trips every
    /// `POLICY` row and rejects unknown strings.
    #[test]
    fn in2b_from_code_round_trips_all_seventy_four_codes_and_rejects_three_unknowns() {
        assert_eq!(POLICY.len(), 74);
        for entry in &POLICY {
            assert_eq!(
                from_code(entry.code.code()),
                Some(entry.code),
                "{} must resolve to itself",
                entry.code.code()
            );
        }
        assert_eq!(from_code(""), None);
        assert_eq!(from_code("no_such_code"), None);
        let wrong_case = POLICY[0].code.code().to_ascii_uppercase();
        assert_ne!(wrong_case, POLICY[0].code.code());
        assert_eq!(from_code(&wrong_case), None);
    }

    /// `IN2B` §12 item 2 (`IN2B` §3 rules 1–2): restore recomputes every
    /// derived field from the current policy and takes the rest verbatim.
    #[test]
    fn in2b_restore_recomputes_every_derived_field_from_current_policy() {
        let auto_code = IncidentCode::SourceColor(SourceColorIncident::UnknownPrimaries);
        let mut auto_apply = stored_record(3, auto_code.code());
        auto_apply.subject = IncidentSubject::Asset(AssetId(1));
        auto_apply.evidence = IncidentEvidence::SourceColor {
            probed: untagged_mp4_probe(),
            assumption: None,
        };
        auto_apply.count = 4;
        auto_apply.subject_name = Some("Interview.mov".to_owned());
        auto_apply.allowed = Some("Rec.709".to_owned());
        auto_apply.refused_op = Some(Operation::DeleteClip { clip: ClipId(1) });
        auto_apply.telemetry.turns = Some(6);
        auto_apply.telemetry.resolver = Some(IncidentResolver::Session {
            harness: "harness".to_owned(),
            model: Some("model".to_owned()),
            stop: "completed".to_owned(),
        });
        assert_eq!(
            policy_class(auto_code, &auto_apply.evidence),
            PolicyClass::AutoApply
        );

        let explain_code = IncidentCode::Label(LabelIncident::PanelWorkerError);
        let mut explained = stored_record(9, explain_code.code());
        explained.state = IncidentState::Resolved(IncidentOutcome::Explained);
        explained.subject_name = Some("Scopes".to_owned());

        let mut conflict = stored_record(12, IncidentCode::EditRevisionConflict.code());
        conflict.opened_wall_millis = Some(1_758_624_000_123);

        // Every record carries `resolved_after` into restore, so the `is_none`
        // check below is falsifiable rather than vacuous (N4 F10).
        let parsed =
            parsed_records_with_resolved_after(vec![auto_apply.clone(), explained, conflict]);

        let mut log = IncidentLog::with_start(Instant::now(), None);
        let report = log.restore(
            parsed,
            vec!["{\"carried\":true}".to_owned()],
            TimelineRevision(100),
            None,
        );
        assert_eq!(report.restored_open, 2);
        assert_eq!(report.restored_resolved, 1);
        assert!(report.unknown_codes.is_empty());
        assert_eq!(report.carried, vec!["{\"carried\":true}".to_owned()]);
        assert_eq!(report.rebased, 3);
        for (id, evidence, subject, code) in [
            (
                IncidentId(3),
                &auto_apply.evidence,
                auto_apply.subject,
                auto_code,
            ),
            (
                IncidentId(9),
                &IncidentEvidence::Plain,
                IncidentSubject::Project,
                explain_code,
            ),
            (
                IncidentId(12),
                &IncidentEvidence::Plain,
                IncidentSubject::Project,
                IncidentCode::EditRevisionConflict,
            ),
        ] {
            let loaded = log.get(id).unwrap();
            assert_eq!(loaded.class, policy_class(code, evidence));
            assert_eq!(loaded.severity, policy_severity(code));
            assert_eq!(loaded.field, code.field());
            assert_eq!(loaded.recoveries, policy_recovery(code, subject, evidence));
            assert!(loaded.telemetry.resolved_after.is_none());
            assert_eq!(loaded.revision, TimelineRevision(100));
            assert!(!loaded.transient);
        }
        let loaded_auto = log.get(IncidentId(3)).unwrap();
        assert_eq!(loaded_auto.evidence, auto_apply.evidence);
        assert_eq!(loaded_auto.count, 4);
        assert_eq!(loaded_auto.allowed, Some("Rec.709".to_owned()));
        assert_eq!(loaded_auto.subject_name, Some("Interview.mov".to_owned()));
        assert_eq!(
            loaded_auto.opened_at,
            Duration::from_nanos(auto_apply.opened_offset_nanos)
        );
        assert_eq!(loaded_auto.telemetry.turns, Some(6));
        assert_eq!(
            report.refused.get(&IncidentId(3)),
            auto_apply.refused_op.as_ref()
        );
        assert_eq!(report.refused.len(), 1);
        // `next_id` resumes above the loaded maximum: the next open takes 13.
        let Observed::Opened(fresh) = log.observe(IncidentObservation::plain(
            explain_code,
            IncidentSubject::Agent,
            "a fresh problem",
            TimelineRevision(100),
        )) else {
            panic!("a fresh observation must open");
        };
        assert_eq!(fresh, IncidentId(13));
    }

    /// `IN2B` §12 item 3 (`IN2B` §3 rule 6): unknown codes skip, count, and
    /// report once through a single aggregate.
    #[test]
    fn in2b_restore_skips_unknown_codes_and_aggregates_once() {
        let mut first = stored_record(1, "nope_a");
        first.observed = "first".to_owned();
        let mut second = stored_record(2, "nope_b");
        second.observed = "second".to_owned();
        let mut third = stored_record(3, "nope_a");
        third.observed = "third".to_owned();
        let mut log = IncidentLog::with_start(Instant::now(), None);
        let report = log.restore(
            parsed_records(vec![first, second, third]),
            Vec::new(),
            TimelineRevision(100),
            None,
        );
        assert_eq!(report.restored_open, 0);
        assert_eq!(report.restored_resolved, 0);
        assert_eq!(
            report.unknown_codes,
            vec![("nope_a".to_owned(), 2), ("nope_b".to_owned(), 1)]
        );
        assert_eq!(log.len(), 1);
        let aggregate = log.open().next().unwrap();
        assert_eq!(
            aggregate.code,
            IncidentCode::Label(LabelIncident::SidecarUnknownCodes)
        );
        assert_eq!(
            aggregate.observed,
            "incidents used codes this build does not know — 3 skipped"
        );
        assert_eq!(aggregate.count, 1);
        // The aggregate is transient and the next write drops it, to be
        // re-aggregated at the next load (N4 F10).
        assert!(aggregate.transient);
        let (_, write_report) = log.records(None, &BTreeMap::new());
        assert_eq!(write_report.written_open, 0);
        assert_eq!(write_report.dropped_transient_open, 1);
    }

    /// `IN2B` §12 item 4 (`IN2B` §3 rule 7): every loaded proposal is stale
    /// with no operations.
    #[test]
    fn in2b_restore_forces_every_proposal_stale_with_no_operations() {
        let mut stored = stored_record(5, IncidentCode::EditRevisionConflict.code());
        stored.proposal = Some(live_looking_proposal());
        let mut log = IncidentLog::with_start(Instant::now(), None);
        let report = log.restore(
            parsed_records(vec![stored]),
            Vec::new(),
            TimelineRevision(100),
            None,
        );
        assert_eq!(report.restored_open, 1);
        let loaded = log.get(IncidentId(5)).unwrap();
        let proposal = loaded.proposal.as_ref().unwrap();
        assert!(proposal.stale);
        assert!(proposal.operations.is_empty());
        assert!(proposal.operation_count >= 1);
        assert_eq!(proposal.base_revision, TimelineRevision(100));
    }

    /// `IN2B` §12 item 5 (`IN2B` §3 rule 8): restore writes no suppression; a
    /// re-observation opens fresh.
    #[test]
    fn in2b_restore_writes_no_suppression_and_reobservation_opens_fresh() {
        let code = IncidentCode::SourceColor(SourceColorIncident::UnknownRange);
        let mut first = stored_record(4, code.code());
        first.state = IncidentState::Resolved(IncidentOutcome::Applied);
        let mut second = stored_record(6, code.code());
        second.observed = "another value".to_owned();
        second.state = IncidentState::Resolved(IncidentOutcome::Explained);
        let mut log = IncidentLog::with_start(Instant::now(), None);
        let report = log.restore(
            parsed_records(vec![first, second]),
            Vec::new(),
            TimelineRevision(100),
            None,
        );
        assert_eq!(report.restored_resolved, 2);
        let Observed::Opened(fresh) = log.observe(IncidentObservation::plain(
            code,
            IncidentSubject::Project,
            "observed 4",
            TimelineRevision(100),
        )) else {
            panic!("a re-observation of a restored resolved pair must open fresh");
        };
        assert_eq!(fresh, IncidentId(7));
        assert_eq!(log.get(fresh).unwrap().state, IncidentState::Open);
    }

    /// `IN2B` §12 item 6 (`IN2B` §3 rule 9): loaded revisions rebase to the
    /// opening revision.
    #[test]
    fn in2b_restore_rebases_both_revisions_to_opening() {
        let mut first = stored_record(1, IncidentCode::EditRevisionConflict.code());
        first.revision = TimelineRevision(7);
        first.proposal = Some(live_looking_proposal());
        let mut second = stored_record(
            2,
            IncidentCode::Label(LabelIncident::PanelWorkerError).code(),
        );
        second.revision = TimelineRevision(41);
        second.state = IncidentState::Resolved(IncidentOutcome::Explained);
        second.proposal = Some(live_looking_proposal());
        let mut log = IncidentLog::with_start(Instant::now(), None);
        let report = log.restore(
            parsed_records(vec![first, second]),
            Vec::new(),
            TimelineRevision(100),
            None,
        );
        assert_eq!(report.rebased, 4);
        for id in [IncidentId(1), IncidentId(2)] {
            let loaded = log.get(id).unwrap();
            assert_eq!(loaded.revision, TimelineRevision(100));
            assert_eq!(
                loaded.proposal.as_ref().unwrap().base_revision,
                TimelineRevision(100)
            );
        }
    }

    /// `IN2B` §12 item 7 (`IN2B` §2 rules 11–12): the builder drops transient
    /// opens and prunes resolved oldest-first by id.
    #[test]
    fn in2b_the_record_builder_drops_transient_opens_and_prunes_oldest_first() {
        let mut log = IncidentLog::with_start(Instant::now(), None);
        let resolved_code = IncidentCode::Label(LabelIncident::PanelWorkerError);
        for index in 1..=300_u64 {
            let Observed::Opened(id) = log.observe(IncidentObservation::plain(
                resolved_code,
                IncidentSubject::Asset(AssetId(index)),
                format!("failure {index}"),
                TimelineRevision(1),
            )) else {
                panic!("distinct subjects must open");
            };
            assert_eq!(id, IncidentId(index));
            assert!(log.resolve(id, IncidentOutcome::Applied));
        }
        // Three non-transient opens survive; three transient opens drop: one
        // untyped panel note, one typed shared-code panel note (the
        // `chat_ui.rs:1559` shape) and one row-11 missing-media aggregate.
        let mark_transient = |mut observation: IncidentObservation| {
            observation.transient = true;
            observation
        };
        for observation in [
            IncidentObservation::plain(
                IncidentCode::SourceColor(SourceColorIncident::UnknownRange),
                IncidentSubject::Asset(AssetId(1001)),
                "open one",
                TimelineRevision(2),
            ),
            IncidentObservation::plain(
                IncidentCode::EditRevisionConflict,
                IncidentSubject::Project,
                "open two",
                TimelineRevision(2),
            ),
            IncidentObservation::plain(
                IncidentCode::Rejection(RejectionIncident::EditPlan),
                IncidentSubject::Project,
                "open three",
                TimelineRevision(2),
            ),
            mark_transient(IncidentObservation::plain(
                resolved_code,
                IncidentSubject::Project,
                "the worker failed",
                TimelineRevision(2),
            )),
            mark_transient(IncidentObservation::plain(
                IncidentCode::SourceColor(SourceColorIncident::UnknownRange),
                IncidentSubject::Asset(AssetId(1002)),
                "proof rendering failed",
                TimelineRevision(2),
            )),
            mark_transient(IncidentObservation::plain(
                IncidentCode::Label(LabelIncident::MediaIncomplete),
                IncidentSubject::Project,
                "3 assets missing",
                TimelineRevision(2),
            )),
        ] {
            let Observed::Opened(_) = log.observe(observation) else {
                panic!("distinct opens must open");
            };
        }
        assert_eq!(log.len(), 306);
        let (records, report) = log.records(None, &BTreeMap::new());
        assert_eq!(
            report,
            WriteReport {
                written_open: 3,
                written_resolved: 256,
                dropped_transient_open: 3,
                pruned_resolved: 44,
            }
        );
        let written: BTreeSet<u64> = records.iter().map(|record| record.id.0).collect();
        assert_eq!(written.len(), 259);
        for id in [301_u64, 302, 303] {
            assert!(written.contains(&id), "non-transient opens are written");
        }
        for id in [304_u64, 305, 306] {
            assert!(!written.contains(&id), "transient opens are dropped");
        }
        for id in 1..=44_u64 {
            assert!(!written.contains(&id), "pruned are the 44 lowest ids");
        }
        for id in 45..=300_u64 {
            assert!(written.contains(&id), "the newest 256 resolved are kept");
        }
        assert_eq!(
            report.written_open
                + report.written_resolved
                + report.dropped_transient_open
                + report.pruned_resolved,
            log.len()
        );
    }

    /// `IN2B` §12 item 8 (`IN2B` §3 rule 4): an `Investigating` entry is
    /// written as `Open` with flushed counters.
    #[test]
    fn in2b_an_investigating_entry_is_written_as_open_with_flushed_counters() {
        let mut log = IncidentLog::with_start(Instant::now(), None);
        let Observed::Opened(id) = log.observe(IncidentObservation::plain(
            IncidentCode::EditRevisionConflict,
            IncidentSubject::Project,
            "revision 9",
            TimelineRevision(9),
        )) else {
            panic!("the first observation must open");
        };
        assert!(log.begin_investigation(id));
        let running = RunningInvestigation {
            id,
            harness: "test-harness".to_owned(),
            model: Some("test-model".to_owned()),
            turns: 9,
            input_tokens: Some(100),
            cached_input_tokens: Some(10),
            cache_creation_input_tokens: Some(11),
            output_tokens: Some(50),
            reasoning_output_tokens: Some(5),
            cost_usd_millionths: Some(42),
        };
        let (records, report) = log.records(Some(&running), &BTreeMap::new());
        assert_eq!(report.written_open, 1);
        assert_eq!(records.len(), 1);
        let record = &records[0];
        assert_eq!(record.state, IncidentState::Open);
        let Some(IncidentResolver::Session {
            harness,
            model,
            stop,
        }) = &record.telemetry.resolver
        else {
            panic!("an investigating entry records a session resolver");
        };
        assert_eq!(harness, "test-harness");
        assert_eq!(model, &Some("test-model".to_owned()));
        assert_eq!(stop, "interrupted: Kinewright closed");
        assert_eq!(record.telemetry.turns, Some(9));
        assert_eq!(record.telemetry.input_tokens, Some(100));
        assert_eq!(record.telemetry.cached_input_tokens, Some(10));
        assert_eq!(record.telemetry.cache_creation_input_tokens, Some(11));
        assert_eq!(record.telemetry.output_tokens, Some(50));
        assert_eq!(record.telemetry.reasoning_output_tokens, Some(5));
        assert_eq!(record.telemetry.cost_usd_millionths, Some(42));

        let (records, _) = log.records(None, &BTreeMap::new());
        let Some(IncidentResolver::Session {
            harness,
            model,
            stop,
        }) = &records[0].telemetry.resolver
        else {
            panic!("the defensive arm records a session resolver too");
        };
        assert_eq!(harness, "");
        assert!(model.is_none());
        assert_eq!(stop, "interrupted: Kinewright closed");
    }

    /// `IN2B` §12 item 9 (`IN2B` §2 rule 14): the seventeen stops persist
    /// verbatim; everything else persists as its category.
    #[test]
    fn in2b_persisted_stop_is_verbatim_for_seventeen_and_categorical_beyond() {
        assert_eq!(STOPS.len(), 17);
        for stop in STOPS {
            assert_eq!(persisted_stop(stop), stop);
        }
        assert_eq!(persisted_stop("harness: /tmp/x"), "harness");
        assert_eq!(
            persisted_stop("observer: the watcher disconnected"),
            "observer"
        );
        assert_eq!(
            persisted_stop("connection reset by peer (os error 104)"),
            "session"
        );
    }

    /// `IN2B` §12 item 12 (`IN2B` §3 rule 14): a fixed wall origin derives
    /// millis exactly, and loaded walls survive a new origin.
    #[test]
    fn in2b_a_wall_origin_derives_millis_and_loaded_walls_survive_a_new_origin() {
        let wall = SystemTime::UNIX_EPOCH + Duration::from_millis(1_758_624_000_123);
        let log = log_with_exact_opened_at(Some(wall), Duration::from_millis(9_120));
        let (records, _) = log.records(None, &BTreeMap::new());
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].opened_wall_millis, Some(1_758_624_009_243));

        let origin_less = log_with_exact_opened_at(None, Duration::from_millis(9_120));
        let (records, _) = origin_less.records(None, &BTreeMap::new());
        assert_eq!(records[0].opened_wall_millis, None);

        let pre_epoch = SystemTime::UNIX_EPOCH - Duration::from_secs(60);
        let skewed = log_with_exact_opened_at(Some(pre_epoch), Duration::from_millis(9_120));
        let (records, _) = skewed.records(None, &BTreeMap::new());
        assert_eq!(records[0].opened_wall_millis, None);

        // Gate box: write under O1, restore under O2, write again — every
        // wall byte-identical across the two writes.
        let origin_one = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let mut first_log = IncidentLog::with_start(Instant::now(), Some(origin_one));
        for (asset, millis) in [(0_u64, 1_000_u64), (1, 2_500)] {
            let Observed::Opened(id) = first_log.observe(IncidentObservation::plain(
                IncidentCode::EditRevisionConflict,
                IncidentSubject::Asset(AssetId(asset)),
                format!("open {asset}"),
                TimelineRevision(9),
            )) else {
                panic!("distinct subjects must open");
            };
            first_log
                .entries
                .iter_mut()
                .find(|entry| entry.id == id)
                .unwrap()
                .opened_at = Duration::from_millis(millis);
        }
        let (first_records, _) = first_log.records(None, &BTreeMap::new());
        assert_eq!(first_records.len(), 2);
        assert_eq!(first_records[0].opened_wall_millis, Some(1_000_001_000));
        assert_eq!(first_records[1].opened_wall_millis, Some(1_000_002_500));
        let origin_two = SystemTime::UNIX_EPOCH + Duration::from_secs(2_000_000);
        let mut second_log = IncidentLog::with_start(Instant::now(), Some(origin_two));
        let _ = second_log.restore(
            first_records.clone(),
            Vec::new(),
            TimelineRevision(100),
            None,
        );
        let (second_records, _) = second_log.records(None, &BTreeMap::new());
        assert_eq!(second_records.len(), 2);
        for (first, second) in first_records.iter().zip(second_records.iter()) {
            assert_eq!(first.id, second.id);
            assert_eq!(first.opened_wall_millis, second.opened_wall_millis);
        }
    }

    /// The rule-13 stash round-trips through the builder: a small op is
    /// carried, an over-ceiling op is omitted — and the ceiling is measured by
    /// [`compact_json_len`], which this test holds against `serde_json`
    /// itself.
    ///
    /// Not a §12 item (no item pins the d5 boundary): a unit test proving the
    /// elision the builder applies.
    #[test]
    fn refused_op_stash_is_carried_when_small_and_omitted_over_the_ceiling() {
        #[derive(serde::Serialize)]
        struct NormalFloats {
            fraction: f64,
            whole: f64,
        }
        #[derive(serde::Serialize)]
        struct ExtremeFloats {
            huge: f64,
            tiny: f64,
        }
        let small = Operation::AddTrack {
            track: Track {
                id: TrackId(42),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: Vec::new(),
            },
        };
        let large = Operation::SetTrackAutomation {
            track: TrackId(41),
            parameter: "x".repeat(5_000),
            curve: None,
        };
        // The counter is exact where floats are absent.
        for operation in [&small, &large] {
            assert_eq!(
                compact_json_len(operation),
                serde_json::to_string(operation).unwrap().len()
            );
        }
        assert!(serde_json::to_string(&large).unwrap().len() > REFUSED_STASH_CEILING_BYTES);
        // Floats count an upper bound: exact on ordinary magnitudes, never
        // short on extreme ones.
        let normal = NormalFloats {
            fraction: 0.1,
            whole: 42.0,
        };
        assert_eq!(
            compact_json_len(&normal),
            serde_json::to_string(&normal).unwrap().len()
        );
        let extreme = ExtremeFloats {
            huge: 1e300,
            tiny: 1e-300,
        };
        assert!(compact_json_len(&extreme) >= serde_json::to_string(&extreme).unwrap().len());
        // Both bool spellings count exactly ("true" is 4 bytes, "false" 5);
        // the Kani spike found them swapped, hidden from the cases above by
        // `skip_serializing_if` on every bool those operations carry.
        for flag in [true, false] {
            assert_eq!(
                compact_json_len(&flag),
                serde_json::to_string(&flag).unwrap().len()
            );
        }

        let mut log = IncidentLog::with_start(Instant::now(), None);
        for (index, observed) in [(1_u64, "first"), (2, "second")] {
            let Observed::Opened(opened) = log.observe(IncidentObservation::plain(
                IncidentCode::EditRevisionConflict,
                IncidentSubject::Asset(AssetId(index)),
                observed,
                TimelineRevision(1),
            )) else {
                panic!("distinct subjects must open");
            };
            assert_eq!(opened, IncidentId(index));
        }
        let refused: BTreeMap<IncidentId, Operation> =
            [(IncidentId(1), small.clone()), (IncidentId(2), large)]
                .into_iter()
                .collect();
        let (records, _) = log.records(None, &refused);
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].refused_op, Some(small));
        assert_eq!(records[1].refused_op, None);
    }

    /// N4 F5: dedup absorbs durability in either order — a durable observation
    /// deduping into a transient open (or a transient one into a durable
    /// incident) leaves a durable incident, so persistence never depends on
    /// arrival order.
    #[test]
    fn dedup_absorbs_durability_in_either_order() {
        for (first_transient, second_transient) in
            [(true, false), (false, true), (true, true), (false, false)]
        {
            let mut log = IncidentLog::with_start(Instant::now(), None);
            let code = IncidentCode::EditRevisionConflict;
            let mut first = IncidentObservation::plain(
                code,
                IncidentSubject::Project,
                "revision 9",
                TimelineRevision(9),
            );
            first.transient = first_transient;
            let Observed::Opened(id) = log.observe(first) else {
                panic!("the first observation must open");
            };
            let mut second = IncidentObservation::plain(
                code,
                IncidentSubject::Project,
                "revision 9",
                TimelineRevision(9),
            );
            second.transient = second_transient;
            assert!(matches!(log.observe(second), Observed::Deduped(deduped) if deduped == id));
            assert_eq!(
                log.get(id).unwrap().transient,
                first_transient && second_transient
            );
        }
        // The reported defect: transient-then-durable persists at the next write.
        let mut log = IncidentLog::with_start(Instant::now(), None);
        let code = IncidentCode::EditRevisionConflict;
        let mut transient = IncidentObservation::plain(
            code,
            IncidentSubject::Project,
            "revision 9",
            TimelineRevision(9),
        );
        transient.transient = true;
        let Observed::Opened(id) = log.observe(transient) else {
            panic!("the first observation must open");
        };
        let durable = IncidentObservation::plain(
            code,
            IncidentSubject::Project,
            "revision 9",
            TimelineRevision(9),
        );
        assert!(matches!(log.observe(durable), Observed::Deduped(_)));
        let (records, report) = log.records(None, &BTreeMap::new());
        assert_eq!(report.written_open, 1);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].id, id);
    }

    /// N4 F6: restore applies observe's serialised-length caps — a hand-written
    /// over-cap `subject_name` or proposal string restores truncated, as live
    /// notes do. (`observed` has no cap at observe, so it restores verbatim.)
    #[test]
    fn restore_applies_observe_length_caps() {
        let mut stored = stored_record(1, IncidentCode::EditRevisionConflict.code());
        stored.subject_name = Some("\u{1}".repeat(5000));
        stored.observed = "o".repeat(5000);
        stored.proposal = Some(IncidentProposal {
            operations: Vec::new(),
            operation_count: 1,
            summary: "s".repeat(500),
            explanation: "\u{1}".repeat(500),
            base_revision: TimelineRevision(5),
            stale: false,
        });
        let mut log = IncidentLog::with_start(Instant::now(), None);
        let _ = log.restore(
            parsed_records(vec![stored]),
            Vec::new(),
            TimelineRevision(100),
            None,
        );
        let loaded = log.get(IncidentId(1)).unwrap();
        let name = loaded.subject_name.as_ref().unwrap();
        assert!(json_escaped_len(name) <= SUBJECT_NAME_CEILING_BYTES);
        assert!(!name.is_empty());
        assert_eq!(loaded.observed, "o".repeat(5000));
        let proposal = loaded.proposal.as_ref().unwrap();
        assert!(json_escaped_len(&proposal.summary) <= INVESTIGATOR_EXPLANATION_CEILING_BYTES);
        assert!(json_escaped_len(&proposal.explanation) <= INVESTIGATOR_EXPLANATION_CEILING_BYTES);
        assert!(!proposal.summary.is_empty() && !proposal.explanation.is_empty());
    }

    /// N4 F2: `next_id` resumes above EVERY id in the file — restored,
    /// unknown-code, and the `id_floor` the app computed from the raw carried
    /// values core cannot parse.
    #[test]
    fn restore_resumes_above_every_id_in_the_file() {
        // Unknown-code ids count toward the resume (the aggregate consumes the
        // resumed id, so the first fresh open lands one above it).
        let known = stored_record(7, IncidentCode::EditRevisionConflict.code());
        let unknown = stored_record(500, "code_from_the_future");
        let mut log = IncidentLog::with_start(Instant::now(), None);
        let report = log.restore(
            parsed_records(vec![known, unknown]),
            Vec::new(),
            TimelineRevision(1),
            None,
        );
        assert_eq!(report.restored_open, 1);
        assert_eq!(
            report.unknown_codes,
            vec![("code_from_the_future".to_owned(), 1)]
        );
        let Observed::Opened(fresh) = log.observe(IncidentObservation::plain(
            IncidentCode::EditRevisionConflict,
            IncidentSubject::Asset(AssetId(77)),
            "fresh",
            TimelineRevision(1),
        )) else {
            panic!("a fresh observation must open");
        };
        assert_eq!(fresh, IncidentId(502));

        // The app's floor covers the raw carried values.
        let known = stored_record(7, IncidentCode::EditRevisionConflict.code());
        let mut log = IncidentLog::with_start(Instant::now(), None);
        let report = log.restore(
            parsed_records(vec![known]),
            vec![r#"{"id":902}"#.to_owned()],
            TimelineRevision(1),
            Some(902),
        );
        assert_eq!(report.carried, vec![r#"{"id":902}"#.to_owned()]);
        let Observed::Opened(fresh) = log.observe(IncidentObservation::plain(
            IncidentCode::EditRevisionConflict,
            IncidentSubject::Asset(AssetId(77)),
            "fresh",
            TimelineRevision(1),
        )) else {
            panic!("a fresh observation must open");
        };
        assert_eq!(fresh, IncidentId(903));
    }

    /// N4 F4, N4.1: unusable ids keep the first record and count the rest — a
    /// duplicate id, and an id past `MAX_RESTORED_ID` (`u64::MAX` here) — so
    /// `next_id` never saturates onto a live id.
    #[test]
    fn restore_skips_invalid_ids_and_counts_them() {
        let first = stored_record(1, IncidentCode::EditRevisionConflict.code());
        let mut second = stored_record(
            1,
            IncidentCode::Label(LabelIncident::PanelWorkerError).code(),
        );
        second.observed = "other".to_owned();
        let mut maxed = stored_record(1, IncidentCode::EditRevisionConflict.code());
        maxed.id = IncidentId(u64::MAX);
        let third = stored_record(2, IncidentCode::EditRevisionConflict.code());
        let mut log = IncidentLog::with_start(Instant::now(), None);
        let report = log.restore(
            parsed_records(vec![first, second, maxed, third]),
            Vec::new(),
            TimelineRevision(1),
            None,
        );
        assert_eq!(report.restored_open, 2);
        assert_eq!(report.invalid_ids, 2);
        let loaded = log.get(IncidentId(1)).unwrap();
        assert_eq!(loaded.code, IncidentCode::EditRevisionConflict);
        assert_eq!(loaded.observed, "observed 1");
        let Observed::Opened(fresh) = log.observe(IncidentObservation::plain(
            IncidentCode::EditRevisionConflict,
            IncidentSubject::Asset(AssetId(77)),
            "fresh",
            TimelineRevision(1),
        )) else {
            panic!("a fresh observation must open");
        };
        assert_eq!(fresh, IncidentId(3));
    }

    /// N4 F10 (`IN2B` §3 rule 5): a record-`Investigating` (hand-written only —
    /// the writer never emits it) loads as `Open` with the interrupted stop,
    /// keeping a hand-written harness identity when the record carries one.
    #[test]
    fn restored_investigating_record_becomes_open_with_the_interrupted_stop() {
        let mut with_session = stored_record(1, IncidentCode::EditRevisionConflict.code());
        with_session.state = IncidentState::Investigating;
        with_session.telemetry.resolver = Some(IncidentResolver::Session {
            harness: "hand".to_owned(),
            model: Some("model".to_owned()),
            stop: "hand-written".to_owned(),
        });
        let mut without_session = stored_record(2, IncidentCode::EditRevisionConflict.code());
        without_session.state = IncidentState::Investigating;
        let mut log = IncidentLog::with_start(Instant::now(), None);
        let report = log.restore(
            parsed_records(vec![with_session, without_session]),
            Vec::new(),
            TimelineRevision(1),
            None,
        );
        assert_eq!(report.restored_open, 2);
        let loaded = log.get(IncidentId(1)).unwrap();
        assert_eq!(loaded.state, IncidentState::Open);
        let Some(IncidentResolver::Session {
            harness,
            model,
            stop,
        }) = &loaded.telemetry.resolver
        else {
            panic!("a restored investigating record carries a session resolver");
        };
        assert_eq!(harness, "hand");
        assert_eq!(model, &Some("model".to_owned()));
        assert_eq!(stop, "interrupted: Kinewright closed");
        let loaded = log.get(IncidentId(2)).unwrap();
        assert_eq!(loaded.state, IncidentState::Open);
        let Some(IncidentResolver::Session {
            harness,
            model,
            stop,
        }) = &loaded.telemetry.resolver
        else {
            panic!("a resolver-less investigating record takes the defensive arm");
        };
        assert_eq!(harness, "");
        assert!(model.is_none());
        assert_eq!(stop, "interrupted: Kinewright closed");
    }

    /// N4 F7: restore into a non-empty log panics in every build.
    #[test]
    #[should_panic(expected = "restore runs once")]
    fn restore_into_a_non_empty_log_panics() {
        let mut log = IncidentLog::with_start(Instant::now(), None);
        let Observed::Opened(_) = log.observe(IncidentObservation::plain(
            IncidentCode::EditRevisionConflict,
            IncidentSubject::Project,
            "revision 9",
            TimelineRevision(9),
        )) else {
            panic!("the first observation must open");
        };
        let _ = log.restore(Vec::new(), Vec::new(), TimelineRevision(9), None);
    }

    /// N4 F8: [`compact_json_len`] is exact for [`Operation`] because it
    /// carries no floats — `f32`/`f64` are not `Eq`, so this fails to compile
    /// the day a float field is added.
    #[test]
    fn operation_has_no_float_fields() {
        fn require_eq<T: Eq>() {}
        require_eq::<Operation>();
    }

    /// N4.1: the restorable id ceiling — records past `MAX_RESTORED_ID` are
    /// skipped and counted, and an `id_floor` past it is clamped to it, so
    /// `next_id` never overflows onto a live id.
    #[test]
    fn restore_enforces_the_id_ceiling() {
        // A floor of `u64::MAX` clamps to the ceiling: the fresh id lands
        // above every file id instead of colliding with restored id 1.
        let known = stored_record(1, IncidentCode::EditRevisionConflict.code());
        let mut log = IncidentLog::with_start(Instant::now(), None);
        let _ = log.restore(
            parsed_records(vec![known]),
            Vec::new(),
            TimelineRevision(1),
            Some(u64::MAX),
        );
        let Observed::Opened(fresh) = log.observe(IncidentObservation::plain(
            IncidentCode::EditRevisionConflict,
            IncidentSubject::Asset(AssetId(77)),
            "fresh",
            TimelineRevision(1),
        )) else {
            panic!("a fresh observation must open");
        };
        assert_eq!(fresh, IncidentId(MAX_RESTORED_ID + 1));

        // `u64::MAX - 1` refuses: known-code to `invalid_ids`, unknown-code to
        // the aggregate — neither restores, neither counts toward the resume.
        let mut refused = stored_record(1, IncidentCode::EditRevisionConflict.code());
        refused.id = IncidentId(u64::MAX - 1);
        let mut strange = stored_record(2, "code_from_the_future");
        strange.id = IncidentId(u64::MAX - 1);
        let mut log = IncidentLog::with_start(Instant::now(), None);
        let report = log.restore(
            parsed_records(vec![refused, strange]),
            Vec::new(),
            TimelineRevision(1),
            None,
        );
        assert_eq!(report.restored_open, 0);
        assert_eq!(report.invalid_ids, 1);
        assert_eq!(
            report.unknown_codes,
            vec![("code_from_the_future".to_owned(), 1)]
        );
        assert!(log.get(IncidentId(u64::MAX - 1)).is_none());
        let Observed::Opened(fresh) = log.observe(IncidentObservation::plain(
            IncidentCode::EditRevisionConflict,
            IncidentSubject::Asset(AssetId(77)),
            "fresh",
            TimelineRevision(1),
        )) else {
            panic!("a fresh observation must open");
        };
        // The resume is untouched by the refused ids: the aggregate took 1.
        assert_eq!(fresh, IncidentId(2));
    }
}

/// Kani proof harnesses: compiled only under `cargo kani`, which passes
/// `--cfg kani`. Never compiled into normal builds.
///
/// Admission rule (adopt-narrowly verdict, spike report
/// `target/review/kani/spike-report.md`): a proof is admitted only if it
/// completes in < 5 min and < 6 GB. Symbolic strings (UTF-8 validation
/// dominates past any bound) and map containers (`BTreeMap` internals OOM
/// even at 1 entry) are out of scope — the string/byte-scan targets were
/// tried and dropped as infeasible. One `kani::assert` per harness
/// wherever a failure must not mask another check: a failing assert aborts
/// the path, so the concrete anchor lives in its own harness rather than
/// ahead of the symbolic one.
#[cfg(kani)]
mod kani_proofs {
    /// Small no-float shape exercising the counter's struct, sequence,
    /// option, map, integer, boolean, string and unit/newtype/struct-variant
    /// enum arms — the value shapes `Operation` uses.
    #[derive(serde::Serialize)]
    struct KaniShape<'a> {
        id: u32,
        flag: bool,
        maybe: Option<u8>,
        pair: [u16; 2],
        name: &'a str,
        meta: KaniMap,
        kind: KaniKind,
    }

    /// One-entry `{"k": value}` map with a manual `Serialize` impl: it
    /// drives exactly the counter's `serialize_map` / `serialize_entry`
    /// arms, without `BTreeMap`'s prover-hostile navigate/dealloc
    /// machinery (a 1-entry `BTreeMap` field OOMs past 6 GB — spike
    /// report §4). What is under test is the counter's accounting, which
    /// sees only `Serializer` calls, never the driver's container.
    struct KaniMap(u32);

    impl serde::Serialize for KaniMap {
        fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            use serde::ser::SerializeMap;
            let mut map = serializer.serialize_map(Some(1))?;
            map.serialize_entry("k", &self.0)?;
            map.end()
        }
    }

    #[derive(serde::Serialize)]
    enum KaniKind {
        Unit,
        Newtype(u32),
        Pair { x: u8, y: bool },
    }

    /// Decimal width of `value` (≤ `u32::MAX`, so ≤ 10 digits).
    fn kani_digits(mut value: u32) -> usize {
        let mut len = 1;
        while value >= 10 {
            value /= 10;
            len += 1;
        }
        len
    }

    /// Independent copy of the `serde_json` escape table (see
    /// `json_escaped_byte_len`), exercised here only on the empty string:
    /// the escape table gets no symbolic coverage — the existing
    /// `serde_json` differential unit tests anchor the table itself on real
    /// data.
    fn kani_escape_len(bytes: &[u8]) -> usize {
        let mut len = 0;
        for &byte in bytes {
            len += match byte {
                b'"' | b'\\' | 0x08 | 0x09 | 0x0a | 0x0c | 0x0d => 2,
                0x00..=0x1f => 6,
                _ => 1,
            };
        }
        len
    }

    /// N4.1: the resume id lands above every restorable input id and above
    /// the clamped floor, and the next `observe` id stays far below
    /// `u64::MAX`. The resume id itself is the checked one — the rule-6
    /// aggregate takes it and is pushed into `entries`, so it is a live id
    /// the moment one is noted — and the floor half is checked explicitly,
    /// since dropping it leaves a `fresh > id`-only harness green.
    #[kani::proof]
    #[kani::unwind(8)]
    fn kani_restore_id_resume() {
        const N: usize = 4;
        let count: usize = kani::any();
        kani::assume(count <= N);
        let ids: [(u64, bool); N] = kani::any();
        let id_floor: Option<u64> = kani::any();
        let resume = super::restore_id_resume(&ids[..count], id_floor);
        // `restore` requires an empty log, whose `next_id` is 1 — and only
        // `observe`'s open arm moves `next_id`, always alongside an
        // `entries.push`, so empty means 1 exactly.
        let resumed = resume.next_id.unwrap_or(1);
        let fresh = resumed + u64::from(resume.unknown_skipped > 0);
        // One assert over the whole conjunction: the resume clears every
        // restorable id and the clamped floor, and the next observe id
        // keeps headroom below `u64::MAX` (`observe`'s `saturating_add`
        // never sticks; Kani also checks the `+ 1` above for overflow).
        let mut ok = fresh <= super::MAX_RESTORED_ID + 2;
        for i in 0..count {
            ok &= ids[i].0 > super::MAX_RESTORED_ID || resumed > ids[i].0;
        }
        if let Some(floor) = id_floor {
            ok &= resumed > floor.min(super::MAX_RESTORED_ID);
        }
        kani::assert(
            ok,
            "resume clears every restorable id and the clamped floor; next observe keeps headroom",
        );
    }

    /// BR2: [`super::compact_json_len`] is exact for no-float shapes. This
    /// anchor pins the hand-written oracle formula itself against a
    /// hand-counted value; it caught the swapped `serialize_bool` widths
    /// the spike found (spike report §4, fixed on main).
    #[kani::proof]
    fn kani_compact_json_len_anchor() {
        // `{"id":0,"flag":false,"maybe":null,"pair":[0,0],"name":"",
        // "meta":{"k":0},"kind":"Unit"}` is 86 bytes by hand count.
        let anchor = KaniShape {
            id: 0,
            flag: false,
            maybe: None,
            pair: [0, 0],
            name: "",
            meta: KaniMap(0),
            kind: KaniKind::Unit,
        };
        kani::assert(
            super::compact_json_len(&anchor) == 86,
            "oracle formula matches the hand-computed anchor",
        );
    }

    /// The symbolic half of the BR2 exactness check: the counter's
    /// structural accounting, integer widths and enum shapes over all small
    /// values (the string field is concrete-empty — a symbolic `&str`
    /// OOMs; see below). The escape table gets no symbolic coverage here —
    /// the `serde_json` differential unit tests remain its anchor.
    #[kani::proof]
    #[kani::unwind(16)]
    fn kani_compact_json_len_symbolic() {
        let id: u32 = kani::any();
        let flag: bool = kani::any();
        let maybe_some: bool = kani::any();
        let maybe_v: u8 = kani::any();
        let maybe = maybe_some.then_some(maybe_v);
        let pair: [u16; 2] = kani::any();
        // Concrete empty string: a symbolic `&str` (even ≤ 4 bytes) OOMs
        // past 6 GB — std UTF-8 validation dominates, the same wall target
        // 2 hit (spike report §4). Escape accounting therefore stays with
        // the `serde_json` differential unit tests; this harness proves the
        // structural accounting, integer widths and enum shapes.
        let nslice: &[u8] = b"";
        let name = "";
        let v: u32 = kani::any();
        let meta = KaniMap(v);
        let which: u8 = kani::any();
        kani::assume(which < 3);
        let n: u32 = kani::any();
        let x: u8 = kani::any();
        let y: bool = kani::any();
        let kind = match which {
            0 => KaniKind::Unit,
            1 => KaniKind::Newtype(n),
            _ => KaniKind::Pair { x, y },
        };
        let shape = KaniShape {
            id,
            flag,
            maybe,
            pair,
            name,
            meta,
            kind,
        };

        let flag_len = if flag { 4 } else { 5 };
        let maybe_len = maybe.map_or(4, |m| kani_digits(u32::from(m)));
        let pair_len = 3 + kani_digits(u32::from(pair[0])) + kani_digits(u32::from(pair[1]));
        let name_len = 2 + kani_escape_len(nslice);
        let meta_len = 6 + kani_digits(v);
        let kind_len = match &shape.kind {
            KaniKind::Unit => 6,
            KaniKind::Newtype(n) => 12 + kani_digits(*n),
            KaniKind::Pair { x, y } => 20 + kani_digits(u32::from(*x)) + if *y { 4 } else { 5 },
        };
        // `{`, one `"key":value` per field, commas, `}`.
        let expected = 1
            + (5 + kani_digits(id))
            + 1
            + (7 + flag_len)
            + 1
            + (8 + maybe_len)
            + 1
            + (7 + pair_len)
            + 1
            + (7 + name_len)
            + 1
            + (7 + meta_len)
            + 1
            + (7 + kind_len)
            + 1;
        kani::assert(
            super::compact_json_len(&shape) == expected,
            "compact_json_len is exact on small no-float shapes",
        );
    }
}
