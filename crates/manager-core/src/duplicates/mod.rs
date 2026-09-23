//! Finding files with identical content.

pub mod group;
pub mod run;

pub use group::group_by_size;
pub use run::{
    DuplicateEvent, DuplicateGroup, DuplicateHandle, DuplicateReport, DuplicateSpec, start,
};
