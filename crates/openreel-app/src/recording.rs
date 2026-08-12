//! In-editor recording (M26): screen, camera, and voice capture.
//!
//! Capture runs the bundled `FFmpeg` CLI as a subprocess - the same
//! drive-the-installed-tool pattern as the agent harnesses. A capture crash
//! can never take the editor down, stopping is `FFmpeg`'s own graceful `q`,
//! and the finished file flows through the ordinary import pipeline: probe,
//! add to the media pool, land on the timeline, cue the monitor, and
//! transcribe - so a recording is text-editable the moment it stops.
//!
//! The agent deliberately has no capture tool: recording is a human act.

use std::{
    io::Write,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::Arc,
    time::{Duration, Instant},
};

use eframe::egui;

use crate::{
    app::OpenReelApp,
    icons::Icon,
    theme::{color, size, space, type_size},
};

/// Capture devices `FFmpeg`'s `DirectShow` backend reports, plus the
/// displays Windows reports for screen capture.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct CaptureDevices {
    pub(crate) video: Vec<String>,
    pub(crate) audio: Vec<String>,
    pub(crate) monitors: Vec<MonitorInfo>,
}

/// One display in virtual-desktop coordinates, exactly as Windows reports
/// them - `gdigrab` takes these raw (negative offsets included, for
/// displays left of or above the primary).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MonitorInfo {
    pub(crate) label: String,
    pub(crate) x: i32,
    pub(crate) y: i32,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) primary: bool,
}

/// What one recording captures. Microphones are `DirectShow` device names;
/// a screen capture with no monitor grabs the whole virtual desktop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RecordingMode {
    Screen {
        microphone: Option<String>,
        monitor: Option<MonitorInfo>,
    },
    Camera {
        camera: String,
        microphone: Option<String>,
    },
    Voice {
        microphone: String,
    },
}

impl RecordingMode {
    pub(crate) fn label(&self) -> &'static str {
        match self {
            Self::Screen { .. } => "Screen",
            Self::Camera { .. } => "Camera",
            Self::Voice { .. } => "Voice",
        }
    }

    /// Audio-only recordings land in an audio container.
    fn extension(&self) -> &'static str {
        match self {
            Self::Screen { .. } | Self::Camera { .. } => "mp4",
            Self::Voice { .. } => "m4a",
        }
    }
}

/// A capture in progress: the `FFmpeg` child writing `path`.
pub(crate) struct ActiveRecording {
    child: Child,
    pub(crate) path: PathBuf,
    pub(crate) started: Instant,
    pub(crate) label: &'static str,
}

/// The record dialog's UI state.
#[derive(Default)]
pub(crate) struct RecordDialog {
    pub(crate) open: bool,
    pub(crate) devices: Option<CaptureDevices>,
    pub(crate) devices_rx: Option<std::sync::mpsc::Receiver<CaptureDevices>>,
    pub(crate) source: RecordSource,
    pub(crate) camera: Option<String>,
    pub(crate) microphone: Option<String>,
    /// `None` records the whole virtual desktop (every display).
    pub(crate) monitor: Option<MonitorInfo>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RecordSource {
    #[default]
    Screen,
    Camera,
    Voice,
}

impl ActiveRecording {
    /// Seconds since capture started, for the top-bar REC chip.
    pub(crate) fn elapsed_label(&self) -> String {
        let seconds = self.started.elapsed().as_secs();
        format!("{:02}:{:02}", seconds / 60, seconds % 60)
    }

    /// Whether the `FFmpeg` child exited on its own (a capture failure).
    pub(crate) fn exited(&mut self) -> Option<std::process::ExitStatus> {
        self.child.try_wait().ok().flatten()
    }

    /// Graceful stop: `FFmpeg`'s interactive `q` writes a valid trailer; a
    /// kill is only the fallback so the file survives whenever possible.
    pub(crate) fn stop(mut self) -> Result<PathBuf, String> {
        if let Some(stdin) = self.child.stdin.as_mut() {
            let _ = stdin.write_all(b"q");
            let _ = stdin.flush();
        }
        drop(self.child.stdin.take());
        let deadline = Instant::now() + Duration::from_secs(8);
        let status = loop {
            match self.child.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(50));
                }
                Ok(None) => {
                    let _ = self.child.kill();
                    let _ = self.child.wait();
                    break None;
                }
                Err(error) => return Err(format!("could not wait for FFmpeg: {error}")),
            }
        };
        let wrote_file = self
            .path
            .metadata()
            .is_ok_and(|metadata| metadata.len() > 0);
        if !wrote_file {
            return Err(match status {
                Some(status) => format!(
                    "FFmpeg wrote no recording (exit {})",
                    status.code().unwrap_or(-1)
                ),
                None => "FFmpeg had to be killed and wrote no recording".to_owned(),
            });
        }
        Ok(self.path)
    }
}

/// Start a capture. The child's stderr goes to a log file beside the
/// recording so failures are diagnosable after the fact.
pub(crate) fn start_recording(mode: &RecordingMode) -> Result<ActiveRecording, String> {
    let ffmpeg = find_ffmpeg().ok_or_else(|| {
        "FFmpeg was not found (bundled beside OpenReel.exe, or on PATH)".to_owned()
    })?;
    let directory = recordings_directory();
    std::fs::create_dir_all(&directory)
        .map_err(|error| format!("could not create {}: {error}", directory.display()))?;
    let path = next_recording_path(&directory, mode.extension());
    let log = std::fs::File::create(path.with_extension("log"))
        .map_err(|error| format!("could not create the recording log: {error}"))?;

    let mut command = Command::new(ffmpeg);
    command
        .args(ffmpeg_record_args(mode, &path))
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::from(log));
    hide_console_window(&mut command);
    let child = command
        .spawn()
        .map_err(|error| format!("could not start FFmpeg: {error}"))?;
    Ok(ActiveRecording {
        child,
        path,
        started: Instant::now(),
        label: mode.label(),
    })
}

/// The `FFmpeg` invocation for one capture. Pure so tests can pin the shapes.
fn ffmpeg_record_args(mode: &RecordingMode, output: &Path) -> Vec<std::ffi::OsString> {
    let mut args: Vec<std::ffi::OsString> = Vec::new();
    let mut push = |arg: &str| args.push(arg.into());
    push("-hide_banner");
    push("-loglevel");
    push("error");
    match mode {
        RecordingMode::Screen {
            microphone,
            monitor,
        } => {
            push("-thread_queue_size");
            push("1024");
            push("-f");
            push("gdigrab");
            push("-framerate");
            push("30");
            if let Some(monitor) = monitor {
                push("-offset_x");
                push(&monitor.x.to_string());
                push("-offset_y");
                push(&monitor.y.to_string());
                push("-video_size");
                push(&format!("{}x{}", monitor.width, monitor.height));
            }
            push("-i");
            push("desktop");
            if let Some(microphone) = microphone {
                push("-thread_queue_size");
                push("1024");
                push("-rtbufsize");
                push("256M");
                push("-f");
                push("dshow");
                push("-i");
                push(&format!("audio={microphone}"));
            }
            // Ultrafast keeps encode cost negligible while capturing; the
            // even-dimension scale guards odd desktop sizes against yuv420p.
            push("-c:v");
            push("libx264");
            push("-preset");
            push("ultrafast");
            push("-pix_fmt");
            push("yuv420p");
            push("-vf");
            push("crop=trunc(iw/2)*2:trunc(ih/2)*2");
            if microphone.is_some() {
                push("-c:a");
                push("aac");
            }
        }
        RecordingMode::Camera { camera, microphone } => {
            push("-thread_queue_size");
            push("1024");
            push("-rtbufsize");
            push("256M");
            push("-f");
            push("dshow");
            push("-i");
            match microphone {
                Some(microphone) => push(&format!("video={camera}:audio={microphone}")),
                None => push(&format!("video={camera}")),
            }
            push("-c:v");
            push("libx264");
            push("-preset");
            push("ultrafast");
            push("-pix_fmt");
            push("yuv420p");
            push("-vf");
            push("crop=trunc(iw/2)*2:trunc(ih/2)*2");
            if microphone.is_some() {
                push("-c:a");
                push("aac");
            }
        }
        RecordingMode::Voice { microphone } => {
            push("-f");
            push("dshow");
            push("-i");
            push(&format!("audio={microphone}"));
            push("-c:a");
            push("aac");
        }
    }
    push("-y");
    args.push(output.into());
    args
}

/// Enumerate displays by asking Windows through a hidden `PowerShell` -
/// the same subprocess pattern as everything else here, and it keeps
/// display geometry out of unsafe Win32 calls.
fn list_monitors() -> Vec<MonitorInfo> {
    let mut command = Command::new("powershell");
    command
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "Add-Type -AssemblyName System.Windows.Forms; \
             [System.Windows.Forms.Screen]::AllScreens | ForEach-Object { \
             '{0}|{1}|{2}|{3}|{4}|{5}' -f $_.DeviceName, $_.Bounds.X, \
             $_.Bounds.Y, $_.Bounds.Width, $_.Bounds.Height, $_.Primary }",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    hide_console_window(&mut command);
    let Ok(output) = command.output() else {
        return Vec::new();
    };
    parse_monitor_lines(&String::from_utf8_lossy(&output.stdout))
}

/// Lines look like `\\.\DISPLAY1|0|0|2560|1440|True`.
fn parse_monitor_lines(stdout: &str) -> Vec<MonitorInfo> {
    stdout
        .lines()
        .filter_map(|line| {
            let mut fields = line.trim().split('|');
            let device = fields.next()?;
            let x = fields.next()?.parse().ok()?;
            let y = fields.next()?.parse().ok()?;
            let width = fields.next()?.parse().ok()?;
            let height = fields.next()?.parse().ok()?;
            let primary = fields.next()?.eq_ignore_ascii_case("true");
            let number = device
                .rsplit("DISPLAY")
                .next()
                .and_then(|digits| digits.parse::<u32>().ok());
            let label = match (number, primary) {
                (Some(number), true) => format!("Display {number} (primary) · {width}×{height}"),
                (Some(number), false) => format!("Display {number} · {width}×{height}"),
                (None, true) => format!("Display (primary) · {width}×{height}"),
                (None, false) => format!("Display · {width}×{height}"),
            };
            Some(MonitorInfo {
                label,
                x,
                y,
                width,
                height,
                primary,
            })
        })
        .collect()
}

/// Enumerate `DirectShow` devices by parsing `FFmpeg`'s listing (it reports the
/// list on stderr and exits nonzero by design).
pub(crate) fn list_capture_devices() -> CaptureDevices {
    let Some(ffmpeg) = find_ffmpeg() else {
        return CaptureDevices::default();
    };
    let mut command = Command::new(ffmpeg);
    command
        .args([
            "-hide_banner",
            "-list_devices",
            "true",
            "-f",
            "dshow",
            "-i",
            "dummy",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    hide_console_window(&mut command);
    let mut devices = match command.output() {
        Ok(output) => parse_dshow_devices(&String::from_utf8_lossy(&output.stderr)),
        Err(_) => CaptureDevices::default(),
    };
    devices.monitors = list_monitors();
    devices
}

/// Device lines look like `[dshow @ ...] "Name" (video)`; alternative-name
/// lines repeat the device and are skipped. Devices that do not declare a
/// category report `(none)` - virtual cameras do this - and count as video.
fn parse_dshow_devices(stderr: &str) -> CaptureDevices {
    let mut devices = CaptureDevices::default();
    for line in stderr.lines() {
        if line.contains("Alternative name") {
            continue;
        }
        let Some(start) = line.find('"') else {
            continue;
        };
        let Some(end) = line[start + 1..].find('"') else {
            continue;
        };
        let name = &line[start + 1..start + 1 + end];
        let rest = &line[start + 1 + end + 1..];
        if rest.contains("(video)") || rest.contains("(none)") {
            devices.video.push(name.to_owned());
        } else if rest.contains("(audio)") {
            devices.audio.push(name.to_owned());
        }
    }
    devices
}

/// The bundled CLI first (beside our executable in installs, under
/// `third_party` in dev checkouts), then whatever PATH offers.
fn find_ffmpeg() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("OPENREEL_FFMPEG") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        for ancestor in exe.ancestors().skip(1) {
            let beside = ancestor.join("ffmpeg.exe");
            if beside.is_file() {
                return Some(beside);
            }
            let dev = ancestor.join("third_party/ffmpeg/bin/ffmpeg.exe");
            if dev.is_file() {
                return Some(dev);
            }
        }
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|directory| directory.join("ffmpeg.exe"))
        .find(|candidate| candidate.is_file())
}

/// Recordings are user-visible files: `Videos\OpenReel` when the profile
/// exists, the system temporary directory otherwise.
fn recordings_directory() -> PathBuf {
    std::env::var_os("USERPROFILE").map_or_else(
        || std::env::temp_dir().join("OpenReel"),
        |home| PathBuf::from(home).join("Videos").join("OpenReel"),
    )
}

/// `Recording 1.mp4`, `Recording 2.mp4`, ... - human names, no timestamps.
fn next_recording_path(directory: &Path, extension: &str) -> PathBuf {
    let taken: Vec<u32> = std::fs::read_dir(directory)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter_map(|entry| recording_number(&entry.file_name().to_string_lossy()))
                .collect()
        })
        .unwrap_or_default();
    let next = taken.iter().max().map_or(1, |max| max.saturating_add(1));
    directory.join(format!("Recording {next}.{extension}"))
}

fn recording_number(file_name: &str) -> Option<u32> {
    let rest = file_name.strip_prefix("Recording ")?;
    let digits = rest.split('.').next()?;
    digits.parse().ok()
}

impl OpenReelApp {
    pub(crate) fn open_record_dialog(&mut self) {
        if self.recording.is_some() {
            return;
        }
        self.record_dialog.open = true;
        if self.record_dialog.devices.is_none() && self.record_dialog.devices_rx.is_none() {
            // Enumeration spawns FFmpeg; keep the UI thread out of it.
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::Builder::new()
                .name("openreel-capture-devices".to_owned())
                .spawn(move || {
                    let _ = tx.send(list_capture_devices());
                })
                .expect("failed to spawn the capture device scan");
            self.record_dialog.devices_rx = Some(rx);
        }
    }

    // One immediate-mode dialog pass, same shape as the export dialog.
    #[allow(clippy::too_many_lines)]
    pub(crate) fn show_record_dialog(&mut self, ctx: &egui::Context) {
        if let Some(rx) = &self.record_dialog.devices_rx
            && let Ok(devices) = rx.try_recv()
        {
            if self.record_dialog.camera.is_none() {
                self.record_dialog.camera = devices.video.first().cloned();
            }
            if self.record_dialog.microphone.is_none() {
                self.record_dialog.microphone = devices.audio.first().cloned();
            }
            // With several displays, default to the primary - recording
            // every screen at once is the surprise, not the expectation.
            if self.record_dialog.monitor.is_none() && devices.monitors.len() > 1 {
                self.record_dialog.monitor = devices
                    .monitors
                    .iter()
                    .find(|monitor| monitor.primary)
                    .or_else(|| devices.monitors.first())
                    .cloned();
            }
            self.record_dialog.devices = Some(devices);
            self.record_dialog.devices_rx = None;
        }
        if !self.record_dialog.open {
            return;
        }
        let mut open = self.record_dialog.open;
        let mut start = false;
        let devices = self.record_dialog.devices.clone();
        egui::Window::new("Record")
            .open(&mut open)
            .resizable(false)
            .show(ctx, |ui| {
                let Some(devices) = devices else {
                    ui.label("Scanning capture devices…");
                    ctx.request_repaint_after(Duration::from_millis(100));
                    return;
                };
                ui.horizontal(|ui| {
                    ui.selectable_value(
                        &mut self.record_dialog.source,
                        RecordSource::Screen,
                        "Screen",
                    );
                    ui.add_enabled_ui(!devices.video.is_empty(), |ui| {
                        ui.selectable_value(
                            &mut self.record_dialog.source,
                            RecordSource::Camera,
                            "Camera",
                        );
                    });
                    ui.add_enabled_ui(!devices.audio.is_empty(), |ui| {
                        ui.selectable_value(
                            &mut self.record_dialog.source,
                            RecordSource::Voice,
                            "Voice",
                        );
                    });
                });
                ui.add_space(space::TWO);
                egui::Grid::new("record-settings")
                    .num_columns(2)
                    .spacing(egui::vec2(space::THREE, space::TWO))
                    .show(ui, |ui| {
                        if self.record_dialog.source == RecordSource::Screen
                            && devices.monitors.len() > 1
                        {
                            ui.label("Display");
                            egui::ComboBox::from_id_salt("record-display")
                                .selected_text(
                                    self.record_dialog
                                        .monitor
                                        .as_ref()
                                        .map_or("All displays", |monitor| monitor.label.as_str()),
                                )
                                .show_ui(ui, |ui| {
                                    for monitor in &devices.monitors {
                                        ui.selectable_value(
                                            &mut self.record_dialog.monitor,
                                            Some(monitor.clone()),
                                            &monitor.label,
                                        );
                                    }
                                    ui.selectable_value(
                                        &mut self.record_dialog.monitor,
                                        None,
                                        "All displays",
                                    );
                                });
                            ui.end_row();
                        }
                        if self.record_dialog.source == RecordSource::Camera {
                            ui.label("Camera");
                            egui::ComboBox::from_id_salt("record-camera")
                                .selected_text(
                                    self.record_dialog.camera.as_deref().unwrap_or("None found"),
                                )
                                .show_ui(ui, |ui| {
                                    for camera in &devices.video {
                                        ui.selectable_value(
                                            &mut self.record_dialog.camera,
                                            Some(camera.clone()),
                                            camera,
                                        );
                                    }
                                });
                            ui.end_row();
                        }
                        ui.label("Microphone");
                        egui::ComboBox::from_id_salt("record-microphone")
                            .selected_text(
                                self.record_dialog.microphone.as_deref().unwrap_or("None"),
                            )
                            .show_ui(ui, |ui| {
                                if self.record_dialog.source != RecordSource::Voice {
                                    ui.selectable_value(
                                        &mut self.record_dialog.microphone,
                                        None,
                                        "None",
                                    );
                                }
                                for microphone in &devices.audio {
                                    ui.selectable_value(
                                        &mut self.record_dialog.microphone,
                                        Some(microphone.clone()),
                                        microphone,
                                    );
                                }
                            });
                        ui.end_row();
                    });
                ui.add_space(space::ONE);
                ui.colored_label(
                    color::TEXT_MUTED,
                    egui::RichText::new(format!(
                        "Saves to {} and lands on the timeline when you stop.",
                        recordings_directory().display()
                    ))
                    .size(type_size::CAPTION),
                );
                ui.add_space(space::TWO);
                let ready = match self.record_dialog.source {
                    RecordSource::Screen => true,
                    RecordSource::Camera => self.record_dialog.camera.is_some(),
                    RecordSource::Voice => self.record_dialog.microphone.is_some(),
                };
                if ui
                    .add_enabled(
                        ready,
                        egui::Button::image_and_text(
                            Icon::Record.image(size::ICON_MD).tint(color::STATUS_DANGER),
                            "Start recording",
                        ),
                    )
                    .clicked()
                {
                    start = true;
                }
            });
        self.record_dialog.open = open && !start;
        if start {
            self.start_recording_from_dialog();
        }
    }

    fn start_recording_from_dialog(&mut self) {
        let microphone = self.record_dialog.microphone.clone();
        let mode = match self.record_dialog.source {
            RecordSource::Screen => RecordingMode::Screen {
                microphone,
                monitor: self.record_dialog.monitor.clone(),
            },
            RecordSource::Camera => {
                let Some(camera) = self.record_dialog.camera.clone() else {
                    self.record_error("Recording", "No camera is selected");
                    return;
                };
                RecordingMode::Camera { camera, microphone }
            }
            RecordSource::Voice => {
                let Some(microphone) = microphone else {
                    self.record_error("Recording", "No microphone is selected");
                    return;
                };
                RecordingMode::Voice { microphone }
            }
        };
        match start_recording(&mode) {
            Ok(active) => {
                self.status = format!("Recording {}…", active.label.to_lowercase());
                self.recording = Some(active);
            }
            Err(error) => self.record_error("Recording", error),
        }
    }

    /// Stop the capture and send the file down the ordinary import path -
    /// probe, media pool, timeline, monitor cue, transcription.
    pub(crate) fn stop_recording_and_import(&mut self) {
        let Some(active) = self.recording.take() else {
            return;
        };
        match active.stop() {
            Ok(path) => self.import_recorded_file(path),
            Err(error) => self.record_error("Recording", error),
        }
    }

    fn import_recorded_file(&mut self, path: PathBuf) {
        self.status = format!("Importing {}…", path.display());
        let media = Arc::clone(&self.analysis);
        let result_tx = self.probe_tx.clone();
        std::thread::Builder::new()
            .name("openreel-recording-probe".to_owned())
            .spawn(move || {
                let result = media.probe(&path);
                let _ = result_tx.send((path, result));
            })
            .expect("failed to spawn the recording probe worker");
    }

    /// Watch a live capture: tick the REC clock and catch `FFmpeg` dying on
    /// its own (salvaging whatever it managed to write).
    pub(crate) fn poll_recording(&mut self, ctx: &egui::Context) {
        let Some(active) = &mut self.recording else {
            return;
        };
        if let Some(status) = active.exited() {
            let path = active.path.clone();
            self.recording = None;
            self.record_error(
                "Recording",
                format!(
                    "Capture stopped unexpectedly (exit {}); see {}",
                    status.code().unwrap_or(-1),
                    path.with_extension("log").display()
                ),
            );
            if path.metadata().is_ok_and(|metadata| metadata.len() > 0) {
                self.import_recorded_file(path);
            }
        } else {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
    }

    /// The top-bar record control: an affordance when idle, a red live chip
    /// with the elapsed clock (click to stop) while capturing.
    pub(crate) fn record_control(&mut self, ui: &mut egui::Ui) {
        if let Some(active) = &self.recording {
            let stop = ui
                .add(
                    egui::Button::image_and_text(
                        Icon::Record.image(size::ICON_MD).tint(color::STATUS_DANGER),
                        format!("REC {}", active.elapsed_label()),
                    )
                    .fill(color::SURFACE_RAISED),
                )
                .on_hover_text("Stop recording and add it to the timeline")
                .clicked();
            if stop {
                self.stop_recording_and_import();
            }
        } else if ui
            .add(egui::Button::image_and_text(
                Icon::Record.image(size::ICON_MD),
                "Record",
            ))
            .on_hover_text("Record the screen, a camera, or a voiceover")
            .clicked()
        {
            self.open_record_dialog();
        }
    }
}

#[cfg(windows)]
fn hide_console_window(command: &mut Command) {
    use std::os::windows::process::CommandExt as _;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn hide_console_window(_command: &mut Command) {}

#[cfg(test)]
mod tests {
    use super::*;

    fn joined(mode: &RecordingMode) -> String {
        ffmpeg_record_args(mode, Path::new("out.mp4"))
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[test]
    fn screen_capture_grabs_the_desktop_and_mixes_the_chosen_microphone() {
        let with_mic = joined(&RecordingMode::Screen {
            microphone: Some("Mic Array".to_owned()),
            monitor: None,
        });
        assert!(with_mic.contains("-f gdigrab -framerate 30 -i desktop"));
        assert!(with_mic.contains("-f dshow -i audio=Mic Array"));
        assert!(with_mic.contains("-c:v libx264 -preset ultrafast -pix_fmt yuv420p"));
        assert!(with_mic.contains("-c:a aac"));
        assert!(with_mic.ends_with("-y out.mp4"));

        let silent = joined(&RecordingMode::Screen {
            microphone: None,
            monitor: None,
        });
        assert!(!silent.contains("dshow"));
        assert!(!silent.contains("-c:a"));
    }

    #[test]
    fn a_chosen_monitor_becomes_a_gdigrab_region_negative_offsets_included() {
        // Real geometry: a display left of the primary sits at x = -2560,
        // and gdigrab takes virtual-desktop coordinates raw (live-verified).
        let monitor = MonitorInfo {
            label: "Display 2 · 2560×1440".to_owned(),
            x: -2560,
            y: 0,
            width: 2560,
            height: 1440,
            primary: false,
        };
        let args = joined(&RecordingMode::Screen {
            microphone: None,
            monitor: Some(monitor),
        });
        assert!(args.contains("-offset_x -2560 -offset_y 0 -video_size 2560x1440 -i desktop"));
    }

    #[test]
    fn monitor_lines_parse_bounds_primary_and_display_numbers() {
        let stdout = "\\\\.\\DISPLAY1|0|0|2560|1440|True\r\n\\\\.\\DISPLAY2|-2560|0|2560|1440|False\r\nnot a monitor line\r\n";
        let monitors = parse_monitor_lines(stdout);
        assert_eq!(monitors.len(), 2);
        assert_eq!(monitors[0].label, "Display 1 (primary) · 2560×1440");
        assert!(monitors[0].primary);
        assert_eq!(monitors[1].label, "Display 2 · 2560×1440");
        assert_eq!(monitors[1].x, -2560);
        assert_eq!((monitors[1].width, monitors[1].height), (2560, 1440));
        assert!(!monitors[1].primary);
    }

    #[test]
    fn camera_capture_binds_video_and_audio_into_one_dshow_input() {
        let both = joined(&RecordingMode::Camera {
            camera: "Integrated Camera".to_owned(),
            microphone: Some("Mic".to_owned()),
        });
        assert!(both.contains("-f dshow -i video=Integrated Camera:audio=Mic"));
        let video_only = joined(&RecordingMode::Camera {
            camera: "Integrated Camera".to_owned(),
            microphone: None,
        });
        assert!(video_only.contains("-i video=Integrated Camera"));
        assert!(!video_only.contains("audio="));
    }

    #[test]
    fn voice_capture_is_audio_only_aac() {
        let voice = joined(&RecordingMode::Voice {
            microphone: "Mic".to_owned(),
        });
        assert!(voice.contains("-f dshow -i audio=Mic"));
        assert!(voice.contains("-c:a aac"));
        assert!(!voice.contains("libx264"));
        assert_eq!(
            RecordingMode::Voice {
                microphone: "Mic".to_owned()
            }
            .extension(),
            "m4a"
        );
    }

    #[test]
    fn dshow_listing_parses_names_skips_alternatives_and_counts_none_as_video() {
        // The (none) case is real: OBS Virtual Camera reports no category.
        let stderr = r#"[dshow @ 0000015] "Integrated Camera" (video)
[dshow @ 0000015]   Alternative name "@device_pnp_\\?\usb#vid"
[dshow @ 0000015] "OBS Virtual Camera" (none)
[dshow @ 0000015] "Microphone Array (Realtek(R) Audio)" (audio)
[dshow @ 0000015]   Alternative name "@device_cm_{33D9A762}"
dummy: Immediate exit requested"#;
        let devices = parse_dshow_devices(stderr);
        assert_eq!(devices.video, ["Integrated Camera", "OBS Virtual Camera"]);
        assert_eq!(devices.audio, ["Microphone Array (Realtek(R) Audio)"]);
    }

    #[test]
    fn recording_names_count_upward_from_the_highest_existing() {
        assert_eq!(recording_number("Recording 7.mp4"), Some(7));
        assert_eq!(recording_number("Recording 12.m4a"), Some(12));
        assert_eq!(recording_number("Recording 3.log"), Some(3));
        assert_eq!(recording_number("Screencap.mp4"), None);
        let directory = std::env::temp_dir().join(format!(
            "openreel-recording-name-test-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("Recording 2.mp4"), b"x").unwrap();
        std::fs::write(directory.join("Recording 5.m4a"), b"x").unwrap();
        assert_eq!(
            next_recording_path(&directory, "mp4"),
            directory.join("Recording 6.mp4")
        );
        std::fs::remove_dir_all(&directory).unwrap();
    }
}
