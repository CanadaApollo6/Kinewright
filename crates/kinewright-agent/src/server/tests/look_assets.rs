//! CC4 look asset import and legacy conversion tests.

use super::*;
use crate::server::look_assets::{lut_error_detail, lut_error_field_start, lut_store_error_result};

// -----------------------------------------------------------------------
// CC4 §10.3.14 — import authorization, plan rejection, and proof honesty
// -----------------------------------------------------------------------

fn cc4_project_directory(label: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let path = std::env::temp_dir().join(format!(
        "kinewright-cc4-{}-{label}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&path).unwrap();
    path
}

/// A real, parseable `.cube` source: the pinned built-in bake, which round
/// trips through the production parser by construction (CC4 §2.6).
fn cc4_write_source_cube(directory: &Path) -> PathBuf {
    let path = directory.join("warm.cube");
    std::fs::write(&path, kinewright_media::BuiltinLook::Warm.canonical_text()).unwrap();
    path
}

fn cc4_service_with_project(
    broker: ConfirmationBroker,
    project_path: Option<PathBuf>,
) -> KinewrightMcp {
    let (core, playback, analysis) = fixture();
    KinewrightMcp::configured(
        core,
        playback,
        analysis,
        None,
        broker,
        true,
        Arc::new(RwLock::new(project_path)),
    )
}

fn cc4_import_request(path: &Path) -> CallToolRequestParams {
    CallToolRequestParams::new("import_lut_asset").with_arguments(serde_json::Map::from_iter([
        ("expected_revision".to_owned(), serde_json::json!(0)),
        ("path".to_owned(), serde_json::json!(path)),
    ]))
}

fn cc4_document_of(service: &KinewrightMcp) -> Arc<Document> {
    let Event::QueryResult(QueryResult::Document(document)) = service
        .core
        .request(Command::Query(Query::Document))
        .unwrap()
    else {
        panic!("expected a document query result");
    };
    document
}

/// CC4 §8, §13: the confirmation is requested before the first byte is
/// read, so a refused import leaves no store file and no document change.
#[test]
fn cc4_refused_import_writes_no_store_file_and_changes_no_document() {
    let directory = cc4_project_directory("import-refused");
    let source = cc4_write_source_cube(&directory);
    let project = directory.join("edit.kinewright");
    let store_root = directory.join("edit.kinewright-assets");
    let broker = ConfirmationBroker::with_timeout(Duration::from_secs(2));
    let service = cc4_service_with_project(broker.clone(), Some(project));

    let result = invoke_in_background(service.clone(), cc4_import_request(&source));
    let request = wait_for_request(&broker);
    assert_eq!(request.tool_name, "import_lut_asset");
    assert!(
        request.description.contains("edit.kinewright-assets"),
        "the operator is told exactly where the bytes would be written: {}",
        request.description
    );
    assert!(broker.reject(request.id, "rejected by user"));

    let result = result
        .recv_timeout(Duration::from_secs(2))
        .unwrap()
        .unwrap();
    assert_eq!(result.is_error, Some(true));
    let structured = result.structured_content.unwrap();
    assert_eq!(structured["code"], "import_refused");
    assert_eq!(structured["details"]["store_file_written"], false);
    assert_eq!(structured["details"]["document_changed"], false);

    assert!(
        !store_root.exists(),
        "a refused import must not create the project LUT store"
    );
    let document = cc4_document_of(&service);
    assert!(document.lut_assets.is_empty());
    let _ = std::fs::remove_dir_all(&directory);
}

/// CC4 §2.4: an approved import parses, hashes, stores, and registers the
/// asset as one undoable `AddLutAsset`.
#[test]
fn cc4_approved_import_registers_the_hashed_asset() {
    let directory = cc4_project_directory("import-approved");
    let source = cc4_write_source_cube(&directory);
    let project = directory.join("edit.kinewright");
    let broker = ConfirmationBroker::with_timeout(Duration::from_secs(2));
    let service = cc4_service_with_project(broker.clone(), Some(project));

    let result = invoke_in_background(service.clone(), cc4_import_request(&source));
    let request = wait_for_request(&broker);
    assert!(broker.approve(request.id));
    let result = result
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .unwrap();
    assert_eq!(result.is_error, Some(false), "{:?}", result.content);

    let expected_sha = kinewright_media::BuiltinLook::Warm.pinned_sha256();
    let structured = result.structured_content.unwrap();
    assert_eq!(structured["lut_asset"]["lut_asset_id"], 1);
    assert_eq!(structured["lut_asset"]["sha256"], expected_sha);
    assert_eq!(structured["lut_asset"]["kind"], "cube_3d");
    assert_eq!(structured["applied"], true);

    let stored = directory
        .join("edit.kinewright-assets")
        .join("luts")
        .join(format!("{expected_sha}.cube"));
    assert!(
        stored.is_file(),
        "the hashed bytes land in the project store"
    );

    let document = cc4_document_of(&service);
    assert_eq!(document.lut_assets.len(), 1);
    assert_eq!(document.lut_assets[0].sha256, expected_sha);

    // The asset is immediately visible to the read-only look surface with
    // a verified availability, because the store root is now known.
    let listed = service.list_look_assets().unwrap();
    let listed = listed.structured_content.unwrap();
    assert_eq!(listed["store_root_known"], true);
    assert_eq!(listed["assets"][0]["availability"]["kind"], "verified");
    assert_eq!(listed["assets"][0]["referenced_by"], serde_json::json!([]));
    let _ = std::fs::remove_dir_all(&directory);
}

/// CC4 §2.2: a project that has never been saved has no store root, and
/// the refusal is typed rather than an invented temporary location.
#[test]
fn cc4_import_requires_a_saved_project() {
    let directory = cc4_project_directory("import-unsaved");
    let source = cc4_write_source_cube(&directory);
    let broker = ConfirmationBroker::with_timeout(Duration::from_millis(50));
    let service = cc4_service_with_project(broker, None);

    let result = service.call_blocking(cc4_import_request(&source)).unwrap();
    assert_eq!(result.is_error, Some(true));
    let structured = result.structured_content.unwrap();
    assert_eq!(structured["code"], "project_not_saved");
    assert_eq!(structured["details"]["field"], "project_path");
    assert!(
        structured["details"]["recovery_action"]
            .as_str()
            .unwrap()
            .contains("Save the project")
    );
    let _ = std::fs::remove_dir_all(&directory);
}

/// A service whose clip 1 carries exactly `effects`.
fn cc4_legacy_service(
    effects: Vec<Effect>,
    broker: ConfirmationBroker,
    project_path: Option<PathBuf>,
) -> KinewrightMcp {
    let (seed_core, _, _) = fixture();
    let Event::QueryResult(QueryResult::Document(seed)) =
        seed_core.request(Command::Query(Query::Document)).unwrap()
    else {
        panic!("expected fixture document");
    };
    let mut document = (*seed).clone();
    document.tracks[0].clips[0].effects = effects;
    document
        .validate()
        .expect("the seeded legacy stack is valid");
    let media = Arc::new(NoopMedia::default());
    KinewrightMcp::configured(
        Core::spawn(document).unwrap(),
        media.clone(),
        media,
        None,
        broker,
        true,
        Arc::new(RwLock::new(project_path)),
    )
}

fn cc4_look_lut(id: u64, preset_token: i64, intensity_percent: i64) -> Effect {
    Effect {
        id: EffectId(id),
        name: "look_lut".to_owned(),
        parameters: BTreeMap::from([
            ("preset_token".to_owned(), ParamValue::Integer(preset_token)),
            (
                "intensity_percent".to_owned(),
                ParamValue::Integer(intensity_percent),
            ),
        ]),
        keyframes: BTreeMap::new(),
    }
}

fn cc4_convert_request(revision: u64, clip: u64, effect: u64) -> CallToolRequestParams {
    CallToolRequestParams::new("convert_legacy_look").with_arguments(serde_json::Map::from_iter([
        ("expected_revision".to_owned(), serde_json::json!(revision)),
        ("clip_id".to_owned(), serde_json::json!(clip)),
        ("effect_id".to_owned(), serde_json::json!(effect)),
    ]))
}

/// CC4 §8, §9: the published `[AddLutAsset, ConvertLegacyLook]` batch is
/// only `ready` because one tool can submit it. The built-in is registered
/// exactly once; a second conversion of the same look reuses that record
/// rather than allocating a duplicate id for identical bytes.
#[test]
fn cc4_convert_legacy_look_registers_the_builtin_once_and_reuses_the_record() {
    let service = cc4_legacy_service(
        vec![cc4_look_lut(5, 1, 50), cc4_look_lut(6, 1, 100)],
        ConfirmationBroker::default(),
        None,
    );

    // The evidence surface names the tool that can actually submit it.
    let context = service
        .call_blocking(CallToolRequestParams::new("get_color_context"))
        .unwrap();
    let context = context.structured_content.unwrap();
    let conversions = context["legacy_look_conversions"].as_array().unwrap();
    assert_eq!(conversions.len(), 2);
    assert_eq!(conversions[0]["status"], "ready");
    assert_eq!(conversions[0]["builtin_name"], "warm");
    assert_eq!(conversions[0]["mix_basis_points"], 5_000);
    assert!(
        conversions[0]["recovery_action"]
            .as_str()
            .unwrap()
            .contains("convert_legacy_look"),
        "{}",
        conversions[0]
    );

    let first = service.call_blocking(cc4_convert_request(0, 1, 5)).unwrap();
    assert_eq!(first.is_error, Some(false), "{:?}", first.content);
    let first = first.structured_content.unwrap();
    assert_eq!(first["applied"], true);
    assert_eq!(first["bit_identical_to_legacy"], false);
    assert_eq!(first["conversion"]["source"], "builtin");
    assert_eq!(first["conversion"]["reused_existing_asset"], false);
    assert_eq!(first["conversion"]["store_file_written"], false);
    assert_eq!(first["conversion"]["mix_basis_points"], 5_000);
    assert_eq!(first["operations"].as_array().unwrap().len(), 2);
    // A built-in needs no store, so its availability is still verified.
    assert_eq!(first["lut_asset"]["availability"]["kind"], "verified");
    assert_eq!(
        first["lut_asset"]["recovery_action"],
        serde_json::Value::Null
    );

    let document = cc4_document_of(&service);
    assert_eq!(document.lut_assets.len(), 1);
    assert_eq!(
        document.lut_assets[0].sha256,
        kinewright_media::BuiltinLook::Warm.pinned_sha256()
    );
    let effects = &document.tracks[0].clips[0].effects;
    assert_eq!(effects[0].name, "creative_look");
    assert_eq!(effects[0].id, EffectId(5));
    assert_eq!(
        effects[0].parameters["mix_basis_points"],
        ParamValue::Integer(5_000)
    );
    assert_eq!(effects[1].name, "look_lut");

    let second = service.call_blocking(cc4_convert_request(1, 1, 6)).unwrap();
    assert_eq!(second.is_error, Some(false), "{:?}", second.content);
    let second = second.structured_content.unwrap();
    assert_eq!(second["conversion"]["reused_existing_asset"], true);
    assert_eq!(second["conversion"]["lut_asset_id"], 1);
    assert_eq!(
        second["operations"].as_array().unwrap().len(),
        1,
        "the registered record is reused, so no second AddLutAsset is emitted"
    );

    let document = cc4_document_of(&service);
    assert_eq!(
        document.lut_assets.len(),
        1,
        "identical bytes are one content-addressed asset"
    );
    assert!(
        document.tracks[0].clips[0]
            .effects
            .iter()
            .all(|effect| effect.name == "creative_look")
    );
}

/// CC4 §8: the conversion is revision-gated and fails closed.
#[test]
fn cc4_convert_legacy_look_fails_closed_on_a_stale_revision() {
    let service = cc4_legacy_service(
        vec![cc4_look_lut(5, 2, 100)],
        ConfirmationBroker::default(),
        None,
    );
    let stale = service.call_blocking(cc4_convert_request(9, 1, 5)).unwrap();
    assert_eq!(stale.is_error, Some(true));
    let stale = stale.structured_content.unwrap();
    assert_eq!(stale["code"], "revision_conflict");
    assert_eq!(stale["details"]["field"], "expected_revision");
    assert_eq!(stale["details"]["observed"], 9);
    assert_eq!(stale["details"]["allowed"], 0);
    assert_eq!(stale["applied"], false);

    let document = cc4_document_of(&service);
    assert!(document.lut_assets.is_empty());
    assert_eq!(document.tracks[0].clips[0].effects[0].name, "look_lut");
}

/// CC4 §8, §13: a `cube_lut` conversion imports through the same
/// confirmation path as `import_lut_asset`, so a refusal leaves no store
/// file and no document change.
#[test]
fn cc4_refused_legacy_cube_conversion_writes_nothing() {
    let directory = cc4_project_directory("convert-refused");
    let source = cc4_write_source_cube(&directory);
    let project = directory.join("edit.kinewright");
    let store_root = directory.join("edit.kinewright-assets");
    let broker = ConfirmationBroker::with_timeout(Duration::from_secs(2));
    let service = cc4_legacy_service(
        vec![Effect {
            id: EffectId(5),
            name: "cube_lut".to_owned(),
            parameters: BTreeMap::from([(
                "path".to_owned(),
                ParamValue::Text(source.display().to_string()),
            )]),
            keyframes: BTreeMap::new(),
        }],
        broker.clone(),
        Some(project),
    );

    let result = invoke_in_background(service.clone(), cc4_convert_request(0, 1, 5));
    let request = wait_for_request(&broker);
    assert_eq!(request.tool_name, "convert_legacy_look");
    assert!(
        request.description.contains("edit.kinewright-assets"),
        "the operator is told exactly where the bytes would be written: {}",
        request.description
    );
    assert!(broker.reject(request.id, "rejected by user"));

    let result = result
        .recv_timeout(Duration::from_secs(2))
        .unwrap()
        .unwrap();
    assert_eq!(result.is_error, Some(true));
    let structured = result.structured_content.unwrap();
    assert_eq!(structured["code"], "import_refused");
    assert_eq!(structured["details"]["store_file_written"], false);
    assert_eq!(structured["details"]["document_changed"], false);

    assert!(
        !store_root.exists(),
        "a refused conversion must not create the project LUT store"
    );
    let document = cc4_document_of(&service);
    assert!(document.lut_assets.is_empty());
    assert_eq!(document.tracks[0].clips[0].effects[0].name, "cube_lut");
    let _ = std::fs::remove_dir_all(&directory);
}

/// CC4 §8: an approved `cube_lut` conversion imports the bytes and
/// converts in one batch.
#[test]
fn cc4_approved_legacy_cube_conversion_imports_and_converts() {
    let directory = cc4_project_directory("convert-approved");
    let source = cc4_write_source_cube(&directory);
    let project = directory.join("edit.kinewright");
    let broker = ConfirmationBroker::with_timeout(Duration::from_secs(2));
    let service = cc4_legacy_service(
        vec![Effect {
            id: EffectId(5),
            name: "cube_lut".to_owned(),
            parameters: BTreeMap::from([
                (
                    "path".to_owned(),
                    ParamValue::Text(source.display().to_string()),
                ),
                ("intensity_percent".to_owned(), ParamValue::Integer(40)),
            ]),
            keyframes: BTreeMap::new(),
        }],
        broker.clone(),
        Some(project),
    );

    let result = invoke_in_background(service.clone(), cc4_convert_request(0, 1, 5));
    let request = wait_for_request(&broker);
    assert!(broker.approve(request.id));
    let result = result
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .unwrap();
    assert_eq!(result.is_error, Some(false), "{:?}", result.content);
    let structured = result.structured_content.unwrap();
    assert_eq!(structured["conversion"]["source"], "imported");
    assert_eq!(structured["conversion"]["store_file_written"], true);
    assert_eq!(structured["conversion"]["mix_basis_points"], 4_000);
    assert_eq!(structured["lut_asset"]["availability"]["kind"], "verified");

    let expected_sha = kinewright_media::BuiltinLook::Warm.pinned_sha256();
    assert!(
        directory
            .join("edit.kinewright-assets")
            .join("luts")
            .join(format!("{expected_sha}.cube"))
            .is_file()
    );
    let document = cc4_document_of(&service);
    assert_eq!(document.lut_assets.len(), 1);
    assert_eq!(document.tracks[0].clips[0].effects[0].name, "creative_look");
    let _ = std::fs::remove_dir_all(&directory);
}

/// CC4 §8: a node that cannot be converted carries the full typed
/// rejection shape, not a bare message.
///
/// The document-level `unconvertible` statuses (`invalid_preset_token`,
/// `missing_external_lut_path`) are only reachable from a hand-edited
/// project - Core rejects both at `validate` - so they are covered as a
/// unit on `legacy_look_conversions_value` in `color_status`.
#[test]
fn cc4_unconvertible_legacy_look_reports_field_observed_and_allowed() {
    let service = cc4_legacy_service(
        vec![Effect {
            id: EffectId(6),
            name: "primary_correction".to_owned(),
            parameters: BTreeMap::from([(
                "exposure_milli_stops".to_owned(),
                ParamValue::Integer(100),
            )]),
            keyframes: BTreeMap::new(),
        }],
        ConfirmationBroker::default(),
        None,
    );

    // A managed node is not a legacy look, and says so with its own shape.
    let refused = service.call_blocking(cc4_convert_request(0, 1, 6)).unwrap();
    assert_eq!(refused.is_error, Some(true));
    let refused = refused.structured_content.unwrap();
    assert_eq!(refused["code"], "not_a_legacy_look");
    assert_eq!(refused["details"]["field"], "effect_id");
    assert_eq!(refused["details"]["observed"], "primary_correction");
    assert_eq!(
        refused["details"]["allowed"],
        serde_json::json!(["look_lut", "cube_lut"])
    );
    assert!(refused["details"]["recovery_action"].is_string());
}

/// CC4 §2.1: importing the same bytes twice is the same asset, so the
/// second import reuses the record instead of allocating a second id.
#[test]
fn cc4_import_lut_asset_reuses_a_record_with_the_same_content_hash() {
    let directory = cc4_project_directory("import-dedup");
    let source = cc4_write_source_cube(&directory);
    let copy = directory.join("warm-copy.cube");
    std::fs::copy(&source, &copy).unwrap();
    let project = directory.join("edit.kinewright");
    let broker = ConfirmationBroker::with_timeout(Duration::from_secs(2));
    let service = cc4_service_with_project(broker.clone(), Some(project));

    let first = invoke_in_background(service.clone(), cc4_import_request(&source));
    assert!(broker.approve(wait_for_request(&broker).id));
    let first = first.recv_timeout(Duration::from_secs(5)).unwrap().unwrap();
    assert_eq!(first.is_error, Some(false));
    assert_eq!(
        first.structured_content.unwrap()["reused_existing_asset"],
        false
    );

    // A different path, the same bytes: still one asset.
    let request = CallToolRequestParams::new("import_lut_asset").with_arguments(
        serde_json::Map::from_iter([
            ("expected_revision".to_owned(), serde_json::json!(1)),
            ("path".to_owned(), serde_json::json!(copy)),
        ]),
    );
    let second = invoke_in_background(service.clone(), request);
    assert!(broker.approve(wait_for_request(&broker).id));
    let second = second
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .unwrap();
    assert_eq!(second.is_error, Some(false), "{:?}", second.content);
    let second = second.structured_content.unwrap();
    assert_eq!(second["reused_existing_asset"], true);
    assert_eq!(second["applied"], false);
    assert_eq!(second["lut_asset"]["lut_asset_id"], 1);

    let document = cc4_document_of(&service);
    assert_eq!(document.lut_assets.len(), 1);
    let _ = std::fs::remove_dir_all(&directory);
}

/// CC4 §8: every `import_lut_asset` rejection is structured, including the
/// revision conflict.
#[test]
fn cc4_import_lut_asset_revision_conflict_is_structured() {
    let directory = cc4_project_directory("import-conflict");
    let source = cc4_write_source_cube(&directory);
    let project = directory.join("edit.kinewright");
    let broker = ConfirmationBroker::with_timeout(Duration::from_millis(50));
    let service = cc4_service_with_project(broker, Some(project));

    let request = CallToolRequestParams::new("import_lut_asset").with_arguments(
        serde_json::Map::from_iter([
            ("expected_revision".to_owned(), serde_json::json!(7)),
            ("path".to_owned(), serde_json::json!(source)),
        ]),
    );
    let result = service.call_blocking(request).unwrap();
    assert_eq!(result.is_error, Some(true));
    let structured = result.structured_content.unwrap();
    assert_eq!(structured["code"], "revision_conflict");
    assert_eq!(structured["details"]["field"], "expected_revision");
    assert_eq!(structured["details"]["observed"], 7);
    assert_eq!(structured["details"]["allowed"], 0);
    assert!(
        structured["details"]["recovery_action"]
            .as_str()
            .unwrap()
            .contains("get_timeline_state")
    );
    assert!(
        !directory.join("edit.kinewright-assets").exists(),
        "a conflict is detected before the store is touched"
    );
    let _ = std::fs::remove_dir_all(&directory);
}

/// CC4 §2.3: a built-in is `verified` from this binary's own bake, so an
/// unsaved project reports it honestly instead of `unknown_no_store`.
/// Only an *imported* asset needs a store to resolve.
#[test]
fn cc4_builtin_availability_needs_no_store_root() {
    let (seed_core, _, _) = fixture();
    let Event::QueryResult(QueryResult::Document(seed)) =
        seed_core.request(Command::Query(Query::Document)).unwrap()
    else {
        panic!("expected fixture document");
    };
    let mut document = (*seed).clone();
    let mut stale =
        kinewright_media::BuiltinLook::Cool.to_lut_asset(kinewright_core::LutAssetId(2));
    stale.sha256 = "0".repeat(64);
    document.lut_assets = vec![
        kinewright_media::BuiltinLook::Warm.to_lut_asset(kinewright_core::LutAssetId(1)),
        stale,
        LutAsset {
            id: kinewright_core::LutAssetId(3),
            sha256: "a".repeat(64),
            title: "Imported".to_owned(),
            kind: kinewright_core::LutAssetKind::Cube3d,
            size: 2,
            byte_len: 64,
            domain_min_millionths: [0; 3],
            domain_max_millionths: [1_000_000; 3],
            source: kinewright_core::LutAssetSource::Imported {
                source_path: "vendor.cube".to_owned(),
            },
        },
    ];
    document
        .validate()
        .expect("the seeded asset table is valid");
    let media = Arc::new(NoopMedia::default());
    let service = KinewrightMcp::configured(
        Core::spawn(document).unwrap(),
        media.clone(),
        media,
        None,
        ConfirmationBroker::default(),
        true,
        Arc::new(RwLock::new(None)),
    );

    let listed = service.list_look_assets().unwrap();
    let listed = listed.structured_content.unwrap();
    assert_eq!(listed["store_root_known"], false);
    assert_eq!(listed["assets"][0]["availability"]["kind"], "verified");
    assert_eq!(
        listed["assets"][0]["recovery_action"],
        serde_json::Value::Null
    );
    assert_eq!(listed["assets"][1]["availability"]["kind"], "changed");
    assert!(
        listed["assets"][1]["recovery_action"]
            .as_str()
            .unwrap()
            .contains("sha256")
    );
    assert_eq!(
        listed["assets"][2]["availability"]["kind"], "unknown_no_store",
        "only imported bytes need a store to resolve"
    );
    assert!(
        listed["assets"][2]["recovery_action"]
            .as_str()
            .unwrap()
            .contains("Save the project")
    );
}

/// CC4 §8: the manifest asserts bypass identity, so a bypass variant that
/// is not the byte-identical twin of the node-removed variant refuses the
/// proof instead of publishing `bypass_matches_absent: false`.
#[test]
#[allow(clippy::too_many_lines)]
fn cc4_bypass_that_is_not_lossless_refuses_the_proof() {
    let (seed_core, _, _) = fixture();
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
    document.lut_assets =
        vec![kinewright_media::BuiltinLook::Warm.to_lut_asset(kinewright_core::LutAssetId(1))];
    document.tracks[0].clips[0].effects = vec![Effect {
        id: EffectId(9),
        name: "creative_look".to_owned(),
        parameters: BTreeMap::from([("lut_asset_id".to_owned(), ParamValue::Integer(1))]),
        keyframes: BTreeMap::new(),
    }];
    document
        .validate()
        .expect("the CC4 stack is a valid document");
    let media = Arc::new(NoopMedia {
        bypass_leaks_pixel: Some(0x7f),
        ..NoopMedia::default()
    });
    let service = KinewrightMcp::new(
        Core::spawn(document).unwrap(),
        media.clone(),
        media,
        ConfirmationBroker::default(),
    );

    let refused = service
        .render_color_proof(&RenderColorProofArgs {
            expected_revision: TimelineRevision(0),
            clip_id: ClipId(1),
            timecode: TimeCode(1),
            profile_assumption: None,
            parameters: BTreeMap::new(),
            effect_id: Some(EffectId(9)),
            look_comparison: Some(LookComparison::Bypass),
            matte_comparison: None,
        })
        .unwrap();
    assert_eq!(refused.is_error, Some(true));
    let structured = refused.structured_content.unwrap();
    assert_eq!(structured["code"], "bypass_not_lossless");
    assert_eq!(structured["details"]["field"], "look_comparison");
    assert_eq!(structured["details"]["effect_id"], 9);
    let observed = &structured["details"]["observed"];
    assert_ne!(
        observed["absent_rgba8_pixels_sha256"],
        observed["bypass_rgba8_pixels_sha256"]
    );
    assert_eq!(observed["absent_raster"]["width"], 320);
    assert_eq!(observed["bypass_raster"]["height"], 180);
    assert!(structured["details"]["recovery_action"].is_string());

    // The same node compares cleanly when the two variants agree.
    let clean_media = Arc::new(NoopMedia::default());
    let (seed_core, _, _) = fixture();
    let Event::QueryResult(QueryResult::Document(seed)) =
        seed_core.request(Command::Query(Query::Document)).unwrap()
    else {
        panic!("expected fixture document");
    };
    let mut clean = (*seed).clone();
    clean.media_pool[0].color_description = document_color_description_for_managed_proof();
    clean.lut_assets =
        vec![kinewright_media::BuiltinLook::Warm.to_lut_asset(kinewright_core::LutAssetId(1))];
    clean.tracks[0].clips[0].effects = vec![Effect {
        id: EffectId(9),
        name: "creative_look".to_owned(),
        parameters: BTreeMap::from([("lut_asset_id".to_owned(), ParamValue::Integer(1))]),
        keyframes: BTreeMap::new(),
    }];
    clean.validate().unwrap();
    let clean_service = KinewrightMcp::new(
        Core::spawn(clean).unwrap(),
        clean_media.clone(),
        clean_media,
        ConfirmationBroker::default(),
    );
    let proof = clean_service
        .render_color_proof(&RenderColorProofArgs {
            expected_revision: TimelineRevision(0),
            clip_id: ClipId(1),
            timecode: TimeCode(1),
            profile_assumption: None,
            parameters: BTreeMap::new(),
            effect_id: Some(EffectId(9)),
            look_comparison: Some(LookComparison::Bypass),
            matte_comparison: None,
        })
        .unwrap();
    assert_eq!(proof.is_error, Some(false), "{:?}", proof.content);
    assert_eq!(
        proof.structured_content.unwrap()["look_comparison"]["bypass_matches_absent"],
        true
    );
}

fn document_color_description_for_managed_proof() -> ColorDescription {
    ColorDescription {
        primaries: ColorPrimaries::Bt709,
        transfer: ColorTransfer::Bt709,
        matrix: ColorMatrix::Bt709,
        range: ColorRange::Limited,
        white_point: ColorWhitePoint::D65,
        bit_depth: ColorBitDepth::Eight,
        confidence_basis_points: 10_000,
        provenance: ColorProvenance::StreamMetadata,
    }
}

/// CC4 §8: the media LUT failure text is parsed with anchored field keys.
///
/// A bare substring search matched `line` inside a path component such as
/// `baseline`, and splitting on the first `"; "` truncated any value that
/// contained one - both of which a real filesystem path can produce.
#[test]
fn lut_error_fields_are_anchored_and_survive_semicolons_in_values() {
    let store = kinewright_core::MediaError::Backend(
        "lut_store_root_invalid: the derived store root is not a directory; observed=/home/e/baseline; takes/edit.kinewright-assets; allowed=a writable directory".to_owned(),
    );
    let result = lut_store_error_result("import_lut_asset", &store);
    let structured = result.structured_content.unwrap();
    assert_eq!(structured["code"], "lut_store_root_invalid");
    assert_eq!(
        structured["message"],
        "the derived store root is not a directory"
    );
    assert_eq!(
        structured["details"]["observed"], "/home/e/baseline; takes/edit.kinewright-assets",
        "the value runs to the next anchored key, not to the first \"; \""
    );
    assert_eq!(structured["details"]["allowed"], "a writable directory");
    assert_eq!(
        structured["details"]["line"],
        serde_json::Value::Null,
        "`baseline` is not a `line` field"
    );

    // The parser's own shape, which leads with `observed` and ends with a
    // 1-based line number.
    let parse = kinewright_core::MediaError::Backend(
        "invalid_lut_sample: observed 1.0 2.0; allowed three floats in 0..=1; line 42".to_owned(),
    );
    let structured = lut_store_error_result("import_lut_asset", &parse)
        .structured_content
        .unwrap();
    assert_eq!(structured["code"], "invalid_lut_sample");
    assert_eq!(structured["details"]["observed"], "1.0 2.0");
    assert_eq!(structured["details"]["allowed"], "three floats in 0..=1");
    assert_eq!(structured["details"]["line"], "42");
}

/// CC4 §8: a *value* that begins with another field's key is still a
/// value.
///
/// The anchor at offset 0 exists for the rendered remainder, which really
/// can lead with a key. Applying it while scanning inside an extracted
/// value made `observed=allowed=x` and `observed line 1 2 3 4` terminate
/// immediately and report the empty string.
#[test]
fn a_lut_error_value_that_begins_with_another_key_is_not_truncated() {
    // The `.cube` sample the parser rejected literally begins with the
    // word `line`, and the trailing `line` field still has to be found.
    let parse = kinewright_core::MediaError::Backend(
        "invalid_lut_sample: observed line 1 2 3 4; allowed three floats in 0..=1; line 12"
            .to_owned(),
    );
    let structured = lut_store_error_result("import_lut_asset", &parse)
        .structured_content
        .unwrap();
    assert_eq!(structured["details"]["observed"], "line 1 2 3 4");
    assert_eq!(structured["details"]["allowed"], "three floats in 0..=1");
    assert_eq!(structured["details"]["line"], "12");

    // The unified `; <key>=<value>` shape, with a value that begins with
    // the next key's name.
    let store = kinewright_core::MediaError::Backend(
        "lut_store_root_invalid: the derived store root is a symbolic link; observed=allowed=x; allowed=a writable directory; line=3"
            .to_owned(),
    );
    let structured = lut_store_error_result("import_lut_asset", &store)
        .structured_content
        .unwrap();
    assert_eq!(
        structured["message"],
        "the derived store root is a symbolic link"
    );
    assert_eq!(structured["details"]["observed"], "allowed=x");
    assert_eq!(structured["details"]["allowed"], "a writable directory");
    assert_eq!(structured["details"]["line"], "3");
}

/// The anchor rules in isolation, so the two callers cannot drift apart.
#[test]
fn lut_error_field_anchors_only_at_a_field_boundary() {
    assert_eq!(
        lut_error_field_start("observed=x", "observed", true),
        Some(0)
    );
    assert_eq!(lut_error_field_start("observed=x", "observed", false), None);
    assert_eq!(
        lut_error_field_start("a; observed=x", "observed", false),
        Some(3)
    );
    // `baseline` is not a `line` field, at either anchor.
    assert_eq!(lut_error_field_start("baseline=x", "line", true), None);
    assert_eq!(lut_error_field_start("a; baseline=x", "line", false), None);
    // A leading detail sentence survives a value that starts with a key.
    assert_eq!(
        lut_error_detail("observed line 1 2 3 4; allowed three; line 12"),
        "observed line 1 2 3 4; allowed three; line 12",
        "a remainder that leads with a key has no detail sentence of its own"
    );
}

/// CC4 §8: `AddLutAsset` is blocked in all four places, exactly as
/// `RelinkAsset` is, because only `import_lut_asset` can write the store.
#[test]
fn cc4_add_lut_asset_is_never_reachable_through_a_plan_or_generated_tool() {
    assert!(
        !operation_tools()
            .unwrap()
            .iter()
            .any(|definition| definition.tool.name == "add_lut_asset"),
        "the generated add_lut_asset operation tool must not exist"
    );
    assert!(crate::schema::UNGENERATED_OPERATION_VARIANTS.contains(&"AddLutAsset"));

    let (core, playback, analysis) = fixture();
    let service = KinewrightMcp::new(core, playback, analysis, ConfirmationBroker::default());
    let asset = kinewright_media::BuiltinLook::Warm.to_lut_asset(kinewright_core::LutAssetId(1));
    let operations = vec![Operation::AddLutAsset {
        asset: asset.clone(),
    }];

    let applied = service
        .apply_edit_plan(TimelineRevision(0), &operations)
        .unwrap();
    assert_eq!(applied.is_error, Some(true));
    assert!(
        applied.content[0]
            .as_text()
            .unwrap()
            .text
            .contains("import_lut_asset")
    );

    let (revision, document) = service.snapshot().unwrap();
    let prepared =
        PreparedPlanStore::default().prepare_operations(revision, revision, &document, operations);
    let error = prepared.expect_err("a prepared plan cannot register a LUT asset");
    assert!(error.to_string().contains("import_lut_asset"), "{error}");

    // The dispatcher refuses the name outright rather than reporting an
    // unknown tool, so the recovery path is stated.
    let dispatched = service
        .call_blocking(CallToolRequestParams::new("add_lut_asset").with_arguments(
            serde_json::Map::from_iter([("expected_revision".to_owned(), serde_json::json!(0))]),
        ))
        .unwrap();
    assert_eq!(dispatched.is_error, Some(true));
    assert!(
        dispatched.content[0]
            .as_text()
            .unwrap()
            .text
            .contains("import_lut_asset")
    );
    assert!(document.lut_assets.is_empty());
}

/// CC4 §8 / M36: the two LUT descriptors never enumerate the `2^53` asset
/// id range, and both planner tools stay well under a kilobyte.
#[test]
fn cc4_lut_tool_descriptors_stay_compact() {
    for kind in [ColorNodeKind::TechnicalLut, ColorNodeKind::CreativeLook] {
        let summary = lut_node_parameter_summary(kind);
        assert!(
            summary.len() < 1_024,
            "{} summary is {} bytes",
            kind.effect_name(),
            summary.len()
        );
        assert!(summary.contains("see list_look_assets"));
        assert!(!summary.contains("9007199254740991"));
        assert!(summary.contains("0 display709, 1 linear, 2 grade709"));
    }
    let registry = KinewrightMcp::capability_tools().unwrap();
    for name in ["plan_technical_lut", "plan_creative_look"] {
        let tool = registry
            .iter()
            .find(|tool| tool.name == name)
            .unwrap_or_else(|| panic!("{name} must be an internal capability"));
        let description = tool.description.as_deref().unwrap_or_default();
        assert!(
            description.len() < 1_024,
            "{name} description is {} bytes",
            description.len()
        );
        assert!(!description.contains("9007199254740991"));
        assert_eq!(
            tool.annotations.as_ref().unwrap().read_only_hint,
            Some(true)
        );
    }
    // The generated effect documentation shares the compact form, so the
    // range never reaches an AddEffect/SetEffectParam schema either.
    let add_effect = operation_tools()
        .unwrap()
        .into_iter()
        .find(|definition| definition.tool.name == "add_effect")
        .unwrap();
    let description = add_effect.tool.description.as_deref().unwrap_or_default();
    assert!(description.contains("lut_asset_id (project LUT asset id; see list_look_assets"));
    assert!(!description.contains("9007199254740991"));
}

/// CC4 §8, §9: the CC4 tools join the internal registry as read-only
/// planners/inspectors plus two confirmed destructive actions, and none of
/// them reaches the seven-tool served surface.
#[test]
fn cc4_agent_surface_registers_the_look_capabilities() {
    let registry = KinewrightMcp::capability_tools().unwrap();
    let served = KinewrightMcp::served_tools().unwrap();
    let catalog = capabilities(&registry);
    for (name, kind) in [
        ("plan_technical_lut", CapabilityKind::Planner),
        ("plan_creative_look", CapabilityKind::Planner),
        ("list_look_assets", CapabilityKind::Inspector),
        ("import_lut_asset", CapabilityKind::Action),
        // CC4 §9: the hand-written conversion capability replaces the
        // generated `ConvertLegacyLook` tool, whose published batch was
        // unsubmittable whenever it opened with `AddLutAsset`.
        ("convert_legacy_look", CapabilityKind::Action),
    ] {
        assert!(
            registry.iter().any(|tool| tool.name == name),
            "{name} must be an internal capability"
        );
        assert!(
            !served.iter().any(|tool| tool.name == name),
            "{name} must stay off the served surface"
        );
        let descriptor = catalog
            .iter()
            .find(|descriptor| descriptor.name == name)
            .unwrap_or_else(|| panic!("{name} must appear in the capability directory"));
        assert_eq!(descriptor.kind, kind);
    }
    for name in ["import_lut_asset", "convert_legacy_look"] {
        let action = registry.iter().find(|tool| tool.name == name).unwrap();
        let annotations = action.annotations.as_ref().unwrap();
        assert_eq!(annotations.read_only_hint, Some(false), "{name}");
        assert_eq!(annotations.destructive_hint, Some(true), "{name}");
    }
    assert_eq!(
        registry
            .iter()
            .filter(|tool| tool.name == "convert_legacy_look")
            .count(),
        1,
        "the hand-written capability must replace the generated operation tool, not duplicate its name"
    );
    assert!(
        crate::runtime::is_invocable_capability("convert_legacy_look"),
        "conversion must be reachable through the compact dispatcher"
    );
}

/// CC4 §8: `render_color_proof` refuses the argument combinations it
/// cannot answer honestly, and a LUT node is *not* one of them: it is
/// carried all the way to the renderer, which fails on its own terms when
/// it has no lattice.
#[test]
#[allow(clippy::too_many_lines)]
fn cc4_render_color_proof_validates_look_arguments_and_renders_lut_nodes() {
    let (core, playback, analysis) = fixture();
    let service = KinewrightMcp::new(core, playback, analysis, ConfirmationBroker::default());

    let conflict = service
        .render_color_proof(&RenderColorProofArgs {
            expected_revision: TimelineRevision(0),
            clip_id: ClipId(1),
            timecode: TimeCode(1),
            profile_assumption: None,
            parameters: BTreeMap::from([("exposure_milli_stops".to_owned(), 100)]),
            effect_id: Some(EffectId(1)),
            look_comparison: None,
            matte_comparison: None,
        })
        .unwrap();
    assert_eq!(conflict.is_error, Some(true));
    assert_eq!(
        conflict.structured_content.unwrap()["code"],
        "look_proof_parameters_conflict"
    );

    let orphan = service
        .render_color_proof(&RenderColorProofArgs {
            expected_revision: TimelineRevision(0),
            clip_id: ClipId(1),
            timecode: TimeCode(1),
            profile_assumption: None,
            parameters: BTreeMap::new(),
            effect_id: None,
            look_comparison: Some(LookComparison::Bypass),
            matte_comparison: None,
        })
        .unwrap();
    assert_eq!(orphan.is_error, Some(true));
    assert_eq!(
        orphan.structured_content.unwrap()["code"],
        "look_comparison_requires_effect_id"
    );

    // CC3 §5: a CC1 primary has no bypass control, so the bypass variant
    // is refused rather than synthesized with an invalid SetEffectParam.
    let (_, seeded) = service.snapshot().unwrap();
    let mut with_primary = (*seeded).clone();
    with_primary.tracks[0].clips[0].effects = vec![Effect {
        id: EffectId(4),
        name: "primary_correction".to_owned(),
        parameters: BTreeMap::from([("exposure_milli_stops".to_owned(), ParamValue::Integer(250))]),
        keyframes: BTreeMap::new(),
    }];
    with_primary.validate().unwrap();
    let primary_service = KinewrightMcp::new(
        Core::spawn(with_primary).unwrap(),
        Arc::new(NoopMedia::default()),
        Arc::new(NoopMedia::default()),
        ConfirmationBroker::default(),
    );
    let unsupported = primary_service
        .render_color_proof(&RenderColorProofArgs {
            expected_revision: TimelineRevision(0),
            clip_id: ClipId(1),
            timecode: TimeCode(1),
            profile_assumption: None,
            parameters: BTreeMap::new(),
            effect_id: Some(EffectId(4)),
            look_comparison: Some(LookComparison::Bypass),
            matte_comparison: None,
        })
        .unwrap();
    assert_eq!(unsupported.is_error, Some(true));
    let structured = unsupported.structured_content.unwrap();
    assert_eq!(structured["code"], "bypass_unsupported_for_node");
    assert_eq!(
        structured["details"]["allowed"],
        serde_json::json!(["before", "after"])
    );

    // A LUT node reaches the renderer like any other managed node.
    let (_, document) = service.snapshot().unwrap();
    let mut with_look = (*document).clone();
    with_look.lut_assets =
        vec![kinewright_media::BuiltinLook::Warm.to_lut_asset(kinewright_core::LutAssetId(1))];
    with_look.tracks[0].clips[0].effects = vec![Effect {
        id: EffectId(9),
        name: "creative_look".to_owned(),
        parameters: BTreeMap::from([("lut_asset_id".to_owned(), ParamValue::Integer(1))]),
        keyframes: BTreeMap::new(),
    }];
    with_look
        .validate()
        .expect("the CC4 stack is a valid document");
    let look_service = KinewrightMcp::new(
        Core::spawn(with_look).unwrap(),
        Arc::new(NoopMedia::default()),
        Arc::new(NoopMedia::default()),
        ConfirmationBroker::default(),
    );
    let refused = look_service
        .render_color_proof(&RenderColorProofArgs {
            expected_revision: TimelineRevision(0),
            clip_id: ClipId(1),
            timecode: TimeCode(1),
            profile_assumption: None,
            parameters: BTreeMap::new(),
            effect_id: Some(EffectId(9)),
            look_comparison: Some(LookComparison::Bypass),
            matte_comparison: None,
        })
        .unwrap();
    // The LUT node is no longer refused up front. The proof proceeds to
    // the renderer, and the `NoopMedia` double has no decoder, so the
    // failure is the render-stage error that double produces - named
    // exactly, not asserted by exclusion.
    assert_eq!(refused.is_error, Some(true));
    let structured = refused.structured_content.unwrap();
    assert_eq!(
        structured["code"], "needs_color_override",
        "the fixture's default source description is what stops this proof, \
         not the LUT node: {structured}"
    );
}
