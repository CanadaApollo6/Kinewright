use std::{sync::Arc, thread};

use crossbeam_channel::{Receiver, Sender, unbounded};
use thiserror::Error;

use crate::{BatchError, Clip, ClipId, Document, JournalCommand, OpError, Operation, apply_batch};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Do(Operation),
    DoBatch(Vec<Operation>),
    Undo,
    Redo,
    Query(Query),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Query {
    Document,
    Clip(ClipId),
    OpLog,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueryResult {
    Document(Arc<Document>),
    Clip(Option<Clip>),
    OpLog(Arc<Vec<Operation>>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    DocumentChanged {
        doc: Arc<Document>,
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
    pub fn spawn(initial_document: Document) -> Result<Self, OpError> {
        initial_document.validate()?;
        let (sender, receiver) = unbounded();
        thread::Builder::new()
            .name("openreel-core".to_owned())
            .spawn(move || run_actor(receiver, CoreState::new(initial_document)))
            .expect("failed to spawn Core actor");
        Ok(Self { sender })
    }

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
    pub fn request(&self, command: Command) -> Result<Event, CoreDisconnected> {
        let (reply, receiver) = unbounded();
        self.sender
            .send(CoreMessage::Request(command, reply))
            .map_err(|_| CoreDisconnected)?;
        receiver.recv().map_err(|_| CoreDisconnected)
    }

    /// Subscribe to a true broadcast stream. Each subscriber gets every event.
    /// The first event is the current document snapshot.
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
    undo: Vec<HistoryEntry>,
    redo: Vec<HistoryEntry>,
    op_log: Vec<Operation>,
}

impl CoreState {
    fn new(document: Document) -> Self {
        Self {
            document: Arc::new(document),
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
        Ok(Arc::clone(&self.document))
    }

    fn undo(&mut self) -> Arc<Document> {
        if let Some(entry) = self.undo.pop() {
            self.redo.push(HistoryEntry {
                document: Arc::clone(&self.document),
                operations: entry.operations,
            });
            self.document = entry.document;
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
        }
        Arc::clone(&self.document)
    }

    fn query(&self, query: Query) -> QueryResult {
        match query {
            Query::Document => QueryResult::Document(Arc::clone(&self.document)),
            Query::Clip(id) => QueryResult::Clip(self.document.clip(id).cloned()),
            Query::OpLog => QueryResult::OpLog(Arc::new(self.op_log.clone())),
        }
    }
}

fn run_actor(receiver: Receiver<CoreMessage>, mut state: CoreState) {
    let mut subscribers: Vec<Sender<Event>> = Vec::new();
    while let Ok(message) = receiver.recv() {
        match message {
            CoreMessage::Subscribe(subscriber) => {
                if subscriber
                    .send(Event::DocumentChanged {
                        doc: Arc::clone(&state.document),
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
                broadcast(&mut subscribers, event);
            }
            CoreMessage::Request(command, reply) => {
                let event = execute_command(&mut state, command);
                broadcast(&mut subscribers, event.clone());
                let _ = reply.send(event);
            }
        }
    }
}

fn execute_command(state: &mut CoreState, command: Command) -> Event {
    match command {
        Command::Do(operation) => match state.do_operation(operation.clone()) {
            Ok(doc) => Event::DocumentChanged {
                doc,
                last_op: Some(operation.clone()),
                journal_command: Some(JournalCommand::Do(operation)),
            },
            Err(error) => Event::OpRejected {
                op: operation,
                error,
            },
        },
        Command::DoBatch(operations) => match state.do_batch(operations.clone()) {
            Ok(doc) => Event::DocumentChanged {
                doc,
                last_op: None,
                journal_command: Some(JournalCommand::DoBatch(operations)),
            },
            Err(error) => Event::BatchRejected { operations, error },
        },
        Command::Undo => Event::DocumentChanged {
            doc: state.undo(),
            last_op: None,
            journal_command: Some(JournalCommand::Undo),
        },
        Command::Redo => Event::DocumentChanged {
            doc: state.redo(),
            last_op: None,
            journal_command: Some(JournalCommand::Redo),
        },
        Command::Query(query) => Event::QueryResult(state.query(query)),
    }
}

fn broadcast(subscribers: &mut Vec<Sender<Event>>, event: Event) {
    subscribers.retain(|subscriber| subscriber.send(event.clone()).is_ok());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AssetId, Clip, ClipId, Effect, EffectId, MediaAsset, MediaKind, ParamValue, Rational,
        TimeCode, Track, TrackId, TrackKind, Transition,
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
        }
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
                    clips: Vec::new(),
                },
            },
        ];

        core.send(Command::DoBatch(operations.clone())).unwrap();
        let Event::DocumentChanged {
            doc,
            last_op: None,
            journal_command: Some(JournalCommand::DoBatch(journaled)),
        } = events.recv_timeout(Duration::from_secs(1)).unwrap()
        else {
            panic!("expected one batch document change");
        };
        assert_eq!(journaled, operations);
        assert!(doc.asset(AssetId(1)).is_some());
        assert_eq!(doc.tracks.len(), 1);
        assert!(events.try_recv().is_err(), "batch emitted more than one broadcast");

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
                    timeline_start: TimeCode(timeline_start),
                    effects: Vec::new(),
                    transition_in: None,
                };
                timeline_start += length;
                clip
            })
            .collect();
        Document {
            tracks: vec![Track {
                id: TrackId(1),
                kind: TrackKind::Video,
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
            }],
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
