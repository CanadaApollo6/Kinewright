#!/usr/bin/env bash
# PF1 probe replay (provenance for docs/PF1-EVIDENCE.md). Recreates the deleted
# measurement worktree at main 6cbc150, the generated media in /tmp/pf1, and the
# instrumentation patch exactly as run on 2026-09-27. Not for commit; scratch only.
# Usage, one phase at a time, from a shell WITHOUT setup-ffmpeg.sh sourced:
#   bash scripts/pf1-probe-replay.sh prepare   (default) worktree, /tmp/pf1 media, patch; builds nothing
#   bash scripts/pf1-probe-replay.sh build     sources setup-ffmpeg.sh in the worktree, builds the test binary
# then run the commands in docs/PF1-EVIDENCE.md §E2. Every scripted source edit asserts
# its exact anchor count (rep; need for the two sed substitutions), so a drifted
# checkout fails instead of skipping or over-applying.
# Cleanup afterwards: rm "$W/third_party"; rm -rf "$W/target"; git -C "$MAIN" worktree remove --force "$W"
set -euo pipefail
MAIN=$(git -C "$(dirname "$0")" rev-parse --show-toplevel)
W=${PF1_PROBE_WORKTREE:-"$MAIN/../Kinewright-pf-probe"}
PHASE=${1:-prepare}
case "$PHASE" in
  prepare) ;;
  build)
    cd "$W"
    source ./scripts/setup-ffmpeg.sh >/dev/null 2>&1
    free -g | sed -n 2p
    time cargo test --release -p kinewright-media --lib --no-run 2>&1 \
      | grep -E "^(error|warning: unused)|error\[|-->|Finished|Executable" | head -40
    exit 0 ;;
  *) echo "usage: $0 [prepare|build]" >&2; exit 2 ;;
esac
git -C "$MAIN" worktree add --detach "$W" 6cbc150
ln -s "$MAIN/third_party" "$W/third_party"

# --- generated media (pinned x264 args) ---
# PROVENANCE: the 2026-09-27 run generated this media (and ran the T2 CLI
# hwaccel timings) with the SYSTEM /usr/bin/ffmpeg n9.0.2, not the pinned
# third_party n8.0 build; the in-engine probes (T1, T3-T9) decode through the
# pinned n8.0 libraries. Run these lines in a shell WITHOUT setup-ffmpeg.sh to
# reproduce exactly.
mkdir -p /tmp/pf1 && cd /tmp/pf1 && X="-c:v libx264 -preset veryfast -pix_fmt yuv420p -color_primaries bt709 -color_trc bt709 -colorspace bt709 -color_range tv -x264-params colorprim=bt709:transfer=bt709:colormatrix=bt709:range=tv"
gen(){ ffmpeg -hide_banner -loglevel error -y -f lavfi -i "$1" $X $2 "$3"; }
time (gen testsrc2=size=1920x1080:rate=30 "-frames:v 600 -g 60" a1080_g60.mp4 &&
gen smptebars=size=1920x1080:rate=30 "-frames:v 600 -g 60" b1080_g60.mp4 &&
gen testsrc2=size=1920x1080:rate=30 "-frames:v 600 -g 250" a1080_g250.mp4 &&
gen testsrc2=size=1080x1920:rate=30 "-frames:v 600 -g 60" p1080x1920_g60.mp4 &&
gen testsrc2=size=1080x1350:rate=30 "-frames:v 600 -g 60" f1080x1350_g60.mp4 &&
gen testsrc2=size=3840x2160:rate=30 "-frames:v 300 -g 60" a2160_g60.mp4)
ls -la
cd /tmp/pf1 && X="-c:v libx264 -preset veryfast -pix_fmt yuv420p -color_primaries bt709 -color_trc bt709 -colorspace bt709 -color_range tv -x264-params colorprim=bt709:transfer=bt709:colormatrix=bt709:range=tv"
time ffmpeg -hide_banner -loglevel error -y -f lavfi -i "testsrc2=size=1920x1080:rate=30,noise=alls=10:allf=t" -f lavfi -i "sine=frequency=220:sample_rate=48000" -t 600 $X -g 250 -c:a aac -b:a 128k talk1080_g250_10min.mp4
time ffmpeg -hide_banner -loglevel error -y -f lavfi -i "testsrc2=size=1920x1080:rate=30,noise=alls=10:allf=t" -frames:v 600 $X -g 60 n1080_g60.mp4
ls -la
for f in talk1080_g250_10min.mp4 a1080_g60.mp4 n1080_g60.mp4; do
  printf '%s,' "$f"; ffprobe -v error -show_entries format=bit_rate,duration -of csv=p=0 "$f"
done

# --- decode.rs instrumentation (pf1 counters, thread override) ---
cd $W/crates/kinewright-media/src && python3 - <<'EOF'
import re
def rep(s, a, b, n=-1, expect=1):
    c = s.count(a)
    assert c == expect, f"pf1 replay: {p}: anchor count {c} != {expect}: {a[:70]!r}"
    out = s.replace(a, b, n)
    assert out.count(b) >= 1, f"pf1 replay: {p}: replacement missing: {b[:70]!r}"
    return out
p='decode.rs'; s=open(p).read()
s=rep(s,"""        context.set_threading(ffmpeg::codec::threading::Config {
            kind: ffmpeg::codec::threading::Type::Frame,
            count: std::thread::available_parallelism()
                .map_or(1, std::num::NonZeroUsize::get)
                .min(16),
        });""","""        let pf1_threads = pf1::THREADS.load(std::sync::atomic::Ordering::Relaxed);
        context.set_threading(ffmpeg::codec::threading::Config {
            kind: if pf1::SLICE.load(std::sync::atomic::Ordering::Relaxed) { ffmpeg::codec::threading::Type::Slice } else { ffmpeg::codec::threading::Type::Frame },
            count: if pf1_threads > 0 { pf1_threads } else { std::thread::available_parallelism()
                .map_or(1, std::num::NonZeroUsize::get)
                .min(16) },
        });""",1)
s=rep(s,"""            let next = self
                .input
                .packets()
                .next()
                .map(|(stream, packet)| (stream.index(), packet));""","""            let pf1_t = std::time::Instant::now();
            let next = self
                .input
                .packets()
                .next()
                .map(|(stream, packet)| (stream.index(), packet));
            pf1::add(&pf1::DEMUX_NS, pf1_t);""",1)
s=rep(s,"""            self.decoder
                .send_packet(&packet)
                .map_err(|error| media_error(&self.path, "video decode failed", error))?;""","""            let pf1_t = std::time::Instant::now();
            self.decoder
                .send_packet(&packet)
                .map_err(|error| media_error(&self.path, "video decode failed", error))?;
            pf1::add(&pf1::DECODE_NS, pf1_t);
            pf1::PACKETS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);""",1)
s=rep(s,"""            let mut decoded = ffmpeg::frame::Video::empty();
            if self.decoder.receive_frame(&mut decoded).is_err() {
                break;
            }""","""            let mut decoded = ffmpeg::frame::Video::empty();
            let pf1_t = std::time::Instant::now();
            let pf1_r = self.decoder.receive_frame(&mut decoded);
            pf1::add(&pf1::DECODE_NS, pf1_t);
            if pf1_r.is_err() {
                break;
            }
            pf1::FRAMES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);""",1)
s=rep(s,"""        let mut rgba = ffmpeg::frame::Video::empty();
        match &mut self.converter {""","""        let mut rgba = ffmpeg::frame::Video::empty();
        let pf1_t = std::time::Instant::now();
        match &mut self.converter {""",1)
s=rep(s,"""        T::from_rgba_frame(
            &rgba,
            self.scaled_width,""","""        pf1::add(&pf1::FILTER_NS, pf1_t);
        pf1::CONVERTS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        T::from_rgba_frame(
            &rgba,
            self.scaled_width,""",1)
s=rep(s,"""        let pixels = read_plane(rgba, width, height, 8)?;
        // MO1 R7: EXIF's mirrored orientations flip in stored-pixel space
        // before the right-angle rotation.
        let pixels = flip_optional(flip_horizontal, width, height, 8, pixels)?;
        let (width, height, pixels) = rotate_bytes(rotation, width, height, 8, pixels)?;
        WorkingFrame::from_rgba64_le(
            width,
            height,
            &pixels,
            &source.description,
            source.assumption,
        )""","""        let pf1_t = std::time::Instant::now();
        let pixels = read_plane(rgba, width, height, 8)?;
        // MO1 R7: EXIF's mirrored orientations flip in stored-pixel space
        // before the right-angle rotation.
        let pixels = flip_optional(flip_horizontal, width, height, 8, pixels)?;
        let (width, height, pixels) = rotate_bytes(rotation, width, height, 8, pixels)?;
        pf1::add(&pf1::PLANE_NS, pf1_t);
        let pf1_t = std::time::Instant::now();
        let r = WorkingFrame::from_rgba64_le(
            width,
            height,
            &pixels,
            &source.description,
            source.assumption,
        );
        pf1::add(&pf1::TRANSFER_NS, pf1_t);
        r""",1)
s=rep(s,"""        self.seek_count = self.seek_count.saturating_add(1);
""","""        self.seek_count = self.seek_count.saturating_add(1);
        pf1::SEEKS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
""",1)
s+='''
pub(crate) mod pf1 {
    use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
    pub(crate) static THREADS: AtomicUsize = AtomicUsize::new(0);
    pub(crate) static SLICE: AtomicBool = AtomicBool::new(false);
    pub(crate) static DEMUX_NS: AtomicU64 = AtomicU64::new(0);
    pub(crate) static DECODE_NS: AtomicU64 = AtomicU64::new(0);
    pub(crate) static FILTER_NS: AtomicU64 = AtomicU64::new(0);
    pub(crate) static PLANE_NS: AtomicU64 = AtomicU64::new(0);
    pub(crate) static TRANSFER_NS: AtomicU64 = AtomicU64::new(0);
    pub(crate) static CONVERTS: AtomicU64 = AtomicU64::new(0);
    pub(crate) static FRAMES: AtomicU64 = AtomicU64::new(0);
    pub(crate) static PACKETS: AtomicU64 = AtomicU64::new(0);
    pub(crate) static SEEKS: AtomicU64 = AtomicU64::new(0);
    pub(crate) fn add(c: &AtomicU64, t: std::time::Instant) {
        c.fetch_add(u64::try_from(t.elapsed().as_nanos()).unwrap_or(u64::MAX), Ordering::Relaxed);
    }
    pub(crate) fn reset() {
        for c in [&DEMUX_NS, &DECODE_NS, &FILTER_NS, &PLANE_NS, &TRANSFER_NS, &CONVERTS, &FRAMES, &PACKETS, &SEEKS] {
            c.store(0, Ordering::Relaxed);
        }
    }
    /// [demux, decode, filter, plane, transfer] ms, converts, frames, packets, seeks
    pub(crate) fn snapshot() -> ([f64; 5], [u64; 4]) {
        let ms = |c: &AtomicU64| c.load(Ordering::Relaxed) as f64 / 1e6;
        ([ms(&DEMUX_NS), ms(&DECODE_NS), ms(&FILTER_NS), ms(&PLANE_NS), ms(&TRANSFER_NS)],
         [CONVERTS.load(Ordering::Relaxed), FRAMES.load(Ordering::Relaxed), PACKETS.load(Ordering::Relaxed), SEEKS.load(Ordering::Relaxed)])
    }
}
'''
open(p,'w').write(s)
print(s.count('pf1::'))
EOF

# --- render.rs phases::render_split, fixture visibility, lib.rs module ---
cd $W/crates/kinewright-media/src && python3 - <<'EOF'
def rep(s, a, b, n=-1, expect=1):
    c = s.count(a)
    assert c == expect, f"pf1 replay: {p}: anchor count {c} != {expect}: {a[:70]!r}"
    out = s.replace(a, b, n)
    assert out.count(b) >= 1, f"pf1 replay: {p}: replacement missing: {b[:70]!r}"
    return out
p='render.rs'; s=open(p).read()
s=rep(s,"""pub(crate) mod phases {
    use std::time::Duration;

    use super::*;
""","""pub(crate) mod phases {
    use std::time::Duration;

    use super::*;

    /// PF1 probe: decoded_layers wall, then the compositor phases.
    pub(crate) fn render_split(
        renderer: &mut FrameRenderer,
        document: &Document,
        at: TimeCode,
        resolution: (u32, u32),
        scale: RenderScale,
        strategy: DecodeStrategy,
    ) -> Result<(Duration, [Duration; 3], Duration), MediaError> {
        let started = std::time::Instant::now();
        let decoded = renderer.decoded_layers(document, at, resolution, scale, strategy)?;
        let decode = started.elapsed();
        let (phases, total) = crate::compositor::phases::monitor(
            &renderer.compositor,
            resolution,
            &compositor_layers(&decoded),
            &document.color_context.monitoring,
            Some(&renderer.lut_library),
        )?;
        Ok((decode, phases, total))
    }
""",1)
open(p,'w').write(s)
p='mo2_perf_fixtures.rs'; s=open(p).read()
s=rep(s,"struct Workload(Document, Vec<(GeneratedMedia, MediaAsset)>);","pub(crate) struct Workload(pub(crate) Document, pub(crate) Vec<(GeneratedMedia, MediaAsset)>);")
s=rep(s,"fn cuts(resolution","pub(crate) fn cuts(resolution")
s=rep(s,"fn blend_heavy(frames","pub(crate) fn blend_heavy(frames")
open(p,'w').write(s)
p='lib.rs'; s=open(p).read()
s=rep(s,"#[cfg(test)]\nmod mo2_perf_fixtures;","#[cfg(test)]\nmod mo2_perf_fixtures;\n#[cfg(test)]\nmod pf1_probe;",1)
open(p,'w').write(s)
EOF
grep -n "pf1_probe" lib.rs; grep -n "fn quantize_monitor" -A12 color_pipeline.rs; grep -n "pub.*fn fixture_gpu_or_skip\|pub(crate) fn" gpu_test_support.rs | head

# --- pf1_probe.rs (ignored probe tests) ---
cat > "$W/crates/kinewright-media/src/pf1_probe.rs" <<'EOF_PROBE'
//! PF1 measurement probe (scratch worktree only; never committed).
#![allow(clippy::all, clippy::pedantic, clippy::nursery, dead_code, unused)]

use std::{
    path::Path,
    sync::atomic::Ordering,
    time::{Duration, Instant},
};

use half::f16;
use kinewright_core::{AssetId, MediaAsset, TimeCode};

use crate::{
    cache::FrameCache,
    color_pipeline::encode_monitor_rgba8,
    compositor::GpuContext,
    decode::{VideoDecoder, pf1, probe_path},
    frame::WorkingFrame,
    mo2_perf_fixtures::{Workload, blend_heavy, cuts},
    render::{DecodeStrategy, FrameRenderer, PREVIEW_MAX_WIDTH, RenderScale, d65_assumption, phases},
};

fn asset(name: &str) -> MediaAsset {
    crate::initialize_ffmpeg().unwrap();
    probe_path(&Path::new("/tmp/pf1").join(name), AssetId(1)).expect("probe")
}

fn open(asset: &MediaAsset, max_width: Option<u32>) -> VideoDecoder {
    VideoDecoder::open_scaled_managed(
        &asset.path,
        asset.fps,
        max_width,
        &asset.color_description,
        d65_assumption(&asset.color_description),
    )
    .expect("open")
}

fn pct(sorted: &[f64], p: f64) -> f64 {
    let i = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[i]
}

fn stats(mut v: Vec<f64>) -> (f64, f64, f64) {
    v.sort_by(f64::total_cmp);
    let mean = v.iter().sum::<f64>() / v.len() as f64;
    (mean, pct(&v, 0.5), pct(&v, 0.95))
}

/// Per-frame decode breakdown, sequential 16-frame windows (the playback path).
#[test]
#[ignore = "pf1 probe"]
fn pf1_decode_breakdown() {
    let files = std::env::var("PF1_FILES").unwrap_or_else(|_| {
        "a1080_g60.mp4,n1080_g60.mp4,a1080_g250.mp4,p1080x1920_g60.mp4,f1080x1350_g60.mp4,a2160_g60.mp4,talk1080_g250_10min.mp4".into()
    });
    let configs: Vec<(bool, usize)> = std::env::var("PF1_CFG")
        .unwrap_or_else(|_| "f1,f4,f16,s16".into())
        .split(',')
        .map(|c| (c.starts_with('s'), c[1..].parse().unwrap()))
        .collect();
    let widths: Vec<Option<u32>> = std::env::var("PF1_WIDTHS")
        .unwrap_or_else(|_| "1280".into())
        .split(',')
        .map(|w| if w == "full" { None } else { Some(w.parse().unwrap()) })
        .collect();
    for file in files.split(',') {
        let asset = asset(file);
        for &width in &widths {
            for &(slice, threads) in &configs {
                pf1::THREADS.store(threads, Ordering::Relaxed);
                pf1::SLICE.store(slice, Ordering::Relaxed);
                let mut decoder = open(&asset, width);
                let mut cache: FrameCache<WorkingFrame> = FrameCache::new(32);
                pf1::reset();
                let frames = 240i64;
                let started = Instant::now();
                let mut window_ms = Vec::new();
                let mut start = 0;
                while start < frames {
                    let end = (start + 15).min(frames - 1);
                    let t = Instant::now();
                    decoder
                        .decode_window_sequential(TimeCode(start), TimeCode(end), &mut cache)
                        .unwrap();
                    window_ms.push(t.elapsed().as_secs_f64() * 1e3);
                    start = end + 1;
                }
                let wall = started.elapsed().as_secs_f64() * 1e3 / frames as f64;
                let ([demux, decode, filter, plane, transfer], [converts, decoded, packets, seeks]) =
                    pf1::snapshot();
                let n = converts.max(1) as f64;
                let (wmean, _, wp95) = stats(window_ms);
                println!(
                    "PF1 decode file={file} width={width:?} threads={}{threads} wall_ms_per_frame={wall:.2} demux={:.2} decode={:.2} swscale_filter={:.2} plane_copy={:.2} transfer_f16={:.2} converts={converts} decoded={decoded} packets={packets} seeks={seeks} window16_mean_ms={wmean:.1} window16_p95_ms={wp95:.1}",
                    if slice { "slice" } else { "frame" },
                    demux / n,
                    decode / n,
                    filter / n,
                    plane / n,
                    transfer / n,
                );
            }
        }
    }
    pf1::THREADS.store(0, Ordering::Relaxed);
    pf1::SLICE.store(false, Ordering::Relaxed);
}

/// Paused seek (DecodeStrategy::Seek, no prefetch) and scrub-step latency.
#[test]
#[ignore = "pf1 probe"]
fn pf1_seek_latency() {
    let files = std::env::var("PF1_FILES")
        .unwrap_or_else(|_| "a1080_g60.mp4,a1080_g250.mp4,talk1080_g250_10min.mp4".into());
    for file in files.split(',') {
        let asset = asset(file);
        let duration = asset.duration.0;
        for &(slice, threads) in &[(false, 1usize), (false, 4), (false, 16), (true, 16)] {
            pf1::THREADS.store(threads, Ordering::Relaxed);
            pf1::SLICE.store(slice, Ordering::Relaxed);
            let mut decoder = open(&asset, Some(PREVIEW_MAX_WIDTH));
            let mut cache: FrameCache<WorkingFrame> = FrameCache::new(32);
            // warm
            decoder.decode_window(TimeCode(5), TimeCode(5), &mut cache).unwrap();
            let mut seed = 12345u64;
            let mut random = Vec::new();
            let mut per_seek_frames = Vec::new();
            pf1::reset();
            for _ in 0..30 {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                let at = ((seed >> 33) as i64) % (duration - 2);
                let before = pf1::FRAMES.load(Ordering::Relaxed);
                let t = Instant::now();
                decoder.decode_window(TimeCode(at), TimeCode(at), &mut cache).unwrap();
                random.push(t.elapsed().as_secs_f64() * 1e3);
                per_seek_frames.push((pf1::FRAMES.load(Ordering::Relaxed) - before) as f64);
            }
            let ([_, decode, filter, plane, transfer], [converts, ..]) = pf1::snapshot();
            // scrub: +1 frame steps from a mid-GOP point
            let mut step = Vec::new();
            let base = (duration / 2) as i64;
            for k in 0..30 {
                let t = Instant::now();
                decoder
                    .decode_window(TimeCode(base + k), TimeCode(base + k), &mut cache)
                    .unwrap();
                step.push(t.elapsed().as_secs_f64() * 1e3);
            }
            let (rm, r50, r95) = stats(random);
            let (fm, _, _) = stats(per_seek_frames);
            let (sm, s50, s95) = stats(step);
            println!(
                "PF1 seek file={file} threads={}{threads} random_mean={rm:.1} p50={r50:.1} p95={r95:.1} decoded_frames_per_seek={fm:.1} decode_ms={:.1} filter_ms={:.1} plane_ms={:.1} transfer_ms={:.1} converts={converts} | scrub_step_mean={sm:.1} p50={s50:.1} p95={s95:.1}",
                if slice { "slice" } else { "frame" },
                decode / 30.0,
                filter / 30.0,
                plane / 30.0,
                transfer / 30.0,
            );
        }
    }
    pf1::THREADS.store(0, Ordering::Relaxed);
    pf1::SLICE.store(false, Ordering::Relaxed);
}

fn e2e(gpu: fn() -> GpuContext, label: &str) {
    let only = std::env::var("PF1_ONLY").unwrap_or_else(|_| "typical,blend_heavy".into());
    let mut work: Vec<(&str, Workload)> = Vec::new();
    if only.contains("typical") {
        work.push(("typical_1080p", cuts((1920, 1080), 600, 3, 24, 15)));
    }
    if only.contains("blend_heavy") {
        work.push(("blend_heavy_1080p", blend_heavy(330)));
    }
    if only.contains("continuous2") {
        // two whole-length sources, both Normal, plain stack
        let Workload(mut doc, media) = cuts((1920, 1080), 600, 2, 2, 590);
        work.push(("continuous2_1080p", Workload(doc, media)));
    }
    for (key, Workload(document, _media)) in &work {
        for &(budget, name) in &[(None, "default_224MiB"), (Some(1usize << 30), "resident_1GiB")] {
            let scale = RenderScale::Proxy {
                max_width: PREVIEW_MAX_WIDTH,
            };
            let dims = scale.output_resolution(document.resolution);
            let mut renderer = FrameRenderer::new(gpu());
            if let Some(b) = budget {
                renderer.set_cache_budget(b);
            }
            let mut totals = Vec::new();
            let (mut dec, mut up, mut gp, mut en) = (0.0, 0.0, 0.0, 0.0);
            pf1::reset();
            let n = 150;
            for frame in 0..(30 + n) {
                if frame == 30 {
                    pf1::reset();
                }
                let at = TimeCode(frame % document.duration.0);
                let t = Instant::now();
                let (d, [u, g, e], _) = phases::render_split(
                    &mut renderer,
                    document,
                    at,
                    dims,
                    scale,
                    DecodeStrategy::Sequential,
                )
                .unwrap();
                let total = t.elapsed().as_secs_f64() * 1e3;
                if frame >= 30 {
                    totals.push(total);
                    dec += d.as_secs_f64() * 1e3;
                    up += u.as_secs_f64() * 1e3;
                    gp += g.as_secs_f64() * 1e3;
                    en += e.as_secs_f64() * 1e3;
                }
            }
            let nf = n as f64;
            let ([demux, decode, filter, plane, transfer], [converts, decoded, _, seeks]) =
                pf1::snapshot();
            let (m, p50, p95) = stats(totals);
            println!(
                "PF1 e2e adapter={label} workload={key} cache={name} dims={dims:?} mean_ms={m:.1} p50={p50:.1} p95={p95:.1} fps={:.1} | decoded_layers={:.1} (demux {:.1} decode {:.1} swscale {:.1} plane {:.1} transfer {:.1}) upload={:.1} gpu+readback={:.1} monitor_encode={:.1} | per frame: converts={:.2} decoded={:.2} seeks={:.2}",
                1e3 / m,
                dec / nf,
                demux / nf,
                decode / nf,
                filter / nf,
                plane / nf,
                transfer / nf,
                up / nf,
                gp / nf,
                en / nf,
                converts as f64 / nf,
                decoded as f64 / nf,
                seeks as f64 / nf,
            );
        }
    }
}

#[test]
#[ignore = "pf1 probe"]
fn pf1_e2e_lavapipe() {
    e2e(|| GpuContext::headless(true).unwrap(), "lavapipe");
}

#[test]
#[ignore = "pf1 probe"]
fn pf1_e2e_hardware() {
    e2e(|| GpuContext::headless(false).unwrap(), "hardware");
}

/// Exhaustive f16-indexed monitor LUT vs the CC1 per-pixel encode; timing.
#[test]
#[ignore = "pf1 probe"]
fn pf1_monitor_encode_lut() {
    let mut rgb = vec![0u8; 65536];
    let mut alpha = vec![0u8; 65536];
    for bits in 0..=u16::MAX {
        let v = f16::from_bits(bits).to_f32();
        let c = encode_monitor_rgba8([v, v, v, v]);
        rgb[bits as usize] = c[0];
        alpha[bits as usize] = c[3];
        assert_eq!(c[1], c[0]);
        assert_eq!(c[2], c[0]);
    }
    // a realistic 1280x720 working frame: gradient + noise, plus over-range
    let (w, h) = (1280usize, 720usize);
    let mut px = Vec::with_capacity(w * h * 4);
    let mut seed = 7u64;
    for i in 0..w * h {
        for c in 0..3 {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let v = ((i % w) as f32 / w as f32) * 1.2 + ((seed >> 40) as f32 / 16777216.0 - 0.5) * 0.1;
            px.push(f16::from_f32(v));
        }
        px.push(f16::from_f32(1.0));
    }
    let mut bytes = vec![0u8; px.len() * 2];
    for (i, v) in px.iter().enumerate() {
        bytes[i * 2..i * 2 + 2].copy_from_slice(&v.to_le_bytes());
    }
    let reps = 20;
    let t = Instant::now();
    let mut out_a = Vec::new();
    for _ in 0..reps {
        let mut rgba = Vec::with_capacity(w * h * 4);
        for p in bytes.as_chunks::<8>().0 {
            let linear = [
                f16::from_le_bytes([p[0], p[1]]).to_f32(),
                f16::from_le_bytes([p[2], p[3]]).to_f32(),
                f16::from_le_bytes([p[4], p[5]]).to_f32(),
                f16::from_le_bytes([p[6], p[7]]).to_f32(),
            ];
            rgba.extend_from_slice(&encode_monitor_rgba8(linear));
        }
        out_a = std::hint::black_box(rgba);
    }
    let exact_ms = t.elapsed().as_secs_f64() * 1e3 / reps as f64;
    let t = Instant::now();
    let mut out_b = Vec::new();
    for _ in 0..reps {
        let mut rgba = Vec::with_capacity(w * h * 4);
        for p in bytes.as_chunks::<8>().0 {
            rgba.extend_from_slice(&[
                rgb[u16::from_le_bytes([p[0], p[1]]) as usize],
                rgb[u16::from_le_bytes([p[2], p[3]]) as usize],
                rgb[u16::from_le_bytes([p[4], p[5]]) as usize],
                alpha[u16::from_le_bytes([p[6], p[7]]) as usize],
            ]);
        }
        out_b = std::hint::black_box(rgba);
    }
    let lut_ms = t.elapsed().as_secs_f64() * 1e3 / reps as f64;
    assert_eq!(out_a, out_b);
    println!("PF1 monitor_encode 1280x720 exact_ms={exact_ms:.2} lut_ms={lut_ms:.2} identical=true exhaustive_65536=true");
}

/// Exhaustive u16-indexed decode-transfer LUT vs WorkingFrame::from_rgba64_le; timing.
#[test]
#[ignore = "pf1 probe"]
fn pf1_transfer_lut() {
    let asset = asset("a1080_g60.mp4");
    let desc = &asset.color_description;
    let assume = d65_assumption(desc);
    // every u16 value in every channel, alpha too
    let mut all = Vec::with_capacity(65536 * 8);
    for v in 0..=u16::MAX {
        for _ in 0..4 {
            all.extend_from_slice(&v.to_le_bytes());
        }
    }
    let reference = WorkingFrame::from_rgba64_le(65536, 1, &all, desc, assume).unwrap();
    let rgb: Vec<f16> = (0..65536).map(|i| reference.pixels[i * 4]).collect();
    let alpha: Vec<f16> = (0..65536).map(|i| reference.pixels[i * 4 + 3]).collect();
    for i in 0..65536 {
        assert_eq!(reference.pixels[i * 4 + 1].to_bits(), rgb[i].to_bits());
        assert_eq!(reference.pixels[i * 4 + 2].to_bits(), rgb[i].to_bits());
    }
    let (w, h) = (1280u32, 720u32);
    let mut frame = Vec::with_capacity((w * h * 8) as usize);
    let mut seed = 3u64;
    for _ in 0..w * h {
        for c in 0..4 {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let v: u16 = if c == 3 { u16::MAX } else { (seed >> 48) as u16 };
            frame.extend_from_slice(&v.to_le_bytes());
        }
    }
    let reps = 10;
    let t = Instant::now();
    let mut a = None;
    for _ in 0..reps {
        a = Some(std::hint::black_box(WorkingFrame::from_rgba64_le(w, h, &frame, desc, assume).unwrap()));
    }
    let exact_ms = t.elapsed().as_secs_f64() * 1e3 / reps as f64;
    let t = Instant::now();
    let mut b = Vec::new();
    for _ in 0..reps {
        let mut out = Vec::with_capacity((w * h * 4) as usize);
        for p in frame.as_chunks::<8>().0 {
            out.push(rgb[u16::from_le_bytes([p[0], p[1]]) as usize]);
            out.push(rgb[u16::from_le_bytes([p[2], p[3]]) as usize]);
            out.push(rgb[u16::from_le_bytes([p[4], p[5]]) as usize]);
            out.push(alpha[u16::from_le_bytes([p[6], p[7]]) as usize]);
        }
        b = std::hint::black_box(out);
    }
    let lut_ms = t.elapsed().as_secs_f64() * 1e3 / reps as f64;
    let a = a.unwrap();
    assert!(a.pixels.iter().zip(&b).all(|(x, y)| x.to_bits() == y.to_bits()));
    println!("PF1 transfer 1280x720 desc={:?}/{:?}/{:?} exact_ms={exact_ms:.2} lut_ms={lut_ms:.2} identical=true exhaustive_65536=true", desc.transfer, desc.range, desc.bit_depth);
}

/// The audio callback advances the clock even when the ring is empty.
#[test]
#[ignore = "pf1 probe"]
fn pf1_underrun_advances_clock() {
    let (_producer, mut consumer) = rtrb::RingBuffer::<f32>::new(4096);
    let position = std::sync::atomic::AtomicU64::new(0);
    let mut out = vec![1.0f32; 2048];
    crate::audio::pf1_render_output(&mut consumer, &mut out, 2, &position);
    println!(
        "PF1 underrun: ring empty, position advanced by {} sample frames, output all silence={}",
        position.load(Ordering::Relaxed),
        out.iter().all(|s| *s == 0.0)
    );
}

/// GPU upload cost of RGBA8 vs RGBA16F at 1280x720 via queue.write_texture (egui's path).
#[test]
#[ignore = "pf1 probe"]
fn pf1_texture_write_cost() {
    for (label, gpu) in [
        ("lavapipe", GpuContext::headless(true).unwrap()),
        ("hardware", GpuContext::headless(false).unwrap()),
    ] {
        let device = gpu.device();
        let queue = gpu.queue();
        for (fmt, bpp, name) in [
            (wgpu::TextureFormat::Rgba8UnormSrgb, 4u32, "rgba8"),
            (wgpu::TextureFormat::Rgba16Float, 8, "rgba16f"),
        ] {
            let (w, h) = (1280u32, 720u32);
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: None,
                size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: fmt,
                usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            let data = vec![128u8; (w * h * bpp) as usize];
            let mut ms = Vec::new();
            for _ in 0..40 {
                let t = Instant::now();
                queue.write_texture(
                    texture.as_image_copy(),
                    &data,
                    wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w * bpp), rows_per_image: Some(h) },
                    wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                );
                queue.submit([]);
                let _ = device.poll(wgpu::PollType::wait_indefinitely());
                ms.push(t.elapsed().as_secs_f64() * 1e3);
            }
            let (m, p50, p95) = stats(ms[5..].to_vec());
            println!("PF1 write_texture adapter={label} format={name} 1280x720 mean={m:.2} p50={p50:.2} p95={p95:.2}");
        }
    }
}
EOF_PROBE

# --- audio.rs probe wrapper ---
cd "$W/crates/kinewright-media/src" && cat >> audio.rs <<'EOF'

#[cfg(test)]
pub(crate) fn pf1_render_output(consumer: &mut Consumer<f32>, output: &mut [f32], channels: usize, position: &AtomicU64) {
    render_output(consumer, output, channels, position, 0);
}
EOF


# --- GpuContext field access fix ---
# (The 2026-09-27 run built here and again after the RSS probe; the build is now
# the separate `build` phase.)
cd $W/crates/kinewright-media/src
# exact-count check around both substitutions: each anchor once before, zero after
need(){ [ "$(grep -cF -- "$1" pf1_probe.rs)" = "$2" ] || { echo "pf1 replay: pf1_probe.rs: '$1' count != $2" >&2; exit 1; }; }
need 'let device = gpu.device();' 1; need 'let queue = gpu.queue();' 1
sed -i 's/let device = gpu.device();/let device = \&gpu.device;/; s/let queue = gpu.queue();/let queue = \&gpu.queue;/' pf1_probe.rs
need 'let device = gpu.device();' 0; need 'let queue = gpu.queue();' 0
need 'let device = &gpu.device;' 1; need 'let queue = &gpu.queue;' 1

# --- RSS probe ---
cat >> $W/crates/kinewright-media/src/pf1_probe.rs <<'EOF'

fn rss_mib() -> f64 {
    let s = std::fs::read_to_string("/proc/self/status").unwrap();
    let kb: f64 = s.lines().find(|l| l.starts_with("VmRSS:")).unwrap().split_whitespace().nth(1).unwrap().parse().unwrap();
    kb / 1024.0
}

/// RSS per open 1080p decoder at the preview raster, by frame-thread count.
#[test]
#[ignore = "pf1 probe"]
fn pf1_decoder_rss() {
    let threads: usize = std::env::var("PF1_T").unwrap().parse().unwrap();
    let file = std::env::var("PF1_FILE").unwrap_or_else(|_| "a1080_g60.mp4".into());
    let asset = asset(&file);
    pf1::THREADS.store(threads, Ordering::Relaxed);
    let base = rss_mib();
    let mut keep = Vec::new();
    for k in 0..4 {
        let mut decoder = open(&asset, Some(PREVIEW_MAX_WIDTH));
        let mut cache: FrameCache<WorkingFrame> = FrameCache::new(1);
        decoder
            .decode_window_sequential(TimeCode(k * 40), TimeCode(k * 40 + 31), &mut cache)
            .unwrap();
        drop(cache);
        keep.push(decoder);
        println!("PF1 rss file={file} threads={threads} decoders={} rss_delta_mib={:.1}", k + 1, rss_mib() - base);
    }
}
EOF
echo ok

echo "prepare done: next run: bash $0 build"
