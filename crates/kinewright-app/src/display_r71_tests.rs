use super::*;
use eframe::egui_wgpu::wgpu;

#[allow(clippy::too_many_lines)]
fn rendered(app: &mut KinewrightApp, ctx: &egui::Context) -> Vec<u8> {
    let state = app.native_display.as_ref().unwrap().renderer.clone();
    let target = state.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("R71 rendered transition"),
        size: wgpu::Extent3d {
            width: 64,
            height: 64,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: state.target_format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(64.0, 64.0));
    let output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(rect),
            ..Default::default()
        },
        |ui| {
            let epoch = ui.ctx().cumulative_frame_nr_for(egui::ViewportId::ROOT);
            app.native_display.as_mut().unwrap().session.advance(epoch);
            let (id, _) = app.program_picture().unwrap();
            ui.painter().image(
                id,
                rect,
                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
            app.presenter
                .marker(epoch, app.paint_clock())
                .add_to(ui.painter(), rect);
            app.finalize_preview(ui.ctx());
        },
    );
    let descriptor = eframe::egui_wgpu::ScreenDescriptor {
        size_in_pixels: [64, 64],
        pixels_per_point: output.pixels_per_point,
    };
    let primitives = ctx.tessellate(output.shapes, output.pixels_per_point);
    let mut renderer = state.renderer.write();
    for (id, delta) in &output.textures_delta.set {
        renderer.update_texture(&state.device, &state.queue, *id, delta);
    }
    let mut encoder = state
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    let mut commands = renderer.update_buffers(
        &state.device,
        &state.queue,
        &mut encoder,
        &primitives,
        &descriptor,
    );
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());
    {
        let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        renderer.render(&mut pass.forget_lifetime(), &primitives, &descriptor);
    }
    let readback = state.device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 64 * 64 * 4,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        target.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(256),
                rows_per_image: Some(64),
            },
        },
        target.size(),
    );
    commands.push(encoder.finish());
    let index = state.queue.submit(commands);
    let (tx, rx) = std::sync::mpsc::channel();
    readback.slice(..).map_async(wgpu::MapMode::Read, move |r| {
        tx.send(r).unwrap();
    });
    state
        .device
        .poll(wgpu::PollType::Wait {
            submission_index: Some(index),
            timeout: None,
        })
        .unwrap();
    rx.recv().unwrap().unwrap();
    let bytes = readback.get_mapped_range(..).to_vec();
    readback.unmap();
    for id in &output.textures_delta.free {
        renderer.free_texture(id);
    }
    bytes
}

fn route_candidate(
    app: &mut KinewrightApp,
    engine: &FfmpegMediaEngine,
    gpu_route: bool,
    color: [u8; 3],
    at: i64,
) -> kinewright_core::FrameStamp {
    let mut document = (*app.focused().document).clone();
    if !gpu_route {
        document.color_context.monitoring.provenance =
            kinewright_core::ColorProvenance::UserOverride;
    }
    Operation::SetSolidColor {
        clip: document.tracks[0].clips[0].id,
        color: kinewright_core::SolidColor {
            r: color[0],
            g: color[1],
            b: color[2],
        },
    }
    .apply(&mut document)
    .unwrap();
    engine.set_document(Arc::new(document));
    engine.seek(TimeCode(at));
    let stamp = engine.stamp();
    if gpu_route {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let session = &app.native_display.as_ref().unwrap().session;
            if let Some(frame) = session.reserve() {
                let ready = frame.stamp == stamp;
                session.defer(frame);
                if ready {
                    break;
                }
            }
            assert!(
                Instant::now() < deadline,
                "GPU transition Ready publication"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    } else {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if let Ok(frame) = app.frames.try_recv()
                && frame.stamp == stamp
            {
                app.presenter.collect(frame);
                break;
            }
            assert!(Instant::now() < deadline, "CPU transition publication");
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    stamp
}

#[test]
fn r71_rendered_monitoring_transitions_preserve_paint_and_scrub_identity() {
    let (mut app, engine, gpu, ctx, _temp) = display_app();
    let mut previous = rendered(&mut app, &ctx);
    assert!(
        previous.iter().any(|v| *v != 0),
        "initial image was actually rendered"
    );
    let mut outcomes = Vec::new();
    for (gpu_route, color, at) in [(false, [30, 160, 80], 1), (true, [40, 70, 220], 2)] {
        let stamp = route_candidate(&mut app, &engine, gpu_route, color, at);
        app.pending_resume = Some((stamp, TimeCode(at)));
        let transition = rendered(&mut app, &ctx);
        app.acknowledge_paint(ctx.cumulative_frame_nr_for(egui::ViewportId::ROOT));
        println!(
            "R71_F1 to_gpu={gpu_route} laid_out_pixels_preserved={} scrub_pending={}",
            transition == previous,
            app.pending_resume.is_some()
        );
        outcomes.push((
            gpu_route,
            transition == previous,
            app.pending_resume.is_some(),
        ));
        engine.pause();
        app.pending_resume = None;
        previous = rendered(&mut app, &ctx);
        assert_ne!(
            previous, transition,
            "the following paint must contain the new distinct-colour image"
        );
    }
    finish(app, engine, &gpu);
    assert!(
        outcomes
            .iter()
            .all(|(_, pixels, pending)| *pixels && *pending),
        "both rendered transitions retain their laid-out pixels and cannot resume an unpainted target: {outcomes:?}"
    );
}

fn live_registrations(
    app: &KinewrightApp,
    known: &mut std::collections::HashSet<egui::TextureId>,
) -> usize {
    if let Some(texture) = &app.texture {
        known.insert(texture.id());
    }
    if let Some((id, _)) = app.native_display.as_ref().unwrap().picture {
        known.insert(id);
    }
    let renderer = app
        .native_display
        .as_ref()
        .unwrap()
        .renderer
        .renderer
        .read();
    known
        .iter()
        .filter(|id| renderer.texture(id).is_some())
        .count()
}
fn flush_freed(ctx: &egui::Context, state: &eframe::egui_wgpu::RenderState) {
    let output = ctx.run_ui(egui::RawInput::default(), |_| {});
    let mut renderer = state.renderer.write();
    for id in output.textures_delta.free {
        renderer.free_texture(&id);
    }
}
#[test]
fn r72_route_registrations_are_bounded_and_clear_or_teardown_releases_all() {
    for clear in [true, false] {
        let (mut app, engine, gpu, ctx, _temp) = display_app();
        rendered(&mut app, &ctx);
        let mut known = std::collections::HashSet::new();
        let mut counts = vec![live_registrations(&app, &mut known)];
        for (gpu_route, color, at) in [
            (false, [30, 160, 80], 1),
            (true, [40, 70, 220], 2),
            (false, [200, 40, 90], 3),
        ] {
            route_candidate(&mut app, &engine, gpu_route, color, at);
            rendered(&mut app, &ctx);
            counts.push(live_registrations(&app, &mut known));
            rendered(&mut app, &ctx);
            counts.push(live_registrations(&app, &mut known));
        }
        let state = app.native_display.as_ref().unwrap().renderer.clone();
        if clear {
            app.clear_preview();
            flush_freed(&ctx, &state);
            assert_eq!(
                live_registrations(&app, &mut known),
                0,
                "clear releases CPU and native registrations"
            );
        }
        finish(app, engine, &gpu);
        flush_freed(&ctx, &state);
        let remaining = known
            .iter()
            .filter(|id| state.renderer.read().texture(id).is_some())
            .count();
        println!(
            "R72_BOUND clear={clear} route_counts={counts:?} remaining_after_teardown={remaining}"
        );
        assert!(
            counts.iter().all(|count| *count <= 2),
            "at most one CPU and one native registration"
        );
        assert!(
            counts.contains(&2),
            "both retained routes were actually registered"
        );
        assert_eq!(
            remaining, 0,
            "teardown releases every observed registration"
        );
    }
}
