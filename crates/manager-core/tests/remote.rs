//! Browsing, editing and searching files over SFTP.
//!
//! Every test here talks to a real (if tiny) SFTP server implementing the
//! real protocol, over an in-memory pipe rather than a socket — see
//! `support/sftp_server.rs` for why and how.

mod support;

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use manager_core::remote::Auth;
use manager_core::{EntryKind, Error, Router, VPath, Vfs};
use support::sftp_server::{self, PASSWORD, USER};
use tempfile::TempDir;
use tokio::time::timeout;

const AUTHORITY: &str = "tester@test-server:22";

/// A `Router` already connected to a server backed by `root`.
///
/// Host keys go to a `known_hosts` file under a throwaway temp directory,
/// never the real `~/.ssh/known_hosts` — a made-up test host has no business
/// leaving a permanent entry in a real person's SSH configuration. The temp
/// directory only needs to outlive the one handshake that reads and writes
/// it, so it's dropped (and cleaned up) right here rather than held for the
/// caller's whole test.
async fn connected(root: &Path) -> Router {
    let stream = sftp_server::serve(root.to_path_buf()).await;
    let known_hosts = TempDir::new().unwrap();
    let router = Router::new();
    timeout(
        Duration::from_secs(10),
        router.connect_stream(
            AUTHORITY,
            stream,
            USER,
            &Auth::Password(PASSWORD.into()),
            &known_hosts.path().join("known_hosts"),
        ),
    )
    .await
    .expect("connecting hung")
    .expect("connecting failed");
    router
}

fn remote(path: &str) -> VPath {
    VPath::remote("sftp", AUTHORITY, path).unwrap()
}

#[tokio::test]
async fn it_connects_over_a_real_tcp_socket_not_just_the_test_pipe() {
    // Every other test in this file hands `connect_stream` an in-memory
    // pipe. This one hands it a genuine `TcpStream` instead — the same type
    // `Router::connect`'s own three-line `TcpStream::connect` produces for a
    // real server — so the handshake, auth and SFTP layers above the
    // transport all get proven against a real socket at least once, without
    // going through `connect` itself and its real `~/.ssh/known_hosts` (see
    // `connected`, above, for why every test avoids that).
    let tmp = TempDir::new().unwrap();
    std::fs::write(tmp.path().join("over-the-wire.txt"), b"hello, socket").unwrap();
    let port = sftp_server::serve_tcp(tmp.path().to_path_buf()).await;
    let socket = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();

    let router = Router::new();
    let known_hosts = TempDir::new().unwrap();
    router
        .connect_stream(
            AUTHORITY,
            socket,
            USER,
            &Auth::Password(PASSWORD.into()),
            &known_hosts.path().join("known_hosts"),
        )
        .await
        .unwrap();

    let entries = router.list(&remote("/")).await.unwrap();
    assert!(entries.iter().any(|e| e.name == "over-the-wire.txt"));
}

#[tokio::test]
async fn a_new_host_key_is_learned_in_the_given_known_hosts_file_and_nowhere_else() {
    // Proves `connect_stream`'s `known_hosts` argument is actually where
    // trust is recorded — not silently falling back to the real
    // `~/.ssh/known_hosts`, which nothing here should ever touch.
    let tmp = TempDir::new().unwrap();
    let stream = sftp_server::serve(tmp.path().to_path_buf()).await;
    let known_hosts_dir = TempDir::new().unwrap();
    let known_hosts = known_hosts_dir.path().join("known_hosts");
    assert!(!known_hosts.exists(), "nothing here yet, before connecting");

    let router = Router::new();
    router
        .connect_stream(
            AUTHORITY,
            stream,
            USER,
            &Auth::Password(PASSWORD.into()),
            &known_hosts,
        )
        .await
        .unwrap();

    let learned = std::fs::read_to_string(&known_hosts)
        .expect("the host key should have been written to the given file");
    assert!(learned.contains("test-server"), "{learned}");
}

#[tokio::test]
async fn connecting_returns_a_landing_path_for_the_panel() {
    let tmp = TempDir::new().unwrap();
    let stream = sftp_server::serve(tmp.path().to_path_buf()).await;
    let router = Router::new();

    let known_hosts = TempDir::new().unwrap();
    let landing = router
        .connect_stream(
            AUTHORITY,
            stream,
            USER,
            &Auth::Password(PASSWORD.into()),
            &known_hosts.path().join("known_hosts"),
        )
        .await
        .unwrap();

    assert_eq!(landing, remote("/"));
}

#[tokio::test]
async fn a_wrong_password_is_refused() {
    let tmp = TempDir::new().unwrap();
    let stream = sftp_server::serve(tmp.path().to_path_buf()).await;
    let router = Router::new();

    let known_hosts = TempDir::new().unwrap();
    let err = router
        .connect_stream(
            AUTHORITY,
            stream,
            USER,
            &Auth::Password("nope".into()),
            &known_hosts.path().join("known_hosts"),
        )
        .await
        .unwrap_err();

    assert!(matches!(err, Error::Remote { .. }), "got {err:?}");
}

#[tokio::test]
async fn a_path_nothing_has_connected_to_says_so() {
    let router = Router::new();

    let err = router.list(&remote("/")).await.unwrap_err();

    assert!(matches!(err, Error::NotConnected(_)), "got {err:?}");
}

#[tokio::test]
async fn it_lists_a_remote_folder() {
    let tmp = TempDir::new().unwrap();
    std::fs::write(tmp.path().join("notes.txt"), b"hello").unwrap();
    std::fs::create_dir(tmp.path().join("photos")).unwrap();
    let router = connected(tmp.path()).await;

    let entries = router.list(&remote("/")).await.unwrap();

    let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    assert!(names.contains(&"notes.txt"));
    assert!(names.contains(&"photos"));
    let notes = entries.iter().find(|e| e.name == "notes.txt").unwrap();
    assert_eq!(notes.kind, EntryKind::File);
    assert_eq!(notes.size, 5);
    let photos = entries.iter().find(|e| e.name == "photos").unwrap();
    assert_eq!(photos.kind, EntryKind::Dir);
}

#[tokio::test]
async fn it_reads_and_writes_a_remote_file() {
    let tmp = TempDir::new().unwrap();
    let router = connected(tmp.path()).await;

    let mut writer = router.open_write(&remote("/new.txt")).await.unwrap();
    tokio::io::AsyncWriteExt::write_all(&mut writer, b"written over sftp")
        .await
        .unwrap();
    tokio::io::AsyncWriteExt::shutdown(&mut writer)
        .await
        .unwrap();
    drop(writer);

    assert_eq!(
        std::fs::read(tmp.path().join("new.txt")).unwrap(),
        b"written over sftp"
    );

    let mut reader = router.open_read(&remote("/new.txt")).await.unwrap();
    let mut got = Vec::new();
    tokio::io::AsyncReadExt::read_to_end(&mut reader, &mut got)
        .await
        .unwrap();
    assert_eq!(got, b"written over sftp");
}

#[tokio::test]
async fn it_creates_folders_and_removes_things() {
    let tmp = TempDir::new().unwrap();
    let router = connected(tmp.path()).await;

    router.create_dir(&remote("/sub")).await.unwrap();
    assert!(tmp.path().join("sub").is_dir());

    std::fs::write(tmp.path().join("sub/file.txt"), b"x").unwrap();
    router.remove_file(&remote("/sub/file.txt")).await.unwrap();
    assert!(!tmp.path().join("sub/file.txt").exists());

    router.remove_dir(&remote("/sub")).await.unwrap();
    assert!(!tmp.path().join("sub").exists());
}

#[tokio::test]
async fn it_renames_within_one_server() {
    let tmp = TempDir::new().unwrap();
    std::fs::write(tmp.path().join("old.txt"), b"x").unwrap();
    let router = connected(tmp.path()).await;

    router
        .rename(&remote("/old.txt"), &remote("/new.txt"))
        .await
        .unwrap();

    assert!(!tmp.path().join("old.txt").exists());
    assert!(tmp.path().join("new.txt").exists());
}

#[tokio::test]
async fn renaming_to_a_different_server_is_reported_as_crossing_devices() {
    // Two separate servers (and so two separate authorities), each backed by
    // its own temp directory. There's no such thing as a single rename
    // across them; the job engine's own fallback (copy, then delete) is
    // exactly what `CrossesDevices` exists to trigger.
    let tmp = TempDir::new().unwrap();
    std::fs::write(tmp.path().join("a.txt"), b"x").unwrap();
    let router = connected(tmp.path()).await;

    let other_tmp = TempDir::new().unwrap();
    let other_stream = sftp_server::serve(other_tmp.path().to_path_buf()).await;
    const OTHER: &str = "tester@other-server:22";
    let other_known_hosts = TempDir::new().unwrap();
    router
        .connect_stream(
            OTHER,
            other_stream,
            USER,
            &Auth::Password(PASSWORD.into()),
            &other_known_hosts.path().join("known_hosts"),
        )
        .await
        .unwrap();
    let elsewhere = VPath::remote("sftp", OTHER, "/a.txt").unwrap();

    let err = router
        .rename(&remote("/a.txt"), &elsewhere)
        .await
        .unwrap_err();

    assert!(matches!(err, Error::CrossesDevices(_)), "got {err:?}");
    assert!(
        tmp.path().join("a.txt").exists(),
        "nothing should have moved"
    );
}

#[tokio::test]
async fn a_missing_file_is_reported_as_not_found() {
    let tmp = TempDir::new().unwrap();
    let router = connected(tmp.path()).await;

    let err = router.stat(&remote("/nope.txt")).await.unwrap_err();

    assert!(matches!(err, Error::NotFound(_)), "got {err:?}");
}

#[tokio::test]
async fn hidden_files_are_flagged_the_unix_way() {
    let tmp = TempDir::new().unwrap();
    std::fs::write(tmp.path().join(".secret"), b"x").unwrap();
    let router = connected(tmp.path()).await;

    let entries = router.list(&remote("/")).await.unwrap();

    assert!(entries.iter().find(|e| e.name == ".secret").unwrap().hidden);
}

#[cfg(unix)]
#[tokio::test]
async fn a_symlink_is_described_and_its_target_read_back() {
    let tmp = TempDir::new().unwrap();
    std::fs::write(tmp.path().join("target.txt"), b"the real file").unwrap();
    std::os::unix::fs::symlink("target.txt", tmp.path().join("link.txt")).unwrap();
    let router = connected(tmp.path()).await;

    let entries = router.list(&remote("/")).await.unwrap();
    let link = entries.iter().find(|e| e.name == "link.txt").unwrap();

    assert!(
        matches!(
            link.kind,
            EntryKind::Symlink {
                target: Some(manager_core::LinkTarget::File)
            }
        ),
        "{:?}",
        link.kind
    );
}

#[cfg(unix)]
#[tokio::test]
async fn set_modified_and_set_permissions_reach_the_real_file() {
    let tmp = TempDir::new().unwrap();
    std::fs::write(tmp.path().join("a.txt"), b"x").unwrap();
    let router = connected(tmp.path()).await;

    let when = std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    router.set_modified(&remote("/a.txt"), when).await.unwrap();
    let meta = std::fs::metadata(tmp.path().join("a.txt")).unwrap();
    assert_eq!(meta.modified().unwrap(), when);

    router
        .set_permissions(
            &remote("/a.txt"),
            manager_core::Permissions {
                readonly: false,
                unix_mode: Some(0o640),
            },
        )
        .await
        .unwrap();
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(tmp.path().join("a.txt"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o640);
}

#[tokio::test]
async fn a_folder_is_copied_out_over_sftp_through_the_ordinary_job_engine() {
    use manager_core::jobs::{self, JobEvent, JobSpec, Outcome};
    use tokio::sync::mpsc;

    let server_root = TempDir::new().unwrap();
    std::fs::write(server_root.path().join("a.txt"), b"hello from the server").unwrap();
    std::fs::create_dir(server_root.path().join("sub")).unwrap();
    std::fs::write(server_root.path().join("sub/b.txt"), b"nested").unwrap();
    let router = Arc::new(connected(server_root.path()).await);

    let local_out = TempDir::new().unwrap();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let _handle = jobs::start(
        Arc::clone(&router) as Arc<dyn Vfs>,
        JobSpec::Copy {
            sources: vec![remote("/a.txt"), remote("/sub")],
            dest: VPath::local(local_out.path()).unwrap(),
        },
        tx,
    );
    // The destination is empty, so the only event a healthy job can
    // produce is the final report.
    let event = timeout(Duration::from_secs(20), rx.recv())
        .await
        .expect("the job hung")
        .expect("the job vanished");
    let outcome = match event {
        JobEvent::Finished(report) => report.outcome,
        JobEvent::Conflict(q) => panic!("unexpected conflict on {}", q.source.path),
        JobEvent::Error(q) => panic!("unexpected error: {}", q.error),
    };

    assert_eq!(outcome, Outcome::Completed);
    assert_eq!(
        std::fs::read_to_string(local_out.path().join("a.txt")).unwrap(),
        "hello from the server"
    );
    assert_eq!(
        std::fs::read_to_string(local_out.path().join("sub/b.txt")).unwrap(),
        "nested"
    );
}
