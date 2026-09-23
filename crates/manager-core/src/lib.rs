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
//! - [`jobs`]: copy, move and delete in the background, with progress, pause and cancel.
//! - [`watch`]: telling the UI when a folder changed behind its back.
//! - [`sort_entries`]: Total Commander–style sorting (folders first, natural order).

pub mod archive;
mod entry;
mod error;
pub mod jobs;
mod local;
mod router;
mod sort;
mod vfs;
pub mod view;
mod vpath;
pub mod watch;

pub use archive::ArchiveFs;
pub use entry::{Entry, EntryKind, LinkTarget, Permissions};
pub use error::{Error, Result};
pub use local::LocalFs;
pub use router::Router;
pub use sort::{SortKey, SortOrder, SortSpec, natural_cmp, sort_entries};
pub use vfs::{Capabilities, ReadStream, Vfs, WriteStream};
pub use vpath::{Base, InnerPath, Layer, VPath};
pub use watch::{WatchHandle, WatchSink};
