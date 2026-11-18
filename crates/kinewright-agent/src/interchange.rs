use kinewright_core::{ImportRecord, LossCode, Rational, TimelineRevision};
use kinewright_project::interchange::{
    ImportOptions, InterchangeInspection, Refusal, RefusalCode, SiteQuery, inspect,
};
use rmcp::model::{CallToolResult, Tool};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use super::{error_structured, read_only_annotations, schema_object, success_structured};

const INSPECT_BYTES: usize = 6 * 1024;
const MISSING_LIMIT: usize = 8;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct InspectArgs {
    path: String,
    #[schemars(length(max = 8))]
    media_roots: Option<Vec<String>>,
    project_fps: Option<Rational>,
    resolution: Option<[u32; 2]>,
    timeline: Option<String>,
    code: Option<String>,
    after: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct ReportArgs {
    index: Option<u32>,
}

pub(super) fn tools() -> [Tool; 2] {
    [
        Tool::new(
            "inspect_interchange",
            "Inspect a local OTIO or FCPXML timeline, including supported bundles, without opening a project. Optional media roots search for missing filenames. Inspection stats media and estimates its metadata. With code, return a page of loss sites after the previous cursor.",
            schema_object::<InspectArgs>(),
        )
        .with_annotations(read_only_annotations()),
        Tool::new(
            "get_import_report",
            "Read one persisted interchange import record at the current timeline revision. The optional zero-based index counts from oldest to newest. Omit it for the newest record.",
            schema_object::<ReportArgs>(),
        )
        .with_annotations(read_only_annotations()),
    ]
}

pub(super) fn import_report(
    args: &ReportArgs,
    revision: TimelineRevision,
    records: &[ImportRecord],
) -> CallToolResult {
    let count = records.len();
    let index = args
        .index
        .map_or_else(|| count.checked_sub(1), |index| usize::try_from(index).ok());
    let record = index.and_then(|index| records.get(index));
    if args.index.is_some() && record.is_none() {
        return refusal_reply(Refusal::new(
            RefusalCode::NotFound,
            "The requested import record does not exist.",
        ));
    }
    success_structured(
        "Interchange import report",
        json!({"revision": revision, "count": count, "index": index, "record": record}),
    )
}

pub(super) fn inspect_timeline(args: InspectArgs) -> CallToolResult {
    let after = args.after.unwrap_or_default();
    let paged = args.code.is_some();
    match inspect_request(args, after) {
        Ok(inspection) => bounded_reply(inspection, after, paged),
        Err(refusal) => refusal_reply(refusal),
    }
}

fn inspect_request(args: InspectArgs, after: u32) -> Result<InterchangeInspection, Refusal> {
    let query = args
        .code
        .map(|code| {
            let code = serde_json::from_value::<LossCode>(Value::String(code)).map_err(|_| {
                Refusal::new(
                    RefusalCode::Malformed,
                    "The requested loss code is unknown.",
                )
            })?;
            Ok::<_, Refusal>(SiteQuery { code, after })
        })
        .transpose()?;
    let project_fps = args
        .project_fps
        .map(|fps| {
            Rational::new(fps.numerator(), fps.denominator()).map_err(|_| {
                Refusal::new(
                    RefusalCode::UnsupportedRate,
                    "The frame rate must be positive.",
                )
            })
        })
        .transpose()?;
    let options = ImportOptions {
        timeline: args.timeline,
        project_fps,
        resolution: args.resolution.map(|[width, height]| (width, height)),
        media_roots: args.media_roots.unwrap_or_default(),
        ..ImportOptions::default()
    };
    inspect(&args.path, options, query)
}

fn bounded_reply(inspection: InterchangeInspection, after: u32, paged: bool) -> CallToolResult {
    let InterchangeInspection {
        mut summary,
        record,
        media,
        mut sites,
    } = inspection;
    summary
        .timeline
        .truncate(summary.timeline.floor_char_boundary(128));
    let missing = || media.iter().filter(|entry| !entry.status.is_available());
    let count = missing().count();
    let mut examples: Vec<_> = missing()
        .take(MISSING_LIMIT)
        .map(|entry| {
            json!({
                "name": &entry.name[..entry.name.floor_char_boundary(24)],
                "reason": entry.status,
            })
        })
        .collect();
    let mut next = sites.next;
    loop {
        let mut report = json!({
            "summary": summary,
            "record": record,
            "missing": examples,
            "missing_omitted": count - examples.len(),
            "media_probed": false,
        });
        if paged {
            report["sites"] = json!(sites.sites);
            report["next"] = json!(next);
        }
        let reply = success_structured("Interchange inspection", report);
        if reply_bytes(&reply) <= INSPECT_BYTES {
            return reply;
        }
        if sites.sites.len() > 1 {
            sites.sites.pop();
            next = Some(after.saturating_add(u32::try_from(sites.sites.len()).unwrap_or_default()));
        } else if examples.pop().is_none() {
            if let [site] = &mut sites.sites[..]
                && site.source.len() > 128
            {
                site.source.truncate(site.source.floor_char_boundary(128));
            } else {
                return refusal_reply(Refusal::new(
                    RefusalCode::TooLarge,
                    "The interchange inspection exceeds 6 KiB.",
                ));
            }
        }
    }
}

pub(super) fn outside_roots() -> CallToolResult {
    refusal_reply(Refusal::new(
        RefusalCode::OutsideRoots,
        "This session requires identity-pinned file access to inspect a timeline.",
    ))
}

fn refusal_reply(mut refusal: Refusal) -> CallToolResult {
    loop {
        let reply = error_structured(refusal.message.as_str(), json!(refusal));
        if reply_bytes(&reply) <= INSPECT_BYTES {
            return reply;
        }
        if refusal.timelines.pop().is_none() && refusal.at.take().is_none() {
            return error_structured(
                "Interchange inspection refused",
                json!({"code": refusal.code, "message": "The interchange inspection was refused."}),
            );
        }
    }
}

fn reply_bytes(reply: &CallToolResult) -> usize {
    serde_json::to_vec(reply).map_or(usize::MAX, |bytes| bytes.len())
}

#[cfg(test)]
mod tests {
    use std::{path::Path, sync::Arc};

    use kinewright_core::{Core, RefusedMedia};
    use kinewright_media::test_support::TempDirectory;
    use kinewright_project::interchange::{PrepareOptions, commit, prepare};
    use rmcp::model::CallToolRequestParams;

    use super::super::{ConfirmationBroker, KinewrightMcp};
    use super::*;

    #[test]
    fn ix1_interchange_registry_keeps_the_seven_tool_surface() {
        let tools = KinewrightMcp::capability_tools().unwrap();
        let served = KinewrightMcp::served_tools().unwrap();
        let registry_metrics = crate::runtime::ToolSurfaceMetrics::measure(&tools);
        let served_metrics = crate::runtime::ToolSurfaceMetrics::measure(&served);
        println!("registry={registry_metrics:?} served={served_metrics:?}");
        assert_eq!(
            (served_metrics.tool_count, served_metrics.serialized_bytes),
            (7, 5_660)
        );
        for name in ["inspect_interchange", "get_import_report", "relink_media"] {
            let tool = tools.iter().find(|tool| tool.name == name).unwrap();
            let metrics = crate::runtime::ToolSurfaceMetrics::measure(std::slice::from_ref(tool));
            println!("{name} {metrics:?}");
        }
    }

    #[test]
    fn ix1_inspection_keeps_a_forward_cursor_at_the_byte_limit() {
        use kinewright_core::{
            AssetId, Fidelity, ImportRecord, InterchangeFormat, LossRow, LossSite, MediaCounts,
        };
        use kinewright_project::interchange::{ImportSummary, MediaEntry, MediaStatus, SitePage};

        let record = ImportRecord {
            format: InterchangeFormat::Otio,
            format_version: "Timeline.1".into(),
            source_name: "source.otio".into(),
            source_sha256: ImportRecord::hash_source(b"source"),
            start_offset_frames: 0,
            items: 64,
            exact: 0,
            approximated: 0,
            dropped: 64,
            media: MediaCounts {
                assets: 9,
                offline: 9,
                remote: 9,
                ..MediaCounts::default()
            },
            estimated_assets: vec![AssetId(1)..AssetId(10)],
            retimed_source_end_drift_frames: None,
            rows: vec![LossRow {
                code: LossCode::EffectDropped,
                fidelity: Fidelity::Dropped,
                count: 64,
                bound: None,
                names: Vec::new(),
                at: vec![LossSite {
                    source: "\0".repeat(570),
                    frame: 0,
                    clip: None,
                }],
                unverified: false,
            }],
            omitted_codes: Vec::new(),
        };
        record.validate().unwrap();
        let media = (0..9)
            .map(|_| MediaEntry {
                name: "\0".repeat(24),
                locator: "remote".into(),
                status: MediaStatus::Remote,
                resolved_path: None,
                relinked_by_name: false,
                matches: None,
            })
            .collect();
        let after = 40;
        let sites = SitePage {
            sites: (0..64)
                .map(|i| LossSite {
                    source: "\0".repeat(120),
                    frame: i,
                    clip: None,
                })
                .collect(),
            next: Some(after + 64),
        };
        let result = bounded_reply(
            InterchangeInspection {
                summary: ImportSummary {
                    timeline: "\0".repeat(128),
                    fps: Rational::default(),
                    resolution: (64, 36),
                },
                record,
                media,
                sites,
            },
            after,
            true,
        );
        assert!(reply_bytes(&result) <= INSPECT_BYTES);
        let content = result.structured_content.unwrap();
        let emitted = content["sites"].as_array().unwrap().len();
        assert!((1..64).contains(&emitted));
        assert_eq!(
            content["next"],
            json!(after + u32::try_from(emitted).unwrap())
        );
        assert_eq!(
            content["missing_omitted"],
            json!(9 - content["missing"].as_array().unwrap().len())
        );
    }

    #[test]
    fn ix1_interchange_capabilities_dispatch_bounded_inspection_and_read_the_persisted_record() {
        let directory = TempDirectory::new("ix1-agent");
        let path = directory.path("source.otio");
        write_inspection_fixture(&path);
        let prepared = prepare(
            path.to_str().unwrap(),
            PrepareOptions {
                import: ImportOptions::default(),
                allowed_shares: std::collections::BTreeSet::new(),
                analysis: Arc::new(RefusedMedia),
            },
        )
        .unwrap();
        let expected = json!(prepared.record());
        let document = commit(prepared, |_, document, _| Ok::<_, ()>(document)).unwrap();
        let core = Core::spawn(document).unwrap();
        let mut service = KinewrightMcp::new(
            core,
            Arc::new(RefusedMedia),
            Arc::new(RefusedMedia),
            ConfirmationBroker::default(),
        );
        let invoke = |service: &KinewrightMcp, name: &str, arguments: Value| {
            service
                .call_exposed_blocking(
                    CallToolRequestParams::new("invoke_capability").with_arguments(
                        json!({"name":name,"arguments":arguments})
                            .as_object()
                            .unwrap()
                            .clone(),
                    ),
                )
                .unwrap()
        };
        let search = service
            .call_exposed_blocking(
                CallToolRequestParams::new("search_capabilities").with_arguments(
                    json!({"query":"interchange","kinds":["inspector"]})
                        .as_object()
                        .unwrap()
                        .clone(),
                ),
            )
            .unwrap()
            .structured_content
            .unwrap();
        for name in ["inspect_interchange", "get_import_report"] {
            assert!(
                search["capabilities"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|entry| entry["name"] == name)
            );
        }
        let first = invoke(
            &service,
            "inspect_interchange",
            json!({"path":path,"code":"effect_dropped"}),
        );
        assert!(reply_bytes(&first) <= INSPECT_BYTES);
        let first = first.structured_content.unwrap();
        assert_eq!(first["record"], expected);
        assert_eq!(first["media_probed"], false);
        assert_eq!(first["missing"][0]["reason"], "remote");
        let next = first["next"].as_u64().unwrap();
        assert_eq!(next, 64);
        assert_eq!(first["sites"].as_array().unwrap().len(), 64);
        let second = invoke(
            &service,
            "inspect_interchange",
            json!({"path":path,"code":"effect_dropped","after":next}),
        );
        let second = second.structured_content.unwrap();
        assert_eq!(second["sites"].as_array().unwrap().len(), 8);
        assert!(second["next"].is_null());
        let refused = invoke(
            &service,
            "inspect_interchange",
            json!({"path":"file://host/share/source.otio"}),
        );
        assert_eq!(refused.structured_content.unwrap()["code"], "remote_path");

        service.host_scoped = true;
        let refused = invoke(&service, "inspect_interchange", json!({"path":path}));
        assert_eq!(refused.structured_content.unwrap()["code"], "outside_roots");
        let records = invoke(&service, "get_import_report", json!({}))
            .structured_content
            .unwrap();
        assert_eq!(records["record"], expected);
        assert_eq!(records["count"], 1);
        assert_eq!(records["index"], 0);
        assert_eq!(records["revision"], 0);
    }
    #[test]
    fn ix1_pages_maximal_import_records_under_the_reply_budget() {
        use kinewright_core::{
            DOCUMENT_MAX_IMPORT_RECORDS, Document, Fidelity, IMPORT_RECORD_MAX_BYTES,
            InterchangeFormat, LossRow, LossSite, MediaCounts,
        };

        let maximal = |index| {
            let mut record = ImportRecord {
                format: InterchangeFormat::Otio,
                format_version: "Timeline.1".into(),
                source_name: format!("edit-{index}.otio"),
                source_sha256: ImportRecord::hash_source(b"source"),
                start_offset_frames: 0,
                items: 1,
                exact: 0,
                approximated: 0,
                dropped: 1,
                media: MediaCounts::default(),
                estimated_assets: Vec::new(),
                retimed_source_end_drift_frames: None,
                rows: vec![LossRow {
                    code: LossCode::EffectDropped,
                    fidelity: Fidelity::Dropped,
                    count: 1,
                    bound: None,
                    unverified: false,
                    names: Vec::new(),
                    at: vec![LossSite {
                        source: String::new(),
                        frame: 0,
                        clip: None,
                    }],
                }],
                omitted_codes: Vec::new(),
            };
            let needed = IMPORT_RECORD_MAX_BYTES - serde_json::to_vec(&record).unwrap().len();
            record.rows[0].at[0].source = "x".repeat(needed);
            assert_eq!(
                serde_json::to_vec(&record).unwrap().len(),
                IMPORT_RECORD_MAX_BYTES
            );
            record
        };
        let document = Document {
            imported_from: (0..DOCUMENT_MAX_IMPORT_RECORDS).map(maximal).collect(),
            ..Document::default()
        };
        let core = Core::spawn(document).unwrap();
        let service = KinewrightMcp::new(
            core,
            Arc::new(RefusedMedia),
            Arc::new(RefusedMedia),
            ConfirmationBroker::default(),
        );
        let dispatch = |arguments: Value| {
            service
                .call_exposed_blocking(
                    CallToolRequestParams::new("invoke_capability").with_arguments(
                        json!({"name":"get_import_report","arguments":arguments})
                            .as_object()
                            .unwrap()
                            .clone(),
                    ),
                )
                .unwrap()
        };
        let newest = dispatch(json!({}));
        assert!(reply_bytes(&newest) <= INSPECT_BYTES);
        let newest = newest.structured_content.unwrap();
        assert_eq!(newest["index"], 15);
        assert_eq!(newest["count"], 16);
        assert_eq!(newest["record"]["source_name"], "edit-15.otio");
        for index in 0..16 {
            let response = dispatch(json!({"index":index}));
            assert!(reply_bytes(&response) <= INSPECT_BYTES);
            let response = response.structured_content.unwrap();
            assert_eq!(response["index"], index);
            assert_eq!(
                response["record"]["source_name"],
                format!("edit-{index}.otio")
            );
        }
        let out_of_range = dispatch(json!({"index":16}));
        assert_eq!(out_of_range.is_error, Some(true));
        assert_eq!(
            out_of_range.structured_content.unwrap()["code"],
            "not_found"
        );
        let empty = import_report(&ReportArgs { index: None }, TimelineRevision(1), &[]);
        assert_eq!(
            empty.structured_content.unwrap(),
            json!({
                "revision": 1, "count": 0, "index": null, "record": null,
            })
        );
    }

    fn write_inspection_fixture(path: &Path) {
        let clip = json!({
            "OTIO_SCHEMA": "Clip.1",
            "source_range": {"start_time":{"value":0,"rate":24},"duration":{"value":24,"rate":24}},
            "media_reference": {"OTIO_SCHEMA":"ExternalReference.1","target_url":"https://example.invalid/a.mov"},
            "effects": [{"OTIO_SCHEMA":"Effect.1","name":"Unmapped effect"}],
        });
        std::fs::write(
            path,
            serde_json::to_vec(&json!({
                "OTIO_SCHEMA":"Timeline.1",
                "name":"Report",
                "global_start_time":{"value":0,"rate":24},
                "tracks":{"OTIO_SCHEMA":"Stack.1","children":[
                    {"OTIO_SCHEMA":"Track.1","kind":"Video","children":vec![clip; 72]}
                ]},
            }))
            .unwrap(),
        )
        .unwrap();
    }
}
