//! Runtime witnesses for owned inspection and the uncached QC handoff.
use super::*;
use crate::app::in1_tests::{in1_harness, in1_shutdown};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use kinewright_media::test_support::{TempDirectory, workspace_target_dir};
use rmcp::{
    ServiceExt as _, model::CallToolRequestParams, transport::StreamableHttpClientTransport,
};

struct OwnedFixture {
    app: KinewrightApp,
    engine: Arc<FfmpegMediaEngine>,
    path: PathBuf,
    locator: PathBuf,
    _retire: Retire,
    _root: TempDirectory,
}

struct Retire(crossbeam_channel::Receiver<()>);

impl Drop for Retire {
    fn drop(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if matches!(
                self.0.try_recv(),
                Err(crossbeam_channel::TryRecvError::Disconnected)
            ) {
                break;
            }
            assert!(Instant::now() < deadline, "media worker did not retire");
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

impl Drop for OwnedFixture {
    fn drop(&mut self) {
        in1_shutdown(&mut self.app);
        for project in &mut self.app.projects {
            project.studio.manual_inputs = None;
            project.studio.selection = None;
        }
        self.engine
            .unadmitted_media(Arc::new(Document::default()))
            .set_document(Arc::new(Document::default()));
        let _ = self.engine.test_applied_scene();
        let _ = std::fs::remove_file(&self.locator);
        for project in &mut self.app.projects {
            if let Some(attachment) = &mut project.ja1 {
                let _ = attachment.close();
            }
        }
    }
}

fn fixture() -> OwnedFixture {
    let root = TempDirectory::new_in(&workspace_target_dir(), "c2-fix2-owned");
    let [red, blue] = kinewright_project::c0_fixtures::build_c0_fixture_pair(root.root()).unwrap();
    let locator = red.history.document.media_pool[0].path.clone();
    assert!(
        !locator.exists(),
        "the generator retired its external source"
    );
    let replacement = blue
        .path
        .with_extension("kinewright-assets")
        .join("studio/objects")
        .join(&blue.media.entry.object.sha256);
    std::fs::copy(replacement, &locator).unwrap();
    let path = red.path;
    let (mut app, engine) = in1_harness(Document::default());
    let retire = Retire(engine.teardown_observer());
    app.open_project(&path);
    OwnedFixture {
        app,
        engine,
        path,
        locator,
        _retire: retire,
        _root: root,
    }
}

#[derive(Debug)]
enum Observed {
    Pixel([u8; 4]),
    Refused,
}

fn pixel(bytes: &[u8], width: usize, height: usize) -> [u8; 4] {
    bytes[((height / 2) * width + width / 2) * 4..][..4]
        .try_into()
        .unwrap()
}

fn red(observed: &Observed) -> bool {
    match observed {
        Observed::Refused => true,
        Observed::Pixel([r, g, b, a]) => *r > 200 && *g < 30 && *b < 30 && *a == 255,
    }
}

fn analysis_observation(result: Result<kinewright_core::RgbaImage, MediaError>) -> Observed {
    match result {
        Ok(image) => Observed::Pixel(pixel(
            &image.pixels,
            image.width as usize,
            image.height as usize,
        )),
        Err(MediaError::MediaAdmissionRequired) => Observed::Refused,
        Err(error) => panic!("unexpected analysis setup failure: {error}"),
    }
}

fn program(fixture: &OwnedFixture) -> Observed {
    let controls = fixture.engine.controls();
    let frames = controls.frames();
    let events = controls.events();
    controls.request_frame(TimeCode::ZERO);
    let stamp = controls.stamp();
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        while let Ok(event) = events.try_recv() {
            if event == MediaEvent::Error(MediaError::MediaAdmissionRequired) {
                return Observed::Refused;
            }
            if matches!(event, MediaEvent::StampedError(found, MediaError::MediaAdmissionRequired) if found.is_current(stamp))
            {
                return Observed::Refused;
            }
        }
        while let Ok(frame) = frames.try_recv() {
            if frame.stamp.is_current(stamp) {
                let value = pixel(
                    &frame.texture.rgba,
                    frame.texture.width as usize,
                    frame.texture.height as usize,
                );
                if value[0] > 200 || value[2] > 200 {
                    return Observed::Pixel(value);
                }
            }
        }
        assert!(
            Instant::now() < deadline,
            "program produced neither certified pixels nor its named refusal"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn branch(app: &mut KinewrightApp) -> Observed {
    let ctx = egui::Context::default();
    let index = app.focused_project;
    let document = app.projects[index].threads[0]
        .branch
        .compare()
        .unwrap()
        .document;
    app.review_agent_branch(&ctx, index, 0, document);
    let deadline = Instant::now() + Duration::from_secs(30);
    let reply = loop {
        if let Ok(reply) = app.ui_replies.1.try_recv() {
            break reply;
        }
        assert!(Instant::now() < deadline, "branch reply timed out");
        std::thread::sleep(Duration::from_millis(5));
    };
    reply(app, &ctx);
    if let Some(texture) = &app.texture {
        let id = texture.id();
        let output = ctx.run_ui(egui::RawInput::default(), |_| {});
        let (_, upload) = output
            .textures_delta
            .set
            .iter()
            .find(|(texture, _)| *texture == id)
            .unwrap();
        let egui::ImageData::Color(image) = &upload.image;
        Observed::Pixel(
            image.pixels[(image.size[1] / 2) * image.size[0] + image.size[0] / 2].to_array(),
        )
    } else {
        assert!(
            format!("{:?}", app.pending_observations).contains("owned_media_admission_required"),
            "branch must retain its named refusal"
        );
        Observed::Refused
    }
}

fn mcp(app: &KinewrightApp) -> Observed {
    let endpoint = app.focused().threads[0]
        .mcp_server
        .as_ref()
        .unwrap()
        .endpoint()
        .to_owned();
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let client = ().serve(StreamableHttpClientTransport::from_uri(endpoint)).await.unwrap();
            let result = client
                .call_tool(
                    CallToolRequestParams::new("invoke_capability").with_arguments(
                        serde_json::json!({"name": "get_frame_at", "arguments": {"timecode": 0}})
                            .as_object()
                            .unwrap()
                            .clone(),
                    ),
                )
                .await
                .unwrap();
            let observed = if result.is_error == Some(true) {
                let changed = result.structured_content.as_ref().is_some_and(|details| {
                    details["availability"]["kind"] == "changed"
                        && details["consumer"] == "frame proof"
                });
                assert!(
                    format!("{result:?}").contains("owned_media_admission_required") || changed,
                    "local endpoint must return the named refusal: {result:?}"
                );
                Observed::Refused
            } else {
                let image = result
                    .content
                    .iter()
                    .find_map(|content| content.as_image())
                    .unwrap();
                let bytes = STANDARD.decode(&image.data).unwrap();
                let image = image::load_from_memory(&bytes).unwrap().to_rgba8();
                Observed::Pixel(pixel(
                    image.as_raw(),
                    image.width() as usize,
                    image.height() as usize,
                ))
            };
            client.cancel().await.unwrap();
            observed
        })
}

#[test]
fn c2_r1_file_open_before_activation_never_decodes_the_locator() {
    let mut fixture = fixture();
    let project = fixture.app.focused();
    assert!(project.source_admission.is_studio());
    assert!(
        project.ja1.is_none()
            && project.ja1_job.is_none()
            && project.studio.manual_inputs.is_none()
    );
    let program = program(&fixture);
    let document = Arc::clone(&fixture.app.focused().document);
    let analysis = analysis_observation(fixture.app.focused().analysis().thumbnail_for_document(
        document,
        TimeCode::ZERO,
        32,
    ));
    let branch = branch(&mut fixture.app);
    let mcp = mcp(&fixture.app);
    let observed = [program, analysis, branch, mcp];
    eprintln!("C2_R1_OWNED_OPEN_NO_LOCATOR: {observed:?}");
    assert!(
        observed.iter().all(red),
        "C2_R1_OWNED_OPEN_NO_LOCATOR: {observed:?}"
    );
}

#[test]
fn c2_r2_uncached_owned_colour_qc_keeps_its_certified_inputs() {
    let mut fixture = fixture();
    fixture.app.focused_mut().ja1 =
        Some(crate::ja1_attachment::Ja1Attachment::open(&fixture.path).unwrap());
    fixture.app.focused_mut().install_ja1_view();
    let ctx = egui::Context::default();
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        fixture.app.poll_background(&ctx);
        if fixture.app.focused().ja1.is_some()
            && fixture.app.focused().ja1_job.is_none()
            && fixture.app.focused().studio.manual_inputs.is_some()
        {
            break;
        }
        assert!(Instant::now() < deadline, "manual admission did not finish");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(
        fixture.app.working_proof_cache.cached_proof().is_none(),
        "the measurement must miss the cache"
    );
    let id = fixture.app.focused().id;
    let revision = fixture.app.focused().revision;
    fixture
        .app
        .request_color_qc_measurement(id, revision.0, TimeCode::ZERO, None, None);
    let deadline = Instant::now() + Duration::from_secs(30);
    while fixture.app.color_qc.is_pending() {
        fixture.app.color_qc.poll(revision);
        assert!(Instant::now() < deadline, "QC measurement timed out");
        std::thread::sleep(Duration::from_millis(5));
    }
    let proof = fixture
        .app
        .working_proof_cache
        .cached_proof()
        .unwrap_or_else(|| panic!("C2_R2_QC_INPUT_HANDOFF: {:?}", fixture.app.color_qc.error()));
    assert!(
        fixture.app.color_qc.current().is_some(),
        "owned QC stays usable: {:?}",
        fixture.app.color_qc.error()
    );
    let offset = ((proof.image.height as usize / 2) * proof.image.width as usize
        + proof.image.width as usize / 2)
        * 4;
    let value = &proof.image.pixels[offset..offset + 4];
    eprintln!("C2_R2_QC_CERTIFIED_RED: {value:?}");
    assert!(
        value[0] > 0.8 && value[1] < 0.05 && value[2] < 0.05,
        "C2_R2_QC_CERTIFIED_RED: {value:?}"
    );
}

#[test]
fn c2_raw_backend_document_routes_are_named_refusals() {
    let fixture = fixture();
    let document = Arc::clone(&fixture.app.focused().document);
    assert_eq!(
        fixture
            .engine
            .thumbnail_for_document(Arc::clone(&document), TimeCode::ZERO, 32),
        Err(MediaError::MediaAdmissionRequired)
    );
    assert_eq!(
        fixture
            .engine
            .working_proof_for_document(document, TimeCode::ZERO),
        Err(MediaError::MediaAdmissionRequired)
    );
}
