//! MS2 slice 1a: certified admission probes run in the confined Linux helper.
//! Pinning, byte identity and interpretation checks stay in the host. There
//! is no in-process fallback on Linux; Windows keeps its probe until 1b.
use crate::bundle::BundleError;
use kinewright_media::owned_io::{CertifiedProbe, CertifiedSource};
use std::sync::{Arc, atomic::AtomicBool};

#[cfg(target_os = "linux")]
pub use kinewright_media_client::{Config, Limits};

#[cfg(target_os = "linux")]
mod linux {
    use super::{Arc, AtomicBool, BundleError, CertifiedProbe, CertifiedSource};
    use kinewright_core::{ColorDescription, MediaKind, Rational, TimeCode};
    use kinewright_media::owned_io::{CertifiedStream, StreamKind};
    use kinewright_media_client::{
        Config, Failure, Limits, Phase, Request, Runtime, Source, Unavailable, UnavailableReason,
    };
    use kinewright_media_ipc::{Kind, MediaSandboxError, ProbeResult, ValidatedProbe};
    use std::{fs::File, path::Path, sync::OnceLock};

    static RUNTIME: OnceLock<Runtime> = OnceLock::new();

    /// Start the process's one probe runtime. A second call is refused.
    ///
    /// # Errors
    /// `Unsupported` for invalid or repeated configuration. `Contention`
    /// when the spawner thread could not start.
    pub fn configure_media_sandbox(config: Config) -> Result<(), BundleError> {
        let already =
            || unavailable(Phase::Runtime.unavailable(UnavailableReason::AlreadyConfigured));
        if RUNTIME.get().is_some() {
            return Err(already());
        }
        let runtime = Runtime::start(config).map_err(unavailable)?;
        RUNTIME.set(runtime).map_err(|_| already())
    }

    /// Packaged layout: the helper and supervisor sit beside the executable.
    ///
    /// # Errors
    /// As [`configure_media_sandbox`], or `Io` when the executable path is
    /// unknown.
    pub fn configure_media_sandbox_beside_executable() -> Result<(), BundleError> {
        let executable = std::env::current_exe().map_err(BundleError::Io)?;
        let directory = executable
            .parent()
            .ok_or_else(|| BundleError::Unsupported("executable has no directory".into()))?;
        configure_media_sandbox(config_in(directory))
    }
    fn config_in(directory: &Path) -> Config {
        Config::new(
            directory.join("kinewright-media-helper"),
            directory.join("kinewright-media-supervisor"),
            Limits::PROBE,
        )
    }

    #[cfg(any(test, feature = "ms2-test-bin-dir"))]
    fn test_runtime() -> &'static Runtime {
        // Test processes start the real confined runtime from the built
        // workspace binaries. Missing binaries fail loudly, never skip.
        RUNTIME.get_or_init(|| {
            let directory = std::env::var_os("KINEWRIGHT_MS2_TEST_BIN_DIR").map_or_else(
                || Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/debug"),
                std::path::PathBuf::from,
            );
            let directory = directory.canonicalize().unwrap_or_else(|error| {
                panic!(
                    "MS2 test binaries directory {} is missing ({error}); \
                     run `cargo build --workspace` or set KINEWRIGHT_MS2_TEST_BIN_DIR",
                    directory.display()
                )
            });
            let config = config_in(&directory);
            for binary in [&config.helper, &config.supervisor] {
                assert!(
                    binary.is_file(),
                    "MS2 binary {} is missing; run `cargo build --workspace` \
                     or set KINEWRIGHT_MS2_TEST_BIN_DIR",
                    binary.display()
                );
            }
            Runtime::start(config).expect("start the MS2 test probe runtime")
        })
    }

    /// The actor retains this capability until native cleanup, independently
    /// of the caller; the clone shares the pinned source, it copies nothing.
    struct Pinned(CertifiedSource);
    impl Source for Pinned {
        fn file(&self) -> &File {
            self.0.sandbox_descriptor()
        }
        fn identity(&self) -> [u8; 32] {
            self.0.identity().source_sha256
        }
        fn check(&self) -> Result<(), kinewright_core::MediaError> {
            self.0.check_read_authority()
        }
    }

    pub(crate) fn probe(
        source: &CertifiedSource,
        cancel: Option<Arc<AtomicBool>>,
    ) -> Result<CertifiedProbe, BundleError> {
        #[cfg(any(test, feature = "ms2-test-bin-dir"))]
        let runtime = Some(test_runtime());
        #[cfg(not(any(test, feature = "ms2-test-bin-dir")))]
        let runtime = RUNTIME.get();
        probe_with(runtime, source, cancel)
    }
    pub(crate) fn probe_with(
        runtime: Option<&Runtime>,
        source: &CertifiedSource,
        cancel: Option<Arc<AtomicBool>>,
    ) -> Result<CertifiedProbe, BundleError> {
        let runtime = runtime.ok_or_else(|| {
            unavailable(Phase::Runtime.unavailable(UnavailableReason::NotConfigured))
        })?;
        let index = |value: Option<usize>| -> Result<Option<u32>, BundleError> {
            value
                .map(|value| {
                    u32::try_from(value)
                        .map_err(|_| BundleError::Corrupt("selected stream index".into()))
                })
                .transpose()
        };
        let quarantine = |source: &CertifiedSource| {
            source
                .check_quarantine()
                .map_err(|error| BundleError::Corrupt(error.to_string()))
        };
        quarantine(source)?;
        #[cfg(any(test, feature = "test-util"))]
        super::CONFINED_PROBE_REQUESTS.set(super::CONFINED_PROBE_REQUESTS.get() + 1);
        let (video, audio) = source.selected_streams().unwrap_or((None, None));
        let (video, audio) = (index(video)?, index(audio)?);
        let pinned = Box::new(Pinned(source.clone()));
        let probe = match cancel {
            Some(cancel) => runtime.probe(Request {
                source: pinned,
                video,
                audio,
                cancel,
                explicit_retry: false,
            }),
            None => runtime.probe_until_deadline(pinned, video, audio),
        }
        .map_err(|failure| sandbox_error(&failure))?;
        // Admission rechecks host authority and byte identity after the
        // helper is gone.
        source
            .check_read_authority()
            .map_err(|error| BundleError::Corrupt(error.to_string()))?;
        quarantine(source)?;
        certified(probe)
    }

    fn sandbox_error(failure: &Failure) -> BundleError {
        match failure {
            Failure::Busy(_) => BundleError::Contention(failure.to_string()),
            Failure::Unavailable(error) => unavailable(*error),
            Failure::Sandbox(MediaSandboxError::SandboxUnavailable) => BundleError::Corrupt(
                "unexpected media sandbox refusal during the probe protocol".into(),
            ),
            Failure::Sandbox(_) | Failure::Authority(_) => {
                BundleError::Corrupt(failure.to_string())
            }
        }
    }

    fn unavailable(error: Unavailable) -> BundleError {
        if error.is_unsupported() {
            BundleError::Unsupported(error.to_string())
        } else {
            BundleError::Contention(error.to_string())
        }
    }

    #[cfg(test)]
    #[test]
    fn ms2_refusal_kind_and_message_include_the_bootstrap_cause() {
        use UnavailableReason::{
            EarlyEof, ExecutableUnavailable, HelperSpawn, NamespacePolicyDenied,
            RequiredNamespacesUnavailable, ScratchDirectory, ScratchDirectoryDenied,
            SupervisorSpawn,
        };

        for (phase, reason, detail) in [
            (
                Phase::Forked,
                RequiredNamespacesUnavailable,
                "helper Forked, RequiredNamespacesUnavailable",
            ),
            (
                Phase::Forked,
                NamespacePolicyDenied,
                "helper Forked, NamespacePolicyDenied",
            ),
            (
                Phase::Launch,
                ExecutableUnavailable,
                "launch, ExecutableUnavailable",
            ),
            (
                Phase::Launch,
                ScratchDirectoryDenied,
                "launch, ScratchDirectoryDenied",
            ),
            (
                Phase::Supervisor,
                ExecutableUnavailable,
                "supervisor startup, ExecutableUnavailable",
            ),
        ] {
            let error = sandbox_error(&phase.unavailable(reason).into());
            assert!(matches!(error, BundleError::Unsupported(_)), "{error:?}");
            assert_eq!(
                error.to_string(),
                format!("unsupported_bundle: media sandbox unavailable ({detail})")
            );
        }
        for (phase, reason, detail) in [
            (Phase::Boundary, EarlyEof, "helper Boundary, EarlyEof"),
            (Phase::Launch, HelperSpawn, "launch, HelperSpawn"),
            (Phase::Launch, ScratchDirectory, "launch, ScratchDirectory"),
            (
                Phase::Supervisor,
                SupervisorSpawn,
                "supervisor startup, SupervisorSpawn",
            ),
        ] {
            let error = sandbox_error(&phase.unavailable(reason).into());
            assert!(matches!(error, BundleError::Contention(_)), "{error:?}");
            assert_eq!(
                error.to_string(),
                format!("contention: media sandbox unavailable ({detail}). Try again.")
            );
        }
    }

    /// Domain values from boundary-checked metadata only.
    fn certified(probe: ValidatedProbe) -> Result<CertifiedProbe, BundleError> {
        let ProbeResult {
            streams,
            duration_microseconds,
            video_stream,
            audio_stream,
            duration_frames,
            fps,
            kind,
            resolution,
            color,
        } = probe.into_metadata();
        let bad = || BundleError::Corrupt("media sandbox: invalid validated probe".into());
        let index = |value: u32| usize::try_from(value).map_err(|_| bad());
        Ok(CertifiedProbe {
            streams: streams
                .into_iter()
                .map(|stream| {
                    Ok(CertifiedStream {
                        index: index(stream.index)?,
                        kind: match stream.kind {
                            kinewright_media_ipc::StreamKind::Video => StreamKind::Video,
                            kinewright_media_ipc::StreamKind::Audio => StreamKind::Audio,
                        },
                        time_base: (stream.time_base[0], stream.time_base[1]),
                        timestamp_origin: stream.timestamp_origin,
                        duration: stream.duration,
                        average_frame_rate: (
                            stream.average_frame_rate[0],
                            stream.average_frame_rate[1],
                        ),
                    })
                })
                .collect::<Result<_, BundleError>>()?,
            duration_microseconds,
            video_stream: video_stream.map(index).transpose()?,
            audio_stream: audio_stream.map(index).transpose()?,
            duration: TimeCode(duration_frames),
            fps: Rational::new(fps[0], fps[1]).map_err(|_| bad())?,
            kind: match kind {
                Kind::Video => MediaKind::Video,
                Kind::Audio => MediaKind::Audio,
                Kind::AudioVideo => MediaKind::AudioVideo,
                Kind::Image => MediaKind::Image,
            },
            resolution: resolution.map(|[w, h]| (w, h)),
            color_description: ColorDescription {
                primaries: color.primaries,
                transfer: color.transfer,
                matrix: color.matrix,
                range: color.range,
                white_point: color.white_point,
                bit_depth: color.bit_depth,
                confidence_basis_points: color.confidence_basis_points,
                provenance: color.provenance,
            },
        })
    }
}
#[cfg(all(test, target_os = "linux"))]
pub(crate) use linux::probe_with;
#[cfg(target_os = "linux")]
pub use linux::{configure_media_sandbox, configure_media_sandbox_beside_executable};

#[cfg(any(test, feature = "test-util"))]
thread_local! {
    static CONFINED_PROBE_REQUESTS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Count confined probe requests sent to the helper by this thread during an
/// operation; the twin of `observe_stream_info_probes` for the MS2 path.
#[cfg(any(test, feature = "test-util"))]
pub fn observe_confined_probes<T>(operation: impl FnOnce() -> T) -> (T, u64) {
    let before = CONFINED_PROBE_REQUESTS.get();
    let result = operation();
    (result, CONFINED_PROBE_REQUESTS.get() - before)
}

/// Windows confinement is slice 1b; until then its probe stays in process.
///
/// # Errors
/// None yet; the signature matches Linux, which can refuse.
#[cfg(not(target_os = "linux"))]
#[allow(clippy::unnecessary_wraps, reason = "matches the Linux signature")]
pub fn configure_media_sandbox_beside_executable() -> Result<(), BundleError> {
    Ok(())
}

pub(crate) fn probe(
    source: &CertifiedSource,
    cancel: Option<Arc<AtomicBool>>,
) -> Result<CertifiedProbe, BundleError> {
    #[cfg(target_os = "linux")]
    {
        linux::probe(source, cancel)
    }
    #[cfg(not(target_os = "linux"))]
    {
        source
            .probe_with_cancellation(cancel)
            .map_err(|error| BundleError::Corrupt(error.to_string()))
    }
}

#[cfg(all(test, target_os = "linux"))]
#[path = "media_sandbox_tests.rs"]
mod tests;
