//! Frame, scope, and CC6 colour QC inspectors.

use super::*;

impl KinewrightMcp {
    pub(super) fn frame_at(&self, timecode: TimeCode) -> Result<CallToolResult, McpError> {
        let document = self.document()?;
        if let Some(error) = self.document_availability_error(&document, "frame proof") {
            return Ok(error);
        }
        if timecode < TimeCode::ZERO || timecode >= document.duration {
            return Ok(error_text(format!(
                "frame {} is outside project range 0..{}",
                timecode.0, document.duration.0
            )));
        }
        let image =
            match self
                .analysis
                .thumbnail_for_document(document, timecode, THUMBNAIL_MAX_WIDTH)
            {
                Ok(image) => image,
                Err(error) => return Ok(error_text(error.to_string())),
            };
        let png = encode_png(&image)?;
        Ok(CallToolResult::success(vec![
            ContentBlock::text(format!(
                "project frame {} ({}x{})",
                timecode.0, image.width, image.height
            )),
            ContentBlock::image(BASE64.encode(png), "image/png"),
        ]))
    }

    pub(super) fn video_scopes(&self, args: &VideoScopesArgs) -> Result<CallToolResult, McpError> {
        let document = self.document()?;
        if let Some(error) = self.document_availability_error(&document, "video scopes") {
            return Ok(error);
        }
        if args.timecode < TimeCode::ZERO || args.timecode >= document.duration {
            return Ok(error_text(format!(
                "frame {} is outside project range 0..{}",
                args.timecode.0, document.duration.0
            )));
        }
        let max_width = args.max_width.unwrap_or(512).clamp(32, 1_024);
        let bins = usize::from(args.bins.unwrap_or(64).clamp(16, 128));
        let image = match self
            .analysis
            .thumbnail_for_document(document, args.timecode, max_width)
        {
            Ok(image) => image,
            Err(error) => return Ok(error_text(error.to_string())),
        };
        let scopes = scope_data(&image, bins);
        Ok(success_structured(
            format!(
                "video scopes at project frame {} from {}x{} compositor output\n{}",
                args.timecode.0, image.width, image.height, scopes
            ),
            scopes,
        ))
    }

    pub(super) fn video_scopes_v2(
        &self,
        args: &VideoScopesV2Args,
    ) -> Result<CallToolResult, McpError> {
        let (revision, document) = self.snapshot()?;
        match video_scopes_v2(&document, revision, self.analysis.as_ref(), args) {
            Ok(value) => Ok(success_structured(
                format!(
                    "CC2 scopes at timeline revision {revision}: {} sample(s), stage={}",
                    value["temporal"]["sample_count"], value["stage"]
                ),
                value,
            )),
            Err(error) => Ok(color_scope_error_result("get_video_scopes_v2", &error)),
        }
    }

    /// CC6 §7: measure the working stage and publish evidence, nothing else.
    ///
    /// The revision is read once and republished; nothing on this path can
    /// advance it, and the response says so at the top level.
    pub(super) fn color_qc(&self, args: &ColorQcArgs) -> Result<CallToolResult, McpError> {
        let (revision, document) = self.snapshot()?;
        match get_color_qc(&document, revision, self.analysis.as_ref(), args) {
            Ok(value) => Ok(success_structured(
                format!(
                    // Read as the typed values they are: `Value`'s own Display
                    // would quote the stage and is only correct by accident.
                    "evidence-only CC6 colour QC at timeline revision {revision}, stage={}, project frame {}; no operation was applied",
                    value["stage"]
                        .as_str()
                        .unwrap_or(kinewright_core::WORKING_PROOF_STAGE),
                    value["report"]["project_frame"]
                        .as_i64()
                        .unwrap_or_default()
                ),
                value,
            )),
            Err(error) => Ok(color_scope_error_result("get_color_qc", &error)),
        }
    }
}

pub(super) fn color_scope_error_result(tool: &str, error: &ScopeError) -> CallToolResult {
    error_structured(
        format!("{tool} rejected: {error}"),
        serde_json::json!({
            "code": error.code(),
            "message": error.to_string(),
            "details": error.details(),
            "evidence_only": true,
            "applied": false,
        }),
    )
}

pub(super) fn scope_data(image: &kinewright_core::RgbaImage, bins: usize) -> serde_json::Value {
    const WAVEFORM_COLUMNS: usize = 64;
    let bins = bins.clamp(1, 256);
    let mut red = vec![0_u64; bins];
    let mut green = vec![0_u64; bins];
    let mut blue = vec![0_u64; bins];
    let mut luma = vec![0_u64; bins];
    let mut channel_sums = [0_u64; 4];
    let mut clipped_black = 0_u64;
    let mut clipped_white = 0_u64;
    let mut waveform_min = [u8::MAX; WAVEFORM_COLUMNS];
    let mut waveform_max = [0_u8; WAVEFORM_COLUMNS];
    let mut waveform_sum = [0_u64; WAVEFORM_COLUMNS];
    let mut waveform_count = [0_u64; WAVEFORM_COLUMNS];
    let width = usize::try_from(image.width).unwrap_or(1).max(1);
    let mut pixel_count = 0_u64;

    for (pixel_index, pixel) in image.pixels.as_chunks::<4>().0.iter().enumerate() {
        let [red_value, green_value, blue_value, alpha] = [pixel[0], pixel[1], pixel[2], pixel[3]];
        if alpha == 0 {
            continue;
        }
        let luma_value = u8::try_from(
            (54_u32 * u32::from(red_value)
                + 183_u32 * u32::from(green_value)
                + 19_u32 * u32::from(blue_value))
                / 256,
        )
        .unwrap_or(u8::MAX);
        let bucket = |value: u8| usize::from(value) * bins / 256;
        red[bucket(red_value)] += 1;
        green[bucket(green_value)] += 1;
        blue[bucket(blue_value)] += 1;
        luma[bucket(luma_value)] += 1;
        channel_sums[0] += u64::from(red_value);
        channel_sums[1] += u64::from(green_value);
        channel_sums[2] += u64::from(blue_value);
        channel_sums[3] += u64::from(luma_value);
        clipped_black += u64::from(luma_value <= 1);
        clipped_white += u64::from(luma_value >= 254);
        pixel_count += 1;

        let pixel_x = pixel_index % width;
        let column = (pixel_x * WAVEFORM_COLUMNS / width).min(WAVEFORM_COLUMNS - 1);
        waveform_min[column] = waveform_min[column].min(luma_value);
        waveform_max[column] = waveform_max[column].max(luma_value);
        waveform_sum[column] += u64::from(luma_value);
        waveform_count[column] += 1;
    }

    let mean_milli = channel_sums.map(|sum| {
        sum.saturating_mul(1_000)
            .checked_div(pixel_count)
            .unwrap_or(0)
    });
    let basis_points = |count: u64| {
        count
            .saturating_mul(10_000)
            .checked_div(pixel_count)
            .unwrap_or(0)
    };
    let waveform = (0..WAVEFORM_COLUMNS)
        .map(|column| {
            let count = waveform_count[column];
            serde_json::json!({
                "column": column,
                "minimum": if count == 0 { 0 } else { waveform_min[column] },
                "maximum": if count == 0 { 0 } else { waveform_max[column] },
                "mean_milli": waveform_sum[column].saturating_mul(1_000).checked_div(count).unwrap_or(0),
                "samples": count,
            })
        })
        .collect::<Vec<_>>();

    serde_json::json!({
        "resolution": [image.width, image.height],
        "visible_pixel_count": pixel_count,
        "histogram_bins": bins,
        "histograms": {
            "red": red,
            "green": green,
            "blue": blue,
            "luma": luma,
        },
        "mean_milli": {
            "red": mean_milli[0],
            "green": mean_milli[1],
            "blue": mean_milli[2],
            "luma": mean_milli[3],
        },
        "clipping_basis_points": {
            "black": basis_points(clipped_black),
            "white": basis_points(clipped_white),
        },
        "waveform_luma": waveform,
    })
}
