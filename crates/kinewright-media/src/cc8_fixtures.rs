//! CC8 fixtures: §10 step 1's encoder precondition, §10 step 3's §9.1
//! fixtures 1, 2 and 5, §10 step 4's §9.1 fixtures 3, 4 and 10, §10 step 5's
//! fixture 6, §10 step 6's fixtures 7 and 8, §10 step 7's fixture 12, and §10
//! step 8's fixture 9.
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

use std::{collections::BTreeMap, path::Path, sync::Arc};

use ffmpeg_next as ffmpeg;
use half::f16;
use kinewright_core::{
    Analysis, AssetId, CC8_BT709_TO_REC2020, CC8_BT2020_CB_DENOMINATOR, CC8_BT2020_CR_DENOMINATOR,
    CC8_BT2020_KB, CC8_BT2020_KG, CC8_BT2020_KR, CC8_HDR_DELIVERY_DEPTH_ALLOWED,
    CC8_HDR_DELIVERY_LANE, CC8_HDR_DELIVERY_MATRIX_ALLOWED, CC8_HDR_DELIVERY_PRIMARIES_ALLOWED,
    CC8_HDR_DELIVERY_RANGE_ALLOWED, CC8_HDR_DELIVERY_RECOVERY_ACTION,
    CC8_HDR_DELIVERY_TRANSFER_ALLOWED, CC8_HDR_DELIVERY_WHITE_POINT_ALLOWED,
    CC8_HDR_DELIVERY_X264_PARAMS, CC8_HLG_NOMINAL_PEAK_NITS,
    CC8_HLG_REFERENCE_WHITE_SIGNAL_PERCENT, CC8_HLG_SCENE_BREAKPOINT, CC8_HLG_SIGNAL_BREAKPOINT,
    CC8_PQ_C1, CC8_PQ_C2, CC8_PQ_C3, CC8_PQ_DELIVERY_RECOVERY_ACTION, CC8_PQ_M2, CC8_PQ_PEAK_NITS,
    CC8_PREVIEW_PEAK_NITS, CC8_PREVIEW_STAGE, CC8_REC2020_TO_BT709, CC8_REFERENCE_WHITE_NITS,
    CC8_REJECTED_HDR_ADJACENT, CC8_SOURCE_PROFILES, ColorBitDepth, ColorContext, ColorDescription,
    ColorMatrix, ColorPrimaries, ColorProvenance, ColorQcCheck, ColorQcRequest, ColorRange,
    ColorSourceError, ColorSourceProfile, ColorSourceProfileAssumption, ColorTransfer,
    ColorWhitePoint, DeliveryColorError, DeliveryColorMismatch, DeliveryEncodeDepth, DeliveryLane,
    DeliveryProfile, DeliveryVerificationRequest, Document, Effect, EffectId, ExportSettings,
    HDR_SOURCE_ON_SDR_DELIVERY, LinearRgbaImage, MediaError, MonitorPreview, MonitorProofMetadata,
    MonitorProofRenderKind, ParamValue, QaSeverity, Rational, TimeCode, WORKING_PROOF_ENCODING,
    WORKING_PROOF_STAGE, WorkingProof, WorkingProofMetadata, YCBCR_CHROMA_LEGAL_HIGH,
    YCBCR_CHROMA_OFFSET, YCBCR_CHROMA_SPAN, YCBCR_LUMA_LEGAL_HIGH, YCBCR_LUMA_OFFSET,
    YCBCR_LUMA_SPAN, bt709_limited_ycbcr, cc8_apply_matrix, cc8_hlg_decode_working_linear,
    cc8_hlg_encode_working_linear, cc8_hlg_inverse_oetf, cc8_hlg_oetf,
    cc8_pq_decode_working_linear, cc8_pq_encode_working_linear, cc8_pq_eotf_nits,
    cc8_pq_inverse_eotf, cc8_preview_peak_working_linear, cc8_preview_tone_map_rgb,
    classify_source, classify_source_with_assumption, delivery_color_mismatches,
    delivery_color_mismatches_for_lane, delivery_conformance, delivery_field_recovery_action,
    document_monitor_preview, encode_delivery_for_lane, measure_color_qc,
};

use crate::{
    Compositor, CompositorLayer,
    cc1_fixtures::{
        LINEAR_CPU_GPU_MAX, LINEAR_CPU_GPU_MEAN, LINEAR_CPU_GPU_P99, LINEAR_OVER_RANGE_MEAN,
        LINEAR_OVER_RANGE_P99, MONITOR_CPU_GPU_MAX, MONITOR_CPU_GPU_MEAN, MONITOR_CPU_GPU_P99,
        abs_code_diff_rgb, decode_managed_working_frame, fallback_gpu, working_frame,
    },
    cc6_fixtures::{
        CC6_DELIVERY_SOURCE_SIZE, cc6_delivery_document, cc6_delivery_settings, cc6_delivery_source,
    },
    color_pipeline::{
        DELIVERY_INTERMEDIATE_WHITE, PrimaryCorrection, PrimaryParameter,
        apply_primary_corrections, decode_hdr_source_working_linear,
        encode_delivery_for_description, encode_delivery_hlg_rec2020_rgba16,
        encode_delivery_rgba16, encode_monitor_for_preview, encode_monitor_rgba8_for_description,
        encode_monitor_rgba8_for_preview, hdr_source_to_working_bt709_linear,
        rgba64_normalization_max, rgba64_promoted_max, source_primaries_to_working_linear,
    },
    decode::{
        VideoDecoder, managed_filter_matrix, managed_filter_range, managed_scale_color_matrix,
        probe_path,
    },
    export::{DeliveryLaneTerms, FrameColorTerms},
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

// ---------------------------------------------------------------------------
// §9.1 fixture 6 — "SDR regression — the byte-equality gate" (§10 step 5).
// ---------------------------------------------------------------------------
//
// The contract's words, verbatim:
//
// > **SDR regression — the byte-equality gate.** Every CC1–CC7 fixture passes
// > with every pinned constant unmoved, and the SDR `x264-params` string,
// > scaler flags, and exported bytes for a fixed SDR project are **unchanged**.
// > This is the fixture the acceptance of CC8 rests on, and §10 step 5 lands it
// > before any export change.
//
// and §12's fourth risk bullet, which says what it is for:
//
// > **The SDR regression gate is the one that must not be weakened.** Every
// > hard block in §0.4 is a literal that CC8 makes lane-derived, and a
// > lane-derivation bug is invisible on the HDR lane and catastrophic on the
// > SDR one. *Mitigation:* §9.1 fixture 6 lands before any export change (§10
// > step 5) and asserts exported bytes, not just tags.
//
// **What "unchanged" is here, and why.** "Unchanged" has to mean *unchanged by
// CC8*, and the only honest anchor for that is this tree's own pre-CC8
// behaviour. It may **not** be a checked-in content hash of one build's H.264
// output: §9.2's first rule — "No gate may be an equality against one FFmpeg
// build's decode output", CC7 §0.3 PM-E12 — forbids exactly that, and CC6 §0.4
// already traced the two CI builds' swscale to different chroma. So the gate is
// built from three claims, each of which is a measurement on whatever build
// runs it:
//
// 1. **Terms.** Every delivery colour term the export handed `FFmpeg` equals
//    the frozen pre-CC8 SDR literal, character for character, on **both** SDR
//    lanes. The terms are not recomputed by this fixture: `export.rs`'s
//    [`DeliveryLaneTerms`] reads each one back out of the object that received
//    it — the opened encoder, the options dictionary, the configured filter
//    graph, and the two stamped frames — so what is compared is what the encode
//    used. This is the clause that catches a lane-derivation bug *by
//    construction*: §0.4's six export-side literals are the six values recorded
//    here, and step 6 cannot move one on an SDR lane without this going red.
// 2. **Bytes.** Two independent exports of the same fixed SDR project, at the
//    same lane, in the same run, are **byte-identical files** — and so are
//    exports whose delivery description is spelled differently but denotes the
//    same lane (`ColorBitDepth::Integer(8)` for `Eight`, `ApplicationDefault`
//    for `UserOverride`). The first is what makes "exported bytes" a claim at
//    all: an export that is not a function of (tree, document, settings, build)
//    has no bytes to be unchanged. The second is the failing direction that
//    catches a step-6 derivation keyed on a field the lane does not depend on.
//    Both are byte comparisons of real H.264 files, never of a pinned digest.
// 3. **Content.** The 16-bit RGBA delivery intermediate — the deterministic
//    frames handed to the filter graph, upstream of all six literals — is
//    byte-identical across two independent renders, and the written file passes
//    CC6's own `verify_delivery_output` at the lane's production budgets with
//    the probed tags asserted. A lane-derivation bug that survived clause 1
//    would still have to move pixels past CC6's decoded-difference budgets.
//
// **What this fixture cannot assert, and where that clause lives.** "Every
// CC1–CC7 fixture passes" is a statement about the suite, not about one test;
// it is discharged by `cargo test -p kinewright-media` and `-p kinewright-core`
// staying green, which is the same way CC6 §11.0 and CC7 §11.0 discharge it.
// What *is* assertable here is the other half of that sentence — "with every
// pinned constant unmoved" — so the pinned delivery constants CC1, CC6 and CC7
// measured are restated below and asserted, and a moved constant fails here
// rather than only in whichever downstream fixture happened to notice.
//
// **The §0.4 survey, item by item, against what this gate covers.** Item 3
// (`export.rs:216-217`), item 4 (`:529`), item 6 (`:425`) and item 8 (`:626`,
// `:638`) are the five production sites, and all five are recorded terms here.
// Item 5 (`DELIVERY_VIDEO_CODEC`) is unchanged under §0.2 Q2 and is asserted as
// `settings.video_codec`. Item 7 (`:1187`) is the odd one out and is recorded
// as a finding rather than covered: that `setparams=...:colorspace=bt709` is
// **not** on the delivery path at all — it is the `-vf` argument of
// `generate_timing_source`, a `#[cfg(test)]` helper inside `export.rs`'s own
// test module that writes the *source* clip for CC6's presented-frame fixture.
// It tags an input, not an output, and step 6 has nothing to derive there.

/// The fixed SDR project's raster. CC6 §11.1's delivery source size, reused
/// rather than re-invented: it is the raster CC6 measured its budgets on, so
/// the verification clause below runs against the population those budgets
/// describe.
const CC8_SDR_REGRESSION_SIZE: (u32, u32) = CC6_DELIVERY_SOURCE_SIZE;

/// Frames in the fixed SDR project.
///
/// Fewer than CC6's 60 because this gate exports the project **six** times
/// (three per lane), and more than one GOP's worth of B-frame reordering
/// because the byte-equality clause is exactly the claim that libx264's
/// lookahead and rate control are a function of the input and not of the run:
/// a single-picture export would make clause 2 vacuous.
const CC8_SDR_REGRESSION_FRAMES: u32 = 20;

/// The pre-CC8 `DELIVERY_X264_PARAMS` — §0.4 item 4, at `export.rs:529` in the
/// survey's numbering — frozen here.
///
/// §5.2 item 2: "The SDR lanes' string is **byte-identical to today's**, and a
/// fixture asserts that." This is that assertion's constant, and it is
/// deliberately a literal in the *fixture* rather than an import of the
/// production constant: step 6 turns that constant into a function of the lane,
/// and a gate that imported it would follow the refactor wherever it went.
const CC8_SDR_FROZEN_X264_PARAMS: &str = "colorprim=bt709:transfer=bt709:colormatrix=bt709";

/// The pre-CC8 encoder preset. Not a colour term, but it rides the same
/// dictionary and a step-6 edit that rebuilt the options would move it.
const CC8_SDR_FROZEN_PRESET: &str = "medium";

/// `DELIVERY_SCALER_FLAGS`, the "scaler flags" fixture 6 names. §5.1 keeps it
/// unchanged and does not re-measure it.
const CC8_SDR_FROZEN_SCALER_FLAGS: &str = "bicubic";

/// §0.4 item 3 (`export.rs:216-217`) — the encoder's colourspace and range, as
/// `FFmpeg` names them (`av_color_space_name` / `av_color_range_name`).
const CC8_SDR_FROZEN_ENCODER_COLORSPACE: &str = "bt709";
const CC8_SDR_FROZEN_ENCODER_COLOR_RANGE: &str = "tv";

/// One lane's frozen delivery terms, in the shape `export.rs` reports them.
struct Cc8FrozenLaneTerms {
    depth: DeliveryEncodeDepth,
    pixel_format: &'static str,
}

/// The frozen `buffer` source arguments of `delivery_filter_graph`.
fn cc8_frozen_buffer_args(resolution: (u32, u32)) -> String {
    format!(
        "video_size={}x{}:pix_fmt=rgba64le:time_base=1/1:pixel_aspect=1/1:colorspace=gbr:range=jpeg",
        resolution.0, resolution.1
    )
}

/// The frozen `scale` arguments, carrying both the scaler flags and
/// `out_color_matrix=bt709` — §0.4 item 6, at `export.rs:425`.
fn cc8_frozen_scale_args(resolution: (u32, u32)) -> String {
    format!(
        "w={}:h={}:flags={CC8_SDR_FROZEN_SCALER_FLAGS}:in_range=jpeg:out_range=mpeg:\
         out_color_matrix=bt709",
        resolution.0, resolution.1
    )
}

/// The frozen stamp of the RGBA64LE delivery intermediate — §0.4 item 8's first
/// `set_color_primaries` call, at `export.rs:626`.
fn cc8_frozen_intermediate_frame() -> FrameColorTerms {
    FrameColorTerms {
        pixel_format: "rgba64le".to_owned(),
        space: "gbr".to_owned(),
        range: "pc".to_owned(),
        primaries: "bt709".to_owned(),
        transfer: "bt709".to_owned(),
    }
}

/// The frozen stamp of the delivery `Y'CbCr` frame — §0.4 item 8's second
/// `set_color_primaries` call, at `export.rs:638`.
fn cc8_frozen_delivery_frame(pixel_format: &str) -> FrameColorTerms {
    FrameColorTerms {
        pixel_format: pixel_format.to_owned(),
        space: "bt709".to_owned(),
        range: "tv".to_owned(),
        primaries: "bt709".to_owned(),
        transfer: "bt709".to_owned(),
    }
}

/// Every frozen term of one SDR lane, assembled.
fn cc8_frozen_lane_terms(lane: &Cc8FrozenLaneTerms, resolution: (u32, u32)) -> DeliveryLaneTerms {
    DeliveryLaneTerms {
        encoder_pixel_format: lane.pixel_format.to_owned(),
        encoder_colorspace: CC8_SDR_FROZEN_ENCODER_COLORSPACE.to_owned(),
        encoder_color_range: CC8_SDR_FROZEN_ENCODER_COLOR_RANGE.to_owned(),
        encoder_options: BTreeMap::from([
            ("preset".to_owned(), CC8_SDR_FROZEN_PRESET.to_owned()),
            (
                "x264-params".to_owned(),
                CC8_SDR_FROZEN_X264_PARAMS.to_owned(),
            ),
        ]),
        buffer_args: cc8_frozen_buffer_args(resolution),
        scale_args: cc8_frozen_scale_args(resolution),
        format_args: format!("pix_fmts={}", lane.pixel_format),
        graph_pixel_format: lane.pixel_format.to_owned(),
        intermediate_frame: cc8_frozen_intermediate_frame(),
        delivery_frame: cc8_frozen_delivery_frame(lane.pixel_format),
    }
}

/// Assert one recorded set of lane terms field by field, so a red run names the
/// §0.4 site that moved rather than printing two large structs.
fn cc8_assert_frozen_lane_terms(
    observed: &DeliveryLaneTerms,
    expected: &DeliveryLaneTerms,
    label: &str,
) {
    for (site, observed, expected) in [
        (
            "§0.4 item 3 (export.rs:216) set_colorspace",
            observed.encoder_colorspace.as_str(),
            expected.encoder_colorspace.as_str(),
        ),
        (
            "§0.4 item 3 (export.rs:217) set_color_range",
            observed.encoder_color_range.as_str(),
            expected.encoder_color_range.as_str(),
        ),
        (
            "video_encoder.set_format, the lane pixel format",
            observed.encoder_pixel_format.as_str(),
            expected.encoder_pixel_format.as_str(),
        ),
        (
            "delivery_filter_graph buffer args",
            observed.buffer_args.as_str(),
            expected.buffer_args.as_str(),
        ),
        (
            "§0.4 item 6 (export.rs:425) scale args: DELIVERY_SCALER_FLAGS + out_color_matrix",
            observed.scale_args.as_str(),
            expected.scale_args.as_str(),
        ),
        (
            "delivery_filter_graph format args",
            observed.format_args.as_str(),
            expected.format_args.as_str(),
        ),
        (
            "delivery_filter_graph output pixel format",
            observed.graph_pixel_format.as_str(),
            expected.graph_pixel_format.as_str(),
        ),
    ] {
        assert_eq!(
            observed, expected,
            "{label}: the SDR lane term at {site} moved; §9.1 fixture 6 forbids it",
        );
    }
    assert_eq!(
        observed.encoder_options, expected.encoder_options,
        "{label}: the encoder options moved (§0.4 item 4, export.rs:529, \
         DELIVERY_X264_PARAMS)",
    );
    assert_eq!(
        observed
            .encoder_options
            .get("x264-params")
            .map(String::as_str),
        Some(CC8_SDR_FROZEN_X264_PARAMS),
        "{label}: §5.2 item 2 requires the SDR lanes' x264-params string to stay \
         byte-identical to today's",
    );
    assert_eq!(
        observed.intermediate_frame, expected.intermediate_frame,
        "{label}: the RGBA intermediate stamp moved (§0.4 item 8, export.rs:626)",
    );
    assert_eq!(
        observed.delivery_frame, expected.delivery_frame,
        "{label}: the delivery Y'CbCr stamp moved (§0.4 item 8, export.rs:638)",
    );
    // Belt and braces: the whole record, so a field added to `DeliveryLaneTerms`
    // by a later step is covered without this list being edited.
    assert_eq!(observed, expected, "{label}: the recorded lane terms moved");
}

/// The SHA-256 of the delivery intermediate at one frame, rendered through the
/// production renderer — the same call `export.rs:302` makes.
fn cc8_delivery_intermediate_hash(
    renderer: &mut crate::render::FrameRenderer,
    document: &Document,
    resolution: (u32, u32),
    frame: u64,
) -> (String, usize) {
    let delivery = renderer
        .render_delivery(
            document,
            TimeCode(i64::try_from(frame).expect("sampled frame index")),
            resolution,
            crate::render::RenderScale::FullResolution,
            crate::render::DecodeStrategy::Seek,
        )
        .unwrap_or_else(|error| panic!("delivery intermediate render at {frame} failed: {error}"));
    assert_eq!((delivery.width, delivery.height), resolution);
    assert_eq!(
        delivery.rgba64le.len(),
        (resolution.0 as usize) * (resolution.1 as usize) * 8,
        "the delivery intermediate must be RGBA64LE",
    );
    (
        crate::sha256::sha256_bytes(&delivery.rgba64le),
        delivery.rgba64le.len(),
    )
}

/// Fixture 6's "with every pinned constant unmoved" clause.
///
/// The delivery constants CC1, CC6 and CC7 measured, restated here and
/// asserted, so a moved constant fails in the SDR regression gate itself rather
/// than only in whichever downstream fixture happened to notice. The sentence's
/// other half — "every CC1–CC7 fixture passes" — is a statement about the suite
/// and is discharged by `cargo test -p kinewright-media` / `-p kinewright-core`
/// staying green, exactly as CC6 §11.0 and CC7 §11.0 discharge it.
fn cc8_assert_pinned_delivery_constants_unmoved() {
    assert_eq!(
        crate::color_pipeline::DELIVERY_INTERMEDIATE_WHITE,
        65_280,
        "CC6 §5.1's delivery intermediate white is a CC8 §5.1 'unchanged and not re-measured' term",
    );
    assert_eq!(DeliveryEncodeDepth::Eight.pixel_format(), "yuv420p");
    assert_eq!(DeliveryEncodeDepth::Ten.pixel_format(), "yuv420p10le");
    assert_eq!(kinewright_core::DELIVERY_VERIFICATION_FRAME_COUNT, 5);
    assert_eq!(kinewright_core::DELIVERY_LUMA_MAX_CODE_8BIT, 8);
    assert_eq!(
        kinewright_core::DELIVERY_LUMA_P99_CODE_8BIT_MILLIONTHS,
        3_000_000
    );
    assert_eq!(
        kinewright_core::DELIVERY_LUMA_MEAN_CODE_8BIT_MILLIONTHS,
        400_000
    );
    assert_eq!(
        kinewright_core::DELIVERY_RGB_MEAN_CODE_8BIT_MILLIONTHS,
        1_750_000
    );
    assert_eq!(
        kinewright_core::DELIVERY_PSNR_FLOOR_DB_HUNDREDTHS_8BIT,
        3_300
    );
    assert_eq!(kinewright_core::DELIVERY_LUMA_MAX_CODE_10BIT, 16);
    assert_eq!(
        kinewright_core::DELIVERY_LUMA_P99_CODE_10BIT_MILLIONTHS,
        4_000_000
    );
    assert_eq!(
        kinewright_core::DELIVERY_LUMA_MEAN_CODE_10BIT_MILLIONTHS,
        1_000_000
    );
    assert_eq!(
        kinewright_core::DELIVERY_RGB_MEAN_CODE_10BIT_MILLIONTHS,
        1_000_000
    );
    assert_eq!(
        kinewright_core::DELIVERY_PSNR_FLOOR_DB_HUNDREDTHS_10BIT,
        3_300
    );
    assert_eq!(crate::cc1_fixtures::MONITOR_CPU_GPU_MAX, 2);
    // Compared by bit pattern: the claim is that the pinned literal is the same
    // literal, which is exactly what `float_cmp` warns is *not* what `==` means
    // for a computed float. Nothing here is computed.
    assert_eq!(
        crate::cc1_fixtures::LINEAR_CPU_GPU_MAX.to_bits(),
        1.5e-3_f32.to_bits(),
        "CC1 §6.2's linear CPU/GPU maximum is a pinned constant",
    );
}

/// **§9.1 fixture 6.** The SDR regression gate, landed before any export change
/// (§10 step 5) so that §10 step 6 finds it already green.
///
/// One fixed SDR project, both SDR lanes, and three claims per lane — the
/// frozen terms, byte-identical files, and CC6's own decoded verification. See
/// the block comment above for what each claim is and why "unchanged" is scoped
/// the way it is.
#[test]
fn cc8_sdr_regression_byte_equality_gate() {
    // Rule 11.0.6: the panicking acquisition, never the skipping one.
    let gpu = fallback_gpu();
    initialize_ffmpeg().expect("FFmpeg initializes");
    let directory = TempDirectory::new("cc8-sdr-regression-byte-equality");
    cc8_assert_pinned_delivery_constants_unmoved();

    // --- the fixed SDR project --------------------------------------------
    let source = cc6_delivery_source(
        &directory,
        CC8_SDR_REGRESSION_SIZE,
        CC8_SDR_REGRESSION_FRAMES,
    );
    let document = Arc::new(cc6_delivery_document(
        &source,
        CC8_SDR_REGRESSION_SIZE,
        CC8_SDR_REGRESSION_FRAMES,
    ));
    // The project is SDR by classification, not by assertion-free assumption:
    // a source that classified as an HDR profile would make every claim below
    // a statement about the wrong lane.
    let source_profile = classify_source_with_assumption(
        &document.media_pool[0].color_description,
        Some(ColorSourceProfileAssumption::D65),
    )
    .expect("the fixed SDR project's source must classify");
    assert!(
        !source_profile.is_hdr(),
        "the SDR regression gate's project must be an SDR project: {source_profile:?}",
    );

    let lanes = [
        Cc8FrozenLaneTerms {
            depth: DeliveryEncodeDepth::Eight,
            pixel_format: "yuv420p",
        },
        Cc8FrozenLaneTerms {
            depth: DeliveryEncodeDepth::Ten,
            pixel_format: "yuv420p10le",
        },
    ];
    assert_eq!(lanes.len(), DeliveryEncodeDepth::ALL.len());

    for lane in &lanes {
        cc8_assert_sdr_lane_regression(&gpu, &directory, &document, lane);
    }
}

/// One SDR lane's three claims. Split out of the gate above so each claim keeps
/// its own name in a backtrace, and so the gate itself reads as the list of
/// claims the contract makes.
fn cc8_assert_sdr_lane_regression(
    gpu: &crate::cc1_fixtures::FixtureGpu,
    directory: &TempDirectory,
    document: &Arc<Document>,
    lane: &Cc8FrozenLaneTerms,
) {
    let label = lane.pixel_format;
    let settings = cc6_delivery_settings(document, lane.depth);
    assert_eq!(settings.video_codec, "libx264");
    assert_eq!(settings.resolution, CC8_SDR_REGRESSION_SIZE);
    // The delivery description this gate is about: SDR Rec.709, both lanes.
    let delivery = settings.delivery_color.clone();
    assert_eq!(delivery.primaries, ColorPrimaries::Bt709);
    assert_eq!(delivery.transfer, ColorTransfer::Bt709);
    assert_eq!(delivery.matrix, ColorMatrix::Bt709);
    assert_eq!(delivery.range, ColorRange::Limited);
    assert_eq!(delivery.white_point, ColorWhitePoint::D65);
    assert_eq!(delivery.bit_depth, lane.depth.color_bit_depth());

    let request = DeliveryVerificationRequest::new(lane.depth, delivery.clone());
    let sampled = request.sample_frames(u64::from(CC8_SDR_REGRESSION_FRAMES));
    let intermediate_bytes =
        cc8_assert_delivery_intermediate_is_reproducible(gpu, document, &sampled, label);

    // --- claim 1: the frozen lane terms, read back from the export -----
    let observed_path = directory.path(&format!("cc8-sdr-regression-{label}-a.mp4"));
    let (progress_tx, _progress_rx) = crossbeam_channel::unbounded();
    let terms = crate::export::export_document_capturing_delivery_terms(
        document.as_ref(),
        &observed_path,
        &settings,
        &progress_tx,
        gpu.context(),
    )
    .expect("the production export must write the SDR lane");
    let frozen = cc8_frozen_lane_terms(lane, CC8_SDR_REGRESSION_SIZE);
    cc8_assert_frozen_lane_terms(&terms, &frozen, label);

    // --- claim 2: exported bytes ---------------------------------------
    //
    // (a) The same project, the same lane, exported again through the
    //     *production* arity — so this also proves the observational seam
    //     used by claim 1 writes no different byte.
    let repeat_path = directory.path(&format!("cc8-sdr-regression-{label}-b.mp4"));
    crate::export::export_document(
        document.as_ref(),
        &repeat_path,
        &settings,
        &progress_tx,
        gpu.context(),
    )
    .expect("the production export must write the SDR lane again");
    let observed_bytes = std::fs::read(&observed_path).expect("the first export must read");
    let repeat_bytes = std::fs::read(&repeat_path).expect("the second export must read");
    assert!(
        observed_bytes.len() > 1_024,
        "{label}: a {} byte export is not a delivery",
        observed_bytes.len(),
    );
    assert_eq!(
        observed_bytes.len(),
        repeat_bytes.len(),
        "{label}: two exports of the same project differed in length",
    );
    assert!(
        observed_bytes == repeat_bytes,
        "{label}: two exports of the same fixed SDR project were not byte-identical; \
         'exported bytes are unchanged' has no meaning on a non-deterministic export path",
    );
    let observed_hash = crate::sha256::sha256_bytes(&observed_bytes);

    let spellings = cc8_assert_equivalent_spellings_export_the_same_bytes(
        gpu,
        directory,
        document,
        &settings,
        lane,
        &frozen,
        &observed_bytes,
    );

    // --- claim 3b: CC6's decoded verification of the written file ------
    let engine = crate::engine::FfmpegMediaEngine::new_with_gpu(gpu.context())
        .expect("the production media engine should start");
    let verification = engine
        .verify_delivery_output(
            Arc::clone(document),
            &observed_path,
            &settings,
            request.clone(),
        )
        .expect("the written SDR export must verify");
    assert!(
        verification.technical_pass,
        "{label}: the SDR export must still pass CC6's verification: {:?}",
        verification.exceptions,
    );
    assert_eq!(verification.decoded_pixel_format, lane.pixel_format);
    assert_eq!(verification.probed.primaries, ColorPrimaries::Bt709);
    assert_eq!(verification.probed.transfer, ColorTransfer::Bt709);
    assert_eq!(verification.probed.matrix, ColorMatrix::Bt709);
    assert_eq!(verification.probed.range, ColorRange::Limited);
    assert_eq!(verification.probed.bit_depth, lane.depth.color_bit_depth());

    println!(
        "CC8_MEASURED fixture=6 lane={label} raster={}x{} frames={CC8_SDR_REGRESSION_FRAMES} \
         x264_params=\"{CC8_SDR_FROZEN_X264_PARAMS}\" scaler_flags={CC8_SDR_FROZEN_SCALER_FLAGS} \
         scale_args=\"{}\" export_bytes={} export_sha256={observed_hash} \
         intermediate_frame_bytes={intermediate_bytes} two_exports_byte_identical=true \
         equivalent_spellings={spellings} gpu_lane={}",
        CC8_SDR_REGRESSION_SIZE.0,
        CC8_SDR_REGRESSION_SIZE.1,
        terms.scale_args,
        observed_bytes.len(),
        gpu.lane.id(),
    );
}

/// Fixture 6's claim 3a: the 16-bit RGBA delivery intermediate is a function of
/// the project, not of the run.
///
/// It sits upstream of all six of §0.4's export-side literals and is
/// deterministic CPU/GPU math, so pinning it is what lets a byte difference
/// between two exports be attributed to the lane terms rather than to the
/// content. Returns the intermediate's per-frame byte count, for the
/// `CC8_MEASURED` line.
fn cc8_assert_delivery_intermediate_is_reproducible(
    gpu: &crate::cc1_fixtures::FixtureGpu,
    document: &Arc<Document>,
    sampled: &[u64],
    label: &str,
) -> usize {
    assert_eq!(
        sampled.len(),
        usize::from(kinewright_core::DELIVERY_VERIFICATION_FRAME_COUNT),
        "the sample must be CC6 §6.2's, not a shortened one",
    );
    let mut passes = Vec::new();
    let mut intermediate_bytes = 0_usize;
    for pass in 0..2 {
        let mut renderer = crate::render::FrameRenderer::new(gpu.context());
        let mut hashes = Vec::new();
        for frame in sampled {
            let (hash, len) = cc8_delivery_intermediate_hash(
                &mut renderer,
                document,
                CC8_SDR_REGRESSION_SIZE,
                *frame,
            );
            intermediate_bytes = len;
            hashes.push(hash);
        }
        assert_eq!(hashes.len(), sampled.len(), "pass {pass}");
        passes.push(hashes);
    }
    assert_eq!(
        passes[0], passes[1],
        "{label}: the 16-bit RGBA delivery intermediate must be byte-identical across two \
         independent renders of the same project",
    );
    // Non-vacuity: the sampled frames must not all be the same raster, or the
    // equality above would hold for a renderer that ignored `at`.
    assert!(
        passes[0]
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            > 1,
        "{label}: every sampled delivery intermediate hashed the same; the sample does not \
         distinguish frames: {:?}",
        passes[0],
    );
    intermediate_bytes
}

/// Fixture 6's claim 2(b): the failing direction for a §10 step 6 lane
/// derivation keyed on a field the SDR lane does not depend on.
///
/// Each spelling below denotes the *same* SDR delivery description and is
/// already accepted by `delivery_color_mismatches` — `ColorBitDepth::Integer(8)`
/// is CC1 §2.1's canonical equal of `Eight`, and both project provenances are
/// accepted by CC6 §4.2. A derivation that read one of them would change the
/// written file without changing any recorded colour term, which is precisely
/// the bug the terms clause alone cannot see. Returns the number of spellings
/// exercised.
fn cc8_assert_equivalent_spellings_export_the_same_bytes(
    gpu: &crate::cc1_fixtures::FixtureGpu,
    directory: &TempDirectory,
    document: &Arc<Document>,
    settings: &ExportSettings,
    lane: &Cc8FrozenLaneTerms,
    frozen: &DeliveryLaneTerms,
    observed_bytes: &[u8],
) -> usize {
    let label = lane.pixel_format;
    let delivery = settings.delivery_color.clone();
    let equivalent = [
        (
            "integer bit depth",
            ColorDescription {
                bit_depth: ColorBitDepth::Integer(u16::from(lane.depth.bits())),
                ..delivery.clone()
            },
        ),
        (
            "application-default provenance",
            ColorDescription {
                provenance: ColorProvenance::ApplicationDefault,
                ..delivery.clone()
            },
        ),
        (
            "user-override provenance",
            ColorDescription {
                provenance: ColorProvenance::UserOverride,
                ..delivery
            },
        ),
    ];
    let count = equivalent.len();
    let (progress_tx, _progress_rx) = crossbeam_channel::unbounded();
    for (spelling, description) in equivalent {
        let mut spelled = settings.clone();
        spelled.delivery_color = description;
        let spelled_path = directory.path(&format!(
            "cc8-sdr-regression-{label}-{}.mp4",
            spelling.replace(' ', "-"),
        ));
        let spelled_terms = crate::export::export_document_capturing_delivery_terms(
            document.as_ref(),
            &spelled_path,
            &spelled,
            &progress_tx,
            gpu.context(),
        )
        .expect("an equivalent SDR delivery spelling must still export");
        cc8_assert_frozen_lane_terms(&spelled_terms, frozen, &format!("{label}/{spelling}"));
        let spelled_bytes = std::fs::read(&spelled_path).expect("the export must read");
        assert!(
            spelled_bytes == observed_bytes,
            "{label}: spelling the same SDR delivery description as '{spelling}' changed the \
             exported bytes; the lane derivation is reading a field the lane does not depend on",
        );
    }
    count
}

// ===========================================================================
// CC8 §10 step 6 — §9.1 fixtures 7 and 8.
// ===========================================================================
//
// Step 6 is "Delivery lane, tags, typed rejection: fixtures 7 and 8." Four
// things land with it, and each has its gate below:
//
//  * **§5.1's lane**, pinned in the authority module as
//    `kinewright_core::CC8_HDR_DELIVERY_LANE` and selected from the delivery
//    `ColorDescription` alone (§5.2 clause 1) by `DeliveryLane`;
//  * **the six §0.4 export-side literals made lane-derived** — the encoder's
//    colourspace and range (§0.4 item 3), `DELIVERY_X264_PARAMS` (item 4), the
//    scaler's `out_color_matrix` (item 6), and the two frame stamps (item 8) —
//    with §9.1 fixture 6 standing guard over the SDR answers;
//  * **the delivery-side colour transform**,
//    `color_pipeline::encode_delivery_hlg_rec2020_rgba16`, which is §3.3's
//    delivery line: primaries conversion to Rec.2020, then the HLG OOTF+OETF
//    pair in the encode direction, then the single clamp and quantization on
//    the unchanged `DELIVERY_INTERMEDIATE_WHITE` scale (§5.1); and
//  * **§5.3's typed rejection**, lane-derived rather than global.
//
// **What step 6 does not take.** §6's report-level QC — the lane-aware gamut
// report, the ungated MaxCLL/MaxFALL rows, the withheld skin reason — is §10
// step 7's and is not started here. The one piece of §6 that *is* here is item
// 1's BT.2020 NCL `Y'CbCr` reference, `kinewright_core::bt2020_ncl_limited_ycbcr`,
// because §9.1 fixture 8 gates on "decoded native-plane BT.2020 legality" and
// on difference budgets whose reference luma goes through the lane's matrix;
// landing fixture 8 against the BT.709 reference would be gating the HDR lane
// on "a wrong number, not an approximate one" (§6 item 1's own words). §4's
// tone-mapped preview is §10 step 8's (below), §7 items 1/2/4's
// `managed_hdr_v1` state and migration are §10 step 9's, and §9.2's measured
// gate table plus `cc8_manifest.json` are §10 step 10's.

// ---------------------------------------------------------------------------
// §9.1 fixture 7 — "Delivery rejection. One failing direction per §5.3 bullet,
// each named."
// ---------------------------------------------------------------------------

/// §5.1's lane as a `ColorDescription`, built from the authority module's own
/// wire spellings rather than from repeated enum literals.
///
/// Every field is checked against [`CC8_HDR_DELIVERY_LANE`]'s cell, so a lane
/// table edited in `cc8_hdr.rs` fails here rather than silently leaving these
/// fixtures testing the old lane.
fn cc8_hdr_delivery_description() -> ColorDescription {
    let description = ColorDescription {
        primaries: ColorPrimaries::Bt2020,
        transfer: ColorTransfer::AribStdB67,
        matrix: ColorMatrix::Bt2020Ncl,
        range: ColorRange::Limited,
        white_point: ColorWhitePoint::D65,
        bit_depth: ColorBitDepth::Ten,
        confidence_basis_points: 10_000,
        provenance: ColorProvenance::UserOverride,
    };
    let lane = CC8_HDR_DELIVERY_LANE;
    assert_eq!(description.primaries.wire(), lane.primaries);
    assert_eq!(description.transfer.wire(), lane.transfer);
    assert_eq!(description.matrix.wire(), lane.matrix);
    assert_eq!(description.range.wire(), lane.range);
    assert_eq!(description.white_point.wire(), lane.white_point);
    assert_eq!(
        description.bit_depth,
        DeliveryEncodeDepth::Ten.color_bit_depth(),
    );
    assert_eq!(DeliveryEncodeDepth::Ten.bits(), lane.bit_depth_bits);
    // The passing direction, asserted once here so every failing direction
    // below is a mutation of something that conforms.
    assert!(
        delivery_color_mismatches(&description).is_empty(),
        "§5.1's own lane must be accepted: {:?}",
        delivery_color_mismatches(&description),
    );
    assert_eq!(
        DeliveryLane::for_description(&description),
        DeliveryLane::HdrHlgRec2020,
    );
    description
}

/// The SDR Rec.709 delivery description at one depth, for the failing
/// directions that put an SDR description on the HDR lane.
fn cc8_sdr_delivery_description(depth: DeliveryEncodeDepth) -> ColorDescription {
    let description = ColorDescription {
        bit_depth: depth.color_bit_depth(),
        ..ColorContext::sdr_rec709().delivery
    };
    assert!(delivery_color_mismatches(&description).is_empty());
    assert_eq!(
        DeliveryLane::for_description(&description),
        DeliveryLane::SdrRec709,
    );
    description
}

/// Assert one §5.3 refusal, in the four structured facts CC6 §4.2 fixed, and
/// return them for the evidence line.
///
/// Every refusal is asserted **twice**: once against the core mismatch list,
/// which is where §5.3 lives, and once against the production export gate,
/// which is what actually stops a file being written. A rejection that existed
/// only in core would let the exporter tag a file CC8 refuses.
fn cc8_assert_delivery_refused(
    label: &str,
    color: &ColorDescription,
    lane: DeliveryLane,
    field: &str,
    observed: &str,
    allowed: &str,
    recovery: &str,
) {
    let mismatch = delivery_color_mismatches_for_lane(color, lane)
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("{label}: §5.3 must refuse this description on {lane:?}"));
    assert_eq!(mismatch.field, field, "{label}: field of {mismatch:?}");
    assert_eq!(mismatch.observed, observed, "{label}: observed");
    assert_eq!(mismatch.allowed, allowed, "{label}: allowed");
    assert_eq!(
        delivery_field_recovery_action(&mismatch),
        recovery,
        "{label}: recovery action",
    );
    // The same refusal at the production export gate, where the lane is
    // derived from the description (§5.2 clause 1) rather than passed in.
    if DeliveryLane::for_description(color) == lane {
        let error = crate::export::validate_delivery_description(color)
            .expect_err("the production export gate must refuse it too");
        let typed = match &error {
            MediaError::DeliveryColor(typed) => typed,
            other => panic!("{label}: delivery rejections must be typed: {other}"),
        };
        assert_eq!(typed.code(), "unsupported_delivery_color", "{label}");
        assert_eq!(typed.field(), field, "{label}");
        assert_eq!(typed.observed(), observed, "{label}");
        assert_eq!(typed.allowed_values(), allowed, "{label}");
        assert_eq!(typed.recovery_action(), recovery, "{label}");
        assert_eq!(error.recovery_code(), Some("unsupported_delivery_color"));
    }
}

/// **§9.1 fixture 7, §5.3 bullet 1.** An HDR description on an SDR lane, and an
/// SDR description on the HDR lane.
///
/// Production never puts a description on the wrong lane —
/// `DeliveryLane::for_description` derives it — so both directions are asked of
/// `delivery_color_mismatches_for_lane`, which is the surface that exists for
/// exactly this bullet. The *derived* answers are asserted alongside, so the
/// fixture also states that production keeps deriving the lane it should.
#[test]
fn cc8_delivery_rejects_an_hdr_description_on_an_sdr_lane_and_an_sdr_one_on_the_hdr_lane() {
    let hdr = cc8_hdr_delivery_description();
    // (a) §5.1's HDR lane description, checked against the SDR lane: refused on
    // `primaries`, in CC6's own phrase, because that is the first field of the
    // fixed check order that an SDR lane disagrees about.
    cc8_assert_delivery_refused(
        "hdr description on the SDR lane",
        &hdr,
        DeliveryLane::SdrRec709,
        "primaries",
        "Bt2020",
        "bt709",
        "Reset the delivery colour target explicitly, or choose a supported delivery depth.",
    );
    // (b) The SDR Rec.709 description, at both managed depths, checked against
    // §5.1's HDR lane: refused on `primaries` with the HDR phrase and the HDR
    // recovery, which names §0.2 Q6's permanent refusal of HDR-from-SDR.
    for depth in DeliveryEncodeDepth::ALL {
        let sdr = cc8_sdr_delivery_description(depth);
        cc8_assert_delivery_refused(
            &format!("sdr description on the HDR lane at {}", depth.as_str()),
            &sdr,
            DeliveryLane::HdrHlgRec2020,
            "primaries",
            "Bt709",
            CC8_HDR_DELIVERY_PRIMARIES_ALLOWED,
            CC8_HDR_DELIVERY_RECOVERY_ACTION,
        );
    }
    // Non-vacuity: each description conforms on the lane it selects, so the two
    // refusals above are about the lane and not about the description.
    assert!(delivery_color_mismatches(&hdr).is_empty());
    assert!(
        delivery_color_mismatches(&cc8_sdr_delivery_description(DeliveryEncodeDepth::Eight))
            .is_empty()
    );
    println!(
        "CC8_MEASURED fixture=7 bullet=lane_crossing hdr_lane={} sdr_lane={}",
        DeliveryLane::HdrHlgRec2020.as_str(),
        DeliveryLane::SdrRec709.as_str(),
    );
}

/// **§9.1 fixture 7, §5.3 bullet 2.** HLG or PQ at 8-bit depth.
///
/// §2.1's 10-bit floor and §5.1's `Bit depth | Ten` row are the same number on
/// the source and delivery sides, and §2.1 is explicit that this is a typed
/// rejection naming the depth rather than a warning.
#[test]
fn cc8_delivery_rejects_hlg_and_pq_at_eight_bit_depth() {
    let mut refused = 0_usize;
    for transfer in [ColorTransfer::AribStdB67, ColorTransfer::Smpte2084] {
        for (spelling, bit_depth) in [
            ("named", ColorBitDepth::Eight),
            ("integer", ColorBitDepth::Integer(8)),
        ] {
            let color = ColorDescription {
                transfer: transfer.clone(),
                bit_depth,
                ..cc8_hdr_delivery_description()
            };
            // PQ is refused on its own field first (bullet 6), so the depth
            // bullet is asserted on the HLG rows and the PQ rows are asserted
            // to be refused *at all* — the two bullets must not mask each
            // other into a single check.
            let mismatches =
                delivery_color_mismatches_for_lane(&color, DeliveryLane::HdrHlgRec2020);
            assert!(
                !mismatches.is_empty(),
                "8-bit {transfer:?} ({spelling}) must be refused",
            );
            let depth_mismatch = mismatches
                .iter()
                .find(|mismatch| mismatch.field == "bit_depth")
                .unwrap_or_else(|| {
                    panic!("8-bit {transfer:?} ({spelling}) must be refused on bit_depth")
                });
            assert_eq!(depth_mismatch.allowed, CC8_HDR_DELIVERY_DEPTH_ALLOWED);
            assert_eq!(depth_mismatch.observed, format!("{:?}", color.bit_depth));
            assert_eq!(
                delivery_field_recovery_action(depth_mismatch),
                CC8_HDR_DELIVERY_RECOVERY_ACTION,
            );
            refused += 1;
        }
    }
    // The one 8-bit HLG description that reaches the export gate first, so the
    // depth refusal is proven on the production surface too.
    let eight_bit_hlg = ColorDescription {
        bit_depth: ColorBitDepth::Eight,
        ..cc8_hdr_delivery_description()
    };
    cc8_assert_delivery_refused(
        "8-bit HLG on the HDR lane",
        &eight_bit_hlg,
        DeliveryLane::HdrHlgRec2020,
        "bit_depth",
        "Eight",
        CC8_HDR_DELIVERY_DEPTH_ALLOWED,
        CC8_HDR_DELIVERY_RECOVERY_ACTION,
    );
    // Non-vacuity: 10 bits is the depth that passes, in both spellings.
    for bit_depth in [ColorBitDepth::Ten, ColorBitDepth::Integer(10)] {
        let ten = ColorDescription {
            bit_depth,
            ..cc8_hdr_delivery_description()
        };
        assert!(delivery_color_mismatches(&ten).is_empty(), "{ten:?}");
    }
    assert_eq!(refused, 4);
    println!(
        "CC8_MEASURED fixture=7 bullet=eight_bit_hdr refused={refused} \
         floor_bits={} lane_bits={}",
        kinewright_core::CC8_HDR_MIN_INTEGER_DEPTH_BITS,
        CC8_HDR_DELIVERY_LANE.bit_depth_bits,
    );
}

/// **§9.1 fixture 7, §5.3 bullet 3.** Mismatched primaries/transfer pairs:
/// Rec.2020 primaries with a BT.709 transfer, and BT.709 primaries with an HLG
/// or PQ transfer.
///
/// §5.3 says these are "rejected on the *combination*", and the combination is
/// what the lane selector reads: neither pair is one of §2.1's two rows, so
/// neither selects §5.1's HDR lane, and each is refused against the SDR lane on
/// the first field of the fixed check order it disagrees about. The bullet is
/// about the pair reaching **no** lane, not about a seventh field being added
/// to §5.3's fixed order.
#[test]
fn cc8_delivery_rejects_mismatched_primaries_and_transfer_pairs() {
    let cases: [(&str, ColorPrimaries, ColorTransfer, &str, &str); 3] = [
        (
            "rec2020 primaries with a bt709 transfer",
            ColorPrimaries::Bt2020,
            ColorTransfer::Bt709,
            "primaries",
            "Bt2020",
        ),
        (
            "bt709 primaries with an HLG transfer",
            ColorPrimaries::Bt709,
            ColorTransfer::AribStdB67,
            "transfer",
            "AribStdB67",
        ),
        (
            "bt709 primaries with a PQ transfer",
            ColorPrimaries::Bt709,
            ColorTransfer::Smpte2084,
            "transfer",
            "Smpte2084",
        ),
    ];
    let case_count = cases.len();
    for (label, primaries, transfer, field, observed) in cases {
        let color = ColorDescription {
            primaries: primaries.clone(),
            transfer: transfer.clone(),
            ..cc8_sdr_delivery_description(DeliveryEncodeDepth::Ten)
        };
        // The combination reaches no HDR row, so the lane it selects is the
        // SDR one — asserted, because that is the whole mechanism of the
        // bullet and an accidental HDR classification would change the
        // refusal's field.
        assert!(
            !kinewright_core::color_description_is_cc8_hdr(&color),
            "{label}: a mismatched pair must not select the HDR lane",
        );
        assert_eq!(
            DeliveryLane::for_description(&color),
            DeliveryLane::SdrRec709,
        );
        cc8_assert_delivery_refused(
            label,
            &color,
            DeliveryLane::SdrRec709,
            field,
            observed,
            "bt709",
            "Reset the delivery colour target explicitly, or choose a supported delivery depth.",
        );
    }
    println!("CC8_MEASURED fixture=7 bullet=mismatched_pairs cases={case_count}");
}

/// **§9.1 fixture 7, §5.3 bullet 4.** `bt2020_cl`, `ictcp`, and every matrix
/// outside the lane table.
///
/// Enumerated from `ColorMatrix`'s own set rather than from a list written
/// here, so a matrix added to the schema is refused or accepted by this fixture
/// on the day it appears instead of being silently unexercised.
#[test]
fn cc8_delivery_rejects_every_matrix_outside_the_hdr_lane_table() {
    let every_matrix = [
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
    ];
    let matrix_count = every_matrix.len();
    let has_bt2020_cl = every_matrix.iter().any(|m| m.wire() == "bt2020_cl");
    let has_ictcp = every_matrix.iter().any(|m| m.wire() == "ictcp");
    let mut accepted = Vec::new();
    let mut refused = 0_usize;
    for matrix in &every_matrix {
        let color = ColorDescription {
            matrix: matrix.clone(),
            ..cc8_hdr_delivery_description()
        };
        if matrix.wire() == CC8_HDR_DELIVERY_LANE.matrix {
            assert!(delivery_color_mismatches(&color).is_empty(), "{color:?}");
            accepted.push(matrix.wire());
            continue;
        }
        cc8_assert_delivery_refused(
            &format!("{} on the HDR lane", matrix.wire()),
            &color,
            DeliveryLane::HdrHlgRec2020,
            "matrix",
            &format!("{matrix:?}"),
            CC8_HDR_DELIVERY_MATRIX_ALLOWED,
            CC8_HDR_DELIVERY_RECOVERY_ACTION,
        );
        refused += 1;
    }
    assert_eq!(
        accepted,
        vec!["bt2020_ncl"],
        "§5.1's `Matrix | bt2020_ncl` row is the whole accepted set on this lane",
    );
    assert_eq!(refused, matrix_count - 1);
    // §1's two named non-deliverables must be among the refusals, by name.
    assert!(has_bt2020_cl);
    assert!(has_ictcp);
    println!("CC8_MEASURED fixture=7 bullet=matrix refused={refused} accepted={accepted:?}");
}

/// **§9.1 fixture 7, §5.3 bullet 5.** Full range on the HDR lane.
///
/// §5.1: "**Full-range HDR delivery is rejected** with a typed reason, as
/// full-range SDR already is." The SDR half is asserted here too, so the
/// sentence's "as ... already is" is a measurement rather than a claim.
#[test]
fn cc8_delivery_rejects_full_range_on_the_hdr_lane() {
    for range in [ColorRange::Full, ColorRange::Unknown] {
        let hdr = ColorDescription {
            range: range.clone(),
            ..cc8_hdr_delivery_description()
        };
        cc8_assert_delivery_refused(
            &format!("{} range on the HDR lane", range.wire()),
            &hdr,
            DeliveryLane::HdrHlgRec2020,
            "range",
            &format!("{range:?}"),
            CC8_HDR_DELIVERY_RANGE_ALLOWED,
            CC8_HDR_DELIVERY_RECOVERY_ACTION,
        );
        let sdr = ColorDescription {
            range: range.clone(),
            ..cc8_sdr_delivery_description(DeliveryEncodeDepth::Ten)
        };
        cc8_assert_delivery_refused(
            &format!("{} range on the SDR lane", range.wire()),
            &sdr,
            DeliveryLane::SdrRec709,
            "range",
            &format!("{range:?}"),
            "limited",
            "Reset the delivery colour target explicitly, or choose a supported delivery depth.",
        );
    }
    // The same for the white point, which §5.1's table also fixes.
    let no_white_point = ColorDescription {
        white_point: ColorWhitePoint::Unknown,
        ..cc8_hdr_delivery_description()
    };
    cc8_assert_delivery_refused(
        "unknown white point on the HDR lane",
        &no_white_point,
        DeliveryLane::HdrHlgRec2020,
        "white_point",
        "Unknown",
        CC8_HDR_DELIVERY_WHITE_POINT_ALLOWED,
        CC8_HDR_DELIVERY_RECOVERY_ACTION,
    );
    println!(
        "CC8_MEASURED fixture=7 bullet=range lane_range={} refused=full,unknown",
        CC8_HDR_DELIVERY_LANE.range,
    );
}

/// **§9.1 fixture 7, §5.3 bullet 6.** PQ on the HLG lane, with a recovery
/// action naming the deferral rather than implying a conversion exists.
///
/// This is the one bullet §5.3 states the recovery text for, so the text is
/// asserted rather than merely asserted to be non-empty: it must name §11's
/// deferral and must say that no conversion exists, because a recovery that
/// said "convert to HLG" would advertise a stage CC8 does not have and §0.2 Q6
/// refuses to add.
#[test]
fn cc8_delivery_rejects_pq_on_the_hlg_lane_and_names_the_deferral() {
    let pq = ColorDescription {
        transfer: ColorTransfer::Smpte2084,
        ..cc8_hdr_delivery_description()
    };
    // A PQ delivery description is still an HDR *pair*, so it selects §5.1's
    // lane and is refused there — not silently reclassified as SDR, which
    // would report the wrong field.
    assert!(kinewright_core::color_description_is_cc8_hdr(&pq));
    assert_eq!(
        DeliveryLane::for_description(&pq),
        DeliveryLane::HdrHlgRec2020,
    );
    cc8_assert_delivery_refused(
        "PQ on the HLG lane",
        &pq,
        DeliveryLane::HdrHlgRec2020,
        "transfer",
        "Smpte2084",
        CC8_HDR_DELIVERY_TRANSFER_ALLOWED,
        CC8_PQ_DELIVERY_RECOVERY_ACTION,
    );
    let recovery = CC8_PQ_DELIVERY_RECOVERY_ACTION;
    assert!(recovery.contains("§11"), "the deferral must be named");
    assert!(
        recovery.contains("No PQ-to-HLG conversion exists"),
        "the recovery must not imply a conversion exists: {recovery}",
    );
    assert!(
        !recovery.to_lowercase().contains("tone map"),
        "§0.2 Q6 refuses tone-mapped delivery; the recovery must not offer one: {recovery}",
    );
    // The delivery-side colour transform refuses PQ as well, so a PQ
    // description cannot reach the encoder even if a caller bypassed the gate:
    // the compositor's own encode is the second refusal.
    let error = crate::color_pipeline::encode_delivery_for_description([1.0, 1.0, 1.0, 1.0], &pq)
        .expect_err("the delivery encode must refuse a PQ description");
    assert!(
        format!("{error}").contains("Smpte2084"),
        "the delivery encode's refusal must name the transfer: {error}",
    );
    println!(
        "CC8_MEASURED fixture=7 bullet=pq_on_hlg_lane recovery_bytes={}",
        recovery.len()
    );
}

/// **§9.1 fixture 7, the standing block.** §10 step 3's
/// `hdr_source_on_sdr_delivery` still blocks, and lifts only when the delivery
/// description is §5.1's lane.
///
/// §7 item 2 is a *permanent* rule, not scaffolding: §0.2 Q6 refuses
/// tone-mapped SDR delivery from an HDR timeline for good. What §10 step 6
/// changes is only which delivery descriptions satisfy
/// `color_description_is_cc8_hdr` **in practice**, so both directions are
/// asserted here — the block is still raised against an SDR delivery target,
/// and it is not raised once the project's delivery description is the HDR
/// lane. Without the second half, step 6 could have left the HDR lane
/// unreachable and every fixture would still have passed.
#[test]
fn cc8_delivery_hdr_source_block_stands_and_lifts_only_on_the_hdr_lane() {
    let hdr_source = ColorDescription {
        white_point: ColorWhitePoint::D65,
        ..cc8_hdr_delivery_description()
    };
    assert_eq!(
        classify_source(&hdr_source),
        Ok(ColorSourceProfile::HlgRec2020),
    );
    let mut asset = kinewright_core::MediaAsset {
        id: AssetId(1),
        path: std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"),
        name: "cc8-hdr-source".to_owned(),
        duration: TimeCode(6),
        fps: Rational::new(25, 1).expect("25 fps"),
        kind: kinewright_core::MediaKind::Video,
        resolution: Some(CC8_PRECONDITION_SIZE),
        source_fingerprint: kinewright_core::MediaSourceFingerprint::default(),
        color_description: hdr_source.clone(),
    };
    asset.color_description = hdr_source;
    let mut document = single_clip_document(asset);
    document.color_context = ColorContext::sdr_rec709();

    let blocked = delivery_conformance(
        &document,
        DeliveryProfile::SourceMaster,
        DeliveryEncodeDepth::Ten,
        50,
        50,
    )
    .expect("an HDR source is reported, not returned as an error");
    let issue = blocked
        .issues
        .iter()
        .find(|issue| issue.code == HDR_SOURCE_ON_SDR_DELIVERY)
        .expect("§7 item 2's block must still be raised on an SDR delivery target");
    assert_eq!(issue.severity, QaSeverity::Error);
    assert!(!blocked.export_ready());

    // The same project with §5.1's delivery description: the block lifts, and
    // it lifts *because* the delivery is HDR rather than because the source
    // stopped being HDR.
    document.color_context.delivery = cc8_hdr_delivery_description();
    let allowed = delivery_conformance(
        &document,
        DeliveryProfile::SourceMaster,
        DeliveryEncodeDepth::Ten,
        50,
        50,
    )
    .expect("the HDR-delivery project is reported too");
    assert!(
        !allowed
            .issues
            .iter()
            .any(|issue| issue.code == HDR_SOURCE_ON_SDR_DELIVERY),
        "§5.1's lane must lift §7 item 2's block: {:?}",
        allowed.issues,
    );
    assert!(
        !allowed
            .issues
            .iter()
            .any(|issue| issue.code == "unsupported_delivery_color"),
        "§5.1's lane must not be reported as an unsupported delivery colour: {:?}",
        allowed.issues,
    );
    println!(
        "CC8_MEASURED fixture=7 bullet=hdr_source_block sdr_delivery_blocked=true \
         hdr_delivery_blocked=false"
    );
}

/// **§9.1 fixture 7 / §7 item 3.** Setting an HDR delivery description is
/// validated against §5.1's table by the ordinary operation.
///
/// §7 item 3: "Setting an HDR delivery description is an ordinary undoable,
/// revision-gated, journalled operation, validated against §5.1's table."
/// `Operation::SetColorContext` is that operation and CC1's own fixtures cover
/// its undo, revision gating, and journalling; what this asserts is the clause
/// step 6 added — the validation — and that an SDR context still applies
/// unchanged, which is the direction §9.1 fixture 6 would not see because it
/// never edits a colour context.
#[test]
fn cc8_setting_an_hdr_delivery_description_is_validated_against_the_lane_table() {
    let mut document = Document {
        color_context: ColorContext::sdr_rec709(),
        ..Document::default()
    };
    // The passing direction: §5.1's lane applies.
    let mut context = ColorContext::sdr_rec709();
    context.delivery = cc8_hdr_delivery_description();
    kinewright_core::apply_batch(
        &mut document,
        &[kinewright_core::Operation::SetColorContext {
            color_context: context.clone(),
        }],
    )
    .expect("§5.1's lane must be settable");
    assert_eq!(document.color_context.delivery, context.delivery);

    // Every failing direction: an HDR *pair* whose remaining fields are not
    // §5.1's row cannot be stored, so the colour status can never show a
    // half-formed HDR lane that only fails at the encoder.
    for (label, delivery) in [
        (
            "PQ",
            ColorDescription {
                transfer: ColorTransfer::Smpte2084,
                ..cc8_hdr_delivery_description()
            },
        ),
        (
            "bt709 matrix",
            ColorDescription {
                matrix: ColorMatrix::Bt709,
                ..cc8_hdr_delivery_description()
            },
        ),
        (
            "full range",
            ColorDescription {
                range: ColorRange::Full,
                ..cc8_hdr_delivery_description()
            },
        ),
        (
            "8-bit",
            ColorDescription {
                bit_depth: ColorBitDepth::Eight,
                ..cc8_hdr_delivery_description()
            },
        ),
    ] {
        let mut broken = ColorContext::sdr_rec709();
        broken.delivery = delivery;
        let error = kinewright_core::apply_batch(
            &mut document.clone(),
            &[kinewright_core::Operation::SetColorContext {
                color_context: broken,
            }],
        )
        .expect_err(&format!("{label}: §7 item 3 must refuse this description"));
        let message = format!("{error}");
        assert!(
            message.contains("unsupported CC8 HDR delivery description"),
            "{label}: the refusal must be the typed §7 item 3 one: {message}",
        );
    }

    // An SDR context still applies, unchanged, through the same operation.
    kinewright_core::apply_batch(
        &mut document,
        &[kinewright_core::Operation::SetColorContext {
            color_context: ColorContext::sdr_rec709(),
        }],
    )
    .expect("an SDR colour context must still apply");
    assert_eq!(
        document.color_context.delivery,
        ColorContext::sdr_rec709().delivery,
    );
    println!("CC8_MEASURED fixture=7 bullet=set_color_context validated_against=§5.1");
}

// ---------------------------------------------------------------------------
// §9.1 fixture 8 — "The cross-platform encoded HDR fixture", the central gate.
// ---------------------------------------------------------------------------
//
// §9.1, verbatim: "A synthetic HDR source is exported through the production
// path, re-probed, decoded, and gated on: probed tags exactly `bt2020` /
// `arib_std_b67` / `bt2020_ncl` / `tv` / `yuv420p10le` / High 10; decoded
// native-plane BT.2020 legality; and difference budgets against the re-rendered
// delivery reference. **In the default lane on both CI operating systems.**"
//
// The last sentence is why nothing here is `cfg`-gated, why no measurement is
// compared for equality against one build's decode, and why every budget below
// is a **constant** asserted against a recorded measurement rather than the
// live figure (§9.2's first rule, CC7 §0.3 PM-E12). The per-build measured
// numbers are printed under `CC8_MEASURED` and are reported, never gated.

/// The starved bitrate for §9.2's second rule, matching CC6 §11.2.13's: an
/// encode that still succeeds and still produces a valid deliverable.
const CC8_STARVED_VIDEO_BITRATE: u64 = 100_000;

/// Fill one `yuv420p10le` picture with [`CC8_SOURCE_BARS`] **rotated by the
/// frame index**.
///
/// [`fill_hdr_bars`] is deliberately static, because §10 step 4's fixtures read
/// frame zero and a moving picture would only cost them a motion-compensated
/// decode. Fixture 8 is the opposite case: it exports six pictures through
/// libx264 and measures decoded differences on five of them, and a still would
/// make the encode a single intra picture repeated — the B-frame reordering,
/// the rate control, and the per-frame sampling would all measure nothing. So
/// the bars rotate by one position per frame, which keeps every sampled frame's
/// content a permutation of the same eight known bars while making consecutive
/// pictures genuinely differ.
fn fill_hdr_delivery_bars(frame: &mut ffmpeg::frame::Video, frame_index: i64) {
    let width = CC8_PRECONDITION_SIZE.0 as usize;
    let height = CC8_PRECONDITION_SIZE.1 as usize;
    let bars = CC8_SOURCE_BARS.len();
    let rotation = usize::try_from(
        frame_index.rem_euclid(i64::try_from(bars).expect("the bar count fits an i64")),
    )
    .expect("a non-negative rotation fits a usize");
    let bar_at = |column: usize| CC8_SOURCE_BARS[(column / CC8_BAR_WIDTH + rotation) % bars];

    let luma_stride = frame.stride(0);
    let luma = frame.data_mut(0);
    for row in 0..height {
        for column in 0..width {
            let at = row * luma_stride + column * 2;
            luma[at..at + 2].copy_from_slice(&bar_at(column).luma.to_le_bytes());
        }
    }
    for (plane, chroma) in [(1_usize, false), (2_usize, true)] {
        let stride = frame.stride(plane);
        let data = frame.data_mut(plane);
        for row in 0..height / 2 {
            for column in 0..width / 2 {
                let bar = bar_at(column * 2);
                let value = if chroma { bar.cr } else { bar.cb };
                let at = row * stride + column * 2;
                data[at..at + 2].copy_from_slice(&value.to_le_bytes());
            }
        }
    }
}

/// One `ffprobe` field of the first video stream of a written file.
///
/// An **independent** reader, in the manner `export.rs`'s own
/// `ffprobe_video_field` is: §9.1 fixture 8 asks for the H.264 *profile*, which
/// `probe_path` does not model, and asking the pinned CLI means the crate's own
/// probe and the CLI must agree about the file rather than the crate agreeing
/// with itself.
fn cc8_ffprobe_video_field(path: &Path, entry: &str) -> String {
    let output = std::process::Command::new(crate::test_support::ffprobe_executable())
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            &format!("stream={entry}"),
            "-of",
            "default=noprint_wrappers=1:nokey=1",
        ])
        .arg(path)
        .output()
        .expect("the provisioned ffprobe should run");
    assert!(
        output.status.success(),
        "ffprobe failed for {}: {}",
        path.display(),
        String::from_utf8_lossy(&output.stderr),
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

/// §9.1 fixture 8's synthetic HDR project: the moving-bar HLG clip, and a
/// document whose delivery description is §5.1's lane.
///
/// The clip is written by §10 step 1's own encoder helper, so a change to the
/// encoder construction cannot move for one CC8 fixture and not another, and it
/// is encoded losslessly (`qp=0`) so the *source* contributes no codec error to
/// the measurement — every difference the fixture reports belongs to the
/// delivery encode it is measuring.
fn cc8_hdr_delivery_project(directory: &TempDirectory) -> (Document, ColorDescription) {
    initialize_ffmpeg().expect("FFmpeg must initialize for the CC8 §9.1 fixture 8 source");
    let path = directory.path("cc8-fixture8-hlg-source.mp4");
    let source_params = encode_hdr_clip(
        &path,
        CC8_PRECONDITION_HLG_TRANSFER_PARAM,
        "fixture8",
        Some("qp=0"),
        fill_hdr_delivery_bars,
    );
    let mut asset = probe_path(&path, AssetId(1)).expect("the fixture 8 HDR source must probe");
    // §2.1's D65 rule: a probed H.264 stream carries no white point, and the
    // explicit assumption is applied to the *working copy* of the description,
    // never to the raw metadata.
    assert_eq!(
        asset.color_description.white_point,
        ColorWhitePoint::Unknown
    );
    let source_description = cc8_hlg_source_description(&asset.color_description);
    assert_eq!(
        classify_source_with_assumption(
            &source_description,
            Some(ColorSourceProfileAssumption::D65)
        ),
        Ok(ColorSourceProfile::HlgRec2020),
        "the passing direction: fixture 8's source must be a CC8 §2.1 HDR profile",
    );
    asset.color_description = source_description;
    assert_eq!(asset.duration, TimeCode(CC8_PRECONDITION_FRAMES));
    assert_eq!(asset.resolution, Some(CC8_PRECONDITION_SIZE));

    let mut document = single_clip_document(asset);
    document.color_context = ColorContext::sdr_rec709();
    // §3.1: the working and monitoring descriptions are unchanged — CC8 changes
    // the *delivery* description and nothing else about the colour context.
    document.color_context.delivery = cc8_hdr_delivery_description();
    document
        .validate()
        .expect("the fixture 8 HDR document should validate");
    println!(
        "CC8_MEASURED fixture=8 source x264_params=\"{source_params}\" bars={} \
         raster={}x{} frames={CC8_PRECONDITION_FRAMES}",
        CC8_SOURCE_BARS.len(),
        CC8_PRECONDITION_SIZE.0,
        CC8_PRECONDITION_SIZE.1,
    );
    (document, cc8_hdr_delivery_description())
}

/// Assert §5.2's lane-derived terms on the record the production export
/// actually handed `FFmpeg`.
///
/// The same [`DeliveryLaneTerms`] observation seam §9.1 fixture 6 reads, on the
/// other lane: every field is read back from the object that received the term,
/// so this is what the encode used and not a second transcription of it. §0.4's
/// site numbering is carried in the failure messages so a red run names the
/// literal that moved.
fn cc8_assert_hdr_lane_terms(terms: &DeliveryLaneTerms) {
    assert_eq!(
        terms.encoder_pixel_format, "yuv420p10le",
        "§5.1's `Pixel format` row",
    );
    assert_eq!(
        terms.encoder_colorspace, "bt2020nc",
        "§0.4 item 3 (export.rs:216) set_colorspace must be the lane's",
    );
    assert_eq!(
        terms.encoder_color_range, "tv",
        "§0.4 item 3 (export.rs:217) set_color_range must be the lane's",
    );
    assert_eq!(
        terms.encoder_options.get("x264-params").map(String::as_str),
        Some(CC8_HDR_DELIVERY_X264_PARAMS),
        "§0.4 item 4 (export.rs:529): the HDR lane's x264-params must be §5.2 item 2's string",
    );
    assert_eq!(
        terms.encoder_options.get("preset").map(String::as_str),
        Some("medium"),
        "the preset rides the same dictionary and §5.1 does not move it",
    );
    // §5.2 item 2's proven string, character for character: this is the exact
    // parameter §10 step 1's precondition encoded and re-probed, so a green
    // precondition is evidence about this export.
    assert_eq!(
        CC8_HDR_DELIVERY_X264_PARAMS,
        format!(
            "{CC8_PRECONDITION_PRIMARIES_PARAM}:{CC8_PRECONDITION_HLG_TRANSFER_PARAM}:\
             {CC8_PRECONDITION_MATRIX_PARAM}"
        ),
        "the lane's x264-params must be the string §10 step 1 proved on this build",
    );
    assert_eq!(
        terms.scale_args,
        format!(
            "w={}:h={}:flags=bicubic:in_range=jpeg:out_range=mpeg:out_color_matrix=bt2020",
            CC8_PRECONDITION_SIZE.0, CC8_PRECONDITION_SIZE.1,
        ),
        "§0.4 item 6 (export.rs:425): out_color_matrix must be the lane's, and \
         DELIVERY_SCALER_FLAGS and the two range terms must not move (§5.1)",
    );
    assert_eq!(
        terms.buffer_args,
        format!(
            "video_size={}x{}:pix_fmt=rgba64le:time_base=1/1:pixel_aspect=1/1:\
             colorspace=gbr:range=jpeg",
            CC8_PRECONDITION_SIZE.0, CC8_PRECONDITION_SIZE.1,
        ),
        "the buffer source describes an RGB intermediate on every lane; §5.1 leaves the \
         single-pass filter graph unchanged",
    );
    assert_eq!(terms.format_args, "pix_fmts=yuv420p10le");
    assert_eq!(terms.graph_pixel_format, "yuv420p10le");
    assert_eq!(
        terms.intermediate_frame,
        FrameColorTerms {
            pixel_format: "rgba64le".to_owned(),
            space: "gbr".to_owned(),
            range: "pc".to_owned(),
            primaries: "bt2020".to_owned(),
            transfer: "arib-std-b67".to_owned(),
        },
        "§0.4 item 8 (export.rs:626): the RGBA intermediate carries the lane's primaries and \
         transfer, because those are what its samples are",
    );
    assert_eq!(
        terms.delivery_frame,
        FrameColorTerms {
            pixel_format: "yuv420p10le".to_owned(),
            space: "bt2020nc".to_owned(),
            range: "tv".to_owned(),
            primaries: "bt2020".to_owned(),
            transfer: "arib-std-b67".to_owned(),
        },
        "§0.4 item 8 (export.rs:638): the delivery Y'CbCr stamp must be the lane's",
    );
}

/// **§9.1 fixture 8.** The cross-platform encoded HDR fixture.
///
/// One synthetic HLG Rec.2020 source, exported through the **production** path
/// (`export_document_capturing_delivery_terms`, which is
/// `export_document_with_luts` with §9.1 fixture 6's observation seam left on),
/// then re-probed and decoded through CC6's own `verify_delivery_output`.
///
/// Four claims:
///
/// 1. **The lane terms** §5.2 makes lane-derived, read back from the objects
///    that received them.
/// 2. **The probed tags**, exactly `bt2020` / `arib_std_b67` / `bt2020_ncl` /
///    `tv` / `yuv420p10le` / High 10 — asserted from *two* independent readers,
///    the crate's `probe_path` and the pinned `ffprobe`, because §5.2 clause 4
///    makes a tag that does not survive a failure and one reader agreeing with
///    itself is not a re-probe.
/// 3. **Decoded native-plane BT.2020 legality**, from CC6's own decoded-plane
///    report, which measures the actual Y, Cb and Cr planes of the written file.
/// 4. **Difference budgets** against the re-rendered delivery reference, whose
///    reference luma goes through §6 item 1's BT.2020 NCL matrix rather than
///    BT.709's.
#[test]
#[allow(clippy::too_many_lines)]
fn cc8_encoded_hdr_delivery_fixture() {
    // Rule 11.0.6: the panicking acquisition, never the skipping one.
    let gpu = fallback_gpu();
    let directory = TempDirectory::new("cc8-encoded-hdr-delivery");
    let (document, delivery) = cc8_hdr_delivery_project(&directory);
    let document = Arc::new(document);

    let settings = DeliveryProfile::SourceMaster.export_settings(
        document.as_ref(),
        DeliveryEncodeDepth::Ten,
        kinewright_core::ExportCancellation::default(),
    );
    assert_eq!(settings.video_codec, CC8_PRECONDITION_CODEC);
    assert_eq!(settings.delivery_color, delivery);
    assert_eq!(
        DeliveryLane::for_description(&settings.delivery_color),
        DeliveryLane::HdrHlgRec2020,
    );

    // --- claim 1: the lane terms, from the production export ---------------
    let output = directory.path("cc8-fixture8-hdr-delivery.mp4");
    let (progress_tx, _progress_rx) = crossbeam_channel::unbounded();
    let terms = crate::export::export_document_capturing_delivery_terms(
        document.as_ref(),
        &output,
        &settings,
        &progress_tx,
        gpu.context(),
    )
    .expect("the production export must write CC8 §5.1's HDR lane");
    cc8_assert_hdr_lane_terms(&terms);

    // --- claim 2: the probed tags, from two independent readers -------------
    let probed = probe_path(&output, AssetId(9))
        .expect("§5.2 clause 4: the written HDR file must re-probe")
        .color_description;
    assert_eq!(probed.primaries, ColorPrimaries::Bt2020);
    assert_eq!(probed.transfer, ColorTransfer::AribStdB67);
    assert_eq!(probed.matrix, ColorMatrix::Bt2020Ncl);
    assert_eq!(probed.range, ColorRange::Limited);
    assert_eq!(probed.bit_depth, ColorBitDepth::Ten);
    assert_eq!(
        probed.provenance,
        ColorProvenance::StreamMetadata,
        "the tags must be read from the stream, not inferred: an inferred description would \
         make every tag assertion above vacuous",
    );
    let ffprobe_profile = cc8_ffprobe_video_field(&output, "profile");
    let ffprobe_pixel_format = cc8_ffprobe_video_field(&output, "pix_fmt");
    assert_eq!(
        ffprobe_profile, "High 10",
        "§9.1 fixture 8 requires High 10; the pixel format alone selects it",
    );
    assert_eq!(ffprobe_pixel_format, "yuv420p10le");
    assert_eq!(cc8_ffprobe_video_field(&output, "color_range"), "tv");
    assert_eq!(cc8_ffprobe_video_field(&output, "color_space"), "bt2020nc");
    assert_eq!(
        cc8_ffprobe_video_field(&output, "color_transfer"),
        "arib-std-b67",
    );
    assert_eq!(
        cc8_ffprobe_video_field(&output, "color_primaries"),
        "bt2020",
    );

    // --- claims 3 and 4: decode, legality, and the budgets ------------------
    let request = DeliveryVerificationRequest::new(DeliveryEncodeDepth::Ten, delivery.clone());
    assert_eq!(
        request.budgets,
        kinewright_core::DeliveryBudgets::for_hdr_delivery_lane(),
        "§9.2 forbids inheriting another lane's budgets, so the HDR request must carry the \
         HDR set",
    );
    assert_ne!(
        request.budgets,
        kinewright_core::DeliveryBudgets::for_depth(DeliveryEncodeDepth::Ten),
        "and the HDR set must actually differ from the 10-bit SDR one, or 'its own budgets' \
         is a claim nothing measured",
    );
    // CC6 §6.3's distinctness rule, applied to CC8's lane: a codec budget and a
    // compositor tolerance must never be silently substitutable for one
    // another, because CC1's are flat-field numbers that would fail instantly
    // on any raster carrying a saturated edge.
    assert_ne!(
        request.budgets.luma_max_code,
        u32::from(crate::cc1_fixtures::MONITOR_CPU_GPU_MAX),
    );
    assert_ne!(request.budgets.luma_p99_code_millionths, 1_000_000);
    assert_ne!(request.budgets.luma_mean_code_millionths, 500_000);
    let engine = crate::engine::FfmpegMediaEngine::new_with_gpu(gpu.context())
        .expect("the production media engine should start");
    let verification = engine
        .verify_delivery_output(Arc::clone(&document), &output, &settings, request.clone())
        .expect("the written HDR export must verify");
    let comparison = &verification.comparison;

    println!(
        "CC8_MEASURED fixture=8 gate=\"Decoded HDR delivery\" lane={} gpu_lane={} \
         luma_max={} luma_p99_millionths={} luma_mean_millionths={} \
         rgb_mean_millionths_8bit_equiv={} psnr_hundredths={:?} rgb_max={} rgb_p99_millionths={} \
         budgets={:?}",
        DeliveryLane::HdrHlgRec2020.as_str(),
        gpu.lane.id(),
        comparison.luma.maximum_code_diff,
        comparison.luma.p99_code_diff_millionths,
        comparison.luma.mean_code_diff_millionths,
        comparison.combined.mean_code_diff_millionths,
        comparison.psnr_db_hundredths,
        comparison.combined.maximum_code_diff,
        comparison.combined.p99_code_diff_millionths,
        comparison.budgets,
    );
    println!(
        "CC8_MEASURED fixture=8 legality bit_depth={} luma_below={} luma_above={} \
         cb_below={} cb_above={} cr_below={} cr_above={} luma_min_hundredths={} \
         luma_max_hundredths={}",
        comparison.decoded_ycbcr.bit_depth,
        comparison.decoded_ycbcr.luma.below_count,
        comparison.decoded_ycbcr.luma.above_count,
        comparison.decoded_ycbcr.cb.below_count,
        comparison.decoded_ycbcr.cb.above_count,
        comparison.decoded_ycbcr.cr.below_count,
        comparison.decoded_ycbcr.cr.above_count,
        comparison.decoded_ycbcr.luma.minimum_code_hundredths,
        comparison.decoded_ycbcr.luma.maximum_code_hundredths,
    );

    assert!(
        verification.technical_pass,
        "the HDR export must pass verification: {:?}",
        verification.exceptions,
    );
    assert_eq!(verification.delivery_bit_depth, DeliveryEncodeDepth::Ten);
    assert_eq!(verification.decoded_pixel_format, "yuv420p10le");
    assert!(
        verification.tags.conforming,
        "§5.2 clause 4: a tag that does not survive is a failure: {:?}",
        verification.tags,
    );
    assert_eq!(verification.probed.primaries, ColorPrimaries::Bt2020);
    assert_eq!(verification.probed.transfer, ColorTransfer::AribStdB67);
    assert_eq!(verification.probed.matrix, ColorMatrix::Bt2020Ncl);
    assert_eq!(verification.probed.range, ColorRange::Limited);
    assert!(comparison.within_budgets, "{comparison:?}");
    // Claim 3, stated as its own assertion: the decoded planes were measured
    // from the file, at the lane depth, and the report is the decoded-plane one
    // rather than a prediction.
    assert_eq!(
        comparison.decoded_ycbcr.source,
        kinewright_core::YCbCrLegalSource::DecodedNativePlanes,
    );
    assert_eq!(comparison.decoded_ycbcr.bit_depth, 10);
    assert!(
        comparison.decoded_ycbcr.luma.samples_seen()
            && comparison.decoded_ycbcr.cb.samples_seen()
            && comparison.decoded_ycbcr.cr.samples_seen(),
        "a legality report over no samples proves nothing: {:?}",
        comparison.decoded_ycbcr,
    );
    assert_eq!(
        comparison.frames.len(),
        usize::from(kinewright_core::DELIVERY_VERIFICATION_FRAME_COUNT),
        "the sample must be CC6 §6.2's, not a shortened one",
    );

    // §9.2's second rule, on the two terms that measure zero or one code on the
    // passing source: a deliberately starved encode of the same project must
    // break the same budgets, so the constants are reachable rather than
    // decorative. Nothing about the starved run is gated as a *number*; what is
    // gated is that it does not fit.
    let starved_output = directory.path("cc8-fixture8-hdr-starved.mp4");
    let mut starved_settings = settings.clone();
    starved_settings.video_bitrate = CC8_STARVED_VIDEO_BITRATE;
    crate::export::export_document_capturing_delivery_terms(
        document.as_ref(),
        &starved_output,
        &starved_settings,
        &progress_tx,
        gpu.context(),
    )
    .expect("the starved HDR export must still write a valid deliverable");
    let starved = engine
        .verify_delivery_output(
            Arc::clone(&document),
            &starved_output,
            &starved_settings,
            request,
        )
        .expect("the starved HDR export must still verify");
    println!(
        "CC8_MEASURED fixture=8 starved bitrate={CC8_STARVED_VIDEO_BITRATE} luma_max={} \
         luma_p99_millionths={} luma_mean_millionths={} rgb_mean_millionths_8bit_equiv={} \
         psnr_hundredths={:?}",
        starved.comparison.luma.maximum_code_diff,
        starved.comparison.luma.p99_code_diff_millionths,
        starved.comparison.luma.mean_code_diff_millionths,
        starved.comparison.combined.mean_code_diff_millionths,
        starved.comparison.psnr_db_hundredths,
    );
    assert!(
        !starved.comparison.within_budgets,
        "a {CC8_STARVED_VIDEO_BITRATE} b/s encode of this HDR source must not fit §5.1's lane \
         budgets, or the budgets bound nothing: {:?}",
        starved.comparison,
    );
    assert!(!starved.technical_pass);
    // A difference failure, not a tag failure: the two are reported separately,
    // and the starved file's tags are still the lane's.
    assert!(starved.tags.conforming, "{:?}", starved.tags);
    let tripped: Vec<String> = starved
        .exceptions
        .iter()
        .filter(|exception| exception.code == "decoded_difference_over_budget")
        .filter_map(|exception| exception.field.clone())
        .collect();
    assert!(
        !tripped.is_empty(),
        "a starved encode reports the budget it broke: {:?}",
        starved.exceptions,
    );
    println!("CC8_MEASURED fixture=8 starved_tripped={tripped:?}");

    // The measurement never moves the file it measured.
    assert_eq!(verification.output_path, output);
    assert!(output.is_file());
}

/// **§9.1 fixture 8's intermediate-white determination**, asserted on its own
/// so it has its own name in a backtrace and its own line in the evidence.
///
/// CC6's `DELIVERY_INTERMEDIATE_WHITE = 65_280` convention is one of the three
/// things §5.1 names as "unchanged and ... not re-measured" on the HDR lane, so
/// the question step 6 has to answer is not *what the number is* but **what
/// white means on the HDR intermediate**. The contract settles it: the
/// intermediate carries the lane's transfer-coded signal, and `65_280` is
/// `libswscale`'s nominal 16-bit RGB white, so what lands on `65_280` is the
/// **HLG signal peak** `E' = 1.0` — not the working diffuse white.
///
/// The consequence is the relation the source side already carries, run
/// backwards: working `1.0` is BT.2408's HLG diffuse white, which is
/// [`CC8_HLG_REFERENCE_WHITE_SIGNAL_PERCENT`] of the signal range, so it
/// encodes to `E' = 0.75`, intermediate code `48_960`, and 10-bit limited luma
/// 721 — the same code `CC8_SOURCE_BARS`' `hlg_reference_white_75` bar is
/// written with. Guessing the other way — putting working `1.0` on `65_280` —
/// would have delivered every HDR project 1.92 stops hot and would have made
/// the specular range unrepresentable.
#[test]
fn cc8_hdr_delivery_intermediate_white_is_the_hlg_signal_peak() {
    use crate::color_pipeline::{DELIVERY_INTERMEDIATE_WHITE, encode_delivery_hlg_rec2020_rgba16};

    // The slack is CC8 §2.2's own half-nit argument, carried into codes rather
    // than invented here. `cc8_hlg_reference_white_signal_lands_on_the_anchor`
    // bounds the 75 % relation at **half a nit**, because 203 is BT.2408's own
    // figure rounded to integer cd/m²; the delivery direction inherits that
    // rounding through the same two stages. Half a nit at the anchor is
    // `0.5 / (peak * gamma * Y_S^(gamma-1)) = 0.5 / 918.5 = 5.44e-4` of scene
    // linear, and the OETF's slope there — `12a / (12E - b) = 0.741` — carries
    // it to `4.03e-4` of signal, which is `26.3` intermediate codes. Rounded up
    // to the next power of two: **32**. The observed deviation is 8 codes
    // (0.15 nits), a 4.0x margin, and it is printed below.
    const DIFFUSE_WHITE_CODE_BOUND: u16 = 32;

    assert_eq!(
        DELIVERY_INTERMEDIATE_WHITE, 65_280,
        "§5.1 keeps CC6's convention unchanged and does not re-measure it",
    );

    // (a) The HLG signal peak lands on nominal white. The working value that
    // produces it is the nominal peak over the anchor — a quotient of two
    // pinned constants, never a literal.
    let peak_working = cc8_nits(CC8_HLG_NOMINAL_PEAK_NITS) / cc8_nits(CC8_REFERENCE_WHITE_NITS);
    let peak_codes =
        encode_delivery_hlg_rec2020_rgba16([peak_working, peak_working, peak_working, 1.0]);
    for channel in 0..3 {
        assert_eq!(
            peak_codes[channel], DELIVERY_INTERMEDIATE_WHITE,
            "the HLG signal peak must land on nominal white: {peak_codes:?}",
        );
    }

    // (b) Working diffuse white lands on BT.2408's 75 % signal, not on nominal
    // white — the determination itself.
    let white_codes = encode_delivery_hlg_rec2020_rgba16([1.0, 1.0, 1.0, 1.0]);
    let expected_signal = cc8_nits(CC8_HLG_REFERENCE_WHITE_SIGNAL_PERCENT) / 100.0;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let expected_code = (expected_signal * f32::from(DELIVERY_INTERMEDIATE_WHITE)).round() as u16;
    for channel in 0..3 {
        assert!(
            white_codes[channel].abs_diff(expected_code) <= DIFFUSE_WHITE_CODE_BOUND,
            "working diffuse white must land on the 75 % HLG signal ({expected_code}), not on \
             nominal white: {white_codes:?}",
        );
        assert!(
            white_codes[channel] < DELIVERY_INTERMEDIATE_WHITE,
            "diffuse white must leave headroom for the specular range: {white_codes:?}",
        );
    }

    // (c) The delivery encode is the exact inverse of the decode chain: the
    // signal that comes back out is the signal the source-side stages would
    // have consumed. This is what makes §3.3's two lines mirror images rather
    // than two independently plausible orders.
    for signal in [0.0_f32, 0.25, 0.5, 0.75, 1.0] {
        let working = cc8_hlg_decode_working_linear([signal; 3]);
        let bt709 = cc8_apply_matrix(kinewright_core::CC8_REC2020_TO_BT709, working);
        let codes = encode_delivery_hlg_rec2020_rgba16([bt709[0], bt709[1], bt709[2], 1.0]);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let expected = (signal * f32::from(DELIVERY_INTERMEDIATE_WHITE)).round() as u16;
        for channel in 0..3 {
            assert!(
                codes[channel].abs_diff(expected) <= 8,
                "the delivery chain must invert the decode chain at signal {signal}: \
                 {codes:?} vs {expected}",
            );
        }
    }

    // (d) Alpha keeps the SDR convention: no transfer, the same scale.
    assert_eq!(
        encode_delivery_hlg_rec2020_rgba16([0.0, 0.0, 0.0, 1.0])[3],
        DELIVERY_INTERMEDIATE_WHITE,
    );

    println!(
        "CC8_MEASURED fixture=8 intermediate_white={DELIVERY_INTERMEDIATE_WHITE} \
         hlg_signal_peak_code={} diffuse_white_code={} diffuse_white_expected_code={} \
         diffuse_white_code_delta={} diffuse_white_code_bound={DIFFUSE_WHITE_CODE_BOUND} \
         diffuse_white_signal_percent={}",
        peak_codes[0],
        white_codes[0],
        expected_code,
        white_codes[0].abs_diff(expected_code),
        CC8_HLG_REFERENCE_WHITE_SIGNAL_PERCENT,
    );
}

// ---------------------------------------------------------------------------
// §9.1 fixture 12 — QC. §10 step 7's deliverable, and CC8 §6's four extensions.
// ---------------------------------------------------------------------------
//
// §9.1 fixture 12, verbatim: "**QC.** BT.2020 legality against hand-derivable
// analytic patches; the dual-triangle gamut report on content inside Rec.2020
// and outside Rec.709; MaxCLL/MaxFALL emitted as ungated rows; the
// withheld-skin reason asserted present on an HDR source and absent on an SDR
// one."
//
// Every raster below is **authored**, not rendered. §6's four extensions are
// arithmetic over one working proof, and a codec or a GPU in the path would put
// its own error between a hand-derived code and the number under test — which
// is exactly the confusion §6 item 1 exists to prevent. `measure_color_qc` is
// pure (CC6 §3.0), so an authored proof measures the production engine with
// nothing else in the way; fixture 8 is where the same engine meets real media.
//
// Nothing here is `cfg`-gated, no constant is per-OS, and no assertion is an
// equality against one build's decode (CC7 §0.3 PM-E12): the reports are
// compared against constants derived from `cc8_hdr`'s pinned coefficients, and
// every measured term is bounded by the house rule — `next_power_of_two` of the
// recorded measurement, times [`CC8_MEASURED_BOUND_HEADROOM`] — and printed
// under `CC8_MEASURED` for §10 step 10.

/// The delivery depth §5.1's HDR lane admits, as the QC code scale.
const CC8_QC_DEPTH: DeliveryEncodeDepth = DeliveryEncodeDepth::Ten;

/// `s = 2^(bits − 8)` at [`CC8_QC_DEPTH`], as an `f64` for the longhand code
/// derivation below.
const CC8_QC_CODE_SCALE: f64 = 4.0;

/// Wrap an authored row of working BT.709 linear triples as a full-resolution
/// working proof.
///
/// The provenance is honest: [`MonitorProofRenderKind::TestDouble`], with
/// `gpu_claim: false`, because nothing rendered this raster. `full_resolution`
/// is `true` because the row **is** the whole raster — a working proof always
/// binds full resolution (CC6 §2.3), and a proof that claimed otherwise would
/// be refused before any measurement ran.
fn cc8_qc_proof(pixels: &[[f32; 3]]) -> WorkingProof {
    assert!(
        !pixels.is_empty(),
        "a QC proof over no pixels proves nothing"
    );
    let width = u32::try_from(pixels.len()).expect("the fixture 12 rasters are small");
    let mut samples = Vec::with_capacity(pixels.len() * 4);
    for [red, green, blue] in pixels {
        // §3.1: the working raster is opaque by construction.
        samples.extend([*red, *green, *blue, 1.0]);
    }
    WorkingProof {
        metadata: WorkingProofMetadata {
            render: MonitorProofMetadata {
                render_kind: MonitorProofRenderKind::TestDouble,
                backend: "test_double".to_owned(),
                adapter: "test_double".to_owned(),
                software_fallback: true,
                gpu_claim: false,
                full_resolution: true,
            },
            stage: WORKING_PROOF_STAGE.to_owned(),
            encoding: WORKING_PROOF_ENCODING.to_owned(),
            raster_aspect_millionths: i64::from(width) * 1_000_000,
        },
        image: LinearRgbaImage {
            width,
            height: 1,
            pixels: samples,
        },
    }
}

/// A QC request on CC8 §5.1's HDR delivery lane.
///
/// The lane is **not** a field: `ColorQcRequest::delivery_lane` derives it from
/// `expected_delivery`, which is §5.2 clause 1's single source of truth, so
/// this helper sets the delivery description and the lane follows. The source
/// profile is the other half — §6 item 4 keys the withheld skin diagnostic on
/// the *source*, not on the lane.
fn cc8_hdr_qc_request(
    checks: Vec<ColorQcCheck>,
    source_profile: Option<ColorSourceProfile>,
) -> ColorQcRequest {
    ColorQcRequest {
        checks,
        delivery_bit_depth: CC8_QC_DEPTH,
        expected_delivery: Some(cc8_hdr_delivery_description()),
        source_profile,
        ..ColorQcRequest::default()
    }
}

/// A QC request on CC6's SDR Rec.709 lane at the same depth — fixture 12's
/// control for every "absent on an SDR one" clause.
fn cc8_sdr_qc_request(checks: Vec<ColorQcCheck>) -> ColorQcRequest {
    ColorQcRequest {
        checks,
        delivery_bit_depth: CC8_QC_DEPTH,
        expected_delivery: Some(cc8_sdr_delivery_description(CC8_QC_DEPTH)),
        source_profile: None,
        ..ColorQcRequest::default()
    }
}

/// The working BT.709 linear triple whose HDR-lane delivery encode is exactly
/// `signal`.
///
/// The exact analytic inverse of the encode the QC engine runs — §3.3's
/// delivery line read backwards — so a patch can be *authored by its signal*
/// and its `Y'CbCr` codes derived from that signal by hand. Going the other way
/// (authoring a working triple and reading back whatever signal appeared) would
/// make every expected code a restatement of the implementation.
fn cc8_working_for_hlg_signal(signal: [f32; 3]) -> [f32; 3] {
    cc8_apply_matrix(CC8_REC2020_TO_BT709, cc8_hlg_decode_working_linear(signal))
}

/// The 10-bit limited-range BT.2020 NCL codes of one HLG signal triple,
/// derived **longhand** from the authority module's pinned coefficients.
///
/// This is CC8 §6 item 1's formula written out where a reader can check it,
/// deliberately not a call to `bt2020_ncl_limited_ycbcr`: an expected value
/// computed by the function under test proves only that the function equals
/// itself.
///
/// ```text
/// Y'      = 0.2627·R' + 0.6780·G' + 0.0593·B'
/// Cb      = (B' − Y') / 1.8814          ( = 2·(1 − Kb) )
/// Cr      = (R' − Y') / 1.4746          ( = 2·(1 − Kr) )
/// Y_code  =  16·4 + 219·4·Y'  =  64 + 876·Y'
/// Cb_code = 128·4 + 224·4·Cb  = 512 + 896·Cb
/// Cr_code = 128·4 + 224·4·Cr  = 512 + 896·Cr
/// ```
fn cc8_hand_derived_bt2020_codes(signal: [f64; 3]) -> [f64; 3] {
    let [red, green, blue] = signal;
    let luma = CC8_BT2020_KR * red + CC8_BT2020_KG * green + CC8_BT2020_KB * blue;
    let cb = (blue - luma) / CC8_BT2020_CB_DENOMINATOR;
    let cr = (red - luma) / CC8_BT2020_CR_DENOMINATOR;
    let luma_offset = f64::from(YCBCR_LUMA_OFFSET) * CC8_QC_CODE_SCALE;
    let luma_span = f64::from(YCBCR_LUMA_SPAN) * CC8_QC_CODE_SCALE;
    let chroma_offset = f64::from(YCBCR_CHROMA_OFFSET) * CC8_QC_CODE_SCALE;
    let chroma_span = f64::from(YCBCR_CHROMA_SPAN) * CC8_QC_CODE_SCALE;
    [
        luma_offset + luma_span * luma,
        chroma_offset + chroma_span * cb,
        chroma_offset + chroma_span * cr,
    ]
}

/// One §9.1 fixture 12 legality patch, authored by its HLG signal.
#[derive(Debug, Clone, Copy)]
struct Cc8QcPatch {
    label: &'static str,
    signal: [f32; 3],
    /// Whether every plane of this patch sits inside the strict legal box —
    /// `[64, 940]` for luma and `[64, 960]` for chroma at 10 bits.
    legal: bool,
}

/// §9.1 fixture 12's analytic patch set.
///
/// The first four are neutral and land on codes a reader can check without a
/// calculator, because `Cb = Cr = 0` and `Y_code = 64 + 876·E'`:
///
/// ```text
/// E' = 0.00  ->  Y = 64    ( legal black )
/// E' = 0.50  ->  Y = 502   ( the HLG OETF's own breakpoint )
/// E' = 0.75  ->  Y = 721   ( BT.2408's HDR reference white, §2.2 )
/// E' = 1.00  ->  Y = 940   ( legal white, the HLG nominal peak )
/// ```
///
/// Those four are exactly the neutral bars `CC8_SOURCE_BARS` writes into
/// fixture 8's synthetic source, which is corroboration and is recorded as
/// such: the same four numbers arrived from the encoder side there and from the
/// QC side here.
///
/// The three saturated patches land on the chroma extremes the BT.2020
/// denominators are *defined* to produce — `Cb = 0.5` exactly at the blue
/// primary because `2·(1 − Kb)` is twice `1 − Kb`, and `Cr = 0.5` exactly at
/// the red primary for the same reason — so `Cb_code = Cr_code = 960`, sitting
/// **on** the legal ceiling and therefore not an excursion (CC6's comparisons
/// are strict).
///
/// The last two are the analytic controls §9.2's second rule requires: without
/// them every excursion count in this fixture measures zero, and a zero count
/// bounds nothing.
const CC8_QC_PATCHES: [Cc8QcPatch; 9] = [
    Cc8QcPatch {
        label: "neutral_legal_black",
        signal: [0.0, 0.0, 0.0],
        legal: true,
    },
    Cc8QcPatch {
        label: "neutral_half_signal",
        signal: [0.5, 0.5, 0.5],
        legal: true,
    },
    Cc8QcPatch {
        label: "neutral_reference_white_75",
        signal: [0.75, 0.75, 0.75],
        legal: true,
    },
    Cc8QcPatch {
        label: "neutral_legal_white",
        signal: [1.0, 1.0, 1.0],
        legal: true,
    },
    Cc8QcPatch {
        label: "rec2020_red_primary",
        signal: [1.0, 0.0, 0.0],
        legal: true,
    },
    Cc8QcPatch {
        label: "rec2020_green_primary",
        signal: [0.0, 1.0, 0.0],
        legal: true,
    },
    Cc8QcPatch {
        label: "rec2020_blue_primary",
        signal: [0.0, 0.0, 1.0],
        legal: true,
    },
    Cc8QcPatch {
        label: "control_above_legal_white",
        signal: [1.1, 1.1, 1.1],
        legal: false,
    },
    Cc8QcPatch {
        label: "control_below_legal_black",
        signal: [-0.05, -0.05, -0.05],
        legal: false,
    },
];

/// §9.1 fixture 12's recorded legality measurement: the largest absolute
/// deviation, in **delivery code units**, between the engine's predicted
/// `Y'CbCr` and [`cc8_hand_derived_bt2020_codes`] over [`CC8_QC_PATCHES`].
///
/// Taken by this fixture on `mifi/ffmpeg-builds 8.0-1` / rustc stable,
/// 2026-08-29, over **27 plane samples** (nine patches, three planes each):
///
/// ```text
/// term                      measured      bound (next_pow2 x 4)  margin
/// predicted_code_deviation  3.853619e-2   2.500000e-1            6.49x
/// ```
///
/// The figure is a round-trip residue and nothing else: the patch is authored
/// as a signal, carried to working BT.709 linear by the exact analytic inverse
/// of the encode, and carried back by the encode, all in `f32`, so what is left
/// is the working width's own error amplified by the `876`/`896` code spans.
/// The worst plane is the Rec.2020 red primary's `Cb`, at `386.9300` against a
/// derivation of `386.8915` — four hundredths of one delivery code out of a
/// 1 024-code range, which is `3.8e-5` of full scale.
///
/// It is **not** a per-OS number and is not compared against a decode: nothing
/// in this fixture's path touches a codec, and the two CI operating systems run
/// the same `f32` IEEE 754 arithmetic through the same authority-module
/// constants. The libm half of [`CC8_MEASURED_BOUND_HEADROOM`]'s argument still
/// applies, because the HLG OETF's `ln`/`exp` branch is on this path.
const CC8_FIXTURE12_LEGALITY_MEASURED: f32 = 3.853_619e-2;

/// §9.1 fixture 12's recorded `MaxCLL`/`MaxFALL` measurement: the largest
/// absolute deviation, in **hundredths of a cd/m²**, between the reported row
/// and the hand-computed value.
///
/// Taken on the same build and date, over the three-pixel raster the fixture
/// documents: **exactly zero**, on both numbers. That is not luck — every value
/// in the raster is a quotient or product of pinned integers
/// ([`CC8_REFERENCE_WHITE_NITS`] and [`CC8_HLG_NOMINAL_PEAK_NITS`]), the
/// conversion back to cd/m² multiplies by the same anchor, and the row is
/// rounded to hundredths, so the two roundings cancel exactly.
///
/// A zero has no next power of two, so §9.2's second rule applies: the bound is
/// the floor the fixture passes — one hundredth of a cd/m², the row's own unit
/// — and the fixture's negative control is what makes it reachable.
const CC8_FIXTURE12_LIGHT_LEVEL_MEASURED: f32 = 0.0;

/// The strict legal box at [`CC8_QC_DEPTH`], as `(low, luma_high, chroma_high)`
/// code values — CC6 §6.4's box, at this fixture's scale.
fn cc8_qc_legal_box() -> (f64, f64, f64) {
    (
        f64::from(YCBCR_LUMA_OFFSET) * CC8_QC_CODE_SCALE,
        f64::from(YCBCR_LUMA_LEGAL_HIGH) * CC8_QC_CODE_SCALE,
        f64::from(YCBCR_CHROMA_LEGAL_HIGH) * CC8_QC_CODE_SCALE,
    )
}

/// The single code one plane of a one-pixel measurement reported.
///
/// A one-pixel population has one extreme, so `minimum == maximum`; asserting
/// that here is what makes reading either one honest.
fn cc8_qc_plane_code(plane: kinewright_core::PlaneLegalExcursion, label: &str) -> f64 {
    assert!(
        plane.samples_seen(),
        "{label}: a plane that saw no sample has no code to report",
    );
    assert_eq!(
        plane.minimum_code_hundredths, plane.maximum_code_hundredths,
        "{label}: a one-pixel population must report one extreme",
    );
    #[allow(clippy::cast_precision_loss)]
    let code = plane.minimum_code_hundredths as f64 / 100.0;
    code
}

/// **§9.1 fixture 12, clause 1.** BT.2020 legality against hand-derivable
/// analytic patches.
///
/// CC8 §6 item 1, verbatim: "Legality (EBU R 103-shaped, as CC6 §6.4) is
/// measured against **the lane's own matrix**. Reusing the BT.709 reference on
/// a BT.2020 file would be a wrong number, not an approximate one."
///
/// Four claims:
///
/// 1. Every patch's predicted `Y'CbCr`, measured through the production
///    `measure_color_qc` on §5.1's lane, equals the longhand BT.2020 NCL
///    derivation from its own signal, within the house-rule bound.
/// 2. The four neutral patches land on the exact integer codes above, and the
///    two chroma extremes land on exactly `960`.
/// 3. **The failing direction**: the same patches through CC6's BT.709
///    reference are a different number — not a nearby one — on every saturated
///    patch, which is what makes clause 1 a measurement rather than a
///    coincidence.
/// 4. The legality counts over the whole patch row are exactly the two control
///    patches, in the directions they were authored for. Without the controls
///    every count is zero, and §9.2's second rule refuses a bound nothing
///    reaches.
#[test]
#[allow(clippy::too_many_lines)]
fn cc8_qc_bt2020_legality_measures_the_lanes_own_matrix() {
    let (low, luma_high, chroma_high) = cc8_qc_legal_box();
    let mut worst = 0.0_f32;
    let mut worst_bt709_gap = f64::INFINITY;

    for patch in CC8_QC_PATCHES {
        let signal = [
            f64::from(patch.signal[0]),
            f64::from(patch.signal[1]),
            f64::from(patch.signal[2]),
        ];
        let expected = cc8_hand_derived_bt2020_codes(signal);
        let working = cc8_working_for_hlg_signal(patch.signal);
        let report = measure_color_qc(
            &cc8_qc_proof(&[working]),
            &cc8_hdr_qc_request(vec![ColorQcCheck::Range, ColorQcCheck::Gamut], None),
        )
        .unwrap_or_else(|error| panic!("{}: the patch must measure: {error}", patch.label));

        // The report says which lane produced these codes, so a reader never
        // has to infer the matrix from the numbers.
        assert_eq!(report.delivery_lane, DeliveryLane::HdrHlgRec2020.as_str());
        let predicted = report.range.predicted_ycbcr;
        assert_eq!(predicted.bit_depth, CC8_QC_DEPTH.bits());
        assert_eq!(
            predicted.source,
            kinewright_core::YCbCrLegalSource::Predicted
        );
        let observed = [
            cc8_qc_plane_code(predicted.luma, patch.label),
            cc8_qc_plane_code(predicted.cb, patch.label),
            cc8_qc_plane_code(predicted.cr, patch.label),
        ];
        for plane in 0..3 {
            #[allow(clippy::cast_possible_truncation)]
            let deviation = (observed[plane] - expected[plane]).abs() as f32;
            worst = worst.max(deviation);
        }

        // Claim 3, per patch: CC6's BT.709 reference on the same `R'G'B'`.
        // On a neutral patch the two references agree exactly — `Cb = Cr = 0`
        // and `Y' = E'` under any set of coefficients summing to one — so the
        // gap is taken over the saturated patches, which are the ones a wrong
        // reference actually mis-states.
        let bt709 = bt709_limited_ycbcr(signal, CC8_QC_DEPTH.bits());
        // Bit equality, not a float comparison with a tolerance: the question
        // is whether this patch was *authored* with three identical channels,
        // and every one in the table is a literal.
        let neutral = patch.signal[0].to_bits() == patch.signal[1].to_bits()
            && patch.signal[1].to_bits() == patch.signal[2].to_bits();
        if !neutral {
            let gap = (0..3)
                .map(|plane| (bt709[plane] - expected[plane]).abs())
                .fold(0.0_f64, f64::max);
            worst_bt709_gap = worst_bt709_gap.min(gap);
        }

        // Claim 4, per patch: the legal box, from the patch's own authored
        // intent rather than from what happened to be measured.
        let inside = observed[0] >= low
            && observed[0] <= luma_high
            && observed[1] >= low
            && observed[1] <= chroma_high
            && observed[2] >= low
            && observed[2] <= chroma_high;
        assert_eq!(
            inside, patch.legal,
            "{}: authored legal={} but codes {observed:?} against box [{low}, {luma_high}] / \
             [{low}, {chroma_high}]",
            patch.label, patch.legal,
        );
        println!(
            "CC8_MEASURED fixture=12 clause=legality patch={} signal={:?} \
             expected_codes=[{:.4}, {:.4}, {:.4}] observed_codes=[{:.4}, {:.4}, {:.4}] \
             bt709_reference_codes=[{:.4}, {:.4}, {:.4}] legal={}",
            patch.label,
            patch.signal,
            expected[0],
            expected[1],
            expected[2],
            observed[0],
            observed[1],
            observed[2],
            bt709[0],
            bt709[1],
            bt709[2],
            patch.legal,
        );
    }

    // Claim 2: the hand-checkable anchors, as literals. `64 + 876·E'` for the
    // neutrals, and the two chroma extremes the denominators define.
    for (signal, expected_luma) in [
        (0.0_f64, 64.0_f64),
        (0.5, 502.0),
        (0.75, 721.0),
        (1.0, 940.0),
    ] {
        let codes = cc8_hand_derived_bt2020_codes([signal; 3]);
        assert!(
            (codes[0] - expected_luma).abs() < 1e-9,
            "neutral signal {signal} must derive luma code {expected_luma}: {codes:?}",
        );
        assert!((codes[1] - 512.0).abs() < 1e-9, "{codes:?}");
        assert!((codes[2] - 512.0).abs() < 1e-9, "{codes:?}");
    }
    assert!(
        (cc8_hand_derived_bt2020_codes([0.0, 0.0, 1.0])[1] - 960.0).abs() < 1e-9,
        "the Rec.2020 blue primary must land Cb on exactly 960: 2·(1 − Kb) is twice (1 − Kb)",
    );
    assert!(
        (cc8_hand_derived_bt2020_codes([1.0, 0.0, 0.0])[2] - 960.0).abs() < 1e-9,
        "the Rec.2020 red primary must land Cr on exactly 960: 2·(1 − Kr) is twice (1 − Kr)",
    );

    // Claim 3, aggregated: the *smallest* gap over the saturated patches, so
    // one patch that happens to agree cannot carry the claim. The bound is not
    // a tolerance and is not measured — it is a statement that a wrong matrix
    // is wrong by whole delivery codes, which is §6 item 1's "a wrong number,
    // not an approximate one".
    assert!(
        worst_bt709_gap > 1.0,
        "CC6's BT.709 reference must differ from the BT.2020 one by more than a delivery code \
         on every saturated patch, or clause 1 proves nothing: {worst_bt709_gap}",
    );

    // Claim 4, aggregated: the whole patch row in one measurement, with the
    // controls doing the bounding §9.2's second rule asks for.
    let row: Vec<[f32; 3]> = CC8_QC_PATCHES
        .iter()
        .map(|patch| cc8_working_for_hlg_signal(patch.signal))
        .collect();
    let report = measure_color_qc(
        &cc8_qc_proof(&row),
        &cc8_hdr_qc_request(vec![ColorQcCheck::Range, ColorQcCheck::Gamut], None),
    )
    .expect("the patch row must measure");
    let predicted = report.range.predicted_ycbcr;
    assert_eq!(
        predicted.luma.above_count, 1,
        "exactly the above-white control: {predicted:?}",
    );
    assert_eq!(
        predicted.luma.below_count, 1,
        "exactly the below-black control: {predicted:?}",
    );
    // The two controls are neutral, so no chroma plane should leave the box —
    // but the Rec.2020 red and blue primaries land `Cr` and `Cb` **exactly on**
    // the ceiling `960`, and CC6's comparison there is strict. An `f32`
    // round-trip residue of a thousandth of a code is therefore enough to tip
    // one of them across, and pretending otherwise would be asserting that the
    // working width is exact. So the claim is the honest one: at most the two
    // boundary patches can be counted, and whatever is counted sits within one
    // delivery code of the ceiling, which is the boundary being measured rather
    // than an excursion being found.
    let chroma_excursions = predicted.cb.above_count
        + predicted.cb.below_count
        + predicted.cr.above_count
        + predicted.cr.below_count;
    assert!(
        chroma_excursions <= 2,
        "only the two primaries that touch the chroma ceiling may be counted: {predicted:?}",
    );
    for (name, plane) in [("cb", predicted.cb), ("cr", predicted.cr)] {
        #[allow(clippy::cast_precision_loss)]
        let maximum = plane.maximum_code_hundredths as f64 / 100.0;
        #[allow(clippy::cast_precision_loss)]
        let minimum = plane.minimum_code_hundredths as f64 / 100.0;
        assert!(
            (maximum - chroma_high).abs() <= 1.0,
            "{name}: the highest chroma sample is the ceiling-touching primary, so it must sit \
             within one code of {chroma_high}: {maximum}",
        );
        assert!(minimum >= low, "{name}: {minimum} below {low}");
    }

    cc8_assert_measured(
        "12",
        "BT.2020 legality excursion",
        "predicted_code_deviation",
        worst,
        CC8_FIXTURE12_LEGALITY_MEASURED,
        // No floor: the term is non-zero, because the report quantizes every
        // plane code to hundredths and the deviation is dominated by that
        // rounding rather than by the arithmetic. §9.2's second rule is
        // answered instead by the two control patches, which are what make the
        // legality counts above reachable.
        0.0,
    );
    println!(
        "CC8_MEASURED fixture=12 clause=legality patches={} luma_above={} luma_below={} \
         chroma_excursions={chroma_excursions} smallest_bt709_gap_codes={worst_bt709_gap:.4}",
        CC8_QC_PATCHES.len(),
        predicted.luma.above_count,
        predicted.luma.below_count,
    );
}

/// How far §9.1 fixture 12's dual-triangle raster is pulled toward the neutral
/// of its own magnitude, as a reciprocal.
///
/// A power of two rather than a chosen figure, and it is not a tolerance: it is
/// how far *inside* the Rec.2020 boundary the raster is authored to sit, so the
/// "inside Rec.2020" half of §9.1 fixture 12's clause is a construction rather
/// than a hope. One sixteenth of the distance to neutral is four orders of
/// magnitude above the `f32` round-trip residue `CC8_FIXTURE3_MEASURED`
/// records, and it leaves the raster far outside the Rec.709 triangle — which
/// the fixture asserts rather than assumes.
const CC8_QC_GAMUT_INSET_RECIPROCAL: f32 = 16.0;

/// Move one Rec.2020 linear triple toward the neutral of its own magnitude.
///
/// `fraction` is signed: a positive fraction moves *inside* the triangle and a
/// negative one moves outside it, which is how the same function produces both
/// the passing raster and its control.
fn cc8_toward_neutral(triple: [f32; 3], fraction: f32) -> [f32; 3] {
    let neutral = triple[0].max(triple[1]).max(triple[2]);
    triple.map(|channel| fraction.mul_add(neutral - channel, channel))
}

/// §9.1 fixture 12's dual-triangle raster, in **working BT.709 linear**.
///
/// [`cc8_wide_gamut_rec2020_raster`] is fixture 3's Rec.2020 raster, reused
/// rather than re-authored, moved `fraction` toward neutral and then carried
/// through §2.3's matrix into the working space — which is exactly the journey
/// §3.3 puts an HDR source through before any grading node sees it.
fn cc8_qc_gamut_raster(fraction: f32) -> Vec<[f32; 3]> {
    cc8_wide_gamut_rec2020_raster()
        .into_iter()
        .map(|triple| cc8_apply_matrix(CC8_REC2020_TO_BT709, cc8_toward_neutral(triple, fraction)))
        .collect()
}

/// **§9.1 fixture 12, clause 2.** The dual-triangle gamut report.
///
/// CC8 §6 item 2, verbatim: "CC8 makes the triangle a property of the report,
/// names it in `ColorGamutReport.definition`, and reports Rec.2020
/// representability for HDR-profile content. **Both are reported when they
/// differ**, because 'outside Rec.709 but inside Rec.2020' is exactly the fact
/// an editor delivering HDR needs, and collapsing it loses the SDR-compatibility
/// signal."
///
/// §12's third risk supplies the other half and this fixture gates both: "§6
/// requires both to be reported only where they differ, with the relation
/// normative and printed as a line."
///
/// Five claims, each with its own failing direction:
///
/// 1. On content inside Rec.2020 and outside Rec.709, both reports are
///    published, they are named by triangle, and the Rec.2020 one is empty.
/// 2. The relation is present as a line, and the one subtraction it sanctions
///    is the SDR-compatibility signal.
/// 3. On the same raster with an **SDR** request, there is no second triangle
///    and no relation line — and the Rec.709 report is byte-identical, so CC8
///    moved nothing CC6 measured.
/// 4. On content that is in gamut in **both** triangles the reports agree, so
///    only one is published. This is the clause §12 asks for, and without it
///    "reported only where they differ" is untested.
/// 5. The analytic control for clause 1's zero: content pushed **outside**
///    Rec.2020 fills the second report and raises its own exception.
#[test]
#[allow(clippy::too_many_lines)]
fn cc8_qc_dual_triangle_gamut_reports_both_only_where_they_differ() {
    let inside = cc8_qc_gamut_raster(1.0 / CC8_QC_GAMUT_INSET_RECIPROCAL);
    let checks = vec![ColorQcCheck::Range, ColorQcCheck::Gamut];
    let hdr = measure_color_qc(
        &cc8_qc_proof(&inside),
        &cc8_hdr_qc_request(checks.clone(), Some(ColorSourceProfile::HlgRec2020)),
    )
    .expect("the wide-gamut raster must measure on the HDR lane");

    // Claim 1.
    assert_eq!(hdr.gamut.triangle, kinewright_core::GAMUT_TRIANGLE_REC709);
    assert_eq!(hdr.gamut.definition, kinewright_core::GAMUT_DEFINITION);
    let rec2020 = hdr
        .gamut_rec2020
        .as_ref()
        .expect("§6 item 2 publishes the second triangle on HDR content that differs");
    assert_eq!(rec2020.triangle, kinewright_core::GAMUT_TRIANGLE_REC2020);
    assert_eq!(
        rec2020.definition,
        kinewright_core::GAMUT_DEFINITION_REC2020
    );
    assert!(
        hdr.gamut.out_of_gamut_pixel_count > 0,
        "a raster of saturated Rec.2020 hues must leave the Rec.709 triangle, or clause 1 is \
         vacuous: {:?}",
        hdr.gamut,
    );
    assert_eq!(
        rec2020.out_of_gamut_pixel_count, 0,
        "the raster is authored one sixteenth inside the Rec.2020 boundary, so nothing may be \
         outside it: {rec2020:?}",
    );
    assert_eq!(rec2020.minimum_linear_millionths, 0);
    assert_eq!(rec2020.below_black_pixel_count, 0);

    // Claim 2. The nesting is a fact about the triangles, so it is asserted as
    // an inequality and not merely printed.
    assert!(
        rec2020.out_of_gamut_pixel_count <= hdr.gamut.out_of_gamut_pixel_count,
        "out-of-Rec.2020 is a subset of out-of-Rec.709: {rec2020:?} vs {:?}",
        hdr.gamut,
    );
    assert_eq!(
        hdr.gamut_triangle_relation.as_deref(),
        Some(kinewright_core::GAMUT_TRIANGLE_RELATION),
    );
    let sdr_incompatible = hdr.gamut.out_of_gamut_pixel_count - rec2020.out_of_gamut_pixel_count;
    assert!(sdr_incompatible > 0);

    // Claim 3.
    let sdr = measure_color_qc(&cc8_qc_proof(&inside), &cc8_sdr_qc_request(checks.clone()))
        .expect("the same raster must measure on the SDR lane");
    assert_eq!(sdr.delivery_lane, DeliveryLane::SdrRec709.as_str());
    assert!(
        sdr.gamut_rec2020.is_none(),
        "an SDR measurement has one triangle: {:?}",
        sdr.gamut_rec2020,
    );
    assert!(sdr.gamut_triangle_relation.is_none());
    assert_eq!(
        sdr.gamut, hdr.gamut,
        "the Rec.709 gamut report is a property of the working raster and not of the lane, so \
         CC8 must not have moved CC6's number",
    );
    assert!(
        sdr.exceptions
            .iter()
            .all(|exception| exception.code != "delivery_gamut_excursion_rec2020"),
    );

    // Claim 4: agreement collapses to one report. A neutral ramp is in gamut in
    // both triangles, so the two reports are equal in every measured field.
    let neutral: Vec<[f32; 3]> = (0..16)
        .map(|step| {
            #[allow(clippy::cast_precision_loss)]
            let magnitude = step as f32 / 4.0;
            [magnitude; 3]
        })
        .collect();
    let agreeing = measure_color_qc(
        &cc8_qc_proof(&neutral),
        &cc8_hdr_qc_request(checks.clone(), Some(ColorSourceProfile::HlgRec2020)),
    )
    .expect("the neutral ramp must measure");
    assert_eq!(agreeing.gamut.out_of_gamut_pixel_count, 0);
    assert!(
        agreeing.gamut_rec2020.is_none(),
        "§12's third risk: both are reported only where they differ, and on in-gamut content \
         they do not: {:?}",
        agreeing.gamut_rec2020,
    );
    assert!(agreeing.gamut_triangle_relation.is_none());

    // Claim 5: the control. The same raster pushed outside Rec.2020.
    let outside = cc8_qc_gamut_raster(-1.0);
    let control = measure_color_qc(
        &cc8_qc_proof(&outside),
        &cc8_hdr_qc_request(checks, Some(ColorSourceProfile::HlgRec2020)),
    )
    .expect("the control raster must measure");
    let control_rec2020 = control
        .gamut_rec2020
        .as_ref()
        .expect("the control differs from the Rec.709 report");
    assert!(
        control_rec2020.out_of_gamut_pixel_count > 0,
        "the control must reach the second triangle, or clause 1's zero bounds nothing: \
         {control_rec2020:?}",
    );
    assert!(control_rec2020.minimum_linear_millionths < 0);
    let raised = control
        .exceptions
        .iter()
        .find(|exception| exception.code == "delivery_gamut_excursion_rec2020")
        .expect("the second triangle raises its own code, never the Rec.709 one");
    assert_eq!(raised.severity, QaSeverity::Warning);
    assert!(raised.message.contains("Rec.2020 chromaticity triangle"));
    assert!(
        raised.message.contains("must never be summed"),
        "the exception carries the relation, so a reader who sees only the exception still \
         cannot add the two counts: {}",
        raised.message,
    );
    assert!(
        control.technical_pass,
        "a gamut excursion is a Warning, never an Error: {:?}",
        control.exceptions,
    );

    println!(
        "CC8_MEASURED fixture=12 clause=dual_triangle pixels={} rec709_out={} rec2020_out={} \
         sdr_incompatible={sdr_incompatible} control_rec2020_out={} \
         control_rec2020_min_linear_millionths={} agreeing_second_report={}",
        inside.len(),
        hdr.gamut.out_of_gamut_pixel_count,
        rec2020.out_of_gamut_pixel_count,
        control_rec2020.out_of_gamut_pixel_count,
        control_rec2020.minimum_linear_millionths,
        agreeing.gamut_rec2020.is_some(),
    );
}

/// **§9.1 fixture 12, clause 3.** `MaxCLL` and `MaxFALL` as ungated rows.
///
/// CC8 §6 item 3, verbatim: "`MaxCLL` and `MaxFALL` measurement — reported,
/// never gated. Computed from the working proof over the sampled frames, in the CC6
/// evidence style, with the sampled population and its bounds recorded beside
/// the number... they are not gated because CC8 does not write them into a file
/// and a threshold on them would be invented."
///
/// The raster is three pixels chosen so both numbers are hand-computable from
/// §2.2's anchor alone:
///
/// ```text
/// black            working 0                       ->     0 cd/m²
/// diffuse white    working 1                       ->   203 cd/m²   ( = the anchor )
/// specular         working 1000/203                ->  1000 cd/m²   ( the HLG nominal peak )
///
/// MaxCLL  = max = 1000 cd/m²
/// MaxFALL = (0 + 203 + 1000) / 3 = 401 cd/m²
/// ```
///
/// Four claims: the two numbers, the recorded population and its bounds, the
/// absence of any gate, and — the analytic control for a clamp that is not
/// there — a negative sample reported as a negative light level.
#[test]
fn cc8_qc_light_level_is_measured_and_reported_but_never_gated() {
    let anchor = cc8_nits(CC8_REFERENCE_WHITE_NITS);
    let specular = cc8_nits(CC8_HLG_NOMINAL_PEAK_NITS) / anchor;
    let raster = [[0.0_f32; 3], [1.0; 3], [specular; 3]];
    let report = measure_color_qc(
        &cc8_qc_proof(&raster),
        &cc8_hdr_qc_request(
            vec![ColorQcCheck::Range, ColorQcCheck::Gamut],
            Some(ColorSourceProfile::HlgRec2020),
        ),
    )
    .expect("the light-level raster must measure");
    let light = &report.light_level;

    // Claim 1: the two numbers, in hundredths of a cd/m².
    #[allow(clippy::cast_precision_loss)]
    let expected_max_cll = f64::from(CC8_HLG_NOMINAL_PEAK_NITS) * 100.0;
    #[allow(clippy::cast_precision_loss)]
    let expected_max_fall =
        (f64::from(CC8_REFERENCE_WHITE_NITS) + f64::from(CC8_HLG_NOMINAL_PEAK_NITS)) / 3.0 * 100.0;
    #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
    let deviation = ((light.max_cll_nits_hundredths as f64 - expected_max_cll)
        .abs()
        .max((light.max_fall_nits_hundredths as f64 - expected_max_fall).abs()))
        as f32;

    // Claim 2: the population and its bounds, recorded beside the number.
    assert_eq!(light.sampled_frame_count, 1);
    assert_eq!(light.sampled_pixel_count, raster.len() as u64);
    assert_eq!(light.sampled_pixel_count, report.visible_pixel_count);
    assert_eq!(light.project_frame, report.project_frame);
    assert_eq!(light.reference_white_nits, CC8_REFERENCE_WHITE_NITS);
    assert_eq!(light.boundary, kinewright_core::LIGHT_LEVEL_BOUNDARY);
    assert!(
        light.boundary.contains("lower bound"),
        "a one-frame MaxCLL must say it is a lower bound on a programme's",
    );

    // Claim 3: nothing gates it. The raster carries a 1 000-nit highlight,
    // which is four and a half stops over diffuse white and raises the range
    // warnings it should — and the measurement still passes, because §6 item 3
    // and §11 make the un-application deliberate.
    assert!(!light.gated);
    assert!(
        report.technical_pass,
        "light level gates nothing: {:?}",
        report.exceptions,
    );
    assert!(
        report.exceptions.iter().all(|exception| {
            !exception.code.contains("cll")
                && !exception.code.contains("fall")
                && !exception.code.contains("light_level")
                && exception.field.as_deref() != Some("light_level")
        }),
        "no exception may be raised from a light-level row: {:?}",
        report.exceptions,
    );

    // Claim 4: the control for the clamp that is not there. A pixel below black
    // reports a negative light level rather than a fabricated zero, which is
    // the same refusal CC6 makes for an unclamped range excursion.
    let negative = measure_color_qc(
        &cc8_qc_proof(&[[-1.0_f32; 3]]),
        &cc8_hdr_qc_request(vec![ColorQcCheck::Range, ColorQcCheck::Gamut], None),
    )
    .expect("the negative control must measure");
    assert_eq!(
        negative.light_level.max_cll_nits_hundredths,
        -i64::from(CC8_REFERENCE_WHITE_NITS) * 100,
        "a working −1.0 is −203 cd/m², reported and not clamped",
    );
    assert_eq!(
        negative.light_level.max_fall_nits_hundredths,
        negative.light_level.max_cll_nits_hundredths,
    );

    cc8_assert_measured(
        "12",
        "MaxCLL/MaxFALL",
        "reported_nits_hundredths_deviation",
        deviation,
        CC8_FIXTURE12_LIGHT_LEVEL_MEASURED,
        // §9.2's second rule again: the term measures zero on this raster
        // because every value is a product of pinned integers, so the floor is
        // the row's own unit — one hundredth of a cd/m². The negative control
        // above bounds the constant from the other side by exercising the same
        // arithmetic on a sample no clamp would have let through.
        1.0,
    );
    println!(
        "CC8_MEASURED fixture=12 clause=light_level max_cll_hundredths={} \
         max_fall_hundredths={} expected_max_cll_hundredths={expected_max_cll:.0} \
         expected_max_fall_hundredths={expected_max_fall:.0} sampled_pixels={} frames={} \
         gated={} negative_control_hundredths={}",
        light.max_cll_nits_hundredths,
        light.max_fall_nits_hundredths,
        light.sampled_pixel_count,
        light.sampled_frame_count,
        light.gated,
        negative.light_level.max_cll_nits_hundredths,
    );
}

/// A media asset carrying one source colour description and nothing else that
/// matters here.
///
/// Authored rather than probed: clause 4's question is what
/// `document_hdr_source_profile` says about a description, and fixture 8
/// already proves that a real HLG file probes to exactly this description.
fn cc8_qc_asset(
    name: &'static str,
    color_description: ColorDescription,
) -> kinewright_core::MediaAsset {
    kinewright_core::MediaAsset {
        id: AssetId(1),
        path: std::path::PathBuf::from(format!("/cc8/{name}.mp4")),
        name: name.to_owned(),
        duration: TimeCode(8),
        fps: Rational::new(25, 1).expect("25/1 is a valid exact frame rate"),
        kind: kinewright_core::MediaKind::Video,
        resolution: Some((320, 180)),
        source_fingerprint: kinewright_core::MediaSourceFingerprint::default(),
        color_description,
    }
}

/// **§9.1 fixture 12, clause 4.** The withheld-skin reason.
///
/// CC8 §6 item 4, verbatim: "On an HDR-profile source the skin report is
/// **withheld with a named reason**, not silently computed against the wrong
/// constants. This is the deferral CC8 is most likely to be asked to reverse,
/// and it should be reversed by measurement or not at all."
///
/// Four claims:
///
/// 1. On an HDR source the diagnostic is absent, the named reason is present,
///    and it names the profile that caused it.
/// 2. The reason is an Info exception too, so it reaches a surface that reads
///    only exceptions — §12's "a named limitation is a different support burden
///    from a mysterious one" — and Info never clears `technical_pass`.
/// 3. On an SDR source the same raster and the same request publish the
///    diagnostic and carry no withheld row at all. This is the "absent on an
///    SDR one" half, and without it clause 1 would pass on a build that
///    withheld everything.
/// 4. The surfaces' own resolver agrees: `document_hdr_source_profile` finds
///    §2.1's profile on an HDR document and nothing on an SDR one, so the agent
///    and the app cannot disagree with this engine about which is which.
#[test]
#[allow(clippy::too_many_lines)]
fn cc8_qc_skin_is_withheld_with_a_named_reason_on_an_hdr_source() {
    // A saturated warm ramp: a raster the CC6 diagnostic has something to say
    // about, so "absent" and "present" are both real answers.
    let raster: Vec<[f32; 3]> = (0..16)
        .map(|step| {
            #[allow(clippy::cast_precision_loss)]
            let magnitude = 0.2 + step as f32 / 32.0;
            [magnitude, magnitude * 0.6, magnitude * 0.45]
        })
        .collect();
    let checks = vec![ColorQcCheck::Range, ColorQcCheck::Gamut, ColorQcCheck::Skin];

    // Claim 1 and 2.
    let hdr = measure_color_qc(
        &cc8_qc_proof(&raster),
        &cc8_hdr_qc_request(checks.clone(), Some(ColorSourceProfile::HlgRec2020)),
    )
    .expect("the HDR skin request must measure");
    assert!(
        hdr.skin.is_none(),
        "§6 item 4 withholds the diagnostic rather than computing it: {:?}",
        hdr.skin,
    );
    let withheld = hdr
        .skin_withheld
        .as_ref()
        .expect("the withheld row is the named reason, and silence is what §6 refuses");
    assert_eq!(withheld.code, kinewright_core::SKIN_WITHHELD_CODE);
    assert_eq!(withheld.reason, kinewright_core::SKIN_WITHHELD_REASON);
    assert_eq!(withheld.boundary, kinewright_core::SKIN_DIAGNOSTIC_BOUNDARY);
    assert_eq!(
        withheld.source_profile,
        ColorSourceProfile::HlgRec2020.id(),
        "the reason names the profile that caused it",
    );
    assert!(
        withheld.reason.contains("Rec.709 primaries")
            && withheld.reason.contains("measurement programme"),
        "the reason must say why the constants do not transfer and what reversing it costs: {}",
        withheld.reason,
    );
    let raised = hdr
        .exceptions
        .iter()
        .find(|exception| exception.code == kinewright_core::SKIN_WITHHELD_CODE)
        .expect("a named limitation reaches an exceptions-only surface");
    assert_eq!(raised.severity, QaSeverity::Info);
    assert_eq!(raised.field.as_deref(), Some("skin"));
    assert!(hdr.technical_pass, "Info never clears technical_pass");

    // The withholding is a property of the source, not of the lane: an HDR
    // source delivered on an SDR lane is exactly §7 item 2's blocked project,
    // and it is the one that most needs to be told.
    let hdr_source_sdr_lane = measure_color_qc(
        &cc8_qc_proof(&raster),
        &ColorQcRequest {
            source_profile: Some(ColorSourceProfile::PqRec2020),
            ..cc8_sdr_qc_request(checks.clone())
        },
    )
    .expect("an HDR source on an SDR lane must measure");
    assert_eq!(
        hdr_source_sdr_lane.delivery_lane,
        DeliveryLane::SdrRec709.as_str()
    );
    assert!(hdr_source_sdr_lane.skin.is_none());
    assert_eq!(
        hdr_source_sdr_lane
            .skin_withheld
            .as_ref()
            .map(|withheld| withheld.source_profile.clone()),
        Some(ColorSourceProfile::PqRec2020.id().to_owned()),
    );

    // Claim 3: the SDR direction.
    let sdr = measure_color_qc(&cc8_qc_proof(&raster), &cc8_sdr_qc_request(checks))
        .expect("the SDR skin request must measure");
    let skin = sdr
        .skin
        .as_ref()
        .expect("CC6's diagnostic is unchanged on an SDR source");
    assert!(skin.considered_pixel_count > 0);
    assert_eq!(skin.boundary, kinewright_core::SKIN_DIAGNOSTIC_BOUNDARY);
    assert!(
        sdr.skin_withheld.is_none(),
        "nothing is withheld on an SDR source: {:?}",
        sdr.skin_withheld,
    );
    assert!(
        sdr.exceptions
            .iter()
            .all(|exception| exception.code != kinewright_core::SKIN_WITHHELD_CODE),
    );

    // Claim 4: the resolver both surfaces call.
    let hdr_document = single_clip_document(cc8_qc_asset(
        "cc8-hlg-source",
        cc8_hlg_source_description(&cc8_hdr_delivery_description()),
    ));
    assert_eq!(
        kinewright_core::document_hdr_source_profile(&hdr_document),
        Some(ColorSourceProfile::HlgRec2020),
    );
    let sdr_document = single_clip_document(cc8_qc_asset(
        "cc8-rec709-source",
        cc8_sdr_delivery_description(CC8_QC_DEPTH),
    ));
    assert_eq!(
        kinewright_core::document_hdr_source_profile(&sdr_document),
        None,
    );

    println!(
        "CC8_MEASURED fixture=12 clause=withheld_skin hdr_profile={} hdr_skin_present={} \
         hdr_withheld_present={} sdr_skin_present={} sdr_withheld_present={} \
         hdr_source_sdr_lane_withheld={} exception_severity={:?}",
        ColorSourceProfile::HlgRec2020.id(),
        hdr.skin.is_some(),
        hdr.skin_withheld.is_some(),
        sdr.skin.is_some(),
        sdr.skin_withheld.is_some(),
        hdr_source_sdr_lane.skin_withheld.is_some(),
        raised.severity,
    );
}

// ===========================================================================
// CC8 §10 step 8 — §9.1 fixture 9.
// ===========================================================================
//
// Step 8 is "Preview and UI: fixture 9." Four things land with it, and each
// has its gate below:
//
//  * **§4's tone-mapping stage**, `kinewright_core::cc8_preview_tone_map`,
//    with its one parameter pinned in the authority module as
//    `CC8_PREVIEW_PEAK_NITS` (§4 item 2) and named in the colour status as
//    `CC8_PREVIEW_STAGE` (§4 item 1, §3.3's monitoring branch);
//  * **the monitoring transform made a function of a named preview arm**,
//    `kinewright_core::MonitorPreview`, selected from the *source* by
//    `document_monitor_preview` and threaded through
//    `Compositor::render_monitor_preview_with_luts` — the same shape §5.2
//    clause 1 gave the delivery encode, applied to the monitoring branch;
//  * **§4 item 3's label**, `CC8_PREVIEW_BADGE` / `CC8_PREVIEW_LABEL`, on the
//    Program viewer and in `get_color_context`; and
//  * **§3.2 items 1 and 2's two named node limitations**, surfaced at the node
//    in the inspector (§8, §12) and in the colour status.
//
// **What step 8 does not take.** §7's `managed_hdr_v1` state and migration are
// §10 step 9's, and §9.2's measured gate table plus `cc8_manifest.json` are
// step 10's — including the "Preview parity" row, which still reads
// `ToBeMeasuredAtImplementation` in `CC8_GATES`. What this section does now is
// take that measurement, assert against it with the step-3 rule's stated
// margin, and print it under `CC8_MEASURED` so step 10 has a recorded figure.
//
// **Where the tone map runs.** On the **CPU**, in
// `compositor::Compositor::readback_for`, which is where CC1's monitor encode
// has always run: the GPU composites and grades into `Rgba16Float` and the
// display transform is applied to the mapped readback. There is therefore no
// WGSL copy of the curve to drift from the Rust one — which is the strongest
// parity statement available, and is *not* the measurement §4 item 4 asks for.
// The measurement below is the one that is meaningful: the production
// software-GPU composite through the production preview readback, against a
// wholly CPU reference of the same raster and the same correction, in monitor
// codes.

/// The monitoring description §4's stage encodes into: CC1's, unchanged.
///
/// §4: "the managed preview applies a named tone-mapping stage from the working
/// space to **the existing Rec.709 monitoring description**." So the preview
/// changes what reaches that description, never the description itself, and
/// this fixture reads it from `ColorContext` rather than building one.
fn cc8_preview_monitoring() -> ColorDescription {
    ColorContext::sdr_rec709().monitoring
}

/// The single delivery quantization, restated for the delivery-reachability
/// fixture below.
///
/// `color_pipeline::quantize_delivery16` is private, and deliberately: it is
/// the one clamp-and-round the delivery boundary is allowed. This is its
/// arithmetic written out so §9.1 fixture 9's failing direction can compare the
/// production delivery encode against a composition that provably contains no
/// tone map, rather than against the production encode itself.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn cc8_quantize_delivery16(value: f32) -> u16 {
    let clamped = if value.is_nan() {
        0.0
    } else {
        value.clamp(0.0, 1.0)
    };
    (clamped * f32::from(DELIVERY_INTERMEDIATE_WHITE)).round() as u16
}

/// A dense working-linear ramp from below black to past §4's peak.
///
/// The three populations §9.1 fixture 9's structural clauses need: undershoot
/// (the out-of-Rec.709 negatives §2.3 produces), the CC1 SDR domain, and HDR
/// magnitudes up to and beyond `cc8_preview_peak_working_linear`.
fn cc8_preview_ramp() -> Vec<f32> {
    const STEPS: i32 = 1_200;
    (-100..=STEPS)
        .map(|step| cc8_as_f32_local(step) * 6.0 / cc8_as_f32_local(STEPS))
        .collect()
}

/// `i32` to `f32` for this section's ramps. Every value is far inside `2^24`.
#[allow(clippy::cast_precision_loss)]
fn cc8_as_f32_local(value: i32) -> f32 {
    value as f32
}

/// §9.1 fixture 9, clause 1 — **determinism, monotonicity, and endpoint
/// behaviour** of §4's stage, measured through the production monitor encode
/// rather than on the curve alone.
///
/// §4 item 4 fixes what may be asserted: "Its fixtures assert determinism,
/// monotonicity, endpoint behaviour, and CPU/GPU parity — **properties, not
/// aesthetics**." Nothing here is a judgment about how the picture looks, and
/// no CC8 exit gate is (§0.2 Q4).
///
/// The three clauses, and what each would catch:
///
/// 1. **Determinism** is asserted bitwise over the whole ramp, twice, on the
///    composition `tone map -> BT.709 encode -> clamp -> quantize`. The curve
///    itself uses only IEEE 754 exact operations, so this would catch a stage
///    that acquired state or a dependence on evaluation order.
/// 2. **Monotonicity** is asserted on the *monitor codes*, which is where a
///    non-monotone tone map would show as banding or inversion, and is
///    necessarily non-strict because the codes are quantized. Strict
///    monotonicity of the curve is `cc8_hdr`'s own unit assertion.
/// 3. **Endpoint behaviour**: zero maps to code 0, §4's pinned peak maps to
///    code 255, everything past the peak stays at 255 through the existing
///    display clamp, and — the non-vacuity clause — diffuse white maps
///    **below** 255 on the tone-mapped arm while the SDR arm clips it to 255.
///    Without that last one a stage that did nothing at all would pass every
///    other clause here.
#[test]
fn cc8_preview_tone_map_is_deterministic_monotone_and_holds_its_endpoints() {
    let monitoring = cc8_preview_monitoring();
    let ramp = cc8_preview_ramp();
    let encode = |value: f32, preview: MonitorPreview| -> u8 {
        encode_monitor_for_preview([value; 3], &monitoring, preview).expect("CC1's monitor target")
            [0]
    };

    // Clause 1: bitwise determinism over the whole ramp.
    let first: Vec<u8> = ramp
        .iter()
        .map(|value| encode(*value, MonitorPreview::Cc8ToneMappedHdr))
        .collect();
    let second: Vec<u8> = ramp
        .iter()
        .map(|value| encode(*value, MonitorPreview::Cc8ToneMappedHdr))
        .collect();
    assert_eq!(first, second, "§4's stage is not deterministic");

    // Clause 2: monotone monitor codes, and a code range that is actually
    // exercised — a ramp that produced one code would be monotone vacuously.
    let mut previous = 0_u8;
    for (index, code) in first.iter().enumerate() {
        assert!(
            *code >= previous,
            "the tone-mapped monitor code fell at working {}: {previous} then {code}",
            ramp[index],
        );
        previous = *code;
    }
    assert_eq!(*first.first().expect("a ramp"), 0);
    assert_eq!(*first.last().expect("a ramp"), 255);

    // Clause 3: the endpoints, on the pinned peak itself.
    let peak = cc8_preview_peak_working_linear();
    assert_eq!(encode(0.0, MonitorPreview::Cc8ToneMappedHdr), 0);
    assert_eq!(
        encode(peak, MonitorPreview::Cc8ToneMappedHdr),
        255,
        "§4's pinned peak must land on monitor white",
    );
    assert_eq!(encode(peak * 2.0, MonitorPreview::Cc8ToneMappedHdr), 255);
    // Negative working values stay at the clamp's floor, and the stage did not
    // turn one into a positive code.
    assert_eq!(encode(-0.5, MonitorPreview::Cc8ToneMappedHdr), 0);

    // Non-vacuity, both directions.
    let tone_mapped_white = encode(1.0, MonitorPreview::Cc8ToneMappedHdr);
    assert!(
        tone_mapped_white < 255,
        "diffuse white must survive the preview as headroom, not as clipped white",
    );
    assert_eq!(
        encode(1.0, MonitorPreview::Direct),
        255,
        "CC1's monitor transform still clips diffuse white to 255",
    );
    let mut moved = 0_usize;
    for value in &ramp {
        let direct = encode(*value, MonitorPreview::Direct);
        let mapped = encode(*value, MonitorPreview::Cc8ToneMappedHdr);
        assert!(
            mapped <= direct,
            "the preview brightened working {value}: {direct} then {mapped}",
        );
        if mapped != direct {
            moved += 1;
        }
    }
    assert!(
        moved * 2 > ramp.len(),
        "the preview moved only {moved} of {} ramp samples; a proven no-op measures nothing",
        ramp.len(),
    );

    println!(
        "CC8_MEASURED fixture=9 clause=structure stage={CC8_PREVIEW_STAGE} \
         peak_nits={CC8_PREVIEW_PEAK_NITS} peak_working={peak:.6} samples={} moved={moved} \
         diffuse_white_code={tone_mapped_white}",
        ramp.len(),
    );
}

/// §9.1 fixture 9, the failing direction — **the preview transform is
/// unreachable from the delivery path**.
///
/// §4 item 5: "It is a *preview* transform. It must not be reachable from the
/// delivery path (§0.2 Q6), and a fixture asserts that (§9)." §0.2 Q6 is what
/// makes this a hard direction rather than a preference: SDR-from-HDR ships "as
/// a preview only, never as a deliverable in CC8", and §11 defers tone-mapped
/// delivery entirely because it "has no defensible objective pass threshold".
///
/// The assertion is an **equality against a composition that provably contains
/// no tone map**, on both delivery lanes, over a raster where the tone map
/// demonstrably moves values. A tone map inserted anywhere in the delivery
/// encode — in `encode_delivery_for_description`, in either lane's arm, or in
/// the QC engine's `encode_delivery_for_lane` — fails it. The report side is
/// included deliberately: a QC surface that predicted a tone-mapped clamp would
/// be reporting a clip no export applies, which is the same error seen from the
/// evidence side.
#[test]
// The array comparisons below are **exact-representation claims**: the delivery
// encode either is the untone-mapped composition or is not, and a tolerance
// there would be a window inside which a preview stage could hide.
#[allow(clippy::float_cmp, clippy::similar_names)]
fn cc8_preview_transform_is_unreachable_from_the_delivery_path() {
    let hdr_delivery = cc8_hdr_delivery_description();
    let sdr_delivery = cc8_sdr_delivery_description(DeliveryEncodeDepth::Ten);
    let mut moved = 0_usize;
    let mut samples = 0_usize;
    let mut separable = 0_usize;

    for step in -20..=120_i32 {
        let value = cc8_as_f32_local(step) / 20.0;
        // A chromatic triple as well as a neutral one, so the primaries
        // conversion inside the HDR lane's encode is exercised rather than
        // collapsing onto the neutral axis.
        for working in [[value; 3], [value, value * 0.4, value * 0.9]] {
            samples += 1;
            let tone_mapped = cc8_preview_tone_map_rgb(working);
            if tone_mapped != working {
                moved += 1;
            }
            let rgba = [working[0], working[1], working[2], 1.0];

            // The HDR lane: §3.3's delivery line composed here from the
            // authority module alone — no tone map anywhere in it.
            let expected_hdr = encode_delivery_for_lane(DeliveryLane::HdrHlgRec2020, working);
            let expected_hdr = [
                cc8_quantize_delivery16(expected_hdr[0]),
                cc8_quantize_delivery16(expected_hdr[1]),
                cc8_quantize_delivery16(expected_hdr[2]),
                cc8_quantize_delivery16(1.0),
            ];
            assert_eq!(
                encode_delivery_hlg_rec2020_rgba16(rgba),
                expected_hdr,
                "the HDR delivery encode of {working:?} is not the untone-mapped composition",
            );
            assert_eq!(
                encode_delivery_for_description(rgba, &hdr_delivery).expect("§5.1's lane"),
                expected_hdr,
                "the lane-selected delivery encode of {working:?} took a preview stage",
            );

            // The SDR lane, for the same reason and with the same shape.
            let expected_sdr = encode_delivery_for_lane(DeliveryLane::SdrRec709, working);
            let expected_sdr = [
                cc8_quantize_delivery16(expected_sdr[0]),
                cc8_quantize_delivery16(expected_sdr[1]),
                cc8_quantize_delivery16(expected_sdr[2]),
                cc8_quantize_delivery16(1.0),
            ];
            assert_eq!(encode_delivery_rgba16(rgba), expected_sdr);
            assert_eq!(
                encode_delivery_for_description(rgba, &sdr_delivery).expect("the SDR lane"),
                expected_sdr,
            );

            // And the distinction is real: a delivery encode that *had* taken
            // the preview would land somewhere else. Asserted only where the
            // delivery quantizer can *show* a difference — a wholly negative
            // triple lands on the clamp's floor from both, which is the single
            // clamp doing its job and not a blind spot this fixture invented.
            if tone_mapped != working && working.iter().all(|value| *value > 0.0) {
                let tone_mapped_rgba = [tone_mapped[0], tone_mapped[1], tone_mapped[2], 1.0];
                assert_ne!(
                    encode_delivery_hlg_rec2020_rgba16(tone_mapped_rgba),
                    expected_hdr,
                    "the HDR delivery encode cannot tell {working:?} from its tone-mapped form, \
                     so this fixture could not detect a preview stage in the delivery path",
                );
                separable += 1;
            }
        }
    }

    assert!(
        moved * 2 > samples,
        "the tone map moved only {moved} of {samples} samples; the equality above would hold \
         against a stage that did nothing",
    );
    assert!(
        separable * 4 > samples,
        "only {separable} of {samples} samples could distinguish a tone-mapped delivery encode \
         from an untone-mapped one, so the equality above proves too little",
    );

    // The two arms are named things, and only one of them names §4's stage —
    // so a delivery path cannot acquire it by asking for a "preview" it has no
    // parameter for.
    assert_eq!(MonitorPreview::Direct.stage(), None);
    assert_eq!(
        MonitorPreview::Cc8ToneMappedHdr.stage(),
        Some(CC8_PREVIEW_STAGE)
    );
    assert!(!MonitorPreview::Direct.is_tone_mapped());
    assert!(MonitorPreview::Cc8ToneMappedHdr.is_tone_mapped());

    println!(
        "CC8_MEASURED fixture=9 clause=delivery_unreachable samples={samples} moved={moved} \
         separable={separable} lanes=\"{} {}\"",
        DeliveryLane::SdrRec709.as_str(),
        DeliveryLane::HdrHlgRec2020.as_str(),
    );
}

/// The SDR-unchanged evidence within §10 step 8's reach, at the monitoring
/// boundary.
///
/// §9.1 fixture 6 — the byte-equality gate — is §10 step 5's and stands
/// unmoved; this is the one thing step 8 can prove about the *monitor* path,
/// which fixture 6 does not reach because fixture 6 gates exported bytes.
///
/// Step 8 touched exactly three shared monitor-path functions, and each has its
/// argument asserted here:
///
/// 1. **`color_pipeline::encode_monitor_rgba8_for_preview`** and its RGB
///    sibling are *new*; the `Direct` arm calls
///    `encode_monitor_rgba8_for_description` verbatim, so the first two claims
///    below are that the new entry point is the old one on that arm, sample for
///    sample, including the negatives and over-range values CC1 §6.2 bands.
/// 2. **`compositor::Compositor::readback_for`** gained a `preview` argument
///    and now calls the preview form. Every existing caller reaches it through
///    `render_monitor_with_luts`, which passes `MonitorPreview::Direct`, so the
///    third claim renders the same layers both ways and asserts byte equality.
/// 3. **`render::FrameRenderer::render`** now selects the arm from the
///    document. The fourth claim is that an SDR document selects `Direct`, so
///    no SDR project can reach §4's stage at all.
///
/// `encode_monitor_rgb8`, `encode_monitor_rgba8`, `encode_monitor_for_description`
/// and `encode_monitor_rgba8_for_description` are **unchanged, character for
/// character**; they are the functions every CC1–CC7 fixture measures, and step
/// 8 added callers rather than editing them.
#[test]
fn cc8_sdr_monitor_transform_is_unmoved_by_the_preview_arm() {
    let monitoring = cc8_preview_monitoring();
    let mut compared = 0_usize;
    for step in -40..=160_i32 {
        let value = cc8_as_f32_local(step) / 40.0;
        let rgba = [value, value * 0.5, value * 1.5 - 0.25, 0.75];
        let rgb = [rgba[0], rgba[1], rgba[2]];
        compared += 1;
        assert_eq!(
            encode_monitor_rgba8_for_preview(rgba, &monitoring, MonitorPreview::Direct)
                .expect("CC1's monitor target"),
            encode_monitor_rgba8_for_description(rgba, &monitoring).expect("CC1's monitor target"),
            "the Direct arm moved the RGBA monitor encode at {rgba:?}",
        );
        assert_eq!(
            encode_monitor_for_preview(rgb, &monitoring, MonitorPreview::Direct)
                .expect("CC1's monitor target"),
            crate::color_pipeline::encode_monitor_rgb8(rgb),
            "the Direct arm moved the RGB monitor encode at {rgb:?}",
        );
    }
    assert!(compared > 0);

    // Claim 3: the production compositor, both ways, on a raster with the
    // negatives and over-range values that make the comparison non-trivial.
    let width = 16_u32;
    let height = 4_u32;
    let rgb: Vec<[f32; 3]> = (0..width * height)
        .map(|index| {
            let position = cc8_as_f32_local(i32::try_from(index).expect("raster index"));
            let value =
                position / cc8_as_f32_local(i32::try_from(width * height).unwrap()) * 3.0 - 0.25;
            [value, value * 0.6, value * 1.2]
        })
        .collect();
    let frame = working_frame(width, height, &rgb);
    let gpu = fallback_gpu();
    let compositor = Compositor::new(gpu.context());
    let layers = [CompositorLayer {
        frame: &frame,
        effects: &[],
        transition: TransitionRenderParams::default(),
    }];
    let unchanged = compositor
        .render_monitor_with_luts((width, height), &layers, &monitoring, None)
        .expect("CC1's monitor readback");
    let direct = compositor
        .render_monitor_preview_with_luts(
            (width, height),
            &layers,
            &monitoring,
            MonitorPreview::Direct,
            None,
        )
        .expect("the Direct preview arm");
    assert_eq!(
        unchanged.rgba.as_ref(),
        direct.rgba.as_ref(),
        "render_monitor_with_luts must be render_monitor_preview_with_luts on the Direct arm",
    );
    let tone_mapped = compositor
        .render_monitor_preview_with_luts(
            (width, height),
            &layers,
            &monitoring,
            MonitorPreview::Cc8ToneMappedHdr,
            None,
        )
        .expect("the tone-mapped preview arm");
    assert_ne!(
        tone_mapped.rgba.as_ref(),
        direct.rgba.as_ref(),
        "the two arms must differ on a raster with HDR magnitudes, or the equality above is \
         evidence about nothing",
    );

    // Claim 4: an SDR document selects the arm that cannot reach §4's stage,
    // and an HDR one selects the arm that does — through core's one classifier.
    let sdr_document = single_clip_document(cc8_qc_asset(
        "cc8-step8-sdr-source",
        cc8_sdr_delivery_description(DeliveryEncodeDepth::Ten),
    ));
    assert_eq!(
        document_monitor_preview(&sdr_document),
        MonitorPreview::Direct,
    );
    let hdr_document = single_clip_document(cc8_qc_asset(
        "cc8-step8-hlg-source",
        cc8_hlg_source_description(&cc8_hdr_delivery_description()),
    ));
    assert_eq!(
        document_monitor_preview(&hdr_document),
        MonitorPreview::Cc8ToneMappedHdr,
    );

    println!(
        "CC8_MEASURED fixture=9 clause=sdr_monitor_unmoved lane={} samples={compared} \
         raster={width}x{height} direct_bytes_equal=true arms_differ=true",
        gpu.lane.id(),
    );
}

/// §9.1 fixture 9's measured **preview parity** figures, in monitor codes:
/// `[max, p99, mean]`.
///
/// §9.2's row is "Preview parity | max / P99 / mean, monitor codes", and these
/// are the numbers this fixture takes, not numbers chosen for it. The fixture's
/// doc comment carries the population, the adapter, and the date beside them.
const CC8_FIXTURE9_PARITY_MEASURED: Cc8MeasuredBand = [1.0, 0.0, 2.604_167e-3];

/// The floor the **P99** preview-parity term is bounded by (§9.2's second
/// rule), because it is the one term that measured exactly zero.
///
/// One 8-bit monitor code. `cc8_next_power_of_two_bound` cannot bound a term
/// that measured zero, and a zero bound is not a gate — it would fail on the
/// first adapter that rounded one sample differently while measuring nothing
/// about the pipeline. The storage format's own granularity here is one code,
/// which is a derived number rather than a chosen one: the same argument
/// `cc8_half_float_ulp` makes for the linear bands, at the width the monitor
/// buffer actually stores.
///
/// §9.2's rule does not stop at the floor — "where a term measures zero on the
/// passing source, a **deliberately starved fixture** bounds the constant from
/// above" — so `cc8_preview_parity_starved_p99` below exceeds it, which is what
/// makes this bound reachable rather than decorative. The `max` and `mean`
/// terms measured non-zero and pass `0.0`, so the floor never binds for them.
const CC8_PREVIEW_PARITY_CODE_FLOOR: f32 = 1.0;

/// The starved control's misreading of §4's pinned peak: **ten percent**.
///
/// Deliberately not a gross starve — replacing the stage entirely would exceed
/// any bound and say nothing about how tight this one is — and deliberately not
/// smaller, because a smaller one does not reach the floor on this raster and
/// the measurement says so rather than the constant hiding it: at 1 % the
/// starved P99 is **0 monitor codes**, because §4's curve differs from itself
/// at two nearby peaks only through the `x / W²` term, which is `≈ x / 24` in
/// working units and therefore sub-code below diffuse white, where most of this
/// raster sits after the parity correction's -1.5 stops.
///
/// So what this control establishes is exactly what §9.2 asks and no more: the
/// one-code floor is **reachable** — a 10 % misreading of the pinned peak
/// produces a P99 of 2 codes and fails the gate — while a 1 % one is genuinely
/// below the monitor buffer's own granularity on this raster and is not a
/// difference the 8-bit display boundary can carry.
const CC8_PREVIEW_STARVED_PEAK_PERCENT: i32 = 110;

/// §4's curve at an arbitrary peak, for the starved control only.
///
/// This is the one place CC8 writes the tone map's arithmetic outside
/// `kinewright_core::cc8_preview_tone_map`, and it is deliberate: a starved
/// control has to evaluate a curve the production stage does **not** have.
/// It is never used as a reference for a passing measurement.
fn cc8_starved_tone_map(value: f32, peak: f32) -> f32 {
    let magnitude = value.abs();
    let mapped = magnitude * (1.0 + magnitude / (peak * peak)) / (1.0 + magnitude);
    if value < 0.0 { -mapped } else { mapped }
}

/// §9.1 fixture 9, clause 2 — **CPU/GPU parity of the tone-mapping stage**, in
/// monitor codes.
///
/// §4 item 4 requires it and §9.2 fixes its shape. What is compared:
///
/// * the **production** path — `Compositor::render_monitor_preview_with_luts`
///   on the software GPU lane, which composites and grades in WGSL, reads the
///   `Rgba16Float` surface back, and applies §4's stage and CC1's display
///   encode to every mapped pixel; against
/// * a **CPU reference** of the same raster and the same correction —
///   `color_pipeline::apply_primary_corrections`, the `f16` storage rounding
///   CC1 §6.2 names, then `encode_monitor_rgba8_for_preview` on the same arm.
///
/// The two sides therefore differ only in the *grading* node, which is exactly
/// CC1 §6.2's comparison seen through §4's stage — and that is the honest claim
/// available here, because the tone map is CPU-only on both sides (see this
/// section's header). The measurement is still worth taking rather than
/// asserting by construction: §4's curve is applied to values the GPU produced,
/// and it *amplifies* small differences below diffuse white, where its slope is
/// steepest, so a shader regression that CC1's monitor gate tolerated could
/// show here first.
///
/// **The gate is this fixture's own measurement**, bounded by the step-3 rule
/// — `next_power_of_two(recorded) * CC8_MEASURED_BOUND_HEADROOM`, computed from
/// the recorded figure and never from the live one. CC1 §6.2's monitor
/// constants are asserted afterwards as a **not-worse cross-check**, not as an
/// inherited budget: §9.2 forbids inheriting a number from another lane, and
/// what the cross-check claims is that the preview arm has not made the monitor
/// path worse on CC1's own terms, which is the SDR-adjacent evidence within
/// this fixture's reach.
///
/// The raster is `cc8_hdr_parity_raster`'s — the decoded HDR bars from real
/// media, where the out-of-Rec.709 negatives and wide-gamut chromaticities
/// come from, plus a dense HLG signal ramp — and the correction is
/// `cc8_parity_correction`, every control non-neutral. Both are §9.1 fixture
/// 10's, deliberately: the two fixtures measure the same production surface at
/// two different boundaries, and using one raster keeps them comparable.
#[test]
#[allow(clippy::too_many_lines)]
fn cc8_preview_cpu_gpu_parity_in_monitor_codes() {
    let (_directory, _description, decoded) = cc8_hdr_bar_source("cc8-step8-preview-parity");
    let (width, height, rgb) = cc8_hdr_parity_raster(&decoded);
    let frame = working_frame(width, height, &rgb);
    let correction = cc8_parity_correction();
    let monitoring = cc8_preview_monitoring();
    let preview = MonitorPreview::Cc8ToneMappedHdr;

    let gpu = fallback_gpu();
    let compositor = Compositor::new(gpu.context());
    let layers = [CompositorLayer {
        frame: &frame,
        effects: &[cc8_correction_effect(1, correction)],
        transition: TransitionRenderParams::default(),
    }];
    let actual = compositor
        .render_monitor_preview_with_luts((width, height), &layers, &monitoring, preview, None)
        .expect("production GPU preview readback");
    // §4 item 4's determinism clause at the production boundary: the same
    // layers rendered twice produce the same bytes.
    let repeat = compositor
        .render_monitor_preview_with_luts((width, height), &layers, &monitoring, preview, None)
        .expect("production GPU preview readback, repeated");
    assert_eq!(
        actual.rgba.as_ref(),
        repeat.rgba.as_ref(),
        "the production preview render is not deterministic",
    );

    let mut expected = Vec::with_capacity(rgb.len() * 4);
    let mut starved = Vec::with_capacity(rgb.len() * 4);
    let starved_peak = cc8_preview_peak_working_linear()
        * cc8_as_f32_local(CC8_PREVIEW_STARVED_PEAK_PERCENT)
        / 100.0;
    let mut moved_samples = 0_usize;
    for source in &rgb {
        let corrected = correction
            .apply_checked(*source)
            .expect("the CC8 parity correction");
        let quantized = corrected.map(|value| f16::from_f32(value).to_f32());
        if quantized
            .iter()
            .zip(source)
            .any(|(corrected, source)| (corrected - source).abs() > 1.0e-3)
        {
            moved_samples += 1;
        }
        let alpha = f16::from_f32(1.0).to_f32();
        expected.extend_from_slice(
            &encode_monitor_rgba8_for_preview(
                [quantized[0], quantized[1], quantized[2], alpha],
                &monitoring,
                preview,
            )
            .expect("CC1's monitor target"),
        );
        // The starved reference: the same composition with §4's peak misread
        // by `CC8_PREVIEW_STARVED_PEAK_PERCENT`.
        let starved_rgb = quantized.map(|value| cc8_starved_tone_map(value, starved_peak));
        starved.extend_from_slice(
            &encode_monitor_rgba8_for_preview(
                [starved_rgb[0], starved_rgb[1], starved_rgb[2], alpha],
                &monitoring,
                MonitorPreview::Direct,
            )
            .expect("CC1's monitor target"),
        );
    }
    // Non-vacuity, CC1's `MIN_CHANGED_LINEAR_BASIS_POINTS` rule: a correction
    // that moved nothing would report a flattering zero.
    assert!(
        moved_samples * 2 > rgb.len(),
        "the CC8 parity correction moved only {moved_samples} of {} pixels; a proven no-op \
         cannot measure parity",
        rgb.len(),
    );
    // And the arm is live: a raster whose tone-mapped codes equalled its
    // untone-mapped ones would measure CC1's gate under a new name.
    let direct: Vec<u8> = rgb
        .iter()
        .flat_map(|source| {
            let corrected = correction
                .apply_checked(*source)
                .expect("the CC8 parity correction")
                .map(|value| f16::from_f32(value).to_f32());
            encode_monitor_rgba8_for_preview(
                [
                    corrected[0],
                    corrected[1],
                    corrected[2],
                    f16::from_f32(1.0).to_f32(),
                ],
                &monitoring,
                MonitorPreview::Direct,
            )
            .expect("CC1's monitor target")
        })
        .collect();
    assert_ne!(direct, expected, "the tone-mapped arm did nothing");

    let metric = abs_code_diff_rgb(actual.rgba.as_ref(), &expected);
    #[allow(clippy::cast_possible_truncation)]
    let terms = [
        ("max", f32::from(metric.max)),
        ("p99", metric.p99 as f32),
        ("mean", metric.mean as f32),
    ];
    for (index, (term, value)) in terms.into_iter().enumerate() {
        // Only the zero term takes the floor; the other two are bounded by
        // their own recorded figures, as `cc8_assert_measured` requires.
        let floor = if CC8_FIXTURE9_PARITY_MEASURED[index] > 0.0 {
            0.0
        } else {
            CC8_PREVIEW_PARITY_CODE_FLOOR
        };
        cc8_assert_measured(
            "9",
            "Preview parity",
            term,
            value,
            CC8_FIXTURE9_PARITY_MEASURED[index],
            floor,
        );
    }

    // §9.2's second rule for the zero term: the starved control exceeds the
    // floor, so a bound of one monitor code is reachable and a misread peak is
    // caught rather than tolerated.
    let starved_metric = abs_code_diff_rgb(actual.rgba.as_ref(), &starved);
    #[allow(clippy::cast_possible_truncation)]
    let starved_p99 = starved_metric.p99 as f32;
    assert!(
        starved_p99 > CC8_PREVIEW_PARITY_CODE_FLOOR,
        "a {CC8_PREVIEW_STARVED_PEAK_PERCENT}% misreading of CC8 §4's pinned peak produced a P99 \
         of {starved_p99} monitor codes, inside the {CC8_PREVIEW_PARITY_CODE_FLOOR}-code floor \
         the zero term is bounded by; the floor would then be a bound nothing can reach",
    );

    // The CC1 §6.2 monitor cross-check: not a gate inherited from another lane,
    // a claim that the monitor path has not become worse while gaining §4's
    // stage.
    assert!(
        metric.max <= MONITOR_CPU_GPU_MAX
            && metric.p99 <= MONITOR_CPU_GPU_P99
            && metric.mean <= MONITOR_CPU_GPU_MEAN,
        "the CC1 §6.2 monitor gate no longer holds through the preview arm: {metric:?}",
    );

    println!(
        "CC8_MEASURED fixture=9 clause=cpu_gpu_parity lane={} raster={width}x{height} \
         moved_samples={moved_samples} max={} p99={:.6} mean={:.6} \
         starved_peak_percent={CC8_PREVIEW_STARVED_PEAK_PERCENT} starved_max={} \
         starved_p99={:.6} \
         cc1_monitor_gate=\"max {MONITOR_CPU_GPU_MAX} p99 {MONITOR_CPU_GPU_P99} mean \
         {MONITOR_CPU_GPU_MEAN}\"",
        gpu.lane.id(),
        metric.max,
        metric.p99,
        metric.mean,
        starved_metric.max,
        starved_metric.p99,
    );
}
