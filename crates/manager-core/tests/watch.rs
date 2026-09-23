//! Tests that watch real, throwaway directories on disk.
//!
//! These are timing tests: they wait for the operating system to tell us about
//! a change. `wait` allows plenty of time so a busy CI machine doesn't fail,
//! while `expect_quiet` waits only briefly, because it's proving a negative.

use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::Duration;
use std::{fs, thread};

use manager_core::watch::WatchSink;
use manager_core::{Error, LocalFs, VPath, Vfs};
use tempfile::TempDir;

/// How long to wait for a change we expect. Generous: CI machines are slow.
const WAIT: Duration = Duration::from_secs(10);
/// How long to wait before believing that nothing is coming.
const QUIET: Duration = Duration::from_millis(750);

fn vp(path: impl AsRef<Path>) -> VPath {
    VPath::local(path).unwrap()
}

/// A sink that puts every reported folder into a channel.
fn sink() -> (WatchSink, Receiver<VPath>) {
    let (tx, rx) = mpsc::channel();
    let sink: WatchSink = Arc::new(move |dir: VPath| {
        // The receiver is gone once the test ends; that's not a failure.
        let _ = tx.send(dir);
    });
    (sink, rx)
}

fn wait(rx: &Receiver<VPath>) -> VPath {
    rx.recv_timeout(WAIT)
        .expect("expected the watcher to report a change")
}

/// Asserts that no change is reported. A disconnected channel counts: it means
/// the watcher (and with it the sink) is gone, so nothing can be reported.
fn expect_quiet(rx: &Receiver<VPath>) {
    match rx.recv_timeout(QUIET) {
        Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => {}
        Ok(dir) => panic!("expected no change, but {dir} was reported"),
    }
}

#[test]
fn local_files_can_be_watched() {
    assert!(LocalFs.capabilities().can_watch);
}

#[tokio::test]
async fn a_new_file_is_reported() {
    let tmp = TempDir::new().unwrap();
    let dir = vp(tmp.path());
    let (sink, rx) = sink();

    let _watch = LocalFs.watch(&dir, sink).await.unwrap();
    fs::write(tmp.path().join("new.txt"), b"hello").unwrap();

    assert_eq!(wait(&rx), dir, "the reported path is the watched folder");
}

#[tokio::test]
async fn a_deleted_file_is_reported() {
    let tmp = TempDir::new().unwrap();
    fs::write(tmp.path().join("doomed.txt"), b"hello").unwrap();
    let dir = vp(tmp.path());
    let (sink, rx) = sink();

    let _watch = LocalFs.watch(&dir, sink).await.unwrap();
    fs::remove_file(tmp.path().join("doomed.txt")).unwrap();

    assert_eq!(wait(&rx), dir);
}

#[tokio::test]
async fn a_renamed_file_is_reported() {
    let tmp = TempDir::new().unwrap();
    fs::write(tmp.path().join("before.txt"), b"hello").unwrap();
    let dir = vp(tmp.path());
    let (sink, rx) = sink();

    let _watch = LocalFs.watch(&dir, sink).await.unwrap();
    fs::rename(tmp.path().join("before.txt"), tmp.path().join("after.txt")).unwrap();

    assert_eq!(wait(&rx), dir);
}

#[tokio::test]
async fn dropping_the_handle_stops_the_watch() {
    let tmp = TempDir::new().unwrap();
    let dir = vp(tmp.path());
    let (sink, rx) = sink();

    let watch = LocalFs.watch(&dir, sink).await.unwrap();
    fs::write(tmp.path().join("first.txt"), b"hello").unwrap();
    wait(&rx);

    drop(watch);
    // Give the watcher thread a moment to shut down before making a change.
    thread::sleep(Duration::from_millis(100));
    while rx.try_recv().is_ok() {} // drain anything already in flight

    fs::write(tmp.path().join("second.txt"), b"hello").unwrap();
    expect_quiet(&rx);
}

#[tokio::test]
async fn a_change_in_the_folder_is_reported_on_every_platform() {
    // The one promise every backend has to keep: touch the folder a panel is
    // showing and the panel hears about it. Whatever else an operating system
    // chooses to tell us on top is its own business.
    let tmp = TempDir::new().unwrap();
    fs::create_dir(tmp.path().join("sub")).unwrap();
    let dir = vp(tmp.path());
    let (sink, rx) = sink();

    let _watch = LocalFs.watch(&dir, sink).await.unwrap();
    fs::write(tmp.path().join("right-here.txt"), b"hello").unwrap();

    assert_eq!(wait(&rx), dir);
}

#[tokio::test]
async fn watching_a_folder_that_is_not_there_fails() {
    let tmp = TempDir::new().unwrap();
    let missing = vp(tmp.path().join("nope"));
    let (sink, _rx) = sink();

    let err = LocalFs.watch(&missing, sink).await.unwrap_err();
    assert!(
        matches!(err, Error::NotFound(_) | Error::Watch { .. }),
        "expected a not-found or watch error, got {err:?}"
    );
}

// The two tests below pin down that *nothing* is reported. That holds where
// the operating system can watch one folder without its subfolders: inotify on
// Linux and `ReadDirectoryChangesW` on Windows. macOS has only FSEvents, which
// is recursive by nature and coalesces events up to a parent folder, so it
// reports more than we asked for. That costs an extra directory listing, which
// `Coalescer` already rate-limits — it doesn't make anything wrong — so the
// strict version is checked where it's real rather than watered down for
// everyone.
#[cfg(not(target_os = "macos"))]
#[tokio::test]
async fn reading_a_file_is_not_a_change() {
    let tmp = TempDir::new().unwrap();
    fs::write(tmp.path().join("quiet.txt"), b"hello").unwrap();
    let dir = vp(tmp.path());
    let (sink, rx) = sink();

    let _watch = LocalFs.watch(&dir, sink).await.unwrap();
    // Opening and reading a file fires access events on Linux. A panel that
    // reloaded on those would reload every time it previewed something.
    let _ = fs::read(tmp.path().join("quiet.txt")).unwrap();

    expect_quiet(&rx);
}

#[cfg(not(target_os = "macos"))]
#[tokio::test]
async fn changes_deep_inside_a_subfolder_are_not_reported() {
    let tmp = TempDir::new().unwrap();
    let deep = tmp.path().join("sub").join("deeper");
    fs::create_dir_all(&deep).unwrap();
    let dir = vp(tmp.path());
    let (sink, rx) = sink();

    // Watching is deliberately shallow: a panel only shows one folder, and
    // watching a whole tree would flood us with events from folders nobody
    // is looking at.
    let _watch = LocalFs.watch(&dir, sink).await.unwrap();
    fs::write(deep.join("buried.txt"), b"hello").unwrap();

    expect_quiet(&rx);
}
