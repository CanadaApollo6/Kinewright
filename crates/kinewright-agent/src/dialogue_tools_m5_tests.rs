#[test]
fn dq_m5_check_accepts_renderer_proven_clocks_and_enabled_origins() {
    let rate = |n, d| Rational::new(n, d).unwrap();
    for (name, source, project, start, end, enabled, ranged) in [
        ("equal", rate(30, 1), rate(30, 1), 0, 120, false, false),
        ("25_in_24", rate(25, 1), rate(24, 1), 12, 112, false, false),
        (
            "ntsc_in_30",
            rate(30000, 1001),
            rate(30, 1),
            0,
            120,
            false,
            false,
        ),
        ("60_in_30", rate(60, 1), rate(30, 1), 0, 120, false, false),
        (
            "stored_source_padding",
            rate(60, 1),
            rate(30, 1),
            0,
            1,
            false,
            false,
        ),
        (
            "source_offset",
            rate(30, 1),
            rate(30, 1),
            12,
            132,
            false,
            false,
        ),
        (
            "enabled_inside_clip",
            rate(25, 1),
            rate(24, 1),
            12,
            112,
            true,
            false,
        ),
        (
            "seek_inside_clip",
            rate(30, 1),
            rate(30, 1),
            0,
            120,
            false,
            true,
        ),
    ] {
        let v = value(vec![], 6_000_000, |_| level(true));
        let mut doc = document(source, project, start, end);
        if enabled {
            doc.tracks[0].clips[0].enabled_curve = Some(kinewright_core::AutomationCurve {
                keyframes: [(0, 0), (23, 1)]
                    .into_iter()
                    .map(|(at, value)| kinewright_core::Keyframe {
                        at: TimeCode(at),
                        value,
                        interpolation: kinewright_core::KeyframeInterpolation::Hold,
                        tangent_in: 0,
                        tangent_out: 0,
                    })
                    .collect(),
            });
        }
        let s = service(doc, v);
        let arguments = if ranged {
            json!({"track_id":1,"detail":"all","range":[23,72]})
        } else {
            json!({"track_id":1,"detail":"all"})
        };
        let result = invoke(&s, "check_dialogue_cut", arguments);
        ok(&result);
        let checked = &content(&result)["check"];
        assert!(
            checked["not_checked"].is_null(),
            "M5_CHECK_CLOCK {name}: {result:?}"
        );
        assert_eq!(checked["segments"].as_array().unwrap().len(), 1, "{name}");
        assert_eq!(
            checked["segments"][0]["start_frame"],
            if enabled { 23 } else { 0 },
            "{name}"
        );
        assert!(checked["recordings"].get("1").is_some(), "{name}");
    }
}

#[test]
fn dq_m5_catalog_and_dispatch_expose_measurements_without_pacing() {
    let (s, _) = fixture();
    let catalog = KinewrightMcp::capability_tools().unwrap();
    let metrics = ToolSurfaceMetrics::measure(&catalog);
    let descriptors: Vec<_> = catalog.iter().filter(|tool| matches!(tool.name.as_ref(), "get_timeline_transcript" | "cut_dialogue" | "check_dialogue_cut")).map(|tool| json!({"name":tool.name,"bytes":serde_json::to_vec(tool).unwrap().len(),"schema_bytes":serde_json::to_vec(&tool.input_schema).unwrap().len(),"description_bytes":tool.description.as_ref().map_or(0, |s|s.len())})).collect();
    println!(
        "M5_SURFACE {}",
        json!({"count":metrics.tool_count,"bytes":metrics.serialized_bytes,"schema_bytes":metrics.input_schema_bytes,"description_bytes":metrics.description_bytes,"descriptors":descriptors})
    );

    for name in dq::TOOL_NAMES {
        assert!(catalog.iter().any(|tool| tool.name == *name));
        assert!(crate::runtime::is_invocable_capability(name));
    }
    assert!(!crate::runtime::is_invocable_capability(
        "get_dialogue_pacing"
    ));
    let transcript = catalog
        .iter()
        .find(|tool| tool.name == "get_timeline_transcript")
        .unwrap();
    assert!(
        transcript.input_schema["properties"]
            .get("word_range")
            .is_some()
    );
    let opened = invoke(&s, "get_capability", json!({"name":"get_dialogue_pacing"}));
    assert_eq!(opened.is_error, Some(true));
    let unknown = invoke(&s, "get_dialogue_pacing", json!({}));
    assert_eq!(unknown.is_error, Some(true));
    let checked = invoke(&s, "check_dialogue_cut", json!({"track_id":1}));
    ok(&checked);
}

#[test]
fn dq_m5_native_manifests_expose_scoped_dialogue_while_app_catalog_serves_it() {
    use crate::orchestration::orchestration_records::Role;
    let dialogue = [
        "get_timeline_transcript",
        "cut_dialogue",
        "check_dialogue_cut",
    ];
    for role in [
        Role::Director,
        Role::Editor,
        Role::Motion,
        Role::Audio,
        Role::Colour,
        Role::Reviewer,
    ] {
        let tools = task_role_tools(role).unwrap();
        for name in dialogue {
            let expected = name != "cut_dialogue" || !matches!(role, Role::Director | Role::Reviewer);
            assert_eq!(
                crate::writer_host::task_mcp::manifest::names(role).contains(&name), expected,
                "M5_NATIVE_MANIFEST {role:?} {name}"
            );
            assert_eq!(
                tools.iter().any(|tool| tool.name == name), expected,
                "M5_NATIVE_SCHEMA {role:?} {name}"
            );
        }
    }
    for name in dialogue {
        assert!(crate::orchestration::task_tool_names::TASK_TOOL_NAMES.contains(&name), "NATIVE_PROVIDER_ALLOWLIST {name}");
        assert_eq!(crate::writer_host::task_mcp::product_tool_name(name, None), name, "NATIVE_DIALOGUE_ACCOUNTING {name}");
    }
    let (mut app, _) = fixture();
    *app.dialogue_measurements.write().unwrap() = None;
    assert!(dialogue.iter().all(|name| {
        app.dialogue_catalog()
            .unwrap()
            .iter()
            .any(|tool| tool.name == *name)
    }));
    app.host_scoped = true;
    assert!(dialogue.iter().all(|name| {
        !app.dialogue_catalog()
            .unwrap()
            .iter()
            .any(|tool| tool.name == *name)
    }));
}
