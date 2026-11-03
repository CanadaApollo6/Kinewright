use kinewright_agent::McpServer;
use kinewright_core::{Core, Document, Marker, MarkerId, TimeCode};
use rmcp::{ServiceExt, model::CallToolRequestParams, transport::StreamableHttpClientTransport};
use std::sync::Arc;

#[test]
fn te1_result_bytes_match_the_result_received_over_mcp() {
    let temp = kinewright_media::test_support::TempDirectory::new_in(
        &kinewright_media::test_support::workspace_target_dir(),
        "te1-results",
    );
    let engine = Arc::new(
        kinewright_media::FfmpegMediaEngine::new_with_data_dir(temp.path("cache"))
            .unwrap()
            .into_legacy_fixture(),
    );
    let mut document = Document::default();
    document.markers.push(Marker {
        id: MarkerId(1),
        position: TimeCode(0),
        label: "arbitrary body never retained by size observer".into(),
        color_token: 0,
    });
    let server = McpServer::start(Core::spawn(document).unwrap(), engine.clone(), engine).unwrap();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let serialized = runtime.block_on(async {
        let client =
            ().serve(StreamableHttpClientTransport::from_uri(server.endpoint()))
                .await
                .unwrap();
        let result = client
            .call_tool(CallToolRequestParams::new("get_timeline_state"))
            .await
            .unwrap();
        let bytes = serde_json::to_vec(&result).unwrap();
        client.cancel().await.unwrap();
        bytes
    });
    let metrics = server.result_sizes();
    assert_eq!(metrics.requests, 1);
    assert_eq!(metrics.count, 1);
    assert_eq!(metrics.results[0].bytes, serialized.len() as u64);
    assert_eq!(metrics.bytes, serialized.len() as u64);
    assert!(!metrics.incomplete);
    assert!(
        !serde_json::to_string(&metrics)
            .unwrap()
            .contains("arbitrary body")
    );
}
