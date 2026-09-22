use std::path::Path;

use async_trait::async_trait;

use crate::{Entry, Result};

/// What a backend is able to do. The UI uses this to grey out actions that
/// don't make sense (you can't `chmod` a file inside a ZIP, for example).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Capabilities {
    pub can_write: bool,
    pub has_unix_permissions: bool,
    pub has_symlinks: bool,
    pub can_watch: bool,
}

/// A Virtual File System: the single interface all storage backends implement.
///
/// The rest of the app only ever talks to a `Vfs`, never to `std::fs` directly.
/// That's what will let us treat a ZIP file, an SFTP server or Google Drive
/// exactly like a local folder later on.
///
/// Methods are `async` because most future backends are network-based.
/// Step 1 only needs reading; writing operations arrive with the job engine.
#[async_trait]
pub trait Vfs: Send + Sync {
    /// A short human-readable name for this backend, e.g. `"local"`.
    fn name(&self) -> &str;

    fn capabilities(&self) -> Capabilities;

    /// Lists the direct children of `path` (not recursive, unsorted).
    async fn list(&self, path: &Path) -> Result<Vec<Entry>>;

    /// Metadata for a single path.
    async fn stat(&self, path: &Path) -> Result<Entry>;
}
