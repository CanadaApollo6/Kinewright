//! CC8 §9's constants authority: the HDR transfer functions, the primaries
//! matrices, the reference-white anchor, and the §9.2 gate table.
//!
//! CC8 §9 names this module — `kinewright_core::cc8_hdr`, "in the manner of
//! `cc7_scenarios`" — as the **single** place a CC8 transfer constant, matrix
//! coefficient, luminance anchor, or gate shape is written down. §10 step 2
//! builds it before anything that consumes it, so every later step imports
//! rather than redefines: restating one of these values as a literal anywhere
//! else in the workspace is the drift CC7 §2.1 forbids and CC8 inherits.
//!
//! It is **data and arithmetic only**, like `cc7_scenarios`: no `Document`
//! mutation, no rendering, no filesystem, no clock, no RNG. Every function
//! here is a pure function of its arguments, so two evaluations produce
//! identical values on both CI operating systems.
//!
//! # The transfers are transcribed here, deliberately
//!
//! CC8 §2.2: "The ST 2084 and ARIB STD-B67 constants are transcribed from
//! their standards into the authority module and are part of this contract;
//! they must not be delegated to a platform colour API or an `FFmpeg` filter."
//! That is why this module carries the rational forms of ST 2084's five
//! constants and ARIB STD-B67's three, rather than calling a backend that
//! would carry its own.
//!
//! Both transfers evaluate in `f32` with **sign-preserving negative
//! extension**, in the manner CC1 §3.1 establishes for BT.709 (`encode_bt709`
//! / `decode_display709`, `kinewright-media/src/color_pipeline.rs:354-394`),
//! so undershoot survives to the final clamp instead of dying at a transfer.
//! [`cc8_sign`] is the same `sgn(0) = 0` helper `grade709_sign` uses, and for
//! the same reason.
//!
//! # What §10 step 3 added
//!
//! §10 step 3 ("source profiles and transfer decode") added §2.1's closed
//! profile table — [`CC8_SOURCE_PROFILES`], the 10-bit floor
//! [`CC8_HDR_MIN_INTEGER_DEPTH_BITS`], and [`CC8_REJECTED_HDR_ADJACENT`] — and
//! the two fused working-linear decodes the managed input path calls,
//! [`cc8_pq_decode_working_linear`] and [`cc8_hlg_decode_working_linear`]. The
//! latter carries §3.3's HLG working-linear determination; read its doc comment
//! before changing it.
//!
//! # What §10 step 6 added
//!
//! §10 step 6 ("delivery lane, tags, typed rejection") landed §5.1's lane table
//! — [`CC8_HDR_DELIVERY_LANE`] — and §5.2's proven `x264-params` string,
//! [`CC8_HDR_DELIVERY_X264_PARAMS`], together with the per-field allowed-value
//! phrases and recovery actions §5.3's typed rejection reports against.
//!
//! # What §10 step 8 added
//!
//! §10 step 8 ("preview and UI") landed §4's tone-mapping stage —
//! [`CC8_PREVIEW_STAGE`], its one pinned parameter [`CC8_PREVIEW_PEAK_NITS`],
//! the curve [`cc8_preview_tone_map`], and the labels every UI surface showing
//! it reads ([`CC8_PREVIEW_LABEL`], [`CC8_PREVIEW_BADGE`]) — together with
//! §3.2 items 1 and 2's two named node limitations,
//! [`CC8_AUTHORED_DOMAIN_LIMITATION`] and [`CC8_QUALIFIER_LIMITATION`].
//!
//! # What §10 step 9 added
//!
//! §10 step 9 ("migration and serialization") landed §2.4's units — the ST 2086
//! increments a declared mastering display is stored in
//! ([`CC8_MASTERING_DISPLAY_CHROMATICITY_INCREMENTS_PER_UNIT`],
//! [`CC8_MASTERING_DISPLAY_LUMINANCE_TEN_THOUSANDTHS_PER_NIT`]), the exact
//! conversion [`cc8_st2086_units`] that refuses rather than rounds, and
//! [`CC8_HDR_STATIC_METADATA_BOUNDARY`], the sentence §8 prints beside the
//! values saying what CC8 does **not** do with them. Step 2's own note said
//! this module pinned no metadata shape because §10 gave §2.4 no step of its
//! own; step 9 is where it lands, because §2.4 stores the values "on the source
//! description" and that is the `ColorDescription` change §7 item 1's
//! byte-unchanged obligation governs.
//!
//! # Why §9.2's table carries no numbers
//!
//! §9.2 is unambiguous: "**Every tolerance below is a placeholder to be
//! measured at implementation.** None may be invented, scaled, or inherited
//! from another lane... The *shape* of each gate is fixed here and is
//! normative; the *number* is not, and a number that appears in this table is
//! a description of what will be measured, not a value."
//!
//! So [`CC8_GATES`] carries every row's shape and the fixture that will
//! measure it, and its value is the single typed variant
//! [`Cc8GateValue::ToBeMeasuredAtImplementation`] — the contract's own phrase,
//! made a type so the absence cannot be papered over with a plausible integer.
//! §10 step 10 measures them; that step adds the measured arm and the margin
//! kinds, following `cc7_scenarios::Cc7BudgetKind` and §9.2's two rules (a
//! constant asserted against the manifest with no `cfg(windows)` and no per-OS
//! value, and a real margin or a `RecordedMargin` row that says why not).
//!
//! The test epsilons in this module's own `tests` are **not** gates and are
//! not §9.2 rows. Each is a bound on an `f32` round trip derived from a stated
//! error-propagation argument, held privately by the test module.

// ===========================================================================
// Integer conversion, and the sign convention shared with CC1/CC3.
// ===========================================================================

/// One CC8 integer as `f32`.
///
/// Every value this module converts — nits, thousandths, ten-thousandths, the
/// ST 2084 rational numerators — is far inside `2^24`, so the conversion is
/// exact and the pedantic precision-loss lint is answered here once rather
/// than at each call site. This is `cc7_scenarios::cc7_as_f64`'s idiom at
/// `f32`, which is the width CC8 §2.2 requires the transfers to evaluate in.
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub const fn cc8_as_f32(value: i32) -> f32 {
    value as f32
}

/// One CC8 integer as `f64`, for the §6 legality reference, which is `f64`
/// because `color_qc::bt709_limited_ycbcr` is. Every `i32` is exact in `f64`,
/// so this is lossless; it is a named helper only because `f64::from` is not
/// yet callable in a `const fn`.
#[must_use]
pub const fn cc8_as_f64(value: i32) -> f64 {
    value as f64
}

/// One CC8 hundred-millionths constant as `f32`.
///
/// ARIB STD-B67's `a`, `b`, and `c` are stated as eight-decimal figures rather
/// than as rationals, so they are pinned as integer hundred-millionths (which
/// exceed `2^24` and so are converted through `f64`, where they are exact)
/// and narrowed once here. `cc8_hlg_constants_are_the_standard_decimals`
/// checks that this narrowing lands on the same `f32` the decimal literal
/// does, so the two-step rounding is not a second definition.
#[must_use]
#[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
pub const fn cc8_hundred_millionths_f32(value: i64) -> f32 {
    (value as f64 / 100_000_000.0) as f32
}

/// The CC1/CC3 sign function: `sgn(0) = 0`.
///
/// [`f32::signum`] is deliberately not used; it returns `±1` at zero and would
/// break `f(0) = 0`, which every transfer in this module holds exactly. This
/// is `grade709_sign`'s rule (`kinewright-media/src/color_pipeline.rs:944`),
/// restated because core cannot see the media crate (`cc7_scenarios`' R-M2
/// boundary).
#[must_use]
pub fn cc8_sign(value: f32) -> f32 {
    if value > 0.0 {
        1.0
    } else if value < 0.0 {
        -1.0
    } else {
        0.0
    }
}

// ===========================================================================
// CC8 §2.1: the closed source-profile set.
// ===========================================================================

/// CC8 §2.1's **10-bit floor**, in bits.
///
/// §2.1: "The 10-bit floor is deliberate and normative: 8-bit PQ or HLG is
/// banding by construction, and accepting it would be a claim CC8 cannot
/// defend. An 8- or 9-bit HDR tuple is a typed rejection naming the depth, not
/// a warning." So this is not a tolerance and not a preference — it is the
/// lower bound of the profile table's own `Integer depth` column.
pub const CC8_HDR_MIN_INTEGER_DEPTH_BITS: u8 = 10;

/// CC8 §2.1's integer-depth ceiling, in bits: the table's `10..=16 bits`.
///
/// The same ceiling CC1 §2.1 puts on an SDR source, kept identical so an HDR
/// tuple is narrowed only at the floor and a future widening is one change in
/// one place.
pub const CC8_HDR_MAX_INTEGER_DEPTH_BITS: u8 = 16;

/// One row of CC8 §2.1's source-profile table.
///
/// The fields are the table's own columns, and each is stored in the **wire
/// spelling** the project schema serialises (`color.rs`'s `color_tag!`
/// strings), not as a `ColorPrimaries`/`ColorTransfer` value. That is the same
/// boundary [`CC8_REC2020_TO_BT709`] takes with its derivation: one definition,
/// asserted against the other rather than duplicated. `color_tag!`'s `wire`
/// accessor is the other side, and
/// `cc8_source_profile_wire_spellings_are_the_color_tag_serde_forms` holds the
/// two together, so a renamed tag breaks the build's tests rather than
/// silently unmatching a profile.
///
/// §2.1: "A profile match is on all listed fields; a partial match is not
/// enough, exactly as CC1 §2.1."
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cc8SourceProfile {
    /// The `Profile id` column: `pq_rec2020` or `hlg_rec2020`.
    pub id: &'static str,
    /// The `Primaries` column. `bt2020` for both rows.
    pub primaries: &'static str,
    /// The `Transfer` column: `smpte2084` or `arib_std_b67`.
    pub transfer: &'static str,
    /// The `Matrix` column: `bt2020_ncl` or `rgb`.
    pub matrices: [&'static str; 2],
    /// The `Range` column: `limited` or `full`.
    pub ranges: [&'static str; 2],
    /// The `White point` column. `d65` for both rows.
    pub white_point: &'static str,
    /// The `Integer depth` column's inclusive floor,
    /// [`CC8_HDR_MIN_INTEGER_DEPTH_BITS`].
    pub min_integer_depth_bits: u8,
    /// The `Integer depth` column's inclusive ceiling,
    /// [`CC8_HDR_MAX_INTEGER_DEPTH_BITS`].
    pub max_integer_depth_bits: u8,
}

impl Cc8SourceProfile {
    /// Whether this row admits a matrix, by wire spelling.
    #[must_use]
    pub fn accepts_matrix(&self, matrix: &str) -> bool {
        self.matrices.contains(&matrix)
    }

    /// Whether this row admits a coded range, by wire spelling.
    #[must_use]
    pub fn accepts_range(&self, range: &str) -> bool {
        self.ranges.contains(&range)
    }

    /// Whether this row admits a white point, by wire spelling.
    ///
    /// §2.1 carries CC1's D65 rule over unchanged: an *unknown* white point is
    /// not admitted here, because admitting it is only allowed "through an
    /// explicit `profile_assumption` recorded in the colour status and proof".
    /// The caller owns that decision; this function answers only what the
    /// table says.
    #[must_use]
    pub fn accepts_white_point(&self, white_point: &str) -> bool {
        self.white_point == white_point
    }

    /// Whether this row admits an integer sample depth.
    #[must_use]
    pub const fn accepts_integer_depth(&self, bits: u8) -> bool {
        bits >= self.min_integer_depth_bits && bits <= self.max_integer_depth_bits
    }
}

/// CC8 §2.1's table, in the contract's own row order.
///
/// This is the **closed** set. §1: "An HDR source that is not one of §2.1's two
/// profiles **must** produce a visible typed status with an explicit override
/// path. It **must not** be silently treated as Rec.709, and it **must not** be
/// silently tone-mapped." [`CC8_REJECTED_HDR_ADJACENT`] enumerates what §2.1
/// names as being outside it, so a fixture cannot quietly drop one.
pub const CC8_SOURCE_PROFILES: [Cc8SourceProfile; 2] = [
    Cc8SourceProfile {
        id: "pq_rec2020",
        primaries: "bt2020",
        transfer: "smpte2084",
        matrices: ["bt2020_ncl", "rgb"],
        ranges: ["limited", "full"],
        white_point: "d65",
        min_integer_depth_bits: CC8_HDR_MIN_INTEGER_DEPTH_BITS,
        max_integer_depth_bits: CC8_HDR_MAX_INTEGER_DEPTH_BITS,
    },
    Cc8SourceProfile {
        id: "hlg_rec2020",
        primaries: "bt2020",
        transfer: "arib_std_b67",
        matrices: ["bt2020_ncl", "rgb"],
        ranges: ["limited", "full"],
        white_point: "d65",
        min_integer_depth_bits: CC8_HDR_MIN_INTEGER_DEPTH_BITS,
        max_integer_depth_bits: CC8_HDR_MAX_INTEGER_DEPTH_BITS,
    },
];

/// The §2.1 row a primaries/transfer pair selects, or `None`.
///
/// The pair is what *identifies* a row — no other column distinguishes
/// `pq_rec2020` from `hlg_rec2020`, and no other column can make a tuple HDR
/// that this pair does not — so this is the single question "is this one of the
/// two profiles' shapes?", asked before any per-field check. A `None` here
/// means the tuple is not in the closed set at all and must be diagnosed by the
/// SDR field rules, which is exactly what keeps a BT.709/PQ or Rec.2020/BT.709
/// mismatch reported against the field CC1 already names it by.
#[must_use]
pub fn cc8_source_profile_for_primaries_and_transfer(
    primaries: &str,
    transfer: &str,
) -> Option<&'static Cc8SourceProfile> {
    CC8_SOURCE_PROFILES
        .iter()
        .find(|profile| profile.primaries == primaries && profile.transfer == transfer)
}

/// The §2.1 row with this id, or `None`.
#[must_use]
pub fn cc8_source_profile_by_id(id: &str) -> Option<&'static Cc8SourceProfile> {
    CC8_SOURCE_PROFILES.iter().find(|profile| profile.id == id)
}

/// Whether a primaries/transfer pair is one of §2.1's two profile shapes.
#[must_use]
pub fn cc8_is_hdr_source_pair(primaries: &str, transfer: &str) -> bool {
    cc8_source_profile_for_primaries_and_transfer(primaries, transfer).is_some()
}

/// One HDR-adjacent tuple CC8 §2.1 places **outside** the closed set.
///
/// §2.1: "`bt2020_cl` (constant luminance), `ictcp`, `chroma_derived_*`, P3
/// primaries in any combination, and `smpte2084`/`arib_std_b67` paired with
/// non-Rec.2020 primaries are **explicit CC8 failures**, not guesses. As in CC1
/// §2.1, the error must name the asset, the unsupported field, the observed
/// value, and the allowed values."
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cc8RejectedHdrTuple {
    /// The source-description field the rejection is reported against — the
    /// same wire field names `ColorSourceError::field` uses.
    pub field: &'static str,
    /// The observed value's wire spelling, or the shape §2.1 names when the
    /// rejection is a family rather than one value.
    pub observed: &'static str,
    /// Why §2.1 places it outside the set, in the contract's own terms.
    pub reason: &'static str,
}

/// Every HDR-adjacent tuple §2.1 names as outside the closed set.
///
/// §9.1 fixture 5 enumerates this table rather than a hand-written list, so a
/// rejection that stops being reachable fails the fixture instead of quietly
/// disappearing from it.
pub const CC8_REJECTED_HDR_ADJACENT: [Cc8RejectedHdrTuple; 8] = [
    Cc8RejectedHdrTuple {
        field: "bit_depth",
        observed: "8",
        reason: "8-bit PQ or HLG is banding by construction (§2.1's 10-bit floor)",
    },
    Cc8RejectedHdrTuple {
        field: "bit_depth",
        observed: "9",
        reason: "below §2.1's 10-bit floor",
    },
    Cc8RejectedHdrTuple {
        field: "matrix",
        observed: "bt2020_cl",
        reason: "constant luminance is outside §1's matrix set",
    },
    Cc8RejectedHdrTuple {
        field: "matrix",
        observed: "ictcp",
        reason: "ICtCp is an explicit §1 non-deliverable",
    },
    Cc8RejectedHdrTuple {
        field: "matrix",
        observed: "chroma_derived_ncl",
        reason: "chroma-derived matrices are outside §2.1's set",
    },
    Cc8RejectedHdrTuple {
        field: "matrix",
        observed: "chroma_derived_cl",
        reason: "chroma-derived matrices are outside §2.1's set",
    },
    Cc8RejectedHdrTuple {
        field: "primaries",
        observed: "display_p3",
        reason: "P3 primaries in any combination are an explicit CC8 failure",
    },
    Cc8RejectedHdrTuple {
        field: "primaries",
        observed: "dci_p3",
        reason: "P3 primaries in any combination are an explicit CC8 failure",
    },
];

/// The allowed-value phrase for a primaries field rejected on an HDR tuple.
///
/// The §2.1 allowed-value strings are pinned here for the same reason the
/// transfer constants are: they are part of the contract's typed-failure
/// surface, and CC1's own strings are asserted verbatim by
/// `cc1_fixtures::cc1_source_profile_classification_is_typed_and_actionable`.
/// Keeping CC8's separate from CC1's is what lets both be verbatim.
pub const CC8_HDR_PRIMARIES_ALLOWED: &str = "bt2020 with smpte2084 or arib_std_b67";
/// The allowed-value phrase for a matrix field rejected on an HDR tuple.
pub const CC8_HDR_MATRIX_ALLOWED: &str = "bt2020_ncl or rgb in a CC8 HDR source profile";
/// The allowed-value phrase for a range field rejected on an HDR tuple.
pub const CC8_HDR_RANGE_ALLOWED: &str = "limited or full in a CC8 HDR source profile";
/// The allowed-value phrase for a white point rejected on an HDR tuple.
pub const CC8_HDR_WHITE_POINT_ALLOWED: &str =
    "d65, or an explicit D65 assumption for a CC8 HDR source profile";
/// The allowed-value phrase for a depth rejected on an HDR tuple: §2.1's floor
/// and ceiling, formatted from the two pinned constants rather than restated.
pub const CC8_HDR_DEPTH_ALLOWED: &str = "integer depth 10..=16 for a CC8 HDR source profile";

/// What a *delivery* description must be before a CC8 HDR source can be
/// exported, as one phrase for CC8 §7 item 2's typed block.
///
/// This is the *shape* question §7 item 2 asks — "is this project's delivery
/// description an HDR one at all?" — and it is deliberately broader than
/// [`CC8_HDR_DELIVERY_LANE`], which §10 step 6 pinned as the one lane an export
/// may actually take. A project whose delivery is `bt2020` + `smpte2084`
/// answers item 2's question (its HDR source is not being tone-mapped) and is
/// still refused at the export gate by §5.3, with §11's PQ deferral named.
pub const CC8_HDR_DELIVERY_ALLOWED: &str = "a CC8 HDR delivery description (bt2020 primaries with smpte2084 or arib_std_b67); \
     §5.1's single delivery lane is the HLG one, and §11 defers PQ/HDR10 delivery";

/// The recovery action for a tuple refused by §2.1's HDR rules.
///
/// §2.1 requires the error to "name the asset, the unsupported field, the
/// observed value, and the allowed values"; §1 requires "an explicit override
/// path". CC1's bare "relink to compatible media" does not say *which* set to
/// relink into for a file that is HDR and is being refused on one named field,
/// so this names the closed set.
///
/// It opens with CC1's own clause deliberately: every surface that checks a
/// recovery action is present looks for "Apply an explicit supported
/// source-colour override", and a CC8 reason is still that instruction — it
/// just says more.
pub const CC8_HDR_RECOVERY_ACTION: &str = "Apply an explicit supported source-colour override matching a CC8 HDR source profile \
     (bt2020 with smpte2084 or arib_std_b67, bt2020_ncl or rgb, limited or full, d65, \
     10..=16-bit integer samples), or relink to media inside that set. CC8 does not \
     infer an HDR profile from a partial match.";

// ===========================================================================
// CC8 §2.4: the units static HDR metadata is declared in (§10 step 9).
// ===========================================================================

/// SMPTE ST 2086's chromaticity unit: increments of `0.00002`, so a stored
/// coordinate is `round(x · 50 000)`.
///
/// CC8 §2.4 requires mastering-display primaries to be "read on probe where the
/// container carries them, stored on the source description with provenance,
/// and **reported**", and "never invented". Storing the integer the bitstream
/// itself carries is what makes that exact: the SEI and the `mdcv` box both
/// code chromaticity in these increments, so
/// [`MasteringDisplayChromaticity`](crate::MasteringDisplayChromaticity)
/// records the declared integer rather than a float derived from it. No
/// rounding happens on the way in — [`cc8_st2086_units`] refuses a value that
/// is not exactly representable instead of nudging it.
pub const CC8_MASTERING_DISPLAY_CHROMATICITY_INCREMENTS_PER_UNIT: u32 = 50_000;

/// SMPTE ST 2086's luminance unit: increments of `0.0001 cd/m²`, so a stored
/// luminance is `round(nits · 10 000)`.
///
/// The other half of [`CC8_MASTERING_DISPLAY_CHROMATICITY_INCREMENTS_PER_UNIT`]'s
/// reasoning, on the `min_luminance`/`max_luminance` rows.
pub const CC8_MASTERING_DISPLAY_LUMINANCE_TEN_THOUSANDTHS_PER_NIT: u32 = 10_000;

/// The exact byte length of libavutil's `AVMasteringDisplayMetadata`, the
/// layout probe reads a declared mastering display out of.
///
/// Twenty-two `int`s: `display_primaries[3][2]` and `white_point[2]` as
/// `AVRational` pairs (16 ints), `min_luminance` and `max_luminance` (4), and
/// the two `has_*` flags. The probe asserts this length rather than trusting a
/// buffer, so a build whose layout differs reports `Unknown` — §2.4's own
/// answer for metadata that was not read — instead of decoding whatever the
/// bytes happen to be.
pub const CC8_AV_MASTERING_DISPLAY_METADATA_BYTES: usize = 88;

/// The exact byte length of libavutil's `AVContentLightMetadata`: `MaxCLL` and
/// `MaxFALL`, two `unsigned`s.
pub const CC8_AV_CONTENT_LIGHT_METADATA_BYTES: usize = 8;

/// What CC8 does, and does not do, with the values §2.4 stores.
///
/// §2.4, verbatim on the second half: "Under §0.2 Q1's decision the HLG lane
/// does not consume them; they exist so the QC surface can report what a source
/// claimed and so a PQ lane has its inputs already modelled." §11 says the same
/// from the deferral's side: "PQ / HDR10 delivery, deferred by §0.2 Q1's
/// decision, and with it mastering-display provenance and gated MaxCLL/MaxFALL.
/// §2.4 and §6 item 3 deliberately produce its inputs and deliberately leave
/// them unapplied."
///
/// It is a constant rather than a comment because §8 reports it beside the
/// values, in the manner of `color_qc::LIGHT_LEVEL_BOUNDARY`: a number on a
/// status surface with no statement of what consumes it is an invitation to
/// assume something does.
pub const CC8_HDR_STATIC_METADATA_BOUNDARY: &str = "Declared by the source and reported as evidence. CC8 §2.4 stores mastering-display \
     primaries and MaxCLL/MaxFALL with their provenance and deliberately leaves them \
     unapplied: §0.2 Q1's HLG lane does not consume them, and §11 defers the PQ/HDR10 lane \
     that would. Absent metadata is unknown and stays unknown; nothing here is inferred from \
     the picture. These are the source's claims, not this build's measurements — CC8 §6 item \
     3's MaxCLL/MaxFALL are a different number, measured from the working proof.";

/// Convert one declared `AVRational` into ST 2086's integer units **exactly**,
/// or refuse.
///
/// `units_per_whole` is the standard's own denominator —
/// [`CC8_MASTERING_DISPLAY_CHROMATICITY_INCREMENTS_PER_UNIT`] for a
/// chromaticity coordinate, [`CC8_MASTERING_DISPLAY_LUMINANCE_TEN_THOUSANDTHS_PER_NIT`]
/// for a luminance — and the conversion is `numerator · units_per_whole /
/// denominator`, taken only when that division is exact.
///
/// Returning `None` rather than rounding is §2.4's "never invented" rule
/// applied to arithmetic: a value that cannot be written in the standard's own
/// units without moving it is not a value this build can honestly say the
/// source declared. In practice `FFmpeg`'s own SEI and box parsers produce
/// exactly these denominators, so the refusing arm is the guard and not the
/// normal path.
///
/// A negative numerator or a non-positive denominator refuses for the same
/// reason: ST 2086's fields are unsigned, so a negative declaration is not a
/// value in these units at all.
#[must_use]
pub fn cc8_st2086_units(numerator: i64, denominator: i64, units_per_whole: u32) -> Option<u32> {
    if numerator < 0 || denominator <= 0 {
        return None;
    }
    let scaled = numerator.checked_mul(i64::from(units_per_whole))?;
    if scaled % denominator != 0 {
        return None;
    }
    u32::try_from(scaled / denominator).ok()
}

// ===========================================================================
// CC8 §2.2: the reference-white anchor.
// ===========================================================================

/// CC8 §2.2's anchor: ITU-R BT.2408's nominal HDR reference white,
/// **203 cd/m²**, the number that maps display-referred absolute luminance
/// into the scene-referred working space where diffuse white is `1.0`.
///
/// §2.2 states this is "a **standards value, not a measurement**, and is not
/// subject to the measured-tolerance rule. Every *tolerance* in §9 is." §12
/// records the standing risk that it is "a choice that looks like a fact":
/// it is pinned here, as one inspectable constant, precisely so that making
/// it per-project later is a small change rather than a shader hunt.
pub const CC8_REFERENCE_WHITE_NITS: i32 = 203;

/// CC8 §2.2: ST 2084's peak, the absolute luminance `E' = 1.0` decodes to.
pub const CC8_PQ_PEAK_NITS: i32 = 10_000;

/// CC8 §2.2: the HLG nominal peak the system gamma is stated against.
pub const CC8_HLG_NOMINAL_PEAK_NITS: i32 = 1_000;

/// CC8 §2.2: the HLG system gamma, `γ = 1.2` at
/// [`CC8_HLG_NOMINAL_PEAK_NITS`], per BT.2100, in thousandths.
///
/// It is an integer in thousandths rather than a float because the house rule
/// pins integers with their unit in the name, and because BT.2100's
/// peak-dependent extension of γ is a *later* slice's decision: §2.2 requires
/// only that the OOTF be separable "so a later slice can vary the peak without
/// disturbing the curve", which is why [`cc8_hlg_ootf_nits`] takes both the
/// peak and the gamma as arguments and this constant supplies the nominal one.
pub const CC8_HLG_SYSTEM_GAMMA_THOUSANDTHS: i32 = 1_200;

/// BT.2408's HLG reference-white signal level: **75 %** of the HLG range.
///
/// Not named by §2.2, but it is the relation that makes §2.2's single anchor
/// consistent across both profiles rather than a coincidence: a 75 % HLG
/// signal, through [`cc8_hlg_inverse_oetf`] and [`cc8_hlg_ootf_nits`] at the
/// nominal peak and gamma, is 203 cd/m² — the same
/// [`CC8_REFERENCE_WHITE_NITS`] the PQ path divides by.
/// `cc8_hlg_reference_white_signal_lands_on_the_anchor` asserts it.
pub const CC8_HLG_REFERENCE_WHITE_SIGNAL_PERCENT: i32 = 75;

/// The system gamma of [`CC8_HLG_SYSTEM_GAMMA_THOUSANDTHS`] as `f32`.
#[must_use]
pub fn cc8_hlg_system_gamma() -> f32 {
    cc8_as_f32(CC8_HLG_SYSTEM_GAMMA_THOUSANDTHS) / 1_000.0
}

/// The nominal HLG peak of [`CC8_HLG_NOMINAL_PEAK_NITS`] as `f32`.
#[must_use]
pub fn cc8_hlg_nominal_peak_nits() -> f32 {
    cc8_as_f32(CC8_HLG_NOMINAL_PEAK_NITS)
}

/// CC8 §2.2's reference-white normalization: absolute cd/m² to working linear.
///
/// ```text
/// working_linear = absolute_nits / CC8_REFERENCE_WHITE_NITS
/// ```
///
/// so diffuse white lands at `1.0`, a 1 000-nit specular highlight at `≈ 4.93`,
/// and ST 2084's 10 000-nit peak at `≈ 49.3` — §3.1's headroom argument, far
/// inside `f16`'s 65 504. It is a plain scale, so it is sign-preserving by
/// construction and needs no negative extension of its own.
///
/// §3.3 places this stage on the **PQ** profile only. HLG is relative and
/// decodes without it; where an HLG display luminance is wanted in working
/// units, it is this same function applied after [`cc8_hlg_ootf_nits`], and
/// the 75 % relation above is why the two profiles agree.
#[must_use]
pub fn cc8_nits_to_working_linear(nits: f32) -> f32 {
    nits / cc8_as_f32(CC8_REFERENCE_WHITE_NITS)
}

/// The exact inverse of [`cc8_nits_to_working_linear`].
#[must_use]
pub fn cc8_working_linear_to_nits(working_linear: f32) -> f32 {
    working_linear * cc8_as_f32(CC8_REFERENCE_WHITE_NITS)
}

// ===========================================================================
// CC8 §2.2: SMPTE ST 2084 (PQ), transcribed from the standard.
// ===========================================================================

/// ST 2084 `m1 = 2610 / 16384`, numerator.
pub const CC8_PQ_M1_NUMERATOR: i32 = 2_610;
/// ST 2084 `m1 = 2610 / 16384`, denominator.
pub const CC8_PQ_M1_DENOMINATOR: i32 = 16_384;
/// ST 2084 `m2 = (2523 / 4096) · 128`, numerator.
pub const CC8_PQ_M2_NUMERATOR: i32 = 2_523;
/// ST 2084 `m2 = (2523 / 4096) · 128`, denominator.
pub const CC8_PQ_M2_DENOMINATOR: i32 = 4_096;
/// ST 2084 `m2 = (2523 / 4096) · 128`, scale.
pub const CC8_PQ_M2_SCALE: i32 = 128;
/// ST 2084 `c1 = 3424 / 4096`, numerator. `c1 = c3 − c2 + 1`.
pub const CC8_PQ_C1_NUMERATOR: i32 = 3_424;
/// ST 2084 `c1 = 3424 / 4096`, denominator.
pub const CC8_PQ_C1_DENOMINATOR: i32 = 4_096;
/// ST 2084 `c2 = (2413 / 4096) · 32`, numerator.
pub const CC8_PQ_C2_NUMERATOR: i32 = 2_413;
/// ST 2084 `c2 = (2413 / 4096) · 32`, denominator.
pub const CC8_PQ_C2_DENOMINATOR: i32 = 4_096;
/// ST 2084 `c2 = (2413 / 4096) · 32`, scale.
pub const CC8_PQ_C2_SCALE: i32 = 32;
/// ST 2084 `c3 = (2392 / 4096) · 32`, numerator.
pub const CC8_PQ_C3_NUMERATOR: i32 = 2_392;
/// ST 2084 `c3 = (2392 / 4096) · 32`, denominator.
pub const CC8_PQ_C3_DENOMINATOR: i32 = 4_096;
/// ST 2084 `c3 = (2392 / 4096) · 32`, scale.
pub const CC8_PQ_C3_SCALE: i32 = 32;

/// ST 2084 `m1`, the inner exponent of the EOTF (CC8 §2.2).
///
/// Every one of the five ST 2084 constants is an exact binary fraction, so
/// each is representable in `f32` without rounding and
/// `cc8_pq_constants_are_their_exact_rational_forms` asserts equality rather
/// than closeness.
pub const CC8_PQ_M1: f32 = cc8_as_f32(CC8_PQ_M1_NUMERATOR) / cc8_as_f32(CC8_PQ_M1_DENOMINATOR);
/// ST 2084 `m2`, the outer exponent (CC8 §2.2).
pub const CC8_PQ_M2: f32 = cc8_as_f32(CC8_PQ_M2_NUMERATOR) / cc8_as_f32(CC8_PQ_M2_DENOMINATOR)
    * cc8_as_f32(CC8_PQ_M2_SCALE);
/// ST 2084 `c1` (CC8 §2.2).
pub const CC8_PQ_C1: f32 = cc8_as_f32(CC8_PQ_C1_NUMERATOR) / cc8_as_f32(CC8_PQ_C1_DENOMINATOR);
/// ST 2084 `c2` (CC8 §2.2).
pub const CC8_PQ_C2: f32 = cc8_as_f32(CC8_PQ_C2_NUMERATOR) / cc8_as_f32(CC8_PQ_C2_DENOMINATOR)
    * cc8_as_f32(CC8_PQ_C2_SCALE);
/// ST 2084 `c3` (CC8 §2.2).
pub const CC8_PQ_C3: f32 = cc8_as_f32(CC8_PQ_C3_NUMERATOR) / cc8_as_f32(CC8_PQ_C3_DENOMINATOR)
    * cc8_as_f32(CC8_PQ_C3_SCALE);

/// The ST 2084 EOTF: a PQ signal to **absolute** luminance in cd/m².
///
/// ```text
/// p       = |E'|^(1/m2)
/// eotf(E) = sgn(E') · 10000 · (max(p − c1, 0) / (c2 − c3·p))^(1/m1)
/// ```
///
/// Two seams are recorded here rather than discovered later, as CC1 §3.1
/// records BT.709's and §9.1 fixture 1 requires:
///
/// 1. **The `max(·, 0)` floor is not injective.** Every signal in
///    `[0, c1^m2 ≈ 7.31e-7]` decodes to 0 cd/m², because ST 2084's inverse
///    sends any strictly positive luminance, however small, to at least
///    `c1^m2`. The `sgn(0) = 0` convention above keeps `0 ↔ 0` exact in both
///    directions, so the foot is only visible at luminances between zero and
///    the first 10-bit code — far below anything a source carries, and
///    recorded here so a later fixture reads it as the standard's shape
///    rather than as a defect.
/// 2. **The rational form has a pole** at `p = c2 / c3`, i.e. at
///    `E' = (c2/c3)^m2 ≈ 1.992`. That is far above any 10-bit code — a
///    limited-range code 1023 is `E' ≈ 1.095` — so it is unreachable from a
///    real source and reachable only from synthetic input. At or past the
///    pole this returns a sign-preserving infinity rather than a plausible
///    finite number, so a caller that gets there is told.
///
/// Negative signals take the sign-preserving extension CC8 §2.2 requires, so
/// undershoot survives the decode.
#[must_use]
pub fn cc8_pq_eotf_nits(signal: f32) -> f32 {
    let sign = cc8_sign(signal);
    let perceptual = signal.abs().powf(1.0 / CC8_PQ_M2);
    let numerator = (perceptual - CC8_PQ_C1).max(0.0);
    let denominator = CC8_PQ_C2 - CC8_PQ_C3 * perceptual;
    if denominator <= 0.0 {
        return sign * f32::INFINITY;
    }
    sign * cc8_as_f32(CC8_PQ_PEAK_NITS) * (numerator / denominator).powf(1.0 / CC8_PQ_M1)
}

/// The ST 2084 inverse EOTF: absolute luminance in cd/m² to a PQ signal.
///
/// ```text
/// y                = (|L| / 10000)^m1
/// inverse_eotf(L)  = sgn(L) · ((c1 + c2·y) / (1 + c3·y))^m2
/// ```
///
/// Bounded above by the pole [`cc8_pq_eotf_nits`] documents: as `L → ∞` the
/// ratio approaches `c2 / c3` and the signal approaches `(c2/c3)^m2 ≈ 1.992`
/// from below. Sign-preserving, per CC8 §2.2.
#[must_use]
pub fn cc8_pq_inverse_eotf(nits: f32) -> f32 {
    let sign = cc8_sign(nits);
    let luminance = (nits.abs() / cc8_as_f32(CC8_PQ_PEAK_NITS)).powf(CC8_PQ_M1);
    sign * ((CC8_PQ_C1 + CC8_PQ_C2 * luminance) / (1.0 + CC8_PQ_C3 * luminance)).powf(CC8_PQ_M2)
}

/// CC8 §2.2's PQ decode, both stages: a PQ signal to working linear.
///
/// The composition §2.2 writes out — `pq_eotf` then division by
/// [`CC8_REFERENCE_WHITE_NITS`] — kept as one named function so a caller
/// cannot take the first stage and forget the second.
#[must_use]
pub fn cc8_pq_decode_working_linear(signal: f32) -> f32 {
    cc8_nits_to_working_linear(cc8_pq_eotf_nits(signal))
}

/// The inverse of [`cc8_pq_decode_working_linear`]: working linear to a PQ
/// signal. CC8 defers PQ *delivery* (§0.2 Q1, §11); this exists because §2.4
/// and §6 item 3 deliberately produce a PQ lane's inputs, and because §9.1
/// fixture 1's round trip needs both directions.
#[must_use]
pub fn cc8_pq_encode_working_linear(working_linear: f32) -> f32 {
    cc8_pq_inverse_eotf(cc8_working_linear_to_nits(working_linear))
}

// ===========================================================================
// CC8 §2.2: ARIB STD-B67 (HLG), transcribed from the standard.
// ===========================================================================

/// ARIB STD-B67 `a = 0.178_832_77`, in hundred-millionths.
pub const CC8_HLG_A_HUNDRED_MILLIONTHS: i64 = 17_883_277;
/// ARIB STD-B67 `b = 0.284_668_92 = 1 − 4a`, in hundred-millionths.
pub const CC8_HLG_B_HUNDRED_MILLIONTHS: i64 = 28_466_892;
/// ARIB STD-B67 `c = 0.559_910_73 = 0.5 − a·ln(4a)`, in hundred-millionths.
pub const CC8_HLG_C_HUNDRED_MILLIONTHS: i64 = 55_991_073;

/// ARIB STD-B67 `a` (CC8 §2.2).
pub const CC8_HLG_A: f32 = cc8_hundred_millionths_f32(CC8_HLG_A_HUNDRED_MILLIONTHS);
/// ARIB STD-B67 `b = 1 − 4a` (CC8 §2.2).
pub const CC8_HLG_B: f32 = cc8_hundred_millionths_f32(CC8_HLG_B_HUNDRED_MILLIONTHS);
/// ARIB STD-B67 `c = 0.5 − a·ln(4a)` (CC8 §2.2).
pub const CC8_HLG_C: f32 = cc8_hundred_millionths_f32(CC8_HLG_C_HUNDRED_MILLIONTHS);

/// The HLG scene-linear breakpoint, `1/12`, where the two OETF branches meet.
pub const CC8_HLG_SCENE_BREAKPOINT: f32 = 1.0 / 12.0;
/// The HLG signal breakpoint, `0.5`, the image of [`CC8_HLG_SCENE_BREAKPOINT`].
pub const CC8_HLG_SIGNAL_BREAKPOINT: f32 = 0.5;

/// The ARIB STD-B67 OETF: scene linear to an HLG signal.
///
/// ```text
/// oetf(E) = sgn(E) · sqrt(3·|E|)                      |E| ≤ 1/12
///         = sgn(E) · (a·ln(12·|E| − b) + c)           |E| > 1/12
/// ```
///
/// The seam is exact by construction: `b = 1 − 4a` and `c = 0.5 − a·ln(4a)`
/// make both branches `0.5` at `E = 1/12`, and in `f32` with the standard's
/// rounded decimals both branches evaluate to exactly `0.5` — asserted, not
/// assumed, by `cc8_hlg_oetf_anchor_points`. `oetf(1) = 1` likewise holds
/// exactly in `f32`, though the same relation in `f64` shows the standard's
/// eight-decimal rounding as `≈ 0.999_999_996`.
///
/// Sign-preserving, per CC8 §2.2.
#[must_use]
pub fn cc8_hlg_oetf(scene_linear: f32) -> f32 {
    let sign = cc8_sign(scene_linear);
    let magnitude = scene_linear.abs();
    if magnitude <= CC8_HLG_SCENE_BREAKPOINT {
        sign * (3.0 * magnitude).sqrt()
    } else {
        sign * (CC8_HLG_A * (12.0 * magnitude - CC8_HLG_B).ln() + CC8_HLG_C)
    }
}

/// The ARIB STD-B67 inverse OETF: an HLG signal to scene linear.
///
/// ```text
/// inverse_oetf(E') = sgn(E') · E'^2 / 3                          |E'| ≤ 1/2
///                  = sgn(E') · (exp((|E'| − c) / a) + b) / 12     |E'| > 1/2
/// ```
///
/// CC8 §2.2 requires this and [`cc8_hlg_inverse_ootf`] to be **two separately
/// named stages**, "so a later slice can vary the peak without disturbing the
/// curve": this stage is peak-independent and takes no peak argument, and the
/// OOTF stage takes both the peak and the system gamma. Sign-preserving.
#[must_use]
pub fn cc8_hlg_inverse_oetf(signal: f32) -> f32 {
    let sign = cc8_sign(signal);
    let magnitude = signal.abs();
    if magnitude <= CC8_HLG_SIGNAL_BREAKPOINT {
        sign * magnitude * magnitude / 3.0
    } else {
        sign * (((magnitude - CC8_HLG_C) / CC8_HLG_A).exp() + CC8_HLG_B) / 12.0
    }
}

/// The BT.2020 non-constant-luminance luma of a linear RGB triple.
///
/// The OOTF's `Y_S` and `Y_D`. §3.3 places the HLG OOTF stages **before** the
/// primaries conversion, so their luma is BT.2020's, not the BT.709 luma
/// §3.2 item 3 keeps for `saturation_percent` and the QC luma.
#[must_use]
pub fn cc8_bt2020_luma(linear_rgb: [f32; 3]) -> f32 {
    CC8_BT2020_LUMA_F32[0] * linear_rgb[0]
        + CC8_BT2020_LUMA_F32[1] * linear_rgb[1]
        + CC8_BT2020_LUMA_F32[2] * linear_rgb[2]
}

/// The ARIB STD-B67 / BT.2100 OOTF: scene linear to **absolute** display
/// luminance in cd/m².
///
/// ```text
/// Y_S    = 0.2627·R_S + 0.6780·G_S + 0.0593·B_S
/// F_D[i] = peak · |Y_S|^(γ − 1) · E_S[i]
/// ```
///
/// The peak and the system gamma are arguments, not constants, because CC8
/// §2.2 requires the OOTF to be separable from the OETF "so a later slice can
/// vary the peak without disturbing the curve".
/// [`cc8_hlg_ootf_nits_nominal`] supplies §2.2's pinned pair.
///
/// **Negative and zero luma.** BT.2100 defines the OOTF for `Y_S ≥ 0` only.
/// The gain here is taken on `|Y_S|`, so a triple whose luma is negative keeps
/// a real gain and every component keeps its own sign — the sign-preserving
/// requirement of §2.2, applied to the stage that would otherwise return
/// `NaN`. At `Y_S = 0` the gain is `0^(γ−1) = 0` and the result is zero: a
/// chromatic triple with exactly zero luma collapses, which is a property of
/// the standard's own OOTF and not an extension made here.
#[must_use]
pub fn cc8_hlg_ootf_nits(scene_linear: [f32; 3], peak_nits: f32, system_gamma: f32) -> [f32; 3] {
    let luma = cc8_bt2020_luma(scene_linear);
    let gain = peak_nits * luma.abs().powf(system_gamma - 1.0);
    [
        gain * scene_linear[0],
        gain * scene_linear[1],
        gain * scene_linear[2],
    ]
}

/// The BT.2100 inverse OOTF: absolute display luminance in cd/m² to scene
/// linear.
///
/// ```text
/// Y_D       = 0.2627·R_D + 0.6780·G_D + 0.0593·B_D
/// E_S[i]    = |Y_D / peak|^((1 − γ) / γ) · F_D[i] / peak
/// ```
///
/// the exact analytic inverse of [`cc8_hlg_ootf_nits`], since
/// `Y_D = peak · Y_S^γ`. Zero display luma returns zero rather than the
/// `0^(−1/6) = ∞` the formula gives, which is the same seam
/// [`cc8_hlg_ootf_nits`] documents seen from the other side. Magnitudes are
/// taken on the luma only, so component signs survive.
#[must_use]
pub fn cc8_hlg_inverse_ootf(display_nits: [f32; 3], peak_nits: f32, system_gamma: f32) -> [f32; 3] {
    let normalized_luma = cc8_bt2020_luma(display_nits) / peak_nits;
    if normalized_luma == 0.0 {
        return [0.0, 0.0, 0.0];
    }
    let gain = normalized_luma
        .abs()
        .powf((1.0 - system_gamma) / system_gamma)
        / peak_nits;
    [
        gain * display_nits[0],
        gain * display_nits[1],
        gain * display_nits[2],
    ]
}

/// [`cc8_hlg_ootf_nits`] at CC8 §2.2's pinned nominal peak and system gamma.
#[must_use]
pub fn cc8_hlg_ootf_nits_nominal(scene_linear: [f32; 3]) -> [f32; 3] {
    cc8_hlg_ootf_nits(
        scene_linear,
        cc8_hlg_nominal_peak_nits(),
        cc8_hlg_system_gamma(),
    )
}

/// [`cc8_hlg_inverse_ootf`] at CC8 §2.2's pinned nominal peak and system
/// gamma.
#[must_use]
pub fn cc8_hlg_inverse_ootf_nominal(display_nits: [f32; 3]) -> [f32; 3] {
    cc8_hlg_inverse_ootf(
        display_nits,
        cc8_hlg_nominal_peak_nits(),
        cc8_hlg_system_gamma(),
    )
}

/// CC8 §3.3's HLG source decode, all three stages: an HLG signal triple to
/// working linear.
///
/// ```text
/// E_S  = inverse_oetf(E')                     per channel, ARIB STD-B67
/// F_D  = ootf(E_S, peak = 1000, gamma = 1.2)  BT.2100, absolute cd/m²
/// w    = F_D / CC8_REFERENCE_WHITE_NITS       §2.2's anchor
/// ```
///
/// # §3.3's HLG working-linear determination (CC8 §10 step 3)
///
/// §3.3's pipeline listing marks reference-white normalization "(\* PQ profile
/// only, §2.2)" and names the HLG stage "HLG inverse OOTF". §2.2 leaves the
/// composition to the implementation — it fixes only that the OETF and OOTF are
/// "two separately named stages, so a later slice can vary the peak without
/// disturbing the curve". Step 3 makes the determination explicitly, and it is
/// this: **the anchor divide does follow the OOTF on the HLG path, and the
/// stage that realizes §3.3's HLG OOTF in the decode direction is the forward
/// [`cc8_hlg_ootf_nits_nominal`], not [`cc8_hlg_inverse_ootf_nominal`].**
///
/// Three things forced it, in order of weight:
///
/// 1. **§3.1 fixes the working space and §3.1 wins.** The working space is
///    "byte-identical to CC1 §2" with diffuse white at `1.0`. BT.2408 puts HLG
///    diffuse white at [`CC8_HLG_REFERENCE_WHITE_SIGNAL_PERCENT`] of the
///    signal range, and `inverse_oetf(0.75) ≈ 0.2650` — so an HLG decode that
///    stopped at the inverse OETF would land diffuse white **1.92 stops under**
///    a PQ source of the same nominal white and under every Rec.709 source,
///    and every CC3 curve and CC5 wheel authored on that domain would be
///    reading the wrong exposure. Stopping after the OOTF without the divide is
///    worse still: diffuse white would land at 203.0.
/// 2. **The constants are otherwise dead.** §2.2 pins
///    [`CC8_HLG_NOMINAL_PEAK_NITS`] and [`CC8_HLG_SYSTEM_GAMMA_THOUSANDTHS`]
///    *for the source side* — "its OETF is defined against a nominal peak with
///    a system gamma that depends on it". A decode that skipped the OOTF would
///    use neither. A decode that applied the OOTF and not the anchor would
///    leave the result in cd/m², which is not a working-space quantity.
/// 3. **§10 step 2 already wrote the relation down.** [`cc8_nits_to_working_linear`]
///    says: "where an HLG display luminance is wanted in working units, it is
///    this same function applied after [`cc8_hlg_ootf_nits`], and the 75 %
///    relation above is why the two profiles agree", and
///    `cc8_hlg_reference_white_signal_lands_on_the_anchor` asserts that
///    75 % → 203 cd/m² → working `1.0`. This function is that assertion made
///    into the production stage; §9.1 fixture 2 gates it.
///
/// **On §3.3's word "inverse".** [`cc8_hlg_inverse_ootf`]'s signature is
/// display cd/m² → scene linear, which is the *delivery* direction — §3.3's own
/// delivery line has the mirror-image looseness, writing "HLG OOTF+OETF" where
/// encoding needs the inverse OOTF and then the OETF. Read consistently, §3.3
/// names each stage by the OETF/OOTF *pair* it belongs to and fixes their
/// order; the function that realizes a stage is the one whose signature matches
/// the direction it is used in. Both stages stay separately named and the OOTF
/// still takes its peak and gamma as arguments, so §2.2's separability
/// requirement is met exactly.
///
/// Sign-preserving throughout: the inverse OETF is sign-preserving per channel,
/// the OOTF takes its gain on `|Y_S|` and keeps each component's sign, and the
/// anchor is a plain positive scale.
#[must_use]
pub fn cc8_hlg_decode_working_linear(signal_rgb: [f32; 3]) -> [f32; 3] {
    let scene_linear = signal_rgb.map(cc8_hlg_inverse_oetf);
    let display_nits = cc8_hlg_ootf_nits_nominal(scene_linear);
    display_nits.map(cc8_nits_to_working_linear)
}

/// The exact inverse of [`cc8_hlg_decode_working_linear`]: working linear to an
/// HLG signal triple.
///
/// The three stages run backwards — [`cc8_working_linear_to_nits`], then
/// [`cc8_hlg_inverse_ootf_nominal`], then [`cc8_hlg_oetf`] per channel — which
/// is why *this* direction is where [`cc8_hlg_inverse_ootf`]'s
/// display-nits-to-scene-linear signature belongs.
///
/// CC8 §5.1 does deliver HLG, but §10 step 6 owns that lane; this exists now
/// because §9.1 fixture 1's round trip needs both directions, in the manner
/// [`cc8_pq_encode_working_linear`] already exists for a deferred PQ lane.
#[must_use]
pub fn cc8_hlg_encode_working_linear(working_linear_rgb: [f32; 3]) -> [f32; 3] {
    let display_nits = working_linear_rgb.map(cc8_working_linear_to_nits);
    let scene_linear = cc8_hlg_inverse_ootf_nominal(display_nits);
    scene_linear.map(cc8_hlg_oetf)
}

// ===========================================================================
// CC8 §6 item 1: the BT.2020 non-constant-luminance Y'CbCr reference.
// ===========================================================================

/// BT.2020 `KR = 0.2627`, in ten-thousandths (CC8 §6 item 1).
pub const CC8_BT2020_KR_TEN_THOUSANDTHS: i32 = 2_627;
/// BT.2020 `KG = 0.6780`, in ten-thousandths. `KR + KG + KB = 10 000` exactly
/// in this unit, which is why the green coefficient is pinned rather than
/// subtracted at each call site.
pub const CC8_BT2020_KG_TEN_THOUSANDTHS: i32 = 6_780;
/// BT.2020 `KB = 0.0593`, in ten-thousandths (CC8 §6 item 1).
pub const CC8_BT2020_KB_TEN_THOUSANDTHS: i32 = 593;

/// Ten thousand: the denominator of every `_TEN_THOUSANDTHS` constant here.
const TEN_THOUSAND: i32 = 10_000;

/// BT.2020 luma coefficient for red, `f64`, for §6's legality sibling of
/// `color_qc::bt709_limited_ycbcr`, which is `f64`.
pub const CC8_BT2020_KR: f64 = cc8_as_f64(CC8_BT2020_KR_TEN_THOUSANDTHS) / cc8_as_f64(TEN_THOUSAND);
/// BT.2020 luma coefficient for green, `f64` (CC8 §6 item 1).
pub const CC8_BT2020_KG: f64 = cc8_as_f64(CC8_BT2020_KG_TEN_THOUSANDTHS) / cc8_as_f64(TEN_THOUSAND);
/// BT.2020 luma coefficient for blue, `f64` (CC8 §6 item 1).
pub const CC8_BT2020_KB: f64 = cc8_as_f64(CC8_BT2020_KB_TEN_THOUSANDTHS) / cc8_as_f64(TEN_THOUSAND);

/// `2 · (1 − KB) = 1.8814`: the BT.2020 Cb normalization denominator, the
/// sibling of `color_qc::BT709_CB_DENOMINATOR` §6 item 1 requires.
pub const CC8_BT2020_CB_DENOMINATOR: f64 = 2.0 * (1.0 - CC8_BT2020_KB);
/// `2 · (1 − KR) = 1.4746`: the BT.2020 Cr normalization denominator.
pub const CC8_BT2020_CR_DENOMINATOR: f64 = 2.0 * (1.0 - CC8_BT2020_KR);

/// The same three coefficients at `f32`, the width the OOTF evaluates in.
///
/// Derived from the same pinned integers as [`CC8_BT2020_KR`] and friends, so
/// this is one definition at two widths rather than two definitions;
/// `cc8_bt2020_luma_coefficients_agree_at_both_widths` holds them together.
pub const CC8_BT2020_LUMA_F32: [f32; 3] = [
    cc8_as_f32(CC8_BT2020_KR_TEN_THOUSANDTHS) / cc8_as_f32(TEN_THOUSAND),
    cc8_as_f32(CC8_BT2020_KG_TEN_THOUSANDTHS) / cc8_as_f32(TEN_THOUSAND),
    cc8_as_f32(CC8_BT2020_KB_TEN_THOUSANDTHS) / cc8_as_f32(TEN_THOUSAND),
];

// ===========================================================================
// CC8 §2.3: the Rec.2020 <-> BT.709 primaries conversion.
// ===========================================================================

/// A chromaticity pair `(x, y)` in ten-thousandths.
///
/// Every chromaticity CC8 needs is stated to four decimals in its standard, so
/// ten-thousandths is an exact integer unit for all of them and the house
/// rule's "integers with the unit in the name" holds all the way down to the
/// primaries.
pub type Cc8ChromaticityTenThousandths = [i32; 2];

/// BT.709 primaries `R(0.6400, 0.3300) G(0.3000, 0.6000) B(0.1500, 0.0600)`,
/// in ten-thousandths, in R, G, B order (CC8 §2.3, CC1 §2's working space).
pub const CC8_BT709_PRIMARIES_TEN_THOUSANDTHS: [Cc8ChromaticityTenThousandths; 3] =
    [[6_400, 3_300], [3_000, 6_000], [1_500, 600]];

/// Rec.2020 primaries `R(0.7080, 0.2920) G(0.1700, 0.7970) B(0.1310, 0.0460)`,
/// in ten-thousandths, in R, G, B order (CC8 §2.3).
pub const CC8_REC2020_PRIMARIES_TEN_THOUSANDTHS: [Cc8ChromaticityTenThousandths; 3] =
    [[7_080, 2_920], [1_700, 7_970], [1_310, 460]];

/// D65 `(0.3127, 0.3290)`, in ten-thousandths — the **shared** white point of
/// both primary sets, which is why the conversion is a pure 3×3 with no
/// chromatic adaptation (CC8 §2.3, §3.1).
pub const CC8_D65_TEN_THOUSANDTHS: Cc8ChromaticityTenThousandths = [3_127, 3_290];

/// CC8 §2.3's Rec.2020 → BT.709 linear-light matrix, row-major.
///
/// §2.3 fixes the representation: "a 3×3 linear-light matrix, applied after
/// transfer decode and before any grading node, with its exact coefficients
/// pinned in the authority module (**derived from the two primary sets and
/// D65, transcribed to f32, not taken from a backend**)." So the coefficients
/// are pinned `f32` rows, and the derivation they are a transcription of is
/// [`cc8_derive_rec2020_to_bt709`], which builds them in `f64` from
/// [`CC8_REC2020_PRIMARIES_TEN_THOUSANDTHS`],
/// [`CC8_BT709_PRIMARIES_TEN_THOUSANDTHS`], and [`CC8_D65_TEN_THOUSANDTHS`]
/// alone. `cc8_pinned_matrices_are_the_derivation_transcribed` asserts each
/// row is bit-for-bit the `f64` derivation narrowed to `f32`, so the
/// transcription is a boundary rather than a second definition — the rule
/// `cc7_scenarios` states for its three transfer transcriptions.
///
/// **The negatives are the design, not a bug.** §2.3: colours outside the
/// Rec.709 triangle become negative BT.709 components and "**must not be
/// clamped**" — CC1 §2.2 invariant 5 already forbids it, and CC8 restates it
/// because a future contributor will be tempted to "fix" them.
/// [`CC8_BT709_TO_REC2020`] restores them at delivery.
pub const CC8_REC2020_TO_BT709: [[f32; 3]; 3] = [
    [1.660_491, -0.587_641_1, -0.072_849_86],
    [-0.124_550_48, 1.132_899_9, -0.008_349_422],
    [-0.018_150_764, -0.100_578_9, 1.118_729_7],
];

/// CC8 §2.3's BT.709 → Rec.2020 linear-light matrix, row-major: the delivery
/// direction, and the inverse of [`CC8_REC2020_TO_BT709`].
///
/// Derived by [`cc8_derive_bt709_to_rec2020`] the same way and to the same
/// rule. The two are inverse in the terms §2.3 states — an unclamped linear
/// round trip that restores out-of-triangle colour — and
/// `cc8_primaries_matrices_are_mutually_inverse` bounds the residual with a
/// stated `f32` unit-in-the-last-place argument rather than a chosen epsilon.
pub const CC8_BT709_TO_REC2020: [[f32; 3]; 3] = [
    [0.627_403_9, 0.329_283_03, 0.043_313_067],
    [0.069_097_29, 0.919_540_4, 0.011_362_315],
    [0.016_391_44, 0.088_013_306, 0.895_595_25],
];

/// Apply a pinned 3×3 to a linear RGB triple. No clamp, ever (CC8 §2.3).
#[must_use]
pub fn cc8_apply_matrix(matrix: [[f32; 3]; 3], linear_rgb: [f32; 3]) -> [f32; 3] {
    [
        matrix[0][0] * linear_rgb[0] + matrix[0][1] * linear_rgb[1] + matrix[0][2] * linear_rgb[2],
        matrix[1][0] * linear_rgb[0] + matrix[1][1] * linear_rgb[1] + matrix[1][2] * linear_rgb[2],
        matrix[2][0] * linear_rgb[0] + matrix[2][1] * linear_rgb[1] + matrix[2][2] * linear_rgb[2],
    ]
}

/// The RGB → CIE XYZ matrix of one primary set and white point, in `f64`.
///
/// The textbook derivation, and the only place CC8's matrices come from:
/// `X_i = x_i / y_i`, `Y_i = 1`, `Z_i = (1 − x_i − y_i) / y_i` per primary; the
/// per-primary scales are `S = M⁻¹ · W` for the white point in the same form;
/// the result is `M` with its columns scaled by `S`. `f64` throughout, then
/// narrowed once, so the pinned `f32` rows are the correctly-rounded
/// transcription of an exact-as-possible derivation rather than of a chain of
/// `f32` roundings.
#[must_use]
pub fn cc8_derive_rgb_to_xyz(
    primaries: [Cc8ChromaticityTenThousandths; 3],
    white_point: Cc8ChromaticityTenThousandths,
) -> [[f64; 3]; 3] {
    let mut unscaled = [[0.0_f64; 3]; 3];
    for (index, primary) in primaries.iter().enumerate() {
        let x = cc8_as_f64(primary[0]) / cc8_as_f64(TEN_THOUSAND);
        let y = cc8_as_f64(primary[1]) / cc8_as_f64(TEN_THOUSAND);
        unscaled[0][index] = x / y;
        unscaled[1][index] = 1.0;
        unscaled[2][index] = (1.0 - x - y) / y;
    }
    let white_x = cc8_as_f64(white_point[0]) / cc8_as_f64(TEN_THOUSAND);
    let white_y = cc8_as_f64(white_point[1]) / cc8_as_f64(TEN_THOUSAND);
    let white = [white_x / white_y, 1.0, (1.0 - white_x - white_y) / white_y];
    let primary_scales = cc8_multiply_vector_f64(cc8_invert_f64(unscaled), white);
    let mut matrix = [[0.0_f64; 3]; 3];
    for (row, output) in matrix.iter_mut().enumerate() {
        for (column, cell) in output.iter_mut().enumerate() {
            *cell = unscaled[row][column] * primary_scales[column];
        }
    }
    matrix
}

/// CC8 §2.3's Rec.2020 → BT.709 matrix, derived in `f64`:
/// `inverse(RGB709 → XYZ) · (RGB2020 → XYZ)`.
#[must_use]
pub fn cc8_derive_rec2020_to_bt709() -> [[f64; 3]; 3] {
    cc8_multiply_f64(
        cc8_invert_f64(cc8_derive_rgb_to_xyz(
            CC8_BT709_PRIMARIES_TEN_THOUSANDTHS,
            CC8_D65_TEN_THOUSANDTHS,
        )),
        cc8_derive_rgb_to_xyz(
            CC8_REC2020_PRIMARIES_TEN_THOUSANDTHS,
            CC8_D65_TEN_THOUSANDTHS,
        ),
    )
}

/// CC8 §2.3's BT.709 → Rec.2020 matrix, derived in `f64`:
/// `inverse(RGB2020 → XYZ) · (RGB709 → XYZ)`.
///
/// Derived from the two XYZ matrices rather than by inverting
/// [`cc8_derive_rec2020_to_bt709`], so neither direction is privileged and
/// neither inherits the other's rounding.
#[must_use]
pub fn cc8_derive_bt709_to_rec2020() -> [[f64; 3]; 3] {
    cc8_multiply_f64(
        cc8_invert_f64(cc8_derive_rgb_to_xyz(
            CC8_REC2020_PRIMARIES_TEN_THOUSANDTHS,
            CC8_D65_TEN_THOUSANDTHS,
        )),
        cc8_derive_rgb_to_xyz(CC8_BT709_PRIMARIES_TEN_THOUSANDTHS, CC8_D65_TEN_THOUSANDTHS),
    )
}

/// The `f64` 3×3 product, row-major.
#[must_use]
pub fn cc8_multiply_f64(left: [[f64; 3]; 3], right: [[f64; 3]; 3]) -> [[f64; 3]; 3] {
    let mut product = [[0.0_f64; 3]; 3];
    for row in 0..3 {
        for column in 0..3 {
            product[row][column] = left[row][0] * right[0][column]
                + left[row][1] * right[1][column]
                + left[row][2] * right[2][column];
        }
    }
    product
}

/// The `f64` 3×3 times a column vector.
#[must_use]
pub fn cc8_multiply_vector_f64(matrix: [[f64; 3]; 3], vector: [f64; 3]) -> [f64; 3] {
    [
        matrix[0][0] * vector[0] + matrix[0][1] * vector[1] + matrix[0][2] * vector[2],
        matrix[1][0] * vector[0] + matrix[1][1] * vector[1] + matrix[1][2] * vector[2],
        matrix[2][0] * vector[0] + matrix[2][1] * vector[1] + matrix[2][2] * vector[2],
    ]
}

/// The `f64` 3×3 inverse by cofactors.
///
/// Both matrices this is used on are well conditioned primary sets sharing a
/// white point, so the determinant is nowhere near zero; a singular argument
/// would produce infinities, which the derivation tests would catch at once.
#[must_use]
pub fn cc8_invert_f64(matrix: [[f64; 3]; 3]) -> [[f64; 3]; 3] {
    let determinant = matrix[0][0] * (matrix[1][1] * matrix[2][2] - matrix[1][2] * matrix[2][1])
        - matrix[0][1] * (matrix[1][0] * matrix[2][2] - matrix[1][2] * matrix[2][0])
        + matrix[0][2] * (matrix[1][0] * matrix[2][1] - matrix[1][1] * matrix[2][0]);
    [
        [
            (matrix[1][1] * matrix[2][2] - matrix[1][2] * matrix[2][1]) / determinant,
            (matrix[0][2] * matrix[2][1] - matrix[0][1] * matrix[2][2]) / determinant,
            (matrix[0][1] * matrix[1][2] - matrix[0][2] * matrix[1][1]) / determinant,
        ],
        [
            (matrix[1][2] * matrix[2][0] - matrix[1][0] * matrix[2][2]) / determinant,
            (matrix[0][0] * matrix[2][2] - matrix[0][2] * matrix[2][0]) / determinant,
            (matrix[0][2] * matrix[1][0] - matrix[0][0] * matrix[1][2]) / determinant,
        ],
        [
            (matrix[1][0] * matrix[2][1] - matrix[1][1] * matrix[2][0]) / determinant,
            (matrix[0][1] * matrix[2][0] - matrix[0][0] * matrix[2][1]) / determinant,
            (matrix[0][0] * matrix[1][1] - matrix[0][1] * matrix[1][0]) / determinant,
        ],
    ]
}

/// One `f64` 3×3 narrowed to the pinned `f32` representation §2.3 mandates.
#[must_use]
#[allow(clippy::cast_possible_truncation)]
pub fn cc8_narrow_matrix(matrix: [[f64; 3]; 3]) -> [[f32; 3]; 3] {
    let mut narrowed = [[0.0_f32; 3]; 3];
    for row in 0..3 {
        for column in 0..3 {
            narrowed[row][column] = matrix[row][column] as f32;
        }
    }
    narrowed
}

// ===========================================================================
// CC8 §5.1 / §5.2: the one HDR delivery lane (§10 step 6).
// ===========================================================================

/// CC8 §5.1's lane table, one row per field.
///
/// §5.1 opens "Exactly one, per §0.2 Q1/Q2", and every field below is that
/// table's own cell. The colour fields are stored in the **wire spelling** the
/// project schema serialises, exactly as [`Cc8SourceProfile`] stores §2.1's,
/// so `color.rs`'s `color_tag!` forms and this table are held together by a
/// test rather than by two hand-maintained copies.
///
/// The codec and pixel-format cells are §5.1's own words too — "H.264 High 10
/// (`libx264`, the existing `DELIVERY_VIDEO_CODEC`)" and `yuv420p10le` — and
/// they are the *existing* CC6 §4.1 ten-bit lane restated, not a new one:
/// "This reuses CC6 §4.1's `DeliveryEncodeDepth::Ten` lane. It adds a *colour
/// description*, not a codec path, so `DELIVERY_SCALER_FLAGS = "bicubic"`, the
/// `DELIVERY_INTERMEDIATE_WHITE = 65_280` convention, and the single-pass
/// filter graph are unchanged and are not re-measured."
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cc8DeliveryLane {
    /// A stable identifier for the lane, for status and evidence surfaces.
    pub id: &'static str,
    /// §5.1's `Codec` row.
    pub codec: &'static str,
    /// §5.1's `Pixel format` row.
    pub pixel_format: &'static str,
    /// §5.1's `Primaries` row.
    pub primaries: &'static str,
    /// §5.1's `Transfer` row.
    pub transfer: &'static str,
    /// §5.1's `Matrix` row.
    pub matrix: &'static str,
    /// §5.1's `Range` row.
    pub range: &'static str,
    /// §5.1's `White point` row.
    pub white_point: &'static str,
    /// §5.1's `Bit depth` row, in bits.
    pub bit_depth_bits: u8,
    /// §5.2 item 2's `x264-params` string for this lane.
    pub x264_params: &'static str,
}

/// CC8 §5.1's lane, the only one CC8 delivers.
///
/// §5.1's closing sentence governs any second: "CC6 §13's rule governs any
/// second lane: 'the second lane is a slice, not a flag.'"
pub const CC8_HDR_DELIVERY_LANE: Cc8DeliveryLane = Cc8DeliveryLane {
    id: "hlg_rec2020_h264_high10",
    codec: "libx264",
    pixel_format: "yuv420p10le",
    primaries: "bt2020",
    transfer: "arib_std_b67",
    matrix: "bt2020_ncl",
    range: "limited",
    white_point: "d65",
    bit_depth_bits: 10,
    x264_params: CC8_HDR_DELIVERY_X264_PARAMS,
};

/// CC8 §5.2 item 2's `x264-params` string for [`CC8_HDR_DELIVERY_LANE`].
///
/// §5.2 item 2 writes it out: "`DELIVERY_X264_PARAMS` becomes a function of the
/// lane. For this lane: `colorprim=bt2020:transfer=arib-std-b67:colormatrix=bt2020nc`."
///
/// It is **character-identical to the string §10 step 1's precondition proved**
/// — `cc8_precondition_libx264_carries_hlg_hdr_tags_through_encode_and_reprobe`
/// encodes with exactly these three terms and re-probes them back — which is
/// what makes step 1's green build evidence about *this* constant rather than
/// about a similar one. `cc8_hdr_delivery_x264_params_is_the_proven_precondition_string`
/// asserts the two have not drifted apart.
///
/// x264's own spellings are **not** the project's wire spellings: x264 writes
/// `arib-std-b67` where the schema writes `arib_std_b67`, and `bt2020nc` where
/// the schema writes `bt2020_ncl`. That is why the string is pinned whole
/// rather than formatted from [`CC8_HDR_DELIVERY_LANE`]'s cells — a formatter
/// would have to carry a second vocabulary, and §0.3(b) makes this string the
/// **only** channel the tags travel on.
pub const CC8_HDR_DELIVERY_X264_PARAMS: &str =
    "colorprim=bt2020:transfer=arib-std-b67:colormatrix=bt2020nc";

/// The `x264-params` string the SDR lanes encode with, frozen by §5.2 item 2.
///
/// §5.2 item 2: "The SDR lanes' string is **byte-identical to today's**, and a
/// fixture asserts that." It is pinned here beside the HDR lane's so that
/// "a function of the lane" is a lookup over two named constants rather than a
/// conditional over a literal, and §9.1 fixture 6 holds it to today's bytes.
pub const CC8_SDR_DELIVERY_X264_PARAMS: &str = "colorprim=bt709:transfer=bt709:colormatrix=bt709";

/// §5.3's allowed-value phrase for `primaries` on [`CC8_HDR_DELIVERY_LANE`].
///
/// The six phrases below name the lane as well as the value, for two reasons.
/// They are what an agent or a UI reads back, and "bt2020" alone does not say
/// *which* contract wanted it; and they are the key
/// [`DeliveryColorError::recovery_action`](crate::DeliveryColorError::recovery_action)
/// uses to tell an HDR-lane refusal from an SDR-lane one, so each must be
/// distinct from every SDR phrase and from each other.
pub const CC8_HDR_DELIVERY_PRIMARIES_ALLOWED: &str = "bt2020 on CC8 §5.1's HDR delivery lane";
/// §5.3's allowed-value phrase for `transfer` on [`CC8_HDR_DELIVERY_LANE`].
pub const CC8_HDR_DELIVERY_TRANSFER_ALLOWED: &str =
    "arib_std_b67 on CC8 §5.1's HDR delivery lane (§11 defers PQ/HDR10 delivery)";
/// §5.3's allowed-value phrase for `matrix` on [`CC8_HDR_DELIVERY_LANE`].
pub const CC8_HDR_DELIVERY_MATRIX_ALLOWED: &str = "bt2020_ncl on CC8 §5.1's HDR delivery lane";
/// §5.3's allowed-value phrase for `range` on [`CC8_HDR_DELIVERY_LANE`].
pub const CC8_HDR_DELIVERY_RANGE_ALLOWED: &str = "limited on CC8 §5.1's HDR delivery lane";
/// §5.3's allowed-value phrase for `white_point` on [`CC8_HDR_DELIVERY_LANE`].
pub const CC8_HDR_DELIVERY_WHITE_POINT_ALLOWED: &str = "d65 on CC8 §5.1's HDR delivery lane";
/// §5.3's allowed-value phrase for `bit_depth` on [`CC8_HDR_DELIVERY_LANE`].
///
/// §2.1's 10-bit floor and §5.1's `Bit depth | Ten` row are the same number
/// seen from the source and the delivery sides, and the phrase says so because
/// §5.3 requires "HLG or PQ at 8-bit depth" to be refused with the depth named.
pub const CC8_HDR_DELIVERY_DEPTH_ALLOWED: &str =
    "10 (ten) on CC8 §5.1's HDR delivery lane; 8-bit HLG or PQ is banding by construction (§2.1)";

/// Every §5.3 allowed-value phrase [`CC8_HDR_DELIVERY_LANE`] can report.
///
/// The recovery-action lookup reads this table rather than a hand-written list,
/// so a phrase added to the lane cannot quietly acquire the SDR recovery.
pub const CC8_HDR_DELIVERY_ALLOWED_PHRASES: [&str; 6] = [
    CC8_HDR_DELIVERY_PRIMARIES_ALLOWED,
    CC8_HDR_DELIVERY_TRANSFER_ALLOWED,
    CC8_HDR_DELIVERY_MATRIX_ALLOWED,
    CC8_HDR_DELIVERY_RANGE_ALLOWED,
    CC8_HDR_DELIVERY_WHITE_POINT_ALLOWED,
    CC8_HDR_DELIVERY_DEPTH_ALLOWED,
];

/// The recovery action for a description refused against §5.1's lane table.
///
/// It covers **both** directions of §5.3's first bullet, because both are
/// cleared the same way and neither is cleared by a conversion: an HDR
/// description that is not §5.1's lane is completed or returned to SDR, and an
/// SDR description put on the HDR lane is exported on the SDR lane it actually
/// describes or replaced in full. §0.2 Q6 refuses HDR-from-SDR permanently and
/// defers tone-mapped SDR-from-HDR delivery, so nothing here turns one into the
/// other and the phrase says so.
pub const CC8_HDR_DELIVERY_RECOVERY_ACTION: &str = "Set the delivery description to CC8 §5.1's lane exactly — bt2020 primaries, \
     arib_std_b67 transfer, bt2020_ncl matrix, limited range, d65, 10-bit — or export on the \
     SDR Rec.709 lane the description actually names. CC8 §0.2 Q6 refuses HDR-from-SDR \
     permanently and defers tone-mapped SDR-from-HDR delivery, so nothing converts one into \
     the other. §5.1 has exactly one HDR lane and CC6 §13's rule governs a second: the second \
     lane is a slice, not a flag.";

/// The recovery action for PQ on the HLG lane (§5.3's last bullet).
///
/// §5.3 requires this one by name: "PQ on the HLG lane, with a recovery action
/// **naming the deferral rather than implying a conversion exists**." So it
/// says what is deferred and what does not exist, and it does not offer a
/// conversion — §0.2 Q6 refuses tone mapping as a deliverable and CC8 has no
/// PQ-to-HLG stage at all.
pub const CC8_PQ_DELIVERY_RECOVERY_ACTION: &str = "CC8 §0.2 Q1 chose HLG (ARIB STD-B67) as the one HDR delivery transfer, and §11 defers \
     PQ / HDR10 delivery together with mastering-display provenance and gated MaxCLL/MaxFALL. \
     No PQ-to-HLG conversion exists in CC8 and none is implied. Set the delivery transfer to \
     arib_std_b67, or keep the PQ target and wait for the deferred PQ slice.";

// ===========================================================================
// CC8 §4: the labelled tone-mapped preview.
// ===========================================================================

/// CC8 §3.3's name for §4's monitoring stage.
///
/// §4 item 1: "The stage is named, ordered, and reported in the colour status
/// like any other." §3.3 places it on the monitoring branch —
/// "monitoring: tone-mapped preview (§4) on an SDR display" — immediately
/// before "final clamp, quantization, and display/codec packing", which is why
/// [`cc8_preview_tone_map`] does not clamp: the single display clamp
/// `kinewright_media::color_pipeline::encode_monitor_rgb8` already performs
/// stays the only one (CC1 §2.2 invariant 5).
///
/// The name says which curve it is, because §4 leaves the curve to the
/// implementation and a reader of the colour status should not have to open the
/// source to find out.
pub const CC8_PREVIEW_STAGE: &str = "tone_map_preview_reinhard_extended";

/// §4 item 2's pinned parameter, and the only one: the absolute luminance the
/// preview maps to monitor white, in cd/m².
///
/// §4 item 2: "Its parameters are pinned integer constants in the authority
/// module." This is that integer, and it is deliberately **not a new number** —
/// it is [`CC8_HLG_NOMINAL_PEAK_NITS`], §2.2's own pinned HLG nominal peak, so
/// the preview's white is the peak the delivery lane's system gamma is stated
/// against rather than a level chosen for how it looks.
/// `cc8_preview_peak_is_the_pinned_hlg_nominal_peak` asserts the two have not
/// drifted apart.
///
/// §9.2's measured-tolerance rule does not reach it, for the reason §2.2 gives
/// about [`CC8_REFERENCE_WHITE_NITS`]: it is a *standards* value carried from
/// BT.2100's nominal HLG peak, not a measurement, and the numbers §9.2 governs
/// are the tolerances §9.1 fixture 9 measures **about** this stage.
pub const CC8_PREVIEW_PEAK_NITS: i32 = CC8_HLG_NOMINAL_PEAK_NITS;

/// [`CC8_PREVIEW_PEAK_NITS`] in working-linear units: `1000 / 203 ≈ 4.926`.
///
/// The tone map's white point `W`, derived from the two pinned integers through
/// §2.2's own [`cc8_nits_to_working_linear`] rather than written down a second
/// time.
#[must_use]
pub fn cc8_preview_peak_working_linear() -> f32 {
    cc8_nits_to_working_linear(cc8_as_f32(CC8_PREVIEW_PEAK_NITS))
}

/// CC8 §4's tone map: one working-linear channel to the Rec.709 monitoring
/// description's linear domain, **extended Reinhard** at
/// [`cc8_preview_peak_working_linear`].
///
/// ```text
/// W        = CC8_PREVIEW_PEAK_NITS / CC8_REFERENCE_WHITE_NITS
/// f(x)     = sgn(x) · |x| · (1 + |x| / W²) / (1 + |x|)
/// ```
///
/// # Why this curve
///
/// §4 fixes the *properties* and leaves the curve to the implementation, and
/// §4 item 4 names the properties its fixtures assert: "determinism,
/// monotonicity, endpoint behaviour, and CPU/GPU parity — properties, not
/// aesthetics". Extended Reinhard is the shape that has all of them with
/// **one** parameter, and that parameter is already pinned by §2.2:
///
/// 1. **Determinism.** Four multiplications, two additions and one division on
///    `f32`, all IEEE 754 exact operations — no `powf`, no `exp`, no `ln` — so
///    the value is bit-identical on both CI operating systems and needs no libm
///    allowance of the kind [`cc8_pq_eotf_nits`] carries.
/// 2. **Monotonicity**, everywhere rather than on an interval:
///    `f'(x) = (1 + (2x + x²)/W²) / (1 + x)²`, which is strictly positive for
///    every `x ≥ 0`, and the odd extension through `f(0) = 0` carries that to
///    the whole real line. Out-of-Rec.709 negatives (§2.3) therefore keep their
///    order as well as their sign.
/// 3. **Endpoint behaviour**, exactly: `f(0) = 0`, and `f(W) = 1` analytically
///    — `W(1 + 1/W)/(1 + W) = 1` — so the pinned peak lands on monitor white by
///    construction rather than by a fitted constant. Above `W` the curve keeps
///    rising past 1.0 and the existing display clamp takes it; nothing new
///    clamps, and no value is ever brightened, because `f(x)/x < 1` for every
///    `x > 0` whenever `W > 1`.
/// 4. **No rendering intent.** It is applied **per channel**, not as a
///    luminance-preserving scale. A hue-preserving variant is a creative
///    decision with a rendering intent, which is exactly what §0.2 Q6 refuses
///    to ship and what §11 defers; a preview that made one would be claiming to
///    be the tone-mapped delivery CC8 does not have.
///
/// This is a **preview** transform. §4 item 5: "It must not be reachable from
/// the delivery path", and nothing in `delivery.rs`,
/// `color_qc::encode_delivery_for_lane`, or
/// `kinewright_media::color_pipeline::encode_delivery_for_description` calls
/// it. §9.1 fixture 9 asserts that failing direction.
#[must_use]
pub fn cc8_preview_tone_map(working_linear: f32) -> f32 {
    let magnitude = working_linear.abs();
    let peak = cc8_preview_peak_working_linear();
    let mapped = magnitude * (1.0 + magnitude / (peak * peak)) / (1.0 + magnitude);
    cc8_sign(working_linear) * mapped
}

/// [`cc8_preview_tone_map`] per channel, which is how §4's stage is applied.
#[must_use]
pub fn cc8_preview_tone_map_rgb(working_linear_rgb: [f32; 3]) -> [f32; 3] {
    working_linear_rgb.map(cc8_preview_tone_map)
}

/// §4 item 3's short label, for a surface with room for a few words.
///
/// §4 item 3: "Every UI surface showing it is labelled as a non-calibrated
/// preview of HDR content. The specific wording is an implementation decision;
/// the requirement that it exist is not." Both this and
/// [`CC8_PREVIEW_LABEL`] are pinned here so that every surface reads **one**
/// wording — the same reason §5.3's allowed-value phrases are pinned — and so
/// that a fixture can assert the label is present rather than assert a string a
/// surface happens to hold.
pub const CC8_PREVIEW_BADGE: &str = "TONE-MAPPED PREVIEW · NOT A REFERENCE";

/// §4 item 3's full label: what the preview is, and the three things it is not.
///
/// §0.2 Q4 decided "a named, explicitly-labelled tone-mapped preview that is
/// not a monitoring reference and carries no exit gate", and §4 opens "CC8
/// provides **no calibrated HDR monitoring path** and must not imply one", so
/// the wording says all of it: not calibrated, not a reference, and not a
/// deliverable. §4's closing paragraph is here too — on an HDR-capable display
/// this is still the preview, because CC8 has "no display-capability query, no
/// HDR swapchain, and no metadata handoff to the compositor".
pub const CC8_PREVIEW_LABEL: &str = "This picture is a tone-mapped SDR approximation of HDR content, not a monitoring \
     reference. It is not calibrated, it is not what the HDR deliverable looks like, and no \
     Kinewright check is a judgment about how it looks. Calibrated HDR monitoring is a separate \
     later programme: CC8 makes no display-capability query and no HDR handoff, so this preview \
     is what an HDR-capable display gets too.";

/// The colour-status reason for `monitoring.calibrated_hdr = false` (§4).
pub const CC8_PREVIEW_NOT_CALIBRATED_REASON: &str = "CC8 §4: no calibrated HDR monitoring path, in any form or claim. What the monitoring \
     branch runs on an HDR-profile source is §4's labelled tone-mapped preview, which carries no \
     CC8 exit gate.";

/// §4 item 5's boundary, as one phrase the colour status prints.
pub const CC8_PREVIEW_DELIVERY_BOUNDARY: &str = "Preview only. CC8 §4 item 5: the tone map must not be reachable from the delivery path, \
     and §0.2 Q6 refuses tone-mapped SDR delivery from an HDR timeline as a deliverable.";

// ---------------------------------------------------------------------------
// CC8 §3.2 items 1 and 2: the two named node limitations, surfaced at the node.
// ---------------------------------------------------------------------------

/// The stable code for §3.2 item 1's condition on curve and wheel nodes.
pub const CC8_AUTHORED_DOMAIN_LIMITATION_CODE: &str = "cc8_node_input_exceeds_authored_domain";

/// §3.2 item 1's named limitation, for the node that has it.
///
/// §3.2 item 1 requires "the colour status to report when a node's input
/// exceeds its authored domain, so an editor is told rather than surprised",
/// and §8 puts the same fact on the node: "The inspector reports §3.2's
/// out-of-authored-domain condition on curve and wheel nodes". §12's mitigation
/// is why it is at the node and not in a file: "both limitations are surfaced
/// in the UI at the node that has them, not documented in a file nobody opens."
///
/// **What triggers it, stated plainly.** The trigger is the *source profile*,
/// not a measured per-node input maximum. On either §2.1 profile the decoded
/// working domain provably exceeds the authored one — §2.2's anchor puts the
/// HLG nominal peak at [`cc8_preview_peak_working_linear`], `≈ 4.93`, which is
/// `grade709 ≈ 2.03` and so ≈ 20 300 basis points against
/// [`COLOR_CURVE_WHITE_BASIS_POINTS`](crate::effect::COLOR_CURVE_WHITE_BASIS_POINTS)'s
/// 10 000 — so the condition is a property of the profile that the node is
/// reading. A *measured* per-node input maximum would need a per-node proof
/// render this build does not have, and inventing one would be worse than
/// naming the profile; that measurement is left to the CC3 amendment §11 already
/// defers ("HDR-aware curve and wheel authoring domains").
pub const CC8_AUTHORED_DOMAIN_LIMITATION: &str = "CC8 §3.2 item 1 — this node is authored on an SDR-shaped domain. Curve and wheel \
     controls are parameterized in basis points of the grade709 range where 10 000 bp is diffuse \
     white; an HDR source decodes far above it (the HLG nominal peak is grade709 ~2.03, about \
     20 300 bp), and the CC4 lattice's add-back rule shifts such a value rather than shaping it. \
     That is the existing behaviour and CC8 does not change it; widening the authored domain is a \
     CC3 amendment with its own parity gate (§11).";

/// The stable code for §3.2 item 2's qualifier limitation on matte nodes.
pub const CC8_QUALIFIER_LIMITATION_CODE: &str = "cc8_hsl_qualifier_domain_clamped";

/// §3.2 item 2's named limitation, for matte inspection.
///
/// §3.2 item 2 is the one CC8 marks **must**: the qualifier collapse "**must**
/// be surfaced as a named limitation in matte inspection whenever a qualifier
/// node runs on an HDR-profile source, so the matte's behaviour is explained
/// rather than merely observed." §12 names it as the limitation most likely to
/// read as a bug.
pub const CC8_QUALIFIER_LIMITATION: &str = "CC8 §3.2 item 2 — the HSL qualifier is genuinely limited on HDR. It clamps grade709 to \
     [0, 1] before deriving hue, saturation and luma, so every value above diffuse white produces \
     the same selector and a specular highlight cannot be qualified apart from a mid-tone. CC8 \
     does not fix this: an HDR-aware qualifier domain needs its own parity gate and its own \
     measured band constants, and §11 defers it.";

// ===========================================================================
// CC8 §9.2: the gate table, with its measurements deliberately absent.
// ===========================================================================

/// The §10 step that measures every [`CC8_GATES`] row: step 10, "measure every
/// §9.2 budget; write `cc8_manifest.json`; reconcile the inventory".
pub const CC8_GATE_MEASUREMENT_STEP: u8 = 10;

/// The shape of one §9.2 gate. §9.2: "The *shape* of each gate is fixed here
/// and is normative; the *number* is not."
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Cc8GateShape {
    /// Max / P99 / mean absolute error, linear domain, banded by magnitude in
    /// the manner of CC1 §6.2.
    BandedLinearAbsolute,
    /// Max / P99 / mean absolute error, linear domain.
    LinearAbsolute,
    /// Max / P99 / mean, per half-float band.
    PerHalfFloatBand,
    /// Max / P99 / mean luma; RGB mean; and a PSNR floor.
    DecodedDelivery,
    /// Basis points outside the legal range.
    LegalityBasisPoints,
    /// Max / P99 / mean, in monitor codes.
    MonitorCodes,
}

/// What a §9.2 row's value is at this step.
///
/// One variant, and that is the point: §9.2 says every tolerance in its table
/// "is a placeholder to be measured at implementation" and that "a number that
/// appears in this table is a description of what will be measured, not a
/// value". Typing the absence keeps a later reader from mistaking a plausible
/// integer for a measured one. §10 step 10 adds the measured arm, carrying the
/// budget, the measurement, and a margin kind in
/// `cc7_scenarios::Cc7BudgetKind`'s manner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Cc8GateValue {
    /// §9.2's own words, verbatim: "to be measured at implementation".
    ToBeMeasuredAtImplementation,
}

/// One row of CC8 §9.2's numeric-gate table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cc8Gate {
    /// The gate's name, as §9.2's first column writes it.
    pub gate: &'static str,
    /// The normative shape of the measurement (§9.2's second column).
    pub shape: Cc8GateShape,
    /// §9.2's third column.
    pub value: Cc8GateValue,
    /// The §9.1 fixture that will produce the measurement.
    pub fixture: u8,
}

/// CC8 §9.2's six rows, shapes only.
///
/// Two rules govern how the numbers are taken when §10 step 10 takes them,
/// both carried forward from CC7 and restated here because this is the module
/// they will be written into:
///
/// - **No gate may be an equality against one `FFmpeg` build's decode output.**
///   What gates is a constant asserted against the manifest, with both the
///   live and the recorded measurement inside that bound. Per-build figures
///   are reported, never gated: no `cfg(windows)`, no per-OS constant, no
///   window invented around one build's output.
/// - **A budget must carry a real margin, and a margin nothing approaches
///   proves nothing.** A term too close to its constant is recorded with its
///   margin (CC7's `RecordedMargin`), and a term that measures zero on the
///   passing source is bounded from above by a deliberately starved fixture.
pub const CC8_GATES: [Cc8Gate; 6] = [
    Cc8Gate {
        gate: "PQ/HLG transfer round trip",
        shape: Cc8GateShape::BandedLinearAbsolute,
        value: Cc8GateValue::ToBeMeasuredAtImplementation,
        fixture: 1,
    },
    Cc8Gate {
        gate: "Primaries round trip",
        shape: Cc8GateShape::LinearAbsolute,
        value: Cc8GateValue::ToBeMeasuredAtImplementation,
        fixture: 3,
    },
    Cc8Gate {
        gate: "CPU vs GPU, HDR magnitudes",
        shape: Cc8GateShape::PerHalfFloatBand,
        value: Cc8GateValue::ToBeMeasuredAtImplementation,
        fixture: 10,
    },
    Cc8Gate {
        gate: "Decoded HDR delivery",
        shape: Cc8GateShape::DecodedDelivery,
        value: Cc8GateValue::ToBeMeasuredAtImplementation,
        fixture: 8,
    },
    Cc8Gate {
        gate: "BT.2020 legality excursion",
        shape: Cc8GateShape::LegalityBasisPoints,
        value: Cc8GateValue::ToBeMeasuredAtImplementation,
        fixture: 12,
    },
    Cc8Gate {
        gate: "Preview parity",
        shape: Cc8GateShape::MonitorCodes,
        value: Cc8GateValue::ToBeMeasuredAtImplementation,
        fixture: 9,
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // Test bounds. None of these is a §9.2 gate; each is a bound on an `f32`
    // round trip, derived from a stated argument rather than chosen.
    // -----------------------------------------------------------------------

    /// The relative bound every `f32` PQ round trip here is held to, `2^-12`.
    ///
    /// Derivation. The composition `nits → E' → nits` introduces about ten
    /// rounded operations, each at most `2^-24` relative. The chain's
    /// amplification is dominated by two terms: ST 2084's `(p − c1)`
    /// subtraction, whose cancellation factor `p / (p − c1)` is ≈ 6.1 over the
    /// interesting decade, and the final `^(1/m1)`, which multiplies relative
    /// error by `1/m1 ≈ 6.28`. That is ≈ 38, so the analytic propagation is
    /// ≈ `380 · 2^-24 ≈ 2^-14`. Rust does not specify `f32::powf`'s accuracy
    /// at all, so the bound is loosened by two further powers of two to
    /// `2^-12`. Observed worst over the ramps below is ≈ `2^-13.2`, so the
    /// bound holds with better than a 2× margin and is still four orders of
    /// magnitude tighter than a mis-transcribed ST 2084 constant would move
    /// the round trip.
    const PQ_ROUND_TRIP_RELATIVE_BOUND: f32 = 1.0 / 4_096.0;

    /// The relative bound every `f32` HLG round trip here is held to, `2^-18`.
    ///
    /// Derivation. `oetf`/`inverse_oetf` are a square root and an exponential
    /// pair over about eight rounded operations. The amplification is the
    /// `(E' − c)` cancellation, ≈ 2.3, times `|(E' − c) / a| ≤ 2.5` through
    /// the exponential — ≈ 5.7 — so the analytic propagation is
    /// ≈ `46 · 2^-24 ≈ 2^-19`, doubled once for `powf`/`exp`/`ln` accuracy
    /// Rust does not specify. Observed worst is ≈ `2^-20.3`.
    const HLG_ROUND_TRIP_RELATIVE_BOUND: f32 = 1.0 / 262_144.0;

    /// The absolute bound on `f(W) = 1` for CC8 §4's preview curve,
    /// `8 · f32::EPSILON`.
    ///
    /// Derived, not chosen: the composition is four multiplications, two
    /// additions and one division, each at most `2^-24` relative, on values of
    /// magnitude at most `1 + W ≈ 5.93`, so the accumulated relative error is
    /// at most `7 · 2^-24 ≈ 4.2e-7`. Rounded up to the next power of two,
    /// `8 · f32::EPSILON`. There is no `powf` in this curve, so no libm
    /// allowance is taken. Observed residual is 0.
    const PREVIEW_ENDPOINT_BOUND: f32 = 8.0 * f32::EPSILON;

    /// The absolute bound on the 3×3 identity residual, `4 · f32::EPSILON`.
    ///
    /// Derivation. Each entry of the product is a three-term dot product of
    /// `f32` values of magnitude at most 1.67, so it accumulates three
    /// multiplication roundings and two addition roundings — five, each at
    /// most `1.67 · 2^-24` — i.e. at most `8.4 · 2^-24 = 2.1 · f32::EPSILON`.
    /// Rounded up to the next power of two, `4 · f32::EPSILON`. Observed
    /// worst is exactly `1 · f32::EPSILON`.
    const MATRIX_IDENTITY_ABSOLUTE_BOUND: f32 = 4.0 * f32::EPSILON;

    /// The same argument in `f64`, where the entries are the derivation's own
    /// and the five roundings are at `1.67 · 2^-53`: `8 · f64::EPSILON`.
    /// Observed worst is `1 · f64::EPSILON`.
    const MATRIX_IDENTITY_ABSOLUTE_BOUND_F64: f64 = 8.0 * f64::EPSILON;

    /// A 10-bit limited-range code ramp as signal values, `(code − 64) / 876`,
    /// so it spans undershoot below black, the nominal range, and the
    /// over-range codes §9.1 fixture 1 requires.
    fn ten_bit_limited_ramp() -> impl Iterator<Item = f32> {
        (0..=1_023_u32).map(|code| (cc8_as_f32(i32::try_from(code).unwrap()) - 64.0) / 876.0)
    }

    fn relative_error(actual: f32, expected: f32) -> f32 {
        ((actual - expected) / expected).abs()
    }

    // -----------------------------------------------------------------------
    // ST 2084.
    // -----------------------------------------------------------------------

    #[test]
    #[allow(clippy::float_cmp)]
    fn cc8_pq_constants_are_their_exact_rational_forms() {
        // Every ST 2084 constant is an exact binary fraction, so these are
        // equalities and not tolerances.
        assert_eq!(CC8_PQ_M1, 2_610.0 / 16_384.0);
        assert_eq!(CC8_PQ_M2, 2_523.0 / 4_096.0 * 128.0);
        assert_eq!(CC8_PQ_C1, 3_424.0 / 4_096.0);
        assert_eq!(CC8_PQ_C2, 2_413.0 / 4_096.0 * 32.0);
        assert_eq!(CC8_PQ_C3, 2_392.0 / 4_096.0 * 32.0);
        // The standard's own identity between the three `c` constants, which
        // catches a single mis-transcribed numerator.
        assert_eq!(CC8_PQ_C1, CC8_PQ_C3 - CC8_PQ_C2 + 1.0);
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn cc8_pq_eotf_holds_the_analytic_endpoints_exactly() {
        // `E' = 0` decodes to 0 cd/m²: the numerator's `max(·, 0)` floor.
        assert_eq!(cc8_pq_eotf_nits(0.0), 0.0);
        // `E' = 1` decodes to the peak exactly: at `p = 1` the numerator
        // `1 − c1` and the denominator `c2 − c3` are the same exact binary
        // fraction, so the ratio is exactly 1.
        assert_eq!(cc8_pq_eotf_nits(1.0), cc8_as_f32(CC8_PQ_PEAK_NITS));
        // `sgn(0) = 0` keeps zero exact in both directions.
        assert_eq!(cc8_pq_inverse_eotf(0.0), 0.0);
        // The seam the doc comment records: any strictly positive luminance,
        // however small, comes back at or above `c1^m2`, and everything at or
        // below `c1^m2` decodes to zero. So the EOTF has a flat foot and the
        // pair is an identity in one order only.
        let c1_to_the_m2 = CC8_PQ_C1.powf(CC8_PQ_M2);
        assert!(
            c1_to_the_m2 > 7.0e-7 && c1_to_the_m2 < 7.5e-7,
            "the ST 2084 foot c1^m2 is {c1_to_the_m2}",
        );
        assert_eq!(cc8_pq_eotf_nits(c1_to_the_m2), 0.0);
        // The inverse of any strictly positive luminance is above the foot,
        // approaching it from above as the luminance falls.
        let near_zero = cc8_pq_inverse_eotf(1.0e-20);
        assert!(near_zero > c1_to_the_m2, "{near_zero} vs {c1_to_the_m2}");
        assert!(near_zero < 1.0e-6, "the foot is under one 10-bit code");
        assert!(cc8_pq_inverse_eotf(1.0e-30) < near_zero);
    }

    #[test]
    fn cc8_pq_round_trips_over_the_ten_bit_ramp() {
        for signal in ten_bit_limited_ramp() {
            let round_tripped = cc8_pq_inverse_eotf(cc8_pq_eotf_nits(signal));
            if signal.abs() < 1.0e-3 {
                // Inside the flat foot the EOTF is not injective, by the
                // standard; the round trip is bounded, not exact.
                assert!(round_tripped.abs() < 1.0e-3);
                continue;
            }
            assert!(
                relative_error(round_tripped, signal) <= PQ_ROUND_TRIP_RELATIVE_BOUND,
                "PQ signal round trip at {signal}: {round_tripped}",
            );
        }
    }

    #[test]
    fn cc8_pq_luminance_round_trips_across_the_full_range() {
        let mut nits = 0.001_f32;
        while nits <= 20_000.0 {
            let round_tripped = cc8_pq_eotf_nits(cc8_pq_inverse_eotf(nits));
            assert!(
                relative_error(round_tripped, nits) <= PQ_ROUND_TRIP_RELATIVE_BOUND,
                "PQ luminance round trip at {nits} nits: {round_tripped}",
            );
            nits *= 1.05;
        }
    }

    #[test]
    fn cc8_pq_is_monotone_over_the_ten_bit_ramp() {
        let mut previous = f32::NEG_INFINITY;
        for signal in ten_bit_limited_ramp() {
            let nits = cc8_pq_eotf_nits(signal);
            assert!(nits >= previous, "PQ EOTF fell at signal {signal}");
            assert!(nits.is_finite(), "PQ EOTF is finite over 10-bit codes");
            previous = nits;
        }
        // Strictly increasing once clear of the flat foot: a monotone test
        // that a constant function would pass proves nothing.
        let low = cc8_pq_eotf_nits(0.001);
        let high = cc8_pq_eotf_nits(0.002);
        assert!(high > low);
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn cc8_pq_negative_extension_is_sign_preserving() {
        for signal in [0.001_f32, 0.05, 0.25, 0.580_69, 0.75, 1.0, 1.09] {
            assert_eq!(cc8_pq_eotf_nits(-signal), -cc8_pq_eotf_nits(signal));
        }
        for nits in [0.5_f32, 100.0, 203.0, 1_000.0, 10_000.0] {
            assert_eq!(cc8_pq_inverse_eotf(-nits), -cc8_pq_inverse_eotf(nits));
        }
        // `sgn(0) = 0`, the reason `f32::signum` is not used.
        assert_eq!(cc8_sign(0.0), 0.0);
        assert_eq!(cc8_pq_eotf_nits(0.0), 0.0);
        assert_eq!(cc8_hlg_oetf(0.0), 0.0);
        assert_eq!(cc8_hlg_inverse_oetf(0.0), 0.0);
    }

    #[test]
    fn cc8_pq_eotf_pole_is_where_the_st2084_denominator_vanishes() {
        // The rational form's pole, `E' = (c2/c3)^m2`, is above every 10-bit
        // code and so is unreachable from a real source.
        let pole = (CC8_PQ_C2 / CC8_PQ_C3).powf(CC8_PQ_M2);
        assert!(pole > 1.99 && pole < 2.0, "ST 2084 pole at {pole}");
        let highest_ten_bit_code = (1_023.0_f32 - 64.0) / 876.0;
        assert!(highest_ten_bit_code < pole);
        assert!(cc8_pq_eotf_nits(highest_ten_bit_code).is_finite());
        assert!(cc8_pq_eotf_nits(pole).is_infinite());
        assert!(cc8_pq_eotf_nits(-pole).is_infinite());
        assert!(cc8_pq_eotf_nits(-pole) < 0.0, "the pole is sign-preserving");
    }

    // -----------------------------------------------------------------------
    // The anchor.
    // -----------------------------------------------------------------------

    #[test]
    #[allow(clippy::float_cmp)]
    fn cc8_reference_white_anchor_is_bt2408s_two_hundred_and_three() {
        assert_eq!(CC8_REFERENCE_WHITE_NITS, 203);
        // §2.2's three worked values: diffuse white at 1.0, a 1 000-nit
        // highlight at ≈ 4.93, ST 2084's peak at ≈ 49.3.
        let white = cc8_pq_decode_working_linear(cc8_pq_inverse_eotf(203.0));
        assert!(
            relative_error(white, 1.0) <= PQ_ROUND_TRIP_RELATIVE_BOUND,
            "203 nits lands at working {white}, not 1.0",
        );
        let highlight = cc8_pq_decode_working_linear(cc8_pq_inverse_eotf(1_000.0));
        assert!(relative_error(highlight, 1_000.0 / 203.0) <= PQ_ROUND_TRIP_RELATIVE_BOUND);
        assert!(highlight > 4.92 && highlight < 4.93, "{highlight}");
        let peak = cc8_pq_decode_working_linear(1.0);
        assert_eq!(peak, 10_000.0 / 203.0);
        assert!(peak > 49.26 && peak < 49.27, "{peak}");
        // §3.1's headroom claim: the peak is far inside f16's 65 504.
        assert!(peak < 65_504.0);
    }

    #[test]
    fn cc8_working_linear_scale_round_trips_exactly() {
        for working in [-4.93_f32, 0.0, 1.0, 4.926_108, 49.261_086] {
            let round_tripped = cc8_nits_to_working_linear(cc8_working_linear_to_nits(working));
            assert!(
                (round_tripped - working).abs() <= 4.0 * f32::EPSILON * working.abs().max(1.0),
                "the anchor scale is a multiply and a divide by 203: {working}",
            );
        }
    }

    #[test]
    fn cc8_pq_working_linear_encode_inverts_the_decode() {
        for working in [0.01_f32, 0.5, 1.0, 4.926, 20.0, 49.26] {
            let round_tripped = cc8_pq_decode_working_linear(cc8_pq_encode_working_linear(working));
            assert!(
                relative_error(round_tripped, working) <= PQ_ROUND_TRIP_RELATIVE_BOUND,
                "working round trip at {working}: {round_tripped}",
            );
        }
    }

    // -----------------------------------------------------------------------
    // ARIB STD-B67.
    // -----------------------------------------------------------------------

    #[test]
    // The standard states `a`, `b`, and `c` to eight decimals, which is past
    // `f32`'s seven; the literals are written as the standard writes them
    // because reproducing the standard's own figure is the assertion, and
    // `excessive_precision` would have this test check a truncation instead.
    #[allow(clippy::float_cmp, clippy::excessive_precision)]
    fn cc8_hlg_constants_are_the_standard_decimals() {
        // The hundred-millionths integers narrow through `f64` to the same
        // `f32` the standard's decimals do, so the two-step rounding in
        // `cc8_hundred_millionths_f32` is not a second definition.
        assert_eq!(CC8_HLG_A, 0.178_832_77_f32);
        assert_eq!(CC8_HLG_B, 0.284_668_92_f32);
        assert_eq!(CC8_HLG_C, 0.559_910_73_f32);
        // `b = 1 − 4a` holds exactly in `f32` with the standard's rounded `a`.
        assert_eq!(CC8_HLG_B, 1.0 - 4.0 * CC8_HLG_A);
        // `c = 0.5 − a·ln(4a)` holds to the standard's own eight-decimal
        // rounding: the bound is half an ulp of an eight-decimal figure,
        // 5e-9, which is the accuracy the standard itself states `c` to. The
        // check runs on the pinned hundred-millionths integers in `f64`, not
        // on the narrowed `f32` constants, so it measures the standard's
        // rounding of `c` rather than `f32`'s rounding of `a`.
        #[allow(clippy::cast_precision_loss)]
        let a = CC8_HLG_A_HUNDRED_MILLIONTHS as f64 / 100_000_000.0;
        #[allow(clippy::cast_precision_loss)]
        let c = CC8_HLG_C_HUNDRED_MILLIONTHS as f64 / 100_000_000.0;
        let derived_c = 0.5_f64 - a * (4.0 * a).ln();
        assert!(
            (derived_c - c).abs() <= 5.0e-9,
            "c = 0.5 - a*ln(4a) gives {derived_c}",
        );
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn cc8_hlg_oetf_anchor_points() {
        assert_eq!(cc8_hlg_oetf(0.0), 0.0);
        // The seam: both branches are exactly 0.5 at E = 1/12 in `f32`.
        assert_eq!(
            cc8_hlg_oetf(CC8_HLG_SCENE_BREAKPOINT),
            CC8_HLG_SIGNAL_BREAKPOINT
        );
        let just_above = CC8_HLG_A * (12.0 * CC8_HLG_SCENE_BREAKPOINT - CC8_HLG_B).ln() + CC8_HLG_C;
        assert_eq!(just_above, CC8_HLG_SIGNAL_BREAKPOINT);
        // The upper anchor: `oetf(1) = 1`, exactly in `f32`.
        assert_eq!(cc8_hlg_oetf(1.0), 1.0);
        // And the inverse at the seam is exactly the breakpoint.
        assert_eq!(
            cc8_hlg_inverse_oetf(CC8_HLG_SIGNAL_BREAKPOINT),
            CC8_HLG_SCENE_BREAKPOINT
        );
    }

    #[test]
    fn cc8_hlg_round_trips_over_the_ten_bit_ramp() {
        for signal in ten_bit_limited_ramp() {
            if signal == 0.0 {
                continue;
            }
            let round_tripped = cc8_hlg_oetf(cc8_hlg_inverse_oetf(signal));
            assert!(
                relative_error(round_tripped, signal) <= HLG_ROUND_TRIP_RELATIVE_BOUND,
                "HLG signal round trip at {signal}: {round_tripped}",
            );
        }
        let mut scene = 1.0e-5_f32;
        while scene <= 12.0 {
            let round_tripped = cc8_hlg_inverse_oetf(cc8_hlg_oetf(scene));
            assert!(
                relative_error(round_tripped, scene) <= HLG_ROUND_TRIP_RELATIVE_BOUND,
                "HLG scene round trip at {scene}: {round_tripped}",
            );
            scene *= 1.01;
        }
    }

    #[test]
    fn cc8_hlg_is_monotone_over_the_ten_bit_ramp() {
        let mut previous = f32::NEG_INFINITY;
        for signal in ten_bit_limited_ramp() {
            let scene = cc8_hlg_inverse_oetf(signal);
            assert!(scene > previous, "HLG inverse OETF fell at signal {signal}");
            previous = scene;
        }
        let mut previous = f32::NEG_INFINITY;
        let mut scene = -1.0_f32;
        while scene <= 4.0 {
            let signal = cc8_hlg_oetf(scene);
            assert!(signal > previous, "HLG OETF fell at scene {scene}");
            previous = signal;
            scene += 0.01;
        }
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn cc8_hlg_negative_extension_is_sign_preserving() {
        for scene in [0.001_f32, 1.0 / 12.0, 0.2, 1.0, 4.93] {
            assert_eq!(cc8_hlg_oetf(-scene), -cc8_hlg_oetf(scene));
        }
        for signal in [0.1_f32, 0.5, 0.75, 1.0, 1.09] {
            assert_eq!(cc8_hlg_inverse_oetf(-signal), -cc8_hlg_inverse_oetf(signal));
        }
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn cc8_hlg_ootf_realizes_the_system_gamma_relation() {
        // On an achromatic triple the OOTF is exactly `peak · s^γ`, which is
        // the system-gamma relation §2.2 pins γ for.
        let gamma = cc8_hlg_system_gamma();
        assert!((gamma - 1.2).abs() <= f32::EPSILON);
        for scene in [0.05_f32, 0.264_962_6, 0.5, 1.0] {
            let display = cc8_hlg_ootf_nits_nominal([scene, scene, scene]);
            let expected = cc8_hlg_nominal_peak_nits() * scene.powf(gamma);
            assert!(
                relative_error(display[0], expected) <= HLG_ROUND_TRIP_RELATIVE_BOUND,
                "OOTF at {scene}: {} vs {expected}",
                display[0],
            );
            assert!(display[1] == display[0] && display[2] == display[0]);
        }
        // Unity scene signal lands on the nominal peak.
        let peak = cc8_hlg_ootf_nits_nominal([1.0, 1.0, 1.0]);
        assert!(
            relative_error(peak[0], cc8_hlg_nominal_peak_nits()) <= HLG_ROUND_TRIP_RELATIVE_BOUND
        );
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn cc8_hlg_ootf_round_trips_with_its_inverse() {
        for scene in [
            [0.1_f32, 0.1, 0.1],
            [0.5, 0.25, 0.75],
            [1.0, 0.2, 0.05],
            [0.3, -0.05, 0.6],
        ] {
            let display = cc8_hlg_ootf_nits_nominal(scene);
            let back = cc8_hlg_inverse_ootf_nominal(display);
            for channel in 0..3 {
                assert!(
                    (back[channel] - scene[channel]).abs()
                        <= HLG_ROUND_TRIP_RELATIVE_BOUND * scene[channel].abs().max(1.0),
                    "OOTF round trip on {scene:?}: {back:?}",
                );
            }
        }
        // The zero-luma seam both directions document.
        assert_eq!(cc8_hlg_ootf_nits_nominal([0.0, 0.0, 0.0]), [0.0, 0.0, 0.0]);
        assert_eq!(
            cc8_hlg_inverse_ootf_nominal([0.0, 0.0, 0.0]),
            [0.0, 0.0, 0.0]
        );
    }

    #[test]
    fn cc8_hlg_reference_white_signal_lands_on_the_anchor() {
        // BT.2408: the 75 % HLG signal is HDR reference white, 203 cd/m² on a
        // 1 000-nit display. The bound is half a nit because 203 is the
        // standard's own figure rounded to integer cd/m²: agreeing to better
        // than half a nit is exactly the claim that this reproduces it.
        let signal = cc8_as_f32(CC8_HLG_REFERENCE_WHITE_SIGNAL_PERCENT) / 100.0;
        let scene = cc8_hlg_inverse_oetf(signal);
        let display = cc8_hlg_ootf_nits_nominal([scene, scene, scene]);
        let anchor = cc8_as_f32(CC8_REFERENCE_WHITE_NITS);
        assert!(
            (display[0] - anchor).abs() < 0.5,
            "75% HLG is {} nits, not {anchor}",
            display[0],
        );
        // And therefore HLG diffuse white lands within the same half nit of
        // working 1.0, which is why one anchor serves both profiles.
        let working = cc8_nits_to_working_linear(display[0]);
        assert!((working - 1.0).abs() < 0.5 / anchor, "{working}");
    }

    #[test]
    fn cc8_hlg_working_linear_decode_lands_diffuse_white_on_one() {
        // §3.3's determination, stated as the number it produces: BT.2408's
        // 75 % HLG signal decodes to working 1.0, the same place PQ's 203 nits
        // lands, so one anchor serves both profiles. The bound is the same
        // half-nit-in-working-units argument
        // `cc8_hlg_reference_white_signal_lands_on_the_anchor` uses, because it
        // is the same claim carried through one more multiply.
        let signal = cc8_as_f32(CC8_HLG_REFERENCE_WHITE_SIGNAL_PERCENT) / 100.0;
        let working = cc8_hlg_decode_working_linear([signal, signal, signal]);
        let bound = 0.5 / cc8_as_f32(CC8_REFERENCE_WHITE_NITS);
        for channel in working {
            assert!(
                (channel - 1.0).abs() < bound,
                "75% HLG decodes to working {working:?}, not 1.0",
            );
        }
        // Unity HLG signal is the nominal peak, so it lands where a
        // 1 000-nit PQ highlight lands. Neither number is written here: both
        // are the pinned constants' quotient.
        let peak = cc8_hlg_decode_working_linear([1.0, 1.0, 1.0]);
        let expected = cc8_hlg_nominal_peak_nits() / cc8_as_f32(CC8_REFERENCE_WHITE_NITS);
        assert!(
            relative_error(peak[0], expected) <= HLG_ROUND_TRIP_RELATIVE_BOUND,
            "HLG peak lands at {peak:?}, not {expected}",
        );
        // The failing direction that catches the two rejected compositions:
        // stopping at the inverse OETF would leave diffuse white near 0.265,
        // and applying the inverse OOTF in the decode direction would leave it
        // near 0.001. Both are far outside the bound above, and asserting the
        // gap keeps this test from passing on a decode that is merely close.
        let stopped_at_the_oetf = cc8_hlg_inverse_oetf(signal);
        assert!(
            (stopped_at_the_oetf - 1.0).abs() > 0.7,
            "the inverse OETF alone must not already be working linear: {stopped_at_the_oetf}",
        );
        let wrong_ootf_direction = cc8_hlg_inverse_ootf_nominal([
            stopped_at_the_oetf,
            stopped_at_the_oetf,
            stopped_at_the_oetf,
        ]);
        assert!(
            wrong_ootf_direction[0] < 0.01,
            "the inverse OOTF is the delivery direction: {wrong_ootf_direction:?}",
        );
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn cc8_hlg_working_linear_pair_round_trips_and_preserves_sign() {
        for working in [
            [0.05_f32, 0.05, 0.05],
            [1.0, 1.0, 1.0],
            [4.926_108, 1.0, 0.25],
            [0.5, -0.05, 0.75],
        ] {
            let round_tripped =
                cc8_hlg_decode_working_linear(cc8_hlg_encode_working_linear(working));
            for channel in 0..3 {
                assert!(
                    (round_tripped[channel] - working[channel]).abs()
                        <= HLG_ROUND_TRIP_RELATIVE_BOUND * working[channel].abs().max(1.0),
                    "HLG working round trip of {working:?} gave {round_tripped:?}",
                );
            }
        }
        assert_eq!(
            cc8_hlg_decode_working_linear([0.0, 0.0, 0.0]),
            [0.0, 0.0, 0.0]
        );
    }

    // -----------------------------------------------------------------------
    // CC8 §2.1's closed profile set.
    // -----------------------------------------------------------------------

    #[test]
    fn cc8_source_profile_table_is_section_2_1s_two_closed_rows() {
        assert_eq!(CC8_SOURCE_PROFILES.len(), 2);
        let ids: Vec<&str> = CC8_SOURCE_PROFILES.iter().map(|row| row.id).collect();
        assert_eq!(ids, vec!["pq_rec2020", "hlg_rec2020"]);
        for row in &CC8_SOURCE_PROFILES {
            // Both rows are Rec.2020 D65 with the same matrix, range, and depth
            // columns; only the transfer distinguishes them, which is why
            // `cc8_source_profile_for_primaries_and_transfer` keys on the pair.
            assert_eq!(row.primaries, "bt2020");
            assert_eq!(row.white_point, "d65");
            assert!(row.accepts_matrix("bt2020_ncl") && row.accepts_matrix("rgb"));
            assert!(!row.accepts_matrix("bt2020_cl") && !row.accepts_matrix("ictcp"));
            assert!(row.accepts_range("limited") && row.accepts_range("full"));
            assert!(row.accepts_white_point("d65") && !row.accepts_white_point("unknown"));
            // §2.1's 10-bit floor, at the two codes either side of it.
            assert_eq!(row.min_integer_depth_bits, CC8_HDR_MIN_INTEGER_DEPTH_BITS);
            assert_eq!(row.max_integer_depth_bits, CC8_HDR_MAX_INTEGER_DEPTH_BITS);
            assert!(!row.accepts_integer_depth(8));
            assert!(!row.accepts_integer_depth(9));
            assert!(row.accepts_integer_depth(10));
            assert!(row.accepts_integer_depth(16));
            assert!(!row.accepts_integer_depth(17));
        }
        assert_eq!(
            cc8_source_profile_for_primaries_and_transfer("bt2020", "smpte2084").map(|row| row.id),
            Some("pq_rec2020"),
        );
        assert_eq!(
            cc8_source_profile_for_primaries_and_transfer("bt2020", "arib_std_b67")
                .map(|row| row.id),
            Some("hlg_rec2020"),
        );
        // The mismatched pairs §2.1 makes explicit failures: an HDR transfer
        // on non-Rec.2020 primaries, and Rec.2020 with an SDR transfer.
        assert!(!cc8_is_hdr_source_pair("bt709", "smpte2084"));
        assert!(!cc8_is_hdr_source_pair("bt709", "arib_std_b67"));
        assert!(!cc8_is_hdr_source_pair("display_p3", "smpte2084"));
        assert!(!cc8_is_hdr_source_pair("dci_p3", "arib_std_b67"));
        assert!(!cc8_is_hdr_source_pair("bt2020", "bt709"));
        assert!(!cc8_is_hdr_source_pair("bt2020", "unknown"));
        assert_eq!(
            cc8_source_profile_by_id("hlg_rec2020").map(|row| row.transfer),
            Some("arib_std_b67"),
        );
        assert!(cc8_source_profile_by_id("hdr10").is_none());
    }

    #[test]
    fn cc8_source_profile_wire_spellings_are_the_color_tag_serde_forms() {
        // The table stores wire spellings rather than `ColorPrimaries` values,
        // so this is the boundary assertion that keeps the transcription from
        // becoming a second definition — the rule
        // `cc8_pinned_matrices_are_the_derivation_transcribed` states for the
        // matrices, applied to the profile table.
        fn wire<T: serde::Serialize>(value: &T) -> String {
            serde_json::to_value(value)
                .expect("a colour tag serialises")
                .as_str()
                .expect("a colour tag serialises as a string")
                .to_owned()
        }

        assert_eq!(wire(&crate::ColorPrimaries::Bt2020), "bt2020");
        assert_eq!(wire(&crate::ColorTransfer::Smpte2084), "smpte2084");
        assert_eq!(wire(&crate::ColorTransfer::AribStdB67), "arib_std_b67");
        assert_eq!(wire(&crate::ColorMatrix::Bt2020Ncl), "bt2020_ncl");
        assert_eq!(wire(&crate::ColorMatrix::Rgb), "rgb");
        assert_eq!(wire(&crate::ColorRange::Limited), "limited");
        assert_eq!(wire(&crate::ColorRange::Full), "full");
        assert_eq!(wire(&crate::ColorWhitePoint::D65), "d65");
        for row in &CC8_SOURCE_PROFILES {
            assert_eq!(row.primaries, wire(&crate::ColorPrimaries::Bt2020));
            assert_eq!(row.white_point, wire(&crate::ColorWhitePoint::D65));
            assert!(row.accepts_matrix(&wire(&crate::ColorMatrix::Bt2020Ncl)));
            assert!(row.accepts_range(&wire(&crate::ColorRange::Limited)));
        }
        // And the rejected table's observed spellings are the same forms, so
        // §9.1 fixture 5 can drive the classifier straight from this table.
        for rejected in &CC8_REJECTED_HDR_ADJACENT {
            match rejected.observed {
                "bt2020_cl" => assert_eq!(wire(&crate::ColorMatrix::Bt2020Cl), rejected.observed),
                "ictcp" => assert_eq!(wire(&crate::ColorMatrix::Ictcp), rejected.observed),
                "chroma_derived_ncl" => {
                    assert_eq!(
                        wire(&crate::ColorMatrix::ChromaDerivedNcl),
                        rejected.observed
                    );
                }
                "chroma_derived_cl" => {
                    assert_eq!(
                        wire(&crate::ColorMatrix::ChromaDerivedCl),
                        rejected.observed
                    );
                }
                "display_p3" => {
                    assert_eq!(wire(&crate::ColorPrimaries::DisplayP3), rejected.observed);
                }
                "dci_p3" => assert_eq!(wire(&crate::ColorPrimaries::DciP3), rejected.observed),
                depth => assert!(
                    depth
                        .parse::<u8>()
                        .is_ok_and(|bits| !CC8_SOURCE_PROFILES[0].accepts_integer_depth(bits)),
                    "{depth} must be a depth the table rejects",
                ),
            }
        }
    }

    #[test]
    fn cc8_rejected_hdr_adjacent_covers_every_family_section_2_1_names() {
        // §2.1 names five families: `bt2020_cl`, `ictcp`, `chroma_derived_*`,
        // P3 primaries, and an HDR transfer on non-Rec.2020 primaries. The
        // last is a *pair* rather than a value, so it is asserted through
        // `cc8_is_hdr_source_pair` above rather than as a row here; the other
        // four plus §2.1's depth floor are rows.
        let fields: Vec<&str> = CC8_REJECTED_HDR_ADJACENT
            .iter()
            .map(|row| row.field)
            .collect();
        assert!(fields.contains(&"matrix"));
        assert!(fields.contains(&"primaries"));
        assert!(fields.contains(&"bit_depth"));
        for row in &CC8_REJECTED_HDR_ADJACENT {
            assert!(!row.reason.is_empty(), "{} has no reason", row.observed);
        }
        let mut observed: Vec<&str> = CC8_REJECTED_HDR_ADJACENT
            .iter()
            .map(|row| row.observed)
            .collect();
        observed.sort_unstable();
        observed.dedup();
        assert_eq!(
            observed.len(),
            CC8_REJECTED_HDR_ADJACENT.len(),
            "a duplicated row would hide a family rather than add one",
        );
    }

    // -----------------------------------------------------------------------
    // BT.2020 luma.
    // -----------------------------------------------------------------------

    #[test]
    #[allow(clippy::float_cmp)]
    fn cc8_bt2020_luma_coefficients_agree_at_both_widths() {
        // The integers sum to exactly one in their own unit, which is the
        // check that catches a mis-transcribed coefficient.
        assert_eq!(
            CC8_BT2020_KR_TEN_THOUSANDTHS
                + CC8_BT2020_KG_TEN_THOUSANDTHS
                + CC8_BT2020_KB_TEN_THOUSANDTHS,
            TEN_THOUSAND
        );
        assert_eq!(CC8_BT2020_KR, 0.2627);
        assert_eq!(CC8_BT2020_KG, 0.678);
        assert_eq!(CC8_BT2020_KB, 0.0593);
        assert_eq!(CC8_BT2020_CB_DENOMINATOR, 2.0 * (1.0 - 0.0593));
        assert_eq!(CC8_BT2020_CR_DENOMINATOR, 2.0 * (1.0 - 0.2627));
        // The `f32` triple is the same three numbers narrowed, not a second
        // transcription.
        #[allow(clippy::cast_possible_truncation)]
        let narrowed = [
            CC8_BT2020_KR as f32,
            CC8_BT2020_KG as f32,
            CC8_BT2020_KB as f32,
        ];
        assert_eq!(CC8_BT2020_LUMA_F32, narrowed);
        // A neutral triple has its own value as luma.
        assert!((cc8_bt2020_luma([1.0, 1.0, 1.0]) - 1.0).abs() <= 2.0 * f32::EPSILON);
    }

    // -----------------------------------------------------------------------
    // CC8 §2.3 primaries.
    // -----------------------------------------------------------------------

    #[test]
    #[allow(clippy::float_cmp)]
    fn cc8_pinned_matrices_are_the_derivation_transcribed() {
        // Bit-for-bit, not within a tolerance: the derivation uses only `+`,
        // `-`, `*`, and `/`, every one of which IEEE 754 defines exactly, so
        // the narrowing is deterministic on both CI operating systems.
        assert_eq!(
            CC8_REC2020_TO_BT709,
            cc8_narrow_matrix(cc8_derive_rec2020_to_bt709())
        );
        assert_eq!(
            CC8_BT709_TO_REC2020,
            cc8_narrow_matrix(cc8_derive_bt709_to_rec2020())
        );
    }

    #[test]
    fn cc8_primaries_matrices_are_mutually_inverse() {
        // In `f64`, where the derivation lives.
        let product =
            cc8_multiply_f64(cc8_derive_rec2020_to_bt709(), cc8_derive_bt709_to_rec2020());
        for (row, cells) in product.iter().enumerate() {
            for (column, cell) in cells.iter().enumerate() {
                let expected = if row == column { 1.0 } else { 0.0 };
                assert!(
                    (cell - expected).abs() <= MATRIX_IDENTITY_ABSOLUTE_BOUND_F64,
                    "f64 product[{row}][{column}] = {cell}",
                );
            }
        }
        // And in the pinned `f32` representation the pipeline will use.
        for (index, basis) in [[1.0_f32, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]
            .into_iter()
            .enumerate()
        {
            let there = cc8_apply_matrix(CC8_REC2020_TO_BT709, basis);
            let back = cc8_apply_matrix(CC8_BT709_TO_REC2020, there);
            for channel in 0..3 {
                let expected = if channel == index { 1.0 } else { 0.0 };
                assert!(
                    (back[channel] - expected).abs() <= MATRIX_IDENTITY_ABSOLUTE_BOUND,
                    "f32 round trip of basis {index}: {back:?}",
                );
            }
        }
    }

    #[test]
    fn cc8_primaries_preserve_the_shared_d65_white_point() {
        // Both sets are D65, so neutral maps to neutral. The residual is the
        // same five-rounding dot product bound.
        for matrix in [CC8_REC2020_TO_BT709, CC8_BT709_TO_REC2020] {
            let white = cc8_apply_matrix(matrix, [1.0, 1.0, 1.0]);
            for channel in white {
                assert!(
                    (channel - 1.0).abs() <= MATRIX_IDENTITY_ABSOLUTE_BOUND,
                    "neutral maps to {white:?}",
                );
            }
        }
    }

    #[test]
    fn cc8_primaries_round_trip_carries_negatives_not_a_clamp() {
        // §2.3: out-of-Rec.709 colours become negative BT.709 components and
        // must not be clamped. Asserting the negatives are present is what
        // keeps this from passing vacuously on in-gamut content (§9.1
        // fixture 3's rule, applied to the matrix in isolation).
        let mut saw_negative = false;
        for wide in [
            [1.0_f32, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0],
            [4.926_108, 0.0, 0.0],
        ] {
            let working = cc8_apply_matrix(CC8_REC2020_TO_BT709, wide);
            if working.iter().any(|channel| *channel < 0.0) {
                saw_negative = true;
            }
            let back = cc8_apply_matrix(CC8_BT709_TO_REC2020, working);
            for channel in 0..3 {
                assert!(
                    (back[channel] - wide[channel]).abs()
                        <= MATRIX_IDENTITY_ABSOLUTE_BOUND * wide[channel].abs().max(1.0),
                    "round trip of {wide:?} gave {back:?}",
                );
            }
        }
        assert!(
            saw_negative,
            "a saturated Rec.2020 primary must leave negative BT.709 components",
        );
    }

    #[test]
    fn cc8_derived_xyz_matrices_reproduce_the_standard_luma_coefficients() {
        // The middle row of RGB -> XYZ is the luminance row, so the BT.2020
        // derivation must reproduce BT.2020's own luma coefficients and the
        // BT.709 one must reproduce BT.709's. This is the check that the
        // chromaticities, not just the matrices, are transcribed correctly.
        let rec2020 = cc8_derive_rgb_to_xyz(
            CC8_REC2020_PRIMARIES_TEN_THOUSANDTHS,
            CC8_D65_TEN_THOUSANDTHS,
        );
        for (index, expected) in [CC8_BT2020_KR, CC8_BT2020_KG, CC8_BT2020_KB]
            .into_iter()
            .enumerate()
        {
            // BT.2020 rounds its published coefficients to four decimals, so
            // the bound is half an ulp of a four-decimal figure.
            assert!(
                (rec2020[1][index] - expected).abs() <= 5.0e-5,
                "BT.2020 luma row: {:?}",
                rec2020[1],
            );
        }
        let bt709 =
            cc8_derive_rgb_to_xyz(CC8_BT709_PRIMARIES_TEN_THOUSANDTHS, CC8_D65_TEN_THOUSANDTHS);
        for (index, expected) in [0.2126_f64, 0.7152, 0.0722].into_iter().enumerate() {
            assert!(
                (bt709[1][index] - expected).abs() <= 5.0e-5,
                "BT.709 luma row: {:?}",
                bt709[1],
            );
        }
    }

    // -----------------------------------------------------------------------
    // CC8 §9.2.
    // -----------------------------------------------------------------------

    #[test]
    fn cc8_gate_table_is_section_9_2s_six_rows_with_no_number_in_it() {
        assert_eq!(CC8_GATES.len(), 6);
        for gate in CC8_GATES {
            assert_eq!(
                gate.value,
                Cc8GateValue::ToBeMeasuredAtImplementation,
                "{} carries a number before §10 step {CC8_GATE_MEASUREMENT_STEP}",
                gate.gate,
            );
            assert!(!gate.gate.is_empty());
            assert!(gate.fixture >= 1 && gate.fixture <= 12, "{}", gate.gate);
        }
        // Six rows, six distinct shapes, six distinct names: a table that
        // repeated a shape would have lost one of §9.2's rows.
        let mut shapes: Vec<Cc8GateShape> = CC8_GATES.iter().map(|gate| gate.shape).collect();
        shapes.sort_by_key(|shape| format!("{shape:?}"));
        shapes.dedup();
        assert_eq!(shapes.len(), 6);
        let mut names: Vec<&str> = CC8_GATES.iter().map(|gate| gate.gate).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), 6);
    }

    // -----------------------------------------------------------------------
    // CC8 §5.1 / §5.2 — §10 step 6's lane.
    // -----------------------------------------------------------------------

    #[test]
    fn cc8_hdr_delivery_lane_is_section_5_1s_table() {
        let lane = CC8_HDR_DELIVERY_LANE;
        assert_eq!(lane.codec, "libx264");
        assert_eq!(lane.pixel_format, "yuv420p10le");
        assert_eq!(lane.primaries, "bt2020");
        assert_eq!(lane.transfer, "arib_std_b67");
        assert_eq!(lane.matrix, "bt2020_ncl");
        assert_eq!(lane.range, "limited");
        assert_eq!(lane.white_point, "d65");
        assert_eq!(lane.bit_depth_bits, 10);
        // §5.1's lane is HLG, and its own §2.1 source row is the HLG one: a
        // lane whose transfer had drifted to PQ would still read plausibly.
        let profile = cc8_source_profile_for_primaries_and_transfer(lane.primaries, lane.transfer)
            .expect("§5.1's lane must be one of §2.1's two profile shapes");
        assert_eq!(profile.id, "hlg_rec2020");
        // §2.1's floor and §5.1's `Bit depth | Ten` row are the same number.
        assert_eq!(lane.bit_depth_bits, CC8_HDR_MIN_INTEGER_DEPTH_BITS);
        assert!(profile.accepts_integer_depth(lane.bit_depth_bits));
        assert!(profile.accepts_matrix(lane.matrix));
        assert!(profile.accepts_range(lane.range));
        assert!(profile.accepts_white_point(lane.white_point));
    }

    #[test]
    fn cc8_hdr_delivery_x264_params_carry_section_5_2s_three_terms() {
        // §5.2 item 2's string, term by term, in x264's own vocabulary — which
        // is deliberately not the schema's (`arib-std-b67`, `bt2020nc`).
        let terms: Vec<&str> = CC8_HDR_DELIVERY_X264_PARAMS.split(':').collect();
        assert_eq!(
            terms,
            vec![
                "colorprim=bt2020",
                "transfer=arib-std-b67",
                "colormatrix=bt2020nc",
            ],
        );
        assert_eq!(
            CC8_HDR_DELIVERY_LANE.x264_params,
            CC8_HDR_DELIVERY_X264_PARAMS
        );
        // The SDR string is frozen at today's bytes (§5.2 item 2, §9.1 fixture 6).
        assert_eq!(
            CC8_SDR_DELIVERY_X264_PARAMS,
            "colorprim=bt709:transfer=bt709:colormatrix=bt709",
        );
        assert_ne!(CC8_HDR_DELIVERY_X264_PARAMS, CC8_SDR_DELIVERY_X264_PARAMS);
    }

    #[test]
    fn cc8_hdr_delivery_allowed_phrases_are_six_distinct_lane_named_strings() {
        let mut phrases = CC8_HDR_DELIVERY_ALLOWED_PHRASES.to_vec();
        phrases.sort_unstable();
        phrases.dedup();
        assert_eq!(
            phrases.len(),
            CC8_HDR_DELIVERY_ALLOWED_PHRASES.len(),
            "the recovery lookup keys on these phrases, so two equal ones would \
             make one field's refusal unreadable",
        );
        for phrase in CC8_HDR_DELIVERY_ALLOWED_PHRASES {
            assert!(
                phrase.contains("CC8 §5.1"),
                "an HDR-lane allowed phrase must name the lane: {phrase}",
            );
            // Distinct from every §2.1 *source* phrase, which reads the same
            // fields on the other side of the pipeline.
            for source_phrase in [
                CC8_HDR_PRIMARIES_ALLOWED,
                CC8_HDR_MATRIX_ALLOWED,
                CC8_HDR_RANGE_ALLOWED,
                CC8_HDR_WHITE_POINT_ALLOWED,
                CC8_HDR_DEPTH_ALLOWED,
            ] {
                assert_ne!(phrase, source_phrase);
            }
        }
        // §5.3's PQ bullet: the recovery names the deferral and offers no
        // conversion.
        assert!(CC8_PQ_DELIVERY_RECOVERY_ACTION.contains("§11"));
        assert!(CC8_PQ_DELIVERY_RECOVERY_ACTION.contains("No PQ-to-HLG conversion exists"));
        assert!(CC8_HDR_DELIVERY_RECOVERY_ACTION.contains("HDR-from-SDR"));
        assert!(CC8_HDR_DELIVERY_RECOVERY_ACTION.contains("second lane is a slice, not a flag"));
    }

    // -----------------------------------------------------------------------
    // CC8 §4: the preview tone map's four properties.
    // -----------------------------------------------------------------------

    /// §4 item 2's parameter is §2.2's pinned HLG nominal peak, not a second
    /// number that happens to equal it today.
    ///
    /// The equalities here are **exact-representation claims**, not tolerance
    /// comparisons: the peak is one division of two integers, so the two sides
    /// are the same arithmetic and must be the same `f32`.
    #[test]
    #[allow(clippy::float_cmp)]
    fn cc8_preview_peak_is_the_pinned_hlg_nominal_peak() {
        assert_eq!(CC8_PREVIEW_PEAK_NITS, CC8_HLG_NOMINAL_PEAK_NITS);
        assert_eq!(
            cc8_preview_peak_working_linear(),
            cc8_nits_to_working_linear(cc8_as_f32(CC8_HLG_NOMINAL_PEAK_NITS)),
        );
        // The curve only compresses when the peak is above diffuse white, and
        // `f(W) = 1` only anchors monitor white there for the same reason.
        assert!(cc8_preview_peak_working_linear() > 1.0);
    }

    /// §4 item 4's endpoint clause: `f(0) = 0` exactly, `f(W) = 1` to the
    /// arithmetic's own precision, and `f` never brightens.
    ///
    /// `f(0) = 0` is asserted exactly and deliberately — it is what the
    /// `sgn(0) = 0` convention buys, and an approximate zero there would be a
    /// different function. `f(W) = 1` carries the derived bound below instead.
    #[test]
    #[allow(clippy::float_cmp)]
    fn cc8_preview_tone_map_holds_its_endpoints() {
        assert_eq!(cc8_preview_tone_map(0.0), 0.0);
        assert_eq!(cc8_preview_tone_map(-0.0), 0.0);

        let peak = cc8_preview_peak_working_linear();
        assert!(
            (cc8_preview_tone_map(peak) - 1.0).abs() <= PREVIEW_ENDPOINT_BOUND,
            "f(W) = {} is not 1.0 within {PREVIEW_ENDPOINT_BOUND:e}",
            cc8_preview_tone_map(peak),
        );

        // No value is ever brightened, so nothing legal becomes clipped by the
        // preview that was not clipped without it.
        for step in 1..=2_000_u32 {
            let value = cc8_as_f32(i32::try_from(step).unwrap()) / 100.0;
            assert!(
                cc8_preview_tone_map(value) < value,
                "the tone map brightened {value}",
            );
        }
    }

    /// §4 item 4's monotonicity clause, over the whole real line the working
    /// space can carry — including the out-of-Rec.709 negatives §2.3 produces.
    ///
    /// The odd-extension equality is exact by construction — the negative arm
    /// is the positive arm's value with its sign flipped — so it is asserted
    /// exactly rather than within a tolerance that would hide a second
    /// definition.
    #[test]
    #[allow(clippy::float_cmp)]
    fn cc8_preview_tone_map_is_strictly_increasing_and_sign_preserving() {
        let mut previous = f32::NEG_INFINITY;
        for step in -6_000..=6_000_i32 {
            let value = cc8_as_f32(step) / 100.0;
            let mapped = cc8_preview_tone_map(value);
            assert!(
                mapped > previous,
                "the tone map fell at {value}: {previous} then {mapped}",
            );
            assert!(mapped.is_finite(), "non-finite tone map at {value}");
            assert_eq!(
                mapped.is_sign_negative() && mapped != 0.0,
                value < 0.0,
                "the tone map did not preserve the sign at {value}",
            );
            // The odd extension is exact, not approximate.
            assert_eq!(cc8_preview_tone_map(-value), -mapped);
            previous = mapped;
        }
    }

    /// §4 item 4's determinism clause at the arithmetic's own level: the curve
    /// uses only IEEE 754 exact operations, so repeated evaluation is bitwise
    /// identical and the RGB form is the scalar form per channel.
    #[test]
    #[allow(clippy::float_cmp)]
    fn cc8_preview_tone_map_is_bitwise_deterministic_and_per_channel() {
        for step in -600..=6_000_i32 {
            let value = cc8_as_f32(step) / 100.0;
            assert_eq!(
                cc8_preview_tone_map(value).to_bits(),
                cc8_preview_tone_map(value).to_bits(),
            );
        }
        let triple = [-0.25_f32, 1.0, 4.5];
        assert_eq!(
            cc8_preview_tone_map_rgb(triple),
            triple.map(cc8_preview_tone_map),
        );
    }

    /// §4 item 3's labels exist, name what the preview is not, and are distinct
    /// from each other; §3.2's two limitations name their own contract clauses.
    #[test]
    fn cc8_preview_and_node_limitation_prose_names_its_own_clauses() {
        assert!(CC8_PREVIEW_BADGE.contains("NOT A REFERENCE"));
        assert!(CC8_PREVIEW_LABEL.contains("not a monitoring"));
        assert!(CC8_PREVIEW_LABEL.contains("not calibrated"));
        assert_ne!(CC8_PREVIEW_BADGE, CC8_PREVIEW_LABEL);
        assert!(CC8_PREVIEW_NOT_CALIBRATED_REASON.contains("§4"));
        assert!(CC8_PREVIEW_DELIVERY_BOUNDARY.contains("§4 item 5"));
        assert!(CC8_PREVIEW_STAGE.starts_with("tone_map_preview"));

        assert!(CC8_AUTHORED_DOMAIN_LIMITATION.contains("§3.2 item 1"));
        // The prose's authored-domain figure is CC3's own constant, so a CC3
        // amendment that widened the domain would leave this sentence wrong and
        // this assertion red.
        assert!(
            CC8_AUTHORED_DOMAIN_LIMITATION.contains("10 000 bp"),
            "the limitation must state CC3's authored-domain top, {}",
            crate::effect::COLOR_CURVE_WHITE_BASIS_POINTS,
        );
        assert_eq!(crate::effect::COLOR_CURVE_WHITE_BASIS_POINTS, 10_000);
        assert!(CC8_QUALIFIER_LIMITATION.contains("§3.2 item 2"));
        assert!(CC8_QUALIFIER_LIMITATION.contains("same selector"));
        assert_ne!(
            CC8_AUTHORED_DOMAIN_LIMITATION_CODE,
            CC8_QUALIFIER_LIMITATION_CODE
        );
    }
}
