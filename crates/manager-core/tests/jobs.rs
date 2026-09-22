//! Tests for the job engine, run against real temporary folders.

use std::collections::{BTreeMap, VecDeque};
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use manager_core::jobs::{
    self, ConflictAction, ConflictAnswer, ErrorAnswer, JobEvent, JobReport, JobSpec, Outcome,
    Phase, Progress,
};
use manager_core::{
    Capabilities, Entry, Error, LocalFs, Permissions, ReadStream, Result, VPath, Vfs, WriteStream,
};
use tempfile::TempDir;
use tokio::io::{AsyncWriteExt, DuplexStream};
use tokio::sync::mpsc;
use tokio::time::timeout;

// ---------------------------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------------------------

fn vp(path: impl AsRef<Path>) -> VPath {
    VPath::local(path).unwrap()
}

fn write(path: impl AsRef<Path>, content: &str) {
    let path = path.as_ref();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

fn set_mtime(path: impl AsRef<Path>, secs_ago: u64) {
    let file = fs::File::options().write(true).open(path).unwrap();
    file.set_modified(SystemTime::now() - Duration::from_secs(secs_ago))
        .unwrap();
}

/// Every file and folder under `dir`: relative path → file content (`None` for folders).
fn tree(dir: impl AsRef<Path>) -> BTreeMap<String, Option<String>> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, Option<String>>) {
        for item in fs::read_dir(dir).unwrap() {
            let path = item.unwrap().path();
            let rel = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            if path.is_dir() && !path.is_symlink() {
                out.insert(rel, None);
                walk(root, &path, out);
            } else {
                out.insert(rel, fs::read_to_string(&path).ok());
            }
        }
    }
    let mut out = BTreeMap::new();
    if dir.as_ref().exists() {
        walk(dir.as_ref(), dir.as_ref(), &mut out);
    }
    out
}

fn files(pairs: &[(&str, Option<&str>)]) -> BTreeMap<String, Option<String>> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.map(str::to_string)))
        .collect()
}

/// Pre-recorded answers for the questions a job will ask.
#[derive(Default)]
struct Script {
    conflicts: VecDeque<ConflictAnswer>,
    errors: VecDeque<ErrorAnswer>,
}

impl Script {
    fn conflict(mut self, action: ConflictAction, apply_to_all: bool) -> Self {
        self.conflicts.push_back(ConflictAnswer {
            action,
            apply_to_all,
        });
        self
    }

    fn error(mut self, answer: ErrorAnswer) -> Self {
        self.errors.push_back(answer);
        self
    }
}

/// What happened during a job.
#[derive(Debug)]
struct Ran {
    report: JobReport,
    progress: Progress,
    /// Names of the sources we were asked about.
    conflicts: Vec<String>,
    /// Error messages we were asked about.
    errors: Vec<String>,
}

async fn run(vfs: Arc<dyn Vfs>, spec: JobSpec, mut script: Script) -> Ran {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let handle = jobs::start(vfs, spec, tx);
    let mut conflicts = Vec::new();
    let mut errors = Vec::new();
    loop {
        let event = timeout(Duration::from_secs(20), rx.recv())
            .await
            .expect("job hung")
            .expect("job vanished without a report");
        match event {
            JobEvent::Conflict(q) => {
                conflicts.push(q.source.name.clone());
                let answer = script.conflicts.pop_front().unwrap_or_else(|| {
                    panic!(
                        "unexpected conflict: {} vs {}",
                        q.source.path, q.existing.path
                    )
                });
                q.answer(answer);
            }
            JobEvent::Error(q) => {
                errors.push(q.error.to_string());
                let answer = script
                    .errors
                    .pop_front()
                    .unwrap_or_else(|| panic!("unexpected error: {}", q.error));
                q.answer(answer);
            }
            JobEvent::Finished(report) => {
                return Ran {
                    report,
                    progress: handle.progress(),
                    conflicts,
                    errors,
                };
            }
        }
    }
}

fn local() -> Arc<dyn Vfs> {
    Arc::new(LocalFs)
}

fn copy(sources: &[&Path], dest: &Path) -> JobSpec {
    JobSpec::Copy {
        sources: sources.iter().map(vp).collect(),
        dest: vp(dest),
    }
}

fn mv(sources: &[&Path], dest: &Path) -> JobSpec {
    JobSpec::Move {
        sources: sources.iter().map(vp).collect(),
        dest: vp(dest),
    }
}

fn assert_no_part_files(dir: &Path) {
    let leftovers: Vec<_> = tree(dir)
        .into_keys()
        .filter(|k| k.ends_with(".part"))
        .collect();
    assert!(
        leftovers.is_empty(),
        "temp files left behind: {leftovers:?}"
    );
}

/// A local file system with knobs for simulating trouble.
#[derive(Default)]
struct TestFs {
    /// `rename` behaves as if source and destination were on different disks.
    cross_device: bool,
    /// `create_dir` fails this many times before working.
    create_dir_failures: AtomicU32,
    /// `open_read` always fails.
    unreadable: bool,
    /// What `trash` was called with.
    trashed: Mutex<Vec<VPath>>,
    /// If set, the next `open_read` returns this pipe instead of the real file.
    slow_reader: Mutex<Option<DuplexStream>>,
}

#[async_trait]
impl Vfs for TestFs {
    fn name(&self) -> &str {
        "test"
    }
    fn capabilities(&self) -> Capabilities {
        LocalFs.capabilities()
    }
    async fn list(&self, path: &VPath) -> Result<Vec<Entry>> {
        LocalFs.list(path).await
    }
    async fn stat(&self, path: &VPath) -> Result<Entry> {
        LocalFs.stat(path).await
    }
    async fn create_dir(&self, path: &VPath) -> Result<()> {
        let left = self.create_dir_failures.load(Ordering::SeqCst);
        if left > 0 {
            self.create_dir_failures.store(left - 1, Ordering::SeqCst);
            return Err(Error::PermissionDenied(path.clone()));
        }
        LocalFs.create_dir(path).await
    }
    async fn open_read(&self, path: &VPath) -> Result<ReadStream> {
        if self.unreadable {
            return Err(Error::PermissionDenied(path.clone()));
        }
        if let Some(pipe) = self.slow_reader.lock().unwrap().take() {
            return Ok(Box::new(pipe));
        }
        LocalFs.open_read(path).await
    }
    async fn open_write(&self, path: &VPath) -> Result<WriteStream> {
        LocalFs.open_write(path).await
    }
    async fn rename(&self, from: &VPath, to: &VPath) -> Result<()> {
        // Our own temp files are renamed within one folder, which always works.
        let is_temp = from.file_name().is_some_and(|n| n.ends_with(".part"));
        if self.cross_device && !is_temp {
            return Err(Error::CrossesDevices(from.clone()));
        }
        LocalFs.rename(from, to).await
    }
    async fn remove_file(&self, path: &VPath) -> Result<()> {
        LocalFs.remove_file(path).await
    }
    async fn remove_dir(&self, path: &VPath) -> Result<()> {
        LocalFs.remove_dir(path).await
    }
    async fn trash(&self, path: &VPath) -> Result<()> {
        if path.file_name().as_deref() == Some("stuck") {
            return Err(Error::PermissionDenied(path.clone()));
        }
        self.trashed.lock().unwrap().push(path.clone());
        Ok(())
    }
    async fn create_symlink(&self, target: &Path, link: &VPath, is_dir: bool) -> Result<()> {
        LocalFs.create_symlink(target, link, is_dir).await
    }
    async fn set_modified(&self, path: &VPath, time: SystemTime) -> Result<()> {
        LocalFs.set_modified(path, time).await
    }
    async fn set_permissions(&self, path: &VPath, permissions: Permissions) -> Result<()> {
        LocalFs.set_permissions(path, permissions).await
    }
}

// ---------------------------------------------------------------------------------------------
// Copy
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn copies_a_file_into_a_folder_keeping_its_date() {
    let tmp = TempDir::new().unwrap();
    let (src, dest) = (tmp.path().join("a"), tmp.path().join("b"));
    write(src.join("hello.txt"), "hello world");
    set_mtime(src.join("hello.txt"), 3600);
    fs::create_dir(&dest).unwrap();

    let ran = run(
        local(),
        copy(&[&src.join("hello.txt")], &dest),
        Script::default(),
    )
    .await;

    assert_eq!(ran.report.outcome, Outcome::Completed);
    assert_eq!(ran.report.items, 1);
    assert_eq!(ran.report.bytes, 11);
    assert_eq!(
        fs::read_to_string(dest.join("hello.txt")).unwrap(),
        "hello world"
    );
    assert!(
        src.join("hello.txt").exists(),
        "copy leaves the source alone"
    );

    let times = |p: &Path| fs::metadata(p).unwrap().modified().unwrap();
    assert_eq!(
        times(&dest.join("hello.txt")),
        times(&src.join("hello.txt"))
    );

    assert_eq!(ran.progress.phase, Phase::Done);
    assert_eq!(ran.progress.done_bytes, ran.progress.total_bytes);
    assert_eq!(ran.progress.fraction(), 1.0);
    assert_no_part_files(tmp.path());
}

#[tokio::test]
async fn copies_a_whole_tree() {
    let tmp = TempDir::new().unwrap();
    let src = tmp.path().join("project");
    write(src.join("readme.md"), "hi");
    write(src.join("src/main.rs"), "fn main() {}");
    write(src.join("src/deep/er/x.txt"), "x");
    fs::create_dir_all(src.join("empty")).unwrap();
    let dest = tmp.path().join("backup");
    fs::create_dir(&dest).unwrap();

    let ran = run(local(), copy(&[&src], &dest), Script::default()).await;

    assert_eq!(tree(dest.join("project")), tree(&src));
    assert_eq!(ran.report.items, 8, "3 files + 5 folders");
    assert_eq!(ran.progress.total_items, 8);
    assert_eq!(ran.progress.done_items, 8);
    assert_eq!(ran.progress.done_bytes, 2 + 12 + 1);
}

#[tokio::test]
async fn copies_a_single_file_under_a_new_name() {
    let tmp = TempDir::new().unwrap();
    write(tmp.path().join("a.txt"), "A");
    run(
        local(),
        copy(&[&tmp.path().join("a.txt")], &tmp.path().join("b.txt")),
        Script::default(),
    )
    .await;
    assert_eq!(fs::read_to_string(tmp.path().join("b.txt")).unwrap(), "A");
}

#[tokio::test]
async fn copying_several_items_to_a_missing_folder_creates_it() {
    let tmp = TempDir::new().unwrap();
    write(tmp.path().join("1.txt"), "1");
    write(tmp.path().join("2.txt"), "2");
    let dest = tmp.path().join("new/place");
    fs::create_dir(tmp.path().join("new")).unwrap();

    run(
        local(),
        copy(
            &[&tmp.path().join("1.txt"), &tmp.path().join("2.txt")],
            &dest,
        ),
        Script::default(),
    )
    .await;
    assert_eq!(
        tree(&dest),
        files(&[("1.txt", Some("1")), ("2.txt", Some("2"))])
    );
}

#[tokio::test]
async fn refuses_to_copy_a_folder_into_itself() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("dir");
    write(dir.join("x.txt"), "x");

    let ran = run(
        local(),
        copy(&[&dir], &dir.join("sub")),
        Script::default().error(ErrorAnswer::Skip),
    )
    .await;
    assert!(ran.errors[0].contains("inside itself"), "{:?}", ran.errors);
    assert_eq!(ran.report.skipped, 1);
    assert_eq!(tree(&dir), files(&[("x.txt", Some("x"))]));
}

#[tokio::test]
async fn refuses_to_copy_a_file_onto_itself() {
    let tmp = TempDir::new().unwrap();
    write(tmp.path().join("x.txt"), "x");
    let ran = run(
        local(),
        copy(&[&tmp.path().join("x.txt")], tmp.path()),
        Script::default().error(ErrorAnswer::Skip),
    )
    .await;
    assert!(ran.errors[0].contains("onto itself"), "{:?}", ran.errors);
    assert_eq!(fs::read_to_string(tmp.path().join("x.txt")).unwrap(), "x");
}

// ---------------------------------------------------------------------------------------------
// Conflicts
// ---------------------------------------------------------------------------------------------

/// `src/{a,b,c}.txt` = "new", `dest/{a,b,c}.txt` = "old".
fn conflict_setup() -> (TempDir, std::path::PathBuf, std::path::PathBuf) {
    let tmp = TempDir::new().unwrap();
    let (src, dest) = (tmp.path().join("src"), tmp.path().join("dest"));
    for name in ["a.txt", "b.txt", "c.txt"] {
        write(src.join(name), "new");
        write(dest.join(name), "old");
    }
    (tmp, src, dest)
}

fn all_in(src: &Path) -> Vec<std::path::PathBuf> {
    ["a.txt", "b.txt", "c.txt"]
        .iter()
        .map(|n| src.join(n))
        .collect()
}

async fn copy_all(src: &Path, dest: &Path, script: Script) -> Ran {
    let sources = all_in(src);
    let refs: Vec<&Path> = sources.iter().map(|p| p.as_path()).collect();
    run(local(), copy(&refs, dest), script).await
}

#[tokio::test]
async fn conflict_answers_apply_per_file() {
    let (_tmp, src, dest) = conflict_setup();
    let script = Script::default()
        .conflict(ConflictAction::Overwrite, false)
        .conflict(ConflictAction::Skip, false)
        .conflict(ConflictAction::Rename, false);

    let ran = copy_all(&src, &dest, script).await;

    assert_eq!(ran.conflicts, ["a.txt", "b.txt", "c.txt"]);
    assert_eq!(
        tree(&dest),
        files(&[
            ("a.txt", Some("new")),
            ("b.txt", Some("old")),
            ("c (2).txt", Some("new")),
            ("c.txt", Some("old")),
        ])
    );
    assert_eq!(ran.report.skipped, 1);
    assert_no_part_files(&dest);
}

#[tokio::test]
async fn apply_to_all_asks_only_once() {
    let (_tmp, src, dest) = conflict_setup();
    let ran = copy_all(
        &src,
        &dest,
        Script::default().conflict(ConflictAction::Overwrite, true),
    )
    .await;
    assert_eq!(ran.conflicts.len(), 1);
    assert!(tree(&dest).values().all(|v| v.as_deref() == Some("new")));
}

#[tokio::test]
async fn overwrite_older_only_replaces_older_files() {
    let (_tmp, src, dest) = conflict_setup();
    set_mtime(dest.join("a.txt"), 7200); // older than the source: replaced
    set_mtime(src.join("b.txt"), 7200); // source is older: kept
    set_mtime(src.join("c.txt"), 7200);

    copy_all(
        &src,
        &dest,
        Script::default().conflict(ConflictAction::OverwriteOlder, true),
    )
    .await;
    assert_eq!(
        tree(&dest),
        files(&[
            ("a.txt", Some("new")),
            ("b.txt", Some("old")),
            ("c.txt", Some("old")),
        ])
    );
}

#[tokio::test]
async fn cancel_from_the_conflict_dialog_stops_everything() {
    let (_tmp, src, dest) = conflict_setup();
    let ran = copy_all(
        &src,
        &dest,
        Script::default().conflict(ConflictAction::Cancel, false),
    )
    .await;
    assert_eq!(ran.report.outcome, Outcome::Cancelled);
    assert!(tree(&dest).values().all(|v| v.as_deref() == Some("old")));
}

#[tokio::test]
async fn folders_are_merged_without_asking() {
    let tmp = TempDir::new().unwrap();
    write(tmp.path().join("src/photos/new.jpg"), "new");
    write(tmp.path().join("dest/photos/old.jpg"), "old");

    run(
        local(),
        copy(&[&tmp.path().join("src/photos")], &tmp.path().join("dest")),
        Script::default(),
    )
    .await;
    assert_eq!(
        tree(tmp.path().join("dest")),
        files(&[
            ("photos", None),
            ("photos/new.jpg", Some("new")),
            ("photos/old.jpg", Some("old")),
        ])
    );
}

// ---------------------------------------------------------------------------------------------
// Move
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn move_on_the_same_disk_renames() {
    let tmp = TempDir::new().unwrap();
    write(tmp.path().join("src/dir/a.txt"), "a");
    fs::create_dir(tmp.path().join("dest")).unwrap();

    let ran = run(
        local(),
        mv(&[&tmp.path().join("src/dir")], &tmp.path().join("dest")),
        Script::default(),
    )
    .await;
    assert!(!tmp.path().join("src/dir").exists());
    assert_eq!(
        tree(tmp.path().join("dest")),
        files(&[("dir", None), ("dir/a.txt", Some("a"))])
    );
    assert_eq!(ran.progress.done_items, ran.progress.total_items);
}

#[tokio::test]
async fn move_across_disks_copies_then_deletes() {
    let tmp = TempDir::new().unwrap();
    write(tmp.path().join("src/dir/a.txt"), "a");
    write(tmp.path().join("src/dir/sub/b.txt"), "b");
    fs::create_dir(tmp.path().join("dest")).unwrap();
    let fs_ = Arc::new(TestFs {
        cross_device: true,
        ..Default::default()
    });

    let ran = run(
        fs_,
        mv(&[&tmp.path().join("src/dir")], &tmp.path().join("dest")),
        Script::default(),
    )
    .await;
    assert_eq!(ran.report.outcome, Outcome::Completed);
    assert_eq!(tree(tmp.path().join("src")), files(&[]));
    assert_eq!(
        tree(tmp.path().join("dest")),
        files(&[
            ("dir", None),
            ("dir/a.txt", Some("a")),
            ("dir/sub", None),
            ("dir/sub/b.txt", Some("b")),
        ])
    );
}

#[tokio::test]
async fn move_keeps_what_was_skipped() {
    let tmp = TempDir::new().unwrap();
    write(tmp.path().join("src/dir/keep.txt"), "mine");
    write(tmp.path().join("src/dir/go.txt"), "go");
    write(tmp.path().join("dest/dir/keep.txt"), "theirs");

    run(
        local(),
        mv(&[&tmp.path().join("src/dir")], &tmp.path().join("dest")),
        Script::default().conflict(ConflictAction::Skip, false),
    )
    .await;
    assert_eq!(
        tree(tmp.path().join("src")),
        files(&[("dir", None), ("dir/keep.txt", Some("mine"))]),
        "the skipped file and its folder stay behind"
    );
    assert_eq!(
        tree(tmp.path().join("dest")),
        files(&[
            ("dir", None),
            ("dir/go.txt", Some("go")),
            ("dir/keep.txt", Some("theirs")),
        ])
    );
}

// ---------------------------------------------------------------------------------------------
// Delete
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn permanent_delete_removes_a_tree() {
    let tmp = TempDir::new().unwrap();
    write(tmp.path().join("gone/a.txt"), "a");
    write(tmp.path().join("gone/deep/b.txt"), "bb");
    write(tmp.path().join("stays.txt"), "s");

    let ran = run(
        local(),
        JobSpec::Delete {
            targets: vec![vp(tmp.path().join("gone"))],
            permanent: true,
        },
        Script::default(),
    )
    .await;
    assert_eq!(tree(tmp.path()), files(&[("stays.txt", Some("s"))]));
    assert_eq!(ran.report.items, 4);
    assert_eq!(ran.progress.done_bytes, 3);
    assert_eq!(ran.progress.fraction(), 1.0);
}

#[tokio::test]
async fn delete_to_trash_uses_the_trash_and_can_skip_failures() {
    let tmp = TempDir::new().unwrap();
    let fs_ = Arc::new(TestFs::default());
    let targets = vec![
        vp(tmp.path().join("a")),
        vp(tmp.path().join("stuck")),
        vp(tmp.path().join("b")),
    ];

    let ran = run(
        fs_.clone(),
        JobSpec::Delete {
            targets: targets.clone(),
            permanent: false,
        },
        Script::default().error(ErrorAnswer::Skip),
    )
    .await;
    assert_eq!(
        *fs_.trashed.lock().unwrap(),
        [targets[0].clone(), targets[2].clone()]
    );
    assert_eq!(ran.report.items, 2);
    assert_eq!(ran.report.skipped, 1);
}

#[tokio::test]
async fn never_deletes_a_root() {
    let ran = run(
        Arc::new(TestFs::default()),
        JobSpec::Delete {
            targets: vec![VPath::parse("sftp://host/").unwrap()],
            permanent: false,
        },
        Script::default().error(ErrorAnswer::Skip),
    )
    .await;
    assert!(ran.errors[0].contains("refusing"), "{:?}", ran.errors);
}

// ---------------------------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn retry_after_an_error() {
    let tmp = TempDir::new().unwrap();
    write(tmp.path().join("dir/a.txt"), "a");
    fs::create_dir(tmp.path().join("dest")).unwrap();
    let fs_ = Arc::new(TestFs {
        create_dir_failures: AtomicU32::new(1),
        ..Default::default()
    });

    let ran = run(
        fs_,
        copy(&[&tmp.path().join("dir")], &tmp.path().join("dest")),
        Script::default().error(ErrorAnswer::Retry),
    )
    .await;
    assert_eq!(ran.errors.len(), 1);
    assert!(ran.errors[0].contains("permission denied"));
    assert_eq!(
        tree(tmp.path().join("dest")),
        files(&[("dir", None), ("dir/a.txt", Some("a"))])
    );
}

#[tokio::test]
async fn skip_all_stops_asking() {
    let (_tmp, src, dest) = conflict_setup();
    let fresh = TempDir::new().unwrap();
    let fs_ = Arc::new(TestFs {
        unreadable: true,
        ..Default::default()
    });
    let sources = all_in(&src);
    let refs: Vec<&Path> = sources.iter().map(|p| p.as_path()).collect();

    let ran = run(
        fs_,
        copy(&refs, fresh.path()),
        Script::default().error(ErrorAnswer::SkipAll),
    )
    .await;
    assert_eq!(ran.errors.len(), 1);
    assert_eq!(ran.report.skipped, 3);
    assert_eq!(ran.report.outcome, Outcome::Completed);
    assert_eq!(tree(fresh.path()), files(&[]));
    drop(dest);
}

// ---------------------------------------------------------------------------------------------
// Pause and cancel
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn pause_stops_work_and_cancel_ends_the_job() {
    let tmp = TempDir::new().unwrap();
    write(tmp.path().join("a.txt"), "a");
    let (tx, mut rx) = mpsc::unbounded_channel();

    let handle = jobs::start(
        local(),
        copy(&[&tmp.path().join("a.txt")], &tmp.path().join("b.txt")),
        tx,
    );
    handle.pause(); // before the job even gets to run
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(handle.is_paused());
    assert!(handle.progress().paused);
    assert!(!tmp.path().join("b.txt").exists());

    handle.cancel();
    handle.resume(); // cancelling is final: this does nothing
    let Some(JobEvent::Finished(report)) = rx.recv().await else {
        panic!()
    };
    assert_eq!(report.outcome, Outcome::Cancelled);
    assert!(!tmp.path().join("b.txt").exists());
}

#[tokio::test]
async fn resume_continues_after_pause() {
    let tmp = TempDir::new().unwrap();
    write(tmp.path().join("a.txt"), "a");
    let (tx, mut rx) = mpsc::unbounded_channel();

    let handle = jobs::start(
        local(),
        copy(&[&tmp.path().join("a.txt")], &tmp.path().join("b.txt")),
        tx,
    );
    handle.pause();
    tokio::time::sleep(Duration::from_millis(20)).await;
    handle.resume();
    let Some(JobEvent::Finished(report)) = rx.recv().await else {
        panic!()
    };
    assert_eq!(report.outcome, Outcome::Completed);
    assert!(tmp.path().join("b.txt").exists());
}

#[tokio::test]
async fn dropping_the_handle_cancels() {
    let tmp = TempDir::new().unwrap();
    write(tmp.path().join("a.txt"), "a");
    let (tx, mut rx) = mpsc::unbounded_channel();
    let handle = jobs::start(
        local(),
        copy(&[&tmp.path().join("a.txt")], &tmp.path().join("b.txt")),
        tx,
    );
    handle.pause();
    drop(handle);
    let Some(JobEvent::Finished(report)) = rx.recv().await else {
        panic!()
    };
    assert_eq!(report.outcome, Outcome::Cancelled);
}

/// The source file is a pipe the test controls, so we can cancel while a file
/// is half-written and check nothing half-written is left behind.
#[tokio::test]
async fn cancel_mid_file_leaves_no_partial_file() {
    let tmp = TempDir::new().unwrap();
    write(tmp.path().join("big.bin"), &"x".repeat(100_000));
    let (mut feed, pipe) = tokio::io::duplex(64 * 1024);
    let fs_ = Arc::new(TestFs {
        slow_reader: Mutex::new(Some(pipe)),
        ..Default::default()
    });
    let (tx, mut rx) = mpsc::unbounded_channel();
    let handle = jobs::start(
        fs_,
        copy(&[&tmp.path().join("big.bin")], &tmp.path().join("copy.bin")),
        tx,
    );

    feed.write_all(&[b'x'; 1000]).await.unwrap();
    timeout(Duration::from_secs(10), async {
        while handle.progress().done_bytes == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("copy never started");
    assert!(
        tmp.path().join(".copy.bin.part").exists(),
        "writing to a temp file"
    );

    handle.cancel();
    feed.write_all(&[b'x'; 1000]).await.unwrap(); // unblock the read so it sees the cancel

    let Some(JobEvent::Finished(report)) = rx.recv().await else {
        panic!()
    };
    assert_eq!(report.outcome, Outcome::Cancelled);
    assert!(!tmp.path().join("copy.bin").exists());
    assert_no_part_files(tmp.path());
    assert_eq!(
        handle.progress().done_bytes,
        0,
        "progress of the lost file is taken back"
    );
}

// ---------------------------------------------------------------------------------------------
// Links and small things
// ---------------------------------------------------------------------------------------------

#[cfg(unix)]
#[tokio::test]
async fn symlinks_are_copied_as_links() {
    let tmp = TempDir::new().unwrap();
    write(tmp.path().join("src/real.txt"), "real");
    std::os::unix::fs::symlink("real.txt", tmp.path().join("src/link")).unwrap();
    fs::create_dir(tmp.path().join("dest")).unwrap();

    run(
        local(),
        copy(&[&tmp.path().join("src/link")], &tmp.path().join("dest")),
        Script::default(),
    )
    .await;
    let copied = tmp.path().join("dest/link");
    assert!(copied.is_symlink());
    assert_eq!(fs::read_link(copied).unwrap(), Path::new("real.txt"));
}

#[cfg(unix)]
#[tokio::test]
async fn permissions_are_kept() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = TempDir::new().unwrap();
    write(tmp.path().join("run.sh"), "#!/bin/sh");
    fs::set_permissions(tmp.path().join("run.sh"), fs::Permissions::from_mode(0o751)).unwrap();

    run(
        local(),
        copy(&[&tmp.path().join("run.sh")], &tmp.path().join("copy.sh")),
        Script::default(),
    )
    .await;
    let mode = fs::metadata(tmp.path().join("copy.sh"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o751);
}

#[test]
fn titles() {
    let a = VPath::parse("sftp://h/a.txt").unwrap();
    let b = VPath::parse("sftp://h/b.txt").unwrap();
    let dest = VPath::parse("sftp://h/dest").unwrap();
    let spec = |sources: Vec<VPath>| JobSpec::Copy {
        sources,
        dest: dest.clone(),
    };
    assert_eq!(spec(vec![a.clone()]).title(), "Copy a.txt → sftp://h/dest");
    assert_eq!(
        spec(vec![a.clone(), b]).title(),
        "Copy 2 items → sftp://h/dest"
    );
    let del = JobSpec::Delete {
        targets: vec![a],
        permanent: false,
    };
    assert_eq!(del.title(), "Trash a.txt");
}

#[test]
fn progress_math() {
    let mut p = Progress {
        total_bytes: 200,
        done_bytes: 50,
        elapsed: Duration::from_secs(2),
        ..Default::default()
    };
    assert_eq!(p.fraction(), 0.25);
    assert_eq!(p.bytes_per_second(), 25.0);
    p.total_bytes = 0;
    p.total_items = 4;
    p.done_items = 1;
    assert_eq!(p.fraction(), 0.25, "falls back to counting items");
    p.total_items = 0;
    assert_eq!(p.fraction(), 0.0);
    p.phase = Phase::Done;
    assert_eq!(p.fraction(), 1.0);
}
