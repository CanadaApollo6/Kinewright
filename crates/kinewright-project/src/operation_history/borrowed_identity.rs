//! One canonical owner view for retained captures and borrowed live state.
use super::{
    CommandOrigin, CreationSizing, Error, Lineage, OwnerHistory, Resource, Resources, Role,
    SnapshotIdentity,
    ledger::{ApprovalLedger, FoldedApprovals},
    owner::Origins,
    snapshot::SnapshotData,
    transition::TerminalFacts,
};
use kinewright_core::undo::CommandKey;
use serde::{Serialize, Serializer, ser::SerializeSeq};
use std::collections::BTreeSet;

#[derive(Serialize)]
struct View<'a> {
    role: Role,
    sizing: CreationSizing,
    fold_frontier: Option<CommandKey>,
    #[serde(with = "super::transition::terminal_wire")]
    terminal_facts: &'a TerminalFacts,
    seeds: SeedValues<'a>,
    origins: OriginValues<'a>,
    resources: ResourceValues<'a>,
    ledger: &'a ApprovalLedger,
    #[serde(with = "super::ledger::folded_wire")]
    folded_approvals: &'a FoldedApprovals,
}
enum SeedValues<'a> {
    Captured(&'a Lineage),
    Live(&'a OwnerHistory),
}
impl Serialize for SeedValues<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Captured(seeds) => seeds.serialize(serializer),
            Self::Live(owner) => {
                use serde::ser::SerializeStruct;
                let Some(fold) = &owner.fold else {
                    return owner.seeds.serialize(serializer);
                };
                super::owner::lineage_encodings(1);
                let mut value = serializer.serialize_struct("Wire", 2)?;
                value.serialize_field("occurrences", &owner.seeds.occurrences)?;
                value.serialize_field("split_ancestry", &fold.ordered_ancestry(&owner.seeds))?;
                value.end()
            }
        }
    }
}
enum OriginValues<'a> {
    Live(&'a Origins),
    Captured(&'a [CommandOrigin]),
}
impl Serialize for OriginValues<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Captured(values) => values.serialize(serializer),
            Self::Live(values) => {
                let mut sequence = serializer.serialize_seq(Some(values.0.len()))?;
                for value in &values.0 {
                    sequence.serialize_element(&value.origin)?;
                }
                sequence.end()
            }
        }
    }
}
enum ResourceValues<'a> {
    Live(&'a Resources),
    Captured(&'a BTreeSet<Resource>),
}
impl Serialize for ResourceValues<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Captured(values) => values.serialize(serializer),
            Self::Live(values) => {
                let mut sequence = serializer.serialize_seq(None)?;
                for value in values.values() {
                    sequence.serialize_element(value)?;
                }
                sequence.end()
            }
        }
    }
}
pub(super) fn captured(data: &SnapshotData) -> Result<[u8; 32], Error> {
    super::identity::digest(&View {
        role: data.role,
        sizing: data.sizing,
        fold_frontier: data.fold_frontier,
        terminal_facts: &data.terminal_facts,
        seeds: SeedValues::Captured(&data.seeds),
        origins: OriginValues::Captured(&data.origins),
        resources: ResourceValues::Captured(&data.resources),
        ledger: &data.ledger,
        folded_approvals: &data.folded_approvals,
    })
}
impl OwnerHistory {
    /// Save/install identity of a borrowed state. It creates no retained capture
    /// and never runs on an ordinary command path.
    pub fn identity(&self) -> Result<SnapshotIdentity, Error> {
        let _workspace = self.reservation.workspace()?;
        Ok(SnapshotIdentity {
            core: self.core.snapshot_identity()?,
            owner: super::identity::digest(&View {
                role: self.reservation.role(),
                sizing: self.reservation.sizing(),
                fold_frontier: self.fold_frontier,
                terminal_facts: &self.terminal_facts,
                seeds: SeedValues::Live(self),
                origins: OriginValues::Live(&self.origins),
                resources: ResourceValues::Live(&self.resources),
                ledger: &self.ledger,
                folded_approvals: &self.folded_approvals,
            })?,
        })
    }
}
