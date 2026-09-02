//! CC3 managed colour-node context and evidence-only node planners.

use super::color_qc::color_scope_error_result;
// A continuation of the parent module's `impl KinewrightMcp`: it reads the
// parent's scope exactly as the code did before the split.
#[allow(clippy::wildcard_imports)]
use super::*;

impl KinewrightMcp {
    pub(super) fn color_context(
        &self,
        args: &ColorContextArgs,
    ) -> Result<CallToolResult, McpError> {
        if let Some(conflict) = raw_only_conflict(args) {
            return Ok(error_structured(
                conflict["message"].as_str().unwrap_or_default().to_owned(),
                conflict,
            ));
        }
        let (revision, document) = self.snapshot()?;
        let looks = self.look_context(&document);
        let value = if args.raw_only {
            // `raw_only_conflict` above already refused `raw_only` combined with
            // an explicit assumption, so there is no assumption left to forward.
            color_context_value_with_options(
                revision,
                &document,
                None,
                &args.asset_ids,
                true,
                &looks,
            )
        } else {
            color_context_value_with_assumptions(
                revision,
                &document,
                args.profile_assumption,
                &args.asset_ids,
                &looks,
            )
        };
        Ok(success_structured(
            format!(
                "timeline_revision={} assets={} working={} monitoring={} delivery={}\n{}",
                revision,
                value["assets"].as_array().map_or(0, Vec::len),
                value["color_context"]["working"],
                value["color_context"]["monitoring"],
                value["color_context"]["delivery"],
                value,
            ),
            value,
        ))
    }

    pub(super) fn primary_correction_plan(
        &self,
        args: &PrimaryCorrectionPlanArgs,
    ) -> Result<CallToolResult, McpError> {
        let (actual_revision, document) = self.snapshot()?;
        let plan = match plan_primary_correction(&document, actual_revision, args) {
            Ok(plan) => plan,
            Err(PrimaryPlanError::RevisionConflict { expected, actual }) => {
                return Ok(revision_conflict_text(expected, actual));
            }
            Err(error) => {
                return Ok(error_structured(
                    format!("primary correction plan rejected: {error}"),
                    serde_json::json!({
                        "code": error.code(),
                        "message": error.to_string(),
                        "details": error.details(),
                        "evidence_only": true,
                        "applied": false,
                    }),
                ));
            }
        };
        let operations = serde_json::to_value(&plan.operations)
            .map_err(|error| McpError::internal_error(error.to_string(), None))?;
        let value = serde_json::json!({
            "timeline_revision": plan.expected_revision.0,
            "clip_id": plan.clip_id.0,
            "effect_id": plan.effect_id.0,
            // Null when the proposal changes nothing and would have had to
            // create the node: no operation allocates that id, so publishing it
            // would name a node that does not exist and may later be reused.
            "target_effect_id": plan.target_effect_id().map(|effect| effect.0),
            "created_new_node": plan.created_new_node,
            "existing_primary_node_count": plan.existing_primary_node_count,
            "no_change": plan.no_change,
            "warnings": plan.warnings,
            "source_profile": plan.source_profile.id(),
            "profile_assumption": plan.profile_assumption,
            "evidence_only": true,
            "applied": false,
            "before": {
                "primary_node_count": plan.existing_primary_node_count,
            },
            "after": {
                "primary_node_count": plan.existing_primary_node_count
                    + usize::from(plan.created_new_node),
            },
            "requested_parameters": plan.requested_parameters,
            "resolved_parameters": plan.resolved_parameters,
            "operations": operations,
            "next": "Review these exact operations; submit them through prepare_edit_plan at the same revision if the edit is requested.",
        });
        Ok(success_structured(
            format!(
                "prepared evidence-only primary correction for clip {} at revision {}; no operation was applied",
                plan.clip_id, plan.expected_revision
            ),
            value,
        ))
    }

    /// Render one evidence-only managed colour-node proposal (CC3 §8, CC4 §8).
    ///
    /// Every node planner shares this response shape so an agent that learned
    /// `plan_color_wheels` can read a `plan_creative_look` result unchanged.
    pub(super) fn color_node_plan<Plan>(
        &self,
        tool: &str,
        plan: Plan,
    ) -> Result<CallToolResult, McpError>
    where
        Plan: FnOnce(&Document, TimelineRevision) -> Result<ColorNodePlan, ColorNodePlanError>,
    {
        let (actual_revision, document) = self.snapshot()?;
        let plan = match plan(&document, actual_revision) {
            Ok(plan) => plan,
            Err(ColorNodePlanError::RevisionConflict { expected, actual }) => {
                return Ok(revision_conflict_text(expected, actual));
            }
            Err(error) => {
                return Ok(error_structured(
                    format!("{tool} rejected: {error}"),
                    serde_json::json!({
                        "code": error.code(),
                        "message": error.to_string(),
                        "details": error.details(),
                        "evidence_only": true,
                        "applied": false,
                    }),
                ));
            }
        };
        let operations = serde_json::to_value(&plan.operations)
            .map_err(|error| McpError::internal_error(error.to_string(), None))?;
        let value = serde_json::json!({
            "timeline_revision": plan.expected_revision.0,
            "expected_revision": plan.expected_revision.0,
            "clip_id": plan.clip_id.0,
            "kind": plan.kind.effect_name(),
            "effect_id": plan.effect_id.0,
            // Null when the proposal changes nothing and would have had to
            // create the node: no operation allocates that id, so publishing it
            // would name a node that does not exist and may later be reused.
            "target_effect_id": plan.target_effect_id().map(|effect| effect.0),
            "created_new_node": plan.created_new_node,
            "targets_existing_node": plan.targets_existing_node,
            "existing_color_node_count": plan.existing_color_node_count,
            "existing_nodes_of_kind": plan.existing_nodes_of_kind,
            "color_node_limit_per_layer": kinewright_core::COLOR_NODE_LIMIT_PER_LAYER,
            "no_change": plan.no_change,
            "warnings": plan.warnings,
            "assumptions": plan.assumptions,
            "source_profile": plan.source_profile.id(),
            "profile_assumption": plan.profile_assumption,
            "evidence_only": true,
            "applied": false,
            "before": {
                "color_node_count": plan.existing_color_node_count,
                "nodes_of_kind": plan.existing_nodes_of_kind,
            },
            "after": {
                "color_node_count": plan.existing_color_node_count
                    + usize::from(plan.created_new_node),
                "nodes_of_kind": plan.existing_nodes_of_kind
                    + usize::from(plan.created_new_node),
            },
            "requested_parameters": plan.requested_parameters,
            "resolved_parameters": plan.resolved_parameters,
            "requested_curves": plan.requested_curves,
            "resolved_curves": plan.resolved_curves,
            // CC4 §8: the LUT planners publish the exact index their
            // InsertEffect uses, so an ordering rejection is unreachable
            // through the ordinary path, plus the bound asset's identity.
            "insert_index": plan.insert_index,
            "lut_asset": plan.lut_asset,
            "lut_node_limit_per_layer": kinewright_core::LUT_NODE_LIMIT_PER_LAYER,
            "role": plan.kind.role(),
            "color_stage": plan.kind.stage().as_str(),
            "operations": operations,
            "next": "Review these exact operations; submit them through prepare_edit_plan at the same revision if the edit is requested.",
        });
        // CC5 §7: inserted rather than written into the literal above, so a
        // CC3/CC4 plan response is byte-unchanged — the keys are absent, not
        // null, when the planner does not touch a matte.
        let mut value = value;
        if let Some(object) = value.as_object_mut() {
            for (key, field) in [
                ("matte", plan.matte),
                ("predicted_coverage", plan.predicted_coverage),
                ("sample_roi_evidence", plan.sample_evidence),
            ] {
                if let Some(field) = field {
                    object.insert(key.to_owned(), field);
                }
            }
        }
        Ok(success_structured(
            format!(
                "prepared evidence-only {} proposal for clip {} at revision {}; no operation was applied",
                plan.kind.effect_name(),
                plan.clip_id,
                plan.expected_revision
            ),
            value,
        ))
    }

    pub(super) fn analyze_color_shot(
        &self,
        args: &AnalyzeColorShotArgs,
    ) -> Result<CallToolResult, McpError> {
        let (revision, document) = self.snapshot()?;
        match analyze_color_shot(&document, revision, self.analysis.as_ref(), args) {
            Ok(value) => Ok(success_structured(
                format!(
                    "evidence-only CC2 color analysis for clip {} at timeline revision {}; no operation was applied",
                    args.clip_id, revision
                ),
                value,
            )),
            Err(error) => Ok(color_scope_error_result("analyze_color_shot", &error)),
        }
    }

    pub(super) fn plan_shot_match(
        &self,
        args: &PlanShotMatchArgs,
    ) -> Result<CallToolResult, McpError> {
        let (revision, document) = self.snapshot()?;
        match plan_shot_match(&document, revision, self.analysis.as_ref(), args) {
            Ok(value) => Ok(success_structured(
                format!(
                    "evidence-only CC2 shot match at timeline revision {revision}; no operation was applied",
                ),
                value,
            )),
            Err(error) => Ok(color_scope_error_result("plan_shot_match", &error)),
        }
    }
}
