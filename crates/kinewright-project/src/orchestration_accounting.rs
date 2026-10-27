//! Additive S4 accounting contracts for the H1 publisher, never a second store.
//!
//! Rust adaptation of <https://github.com/pingdotgg/t3code> at
//! 5d91ab1117ad51ef1288a94717d85c70e7af09fe, apps/server/src/orchestration-v2/
//! {ProviderEventIngestor,ProjectionStore}.ts. Notice: LICENSES/t3code-MIT.txt.
//! Kinewright adds task-wide allowances, conservative clocks and typed admission.
#![allow(clippy::missing_errors_doc)]

use crate::{
    bundle_records::{ObjectRef, ProjectId, RecordKind, SessionId},
    receipts::CommandId,
};
use serde::{Deserialize, Serialize};
use std::io::Write;

pub const RUN_MILLIS: u64 = 6 * 60 * 60 * 1000;
pub const TASK_SENDS: usize = 80;
pub const TASKS: usize = 128;
pub const ITEMS: usize = 256;
pub const RECORD_BYTES: usize = 64 * 1024 * 1024;
pub const USAGE_EVENTS: usize = 10_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountingRefusal {
    Capacity,
    Invalid,
    Scope,
    ClockUncertain,
    Time,
    Sends,
    Tokens,
    UsageUnavailable,
    Replan,
    PublicationUnknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderSelection {
    pub connector: String,
    pub model: String,
    pub options: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderMetadata {
    pub attempt: SessionId,
    pub requested: ProviderSelection,
    pub resolved: ProviderSelection,
    pub cli_version: String,
}

/// Values retain known partial totals. Completeness is tracked separately.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UsageValues {
    pub input: Option<u64>,
    pub output: Option<u64>,
    pub cache_read: Option<u64>,
    pub cache_write: Option<u64>,
    pub reasoning: Option<u64>,
    pub cost_micros: Option<u64>,
}

impl UsageValues {
    #[must_use]
    pub fn values(&self) -> [Option<u64>; 6] {
        [
            self.input,
            self.output,
            self.cache_read,
            self.cache_write,
            self.reasoning,
            self.cost_micros,
        ]
    }
    #[must_use]
    pub fn from_values(v: [Option<u64>; 6]) -> Self {
        Self {
            input: v[0],
            output: v[1],
            cache_read: v[2],
            cache_write: v[3],
            reasoning: v[4],
            cost_micros: v[5],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageMode {
    Incremental,
    Cumulative,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UsageSample {
    pub attempt: SessionId,
    pub turn: u64,
    pub event: String,
    pub ordinal: u64,
    pub mode: UsageMode,
    pub values: UsageValues,
    pub complete: bool,
    pub input_includes_cache: bool,
    pub output_includes_reasoning: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UsageTotals {
    pub known: UsageValues,
    pub complete: [bool; 6],
    pub token_lower_bound: u64,
    pub tokens_complete: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SendConsumption {
    pub command: CommandId,
    pub attempt: SessionId,
    pub payload: ObjectRef,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepairCount {
    pub delivery: SessionId,
    pub attempt: SessionId,
    pub rounds: u8,
    pub last_failure: String,
    pub repeated: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskAccounting {
    pub task: SessionId,
    pub token_limit: Option<u64>,
    pub sends: Vec<SendConsumption>,
    pub providers: Vec<ProviderMetadata>,
    pub usage: Vec<UsageSample>,
    pub totals: UsageTotals,
    pub repairs: Vec<RepairCount>,
    pub last_failure: Option<String>,
    pub repeated_failures: u8,
    pub wait_reason: Option<String>,
    pub pause: Option<AccountingRefusal>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccountingRecord {
    pub version: u32,
    pub project: ProjectId,
    pub run: SessionId,
    pub previous: Option<ObjectRef>,
    pub revision: u64,
    pub start_unix_ms: u64,
    pub deadline_unix_ms: u64,
    pub last_unix_ms: u64,
    pub elapsed_ms: u64,
    pub clock_uncertain: bool,
    pub durable_event_bytes: u64,
    pub tasks: Vec<TaskAccounting>,
}

/// A bounded encoder includes transient serialized bytes in admission.
pub fn encode<T: Serialize>(value: &T, limit: usize) -> Result<Vec<u8>, AccountingRefusal> {
    struct Output {
        bytes: Vec<u8>,
        limit: usize,
    }
    impl Write for Output {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
                return Err(std::io::Error::other("accounting capacity"));
            }
            self.bytes
                .try_reserve_exact(bytes.len())
                .map_err(std::io::Error::other)?;
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut output = Output {
        bytes: Vec::new(),
        limit,
    };
    serde_json::to_writer(&mut output, value).map_err(|_| AccountingRefusal::Capacity)?;
    Ok(output.bytes)
}

fn reference(reference: &ObjectRef) -> Result<(), AccountingRefusal> {
    if reference.byte_length == 0
        || reference.sha256.len() != 64
        || !reference
            .sha256
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(AccountingRefusal::Invalid);
    }
    Ok(())
}

impl AccountingRecord {
    pub fn new(project: ProjectId, run: SessionId, now: u64) -> Result<Self, AccountingRefusal> {
        Ok(Self {
            version: 1,
            project,
            run,
            previous: None,
            revision: 0,
            start_unix_ms: now,
            deadline_unix_ms: now
                .checked_add(RUN_MILLIS)
                .ok_or(AccountingRefusal::ClockUncertain)?,
            last_unix_ms: now,
            elapsed_ms: 0,
            clock_uncertain: false,
            durable_event_bytes: 0,
            tasks: vec![],
        })
    }

    pub fn dependencies(&self) -> Result<Vec<ObjectRef>, AccountingRefusal> {
        self.validate()?;
        let mut refs = self.previous.iter().cloned().collect::<Vec<_>>();
        for task in &self.tasks {
            refs.extend(task.sends.iter().map(|send| send.payload.clone()));
        }
        refs.sort();
        refs.dedup();
        Ok(refs)
    }

    pub fn validate(&self) -> Result<(), AccountingRefusal> {
        if self.version != 1
            || self.deadline_unix_ms.checked_sub(self.start_unix_ms) != Some(RUN_MILLIS)
            || self.last_unix_ms < self.start_unix_ms
            || self.elapsed_ms < self.last_unix_ms - self.start_unix_ms
            || (self.revision == 0) != self.previous.is_none()
        {
            return Err(AccountingRefusal::Invalid);
        }
        if self.durable_event_bytes > RECORD_BYTES as u64 {
            return Err(AccountingRefusal::Capacity);
        }
        if self.tasks.len() > TASKS {
            return Err(AccountingRefusal::Capacity);
        }
        if let Some(previous) = &self.previous {
            reference(previous)?;
            if previous.record_kind != RecordKind::RunIndex {
                return Err(AccountingRefusal::Invalid);
            }
        }
        for (index, task) in self.tasks.iter().enumerate() {
            if self.tasks[..index].iter().any(|old| old.task == task.task) {
                return Err(AccountingRefusal::Scope);
            }
            if task.sends.len() > TASK_SENDS
                || task.providers.len() > TASK_SENDS
                || task.usage.len() > USAGE_EVENTS
                || task.repairs.len() > ITEMS
            {
                return Err(AccountingRefusal::Capacity);
            }
            if task.repeated_failures > 2
                || task
                    .last_failure
                    .as_ref()
                    .is_some_and(|s| s.len() != 64 || !s.bytes().all(|b| b.is_ascii_hexdigit()))
            {
                return Err(AccountingRefusal::Invalid);
            }
            if task.token_limit == Some(0)
                || task.wait_reason.as_ref().is_some_and(|s| s.len() > 1024)
            {
                return Err(AccountingRefusal::Invalid);
            }
            for (n, send) in task.sends.iter().enumerate() {
                reference(&send.payload)?;
                if send.payload.byte_length > 256 * 1024
                    || send.command.sequence == 0
                    || !task.providers.iter().any(|p| p.attempt == send.attempt)
                    || task.sends[..n].iter().any(|s| s.command == send.command)
                {
                    return Err(AccountingRefusal::Scope);
                }
            }
            for (n, provider) in task.providers.iter().enumerate() {
                if task.providers[..n]
                    .iter()
                    .any(|p| p.attempt == provider.attempt)
                    || provider.cli_version.is_empty()
                    || provider.cli_version.len() > 256
                {
                    return Err(AccountingRefusal::Invalid);
                }
                provider.validate_selection()?;
            }
            for sample in &task.usage {
                if sample.event.is_empty()
                    || sample.event.len() > 256
                    || sample.ordinal == 0
                    || !task.providers.iter().any(|p| p.attempt == sample.attempt)
                {
                    return Err(AccountingRefusal::Invalid);
                }
            }
            if task.totals != normalize(&task.usage)? {
                return Err(AccountingRefusal::Invalid);
            }
            for (n, repair) in task.repairs.iter().enumerate() {
                if repair.rounds > 2
                    || repair.repeated > 2
                    || repair.last_failure.len() != 64
                    || !repair.last_failure.bytes().all(|b| b.is_ascii_hexdigit())
                    || task.repairs[..n]
                        .iter()
                        .any(|r| r.delivery == repair.delivery && r.attempt == repair.attempt)
                {
                    return Err(AccountingRefusal::Invalid);
                }
            }
        }
        encode(self, RECORD_BYTES)?;
        Ok(())
    }

    /// Admission starts from the acknowledged record and retains its exact edge.
    pub fn successor(&self) -> Result<Self, AccountingRefusal> {
        self.validate()?;
        let mut next = self.clone();
        next.previous = Some(self.record_reference()?);
        next.revision = next
            .revision
            .checked_add(1)
            .ok_or(AccountingRefusal::Capacity)?;
        Ok(next)
    }

    pub fn consume_event_bytes(&mut self, bytes: u64) -> Result<(), AccountingRefusal> {
        let next = self
            .durable_event_bytes
            .checked_add(bytes)
            .ok_or(AccountingRefusal::Capacity)?;
        if next > RECORD_BYTES as u64 {
            return Err(AccountingRefusal::Capacity);
        }
        self.durable_event_bytes = next;
        Ok(())
    }

    pub fn validate_successor(&self, previous: &Self) -> Result<(), AccountingRefusal> {
        self.validate()?;
        let expected = previous.successor()?;
        if self.project != previous.project
            || self.run != previous.run
            || self.previous != expected.previous
            || self.revision != expected.revision
            || self.start_unix_ms != previous.start_unix_ms
            || self.deadline_unix_ms != previous.deadline_unix_ms
            || self.last_unix_ms < previous.last_unix_ms
            || self.elapsed_ms < previous.elapsed_ms
            || self.durable_event_bytes < previous.durable_event_bytes
            || (previous.clock_uncertain && !self.clock_uncertain)
        {
            return Err(AccountingRefusal::Scope);
        }
        for old in &previous.tasks {
            let new = self
                .tasks
                .iter()
                .find(|task| task.task == old.task)
                .ok_or(AccountingRefusal::Scope)?;
            if new.token_limit != old.token_limit
                || !new.sends.starts_with(&old.sends)
                || !new.providers.starts_with(&old.providers)
                || !new.usage.starts_with(&old.usage)
                || (old.pause.is_some() && new.pause != old.pause)
            {
                return Err(AccountingRefusal::Scope);
            }
            for repair in &old.repairs {
                if !new.repairs.iter().any(|r| {
                    r.delivery == repair.delivery
                        && r.attempt == repair.attempt
                        && r.rounds >= repair.rounds
                }) {
                    return Err(AccountingRefusal::Scope);
                }
            }
        }
        Ok(())
    }

    /// Canonical bytes of `OrchestrationRecord::Accounting`, including its tag.
    pub fn record_bytes(&self) -> Result<Vec<u8>, AccountingRefusal> {
        #[derive(Serialize)]
        struct Payload<'a> {
            kind: &'static str,
            record: &'a AccountingRecord,
        }
        #[derive(Serialize)]
        struct Record<'a> {
            version: u32,
            payload: Payload<'a>,
        }
        encode(
            &Record {
                version: 1,
                payload: Payload {
                    kind: "accounting",
                    record: self,
                },
            },
            RECORD_BYTES,
        )
    }
    pub fn record_reference(&self) -> Result<ObjectRef, AccountingRefusal> {
        let bytes = self.record_bytes()?;
        Ok(ObjectRef {
            sha256: kinewright_media::sha256_bytes(&bytes),
            byte_length: bytes.len() as u64,
            record_kind: RecordKind::RunIndex,
        })
    }

    /// Both wall time and an owner-measured monotonic delta can only spend time.
    /// On reopen the owner certifies the wall clock; an uncertain clock pauses.
    pub fn observe_clock(
        &mut self,
        now: u64,
        monotonic_delta: u64,
        trusted: bool,
    ) -> Result<(), AccountingRefusal> {
        if !trusted || now < self.last_unix_ms {
            self.clock_uncertain = true;
        }
        self.elapsed_ms = self
            .elapsed_ms
            .checked_add(monotonic_delta)
            .ok_or(AccountingRefusal::Time)?
            .max(now.saturating_sub(self.start_unix_ms));
        self.last_unix_ms = self.last_unix_ms.max(now);
        self.check_clock()
    }

    pub fn check_clock(&self) -> Result<(), AccountingRefusal> {
        if self.clock_uncertain {
            return Err(AccountingRefusal::ClockUncertain);
        }
        if self.elapsed_ms >= RUN_MILLIS || self.last_unix_ms >= self.deadline_unix_ms {
            return Err(AccountingRefusal::Time);
        }
        Ok(())
    }

    pub fn add_task(
        &mut self,
        task: SessionId,
        token_limit: Option<u64>,
    ) -> Result<(), AccountingRefusal> {
        self.check_clock()?;
        if self.tasks.len() >= TASKS {
            return Err(AccountingRefusal::Capacity);
        }
        if token_limit == Some(0) || self.tasks.iter().any(|t| t.task == task) {
            return Err(AccountingRefusal::Invalid);
        }
        self.tasks.push(TaskAccounting {
            task,
            token_limit,
            sends: vec![],
            providers: vec![],
            usage: vec![],
            totals: UsageTotals::default(),
            repairs: vec![],
            last_failure: None,
            repeated_failures: 0,
            wait_reason: None,
            pause: None,
        });
        Ok(())
    }

    pub fn task_mut(&mut self, task: &SessionId) -> Result<&mut TaskAccounting, AccountingRefusal> {
        self.tasks
            .iter_mut()
            .find(|t| t.task == *task)
            .ok_or(AccountingRefusal::Scope)
    }
}

/// Replay the bounded identified samples. Missing categories remain unavailable.
pub fn normalize(samples: &[UsageSample]) -> Result<UsageTotals, AccountingRefusal> {
    use std::collections::{BTreeMap, BTreeSet};
    struct Turn {
        values: [Option<u64>; 6],
        covered: [bool; 6],
        ordinal: u64,
        finished: bool,
        cache_included: bool,
        reasoning_included: bool,
    }
    if samples.len() > USAGE_EVENTS {
        return Err(AccountingRefusal::Capacity);
    }
    let mut identities = BTreeSet::new();
    let mut turns = BTreeMap::new();
    for sample in samples {
        if !identities.insert((&sample.attempt, sample.turn, &sample.event)) {
            return Err(AccountingRefusal::Scope);
        }
        let turn = turns.entry((&sample.attempt, sample.turn)).or_insert(Turn {
            values: [None; 6],
            covered: [true; 6],
            ordinal: 0,
            finished: false,
            cache_included: sample.input_includes_cache,
            reasoning_included: sample.output_includes_reasoning,
        });
        if sample.ordinal <= turn.ordinal
            || sample.input_includes_cache != turn.cache_included
            || sample.output_includes_reasoning != turn.reasoning_included
        {
            return Err(AccountingRefusal::Invalid);
        }
        for (index, value) in sample.values.values().iter().copied().enumerate() {
            if let Some(value) = value {
                let old = turn.values[index].unwrap_or(0);
                turn.values[index] = Some(match sample.mode {
                    UsageMode::Incremental => {
                        old.checked_add(value).ok_or(AccountingRefusal::Capacity)?
                    }
                    UsageMode::Cumulative => {
                        turn.covered[index] |= value >= old;
                        old.max(value)
                    }
                });
            } else {
                turn.covered[index] = false;
            }
        }
        turn.ordinal = sample.ordinal;
        turn.finished = sample.complete;
    }
    let mut known = [None::<u64>; 6];
    let mut complete = [!turns.is_empty(); 6];
    let mut tokens = 0u64;
    let mut tokens_complete = !turns.is_empty();
    for turn in turns.values() {
        for (index, value) in turn.values.iter().enumerate() {
            complete[index] &= turn.finished && turn.covered[index] && value.is_some();
            if let Some(value) = value {
                known[index] = Some(
                    known[index]
                        .unwrap_or(0)
                        .checked_add(*value)
                        .ok_or(AccountingRefusal::Capacity)?,
                );
            }
        }
        for index in [0, 1]
            .into_iter()
            .chain((!turn.cache_included).then_some(2))
            .chain((!turn.cache_included).then_some(3))
            .chain((!turn.reasoning_included).then_some(4))
        {
            tokens_complete &= turn.finished && turn.covered[index] && turn.values[index].is_some();
            tokens = tokens
                .checked_add(turn.values[index].unwrap_or(0))
                .ok_or(AccountingRefusal::Capacity)?;
        }
    }
    Ok(UsageTotals {
        known: UsageValues::from_values(known),
        complete,
        token_lower_bound: tokens,
        tokens_complete,
    })
}

impl TaskAccounting {
    pub fn select_provider(&mut self, provider: ProviderMetadata) -> Result<(), AccountingRefusal> {
        if let Some(old) = self
            .providers
            .iter()
            .find(|p| p.attempt == provider.attempt)
        {
            return if *old == provider {
                Ok(())
            } else {
                Err(AccountingRefusal::Scope)
            };
        }
        if self.providers.len() >= TASK_SENDS {
            return Err(AccountingRefusal::Capacity);
        }
        self.providers.push(provider);
        Ok(())
    }

    pub fn consume_send(&mut self, send: SendConsumption) -> Result<(), AccountingRefusal> {
        if let Some(old) = self.sends.iter().find(|old| old.command == send.command) {
            return if *old == send {
                Ok(())
            } else {
                Err(AccountingRefusal::Scope)
            };
        }
        if let Some(reason) = self.pause {
            return Err(reason);
        }
        if self.wait_reason.is_some() {
            return Err(AccountingRefusal::Replan);
        }
        if self.sends.len() >= TASK_SENDS {
            return Err(AccountingRefusal::Sends);
        }
        if !self.providers.iter().any(|p| p.attempt == send.attempt) {
            return Err(AccountingRefusal::Scope);
        }
        reference(&send.payload)?;
        if send.payload.byte_length > 256 * 1024 {
            return Err(AccountingRefusal::Capacity);
        }
        if self
            .token_limit
            .is_some_and(|cap| self.totals.token_lower_bound >= cap)
        {
            return Err(AccountingRefusal::Tokens);
        }
        if self.token_limit.is_some() && !self.usage.is_empty() && !self.totals.tokens_complete {
            return Err(AccountingRefusal::UsageUnavailable);
        }
        self.sends.push(send);
        Ok(())
    }

    pub fn repair(
        &mut self,
        delivery: &SessionId,
        attempt: &SessionId,
        failure: String,
    ) -> Result<(), AccountingRefusal> {
        if failure.len() != 64 || !failure.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(AccountingRefusal::Invalid);
        }
        if self.pause == Some(AccountingRefusal::Replan) {
            return Err(AccountingRefusal::Replan);
        }
        self.repeated_failures = if self.last_failure.as_ref() == Some(&failure) {
            self.repeated_failures.saturating_add(1).min(2)
        } else {
            1
        };
        self.last_failure = Some(failure.clone());
        if !self
            .repairs
            .iter()
            .any(|r| r.delivery == *delivery && r.attempt == *attempt)
        {
            if self.repairs.len() >= ITEMS {
                return Err(AccountingRefusal::Capacity);
            }
            self.repairs.push(RepairCount {
                delivery: delivery.clone(),
                attempt: attempt.clone(),
                rounds: 0,
                last_failure: String::new(),
                repeated: 0,
            });
        }
        let repair = self
            .repairs
            .iter_mut()
            .find(|r| r.delivery == *delivery && r.attempt == *attempt)
            .ok_or(AccountingRefusal::Invalid)?;
        if repair.rounds >= 2 {
            self.pause = Some(AccountingRefusal::Replan);
            return Err(AccountingRefusal::Replan);
        }
        repair.rounds += 1;
        repair.repeated = if repair.last_failure == failure {
            repair.repeated.saturating_add(1)
        } else {
            1
        };
        repair.last_failure = failure;
        if self.repeated_failures >= 2 {
            self.pause = Some(AccountingRefusal::Replan);
            return Err(AccountingRefusal::Replan);
        }
        Ok(())
    }
}

impl ProviderMetadata {
    fn validate_selection(&self) -> Result<(), AccountingRefusal> {
        for selection in [&self.requested, &self.resolved] {
            if selection.connector.is_empty()
                || selection.connector.len() > 256
                || selection.model.is_empty()
                || selection.model.len() > 256
                || selection.options.len() > 64
                || selection
                    .options
                    .iter()
                    .any(|(k, v)| k.is_empty() || k.len() > 256 || v.len() > 1024)
                || selection
                    .options
                    .iter()
                    .enumerate()
                    .any(|(n, (k, _))| selection.options[..n].iter().any(|(old, _)| old == k))
            {
                return Err(AccountingRefusal::Invalid);
            }
        }
        Ok(())
    }
}
