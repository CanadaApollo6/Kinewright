//! Audited exec/bootstrap FFI and ownership of inherited descriptors.
use kinewright_job_sys::media_boundary::MediaFile;
use kinewright_job_sys::media_boundary::{self as boundary, Mode};
use kinewright_media_ipc::{Frame, Message, read_frame, write_frame};
use std::{
    io::{self, Read},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

fn inherited(fd: i32) -> MediaFile {
    // SAFETY: this separately exec'd binary takes each reserved inherited fd once,
    // after fork has given the shim and payload independent descriptor ownership.
    unsafe { boundary::inherited_file(fd) }
}
fn expect(reader: &mut impl Read, expected: fn(&Message) -> bool) -> io::Result<Frame> {
    let frame = read_frame(reader)?;
    if !expected(&frame.message) {
        return Err(io::Error::other("media bootstrap sequence"));
    }
    Ok(frame)
}
fn send(writer: &mut MediaFile, request: u32, generation: u64, message: Message) -> io::Result<()> {
    write_frame(
        writer,
        &Frame {
            request,
            generation,
            message,
        },
    )
}
struct Control {
    active: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
}
fn watchdog(command: Arc<MediaFile>, request_id: u32, generation: u64) -> io::Result<Control> {
    let active = Arc::new(AtomicBool::new(false));
    let cancel = Arc::new(AtomicBool::new(false));
    let control = command;
    let monitoring = Arc::clone(&active);
    let interrupt = Arc::clone(&cancel);
    let thread_ready = Arc::new(AtomicBool::new(false));
    let ready = Arc::clone(&thread_ready);
    let _watchdog = std::thread::Builder::new()
        .stack_size(512 * 1024)
        .spawn(move || {
            ready.store(true, Ordering::Release);
            loop {
                match boundary::watchdog_poll(monitoring.load(Ordering::Acquire)) {
                    Ok(true) => match read_frame(&mut control.as_ref()) {
                        Ok(frame)
                            if frame.request == request_id
                                && frame.generation == generation
                                && matches!(
                                    frame.message,
                                    Message::Cancel {} | Message::Close {}
                                ) =>
                        {
                            interrupt.store(true, Ordering::Release);
                        }
                        _ => boundary::exit_helper(125),
                    },
                    Ok(false) => {}
                    Err(_) => boundary::exit_helper(125),
                }
            }
        })?;
    // Rust's thread startup performs runtime queries before entering the closure.
    // Degraded mode installs TSYNC only once that trusted initialization is complete.
    while !thread_ready.load(Ordering::Acquire) {
        std::thread::yield_now();
    }
    Ok(Control { active, cancel })
}

pub fn run() -> io::Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    #[cfg(feature = "slow-tests")]
    let witness = crate::witness::Witness::Off;
    #[cfg(feature = "slow-tests")]
    let (args, witness) = if crate::WITNESS_EXECUTABLE {
        let mut args = args;
        let stage = args
            .pop()
            .ok_or_else(|| io::Error::other("witness stage"))?;
        let witness = crate::witness::Witness::parse(&stage)?;
        (args, witness)
    } else {
        (args, witness)
    };
    run_inner(
        &args,
        #[cfg(feature = "slow-tests")]
        witness,
    )
}

fn run_inner(
    args: &[String],
    #[cfg(feature = "slow-tests")] witness: crate::witness::Witness,
) -> io::Result<()> {
    if args.len() != 4 {
        return Err(io::Error::other("media bootstrap arguments"));
    }
    let app_pid = args[0].parse().map_err(io::Error::other)?;
    let root = PathBuf::from(&args[1]);
    let mode = match args[2].as_str() {
        "scope" => Mode::Scope,
        "degraded" => Mode::Degraded,
        _ => return Err(io::Error::other("media mode")),
    };
    // This is an admitted host limit. Slice 1a measurements freeze its production value.
    let budget: u64 = args[3].parse().map_err(io::Error::other)?;
    if !(64 * 1024 * 1024..=2 * 1024 * 1024 * 1024).contains(&budget) {
        return Err(io::Error::other("probe reservation"));
    }
    // SAFETY: fd 8, when present, is the sole optional inherited thread pidfd;
    // the helper has not opened files or created another owner for that descriptor.
    let spawner = unsafe { boundary::inherited_spawner() };
    #[cfg(feature = "slow-tests")]
    witness.before_shim_arm(app_pid, spawner.as_ref())?;
    boundary::arm_parent_death(app_pid, spawner.as_ref())?;
    boundary::new_session()?;
    let mut release = inherited(7);
    let mut token = [0];
    release.read_exact(&mut token)?;
    if token != [1] {
        return Err(io::Error::other("scope join not released"));
    }
    drop(release);
    boundary::enter_namespaces(app_pid, spawner.as_ref())?;
    drop(spawner);
    let parent = boundary::pidfd(std::process::id(), false)?;
    if let Some(pid) = kinewright_job_sys::fork_payload_init()? {
        drop(parent);
        let mut response = inherited(boundary::RESPONSE);
        send(&mut response, 1, 1, Message::Forked { pid })?;
        drop(response);
        boundary::close_shim_fds()?;
        let status = kinewright_job_sys::wait_payload_init(pid)?;
        std::process::exit(status);
    }
    #[cfg(feature = "slow-tests")]
    witness.before_arm(&parent)?;
    boundary::arm_parent_death(0, Some(&parent))?;
    drop(parent);
    payload(
        &root,
        mode,
        budget,
        #[cfg(feature = "slow-tests")]
        witness,
    )
}

fn payload(
    root: &std::path::Path,
    mode: Mode,
    budget: u64,
    #[cfg(feature = "slow-tests")] witness: crate::witness::Witness,
) -> io::Result<()> {
    let command = Arc::new(inherited(boundary::COMMAND));
    let mut reader = command.as_ref();
    let mut response = inherited(boundary::RESPONSE);
    // Take ownership before lockdown: Rust may validate a raw descriptor with
    // F_GETFD. This transfers the existing capability without reading any media.
    let source = inherited(boundary::SOURCE);
    let start = expect(&mut reader, |message| {
        matches!(message, Message::EnterBoundary {})
    })?;
    // No source reads or parser calls have occurred before resource join and
    // namespace setup. Prime runtime CPU detection before the filesystem disappears.
    prime_runtime(budget)?;
    boundary::close_inventory()?;
    boundary::empty_root(root)?;
    boundary::limits(budget)?;
    let landlock = boundary::landlock();
    let control = control(
        Arc::clone(&command),
        &start,
        #[cfg(feature = "slow-tests")]
        witness,
    )?;
    boundary::install_filter(mode)?;
    send(
        &mut response,
        start.request,
        start.generation,
        Message::Boundary { landlock },
    )?;
    #[cfg(feature = "slow-tests")]
    witness.before_ready();
    let confirmation = expect(&mut reader, |message| {
        matches!(message, Message::ConfirmBoundary {})
    })?;
    if confirmation.request != start.request || confirmation.generation != start.generation {
        return Err(io::Error::other("stale media bootstrap confirmation"));
    }
    boundary::drop_dumpable()?;
    send(
        &mut response,
        start.request,
        start.generation,
        Message::Ready {},
    )?;
    let request = read_frame(&mut reader)?;
    if request.request != start.request || request.generation != start.generation {
        return Err(io::Error::other("stale media probe request"));
    }
    let Message::Probe { video, audio } = request.message else {
        return Err(io::Error::other("probe request required"));
    };
    let length = boundary::source_length()?;
    control.active.store(true, Ordering::Release);
    match crate::probe::probe(
        source,
        length,
        [video, audio],
        budget,
        Arc::clone(&control.cancel),
        || {
            #[cfg(feature = "slow-tests")]
            witness.decoder_checkpoint(&mut response, request.request, request.generation);
        },
    ) {
        Ok(result) => {
            send(
                &mut response,
                request.request,
                request.generation,
                Message::ProbeResult {
                    result: Box::new(result),
                },
            )?;
            send(
                &mut response,
                request.request,
                request.generation,
                Message::Complete {},
            )?;
        }
        Err(error) => send(
            &mut response,
            request.request,
            request.generation,
            Message::Failed {
                error: if control.cancel.load(Ordering::Acquire) {
                    kinewright_media_ipc::MediaSandboxError::Cancelled
                } else {
                    error
                },
            },
        )?,
    }
    // Keep the one-shot lease alive until the host consumes Complete and closes
    // it. The watchdog alone reads post-Probe commands; EOF/lifetime loss remains
    // fatal, and host wall deadlines retain non-cooperative cleanup authority.
    while !control.cancel.load(Ordering::Acquire) {
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    Ok(())
}

fn control(
    command: Arc<MediaFile>,
    start: &Frame,
    #[cfg(feature = "slow-tests")] witness: crate::witness::Witness,
) -> io::Result<Control> {
    #[cfg(feature = "slow-tests")]
    if witness != crate::witness::Witness::Off {
        // Only the distinct witness executable can choose a mode without the
        // cooperative monitor. Production always starts the trusted watchdog.
        return Ok(Control {
            active: Arc::new(AtomicBool::new(false)),
            cancel: Arc::new(AtomicBool::new(false)),
        });
    }
    watchdog(command, start.request, start.generation)
}

fn prime_runtime(budget: u64) -> io::Result<()> {
    ffmpeg_next::init().map_err(io::Error::other)?;
    ffmpeg_next::log::set_level(ffmpeg_next::log::Level::Quiet);
    // SAFETY: CPU feature/count queries read runtime configuration only. The
    // process-global native allocation ceiling is initialized before any parser.
    unsafe {
        ffmpeg_next::ffi::av_get_cpu_flags();
        ffmpeg_next::ffi::av_cpu_count();
        ffmpeg_next::ffi::av_max_alloc(
            usize::try_from((budget / 2).min(256 * 1024 * 1024)).map_err(io::Error::other)?,
        );
    }
    if std::fs::read_dir("/proc/self/task")?.count() != 1 {
        return Err(io::Error::other("media runtime created bootstrap threads"));
    }
    Ok(())
}
