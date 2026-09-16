//! The session pump: one owner, one loop (IN2 §3.5).
//!
//! **Not feature-gated, deliberately.** `kinewright-app` takes this crate with
//! `default-features = false`, so the loop that enforces a session's turn,
//! wall-time and token budgets cannot live in `eval.rs`: the application would
//! not compile it and would have to grow a second copy. There is exactly one
//! agent-event loop in this crate and it is [`pump_session`]; `collect_session`
//! (`eval.rs`) calls it with an eval-specific [`SessionObserver`], and the
//! application's investigator module calls it from the session thread that owns
//! the [`AgentSession`] (IN2 §3.5 rules 26, 27, 30).
//!
//! The pump owns nothing but the loop. The session, the prompts, the broker,
//! the budgets, the counters and the cancellation flag are all arguments,
//! because the two callers disagree about almost every one of them.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use crossbeam_channel::Receiver;
use kinewright_core::{AgentEvent, AgentSession};

use crate::ConfirmationBroker;

/// How long the pump waits for one event before re-checking its budgets, the
/// broker, the cancellation flag and the observer.
///
/// The same 100 ms tick `collect_session` has always used, kept so the eval
/// harness's timing behaviour does not move across the rewrite (IN2 §5.2
/// rule 4, §9 regression R-B).
const TICK: Duration = Duration::from_millis(100);

/// The literal reason an investigator session's confirmation is rejected with
/// (IN2 §4.6 rule 27).
pub const INVESTIGATOR_CONFIRMATION_REFUSAL: &str =
    "an investigator session may not ask for a confirmation";

/// The reason the pump reports when it stopped because the cancellation flag
/// was set (IN2 §3.5 rule 27).
///
/// The flag carries no reason of its own, and the party that set it — the
/// application's investigator module — already knows which of §3.7 rule 38's
/// rows it is writing, so the app's own reason is the authoritative one and
/// this string is what the pump says when asked (IN2 erratum D-R60).
pub const CANCELLED_BY_OWNER: &str = "the session was cancelled by its owner";

/// The three budgets the pump owns (IN2 §5).
///
/// Turns are also enforced by every production driver, at a boundary each one
/// defines differently (IN2 §5.1 rule 2), so the pump counts its own and is the
/// subject of every budget assertion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionLimits {
    /// User messages the pump will send.
    pub max_turns: u32,
    /// Wall time from the pump's first send.
    pub max_wall_time: Duration,
    /// Provider tokens, or `None` on a harness that reports none — Cursor
    /// constructs no `AgentEvent::Cost` at all, so its sessions run on turns
    /// and time (IN2 §5.3 rule 9).
    pub max_tokens: Option<u64>,
}

/// Which budget stopped the session (IN2 §3.5 rule 28).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BudgetKind {
    /// [`SessionLimits::max_turns`].
    Turns,
    /// [`SessionLimits::max_wall_time`].
    WallTime,
    /// [`SessionLimits::max_tokens`]. A **post-turn** stop: `AgentEvent::Cost`
    /// arrives once per turn completion, so the enforced ceiling is
    /// `max_tokens` plus one turn (IN2 §5.2 rule 6).
    Tokens,
}

/// Why a session was cancelled, when the stop was somebody's decision rather
/// than a budget (IN2 §0.4 k).
///
/// The harness failure lives here because §2.2 rule 8's one-strike disable has
/// to tell a harness that refused to start from a person who switched the
/// investigator off, and a flat `Cancelled` cannot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopReason {
    /// The person switched the investigator off.
    OffSwitch,
    /// The person resolved the incident by hand, or it was resolved elsewhere.
    ResolvedElsewhere,
    /// The harness refused to start or refused a turn (IN2 §2.2 rule 8).
    Harness(String),
    /// The owner asked to stop, with its own reason: either a
    /// [`SessionFlow::Stop`] from the observer, or the cancellation flag, whose
    /// reason is [`CANCELLED_BY_OWNER`].
    Observer(String),
}

/// How a session ended. **Exactly five variants** (IN2 §3.5 rule 28).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionStop {
    /// Every prompt was answered and the prompt source ran out.
    Completed,
    /// One of the three budgets fired.
    Budget(BudgetKind),
    /// Somebody stopped it.
    Cancelled(StopReason),
    /// The agent event stream disconnected.
    Disconnected,
    /// A confirmation was raised inside a session run under
    /// [`ConfirmationPolicy::RejectAndStop`] (IN2 §4.6 rule 27).
    PolicyViolation,
}

/// How a confirmation raised mid-session is answered.
///
/// A parameter and not a convention, because the two callers' policies are
/// opposite: the eval harness approves every request, and an investigator
/// session that raises one has committed a policy violation (IN2 §3.5
/// rule 29d).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmationPolicy {
    /// The eval harness's rule: approve every request and keep going.
    ApproveAll,
    /// The investigator's rule: reject the request and end the session with
    /// [`SessionStop::PolicyViolation`].
    RejectAndStop,
}

/// What the session spent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SessionCounters {
    /// User messages the pump sent and the harness accepted.
    pub turns: u32,
    /// Provider input tokens, accumulated across every `AgentEvent::Cost`.
    pub input_tokens: u64,
    /// Provider output tokens, accumulated the same way.
    pub output_tokens: u64,
    /// Wall time since the pump started.
    pub elapsed: Duration,
    /// `AgentEvent::ToolCall` events observed.
    pub tool_calls: u32,
}

/// The five atomics the user interface reads each frame; the pump is the only
/// writer (IN2 §3.5 rule 27, §5.5 rule 15).
///
/// `elapsed` is stored as whole milliseconds, which is the resolution the card
/// renders (`"18s of 90s"`) and the only field of [`SessionCounters`] that is
/// not already an integer.
#[derive(Debug, Default)]
pub struct SharedCounters {
    turns: AtomicU32,
    input_tokens: AtomicU64,
    output_tokens: AtomicU64,
    elapsed_millis: AtomicU64,
    tool_calls: AtomicU32,
}

impl SharedCounters {
    /// Read every counter, in five relaxed loads.
    ///
    /// Relaxed is the right ordering: the reader is a user-interface frame that
    /// wants the freshest number it can cheaply have, and a frame that reads
    /// turn six's token count beside turn five's turn count is a frame nobody
    /// can tell from the one before it.
    #[must_use]
    pub fn snapshot(&self) -> SessionCounters {
        SessionCounters {
            turns: self.turns.load(Ordering::Relaxed),
            input_tokens: self.input_tokens.load(Ordering::Relaxed),
            output_tokens: self.output_tokens.load(Ordering::Relaxed),
            elapsed: Duration::from_millis(self.elapsed_millis.load(Ordering::Relaxed)),
            tool_calls: self.tool_calls.load(Ordering::Relaxed),
        }
    }

    fn publish(&self, counters: &SessionCounters) {
        self.turns.store(counters.turns, Ordering::Relaxed);
        self.input_tokens
            .store(counters.input_tokens, Ordering::Relaxed);
        self.output_tokens
            .store(counters.output_tokens, Ordering::Relaxed);
        self.elapsed_millis.store(
            u64::try_from(counters.elapsed.as_millis()).unwrap_or(u64::MAX),
            Ordering::Relaxed,
        );
        self.tool_calls
            .store(counters.tool_calls, Ordering::Relaxed);
    }
}

/// One session's cost, accumulated across every `AgentEvent::Cost` it saw
/// (IN2 §5.4 rule 11).
///
/// **`mirror_agent_cost` overwrites**: all six of its writes are straight
/// assignments, and a harness emits one `Cost` per turn completion, so calling
/// it per event would leave the incident's telemetry holding turn six's tokens
/// while [`SessionCounters`] holds the sum — two different numbers on one card.
/// The session therefore accumulates here and mirrors **once**, at the end,
/// through `IncidentLog::telemetry_mut`, which makes the incident's token
/// totals equal the session's counters by construction.
///
/// The optional four stay `None` until a harness reports them, and stop
/// accumulating the moment one turn reports `None` where an earlier turn
/// reported a number — the same rule `collect_session` keeps for `cost_usd`,
/// because a partial sum is a number nobody can act on.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct SessionCost {
    /// How many `AgentEvent::Cost` events were folded in. Zero means the
    /// harness reports none, which is the Cursor shape (IN2 §5.3 rule 9).
    pub events: u32,
    /// Accumulated provider input tokens.
    pub input_tokens: u64,
    /// Accumulated provider output tokens.
    pub output_tokens: u64,
    /// Accumulated cached input tokens, when every reporting turn reported one.
    pub cached_input_tokens: Option<u64>,
    /// Accumulated cache-creation input tokens, on the same rule.
    pub cache_creation_input_tokens: Option<u64>,
    /// Accumulated reasoning output tokens, on the same rule.
    pub reasoning_output_tokens: Option<u64>,
    /// Accumulated dollar cost, on the same rule.
    pub cost_usd: Option<f64>,
    /// Set once a reporting turn reported `None` for a field that had a value,
    /// so a later turn cannot re-open a sum that is already incomplete.
    incomplete: bool,
}

impl SessionCost {
    /// Fold one event in. Any event that is not a `Cost` is ignored.
    pub fn accumulate(&mut self, event: &AgentEvent) {
        let AgentEvent::Cost {
            input_tokens,
            cached_input_tokens,
            cache_creation_input_tokens,
            output_tokens,
            reasoning_output_tokens,
            cost_usd,
        } = event
        else {
            return;
        };
        let first = self.events == 0;
        self.events = self.events.saturating_add(1);
        self.input_tokens = self.input_tokens.saturating_add(*input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(*output_tokens);
        accumulate_optional(&mut self.cached_input_tokens, *cached_input_tokens, first);
        accumulate_optional(
            &mut self.cache_creation_input_tokens,
            *cache_creation_input_tokens,
            first,
        );
        accumulate_optional(
            &mut self.reasoning_output_tokens,
            *reasoning_output_tokens,
            first,
        );
        match (cost_usd, self.incomplete) {
            (Some(cost), false) => *self.cost_usd.get_or_insert(0.0) += *cost,
            (Some(_), true) => {}
            (None, _) => {
                self.incomplete = true;
                self.cost_usd = None;
            }
        }
    }

    /// The whole session as one `AgentEvent::Cost`, to hand to
    /// `mirror_agent_cost` exactly once.
    ///
    /// `None` when the harness reported nothing, which is what makes the card's
    /// *"unknown"* honest rather than a zero (IN2 §5.3 rule 9).
    #[must_use]
    pub const fn as_event(&self) -> Option<AgentEvent> {
        if self.events == 0 {
            return None;
        }
        Some(AgentEvent::Cost {
            input_tokens: self.input_tokens,
            cached_input_tokens: self.cached_input_tokens,
            cache_creation_input_tokens: self.cache_creation_input_tokens,
            output_tokens: self.output_tokens,
            reasoning_output_tokens: self.reasoning_output_tokens,
            cost_usd: self.cost_usd,
        })
    }
}

/// Add `reported` into `total`, or give up on the field for good.
fn accumulate_optional(total: &mut Option<u64>, reported: Option<u64>, first: bool) {
    match (reported, *total) {
        (Some(value), Some(running)) => *total = Some(running.saturating_add(value)),
        (Some(value), None) if first => *total = Some(value),
        // A field that arrived late, or a field one turn did not report: a
        // partial sum is worse than no number.
        (Some(_), None) | (None, _) => *total = None,
    }
}

/// Whether the pump keeps going.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionFlow {
    /// Keep pumping.
    Continue,
    /// Stop, with a reason the caller records as
    /// [`StopReason::Observer`].
    Stop(String),
}

/// Everything the pump deliberately does not own.
///
/// The eval harness's four remaining budgets — `max_operations`,
/// `max_tool_calls`, `max_cost_usd` and its cost bookkeeping — live in its own
/// observer, and the investigator's live in its (IN2 §3.5 rule 30).
pub trait SessionObserver {
    /// Every event, after the pump's own accounting has seen it.
    fn on_event(&mut self, event: &AgentEvent) -> SessionFlow;
    /// Once per tick, with the counters as they stand.
    fn on_tick(&mut self, counters: &SessionCounters) -> SessionFlow;
    /// Reported when a confirmation could not be answered. The eval harness's
    /// *"confirmation N disappeared before approval"* is the one caller
    /// (IN2 §3.5 rule 29).
    fn on_confirmation_error(&mut self, _message: String) {}
}

/// An observer that watches nothing and stops for nothing.
///
/// The shape a caller with no budgets of its own passes, so
/// `Option<&mut dyn SessionObserver>` is not a ninth kind of argument.
#[derive(Debug, Default)]
pub struct QuietObserver;

impl SessionObserver for QuietObserver {
    fn on_event(&mut self, _event: &AgentEvent) -> SessionFlow {
        SessionFlow::Continue
    }

    fn on_tick(&mut self, _counters: &SessionCounters) -> SessionFlow {
        SessionFlow::Continue
    }
}

/// Answer every confirmation waiting on `broker`, and say whether any of them
/// was a policy violation.
///
/// `pending_requests()` **drains**, so every request in a batch has already
/// left the channel and can only be answered here: rejecting the first and
/// stopping would leave a second one to expire on the 5 s broker timeout with
/// nobody to tell. The batch is answered in full, and only then does the
/// violation stop the session (IN2 §4.6 rule 27).
fn answer_confirmations(
    confirmations: Option<&ConfirmationBroker>,
    policy: ConfirmationPolicy,
    observer: &mut dyn SessionObserver,
) -> bool {
    let Some(broker) = confirmations else {
        return false;
    };
    let mut violated = false;
    for request in broker.pending_requests() {
        match policy {
            ConfirmationPolicy::ApproveAll => {
                if !broker.approve(request.id) {
                    observer.on_confirmation_error(format!(
                        "confirmation {} disappeared before approval",
                        request.id
                    ));
                }
            }
            ConfirmationPolicy::RejectAndStop => {
                broker.reject(request.id, INVESTIGATOR_CONFIRMATION_REFUSAL);
                violated = true;
            }
        }
    }
    violated
}

/// Drive one agent session to a stop, enforcing wall time and tokens between
/// events and counting turns at each accepted send.
///
/// **The one agent-event loop in this crate** (IN2 §3.5 rule 30, §9 clause 16).
/// `prompts` is the prompt source: the investigator yields its opening message
/// once and then `None`; the eval harness yields `collect_session`'s prompt
/// list. `cancel` is observed between events, and the pump calls
/// [`AgentSession::interrupt`] itself when it sees it set — which is how the
/// application cancels a session without a second owner of the `&mut`
/// (IN2 §3.5 rule 27).
///
/// Returns the stop and the counters at the moment of it. The counters are
/// also published into `counters` as they move, so a user interface can read
/// them every frame while the session runs.
// IN2 §0.4 o: nine arguments is what N2.5/B2's execution model costs, and the
// two callers disagree about six of them — the prompt source, the broker, the
// policy, the limits, the cancellation flag and the observer. A struct would
// hide exactly the thing a reader of this signature needs to see, in the same
// shape `McpServer::start_configured` already carries.
#[allow(clippy::too_many_arguments)]
pub fn pump_session(
    session: &mut dyn AgentSession,
    events: &Receiver<AgentEvent>,
    prompts: &mut dyn Iterator<Item = String>,
    confirmations: Option<&ConfirmationBroker>,
    policy: ConfirmationPolicy,
    limits: &SessionLimits,
    counters: &Arc<SharedCounters>,
    cancel: &Arc<AtomicBool>,
    observer: &mut dyn SessionObserver,
) -> (SessionStop, SessionCounters) {
    let started = Instant::now();
    let mut spent = SessionCounters::default();
    let mut stop = SessionStop::Completed;
    counters.publish(&spent);
    'prompts: loop {
        if cancel.load(Ordering::Relaxed) {
            stop = SessionStop::Cancelled(StopReason::Observer(CANCELLED_BY_OWNER.to_owned()));
            break;
        }
        let Some(prompt) = prompts.next() else {
            break;
        };
        // IN2 §5.1 rule 3: the pump is the assertion's subject, independently
        // of whichever driver also refuses the send.
        //
        // **Erratum D-R68.** This arm is only reachable when the prompt source
        // yields more than `max_turns` items, which is the eval harness's
        // shape and not the investigator's: the investigator passes
        // `std::iter::once(opening_message)` (§3.5 rule 29a), so for it the
        // effective turn bound is `SessionConfig.max_turns` at the driver
        // (§5.1 rule 1) and `spent.turns` is the **reported** number the card
        // reads, published per driver turn boundary.
        if spent.turns >= limits.max_turns {
            stop = SessionStop::Budget(BudgetKind::Turns);
            break;
        }
        // Between turns as well as between events: a turn that finished just
        // under the bound must not be allowed to start another one. The
        // pre-IN2 `collect_session` checked only inside a turn, so this is
        // strictly tighter and cannot let a session run longer than it did.
        spent.elapsed = started.elapsed();
        if spent.elapsed > limits.max_wall_time {
            stop = SessionStop::Budget(BudgetKind::WallTime);
            break;
        }
        if let Err(error) = session.send_user_message(prompt) {
            stop = SessionStop::Cancelled(StopReason::Harness(error.to_string()));
            break;
        }
        spent.turns = spent.turns.saturating_add(1);
        spent.elapsed = started.elapsed();
        counters.publish(&spent);
        let mut turn_done = false;
        while !turn_done {
            if answer_confirmations(confirmations, policy, observer) {
                stop = SessionStop::PolicyViolation;
                break 'prompts;
            }
            if cancel.load(Ordering::Relaxed) {
                stop = SessionStop::Cancelled(StopReason::Observer(CANCELLED_BY_OWNER.to_owned()));
                break 'prompts;
            }
            spent.elapsed = started.elapsed();
            counters.publish(&spent);
            if spent.elapsed > limits.max_wall_time {
                stop = SessionStop::Budget(BudgetKind::WallTime);
                break 'prompts;
            }
            // IN2 §5.2 rule 6: a post-turn stop. `AgentEvent::Cost` arrives
            // once per turn completion, so the earliest this can fire is after
            // a turn that already exceeded the bound, and `Budget(Tokens)`
            // means `max_tokens + one turn`.
            if let Some(maximum) = limits.max_tokens
                && spent.input_tokens.saturating_add(spent.output_tokens) > maximum
            {
                stop = SessionStop::Budget(BudgetKind::Tokens);
                break 'prompts;
            }
            if let SessionFlow::Stop(reason) = observer.on_tick(&spent) {
                stop = SessionStop::Cancelled(StopReason::Observer(reason));
                break 'prompts;
            }
            match events.recv_timeout(TICK) {
                Ok(event) => {
                    match &event {
                        AgentEvent::ToolCall { .. } => {
                            spent.tool_calls = spent.tool_calls.saturating_add(1);
                        }
                        AgentEvent::Cost {
                            input_tokens,
                            output_tokens,
                            ..
                        } => {
                            spent.input_tokens = spent.input_tokens.saturating_add(*input_tokens);
                            spent.output_tokens =
                                spent.output_tokens.saturating_add(*output_tokens);
                        }
                        AgentEvent::Done => turn_done = true,
                        _ => {}
                    }
                    counters.publish(&spent);
                    if let SessionFlow::Stop(reason) = observer.on_event(&event) {
                        stop = SessionStop::Cancelled(StopReason::Observer(reason));
                        break 'prompts;
                    }
                }
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                    stop = SessionStop::Disconnected;
                    break 'prompts;
                }
            }
        }
    }
    spent.elapsed = started.elapsed();
    counters.publish(&spent);
    // One `interrupt`, on the pump's own thread, on every path out — including
    // the cancellation flag's, which is the whole reason the flag exists rather
    // than a second owner of the `&mut` (IN2 §3.5 rule 27).
    session.interrupt();
    (stop, spent)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;
    use std::thread;

    use kinewright_core::{AgentDriver, AgentError, Core, Document, IncidentId, SessionConfig};
    use kinewright_media::FfmpegMediaEngine;

    use super::*;
    use crate::{McpServer, ScriptedCall, ScriptedCost, ScriptedDriver, ScriptedTurn};

    /// A live `McpServer` on an empty project, so every scripted tool call in
    /// these tests is a real call answered by the real dispatcher.
    fn live_server() -> McpServer {
        let core = Core::spawn(Document::default()).expect("the fixture core starts");
        let media = Arc::new(FfmpegMediaEngine::new().expect("the media engine starts"));
        McpServer::start(core, Arc::clone(&media) as _, media as _).expect("the MCP server starts")
    }

    fn one_call_turn() -> ScriptedTurn {
        ScriptedTurn::new(
            vec![ScriptedCall {
                tool: "get_timeline_state".to_owned(),
                arguments: serde_json::json!({}),
            }],
            "read the cut",
        )
    }

    fn session_for(server: &McpServer, script: Vec<ScriptedTurn>) -> Box<dyn AgentSession> {
        ScriptedDriver::new(script)
            .start_session(SessionConfig {
                mcp_url: Some(server.endpoint().to_owned()),
                ..SessionConfig::default()
            })
            .expect("the scripted session starts")
    }

    /// Drive `script` to a stop under `limits`, with `prompts` user messages.
    fn drive(
        server: &McpServer,
        script: Vec<ScriptedTurn>,
        prompts: usize,
        limits: SessionLimits,
    ) -> (SessionStop, SessionCounters) {
        let mut session = session_for(server, script);
        let events = session.events();
        let mut prompts = (0..prompts).map(|turn| format!("turn {turn}"));
        let counters = Arc::new(SharedCounters::default());
        let cancel = Arc::new(AtomicBool::new(false));
        let mut observer = QuietObserver;
        pump_session(
            session.as_mut(),
            &events,
            &mut prompts,
            None,
            ConfirmationPolicy::RejectAndStop,
            &limits,
            &counters,
            &cancel,
            &mut observer,
        )
    }

    /// An [`AgentSession`] that counts its own interrupts, wrapped around the
    /// real scripted one so the session under test is still the production
    /// type (IN2 §9.1 item 19).
    struct CountingSession {
        inner: Box<dyn AgentSession>,
        interrupts: Arc<AtomicUsize>,
    }

    impl AgentSession for CountingSession {
        fn send_user_message(&mut self, text: String) -> Result<(), AgentError> {
            self.inner.send_user_message(text)
        }

        fn events(&self) -> Receiver<AgentEvent> {
            self.inner.events()
        }

        fn interrupt(&mut self) {
            self.interrupts.fetch_add(1, Ordering::Relaxed);
            self.inner.interrupt();
        }
    }

    /// IN2 §9.1 item 15 and §9 clause 5: three scripted sessions stop at three
    /// **distinct** `BudgetKind` values, each with non-zero counters.
    ///
    /// Scripts S4, S5 and S6. Nothing here asserts any of §5.3 rule 10's three
    /// default values: the clause asserts that each bound *fires*, at whatever
    /// the constant is.
    #[test]
    fn in2_the_pump_stops_at_each_of_its_three_bounds() {
        let server = live_server();

        // S4: max_turns + 1 turns of one call each.
        let (stop, spent) = drive(
            &server,
            vec![one_call_turn(), one_call_turn(), one_call_turn()],
            3,
            SessionLimits {
                max_turns: 2,
                max_wall_time: Duration::from_secs(30),
                max_tokens: None,
            },
        );
        assert_eq!(stop, SessionStop::Budget(BudgetKind::Turns));
        assert_eq!(spent.turns, 2, "the pump sent exactly its budget");
        assert!(spent.tool_calls >= 2, "{spent:?}");

        // S5: one turn whose script sleeps past the bound.
        let (stop, spent) = drive(
            &server,
            vec![one_call_turn().with_pause(Duration::from_millis(900))],
            1,
            SessionLimits {
                max_turns: 4,
                max_wall_time: Duration::from_millis(150),
                max_tokens: None,
            },
        );
        assert_eq!(stop, SessionStop::Budget(BudgetKind::WallTime));
        assert_eq!(spent.turns, 1);
        assert!(spent.elapsed >= Duration::from_millis(150), "{spent:?}");

        // S6: two turns, the second declaring a cost past the bound.
        let costed = |tokens: u64| {
            one_call_turn().with_cost(ScriptedCost {
                input_tokens: tokens,
                output_tokens: tokens,
                cached_input_tokens: None,
                cache_creation_input_tokens: None,
                reasoning_output_tokens: None,
                cost_usd: None,
            })
        };
        let (stop, spent) = drive(
            &server,
            vec![costed(40), costed(400)],
            2,
            SessionLimits {
                max_turns: 8,
                max_wall_time: Duration::from_secs(30),
                max_tokens: Some(200),
            },
        );
        assert_eq!(stop, SessionStop::Budget(BudgetKind::Tokens));
        assert!(spent.turns >= 1, "{spent:?}");
        assert!(spent.elapsed > Duration::ZERO, "{spent:?}");

        server.shutdown();
    }

    /// IN2 §9.1 item 16 and §5.2 rule 6: the token stop is a **post-turn**
    /// stop, so the accumulated total at the stop is greater than the bound.
    ///
    /// `AgentEvent::Cost` arrives once per turn completion, so the earliest a
    /// token bound can fire is after a turn that already exceeded it. A budget
    /// that does not say this is not the number it claims.
    #[test]
    fn in2_the_token_stop_is_a_post_turn_stop() {
        const MAXIMUM: u64 = 100;
        let server = live_server();
        let turn = one_call_turn().with_cost(ScriptedCost {
            input_tokens: 90,
            output_tokens: 90,
            cached_input_tokens: None,
            cache_creation_input_tokens: None,
            reasoning_output_tokens: None,
            cost_usd: None,
        });
        let (stop, spent) = drive(
            &server,
            vec![turn.clone(), turn],
            2,
            SessionLimits {
                max_turns: 8,
                max_wall_time: Duration::from_secs(30),
                max_tokens: Some(MAXIMUM),
            },
        );
        assert_eq!(stop, SessionStop::Budget(BudgetKind::Tokens));
        assert!(
            spent.input_tokens.saturating_add(spent.output_tokens) > MAXIMUM,
            "the enforced ceiling is max_tokens plus one turn: {spent:?}"
        );
        server.shutdown();
    }

    /// Raise one real confirmation on `broker` from another thread and hand
    /// back what the broker answered.
    fn raise_confirmation(broker: &ConfirmationBroker) -> thread::JoinHandle<Result<(), String>> {
        let broker = broker.clone();
        thread::spawn(move || {
            broker.confirm(
                "apply_edit_plan",
                "Plan removes 0 clips and 0 tracks, and 1 bin - approve?".to_owned(),
            )
        })
    }

    fn drive_with_broker(
        server: &McpServer,
        broker: &ConfirmationBroker,
        policy: ConfirmationPolicy,
    ) -> (SessionStop, SessionCounters) {
        let mut session = session_for(
            server,
            vec![one_call_turn().with_pause(Duration::from_millis(400))],
        );
        let events = session.events();
        let mut prompts = std::iter::once("investigate".to_owned());
        let counters = Arc::new(SharedCounters::default());
        let cancel = Arc::new(AtomicBool::new(false));
        let mut observer = QuietObserver;
        pump_session(
            session.as_mut(),
            &events,
            &mut prompts,
            Some(broker),
            policy,
            &SessionLimits {
                max_turns: 4,
                max_wall_time: Duration::from_secs(20),
                max_tokens: None,
            },
            &counters,
            &cancel,
            &mut observer,
        )
    }

    /// IN2 §9.1 item 17, §4.6 rule 27 and §9 clause 14: a confirmation raised
    /// inside an investigator session is a **policy violation**.
    ///
    /// The session's only brokered path is `commit_edit_plan` ->
    /// `apply_edit_plan`, and it fires only for an operation the proposal's
    /// destructive check already refuses — so a request means the session tried
    /// to commit a destructive plan on its branch, caught one step earlier.
    #[test]
    fn in2_a_confirmation_inside_a_session_ends_it_with_a_policy_violation() {
        let server = live_server();
        let incident = IncidentId(7);
        let broker = ConfirmationBroker::for_incident(Duration::from_secs(5), incident);
        assert_eq!(broker.incident(), Some(incident));
        let raised = raise_confirmation(&broker);
        let (stop, spent) = drive_with_broker(&server, &broker, ConfirmationPolicy::RejectAndStop);
        assert_eq!(stop, SessionStop::PolicyViolation);
        assert_eq!(spent.turns, 1, "the session did send its opening message");
        assert_eq!(
            raised.join().unwrap(),
            Err(INVESTIGATOR_CONFIRMATION_REFUSAL.to_owned()),
            "the request is rejected with the contract's literal reason"
        );
        server.shutdown();
    }

    /// IN2 §9.1 item 18 and §3.5 rule 29d: the same request under `ApproveAll`
    /// is approved and the session runs on.
    ///
    /// This is the eval harness's behaviour, and the half a single
    /// `Option<&ConfirmationBroker>` could not express: the policy is a
    /// parameter because the two callers' rules are opposite.
    #[test]
    fn in2_the_pump_under_approve_all_approves_and_continues() {
        let server = live_server();
        let broker = ConfirmationBroker::with_timeout(Duration::from_secs(5));
        assert_eq!(
            broker.incident(),
            None,
            "a chat-panel broker stamps nothing"
        );
        let raised = raise_confirmation(&broker);
        let (stop, spent) = drive_with_broker(&server, &broker, ConfirmationPolicy::ApproveAll);
        assert_eq!(stop, SessionStop::Completed, "the session ran to the end");
        assert_eq!(spent.turns, 1);
        assert!(spent.tool_calls >= 1, "{spent:?}");
        assert_eq!(raised.join().unwrap(), Ok(()));
        server.shutdown();
    }

    /// IN2 §9.1 item 19 and §3.5 rule 27: the cancellation flag is observed
    /// between events and the pump calls `interrupt` **itself**, exactly once,
    /// from its own thread.
    ///
    /// This is how the application cancels a session without a second owner of
    /// the `&mut`.
    #[test]
    fn in2_a_cancelled_pump_interrupts_from_its_own_thread() {
        let server = live_server();
        let interrupts = Arc::new(AtomicUsize::new(0));
        let mut session = CountingSession {
            inner: session_for(
                &server,
                vec![one_call_turn().with_pause(Duration::from_millis(600))],
            ),
            interrupts: Arc::clone(&interrupts),
        };
        let events = session.events();
        let mut prompts = std::iter::once("investigate".to_owned());
        let counters = Arc::new(SharedCounters::default());
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&cancel);
        let canceller = thread::spawn(move || {
            thread::sleep(Duration::from_millis(50));
            flag.store(true, Ordering::Relaxed);
        });
        let mut observer = QuietObserver;
        let (stop, spent) = pump_session(
            &mut session,
            &events,
            &mut prompts,
            None,
            ConfirmationPolicy::RejectAndStop,
            &SessionLimits {
                max_turns: 4,
                max_wall_time: Duration::from_secs(20),
                max_tokens: None,
            },
            &counters,
            &cancel,
            &mut observer,
        );
        canceller.join().unwrap();
        assert!(
            matches!(stop, SessionStop::Cancelled(StopReason::Observer(_))),
            "{stop:?}"
        );
        assert_eq!(
            interrupts.load(Ordering::Relaxed),
            1,
            "the pump interrupts once, on its own thread"
        );
        assert!(spent.elapsed < Duration::from_secs(20), "{spent:?}");
        // The counters the user interface reads are the counters returned.
        assert_eq!(counters.snapshot().turns, spent.turns);
        server.shutdown();
    }

    /// IN2 §9.1 item 20 and §3.5 rule 29b: a disconnected event stream is a
    /// stop the pump names.
    ///
    /// `collect_session`'s *"agent event stream disconnected"* error had
    /// nowhere to come from before this, and now does.
    #[test]
    fn in2_a_disconnected_event_stream_stops_the_pump() {
        let server = live_server();
        let mut session = session_for(&server, vec![one_call_turn()]);
        let (events_tx, events) = crossbeam_channel::unbounded::<AgentEvent>();
        drop(events_tx);
        let mut prompts = std::iter::once("investigate".to_owned());
        let counters = Arc::new(SharedCounters::default());
        let cancel = Arc::new(AtomicBool::new(false));
        let mut observer = QuietObserver;
        let (stop, spent) = pump_session(
            session.as_mut(),
            &events,
            &mut prompts,
            None,
            ConfirmationPolicy::RejectAndStop,
            &SessionLimits {
                max_turns: 4,
                max_wall_time: Duration::from_secs(20),
                max_tokens: None,
            },
            &counters,
            &cancel,
            &mut observer,
        );
        assert_eq!(stop, SessionStop::Disconnected);
        assert_eq!(spent.turns, 1, "the prompt was sent before the stream died");
        server.shutdown();
    }

    /// IN2 §5.4 rule 11 and §9 clause 7: a session's cost accumulates across
    /// turns and is mirrored **once**.
    ///
    /// `mirror_agent_cost` overwrites, and a harness emits one `Cost` per turn,
    /// so mirroring per event would leave the incident holding the last turn's
    /// tokens while the session's counters hold the sum. Accumulating first is
    /// what makes the incident's totals equal the counters by construction.
    #[test]
    fn in2_a_sessions_cost_accumulates_across_turns_and_mirrors_once() {
        let turn = |input: u64, output: u64, reasoning: Option<u64>, dollars: Option<f64>| {
            AgentEvent::Cost {
                input_tokens: input,
                cached_input_tokens: Some(input / 2),
                cache_creation_input_tokens: Some(4),
                output_tokens: output,
                reasoning_output_tokens: reasoning,
                cost_usd: dollars,
            }
        };

        // A harness that reports nothing at all is the Cursor shape.
        let quiet = SessionCost::default();
        assert_eq!(quiet.events, 0);
        assert!(quiet.as_event().is_none(), "unknown, not zero");

        let mut cost = SessionCost::default();
        cost.accumulate(&AgentEvent::Text("ignored".to_owned()));
        cost.accumulate(&turn(100, 10, Some(2), Some(0.01)));
        cost.accumulate(&turn(200, 20, Some(3), Some(0.02)));
        assert_eq!(cost.events, 2);
        let event = cost.as_event().expect("two reporting turns");
        let mut telemetry = kinewright_core::IncidentTelemetry::default();
        crate::mirror_agent_cost(&mut telemetry, &event);
        assert_eq!(telemetry.input_tokens, Some(300));
        assert_eq!(telemetry.output_tokens, Some(30));
        assert_eq!(telemetry.cached_input_tokens, Some(150));
        assert_eq!(telemetry.cache_creation_input_tokens, Some(8));
        assert_eq!(telemetry.reasoning_output_tokens, Some(5));
        assert_eq!(telemetry.cost_usd_millionths, Some(30_000));

        // One turn that reports nothing for a field ends that field's sum, and
        // a later turn cannot re-open it: a partial sum is worse than none.
        let mut partial = SessionCost::default();
        partial.accumulate(&turn(100, 10, Some(2), Some(0.01)));
        partial.accumulate(&turn(200, 20, None, None));
        partial.accumulate(&turn(400, 40, Some(9), Some(0.09)));
        let event = partial.as_event().unwrap();
        let mut telemetry = kinewright_core::IncidentTelemetry::default();
        crate::mirror_agent_cost(&mut telemetry, &event);
        assert_eq!(
            telemetry.input_tokens,
            Some(700),
            "the two totals still add"
        );
        assert_eq!(telemetry.reasoning_output_tokens, None);
        assert_eq!(telemetry.cost_usd_millionths, None);
    }

    /// IN2 §9.1 item 35, §3.5 rule 30 and §9 clause 16: **this crate has
    /// exactly one agent-event loop and it is in `session.rs`.**
    ///
    /// A source-shape test over this crate's own `src` only, reached through
    /// `CARGO_MANIFEST_DIR`, matching the three shapes an event loop can take
    /// rather than a bound name — a test keyed on a variable called `events`
    /// pins nothing. The known non-session sites are allowlisted **by file**
    /// and counted, so the test cannot pass by reading nothing.
    ///
    /// **Each file is read up to its own test module and no further.** The
    /// property is about production loops; a test helper that waits on a
    /// channel with a deadline is not a second owner of the agent event
    /// stream, and the house's `items_after_test_module` rule is what makes
    /// the truncation exact.
    ///
    /// Line endings are normalised before matching, because a Windows checkout
    /// reads these files with CRLF (the `IN1b` lesson).
    #[test]
    fn in2_the_agent_crate_has_one_agent_event_loop() {
        /// Files allowed to hold a `recv_timeout`-shaped wait that is not an
        /// agent-event loop, with the reason each one is not.
        const ALLOWED: [&str; 3] = [
            // The export queue's own worker waits.
            "export_queue.rs",
            // The ACP transport's reply wait.
            "acp.rs",
            // `ConfirmationBroker::confirm`'s decision wait.
            "server.rs",
        ];
        /// Where a file's production code ends, by the house's own rule that
        /// the test module is last.
        const TEST_MODULE: &str = "#[cfg(test)]\nmod tests {";
        let source = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/src"));
        let mut read = 0_usize;
        let mut allowlisted = 0_usize;
        let mut truncated = 0_usize;
        let mut loops = Vec::new();
        let mut pending = vec![source.to_path_buf()];
        while let Some(directory) = pending.pop() {
            for entry in std::fs::read_dir(&directory).expect("the crate's own src is readable") {
                let path = entry.expect("a readable directory entry").path();
                if path.is_dir() {
                    pending.push(path);
                    continue;
                }
                if path.extension().is_none_or(|extension| extension != "rs") {
                    continue;
                }
                read += 1;
                let whole = std::fs::read_to_string(&path)
                    .expect("a readable source file")
                    .replace("\r\n", "\n");
                let text = match whole.find(TEST_MODULE) {
                    Some(index) => {
                        truncated += 1;
                        &whole[..index]
                    }
                    None => &whole[..],
                };
                let waits = text.matches(".recv_timeout(").count()
                    + text.matches(".recv_deadline(").count()
                    + text.matches("crossbeam_channel::select").count();
                if waits == 0 {
                    continue;
                }
                let name = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or_default()
                    .to_owned();
                if ALLOWED.contains(&name.as_str()) {
                    allowlisted += 1;
                } else {
                    loops.push(name);
                }
            }
        }
        assert!(read >= 10, "the walk read only {read} source files");
        assert!(
            truncated > 0,
            "the test-module truncation must bite, or it proves nothing"
        );
        assert!(
            allowlisted > 0,
            "the allowlisted non-session waits must exist, or this test proves nothing"
        );
        assert_eq!(
            loops,
            ["session.rs"],
            "IN2 §3.5 rule 30: the only agent-event loop in this crate is \
             pump_session's. A file named here is either a second production \
             event loop — which is the thing this test exists to refuse — or a \
             production channel wait that is not an agent-event loop at all, \
             in which case add it to ALLOWED above with the reason it is not \
             one. A test helper does not need an entry: each file is read only \
             up to its own `#[cfg(test)] mod tests {{`."
        );
    }
}
