//! `manager-core` is the engine of the file manager.
//!
//! It knows nothing about terminals or windows. Frontends (the TUI and the GUI)
//! ask it for directory listings, then show them however they like.
//!
//! The main pieces:
//! - [`VPath`]: an address for any file anywhere: local disk, server, or inside an archive.
//! - [`Vfs`]: the one trait every storage backend implements (local disk now; archives,
//!   SFTP, cloud later).
//! - [`LocalFs`]: the backend for the computer's own disks.
//! - [`Entry`]: everything we know about one file or folder.
//! - [`sort_entries`]: Total Commander–style sorting (folders first, natural order).

mod entry;
mod error;
mod local;
mod sort;
mod vfs;
mod vpath;

pub use entry::{Entry, EntryKind, LinkTarget, Permissions};
pub use error::{Error, Result};
pub use local::LocalFs;
pub use sort::{SortKey, SortOrder, SortSpec, natural_cmp, sort_entries};
pub use vfs::{Capabilities, Vfs};
pub use vpath::{Base, InnerPath, Layer, VPath};
