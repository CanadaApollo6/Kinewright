//! S4b C6 fix: the installed CLI's own `--version`, read under the guardian.
//!
//! The probe runs the sealed image of the identity-checked executable with the
//! containment of a launch (the guardian's namespaces, Landlock allowlist and
//! seccomp), plus the unpaid policy's loopback-only network namespace whose
//! relay goes nowhere. It has no credentials: the private store is empty and
//! nothing is mounted. Its output is read for a bounded time and size, and the
//! whole unit is terminated and reaped before the version is accepted.
use crate::orchestration::adapters::{CertificationRefusal, CliVersion, ExecutableIdentity};
use std::{
    io::{self, Read},
    os::unix::fs::{DirBuilderExt, MetadataExt},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

/// The longest a `--version` run may take before its unit is stopped.
const DEADLINE: Duration = Duration::from_secs(10);
/// The most output a `--version` run may write.
const OUTPUT_CAP: usize = 4096;
/// The loopback port of the probe's own network namespace. Nothing is relayed
/// from it: the host never starts the relay.
const PROBE_PORT: u16 = 47_731;

/// The exact output form of each provider's `--version`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum VersionForm {
    /// `codex-cli X.Y.Z`
    Codex,
    /// `X.Y.Z (Claude Code)`
    Claude,
}

impl VersionForm {
    /// One UTF-8 line in the provider's exact form, with at most one trailing
    /// newline. The version field must be a plain release `X.Y.Z`; a
    /// prerelease, a second line, extra words or any other shape is refused.
    pub(crate) fn parse(self, output: &[u8]) -> Result<CliVersion, CertificationRefusal> {
        let refused = CertificationRefusal::UnqualifiedVersion;
        let text = std::str::from_utf8(output).map_err(|_| refused)?;
        let line = text.strip_suffix('\n').unwrap_or(text);
        if line.contains(['\n', '\r']) {
            return Err(refused);
        }
        let field = match self {
            Self::Codex => line.strip_prefix("codex-cli "),
            Self::Claude => line.strip_suffix(" (Claude Code)"),
        }
        .ok_or(refused)?;
        CliVersion::parse(field)
    }
}

/// How one provider's version is read.
pub(crate) struct VersionProbe<'a> {
    pub guardian: &'a Path,
    pub form: VersionForm,
    /// Grant the executable's own install tree, as its launch does.
    pub user_local_runtime: bool,
    /// The person's credential folder, which no grant may reach.
    pub credential_root: Option<PathBuf>,
}

/// What the contained run wrote and how it ended.
struct Run {
    output: Vec<u8>,
    exited_zero: bool,
}

struct ProbeOwner;
impl crate::guardian::DurableAttemptOwner for ProbeOwner {
    fn reserve(&mut self, _: &crate::job_guardian::protocol::AttemptIdentity) -> io::Result<()> {
        Ok(())
    }
    fn authorize_commit(
        &mut self,
        _: &crate::job_guardian::protocol::RecoveryIdentity,
    ) -> io::Result<()> {
        Ok(())
    }
}

/// The installed CLI's version, read under the guardian. The executable's
/// identity is checked before its bytes are measured and sealed, and again
/// after the unit is gone.
pub(crate) fn installed_version(
    probe: &VersionProbe<'_>,
    executable: &ExecutableIdentity,
) -> Result<CliVersion, CertificationRefusal> {
    static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let root = std::env::temp_dir().join(format!(
        "kvp-{}-{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&root)
        .map_err(|_| CertificationRefusal::SandboxUnavailable)?;
    let run = contained(probe, executable, &root);
    let _ = std::fs::remove_dir_all(&root);
    let run = run?;
    identity(executable)?;
    if !run.exited_zero {
        return Err(CertificationRefusal::UnqualifiedVersion);
    }
    probe.form.parse(&run.output)
}

fn identity(executable: &ExecutableIdentity) -> Result<std::fs::File, CertificationRefusal> {
    let file = std::fs::File::open(&executable.path)
        .map_err(|_| CertificationRefusal::ExecutableIdentity)?;
    let metadata = file
        .metadata()
        .map_err(|_| CertificationRefusal::ExecutableIdentity)?;
    if !metadata.is_file() || [metadata.dev(), metadata.ino()] != executable.file_identity {
        return Err(CertificationRefusal::ExecutableIdentity);
    }
    Ok(file)
}

fn contained(
    probe: &VersionProbe<'_>,
    executable: &ExecutableIdentity,
    root: &Path,
) -> Result<Run, CertificationRefusal> {
    use crate::job_guardian::protocol::AttemptIdentity;
    let sandbox = |_| CertificationRefusal::SandboxUnavailable;
    // The sealed image holds exactly the bytes of the identity-checked file.
    let digest =
        kinewright_media::sha256_reader(&mut identity(executable)?.take(512 * 1024 * 1024 + 1))
            .map_err(|_| CertificationRefusal::ExecutableIdentity)?;
    let image = kinewright_job_sys::seal_executable(&executable.path, &digest)
        .map_err(|_| CertificationRefusal::ExecutableIdentity)?;
    let scratch = root.join("s");
    let store = scratch.join("c");
    for directory in [&scratch, &store] {
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(directory)
            .map_err(sandbox)?;
    }
    let listeners = crate::provider_pipes::ProviderPipeListeners::bind(&scratch.join("io"))
        .and_then(crate::provider_pipes::ProviderPipeListeners::with_relay)
        .map_err(sandbox)?;
    let program = program(
        probe,
        executable,
        scratch,
        store,
        listeners.directory(),
        digest,
        &image,
    )
    .map_err(sandbox)?;
    let fresh = || {
        kinewright_project::bundle_records::SessionId::fresh()
            .map(|id| id.as_str().to_owned())
            .map_err(|_| CertificationRefusal::SandboxUnavailable)
    };
    let attempt = AttemptIdentity {
        project: fresh()?,
        run: fresh()?,
        task: fresh()?,
        attempt: fresh()?,
        nonce: fresh()?,
        owner_instance: fresh()?,
    };
    // A private namespace: the probe never enters a run's claim or recovery.
    let mut claim = crate::execution_claim::MachineClaim::acquire_private(
        &root.join("m"),
        &attempt.project,
        &attempt.run,
    )
    .map_err(sandbox)?;
    let mut owner = ProbeOwner;
    let mut session = crate::guardian::ContainedSession::prepare(
        &mut claim,
        &mut owner,
        attempt,
        program,
        probe.guardian,
        &crate::guardian::CertifiedProfile::linux_allowlist(),
    )
    .map_err(sandbox)?;
    let run = (|| {
        let mut pipes = listeners.accept().map_err(sandbox)?;
        pipes.credentials(b"").map_err(sandbox)?;
        // The relay channel is dropped here: a loopback connection inside the
        // probe's namespace reaches nothing.
        let reader = pipes.into_reader();
        session.commit(&mut owner).map_err(sandbox)?;
        bounded(reader)
    })();
    // Whatever happened, the whole unit is stopped and reaped first.
    session.close_lifetime();
    let cleaned = session
        .cleanup()
        .and_then(|proof| claim.retire(&proof))
        .and_then(|()| claim.finish());
    drop(image);
    cleaned.map_err(sandbox)?;
    run
}

/// The launch's containment for one `--version` run: the sealed image, no
/// credential mount, an empty private store, the launch's own runtime grants,
/// and the unpaid policy's loopback-only network namespace.
fn program(
    probe: &VersionProbe<'_>,
    executable: &ExecutableIdentity,
    scratch: PathBuf,
    store: PathBuf,
    pipes: &Path,
    digest: String,
    image: &std::fs::File,
) -> io::Result<crate::job_guardian::protocol::Program> {
    use crate::job_guardian::protocol::{ExecutableImage, Program, ProviderSandbox, UnpaidPolicy};
    let scratch_text = scratch.to_string_lossy().into_owned();
    let mut environment = std::collections::BTreeMap::from([
        ("PATH".to_owned(), "/usr/bin:/bin".to_owned()),
        ("LANG".to_owned(), "C.UTF-8".to_owned()),
        ("HOME".to_owned(), scratch_text.clone()),
        ("TMPDIR".to_owned(), scratch_text),
    ]);
    // One fixed environment for every provider: private homes for both CLIs,
    // and no autoupdate, telemetry, error reporting or nonessential traffic.
    let store_text = store.to_string_lossy().into_owned();
    environment.extend(
        [
            ("CODEX_HOME", store_text.as_str()),
            ("CLAUDE_CONFIG_DIR", store_text.as_str()),
            ("DISABLE_AUTOUPDATER", "1"),
            ("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1"),
            ("DISABLE_TELEMETRY", "1"),
            ("DISABLE_ERROR_REPORTING", "1"),
        ]
        .map(|(key, value)| (key.to_owned(), value.to_owned())),
    );
    let program = Program {
        image: Some(ExecutableImage {
            digest,
            source: Some(crate::executable_images::source(image)),
        }),
        credential_root: probe.credential_root.clone(),
        executable: executable.path.clone(),
        arguments: vec!["--version".into()],
        environment,
        working_directory: scratch.clone(),
        pipe_directory: Some(pipes.into()),
        sandbox: ProviderSandbox::LinuxAllowlist {
            credential_mount: None,
            connect_tcp: vec![PROBE_PORT],
            runtime: crate::native_launcher::runtime_grants(executable, probe.user_local_runtime)?,
            scratch,
            credential_store: store,
            executable_identity: executable.file_identity,
            unpaid: Some(UnpaidPolicy {
                mcp_port: PROBE_PORT,
            }),
        },
    };
    program.validate()?;
    Ok(program)
}

/// Read at most [`OUTPUT_CAP`] bytes for at most [`DEADLINE`]; stderr is
/// drained and discarded. A run that does not finish in time, writes too
/// much, or whose exit the guardian does not report is refused.
fn bounded(reader: crate::provider_pipes::ProviderReader) -> Result<Run, CertificationRefusal> {
    let refused = |_| CertificationRefusal::UnqualifiedVersion;
    let crate::provider_pipes::ProviderReader {
        mut output,
        mut diagnostics,
        guardian,
        relay,
    } = reader;
    drop(relay);
    output.set_nonblocking(true).map_err(refused)?;
    diagnostics.set_nonblocking(true).map_err(refused)?;
    let deadline = Instant::now() + DEADLINE;
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 4096];
    let mut stderr_open = true;
    loop {
        let mut progressed = false;
        match output.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => {
                bytes.extend_from_slice(&buffer[..read]);
                if bytes.len() > OUTPUT_CAP {
                    return Err(CertificationRefusal::UnqualifiedVersion);
                }
                progressed = true;
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
            Err(error) if error.kind() == io::ErrorKind::Interrupted => progressed = true,
            Err(error) => return Err(refused(error)),
        }
        if stderr_open {
            match diagnostics.read(&mut buffer) {
                Ok(0) => stderr_open = false,
                Ok(_) => progressed = true,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                Err(_) => stderr_open = false,
            }
        }
        if Instant::now() >= deadline {
            return Err(CertificationRefusal::UnqualifiedVersion);
        }
        if !progressed {
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    let exited_zero = guardian.status() == crate::provider_pipes::GuardianStatus::Exited(0);
    Ok(Run {
        output: bytes,
        exited_zero,
    })
}

#[cfg(test)]
mod tests {
    use super::{VersionForm, VersionProbe, installed_version};
    use crate::orchestration::adapters::{CertificationRefusal, CliVersion, ExecutableIdentity};
    use std::{
        os::unix::fs::MetadataExt,
        path::{Path, PathBuf},
        time::{Duration, Instant},
    };

    /// Each provider's exact form is the only one accepted.
    #[test]
    fn s4b_c6_version_forms_are_exact() {
        let refused = Err(CertificationRefusal::UnqualifiedVersion);
        for (form, output, expected) in [
            (
                VersionForm::Codex,
                &b"codex-cli 0.166.0\n"[..],
                Ok(CliVersion(0, 166, 0)),
            ),
            (
                VersionForm::Codex,
                b"codex-cli 0.200.1",
                Ok(CliVersion(0, 200, 1)),
            ),
            (
                VersionForm::Claude,
                b"2.1.317 (Claude Code)\n",
                Ok(CliVersion(2, 1, 317)),
            ),
            (
                VersionForm::Codex,
                b"codex-cli 0.165.9-beta (upgrade to 0.200.0 recommended)\n",
                refused,
            ),
            (VersionForm::Codex, b"codex-cli 0.166.0-beta.1\n", refused),
            (VersionForm::Codex, b"codex-cli 0.166.0\n0.200.0\n", refused),
            (VersionForm::Codex, b"codex-cli 0.166.0\r\n", refused),
            (VersionForm::Codex, b"codex-cli  0.166.0\n", refused),
            (VersionForm::Codex, b"codex 0.166.0\n", refused),
            (VersionForm::Codex, b"0.166.0\n", refused),
            (VersionForm::Codex, b"codex-cli \xff0.166.0\n", refused),
            (VersionForm::Codex, b"", refused),
            (VersionForm::Claude, b"2.1.317\n", refused),
            (VersionForm::Claude, b"2.1.317 (Claude Code)\n\n", refused),
            (VersionForm::Claude, b"codex-cli 0.166.0\n", refused),
        ] {
            assert_eq!(
                form.parse(output),
                expected,
                "C6_VERSION_FORM {form:?} {output:?}"
            );
        }
    }

    /// A fixture copy named for its behaviour, so `--version` alone selects it.
    fn fixture(directory: &Path, name: &str) -> (PathBuf, ExecutableIdentity) {
        let built = directory.join("version-probe");
        if !built.exists() {
            let status = std::process::Command::new("rustc")
                .args(["--edition", "2024", "-O"])
                .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/version_probe.rs"))
                .arg("-o")
                .arg(&built)
                .status()
                .expect("C6 version fixture compiler");
            assert!(status.success(), "C6 version fixture compiled");
        }
        let path = directory.join(name);
        std::fs::copy(&built, &path).unwrap();
        let metadata = std::fs::metadata(&path).unwrap();
        let identity = ExecutableIdentity {
            path: path.clone(),
            file_identity: [metadata.dev(), metadata.ino()],
        };
        (path, identity)
    }

    fn guardian() -> PathBuf {
        let executable = std::env::current_exe().unwrap();
        let directory = executable.parent().unwrap();
        let directory = if directory.file_name().is_some_and(|name| name == "deps") {
            directory.parent().unwrap()
        } else {
            directory
        };
        directory.join("kinewright-job-guardian")
    }

    fn folder(name: &str) -> kinewright_media::test_support::TempDirectory {
        kinewright_media::test_support::TempDirectory::new_in(
            &kinewright_media::test_support::workspace_target_dir(),
            name,
        )
    }

    fn probe(guardian: &Path, form: VersionForm) -> VersionProbe<'_> {
        VersionProbe {
            guardian,
            form,
            user_local_runtime: false,
            credential_root: None,
        }
    }

    /// The exact-form parse holds through `installed_version` itself.
    #[test]
    #[ignore = "on-demand: equipped Linux guardian test"]
    fn s4b_c6_version_probe_reads_only_the_exact_form() {
        let temp = folder("c6-vp-forms");
        let guardian = guardian();
        let refused = Err(CertificationRefusal::UnqualifiedVersion);
        for (name, form, expected) in [
            ("codex-ok", VersionForm::Codex, Ok(CliVersion(0, 166, 0))),
            ("claude-ok", VersionForm::Claude, Ok(CliVersion(2, 1, 317))),
            ("codex-banner", VersionForm::Codex, refused),
            ("codex-beta", VersionForm::Codex, refused),
            ("codex-lines", VersionForm::Codex, refused),
            ("codex-bytes", VersionForm::Codex, refused),
            ("codex-malformed", VersionForm::Codex, refused),
            ("codex-fails", VersionForm::Codex, refused),
            ("codex-flood", VersionForm::Codex, refused),
            ("claude-bare", VersionForm::Claude, refused),
        ] {
            let (_, executable) = fixture(temp.root(), name);
            assert_eq!(
                installed_version(&probe(&guardian, form), &executable),
                expected,
                "C6_VERSION_PROBE_FORM {name}"
            );
        }
    }

    /// The provider runs inside the guardian's containment: it cannot write
    /// outside its private home or open a network connection, and nothing it
    /// leaves behind outlives the probe.
    #[test]
    #[ignore = "on-demand: equipped Linux guardian test"]
    fn s4b_c6_version_probe_is_contained() {
        let temp = folder("c6-vp-contained");
        let guardian = guardian();
        let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let (path, executable) = fixture(temp.root(), &format!("codex-escape-{port}"));
        let version = installed_version(&probe(&guardian, VersionForm::Codex), &executable);
        let escaped = path.with_file_name("escaped");
        assert!(!escaped.exists(), "C6_VERSION_PROBE_WROTE_OUTSIDE_HOME");
        assert!(
            listener.accept().is_err(),
            "C6_VERSION_PROBE_REACHED_THE_NETWORK"
        );
        assert_eq!(version, Ok(CliVersion(0, 166, 0)), "C6_VERSION_PROBE_READ");
    }

    /// A descendant that keeps stdout open is killed with the unit, so the
    /// read ends when the provider exits, well inside the bound.
    #[test]
    #[ignore = "on-demand: equipped Linux guardian test"]
    fn s4b_c6_version_probe_kills_a_descendant_holding_stdout() {
        let temp = folder("c6-vp-descendant");
        let guardian = guardian();
        let (_, executable) = fixture(temp.root(), "codex-descendant");
        let started = Instant::now();
        let version = installed_version(&probe(&guardian, VersionForm::Codex), &executable);
        let elapsed = started.elapsed();
        assert!(
            elapsed < Duration::from_secs(5),
            "C6_VERSION_PROBE_DESCENDANT_OUTLIVED {elapsed:?}"
        );
        assert_eq!(version, Ok(CliVersion(0, 166, 0)), "C6_VERSION_PROBE_READ");
    }

    /// A provider that never finishes is stopped at the bound and refused.
    #[test]
    #[ignore = "on-demand: equipped Linux guardian test"]
    fn s4b_c6_version_probe_hang_is_bounded() {
        let temp = folder("c6-vp-hang");
        let guardian = guardian();
        let (_, executable) = fixture(temp.root(), "codex-hang");
        let (sender, receiver) = std::sync::mpsc::channel();
        let started = Instant::now();
        std::thread::spawn(move || {
            let _ = sender.send(installed_version(
                &probe(&guardian, VersionForm::Codex),
                &executable,
            ));
        });
        let version = receiver
            .recv_timeout(Duration::from_secs(20))
            .expect("C6_VERSION_PROBE_UNBOUNDED");
        assert_eq!(
            version,
            Err(CertificationRefusal::UnqualifiedVersion),
            "C6_VERSION_PROBE_HANG_REFUSED"
        );
        assert!(
            started.elapsed() < Duration::from_secs(15),
            "C6_VERSION_PROBE_UNBOUNDED {:?}",
            started.elapsed()
        );
    }
}
