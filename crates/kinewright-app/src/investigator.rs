//! The IN2 investigator: refused-edit sessions inside a visible budget.
//!
//! This module owns everything §14 row C assigns to the app crate: the
//! settings file (§2.1), harness discovery refresh (§2.2 rule 7), the
//! one-strike disable (§2.2 rule 8), the per-project session queue (§3.1–§3.2),
//! the opening message (§3.4), the one session thread and its observer (§3.5,
//! D-R64), the end table (§3.7 rule 38), the proposal card (§4.3), and the
//! approve / reject / re-investigate / mute actions (§4.4, §2.4).
//!
//! Session state lives on [`InvestigatorSession`], one per [`ProjectSession`]
//! (`crate::project`), never on the app struct. The only app-wide state here
//! is behind module statics: the one-strike set, the detection-refresh
//! channel, the settings-write failure flag, and the settings-window edge.
//!
//! [`ProjectSession`]: crate::project::ProjectSession

use std::collections::{BTreeMap, HashSet, VecDeque};
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Mutex, OnceLock,
    atomic::{AtomicBool, AtomicU32, Ordering},
};
use std::thread::JoinHandle;
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender, TryRecvError, unbounded};
use kinewright_agent::{
    BranchApplyOutcome, BudgetKind, ConfirmationBroker, ConfirmationPolicy,
    INVESTIGATOR_CAPABILITY_DENYLIST, INVESTIGATOR_CONFIRMATION_REFUSAL, INVESTIGATOR_TOOL_NAMES,
    INVESTIGATOR_WORKER_THREADS, IncidentLogHandle, InvestigatorSessionContext, McpServer,
    ScriptedDriver, SessionCost, SessionCounters, SessionFlow, SessionLimits, SessionObserver,
    SessionStop, SharedCounters, StopReason, TimelineBranch, apply_to_live, harness_driver,
    mirror_agent_cost, operation_tool_name,
};
use kinewright_core::{
    AgentDriver, AgentEvent, AuthenticationStatus, Command, Event, INVESTIGATOR_ALLOWLIST,
    Incident, IncidentCode, IncidentId, IncidentObservation, IncidentOutcome, IncidentProposal,
    IncidentResolver, IncidentState, IncidentSubject, LabelIncident, Operation, Query, QueryResult,
    RunningInvestigation, SessionConfig, TimelineRevision,
};
use serde::{Deserialize, Serialize};

use crate::app::KinewrightApp;
use crate::chat_ui::{AgentHarnessChoice, HarnessUpdate, detect_harness};

// ---------------------------------------------------------------------------
// Settings file (IN2 §2.1)
// ---------------------------------------------------------------------------

/// The only settings-file version this reader understands.
pub(crate) const INVESTIGATOR_SETTINGS_VERSION: u32 = 1;

/// At most this many investigator sessions run at once, across every project
/// (IN2 §3.2 rule 12).
pub(crate) const INVESTIGATOR_MAX_CONCURRENT_SESSIONS: usize = 2;

/// The three session budgets (IN2 §2.1 rule 2, §5.3 rule 10).
#[allow(
    clippy::struct_field_names,
    reason = "the persisted wire names are normative"
)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct InvestigatorBudgets {
    /// Turn budget: the driver's `max_turns` and the pump's turn arm (D-R68).
    #[serde(default = "default_max_turns")]
    pub(crate) max_turns: u32,
    /// Wall-clock budget in seconds.
    #[serde(default = "default_max_wall_time_seconds")]
    pub(crate) max_wall_time_seconds: u64,
    /// Token budget. The pump's token arm only fires on harness-reported
    /// costs, so a silent harness runs on turns and wall time (§5.3 rule 9).
    #[serde(default = "default_max_tokens")]
    pub(crate) max_tokens: u64,
}

fn default_max_turns() -> u32 {
    6
}

fn default_max_wall_time_seconds() -> u64 {
    90
}

fn default_max_tokens() -> u64 {
    40_000
}

impl Default for InvestigatorBudgets {
    fn default() -> Self {
        Self {
            max_turns: default_max_turns(),
            max_wall_time_seconds: default_max_wall_time_seconds(),
            max_tokens: default_max_tokens(),
        }
    }
}

impl InvestigatorBudgets {
    /// Clamp every budget into its §2.1 rule 2 range, in place.
    pub(crate) fn clamp(&mut self) {
        self.max_turns = self.max_turns.clamp(1, 64);
        self.max_wall_time_seconds = self.max_wall_time_seconds.clamp(5, 900);
        self.max_tokens = self.max_tokens.clamp(1_000, 2_000_000);
    }
}

/// The investigator settings file (§2.1 rule 2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct InvestigatorSettings {
    /// File version. Missing reads as 0, which is unknown (§2.1 rule 3).
    #[serde(default)]
    pub(crate) version: u32,
    /// The off switch (§2.3 rule 9). Default off.
    #[serde(default)]
    pub(crate) enabled: bool,
    /// Chosen harness key, or `None` for the first available harness.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) harness: Option<String>,
    /// Model override, or `None` for the CLI's default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) model: Option<String>,
    /// Effort override, or `None` for the CLI's default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) effort: Option<String>,
    /// Service-tier override, or `None` for the provider's standard tier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) service_tier: Option<String>,
    #[serde(default)]
    pub(crate) budgets: InvestigatorBudgets,
}

impl Default for InvestigatorSettings {
    fn default() -> Self {
        Self {
            version: INVESTIGATOR_SETTINGS_VERSION,
            enabled: false,
            harness: None,
            model: None,
            effort: None,
            service_tier: None,
            budgets: InvestigatorBudgets::default(),
        }
    }
}

impl InvestigatorSettings {
    /// Clamp the budgets in place (§2.1 rule 2).
    pub(crate) fn clamp(&mut self) {
        self.budgets.clamp();
    }
}

/// Which platform's settings path to resolve (T10's three rows).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InvestigatorConfigPlatform {
    Windows,
    Macos,
    Unix,
}

/// Resolve the settings path from explicit environment values.
///
/// Pure over its inputs so the item-39 test pins all three of T10's rows on
/// any host: `appdata` is `%APPDATA%`, `xdg_config_home` is
/// `$XDG_CONFIG_HOME`, `home` is `$HOME`. Callers pass `None` for an unset (or
/// empty) variable. Returns the path and whether it is durable — the
/// `temp_dir` fallback is not, and the settings card says so (§2.1 rule 1).
#[must_use]
pub(crate) fn resolve_investigator_config_path(
    platform: InvestigatorConfigPlatform,
    appdata: Option<&Path>,
    xdg_config_home: Option<&Path>,
    home: Option<&Path>,
) -> (PathBuf, bool) {
    let fallback = || {
        (
            std::env::temp_dir()
                .join("Kinewright")
                .join("investigator.json"),
            false,
        )
    };
    match platform {
        InvestigatorConfigPlatform::Windows => appdata.map_or_else(fallback, |dir| {
            (dir.join("Kinewright").join("investigator.json"), true)
        }),
        InvestigatorConfigPlatform::Macos => home.map_or_else(fallback, |dir| {
            (
                dir.join("Library")
                    .join("Application Support")
                    .join("Kinewright")
                    .join("investigator.json"),
                true,
            )
        }),
        InvestigatorConfigPlatform::Unix => {
            if let Some(dir) = xdg_config_home {
                (dir.join("Kinewright").join("investigator.json"), true)
            } else if let Some(dir) = home {
                (
                    dir.join(".config")
                        .join("Kinewright")
                        .join("investigator.json"),
                    true,
                )
            } else {
                fallback()
            }
        }
    }
}

fn env_path(name: &str) -> Option<PathBuf> {
    let value = std::env::var_os(name)?;
    if value.is_empty() {
        return None;
    }
    Some(PathBuf::from(value))
}

/// The production settings path: this host's platform with the real
/// environment (§2.1 rule 1).
#[must_use]
pub(crate) fn investigator_config_path() -> (PathBuf, bool) {
    let platform = if cfg!(windows) {
        InvestigatorConfigPlatform::Windows
    } else if cfg!(target_os = "macos") {
        InvestigatorConfigPlatform::Macos
    } else {
        InvestigatorConfigPlatform::Unix
    };
    let appdata = env_path("APPDATA");
    let xdg_config_home = env_path("XDG_CONFIG_HOME");
    let home = env_path("HOME");
    resolve_investigator_config_path(
        platform,
        appdata.as_deref(),
        xdg_config_home.as_deref(),
        home.as_deref(),
    )
}

/// Resolve the working directory passed to a harness session (§3.3 rule 18).
/// A saved project runs from its containing directory; an untitled project
/// follows the app's current directory.
#[must_use]
pub(crate) fn investigator_working_directory(project_path: Option<&Path>) -> Option<PathBuf> {
    project_path
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .or_else(|| std::env::current_dir().ok())
}

/// Why a settings load fell back to the default (§2.1 rule 3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SettingsLoadProblem {
    /// The file exists but cannot be read.
    Unreadable(String),
    /// The file is not the settings shape.
    Unparseable(String),
    /// The file carries a version this reader does not know.
    UnknownVersion(u32),
}

/// A settings load: the settings to use, and the problem to report, if any.
///
/// A missing file is silent and yields the default; every other failure
/// yields the default plus one problem, and the caller opens one
/// `agent_unclassified` incident for it (§2.1 rule 3).
pub(crate) struct SettingsLoad {
    pub(crate) settings: InvestigatorSettings,
    pub(crate) problem: Option<SettingsLoadProblem>,
}

/// Read settings only when the resolved path is durable. A temporary fallback
/// is deliberately an in-memory-only run (§2.1 rule 1, review fix F5).
pub(crate) fn load_investigator_settings_for_run(path: &Path, durable: bool) -> SettingsLoad {
    if durable {
        load_investigator_settings_from(path)
    } else {
        SettingsLoad {
            settings: InvestigatorSettings::default(),
            problem: None,
        }
    }
}

/// Install one startup load on the app, reporting a malformed, unreadable or
/// unknown-version file exactly once through the ordinary incident router.
/// The caller deliberately does not write the file back: the original bytes
/// stay intact until a person changes a setting (§2.1 rule 3).
#[cfg(test)]
pub(crate) fn apply_settings_load(app: &mut KinewrightApp, load: SettingsLoad) {
    apply_settings_load_for_run(app, load, true);
}

/// Install settings and remember whether this process may persist them. A
/// temporary resolver fallback is a deliberately non-durable run: it starts
/// from defaults and never reads or writes the fallback file (§2.1 rule 1,
/// review fix F5).
pub(crate) fn apply_settings_load_for_run(
    app: &mut KinewrightApp,
    load: SettingsLoad,
    durable: bool,
) {
    if let Some(session) = app.projects[0].investigator.as_mut() {
        session.settings = load.settings;
        session.settings_durable = durable;
    }
    if let Some(problem) = load.problem {
        app.note_label(
            LabelIncident::Agent,
            IncidentSubject::Agent,
            problem.message(),
        );
    }
}

impl SettingsLoadProblem {
    /// The incident body for this load failure (§2.1 rule 3).
    pub(crate) fn message(&self) -> String {
        match self {
            Self::Unreadable(reason) => {
                format!("the investigator settings file could not be read: {reason}")
            }
            Self::Unparseable(reason) => {
                format!("the investigator settings file could not be parsed: {reason}")
            }
            Self::UnknownVersion(version) => {
                format!("the investigator settings file has unknown version {version}")
            }
        }
    }
}

/// Load the settings file at `path` (§2.1 rule 3).
pub(crate) fn load_investigator_settings_from(path: &Path) -> SettingsLoad {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return SettingsLoad {
                settings: InvestigatorSettings::default(),
                problem: None,
            };
        }
        Err(error) => {
            return SettingsLoad {
                settings: InvestigatorSettings::default(),
                problem: Some(SettingsLoadProblem::Unreadable(error.to_string())),
            };
        }
    };
    let mut settings: InvestigatorSettings = match serde_json::from_slice(&bytes) {
        Ok(settings) => settings,
        Err(error) => {
            return SettingsLoad {
                settings: InvestigatorSettings::default(),
                problem: Some(SettingsLoadProblem::Unparseable(error.to_string())),
            };
        }
    };
    if settings.version != INVESTIGATOR_SETTINGS_VERSION {
        return SettingsLoad {
            settings: InvestigatorSettings::default(),
            problem: Some(SettingsLoadProblem::UnknownVersion(settings.version)),
        };
    }
    settings.clamp();
    SettingsLoad {
        settings,
        problem: None,
    }
}

/// Write the settings file atomically: serialise to `<path>.tmp` in the same
/// directory, fsync, rename over the path (§2.1 rule 4).
///
/// # Errors
///
/// Returns the I/O error when the directory, the temporary file, the fsync,
/// or the rename fails; the caller reports the first failure per session.
pub(crate) fn save_investigator_settings_to(
    path: &Path,
    settings: &InvestigatorSettings,
) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut settings = settings.clone();
    settings.version = INVESTIGATOR_SETTINGS_VERSION;
    let json = serde_json::to_string_pretty(&settings).map_err(std::io::Error::other)?;
    let tmp_path = path.with_extension("json.tmp");
    {
        let mut file = fs::File::create(&tmp_path)?;
        file.write_all(json.as_bytes())?;
        file.sync_all()?;
    }
    fs::rename(&tmp_path, path)?;
    Ok(())
}

/// Persist settings for a run only when its path is durable. The fallback path
/// is intentionally a no-op, so an old temporary file cannot re-enable the
/// investigator on a later run (review fix F5).
pub(crate) fn save_investigator_settings_for_run(
    path: &Path,
    durable: bool,
    settings: &InvestigatorSettings,
) -> std::io::Result<()> {
    if durable {
        save_investigator_settings_to(path, settings)
    } else {
        Ok(())
    }
}

static SETTINGS_WRITE_FAILURE_REPORTED: AtomicBool = AtomicBool::new(false);

fn claim_first_report(flag: &AtomicBool) -> bool {
    !flag.swap(true, Ordering::SeqCst)
}

/// Whether a failed settings write should open its incident: true once per
/// app session (§2.1 rule 4).
pub(crate) fn should_report_settings_write_failure() -> bool {
    claim_first_report(&SETTINGS_WRITE_FAILURE_REPORTED)
}

// ---------------------------------------------------------------------------
// One-strike disable (IN2 §2.2 rule 8)
// ---------------------------------------------------------------------------

static DISABLED_HARNESSES: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

fn disabled_harnesses() -> &'static Mutex<HashSet<String>> {
    DISABLED_HARNESSES.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Whether `key` was disabled by a one-strike failure earlier this session.
pub(crate) fn harness_disabled_by_one_strike(key: &str) -> bool {
    disabled_harnesses()
        .lock()
        .is_ok_and(|set| set.contains(key))
}

/// Disable `key` for the rest of the app session (§2.2 rule 8).
pub(crate) fn disable_harness_for_session(key: &str) {
    if let Ok(mut set) = disabled_harnesses().lock() {
        set.insert(key.to_owned());
    }
}

// ---------------------------------------------------------------------------
// Discovery refresh (IN2 §2.2 rule 7)
// ---------------------------------------------------------------------------

static REFRESH_CHANNEL: OnceLock<(Sender<HarnessUpdate>, Receiver<HarnessUpdate>)> =
    OnceLock::new();

fn refresh_channel() -> &'static (Sender<HarnessUpdate>, Receiver<HarnessUpdate>) {
    REFRESH_CHANNEL.get_or_init(unbounded)
}

static REFRESH_RUNNING: AtomicBool = AtomicBool::new(false);

/// Re-run harness detection on a background thread, delivering through the
/// same update shape the startup probe uses. At most one re-probe runs at a
/// time; detection-only, never catalogs, never on the frame.
pub(crate) fn refresh_harness_detection() {
    if REFRESH_RUNNING.swap(true, Ordering::SeqCst) {
        return;
    }
    let updates = refresh_channel().0.clone();
    std::thread::spawn(move || {
        for choice in AgentHarnessChoice::ALL {
            let detected = HarnessUpdate::Detected(choice, Box::new(detect_harness(choice)));
            if updates.send(detected).is_err() {
                break;
            }
        }
        REFRESH_RUNNING.store(false, Ordering::SeqCst);
    });
}

/// Drain detection-refresh deliveries (§2.2 rule 7).
pub(crate) fn drain_detection_refresh() -> Vec<HarnessUpdate> {
    refresh_channel().1.try_iter().collect()
}

static SETTINGS_WAS_OPEN: AtomicBool = AtomicBool::new(false);

/// Note the settings window's open state; on the closed-to-open edge, re-run
/// detection so a harness installed after launch appears (§2.2 rule 7).
pub(crate) fn note_settings_window(open: bool) {
    if open {
        if !SETTINGS_WAS_OPEN.swap(true, Ordering::SeqCst) {
            refresh_harness_detection();
        }
    } else {
        SETTINGS_WAS_OPEN.store(false, Ordering::SeqCst);
    }
}

// ---------------------------------------------------------------------------
// Session state (IN2 §3.1–§3.2)
// ---------------------------------------------------------------------------

/// One incident waiting for an investigator session (IN2 §3.2 rule 9).
#[derive(Debug, Clone)]
pub(crate) struct QueuedIncident {
    pub(crate) id: IncidentId,
    pub(crate) code: IncidentCode,
    pub(crate) subject: IncidentSubject,
    /// The refused operation the drain already held (§3.4 rule 22).
    pub(crate) refused: Option<Operation>,
}

/// The session-owned pair holders needed by the proposal-card callers.
///
/// Re-investigation deduplicates on `(code, subject)`, not on the incident id:
/// two observations may therefore produce two incidents while only one pair
/// may be running or queued.  The UI receives this snapshot instead of
/// guessing from the one running card's id (review fix H1).
#[derive(Debug, Clone, Default)]
pub(crate) struct InvestigatorSessionPairs {
    running: Option<(IncidentId, IncidentCode, IncidentSubject)>,
    queued: Vec<(IncidentId, IncidentCode, IncidentSubject)>,
}

impl InvestigatorSessionPairs {
    /// Whether a card for this incident may honestly show Re-investigate.
    ///
    /// A running pair always blocks the press, including the same incident.
    /// A queued pair blocks a different incident, while pressing the same
    /// already-queued incident is the successful G7 no-op.
    #[must_use]
    pub(crate) fn can_reinvestigate(
        &self,
        id: IncidentId,
        code: IncidentCode,
        subject: IncidentSubject,
    ) -> bool {
        if self
            .running
            .is_some_and(|(_, running_code, running_subject)| {
                running_code == code && running_subject == subject
            })
        {
            return false;
        }
        !self
            .queued
            .iter()
            .any(|(queued_id, queued_code, queued_subject)| {
                *queued_code == code && *queued_subject == subject && *queued_id != id
            })
    }
}

/// A refused operation waiting for its observation to route.
///
/// The `OpRejected` drain and the branch-apply arms run before
/// `route_incidents` observes anything, so the incident id does not exist
/// yet; the full observation is the key that reunites the two.
#[derive(Debug, Clone)]
pub(crate) struct PendingRefused {
    pub(crate) observation: IncidentObservation,
    pub(crate) op: Operation,
}

/// What the session thread reports back (§3.2 rule 8).
///
/// The contract names the channel payload `(SessionStop, SessionCounters)`;
///
/// the accumulated cost crosses on the same channel because the observer that
/// owns it lives on the session thread and D-R64 mirrors it once, at the end.
#[derive(Debug, Clone)]
pub(crate) struct InvestigatorSessionResult {
    pub(crate) stop: SessionStop,
    pub(crate) counters: SessionCounters,
    pub(crate) cost: SessionCost,
}

enum InvestigatorFinishOutcome {
    Done(InvestigatorSessionResult),
    Died,
}

/// One running investigator session (§3.2 rule 8).
pub(crate) struct RunningSession {
    pub(crate) id: IncidentId,
    pub(crate) code: IncidentCode,
    pub(crate) subject: IncidentSubject,
    /// Harness key the session runs on (settings or test key).
    pub(crate) harness: String,
    pub(crate) model: Option<String>,
    /// The session's branch (§3.2 rule 8). Never read: the server holds its
    /// own core clone, and this handle's job is ownership — the branch lives
    /// exactly as long as the session does.
    #[allow(dead_code)]
    pub(crate) branch: TimelineBranch,
    pub(crate) server: Option<McpServer>,
    /// The app's broker handle: cancel answers through it (§3.7 rule 37).
    pub(crate) broker: ConfirmationBroker,
    pub(crate) thread: Option<JoinHandle<()>>,
    pub(crate) counters: Arc<SharedCounters>,
    /// How many cost events the observer has seen; the card's tokens row
    /// reads "unknown" until the first one (§5.3 rule 9, §5.5 rule 14).
    pub(crate) cost_events: Arc<AtomicU32>,
    pub(crate) cancel: Arc<AtomicBool>,
    pub(crate) result: Receiver<InvestigatorSessionResult>,
    pub(crate) budgets: InvestigatorBudgets,
}

/// The per-project investigator state, held as
/// `ProjectSession::investigator` (IN2 §3.2 rule 8).
#[derive(Default)]
pub(crate) struct InvestigatorSession {
    /// This project's settings copy. Production keeps every copy in sync:
    /// the file is read once at startup and every settings change writes all
    /// copies plus the file. Tests set their own copy directly, so no global
    /// settings store exists to leak between tests.
    pub(crate) settings: InvestigatorSettings,
    /// Whether this run may read and write the settings file.
    pub(crate) settings_durable: bool,
    queue: VecDeque<QueuedIncident>,
    running: Option<RunningSession>,
    pending_refused: Vec<PendingRefused>,
    /// The live per-project mutes (§2.4). The `Document.investigator` snapshot
    /// is the opened file's fossil — core never changes it, and a core event
    /// would clobber a snapshot edit — so the session copy is authoritative
    /// for the enqueue predicate and is injected into the saved bytes.
    muted_codes: Vec<String>,
    /// The mutes as of the last save, for the dirty check.
    saved_muted_codes: Vec<String>,
    /// Test injection: when present, the session starts on this driver and
    /// skips harness detection. Always `None` in production.
    pub(crate) test_driver: Option<ScriptedDriver>,
    /// Test-only gate that makes the driver's start path stay blocked while
    /// the frame thread exercises cancellation. Production never sets it.
    #[cfg(test)]
    pub(crate) test_start_delay: Option<Duration>,
    /// Test-only hook called after the first approval conflict is re-read and
    /// immediately before the retry.
    #[cfg(test)]
    pub(crate) approval_retry_hook: Option<Box<dyn FnMut(u32)>>,
}

impl InvestigatorSession {
    /// A session whose mutes start from the opened document's (§2.4).
    pub(crate) fn with_muted_codes(muted_codes: Vec<String>) -> Self {
        Self {
            settings_durable: true,
            saved_muted_codes: muted_codes.clone(),
            muted_codes,
            ..Self::default()
        }
    }

    /// The live mutes.
    pub(crate) fn muted_codes(&self) -> &[String] {
        &self.muted_codes
    }

    /// Mute `code`, silently keeping one copy (§2.4 rule 14).
    pub(crate) fn mute_code(&mut self, code: &str) {
        if !self.muted_codes.iter().any(|muted| muted == code) {
            self.muted_codes.push(code.to_owned());
        }
    }

    /// Remove one mute. Returns whether anything changed.
    pub(crate) fn unmute_code(&mut self, code: &str) -> bool {
        let before = self.muted_codes.len();
        self.muted_codes.retain(|muted| muted != code);
        self.muted_codes.len() != before
    }

    /// Whether the mutes changed since the last save.
    pub(crate) fn mutes_dirty(&self) -> bool {
        self.muted_codes != self.saved_muted_codes
    }

    /// Note that the current mutes were saved.
    pub(crate) fn note_mutes_saved(&mut self) {
        self.saved_muted_codes.clone_from(&self.muted_codes);
    }

    /// Whether `code` is muted on this project (§3.1 rule 4, §2.4 rule 14).
    pub(crate) fn is_muted(&self, code: IncidentCode) -> bool {
        let code = code.code();
        self.muted_codes.iter().any(|muted| muted == code)
    }

    /// Stash a refused operation for the observation it was refused with.
    pub(crate) fn stash_refused(&mut self, observation: IncidentObservation, op: Operation) {
        const MAX_PENDING_REFUSED: usize = 8;
        if self.pending_refused.len() >= MAX_PENDING_REFUSED {
            self.pending_refused.remove(0);
        }
        self.pending_refused
            .push(PendingRefused { observation, op });
    }

    /// Take the refused operation stashed for `observation`, if any.
    ///
    /// Keyed on `(code, subject, observed)` only (`IN2B` §3 rule 13, N2/S-3):
    /// `name` and `transient` are ignored, so a renamed re-observation still
    /// reunites with the stash its unnamed twin left.
    pub(crate) fn take_refused(&mut self, observation: &IncidentObservation) -> Option<Operation> {
        let index = self.pending_refused.iter().position(|pending| {
            pending.observation.code == observation.code
                && pending.observation.subject == observation.subject
                && pending.observation.observed == observation.observed
        })?;
        Some(self.pending_refused.swap_remove(index).op)
    }

    #[cfg(test)]
    pub(crate) fn pending_refused_count(&self) -> usize {
        self.pending_refused.len()
    }

    /// Whether a session is running on this project.
    pub(crate) fn is_running(&self) -> bool {
        self.running.is_some()
    }

    /// The running session as the sidecar record builder reads it (`IN2B` §3
    /// rule 4).
    ///
    /// Harness and model come from the session's config, turns and the two
    /// plain token sums from its shared counters; the token sums ride only
    /// once a cost event has been seen (the card's `tokens_known` rule), and
    /// the four cached/reasoning/cost mirrors stay `None` — the accumulated
    /// `SessionCost` lives on the session thread and mirrors once, at the end
    /// (D-R64), so no mid-run store exists to read. `None` when no session
    /// runs, in which case an `Investigating` entry writes the defensive
    /// empty-harness resolver.
    pub(crate) fn running_investigation(&self) -> Option<RunningInvestigation> {
        let running = self.running.as_ref()?;
        let counters = running.counters.snapshot();
        let tokens_known = running.cost_events.load(Ordering::Relaxed) > 0;
        Some(RunningInvestigation {
            id: running.id,
            harness: running.harness.clone(),
            model: running.model.clone(),
            turns: counters.turns,
            input_tokens: tokens_known.then_some(counters.input_tokens),
            cached_input_tokens: None,
            cache_creation_input_tokens: None,
            output_tokens: tokens_known.then_some(counters.output_tokens),
            reasoning_output_tokens: None,
            cost_usd_millionths: None,
        })
    }

    #[cfg(test)]
    pub(crate) fn running_endpoint(&self) -> Option<String> {
        self.running
            .as_ref()
            .and_then(|running| running.server.as_ref())
            .map(|server| server.endpoint().to_owned())
    }

    /// Snapshot the pair-level holders for the proposal-card callers.
    pub(crate) fn session_pairs(&self) -> InvestigatorSessionPairs {
        InvestigatorSessionPairs {
            running: self
                .running
                .as_ref()
                .map(|running| (running.id, running.code, running.subject)),
            queued: self
                .queue
                .iter()
                .map(|queued| (queued.id, queued.code, queued.subject))
                .collect(),
        }
    }

    /// The production press-path predicate for Re-investigate.
    pub(crate) fn can_reinvestigate(
        &self,
        id: IncidentId,
        code: IncidentCode,
        subject: IncidentSubject,
    ) -> bool {
        self.session_pairs().can_reinvestigate(id, code, subject)
    }

    /// Enqueue one incident (§3.1 rule 4, §3.2 rule 10).
    ///
    /// The same `(code, subject)` pair is queued once: a pair already queued
    /// or running is left alone. Returns whether anything was queued.
    pub(crate) fn enqueue(&mut self, incident: QueuedIncident) -> bool {
        if self
            .queue
            .iter()
            .any(|queued| queued.code == incident.code && queued.subject == incident.subject)
            || self.running.as_ref().is_some_and(|running| {
                running.code == incident.code && running.subject == incident.subject
            })
        {
            return false;
        }
        self.queue.push_back(incident);
        true
    }

    /// Drop every queued pair (the off switch, §2.3 rule 10).
    pub(crate) fn clear_queue(&mut self) {
        self.queue.clear();
    }

    /// Copy every queued incident's refused op into the write-time stash map
    /// (`IN2B` §3 rule 13): a close/reopen preserves opening context the
    /// queued session has not started yet. A copy, never a move — the queue
    /// item keeps its op for `compose_opening_message` at session start.
    pub(crate) fn copy_queued_refused_into(&self, map: &mut BTreeMap<IncidentId, Operation>) {
        for queued in &self.queue {
            if let Some(op) = &queued.refused {
                map.insert(queued.id, op.clone());
            }
        }
    }

    /// Shut the running session down and return its incident to `Open`: the
    /// hand-resolve path (§3.7 rule 41) and project close share it.
    pub(crate) fn shutdown_for_close(&mut self, reason: &str, incidents: &IncidentLogHandle) {
        let Some(mut running) = self.running.take() else {
            return;
        };
        running.cancel.store(true, Ordering::SeqCst);
        running.broker.reject_all(reason);
        signal_running_server(&mut running);
        let turns = running.counters.snapshot().turns;
        let resolver = session_resolver(&running.harness, running.model.as_deref(), reason);
        return_investigation_to_open(incidents, running.id, resolver, turns, None);
        detach_running_session(running);
    }
}

/// The running session's counters as the cards read them (§5.5 rules 14–15).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct InvestigatingCard {
    pub(crate) id: IncidentId,
    pub(crate) turns: u32,
    pub(crate) max_turns: u32,
    pub(crate) input_tokens: u64,
    pub(crate) output_tokens: u64,
    pub(crate) tokens_known: bool,
    pub(crate) max_tokens: u64,
    pub(crate) elapsed_secs: u64,
    pub(crate) max_wall_secs: u64,
}

// ---------------------------------------------------------------------------
// Opening message (IN2 §3.4)
// ---------------------------------------------------------------------------

/// Past this many bytes the refused operation is elided (§3.4 rule 23).
pub(crate) const REFUSED_OPERATION_CEILING_BYTES: usize = 4096;

/// The elision marker an over-ceiling operation carries (§3.4 rule 23).
pub(crate) const OPERATION_ELIDED_MARKER: &str = "… (operation elided at 4096 bytes)";

/// The instruction paragraph every session opens with (§3.4 rule 24).
pub(crate) const INVESTIGATOR_INSTRUCTIONS: &str = "You are Kinewright's investigator. The incident below was observed against the live project; this session runs on a branch of it. Read the current timeline with get_timeline_state, find what the incident needs with search_capabilities and get_capability, and prove the fix on this branch with prepare_edit_plan and commit_edit_plan. When the branch holds the fix, record it with propose_fix, passing this incident's id and one sentence a person can act on. Never record an outcome: resolve_incident is not yours to call, and no confirmation you raise will ever be approved — one ends the session. If no honest fix exists, say so and stop.";

/// Render the refused operation for the opening message (§3.4 rule 23).
///
/// The serialized operation in full under the ceiling; past it, the operation
/// tool name plus the elision marker. The ceiling is measured over the wire
/// JSON bytes (§3.4 rule 23, review fix F11).
fn render_refused_operation(operation: &Operation) -> String {
    let rendered = serde_json::to_string(operation).unwrap_or_default();
    if rendered.len() <= REFUSED_OPERATION_CEILING_BYTES {
        return rendered;
    }
    format!(
        "{} {OPERATION_ELIDED_MARKER}",
        operation_tool_name(operation)
    )
}

/// Compose the session's opening message (§3.4 rules 20–23): the instruction
/// paragraph, the incident's own wire body, and the refused operation.
pub(crate) fn compose_opening_message(incident: &Incident, refused: Option<&Operation>) -> String {
    let mut message = String::from(INVESTIGATOR_INSTRUCTIONS);
    message.push_str("\n\nIncident:\n");
    message.push_str(&serde_json::to_string(incident).unwrap_or_default());
    if let Some(operation) = refused {
        message.push_str("\n\nRefused operation:\n");
        message.push_str(&render_refused_operation(operation));
    }
    message
}

// ---------------------------------------------------------------------------
// Observer (IN2 §3.5 rule 30, D-R64)
// ---------------------------------------------------------------------------

/// Matches the destructive-proposal refusal in a tool result (§6 rule 3).
///
/// The structured body carries the `proposal_destructive` code; the text-only
/// rendering carries the refusal's message. Both are this server's literals.
#[allow(
    clippy::option_option,
    reason = "the three states distinguish no refusal, an untyped refusal, and its variant"
)]
fn destructive_refusal_variant(result: &str) -> Option<Option<String>> {
    const STRUCTURED_CODE: &str = "proposal_destructive";
    const TEXT_MESSAGE: &str = "may not propose a destructive operation";
    if !result.contains(STRUCTURED_CODE) && !result.contains(TEXT_MESSAGE) {
        return None;
    }
    Some(extract_observed_variant(result))
}

/// Best-effort `"observed":"<variant>"` extraction from a refusal rendering.
fn extract_observed_variant(result: &str) -> Option<String> {
    const KEY: &str = "\"observed\":\"";
    let start = result.find(KEY)? + KEY.len();
    let end = result[start..].find('"')?;
    Some(result[start..start + end].to_owned())
}

/// The prefix every destructive-stop reason carries, so the end table can
/// tell it from any other observer stop.
pub(crate) const DESTRUCTIVE_STOP_PREFIX: &str = "the session proposed a destructive operation";

/// The investigator's observer: cost accumulation, cost-event counting, and
/// the destructive-proposal stop (D-R64, §5.3 rule 9, §6 rule 3).
pub(crate) struct InvestigatorObserver {
    cost: SessionCost,
    cost_events: Arc<AtomicU32>,
}

impl InvestigatorObserver {
    pub(crate) fn new(cost_events: Arc<AtomicU32>) -> Self {
        Self {
            cost: SessionCost::default(),
            cost_events,
        }
    }

    pub(crate) fn cost(&self) -> SessionCost {
        self.cost
    }
}

impl SessionObserver for InvestigatorObserver {
    fn on_event(&mut self, event: &AgentEvent) -> SessionFlow {
        self.cost.accumulate(event);
        if matches!(event, AgentEvent::Cost { .. }) {
            self.cost_events.fetch_add(1, Ordering::Relaxed);
        }
        if let AgentEvent::ToolResult { result, .. } = event
            && let Some(variant) = destructive_refusal_variant(result)
        {
            let reason = variant.map_or_else(
                || DESTRUCTIVE_STOP_PREFIX.to_owned(),
                |variant| format!("{DESTRUCTIVE_STOP_PREFIX}: {variant}"),
            );
            return SessionFlow::Stop(reason);
        }
        if let AgentEvent::ToolResult { result, .. } = event
            && result.contains(INVESTIGATOR_CONFIRMATION_REFUSAL)
        {
            return SessionFlow::Stop("the session asked for a confirmation".to_owned());
        }
        SessionFlow::Continue
    }

    fn on_tick(&mut self, _counters: &SessionCounters) -> SessionFlow {
        SessionFlow::Continue
    }
}

// ---------------------------------------------------------------------------
// Session thread (IN2 §3.5: one owner, one thread, one loop)
// ---------------------------------------------------------------------------

/// Run one investigator session to completion on the session thread.
///
/// This owns the only stage-D loop call in the app crate: the thread builds
/// the harness session, drives it through `prompts`, and reports back. A
/// harness that refuses to start reports as `Cancelled(Harness)`, the same
/// stop the loop itself reports when the first turn fails (§2.2 rule 8).
#[allow(clippy::too_many_arguments)]
fn run_investigator_session(
    driver: &dyn AgentDriver,
    config: SessionConfig,
    prompts: Vec<String>,
    limits: SessionLimits,
    broker: &ConfirmationBroker,
    counters: &Arc<SharedCounters>,
    cost_events: Arc<AtomicU32>,
    cancel: &Arc<AtomicBool>,
    result_tx: &Sender<InvestigatorSessionResult>,
) {
    let mut session = match driver.start_session(config) {
        Ok(session) => session,
        Err(error) => {
            let _ = result_tx.send(InvestigatorSessionResult {
                stop: SessionStop::Cancelled(StopReason::Harness(error.to_string())),
                counters: SessionCounters::default(),
                cost: SessionCost::default(),
            });
            return;
        }
    };
    let events = session.events();
    let mut prompts = prompts.into_iter();
    let mut observer = InvestigatorObserver::new(cost_events);
    let (stop, counters) = kinewright_agent::pump_session(
        session.as_mut(),
        &events,
        &mut prompts,
        Some(broker),
        ConfirmationPolicy::RejectAndStop,
        &limits,
        counters,
        cancel,
        &mut observer,
    );
    let _ = result_tx.send(InvestigatorSessionResult {
        stop,
        counters,
        cost: observer.cost(),
    });
}

/// Signal the session server without joining its server thread.
///
/// Cancellation callers invoke this after setting the flag and rejecting all
/// confirmations, but before writing the incident end.  That ordering closes
/// the late-tool window while keeping the frame thread non-blocking (H3).
fn signal_running_server(running: &mut RunningSession) {
    if let Some(server) = running.server.take() {
        drop(server);
    }
}

/// Move the remaining session-owned resources to a detached reaper. The
/// frame thread never joins: the reaper joins only the harness thread, drains
/// its result channel, and drops the broker/branch (review fix G3).
fn detach_running_session(mut running: RunningSession) {
    // Normal completion has already written its end.  Signalling here also
    // covers the completion and unexpected-disconnect paths; cancellation
    // paths signal earlier, before their end write.
    signal_running_server(&mut running);
    let RunningSession {
        thread,
        branch,
        broker,
        result,
        ..
    } = running;
    std::thread::Builder::new()
        .name("kinewright-investigator-reaper".to_owned())
        .spawn(move || {
            if let Some(thread) = thread {
                let _ = thread.join();
            }
            for _ in result.try_iter() {}
            drop(broker);
            drop(branch);
        })
        .expect("investigator reaper thread must spawn");
}

// ---------------------------------------------------------------------------
// Lifecycle (IN2 §3.1–§3.2, §3.6–§3.7)
// ---------------------------------------------------------------------------

/// Return one incident to `Open` with its session telemetry.
///
/// One write guard holds the transition, the single cost mirror (D-R64), the
/// turn count, and the resolver together (§4.4 rule 18's lock discipline).
/// False — with nothing written — when the entry is no longer
/// `Investigating`: somebody else owns the record now.
fn return_investigation_to_open(
    incidents: &IncidentLogHandle,
    id: IncidentId,
    resolver: IncidentResolver,
    turns: u32,
    mirror: Option<&AgentEvent>,
) -> bool {
    let mut log = incidents
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if !log.end_investigation(id, resolver.clone()) {
        return false;
    }
    if let Some(telemetry) = log.telemetry_mut(id) {
        if let Some(event) = mirror {
            mirror_agent_cost(telemetry, event);
        }
        telemetry.turns = Some(turns);
        telemetry.resolver = Some(resolver);
    }
    true
}

/// Write one finished session's telemetry without changing the state: the
/// `Completed`-with-proposal end (impl-agent.md §8's end table).
fn write_session_telemetry_only(
    incidents: &IncidentLogHandle,
    id: IncidentId,
    resolver: IncidentResolver,
    turns: u32,
    mirror: Option<&AgentEvent>,
) {
    let mut log = incidents
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(telemetry) = log.telemetry_mut(id) {
        if let Some(event) = mirror {
            mirror_agent_cost(telemetry, event);
        }
        telemetry.turns = Some(turns);
        telemetry.resolver = Some(resolver);
    }
}

fn session_resolver(harness: &str, model: Option<&str>, stop: &str) -> IncidentResolver {
    IncidentResolver::Session {
        harness: harness.to_owned(),
        model: model.map(str::to_owned),
        stop: stop.to_owned(),
    }
}

/// The off switch's stop string (§3.7 rule 38).
const OFF_SWITCH_STOP: &str = "the investigator was switched off";

impl KinewrightApp {
    /// The harness key a session on `project_index` would run on, or `None`
    /// when no session may start (§3.1 rule 4, §2.2 rules 7–8).
    ///
    /// A test driver skips detection but never the one-strike list.
    fn investigator_harness_key(&self, project_index: usize) -> Option<String> {
        let session = self.projects[project_index].investigator.as_ref()?;
        if !session.settings.enabled {
            return None;
        }
        let key = if session.test_driver.is_some() {
            session
                .settings
                .harness
                .clone()
                .unwrap_or_else(|| "scripted".to_owned())
        } else {
            match &session.settings.harness {
                Some(key) => {
                    let choice = AgentHarnessChoice::from_key(key)?;
                    let info = self.harness[choice.index()].info.as_ref()?;
                    if info.authentication == AuthenticationStatus::Unauthenticated {
                        return None;
                    }
                    key.clone()
                }
                None => AgentHarnessChoice::ALL.iter().find_map(|choice| {
                    let info = self.harness[choice.index()].info.as_ref()?;
                    if info.authentication == AuthenticationStatus::Unauthenticated {
                        return None;
                    }
                    Some(choice.key().to_owned())
                })?,
            }
        };
        if harness_disabled_by_one_strike(&key) {
            return None;
        }
        Some(key)
    }

    /// Whether a refused operation may be retained for a future opening
    /// message. This is deliberately the same gate as queueing/taking one:
    /// disabled, muted, unknown, or unauthenticated investigators do not grow
    /// a hidden backlog (§3.4 rule 22, review fix F2).
    fn may_stash_refused(&self, project_index: usize, code: IncidentCode) -> bool {
        let Some(session) = self.projects[project_index].investigator.as_ref() else {
            return false;
        };
        session.settings.enabled
            && INVESTIGATOR_ALLOWLIST.contains(&code)
            && !session.is_muted(code)
            && self.investigator_harness_key(project_index).is_some()
    }

    /// Whether a session may start for this pair on this project (`IN2B` §3
    /// rule 12, E-C1): the ONE shared eligibility predicate — allowlisted,
    /// enabled, unmuted, harness present, pair idle — called by
    /// `consider_investigator_queue`, the Investigate press path, and the
    /// card's caller. A shown button never returns `false`. Pair-idle shares
    /// Re-investigate's rule: a running pair always blocks, a queued pair
    /// blocks a different incident, and the same already-queued incident is
    /// the successful no-op.
    pub(crate) fn investigator_session_eligible(
        &self,
        project_index: usize,
        id: IncidentId,
        code: IncidentCode,
        subject: IncidentSubject,
    ) -> bool {
        if !INVESTIGATOR_ALLOWLIST.contains(&code) {
            return false;
        }
        let Some(session) = self.projects[project_index].investigator.as_ref() else {
            return false;
        };
        if !session.settings.enabled || session.is_muted(code) {
            return false;
        }
        if self.investigator_harness_key(project_index).is_none() {
            return false;
        }
        session.can_reinvestigate(id, code, subject)
    }

    /// Consider one freshly opened incident for an investigator session
    /// (§3.1 rule 4, §3.2 rule 10).
    pub(crate) fn consider_investigator_queue(
        &mut self,
        id: IncidentId,
        observation: &IncidentObservation,
    ) {
        let focused = self.focused_project;
        self.consider_investigator_queue_for(focused, id, observation);
    }

    /// [`Self::consider_investigator_queue`] routed at one project: the
    /// router funnels here for the focused project, the Investigate press
    /// for the card's. One predicate, three callers, zero drift (§3
    /// rules 11–12).
    fn consider_investigator_queue_for(
        &mut self,
        project_index: usize,
        id: IncidentId,
        observation: &IncidentObservation,
    ) {
        if !self.investigator_session_eligible(
            project_index,
            id,
            observation.code,
            observation.subject,
        ) {
            return;
        }
        let mut refused = None;
        for project in &mut self.projects {
            if let Some(session) = project.investigator.as_mut()
                && let Some(op) = session.take_refused(observation)
            {
                refused = Some(op);
                break;
            }
        }
        // The restored stash rides second: a fresh `take_refused` wins over
        // the restored op — fresher evidence (`IN2B` §3 rule 13, N2/S-2).
        // Consumed (removed) exactly when a session is about to queue for
        // the incident; an ineligible pass leaves it for a later press.
        let restored = self.projects[project_index].refused_by_id.remove(&id);
        let refused = refused.or(restored);
        let queued = QueuedIncident {
            id,
            code: observation.code,
            subject: observation.subject,
            refused,
        };
        self.projects[project_index]
            .investigator
            .get_or_insert_default()
            .enqueue(queued);
    }

    /// Drain finished sessions and start queued ones, on every project (§3.2).
    pub(crate) fn pump_investigator_sessions(&mut self) {
        for index in 0..self.projects.len() {
            self.finish_investigator_session(index);
            self.start_next_investigation(index);
        }
    }

    /// How many investigator sessions are running across every project (§3.2
    /// rule 12). A method, not a field: the cap is derived, never stored.
    pub(crate) fn investigator_running_count(&self) -> usize {
        self.projects
            .iter()
            .filter(|project| {
                project
                    .investigator
                    .as_ref()
                    .is_some_and(InvestigatorSession::is_running)
            })
            .count()
    }

    /// Finish the running session on one project, if it finished (§3.7).
    fn finish_investigator_session(&mut self, project_index: usize) {
        let running_id = self.projects[project_index]
            .investigator
            .as_ref()
            .and_then(|session| session.running.as_ref())
            .map(|running| running.id);
        let Some(id) = running_id else { return };
        let state = self.projects[project_index]
            .incidents
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(id)
            .map(|incident| incident.state);
        if state != Some(IncidentState::Investigating) {
            // Resolved by hand (or otherwise left) mid-session: cancel and
            // write nothing (§3.7 rule 41).
            self.cancel_investigator_silently(project_index);
            return;
        }
        let outcome = {
            let running = self.projects[project_index]
                .investigator
                .as_ref()
                .and_then(|session| session.running.as_ref())
                .expect("a running session was observed above");
            match running.result.try_recv() {
                Ok(result) => InvestigatorFinishOutcome::Done(result),
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => InvestigatorFinishOutcome::Died,
            }
        };
        let running = self.projects[project_index]
            .investigator
            .as_mut()
            .and_then(|session| session.running.take())
            .expect("a running session was observed above");
        match outcome {
            InvestigatorFinishOutcome::Done(result) => {
                self.write_investigator_end(project_index, &running, result);
            }
            InvestigatorFinishOutcome::Died => {
                self.write_investigator_unexpected_end(project_index, &running);
            }
        }
        detach_running_session(running);
    }

    /// Start the next queued session on one project, if one may start (§3.2).
    fn start_next_investigation(&mut self, project_index: usize) {
        if self.projects[project_index]
            .investigator
            .as_ref()
            .is_some_and(InvestigatorSession::is_running)
        {
            return;
        }
        if self.investigator_running_count() >= INVESTIGATOR_MAX_CONCURRENT_SESSIONS {
            return;
        }
        let Some(harness_key) = self.investigator_harness_key(project_index) else {
            return;
        };
        loop {
            let queued = self.projects[project_index]
                .investigator
                .as_mut()
                .and_then(|session| session.queue.pop_front());
            let Some(queued) = queued else { return };
            let incident = self.projects[project_index]
                .incidents
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get(queued.id)
                .cloned();
            let Some(incident) = incident else { continue };
            if incident.state != IncidentState::Open {
                continue;
            }
            {
                let mut log = self.projects[project_index]
                    .incidents
                    .write()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if !log.begin_investigation(queued.id) {
                    continue;
                }
            }
            let opening = compose_opening_message(&incident, queued.refused.as_ref());
            if let Ok(()) = self.spawn_investigator_session(
                project_index,
                &queued,
                &harness_key,
                vec![opening],
                Vec::new(),
            ) {
                return;
            }
            // Branch or server construction failed: never leave the incident
            // `Investigating` with no session behind it.
            let incidents = Arc::clone(&self.projects[project_index].incidents);
            return_investigation_to_open(
                &incidents,
                incident.id,
                session_resolver(
                    &harness_key,
                    None,
                    "the investigator session could not start",
                ),
                0,
                None,
            );
        }
    }

    /// Build the branch, server, and thread for one queued pair (§3.3).
    ///
    /// Production passes one prompt — the opening message (§3.5 rule 29a) —
    /// and no branch preload. The scripted-suite tests pass more prompts to
    /// reach the pump's own turn counter (S4) or a post-turn token stop (S6),
    /// and preload the branch edits a static script cannot commit itself: a
    /// `prepare_edit_plan` id is only known after the call, so S1's proposal
    /// content and S7's destructive branch are seeded here, deterministically,
    /// before the pump thread starts.
    #[allow(
        clippy::too_many_lines,
        reason = "the session construction keeps the lifecycle in one audited path"
    )]
    pub(crate) fn spawn_investigator_session(
        &mut self,
        project_index: usize,
        queued: &QueuedIncident,
        harness_key: &str,
        prompts: Vec<String>,
        preload: Vec<Operation>,
    ) -> Result<(), ()> {
        let settings = self.projects[project_index]
            .investigator
            .as_ref()
            .map(|session| session.settings.clone())
            .unwrap_or_default();
        let live_revision = self.projects[project_index].revision;
        let branch = TimelineBranch::new_at(
            "investigate",
            live_revision,
            Arc::clone(&self.projects[project_index].document),
        )
        .map_err(|_| ())?;
        if !preload.is_empty()
            && !matches!(
                branch.core().request(Command::DoBatchIfRevision {
                    expected: live_revision,
                    operations: preload,
                }),
                Ok(Event::DocumentChanged { .. })
            )
        {
            return Err(());
        }
        let server = McpServer::start_investigator_session(
            branch.core(),
            Arc::clone(&self.playback),
            Arc::clone(&self.analysis),
            Arc::clone(&self.exporter),
            Arc::clone(&self.projects[project_index].agent_project_path),
            Arc::clone(&self.projects[project_index].incidents),
            &INVESTIGATOR_CAPABILITY_DENYLIST,
            INVESTIGATOR_WORKER_THREADS,
            InvestigatorSessionContext {
                incident: queued.id,
                base_revision: live_revision,
            },
        )
        .map_err(|_| ())?;
        let driver: Box<dyn AgentDriver> = if let Some(test_driver) = self.projects[project_index]
            .investigator
            .as_ref()
            .and_then(|session| session.test_driver.clone())
        {
            Box::new(test_driver)
        } else {
            let Some(driver) = harness_driver(harness_key) else {
                server.shutdown();
                return Err(());
            };
            driver
        };
        let endpoint = server.endpoint().to_owned();
        let broker = server.confirmations();
        let working_directory =
            investigator_working_directory(self.projects[project_index].project_path.as_deref());
        #[cfg(test)]
        let test_start_delay = self.projects[project_index]
            .investigator
            .as_ref()
            .and_then(|session| session.test_start_delay);
        let config = SessionConfig {
            mcp_url: Some(endpoint),
            tool_names: Some(
                INVESTIGATOR_TOOL_NAMES
                    .iter()
                    .map(|name| (*name).to_owned())
                    .collect(),
            ),
            model: settings.model.clone(),
            effort: settings.effort.clone(),
            service_tier: settings.service_tier.clone(),
            max_turns: Some(settings.budgets.max_turns),
            working_directory,
        };
        let limits = SessionLimits {
            max_turns: settings.budgets.max_turns,
            max_wall_time: Duration::from_secs(settings.budgets.max_wall_time_seconds),
            max_tokens: Some(settings.budgets.max_tokens),
        };
        let counters = Arc::new(SharedCounters::default());
        let cost_events = Arc::new(AtomicU32::new(0));
        let cancel = Arc::new(AtomicBool::new(false));
        let (result_tx, result_rx) = unbounded();
        let Ok(thread) = std::thread::Builder::new()
            .name("kinewright-investigator".to_owned())
            .spawn({
                let broker = broker.clone();
                let counters = Arc::clone(&counters);
                let cost_events = Arc::clone(&cost_events);
                let cancel = Arc::clone(&cancel);
                move || {
                    #[cfg(test)]
                    if let Some(delay) = test_start_delay {
                        std::thread::sleep(delay);
                    }
                    run_investigator_session(
                        driver.as_ref(),
                        config,
                        prompts,
                        limits,
                        &broker,
                        &counters,
                        cost_events,
                        &cancel,
                        &result_tx,
                    );
                }
            })
        else {
            server.shutdown();
            return Err(());
        };
        let running = RunningSession {
            id: queued.id,
            code: queued.code,
            subject: queued.subject,
            harness: harness_key.to_owned(),
            model: settings.model.clone(),
            branch,
            server: Some(server),
            broker,
            thread: Some(thread),
            counters,
            cost_events,
            cancel,
            result: result_rx,
            budgets: settings.budgets.clone(),
        };
        self.projects[project_index]
            .investigator
            .as_mut()
            .expect("a session state exists while its session starts")
            .running = Some(running);
        Ok(())
    }

    /// The end table (§3.7 rule 38, impl-agent.md §8), written verbatim.
    #[allow(
        clippy::too_many_lines,
        reason = "the exhaustive end table is kept together for review"
    )]
    fn write_investigator_end(
        &mut self,
        project_index: usize,
        running: &RunningSession,
        result: InvestigatorSessionResult,
    ) {
        let InvestigatorSessionResult {
            stop,
            counters,
            cost,
        } = result;
        let mirror = cost.as_event();
        let id = running.id;
        let resolver =
            |stop: &str| session_resolver(&running.harness, running.model.as_deref(), stop);
        match stop {
            SessionStop::Completed => {
                let has_proposal = self.projects[project_index]
                    .incidents
                    .read()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .get(id)
                    .is_some_and(|incident| incident.proposal.is_some());
                if has_proposal {
                    // Leave `Investigating`: the card offers Approve and Reject.
                    let incidents = Arc::clone(&self.projects[project_index].incidents);
                    write_session_telemetry_only(
                        &incidents,
                        id,
                        resolver("completed"),
                        counters.turns,
                        mirror.as_ref(),
                    );
                } else {
                    let mut log = self.projects[project_index]
                        .incidents
                        .write()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    if log.resolve(id, IncidentOutcome::Explained)
                        && let Some(telemetry) = log.telemetry_mut(id)
                    {
                        if let Some(event) = &mirror {
                            mirror_agent_cost(telemetry, event);
                        }
                        telemetry.turns = Some(counters.turns);
                        telemetry.resolver = Some(resolver("completed"));
                    }
                }
            }
            SessionStop::Budget(kind) => {
                let stop = match kind {
                    BudgetKind::Turns => "budget: turns",
                    BudgetKind::WallTime => "budget: wall time",
                    BudgetKind::Tokens => "budget: tokens",
                };
                let incidents = Arc::clone(&self.projects[project_index].incidents);
                return_investigation_to_open(
                    &incidents,
                    id,
                    resolver(stop),
                    counters.turns,
                    mirror.as_ref(),
                );
            }
            SessionStop::PolicyViolation => {
                let incidents = Arc::clone(&self.projects[project_index].incidents);
                return_investigation_to_open(
                    &incidents,
                    id,
                    resolver("the session asked for a confirmation"),
                    counters.turns,
                    mirror.as_ref(),
                );
            }
            SessionStop::Disconnected => {
                let incidents = Arc::clone(&self.projects[project_index].incidents);
                return_investigation_to_open(
                    &incidents,
                    id,
                    resolver("the agent event stream disconnected"),
                    counters.turns,
                    mirror.as_ref(),
                );
            }
            SessionStop::Cancelled(StopReason::Harness(message)) => {
                let incidents = Arc::clone(&self.projects[project_index].incidents);
                return_investigation_to_open(
                    &incidents,
                    id,
                    resolver(&message),
                    counters.turns,
                    mirror.as_ref(),
                );
                disable_harness_for_session(&running.harness);
                self.note_label(
                    LabelIncident::Agent,
                    IncidentSubject::Agent,
                    format!(
                        "the {} harness failed while investigating: {message}",
                        running.harness
                    ),
                );
            }
            SessionStop::Cancelled(StopReason::Observer(reason)) => {
                // An observer stop the app did not ask for: the
                // destructive-proposal stop, or a future observer reason. The
                // app-set flag never reaches this table — a cancel joins the
                // thread and writes its own reason instead (§3.7 rule 37).
                let incidents = Arc::clone(&self.projects[project_index].incidents);
                return_investigation_to_open(
                    &incidents,
                    id,
                    resolver(&reason),
                    counters.turns,
                    mirror.as_ref(),
                );
            }
            SessionStop::Cancelled(StopReason::OffSwitch) => {
                // The loop never reports this; the off switch writes its own
                // end. The arm exists so a future reporter still lands `Open`.
                let incidents = Arc::clone(&self.projects[project_index].incidents);
                return_investigation_to_open(
                    &incidents,
                    id,
                    resolver(OFF_SWITCH_STOP),
                    counters.turns,
                    mirror.as_ref(),
                );
            }
            SessionStop::Cancelled(StopReason::ResolvedElsewhere) => {
                // Somebody else owns the record now: write nothing (rule 41).
            }
        }
    }

    /// The session thread died without reporting: reap nothing, shut the
    /// server down, and never leave the incident `Investigating`.
    fn write_investigator_unexpected_end(
        &mut self,
        project_index: usize,
        running: &RunningSession,
    ) {
        let incidents = Arc::clone(&self.projects[project_index].incidents);
        return_investigation_to_open(
            &incidents,
            running.id,
            session_resolver(
                &running.harness,
                running.model.as_deref(),
                "the investigator session ended unexpectedly",
            ),
            running.counters.snapshot().turns,
            None,
        );
    }

    /// Cancel the running session for the off switch: set the flag, answer
    /// every confirmation, write the end, and hand ownership to a detached
    /// reaper (§3.7 rule 37, review fix F3).
    fn cancel_investigator_for_off_switch(&mut self, project_index: usize) {
        let Some(mut running) = self.projects[project_index]
            .investigator
            .as_mut()
            .and_then(|session| session.running.take())
        else {
            return;
        };
        running.cancel.store(true, Ordering::SeqCst);
        running.broker.reject_all(OFF_SWITCH_STOP);
        signal_running_server(&mut running);
        let incidents = Arc::clone(&self.projects[project_index].incidents);
        return_investigation_to_open(
            &incidents,
            running.id,
            session_resolver(&running.harness, running.model.as_deref(), OFF_SWITCH_STOP),
            running.counters.snapshot().turns,
            None,
        );
        detach_running_session(running);
    }

    /// Cancel the running session without changing a person-owned outcome;
    /// `shutdown_for_close` still writes an end when the entry remains
    /// Investigating (§3.7 rule 41, review fix F3).
    fn cancel_investigator_silently(&mut self, project_index: usize) {
        let incidents = Arc::clone(&self.projects[project_index].incidents);
        if let Some(session) = self.projects[project_index].investigator.as_mut() {
            session.shutdown_for_close("the incident was resolved elsewhere", &incidents);
        }
    }

    /// Run the off switch (§2.3 rule 10): clear every queue and cancel every
    /// running session, writing each end with the app's own reason and
    /// suppressing nothing. No file write: the settings window persists
    /// through [`Self::update_investigator_settings`], and the off-switch
    /// test must not touch the person's real config file.
    pub(crate) fn set_investigator_enabled(&mut self, enabled: bool) {
        for project in &mut self.projects {
            project
                .investigator
                .get_or_insert_default()
                .settings
                .enabled = enabled;
        }
        if !enabled {
            for index in 0..self.projects.len() {
                if let Some(session) = self.projects[index].investigator.as_mut() {
                    session.clear_queue();
                    session.pending_refused.clear();
                }
                self.cancel_investigator_for_off_switch(index);
            }
        }
    }

    /// Apply one settings change to every project's copy and run the
    /// off-switch side effects. Persistence is a UI commit, not a keystroke or
    /// drag-frame side effect (§2.1 rule 4, review fix F14).
    pub(crate) fn update_investigator_settings(
        &mut self,
        update: impl Fn(&mut InvestigatorSettings),
    ) {
        for project in &mut self.projects {
            update(&mut project.investigator.get_or_insert_default().settings);
        }
        let enabled = self
            .focused()
            .investigator
            .as_ref()
            .is_some_and(|session| session.settings.enabled);
        // The side effects run through the same path the test drives, so the
        // tested behaviour and the shipped behaviour cannot drift.
        if !enabled {
            self.set_investigator_enabled(false);
        }
    }

    /// Persist the current app-wide investigator settings once a settings
    /// control commits or the section closes. A non-durable resolver run is a
    /// no-op (§2.1 rule 1, review fixes F5 and F14).
    pub(crate) fn commit_investigator_settings(&mut self) {
        let settings = self
            .focused()
            .investigator
            .as_ref()
            .map(|session| session.settings.clone())
            .unwrap_or_default();
        let durable = self
            .focused()
            .investigator
            .as_ref()
            .is_some_and(|session| session.settings_durable);
        let (path, resolved_durable) = investigator_config_path();
        if let Err(error) =
            save_investigator_settings_for_run(&path, durable && resolved_durable, &settings)
            && should_report_settings_write_failure()
        {
            self.note_label(
                LabelIncident::Agent,
                IncidentSubject::Agent,
                format!("the investigator settings could not be saved: {error}"),
            );
        }
    }

    /// The running session's card counters for one project, if a session is
    /// running there (§5.5 rules 14–15).
    pub(crate) fn investigating_card_for_project(
        &self,
        project_index: usize,
    ) -> Option<InvestigatingCard> {
        let running = self.projects[project_index]
            .investigator
            .as_ref()?
            .running
            .as_ref()?;
        let counters = running.counters.snapshot();
        Some(InvestigatingCard {
            id: running.id,
            turns: counters.turns,
            max_turns: running.budgets.max_turns,
            input_tokens: counters.input_tokens,
            output_tokens: counters.output_tokens,
            tokens_known: running.cost_events.load(Ordering::Relaxed) > 0,
            max_tokens: running.budgets.max_tokens,
            elapsed_secs: counters.elapsed.as_secs(),
            max_wall_secs: running.budgets.max_wall_time_seconds,
        })
    }

    /// The pair-level queue/running snapshot for incident-card callers.
    /// Session state remains on `ProjectSession`; this is only a read-only
    /// view and adds no app-wide field (review fix H1).
    pub(crate) fn investigator_session_pairs_for_project(
        &self,
        project_index: usize,
    ) -> InvestigatorSessionPairs {
        self.projects[project_index]
            .investigator
            .as_ref()
            .map_or_else(
                InvestigatorSessionPairs::default,
                InvestigatorSession::session_pairs,
            )
    }

    #[cfg(test)]
    pub(crate) fn investigator_pending_confirmation_requests(
        &self,
        project_index: usize,
    ) -> Vec<kinewright_agent::ConfirmationRequest> {
        self.projects[project_index]
            .investigator
            .as_ref()
            .and_then(|session| session.running.as_ref())
            .map_or_else(Vec::new, |running| running.broker.pending_requests())
    }

    #[cfg(test)]
    pub(crate) fn investigator_take_and_reject_confirmation_requests(
        &self,
        project_index: usize,
    ) -> Vec<kinewright_agent::ConfirmationRequest> {
        let Some(running) = self.projects[project_index]
            .investigator
            .as_ref()
            .and_then(|session| session.running.as_ref())
        else {
            return Vec::new();
        };
        let requests = running.broker.pending_requests();
        for request in &requests {
            running
                .broker
                .reject(request.id, INVESTIGATOR_CONFIRMATION_REFUSAL);
        }
        requests
    }

    #[cfg(test)]
    pub(crate) fn investigator_branch_applied_operation_count(
        &self,
        project_index: usize,
    ) -> Option<usize> {
        let running = self.projects[project_index]
            .investigator
            .as_ref()?
            .running
            .as_ref()?;
        match running
            .branch
            .core()
            .request(Command::Query(Query::AppliedOperations))
        {
            Ok(Event::QueryResult(QueryResult::AppliedOperations(operations))) => {
                Some(operations.len())
            }
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Proposal card and actions (IN2 §4.3–§4.4)
// ---------------------------------------------------------------------------

/// One proposal action (§4.3 rule 12).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProposalAction {
    Approve,
    Reject,
    Reinvestigate,
}

/// The proposal card (§4.3 rule 12), exactly the printed shape.
#[derive(Debug, Clone)]
pub(crate) struct ProposalCardView {
    pub(crate) subject_label: String,
    pub(crate) headline: &'static str,
    pub(crate) explanation: String,
    pub(crate) operation_summary: Vec<String>,
    pub(crate) approval_note: &'static str,
    pub(crate) stale: bool,
    pub(crate) actions: Vec<ProposalAction>,
}

/// Build the proposal card for one incident and its proposal (§4.3 rules
/// 12–15).
///
/// `operation_summary` renders one line per operation with the person's
/// subject label. A stale proposal offers only Re-investigate: Approve could
/// not succeed, and Reject answers a question the merge already settled.
#[cfg(test)]
pub(crate) fn proposal_card(incident: &Incident, proposal: &IncidentProposal) -> ProposalCardView {
    proposal_card_with_reinvestigate(incident, proposal, true)
}

/// Whether an incident should have a proposal card at all. Resolved entries
/// retain proposals as history, but a resolved proposal is no longer an
/// actionable card (§4.3 rule 12, review fix G2).
#[must_use]
pub(crate) fn shows_proposal_card(incident: &Incident) -> bool {
    incident.state.is_open() && incident.proposal.is_some()
}

/// Build a proposal card when the caller can account for a currently running
/// session on the same incident. A stale proposal cannot offer a second
/// Re-investigate while that same incident already has a running thread: the
/// press path deliberately returns false in that state (review fix G1).
pub(crate) fn proposal_card_with_reinvestigate(
    incident: &Incident,
    proposal: &IncidentProposal,
    can_reinvestigate: bool,
) -> ProposalCardView {
    let operation_summary: Vec<String> = proposal
        .operations
        .iter()
        .map(|operation| {
            format!(
                "{} — {}",
                operation_tool_name(operation),
                incident.subject.label()
            )
        })
        .collect();
    let actions = if proposal.stale && can_reinvestigate {
        vec![ProposalAction::Reinvestigate]
    } else if proposal.stale {
        Vec::new()
    } else if can_reinvestigate {
        vec![
            ProposalAction::Approve,
            ProposalAction::Reject,
            ProposalAction::Reinvestigate,
        ]
    } else {
        vec![ProposalAction::Approve, ProposalAction::Reject]
    };
    ProposalCardView {
        subject_label: incident.subject.label(),
        headline: if proposal.stale {
            "The proposed fix no longer applies"
        } else {
            "The investigator proposes a fix"
        },
        explanation: proposal.explanation.clone(),
        operation_summary,
        approval_note: "Approve applies these edits to your project; one Undo takes them back.",
        stale: proposal.stale,
        actions,
    }
}

impl KinewrightApp {
    /// The live core's current revision, asked synchronously (§4.4 rule 18's
    /// re-read). `None` when the core has stopped or answers impossibly.
    ///
    /// Core currently exposes only its unbounded `request` receiver; stage C
    /// cannot add a timeout-shaped cross-crate API without changing that
    /// contract. The second synchronous wait is inside the unchanged agent
    /// `apply_to_live` helper for the same reason (review fix F3; recorded as
    /// an unresolved lead decision in the report).
    fn live_revision_now(&self, project_index: usize) -> Option<TimelineRevision> {
        match self.projects[project_index]
            .core
            .request(Command::Query(Query::Snapshot))
        {
            Ok(Event::QueryResult(QueryResult::Snapshot { revision, .. })) => Some(revision),
            _ => None,
        }
    }

    /// Apply the approved proposal (§4.4 rule 18).
    ///
    /// True when the press was handled — the incident had a fresh proposal in
    /// `Open` or `Investigating`. False when the card had nothing approvable.
    pub(crate) fn approve_investigator_proposal(
        &mut self,
        project_index: usize,
        id: IncidentId,
    ) -> bool {
        let (operations, subject) = {
            let log = self.projects[project_index]
                .incidents
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some(incident) = log.get(id) else {
                return false;
            };
            if !matches!(
                incident.state,
                IncidentState::Open | IncidentState::Investigating
            ) {
                return false;
            }
            let Some(proposal) = incident.proposal.as_ref() else {
                return false;
            };
            if proposal.stale {
                return false;
            }
            (proposal.operations.clone(), incident.subject)
        };
        // First attempt at the tracked live revision; one retry at the
        // freshly re-read live revision; then terminal (§4.4 rule 18).
        let mut expected = self.projects[project_index].revision;
        let mut first = true;
        let outcome = loop {
            let attempt = apply_to_live(
                &self.projects[project_index].core,
                expected,
                operations.clone(),
            );
            match attempt {
                Ok(BranchApplyOutcome::Conflict { actual, .. }) if first => {
                    first = false;
                    // Rule 18's re-read is a synchronous revision query rather
                    // than an event drain, which would steal the frame drain's
                    // events: an `OpRejected` consumed here is an incident the
                    // person never sees. The value is the same one a drain
                    // would publish — live's counter, now — and under a
                    // concurrent committer it is fresher than the conflict's
                    // own `actual`, which is only the fallback for a dead core
                    // (whose retry fails, so the `Err` arm still reports it).
                    expected = self.live_revision_now(project_index).unwrap_or(actual);
                    #[cfg(test)]
                    if let Some(session) = self.projects[project_index].investigator.as_mut()
                        && let Some(hook) = session.approval_retry_hook.as_mut()
                    {
                        hook(1);
                    }
                }
                outcome => break outcome,
            }
        };
        match outcome {
            Ok(BranchApplyOutcome::Applied { .. }) => {
                let resolved = self.projects[project_index]
                    .incidents
                    .write()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .resolve(id, IncidentOutcome::Applied);
                if resolved {
                    // Rule 18's audit line goes through the crate's one
                    // `note_incident` call site (`IN1b` §5.3 rule 18): on an
                    // `Explain` incident `audit_new_incident` writes the line
                    // and sends nothing, so the sink gate's count stays 1
                    // while the write still goes through `note_incident`.
                    self.audit_new_incident(project_index, id, &mut Vec::new());
                }
                true
            }
            Ok(BranchApplyOutcome::Conflict { .. }) => {
                self.return_proposal_open_with_stale(
                    project_index,
                    id,
                    "the proposal conflicted twice against the live timeline",
                );
                true
            }
            Ok(BranchApplyOutcome::Rejected { error, .. }) => {
                self.return_proposal_open_with_stale(
                    project_index,
                    id,
                    "the live timeline refused the proposal",
                );
                let revision = self.projects[project_index].revision;
                self.note_observation(IncidentObservation::from_batch_error(
                    &error, subject, revision,
                ));
                true
            }
            Ok(BranchApplyOutcome::NoChanges) => {
                self.return_proposal_open_with_stale(
                    project_index,
                    id,
                    "the proposal carried no operations",
                );
                true
            }
            Err(error) => {
                self.return_proposal_open_with_stale(project_index, id, &error.to_string());
                true
            }
        }
    }

    /// Mark the proposal stale and return the incident to `Open`, keeping the
    /// session's harness and model on the new resolver (§4.4 rules 18–19).
    ///
    /// `pub(crate)` for the scripted stale-terminal fixture; production
    /// approval paths also call this helper for every terminal apply failure.
    pub(crate) fn return_proposal_open_with_stale(
        &mut self,
        project_index: usize,
        id: IncidentId,
        stop: &str,
    ) {
        let mut log = self.projects[project_index]
            .incidents
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        log.mark_proposal_stale(id);
        let resolver = match log
            .get(id)
            .and_then(|incident| incident.telemetry.resolver.clone())
        {
            Some(IncidentResolver::Session { harness, model, .. }) => {
                session_resolver(&harness, model.as_deref(), stop)
            }
            _ => session_resolver("", None, stop),
        };
        let state = log.get(id).map(|incident| incident.state);
        if state == Some(IncidentState::Investigating) {
            log.end_investigation(id, resolver);
        } else if state == Some(IncidentState::Open)
            && let Some(telemetry) = log.telemetry_mut(id)
        {
            // A proposal can outlive a budget/policy/disconnect end. It is
            // already `Open`, so there is no end transition left to write;
            // approval's terminal stale result still records its resolver.
            telemetry.resolver = Some(resolver);
        }
    }

    /// Reject the proposal (§4.4 rule 19): resolve `Rejected` and leave the
    /// proposal on the entry, inert — a suppressed pair in an unrendered
    /// state (lead ruling §8.2).
    pub(crate) fn reject_investigator_proposal(
        &mut self,
        project_index: usize,
        id: IncidentId,
    ) -> bool {
        let mut log = self.projects[project_index]
            .incidents
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !log.get(id).is_some_and(|incident| {
            matches!(
                incident.state,
                IncidentState::Open | IncidentState::Investigating
            ) && incident
                .proposal
                .as_ref()
                .is_some_and(|proposal| !proposal.stale)
        }) {
            return false;
        }
        log.resolve(id, IncidentOutcome::Rejected)
    }

    /// The Investigate press on a loaded, never-re-seen row (`IN2B` §3
    /// rule 12): enqueues through `consider_investigator_queue` and removes
    /// the id from `loaded_open_ids`.
    ///
    /// The observation `consider` needs is rebuilt from the incident's own
    /// triple — only `(code, subject, observed)` is read (queue identity
    /// plus `take_refused`'s key), so the rebuilt revision and the default
    /// `name`/`transient` never matter. Returns whether anything queued: a
    /// shown button never returns `false`, and the shared predicate is
    /// re-checked here, so a mute flipped between render and press refuses
    /// honestly without consuming the set.
    pub(crate) fn investigate(&mut self, project_index: usize, id: IncidentId) -> bool {
        let (code, subject, observed, resolver_none) = {
            let log = self.projects[project_index]
                .incidents
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some(incident) = log.get(id) else {
                return false;
            };
            (
                incident.code,
                incident.subject,
                incident.observed.clone(),
                incident.telemetry.resolver.is_none(),
            )
        };
        if !resolver_none {
            return false;
        }
        if !self.projects[project_index].loaded_open_ids.contains(&id) {
            return false;
        }
        if !self.investigator_session_eligible(project_index, id, code, subject) {
            return false;
        }
        let revision = self.projects[project_index].revision;
        let observation = IncidentObservation::plain(code, subject, observed, revision);
        self.consider_investigator_queue_for(project_index, id, &observation);
        self.projects[project_index].loaded_open_ids.remove(&id);
        true
    }

    /// Re-investigate (§4.4 rule 19): accept an open or completed investigated
    /// incident when no thread for it is running. A completed investigation is
    /// first returned to `Open`; `begin_investigation` then marks the old
    /// proposal stale before the new session starts. An explicit press bypasses
    /// the mute.
    pub(crate) fn reinvestigate(&mut self, project_index: usize, id: IncidentId) -> bool {
        let (state, code, subject) = {
            let log = self.projects[project_index]
                .incidents
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some(incident) = log.get(id) else {
                return false;
            };
            (incident.state, incident.code, incident.subject)
        };
        if !matches!(state, IncidentState::Open | IncidentState::Investigating) {
            return false;
        }
        let pair_available = self.projects[project_index]
            .investigator
            .as_ref()
            .is_none_or(|session| session.can_reinvestigate(id, code, subject));
        if !pair_available {
            return false;
        }
        if state == IncidentState::Investigating {
            let resolver =
                {
                    let log = self.projects[project_index]
                        .incidents
                        .read()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    log.get(id)
                        .and_then(|incident| incident.telemetry.resolver.as_ref())
                        .and_then(|resolver| match resolver {
                            IncidentResolver::Session { harness, model, .. } => Some(
                                session_resolver(harness, model.as_deref(), "re-investigated"),
                            ),
                            IncidentResolver::Router | IncidentResolver::Person => None,
                        })
                        .unwrap_or_else(|| session_resolver("", None, "re-investigated"))
                };
            let mut log = self.projects[project_index]
                .incidents
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if !log.end_investigation(id, resolver) {
                return false;
            }
        }
        let session = self.projects[project_index]
            .investigator
            .get_or_insert_default();
        let already_queued = session
            .queue
            .iter()
            .any(|queued| queued.id == id && queued.code == code && queued.subject == subject);
        let enqueued = already_queued
            || session.enqueue(QueuedIncident {
                id,
                code,
                subject,
                refused: None,
            });
        self.start_next_investigation(project_index);
        enqueued
    }

    /// Stash the first refused operation beside its observation (IN2 §3.4
    /// rule 22): the batch arms carry a `Vec`, the queue carries one.
    pub(crate) fn stash_batch_refused(
        &mut self,
        project_index: usize,
        observation: &IncidentObservation,
        operations: &[Operation],
    ) {
        if self.may_stash_refused(project_index, observation.code)
            && let Some(op) = operations.first()
        {
            self.projects[project_index]
                .investigator
                .get_or_insert_default()
                .stash_refused(observation.clone(), op.clone());
        }
    }

    /// Retain a refused single operation only while the investigator gate is
    /// live. The incident itself still routes normally when the gate is off.
    pub(crate) fn stash_refused_if_eligible(
        &mut self,
        project_index: usize,
        observation: &IncidentObservation,
        operation: &Operation,
    ) {
        if self.may_stash_refused(project_index, observation.code) {
            self.projects[project_index]
                .investigator
                .get_or_insert_default()
                .stash_refused(observation.clone(), operation.clone());
        }
    }

    /// Mute one code on one project (§2.4 rule 14).
    pub(crate) fn mute_investigator_code(&mut self, project_index: usize, code: IncidentCode) {
        self.projects[project_index]
            .investigator
            .get_or_insert_default()
            .mute_code(code.code());
    }

    /// Remove one mute. Returns whether anything changed.
    pub(crate) fn unmute_investigator_code(&mut self, project_index: usize, code: &str) -> bool {
        self.projects[project_index]
            .investigator
            .get_or_insert_default()
            .unmute_code(code)
    }
}

#[cfg(test)]
mod tests {
    use kinewright_media::test_support::TempDirectory;

    use super::*;

    impl InvestigatorSession {
        /// Number of incidents waiting behind the app-wide session cap.
        pub(crate) fn queued_count(&self) -> usize {
            self.queue.len()
        }

        /// Install a fabricated pending session (C5 item-24 rig): the pump
        /// sees a running session whose result never arrives, so queued
        /// incidents wait instead of starting. The caller holds the result
        /// sender — dropping it reads as `Died`.
        pub(crate) fn install_pending_test_session(&mut self, running: RunningSession) {
            self.running = Some(running);
        }
    }

    fn collect_rust_sources(path: &Path, paths: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(path)
            .expect("the app source directory reads")
            .collect::<Result<Vec<_>, _>>()
            .expect("every source entry reads")
        {
            let path = entry.path();
            if path.is_dir() {
                collect_rust_sources(&path, paths);
            } else if path.extension().and_then(|ext| ext.to_str()) == Some("rs") {
                paths.push(path);
            }
        }
    }

    /// IN2 §9.1 item 36: the app crate has no agent-event loop of its own
    /// and drives every session through one loop call.
    #[test]
    fn in2_the_app_crate_has_no_agent_event_loop_and_one_pump_call() {
        let root = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/src"));
        let mut files = 0_usize;
        let mut truncated = 0_usize;
        let mut sends = 0_usize;
        let mut chat_sends = 0_usize;
        let mut pumps = 0_usize;
        let mut investigator_pumps = 0_usize;
        let mut recovery_waits = 0_usize;
        let mut foreign_waits = 0_usize;
        let mut paths = Vec::new();
        collect_rust_sources(&root, &mut paths);
        paths.sort();
        for path in paths {
            let text = fs::read_to_string(&path).expect("every source file reads");
            // IN1b N6's Windows lesson: match line-ending agnostically.
            let text = text.replace("\r\n", "\n");
            let production = match text.split_once("#[cfg(test)]\nmod tests") {
                Some((production, _)) => {
                    truncated += 1;
                    production
                }
                None => text.as_str(),
            };
            files += 1;
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("");
            let sends_here = production.matches("send_user_message(").count();
            sends += sends_here;
            if name == "chat_ui.rs" {
                chat_sends += sends_here;
            }
            let pumps_here = production.matches("pump_session(").count();
            pumps += pumps_here;
            if name == "investigator.rs" {
                investigator_pumps += pumps_here;
            }
            // `recovery.rs` has test-only helpers before its production
            // recovery worker, so its known waits are counted from the full
            // normalized file. Every other crate file is measured only up to
            // its test module, keeping test drains out of the production-loop
            // assertion.
            let waits_source = if name == "recovery.rs" {
                text.as_str()
            } else {
                production
            };
            let waits_here = waits_source.matches("recv_timeout(").count()
                + waits_source.matches("recv_deadline(").count()
                + waits_source.matches("select!").count()
                + waits_source.matches("select(").count();
            if name == "recovery.rs" {
                recovery_waits += waits_here;
            } else {
                foreign_waits += waits_here;
            }
        }
        assert!(files >= 10, "the test read {files} files, not nothing");
        assert!(truncated >= 1, "at least one file has a test module");
        assert_eq!(sends, 1, "exactly one user-message send in the app crate");
        assert_eq!(
            chat_sends, 1,
            "the one send is the chat composer's existing one"
        );
        assert_eq!(pumps, 1, "exactly one loop call in the app crate");
        assert_eq!(
            investigator_pumps, 1,
            "the one loop call is the session thread's"
        );
        assert!(
            recovery_waits >= 1,
            "recovery.rs keeps its known waits ({recovery_waits}), allowlisted by name"
        );
        assert_eq!(foreign_waits, 0, "no agent-event loop anywhere else");
    }

    /// IN2 §9.1 item 37: the settings file round-trips, defaults off, and
    /// clamps both ends.
    #[test]
    fn in2_the_settings_file_round_trips_and_defaults_off() {
        let temporary = TempDirectory::new("in2-settings-round-trip");
        let path = temporary.path("investigator.json");
        let mut settings = InvestigatorSettings {
            enabled: true,
            harness: Some("codex".to_owned()),
            model: Some("gpt-5".to_owned()),
            effort: Some("high".to_owned()),
            service_tier: Some("priority".to_owned()),
            budgets: InvestigatorBudgets {
                max_turns: 12,
                max_wall_time_seconds: 120,
                max_tokens: 80_000,
            },
            ..InvestigatorSettings::default()
        };
        save_investigator_settings_to(&path, &settings).expect("a settings write succeeds");
        // The write is atomic: no temporary file is left behind.
        assert_eq!(
            fs::read_dir(temporary.root()).unwrap().count(),
            1,
            "one file, no leftover temporary"
        );
        let load = load_investigator_settings_from(&path);
        assert!(load.problem.is_none(), "a clean file loads cleanly");
        settings.version = INVESTIGATOR_SETTINGS_VERSION;
        assert_eq!(load.settings, settings, "write, re-read, equal");
        assert_eq!(load.settings.budgets.max_turns, 12);
        assert_eq!(load.settings.budgets.max_wall_time_seconds, 120);
        assert_eq!(load.settings.budgets.max_tokens, 80_000);

        // A missing file yields the defaults: off, and no incident.
        let missing = temporary.path("missing.json");
        let load = load_investigator_settings_from(&missing);
        assert!(load.problem.is_none(), "a missing file is silent");
        assert!(!load.settings.enabled, "a missing file defaults off");
        assert_eq!(load.settings, InvestigatorSettings::default());

        // An unknown version yields the defaults plus the one problem.
        let versioned = temporary.path("versioned.json");
        fs::write(&versioned, r#"{"version":99,"enabled":true}"#).unwrap();
        let load = load_investigator_settings_from(&versioned);
        assert_eq!(load.problem, Some(SettingsLoadProblem::UnknownVersion(99)));
        assert!(!load.settings.enabled, "an unknown version defaults off");

        // The budget clamps hold at both ends.
        let clamped = temporary.path("clamped.json");
        fs::write(
            &clamped,
            r#"{"version":1,"budgets":{"max_turns":0,"max_wall_time_seconds":99999,"max_tokens":50}}"#,
        )
        .unwrap();
        let load = load_investigator_settings_from(&clamped);
        assert!(load.problem.is_none());
        assert_eq!(load.settings.budgets.max_turns, 1);
        assert_eq!(load.settings.budgets.max_wall_time_seconds, 900);
        assert_eq!(load.settings.budgets.max_tokens, 1_000);
        fs::write(
            &clamped,
            r#"{"version":1,"budgets":{"max_turns":99,"max_wall_time_seconds":1,"max_tokens":99999999}}"#,
        )
        .unwrap();
        let load = load_investigator_settings_from(&clamped);
        assert!(load.problem.is_none());
        assert_eq!(load.settings.budgets.max_turns, 64);
        assert_eq!(load.settings.budgets.max_wall_time_seconds, 5);
        assert_eq!(load.settings.budgets.max_tokens, 2_000_000);
    }

    #[test]
    fn in2_a_non_durable_settings_run_ignores_and_does_not_write_fallback_file() {
        let temporary = TempDirectory::new("in2-settings-non-durable");
        let path = temporary.path("fallback.json");
        let original = br#"{"version":1,"enabled":true}"#;
        fs::write(&path, original).expect("the fallback fixture writes");
        let (_, durable) =
            resolve_investigator_config_path(InvestigatorConfigPlatform::Unix, None, None, None);
        assert!(
            !durable,
            "the injected no-environment resolution is non-durable"
        );
        let loaded = load_investigator_settings_for_run(&path, durable);
        assert!(
            !loaded.settings.enabled,
            "fallback bytes cannot enable this run"
        );
        assert!(loaded.problem.is_none(), "fallback defaults are silent");
        let loaded_settings = loaded.settings.clone();
        let (_fixture, mut app) = crate::app::in1_tests::in1b_app();
        apply_settings_load_for_run(&mut app, loaded, durable);
        assert!(
            !app.focused()
                .investigator
                .as_ref()
                .unwrap()
                .settings
                .enabled
        );
        assert!(
            !app.focused()
                .investigator
                .as_ref()
                .unwrap()
                .settings_durable
        );
        app.update_investigator_settings(|settings| settings.enabled = true);
        app.commit_investigator_settings();
        crate::app::in1_tests::in1_shutdown(&mut app);
        let mut changed = loaded_settings;
        changed.enabled = true;
        save_investigator_settings_for_run(&path, durable, &changed)
            .expect("a non-durable save is an intentional no-op");
        assert_eq!(
            fs::read(&path).unwrap(),
            original,
            "fallback bytes stay untouched"
        );
    }

    #[test]
    fn in2_the_refused_operation_ceiling_counts_serialized_json_bytes() {
        let large = Operation::SetTrackAutomation {
            track: kinewright_core::TrackId(41),
            parameter: "x".repeat(5_000),
            curve: None,
        };
        let serialized = serde_json::to_vec(&large).expect("the operation serializes");
        assert!(
            serialized.len() > REFUSED_OPERATION_CEILING_BYTES,
            "the fixture exceeds the wire-byte ceiling"
        );
        let rendered = render_refused_operation(&large);
        assert!(
            rendered.contains(OPERATION_ELIDED_MARKER),
            "an over-ceiling serialized operation is elided"
        );
        assert!(
            !rendered.contains(&"x".repeat(100)),
            "the long JSON is not copied into the prompt"
        );

        let small = Operation::AddTrack {
            track: kinewright_core::Track {
                id: kinewright_core::TrackId(42),
                kind: kinewright_core::TrackKind::Video,
                sync_lock: true,
                clips: Vec::new(),
            },
        };
        let small_wire = serde_json::to_string(&small).expect("the small operation serializes");
        assert!(
            !small_wire.is_empty(),
            "the under-ceiling wire has positive length"
        );
        assert_eq!(render_refused_operation(&small), small_wire);
    }

    /// IN2 §9.1 item 38: a bad settings file opens one typed agent incident
    /// and is not repaired behind the person's back.
    #[test]
    fn in2_an_unreadable_or_unparseable_settings_file_opens_one_agent_incident_and_is_not_overwritten()
     {
        let temporary = TempDirectory::new("in2-settings-problem");
        let path = temporary.path("investigator.json");
        let original = br#"{"version":"not-a-number","enabled":true}"#;
        fs::write(&path, original).expect("the malformed settings file writes");
        let load = load_investigator_settings_from(&path);
        assert!(
            matches!(load.problem, Some(SettingsLoadProblem::Unparseable(_))),
            "the malformed file is classified as one parse problem"
        );
        assert_eq!(load.settings, InvestigatorSettings::default());

        let (_fixture, mut app) = crate::app::in1_tests::in1b_app();
        apply_settings_load(&mut app, load);
        app.route_incidents();
        let log = app.projects[0].incidents.read().unwrap();
        let agent_incidents = log
            .all()
            .filter(|incident| {
                incident.code == kinewright_core::IncidentCode::Label(LabelIncident::Agent)
                    && incident.subject == IncidentSubject::Agent
            })
            .count();
        assert!(
            agent_incidents > 0,
            "one typed settings incident was opened"
        );
        assert_eq!(agent_incidents, 1, "the load opens one incident");
        drop(log);
        assert_eq!(
            fs::read(&path).unwrap(),
            original,
            "the bad bytes remain intact"
        );

        let unreadable = temporary.path("unreadable");
        fs::create_dir(&unreadable).expect("the unreadable fixture directory creates");
        let unreadable_load = load_investigator_settings_from(&unreadable);
        assert!(
            matches!(
                unreadable_load.problem,
                Some(SettingsLoadProblem::Unreadable(_))
            ),
            "a path that cannot be read as a file is also reported"
        );
        assert!(
            !unreadable_load.settings.enabled,
            "failure still defaults off"
        );
        crate::app::in1_tests::in1_shutdown(&mut app);
    }

    /// IN2 §9.1 item 39: the config path resolves per platform and reports
    /// durability.
    #[test]
    fn in2_the_config_path_resolves_per_platform_and_reports_durability() {
        use std::path::Path;

        let xdg = Path::new("/tmp/in2-xdg");
        let home = Path::new("/tmp/in2-home");
        let appdata = Path::new("C:\\Users\\in2\\AppData\\Roaming");

        // With `XDG_CONFIG_HOME` set: the first of T10's three rows.
        let (path, durable) = resolve_investigator_config_path(
            InvestigatorConfigPlatform::Unix,
            None,
            Some(xdg),
            Some(home),
        );
        assert_eq!(path, xdg.join("Kinewright").join("investigator.json"));
        assert!(durable);

        // Unset-with-`HOME`: the second row.
        let (path, durable) = resolve_investigator_config_path(
            InvestigatorConfigPlatform::Unix,
            None,
            None,
            Some(home),
        );
        assert_eq!(
            path,
            home.join(".config")
                .join("Kinewright")
                .join("investigator.json")
        );
        assert!(durable);

        // Both unset: the fallback, and only it is not durable.
        let (path, durable) =
            resolve_investigator_config_path(InvestigatorConfigPlatform::Unix, None, None, None);
        assert_eq!(
            path,
            std::env::temp_dir()
                .join("Kinewright")
                .join("investigator.json")
        );
        assert!(!durable, "only the fallback is not durable");

        // The Windows row, with and without `%APPDATA%`.
        let (path, durable) = resolve_investigator_config_path(
            InvestigatorConfigPlatform::Windows,
            Some(appdata),
            None,
            None,
        );
        assert_eq!(path, appdata.join("Kinewright").join("investigator.json"));
        assert!(durable);
        let (_, durable) =
            resolve_investigator_config_path(InvestigatorConfigPlatform::Windows, None, None, None);
        assert!(!durable);

        // The macOS row, with and without `$HOME`.
        let (path, durable) = resolve_investigator_config_path(
            InvestigatorConfigPlatform::Macos,
            None,
            None,
            Some(home),
        );
        assert_eq!(
            path,
            home.join("Library")
                .join("Application Support")
                .join("Kinewright")
                .join("investigator.json")
        );
        assert!(durable);
        let (_, durable) =
            resolve_investigator_config_path(InvestigatorConfigPlatform::Macos, None, None, None);
        assert!(!durable);
    }

    /// The once-per-session write-failure gate fires exactly once.
    #[test]
    fn settings_write_failure_reports_once_per_flag() {
        use std::sync::atomic::AtomicBool;

        let flag = AtomicBool::new(false);
        assert!(claim_first_report(&flag));
        assert!(!claim_first_report(&flag));
        assert!(!claim_first_report(&flag));
    }
}
