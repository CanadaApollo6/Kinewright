//! CC8 fixtures: §10 step 1's encoder precondition, §10 step 3's §9.1
//! fixtures 1, 2 and 5, and §10 step 4's §9.1 fixtures 3, 4 and 10.
//!
//! §9 fixes the layout: "Constants authority: `kinewright_core::cc8_hdr`, in
//! the manner of `cc7_scenarios`. Fixtures: `kinewright_media::cc8_fixtures`.
//! Manifest: `cc8_manifest.json`. Test names are `cc8_`-prefixed and the
//! manifest asserts the inventory equals the declared set." The manifest is §10
//! step 10's, so each measuring fixture here prints a `CC8_MEASURED` line in
//! the manner step 1's `CC8_PRECONDITION` line established, and states its
//! measurement in the comment beside the constant it bounds.
//!
//! # §10 step 1
//!
//! `docs/CC8-HDR-INTERPRETATION-AND-DELIVERY.md` §0.3(e) records that the
//! Windows CI `FFmpeg` is a *different package* from the Linux one
//! (`System233/ffmpeg-msvc-prebuilt ffmpeg-8.0.1-r3`, an MSVC/vcpkg build,
//! against Linux's `mifi/ffmpeg-builds 8.0-1`), that its x264 is therefore a
//! different port at a potentially different core version, and that
//! `arib-std-b67` is a later addition to x264's `colorprim`/`transfer` tables
//! than the CC6 lane's `bt709` values. §10 step 1 makes confirming that build's
//! libx264 the first slice of CC8: **"Nothing else starts until this is
//! green — it can invalidate §0.2 Q2 and therefore the whole lane."**
//!
//! This module is that slice, and only that slice. It is deliberately allowed
//! to land before the `cc8_hdr` authority module (§10 step 2) exists, so it
//! owns its own literals rather than importing constants that are not written
//! yet; when §10 step 2 lands, these strings become the authority module's and
//! this file reads them from there.
//!
//! **A red Windows run is this test doing its job.** Per CC6 §11.2.11's rule,
//! which §0.3(e) inherits by name, the precondition **fails typed rather than
//! skipping**: there is no `#[cfg(target_os)]`, no `#[ignore]`, and no
//! `Result` swallowed into a pass. Every failing direction — libx264 absent,
//! the 10-bit lane's pixel format unadvertised, the encoder refusing the HDR
//! `x264-params`, or a tag that does not survive the re-probe — panics with a
//! [`DeliveryColorError`]-shaped message naming `code` / `field` / `observed` /
//! `allowed`, so the first Windows run answers §0.2 Q2 in a red or green
//! build rather than in silence.
//!
//! **What is and is not measured here.** §0.3(e) asks about the *encoder
//! build's capability*, not about the delivery lane's plumbing, so the encoder
//! is constructed locally through the same `ffmpeg-next` mechanism
//! `export.rs:195-233` uses — `codec::context::Context::new_with_codec`, then
//! `set_colorspace` / `set_color_range` on the context, then the tags through
//! the `"x264-params"` entry of the options dictionary handed to
//! `open_as_with`, exactly as `export.rs:238` passes
//! `DELIVERY_X264_PARAMS`. It deliberately does **not** call
//! `export_document`: `export.rs:216-217`, `:425`, `:529`, `:626`, `:638` and
//! `:1187` hard-code BT.709 today, and §0.4 lists all six as hard stops whose
//! lane-derivation is **§10 step 6**, after the SDR regression gate of §10
//! step 5. Refactoring the delivery lane to make this precondition runnable
//! through it would invert the contract's own order, so the honest scope for
//! step 1 is the encoder build, tested through the encoder-build mechanism.
//!
//! **Why the tags cannot ride the generic fields.** §0.3(b) measured that
//! `FFmpeg`'s generic codec-context colour fields drop primaries and transfer
//! through libx264's SPS, reproducing what `export.rs:220-223` already
//! documents for the SDR lane. The `ffmpeg-next` 8.0 encoder surface says the
//! same thing structurally: `codec::encoder::Video` exposes `set_colorspace`
//! and `set_color_range` and **no** primaries or transfer setter at all. The
//! `x264-params` string is therefore the only channel, and §5.2 makes
//! re-probing the written file the standing proof that it stayed open.
//!
//! The re-probe runs through `decode::probe_path`, the production probe that
//! `media_matrix_tests::probe_preserves_ten_bit_bt2020_source_metadata` and
//! CC6's `verify.rs` path both read, so a mapping regression in
//! `decode.rs:240-286` fails here too. §0.3(f)'s claim that the probe side
//! needs no production change is confirmed by this file passing with none.

use std::{collections::BTreeMap, path::Path};

use ffmpeg_next as ffmpeg;
use half::f16;
use kinewright_core::{
    AssetId, CC8_BT709_TO_REC2020, CC8_BT2020_CB_DENOMINATOR, CC8_BT2020_CR_DENOMINATOR,
    CC8_BT2020_KB, CC8_BT2020_KG, CC8_BT2020_KR, CC8_HLG_NOMINAL_PEAK_NITS,
    CC8_HLG_REFERENCE_WHITE_SIGNAL_PERCENT, CC8_HLG_SCENE_BREAKPOINT, CC8_HLG_SIGNAL_BREAKPOINT,
    CC8_PQ_C1, CC8_PQ_C2, CC8_PQ_C3, CC8_PQ_M2, CC8_PQ_PEAK_NITS, CC8_REFERENCE_WHITE_NITS,
    CC8_REJECTED_HDR_ADJACENT, CC8_SOURCE_PROFILES, ColorBitDepth, ColorDescription, ColorMatrix,
    ColorPrimaries, ColorProvenance, ColorRange, ColorSourceError, ColorSourceProfile,
    ColorSourceProfileAssumption, ColorTransfer, ColorWhitePoint, DeliveryColorError,
    DeliveryColorMismatch, DeliveryEncodeDepth, DeliveryProfile, Effect, EffectId,
    HDR_SOURCE_ON_SDR_DELIVERY, ParamValue, QaSeverity, Rational, cc8_apply_matrix,
    cc8_hlg_decode_working_linear, cc8_hlg_encode_working_linear, cc8_hlg_inverse_oetf,
    cc8_hlg_oetf, cc8_pq_decode_working_linear, cc8_pq_encode_working_linear, cc8_pq_eotf_nits,
    cc8_pq_inverse_eotf, classify_source, classify_source_with_assumption, delivery_conformance,
};

use crate::{
    Compositor, CompositorLayer,
    cc1_fixtures::{
        LINEAR_CPU_GPU_MAX, LINEAR_CPU_GPU_MEAN, LINEAR_CPU_GPU_P99, LINEAR_OVER_RANGE_MEAN,
        LINEAR_OVER_RANGE_P99, decode_managed_working_frame, fallback_gpu, working_frame,
    },
    color_pipeline::{
        PrimaryCorrection, PrimaryParameter, apply_primary_corrections,
        decode_hdr_source_working_linear, hdr_source_to_working_bt709_linear,
        rgba64_normalization_max, rgba64_promoted_max, source_primaries_to_working_linear,
    },
    decode::{
        VideoDecoder, managed_filter_matrix, managed_filter_range, managed_scale_color_matrix,
        probe_path,
    },
    frame::WorkingFrame,
    initialize_ffmpeg,
    test_support::{TempDirectory, single_clip_document},
    timeline::TransitionRenderParams,
};

/// The encoder CC8 §5.1 keeps from CC6 §4.1 — `export.rs`'s
/// `DELIVERY_VIDEO_CODEC`, restated rather than imported because that constant
/// is private to `export.rs` and §10 step 6, not step 1, is what moves it.
const CC8_PRECONDITION_CODEC: &str = "libx264";

/// The HDR lane's pixel format (§5.1: `yuv420p10le`), which is CC6's existing
/// [`DeliveryEncodeDepth::Ten`] lane format.
const CC8_PRECONDITION_PIXEL: ffmpeg::format::Pixel = ffmpeg::format::Pixel::YUV420P10LE;

/// The `x264-params` primaries and matrix terms of §5.1's lane. Only the
/// transfer differs between the two legs below.
const CC8_PRECONDITION_PRIMARIES_PARAM: &str = "colorprim=bt2020";
const CC8_PRECONDITION_MATRIX_PARAM: &str = "colormatrix=bt2020nc";

/// §5.1's transfer, HLG. The later of x264's two HDR transfer names and the
/// one §0.3(e) singles out as the Windows risk.
const CC8_PRECONDITION_HLG_TRANSFER_PARAM: &str = "transfer=arib-std-b67";

/// The PQ transfer. §0.2 Q1's alternative answer, measured here so a Windows
/// build that carries one HDR transfer but not the other is distinguished by
/// *which* leg goes red rather than by a single ambiguous failure.
const CC8_PRECONDITION_PQ_TRANSFER_PARAM: &str = "transfer=smpte2084";

/// A raster small enough to encode in well under a second and still exercise
/// chroma subsampling on both axes.
const CC8_PRECONDITION_SIZE: (u32, u32) = (64, 64);

/// Enough coded pictures for libx264 to emit a real SPS and for the muxer to
/// write a track the probe can read.
const CC8_PRECONDITION_FRAMES: i64 = 6;

/// Frames per second of the precondition clip.
const CC8_PRECONDITION_FPS: i32 = 25;

/// Bit rate of the precondition clip. The precondition asserts *tags*, never a
/// difference budget, so this only has to be a rate libx264 accepts.
const CC8_PRECONDITION_BITRATE: usize = 2_000_000;

/// Panic with a [`DeliveryColorError`]-shaped message, in the form CC6
/// §11.2.11's `assert_libx264_advertises_the_ten_bit_lane` established.
///
/// This is the file's only failure exit: it is what makes "fails typed rather
/// than skipping" true in code rather than in a comment.
fn fail_typed(error: &DeliveryColorError, leg: &str, detail: &str) -> ! {
    panic!(
        "{}: field {}, observed {}, allowed {} — {detail} ({leg} leg). CC8 §0.3(e)/§10 step 1 \
         fails typed rather than skipping, so this is a red build, not silence; recovery: {}",
        error.code(),
        error.field(),
        error.observed(),
        error.allowed_values(),
        error.recovery_action()
    );
}

/// This build's libx264 must exist and must advertise the HDR lane's pixel
/// format before any parameter string is worth trying.
///
/// Identical in shape to CC6 §11.2.11's check, and deliberately so: CC8 §5.1
/// reuses that lane's depth, so a build that fails here fails CC6 too and the
/// two must not disagree about how they say it.
fn assert_libx264_advertises_the_hdr_lane_pixel_format() {
    let Some(codec) = ffmpeg::encoder::find_by_name(CC8_PRECONDITION_CODEC) else {
        fail_typed(
            &DeliveryColorError::UnsupportedCodec {
                observed: "libx264 is absent from this build".to_owned(),
                allowed: "libx264",
            },
            "encoder lookup",
            "the managed delivery encoder is not present, so the HDR lane cannot exist here",
        );
    };
    let advertised = codec
        .video()
        .expect("libx264 is a video encoder")
        .formats()
        .map(std::iter::Iterator::collect::<Vec<_>>)
        .unwrap_or_default();
    if !advertised.contains(&CC8_PRECONDITION_PIXEL) {
        fail_typed(
            &DeliveryColorError::EncoderPixelFormatUnavailable {
                observed: if advertised.is_empty() {
                    "no advertised pixel formats".to_owned()
                } else {
                    advertised
                        .iter()
                        .map(|format| format!("{format:?}"))
                        .collect::<Vec<_>>()
                        .join(" ")
                },
                allowed: DeliveryEncodeDepth::Ten.pixel_format().to_owned(),
            },
            "pixel format",
            "this build cannot open the 10-bit lane the HDR delivery lane is built on",
        );
    }
}

/// Fill one `yuv420p10le` picture with a horizontal luma ramp over the legal
/// 10-bit range and neutral chroma.
///
/// The content is irrelevant to what is being proved — the tags are in the
/// SPS, not in the pixels — but a ramp rather than a flat field keeps libx264
/// from producing a degenerate stream, and neutral chroma at 512 keeps the
/// picture legal so the probe reads a normal file.
fn fill_ramp(frame: &mut ffmpeg::frame::Video, frame_index: i64) {
    const LUMA_FLOOR: u16 = 64;
    const LUMA_CEILING: u16 = 940;
    const NEUTRAL_CHROMA: u16 = 512;

    let width = CC8_PRECONDITION_SIZE.0 as usize;
    let height = CC8_PRECONDITION_SIZE.1 as usize;
    let phase = usize::try_from(frame_index.rem_euclid(CC8_PRECONDITION_FRAMES))
        .expect("a non-negative frame phase fits a usize");

    let luma_stride = frame.stride(0);
    let luma = frame.data_mut(0);
    for row in 0..height {
        for column in 0..width {
            let step = u16::try_from((column + phase) % width)
                .expect("the precondition raster is far narrower than u16::MAX");
            let last = u16::try_from(width - 1).expect("likewise for the last column");
            let value = LUMA_FLOOR + (LUMA_CEILING - LUMA_FLOOR) * step / last;
            let at = row * luma_stride + column * 2;
            luma[at..at + 2].copy_from_slice(&value.to_le_bytes());
        }
    }

    for plane in 1..3 {
        let stride = frame.stride(plane);
        let data = frame.data_mut(plane);
        for row in 0..height / 2 {
            for column in 0..width / 2 {
                let at = row * stride + column * 2;
                data[at..at + 2].copy_from_slice(&NEUTRAL_CHROMA.to_le_bytes());
            }
        }
    }
}

/// Encode [`CC8_PRECONDITION_FRAMES`] pictures of `yuv420p10le` H.264 to `out`,
/// tagging them with §5.1's HDR colour description through `transfer_param`.
///
/// The construction mirrors `export.rs:195-233` term for term: the encoder is
/// built with `Context::new_with_codec(...).encoder().video()`, the colourspace
/// and range are set on the context, the global-header flag is taken from the
/// muxer's format, and the colour tags travel in the `"x264-params"` entry of
/// the [`ffmpeg::Dictionary`] handed to `open_as_with` — the same mechanism
/// and the same option *name* the SDR lane uses for
/// `colorprim=bt709:transfer=bt709:colormatrix=bt709`, not `x264opts` and not
/// a CLI subprocess. `range=tv` is absent for CC6 §4.3's measured reason: it is
/// not an x264 parameter, and `set_color_range(Range::MPEG)` reaches the SPS on
/// its own — which the `ColorRange::Limited` assertion downstream then proves
/// rather than assumes.
///
/// Returns the exact parameter string that was handed to libx264, so the
/// caller can name it in evidence.
fn encode_hdr_precondition_clip(out: &Path, transfer_param: &str, leg: &str) -> String {
    encode_hdr_clip(out, transfer_param, leg, None, fill_ramp)
}

/// The general form of [`encode_hdr_precondition_clip`], added by §10 step 4.
///
/// Step 4 needs a clip whose *samples* are known as well as whose tags are, so
/// the two things step 1's helper fixed — the picture content and the absence of
/// any extra `x264-params` term — become arguments here. Everything else is
/// unchanged and step 1's precondition still runs through this function, so a
/// change to the encoder construction cannot move for one fixture and not the
/// other.
///
/// `extra_x264_params` is appended after the three colour terms with a `:`
/// separator, which is how §10 step 4's source clips ask for `qp=0`: a lossless
/// encode makes the decoded 10-bit samples **exactly** the samples that were
/// written, so a fixture can state a working-space expectation from the codes it
/// authored rather than from what a lossy encoder happened to produce. That
/// changes the H.264 profile (x264 selects High 4:4:4 Predictive for lossless),
/// which is deliberate and harmless here: these are *source* clips for the
/// decode path, not the §5.1 delivery lane, whose High 10 profile is asserted by
/// §10 step 1's precondition and by §9.1 fixture 8 in §10 step 6.
#[allow(clippy::too_many_lines)]
fn encode_hdr_clip(
    out: &Path,
    transfer_param: &str,
    leg: &str,
    extra_x264_params: Option<&str>,
    fill: fn(&mut ffmpeg::frame::Video, i64),
) -> String {
    let mut x264_params = format!(
        "{CC8_PRECONDITION_PRIMARIES_PARAM}:{transfer_param}:{CC8_PRECONDITION_MATRIX_PARAM}"
    );
    if let Some(extra) = extra_x264_params {
        x264_params.push(':');
        x264_params.push_str(extra);
    }

    let mut muxer = ffmpeg::format::output(&out).expect("the precondition muxer opens");
    let global_header = muxer
        .format()
        .flags()
        .contains(ffmpeg::format::Flags::GLOBAL_HEADER);
    let codec = ffmpeg::encoder::find_by_name(CC8_PRECONDITION_CODEC)
        .expect("libx264 was already asserted present");

    let time_base = ffmpeg::Rational(1, CC8_PRECONDITION_FPS);
    let frame_rate = ffmpeg::Rational(CC8_PRECONDITION_FPS, 1);
    let mut encoder = ffmpeg::codec::context::Context::new_with_codec(codec)
        .encoder()
        .video()
        .expect("libx264 opens as a video encoder");
    encoder.set_width(CC8_PRECONDITION_SIZE.0);
    encoder.set_height(CC8_PRECONDITION_SIZE.1);
    encoder.set_format(CC8_PRECONDITION_PIXEL);
    encoder.set_time_base(time_base);
    encoder.set_frame_rate(Some(frame_rate));
    encoder.set_bit_rate(CC8_PRECONDITION_BITRATE);
    encoder.set_gop(u32::try_from(CC8_PRECONDITION_FPS).unwrap_or(25));
    if global_header {
        encoder.set_flags(ffmpeg::codec::Flags::GLOBAL_HEADER);
    }
    // §5.2 clause 1: the encoder colourspace and range are the lane's, not a
    // literal BT.709. These two are the *only* colour fields `ffmpeg-next`
    // 8.0's encoder surface exposes; primaries and transfer have no setter at
    // all, which is §0.3(b)'s finding restated as an API fact.
    encoder.set_colorspace(ffmpeg::color::Space::BT2020NCL);
    encoder.set_color_range(ffmpeg::color::Range::MPEG);

    let mut options = ffmpeg::Dictionary::new();
    options.set("preset", "medium");
    options.set("x264-params", &x264_params);

    let mut encoder = match encoder.open_as_with(codec, options) {
        Ok(encoder) => encoder,
        Err(error) => fail_typed(
            &DeliveryColorError::UnsupportedField(DeliveryColorMismatch {
                field: "x264-params".to_owned(),
                observed: format!("libx264 refused \"{x264_params}\": {error}"),
                allowed: x264_params.clone(),
            }),
            leg,
            "this build's libx264 does not accept the HDR tag parameters, which invalidates \
             CC8 §0.2 Q2's lane",
        ),
    };

    let stream_index = {
        let mut stream = muxer.add_stream(codec).expect("the video stream is added");
        stream.set_time_base(time_base);
        stream.set_rate(frame_rate);
        stream.set_parameters(&encoder);
        stream.index()
    };
    muxer
        .write_header()
        .expect("the precondition header writes");
    let output_time_base = muxer
        .stream(stream_index)
        .expect("the video stream survives the header")
        .time_base();

    for index in 0..CC8_PRECONDITION_FRAMES {
        let mut frame = ffmpeg::frame::Video::new(
            CC8_PRECONDITION_PIXEL,
            CC8_PRECONDITION_SIZE.0,
            CC8_PRECONDITION_SIZE.1,
        );
        fill(&mut frame, index);
        // `export.rs:634-641`'s `stamp_delivery_yuv_color`, with §5.1's values
        // in place of BT.709's. Frame-level tags do not reach the SPS on their
        // own — that is the whole of §0.3(b) — so this is consistency, not the
        // mechanism under test.
        frame.set_color_space(ffmpeg::color::Space::BT2020NCL);
        frame.set_color_range(ffmpeg::color::Range::MPEG);
        frame.set_color_primaries(ffmpeg::color::Primaries::BT2020);
        frame.set_color_transfer_characteristic(
            if transfer_param == CC8_PRECONDITION_HLG_TRANSFER_PARAM {
                ffmpeg::color::TransferCharacteristic::ARIB_STD_B67
            } else {
                ffmpeg::color::TransferCharacteristic::SMPTE2084
            },
        );
        frame.set_pts(Some(index));
        encoder.send_frame(&frame).expect("the HDR frame encodes");
        drain(
            &mut encoder,
            &mut muxer,
            stream_index,
            time_base,
            output_time_base,
        );
    }
    encoder.send_eof().expect("the encoder flushes");
    drain(
        &mut encoder,
        &mut muxer,
        stream_index,
        time_base,
        output_time_base,
    );
    muxer
        .write_trailer()
        .expect("the precondition trailer writes");

    x264_params
}

/// `export.rs:679-695`'s `drain_packets`, at the one-tick packet duration the
/// production path uses for the same `mov` edit-list reason.
fn drain(
    encoder: &mut ffmpeg::encoder::Video,
    muxer: &mut ffmpeg::format::context::Output,
    stream_index: usize,
    encoder_time_base: ffmpeg::Rational,
    output_time_base: ffmpeg::Rational,
) {
    let mut packet = ffmpeg::Packet::empty();
    while encoder.receive_packet(&mut packet).is_ok() {
        packet.set_stream(stream_index);
        packet.set_duration(1);
        packet.rescale_ts(encoder_time_base, output_time_base);
        packet
            .write_interleaved(muxer)
            .expect("the precondition packet muxes");
    }
}

/// Assert one probed field, failing typed with the field's wire name.
fn assert_probed_field(observed: String, allowed: &str, field: &str, leg: &str, path: &Path) {
    if observed != allowed {
        fail_typed(
            &DeliveryColorError::UnsupportedField(DeliveryColorMismatch {
                field: field.to_owned(),
                observed,
                allowed: allowed.to_owned(),
            }),
            leg,
            &format!(
                "the tag did not survive the re-probe of {}; §5.2 clause 4 makes a tag that does \
                 not survive a failure, never a warning",
                path.display()
            ),
        );
    }
}

/// The precondition's shared body: encode one leg, re-probe it, and assert
/// §5.1's four colour fields and the lane depth came back.
///
/// `expected_transfer` is passed in rather than derived so the two legs cannot
/// silently assert the same thing.
fn assert_hdr_tags_survive_encode_and_reprobe(
    leg: &str,
    transfer_param: &str,
    expected_transfer: &ColorTransfer,
) -> ColorDescription {
    initialize_ffmpeg().expect("FFmpeg must initialize for the CC8 encoder precondition");
    assert_libx264_advertises_the_hdr_lane_pixel_format();

    let directory = TempDirectory::new("cc8-hdr-encoder-precondition");
    let path = directory.path(&format!("cc8-precondition-{leg}.mp4"));
    let x264_params = encode_hdr_precondition_clip(&path, transfer_param, leg);

    let asset = probe_path(&path, AssetId(1)).expect("the written HDR clip must re-probe");
    let color = asset.color_description;

    assert_probed_field(
        format!("{:?}", color.primaries),
        &format!("{:?}", ColorPrimaries::Bt2020),
        "primaries",
        leg,
        &path,
    );
    assert_probed_field(
        format!("{:?}", color.transfer),
        &format!("{expected_transfer:?}"),
        "transfer",
        leg,
        &path,
    );
    assert_probed_field(
        format!("{:?}", color.matrix),
        &format!("{:?}", ColorMatrix::Bt2020Ncl),
        "matrix",
        leg,
        &path,
    );
    assert_probed_field(
        format!("{:?}", color.range),
        &format!("{:?}", ColorRange::Limited),
        "range",
        leg,
        &path,
    );
    assert_probed_field(
        format!("{:?}", color.bit_depth),
        &format!("{:?}", ColorBitDepth::Ten),
        "bit_depth",
        leg,
        &path,
    );
    // A probed file necessarily carries stream metadata; anything else would
    // mean the probe inferred the description instead of reading it, and the
    // assertions above would then be measuring the probe's defaults rather
    // than libx264's SPS. This is the non-vacuity clause.
    assert_eq!(
        color.provenance,
        ColorProvenance::StreamMetadata,
        "the HDR tags must be read from the stream, not inferred: an inferred description would \
         make the tag assertions vacuous ({leg} leg)"
    );

    println!(
        "CC8_PRECONDITION leg={leg} codec={CC8_PRECONDITION_CODEC} \
         pixel_format={CC8_PRECONDITION_PIXEL:?} x264_params=\"{x264_params}\" \
         probed_primaries={:?} probed_transfer={:?} probed_matrix={:?} probed_range={:?} \
         probed_bit_depth={:?} probed_provenance={:?} confidence_basis_points={}",
        color.primaries,
        color.transfer,
        color.matrix,
        color.range,
        color.bit_depth,
        color.provenance,
        color.confidence_basis_points,
    );

    color
}

/// CC8 §0.3(e) / §10 step 1, HLG leg — the precondition for §5.1's lane as
/// specified.
///
/// Proves that *this build's* libx264 accepts
/// `colorprim=bt2020:transfer=arib-std-b67:colormatrix=bt2020nc` on a
/// `yuv420p10le` H.264 encode, and that all four tags plus the 10-bit depth
/// survive a re-probe through the production probe path.
///
/// **A red run on Windows is this test doing its job**, not a flake and not a
/// reason to add a `#[cfg]`: §0.3(e) says the MSVC/vcpkg x264 may not carry
/// `arib-std-b67`, and if it does not, §0.2 Q2's whole lane is invalid and CC8
/// must be re-answered rather than the test weakened.
#[test]
fn cc8_precondition_libx264_carries_hlg_hdr_tags_through_encode_and_reprobe() {
    let color = assert_hdr_tags_survive_encode_and_reprobe(
        "hlg",
        CC8_PRECONDITION_HLG_TRANSFER_PARAM,
        &ColorTransfer::AribStdB67,
    );
    // Distinctness from the PQ leg, asserted rather than assumed: two legs
    // that both re-probed as PQ would pass while proving half of what they
    // claim.
    assert_eq!(color.transfer, ColorTransfer::AribStdB67);
    assert_ne!(color.transfer, ColorTransfer::Smpte2084);
}

/// CC8 §0.3(e) / §10 step 1, PQ leg — §0.2 Q1's alternative transfer.
///
/// Same encoder build, same lane, `transfer=smpte2084` in place of
/// `arib-std-b67`. Running both legs means a build that carries one HDR
/// transfer and not the other is diagnosed by *which* test goes red, which is
/// the difference between "CC8's lane needs re-answering" and "CC8's lane is
/// fine and Q1's alternative is not available here".
///
/// **A red run on Windows is this test doing its job**, for §0.3(e)'s reason.
#[test]
fn cc8_precondition_libx264_carries_pq_hdr_tags_through_encode_and_reprobe() {
    let color = assert_hdr_tags_survive_encode_and_reprobe(
        "pq",
        CC8_PRECONDITION_PQ_TRANSFER_PARAM,
        &ColorTransfer::Smpte2084,
    );
    assert_eq!(color.transfer, ColorTransfer::Smpte2084);
    assert_ne!(color.transfer, ColorTransfer::AribStdB67);
}

// ===========================================================================
// CC8 §10 step 3 — §9.1 fixtures 1, 2 and 5.
// ===========================================================================
//
// Step 3 is "source profiles and transfer decode, with fixtures 1, 2, 5".
// Fixtures 1 and 2 are analytic: §9.1 states them as properties of the
// transfers and the anchor, so they are evaluated on the pinned functions and
// need no media. Fixture 5 is about *refusal reaching the production path*, so
// it drives a real BT.2020 10-bit clip through `probe_path`, the managed
// decoder, and the delivery conformance report.
//
// None of the three asserts anything about a rendered HDR frame. §3.3's
// primaries conversion, and with it the managed HDR frame decode, is §10 step 4
// below (fixtures 3, 4 and 10); the delivery lane is step 6 (fixtures 7, 8).

/// The number of magnitude bands §9.1 fixture 1's gate is reported in.
///
/// CC1 §6.2 bands a linear-domain comparison "by the magnitude of the CPU
/// reference value, because a half-float ULP doubles at 1.0": `|x| <= 1`,
/// `1 < |x| <= 2`, and everything above 2 excluded. §9.1 fixture 10 requires
/// the third band to be *extended* to HDR values and recorded rather than
/// dropped, and fixture 1's gate shape is "banded by magnitude as CC1 §6.2",
/// so the same three bands are used here with the third one live.
const CC8_BAND_COUNT: usize = 3;

/// The two band edges, in working-linear units: CC1 §6.2's `1` and `2`.
const CC8_BAND_EDGES: [f32; 2] = [1.0, 2.0];

/// The stable band names, for the `CC8_MEASURED` line and the eventual
/// `cc8_manifest.json` row.
const CC8_BAND_NAMES: [&str; CC8_BAND_COUNT] = ["abs_le_1", "abs_1_to_2", "abs_above_2_hdr"];

/// Max, P99, and mean absolute error over one magnitude band, with the
/// population that produced them.
#[derive(Debug, Clone, Copy, Default)]
struct Cc8BandError {
    max: f32,
    p99: f32,
    mean: f32,
    samples: usize,
}

/// Accumulate absolute errors into CC1 §6.2's magnitude bands.
#[derive(Debug, Default)]
struct Cc8BandedError {
    errors: [Vec<f32>; CC8_BAND_COUNT],
}

impl Cc8BandedError {
    fn push(&mut self, reference: f32, error: f32) {
        let magnitude = reference.abs();
        let band = if magnitude <= CC8_BAND_EDGES[0] {
            0
        } else if magnitude <= CC8_BAND_EDGES[1] {
            1
        } else {
            2
        };
        self.errors[band].push(error);
    }

    /// The band statistics. P99 is the `ceil(0.99 · n) - 1`-th order statistic
    /// of the sorted errors, which is CC1's convention: an inclusive
    /// nearest-rank percentile, so a single outlier in a hundred samples is
    /// still inside P99 rather than rounded away.
    #[allow(clippy::cast_precision_loss)]
    fn summarize(&self) -> [Cc8BandError; CC8_BAND_COUNT] {
        let mut summary = [Cc8BandError::default(); CC8_BAND_COUNT];
        for (band, errors) in self.errors.iter().enumerate() {
            if errors.is_empty() {
                continue;
            }
            let mut sorted = errors.clone();
            sorted.sort_by(f32::total_cmp);
            // `ceil(0.99 * n)` in integer arithmetic, so the rank is exact
            // and carries no float cast.
            let rank = (sorted.len() * 99).div_ceil(100).max(1).min(sorted.len()) - 1;
            summary[band] = Cc8BandError {
                max: sorted[sorted.len() - 1],
                p99: sorted[rank],
                mean: sorted.iter().sum::<f32>() / sorted.len() as f32,
                samples: sorted.len(),
            };
        }
        summary
    }
}

/// One 10-bit limited-range code as a normalized signal value, `(code - 64)
/// / 876`.
///
/// The full `0..=1023` sweep therefore spans undershoot below legal black
/// (negative signals, codes `0..64`), the nominal range, and the over-range
/// codes above legal white — the three populations §9.1 fixture 1 names
/// ("including negatives and over-range").
#[allow(clippy::cast_precision_loss)]
fn cc8_ten_bit_signal(code: u32) -> f32 {
    (code as f32 - 64.0) / 876.0
}

/// One band's recorded `(max, p99, mean)` measurement.
type Cc8MeasuredBand = [f32; 3];

/// §9.1 fixture 1's recorded measurement, per profile and per magnitude band.
///
/// Rows are `[max, p99, mean]` in the band order of [`CC8_BAND_NAMES`], and
/// the two outer entries are `pq_rec2020` then `hlg_rec2020`, matching
/// `kinewright_core::CC8_SOURCE_PROFILES`. The fixture's doc comment carries
/// the same figures with their sample counts and the date and toolchain they
/// were taken on; these are the numbers the assertions are derived from and
/// the numbers §10 step 10 carries into `cc8_manifest.json`.
const CC8_FIXTURE1_MEASURED: [[Cc8MeasuredBand; CC8_BAND_COUNT]; 2] = [
    [
        [4.249_811e-5, 3.075_6e-5, 1.359_05e-6],
        [8.761_883e-5, 8.761_883e-5, 1.008_877e-5],
        [1.080_322e-2, 1.014_709e-2, 6.137_814e-4],
    ],
    [
        [3.576_279e-7, 1.490_116e-7, 6.477_401e-9],
        [8.344_65e-7, 8.344_65e-7, 1.021_794e-7],
        [7.629_395e-6, 5.722_046e-6, 2.547_008e-7],
    ],
];

/// How far above the recorded measurement each assertion's bound sits.
///
/// **Two powers of two**, which is the argument `cc8_hdr`'s own test bounds
/// make and this fixture inherits rather than re-invents: "Rust does not
/// specify `f32::powf`'s accuracy at all, so the bound is loosened by two
/// further powers of two." Every term in §9.1 fixture 1 goes through `powf`,
/// `exp`, or `ln`, and CC8 runs on two operating systems with different libm
/// implementations, so the same allowance applies with the same reason.
///
/// §10 step 4's fixtures use the same rule, and each states which half of the
/// argument it is leaning on. Where a fixture's arithmetic is only `+`, `-`,
/// `*`, and `/` — §9.1 fixture 3's matrix round trip in isolation — IEEE 754
/// defines every operation exactly, so the figure is identical on both CI
/// operating systems and the 4x is a pure margin under §9.2's second rule
/// ("a budget must carry a real margin"). Where the raster reaching a fixture
/// was produced by the transfer stages, the libm half applies as well.
///
/// It is applied on top of [`cc8_next_power_of_two_bound`], so the realised
/// margin over the recorded figures is between 4x and 8x — a real margin
/// (§9.2's second rule), and still three or more orders of magnitude tighter
/// than a mis-transcribed transfer constant or a dropped stage would move the
/// round trip.
const CC8_MEASURED_BOUND_HEADROOM: f32 = 4.0;

/// Round a measured error up to the next power of two.
///
/// A *rule* rather than a chosen number, so every band's bound is derived the
/// same way and none of them is an invented figure. §9.2's "no invented
/// constant" applies to the bound as much as to the measurement.
fn cc8_next_power_of_two_bound(measured: f32) -> f32 {
    if measured <= 0.0 {
        return 0.0;
    }
    let exponent = measured.log2().ceil();
    exponent.exp2()
}

/// Sweep the full 10-bit ramp on one CC8 §2.1 profile, asserting §9.1
/// fixture 1's structural clauses and returning the banded round-trip error.
///
/// The structural clauses are asserted here rather than in the caller because
/// they are per-sample: strict monotonicity across the whole ramp, finiteness,
/// and the presence of the two populations fixture 1 names — undershoot below
/// legal black, and over-range values in the HDR band CC1 §6.2 excluded and
/// §9.1 fixture 10 extends. A ramp missing either population would make the
/// banded measurement prove less than it claims.
fn cc8_sweep_ten_bit_ramp(profile: ColorSourceProfile) -> [Cc8BandError; CC8_BAND_COUNT] {
    let mut banded = Cc8BandedError::default();
    let mut previous_working = f32::NEG_INFINITY;
    let mut saw_negative_working = false;
    let mut saw_over_range_working = false;

    for code in 0..=1_023_u32 {
        let signal = cc8_ten_bit_signal(code);
        let working = decode_hdr_source_working_linear(profile, [signal, signal, signal])
            .expect("both CC8 §2.1 profiles decode");
        assert!(
            working[0] > previous_working,
            "{} fell at 10-bit code {code} (signal {signal}): {} then {}",
            profile.id(),
            previous_working,
            working[0],
        );
        previous_working = working[0];
        assert!(
            working.iter().all(|value| value.is_finite()),
            "{} produced a non-finite working value at code {code}",
            profile.id(),
        );
        if working[0] < 0.0 {
            saw_negative_working = true;
        }
        if working[0] > CC8_BAND_EDGES[1] {
            saw_over_range_working = true;
        }

        let round_tripped = match profile {
            ColorSourceProfile::PqRec2020 => working
                .map(|value| cc8_pq_decode_working_linear(cc8_pq_encode_working_linear(value))),
            _ => cc8_hlg_decode_working_linear(cc8_hlg_encode_working_linear(working)),
        };
        for channel in 0..3 {
            banded.push(
                working[channel],
                (round_tripped[channel] - working[channel]).abs(),
            );
        }
    }

    assert!(
        saw_negative_working,
        "{}'s 10-bit ramp must reach below legal black, or the negative extension is untested",
        profile.id(),
    );
    assert!(
        saw_over_range_working,
        "{}'s 10-bit ramp must reach the HDR band above working 2.0, or §9.1 fixture 10's \
         extended band has no population",
        profile.id(),
    );
    banded.summarize()
}

/// §9.1 fixture 1, PQ leg and HLG leg: the transfer round trip over a 10-bit
/// ramp, banded by magnitude, with the seams recorded.
///
/// The round trip is measured **in the linear domain**, which is §9.2's row
/// shape: each 10-bit code is decoded to working linear, re-encoded, and
/// decoded again, and the absolute difference between the two working values is
/// what is banded. Measuring in the signal domain instead would report the
/// error where the transfer is steepest rather than where a grading node reads
/// it.
///
/// The figures in [`CC8_FIXTURE1_MEASURED`] are **measurements, not choices**,
/// taken by this fixture over the full `0..=1023` ramp on both profiles, on
/// `mifi/ffmpeg-builds 8.0-1` / rustc stable, 2026-08-29. Each band carries
/// 3 072 channel samples in total, split by magnitude:
///
/// ```text
/// profile      band              max          p99          mean        n
/// pq_rec2020   abs_le_1          4.249811e-5  3.075600e-5  1.359050e-6 1719
/// pq_rec2020   abs_1_to_2        8.761883e-5  8.761883e-5  1.008877e-5  195
/// pq_rec2020   abs_above_2_hdr   1.080322e-2  1.014709e-2  6.137814e-4 1158
/// hlg_rec2020  abs_le_1          3.576279e-7  1.490116e-7  6.477401e-9 2163
/// hlg_rec2020  abs_1_to_2        8.344650e-7  8.344650e-7  1.021794e-7  294
/// hlg_rec2020  abs_above_2_hdr   7.629395e-6  5.722046e-6  2.547008e-7  615
/// ```
///
/// PQ is three to four orders of magnitude looser than HLG, and that is the
/// standard's shape rather than a defect: ST 2084's `(p - c1)` subtraction
/// cancels and its outer `^(1/m1)` multiplies relative error by `1/m1 ~ 6.28`,
/// which is the same amplification `cc8_hdr`'s own
/// `PQ_ROUND_TRIP_RELATIVE_BOUND` derives, while HLG's branches are a square
/// root and a logarithm with no comparable cancellation.
///
/// They are not §9.2 gate constants yet: [`kinewright_core::CC8_GATES`] row
/// "PQ/HLG transfer round trip" still reads
/// `ToBeMeasuredAtImplementation`, because §10 step 10 is the step that writes
/// a measured arm and reconciles the manifest. What this fixture does now is
/// take the measurement, assert against it with a stated margin, and print it
/// under `CC8_MEASURED` so step 10 has a recorded figure rather than a fresh
/// guess.
#[test]
fn cc8_transfer_round_trip_over_a_ten_bit_ramp_is_banded_and_monotone() {
    let mut evidence = Vec::new();
    for (profile_index, profile) in [
        ColorSourceProfile::PqRec2020,
        ColorSourceProfile::HlgRec2020,
    ]
    .into_iter()
    .enumerate()
    {
        let summary = cc8_sweep_ten_bit_ramp(profile);
        for (index, band) in summary.iter().enumerate() {
            assert!(
                band.samples > 0,
                "{} band {} is empty; a gate over an empty band proves nothing",
                profile.id(),
                CC8_BAND_NAMES[index],
            );
            // Each term is bounded by the **recorded** figure raised two
            // powers of two, never by the live one: a bound computed from the
            // value it is bounding would pass unconditionally, which §9.1's
            // "no vacuous assertion" rule forbids. A regression that made the
            // round trip more than 4-8x worse fails here; an improvement is
            // reported by the printed line and re-pinned by §10 step 10.
            let recorded = CC8_FIXTURE1_MEASURED[profile_index][index];
            for (term_index, (term, value)) in
                [("max", band.max), ("p99", band.p99), ("mean", band.mean)]
                    .into_iter()
                    .enumerate()
            {
                let bound =
                    cc8_next_power_of_two_bound(recorded[term_index]) * CC8_MEASURED_BOUND_HEADROOM;
                assert!(
                    bound > 0.0 && bound.is_finite(),
                    "{} {} {term} has no recorded measurement to bound it",
                    profile.id(),
                    CC8_BAND_NAMES[index],
                );
                assert!(
                    value <= bound,
                    "{} {} {term} = {value:.6e} exceeds its recorded bound {bound:.6e} \
                     (recorded measurement {:.6e})",
                    profile.id(),
                    CC8_BAND_NAMES[index],
                    recorded[term_index],
                );
                println!(
                    "CC8_MEASURED fixture=1 gate=\"PQ/HLG transfer round trip\" \
                     profile={} band={} term={term} measured={value:.6e} \
                     recorded={:.6e} bound={bound:.6e} margin={:.2}x samples={}",
                    profile.id(),
                    CC8_BAND_NAMES[index],
                    recorded[term_index],
                    bound / value.max(f32::MIN_POSITIVE),
                    band.samples,
                );
            }
            evidence.push((profile.id(), CC8_BAND_NAMES[index], *band));
        }
    }
    assert_eq!(evidence.len(), 2 * CC8_BAND_COUNT);
}

/// §9.1 fixture 1's seam clause: "exact analytic inverse at the segment seams,
/// with the seam behaviour recorded explicitly as CC1 §3.1 does for BT.709."
///
/// Three seams exist across the two transfers and each is asserted as an
/// **equality** rather than a tolerance, because each is exact — which is why
/// `float_cmp` is allowed here, exactly as `cc8_hdr`'s own
/// `cc8_hlg_oetf_anchor_points` and `cc8_pq_eotf_holds_the_analytic_endpoints_exactly`
/// allow it. A tolerance here would let a mis-transcribed constant through.
#[test]
#[allow(clippy::float_cmp)]
fn cc8_transfer_segment_seams_are_exact_analytic_inverses() {
    //
    // 1. ARIB STD-B67's branch seam at scene `1/12` <-> signal `0.5`. `b = 1 -
    //    4a` and `c = 0.5 - a*ln(4a)` make both branches meet there, and in f32
    //    with the standard's rounded decimals they meet exactly.
    assert_eq!(
        cc8_hlg_inverse_oetf(CC8_HLG_SIGNAL_BREAKPOINT),
        CC8_HLG_SCENE_BREAKPOINT
    );
    assert_eq!(
        cc8_hlg_oetf(CC8_HLG_SCENE_BREAKPOINT),
        CC8_HLG_SIGNAL_BREAKPOINT
    );
    // 2. ST 2084's flat foot. Its inverse sends every strictly positive
    //    luminance to at least `c1^m2`, so the EOTF is not injective below
    //    that signal and the round trip is an identity in one direction only.
    //    The foot sits below the first non-zero 10-bit code, which is why the
    //    banded measurement above never enters it — recorded here so a later
    //    reader sees that as the standard's shape and not as a gap.
    let foot = CC8_PQ_C1.powf(CC8_PQ_M2);
    assert_eq!(cc8_pq_eotf_nits(foot), 0.0);
    assert!(
        foot < cc8_ten_bit_signal(65),
        "the ST 2084 foot {foot} must be under the first non-zero 10-bit code",
    );
    // 3. ST 2084's pole at `(c2/c3)^m2`, above every 10-bit code, where the
    //    rational form's denominator vanishes. Unreachable from real media and
    //    sign-preservingly infinite from synthetic input rather than plausibly
    //    finite.
    let pole = (CC8_PQ_C2 / CC8_PQ_C3).powf(CC8_PQ_M2);
    assert!(cc8_ten_bit_signal(1_023) < pole);
    assert!(cc8_pq_eotf_nits(pole).is_infinite());
    // And zero is exact in both directions on both transfers, which is what
    // the `sgn(0) = 0` convention buys.
    assert_eq!(cc8_pq_decode_working_linear(0.0), 0.0);
    assert_eq!(cc8_hlg_decode_working_linear([0.0; 3]), [0.0; 3]);
    println!(
        "CC8_MEASURED fixture=1 seams hlg_scene_breakpoint={CC8_HLG_SCENE_BREAKPOINT} \
         hlg_signal_breakpoint={CC8_HLG_SIGNAL_BREAKPOINT} pq_foot={foot:.6e} pq_pole={pole}"
    );
}

/// §9.1 fixture 2: the reference-white anchor lands the pinned working values.
///
/// "A PQ source at exactly 203 nits lands at working `1.0`; 1 000 nits at
/// `≈ 4.93`; 10 000 nits at `≈ 49.3`. Values pinned as integers in the
/// authority module."
///
/// No number in §9.1's sentence is written here. 203 is
/// [`CC8_REFERENCE_WHITE_NITS`], 10 000 is [`CC8_PQ_PEAK_NITS`], and 1 000 is
/// [`CC8_HLG_NOMINAL_PEAK_NITS`] — §2.2 pins the last as HLG's nominal peak,
/// and the specular highlight §2.2 works through on the PQ side is the same
/// luminance, which is why one constant serves both statements. `4.93` and
/// `49.3` are the quotients of those pinned integers and are never literals.
///
/// The HLG leg is §10 step 3's own determination, recorded in
/// `cc8_hlg_decode_working_linear`'s doc comment: BT.2408's 75 % HLG signal
/// lands on the *same* working `1.0`, so one anchor serves both profiles.
#[test]
fn cc8_reference_white_anchor_lands_the_pinned_working_values() {
    /// The relative bound this fixture holds the anchor identities to.
    ///
    /// Derivation, not choice. Each identity is a PQ round trip
    /// (`nits -> E' -> nits`) followed by one division, so it inherits
    /// `cc8_hdr`'s own `PQ_ROUND_TRIP_RELATIVE_BOUND` argument: about ten
    /// rounded operations amplified by ST 2084's `(p - c1)` cancellation
    /// (≈ 6.1) and the final `^(1/m1)` (≈ 6.28), giving ≈ `2^-14` analytically,
    /// loosened two powers of two to `2^-12` because Rust does not specify
    /// `f32::powf`'s accuracy. Measured worst over the four identities below is
    /// **4.375267e-5** (2026-08-29), a 5.58x margin under the bound. The four
    /// identities measure, in order,
    /// `pq_reference_white` 2.205372e-5, `pq_specular_highlight` 4.375267e-5,
    /// `pq_peak` 0, and `hlg_peak` 9.679794e-8. It is the PQ specular highlight
    /// that sets the worst case, which is the ST 2084 cancellation the bound is
    /// derived from; `pq_peak` is exactly zero because `E' = 1` decodes to the
    /// peak exactly and the anchor divide is the only remaining operation, and
    /// HLG is three orders of magnitude tighter for the reason
    /// `cc8_transfer_round_trip_over_a_ten_bit_ramp_is_banded_and_monotone`
    /// records.
    const ANCHOR_RELATIVE_BOUND: f32 = 1.0 / 4_096.0;

    #[allow(clippy::cast_precision_loss)]
    fn nits(value: i32) -> f32 {
        value as f32
    }

    fn relative_error(actual: f32, expected: f32) -> f32 {
        ((actual - expected) / expected).abs()
    }

    let anchor = nits(CC8_REFERENCE_WHITE_NITS);
    let mut worst = 0.0_f32;
    let mut record = |label: &str, actual: f32, expected: f32| {
        let error = relative_error(actual, expected);
        worst = worst.max(error);
        assert!(
            error <= ANCHOR_RELATIVE_BOUND,
            "{label}: {actual} is not {expected} (relative error {error})",
        );
        println!(
            "CC8_MEASURED fixture=2 identity={label} working={actual:.9} \
             expected={expected:.9} relative_error={error:.6e}"
        );
    };

    // §9.1 fixture 2's three PQ statements, each with the anchor divide fused
    // in by `cc8_pq_decode_working_linear` so the second stage cannot be lost.
    record(
        "pq_reference_white",
        cc8_pq_decode_working_linear(cc8_pq_inverse_eotf(anchor)),
        1.0,
    );
    record(
        "pq_specular_highlight",
        cc8_pq_decode_working_linear(cc8_pq_inverse_eotf(nits(CC8_HLG_NOMINAL_PEAK_NITS))),
        nits(CC8_HLG_NOMINAL_PEAK_NITS) / anchor,
    );
    record(
        "pq_peak",
        cc8_pq_decode_working_linear(1.0),
        nits(CC8_PQ_PEAK_NITS) / anchor,
    );

    // §3.1's headroom claim, which the peak value is the whole argument for:
    // "PQ's 10 000-nit peak is ≈ 49.3 in working units, far inside f16's
    // 65 504 maximum". Asserted through `half::f16` rather than against the
    // number 65 504, so this is a statement about the storage type the working
    // space actually uses.
    let peak = cc8_pq_decode_working_linear(1.0);
    assert!(f16::from_f32(peak).is_finite() && f16::from_f32(peak).to_f32() > 49.0);

    // §10 step 3's HLG determination: the same anchor, reached through
    // §3.3's HLG stage chain.
    let hlg_white = nits(CC8_HLG_REFERENCE_WHITE_SIGNAL_PERCENT) / 100.0;
    let decoded_white = cc8_hlg_decode_working_linear([hlg_white; 3]);
    // The bound is half a nit expressed in working units: 203 is BT.2408's own
    // figure rounded to integer cd/m², so agreeing to better than half a nit is
    // exactly the claim that this reproduces it, and a tighter bound would be
    // asserting more precision than the standard states.
    let half_nit_in_working_units = 0.5 / anchor;
    assert!(
        (decoded_white[0] - 1.0).abs() < half_nit_in_working_units,
        "75% HLG decodes to working {decoded_white:?}, not 1.0",
    );
    record(
        "hlg_peak",
        cc8_hlg_decode_working_linear([1.0; 3])[0],
        nits(CC8_HLG_NOMINAL_PEAK_NITS) / anchor,
    );
    println!(
        "CC8_MEASURED fixture=2 reference_white_nits={CC8_REFERENCE_WHITE_NITS} \
         pq_peak_nits={CC8_PQ_PEAK_NITS} hlg_nominal_peak_nits={CC8_HLG_NOMINAL_PEAK_NITS} \
         hlg_reference_white_signal_percent={CC8_HLG_REFERENCE_WHITE_SIGNAL_PERCENT} \
         worst_relative_error={worst:.6e}"
    );

    // The failing direction: without §2.2's normalization the three identities
    // are absolute cd/m², not working units, and diffuse white would land 203×
    // high. Asserting the gap keeps this fixture from passing on a decode that
    // simply forgot the anchor.
    assert!(cc8_pq_eotf_nits(cc8_pq_inverse_eotf(anchor)) > anchor / 2.0);
}

/// Every HDR-adjacent tuple CC8 §2.1 places outside the closed set, built by
/// mutating **one** field of a complete, D65-stamped HDR description so each
/// refusal is attributable to exactly one column.
///
/// The list is enumerated from `kinewright_core`'s own tables —
/// [`CC8_SOURCE_PROFILES`] and [`CC8_REJECTED_HDR_ADJACENT`] — rather than
/// written out here, so a rejection that stopped being reachable fails
/// §9.1 fixture 5 instead of quietly disappearing from it.
fn cc8_fixture5_cases(complete: &ColorDescription) -> Vec<(String, ColorDescription)> {
    let mut cases: Vec<(String, ColorDescription)> = Vec::new();

    // §2.1's 10-bit floor, on **both** profiles: "8-bit PQ, 8-bit HLG".
    for row in &CC8_SOURCE_PROFILES {
        let transfer = if row.transfer == CC8_SOURCE_PROFILES[0].transfer {
            ColorTransfer::Smpte2084
        } else {
            ColorTransfer::AribStdB67
        };
        for depth in [ColorBitDepth::Eight, ColorBitDepth::Integer(9)] {
            cases.push((
                format!("{}_depth_{depth:?}", row.id),
                ColorDescription {
                    transfer: transfer.clone(),
                    bit_depth: depth,
                    ..complete.clone()
                },
            ));
        }
    }

    // The named matrix and primaries families, driven from the table.
    for rejected in &CC8_REJECTED_HDR_ADJACENT {
        let description = match rejected.observed {
            "bt2020_cl" => Some(ColorMatrix::Bt2020Cl),
            "ictcp" => Some(ColorMatrix::Ictcp),
            "chroma_derived_ncl" => Some(ColorMatrix::ChromaDerivedNcl),
            "chroma_derived_cl" => Some(ColorMatrix::ChromaDerivedCl),
            _ => None,
        }
        .map(|matrix| ColorDescription {
            matrix,
            ..complete.clone()
        })
        .or_else(|| {
            match rejected.observed {
                "display_p3" => Some(ColorPrimaries::DisplayP3),
                "dci_p3" => Some(ColorPrimaries::DciP3),
                _ => None,
            }
            .map(|primaries| ColorDescription {
                primaries,
                ..complete.clone()
            })
        });
        if let Some(description) = description {
            cases.push((format!("rejected_{}", rejected.observed), description));
        }
    }

    // "every mismatched primaries/transfer pair": each HDR transfer on
    // non-Rec.2020 primaries, and Rec.2020 primaries with an SDR transfer.
    for transfer in [ColorTransfer::Smpte2084, ColorTransfer::AribStdB67] {
        for primaries in [
            ColorPrimaries::Bt709,
            ColorPrimaries::Srgb,
            ColorPrimaries::DisplayP3,
            ColorPrimaries::DciP3,
        ] {
            cases.push((
                format!("mismatched_{primaries:?}_{transfer:?}"),
                ColorDescription {
                    primaries,
                    transfer: transfer.clone(),
                    ..complete.clone()
                },
            ));
        }
    }
    for transfer in [
        ColorTransfer::Bt709,
        ColorTransfer::Bt1886,
        ColorTransfer::Srgb,
    ] {
        cases.push((
            format!("mismatched_Bt2020_{transfer:?}"),
            ColorDescription {
                transfer,
                ..complete.clone()
            },
        ));
    }
    cases
}

/// Assert that one HDR-adjacent tuple is refused everywhere §9.1 fixture 5
/// requires: by the classifier, by the managed decoder on real media, and by
/// the delivery conformance report that gates export.
///
/// Returns the refusal's `(code, field)` so the caller can prove the case list
/// covered every family §2.1 names rather than hitting one reason repeatedly.
fn cc8_assert_case_is_refused(
    label: &str,
    description: &ColorDescription,
    asset: &kinewright_core::MediaAsset,
    path: &Path,
    fps: Rational,
    assumption: Option<ColorSourceProfileAssumption>,
) -> (String, String) {
    // 1. The classifier refuses, typed, with all four §2.1 facts.
    let error = classify_source_with_assumption(description, assumption)
        .expect_err("an HDR-adjacent tuple outside §2.1's closed set must be refused, not guessed");
    assert!(!error.code().is_empty(), "{label} has no code");
    assert!(!error.field().is_empty(), "{label} has no field");
    assert!(
        !error.observed().is_empty(),
        "{label} has no observed value"
    );
    assert!(
        !error.allowed_values().is_empty(),
        "{label} names no allowed values",
    );
    assert!(
        error
            .recovery_action()
            .contains("Apply an explicit supported source-colour override"),
        "{label} names no recovery action: {}",
        error.recovery_action(),
    );
    assert!(
        error.actionable_message().contains(&error.observed()),
        "{label}'s actionable message drops its observed value",
    );

    // 2. The managed decoder refuses the same tuple on real media, so the
    //    refusal is not confined to the classifier.
    let Err(decode_error) =
        VideoDecoder::open_scaled_managed(path, fps, None, description, assumption)
    else {
        panic!("{label} must block managed decode")
    };
    let decode_message = decode_error.to_string();
    assert!(
        decode_message.contains("managed source profile rejected"),
        "{label} blocked for the wrong reason: {decode_message}",
    );

    // 3. Export is blocked: the delivery conformance report carries the
    //    same code, field, observed value, allowed values, and recovery.
    let mut blocked_asset = asset.clone();
    blocked_asset.color_description = description.clone();
    let document = single_clip_document(blocked_asset);
    let report = delivery_conformance(
        &document,
        DeliveryProfile::SourceMaster,
        DeliveryEncodeDepth::Ten,
        50,
        50,
    )
    .expect("an unsupported source is reported, not returned as an error");
    let issue = report
        .issues
        .iter()
        .find(|issue| issue.code == "unsupported_source_color")
        .unwrap_or_else(|| panic!("{label} must block managed export"));
    assert_eq!(issue.severity, QaSeverity::Error);
    for fragment in [
        format!("code={}", error.code()),
        format!("field={}", error.field()),
        format!("observed={}", error.observed()),
        format!("allowed={}", error.allowed_values()),
    ] {
        assert!(
            issue.message.contains(&fragment),
            "{label}'s export block omits {fragment:?}: {}",
            issue.message,
        );
    }
    assert!(
        issue.message.contains("Apply an explicit supported"),
        "{label}'s export block omits the recovery action",
    );
    assert!(!report.export_ready(), "{label} must not be exportable");

    (error.code().to_owned(), error.field().to_owned())
}

/// The other half of §1's rule, and §7 item 2's: a source that **is** one of
/// §2.1's profiles is not silently treated as Rec.709 either.
///
/// It blocks managed export on the HDR-source/SDR-delivery mismatch — which
/// §0.2 Q6 makes permanent, not scaffolding — while the managed *frame* decoder
/// now opens it, because §10 step 4 landed §3.3's primaries conversion and the
/// BT.2020 NCL source matrix decode. The two answers are asserted together here
/// so neither can be mistaken for the other: a decodable HDR frame is not a
/// deliverable one, and §5.1's lane is still §10 step 6's.
fn cc8_assert_classified_hdr_source_blocks_export_and_frame_decode(
    asset: &kinewright_core::MediaAsset,
    complete: &ColorDescription,
    path: &Path,
    fps: Rational,
    assumption: Option<ColorSourceProfileAssumption>,
) {
    let mut hdr_asset = asset.clone();
    hdr_asset.color_description = complete.clone();
    let hdr_document = single_clip_document(hdr_asset);
    let hdr_report = delivery_conformance(
        &hdr_document,
        DeliveryProfile::SourceMaster,
        DeliveryEncodeDepth::Ten,
        50,
        50,
    )
    .expect("an HDR source is reported, not returned as an error");
    let hdr_issue = hdr_report
        .issues
        .iter()
        .find(|issue| issue.code == HDR_SOURCE_ON_SDR_DELIVERY)
        .expect("§7 item 2: an HDR source with an SDR delivery target blocks export");
    assert_eq!(hdr_issue.severity, QaSeverity::Error);
    assert!(
        hdr_issue
            .message
            .contains(ColorSourceProfile::PqRec2020.id())
    );
    assert!(!hdr_report.export_ready());
    // §10 step 4: the managed frame decoder opens the same source it refused at
    // step 3. The failing direction is now the opposite one — a refusal here
    // would mean the primaries stage or the BT.2020 NCL matrix decode had been
    // lost — so the assertion is that it opens, and the export block above is
    // what proves the two answers stayed separate.
    VideoDecoder::open_scaled_managed(path, fps, None, complete, assumption).unwrap_or_else(
        |error| {
            panic!("§10 step 4 must decode a classified CC8 §2.1 HDR source: {error}");
        },
    );
}

/// §9.1 fixture 5: every HDR-adjacent tuple outside CC8 §2.1's closed set
/// blocks managed decode and export, and names its recovery action.
///
/// §9.1's list, verbatim: "8-bit PQ, 8-bit HLG, `bt2020_cl`, `ictcp`, P3
/// primaries, and every mismatched primaries/transfer pair block managed proof
/// and export and name the recovery action."
///
/// The cases are enumerated from the authority module —
/// [`CC8_REJECTED_HDR_ADJACENT`] and [`CC8_SOURCE_PROFILES`] — rather than from
/// a list written here, so a rejection that stopped being reachable would fail
/// this fixture instead of quietly disappearing from it.
///
/// The source is a **real** BT.2020 10-bit clip, written by §10 step 1's own
/// encoder helper and re-probed through the production `probe_path`. That
/// matters for two reasons. It makes the *passing* direction non-vacuous: the
/// unmutated probed description classifies as a CC8 profile, so every refusal
/// below is the mutation being refused rather than the media being unreadable.
/// And it makes the refusals reach the production surfaces — the managed
/// decoder and the delivery conformance report — rather than the classifier
/// alone.
///
/// **Managed *proof* is covered through the decoder**, which is the stage the
/// proof path opens a source with (`render.rs`'s
/// `VideoDecoder::open_scaled_managed`); the proof renderer itself needs a GPU
/// and its CC8 parity fixture is §9.1 fixture 10, in §10 step 4.
#[test]
fn cc8_unsupported_hdr_metadata_blocks_managed_decode_and_export() {
    initialize_ffmpeg().expect("FFmpeg must initialize for the CC8 §9.1 fixture 5 source");
    let directory = TempDirectory::new("cc8-unsupported-hdr-metadata");
    let path = directory.path("cc8-fixture5-source.mp4");
    let _ = encode_hdr_precondition_clip(&path, CC8_PRECONDITION_PQ_TRANSFER_PARAM, "fixture5");

    let asset = probe_path(&path, AssetId(1)).expect("the fixture 5 source must probe");
    let probed = asset.color_description.clone();
    // A probed H.264 stream carries no white point, so §2.1's D65 rule is on
    // the passing path here — which is the rule this fixture also proves does
    // not rewrite the raw metadata.
    assert_eq!(probed.white_point, ColorWhitePoint::Unknown);
    let assumption = Some(ColorSourceProfileAssumption::D65);
    assert_eq!(
        classify_source_with_assumption(&probed, assumption),
        Ok(ColorSourceProfile::PqRec2020),
        "the passing direction: the unmutated probed source is a CC8 §2.1 profile",
    );
    assert_eq!(
        probed.white_point,
        ColorWhitePoint::Unknown,
        "§2.1: the raw source metadata stays Unknown. No code may rewrite it.",
    );
    assert_eq!(
        classify_source(&probed),
        Err(ColorSourceError::UnknownWhitePoint),
        "and without the explicit assumption it stays an honest unknown",
    );

    let complete = ColorDescription {
        white_point: ColorWhitePoint::D65,
        ..probed.clone()
    };
    let cases = cc8_fixture5_cases(&complete);

    let fps = Rational::new(1, 1).expect("one fps");
    let mut evidence = Vec::new();
    for (label, description) in &cases {
        let (code, field) =
            cc8_assert_case_is_refused(label, description, &asset, &path, fps, assumption);
        evidence.push((label.clone(), code, field));
    }

    // Non-vacuity on the count and on the coverage: every family §2.1 names
    // must have produced at least one case, and the two profile rows must each
    // have contributed their own depth rejections.
    assert_eq!(cases.len(), evidence.len());
    assert_eq!(
        cases.len(),
        4 + CC8_REJECTED_HDR_ADJACENT.len() - 2 + 8 + 3,
        "the case list must be the authority table's, not a hand-written one",
    );
    for family in [
        "unsupported_hdr_source_bit_depth",
        "unsupported_hdr_source_matrix",
        "unsupported_source_primaries",
        "unsupported_source_transfer",
    ] {
        assert!(
            evidence.iter().any(|(_, code, _)| code == family),
            "no case produced {family}",
        );
    }

    cc8_assert_classified_hdr_source_blocks_export_and_frame_decode(
        &asset, &complete, &path, fps, assumption,
    );

    println!(
        "CC8_MEASURED fixture=5 cases={} probed_primaries={:?} probed_transfer={:?} \
         probed_matrix={:?} probed_range={:?} probed_bit_depth={:?} codes={:?}",
        cases.len(),
        probed.primaries,
        probed.transfer,
        probed.matrix,
        probed.range,
        probed.bit_depth,
        {
            let mut codes: Vec<&str> = evidence.iter().map(|(_, code, _)| code.as_str()).collect();
            codes.sort_unstable();
            codes.dedup();
            codes
        },
    );
}

// ===========================================================================
// CC8 §10 step 4 — §9.1 fixtures 3, 4 and 10.
// ===========================================================================
//
// Step 4 is "Primaries conversion, with fixture 3; then fixtures 4 and 10."
// Three things land with it and each has its gate below:
//
//  * §2.3/§3.3's **primaries conversion to working BT.709 D65**
//    (`color_pipeline::source_primaries_to_working_linear`), non-identity for
//    the first time in the project's history — §9.1 fixture 3;
//  * the **managed HDR frame decode**: `decode::managed_filter_graph` now
//    carries the BT.2020 NCL source matrix into swscale's YCbCr -> RGB
//    conversion, `color_pipeline::rgba64_normalization_max` has the BT.2020
//    limited sibling of CC1 §3.1's `P_8` special case, and
//    `frame::WorkingFrame::from_rgba64_le` runs §3.3's HDR chain — gated by the
//    two production-path fixtures between fixture 3 and fixture 4; and
//  * §9.1 fixture 4's no-intermediate-clamp claim and fixture 10's CPU/GPU
//    parity, both at HDR magnitudes.
//
// **Where the stages run, and why.** CC1 §3.1 splits the managed input
// conversion in two: an "explicitly configured swscale conversion" performs
// range expansion and the matrix decode at *decode time on the CPU*, producing
// coded RGB in `RGBA64`, and "after that boundary, Kinewright owns
// normalization, transfer decoding, every primary-control equation, output
// transfer, output range, and final packing" — with normalization and transfer
// decode also on the CPU, in `WorkingFrame::from_rgba64_le`, and only the
// *grading nodes* on the GPU. CC8 changes neither the split nor §3.3's order:
// the two `*` transfer stages, the HLG OOTF, the reference-white normalization,
// and the primaries conversion are all source-side stages, so all five run
// where CC1 already runs the source-side work — once per pixel at decode time,
// before the working frame exists. **No CC8 shader work is required by §10
// step 4 and none is added.** The compositor sees a working BT.709 D65 linear
// frame and cannot tell whether it came from an HDR or an SDR source, which is
// exactly §3.1's claim that "the working space is unchanged".
//
// That is also why fixture 10 is a *parity* fixture rather than a shader
// fixture: what §9.1 fixture 10 asks for is that the existing grading shader,
// which CC1 gated on `|linear| <= 2`, still matches the CPU reference when the
// magnitudes it is handed are HDR ones. It does not ask for a new GPU stage,
// and adding one would be a change §3.3 does not order.

/// One pinned CC8 luminance constant as `f32`.
///
/// `kinewright_core::cc8_as_f32`'s idiom, restated locally for the fixture's
/// own arithmetic so the cast lint is answered once rather than at each call
/// site. Every value it converts — 203, 1 000, 10 000 — is far inside `2^24`,
/// so the conversion is exact.
#[allow(clippy::cast_precision_loss)]
fn cc8_nits(value: i32) -> f32 {
    value as f32
}

/// The width of §9.1 fixture 4's HDR ramp, in pixels.
const CC8_RAMP_WIDTH: u32 = 64;

/// The height of §9.1 fixture 4's HDR ramp, in pixels. Four rows rather than
/// one so the production linear sampler is measured on texel interiors, which
/// is CC1's reason for its own multi-row rasters.
const CC8_RAMP_HEIGHT: u32 = 4;

/// One vertical bar of §10 step 4's synthetic HDR source clip, in 10-bit
/// limited-range BT.2020 NCL code values.
///
/// Authored in Rust as `cc7_sources` and §10 step 1's encoder are, and encoded
/// **losslessly** (`qp=0`), so the decoded samples are exactly these codes and
/// a fixture can state its expectation from the numbers it wrote rather than
/// from what a lossy encoder produced. Every bar's coded RGB stays inside
/// `[0, 65535/65280]` so that no channel is clipped at the unsigned `RGBA64`
/// boundary; that ceiling is CC1 §3.1's `P_8` convention seen from above and is
/// why the over-range bar is code 943 rather than something more dramatic.
#[derive(Debug, Clone, Copy)]
struct Cc8SourceBar {
    label: &'static str,
    luma: u16,
    cb: u16,
    cr: u16,
}

/// The eight bars, in raster order.
///
/// Four are neutral and carry §2.2's own worked points — legal black, the
/// half-signal, BT.2408's 75 % HDR reference white, and legal white, which is
/// the HLG nominal peak. Three are saturated enough to sit **outside the
/// Rec.709 triangle**, which is what makes the primaries conversion's negative
/// components reachable from real media rather than only from a synthetic
/// triple. The last is an over-range code above legal white.
const CC8_SOURCE_BARS: [Cc8SourceBar; 8] = [
    Cc8SourceBar {
        label: "legal_black",
        luma: 64,
        cb: 512,
        cr: 512,
    },
    Cc8SourceBar {
        label: "hlg_half_signal",
        luma: 502,
        cb: 512,
        cr: 512,
    },
    Cc8SourceBar {
        label: "hlg_reference_white_75",
        luma: 721,
        cb: 512,
        cr: 512,
    },
    Cc8SourceBar {
        label: "hlg_legal_white",
        luma: 940,
        cb: 512,
        cr: 512,
    },
    Cc8SourceBar {
        label: "wide_gamut_red",
        luma: 400,
        cb: 380,
        cr: 780,
    },
    Cc8SourceBar {
        label: "wide_gamut_green",
        luma: 600,
        cb: 350,
        cr: 330,
    },
    Cc8SourceBar {
        label: "wide_gamut_blue",
        luma: 350,
        cb: 780,
        cr: 430,
    },
    Cc8SourceBar {
        label: "over_legal_white",
        luma: 943,
        cb: 512,
        cr: 512,
    },
];

/// The width of one bar in the [`CC8_PRECONDITION_SIZE`] raster.
const CC8_BAR_WIDTH: usize = CC8_PRECONDITION_SIZE.0 as usize / CC8_SOURCE_BARS.len();

/// The 10-bit source depth of every CC8 §2.1 clip in this file: §2.1's floor.
const CC8_SOURCE_BITS: u32 = 10;

/// Fill one `yuv420p10le` picture with [`CC8_SOURCE_BARS`].
///
/// Bar boundaries land on multiples of [`CC8_BAR_WIDTH`] (8), which is even, so
/// no 4:2:0 chroma sample straddles two bars and the interior of every bar is
/// exact after chroma upsampling. The picture is identical on every frame
/// because the encode is lossless and the fixtures read frame zero; a moving
/// picture would add nothing and would cost a motion-compensated decode.
fn fill_hdr_bars(frame: &mut ffmpeg::frame::Video, _frame_index: i64) {
    let width = CC8_PRECONDITION_SIZE.0 as usize;
    let height = CC8_PRECONDITION_SIZE.1 as usize;

    let luma_stride = frame.stride(0);
    let luma = frame.data_mut(0);
    for row in 0..height {
        for column in 0..width {
            let bar = CC8_SOURCE_BARS[column / CC8_BAR_WIDTH];
            let at = row * luma_stride + column * 2;
            luma[at..at + 2].copy_from_slice(&bar.luma.to_le_bytes());
        }
    }

    for (plane, chroma) in [(1_usize, false), (2_usize, true)] {
        let stride = frame.stride(plane);
        let data = frame.data_mut(plane);
        for row in 0..height / 2 {
            for column in 0..width / 2 {
                let bar = CC8_SOURCE_BARS[(column * 2) / CC8_BAR_WIDTH];
                let value = if chroma { bar.cr } else { bar.cb };
                let at = row * stride + column * 2;
                data[at..at + 2].copy_from_slice(&value.to_le_bytes());
            }
        }
    }
}

/// CC1 §3.1's native-code reference equations, at BT.2020 non-constant
/// luminance: one bar's limited-range 10-bit `Y'CbCr` to coded RGB.
///
/// This is **fixture-reference evidence**, exactly as CC1 §3.1 says of its own
/// BT.709 equations ("the native source-code equations that follow are
/// fixture-reference evidence for the configured matrix/range conversion, not
/// the direct normalization algorithm after swscale"). The production path
/// takes the matrix inside swscale; this recomputes it independently so the two
/// can be compared.
///
/// The coefficients are `kinewright_core::cc8_hdr`'s — `CC8_BT2020_KR`,
/// `CC8_BT2020_KG`, `CC8_BT2020_KB` and the two normalization denominators §6
/// item 1 pins — and not a literal, so a mis-transcribed coefficient fails the
/// authority module's own test rather than silently agreeing with itself here.
/// It is evaluated in `f64` and narrowed once, in the manner CC1's reference
/// equations are.
#[allow(clippy::cast_possible_truncation)]
fn cc8_bt2020_limited_coded_rgb(bar: Cc8SourceBar) -> [f32; 3] {
    let scale = f64::from(1_u32 << (CC8_SOURCE_BITS - 8));
    let luma = (f64::from(bar.luma) - 16.0 * scale) / (219.0 * scale);
    let cb = (f64::from(bar.cb) - 128.0 * scale) / (224.0 * scale);
    let cr = (f64::from(bar.cr) - 128.0 * scale) / (224.0 * scale);
    let red = luma + CC8_BT2020_CR_DENOMINATOR * cr;
    let blue = luma + CC8_BT2020_CB_DENOMINATOR * cb;
    let green = (luma - CC8_BT2020_KR * red - CC8_BT2020_KB * blue) / CC8_BT2020_KG;
    [red as f32, green as f32, blue as f32]
}

/// The §2.1 HLG description of the clip [`cc8_hdr_bar_source`] writes, with
/// §2.1's explicit D65 assumption already applied to the white point.
fn cc8_hlg_source_description(probed: &ColorDescription) -> ColorDescription {
    ColorDescription {
        white_point: ColorWhitePoint::D65,
        ..probed.clone()
    }
}

/// Encode, probe, and managed-decode §10 step 4's synthetic HLG source clip.
///
/// Returns the temporary directory (which must outlive the frame), the probed
/// and D65-completed description, and the decoded working frame.
fn cc8_hdr_bar_source(label: &str) -> (TempDirectory, ColorDescription, WorkingFrame) {
    initialize_ffmpeg().expect("FFmpeg must initialize for the CC8 §10 step 4 source");
    let directory = TempDirectory::new(label);
    let path = directory.path("cc8-step4-hlg-bars.mp4");
    let x264_params = encode_hdr_clip(
        &path,
        CC8_PRECONDITION_HLG_TRANSFER_PARAM,
        label,
        Some("qp=0"),
        fill_hdr_bars,
    );
    let asset = probe_path(&path, AssetId(1)).expect("the step 4 HDR source must probe");
    let probed = asset.color_description.clone();
    // The passing direction is not vacuous only if the clip really is one of
    // §2.1's profiles, so this is asserted rather than assumed.
    assert_eq!(probed.primaries, ColorPrimaries::Bt2020);
    assert_eq!(probed.transfer, ColorTransfer::AribStdB67);
    assert_eq!(probed.matrix, ColorMatrix::Bt2020Ncl);
    assert_eq!(probed.range, ColorRange::Limited);
    assert_eq!(probed.bit_depth, ColorBitDepth::Ten);
    let description = cc8_hlg_source_description(&probed);
    assert_eq!(
        classify_source_with_assumption(&description, Some(ColorSourceProfileAssumption::D65)),
        Ok(ColorSourceProfile::HlgRec2020),
    );
    println!(
        "CC8_MEASURED step=4 source x264_params=\"{x264_params}\" bars={}",
        CC8_SOURCE_BARS.len()
    );
    let frame = decode_managed_working_frame(&path, &description);
    (directory, description, frame)
}

/// The decoded working RGB of one bar, sampled at the bar's interior.
///
/// Column and row are both taken at the centre of the bar so no sample sits on
/// a 4:2:0 chroma-upsampling seam; CC1's own bar fixture samples bar interiors
/// for the same reason.
fn cc8_decoded_bar(frame: &WorkingFrame, bar_index: usize) -> [f32; 3] {
    let column = bar_index * CC8_BAR_WIDTH + CC8_BAR_WIDTH / 2;
    let row = frame.height as usize / 2;
    let at = (row * frame.width as usize + column) * 4;
    [
        frame.pixels[at].to_f32(),
        frame.pixels[at + 1].to_f32(),
        frame.pixels[at + 2].to_f32(),
    ]
}

/// One `Rgba16Float` unit in the last place at `magnitude`.
///
/// The smallest difference the working storage format can represent there, and
/// therefore the smallest difference a CPU/GPU comparison over that storage can
/// *report*. CC1 §6.2 already reasons this way — its over-range P99 of
/// `9.765625e-4` is exactly one `f16` ULP just above 1.0, chosen "instead of
/// quietly widening the gate everywhere" — so this is CC1's own derivation made
/// into a function rather than a second convention.
fn cc8_half_float_ulp(magnitude: f32) -> f32 {
    let value = f16::from_f32(magnitude.abs().max(1.0));
    (f16::from_bits(value.to_bits() + 1).to_f32() - value.to_f32()).abs()
}

/// Bound one measured term by the step-3 rule and print it as evidence.
///
/// The rule, unchanged from §9.1 fixture 1: the bound is
/// `next_power_of_two(recorded) * CC8_MEASURED_BOUND_HEADROOM`, computed from
/// the **recorded** figure and never from the live one, because a bound derived
/// from the value it bounds passes unconditionally.
///
/// `floor` is the answer to §9.2's second rule in the one case the
/// next-power-of-two rule cannot handle: a term that **measured exactly zero**
/// on the passing source. Zero has no next power of two, and a zero bound is
/// not a gate — it would fail on the first machine whose GPU rounds one sample
/// differently while measuring nothing about the pipeline. §9.2 names the
/// remedy: "where a term measures zero on the passing source, a deliberately
/// starved fixture bounds the constant from above." So a caller with a zero
/// recorded term passes the storage format's own granularity at the band's top
/// magnitude — [`cc8_half_float_ulp`], a derived number and not an invented one
/// — and must *also* assert a starved control that exceeds it, which is what
/// makes the bound reachable rather than decorative. Callers whose recorded
/// term is non-zero pass `0.0` and the floor never binds.
fn cc8_assert_measured(
    fixture: &str,
    gate: &str,
    term: &str,
    measured: f32,
    recorded: f32,
    floor: f32,
) {
    let bound = (cc8_next_power_of_two_bound(recorded) * CC8_MEASURED_BOUND_HEADROOM).max(floor);
    assert!(
        bound > 0.0 && bound.is_finite(),
        "fixture {fixture} {gate} {term} has no recorded measurement to bound it, and no \
         starved floor either",
    );
    assert!(
        measured <= bound,
        "fixture {fixture} {gate} {term} = {measured:.6e} exceeds its recorded bound \
         {bound:.6e} (recorded measurement {recorded:.6e})",
    );
    // A term that measured zero has no finite margin ratio, and printing one
    // would put a meaningless 1e36 into the evidence a later step reads.
    let margin = if measured > 0.0 {
        format!("{:.2}x", bound / measured)
    } else {
        "unbounded_below_measurement".to_owned()
    };
    println!(
        "CC8_MEASURED fixture={fixture} gate=\"{gate}\" term={term} measured={measured:.6e} \
         recorded={recorded:.6e} bound={bound:.6e} floor={floor:.6e} margin={margin}",
    );
}

/// Max, P99, and mean of a set of absolute errors, in CC1's nearest-rank P99
/// convention.
fn cc8_summarize(errors: &mut [f32]) -> Cc8MeasuredBand {
    assert!(
        !errors.is_empty(),
        "a summary over no samples proves nothing"
    );
    errors.sort_by(f32::total_cmp);
    #[allow(clippy::cast_precision_loss)]
    let mean = errors.iter().sum::<f32>() / errors.len() as f32;
    let rank = (errors.len() * 99).div_ceil(100).max(1).min(errors.len()) - 1;
    [errors[errors.len() - 1], errors[rank], mean]
}

// ---------------------------------------------------------------------------
// §9.1 fixture 3 — the primaries stage's gate.
// ---------------------------------------------------------------------------

/// The number of saturated hues swept around the Rec.2020 RGB cube's surface.
const CC8_HUE_STEPS: usize = 96;

/// One fully saturated Rec.2020 hue, on the surface of the RGB cube.
///
/// Six linear segments between the primaries and secondaries. Every one of
/// these is at or near the Rec.2020 gamut boundary and therefore outside the
/// Rec.709 triangle for most of the sweep, which is what §9.1 fixture 3 means
/// by "a raster including out-of-709 primaries".
fn cc8_saturated_rec2020_hue(step: usize) -> [f32; 3] {
    #[allow(clippy::cast_precision_loss)]
    let position = 6.0 * step as f32 / CC8_HUE_STEPS as f32;
    let sector = position.floor();
    let fraction = position - sector;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    match sector as usize % 6 {
        0 => [1.0, fraction, 0.0],
        1 => [1.0 - fraction, 1.0, 0.0],
        2 => [0.0, 1.0, fraction],
        3 => [0.0, 1.0 - fraction, 1.0],
        4 => [fraction, 0.0, 1.0],
        _ => [1.0, 0.0, 1.0 - fraction],
    }
}

/// §9.1 fixture 3's raster: Rec.2020 linear light at four magnitudes.
///
/// The magnitudes are the pinned constants' own quotients and never literals —
/// diffuse white is `1.0`, the specular highlight is
/// `CC8_HLG_NOMINAL_PEAK_NITS / CC8_REFERENCE_WHITE_NITS`, and the top is
/// `CC8_PQ_PEAK_NITS / CC8_REFERENCE_WHITE_NITS`, so the raster spans the whole
/// working range §3.1's headroom argument is about. Each magnitude carries the
/// neutral triple (an in-gamut control, which the matrix must leave neutral),
/// the three saturated Rec.2020 primaries, and [`CC8_HUE_STEPS`] saturated
/// hues.
fn cc8_wide_gamut_rec2020_raster() -> Vec<[f32; 3]> {
    let anchor = cc8_nits(CC8_REFERENCE_WHITE_NITS);
    let magnitudes = [
        0.05_f32,
        1.0,
        cc8_nits(CC8_HLG_NOMINAL_PEAK_NITS) / anchor,
        cc8_nits(CC8_PQ_PEAK_NITS) / anchor,
    ];
    let mut raster = Vec::with_capacity(magnitudes.len() * (CC8_HUE_STEPS + 4));
    for magnitude in magnitudes {
        raster.push([magnitude; 3]);
        for basis in [[1.0_f32, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]] {
            raster.push(basis.map(|channel| channel * magnitude));
        }
        for step in 0..CC8_HUE_STEPS {
            raster.push(cc8_saturated_rec2020_hue(step).map(|channel| channel * magnitude));
        }
    }
    raster
}

/// §9.1 fixture 3's recorded measurement: `[max, p99, mean]` absolute error of
/// the Rec.2020 -> BT.709 -> Rec.2020 round trip, linear domain, over
/// [`cc8_wide_gamut_rec2020_raster`].
///
/// Taken by this fixture on `mifi/ffmpeg-builds 8.0-1` / rustc stable,
/// 2026-08-29, over **1 200 RGB channel samples**:
///
/// ```text
/// term  measured      bound (next_pow2 x 4)  margin
/// max   7.629395e-6   3.051758e-5            4.00x
/// p99   3.814697e-6   1.525879e-5            4.00x
/// mean  3.456745e-7   1.907349e-6            5.52x
/// ```
///
/// The figures scale with the raster's magnitude, which reaches
/// `CC8_PQ_PEAK_NITS / CC8_REFERENCE_WHITE_NITS ~ 49.26`: §9.2's row shape for
/// this gate is *absolute* linear error, so the number is dominated by the top
/// magnitude and is deliberately not normalised. `7.63e-6` is one `f32` ULP at
/// 49.26 rounded up, which is the round trip doing as well as the working
/// arithmetic's own width allows.
const CC8_FIXTURE3_MEASURED: Cc8MeasuredBand = [7.629_395e-6, 3.814_697_3e-6, 3.456_745e-7];

/// §9.1 fixture 3 — **primaries round trip**, at raster scale.
///
/// §9.1: "Rec.2020 -> BT.709 -> Rec.2020 over a raster including out-of-709
/// primaries; the intermediate carries negatives (asserted present, so the
/// fixture cannot pass vacuously on in-gamut content); the round trip is within
/// the §9.2 linear budget."
///
/// `cc8_hdr`'s own `cc8_primaries_round_trip_carries_negatives_not_a_clamp`
/// prototyped this on four triples in the authority module; this is that claim
/// at raster scale and, critically, **through the production stage**
/// `color_pipeline::source_primaries_to_working_linear` rather than through a
/// bare matrix multiply, so a stage that selected the wrong matrix — or applied
/// none — fails here.
///
/// Four things are asserted that the prototype could not:
///
/// 1. **The stage is selected by source profile and by nothing else** (§3.3).
///    Both §2.1 rows produce the same conversion, because both carry the same
///    `Primaries` column; both CC1 SDR profiles return their argument
///    **bit-for-bit**, which is the identity §3.3 promises and the byte-equality
///    §9.1 fixture 6 will demand.
/// 2. **The negatives are present and material.** A round-trip fixture over
///    in-gamut content would pass while proving nothing about §2.3's central
///    claim, so the count of negative channels and the most negative value are
///    both asserted, and the second is asserted against a threshold no rounding
///    residue could reach.
/// 3. **The stage is not the identity on an HDR profile.** The failing
///    direction for a conversion that was accidentally removed.
/// 4. **Nothing is clamped.** Every negative survives to the round trip, which
///    is what the error budget below measures; a clamp would show up as an
///    error of order the negative itself, four orders of magnitude above the
///    recorded figure.
#[test]
// The three `assert_eq!`s below are equalities **by design**, not tolerances:
// §3.3 promises the SDR arm returns its argument unchanged (byte-equality, the
// obligation §9.1 fixture 6 will inherit) and both §2.1 rows select the same
// matrix, so a near-equality here would let a real regression through.
#[allow(clippy::float_cmp)]
fn cc8_primaries_round_trip_over_a_wide_gamut_raster_carries_negatives() {
    let raster = cc8_wide_gamut_rec2020_raster();
    let mut errors: Vec<f32> = Vec::with_capacity(raster.len() * 3);
    let mut negative_channels = 0_usize;
    let mut most_negative = 0.0_f32;
    let mut largest_stage_change = 0.0_f32;

    for rec2020 in &raster {
        let working = source_primaries_to_working_linear(ColorSourceProfile::PqRec2020, *rec2020);
        // Clause 1: the matrix is a property of the profile's primaries column,
        // so both §2.1 rows must agree exactly.
        assert_eq!(
            working,
            source_primaries_to_working_linear(ColorSourceProfile::HlgRec2020, *rec2020),
            "both CC8 §2.1 rows carry the same primaries column",
        );
        // Clause 1, the SDR half: bit-for-bit identity, not "close to it".
        assert_eq!(
            source_primaries_to_working_linear(ColorSourceProfile::Rec709Video, *rec2020),
            *rec2020,
        );
        assert_eq!(
            source_primaries_to_working_linear(ColorSourceProfile::SrgbFull, *rec2020),
            *rec2020,
        );

        for channel in 0..3 {
            if working[channel] < 0.0 {
                negative_channels += 1;
                most_negative = most_negative.min(working[channel]);
            }
            largest_stage_change =
                largest_stage_change.max((working[channel] - rec2020[channel]).abs());
        }

        let back = cc8_apply_matrix(CC8_BT709_TO_REC2020, working);
        for channel in 0..3 {
            errors.push((back[channel] - rec2020[channel]).abs());
        }
    }

    // Clause 2: the intermediate carries negatives, and enough of them that the
    // fixture cannot have passed on in-gamut content. A saturated Rec.2020 hue
    // sweep leaves a negative BT.709 component over most of its arc, so this
    // threshold is a floor well under what the raster actually produces rather
    // than a number tuned to it.
    assert!(
        negative_channels * 10 > errors.len(),
        "only {negative_channels} of {} channels went negative; §9.1 fixture 3 requires the \
         intermediate to carry negatives, asserted present",
        errors.len(),
    );
    assert!(
        most_negative < -0.1,
        "the most negative BT.709 component was {most_negative}, which is rounding noise rather \
         than an out-of-triangle colour",
    );
    // Clause 3: on an HDR profile the stage is emphatically not the identity.
    assert!(
        largest_stage_change > 0.1,
        "the primaries stage moved nothing ({largest_stage_change}); a lost conversion would \
         composite Rec.2020 values as BT.709, which CC8 §1 forbids",
    );

    let sample_count = errors.len();
    let measured = cc8_summarize(&mut errors);
    for (index, term) in ["max", "p99", "mean"].into_iter().enumerate() {
        cc8_assert_measured(
            "3",
            "Primaries round trip",
            term,
            measured[index],
            CC8_FIXTURE3_MEASURED[index],
            0.0,
        );
    }
    println!(
        "CC8_MEASURED fixture=3 samples={sample_count} negative_channels={negative_channels} \
         most_negative={most_negative:.6} largest_stage_change={largest_stage_change:.6}",
    );
}

// ---------------------------------------------------------------------------
// The managed HDR frame decode: the production path §10 step 4 opened.
// ---------------------------------------------------------------------------

/// §10 step 4's recorded measurement for the managed decode of
/// [`CC8_SOURCE_BARS`]: `[max, p99, mean]` absolute working-linear error
/// between the decoded frame and the CPU reference chain.
///
/// Taken on `mifi/ffmpeg-builds 8.0-1` / rustc stable, 2026-08-29, over **24
/// RGB channel samples** (eight bars):
///
/// ```text
/// term  measured      bound (next_pow2 x 4)  margin
/// max   1.163006e-3   7.812500e-3            6.72x
/// p99   1.163006e-3   7.812500e-3            6.72x
/// mean  2.775358e-4   1.953125e-3            7.04x
/// ```
///
/// The error here is **not** the primaries stage's: it is swscale's fixed-point
/// matrix and range rounding at the `RGBA64` boundary plus the `Rgba16Float`
/// storage quantization, amplified by the HLG inverse OETF and the OOTF's
/// `^1.2` gain and then scaled by the anchor — which is why it is two to three
/// orders of magnitude looser than §9.1 fixture 3's pure-matrix figure, and why
/// the bound is measured here rather than inherited from there. It is a
/// *reported* comparison against a CPU reference, never an equality against a
/// decoded code, so CC7 §0.3 PM-E12's rule is satisfied: the constant is one
/// number asserted on both operating systems with no `cfg(windows)` and no
/// per-OS value.
const CC8_MANAGED_DECODE_MEASURED: Cc8MeasuredBand = [1.163_006e-3, 1.163_006e-3, 2.775_358e-4];

/// The managed decode of a classified CC8 §2.1 HDR source lands working
/// BT.709 D65 linear light — §10 step 4's production-path gate.
///
/// This is the fixture that would have failed at §10 step 3, when
/// `VideoDecoder::open_scaled_managed` refused an HDR profile outright. It
/// exercises the whole of §3.3's source side through the production path:
/// swscale's BT.2020 NCL matrix decode and range expansion, the `RGBA64`
/// normalization, the ARIB STD-B67 inverse OETF, the BT.2100 OOTF at §2.2's
/// nominal peak and gamma, the reference-white normalization, and the primaries
/// conversion — with no clamp between any two of them.
///
/// The CPU reference recomputes the same chain independently: CC1 §3.1's
/// native-code equations at BT.2020 NCL
/// ([`cc8_bt2020_limited_coded_rgb`], from `cc8_hdr`'s own coefficients), then
/// `color_pipeline::hdr_source_to_working_bt709_linear`. So the comparison is
/// between two different realisations of §3.3's order, not between the pipeline
/// and itself.
///
/// Three non-vacuity clauses:
///
/// * **the anchor lands where §2.2 says it does** on real media — BT.2408's
///   75 % HLG signal at working `1.0` and legal white at the nominal peak over
///   the anchor;
/// * **the decoded frame carries negative BT.709 components** on the three
///   wide-gamut bars, which is §2.3's design reaching the compositor; and
/// * **the HDR band is populated**, so the fixture is measuring HDR magnitudes
///   rather than an SDR raster that happens to be tagged.
#[test]
fn cc8_managed_decode_of_an_hdr_source_lands_working_bt709_linear() {
    let (_directory, _description, frame) = cc8_hdr_bar_source("cc8-step4-managed-decode");
    let anchor = cc8_nits(CC8_REFERENCE_WHITE_NITS);

    let mut errors: Vec<f32> = Vec::with_capacity(CC8_SOURCE_BARS.len() * 3);
    let mut negative_channels = 0_usize;
    let mut hdr_band_channels = 0_usize;
    for (index, bar) in CC8_SOURCE_BARS.into_iter().enumerate() {
        let expected = hdr_source_to_working_bt709_linear(
            ColorSourceProfile::HlgRec2020,
            cc8_bt2020_limited_coded_rgb(bar),
        )
        .expect("the HLG profile decodes");
        let decoded = cc8_decoded_bar(&frame, index);
        for channel in 0..3 {
            errors.push((decoded[channel] - expected[channel]).abs());
            if decoded[channel] < 0.0 {
                negative_channels += 1;
            }
            if decoded[channel].abs() > CC8_BAND_EDGES[1] {
                hdr_band_channels += 1;
            }
            assert!(
                decoded[channel].is_finite(),
                "{} channel {channel} decoded to a non-finite {}",
                bar.label,
                decoded[channel],
            );
        }
        println!(
            "CC8_MEASURED fixture=managed_decode bar={} luma={} cb={} cr={} \
             decoded={decoded:?} expected={expected:?}",
            bar.label, bar.luma, bar.cb, bar.cr,
        );
    }

    // §2.2's two anchored points, on real media rather than on the pinned
    // functions: this is §9.1 fixture 2's claim carried through the decoder.
    let reference_white = cc8_decoded_bar(&frame, 2);
    let half_nit_in_working_units = 0.5 / anchor;
    for channel in reference_white {
        assert!(
            (channel - 1.0).abs() < half_nit_in_working_units,
            "the 75% HLG bar decoded to {reference_white:?}, not working 1.0",
        );
    }
    let legal_white = cc8_decoded_bar(&frame, 3);
    let expected_peak = cc8_nits(CC8_HLG_NOMINAL_PEAK_NITS) / anchor;
    for channel in legal_white {
        assert!(
            (channel - expected_peak).abs() < 4.0 * half_nit_in_working_units,
            "legal white decoded to {legal_white:?}, not {expected_peak}",
        );
    }
    // §2.3's negatives, reaching the compositor from real media.
    assert!(
        negative_channels >= 3,
        "no wide-gamut bar produced a negative BT.709 component; the primaries stage did not \
         reach the decoded frame",
    );
    // And the HDR band really is populated.
    assert!(
        hdr_band_channels >= 3,
        "the decoded raster never exceeded working {}; this fixture would be measuring an SDR \
         raster",
        CC8_BAND_EDGES[1],
    );

    let sample_count = errors.len();
    let measured = cc8_summarize(&mut errors);
    for (index, term) in ["max", "p99", "mean"].into_iter().enumerate() {
        cc8_assert_measured(
            "managed_decode",
            "Managed HDR decode vs CPU reference",
            term,
            measured[index],
            CC8_MANAGED_DECODE_MEASURED[index],
            0.0,
        );
    }
    println!(
        "CC8_MEASURED fixture=managed_decode samples={sample_count} \
         negative_channels={negative_channels} hdr_band_channels={hdr_band_channels}",
    );
}

/// The BT.2020 limited lane uses CC1 §3.1's `P_8` denominator, measured.
///
/// CC1 §3.1 records that "limited BT.709 YUV-to-RGB conversion uses `FFmpeg`'s
/// 8-bit fixed-point RGB scale even when the source planes are 10 bits (or
/// deeper), so its nominal legal-white denominator is `P_8 = 65280`, not
/// `P_N`", and that legal white lands on `65283` after fixed-point rounding.
/// `color_pipeline::rgba64_normalization_max` had no BT.2020 sibling for that
/// special case, so §10 step 4 added one — and this fixture is why the addition
/// is a measurement rather than an assumption.
///
/// It measures the denominator **through the decoded frame** rather than by
/// asserting a number about swscale: the legal-white bar's working value is
/// compared with the two candidate predictions, `P_8 = 65280` and the 10-bit
/// promoted maximum `P_10 = 65472`, and the fixture requires the `P_8`
/// prediction to be nearer by a wide margin. That is a statement the fixture
/// can make on either CI operating system without gating on one build's decode
/// output (CC7 §0.3 PM-E12): both predictions are computed from the pinned
/// constants, and what is asserted is which of the two the pipeline agrees
/// with, not an equality against a decoded code.
#[test]
fn cc8_bt2020_limited_boundary_uses_the_p8_denominator() {
    let mut limited_10 = ColorDescription {
        primaries: ColorPrimaries::Bt2020,
        transfer: ColorTransfer::AribStdB67,
        matrix: ColorMatrix::Bt2020Ncl,
        range: ColorRange::Limited,
        white_point: ColorWhitePoint::D65,
        bit_depth: ColorBitDepth::Ten,
        confidence_basis_points: 10_000,
        provenance: ColorProvenance::UserOverride,
    };
    // The two candidates, from the two functions that compute them.
    assert_eq!(rgba64_promoted_max(&limited_10), Ok(65_472));
    assert_eq!(rgba64_normalization_max(&limited_10), Ok(65_280));
    // The BT.709 answer is unmoved, which is the SDR half of the claim.
    limited_10.matrix = ColorMatrix::Bt709;
    assert_eq!(rgba64_normalization_max(&limited_10), Ok(65_280));

    let (_directory, _description, frame) = cc8_hdr_bar_source("cc8-step4-p8-denominator");
    let legal_white = cc8_decoded_bar(&frame, 3)[0];
    // Both predictions run the same §3.3 chain; only the denominator differs.
    // `P_8` maps legal-white RGBA64 65283 to a signal just above 1.0, `P_10`
    // maps it to 0.99711 — and the HLG chain's `^1.2` gain turns that into a
    // working difference far outside any rounding budget.
    let predict = |denominator: f32| {
        let signal = 65_283.0 / denominator;
        cc8_hlg_decode_working_linear([signal; 3])[0]
    };
    let p8 = predict(65_280.0);
    let p10 = predict(65_472.0);
    let p8_error = (legal_white - p8).abs();
    let p10_error = (legal_white - p10).abs();
    assert!(
        p8_error * 8.0 < p10_error,
        "the BT.2020 limited boundary does not use P_8: decoded {legal_white}, \
         P_8 prediction {p8} (error {p8_error}), P_10 prediction {p10} (error {p10_error})",
    );
    println!(
        "CC8_MEASURED fixture=p8_denominator decoded={legal_white:.6} p8={p8:.6} \
         p8_error={p8_error:.6e} p10={p10:.6} p10_error={p10_error:.6e}"
    );
}

// ---------------------------------------------------------------------------
// §9.1 fixture 4 — no intermediate clamp, at HDR magnitudes.
// ---------------------------------------------------------------------------

/// One `primary_correction` effect carrying every control of `correction`.
///
/// `cc1_fixtures`' equivalent is private to that module; this is the same
/// construction over `PrimaryParameter::ALL` so a control added to the
/// descriptor cannot be silently dropped from a CC8 effect.
fn cc8_correction_effect(id: u64, correction: PrimaryCorrection) -> Effect {
    Effect {
        id: EffectId(id),
        name: "primary_correction".to_owned(),
        parameters: PrimaryParameter::ALL
            .into_iter()
            .map(|parameter| {
                (
                    parameter.name().to_owned(),
                    ParamValue::Integer(correction.parameter(parameter)),
                )
            })
            .collect(),
        keyframes: BTreeMap::new(),
    }
}

/// §9.1 fixture 4's recorded measurement: `[max, p99, mean]` absolute
/// working-linear error of the two-node HDR recovery through the production
/// WGSL compositor, over the HDR ramp.
///
/// Taken on lavapipe (LLVM 20.1.2, 256 bits) / rustc stable, 2026-08-29, over
/// **768 RGB channel samples**, 88 of them in the HDR band:
///
/// ```text
/// term  measured  floor (one f16 ULP at 49.25)  starved control
/// max   0         3.125000e-2                   4.889645e+1
/// p99   0         3.125000e-2                   "
/// mean  0         3.125000e-2                   "
/// ```
///
/// **Zero is the measurement, not a missing one.** A `+1.5 / -1.5` stop pair is
/// inexact in `f32`, but its residual is of order `2^-23` relative — four orders
/// of magnitude below one `Rgba16Float` ULP at these magnitudes — so it vanishes
/// at the working storage boundary and every recovered sample comes back on the
/// same `f16` code it started on. §9.2's second rule governs that case
/// ("where a term measures zero on the passing source, a deliberately starved
/// fixture bounds the constant from above"), so the gate is the storage
/// granularity `cc8_half_float_ulp(49.25) = 2^-5` and the starved control is
/// what a clamping pipeline would report on the same raster: `48.9`, three
/// orders of magnitude above the bound, which is what makes the bound reachable
/// rather than decorative.
const CC8_FIXTURE4_MEASURED: Cc8MeasuredBand = [0.0, 0.0, 0.0];

/// §9.1 fixture 4 — **no intermediate clamp on HDR**.
///
/// §9.1: "An HDR highlight raster is corrected with a negative exposure and
/// recovers values that a clamp would have destroyed — CC1 §6.1's fixture 4, at
/// HDR magnitudes."
///
/// The raster is **decoded**, not invented: each ramp sample is an HLG signal
/// value taken through `color_pipeline::hdr_source_to_working_bt709_linear`, so
/// the magnitudes under test are the ones §3.3's chain actually produces and the
/// fixture cannot pass by testing a number no source can reach.
///
/// Three claims, in CC1 §6.1 fixture 4's own shape:
///
/// 1. **Recovery.** `+1 stop` then `-1 stop` through the production WGSL
///    compositor returns the HDR input.
/// 2. **The clamp control is materially different.** Clamping to `[0, 1]`
///    between the two nodes destroys the highlight; the fixture asserts the gap
///    rather than merely asserting recovery, because a comparison of the
///    pipeline with itself proves nothing.
/// 3. **A ramp, not a pixel**, at a **non-dyadic** exposure. CC1 §6.1.4's
///    reason for the ramp: a single texel can pass by accident, and a clamp at
///    any stage shows up as a plateau. The non-dyadic gain is CC8's own
///    addition and is why the ramp's recovery error is a live measurement — see
///    the comment at the ramp. Every ramp sample above working `2.0` is asserted
///    to have survived at its own value rather than at the clamped plateau's,
///    and the clamped plateau is measured as the starved control that bounds
///    the recovery constant from above (§9.2's second rule).
#[test]
#[allow(clippy::too_many_lines)]
fn cc8_no_intermediate_clamp_recovers_hdr_highlights() {
    let positive = PrimaryCorrection {
        exposure_milli_stops: 1_000,
        ..PrimaryCorrection::default()
    };
    let negative = PrimaryCorrection {
        exposure_milli_stops: -1_000,
        ..PrimaryCorrection::default()
    };
    // One HDR highlight, decoded through §3.3's chain from an HLG signal at
    // legal white: the nominal peak over the anchor, ~4.93.
    let highlight = hdr_source_to_working_bt709_linear(ColorSourceProfile::HlgRec2020, [1.0; 3])
        .expect("the HLG profile decodes")
        .map(|value| f16::from_f32(value).to_f32());
    assert!(
        highlight[0] > CC8_BAND_EDGES[1],
        "the fixture's input must be an HDR magnitude, not an over-range SDR one: {highlight:?}",
    );

    // Claim 1, on the CPU reference first.
    let over_range = positive
        .apply_checked(highlight)
        .expect("positive exposure on an HDR highlight");
    let recovered =
        apply_primary_corrections(highlight, &[positive, negative]).expect("HDR recovery");
    for channel in 0..3 {
        assert!(
            (recovered[channel] - highlight[channel]).abs()
                <= f32::EPSILON * highlight[channel].abs() * 8.0,
            "CPU recovery lost the HDR highlight: {recovered:?} vs {highlight:?}",
        );
    }
    // Claim 2: what a display-range clamp between the two nodes would produce.
    let clamped_between_nodes = negative
        .apply_checked(over_range.map(|value| value.clamp(0.0, 1.0)))
        .expect("clamped HDR recovery");
    assert!(
        (clamped_between_nodes[0] - recovered[0]).abs() > highlight[0] / 4.0,
        "the clamped control produced the managed result: clamped={clamped_between_nodes:?} \
         managed={recovered:?}",
    );

    // Claim 1 and 3 through the production WGSL compositor.
    let gpu = fallback_gpu();
    let compositor = Compositor::new(gpu.context());
    let single = working_frame(1, 1, &[highlight]);
    let working = compositor
        .render_working(
            (1, 1),
            &[CompositorLayer {
                frame: &single,
                effects: &[
                    cc8_correction_effect(1, positive),
                    cc8_correction_effect(2, negative),
                ],
                transition: TransitionRenderParams::default(),
            }],
        )
        .expect("production WGSL HDR no-intermediate-clamp readback")
        .pixels;
    for channel in 0..3 {
        assert!(
            (working[channel] - highlight[channel]).abs() <= highlight[channel].abs() * 1.0e-2,
            "production WGSL clamped or failed HDR recovery: actual={} expected={}",
            working[channel],
            highlight[channel],
        );
    }

    // Claim 3: an HDR ramp reaching the PQ peak in working units, recovered by
    // a **non-dyadic** exposure pair.
    //
    // CC1's fixture 4 uses +/-1 stop, which is a multiply and a divide by two
    // and is therefore exact in binary floating point, so a zero recovery error
    // there would say nothing. +/-1.5 stops has a gain of `2^1.5`, which is not
    // a power of two, and the round trip is inexact in `f32`.
    //
    // **It still measures identically zero**, and that is the finding rather
    // than a hole in the fixture: the `f32` round-trip residual is of order
    // `2^-23` relative, four orders of magnitude below one `Rgba16Float` ULP at
    // these magnitudes, so it vanishes at the working storage boundary. A term
    // that is zero on the passing source is exactly the case §9.2's second rule
    // legislates for, so this gate is the storage floor
    // (`cc8_half_float_ulp` at the ramp's top) plus the starved clamping
    // control that bounds it from above.
    let up = PrimaryCorrection {
        exposure_milli_stops: 1_500,
        ..PrimaryCorrection::default()
    };
    let down = PrimaryCorrection {
        exposure_milli_stops: -1_500,
        ..PrimaryCorrection::default()
    };
    let ramp_rgb = (0..CC8_RAMP_WIDTH * CC8_RAMP_HEIGHT)
        .map(|index| {
            #[allow(clippy::cast_precision_loss)]
            let position = (index % CC8_RAMP_WIDTH) as f32 / (CC8_RAMP_WIDTH - 1) as f32;
            // A PQ signal ramp: the PQ profile is the one that reaches
            // `CC8_PQ_PEAK_NITS / CC8_REFERENCE_WHITE_NITS` at signal 1.0, so
            // the ramp's top is the highest magnitude §3.1's headroom argument
            // is about.
            hdr_source_to_working_bt709_linear(ColorSourceProfile::PqRec2020, [position; 3])
                .expect("the PQ profile decodes")
                .map(|value| f16::from_f32(value).to_f32())
        })
        .collect::<Vec<_>>();
    let ramp_frame = working_frame(CC8_RAMP_WIDTH, CC8_RAMP_HEIGHT, &ramp_rgb);
    let ramp_working = compositor
        .render_working(
            (CC8_RAMP_WIDTH, CC8_RAMP_HEIGHT),
            &[CompositorLayer {
                frame: &ramp_frame,
                effects: &[cc8_correction_effect(3, up), cc8_correction_effect(4, down)],
                transition: TransitionRenderParams::default(),
            }],
        )
        .expect("production WGSL HDR over-range ramp readback")
        .pixels;

    // The plateau a clamping pipeline would produce, computed from the pair
    // itself rather than written down: every input above 1.0 would clamp to
    // 1.0 between the nodes and come back at `down(1.0)`.
    let clamp_plateau = down.apply_checked([1.0; 3]).expect("the clamp plateau")[0];
    let mut errors: Vec<f32> = Vec::with_capacity(ramp_rgb.len() * 3);
    let mut starved = 0.0_f32;
    let mut hdr_samples = 0_usize;
    for (index, pixel) in ramp_working.as_chunks::<4>().0.iter().enumerate() {
        for channel in 0..3 {
            errors.push((pixel[channel] - ramp_rgb[index][channel]).abs());
        }
        if ramp_rgb[index][0] <= CC8_BAND_EDGES[1] {
            continue;
        }
        hdr_samples += 1;
        // A clamp anywhere between the two nodes would cap every HDR input at
        // 1.0, so every sample above it would collapse onto the plateau.
        assert!(
            pixel[0] > clamp_plateau * 1.05,
            "HDR ramp sample {index} (source {}) collapsed to the clamped plateau \
             {clamp_plateau}: {}",
            ramp_rgb[index][0],
            pixel[0],
        );
        // The starved control §9.2's second rule asks for: what the same
        // measurement would report if the pipeline did clamp. It is compared
        // with the bound below, so the bound is demonstrably reachable rather
        // than a margin nothing approaches.
        starved = starved.max((clamp_plateau - ramp_rgb[index][0]).abs());
    }
    assert!(
        hdr_samples > 0,
        "the ramp reached no HDR magnitude, so the clamp claim was never tested",
    );

    let sample_count = errors.len();
    let ramp_top = ramp_rgb
        .iter()
        .flat_map(|pixel| pixel.iter())
        .fold(0.0_f32, |top, value| top.max(value.abs()));
    let floor = cc8_half_float_ulp(ramp_top);
    let measured = cc8_summarize(&mut errors);
    for (index, term) in ["max", "p99", "mean"].into_iter().enumerate() {
        cc8_assert_measured(
            "4",
            "HDR no-intermediate-clamp recovery",
            term,
            measured[index],
            CC8_FIXTURE4_MEASURED[index],
            floor,
        );
    }
    let max_bound = (cc8_next_power_of_two_bound(CC8_FIXTURE4_MEASURED[0])
        * CC8_MEASURED_BOUND_HEADROOM)
        .max(floor);
    assert!(
        starved > max_bound * 100.0,
        "the starved (clamping) control measured {starved}, which does not exceed the \
         no-clamp bound {max_bound} by a margin that makes the bound meaningful",
    );
    println!(
        "CC8_MEASURED fixture=4 lane={} highlight={highlight:?} recovered={recovered:?} \
         clamped_between_nodes={clamped_between_nodes:?} clamp_plateau={clamp_plateau} \
         ramp_samples={sample_count} ramp_top={ramp_top} hdr_samples={hdr_samples} \
         floor={floor:.6e} starved={starved:.6e} max_bound={max_bound:.6e}",
        gpu.lane.id(),
    );
}

// ---------------------------------------------------------------------------
// §9.1 fixture 10 — CPU/GPU parity at HDR magnitudes.
// ---------------------------------------------------------------------------

/// §9.1 fixture 10's recorded measurement, `[max, p99, mean]` per magnitude
/// band in [`CC8_BAND_NAMES`] order.
///
/// Taken on lavapipe (LLVM 20.1.2, 256 bits) / rustc stable, 2026-08-29, over a
/// 64x8 raster:
///
/// ```text
/// band             samples  max          p99          mean         floor
/// abs_le_1            1264  4.882812e-4  4.882812e-4  9.045420e-5  9.765625e-4
/// abs_1_to_2           136  9.765625e-4  9.765625e-4  1.723346e-4  1.953125e-3
/// abs_above_2_hdr      136  1.953125e-3  1.953125e-3  4.595588e-4  3.906250e-3
/// ```
///
/// Every max and P99 is exactly one `Rgba16Float` ULP at its band, which is the
/// shape CC1 §6.2 predicts ("a half-float ULP doubles at 1.0") and the reason
/// the third band's figures are larger than the first two by the ratio of the
/// magnitudes rather than by any property of the shader. The third band is the
/// one CC1 §6.2 excluded and §9.1 fixture 10 requires to be "extended to HDR
/// values and its own band recorded".
const CC8_FIXTURE10_MEASURED: [Cc8MeasuredBand; CC8_BAND_COUNT] = [
    [4.882_812e-4, 4.882_812e-4, 9.045_42e-5],
    [9.765_625e-4, 9.765_625e-4, 1.723_346e-4],
    [1.953_125e-3, 1.953_125e-3, 4.595_588e-4],
];

/// The correction §9.1 fixture 10 measures parity under.
///
/// Every control is non-neutral, so the comparison exercises the whole shader
/// rather than one term, and the case is asserted non-vacuous below.
fn cc8_parity_correction() -> PrimaryCorrection {
    PrimaryCorrection {
        exposure_milli_stops: -1_500,
        temperature_percent: 20,
        tint_percent: -15,
        contrast_percent: 20,
        contrast_pivot_basis_points: 4_500,
        blacks_percent: -20,
        shadows_percent: 15,
        highlights_percent: -25,
        whites_percent: 20,
        saturation_percent: 30,
    }
}

/// The parity raster: §3.3's decoded output at HDR magnitudes, including the
/// out-of-Rec.709 negatives the primaries stage produces.
///
/// Two halves, and both are needed:
///
/// * the top rows are the **decoded bars** from real media, which is where the
///   negatives and the wide-gamut chromaticities come from — a synthetic ramp
///   would carry neither; and
/// * the bottom rows are a **dense HLG signal ramp** taken through
///   `hdr_source_to_working_bt709_linear`, which is where the population of
///   CC1 §6.2's middle band comes from. Eight bars alone leave that band with a
///   handful of identical values whose parity error is zero by coincidence, and
///   §9.2's second rule refuses a gate over a term that measures zero because
///   the raster was too thin.
///
/// Both halves are repeated into wide low-frequency blocks for the reason CC1's
/// `representative_frame` gives: the production linear sampler is measured on
/// texel interiors instead of on interpolated seams.
fn cc8_hdr_parity_raster(frame: &WorkingFrame) -> (u32, u32, Vec<[f32; 3]>) {
    let bars: Vec<[f32; 3]> = (0..CC8_SOURCE_BARS.len())
        .map(|index| cc8_decoded_bar(frame, index))
        .collect();
    let width = CC8_PRECONDITION_SIZE.0;
    let height = 8_u32;
    #[allow(clippy::cast_possible_truncation)]
    let bar_width = width / bars.len() as u32;
    let rgb = (0..width * height)
        .map(|index| {
            let column = index % width;
            if index / width < height / 2 {
                return bars[(column / bar_width) as usize];
            }
            #[allow(clippy::cast_precision_loss)]
            let signal = column as f32 / (width - 1) as f32;
            hdr_source_to_working_bt709_linear(ColorSourceProfile::HlgRec2020, [signal; 3])
                .expect("the HLG profile decodes")
                .map(|value| f16::from_f32(value).to_f32())
        })
        .collect::<Vec<_>>();
    (width, height, rgb)
}

/// §9.1 fixture 10 — **parity**: CPU reference versus software GPU on HDR
/// magnitudes.
///
/// §9.1: "CPU reference versus software GPU on HDR magnitudes, under CC1 §6.2's
/// banded half-float rule, with the over-range band extended to HDR values and
/// its own band recorded."
///
/// CC1 §6.2 bands the linear comparison by the magnitude of the CPU reference
/// "because a half-float ULP doubles at 1.0", and **excludes** everything above
/// `2.0` as outside its stated domain. That exclusion is exactly what CC8 has to
/// remove: an HDR working value is routinely above 2.0, so a fixture that
/// inherited CC1's bands unchanged would drop every HDR sample out of the gate
/// and report parity it never measured. So the third band is live here, carries
/// its own measured constant, and its population is asserted non-empty.
///
/// The first two bands are additionally cross-checked against CC1 §6.2's own
/// constants. That is not an inherited budget in §9.2's sense — each band's
/// *gate* is its own measured figure under the step-3 rule — but a claim that
/// the shader has not become worse on the CC1 domain while gaining an HDR one,
/// which is the SDR-unchanged evidence within this fixture's reach.
///
/// **What runs where.** The HDR interpretation stages are decode-time CPU work
/// (see this section's header), so the two sides of this comparison differ only
/// in the *grading* node — the production WGSL against
/// `color_pipeline::apply_primary_corrections` — over an identical
/// `Rgba16Float` input. That is the same comparison CC1 §6.2 defines, at HDR
/// magnitudes, which is what §9.1 fixture 10 asks for.
#[test]
#[allow(clippy::too_many_lines)]
fn cc8_cpu_gpu_parity_on_hdr_magnitudes_bands_the_hdr_range() {
    let (_directory, _description, decoded) = cc8_hdr_bar_source("cc8-step4-parity");
    let (width, height, rgb) = cc8_hdr_parity_raster(&decoded);
    let frame = working_frame(width, height, &rgb);
    let correction = cc8_parity_correction();

    let gpu = fallback_gpu();
    let compositor = Compositor::new(gpu.context());
    let actual = compositor
        .render_working(
            (width, height),
            &[CompositorLayer {
                frame: &frame,
                effects: &[cc8_correction_effect(1, correction)],
                transition: TransitionRenderParams::default(),
            }],
        )
        .expect("production WGSL HDR parity readback")
        .pixels;

    let mut banded = Cc8BandedError::default();
    let mut moved_samples = 0_usize;
    let mut non_finite = 0_usize;
    for (index, pixel) in actual.as_chunks::<4>().0.iter().enumerate() {
        let reference = correction
            .apply_checked(rgb[index])
            .expect("the CC8 parity correction")
            .map(|value| f16::from_f32(value).to_f32());
        for channel in 0..3 {
            if !pixel[channel].is_finite() || !reference[channel].is_finite() {
                non_finite += 1;
                continue;
            }
            if (reference[channel] - rgb[index][channel]).abs() > 1.0e-3 {
                moved_samples += 1;
            }
            banded.push(
                reference[channel],
                (pixel[channel] - reference[channel]).abs(),
            );
        }
    }
    // CC1's finiteness claim, restated: a NaN excluded from a band would let a
    // broken shader report parity.
    assert_eq!(
        non_finite, 0,
        "the HDR parity comparison produced {non_finite} non-finite samples",
    );
    // Non-vacuity: a correction that moved nothing would report a flattering
    // zero, which is CC1's `MIN_CHANGED_LINEAR_BASIS_POINTS` rule.
    assert!(
        moved_samples * 2 > width as usize * height as usize * 3,
        "the CC8 parity correction moved only {moved_samples} samples; a proven no-op cannot \
         measure parity",
    );

    let summary = banded.summarize();
    // The top magnitude of each CC1 §6.2 band, for the storage-granularity
    // floor below. The first two are the band edges themselves; the HDR band's
    // top is the raster's own largest reference magnitude, which is where its
    // `Rgba16Float` ULP is widest.
    let hdr_band_top = rgb
        .iter()
        .flat_map(|pixel| pixel.iter())
        .fold(0.0_f32, |top, value| top.max(value.abs()));
    let band_top = [CC8_BAND_EDGES[0], CC8_BAND_EDGES[1], hdr_band_top];
    for (index, band) in summary.iter().enumerate() {
        assert!(
            band.samples > 0,
            "band {} is empty; §9.1 fixture 10 requires the HDR band to be populated, and CC1 \
             §6.2's two bands to stay populated beside it",
            CC8_BAND_NAMES[index],
        );
        // The floor is one `Rgba16Float` ULP at the band's top magnitude, and
        // it binds only where the recorded term is zero — see
        // `cc8_assert_measured`. It is what keeps a band whose parity happened
        // to be exact on this adapter from carrying a zero gate onto the other
        // CI operating system's GPU.
        let floor = cc8_half_float_ulp(band_top[index]);
        for (term_index, (term, value)) in
            [("max", band.max), ("p99", band.p99), ("mean", band.mean)]
                .into_iter()
                .enumerate()
        {
            cc8_assert_measured(
                "10",
                &format!("CPU vs GPU, HDR magnitudes [{}]", CC8_BAND_NAMES[index]),
                term,
                value,
                CC8_FIXTURE10_MEASURED[index][term_index],
                floor,
            );
        }
        println!(
            "CC8_MEASURED fixture=10 band={} samples={} max={:.6e} p99={:.6e} mean={:.6e}",
            CC8_BAND_NAMES[index], band.samples, band.max, band.p99, band.mean,
        );
    }

    // The CC1 §6.2 cross-check on the two bands CC1 defined.
    assert!(
        summary[0].max <= LINEAR_CPU_GPU_MAX
            && summary[0].p99 <= LINEAR_CPU_GPU_P99
            && summary[0].mean <= LINEAR_CPU_GPU_MEAN,
        "the CC1 §6.2 in-gamut gate no longer holds on the HDR raster: {:?}",
        summary[0],
    );
    assert!(
        summary[1].max <= LINEAR_CPU_GPU_MAX
            && summary[1].p99 <= LINEAR_OVER_RANGE_P99
            && summary[1].mean <= LINEAR_OVER_RANGE_MEAN,
        "the CC1 §6.2 over-range gate no longer holds on the HDR raster: {:?}",
        summary[1],
    );
    println!(
        "CC8_MEASURED fixture=10 lane={} raster={width}x{height} moved_samples={moved_samples} \
         cc1_in_gamut_gate=\"max {LINEAR_CPU_GPU_MAX} p99 {LINEAR_CPU_GPU_P99} mean \
         {LINEAR_CPU_GPU_MEAN}\" cc1_over_range_gate=\"max {LINEAR_CPU_GPU_MAX} p99 \
         {LINEAR_OVER_RANGE_P99} mean {LINEAR_OVER_RANGE_MEAN}\"",
        gpu.lane.id(),
    );
}

// ---------------------------------------------------------------------------
// The SDR-unchanged evidence within §10 step 4's reach.
// ---------------------------------------------------------------------------

/// Every CC1 source description still selects the *same* managed filter-graph
/// terms it selected before §10 step 4 made them lane-derived.
///
/// §9.1 fixture 6 — the byte-equality gate — is **§10 step 5's**, and this is
/// not it. What this asserts is the one thing step 4 can prove about the SDR
/// lane without an export: the two functions step 4 turned from literals into
/// derivations, `decode::managed_filter_matrix` (the `buffer` source's
/// `colorspace`) and `decode::managed_scale_color_matrix` (the `scale` filter's
/// `in_color_matrix` / `out_color_matrix`), return the strings the SDR lane has
/// always used for every description CC1 §2.1 admits, and return the BT.2020
/// spellings only for the matrix column that no CC1 profile can carry.
///
/// §12 names this exactly: "Every hard block in §0.4 is a literal that CC8
/// makes lane-derived, and a lane-derivation bug is invisible on the HDR lane
/// and catastrophic on the SDR one." So the derivation is asserted from the
/// classifier's side — a description is enumerated, classified, and the terms
/// are required to match the profile the classifier reports.
#[test]
fn cc8_sdr_managed_filter_graph_terms_are_unchanged_by_the_hdr_lane() {
    let mut checked = 0_usize;
    for primaries in [ColorPrimaries::Bt709, ColorPrimaries::Srgb] {
        for transfer in [
            ColorTransfer::Bt709,
            ColorTransfer::Bt1886,
            ColorTransfer::Srgb,
        ] {
            for matrix in [ColorMatrix::Bt709, ColorMatrix::Rgb, ColorMatrix::Identity] {
                for range in [ColorRange::Limited, ColorRange::Full] {
                    for bit_depth in [ColorBitDepth::Eight, ColorBitDepth::Ten] {
                        let description = ColorDescription {
                            primaries: primaries.clone(),
                            transfer: transfer.clone(),
                            matrix: matrix.clone(),
                            range: range.clone(),
                            white_point: ColorWhitePoint::D65,
                            bit_depth: bit_depth.clone(),
                            confidence_basis_points: 10_000,
                            provenance: ColorProvenance::UserOverride,
                        };
                        let Ok(profile) = classify_source(&description) else {
                            continue;
                        };
                        assert!(
                            !profile.is_hdr(),
                            "a CC1 tuple classified as an HDR profile: {description:?}",
                        );
                        checked += 1;
                        // The pre-step-4 answers, restated verbatim: `gbr` for
                        // an RGB/identity source and `bt709` for everything
                        // else on the `buffer` side, and the literal `bt709`
                        // the scale filter used to carry.
                        let expected_source_matrix =
                            if matches!(matrix, ColorMatrix::Rgb | ColorMatrix::Identity) {
                                "gbr"
                            } else {
                                "bt709"
                            };
                        assert_eq!(managed_filter_matrix(&description), expected_source_matrix);
                        assert_eq!(managed_scale_color_matrix(&description), "bt709");
                        assert_eq!(
                            managed_filter_range(&description),
                            if range == ColorRange::Limited {
                                "mpeg"
                            } else {
                                "jpeg"
                            },
                        );
                        // And CC1 §3.1's normalization denominator is the one
                        // it always was — the BT.2020 arm added to
                        // `rgba64_normalization_max` cannot be reached from
                        // here, because no CC1 profile carries that matrix.
                        assert_eq!(
                            rgba64_normalization_max(&description),
                            Ok(
                                if matches!(matrix, ColorMatrix::Bt709)
                                    && range == ColorRange::Limited
                                {
                                    65_280
                                } else if matches!(matrix, ColorMatrix::Rgb | ColorMatrix::Identity)
                                    && bit_depth == ColorBitDepth::Ten
                                {
                                    65_535
                                } else if bit_depth == ColorBitDepth::Ten {
                                    65_472
                                } else {
                                    65_280
                                }
                            ),
                            "{description:?}",
                        );
                    }
                }
            }
        }
    }
    // Non-vacuity: the sweep must actually have produced CC1 profiles.
    assert!(
        checked >= 12,
        "the CC1 sweep classified only {checked} tuples; the assertions above never ran on a \
         representative SDR set",
    );

    // The failing direction: the BT.2020 NCL column, which only a CC8 §2.1 HDR
    // profile can carry, takes the other spellings — so the derivation is real
    // rather than a constant `bt709` in disguise.
    let hdr = ColorDescription {
        primaries: ColorPrimaries::Bt2020,
        transfer: ColorTransfer::AribStdB67,
        matrix: ColorMatrix::Bt2020Ncl,
        range: ColorRange::Limited,
        white_point: ColorWhitePoint::D65,
        bit_depth: ColorBitDepth::Ten,
        confidence_basis_points: 10_000,
        provenance: ColorProvenance::UserOverride,
    };
    assert_eq!(classify_source(&hdr), Ok(ColorSourceProfile::HlgRec2020));
    assert_eq!(managed_filter_matrix(&hdr), "bt2020nc");
    assert_eq!(managed_scale_color_matrix(&hdr), "bt2020");
    assert_eq!(rgba64_normalization_max(&hdr), Ok(65_280));
    println!(
        "CC8_MEASURED fixture=sdr_unchanged cc1_tuples={checked} \
         hdr_source_matrix=bt2020nc hdr_scale_matrix=bt2020"
    );
}
