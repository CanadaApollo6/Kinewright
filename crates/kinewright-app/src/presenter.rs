//! PF1 R-2/R-5: the final consumer's frame validation, the display cell that
//! describes the bound preview image, and the paint marker whose mark
//! `App::logic` acks at a later root epoch.

use std::{
    sync::{Arc, Mutex, PoisonError},
    time::Instant,
};

use eframe::{egui, egui_wgpu};
use kinewright_core::{FrameStamp, PreviewFrame, TimeCode};

/// What the preview texture shows now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DisplayCell {
    pub(crate) stamp: FrameStamp,
    pub(crate) at: TimeCode,
    /// Bumped by every binding: one ack per bound image.
    pub(crate) frame_id: u64,
    /// An image of an older epoch, kept up (marked) until a current one binds.
    pub(crate) stale: bool,
}

/// A paint of a bound, current image in root epoch `epoch`, at `painted`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PaintMark {
    pub(crate) cell: DisplayCell,
    pub(crate) epoch: u64,
    pub(crate) painted: Instant,
}

type Shared<T> = Arc<Mutex<Option<T>>>;

/// `Playback::stamp`, read at paint time.
pub(crate) type Latest = Box<dyn Fn() -> FrameStamp + Send + Sync>;

fn read<T: Copy>(shared: &Shared<T>) -> Option<T> {
    *shared.lock().unwrap_or_else(PoisonError::into_inner)
}

fn write<T>(shared: &Shared<T>, value: Option<T>) -> Option<T> {
    std::mem::replace(
        &mut shared.lock().unwrap_or_else(PoisonError::into_inner),
        value,
    )
}

#[derive(Default)]
pub(crate) struct Presenter {
    candidates: Vec<PreviewFrame>,
    cell: Shared<DisplayCell>,
    mark: Shared<PaintMark>,
    next_frame_id: u64,
    acked: Option<u64>,
}

/// R-2 selection: the newest candidate of the current epoch with seq ≥ the
/// shown seq and, while playing, `position − 1 < at ≤ position`. Candidates
/// still ahead of the clock stay; the rest are consumed (expired ones are
/// never shown: the engine counts them dropped).
pub(crate) fn choose_candidate(
    candidates: &mut Vec<PreviewFrame>,
    latest: FrameStamp,
    shown: Option<FrameStamp>,
    playing: Option<TimeCode>,
) -> Option<PreviewFrame> {
    let floor = shown.filter(|shown| shown.epoch == latest.epoch);
    let qualifies = |frame: &PreviewFrame| {
        frame.stamp.epoch == latest.epoch
            && floor.is_none_or(|shown| frame.stamp.seq >= shown.seq)
            && playing.is_none_or(|position| frame.at == position)
    };
    let mut chosen: Option<PreviewFrame> = None;
    let mut kept = Vec::new();
    for frame in candidates.drain(..) {
        if qualifies(&frame) {
            if chosen
                .as_ref()
                .is_none_or(|best| frame.stamp.seq >= best.stamp.seq)
            {
                chosen = Some(frame);
            }
        } else if frame.stamp.epoch == latest.epoch && playing.is_some_and(|p| frame.at > p) {
            kept.push(frame);
        }
    }
    *candidates = kept;
    chosen
}

impl Presenter {
    /// `poll_background` only collects (bounded: the channel holds two).
    pub(crate) fn collect(&mut self, frame: PreviewFrame) {
        if self.candidates.len() >= 4 {
            self.candidates.remove(0);
        }
        self.candidates.push(frame);
    }

    /// `finalize_preview` (review A F2): choose a candidate under `now`
    /// (the latest stamp and, while playing, the clock), `prepare` its CPU
    /// image, then re-check it under `now` again. Only a still-eligible
    /// frame is returned with its image, the cell describing it written in
    /// the same step, for the caller to bind at once; otherwise an
    /// older-epoch image turns stale.
    pub(crate) fn finalize<P>(
        &mut self,
        now: impl Fn() -> (FrameStamp, Option<TimeCode>),
        prepare: impl FnOnce(&PreviewFrame) -> P,
    ) -> Option<(PreviewFrame, P)> {
        let shown = read(&self.cell);
        let (latest, playing) = now();
        let chosen = choose_candidate(
            &mut self.candidates,
            latest,
            shown.map(|c| c.stamp),
            playing,
        );
        let prepared = chosen.map(|frame| {
            let image = prepare(&frame);
            (frame, image)
        });
        let (latest, playing) = now();
        let chosen = prepared.filter(|(frame, _)| {
            frame.stamp.epoch == latest.epoch && playing.is_none_or(|position| frame.at == position)
        });
        let cell = match (&chosen, shown) {
            (Some((frame, _)), _) => {
                self.next_frame_id += 1;
                let (stamp, at, frame_id) = (frame.stamp, frame.at, self.next_frame_id);
                DisplayCell {
                    stamp,
                    at,
                    frame_id,
                    stale: false,
                }
            }
            (None, Some(cell)) if cell.stamp.epoch < latest.epoch => DisplayCell {
                stale: true,
                ..cell
            },
            _ => return None,
        };
        write(&self.cell, Some(cell));
        chosen
    }

    /// The texture was dropped or replaced by something that is not a frame.
    pub(crate) fn clear(&mut self) {
        self.candidates.clear();
        write(&self.cell, None);
        write(&self.mark, None);
    }

    pub(crate) fn stale(&self) -> bool {
        read(&self.cell).is_some_and(|cell| cell.stale)
    }

    /// Review A F4: the bound image is `target`, current under `latest` and
    /// no older than `since`.
    pub(crate) fn shows(&self, since: FrameStamp, target: TimeCode, latest: FrameStamp) -> bool {
        read(&self.cell).is_some_and(|cell| {
            !cell.stale
                && cell.at == target
                && cell.stamp.epoch == latest.epoch
                && cell.stamp.seq >= since.seq
        })
    }

    /// The marker for this layout pass of root epoch `epoch`: it holds the
    /// cell itself, never a copied stamp, so a later binding in the same pass
    /// is what it marks.
    pub(crate) fn marker(&self, epoch: u64, latest: Latest) -> PaintMarker {
        let (cell, mark) = (Arc::clone(&self.cell), Arc::clone(&self.mark));
        PaintMarker {
            cell,
            mark,
            epoch,
            latest,
        }
    }

    /// `App::logic` at root epoch `now`: a mark from an earlier epoch is
    /// acked once per bound image.
    pub(crate) fn take_ack(&mut self, now: u64) -> Option<(FrameStamp, TimeCode, Instant)> {
        let mark = read(&self.mark).filter(|mark| mark.epoch < now)?;
        write(&self.mark, None);
        let cell = mark.cell;
        (self.acked != Some(cell.frame_id)).then(|| {
            self.acked = Some(cell.frame_id);
            (cell.stamp, cell.at, mark.painted)
        })
    }
}

pub(crate) struct PaintMarker {
    cell: Shared<DisplayCell>,
    mark: Shared<PaintMark>,
    epoch: u64,
    latest: Latest,
}

impl PaintMarker {
    /// The paint: marks the bound image unless it is stale or a seek issued
    /// since has made its epoch old.
    pub(crate) fn record(&self) {
        let latest = (self.latest)();
        if let Some(cell) = read(&self.cell)
            && !cell.stale
            && cell.stamp.epoch >= latest.epoch
        {
            write(
                &self.mark,
                Some(PaintMark {
                    cell,
                    epoch: self.epoch,
                    painted: Instant::now(),
                }),
            );
        }
    }

    /// Mark over `image_rect`, clipped to it within `painter`'s clip (review
    /// A F3): a viewer whose visible part misses the image paints nothing.
    pub(crate) fn add_to(self, painter: &egui::Painter, image_rect: egui::Rect) {
        let callback = egui_wgpu::Callback::new_paint_callback(image_rect, self);
        painter
            .with_clip_rect(image_rect)
            .add(egui::Shape::Callback(callback));
    }
}

/// egui-wgpu calls `paint` only for a non-empty clip, after the surface
/// texture is acquired, and then submits; an abandoned frame never calls it.
impl egui_wgpu::CallbackTrait for PaintMarker {
    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        _render_pass: &mut egui_wgpu::wgpu::RenderPass<'static>,
        _resources: &egui_wgpu::CallbackResources,
    ) {
        self.record();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use kinewright_core::FrameTexture;

    use super::*;

    fn frame(epoch: u64, seq: u64, at: i64) -> PreviewFrame {
        let rgba = Arc::new(vec![0; 4]);
        let texture = FrameTexture {
            width: 1,
            height: 1,
            rgba,
        };
        let stamp = FrameStamp { epoch, seq };
        PreviewFrame {
            at: TimeCode(at),
            stamp,
            texture,
        }
    }

    /// The engine's latest epoch, as the marker reads it at paint time.
    fn engine_epoch(epoch: u64) -> (Arc<AtomicU64>, impl Fn() -> Latest) {
        let shared = Arc::new(AtomicU64::new(epoch));
        let reader = Arc::clone(&shared);
        let latest = move || -> Latest {
            let reader = Arc::clone(&reader);
            Box::new(move || FrameStamp {
                epoch: reader.load(Ordering::Acquire),
                seq: 0,
            })
        };
        (shared, latest)
    }

    fn acked(at: i64, epoch: u64, seq: u64) -> (FrameStamp, TimeCode) {
        (FrameStamp { epoch, seq }, TimeCode(at))
    }

    /// `finalize` with nothing moving between choice and binding.
    fn fin(
        presenter: &mut Presenter,
        latest: FrameStamp,
        playing: Option<TimeCode>,
    ) -> Option<PreviewFrame> {
        let now = move || (latest, playing);
        presenter.finalize(now, |_| ()).map(|(frame, ())| frame)
    }

    fn ack(presenter: &mut Presenter, now: u64) -> Option<(FrameStamp, TimeCode)> {
        presenter.take_ack(now).map(|(stamp, at, _)| (stamp, at))
    }

    /// I18 witnesses. A-layout/B-bind: the marker laid out before a binding
    /// in the same pass marks the binding. C-deferred: no binding leaves the
    /// held image, already acked, so no second ack. Seek-before-paint: an
    /// epoch bump between binding and paint records nothing; the image then
    /// turns stale, is marked on screen, and is never acked.
    #[test]
    fn the_marker_marks_what_is_bound_when_it_paints() {
        let (epoch, latest) = engine_epoch(1);
        let mut presenter = Presenter::default();
        presenter.collect(frame(1, 1, 5));
        assert!(fin(&mut presenter, FrameStamp { epoch: 1, seq: 1 }, None).is_some());
        let marker = presenter.marker(1, latest()); // A: layout of pass 1…
        presenter.collect(frame(1, 2, 6));
        let now = FrameStamp { epoch: 1, seq: 2 };
        assert!(fin(&mut presenter, now, None).is_some()); // …B: a binding.
        marker.record();
        assert_eq!(ack(&mut presenter, 1), None, "not before a later epoch");
        assert_eq!(
            ack(&mut presenter, 2),
            Some(acked(6, 1, 2)),
            "the binding, B"
        );
        let marker = presenter.marker(2, latest());
        assert!(fin(&mut presenter, now, None).is_none()); // C: deferred.
        marker.record();
        assert_eq!(ack(&mut presenter, 3), None, "one ack per bound image");
        presenter.collect(frame(1, 3, 7));
        assert!(fin(&mut presenter, FrameStamp { epoch: 1, seq: 3 }, None).is_some());
        let marker = presenter.marker(3, latest());
        epoch.store(2, Ordering::Release); // A seek before the paint.
        marker.record();
        assert_eq!(ack(&mut presenter, 4), None, "seek-before-paint");
        assert!(fin(&mut presenter, FrameStamp { epoch: 2, seq: 4 }, None).is_none());
        assert!(presenter.stale(), "the old image stays up, marked");
        presenter.marker(4, latest()).record();
        assert_eq!(ack(&mut presenter, 5), None, "a stale paint is never acked");
    }

    /// I18: an abandoned paint (surface error, invisible viewport), a zero
    /// clip and a discarded pass never call `paint`, so nothing is acked.
    #[test]
    fn a_marker_that_never_paints_is_never_acked() {
        let (_epoch, latest) = engine_epoch(1);
        let mut presenter = Presenter::default();
        presenter.collect(frame(1, 1, 0));
        fin(&mut presenter, FrameStamp { epoch: 1, seq: 1 }, None);
        drop(presenter.marker(1, latest()));
        assert_eq!(ack(&mut presenter, 2), None);
        presenter.clear();
        presenter.marker(2, latest()).record();
        assert_eq!(ack(&mut presenter, 3), None, "a cleared cell marks nothing");
    }

    /// Review A F2 barrier witness: the audio callback advances the clock
    /// from 10 to 11 while the image of frame 10 is prepared. Nothing binds
    /// (the old finalize bound it and wrote the cell first); frame 11 does.
    #[test]
    fn a_clock_advance_during_preparation_binds_nothing() {
        let (_epoch, latest) = engine_epoch(1);
        let stamp = FrameStamp { epoch: 1, seq: 1 };
        let clock = Arc::new(std::sync::atomic::AtomicI64::new(10));
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let callback = {
            let (clock, barrier) = (Arc::clone(&clock), Arc::clone(&barrier));
            std::thread::spawn(move || {
                barrier.wait();
                clock.store(11, Ordering::Release);
                barrier.wait();
            })
        };
        let now = || (stamp, Some(TimeCode(clock.load(Ordering::Acquire))));
        let mut presenter = Presenter::default();
        presenter.collect(frame(1, 1, 10));
        presenter.collect(frame(1, 1, 11));
        let bound = presenter.finalize(now, |frame| {
            assert_eq!(frame.at, TimeCode(10), "chosen under clock 10");
            barrier.wait();
            barrier.wait();
        });
        callback.join().unwrap();
        assert!(bound.is_none(), "an expired frame never binds");
        assert_eq!(read(&presenter.cell), None, "the cell is untouched");
        presenter.marker(1, latest()).record();
        assert_eq!(ack(&mut presenter, 2), None);
        let bound = presenter.finalize(now, |_| ()).map(|(frame, ())| frame.at);
        assert_eq!(bound, Some(TimeCode(11)));
    }

    /// Review A S2: I18 through the real `CallbackTrait::paint`, run by
    /// egui-wgpu's `Renderer` on a headless device (lavapipe by default).
    struct Painted {
        gpu: kinewright_media::GpuContext,
        renderer: egui_wgpu::Renderer,
        view: egui_wgpu::wgpu::TextureView,
        ctx: egui::Context,
    }

    /// How a laid-out pass reaches the GPU.
    #[derive(Clone, Copy)]
    enum Pass {
        /// Tessellated, prepared and rendered in a submitted pass.
        Rendered,
        /// Prepared, but the pass is never recorded (no surface texture).
        Discarded,
        /// eframe skips tessellating and painting an invisible root.
        Invisible,
    }

    const SIDE: f32 = 64.0;

    impl Painted {
        fn new() -> Self {
            use egui_wgpu::wgpu;
            let gpu = kinewright_media::GpuContext::headless(true).expect("a headless device");
            let format = wgpu::TextureFormat::Rgba8Unorm;
            let renderer = egui_wgpu::Renderer::new(
                &gpu.device,
                format,
                egui_wgpu::RendererOptions::default(),
            );
            let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("i18-target"),
                size: wgpu::Extent3d {
                    width: 64,
                    height: 64,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            Self {
                gpu,
                renderer,
                view,
                ctx: egui::Context::default(),
            }
        }

        /// One root pass: the production marker over `image`, inside a
        /// viewer clipped to `clip`, then `pass`.
        fn pass(
            &mut self,
            presenter: &Presenter,
            latest: Latest,
            clip: egui::Rect,
            image: egui::Rect,
            pass: Pass,
        ) {
            use egui_wgpu::wgpu;
            let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(SIDE, SIDE));
            let input = egui::RawInput {
                screen_rect: Some(screen),
                ..Default::default()
            };
            let epoch = self.ctx.cumulative_frame_nr_for(egui::ViewportId::ROOT);
            let mut latest = Some(latest);
            let output = self.ctx.run_ui(input, |ui| {
                let viewer = ui.painter().with_clip_rect(clip);
                if let Some(latest) = latest.take() {
                    presenter.marker(epoch, latest).add_to(&viewer, image);
                }
            });
            if matches!(pass, Pass::Invisible) {
                return;
            }
            let primitives = self.ctx.tessellate(output.shapes, output.pixels_per_point);
            let descriptor = egui_wgpu::ScreenDescriptor {
                size_in_pixels: [64, 64],
                pixels_per_point: output.pixels_per_point,
            };
            let (device, queue) = (&self.gpu.device, &self.gpu.queue);
            let mut encoder =
                device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
            let mut commands = (self.renderer).update_buffers(
                device,
                queue,
                &mut encoder,
                &primitives,
                &descriptor,
            );
            if matches!(pass, Pass::Rendered) {
                let render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("i18"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &self.view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                        depth_slice: None,
                    })],
                    ..Default::default()
                });
                let mut render_pass = render_pass.forget_lifetime();
                self.renderer
                    .render(&mut render_pass, &primitives, &descriptor);
            }
            commands.push(encoder.finish());
            queue.submit(commands);
        }

        fn epoch(&self) -> u64 {
            self.ctx.cumulative_frame_nr_for(egui::ViewportId::ROOT)
        }
    }

    /// Review A S2/F3: a rendered pass acks its bound image once, at the
    /// paint instant; a discarded pass, an invisible root and a viewer
    /// whose positive clip misses the image (review A F3) ack nothing. A
    /// no-op `paint` fails the first assertion; the old viewer-rect clip
    /// fails the clipped-image one.
    #[test]
    fn the_real_paint_callback_acks_only_painted_images() {
        let mut painted = Painted::new();
        let (_engine, latest) = engine_epoch(1);
        let mut presenter = Presenter::default();
        let image = egui::Rect::from_min_size(egui::pos2(8.0, 16.0), egui::vec2(48.0, 40.0));
        let whole = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(SIDE, SIDE));
        let top_strip = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(SIDE, 8.0));
        let bind = |presenter: &mut Presenter, seq: u64| {
            presenter.collect(frame(1, seq, 0));
            assert!(fin(presenter, FrameStamp { epoch: 1, seq }, None).is_some());
        };

        bind(&mut presenter, 1);
        let before = Instant::now();
        painted.pass(&presenter, latest(), whole, image, Pass::Rendered);
        let (stamp, at, when) = presenter.take_ack(painted.epoch()).expect("painted");
        assert_eq!((stamp, at), acked(0, 1, 1));
        assert!(when >= before, "the paint instant");
        assert_eq!(ack(&mut presenter, painted.epoch() + 1), None, "once");

        for (seq, clip, pass, case) in [
            (2, whole, Pass::Discarded, "discarded pass"),
            (3, whole, Pass::Invisible, "invisible root"),
            (4, top_strip, Pass::Rendered, "clipped image"),
        ] {
            bind(&mut presenter, seq);
            assert!(top_strip.is_positive() && !top_strip.intersects(image));
            painted.pass(&presenter, latest(), clip, image, pass);
            painted.pass(&presenter, latest(), clip, image, Pass::Invisible);
            assert_eq!(ack(&mut presenter, painted.epoch()), None, "{case}");
        }
        painted.pass(&presenter, latest(), whole, image, Pass::Rendered);
        assert_eq!(
            ack(&mut presenter, painted.epoch()),
            Some(acked(0, 1, 4)),
            "the same image, painted"
        );
    }

    /// R-2 while playing: only the clock's own frame binds; an expired one
    /// is consumed unseen, an early one waits.
    #[test]
    fn playback_binds_only_the_clocks_frame() {
        let latest = FrameStamp { epoch: 3, seq: 9 };
        let mut candidates = vec![frame(3, 9, 9), frame(3, 9, 10), frame(3, 9, 11)];
        let chosen = choose_candidate(&mut candidates, latest, None, Some(TimeCode(10)));
        assert_eq!(chosen.map(|frame| frame.at), Some(TimeCode(10)));
        let left: Vec<_> = candidates.iter().map(|frame| frame.at.0).collect();
        assert_eq!(left, [11], "early waits; expired is gone");
        let mut old = vec![frame(2, 9, 10)];
        assert!(choose_candidate(&mut old, latest, None, Some(TimeCode(10))).is_none());
    }

    /// I8 (app path): 1,000 seeded interleavings of drags, seeks, deliveries
    /// (in any order, old epochs included), passes and paints. Only current,
    /// in-order images bind; while playing only the clock's frame; an ack is
    /// for an image current when painted, once per image; and the release
    /// target binds with the newest stamp (L-6).
    #[test]
    fn seeded_interleavings_bind_and_ack_only_current_frames() {
        for seed in 0..1_000_u64 {
            let mut rng = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
            let mut below = |bound: u64| {
                rng ^= rng << 13;
                rng ^= rng >> 7;
                rng ^= rng << 17;
                rng % bound
            };
            let (engine, latest_fn) = engine_epoch(0);
            let mut presenter = Presenter::default();
            let (mut latest, mut in_flight) = (FrameStamp::default(), Vec::new());
            let (mut bound, mut root) = (None::<(FrameStamp, TimeCode)>, 1_u64);
            let (mut painted, mut acks) = (Vec::new(), Vec::new());
            for _ in 0..40 {
                let at = i64::try_from(below(30)).unwrap();
                match below(6) {
                    0 => {
                        latest.seq += 1;
                        in_flight.push(frame(latest.epoch, latest.seq, at));
                    }
                    1 => {
                        (latest.epoch, latest.seq) = (latest.epoch + 1, latest.seq + 1);
                        engine.store(latest.epoch, Ordering::Release);
                        in_flight.push(frame(latest.epoch, latest.seq, at));
                    }
                    2 if !in_flight.is_empty() => {
                        let index = usize::try_from(below(in_flight.len() as u64)).unwrap();
                        presenter.collect(in_flight.remove(index));
                    }
                    3 => {
                        let playing = (below(2) == 0).then_some(TimeCode(at));
                        if let Some(frame) = fin(&mut presenter, latest, playing) {
                            assert_eq!(frame.stamp.epoch, latest.epoch, "seed {seed}");
                            if let Some((shown, _)) = bound.filter(|b| b.0.epoch == latest.epoch) {
                                assert!(frame.stamp.seq >= shown.seq, "seed {seed}: order");
                            }
                            assert!(playing.is_none_or(|p| p == frame.at), "seed {seed}");
                            bound = Some((frame.stamp, frame.at));
                        }
                    }
                    4 => {
                        presenter.marker(root, latest_fn()).record();
                        if let Some(cell) = read(&presenter.cell)
                            && !cell.stale
                            && cell.stamp.epoch == latest.epoch
                        {
                            painted.push((cell.stamp, cell.at));
                        }
                    }
                    _ => {
                        root += 1;
                        if let Some(ack) = ack(&mut presenter, root) {
                            assert!(painted.contains(&ack), "seed {seed}: unpainted ack");
                            assert_ne!(acks.last(), Some(&ack), "seed {seed}: twice");
                            acks.push(ack);
                        }
                    }
                }
            }
            (latest.epoch, latest.seq) = (latest.epoch + 1, latest.seq + 1);
            let target = TimeCode(i64::try_from(below(30)).unwrap());
            in_flight.push(frame(latest.epoch, latest.seq, target.0));
            // The preview publishes in order: the target arrives last.
            for frame in in_flight.drain(..) {
                presenter.collect(frame);
            }
            let shown = fin(&mut presenter, latest, None).map(|f| (f.stamp, f.at));
            assert_eq!(shown, Some((latest, target)), "seed {seed}: L-6");
        }
    }
}
