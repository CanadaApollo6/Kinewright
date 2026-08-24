use std::{sync::Arc, thread};

use crossbeam_channel::{Receiver, Sender, unbounded};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{BatchError, Clip, ClipId, Document, JournalCommand, OpError, Operation, apply_batch};

/// Monotonic identity for one authoritative runtime timeline state.
///
/// Revisions belong to a running [`Core`] actor and are deliberately not part
/// of the serialized project document. Opening a project starts a new lineage
/// at revision zero.
#[derive(
    Debug,
    Default,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    JsonSchema,
)]
#[serde(transparent)]
pub struct TimelineRevision(pub u64);

impl std::fmt::Display for TimelineRevision {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Do(Operation),
    DoBatch(Vec<Operation>),
    /// Apply one operation only if the caller planned it against the current revision.
    DoIfRevision {
        expected: TimelineRevision,
        operation: Operation,
    },
    /// Apply one atomic batch only if the caller planned it against the current revision.
    DoBatchIfRevision {
        expected: TimelineRevision,
        operations: Vec<Operation>,
    },
    Undo,
    Redo,
    Query(Query),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Query {
    Document,
    Snapshot,
    Clip(ClipId),
    OpLog,
    /// Operations represented by the current undo stack, excluding undone work.
    AppliedOperations,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueryResult {
    Document(Arc<Document>),
    Snapshot {
        revision: TimelineRevision,
        document: Arc<Document>,
    },
    Clip(Option<Clip>),
    OpLog(Arc<Vec<Operation>>),
    AppliedOperations(Arc<Vec<Operation>>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    DocumentChanged {
        doc: Arc<Document>,
        revision: TimelineRevision,
        last_op: Option<Operation>,
        /// Exact accepted history command. `None` is reserved for the initial
        /// snapshot sent to a new subscriber.
        journal_command: Option<JournalCommand>,
    },
    OpRejected {
        op: Operation,
        error: OpError,
    },
    BatchRejected {
        operations: Vec<Operation>,
        error: BatchError,
    },
    RevisionConflict {
        expected: TimelineRevision,
        actual: TimelineRevision,
    },
    QueryResult(QueryResult),
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("the Core actor has stopped")]
pub struct CoreDisconnected;

enum CoreMessage {
    Command(Command),
    Request(Command, Sender<Event>),
    Subscribe(Sender<Event>),
}

/// Cloneable client handle for the single thread that owns project state.
#[derive(Clone)]
pub struct Core {
    sender: Sender<CoreMessage>,
}

impl Core {
    /// Start the core actor with a validated project document.
    ///
    /// # Errors
    ///
    /// Returns an operation error when the initial document is invalid.
    ///
    /// # Panics
    ///
    /// Panics if the operating system cannot create the actor thread.
    pub fn spawn(initial_document: Document) -> Result<Self, OpError> {
        initial_document.validate()?;
        let (sender, receiver) = unbounded();
        thread::Builder::new()
            .name("kinewright-core".to_owned())
            .spawn(move || run_actor(&receiver, CoreState::new(initial_document)))
            .expect("failed to spawn Core actor");
        Ok(Self { sender })
    }

    /// Queue a command for the core actor.
    ///
    /// # Errors
    ///
    /// Returns [`CoreDisconnected`] if the actor has stopped.
    pub fn send(&self, command: Command) -> Result<(), CoreDisconnected> {
        self.sender
            .send(CoreMessage::Command(command))
            .map_err(|_| CoreDisconnected)
    }

    /// Execute a command through the actor and wait for its authoritative event.
    ///
    /// This is intended for background integrations such as MCP tool calls that
    /// must return the exact outcome of their own command. The command is still
    /// broadcast to every subscriber and uses the same operation and history
    /// path as [`Self::send`].
    ///
    /// # Errors
    ///
    /// Returns [`CoreDisconnected`] if the actor stops before replying.
    pub fn request(&self, command: Command) -> Result<Event, CoreDisconnected> {
        let (reply, receiver) = unbounded();
        self.sender
            .send(CoreMessage::Request(command, reply))
            .map_err(|_| CoreDisconnected)?;
        receiver.recv().map_err(|_| CoreDisconnected)
    }

    /// Subscribe to a true broadcast stream. Each subscriber gets every event.
    /// The first event is the current document snapshot.
    ///
    /// # Errors
    ///
    /// Returns [`CoreDisconnected`] if the actor has stopped.
    pub fn subscribe(&self) -> Result<Receiver<Event>, CoreDisconnected> {
        let (sender, receiver) = unbounded();
        self.sender
            .send(CoreMessage::Subscribe(sender))
            .map_err(|_| CoreDisconnected)?;
        Ok(receiver)
    }
}

#[derive(Clone)]
struct HistoryEntry {
    document: Arc<Document>,
    operations: Vec<Operation>,
}

struct CoreState {
    document: Arc<Document>,
    revision: TimelineRevision,
    undo: Vec<HistoryEntry>,
    redo: Vec<HistoryEntry>,
    op_log: Vec<Operation>,
}

impl CoreState {
    fn new(document: Document) -> Self {
        Self {
            document: Arc::new(document),
            revision: TimelineRevision::default(),
            undo: Vec::new(),
            redo: Vec::new(),
            op_log: Vec::new(),
        }
    }

    fn do_operation(&mut self, operation: Operation) -> Result<Arc<Document>, OpError> {
        let before = Arc::clone(&self.document);
        let mut after = (*before).clone();
        operation.apply(&mut after)?;
        self.undo.push(HistoryEntry {
            document: before,
            operations: vec![operation.clone()],
        });
        self.redo.clear();
        self.op_log.push(operation);
        self.document = Arc::new(after);
        self.increment_revision();
        Ok(Arc::clone(&self.document))
    }

    fn do_batch(&mut self, operations: Vec<Operation>) -> Result<Arc<Document>, BatchError> {
        let before = Arc::clone(&self.document);
        let mut after = (*before).clone();
        apply_batch(&mut after, &operations)?;
        self.undo.push(HistoryEntry {
            document: before,
            operations: operations.clone(),
        });
        self.redo.clear();
        self.op_log.extend(operations);
        self.document = Arc::new(after);
        self.increment_revision();
        Ok(Arc::clone(&self.document))
    }

    fn undo(&mut self) -> Arc<Document> {
        if let Some(entry) = self.undo.pop() {
            self.redo.push(HistoryEntry {
                document: Arc::clone(&self.document),
                operations: entry.operations,
            });
            self.document = entry.document;
            self.increment_revision();
        }
        Arc::clone(&self.document)
    }

    fn redo(&mut self) -> Arc<Document> {
        if let Some(entry) = self.redo.pop() {
            self.undo.push(HistoryEntry {
                document: Arc::clone(&self.document),
                operations: entry.operations,
            });
            self.document = entry.document;
            self.increment_revision();
        }
        Arc::clone(&self.document)
    }

    fn query(&self, query: &Query) -> QueryResult {
        match query {
            Query::Document => QueryResult::Document(Arc::clone(&self.document)),
            Query::Snapshot => QueryResult::Snapshot {
                revision: self.revision,
                document: Arc::clone(&self.document),
            },
            Query::Clip(id) => QueryResult::Clip(self.document.clip(*id).cloned()),
            Query::OpLog => QueryResult::OpLog(Arc::new(self.op_log.clone())),
            Query::AppliedOperations => QueryResult::AppliedOperations(Arc::new(
                self.undo
                    .iter()
                    .flat_map(|entry| entry.operations.iter().cloned())
                    .collect(),
            )),
        }
    }

    fn increment_revision(&mut self) {
        self.revision = TimelineRevision(
            self.revision
                .0
                .checked_add(1)
                .expect("timeline revision overflowed"),
        );
    }
}

fn run_actor(receiver: &Receiver<CoreMessage>, mut state: CoreState) {
    let mut subscribers: Vec<Sender<Event>> = Vec::new();
    while let Ok(message) = receiver.recv() {
        match message {
            CoreMessage::Subscribe(subscriber) => {
                if subscriber
                    .send(Event::DocumentChanged {
                        doc: Arc::clone(&state.document),
                        revision: state.revision,
                        last_op: None,
                        journal_command: None,
                    })
                    .is_ok()
                {
                    subscribers.push(subscriber);
                }
            }
            CoreMessage::Command(command) => {
                let event = execute_command(&mut state, command);
                broadcast(&mut subscribers, &event);
            }
            CoreMessage::Request(command, reply) => {
                let event = execute_command(&mut state, command);
                broadcast(&mut subscribers, &event);
                let _ = reply.send(event);
            }
        }
    }
}

fn execute_command(state: &mut CoreState, command: Command) -> Event {
    match command {
        Command::Do(operation) => execute_operation(state, operation),
        Command::DoBatch(operations) => execute_batch(state, operations),
        Command::DoIfRevision {
            expected,
            operation,
        } => revision_conflict(state, expected)
            .unwrap_or_else(|| execute_operation(state, operation)),
        Command::DoBatchIfRevision {
            expected,
            operations,
        } => revision_conflict(state, expected).unwrap_or_else(|| execute_batch(state, operations)),
        Command::Undo => {
            let doc = state.undo();
            Event::DocumentChanged {
                doc,
                revision: state.revision,
                last_op: None,
                journal_command: Some(JournalCommand::Undo),
            }
        }
        Command::Redo => {
            let doc = state.redo();
            Event::DocumentChanged {
                doc,
                revision: state.revision,
                last_op: None,
                journal_command: Some(JournalCommand::Redo),
            }
        }
        Command::Query(query) => Event::QueryResult(state.query(&query)),
    }
}

fn revision_conflict(state: &CoreState, expected: TimelineRevision) -> Option<Event> {
    (expected != state.revision).then_some(Event::RevisionConflict {
        expected,
        actual: state.revision,
    })
}

fn execute_operation(state: &mut CoreState, operation: Operation) -> Event {
    match state.do_operation(operation.clone()) {
        Ok(doc) => Event::DocumentChanged {
            doc,
            revision: state.revision,
            last_op: Some(operation.clone()),
            journal_command: Some(JournalCommand::Do(operation)),
        },
        Err(error) => Event::OpRejected {
            op: operation,
            error,
        },
    }
}

fn execute_batch(state: &mut CoreState, operations: Vec<Operation>) -> Event {
    match state.do_batch(operations.clone()) {
        Ok(doc) => Event::DocumentChanged {
            doc,
            revision: state.revision,
            last_op: None,
            journal_command: Some(JournalCommand::DoBatch(operations)),
        },
        Err(error) => Event::BatchRejected { operations, error },
    }
}

fn broadcast(subscribers: &mut Vec<Sender<Event>>, event: &Event) {
    subscribers.retain(|subscriber| subscriber.send(event.clone()).is_ok());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AssetId, Clip, ClipContent, ClipId, ColorBitDepth, ColorDescription, ColorMatrix,
        ColorPrimaries, ColorProvenance, ColorRange, ColorTransfer, ColorWhitePoint, Effect,
        EffectId, MediaAsset, MediaKind, ParamValue, Rational, TimeCode, Track, TrackId, TrackKind,
        Transition,
    };
    use proptest::prelude::*;
    use std::{collections::BTreeMap, path::PathBuf, time::Duration};

    fn asset(id: u64) -> MediaAsset {
        MediaAsset {
            id: AssetId(id),
            path: PathBuf::from("fixture.mp4"),
            name: "fixture".to_owned(),
            duration: TimeCode(120),
            fps: Rational::new(30, 1).unwrap(),
            kind: MediaKind::Video,
            resolution: Some((1_920, 1_080)),
            color_description: crate::ColorDescription::default(),
        }
    }

    fn user_color_override() -> ColorDescription {
        ColorDescription {
            primaries: ColorPrimaries::Bt709,
            transfer: ColorTransfer::Bt709,
            matrix: ColorMatrix::Bt709,
            range: ColorRange::Limited,
            white_point: ColorWhitePoint::D65,
            bit_depth: ColorBitDepth::Ten,
            confidence_basis_points: 9_000,
            provenance: ColorProvenance::UserOverride,
        }
    }

    #[test]
    fn color_override_events_journal_and_history_preserve_the_exact_operation() {
        let core = Core::spawn(Document::default()).unwrap();
        core.request(Command::Do(Operation::AddAsset { asset: asset(1) }))
            .unwrap();
        let operation = Operation::SetAssetColorDescription {
            asset: AssetId(1),
            color_description: user_color_override(),
        };

        let Event::DocumentChanged {
            doc,
            revision: TimelineRevision(2),
            last_op: Some(last_op),
            journal_command: Some(JournalCommand::Do(journaled)),
        } = core.request(Command::Do(operation.clone())).unwrap()
        else {
            panic!("expected accepted color override");
        };
        assert_eq!(last_op, operation);
        assert_eq!(journaled, operation);
        assert_eq!(
            doc.asset(AssetId(1)).unwrap().color_description,
            user_color_override()
        );
        let journal_json = serde_json::to_value(JournalCommand::Do(operation.clone())).unwrap();
        assert_eq!(
            journal_json["Do"]["SetAssetColorDescription"]["asset"],
            serde_json::json!(1)
        );

        let Event::DocumentChanged {
            doc,
            revision: TimelineRevision(3),
            journal_command: Some(JournalCommand::Undo),
            ..
        } = core.request(Command::Undo).unwrap()
        else {
            panic!("expected color override undo");
        };
        assert!(
            doc.asset(AssetId(1))
                .unwrap()
                .color_description
                .is_unknown()
        );

        let Event::DocumentChanged {
            doc,
            revision: TimelineRevision(4),
            journal_command: Some(JournalCommand::Redo),
            ..
        } = core.request(Command::Redo).unwrap()
        else {
            panic!("expected color override redo");
        };
        assert_eq!(
            doc.asset(AssetId(1)).unwrap().color_description,
            user_color_override()
        );
    }

    #[test]
    fn actor_broadcasts_changes_and_keeps_an_append_only_log() {
        let core = Core::spawn(Document::default()).unwrap();
        let events = core.subscribe().unwrap();
        assert!(matches!(
            events.recv_timeout(Duration::from_secs(1)).unwrap(),
            Event::DocumentChanged { last_op: None, .. }
        ));

        let operation = Operation::AddAsset { asset: asset(1) };
        core.send(Command::Do(operation.clone())).unwrap();
        assert!(matches!(
            events.recv_timeout(Duration::from_secs(1)).unwrap(),
            Event::DocumentChanged { last_op: Some(op), .. } if op == operation
        ));

        core.send(Command::Undo).unwrap();
        let Event::DocumentChanged { doc, .. } =
            events.recv_timeout(Duration::from_secs(1)).unwrap()
        else {
            panic!("expected document change");
        };
        assert!(doc.media_pool.is_empty());

        core.send(Command::Query(Query::OpLog)).unwrap();
        let Event::QueryResult(QueryResult::OpLog(log)) =
            events.recv_timeout(Duration::from_secs(1)).unwrap()
        else {
            panic!("expected op log");
        };
        assert_eq!(&*log, &[operation]);
    }

    #[test]
    fn rejected_operation_does_not_change_state_or_log() {
        let core = Core::spawn(Document::default()).unwrap();
        let events = core.subscribe().unwrap();
        let _initial = events.recv_timeout(Duration::from_secs(1)).unwrap();
        let operation = Operation::AddAsset { asset: asset(1) };
        core.send(Command::Do(operation.clone())).unwrap();
        let _accepted = events.recv_timeout(Duration::from_secs(1)).unwrap();
        core.send(Command::Do(operation.clone())).unwrap();
        assert!(matches!(
            events.recv_timeout(Duration::from_secs(1)).unwrap(),
            Event::OpRejected { op, error: OpError::DuplicateAsset(AssetId(1)) } if op == operation
        ));
        core.send(Command::Query(Query::OpLog)).unwrap();
        let Event::QueryResult(QueryResult::OpLog(log)) =
            events.recv_timeout(Duration::from_secs(1)).unwrap()
        else {
            panic!("expected op log");
        };
        assert_eq!(log.len(), 1);
    }

    #[test]
    fn request_returns_the_same_authoritative_event_that_subscribers_receive() {
        let core = Core::spawn(Document::default()).unwrap();
        let events = core.subscribe().unwrap();
        let _initial = events.recv_timeout(Duration::from_secs(1)).unwrap();
        let operation = Operation::AddAsset { asset: asset(1) };

        let reply = core.request(Command::Do(operation.clone())).unwrap();
        let broadcast = events.recv_timeout(Duration::from_secs(1)).unwrap();

        assert_eq!(reply, broadcast);
        assert!(matches!(
            reply,
            Event::DocumentChanged { doc, last_op: Some(op), .. }
                if op == operation && doc.asset(AssetId(1)).is_some()
        ));
    }

    #[test]
    fn revision_preconditions_reject_stale_work_without_touching_history() {
        let core = Core::spawn(Document::default()).unwrap();
        let Event::QueryResult(QueryResult::Snapshot {
            revision: initial_revision,
            document: initial_document,
        }) = core.request(Command::Query(Query::Snapshot)).unwrap()
        else {
            panic!("expected revisioned snapshot");
        };
        assert_eq!(initial_revision, TimelineRevision(0));

        let accepted = core
            .request(Command::DoIfRevision {
                expected: initial_revision,
                operation: Operation::AddAsset { asset: asset(1) },
            })
            .unwrap();
        assert!(matches!(
            accepted,
            Event::DocumentChanged {
                revision: TimelineRevision(1),
                ..
            }
        ));

        let stale = core
            .request(Command::DoBatchIfRevision {
                expected: initial_revision,
                operations: vec![Operation::AddAsset { asset: asset(2) }],
            })
            .unwrap();
        assert_eq!(
            stale,
            Event::RevisionConflict {
                expected: TimelineRevision(0),
                actual: TimelineRevision(1),
            }
        );

        let Event::QueryResult(QueryResult::Snapshot { revision, document }) =
            core.request(Command::Query(Query::Snapshot)).unwrap()
        else {
            panic!("expected revisioned snapshot");
        };
        assert_eq!(revision, TimelineRevision(1));
        assert_eq!(&*initial_document, &Document::default());
        assert!(document.asset(AssetId(1)).is_some());
        assert!(document.asset(AssetId(2)).is_none());

        let Event::QueryResult(QueryResult::OpLog(log)) =
            core.request(Command::Query(Query::OpLog)).unwrap()
        else {
            panic!("expected operation log");
        };
        assert_eq!(log.len(), 1);
    }

    #[test]
    fn accepted_batches_and_real_history_moves_increment_revision_once() {
        let core = Core::spawn(Document::default()).unwrap();
        let accepted = core
            .request(Command::DoBatchIfRevision {
                expected: TimelineRevision(0),
                operations: vec![
                    Operation::AddAsset { asset: asset(1) },
                    Operation::AddTrack {
                        track: Track {
                            id: TrackId(1),
                            kind: TrackKind::Video,
                            sync_lock: true,
                            clips: Vec::new(),
                        },
                    },
                ],
            })
            .unwrap();
        assert!(matches!(
            accepted,
            Event::DocumentChanged {
                revision: TimelineRevision(1),
                ..
            }
        ));

        let undone = core.request(Command::Undo).unwrap();
        assert!(matches!(
            undone,
            Event::DocumentChanged {
                revision: TimelineRevision(2),
                ref doc,
                ..
            } if **doc == Document::default()
        ));

        let no_op_undo = core.request(Command::Undo).unwrap();
        assert!(matches!(
            no_op_undo,
            Event::DocumentChanged {
                revision: TimelineRevision(2),
                journal_command: Some(JournalCommand::Undo),
                ..
            }
        ));
    }

    #[test]
    fn applied_operations_follow_undo_and_redo_instead_of_the_append_only_log() {
        let core = Core::spawn(Document::default()).unwrap();
        let first = Operation::AddAsset { asset: asset(1) };
        let second = Operation::AddTrack {
            track: Track {
                id: TrackId(1),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: Vec::new(),
            },
        };
        core.request(Command::Do(first.clone())).unwrap();
        core.request(Command::Do(second.clone())).unwrap();
        core.request(Command::Undo).unwrap();

        let Event::QueryResult(QueryResult::AppliedOperations(applied)) = core
            .request(Command::Query(Query::AppliedOperations))
            .unwrap()
        else {
            panic!("expected applied operation query");
        };
        assert_eq!(&*applied, std::slice::from_ref(&first));
        let Event::QueryResult(QueryResult::OpLog(log)) =
            core.request(Command::Query(Query::OpLog)).unwrap()
        else {
            panic!("expected append-only log query");
        };
        assert_eq!(&*log, &[first.clone(), second.clone()]);

        core.request(Command::Redo).unwrap();
        let Event::QueryResult(QueryResult::AppliedOperations(applied)) = core
            .request(Command::Query(Query::AppliedOperations))
            .unwrap()
        else {
            panic!("expected applied operation query");
        };
        assert_eq!(&*applied, &[first, second]);
    }

    #[test]
    fn batch_broadcasts_once_appends_each_op_and_undoes_as_one_snapshot() {
        let core = Core::spawn(Document::default()).unwrap();
        let events = core.subscribe().unwrap();
        let _initial = events.recv_timeout(Duration::from_secs(1)).unwrap();
        let operations = vec![
            Operation::AddAsset { asset: asset(1) },
            Operation::AddTrack {
                track: Track {
                    id: TrackId(1),
                    kind: TrackKind::Video,
                    sync_lock: true,
                    clips: Vec::new(),
                },
            },
        ];

        core.send(Command::DoBatch(operations.clone())).unwrap();
        let Event::DocumentChanged {
            doc,
            revision: TimelineRevision(1),
            last_op: None,
            journal_command: Some(JournalCommand::DoBatch(journaled)),
        } = events.recv_timeout(Duration::from_secs(1)).unwrap()
        else {
            panic!("expected one batch document change");
        };
        assert_eq!(journaled, operations);
        assert!(doc.asset(AssetId(1)).is_some());
        assert_eq!(doc.tracks.len(), 1);
        assert!(
            events.try_recv().is_err(),
            "batch emitted more than one broadcast"
        );

        core.send(Command::Query(Query::OpLog)).unwrap();
        let Event::QueryResult(QueryResult::OpLog(log)) =
            events.recv_timeout(Duration::from_secs(1)).unwrap()
        else {
            panic!("expected op log");
        };
        assert_eq!(&*log, &operations);

        core.send(Command::Undo).unwrap();
        let Event::DocumentChanged { doc, .. } =
            events.recv_timeout(Duration::from_secs(1)).unwrap()
        else {
            panic!("expected undo document change");
        };
        assert_eq!(&*doc, &Document::default());
    }

    #[test]
    fn rejected_batch_is_atomic_and_reports_the_failed_operation() {
        let mut state = CoreState::new(Document::default());
        let before = Arc::clone(&state.document);
        let operations = vec![
            Operation::AddAsset { asset: asset(1) },
            Operation::AddAsset { asset: asset(1) },
            Operation::AddAsset { asset: asset(2) },
        ];

        assert_eq!(
            state.do_batch(operations),
            Err(BatchError::OperationFailed {
                op_number: 2,
                error: OpError::DuplicateAsset(AssetId(1)),
            })
        );
        assert_eq!(&*state.document, &*before);
        assert!(state.undo.is_empty());
        assert!(state.redo.is_empty());
        assert!(state.op_log.is_empty());
    }

    fn generated_document(lengths: &[u16], gaps: &[u8]) -> Document {
        let mut timeline_start = 0_i64;
        let clips = lengths
            .iter()
            .enumerate()
            .map(|(index, length)| {
                timeline_start += i64::from(gaps[index % gaps.len()]);
                let length = i64::from(*length);
                let clip = Clip {
                    id: ClipId(index as u64 + 1),
                    asset: AssetId(1),
                    source_range: TimeCode(0)..TimeCode(length),
                    content: ClipContent::Media,
                    timeline_start: TimeCode(timeline_start),
                    effects: Vec::new(),
                    transition_in: None,
                    link: None,
                    audio_gain_tenth_db: 0,
                    audio_fade_in_frames: TimeCode::ZERO,
                    audio_fade_out_frames: TimeCode::ZERO,
                    speed_percent: 100,
                };
                timeline_start += length;
                clip
            })
            .collect();
        Document {
            catalog: crate::MediaCatalog::default(),
            audio_mix: crate::AudioMix::default(),
            color_context: crate::ColorContext::default(),
            tracks: vec![Track {
                id: TrackId(1),
                kind: TrackKind::Video,
                sync_lock: true,
                clips,
            }],
            media_pool: vec![MediaAsset {
                id: AssetId(1),
                path: PathBuf::from("generated.mp4"),
                name: "generated".to_owned(),
                duration: TimeCode(300),
                fps: Rational::new(30, 1).unwrap(),
                kind: MediaKind::Video,
                resolution: Some((1_920, 1_080)),
                color_description: crate::ColorDescription::default(),
            }],
            markers: Vec::new(),
            fps: Rational::new(30, 1).unwrap(),
            resolution: (1_920, 1_080),
            duration: TimeCode(timeline_start),
        }
    }

    proptest! {
        #[test]
        fn batch_matches_sequential_application(
            kinds in prop::collection::vec(any::<bool>(), 1..24),
        ) {
            let mut next_asset = 1_u64;
            let mut next_track = 1_u64;
            let operations = kinds
                .into_iter()
                .map(|is_asset| {
                    if is_asset {
                        let operation = Operation::AddAsset { asset: asset(next_asset) };
                        next_asset += 1;
                        operation
                    } else {
                        let operation = Operation::AddTrack {
                            track: Track {
                                id: TrackId(next_track),
                                kind: TrackKind::Video,
                                sync_lock: true,
                                clips: Vec::new(),
                            },
                        };
                        next_track += 1;
                        operation
                    }
                })
                .collect::<Vec<_>>();
            let mut sequential = Document::default();
            for operation in &operations {
                operation.apply(&mut sequential).unwrap();
            }
            let mut batched = Document::default();
            apply_batch(&mut batched, &operations).unwrap();
            prop_assert_eq!(batched, sequential);
        }

        #[test]
        fn batch_rejection_never_exposes_a_valid_prefix(
            prefix_length in 1_usize..24,
        ) {
            let mut operations = (1..=prefix_length)
                .map(|id| Operation::AddAsset { asset: asset(id as u64) })
                .collect::<Vec<_>>();
            operations.push(Operation::AddAsset { asset: asset(1) });
            operations.push(Operation::AddAsset { asset: asset(999) });
            let mut document = Document::default();
            let before = document.clone();
            let error = apply_batch(&mut document, &operations).unwrap_err();
            prop_assert_eq!(
                error,
                BatchError::OperationFailed {
                    op_number: prefix_length + 1,
                    error: OpError::DuplicateAsset(AssetId(1)),
                }
            );
            prop_assert_eq!(document, before);
        }

        #[test]
        fn generated_documents_survive_arbitrary_do_undo_redo_sequences(
            lengths in prop::collection::vec(1_u16..80, 1..8),
            gaps in prop::collection::vec(0_u8..8, 1..8),
            actions in prop::collection::vec(0_u8..10, 0..100),
        ) {
            let initial = generated_document(&lengths, &gaps);
            prop_assert!(initial.validate().is_ok());
            let mut state = CoreState::new(initial.clone());
            let mut expected = Arc::new(initial);
            let mut expected_undo = Vec::new();
            let mut expected_redo = Vec::new();
            let mut next_asset_id = 2_u64;
            let mut next_track_id = 2_u64;
            let mut next_effect_id = 1_u64;
            let mut successful_dos = 0_usize;

            for action in actions {
                match action {
                    0 => {
                        let operation = Operation::AddAsset { asset: asset(next_asset_id) };
                        next_asset_id += 1;
                        expected_undo.push(Arc::clone(&expected));
                        expected_redo.clear();
                        let mut after = (*expected).clone();
                        operation.apply(&mut after).unwrap();
                        expected = Arc::new(after);
                        state.do_operation(operation).unwrap();
                        successful_dos += 1;
                    }
                    1 => {
                        let operation = Operation::AddTrack {
                            track: Track {
                                id: TrackId(next_track_id),
                                kind: TrackKind::Video,
                                sync_lock: true,
                                clips: Vec::new(),
                            },
                        };
                        next_track_id += 1;
                        expected_undo.push(Arc::clone(&expected));
                        expected_redo.clear();
                        let mut after = (*expected).clone();
                        operation.apply(&mut after).unwrap();
                        expected = Arc::new(after);
                        state.do_operation(operation).unwrap();
                        successful_dos += 1;
                    }
                    2 => {
                        if let Some(track) = expected.tracks.iter().rev().find(|track| track.clips.is_empty()) {
                            let operation = Operation::RemoveTrack { track: track.id };
                            expected_undo.push(Arc::clone(&expected));
                            expected_redo.clear();
                            let mut after = (*expected).clone();
                            operation.apply(&mut after).unwrap();
                            expected = Arc::new(after);
                            state.do_operation(operation).unwrap();
                            successful_dos += 1;
                        }
                    }
                    3 => {
                        let operation = Operation::AddEffect {
                            clip: ClipId(1),
                            effect: Effect {
                                id: EffectId(next_effect_id),
                                name: "brightness".to_owned(),
                                parameters: BTreeMap::new(),
                                keyframes: BTreeMap::new(),
                            },
                        };
                        next_effect_id += 1;
                        expected_undo.push(Arc::clone(&expected));
                        expected_redo.clear();
                        let mut after = (*expected).clone();
                        operation.apply(&mut after).unwrap();
                        expected = Arc::new(after);
                        state.do_operation(operation).unwrap();
                        successful_dos += 1;
                    }
                    4 => {
                        if let Some(effect) = expected.clip(ClipId(1)).and_then(|clip| clip.effects.first()) {
                            let operation = Operation::SetEffectParam {
                                clip: ClipId(1),
                                effect: effect.id,
                                name: "percent".to_owned(),
                                value: ParamValue::Integer(25),
                            };
                            expected_undo.push(Arc::clone(&expected));
                            expected_redo.clear();
                            let mut after = (*expected).clone();
                            operation.apply(&mut after).unwrap();
                            expected = Arc::new(after);
                            state.do_operation(operation).unwrap();
                            successful_dos += 1;
                        }
                    }
                    5 => {
                        if let Some(effect) = expected.clip(ClipId(1)).and_then(|clip| clip.effects.first()) {
                            let operation = Operation::RemoveEffect {
                                clip: ClipId(1),
                                effect: effect.id,
                            };
                            expected_undo.push(Arc::clone(&expected));
                            expected_redo.clear();
                            let mut after = (*expected).clone();
                            operation.apply(&mut after).unwrap();
                            expected = Arc::new(after);
                            state.do_operation(operation).unwrap();
                            successful_dos += 1;
                        }
                    }
                    6 => {
                        if expected.clip(ClipId(1)).is_some_and(|clip| clip.transition_in.is_none()) {
                            let operation = Operation::AddTransition {
                                clip: ClipId(1),
                                transition: Transition {
                                    name: "crossfade".to_owned(),
                                    duration: TimeCode(1),
                                },
                            };
                            expected_undo.push(Arc::clone(&expected));
                            expected_redo.clear();
                            let mut after = (*expected).clone();
                            operation.apply(&mut after).unwrap();
                            expected = Arc::new(after);
                            state.do_operation(operation).unwrap();
                            successful_dos += 1;
                        }
                    }
                    7 => {
                        if expected.clip(ClipId(1)).is_some_and(|clip| clip.transition_in.is_some()) {
                            let operation = Operation::RemoveTransition { clip: ClipId(1) };
                            expected_undo.push(Arc::clone(&expected));
                            expected_redo.clear();
                            let mut after = (*expected).clone();
                            operation.apply(&mut after).unwrap();
                            expected = Arc::new(after);
                            state.do_operation(operation).unwrap();
                            successful_dos += 1;
                        }
                    }
                    8 => {
                        if let Some(previous) = expected_undo.pop() {
                            expected_redo.push(Arc::clone(&expected));
                            expected = previous;
                        }
                        state.undo();
                    }
                    _ => {
                        if let Some(next) = expected_redo.pop() {
                            expected_undo.push(Arc::clone(&expected));
                            expected = next;
                        }
                        state.redo();
                    }
                }
                prop_assert_eq!(&*state.document, &*expected);
                prop_assert!(state.document.validate().is_ok());
                prop_assert_eq!(state.op_log.len(), successful_dos);
            }
        }
    }
}
