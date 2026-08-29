//! CC8 §10 step 1 — the encoder precondition for the HDR delivery lane.
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
use kinewright_core::{
    AssetId, ColorBitDepth, ColorDescription, ColorMatrix, ColorPrimaries, ColorProvenance,
    ColorRange, ColorTransfer, DeliveryColorError, DeliveryColorMismatch, DeliveryEncodeDepth,
};

use crate::{decode::probe_path, initialize_ffmpeg, test_support::TempDirectory};

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
