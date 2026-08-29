//! CC6 §8.1: the read-only Colour QC window.
//!
//! The window measures a **cloned snapshot** of the focused project at one
//! frame and renders integers. It holds no edit path, constructs no
//! [`kinewright_core::Operation`], and never marks the project dirty — the
//! same contract the M41 media cache dialog states in the same voice.
//!
//! The worker discipline is the scopes panel's, unchanged: one thread at a
//! time, latest request wins, responses filtered by a monotonic generation
//! and the exact editor context key, so a response that no longer describes
//! the live editor is dropped before it can be painted.

use std::{
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
};

use eframe::egui;
use kinewright_core::{
    Analysis, ClipId, ColorQcCheck, ColorQcReport, ColorQcRequest, DeliveryEncodeDepth, Document,
    EffectId, MatteRegionDescription, MatteRegionScope, MediaError, QaSeverity, TimeCode,
    WorkingProof, WorkingProofMetadata, delivery_color_for_depth, matte_coverage_statistics,
    measure_color_qc,
};

use crate::{
    app::KinewrightApp,
    color_scopes_ui::ScopeRoi,
    matte_overlay_ui::{AnalysisMatteProofSource, MatteProofSource, MatteTarget},
    theme::{self, color, type_size},
};

/// The standing banner. CC6 §8: every QC surface says what it does not do, in
/// the same voice `color_scopes_ui` and `media_workflow` already use.
pub(crate) const COLOR_QC_BANNER: &str =
    "Measuring never changes the project document, the grade, or the exported file.";

/// The per-node toggle's label. The cost is in the label, not in a tooltip:
/// CC6 §3.7 renders one baseline plus up to sixteen removals.
pub(crate) const PER_NODE_TOGGLE_LABEL: &str = "Per node (renders up to 17 full-resolution frames)";

/// The pre-export tag note (CC6 §3.6).
///
/// One `\`-continued literal rather than an indented multi-line string: egui
/// does not collapse whitespace, so an embedded run of spaces is drawn as a
/// gap in the middle of a sentence.
pub(crate) const PRE_EXPORT_TAG_NOTE: &str = "Nothing has been exported yet, so observed is the same materialised value as expected: this \
     check answers whether these tags would be accepted at this depth. A written file is compared \
     against its probe after an export.";

/// The editor context one full-resolution working proof describes.
///
/// The proof is a function of the project, its revision, and the frame, and of
/// nothing else a QC surface chooses: an ROI, a matte scope, and a delivery
/// lane are all measured *from* it rather than rendered into it. That is
/// exactly why one render can feed both the Colour QC window and the viewer's
/// clipping mask.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WorkingProofKey {
    pub(crate) session_id: u64,
    pub(crate) revision: u64,
    pub(crate) frame: TimeCode,
}

/// The one working proof the Colour QC window and the QC clipping mask share.
///
/// CC6 §8.1 and §8.2 measure the same `working_linear_post_composite` raster at
/// the same frame, and each worker owns its own `FrameRenderer` and decoder.
/// Without this, turning the mask on and pressing Measure stood two
/// full-resolution renderers side by side for one identical raster.
///
/// The render happens **under the lock**, deliberately: releasing it around the
/// render would let two workers miss the same key and start two renderers,
/// which is the thing this type exists to prevent. Whoever waits is a
/// background worker — never the UI thread — and it wakes with the proof it
/// would otherwise have spent a render producing. There is exactly one lock
/// site per worker and no nesting, so the two cannot deadlock each other.
///
/// One entry: a proof is a whole full-resolution `f32` raster, and the only
/// frame two surfaces ever want at once is the one under the playhead.
#[derive(Default)]
pub(crate) struct WorkingProofCache {
    entry: Mutex<Option<(WorkingProofKey, Arc<WorkingProof>)>>,
}

impl std::fmt::Debug for WorkingProofCache {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("WorkingProofCache").finish()
    }
}

impl WorkingProofCache {
    /// The proof for `key`, rendered by `source` only if nobody has one.
    ///
    /// # Errors
    ///
    /// Returns the source's own message when the render fails. A failure is
    /// not cached: the next caller asks again.
    pub(crate) fn proof(
        &self,
        source: &dyn ColorQcSource,
        document: Arc<Document>,
        key: WorkingProofKey,
    ) -> Result<Arc<WorkingProof>, String> {
        self.proof_with(key, move || source.working_proof(document, key))
    }

    /// Drop the stored proof when it no longer describes the editor context.
    ///
    /// A full-resolution scene-linear raster is the largest thing either QC
    /// surface touches, so it is kept only for the frame under the playhead
    /// rather than until something else happens to want a different one.
    ///
    /// Called from the frame loop, so it **never waits**: a locked cache means
    /// a worker is rendering into it at this instant, and whatever it stores
    /// is tested against the live context on the next frame anyway. Blocking
    /// here would stall the UI thread for a whole render.
    pub(crate) fn retain_context(&self, live: WorkingProofKey) {
        let mut entry = match self.entry.try_lock() {
            Ok(entry) => entry,
            // A worker panicked mid-render; the entry is still readable and
            // still worth dropping.
            Err(std::sync::TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
            // A render is in flight right now. Whatever it stores is tested
            // against the live context on the next frame.
            Err(std::sync::TryLockError::WouldBlock) => return,
        };
        if entry.as_ref().is_some_and(|(stored, _)| *stored != live) {
            *entry = None;
        }
    }

    /// The proof for `key`, rendered by `render` only if nobody has one.
    fn proof_with(
        &self,
        key: WorkingProofKey,
        render: impl FnOnce() -> Result<WorkingProof, String>,
    ) -> Result<Arc<WorkingProof>, String> {
        // A poisoned lock means a worker panicked mid-render. This is a cache:
        // the next reader re-renders rather than propagating that panic into a
        // second thread and wedging every QC surface.
        let mut entry = self.entry.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((stored, proof)) = entry.as_ref()
            && *stored == key
        {
            return Ok(Arc::clone(proof));
        }
        let proof = Arc::new(render()?);
        *entry = Some((key, Arc::clone(&proof)));
        Ok(proof)
    }
}

/// The blocking work one colour QC measurement performs.
///
/// Naming it as a trait keeps the panel testable without an `Analysis`
/// backend, exactly as CC5's `MatteProofSource` does for the matte view: a
/// test can supply a source that blocks on command, counts the renders that
/// actually started, and records every call it was asked for.
pub(crate) trait ColorQcSource: Send + Sync + 'static {
    /// Render the full-resolution scene-linear working proof (CC6 §2.2).
    ///
    /// This **renders**; it never consults [`WorkingProofCache`]. Sharing is
    /// the cache's job and the cache calls this method under its own lock, so
    /// a source that reached back into it would deadlock.
    fn working_proof(
        &self,
        document: Arc<Document>,
        key: WorkingProofKey,
    ) -> Result<WorkingProof, String>;

    /// The report, its per-node attribution, and the provenance of the render
    /// they were both measured on (CC6 §3.7).
    ///
    /// One call rather than two, because the baseline render is shared:
    /// measuring the report separately and then asking for the attribution
    /// rendered this frame twice — eighteen full-resolution renders for
    /// sixteen candidates. The returned metadata therefore describes the very
    /// render the report was measured on, never a second opinion.
    fn measure_with_nodes(
        &self,
        document: Arc<Document>,
        key: WorkingProofKey,
        request: &ColorQcRequest,
    ) -> Result<(ColorQcReport, WorkingProofMetadata), String>;
}

/// The production source: the live analysis backend.
pub(crate) struct AnalysisColorQcSource {
    analysis: Arc<dyn Analysis>,
    cache: Arc<WorkingProofCache>,
}

impl AnalysisColorQcSource {
    pub(crate) const fn new(analysis: Arc<dyn Analysis>, cache: Arc<WorkingProofCache>) -> Self {
        Self { analysis, cache }
    }
}

impl ColorQcSource for AnalysisColorQcSource {
    fn working_proof(
        &self,
        document: Arc<Document>,
        key: WorkingProofKey,
    ) -> Result<WorkingProof, String> {
        self.analysis
            .working_proof_for_document(document, key.frame)
            .map_err(|error| error.to_string())
    }

    fn measure_with_nodes(
        &self,
        document: Arc<Document>,
        key: WorkingProofKey,
        request: &ColorQcRequest,
    ) -> Result<(ColorQcReport, WorkingProofMetadata), String> {
        // `measure_color_qc_with_nodes` renders the baseline itself and hands
        // back only the report, so the provenance of that render is observed
        // where it happens rather than reproduced by rendering again.
        let analysis = BaselineProofAnalysis {
            inner: Arc::clone(&self.analysis),
            cache: Arc::clone(&self.cache),
            document: Arc::clone(&document),
            key,
            baseline: Mutex::new(None),
        };
        let report = kinewright_core::nodes::measure_color_qc_with_nodes(
            &analysis, document, key.frame, request,
        )
        .map_err(|error| error.to_string())?;
        let metadata = analysis.baseline_metadata().ok_or_else(|| {
            "the per-node pass finished without rendering a baseline working proof".to_owned()
        })?;
        Ok((report, metadata))
    }
}

/// An [`Analysis`] that shares the **baseline** working proof with the rest of
/// the app and remembers the provenance of the one it served.
///
/// CC6 §3.7 renders one baseline plus up to sixteen scratch removals through
/// this backend. Only the baseline describes the project as it stands, so only
/// the baseline may be served from — or stored into — the shared cache. The
/// test is `Arc::ptr_eq` against the document this proxy was built for, which
/// cannot produce a false positive: `measure_node_contributions` allocates a
/// fresh `Arc` for every removal. A false negative would cost one render and
/// nothing else.
///
/// Every other method forwards, unexamined and unchanged.
struct BaselineProofAnalysis {
    inner: Arc<dyn Analysis>,
    cache: Arc<WorkingProofCache>,
    document: Arc<Document>,
    key: WorkingProofKey,
    baseline: Mutex<Option<WorkingProofMetadata>>,
}

impl BaselineProofAnalysis {
    /// The provenance of the baseline proof, or `None` if none was rendered.
    fn baseline_metadata(&self) -> Option<WorkingProofMetadata> {
        self.baseline
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl Analysis for BaselineProofAnalysis {
    fn working_proof_for_document(
        &self,
        document: Arc<Document>,
        at: TimeCode,
    ) -> Result<WorkingProof, MediaError> {
        let proof = if at == self.key.frame && Arc::ptr_eq(&document, &self.document) {
            let inner = Arc::clone(&self.inner);
            let shared = self
                .cache
                .proof_with(self.key, move || {
                    inner
                        .working_proof_for_document(document, at)
                        .map_err(|error| error.to_string())
                })
                .map_err(MediaError::Backend)?;
            (*shared).clone()
        } else {
            // A scratch clone with one node removed: never storable under the
            // baseline's key.
            self.inner.working_proof_for_document(document, at)?
        };
        // The **first** render of the pass is the baseline: CC6 §3.7 renders
        // and measures it before it removes anything. Recorded from the call
        // order rather than from the sharing test above, so a baseline that
        // arrives under a rebuilt `Arc` still yields the right provenance
        // instead of failing the whole measurement.
        let mut baseline = self.baseline.lock().unwrap_or_else(PoisonError::into_inner);
        if baseline.is_none() {
            *baseline = Some(proof.metadata.clone());
        }
        Ok(proof)
    }

    fn probe(&self, path: &std::path::Path) -> Result<kinewright_core::MediaAsset, MediaError> {
        self.inner.probe(path)
    }

    fn media_availability(
        &self,
        asset: &kinewright_core::MediaAsset,
    ) -> kinewright_core::MediaAvailabilityStatus {
        self.inner.media_availability(asset)
    }

    fn thumbnail_at(
        &self,
        at: TimeCode,
        max_width: u32,
    ) -> Result<kinewright_core::RgbaImage, MediaError> {
        self.inner.thumbnail_at(at, max_width)
    }

    fn thumbnail_for_document(
        &self,
        document: Arc<Document>,
        at: TimeCode,
        max_width: u32,
    ) -> Result<kinewright_core::RgbaImage, MediaError> {
        self.inner.thumbnail_for_document(document, at, max_width)
    }

    fn monitor_proof_for_document(
        &self,
        document: Arc<Document>,
        at: TimeCode,
    ) -> Result<kinewright_core::MonitorProof, MediaError> {
        self.inner.monitor_proof_for_document(document, at)
    }

    fn matte_proof_for_document(
        &self,
        document: Arc<Document>,
        at: TimeCode,
        clip: ClipId,
        effect: EffectId,
    ) -> Result<kinewright_core::MatteProof, MediaError> {
        self.inner
            .matte_proof_for_document(document, at, clip, effect)
    }

    fn verify_delivery_output(
        &self,
        document: Arc<Document>,
        path: &std::path::Path,
        settings: &kinewright_core::ExportSettings,
        request: kinewright_core::DeliveryVerificationRequest,
    ) -> Result<kinewright_core::DeliveryVerification, MediaError> {
        self.inner
            .verify_delivery_output(document, path, settings, request)
    }

    fn request_transcription(&self, asset: kinewright_core::MediaAsset) {
        self.inner.request_transcription(asset);
    }

    fn request_transcription_with_language(
        &self,
        asset: kinewright_core::MediaAsset,
        language: Option<&str>,
    ) {
        self.inner
            .request_transcription_with_language(asset, language);
    }

    fn transcript_status(
        &self,
        asset: &kinewright_core::MediaAsset,
    ) -> kinewright_core::TranscriptStatus {
        self.inner.transcript_status(asset)
    }

    fn timeline_transcript(
        &self,
        document: &Document,
        range: Option<std::ops::Range<TimeCode>>,
    ) -> Result<Vec<kinewright_core::TimelineTranscriptWord>, MediaError> {
        self.inner.timeline_transcript(document, range)
    }

    fn request_silence_detection(&self, asset: kinewright_core::MediaAsset) {
        self.inner.request_silence_detection(asset);
    }

    fn silence_status(
        &self,
        asset: &kinewright_core::MediaAsset,
    ) -> kinewright_core::SilenceStatus {
        self.inner.silence_status(asset)
    }

    fn timeline_silences(
        &self,
        document: &Document,
        range: Option<std::ops::Range<TimeCode>>,
        minimum_source_frames: TimeCode,
    ) -> Result<Vec<kinewright_core::TimelineSilenceSpan>, MediaError> {
        self.inner
            .timeline_silences(document, range, minimum_source_frames)
    }

    fn request_scene_detection(&self, asset: kinewright_core::MediaAsset) {
        self.inner.request_scene_detection(asset);
    }

    fn scene_status(&self, asset: &kinewright_core::MediaAsset) -> kinewright_core::SceneStatus {
        self.inner.scene_status(asset)
    }

    fn timeline_scene_changes(
        &self,
        document: &Document,
        range: Option<std::ops::Range<TimeCode>>,
        minimum_confidence_basis_points: u16,
    ) -> Result<Vec<kinewright_core::TimelineSceneChange>, MediaError> {
        self.inner
            .timeline_scene_changes(document, range, minimum_confidence_basis_points)
    }

    fn asset_loudness(
        &self,
        asset: &kinewright_core::MediaAsset,
    ) -> Result<kinewright_core::AudioLoudness, MediaError> {
        self.inner.asset_loudness(asset)
    }

    fn timeline_loudness(
        &self,
        document: &Document,
    ) -> Result<kinewright_core::AudioLoudness, MediaError> {
        self.inner.timeline_loudness(document)
    }

    fn request_beat_detection(&self, asset: kinewright_core::MediaAsset) {
        self.inner.request_beat_detection(asset);
    }

    fn beat_status(&self, asset: &kinewright_core::MediaAsset) -> kinewright_core::BeatStatus {
        self.inner.beat_status(asset)
    }

    fn timeline_beats(
        &self,
        document: &Document,
        range: Option<std::ops::Range<TimeCode>>,
        minimum_strength_basis_points: u16,
    ) -> Result<Vec<kinewright_core::TimelineBeat>, MediaError> {
        self.inner
            .timeline_beats(document, range, minimum_strength_basis_points)
    }

    fn cancel_analysis(
        &self,
        asset: &kinewright_core::MediaAsset,
        kind: kinewright_core::AnalysisKind,
    ) -> bool {
        self.inner.cancel_analysis(asset, kind)
    }

    fn request_waveform(
        &self,
        asset: kinewright_core::MediaAsset,
        request_generation: u64,
    ) -> bool {
        self.inner.request_waveform(asset, request_generation)
    }

    fn request_thumbnail(
        &self,
        asset: kinewright_core::MediaAsset,
        source_at: TimeCode,
        max_width: u32,
        request_generation: u64,
    ) -> bool {
        self.inner
            .request_thumbnail(asset, source_at, max_width, request_generation)
    }

    fn visual_asset_results(
        &self,
    ) -> crossbeam_channel::Receiver<kinewright_core::VisualAssetResult> {
        self.inner.visual_asset_results()
    }

    fn cache_inventory(&self) -> kinewright_core::MediaCacheInventory {
        self.inner.cache_inventory()
    }

    fn clear_cache(
        &self,
        family: kinewright_core::MediaCacheFamily,
    ) -> Result<kinewright_core::MediaCacheClearResult, MediaError> {
        self.inner.clear_cache(family)
    }
}

/// The editor context one measurement describes.
///
/// Independent of the `Document` pointer so a stale response can be rejected
/// without holding the snapshot it was measured from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ColorQcKey {
    pub(crate) session_id: u64,
    pub(crate) revision: u64,
    pub(crate) frame: TimeCode,
    /// `None` measures the whole raster. A region is something the user
    /// named; the absence of one is a fact the report has to keep.
    pub(crate) roi: Option<ScopeRoi>,
    pub(crate) matte: Option<MatteTarget>,
    pub(crate) per_node: bool,
    pub(crate) depth: DeliveryEncodeDepth,
}

impl ColorQcKey {
    /// The render this measurement needs, shorn of everything measured *from*
    /// that render rather than into it.
    pub(crate) const fn proof_key(self) -> WorkingProofKey {
        WorkingProofKey {
            session_id: self.session_id,
            revision: self.revision,
            frame: self.frame,
        }
    }
}

/// One accepted measurement and the provenance of the proof it was taken on.
#[derive(Debug, Clone)]
pub(crate) struct ColorQcMeasurement {
    pub(crate) key: ColorQcKey,
    pub(crate) report: ColorQcReport,
    pub(crate) metadata: WorkingProofMetadata,
}

struct ColorQcResponse {
    generation: u64,
    key: ColorQcKey,
    result: Result<Box<ColorQcMeasurement>, String>,
}

/// A request the window accepted while a worker was still rendering.
struct QueuedMeasurement {
    generation: u64,
    key: ColorQcKey,
    source: Arc<dyn ColorQcSource>,
    cache: Arc<WorkingProofCache>,
    matte_source: Option<Arc<dyn MatteProofSource>>,
    document: Arc<Document>,
    request: ColorQcRequest,
}

struct ActiveWorker {
    cancelled: Arc<AtomicBool>,
    handle: thread::JoinHandle<()>,
}

impl ActiveWorker {
    fn retire(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    fn is_finished(&self) -> bool {
        self.handle.is_finished()
    }
}

/// Resolves every live worker exactly once: with its report, or with an error
/// if the thread unwinds before it delivers one.
struct WorkerCompletion {
    generation: u64,
    key: ColorQcKey,
    response_tx: mpsc::Sender<ColorQcResponse>,
    cancelled: Arc<AtomicBool>,
    delivered: bool,
}

impl WorkerCompletion {
    fn is_retired(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    fn deliver(&mut self, result: Result<Box<ColorQcMeasurement>, String>) {
        if self.delivered {
            return;
        }
        self.delivered = true;
        if self.is_retired() {
            return;
        }
        let _ = self.response_tx.send(ColorQcResponse {
            generation: self.generation,
            key: self.key,
            result,
        });
    }
}

impl Drop for WorkerCompletion {
    fn drop(&mut self) {
        self.deliver(Err(
            "the colour QC worker stopped before it delivered a measurement".to_owned(),
        ));
    }
}

/// Every piece of ephemeral Colour QC state.
///
/// Nothing here is part of the project document, and no method on this type
/// emits a Core operation — the type has no way to reach one.
pub(crate) struct ColorQcState {
    pub(crate) open: bool,
    /// CC6 §3.7 is off by default: it is the only part of the window that
    /// costs seventeen full-resolution renders.
    per_node: bool,
    /// The delivery lane the tag check and the `Y'CbCr` reference use.
    depth: DeliveryEncodeDepth,
    /// The editor context the live measurement must describe: project,
    /// revision, playhead. Deliberately *not* the whole request key — the ROI
    /// and the lane are choices the next measurement makes, not facts about
    /// the editor that can go stale underneath one.
    current_context: Option<(u64, u64, TimeCode)>,
    current: Option<ColorQcMeasurement>,
    pending: Option<(u64, ColorQcKey)>,
    active: Option<ActiveWorker>,
    queued: Option<QueuedMeasurement>,
    generation: u64,
    response_tx: mpsc::Sender<ColorQcResponse>,
    response_rx: mpsc::Receiver<ColorQcResponse>,
    error: Option<String>,
    #[cfg(test)]
    spawned_workers: u64,
}

impl std::fmt::Debug for ColorQcState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ColorQcState")
            .field("open", &self.open)
            .field("per_node", &self.per_node)
            .field("depth", &self.depth)
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

impl Default for ColorQcState {
    fn default() -> Self {
        // Unbounded: a superseded worker is retired by its flag and filtered
        // by generation, so a full queue must never be able to drop the
        // response the window is waiting for.
        let (response_tx, response_rx) = mpsc::channel();
        Self {
            open: false,
            per_node: false,
            depth: DeliveryEncodeDepth::Eight,
            current_context: None,
            current: None,
            pending: None,
            active: None,
            queued: None,
            generation: 0,
            response_tx,
            response_rx,
            error: None,
            #[cfg(test)]
            spawned_workers: 0,
        }
    }
}

impl ColorQcState {
    #[must_use]
    pub(crate) const fn is_pending(&self) -> bool {
        self.pending.is_some()
    }

    #[must_use]
    pub(crate) const fn per_node(&self) -> bool {
        self.per_node
    }

    /// Turning the toggle on or off changes what a measurement means, so the
    /// current one is retired rather than relabelled.
    pub(crate) fn set_per_node(&mut self, enabled: bool) {
        if self.per_node == enabled {
            return;
        }
        self.per_node = enabled;
        self.invalidate();
    }

    #[must_use]
    pub(crate) const fn depth(&self) -> DeliveryEncodeDepth {
        self.depth
    }

    /// The delivery lane the pre-export tag check materialises against.
    pub(crate) fn set_depth(&mut self, depth: DeliveryEncodeDepth) {
        if self.depth == depth {
            return;
        }
        self.depth = depth;
        self.invalidate();
    }

    #[must_use]
    pub(crate) const fn current(&self) -> Option<&ColorQcMeasurement> {
        self.current.as_ref()
    }

    #[must_use]
    pub(crate) fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// The last report's per-node contributions, for the inspector's node
    /// header line (CC6 §8.3).
    #[must_use]
    pub(crate) fn node_clipping(&self) -> ColorQcNodeClipping {
        ColorQcNodeClipping::from_measurement(self.current.as_ref())
    }

    /// Observe the editor context every frame, exactly as the scopes panel
    /// does: a project switch, a revision change, or a playhead move retires
    /// the current measurement rather than letting it describe a frame nobody
    /// is looking at.
    pub(crate) fn observe_context(&mut self, session_id: u64, revision: u64, frame: TimeCode) {
        let context = (session_id, revision, frame);
        if self
            .current_context
            .is_some_and(|previous| previous != context)
        {
            self.invalidate();
        }
        self.current_context = Some(context);
    }

    /// The editor context a response has to still describe to be accepted.
    const fn context_of(key: ColorQcKey) -> (u64, u64, TimeCode) {
        (key.session_id, key.revision, key.frame)
    }

    /// Accept one bounded measurement request from the live backend.
    pub(crate) fn request_measurement(
        &mut self,
        analysis: Arc<dyn Analysis>,
        cache: Arc<WorkingProofCache>,
        document: Arc<Document>,
        key: ColorQcKey,
        request: ColorQcRequest,
    ) {
        let matte_source: Option<Arc<dyn MatteProofSource>> = key
            .matte
            .map(|_| Arc::new(AnalysisMatteProofSource(Arc::clone(&analysis))) as Arc<_>);
        self.request_measurement_from(
            Arc::new(AnalysisColorQcSource::new(analysis, Arc::clone(&cache))),
            cache,
            matte_source,
            document,
            key,
            request,
        );
    }

    /// Latest-request-wins single flight.
    ///
    /// A request that arrives while a worker is still rendering does **not**
    /// start a second one: the cancel flag cannot interrupt a render already
    /// in progress, so spawning per request would stack concurrent
    /// `FrameRenderer`s, each holding its own cache budget. The newest request
    /// parks in `queued` and starts once the running worker finishes.
    pub(crate) fn request_measurement_from(
        &mut self,
        source: Arc<dyn ColorQcSource>,
        cache: Arc<WorkingProofCache>,
        matte_source: Option<Arc<dyn MatteProofSource>>,
        document: Arc<Document>,
        key: ColorQcKey,
        request: ColorQcRequest,
    ) {
        self.current_context = Some(Self::context_of(key));
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        self.pending = Some((generation, key));
        self.current = None;
        self.error = None;
        let measurement = QueuedMeasurement {
            generation,
            key,
            source,
            cache,
            matte_source,
            document,
            request,
        };
        if self
            .active
            .as_ref()
            .is_some_and(|worker| !worker.is_finished())
        {
            // The running worker's result belongs to an older generation and
            // is filtered on arrival; retiring it also stops it delivering at
            // all. It still has to finish before the next render may start.
            if let Some(worker) = self.active.as_ref() {
                worker.retire();
            }
            self.queued = Some(measurement);
            return;
        }
        self.reap_finished_worker();
        self.spawn(measurement);
    }

    fn spawn(&mut self, measurement: QueuedMeasurement) {
        let QueuedMeasurement {
            generation,
            key,
            source,
            cache,
            matte_source,
            document,
            request,
        } = measurement;
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancelled = Arc::clone(&cancelled);
        let response_tx = self.response_tx.clone();
        let spawn_result = thread::Builder::new()
            .name("kinewright-color-qc".to_owned())
            .spawn(move || {
                let mut completion = WorkerCompletion {
                    generation,
                    key,
                    response_tx,
                    cancelled: worker_cancelled,
                    delivered: false,
                };
                if completion.is_retired() {
                    completion.delivered = true;
                    return;
                }
                completion.deliver(measure_on_worker(
                    source.as_ref(),
                    cache.as_ref(),
                    matte_source.as_deref(),
                    document,
                    key,
                    request,
                ));
            });
        let Ok(handle) = spawn_result else {
            self.pending = None;
            self.active = None;
            self.error = Some("Could not start the colour QC worker".to_owned());
            return;
        };
        #[cfg(test)]
        {
            self.spawned_workers += 1;
        }
        self.active = Some(ActiveWorker { cancelled, handle });
    }

    fn reap_finished_worker(&mut self) {
        if self.active.as_ref().is_some_and(ActiveWorker::is_finished)
            && let Some(worker) = self.active.take()
        {
            let _ = worker.handle.join();
        }
    }

    /// Drain worker responses, accepting only the live generation and key.
    /// This is also where a parked request starts.
    pub(crate) fn poll(&mut self) {
        while let Ok(response) = self.response_rx.try_recv() {
            if self.pending != Some((response.generation, response.key))
                || self.current_context != Some(Self::context_of(response.key))
            {
                continue;
            }
            self.pending = None;
            match response.result {
                Ok(measurement) => {
                    self.current = Some(*measurement);
                    self.error = None;
                }
                Err(message) => {
                    self.current = None;
                    self.error = Some(message);
                }
            }
        }
        self.reap_finished_worker();
        if self.active.is_none()
            && let Some(measurement) = self.queued.take()
        {
            self.spawn(measurement);
        }
    }

    fn invalidate(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        if let Some(worker) = self.active.as_ref() {
            worker.retire();
        }
        self.queued = None;
        self.pending = None;
        self.current = None;
        self.error = None;
    }

    #[cfg(test)]
    const fn spawned_workers(&self) -> u64 {
        self.spawned_workers
    }
}

/// The whole worker body: one working proof, one pure measurement, and — only
/// when the toggle asked for it — the per-node attribution.
///
/// **One render feeds one report.** With the per-node toggle on, the whole
/// measurement is [`ColorQcSource::measure_with_nodes`]: the report, its
/// attribution, and the provenance line all come from the baseline render CC6
/// §3.7 performs, so sixteen candidates cost seventeen renders and not
/// eighteen. With it off, the proof comes from the shared
/// [`WorkingProofCache`], so the viewer's clipping mask and this window share
/// one render of the same frame.
///
/// Pure with respect to the document: [`measure_color_qc`] performs no I/O and
/// constructs no `Operation`, and the per-node pass mutates a clone inside
/// core (CC6 §3.7). Nothing here can reach the live document.
fn measure_on_worker(
    source: &dyn ColorQcSource,
    cache: &WorkingProofCache,
    matte_source: Option<&dyn MatteProofSource>,
    document: Arc<Document>,
    key: ColorQcKey,
    mut request: ColorQcRequest,
) -> Result<Box<ColorQcMeasurement>, String> {
    if let (Some(target), Some(matte_source)) = (key.matte, matte_source) {
        let proof = matte_source.matte_proof(
            Arc::clone(&document),
            key.frame,
            target.clip,
            target.effect,
        )?;
        let statistics =
            matte_coverage_statistics(&proof.coverage).map_err(|error| error.to_string())?;
        request.matte_region = Some(MatteRegionScope {
            description: MatteRegionDescription::new(
                target.clip,
                target.effect,
                statistics.covered_pixel_count,
            ),
            coverage: proof.coverage,
        });
    }
    let proof_key = key.proof_key();
    let (report, metadata) = if key.per_node {
        source.measure_with_nodes(document, proof_key, &request)?
    } else {
        let proof = cache.proof(source, document, proof_key)?;
        let report = measure_color_qc(&proof, &request).map_err(|error| error.to_string())?;
        (report, proof.metadata.clone())
    };
    Ok(Box::new(ColorQcMeasurement {
        key,
        report,
        metadata,
    }))
}

/// The last report's per-node clipping, keyed by `(ClipId, EffectId)`.
///
/// Cloned out of the report rather than borrowed, so the inspector can render
/// it without holding a borrow of the app across a frame that also submits
/// edits. Empty when there is no report or the report carries no attribution.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ColorQcNodeClipping {
    frame: i64,
    entries: Vec<(ClipId, EffectId, i32, i32)>,
}

impl ColorQcNodeClipping {
    /// A snapshot with the entries a test needs.
    ///
    /// Test support: production builds this from a `ColorQcReport`, which the
    /// inspector has no way — and no reason — to construct.
    #[cfg(test)]
    pub(crate) fn from_entries(frame: i64, entries: Vec<(ClipId, EffectId, i32, i32)>) -> Self {
        Self { frame, entries }
    }

    fn from_measurement(measurement: Option<&ColorQcMeasurement>) -> Self {
        let Some(measurement) = measurement else {
            return Self::default();
        };
        let Some(nodes) = measurement.report.nodes.as_ref() else {
            return Self::default();
        };
        Self {
            frame: measurement.report.project_frame,
            entries: nodes
                .nodes
                .iter()
                .map(|node| {
                    (
                        node.clip,
                        node.effect,
                        node.range_basis_points_delta,
                        node.gamut_basis_points_delta,
                    )
                })
                .collect(),
        }
    }

    /// The muted line one colour node's header shows, or `None`.
    ///
    /// Absent when there is no report, when the node is not in it, or when
    /// both deltas are `<= 0` (CC6 §8.3): a node that removed no clipping has
    /// nothing to report, and a negative delta means removing it made things
    /// worse, which is not a contribution.
    ///
    /// The frame is printed so a stale reading is visible rather than
    /// misleading: this is a report of the last measurement, never a live
    /// computation.
    #[must_use]
    pub(crate) fn line_for(&self, clip: ClipId, effect: EffectId) -> Option<String> {
        let (_, _, range, gamut) = self
            .entries
            .iter()
            .find(|(node_clip, node_effect, _, _)| *node_clip == clip && *node_effect == effect)?;
        if *range <= 0 && *gamut <= 0 {
            return None;
        }
        // `{:+}` renders the contract's `+{n}` for a positive delta and the
        // honest `-{n}` for a negative one. Clamping to zero would print `+0`
        // for a node whose removal made the frame *worse*, which is a
        // different fact than "contributed nothing".
        Some(format!(
            "Clipping contribution: {range:+} bp range · {gamut:+} bp gamut (frame {})",
            self.frame
        ))
    }
}

/// The `QaSeverity` colours the branch QA card already uses (`chat_ui.rs`).
#[must_use]
pub(crate) const fn severity_color(severity: QaSeverity) -> egui::Color32 {
    match severity {
        QaSeverity::Error => color::STATUS_DANGER,
        QaSeverity::Warning => color::STATUS_WARNING,
        QaSeverity::Info => color::TEXT_MUTED,
    }
}

/// Render a millionths integer as a signed decimal, without inventing a float.
#[must_use]
fn millionths(value: i64) -> String {
    let sign = if value < 0 { "-" } else { "" };
    let magnitude = value.unsigned_abs();
    format!(
        "{sign}{}.{:06}",
        magnitude / 1_000_000,
        magnitude % 1_000_000
    )
}

/// Render a centidegree integer as degrees.
#[must_use]
fn centidegrees(value: i32) -> String {
    let sign = if value < 0 { "-" } else { "" };
    let magnitude = value.unsigned_abs();
    format!("{sign}{}.{:02}°", magnitude / 100, magnitude % 100)
}

impl KinewrightApp {
    /// The region a skin measurement would be scoped to, if any (CC6 §3.5).
    ///
    /// Skin is a diagnostic of a region the user named. The scopes panel's ROI
    /// and the inspector's expanded matte section are the two regions the app
    /// already has; with neither, the section says so instead of measuring the
    /// whole frame and calling the answer "skin".
    fn color_qc_region(&self) -> (Option<ScopeRoi>, Option<MatteTarget>) {
        let roi = self.color_scopes.roi;
        let roi = (roi != ScopeRoi::full_frame()).then_some(roi);
        (roi, self.matte_overlay.expanded())
    }

    /// CC6 §8.1: the read-only Colour QC window.
    #[allow(clippy::too_many_lines)]
    pub(crate) fn show_color_qc_window(&mut self, ctx: &egui::Context) {
        // `poll_background` has already observed the context and drained the
        // worker this frame, so the inspector and this window read the same
        // measurement rather than two frames of it.
        if !self.color_qc.open {
            return;
        }
        let session_id = self.focused().id;
        let revision = self.focused().revision.0;
        let frame = self.focused().position;
        let mut open = self.color_qc.open;
        let (roi, matte) = self.color_qc_region();
        let mut measure = false;
        let mut per_node = self.color_qc.per_node();
        let mut depth = self.color_qc.depth();
        // A cloned snapshot: nothing the window draws can reach the live
        // document, and nothing it draws is allowed to change it.
        let measurement = self.color_qc.current().cloned();
        let pending = self.color_qc.is_pending();
        let error = self.color_qc.error().map(str::to_owned);
        egui::Window::new("Colour QC")
            .open(&mut open)
            .default_width(560.0)
            .resizable(true)
            .show(ctx, |ui| {
                ui.label(theme::caps_label("COLOUR QC", color::TEXT_MUTED));
                ui.colored_label(color::TEXT_MUTED, COLOR_QC_BANNER);
                ui.horizontal_wrapped(|ui| {
                    ui.colored_label(color::TEXT_MUTED, "Stage:");
                    ui.monospace(kinewright_core::WORKING_PROOF_STAGE);
                    ui.colored_label(color::TEXT_MUTED, format!("· frame {frame}"));
                });
                ui.horizontal_wrapped(|ui| {
                    if ui
                        .add_enabled(!pending, egui::Button::new("Measure current frame"))
                        .clicked()
                    {
                        measure = true;
                    }
                    if pending {
                        ui.colored_label(
                            color::STATUS_WARNING,
                            "Rendering full-resolution working proof…",
                        );
                    } else if let Some(error) = &error {
                        ui.colored_label(
                            color::STATUS_DANGER,
                            format!("Colour QC unavailable: {error}"),
                        );
                    } else if measurement.is_none() {
                        ui.colored_label(color::TEXT_MUTED, "No frame measured");
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    ui.colored_label(color::TEXT_MUTED, "Delivery lane");
                    for lane in DeliveryEncodeDepth::ALL {
                        ui.radio_value(&mut depth, lane, delivery_depth_label(lane));
                    }
                });
                ui.checkbox(&mut per_node, PER_NODE_TOGGLE_LABEL)
                    .on_hover_text(
                        "CC6 §3.7 attributes clipping by removing each colour node from a \
                         scratch clone and re-measuring. The live document is never touched.",
                    );
                egui::ScrollArea::vertical()
                    .max_height(480.0)
                    .show(ui, |ui| {
                        let Some(measurement) = measurement.as_ref() else {
                            ui.colored_label(
                                color::TEXT_MUTED,
                                "Measure a frame to count the pixels the delivery clamp would eat.",
                            );
                            return;
                        };
                        color_qc_sections(ui, measurement);
                    });
            });
        self.color_qc.open = open;
        self.color_qc.set_per_node(per_node);
        self.color_qc.set_depth(depth);
        if measure {
            self.request_color_qc_measurement(session_id, revision, frame, roi, matte);
        }
    }

    fn request_color_qc_measurement(
        &mut self,
        session_id: u64,
        revision: u64,
        frame: TimeCode,
        roi: Option<ScopeRoi>,
        matte: Option<MatteTarget>,
    ) {
        let key = ColorQcKey {
            session_id,
            revision,
            frame,
            roi,
            matte,
            per_node: self.color_qc.per_node(),
            depth: self.color_qc.depth(),
        };
        let document = Arc::clone(&self.focused().document);
        let mut checks = vec![ColorQcCheck::Range, ColorQcCheck::Gamut, ColorQcCheck::Tags];
        if roi.is_some() || matte.is_some() {
            checks.push(ColorQcCheck::Skin);
        }
        if key.per_node {
            checks.push(ColorQcCheck::PerNode);
        }
        let request = ColorQcRequest {
            roi: roi.map(ScopeRoi::to_core),
            matte_region: None,
            checks,
            delivery_bit_depth: key.depth,
            // Pre-export mode (CC6 §3.6): the tags the document *would* be
            // delivered with at this lane, materialised the one way
            // `delivery_color_for_depth` names.
            expected_delivery: Some(delivery_color_for_depth(&document, key.depth)),
            observed_delivery: None,
            max_nodes: u8::try_from(kinewright_core::MAX_QC_NODE_CONTRIBUTIONS).unwrap_or(16),
            project_frame: frame.0,
        };
        let analysis = Arc::clone(&self.analysis);
        let cache = Arc::clone(&self.working_proof_cache);
        self.color_qc
            .request_measurement(analysis, cache, document, key, request);
    }
}

/// The delivery lane radio's label.
const fn delivery_depth_label(depth: DeliveryEncodeDepth) -> &'static str {
    match depth {
        DeliveryEncodeDepth::Eight => "8-bit H.264",
        DeliveryEncodeDepth::Ten => "10-bit H.264",
    }
}

/// Every section of the window body, over a cloned measurement.
///
/// A free function taking a snapshot rather than a method on the app: it
/// cannot reach the live document, cannot construct an `Operation`, and can
/// therefore be rendered in a headless test without a window.
pub(crate) fn color_qc_sections(ui: &mut egui::Ui, measurement: &ColorQcMeasurement) {
    let report = &measurement.report;
    region_line(ui, report);
    range_section(ui, report);
    gamut_section(ui, report);
    // The region the *measurement* was scoped to, not whatever the scopes
    // panel is set to now: a report taken with no region must keep saying so
    // after an ROI is typed, or the prompt describes a measurement nobody has
    // taken yet.
    skin_section(
        ui,
        report,
        measurement.key.roi.is_some() || measurement.key.matte.is_some(),
    );
    tags_section(ui, report);
    nodes_section(ui, report);
    exceptions_section(ui, report);
    provenance_footer(ui, measurement);
}

fn section_heading(ui: &mut egui::Ui, title: &str) {
    ui.add_space(4.0);
    ui.label(egui::RichText::new(title).font(theme::semibold(type_size::CAPTION)));
}

fn region_line(ui: &mut egui::Ui, report: &ColorQcReport) {
    let pixels = &report.region.pixel_roi;
    ui.horizontal_wrapped(|ui| {
        ui.colored_label(color::TEXT_MUTED, "Region measured:");
        ui.monospace(format!(
            "{}×{} at ({}, {})",
            pixels.width, pixels.height, pixels.x, pixels.y
        ));
        match &report.region.matte_region {
            Some(matte) => ui.colored_label(
                color::TEXT_MUTED,
                format!(
                    "· matte clip {} effect {} · {} px covered",
                    matte.clip.0, matte.effect.0, matte.covered_pixel_count
                ),
            ),
            None => ui.colored_label(color::TEXT_MUTED, "· no matte scope"),
        };
    });
}

fn range_section(ui: &mut egui::Ui, report: &ColorQcReport) {
    section_heading(ui, "RANGE");
    egui::Grid::new("color-qc-range")
        .num_columns(5)
        .striped(true)
        .show(ui, |ui| {
            ui.colored_label(color::TEXT_MUTED, "channel");
            ui.colored_label(color::TEXT_MUTED, "over bp");
            ui.colored_label(color::TEXT_MUTED, "under bp");
            ui.colored_label(color::TEXT_MUTED, "max over");
            ui.colored_label(color::TEXT_MUTED, "min under");
            ui.end_row();
            for (label, channel) in [
                ("R", &report.range.red),
                ("G", &report.range.green),
                ("B", &report.range.blue),
            ] {
                ui.monospace(label);
                ui.monospace(channel.over_basis_points.to_string());
                ui.monospace(channel.under_basis_points.to_string());
                ui.monospace(millionths(channel.maximum_over_excursion_millionths));
                ui.monospace(millionths(channel.minimum_under_excursion_millionths));
                ui.end_row();
            }
        });
    ui.colored_label(
        color::TEXT_MUTED,
        format!(
            "clamped {} px · {} bp of {} visible px",
            report.range.clamped_pixel_count,
            report.range.clamped_basis_points,
            report.visible_pixel_count
        ),
    );
}

fn gamut_section(ui: &mut egui::Ui, report: &ColorQcReport) {
    section_heading(ui, "GAMUT");
    ui.horizontal_wrapped(|ui| {
        ui.monospace(format!("{} bp", report.gamut.out_of_gamut_basis_points));
        ui.colored_label(
            color::TEXT_MUTED,
            format!("{} px out of gamut", report.gamut.out_of_gamut_pixel_count),
        );
    });
    ui.horizontal_wrapped(|ui| {
        ui.colored_label(color::TEXT_MUTED, "min linear");
        ui.monospace(millionths(report.gamut.minimum_linear_millionths));
        ui.colored_label(color::TEXT_MUTED, "· max desaturation");
        ui.monospace(millionths(report.gamut.maximum_desaturation_millionths));
        ui.colored_label(
            color::TEXT_MUTED,
            format!("· below black {} px", report.gamut.below_black_pixel_count),
        );
    });
    // CC6 §14: the relation is a line, not a tooltip. Two named reports over
    // one pixel set invite double-counting in any summary written from them.
    ui.add(
        egui::Label::new(
            egui::RichText::new(report.gamut.definition.as_str()).color(color::TEXT_MUTED),
        )
        .wrap(),
    );
}

fn skin_section(ui: &mut egui::Ui, report: &ColorQcReport, region_available: bool) {
    section_heading(ui, "SKIN");
    let Some(skin) = report.skin.as_ref() else {
        ui.colored_label(
            color::TEXT_MUTED,
            if region_available {
                "No skin diagnostic in this measurement."
            } else {
                "Set a scopes ROI or expand a matte section, then measure: skin is a diagnostic \
                 of a region you name, never of the whole frame."
            },
        );
        return;
    };
    ui.horizontal_wrapped(|ui| {
        ui.colored_label(color::TEXT_MUTED, "mean hue");
        ui.monospace(
            skin.mean_hue_centidegrees
                .map_or_else(|| "—".to_owned(), centidegrees),
        );
        ui.colored_label(color::TEXT_MUTED, "· spread");
        ui.monospace(centidegrees(skin.circular_spread_centidegrees));
        ui.colored_label(color::TEXT_MUTED, "· median chroma");
        ui.monospace(millionths(skin.median_chroma_millionths));
    });
    ui.horizontal_wrapped(|ui| {
        ui.colored_label(color::TEXT_MUTED, "in band");
        ui.monospace(format!("{} bp", skin.in_band_basis_points));
        ui.colored_label(
            color::TEXT_MUTED,
            format!(
                "· band {}±{} · considered {} px · excluded achromatic {} px",
                centidegrees(skin.band_center_centidegrees),
                centidegrees(skin.band_half_width_centidegrees),
                skin.considered_pixel_count,
                skin.excluded_achromatic_pixel_count
            ),
        );
    });
    ui.add(
        egui::Label::new(egui::RichText::new(skin.boundary.as_str()).color(color::TEXT_MUTED))
            .wrap(),
    );
}

fn tags_section(ui: &mut egui::Ui, report: &ColorQcReport) {
    section_heading(ui, "TAGS");
    let Some(tags) = report.tags.as_ref() else {
        ui.colored_label(
            color::TEXT_MUTED,
            "No delivery tag check in this measurement.",
        );
        return;
    };
    ui.horizontal_wrapped(|ui| {
        ui.colored_label(color::TEXT_MUTED, "tag_source");
        ui.monospace(tags.tag_source.as_str());
        ui.colored_label(
            color::TEXT_MUTED,
            format!("· delivery depth {} bit", report.delivery_bit_depth),
        );
    });
    if tags.tag_source == kinewright_core::DeliveryTagSource::MaterialisedExportSettings.as_str() {
        // Stated rather than left to be inferred from two identical columns:
        // CC6 §3.6's pre-export mode has no independent observation, so
        // "observed" is the materialised value and the check answers "would
        // these tags be accepted at this depth?" — not "does the file match?".
        ui.add(
            egui::Label::new(egui::RichText::new(PRE_EXPORT_TAG_NOTE).color(color::TEXT_MUTED))
                .wrap(),
        );
    }
    egui::Grid::new("color-qc-tags")
        .num_columns(3)
        .striped(true)
        .show(ui, |ui| {
            ui.colored_label(color::TEXT_MUTED, "field");
            ui.colored_label(color::TEXT_MUTED, "expected");
            ui.colored_label(color::TEXT_MUTED, "observed");
            ui.end_row();
            for (field, expected, observed) in tag_field_rows(tags) {
                let tone = tag_field_color(tags, field);
                ui.colored_label(tone, field);
                ui.colored_label(tone, expected);
                ui.colored_label(tone, observed);
                ui.end_row();
            }
        });
    for line in tag_lines(tags) {
        ui.add(egui::Label::new(egui::RichText::new(line.text).color(line.color)).wrap());
    }
}

/// One rendered tag row.
pub(crate) struct TagLine {
    pub(crate) text: String,
    pub(crate) color: egui::Color32,
}

/// The eight delivery fields, in the fixed check order CC6 §3.6 pins.
///
/// Rendered whether or not they disagree: "expected vs observed per field" is
/// what the section is for, and a field that is right is evidence too.
#[must_use]
pub(crate) fn tag_field_rows(
    tags: &kinewright_core::DeliveryTagCheck,
) -> [(&'static str, String, String); 8] {
    let expected = &tags.expected;
    let observed = &tags.observed;
    [
        (
            "primaries",
            format!("{:?}", expected.primaries),
            format!("{:?}", observed.primaries),
        ),
        (
            "transfer",
            format!("{:?}", expected.transfer),
            format!("{:?}", observed.transfer),
        ),
        (
            "matrix",
            format!("{:?}", expected.matrix),
            format!("{:?}", observed.matrix),
        ),
        (
            "range",
            format!("{:?}", expected.range),
            format!("{:?}", observed.range),
        ),
        (
            "white_point",
            format!("{:?}", expected.white_point),
            format!("{:?}", observed.white_point),
        ),
        (
            "bit_depth",
            format!("{:?}", expected.bit_depth),
            format!("{:?}", observed.bit_depth),
        ),
        (
            "provenance",
            format!("{:?}", expected.provenance),
            format!("{:?}", observed.provenance),
        ),
        (
            // The wire name `delivery_color_mismatches` emits, not a prettier
            // one: `tag_field_color` matches on it, so a shortened label would
            // silently draw a real mismatch as an agreeing row.
            "confidence_basis_points",
            expected.confidence_basis_points.to_string(),
            observed.confidence_basis_points.to_string(),
        ),
    ]
}

/// The tone one field row is drawn in.
///
/// Three states, deliberately distinguishable: a mismatch is a wrong tag, a
/// not-representable field is one the container has no syntax for and is
/// **not** evidence of a wrong tag, and everything else is agreement.
#[must_use]
pub(crate) fn tag_field_color(
    tags: &kinewright_core::DeliveryTagCheck,
    field: &str,
) -> egui::Color32 {
    if tags
        .mismatches
        .iter()
        .any(|mismatch| mismatch.field == field)
    {
        return color::STATUS_DANGER;
    }
    if tags
        .not_representable
        .iter()
        .any(|entry| entry.field == field)
    {
        return color::TEXT_MUTED;
    }
    color::TEXT_SECONDARY
}

/// Every line the Tags section renders, in order.
///
/// A field the container has no syntax for is **not** a wrong tag, so a
/// not-representable row is drawn muted and labelled, visually distinct from a
/// mismatch (CC6 §3.6).
#[must_use]
pub(crate) fn tag_lines(tags: &kinewright_core::DeliveryTagCheck) -> Vec<TagLine> {
    let mut lines = Vec::new();
    if tags.conforming {
        lines.push(TagLine {
            text: "CONFORMING · every checked delivery tag matches".to_owned(),
            color: color::STATUS_SUCCESS,
        });
    }
    for mismatch in &tags.mismatches {
        lines.push(TagLine {
            text: format!(
                "MISMATCH · {} · observed {} · expected {}",
                mismatch.field, mismatch.observed, mismatch.allowed
            ),
            color: color::STATUS_DANGER,
        });
    }
    for entry in &tags.not_representable {
        lines.push(TagLine {
            text: format!(
                "NOT REPRESENTABLE · {} · expected {} · {}",
                entry.field, entry.expected, entry.reason
            ),
            color: color::TEXT_MUTED,
        });
    }
    lines
}

fn nodes_section(ui: &mut egui::Ui, report: &ColorQcReport) {
    section_heading(ui, "PER NODE");
    let Some(nodes) = report.nodes.as_ref() else {
        ui.colored_label(
            color::TEXT_MUTED,
            "Off. Turn on the per-node toggle above and measure again.",
        );
        return;
    };
    ui.colored_label(
        color::TEXT_MUTED,
        format!(
            "attribution {} · baseline {} bp range · {} bp gamut · {} candidate node(s){}",
            nodes.attribution,
            nodes.baseline_range_basis_points,
            nodes.baseline_gamut_basis_points,
            nodes.considered_node_count,
            if nodes.truncated { " · truncated" } else { "" }
        ),
    );
    egui::Grid::new("color-qc-nodes")
        .num_columns(4)
        .striped(true)
        .show(ui, |ui| {
            ui.colored_label(color::TEXT_MUTED, "node");
            ui.colored_label(color::TEXT_MUTED, "range Δbp");
            ui.colored_label(color::TEXT_MUTED, "gamut Δbp");
            ui.colored_label(color::TEXT_MUTED, "state");
            ui.end_row();
            for node in &nodes.nodes {
                ui.monospace(format!(
                    "{} · clip {} effect {}",
                    node.node_kind, node.clip.0, node.effect.0
                ));
                ui.monospace(format!("{:+}", node.range_basis_points_delta));
                ui.monospace(format!("{:+}", node.gamut_basis_points_delta));
                if node.active {
                    ui.colored_label(color::TEXT_MUTED, "active");
                } else {
                    ui.colored_label(
                        color::TEXT_MUTED,
                        node.inactive_reason
                            .as_deref()
                            .unwrap_or("inactive for this frame"),
                    );
                }
                ui.end_row();
            }
        });
}

fn exceptions_section(ui: &mut egui::Ui, report: &ColorQcReport) {
    section_heading(ui, "EXCEPTIONS");
    ui.colored_label(
        if report.technical_pass {
            color::STATUS_SUCCESS
        } else {
            color::STATUS_DANGER
        },
        if report.technical_pass {
            "technical_pass · no error-severity finding"
        } else {
            "technical_pass = false · an error-severity finding is present"
        },
    );
    if report.exceptions.is_empty() {
        ui.colored_label(color::TEXT_MUTED, "No exceptions.");
        return;
    }
    for exception in &report.exceptions {
        ui.add(
            egui::Label::new(
                egui::RichText::new(format!(
                    "{:?} · {} · {}",
                    exception.severity, exception.code, exception.message
                ))
                .color(severity_color(exception.severity)),
            )
            .wrap(),
        );
    }
}

fn provenance_footer(ui: &mut egui::Ui, measurement: &ColorQcMeasurement) {
    ui.separator();
    let render = &measurement.metadata.render;
    ui.horizontal_wrapped(|ui| {
        ui.colored_label(
            if measurement.report.full_resolution {
                color::STATUS_SUCCESS
            } else {
                color::STATUS_DANGER
            },
            if measurement.report.full_resolution {
                "FULL RESOLUTION"
            } else {
                "NOT FULL RESOLUTION"
            },
        );
        ui.colored_label(
            color::TEXT_MUTED,
            format!(
                "{}×{}",
                measurement.report.raster.0, measurement.report.raster.1
            ),
        );
        ui.colored_label(color::TEXT_MUTED, measurement.report.stage.as_str());
        ui.colored_label(
            color::TEXT_MUTED,
            format!("{} · {}", render.backend, render.adapter),
        );
        ui.colored_label(
            color::TEXT_MUTED,
            if render.gpu_claim && !render.software_fallback {
                "GPU compositor"
            } else {
                "software fallback"
            },
        );
    });
    ui.colored_label(
        color::TEXT_MUTED,
        format!(
            "{} · {} · {} · lane {} · frame {}",
            measurement.metadata.encoding,
            measurement.report.provenance.engine,
            measurement.report.provenance.accumulator_precision,
            delivery_depth_label(measurement.key.depth),
            measurement.key.frame
        ),
    );
}

#[cfg(test)]
mod tests {
    use std::sync::{Mutex, atomic::AtomicU64, mpsc};

    use kinewright_core::{
        ColorContext, ColorQcNodeContributions, Document, LinearRgbaImage, MonitorProofMetadata,
        Rational, TimeCode, WORKING_PROOF_ENCODING, WORKING_PROOF_STAGE,
    };

    use super::*;

    fn document() -> Arc<Document> {
        Arc::new(Document {
            resolution: (2, 1),
            fps: Rational::new(25, 1).unwrap(),
            color_context: ColorContext::sdr_rec709(),
            ..Document::default()
        })
    }

    /// A two-pixel linear proof: one in range, one over range.
    fn proof() -> WorkingProof {
        WorkingProof {
            image: LinearRgbaImage {
                width: 2,
                height: 1,
                pixels: vec![0.5, 0.5, 0.5, 1.0, 2.0, 0.5, 0.5, 1.0],
            },
            metadata: WorkingProofMetadata {
                render: MonitorProofMetadata::test_double(),
                stage: WORKING_PROOF_STAGE.to_owned(),
                encoding: WORKING_PROOF_ENCODING.to_owned(),
                raster_aspect_millionths: 2_000_000,
            },
        }
    }

    fn key() -> ColorQcKey {
        ColorQcKey {
            session_id: 1,
            revision: 7,
            frame: TimeCode(3),
            roi: None,
            matte: None,
            per_node: false,
            depth: DeliveryEncodeDepth::Eight,
        }
    }

    /// A source that records every call it was asked for, **counts every
    /// full-resolution render it performed**, and can be held open until the
    /// test releases it.
    ///
    /// The render count is the point: CC6 §3.7's cost is renders, and the only
    /// way to prove the window does not pay for the same frame twice is to
    /// count them. Each render stamps its ordinal into the proof's metadata,
    /// so a report can be traced to the exact render it was measured on.
    struct RecordingSource {
        calls: Arc<Mutex<Vec<&'static str>>>,
        renders: Arc<AtomicU64>,
        /// How many colour nodes the per-node pass finds. It renders one
        /// baseline plus one removal per candidate, exactly as core does.
        candidates: usize,
        gate: Option<Mutex<mpsc::Receiver<()>>>,
    }

    impl RecordingSource {
        fn new(calls: &Arc<Mutex<Vec<&'static str>>>) -> Self {
            Self {
                calls: Arc::clone(calls),
                renders: Arc::new(AtomicU64::new(0)),
                candidates: 0,
                gate: None,
            }
        }

        /// One full-resolution render, stamped with its own ordinal.
        fn render(&self) -> WorkingProof {
            let ordinal = self.renders.fetch_add(1, Ordering::AcqRel) + 1;
            let mut proof = proof();
            proof.metadata.raster_aspect_millionths = i64::try_from(ordinal).unwrap_or(i64::MAX);
            proof
        }

        fn render_count(&self) -> u64 {
            self.renders.load(Ordering::Acquire)
        }
    }

    impl ColorQcSource for RecordingSource {
        fn working_proof(
            &self,
            _document: Arc<Document>,
            _key: WorkingProofKey,
        ) -> Result<WorkingProof, String> {
            if let Some(gate) = &self.gate {
                let _ = gate.lock().unwrap().recv();
            }
            self.calls.lock().unwrap().push("working_proof");
            Ok(self.render())
        }

        fn measure_with_nodes(
            &self,
            _document: Arc<Document>,
            _key: WorkingProofKey,
            request: &ColorQcRequest,
        ) -> Result<(ColorQcReport, WorkingProofMetadata), String> {
            self.calls.lock().unwrap().push("measure_with_nodes");
            // The shape of CC6 §3.7's pass: one baseline render, then one
            // render per candidate, every one of them counted.
            let baseline = self.render();
            let mut report =
                measure_color_qc(&baseline, request).map_err(|error| error.to_string())?;
            let baseline_range = report.range.clamped_basis_points;
            let baseline_gamut = report.gamut.out_of_gamut_basis_points;
            let nodes = (0..self.candidates)
                .map(|index| kinewright_core::ColorNodeQcContribution {
                    clip: ClipId(1),
                    effect: EffectId(u64::try_from(index).unwrap_or(u64::MAX) + 1),
                    node_kind: "primary_correction".to_owned(),
                    active: true,
                    inactive_reason: None,
                    range_basis_points_delta: 0,
                    gamut_basis_points_delta: 0,
                })
                .inspect(|_| {
                    let _ = self.render();
                })
                .collect::<Vec<_>>();
            kinewright_core::attach_node_contributions(
                &mut report,
                ColorQcNodeContributions {
                    baseline_range_basis_points: baseline_range,
                    baseline_gamut_basis_points: baseline_gamut,
                    considered_node_count: u32::try_from(self.candidates).unwrap_or(u32::MAX),
                    truncated: false,
                    attribution: kinewright_core::NODE_ATTRIBUTION_REMOVED.to_owned(),
                    nodes,
                },
            );
            Ok((report, baseline.metadata))
        }
    }

    /// A source whose render panics, to prove a worker that unwinds still
    /// resolves the window.
    struct PanickingSource;

    impl ColorQcSource for PanickingSource {
        fn working_proof(
            &self,
            _document: Arc<Document>,
            _key: WorkingProofKey,
        ) -> Result<WorkingProof, String> {
            panic!("the renderer fell over");
        }

        fn measure_with_nodes(
            &self,
            _document: Arc<Document>,
            _key: WorkingProofKey,
            _request: &ColorQcRequest,
        ) -> Result<(ColorQcReport, WorkingProofMetadata), String> {
            panic!("the renderer fell over");
        }
    }

    fn request() -> ColorQcRequest {
        ColorQcRequest {
            project_frame: 3,
            ..ColorQcRequest::default()
        }
    }

    fn poll_until(state: &mut ColorQcState, mut done: impl FnMut(&ColorQcState) -> bool) {
        for _ in 0..2_000 {
            state.poll();
            if done(state) {
                return;
            }
            thread::sleep(std::time::Duration::from_millis(1));
        }
        panic!("the colour QC worker never settled");
    }

    /// CC6 §8.1: the window measures and reports. It has no edit path, and the
    /// only thing it ever asks the backend for is a proof.
    #[test]
    fn measurement_has_no_operation_side_effect() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let source = Arc::new(RecordingSource::new(&calls));
        let document = document();
        let before = (*document).clone();
        let mut state = ColorQcState::default();
        state.observe_context(1, 7, TimeCode(3));
        state.request_measurement_from(
            source,
            Arc::default(),
            None,
            Arc::clone(&document),
            key(),
            request(),
        );
        poll_until(&mut state, |state| state.current().is_some());

        let measurement = state.current().expect("a measurement landed");
        assert!(
            measurement.report.evidence_only,
            "a colour QC report is evidence only"
        );
        assert_eq!(
            *calls.lock().unwrap(),
            vec!["working_proof"],
            "measuring asks for a proof and nothing else: no render, no apply, no operation"
        );
        // The state type has no operation channel at all, so the strongest
        // observable claim is that the document it was handed is untouched.
        assert_eq!(
            *document, before,
            "the snapshot the worker measured is unchanged afterwards"
        );
    }

    /// CC6 §8.1: single flight. A request during a render waits for the
    /// running worker rather than standing a second `FrameRenderer` beside it.
    #[test]
    fn a_request_during_a_render_waits_for_the_running_worker() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let (release, gate) = mpsc::channel();
        let blocking = Arc::new(RecordingSource {
            gate: Some(Mutex::new(gate)),
            ..RecordingSource::new(&calls)
        });
        let free = Arc::new(RecordingSource::new(&calls));
        let document = document();
        let mut state = ColorQcState::default();
        state.observe_context(1, 7, TimeCode(3));
        state.request_measurement_from(
            blocking,
            Arc::default(),
            None,
            Arc::clone(&document),
            key(),
            request(),
        );
        let second = ColorQcKey {
            frame: TimeCode(4),
            ..key()
        };
        state.observe_context(1, 7, TimeCode(4));
        state.request_measurement_from(free, Arc::default(), None, document, second, request());
        assert_eq!(
            state.spawned_workers(),
            1,
            "the second request parks instead of starting a second renderer"
        );
        // Let the first worker finish; only then may the parked one start.
        let _ = release.send(());
        poll_until(&mut state, |state| state.current().is_some());
        assert_eq!(
            state.spawned_workers(),
            2,
            "the parked request starts once the running worker has finished"
        );
        assert_eq!(
            state.current().expect("a measurement landed").key.frame,
            TimeCode(4),
            "the newest request is the one that answers"
        );
    }

    /// CC6 §3.7 and §11.2.21: the per-node measurement is **one** pass.
    ///
    /// Sixteen candidates cost one baseline render plus sixteen removals —
    /// seventeen. Measuring the report separately and then asking for the
    /// attribution rendered the same baseline twice, so the toggle's own label
    /// ("up to 17 full-resolution frames") was a render short of the truth.
    #[test]
    fn the_per_node_measurement_renders_one_baseline_and_one_frame_per_candidate() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let source = Arc::new(RecordingSource {
            candidates: kinewright_core::MAX_QC_NODE_CONTRIBUTIONS,
            ..RecordingSource::new(&calls)
        });
        let per_node = ColorQcKey {
            per_node: true,
            ..key()
        };
        let mut state = ColorQcState::default();
        state.observe_context(1, 7, TimeCode(3));
        state.request_measurement_from(
            Arc::clone(&source) as Arc<dyn ColorQcSource>,
            Arc::default(),
            None,
            document(),
            per_node,
            request(),
        );
        poll_until(&mut state, |state| state.current().is_some());

        assert_eq!(
            *calls.lock().unwrap(),
            vec!["measure_with_nodes"],
            "the window asks once: a second `working_proof` here is a wasted \
             full-resolution render of a frame it is already rendering"
        );
        assert_eq!(
            source.render_count(),
            1 + kinewright_core::MAX_QC_NODE_CONTRIBUTIONS as u64,
            "seventeen renders for sixteen candidates, not eighteen"
        );
        let measurement = state.current().expect("a measurement landed");
        assert!(
            measurement.report.nodes.is_some(),
            "the attribution is attached to the report it belongs to"
        );
        assert_eq!(
            measurement.metadata.raster_aspect_millionths, 1,
            "the report and the provenance line describe the first render — the \
             baseline the attribution was measured against"
        );

        // And with the toggle off, exactly one render answers the whole window.
        let calls = Arc::new(Mutex::new(Vec::new()));
        let source = Arc::new(RecordingSource::new(&calls));
        let mut state = ColorQcState::default();
        state.observe_context(1, 7, TimeCode(3));
        state.request_measurement_from(
            Arc::clone(&source) as Arc<dyn ColorQcSource>,
            Arc::default(),
            None,
            document(),
            key(),
            request(),
        );
        poll_until(&mut state, |state| state.current().is_some());
        assert_eq!(*calls.lock().unwrap(), vec!["working_proof"]);
        assert_eq!(source.render_count(), 1);
        assert!(
            state
                .current()
                .expect("a measurement landed")
                .report
                .nodes
                .is_none(),
            "and it carries no attribution nobody asked for"
        );
    }

    /// CC6 §8.1/§8.2: the window and the viewer's clipping mask measure the
    /// same raster at the same frame. One render answers both.
    ///
    /// Each surface owns its own worker, and each worker owns its own
    /// `FrameRenderer`: without the shared cache, having the mask on and
    /// pressing Measure stood two full-resolution renderers side by side for
    /// one identical frame.
    #[test]
    fn one_render_feeds_both_the_qc_window_and_the_clipping_mask() {
        use crate::preview_ui::{
            QcMaskConditions, QcMaskKey, QcMaskState, QcMaskStatus, QcMaskView,
        };

        let calls = Arc::new(Mutex::new(Vec::new()));
        let source = Arc::new(RecordingSource::new(&calls));
        let cache = Arc::new(WorkingProofCache::default());
        let document = document();

        let mut window = ColorQcState::default();
        window.observe_context(1, 7, TimeCode(3));
        window.request_measurement_from(
            Arc::clone(&source) as Arc<dyn ColorQcSource>,
            Arc::clone(&cache),
            None,
            Arc::clone(&document),
            key(),
            request(),
        );
        poll_until(&mut window, |window| window.current().is_some());
        assert_eq!(source.render_count(), 1, "the window rendered the frame");

        let mask_key = QcMaskKey {
            session_id: key().session_id,
            revision: key().revision,
            frame: key().frame,
        };
        let conditions = QcMaskConditions::default();
        let mut mask = QcMaskState::default();
        mask.set_view(QcMaskView::Clipping);
        assert!(
            mask.request_view_if_needed(
                conditions,
                Arc::clone(&source) as Arc<dyn ColorQcSource>,
                Arc::clone(&cache),
                document,
                mask_key,
            ),
            "the mask has no picture yet, so it asks for one"
        );
        for _ in 0..2_000 {
            mask.poll();
            if matches!(mask.status(mask_key, conditions), QcMaskStatus::Ready) {
                break;
            }
            thread::sleep(std::time::Duration::from_millis(1));
        }
        assert_eq!(
            mask.status(mask_key, conditions),
            QcMaskStatus::Ready,
            "the mask is drawn from the proof the window already rendered"
        );
        assert_eq!(
            source.render_count(),
            1,
            "and it cost no second full-resolution render"
        );

        // A different frame is a different proof, and is rendered.
        let moved = QcMaskKey {
            frame: TimeCode(4),
            ..mask_key
        };
        mask.request_view(
            Arc::clone(&source) as Arc<dyn ColorQcSource>,
            cache,
            Arc::new(Document::default()),
            moved,
        );
        for _ in 0..2_000 {
            mask.poll();
            if matches!(mask.status(moved, conditions), QcMaskStatus::Ready) {
                break;
            }
            thread::sleep(std::time::Duration::from_millis(1));
        }
        assert_eq!(
            source.render_count(),
            2,
            "the cache shares one frame, it does not answer for another"
        );
    }

    /// The shared proof lives for the frame under the playhead and no longer.
    ///
    /// A full-resolution scene-linear raster is far too large to keep for a
    /// frame nobody is looking at, and the frame loop is the only thing that
    /// knows the editor has moved on.
    #[test]
    fn the_shared_proof_is_dropped_once_the_editor_moves_on() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let source = Arc::new(RecordingSource::new(&calls));
        let cache = WorkingProofCache::default();
        let live = key().proof_key();

        let first = cache
            .proof(source.as_ref(), document(), live)
            .expect("the first call renders");
        assert_eq!(source.render_count(), 1);
        let again = cache
            .proof(source.as_ref(), document(), live)
            .expect("the second is served");
        assert_eq!(
            source.render_count(),
            1,
            "the same frame is not re-rendered"
        );
        assert!(Arc::ptr_eq(&first, &again), "and it is the same proof");

        cache.retain_context(live);
        let _ = cache
            .proof(source.as_ref(), document(), live)
            .expect("still the live frame");
        assert_eq!(
            source.render_count(),
            1,
            "the live frame's proof survives the frame loop"
        );

        cache.retain_context(WorkingProofKey {
            frame: TimeCode(4),
            ..live
        });
        let _ = cache
            .proof(source.as_ref(), document(), live)
            .expect("renders again");
        assert_eq!(
            source.render_count(),
            2,
            "a proof for a frame nobody is looking at is dropped, not kept"
        );
    }

    /// A worker that unwinds still resolves the window: with an error, with
    /// nothing pending, and with no measurement claiming to describe anything.
    ///
    /// Without `WorkerCompletion`'s `Drop`, a panicking render sent nothing at
    /// all and the window sat on "Rendering full-resolution working proof…"
    /// forever, with no request in flight and no way to retry.
    /// The shared cache is deliberately carried across both halves: a render
    /// that unwinds does so *inside* `WorkingProofCache`'s lock and poisons it,
    /// and this is a cache — the next reader must re-render through it rather
    /// than inherit the panic and wedge every QC surface in the session. With a
    /// fresh cache per case the recovery was never exercised at all.
    #[test]
    fn a_panicking_render_resolves_the_window_with_an_error() {
        let cache = Arc::new(WorkingProofCache::default());
        for per_node in [false, true] {
            let mut state = ColorQcState::default();
            state.observe_context(1, 7, TimeCode(3));
            state.request_measurement_from(
                Arc::new(PanickingSource),
                Arc::clone(&cache),
                None,
                document(),
                ColorQcKey { per_node, ..key() },
                request(),
            );
            poll_until(&mut state, |state| state.error().is_some());
            assert!(
                !state.is_pending(),
                "nothing is in flight, so nothing may claim to be pending"
            );
            assert!(
                state.current().is_none(),
                "and no measurement is shown for a render that never finished"
            );
            let error = state.error().expect("the failure is reported");
            assert!(
                error.contains("stopped before it delivered"),
                "the message says what happened: {error}"
            );
        }

        // The same cache, now poisoned by the panic that unwound through its
        // lock, still serves the next measurement.
        let calls = Arc::new(Mutex::new(Vec::new()));
        let source = Arc::new(RecordingSource::new(&calls));
        let mut state = ColorQcState::default();
        state.observe_context(1, 7, TimeCode(3));
        state.request_measurement_from(
            Arc::clone(&source) as Arc<dyn ColorQcSource>,
            Arc::clone(&cache),
            None,
            document(),
            key(),
            request(),
        );
        poll_until(&mut state, |state| {
            state.current().is_some() || state.error().is_some()
        });
        assert_eq!(
            state.error(),
            None,
            "a poisoned cache is a cache whose last writer panicked, not a broken session"
        );
        assert!(
            state.current().is_some(),
            "the next measurement lands through the same shared cache"
        );
        assert_eq!(source.render_count(), 1, "and it really did render");
    }

    /// One measurement with every optional section present.
    fn full_measurement() -> ColorQcMeasurement {
        let request = ColorQcRequest {
            checks: vec![
                ColorQcCheck::Range,
                ColorQcCheck::Gamut,
                ColorQcCheck::Tags,
                ColorQcCheck::Skin,
            ],
            delivery_bit_depth: DeliveryEncodeDepth::Eight,
            expected_delivery: Some(ColorContext::sdr_rec709().delivery),
            max_nodes: 16,
            project_frame: 3,
            ..ColorQcRequest::default()
        };
        let proof = proof();
        let mut report = measure_color_qc(&proof, &request).expect("the fixture measures");
        let baseline_range = report.range.clamped_basis_points;
        let baseline_gamut = report.gamut.out_of_gamut_basis_points;
        kinewright_core::attach_node_contributions(
            &mut report,
            ColorQcNodeContributions {
                baseline_range_basis_points: baseline_range,
                baseline_gamut_basis_points: baseline_gamut,
                considered_node_count: 1,
                truncated: false,
                attribution: kinewright_core::NODE_ATTRIBUTION_REMOVED.to_owned(),
                nodes: vec![kinewright_core::ColorNodeQcContribution {
                    clip: ClipId(1),
                    effect: EffectId(2),
                    node_kind: "primary_correction".to_owned(),
                    active: true,
                    inactive_reason: None,
                    range_basis_points_delta: 903,
                    gamut_basis_points_delta: 41,
                }],
            },
        );
        ColorQcMeasurement {
            key: ColorQcKey {
                per_node: true,
                ..key()
            },
            report,
            metadata: proof.metadata,
        }
    }

    /// CC6 §8.1: the window body draws, headless, and every section it
    /// promises is on screen with the integers it measured.
    ///
    /// Rendered rather than asserted section by section: the body is the one
    /// place all seven sections have to coexist, and a panic in a `Grid` or a
    /// missing section is invisible to a test of the values alone.
    #[test]
    fn the_colour_qc_window_body_renders_every_section() {
        let measurement = full_measurement();
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let output = ctx.run_ui(egui::RawInput::default(), |ui| {
            color_qc_sections(ui, &measurement);
        });
        let painted = crate::theme::painted_text(&output).join("\n");

        for heading in ["RANGE", "GAMUT", "SKIN", "TAGS", "PER NODE", "EXCEPTIONS"] {
            assert!(
                painted.contains(heading),
                "the {heading} section is missing from the window body:\n{painted}"
            );
        }
        assert!(
            painted.contains("Region measured:"),
            "the region line names what was measured:\n{painted}"
        );
        assert!(
            painted.contains(&format!(
                "clamped {} px",
                measurement.report.range.clamped_pixel_count
            )),
            "the range section prints its own integers:\n{painted}"
        );
        assert!(
            painted.contains(PRE_EXPORT_TAG_NOTE),
            "the pre-export note is one sentence, drawn whole:\n{painted}"
        );
        assert!(
            !painted.contains("value                      as"),
            "and it carries no embedded run of spaces — egui does not collapse them"
        );
        assert!(
            painted.contains("primary_correction · clip 1 effect 2"),
            "the per-node grid names each node:\n{painted}"
        );
        assert!(
            painted.contains("+903"),
            "with the deltas it measured:\n{painted}"
        );
        assert!(
            painted.contains(measurement.report.stage.as_str()),
            "and the provenance footer says which stage it measured:\n{painted}"
        );
    }

    /// CC6 §8.3: absent when both deltas are non-positive, present with the
    /// frame when either is positive.
    #[test]
    fn the_node_header_line_is_absent_when_both_deltas_are_non_positive() {
        let clipping = ColorQcNodeClipping {
            frame: 12,
            entries: vec![
                (ClipId(1), EffectId(1), 0, 0),
                (ClipId(1), EffectId(2), -4, 0),
                (ClipId(1), EffectId(3), 903, 41),
            ],
        };
        assert_eq!(
            clipping.line_for(ClipId(1), EffectId(1)),
            None,
            "a node that removed no clipping has nothing to report"
        );
        assert_eq!(
            clipping.line_for(ClipId(1), EffectId(2)),
            None,
            "a negative delta is not a contribution"
        );
        assert_eq!(
            clipping.line_for(ClipId(1), EffectId(4)),
            None,
            "a node the report does not name has no line"
        );
        assert_eq!(
            clipping.line_for(ClipId(1), EffectId(3)).as_deref(),
            Some("Clipping contribution: +903 bp range · +41 bp gamut (frame 12)"),
            "the frame is shown so a stale reading is visible"
        );
        assert_eq!(
            ColorQcNodeClipping::default().line_for(ClipId(1), EffectId(3)),
            None,
            "no report means no line"
        );

        // One positive and one negative: the line appears, and the negative
        // delta is shown as the negative it is rather than clamped to `+0`.
        let mixed = ColorQcNodeClipping {
            frame: 12,
            entries: vec![(ClipId(2), EffectId(1), 5, -3)],
        };
        assert_eq!(
            mixed.line_for(ClipId(2), EffectId(1)).as_deref(),
            Some("Clipping contribution: +5 bp range · -3 bp gamut (frame 12)"),
            "removing this node made the gamut worse, and the line says so"
        );
    }

    /// CC6 §3.6/§8.1: a field the container has no syntax for is not a wrong
    /// tag, and the two must not look alike.
    #[test]
    fn a_not_representable_tag_row_is_visually_distinct_from_a_mismatch() {
        let expected = kinewright_core::ColorContext::sdr_rec709().delivery;
        let observed = kinewright_core::ColorDescription {
            primaries: kinewright_core::ColorPrimaries::Bt2020,
            white_point: kinewright_core::ColorWhitePoint::Unknown,
            provenance: kinewright_core::ColorProvenance::StreamMetadata,
            ..expected.clone()
        };
        let tags = kinewright_core::delivery_tag_check(
            &expected,
            &observed,
            kinewright_core::DeliveryTagSource::ProbedOutputFile,
        );

        let rows = tag_field_rows(&tags);
        assert_eq!(
            rows.iter().map(|(field, _, _)| *field).collect::<Vec<_>>(),
            vec![
                "primaries",
                "transfer",
                "matrix",
                "range",
                "white_point",
                "bit_depth",
                "provenance",
                "confidence_basis_points",
            ],
            "every checked field is rendered, in the fixed check order"
        );

        let mismatch = tag_field_color(&tags, "primaries");
        let not_representable = tag_field_color(&tags, "white_point");
        let agreeing = tag_field_color(&tags, "transfer");
        assert_eq!(mismatch, color::STATUS_DANGER);
        assert_eq!(not_representable, color::TEXT_MUTED);
        assert_eq!(agreeing, color::TEXT_SECONDARY);
        assert_ne!(
            mismatch, not_representable,
            "a field H.264 cannot carry must not read as a wrong tag"
        );
        assert_ne!(agreeing, mismatch);
        assert_ne!(agreeing, not_representable);

        // `provenance` legitimately differs on a probed description and is
        // deliberately excluded from the mismatch list (CC6 §3.6).
        assert_eq!(
            tag_field_color(&tags, "provenance"),
            color::TEXT_SECONDARY,
            "a probed description is correct to carry StreamMetadata"
        );

        let lines = tag_lines(&tags);
        assert!(
            lines.iter().any(|line| line.text.starts_with("MISMATCH")
                && line.color == color::STATUS_DANGER)
        );
        assert!(
            lines
                .iter()
                .any(|line| line.text.starts_with("NOT REPRESENTABLE")
                    && line.color == color::TEXT_MUTED)
        );
    }

    /// The window only ever runs the **pre-export** mode, so that is the mode
    /// the grid has to be right in.
    ///
    /// Every field name `delivery_color_mismatches` can emit must appear in
    /// the grid, or `tag_field_color`'s string match silently draws a real
    /// mismatch as an agreeing row — which is exactly what a shortened
    /// `"confidence"` label did.
    #[test]
    fn every_pre_export_mismatch_field_is_coloured_in_the_grid() {
        // One description that trips every check in the fixed order.
        //
        // The transfer is `Bt1886` rather than `Smpte2084` because CC8 §5.3
        // makes the accepted set a function of the delivery **lane**:
        // `Bt2020` + `Smpte2084` is one of §2.1's HDR pairs and selects §5.1's
        // HDR lane, where `primaries=bt2020` is correct and only seven checks
        // trip. `Bt2020` + `Bt1886` is a mismatched pair (§5.3's third
        // bullet), stays on the SDR lane, and still trips all eight. The grid
        // colours by *field name* and both lanes report the same eight field
        // names, so this fixture covers the HDR lane too.
        let broken = kinewright_core::ColorDescription {
            primaries: kinewright_core::ColorPrimaries::Bt2020,
            transfer: kinewright_core::ColorTransfer::Bt1886,
            matrix: kinewright_core::ColorMatrix::Smpte170M,
            range: kinewright_core::ColorRange::Full,
            white_point: kinewright_core::ColorWhitePoint::Unknown,
            bit_depth: kinewright_core::ColorBitDepth::Twelve,
            provenance: kinewright_core::ColorProvenance::Unknown,
            confidence_basis_points: 0,
        };
        let mismatches = kinewright_core::delivery_color_mismatches(&broken);
        assert_eq!(
            mismatches.len(),
            8,
            "the fixture trips every check, so nothing in the grid is untested"
        );

        // Pre-export mode: `expected` and `observed` are the same materialised
        // value, exactly as `measure_color_qc` builds it.
        let tags = kinewright_core::delivery_tag_check(
            &broken,
            &broken,
            kinewright_core::DeliveryTagSource::MaterialisedExportSettings,
        );
        assert_eq!(tags.tag_source, "materialised_export_settings");
        assert!(tags.not_representable.is_empty(), "nothing was probed");

        let rows = tag_field_rows(&tags);
        let fields: Vec<&str> = rows.iter().map(|(field, _, _)| *field).collect();
        for mismatch in &mismatches {
            assert!(
                fields.contains(&mismatch.field.as_str()),
                "{} has no row in the grid, so its mismatch would be invisible there",
                mismatch.field
            );
            assert_eq!(
                tag_field_color(&tags, &mismatch.field),
                color::STATUS_DANGER,
                "{} is a mismatch and must be drawn as one",
                mismatch.field
            );
        }

        // And a conforming description colours nothing red.
        let managed = kinewright_core::ColorContext::sdr_rec709().delivery;
        let conforming = kinewright_core::delivery_tag_check(
            &managed,
            &managed,
            kinewright_core::DeliveryTagSource::MaterialisedExportSettings,
        );
        assert!(conforming.conforming);
        for (field, expected, observed) in tag_field_rows(&conforming) {
            assert_ne!(tag_field_color(&conforming, field), color::STATUS_DANGER);
            assert_eq!(
                expected, observed,
                "pre-export mode has nothing probed, so both columns carry the \
                 materialised value"
            );
        }
    }

    #[test]
    fn millionths_and_centidegrees_render_as_signed_integers() {
        assert_eq!(millionths(1_500_000), "1.500000");
        assert_eq!(millionths(-24_902), "-0.024902");
        assert_eq!(millionths(0), "0.000000");
        assert_eq!(centidegrees(12_339), "123.39°");
        assert_eq!(centidegrees(-100), "-1.00°");
    }
}
