#[path = "../../kinewright-core/tests/support/history_oracle.rs"]
mod history_oracle;

use kinewright_project::{
    bundle_records::{BatchId, BranchId},
    operation_actor::{Core, Error},
    operation_history::{CreationOrigin, HistoryPool, PoolLimits, Role},
};

// These are portable operation/actor contracts. Use the production constructor
// with an isolated pool so they also run without test-util or a durable store.
fn spawn_actor(document: kinewright_core::Document) -> Result<Core, Error> {
    Core::spawn(
        &HistoryPool::new(PoolLimits::default()),
        Role::Ordinary,
        CreationOrigin {
            branch: BranchId::fresh().unwrap(),
            batch: BatchId::fresh().unwrap(),
        },
        document,
    )
}

#[path = "kernel_contracts/au1_core.rs"]
mod au1_core;
#[path = "kernel_contracts/au2_core.rs"]
mod au2_core;
#[path = "kernel_contracts/au4_core.rs"]
mod au4_core;
#[path = "kernel_contracts/cc3_core.rs"]
mod cc3_core;
#[path = "kernel_contracts/cc4_core.rs"]
mod cc4_core;
#[path = "kernel_contracts/cc5_core.rs"]
mod cc5_core;
#[path = "kernel_contracts/contracts.rs"]
mod contracts;
#[path = "kernel_contracts/mo2_core.rs"]
mod mo2_core;
