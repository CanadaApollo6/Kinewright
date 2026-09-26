//! AW1 S0–S1 fix-round race/scenario review (Opus, 2026-09-25).
//!
//! Adversarial scenarios against the AF1 lock/discovery protocol, run as
//! ordinary cargo tests: threads, re-exec'd child test processes,
//! `Child::kill` (SIGKILL) and `process::exit` at `test_hook` points. No
//! `LD_PRELOAD`, no syscall interposition.
//!
//! Naming: `guard_*` must pass (the protocol holds). `defect_*` keep the
//! names of the findings in rereview-aw1-s01-race.md: each describes a
//! defect of the 503d221 baseline (history, not current behaviour), asserts
//! the fixed contract, and is a green regression pin since its fix commit.
//! The one exception is the `#[ignore]`d S5 obligation, which is still open.
//! `RACE_ITERS` overrides the default 200 iterations.
//!
//! Mounted from lockfile.rs as a child module (`#[path]`) so it can reach the
//! private `file` of `LockfileHandle` for the forked-duplicate scenarios.

#[cfg(unix)]
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::{
    fs,
    io::Write as _,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{Arc, Barrier},
    time::{Duration, Instant},
};

use kinewright_media::test_support::TempDirectory;

use super::*;
use crate::recovery::{JOURNAL_MAGIC, allocate_journal_path, journal_file_name};
#[cfg(unix)]
use crate::{
    project::ProjectSaveError, save_headless, session::SidecarSession, sidecar::SidecarMode,
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
        let verdict = settle_two_waiters(round, &a, &b, || {
            assert!(wa.0.wait().unwrap().success());
            assert!(wb.0.wait().unwrap().success());
        });
        assert_eq!(
            field(&verdict, "reclaimed"),
            owner.0.id().to_string(),
            "round {round}: the winner reclaims the killed owner"
        );
    }
}

/// Stop both waiters, then read the winner's verdict while it still holds
/// (J1) — only then write `done` and `reap` the children. A loser whose
/// claim began before `stop` may own once the winner releases; reading
/// after `done` could pick that late, fresh verdict (the round-3 B1
/// harness race). A late loser must own fresh: the winner released
/// cleanly, so it never reclaims the killed owner a second time.
fn settle_two_waiters(round: usize, a: &Path, b: &Path, reap: impl FnOnce()) -> String {
    for s in [a, b] {
        fs::write(s.join("stop"), "").unwrap();
    }
    let owners: Vec<&Path> = [a, b]
        .into_iter()
        .filter(|s| s.join("owned").exists())
        .collect();
    assert_eq!(
        owners.len(),
        1,
        "round {round}: exactly one waiter owns while the winner holds"
    );
    let winner = owners[0];
    let verdict = fs::read_to_string(winner.join("owned")).unwrap();
    for s in [a, b] {
        fs::write(s.join("done"), "").unwrap();
    }
    reap();
    let loser = if winner == a { b } else { a };
    if let Ok(late) = fs::read_to_string(loser.join("owned")) {
        assert_eq!(
            field(&late, "reclaimed"),
            "none",
            "round {round}: a late loser owns fresh, after the winner's release"
        );
    }
    verdict
}

/// J1 regression of the harness (the round-3 race review's deterministic
/// reproducer of CI 36205653408's Windows red): waiter `a` pauses inside
/// a claim it began before `stop`; `b` wins and reclaims the killed owner;
/// `a` resumes only after `done` and `b`'s release, so it owns FRESH. The
/// harness must still report `b`'s verdict — at 9a14698 it picked the
/// verdict after `done`, checking `a` first, and read `reclaimed=none`.
#[test]
fn j1_two_waiters_harness_reports_the_real_winner() {
    let fx = fixture("j1-two-waiters-harness");
    let signals = fx.dir.path("owner");
    let mut owner = spawn("hold", &fx.project, &fx.recovery, &signals, Opts::default());
    wait_for(&signals.join("owned"));
    let a = fx.dir.path("a");
    let b = fx.dir.path("b");
    let mut wa = spawn(
        "waiter",
        &fx.project,
        &fx.recovery,
        &a,
        Opts {
            hook: Some("before_lock_open"),
            ..Opts::default()
        },
    );
    wait_for(&a.join("paused"));
    let mut wb = spawn("waiter", &fx.project, &fx.recovery, &b, Opts::default());
    owner.0.kill().unwrap();
    owner.0.wait().unwrap();
    wait_for(&b.join("owned"));
    let verdict = settle_two_waiters(0, &a, &b, || {
        assert!(wb.0.wait().unwrap().success());
        fs::write(a.join("resume"), "").unwrap();
        assert!(wa.0.wait().unwrap().success());
    });
    assert!(
        a.join("owned").exists(),
        "the paused loser owned late (the race was exercised)"
    );
    assert_eq!(
        field(&verdict, "reclaimed"),
        owner.0.id().to_string(),
        "the harness reports the real winner's reclaim"
    );
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
/// repo / unzipped share). Baseline 503d221 published THROUGH it,
/// clobbering the target. Fixed by G1: the strict writer renames over the
/// link itself — the victim keeps its bytes, the discovery becomes a
/// regular file, and the acquire succeeds (a refusal would `DoS` every
/// acquire until manual cleanup).
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

/// Baseline 503d221: the `ForeignHost` refusal path closed its flocked file
/// with a bare `drop` — not the explicit unlock every other held-refusal
/// path used. Under a concurrent spawn storm (every fork copies the fd
/// table until exec), a refusal could leave the flock held by a forked
/// duplicate, so an immediate second claimant read `Contention` instead of
/// `ForeignHost`. Fixed by G2 (every post-flock exit unlocks through an
/// RAII guard); this pins zero leaks. The release path is the control.
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

/// AF1: "Stale discovery with a free lock reclaims with a warning."
/// Baseline 503d221 reclaimed a torn or unparseable stale discovery
/// silently. Fixed by G7: it reclaims WITH the typed unreadable warning —
/// except the pid-overflow shape, whose lenient hostname is a known foreign
/// host and refuses under AF5 instead. Nothing reclaims silently.
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

/// AF5 fail-open at baseline 503d221: a stale claim from a KNOWN foreign
/// host that the build could not parse (a newer writer: new `mode`
/// variant, or a field of another type) bypassed the foreign-host refusal
/// and was reclaimed silently. Fixed by G7 (lenient hostname read): it
/// refuses `ForeignHost`.
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

/// Baseline 503d221 failed closed globally: ONE unreadable entry anywhere
/// in the shared recovery dir (here a directory named `*.journal`) refused
/// every project's acquisition with `RecoveryLookup`. Fixed by G6 (only
/// regular files are header-read) and H4 (the name match comes first, so a
/// non-matched entry of another type is skipped before any open): an
/// unrelated entry never blocks this project.
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
    if let Ok(held) = got {
        held.handle.release().unwrap();
    }
    // H4: a non-matched FIFO is skipped before any open — opening it would
    // block the claimant under its flock. Bounded wait: never a hung test.
    #[cfg(unix)]
    {
        let fifo = fx
            .recovery
            .join("zz-unrelated-fifo-0000000000000000.journal");
        assert!(
            Command::new("mkfifo")
                .arg(&fifo)
                .status()
                .unwrap()
                .success()
        );
        let (project, recovery) = (fx.project.clone(), fx.recovery.clone());
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let got = claim(&project, &recovery, "http://b").map(|held| held.handle.release());
            let _ = sender.send(format!("{got:?}"));
        });
        let verdict = receiver.recv_timeout(Duration::from_secs(5));
        assert_eq!(
            verdict.as_deref(),
            Ok("Ok(Ok(()))"),
            "a non-matched FIFO is skipped unopened"
        );
    }
}

/// H4 (race SW3): a NAME-matched journal refuses whatever its type — a
/// symlink (live or dangling), a FIFO (never opened) or a directory — the
/// name is evaluated before the regular-file filter.
#[cfg(unix)]
#[test]
fn h4_name_matched_journal_of_any_type_refuses() {
    for kind in ["symlink", "dangling", "fifo", "dir"] {
        let fx = fixture("race-h4-named");
        let named = fx.recovery.join(journal_file_name(&fx.project));
        let real = fx.dir.path("elsewhere.journal-data");
        match kind {
            "symlink" | "dangling" => {
                if kind == "symlink" {
                    fs::write(&real, b"KINEWRIGHT-JOURNAL 1\n{}\n").unwrap();
                }
                std::os::unix::fs::symlink(&real, &named).unwrap();
            }
            "fifo" => assert!(
                Command::new("mkfifo")
                    .arg(&named)
                    .status()
                    .unwrap()
                    .success()
            ),
            _ => fs::create_dir(&named).unwrap(),
        }
        let got = claim(&fx.project, &fx.recovery, "http://a");
        assert!(
            matches!(&got, Err(LockfileError::PendingRecovery { journal }) if *journal == named),
            "{kind}: owned past a name-matched pending journal: {:?}",
            got.map(|_| ())
        );
    }
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
        crate::recovery::pending_journal_for_project(&fx.recovery, &fx.project).unwrap(),
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

/// H3 (race SW2): an alias journal whose `initial_document` is 64 MiB —
/// written BEFORE `project_path`, so the whole body must be skipped to
/// reach it — is IDENTIFIED (pending), and the streaming parse adds
/// < 16 MiB peak RSS (child-measured `VmHWM`): the body is never buffered.
#[cfg(target_os = "linux")]
#[test]
fn h3_big_alias_document_identifies_in_bounded_memory() {
    let fx = fixture("race-h3-big-document");
    let alias = fx.recovery.join("renamed-0123456789abcdef.journal");
    {
        let mut file = std::io::BufWriter::new(fs::File::create(&alias).unwrap());
        file.write_all(JOURNAL_MAGIC).unwrap();
        file.write_all(br#"{"format_version":1,"initial_document":{"clips":["#)
            .unwrap();
        let clip = format!(r#"{{"id":1,"label":"{}"}}"#, "x".repeat(1024));
        for index in 0..64 * 1024 {
            if index > 0 {
                file.write_all(b",").unwrap();
            }
            file.write_all(clip.as_bytes()).unwrap();
        }
        let path = crate::project::canonical_project_identity(&fx.project).unwrap();
        write!(
            file,
            r#"]}},"project_path":{},"writer_format_version":1}}"#,
            serde_json::to_string(&path).unwrap()
        )
        .unwrap();
        file.write_all(b"\n").unwrap();
    }
    assert!(
        fs::metadata(&alias).unwrap().len() > 64 << 20,
        "a 64 MiB body"
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
    let field = |key: &str| -> u64 {
        let value = measured.split(key).nth(1).unwrap().split_whitespace();
        value.take(1).collect::<String>().parse().unwrap()
    };
    let (before, after) = (field("hwm_before_kib="), field("hwm_after_kib="));
    eprintln!("RACE3: 64 MiB alias document: {measured}");
    assert!(
        measured.contains("verdict=Err(PendingRecovery"),
        "the big alias journal is identified: {measured}"
    );
    assert!(
        after.saturating_sub(before) < 16 * 1024,
        "the 64 MiB body adds < 16 MB peak RSS: {before} -> {after} KiB"
    );
}

// ───────────────────────── G8: dangling alias ─────────────────────────

/// AF2 hole at baseline 503d221: a DANGLING symlink alias (the target not
/// yet saved) did not canonicalise, so its identity was
/// `<link dir>/<link name>` — a different lock from the real target's, two
/// owners of one future file. Fixed by G8 (and H5): the link chain is
/// resolved first, so the alias unifies with its target.
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

// ───────────────────────── G10: journal-writer rule ─────────────────────────

/// AF3 nesting rule, release half: `release` removes the discovery while
/// still holding the flock — a successor may create journals for the
/// identity only after the unlock. A child paused between the removal and
/// the unlock still excludes, ownerless (the discovery is already gone);
/// once it resumes, the release completes and the next claim owns clean.
#[test]
fn guard_release_removes_discovery_before_the_unlock() {
    let fx = fixture("race-release-window");
    let signals = fx.dir.path("owner");
    let mut owner = spawn(
        "hold",
        &fx.project,
        &fx.recovery,
        &signals,
        Opts {
            hook: Some("release_after_remove_before_unlock"),
            ..Opts::default()
        },
    );
    wait_for(&signals.join("owned"));
    let discovery = discovery_path_for_project(Some(&fx.project)).unwrap();
    assert!(discovery.exists(), "the owner published");
    fs::write(signals.join("release"), "").unwrap();
    wait_for(&signals.join("paused"));
    assert!(
        !discovery.exists(),
        "the discovery is gone while the flock is still held"
    );
    let probe = claim(&fx.project, &fx.recovery, "http://probe/mid-release");
    assert!(
        matches!(probe, Err(LockfileError::Contention { owner: None, .. })),
        "mid-release still excludes, ownerless: {probe:?}"
    );
    fs::write(signals.join("resume"), "").unwrap();
    wait_for(&signals.join("exited"));
    assert!(owner.0.wait().unwrap().success(), "clean release exits 0");
    let next = claim(&fx.project, &fx.recovery, "http://probe/after")
        .expect("after the unlock the next claim owns");
    next.handle.release().unwrap();
}

/// AF3 nesting rule, transfer half: a first save at a fresh path (a
/// save-as/second-identity transfer) under a stale handle refuses
/// `LockLost` like any other save — no project, no sidecar, no journal —
/// because a journal for an identity is created only under its lock.
#[cfg(unix)]
#[test]
fn guard_save_as_transfer_refuses_on_a_lost_lock() {
    let fx = fixture("race-lost-transfer");
    let lock = lockfile_path_for_project(Some(&fx.project)).unwrap();
    let a = claim(&fx.project, &fx.recovery, "http://a").expect("A claims");
    fs::remove_file(&lock).unwrap(); // The hand delete.
    let b = claim(&fx.project, &fx.recovery, "http://b").expect("B claims the re-created object");
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
    let fresh = fx.dir.path("save-as.kinewright");
    let saved = save_headless(
        &kinewright_core::Document::default(),
        &fresh,
        None,
        &mut session,
        "",
        kinewright_core::TimelineRevision::default(),
        &fx.recovery,
        Some(&a.handle),
    );
    assert!(
        matches!(saved, Err(ProjectSaveError::LockLost { .. })),
        "a save-as transfer under a stale handle refuses LockLost"
    );
    assert!(!fresh.exists(), "no project lands at the fresh path");
    let sidecar = crate::sidecar_path_for_project(Some(&fresh)).unwrap();
    assert!(!sidecar.exists(), "no sidecar lands at the fresh stem");
    let recovery_entries: Vec<_> = fs::read_dir(&fx.recovery).unwrap().collect();
    assert!(
        recovery_entries.is_empty(),
        "no journal is created without the lock"
    );
    b.handle.release().unwrap();
}

// ───────── Round-2 race review, folded (fix round 3): fixed behaviour ─────────
//
// From rereview2-race-scenarios/aw1_race2_tests.rs (Opus, 2026-09-25). The
// review's `r2_defect_*` were RED at 9487af0; rewritten here to assert the
// fixed behaviour. Hooks: `before_lock_open`, `read_owner_after_meta`,
// `publish_before_rename` (review2-hooks.diff, test-util only).

/// N-a: P's sweep never reaches an in-flight publish temp of a sibling
/// project whose discovery name extends P's
/// (`edit.kinewright.lock.json.x.kinewright`): the sibling publishes.
#[test]
fn h7_sweep_spares_a_sibling_projects_inflight_temp() {
    let fx = fixture("race-h7-sweep-sibling");
    let sibling = fx.dir.path("edit.kinewright.lock.json.x.kinewright");
    fs::write(&sibling, b"{}").unwrap();
    let signals = fx.dir.path("sibling");
    let mut owner = spawn(
        "hold",
        &sibling,
        &fx.recovery,
        &signals,
        Opts {
            hook: Some("publish_before_rename"),
            ..Opts::default()
        },
    );
    wait_for(&signals.join("paused"));
    let p = claim(&fx.project, &fx.recovery, "http://p").expect("P owns");
    fs::write(signals.join("resume"), "").unwrap();
    let sibling_owned = wait_either(&signals.join("owned"), &signals.join("error"));
    let error = fs::read_to_string(signals.join("error")).unwrap_or_default();
    if sibling_owned {
        fs::write(signals.join("release"), "").unwrap();
    }
    owner.0.wait().unwrap();
    p.handle.release().unwrap();
    assert!(
        sibling_owned,
        "P's sweep deleted a sibling's in-flight temp: {error}"
    );
}

/// N-b: a link planted between the lock's symlink check and its open is
/// never followed (`O_NOFOLLOW`): no ownership, and the link's target is
/// never created.
#[cfg(unix)]
#[test]
fn h7_lock_link_planted_after_the_check_creates_nothing() {
    let fx = fixture("race-h7-lock-link");
    let lock = lockfile_path_for_project(Some(&fx.project)).unwrap();
    let target = fx.dir.path("created-through-link.txt");
    let signals = fx.dir.path("owner");
    let mut owner = spawn(
        "hold",
        &fx.project,
        &fx.recovery,
        &signals,
        Opts {
            hook: Some("before_lock_open"),
            ..Opts::default()
        },
    );
    wait_for(&signals.join("paused"));
    std::os::unix::fs::symlink(&target, &lock).unwrap();
    fs::write(signals.join("resume"), "").unwrap();
    let claimed = wait_either(&signals.join("owned"), &signals.join("error"));
    if claimed {
        fs::write(signals.join("release"), "").unwrap();
    }
    owner.0.wait().unwrap();
    assert!(!claimed, "never owns through a link");
    assert!(!target.exists(), "the open followed the late link");
}

/// N-c: the stale discovery is swapped for a FIFO after the pre-check; the
/// claimant (holding the flock) never blocks in `open` — it reads the fd
/// as non-regular, reclaims with the warning, and returns within 2 s.
#[cfg(unix)]
#[test]
fn h7_fifo_swapped_in_after_the_precheck_never_blocks() {
    let fx = fixture("race-h7-fifo-swap");
    let discovery = plant_stale(&fx, "http://stale");
    let signals = fx.dir.path("owner");
    let mut owner = spawn(
        "hold",
        &fx.project,
        &fx.recovery,
        &signals,
        Opts {
            hook: Some("read_owner_after_meta"),
            ..Opts::default()
        },
    );
    wait_for(&signals.join("paused"));
    fs::remove_file(&discovery).unwrap();
    assert!(
        Command::new("mkfifo")
            .arg(&discovery)
            .status()
            .unwrap()
            .success()
    );
    fs::write(signals.join("resume"), "").unwrap();
    let until = Instant::now() + Duration::from_secs(2);
    let mut returned = false;
    while Instant::now() < until {
        if signals.join("owned").exists() || signals.join("error").exists() {
            returned = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    if !returned {
        // Unblock a regressed claimant: open the write end, close — EOF.
        drop(fs::OpenOptions::new().write(true).open(&discovery));
    }
    let claimed = wait_either(&signals.join("owned"), &signals.join("error"));
    if claimed {
        fs::write(signals.join("release"), "").unwrap();
    }
    owner.0.wait().unwrap();
    assert!(returned, "the claimant blocked on a FIFO under its flock");
    assert!(claimed, "the unreadable stale discovery reclaims");
}

/// N-d: two handles of ONE process, same endpoint, same second (the object
/// hand-deleted, then a same-process re-claim — the GUI's one endpoint):
/// the stale handle's release compares `claim_id` and spares the live
/// owner's discovery.
#[cfg(unix)]
#[test]
fn h7_stale_same_endpoint_release_spares_the_successor() {
    let mut spared = None;
    for _ in 0..5 {
        let fx = fixture("race-h7-same-endpoint");
        let lock = lockfile_path_for_project(Some(&fx.project)).unwrap();
        let a = claim(&fx.project, &fx.recovery, "http://gui").unwrap();
        fs::remove_file(&lock).unwrap(); // The hand delete.
        let b = claim(&fx.project, &fx.recovery, "http://gui").unwrap();
        if a.handle.claim.started_at_unix != b.handle.claim.started_at_unix {
            continue; // Crossed a second boundary; retry the same-second case.
        }
        a.handle.release().unwrap();
        spared = Some(b.handle.discovery.exists());
        b.handle.release().unwrap();
        break;
    }
    assert_eq!(
        spared,
        Some(true),
        "the stale handle's release removed the live owner's discovery"
    );
}

/// The publish temps of `discovery` present in its folder (exact AF1 shape).
fn temps_of(discovery: &Path) -> Vec<PathBuf> {
    let name = discovery.file_name().unwrap();
    let mut found: Vec<PathBuf> = fs::read_dir(discovery.parent().unwrap())
        .unwrap()
        .flatten()
        .filter(|entry| is_discovery_temp(&entry.file_name(), name))
        .map(|entry| entry.path())
        .collect();
    found.sort();
    found
}

/// N-a (review guard, pinned against the exact matcher): a publisher dies
/// (SIGKILL or exit) between the synced temp write and the rename. Exactly
/// one temp is left, the prior discovery is untouched, and the next holder
/// sweeps the crashed process's temp and reclaims the RIGHT predecessor.
#[test]
fn h7_crash_between_temp_write_and_rename_is_swept() {
    for round in 0..iterations().min(60) {
        let fx = fixture("h7-crash-rename");
        let stale = round % 2 == 1;
        let before = stale.then(|| fs::read(plant_stale(&fx, "http://stale")).unwrap());
        let discovery = discovery_path_for_project(Some(&fx.project)).unwrap();
        let signals = fx.dir.path("owner");
        let kill = round % 4 < 2;
        let mut owner = spawn(
            "hold",
            &fx.project,
            &fx.recovery,
            &signals,
            Opts {
                hook: Some("publish_before_rename"),
                exit_at_hook: !kill,
                ..Opts::default()
            },
        );
        wait_for(&signals.join("paused"));
        if kill {
            owner.0.kill().unwrap();
        }
        owner.0.wait().unwrap();
        let temps = temps_of(&discovery);
        assert_eq!(temps.len(), 1, "round {round}: one temp left: {temps:?}");
        match &before {
            Some(bytes) => assert_eq!(&fs::read(&discovery).unwrap(), bytes, "round {round}"),
            None => assert!(!discovery.exists(), "round {round}"),
        }
        let next = claim(&fx.project, &fx.recovery, "http://next").expect("the next claim owns");
        assert!(temps_of(&discovery).is_empty(), "round {round}: swept");
        match before {
            Some(_) => assert_eq!(
                next.reclaimed.as_ref().map(|r| r.endpoint.as_str()),
                Some("http://stale"),
                "round {round}: reclaims the pre-crash claim, not the crashed publisher"
            ),
            None => assert!(next.reclaimed.is_none(), "round {round}"),
        }
        next.handle.release().unwrap();
    }
}

/// N-a (review guard): symlinks planted at the next 300 temp names, aimed
/// at a victim, are swept after the flock (removed, never followed) — the
/// victim keeps its bytes and the discovery is a fresh regular file.
#[cfg(unix)]
#[test]
fn h7_planted_temp_links_are_swept_not_followed() {
    let fx = fixture("h7-planted-temp-links");
    let discovery = discovery_path_for_project(Some(&fx.project)).unwrap();
    let victim = fx.dir.path("victim.txt");
    fs::write(&victim, b"precious").unwrap();
    let name = discovery
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let seq = DISCOVERY_TEMP_SEQ.load(Ordering::SeqCst);
    for n in seq..seq + 300 {
        std::os::unix::fs::symlink(
            &victim,
            fx.dir
                .path(&format!(".{name}.{}.{n}.tmp", std::process::id())),
        )
        .unwrap();
    }
    let got = claim(&fx.project, &fx.recovery, "http://a").expect("links swept, fresh temp");
    assert_eq!(fs::read(&victim).unwrap(), b"precious");
    assert!(
        fs::symlink_metadata(&discovery)
            .unwrap()
            .file_type()
            .is_file()
    );
    assert!(temps_of(&discovery).is_empty());
    got.handle.release().unwrap();
}

/// One claim's verdict, as text: a reclaim (with its warning flag and the
/// reclaimed host), a `ForeignHost` refusal, or any other error.
fn verdict_of(fx: &Fx) -> String {
    match claim(&fx.project, &fx.recovery, "http://new") {
        Ok(acquired) => {
            let text = format!(
                "reclaim(unreadable={}, host={:?})",
                acquired.reclaimed_unreadable,
                acquired.reclaimed.as_ref().map(|r| r.hostname.clone())
            );
            acquired.handle.release().unwrap();
            text
        }
        Err(LockfileError::ForeignHost { host, .. }) => format!("ForeignHost({host:?})"),
        Err(error) => format!("{error:?}"),
    }
}

fn stale_with(tag: &str, bytes: &[u8]) -> Fx {
    let fx = fixture(tag);
    let discovery = plant_stale(&fx, "http://stale");
    fs::write(&discovery, bytes).unwrap();
    fx
}

/// N-g + G7 (review guard, tightened): hostile unreadable claims inside the
/// lenient bound — deep nesting, type-confused hostnames, duplicate keys,
/// blank or `UNKNOWN.` hosts — each get a typed verdict, never a silent
/// reclaim; an empty or `UNKNOWN.` hostname is the unknown host, so it
/// reclaims with the warning rather than refusing as a foreign host "".
#[test]
fn h7_lenient_parse_hostile_shapes_are_typed() {
    let own = current_hostname();
    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("deep-open-60k", "[".repeat(60_000).into_bytes()),
        ("deep-object-60k", r#"{"a":"#.repeat(12_000).into_bytes()),
        (
            "flat-array-60k",
            format!("[{}0]", "0,".repeat(30_000)).into_bytes(),
        ),
        ("host-number", br#"{"hostname":5}"#.to_vec()),
        (
            "host-array",
            br#"{"hostname":["other-machine.invalid"]}"#.to_vec(),
        ),
        (
            "host-object",
            br#"{"hostname":{"name":"other-machine.invalid"}}"#.to_vec(),
        ),
        ("host-null", br#"{"hostname":null}"#.to_vec()),
        ("host-UNKNOWN.", br#"{"hostname":"UNKNOWN."}"#.to_vec()),
        ("host-empty", br#"{"hostname":""}"#.to_vec()),
        (
            "host-own-upper-lenient",
            format!(r#"{{"hostname":"{}"}}"#, own.to_uppercase()).into_bytes(),
        ),
        (
            "dup-own-then-foreign",
            format!(r#"{{"hostname":"{own}","hostname":"other-machine.invalid"}}"#).into_bytes(),
        ),
        (
            "dup-foreign-then-own",
            format!(r#"{{"hostname":"other-machine.invalid","hostname":"{own}"}}"#).into_bytes(),
        ),
        (
            "utf8-bom",
            b"\xef\xbb\xbf{\"hostname\":\"other-machine.invalid\"}".to_vec(),
        ),
    ];
    for (name, bytes) in cases {
        let fx = stale_with("h7-lenient", &bytes);
        let verdict = verdict_of(&fx);
        eprintln!("RACE3: lenient {name}: {verdict}");
        assert!(
            verdict.starts_with("reclaim(unreadable=true") || verdict.starts_with("ForeignHost("),
            "{name}: typed, never silent: {verdict}"
        );
        if matches!(name, "host-empty" | "host-UNKNOWN.") {
            assert!(
                verdict.starts_with("reclaim(unreadable=true"),
                "{name}: the unknown host reclaims with the warning: {verdict}"
            );
        }
    }
}

/// N-e (documented in AF5, not a defect): a known foreign claim past the
/// lenient bounds (> 64 KiB, or nesting deeper than 128) is unreadable and
/// reclaims — always WITH the unreadable warning, never silently.
#[test]
fn h7_foreign_claim_past_the_lenient_bounds_reclaims_with_the_warning() {
    let deep = format!(
        r#"{{"hostname":"other-machine.invalid","future":{}1{}}}"#,
        "[".repeat(200),
        "]".repeat(200)
    );
    let big = format!(
        r#"{{"hostname":"other-machine.invalid","future":"{}"}}"#,
        "x".repeat(70_000)
    );
    for (name, bytes) in [("deep-200", deep), ("big-70k", big)] {
        let fx = stale_with("h7-lenient-bounds", bytes.as_bytes());
        let verdict = verdict_of(&fx);
        eprintln!("RACE3: foreign past lenient bounds {name}: {verdict}");
        assert!(
            verdict.starts_with("reclaim(unreadable=true"),
            "{name}: the AF5-documented limit reclaims with the warning: {verdict}"
        );
    }
}

/// H5 (review probe, now asserted): a relative chain through a symlinked
/// directory with `..` (which the kernel resolves physically) unifies with
/// its target, and writing through it reaches exactly that target.
#[cfg(unix)]
#[test]
fn h5_relative_chain_through_a_symlinked_dir_matches_the_kernel() {
    use std::os::unix::fs::symlink;
    let dir = TempDirectory::new("h5-relative-chain");
    fs::create_dir_all(dir.path("x/y")).unwrap();
    fs::create_dir_all(dir.path("x/real")).unwrap();
    symlink(dir.path("x/y"), dir.path("sub")).unwrap();
    symlink(
        "../real/target.kinewright",
        dir.path("x/y/link2.kinewright"),
    )
    .unwrap();
    symlink("sub/link2.kinewright", dir.path("link1.kinewright")).unwrap();
    let head = dir.path("link1.kinewright");
    let target = dir.path("x/real/target.kinewright");
    let identity = |path: &Path| crate::project::canonical_project_identity(path);
    assert_eq!(
        identity(&head),
        identity(&target),
        "the relative chain resolves like the kernel"
    );
    let recovery = dir.path("recovery");
    fs::create_dir(&recovery).unwrap();
    let real = claim(&target, &recovery, "http://real").expect("the target spelling owns");
    let alias = claim(&head, &recovery, "http://alias");
    assert!(
        matches!(alias, Err(LockfileError::Contention { .. })),
        "the relative alias contends with its target: {:?}",
        alias.map(|_| ())
    );
    real.handle.release().unwrap();
    fs::write(&head, b"{}").unwrap();
    assert!(
        target.exists(),
        "the kernel resolved the chain to the same file"
    );
}

/// H5 (review probe, now asserted): the identity bound agrees with the
/// kernel's (Linux `MAXSYMLINKS` = 40). A 40-link dangling chain unifies and
/// the kernel writes through it to the target; a 41-link chain refuses
/// typed and the kernel refuses the write too — no chain the kernel follows
/// gets a second identity.
#[cfg(target_os = "linux")]
#[test]
fn h5_link_bound_agrees_with_the_kernel() {
    let dir = TempDirectory::new("h5-kernel-bound");
    let root = fs::canonicalize(dir.root()).unwrap();
    let recovery = root.join("recovery");
    fs::create_dir(&recovery).unwrap();
    for hops in [40_usize, 41] {
        let base = root.join(format!("chain{hops}"));
        fs::create_dir(&base).unwrap();
        for index in 0..hops {
            let to = if index + 1 == hops {
                "end.kinewright".to_owned()
            } else {
                format!("l{}.kinewright", index + 1)
            };
            std::os::unix::fs::symlink(to, base.join(format!("l{index}.kinewright"))).unwrap();
        }
        let head = base.join("l0.kinewright");
        let end = base.join("end.kinewright");
        let identity = crate::project::canonical_project_identity(&head);
        let alias = claim(&head, &recovery, "http://head");
        let alias_verdict = format!("{:?}", alias.as_ref().map(|_| ()));
        drop(alias);
        let kernel_follows = fs::write(&head, b"{}").is_ok() && end.exists();
        eprintln!(
            "RACE3: {hops}-link chain: identity={identity:?} alias={alias_verdict} kernel_follows={kernel_follows}"
        );
        if hops == 40 {
            assert!(kernel_follows, "the kernel follows 40 links");
            assert_eq!(
                identity.as_deref().ok(),
                Some(end.as_path()),
                "40 links unify"
            );
        } else {
            assert!(!kernel_follows, "the kernel refuses 41 links");
            assert!(identity.is_err(), "41 links refuse typed");
            assert!(
                alias_verdict.contains("Identity("),
                "the acquire refuses typed: {alias_verdict}"
            );
        }
    }
}

// ───────── Round-3 race review, folded (fix round 4): fixed behaviour ─────────
//
// From rereview3-race-scenarios/aw1_race3_tests.rs (Opus, 2026-09-25),
// rewritten to assert the fixed behaviour.

/// J2 (race S1, split identity): the claimant pauses holding the flock,
/// before its scan, and the alias it claimed through is re-pointed from T
/// to U. The scan uses the identity the acquire resolved once (T), so T's
/// name-matched pending journal refuses — at 9a14698 the scan re-resolved
/// to U and the claimant owned T's lock past T's journal.
#[cfg(unix)]
#[test]
fn j2_alias_repointed_before_the_scan_still_refuses_its_journal() {
    let fx = fixture("j2-split-identity");
    let pending = plant_journal(&fx, "base");
    let other = fx.dir.path("other.kinewright");
    fs::write(&other, b"{}").unwrap();
    let alias = fx.dir.path("alias.kinewright");
    std::os::unix::fs::symlink(&fx.project, &alias).unwrap();
    let signals = fx.dir.path("owner");
    let mut owner = spawn(
        "hold",
        &alias,
        &fx.recovery,
        &signals,
        Opts {
            hook: Some("after_flock_before_scan"),
            ..Opts::default()
        },
    );
    wait_for(&signals.join("paused"));
    let staged = fx.dir.path("staged");
    std::os::unix::fs::symlink(&other, &staged).unwrap();
    fs::rename(&staged, &alias).unwrap();
    fs::write(signals.join("resume"), "").unwrap();
    let took = wait_either(&signals.join("owned"), &signals.join("error"));
    let error = fs::read_to_string(signals.join("error")).unwrap_or_default();
    if took {
        fs::write(signals.join("release"), "").unwrap();
    }
    owner.0.wait().unwrap();
    assert!(
        !took && error.starts_with("PendingRecovery"),
        "T's pending journal refuses the claim made as T: {error}"
    );
    assert!(pending.exists(), "the pending journal is untouched");
}

/// J2 (race S1, raw fallback): the alias becomes a 41-link chain after
/// the acquire resolved it. Nothing re-resolves, so the claimant owns
/// T's lock (a probe of T contends) and no raw-path
/// `alias.kinewright.kinewright.lock` appears — at 9a14698 the scan's
/// re-resolution failed (`RecoveryLookup`) and a later resolution could
/// fall back to the raw-path lock object.
#[cfg(unix)]
#[test]
fn j2_chain_grown_past_the_bound_after_resolution_changes_nothing() {
    let fx = fixture("j2-grown-chain");
    let alias = fx.dir.path("alias.kinewright");
    std::os::unix::fs::symlink(&fx.project, &alias).unwrap();
    let signals = fx.dir.path("owner");
    let mut owner = spawn(
        "hold",
        &alias,
        &fx.recovery,
        &signals,
        Opts {
            hook: Some("before_lock_open"),
            ..Opts::default()
        },
    );
    wait_for(&signals.join("paused"));
    let chain = fx.dir.path("chain");
    fs::create_dir(&chain).unwrap();
    for index in 0..41 {
        let to = if index == 40 {
            fx.project.clone()
        } else {
            chain.join(format!("l{}", index + 1))
        };
        std::os::unix::fs::symlink(to, chain.join(format!("l{index}"))).unwrap();
    }
    let staged = fx.dir.path("staged");
    std::os::unix::fs::symlink(chain.join("l0"), &staged).unwrap();
    fs::rename(&staged, &alias).unwrap();
    fs::write(signals.join("resume"), "").unwrap();
    let took = wait_either(&signals.join("owned"), &signals.join("error"));
    let error = fs::read_to_string(signals.join("error")).unwrap_or_default();
    let probe = claim(&fx.project, &fx.recovery, "http://probe-t");
    let probe_verdict = format!("{:?}", probe.as_ref().map(|_| ()));
    drop(probe);
    if took {
        fs::write(signals.join("release"), "").unwrap();
    }
    owner.0.wait().unwrap();
    assert!(took, "the claim made as T owns T: {error}");
    assert!(
        probe_verdict.contains("Contention"),
        "T's lock is the one held: {probe_verdict}"
    );
    assert!(
        !fx.dir.path("alias.kinewright.kinewright.lock").exists(),
        "no raw-path lock object"
    );
}
