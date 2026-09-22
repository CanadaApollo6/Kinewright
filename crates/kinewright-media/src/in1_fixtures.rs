//! IN1 §3 rule 13's nine media gates for
//! `docs/IN1-INCIDENTS-AND-THE-COLOUR-CASE.md`, plus §4.2 rule 11's
//! construction-site inventory.
//!
//! These fixtures live in the media crate for the reason every earlier
//! `ccN_fixtures.rs` does: `VideoDecoder::open_scaled_managed` and
//! `contextual_managed_decode_error` are `pub(crate)` seams, and the evidence
//! has to exercise the real decode path rather than a public
//! re-implementation of it. IN1 widens no public surface to reach them.
//!
//! What this file owns:
//!
//! * §3 rule 5 — the four pinned probed tuples, field by field, including the
//!   two named confidence constants and the two provenances that differ;
//! * §3 rule 7 — that both untagged sources refuse with the *same* code while
//!   differing in three of their eight description fields, and that the tagged
//!   twins classify **only** through the D65 assumption;
//! * §4.2 — that the untagged managed decode carries the typed refusal out to
//!   the caller with its recovery code intact, that the tagged twin decodes,
//!   and that `MediaError::SourceColor` and `MediaError::SourceColorForAsset`
//!   each have exactly one construction site in the crate;
//! * §9 clause 11 — the 708 B message template (`IN2B` §8 D-B4, erratum
//!   E-B5), pinned with the per-run temp path replaced at both of its two
//!   occurrences;
//! * §3 rule 14's fixture 9 — that the recovery actually recovers: after
//!   `recovery_description`, the same decode of the same file succeeds.
//!
//! Everything else IN1 declares is owned by the file that owns the code it
//! measures: `crates/kinewright-core/src/incident.rs` and
//! `crates/kinewright-core/tests/contracts.rs` (§2, §4.1, §4.3, §4.4), the app
//! (§5) and `crates/kinewright-agent/tests/mcp_server.rs` (§6, §7).
//!
//! None of these tests needs a window system, a network or an audio device.

use std::path::Path;

use kinewright_core::{
    AssetId, COLOR_CONFIDENCE_MAX_BASIS_POINTS, ColorBitDepth, ColorDescription, ColorMatrix,
    ColorPrimaries, ColorProvenance, ColorRange, ColorSourceError, ColorSourceProfile,
    ColorSourceProfileAssumption, ColorTransfer, ColorWhitePoint, MediaError, Rational,
    classify_source, classify_source_with_assumption, recovery_description,
};

use crate::{
    decode::{VideoDecoder, probe_path},
    in1_sources::{
        IN1_UNTAGGED_MP4_CONFIDENCE_BASIS_POINTS, IN1_UNTAGGED_WEBM_CONFIDENCE_BASIS_POINTS,
        In1Source, in1_source,
    },
    initialize_ffmpeg,
    render::{PREVIEW_MAX_WIDTH, contextual_managed_decode_error, d65_assumption},
    test_support::GeneratedMedia,
};

/// IN1 §9 clause 11's template: the rendered `Display` of the managed-decode
/// failure on `in1_untagged.mp4` at `c3a5814`, with the per-run temp path
/// replaced by `{path}` at **both** of its two occurrences.
///
/// A rendered literal is undischargeable — every IN1 fixture is a
/// [`GeneratedMedia`] under a unique temp stem, so the path differs per run and
/// per operating system, and §1 item 9 forbids a per-OS constant. The
/// `replace` form pins every byte that is not the path and survives Windows'
/// `\` separators.
///
/// The same 708 bytes are pinned on the core side against
/// `SourceColorRefusal`'s `#[error(...)]` template; this copy is what proves
/// the **production path** renders them, which is the half core cannot see.
const IN1_MANAGED_DECODE_REFUSAL: &str = concat!(
    r#"managed decode for asset 1 ({path}) failed: managed source profile rejected "#,
    r#"for {path} (assumption=None): source colour "#,
    r#"primaries are unknown [source_color=unknown_source_primaries, field=primaries, "#,
    r#"observed=unknown, allowed=bt709 or srgb in a supported CC1 profile, recovery=Apply "#,
    r#"an explicit supported source-colour override or relink to compatible media., "#,
    r#"assumption=None, description=ColorDescription { primaries: Unknown, transfer: "#,
    r#"Unknown, matrix: Unknown, range: Unknown, white_point: Unknown, bit_depth: Eight, "#,
    r#"confidence_basis_points: 2000, provenance: Inferred }]. Recovery: apply an explicit "#,
    r#"supported source-colour override, transcode to a supported integer format, or relink"#,
    r#" to compatible media."#,
);

/// One fixture, generated and probed through the production probe.
struct In1Probe {
    media: GeneratedMedia,
    fps: Rational,
    description: ColorDescription,
}

impl In1Probe {
    /// Generate `source` and probe it as `AssetId(1)`, exactly as an import
    /// would.
    fn open(source: In1Source) -> Self {
        initialize_ffmpeg().expect("FFmpeg must initialize for IN1 media fixtures");
        let media = in1_source(source);
        let asset = probe_path(media.path(), AssetId(1)).unwrap_or_else(|error| {
            panic!("{} must probe: {error}", source.file_name());
        });
        Self {
            media,
            fps: asset.fps,
            description: asset.color_description,
        }
    }

    /// The managed decode `Renderer::decode_video_frame` performs, with the
    /// contextual wrap it applies, over `description`.
    fn managed_decode(&self, description: &ColorDescription) -> Result<(), MediaError> {
        let assumption = d65_assumption(description);
        VideoDecoder::open_scaled_managed(
            self.media.path(),
            self.fps,
            Some(PREVIEW_MAX_WIDTH),
            description,
            assumption,
        )
        .map(drop)
        .map_err(|error| {
            contextual_managed_decode_error(
                AssetId(1),
                self.media.path(),
                description,
                assumption,
                error,
            )
        })
    }
}

/// IN1 §3 rule 5 row 1.
#[test]
fn in1_untagged_mp4_probes_the_pinned_tuple() {
    let probe = In1Probe::open(In1Source::UntaggedMp4);
    let description = &probe.description;
    assert_eq!(description.primaries, ColorPrimaries::Unknown);
    assert_eq!(description.transfer, ColorTransfer::Unknown);
    assert_eq!(description.matrix, ColorMatrix::Unknown);
    assert_eq!(description.range, ColorRange::Unknown);
    assert_eq!(description.white_point, ColorWhitePoint::Unknown);
    assert_eq!(description.bit_depth, ColorBitDepth::Eight);
    assert_eq!(
        description.confidence_basis_points,
        IN1_UNTAGGED_MP4_CONFIDENCE_BASIS_POINTS
    );
    assert_eq!(description.provenance, ColorProvenance::Inferred);
}

/// IN1 §3 rule 5 row 3 — the Helen Hill row, whose `range` and provenance are
/// the container's doing and not the codec's.
#[test]
fn in1_untagged_webm_probes_the_pinned_tuple() {
    let probe = In1Probe::open(In1Source::UntaggedWebm);
    let description = &probe.description;
    assert_eq!(description.primaries, ColorPrimaries::Unknown);
    assert_eq!(description.transfer, ColorTransfer::Unknown);
    assert_eq!(description.matrix, ColorMatrix::Unknown);
    assert_eq!(
        description.range,
        ColorRange::Limited,
        "the Matroska/WebM muxer always writes a Colour/Range element"
    );
    assert_eq!(description.white_point, ColorWhitePoint::Unknown);
    assert_eq!(description.bit_depth, ColorBitDepth::Eight);
    assert_eq!(
        description.confidence_basis_points,
        IN1_UNTAGGED_WEBM_CONFIDENCE_BASIS_POINTS
    );
    assert_eq!(description.provenance, ColorProvenance::StreamMetadata);
}

/// IN1 §3 rule 5 rows 2 and 4. The white point is `Unknown` on **both**, which
/// is the fact behind §2.2 rule 6: `color_description_from_decoder` never sets
/// one, so every correctly tagged source in existence fails the bare
/// classifier with `unknown_source_white_point`.
#[test]
fn in1_tagged_twins_probe_bt709_with_an_unknown_white_point() {
    for source in [In1Source::TaggedMp4, In1Source::TaggedWebm] {
        let probe = In1Probe::open(source);
        let description = &probe.description;
        let named = source.file_name();
        assert_eq!(description.primaries, ColorPrimaries::Bt709, "{named}");
        assert_eq!(description.transfer, ColorTransfer::Bt709, "{named}");
        assert_eq!(description.matrix, ColorMatrix::Bt709, "{named}");
        assert_eq!(description.range, ColorRange::Limited, "{named}");
        assert_eq!(description.white_point, ColorWhitePoint::Unknown, "{named}");
        assert_eq!(description.bit_depth, ColorBitDepth::Eight, "{named}");
        assert_eq!(
            description.confidence_basis_points, COLOR_CONFIDENCE_MAX_BASIS_POINTS,
            "{named}"
        );
        assert_eq!(
            description.provenance,
            ColorProvenance::StreamMetadata,
            "{named}"
        );
    }
}

/// IN1 §3 rule 7: one code, one field, one observed value and one allowed
/// phrase for both containers, from two descriptions that differ in three of
/// their eight fields. The card headline is identical for both by construction
/// because of exactly this.
#[test]
fn in1_both_untagged_sources_refuse_with_the_same_code() {
    let mp4 = In1Probe::open(In1Source::UntaggedMp4);
    let webm = In1Probe::open(In1Source::UntaggedWebm);

    let differing = [
        mp4.description.range != webm.description.range,
        mp4.description.confidence_basis_points != webm.description.confidence_basis_points,
        mp4.description.provenance != webm.description.provenance,
    ];
    assert_eq!(
        differing.iter().filter(|differs| **differs).count(),
        3,
        "the two untagged descriptions must differ in exactly these three fields"
    );
    assert_eq!(mp4.description.primaries, webm.description.primaries);
    assert_eq!(mp4.description.transfer, webm.description.transfer);
    assert_eq!(mp4.description.matrix, webm.description.matrix);
    assert_eq!(mp4.description.white_point, webm.description.white_point);
    assert_eq!(mp4.description.bit_depth, webm.description.bit_depth);

    for probe in [&mp4, &webm] {
        let error =
            classify_source(&probe.description).expect_err("an untagged source must not classify");
        assert_eq!(error.code(), "unknown_source_primaries");
        assert_eq!(error.field(), "primaries");
        assert_eq!(error.observed(), "unknown");
        assert_eq!(
            error.allowed_values(),
            "bt709 or srgb in a supported CC1 profile"
        );
    }
}

/// IN1 §3 rule 7's second half: a tagged twin reaches `Rec709Video` **only**
/// through the D65 assumption the decoder already applies.
#[test]
fn in1_tagged_twins_classify_only_through_the_d65_assumption() {
    for source in [In1Source::TaggedMp4, In1Source::TaggedWebm] {
        let probe = In1Probe::open(source);
        let named = source.file_name();
        let bare = classify_source(&probe.description)
            .expect_err("a probed BT.709 source carries no white point");
        assert_eq!(bare, ColorSourceError::UnknownWhitePoint, "{named}");
        assert_eq!(bare.code(), "unknown_source_white_point", "{named}");
        assert_eq!(bare.field(), "white_point", "{named}");
        assert_eq!(
            d65_assumption(&probe.description),
            Some(ColorSourceProfileAssumption::D65),
            "{named}"
        );
        assert_eq!(
            classify_source_with_assumption(
                &probe.description,
                Some(ColorSourceProfileAssumption::D65)
            ),
            Ok(ColorSourceProfile::Rec709Video),
            "{named}"
        );
    }
}

/// IN1 §3 rule 13 fixture 6, and §4.2's whole point: the refusal leaves the
/// production decode path **typed**, with the asset, the path, the probed
/// description and the assumption attached, so the app can build an incident
/// without re-probing and without parsing a sentence.
#[test]
fn in1_the_untagged_managed_decode_carries_the_typed_refusal() {
    let probe = In1Probe::open(In1Source::UntaggedMp4);
    let error = probe
        .managed_decode(&probe.description)
        .expect_err("an untagged source must not decode managed");
    let MediaError::SourceColorForAsset(refusal) = &error else {
        panic!("the managed decode must refuse with SourceColorForAsset, got {error:?}");
    };
    assert_eq!(refusal.asset, AssetId(1));
    assert_eq!(refusal.path, probe.media.path());
    assert_eq!(refusal.error.code(), "unknown_source_primaries");
    assert_eq!(refusal.description, probe.description);
    assert_eq!(refusal.assumption, None);
    assert_eq!(error.recovery_code(), Some("unknown_source_primaries"));
}

/// IN1 §9 clause 11: the message did not move, pinned as a template because it
/// interpolates a per-run temp path twice.
#[test]
fn in1_the_refusal_message_is_byte_identical_to_c3a5814() {
    let probe = In1Probe::open(In1Source::UntaggedMp4);
    let error = probe
        .managed_decode(&probe.description)
        .expect_err("an untagged source must not decode managed");
    let path = probe.media.path().display().to_string();
    let message = error.to_string();
    assert_eq!(
        message.matches(&path).count(),
        2,
        "the historical sentence interpolates the path twice: {message}"
    );
    assert_eq!(message.replace(&path, "{path}"), IN1_MANAGED_DECODE_REFUSAL);
    assert_eq!(IN1_MANAGED_DECODE_REFUSAL.len(), 708);
    println!(
        "IN1 rendered refusal: {} B with a {} B path twice; template {} B",
        message.len(),
        path.len(),
        IN1_MANAGED_DECODE_REFUSAL.len()
    );
}

/// IN1 §3 rule 13 fixture 8, the negative: the tagged twins decode managed and
/// raise nothing at all.
///
/// Fixture 8 names `in1_tagged.mp4` alone, but §3 rule 7 says **both** tagged
/// twins must reach `Ok(Rec709Video)` through the decoder's own D65 assumption,
/// and probe-2 §R9 discharged both through `open_scaled_managed`. The `WebM` twin
/// is already interned by the time this runs, so the second decode is free.
#[test]
fn in1_the_tagged_mp4_decodes_managed() {
    for source in [In1Source::TaggedMp4, In1Source::TaggedWebm] {
        let probe = In1Probe::open(source);
        probe
            .managed_decode(&probe.description)
            .unwrap_or_else(|error| {
                panic!(
                    "{} must decode managed through the D65 assumption: {error}",
                    source.file_name()
                )
            });
    }
}

/// IN1 §3 rule 13 fixture 9 — the one §3 rule 14 says must not be cut. Without
/// it the slice would prove that an incident was opened, classified and marked
/// resolved, and would never prove that the frame appears.
///
/// Fixture 9 is worded "after `assume_rec709_operation`", and this test applies
/// [`recovery_description`] instead. They carry the same bytes:
/// `assume_rec709_operation` only wraps `recovery_description` in a
/// `SetAssetColorDescription` (`incident.rs:761-766`), and a media fixture has
/// no `Document` to apply an `Operation` to — applying the operation is the
/// app's §9 clause 3 test. Substituting the builder the operation carries is
/// the only honest form here.
#[test]
fn in1_the_assumed_description_decodes_managed() {
    let probe = In1Probe::open(In1Source::UntaggedMp4);
    probe
        .managed_decode(&probe.description)
        .expect_err("the probed description must refuse first, or the test proves nothing");

    let assumed = recovery_description(&probe.description);
    assert_eq!(assumed.provenance, ColorProvenance::AgentAssumption);
    assert_eq!(assumed.primaries, ColorPrimaries::Bt709);
    assert_eq!(assumed.white_point, ColorWhitePoint::D65);
    probe
        .managed_decode(&assumed)
        .expect("the recovery must actually recover: the same file decodes managed");
}

/// The one `match` arm in the crate that *destructures* `SourceColor` rather
/// than building one: `contextual_managed_decode_error`'s new arm
/// (`render.rs:710`). It is excluded by its exact text rather than by "contains
/// a fat arrow", because a construction on the **right-hand** side of an arm —
/// `Err(error) => MediaError::SourceColor(error),` — is exactly the site
/// §4.2 rule 11 exists to catch.
const IN1_SOURCE_COLOR_DESTRUCTURING_ARM: &str = "MediaError::SourceColor(error) => {";

/// IN1 §4.2 rule 11: one construction site each, so the typed refusal cannot
/// be minted anywhere the contract did not put it.
///
/// The whole crate is walked at test time rather than a hand-written file list,
/// so a module added by Part B — which migrates 125 error paths through
/// precisely these files — cannot escape the guard by not being listed. Only
/// the production half of each file is counted — everything before its
/// `#[cfg(test)] mod tests` — because the amended `render.rs` test names the
/// constructors on purpose, and the `#[cfg(test)]` fixture modules are skipped
/// outright for the same reason: they drive the production wrap and assert on
/// its typed errors, so their `matches!` patterns would read as extra sites.
#[test]
fn in1_the_typed_refusals_have_one_construction_site_each() {
    let sources = in1_crate_sources();
    assert!(
        sources.iter().any(|(name, _)| name == "decode.rs")
            && sources.iter().any(|(name, _)| name == "render.rs"),
        "the crate walk must reach the two files the contract names, found {} files",
        sources.len()
    );

    let mut source_color = Vec::new();
    let mut for_asset = Vec::new();
    let mut wrap_calls = Vec::new();
    for (name, source) in &sources {
        if name.ends_with("_fixtures.rs") {
            continue;
        }
        for line in in1_production_lines(source) {
            let line = line.trim();
            if line == IN1_SOURCE_COLOR_DESTRUCTURING_ARM {
                continue;
            }
            if line.contains("MediaError::SourceColorForAsset") {
                for_asset.push((name.as_str(), line));
            } else if line.contains("MediaError::SourceColor") {
                source_color.push((name.as_str(), line));
            }
            // S1: the fixtures above drive `open_scaled_managed` and apply the
            // wrap themselves, so they would still pass if the production call
            // lost its `.map_err`. This pins the seam they stand on.
            if line.contains("contextual_managed_decode_error(")
                && !line.starts_with("pub(crate) fn ")
            {
                wrap_calls.push((name.as_str(), line));
            }
        }
    }

    assert_eq!(
        source_color,
        [("decode.rs", ".map_err(MediaError::SourceColor)?;")],
        "MediaError::SourceColor is constructed only by `open_scaled_managed`"
    );
    assert_eq!(
        for_asset,
        [(
            "render.rs",
            "MediaError::SourceColorForAsset(Box::new(SourceColorRefusal {",
        )],
        "MediaError::SourceColorForAsset is constructed only by \
         `contextual_managed_decode_error`"
    );
    assert_eq!(
        wrap_calls,
        [(
            "render.rs",
            "contextual_managed_decode_error(asset, path, description, assumption, error)",
        )],
        "`Renderer::decode_video_frame` is the one production caller of the wrap"
    );
    println!(
        "IN1 §4.2 rule 11: {} crate source files walked",
        sources.len()
    );
}

/// Every `*.rs` file under this crate's `src/`, as `(file name, text)`, read at
/// test time and sorted by name.
///
/// Read from disk rather than `include_str!`ed so the set cannot fall behind
/// the crate: a module nobody listed is still walked.
fn in1_crate_sources() -> Vec<(String, String)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut found = Vec::new();
    in1_collect_sources(&root, &root, &mut found);
    found.sort();
    found
}

/// Push every `*.rs` file under `directory` onto `found`, recursing into
/// subdirectories, with each name relative to `root` and `/`-separated.
fn in1_collect_sources(root: &Path, directory: &Path, found: &mut Vec<(String, String)>) {
    let entries = std::fs::read_dir(directory)
        .unwrap_or_else(|error| panic!("{} must be readable: {error}", directory.display()));
    for entry in entries {
        let path = entry
            .unwrap_or_else(|error| panic!("{} must enumerate: {error}", directory.display()))
            .path();
        if path.is_dir() {
            in1_collect_sources(root, &path, found);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            let name = path
                .strip_prefix(root)
                .expect("every walked path sits under the crate's src/")
                .to_string_lossy()
                .replace('\\', "/");
            let text = std::fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("{} must be readable: {error}", path.display()));
            found.push((name, text));
        }
    }
}

/// The lines before a file's `#[cfg(test)] mod tests` block, or all of them
/// when it has none.
///
/// Line-based rather than byte-based on purpose: `str::lines()` strips a
/// trailing `\r`, so reconstructing a byte offset from line lengths undercounts
/// by one byte per line on a CRLF checkout — which `windows-latest` produces by
/// default, because the repository sets no `.gitattributes` and CI sets no
/// `core.autocrlf`.
fn in1_production_lines(source: &str) -> impl Iterator<Item = &str> {
    let mut take = source.lines().count();
    let mut lines = source.lines().enumerate().peekable();
    while let Some((index, line)) = lines.next() {
        if line.trim_end() == "#[cfg(test)]"
            && lines
                .peek()
                .is_some_and(|(_, next)| next.starts_with("mod tests"))
        {
            take = index;
            break;
        }
    }
    source.lines().take(take)
}
