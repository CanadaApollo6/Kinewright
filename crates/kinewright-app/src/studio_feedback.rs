//! S3 C3: typed and drawn comments on the displayed picture, private saved
//! revisions and immutable Revise cut batches. Every anchor comes from the bound
//! image, never from the person's latest selection or requested position.
use crate::{app::KinewrightApp, presenter::DisplayCell};
use eframe::egui::{self, Pos2, Rect, Vec2};
use kinewright_core::{ClipId, Document, FrameTexture, HistorySnapshot, TimeCode, TrackKind};
use kinewright_project::{
    bundle::{BundleError, ProjectBundle},
    bundle_records::{BranchCheckpoint, ObjectRef},
    editing_session::Ja1EditingSession,
    receipts::{CommandId, IssuerId, ReceiptLookup, ReceiptResult},
    storage_budget::StorageLimits,
    store::{ProjectStore, StorePin},
    studio_records::{
        CommentId, FeedbackAnchor, FeedbackBatch, FeedbackBatchId, FeedbackEvidenceKind,
        FeedbackIndex, FeedbackRevision, FeedbackTarget, FrameRate, MetadataChange,
        MetadataEnvelope, MetadataResult, MetadataScope, MetadataUpdate, Orientation,
        ReviseRequest, SnapshotDescriptor, SnapshotKey,
    },
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

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

/// The manual history position a picture of the current edit shows: its exact
/// document at its exact journal position. Only a published snapshot of this
/// exact cut acknowledges it; a later head never does, even with equal content.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AcceptedCut {
    pub(crate) document_hash: String,
    pub(crate) batches: u64,
}
impl AcceptedCut {
    pub(crate) fn of(history: &HistorySnapshot) -> Result<Self, String> {
        let document = serde_json::to_vec(&*history.document).map_err(|e| e.to_string())?;
        Ok(Self {
            document_hash: kinewright_media::sha256_bytes(&document),
            batches: history.floor_offset + history.batches.len() as u64,
        })
    }
    /// A later head never acknowledges an earlier cut.
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
#[derive(Clone, Debug, PartialEq, Eq)]
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

pub(crate) enum Origin {
    /// A new comment on the bound picture. A mark on the current edit names its
    /// snapshot only once that exact cut is published.
    Captured {
        proof: FrameProof,
        readback: Option<Readback>,
        snapshot: Option<SnapshotKey>,
        layers: Vec<Layer>,
        layer: Option<usize>,
    },
    /// A later revision keeps its comment's original anchor and frame proof.
    Revises {
        anchor: FeedbackAnchor,
        frame_proof_ref: ObjectRef,
    },
}
pub(crate) struct PendingWrite {
    pub(crate) envelope: MetadataEnvelope,
    pub(crate) update: MetadataUpdate,
    /// Why the last reply did not settle, if it did not.
    pub(crate) unknown: Option<String>,
    _pins: Vec<StorePin>,
}
pub(crate) enum DraftState {
    Editing,
    Refused(String),
    /// The original command, retried exactly until its outcome is known.
    Pending(Box<PendingWrite>),
}
pub(crate) struct Draft {
    pub(crate) comment: CommentId,
    pub(crate) origin: Origin,
    pub(crate) text: String,
    pub(crate) strokes: Vec<Stroke>,
    pub(crate) state: DraftState,
}
impl Draft {
    pub(crate) fn pending(&self) -> bool {
        matches!(self.state, DraftState::Pending(_))
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
            Ok((descriptor.key, AcceptedCut::of(&history)?))
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

/// A saved comment's newest revision, as the Notes list shows it.
#[derive(Clone, Debug)]
pub(crate) struct SavedNote {
    pub(crate) comment: CommentId,
    pub(crate) reference: ObjectRef,
    pub(crate) revision: FeedbackRevision,
    pub(crate) text: String,
}
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

/// Studio feedback state for one project session.
#[derive(Default)]
pub(crate) struct Feedback {
    pub(crate) drafts: Vec<Draft>,
    pub(crate) active: Option<CommentId>,
    pub(crate) drawing: bool,
    issuer: Option<IssuerId>,
    /// The Revise cut selection: exact saved revisions, possibly across versions.
    pub(crate) selection: Vec<ObjectRef>,
    pub(crate) submission: Option<Submission>,
    pub(crate) saved: Vec<SavedNote>,
    pub(crate) batches: Vec<FeedbackBatchId>,
    pub(crate) message: Option<String>,
    pub(crate) loaded: bool,
    pub(crate) scene: Option<String>,
}
impl Feedback {
    pub(crate) fn draft_mut(&mut self, comment: &CommentId) -> Option<&mut Draft> {
        self.drafts
            .iter_mut()
            .find(|draft| &draft.comment == comment)
    }
    /// Start a revision of a saved comment. It keeps the original anchor and proof.
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
            },
            text: note.text.clone(),
            strokes: Vec::new(),
            state: DraftState::Editing,
        });
        self.active = Some(note.comment.clone());
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
            let revision = port.revision(reference)?;
            let text =
                String::from_utf8_lossy(&port.evidence(&revision.original_text_ref)?).into_owned();
            saved.push(SavedNote {
                comment: comment.clone(),
                reference: reference.clone(),
                revision,
                text,
            });
        }
        saved.sort_by_key(|note| std::cmp::Reverse(note.revision.anchor.frame));
        self.saved = saved;
        self.batches = index.batches.keys().cloned().collect();
        if self.drafts.iter().any(|draft| draft.waiting().is_some()) {
            self.promote(&port.published()?);
        }
        self.loaded = true;
        Ok(())
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
        outcome
    }
    fn save_inner(&mut self, port: &mut dyn FeedbackPort, comment: &CommentId) -> Outcome {
        let comments = self
            .saved
            .iter()
            .filter(|note| &note.comment != comment)
            .count();
        let Some(draft) = self
            .drafts
            .iter_mut()
            .find(|draft| &draft.comment == comment)
        else {
            return Outcome::Refused("This draft is gone.".into());
        };
        let reply = if let DraftState::Pending(pending) = &draft.state {
            port.metadata(&pending.envelope, &pending.update)
        } else {
            let write = match prepare(port, &mut self.issuer, draft, comments) {
                Ok(write) => write,
                Err(outcome) => {
                    if let Outcome::Refused(why) = &outcome {
                        draft.state = DraftState::Refused(why.clone());
                    }
                    return outcome;
                }
            };
            let reply = port.metadata(&write.envelope, &write.update);
            draft.state = DraftState::Pending(Box::new(write));
            reply
        };
        match reply {
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
    }
    /// Load a submitted batch's exact revisions into the selection. The batch itself
    /// stays immutable; submitting again creates a new batch.
    pub(crate) fn reopen(
        &mut self,
        port: &dyn FeedbackPort,
        batch: &FeedbackBatchId,
    ) -> Result<(), String> {
        self.selection = port.batch(batch)?.revision_refs;
        Ok(())
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
        outcome
    }
    fn submit_inner(&mut self, port: &mut dyn FeedbackPort) -> Outcome {
        if self.submission.is_none() {
            if self.selection.is_empty() {
                return Outcome::Refused("Choose saved notes for Revise cut first.".into());
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
        }
        let Some(submission) = &mut self.submission else {
            return Outcome::Refused("No batch is pending.".into());
        };
        match port.metadata(&submission.write.envelope, &submission.write.update) {
            Reply::Durable(MetadataResult::Batch(id)) if id == submission.batch.batch_id => {
                let submitted = submission.batch.revision_refs.clone();
                self.submission = None;
                self.selection
                    .retain(|reference| !submitted.contains(reference));
                let _ = self.refresh(&*port);
                Outcome::Submitted(id)
            }
            Reply::Durable(_) => {
                submission.write.unknown = Some("unexpected batch receipt".into());
                Outcome::Unknown("unexpected batch receipt".into())
            }
            Reply::Unknown(why) => {
                submission.write.unknown = Some(why.clone());
                Outcome::Unknown(why)
            }
            Reply::Refused(why) => {
                self.submission = None;
                Outcome::Refused(why)
            }
        }
    }
}

/// Seal a draft's evidence and build its exact command. Ingress caps refuse with
/// the draft intact.
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
    let strokes = strokes_bytes(&draft.strokes).map_err(Outcome::Refused)?;
    let mut pins = Vec::new();
    let mut admit = |kind, bytes: &[u8]| {
        let (reference, pin) = port.admit(kind, bytes).map_err(Outcome::Refused)?;
        pins.extend(pin);
        Ok::<_, Outcome>(reference)
    };
    let original_text_ref = admit(FeedbackEvidenceKind::Text, draft.text.as_bytes())?;
    let strokes_ref = admit(FeedbackEvidenceKind::Strokes, &strokes)?;
    let (anchor, frame_proof_ref) = match &mut draft.origin {
        Origin::Revises {
            anchor,
            frame_proof_ref,
        } => (anchor.clone(), frame_proof_ref.clone()),
        Origin::Captured {
            proof,
            readback,
            snapshot,
            layers,
            layer,
        } => {
            if let Some(readback) = readback.take() {
                proof.pixels = Some(readback.finish().map_err(Outcome::Refused)?);
            }
            let bytes = serde_json::to_vec(&*proof).map_err(|e| Outcome::Refused(e.to_string()))?;
            let reference = admit(FeedbackEvidenceKind::FrameProof, &bytes)?;
            let snapshot = snapshot
                .clone()
                .ok_or_else(|| Outcome::Waiting("This mark has no published version.".into()))?;
            (
                FeedbackAnchor {
                    snapshot,
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
                },
                reference,
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
        voice_ref: None,
        transcript_revision: None,
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
    Revise(Box<SavedNote>),
    Reopen(FeedbackBatchId),
    Submit,
    FirstSave,
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
    /// Start a draft on the bound picture: its version or exact current-edit cut,
    /// its frame and its pixels, whatever the selection or playhead requests next.
    pub(crate) fn capture_feedback(&mut self) -> Result<CommentId, String> {
        let cell = self.presenter.bound().ok_or("Nothing is shown yet.")?;
        let session = self.focused();
        let studio = &session.studio;
        let history;
        let manual = match (&cell.context, &studio.manual_inputs) {
            (None, Some(inputs))
                if !cell.stale
                    && self
                        .presenter
                        .context_epoch()
                        .is_some_and(|epoch| cell.stamp.epoch >= epoch) =>
            {
                history = session.core.export_history().map_err(text)?;
                (*history.document == *inputs.document())
                    .then(|| AcceptedCut::of(&history))
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
        Ok(comment)
    }
    fn feedback_message(&mut self, result: Result<Outcome, String>) {
        if let Err(why) = result {
            self.focused_mut().studio.feedback.message = Some(why);
        }
    }
    /// Draw on the displayed image and paint the active draft's strokes.
    #[allow(clippy::too_many_lines, reason = "one immediate-mode overlay")]
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
                let current = bound.as_ref().map(|cell| cell.frame_id);
                let reuse = {
                    let feedback = &self.focused().studio.feedback;
                    feedback.active.as_ref().and_then(|active| {
                        feedback.drafts.iter().find(|draft| {
                            &draft.comment == active
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
                match comment {
                    Ok(comment) => {
                        let feedback = &mut self.focused_mut().studio.feedback;
                        if let Some(draft) = feedback.draft_mut(&comment) {
                            draft.strokes.push(Vec::new());
                        }
                        feedback.active = Some(comment);
                    }
                    Err(why) => self.focused_mut().studio.feedback.message = Some(why),
                }
                self.focused_mut().studio.tab = crate::studio::Tab::Notes;
            }
            if response.dragged()
                && let Some(point) = response
                    .interact_pointer_pos()
                    .and_then(|at| image.normalise(at))
            {
                let limit = StorageLimits::default().comment_points;
                let feedback = &mut self.focused_mut().studio.feedback;
                if let Some(comment) = feedback.active.clone()
                    && let Some(draft) = feedback.draft_mut(&comment)
                    && !draft.strokes.is_empty()
                {
                    if (points(&draft.strokes) as u64) < limit {
                        if let Some(stroke) = draft.strokes.last_mut()
                            && stroke.last() != Some(&point)
                        {
                            stroke.push(point);
                        }
                    } else {
                        feedback.message = Some(format!(
                            "This drawing reached {limit} points. Save it or start another note."
                        ));
                    }
                }
            }
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
        if bound.is_none_or(|cell| cell.frame_id != proof.bound.frame_id) {
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
    /// `KINEWRIGHT_SCREENSHOT_STUDIO_SCENE`: a draft being drawn, or the Revise cut selector.
    fn stage_feedback_scene(&mut self, scene: &str) {
        self.focused_mut().studio.tab = crate::studio::Tab::Notes;
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
        let feedback = &mut self.focused_mut().studio.feedback;
        feedback.drawing = false;
        feedback.selection = feedback
            .saved
            .iter()
            .map(|note| note.reference.clone())
            .collect();
        feedback.message = None;
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
        ui.horizontal(|ui| {
            ui.label("Notes · newest first");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("Refresh").clicked() {
                    self.focused_mut().studio.feedback.loaded = false;
                }
            });
        });
        if let Some(message) = &self.focused().studio.feedback.message {
            ui.colored_label(po0::TEXT_2, message);
        }
        let mut action = None;
        let fps = self.program_document().fps;
        let footer = 40.0;
        egui::ScrollArea::vertical()
            .id_salt("studio-notes")
            .auto_shrink([false, false])
            .max_height((ui.available_height() - footer).max(60.0))
            .show(ui, |ui| {
                let Feedback {
                    drafts,
                    saved,
                    selection,
                    batches,
                    active,
                    ..
                } = &mut self.focused_mut().studio.feedback;
                if drafts.is_empty() && saved.is_empty() {
                    ui.colored_label(
                        po0::TEXT_3,
                        "Draw on the picture or edit a saved note. Notes stay private until Revise cut.",
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
                                .desired_rows(2)
                                .desired_width(f32::INFINITY)
                                .hint_text("What should change here?"),
                        );
                        if edit.gained_focus() {
                            *active = Some(draft.comment.clone());
                        }
                        if let Origin::Captured { layers, layer, .. } = &mut draft.origin
                            && !layers.is_empty()
                            && !pending
                        {
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
                        }
                        ui.horizontal(|ui| {
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
                for note in saved.iter() {
                    ui.horizontal(|ui| {
                        let mut chosen = selection.contains(&note.reference);
                        let line = note.text.lines().next().unwrap_or("Drawing");
                        let first = if line.chars().count() > 28 {
                            format!("{}…", line.chars().take(27).collect::<String>())
                        } else {
                            line.to_owned()
                        };
                        if ui
                            .checkbox(
                                &mut chosen,
                                format!(
                                    "{} · {first}",
                                    crate::timeline_ui::format_timecode(
                                        TimeCode(note.revision.anchor.frame),
                                        fps
                                    )
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
            });
        let feedback = &self.focused().studio.feedback;
        let pending = feedback.submission.is_some();
        let label = if pending {
            "Retry Revise cut".to_owned()
        } else {
            format!("Revise cut · {} selected", feedback.selection.len())
        };
        let unsaved = self
            .focused()
            .ja1
            .as_ref()
            .is_some_and(|attachment| attachment.project_path().is_none());
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    pending || !feedback.selection.is_empty(),
                    egui::Button::new(label),
                )
                .clicked()
            {
                action = Some(NoteAction::Submit);
            }
            if unsaved && ui.button("Save project…").clicked() {
                action = Some(NoteAction::FirstSave);
            }
        });
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
            }
            Some(NoteAction::Toggle(reference)) => {
                self.focused_mut().studio.feedback.toggle(&reference);
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
            None => {}
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
#[path = "studio_feedback_tests.rs"]
mod ja_s3_feedback;
