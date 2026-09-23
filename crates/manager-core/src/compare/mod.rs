//! Comparing two folder trees, and syncing what differs.

pub mod diff;
pub mod hash;
pub mod run;

pub use diff::{DiffEntry, DiffStatus, Side};
pub use run::{CompareEvent, CompareHandle, CompareReport, CompareSpec, start};
pub use sync::{SyncAction, SyncDirection, plan_sync, sync_jobs};

mod sync;
