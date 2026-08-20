use kinewright_core::MediaError;

const MIN_CUBE_SIZE: u32 = 2;
const MAX_CUBE_SIZE: u32 = 64;

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CubeLut {
    pub size: u32,
    pub domain_min: [f32; 3],
    pub domain_max: [f32; 3],
    /// RGBA32F samples in IRIDAS red-fastest order.
    pub rgba: Vec<f32>,
}

impl CubeLut {
    pub(crate) fn identity() -> Self {
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
        }
    }
}

pub(crate) fn parse_cube_lut(source: &str) -> Result<CubeLut, MediaError> {
    let mut size = None;
    let mut domain_min = [0.0_f32; 3];
    let mut domain_max = [1.0_f32; 3];
    let mut values = Vec::<f32>::new();

    for (line_index, raw_line) in source.lines().enumerate() {
        let line = raw_line.split('#').next().unwrap_or_default().trim();
        if line.is_empty() {
            continue;
        }
        let fields = line.split_whitespace().collect::<Vec<_>>();
        let keyword = fields[0].to_ascii_uppercase();
        match keyword.as_str() {
            "TITLE" => {}
            "LUT_3D_SIZE" => {
                if fields.len() != 2 || size.is_some() || !values.is_empty() {
                    return Err(lut_error(line_index, "malformed or repeated LUT_3D_SIZE"));
                }
                let parsed = fields[1]
                    .parse::<u32>()
                    .map_err(|_| lut_error(line_index, "invalid LUT_3D_SIZE"))?;
                if !(MIN_CUBE_SIZE..=MAX_CUBE_SIZE).contains(&parsed) {
                    return Err(lut_error(
                        line_index,
                        "LUT_3D_SIZE must be in the inclusive range 2..=64",
                    ));
                }
                size = Some(parsed);
            }
            "LUT_1D_SIZE" | "LUT_2D_SIZE" => {
                return Err(lut_error(line_index, "only 3D .cube LUTs are supported"));
            }
            "DOMAIN_MIN" => parse_domain(&fields, line_index, &mut domain_min)?,
            "DOMAIN_MAX" => parse_domain(&fields, line_index, &mut domain_max)?,
            _ => {
                if size.is_none() || fields.len() != 3 {
                    return Err(lut_error(line_index, "expected one RGB LUT triple"));
                }
                for field in fields {
                    values.push(
                        field
                            .parse::<f32>()
                            .map_err(|_| lut_error(line_index, "invalid LUT sample"))?,
                    );
                }
            }
        }
    }

    let size =
        size.ok_or_else(|| MediaError::Backend(".cube LUT has no LUT_3D_SIZE".to_owned()))?;
    if domain_min.iter().zip(domain_max).any(|(minimum, maximum)| {
        !minimum.is_finite() || !maximum.is_finite() || *minimum >= maximum
    }) {
        return Err(MediaError::Backend(
            ".cube LUT domains must be finite and strictly increasing".to_owned(),
        ));
    }
    let expected = usize::try_from(size)
        .unwrap_or(usize::MAX)
        .saturating_pow(3)
        .saturating_mul(3);
    if values.len() != expected || values.iter().any(|value| !value.is_finite()) {
        return Err(MediaError::Backend(format!(
            ".cube LUT has {} scalar samples; expected {expected}",
            values.len()
        )));
    }
    let rgba = values
        .as_chunks::<3>()
        .0
        .iter()
        .flat_map(|rgb| [rgb[0], rgb[1], rgb[2], 1.0])
        .collect();
    Ok(CubeLut {
        size,
        domain_min,
        domain_max,
        rgba,
    })
}

fn parse_domain(
    fields: &[&str],
    line_index: usize,
    destination: &mut [f32; 3],
) -> Result<(), MediaError> {
    if fields.len() != 4 {
        return Err(lut_error(line_index, "domain requires three values"));
    }
    for (destination, field) in destination.iter_mut().zip(&fields[1..]) {
        *destination = field
            .parse::<f32>()
            .map_err(|_| lut_error(line_index, "invalid domain value"))?;
    }
    Ok(())
}

fn lut_error(zero_based_line: usize, message: &str) -> MediaError {
    MediaError::Backend(format!(
        ".cube LUT line {}: {message}",
        zero_based_line.saturating_add(1)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
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
        assert!(
            lut.domain_min
                .iter()
                .all(|value| (*value + 1.0).abs() < f32::EPSILON)
        );
        assert!(
            lut.domain_max
                .iter()
                .all(|value| (*value - 1.0).abs() < f32::EPSILON)
        );
        assert_eq!(&lut.rgba[4..8], &[1.0, 0.0, 0.0, 1.0]);
        assert_eq!(lut.rgba.len(), 32);
    }

    #[test]
    fn rejects_wrong_counts_non_finite_values_and_1d_luts() {
        assert!(parse_cube_lut("LUT_3D_SIZE 2\n0 0 0\n").is_err());
        assert!(parse_cube_lut("LUT_3D_SIZE 2\nNaN 0 0\n").is_err());
        assert!(parse_cube_lut("LUT_1D_SIZE 2\n0 0 0\n1 1 1\n").is_err());
    }
}
