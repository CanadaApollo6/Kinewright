#![cfg(feature = "eval-harness")]
// Compile the native modules as tests so C2 can supply no production profile token.
#[allow(
    dead_code,
    unused_imports,
    reason = "the native test crate includes the shared production module surface"
)]
#[path = "../src/orchestration/execution_claim.rs"]
mod execution_claim;
#[allow(
    dead_code,
    unused_imports,
    reason = "the native test crate includes the shared production module surface"
)]
#[path = "../src/orchestration/guardian.rs"]
mod guardian;
#[allow(
    dead_code,
    unused_imports,
    reason = "the native test crate includes the shared production module surface"
)]
#[path = "../src/job_guardian/mod.rs"]
mod job_guardian;

#[cfg(target_os = "linux")]
use crate::guardian::RecoveryObserver;
use crate::{
    execution_claim::MachineClaim,
    guardian::{CertifiedProfile, ContainedSession, DurableAttemptOwner},
    job_guardian::protocol::{self, AttemptIdentity, Program, RecoveryIdentity},
};
#[cfg(target_os = "linux")]
use crate::{
    guardian::spawn_transport,
    job_guardian::protocol::{GuardianMessage, HostMessage},
};
use kinewright_job_sys::ProcessHandle;
#[cfg(target_os = "linux")]
use std::os::unix::fs::FileTypeExt;
use std::{
    fs::{self, File},
    io,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

static SEQUENCE: AtomicU64 = AtomicU64::new(0);
static FIXTURE: OnceLock<PathBuf> = OnceLock::new();
fn fixture() -> &'static Path {
    FIXTURE.get_or_init(|| {
        if let Some(binary) = std::env::var_os("KINEWRIGHT_S4_PREBUILT_NATIVE") {
            let binary = PathBuf::from(binary);
            assert!(
                binary.is_absolute() && binary.is_file(),
                "prebuilt native fixture prerequisite"
            );
            return binary;
        }
        let target = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
        fs::create_dir_all(&target).unwrap();
        let binary = target.join(format!(
            "c2-native-{}{}",
            std::process::id(),
            std::env::consts::EXE_SUFFIX
        ));
        let status = Command::new("rustc")
            .args(["--edition", "2024"])
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ja_s4_native.rs"))
            .arg("-o")
            .arg(&binary)
            .status()
            .unwrap();
        assert!(status.success(), "native fixture compiler prerequisite");
        binary
    })
}
fn guardian() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_kinewright-job-guardian"))
}
fn root(label: &str) -> PathBuf {
    let path = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "kinewright-c2-{label}-{}-{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::SeqCst)
    ));
    fs::create_dir_all(&path).unwrap();
    path
}
fn identity() -> AttemptIdentity {
    use std::fmt::Write as _;
    let mut nonce = [0; 16];
    getrandom::fill(&mut nonce).unwrap();
    let mut rendered = String::with_capacity(32);
    for byte in nonce {
        write!(&mut rendered, "{byte:02x}").unwrap();
    }
    let nonce = rendered;
    AttemptIdentity {
        project: "a1".into(),
        run: "b2".into(),
        task: "c3".into(),
        attempt: nonce.clone(),
        nonce,
        owner_instance: "1234567890abcdef1234567890abcdef".into(),
    }
}
fn program(root: &Path) -> Program {
    Program {
        executable: fixture().into(),
        arguments: vec![root.to_string_lossy().into_owned(), "payload".into()],
    }
}
struct Owner {
    root: PathBuf,
    refuse: bool,
    reservations: usize,
    commits: usize,
}
impl Owner {
    fn new(root: &Path) -> Self {
        Self {
            root: root.into(),
            refuse: false,
            reservations: 0,
            commits: 0,
        }
    }
}
impl DurableAttemptOwner for Owner {
    fn reserve(&mut self, id: &AttemptIdentity) -> io::Result<()> {
        fs::write(
            self.root.join("fixture-reservation.json"),
            serde_json::to_vec(id)?,
        )?;
        #[cfg(windows)]
        File::options()
            .write(true)
            .open(self.root.join("fixture-reservation.json"))?
            .sync_all()?;
        #[cfg(not(windows))]
        File::open(self.root.join("fixture-reservation.json"))?.sync_all()?;
        self.reservations += 1;
        Ok(())
    }
    fn authorize_commit(&mut self, recovery: &RecoveryIdentity) -> io::Result<()> {
        if self.refuse {
            return Err(io::Error::other("durable owner refused COMMIT"));
        }
        fs::write(
            self.root.join("fixture-committed.json"),
            serde_json::to_vec(recovery)?,
        )?;
        #[cfg(windows)]
        File::options()
            .write(true)
            .open(self.root.join("fixture-committed.json"))?
            .sync_all()?;
        #[cfg(not(windows))]
        File::open(self.root.join("fixture-committed.json"))?.sync_all()?;
        self.commits += 1;
        Ok(())
    }
}
fn wait_file(path: &Path) {
    let deadline = Instant::now() + protocol::STARTUP;
    while !path.exists() {
        assert!(
            Instant::now() < deadline,
            "native fixture signal {}",
            path.display()
        );
        thread::sleep(Duration::from_millis(1));
    }
}
fn fixture_pid(root: &Path, role: &str) -> u32 {
    let path = root.join(format!("{role}.json"));
    wait_file(&path);
    let value: serde_json::Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    assert_eq!(value["version"], 1);
    u32::try_from(value["pid"].as_u64().unwrap()).unwrap()
}
fn pin_descendants(session: &mut ContainedSession, root: &Path) -> Vec<ProcessHandle> {
    ["payload", "grandchild"]
        .iter()
        .map(|role| {
            let process = ProcessHandle::open(fixture_pid(root, role)).unwrap();
            assert!(
                !process.reaped().unwrap(),
                "fixture must be independently live before fault"
            );
            session
                .observe_descendant(process.pid(), process.start())
                .unwrap();
            process
        })
        .collect()
}
fn assert_retired(claim: &mut MachineClaim, proof: &crate::guardian::CleanupProof) {
    assert_eq!(claim.retained_attempts(), 1);
    claim.retire(proof).unwrap();
    assert_eq!(claim.retained_attempts(), 0);
    assert!(
        claim.retire(proof).is_err(),
        "retirement must happen exactly once"
    );
}

#[test]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn s4_precommit_cannot_execute() {
    #[cfg(target_os = "linux")]
    if !equipped_session("s4_precommit_cannot_execute") {
        return;
    }
    let root = root("precommit");
    #[cfg(target_os = "linux")]
    let _cleanup = FixtureCleanup(root.clone());
    let id = identity();
    let mut claim =
        MachineClaim::acquire_fixture(&root.join("machine"), &id.project, &id.run).unwrap();
    let mut owner = Owner::new(&root);
    let mut session = ContainedSession::prepare(
        &mut claim,
        &mut owner,
        id,
        program(&root),
        guardian(),
        &CertifiedProfile::fixture(),
    )
    .unwrap();
    let payload = ProcessHandle::open(session.payload_identity().0).unwrap();
    assert!(
        !payload.reaped().unwrap(),
        "a contained native process must exist before COMMIT"
    );
    thread::sleep(Duration::from_millis(100));
    assert!(
        !root.join("payload.json").exists(),
        "zero provider execution before durable COMMIT"
    );
    owner.refuse = true;
    assert!(session.commit(&mut owner).is_err());
    assert!(!root.join("payload.json").exists());
    assert_eq!(owner.reservations, 1);
    assert_eq!(owner.commits, 0);
    assert!(MachineClaim::acquire_fixture(&root.join("machine"), "d4", "e5").is_err());
    let proof = session.cleanup().unwrap();
    assert_retired(&mut claim, &proof);
    claim.finish().unwrap();
    remove_root(root);
}

#[test]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn s4_cancel_tree_and_retire_once() {
    #[cfg(target_os = "linux")]
    if !equipped_session("s4_cancel_tree_and_retire_once") {
        return;
    }
    let root = root("cancel");
    #[cfg(target_os = "linux")]
    let _cleanup = FixtureCleanup(root.clone());
    let id = identity();
    let mut claim =
        MachineClaim::acquire_fixture(&root.join("machine"), &id.project, &id.run).unwrap();
    let mut owner = Owner::new(&root);
    let mut session = ContainedSession::prepare(
        &mut claim,
        &mut owner,
        id,
        program(&root),
        guardian(),
        &CertifiedProfile::fixture(),
    )
    .unwrap();
    session.commit(&mut owner).unwrap();
    let descendants = pin_descendants(&mut session, &root);
    #[cfg(target_os = "linux")]
    {
        let stat = fs::read_to_string(format!("/proc/{}/stat", descendants[1].pid())).unwrap();
        let fields: Vec<_> = stat
            .rsplit_once(')')
            .unwrap()
            .1
            .split_whitespace()
            .collect();
        assert_eq!(
            fields[3].parse::<u32>().unwrap(),
            descendants[1].pid(),
            "grandchild must call setsid"
        );
    }
    assert!(MachineClaim::acquire_fixture(&root.join("machine"), "d4", "e5").is_err());
    let started = Instant::now();
    session.request_cancel().unwrap();
    let proof = session.cleanup().unwrap();
    assert!(!proof.guardian_lost());
    println!(
        "C2 phase_evidence={}",
        serde_json::json!({"recovery":session.recovery(), "startup_us":session.phase_timestamps_us(), "cleanup_us":session.cleanup_timestamps_us()})
    );
    assert!(
        started.elapsed() >= Duration::from_millis(490),
        "cancel must retain the 500 ms grace"
    );
    assert!(started.elapsed() <= protocol::GRACE + protocol::CLEANUP);
    for process in descendants {
        assert!(
            process.reaped().unwrap(),
            "no live or zombie fixture identity"
        );
    }
    assert_retired(&mut claim, &proof);
    println!(
        "C2 cancel kill-to-empty/reap bounded; elapsed_us={}",
        started.elapsed().as_micros()
    );
    claim.finish().unwrap();
    remove_root(root);
}

#[cfg(target_os = "linux")]
#[test]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn s4_guardian_death_and_observation_loss() {
    #[cfg(target_os = "linux")]
    if !equipped_session("s4_guardian_death_and_observation_loss") {
        return;
    }
    let root = root("guardian-death");
    #[cfg(target_os = "linux")]
    let _cleanup = FixtureCleanup(root.clone());
    let id = identity();
    let machine = root.join("machine");
    let mut claim = MachineClaim::acquire_fixture(&machine, &id.project, &id.run).unwrap();
    let mut owner = Owner::new(&root);
    let mut session = ContainedSession::prepare(
        &mut claim,
        &mut owner,
        id,
        program(&root),
        guardian(),
        &CertifiedProfile::fixture(),
    )
    .unwrap();
    session.commit(&mut owner).unwrap();
    let descendants = pin_descendants(&mut session, &root);
    let mut observer = RecoveryObserver::open(session.recovery()).unwrap();
    let guardian = ProcessHandle::open(session.recovery().guardian_pid).unwrap();
    guardian.kill().unwrap();
    // Deliberately lose the host's ordinary pipe/cleanup observer and OS lock.
    drop(session);
    drop(claim);
    assert!(
        MachineClaim::acquire_fixture(&machine, "d4", "e5").is_err(),
        "free lock/EOF is not emptiness"
    );
    let mut recovered = MachineClaim::recover_fixture(&machine).unwrap();
    assert_eq!(
        recovered.retained_attempts(),
        1,
        "missing observation retains the actual reservation"
    );
    assert!(
        recovered.reserve(&identity()).is_err(),
        "recovery cannot overlap a replacement"
    );
    let outcome = observer.wait_for_cleanup();
    if outcome.is_err() {
        let _ = observer.stop_tree();
    }
    let proof = outcome.unwrap();
    assert!(
        proof.guardian_lost(),
        "guardian loss permanently disqualifies unsaved output"
    );
    for process in descendants {
        assert!(process.reaped().unwrap());
    }
    assert_retired(&mut recovered, &proof);
    recovered.finish().unwrap();
    remove_root(root);
}

#[cfg(target_os = "linux")]
#[test]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn s4_real_host_death_skips_grace() {
    #[cfg(target_os = "linux")]
    if !equipped_session("s4_real_host_death_skips_grace") {
        return;
    }
    let root = root("host-death");
    #[cfg(target_os = "linux")]
    let _cleanup = FixtureCleanup(root.clone());
    let host = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "s4_native_host_fixture", "--nocapture"])
        .env("KINEWRIGHT_S4_HOST_FIXTURE", &root)
        .env("KINEWRIGHT_S4_NATIVE", fixture())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut host = FixtureChild(host);
    wait_file(&root.join("host-ready.json"));
    let recovery: RecoveryIdentity =
        serde_json::from_slice(&fs::read(root.join("host-ready.json")).unwrap()).unwrap();
    let mut observer = RecoveryObserver::open(&recovery).unwrap();
    let child = ProcessHandle::open(fixture_pid(&root, "payload")).unwrap();
    let grandchild = ProcessHandle::open(fixture_pid(&root, "grandchild")).unwrap();
    assert!(!child.reaped().unwrap() && !grandchild.reaped().unwrap());
    let started = Instant::now();
    let _holder = FixtureProcess(ProcessHandle::open(fixture_pid(&root, "writer-holder")).unwrap());
    host.kill().unwrap();
    host.wait().unwrap();
    let outcome = observer.wait_for_cleanup();
    if outcome.is_err() {
        let _ = observer.stop_tree();
    }
    let proof = outcome.unwrap();
    assert!(child.reaped().unwrap() && grandchild.reaped().unwrap());
    assert!(started.elapsed() < protocol::CLEANUP);
    let mut recovered = MachineClaim::recover_fixture(&root.join("machine")).unwrap();
    assert_retired(&mut recovered, &proof);
    recovered.finish().unwrap();
    println!(
        "C2 real host death empty/reaped_us={}",
        started.elapsed().as_micros()
    );
    remove_root(root);
}

#[cfg(any(target_os = "linux", windows))]
#[test]
fn s4_native_host_fixture() {
    let Some(root) = std::env::var_os("KINEWRIGHT_S4_HOST_FIXTURE") else {
        return;
    };
    let root = PathBuf::from(root);
    let id = identity();
    let mut claim =
        MachineClaim::acquire_fixture(&root.join("machine"), &id.project, &id.run).unwrap();
    let mut owner = Owner::new(&root);
    let program = Program {
        executable: std::env::var_os("KINEWRIGHT_S4_NATIVE").unwrap().into(),
        arguments: vec![
            root.to_string_lossy().into_owned(),
            if root.join("stubborn").exists() {
                "stubborn-payload".into()
            } else {
                "payload".into()
            },
        ],
    };
    let mut session = ContainedSession::prepare(
        &mut claim,
        &mut owner,
        id,
        program,
        guardian(),
        &CertifiedProfile::fixture(),
    )
    .unwrap();
    session.commit(&mut owner).unwrap();
    pin_descendants(&mut session, &root);
    // An unrelated native fixture catches accidental lifetime-writer inheritance.
    let mut writer_holder = Command::new(std::env::var_os("KINEWRIGHT_S4_NATIVE").unwrap())
        .arg(&root)
        .arg("writer-holder")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    fs::write(
        root.join("host-ready.partial"),
        serde_json::to_vec(session.recovery()).unwrap(),
    )
    .unwrap();
    fs::rename(
        root.join("host-ready.partial"),
        root.join("host-ready.json"),
    )
    .unwrap();
    let _ = &mut writer_holder;
    loop {
        if root.join("cancel").exists() {
            let _ = session.request_cancel();
        }
        thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(target_os = "linux")]
#[test]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn s4_lost_startup_acknowledgments() {
    #[cfg(target_os = "linux")]
    if !equipped_session("s4_lost_startup_acknowledgments") {
        return;
    }
    for stage in ["owned", "contained", "executing"] {
        let root = root(stage);
        #[cfg(target_os = "linux")]
        let _cleanup = FixtureCleanup(root.clone());
        let id = identity();
        let mut claim =
            MachineClaim::acquire_fixture(&root.join("machine"), &id.project, &id.run).unwrap();
        claim.reserve(&id).unwrap();
        let private = root.join("native");
        fs::create_dir(&private).unwrap();
        let (mut launcher, mut writer, replies) =
            spawn_transport(&CertifiedProfile::fixture(), guardian(), &private, &id).unwrap();
        protocol::write_frame(
            &mut writer,
            &HostMessage::Prepare {
                identity: id.clone(),
                program: program(&root),
            },
        )
        .unwrap();
        let deadline = Instant::now() + protocol::STARTUP;
        let next = |deadline| loop {
            match replies.try_recv() {
                Ok(Ok(message)) => break message,
                Ok(Err(error)) => panic!("native transport failed: {error}"),
                Err(_) => {
                    assert!(Instant::now() < deadline);
                    thread::sleep(Duration::from_millis(1));
                }
            }
        };
        let GuardianMessage::Owned { recovery, .. } = next(deadline) else {
            panic!("OWNED");
        };
        claim.bind(&recovery).unwrap();
        let mut observer = RecoveryObserver::open(&recovery).unwrap();
        if stage != "owned" {
            protocol::write_frame(
                &mut writer,
                &HostMessage::Contain {
                    nonce: id.nonce.clone(),
                },
            )
            .unwrap();
            let GuardianMessage::Contained { pid, start, .. } =
                next(Instant::now() + protocol::STARTUP)
            else {
                panic!("CONTAINED");
            };
            let process = ProcessHandle::open(pid).unwrap();
            assert_eq!(process.start(), start);
            assert!(!process.reaped().unwrap());
            assert!(!root.join("payload.json").exists(), "pre-COMMIT execution");
        }
        if stage == "executing" {
            protocol::write_frame(
                &mut writer,
                &HostMessage::Commit {
                    nonce: id.nonce.clone(),
                },
            )
            .unwrap();
            wait_file(&root.join("grandchild.json"));
        }
        // The host loses this ACK and abandons startup. No alternate launcher runs.
        drop(writer);
        let outcome = observer.wait_for_cleanup();
        if outcome.is_err() {
            let _ = observer.stop_tree();
        }
        let proof = outcome.unwrap();
        if stage != "executing" {
            assert!(!root.join("payload.json").exists());
        }
        assert_retired(&mut claim, &proof);
        claim.finish().unwrap();
        launcher.wait().unwrap();
        remove_root(root);
    }
}

#[test]
fn s4_machine_claim_stays_scoped_and_bounded() {
    let root = root("claim");
    #[cfg(target_os = "linux")]
    let _cleanup = FixtureCleanup(root.clone());
    let id = identity();
    let mut claim = MachineClaim::acquire_fixture(&root, &id.project, &id.run).unwrap();
    for _ in 0..3 {
        claim.reserve(&identity()).unwrap();
    }
    assert!(claim.reserve(&identity()).is_err());
    assert!(
        claim
            .reserve(&AttemptIdentity {
                project: "d4".into(),
                ..identity()
            })
            .is_err()
    );
    assert!(MachineClaim::acquire_fixture(&root, "d4", "e5").is_err());
    drop(claim);
    assert!(MachineClaim::acquire_fixture(&root, "d4", "e5").is_err());
    let recovered = MachineClaim::recover_fixture(&root).unwrap();
    assert_eq!(recovered.retained_attempts(), 3);
    assert!(recovered.finish().is_err());
    remove_root(root);
}

#[cfg(target_os = "linux")]
#[test]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn s4_retained_inode_deletion_signal() {
    #[cfg(target_os = "linux")]
    if !equipped_session("s4_retained_inode_deletion_signal") {
        return;
    }
    let root = root("kernel-signal");
    #[cfg(target_os = "linux")]
    let _cleanup = FixtureCleanup(root.clone());
    let id = identity();
    let mut claim =
        MachineClaim::acquire_fixture(&root.join("machine"), &id.project, &id.run).unwrap();
    let mut owner = Owner::new(&root);
    let mut session = ContainedSession::prepare(
        &mut claim,
        &mut owner,
        id,
        program(&root),
        guardian(),
        &CertifiedProfile::fixture(),
    )
    .unwrap();
    let leaf =
        kinewright_job_sys::Cgroup::open(Path::new(&session.recovery().containment)).unwrap();
    assert!(
        leaf.try_remove().is_err(),
        "kernel must refuse rmdir of a real nonempty leaf"
    );
    let empty = leaf.create_child("native-signal").unwrap();
    let live_links = match empty.link_count() {
        Ok(links) if links > 0 => links,
        observed => {
            skip_kernel_signal(&format!("live fstat signal unavailable: {observed:?}"));
            session.close_lifetime();
            return;
        }
    };
    assert!(
        !match empty.kernel_deleted() {
            Ok(deleted) => deleted,
            Err(error) => {
                skip_kernel_signal(&format!("live inode verification unavailable: {error}"));
                session.close_lifetime();
                return;
            }
        },
        "a live retained inode must not prove deletion"
    );
    empty.try_remove().unwrap();
    let deleted_links = empty.link_count();
    if live_links == 0 || !deleted_links.as_ref().is_ok_and(|links| *links == 0) {
        let reason = format!("live_nlink={live_links}, deleted_nlink={deleted_links:?}");
        skip_kernel_signal(&reason);
        session.close_lifetime();
        return;
    }
    assert!(empty.kernel_deleted().unwrap());
    let kernel = Command::new("uname").arg("-r").output().unwrap();
    println!(
        "C2 kernel={} live_nlink={live_links} removed_nlink={} nonempty_rmdir=refused",
        String::from_utf8_lossy(&kernel.stdout).trim(),
        deleted_links.unwrap()
    );
    session.close_lifetime();
    let proof = session.cleanup().unwrap();
    assert_retired(&mut claim, &proof);
    claim.finish().unwrap();
    remove_root(root);
}

#[cfg(target_os = "linux")]
#[allow(
    clippy::explicit_write,
    reason = "bypass libtest capture so unsupported kernel signals are visible in CI"
)]
fn skip_kernel_signal(reason: &str) {
    use std::io::Write as _;
    let kernel = Command::new("uname").arg("-r").output().unwrap();
    let reason = format!(
        "kernel={} retained-inode-signal {reason}",
        String::from_utf8_lossy(&kernel.stdout).trim()
    );
    writeln!(
        std::io::stderr(),
        "\nC2_NATIVE_NOT_RUN test=s4_retained_inode_deletion_signal reason={reason}"
    )
    .unwrap();
    fs::write(
        Path::new(env!("CARGO_TARGET_TMPDIR")).join("c2-kernel-signal-not-run.txt"),
        reason,
    )
    .unwrap();
}

#[cfg(target_os = "linux")]
#[test]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn s4_reaped_guardian_and_identity_are_required() {
    #[cfg(target_os = "linux")]
    if !equipped_session("s4_reaped_guardian_and_identity_are_required") {
        return;
    }
    let root = root("reap-identity");
    #[cfg(target_os = "linux")]
    let _cleanup = FixtureCleanup(root.clone());
    let id = identity();
    let mut claim =
        MachineClaim::acquire_fixture(&root.join("machine"), &id.project, &id.run).unwrap();
    claim.reserve(&id).unwrap();
    let private = root.join("native");
    fs::create_dir(&private).unwrap();
    let (mut launcher, mut writer, replies) =
        spawn_transport(&CertifiedProfile::fixture(), guardian(), &private, &id).unwrap();
    protocol::write_frame(
        &mut writer,
        &HostMessage::Prepare {
            identity: id.clone(),
            program: program(&root),
        },
    )
    .unwrap();
    let deadline = Instant::now() + protocol::STARTUP;
    let recovery = loop {
        if let Ok(Ok(GuardianMessage::Owned { recovery, .. })) = replies.try_recv() {
            break recovery;
        }
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(1));
    };
    claim.bind(&recovery).unwrap();
    let mut wrong = recovery.clone();
    wrong.inode ^= 1;
    assert!(
        RecoveryObserver::open(&wrong).is_err(),
        "foreign retained inode cannot prove this attempt empty"
    );
    let leaf = Path::new(&recovery.containment);
    let isolated_service = leaf.join(recovery.attempt.unit());
    fs::create_dir(&isolated_service).unwrap();
    fs::create_dir(isolated_service.join("payload")).unwrap();
    let empty_leaf = kinewright_job_sys::Cgroup::open(&isolated_service.join("payload")).unwrap();
    let empty_service = kinewright_job_sys::Cgroup::open(&isolated_service).unwrap();
    let mut isolated = recovery.clone();
    isolated.containment = empty_leaf.path().to_string_lossy().into_owned();
    (isolated.device, isolated.inode) = empty_leaf.identity();
    (isolated.service_device, isolated.service_inode) = empty_service.identity();
    let mut isolated_observer = RecoveryObserver::open(&isolated).unwrap();
    assert!(
        isolated_observer.wait_for_cleanup().is_err(),
        "even verified empty native groups cannot retire a still-live guardian"
    );
    empty_leaf.try_remove().unwrap();
    empty_service.try_remove().unwrap();
    let mut observer = RecoveryObserver::open(&recovery).unwrap();
    assert!(
        observer.wait_for_cleanup().is_err(),
        "an empty leaf cannot retire a still-live guardian"
    );
    drop(writer);
    let proof = observer.wait_for_cleanup().unwrap();
    assert_retired(&mut claim, &proof);
    claim.finish().unwrap();
    launcher.wait().unwrap();
    remove_root(root);
}

#[cfg(target_os = "linux")]
#[test]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn s4_deleted_cgroup_zombies_still_block_retirement() {
    #[cfg(target_os = "linux")]
    if !equipped_session("s4_deleted_cgroup_zombies_still_block_retirement") {
        return;
    }
    let root = root("zombie");
    #[cfg(target_os = "linux")]
    let _cleanup = FixtureCleanup(root.clone());
    let id = identity();
    let mut claim =
        MachineClaim::acquire_fixture(&root.join("machine"), &id.project, &id.run).unwrap();
    let mut owner = Owner::new(&root);
    let mut native = program(&root);
    native.arguments[1] = "zombie-parent".into();
    let mut session = ContainedSession::prepare(
        &mut claim,
        &mut owner,
        id,
        native,
        guardian(),
        &CertifiedProfile::fixture(),
    )
    .unwrap();
    let leaf =
        kinewright_job_sys::Cgroup::open(Path::new(&session.recovery().containment)).unwrap();
    let mut zombie_leaf = leaf.create_child("zombie-witness").unwrap();
    fs::write(
        root.join("zombie-target"),
        zombie_leaf.path().as_os_str().as_encoded_bytes(),
    )
    .unwrap();
    session.commit(&mut owner).unwrap();
    let pid = fixture_pid(&root, "zombie-child");
    let process = ProcessHandle::open(pid).unwrap();
    let deadline = Instant::now() + protocol::STARTUP;
    loop {
        let stat = fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
        if stat.rsplit_once(')').unwrap().1.split_whitespace().next() == Some("Z") {
            break;
        }
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(1));
    }
    assert!(
        zombie_leaf.empty().unwrap(),
        "events deliberately excludes this real zombie"
    );
    zombie_leaf.try_remove().unwrap();
    assert!(zombie_leaf.kernel_deleted().unwrap());
    let scan_deadline = Instant::now() + protocol::CLEANUP;
    loop {
        if let Ok(empty) = zombie_leaf.no_processes() {
            assert!(
                !empty,
                "deleted cgroup's zombie must block independent retirement"
            );
            break;
        }
        assert!(
            Instant::now() < scan_deadline,
            "complete native process scan unavailable"
        );
        thread::sleep(Duration::from_millis(1));
    }
    assert!(!process.reaped().unwrap());
    session.close_lifetime();
    let proof = session.cleanup().unwrap();
    assert!(process.reaped().unwrap());
    assert_retired(&mut claim, &proof);
    claim.finish().unwrap();
    remove_root(root);
}

#[cfg(target_os = "linux")]
#[test]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn s4_guardian_subreaps_real_orphans() {
    #[cfg(target_os = "linux")]
    if !equipped_session("s4_guardian_subreaps_real_orphans") {
        return;
    }
    let root = root("subreaper");
    #[cfg(target_os = "linux")]
    let _cleanup = FixtureCleanup(root.clone());
    let id = identity();
    let mut claim =
        MachineClaim::acquire_fixture(&root.join("machine"), &id.project, &id.run).unwrap();
    let mut owner = Owner::new(&root);
    let mut native = program(&root);
    native.arguments[1] = "orphan-payload".into();
    let mut session = ContainedSession::prepare(
        &mut claim,
        &mut owner,
        id,
        native,
        guardian(),
        &CertifiedProfile::fixture(),
    )
    .unwrap();
    session.commit(&mut owner).unwrap();
    wait_file(&root.join("adopted.json"));
    let adopted: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join("adopted.json")).unwrap()).unwrap();
    assert_eq!(
        adopted["parent"].as_u64(),
        Some(u64::from(session.recovery().guardian_pid)),
        "real orphan must be adopted by the native guardian subreaper"
    );
    let orphan =
        ProcessHandle::open(u32::try_from(adopted["pid"].as_u64().unwrap()).unwrap()).unwrap();
    session
        .observe_descendant(orphan.pid(), orphan.start())
        .unwrap();
    session.close_lifetime();
    let proof = session.cleanup().unwrap();
    assert!(orphan.reaped().unwrap());
    assert_retired(&mut claim, &proof);
    claim.finish().unwrap();
    remove_root(root);
}

#[cfg(target_os = "linux")]
#[test]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn s4_startup_deadline_kills_a_contained_shim() {
    #[cfg(target_os = "linux")]
    if !equipped_session("s4_startup_deadline_kills_a_contained_shim") {
        return;
    }
    let root = root("startup-deadline");
    #[cfg(target_os = "linux")]
    let _cleanup = FixtureCleanup(root.clone());
    let id = identity();
    let mut claim =
        MachineClaim::acquire_fixture(&root.join("machine"), &id.project, &id.run).unwrap();
    claim.reserve(&id).unwrap();
    let private = root.join("native");
    fs::create_dir(&private).unwrap();
    let (mut launcher, mut writer, replies) =
        spawn_transport(&CertifiedProfile::fixture(), guardian(), &private, &id).unwrap();
    protocol::write_frame(
        &mut writer,
        &HostMessage::Prepare {
            identity: id.clone(),
            program: program(&root),
        },
    )
    .unwrap();
    let receive = || {
        let deadline = Instant::now() + protocol::STARTUP;
        loop {
            if let Ok(Ok(message)) = replies.try_recv() {
                break message;
            }
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(1));
        }
    };
    let GuardianMessage::Owned { recovery, .. } = receive() else {
        panic!("OWNED");
    };
    claim.bind(&recovery).unwrap();
    let mut observer = RecoveryObserver::open(&recovery).unwrap();
    protocol::write_frame(
        &mut writer,
        &HostMessage::Contain {
            nonce: id.nonce.clone(),
        },
    )
    .unwrap();
    let GuardianMessage::Contained { pid, .. } = receive() else {
        panic!("CONTAINED");
    };
    let child = ProcessHandle::open(pid).unwrap();
    assert!(!child.reaped().unwrap());
    let started = Instant::now();
    thread::sleep(Duration::from_millis(4800));
    let proof = observer.wait_for_cleanup().unwrap();
    assert!(started.elapsed() <= protocol::STARTUP + protocol::CLEANUP);
    assert!(
        !root.join("payload.json").exists(),
        "expired startup cannot release provider code"
    );
    assert!(child.reaped().unwrap());
    println!(
        "C2 missing COMMIT startup-to-empty/reaped_us={}",
        started.elapsed().as_micros()
    );
    assert_retired(&mut claim, &proof);
    claim.finish().unwrap();
    drop(writer);
    launcher.wait().unwrap();
    remove_root(root);
}

#[cfg(windows)]
#[test]
fn s4_windows_job_membership_and_last_handle_closure() {
    let root = root("windows-job");
    #[cfg(target_os = "linux")]
    let _cleanup = FixtureCleanup(root.clone());
    let mut job = kinewright_job_sys::Job::create().unwrap();
    job.spawn_suspended(fixture(), &program(&root).arguments)
        .unwrap();
    job.verify().unwrap();
    assert_eq!(job.members().unwrap().len(), 1);
    assert!(!root.join("payload.json").exists());
    job.resume().unwrap();
    wait_file(&root.join("grandchild.json"));
    let members = job.members().unwrap();
    assert_eq!(members.len(), 2);
    for role in ["payload", "grandchild"] {
        assert!(members.iter().any(|p| p.pid() == fixture_pid(&root, role)));
    }
    drop(job);
    let deadline = Instant::now() + protocol::CLEANUP;
    while !members.iter().all(|p| p.reaped().unwrap()) {
        assert!(
            Instant::now() < deadline,
            "last Job-handle close must kill every member"
        );
        thread::sleep(Duration::from_millis(1));
    }
    remove_root(root);
}

#[cfg(windows)]
#[test]
fn s4_windows_guardian_death_retains_cleanup_reservation() {
    let root = root("windows-guardian-death");
    #[cfg(target_os = "linux")]
    let _cleanup = FixtureCleanup(root.clone());
    let id = identity();
    let machine = root.join("machine");
    let mut claim = MachineClaim::acquire_fixture(&machine, &id.project, &id.run).unwrap();
    let mut owner = Owner::new(&root);
    let mut session = ContainedSession::prepare(
        &mut claim,
        &mut owner,
        id,
        program(&root),
        guardian(),
        &CertifiedProfile::fixture(),
    )
    .unwrap();
    session.commit(&mut owner).unwrap();
    let descendants = pin_descendants(&mut session, &root);
    let guardian = ProcessHandle::open(session.recovery().guardian_pid).unwrap();
    guardian.kill().unwrap();
    let deadline = Instant::now() + protocol::CLEANUP;
    while !guardian.reaped().unwrap() || !descendants.iter().all(|p| p.reaped().unwrap()) {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(1));
    }
    assert!(
        session.cleanup().is_err(),
        "lost Job observation cannot release a reservation"
    );
    assert_eq!(claim.retained_attempts(), 1);
    assert!(MachineClaim::acquire_fixture(&machine, "d4", "e5").is_err());
    drop(session);
    drop(claim);
    remove_root(root);
}

#[cfg(windows)]
#[test]
fn s4_windows_real_host_death_kills_the_job() {
    let root = root("windows-host-death");
    #[cfg(target_os = "linux")]
    let _cleanup = FixtureCleanup(root.clone());
    let host = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "s4_native_host_fixture", "--nocapture"])
        .env("KINEWRIGHT_S4_HOST_FIXTURE", &root)
        .env("KINEWRIGHT_S4_NATIVE", fixture())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut host = FixtureChild(host);
    wait_file(&root.join("host-ready.json"));
    let recovery: RecoveryIdentity =
        serde_json::from_slice(&fs::read(root.join("host-ready.json")).unwrap()).unwrap();
    let guardian = ProcessHandle::open(recovery.guardian_pid).unwrap();
    assert_eq!(guardian.start(), recovery.guardian_start);
    let child = ProcessHandle::open(fixture_pid(&root, "payload")).unwrap();
    let grandchild = ProcessHandle::open(fixture_pid(&root, "grandchild")).unwrap();
    let processes = [guardian, child, grandchild];
    assert!(processes.iter().all(|p| !p.reaped().unwrap()));
    let _holder = FixtureProcess(ProcessHandle::open(fixture_pid(&root, "writer-holder")).unwrap());
    host.kill().unwrap();
    host.wait().unwrap();
    let deadline = Instant::now() + protocol::CLEANUP;
    while !processes.iter().all(|p| p.reaped().unwrap()) {
        assert!(
            Instant::now() < deadline,
            "host death must empty the real Windows fixture Job"
        );
        thread::sleep(Duration::from_millis(1));
    }
    let machine = root.join("machine");
    assert!(MachineClaim::acquire_fixture(&machine, "d4", "e5").is_err());
    let recovered = MachineClaim::recover_fixture(&machine).unwrap();
    assert_eq!(
        recovered.retained_attempts(),
        1,
        "process exit alone cannot retire the claim"
    );
    drop(recovered);
    remove_root(root);
}

#[cfg(target_os = "linux")]
#[allow(
    clippy::explicit_write,
    reason = "CI must observe skips even with libtest capture"
)]
fn equipped_session(test: &str) -> bool {
    static PROBE: OnceLock<Result<(), String>> = OnceLock::new();
    let result = PROBE.get_or_init(|| {
        kinewright_job_sys::require_nsdelegate().map_err(|e| e.to_string())?;
        for args in [
            vec![
                "5s",
                "systemd-run",
                "--user",
                "--quiet",
                "--wait",
                "--collect",
                "/usr/bin/true",
            ],
            vec![
                "5s",
                "unshare",
                "--user",
                "--cgroup",
                "--mount",
                "--map-current-user",
                "--propagation",
                "private",
                "/bin/sh",
                "-c",
                "test \"$(cat /proc/self/cgroup)\" = '0::/'",
            ],
        ] {
            let output = Command::new("timeout")
                .args(args)
                .output()
                .map_err(|e| e.to_string())?;
            if !output.status.success() {
                return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
            }
        }
        let unit = identity().unit();
        let output = Command::new("timeout")
            .args([
                "5s",
                "systemd-run",
                "--user",
                "--quiet",
                "--wait",
                "--collect",
                "--pipe",
            ])
            .arg(format!("--unit={unit}"))
            .arg(guardian())
            .arg("--probe")
            .arg(&unit)
            .output()
            .map_err(|e| e.to_string())?;
        if !output.status.success() {
            let _ = Command::new("systemctl")
                .args(["--user", "stop", &unit])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
        }
        Ok(())
    });
    if let Err(reason) = result {
        use std::io::Write as _;
        writeln!(
            std::io::stderr(),
            "\nC2_NATIVE_NOT_RUN test={test} reason={reason}"
        )
        .unwrap();
        return false;
    }
    true
}

#[cfg(target_os = "linux")]
fn migration_refused(outside: bool) {
    let root = root("migration");
    let _cleanup = FixtureCleanup(root.clone());
    let id = identity();
    let mut claim =
        MachineClaim::acquire_fixture(&root.join("machine"), &id.project, &id.run).unwrap();
    let mut owner = Owner::new(&root);
    let mut native = program(&root);
    native.arguments[1] = "migration-payload".into();
    let mut session = ContainedSession::prepare(
        &mut claim,
        &mut owner,
        id.clone(),
        native,
        guardian(),
        &CertifiedProfile::fixture(),
    )
    .unwrap();
    let service = Path::new(&session.recovery().containment).parent().unwrap();
    let parent = if outside {
        service.parent().unwrap()
    } else {
        service
    };
    let parent_group = kinewright_job_sys::Cgroup::open(parent).unwrap();
    let sibling = parent_group
        .create_child(&format!("c2-{}", id.nonce))
        .unwrap();
    let _sibling_cleanup = SiblingCleanup(sibling.path().to_owned());
    let targets = if outside {
        vec![sibling.path().join("cgroup.procs")]
    } else {
        vec![
            service.join("cgroup.procs"),
            sibling.path().join("cgroup.procs"),
        ]
    };
    fs::write(
        root.join("migration-targets.txt"),
        targets
            .iter()
            .map(|p| p.to_string_lossy())
            .collect::<Vec<_>>()
            .join("\n"),
    )
    .unwrap();
    session.commit(&mut owner).unwrap();
    wait_file(&root.join("migration-result.json"));
    let result: Vec<serde_json::Value> =
        serde_json::from_slice(&fs::read(root.join("migration-result.json")).unwrap()).unwrap();
    let descendant = ProcessHandle::open(fixture_pid(&root, "migration-child")).unwrap();
    assert!(!descendant.reaped().unwrap());
    let refused = result.iter().all(|v| v["refused"] == true);
    let mut observer = RecoveryObserver::open(session.recovery()).unwrap();
    ProcessHandle::open(session.recovery().guardian_pid)
        .unwrap()
        .kill()
        .unwrap();
    drop(session);
    assert_eq!(
        claim.retained_attempts(),
        1,
        "guardian death alone cannot retire the claim"
    );
    assert!(MachineClaim::acquire_fixture(&root.join("machine"), "d4", "e5").is_err());
    let outcome = observer.wait_for_cleanup();
    if outcome.is_err() || !descendant.reaped().unwrap() {
        descendant.kill().unwrap();
        let _ = observer.stop_tree();
    }
    assert!(
        refused,
        "namespace must refuse every attempted migration: {result:?}"
    );
    let proof = outcome.unwrap();
    assert!(
        descendant.reaped().unwrap(),
        "no descendant survives guardian death"
    );
    assert_retired(&mut claim, &proof);
    claim.finish().unwrap();
    if let Err(error) = sibling.try_remove() {
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
    }
    remove_root(root);
}

#[cfg(target_os = "linux")]
struct SiblingCleanup(PathBuf);
#[cfg(target_os = "linux")]
impl Drop for SiblingCleanup {
    fn drop(&mut self) {
        let _ = fs::remove_dir(&self.0);
    }
}

#[cfg(target_os = "linux")]
#[test]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn s4_c2_migration_outside_service_refused() {
    if !equipped_session("s4_c2_migration_outside_service_refused") {
        return;
    }
    migration_refused(true);
}

#[cfg(target_os = "linux")]
#[test]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn s4_c2_migration_to_service_root_refused() {
    if !equipped_session("s4_c2_migration_to_service_root_refused") {
        return;
    }
    migration_refused(false);
}

#[cfg(target_os = "linux")]
#[test]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn s4_c2_runtime_dir_masked() {
    if !equipped_session("s4_c2_runtime_dir_masked") {
        return;
    }
    let runtime = PathBuf::from(format!("/run/user/{}", rustix::process::getuid().as_raw()));
    for socket in ["systemd/private", "bus"] {
        assert!(
            fs::metadata(runtime.join(socket))
                .unwrap()
                .file_type()
                .is_socket(),
            "real host socket prerequisite"
        );
    }
    let root = root("runtime-mask");
    let _cleanup = FixtureCleanup(root.clone());
    let id = identity();
    let mut claim =
        MachineClaim::acquire_fixture(&root.join("machine"), &id.project, &id.run).unwrap();
    let mut owner = Owner::new(&root);
    let mut native = program(&root);
    native.arguments[1] = "runtime-mask".into();
    let mut session = ContainedSession::prepare(
        &mut claim,
        &mut owner,
        id,
        native,
        guardian(),
        &CertifiedProfile::fixture(),
    )
    .unwrap();
    session.commit(&mut owner).unwrap();
    wait_file(&root.join("runtime-result.json"));
    let result: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join("runtime-result.json")).unwrap()).unwrap();
    session.close_lifetime();
    let proof = session.cleanup().unwrap();
    assert_retired(&mut claim, &proof);
    claim.finish().unwrap();
    assert_eq!(
        result["empty"], true,
        "the private runtime tmpfs must be empty"
    );
    assert_eq!(
        result["connections_refused"],
        serde_json::json!([true, true]),
        "both user-manager and session-bus path sockets must be masked"
    );
    remove_root(root);
}

#[cfg(target_os = "linux")]
#[test]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn s4_c2_launch_refuses_without_nsdelegate() {
    if !equipped_session("s4_c2_launch_refuses_without_nsdelegate") {
        return;
    }
    let root = root("missing-nsdelegate");
    let _cleanup = FixtureCleanup(root.clone());
    let id = identity();
    let mut claim =
        MachineClaim::acquire_fixture(&root.join("machine"), &id.project, &id.run).unwrap();
    let mut owner = Owner::new(&root);
    let mut profile = CertifiedProfile::fixture();
    profile.missing_nsdelegate = true;
    let outcome = ContainedSession::prepare(
        &mut claim,
        &mut owner,
        id,
        program(&root),
        guardian(),
        &profile,
    );
    let reason = match outcome {
        Err(error) => error.to_string(),
        Ok(mut session) => {
            session.close_lifetime();
            let _ = session.cleanup();
            String::new()
        }
    };
    assert!(
        reason.contains("cgroup_nsdelegate_unavailable"),
        "missing prerequisite must refuse launch with its named reason: {reason}"
    );
    assert!(
        !root.join("payload.json").exists(),
        "no fallback provider launch"
    );
    remove_root(root);
}

#[cfg(target_os = "linux")]
#[test]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn s4_c2_blocked_guardian_exits_and_manager_kills() {
    if !equipped_session("s4_c2_blocked_guardian_exits_and_manager_kills") {
        return;
    }
    let root = root("blocked-manager");
    let _cleanup = FixtureCleanup(root.clone());
    fs::write(root.join("stubborn"), b"").unwrap();
    let child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "s4_native_host_fixture", "--nocapture"])
        .env("KINEWRIGHT_S4_HOST_FIXTURE", &root)
        .env("KINEWRIGHT_S4_NATIVE", fixture())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut host = FixtureChild(child);
    wait_file(&root.join("host-ready.json"));
    let recovery: RecoveryIdentity =
        serde_json::from_slice(&fs::read(root.join("host-ready.json")).unwrap()).unwrap();
    let mut observer = RecoveryObserver::open(&recovery).unwrap();
    let descendant = ProcessHandle::open(fixture_pid(&root, "grandchild")).unwrap();
    let guardian_process = ProcessHandle::open(recovery.guardian_pid).unwrap();
    let stat = fs::read_to_string(format!("/proc/{}/status", descendant.pid())).unwrap();
    let ignored = stat
        .lines()
        .find_map(|l| l.strip_prefix("SigIgn:\t"))
        .unwrap();
    assert_ne!(
        u64::from_str_radix(ignored, 16).unwrap() & (1 << 14),
        0,
        "fixture must ignore SIGTERM"
    );
    // Only the observer fixture migrates it, deliberately bypassing the child's namespace.
    let service = Path::new(&recovery.containment).parent().unwrap();
    fs::write(service.join("cgroup.procs"), descendant.pid().to_string()).unwrap();
    fs::write(root.join("cancel"), b"").unwrap();
    let private = root
        .join("machine")
        .join(format!("job-{}", recovery.attempt.nonce));
    wait_file(&private.join("native-completion.json"));
    let completion: GuardianMessage =
        serde_json::from_slice(&fs::read(private.join("native-completion.json")).unwrap()).unwrap();
    assert!(matches!(completion, GuardianMessage::CleanupBlocked { .. }));
    let _holder = FixtureProcess(ProcessHandle::open(fixture_pid(&root, "writer-holder")).unwrap());
    host.kill().unwrap();
    host.wait().unwrap();
    let mut recovered = MachineClaim::recover_fixture(&root.join("machine")).unwrap();
    assert_eq!(recovered.retained_attempts(), 1);
    assert!(MachineClaim::acquire_fixture(&root.join("machine"), "d4", "e5").is_err());
    let proof = observer.wait_for_cleanup().unwrap();
    assert!(
        guardian_process.reaped().unwrap(),
        "blocked guardian must exit so manager escalation can run"
    );
    assert!(
        descendant.reaped().unwrap(),
        "manager must kill the whole service subtree"
    );
    assert_retired(&mut recovered, &proof);
    recovered.finish().unwrap();
    remove_root(root);
}

struct FixtureChild(std::process::Child);
impl std::ops::Deref for FixtureChild {
    type Target = std::process::Child;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl std::ops::DerefMut for FixtureChild {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}
impl Drop for FixtureChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[cfg(target_os = "linux")]
struct FixtureCleanup(PathBuf);
#[cfg(target_os = "linux")]
impl Drop for FixtureCleanup {
    fn drop(&mut self) {
        fn visit(path: &Path) {
            let Ok(entries) = fs::read_dir(path) else {
                return;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    visit(&path);
                    continue;
                }
                if path.extension().is_none_or(|e| e != "json") {
                    continue;
                }
                let Ok(bytes) = fs::read(&path) else {
                    continue;
                };
                let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
                    continue;
                };
                if let (Some(pid), Some(start)) = (value["pid"].as_u64(), value["start"].as_u64())
                    && let Ok(pid) = u32::try_from(pid)
                    && let Ok(process) = ProcessHandle::open(pid)
                    && process.start() == start
                {
                    let _ = process.kill();
                    let deadline = Instant::now() + protocol::CLEANUP;
                    while !process.reaped().unwrap_or(false) && Instant::now() < deadline {
                        thread::sleep(Duration::from_millis(1));
                    }
                }
                if let Some(nonce) = value["attempt"]["nonce"].as_str()
                    && nonce.len() == 32
                    && nonce.bytes().all(|b| b.is_ascii_hexdigit())
                {
                    let unit = format!("kinewright-job-{nonce}.service");
                    for args in [
                        vec!["kill", "--kill-whom=all", "--signal=KILL"],
                        vec!["stop"],
                        vec!["reset-failed"],
                    ] {
                        let _ = Command::new("timeout")
                            .args(["5s", "systemctl", "--user"])
                            .args(args)
                            .arg(&unit)
                            .stdout(Stdio::null())
                            .stderr(Stdio::null())
                            .status();
                    }
                }
            }
        }
        visit(&self.0);
    }
}

#[cfg(windows)]
#[test]
fn s4_c2_windows_claim_host_fixture() {
    let Some(root) = std::env::var_os("KINEWRIGHT_C2_CLAIM_FIXTURE") else {
        return;
    };
    let root = PathBuf::from(root);
    let role = std::env::var("KINEWRIGHT_C2_CLAIM_ROLE").unwrap();
    let outcome = MachineClaim::acquire("a1", "b2");
    let result = match &outcome {
        Ok(claim) => serde_json::json!({"root":claim.root(), "acquired":true}),
        Err(error) => serde_json::json!({"reason":error.to_string(), "acquired":false}),
    };
    let path = root.join(format!("claim-{role}.json"));
    if let Ok(mut claim) = outcome {
        let id = identity();
        claim.reserve(&id).unwrap();
        fs::write(
            root.join("claim-owned.partial"),
            serde_json::to_vec(&id).unwrap(),
        )
        .unwrap();
        fs::rename(
            root.join("claim-owned.partial"),
            root.join("claim-owned.json"),
        )
        .unwrap();
        fs::write(
            path.with_extension("partial"),
            serde_json::to_vec(&result).unwrap(),
        )
        .unwrap();
        fs::rename(path.with_extension("partial"), path).unwrap();
        loop {
            thread::sleep(Duration::from_millis(10));
        }
    }
    fs::write(
        path.with_extension("partial"),
        serde_json::to_vec(&result).unwrap(),
    )
    .unwrap();
    fs::rename(path.with_extension("partial"), path).unwrap();
}

#[cfg(windows)]
#[test]
#[cfg_attr(
    all(windows, not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn s4_c2_windows_claim_ignores_localappdata() {
    let root = root("windows-account-claim");
    let launch = |role: &str| {
        FixtureChild(
            Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "s4_c2_windows_claim_host_fixture", "--nocapture"])
                .env("KINEWRIGHT_C2_CLAIM_FIXTURE", &root)
                .env("KINEWRIGHT_C2_CLAIM_ROLE", role)
                .env("LOCALAPPDATA", root.join(format!("environment-{role}")))
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        )
    };
    let mut first = launch("first");
    wait_file(&root.join("claim-first.json"));
    let result: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join("claim-first.json")).unwrap()).unwrap();
    assert_eq!(
        result["acquired"], true,
        "CI must provide an unused account claim namespace"
    );
    let namespace = PathBuf::from(result["root"].as_str().unwrap());
    let check_refused = |role: &str| {
        let mut child = launch(role);
        wait_file(&root.join(format!("claim-{role}.json")));
        let result: serde_json::Value =
            serde_json::from_slice(&fs::read(root.join(format!("claim-{role}.json"))).unwrap())
                .unwrap();
        child.kill().ok();
        child.wait().unwrap();
        assert_eq!(
            result["acquired"], false,
            "same-account processes must meet despite different environments"
        );
        assert!(
            result["reason"]
                .as_str()
                .unwrap()
                .contains("studio_run_active")
        );
    };
    check_refused("second");
    first.kill().unwrap();
    first.wait().unwrap();
    check_refused("retained");
    let id: AttemptIdentity =
        serde_json::from_slice(&fs::read(root.join("claim-owned.json")).unwrap()).unwrap();
    let recovery = MachineClaim::recover().unwrap();
    assert_eq!(recovery.recovery_identities(), std::slice::from_ref(&id));
    drop(recovery);
    fs::remove_file(namespace.join(format!("attempt-{}.json", id.nonce))).unwrap();
    fs::remove_file(namespace.join("run.json")).unwrap();
    // No provider was launched. These are solely this process-only fixture's records.
    remove_root(root);
}

fn remove_root(root: PathBuf) {
    #[cfg(target_os = "linux")]
    drop(FixtureCleanup(root.clone()));
    fs::remove_dir_all(root).unwrap();
}

struct FixtureProcess(ProcessHandle);
impl Drop for FixtureProcess {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let deadline = Instant::now() + protocol::CLEANUP;
        while !self.0.reaped().unwrap_or(false) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(1));
        }
    }
}
