//! S3 C3: typed and drawn comments on the displayed picture, private saved
//! revisions and immutable Revise cut batches. Every anchor comes from the bound
//! image, never from the person's latest selection or requested position.
use crate::{app::KinewrightApp, presenter::DisplayCell};
use eframe::egui::{self, Pos2, Rect, Vec2};
use kinewright_core::{ClipId, Document, FrameTexture, HistorySnapshot, TimeCode, TrackKind};
use kinewright_project::{
    bundle::{BundleError, ProjectBundle},
    bundle_records::{BranchCheckpoint, BranchId, ObjectRef, ProjectId},
    editing_session::Ja1EditingSession,
    receipts::{CommandId, IssuerId, ReceiptLookup, ReceiptResult},
    storage_budget::StorageLimits,
    store::{ProjectStore, StorePin},
    studio_records::{
        CommentId, FeedbackAnchor, FeedbackBatch, FeedbackBatchId, FeedbackEvidenceKind,
        FeedbackIndex, FeedbackRevision, FeedbackTarget, FrameRate, MetadataChange,
        MetadataEnvelope, MetadataResult, MetadataScope, MetadataUpdate, Orientation,
        ReviseRequest, SnapshotDescriptor, SnapshotKey, VoiceRef,
    },
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    io::Write as _,
    path::{Path, PathBuf},
    sync::Arc,
};

/// The picture as laid out: its rectangle after letterboxing and scaling.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DisplayedImage {
    pub(crate) rect: Rect,
    pub(crate) orientation: Orientation,
}
impl DisplayedImage {
    /// `texture` is the bound image's pixel size before orientation.
    #[allow(
        clippy::cast_precision_loss,
        reason = "image sides map to screen points well inside f32 precision"
    )]
    pub(crate) fn fit(area: Rect, texture: [u32; 2], orientation: Orientation) -> Self {
        let [width, height] = texture.map(|side| side as f32);
        let shown = if matches!(orientation, Orientation::Rotate90 | Orientation::Rotate270) {
            Vec2::new(height, width)
        } else {
            Vec2::new(width, height)
        };
        Self {
            rect: crate::studio::fit(area, shown),
            orientation,
        }
    }
    /// The raster-normalised point under `at`, or `None` in the letterbox bars.
    pub(crate) fn normalise(&self, at: Pos2) -> Option<[f32; 2]> {
        if !self.rect.contains(at) || self.rect.width() <= 0.0 || self.rect.height() <= 0.0 {
            return None;
        }
        let x = (at.x - self.rect.left()) / self.rect.width();
        let y = (at.y - self.rect.top()) / self.rect.height();
        Some(match self.orientation {
            Orientation::Upright => [x, y],
            Orientation::Rotate90 => [y, 1.0 - x],
            Orientation::Rotate180 => [1.0 - x, 1.0 - y],
            Orientation::Rotate270 => [1.0 - y, x],
        })
    }
    pub(crate) fn screen(&self, [u, v]: [f32; 2]) -> Pos2 {
        let [x, y] = match self.orientation {
            Orientation::Upright => [u, v],
            Orientation::Rotate90 => [1.0 - v, u],
            Orientation::Rotate180 => [1.0 - u, 1.0 - v],
            Orientation::Rotate270 => [v, 1.0 - u],
        };
        self.rect.min + Vec2::new(x * self.rect.width(), y * self.rect.height())
    }
}

/// One drawn stroke in raster-normalised points.
pub(crate) type Stroke = Vec<[f32; 2]>;
fn quantise(value: f32) -> f64 {
    (f64::from(value) * 10_000.0).round() / 10_000.0
}
/// The exact strokes evidence: nonempty strokes of `[x, y]` pairs at 1e-4.
pub(crate) fn strokes_bytes(strokes: &[Stroke]) -> Result<Vec<u8>, String> {
    let strokes = strokes
        .iter()
        .filter(|stroke| !stroke.is_empty())
        .map(|stroke| {
            stroke
                .iter()
                .map(|[x, y]| [quantise(*x), quantise(*y)])
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    serde_json::to_vec(&strokes).map_err(|error| error.to_string())
}
fn points(strokes: &[Stroke]) -> usize {
    strokes.iter().map(Vec::len).sum()
}

/// The manual history a picture of the current edit shows: its branch, the digest
/// of its whole history (baseline, every batch and the undo/redo cursor), and its
/// document at its journal position. Only a published snapshot of this exact
/// history on this branch acknowledges it. A later head, another branch, or another
/// history with equal content and length never does.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AcceptedCut {
    pub(crate) branch: BranchId,
    pub(crate) history_sha256: String,
    pub(crate) document_hash: String,
    pub(crate) batches: u64,
}
impl AcceptedCut {
    pub(crate) fn of(branch: &BranchId, history: &HistorySnapshot) -> Result<Self, String> {
        let document = serde_json::to_vec(&*history.document).map_err(text)?;
        let whole = serde_json::to_vec(history).map_err(text)?;
        Ok(Self {
            branch: branch.clone(),
            history_sha256: kinewright_media::sha256_bytes(&whole),
            document_hash: kinewright_media::sha256_bytes(&document),
            batches: history.floor_offset + history.batches.len() as u64,
        })
    }
    /// Only the same branch and history acknowledge a cut.
    pub(crate) fn acknowledged_by(&self, published: &Self) -> bool {
        self == published
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Shown {
    Version { snapshot: SnapshotKey },
    Cut { cut: AcceptedCut },
}
/// The presenter's binding of the image the mark was made on.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BoundIdentity {
    pub(crate) frame_id: u64,
    pub(crate) epoch: u64,
    pub(crate) seq: u64,
    pub(crate) stale: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PixelDigest {
    pub(crate) width: u32,
    pub(crate) height: u32,
    /// SHA-256 of the RGBA8 bytes.
    pub(crate) sha256: String,
}
/// Proofs name the displayed identity and a pixel digest, never the pixels.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FrameProof {
    pub(crate) version: u32,
    pub(crate) shown: Shown,
    pub(crate) frame: i64,
    pub(crate) fps: FrameRate,
    pub(crate) raster: [u32; 2],
    pub(crate) orientation: Orientation,
    pub(crate) bound: BoundIdentity,
    /// The GPU route binds no CPU copy; its proof records the bound identity only.
    pub(crate) pixels: Option<PixelDigest>,
}

/// An asynchronous digest of the bound pixels. It owns the image it started on,
/// so it cannot complete with later pixels under this label.
pub(crate) struct Readback(std::thread::JoinHandle<PixelDigest>);
impl Readback {
    fn start(texture: &FrameTexture) -> Result<Self, String> {
        let (width, height, rgba) = (texture.width, texture.height, Arc::clone(&texture.rgba));
        std::thread::Builder::new()
            .name("kinewright-frame-proof".into())
            .spawn(move || PixelDigest {
                width,
                height,
                sha256: kinewright_media::sha256_bytes(&rgba),
            })
            .map(Self)
            .map_err(|error| error.to_string())
    }
    fn finish(self) -> Result<PixelDigest, String> {
        self.0
            .join()
            .map_err(|_| "The frame proof readback failed.".to_owned())
    }
}

/// What the picture shows now. Only `cell` and its pixels anchor a mark.
pub(crate) struct View<'a> {
    pub(crate) cell: &'a DisplayCell,
    /// The CPU copy of a bound image and the cell id it was bound with.
    pub(crate) pixels: Option<(u64, &'a FrameTexture)>,
    /// The person's latest version choice and requested position. Either can run
    /// ahead of the picture while a selection or seek is pending, so neither anchors.
    pub(crate) selection: Option<&'a SnapshotKey>,
    pub(crate) requested: TimeCode,
    /// The current edit's exact cut and document, when the cell shows the manual Core.
    pub(crate) manual: Option<(AcceptedCut, &'a Document)>,
    pub(crate) texture: [u32; 2],
}

/// A clip under the mark. Overlap, blur and alpha never imply ownership: the person
/// chooses one, or the target stays unresolved.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Layer {
    pub(crate) clip: ClipId,
    pub(crate) label: String,
}
/// Video clips at `frame`, top layer first.
pub(crate) fn layers_at(document: &Document, frame: TimeCode) -> Vec<Layer> {
    let mut layers = Vec::new();
    for (index, track) in document.tracks.iter().enumerate().rev() {
        if track.kind != TrackKind::Video {
            continue;
        }
        for clip in &track.clips {
            let end = document
                .clip_duration(clip)
                .map(|duration| TimeCode(clip.timeline_start.0.saturating_add(duration.0)));
            if clip.timeline_start <= frame && end.is_ok_and(|end| frame < end) {
                let name = document
                    .media_pool
                    .iter()
                    .find(|asset| asset.id == clip.asset)
                    .map_or("Clip", |asset| asset.name.as_str());
                layers.push(Layer {
                    clip: clip.id,
                    label: format!("{name} · track {}", index + 1),
                });
            }
        }
    }
    layers
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[allow(
    clippy::large_enum_variant,
    reason = "a session holds a handful of drafts; both variants are full evidence"
)]
pub(crate) enum Origin {
    /// A new comment on the bound picture. A mark on the current edit names its
    /// snapshot only once that exact cut is published.
    Captured {
        proof: FrameProof,
        /// Settled into `proof.pixels` before the draft is kept on disk.
        #[serde(skip)]
        readback: Option<Readback>,
        snapshot: Option<SnapshotKey>,
        layers: Vec<Layer>,
        layer: Option<usize>,
    },
    /// A later revision keeps its comment's original anchor, frame proof, strokes
    /// and voice evidence unless the person changes them.
    Revises {
        anchor: FeedbackAnchor,
        frame_proof_ref: ObjectRef,
        strokes_ref: ObjectRef,
        /// The original strokes as drawn, to tell whether the person changed them.
        strokes: Vec<Stroke>,
        voice_ref: Option<VoiceRef>,
        transcript_revision: Option<ObjectRef>,
    },
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PendingWrite {
    pub(crate) envelope: MetadataEnvelope,
    pub(crate) update: MetadataUpdate,
    /// Why the last reply did not settle, if it did not.
    pub(crate) unknown: Option<String>,
    #[serde(skip)]
    _pins: Vec<StorePin>,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "state", content = "detail", rename_all = "snake_case")]
pub(crate) enum DraftState {
    Editing,
    Refused(String),
    /// The original command, retried exactly until its outcome is known.
    Pending(Box<PendingWrite>),
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Draft {
    pub(crate) comment: CommentId,
    pub(crate) origin: Origin,
    pub(crate) text: String,
    pub(crate) strokes: Vec<Stroke>,
    pub(crate) state: DraftState,
    /// Captured in this run from the picture the presenter bound. Frame IDs
    /// restart with each launch, so a restored draft is never drawn on or
    /// painted over by frame-ID match: a new stroke captures afresh.
    #[serde(skip)]
    pub(crate) live: bool,
}
impl Draft {
    pub(crate) fn pending(&self) -> bool {
        matches!(self.state, DraftState::Pending(_))
    }
    /// Whether the person has typed or drawn anything, or a write is in flight.
    pub(crate) fn holds_work(&self) -> bool {
        !matches!(self.state, DraftState::Editing)
            || !self.text.trim().is_empty()
            || self.strokes.iter().any(|stroke| !stroke.is_empty())
            || matches!(self.origin, Origin::Revises { .. })
    }
    /// Fold a finished pixel digest into the proof, so the kept draft carries it.
    fn settle_readback(&mut self) {
        if let Origin::Captured {
            proof, readback, ..
        } = &mut self.origin
            && let Some(readback) = readback.take()
        {
            proof.pixels = readback.finish().ok();
        }
    }
    /// Why the draft cannot be saved yet, if it cannot.
    pub(crate) fn waiting(&self) -> Option<&'static str> {
        match &self.origin {
            Origin::Captured {
                snapshot: None,
                proof:
                    FrameProof {
                        shown: Shown::Cut { .. },
                        ..
                    },
                ..
            } => Some("Waits until this exact edit is published as a version."),
            _ => None,
        }
    }
    pub(crate) fn frame(&self) -> i64 {
        match &self.origin {
            Origin::Captured { proof, .. } => proof.frame,
            Origin::Revises { anchor, .. } => anchor.frame,
        }
    }
}

/// Capture a draft from the bound image. A pending selection or seek never
/// relabels the picture on screen.
pub(crate) fn capture(view: &View<'_>) -> Result<Draft, String> {
    let cell = view.cell;
    let (shown, document) = match (&cell.context, &view.manual) {
        (Some(context), _) => (
            Shown::Version {
                snapshot: context.snapshot.clone(),
            },
            context.inputs.document(),
        ),
        (None, Some((cut, document))) => (Shown::Cut { cut: cut.clone() }, *document),
        (None, None) => {
            return Err(
                "This picture's edit is not established yet. Wait for it to show, then mark it."
                    .into(),
            );
        }
    };
    let _ = (view.selection, view.requested);
    let raster = if document.resolution.0 > 0 && document.resolution.1 > 0 {
        [document.resolution.0, document.resolution.1]
    } else {
        view.texture
    };
    let readback = match view.pixels {
        Some((frame_id, texture)) if frame_id == cell.frame_id => Some(Readback::start(texture)?),
        _ => None,
    };
    let snapshot = match &shown {
        Shown::Version { snapshot } => Some(snapshot.clone()),
        Shown::Cut { .. } => None,
    };
    Ok(Draft {
        comment: CommentId::fresh().map_err(|error| error.to_string())?,
        origin: Origin::Captured {
            proof: FrameProof {
                version: 1,
                shown,
                frame: cell.at.0,
                fps: FrameRate {
                    numerator: document.fps.numerator(),
                    denominator: document.fps.denominator(),
                },
                raster,
                orientation: Orientation::Upright,
                bound: BoundIdentity {
                    frame_id: cell.frame_id,
                    epoch: cell.stamp.epoch,
                    seq: cell.stamp.seq,
                    stale: cell.stale,
                },
                pixels: None,
            },
            readback,
            snapshot,
            layers: layers_at(document, cell.at),
            layer: None,
        },
        text: String::new(),
        strokes: Vec::new(),
        state: DraftState::Editing,
        live: true,
    })
}

/// A metadata reply as the person sees it: durable, definitely refused, or unknown.
pub(crate) enum Reply {
    Durable(MetadataResult),
    Refused(String),
    Unknown(String),
}
fn local_reply(result: Result<ReceiptLookup, BundleError>) -> Reply {
    match result {
        Ok(ReceiptLookup::Committed(receipt) | ReceiptLookup::Retired(receipt)) => {
            match receipt.result {
                ReceiptResult::Metadata { result } => Reply::Durable(result),
                _ => Reply::Refused("The receipt is not a feedback receipt.".into()),
            }
        }
        Ok(ReceiptLookup::IdPayloadConflict) => {
            Reply::Refused("The retry changed the original command.".into())
        }
        Ok(_) => Reply::Unknown("The outcome is not settled yet.".into()),
        Err(
            error @ (BundleError::DurabilityUnknown
            | BundleError::DurabilityUnknownAt(_)
            | BundleError::Unsettled),
        ) => Reply::Unknown(error.to_string()),
        Err(error) => Reply::Refused(error.to_string()),
    }
}

/// The owner a feedback write reaches: the in-process writer or the private host.
pub(crate) trait FeedbackPort {
    /// Seal one bounded evidence object. In-process owners return its pin.
    fn admit(
        &mut self,
        kind: FeedbackEvidenceKind,
        bytes: &[u8],
    ) -> Result<(ObjectRef, Option<StorePin>), String>;
    /// The next envelope of this attachment's issuer, and the root generation.
    fn envelope(
        &mut self,
        issuer: &mut Option<IssuerId>,
    ) -> Result<(MetadataEnvelope, u64), String>;
    fn metadata(&mut self, envelope: &MetadataEnvelope, update: &MetadataUpdate) -> Reply;
    fn index(&self) -> Result<FeedbackIndex, String>;
    fn revision(&self, reference: &ObjectRef) -> Result<FeedbackRevision, String>;
    fn evidence(&self, reference: &ObjectRef) -> Result<Vec<u8>, String>;
    fn batch(&self, batch: &FeedbackBatchId) -> Result<FeedbackBatch, String>;
    /// Published versions and the exact cut each one acknowledges.
    fn published(&self) -> Result<Vec<(SnapshotKey, AcceptedCut)>, String>;
}
fn text(error: impl std::fmt::Display) -> String {
    error.to_string()
}
fn published_cuts(
    store: &ProjectStore,
    descriptors: impl IntoIterator<Item = SnapshotDescriptor>,
) -> Result<Vec<(SnapshotKey, AcceptedCut)>, String> {
    descriptors
        .into_iter()
        .map(|descriptor| {
            let checkpoint = store
                .read_record::<BranchCheckpoint>(&descriptor.checkpoint_ref)
                .map_err(text)?;
            let history = store
                .read_record::<HistorySnapshot>(&checkpoint.history_ref)
                .map_err(text)?;
            Ok((
                descriptor.key,
                AcceptedCut::of(&checkpoint.branch_id, &history)?,
            ))
        })
        .collect()
}
impl FeedbackPort for ProjectBundle {
    fn admit(
        &mut self,
        kind: FeedbackEvidenceKind,
        bytes: &[u8],
    ) -> Result<(ObjectRef, Option<StorePin>), String> {
        let sealed = self.admit_feedback_evidence(kind, bytes).map_err(text)?;
        Ok((sealed.reference.clone(), Some(sealed.into_pin())))
    }
    fn envelope(
        &mut self,
        issuer: &mut Option<IssuerId>,
    ) -> Result<(MetadataEnvelope, u64), String> {
        let id = match issuer {
            Some(id) => id.clone(),
            None => issuer
                .insert(self.issue_metadata_issuer().map_err(text)?)
                .clone(),
        };
        let sequence = self.metadata_issuer_sequence(&id).map_err(text)?;
        let generation = self
            .root()
            .ok_or("The project has no saved root yet.")?
            .generation;
        Ok((
            self.metadata_envelope(CommandId {
                issuer: id,
                sequence,
            }),
            generation,
        ))
    }
    fn metadata(&mut self, envelope: &MetadataEnvelope, update: &MetadataUpdate) -> Reply {
        let result = self.metadata_command(envelope, MetadataScope::Feedback, update);
        if matches!(result, Err(BundleError::Unsettled)) && self.reconcile_metadata().is_ok() {
            return local_reply(self.metadata_command(envelope, MetadataScope::Feedback, update));
        }
        local_reply(result)
    }
    fn index(&self) -> Result<FeedbackIndex, String> {
        Ok((*self.feedback_index().map_err(text)?).clone())
    }
    fn revision(&self, reference: &ObjectRef) -> Result<FeedbackRevision, String> {
        Ok(self.read_feedback(reference).map_err(text)?.revision)
    }
    fn evidence(&self, reference: &ObjectRef) -> Result<Vec<u8>, String> {
        Ok((*self.store().read_evidence(reference).map_err(text)?).clone())
    }
    fn batch(&self, batch: &FeedbackBatchId) -> Result<FeedbackBatch, String> {
        Ok(self.read_submitted_batch(batch).map_err(text)?.batch)
    }
    fn published(&self) -> Result<Vec<(SnapshotKey, AcceptedCut)>, String> {
        let index = self.version_index().map_err(text)?;
        published_cuts(self.store(), index.snapshots.values().cloned())
    }
}
impl FeedbackPort for Ja1EditingSession {
    fn admit(
        &mut self,
        kind: FeedbackEvidenceKind,
        bytes: &[u8],
    ) -> Result<(ObjectRef, Option<StorePin>), String> {
        let sealed = self.admit_feedback_evidence(kind, bytes).map_err(text)?;
        Ok((sealed.reference.clone(), Some(sealed.into_pin())))
    }
    fn envelope(
        &mut self,
        issuer: &mut Option<IssuerId>,
    ) -> Result<(MetadataEnvelope, u64), String> {
        let id = match issuer {
            Some(id) => id.clone(),
            None => issuer
                .insert(self.issue_metadata_issuer().map_err(text)?)
                .clone(),
        };
        let sequence = self.metadata_issuer_sequence(&id).map_err(text)?;
        let envelope = self
            .metadata_envelope(CommandId {
                issuer: id,
                sequence,
            })
            .map_err(text)?;
        Ok((envelope, self.metadata_generation().map_err(text)?))
    }
    fn metadata(&mut self, envelope: &MetadataEnvelope, update: &MetadataUpdate) -> Reply {
        let result = self.metadata_command(envelope, MetadataScope::Feedback, update);
        if matches!(result, Err(BundleError::Unsettled)) && self.reconcile_metadata().is_ok() {
            return local_reply(self.metadata_command(envelope, MetadataScope::Feedback, update));
        }
        local_reply(result)
    }
    fn index(&self) -> Result<FeedbackIndex, String> {
        Ok((*self.feedback_index().map_err(text)?).clone())
    }
    fn revision(&self, reference: &ObjectRef) -> Result<FeedbackRevision, String> {
        Ok(self.read_feedback(reference).map_err(text)?.revision)
    }
    fn evidence(&self, reference: &ObjectRef) -> Result<Vec<u8>, String> {
        Ok((*self
            .store()
            .map_err(text)?
            .read_evidence(reference)
            .map_err(text)?)
        .clone())
    }
    fn batch(&self, batch: &FeedbackBatchId) -> Result<FeedbackBatch, String> {
        Ok(self.read_submitted_batch(batch).map_err(text)?.batch)
    }
    fn published(&self) -> Result<Vec<(SnapshotKey, AcceptedCut)>, String> {
        let descriptors = self
            .studio_versions()
            .map_err(text)?
            .iter()
            .map(|key| Ok(self.read_snapshot(key).map_err(text)?.descriptor))
            .collect::<Result<Vec<_>, String>>()?;
        published_cuts(self.store().map_err(text)?, descriptors)
    }
}

/// The host feedback adapter: writes and the private index go through the shared
/// authenticated `WriterHost`; immutable records are read from this owner's store.
pub(crate) struct HostFeedback<'a> {
    pub(crate) client: &'a kinewright_agent::writer_host::HostClient,
    pub(crate) reader: &'a Ja1EditingSession,
}
impl HostFeedback<'_> {
    fn request(
        &self,
        command: kinewright_agent::writer_host::HostCommand,
    ) -> Result<kinewright_agent::writer_host::HostReply, String> {
        let reply = self.client.request(command).map_err(text)?;
        match reply.error {
            Some(error) => Err(error.message),
            None => Ok(reply),
        }
    }
}
impl FeedbackPort for HostFeedback<'_> {
    fn admit(
        &mut self,
        kind: FeedbackEvidenceKind,
        bytes: &[u8],
    ) -> Result<(ObjectRef, Option<StorePin>), String> {
        let reply = self.request(
            kinewright_agent::writer_host::HostCommand::AdmitFeedbackEvidence {
                kind,
                bytes: bytes.to_vec(),
            },
        )?;
        Ok((
            reply
                .feedback_evidence
                .ok_or("The host returned no evidence reference.")?,
            None,
        ))
    }
    fn envelope(
        &mut self,
        issuer: &mut Option<IssuerId>,
    ) -> Result<(MetadataEnvelope, u64), String> {
        let grant = self
            .request(
                kinewright_agent::writer_host::HostCommand::IssueMetadataIssuer {
                    issuer: issuer.clone(),
                },
            )?
            .metadata_issuer
            .ok_or("The host issued no metadata envelope.")?;
        *issuer = Some(grant.envelope.id.issuer.clone());
        Ok((grant.envelope, grant.root_generation))
    }
    fn metadata(&mut self, envelope: &MetadataEnvelope, update: &MetadataUpdate) -> Reply {
        let reply =
            match self
                .client
                .request(kinewright_agent::writer_host::HostCommand::Metadata {
                    envelope: envelope.clone(),
                    update: update.clone(),
                }) {
                Ok(reply) => reply,
                Err(error) => return Reply::Unknown(error.to_string()),
            };
        if let Some(error) = reply.error {
            return if matches!(error.code.as_str(), "durability_unknown" | "unsettled") {
                Reply::Unknown(error.message)
            } else {
                Reply::Refused(error.message)
            };
        }
        match reply.metadata.map(|receipt| receipt.result) {
            Some(ReceiptResult::Metadata { result }) => Reply::Durable(result),
            Some(_) => Reply::Refused("The receipt is not a feedback receipt.".into()),
            None => Reply::Unknown("The host returned no receipt.".into()),
        }
    }
    fn index(&self) -> Result<FeedbackIndex, String> {
        self.request(kinewright_agent::writer_host::HostCommand::ListFeedback)?
            .feedback_index
            .ok_or_else(|| "The host returned no feedback index.".into())
    }
    fn revision(&self, reference: &ObjectRef) -> Result<FeedbackRevision, String> {
        Ok((*self
            .reader
            .store()
            .map_err(text)?
            .read_record::<FeedbackRevision>(reference)
            .map_err(text)?)
        .clone())
    }
    fn evidence(&self, reference: &ObjectRef) -> Result<Vec<u8>, String> {
        Ok((*self
            .reader
            .store()
            .map_err(text)?
            .read_evidence(reference)
            .map_err(text)?)
        .clone())
    }
    fn batch(&self, batch: &FeedbackBatchId) -> Result<FeedbackBatch, String> {
        self.request(
            kinewright_agent::writer_host::HostCommand::ReadSubmittedBatch {
                batch: batch.clone(),
            },
        )?
        .submitted_batch
        .ok_or_else(|| "The host returned no batch.".into())
    }
    fn published(&self) -> Result<Vec<(SnapshotKey, AcceptedCut)>, String> {
        let versions = self
            .request(kinewright_agent::writer_host::HostCommand::ListVersions)?
            .versions;
        let descriptors = versions
            .into_iter()
            .map(|key| {
                self.request(kinewright_agent::writer_host::HostCommand::ReadSnapshot { key })?
                    .snapshot
                    .ok_or_else(|| "The host returned no snapshot.".to_owned())
            })
            .collect::<Result<Vec<_>, String>>()?;
        published_cuts(self.reader.store().map_err(text)?, descriptors)
    }
}

/// A saved comment's revision, as the Notes list shows it.
#[derive(Clone, Debug)]
pub(crate) struct SavedNote {
    pub(crate) comment: CommentId,
    pub(crate) reference: ObjectRef,
    pub(crate) revision: FeedbackRevision,
    pub(crate) text: String,
    pub(crate) strokes: Vec<Stroke>,
}
/// Strokes evidence as drawn: nested strokes, or the flat single-stroke form.
fn parse_strokes(bytes: &[u8]) -> Vec<Stroke> {
    serde_json::from_slice::<Vec<Stroke>>(bytes)
        .or_else(|_| serde_json::from_slice::<Stroke>(bytes).map(|flat| vec![flat]))
        .unwrap_or_default()
}
fn saved_note(
    port: &dyn FeedbackPort,
    comment: &CommentId,
    reference: &ObjectRef,
) -> Result<SavedNote, String> {
    let revision = port.revision(reference)?;
    Ok(SavedNote {
        comment: comment.clone(),
        reference: reference.clone(),
        text: String::from_utf8_lossy(&port.evidence(&revision.original_text_ref)?).into_owned(),
        strokes: parse_strokes(&port.evidence(&revision.strokes_ref)?),
        revision,
    })
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Submission {
    pub(crate) batch: FeedbackBatch,
    pub(crate) write: PendingWrite,
}
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    Saved(ObjectRef),
    Submitted(FeedbackBatchId),
    /// Kept as a draft: the exact edit is not published, or the project is unsaved.
    Waiting(String),
    /// Definitely not written. The draft or selection is intact.
    Refused(String),
    /// The original command is retained for an exact retry.
    Unknown(String),
}

/// What a project's feedback keeps on this computer between sessions: unsaved
/// drafts, every in-flight command exactly as sent, and the Revise cut
/// selection. The file names and carries the project's durable identity, and
/// nothing in it is merged or retried for any other project (S3 C3 fix 2).
#[derive(Serialize)]
struct KeptRef<'a> {
    version: u32,
    project: &'a ProjectId,
    drafts: &'a [Draft],
    selection: &'a [ObjectRef],
    submission: Option<&'a Submission>,
    detached: &'a [Detached],
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Kept {
    version: u32,
    project: ProjectId,
    drafts: Vec<Draft>,
    selection: Vec<ObjectRef>,
    submission: Option<Submission>,
    detached: Vec<Detached>,
}
const KEPT_VERSION: u32 = 2;
const RESTARTED: &str = "Kinewright closed before this outcome was known";
const OPEN_ELSEWHERE: &str = "This project's notes are open in another window";

/// A sent command whose draft no longer matches it. It is retried exactly until
/// its outcome is known, and its draft stays a separate draft whatever the
/// receipt says.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Detached {
    pub(crate) comment: Option<CommentId>,
    pub(crate) write: PendingWrite,
}

/// Where this project's feedback is kept between sessions.
#[derive(Default)]
pub(crate) enum Keeping {
    /// The project has never been saved: closing discards notes.
    #[default]
    Unsaved,
    /// Kept in per-user state, in the file named by the project's identity.
    /// The lock makes this session the file's one live owner until the identity
    /// changes or the session drops.
    Kept {
        project: ProjectId,
        path: PathBuf,
        _owner: kinewright_project::FeedbackStateLock,
    },
    /// The project is saved, but its kept state cannot be read or has no
    /// persistent place. Nothing is sent and nothing is written over it.
    Failed {
        project: Option<ProjectId>,
        why: String,
    },
}
impl Keeping {
    pub(crate) fn project(&self) -> Option<&ProjectId> {
        match self {
            Self::Unsaved => None,
            Self::Kept { project, .. } => Some(project),
            Self::Failed { project, .. } => project.as_ref(),
        }
    }
}

/// Write `bytes` to `path` so that a crash leaves the old or the new file,
/// never a torn one. The folder and the file are private to this user.
fn write_atomically(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let directory = path.parent().unwrap_or(Path::new("."));
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(directory)?;
    let partial = path.with_extension("json.partial");
    {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut file = options.open(&partial)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    std::fs::rename(&partial, path)?;
    #[cfg(unix)]
    std::fs::File::open(directory)?.sync_all()?;
    Ok(())
}

/// The anchor a captured draft's save names, once its snapshot is known.
fn captured_anchor(
    proof: &FrameProof,
    snapshot: Option<&SnapshotKey>,
    layers: &[Layer],
    layer: Option<usize>,
) -> Option<FeedbackAnchor> {
    Some(FeedbackAnchor {
        snapshot: snapshot?.clone(),
        frame: proof.frame,
        fps: proof.fps.clone(),
        raster_size: proof.raster,
        orientation: proof.orientation.clone(),
        target: layer
            .and_then(|index| layers.get(index))
            .map(|layer| FeedbackTarget {
                local_clip_id: layer.clip,
                occurrence_id: None,
                local_frame: None,
            }),
        source_ref: None,
        script_beat_id: None,
    })
}

/// Whether `write` is exactly the save this draft's content sends: its comment,
/// the save kind, the text and stroke digests, and every evidence reference.
/// A kept file edited after the send fails this, so an old receipt never
/// clears content it did not carry.
fn command_matches(draft: &Draft, write: &PendingWrite) -> bool {
    let MetadataChange::SaveFeedback(revision) = &write.update.change else {
        return false;
    };
    let same = |reference: &ObjectRef, bytes: &[u8]| {
        reference.byte_length == bytes.len() as u64
            && reference.sha256 == kinewright_media::sha256_bytes(bytes)
    };
    let drawn = |reference: &ObjectRef| {
        strokes_bytes(&draft.strokes).is_ok_and(|bytes| same(reference, &bytes))
    };
    if revision.comment_id != draft.comment
        || !same(&revision.original_text_ref, draft.text.as_bytes())
    {
        return false;
    }
    match &draft.origin {
        Origin::Revises {
            anchor,
            frame_proof_ref,
            strokes_ref,
            strokes,
            voice_ref,
            transcript_revision,
        } => {
            (if *strokes == draft.strokes {
                revision.strokes_ref == *strokes_ref
            } else {
                drawn(&revision.strokes_ref)
            }) && revision.anchor == *anchor
                && revision.frame_proof_ref == *frame_proof_ref
                && revision.voice_ref == *voice_ref
                && revision.transcript_revision == *transcript_revision
        }
        Origin::Captured {
            proof,
            snapshot,
            layers,
            layer,
            ..
        } => {
            drawn(&revision.strokes_ref)
                && serde_json::to_vec(proof)
                    .is_ok_and(|bytes| same(&revision.frame_proof_ref, &bytes))
                && captured_anchor(proof, snapshot.as_ref(), layers, *layer)
                    .is_some_and(|anchor| anchor == revision.anchor)
                && revision.voice_ref.is_none()
                && revision.transcript_revision.is_none()
        }
    }
}
/// Whether a kept submission's batch is the batch its command sends.
fn submission_matches(submission: &Submission) -> bool {
    matches!(
        &submission.write.update.change,
        MetadataChange::SubmitBatch { batch, request }
            if *batch == submission.batch
                && request.batch_id == batch.batch_id
                && request.command_id == submission.write.envelope.id
    )
}

/// Studio feedback state for one project session.
#[derive(Default)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "independent flags: drawing mode, loaded, kept state changed, resume pending"
)]
pub(crate) struct Feedback {
    pub(crate) drafts: Vec<Draft>,
    pub(crate) active: Option<CommentId>,
    pub(crate) drawing: bool,
    /// The stroke being drawn: its draft and the bound frame it started on.
    /// Points over any other picture end it.
    pub(crate) gesture: Option<(CommentId, u64)>,
    /// Issued by this project's owner; reset when the project identity changes.
    issuer: Option<IssuerId>,
    /// The Revise cut selection: exact saved revisions, possibly across versions.
    pub(crate) selection: Vec<ObjectRef>,
    /// Selected revisions that are no longer their comment's newest.
    pub(crate) older: BTreeMap<ObjectRef, SavedNote>,
    pub(crate) submission: Option<Submission>,
    /// Sent commands that no longer match their drafts, retried to settle.
    pub(crate) detached: Vec<Detached>,
    pub(crate) saved: Vec<SavedNote>,
    pub(crate) batches: Vec<FeedbackBatchId>,
    pub(crate) message: Option<String>,
    pub(crate) loaded: bool,
    pub(crate) scene: Option<String>,
    /// Where this project's feedback is kept between sessions.
    pub(crate) keeping: Keeping,
    /// Why the last write of the kept state failed, if it did.
    pub(crate) persist_error: Option<String>,
    /// Kept state changed since it was last written.
    pub(crate) changed: bool,
    /// Restored commands wait for an exact retry.
    pub(crate) resume: bool,
    /// The draft whose text field takes focus next.
    pub(crate) focus: Option<CommentId>,
    /// The per-user state folder, replaced in tests.
    #[cfg(test)]
    pub(crate) directory_override: Option<PathBuf>,
}
impl Feedback {
    pub(crate) fn draft_mut(&mut self, comment: &CommentId) -> Option<&mut Draft> {
        self.drafts
            .iter_mut()
            .find(|draft| &draft.comment == comment)
    }
    /// Feedback that closing now would leave outside the project.
    pub(crate) fn unsaved(&self) -> bool {
        self.submission.is_some()
            || !self.detached.is_empty()
            || self.drafts.iter().any(Draft::holds_work)
    }
    /// The close and quit warning for unsaved feedback, or empty.
    pub(crate) fn close_text(&self) -> String {
        if !self.unsaved() {
            return String::new();
        }
        let drafts = self
            .drafts
            .iter()
            .filter(|draft| draft.holds_work())
            .count();
        let notes = match drafts {
            0 => String::new(),
            1 => "1 note is not saved".to_owned(),
            n => format!("{n} notes are not saved"),
        };
        let what = match (notes.is_empty(), self.submission.is_some()) {
            (false, true) => format!("{notes}, and a Revise cut request is not confirmed."),
            (false, false) => format!("{notes}."),
            (true, true) => "A Revise cut request is not confirmed.".to_owned(),
            (true, false) => "An earlier note send is not confirmed.".to_owned(),
        };
        match (&self.keeping, &self.persist_error) {
            (Keeping::Kept { .. }, None) => format!(
                "{what} They are kept on this computer: reopening this project shows them again and retries any unconfirmed request exactly. Save them in Notes to add them to the project."
            ),
            (Keeping::Kept { .. }, Some(why)) | (Keeping::Failed { why, .. }, _) => {
                format!(
                    "{what} They could not be kept on this computer ({why}). Closing discards them."
                )
            }
            (Keeping::Unsaved, _) => format!("{what} Closing discards them."),
        }
    }
    /// The per-user folder kept state goes in, or why there is none.
    #[cfg_attr(
        not(test),
        allow(clippy::unused_self, reason = "tests replace the folder through self")
    )]
    fn state_directory(&self) -> Result<PathBuf, String> {
        #[cfg(test)]
        if let Some(directory) = &self.directory_override {
            return Ok(directory.clone());
        }
        kinewright_project::feedback_state_directory()
    }
    /// Write the kept state now. Settles pixel digests first, so the kept proofs
    /// carry them. An unsaved project keeps nothing; a failed one writes nothing.
    pub(crate) fn persist(&mut self) -> Result<(), String> {
        let (project, path) = match &self.keeping {
            Keeping::Unsaved => {
                self.changed = false;
                return Ok(());
            }
            Keeping::Failed { why, .. } => return Err(why.clone()),
            Keeping::Kept { project, path, .. } => (project.clone(), path.clone()),
        };
        self.drafts.iter_mut().for_each(Draft::settle_readback);
        let result = if self.drafts.is_empty()
            && self.selection.is_empty()
            && self.submission.is_none()
            && self.detached.is_empty()
        {
            match std::fs::remove_file(&path) {
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
                _ => Ok(()),
            }
        } else {
            serde_json::to_vec(&KeptRef {
                version: KEPT_VERSION,
                project: &project,
                drafts: &self.drafts,
                selection: &self.selection,
                submission: self.submission.as_ref(),
                detached: &self.detached,
            })
            .map_err(std::io::Error::other)
            .and_then(|bytes| write_atomically(&path, &bytes))
        };
        match result {
            Ok(()) => {
                self.changed = false;
                self.persist_error = None;
                Ok(())
            }
            Err(error) => {
                let why = error.to_string();
                self.persist_error = Some(why.clone());
                Err(why)
            }
        }
    }
    /// The exact command about to be sent is durably kept, or why it is not.
    /// Only a durable file for this project counts: an unsaved project or one
    /// whose kept state failed to load refuses the send.
    fn retain_for_dispatch(&mut self) -> Result<(), String> {
        match &self.keeping {
            Keeping::Unsaved => Err("the project is not saved yet".to_owned()),
            Keeping::Failed { why, .. } => Err(why.clone()),
            Keeping::Kept { .. } => self.persist(),
        }
    }
    /// Keep feedback for `project` from now on, and bring back what was kept for
    /// it. Leaving a kept project first writes its state, which stays in its own
    /// file with its in-flight commands; unsent drafts and the selection come
    /// along (a first save or Save As). A file that cannot be read is left
    /// untouched, and a file naming another project is set aside intact.
    pub(crate) fn open(
        &mut self,
        project: Result<ProjectId, String>,
        directory: Result<PathBuf, String>,
    ) -> Result<(), String> {
        if matches!(self.keeping, Keeping::Kept { .. }) {
            let left = self.persist();
            self.leave(left.is_ok());
        }
        // The old file's lock is released only after its last write.
        self.keeping = Keeping::Unsaved;
        self.issuer = None;
        self.saved.clear();
        self.older.clear();
        self.batches.clear();
        self.loaded = false;
        self.persist_error = None;
        self.gesture = None;
        let project = match project {
            Ok(project) => project,
            Err(why) => {
                let why = format!("this project's identity cannot be read ({why})");
                self.keeping = Keeping::Failed {
                    project: None,
                    why: why.clone(),
                };
                return Err(why);
            }
        };
        let fail = |feedback: &mut Self, why: String| {
            feedback.keeping = Keeping::Failed {
                project: Some(project.clone()),
                why: why.clone(),
            };
            Err(why)
        };
        let directory = match directory {
            Ok(directory) => directory,
            Err(why) => return fail(self, format!("no persistent place for notes: {why}")),
        };
        // One live owner per file, taken before it is read: a second copy of
        // this project never merges, sets aside or writes the first's notes.
        let owner = match kinewright_project::lock_feedback_state(&directory, project.as_str()) {
            Ok(owner) => owner,
            Err(kinewright_project::FeedbackStateRefusal::Held) => {
                return fail(self, OPEN_ELSEWHERE.to_owned());
            }
            Err(kinewright_project::FeedbackStateRefusal::Failed(why)) => {
                return fail(self, format!("kept notes cannot be locked: {why}"));
            }
        };
        let path = directory.join(format!("{}.json", project.as_str()));
        let kept = match std::fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice::<Kept>(&bytes) {
                Ok(kept) if kept.version == KEPT_VERSION => Some(kept),
                Ok(_) => return fail(self, "kept notes have an unknown version".to_owned()),
                Err(error) => return fail(self, format!("kept notes cannot be read: {error}")),
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return fail(self, format!("kept notes cannot be read: {error}")),
        };
        let kept = match kept {
            Some(kept) if kept.project != project => {
                // Not this project's: keep it whole, beside, and start fresh.
                let aside = path.with_extension(format!(
                    "json.other-{}-{}",
                    kept.project.as_str(),
                    std::process::id()
                ));
                if let Err(error) = std::fs::rename(&path, &aside) {
                    return fail(
                        self,
                        format!("kept notes name another project and cannot be set aside: {error}"),
                    );
                }
                None
            }
            kept => kept,
        };
        self.keeping = Keeping::Kept {
            project: project.clone(),
            path,
            _owner: owner,
        };
        self.changed |= self.unsaved();
        if let Some(kept) = kept {
            self.merge(&project, kept);
        }
        Ok(())
    }
    /// Leave a kept project: what is in flight stays in its file when that was
    /// written, and only unsent drafts and the selection carry on. When it could
    /// not be written, nothing is dropped: every draft carries as unsent, and an
    /// unconfirmed batch's notes go back into the selection.
    fn leave(&mut self, written: bool) {
        if written {
            self.drafts.retain(|draft| !draft.pending());
            self.submission = None;
            self.detached.clear();
        } else {
            for draft in &mut self.drafts {
                if draft.pending() {
                    draft.state = DraftState::Editing;
                }
            }
            if let Some(submission) = self.submission.take() {
                for reference in submission.batch.revision_refs {
                    if !self.selection.contains(&reference) {
                        self.selection.push(reference);
                    }
                }
            }
            self.detached.clear();
            self.message = Some(
                "The previous project's unconfirmed sends could not be kept; their notes are here as drafts."
                    .into(),
            );
        }
        if self
            .active
            .as_ref()
            .is_some_and(|active| !self.drafts.iter().any(|draft| &draft.comment == active))
        {
            self.active = None;
        }
    }
    /// Merge this project's kept state. A restored command is retried only
    /// when it is this project's and still exactly its draft's; otherwise its
    /// draft becomes a separate unsent draft and the command settles alone.
    fn merge(&mut self, project: &ProjectId, kept: Kept) {
        let ours = |write: &PendingWrite| write.envelope.project_id == *project;
        for mut draft in kept.drafts {
            if let DraftState::Pending(pending) = &mut draft.state {
                pending.unknown.get_or_insert_with(|| RESTARTED.to_owned());
            }
            let pending = std::mem::replace(&mut draft.state, DraftState::Editing);
            match pending {
                DraftState::Pending(write) if ours(&write) && command_matches(&draft, &write) => {
                    draft.state = DraftState::Pending(write);
                }
                DraftState::Pending(write) => {
                    if ours(&write) {
                        self.detached.push(Detached {
                            comment: Some(draft.comment.clone()),
                            write: *write,
                        });
                    }
                }
                state => draft.state = state,
            }
            if let Some(index) = self.drafts.iter().position(|d| d.comment == draft.comment) {
                // The same comment is already open here. Identical content is
                // one draft; different content is kept as two, and the one that
                // carries a command keeps the comment.
                let open = &mut self.drafts[index];
                let identical = open.text == draft.text && open.strokes == draft.strokes;
                if draft.pending() {
                    if identical {
                        self.drafts.remove(index);
                    } else if let Ok(fresh) = CommentId::fresh() {
                        open.comment = fresh;
                    }
                } else if identical || !draft.holds_work() {
                    continue;
                } else if let Ok(fresh) = CommentId::fresh() {
                    draft.comment = fresh;
                }
            }
            self.drafts.push(draft);
        }
        for reference in kept.selection {
            if !self.selection.contains(&reference) {
                self.selection.push(reference);
            }
        }
        for mut detached in kept.detached {
            if ours(&detached.write) {
                detached
                    .write
                    .unknown
                    .get_or_insert_with(|| RESTARTED.to_owned());
                self.detached.push(detached);
            }
        }
        if let Some(mut submission) = kept.submission.filter(|s| ours(&s.write)) {
            submission
                .write
                .unknown
                .get_or_insert_with(|| RESTARTED.to_owned());
            let sent = match &submission.write.update.change {
                MetadataChange::SubmitBatch { batch, request }
                    if request.batch_id == batch.batch_id
                        && request.command_id == submission.write.envelope.id =>
                {
                    Some(batch.clone())
                }
                _ => None,
            };
            if !submission_matches(&submission) {
                // The kept batch differs from the command: the command is what
                // was sent, and the kept batch's notes go back into the selection.
                for reference in &submission.batch.revision_refs {
                    if !self.selection.contains(reference) {
                        self.selection.push(reference.clone());
                    }
                }
            }
            match sent {
                Some(batch) if self.submission.is_none() => {
                    submission.batch = batch;
                    self.submission = Some(submission);
                }
                _ => self.detached.push(Detached {
                    comment: None,
                    write: submission.write,
                }),
            }
        }
        self.resume = self.submission.is_some()
            || !self.detached.is_empty()
            || self.drafts.iter().any(Draft::pending);
        self.changed = true;
    }
    /// Retry the detached commands `which` selects, exactly. A durable or
    /// refused reply settles one; its draft, if any, stays as it is. Returns
    /// whether none of them is still unknown.
    fn settle_detached(
        &mut self,
        port: &mut dyn FeedbackPort,
        which: impl Fn(&Detached) -> bool,
    ) -> bool {
        let mut settled = true;
        for mut detached in std::mem::take(&mut self.detached) {
            if !which(&detached) {
                self.detached.push(detached);
                continue;
            }
            self.changed = true;
            if let Reply::Unknown(why) =
                port.metadata(&detached.write.envelope, &detached.write.update)
            {
                detached.write.unknown = Some(why);
                self.detached.push(detached);
                settled = false;
            }
        }
        settled
    }
    /// Retry every restored command exactly: detached ones, saves, then the batch.
    pub(crate) fn resume(&mut self, port: &mut dyn FeedbackPort) {
        self.resume = false;
        self.settle_detached(port, |_| true);
        let pending = self
            .drafts
            .iter()
            .filter(|draft| draft.pending())
            .map(|draft| draft.comment.clone())
            .collect::<Vec<_>>();
        for comment in &pending {
            self.save(port, comment);
        }
        if self.submission.is_some() {
            self.submit(port);
        }
        if let Err(why) = self.refresh(&*port) {
            self.message = Some(why);
        }
        self.keep();
    }
    /// Start a revision of a saved comment from all of its evidence. Each field is
    /// kept unless the person changes it.
    pub(crate) fn revise(&mut self, note: &SavedNote) {
        if self
            .drafts
            .iter()
            .any(|draft| draft.comment == note.comment)
        {
            self.active = Some(note.comment.clone());
            return;
        }
        self.drafts.push(Draft {
            comment: note.comment.clone(),
            origin: Origin::Revises {
                anchor: note.revision.anchor.clone(),
                frame_proof_ref: note.revision.frame_proof_ref.clone(),
                strokes_ref: note.revision.strokes_ref.clone(),
                strokes: note.strokes.clone(),
                voice_ref: note.revision.voice_ref.clone(),
                transcript_revision: note.revision.transcript_revision.clone(),
            },
            text: note.text.clone(),
            strokes: note.strokes.clone(),
            state: DraftState::Editing,
            live: false,
        });
        self.active = Some(note.comment.clone());
        self.focus = Some(note.comment.clone());
        self.changed = true;
    }
    /// Name each current-edit mark's snapshot once its own cut is published.
    pub(crate) fn promote(&mut self, published: &[(SnapshotKey, AcceptedCut)]) {
        for draft in &mut self.drafts {
            if let Origin::Captured {
                proof:
                    FrameProof {
                        shown: Shown::Cut { cut },
                        ..
                    },
                snapshot: snapshot @ None,
                ..
            } = &mut draft.origin
                && let Some((key, _)) = published
                    .iter()
                    .find(|(_, published)| cut.acknowledged_by(published))
            {
                *snapshot = Some(key.clone());
                self.changed = true;
            }
        }
    }
    /// Reload the private saved list. Saved-but-unsubmitted notes never leave this port.
    pub(crate) fn refresh(&mut self, port: &dyn FeedbackPort) -> Result<(), String> {
        let index = port.index()?;
        let mut saved = Vec::new();
        for (comment, revisions) in &index.revisions {
            let Some(reference) = revisions.last() else {
                continue;
            };
            saved.push(saved_note(port, comment, reference)?);
        }
        saved.sort_by_key(|note| std::cmp::Reverse(note.revision.anchor.frame));
        let mut older = BTreeMap::new();
        for reference in &self.selection {
            if saved.iter().any(|note| &note.reference == reference) {
                continue;
            }
            if let Some((comment, _)) = index
                .revisions
                .iter()
                .find(|(_, revisions)| revisions.contains(reference))
            {
                older.insert(reference.clone(), saved_note(port, comment, reference)?);
            }
        }
        self.saved = saved;
        self.older = older;
        self.batches = index.batches.keys().cloned().collect();
        if self.drafts.iter().any(|draft| draft.waiting().is_some()) {
            self.promote(&port.published()?);
        }
        self.loaded = true;
        Ok(())
    }
    /// The Revise cut selection as the person sees it: each exact revision, and
    /// the newest revision of its comment when that is a different one.
    pub(crate) fn selection_rows(&self) -> Vec<(ObjectRef, Option<&SavedNote>, Option<ObjectRef>)> {
        self.selection
            .iter()
            .map(|reference| {
                let note = self
                    .saved
                    .iter()
                    .find(|note| &note.reference == reference)
                    .or_else(|| self.older.get(reference));
                let newer = note.and_then(|note| {
                    self.saved
                        .iter()
                        .find(|latest| {
                            latest.comment == note.comment && &latest.reference != reference
                        })
                        .map(|latest| latest.reference.clone())
                });
                (reference.clone(), note, newer)
            })
            .collect()
    }
    /// Save one draft as an immutable revision. Only a durable receipt that the
    /// index retains clears it; refusal or an unknown outcome keeps it.
    pub(crate) fn save(&mut self, port: &mut dyn FeedbackPort, comment: &CommentId) -> Outcome {
        let outcome = self.save_inner(port, comment);
        self.message = match &outcome {
            Outcome::Saved(_) => Some("Saved. Agents see it only after Revise cut.".into()),
            Outcome::Waiting(why) | Outcome::Refused(why) => Some(why.clone()),
            Outcome::Unknown(why) => Some(format!(
                "Not saved yet: the outcome is unknown ({why}). Retry keeps this exact note."
            )),
            Outcome::Submitted(_) => None,
        };
        self.keep();
        outcome
    }
    /// Write the kept state, and say so if it could not be written.
    fn keep(&mut self) {
        self.changed = true;
        if matches!(self.keeping, Keeping::Failed { .. }) {
            // Nothing is written over unreadable kept notes; open said why.
            self.changed = false;
        } else if let Err(why) = self.persist() {
            self.message = Some(format!(
                "Notes could not be kept on this computer ({why}). Closing discards unsaved notes."
            ));
        }
    }
    fn save_inner(&mut self, port: &mut dyn FeedbackPort, comment: &CommentId) -> Outcome {
        let comments = self
            .saved
            .iter()
            .filter(|note| &note.comment != comment)
            .count();
        let Some(pending) = self
            .drafts
            .iter()
            .find(|draft| &draft.comment == comment)
            .map(Draft::pending)
        else {
            return Outcome::Refused("This draft is gone.".into());
        };
        if !pending {
            // An earlier send of this comment that no longer matches its draft
            // settles first, so the two never race for the same revision.
            if !self.settle_detached(port, |detached| detached.comment.as_ref() == Some(comment)) {
                return Outcome::Unknown(
                    "An earlier send of this note is not confirmed yet. Retry checks it first."
                        .into(),
                );
            }
            let Some(draft) = self
                .drafts
                .iter_mut()
                .find(|draft| &draft.comment == comment)
            else {
                return Outcome::Refused("This draft is gone.".into());
            };
            let write = match prepare(port, &mut self.issuer, draft, comments) {
                Ok(write) => write,
                Err(outcome) => {
                    if let Outcome::Refused(why) = &outcome {
                        draft.state = DraftState::Refused(why.clone());
                    }
                    return outcome;
                }
            };
            draft.state = DraftState::Pending(Box::new(write));
            // The exact command is kept before it is sent: a restart retries it,
            // never a rebuilt one that could save the note twice.
            if let Err(why) = self.retain_for_dispatch() {
                if let Some(draft) = self.draft_mut(comment) {
                    draft.state = DraftState::Editing;
                }
                return Outcome::Refused(format!(
                    "Not sent: this note could not be kept on this computer first ({why})."
                ));
            }
        }
        let Some(Draft {
            state: DraftState::Pending(pending),
            ..
        }) = self.draft_mut(comment)
        else {
            return Outcome::Refused("This draft is gone.".into());
        };
        let (envelope, update) = (pending.envelope.clone(), pending.update.clone());
        match port.metadata(&envelope, &update) {
            Reply::Durable(MetadataResult::Feedback(reference))
                if port.index().is_ok_and(|index| {
                    index
                        .revisions
                        .get(comment)
                        .is_some_and(|revisions| revisions.contains(&reference))
                }) =>
            {
                self.drafts.retain(|draft| &draft.comment != comment);
                if self.active.as_ref() == Some(comment) {
                    self.active = None;
                }
                let _ = self.refresh(&*port);
                Outcome::Saved(reference)
            }
            Reply::Durable(_) => {
                let why = "The saved receipt is not visible in the index yet.".to_owned();
                self.mark_unknown(comment, &why);
                Outcome::Unknown(why)
            }
            Reply::Unknown(why) => {
                self.mark_unknown(comment, &why);
                Outcome::Unknown(why)
            }
            Reply::Refused(why) => {
                if let Some(draft) = self.draft_mut(comment) {
                    draft.state = DraftState::Refused(why.clone());
                }
                Outcome::Refused(why)
            }
        }
    }
    fn mark_unknown(&mut self, comment: &CommentId, why: &str) {
        if let Some(Draft {
            state: DraftState::Pending(pending),
            ..
        }) = self.draft_mut(comment)
        {
            pending.unknown = Some(why.to_owned());
        }
    }
    /// Toggle one exact saved revision in the Revise cut selection.
    pub(crate) fn toggle(&mut self, reference: &ObjectRef) {
        if let Some(index) = self.selection.iter().position(|r| r == reference) {
            self.selection.remove(index);
        } else {
            self.selection.push(reference.clone());
        }
        self.changed = true;
    }
    /// Put `newer` where `older` was in the selection, once.
    pub(crate) fn replace(&mut self, older: &ObjectRef, newer: &ObjectRef) {
        let Some(index) = self.selection.iter().position(|r| r == older) else {
            return;
        };
        if self.selection.contains(newer) {
            self.selection.remove(index);
        } else {
            self.selection[index] = newer.clone();
        }
        self.changed = true;
    }
    /// Load a submitted batch's exact revisions into the selection. The batch itself
    /// stays immutable; submitting again creates a new batch.
    pub(crate) fn reopen(
        &mut self,
        port: &dyn FeedbackPort,
        batch: &FeedbackBatchId,
    ) -> Result<(), String> {
        self.selection = port.batch(batch)?.revision_refs;
        self.changed = true;
        self.refresh(port)
    }
    /// Durably create one immutable batch of the selected exact revisions. An unknown
    /// outcome retries the original batch, whatever the selection became since.
    pub(crate) fn submit(&mut self, port: &mut dyn FeedbackPort) -> Outcome {
        let outcome = self.submit_inner(port);
        self.message = match &outcome {
            Outcome::Submitted(_) => Some("Submitted · waiting for a run".into()),
            Outcome::Waiting(why) | Outcome::Refused(why) => Some(why.clone()),
            Outcome::Unknown(why) => Some(format!(
                "Not submitted yet: the outcome is unknown ({why}). Retry sends this exact batch."
            )),
            Outcome::Saved(_) => None,
        };
        self.keep();
        outcome
    }
    fn submit_inner(&mut self, port: &mut dyn FeedbackPort) -> Outcome {
        if self.submission.is_none() {
            if self.selection.is_empty() {
                return Outcome::Refused("Choose saved notes for Revise cut first.".into());
            }
            if !self.settle_detached(port, |detached| detached.comment.is_none()) {
                return Outcome::Unknown(
                    "An earlier Revise cut request is not confirmed yet. Retry checks it first."
                        .into(),
                );
            }
            let (envelope, generation) = match port.envelope(&mut self.issuer) {
                Ok(issued) => issued,
                Err(why) => return Outcome::Refused(why),
            };
            let Some(head) = envelope.expected_head.clone() else {
                return Outcome::Waiting(
                    "Save the project before asking for a revised cut.".into(),
                );
            };
            let batch = match FeedbackBatchId::fresh() {
                Ok(batch_id) => FeedbackBatch {
                    batch_id,
                    revision_refs: self.selection.clone(),
                },
                Err(error) => return Outcome::Refused(error.to_string()),
            };
            let update = MetadataUpdate {
                expected_root_generation: generation,
                change: MetadataChange::SubmitBatch {
                    batch: batch.clone(),
                    request: ReviseRequest {
                        command_id: envelope.id.clone(),
                        batch_id: batch.batch_id.clone(),
                        expected_candidate_head: head,
                    },
                },
            };
            self.submission = Some(Submission {
                batch,
                write: PendingWrite {
                    envelope,
                    update,
                    unknown: None,
                    _pins: Vec::new(),
                },
            });
            // Kept before it is sent, so a restart resends this exact batch.
            if let Err(why) = self.retain_for_dispatch() {
                self.submission = None;
                return Outcome::Refused(format!(
                    "Not sent: this request could not be kept on this computer first ({why})."
                ));
            }
        }
        let Some(submission) = &mut self.submission else {
            return Outcome::Refused("No batch is pending.".into());
        };
        match port.metadata(&submission.write.envelope, &submission.write.update) {
            Reply::Durable(MetadataResult::Batch(id))
                if id == submission.batch.batch_id
                    && port
                        .index()
                        .is_ok_and(|index| index.batches.contains_key(&id)) =>
            {
                let submitted = submission.batch.revision_refs.clone();
                self.submission = None;
                self.selection
                    .retain(|reference| !submitted.contains(reference));
                let _ = self.refresh(&*port);
                Outcome::Submitted(id)
            }
            Reply::Durable(_) => {
                let why = "The batch receipt is not visible in the index yet.".to_owned();
                submission.write.unknown = Some(why.clone());
                Outcome::Unknown(why)
            }
            Reply::Unknown(why) => {
                submission.write.unknown = Some(why.clone());
                Outcome::Unknown(why)
            }
            Reply::Refused(why) => {
                // Definitely not written: the exact revisions go back into the
                // selection, ahead of anything chosen since.
                let mut kept = submission.batch.revision_refs.clone();
                self.submission = None;
                for reference in self.selection.drain(..) {
                    if !kept.contains(&reference) {
                        kept.push(reference);
                    }
                }
                self.selection = kept;
                Outcome::Refused(why)
            }
        }
    }
}

/// Seal a draft's evidence and build its exact command. Ingress caps refuse with
/// the draft intact.
#[allow(
    clippy::too_many_lines,
    reason = "one ingress path: caps, evidence admission, then the exact command"
)]
fn prepare(
    port: &mut dyn FeedbackPort,
    issuer: &mut Option<IssuerId>,
    draft: &mut Draft,
    comments: usize,
) -> Result<PendingWrite, Outcome> {
    let limits = StorageLimits::default();
    if draft.text.len() as u64 > limits.comment_text_bytes {
        return Err(Outcome::Refused(format!(
            "This note is longer than {} KiB. Shorten it to save.",
            limits.comment_text_bytes >> 10
        )));
    }
    if points(&draft.strokes) as u64 > limits.comment_points {
        return Err(Outcome::Refused(format!(
            "This drawing has more than {} points. Remove a stroke to save.",
            limits.comment_points
        )));
    }
    let index = port.index().map_err(Outcome::Refused)?;
    let previous = index.revisions.get(&draft.comment);
    if previous.is_none() && comments.max(index.revisions.len()) as u64 >= limits.comments {
        return Err(Outcome::Refused(format!(
            "This project already has {} notes, the most it can hold.",
            limits.comments
        )));
    }
    if let Some(why) = draft.waiting() {
        return Err(Outcome::Waiting(why.into()));
    }
    draft.settle_readback();
    // A revision keeps its original strokes evidence unless the person changed them.
    let redrawn = match &draft.origin {
        Origin::Revises { strokes, .. } => *strokes != draft.strokes,
        Origin::Captured { .. } => true,
    };
    let strokes = redrawn
        .then(|| strokes_bytes(&draft.strokes))
        .transpose()
        .map_err(Outcome::Refused)?;
    let mut pins = Vec::new();
    let mut admit = |kind, bytes: &[u8]| {
        let (reference, pin) = port.admit(kind, bytes).map_err(Outcome::Refused)?;
        pins.extend(pin);
        Ok::<_, Outcome>(reference)
    };
    let original_text_ref = admit(FeedbackEvidenceKind::Text, draft.text.as_bytes())?;
    let drawn = strokes
        .map(|bytes| admit(FeedbackEvidenceKind::Strokes, &bytes))
        .transpose()?;
    let (anchor, frame_proof_ref, strokes_ref, voice_ref, transcript_revision) = match &draft.origin
    {
        Origin::Revises {
            anchor,
            frame_proof_ref,
            strokes_ref,
            voice_ref,
            transcript_revision,
            ..
        } => (
            anchor.clone(),
            frame_proof_ref.clone(),
            drawn.unwrap_or_else(|| strokes_ref.clone()),
            voice_ref.clone(),
            transcript_revision.clone(),
        ),
        Origin::Captured {
            proof,
            snapshot,
            layers,
            layer,
            ..
        } => {
            let bytes = serde_json::to_vec(proof).map_err(|e| Outcome::Refused(e.to_string()))?;
            let reference = admit(FeedbackEvidenceKind::FrameProof, &bytes)?;
            (
                captured_anchor(proof, snapshot.as_ref(), layers, *layer).ok_or_else(|| {
                    Outcome::Waiting("This mark has no published version.".into())
                })?,
                reference,
                drawn.ok_or_else(|| Outcome::Refused("The drawing was not sealed.".into()))?,
                None,
                None,
            )
        }
    };
    let index = port.index().map_err(Outcome::Refused)?;
    let previous = index.revisions.get(&draft.comment);
    let revision = FeedbackRevision {
        comment_id: draft.comment.clone(),
        revision: previous.map_or(1, |revisions| revisions.len() as u64 + 1),
        predecessor: previous.and_then(|revisions| revisions.last()).cloned(),
        anchor,
        original_text_ref,
        strokes_ref,
        frame_proof_ref,
        voice_ref,
        transcript_revision,
    };
    let (envelope, generation) = port.envelope(issuer).map_err(Outcome::Refused)?;
    Ok(PendingWrite {
        envelope,
        update: MetadataUpdate {
            expected_root_generation: generation,
            change: MetadataChange::SaveFeedback(revision),
        },
        unknown: None,
        _pins: pins,
    })
}

enum NoteAction {
    Save(CommentId),
    Discard(CommentId),
    Toggle(ObjectRef),
    Replace(ObjectRef, ObjectRef),
    Revise(Box<SavedNote>),
    Reopen(FeedbackBatchId),
    Submit,
    FirstSave,
    NewNote,
}

impl KinewrightApp {
    /// Run `act` against this project's feedback owner: the shared host when one is
    /// attached, otherwise the in-process session. Unsaved projects take S2's
    /// first-save flow before any feedback is durable.
    pub(crate) fn with_feedback<R>(
        &mut self,
        act: impl FnOnce(&mut Feedback, &mut dyn FeedbackPort) -> R,
    ) -> Result<R, String> {
        let session = self.focused_mut();
        let feedback = &mut session.studio.feedback;
        let attachment = session
            .ja1
            .as_mut()
            .ok_or("Studio is busy saving or opening. Try again in a moment.")?;
        if attachment.project_path().is_none() {
            return Err("Save the project first. Notes become durable with it.".into());
        }
        if let Some(client) = attachment.host_client() {
            let mut port = HostFeedback {
                client: &client,
                reader: &attachment.owner,
            };
            Ok(act(feedback, &mut port))
        } else {
            Ok(act(feedback, &mut attachment.owner))
        }
    }
    /// Once per Studio frame: bring back this project's kept drafts, retry their
    /// in-flight commands exactly, and keep what changed.
    pub(crate) fn feedback_frame(&mut self) {
        // Kept notes follow the project's durable identity, not its path: a
        // rename or move finds them, and Save As (a new identity) starts its own.
        let identity = self
            .focused()
            .ja1
            .as_ref()
            .filter(|attachment| attachment.project_path().is_some())
            .map(|attachment| attachment.owner.project_id().map_err(text));
        let feedback = &mut self.focused_mut().studio.feedback;
        let reopen = match (&identity, &feedback.keeping) {
            (None, _) | (Some(Err(_)), Keeping::Failed { project: None, .. }) => false,
            (Some(Ok(project)), keeping) => keeping.project() != Some(project),
            (Some(Err(_)), _) => true,
        };
        if let Some(identity) = identity.filter(|_| reopen) {
            let directory = feedback.state_directory();
            if let Err(why) = feedback.open(identity, directory) {
                feedback.message = Some(format!(
                    "Notes are not sent until they can be kept on this computer ({why}). Kept notes are left as they were."
                ));
            }
        }
        if feedback.resume {
            let resumed = self.with_feedback(|feedback, port| feedback.resume(port));
            if let Err(why) = resumed {
                let feedback = &mut self.focused_mut().studio.feedback;
                feedback.resume = false;
                feedback.message = Some(why);
            }
        }
        let feedback = &mut self.focused_mut().studio.feedback;
        if feedback.changed && feedback.gesture.is_none() {
            feedback.keep();
        }
    }
    /// Start a draft on the bound picture: its version or exact current-edit cut,
    /// its frame and its pixels, whatever the selection or playhead requests next.
    pub(crate) fn capture_feedback(&mut self) -> Result<CommentId, String> {
        let cell = self.presenter.bound().ok_or("Nothing is shown yet.")?;
        let session = self.focused();
        let studio = &session.studio;
        let history;
        let manual = match (&cell.context, &studio.manual_inputs, &session.ja1) {
            (None, Some(inputs), Some(attachment))
                if !cell.stale
                    && self
                        .presenter
                        .context_epoch()
                        .is_some_and(|epoch| cell.stamp.epoch >= epoch) =>
            {
                let branch = attachment.owner.manual_branch_id().map_err(text)?;
                history = session.core.export_history().map_err(text)?;
                // Every change before the export has its event queued by now. An
                // unprocessed one means the picture may not show this history yet.
                (session.core_events.is_empty() && *history.document == *inputs.document())
                    .then(|| AcceptedCut::of(&branch, &history))
                    .transpose()?
                    .map(|cut| (cut, inputs.document()))
            }
            _ => None,
        };
        let texture = self.program_picture().map_or([0, 0], |(_, size)| {
            #[allow(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "texture sides are whole pixel counts"
            )]
            [size.x as u32, size.y as u32]
        });
        let draft = capture(&View {
            cell: &cell,
            pixels: studio
                .cpu_frame
                .as_ref()
                .zip(studio.cpu_frame_id)
                .map(|(frame, id)| (id, frame)),
            selection: studio
                .selection
                .as_ref()
                .map(|context| &context.snapshot.descriptor.key),
            requested: self.program_position(),
            manual,
            texture,
        })?;
        let comment = draft.comment.clone();
        let feedback = &mut self.focused_mut().studio.feedback;
        feedback.drafts.push(draft);
        feedback.active = Some(comment.clone());
        feedback.changed = true;
        Ok(comment)
    }
    /// New note: a typed draft on the bound picture, with its text field focused.
    pub(crate) fn new_feedback_note(&mut self) -> Result<CommentId, String> {
        self.focused_mut().studio.tab = crate::studio::Tab::Notes;
        let comment = self.capture_feedback();
        let feedback = &mut self.focused_mut().studio.feedback;
        match &comment {
            Ok(comment) => feedback.focus = Some(comment.clone()),
            Err(why) => feedback.message = Some(why.clone()),
        }
        comment
    }
    /// A stroke starts on the bound picture: the active draft if it was captured
    /// from this same picture, otherwise a new capture. A failed capture starts no
    /// stroke, so later points go nowhere.
    pub(crate) fn feedback_stroke_start(&mut self) {
        self.focused_mut().studio.feedback.gesture = None;
        let current = self.presenter.bound().map(|cell| cell.frame_id);
        let reuse = {
            let feedback = &self.focused().studio.feedback;
            feedback
                .active
                .as_ref()
                .and_then(|active| {
                    feedback.drafts.iter().find(|draft| {
                        &draft.comment == active
                            && draft.live
                            && !draft.pending()
                            && matches!(&draft.origin, Origin::Captured { proof, .. } if Some(proof.bound.frame_id) == current)
                    })
                })
                .map(|draft| draft.comment.clone())
        };
        let comment = match reuse {
            Some(comment) => Ok(comment),
            None => self.capture_feedback(),
        };
        let feedback = &mut self.focused_mut().studio.feedback;
        match comment {
            Ok(comment) => {
                if let Some(draft) = feedback.draft_mut(&comment)
                    && let Origin::Captured { proof, .. } = &draft.origin
                {
                    let frame = proof.bound.frame_id;
                    draft.strokes.push(Vec::new());
                    feedback.gesture = Some((comment.clone(), frame));
                }
                feedback.active = Some(comment);
                feedback.changed = true;
            }
            Err(why) => feedback.message = Some(why),
        }
        self.focused_mut().studio.tab = crate::studio::Tab::Notes;
    }
    /// Admit one point of the stroke in progress, only while the presenter still
    /// shows the picture the stroke started on. Returns whether it was admitted.
    pub(crate) fn feedback_stroke_point(&mut self, point: [f32; 2]) -> bool {
        let shown = self.presenter.bound().map(|cell| cell.frame_id);
        let limit = StorageLimits::default().comment_points;
        let feedback = &mut self.focused_mut().studio.feedback;
        let Some((comment, frame)) = feedback.gesture.clone() else {
            return false;
        };
        let Some(draft) = feedback
            .draft_mut(&comment)
            .filter(|draft| shown == Some(frame) && !draft.pending())
        else {
            feedback.gesture = None;
            feedback.changed = true;
            feedback.message = Some(
                "The picture changed, so this stroke ends here. Pause playback to draw.".into(),
            );
            return false;
        };
        if (points(&draft.strokes) as u64) >= limit {
            feedback.message = Some(format!(
                "This drawing reached {limit} points. Save it or start another note."
            ));
            return false;
        }
        if let Some(stroke) = draft.strokes.last_mut()
            && stroke.last() != Some(&point)
        {
            stroke.push(point);
        }
        true
    }
    pub(crate) fn feedback_stroke_end(&mut self) {
        let feedback = &mut self.focused_mut().studio.feedback;
        if feedback.gesture.take().is_some() {
            feedback.changed = true;
        }
    }
    fn feedback_message(&mut self, result: Result<Outcome, String>) {
        if let Err(why) = result {
            self.focused_mut().studio.feedback.message = Some(why);
        }
    }
    /// Draw on the displayed image and paint the active draft's strokes.
    pub(crate) fn feedback_overlay(&mut self, ui: &mut egui::Ui, area: Rect, texture: Vec2) {
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "texture sides are whole pixel counts"
        )]
        let image = DisplayedImage::fit(
            area,
            [texture.x as u32, texture.y as u32],
            Orientation::Upright,
        );
        let bound = self.presenter.bound();
        if let Some(scene) = self.focused().studio.feedback.scene.clone()
            && bound.as_ref().is_some_and(|cell| !cell.stale)
        {
            self.focused_mut().studio.feedback.scene = None;
            self.stage_feedback_scene(&scene);
        }
        if self.focused().studio.feedback.drawing {
            let response = ui.interact(
                image.rect,
                ui.id().with("studio-feedback-draw"),
                egui::Sense::drag(),
            );
            if response.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
            }
            if response.drag_started() {
                self.feedback_stroke_start();
            }
            if response.dragged()
                && let Some(point) = response
                    .interact_pointer_pos()
                    .and_then(|at| image.normalise(at))
            {
                self.feedback_stroke_point(point);
            }
            if response.drag_stopped() {
                self.feedback_stroke_end();
            }
        } else {
            self.feedback_stroke_end();
        }
        let feedback = &self.focused().studio.feedback;
        let Some(draft) = feedback.active.as_ref().and_then(|active| {
            feedback
                .drafts
                .iter()
                .find(|draft| &draft.comment == active)
        }) else {
            return;
        };
        let Origin::Captured { proof, .. } = &draft.origin else {
            return;
        };
        if !draft.live || bound.is_none_or(|cell| cell.frame_id != proof.bound.frame_id) {
            return;
        }
        let painter = ui.painter_at(image.rect);
        for stroke in &draft.strokes {
            let line = stroke
                .iter()
                .map(|point| image.screen(*point))
                .collect::<Vec<_>>();
            if line.len() == 1 {
                painter.circle_filled(line[0], 2.0, crate::studio::po0::YOU);
            } else {
                painter.add(egui::Shape::line(
                    line,
                    egui::Stroke::new(3.0, crate::studio::po0::YOU),
                ));
            }
        }
    }
    /// `KINEWRIGHT_SCREENSHOT_STUDIO_SCENE`: a draft being drawn, a new typed note,
    /// or the Revise cut selector.
    fn stage_feedback_scene(&mut self, scene: &str) {
        self.focused_mut().studio.tab = crate::studio::Tab::Notes;
        if scene == "new-note" {
            let _ = self.new_feedback_note();
            return;
        }
        let Ok(comment) = self.capture_feedback() else {
            return;
        };
        let feedback = &mut self.focused_mut().studio.feedback;
        if let Some(draft) = feedback.draft_mut(&comment) {
            draft.text = "Hold this shot a beat longer, and lift the shadows here.".into();
            let ring = (0..=40)
                .map(|step| {
                    let angle =
                        f32::from(u8::try_from(step).unwrap_or(0)) / 40.0 * std::f32::consts::TAU;
                    [0.62 + 0.12 * angle.cos(), 0.42 + 0.16 * angle.sin()]
                })
                .collect();
            draft.strokes.push(ring);
            draft
                .strokes
                .push(vec![[0.46, 0.66], [0.52, 0.6], [0.56, 0.55]]);
        }
        feedback.drawing = true;
        if scene != "batch" {
            return;
        }
        let saved = self.with_feedback(|feedback, port| {
            let first = feedback.save(port, &comment);
            let _ = feedback.refresh(&*port);
            first
        });
        if let Ok(second) = self.capture_feedback() {
            if let Some(draft) = self.focused_mut().studio.feedback.draft_mut(&second) {
                draft.text = "Trim the pause before the second line.".into();
            }
            let _ = self.with_feedback(|feedback, port| feedback.save(port, &second));
        }
        self.feedback_message(saved);
        // Select the first note's revision, then revise it: the selector shows the
        // older selected revision with its replace and remove controls.
        let first = self
            .focused()
            .studio
            .feedback
            .saved
            .iter()
            .find(|note| note.comment == comment)
            .cloned();
        let feedback = &mut self.focused_mut().studio.feedback;
        feedback.drawing = false;
        feedback.selection = feedback
            .saved
            .iter()
            .map(|note| note.reference.clone())
            .collect();
        if let Some(first) = first {
            feedback.revise(&first);
            if let Some(draft) = feedback.draft_mut(&comment) {
                draft.text = "Hold this shot two beats longer.".into();
            }
            let _ = self.with_feedback(|feedback, port| {
                feedback.save(port, &comment);
                feedback.refresh(&*port)
            });
        }
        self.focused_mut().studio.feedback.message = None;
    }
    /// The Notes tab: drafts, private saved notes, the Revise cut selection and batches.
    #[allow(clippy::too_many_lines, reason = "one immediate-mode panel")]
    pub(crate) fn studio_notes(&mut self, ui: &mut egui::Ui) {
        use crate::studio::po0;
        if !self.focused().studio.feedback.loaded && self.focused().ja1.is_some() {
            self.focused_mut().studio.feedback.loaded = true;
            let refreshed = self.with_feedback(|feedback, port| feedback.refresh(&*port));
            if let Ok(Err(why)) | Err(why) = refreshed {
                self.focused_mut().studio.feedback.message = Some(why);
            }
        }
        let mut action = None;
        ui.horizontal(|ui| {
            ui.label("Notes · newest first");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("Refresh").clicked() {
                    self.focused_mut().studio.feedback.loaded = false;
                }
                if ui
                    .small_button("New note")
                    .on_hover_text("Type a note on the picture shown now (Alt+M)")
                    .clicked()
                {
                    action = Some(NoteAction::NewNote);
                }
            });
        });
        if let Some(message) = &self.focused().studio.feedback.message {
            ui.colored_label(po0::TEXT_2, message);
        }
        let fps = self.program_document().fps;
        let pending = self.focused().studio.feedback.submission.is_some();
        let selected = self.focused().studio.feedback.selection.len();
        let unsaved = self
            .focused()
            .ja1
            .as_ref()
            .is_some_and(|attachment| attachment.project_path().is_none());
        let footer = |ui: &mut egui::Ui, action: &mut Option<NoteAction>| {
            let label = if pending {
                "Retry Revise cut".to_owned()
            } else {
                format!("Revise cut · {selected} selected")
            };
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(pending || selected > 0, egui::Button::new(label))
                    .clicked()
                {
                    *action = Some(NoteAction::Submit);
                }
                if unsaved && ui.button("Save project…").clicked() {
                    *action = Some(NoteAction::FirstSave);
                }
            });
        };
        // A short panel scrolls its footer with the notes, so a draft card is never
        // hidden behind it.
        let footer_height = 40.0;
        let compact = ui.available_height() < 4.0 * footer_height + 100.0;
        egui::ScrollArea::vertical()
            .id_salt("studio-notes")
            .auto_shrink([false, false])
            .max_height(if compact {
                ui.available_height()
            } else {
                (ui.available_height() - footer_height).max(60.0)
            })
            .show(ui, |ui| {
                let feedback = &mut self.focused_mut().studio.feedback;
                let rows = feedback
                    .selection_rows()
                    .into_iter()
                    .map(|(reference, note, newer)| (reference, note.cloned(), newer))
                    .collect::<Vec<_>>();
                let Feedback {
                    drafts,
                    saved,
                    selection,
                    batches,
                    active,
                    focus,
                    changed,
                    ..
                } = feedback;
                if drafts.is_empty() && saved.is_empty() {
                    ui.colored_label(
                        po0::TEXT_3,
                        "Draw on the picture, or press New note to type one. Notes stay private until Revise cut.",
                    );
                }
                for draft in drafts.iter_mut().rev() {
                    let status = match &draft.state {
                        DraftState::Pending(pending) if pending.unknown.is_some() => {
                            "Not saved yet · outcome unknown · Retry sends this exact note".to_owned()
                        }
                        DraftState::Pending(_) => "Saving…".to_owned(),
                        DraftState::Refused(why) => format!("Not saved · {why}"),
                        DraftState::Editing => draft
                            .waiting()
                            .unwrap_or("Draft · not saved")
                            .to_owned(),
                    };
                    let frame = draft.frame();
                    egui::Frame::group(ui.style()).show(ui, |ui| {
                        ui.small(format!(
                            "{} · {status}",
                            crate::timeline_ui::format_timecode(TimeCode(frame), fps)
                        ));
                        let pending = draft.pending();
                        let edit = ui.add_enabled(
                            !pending,
                            egui::TextEdit::multiline(&mut draft.text)
                                .id(egui::Id::new(("studio-note-text", draft.comment.as_str())))
                                .desired_rows(2)
                                .desired_width(f32::INFINITY)
                                .hint_text("What should change here?"),
                        );
                        if focus.as_ref() == Some(&draft.comment) {
                            edit.request_focus();
                            edit.scroll_to_me(Some(egui::Align::Center));
                            *focus = None;
                        }
                        if edit.gained_focus() {
                            *active = Some(draft.comment.clone());
                        }
                        *changed |= edit.changed();
                        ui.horizontal_wrapped(|ui| {
                            if let Origin::Captured { layers, layer, .. } = &mut draft.origin
                                && !layers.is_empty()
                                && !pending
                            {
                                let before = *layer;
                                let chosen = layer
                                    .and_then(|index| layers.get(index))
                                    .map_or("Applies to · not chosen", |layer| layer.label.as_str())
                                    .to_owned();
                                egui::ComboBox::from_id_salt(("studio-note-layer", draft.comment.as_str()))
                                    .selected_text(chosen)
                                    .show_ui(ui, |ui| {
                                        ui.selectable_value(layer, None, "Not chosen");
                                        for (index, choice) in layers.iter().enumerate() {
                                            ui.selectable_value(layer, Some(index), &choice.label);
                                        }
                                    });
                                *changed |= before != *layer;
                            }
                            let label = if pending { "Retry" } else { "Save note" };
                            if ui
                                .add_enabled(draft.waiting().is_none(), egui::Button::new(label))
                                .clicked()
                            {
                                action = Some(NoteAction::Save(draft.comment.clone()));
                            }
                            if !pending && ui.button("Discard").clicked() {
                                action = Some(NoteAction::Discard(draft.comment.clone()));
                            }
                        });
                    });
                }
                if !saved.is_empty() {
                    ui.add_space(8.0);
                    ui.label("Saved · private until Revise cut");
                }
                let short = |text: &str| {
                    let line = text.lines().next().unwrap_or("Drawing");
                    if line.chars().count() > 28 {
                        format!("{}…", line.chars().take(27).collect::<String>())
                    } else {
                        line.to_owned()
                    }
                };
                for note in saved.iter() {
                    ui.horizontal(|ui| {
                        let mut chosen = selection.contains(&note.reference);
                        if ui
                            .checkbox(
                                &mut chosen,
                                format!(
                                    "{} · {}",
                                    crate::timeline_ui::format_timecode(
                                        TimeCode(note.revision.anchor.frame),
                                        fps
                                    ),
                                    short(&note.text)
                                ),
                            )
                            .on_hover_text(format!(
                                "Revision {} on version {}",
                                note.revision.revision,
                                note.revision.anchor.snapshot.version.as_str()
                            ))
                            .changed()
                        {
                            action = Some(NoteAction::Toggle(note.reference.clone()));
                        }
                        if ui.small_button("Edit").clicked() {
                            action = Some(NoteAction::Revise(Box::new(note.clone())));
                        }
                    });
                }
                if !rows.is_empty() {
                    ui.add_space(8.0);
                    ui.label("Revise cut will send exactly these revisions");
                }
                for (reference, note, newer) in rows {
                    ui.horizontal_wrapped(|ui| {
                        let label = note.as_ref().map_or_else(
                            || "A saved revision".to_owned(),
                            |note| {
                                format!(
                                    "{} · {} · revision {}{}",
                                    crate::timeline_ui::format_timecode(
                                        TimeCode(note.revision.anchor.frame),
                                        fps
                                    ),
                                    short(&note.text),
                                    note.revision.revision,
                                    if newer.is_some() { " (older)" } else { "" }
                                )
                            },
                        );
                        ui.colored_label(po0::TEXT_2, label);
                        if let Some(newer) = newer
                            && ui.small_button("Use newest").clicked()
                        {
                            action = Some(NoteAction::Replace(reference.clone(), newer));
                        }
                        if ui.small_button("Remove").clicked() {
                            action = Some(NoteAction::Toggle(reference.clone()));
                        }
                    });
                }
                if !batches.is_empty() {
                    ui.add_space(8.0);
                    ui.label("Revise cut requests");
                }
                for batch in batches.iter() {
                    ui.horizontal(|ui| {
                        ui.colored_label(po0::TEXT_2, "Submitted · waiting for a run");
                        if ui.small_button("Reopen").clicked() {
                            action = Some(NoteAction::Reopen(batch.clone()));
                        }
                    });
                }
                if compact {
                    ui.add_space(4.0);
                    footer(ui, &mut action);
                }
            });
        if !compact {
            footer(ui, &mut action);
        }
        match action {
            Some(NoteAction::Save(comment)) => {
                let result = self.with_feedback(|feedback, port| feedback.save(port, &comment));
                self.feedback_message(result);
            }
            Some(NoteAction::Discard(comment)) => {
                let feedback = &mut self.focused_mut().studio.feedback;
                feedback
                    .drafts
                    .retain(|draft| draft.comment != comment || draft.pending());
                feedback.changed = true;
            }
            Some(NoteAction::Toggle(reference)) => {
                self.focused_mut().studio.feedback.toggle(&reference);
            }
            Some(NoteAction::Replace(older, newer)) => {
                self.focused_mut().studio.feedback.replace(&older, &newer);
            }
            Some(NoteAction::Revise(note)) => self.focused_mut().studio.feedback.revise(&note),
            Some(NoteAction::Reopen(batch)) => {
                let result = self
                    .with_feedback(|feedback, port| feedback.reopen(&*port, &batch))
                    .and_then(|reopened| reopened);
                if let Err(why) = result {
                    self.focused_mut().studio.feedback.message = Some(why);
                }
            }
            Some(NoteAction::Submit) => {
                let result = self.with_feedback(|feedback, port| feedback.submit(port));
                self.feedback_message(result);
            }
            Some(NoteAction::FirstSave) => {
                self.save_project(false);
            }
            Some(NoteAction::NewNote) => {
                let _ = self.new_feedback_note();
            }
            None => {}
        }
        let feedback = &mut self.focused_mut().studio.feedback;
        if feedback.changed && feedback.gesture.is_none() {
            feedback.keep();
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
#[path = "studio_feedback_tests.rs"]
mod ja_s3_feedback;
