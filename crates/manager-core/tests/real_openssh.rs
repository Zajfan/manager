//! Checking `RemoteFs::create_symlink` against the real, authoritative
//! OpenSSH `sftp-server` binary — not a stand-in, not this project's own
//! test server, the actual thing almost every SFTP server anyone connects
//! to really is.
//!
//! SFTP itself doesn't care how the bytes get to it, so this talks to the
//! binary directly over its own stdin/stdout — the same protocol it speaks
//! once `sshd` hands a client off to it — no SSH transport, no network, and
//! no risk of two implementations of the same crate quietly agreeing with
//! each other about something the real world disagrees with.
//!
//! `sftp-server` isn't at a fixed, portable path, so this looks in the usual
//! places first and prints a note and returns, rather than failing, when it
//! can't find one. Where it's available (most real Linux machines and many
//! CI images), it runs and means something; where it isn't, this file adds
//! nothing and costs nothing.

use std::pin::Pin;
use std::process::Stdio;
use std::task::{Context, Poll};

use tempfile::TempDir;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

/// The usual places OpenSSH's `sftp-server` gets installed. This project
/// doesn't need to agree with any particular distribution: it just needs one
/// of these to exist for the test to mean something.
const CANDIDATES: &[&str] = &[
    "/usr/libexec/openssh/sftp-server",  // Fedora, RHEL
    "/usr/lib/openssh/sftp-server",      // Debian, Ubuntu
    "/usr/lib/ssh/sftp-server",          // Arch, some others
    "/usr/libexec/sftp-server",          // macOS's own built-in OpenSSH
    "/opt/homebrew/libexec/sftp-server", // macOS, Homebrew (Apple Silicon)
    "/usr/local/libexec/sftp-server",    // macOS, Homebrew (Intel)
];

fn find_sftp_server() -> Option<&'static str> {
    CANDIDATES
        .iter()
        .copied()
        .find(|path| std::path::Path::new(path).exists())
}

/// Glues a child process's separate stdin and stdout into the one
/// `AsyncRead + AsyncWrite` stream `SftpSession` wants — exactly what a
/// two-way SSH channel would otherwise provide.
struct ChildPipe {
    stdin: ChildStdin,
    stdout: ChildStdout,
    // Kept alive so the process isn't reaped (and the pipes closed) while
    // this is still in use; never read after construction.
    _child: Child,
}

impl AsyncRead for ChildPipe {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.get_mut().stdout).poll_read(cx, buf)
    }
}

impl AsyncWrite for ChildPipe {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.get_mut().stdin).poll_write(cx, data)
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.get_mut().stdin).poll_flush(cx)
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.get_mut().stdin).poll_shutdown(cx)
    }
}

fn spawn_sftp_server(binary: &str) -> ChildPipe {
    let mut child = Command::new(binary)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("couldn't start the real sftp-server binary");
    let stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    ChildPipe {
        stdin,
        stdout,
        _child: child,
    }
}

/// This is the one this whole file exists for. OpenSSH's own `sftp-server`
/// is well known to implement `SSH_FXP_SYMLINK`'s two path arguments in the
/// opposite order from the one the protocol spec actually defines — a
/// decades-old, deliberately-kept-for-compatibility historical bug. This
/// proves, against the real binary rather than a guess, which order this
/// project's own `RemoteFs::create_symlink` needs to send them in.
#[tokio::test]
async fn symlink_against_the_real_openssh_binary() {
    let Some(binary) = find_sftp_server() else {
        eprintln!("skipping: no OpenSSH sftp-server binary found in any of {CANDIDATES:?}");
        return;
    };
    let tmp = TempDir::new().unwrap();
    std::fs::write(tmp.path().join("target.txt"), b"the real file").unwrap();

    let session = russh_sftp::client::SftpSession::new(spawn_sftp_server(binary))
        .await
        .expect("the real sftp-server didn't complete its version handshake");
    let link = tmp.path().join("link.txt").to_string_lossy().into_owned();
    let target = tmp.path().join("target.txt").to_string_lossy().into_owned();

    // What `RemoteFs::create_symlink` now sends, after this test proved the
    // spec-literal order fails against the real binary: OpenSSH's own debug
    // log labels the two arguments "old" and "new", the same names
    // `rename(2)`/`symlink(2)` use — meaning it wants the *target* first and
    // the *new link's path* second, the reverse of `russh_sftp`'s own
    // parameter names (`symlink(path, target)`, `path` meaning the link).
    session.symlink(target.clone(), link.clone()).await.expect(
        "the request should succeed with (target, link) order; if this now fails, \
         the real server's behaviour has changed and RemoteFs::create_symlink \
         needs re-checking against it",
    );

    // Read the *real* filesystem directly — not through SFTP again, so this
    // can't be fooled by the client and server quietly agreeing on the same
    // mistake the way testing only against this project's own test server
    // would be.
    let link_path = std::path::Path::new(&link);
    assert!(
        std::fs::symlink_metadata(link_path).is_ok_and(|m| m.file_type().is_symlink()),
        "expected a symlink at {link:?}"
    );
    assert_eq!(
        std::fs::read_link(link_path).unwrap().to_string_lossy(),
        target
    );
}
