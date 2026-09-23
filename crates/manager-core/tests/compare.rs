//! Comparing real folder trees, real archives, and syncing between them.

use std::fs::{self, File};
use std::io::Write;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use manager_core::compare::{
    CompareEvent, CompareReport, CompareSpec, DiffEntry, DiffStatus, SyncDirection, plan_sync, run,
    sync_jobs,
};
use manager_core::jobs::{self, JobEvent, JobSpec, Outcome};
use manager_core::{Router, VPath, Vfs};
use tempfile::TempDir;
use tokio::sync::mpsc;
use tokio::time::timeout;
use zip::write::{SimpleFileOptions, ZipWriter};

fn vp(path: impl AsRef<Path>) -> VPath {
    VPath::local(path).unwrap()
}

fn write(path: &Path, content: &[u8]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

fn set_mtime(path: &Path, secs_ago: u64) {
    set_mtime_to(path, SystemTime::now() - Duration::from_secs(secs_ago));
}

fn set_mtime_to(path: &Path, time: SystemTime) {
    let file = fs::File::options().write(true).open(path).unwrap();
    file.set_modified(time).unwrap();
}

struct Ran {
    entries: Vec<DiffEntry>,
    report: CompareReport,
}

async fn compare(vfs: Arc<dyn Vfs>, spec: CompareSpec) -> Ran {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let _handle = run::start(vfs, spec, tx);
    let mut entries = Vec::new();
    loop {
        let event = timeout(Duration::from_secs(20), rx.recv())
            .await
            .expect("the comparison hung")
            .expect("the comparison vanished without a report");
        match event {
            CompareEvent::Found(entry) => entries.push(*entry),
            CompareEvent::Finished(report) => {
                entries.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
                return Ran { entries, report };
            }
        }
    }
}

fn spec(left: &Path, right: &Path) -> CompareSpec {
    CompareSpec {
        left: vp(left),
        right: vp(right),
        by_content: false,
        include_hidden: false,
    }
}

fn find<'a>(ran: &'a Ran, name: &str) -> &'a DiffEntry {
    ran.entries
        .iter()
        .find(|e| e.name() == name)
        .unwrap_or_else(|| panic!("no entry called {name:?} among {:?}", names(ran)))
}

fn names(ran: &Ran) -> Vec<&str> {
    ran.entries.iter().map(DiffEntry::name).collect()
}

#[tokio::test]
async fn identical_trees_have_no_differences() {
    let tmp = TempDir::new().unwrap();
    let (left, right) = (tmp.path().join("left"), tmp.path().join("right"));
    for base in [&left, &right] {
        write(&base.join("a.txt"), b"same everywhere");
        write(&base.join("docs/guide.md"), b"a guide");
    }
    // The exact same instant on both sides: filesystems commonly keep
    // nanosecond resolution, so two separate `SystemTime::now()` calls (one
    // per side) would differ by more than zero and this would be flaky
    // rather than wrong.
    let stamp = SystemTime::now() - Duration::from_secs(100);
    for name in ["a.txt", "docs/guide.md"] {
        set_mtime_to(&left.join(name), stamp);
        set_mtime_to(&right.join(name), stamp);
    }

    let ran = compare(Arc::new(Router::new()), spec(&left, &right)).await;

    assert!(ran.entries.is_empty(), "{:?}", names(&ran));
    assert_eq!(ran.report.differences, 0);
    assert!(!ran.report.cancelled);
}

#[tokio::test]
async fn a_file_only_on_one_side_is_reported() {
    let tmp = TempDir::new().unwrap();
    let (left, right) = (tmp.path().join("left"), tmp.path().join("right"));
    write(&left.join("only-left.txt"), b"x");
    fs::create_dir_all(&right).unwrap();

    let ran = compare(Arc::new(Router::new()), spec(&left, &right)).await;

    let entry = find(&ran, "only-left.txt");
    assert_eq!(entry.status, DiffStatus::LeftOnly);
    assert!(entry.left.is_some());
    assert!(entry.right.is_none());
}

#[tokio::test]
async fn differing_content_is_reported_by_size_without_hashing() {
    let tmp = TempDir::new().unwrap();
    let (left, right) = (tmp.path().join("left"), tmp.path().join("right"));
    write(&left.join("a.txt"), b"short");
    write(&right.join("a.txt"), b"a good deal longer than that");

    let ran = compare(Arc::new(Router::new()), spec(&left, &right)).await;

    assert_eq!(find(&ran, "a.txt").status, DiffStatus::Differs);
}

#[tokio::test]
async fn recursion_goes_all_the_way_down_a_shared_tree() {
    let tmp = TempDir::new().unwrap();
    let (left, right) = (tmp.path().join("left"), tmp.path().join("right"));
    write(&left.join("docs/old/buried.md"), b"deep on the left");
    fs::create_dir_all(right.join("docs/old")).unwrap();

    let ran = compare(Arc::new(Router::new()), spec(&left, &right)).await;

    let entry = find(&ran, "buried.md");
    assert_eq!(entry.rel_path, ["docs", "old", "buried.md"]);
    assert_eq!(entry.status, DiffStatus::LeftOnly);
}

#[tokio::test]
async fn a_folder_missing_on_one_side_is_reported_once_and_not_opened() {
    let tmp = TempDir::new().unwrap();
    let (left, right) = (tmp.path().join("left"), tmp.path().join("right"));
    write(&left.join("extra/inside/deep.txt"), b"a whole subtree");
    fs::create_dir_all(&right).unwrap();

    let ran = compare(Arc::new(Router::new()), spec(&left, &right)).await;

    assert_eq!(
        names(&ran),
        ["extra"],
        "the folder itself, not what's inside it"
    );
    assert_eq!(find(&ran, "extra").status, DiffStatus::LeftOnly);
}

#[tokio::test]
async fn hidden_things_are_left_out_unless_asked_for() {
    let tmp = TempDir::new().unwrap();
    let (left, right) = (tmp.path().join("left"), tmp.path().join("right"));
    write(&left.join(".secret"), b"x");
    fs::create_dir_all(&right).unwrap();
    #[cfg(windows)]
    {
        std::process::Command::new("attrib")
            .arg("+h")
            .arg(left.join(".secret"))
            .status()
            .unwrap();
    }

    let without = compare(Arc::new(Router::new()), spec(&left, &right)).await;
    assert!(without.entries.is_empty(), "{:?}", names(&without));

    let mut asked = spec(&left, &right);
    asked.include_hidden = true;
    let with = compare(Arc::new(Router::new()), asked).await;
    assert_eq!(names(&with), [".secret"]);
}

#[tokio::test]
async fn same_size_different_bytes_is_only_caught_when_asked_to_check_content() {
    let tmp = TempDir::new().unwrap();
    let (left, right) = (tmp.path().join("left"), tmp.path().join("right"));
    write(&left.join("a.txt"), b"aaaaa");
    write(&right.join("a.txt"), b"bbbbb");
    // Same size, and give them the same timestamp so the metadata guess sees no difference.
    let now = SystemTime::now();
    for path in [left.join("a.txt"), right.join("a.txt")] {
        File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(now)
            .unwrap();
    }

    let by_metadata = compare(Arc::new(Router::new()), spec(&left, &right)).await;
    assert!(
        by_metadata.entries.is_empty(),
        "the metadata guess can't tell"
    );

    let mut by_content = spec(&left, &right);
    by_content.by_content = true;
    let ran = compare(Arc::new(Router::new()), by_content).await;
    assert_eq!(find(&ran, "a.txt").status, DiffStatus::Differs);
}

#[tokio::test]
async fn same_size_same_bytes_is_confirmed_same_when_checking_content() {
    let tmp = TempDir::new().unwrap();
    let (left, right) = (tmp.path().join("left"), tmp.path().join("right"));
    write(&left.join("a.txt"), b"identical");
    write(&right.join("a.txt"), b"identical");
    // Different timestamps, so the metadata guess alone would call it different.
    set_mtime(&left.join("a.txt"), 500);
    set_mtime(&right.join("a.txt"), 10);

    let mut by_content = spec(&left, &right);
    by_content.by_content = true;
    let ran = compare(Arc::new(Router::new()), by_content).await;

    assert!(ran.entries.is_empty(), "the bytes really do match");
}

#[tokio::test]
async fn a_file_and_a_folder_sharing_a_name_is_a_kind_mismatch() {
    let tmp = TempDir::new().unwrap();
    let (left, right) = (tmp.path().join("left"), tmp.path().join("right"));
    write(&left.join("thing"), b"a file");
    fs::create_dir_all(right.join("thing")).unwrap();

    let ran = compare(Arc::new(Router::new()), spec(&left, &right)).await;

    assert_eq!(find(&ran, "thing").status, DiffStatus::KindMismatch);
}

#[tokio::test]
async fn a_folder_it_cannot_read_is_stepped_over_not_fatal() {
    let tmp = TempDir::new().unwrap();
    let (left, right) = (tmp.path().join("left"), tmp.path().join("right"));
    write(&left.join("ok.txt"), b"fine");
    write(&right.join("ok.txt"), b"fine");
    write(&left.join("shut/inside.txt"), b"locked away");
    fs::create_dir_all(&right).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(left.join("shut"), fs::Permissions::from_mode(0o000)).unwrap();
    }

    let ran = compare(Arc::new(Router::new()), spec(&left, &right)).await;

    // `shut` itself is still reported as left-only, without opening it.
    assert!(names(&ran).contains(&"shut"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(left.join("shut"), fs::Permissions::from_mode(0o755)).unwrap();
    }
}

#[tokio::test]
async fn a_comparison_can_be_stopped() {
    let tmp = TempDir::new().unwrap();
    let (left, right) = (tmp.path().join("left"), tmp.path().join("right"));
    fs::create_dir_all(&right).unwrap();
    for i in 0..2000 {
        write(&left.join(format!("file{i:04}.txt")), b"x");
    }

    let (tx, mut rx) = mpsc::unbounded_channel();
    let handle = run::start(Arc::new(Router::new()), spec(&left, &right), tx);
    handle.cancel();

    let report = loop {
        let event = timeout(Duration::from_secs(20), rx.recv())
            .await
            .expect("hung")
            .expect("vanished");
        if let CompareEvent::Finished(report) = event {
            break report;
        }
    };

    assert!(report.cancelled);
    assert!(
        report.differences < 2000,
        "it stopped early: {}",
        report.differences
    );
}

#[tokio::test]
async fn it_compares_inside_archives_like_any_other_folder() {
    let tmp = TempDir::new().unwrap();
    let right = tmp.path().join("right");
    write(&right.join("only-in-plain.txt"), b"x");
    let zip_path = tmp.path().join("bundle.zip");
    let mut zip = ZipWriter::new(File::create(&zip_path).unwrap());
    zip.start_file("only-in-plain.txt", SimpleFileOptions::default())
        .unwrap();
    zip.write_all(b"x").unwrap();
    zip.start_file("only-in-zip.txt", SimpleFileOptions::default())
        .unwrap();
    zip.write_all(b"z").unwrap();
    zip.finish().unwrap();

    let ran = compare(
        Arc::new(Router::new()),
        spec_paths(vp(&zip_path).enter("zip").unwrap(), vp(&right)),
    )
    .await;

    // It exists in the zip (the left side of this comparison) and not in
    // the plain folder (the right side).
    assert!(names(&ran).contains(&"only-in-zip.txt"));
    assert_eq!(find(&ran, "only-in-zip.txt").status, DiffStatus::LeftOnly);
}

fn spec_paths(left: VPath, right: VPath) -> CompareSpec {
    CompareSpec {
        left,
        right,
        by_content: false,
        include_hidden: false,
    }
}

#[tokio::test]
async fn comparing_two_empty_folders_finds_nothing_and_says_so() {
    let tmp = TempDir::new().unwrap();
    let (left, right) = (tmp.path().join("left"), tmp.path().join("right"));
    fs::create_dir_all(&left).unwrap();
    fs::create_dir_all(&right).unwrap();

    let ran = compare(Arc::new(Router::new()), spec(&left, &right)).await;

    assert!(ran.entries.is_empty());
    assert_eq!(ran.report.differences, 0);
}

// -------------------------------------------------------------------------------------------
// Syncing what a comparison found, through the ordinary job engine
// -------------------------------------------------------------------------------------------

// These syncs land in an empty destination, so the only event a healthy job
// can produce is the final report.
async fn run_job(vfs: Arc<dyn Vfs>, spec: JobSpec) -> Outcome {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let _handle = jobs::start(vfs, spec, tx);
    let event = timeout(Duration::from_secs(20), rx.recv())
        .await
        .expect("the job hung")
        .expect("the job vanished");
    match event {
        JobEvent::Finished(report) => report.outcome,
        JobEvent::Conflict(q) => panic!("unexpected conflict on {}", q.source.path),
        JobEvent::Error(q) => panic!("unexpected error: {}", q.error),
    }
}

#[tokio::test]
async fn syncing_left_to_right_fills_in_what_the_right_was_missing() {
    let tmp = TempDir::new().unwrap();
    let (left, right) = (tmp.path().join("left"), tmp.path().join("right"));
    write(&left.join("new.txt"), b"brand new");
    write(&left.join("docs/deep/buried.txt"), b"nested and new");
    fs::create_dir_all(&right).unwrap();

    let ran = compare(Arc::new(Router::new()), spec(&left, &right)).await;
    let actions = plan_sync(
        &ran.entries,
        SyncDirection::LeftToRight,
        &vp(&left),
        &vp(&right),
    )
    .unwrap();
    for job in sync_jobs(&actions) {
        assert_eq!(
            run_job(Arc::new(Router::new()), job).await,
            Outcome::Completed
        );
    }

    assert_eq!(fs::read(right.join("new.txt")).unwrap(), b"brand new");
    assert_eq!(
        fs::read(right.join("docs/deep/buried.txt")).unwrap(),
        b"nested and new"
    );
}

#[tokio::test]
async fn after_syncing_the_trees_compare_equal() {
    let tmp = TempDir::new().unwrap();
    let (left, right) = (tmp.path().join("left"), tmp.path().join("right"));
    write(&left.join("a.txt"), b"only on the left");
    write(&left.join("docs/b.txt"), b"nested, only on the left");
    write(&right.join("c.txt"), b"only on the right");
    fs::create_dir_all(&right).unwrap();

    let before = compare(Arc::new(Router::new()), spec(&left, &right)).await;
    // A full two-way fill: bring across everything either side is missing.
    for direction in [SyncDirection::LeftToRight, SyncDirection::RightToLeft] {
        let actions = plan_sync(&before.entries, direction, &vp(&left), &vp(&right)).unwrap();
        for job in sync_jobs(&actions) {
            assert_eq!(
                run_job(Arc::new(Router::new()), job).await,
                Outcome::Completed
            );
        }
    }

    let after = compare(Arc::new(Router::new()), spec(&left, &right)).await;
    assert!(after.entries.is_empty(), "{:?}", names(&after));
}
