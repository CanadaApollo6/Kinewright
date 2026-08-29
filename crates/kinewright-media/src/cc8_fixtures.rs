//! CC8 fixtures: §10 step 1's encoder precondition, and §10 step 3's
//! §9.1 fixtures 1, 2 and 5.
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

use std::path::Path;

use ffmpeg_next as ffmpeg;
use half::f16;
use kinewright_core::{
    AssetId, CC8_HLG_NOMINAL_PEAK_NITS, CC8_HLG_REFERENCE_WHITE_SIGNAL_PERCENT,
    CC8_HLG_SCENE_BREAKPOINT, CC8_HLG_SIGNAL_BREAKPOINT, CC8_PQ_C1, CC8_PQ_C2, CC8_PQ_C3,
    CC8_PQ_M2, CC8_PQ_PEAK_NITS, CC8_REFERENCE_WHITE_NITS, CC8_REJECTED_HDR_ADJACENT,
    CC8_SOURCE_PROFILES, ColorBitDepth, ColorDescription, ColorMatrix, ColorPrimaries,
    ColorProvenance, ColorRange, ColorSourceError, ColorSourceProfile,
    ColorSourceProfileAssumption, ColorTransfer, ColorWhitePoint, DeliveryColorError,
    DeliveryColorMismatch, DeliveryEncodeDepth, DeliveryProfile, HDR_SOURCE_ON_SDR_DELIVERY,
    QaSeverity, Rational, cc8_hlg_decode_working_linear, cc8_hlg_encode_working_linear,
    cc8_hlg_inverse_oetf, cc8_hlg_oetf, cc8_pq_decode_working_linear, cc8_pq_encode_working_linear,
    cc8_pq_eotf_nits, cc8_pq_inverse_eotf, classify_source, classify_source_with_assumption,
    delivery_conformance,
};

use crate::{
    color_pipeline::decode_hdr_source_working_linear,
    decode::{VideoDecoder, probe_path},
    initialize_ffmpeg,
    test_support::{TempDirectory, single_clip_document},
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
    let x264_params = format!(
        "{CC8_PRECONDITION_PRIMARIES_PARAM}:{transfer_param}:{CC8_PRECONDITION_MATRIX_PARAM}"
    );

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
        fill_ramp(&mut frame, index);
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
// primaries conversion is §10 step 4 (fixture 3), the delivery lane is step 6
// (fixtures 7, 8), and `decode::hdr_managed_decode_not_yet_available` keeps the
// production frame path from compositing Rec.2020 values as BT.709 in the
// meantime.

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
/// further powers of two." Every term here goes through `powf`, `exp`, or
/// `ln`, and CC8 runs on two operating systems with different libm
/// implementations, so the same allowance applies with the same reason.
///
/// It is applied on top of [`cc8_next_power_of_two_bound`], so the realised
/// margin over the recorded figures is between 4x and 8x — a real margin
/// (§9.2's second rule), and still three or more orders of magnitude tighter
/// than a mis-transcribed transfer constant or a dropped stage would move the
/// round trip.
const CC8_FIXTURE1_BOUND_HEADROOM: f32 = 4.0;

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
                    cc8_next_power_of_two_bound(recorded[term_index]) * CC8_FIXTURE1_BOUND_HEADROOM;
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
/// §0.2 Q6 makes permanent, not scaffolding — and the managed *frame* decoder
/// refuses it with §10 step 3's boundary reason until step 4 lands §3.3's
/// primaries conversion.
fn cc8_assert_classified_hdr_source_blocks_export_and_frame_decode(
    asset: &kinewright_core::MediaAsset,
    complete: &ColorDescription,
    path: &Path,
    fps: Rational,
    assumption: Option<ColorSourceProfileAssumption>,
) {
    // The other half of §1's rule, and §7 item 2's: a source that *is* one of
    // §2.1's profiles is not silently treated as Rec.709 either. It blocks
    // export on the HDR/SDR delivery mismatch, and the managed frame decoder
    // refuses it with the §10 step 4 boundary reason.
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
    let Err(hdr_decode_error) =
        VideoDecoder::open_scaled_managed(path, fps, None, complete, assumption)
    else {
        panic!("§10 step 3 does not yet decode an HDR frame")
    };
    assert!(
        hdr_decode_error
            .to_string()
            .contains("managed HDR frame decode is not available yet"),
        "{hdr_decode_error}",
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
