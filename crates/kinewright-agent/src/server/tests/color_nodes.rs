//! CC3 colour context and node planner tests.

use super::*;

#[test]
fn m34_agent_tools_expose_creator_plans_tracking_and_delivery_jobs() {
    let tools = KinewrightMcp::tools().unwrap();
    let names = tools
        .iter()
        .map(|tool| tool.name.as_ref())
        .collect::<BTreeSet<_>>();
    for name in [
        "plan_beat_pacing",
        "plan_music_fit",
        "plan_speaker_multicam",
        "track_reframe_subject",
        "get_color_context",
        "render_color_proof",
        "get_delivery_profiles",
        "get_delivery_conformance",
        "queue_export",
        "get_export_jobs",
        "cancel_export",
    ] {
        assert!(names.contains(name), "missing M34 tool {name}");
    }

    let mut registered = crate::schema::capability_tool_names().unwrap();
    registered.sort_unstable();
    let mut served = tools
        .into_iter()
        .map(|tool| tool.name.into_owned())
        .collect::<Vec<_>>();
    served.sort_unstable();
    assert_eq!(registered, served);

    let (core, playback, analysis) = fixture();
    let service = KinewrightMcp::new(core, playback, analysis, ConfirmationBroker::default());
    let profiles = service.delivery_profiles().unwrap();
    let profiles = profiles.structured_content.unwrap();
    let source_master = profiles["profiles"]
        .as_array()
        .unwrap()
        .iter()
        .find(|profile| profile["id"] == "source_master")
        .unwrap();
    assert_eq!(source_master["delivery_color"]["primaries"], "bt709");
    assert_eq!(source_master["delivery_color"]["matrix"], "bt709");
    assert_eq!(source_master["delivery_color"]["range"], "limited");

    let conformance = service
        .delivery_conformance(&DeliveryConformanceArgs {
            profile: DeliveryProfile::SourceMaster,
            focus_x_percent: 50,
            focus_y_percent: 50,
            delivery_bit_depth: DeliveryEncodeDepth::Eight,
        })
        .unwrap();
    let conformance = conformance.structured_content.unwrap();
    assert_eq!(
        conformance["delivery_color"],
        source_master["delivery_color"]
    );
    assert_eq!(
        conformance["report"]["delivery_color"],
        source_master["delivery_color"]
    );
}

#[test]
fn color_context_is_a_read_only_internal_capability_with_revisioned_source_data() {
    let registry = KinewrightMcp::capability_tools().unwrap();
    assert!(registry.iter().any(|tool| tool.name == "get_color_context"));
    assert!(
        registry
            .iter()
            .any(|tool| tool.name == "plan_primary_correction")
    );
    let served = KinewrightMcp::served_tools().unwrap();
    assert!(served.iter().all(|tool| tool.name != "get_color_context"));

    let (core, playback, analysis) = fixture();
    let service = KinewrightMcp::new(core, playback, analysis, ConfirmationBroker::default());
    let before = service.snapshot().unwrap();
    let result = service
        .call_blocking(CallToolRequestParams::new("get_color_context"))
        .unwrap();
    assert_eq!(result.is_error, Some(false));
    let value = result.structured_content.unwrap();
    assert_eq!(value["timeline_revision"], 0);
    assert_eq!(value["color_context"]["working"]["primaries"], "bt709");
    assert_eq!(value["color_context"]["working"]["matrix"], "rgb");
    assert_eq!(
        value["color_context"]["working"]["confidence_basis_points"],
        10_000
    );
    assert_eq!(
        value["color_context"]["working"]["provenance"],
        "application_default"
    );
    assert_eq!(value["color_context"]["monitoring"]["range"], "full");
    assert_eq!(value["color_context"]["delivery"]["range"], "limited");
    assert_eq!(value["assets"].as_array().unwrap().len(), 1);
    assert_eq!(value["assets"][0]["id"], 1);
    assert_eq!(
        value["assets"][0]["source"]["raw_description"]["primaries"],
        "unknown"
    );
    assert_eq!(
        value["assets"][0]["source"]["raw_description"]["confidence_basis_points"],
        0
    );
    assert_eq!(
        value["assets"][0]["source"]["raw_description"]["provenance"],
        "unknown"
    );
    assert_eq!(
        value["assets"][0]["source"]["status"]["status"],
        "needs_color_override"
    );
    assert_eq!(value["assets"][0]["managed_blocking"], true);
    assert_eq!(service.snapshot().unwrap(), before);

    let invoked = service
        .call_blocking(
            CallToolRequestParams::new("invoke_capability").with_arguments(
                json!({"name": "get_color_context", "arguments": {}})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .unwrap();
    assert_eq!(invoked.is_error, Some(false));
    assert_eq!(invoked.structured_content.unwrap()["timeline_revision"], 0);
}

/// CC3 §8: both planners join the internal registry as read-only planner
/// capabilities, stay off the seven-tool served surface, and return exact
/// unapplied operations through the compact dispatcher.
#[test]
#[allow(clippy::too_many_lines)]
fn cc3_planners_are_read_only_internal_planner_capabilities() {
    let registry = KinewrightMcp::capability_tools().unwrap();
    let served = KinewrightMcp::served_tools().unwrap();
    let catalog = capabilities(&registry);
    for name in ["plan_color_wheels", "plan_color_curves"] {
        let tool = registry
            .iter()
            .find(|tool| tool.name == name)
            .unwrap_or_else(|| panic!("{name} must be an internal capability"));
        assert_eq!(
            tool.annotations
                .as_ref()
                .and_then(|annotations| annotations.read_only_hint),
            Some(true),
            "{name} is evidence-only"
        );
        assert!(
            served.iter().all(|tool| tool.name != name),
            "{name} must not enlarge the served surface"
        );
        let capability = catalog
            .iter()
            .find(|capability| capability.name == name)
            .unwrap();
        assert_eq!(capability.kind, CapabilityKind::Planner);
        assert!(is_invocable_capability(name));
    }

    let (seed_core, playback, analysis) = fixture();
    let Event::QueryResult(QueryResult::Document(seed)) =
        seed_core.request(Command::Query(Query::Document)).unwrap()
    else {
        panic!("expected fixture document");
    };
    let mut document = (*seed).clone();
    document.media_pool[0].color_description = ColorDescription {
        primaries: ColorPrimaries::Bt709,
        transfer: ColorTransfer::Bt709,
        matrix: ColorMatrix::Bt709,
        range: ColorRange::Limited,
        white_point: ColorWhitePoint::D65,
        bit_depth: ColorBitDepth::Eight,
        confidence_basis_points: 10_000,
        provenance: ColorProvenance::StreamMetadata,
    };
    let core = Core::spawn(document).unwrap();
    let service = KinewrightMcp::new(core, playback, analysis, ConfirmationBroker::default());
    let before = service.snapshot().unwrap();

    let invoke = |name: &str, arguments: serde_json::Value| {
        service
            .call_blocking(
                CallToolRequestParams::new("invoke_capability").with_arguments(
                    json!({"name": name, "arguments": arguments})
                        .as_object()
                        .unwrap()
                        .clone(),
                ),
            )
            .unwrap()
    };

    let wheels = invoke(
        "plan_color_wheels",
        json!({
            "expected_revision": 0,
            "clip_id": 1,
            "parameters": {"gain_red_thousandths": 1_200}
        }),
    );
    assert_eq!(wheels.is_error, Some(false));
    let wheels = wheels.structured_content.unwrap();
    assert_eq!(wheels["applied"], false);
    assert_eq!(wheels["evidence_only"], true);
    assert_eq!(wheels["kind"], "color_wheels");
    assert_eq!(wheels["created_new_node"], true);
    assert_eq!(wheels["existing_color_node_count"], 0);
    assert_eq!(wheels["resolved_parameters"]["gain_red_thousandths"], 1_200);
    assert_eq!(
        wheels["resolved_parameters"]["gain_blue_thousandths"],
        1_000
    );
    assert_eq!(wheels["operations"].as_array().unwrap().len(), 1);
    assert_eq!(wheels["after"]["color_node_count"], 1);

    let curves = invoke(
        "plan_color_curves",
        json!({
            "expected_revision": 0,
            "clip_id": 1,
            "curves": {"master": [[0, 0], [5_000, 6_000], [10_000, 10_000]]}
        }),
    );
    assert_eq!(curves.is_error, Some(false));
    let curves = curves.structured_content.unwrap();
    assert_eq!(curves["applied"], false);
    assert_eq!(curves["kind"], "color_curves");
    assert_eq!(
        curves["resolved_curves"]["master"],
        json!([[0, 0], [5_000, 6_000], [10_000, 10_000]])
    );
    assert_eq!(curves["requested_parameters"]["master_point_count"], 3);
    assert_eq!(curves["requested_parameters"]["master_y1"], 6_000);

    // A rejected request keeps the CC1/CC2 field/observed/allowed shape.
    let rejected = invoke(
        "plan_color_curves",
        json!({
            "expected_revision": 0,
            "clip_id": 1,
            "curves": {"red": [[0, 0], [5_000, 0], [5_000, 9_000]]}
        }),
    );
    assert_eq!(rejected.is_error, Some(true));
    let rejected = rejected.structured_content.unwrap();
    assert_eq!(rejected["code"], "invalid_curve_points");
    assert_eq!(rejected["details"]["field"], "curves.red[2].x");
    assert_eq!(rejected["details"]["observed"], 5_000);
    assert_eq!(rejected["applied"], false);

    assert_eq!(
        service.snapshot().unwrap(),
        before,
        "neither planner may touch the live document"
    );
}
