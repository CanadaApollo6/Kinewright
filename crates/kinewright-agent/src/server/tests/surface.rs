//! Server surface tests: confirmation broker, edit plans, and the compact dispatcher.

use super::*;

struct CountingPlayback(AtomicUsize);

impl Playback for CountingPlayback {
    fn set_document(&self, _doc: Arc<Document>) {
        self.0.fetch_add(1, AtomicOrdering::Relaxed);
    }

    fn request_frame(&self, _t: TimeCode) {}

    fn frames(&self) -> crossbeam_channel::Receiver<(TimeCode, FrameTexture)> {
        crossbeam_channel::never()
    }

    fn events(&self) -> crossbeam_channel::Receiver<MediaEvent> {
        crossbeam_channel::never()
    }

    fn play(&self, _from: TimeCode) {}

    fn pause(&self) {}

    fn seek(&self, _to: TimeCode) {}

    fn position(&self) -> TimeCode {
        TimeCode::ZERO
    }

    fn output_peaks(&self) -> [f32; 2] {
        [0.0; 2]
    }
}

#[test]
fn isolated_handler_edits_and_renders_without_publishing_to_live_playback() {
    let (core, _, analysis) = fixture();
    let playback = Arc::new(CountingPlayback(AtomicUsize::new(0)));
    let service = KinewrightMcp::configured(
        core,
        playback.clone(),
        analysis,
        None,
        ConfirmationBroker::default(),
        false,
        Arc::new(RwLock::new(None)),
    );
    let proof = service.frame_at(TimeCode(1)).unwrap();
    assert_eq!(proof.is_error, Some(false));
    let edit = service.apply_operation(
        "add_marker",
        TimelineRevision::default(),
        Operation::AddMarker {
            marker: Marker {
                id: MarkerId(1),
                position: TimeCode(1),
                label: "Branch".to_owned(),
                color_token: 0,
            },
        },
    );
    assert_eq!(edit.is_error, Some(false));
    assert_eq!(playback.0.load(AtomicOrdering::Relaxed), 0);
}

#[test]
fn m31_agent_tools_expose_captions_qa_and_delivery_proofs() {
    let names = KinewrightMcp::tools()
        .unwrap()
        .into_iter()
        .map(|tool| tool.name.into_owned())
        .collect::<Vec<_>>();
    for name in [
        "get_caption_presets",
        "get_captions",
        "get_transcripts",
        "plan_caption_corrections",
        "add_styled_captions",
        "get_qa_report",
        "get_delivery_variants",
        "get_delivery_variant_storyboard",
        "get_editorial_readiness",
    ] {
        assert!(names.iter().any(|candidate| candidate == name));
    }

    let (core, playback, analysis) = fixture();
    let service = KinewrightMcp::new(core, playback, analysis, ConfirmationBroker::default());
    let result = service
        .delivery_variant_storyboard(DeliveryStoryboardArgs {
            aspect: DeliveryAspect::Vertical,
            focus_x_percent: 25,
            focus_y_percent: 50,
            storyboard: StoryboardArgs {
                range: None,
                frame_count: Some(2),
                max_width: Some(64),
            },
        })
        .unwrap();
    assert_eq!(result.is_error, Some(false));
    assert_eq!(
        result.structured_content.unwrap()["delivery_variant"]["aspect"],
        "vertical"
    );

    let readiness = service
        .editorial_readiness(&EditorialReadinessArgs {
            profile: DeliveryProfile::VerticalShort,
            check_silence: true,
            min_silence_source_frames: Some(TimeCode(20)),
            focus_x_percent: 50,
            focus_y_percent: 50,
            storyboard: StoryboardArgs {
                range: None,
                frame_count: Some(2),
                max_width: Some(64),
            },
        })
        .unwrap();
    assert_eq!(readiness.is_error, Some(false));
    assert_eq!(readiness.structured_content.unwrap()["ready"], false);

    let readiness = service
        .editorial_readiness(&EditorialReadinessArgs {
            profile: DeliveryProfile::VerticalShort,
            check_silence: false,
            min_silence_source_frames: None,
            focus_x_percent: 50,
            focus_y_percent: 50,
            storyboard: StoryboardArgs {
                range: None,
                frame_count: Some(2),
                max_width: Some(64),
            },
        })
        .unwrap();
    assert_eq!(readiness.is_error, Some(false));
    let readiness = readiness.structured_content.unwrap();
    assert_eq!(readiness["silence"]["checked"], false);
    assert_eq!(readiness["silence"]["pending_asset_ids"], json!([]));
    let qa_color_warning = readiness["qa"]["warning_issues"]
        .as_array()
        .unwrap()
        .iter()
        .find(|issue| issue["code"] == "source_color_metadata_uncertain")
        .expect("readiness should expose source colour review by asset");
    assert_eq!(qa_color_warning["asset"], 1);
    assert_eq!(qa_color_warning["severity"], "warning");
    let delivery_color_warning = readiness["delivery"]["warning_issues"]
        .as_array()
        .unwrap()
        .iter()
        .find(|issue| issue["code"] == "source_color_metadata_uncertain")
        .expect("delivery readiness should retain source colour review");
    assert_eq!(delivery_color_warning["asset"], 1);
}

#[test]
fn caption_inspection_and_correction_planning_are_compact_and_revision_bound() {
    let (core, playback, analysis) = fixture();
    let operations = vec![
        Operation::AddTrack {
            track: Track {
                id: TrackId(2),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: Vec::new(),
            },
        },
        Operation::AddTitle {
            track: TrackId(2),
            at: TimeCode::ZERO,
            duration: TimeCode(30),
            title: CaptionPreset::Social.title("Map Steady the Exped"),
        },
    ];
    let event = core
        .request(Command::DoBatchIfRevision {
            expected: TimelineRevision::default(),
            operations,
        })
        .unwrap();
    let Event::DocumentChanged { revision, .. } = event else {
        panic!("caption fixture should apply");
    };
    let service = KinewrightMcp::new(core, playback, analysis, ConfirmationBroker::default());

    let page = service
        .captions(CaptionListArgs {
            range: None,
            offset: None,
            limit: Some(1),
        })
        .unwrap();
    assert_eq!(page.is_error, Some(false));
    let page = page.structured_content.unwrap();
    assert_eq!(page["total"], 1);
    assert_eq!(page["captions"][0]["clip_id"], 2);
    assert_eq!(page["captions"][0]["text"], "Map Steady the Exped");

    let plan = service
        .plan_caption_corrections(CaptionCorrectionPlanArgs {
            expected_revision: revision,
            corrections: vec![CaptionCorrection {
                clip_id: ClipId(2),
                text: "River map steadies the expedition".to_owned(),
            }],
        })
        .unwrap();
    assert_eq!(plan.is_error, Some(false));
    let plan = plan.structured_content.unwrap();
    assert_eq!(plan["timeline_revision"], revision.0);
    assert_eq!(plan["prepared_edit_plan"]["plan_id"], 1);
    assert_eq!(plan["prepared_edit_plan"]["preview"]["operation_count"], 1);

    let unchanged = service
        .captions(CaptionListArgs {
            range: None,
            offset: None,
            limit: None,
        })
        .unwrap()
        .structured_content
        .unwrap();
    assert_eq!(unchanged["captions"][0]["text"], "Map Steady the Exped");

    let stale = service
        .plan_caption_corrections(CaptionCorrectionPlanArgs {
            expected_revision: TimelineRevision::default(),
            corrections: vec![CaptionCorrection {
                clip_id: ClipId(2),
                text: "River map steadies the expedition".to_owned(),
            }],
        })
        .unwrap();
    assert_eq!(stale.is_error, Some(true));

    let media_clip = service
        .plan_caption_corrections(CaptionCorrectionPlanArgs {
            expected_revision: revision,
            corrections: vec![CaptionCorrection {
                clip_id: ClipId(1),
                text: "Not a caption".to_owned(),
            }],
        })
        .unwrap();
    assert_eq!(media_clip.is_error, Some(true));

    let committed = commit_prepared_plan(&service, &plan, revision);
    assert_eq!(committed.is_error, Some(false));
    let corrected = service
        .captions(CaptionListArgs {
            range: None,
            offset: None,
            limit: None,
        })
        .unwrap()
        .structured_content
        .unwrap();
    assert_eq!(
        corrected["captions"][0]["text"],
        "River map steadies the expedition"
    );
}

#[test]
fn approved_confirmation_applies_the_operation() {
    let (core, playback, analysis) = fixture();
    let broker = ConfirmationBroker::with_timeout(Duration::from_secs(1));
    let result = invoke_in_background(
        KinewrightMcp::new(core.clone(), playback, analysis, broker.clone()),
        delete_request(),
    );
    let request = wait_for_request(&broker);
    assert!(broker.approve(request.id));
    let result = result
        .recv_timeout(Duration::from_secs(1))
        .unwrap()
        .unwrap();
    assert_eq!(result.is_error, Some(false));
    let Event::QueryResult(QueryResult::Document(document)) =
        core.request(Command::Query(Query::Document)).unwrap()
    else {
        panic!("expected document query result");
    };
    assert!(document.clip(ClipId(1)).is_none());
}

#[test]
fn rejected_confirmation_returns_a_refusal_tool_result() {
    let (core, playback, analysis) = fixture();
    let broker = ConfirmationBroker::with_timeout(Duration::from_secs(1));
    let result = invoke_in_background(
        KinewrightMcp::new(core, playback, analysis, broker.clone()),
        delete_request(),
    );
    let request = wait_for_request(&broker);
    assert!(broker.reject(request.id, "rejected by user"));
    let result = result
        .recv_timeout(Duration::from_secs(1))
        .unwrap()
        .unwrap();
    assert_eq!(result.is_error, Some(true));
    assert!(
        result.content[0]
            .as_text()
            .unwrap()
            .text
            .contains("rejected by user")
    );
}

#[test]
fn confirmation_timeout_rejects_the_operation() {
    let (core, playback, analysis) = fixture();
    let broker = ConfirmationBroker::with_timeout(Duration::from_millis(10));
    let service = KinewrightMcp::new(core, playback, analysis, broker);
    let result = service.call_blocking(delete_request()).unwrap();
    assert_eq!(result.is_error, Some(true));
    assert!(
        result.content[0]
            .as_text()
            .unwrap()
            .text
            .contains("timed out")
    );
}

#[test]
fn interrupting_a_pending_confirmation_does_not_deadlock() {
    let (core, playback, analysis) = fixture();
    let broker = ConfirmationBroker::with_timeout(Duration::from_secs(30));
    let result = invoke_in_background(
        KinewrightMcp::new(core, playback, analysis, broker.clone()),
        delete_request(),
    );
    let _request = wait_for_request(&broker);
    broker.reject_all("session interrupted");
    let result = result
        .recv_timeout(Duration::from_secs(1))
        .unwrap()
        .unwrap();
    assert_eq!(result.is_error, Some(true));
    assert!(
        result.content[0]
            .as_text()
            .unwrap()
            .text
            .contains("session interrupted")
    );
}

#[test]
fn removing_a_nonempty_track_requires_confirmation() {
    let (core, playback, analysis) = fixture();
    let broker = ConfirmationBroker::with_timeout(Duration::from_secs(1));
    let request = CallToolRequestParams::new("remove_track").with_arguments(
        json!({"expected_revision": 0, "track": 1})
            .as_object()
            .unwrap()
            .clone(),
    );
    let result = invoke_in_background(
        KinewrightMcp::new(core, playback, analysis, broker.clone()),
        request,
    );
    let request = wait_for_request(&broker);
    assert!(request.description.contains("1 clip(s)"));
    assert!(broker.reject(request.id, "keep the track"));
    let result = result
        .recv_timeout(Duration::from_secs(1))
        .unwrap()
        .unwrap();
    assert_eq!(result.is_error, Some(true));
}

#[test]
fn ripple_delete_is_destructive_while_marker_and_title_edits_are_suggestions() {
    let (core, playback, analysis) = fixture();
    let service = KinewrightMcp::new(core, playback, analysis, ConfirmationBroker::default());
    let document = service.document().unwrap();
    assert!(
        KinewrightMcp::confirmation_description(
            &document,
            &Operation::RippleDeleteClip { clip: ClipId(1) },
        )
        .is_some()
    );
    for operation in [
        Operation::AddMarker {
            marker: Marker {
                id: MarkerId(1),
                position: TimeCode(5),
                label: "Review".to_owned(),
                color_token: 0,
            },
        },
        Operation::MoveMarker {
            marker: MarkerId(1),
            to: TimeCode(10),
        },
        Operation::RemoveMarker {
            marker: MarkerId(1),
        },
        Operation::AddTitle {
            track: TrackId(1),
            at: TimeCode(60),
            duration: TimeCode(30),
            title: Title::default(),
        },
        Operation::SetTitleParam {
            clip: ClipId(1),
            name: "text".to_owned(),
            value: ParamValue::Text("Title".to_owned()),
        },
    ] {
        assert!(KinewrightMcp::confirmation_description(&document, &operation).is_none());
    }
}

#[test]
fn generated_plan_schema_composes_the_operation_schema() {
    let tool = KinewrightMcp::tools()
        .unwrap()
        .into_iter()
        .find(|tool| tool.name == "apply_edit_plan")
        .unwrap();
    let schema = serde_json::to_string(&tool.input_schema).unwrap();
    assert!(schema.contains("AddTrack"));
    assert!(schema.contains("DeleteClip"));
    assert!(schema.contains("operations"));
}

#[test]
fn served_surface_is_small_and_keeps_the_internal_registry_discoverable() {
    let registry = KinewrightMcp::capability_tools().unwrap();
    let served = KinewrightMcp::served_tools().unwrap();
    let names = served
        .iter()
        .map(|tool| tool.name.as_ref())
        .collect::<Vec<_>>();
    assert_eq!(names, crate::runtime::COMPACT_TOOL_NAMES);

    let registry_metrics = ToolSurfaceMetrics::measure(&registry);
    let served_metrics = ToolSurfaceMetrics::measure(&served);
    println!("registry={registry_metrics:?} served={served_metrics:?}");
    assert_eq!(
        registry_metrics.tool_count,
        operation_tools().unwrap().len() + crate::schema::INSPECTOR_TOOL_NAMES.len()
    );
    assert_eq!(served_metrics.tool_count, 7);
    assert!(served_metrics.tool_count < registry_metrics.tool_count / 4);
    assert!(served_metrics.serialized_bytes < registry_metrics.serialized_bytes / 4);
    // CC7 §5.4, R2-MAJ-3: M36's registry byte count is only measurable from
    // inside the crate (`capability_tools` is private), so it is pinned
    // here beside the served figure CC7 asserts is byte-identical to CC6's.
    // Errata D-E9 claimed this test already did that; it did not until now.
    assert_eq!(
        (
            registry_metrics.serialized_bytes,
            served_metrics.serialized_bytes
        ),
        (1_280_060, 5_660),
        "registry={registry_metrics:?} served={served_metrics:?}"
    );

    let catalog = capabilities(&registry);
    assert!(
        catalog
            .iter()
            .any(|capability| capability.name == "split_clip")
    );
    assert!(
        catalog
            .iter()
            .any(|capability| capability.name == "get_timeline_storyboard")
    );
}

#[test]
fn compact_prepare_and_commit_is_revision_gated_and_atomic() {
    let (core, playback, analysis) = fixture();
    let service = KinewrightMcp::configured(
        core.clone(),
        playback,
        analysis,
        None,
        ConfirmationBroker::default(),
        true,
        Arc::new(RwLock::new(None)),
    );
    let prepared = service
        .call_blocking(
            CallToolRequestParams::new("prepare_edit_plan").with_arguments(
                json!({
                    "expected_revision": 0,
                    "operations": [{"op": "split_clip", "clip": 1, "at": 30}]
                })
                .as_object()
                .unwrap()
                .clone(),
            ),
        )
        .unwrap();
    assert_eq!(prepared.is_error, Some(false));
    let prepared = prepared.structured_content.unwrap();
    assert_eq!(prepared["preview"]["operation_count"], 1);
    assert_eq!(prepared["preview"]["before_clips"], 1);
    assert_eq!(prepared["preview"]["after_clips"], 2);

    let Event::QueryResult(QueryResult::Document(before_commit)) =
        core.request(Command::Query(Query::Document)).unwrap()
    else {
        panic!("expected document query result");
    };
    assert_eq!(before_commit.tracks[0].clips.len(), 1);

    let committed = service
        .call_blocking(
            CallToolRequestParams::new("commit_edit_plan").with_arguments(
                json!({
                    "plan_id": prepared["plan_id"],
                    "expected_revision": 0
                })
                .as_object()
                .unwrap()
                .clone(),
            ),
        )
        .unwrap();
    assert_eq!(committed.is_error, Some(false));
    let Event::QueryResult(QueryResult::Snapshot { revision, document }) =
        core.request(Command::Query(Query::Snapshot)).unwrap()
    else {
        panic!("expected snapshot query result");
    };
    assert_eq!(revision, TimelineRevision(1));
    assert_eq!(document.tracks[0].clips.len(), 2);

    let duplicate = service
        .call_blocking(
            CallToolRequestParams::new("commit_edit_plan").with_arguments(
                json!({
                    "plan_id": prepared["plan_id"],
                    "expected_revision": 0
                })
                .as_object()
                .unwrap()
                .clone(),
            ),
        )
        .unwrap();
    assert_eq!(duplicate.is_error, Some(true));
}

#[test]
fn compact_capability_dispatcher_opens_and_invokes_existing_inspectors() {
    let (core, playback, analysis) = fixture();
    let service = KinewrightMcp::configured(
        core,
        playback,
        analysis,
        None,
        ConfirmationBroker::default(),
        true,
        Arc::new(RwLock::new(None)),
    );
    let opened = service
        .call_blocking(
            CallToolRequestParams::new("get_capability").with_arguments(
                json!({"name": "get_clip_info"})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .unwrap();
    assert_eq!(opened.is_error, Some(false));
    assert_eq!(
        opened.structured_content.unwrap()["invocation"],
        "invoke_capability"
    );

    let invoked = service
        .call_blocking(
            CallToolRequestParams::new("invoke_capability").with_arguments(
                json!({
                    "name": "get_clip_info",
                    "arguments": {"clip_id": 1}
                })
                .as_object()
                .unwrap()
                .clone(),
            ),
        )
        .unwrap();
    assert_eq!(invoked.is_error, Some(false));
    assert!(
        invoked.content[0]
            .as_text()
            .unwrap()
            .text
            .contains("clip 1")
    );
}

#[test]
fn compact_agent_plan_operations_decode_without_a_rust_enum_envelope() {
    let decoded = decode_plan_operation_value(json!({
        "op": "split_clip",
        "clip": 1,
        "at": 30
    }))
    .unwrap();
    assert_eq!(
        decoded,
        Operation::SplitClip {
            clip: ClipId(1),
            at: TimeCode(30),
        }
    );
    let snake_envelope = decode_plan_operation_value(json!({"add_marker": {"marker": {
        "id": 1, "position": 30, "label": "proof", "color_token": 0
    }}}))
    .unwrap();
    assert!(matches!(snake_envelope, Operation::AddMarker { .. }));
}

#[test]
fn edit_plan_applies_atomically_and_undoes_once() {
    let (core, playback, analysis) = fixture();
    let Event::QueryResult(QueryResult::Document(original)) =
        core.request(Command::Query(Query::Document)).unwrap()
    else {
        panic!("expected document");
    };
    let service = KinewrightMcp::new(
        core.clone(),
        playback,
        analysis,
        ConfirmationBroker::default(),
    );
    let result = service
        .call_blocking(plan_request(json!([
            {"AddTrack": {"track": {"id": 2, "kind": "Video", "clips": []}}},
            {"MoveClip": {"clip": 1, "to_track": 2, "to": 0}}
        ])))
        .unwrap();
    assert_eq!(result.is_error, Some(false));
    let text = &result.content[0].as_text().unwrap().text;
    assert!(text.contains("op 1 add_track: applied"));
    assert!(text.contains("op 2 move_clip: applied"));

    let Event::DocumentChanged { doc, .. } = core.request(Command::Undo).unwrap() else {
        panic!("expected undo result");
    };
    assert_eq!(&*doc, &*original);
}

#[test]
fn successful_bulk_plan_outcomes_are_counted_instead_of_repeated() {
    let operations = (1..=48)
        .map(|id| Operation::AddMarker {
            marker: Marker {
                id: MarkerId(id),
                position: TimeCode(i64::try_from(id).unwrap()),
                label: String::new(),
                color_token: 0,
            },
        })
        .collect::<Vec<_>>();

    let rendered = render_plan_outcomes(&operations, None, None);
    assert_eq!(rendered, "applied 48 operations atomically (add_marker=48)");
}

#[test]
fn capability_discovery_batches_queries_and_schema_opens() {
    let tools = KinewrightMcp::tools().unwrap();
    let found = search_capability_queries(
        &tools,
        &CapabilitySearchArgs {
            query: None,
            queries: vec!["dialogue assembly".to_owned(), "styled captions".to_owned()],
            kinds: Vec::new(),
            limit: None,
        },
    );
    assert!(
        found
            .iter()
            .any(|capability| capability.name == "plan_dialogue_assembly")
    );
    assert!(
        found
            .iter()
            .any(|capability| capability.name == "add_styled_captions")
    );

    let opened = open_capabilities(
        &tools,
        CapabilityArgs {
            name: Some("plan_dialogue_assembly".to_owned()),
            names: vec![
                "add_styled_captions".to_owned(),
                "plan_dialogue_assembly".to_owned(),
            ],
        },
    );
    assert_eq!(opened.is_error, Some(false));
    let structured = opened.structured_content.unwrap();
    assert_eq!(structured["capabilities"].as_array().unwrap().len(), 2);
    let serialized = serde_json::to_string(&structured).unwrap();
    assert!(serialized.contains("script"));
    assert!(serialized.contains("Punctuation becomes a hard cue-grouping"));
}

#[test]
fn mixed_validity_edit_plan_rejects_without_partial_state() {
    let (core, playback, analysis) = fixture();
    let service = KinewrightMcp::new(
        core.clone(),
        playback,
        analysis,
        ConfirmationBroker::default(),
    );
    let result = service
        .call_blocking(plan_request(json!([
            {"AddTrack": {"track": {"id": 2, "kind": "Video", "clips": []}}},
            {"AddTrack": {"track": {"id": 2, "kind": "Video", "clips": []}}}
        ])))
        .unwrap();
    assert_eq!(result.is_error, Some(true));
    let text = &result.content[0].as_text().unwrap().text;
    assert!(text.contains("op 1 add_track: rolled back"));
    assert!(text.contains("op 2 add_track: rejected"));
    let Event::QueryResult(QueryResult::Document(document)) =
        core.request(Command::Query(Query::Document)).unwrap()
    else {
        panic!("expected document");
    };
    assert!(document.tracks.iter().all(|track| track.id != TrackId(2)));
}

#[test]
fn destructive_edit_plan_uses_one_summary_confirmation_for_approve_and_reject() {
    for approve in [true, false] {
        let (core, playback, analysis) = fixture();
        let broker = ConfirmationBroker::with_timeout(Duration::from_secs(1));
        let result = invoke_in_background(
            KinewrightMcp::new(core.clone(), playback, analysis, broker.clone()),
            plan_request(json!([
                {"RemoveTrack": {"track": 1}}
            ])),
        );
        let request = wait_for_request(&broker);
        assert_eq!(request.tool_name, "apply_edit_plan");
        assert_eq!(
            request.description,
            "Plan removes 1 clip and 1 track - approve?"
        );
        if approve {
            assert!(broker.approve(request.id));
        } else {
            assert!(broker.reject(request.id, "keep the plan unchanged"));
        }
        let result = result
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .unwrap();
        assert_eq!(result.is_error, Some(!approve));
        let Event::QueryResult(QueryResult::Document(document)) =
            core.request(Command::Query(Query::Document)).unwrap()
        else {
            panic!("expected document");
        };
        assert_eq!(document.tracks.is_empty(), approve);
    }
}

/// `schema::INSPECTOR_TOOL_NAMES` and `server::inspector_tools()` are two
/// hand-maintained lists of the same 75 capabilities. Neither can be derived
/// from the other without moving the tool descriptions across crates, so this
/// test is the seam: a name added to one and not the other fails here by
/// name rather than as a count somewhere else.
#[test]
fn inspector_tool_names_are_exactly_the_inspector_registry() {
    let mut registry = crate::server::inspector_tools()
        .iter()
        .map(|tool| tool.name.to_string())
        .collect::<Vec<_>>();
    let mut listed = crate::schema::INSPECTOR_TOOL_NAMES
        .iter()
        .map(|name| (*name).to_owned())
        .collect::<Vec<_>>();
    assert_eq!(
        registry.len(),
        listed.len(),
        "the registry and INSPECTOR_TOOL_NAMES disagree on how many inspector tools exist"
    );
    registry.sort_unstable();
    listed.sort_unstable();
    assert_eq!(
        registry, listed,
        "INSPECTOR_TOOL_NAMES must name exactly the tools inspector_tools() registers"
    );
}
