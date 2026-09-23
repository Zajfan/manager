//! `RemoteFs`: a [`Vfs`] over one or more SFTP connections.
//!
//! Connecting is deliberately not part of the [`Vfs`] trait: every other
//! backend can do everything a path needs from nothing but the path itself,
//! but an SFTP server needs a username and something to prove it with first.
//! [`RemoteFs::connect`] is the one place that happens; every `Vfs` method
//! below just looks up the connection that was already made, keyed by the
//! path's authority (the same string a `sftp://` URI's `user@host:port`
//! carries), and says [`Error::NotConnected`] if there isn't one yet.
//!
//! One consequence worth knowing: if a connection drops partway through a
//! job (a network blip), the job surfaces that as an ordinary
//! [`JobEvent::Error`](crate::jobs::JobEvent::Error) — retrying it will fail
//! again with `NotConnected` until something reconnects. There's no
//! mid-job "please type your password again" prompt; reconnecting is a
//! deliberate, separate step through [`RemoteFs::connect`].

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use async_trait::async_trait;
use russh_sftp::client::error::Error as SftpError;
use russh_sftp::protocol::{FileAttributes, FileType, StatusCode};

use crate::remote::auth::Auth;
use crate::remote::client::{self, Connection};
use crate::vfs::{ReadStream, WriteStream};
use crate::vpath::Base;
use crate::{Capabilities, Entry, EntryKind, Error, LinkTarget, Permissions, Result, VPath, Vfs};

/// Connections that have been made, keyed by authority (`user@host:port`).
#[derive(Debug, Default)]
pub struct RemoteFs {
    pool: Mutex<HashMap<String, Arc<Connection>>>,
}

impl RemoteFs {
    pub fn new() -> RemoteFs {
        RemoteFs::default()
    }

    /// Connects to `host:port` as `username` and remembers the connection
    /// under `authority`, so every `VPath` carrying that same authority can
    /// find it again. Replaces whatever was connected there before.
    ///
    /// On success, returns the path to land the panel on: the server's own
    /// idea of this user's home folder when it says, "/" otherwise (some
    /// servers don't support asking).
    pub async fn connect(
        &self,
        authority: &str,
        host: &str,
        port: u16,
        username: &str,
        auth: &Auth,
    ) -> Result<VPath> {
        let connection = client::connect_tcp(host, port, username, auth).await?;
        self.remember(authority, connection).await
    }

    /// The same, but over a stream that's already open rather than a real
    /// `host:port` — what `tests/remote.rs` uses to connect to an in-process
    /// server over a pipe, with everything past the transport identical to a
    /// real connection.
    pub async fn connect_stream<S>(
        &self,
        authority: &str,
        stream: S,
        host: &str,
        port: u16,
        username: &str,
        auth: &Auth,
    ) -> Result<VPath>
    where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
    {
        let connection = client::handshake(stream, host, port, username, auth).await?;
        self.remember(authority, connection).await
    }

    async fn remember(&self, authority: &str, connection: Connection) -> Result<VPath> {
        // Best effort: a server that doesn't support this just means landing
        // on "/" instead of this user's actual home folder.
        let home = connection
            .sftp
            .canonicalize(".")
            .await
            .unwrap_or_else(|_| "/".into());
        let Ok(mut pool) = self.pool.lock() else {
            return VPath::remote("sftp", authority, &home); // a poisoned pool just means no caching
        };
        pool.insert(authority.to_string(), Arc::new(connection));
        VPath::remote("sftp", authority, &home)
    }

    /// Whether `authority` has a live connection.
    pub fn is_connected(&self, authority: &str) -> bool {
        self.pool
            .lock()
            .is_ok_and(|pool| pool.contains_key(authority))
    }

    /// The already-open connection for `path`, and the POSIX-style path
    /// string to send it on the wire.
    fn session(&self, path: &VPath) -> Result<(Arc<Connection>, String)> {
        if !path.layers().is_empty() {
            return Err(unsupported(path));
        }
        let Base::Remote {
            authority,
            path: inner,
            ..
        } = path.base()
        else {
            return Err(unsupported(path));
        };
        let connection = self
            .pool
            .lock()
            .ok()
            .and_then(|pool| pool.get(authority).cloned())
            .ok_or_else(|| Error::NotConnected(path.clone()))?;
        Ok((connection, inner.to_string()))
    }
}

fn unsupported(path: &VPath) -> Error {
    Error::Unsupported {
        backend: "sftp",
        path: path.clone(),
    }
}

#[async_trait]
impl Vfs for RemoteFs {
    fn name(&self) -> &str {
        "sftp"
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            can_write: true,
            has_unix_permissions: true,
            has_symlinks: true,
            can_watch: false,
            has_trash: false,
        }
    }

    async fn list(&self, path: &VPath) -> Result<Vec<Entry>> {
        let (conn, wire) = self.session(path)?;
        let listing = conn
            .sftp
            .read_dir(&wire)
            .await
            .map_err(|e| sftp_error(path, e))?;

        let mut out = Vec::new();
        for item in listing {
            let name = item.file_name();
            if name == "." || name == ".." {
                continue;
            }
            let child_path = path.join(&name)?;
            let child_wire = join_wire(&wire, &name);
            out.push(to_entry(&conn, child_path, name, &child_wire, item.metadata()).await);
        }
        Ok(out)
    }

    async fn stat(&self, path: &VPath) -> Result<Entry> {
        let (conn, wire) = self.session(path)?;
        let meta = conn
            .sftp
            .symlink_metadata(&wire)
            .await
            .map_err(|e| sftp_error(path, e))?;
        let name = path.file_name().unwrap_or_default();
        Ok(to_entry(&conn, path.clone(), name, &wire, meta).await)
    }

    async fn create_dir(&self, path: &VPath) -> Result<()> {
        let (conn, wire) = self.session(path)?;
        conn.sftp
            .create_dir(&wire)
            .await
            .map_err(|e| sftp_error(path, e))
    }

    async fn open_read(&self, path: &VPath) -> Result<ReadStream> {
        let (conn, wire) = self.session(path)?;
        let file = conn
            .sftp
            .open(&wire)
            .await
            .map_err(|e| sftp_error(path, e))?;
        Ok(Box::new(file))
    }

    async fn open_write(&self, path: &VPath) -> Result<WriteStream> {
        let (conn, wire) = self.session(path)?;
        let file = conn
            .sftp
            .create(&wire)
            .await
            .map_err(|e| sftp_error(path, e))?;
        Ok(Box::new(file))
    }

    async fn rename(&self, from: &VPath, to: &VPath) -> Result<()> {
        let (conn, from_wire) = self.session(from)?;
        // A rename to a different server isn't one rename; treating it as
        // "crossing devices" is exactly right — that's the job engine's own
        // signal to fall back to copying and then deleting.
        let (_, to_wire) = match self.session(to) {
            Ok(pair) if same_authority(from, to) => pair,
            _ => return Err(Error::CrossesDevices(from.clone())),
        };
        conn.sftp
            .rename(from_wire, to_wire)
            .await
            .map_err(|e| sftp_error(from, e))
    }

    async fn remove_file(&self, path: &VPath) -> Result<()> {
        let (conn, wire) = self.session(path)?;
        conn.sftp
            .remove_file(&wire)
            .await
            .map_err(|e| sftp_error(path, e))
    }

    async fn remove_dir(&self, path: &VPath) -> Result<()> {
        let (conn, wire) = self.session(path)?;
        conn.sftp
            .remove_dir(&wire)
            .await
            .map_err(|e| sftp_error(path, e))
    }

    async fn create_symlink(
        &self,
        target: &Path,
        link: &VPath,
        _target_is_dir: bool,
    ) -> Result<()> {
        let (conn, wire) = self.session(link)?;
        // `russh_sftp` names its parameters (path, target) in the order the
        // SFTP spec actually defines: the new link first, what it points to
        // second. OpenSSH's own `sftp-server` is well known to implement
        // this backwards; this has only been checked against this crate's
        // own client and server (see `tests/remote.rs`), not a real OpenSSH
        // one, so verify it before relying on it against a real server.
        conn.sftp
            .symlink(wire, target.to_string_lossy().into_owned())
            .await
            .map_err(|e| sftp_error(link, e))
    }

    async fn set_modified(&self, path: &VPath, time: SystemTime) -> Result<()> {
        let (conn, wire) = self.session(path)?;
        // SFTPv3 timestamps are 32-bit Unix seconds; a date past 2106
        // saturates rather than wrapping into the past.
        let secs = time
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| u32::try_from(d.as_secs()).unwrap_or(u32::MAX))
            .unwrap_or(0);
        let attrs = FileAttributes {
            mtime: Some(secs),
            ..FileAttributes::empty()
        };
        conn.sftp
            .set_metadata(wire, attrs)
            .await
            .map_err(|e| sftp_error(path, e))
    }

    async fn set_permissions(&self, path: &VPath, permissions: Permissions) -> Result<()> {
        let Some(mode) = permissions.unix_mode else {
            return Ok(()); // nothing to send; the trait says this is best-effort
        };
        let (conn, wire) = self.session(path)?;
        let attrs = FileAttributes {
            permissions: Some(mode),
            ..FileAttributes::empty()
        };
        conn.sftp
            .set_metadata(wire, attrs)
            .await
            .map_err(|e| sftp_error(path, e))
    }
}

fn same_authority(a: &VPath, b: &VPath) -> bool {
    matches!(
        (a.base(), b.base()),
        (Base::Remote { authority: x, .. }, Base::Remote { authority: y, .. }) if x == y
    )
}

fn join_wire(dir: &str, name: &str) -> String {
    if dir.ends_with('/') {
        format!("{dir}{name}")
    } else {
        format!("{dir}/{name}")
    }
}

/// Builds an [`Entry`] from what SFTP told us. For a symlink this costs two
/// more round trips (its raw target text, and a `stat` that follows it to
/// say whether the far end is a file or a folder) — the honest price of
/// showing broken links instead of hiding them, the same choice `LocalFs`
/// makes for real symlinks.
async fn to_entry(
    conn: &Connection,
    path: VPath,
    name: String,
    wire: &str,
    meta: FileAttributes,
) -> Entry {
    let hidden = name.starts_with('.');
    let permissions = Permissions {
        readonly: meta.permissions().is_readonly(),
        unix_mode: meta.permissions,
    };
    let (kind, link_target) = match meta.file_type() {
        FileType::Dir => (EntryKind::Dir, None),
        FileType::File => (EntryKind::File, None),
        FileType::Symlink => {
            let raw = conn.sftp.read_link(wire).await.ok();
            let target = match conn.sftp.metadata(wire).await {
                Ok(followed) => Some(match followed.file_type() {
                    FileType::Dir => LinkTarget::Dir,
                    FileType::File => LinkTarget::File,
                    FileType::Symlink | FileType::Other => LinkTarget::Other,
                }),
                Err(_) => None, // broken: shown anyway, never hidden
            };
            (EntryKind::Symlink { target }, raw.map(PathBuf::from))
        }
        FileType::Other => (EntryKind::Other, None),
    };
    Entry {
        name,
        path,
        kind,
        size: meta.len(),
        modified: meta.modified().ok(),
        created: None, // SFTP has no notion of a creation time
        accessed: meta.accessed().ok(),
        permissions,
        hidden,
        link_target,
    }
}

fn sftp_error(path: &VPath, err: SftpError) -> Error {
    match err {
        SftpError::Status(status) => match status.status_code {
            StatusCode::NoSuchFile => Error::NotFound(path.clone()),
            StatusCode::PermissionDenied => Error::PermissionDenied(path.clone()),
            _ => Error::InvalidOperation(format!("{path}: {}", status.error_message)),
        },
        other => Error::InvalidOperation(format!("{path}: {other}")),
    }
}
