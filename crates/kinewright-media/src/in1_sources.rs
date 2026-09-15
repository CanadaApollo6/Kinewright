//! IN1 §3: the four source generators for the colour case.
//!
//! **This module is test support.** It shells out to the provisioned `FFmpeg`
//! CLI through [`crate::test_support::run_ffmpeg`], which **panics** when the
//! binary is missing and when it reports a nonzero exit, so nothing in
//! production may reach for it. It is `pub` rather than `cfg(test)` for the
//! reason `cc7_sources` records in its own module doc and IN1 §3 rule 15
//! restates: the agent's `tests/mcp_server.rs` needs these fixtures across a
//! crate boundary, and a `cfg(test)` module is invisible there.
//!
//! It is the **one** generator for the colour case: the media fixtures
//! (`in1_fixtures.rs`), the app's router test and the agent's scripted tests
//! all call it, so the probed tuple cannot drift between the three claims made
//! about it.
//!
//! # The geometry, and why it costs nothing new
//!
//! IN1 §3 rule 1 reuses CC7's and AU6's raster and rate verbatim — 25 fps,
//! 320×180, `yuv420p`, 50 frames, two seconds — so the fixture set adds no new
//! geometry to the workspace. The picture is `testsrc`; no IN1 claim reads a
//! pixel value (§3 rule 2), so the raster content is not pinned and the
//! generators author nothing in Rust.
//!
//! # Why the tags go through `setparams` and not through `-color_*`
//!
//! On the pinned `FFmpeg` 8 (`third_party/ffmpeg/bin/ffmpeg`,
//! `n8.0-23-gd1f31a829d-20251022`, `libavcodec 62.11.100`) the *output-option*
//! form `-color_primaries bt709 -color_trc bt709` is **silently ignored** and
//! `ffprobe` reports `unknown`: frame properties from the filter chain win
//! over the encoder options. The tagged twins therefore take the filter in
//! `cc7_sources`' spelling (`cc7_sources.rs:540`), `range=limited` rather than
//! `range=tv` (IN1 §3 rule 4). The untagged pair passes the four `unspecified`
//! output options and **no** `setparams` filter — which is exactly the case
//! where the output options are not ignored, because there is nothing in the
//! filter chain to override them.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, OnceLock},
};

use crate::test_support::GeneratedMedia;

/// IN1 §3 rule 1: the fixture frame rate, CC7's and AU6's verbatim.
pub const IN1_SOURCE_FPS: u32 = 25;
/// IN1 §3 rule 1: the fixture raster width, CC7's and AU6's verbatim.
pub const IN1_SOURCE_WIDTH: u32 = 320;
/// IN1 §3 rule 1: the fixture raster height, CC7's and AU6's verbatim.
pub const IN1_SOURCE_HEIGHT: u32 = 180;
/// IN1 §3 rule 1: 50 frames, which is two seconds at [`IN1_SOURCE_FPS`].
pub const IN1_SOURCE_FRAMES: u32 = 50;
/// IN1 §3 rule 1: the fixture pixel format, CC7's and AU6's verbatim.
pub const IN1_SOURCE_PIXEL_FORMAT: &str = "yuv420p";

const _: () = assert!(
    IN1_SOURCE_FRAMES.is_multiple_of(IN1_SOURCE_FPS),
    "`testsrc`'s `duration=` is written in whole seconds, so the frame count \
     must divide the rate"
);

/// The confidence `in1_untagged.mp4` probes at: nothing in the stream is
/// tagged, so `color_description_from_decoder` reports the bit depth alone and
/// falls back to `ColorProvenance::Inferred` (IN1 §3 rules 5 and 6).
pub const IN1_UNTAGGED_MP4_CONFIDENCE_BASIS_POINTS: u16 = 2_000;

/// The confidence `in1_untagged_vp9.webm` probes at, which is **higher** than
/// its MP4 twin's for a container reason rather than a codec one.
///
/// The untagged `WebM` reports `range=Limited` even though `unspecified` was
/// requested, because the Matroska/WebM *muxer* always writes a `Colour/Range`
/// element: a Matroska-family container is never fully untagged, whatever the
/// codec. One known field is enough for the probe to report
/// `ColorProvenance::StreamMetadata` rather than `Inferred`, which is what
/// lifts the confidence from [`IN1_UNTAGGED_MP4_CONFIDENCE_BASIS_POINTS`] to
/// this value (IN1 §3 rule 6, N2.5/muxer).
pub const IN1_UNTAGGED_WEBM_CONFIDENCE_BASIS_POINTS: u16 = 4_000;

/// IN1 §3 rule 4's tagging filter, in `cc7_sources.rs:540`'s spelling.
const IN1_TAGGED_FILTER: &str =
    "setparams=range=limited:color_primaries=bt709:color_trc=bt709:colorspace=bt709";

/// IN1 §3 rule 4's untagging output options. There is no filter to override
/// them, so unlike the tagged recipe these do take effect.
const IN1_UNTAGGED_OPTIONS: [&str; 8] = [
    "-color_primaries",
    "unspecified",
    "-color_trc",
    "unspecified",
    "-colorspace",
    "unspecified",
    "-color_range",
    "unspecified",
];

/// Which of IN1 §3 rule 3's four fixtures a generator writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum In1Source {
    /// `in1_untagged.mp4` — `libx264`, tags stripped. The both-OS CI gate.
    UntaggedMp4,
    /// `in1_tagged.mp4` — `libx264`, `setparams` BT.709. Proves the negative.
    TaggedMp4,
    /// `in1_untagged_vp9.webm` — `libvpx-vp9`, tags stripped.
    UntaggedWebm,
    /// `in1_tagged_vp9.webm` — `libvpx-vp9`, `setparams` BT.709.
    TaggedWebm,
}

impl In1Source {
    /// The fixture's stem, which is also its `GeneratedMedia` label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::UntaggedMp4 => "in1_untagged",
            Self::TaggedMp4 => "in1_tagged",
            Self::UntaggedWebm => "in1_untagged_vp9",
            Self::TaggedWebm => "in1_tagged_vp9",
        }
    }

    /// The fixture's container extension.
    #[must_use]
    pub const fn extension(self) -> &'static str {
        match self {
            Self::UntaggedMp4 | Self::TaggedMp4 => "mp4",
            Self::UntaggedWebm | Self::TaggedWebm => "webm",
        }
    }

    /// The fixture's file name, as IN1 §3 rule 3's table spells it.
    #[must_use]
    pub fn file_name(self) -> String {
        format!("{}.{}", self.label(), self.extension())
    }

    /// Whether the recipe carries [`IN1_TAGGED_FILTER`].
    const fn is_tagged(self) -> bool {
        matches!(self, Self::TaggedMp4 | Self::TaggedWebm)
    }

    /// The encoder arguments for this fixture's container.
    const fn encoder(self) -> [&'static str; 4] {
        match self {
            Self::UntaggedMp4 | Self::TaggedMp4 => ["-c:v", "libx264", "-preset", "veryfast"],
            Self::UntaggedWebm | Self::TaggedWebm => ["-c:v", "libvpx-vp9", "-b:v", "200k"],
        }
    }
}

/// One encoded payload per fixture, encoded on the first miss.
///
/// The `WebM` pair costs 0.51 s of the set's 0.57 s (IN1 §3 rule 16), so a suite
/// that touches a fixture from several tests pays for it once.
static IN1_SOURCE_BYTES: OnceLock<Mutex<HashMap<In1Source, Arc<[u8]>>>> = OnceLock::new();

/// IN1 §3 rule 3: one of the four fixtures, written to its own temp file.
///
/// The bytes are interned per process in the shape `cc7_sources` uses
/// (`cc7_sources.rs:503-515`) and re-written to a fresh [`GeneratedMedia`] per
/// call, so every caller still owns a `Drop` guard that removes its own file
/// (IN1 §3 rule 11).
///
/// # Panics
///
/// Panics when the provisioned `FFmpeg` CLI is missing or reports a nonzero
/// exit, exactly as [`crate::test_support::run_ffmpeg`] does.
#[must_use]
pub fn in1_source(source: In1Source) -> GeneratedMedia {
    GeneratedMedia::from_bytes(
        source.label(),
        source.extension(),
        &in1_source_bytes(source),
    )
}

/// IN1 §3 rule 3: `in1_untagged.mp4`, the both-OS CI gate.
///
/// # Panics
///
/// Panics as [`in1_source`] does.
#[must_use]
pub fn in1_untagged_mp4() -> GeneratedMedia {
    in1_source(In1Source::UntaggedMp4)
}

/// IN1 §3 rule 3: `in1_tagged.mp4`, the BT.709-tagged twin that proves the
/// negative.
///
/// # Panics
///
/// Panics as [`in1_source`] does.
#[must_use]
pub fn in1_tagged_mp4() -> GeneratedMedia {
    in1_source(In1Source::TaggedMp4)
}

/// IN1 §3 rule 3: `in1_untagged_vp9.webm`, the Helen Hill row.
///
/// # Panics
///
/// Panics as [`in1_source`] does.
#[must_use]
pub fn in1_untagged_webm() -> GeneratedMedia {
    in1_source(In1Source::UntaggedWebm)
}

/// IN1 §3 rule 3: `in1_tagged_vp9.webm`, the BT.709-tagged twin that proves
/// the negative in a Matroska-family container.
///
/// # Panics
///
/// Panics as [`in1_source`] does.
#[must_use]
pub fn in1_tagged_webm() -> GeneratedMedia {
    in1_source(In1Source::TaggedWebm)
}

/// The interned payload for `source`, encoded on the first miss.
fn in1_source_bytes(source: In1Source) -> Arc<[u8]> {
    let cache = IN1_SOURCE_BYTES.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = cache
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(hit) = guard.get(&source) {
        return Arc::clone(hit);
    }
    let bytes: Arc<[u8]> = encode_in1_source(source).into();
    guard.insert(source, Arc::clone(&bytes));
    bytes
}

/// One `FFmpeg` pass over IN1 §3 rule 4's recipe. The file is read and dropped.
fn encode_in1_source(source: In1Source) -> Vec<u8> {
    let picture = format!(
        "testsrc=size={IN1_SOURCE_WIDTH}x{IN1_SOURCE_HEIGHT}:rate={IN1_SOURCE_FPS}:duration={}",
        IN1_SOURCE_FRAMES / IN1_SOURCE_FPS
    );
    let mut arguments = vec!["-f", "lavfi", "-i", picture.as_str()];
    if source.is_tagged() {
        arguments.extend_from_slice(&["-vf", IN1_TAGGED_FILTER]);
    }
    arguments.extend_from_slice(&["-pix_fmt", IN1_SOURCE_PIXEL_FORMAT]);
    arguments.extend_from_slice(&source.encoder());
    if !source.is_tagged() {
        arguments.extend_from_slice(&IN1_UNTAGGED_OPTIONS);
    }
    let encoded = GeneratedMedia::ffmpeg(source.label(), &arguments, source.extension());
    std::fs::read(encoded.path()).unwrap_or_else(|error| {
        panic!(
            "the interned IN1 encode for {} must be readable: {error}",
            source.file_name()
        )
    })
}
