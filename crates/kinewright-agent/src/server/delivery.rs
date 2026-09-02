//! Delivery variants, profiles, conformance, editorial readiness, and the export queue.

// A continuation of the parent module's `impl KinewrightMcp`: it reads the
// parent's scope exactly as the code did before the split.
#[allow(clippy::wildcard_imports)]
use super::*;

impl KinewrightMcp {
    pub(super) fn delivery_variants() -> CallToolResult {
        let variants = DeliveryAspect::ALL.map(|aspect| {
            let (width, height) = aspect.resolution();
            serde_json::json!({
                "aspect": aspect,
                "label": aspect.as_str(),
                "resolution": {"width": width, "height": height},
                "framing": "deterministic cover crop with explicit focal point",
            })
        });
        success_text(
            serde_json::to_string_pretty(&variants)
                .unwrap_or_else(|error| format!("could not serialize variants: {error}")),
        )
    }

    pub(super) fn delivery_profiles(&self) -> Result<CallToolResult, McpError> {
        let (revision, document) = self.snapshot()?;
        let profiles = DeliveryProfile::ALL.map(|profile| {
            let settings = profile.export_settings(
                &document,
                DeliveryEncodeDepth::Eight,
                ExportCancellation::default(),
            );
            serde_json::json!({
                "id": profile.as_str(),
                "container": profile.container_extension(),
                "aspect": profile.aspect(),
                "resolution": {
                    "width": settings.resolution.0,
                    "height": settings.resolution.1,
                },
                "video_codec": settings.video_codec,
                "audio_codec": settings.audio_codec,
                "video_bitrate": settings.video_bitrate,
                "audio_bitrate": settings.audio_bitrate,
                "fps": {
                    "numerator": settings.fps.numerator(),
                    "denominator": settings.fps.denominator(),
                },
                "delivery_color": settings.delivery_color,
            })
        });
        let structured = serde_json::json!({
            "timeline_revision": revision,
            "profiles": profiles,
        });
        Ok(success_structured(
            serde_json::to_string_pretty(&structured)
                .map_err(|error| McpError::internal_error(error.to_string(), None))?,
            structured,
        ))
    }

    pub(super) fn delivery_conformance(
        &self,
        args: &DeliveryConformanceArgs,
    ) -> Result<CallToolResult, McpError> {
        let (revision, document) = self.snapshot()?;
        let report = match delivery_conformance(
            &document,
            args.profile,
            args.delivery_bit_depth,
            args.focus_x_percent,
            args.focus_y_percent,
        ) {
            Ok(report) => report,
            Err(error) => return Ok(error_text(error.to_string())),
        };
        let structured = serde_json::json!({
            "timeline_revision": revision,
            "export_ready": report.export_ready(),
            "delivery_color": report.delivery_color,
            "report": report,
        });
        Ok(success_structured(
            serde_json::to_string_pretty(&structured)
                .map_err(|error| McpError::internal_error(error.to_string(), None))?,
            structured,
        ))
    }

    pub(super) fn editorial_readiness(
        &self,
        args: &EditorialReadinessArgs,
    ) -> Result<CallToolResult, McpError> {
        let minimum = args.min_silence_source_frames.unwrap_or(TimeCode(20));
        if args.check_silence && minimum <= TimeCode::ZERO {
            return Ok(error_text("min_silence_source_frames must be positive"));
        }
        if args.focus_x_percent > 100 || args.focus_y_percent > 100 {
            return Ok(error_text("delivery focus percentages must be in 0..=100"));
        }
        let (revision, document) = self.snapshot()?;
        let (cuttable, pending_silence_assets) = if args.check_silence {
            self.editorial_silence_evidence(&document, minimum)?
        } else {
            (Vec::new(), Vec::new())
        };
        let qa = qa_document(&document);
        let conformance = match delivery_conformance(
            &document,
            args.profile,
            DeliveryEncodeDepth::Eight,
            args.focus_x_percent,
            args.focus_y_percent,
        ) {
            Ok(report) => report,
            Err(error) => return Ok(error_text(error.to_string())),
        };
        let mut storyboard = self.editorial_readiness_storyboard(revision, &document, args)?;
        if storyboard.is_error == Some(true) {
            return Ok(storyboard);
        }
        let qa_errors = qa.count(kinewright_core::QaSeverity::Error);
        let conformance_errors = conformance
            .issues
            .iter()
            .filter(|issue| issue.severity == kinewright_core::QaSeverity::Error)
            .count();
        let qa_warnings = qa
            .issues
            .iter()
            .filter(|issue| issue.severity == kinewright_core::QaSeverity::Warning)
            .collect::<Vec<_>>();
        let conformance_warnings = conformance
            .issues
            .iter()
            .filter(|issue| issue.severity == kinewright_core::QaSeverity::Warning)
            .collect::<Vec<_>>();
        let ready = pending_silence_assets.is_empty()
            && cuttable.is_empty()
            && qa_errors == 0
            && conformance_errors == 0;
        let cuttable_json = cuttable
            .iter()
            .map(|span| {
                serde_json::json!({
                    "asset_id": span.asset,
                    "track_id": span.track,
                    "clip_id": span.clip,
                    "source_start": span.source_start,
                    "source_end": span.source_end,
                    "project_start": span.project_start,
                    "project_end": span.project_end,
                })
            })
            .collect::<Vec<_>>();
        let structured = serde_json::json!({
            "timeline_revision": revision,
            "ready": ready,
            "silence": {
                "checked": args.check_silence,
                "minimum_source_frames": minimum,
                "cuttable_count": cuttable.len(),
                "spans": cuttable_json,
                "pending_asset_ids": pending_silence_assets,
            },
            "qa": {
                "export_ready": qa.export_ready(),
                "error_count": qa_errors,
                "warning_count": qa_warnings.len(),
                "warning_issues": qa_warnings,
                "blocking_issues": qa.issues.iter().filter(|issue| issue.severity == kinewright_core::QaSeverity::Error).collect::<Vec<_>>(),
            },
            "delivery": {
                "profile": args.profile,
                "delivery_color": conformance.delivery_color,
                "export_ready": conformance.export_ready(),
                "resolution": conformance.resolution,
                "error_count": conformance_errors,
                "warning_count": conformance_warnings.len(),
                "warning_issues": conformance_warnings,
                "blocking_issues": conformance.issues.iter().filter(|issue| issue.severity == kinewright_core::QaSeverity::Error).collect::<Vec<_>>(),
            },
            "storyboard": storyboard.structured_content,
        });
        let summary = serde_json::to_string(&structured)
            .map_err(|error| McpError::internal_error(error.to_string(), None))?;
        let mut result = CallToolResult::success(vec![ContentBlock::text(summary)]);
        result.content.append(&mut storyboard.content);
        result.structured_content = Some(structured);
        Ok(result)
    }

    fn editorial_silence_evidence(
        &self,
        document: &Document,
        minimum: TimeCode,
    ) -> Result<(Vec<TimelineSilenceSpan>, Vec<AssetId>), McpError> {
        let transcripts = document
            .media_pool
            .iter()
            .filter_map(|asset| match self.analysis.transcript_status(asset) {
                TranscriptStatus::Ready(transcript) => Some((asset.id, transcript)),
                _ => None,
            })
            .collect::<BTreeMap<_, _>>();
        let pending = document
            .media_pool
            .iter()
            .filter(|asset| {
                !matches!(
                    self.analysis.silence_status(asset),
                    SilenceStatus::Ready(_) | SilenceStatus::NoAudio
                )
            })
            .map(|asset| asset.id)
            .collect::<Vec<_>>();
        let spans = self
            .analysis
            .timeline_silences(document, None, minimum)
            .map_err(|error| McpError::internal_error(error.to_string(), None))?;
        Ok((
            cuttable_timeline_silences(document, &spans, &transcripts, minimum),
            pending,
        ))
    }

    fn editorial_readiness_storyboard(
        &self,
        revision: TimelineRevision,
        document: &Document,
        args: &EditorialReadinessArgs,
    ) -> Result<CallToolResult, McpError> {
        let document = match document_for_delivery_profile(
            document,
            args.profile,
            args.focus_x_percent,
            args.focus_y_percent,
        ) {
            Ok(document) => Arc::new(document),
            Err(error) => return Ok(error_text(error.to_string())),
        };
        self.storyboard_for_document(
            revision,
            &document,
            StoryboardArgs {
                range: args
                    .storyboard
                    .range
                    .as_ref()
                    .map(|range| TranscriptRangeArgs {
                        start: range.start,
                        end: range.end,
                    }),
                frame_count: args.storyboard.frame_count,
                max_width: args.storyboard.max_width,
            },
            "editorial readiness storyboard",
            Some(serde_json::json!({
                "profile": args.profile,
                "focus_x_percent": args.focus_x_percent,
                "focus_y_percent": args.focus_y_percent,
                "resolution": {"width": document.resolution.0, "height": document.resolution.1},
            })),
        )
    }

    pub(super) fn queue_export(&self, args: QueueExportArgs) -> Result<CallToolResult, McpError> {
        let Some(queue) = &self.export_queue else {
            return Ok(error_text(
                "agent exports are unavailable because this MCP server has no export backend",
            ));
        };
        let (actual_revision, document) = self.snapshot()?;
        if args.expected_revision != actual_revision {
            return Ok(revision_conflict_text(
                args.expected_revision,
                actual_revision,
            ));
        }
        if document
            .media_pool
            .iter()
            .any(|asset| paths_resolve_equal(&args.output_path, &asset.path))
        {
            return Ok(error_text(
                "refusing to export over a source media asset used by this project",
            ));
        }
        if args.overwrite {
            let description = format!(
                "The agent wants permission to replace the regular file at {} if it exists when this queued export starts.",
                args.output_path.display()
            );
            if let Err(reason) = self.confirmations.confirm("queue_export", description) {
                return Ok(error_text(format!(
                    "refused destructive tool queue_export: {reason}"
                )));
            }
        }
        let record = match queue.enqueue(
            &document,
            QueueExportRequest {
                output_path: args.output_path,
                profile: args.profile,
                focus_x_percent: args.focus_x_percent,
                focus_y_percent: args.focus_y_percent,
                overwrite: args.overwrite,
                verify: args.verify,
                delivery_bit_depth: args.delivery_bit_depth,
            },
        ) {
            Ok(record) => record,
            Err(error) => return Ok(export_queue_error_result(error)),
        };
        let structured = serde_json::json!({
            "timeline_revision": actual_revision.0,
            "job": record,
        });
        Ok(success_structured(
            format!(
                "queued export job {} from immutable timeline revision {} to {}",
                record.id.0,
                actual_revision.0,
                record.output_path.display(),
            ),
            structured,
        ))
    }

    pub(super) fn export_jobs(&self) -> CallToolResult {
        let Some(queue) = &self.export_queue else {
            return error_text(
                "agent exports are unavailable because this MCP server has no export backend",
            );
        };
        let jobs = queue.list();
        success_structured(
            format!("{} retained export job(s)", jobs.len()),
            serde_json::json!({"jobs": jobs}),
        )
    }

    pub(super) fn cancel_export(&self, job_id: ExportJobId) -> CallToolResult {
        let Some(queue) = &self.export_queue else {
            return error_text(
                "agent exports are unavailable because this MCP server has no export backend",
            );
        };
        let Some(job) = queue.cancel(job_id) else {
            return error_text(format!("export job {} does not exist", job_id.0));
        };
        success_structured(
            format!("export job {} is now {:?}", job_id.0, job.state),
            serde_json::json!({"job": job}),
        )
    }

    pub(super) fn delivery_variant_storyboard(
        &self,
        args: DeliveryStoryboardArgs,
    ) -> Result<CallToolResult, McpError> {
        let (revision, document) = self.snapshot()?;
        let variant =
            match DeliveryVariant::new(args.aspect, args.focus_x_percent, args.focus_y_percent) {
                Ok(variant) => variant,
                Err(error) => return Ok(error_text(error.to_string())),
            };
        let document = match document_for_delivery_variant(&document, variant) {
            Ok(document) => Arc::new(document),
            Err(error) => return Ok(error_text(error.to_string())),
        };
        let metadata = serde_json::json!({
            "aspect": variant.aspect,
            "aspect_label": variant.aspect.as_str(),
            "focus_x_percent": variant.focus_x_percent,
            "focus_y_percent": variant.focus_y_percent,
            "resolution": {"width": document.resolution.0, "height": document.resolution.1},
        });
        self.storyboard_for_document(
            revision,
            &document,
            args.storyboard,
            "delivery variant storyboard",
            Some(metadata),
        )
    }
}

/// Turn one export-queue refusal into the structured result the agent reads.
///
/// Lifted out of `queue_export` so each CC4 rejection keeps its full typed
/// payload without the tool body growing past what one screen can hold.
fn export_queue_error_result(error: ExportQueueError) -> CallToolResult {
    match error {
        // CC4 §2.3: a blocked look is a typed, recoverable status naming
        // the asset, its recorded hash, the expected store path, and the
        // nodes that would have evaluated it — never a render-time failure.
        ExportQueueError::LutPreflight(report) => error_structured(
            report.summary(),
            serde_json::json!({
                "code": "lut_preflight_blocked",
                "message": report.summary(),
                "details": {
                    "field": "lut_assets",
                    "observed": report.issues,
                    "allowed": "every look a rendered frame could need hashes to its recorded sha256",
                    "recovery_action": "Call list_look_assets, then restore the store file or import a replacement and retarget the node before exporting.",
                    "checked_lut_assets": report.checked_lut_assets,
                },
                "applied": false,
            }),
        ),
        // CC4 §2.2: "there is no project path" and "the path is published but
        // its derived root is refused" are different failures with opposite
        // recoveries. Collapsing them would tell an operator who already saved
        // the project to save it again, which is a loop that cannot terminate.
        error @ ExportQueueError::LutStoreNotSaved => error_structured(
            format!("export blocked: {error}"),
            serde_json::json!({
                "code": "project_not_saved",
                "message": "this timeline carries a LUT node that could evaluate, but the project has no saved path, so its LUT store root cannot be derived (CC4 §2.2)",
                "details": {
                    "field": "project_path",
                    "observed": "project_not_saved",
                    "allowed": "a saved project file path such as <dir>/<stem>.kinewright",
                    "recovery_action": "Save the project first; the LUT store root is <dir>/<stem>.kinewright-assets and is derived from that path.",
                },
                "applied": false,
            }),
        ),
        ExportQueueError::LutStoreRootInvalid { reason } => error_structured(
            format!("export blocked: {reason}"),
            serde_json::json!({
                "code": "lut_store_root_invalid",
                "message": "this timeline carries a LUT node that could evaluate, and the project is saved, but the store root derived from its path is refused (CC4 §2.2)",
                "details": {
                    "field": "lut_store_root",
                    "observed": reason,
                    "allowed": "a writable <dir>/<stem>.kinewright-assets directory that is not a symbolic link",
                    "recovery_action": "Move the project to a directory where its <stem>.kinewright-assets store can be created, or remove the file or symlink occupying that path; the project is already saved, so saving it again cannot help.",
                },
                "applied": false,
            }),
        ),
        error => error_text(error.to_string()),
    }
}

pub(super) fn paths_resolve_equal(left: &Path, right: &Path) -> bool {
    let resolved = |path: &Path| {
        if let Ok(canonical) = path.canonicalize() {
            return Some(canonical);
        }
        let absolute = if path.is_absolute() {
            path.to_owned()
        } else {
            std::env::current_dir().ok()?.join(path)
        };
        let parent = absolute.parent()?.canonicalize().ok()?;
        Some(parent.join(absolute.file_name()?))
    };
    let (Some(left), Some(right)) = (resolved(left), resolved(right)) else {
        return false;
    };
    #[cfg(windows)]
    {
        left.to_string_lossy()
            .eq_ignore_ascii_case(&right.to_string_lossy())
    }
    #[cfg(not(windows))]
    {
        left == right
    }
}
