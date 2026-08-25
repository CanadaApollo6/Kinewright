//! `.cube` 3D LUT parsing and the pinned canonical `.cube` serializer (CC4 §2.5, §2.6).
//!
//! The parser is the only place LUT bytes become samples. It reports typed
//! [`LutParseError`] codes with the 1-based line, the observed text, and the
//! allowed shape so import, the agent surface, and the UI can offer a stable
//! recovery action instead of parsing English.

use std::fmt::Write as _;

use kinewright_core::MediaError;

/// Smallest lattice edge length a `.cube` file may declare (CC4 §2.1).
pub const MIN_CUBE_SIZE: u32 = 2;
/// Largest lattice edge length a `.cube` file may declare.
///
/// CC4 §2.5 raises this from 64 to 65: 65 is the most common vendor export
/// grid and `65 - 1 = 64` is a power of two, which CC4 §3.5's exactness claim
/// depends on.
pub const MAX_CUBE_SIZE: u32 = 65;

/// The UTF-8 byte-order mark some exporters prepend to a `.cube` file.
const BYTE_ORDER_MARK: char = '\u{feff}';

/// The longest observed fragment an error message quotes back.
const OBSERVED_LIMIT: usize = 120;

/// Machine-readable `.cube` rejection codes (CC4 §2.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LutParseErrorCode {
    /// `LUT_1D_SIZE` or `LUT_2D_SIZE`: reserved, not evaluated by CC4.
    UnsupportedLutFormat,
    /// `LUT_3D_SIZE` outside [`MIN_CUBE_SIZE`]`..=`[`MAX_CUBE_SIZE`].
    LutSizeOutOfRange,
    /// Repeated or missing `LUT_3D_SIZE`, a non-triple data line, an
    /// unparsable number, or non-UTF-8 bytes.
    MalformedLutFile,
    /// A non-finite domain bound, or `DOMAIN_MIN >= DOMAIN_MAX` on a channel.
    LutDomainInvalid,
    /// The sample count is not `3 * S^3`.
    LutSampleCountMismatch,
    /// A `NaN` or infinite lattice sample.
    LutSampleNotFinite,
}

impl LutParseErrorCode {
    /// The stable `snake_case` token used in errors, manifests, and the UI.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UnsupportedLutFormat => "unsupported_lut_format",
            Self::LutSizeOutOfRange => "lut_size_out_of_range",
            Self::MalformedLutFile => "malformed_lut_file",
            Self::LutDomainInvalid => "lut_domain_invalid",
            Self::LutSampleCountMismatch => "lut_sample_count_mismatch",
            Self::LutSampleNotFinite => "lut_sample_not_finite",
        }
    }
}

/// One typed `.cube` rejection.
///
/// `line` is 1-based and absent when the fault is a property of the whole
/// file, such as a missing `LUT_3D_SIZE` or a sample-count mismatch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LutParseError {
    /// The stable rejection code.
    pub code: LutParseErrorCode,
    /// The 1-based source line, when the fault belongs to one line.
    pub line: Option<usize>,
    /// What the file contained, sanitized for display.
    pub observed: String,
    /// What the parser would have accepted.
    pub allowed: String,
}

impl LutParseError {
    fn new(code: LutParseErrorCode, line: Option<usize>, observed: &str, allowed: &str) -> Self {
        Self {
            code,
            line,
            observed: sanitize(observed),
            allowed: allowed.to_owned(),
        }
    }
}

impl std::fmt::Display for LutParseError {
    /// Render the pinned wire format
    /// `<code>: observed=<v>; allowed=<v>; line=<n>`.
    ///
    /// The `key=value` spelling is the one CC4 documents and the one the
    /// agent's anchored field parser reads back, and it is the same shape
    /// `LutStoreError` renders, so a single reader recovers `observed`,
    /// `allowed`, and `line` from either kind of LUT failure. A bare
    /// `observed <v>` would still parse, but leaving the two renderings
    /// different is how they drift apart.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{}: observed={}; allowed={}",
            self.code.as_str(),
            self.observed,
            self.allowed
        )?;
        if let Some(line) = self.line {
            write!(formatter, "; line={line}")?;
        }
        Ok(())
    }
}

impl std::error::Error for LutParseError {}

impl From<LutParseError> for MediaError {
    /// Map a typed parse rejection onto the crate's transport error.
    ///
    /// CC4 maps LUT errors to `MediaError::Backend` with a stable
    /// `<code>: …; observed=<v>; allowed=<v>; line=<n>` shape (a recorded
    /// departure from §2.5's `MediaError::Lut`), the same shape
    /// `LutStoreError` renders; the typed [`LutParseError`] stays public, via
    /// [`parse_cube_lut_typed`], for callers that need the structure.
    fn from(error: LutParseError) -> Self {
        Self::Backend(error.to_string())
    }
}

/// Quote file text back without control characters or unbounded length.
fn sanitize(text: &str) -> String {
    let mut sanitized = String::with_capacity(text.len().min(OBSERVED_LIMIT));
    for character in text.chars().take(OBSERVED_LIMIT) {
        if character.is_control() {
            sanitized.push(' ');
        } else {
            sanitized.push(character);
        }
    }
    if text.chars().nth(OBSERVED_LIMIT).is_some() {
        sanitized.push('…');
    }
    sanitized
}

/// A parsed 3D `.cube` lattice.
///
/// The samples are stored RGBA32F in IRIDAS red-fastest order so the
/// compositor can upload them without a second pass.
#[derive(Debug, Clone, PartialEq)]
pub struct CubeLut {
    /// Lattice edge length `S`.
    pub size: u32,
    /// `DOMAIN_MIN` per channel, defaulting to `0` when the file omits it.
    pub domain_min: [f32; 3],
    /// `DOMAIN_MAX` per channel, defaulting to `1` when the file omits it.
    pub domain_max: [f32; 3],
    /// RGBA32F samples in IRIDAS red-fastest order.
    pub rgba: Vec<f32>,
    /// The `TITLE` keyword when the file carried a non-empty one.
    pub title: Option<String>,
}

impl CubeLut {
    /// The `S = 2` identity lattice over `[0, 1]`.
    #[must_use]
    pub fn identity() -> Self {
        let mut rgba = Vec::with_capacity(32);
        for blue in [0.0_f32, 1.0] {
            for green in [0.0_f32, 1.0] {
                for red in [0.0_f32, 1.0] {
                    rgba.extend_from_slice(&[red, green, blue, 1.0]);
                }
            }
        }
        Self {
            size: 2,
            domain_min: [0.0; 3],
            domain_max: [1.0; 3],
            rgba,
            title: None,
        }
    }

    /// The number of RGB scalar samples the lattice holds: `3 * S^3`.
    ///
    /// This is the count CC4 §2.5's `lut_sample_count_mismatch` compares
    /// against, not the length of [`CubeLut::rgba`], which carries a fourth
    /// alpha component per lattice point for upload.
    #[must_use]
    pub fn sample_count(&self) -> usize {
        usize::try_from(self.size)
            .unwrap_or(usize::MAX)
            .saturating_pow(3)
            .saturating_mul(3)
    }

    /// One lattice point's RGB triple, indexed red-fastest.
    #[must_use]
    pub fn sample(&self, index: usize) -> Option<[f32; 3]> {
        let base = index.checked_mul(4)?;
        let triple = self.rgba.get(base..base.checked_add(3)?)?;
        Some([triple[0], triple[1], triple[2]])
    }

    /// The integer domain mirrors CC4 §2.1 records, rounded half away from zero.
    #[must_use]
    pub fn domain_millionths(&self) -> ([i64; 3], [i64; 3]) {
        (
            self.domain_min.map(millionths),
            self.domain_max.map(millionths),
        )
    }

    /// Serialize the lattice in CC4 §2.6's pinned canonical `.cube` form.
    ///
    /// LF endings, `{:.6}` fixed decimals for every number including the
    /// domain lines, `S^3` red-fastest sample lines, and no trailing blank
    /// line. This is the text the built-in look hashes are pinned over, so any
    /// change to it is a visible test failure.
    #[must_use]
    pub fn canonical_text(&self, title: &str) -> String {
        let lattice_points = usize::try_from(self.size)
            .unwrap_or(usize::MAX)
            .saturating_pow(3);
        let mut text = String::with_capacity(128 + lattice_points.saturating_mul(27));
        let _ = writeln!(text, "TITLE \"{title}\"");
        let _ = writeln!(text, "LUT_3D_SIZE {}", self.size);
        let _ = writeln!(
            text,
            "DOMAIN_MIN {:.6} {:.6} {:.6}",
            self.domain_min[0], self.domain_min[1], self.domain_min[2]
        );
        let _ = writeln!(
            text,
            "DOMAIN_MAX {:.6} {:.6} {:.6}",
            self.domain_max[0], self.domain_max[1], self.domain_max[2]
        );
        for point in 0..lattice_points {
            let [red, green, blue] = self.sample(point).unwrap_or([0.0; 3]);
            let _ = writeln!(text, "{red:.6} {green:.6} {blue:.6}");
        }
        text
    }
}

/// Round one domain bound to the integer millionths CC4 §2.1 stores.
#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
fn millionths(value: f32) -> i64 {
    // `f64::round` rounds half away from zero, which is the documented rule.
    // The clamp keeps an absurd domain from relying on saturating-cast
    // behaviour for its result.
    let scaled = (f64::from(value) * 1_000_000.0).round();
    scaled.clamp(i64::MIN as f64, i64::MAX as f64) as i64
}

/// Parse `.cube` bytes, stripping a leading UTF-8 BOM before decoding.
///
/// # Errors
///
/// Returns [`LutParseErrorCode::MalformedLutFile`] when the bytes are not
/// UTF-8, and otherwise whatever [`parse_cube_lut_typed`] reports.
pub fn parse_cube_lut_bytes(bytes: &[u8]) -> Result<CubeLut, LutParseError> {
    let source = std::str::from_utf8(bytes).map_err(|error| {
        let line = bytes
            .get(..error.valid_up_to())
            .map_or(1, |valid| valid.split(|byte| *byte == b'\n').count());
        LutParseError::new(
            LutParseErrorCode::MalformedLutFile,
            Some(line),
            &format!("a non-UTF-8 byte at offset {}", error.valid_up_to()),
            "UTF-8 text",
        )
    })?;
    parse_cube_lut_typed(source)
}

/// Parse a `.cube` document into a lattice, reporting CC4 §2.5's typed codes.
///
/// # Errors
///
/// Returns a [`LutParseError`] whose `code`, `line`, `observed`, and `allowed`
/// name exactly what the file contained and what would have been accepted.
pub fn parse_cube_lut_typed(source: &str) -> Result<CubeLut, LutParseError> {
    let mut parser = CubeParser::default();
    let source = source.strip_prefix(BYTE_ORDER_MARK).unwrap_or(source);
    for (line_index, raw_line) in source.lines().enumerate() {
        let line_number = line_index.saturating_add(1);
        let line = raw_line.split('#').next().unwrap_or_default().trim();
        if line.is_empty() {
            continue;
        }
        parser.consume_line(line_number, line)?;
    }
    parser.finish()
}

/// Parse a `.cube` document, mapping the typed rejection onto [`MediaError`].
///
/// # Errors
///
/// Returns a [`MediaError::Backend`] whose message begins with the stable
/// [`LutParseErrorCode::as_str`] token.
pub fn parse_cube_lut(source: &str) -> Result<CubeLut, MediaError> {
    parse_cube_lut_typed(source).map_err(MediaError::from)
}

/// Line-by-line accumulator for [`parse_cube_lut_typed`].
#[derive(Debug)]
struct CubeParser {
    title: Option<String>,
    size: Option<u32>,
    domain_min: [f32; 3],
    domain_max: [f32; 3],
    domain_line: Option<usize>,
    values: Vec<f32>,
}

impl Default for CubeParser {
    fn default() -> Self {
        Self {
            title: None,
            size: None,
            domain_min: [0.0; 3],
            domain_max: [1.0; 3],
            domain_line: None,
            values: Vec::new(),
        }
    }
}

impl CubeParser {
    fn consume_line(&mut self, line: usize, text: &str) -> Result<(), LutParseError> {
        let fields = text.split_whitespace().collect::<Vec<_>>();
        let Some(first) = fields.first() else {
            return Ok(());
        };
        let keyword = first.to_ascii_uppercase();
        match keyword.as_str() {
            "TITLE" => {
                self.consume_title(text, first.len());
                Ok(())
            }
            "LUT_3D_SIZE" => self.consume_size(line, text, &fields),
            "LUT_1D_SIZE" | "LUT_2D_SIZE" => Err(LutParseError::new(
                LutParseErrorCode::UnsupportedLutFormat,
                Some(line),
                text,
                "a 3D .cube LUT declared with LUT_3D_SIZE",
            )),
            "DOMAIN_MIN" => {
                Self::consume_domain(line, text, &fields, &mut self.domain_min)?;
                self.domain_line = Some(line);
                Ok(())
            }
            "DOMAIN_MAX" => {
                Self::consume_domain(line, text, &fields, &mut self.domain_max)?;
                self.domain_line = Some(line);
                Ok(())
            }
            _ => self.consume_samples(line, text, &fields),
        }
    }

    /// Capture `TITLE`, accepting a quoted or a bare remainder of the line.
    fn consume_title(&mut self, text: &str, keyword_len: usize) {
        let remainder = text.split_at(keyword_len).1.trim();
        let unquoted = remainder
            .strip_prefix('"')
            .and_then(|rest| rest.strip_suffix('"'))
            .unwrap_or(remainder)
            .trim();
        if !unquoted.is_empty() {
            self.title = Some(unquoted.to_owned());
        }
    }

    fn consume_size(
        &mut self,
        line: usize,
        text: &str,
        fields: &[&str],
    ) -> Result<(), LutParseError> {
        if fields.len() != 2 {
            return Err(LutParseError::new(
                LutParseErrorCode::MalformedLutFile,
                Some(line),
                text,
                "LUT_3D_SIZE followed by exactly one integer",
            ));
        }
        if self.size.is_some() || !self.values.is_empty() {
            return Err(LutParseError::new(
                LutParseErrorCode::MalformedLutFile,
                Some(line),
                text,
                "exactly one LUT_3D_SIZE keyword, before any sample line",
            ));
        }
        let declared = fields[1];
        if !declared.bytes().all(|byte| byte.is_ascii_digit()) || declared.is_empty() {
            return Err(LutParseError::new(
                LutParseErrorCode::MalformedLutFile,
                Some(line),
                declared,
                "a decimal integer",
            ));
        }
        let parsed = declared.parse::<u64>().unwrap_or(u64::MAX);
        if parsed < u64::from(MIN_CUBE_SIZE) || parsed > u64::from(MAX_CUBE_SIZE) {
            return Err(LutParseError::new(
                LutParseErrorCode::LutSizeOutOfRange,
                Some(line),
                declared,
                &format!("an integer in {MIN_CUBE_SIZE}..={MAX_CUBE_SIZE}"),
            ));
        }
        self.size = Some(u32::try_from(parsed).unwrap_or(MAX_CUBE_SIZE));
        Ok(())
    }

    fn consume_domain(
        line: usize,
        text: &str,
        fields: &[&str],
        destination: &mut [f32; 3],
    ) -> Result<(), LutParseError> {
        if fields.len() != 4 {
            return Err(LutParseError::new(
                LutParseErrorCode::MalformedLutFile,
                Some(line),
                text,
                "a domain keyword followed by exactly three values",
            ));
        }
        for (channel, field) in destination.iter_mut().zip(&fields[1..]) {
            let value = field.parse::<f32>().map_err(|_| {
                LutParseError::new(
                    LutParseErrorCode::MalformedLutFile,
                    Some(line),
                    field,
                    "a decimal number",
                )
            })?;
            if !value.is_finite() {
                return Err(LutParseError::new(
                    LutParseErrorCode::LutDomainInvalid,
                    Some(line),
                    field,
                    "a finite domain bound",
                ));
            }
            *channel = value;
        }
        Ok(())
    }

    fn consume_samples(
        &mut self,
        line: usize,
        text: &str,
        fields: &[&str],
    ) -> Result<(), LutParseError> {
        if self.size.is_none() {
            return Err(LutParseError::new(
                LutParseErrorCode::MalformedLutFile,
                Some(line),
                text,
                "LUT_3D_SIZE before the first sample line",
            ));
        }
        if fields.len() != 3 {
            return Err(LutParseError::new(
                LutParseErrorCode::MalformedLutFile,
                Some(line),
                text,
                "exactly three whitespace-separated sample values",
            ));
        }
        for field in fields {
            let value = field.parse::<f32>().map_err(|_| {
                LutParseError::new(
                    LutParseErrorCode::MalformedLutFile,
                    Some(line),
                    field,
                    "a decimal number",
                )
            })?;
            if !value.is_finite() {
                return Err(LutParseError::new(
                    LutParseErrorCode::LutSampleNotFinite,
                    Some(line),
                    field,
                    "a finite sample value",
                ));
            }
            self.values.push(value);
        }
        Ok(())
    }

    fn finish(self) -> Result<CubeLut, LutParseError> {
        let size = self.size.ok_or_else(|| {
            LutParseError::new(
                LutParseErrorCode::MalformedLutFile,
                None,
                "no LUT_3D_SIZE keyword",
                "exactly one LUT_3D_SIZE keyword",
            )
        })?;
        for (channel, (minimum, maximum)) in
            self.domain_min.into_iter().zip(self.domain_max).enumerate()
        {
            if minimum >= maximum {
                return Err(LutParseError::new(
                    LutParseErrorCode::LutDomainInvalid,
                    self.domain_line,
                    &format!("channel {channel}: DOMAIN_MIN {minimum}, DOMAIN_MAX {maximum}"),
                    "DOMAIN_MIN strictly less than DOMAIN_MAX on every channel",
                ));
            }
        }
        let expected = usize::try_from(size)
            .unwrap_or(usize::MAX)
            .saturating_pow(3)
            .saturating_mul(3);
        if self.values.len() != expected {
            return Err(LutParseError::new(
                LutParseErrorCode::LutSampleCountMismatch,
                None,
                &self.values.len().to_string(),
                &format!("{expected} scalar samples (3 * {size}^3)"),
            ));
        }
        let rgba = self
            .values
            .as_chunks::<3>()
            .0
            .iter()
            .flat_map(|rgb| [rgb[0], rgb[1], rgb[2], 1.0])
            .collect();
        Ok(CubeLut {
            size,
            domain_min: self.domain_min,
            domain_max: self.domain_max,
            rgba,
            title: self.title,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a syntactically valid `.cube` document of the requested size with
    /// a lattice that is trivially reproducible by hand.
    fn identity_source(size: u32) -> String {
        let mut source = format!("LUT_3D_SIZE {size}\n");
        let last = f64::from(size - 1);
        for blue in 0..size {
            for green in 0..size {
                for red in 0..size {
                    let _ = writeln!(
                        source,
                        "{:.6} {:.6} {:.6}",
                        f64::from(red) / last,
                        f64::from(green) / last,
                        f64::from(blue) / last
                    );
                }
            }
        }
        source
    }

    /// The domain bounds under test are exact binary fractions parsed from
    /// text, so an exact comparison is the assertion the contract wants.
    #[test]
    #[allow(clippy::float_cmp)]
    fn parses_red_fastest_iridas_cube_and_domains() {
        let source = "\
LUT_3D_SIZE 2
DOMAIN_MIN -1.0 -1.0 -1.0
DOMAIN_MAX 1.0 1.0 1.0
0 0 0
1 0 0
0 1 0
1 1 0
0 0 1
1 0 1
0 1 1
1 1 1
";
        let lut = parse_cube_lut(source).unwrap();
        assert_eq!(lut.size, 2);
        assert_eq!(lut.domain_min, [-1.0; 3]);
        assert_eq!(lut.domain_max, [1.0; 3]);
        assert_eq!(&lut.rgba[4..8], &[1.0, 0.0, 0.0, 1.0]);
        assert_eq!(lut.rgba.len(), 32);
        assert_eq!(lut.sample_count(), 24);
        assert_eq!(lut.sample(1), Some([1.0, 0.0, 0.0]));
        assert_eq!(lut.title, None);
        assert_eq!(lut.domain_millionths(), ([-1_000_000; 3], [1_000_000; 3]));
    }

    #[test]
    fn accepts_every_supported_lattice_size() {
        for size in [2_u32, 17, 33, 65] {
            let lut = parse_cube_lut_typed(&identity_source(size))
                .unwrap_or_else(|error| panic!("size {size} should parse: {error}"));
            assert_eq!(lut.size, size);
            let points = usize::try_from(size).unwrap().pow(3);
            assert_eq!(lut.sample_count(), points * 3);
            assert_eq!(lut.rgba.len(), points * 4);
        }
    }

    #[test]
    fn captures_quoted_and_bare_titles() {
        let quoted = parse_cube_lut_typed("TITLE \"Kodak 2383 D65\"\nLUT_3D_SIZE 2\n0 0 0\n1 0 0\n0 1 0\n1 1 0\n0 0 1\n1 0 1\n0 1 1\n1 1 1\n").unwrap();
        assert_eq!(quoted.title.as_deref(), Some("Kodak 2383 D65"));
        let bare = parse_cube_lut_typed("title Kodak 2383 D65\nLUT_3D_SIZE 2\n0 0 0\n1 0 0\n0 1 0\n1 1 0\n0 0 1\n1 0 1\n0 1 1\n1 1 1\n").unwrap();
        assert_eq!(bare.title.as_deref(), Some("Kodak 2383 D65"));
        let empty = parse_cube_lut_typed(
            "TITLE \"\"\nLUT_3D_SIZE 2\n0 0 0\n1 0 0\n0 1 0\n1 1 0\n0 0 1\n1 0 1\n0 1 1\n1 1 1\n",
        )
        .unwrap();
        assert_eq!(empty.title, None);
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn accepts_comments_blank_lines_crlf_lowercase_scientific_and_a_bom() {
        let source = "\u{feff}# a leading comment\r\n\r\nlut_3d_size 2\r\ndomain_min 0 0 0\r\ndomain_max 1e0 1E0 1.0\r\n0 0 0\r\n1e0 0 0 # trailing comment\r\n0 1 0\r\n1 1 0\r\n0 0 1\r\n1 0 1\r\n0 1 1\r\n1 1 1\r\n";
        let lut = parse_cube_lut_typed(source).unwrap();
        assert_eq!(lut.size, 2);
        assert_eq!(lut.domain_max, [1.0; 3]);
        assert_eq!(lut.sample(1), Some([1.0, 0.0, 0.0]));
    }

    #[test]
    fn rejects_one_dimensional_luts_with_a_typed_code() {
        let error = parse_cube_lut_typed("LUT_1D_SIZE 2\n0 0 0\n1 1 1\n").unwrap_err();
        assert_eq!(error.code, LutParseErrorCode::UnsupportedLutFormat);
        assert_eq!(error.code.as_str(), "unsupported_lut_format");
        assert_eq!(error.line, Some(1));
        assert_eq!(error.observed, "LUT_1D_SIZE 2");
        assert_eq!(error.allowed, "a 3D .cube LUT declared with LUT_3D_SIZE");
    }

    #[test]
    fn rejects_sizes_outside_the_supported_range() {
        for declared in ["1", "66"] {
            let error =
                parse_cube_lut_typed(&format!("LUT_3D_SIZE {declared}\n0 0 0\n")).unwrap_err();
            assert_eq!(error.code, LutParseErrorCode::LutSizeOutOfRange);
            assert_eq!(error.line, Some(1));
            assert_eq!(error.observed, declared);
            assert_eq!(error.allowed, "an integer in 2..=65");
        }
    }

    #[test]
    fn rejects_a_two_value_data_line_with_its_line_number() {
        let error = parse_cube_lut_typed("LUT_3D_SIZE 2\n0 0 0\n1 0\n").unwrap_err();
        assert_eq!(error.code, LutParseErrorCode::MalformedLutFile);
        assert_eq!(error.line, Some(3));
        assert_eq!(error.observed, "1 0");
        assert_eq!(
            error.allowed,
            "exactly three whitespace-separated sample values"
        );
    }

    #[test]
    fn rejects_a_repeated_size_keyword_with_its_line_number() {
        let error = parse_cube_lut_typed("LUT_3D_SIZE 2\nLUT_3D_SIZE 3\n").unwrap_err();
        assert_eq!(error.code, LutParseErrorCode::MalformedLutFile);
        assert_eq!(error.line, Some(2));
        assert_eq!(error.observed, "LUT_3D_SIZE 3");
        assert_eq!(
            error.allowed,
            "exactly one LUT_3D_SIZE keyword, before any sample line"
        );
    }

    #[test]
    fn rejects_a_missing_size_keyword() {
        let error = parse_cube_lut_typed("TITLE \"no size\"\n").unwrap_err();
        assert_eq!(error.code, LutParseErrorCode::MalformedLutFile);
        assert_eq!(error.line, None);
        assert_eq!(error.observed, "no LUT_3D_SIZE keyword");
        assert_eq!(error.allowed, "exactly one LUT_3D_SIZE keyword");
    }

    #[test]
    fn rejects_a_collapsed_and_a_non_finite_domain() {
        let collapsed = parse_cube_lut_typed(
            "LUT_3D_SIZE 2\nDOMAIN_MIN 1 1 1\nDOMAIN_MAX 1 1 1\n0 0 0\n1 0 0\n0 1 0\n1 1 0\n0 0 1\n1 0 1\n0 1 1\n1 1 1\n",
        )
        .unwrap_err();
        assert_eq!(collapsed.code, LutParseErrorCode::LutDomainInvalid);
        assert_eq!(collapsed.line, Some(3));
        assert_eq!(collapsed.observed, "channel 0: DOMAIN_MIN 1, DOMAIN_MAX 1");
        assert_eq!(
            collapsed.allowed,
            "DOMAIN_MIN strictly less than DOMAIN_MAX on every channel"
        );

        let non_finite =
            parse_cube_lut_typed("LUT_3D_SIZE 2\nDOMAIN_MAX inf 1 1\n0 0 0\n").unwrap_err();
        assert_eq!(non_finite.code, LutParseErrorCode::LutDomainInvalid);
        assert_eq!(non_finite.line, Some(2));
        assert_eq!(non_finite.observed, "inf");
        assert_eq!(non_finite.allowed, "a finite domain bound");
    }

    #[test]
    fn rejects_sample_counts_one_above_and_one_below_the_lattice() {
        let short = parse_cube_lut_typed("LUT_3D_SIZE 2\n0 0 0\n").unwrap_err();
        assert_eq!(short.code, LutParseErrorCode::LutSampleCountMismatch);
        assert_eq!(short.line, None);
        assert_eq!(short.observed, "3");
        assert_eq!(short.allowed, "24 scalar samples (3 * 2^3)");

        let mut long = identity_source(2);
        long.push_str("1 1 1\n");
        let error = parse_cube_lut_typed(&long).unwrap_err();
        assert_eq!(error.code, LutParseErrorCode::LutSampleCountMismatch);
        assert_eq!(error.observed, "27");
    }

    #[test]
    fn rejects_non_finite_samples_with_the_offending_token() {
        for token in ["NaN", "inf", "-inf"] {
            let error =
                parse_cube_lut_typed(&format!("LUT_3D_SIZE 2\n0 0 0\n{token} 0 0\n")).unwrap_err();
            assert_eq!(error.code, LutParseErrorCode::LutSampleNotFinite);
            assert_eq!(error.line, Some(3));
            assert_eq!(error.observed, token);
            assert_eq!(error.allowed, "a finite sample value");
        }
    }

    #[test]
    fn rejects_an_unparsable_sample_token() {
        let error = parse_cube_lut_typed("LUT_3D_SIZE 2\n0 0 zero\n").unwrap_err();
        assert_eq!(error.code, LutParseErrorCode::MalformedLutFile);
        assert_eq!(error.line, Some(2));
        assert_eq!(error.observed, "zero");
        assert_eq!(error.allowed, "a decimal number");
    }

    #[test]
    fn rejects_non_utf8_bytes_as_a_malformed_file() {
        let mut bytes = b"LUT_3D_SIZE 2\n0 0 0\n".to_vec();
        bytes.push(0xff);
        let error = parse_cube_lut_bytes(&bytes).unwrap_err();
        assert_eq!(error.code, LutParseErrorCode::MalformedLutFile);
        assert_eq!(error.line, Some(3));
        assert_eq!(error.observed, "a non-UTF-8 byte at offset 20");
        assert_eq!(error.allowed, "UTF-8 text");
    }

    #[test]
    fn strips_a_byte_order_mark_from_bytes_as_well_as_text() {
        let mut bytes = "\u{feff}".as_bytes().to_vec();
        bytes.extend_from_slice(identity_source(2).as_bytes());
        assert_eq!(parse_cube_lut_bytes(&bytes).unwrap().size, 2);
    }

    #[test]
    fn media_error_conversion_keeps_the_code_as_a_stable_prefix() {
        let error = parse_cube_lut("LUT_1D_SIZE 2\n").unwrap_err();
        let MediaError::Backend(message) = error else {
            panic!("the parser maps rejections onto MediaError::Backend");
        };
        assert!(
            message.starts_with("unsupported_lut_format: "),
            "message should lead with the code: {message}"
        );
        // CC4 §2.5's wire format is `key=value`, anchored at `"; "`, which is
        // what the agent's field reader recovers `observed`, `allowed`, and
        // `line` from. `observed <v>` would parse too, but only because the
        // reader tolerates both spellings; the rendering is pinned here so it
        // cannot drift away from the documented one.
        assert!(
            message.contains("; line=1"),
            "message should name the line as an anchored field: {message}"
        );
        assert!(
            message.contains("observed=LUT_1D_SIZE"),
            "message should quote the observed keyword: {message}"
        );
        assert!(
            message.contains("; allowed="),
            "message should name what was allowed: {message}"
        );
    }

    #[test]
    fn the_rendered_parse_failure_is_the_documented_field_shape() {
        // The exact string, so a change to the wire format is a test failure
        // rather than a silently broken agent parser.
        let typed = parse_cube_lut_typed("LUT_3D_SIZE 1\n").unwrap_err();
        assert_eq!(typed.code, LutParseErrorCode::LutSizeOutOfRange);
        assert_eq!(
            typed.to_string(),
            format!(
                "lut_size_out_of_range: observed=1; \
                 allowed=an integer in {MIN_CUBE_SIZE}..={MAX_CUBE_SIZE}; line=1"
            )
        );

        // A whole-file fault has no line, and simply omits that field.
        let no_line = parse_cube_lut_typed("TITLE \"no size\"\n").unwrap_err();
        assert_eq!(no_line.line, None);
        assert!(
            !no_line.to_string().contains("line="),
            "a file-scoped fault names no line: {no_line}"
        );
    }

    #[test]
    fn canonical_text_is_byte_exact_for_a_tiny_lattice() {
        let source = "\
LUT_3D_SIZE 2
DOMAIN_MIN -1 -1 -1
DOMAIN_MAX 2 2 2
0 0 0
0.5 0 0
0 0.5 0
0.5 0.5 0
0 0 0.5
0.5 0 0.5
0 0.5 0.5
1 1 1
";
        let lut = parse_cube_lut_typed(source).unwrap();
        let expected = concat!(
            "TITLE \"kinewright.test.v1\"\n",
            "LUT_3D_SIZE 2\n",
            "DOMAIN_MIN -1.000000 -1.000000 -1.000000\n",
            "DOMAIN_MAX 2.000000 2.000000 2.000000\n",
            "0.000000 0.000000 0.000000\n",
            "0.500000 0.000000 0.000000\n",
            "0.000000 0.500000 0.000000\n",
            "0.500000 0.500000 0.000000\n",
            "0.000000 0.000000 0.500000\n",
            "0.500000 0.000000 0.500000\n",
            "0.000000 0.500000 0.500000\n",
            "1.000000 1.000000 1.000000\n",
        );
        assert_eq!(lut.canonical_text("kinewright.test.v1"), expected);
        assert!(!expected.contains('\r'), "the serializer is LF only");
        assert!(
            expected.ends_with("1.000000 1.000000 1.000000\n"),
            "no trailing blank line"
        );
    }

    #[test]
    fn canonical_text_round_trips_through_the_parser_in_lf_and_crlf_form() {
        let lut = parse_cube_lut_typed(&identity_source(2)).unwrap();
        let text = lut.canonical_text("kinewright.test.v1");
        let reparsed = parse_cube_lut_typed(&text).unwrap();
        assert_eq!(reparsed.rgba, lut.rgba);
        assert_eq!(reparsed.title.as_deref(), Some("kinewright.test.v1"));
        let crlf = text.replace('\n', "\r\n");
        assert_eq!(parse_cube_lut_typed(&crlf).unwrap().rgba, lut.rgba);
    }

    #[test]
    fn domain_millionths_round_half_away_from_zero() {
        let mut lut = CubeLut::identity();
        // 1/128 scales to exactly 7812.5 millionths, so this is a true tie:
        // half away from zero gives 7813, while half-to-even would give 7812.
        lut.domain_min = [-0.007_812_5, -0.25, 0.0];
        lut.domain_max = [0.007_812_5, 0.25, 1.0];
        let (minimum, maximum) = lut.domain_millionths();
        assert_eq!(minimum, [-7813, -250_000, 0]);
        assert_eq!(maximum, [7813, 250_000, 1_000_000]);
    }

    #[test]
    fn maximum_size_matches_the_core_asset_bound() {
        assert_eq!(MIN_CUBE_SIZE, kinewright_core::LUT_SIZE_MIN);
        assert_eq!(MAX_CUBE_SIZE, kinewright_core::LUT_SIZE_MAX);
    }
}
