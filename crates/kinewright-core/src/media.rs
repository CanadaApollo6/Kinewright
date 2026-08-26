use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use crossbeam_channel::{Receiver, Sender};
use thiserror::Error;

use crate::{
    AssetId, ClipId, ColorDescription, DeliveryVerification, DeliveryVerificationRequest, Document,
    EffectId, LutAsset, LutAssetId, MediaAsset, MediaSourceFingerprint, NormalizedRoi, Rational,
    SCOPE_BASIS_POINTS, TimeCode, TrackId,
};

/// The runtime truth about whether an imported source can currently be read.
///
/// This is deliberately not persisted in [`MediaAsset`]: availability depends
/// on the current machine and can change while a project is open. A verified
/// source has the same SHA-256 and byte length as its persisted fingerprint;
/// an unverified source is readable but belongs to a legacy asset without a
/// fingerprint. `Changed` means the path is readable but no longer matches the
/// source identity. `Unreadable` preserves the backend reason for failures that
/// are more specific than a missing path.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum MediaAvailabilityKind {
    OnlineVerified,
    OnlineUnverified,
    OfflineMissing,
    Changed,
    Unreadable,
}

/// A typed, machine-readable media availability observation.
#[derive(
    Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
pub struct MediaAvailabilityStatus {
    pub kind: MediaAvailabilityKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(default)]
    pub observed_fingerprint: Option<MediaSourceFingerprint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(default)]
    pub reason: Option<String>,
}

/// One live source-availability failure that blocks export.
///
/// This keeps the availability observation out of persisted project state:
/// source status belongs to the machine that is about to render, while the
/// imported fingerprint remains part of the project document.
#[derive(
    Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
pub struct ExportMediaPreflightIssue {
    pub asset: AssetId,
    pub asset_name: String,
    pub availability: MediaAvailabilityStatus,
}

/// Live source-identity result for the timeline-referenced portion of one
/// immutable export document.
///
/// `OnlineUnverified` deliberately blocks export. It identifies a legacy
/// source that has no persisted fingerprint, so a readable path alone cannot
/// prove it is the media that was edited. Re-import or explicitly relink the
/// source to record a verified identity before delivery. This policy applies
/// equally to video and audio assets and is intentionally stricter than
/// ordinary preview availability.
#[derive(
    Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
pub struct ExportMediaPreflightReport {
    /// Asset ids checked in deterministic media-pool order. Unused media-bin
    /// entries are intentionally absent.
    pub checked_assets: Vec<AssetId>,
    /// Every status other than `online_verified`, retained with its typed
    /// backend reason for UI and agent recovery surfaces.
    pub issues: Vec<ExportMediaPreflightIssue>,
}

impl ExportMediaPreflightReport {
    /// Return whether every timeline-referenced source was live and matched
    /// its persisted fingerprint at preflight time.
    #[must_use]
    pub const fn export_ready(&self) -> bool {
        self.issues.is_empty()
    }

    /// Produce a concise human-readable summary while preserving the full
    /// typed report for callers that need individual recovery actions.
    #[must_use]
    pub fn summary(&self) -> String {
        if self.issues.is_empty() {
            return format!(
                "{} timeline-referenced source(s) are online and fingerprint-verified",
                self.checked_assets.len()
            );
        }
        let details = self
            .issues
            .iter()
            .map(|issue| {
                let reason = issue
                    .availability
                    .reason
                    .as_deref()
                    .unwrap_or("no backend reason was provided");
                format!(
                    "{} ({:?}): {reason}",
                    issue.asset_name, issue.availability.kind
                )
            })
            .collect::<Vec<_>>()
            .join("; ");
        format!(
            "Export blocked: {} timeline-referenced source(s) need relink or recovery: {details}",
            self.issues.len()
        )
    }
}

/// The runtime truth about whether a project-owned LUT asset can currently be
/// read from the store (CC4 §2.3).
///
/// Availability is runtime state, never project state: it depends on the
/// current machine and is never serialized into a document. Core has no
/// filesystem or project-directory concept, so it never computes these values
/// — the media layer observes them against the derived store root and passes
/// them in.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum LutAvailabilityKind {
    /// The store file exists, is a regular file, and hashes to `sha256`.
    Verified,
    /// The store file is absent or is not a regular file.
    Missing,
    /// A file exists but its bytes hash to something else.
    Changed,
    /// The path exists but bytes or metadata cannot be read.
    Unreadable,
}

/// A typed, machine-readable LUT availability observation (CC4 §2.3).
///
/// M41's `online_unverified` has no CC4 equivalent: a LUT asset can only be
/// created with a hash, so there is no legacy unverified state.
#[derive(
    Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
pub struct LutAvailabilityStatus {
    pub kind: LutAvailabilityKind,
    /// The hash actually observed, present when a file was read and hashed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(default)]
    pub observed_sha256: Option<String>,
    /// The backend reason, preserved verbatim for recovery surfaces.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(default)]
    pub reason: Option<String>,
    /// The expected store path, so a human can be told where to look.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(default)]
    pub path: Option<PathBuf>,
}

/// One live LUT-availability failure that blocks proof and export (CC4 §2.3).
#[derive(
    Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
pub struct ExportLutPreflightIssue {
    pub lut_asset: LutAssetId,
    pub title: String,
    /// The hash the project records, which is the identity being looked for.
    pub sha256: String,
    pub kind: LutAvailabilityKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(default)]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(default)]
    pub path: Option<PathBuf>,
    /// Every node that could evaluate this asset on some frame, in document
    /// order. An asset referenced only by nodes that can never evaluate does
    /// not appear in the report at all.
    pub referenced_by: Vec<(ClipId, EffectId)>,
}

/// Live LUT-identity result over the assets referenced by possibly-active
/// nodes in one immutable export document (CC4 §2.3).
///
/// The mirror of [`ExportMediaPreflightReport`] for looks. A `missing`,
/// `changed`, or `unreadable` asset referenced by a node that could evaluate
/// blocks managed proof and export; an asset referenced only by bypassed or
/// `mix = 0` nodes does not block, because those nodes are never evaluated.
#[derive(
    Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
pub struct ExportLutPreflightReport {
    /// Asset ids checked, in `Document::lut_assets` order. Assets no
    /// possibly-active node references are intentionally absent.
    pub checked_lut_assets: Vec<LutAssetId>,
    /// Every status other than `verified`, retained with its typed backend
    /// reason for UI and agent recovery surfaces.
    pub issues: Vec<ExportLutPreflightIssue>,
}

impl ExportLutPreflightReport {
    /// Return whether every look a frame could need hashed to its recorded
    /// identity at preflight time.
    #[must_use]
    pub const fn export_ready(&self) -> bool {
        self.issues.is_empty()
    }

    /// Produce a concise human-readable summary while preserving the full
    /// typed report for callers that need individual recovery actions.
    #[must_use]
    pub fn summary(&self) -> String {
        if self.issues.is_empty() {
            return format!(
                "{} referenced LUT asset(s) are present and hash-verified",
                self.checked_lut_assets.len()
            );
        }
        let details = self
            .issues
            .iter()
            .map(|issue| {
                let reason = issue
                    .reason
                    .as_deref()
                    .unwrap_or("no backend reason was provided");
                format!("{} ({:?}): {reason}", issue.title, issue.kind)
            })
            .collect::<Vec<_>>()
            .join("; ");
        format!(
            "Export blocked: {} LUT asset(s) need restore or replacement: {details}",
            self.issues.len()
        )
    }
}

/// Recheck every LUT asset a frame could need immediately before a proof or an
/// export is accepted (CC4 §2.3).
///
/// The observation comes from the caller's store-backed probe rather than the
/// persisted document, for the same reason M41's media preflight does: the
/// bytes are machine-local and can change while a project is open. Core
/// supplies only the document-side half — which assets are referenced by nodes
/// that could evaluate — because it has no filesystem concept.
///
/// Assets referenced only by nodes that can never evaluate (bypassed on every
/// frame, `mix = 0` on every frame, or unbound) are skipped entirely, so a
/// look an operator has switched off cannot block a delivery.
#[must_use]
pub fn export_lut_preflight_with(
    document: &Document,
    availability_for: &dyn Fn(&LutAsset) -> LutAvailabilityStatus,
) -> ExportLutPreflightReport {
    let mut checked_lut_assets = Vec::new();
    let mut issues = Vec::new();
    for asset in &document.lut_assets {
        let referenced_by = possibly_active_lut_references(document, asset.id);
        if referenced_by.is_empty() {
            continue;
        }
        checked_lut_assets.push(asset.id);
        let availability = availability_for(asset);
        if availability.kind == LutAvailabilityKind::Verified {
            continue;
        }
        issues.push(ExportLutPreflightIssue {
            lut_asset: asset.id,
            title: asset.title.clone(),
            sha256: asset.sha256.clone(),
            kind: availability.kind,
            reason: availability.reason,
            path: availability.path,
            referenced_by,
        });
    }
    ExportLutPreflightReport {
        checked_lut_assets,
        issues,
    }
}

/// Every node that references one asset and could evaluate on some frame.
fn possibly_active_lut_references(document: &Document, id: LutAssetId) -> Vec<(ClipId, EffectId)> {
    let referencing = document.lut_asset_references(id);
    document
        .tracks
        .iter()
        .flat_map(|track| &track.clips)
        .flat_map(|clip| clip.effects.iter().map(move |effect| (clip.id, effect)))
        .filter(|(clip, effect)| {
            referencing.contains(&(*clip, effect.id)) && crate::lut_node_may_be_active(effect)
        })
        .map(|(clip, effect)| (clip, effect.id))
        .collect()
}

/// Fixed cache families exposed by the media runtime. The generated-proxy
/// family is present in the contract so clients can distinguish an intentional
/// unsupported feature from an empty cache; M41 does not generate proxies.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum MediaCacheFamily {
    PreviewMemory,
    VisualAssets,
    DerivedAnalysis,
    Transcripts,
    GeneratedProxy,
}

/// Inventory for one owned or explicitly unsupported cache family.
#[derive(
    Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
pub struct MediaCacheFamilyStatus {
    pub family: MediaCacheFamily,
    pub supported: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(default)]
    pub root: Option<PathBuf>,
    pub file_count: u64,
    pub bytes: u64,
    /// The family can be repopulated by an active worker or normal preview
    /// activity after a clear. This avoids promising permanence to callers.
    pub may_repopulate: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(default)]
    pub note: Option<String>,
}

/// Snapshot of the fixed media-cache families.
#[derive(
    Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
pub struct MediaCacheInventory {
    pub families: Vec<MediaCacheFamilyStatus>,
}

/// Result of a scoped cache clear. Clearing is idempotent: absent roots and
/// already-removed files report zero removed files and bytes.
#[derive(
    Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
pub struct MediaCacheClearResult {
    pub family: MediaCacheFamily,
    pub supported: bool,
    pub removed_file_count: u64,
    pub removed_bytes: u64,
    pub may_repopulate: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(default)]
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameTexture {
    pub width: u32,
    pub height: u32,
    pub rgba: Arc<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RgbaImage {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

/// The renderer implementation that produced a managed monitor proof.
///
/// This is intentionally a core-owned vocabulary: proof manifests must not
/// expose a `wgpu` type or make a backend-specific claim that a test double
/// cannot support.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum MonitorProofRenderKind {
    GpuPreview,
    TestDouble,
}

/// Backend provenance attached to one full-resolution managed proof.
#[derive(
    Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
pub struct MonitorProofMetadata {
    pub render_kind: MonitorProofRenderKind,
    /// Stable backend identifier, such as `vulkan` or `dx12`.
    pub backend: String,
    /// Adapter/device name reported by the active renderer.
    pub adapter: String,
    /// True when the renderer is known to be a software fallback.
    pub software_fallback: bool,
    /// True only when the renderer can honestly claim a GPU compositor path.
    pub gpu_claim: bool,
    /// Full-raster proof marker. Managed proof must always set this to true.
    pub full_resolution: bool,
}

impl MonitorProofMetadata {
    /// Metadata for deterministic analysis/test doubles. It never claims GPU
    /// rendering and makes the non-production backend explicit.
    #[must_use]
    pub fn test_double() -> Self {
        Self {
            render_kind: MonitorProofRenderKind::TestDouble,
            backend: "test_double".to_owned(),
            adapter: "test_double".to_owned(),
            software_fallback: true,
            gpu_claim: false,
            full_resolution: true,
        }
    }
}

/// One full-resolution managed monitor proof and its renderer provenance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonitorProof {
    pub image: RgbaImage,
    pub metadata: MonitorProofMetadata,
}

/// Row-major, top-left-origin linear RGBA readback of the working surface.
///
/// `pixels.len() == width * height * 4`, interleaved red, green, blue, alpha.
/// Values are **scene-linear** BT.709/D65 light with no transfer and no
/// clamp: they may be negative and may exceed `1.0`, which is the entire
/// reason CC6 §2.3 names this surface. It is not display-referred, it is not
/// a CPU reference, and a proxy raster may never be substituted for it.
#[derive(Debug, Clone, PartialEq)]
pub struct LinearRgbaImage {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<f32>,
}

/// Always `"working_linear_post_composite"`.
///
/// Equals [`crate::ScopeStage::WorkingLinearPostComposite`]'s wire name: one
/// vocabulary, two consumers (CC6 §2.1).
pub const WORKING_PROOF_STAGE: &str = "working_linear_post_composite";

/// Always `"scene_linear_bt709_f32"`: BT.709 primaries, D65, linear light, no
/// clamp.
pub const WORKING_PROOF_ENCODING: &str = "scene_linear_bt709_f32";

/// Provenance for one full-raster scene-linear working proof.
///
/// This composes [`MonitorProofMetadata`] rather than extending
/// [`MonitorProofRenderKind`], for CC5 §4.1's reason verbatim: that vocabulary
/// names the renderer implementation, not an output target.
#[derive(
    Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
pub struct WorkingProofMetadata {
    /// Renderer provenance, reused unchanged from the managed monitor proof.
    pub render: MonitorProofMetadata,
    /// Always [`WORKING_PROOF_STAGE`].
    pub stage: String,
    /// Always [`WORKING_PROOF_ENCODING`].
    pub encoding: String,
    /// Rendered raster aspect ratio (`width / height`) in millionths.
    pub raster_aspect_millionths: i64,
}

/// One full-raster scene-linear working proof and its renderer provenance.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkingProof {
    pub image: LinearRgbaImage,
    pub metadata: WorkingProofMetadata,
}

/// Stable coverage encoding recorded in every [`MatteProofMetadata`].
///
/// The coverage raster carries `round(255 · clamp(m, 0, 1))` in all three
/// colour channels with an opaque alpha and **no transfer function at all**.
/// It is an integer quantization of a coverage scalar, not a monitoring
/// transform, which is why it is named separately from a monitor proof.
pub const MATTE_COVERAGE_ENCODING: &str = "linear_coverage_u8";

/// Full coverage in [`MATTE_COVERAGE_ENCODING`]: `m = 1` encodes to code 255.
pub const MATTE_COVERAGE_SCALE: u16 = 255;

/// Provenance and resolved matte identity for one full-raster matte proof.
///
/// This composes [`MonitorProofMetadata`] rather than extending
/// [`MonitorProofRenderKind`]: that vocabulary names the renderer
/// implementation (`GpuPreview` / `TestDouble`), not an output target. Adding
/// a `Matte` render kind would make provenance mean two things at once, so the
/// output target is stated by this struct's own fields instead.
#[derive(
    Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
pub struct MatteProofMetadata {
    /// Renderer provenance, reused unchanged from the managed monitor proof.
    pub render: MonitorProofMetadata,
    /// The clip whose colour node was inspected.
    pub clip: ClipId,
    /// The matte-carrying colour node's effect identity.
    pub effect: EffectId,
    /// The colour-node kind that carries the matte, such as `color_wheels`.
    pub node_kind: String,
    /// Always [`MATTE_COVERAGE_ENCODING`].
    pub coverage_encoding: String,
    /// Always [`MATTE_COVERAGE_SCALE`].
    pub coverage_scale: u16,
    /// Rendered raster aspect ratio (`width / height`) in millionths.
    pub raster_aspect_millionths: i64,
    /// The node's resolved `matte_enabled` state.
    pub matte_enabled: bool,
    /// Number of resolved matte windows contributing to the coverage.
    pub window_count: u8,
    /// True when the node's qualifier contributes to the coverage.
    pub qualifier_enabled: bool,
}

/// One full-raster matte coverage proof and its renderer provenance.
///
/// `coverage` is `R = G = B = round(255 · m)` with `A = 255` everywhere; see
/// [`MATTE_COVERAGE_ENCODING`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatteProof {
    pub coverage: RgbaImage,
    pub metadata: MatteProofMetadata,
}

/// Typed failures produced while rendering a matte proof.
///
/// A matte proof never returns a blank frame: a node that contributes nothing
/// fails with a stable code instead, so a caller cannot mistake "no coverage
/// was requested" for "coverage is empty".
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum MatteProofError {
    #[error("matte_proof_node_inactive: colour node is inactive: {reason}")]
    NodeInactive {
        /// Stable inactivity reason token, such as `bypassed` or `matte_excluded`.
        reason: String,
    },
    #[error("matte_proof_no_matte: colour node carries no matte")]
    NoMatte,
    #[error("matte_proof_effect_not_found: clip {clip} carries no effect {effect}")]
    EffectNotFound { clip: ClipId, effect: EffectId },
    /// The clip exists but is not an active visual layer at the proved frame.
    ///
    /// Distinct from [`Self::EffectNotFound`] on purpose: "your node id is
    /// wrong" and "your node is fine but this clip is not on screen at this
    /// timecode" have different recoveries, and collapsing them sent callers
    /// hunting for a node that was never missing (CC5 §4.1).
    #[error("matte_proof_clip_not_visible: clip {clip} is not an active visual layer at {at}")]
    ClipNotVisible {
        clip: ClipId,
        /// The project frame at which the clip was not on screen.
        at: TimeCode,
    },
    #[error(
        "matte_proof_not_a_color_node: effect {effect} on clip {clip} is {name}, which cannot carry a matte"
    )]
    NotAColorNode {
        clip: ClipId,
        effect: EffectId,
        /// The effect name that was found instead of a matte-capable node.
        name: String,
    },
}

impl MatteProofError {
    /// Stable machine-readable status code for agent and UI consumers.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::NodeInactive { .. } => "matte_proof_node_inactive",
            Self::NoMatte => "matte_proof_no_matte",
            Self::EffectNotFound { .. } => "matte_proof_effect_not_found",
            Self::ClipNotVisible { .. } => "matte_proof_clip_not_visible",
            Self::NotAColorNode { .. } => "matte_proof_not_a_color_node",
        }
    }
}

impl From<MatteProofError> for MediaError {
    fn from(error: MatteProofError) -> Self {
        Self::Backend(error.to_string())
    }
}

/// Typed failures produced while measuring a coverage raster.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum MatteCoverageError {
    #[error("matte_coverage_invalid_dimensions: coverage raster is {observed}, allowed {allowed}")]
    InvalidDimensions {
        /// The rejected `WIDTHxHEIGHT` raster.
        observed: String,
        /// The requirement that was violated.
        allowed: &'static str,
    },
    #[error(
        "matte_coverage_buffer_length_mismatch: coverage buffer is {observed} bytes, allowed {allowed} bytes"
    )]
    BufferLengthMismatch { observed: usize, allowed: u64 },
    #[error(
        "matte_coverage_alpha_not_opaque: coverage pixel ({x}, {y}) has alpha {observed}, allowed {allowed}"
    )]
    AlphaNotOpaque {
        x: u32,
        y: u32,
        observed: u8,
        allowed: u8,
    },
    #[error(
        "matte_coverage_not_grey: coverage pixel ({x}, {y}) is ({red}, {green}, {blue}), allowed {allowed}"
    )]
    NotGrey {
        x: u32,
        y: u32,
        red: u8,
        green: u8,
        blue: u8,
        /// The requirement that was violated.
        allowed: &'static str,
    },
    #[error("matte_coverage_overflow: coverage statistic {operation} overflowed")]
    ArithmeticOverflow { operation: &'static str },
}

impl MatteCoverageError {
    /// Stable machine-readable status code for agent and UI consumers.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::InvalidDimensions { .. } => "matte_coverage_invalid_dimensions",
            Self::BufferLengthMismatch { .. } => "matte_coverage_buffer_length_mismatch",
            Self::AlphaNotOpaque { .. } => "matte_coverage_alpha_not_opaque",
            Self::NotGrey { .. } => "matte_coverage_not_grey",
            Self::ArithmeticOverflow { .. } => "matte_coverage_overflow",
        }
    }
}

impl From<MatteCoverageError> for MediaError {
    fn from(error: MatteCoverageError) -> Self {
        Self::Backend(error.to_string())
    }
}

/// Number of buckets in a [`MatteCoverageStatistics`] coverage histogram.
pub const MATTE_COVERAGE_HISTOGRAM_BUCKETS: usize = 16;

/// Deterministic integer statistics of one matte coverage raster.
///
/// Every value is derived from the 8-bit coverage codes alone: no floating
/// point is used, and no project state is consulted.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
pub struct MatteCoverageStatistics {
    /// Pixels with `m > 0`: the set the correction touched at all.
    pub covered_pixel_count: u64,
    /// Pixels at code 255.
    pub full_pixel_count: u64,
    /// Pixels in codes `1..=254`.
    pub partial_pixel_count: u64,
    /// Every pixel in the raster, covered or not.
    pub total_pixel_count: u64,
    /// `floor(covered · 10000 / total)`, the CC2 integer-floor rule.
    pub covered_basis_points: u32,
    /// Counts for `bucket = min(15, floor(code · 16 / 256))`, over **every**
    /// pixel including code 0, so the buckets sum to `total_pixel_count`.
    pub coverage_histogram: [u64; MATTE_COVERAGE_HISTOGRAM_BUCKETS],
    /// The tightest half-open pixel rectangle containing every `m > 0` pixel,
    /// converted to basis points, or `None` when coverage is empty.
    ///
    /// The pixel rectangle `[left, right) × [top, bottom)` converts with the
    /// CC2 ROI rule read in reverse: a start boundary floors and an exclusive
    /// end boundary ceils, so the reported rectangle covers every covered
    /// pixel completely.  Concretely
    /// `x = floor(left · 10000 / width)` and
    /// `width = ceil(right · 10000 / width) − x`, and likewise on the vertical
    /// axis.  Feeding the result back through
    /// [`NormalizedRoi::to_pixels`](crate::NormalizedRoi::to_pixels) therefore
    /// never drops a covered pixel.
    pub bounding_box_basis_points: Option<NormalizedRoi>,
    /// The coverage-weighted centroid `Σ m·p / Σ m` in basis points, with `p`
    /// the pixel centre `((x + 0.5) / width, (y + 0.5) / height)`, rounded
    /// half away from zero.  `None` when coverage is empty.
    ///
    /// This is a statistic *of the matte*, not a colour measurement, so
    /// weighting by partial coverage is correct here and does not contradict
    /// CC2's "partial alpha is not a weight" rule, which governs scope inputs.
    pub centroid_basis_points: Option<(i64, i64)>,
    /// Always true: the centroid above is coverage-weighted.
    pub weighted_by_coverage: bool,
}

/// Measure a matte coverage raster.
///
/// The raster must be exactly what a matte proof produces: `R = G = B` in
/// every pixel and `A = 255` everywhere.  Both requirements are checked rather
/// than assumed, because a caller can hand this function any RGBA image and a
/// silently mis-encoded coverage would produce plausible but wrong statistics.
///
/// # Errors
///
/// Returns [`MatteCoverageError`] for zero dimensions, a pixel buffer whose
/// length contradicts the dimensions, a non-opaque or non-grey pixel, or
/// statistic arithmetic that cannot be represented.
pub fn matte_coverage_statistics(
    coverage: &RgbaImage,
) -> Result<MatteCoverageStatistics, MatteCoverageError> {
    if coverage.width == 0 || coverage.height == 0 {
        return Err(MatteCoverageError::InvalidDimensions {
            observed: format!("{}x{}", coverage.width, coverage.height),
            allowed: "width and height must both be non-zero",
        });
    }
    let total_pixel_count = u64::from(coverage.width)
        .checked_mul(u64::from(coverage.height))
        .ok_or(MatteCoverageError::ArithmeticOverflow {
            operation: "total_pixel_count",
        })?;
    let allowed_bytes =
        total_pixel_count
            .checked_mul(4)
            .ok_or(MatteCoverageError::ArithmeticOverflow {
                operation: "pixel buffer length",
            })?;
    if usize::try_from(allowed_bytes).ok() != Some(coverage.pixels.len()) {
        return Err(MatteCoverageError::BufferLengthMismatch {
            observed: coverage.pixels.len(),
            allowed: allowed_bytes,
        });
    }

    let scan = scan_coverage(coverage)?;

    let covered_basis_points = u32::try_from(
        scan.covered_pixel_count
            .checked_mul(u64::from(SCOPE_BASIS_POINTS))
            .ok_or(MatteCoverageError::ArithmeticOverflow {
                operation: "covered_basis_points",
            })?
            / total_pixel_count,
    )
    .map_err(|_| MatteCoverageError::ArithmeticOverflow {
        operation: "covered_basis_points",
    })?;

    let bounding_box_basis_points = if scan.covered_pixel_count == 0 {
        None
    } else {
        Some(coverage_bounding_box(
            (scan.left, scan.right, coverage.width),
            (scan.top, scan.bottom, coverage.height),
        )?)
    };
    let centroid_basis_points = if scan.coverage_sum == 0 {
        None
    } else {
        Some((
            weighted_centre(scan.weighted_x, scan.coverage_sum, coverage.width)?,
            weighted_centre(scan.weighted_y, scan.coverage_sum, coverage.height)?,
        ))
    };

    Ok(MatteCoverageStatistics {
        covered_pixel_count: scan.covered_pixel_count,
        full_pixel_count: scan.full_pixel_count,
        partial_pixel_count: scan.partial_pixel_count,
        total_pixel_count,
        covered_basis_points,
        coverage_histogram: scan.coverage_histogram,
        bounding_box_basis_points,
        centroid_basis_points,
        weighted_by_coverage: true,
    })
}

/// Per-pixel coverage evidence accumulated by [`scan_coverage`].
struct CoverageScan {
    covered_pixel_count: u64,
    full_pixel_count: u64,
    partial_pixel_count: u64,
    coverage_histogram: [u64; MATTE_COVERAGE_HISTOGRAM_BUCKETS],
    /// Half-open pixel bounds of the covered set, valid only when
    /// `covered_pixel_count` is non-zero.
    left: u32,
    top: u32,
    right: u32,
    bottom: u32,
    /// `Σ m`, and `Σ m·(2p + 1)` on each axis, for the weighted centroid.
    coverage_sum: u128,
    weighted_x: u128,
    weighted_y: u128,
}

/// Walk a validated coverage raster once, rejecting any pixel that is not an
/// opaque grey coverage sample.
fn scan_coverage(coverage: &RgbaImage) -> Result<CoverageScan, MatteCoverageError> {
    let mut scan = CoverageScan {
        covered_pixel_count: 0,
        full_pixel_count: 0,
        partial_pixel_count: 0,
        coverage_histogram: [0; MATTE_COVERAGE_HISTOGRAM_BUCKETS],
        left: u32::MAX,
        top: u32::MAX,
        right: 0,
        bottom: 0,
        coverage_sum: 0,
        weighted_x: 0,
        weighted_y: 0,
    };
    let (pixels, _) = coverage.pixels.as_chunks::<4>();
    let mut samples = pixels.iter();
    for y in 0..coverage.height {
        for x in 0..coverage.width {
            // The caller validated the buffer length against the dimensions,
            // so the iterator cannot run out; a defensive error keeps that
            // from becoming a panic if the check ever changes.
            let &[code, green, blue, alpha] =
                samples
                    .next()
                    .ok_or(MatteCoverageError::ArithmeticOverflow {
                        operation: "pixel buffer length",
                    })?;
            if alpha != 255 {
                return Err(MatteCoverageError::AlphaNotOpaque {
                    x,
                    y,
                    observed: alpha,
                    allowed: 255,
                });
            }
            if green != code || blue != code {
                return Err(MatteCoverageError::NotGrey {
                    x,
                    y,
                    red: code,
                    green,
                    blue,
                    allowed: "red, green, and blue must be equal",
                });
            }
            // `code / 16` is `min(15, floor(code * 16 / 256))` for every 8-bit
            // code, computed without a division that could round differently.
            scan.coverage_histogram[usize::from(code >> 4)] += 1;
            if code == 0 {
                continue;
            }
            scan.covered_pixel_count += 1;
            if code == 255 {
                scan.full_pixel_count += 1;
            } else {
                scan.partial_pixel_count += 1;
            }
            scan.left = scan.left.min(x);
            scan.top = scan.top.min(y);
            scan.right = scan.right.max(x + 1);
            scan.bottom = scan.bottom.max(y + 1);
            let weight = u128::from(code);
            scan.coverage_sum += weight;
            scan.weighted_x += weight * (u128::from(x) * 2 + 1);
            scan.weighted_y += weight * (u128::from(y) * 2 + 1);
        }
    }
    Ok(scan)
}

/// Convert a half-open pixel rectangle to a normalized basis-point rectangle,
/// flooring each start boundary and ceiling each exclusive end boundary.
fn coverage_bounding_box(
    horizontal: (u32, u32, u32),
    vertical: (u32, u32, u32),
) -> Result<NormalizedRoi, MatteCoverageError> {
    let (left, right, width) = horizontal;
    let (top, bottom, height) = vertical;
    let (x_basis_points, width_basis_points) = coverage_span(left, right, width)?;
    let (y_basis_points, height_basis_points) = coverage_span(top, bottom, height)?;
    Ok(NormalizedRoi::new(
        x_basis_points,
        y_basis_points,
        width_basis_points,
        height_basis_points,
    ))
}

/// One axis of [`coverage_bounding_box`], returning `(start, extent)`.
fn coverage_span(start: u32, end: u32, extent: u32) -> Result<(u32, u32), MatteCoverageError> {
    let scale = u64::from(SCOPE_BASIS_POINTS);
    let extent = u64::from(extent);
    let start_basis_points = u64::from(start) * scale / extent;
    let end_basis_points = (u64::from(end) * scale).div_ceil(extent);
    let overflow = MatteCoverageError::ArithmeticOverflow {
        operation: "bounding_box_basis_points",
    };
    let start_basis_points = u32::try_from(start_basis_points).map_err(|_| overflow.clone())?;
    let end_basis_points = u32::try_from(end_basis_points).map_err(|_| overflow.clone())?;
    let span = end_basis_points
        .checked_sub(start_basis_points)
        .ok_or(overflow)?;
    Ok((start_basis_points, span))
}

/// `Σ m·(2p + 1) · 5000 / (extent · Σ m)` rounded half away from zero, which
/// is `Σ m·centre / Σ m` in basis points for pixel centres `(p + 0.5)`.
fn weighted_centre(
    weighted: u128,
    coverage_sum: u128,
    extent: u32,
) -> Result<i64, MatteCoverageError> {
    let overflow = MatteCoverageError::ArithmeticOverflow {
        operation: "centroid_basis_points",
    };
    let half_scale = u128::from(SCOPE_BASIS_POINTS) / 2;
    let numerator = weighted
        .checked_mul(half_scale)
        .ok_or_else(|| overflow.clone())?;
    let denominator = coverage_sum
        .checked_mul(u128::from(extent))
        .ok_or_else(|| overflow.clone())?;
    // Every value is non-negative, so half-up rounding is half away from zero.
    let doubled_denominator = denominator.checked_mul(2).ok_or_else(|| overflow.clone())?;
    let rounded = numerator
        .checked_mul(2)
        .and_then(|doubled| doubled.checked_add(denominator))
        .ok_or_else(|| overflow.clone())?
        / doubled_denominator;
    i64::try_from(rounded).map_err(|_| overflow)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WaveformPeak {
    pub minimum: i16,
    pub maximum: i16,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WaveformData {
    pub asset: AssetId,
    /// Monotonic request generation supplied by the caller. This is runtime
    /// delivery metadata only and is deliberately excluded from the
    /// content-addressed disk cache serialization.
    #[serde(skip, default)]
    pub request_generation: u64,
    /// The requesting asset's media path. Asset ids are per-document, so
    /// with several projects open the path is the identity consumers key
    /// by. Not persisted: the disk cache is content-addressed, and the
    /// serving worker rebinds the path per request.
    #[serde(skip)]
    pub path: std::path::PathBuf,
    pub content_sha256: String,
    pub source_fps: Rational,
    pub source_frames: TimeCode,
    pub peaks: Vec<WaveformPeak>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ThumbnailKey {
    pub asset: AssetId,
    pub source_at: TimeCode,
    pub max_width: u32,
}

#[derive(Debug, Clone)]
pub struct ThumbnailFrame {
    pub key: ThumbnailKey,
    /// Monotonic request generation supplied by the caller. This is runtime
    /// delivery metadata only and is not part of the thumbnail cache key.
    pub request_generation: u64,
    /// See [`WaveformData::path`]: the content identity behind the id.
    pub path: std::path::PathBuf,
    pub image: Arc<RgbaImage>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VisualRequestKind {
    Waveform,
    Thumbnail(ThumbnailKey),
}

#[derive(Debug, Clone)]
pub enum VisualAssetResult {
    Waveform(Arc<WaveformData>),
    Thumbnail(ThumbnailFrame),
    Failed {
        asset: AssetId,
        /// Monotonic request generation supplied by the caller. This remains
        /// available when a worker cannot produce a waveform or thumbnail.
        request_generation: u64,
        /// See [`WaveformData::path`]: the content identity behind the id.
        path: std::path::PathBuf,
        request: VisualRequestKind,
        message: String,
    },
}

#[derive(
    Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
pub struct ExportSettings {
    pub fps: Rational,
    pub resolution: (u32, u32),
    /// Colour metadata declared for the encoded delivery, and the authority
    /// for the delivery encode itself.
    ///
    /// This is **not** a tag-only contract. Since CC1 the export path performs
    /// the managed delivery transform — the compositor's delivery readback
    /// encodes through `encode_delivery_for_description` and quantizes through
    /// `quantize_delivery16`, and the export filter graph performs the
    /// full-to-limited range and RGB-to-`Y'CbCr` conversion. `bit_depth` is the
    /// single authority for the delivery encode depth (CC6 §4.1/§4.4): the
    /// codec pixel format and the filter graph's `format` node are both driven
    /// from it, so they cannot diverge.
    pub delivery_color: ColorDescription,
    pub video_codec: String,
    pub audio_codec: String,
    pub video_bitrate: u64,
    pub audio_bitrate: u64,
    /// Runtime cancellation token. Deliberately **not** serialized: it is a
    /// live handle, not a setting, and a deserialized value reconstructs a
    /// fresh, uncancelled token (CC6 §9.5).
    #[serde(skip)]
    pub cancellation: ExportCancellation,
}

/// Delivery loudness measured from decoded PCM using the ITU-R BS.1770
/// K-weighting and gating model. Decibel values use hundredths so agent and
/// project contracts remain deterministic and JSON-safe.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
pub struct AudioLoudness {
    /// Gated programme loudness. `None` means the decoded signal was silent.
    pub integrated_lufs_hundredths: Option<i32>,
    /// Highest decoded sample before loudness weighting. `None` means silence.
    pub sample_peak_dbfs_hundredths: Option<i32>,
    pub sample_rate: u32,
    pub channels: u16,
    pub sample_frames: u64,
}

#[derive(Debug, Clone, Default)]
pub struct ExportCancellation(Arc<AtomicBool>);

impl ExportCancellation {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

impl PartialEq for ExportCancellation {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for ExportCancellation {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportProgress {
    pub completed_frames: u64,
    pub total_frames: u64,
}

pub type ProgressSink = Sender<ExportProgress>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackState {
    Paused,
    Playing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MediaEvent {
    Position(TimeCode),
    PlaybackStateChanged(PlaybackState),
    Error(MediaError),
}

/// One recognized word. Both boundaries are half-open source-frame positions
/// in the owning asset's exact frame rate.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TranscriptWord {
    pub text: String,
    pub source_start: TimeCode,
    pub source_end: TimeCode,
    /// Optional stable diarization label. Plain Whisper transcripts leave it
    /// unset; speaker-aware backends can populate it without changing edits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speaker: Option<String>,
}

/// Derived, reproducible speech data for one media asset. This deliberately
/// does not live in `Document` or the operation journal.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AssetTranscript {
    pub asset: AssetId,
    pub content_sha256: String,
    pub source_fps: Rational,
    pub words: Vec<TranscriptWord>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TranscriptStatus {
    NotRequested,
    Queued,
    Hashing,
    DownloadingModel {
        downloaded_bytes: u64,
        total_bytes: Option<u64>,
    },
    Transcribing {
        progress_percent: u8,
    },
    Ready(Arc<AssetTranscript>),
    NoSpeech,
    Cancelled,
    Failed(String),
}

impl TranscriptStatus {
    #[must_use]
    pub const fn is_running(&self) -> bool {
        matches!(
            self,
            Self::Queued
                | Self::Hashing
                | Self::DownloadingModel { .. }
                | Self::Transcribing { .. }
        )
    }
}

/// A source word mapped through a clip onto the project timeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimelineTranscriptWord {
    pub text: String,
    pub speaker: Option<String>,
    pub asset: AssetId,
    pub track: TrackId,
    pub clip: ClipId,
    pub source_start: TimeCode,
    pub source_end: TimeCode,
    pub project_start: TimeCode,
    pub project_end: TimeCode,
}

/// A half-open silent range in one asset's exact source-frame time base.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SilenceSpan {
    pub source_start: TimeCode,
    pub source_end: TimeCode,
}

/// Derived, reproducible audio-energy analysis for one media asset.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AssetSilences {
    pub asset: AssetId,
    pub content_sha256: String,
    pub source_fps: Rational,
    pub source_frames: TimeCode,
    /// Detection threshold in hundredths of a dBFS (for example, -4000 is -40 dBFS).
    pub threshold_dbfs_hundredths: i32,
    pub window_milliseconds: u32,
    pub spans: Vec<SilenceSpan>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SilenceStatus {
    NotRequested,
    Queued,
    Hashing,
    Analyzing,
    Ready(Arc<AssetSilences>),
    NoAudio,
    Cancelled,
    Failed(String),
}

impl SilenceStatus {
    #[must_use]
    pub const fn is_running(&self) -> bool {
        matches!(self, Self::Queued | Self::Hashing | Self::Analyzing)
    }
}

/// One candidate scene boundary. Confidence is stored as basis points from
/// 0.00% through 100.00% so the derived contract remains deterministic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SceneChange {
    pub source_frame: TimeCode,
    pub confidence_basis_points: u16,
}

/// Derived, reproducible scene-difference analysis for one media asset.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AssetSceneChanges {
    pub asset: AssetId,
    pub content_sha256: String,
    pub source_fps: Rational,
    pub source_frames: TimeCode,
    pub proxy_width: u32,
    pub changes: Vec<SceneChange>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SceneStatus {
    NotRequested,
    Queued,
    Hashing,
    Analyzing,
    Ready(Arc<AssetSceneChanges>),
    NoVideo,
    Cancelled,
    Failed(String),
}

impl SceneStatus {
    #[must_use]
    pub const fn is_running(&self) -> bool {
        matches!(self, Self::Queued | Self::Hashing | Self::Analyzing)
    }
}

/// A source silence span mapped through one clip onto project frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimelineSilenceSpan {
    pub asset: AssetId,
    pub track: TrackId,
    pub clip: ClipId,
    pub source_start: TimeCode,
    pub source_end: TimeCode,
    pub project_start: TimeCode,
    pub project_end: TimeCode,
}

/// A source scene boundary mapped through one clip onto a project frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimelineSceneChange {
    pub asset: AssetId,
    pub track: TrackId,
    pub clip: ClipId,
    pub source_frame: TimeCode,
    pub project_frame: TimeCode,
    pub confidence_basis_points: u16,
}

/// One locally detected rhythmic onset in an asset's exact source-frame grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BeatMarker {
    pub source_frame: TimeCode,
    /// Relative onset strength from 0.00% through 100.00%.
    pub strength_basis_points: u16,
}

/// Derived, reproducible rhythmic analysis for one audio-capable asset.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AssetBeats {
    pub asset: AssetId,
    pub content_sha256: String,
    pub source_fps: Rational,
    pub source_frames: TimeCode,
    /// Robust interval estimate in thousandths of a beat per minute.
    pub estimated_bpm_milli: u32,
    pub beats: Vec<BeatMarker>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BeatStatus {
    NotRequested,
    Queued,
    Hashing,
    Analyzing { progress_percent: Option<u8> },
    Ready(Arc<AssetBeats>),
    NoAudio,
    Cancelled,
    Failed(String),
}

impl BeatStatus {
    #[must_use]
    pub const fn is_running(&self) -> bool {
        matches!(self, Self::Queued | Self::Hashing | Self::Analyzing { .. })
    }
}

/// One source beat mapped through a real-time media clip onto project frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TimelineBeat {
    pub asset: AssetId,
    pub track: TrackId,
    pub clip: ClipId,
    pub source_frame: TimeCode,
    pub project_frame: TimeCode,
    pub strength_basis_points: u16,
    pub estimated_bpm_milli: u32,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum AnalysisKind {
    Transcript,
    Silence,
    Scene,
    Beat,
}

impl AnalysisKind {
    pub const ALL: [Self; 4] = [Self::Transcript, Self::Silence, Self::Scene, Self::Beat];
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum AnalysisPhase {
    NotRequested,
    Queued,
    Hashing,
    Downloading,
    Analyzing,
    Ready,
    Unavailable,
    Cancelled,
    Failed,
}

/// Provider-neutral lifecycle record for one derived-media job.
#[derive(
    Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
pub struct AnalysisJobStatus {
    pub asset: AssetId,
    pub kind: AnalysisKind,
    pub phase: AnalysisPhase,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress_percent: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum MediaError {
    #[error("media operation is not implemented")]
    NotImplemented,
    #[error("export was cancelled")]
    Cancelled,
    /// The managed renderer cannot prove that a decoder's native format is a
    /// supported integer source surface. This remains structured so proof
    /// callers can offer a stable recovery action instead of parsing text.
    #[error(
        "unsupported_decoder_format: {reason} (path={path}, format={format}, declared_bit_depth={declared_bit_depth:?}, decoder_bit_depth={decoder_bit_depth:?})"
    )]
    UnsupportedDecoderFormat {
        /// The source path involved in the failed managed decode.
        path: PathBuf,
        /// The `FFmpeg` pixel-format name when available.
        format: String,
        /// The declared source integer depth, if it was valid enough to read.
        declared_bit_depth: Option<u8>,
        /// The decoder's native integer depth, if it was recognized.
        decoder_bit_depth: Option<u8>,
        /// A stable human-readable reason for recovery surfaces.
        reason: String,
    },
    /// A managed delivery encode was refused with a typed colour reason.
    ///
    /// Structured rather than a `Backend(String)` so a rejection carries the
    /// same four facts a source rejection has carried since CC0 (CC6 §4.2).
    #[error(transparent)]
    DeliveryColor(#[from] crate::DeliveryColorError),
    /// Post-export delivery verification could not produce an honest
    /// measurement (CC6 §4.2).
    #[error(transparent)]
    DeliveryVerification(#[from] crate::DeliveryVerificationError),
    /// A colour QC measurement was refused with a typed reason (CC6 §3.8).
    ///
    /// [`crate::color_qc::nodes`] renders through an [`Analysis`] backend, so
    /// every refusal it raises has to travel as a `MediaError`. Carrying the
    /// [`crate::ColorQcError`] itself keeps `field`, `observed`,
    /// `allowed_values`, and `recovery_action` intact all the way to an agent
    /// or UI surface, which would otherwise have to parse the rendered
    /// message back apart to recover the code.
    ///
    /// Not `#[from]` only because `color_qc.rs` owns the hand-written
    /// `From<ColorQcError> for MediaError`, which builds exactly this variant
    /// (errata E32); `?` on a `ColorQcError` therefore keeps the typed refusal,
    /// and nothing flattens it to [`Self::Backend`] any more.
    #[error(transparent)]
    ColorQc(crate::ColorQcError),
    #[error("media backend error: {0}")]
    Backend(String),
}

impl MediaError {
    /// Return the machine-readable recovery code, when this error has one.
    #[must_use]
    pub const fn recovery_code(&self) -> Option<&'static str> {
        match self {
            Self::UnsupportedDecoderFormat { .. } => Some("unsupported_decoder_format"),
            Self::DeliveryColor(error) => Some(error.code()),
            Self::DeliveryVerification(error) => Some(error.code()),
            Self::ColorQc(error) => Some(error.code()),
            Self::NotImplemented | Self::Cancelled | Self::Backend(_) => None,
        }
    }
}

pub trait Playback: Send + Sync {
    fn set_document(&self, doc: Arc<Document>);
    fn request_frame(&self, t: TimeCode);
    fn frames(&self) -> Receiver<(TimeCode, FrameTexture)>;
    /// Non-blocking playback status stream. Implementations may coalesce ticks.
    fn events(&self) -> Receiver<MediaEvent>;
    fn play(&self, from: TimeCode);
    fn pause(&self);
    /// Seek transport and audio to an exact project frame without blocking the caller.
    fn seek(&self, to: TimeCode);
    /// Read the atomically published audio-master position.
    fn position(&self) -> TimeCode;
    /// Read the current post-limiter master output peaks for left and right.
    fn output_peaks(&self) -> [f32; 2];
}

pub trait Analysis: Send + Sync {
    /// Inspect a media file and return its project metadata.
    ///
    /// # Errors
    ///
    /// Returns a media error when the file cannot be probed.
    fn probe(&self, path: &Path) -> Result<MediaAsset, MediaError>;
    /// Inspect whether the source path is currently available and, when the
    /// file can be read, compare its fingerprint with the imported identity.
    /// Stateful media backends should override this to return verified,
    /// changed, and unreadable observations rather than the conservative
    /// default.
    fn media_availability(&self, asset: &MediaAsset) -> MediaAvailabilityStatus {
        if !asset.path.is_file() {
            return MediaAvailabilityStatus {
                kind: MediaAvailabilityKind::OfflineMissing,
                observed_fingerprint: None,
                reason: Some(format!(
                    "media path is missing or not a regular file: {}",
                    asset.path.display()
                )),
            };
        }
        MediaAvailabilityStatus {
            kind: MediaAvailabilityKind::OnlineUnverified,
            observed_fingerprint: None,
            reason: Some("source identity is not available for verification".to_owned()),
        }
    }
    /// Decode a thumbnail at an exact project frame.
    ///
    /// # Errors
    ///
    /// Returns a media error when decoding or compositing fails.
    fn thumbnail_at(&self, t: TimeCode, max_w: u32) -> Result<RgbaImage, MediaError>;
    /// Decode a thumbnail against an explicit immutable document without
    /// changing the live playback document.
    ///
    /// Backends that do not maintain playback state may use the default.
    /// Stateful backends should override this method so branch proofs cannot
    /// disturb the user's monitor or transport.
    ///
    /// # Errors
    ///
    /// Returns a media error when decoding or compositing fails.
    fn thumbnail_for_document(
        &self,
        _document: Arc<Document>,
        t: TimeCode,
        max_w: u32,
    ) -> Result<RgbaImage, MediaError> {
        self.thumbnail_at(t, max_w)
    }
    /// Render one exact project frame at the document's full resolution for
    /// managed colour proof. This is intentionally separate from thumbnail
    /// rendering: proxy dimensions cannot establish full-raster conformance.
    ///
    /// Stateful backends should use a branch-scoped renderer and must not
    /// mutate the live playback document or reuse a stale proxy surface.
    ///
    /// # Errors
    ///
    /// Returns a media error when the backend cannot provide a managed
    /// full-resolution proof frame.
    fn monitor_proof_for_document(
        &self,
        _document: Arc<Document>,
        _t: TimeCode,
    ) -> Result<MonitorProof, MediaError> {
        Err(MediaError::NotImplemented)
    }
    /// Render one exact project frame's matte coverage for a single colour
    /// node, at the document's full resolution.
    ///
    /// Like [`Analysis::monitor_proof_for_document`] this is deliberately
    /// separate from thumbnail and monitor rendering. The backend must render
    /// the target clip in isolation, so no other layer composites over the
    /// coverage, at the document's full raster; a proxy raster cannot
    /// establish coverage conformance. The readback applies **no transfer at
    /// all**: the coverage image is `round(255 · clamp(m, 0, 1))` in every
    /// colour channel with `A = 255`, an integer quantization of a coverage
    /// scalar rather than a monitoring transform.
    ///
    /// The returned [`MatteProofMetadata::render`] is ordinary renderer
    /// provenance: [`MonitorProofRenderKind`] names the renderer
    /// implementation, not an output target, and is never extended with a
    /// matte value. The output target is stated by the matte metadata's own
    /// fields.
    ///
    /// Stateful backends must use a branch-scoped renderer and must not mutate
    /// the live playback document or reuse a stale proxy surface.
    ///
    /// # Errors
    ///
    /// Returns a media error when the backend cannot render an isolated
    /// full-resolution coverage frame. A node that is inactive or carries no
    /// matte fails typed — see [`MatteProofError`] — rather than returning a
    /// blank frame.
    fn matte_proof_for_document(
        &self,
        _document: Arc<Document>,
        _at: TimeCode,
        _clip: ClipId,
        _effect: EffectId,
    ) -> Result<MatteProof, MediaError> {
        Err(MediaError::NotImplemented)
    }
    /// Render one exact project frame's composited **scene-linear** working
    /// surface at the document's full resolution (CC6 §2.2).
    ///
    /// Like [`Analysis::monitor_proof_for_document`] this is deliberately
    /// separate from thumbnail and monitor rendering, and for the same reason:
    /// proxy dimensions cannot establish full-raster conformance. It takes no
    /// scale and binds full resolution, so **no proxy working proof can be
    /// produced at all** — a QC consumer that finds
    /// `metadata.render.full_resolution == false` refuses typed rather than
    /// measuring a raster it cannot vouch for (CC6 §2.3).
    ///
    /// The readback applies **no transfer and no clamp**: values are the
    /// production `Rgba16Float` composite target's own linear light, which may
    /// be negative and may exceed `1.0`. That is the surface CC6 measures,
    /// because the delivery clamp is the only clamp in the managed pipeline
    /// and nothing downstream of it can count what it ate.
    ///
    /// Stateful backends must use a branch-scoped renderer and must not mutate
    /// the live playback document or reuse a stale proxy surface.
    ///
    /// # Errors
    ///
    /// Returns a media error when the backend cannot render an isolated
    /// full-resolution linear frame.
    fn working_proof_for_document(
        &self,
        _document: Arc<Document>,
        _at: TimeCode,
    ) -> Result<WorkingProof, MediaError> {
        Err(MediaError::NotImplemented)
    }
    /// Decode a finished delivery encode and compare it against a freshly
    /// rendered delivery reference (CC6 §6.1).
    ///
    /// The implementation decodes through the crate's own bindings-based
    /// decoder in one seek-based pass over a bounded, deterministic frame
    /// sample; probes the written file's tags; measures the decoded file's
    /// **native** `Y'CbCr` planes; and compares against the per-lane budgets the
    /// request carries. It is a measurement: it must never move, rename, or
    /// delete the encode it just read.
    ///
    /// # Errors
    ///
    /// Returns a media error when the output cannot be decoded, a sampled
    /// reference render is not full-resolution, or the decoded frame count
    /// disagrees with the document's implied count.
    fn verify_delivery_output(
        &self,
        _document: Arc<Document>,
        _path: &Path,
        _settings: &ExportSettings,
        _request: DeliveryVerificationRequest,
    ) -> Result<DeliveryVerification, MediaError> {
        Err(MediaError::NotImplemented)
    }
    /// Queue derived speech recognition without blocking the caller. Repeated
    /// requests for the same asset are coalesced by the implementation.
    fn request_transcription(&self, asset: MediaAsset);
    /// Queue speech recognition with an optional ISO 639-1 language hint.
    /// Backends that do not support hints fall back to ordinary transcription.
    fn request_transcription_with_language(&self, asset: MediaAsset, _language: Option<&str>) {
        self.request_transcription(asset);
    }
    /// Return the latest state for an asset's derived transcript.
    ///
    /// Takes the full asset, not just its id: asset ids are per-document, so
    /// with several projects open the same id can name different files. The
    /// asset's path is the identity derived data is keyed by.
    fn transcript_status(&self, asset: &MediaAsset) -> TranscriptStatus;
    /// Return words currently audible on the timeline, optionally restricted
    /// to a half-open range of project frames.
    ///
    /// # Errors
    ///
    /// Returns a media error when timeline/source frame mapping fails.
    fn timeline_transcript(
        &self,
        document: &Document,
        range: Option<std::ops::Range<TimeCode>>,
    ) -> Result<Vec<TimelineTranscriptWord>, MediaError>;
    /// Queue windowed-RMS silence analysis without blocking the caller.
    fn request_silence_detection(&self, asset: MediaAsset);
    /// See [`Analysis::transcript_status`] for why this takes the full asset.
    fn silence_status(&self, asset: &MediaAsset) -> SilenceStatus;
    /// Return detected silence spans mapped into project time.
    ///
    /// # Errors
    ///
    /// Returns a media error when timeline/source frame mapping fails.
    fn timeline_silences(
        &self,
        document: &Document,
        range: Option<std::ops::Range<TimeCode>>,
        minimum_source_frames: TimeCode,
    ) -> Result<Vec<TimelineSilenceSpan>, MediaError>;
    /// Queue proxy-resolution scene analysis without blocking the caller.
    fn request_scene_detection(&self, asset: MediaAsset);
    /// See [`Analysis::transcript_status`] for why this takes the full asset.
    fn scene_status(&self, asset: &MediaAsset) -> SceneStatus;
    /// Return scene changes mapped into project time.
    ///
    /// # Errors
    ///
    /// Returns a media error when timeline/source frame mapping fails.
    fn timeline_scene_changes(
        &self,
        document: &Document,
        range: Option<std::ops::Range<TimeCode>>,
        minimum_confidence_basis_points: u16,
    ) -> Result<Vec<TimelineSceneChange>, MediaError>;
    /// Measure a decoded source asset using the delivery loudness contract.
    ///
    /// # Errors
    ///
    /// Returns a media error when audio cannot be decoded or measured.
    fn asset_loudness(&self, _asset: &MediaAsset) -> Result<AudioLoudness, MediaError> {
        Err(MediaError::NotImplemented)
    }
    /// Render the current audio graph in memory and measure its delivery loudness.
    ///
    /// # Errors
    ///
    /// Returns a media error when timeline audio cannot be rendered or measured.
    fn timeline_loudness(&self, _document: &Document) -> Result<AudioLoudness, MediaError> {
        Err(MediaError::NotImplemented)
    }
    /// Queue deterministic beat/onset analysis without blocking the caller.
    fn request_beat_detection(&self, _asset: MediaAsset) {}
    /// Return the latest beat-analysis state for an asset.
    fn beat_status(&self, _asset: &MediaAsset) -> BeatStatus {
        BeatStatus::NotRequested
    }
    /// Return detected beats mapped into project time.
    ///
    /// # Errors
    ///
    /// Returns a media error when timeline/source frame mapping fails.
    fn timeline_beats(
        &self,
        _document: &Document,
        _range: Option<std::ops::Range<TimeCode>>,
        _minimum_strength_basis_points: u16,
    ) -> Result<Vec<TimelineBeat>, MediaError> {
        Ok(Vec::new())
    }
    /// Return a uniform lifecycle view over every analysis family.
    fn analysis_jobs(&self, asset: &MediaAsset) -> Vec<AnalysisJobStatus> {
        analysis_job_statuses(self, asset)
    }
    /// Cooperatively cancel queued or running work. Repeated cancellation is harmless.
    fn cancel_analysis(&self, _asset: &MediaAsset, _kind: AnalysisKind) -> bool {
        false
    }
    /// Queue a content-addressed waveform extraction without blocking the caller.
    fn request_waveform(&self, asset: MediaAsset, request_generation: u64) -> bool;
    /// Queue one source-frame thumbnail without blocking the caller.
    fn request_thumbnail(
        &self,
        asset: MediaAsset,
        source_at: TimeCode,
        max_width: u32,
        request_generation: u64,
    ) -> bool;
    /// Bounded stream of ready waveform and thumbnail data.
    fn visual_asset_results(&self) -> Receiver<VisualAssetResult>;
    /// Return inventory for the fixed media-cache families. The default is an
    /// empty inventory for stateless test backends.
    fn cache_inventory(&self) -> MediaCacheInventory {
        MediaCacheInventory {
            families: Vec::new(),
        }
    }
    /// Clear one owned cache family. Implementations must not interpret this
    /// as permission to delete project files, source media, or model weights.
    ///
    /// # Errors
    ///
    /// Returns a media error when the cache family cannot be inspected or
    /// cleared safely, or when the implementation does not own that family.
    fn clear_cache(&self, _family: MediaCacheFamily) -> Result<MediaCacheClearResult, MediaError> {
        Err(MediaError::NotImplemented)
    }
}

/// Recheck every timeline-referenced source immediately before an export is
/// accepted or started.
///
/// The observation comes from the active [`Analysis`] backend rather than the
/// persisted document, because file availability and content identity are
/// machine-local and can change while a project is open. Only
/// [`MediaAvailabilityKind::OnlineVerified`] is accepted. In particular,
/// `OnlineUnverified` legacy sources are blocked until they have been
/// verified through import/relink, preventing a same-path replacement from
/// silently entering delivery. Audio sources use the same rule as visual
/// sources, while unused media-pool entries are excluded.
#[must_use]
pub fn export_media_preflight(
    document: &Document,
    analysis: &dyn Analysis,
) -> ExportMediaPreflightReport {
    export_media_preflight_with(document, |asset| analysis.media_availability(asset))
}

fn export_media_preflight_with(
    document: &Document,
    mut availability_for: impl FnMut(&MediaAsset) -> MediaAvailabilityStatus,
) -> ExportMediaPreflightReport {
    let mut checked_assets = Vec::new();
    let mut issues = Vec::new();
    for asset in document.timeline_referenced_media_assets() {
        checked_assets.push(asset.id);
        let availability = availability_for(asset);
        if availability.kind != MediaAvailabilityKind::OnlineVerified {
            issues.push(ExportMediaPreflightIssue {
                asset: asset.id,
                asset_name: asset.name.clone(),
                availability,
            });
        }
    }
    ExportMediaPreflightReport {
        checked_assets,
        issues,
    }
}

fn analysis_job_statuses<A: Analysis + ?Sized>(
    analysis: &A,
    asset: &MediaAsset,
) -> Vec<AnalysisJobStatus> {
    vec![
        transcript_job_status(asset.id, analysis.transcript_status(asset)),
        silence_job_status(asset.id, analysis.silence_status(asset)),
        scene_job_status(asset.id, analysis.scene_status(asset)),
        beat_job_status(asset.id, analysis.beat_status(asset)),
    ]
}

fn job(
    asset: AssetId,
    kind: AnalysisKind,
    phase: AnalysisPhase,
    progress_percent: Option<u8>,
    error: Option<String>,
) -> AnalysisJobStatus {
    AnalysisJobStatus {
        asset,
        kind,
        phase,
        progress_percent,
        error,
    }
}

fn transcript_job_status(asset: AssetId, status: TranscriptStatus) -> AnalysisJobStatus {
    match status {
        TranscriptStatus::NotRequested => job(
            asset,
            AnalysisKind::Transcript,
            AnalysisPhase::NotRequested,
            None,
            None,
        ),
        TranscriptStatus::Queued => job(
            asset,
            AnalysisKind::Transcript,
            AnalysisPhase::Queued,
            Some(0),
            None,
        ),
        TranscriptStatus::Hashing => job(
            asset,
            AnalysisKind::Transcript,
            AnalysisPhase::Hashing,
            None,
            None,
        ),
        TranscriptStatus::DownloadingModel {
            downloaded_bytes,
            total_bytes,
        } => job(
            asset,
            AnalysisKind::Transcript,
            AnalysisPhase::Downloading,
            total_bytes.and_then(|total| percent(downloaded_bytes, total)),
            None,
        ),
        TranscriptStatus::Transcribing { progress_percent } => job(
            asset,
            AnalysisKind::Transcript,
            AnalysisPhase::Analyzing,
            Some(progress_percent),
            None,
        ),
        TranscriptStatus::Ready(_) => job(
            asset,
            AnalysisKind::Transcript,
            AnalysisPhase::Ready,
            Some(100),
            None,
        ),
        TranscriptStatus::NoSpeech => job(
            asset,
            AnalysisKind::Transcript,
            AnalysisPhase::Unavailable,
            Some(100),
            None,
        ),
        TranscriptStatus::Cancelled => job(
            asset,
            AnalysisKind::Transcript,
            AnalysisPhase::Cancelled,
            None,
            None,
        ),
        TranscriptStatus::Failed(error) => job(
            asset,
            AnalysisKind::Transcript,
            AnalysisPhase::Failed,
            None,
            Some(error),
        ),
    }
}

fn silence_job_status(asset: AssetId, status: SilenceStatus) -> AnalysisJobStatus {
    let (phase, progress, error) = match status {
        SilenceStatus::NotRequested => (AnalysisPhase::NotRequested, None, None),
        SilenceStatus::Queued => (AnalysisPhase::Queued, Some(0), None),
        SilenceStatus::Hashing => (AnalysisPhase::Hashing, None, None),
        SilenceStatus::Analyzing => (AnalysisPhase::Analyzing, None, None),
        SilenceStatus::Ready(_) => (AnalysisPhase::Ready, Some(100), None),
        SilenceStatus::NoAudio => (AnalysisPhase::Unavailable, Some(100), None),
        SilenceStatus::Cancelled => (AnalysisPhase::Cancelled, None, None),
        SilenceStatus::Failed(error) => (AnalysisPhase::Failed, None, Some(error)),
    };
    job(asset, AnalysisKind::Silence, phase, progress, error)
}

fn scene_job_status(asset: AssetId, status: SceneStatus) -> AnalysisJobStatus {
    let (phase, progress, error) = match status {
        SceneStatus::NotRequested => (AnalysisPhase::NotRequested, None, None),
        SceneStatus::Queued => (AnalysisPhase::Queued, Some(0), None),
        SceneStatus::Hashing => (AnalysisPhase::Hashing, None, None),
        SceneStatus::Analyzing => (AnalysisPhase::Analyzing, None, None),
        SceneStatus::Ready(_) => (AnalysisPhase::Ready, Some(100), None),
        SceneStatus::NoVideo => (AnalysisPhase::Unavailable, Some(100), None),
        SceneStatus::Cancelled => (AnalysisPhase::Cancelled, None, None),
        SceneStatus::Failed(error) => (AnalysisPhase::Failed, None, Some(error)),
    };
    job(asset, AnalysisKind::Scene, phase, progress, error)
}

fn beat_job_status(asset: AssetId, status: BeatStatus) -> AnalysisJobStatus {
    let (phase, progress, error) = match status {
        BeatStatus::NotRequested => (AnalysisPhase::NotRequested, None, None),
        BeatStatus::Queued => (AnalysisPhase::Queued, Some(0), None),
        BeatStatus::Hashing => (AnalysisPhase::Hashing, None, None),
        BeatStatus::Analyzing { progress_percent } => {
            (AnalysisPhase::Analyzing, progress_percent, None)
        }
        BeatStatus::Ready(_) => (AnalysisPhase::Ready, Some(100), None),
        BeatStatus::NoAudio => (AnalysisPhase::Unavailable, Some(100), None),
        BeatStatus::Cancelled => (AnalysisPhase::Cancelled, None, None),
        BeatStatus::Failed(error) => (AnalysisPhase::Failed, None, Some(error)),
    };
    job(asset, AnalysisKind::Beat, phase, progress, error)
}

fn percent(value: u64, total: u64) -> Option<u8> {
    if total == 0 {
        return None;
    }
    Some(u8::try_from(value.saturating_mul(100).saturating_div(total).min(100)).unwrap_or(100))
}

pub trait Export: Send + Sync {
    /// Export the current document to a media file.
    ///
    /// # Errors
    ///
    /// Returns a media error when export fails or is cancelled.
    fn export(
        &self,
        out: &Path,
        settings: ExportSettings,
        progress: ProgressSink,
    ) -> Result<(), MediaError>;

    /// Export an explicit immutable document without replacing live playback.
    ///
    /// The default preserves compatibility for stateless test backends.
    /// Stateful production backends should override it.
    ///
    /// # Errors
    ///
    /// Returns a media error when export fails or is cancelled.
    fn export_document(
        &self,
        _document: Arc<Document>,
        out: &Path,
        settings: ExportSettings,
        progress: ProgressSink,
    ) -> Result<(), MediaError> {
        self.export(out, settings, progress)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Clip, ClipContent, MediaKind, Rational, Track, TrackKind};

    fn asset(id: u64, kind: MediaKind) -> MediaAsset {
        MediaAsset {
            id: AssetId(id),
            path: format!("fixture-{id}.mov").into(),
            name: format!("fixture-{id}"),
            duration: TimeCode(30),
            fps: Rational::new(30, 1).unwrap(),
            kind,
            resolution: Some((1920, 1080)),
            source_fingerprint: MediaSourceFingerprint::unknown(),
            color_description: ColorDescription::default(),
        }
    }

    fn media_clip(id: u64) -> Clip {
        Clip {
            id: ClipId(id),
            asset: AssetId(id),
            source_range: TimeCode::ZERO..TimeCode(30),
            content: ClipContent::Media,
            timeline_start: TimeCode::ZERO,
            effects: Vec::new(),
            transition_in: None,
            link: None,
            audio_gain_tenth_db: 0,
            audio_fade_in_frames: TimeCode::ZERO,
            audio_fade_out_frames: TimeCode::ZERO,
            speed_percent: 100,
        }
    }

    fn document_with_video_audio_and_unused() -> Document {
        Document {
            tracks: vec![
                Track {
                    id: TrackId(1),
                    kind: TrackKind::Video,
                    sync_lock: true,
                    clips: vec![media_clip(1)],
                },
                Track {
                    id: TrackId(2),
                    kind: TrackKind::Audio,
                    sync_lock: true,
                    clips: vec![media_clip(2)],
                },
            ],
            media_pool: vec![
                asset(1, MediaKind::Video),
                asset(2, MediaKind::Audio),
                asset(3, MediaKind::AudioVideo),
            ],
            duration: TimeCode(30),
            ..Document::default()
        }
    }

    fn status(kind: MediaAvailabilityKind) -> MediaAvailabilityStatus {
        MediaAvailabilityStatus {
            kind,
            observed_fingerprint: None,
            reason: Some("fixture availability".to_owned()),
        }
    }

    #[test]
    fn export_preflight_checks_only_timeline_video_and_audio_sources() {
        let document = document_with_video_audio_and_unused();
        let mut observed = Vec::new();
        let report = export_media_preflight_with(&document, |asset| {
            observed.push(asset.id);
            status(MediaAvailabilityKind::OnlineVerified)
        });

        assert!(report.export_ready());
        assert_eq!(report.checked_assets, vec![AssetId(1), AssetId(2)]);
        assert_eq!(observed, vec![AssetId(1), AssetId(2)]);
    }

    #[test]
    fn export_preflight_blocks_changed_and_legacy_unverified_sources() {
        let document = document_with_video_audio_and_unused();
        let report = export_media_preflight_with(&document, |asset| match asset.id {
            AssetId(1) => status(MediaAvailabilityKind::Changed),
            AssetId(2) => status(MediaAvailabilityKind::OnlineUnverified),
            _ => unreachable!("unused assets are not observed"),
        });

        assert!(!report.export_ready());
        assert_eq!(report.issues.len(), 2);
        assert_eq!(report.issues[0].asset, AssetId(1));
        assert_eq!(
            report.issues[0].availability.kind,
            MediaAvailabilityKind::Changed
        );
        assert_eq!(report.issues[1].asset, AssetId(2));
        assert_eq!(
            report.issues[1].availability.kind,
            MediaAvailabilityKind::OnlineUnverified
        );
        assert!(report.summary().contains("relink or recovery"));
    }

    /// A backend that implements only the trait's required methods.
    ///
    /// Its whole job is to prove that a new defaulted method — CC5's
    /// [`Analysis::matte_proof_for_document`] — cannot silently break an
    /// existing implementation and never invents a proof of its own.
    struct MinimalAnalysis {
        visual_results: Receiver<VisualAssetResult>,
    }

    impl MinimalAnalysis {
        fn new() -> Self {
            let (_sender, visual_results) = crossbeam_channel::unbounded();
            Self { visual_results }
        }
    }

    impl Analysis for MinimalAnalysis {
        fn probe(&self, _path: &Path) -> Result<MediaAsset, MediaError> {
            Err(MediaError::NotImplemented)
        }

        fn thumbnail_at(&self, _t: TimeCode, _max_w: u32) -> Result<RgbaImage, MediaError> {
            Err(MediaError::NotImplemented)
        }

        fn request_transcription(&self, _asset: MediaAsset) {}

        fn transcript_status(&self, _asset: &MediaAsset) -> TranscriptStatus {
            TranscriptStatus::NotRequested
        }

        fn timeline_transcript(
            &self,
            _document: &Document,
            _range: Option<std::ops::Range<TimeCode>>,
        ) -> Result<Vec<TimelineTranscriptWord>, MediaError> {
            Ok(Vec::new())
        }

        fn request_silence_detection(&self, _asset: MediaAsset) {}

        fn silence_status(&self, _asset: &MediaAsset) -> SilenceStatus {
            SilenceStatus::NotRequested
        }

        fn timeline_silences(
            &self,
            _document: &Document,
            _range: Option<std::ops::Range<TimeCode>>,
            _minimum_source_frames: TimeCode,
        ) -> Result<Vec<TimelineSilenceSpan>, MediaError> {
            Ok(Vec::new())
        }

        fn request_scene_detection(&self, _asset: MediaAsset) {}

        fn scene_status(&self, _asset: &MediaAsset) -> SceneStatus {
            SceneStatus::NotRequested
        }

        fn timeline_scene_changes(
            &self,
            _document: &Document,
            _range: Option<std::ops::Range<TimeCode>>,
            _minimum_confidence_basis_points: u16,
        ) -> Result<Vec<TimelineSceneChange>, MediaError> {
            Ok(Vec::new())
        }

        fn request_waveform(&self, _asset: MediaAsset, _request_generation: u64) -> bool {
            false
        }

        fn request_thumbnail(
            &self,
            _asset: MediaAsset,
            _source_at: TimeCode,
            _max_width: u32,
            _request_generation: u64,
        ) -> bool {
            false
        }

        fn visual_asset_results(&self) -> Receiver<VisualAssetResult> {
            self.visual_results.clone()
        }
    }

    /// CC5 §4.1: the matte proof is a defaulted trait method, so a backend
    /// that cannot render coverage fails typed rather than returning a frame.
    #[test]
    fn matte_proof_defaults_to_not_implemented() {
        let analysis = MinimalAnalysis::new();
        let document = Arc::new(document_with_video_audio_and_unused());

        assert_eq!(
            analysis.matte_proof_for_document(
                Arc::clone(&document),
                TimeCode::ZERO,
                ClipId(1),
                EffectId(1),
            ),
            Err(MediaError::NotImplemented)
        );
        // The monitor proof default is unchanged.
        assert_eq!(
            analysis.monitor_proof_for_document(document, TimeCode::ZERO),
            Err(MediaError::NotImplemented)
        );
    }

    /// CC6 §2.2/§6.1: the working proof and the delivery verification are
    /// defaulted trait methods, so a backend that can render neither fails
    /// typed rather than inventing a proof or a pass.
    #[test]
    fn working_proof_and_delivery_verification_default_to_not_implemented() {
        let analysis = MinimalAnalysis::new();
        let document = Arc::new(document_with_video_audio_and_unused());
        let settings = crate::DeliveryProfile::SourceMaster.export_settings(
            &document,
            crate::DeliveryEncodeDepth::Eight,
            ExportCancellation::default(),
        );

        assert_eq!(
            analysis
                .working_proof_for_document(Arc::clone(&document), TimeCode::ZERO)
                .err(),
            Some(MediaError::NotImplemented)
        );
        assert_eq!(
            analysis
                .verify_delivery_output(
                    document,
                    Path::new("never-read.mp4"),
                    &settings,
                    crate::DeliveryVerificationRequest::new(
                        crate::DeliveryEncodeDepth::Eight,
                        settings.delivery_color.clone(),
                    ),
                )
                .err(),
            Some(MediaError::NotImplemented)
        );
    }

    /// CC6 §9.7: the two new typed delivery errors carry their own recovery
    /// codes through `MediaError`, while a backend error still carries none.
    #[test]
    fn typed_delivery_errors_carry_their_recovery_code_through_media_error() {
        let color = MediaError::from(crate::DeliveryColorError::EncoderPixelFormatUnavailable {
            observed: "yuv420p".to_owned(),
            allowed: "yuv420p10le".to_owned(),
        });
        assert_eq!(
            color.recovery_code(),
            Some("delivery_encoder_pixel_format_unavailable")
        );
        let verification =
            MediaError::from(crate::DeliveryVerificationError::PlaneOutOfContainer {
                observed: "1024".to_owned(),
                allowed: "0..=1023",
            });
        assert_eq!(
            verification.recovery_code(),
            Some("delivery_verification_plane_out_of_container")
        );
        assert_eq!(
            MediaError::Backend("opaque".to_owned()).recovery_code(),
            None
        );
        assert_eq!(MediaError::NotImplemented.recovery_code(), None);
    }

    /// CC6 §3.8: every `ColorQcError` variant survives the trip through
    /// `MediaError` with its own code, so the per-node attribution path never
    /// has to recover a typed refusal by parsing a rendered message.
    #[test]
    fn every_color_qc_refusal_keeps_its_code_through_media_error() {
        for expected in [
            crate::ColorQcError::ProxyProofRefused {
                observed: "false".to_owned(),
                allowed: "true",
            },
            crate::ColorQcError::RasterLengthMismatch {
                observed: "3".to_owned(),
                allowed: "4".to_owned(),
            },
            crate::ColorQcError::EmptyPopulation {
                observed: "0 visible pixels".to_owned(),
                allowed: "at least one",
            },
            crate::ColorQcError::NodeBudgetExceeded {
                observed: "17".to_owned(),
                allowed: "1..=16",
            },
            crate::ColorQcError::MatteRegionRasterMismatch {
                observed: "8x8".to_owned(),
                allowed: "16x16".to_owned(),
            },
            crate::ColorQcError::NodeRemovalRejected {
                clip: crate::ClipId(1),
                effect: crate::EffectId(2),
                reason: "clips are not sorted".to_owned(),
            },
        ] {
            let carried = MediaError::ColorQc(expected.clone());
            assert_eq!(carried.recovery_code(), Some(expected.code()));
            // `#[error(transparent)]`: the rendered message is the refusal's
            // own, with no wrapper label to strip back off.
            assert_eq!(carried.to_string(), expected.to_string());
            assert_eq!(carried, MediaError::ColorQc(expected));
        }
    }
}
