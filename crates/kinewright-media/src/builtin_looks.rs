//! Built-in generated LUT assets: the four legacy looks plus identity (CC4 §2.6).
//!
//! Each look is baked in the binary from the closed-form formula the legacy
//! shader used, serialized through the pinned canonical `.cube` serializer, and
//! hashed. The hashes are pinned literals here and re-asserted by the fixture,
//! so changing a bake is a visible test failure and never a silent re-render.
//!
//! Built-in bytes are never written to the project store: a built-in asset is
//! `verified` exactly when this binary's bake hashes to the recorded sha256.

use std::sync::{Arc, LazyLock};

use kinewright_core::{LutAsset, LutAssetId, LutAssetKind, LutAssetSource};

use crate::{lut::CubeLut, sha256::sha256_bytes};

/// Lattice edge length of the four creative bakes (CC4 §2.6).
pub const BUILTIN_LOOK_SIZE: u32 = 17;
/// Lattice edge length of the identity bake.
pub const BUILTIN_IDENTITY_SIZE: u32 = 2;
/// `DOMAIN_MIN` of the four creative bakes, chosen so the CC3 §10.2 raster
/// lies inside the lattice on every channel.
pub const BUILTIN_LOOK_DOMAIN_MIN: f32 = -1.0;
/// `DOMAIN_MAX` of the four creative bakes.
pub const BUILTIN_LOOK_DOMAIN_MAX: f32 = 2.0;

/// The pinned content hash of every built-in bake's canonical text (CC4 §2.6).
///
/// These are literals, not computed constants: `builtin_bakes_match_pinned_hashes`
/// re-derives them from the live bake, so a formula, lattice, domain, or
/// serializer change fails the suite instead of silently re-rendering an old
/// project.
pub const BUILTIN_LOOK_SHA256: [(&str, &str); 5] = [
    (
        "identity",
        "b17322738cb6529fd17b1b14998358fcd9d43f3a37699397625cd634d9b3e38b",
    ),
    (
        "warm",
        "d4f7821f9cd58556be4264574a6f6dd5fdd32a18c34197e6a3773f979105e0d1",
    ),
    (
        "cool",
        "e3f80aa611c97bcedc428be4862cd4aadb94cdf66a3c6f0d7aac87d365ffee73",
    ),
    (
        "monochrome",
        "549d9311993e5adfcbdb49ff0ef67cd39f6f724b338012be9e8e54d0b734b268",
    ),
    (
        "bleach_bypass",
        "5dc52536b14f0fbbedfcd3465ee656372c7d0456d81fe3f33d8e1163ff256a0e",
    ),
];

/// Rec.709 luma weights used by the channel-mixing looks (CC4 §2.6).
const LUMA_WEIGHTS: [f64; 3] = [0.2126, 0.7152, 0.0722];

/// One built-in generated LUT asset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum BuiltinLook {
    /// `L(e) = e`, baked at `S = 2` over `[0, 1]`.
    Identity,
    /// Legacy `look_lut` preset token 1.
    Warm,
    /// Legacy `look_lut` preset token 2.
    Cool,
    /// Legacy `look_lut` preset token 3.
    Monochrome,
    /// Legacy `look_lut` preset token 4.
    BleachBypass,
}

impl BuiltinLook {
    /// Every built-in, in catalogue order. The index into this array is also
    /// the index into [`BUILTIN_LOOK_SHA256`].
    pub const ALL: [Self; 5] = [
        Self::Identity,
        Self::Warm,
        Self::Cool,
        Self::Monochrome,
        Self::BleachBypass,
    ];

    /// The stable name recorded in [`LutAssetSource::Builtin`].
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Identity => "identity",
            Self::Warm => "warm",
            Self::Cool => "cool",
            Self::Monochrome => "monochrome",
            Self::BleachBypass => "bleach_bypass",
        }
    }

    /// The human title CC4 §2.6 coins for the look.
    #[must_use]
    pub const fn title(self) -> &'static str {
        match self {
            Self::Identity => "Identity",
            Self::Warm => "Warm",
            Self::Cool => "Cool",
            Self::Monochrome => "Monochrome",
            Self::BleachBypass => "Bleach bypass",
        }
    }

    /// The `TITLE` keyword written into the canonical text.
    #[must_use]
    pub const fn cube_title(self) -> &'static str {
        match self {
            Self::Identity => "kinewright.look.identity.v1",
            Self::Warm => "kinewright.look.warm.v1",
            Self::Cool => "kinewright.look.cool.v1",
            Self::Monochrome => "kinewright.look.monochrome.v1",
            Self::BleachBypass => "kinewright.look.bleach_bypass.v1",
        }
    }

    /// The legacy `look_lut` preset token mapping (CC4 §2.6), normative.
    ///
    /// The legacy descriptor's range is `0..=4` with neutral `0`, so token `0`
    /// maps to the identity bake and a neutral legacy node converts to a
    /// managed node that is numerically the identity.
    #[must_use]
    pub const fn from_preset_token(token: i64) -> Option<Self> {
        match token {
            0 => Some(Self::Identity),
            1 => Some(Self::Warm),
            2 => Some(Self::Cool),
            3 => Some(Self::Monochrome),
            4 => Some(Self::BleachBypass),
            _ => None,
        }
    }

    /// Resolve a [`LutAssetSource::Builtin`] name back to a look.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|look| look.name() == name)
    }

    /// The lattice edge length this look is baked at.
    #[must_use]
    pub const fn size(self) -> u32 {
        match self {
            Self::Identity => BUILTIN_IDENTITY_SIZE,
            _ => BUILTIN_LOOK_SIZE,
        }
    }

    /// The bake domain, `[0, 1]` for identity and `[-1, 2]` for the looks.
    #[must_use]
    pub const fn domain(self) -> (f32, f32) {
        match self {
            Self::Identity => (0.0, 1.0),
            _ => (BUILTIN_LOOK_DOMAIN_MIN, BUILTIN_LOOK_DOMAIN_MAX),
        }
    }

    /// The closed-form look, evaluated in f64 on display-coded RGB (CC4 §2.6).
    ///
    /// No clamp: CC1 §2.2 invariant 5 forbids an intermediate clamp, which is
    /// the documented behaviour difference from the legacy stage.
    #[must_use]
    pub fn formula(self, e: [f64; 3]) -> [f64; 3] {
        match self {
            Self::Identity => e,
            Self::Warm => [
                (e[0] - 0.5) * 1.08 + 0.54,
                (e[1] - 0.5) * 1.08 + 0.50,
                (e[2] - 0.5) * 1.08 + 0.46,
            ],
            Self::Cool => [
                (e[0] - 0.5) * 1.12 + 0.46,
                (e[1] - 0.5) * 1.12 + 0.50,
                (e[2] - 0.5) * 1.12 + 0.55,
            ],
            Self::Monochrome => {
                let luma = luma(e);
                [luma, luma, luma]
            }
            Self::BleachBypass => {
                let luma = luma(e);
                let mixed = e.map(|channel| luma + (channel - luma) * 0.35);
                mixed.map(|channel| (channel - 0.5) * 1.35 + 0.5)
            }
        }
    }

    /// Bake the look into a lattice, red-fastest, values computed in f64 and
    /// stored as f32.
    ///
    /// This recomputes on every call so the determinism fixture compares two
    /// independent bakes. Render paths use [`BuiltinLook::cached_bake`].
    #[must_use]
    #[allow(clippy::cast_possible_truncation)]
    pub fn bake(self) -> CubeLut {
        let size = self.size();
        let (minimum, maximum) = self.domain();
        let points = usize::try_from(size).unwrap_or(0).saturating_pow(3);
        let mut rgba = Vec::with_capacity(points.saturating_mul(4));
        for blue in 0..size {
            for green in 0..size {
                for red in 0..size {
                    let encoded = [
                        lattice_coordinate(red, size, minimum, maximum),
                        lattice_coordinate(green, size, minimum, maximum),
                        lattice_coordinate(blue, size, minimum, maximum),
                    ];
                    let value = self.formula(encoded);
                    rgba.extend_from_slice(&[
                        value[0] as f32,
                        value[1] as f32,
                        value[2] as f32,
                        1.0,
                    ]);
                }
            }
        }
        CubeLut {
            size,
            domain_min: [minimum; 3],
            domain_max: [maximum; 3],
            rgba,
            title: Some(self.cube_title().to_owned()),
        }
    }

    /// The process-wide cached bake, computed once.
    #[must_use]
    pub fn cached_bake(self) -> Arc<CubeLut> {
        Arc::clone(&BAKES[self.index()].lut)
    }

    /// The pinned canonical `.cube` text of this look (CC4 §2.6), computed once.
    #[must_use]
    pub fn canonical_text(self) -> &'static str {
        &BAKES[self.index()].text
    }

    /// The sha256 of [`BuiltinLook::canonical_text`], computed once.
    #[must_use]
    pub fn sha256(self) -> &'static str {
        &BAKES[self.index()].sha256
    }

    /// The pinned literal hash from [`BUILTIN_LOOK_SHA256`].
    #[must_use]
    pub const fn pinned_sha256(self) -> &'static str {
        BUILTIN_LOOK_SHA256[self.index()].1
    }

    /// The byte length of the canonical text, the `byte_len` a record carries.
    #[must_use]
    pub fn byte_len(self) -> u64 {
        self.canonical_text().len() as u64
    }

    /// The project record for this built-in, carrying the pinned hash.
    #[must_use]
    pub fn to_lut_asset(self, id: LutAssetId) -> LutAsset {
        let (domain_min_millionths, domain_max_millionths) = self.cached_bake().domain_millionths();
        LutAsset {
            id,
            sha256: self.pinned_sha256().to_owned(),
            title: self.title().to_owned(),
            kind: LutAssetKind::Cube3d,
            size: self.size(),
            byte_len: self.byte_len(),
            domain_min_millionths,
            domain_max_millionths,
            source: LutAssetSource::Builtin {
                name: self.name().to_owned(),
            },
        }
    }

    /// Index into [`BuiltinLook::ALL`], [`BUILTIN_LOOK_SHA256`], and [`BAKES`].
    const fn index(self) -> usize {
        match self {
            Self::Identity => 0,
            Self::Warm => 1,
            Self::Cool => 2,
            Self::Monochrome => 3,
            Self::BleachBypass => 4,
        }
    }
}

/// Rec.709 luma of a display-coded triple.
fn luma(e: [f64; 3]) -> f64 {
    LUMA_WEIGHTS[0] * e[0] + LUMA_WEIGHTS[1] * e[1] + LUMA_WEIGHTS[2] * e[2]
}

/// The encoded value at one lattice index, evaluated in f64.
fn lattice_coordinate(index: u32, size: u32, minimum: f32, maximum: f32) -> f64 {
    let last = f64::from(size.saturating_sub(1)).max(1.0);
    let position = f64::from(index) / last;
    let minimum = f64::from(minimum);
    minimum + (f64::from(maximum) - minimum) * position
}

/// One memoized bake: the lattice, its canonical text, and the text's hash.
struct BakedLook {
    lut: Arc<CubeLut>,
    text: String,
    sha256: String,
}

/// The five bakes, computed once per process.
static BAKES: LazyLock<[BakedLook; 5]> = LazyLock::new(|| {
    BuiltinLook::ALL.map(|look| {
        let lut = look.bake();
        let text = lut.canonical_text(look.cube_title());
        let sha256 = sha256_bytes(text.as_bytes());
        BakedLook {
            lut: Arc::new(lut),
            text,
            sha256,
        }
    })
});

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lut::parse_cube_lut_typed;

    /// Rec.709 luma, transcribed independently from CC4 §2.6 for the fixtures.
    fn expected_luma(e: [f64; 3]) -> f64 {
        0.2126 * e[0] + 0.7152 * e[1] + 0.0722 * e[2]
    }

    /// The §2.6 formulas transcribed independently of [`BuiltinLook::formula`].
    fn expected_formula(look: BuiltinLook, e: [f64; 3]) -> [f64; 3] {
        match look {
            BuiltinLook::Identity => e,
            BuiltinLook::Warm => [
                (e[0] - 0.5) * 1.08 + 0.54,
                (e[1] - 0.5) * 1.08 + 0.50,
                (e[2] - 0.5) * 1.08 + 0.46,
            ],
            BuiltinLook::Cool => [
                (e[0] - 0.5) * 1.12 + 0.46,
                (e[1] - 0.5) * 1.12 + 0.50,
                (e[2] - 0.5) * 1.12 + 0.55,
            ],
            BuiltinLook::Monochrome => {
                let luma = expected_luma(e);
                [luma, luma, luma]
            }
            BuiltinLook::BleachBypass => {
                let luma = expected_luma(e);
                let mixed = [
                    luma + (e[0] - luma) * 0.35,
                    luma + (e[1] - luma) * 0.35,
                    luma + (e[2] - luma) * 0.35,
                ];
                [
                    (mixed[0] - 0.5) * 1.35 + 0.5,
                    (mixed[1] - 0.5) * 1.35 + 0.5,
                    (mixed[2] - 0.5) * 1.35 + 0.5,
                ]
            }
        }
    }

    #[test]
    fn builtin_bakes_match_pinned_hashes() {
        for (index, look) in BuiltinLook::ALL.into_iter().enumerate() {
            let text = look.bake().canonical_text(look.cube_title());
            let observed = sha256_bytes(text.as_bytes());
            assert_eq!(BUILTIN_LOOK_SHA256[index].0, look.name());
            assert_eq!(
                observed,
                look.pinned_sha256(),
                "the {} bake no longer hashes to its pinned literal",
                look.name()
            );
            assert_eq!(look.sha256(), look.pinned_sha256());
            assert_eq!(look.byte_len(), text.len() as u64);
        }
    }

    /// The bake domains are exact binary fractions, so exact comparison is
    /// the determinism assertion the contract wants.
    #[test]
    #[allow(clippy::float_cmp)]
    fn two_bakes_are_byte_identical() {
        for look in BuiltinLook::ALL {
            let first = look.bake();
            let second = look.bake();
            assert_eq!(first.size, second.size);
            assert_eq!(first.domain_min, second.domain_min);
            assert_eq!(first.domain_max, second.domain_max);
            let first_bits = first
                .rgba
                .iter()
                .map(|value| value.to_bits())
                .collect::<Vec<_>>();
            let second_bits = second
                .rgba
                .iter()
                .map(|value| value.to_bits())
                .collect::<Vec<_>>();
            assert_eq!(
                first_bits,
                second_bits,
                "{} bake is not deterministic",
                look.name()
            );
            assert_eq!(
                first.canonical_text(look.cube_title()),
                second.canonical_text(look.cube_title())
            );
        }
    }

    #[test]
    fn canonical_text_uses_lf_endings_and_six_decimals() {
        for look in BuiltinLook::ALL {
            let text = look.canonical_text();
            assert!(!text.contains('\r'), "{} must be LF only", look.name());
            assert!(
                text.ends_with('\n'),
                "{} must end with one newline",
                look.name()
            );
            assert!(
                !text.ends_with("\n\n"),
                "{} must have no trailing blank line",
                look.name()
            );
            let mut lines = text.lines();
            assert_eq!(
                lines.next().unwrap(),
                format!("TITLE \"{}\"", look.cube_title())
            );
            assert_eq!(
                lines.next().unwrap(),
                format!("LUT_3D_SIZE {}", look.size())
            );
            let (minimum, maximum) = look.domain();
            assert_eq!(
                lines.next().unwrap(),
                format!("DOMAIN_MIN {minimum:.6} {minimum:.6} {minimum:.6}")
            );
            assert_eq!(
                lines.next().unwrap(),
                format!("DOMAIN_MAX {maximum:.6} {maximum:.6} {maximum:.6}")
            );
            let points = usize::try_from(look.size()).unwrap().pow(3);
            assert_eq!(text.lines().count(), points + 4);
            for line in text.lines().skip(4) {
                let fields = line.split_whitespace().collect::<Vec<_>>();
                assert_eq!(fields.len(), 3, "sample line {line} must be a triple");
                for field in fields {
                    let decimals = field.split_once('.').map(|(_, rest)| rest.len());
                    assert_eq!(decimals, Some(6), "{field} must carry six decimals");
                }
            }
        }
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn the_four_looks_record_the_negative_one_to_two_domain() {
        for look in [
            BuiltinLook::Warm,
            BuiltinLook::Cool,
            BuiltinLook::Monochrome,
            BuiltinLook::BleachBypass,
        ] {
            let lut = look.cached_bake();
            assert_eq!(lut.size, 17);
            assert_eq!(lut.domain_min, [-1.0; 3]);
            assert_eq!(lut.domain_max, [2.0; 3]);
            assert_eq!(lut.domain_millionths(), ([-1_000_000; 3], [2_000_000; 3]));
        }
        let identity = BuiltinLook::Identity.cached_bake();
        assert_eq!(identity.size, 2);
        assert_eq!(identity.domain_min, [0.0; 3]);
        assert_eq!(identity.domain_max, [1.0; 3]);
    }

    #[test]
    fn bakes_reproduce_the_closed_form_formula_at_lattice_points() {
        for look in BuiltinLook::ALL {
            let lut = look.cached_bake();
            let size = look.size();
            let (minimum, maximum) = look.domain();
            let last = f64::from(size - 1);
            for (red, green, blue) in [
                (0_u32, 0_u32, 0_u32),
                (1 % size, 2 % size, 3 % size),
                (size - 1, 0, size / 2),
            ] {
                let encoded = [red, green, blue].map(|index| {
                    f64::from(minimum)
                        + (f64::from(maximum) - f64::from(minimum)) * (f64::from(index) / last)
                });
                let expected = expected_formula(look, encoded);
                let index = usize::try_from((blue * size + green) * size + red).unwrap();
                let observed = lut.sample(index).unwrap();
                for (channel, (observed, expected)) in
                    observed.into_iter().zip(expected).enumerate()
                {
                    let difference = f64::from(observed) - expected;
                    assert!(
                        difference.abs() <= 2e-6,
                        "{} channel {channel} at {encoded:?}: observed {observed}, expected {expected}",
                        look.name()
                    );
                }
            }
        }
    }

    #[test]
    fn identity_bake_is_the_exact_unit_cube() {
        let lut = BuiltinLook::Identity.cached_bake();
        assert_eq!(
            lut.rgba,
            vec![
                0.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0, 1.0, 1.0, 0.0, 1.0,
                0.0, 0.0, 1.0, 1.0, 1.0, 0.0, 1.0, 1.0, 0.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0,
            ]
        );
    }

    #[test]
    fn canonical_text_round_trips_through_the_production_parser() {
        for look in BuiltinLook::ALL {
            let text = look.canonical_text();
            let parsed = parse_cube_lut_typed(text).unwrap();
            assert_eq!(parsed.size, look.size());
            assert_eq!(parsed.title.as_deref(), Some(look.cube_title()));
            let crlf = text.replace('\n', "\r\n");
            let reparsed = parse_cube_lut_typed(&crlf).unwrap();
            assert_eq!(reparsed.rgba, parsed.rgba);
        }
    }

    #[test]
    fn preset_tokens_and_names_map_both_ways() {
        assert_eq!(BuiltinLook::from_preset_token(1), Some(BuiltinLook::Warm));
        assert_eq!(BuiltinLook::from_preset_token(2), Some(BuiltinLook::Cool));
        assert_eq!(
            BuiltinLook::from_preset_token(3),
            Some(BuiltinLook::Monochrome)
        );
        assert_eq!(
            BuiltinLook::from_preset_token(4),
            Some(BuiltinLook::BleachBypass)
        );
        assert_eq!(
            BuiltinLook::from_preset_token(0),
            Some(BuiltinLook::Identity)
        );
        assert_eq!(BuiltinLook::from_preset_token(5), None);
        assert_eq!(BuiltinLook::from_preset_token(-1), None);
        for look in BuiltinLook::ALL {
            assert_eq!(BuiltinLook::from_name(look.name()), Some(look));
        }
        assert_eq!(BuiltinLook::from_name("sepia"), None);
    }

    #[test]
    fn builtin_records_carry_the_pinned_hash_and_provenance() {
        let asset = BuiltinLook::Warm.to_lut_asset(LutAssetId(7));
        assert_eq!(asset.id, LutAssetId(7));
        assert_eq!(asset.sha256, BuiltinLook::Warm.pinned_sha256());
        assert_eq!(asset.title, "Warm");
        assert_eq!(asset.kind, LutAssetKind::Cube3d);
        assert_eq!(asset.size, 17);
        assert_eq!(asset.byte_len, BuiltinLook::Warm.byte_len());
        assert_eq!(asset.domain_min_millionths, [-1_000_000; 3]);
        assert_eq!(asset.domain_max_millionths, [2_000_000; 3]);
        assert_eq!(
            asset.source,
            LutAssetSource::Builtin {
                name: "warm".to_owned()
            }
        );
        assert!(kinewright_core::validate_lut_asset(&asset).is_ok());
    }
}
