//! Media pool status, cache inventory, import, and relink tools.

// A continuation of the parent module's `impl KinewrightMcp`: it reads the
// parent's scope exactly as the code did before the split.
#[allow(clippy::wildcard_imports)]
use super::*;

impl KinewrightMcp {
    pub(super) fn media_status(&self) -> Result<CallToolResult, McpError> {
        let (revision, document) = self.snapshot()?;
        let assets = document
            .media_pool
            .iter()
            .map(|asset| {
                serde_json::json!({
                    "asset_id": asset.id.0,
                    "path": asset.path,
                    "persisted_fingerprint": asset.source_fingerprint,
                    "availability": self.analysis.media_availability(asset),
                    "analysis_jobs": self.analysis.analysis_jobs(asset),
                })
            })
            .collect::<Vec<_>>();
        let value = serde_json::json!({
            "timeline_revision": revision.0,
            "preview": {
                "mode": "in_memory",
                "max_width": MEDIA_PREVIEW_MAX_WIDTH,
                "persistent": false,
                "generated_proxy_supported": false,
            },
            "assets": assets,
        });
        Ok(success_structured(
            format!(
                "media status at timeline revision {}: {} asset(s), preview mode=in_memory max_width={} persistent=false generated_proxy_supported=false",
                revision,
                value["assets"].as_array().map_or(0, Vec::len),
                MEDIA_PREVIEW_MAX_WIDTH,
            ),
            value,
        ))
    }

    pub(super) fn cache_status(&self) -> Result<CallToolResult, McpError> {
        let inventory: MediaCacheInventory = self.analysis.cache_inventory();
        let value = serde_json::to_value(&inventory)
            .map_err(|error| McpError::internal_error(error.to_string(), None))?;
        Ok(success_structured(
            format!("media cache status: {} family(s)", inventory.families.len()),
            value,
        ))
    }

    pub(super) fn clear_media_cache(
        &self,
        family: MediaCacheFamily,
    ) -> Result<CallToolResult, McpError> {
        match self.analysis.clear_cache(family) {
            Ok(result) if !result.supported => {
                let generated_proxy = family == MediaCacheFamily::GeneratedProxy;
                let code = if generated_proxy {
                    "unsupported_generated_proxy"
                } else {
                    "unsupported_cache_family"
                };
                Ok(error_structured(
                    format!("cannot clear {family:?} media cache: {code}"),
                    serde_json::json!({
                        "family": family,
                        "supported": false,
                        "code": code,
                        "message": result.note,
                    }),
                ))
            }
            Ok(result) => {
                let value = serde_json::to_value(&result)
                    .map_err(|error| McpError::internal_error(error.to_string(), None))?;
                Ok(success_structured(
                    format!(
                        "cleared {:?} media cache: {} file(s), {} byte(s)",
                        family, result.removed_file_count, result.removed_bytes
                    ),
                    value,
                ))
            }
            Err(error) if error == kinewright_core::MediaError::NotImplemented => {
                let generated_proxy = family == MediaCacheFamily::GeneratedProxy;
                let code = if generated_proxy {
                    "unsupported_generated_proxy"
                } else {
                    "unsupported_cache_family"
                };
                Ok(error_structured(
                    format!("cannot clear {family:?} media cache: {code}"),
                    serde_json::json!({
                        "family": family,
                        "supported": false,
                        "code": code,
                        "message": error.to_string(),
                    }),
                ))
            }
            Err(error) => Ok(error_structured(
                format!("could not clear {family:?} media cache: {error}"),
                serde_json::json!({
                    "family": family,
                    "supported": true,
                    "code": "cache_clear_failed",
                    "message": error.to_string(),
                }),
            )),
        }
    }

    pub(super) fn relink_media(&self, args: &RelinkMediaArgs) -> Result<CallToolResult, McpError> {
        let (actual_revision, document) = self.snapshot()?;
        if args.expected_revision != actual_revision {
            return Ok(revision_conflict_text(
                args.expected_revision,
                actual_revision,
            ));
        }
        let Some(current) = document.asset(args.asset_id) else {
            return Ok(error_text(format!(
                "asset {} does not exist",
                args.asset_id
            )));
        };

        // Probe and hash the replacement before constructing the Core
        // operation. Core remains filesystem-free and receives only this
        // typed candidate; all mismatches therefore remain atomic Core
        // rejections after this read-only preflight.
        let probed = match self.analysis.probe(&args.path) {
            Ok(asset) => asset,
            Err(error) => return Ok(error_text(error.to_string())),
        };
        let candidate = RelinkCandidate {
            path: args.path.clone(),
            fingerprint: probed.source_fingerprint,
            kind: probed.kind,
            fps: probed.fps,
            duration: probed.duration,
            resolution: probed.resolution,
        };
        let operation = Operation::RelinkAsset {
            asset: args.asset_id,
            candidate,
            allow_unverified_source: args.allow_unverified_source,
        };
        let result = self.apply_operation("relink_media", args.expected_revision, operation);
        if result.is_error != Some(true) {
            // Refresh content-addressed analysis for the replacement path.
            // The operation itself remains the one Core history entry.
            if let Ok((_, updated)) = self.snapshot()
                && let Some(updated_asset) = updated.asset(current.id)
            {
                self.request_asset_analysis(updated_asset.clone());
            }
        }
        Ok(result)
    }

    pub(super) fn import_media(
        &self,
        expected_revision: TimelineRevision,
        path: &Path,
    ) -> CallToolResult {
        let asset = match self.analysis.probe(path) {
            Ok(asset) => asset,
            Err(error) => return error_text(error.to_string()),
        };
        self.apply_operation(
            "import_media",
            expected_revision,
            Operation::AddAsset { asset },
        )
    }
}
