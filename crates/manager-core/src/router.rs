//! Sending each path to the backend that can handle it.

use std::path::Path;
use std::time::SystemTime;

use async_trait::async_trait;

use crate::remote::{Auth, RemoteFs};
use crate::vfs::{ReadStream, WriteStream};
use crate::vpath::Base;
use crate::watch::{WatchHandle, WatchSink};
use crate::{ArchiveFs, Capabilities, Entry, LocalFs, Permissions, Result, VPath, Vfs};

/// The [`Vfs`] the app actually talks to.
///
/// It looks at a path and hands the work to whichever backend owns it: a plain
/// path goes to the disk, a path with archive layers goes inside the archive.
/// Because everything else — the job engine included — only sees one `Vfs`,
/// copying a file out of a ZIP is the same code as copying between folders.
#[derive(Default)]
pub struct Router {
    local: LocalFs,
    archives: ArchiveFs,
    remote: RemoteFs,
}

impl Router {
    pub fn new() -> Router {
        Router::default()
    }

    /// The backend that owns `path`.
    fn pick(&self, path: &VPath) -> &dyn Vfs {
        if !path.layers().is_empty() {
            return &self.archives;
        }
        match path.base() {
            Base::Local(_) => &self.local,
            Base::Remote { .. } => &self.remote,
        }
    }

    /// Connects to an SFTP server and remembers it under `authority`
    /// (`user@host:port`, the same form a `sftp://` `VPath` carries), so
    /// every path with that authority can be used right after this returns.
    pub async fn connect(
        &self,
        authority: &str,
        host: &str,
        port: u16,
        username: &str,
        auth: &Auth,
    ) -> Result<VPath> {
        self.remote
            .connect(authority, host, port, username, auth)
            .await
    }

    /// The same, but over a stream that's already open, checking host keys
    /// against `known_hosts` instead of the real `~/.ssh/known_hosts`. See
    /// [`RemoteFs::connect_stream`] — used only by tests.
    pub async fn connect_stream<S>(
        &self,
        authority: &str,
        stream: S,
        username: &str,
        auth: &Auth,
        known_hosts: &std::path::Path,
    ) -> Result<VPath>
    where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
    {
        self.remote
            .connect_stream(
                authority,
                stream,
                ("test-server", 22),
                username,
                auth,
                known_hosts,
            )
            .await
    }

    /// Whether `authority` already has a connection open.
    pub fn is_connected(&self, authority: &str) -> bool {
        self.remote.is_connected(authority)
    }
}

#[async_trait]
impl Vfs for Router {
    fn name(&self) -> &str {
        "manager"
    }

    /// What the disk can do. Callers that care about one path should ask the
    /// backend for that path instead; this is the general answer.
    fn capabilities(&self) -> Capabilities {
        self.local.capabilities()
    }

    async fn list(&self, path: &VPath) -> Result<Vec<Entry>> {
        self.pick(path).list(path).await
    }

    async fn stat(&self, path: &VPath) -> Result<Entry> {
        self.pick(path).stat(path).await
    }

    async fn create_dir(&self, path: &VPath) -> Result<()> {
        self.pick(path).create_dir(path).await
    }

    async fn open_read(&self, path: &VPath) -> Result<ReadStream> {
        self.pick(path).open_read(path).await
    }

    async fn read_window(&self, path: &VPath, offset: u64, len: usize) -> Result<Vec<u8>> {
        self.pick(path).read_window(path, offset, len).await
    }

    async fn open_write(&self, path: &VPath) -> Result<WriteStream> {
        self.pick(path).open_write(path).await
    }

    /// Renaming only ever happens within one backend. Anything else is
    /// reported as a move between devices, which makes the job engine fall
    /// back to copying and then deleting — the only way to cross a boundary.
    /// (Two SFTP paths on different servers get this same treatment: whether
    /// their authorities actually match is `RemoteFs`'s own call to make.)
    async fn rename(&self, from: &VPath, to: &VPath) -> Result<()> {
        let same_kind = matches!(
            (from.base(), to.base()),
            (Base::Local(_), Base::Local(_)) | (Base::Remote { .. }, Base::Remote { .. })
        );
        if from.layers().is_empty() && to.layers().is_empty() && same_kind {
            return self.pick(from).rename(from, to).await;
        }
        Err(crate::Error::CrossesDevices(from.clone()))
    }

    async fn remove_file(&self, path: &VPath) -> Result<()> {
        self.pick(path).remove_file(path).await
    }

    async fn remove_dir(&self, path: &VPath) -> Result<()> {
        self.pick(path).remove_dir(path).await
    }

    async fn trash(&self, path: &VPath) -> Result<()> {
        self.pick(path).trash(path).await
    }

    async fn watch(&self, dir: &VPath, sink: WatchSink) -> Result<WatchHandle> {
        self.pick(dir).watch(dir, sink).await
    }

    async fn create_symlink(&self, target: &Path, link: &VPath, target_is_dir: bool) -> Result<()> {
        self.pick(link)
            .create_symlink(target, link, target_is_dir)
            .await
    }

    async fn set_modified(&self, path: &VPath, time: SystemTime) -> Result<()> {
        self.pick(path).set_modified(path, time).await
    }

    async fn set_permissions(&self, path: &VPath, permissions: Permissions) -> Result<()> {
        self.pick(path).set_permissions(path, permissions).await
    }
}
