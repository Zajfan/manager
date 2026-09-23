//! A small, real SFTP server, backed by a real temp directory, that speaks to
//! a client over an in-memory pipe rather than a socket.
//!
//! This exists so `tests/remote.rs` can exercise the whole client stack —
//! the SSH handshake, authentication, and every SFTP request `RemoteFs`
//! makes — without opening a port or depending on a real SSH daemon being
//! installed and reachable, matching how every other backend in this project
//! is tested against something real rather than a mock.
//!
//! It's deliberately not a general-purpose SFTP server: no other project
//! should reuse this. It implements just enough of the protocol, does no
//! permission checking of its own beyond what the real filesystem already
//! does, and every path is trusted to be handed straight to
//! `Path::join` — fine for a server that only ever talks to the one client
//! this test process starts, over a pipe nothing else can reach.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use russh::keys::PrivateKey;
use russh::keys::ssh_key::private::Ed25519Keypair;
use russh::server::{Auth as SshAuth, ChannelOpenHandle, Handler as SshHandler, Msg, Session};
use russh::{Channel, ChannelId};
use russh_sftp::protocol::{
    Attrs, Data, File, FileAttributes, Handle, Name, OpenFlags, Status, StatusCode, Version,
};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio::sync::Mutex;

/// Username and password the server accepts. Anything else is refused.
pub const USER: &str = "tester";
pub const PASSWORD: &str = "letmein";

/// Starts the server on one end of an in-memory pipe, serving `root`, and
/// returns the other end for a client to connect on.
///
/// The server runs on its own background task for as long as the returned
/// stream (or anything cloned from the connection made on it) is alive.
pub async fn serve(root: PathBuf) -> tokio::io::DuplexStream {
    let (client_end, server_end) = tokio::io::duplex(64 * 1024);
    spawn_server(root, server_end);
    client_end
}

/// The same server, reachable over a real loopback TCP socket on an
/// OS-assigned port, for the one test that exists to prove the real
/// `TcpStream`-based connect path — not just the pipe every other test
/// uses — actually works.
pub async fn serve_tcp(root: PathBuf) -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("couldn't bind a loopback port for the test server");
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        // One connection is all any test needs.
        if let Ok((stream, _)) = listener.accept().await {
            spawn_server(root, stream);
        }
    });
    port
}

fn spawn_server<S>(root: PathBuf, stream: S)
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let config = Arc::new(russh::server::Config {
        // A fixed seed: this key only ever has to satisfy this one process's
        // own client, so there's nothing "random" needs to protect against,
        // and it saves the test suite a dependency on an RNG crate.
        keys: vec![PrivateKey::from(Ed25519Keypair::from_seed(&[7u8; 32]))],
        ..Default::default()
    });
    let handler = SshSession {
        root,
        channels: Arc::new(Mutex::new(HashMap::new())),
    };

    // `run_stream` itself starts by reading the client's SSH banner — so it
    // must not be awaited before the caller can hand the other end (or the
    // listening socket) to a client. Awaiting it here would mean waiting for
    // a client that can't send anything yet: a deadlock between two sides
    // each waiting for the other to go first.
    tokio::spawn(async move {
        if let Ok(running) = russh::server::run_stream(config, stream, handler).await {
            let _ = running.await;
        }
    });
}

// -------------------------------------------------------------------------------------------
// The SSH-level handler: authentication, and handing the sftp subsystem off
// -------------------------------------------------------------------------------------------

struct SshSession {
    root: PathBuf,
    channels: Arc<Mutex<HashMap<ChannelId, Channel<Msg>>>>,
}

impl SshHandler for SshSession {
    type Error = anyhow::Error;

    async fn auth_password(&mut self, user: &str, password: &str) -> anyhow::Result<SshAuth> {
        Ok(if user == USER && password == PASSWORD {
            SshAuth::Accept
        } else {
            SshAuth::reject()
        })
    }

    async fn channel_open_session(
        &mut self,
        channel: Channel<Msg>,
        reply: ChannelOpenHandle,
        _session: &mut Session,
    ) -> anyhow::Result<()> {
        self.channels.lock().await.insert(channel.id(), channel);
        reply.accept().await;
        Ok(())
    }

    async fn subsystem_request(
        &mut self,
        channel_id: ChannelId,
        name: &str,
        session: &mut Session,
    ) -> anyhow::Result<()> {
        if name != "sftp" {
            session.channel_failure(channel_id)?;
            return Ok(());
        }
        let Some(channel) = self.channels.lock().await.remove(&channel_id) else {
            session.channel_failure(channel_id)?;
            return Ok(());
        };
        session.channel_success(channel_id)?;
        let fs = FsHandler::new(self.root.clone());
        tokio::spawn(russh_sftp::server::run(channel.into_stream(), fs));
        Ok(())
    }
}

// -------------------------------------------------------------------------------------------
// The SFTP-level handler: every request translated into a real filesystem call
// -------------------------------------------------------------------------------------------

struct FsHandler {
    root: PathBuf,
    files: HashMap<String, tokio::fs::File>,
    /// What's left to hand out for an in-progress `readdir`.
    dirs: HashMap<String, Vec<(String, FileAttributes)>>,
    next_handle: u64,
}

impl FsHandler {
    fn new(root: PathBuf) -> FsHandler {
        FsHandler {
            root,
            files: HashMap::new(),
            dirs: HashMap::new(),
            next_handle: 0,
        }
    }

    /// The real path a wire path (always POSIX-absolute) refers to.
    fn resolve(&self, wire: &str) -> PathBuf {
        let relative = wire.trim_start_matches('/');
        if relative.is_empty() {
            self.root.clone()
        } else {
            self.root.join(relative)
        }
    }

    fn new_handle(&mut self) -> String {
        self.next_handle += 1;
        self.next_handle.to_string()
    }
}

/// The one status this server ever returns for "that went wrong somehow":
/// close enough for a test double, and every genuine test failure this could
/// mask is exactly the kind a real assertion on the *result* would still
/// catch.
fn failure(_err: impl std::fmt::Display) -> StatusCode {
    StatusCode::Failure
}

fn no_such_file(_err: impl std::fmt::Display) -> StatusCode {
    StatusCode::NoSuchFile
}

fn ok(id: u32) -> Status {
    Status {
        id,
        status_code: StatusCode::Ok,
        error_message: "Ok".to_string(),
        language_tag: "en-US".to_string(),
    }
}

impl russh_sftp::server::Handler for FsHandler {
    type Error = StatusCode;

    fn unimplemented(&self) -> Self::Error {
        StatusCode::OpUnsupported
    }

    async fn init(
        &mut self,
        _version: u32,
        _extensions: HashMap<String, String>,
    ) -> Result<Version, Self::Error> {
        Ok(Version::new())
    }

    async fn close(&mut self, id: u32, handle: String) -> Result<Status, Self::Error> {
        self.files.remove(&handle);
        self.dirs.remove(&handle);
        Ok(ok(id))
    }

    async fn open(
        &mut self,
        id: u32,
        filename: String,
        pflags: OpenFlags,
        _attrs: FileAttributes,
    ) -> Result<Handle, Self::Error> {
        let path = self.resolve(&filename);
        let mut options = tokio::fs::OpenOptions::new();
        options
            .read(pflags.contains(OpenFlags::READ))
            .write(pflags.contains(OpenFlags::WRITE))
            .append(pflags.contains(OpenFlags::APPEND))
            .create(pflags.contains(OpenFlags::CREATE))
            .truncate(pflags.contains(OpenFlags::TRUNCATE))
            .create_new(pflags.contains(OpenFlags::EXCLUDE) && pflags.contains(OpenFlags::CREATE));
        let file = options.open(&path).await.map_err(no_such_file)?;
        let handle = self.new_handle();
        self.files.insert(handle.clone(), file);
        Ok(Handle { id, handle })
    }

    async fn read(
        &mut self,
        id: u32,
        handle: String,
        offset: u64,
        len: u32,
    ) -> Result<Data, Self::Error> {
        let file = self.files.get_mut(&handle).ok_or(StatusCode::Failure)?;
        file.seek(std::io::SeekFrom::Start(offset))
            .await
            .map_err(failure)?;
        let mut buffer = vec![0u8; len as usize];
        let read = file.read(&mut buffer).await.map_err(failure)?;
        if read == 0 {
            return Err(StatusCode::Eof);
        }
        buffer.truncate(read);
        Ok(Data { id, data: buffer })
    }

    async fn write(
        &mut self,
        id: u32,
        handle: String,
        offset: u64,
        data: Vec<u8>,
    ) -> Result<Status, Self::Error> {
        let file = self.files.get_mut(&handle).ok_or(StatusCode::Failure)?;
        file.seek(std::io::SeekFrom::Start(offset))
            .await
            .map_err(failure)?;
        file.write_all(&data).await.map_err(failure)?;
        Ok(ok(id))
    }

    async fn lstat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        let meta = tokio::fs::symlink_metadata(self.resolve(&path))
            .await
            .map_err(no_such_file)?;
        Ok(Attrs {
            id,
            attrs: (&meta).into(),
        })
    }

    async fn stat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        let meta = tokio::fs::metadata(self.resolve(&path))
            .await
            .map_err(no_such_file)?;
        Ok(Attrs {
            id,
            attrs: (&meta).into(),
        })
    }

    async fn setstat(
        &mut self,
        id: u32,
        path: String,
        attrs: FileAttributes,
    ) -> Result<Status, Self::Error> {
        apply_attrs(&self.resolve(&path), &attrs).map_err(failure)?;
        Ok(ok(id))
    }

    async fn opendir(&mut self, id: u32, path: String) -> Result<Handle, Self::Error> {
        let dir = self.resolve(&path);
        if !dir.is_dir() {
            return Err(StatusCode::NoSuchFile);
        }
        let mut entries = Vec::new();
        let mut read = tokio::fs::read_dir(&dir).await.map_err(failure)?;
        while let Some(item) = read.next_entry().await.map_err(failure)? {
            let meta = item.metadata().await.map_err(failure)?;
            entries.push((
                item.file_name().to_string_lossy().into_owned(),
                (&meta).into(),
            ));
        }
        let handle = self.new_handle();
        self.dirs.insert(handle.clone(), entries);
        Ok(Handle { id, handle })
    }

    async fn readdir(&mut self, id: u32, handle: String) -> Result<Name, Self::Error> {
        let entries = self.dirs.get_mut(&handle).ok_or(StatusCode::Failure)?;
        if entries.is_empty() {
            return Err(StatusCode::Eof);
        }
        let files = std::mem::take(entries)
            .into_iter()
            .map(|(name, attrs)| File::new(name, attrs))
            .collect();
        Ok(Name { id, files })
    }

    async fn remove(&mut self, id: u32, filename: String) -> Result<Status, Self::Error> {
        tokio::fs::remove_file(self.resolve(&filename))
            .await
            .map_err(no_such_file)?;
        Ok(ok(id))
    }

    async fn mkdir(
        &mut self,
        id: u32,
        path: String,
        _attrs: FileAttributes,
    ) -> Result<Status, Self::Error> {
        tokio::fs::create_dir(self.resolve(&path))
            .await
            .map_err(failure)?;
        Ok(ok(id))
    }

    async fn rmdir(&mut self, id: u32, path: String) -> Result<Status, Self::Error> {
        tokio::fs::remove_dir(self.resolve(&path))
            .await
            .map_err(no_such_file)?;
        Ok(ok(id))
    }

    async fn realpath(&mut self, id: u32, path: String) -> Result<Name, Self::Error> {
        let normalized = normalize(&path);
        Ok(Name {
            id,
            files: vec![File::dummy(normalized)],
        })
    }

    async fn rename(
        &mut self,
        id: u32,
        oldpath: String,
        newpath: String,
    ) -> Result<Status, Self::Error> {
        tokio::fs::rename(self.resolve(&oldpath), self.resolve(&newpath))
            .await
            .map_err(failure)?;
        Ok(ok(id))
    }

    async fn readlink(&mut self, id: u32, path: String) -> Result<Name, Self::Error> {
        let target = tokio::fs::read_link(self.resolve(&path))
            .await
            .map_err(no_such_file)?;
        Ok(Name {
            id,
            files: vec![File::dummy(target.to_string_lossy().into_owned())],
        })
    }

    #[cfg(unix)]
    async fn symlink(
        &mut self,
        id: u32,
        // Named for what real OpenSSH `sftp-server` actually does with these
        // two fields, not what the SFTP spec (or this crate's own client
        // parameter names) would suggest — confirmed against the real
        // binary in `tests/real_openssh.rs`. Matching that, rather than the
        // spec, is the whole point of this handler: a fake server that
        // quietly did the "correct" thing here would let `RemoteFs` and its
        // own tests agree with each other while both disagreeing with every
        // real server anyone actually connects to.
        old: String,
        new: String,
    ) -> Result<Status, Self::Error> {
        tokio::fs::symlink(old, self.resolve(&new))
            .await
            .map_err(failure)?;
        Ok(ok(id))
    }
}

/// `..`/`.`/empty segments collapsed, always returned with a leading `/`.
/// Doesn't allow climbing above the root — there's nothing above it to climb
/// into on the wire anyway.
fn normalize(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    format!("/{}", parts.join("/"))
}

fn apply_attrs(path: &Path, attrs: &FileAttributes) -> std::io::Result<()> {
    if let Some(secs) = attrs.mtime {
        let time = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(secs.into());
        std::fs::File::options()
            .write(true)
            .open(path)?
            .set_modified(time)?;
    }
    #[cfg(unix)]
    if let Some(mode) = attrs.permissions {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))?;
    }
    Ok(())
}
