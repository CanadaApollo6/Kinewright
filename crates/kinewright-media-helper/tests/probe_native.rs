#![cfg(target_os = "linux")]
#![allow(unsafe_code)]
#[path = "../../kinewright-job-sys/test_support/deadline.rs"]
mod deadline;

use kinewright_job_sys::media_boundary;
use kinewright_media_ipc::{
    Frame, MediaSandboxError, Message, ProbeResult, ProbeSession, read_frame, write_frame,
};
use std::{
    fs::{self, File},
    io::{self, Read, Write},
    os::fd::{AsFd, AsRawFd},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/ms2-s1a")
            .join(format!(
                "native-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
        fs::create_dir_all(root.parent().unwrap()).unwrap();
        fs::create_dir(&root).unwrap();
        Self(root)
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
struct Running {
    child: Child,
    reaped: bool,
}
impl Drop for Running {
    fn drop(&mut self) {
        if !self.reaped {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}
struct DeadlineReader {
    file: File,
    end: Instant,
}
impl Read for DeadlineReader {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        media_boundary::wait_readable(
            self.file.as_fd(),
            self.end.saturating_duration_since(Instant::now()),
        )?;
        self.file.read(output)
    }
}
fn ffmpeg(output: &Path, still: bool) {
    let mut command = Command::new("ffmpeg");
    command.args([
        "-v",
        "error",
        "-f",
        "lavfi",
        "-i",
        "color=c=red:size=64x32:rate=30",
        "-threads",
        "1",
    ]);
    if still {
        command.args([
            "-frames:v",
            "1",
            "-c:v",
            if output.extension().is_some_and(|v| v == "jpg") {
                "mjpeg"
            } else {
                "png"
            },
        ]);
    } else {
        command.args(["-t", "1", "-c:v", "libx264", "-pix_fmt", "yuv420p"]);
    }
    let result = command.arg(output).stdin(Stdio::null()).output().unwrap();
    assert!(result.status.success(), "{result:?}");
}
fn send(file: &mut File, message: Message) {
    write_frame(
        file,
        &Frame {
            request: 1,
            generation: 1,
            message,
        },
    )
    .unwrap();
}
fn inspect_boundary(pid: u32) {
    let status = fs::read_to_string(format!("/proc/{pid}/status")).unwrap();
    assert!(status.lines().any(|line| line == "Seccomp:\t2"), "{status}");
    assert!(
        status.lines().any(|line| line == "NoNewPrivs:\t1"),
        "{status}"
    );
    for namespace in ["user", "mnt", "net", "pid"] {
        match fs::read_link(format!("/proc/{pid}/ns/{namespace}")) {
            Ok(actual) => assert_ne!(
                actual,
                fs::read_link(format!("/proc/self/ns/{namespace}")).unwrap()
            ),
            Err(error) => eprintln!(
                "MS2_UNOBSERVABLE {namespace}: {error}; trusted bootstrap and negative tests required"
            ),
        }
    }
}

fn run(directory: &Directory, source: &Path) -> Result<ProbeResult, MediaSandboxError> {
    let root = directory.0.join("root");
    fs::create_dir(&root).unwrap();
    let source = File::open(source).unwrap();
    let (command_read, command_write) =
        rustix::pipe::pipe_with(rustix::pipe::PipeFlags::CLOEXEC).unwrap();
    let (response_read, response_write) =
        rustix::pipe::pipe_with(rustix::pipe::PipeFlags::CLOEXEC).unwrap();
    let (lifetime_read, _lifetime_write) =
        rustix::pipe::pipe_with(rustix::pipe::PipeFlags::CLOEXEC).unwrap();
    let (release_read, release_write) =
        rustix::pipe::pipe_with(rustix::pipe::PipeFlags::CLOEXEC).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_kinewright-media-helper"));
    command
        .args([
            std::process::id().to_string(),
            root.to_str().unwrap().into(),
            "degraded".into(),
            (2_u64 * 1024 * 1024 * 1024).to_string(),
            (1024_u64 * 1024 * 1024).to_string(),
        ])
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // SAFETY: all five original descriptors stay owned through spawn; the child
    // receives only copies, in the reserved slots, after exec clears CLOEXEC.
    unsafe {
        media_boundary::inherit_media_fds(
            &mut command,
            [
                command_read.as_raw_fd(),
                response_write.as_raw_fd(),
                lifetime_read.as_raw_fd(),
                source.as_raw_fd(),
                release_read.as_raw_fd(),
            ],
            None,
        );
    }
    let mut child = Running {
        child: command.spawn().unwrap(),
        reaped: false,
    };
    drop((
        command_read,
        response_write,
        lifetime_read,
        release_read,
        source,
    ));
    let mut command = File::from(command_write);
    let mut response = DeadlineReader {
        file: File::from(response_read),
        end: Instant::now() + Duration::from_secs(15),
    };
    File::from(release_write).write_all(&[1]).unwrap();
    let forked = read_frame(&mut response).unwrap();
    let Message::Forked { pid } = forked.message else {
        panic!("{forked:?}");
    };
    send(&mut command, Message::EnterBoundary {});
    let boundary = read_frame(&mut response).unwrap();
    assert!(matches!(boundary.message, Message::Boundary { .. }));
    inspect_boundary(pid);
    send(&mut command, Message::ConfirmBoundary {});
    let mut session = ProbeSession::new(1, 1, [None, None]);
    assert!(
        session
            .accept(read_frame(&mut response).unwrap())
            .unwrap()
            .is_none()
    );
    send(
        &mut command,
        Message::Probe {
            video: None,
            audio: None,
        },
    );
    let result = session.accept(read_frame(&mut response).unwrap());
    let result = match result {
        Ok(Some(metadata)) => {
            session.accept(read_frame(&mut response).unwrap()).unwrap();
            Ok(metadata.into_metadata())
        }
        Err(error) => Err(error),
        other => panic!("unexpected probe reply {other:?}"),
    };
    send(&mut command, Message::Close {});
    let status = child.child.wait().unwrap();
    child.reaped = true;
    assert!(status.success());
    result
}
#[test]
fn confined_video_probe_decodes_a_first_frame_and_closes() {
    let _deadline = deadline::test();
    let directory = Directory::new();
    let source = directory.0.join("video.mp4");
    ffmpeg(&source, false);
    let result = run(&directory, &source).unwrap();
    assert_eq!(result.resolution, Some([64, 32]));
    assert_eq!(result.duration_frames, 30);
}
/// `FFmpeg`'s `max_pixels` check counts the decoder's padded allocation, so
/// SD (720 wide, off the stride alignment), tiny pictures and MPEG-4 Part 2
/// (16-pixel edges) need the reserved envelope, not the declared picture.
#[test]
fn confined_video_probe_admits_padded_decoder_allocations() {
    let _deadline = deadline::test();
    for (size, codec, name, expected) in [
        ("720x480", "libx264", "sd.mkv", [720, 480]),
        ("32x24", "libx264", "tiny.mkv", [32, 24]),
        ("64x64", "mpeg4", "part2.avi", [64, 64]),
    ] {
        let directory = Directory::new();
        let source = directory.0.join(name);
        let result = Command::new("ffmpeg")
            .args(["-v", "error", "-f", "lavfi", "-i"])
            .arg(format!("color=c=red:size={size}:rate=24"))
            .args(["-t", "0.5", "-c:v", codec, "-pix_fmt", "yuv420p"])
            .arg(&source)
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(result.status.success(), "{result:?}");
        let result = run(&directory, &source).unwrap_or_else(|error| panic!("{name}: {error:?}"));
        assert_eq!(result.resolution, Some(expected), "{name}");
    }
}
#[test]
fn confined_still_probe_inspects_and_closes() {
    let _deadline = deadline::test();
    for extension in ["png", "jpg"] {
        let directory = Directory::new();
        let source = directory.0.join(format!("still.{extension}"));
        ffmpeg(&source, true);
        let result = run(&directory, &source).unwrap();
        assert_eq!(result.resolution, Some([64, 32]));
        assert_eq!(result.kind, kinewright_media_ipc::Kind::Image);
        assert_eq!(result.duration_frames, 1);
        if extension == "jpg" {
            assert_eq!(
                result.color.matrix,
                kinewright_core::ColorMatrix::Other("bt470bg".into())
            );
        }
    }
}
#[test]
fn confined_malformed_probe_refuses() {
    let _deadline = deadline::test();
    let directory = Directory::new();
    let source = directory.0.join("bad.mp4");
    fs::write(&source, b"bad-media").unwrap();
    assert_eq!(
        run(&directory, &source).unwrap_err(),
        MediaSandboxError::UnsupportedMedia
    );
}

#[test]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn confined_uhd_av1_probe_uses_the_final_filter() {
    let _deadline = deadline::test();
    let directory = Directory::new();
    let source = directory.0.join("uhd-av1.mkv");
    let result = Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "color=c=blue:size=3840x2160:rate=1",
            "-frames:v",
            "2",
            "-c:v",
            "libaom-av1",
            "-cpu-used",
            "8",
            "-threads",
            "1",
            "-row-mt",
            "0",
        ])
        .arg(&source)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(result.status.success(), "{result:?}");
    let result = run(&directory, &source).unwrap();
    assert_eq!(result.resolution, Some([3840, 2160]));
}

#[test]
fn confined_audio_probes_keep_the_channel_and_rate_bounds() {
    let _deadline = deadline::test();
    for (codec, extension) in [("pcm_s16le", "wav"), ("libmp3lame", "mp3")] {
        let directory = Directory::new();
        let source = directory.0.join(format!("audio.{extension}"));
        let fixture = Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:sample_rate=48000",
                "-t",
                "1",
                "-c:a",
                codec,
                "-threads",
                "1",
            ])
            .arg(&source)
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(fixture.status.success(), "{fixture:?}");
        let result = run(&directory, &source).unwrap();
        assert_eq!(result.kind, kinewright_media_ipc::Kind::Audio);
        assert_eq!(result.resolution, None);
        assert!(result.duration_frames > 0);
    }
}
