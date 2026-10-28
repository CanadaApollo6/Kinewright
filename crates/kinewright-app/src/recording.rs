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
use kinewright_core::{IncidentSubject, LabelIncident};

use crate::{
    app::KinewrightApp,
    icons::Icon,
    theme::{self, color, size, space, type_size},
};

/// Capture devices the platform `FFmpeg` input backend reports, plus
/// displays the OS reports for screen capture.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct CaptureDevices {
    pub(crate) video: Vec<String>,
    pub(crate) audio: Vec<String>,
    pub(crate) monitors: Vec<MonitorInfo>,
}

/// One display in virtual-desktop coordinates. Windows `gdigrab` and Linux
/// `x11grab` both take these raw (negative offsets included, for displays
/// left of or above the primary).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MonitorInfo {
    pub(crate) label: String,
    pub(crate) x: i32,
    pub(crate) y: i32,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) primary: bool,
}

/// What one recording captures. Microphones are platform device names
/// (`DirectShow` on Windows, Pulse/ALSA on Linux); a screen capture with
/// no monitor grabs the whole virtual desktop.
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
    pub(crate) fn stop(self) -> Result<PathBuf, String> {
        self.finish().map(|(path, _complete)| path)
    }

    fn finish(mut self) -> Result<(PathBuf, bool), String> {
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
                Err(error) => {
                    let _ = self.child.kill();
                    let _ = self.child.wait();
                    return Err(format!("could not wait for FFmpeg: {error}"));
                }
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
        let complete = status.is_some_and(|status| status.success());
        Ok((self.path, complete))
    }
}

/// Start a capture. The child's stderr goes to a log file beside the
/// recording so failures are diagnosable after the fact.
pub(crate) fn start_recording(
    mode: &RecordingMode,
    directory: &Path,
) -> Result<ActiveRecording, String> {
    let ffmpeg = find_ffmpeg()
        .ok_or_else(|| "FFmpeg was not found (bundled beside Kinewright, or on PATH)".to_owned())?;
    std::fs::create_dir_all(directory)
        .map_err(|error| format!("could not create {}: {error}", directory.display()))?;
    let path = next_recording_path(directory, mode.extension());
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

fn start_feedback_recording(microphone: &str, directory: &Path) -> Result<ActiveRecording, String> {
    let ffmpeg = find_ffmpeg().ok_or_else(|| "FFmpeg was not found".to_owned())?;
    std::fs::create_dir_all(directory).map_err(|e| e.to_string())?;
    let path = next_recording_path(directory, "wav");
    let log = std::fs::File::create(path.with_extension("log")).map_err(|e| e.to_string())?;
    let mut args = ffmpeg_record_args(
        &RecordingMode::Voice {
            microphone: microphone.into(),
        },
        &path,
    );
    args.pop();
    args.extend(
        [
            "-t",
            "120",
            "-ac",
            "1",
            "-ar",
            "16000",
            "-c:a",
            "pcm_s16le",
            "-f",
            "wav",
        ]
        .map(std::ffi::OsString::from),
    );
    args.push(path.clone().into_os_string());
    let mut command = Command::new(ffmpeg);
    command
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::from(log));
    hide_console_window(&mut command);
    let child = command.spawn().map_err(|e| e.to_string())?;
    Ok(ActiveRecording {
        child,
        path,
        started: Instant::now(),
        label: "Feedback voice",
    })
}

/// The `FFmpeg` invocation for one capture. Pure so tests can pin the shapes.
fn ffmpeg_record_args(mode: &RecordingMode, output: &Path) -> Vec<std::ffi::OsString> {
    #[cfg(windows)]
    {
        ffmpeg_record_args_windows(mode, output)
    }
    #[cfg(target_os = "linux")]
    {
        ffmpeg_record_args_linux(mode, output, &x11_display())
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = (mode, output);
        Vec::new()
    }
}

fn push_arg(args: &mut Vec<std::ffi::OsString>, arg: &str) {
    args.push(arg.into());
}

fn push_video_encode(args: &mut Vec<std::ffi::OsString>, with_audio: bool) {
    for arg in [
        "-c:v",
        "libx264",
        "-preset",
        "ultrafast",
        "-pix_fmt",
        "yuv420p",
        "-vf",
        "crop=trunc(iw/2)*2:trunc(ih/2)*2",
        "-fps_mode",
        "cfr",
        "-r",
        "30",
    ] {
        push_arg(args, arg);
    }
    if with_audio {
        push_arg(args, "-c:a");
        push_arg(args, "aac");
    }
}

#[cfg(windows)]
fn ffmpeg_record_args_windows(mode: &RecordingMode, output: &Path) -> Vec<std::ffi::OsString> {
    let mut args: Vec<std::ffi::OsString> = Vec::new();
    push_arg(&mut args, "-hide_banner");
    push_arg(&mut args, "-loglevel");
    push_arg(&mut args, "error");
    match mode {
        RecordingMode::Screen {
            microphone,
            monitor,
        } => {
            push_arg(&mut args, "-thread_queue_size");
            push_arg(&mut args, "1024");
            push_arg(&mut args, "-f");
            push_arg(&mut args, "gdigrab");
            push_arg(&mut args, "-framerate");
            push_arg(&mut args, "30");
            if let Some(monitor) = monitor {
                push_arg(&mut args, "-offset_x");
                push_arg(&mut args, &monitor.x.to_string());
                push_arg(&mut args, "-offset_y");
                push_arg(&mut args, &monitor.y.to_string());
                push_arg(&mut args, "-video_size");
                push_arg(&mut args, &format!("{}x{}", monitor.width, monitor.height));
            }
            push_arg(&mut args, "-i");
            push_arg(&mut args, "desktop");
            if let Some(microphone) = microphone {
                push_arg(&mut args, "-thread_queue_size");
                push_arg(&mut args, "1024");
                push_arg(&mut args, "-rtbufsize");
                push_arg(&mut args, "256M");
                push_arg(&mut args, "-f");
                push_arg(&mut args, "dshow");
                push_arg(&mut args, "-i");
                push_arg(&mut args, &format!("audio={microphone}"));
            }
            push_video_encode(&mut args, microphone.is_some());
        }
        RecordingMode::Camera { camera, microphone } => {
            push_arg(&mut args, "-thread_queue_size");
            push_arg(&mut args, "1024");
            push_arg(&mut args, "-rtbufsize");
            push_arg(&mut args, "256M");
            push_arg(&mut args, "-f");
            push_arg(&mut args, "dshow");
            push_arg(&mut args, "-i");
            match microphone {
                Some(microphone) => {
                    push_arg(&mut args, &format!("video={camera}:audio={microphone}"));
                }
                None => push_arg(&mut args, &format!("video={camera}")),
            }
            push_video_encode(&mut args, microphone.is_some());
        }
        RecordingMode::Voice { microphone } => {
            push_arg(&mut args, "-f");
            push_arg(&mut args, "dshow");
            push_arg(&mut args, "-i");
            push_arg(&mut args, &format!("audio={microphone}"));
            push_arg(&mut args, "-c:a");
            push_arg(&mut args, "aac");
        }
    }
    push_arg(&mut args, "-y");
    args.push(output.into());
    args
}

#[cfg(target_os = "linux")]
fn x11_display() -> String {
    std::env::var("DISPLAY")
        .ok()
        .filter(|display| !display.is_empty())
        .unwrap_or_else(|| ":0.0".to_owned())
}

#[cfg(target_os = "linux")]
fn linux_audio_format(device: &str) -> &'static str {
    if device.starts_with("hw:")
        || device.starts_with("plughw:")
        || device.starts_with("sysdefault")
        || device.starts_with("default:")
    {
        "alsa"
    } else {
        "pulse"
    }
}

#[cfg(target_os = "linux")]
fn ffmpeg_record_args_linux(
    mode: &RecordingMode,
    output: &Path,
    display: &str,
) -> Vec<std::ffi::OsString> {
    let mut args: Vec<std::ffi::OsString> = Vec::new();
    push_arg(&mut args, "-hide_banner");
    push_arg(&mut args, "-loglevel");
    push_arg(&mut args, "error");
    match mode {
        RecordingMode::Screen {
            microphone,
            monitor,
        } => {
            push_arg(&mut args, "-thread_queue_size");
            push_arg(&mut args, "1024");
            push_arg(&mut args, "-f");
            push_arg(&mut args, "x11grab");
            push_arg(&mut args, "-framerate");
            push_arg(&mut args, "30");
            if let Some(monitor) = monitor {
                push_arg(&mut args, "-video_size");
                push_arg(&mut args, &format!("{}x{}", monitor.width, monitor.height));
            }
            push_arg(&mut args, "-i");
            push_arg(
                &mut args,
                &match monitor {
                    Some(monitor) => format!("{display}+{},{}", monitor.x, monitor.y),
                    None => display.to_owned(),
                },
            );
            if let Some(microphone) = microphone {
                push_arg(&mut args, "-thread_queue_size");
                push_arg(&mut args, "1024");
                push_arg(&mut args, "-f");
                push_arg(&mut args, linux_audio_format(microphone));
                push_arg(&mut args, "-i");
                push_arg(&mut args, microphone);
            }
            push_video_encode(&mut args, microphone.is_some());
        }
        RecordingMode::Camera { camera, microphone } => {
            push_arg(&mut args, "-thread_queue_size");
            push_arg(&mut args, "1024");
            push_arg(&mut args, "-f");
            push_arg(&mut args, "v4l2");
            push_arg(&mut args, "-framerate");
            push_arg(&mut args, "30");
            push_arg(&mut args, "-i");
            push_arg(&mut args, camera);
            if let Some(microphone) = microphone {
                push_arg(&mut args, "-thread_queue_size");
                push_arg(&mut args, "1024");
                push_arg(&mut args, "-f");
                push_arg(&mut args, linux_audio_format(microphone));
                push_arg(&mut args, "-i");
                push_arg(&mut args, microphone);
            }
            push_video_encode(&mut args, microphone.is_some());
        }
        RecordingMode::Voice { microphone } => {
            push_arg(&mut args, "-f");
            push_arg(&mut args, linux_audio_format(microphone));
            push_arg(&mut args, "-i");
            push_arg(&mut args, microphone);
            push_arg(&mut args, "-c:a");
            push_arg(&mut args, "aac");
        }
    }
    push_arg(&mut args, "-y");
    args.push(output.into());
    args
}

fn list_monitors() -> Vec<MonitorInfo> {
    #[cfg(windows)]
    {
        list_monitors_windows()
    }
    #[cfg(target_os = "linux")]
    {
        list_monitors_linux()
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        Vec::new()
    }
}

/// Enumerate displays by asking Windows through a hidden `PowerShell` -
/// the same subprocess pattern as everything else here, and it keeps
/// display geometry out of unsafe Win32 calls.
#[cfg(windows)]
fn list_monitors_windows() -> Vec<MonitorInfo> {
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

/// Enumerate displays from `xrandr --current`. Wayland-only sessions without
/// `XWayland` report nothing here; screen capture then uses the full `X11` root.
#[cfg(target_os = "linux")]
fn list_monitors_linux() -> Vec<MonitorInfo> {
    let mut command = Command::new("xrandr");
    command
        .args(["--current"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let Ok(output) = command.output() else {
        return Vec::new();
    };
    parse_xrandr_monitors(&String::from_utf8_lossy(&output.stdout))
}

/// Lines look like `\\.\DISPLAY1|0|0|2560|1440|True`.
#[cfg(any(windows, test))]
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
            Some(monitor_info(device, x, y, width, height, primary))
        })
        .collect()
}

fn monitor_info(name: &str, x: i32, y: i32, width: u32, height: u32, primary: bool) -> MonitorInfo {
    let number = name
        .rsplit("DISPLAY")
        .next()
        .and_then(|digits| digits.parse::<u32>().ok());
    let label = match (number, primary) {
        (Some(number), true) => format!("Display {number} (primary) · {width}×{height}"),
        (Some(number), false) => format!("Display {number} · {width}×{height}"),
        (None, true) => format!("{name} (primary) · {width}×{height}"),
        (None, false) => format!("{name} · {width}×{height}"),
    };
    MonitorInfo {
        label,
        x,
        y,
        width,
        height,
        primary,
    }
}

/// `xrandr --current` geometry lines look like `HDMI-1 connected primary 1920x1080+0+0`.
#[cfg(any(target_os = "linux", test))]
fn parse_xrandr_monitors(stdout: &str) -> Vec<MonitorInfo> {
    stdout
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let connected = line.find(" connected")?;
            let name = line[..connected].trim();
            if name.is_empty() {
                return None;
            }
            let rest = line[connected + " connected".len()..].trim_start();
            let primary = rest.starts_with("primary ");
            let geometry = if primary {
                rest.strip_prefix("primary ").unwrap_or(rest).trim_start()
            } else {
                rest
            };
            let (width, height, x, y) = parse_xrandr_geometry(geometry)?;
            Some(monitor_info(name, x, y, width, height, primary))
        })
        .collect()
}

#[cfg(any(target_os = "linux", test))]
fn parse_xrandr_geometry(geometry: &str) -> Option<(u32, u32, i32, i32)> {
    let x_at = geometry.find('x')?;
    let width = geometry[..x_at].parse().ok()?;
    let after_x = &geometry[x_at + 1..];
    let height_end = after_x.find(|character: char| !character.is_ascii_digit())?;
    let height = after_x[..height_end].parse().ok()?;
    let offsets = after_x[height_end..]
        .split_whitespace()
        .next()?
        .strip_prefix('+')?;
    let separator = offsets.rfind('+')?;
    let x = offsets[..separator].parse().ok()?;
    let y = offsets[separator + 1..].parse().ok()?;
    Some((width, height, x, y))
}

pub(crate) fn list_capture_devices() -> CaptureDevices {
    let Some(ffmpeg) = find_ffmpeg() else {
        return CaptureDevices {
            monitors: list_monitors(),
            ..CaptureDevices::default()
        };
    };
    #[cfg(windows)]
    {
        list_dshow_devices(&ffmpeg)
    }
    #[cfg(target_os = "linux")]
    {
        list_linux_devices(&ffmpeg)
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = ffmpeg;
        CaptureDevices {
            monitors: list_monitors(),
            ..CaptureDevices::default()
        }
    }
}

/// Enumerate `DirectShow` devices by parsing `FFmpeg`'s listing (it reports the
/// list on stderr and exits nonzero by design).
#[cfg(windows)]
fn list_dshow_devices(ffmpeg: &Path) -> CaptureDevices {
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

#[cfg(target_os = "linux")]
fn list_linux_devices(ffmpeg: &Path) -> CaptureDevices {
    let mut devices = CaptureDevices {
        video: ffmpeg_listed_sources(ffmpeg, "v4l2"),
        audio: ffmpeg_listed_sources(ffmpeg, "pulse"),
        monitors: list_monitors(),
    };
    if devices.audio.is_empty() {
        devices.audio = ffmpeg_listed_sources(ffmpeg, "alsa");
    }
    devices
}

#[cfg(target_os = "linux")]
fn ffmpeg_listed_sources(ffmpeg: &Path, device: &str) -> Vec<String> {
    let mut command = Command::new(ffmpeg);
    command
        .args(["-hide_banner", "-sources", device])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    hide_console_window(&mut command);
    let Ok(output) = command.output() else {
        return Vec::new();
    };
    let listing = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    parse_ffmpeg_sources(&listing)
}

/// `ffmpeg -sources` prints `name [description]` after an auto-detected header.
#[cfg(any(target_os = "linux", test))]
fn parse_ffmpeg_sources(listing: &str) -> Vec<String> {
    let mut sources = Vec::new();
    let mut in_list = false;
    for line in listing.lines() {
        let line = line.trim();
        if line.starts_with("Auto-detected sources") {
            in_list = true;
            continue;
        }
        if !in_list || line.is_empty() || line.starts_with('[') {
            continue;
        }
        let name = line.split('[').next().unwrap_or(line).trim();
        if name.is_empty()
            || name == "*"
            || name.eq_ignore_ascii_case("none")
            || name.starts_with("Cannot")
        {
            continue;
        }
        sources.push(name.to_owned());
    }
    sources
}

/// Device lines look like `[dshow @ ...] "Name" (video)`; alternative-name
/// lines repeat the device and are skipped. Devices that do not declare a
/// category report `(none)` - virtual cameras do this - and count as video.
#[cfg(any(windows, test))]
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
    if let Some(path) = std::env::var_os("KINEWRIGHT_FFMPEG") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }
    let names = ffmpeg_cli_names();
    if let Ok(exe) = std::env::current_exe() {
        for ancestor in exe.ancestors().skip(1) {
            for name in names {
                let beside = ancestor.join(name);
                if beside.is_file() {
                    return Some(beside);
                }
                let nested = ancestor.join("bin").join(name);
                if nested.is_file() {
                    return Some(nested);
                }
                let dev = ancestor.join("third_party/ffmpeg/bin").join(name);
                if dev.is_file() {
                    return Some(dev);
                }
            }
        }
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).find_map(|directory| {
        names
            .iter()
            .map(|name| directory.join(name))
            .find(|candidate| candidate.is_file())
    })
}

fn ffmpeg_cli_names() -> &'static [&'static str] {
    if cfg!(windows) {
        &["ffmpeg.exe"]
    } else {
        &["ffmpeg"]
    }
}

/// Recordings are user-visible files, grouped by a filesystem-safe project name.
fn recordings_directory(project_name: &str) -> PathBuf {
    recordings_root().join(sanitize_recordings_folder(project_name))
}

fn recordings_root() -> PathBuf {
    let home = if cfg!(windows) {
        std::env::var_os("USERPROFILE")
    } else {
        std::env::var_os("HOME")
    };
    home.map_or_else(
        || std::env::temp_dir().join("Kinewright"),
        |home| PathBuf::from(home).join("Videos").join("Kinewright"),
    )
}

fn sanitize_recordings_folder(project_name: &str) -> String {
    let mut sanitized = project_name
        .chars()
        .map(|character| {
            if character.is_control()
                || matches!(
                    character,
                    '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'
                )
            {
                '_'
            } else {
                character
            }
        })
        .collect::<String>();
    while matches!(sanitized.chars().last(), Some(' ' | '.')) {
        sanitized.pop();
    }
    let leading_bytes = sanitized
        .char_indices()
        .find(|(_, character)| !matches!(character, ' ' | '.'))
        .map_or(sanitized.len(), |(index, _)| index);
    sanitized.drain(..leading_bytes);
    let reserved = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    if sanitized.is_empty() {
        "Project".to_owned()
    } else if reserved
        .iter()
        .any(|reserved| sanitized.eq_ignore_ascii_case(reserved))
    {
        format!("_{sanitized}")
    } else {
        sanitized
    }
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

impl KinewrightApp {
    pub(crate) fn open_record_dialog(&mut self) {
        if self.recording.is_some() {
            return;
        }
        self.record_dialog.open = true;
        if self.record_dialog.devices.is_none() && self.record_dialog.devices_rx.is_none() {
            // Enumeration spawns FFmpeg; keep the UI thread out of it.
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::Builder::new()
                .name("kinewright-capture-devices".to_owned())
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
        let recording_directory = recordings_directory(&self.project_name());
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
                        recording_directory.display()
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
                    self.note_recording_failure("No camera is selected");
                    return;
                };
                RecordingMode::Camera { camera, microphone }
            }
            RecordSource::Voice => {
                let Some(microphone) = microphone else {
                    self.note_recording_failure("No microphone is selected");
                    return;
                };
                RecordingMode::Voice { microphone }
            }
        };
        let directory = recordings_directory(&self.project_name());
        match start_recording(&mode, &directory) {
            Ok(active) => {
                self.status = format!("Recording {}…", active.label.to_lowercase());
                self.recording = Some(active);
            }
            Err(error) => self.note_recording_failure(error),
        }
    }

    /// Appendix B rows 111-115: every recording refusal is
    /// `recording_unclassified` against the project.
    ///
    /// The subject is the project because a capture names no asset yet — the
    /// file it would have produced does not exist — and a device that keeps
    /// refusing is one problem, not one per attempt.
    fn note_recording_failure(&mut self, observed: impl Into<String>) {
        self.note_label(LabelIncident::Recording, IncidentSubject::Project, observed);
    }

    /// Stop the capture and send the file down the ordinary import path -
    /// probe, media pool, timeline, monitor cue, transcription.
    pub(crate) fn stop_recording_and_import(&mut self) {
        let Some(active) = self.recording.take() else {
            return;
        };
        match active.stop() {
            Ok(path) => self.import_recorded_file(path),
            Err(error) => self.note_recording_failure(error),
        }
    }

    fn import_recorded_file(&mut self, path: PathBuf) {
        if self.focused().ja1_job.is_some() {
            self.status = format!(
                "Recording retained at {}. Import the recording when the Studio operation finishes.",
                path.display()
            );
            return;
        }
        if self.focused().ja1.is_some() {
            self.start_ja1(crate::ja1_attachment::Action::ImportMedia(path));
            return;
        }
        self.status = format!("Importing {}…", path.display());
        let session_id = self.focused().id;
        let media = Arc::clone(&self.analysis);
        let result_tx = self.probe_tx.clone();
        std::thread::Builder::new()
            .name("kinewright-recording-probe".to_owned())
            .spawn(move || {
                let result = media.probe(&path);
                let _ = result_tx.send((session_id, path, result));
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
            self.note_recording_failure(format!(
                "Capture stopped unexpectedly (exit {}); see {}",
                status.code().unwrap_or(-1),
                path.with_extension("log").display()
            ));
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
            let mut rec = theme::caps_label("REC", color::TEXT_SECONDARY);
            rec.append(
                &format!(" {}", active.elapsed_label()),
                0.0,
                egui::TextFormat {
                    font_id: theme::code_font(),
                    color: color::TEXT_SECONDARY,
                    ..Default::default()
                },
            );
            let stop = ui
                .add(
                    egui::Button::image_and_text(
                        Icon::Record.image(size::ICON_MD).tint(color::STATUS_DANGER),
                        rec,
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

/// C5's capture/owner boundary. C6 supplies the displayed cell and host upload adapter.
#[allow(dead_code, reason = "Worker A connects the C5 backend to Studio in C6")]
pub(crate) mod feedback_capture {
    use super::{Arc, Duration, Instant, PathBuf, start_feedback_recording};
    use kinewright_media::voice_note::{PlayheadLog, VoiceJob, VoiceJobKey, VoiceTranscript};
    use kinewright_project::{
        bundle::{BundleError, ProjectBundle},
        bundle_records::{ObjectRef, ProjectId, RecordKind},
        receipts::{ReceiptLookup, ReceiptResult},
        storage_budget::StorageReservation,
        store::StorePin,
        studio_records::{
            FeedbackAnchor, JobId, MetadataChange, MetadataEnvelope, MetadataResult, MetadataScope,
            MetadataUpdate, NoteId, VoiceRef,
        },
        studio_store::PinnedSnapshot,
    };
    use std::{
        io::{Read, Seek},
        sync::atomic::{AtomicBool, Ordering},
        sync::mpsc::{Receiver, TryRecvError},
    };
    pub(crate) type JobKey = VoiceJobKey<ProjectId, NoteId, JobId>;

    static CAPTURE_ACTIVE: AtomicBool = AtomicBool::new(false);
    struct CapturePermit;
    impl CapturePermit {
        fn acquire() -> Result<Self, String> {
            CAPTURE_ACTIVE
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
                .map_err(|_| "a feedback capture is already running".to_owned())?;
            Ok(Self)
        }
    }
    impl Drop for CapturePermit {
        fn drop(&mut self) {
            CAPTURE_ACTIVE.store(false, Ordering::Release);
        }
    }
    struct CaptureStop(Arc<AtomicBool>);
    impl Drop for CaptureStop {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Release);
        }
    }
    pub(crate) struct CaptureReservation {
        project: ProjectId,
        reservation: StorageReservation,
    }
    impl CaptureReservation {
        pub(crate) fn reserve(owner: &ProjectBundle) -> Result<Self, BundleError> {
            Ok(Self {
                project: owner.project_id().clone(),
                reservation: owner.store().reserve_copy(3_840_000 + 65_536)?,
            })
        }
    }
    pub(crate) struct Capture {
        project: ProjectId,
        anchor: FeedbackAnchor,
        note_id: NoteId,
        log: Option<PlayheadLog<FeedbackAnchor>>,
        stop: CaptureStop,
        result: Receiver<CaptureWorkerResult>,
    }
    struct CaptureWorkerResult {
        result: Result<(PathBuf, bool), String>,
        snapshot: PinnedSnapshot,
        reservation: StorageReservation,
    }
    #[derive(Clone)]
    pub(crate) struct PendingVoiceWrite {
        pub(crate) envelope: MetadataEnvelope,
        pub(crate) update: MetadataUpdate,
    }
    pub(crate) struct CapturedAudio {
        project: ProjectId,
        anchor: FeedbackAnchor,
        note_id: NoteId,
        log: Option<PlayheadLog<FeedbackAnchor>>,
        pub(crate) path: PathBuf,
        pub(crate) complete: bool,
        pub(crate) capture_error: Option<String>,
        snapshot: Option<PinnedSnapshot>,
        reservation: Option<StorageReservation>,
        pending_write: Option<PendingVoiceWrite>,
    }
    pub(crate) struct SavedAudio {
        project: ProjectId,
        anchor: FeedbackAnchor,
        voice: VoiceRef,
        job: Option<VoiceJob<JobKey>>,
        expected_chosen: Option<ObjectRef>,
        pending_write: Option<PendingVoiceWrite>,
        pending_asr: Option<PendingAsr>,
        pub(crate) complete: bool,
        pub(crate) capture_error: Option<String>,
        _pin: StorePin,
    }
    type OriginalAsr =
        kinewright_media::voice_note::OriginalVoiceAsr<ProjectId, NoteId, JobId, FeedbackAnchor>;
    enum PendingAsr {
        Unvalidated(Box<OriginalAsr>),
        Completion(Box<OriginalAsr>),
        Sealed {
            reference: ObjectRef,
            _pin: StorePin,
        },
    }
    impl Capture {
        pub(crate) fn anchor(&self) -> &FeedbackAnchor {
            &self.anchor
        }
        pub(crate) fn note_id(&self) -> &NoteId {
            &self.note_id
        }
        pub(crate) fn record_commentary_displayed(
            &mut self,
            sample: kinewright_media::voice_note::PlayheadSample<FeedbackAnchor>,
        ) -> Result<(), kinewright_core::MediaError> {
            let result = self
                .log
                .as_mut()
                .ok_or_else(|| {
                    kinewright_core::MediaError::Backend("capture anchor log unavailable".into())
                })?
                .push(sample);
            if result.is_err() {
                self.stop();
            }
            result
        }

        pub(crate) fn start(
            budget: CaptureReservation,
            microphone: String,
            directory: PathBuf,
            anchor: FeedbackAnchor,
            snapshot: PinnedSnapshot,
        ) -> Result<Self, String> {
            let CaptureReservation {
                project,
                reservation,
            } = budget;
            if snapshot.descriptor.key != anchor.snapshot {
                return Err("capture must pin the displayed snapshot".into());
            }
            let permit = CapturePermit::acquire()?;
            let note_id = NoteId::fresh().map_err(|e| e.to_string())?;
            let stop = Arc::new(AtomicBool::new(false));
            let stopping = stop.clone();
            let (tx, result) = std::sync::mpsc::sync_channel(1);
            std::thread::Builder::new()
                .name("feedback-capture".into())
                .spawn(move || {
                    let _permit = permit;
                    let capture = (|| {
                        let mut active = start_feedback_recording(&microphone, &directory)?;
                        let deadline = Instant::now() + Duration::from_secs(120);
                        let mut unexpected_exit = false;
                        while !stopping.load(Ordering::Acquire) && Instant::now() < deadline {
                            if active.exited().is_some() {
                                unexpected_exit = true;
                                break;
                            }
                            std::thread::sleep(Duration::from_millis(25));
                        }
                        let (path, complete) = active.finish()?;
                        Ok((path, complete && !unexpected_exit))
                    })();
                    let _ = tx.send(CaptureWorkerResult {
                        result: capture,
                        snapshot,
                        reservation,
                    });
                })
                .map_err(|e| e.to_string())?;
            Ok(Self {
                log: Some(PlayheadLog::new(anchor.clone())),
                project,
                anchor,
                note_id,
                stop: CaptureStop(stop),
                result,
            })
        }
        pub(crate) fn stop(&self) {
            self.stop.0.store(true, Ordering::Release);
        }
        pub(crate) fn finish(self) -> Result<CapturedAudio, Box<Self>> {
            let (result, snapshot, reservation) = match self.result.try_recv() {
                Ok(worker) => (
                    worker.result,
                    Some(worker.snapshot),
                    Some(worker.reservation),
                ),
                Err(TryRecvError::Empty) => return Err(Box::new(self)),
                Err(TryRecvError::Disconnected) => {
                    (Err("capture worker disconnected".into()), None, None)
                }
            };
            let (path, complete, capture_error) = match result {
                Ok((path, complete)) => (path, complete, None),
                Err(error) => (PathBuf::new(), false, Some(error)),
            };
            Ok(CapturedAudio {
                project: self.project,
                anchor: self.anchor,
                note_id: self.note_id,
                log: self.log,
                path,
                complete,
                capture_error,
                snapshot,
                reservation,
                pending_write: None,
            })
        }
    }

    fn evidence(
        owner: &mut ProjectBundle,
        bytes: &[u8],
        dependencies: &[ObjectRef],
    ) -> Result<kinewright_project::store::SealedObject, BundleError> {
        let reference = ObjectRef {
            sha256: kinewright_media::sha256_bytes(bytes),
            byte_length: bytes.len() as u64,
            record_kind: RecordKind::OwnedEvidence,
        };
        owner
            .store_mut()?
            .admit_blob(&mut &*bytes, &reference, dependencies)
    }
    fn save_voice(
        owner: &mut ProjectBundle,
        envelope: &MetadataEnvelope,
        voice: &VoiceRef,
        pending: &mut Option<PendingVoiceWrite>,
    ) -> Result<(), BundleError> {
        if let Some(original) = pending {
            if &original.envelope != envelope
                || original.update.change != MetadataChange::SaveVoice(voice.clone())
            {
                return Err(BundleError::Unsettled);
            }
        } else {
            *pending = Some(PendingVoiceWrite {
                envelope: envelope.clone(),
                update: MetadataUpdate {
                    expected_root_generation: owner
                        .root()
                        .ok_or(BundleError::HistoryBarrierInvariant)?
                        .generation,
                    change: MetadataChange::SaveVoice(voice.clone()),
                },
            });
        }
        let original = pending
            .as_ref()
            .ok_or(BundleError::HistoryBarrierInvariant)?;
        let lookup = owner.metadata_command(
            &original.envelope,
            MetadataScope::Feedback,
            &original.update,
        )?;
        let (ReceiptLookup::Committed(receipt) | ReceiptLookup::Retired(receipt)) = lookup else {
            return Err(BundleError::Unsettled);
        };
        let index = owner.feedback_index()?;
        let retained = index.notes.get(&voice.note_id);
        if !matches!(receipt.result, ReceiptResult::Metadata { result: MetadataResult::Voice(ref id) } if id == &voice.note_id)
            || retained.is_none_or(|saved| {
                saved.audio_ref != voice.audio_ref
                    || saved.playhead_log_ref != voice.playhead_log_ref
                    || saved.duration_micros != voice.duration_micros
            })
        {
            return Err(BundleError::HistoryChanged);
        }
        *pending = None;
        Ok(())
    }
    impl CapturedAudio {
        pub(crate) fn anchor(&self) -> &FeedbackAnchor {
            &self.anchor
        }
        pub(crate) fn note_id(&self) -> &NoteId {
            &self.note_id
        }

        pub(crate) fn pending_write(&self) -> Option<&PendingVoiceWrite> {
            self.pending_write.as_ref()
        }
        /// Failure returns the complete draft and original command to its caller.
        #[allow(
            clippy::too_many_lines,
            reason = "Audio and its capture log share one durable acknowledgment boundary"
        )]
        pub(crate) fn acknowledge(
            mut self,
            owner: &mut ProjectBundle,
            envelope: &MetadataEnvelope,
        ) -> Result<SavedAudio, Box<(Self, BundleError)>> {
            let result = (|| {
                if envelope.project_id != self.project || owner.project_id() != &self.project {
                    return Err(BundleError::HistoryChanged);
                }
                if self.snapshot.is_none() || self.reservation.is_none() {
                    return Err(BundleError::HistoryChanged);
                }
                owner.read_snapshot(&self.anchor.snapshot)?;
                let mut file = std::fs::File::open(&self.path)?;
                let length = file.metadata()?.len();
                if length > 3_840_000 + 65_536 {
                    return Err(BundleError::Capacity("voice capture bytes"));
                }
                let mut wav = Vec::new();
                std::io::Read::by_ref(&mut file)
                    .take(length + 1)
                    .read_to_end(&mut wav)?;
                if wav.len() as u64 != length {
                    return Err(BundleError::HistoryChanged);
                }
                let duration = kinewright_media::voice_note::voice_audio_duration(&wav)
                    .map_err(|e| BundleError::Corrupt(e.to_string()))?;
                let audio_ref = ObjectRef {
                    sha256: kinewright_media::sha256_bytes(&wav),
                    byte_length: length,
                    record_kind: RecordKind::OwnedEvidence,
                };
                file.rewind()?;
                let audio = owner.store_mut()?.admit_blob(&mut file, &audio_ref, &[])?;
                let mut log = self
                    .log
                    .as_ref()
                    .ok_or_else(|| BundleError::Corrupt("voice anchor log missing".into()))?
                    .clone();
                log.version = 2;
                log.capture_status = Some(kinewright_media::voice_note::CaptureStatus {
                    complete: self.complete,
                    capture_error: self.capture_error.clone(),
                });
                log.validate(duration)
                    .map_err(|e| BundleError::Corrupt(e.to_string()))?;
                let mut dependencies = Vec::new();
                let mut pins = Vec::new();
                let mut bounds = std::collections::BTreeMap::new();
                for sample in &log.samples {
                    let anchor = &sample.displayed;
                    if !bounds.contains_key(&anchor.snapshot.version) {
                        let pinned = owner.read_snapshot(&anchor.snapshot)?;
                        let document = owner.store().read_record::<kinewright_core::Document>(
                            &pinned.descriptor.document_ref,
                        )?;
                        dependencies.push(pinned.descriptor.checkpoint_ref.clone());
                        bounds.insert(
                            anchor.snapshot.version.clone(),
                            (anchor.snapshot.clone(), document.fps, document.duration.0),
                        );
                        pins.push(pinned);
                    }
                    let (key, fps, end) = &bounds[&anchor.snapshot.version];
                    if key != &anchor.snapshot
                        || anchor.frame < 0
                        || anchor.frame >= *end
                        || anchor.fps.numerator != fps.numerator()
                        || anchor.fps.denominator != fps.denominator()
                    {
                        return Err(BundleError::HistoryChanged);
                    }
                }
                let bytes =
                    serde_json::to_vec(&log).map_err(|e| BundleError::Corrupt(e.to_string()))?;
                if bytes.len() > 8 * 1024 * 1024 {
                    return Err(BundleError::Capacity("commentary log bytes"));
                }
                let log = evidence(owner, &bytes, &dependencies)?;
                let voice = VoiceRef {
                    note_id: self.note_id.clone(),
                    audio_ref: audio.reference.clone(),
                    duration_micros: duration,
                    range: None,
                    playhead_log_ref: Some(log.reference.clone()),
                    original_asr_ref: None,
                    chosen_transcript_revision: None,
                };
                save_voice(owner, envelope, &voice, &mut self.pending_write)?;
                let mut roots = vec![voice.audio_ref.clone()];
                roots.extend(voice.playhead_log_ref.clone());
                let pin = owner.store().pin(&roots)?;
                Ok((voice, pin))
            })();
            match result {
                Ok((voice, pin)) => Ok(SavedAudio {
                    project: self.project,
                    anchor: self.anchor,
                    voice,
                    job: None,
                    expected_chosen: None,
                    pending_write: None,
                    pending_asr: None,
                    complete: self.complete,
                    capture_error: self.capture_error,
                    _pin: pin,
                }),
                Err(error) => Err(Box::new((self, error))),
            }
        }
    }
    impl SavedAudio {
        pub(crate) fn anchor(&self) -> &FeedbackAnchor {
            &self.anchor
        }

        pub(crate) fn pending_write(&self) -> Option<&PendingVoiceWrite> {
            self.pending_write.as_ref()
        }
        /// Settle the original command after an unknown reply, even if its job completed.
        pub(crate) fn retry_pending(
            &mut self,
            owner: &mut ProjectBundle,
        ) -> Result<bool, BundleError> {
            if owner.project_id() != &self.project {
                return Err(BundleError::HistoryChanged);
            }
            if self.pending_asr.is_some() {
                return self.publish_asr(owner, None);
            }
            let Some(pending) = self.pending_write.clone() else {
                return Ok(false);
            };
            let MetadataChange::SaveVoice(voice) = pending.update.change else {
                return Err(BundleError::HistoryChanged);
            };
            save_voice(owner, &pending.envelope, &voice, &mut self.pending_write)?;
            self.voice = voice;
            Ok(true)
        }
        pub(crate) fn reopen(owner: &ProjectBundle, note: &NoteId) -> Result<Self, BundleError> {
            let index = owner.feedback_index()?;
            let voice = index
                .notes
                .get(note)
                .cloned()
                .ok_or(BundleError::HistoryChanged)?;
            let log_ref = voice.playhead_log_ref.as_ref().ok_or_else(|| {
                BundleError::Unsupported("voice has no retained anchor log".into())
            })?;
            let bytes = owner.store().read_evidence(log_ref)?;
            let log: PlayheadLog<FeedbackAnchor> =
                serde_json::from_slice(&bytes).map_err(|e| BundleError::Corrupt(e.to_string()))?;
            log.validate(voice.duration_micros)
                .map_err(|e| BundleError::Corrupt(e.to_string()))?;
            let anchor = log.samples[0].displayed.clone();
            let snapshot = owner.read_snapshot(&anchor.snapshot)?;
            let pin = owner.store().pin(&[
                voice.audio_ref.clone(),
                log_ref.clone(),
                snapshot.descriptor.checkpoint_ref.clone(),
            ])?;
            Ok(Self {
                project: owner.project_id().clone(),
                anchor,
                voice,
                job: None,
                expected_chosen: None,
                pending_write: None,
                pending_asr: None,
                complete: log
                    .capture_status
                    .as_ref()
                    .is_some_and(|status| status.complete),
                capture_error: log.capture_status.as_ref().map_or_else(
                    || Some("Capture completion status was not retained by this older note".into()),
                    |status| status.capture_error.clone(),
                ),
                _pin: pin,
            })
        }
        pub(crate) fn remark_voice(
            &self,
            start_micros: u64,
            end_micros: u64,
        ) -> Result<VoiceRef, BundleError> {
            if start_micros >= end_micros || end_micros > self.voice.duration_micros {
                return Err(BundleError::Capacity("commentary audio range"));
            }
            let mut voice = self.voice.clone();
            voice.range = Some(kinewright_project::studio_records::AudioRange {
                start_micros,
                end_micros,
            });
            Ok(voice)
        }
        pub(crate) fn voice(&self) -> &VoiceRef {
            &self.voice
        }
        pub(crate) fn start_job(
            &mut self,
            owner: &ProjectBundle,
            transcript_revision: u64,
        ) -> Result<JobKey, BundleError> {
            if self.pending_write.is_some() || self.pending_asr.is_some() {
                return Err(BundleError::Unsettled);
            }
            if owner.project_id() != &self.project
                || owner.feedback_index()?.notes.get(&self.voice.note_id) != Some(&self.voice)
            {
                return Err(BundleError::HistoryChanged);
            }
            let key = JobKey {
                project: self.project.clone(),
                note: self.voice.note_id.clone(),
                job: JobId::fresh()?,
                audio_sha256: self.voice.audio_ref.sha256.clone(),
                transcript_revision,
            };
            self.job = Some(VoiceJob::new(key.clone()));
            self.expected_chosen = self.voice.chosen_transcript_revision.clone();
            Ok(key)
        }
        pub(crate) fn spawn_job(
            &self,
            owner: &ProjectBundle,
            data_dir: PathBuf,
            key: &JobKey,
        ) -> Result<kinewright_media::voice_note::VoiceTask<JobKey>, BundleError> {
            let Some(job) = &self.job else {
                return Err(BundleError::HistoryChanged);
            };
            if owner.project_id() != &self.project
                || job.key() != key
                || owner.feedback_index()?.notes.get(&self.voice.note_id) != Some(&self.voice)
            {
                return Err(BundleError::HistoryChanged);
            }
            let audio = owner.store().read_evidence(&self.voice.audio_ref)?;
            kinewright_media::voice_note::spawn_voice_transcription(
                key.clone(),
                audio,
                data_dir,
                job.cancellation(),
            )
            .map_err(|e| BundleError::Corrupt(e.to_string()))
        }
        pub(crate) fn cancel(&self) {
            if let Some(job) = &self.job {
                job.cancel();
            }
        }
        pub(crate) fn choose_transcript(
            &mut self,
            owner: &mut ProjectBundle,
            envelope: &MetadataEnvelope,
            reference: ObjectRef,
        ) -> Result<(), BundleError> {
            if owner.project_id() != &self.project
                || owner.feedback_index()?.notes.get(&self.voice.note_id) != Some(&self.voice)
            {
                return Err(BundleError::HistoryChanged);
            }
            let text = owner.store().read_evidence(&reference)?;
            if text.len() > kinewright_media::voice_note::MAX_VOICE_TEXT_BYTES {
                return Err(BundleError::Capacity("voice transcript text"));
            }
            std::str::from_utf8(&text).map_err(|e| BundleError::Corrupt(e.to_string()))?;
            if self.pending_asr.is_some() {
                return Err(BundleError::Unsettled);
            }
            self.cancel();
            let mut voice = self.voice.clone();
            voice.chosen_transcript_revision = Some(reference);
            save_voice(owner, envelope, &voice, &mut self.pending_write)?;
            self.voice = voice;
            Ok(())
        }
        pub(crate) fn accept_asr(
            &mut self,
            owner: &mut ProjectBundle,
            envelope: &MetadataEnvelope,
            key: &JobKey,
            current: &JobKey,
            transcript: VoiceTranscript,
        ) -> Result<bool, BundleError> {
            if self.pending_write.is_some() || self.pending_asr.is_some() {
                return Err(BundleError::Unsettled);
            }
            if owner.project_id() != &self.project {
                return Ok(false);
            }
            transcript
                .validate(self.voice.duration_micros)
                .map_err(|e| BundleError::Corrupt(e.to_string()))?;
            if !matches!(transcript, VoiceTranscript::Ready(_)) && !self.live_note_for_asr(owner)? {
                return Ok(false);
            }
            let log = self
                .voice
                .playhead_log_ref
                .as_ref()
                .ok_or(BundleError::HistoryChanged)?
                .clone();
            let Some(transcript) = self
                .job
                .as_mut()
                .and_then(|job| job.accept(key, current, transcript))
            else {
                return Ok(false);
            };
            if let VoiceTranscript::Ready(_) = &transcript {
                let original = kinewright_media::voice_note::OriginalVoiceAsr {
                    version: 1,
                    job: key.clone(),
                    anchor: self.anchor.clone(),
                    playhead_log_sha256: log.sha256.clone(),
                    model_sha256: kinewright_media::WHISPER_MODEL_SHA256.into(),
                    transcript,
                };
                self.pending_asr = Some(PendingAsr::Unvalidated(Box::new(original)));
                return self.publish_asr(owner, Some(envelope));
            }
            Ok(true)
        }
        fn live_note_for_asr(&self, owner: &ProjectBundle) -> Result<bool, BundleError> {
            Ok(owner
                .feedback_index()?
                .notes
                .get(&self.voice.note_id)
                .is_some_and(|saved| {
                    saved.audio_ref == self.voice.audio_ref
                        && saved.chosen_transcript_revision == self.expected_chosen
                        && saved.original_asr_ref.is_none()
                }))
        }
        fn publish_asr(
            &mut self,
            owner: &mut ProjectBundle,
            envelope: Option<&MetadataEnvelope>,
        ) -> Result<bool, BundleError> {
            if matches!(self.pending_asr, Some(PendingAsr::Unvalidated(_))) {
                if !self.live_note_for_asr(owner)? {
                    self.pending_asr = None;
                    return Ok(false);
                }
                let Some(PendingAsr::Unvalidated(original)) = self.pending_asr.take() else {
                    return Err(BundleError::HistoryBarrierInvariant);
                };
                self.pending_asr = Some(PendingAsr::Completion(original));
            }
            if let Some(PendingAsr::Completion(original)) = &self.pending_asr {
                let log = self
                    .voice
                    .playhead_log_ref
                    .as_ref()
                    .ok_or(BundleError::HistoryChanged)?;
                let asr = evidence(
                    owner,
                    &serde_json::to_vec(original)
                        .map_err(|e| BundleError::Corrupt(e.to_string()))?,
                    &[self.voice.audio_ref.clone(), log.clone()],
                )?;
                let reference = asr.reference.clone();
                self.pending_asr = Some(PendingAsr::Sealed {
                    reference,
                    _pin: asr.into_pin(),
                });
            }
            let Some(PendingAsr::Sealed { reference, .. }) = &self.pending_asr else {
                return Ok(false);
            };
            let reference = reference.clone();
            let (envelope, voice) = if let Some(pending) = &self.pending_write {
                let MetadataChange::SaveVoice(voice) = &pending.update.change else {
                    return Err(BundleError::HistoryChanged);
                };
                (pending.envelope.clone(), voice.clone())
            } else {
                let mut voice = owner
                    .feedback_index()?
                    .notes
                    .get(&self.voice.note_id)
                    .cloned()
                    .ok_or(BundleError::HistoryChanged)?;
                if voice.audio_ref != self.voice.audio_ref
                    || voice.playhead_log_ref != self.voice.playhead_log_ref
                    || voice.duration_micros != self.voice.duration_micros
                    || voice
                        .original_asr_ref
                        .as_ref()
                        .is_some_and(|saved| saved != &reference)
                {
                    return Err(BundleError::HistoryChanged);
                }
                if voice.original_asr_ref.as_ref() == Some(&reference) {
                    self.voice = voice;
                    self.pending_asr = None;
                    return Ok(true);
                }
                voice.original_asr_ref = Some(reference);
                let envelope = match envelope {
                    Some(envelope) => envelope.clone(),
                    None => MetadataEnvelope {
                        id: kinewright_project::receipts::CommandId {
                            issuer: owner.issue_metadata_issuer()?,
                            sequence: 1,
                        },
                        project_id: self.project.clone(),
                        target_branch: owner.manual_branch_id().clone(),
                        owner_epoch: owner.store().owner_epoch(),
                        expected_head: owner.manual_head().cloned(),
                    },
                };
                (envelope, voice)
            };
            if let Err(error) = save_voice(owner, &envelope, &voice, &mut self.pending_write) {
                if !matches!(
                    error,
                    BundleError::DurabilityUnknown
                        | BundleError::DurabilityUnknownAt(_)
                        | BundleError::Unsettled
                ) && owner.publication_state()
                    != kinewright_project::bundle::PublicationState::Unknown
                {
                    self.pending_write = None;
                }
                return Err(error);
            }
            self.voice = voice;
            self.pending_asr = None;
            Ok(true)
        }
    }
    pub(crate) struct AnchoredRemark {
        pub(crate) original: kinewright_media::voice_note::CommentaryRemark<FeedbackAnchor>,
        pub(crate) anchor: FeedbackAnchor,
        pub(crate) spoken: kinewright_media::voice_note::SpokenTimecode,
    }
    pub(crate) fn anchor_remark(
        owner: &ProjectBundle,
        remark: kinewright_media::voice_note::CommentaryRemark<FeedbackAnchor>,
    ) -> Result<AnchoredRemark, BundleError> {
        use kinewright_core::{
            Document, FrameRounding, Rational, TimeCode, map_frames_with_rounding,
        };
        use kinewright_media::voice_note::{SpokenTimecode, spoken_timecode};
        let snapshot = owner.read_snapshot(&remark.playhead_anchor.snapshot)?;
        let document = owner
            .store()
            .read_record::<Document>(&snapshot.descriptor.document_ref)?;
        let micros_rate =
            Rational::new(1_000_000, 1).map_err(|e| BundleError::Corrupt(e.to_string()))?;
        let duration = map_frames_with_rounding(
            document.duration,
            document.fps,
            micros_rate,
            FrameRounding::Floor,
        )
        .map_err(|e| BundleError::Corrupt(e.to_string()))?;
        let mut anchor = remark.playhead_anchor.clone();
        let mut spoken = spoken_timecode(
            &remark.text,
            u64::try_from(duration.0).map_err(|e| BundleError::Corrupt(e.to_string()))?,
        );
        if let SpokenTimecode::Parsed(micros) = spoken {
            let frame = map_frames_with_rounding(
                TimeCode(i64::try_from(micros).map_err(|e| BundleError::Corrupt(e.to_string()))?),
                micros_rate,
                document.fps,
                FrameRounding::Floor,
            )
            .map_err(|e| BundleError::Corrupt(e.to_string()))?;
            if frame >= TimeCode::ZERO && frame < document.duration {
                anchor.frame = frame.0;
                anchor.target = None;
                anchor.source_ref = None;
                anchor.script_beat_id = None;
            } else {
                spoken = SpokenTimecode::OutOfRange;
            }
        }
        Ok(AnchoredRemark {
            original: remark,
            anchor,
            spoken,
        })
    }
    #[cfg(all(test, target_os = "linux"))]
    #[allow(
        clippy::too_many_lines,
        reason = "Stateful durability witnesses retain their complete owner and restart sequence"
    )]
    mod tests {
        use super::*;
        use kinewright_media::{
            test_support::{TempDirectory, workspace_target_dir},
            voice_note::{
                PlayheadEvent, PlayheadSample, SpokenTimecode, VoiceWord, commentary_remarks,
            },
        };
        use kinewright_project::{
            bundle::LoadedBundle, c0_fixtures::build_c0_fixture_pair, receipts::CommandId,
            studio_records::*,
        };
        use std::path::Path;
        static INFERENCE_TESTS: std::sync::Mutex<()> = std::sync::Mutex::new(());
        fn envelope(owner: &mut ProjectBundle) -> MetadataEnvelope {
            MetadataEnvelope {
                id: CommandId {
                    issuer: owner.issue_metadata_issuer().unwrap(),
                    sequence: 1,
                },
                project_id: owner.project_id().clone(),
                target_branch: owner.manual_branch_id().clone(),
                owner_epoch: owner.store().owner_epoch(),
                expected_head: owner.manual_head().cloned(),
            }
        }
        fn metadata(
            owner: &mut ProjectBundle,
            scope: MetadataScope,
            change: MetadataChange,
        ) -> MetadataResult {
            let e = envelope(owner);
            let update = MetadataUpdate {
                expected_root_generation: owner.root().unwrap().generation,
                change,
            };
            let ReceiptLookup::Committed(receipt) =
                owner.metadata_command(&e, scope, &update).unwrap()
            else {
                panic!("committed metadata")
            };
            let ReceiptResult::Metadata { result } = receipt.result else {
                panic!("metadata result")
            };
            result
        }
        fn snapshot(owner: &mut ProjectBundle) -> SnapshotKey {
            let branch = owner.manual_branch_id().clone();
            let p = PublishSnapshot {
                version: VersionId::fresh().unwrap(),
                expected_head: owner.manual_head().unwrap().clone(),
                checkpoint_ref: owner.checkpoint_reference(&branch).unwrap(),
                parent_version: None,
                run_provenance: None,
                revision_refs: vec![],
            };
            let MetadataResult::Snapshot(key) = metadata(
                owner,
                MetadataScope::Snapshot,
                MetadataChange::PublishSnapshot(p),
            ) else {
                panic!("snapshot")
            };
            key
        }
        fn setup(root: &Path) -> (ProjectBundle, FeedbackAnchor) {
            std::fs::create_dir_all(root).unwrap();
            let [fixture, _] = build_c0_fixture_pair(root).unwrap();
            let mut owner = LoadedBundle::read(&fixture.path)
                .unwrap()
                .into_writer()
                .unwrap();
            let key = snapshot(&mut owner);
            let anchor = FeedbackAnchor {
                snapshot: key,
                frame: 8,
                fps: FrameRate {
                    numerator: 30,
                    denominator: 1,
                },
                raster_size: [1920, 1080],
                orientation: Orientation::Upright,
                target: None,
                source_ref: None,
                script_beat_id: None,
            };
            (owner, anchor)
        }
        fn wav(amplitude: i16) -> Vec<u8> {
            let mut bytes = Vec::new();
            let data_bytes = 96_000u32;
            bytes.extend_from_slice(b"RIFF");
            bytes.extend_from_slice(&(36 + data_bytes).to_le_bytes());
            bytes.extend_from_slice(b"WAVEfmt ");
            bytes.extend_from_slice(&16u32.to_le_bytes());
            bytes.extend_from_slice(&1u16.to_le_bytes());
            bytes.extend_from_slice(&1u16.to_le_bytes());
            bytes.extend_from_slice(&16000u32.to_le_bytes());
            bytes.extend_from_slice(&32000u32.to_le_bytes());
            bytes.extend_from_slice(&2u16.to_le_bytes());
            bytes.extend_from_slice(&16u16.to_le_bytes());
            bytes.extend_from_slice(b"data");
            bytes.extend_from_slice(&data_bytes.to_le_bytes());
            for _ in 0..48000 {
                bytes.extend_from_slice(&amplitude.to_le_bytes());
            }
            bytes
        }
        fn capture(
            root: &Path,
            owner: &ProjectBundle,
            anchor: FeedbackAnchor,
            amplitude: i16,
        ) -> CapturedAudio {
            let note_id = NoteId::fresh().unwrap();
            let path = root.join(format!("{}.wav", note_id.as_str()));
            std::fs::write(&path, wav(amplitude)).unwrap();
            let mut log = PlayheadLog::new(anchor.clone());
            log.version = 2;
            log.capture_status = Some(kinewright_media::voice_note::CaptureStatus {
                complete: true,
                capture_error: None,
            });
            CapturedAudio {
                project: owner.project_id().clone(),
                log: Some(log),
                snapshot: Some(owner.read_snapshot(&anchor.snapshot).unwrap()),
                reservation: Some(owner.store().reserve_copy(4_000_000).unwrap()),
                anchor,
                note_id,
                path,
                complete: true,
                capture_error: None,
                pending_write: None,
            }
        }
        fn acknowledge(captured: CapturedAudio, owner: &mut ProjectBundle) -> SavedAudio {
            let e = envelope(owner);
            captured
                .acknowledge(owner, &e)
                .unwrap_or_else(|failure| panic!("audio acknowledgment: {}", failure.1))
        }
        fn asr() -> VoiceTranscript {
            VoiceTranscript::Ready(vec![VoiceWord {
                text: "make it warmer.".into(),
                start_micros: 100_000,
                end_micros: 500_000,
            }])
        }
        fn original_asr(saved: &SavedAudio, key: &JobKey) -> (ObjectRef, Vec<u8>) {
            let original = kinewright_media::voice_note::OriginalVoiceAsr {
                version: 1,
                job: key.clone(),
                anchor: saved.anchor.clone(),
                playhead_log_sha256: saved
                    .voice
                    .playhead_log_ref
                    .as_ref()
                    .unwrap()
                    .sha256
                    .clone(),
                model_sha256: kinewright_media::WHISPER_MODEL_SHA256.into(),
                transcript: asr(),
            };
            let bytes = serde_json::to_vec(&original).unwrap();
            (
                ObjectRef {
                    sha256: kinewright_media::sha256_bytes(&bytes),
                    byte_length: bytes.len() as u64,
                    record_kind: RecordKind::OwnedEvidence,
                },
                bytes,
            )
        }
        fn move_for_asr(owner: &mut ProjectBundle, core: &kinewright_core::Core, frame: i64) {
            let document = owner
                .branch_history(owner.manual_branch_id())
                .unwrap()
                .document
                .clone();
            let clip = document.tracks[0].clips[0].id;
            assert!(matches!(
                core.request(kinewright_core::Command::Do(
                    kinewright_core::Operation::MoveClip {
                        clip,
                        to_track: kinewright_core::TrackId(1),
                        to: kinewright_core::TimeCode(frame),
                    }
                ))
                .unwrap(),
                kinewright_core::Event::DocumentChanged { .. }
            ));
            owner
                .save_branch(&owner.manual_branch_id().clone(), core)
                .unwrap();
        }
        #[test]
        fn s3_voice_fix1_stale_transcript_choice() {
            let root = TempDirectory::new_in(&workspace_target_dir(), "s3-stale-choice");
            let (mut owner, anchor) = setup(root.root());
            let mut stale = acknowledge(capture(root.root(), &owner, anchor, 1000), &mut owner);
            let mut fresh = SavedAudio::reopen(&owner, &stale.voice.note_id).unwrap();
            let first = evidence(&mut owner, b"fresh edited words", &[]).unwrap();
            let first_ref = first.reference.clone();
            let e = envelope(&mut owner);
            fresh
                .choose_transcript(&mut owner, &e, first_ref.clone())
                .unwrap();
            drop(first);
            let second = evidence(&mut owner, b"stale replacement", &[]).unwrap();
            let e = envelope(&mut owner);
            assert!(
                matches!(
                    stale.choose_transcript(&mut owner, &e, second.reference.clone()),
                    Err(BundleError::HistoryChanged)
                ),
                "S3_VOICE_STALE_CHOICE_REFUSED"
            );
            assert_eq!(
                owner.feedback_index().unwrap().notes[&fresh.voice.note_id]
                    .chosen_transcript_revision,
                Some(first_ref.clone())
            );
            snapshot(&mut owner);
            owner.collect_after_ack().unwrap();
            assert_eq!(
                &*owner.store().read_evidence(&first_ref).unwrap(),
                b"fresh edited words"
            );

            let key = fresh.start_job(&owner, 1).unwrap();
            let invalid = evidence(&mut owner, &[255], &[]).unwrap();
            let e = envelope(&mut owner);
            assert!(
                fresh
                    .choose_transcript(&mut owner, &e, invalid.reference.clone())
                    .is_err()
            );
            let e = envelope(&mut owner);
            assert!(
                fresh.accept_asr(&mut owner, &e, &key, &key, asr()).unwrap(),
                "S3_VOICE_INVALID_CHOICE_KEEPS_JOB"
            );
        }
        #[test]
        fn s3_voice_fix1_asr_admission_retry() {
            for index_read in [false, true] {
                eprintln!("S3_ASR_FAILURE_PHASE index_read={index_read}");
                let root = TempDirectory::new_in(&workspace_target_dir(), "s3-asr-admission");
                let (mut owner, anchor) = setup(root.root());
                let mut limits = owner.store().limits().clone();
                limits.owned_bytes = 16 << 20;
                owner.restrict_limits_probe(limits).unwrap();
                let mut saved = acknowledge(capture(root.root(), &owner, anchor, 1000), &mut owner);
                let key = saved.start_job(&owner, 0).unwrap();
                let (reference, bytes) = original_asr(&saved, &key);
                let e = envelope(&mut owner);
                let usage = owner.store().usage().unwrap();
                let mut pressure = Vec::new();
                if index_read {
                    let mut remaining = owner.store().limits().metadata_bytes
                        - usage.metadata_bytes
                        - usage.decoded_bytes
                        - (1 << 19);
                    while remaining > 0 {
                        let bytes = remaining.min(owner.store().limits().decoded_record_bytes);
                        pressure.push(owner.store().reserve_probe(bytes, 0).unwrap());
                        remaining -= bytes;
                    }
                } else {
                    pressure.push(
                        owner
                            .store()
                            .reserve_copy(
                                owner.store().limits().owned_bytes
                                    + owner.store().limits().metadata_bytes
                                    - (usage.physical_bytes - usage.copy_bytes)
                                    - usage.reserved_bytes
                                    - 512,
                            )
                            .unwrap(),
                    );
                }
                assert!(
                    matches!(
                        saved.accept_asr(&mut owner, &e, &key, &key, asr()),
                        Err(BundleError::Capacity(_))
                    ),
                    "S3_VOICE_ASR_ADMISSION_REFUSED"
                );
                drop(pressure);
                let chosen = if index_read {
                    None
                } else {
                    let mut editor = SavedAudio::reopen(&owner, &saved.voice.note_id).unwrap();
                    let text =
                        evidence(&mut owner, b"edited after admission refusal", &[]).unwrap();
                    let reference = text.reference.clone();
                    let e = envelope(&mut owner);
                    editor
                        .choose_transcript(&mut owner, &e, reference.clone())
                        .unwrap();
                    Some(reference)
                };
                assert!(
                    saved.retry_pending(&mut owner).unwrap(),
                    "S3_VOICE_ASR_COMPLETION_RETAINED"
                );
                assert_eq!(saved.voice.chosen_transcript_revision, chosen);
                assert_eq!(saved.voice.original_asr_ref, Some(reference.clone()));
                assert_eq!(&*owner.store().read_evidence(&reference).unwrap(), &bytes);
                drop(saved);
                drop(owner);
                let owner = LoadedBundle::read(&root.path("c0-red.kinewright"))
                    .unwrap()
                    .into_writer()
                    .unwrap();
                assert_eq!(&*owner.store().read_evidence(&reference).unwrap(), &bytes);
            }
        }
        #[test]
        fn s3_voice_fix1_asr_refusal_collection_retry() {
            let root = TempDirectory::new_in(&workspace_target_dir(), "s3-asr-refusal");
            let (mut owner, anchor) = setup(root.root());
            let mut saved = acknowledge(capture(root.root(), &owner, anchor, 1000), &mut owner);
            let note = saved.voice.note_id.clone();
            let key = saved.start_job(&owner, 0).unwrap();
            let (reference, bytes) = original_asr(&saved, &key);
            let stale = envelope(&mut owner);
            let core = kinewright_core::Core::spawn(
                (*owner
                    .branch_history(owner.manual_branch_id())
                    .unwrap()
                    .document)
                    .clone(),
            )
            .unwrap();
            core.restore_history(
                (*owner.branch_history(owner.manual_branch_id()).unwrap()).clone(),
            )
            .unwrap();
            move_for_asr(&mut owner, &core, 1);
            assert!(matches!(
                saved.accept_asr(&mut owner, &stale, &key, &key, asr()),
                Err(BundleError::HistoryChanged)
            ));
            move_for_asr(&mut owner, &core, 2);
            let mut editor = SavedAudio::reopen(&owner, &note).unwrap();
            let chosen = evidence(&mut owner, b"edited while ASR publication waited", &[]).unwrap();
            let e = envelope(&mut owner);
            editor
                .choose_transcript(&mut owner, &e, chosen.reference.clone())
                .unwrap();
            owner.collect_after_ack().unwrap();
            assert_eq!(
                &*owner
                    .store()
                    .read_evidence(&reference)
                    .expect("S3_VOICE_PENDING_ASR_PIN_SURVIVES_COLLECTION"),
                &bytes
            );
            assert!(
                saved.retry_pending(&mut owner).unwrap(),
                "S3_VOICE_DEFINITE_REFUSAL_REBASES"
            );
            assert_eq!(
                saved.voice.chosen_transcript_revision,
                Some(chosen.reference.clone())
            );
            assert!(
                saved.start_job(&owner, 1).is_ok(),
                "S3_VOICE_REFUSAL_DOES_NOT_WEDGE_JOBS"
            );
            drop(editor);
            drop(chosen);
            drop(core);
            drop(saved);
            drop(owner);
            let owner = LoadedBundle::read(&root.path("c0-red.kinewright"))
                .unwrap()
                .into_writer()
                .unwrap();
            let reopened = SavedAudio::reopen(&owner, &note).unwrap();
            assert_eq!(reopened.voice.original_asr_ref, Some(reference.clone()));
            assert_eq!(&*owner.store().read_evidence(&reference).unwrap(), &bytes);
        }
        #[test]
        fn s3_voice_fix1_unknown_reply_exact_replay() {
            let root = TempDirectory::new_in(&workspace_target_dir(), "s3-asr-unknown-reply");
            let (mut owner, anchor) = setup(root.root());
            let mut saved = acknowledge(capture(root.root(), &owner, anchor, 1000), &mut owner);
            let key = saved.start_job(&owner, 0).unwrap();
            let before = saved.voice.clone();
            let e = envelope(&mut owner);
            let generation = owner.root().unwrap().generation;
            assert!(saved.accept_asr(&mut owner, &e, &key, &key, asr()).unwrap());
            let accepted = saved.voice.clone();
            let reference = accepted.original_asr_ref.clone().unwrap();
            saved.voice = before;
            saved.pending_asr = Some(PendingAsr::Sealed {
                reference: reference.clone(),
                _pin: owner.store().pin(&[reference]).unwrap(),
            });
            saved.pending_write = Some(PendingVoiceWrite {
                envelope: e.clone(),
                update: MetadataUpdate {
                    expected_root_generation: generation,
                    change: MetadataChange::SaveVoice(accepted.clone()),
                },
            });
            let core = kinewright_core::Core::spawn(
                (*owner
                    .branch_history(owner.manual_branch_id())
                    .unwrap()
                    .document)
                    .clone(),
            )
            .unwrap();
            core.restore_history(
                (*owner.branch_history(owner.manual_branch_id()).unwrap()).clone(),
            )
            .unwrap();
            move_for_asr(&mut owner, &core, 1);
            assert_ne!(e.expected_head.as_ref(), owner.manual_head());
            let before_retry = owner.root().unwrap().generation;
            assert!(saved.retry_pending(&mut owner).unwrap());
            assert_eq!(
                owner.root().unwrap().generation,
                before_retry,
                "S3_VOICE_UNKNOWN_REPLAYS_WITHOUT_NEW_PUBLICATION"
            );
            assert_eq!(saved.voice, accepted);
            assert!(saved.pending_asr.is_none());
        }
        #[test]
        fn s3_voice_fix1_incomplete_capture_is_retained() {
            let root = TempDirectory::new_in(&workspace_target_dir(), "s3-incomplete-voice");
            let (mut owner, anchor) = setup(root.root());
            let mut captured = capture(root.root(), &owner, anchor, 1000);
            captured.complete = false;
            captured.capture_error = Some("FFmpeg exited before Stop".into());
            let saved = acknowledge(captured, &mut owner);
            let note = saved.voice.note_id.clone();
            assert!(!saved.complete);
            assert_eq!(
                saved.capture_error.as_deref(),
                Some("FFmpeg exited before Stop")
            );
            let audio = saved.voice.audio_ref.clone();
            drop(saved);
            drop(owner);
            let owner = LoadedBundle::read(&root.path("c0-red.kinewright"))
                .unwrap()
                .into_writer()
                .unwrap();
            let saved = SavedAudio::reopen(&owner, &note).unwrap();
            assert!(!saved.complete);
            assert_eq!(
                saved.capture_error.as_deref(),
                Some("FFmpeg exited before Stop")
            );
            assert_eq!(&*owner.store().read_evidence(&audio).unwrap(), &wav(1000));
        }
        #[derive(serde::Serialize, serde::Deserialize)]
        struct CrashReceipt {
            note: NoteId,
            anchor: FeedbackAnchor,
            audio_sha256: String,
            log_sha256: String,
        }
        fn crash(root: &Path, commentary: bool) -> CrashReceipt {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "recording::feedback_capture::tests::s3_voice_audio_retention_crash_child",
                    "--nocapture",
                ])
                .env("KINEWRIGHT_S3_C5_CRASH_ROOT", root)
                .env(
                    "KINEWRIGHT_S3_C5_COMMENTARY",
                    if commentary { "1" } else { "0" },
                )
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "crash child failed: {:?}\n{}\n{}",
                output.status,
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            serde_json::from_slice(&std::fs::read(root.join("ack.json")).unwrap()).unwrap()
        }
        #[test]
        fn s3_voice_audio_retention_crash_child() {
            let Some(root) = std::env::var_os("KINEWRIGHT_S3_C5_CRASH_ROOT") else {
                return;
            };
            let root = PathBuf::from(root);
            let (mut owner, anchor) = setup(&root);
            let mut captured = capture(&root, &owner, anchor.clone(), 1000);
            if std::env::var("KINEWRIGHT_S3_C5_COMMENTARY").as_deref() == Ok("1") {
                let mut second = anchor.clone();
                second.snapshot = snapshot(&mut owner);
                second.frame = 2;
                let log = captured.log.as_mut().unwrap();
                let mut paused = anchor.clone();
                paused.frame = 8;
                log.push(PlayheadSample {
                    audio_micros: 500_000,
                    displayed: paused,
                    event: PlayheadEvent::Pause,
                })
                .unwrap();
                let mut seek = anchor.clone();
                seek.frame = 3;
                log.push(PlayheadSample {
                    audio_micros: 1_000_000,
                    displayed: seek,
                    event: PlayheadEvent::Seek,
                })
                .unwrap();
                log.push(PlayheadSample {
                    audio_micros: 1_500_000,
                    displayed: second,
                    event: PlayheadEvent::VersionSwitch,
                })
                .unwrap();
            }
            let receipt = CrashReceipt {
                note: captured.note_id.clone(),
                anchor,
                audio_sha256: kinewright_media::sha256_bytes(&wav(1000)),
                log_sha256: kinewright_media::sha256_bytes(
                    &serde_json::to_vec(captured.log.as_ref().unwrap()).unwrap(),
                ),
            };
            let _saved = acknowledge(captured, &mut owner);
            std::fs::write(
                root.join("ack.partial"),
                serde_json::to_vec(&receipt).unwrap(),
            )
            .unwrap();
            std::fs::rename(root.join("ack.partial"), root.join("ack.json")).unwrap();
            std::process::exit(0);
        }
        #[test]
        fn s3_voice_audio_retention() {
            let _inference = INFERENCE_TESTS.lock().unwrap();
            let root = TempDirectory::new_in(&workspace_target_dir(), "s3-voice-retention");
            let receipt = crash(root.root(), false);
            let mut owner = LoadedBundle::read(&root.path("c0-red.kinewright"))
                .unwrap()
                .into_writer()
                .unwrap();
            let mut saved = SavedAudio::reopen(&owner, &receipt.note).unwrap();
            assert_eq!(saved.anchor, receipt.anchor, "S3_VOICE_ANCHOR_RESTART");
            assert_eq!(
                saved.voice.audio_ref.sha256, receipt.audio_sha256,
                "S3_VOICE_AUDIO_ACK_RESTART"
            );
            assert_eq!(
                &*owner.store().read_evidence(&saved.voice.audio_ref).unwrap(),
                &wav(1000)
            );
            assert_eq!(
                saved.voice.playhead_log_ref.as_ref().unwrap().sha256,
                receipt.log_sha256,
                "S3_VOICE_LOG_ACK_RESTART"
            );
            let key = saved.start_job(&owner, 0).unwrap();
            let mut task = saved
                .spawn_job(&owner, root.path("missing-model"), &key)
                .unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            let completion = loop {
                if let Some(done) = task.try_complete().unwrap() {
                    break done;
                }
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(5));
            };
            assert_eq!(completion.result.unwrap(), VoiceTranscript::AwaitingModel);
            drop(task);
            assert_eq!(saved.anchor, receipt.anchor);
            let e = envelope(&mut owner);
            assert!(
                saved
                    .accept_asr(&mut owner, &e, &key, &key, VoiceTranscript::AwaitingModel)
                    .unwrap()
            );
            let key = saved.start_job(&owner, 1).unwrap();
            let e = envelope(&mut owner);
            let original_voice = saved.voice.clone();
            let original_generation = owner.root().unwrap().generation;
            assert!(saved.accept_asr(&mut owner, &e, &key, &key, asr()).unwrap());
            let accepted_voice = saved.voice.clone();
            saved.pending_write = Some(PendingVoiceWrite {
                envelope: e,
                update: MetadataUpdate {
                    expected_root_generation: original_generation,
                    change: MetadataChange::SaveVoice(accepted_voice.clone()),
                },
            });
            saved.voice = original_voice;
            assert!(matches!(
                saved.start_job(&owner, 2),
                Err(BundleError::Unsettled)
            ));
            assert!(saved.retry_pending(&mut owner).unwrap());
            assert_eq!(saved.voice, accepted_voice);
            assert!(!saved.retry_pending(&mut owner).unwrap());
            let original_asr = saved.voice.original_asr_ref.clone().unwrap();
            let original_bytes = owner.store().read_evidence(&original_asr).unwrap().to_vec();
            let chosen = evidence(&mut owner, b"chosen editable transcript", &[]).unwrap();
            let e = envelope(&mut owner);
            saved
                .choose_transcript(&mut owner, &e, chosen.reference.clone())
                .unwrap();
            let text = evidence(&mut owner, b"private selected note", &[]).unwrap();
            let strokes = evidence(&mut owner, b"[]", &[]).unwrap();
            let proof = evidence(&mut owner, b"frame eight proof", &[]).unwrap();
            let revision = FeedbackRevision {
                comment_id: CommentId::fresh().unwrap(),
                revision: 1,
                predecessor: None,
                anchor: saved.anchor.clone(),
                original_text_ref: text.reference.clone(),
                strokes_ref: strokes.reference.clone(),
                frame_proof_ref: proof.reference.clone(),
                voice_ref: Some(saved.voice.clone()),
                transcript_revision: Some(chosen.reference.clone()),
            };
            let MetadataResult::Feedback(reference) = metadata(
                &mut owner,
                MetadataScope::Feedback,
                MetadataChange::SaveFeedback(revision),
            ) else {
                panic!("feedback")
            };
            let e = envelope(&mut owner);
            let batch = FeedbackBatchId::fresh().unwrap();
            let update = MetadataUpdate {
                expected_root_generation: owner.root().unwrap().generation,
                change: MetadataChange::SubmitBatch {
                    batch: FeedbackBatch {
                        batch_id: batch.clone(),
                        revision_refs: vec![reference],
                    },
                    request: ReviseRequest {
                        command_id: e.id.clone(),
                        batch_id: batch.clone(),
                        expected_candidate_head: owner.manual_head().unwrap().clone(),
                    },
                },
            };
            assert!(matches!(
                owner
                    .metadata_command(&e, MetadataScope::Feedback, &update)
                    .unwrap(),
                ReceiptLookup::Committed(_)
            ));
            let later = evidence(&mut owner, b"later edit", &[]).unwrap();
            let e = envelope(&mut owner);
            saved
                .choose_transcript(&mut owner, &e, later.reference.clone())
                .unwrap();
            let submitted = owner.read_submitted_batch(&batch).unwrap();
            assert_eq!(
                submitted.revisions[0].transcript_revision,
                Some(chosen.reference.clone())
            );
            assert_eq!(
                submitted.revisions[0]
                    .voice_ref
                    .as_ref()
                    .unwrap()
                    .original_asr_ref,
                Some(original_asr.clone())
            );
            assert_eq!(
                &*owner.store().read_evidence(&original_asr).unwrap(),
                &original_bytes,
                "S3_VOICE_ORIGINAL_ASR_RETAINED"
            );
            assert_eq!(
                &*owner.store().read_evidence(&saved.voice.audio_ref).unwrap(),
                &wav(1000)
            );
        }
        #[test]
        fn s3_commentary_retention() {
            let root = TempDirectory::new_in(&workspace_target_dir(), "s3-commentary-retention");
            let receipt = crash(root.root(), true);
            let mut owner = LoadedBundle::read(&root.path("c0-red.kinewright"))
                .unwrap()
                .into_writer()
                .unwrap();
            let mut saved = SavedAudio::reopen(&owner, &receipt.note).unwrap();
            let log_ref = saved.voice.playhead_log_ref.clone().unwrap();
            assert_eq!(
                log_ref.sha256, receipt.log_sha256,
                "S3_COMMENTARY_AUDIO_LOG_ATOMIC_ACK"
            );
            let log_bytes = owner.store().read_evidence(&log_ref).unwrap().to_vec();
            let log: PlayheadLog<FeedbackAnchor> = serde_json::from_slice(&log_bytes).unwrap();
            assert_eq!(saved.anchor, receipt.anchor);
            assert_eq!(log.samples[3].event, PlayheadEvent::VersionSwitch);
            let words = vec![
                VoiceWord {
                    text: "change this.".into(),
                    start_micros: 100_000,
                    end_micros: 1_200_000,
                },
                VoiceWord {
                    text: "next.".into(),
                    start_micros: 1_600_000,
                    end_micros: 2_200_000,
                },
            ];
            let key = saved.start_job(&owner, 0).unwrap();
            let e = envelope(&mut owner);
            assert!(
                saved
                    .accept_asr(
                        &mut owner,
                        &e,
                        &key,
                        &key,
                        VoiceTranscript::Ready(words.clone())
                    )
                    .unwrap()
            );
            let original_asr = saved.voice.original_asr_ref.clone().unwrap();
            let original_asr_bytes = owner.store().read_evidence(&original_asr).unwrap().to_vec();
            let mut remarks =
                commentary_remarks(&words, &log, saved.voice.duration_micros).unwrap();
            assert_eq!(remarks[0].playhead_anchor, receipt.anchor);
            assert_eq!(
                remarks[0]
                    .crossed_events
                    .iter()
                    .map(|e| e.event)
                    .collect::<Vec<_>>(),
                vec![PlayheadEvent::Pause, PlayheadEvent::Seek]
            );
            assert_eq!(
                remarks[1].playhead_anchor.snapshot,
                log.samples[3].displayed.snapshot
            );
            for range in [
                (100_000, 600_000),
                (600_000, 1_200_000),
                (100_000, 1_200_000),
            ] {
                assert_eq!(
                    saved.remark_voice(range.0, range.1).unwrap().audio_ref,
                    saved.voice.audio_ref
                );
            }
            let mut spoken = remarks[0].clone();
            spoken.text = "at 0 seconds".into();
            let chosen = anchor_remark(&owner, spoken).unwrap();
            assert_eq!(chosen.anchor.frame, 0);
            assert_eq!(chosen.spoken, SpokenTimecode::Parsed(0));
            assert_eq!(chosen.original.playhead_anchor, receipt.anchor);
            let mut outside = remarks[1].clone();
            outside.text = "at 22 seconds".into();
            let flagged = anchor_remark(&owner, outside).unwrap();
            assert_eq!(flagged.spoken, SpokenTimecode::OutOfRange);
            assert_eq!(flagged.anchor, remarks[1].playhead_anchor);
            let mut ambiguous = remarks[1].clone();
            ambiguous.text = "at one ten seconds".into();
            let flagged = anchor_remark(&owner, ambiguous).unwrap();
            assert_eq!(flagged.spoken, SpokenTimecode::Ambiguous);
            assert_eq!(flagged.anchor, remarks[1].playhead_anchor);
            remarks[0].text = "edited remark".into();
            remarks.remove(1);
            remarks.clear();
            assert_eq!(
                &*owner.store().read_evidence(&original_asr).unwrap(),
                &original_asr_bytes,
                "S3_COMMENTARY_ORIGINAL_ASR_RETAINED"
            );
            assert_eq!(
                &*owner.store().read_evidence(&log_ref).unwrap(),
                &log_bytes,
                "S3_COMMENTARY_ORIGINAL_LOG_RETAINED"
            );
            assert_eq!(
                &*owner.store().read_evidence(&saved.voice.audio_ref).unwrap(),
                &wav(1000),
                "S3_COMMENTARY_ORIGINAL_AUDIO_RETAINED"
            );
        }
        #[test]
        fn s3_voice_job_identity() {
            let root = TempDirectory::new_in(&workspace_target_dir(), "s3-voice-job-owner");
            let (mut owner, anchor) = setup(root.root());
            let mut first = acknowledge(
                capture(root.root(), &owner, anchor.clone(), 1000),
                &mut owner,
            );
            let first_key = first.start_job(&owner, 0).unwrap();
            let mut selected = anchor.clone();
            selected.snapshot = snapshot(&mut owner);
            selected.frame = 4;
            let mut second = acknowledge(
                capture(root.root(), &owner, selected.clone(), 2000),
                &mut owner,
            );
            let second_key = second.start_job(&owner, 0).unwrap();
            let e = envelope(&mut owner);
            assert!(
                !second
                    .accept_asr(&mut owner, &e, &first_key, &second_key, asr())
                    .unwrap(),
                "S3_VOICE_LATE_OTHER_NOTE"
            );
            assert!(
                owner.feedback_index().unwrap().notes[&second.voice.note_id]
                    .original_asr_ref
                    .is_none()
            );
            let e = envelope(&mut owner);
            assert!(
                first
                    .accept_asr(&mut owner, &e, &first_key, &first_key, asr())
                    .unwrap()
            );
            assert_eq!(first.anchor, anchor);
            assert_ne!(first.anchor.snapshot, selected.snapshot);
            let copy_path = root.path("other-project.kinewright");
            let kinewright_project::bundle_copy::SaveAsOutcome::Copied(copy) =
                owner.save_as(&copy_path).unwrap()
            else {
                panic!("new project copy")
            };
            let mut copy_owner = copy
                .into_destination()
                .unwrap_or_else(|_| panic!("committed copy"));
            let e = envelope(&mut copy_owner);
            let before = std::fs::read(&copy_path).unwrap();
            assert!(
                !second
                    .accept_asr(&mut copy_owner, &e, &second_key, &second_key, asr())
                    .unwrap(),
                "S3_VOICE_SOURCE_JOB_CANNOT_TARGET_COPY"
            );
            assert_eq!(std::fs::read(&copy_path).unwrap(), before);
            let mut copied_note = SavedAudio::reopen(&copy_owner, &second.voice.note_id).unwrap();
            let copy_key = copied_note.start_job(&copy_owner, 0).unwrap();
            assert_ne!(copy_key.project, second_key.project);
            assert_eq!(&copy_key.project, copy_owner.project_id());
            let e = envelope(&mut copy_owner);
            assert!(
                copied_note
                    .accept_asr(&mut copy_owner, &e, &copy_key, &copy_key, asr())
                    .unwrap()
            );
            assert!(
                owner.feedback_index().unwrap().notes[&second.voice.note_id]
                    .original_asr_ref
                    .is_none()
            );
            second.cancel();
            let e = envelope(&mut owner);
            assert!(
                !second
                    .accept_asr(&mut owner, &e, &second_key, &second_key, asr())
                    .unwrap()
            );
            assert_eq!(
                &*owner
                    .store()
                    .read_evidence(&second.voice.audio_ref)
                    .unwrap(),
                &wav(2000)
            );
            let mut edited_key = second_key.clone();
            edited_key.transcript_revision += 1;
            let fresh = second.start_job(&owner, 1).unwrap();
            let e = envelope(&mut owner);
            assert!(
                !second
                    .accept_asr(&mut owner, &e, &fresh, &edited_key, asr())
                    .unwrap(),
                "S3_VOICE_STALE_EDIT_REVISION"
            );
        }
        fn real_speech_control(commentary: bool) {
            use kinewright_media::{
                WHISPER_MODEL_NAME, WHISPER_MODEL_SHA256, default_data_dir, sha256_file,
                test_support::{SpeechClip, normalized_words, run_ffmpeg},
            };
            let model_root = std::env::var_os("KINEWRIGHT_S3_VOICE_MODEL_DATA_DIR")
                .map_or_else(default_data_dir, PathBuf::from);
            let model = model_root.join("models/whisper").join(WHISPER_MODEL_NAME);
            let control = if commentary {
                "s3_real_commentary_control"
            } else {
                "s3_real_spoken_note_control"
            };
            if !model.is_file() || sha256_file(&model).ok().as_deref() != Some(WHISPER_MODEL_SHA256)
            {
                eprintln!(
                    "NOT RUN {control}: pinned Whisper small is absent from {}",
                    model.display()
                );
                return;
            }
            let _inference = INFERENCE_TESTS.lock().unwrap();
            let root = TempDirectory::new_in(&workspace_target_dir(), control);
            let cache = root.path("voice-control-cache");
            std::fs::create_dir_all(cache.join("models/whisper")).unwrap();
            std::os::unix::fs::symlink(
                &model,
                cache.join("models/whisper").join(WHISPER_MODEL_NAME),
            )
            .unwrap();
            let (mut owner, anchor) = setup(root.root());
            let mut captured = capture(root.root(), &owner, anchor.clone(), 1000);
            let text = if commentary {
                "At zero seconds make this warmer. Keep the title on this version."
            } else {
                "Please make this chart warmer."
            };
            let speech = SpeechClip::plain(control, text, "KINEWRIGHT_S3_VOICE_CONTROL_AUDIO");
            run_ffmpeg(
                &[
                    "-i",
                    speech.mp4.to_str().unwrap(),
                    "-vn",
                    "-ac",
                    "1",
                    "-ar",
                    "16000",
                    "-c:a",
                    "pcm_s16le",
                ],
                &captured.path,
            );
            if commentary {
                let mut switched = anchor.clone();
                switched.snapshot = snapshot(&mut owner);
                switched.frame = 3;
                captured
                    .log
                    .as_mut()
                    .unwrap()
                    .push(PlayheadSample {
                        audio_micros: 1_000_000,
                        displayed: switched,
                        event: PlayheadEvent::VersionSwitch,
                    })
                    .unwrap();
            }
            let mut saved = acknowledge(captured, &mut owner);
            let key = saved.start_job(&owner, 0).unwrap();
            let mut task = saved.spawn_job(&owner, cache, &key).unwrap();
            let deadline = Instant::now() + Duration::from_secs(180);
            let completed = loop {
                if let Some(done) = task.try_complete().unwrap() {
                    break done;
                }
                assert!(
                    Instant::now() < deadline,
                    "CPU Whisper small control timed out"
                );
                std::thread::sleep(Duration::from_millis(10));
            };
            let transcript = completed.result.unwrap();
            let VoiceTranscript::Ready(words) = &transcript else {
                panic!("real speech must produce editable text")
            };
            let original_words = words.clone();
            let normalized = normalized_words(
                &words
                    .iter()
                    .map(|w| w.text.as_str())
                    .collect::<Vec<_>>()
                    .join(" "),
            );
            assert!(
                normalized.iter().any(|word| word == "warmer"),
                "real inference did not recognize the spoken request"
            );
            let e = envelope(&mut owner);
            assert!(
                saved
                    .accept_asr(&mut owner, &e, &key, &key, transcript)
                    .unwrap()
            );
            let original_ref = saved.voice.original_asr_ref.clone().unwrap();
            let original: kinewright_media::voice_note::OriginalVoiceAsr<
                ProjectId,
                NoteId,
                JobId,
                FeedbackAnchor,
            > = serde_json::from_slice(&owner.store().read_evidence(&original_ref).unwrap())
                .unwrap();
            assert_eq!(original.anchor, anchor);
            if commentary {
                let log: PlayheadLog<FeedbackAnchor> = serde_json::from_slice(
                    &owner
                        .store()
                        .read_evidence(saved.voice.playhead_log_ref.as_ref().unwrap())
                        .unwrap(),
                )
                .unwrap();
                let remarks =
                    commentary_remarks(&original_words, &log, saved.voice.duration_micros).unwrap();
                assert!(!remarks.is_empty());
                assert_eq!(remarks[0].playhead_anchor, anchor);
                for remark in remarks {
                    assert_eq!(
                        log.anchor_at(remark.start_micros),
                        Some(&remark.playhead_anchor)
                    );
                }
            }
            let edited = evidence(&mut owner, b"Please make it softer.", &[]).unwrap();
            let e = envelope(&mut owner);
            saved
                .choose_transcript(&mut owner, &e, edited.reference.clone())
                .unwrap();
            assert_eq!(saved.voice.original_asr_ref, Some(original_ref));
            assert_eq!(original.transcript, VoiceTranscript::Ready(original_words));
            assert_eq!(
                saved.voice.chosen_transcript_revision,
                Some(edited.reference)
            );
        }
        #[test]
        fn s3_real_spoken_note_control() {
            real_speech_control(false);
        }
        #[test]
        fn s3_real_commentary_control() {
            real_speech_control(true);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `IN1b` §7 item 16, label 10 of 17, **driven** (review-app-2 S3).
    ///
    /// Appendix B rows 111 and 112: `start_recording_from_dialog` refuses
    /// before it ever reaches `start_recording`, so no camera and no
    /// microphone need exist. The test lives here rather than in `app.rs`'s
    /// `in1_tests` because both the arm and
    /// [`KinewrightApp::note_recording_failure`] are private to this module.
    ///
    /// The subject is the project: a capture names no asset yet — the file it
    /// would have produced does not exist — and a device that keeps refusing
    /// is one problem, not one per attempt.
    #[test]
    fn in1b_recording_produces_its_declared_code_class_and_severity() {
        use crate::app::in1_tests::{in1_shutdown, in1b_app, in1b_declares, in1b_route_one};

        for (source, message) in [
            (RecordSource::Camera, "No camera is selected"),
            (RecordSource::Voice, "No microphone is selected"),
        ] {
            let (_fixture, mut app) = in1b_app();
            app.record_dialog.source = source;
            app.record_dialog.camera = None;
            app.record_dialog.microphone = None;
            app.start_recording_from_dialog();
            assert!(
                app.recording.is_none(),
                "the refusal is before any capture starts"
            );
            let incident = in1b_route_one(&mut app);
            in1b_declares(
                &incident,
                "recording_unclassified",
                kinewright_core::IncidentSubject::Project,
                kinewright_core::IncidentSeverity::Blocks,
            );
            assert_eq!(incident.observed, message);
            in1_shutdown(&mut app);
        }
    }

    #[cfg(windows)]
    fn joined(mode: &RecordingMode) -> String {
        ffmpeg_record_args(mode, Path::new("out.mp4"))
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[cfg(windows)]
    #[test]
    fn screen_capture_grabs_the_desktop_and_mixes_the_chosen_microphone() {
        let with_mic = joined(&RecordingMode::Screen {
            microphone: Some("Mic Array".to_owned()),
            monitor: None,
        });
        assert!(with_mic.contains("-f gdigrab -framerate 30 -i desktop"));
        assert!(with_mic.contains("-f dshow -i audio=Mic Array"));
        assert!(with_mic.contains("-c:v libx264 -preset ultrafast -pix_fmt yuv420p"));
        assert!(with_mic.contains("-fps_mode cfr -r 30"));
        assert!(with_mic.contains("-c:a aac"));
        assert!(with_mic.ends_with("-y out.mp4"));

        let silent = joined(&RecordingMode::Screen {
            microphone: None,
            monitor: None,
        });
        assert!(!silent.contains("dshow"));
        assert!(!silent.contains("-c:a"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn screen_capture_grabs_the_x11_root_and_mixes_the_chosen_microphone() {
        let with_mic = ffmpeg_record_args_linux(
            &RecordingMode::Screen {
                microphone: Some("alsa_input.pci-0000_00_1f.3.analog-stereo".to_owned()),
                monitor: None,
            },
            Path::new("out.mp4"),
            ":0.0",
        )
        .iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(" ");
        assert!(with_mic.contains("-f x11grab -framerate 30 -i :0.0"));
        assert!(with_mic.contains("-f pulse -i alsa_input.pci-0000_00_1f.3.analog-stereo"));
        assert!(with_mic.contains("-c:v libx264 -preset ultrafast -pix_fmt yuv420p"));
        assert!(with_mic.contains("-fps_mode cfr -r 30"));
        assert!(with_mic.contains("-c:a aac"));
        assert!(with_mic.ends_with("-y out.mp4"));

        let silent = ffmpeg_record_args_linux(
            &RecordingMode::Screen {
                microphone: None,
                monitor: None,
            },
            Path::new("out.mp4"),
            ":1",
        )
        .iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(" ");
        assert!(silent.contains("-i :1"));
        assert!(!silent.contains("pulse"));
        assert!(!silent.contains("alsa"));
        assert!(!silent.contains("-c:a"));
    }

    #[cfg(windows)]
    #[test]
    fn a_chosen_monitor_becomes_a_gdigrab_region_negative_offsets_included() {
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

    #[cfg(target_os = "linux")]
    #[test]
    fn a_chosen_monitor_becomes_an_x11grab_region_negative_offsets_included() {
        let monitor = MonitorInfo {
            label: "DP-2 · 2560×1440".to_owned(),
            x: -2560,
            y: 0,
            width: 2560,
            height: 1440,
            primary: false,
        };
        let args = ffmpeg_record_args_linux(
            &RecordingMode::Screen {
                microphone: None,
                monitor: Some(monitor),
            },
            Path::new("out.mp4"),
            ":0.0",
        )
        .iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(" ");
        assert!(args.contains("-video_size 2560x1440 -i :0.0+-2560,0"));
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
    fn xrandr_lines_parse_bounds_primary_and_negative_offsets() {
        let stdout = "Screen 0: minimum 320 x 200, current 4480 x 1440, maximum 16384 x 16384\n\
HDMI-1 connected primary 1920x1080+0+0 (normal left inverted right x axis y axis) 510mm x 290mm\n\
DP-2 connected 2560x1440+-2560+0 (normal left inverted right x axis y axis) 597mm x 336mm\n\
VNC-0 connected 1920x1200+0+0 0mm x 0mm\n\
DP-3 connected (normal left inverted right x axis y axis)\n\
HDMI-2 disconnected (normal left inverted right x axis y axis)\n";
        let monitors = parse_xrandr_monitors(stdout);
        assert_eq!(monitors.len(), 3);
        assert_eq!(monitors[0].label, "HDMI-1 (primary) · 1920×1080");
        assert!(monitors[0].primary);
        assert_eq!((monitors[0].x, monitors[0].y), (0, 0));
        assert_eq!(monitors[1].label, "DP-2 · 2560×1440");
        assert_eq!(monitors[1].x, -2560);
        assert_eq!((monitors[1].width, monitors[1].height), (2560, 1440));
        assert!(!monitors[1].primary);
        assert_eq!(monitors[2].label, "VNC-0 · 1920×1200");
        assert_eq!((monitors[2].width, monitors[2].height), (1920, 1200));
    }

    #[cfg(windows)]
    #[test]
    fn camera_capture_binds_video_and_audio_into_one_dshow_input() {
        let both = joined(&RecordingMode::Camera {
            camera: "Integrated Camera".to_owned(),
            microphone: Some("Mic".to_owned()),
        });
        assert!(both.contains("-f dshow -i video=Integrated Camera:audio=Mic"));
        assert!(both.contains("-fps_mode cfr -r 30"));
        let video_only = joined(&RecordingMode::Camera {
            camera: "Integrated Camera".to_owned(),
            microphone: None,
        });
        assert!(video_only.contains("-i video=Integrated Camera"));
        assert!(!video_only.contains("audio="));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn camera_capture_binds_v4l2_video_and_pulse_audio() {
        let both = ffmpeg_record_args_linux(
            &RecordingMode::Camera {
                camera: "/dev/video0".to_owned(),
                microphone: Some("default".to_owned()),
            },
            Path::new("out.mp4"),
            ":0.0",
        )
        .iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(" ");
        assert!(both.contains("-f v4l2 -framerate 30 -i /dev/video0"));
        assert!(both.contains("-f pulse -i default"));
        assert!(both.contains("-fps_mode cfr -r 30"));
        let video_only = ffmpeg_record_args_linux(
            &RecordingMode::Camera {
                camera: "/dev/video0".to_owned(),
                microphone: None,
            },
            Path::new("out.mp4"),
            ":0.0",
        )
        .iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(" ");
        assert!(video_only.contains("-i /dev/video0"));
        assert!(!video_only.contains("pulse"));
        assert!(!video_only.contains("-c:a"));
    }

    #[cfg(windows)]
    #[test]
    fn voice_capture_is_audio_only_aac() {
        let voice = joined(&RecordingMode::Voice {
            microphone: "Mic".to_owned(),
        });
        assert!(voice.contains("-f dshow -i audio=Mic"));
        assert!(voice.contains("-c:a aac"));
        assert!(!voice.contains("libx264"));
        assert!(!voice.contains("-fps_mode"));
        assert_eq!(
            RecordingMode::Voice {
                microphone: "Mic".to_owned()
            }
            .extension(),
            "m4a"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn voice_capture_is_audio_only_aac() {
        let voice = ffmpeg_record_args_linux(
            &RecordingMode::Voice {
                microphone: "hw:0".to_owned(),
            },
            Path::new("out.m4a"),
            ":0.0",
        )
        .iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(" ");
        assert!(voice.contains("-f alsa -i hw:0"));
        assert!(voice.contains("-c:a aac"));
        assert!(!voice.contains("libx264"));
        assert!(!voice.contains("-fps_mode"));
        assert_eq!(
            RecordingMode::Voice {
                microphone: "hw:0".to_owned()
            }
            .extension(),
            "m4a"
        );
    }

    #[test]
    fn ffmpeg_sources_listing_parses_names_and_skips_none() {
        let listing = "Auto-detected sources for v4l2:\n\
 /dev/video0 [Integrated Camera: Integrated Camera]\n\
 /dev/video1 [Integrated Camera: Integrated Camera]\n\
 none [None]\n\
Auto-detected sources for pulse:\n\
 alsa_input.pci-0000_00_1f.3.analog-stereo [Built-in Audio Analog Stereo]\n";
        assert_eq!(
            parse_ffmpeg_sources(listing),
            [
                "/dev/video0",
                "/dev/video1",
                "alsa_input.pci-0000_00_1f.3.analog-stereo"
            ]
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
            "kinewright-recording-name-test-{}",
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

    #[test]
    fn recording_directories_use_filesystem_safe_project_names() {
        assert_eq!(sanitize_recordings_folder("Project 1"), "Project 1");
        assert_eq!(
            sanitize_recordings_folder("  Interview: final?  "),
            "Interview_ final_"
        );
        assert_eq!(sanitize_recordings_folder("..."), "Project");
        assert_eq!(sanitize_recordings_folder("con"), "_con");
        assert_eq!(
            recordings_directory("Project: 2").file_name(),
            Some(std::ffi::OsStr::new("Project_ 2"))
        );
    }
}
