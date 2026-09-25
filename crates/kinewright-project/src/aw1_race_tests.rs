//! AW1 S0–S1 fix-round race/scenario review (Opus, 2026-09-25).
//!
//! Adversarial scenarios against the AF1 lock/discovery protocol, run as
//! ordinary cargo tests: threads, re-exec'd child test processes,
//! `Child::kill` (SIGKILL) and `process::exit` at `test_hook` points. No
//! `LD_PRELOAD`, no syscall interposition.
//!
//! Naming: `guard_*` must pass (the protocol holds); `defect_*` assert the
//! contract and were RED on 503d221 (each is a finding in
//! rereview-aw1-s01-race.md) — they are green recorded regressions now, one
//! added per fix commit. `RACE_ITERS` overrides the default 200 iterations.
//!
//! Mounted from lockfile.rs as a child module (`#[path]`) so it can reach the
//! private `file` of `LockfileHandle` for the forked-duplicate scenarios.

use std::{
    fs,
    io::Write as _,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc, Barrier,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use kinewright_media::test_support::TempDirectory;

use super::*;
use crate::{
    project::ProjectSaveError,
    recovery::{JOURNAL_MAGIC, allocate_journal_path, journal_file_name},
    save_headless,
    session::SidecarSession,
    sidecar::SidecarMode,
};

const CHILD_TEST: &str = "lockfile::race_tests::race_child";

fn iterations() -> usize {
    std::env::var("RACE_ITERS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(200)
}

fn claim(project: &Path, recovery: &Path, endpoint: &str) -> Result<AcquiredLock, LockfileError> {
    acquire_project_lock_with_policy(
        project,
        LockMode::Headless,
        endpoint,
        recovery,
        1,
        Duration::ZERO,
    )
}

struct Fx {
    dir: TempDirectory,
    project: PathBuf,
    recovery: PathBuf,
}

fn fixture(tag: &str) -> Fx {
    let dir = TempDirectory::new(tag);
    let project = dir.path("edit.kinewright");
    fs::write(&project, b"{}").unwrap();
    let recovery = dir.path("recovery");
    fs::create_dir(&recovery).unwrap();
    Fx {
        dir,
        project,
        recovery,
    }
}

/// A crashed previous owner: acquire, then drop without `release` (the
/// discovery stays, the lock frees).
fn plant_stale(fx: &Fx, endpoint: &str) -> PathBuf {
    let first = claim(&fx.project, &fx.recovery, endpoint).expect("the stale owner acquires");
    let discovery = first.handle.discovery.clone();
    drop(first);
    assert!(discovery.exists());
    discovery
}

fn patch_discovery(discovery: &Path, key: &str, value: serde_json::Value) {
    let mut claim: serde_json::Value =
        serde_json::from_slice(&fs::read(discovery).unwrap()).unwrap();
    claim[key] = value;
    fs::write(discovery, serde_json::to_vec_pretty(&claim).unwrap()).unwrap();
}

/// A child that is always reaped: killed on drop, then waited.
struct Kid(Child);

impl Drop for Kid {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[derive(Default)]
struct Opts<'a> {
    hook: Option<&'a str>,
    exit_at_hook: bool,
    cwd: Option<&'a Path>,
    env: Vec<(&'a str, String)>,
}

fn spawn(mode: &str, project: &Path, recovery: &Path, signals: &Path, opts: Opts<'_>) -> Kid {
    fs::create_dir_all(signals).unwrap();
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", CHILD_TEST, "--nocapture", "--test-threads=1"])
        .env("RACE_CHILD", mode)
        .env("RACE_PROJECT", project)
        .env("RACE_RECOVERY", recovery)
        .env("REV2_SIGNALS", signals)
        .env_remove("REV2_HOOK")
        .env_remove("REV2_EXIT")
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some(hook) = opts.hook {
        command.env("REV2_HOOK", hook);
    }
    if opts.exit_at_hook {
        command.env("REV2_EXIT", "1");
    }
    if let Some(cwd) = opts.cwd {
        command.current_dir(cwd);
    }
    for (key, value) in opts.env {
        command.env(key, value);
    }
    Kid(command.spawn().unwrap())
}

fn wait_for(path: &Path) {
    let until = Instant::now() + Duration::from_secs(30);
    while !path.exists() {
        assert!(Instant::now() < until, "waiting for {}", path.display());
        std::thread::sleep(Duration::from_micros(500));
    }
}

/// Wait for either signal; `true` when `first` landed.
fn wait_either(first: &Path, second: &Path) -> bool {
    let until = Instant::now() + Duration::from_secs(30);
    loop {
        if first.exists() {
            return true;
        }
        if second.exists() {
            return false;
        }
        assert!(Instant::now() < until, "waiting for {}", first.display());
        std::thread::sleep(Duration::from_micros(500));
    }
}

/// Child signals land atomically (temp + rename): the parent polls for
/// existence and must never read a created-but-unwritten file.
fn signal(path: &Path, contents: impl AsRef<[u8]>) -> std::io::Result<()> {
    let temp = path.with_extension("signal-tmp");
    fs::write(&temp, contents)?;
    fs::rename(&temp, path)
}

fn describe(pid: u32, endpoint: &str, reclaimed: Option<&ReclaimedOwner>) -> String {
    match reclaimed {
        Some(previous) => format!(
            "pid={pid} endpoint={endpoint} reclaimed={} reclaimed_endpoint={}",
            previous.pid, previous.endpoint
        ),
        None => format!("pid={pid} endpoint={endpoint} reclaimed=none reclaimed_endpoint=none"),
    }
}

fn field<'a>(text: &'a str, key: &str) -> &'a str {
    text.split_whitespace()
        .find_map(|pair| {
            pair.strip_prefix(key)
                .and_then(|rest| rest.strip_prefix('='))
        })
        .unwrap_or_else(|| panic!("no {key} in {text}"))
}

/// Tiny xorshift: no `rand` dependency.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

#[derive(Debug, Default)]
struct HammerStats {
    acquired: usize,
    contended: usize,
    reclaims: usize,
    violations: Vec<String>,
    errors: Vec<String>,
}

/// The mutual-exclusion witness loop shared by threads and processes.
/// While owning: an exclusive `create_new` marker must succeed (a second
/// concurrent owner fails it), the discovery must name us, and the reclaim
/// verdict must match how the previous owner exited (release → none,
/// crash-drop → that owner's endpoint).
fn hammer(
    project: &Path,
    recovery: &Path,
    witness: &Path,
    tag: &str,
    rounds: usize,
    seed: u64,
) -> HammerStats {
    let marker = witness.join("owner.marker");
    let last_exit = witness.join("last-exit");
    let own = current_hostname();
    let mut rng = Rng(seed | 1);
    let mut stats = HammerStats::default();
    for round in 0..rounds {
        let endpoint = format!("http://127.0.0.1:1/{tag}-{round}");
        match claim(project, recovery, &endpoint) {
            Ok(acquired) => {
                stats.acquired += 1;
                let stamped = match fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&marker)
                {
                    Ok(mut file) => {
                        let _ = file.write_all(endpoint.as_bytes());
                        true
                    }
                    Err(error) => {
                        stats.violations.push(format!(
                            "{endpoint}: TWO OWNERS — marker held by {:?} ({error})",
                            fs::read_to_string(&marker).ok()
                        ));
                        false
                    }
                };
                match read_owner(&acquired.handle.discovery, &own) {
                    DiscoveryRead::Owner(owner) if owner.endpoint == endpoint => {}
                    other => stats
                        .violations
                        .push(format!("{endpoint}: discovery names {other:?}")),
                }
                let previous = fs::read_to_string(&last_exit).ok();
                let previous = previous.as_deref().and_then(|text| text.split_once(' '));
                match (previous, acquired.reclaimed.as_ref()) {
                    (Some(("drop", dropped)), Some(reclaimed)) if reclaimed.endpoint == dropped => {
                        stats.reclaims += 1;
                    }
                    (Some(("release", _)) | None, None) => {}
                    (previous, reclaimed) => stats.violations.push(format!(
                        "{endpoint}: previous exit {previous:?} but reclaimed {reclaimed:?}"
                    )),
                }
                for _ in 0..(rng.next() % 200) {
                    std::hint::spin_loop();
                }
                let crash = rng.next().is_multiple_of(3);
                fs::write(
                    &last_exit,
                    format!("{} {endpoint}", if crash { "drop" } else { "release" }),
                )
                .unwrap();
                if stamped {
                    fs::remove_file(&marker).unwrap();
                }
                if crash {
                    drop(acquired);
                } else if let Err(error) = acquired.handle.release() {
                    stats.errors.push(format!("release: {error}"));
                }
            }
            Err(LockfileError::Contention { .. }) => {
                stats.contended += 1;
                std::thread::sleep(Duration::from_micros(rng.next() % 300));
            }
            Err(error) => stats.errors.push(format!("{error:?}")),
        }
        if rng.next().is_multiple_of(4) {
            std::thread::yield_now();
        }
    }
    stats
}

fn vm_hwm_kib() -> u64 {
    fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            status.lines().find_map(|line| {
                line.strip_prefix("VmHWM:")
                    .and_then(|rest| rest.trim().trim_end_matches("kB").trim().parse().ok())
            })
        })
        .unwrap_or(0)
}

/// Re-exec entry: a plain test run returns immediately.
#[test]
fn race_child() {
    let Ok(mode) = std::env::var("RACE_CHILD") else {
        return;
    };
    let var = |key: &str| PathBuf::from(std::env::var_os(key).unwrap());
    let project = var("RACE_PROJECT");
    let recovery = var("RACE_RECOVERY");
    let signals = var("REV2_SIGNALS");
    let me = std::process::id();
    let endpoint = format!("http://127.0.0.1:1/child-{me}");
    match mode.as_str() {
        // One claim; hold until `release` (explicit release) or `done` (drop).
        "hold" => {
            if std::env::var_os("RACE_WAIT_START").is_some() {
                wait_for(&signals.join("start"));
            }
            match claim(&project, &recovery, &endpoint) {
                Ok(acquired) => {
                    signal(
                        &signals.join("owned"),
                        describe(me, &endpoint, acquired.reclaimed.as_ref()),
                    )
                    .unwrap();
                    if wait_either(&signals.join("release"), &signals.join("done")) {
                        acquired.handle.release().unwrap();
                    } else {
                        drop(acquired);
                    }
                    fs::write(signals.join("exited"), "").unwrap();
                }
                Err(error) => signal(&signals.join("error"), format!("{error:?}")).unwrap(),
            }
        }
        // Spin on single-attempt claims until owning, `stop`, or a non-
        // contention error.
        "waiter" => loop {
            if signals.join("stop").exists() {
                signal(&signals.join("error"), "stopped").unwrap();
                return;
            }
            match claim(&project, &recovery, &endpoint) {
                Ok(acquired) => {
                    signal(
                        &signals.join("owned"),
                        describe(me, &endpoint, acquired.reclaimed.as_ref()),
                    )
                    .unwrap();
                    wait_for(&signals.join("done"));
                    acquired.handle.release().unwrap();
                    return;
                }
                Err(LockfileError::Contention { .. }) => {
                    std::thread::sleep(Duration::from_micros(200));
                }
                Err(error) => {
                    signal(&signals.join("error"), format!("{error:?}")).unwrap();
                    return;
                }
            }
        },
        "hammer" => {
            wait_for(&signals.join("start"));
            let witness = var("RACE_WITNESS");
            let rounds: usize = std::env::var("RACE_ROUNDS").unwrap().parse().unwrap();
            let stats = hammer(
                &project,
                &recovery,
                &witness,
                &format!("p{me}"),
                rounds,
                u64::from(me),
            );
            signal(
                &signals.join(format!("result-{me}")),
                format!(
                    "acquired={} contended={} reclaims={} violations={} errors={}\n{:#?}\n{:#?}",
                    stats.acquired,
                    stats.contended,
                    stats.reclaims,
                    stats.violations.len(),
                    stats.errors.len(),
                    stats.violations,
                    stats.errors
                ),
            )
            .unwrap();
        }
        // Peak RSS + wall time of one acquire (the journal-scan probe).
        "measure" => {
            let before = vm_hwm_kib();
            let began = Instant::now();
            let verdict = claim(&project, &recovery, &endpoint).map(|_| ());
            let elapsed = began.elapsed();
            signal(
                &signals.join("measured"),
                format!(
                    "ms={} hwm_before_kib={before} hwm_after_kib={} verdict={verdict:?}",
                    elapsed.as_millis(),
                    vm_hwm_kib()
                ),
            )
            .unwrap();
        }
        other => panic!("unknown RACE_CHILD mode {other}"),
    }
}

// ───────────────────────── Contention ─────────────────────────

/// N threads race single-attempt claims for `iterations()` rounds each,
/// mixing clean releases and crash-drops: never two owners at any instant,
/// the discovery always names the holder, and reclaim verdicts track how
/// the previous owner exited.
#[test]
fn guard_threads_hammer_one_owner_at_every_instant() {
    let fx = fixture("race-thread-hammer");
    let witness = fx.dir.path("witness");
    fs::create_dir(&witness).unwrap();
    let start = Arc::new(Barrier::new(8));
    let workers: Vec<_> = (0..8u64)
        .map(|index| {
            let (project, recovery, witness, start) = (
                fx.project.clone(),
                fx.recovery.clone(),
                witness.clone(),
                start.clone(),
            );
            std::thread::spawn(move || {
                start.wait();
                hammer(
                    &project,
                    &recovery,
                    &witness,
                    &format!("t{index}"),
                    iterations() * 5,
                    0x9e37_79b9 * (index + 1),
                )
            })
        })
        .collect();
    let stats: Vec<HammerStats> = workers.into_iter().map(|w| w.join().unwrap()).collect();
    let acquired: usize = stats.iter().map(|s| s.acquired).sum();
    let contended: usize = stats.iter().map(|s| s.contended).sum();
    let reclaims: usize = stats.iter().map(|s| s.reclaims).sum();
    eprintln!(
        "RACE: threads hammer 8x{}: acquired={acquired} contended={contended} reclaims={reclaims}",
        iterations() * 5
    );
    for s in &stats {
        assert!(s.violations.is_empty(), "violations: {:#?}", s.violations);
        assert!(s.errors.is_empty(), "errors: {:#?}", s.errors);
    }
    assert!(
        acquired > 0 && contended > 0,
        "the race actually interleaved"
    );
}

/// Six child processes plus four parent threads hammer the same lock.
#[test]
fn guard_processes_hammer_one_owner_at_every_instant() {
    let fx = fixture("race-proc-hammer");
    let witness = fx.dir.path("witness");
    fs::create_dir(&witness).unwrap();
    let signals = fx.dir.path("signals");
    let kids: Vec<Kid> = (0..6)
        .map(|_| {
            spawn(
                "hammer",
                &fx.project,
                &fx.recovery,
                &signals,
                Opts {
                    env: vec![
                        ("RACE_WITNESS", witness.display().to_string()),
                        ("RACE_ROUNDS", (iterations() * 5).to_string()),
                    ],
                    ..Opts::default()
                },
            )
        })
        .collect();
    fs::write(signals.join("start"), "").unwrap();
    let threads: Vec<_> = (0..4u64)
        .map(|index| {
            let (project, recovery, witness) =
                (fx.project.clone(), fx.recovery.clone(), witness.clone());
            std::thread::spawn(move || {
                hammer(
                    &project,
                    &recovery,
                    &witness,
                    &format!("pt{index}"),
                    iterations() * 5,
                    0x51_7cc1 * (index + 3),
                )
            })
        })
        .collect();
    let stats: Vec<HammerStats> = threads.into_iter().map(|w| w.join().unwrap()).collect();
    let mut child_acquired = 0;
    let mut child_contended = 0;
    for mut kid in kids {
        let pid = kid.0.id();
        assert!(
            kid.0.wait().unwrap().success(),
            "hammer child {pid} exits cleanly"
        );
        let result = fs::read_to_string(signals.join(format!("result-{pid}"))).unwrap();
        assert!(
            result.contains("violations=0 errors=0"),
            "child {pid}: {result}"
        );
        child_acquired += field(&result, "acquired").parse::<usize>().unwrap();
        child_contended += field(&result, "contended").parse::<usize>().unwrap();
    }
    for s in &stats {
        assert!(s.violations.is_empty(), "violations: {:#?}", s.violations);
        assert!(s.errors.is_empty(), "errors: {:#?}", s.errors);
    }
    let parent_acquired: usize = stats.iter().map(|s| s.acquired).sum();
    eprintln!(
        "RACE: process hammer 6 children + 4 threads x{}: child acquired={child_acquired} contended={child_contended}, parent acquired={parent_acquired}",
        iterations() * 5
    );
    assert!(child_acquired > 0 && parent_acquired > 0);
}

/// Barrier rounds: 8 threads claim at once and winners HOLD until everyone
/// has tried — exactly one winner per round.
#[test]
fn guard_thread_rounds_exactly_one_winner() {
    let fx = fixture("race-thread-rounds");
    for round in 0..iterations() {
        let start = Arc::new(Barrier::new(8));
        let finish = Arc::new(Barrier::new(8));
        let workers: Vec<_> = (0..8)
            .map(|index| {
                let (project, recovery, start, finish) = (
                    fx.project.clone(),
                    fx.recovery.clone(),
                    start.clone(),
                    finish.clone(),
                );
                std::thread::spawn(move || {
                    start.wait();
                    let got = claim(&project, &recovery, &format!("http://r{round}/{index}"));
                    finish.wait();
                    match got {
                        Ok(acquired) => {
                            acquired.handle.release().unwrap();
                            1
                        }
                        Err(LockfileError::Contention { .. }) => 0,
                        Err(error) => panic!("round {round}: {error:?}"),
                    }
                })
            })
            .collect();
        let winners: usize = workers.into_iter().map(|w| w.join().unwrap()).sum();
        assert_eq!(winners, 1, "round {round}");
    }
}

/// Barrier rounds across processes: 4 children + the parent claim at once.
#[test]
fn guard_process_rounds_exactly_one_winner() {
    let fx = fixture("race-proc-rounds");
    let rounds = iterations();
    let mut parent_wins = 0;
    for round in 0..rounds {
        let base = fx.dir.path(&format!("round-{round}"));
        let kids: Vec<(PathBuf, Kid)> = (0..4)
            .map(|index| {
                let signals = base.join(index.to_string());
                let kid = spawn(
                    "hold",
                    &fx.project,
                    &fx.recovery,
                    &signals,
                    Opts {
                        env: vec![("RACE_WAIT_START", "1".to_owned())],
                        ..Opts::default()
                    },
                );
                (signals, kid)
            })
            .collect();
        for (signals, _) in &kids {
            fs::write(signals.join("start"), "").unwrap();
        }
        let mine = claim(&fx.project, &fx.recovery, "http://parent/round");
        let mut owners = usize::from(mine.is_ok());
        for (signals, _) in &kids {
            if wait_either(&signals.join("owned"), &signals.join("error")) {
                owners += 1;
            } else {
                let error = fs::read_to_string(signals.join("error")).unwrap();
                assert!(error.starts_with("Contention"), "round {round}: {error}");
            }
        }
        assert_eq!(owners, 1, "round {round}: exactly one owner");
        if let Ok(mine) = mine {
            parent_wins += 1;
            mine.handle.release().unwrap();
        }
        for (signals, mut kid) in kids {
            fs::write(signals.join("release"), "").unwrap();
            assert!(kid.0.wait().unwrap().success());
        }
    }
    eprintln!("RACE: process rounds {rounds}: parent won {parent_wins}");
}

// ───────────────────────── Owner dies ─────────────────────────

#[derive(Clone, Copy, Debug, PartialEq)]
enum Death {
    Kill,
    Exit,
}

/// Kill (SIGKILL) or `process::exit` the owner at `hook`; a spinning waiter
/// reclaims; the parent checks the waiter's reclaim verdict and that the
/// waiter is the sole owner afterwards.
#[allow(clippy::too_many_lines)]
fn owner_dies_at(hook: &str, death: Death, rounds: usize) {
    let mut stale_rounds = 0;
    for round in 0..rounds {
        let fx = fixture(&format!("race-die-{hook}"));
        let prior_stale = round % 2 == 1;
        if prior_stale {
            plant_stale(&fx, "http://stale/owner");
            stale_rounds += 1;
        }
        let signals = fx.dir.path("owner");
        let release_hook = hook == "release_after_remove_before_unlock";
        let mut owner = spawn(
            "hold",
            &fx.project,
            &fx.recovery,
            &signals,
            Opts {
                hook: Some(hook),
                exit_at_hook: death == Death::Exit,
                ..Opts::default()
            },
        );
        let owner_pid = owner.0.id();
        if release_hook {
            wait_for(&signals.join("owned"));
            fs::write(signals.join("release"), "").unwrap();
        }
        wait_for(&signals.join("paused"));
        if death == Death::Kill {
            // While paused, the parent sees exactly what the hook implies.
            let probe = claim(&fx.project, &fx.recovery, "http://probe/while-paused");
            match hook {
                "after_lock_open_before_flock" => {
                    // The child has not flocked: the probe may own. Release it
                    // so the child's (never-resumed) state is what dies.
                    probe
                        .expect("before its flock the child excludes nothing")
                        .handle
                        .release()
                        .unwrap();
                }
                "after_flock_before_scan" | "after_flock_before_publish" => {
                    let Err(LockfileError::Contention { owner, .. }) = probe else {
                        panic!("round {round} {hook}: flocked child must exclude, got {probe:?}");
                    };
                    // With a stale discovery the contender names the DEAD owner
                    // (advisory; see nit N-ADVISORY).
                    assert_eq!(
                        owner.map(|o| o.endpoint),
                        prior_stale.then(|| "http://stale/owner".to_owned()),
                        "round {round} {hook}"
                    );
                }
                "after_publish_before_return" => {
                    let Err(LockfileError::Contention {
                        owner: Some(owner), ..
                    }) = probe
                    else {
                        panic!(
                            "round {round} {hook}: published child must exclude named, got {probe:?}"
                        );
                    };
                    assert_eq!(owner.pid, owner_pid);
                }
                "release_after_remove_before_unlock" => {
                    assert!(
                        matches!(probe, Err(LockfileError::Contention { owner: None, .. })),
                        "round {round} {hook}: mid-release still excludes, ownerless: {probe:?}"
                    );
                }
                other => panic!("{other}"),
            }
        }
        let waiter_signals = fx.dir.path("waiter");
        let mut waiter = spawn(
            "waiter",
            &fx.project,
            &fx.recovery,
            &waiter_signals,
            Opts::default(),
        );
        match death {
            Death::Kill => {
                owner.0.kill().unwrap();
                owner.0.wait().unwrap();
            }
            Death::Exit => assert_eq!(owner.0.wait().unwrap().code(), Some(77)),
        }
        wait_for(&waiter_signals.join("owned"));
        assert!(!waiter_signals.join("error").exists());
        let verdict = fs::read_to_string(waiter_signals.join("owned")).unwrap();
        let reclaimed = field(&verdict, "reclaimed_endpoint");
        let expected = match hook {
            "after_publish_before_return" => format!("http://127.0.0.1:1/child-{owner_pid}"),
            "release_after_remove_before_unlock" => "none".to_owned(),
            // The Kill variant's probe owned-and-released before the kill,
            // consuming any stale discovery.
            "after_lock_open_before_flock" if death == Death::Kill => "none".to_owned(),
            _ if prior_stale => "http://stale/owner".to_owned(),
            _ => "none".to_owned(),
        };
        assert_eq!(
            reclaimed, expected,
            "round {round} {hook} {death:?}: {verdict}"
        );
        // The waiter is now the sole, named owner.
        let after = claim(&fx.project, &fx.recovery, "http://probe/after");
        let Err(LockfileError::Contention {
            owner: Some(now), ..
        }) = after
        else {
            panic!("round {round} {hook}: the waiter must be the sole owner, got {after:?}");
        };
        assert_eq!(now.pid, waiter.0.id(), "round {round} {hook}");
        fs::write(waiter_signals.join("done"), "").unwrap();
        assert!(waiter.0.wait().unwrap().success());
    }
    eprintln!(
        "RACE: owner dies at {hook} via {death:?}: {rounds} rounds ({stale_rounds} with a prior stale discovery) clean"
    );
}

#[test]
fn guard_owner_killed_after_open_before_flock() {
    owner_dies_at("after_lock_open_before_flock", Death::Kill, iterations());
}

#[test]
fn guard_owner_killed_after_flock_before_scan() {
    owner_dies_at("after_flock_before_scan", Death::Kill, iterations());
}

#[test]
fn guard_owner_killed_before_publish() {
    owner_dies_at("after_flock_before_publish", Death::Kill, iterations());
}

#[test]
fn guard_owner_killed_after_publish() {
    owner_dies_at("after_publish_before_return", Death::Kill, iterations());
}

#[test]
fn guard_owner_killed_mid_release() {
    owner_dies_at(
        "release_after_remove_before_unlock",
        Death::Kill,
        iterations(),
    );
}

#[test]
fn guard_owner_exits_at_every_hook() {
    for hook in [
        "after_lock_open_before_flock",
        "after_flock_before_scan",
        "after_flock_before_publish",
        "after_publish_before_return",
        "release_after_remove_before_unlock",
    ] {
        owner_dies_at(hook, Death::Exit, iterations() / 4);
    }
}

/// Two waiters spin while the owner is killed after publishing: exactly
/// one reclaims, the other never owns beside it.
#[test]
fn guard_two_waiters_one_reclaims_after_a_kill() {
    for round in 0..iterations() {
        let fx = fixture("race-two-waiters");
        let signals = fx.dir.path("owner");
        let mut owner = spawn("hold", &fx.project, &fx.recovery, &signals, Opts::default());
        wait_for(&signals.join("owned"));
        let a = fx.dir.path("a");
        let b = fx.dir.path("b");
        let mut wa = spawn("waiter", &fx.project, &fx.recovery, &a, Opts::default());
        let mut wb = spawn("waiter", &fx.project, &fx.recovery, &b, Opts::default());
        owner.0.kill().unwrap();
        owner.0.wait().unwrap();
        let until = Instant::now() + Duration::from_secs(30);
        while !a.join("owned").exists() && !b.join("owned").exists() {
            assert!(Instant::now() < until);
            std::thread::sleep(Duration::from_micros(500));
        }
        // Let the loser spin a little against the winner, then stop it.
        std::thread::sleep(Duration::from_millis(5));
        for s in [&a, &b] {
            fs::write(s.join("stop"), "").unwrap();
        }
        let winners = [&a, &b].iter().filter(|s| s.join("owned").exists()).count();
        assert_eq!(winners, 1, "round {round}: exactly one waiter reclaims");
        for s in [&a, &b] {
            fs::write(s.join("done"), "").unwrap();
        }
        assert!(wa.0.wait().unwrap().success());
        assert!(wb.0.wait().unwrap().success());
        let winner = if a.join("owned").exists() { &a } else { &b };
        let verdict = fs::read_to_string(winner.join("owned")).unwrap();
        assert_eq!(
            field(&verdict, "reclaimed"),
            owner.0.id().to_string(),
            "round {round}: the winner reclaims the killed owner"
        );
    }
}

// ───────────────────────── Discovery file states ─────────────────────────

fn garbage_variants() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        ("empty", Vec::new()),
        ("torn-json", br#"{"format_version":1,"mode":"gui","project":"/x","pid":4"#.to_vec()),
        ("binary", vec![0xff, 0x00, 0xfe, 0x7f, 0x80]),
        ("wrong-shape", br"[1,2,3]".to_vec()),
        ("null", b"null".to_vec()),
        ("pid-overflow", br#"{"format_version":1,"mode":"gui","project":"/x","pid":99999999999,"hostname":"h","endpoint":"e","token_ref":"t","kinewright_version":"0","started_at_unix":0,"reclaimed_from":null}"#.to_vec()),
    ]
}

/// A missing discovery with a held lock: ownerless contention, and the
/// owner's release tolerates the absence.
#[test]
fn guard_missing_discovery_with_held_lock() {
    let fx = fixture("race-missing-discovery");
    let held = claim(&fx.project, &fx.recovery, "http://held").unwrap();
    fs::remove_file(&held.handle.discovery).unwrap();
    let probe = claim(&fx.project, &fx.recovery, "http://probe");
    assert!(
        matches!(probe, Err(LockfileError::Contention { owner: None, .. })),
        "{probe:?}"
    );
    held.handle
        .release()
        .expect("release tolerates a vanished discovery");
    claim(&fx.project, &fx.recovery, "http://after").expect("the lock frees");
}

/// Garbage discovery with a held lock: typed ownerless contention, no panic,
/// no false owner.
#[test]
fn guard_garbage_discovery_with_held_lock() {
    for (name, bytes) in garbage_variants() {
        let fx = fixture("race-garbage-held");
        let held = claim(&fx.project, &fx.recovery, "http://held").unwrap();
        fs::write(&held.handle.discovery, &bytes).unwrap();
        let probe = claim(&fx.project, &fx.recovery, "http://probe");
        assert!(
            matches!(probe, Err(LockfileError::Contention { owner: None, .. })),
            "{name}: {probe:?}"
        );
        held.handle.release().unwrap();
    }
}

/// A directory squatting on the discovery or lock path: typed errors only.
#[test]
fn guard_directories_on_the_lock_paths_are_typed() {
    let fx = fixture("race-dir-discovery");
    let discovery = discovery_path_for_project(Some(&fx.project)).unwrap();
    fs::create_dir(&discovery).unwrap();
    let got = claim(&fx.project, &fx.recovery, "http://a");
    assert!(matches!(got, Err(LockfileError::Io(_))), "{got:?}");
    // The failed publish unlocked: a second try is the same typed Io.
    let got = claim(&fx.project, &fx.recovery, "http://b");
    assert!(matches!(got, Err(LockfileError::Io(_))), "{got:?}");

    let fx = fixture("race-dir-lock");
    fs::create_dir(lockfile_path_for_project(Some(&fx.project)).unwrap()).unwrap();
    let got = claim(&fx.project, &fx.recovery, "http://a");
    assert!(matches!(got, Err(LockfileError::Io(_))), "{got:?}");
}

// ───────────────────────── Forked duplicates ─────────────────────────

/// A real child process inherits a duplicate of the owner's lock fd (as its
/// stdin — the same open file description) and outlives the release; a
/// fresh claimant in ANOTHER process must still own immediately. Both the
/// explicit `release` and the crash-style drop unlock.
#[cfg(unix)]
#[test]
fn guard_release_unlocks_past_a_forked_child_duplicate() {
    for round in 0..iterations() {
        let fx = fixture("race-fork-dup");
        let owner = claim(&fx.project, &fx.recovery, "http://owner").unwrap();
        let duplicate = owner.handle.file.try_clone().unwrap();
        let sleeper = Kid(Command::new("sleep")
            .arg("30")
            .stdin(Stdio::from(duplicate))
            .spawn()
            .unwrap());
        if round % 2 == 0 {
            owner.handle.release().unwrap();
        } else {
            drop(owner);
        }
        let signals = fx.dir.path("claimant");
        let mut claimant = spawn("hold", &fx.project, &fx.recovery, &signals, Opts::default());
        let claimed = wait_either(&signals.join("owned"), &signals.join("error"));
        let error = fs::read_to_string(signals.join("error")).unwrap_or_default();
        fs::write(signals.join("release"), "").unwrap();
        claimant.0.wait().unwrap();
        drop(sleeper);
        assert!(
            claimed,
            "round {round}: the forked duplicate held the release hostage: {error}"
        );
    }
}

// ───────────────────────── Paths and journals ─────────────────────────

/// AF2: five spellings of one project (real, file symlink in another dir,
/// relative at the cwd, `..` detour, symlinked directory) race as five
/// processes per round — exactly one owner.
#[cfg(unix)]
#[test]
fn guard_alias_spellings_race_one_owner() {
    let dir = TempDirectory::new("race-aliases");
    let real_dir = dir.path("real");
    let link_dir = dir.path("link");
    fs::create_dir(&real_dir).unwrap();
    fs::create_dir(&link_dir).unwrap();
    let real = real_dir.join("edit.kinewright");
    fs::write(&real, b"{}").unwrap();
    let file_alias = link_dir.join("alias.kinewright");
    std::os::unix::fs::symlink(&real, &file_alias).unwrap();
    let dir_alias = dir.path("linkdir");
    std::os::unix::fs::symlink(&real_dir, &dir_alias).unwrap();
    let recovery = dir.path("recovery");
    fs::create_dir(&recovery).unwrap();
    let spellings: Vec<(PathBuf, Option<PathBuf>)> = vec![
        (real.clone(), None),
        (file_alias, None),
        (PathBuf::from("edit.kinewright"), Some(real_dir.clone())),
        (
            real_dir.join("..").join("real").join("edit.kinewright"),
            None,
        ),
        (dir_alias.join("edit.kinewright"), None),
    ];
    for (spelling, _) in &spellings {
        if spelling.is_absolute() {
            assert_eq!(
                lockfile_path_for_project(Some(spelling)),
                lockfile_path_for_project(Some(&real)),
                "{}",
                spelling.display()
            );
        }
    }
    for round in 0..iterations() {
        let base = dir.path(&format!("round-{round}"));
        let kids: Vec<(PathBuf, Kid)> = spellings
            .iter()
            .enumerate()
            .map(|(index, (spelling, cwd))| {
                let signals = base.join(index.to_string());
                let kid = spawn(
                    "hold",
                    spelling,
                    &recovery,
                    &signals,
                    Opts {
                        cwd: cwd.as_deref(),
                        env: vec![("RACE_WAIT_START", "1".to_owned())],
                        ..Opts::default()
                    },
                );
                (signals, kid)
            })
            .collect();
        for (signals, _) in &kids {
            fs::write(signals.join("start"), "").unwrap();
        }
        let mut owners = 0;
        for (signals, _) in &kids {
            if wait_either(&signals.join("owned"), &signals.join("error")) {
                owners += 1;
            } else {
                let error = fs::read_to_string(signals.join("error")).unwrap();
                assert!(error.starts_with("Contention"), "round {round}: {error}");
            }
        }
        assert_eq!(owners, 1, "round {round}: one owner across five spellings");
        for (signals, mut kid) in kids {
            fs::write(signals.join("release"), "").unwrap();
            assert!(kid.0.wait().unwrap().success());
        }
    }
}

fn alias_header_journal(recovery: &Path, header_project: &Path, named_as: &Path) -> PathBuf {
    let journal = recovery.join(journal_file_name(named_as));
    let header = serde_json::json!({
        "format_version": 1,
        "project_path": header_project,
        "writer_format_version": 1,
        "initial_document": kinewright_core::Document::default(),
    });
    let mut bytes = JOURNAL_MAGIC.to_vec();
    bytes.extend_from_slice(&serde_json::to_vec(&header).unwrap());
    bytes.push(b'\n');
    fs::write(&journal, &bytes).unwrap();
    journal
}

/// The three journal shapes AF3 must see: base, `-N` suffix, alias-named.
fn plant_journal(fx: &Fx, shape: &str) -> PathBuf {
    match shape {
        "base" => {
            let journal = fx.recovery.join(journal_file_name(&fx.project));
            fs::write(&journal, b"").unwrap(); // Created, header not yet written.
            journal
        }
        "suffix" => {
            let base = fx.recovery.join(journal_file_name(&fx.project));
            let reserved =
                allocate_journal_path(&fx.recovery, Some(&fx.project), Path::new(""), &[&base]);
            let suffixed = allocate_journal_path(
                &fx.recovery,
                Some(&fx.project),
                Path::new(""),
                &[&base, &reserved],
            );
            fs::write(&suffixed, b"crash").unwrap();
            suffixed
        }
        "alias" => alias_header_journal(
            &fx.recovery,
            &fx.project,
            &fx.dir.path("elsewhere.kinewright"),
        ),
        other => panic!("{other}"),
    }
}

/// AF3 TOCTOU, guarded half: a journal appearing after the flock but
/// before the scan refuses — for every journal shape.
#[test]
fn guard_journal_appearing_before_the_scan_refuses() {
    for shape in ["base", "suffix", "alias"] {
        for round in 0..iterations() / 10 {
            let fx = fixture("race-journal-before-scan");
            let signals = fx.dir.path("owner");
            let mut owner = spawn(
                "hold",
                &fx.project,
                &fx.recovery,
                &signals,
                Opts {
                    hook: Some("after_flock_before_scan"),
                    ..Opts::default()
                },
            );
            wait_for(&signals.join("paused"));
            let journal = plant_journal(&fx, shape);
            fs::write(signals.join("resume"), "").unwrap();
            let claimed = wait_either(&signals.join("owned"), &signals.join("error"));
            if claimed {
                fs::write(signals.join("release"), "").unwrap();
            }
            owner.0.wait().unwrap();
            assert!(
                !claimed,
                "{shape} round {round}: a pending journal was ignored"
            );
            let error = fs::read_to_string(signals.join("error")).unwrap();
            assert!(
                error.starts_with("PendingRecovery")
                    && error.contains(&*journal.file_name().unwrap().to_string_lossy()),
                "{shape}: {error}"
            );
        }
    }
}

/// AF3 TOCTOU, open half: a journal created after the scan (by a writer
/// that does not hold the project lock — today's GUI `JournalWriter`, which
/// S5 has not yet put under the lock) is never seen: the claimant owns with
/// a pending journal on disk. AF3's refusal is only as strong as the rule
/// that journal writers hold the lock. Ignored until S5 wires the GUI lock.
#[test]
#[ignore = "S5 obligation: journal writers must hold the lock"]
fn defect_journal_appearing_after_the_scan_is_missed() {
    let mut missed = Vec::new();
    for shape in ["base", "suffix", "alias"] {
        let fx = fixture("race-journal-after-scan");
        let signals = fx.dir.path("owner");
        let mut owner = spawn(
            "hold",
            &fx.project,
            &fx.recovery,
            &signals,
            Opts {
                hook: Some("after_flock_before_publish"),
                ..Opts::default()
            },
        );
        wait_for(&signals.join("paused"));
        plant_journal(&fx, shape);
        fs::write(signals.join("resume"), "").unwrap();
        if wait_either(&signals.join("owned"), &signals.join("error")) {
            missed.push(shape);
            fs::write(signals.join("release"), "").unwrap();
        }
        owner.0.wait().unwrap();
    }
    assert!(
        missed.is_empty(),
        "owned with a pending journal on disk: {missed:?}"
    );
}

// ───────────────────────── Hostnames ─────────────────────────

/// A known-foreign stale claim: 8 concurrent claimants per round, 200
/// rounds — nobody owns, the foreign discovery is never touched.
#[test]
fn guard_foreign_stale_claim_concurrent_claimants_never_own() {
    let fx = fixture("race-foreign-concurrent");
    let discovery = plant_stale(&fx, "http://foreign");
    patch_discovery(
        &discovery,
        "hostname",
        serde_json::json!("other-machine.invalid"),
    );
    let before = fs::read(&discovery).unwrap();
    let (mut foreign, mut contended) = (0, 0);
    for round in 0..iterations() {
        let start = Arc::new(Barrier::new(8));
        let workers: Vec<_> = (0..8)
            .map(|index| {
                let (project, recovery, start) =
                    (fx.project.clone(), fx.recovery.clone(), start.clone());
                std::thread::spawn(move || {
                    start.wait();
                    claim(&project, &recovery, &format!("http://r{round}/{index}"))
                })
            })
            .collect();
        for worker in workers {
            match worker.join().unwrap() {
                Err(LockfileError::ForeignHost { host, .. }) => {
                    assert_eq!(host, "other-machine.invalid");
                    foreign += 1;
                }
                Err(LockfileError::Contention { owner, .. }) => {
                    // Transient: another claimant held the flock for its check.
                    assert_eq!(
                        owner.map(|o| o.hostname).as_deref(),
                        Some("other-machine.invalid")
                    );
                    contended += 1;
                }
                other => panic!("round {round}: nobody may own a foreign claim: {other:?}"),
            }
        }
        assert_eq!(fs::read(&discovery).unwrap(), before, "round {round}");
    }
    eprintln!(
        "RACE: foreign stale x8 threads x{}: ForeignHost={foreign} transient Contention={contended}",
        iterations()
    );
}

/// An `unknown`-host stale claim: 8 concurrent claimants per round, winners
/// hold — exactly one reclaims (with the warning), per round.
#[test]
fn guard_unknown_host_stale_claim_exactly_one_reclaims() {
    for round in 0..iterations() {
        let fx = fixture("race-unknown-concurrent");
        let discovery = plant_stale(&fx, "http://legacy");
        patch_discovery(&discovery, "hostname", serde_json::json!("unknown"));
        let start = Arc::new(Barrier::new(8));
        let finish = Arc::new(Barrier::new(8));
        let workers: Vec<_> = (0..8)
            .map(|index| {
                let (project, recovery, start, finish) = (
                    fx.project.clone(),
                    fx.recovery.clone(),
                    start.clone(),
                    finish.clone(),
                );
                std::thread::spawn(move || {
                    start.wait();
                    let got = claim(&project, &recovery, &format!("http://r{round}/{index}"));
                    finish.wait();
                    got.map(|acquired| {
                        let reclaimed = acquired.reclaimed.clone();
                        acquired.handle.release().unwrap();
                        reclaimed
                    })
                })
            })
            .collect();
        let mut winners = Vec::new();
        for worker in workers {
            match worker.join().unwrap() {
                Ok(reclaimed) => winners.push(reclaimed),
                Err(LockfileError::Contention { .. }) => {}
                Err(error) => panic!("round {round}: {error:?}"),
            }
        }
        assert_eq!(winners.len(), 1, "round {round}");
        let reclaimed = winners[0].as_ref().expect("the reclaim warns");
        assert_eq!(reclaimed.hostname, "unknown");
        assert_eq!(reclaimed.endpoint, "http://legacy");
    }
}

// ───────────────────────── G1: strict discovery ─────────────────────────

/// The discovery path is a symlink planted in the project folder (a cloned
/// repo / unzipped share): the publish must not write THROUGH it. Ruled
/// (G1): the strict writer renames over the link itself — the victim keeps
/// its bytes, the discovery becomes a regular file, and the acquire
/// succeeds (a refusal would `DoS` every acquire until manual cleanup).
#[cfg(unix)]
#[test]
fn defect_discovery_symlink_clobbers_its_target() {
    let fx = fixture("race-discovery-symlink");
    let victim = fx.dir.path("victim.txt");
    fs::write(&victim, b"precious user bytes").unwrap();
    let discovery = discovery_path_for_project(Some(&fx.project)).unwrap();
    std::os::unix::fs::symlink(&victim, &discovery).unwrap();
    let acquired = claim(&fx.project, &fx.recovery, "http://a")
        .expect("the publish replaces the planted link");
    assert_eq!(
        fs::read(&victim).unwrap(),
        b"precious user bytes",
        "the victim keeps its bytes"
    );
    assert!(
        fs::symlink_metadata(&discovery)
            .unwrap()
            .file_type()
            .is_file(),
        "the link itself was replaced by a regular discovery"
    );
    acquired.handle.release().unwrap();
    // A dangling planted link takes the same path: nothing is created at
    // its target, and the discovery lands as a regular file.
    let fx = fixture("race-discovery-dangling");
    let discovery = discovery_path_for_project(Some(&fx.project)).unwrap();
    let missing = fx.dir.path("never-created.txt");
    std::os::unix::fs::symlink(&missing, &discovery).unwrap();
    let acquired = claim(&fx.project, &fx.recovery, "http://a")
        .expect("the publish replaces a dangling planted link");
    assert!(!missing.exists(), "nothing is created through the link");
    assert!(
        fs::symlink_metadata(&discovery)
            .unwrap()
            .file_type()
            .is_file(),
        "the dangling link was replaced by a regular discovery"
    );
    acquired.handle.release().unwrap();
}

/// A FIFO squatting on the discovery path: the owner read must not block in
/// `open` — a contender returns promptly (ownerless contention while held,
/// a reclaim with a free lock), never hanging with the flock held. (A
/// regressed reader leaks its worker instead of hanging the suite: the
/// 5 s bound fails first, on a fixture-local lock no other test meets.)
#[cfg(unix)]
#[test]
fn fifo_discovery_does_not_hang_the_claim() {
    /// Claim on a worker; returns the verdict, or `None` past the bound.
    fn prompt_claim(project: PathBuf, recovery: PathBuf, endpoint: &str) -> Option<String> {
        let done = Arc::new(AtomicBool::new(false));
        let worker = {
            let done = done.clone();
            let endpoint = endpoint.to_owned();
            std::thread::spawn(move || {
                let got = claim(&project, &recovery, &endpoint);
                done.store(true, Ordering::SeqCst);
                format!("{got:?}")
            })
        };
        let until = Instant::now() + Duration::from_secs(5);
        while !done.load(Ordering::SeqCst) && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(5));
        }
        if !done.load(Ordering::SeqCst) {
            return None;
        }
        Some(worker.join().unwrap())
    }
    let fx = fixture("race-fifo-prompt");
    let held = claim(&fx.project, &fx.recovery, "http://held").unwrap();
    let discovery = held.handle.discovery.clone();
    fs::remove_file(&discovery).unwrap();
    assert!(
        Command::new("mkfifo")
            .arg(&discovery)
            .status()
            .unwrap()
            .success()
    );
    let verdict = prompt_claim(fx.project.clone(), fx.recovery.clone(), "http://probe")
        .expect("the held-lock claim returns promptly");
    assert!(
        verdict.starts_with("Err(Contention") && verdict.contains("owner: None"),
        "ownerless contention, got {verdict}"
    );
    fs::remove_file(&discovery).unwrap();
    held.handle.release().unwrap();
    // Free-lock leg: the claimant reads while holding the flock — it must
    // still return promptly, reclaiming (the unreadable warning is G7's).
    assert!(
        Command::new("mkfifo")
            .arg(&discovery)
            .status()
            .unwrap()
            .success()
    );
    let verdict = prompt_claim(fx.project.clone(), fx.recovery.clone(), "http://free")
        .expect("the free-lock claim returns promptly");
    assert!(
        verdict.starts_with("Ok("),
        "the free lock reclaims, got {verdict}"
    );
}

// ───────────────────────── G2: forked duplicates ─────────────────────────

/// The `ForeignHost` refusal path closes its flocked file with a bare `drop`
/// (lockfile.rs, `drop(file)` before `return Err(ForeignHost…)`) — not the
/// explicit `unlock_attempt` every other held-refusal path uses. Under a
/// concurrent spawn storm (every fork copies the fd table until exec), a
/// refusal can leave the flock held by a forked duplicate, so an immediate
/// second claimant reads `Contention` instead of `ForeignHost`. The release
/// path (explicit unlock) is the control.
#[cfg(unix)]
#[test]
fn defect_foreign_refusal_leaks_its_flock_to_forked_children() {
    let fx = fixture("race-foreign-storm");
    let discovery = plant_stale(&fx, "http://foreign");
    patch_discovery(
        &discovery,
        "hostname",
        serde_json::json!("other-machine.invalid"),
    );
    let stop = Arc::new(AtomicBool::new(false));
    let spawned = Arc::new(AtomicUsize::new(0));
    let storm: Vec<_> = (0..4)
        .map(|_| {
            let (stop, spawned) = (stop.clone(), spawned.clone());
            std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    if let Ok(mut child) = Command::new("true")
                        .stdin(Stdio::null())
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .spawn()
                    {
                        let _ = child.wait();
                        spawned.fetch_add(1, Ordering::Relaxed);
                    }
                }
            })
        })
        .collect();
    let rounds = iterations() * 25;
    let mut foreign_leaks = 0;
    let attempts = rounds * 2;
    for _ in 0..attempts {
        // Every claim must read ForeignHost; `Contention` means an earlier
        // refusal's flock is still held — by a forked duplicate.
        match claim(&fx.project, &fx.recovery, "http://a") {
            Err(LockfileError::ForeignHost { .. }) => {}
            Err(LockfileError::Contention { .. }) => foreign_leaks += 1,
            other => panic!("{other:?}"),
        }
    }
    // Control: the explicit-unlock release path under the same storm.
    let control = fixture("race-release-storm");
    let mut release_leaks = 0;
    for _ in 0..rounds {
        claim(&control.project, &control.recovery, "http://a")
            .unwrap()
            .handle
            .release()
            .unwrap();
        match claim(&control.project, &control.recovery, "http://b") {
            Ok(got) => got.handle.release().unwrap(),
            Err(_) => release_leaks += 1,
        }
    }
    stop.store(true, Ordering::Relaxed);
    for thread in storm {
        thread.join().unwrap();
    }
    eprintln!(
        "RACE: spawn storm ({} spawns): foreign-refusal leaks {foreign_leaks}/{attempts}, release-path leaks {release_leaks}/{rounds}",
        spawned.load(Ordering::Relaxed)
    );
    assert_eq!(
        release_leaks, 0,
        "the explicit-unlock control must be clean"
    );
    assert_eq!(
        foreign_leaks, 0,
        "the ForeignHost refusal left its flock with a forked duplicate"
    );
}

// ───────────────────────── G7: unreadable discovery ─────────────────────────

/// AF1: "Stale discovery with a free lock reclaims with a warning." A torn
/// or unparseable stale discovery reclaims WITH the typed unreadable
/// warning (G7) — except the pid-overflow shape, whose lenient hostname is
/// a known foreign host and refuses under AF5 instead. Nothing reclaims
/// silently.
#[test]
fn defect_garbage_stale_discovery_reclaims_without_warning() {
    let mut silent = Vec::new();
    for (name, bytes) in garbage_variants() {
        let fx = fixture("race-garbage-free");
        let discovery = plant_stale(&fx, "http://dead");
        fs::write(&discovery, &bytes).unwrap();
        if name == "pid-overflow" {
            let got = claim(&fx.project, &fx.recovery, "http://new");
            assert!(
                matches!(&got, Err(LockfileError::ForeignHost { host, .. }) if host == "h"),
                "{name}: a lenient foreign hostname refuses, got {got:?}"
            );
            continue;
        }
        let got = claim(&fx.project, &fx.recovery, "http://new").expect("the free lock reclaims");
        if got.reclaimed.is_none() || !got.reclaimed_unreadable {
            silent.push(name);
        } else {
            got.handle.release().unwrap();
        }
    }
    assert!(
        silent.is_empty(),
        "stale discovery reclaimed with no warning for: {silent:?}"
    );
}

/// AF5 fail-open: a stale claim from a KNOWN foreign host that this build
/// cannot parse (a newer writer: new `mode` variant, or a field this build
/// lacks) bypasses the foreign-host refusal and is reclaimed silently.
#[test]
fn defect_unparseable_foreign_claim_bypasses_foreign_host() {
    for (name, patch) in [
        ("future-mode", ("mode", serde_json::json!("cloud"))),
        ("pid-as-string", ("pid", serde_json::json!("4242"))),
    ] {
        let fx = fixture("race-future-foreign");
        let discovery = plant_stale(&fx, "http://foreign");
        patch_discovery(
            &discovery,
            "hostname",
            serde_json::json!("other-machine.invalid"),
        );
        patch_discovery(&discovery, "format_version", serde_json::json!(2));
        patch_discovery(&discovery, patch.0, patch.1);
        let got = claim(&fx.project, &fx.recovery, "http://local");
        assert!(
            matches!(&got, Err(LockfileError::ForeignHost { host, .. }) if host == "other-machine.invalid"),
            "{name}: a known foreign host must refuse, got {:?}",
            got.map(|a| a.reclaimed)
        );
    }
}

// ───────────────────────── G6: streaming scan ─────────────────────────

/// Fail-closed is global: ONE unreadable entry anywhere in the shared
/// recovery dir (here a directory named `*.journal`) refuses every
/// project's acquisition with `RecoveryLookup`.
#[test]
fn defect_unrelated_unreadable_journal_blocks_every_project() {
    let fx = fixture("race-junk-journal");
    fs::create_dir(fx.recovery.join("zz-unrelated-0000000000000000.journal")).unwrap();
    let got = claim(&fx.project, &fx.recovery, "http://a");
    assert!(
        got.is_ok(),
        "an unrelated recovery-dir entry blocked this project: {:?}",
        got.err()
    );
}

/// G6/RS3: the takeover scan streams headers — a 256 MiB unrelated journal
/// adds less than 16 MB peak RSS to one acquire. Child-measured `VmHWM`,
/// so the parent's allocator state cannot pollute the bound; at 503d221
/// the same shape peaked at ~270 MB.
#[cfg(target_os = "linux")]
#[test]
fn journal_scan_bounds_unrelated_reads() {
    let fx = fixture("race-scan-bounded");
    let other = fx.dir.path("other.kinewright");
    fs::write(&other, "{}").unwrap();
    let big = alias_header_journal(&fx.recovery, &other, &other);
    {
        let mut file = fs::OpenOptions::new().append(true).open(&big).unwrap();
        let chunk = vec![0x61; 1 << 20];
        for _ in 0..256 {
            file.write_all(&chunk).unwrap();
        }
    }
    assert_eq!(
        pending_journal_for_project(&fx.recovery, &fx.project).unwrap(),
        None,
        "the unrelated journal never pends"
    );
    let signals = fx.dir.path("measured");
    let mut kid = spawn(
        "measure",
        &fx.project,
        &fx.recovery,
        &signals,
        Opts::default(),
    );
    kid.0.wait().unwrap();
    let measured = fs::read_to_string(signals.join("measured")).unwrap();
    let field = |key: &str| {
        measured
            .split(key)
            .nth(1)
            .unwrap()
            .split_whitespace()
            .next()
            .unwrap()
            .parse::<u64>()
            .unwrap()
    };
    let before = field("hwm_before_kib=");
    let after = field("hwm_after_kib=");
    assert!(
        measured.contains("verdict=Ok(())"),
        "the unrelated journal never blocks: {measured}"
    );
    assert!(
        after.saturating_sub(before) < 16 * 1024,
        "one acquire adds < 16 MB peak RSS over a 256 MiB unrelated journal: {before} -> {after} KiB"
    );
}

// ───────────────────────── G8: dangling alias ─────────────────────────

/// AF2 hole: a DANGLING symlink alias (the target not yet saved) does not
/// canonicalise, so its identity is `<link dir>/<link name>` — a different
/// lock from the real target's. Two owners of one future file.
#[cfg(unix)]
#[test]
fn defect_dangling_symlink_alias_double_owns() {
    let dir = TempDirectory::new("race-dangling");
    let real_dir = dir.path("real");
    let link_dir = dir.path("link");
    fs::create_dir(&real_dir).unwrap();
    fs::create_dir(&link_dir).unwrap();
    let target = real_dir.join("new.kinewright");
    let alias = link_dir.join("alias.kinewright");
    std::os::unix::fs::symlink(&target, &alias).unwrap();
    let recovery = dir.path("recovery");
    fs::create_dir(&recovery).unwrap();
    let first = claim(&target, &recovery, "http://real").expect("the real spelling owns");
    let second = claim(&alias, &recovery, "http://alias");
    let double = second.is_ok();
    // And the identities later converge once the file exists — too late.
    fs::write(&target, b"{}").unwrap();
    let converged =
        lockfile_path_for_project(Some(&alias)) == lockfile_path_for_project(Some(&target));
    eprintln!(
        "RACE: dangling alias double-owns={double}, identities converge after first save={converged}"
    );
    drop(second);
    drop(first);
    assert!(!double, "a dangling alias owned beside the real target");
}

// ───────────────────────── G9: deleted lock object ─────────────────────────

/// AF1/S4, FIXED: the lock object deleted under a live owner. The live
/// owner's release must NOT remove the successor's discovery, and the live
/// owner's handle fails `verify` — so its next save refuses `LockLost`
/// instead of writing.
#[cfg(unix)]
#[test]
fn defect_lock_object_deleted_under_a_live_owner() {
    let fx = fixture("race-external-unlink");
    let lock = lockfile_path_for_project(Some(&fx.project)).unwrap();
    let a = claim(&fx.project, &fx.recovery, "http://a").expect("A claims");
    fs::remove_file(&lock).unwrap(); // The hand delete.
    let b = claim(&fx.project, &fx.recovery, "http://b").expect("B claims the re-created object");
    assert!(
        matches!(a.handle.verify(), Err(LockfileError::LockLost { .. })),
        "A's handle detects the swapped object"
    );
    assert!(
        b.handle.verify().is_ok(),
        "the current owner's handle verifies"
    );
    // A's next save refuses instead of writing.
    let log = std::sync::Arc::new(std::sync::RwLock::new(
        kinewright_core::IncidentLog::with_start(
            std::time::Instant::now(),
            Some(std::time::SystemTime::now()),
        ),
    ));
    let mut session = SidecarSession::load(
        &SidecarMode::Load {
            project_digest: String::new(),
        },
        Some(&fx.project),
        log,
        kinewright_core::TimelineRevision::default(),
        None,
        None,
    )
    .session;
    let before = fs::read(&fx.project).unwrap();
    let saved = save_headless(
        &kinewright_core::Document::default(),
        &fx.project,
        None,
        &mut session,
        "",
        kinewright_core::TimelineRevision::default(),
        &fx.recovery,
        Some(&a.handle),
    );
    assert!(
        matches!(saved, Err(ProjectSaveError::LockLost { .. })),
        "A's next save refuses LockLost"
    );
    assert_eq!(fs::read(&fx.project).unwrap(), before, "no write lands");
    // A's release leaves B's discovery alone.
    a.handle.release().unwrap();
    let discovery = discovery_path_for_project(Some(&fx.project)).unwrap();
    let after = fs::read_to_string(&discovery).expect("B's discovery survives A's release");
    assert!(
        after.contains("http://b"),
        "B's discovery still names B: {after}"
    );
    b.handle.release().unwrap();
    assert!(!discovery.exists(), "B's own release still cleans up");
}

// ───────────────────────── G11: hostname spelling ─────────────────────────

/// G11/N2: own-host spelling variants reclaim — the comparison is
/// case-insensitive after trimming a trailing dot. Anything else (an FQDN)
/// still refuses fail-closed, naming the discovery to delete if this
/// machine was renamed.
#[test]
fn own_host_spelling_variants_reclaim() {
    let own = current_hostname();
    assert_ne!(own, UNKNOWN_HOSTNAME, "the probe needs a real hostname");
    for (name, spelling) in [
        ("upper", own.to_uppercase()),
        ("trailing-dot", format!("{own}.")),
        ("both", format!("{}.", own.to_uppercase())),
    ] {
        let fx = fixture("race-host-spelling");
        let discovery = plant_stale(&fx, "http://old");
        patch_discovery(&discovery, "hostname", serde_json::json!(spelling));
        let got = claim(&fx.project, &fx.recovery, "http://new");
        assert!(got.is_ok(), "{name} ({spelling}) reclaims, got {got:?}");
    }
    let fx = fixture("race-host-fqdn");
    let discovery = plant_stale(&fx, "http://old");
    patch_discovery(
        &discovery,
        "hostname",
        serde_json::json!(format!("{own}.localdomain")),
    );
    match claim(&fx.project, &fx.recovery, "http://new") {
        Err(error @ LockfileError::ForeignHost { .. }) => {
            assert!(
                error.to_string().contains(&*discovery.to_string_lossy()),
                "the refusal names the discovery to delete: {error}"
            );
        }
        other => panic!("an FQDN still refuses, got {other:?}"),
    }
}
