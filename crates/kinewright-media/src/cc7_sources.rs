//! CC7 §3: the source generators for the six named colour workflows.
//!
//! **This module is test support.** It shells out to the provisioned `FFmpeg`
//! CLI through [`crate::test_support::run_ffmpeg`], which **panics** when the
//! binary is missing and when it reports a nonzero exit
//! (`test_support.rs:274-298`), so nothing in production may reach for it. It
//! is `pub` rather than `cfg(test)` because the agent's `tests/mcp_server.rs`
//! and the eval binary both need it and a `cfg(test)` module is invisible
//! across a crate boundary (A11).
//!
//! It is the **one** generator: the media fixtures, the agent end-to-end
//! tests, and `color-workflow-v6`'s fixture builders all call it, so a raster
//! cannot drift between the three claims made about it.
//!
//! # Rule 11.0.1's source-content exemption
//!
//! Every raster here is authored in Rust (idiom A) from
//! [`kinewright_core::cc7_scenarios`]'s analytic tables, and the `Y'CbCr`
//! conversion uses an **independently transcribed** limited-range BT.709
//! forward matrix rather than `bt709_limited_ycbcr`. CC7 §11.0.1 forbids
//! obtaining an *expected value* that way and explicitly permits *authoring
//! source content* that way (minor 8): nothing in this module is ever compared
//! against the output of a function it called to build the picture.
//!
//! `lavfi` authors nothing that carries an expectation (CC7 §3.2): `geq` and
//! `lutrgb` floor rather than round, and 8→16-bit promotion through swscale is
//! not `×257` (CC6 measured `32 790`), so every CC7 raster is idiom A.

use std::{
    fmt::Write as _,
    path::{Path, PathBuf},
};

use kinewright_core::cc7_scenarios::{
    CC7_CHART_BAND_RECT, CC7_CHART_PATCH_WIDTH, CC7_CHART_PATCHES, CC7_LOG_CUBE_SIZE,
    CC7_PRIMARY_BAND_RECT, CC7_PRIMARY_PATCHES, CC7_RAMP_RECT, CC7_ROW_BAND_RECT,
    CC7_ROW_PATCH_WIDTH, CC7_ROW_PATCHES, CC7_SKIN_BAND_RECT, CC7_SOURCE_FPS, CC7_SOURCE_FRAMES,
    CC7_SOURCE_HEIGHT, CC7_SOURCE_WIDTH, CC7_SURROUND_CODE, CC7_SURROUND_GRADE709_MILLIONTHS,
    CC7_TRACK_FRAMES, CC7_TRACK_SQUARE_SIZE, CC7_TRACK_STATIC_PATCH_BOTTOM,
    CC7_TRACK_STATIC_PATCH_TOP, Cc7Camera, Cc7PixelRect, Cc7Scenario, cc7_analytic_square_top_left,
    cc7_as_f64, cc7_camera_code, cc7_decode_display709, cc7_display_code, cc7_encode_bt709,
    cc7_grade709_decode, cc7_log_value, cc7_ramp_code, cc7_square_is_drawn,
};

/// CC7 §3.4's `.cube` `TITLE`, **one constant**: core owns it (§2.7), writes it
/// into the `LutAsset` record (`cc7_scenarios.rs:1870`), and this module writes
/// the same bytes into the file's `TITLE` line, so the recorded title and the
/// file it names cannot drift.
pub use kinewright_core::cc7_scenarios::CC7_LOG_CUBE_TITLE;

use crate::test_support::GeneratedMedia;

// ===========================================================================
// CC7 §2.3.3 and §2.3.6: the rasters, authored pixel by pixel.
// ===========================================================================

/// Which CC7 raster a generator writes.
///
/// `Cc7Camera::LogLike` is **not** a camera here: it has no linear-light
/// transform (`cc7_scenarios::cc7_camera_transform` resolves it to the
/// identity) and its content is the log carrier, so [`Cc7SourceKind::camera`]
/// normalises `Camera(LogLike)` to [`Cc7SourceKind::Log`] and every method on
/// this type does the same before it answers. The two are one value, one
/// label, one raster and one encode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Cc7SourceKind {
    /// The 60-frame base scene as one of the four cameras sees it.
    ///
    /// Prefer [`Cc7SourceKind::camera`], which normalises the `LogLike` alias
    /// away; a directly constructed `Camera(LogLike)` behaves as `Log`.
    Camera(Cc7Camera),
    /// The 60-frame log-like carrier.
    Log,
    /// The 100-frame tracked scene.
    Tracked,
}

impl Cc7SourceKind {
    /// The kind that renders `camera`'s raster, with `LogLike` normalised to
    /// [`Cc7SourceKind::Log`].
    ///
    /// This is the only constructor CC7 uses for a camera, so no two
    /// `PartialEq`-distinct kinds ever name the same raster and a caller that
    /// de-duplicates through a `HashSet` builds each encode once.
    #[must_use]
    pub const fn camera(camera: Cc7Camera) -> Self {
        match camera {
            Cc7Camera::LogLike => Self::Log,
            other => Self::Camera(other),
        }
    }

    /// `self` with a directly constructed `Camera(LogLike)` folded into
    /// [`Cc7SourceKind::Log`].
    #[must_use]
    pub const fn normalized(self) -> Self {
        match self {
            Self::Camera(camera) => Self::camera(camera),
            other => other,
        }
    }

    /// The frame count this raster is written at.
    #[must_use]
    pub const fn frames(self) -> u32 {
        match self.normalized() {
            Self::Tracked => CC7_TRACK_FRAMES,
            Self::Camera(_) | Self::Log => CC7_SOURCE_FRAMES,
        }
    }

    /// The label the generated file is named after.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self.normalized() {
            Self::Camera(Cc7Camera::A) => "cc7-camera-a",
            Self::Camera(Cc7Camera::B) => "cc7-camera-b",
            Self::Camera(Cc7Camera::C1) => "cc7-camera-c1",
            Self::Camera(Cc7Camera::C2) => "cc7-camera-c2",
            // `normalized` has already folded `Camera(LogLike)` into `Log`.
            Self::Camera(Cc7Camera::LogLike) | Self::Log => "cc7-log-carrier",
            Self::Tracked => "cc7-tracked",
        }
    }

    /// Whether every frame of this raster is the same picture.
    ///
    /// Only the tracked scene moves (CC7 §11.1), so the other two are authored
    /// once and repeated.
    #[must_use]
    const fn is_static(self) -> bool {
        !matches!(self.normalized(), Self::Tracked)
    }
}

/// Which of CC7 §2.3.3's five named regions `(x, y)` falls in.
///
/// The geometry is core's: the four band rects and the two patch widths are
/// [`CC7_RAMP_RECT`], [`CC7_CHART_BAND_RECT`], [`CC7_PRIMARY_BAND_RECT`],
/// [`CC7_ROW_BAND_RECT`], [`CC7_CHART_PATCH_WIDTH`] and
/// [`CC7_ROW_PATCH_WIDTH`], so the module restates no band edge and no patch
/// width of its own, and `Cc7Patch::rect` — which is built from the same
/// constants — is what
/// `cc7_base_scene_populations_are_the_contract_table` classifies by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Cc7BaseRegion {
    /// The horizontal neutral ramp, whose code is a function of `x`.
    Ramp,
    /// `CC7_CHART_PATCHES[index]`.
    Chart(usize),
    /// `CC7_PRIMARY_PATCHES[index]`.
    Primary(usize),
    /// `CC7_ROW_PATCHES[index]`.
    Row(usize),
    /// Everything else: §2.3.3's remainder.
    Surround,
}

/// Whether the half-open rect `rect` covers `(x, y)`.
const fn rect_contains(rect: Cc7PixelRect, x: u32, y: u32) -> bool {
    x >= rect.x && x < rect.x + rect.width && y >= rect.y && y < rect.y + rect.height
}

/// The patch index `x` falls on inside a band of `width`-wide patches, or
/// `None` past the band's last patch.
fn patch_index(rect: Cc7PixelRect, x: u32, width: u32, count: usize) -> Option<usize> {
    let index = ((x - rect.x) / width) as usize;
    (index < count).then_some(index)
}

/// CC7 §2.3.3's region at `(x, y)`, decided by position alone.
fn base_region(x: u32, y: u32) -> Cc7BaseRegion {
    if rect_contains(CC7_RAMP_RECT, x, y) {
        return Cc7BaseRegion::Ramp;
    }
    if rect_contains(CC7_CHART_BAND_RECT, x, y)
        && let Some(index) = patch_index(
            CC7_CHART_BAND_RECT,
            x,
            CC7_CHART_PATCH_WIDTH,
            CC7_CHART_PATCHES.len(),
        )
    {
        return Cc7BaseRegion::Chart(index);
    }
    if rect_contains(CC7_PRIMARY_BAND_RECT, x, y)
        && let Some(index) = patch_index(
            CC7_PRIMARY_BAND_RECT,
            x,
            CC7_CHART_PATCH_WIDTH,
            CC7_PRIMARY_PATCHES.len(),
        )
    {
        return Cc7BaseRegion::Primary(index);
    }
    if rect_contains(CC7_ROW_BAND_RECT, x, y)
        && let Some(index) = patch_index(
            CC7_ROW_BAND_RECT,
            x,
            CC7_ROW_PATCH_WIDTH,
            CC7_ROW_PATCHES.len(),
        )
    {
        return Cc7BaseRegion::Row(index);
    }
    Cc7BaseRegion::Surround
}

fn linear_of_display_code(code: u8) -> f64 {
    cc7_decode_display709(f64::from(code) / 255.0)
}

fn linear_of_grade709_millionths(grade709: [i64; 3]) -> [f64; 3] {
    grade709.map(|value| cc7_grade709_decode(cc7_as_f64(value) / 1_000_000.0))
}

/// CC7 §3.1's `cc7_base_scene_rgb`: the camera-A display code at `(x, y)`.
///
/// The analytic scene-linear value behind the code is **not** computed here —
/// only [`cc7_log_scene_rgb`] needs it, and §2.4.3 feeds the camera transform
/// the display code rather than the linear (see [`cc7_camera_scene_rgb`]).
#[must_use]
pub fn cc7_base_scene_rgb(x: u32, y: u32) -> [u8; 3] {
    match base_region(x, y) {
        Cc7BaseRegion::Ramp => [cc7_ramp_code(x); 3],
        Cc7BaseRegion::Chart(index) => CC7_CHART_PATCHES[index].display_code_cam_a,
        Cc7BaseRegion::Primary(index) => CC7_PRIMARY_PATCHES[index].display_code_cam_a,
        Cc7BaseRegion::Row(index) => CC7_ROW_PATCHES[index].display_code_cam_a,
        Cc7BaseRegion::Surround => [CC7_SURROUND_CODE; 3],
    }
}

/// The analytic scene-linear value behind `(x, y)`, which CC7 §2.4.2's curve
/// is fed.
///
/// The chart, primaries and ramp are authored **as display codes**, so their
/// linear is the decode of the code; the row patches and the surround are
/// authored **in grade709**, so theirs is `grade709_decode(g)` and *not* the
/// decode of the rounded 8-bit code (§2.4.1's split, A-E13).
fn base_linear(x: u32, y: u32) -> [f64; 3] {
    match base_region(x, y) {
        Cc7BaseRegion::Ramp => [linear_of_display_code(cc7_ramp_code(x)); 3],
        Cc7BaseRegion::Chart(index) => CC7_CHART_PATCHES[index]
            .display_code_cam_a
            .map(linear_of_display_code),
        Cc7BaseRegion::Primary(index) => CC7_PRIMARY_PATCHES[index]
            .display_code_cam_a
            .map(linear_of_display_code),
        Cc7BaseRegion::Row(index) => linear_of_grade709_millionths(
            CC7_ROW_PATCHES[index]
                .grade709
                .expect("every row patch is authored from its grade709 value"),
        ),
        Cc7BaseRegion::Surround => linear_of_grade709_millionths(CC7_SURROUND_GRADE709_MILLIONTHS),
    }
}

/// CC7 §3.1's `cc7_camera_scene_rgb`: the base scene through one camera's
/// linear-light transform (CC7 §2.4.3).
///
/// `Cc7Camera::LogLike` is routed **here**, in the one place, to
/// [`cc7_log_scene_rgb`]: it has no linear-light transform — `cc7_camera_code`
/// would resolve it to the identity and hand back the cam-A raster — and its
/// content is §2.4.2's carrier. `cc7_source_rgb` and [`cc7_camera_source`] go
/// through this function, so the two entry points cannot disagree about what
/// `LogLike` means.
#[must_use]
pub fn cc7_camera_scene_rgb(camera: Cc7Camera, x: u32, y: u32) -> [u8; 3] {
    if matches!(camera, Cc7Camera::LogLike) {
        return cc7_log_scene_rgb(x, y);
    }
    // §2.4.3's `code_out(c)` takes the **display code**, not the analytic
    // linear the log curve is fed; the asymmetry with `cc7_log_scene_rgb` is
    // the contract's.
    cc7_camera_code(camera, cc7_base_scene_rgb(x, y))
}

/// CC7 §3.1's `cc7_log_scene_rgb`: the base scene's **linear** values through
/// CC7 §2.4.2's curve.
#[must_use]
pub fn cc7_log_scene_rgb(x: u32, y: u32) -> [u8; 3] {
    base_linear(x, y).map(|linear| cc7_display_code(cc7_log_value(linear)))
}

/// The (f) raster's four static skin patches: [`CC7_SKIN_BAND_RECT`]'s `x` and
/// width at CC7 §2.3.6's `y 4..20`.
///
/// The two `y` figures are pinned to core's own `i64` constants by the
/// compile-time assertion below rather than cast into `u32` at every use.
const TRACK_PATCH_RECT: Cc7PixelRect =
    Cc7PixelRect::new(CC7_SKIN_BAND_RECT.x, 4, CC7_SKIN_BAND_RECT.width, 16);

const _: () = assert!(
    CC7_TRACK_STATIC_PATCH_TOP == 4 && CC7_TRACK_STATIC_PATCH_BOTTOM == 20,
    "TRACK_PATCH_RECT restates CC7 §2.3.6's static patch band y 4..20"
);

/// CC7 §3.1's `cc7_tracked_scene_rgb`: the surround, the four static skin
/// patches at `y 4..20, x 0..48`, and the moving `product_red` square drawn
/// **last** (opaque, on top), except on the occluded frames `43..=47`.
///
/// # Panics
///
/// Panics when the square's analytic path would leave the raster or touch the
/// static patch rows, which CC7 §2.3.6 requires the generator to assert.
#[must_use]
pub fn cc7_tracked_scene_rgb(x: u32, y: u32, frame: u32) -> [u8; 3] {
    let frame_index = i64::from(frame);
    let (left, top) = cc7_analytic_square_top_left(frame_index);
    assert!(
        left >= 0,
        "frame {frame}: the square must not leave the raster"
    );
    assert!(
        left + CC7_TRACK_SQUARE_SIZE <= i64::from(CC7_SOURCE_WIDTH),
        "frame {frame}: the square must not leave the raster"
    );
    assert!(
        top >= CC7_TRACK_SQUARE_SIZE,
        "frame {frame}: the square must clear the static patch rows"
    );
    assert!(
        top + CC7_TRACK_SQUARE_SIZE <= i64::from(CC7_SOURCE_HEIGHT),
        "frame {frame}: the square must not leave the raster"
    );

    if cc7_square_is_drawn(frame_index) {
        let inside_x = i64::from(x) >= left && i64::from(x) < left + CC7_TRACK_SQUARE_SIZE;
        let inside_y = i64::from(y) >= top && i64::from(y) < top + CC7_TRACK_SQUARE_SIZE;
        if inside_x && inside_y {
            return CC7_ROW_PATCHES[4].display_code_cam_a;
        }
    }
    if rect_contains(TRACK_PATCH_RECT, x, y)
        && let Some(index) = patch_index(
            TRACK_PATCH_RECT,
            x,
            CC7_ROW_PATCH_WIDTH,
            CC7_ROW_PATCHES.len(),
        )
    {
        return CC7_ROW_PATCHES[index].display_code_cam_a;
    }
    [CC7_SURROUND_CODE; 3]
}

/// One pixel of one CC7 raster, for whichever generator is being written.
#[must_use]
pub fn cc7_source_rgb(kind: Cc7SourceKind, x: u32, y: u32, frame: u32) -> [u8; 3] {
    match kind.normalized() {
        Cc7SourceKind::Log => cc7_log_scene_rgb(x, y),
        // `normalized` has folded `Camera(LogLike)` into `Log`, and
        // `cc7_camera_scene_rgb` routes it to the same raster in any case.
        Cc7SourceKind::Camera(camera) => cc7_camera_scene_rgb(camera, x, y),
        Cc7SourceKind::Tracked => cc7_tracked_scene_rgb(x, y, frame),
    }
}

// ===========================================================================
// CC7 §3.2: the mux recipe, normative.
// ===========================================================================

/// The §3.2 forward BT.709 limited-range matrix, **independently transcribed**
/// in `f64` from the contract's equations.
///
/// Rule 11.0.1 forbids obtaining an *expected value* from
/// `bt709_limited_ycbcr`; it explicitly permits generating *source content*
/// with an independent transcription, which is what this is. Nothing in this
/// file compares a measurement against the output of this function.
///
/// **Provenance:** this is CC6's transcription (`cc6_fixtures.rs:340-369`)
/// carried over, not a second independent reading of §3.2's equations, so a
/// transcription error in CC6 would be inherited here rather than caught. It
/// is recorded rather than re-derived because R3 C1 re-derived the matrix from
/// the contract's prose and it matches (`review-sources.md` m3, C1).
///
/// # Panics
///
/// Panics when a computed code would leave the 8-bit container, which cannot
/// happen for an in-range `R'G'B'` triple and would mean the matrix drifted.
#[must_use]
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub fn cc7_bt709_limited_source_codes(rgb: [u8; 3]) -> [u8; 3] {
    const KR: f64 = 0.2126;
    const KB: f64 = 0.0722;
    const KG: f64 = 1.0 - KR - KB;
    const CB_DENOMINATOR: f64 = 1.8556;
    const CR_DENOMINATOR: f64 = 1.5748;
    let [red, green, blue] = rgb.map(|code| f64::from(code) / 255.0);
    let luma = KR * red + KG * green + KB * blue;
    let cb = (blue - luma) / CB_DENOMINATOR;
    let cr = (red - luma) / CR_DENOMINATOR;
    let code = |value: f64| {
        let rounded = value.round();
        assert!(
            (0.0..=255.0).contains(&rounded),
            "a source code must land inside the 8-bit container: {value}"
        );
        rounded as u8
    };
    [
        code(16.0 + 219.0 * luma),
        code(128.0 + 224.0 * cb),
        code(128.0 + 224.0 * cr),
    ]
}

/// One frame of a CC7 raster as `yuv444p` planes, luma then Cb then Cr.
///
/// This is the byte-exact authored content `cc7_ffv1_round_trip_is_byte_exact`
/// compares a decoded frame against.
#[must_use]
pub fn cc7_source_frame_planes(kind: Cc7SourceKind, frame: u32) -> Vec<u8> {
    let count = (CC7_SOURCE_WIDTH * CC7_SOURCE_HEIGHT) as usize;
    let mut luma = Vec::with_capacity(count);
    let mut cb = Vec::with_capacity(count);
    let mut cr = Vec::with_capacity(count);
    for y in 0..CC7_SOURCE_HEIGHT {
        for x in 0..CC7_SOURCE_WIDTH {
            let codes = cc7_bt709_limited_source_codes(cc7_source_rgb(kind, x, y, frame));
            luma.push(codes[0]);
            cb.push(codes[1]);
            cr.push(codes[2]);
        }
    }
    luma.append(&mut cb);
    luma.append(&mut cr);
    luma
}

/// Every frame of a CC7 raster, concatenated as the muxer's `.yuv` input.
///
/// The camera and log rasters are static (CC7 §11.1), so their one authored
/// frame is repeated rather than recomputed sixty times; only the tracked
/// raster is rendered per frame.
#[must_use]
pub fn cc7_source_planes(kind: Cc7SourceKind) -> Vec<u8> {
    let frames = kind.frames() as usize;
    if kind.is_static() {
        return cc7_source_frame_planes(kind, 0).repeat(frames);
    }
    let mut raw = Vec::new();
    for frame in 0..kind.frames() {
        raw.extend(cc7_source_frame_planes(kind, frame));
    }
    raw
}

/// A temp file that is removed when it goes out of scope, **including on a
/// panic**.
///
/// `run_ffmpeg` panics on a missing binary and on a nonzero exit
/// (`test_support.rs:274-298`), so a plain `remove_file` after the call is
/// unreachable on exactly the path that leaks — a failing CI lane would leave
/// 10.4 MB (camera/log) or 17.3 MB (tracked) behind per invocation. CC6 gets
/// this from `TempDirectory`'s `Drop` (`test_support.rs:44-48`); `cc7_source`
/// takes no directory argument, so it carries its own guard.
struct RawFrames {
    path: PathBuf,
}

impl RawFrames {
    /// Write `bytes` to a uniquely named `.yuv` beside the other temp media.
    fn write(label: &str, bytes: &[u8]) -> Self {
        let path = std::env::temp_dir().join(format!(
            "{label}-{}-{}.yuv",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("the system clock follows the Unix epoch")
                .as_nanos()
        ));
        std::fs::write(&path, bytes).expect("the raw CC7 source should write");
        Self { path }
    }

    /// The path, as `FFmpeg`'s `-i` argument.
    fn input(&self) -> String {
        self.path.to_string_lossy().into_owned()
    }
}

impl Drop for RawFrames {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Write one CC7 raster as a tagged limited-range BT.709 FFV1 `.mkv`.
///
/// The recipe is CC6's verbatim (`cc6_fixtures.rs:555-596`), because CC1
/// rejects an untagged source and FFV1's losslessness is already proven
/// in-suite by `verify_native_ramp` (`cc1_fixtures.rs:509-541`): the frames are
/// written to a temp `.yuv` because [`run_ffmpeg`] cannot pipe stdin, and the
/// tags are set by `setparams` **and** the explicit `-color_*` flags.
///
/// # Panics
///
/// Panics when the provisioned `FFmpeg` CLI is missing or reports a nonzero
/// exit, exactly as [`run_ffmpeg`] does.
#[must_use]
pub fn cc7_source(kind: Cc7SourceKind) -> GeneratedMedia {
    let raw = cc7_source_planes(kind);
    assert_eq!(
        raw.len(),
        (CC7_SOURCE_WIDTH * CC7_SOURCE_HEIGHT * 3 * kind.frames()) as usize
    );
    let frames = RawFrames::write(kind.label(), &raw);
    let size = format!("{CC7_SOURCE_WIDTH}x{CC7_SOURCE_HEIGHT}");
    let rate = CC7_SOURCE_FPS.to_string();
    let input = frames.input();
    let arguments = [
        "-f",
        "rawvideo",
        "-pix_fmt",
        "yuv444p",
        "-s",
        &size,
        "-r",
        &rate,
        "-i",
        &input,
        "-vf",
        "setparams=range=limited:color_primaries=bt709:color_trc=bt709:colorspace=bt709",
        "-c:v",
        "ffv1",
        "-level",
        "3",
        "-g",
        "1",
        "-pix_fmt",
        "yuv444p",
        "-color_primaries",
        "bt709",
        "-color_trc",
        "bt709",
        "-colorspace",
        "bt709",
        "-color_range",
        "tv",
    ];
    GeneratedMedia::ffmpeg(kind.label(), &arguments, "mkv")
    // `frames` drops here, removing the `.yuv` — and it drops on a
    // `run_ffmpeg` panic too, which the old explicit `remove_file` did not.
}

/// CC7 §3.1's `cc7_camera_source`: the 60-frame base scene as one camera.
///
/// # Panics
///
/// Panics as [`cc7_source`] does.
#[must_use]
pub fn cc7_camera_source(camera: Cc7Camera) -> GeneratedMedia {
    cc7_source(Cc7SourceKind::camera(camera))
}

/// CC7 §3.1's `cc7_log_source`: the 60-frame log-like carrier, **BT.709
/// tagged**.
///
/// Log tags stay refused (CC7 §3.3): `classify_source` accepts only
/// `Srgb | Bt709 | Bt1886` (`color.rs:730-739`) and `open_scaled_managed`
/// blocks managed decode for `Log`/`LogC`/`Log3G10` (`decode.rs:1092-1097`).
/// The carrier's *content* is log-ish and is undone by a node — that is the
/// whole scenario — and CC7 cites CC1's refusal fixtures rather than
/// duplicating them.
///
/// # Panics
///
/// Panics as [`cc7_source`] does.
#[must_use]
pub fn cc7_log_source() -> GeneratedMedia {
    cc7_source(Cc7SourceKind::Log)
}

/// CC7 §3.1's `cc7_tracked_source`: the 100-frame tracked scene with the
/// `43..=47` occlusion.
///
/// # Panics
///
/// Panics as [`cc7_source`] does.
#[must_use]
pub fn cc7_tracked_source() -> GeneratedMedia {
    cc7_source(Cc7SourceKind::Tracked)
}

/// The rasters one scenario's documents are cut from, in clip order.
///
/// Scenario (b) returns three: the reference and **both** candidates, because
/// (b1) and (b2) are two documents over the same reference (CC7 §2.5).
#[must_use]
pub fn cc7_scenario_source_kinds(scenario: Cc7Scenario) -> Vec<Cc7SourceKind> {
    match scenario {
        Cc7Scenario::MixedCamera => vec![
            Cc7SourceKind::camera(Cc7Camera::A),
            Cc7SourceKind::camera(Cc7Camera::B),
        ],
        Cc7Scenario::WhiteBalance => vec![
            Cc7SourceKind::camera(Cc7Camera::A),
            Cc7SourceKind::camera(Cc7Camera::C1),
            Cc7SourceKind::camera(Cc7Camera::C2),
        ],
        Cc7Scenario::LogLike => vec![Cc7SourceKind::Log],
        Cc7Scenario::ProductAndSkin | Cc7Scenario::CreativeLook => {
            vec![Cc7SourceKind::camera(Cc7Camera::A)]
        }
        Cc7Scenario::TrackedSecondary => vec![Cc7SourceKind::Tracked],
    }
}

/// CC7 §3.1's `cc7_scenario_sources`.
///
/// # Panics
///
/// Panics as [`cc7_source`] does.
#[must_use]
pub fn cc7_scenario_sources(scenario: Cc7Scenario) -> Vec<GeneratedMedia> {
    cc7_scenario_source_kinds(scenario)
        .into_iter()
        .map(cc7_source)
        .collect()
}

// ===========================================================================
// CC7 §3.4: the log-like inverse `.cube`.
// ===========================================================================

/// The `.cube` header, in CC4 §2.6's pinned canonical form.
///
/// [`CC7_LOG_CUBE_TITLE`]'s length is load-bearing for
/// `CC7_LOG_CUBE_BYTES_REPORTED`: CC4's canonical `.cube` text
/// (`lut.rs:219-240`) is `TITLE`, `LUT_3D_SIZE`, and two six-decimal domain
/// lines — `100 + title.len()` bytes of header **for a two-digit
/// `LUT_3D_SIZE`** — and each of the `S³` sample lines is exactly 27, so `65³`
/// is `115 + 274 625 · 27 = 7 414 990`.
fn cube_header(size: u32) -> String {
    let mut text = String::new();
    let _ = writeln!(text, "TITLE \"{CC7_LOG_CUBE_TITLE}\"");
    let _ = writeln!(text, "LUT_3D_SIZE {size}");
    let _ = writeln!(text, "DOMAIN_MIN {:.6} {:.6} {:.6}", 0.0, 0.0, 0.0);
    let _ = writeln!(text, "DOMAIN_MAX {:.6} {:.6} {:.6}", 1.0, 1.0, 1.0);
    text
}

/// Red-fastest lattice text for a per-channel separable transfer.
fn separable_cube(size: u32, transfer: impl Fn(f64) -> f64) -> String {
    assert!(
        size >= 2,
        "a .cube lattice needs at least two points a side"
    );
    let steps = f64::from(size - 1);
    let axis = (0..size)
        .map(|index| transfer(f64::from(index) / steps))
        .collect::<Vec<_>>();
    let points = (size as usize).saturating_pow(3);
    let header = cube_header(size);
    let mut text = String::with_capacity(header.len() + points * 27);
    text.push_str(&header);
    for blue in 0..size as usize {
        for green in 0..size as usize {
            for red in 0..size as usize {
                let _ = writeln!(
                    text,
                    "{:.6} {:.6} {:.6}",
                    axis[red], axis[green], axis[blue]
                );
            }
        }
    }
    text
}

/// CC7 §3.4's `log_like_inverse_cube(size)`.
///
/// The output for lattice input `e ∈ [0, 1]` is
/// `clamp(encode_bt709(2^(12e − 8)), 0, 1)`, identical on all three channels.
/// It is bound at `input_encoding_token = 0` (`Display709`,
/// `color_pipeline.rs:1286-1293`), so its input is `e = encode_bt709(x)` and
/// the production path is `z = Lut3d::lookup(e)` (tetrahedral) then
/// `x' = decode_display709(z)`.
///
/// **The output clamp is kept.** A `.cube` whose domain is `[0, 1]` and whose
/// outputs leave it is not a well-formed cube, and CC7 does not author one to
/// buy four codes.
///
/// # Panics
///
/// Panics when `size` is below two.
#[must_use]
pub fn log_like_inverse_cube(size: u32) -> String {
    separable_cube(size, |e| {
        cc7_encode_bt709(2.0_f64.powf(12.0 * e - 8.0)).clamp(0.0, 1.0)
    })
}

/// An identity `.cube` of the same shape, §4(c)(2)'s failing direction: the
/// operator imports a look that does nothing and the log carrier is monitored
/// as if it were display-encoded.
///
/// # Panics
///
/// Panics when `size` is below two.
#[must_use]
pub fn identity_cube(size: u32) -> String {
    separable_cube(size, |e| e)
}

fn write_cube(directory: &Path, name: &str, text: &str) -> PathBuf {
    let path = directory.join(name);
    std::fs::write(&path, text).expect("a CC7 .cube should write");
    path
}

/// CC7 §3.1's `write_log_like_inverse_cube`.
///
/// # Panics
///
/// Panics when the file cannot be written.
#[must_use]
pub fn write_log_like_inverse_cube(directory: &Path, size: u32) -> PathBuf {
    write_cube(
        directory,
        &format!("cc7-log-inverse-{size}.cube"),
        &log_like_inverse_cube(size),
    )
}

/// The identity twin of [`write_log_like_inverse_cube`].
///
/// # Panics
///
/// Panics when the file cannot be written.
#[must_use]
pub fn write_identity_cube(directory: &Path, size: u32) -> PathBuf {
    write_cube(
        directory,
        &format!("cc7-identity-{size}.cube"),
        &identity_cube(size),
    )
}

/// The canonical lattice size CC7 pins (A22), re-exported here so a caller
/// that only sees the generator does not have to reach into core for it.
pub const CC7_CANONICAL_CUBE_SIZE: u32 = CC7_LOG_CUBE_SIZE;

// ===========================================================================
// CC7 §3.5: the non-vacuity fixtures.
//
// A source that fails one of these makes every claim measured on it
// meaningless, so they run before the gates that consume them.
// ===========================================================================

#[cfg(test)]
mod tests {
    use std::process::Command as ProcessCommand;

    use kinewright_core::cc7_scenarios::{
        CC7_CHART_PATCH_PIXELS, CC7_LOG_CARRIER_LUMA_PERCENTILES_CODE8,
        CC7_LOG_CARRIER_LUMA_PERCENTILES_CODE16, CC7_LOG_CHART_CODES, CC7_LOG_CUBE_BYTES_REPORTED,
        CC7_LOG_FIRST_PERCENTILE_MIN_CODE16, CC7_LOG_P99_MAX_CODE16, CC7_LOG_ROW_CODES,
        CC7_LOG_SURROUND_CODE, CC7_MATTE_SAMPLE_HUE_WIDTH_CENTIDEGREES, CC7_MATTE_SAMPLE_SOFTNESS,
        CC7_PRODUCT_SAMPLE_HUE_MEDIAN_CENTIDEGREES, CC7_RASTER_PIXELS, CC7_REGION_POPULATIONS,
        CC7_ROW_PATCH_PIXELS, CC7_SCOPE_SIXTEEN_BIT_SCALE, CC7_TRACK_ANALYTIC_CENTRES_BASIS_POINTS,
        CC7_TRACK_OCCLUSION_FIRST_FRAME, CC7_TRACK_OCCLUSION_LAST_FRAME, CC7_TRACK_SQUARE_SIZE,
        Cc7Patch, Cc7Scenario, cc7_analytic_square_centre_basis_points,
        cc7_analytic_square_top_left, cc7_ramp_code, cc7_tracking_sample_frames,
    };

    use crate::{LUT_MAX_FILE_BYTES, parse_cube_lut, test_support::ffmpeg_executable};

    use super::{
        CC7_CHART_PATCHES, CC7_LOG_CUBE_TITLE, CC7_PRIMARY_PATCHES, CC7_RAMP_RECT, CC7_ROW_PATCHES,
        CC7_SOURCE_HEIGHT, CC7_SOURCE_WIDTH, CC7_SURROUND_CODE, CC7_TRACK_FRAMES, Cc7Camera,
        Cc7SourceKind, cc7_base_scene_rgb, cc7_camera_scene_rgb, cc7_camera_source,
        cc7_log_scene_rgb, cc7_log_source, cc7_scenario_source_kinds, cc7_source_frame_planes,
        cc7_source_planes, cc7_tracked_scene_rgb, cc7_tracked_source, identity_cube,
        log_like_inverse_cube, rect_contains,
    };

    /// `luma_code`, transcribed from `kinewright-core::scopes:1314`.
    fn luma_code(rgb: [u8; 3]) -> u8 {
        let weighted =
            54_u32 * u32::from(rgb[0]) + 183_u32 * u32::from(rgb[1]) + 19_u32 * u32::from(rgb[2]);
        u8::try_from(weighted / 256).unwrap_or(u8::MAX)
    }

    /// The nearest-rank convention, transcribed from `scopes.rs:1319-1338`:
    /// `rank = ceil(count · p / 100)`, and the published field is `value ×
    /// 257`, a **16-bit** code.
    fn percentile_code16(histogram: &[u64; 256], percentile: u64) -> i64 {
        let count: u64 = histogram.iter().sum();
        assert!(count > 0);
        let rank = (u128::from(count) * u128::from(percentile)).div_ceil(100);
        let mut cumulative = 0_u128;
        for (value, frequency) in histogram.iter().copied().enumerate() {
            cumulative += u128::from(frequency);
            if cumulative >= rank {
                return i64::try_from(value).unwrap_or(255) * CC7_SCOPE_SIXTEEN_BIT_SCALE;
            }
        }
        unreachable!("the rank never exceeds the sample count")
    }

    fn luma_histogram(pixel: impl Fn(u32, u32) -> [u8; 3]) -> [u64; 256] {
        let mut histogram = [0_u64; 256];
        for y in 0..CC7_SOURCE_HEIGHT {
            for x in 0..CC7_SOURCE_WIDTH {
                histogram[luma_code(pixel(x, y)) as usize] += 1;
            }
        }
        histogram
    }

    /// The HSV hue of a saturated primary, in centidegrees.
    ///
    /// For a `0`/`255` primary the hue is invariant under any monotone
    /// per-channel transform, so this is also its grade709 hue, which is what
    /// the CC5 §2.4 qualifier evaluates.
    fn primary_hue_centidegrees(rgb: [u8; 3]) -> i64 {
        let [red, green, blue] = rgb.map(i64::from);
        let maximum = red.max(green).max(blue);
        let minimum = red.min(green).min(blue);
        assert!(maximum > minimum, "a primary is not achromatic");
        let span = maximum - minimum;
        if maximum == red {
            ((green - blue) * 6_000 / span + 36_000) % 36_000
        } else if maximum == green {
            (blue - red) * 6_000 / span + 12_000
        } else {
            (red - green) * 6_000 / span + 24_000
        }
    }

    /// Circular distance between two hues, in centidegrees.
    fn hue_distance(left: i64, right: i64) -> i64 {
        let raw = (left - right).abs() % 36_000;
        raw.min(36_000 - raw)
    }

    /// Decode one generated `.mkv` to `yuv444p` rawvideo, in
    /// `verify_native_ramp`'s shape (`cc1_fixtures.rs:510-541`).
    fn decode_yuv444p(path: &std::path::Path) -> Vec<u8> {
        let output = ProcessCommand::new(ffmpeg_executable())
            .args(["-hide_banner", "-loglevel", "error", "-i"])
            .arg(path)
            .args(["-f", "rawvideo", "-pix_fmt", "yuv444p", "pipe:1"])
            .output()
            .unwrap_or_else(|error| panic!("CC7 source decode failed to start: {error}"));
        assert!(
            output.status.success(),
            "CC7 source decode failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        output.stdout
    }

    // -----------------------------------------------------------------------
    // §3.5(6) / §11.2.12 — key `raster.populations`.
    // -----------------------------------------------------------------------

    /// The patch table entry whose `rect` covers `(x, y)`, if any.
    ///
    /// The classification is the **patch tables'** own geometry, so a band the
    /// generator draws narrower, wider or displaced than `Cc7Patch::rect`
    /// moves a count *and* trips the authored-code equality beside it.
    fn patch_at(table: &'static [Cc7Patch], x: u32, y: u32) -> Option<&'static Cc7Patch> {
        table.iter().find(|patch| rect_contains(patch.rect, x, y))
    }

    /// Classify every pixel of the authored raster by the patch tables' own
    /// rects and by [`CC7_RAMP_RECT`], assert the code the generator wrote
    /// there is that region's code, and count it.
    fn measured_base_populations() -> [(&'static str, u32); 5] {
        let mut ramp = 0_u32;
        let mut chart = 0_u32;
        let mut primaries = 0_u32;
        let mut row = 0_u32;
        let mut surround = 0_u32;
        for y in 0..CC7_SOURCE_HEIGHT {
            for x in 0..CC7_SOURCE_WIDTH {
                let authored = cc7_base_scene_rgb(x, y);
                let named = if rect_contains(CC7_RAMP_RECT, x, y) {
                    ramp += 1;
                    let code = cc7_ramp_code(x);
                    assert_eq!(authored, [code; 3], "({x}, {y}) is the neutral ramp");
                    continue;
                } else if let Some(patch) = patch_at(&CC7_CHART_PATCHES, x, y) {
                    chart += 1;
                    patch
                } else if let Some(patch) = patch_at(&CC7_PRIMARY_PATCHES, x, y) {
                    primaries += 1;
                    patch
                } else if let Some(patch) = patch_at(&CC7_ROW_PATCHES, x, y) {
                    row += 1;
                    patch
                } else {
                    surround += 1;
                    assert_eq!(
                        authored, [CC7_SURROUND_CODE; 3],
                        "({x}, {y}) is outside every named region and must be the surround"
                    );
                    continue;
                };
                assert_eq!(
                    authored, named.display_code_cam_a,
                    "({x}, {y}) is {}",
                    named.name
                );
            }
        }
        [
            ("neutral_ramp_band", ramp),
            ("achromatic_chart_band", chart),
            ("primaries_band", primaries),
            ("patch_row", row),
            ("surround", surround),
        ]
    }

    /// §2.3.3's five population counts and their sum, measured over the
    /// **authored raster**: every pixel is classified by the patch tables'
    /// rects and by [`CC7_RAMP_RECT`], and the code the generator wrote there
    /// is asserted to be that region's code before the pixel is counted.
    ///
    /// *Fails:* any band whose edge moves. A one-pixel `y` shift of the
    /// primaries band makes either `(0, 56)` the surround (shifted down) or
    /// `(0, 55)` a primary (shifted up), and the four `y` probes below assert
    /// both edges and the surround rows either side of them, measured — a
    /// shift does **not** change the surround *count*, because it trades 40
    /// pixels for 40, which is why the old count-only formulation could not
    /// see it. A chart band narrowed to `x < 92` makes `chart11`'s last four
    /// columns surround, which the `x`-edge probes and the authored-code
    /// equality inside the loop both catch.
    #[test]
    fn cc7_base_scene_populations_are_the_contract_table() {
        // Failing direction, the `y` edges: the primaries band's first and
        // last authored rows and the surround rows either side of them, so a
        // one-pixel shift **either way** fires one of these four by name (a
        // shift does *not* move the surround count — it trades 40 pixels for
        // 40 — which is why a count alone cannot see it).
        assert_eq!(
            cc7_base_scene_rgb(0, 55),
            [CC7_SURROUND_CODE; 3],
            "the row above the primaries band is the surround"
        );
        assert_ne!(
            cc7_base_scene_rgb(0, 56),
            [CC7_SURROUND_CODE; 3],
            "the primaries band starts at y 56"
        );
        assert_ne!(
            cc7_base_scene_rgb(0, 71),
            [CC7_SURROUND_CODE; 3],
            "the primaries band's last authored row is y 71"
        );
        assert_eq!(
            cc7_base_scene_rgb(0, 72),
            [CC7_SURROUND_CODE; 3],
            "the primaries band ends at y 72"
        );

        // Failing direction, the `x` edges: the last authored column of each
        // band and the first surround column past it, so a band narrowed or
        // widened by even one column fires here as well as in the loop.
        for (name, last_x, first_surround_x, y, code) in [
            (
                "achromatic_chart_band",
                95_u32,
                96_u32,
                36_u32,
                CC7_CHART_PATCHES[11].display_code_cam_a,
            ),
            (
                "primaries_band",
                39,
                40,
                56,
                CC7_PRIMARY_PATCHES[4].display_code_cam_a,
            ),
            (
                "patch_row",
                83,
                84,
                76,
                CC7_ROW_PATCHES[6].display_code_cam_a,
            ),
        ] {
            assert_eq!(
                cc7_base_scene_rgb(last_x, y),
                code,
                "{name}: x {last_x} is the band's last authored column"
            );
            assert_eq!(
                cc7_base_scene_rgb(first_surround_x, y),
                [CC7_SURROUND_CODE; 3],
                "{name}: x {first_surround_x} is past the band"
            );
        }

        let measured = measured_base_populations();
        let [_, (_, chart), (_, primaries), (_, row), _] = measured;
        assert_eq!(measured, CC7_REGION_POPULATIONS);
        assert_eq!(
            measured.iter().map(|(_, count)| count).sum::<u32>(),
            CC7_RASTER_PIXELS
        );
        assert_eq!(chart, 12 * CC7_CHART_PATCH_PIXELS);
        assert_eq!(primaries, 5 * CC7_CHART_PATCH_PIXELS);
        assert_eq!(row, 7 * CC7_ROW_PATCH_PIXELS);
    }

    // -----------------------------------------------------------------------
    // §3.5(7) / §11.2.13 — key `raster.a1_guard`.
    // -----------------------------------------------------------------------

    /// **This is the fixture that stops a later tidy-up from putting the red
    /// primary back and silently breaking (d).**
    #[test]
    fn cc7_the_chart_band_is_achromatic_and_the_primaries_band_has_no_red() {
        for patch in &CC7_CHART_PATCHES {
            let [red, green, blue] = patch.display_code_cam_a;
            assert!(
                red == green && green == blue,
                "{} must be achromatic",
                patch.name
            );
            // …and so is every pixel actually written into the band.
            let authored = cc7_base_scene_rgb(patch.rect.x + 4, patch.rect.y + 8);
            assert_eq!(authored, patch.display_code_cam_a, "{}", patch.name);
            assert!(authored[0] == authored[1] && authored[1] == authored[2]);
        }

        assert_eq!(CC7_PRIMARY_PATCHES.len(), 5);
        // …and the primaries band is probed on the raster, not only in the
        // table, so a wrong x-origin, a wrong patch order or a wrong index
        // inside the band is visible here rather than only in the population
        // fixture (m5).
        for patch in &CC7_PRIMARY_PATCHES {
            assert_eq!(
                cc7_base_scene_rgb(patch.rect.x + 4, patch.rect.y + 8),
                patch.display_code_cam_a,
                "{}",
                patch.name
            );
        }
        for y in 0..CC7_SOURCE_HEIGHT {
            for x in 0..CC7_SOURCE_WIDTH {
                assert_ne!(
                    cc7_base_scene_rgb(x, y),
                    [255, 0, 0],
                    "({x}, {y}) is the pure red primary, which A1 removed so (d)'s exact containment can pass"
                );
            }
        }

        // The derived `product_red` qualifier's hue centre is more than
        // `hue_width + softness` from every primary's hue …
        let threshold = CC7_MATTE_SAMPLE_HUE_WIDTH_CENTIDEGREES + CC7_MATTE_SAMPLE_SOFTNESS;
        for patch in &CC7_PRIMARY_PATCHES {
            let hue = primary_hue_centidegrees(patch.display_code_cam_a);
            let distance = hue_distance(hue, CC7_PRODUCT_SAMPLE_HUE_MEDIAN_CENTIDEGREES);
            assert!(
                distance > threshold,
                "{} at hue {hue} cd is only {distance} cd from the qualifier centre",
                patch.name
            );
        }
        // … and the pure red the band no longer carries would have been
        // captured, which is why it is absent rather than merely unused.
        assert!(
            hue_distance(0, CC7_PRODUCT_SAMPLE_HUE_MEDIAN_CENTIDEGREES) < threshold,
            "the red primary's hue 0 cd is inside the qualifier, which is the reason A1 removed it"
        );
    }

    // -----------------------------------------------------------------------
    // §3.5(1) / §11.2.14 — key `sources.non_vacuity`.
    // -----------------------------------------------------------------------

    /// Each candidate camera differs from the reference at every achromatic
    /// chart patch that carries any light, and over the raster as a whole.
    ///
    /// **`chart00` is the one exception, and it is structural**: the black
    /// patch decodes to exactly zero scene-linear light, and no per-channel
    /// gain, exposure scale, or luma-preserving saturation mix moves zero. §3.5(1)
    /// says "each of the twelve"; the eleven that carry light are asserted to
    /// differ and the black patch is asserted to be identical, which is the
    /// honest form of the same claim.
    #[test]
    fn cc7_camera_sources_differ_from_the_reference_at_every_neutral_patch() {
        const MINIMUM_MEAN_ABSOLUTE_DIFFERENCE: u64 = 5;
        for camera in [Cc7Camera::B, Cc7Camera::C1, Cc7Camera::C2] {
            for (index, patch) in CC7_CHART_PATCHES.iter().enumerate() {
                let x = patch.rect.x + 4;
                let y = patch.rect.y + 8;
                let reference = cc7_base_scene_rgb(x, y);
                let candidate = cc7_camera_scene_rgb(camera, x, y);
                assert_eq!(reference, patch.display_code_cam_a, "{}", patch.name);
                if index == 0 {
                    assert_eq!(
                        candidate, reference,
                        "{camera:?}: the black patch carries no light, so no camera moves it"
                    );
                } else {
                    assert!(
                        (0..3).any(|channel| candidate[channel] != reference[channel]),
                        "{camera:?} on {}: at least one channel must differ, or the source is vacuous",
                        patch.name
                    );
                }
            }

            let mut total = 0_u64;
            for y in 0..CC7_SOURCE_HEIGHT {
                for x in 0..CC7_SOURCE_WIDTH {
                    let reference = cc7_base_scene_rgb(x, y);
                    let candidate = cc7_camera_scene_rgb(camera, x, y);
                    for channel in 0..3 {
                        total += u64::from(candidate[channel].abs_diff(reference[channel]));
                    }
                }
            }
            let samples = u64::from(CC7_RASTER_PIXELS) * 3;
            assert!(
                total / samples >= MINIMUM_MEAN_ABSOLUTE_DIFFERENCE,
                "{camera:?}: mean absolute code difference {} is below {MINIMUM_MEAN_ABSOLUTE_DIFFERENCE}",
                total / samples
            );
        }

        // Failing direction: cam A against itself measures zero.
        for y in 0..CC7_SOURCE_HEIGHT {
            for x in 0..CC7_SOURCE_WIDTH {
                assert_eq!(
                    cc7_camera_scene_rgb(Cc7Camera::A, x, y),
                    cc7_base_scene_rgb(x, y),
                    "camera A is the identity"
                );
            }
        }
    }

    // -----------------------------------------------------------------------
    // §3.5(2) / §11.2.15 — key `sources.log_signature`.
    // -----------------------------------------------------------------------

    /// The carrier's chart codes are §2.4.2's stored log codes exactly, and its
    /// luma signature separates it from cam A in the **16-bit** unit
    /// `analyze_color_shot` publishes (A21).
    ///
    /// It also pins the **one** meaning of `Cc7Camera::LogLike`: it is the
    /// carrier, on both public entry points and in the source kind, so a
    /// caller that samples `cc7_camera_scene_rgb(LogLike, …)` as the (c)
    /// clip's per-pixel expectation measures the raster the clip actually
    /// carries.
    ///
    /// *Fails:* cam A fails **both** bounds, asserted; and a
    /// `cc7_camera_scene_rgb` that resolved `LogLike` to the identity would
    /// return the cam-A raster, which is asserted **not** to be the carrier at
    /// every pixel of the chart band.
    #[test]
    fn cc7_log_source_is_not_the_base_scene() {
        // `LogLike` is the carrier on every path, and `Camera(LogLike)` is not
        // a second, `PartialEq`-distinct name for the log source.
        assert_eq!(
            Cc7SourceKind::camera(Cc7Camera::LogLike),
            Cc7SourceKind::Log
        );
        assert_eq!(
            Cc7SourceKind::Camera(Cc7Camera::LogLike).normalized(),
            Cc7SourceKind::Log
        );
        for scenario in [
            Cc7Scenario::MixedCamera,
            Cc7Scenario::WhiteBalance,
            Cc7Scenario::LogLike,
            Cc7Scenario::ProductAndSkin,
            Cc7Scenario::CreativeLook,
            Cc7Scenario::TrackedSecondary,
        ] {
            for kind in cc7_scenario_source_kinds(scenario) {
                assert_ne!(
                    kind,
                    Cc7SourceKind::Camera(Cc7Camera::LogLike),
                    "{scenario:?} must name the carrier as Cc7SourceKind::Log"
                );
                assert_eq!(kind, kind.normalized(), "{scenario:?}");
            }
        }
        let mut differs = 0_u32;
        for y in 0..CC7_SOURCE_HEIGHT {
            for x in 0..CC7_SOURCE_WIDTH {
                assert_eq!(
                    cc7_camera_scene_rgb(Cc7Camera::LogLike, x, y),
                    cc7_log_scene_rgb(x, y),
                    "({x}, {y}): LogLike is the carrier, not the base scene"
                );
                if cc7_log_scene_rgb(x, y) != cc7_base_scene_rgb(x, y) {
                    differs += 1;
                }
            }
        }
        assert!(
            differs > CC7_RASTER_PIXELS / 2,
            "the carrier differs from the base scene at only {differs} pixels, so routing \
             LogLike to the identity would be invisible"
        );

        for (index, patch) in CC7_CHART_PATCHES.iter().enumerate() {
            let code = CC7_LOG_CHART_CODES[index];
            assert_eq!(
                cc7_log_scene_rgb(patch.rect.x + 4, patch.rect.y + 8),
                [code, code, code],
                "{}: the carrier's chart code must be the contract's stored log code",
                patch.name
            );
        }
        for (index, patch) in CC7_ROW_PATCHES.iter().enumerate() {
            assert_eq!(
                cc7_log_scene_rgb(patch.rect.x + 6, patch.rect.y + 8),
                CC7_LOG_ROW_CODES[index],
                "{}",
                patch.name
            );
        }
        assert_eq!(
            cc7_log_scene_rgb(200, 120),
            [CC7_LOG_SURROUND_CODE; 3],
            "the surround through the curve"
        );

        let carrier = luma_histogram(cc7_log_scene_rgb);
        let carrier_first = percentile_code16(&carrier, 1);
        let carrier_median = percentile_code16(&carrier, 50);
        let carrier_p99 = percentile_code16(&carrier, 99);
        assert_eq!(
            [carrier_first, carrier_median, carrier_p99],
            CC7_LOG_CARRIER_LUMA_PERCENTILES_CODE16,
            "the authored carrier's luma percentiles are probe-3's measured ones"
        );
        for index in 0..3 {
            assert_eq!(
                CC7_LOG_CARRIER_LUMA_PERCENTILES_CODE16[index],
                CC7_LOG_CARRIER_LUMA_PERCENTILES_CODE8[index] * CC7_SCOPE_SIXTEEN_BIT_SCALE
            );
        }
        assert!(carrier_first >= CC7_LOG_FIRST_PERCENTILE_MIN_CODE16);
        assert!(carrier_p99 <= CC7_LOG_P99_MAX_CODE16);

        // The failing direction — cam A fails both bounds — is
        // `cc7_c_the_base_scene_does_not_read_as_log`, §4(c)(1)'s own name for
        // it, below.
    }

    /// CC7 §4(c)(1)'s failing direction, under the contract's own name.
    ///
    /// Cam A fails **both** signature bounds, `2 827 < 5 140` and
    /// `62 194 > 51 400`, so the carrier's signature is a measurement of the
    /// carrier and not a property every source has. §4.2's `log signature`
    /// row names this fixture.
    ///
    /// It is a `#[test]` of its own rather than a paragraph inside
    /// `cc7_log_source_is_not_the_base_scene` (which carries §11.2.15's
    /// *other* failing direction, the `LogLike`-resolves-to-the-identity one):
    /// the claim is a pure histogram over the authored raster, so a named
    /// fixture costs nothing and §4.2's row now resolves to a test rather
    /// than to a comment inside one.
    ///
    /// Cam A's **authored-raster** p1 is `11` (16-bit `2 827`) rather than
    /// the `10` (`2 570`) probe-3 measured through the managed decode — the
    /// one-code decode round trip §2.4.1 records — and it fails the floor
    /// either way, which is why the gate is stated as an inequality against
    /// the constant rather than as an equality against a decoded number.
    #[test]
    fn cc7_c_the_base_scene_does_not_read_as_log() {
        let reference = luma_histogram(cc7_base_scene_rgb);
        let reference_first = percentile_code16(&reference, 1);
        let reference_p99 = percentile_code16(&reference, 99);
        assert_eq!([reference_first, reference_p99], [2_827, 62_194]);
        assert!(
            reference_first < CC7_LOG_FIRST_PERCENTILE_MIN_CODE16,
            "cam A must fail the first-percentile floor"
        );
        assert!(
            reference_p99 > CC7_LOG_P99_MAX_CODE16,
            "cam A must fail the 99th-percentile ceiling"
        );

        // Non-vacuity, in the same terms: the carrier passes both bounds the
        // base scene fails, so the two sources are separated by the gate and
        // not by the fixture's choice of assertion.
        let carrier = luma_histogram(cc7_log_scene_rgb);
        assert!(percentile_code16(&carrier, 1) >= CC7_LOG_FIRST_PERCENTILE_MIN_CODE16);
        assert!(percentile_code16(&carrier, 99) <= CC7_LOG_P99_MAX_CODE16);
    }

    // -----------------------------------------------------------------------
    // §3.5(3) / §11.2.16 — key `sources.tracking`.
    // -----------------------------------------------------------------------

    /// At each of the eleven sampled frames the pixel at the analytic centre
    /// is the `product_red` code, except at frame **47**, where it is the
    /// surround; consecutive sampled centres differ by at least one pixel; and
    /// the square is fully inside the raster at every frame `0..99`.
    ///
    /// *Fails:* a source with the square drawn on every frame reports
    /// `product_red` at 47, asserted.
    #[test]
    fn cc7_tracked_source_moves_and_occludes() {
        let product_red = CC7_ROW_PATCHES[4].display_code_cam_a;
        let mut centres = Vec::new();
        for (index, frame) in cc7_tracking_sample_frames().into_iter().enumerate() {
            let (left, top) = cc7_analytic_square_top_left(frame);
            let half = CC7_TRACK_SQUARE_SIZE / 2;
            let centre_x = u32::try_from(left + half).expect("an in-frame centre");
            let centre_y = u32::try_from(top + half).expect("an in-frame centre");
            let frame_index = u32::try_from(frame).expect("a non-negative frame");
            let pixel = cc7_tracked_scene_rgb(centre_x, centre_y, frame_index);
            if frame == CC7_TRACK_OCCLUSION_LAST_FRAME {
                assert_eq!(
                    pixel, [CC7_SURROUND_CODE; 3],
                    "frame {frame} is inside the occlusion, so the centre is surround"
                );
            } else {
                assert_eq!(pixel, product_red, "frame {frame}");
            }
            // The analytic centre in basis points is the table's.
            let (centre_bp_x, centre_bp_y) = cc7_analytic_square_centre_basis_points(frame);
            assert_eq!(
                [centre_bp_x, centre_bp_y],
                CC7_TRACK_ANALYTIC_CENTRES_BASIS_POINTS[index],
                "frame {frame}"
            );
            centres.push((left, top));
        }
        for pair in centres.windows(2) {
            let moved = (pair[0].0 - pair[1].0)
                .abs()
                .max((pair[0].1 - pair[1].1).abs());
            assert!(
                moved >= 1,
                "two consecutive sampled centres must differ by at least one pixel"
            );
        }

        // Every occluded frame is surround at the centre; every other frame is
        // the square.
        for frame in 0..CC7_TRACK_FRAMES {
            let frame_index = i64::from(frame);
            let (left, top) = cc7_analytic_square_top_left(frame_index);
            let half = CC7_TRACK_SQUARE_SIZE / 2;
            let pixel = cc7_tracked_scene_rgb(
                u32::try_from(left + half).expect("in frame"),
                u32::try_from(top + half).expect("in frame"),
                frame,
            );
            let occluded = (CC7_TRACK_OCCLUSION_FIRST_FRAME..=CC7_TRACK_OCCLUSION_LAST_FRAME)
                .contains(&frame_index);
            if occluded {
                assert_eq!(pixel, [CC7_SURROUND_CODE; 3], "frame {frame}");
            } else {
                assert_eq!(pixel, product_red, "frame {frame}");
            }
            assert!(left >= 0 && left + CC7_TRACK_SQUARE_SIZE <= i64::from(CC7_SOURCE_WIDTH));
            assert!(top >= 0 && top + CC7_TRACK_SQUARE_SIZE <= i64::from(CC7_SOURCE_HEIGHT));
        }

        // Failing direction, §11.2.16's own words: **a source with the square
        // drawn on every frame reports `product_red` at 47**. The raster
        // function is pure, so the alternative source is one line — the same
        // geometry with the occlusion suppressed — and it is measured at the
        // frame-47 centre rather than argued about.
        let always_drawn = |x: u32, y: u32, frame: u32| -> [u8; 3] {
            let (left, top) = cc7_analytic_square_top_left(i64::from(frame));
            let inside_x = i64::from(x) >= left && i64::from(x) < left + CC7_TRACK_SQUARE_SIZE;
            let inside_y = i64::from(y) >= top && i64::from(y) < top + CC7_TRACK_SQUARE_SIZE;
            if inside_x && inside_y {
                return CC7_ROW_PATCHES[4].display_code_cam_a;
            }
            cc7_tracked_scene_rgb(x, y, frame)
        };
        let occluded_last =
            u32::try_from(CC7_TRACK_OCCLUSION_LAST_FRAME).expect("a non-negative frame");
        let (left, top) = cc7_analytic_square_top_left(CC7_TRACK_OCCLUSION_LAST_FRAME);
        let half = CC7_TRACK_SQUARE_SIZE / 2;
        let centre_x = u32::try_from(left + half).expect("an in-frame centre");
        let centre_y = u32::try_from(top + half).expect("an in-frame centre");
        assert_eq!(
            always_drawn(centre_x, centre_y, occluded_last),
            product_red,
            "a source drawn on every frame reports product_red at 47"
        );
        assert_eq!(
            cc7_tracked_scene_rgb(centre_x, centre_y, occluded_last),
            [CC7_SURROUND_CODE; 3],
            "this source does not, so the occlusion is a property of the raster"
        );
        // …and it is exactly the five contract frames that are suppressed.
        let occluded_frames = (0..CC7_TRACK_FRAMES)
            .filter(|frame| !kinewright_core::cc7_scenarios::cc7_square_is_drawn(i64::from(*frame)))
            .collect::<Vec<_>>();
        assert_eq!(occluded_frames, vec![43, 44, 45, 46, 47]);
    }

    // -----------------------------------------------------------------------
    // §3.5(4) / §11.2.17 — key `sources.tracking`.
    // -----------------------------------------------------------------------

    /// §2.3.6's generator bounds over all 100 frames: the square never covers
    /// the four static skin patches at `y 4..20`.
    #[test]
    fn cc7_tracked_square_never_covers_the_static_patch_row() {
        for frame in 0..CC7_TRACK_FRAMES {
            let (left, top) = cc7_analytic_square_top_left(i64::from(frame));
            assert!(left >= 0, "frame {frame}");
            assert!(
                left + CC7_TRACK_SQUARE_SIZE <= i64::from(CC7_SOURCE_WIDTH),
                "frame {frame}"
            );
            assert!(top >= CC7_TRACK_SQUARE_SIZE, "frame {frame}");
            assert!(
                top + CC7_TRACK_SQUARE_SIZE <= i64::from(CC7_SOURCE_HEIGHT),
                "frame {frame}"
            );

            // The static patches are still themselves at every frame.
            for (index, patch) in CC7_ROW_PATCHES.iter().take(4).enumerate() {
                let x = u32::try_from(index).expect("a patch index") * 12 + 6;
                assert_eq!(
                    cc7_tracked_scene_rgb(x, 12, frame),
                    patch.display_code_cam_a,
                    "frame {frame}: {} must never be covered",
                    patch.name
                );
            }
        }
        // The tracked raster carries the four skin patches and nothing else
        // from the base scene's bands.
        assert_eq!(cc7_tracked_scene_rgb(60, 12, 0), [CC7_SURROUND_CODE; 3]);
        assert_eq!(cc7_tracked_scene_rgb(4, 40, 0), [CC7_SURROUND_CODE; 3]);
    }

    // -----------------------------------------------------------------------
    // §3.5(5) / §11.2.18 — key `sources.lossless`.
    // -----------------------------------------------------------------------

    /// One generated `.mkv` per generator, re-decoded and compared byte-exact
    /// against the authored `yuv444p` planes, in `verify_native_ramp`'s shape.
    ///
    /// *Fails:* a `libx264 -crf 23` mux of the same planes is asserted **not**
    /// byte-exact, so the FFV1 claim is a measurement rather than a label.
    #[test]
    fn cc7_ffv1_round_trip_is_byte_exact() {
        /// One generator: its declared name, the raster it writes, and the
        /// public entry point CC7 §3.1 names.
        type Cc7Generator = (
            &'static str,
            Cc7SourceKind,
            fn() -> crate::test_support::GeneratedMedia,
        );
        let generators: [Cc7Generator; 3] = [
            (
                "cc7_camera_source",
                Cc7SourceKind::Camera(Cc7Camera::B),
                || cc7_camera_source(Cc7Camera::B),
            ),
            ("cc7_log_source", Cc7SourceKind::Log, cc7_log_source),
            (
                "cc7_tracked_source",
                Cc7SourceKind::Tracked,
                cc7_tracked_source,
            ),
        ];
        for (name, kind, generate) in generators {
            let expected = cc7_source_planes(kind);
            let media = generate();
            let decoded = decode_yuv444p(media.path());
            assert_eq!(
                decoded.len(),
                expected.len(),
                "{name}: the decoded byte length changed"
            );
            assert_eq!(decoded, expected, "{name}: FFV1 did not round-trip");
        }

        // Failing direction: the same planes through `libx264 -crf 23`.
        let kind = Cc7SourceKind::Camera(Cc7Camera::B);
        let expected = cc7_source_frame_planes(kind, 0);
        // The same uniquely named, `Drop`-removed temp file the generator
        // uses, so a panic here leaks nothing either.
        let raw = super::RawFrames::write("cc7-lossy-control", &expected);
        let size = format!("{CC7_SOURCE_WIDTH}x{CC7_SOURCE_HEIGHT}");
        let input = raw.input();
        let lossy = crate::test_support::GeneratedMedia::ffmpeg(
            "cc7-lossy-control",
            &[
                "-f",
                "rawvideo",
                "-pix_fmt",
                "yuv444p",
                "-s",
                &size,
                "-r",
                "25",
                "-i",
                &input,
                "-c:v",
                "libx264",
                "-crf",
                "23",
                "-pix_fmt",
                "yuv444p",
                "-frames:v",
                "1",
            ],
            "mkv",
        );
        let decoded = decode_yuv444p(lossy.path());
        drop(raw);
        assert_eq!(decoded.len(), expected.len());
        assert_ne!(
            decoded, expected,
            "a lossy encode must not round-trip byte-exact, or the FFV1 claim measures nothing"
        );
    }

    // -----------------------------------------------------------------------
    // §3.4 — the authored `.cube`, and the byte count the manifest records.
    // -----------------------------------------------------------------------

    /// The canonical `.cube` is well-formed, parses, carries the pinned size,
    /// and measures `CC7_LOG_CUBE_BYTES_REPORTED` bytes.
    #[test]
    fn cc7_log_like_inverse_cube_is_canonical_text_of_the_pinned_size() {
        for (size, bytes) in [(17_u32, 132_766_i64), (33, 970_414), (65, 7_414_990)] {
            let text = log_like_inverse_cube(size);
            assert_eq!(
                i64::try_from(text.len()).expect("a cube fits in an i64"),
                bytes,
                "the size {size} cube's byte count"
            );
            let parsed = parse_cube_lut(&text).expect("a well-formed .cube parses");
            assert_eq!(parsed.size, size);
            assert_eq!(parsed.title.as_deref(), Some(CC7_LOG_CUBE_TITLE));
        }
        assert_eq!(
            CC7_LOG_CUBE_TITLE.len(),
            15,
            "the header is 100 + title.len() for a two-digit LUT_3D_SIZE, which all three of these are"
        );
        assert_eq!(
            i64::try_from(log_like_inverse_cube(super::CC7_CANONICAL_CUBE_SIZE).len())
                .expect("a cube fits in an i64"),
            CC7_LOG_CUBE_BYTES_REPORTED
        );
        assert!(
            CC7_LOG_CUBE_BYTES_REPORTED
                < i64::try_from(LUT_MAX_FILE_BYTES).expect("the store limit fits in an i64"),
            "the canonical cube must fit inside LUT_MAX_FILE_BYTES"
        );

        // The output clamp is kept: no sample leaves the `[0, 1]` domain.
        for line in log_like_inverse_cube(17).lines().skip(4) {
            for field in line.split_whitespace() {
                let value: f64 = field.parse().expect("a six-decimal sample");
                assert!(
                    (0.0..=1.0).contains(&value),
                    "a .cube whose domain is [0, 1] and whose outputs leave it is not well formed"
                );
            }
        }

        // The identity twin is the same shape, so the (c) failing direction
        // differs only in its transfer.
        let identity = identity_cube(33);
        assert_eq!(identity.len(), 970_414);
        assert_ne!(identity, log_like_inverse_cube(33));
        assert!(parse_cube_lut(&identity).is_ok());
    }
}
