//! Handshaking with an SSH server and opening an SFTP channel on top of it.
//!
//! Split from [`super::fs::RemoteFs`] so the handshake can run over any
//! `AsyncRead + AsyncWrite`, not just a real [`TcpStream`]: tests hand it one
//! end of an in-process pipe, wired to a real (if tiny) SFTP server, and
//! exercise the whole client without opening a socket or touching the
//! filesystem outside a temp directory.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use russh::client::{Config, Handle, connect_stream};
use russh::keys::{self, PrivateKeyWithHashAlg, PublicKeyOrCertificate, load_secret_key};
use russh_sftp::client::SftpSession;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;

use crate::remote::auth::Auth;
use crate::{Error, Result};

/// A working connection: the SSH session (kept alive for as long as the SFTP
/// channel needs it) and the SFTP session on top of it.
pub(crate) struct Connection {
    /// Never read after connecting, but dropping it would hang up the
    /// channel the `sftp` field depends on.
    _handle: Handle<ClientHandler>,
    pub(crate) sftp: SftpSession,
}

impl std::fmt::Debug for Connection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Connection")
    }
}

/// Connects to a real server over TCP.
pub(crate) async fn connect_tcp(
    host: &str,
    port: u16,
    username: &str,
    auth: &Auth,
) -> Result<Connection> {
    let label = format!("{host}:{port}");
    let stream = TcpStream::connect((host, port))
        .await
        .map_err(|e| remote_error(&label, format!("couldn't reach the server: {e}")))?;
    handshake(stream, host, port, username, auth, None).await
}

/// Does the handshake, authentication and SFTP subsystem request over
/// whatever stream it's handed. The seam that makes this all testable
/// without a socket.
///
/// `known_hosts` overrides where host keys are checked and remembered.
/// `None` means the real `~/.ssh/known_hosts` — what every real connection
/// uses. Tests always pass `Some` of a path under their own temp directory:
/// without that, connecting to a made-up test host would write a real,
/// permanent entry into the developer's own SSH configuration every time the
/// suite runs, which is not a side effect a test suite gets to have.
pub(crate) async fn handshake<S>(
    stream: S,
    host: &str,
    port: u16,
    username: &str,
    auth: &Auth,
    known_hosts: Option<&Path>,
) -> Result<Connection>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let label = format!("{host}:{port}");
    let config = Arc::new(Config::default());
    let handler = ClientHandler {
        host: host.to_string(),
        port,
        known_hosts: known_hosts.map(Path::to_path_buf),
    };

    let mut handle = connect_stream(config, stream, handler)
        .await
        .map_err(|e| remote_error(&label, format!("couldn't negotiate the connection: {e}")))?;

    let authenticated = match auth {
        Auth::Password(password) => handle
            .authenticate_password(username, password)
            .await
            .map_err(|e| remote_error(&label, e.to_string()))?,
        Auth::KeyFile { path, passphrase } => {
            let key = load_secret_key(path, passphrase.as_deref()).map_err(|e| {
                remote_error(&label, format!("couldn't read the key at {path:?}: {e}"))
            })?;
            handle
                .authenticate_publickey(username, PrivateKeyWithHashAlg::new(Arc::new(key), None))
                .await
                .map_err(|e| remote_error(&label, e.to_string()))?
        }
    };
    if !authenticated.success() {
        return Err(remote_error(
            &label,
            "the username or password was refused".into(),
        ));
    }

    let channel = handle
        .channel_open_session()
        .await
        .map_err(|e| remote_error(&label, e.to_string()))?;
    channel
        .request_subsystem(true, "sftp")
        .await
        .map_err(|e| remote_error(&label, e.to_string()))?;
    let sftp = SftpSession::new(channel.into_stream())
        .await
        .map_err(|e| remote_error(&label, format!("the SFTP subsystem didn't start: {e}")))?;

    Ok(Connection {
        _handle: handle,
        sftp,
    })
}

fn remote_error(authority: &str, message: String) -> Error {
    Error::Remote {
        authority: authority.into(),
        message: message.into(),
    }
}

/// Verifies the server's host key against the ones already trusted, exactly
/// the way `ssh`, `scp` and every other OpenSSH client does: reading and
/// updating `~/.ssh/known_hosts`. A key that's changed since we last
/// connected is refused rather than silently accepted — the one thing this
/// whole check exists to catch — and a certificate (rather than a bare key)
/// is refused too, since verifying one needs a trusted CA, which nothing
/// here has any way to be told about yet.
pub(crate) struct ClientHandler {
    host: String,
    port: u16,
    known_hosts: Option<PathBuf>,
}

impl russh::client::Handler for ClientHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &PublicKeyOrCertificate,
    ) -> std::result::Result<bool, Self::Error> {
        let PublicKeyOrCertificate::PublicKey { key, .. } = server_public_key else {
            return Ok(false);
        };
        let checked = match &self.known_hosts {
            Some(path) => {
                keys::known_hosts::check_known_hosts_path(&self.host, self.port, key, path)
            }
            None => keys::known_hosts::check_known_hosts(&self.host, self.port, key),
        };
        match checked {
            Ok(true) => Ok(true),
            Ok(false) => {
                // Never seen this host before: trust it, the way `ssh` asks
                // "are you sure you want to continue connecting?" and then
                // remembers the answer. There's nowhere here to ask first —
                // that's the honest cost of not yet having interactive
                // per-call prompts (see the module docs on `RemoteFs`).
                let _ = match &self.known_hosts {
                    Some(path) => {
                        keys::known_hosts::learn_known_hosts_path(&self.host, self.port, key, path)
                    }
                    None => keys::known_hosts::learn_known_hosts(&self.host, self.port, key),
                };
                Ok(true)
            }
            // `KeyChanged` and anything else: refuse. A key that doesn't
            // match what we trusted before is exactly what this check is
            // for; anything unreadable about the known_hosts file itself
            // is safer to treat the same way.
            Err(_) => Ok(false),
        }
    }
}
