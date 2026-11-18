//! Shared commands and observations for the project actor.
use crate::{
    BatchError, Clip, ClipId, CoreMutationRefusal, Document, JournalCommand, OpError, Operation,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use thiserror::Error;

/// Monotonic identity for one authoritative runtime timeline state.
///
/// Revisions belong to a running project actor and are deliberately not part
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
// Moving the actor across crates must not change the public tool schema or its
// pinned role manifest. Keep schema prose independent of Rustdoc's local links.
#[schemars(
    description = "Monotonic identity for one authoritative runtime timeline state.\n\nRevisions belong to a running [`Core`] actor and are deliberately not part\nof the serialized project document. Opening a project starts a new lineage\nat revision zero."
)]
pub struct TimelineRevision(pub u64);

impl std::fmt::Display for TimelineRevision {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

/// An in-process correlation id for one revision-gated send (`IN1b` §4 rule 2).
///
/// Never serialised: it exists so a router can match a refusal to the send it
/// refused by identity rather than by `(project, expected)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CommandToken(pub u64);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Do(Operation),
    DoBatch(Vec<Operation>),
    /// Apply one atomic batch and merge it into the newest history entry when
    /// that entry was opened with the same coalesce key.
    ///
    /// Live controls such as a dragged slider send one batch per UI frame so
    /// the preview keeps up. Coalescing keeps the whole gesture as a single
    /// undo step whose undo target stays the document from before the gesture
    /// began; the timeline revision still advances for every accepted batch.
    ///
    /// The key is chosen by the caller and must identify one gesture. Two
    /// separate gestures over the same control must use different keys, so the
    /// second gesture opens its own undo entry.
    DoBatchCoalesced {
        operations: Vec<Operation>,
        coalesce_key: String,
    },
    /// Apply one operation only if the caller planned it against the current revision.
    DoIfRevision {
        expected: TimelineRevision,
        operation: Operation,
        /// Echoed on whichever event this send produces (`IN1b` §4 rule 3).
        /// `None` means "this send does not care".
        token: Option<CommandToken>,
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(clippy::large_enum_variant)]
pub enum QueryResult {
    Document(Arc<Document>),
    Snapshot {
        revision: TimelineRevision,
        document: Arc<Document>,
    },
    Clip(Option<Clip>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(clippy::large_enum_variant)]
pub enum Event {
    MutationRefused {
        reason: CoreMutationRefusal,
        token: Option<CommandToken>,
    },
    DocumentChanged {
        doc: Arc<Document>,
        revision: TimelineRevision,
        last_op: Option<Operation>,
        /// Exact accepted history command. `None` is reserved for the initial
        /// snapshot sent to a new subscriber.
        journal_command: Option<JournalCommand>,
        /// The token of the `DoIfRevision` that landed, when there was one.
        token: Option<CommandToken>,
    },
    OpRejected {
        op: Operation,
        error: OpError,
        /// The token of the `DoIfRevision` whose operation core rejected, when
        /// it carried one (`IN1b` erratum A-R15).
        ///
        /// A revision-gated send can end three ways, and before this field two
        /// of them were correlatable and one was not: the gate refuses
        /// (`RevisionConflict`), the operation lands (`DocumentChanged`), or
        /// the gate passes and `Document::apply` rejects the operation — this
        /// event. A router that matches by identity could therefore never
        /// learn that its send had been answered, and left it outstanding for
        /// the rest of the session.
        token: Option<CommandToken>,
    },
    BatchRejected {
        operations: Vec<Operation>,
        error: BatchError,
    },
    RevisionConflict {
        expected: TimelineRevision,
        actual: TimelineRevision,
        /// The token of the refused send, when it carried one.
        token: Option<CommandToken>,
    },
    QueryResult(QueryResult),
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("the Core actor has stopped")]
pub struct CoreDisconnected;
