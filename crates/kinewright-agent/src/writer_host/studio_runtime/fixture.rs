//! Protocol-child test ports. These never register or qualify an installed provider.
#![allow(clippy::missing_errors_doc)]
use super::{ProfileFactory, RuntimePorts};
use crate::orchestration::{
    AdmittedAttempt,
    adapters::{
        AttemptBinding, CertificationRefusal, CertifiedConnectorProfile, ConnectorRuntimeChecks,
        ConnectorSpecification, ExecutableIdentity, RuntimeQualification, certify_connector,
    },
};
use kinewright_core::AgentEvent;
use std::{
    collections::BTreeMap,
    io,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ProtocolFixtureMode {
    Normal,
    Startup,
    FullQueue,
}
#[derive(Clone)]
pub struct ProtocolObservation {
    pub scratch: PathBuf,
    pub endpoint: String,
    pub identity: crate::orchestration::ExecutionIdentity,
}
struct State {
    root: PathBuf,
    mode: ProtocolFixtureMode,
    released: AtomicBool,
    startup: AtomicBool,
    event: AtomicBool,
    observations: Mutex<Vec<ProtocolObservation>>,
    queues: Mutex<Vec<crossbeam_channel::Receiver<AgentEvent>>>,
    checks: Mutex<BTreeMap<String, RuntimeQualification>>,
}
pub struct ProtocolFixture(Arc<State>);
impl ProtocolFixture {
    #[must_use]
    pub fn new(root: &Path, mode: ProtocolFixtureMode) -> Self {
        Self(Arc::new(State {
            root: root.into(),
            mode,
            released: AtomicBool::new(mode == ProtocolFixtureMode::Normal),
            startup: AtomicBool::new(false),
            event: AtomicBool::new(false),
            observations: Mutex::default(),
            queues: Mutex::default(),
            checks: Mutex::default(),
        }))
    }
    pub fn release(&self) {
        self.0.released.store(true, Ordering::Release);
    }
    #[must_use]
    pub fn startup_entered(&self) -> bool {
        self.0.startup.load(Ordering::Acquire)
    }
    #[must_use]
    pub fn event_entered(&self) -> bool {
        self.0.event.load(Ordering::Acquire)
    }
    #[must_use]
    pub fn native_queue_full(&self) -> bool {
        self.0
            .queues
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            // EventSender reserves two slots for terminal control events.
            .any(|queue| queue.len() == crate::protocol::limits::EVENTS - 2)
    }
    #[must_use]
    pub fn observations(&self) -> Vec<ProtocolObservation> {
        self.0
            .observations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
    #[must_use]
    pub fn claim_root(&self) -> PathBuf {
        self.0.root.join("native-claim")
    }
    pub fn cleanup(&self) -> io::Result<()> {
        if self.claim_root().join("run.json").exists() {
            return Err(io::Error::other(
                "fixture cleanup proof/claim retirement pending",
            ));
        }
        for observation in self.observations() {
            std::fs::remove_dir_all(observation.scratch)?;
        }
        Ok(())
    }
    pub(in crate::writer_host) fn ports(&self) -> RuntimePorts {
        let events = self.0.clone();
        let queues = self.0.clone();
        RuntimePorts {
            profiles: Some(Arc::new(Factory(self.0.clone()))),
            checks: Arc::new(Factory(self.0.clone())),
            guardian: target_dir().join("kinewright-job-guardian"),
            ceiling: None,
            fixture_claim: Some(self.claim_root()),
            fixture_autostart: true,
            witness: Some(Arc::new(super::RuntimeWitness {
                on_event: Arc::new(move |_| {
                    if events.mode == ProtocolFixtureMode::FullQueue {
                        events.event.store(true, Ordering::Release);
                        for observation in events
                            .observations
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .iter()
                        {
                            let _ = std::fs::write(observation.scratch.join("event-release"), b"");
                        }
                        let _ = wait_release(&events);
                    }
                }),
                on_native_events: Arc::new(move |receiver| {
                    queues
                        .queues
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .push(receiver);
                }),
            })),
        }
    }
}
impl Drop for ProtocolFixture {
    fn drop(&mut self) {
        self.release();
    }
}
fn wait_release(state: &State) -> io::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !state.released.load(Ordering::Acquire) {
        if Instant::now() >= deadline {
            return Err(io::Error::other("fixture barrier timeout"));
        }
        std::thread::park_timeout(Duration::from_millis(1));
    }
    Ok(())
}
fn target_dir() -> PathBuf {
    let executable = std::env::current_exe().unwrap();
    let parent = executable.parent().unwrap();
    if parent.file_name().is_some_and(|name| name == "deps") {
        parent.parent().unwrap().into()
    } else {
        parent.into()
    }
}
fn child() -> io::Result<PathBuf> {
    static CHILD: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    static BUILD: Mutex<()> = Mutex::new(());
    let _build = BUILD
        .lock()
        .map_err(|_| io::Error::other("fixture build lock"))?;
    if let Some(child) = CHILD.get() {
        return Ok(child.clone());
    }
    let path = target_dir().join(format!("s4b-c4-protocol-{}", std::process::id()));
    let status = std::process::Command::new("rustc")
        .args(["--edition", "2024"])
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ja_s4_native.rs"))
        .arg("-o")
        .arg(&path)
        .status()?;
    if !status.success() {
        return Err(io::Error::other("protocol fixture compilation"));
    }
    let _ = CHILD.set(path.clone());
    Ok(path)
}
fn digest(value: &impl serde::Serialize) -> [u8; 32] {
    let hash = kinewright_media::sha256_bytes(&serde_json::to_vec(value).unwrap());
    std::array::from_fn(|index| u8::from_str_radix(&hash[index * 2..index * 2 + 2], 16).unwrap())
}
struct Factory(Arc<State>);
impl ProfileFactory for Factory {
    fn bind(&self, _: &AdmittedAttempt) -> io::Result<CertifiedConnectorProfile> {
        Err(io::Error::other("fixture needs real task endpoint"))
    }
    fn bind_for_task(
        &self,
        admitted: &AdmittedAttempt,
        endpoint: &str,
    ) -> io::Result<CertifiedConnectorProfile> {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        self.0.startup.store(true, Ordering::Release);
        if self.0.mode == ProtocolFixtureMode::Startup {
            wait_release(&self.0)?;
        }
        let identity = admitted.identity();
        let scratch = std::env::temp_dir().join(format!("n4-{}", &identity.nonce.as_str()[..8]));
        std::fs::create_dir(&scratch)?;
        std::fs::set_permissions(&scratch, std::fs::Permissions::from_mode(0o700))?;
        if self.0.mode == ProtocolFixtureMode::FullQueue {
            std::fs::write(scratch.join("event-mode"), b"full")?;
        }
        let credentials = scratch.join("credentials");
        std::fs::create_dir(&credentials)?;
        std::fs::set_permissions(&credentials, std::fs::Permissions::from_mode(0o700))?;
        let binding = AttemptBinding::issue(
            admitted,
            endpoint.into(),
            vec![self.0.root.clone()],
            scratch.clone(),
        )
        .map_err(io::Error::other)?;
        let mut runtime = CertifiedConnectorProfile::fixture(admitted)
            .runtime()
            .clone();
        let executable = child()?;
        let metadata = std::fs::metadata(&executable)?;
        runtime.executable = ExecutableIdentity {
            path: executable,
            file_identity: [metadata.dev(), metadata.ino()],
        };
        runtime.launch_context.executable = runtime.executable.clone();
        runtime.launch_context.credential_store.root = credentials;
        runtime.authentication.context = runtime.launch_context.clone();
        runtime.authentication.refreshed_store = runtime.launch_context.credential_store.clone();
        self.0
            .checks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(identity.nonce.as_str().into(), runtime);
        let profile =
            certify_connector("scripted", binding.clone(), self).map_err(io::Error::other)?;
        let program = crate::native_launcher::program(&profile, "placeholder")?;
        let mut options = program.arguments;
        options.pop();
        let mut checks = self
            .0
            .checks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let runtime = checks.get_mut(identity.nonce.as_str()).unwrap();
        runtime.launch_context.environment_digest = digest(&program.environment);
        runtime.launch_context.settings_digest = digest(&options);
        runtime.authentication.context = runtime.launch_context.clone();
        drop(checks);
        self.0
            .observations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(ProtocolObservation {
                scratch,
                endpoint: endpoint.into(),
                identity: identity.clone(),
            });
        certify_connector("scripted", binding, self).map_err(io::Error::other)
    }
}
impl ConnectorRuntimeChecks for Factory {
    fn probe(
        &self,
        _: &ConnectorSpecification,
        binding: &AttemptBinding,
    ) -> Result<RuntimeQualification, CertificationRefusal> {
        self.0
            .checks
            .lock()
            .map_err(|_| CertificationRefusal::ExecutableIdentity)?
            .get(binding.identity().nonce.as_str())
            .cloned()
            .ok_or(CertificationRefusal::ExecutableIdentity)
    }
}

/// A bounded proof-observation barrier for the real child-launch witness. No
/// provider profile, authority, or evidence can be supplied through this port.
pub(crate) fn recovery_barrier() -> io::Result<()> {
    let Some(path) = std::env::var_os("KINEWRIGHT_C4_RECOVERY_FIXTURE_BARRIER") else {
        return Ok(());
    };
    let path = PathBuf::from(path);
    std::fs::write(path.join("entered"), b"")?;
    let deadline = Instant::now() + Duration::from_secs(15);
    while !path.join("release").exists() {
        if Instant::now() >= deadline {
            return Err(io::Error::other("fixture recovery proof barrier expired"));
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    Ok(())
}
