//! Finding duplicate files in real folder trees, and archives.

use std::fs::{self, File};
use std::io::Write;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use manager_core::duplicates::{
    DuplicateEvent, DuplicateGroup, DuplicateReport, DuplicateSpec, run,
};
use manager_core::search::Masks;
use manager_core::{Router, VPath, Vfs};
use tempfile::TempDir;
use tokio::sync::mpsc;
use tokio::time::timeout;
use zip::write::{SimpleFileOptions, ZipWriter};

fn vp(path: impl AsRef<Path>) -> VPath {
    VPath::local(path).unwrap()
}

fn spec(root: &Path) -> DuplicateSpec {
    DuplicateSpec {
        root: vp(root),
        ..Default::default()
    }
}

struct Ran {
    groups: Vec<DuplicateGroup>,
    report: DuplicateReport,
}

async fn find(vfs: Arc<dyn Vfs>, spec: DuplicateSpec) -> Ran {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let _handle = run::start(vfs, spec, tx);
    let mut groups = Vec::new();
    loop {
        let event = timeout(Duration::from_secs(20), rx.recv())
            .await
            .expect("the search hung")
            .expect("the search vanished without a report");
        match event {
            DuplicateEvent::Found(group) => groups.push(*group),
            DuplicateEvent::Finished(report) => {
                groups.sort_by_key(|g| g.files.first().map(|e| e.name.clone()));
                return Ran { groups, report };
            }
        }
    }
}

fn names(group: &DuplicateGroup) -> Vec<&str> {
    let mut names: Vec<&str> = group.files.iter().map(|e| e.name.as_str()).collect();
    names.sort();
    names
}

#[tokio::test]
async fn two_files_with_the_same_content_are_a_group() {
    let tmp = TempDir::new().unwrap();
    fs::write(tmp.path().join("a.txt"), b"identical content").unwrap();
    fs::write(tmp.path().join("b.txt"), b"identical content").unwrap();

    let ran = find(Arc::new(Router::new()), spec(tmp.path())).await;

    assert_eq!(ran.groups.len(), 1);
    assert_eq!(names(&ran.groups[0]), ["a.txt", "b.txt"]);
    assert_eq!(ran.report.groups, 1);
    assert_eq!(ran.report.extra_files, 1);
}

#[tokio::test]
async fn files_the_same_size_but_different_content_are_not_a_group() {
    let tmp = TempDir::new().unwrap();
    fs::write(tmp.path().join("a.txt"), b"aaaaaaaaaa").unwrap();
    fs::write(tmp.path().join("b.txt"), b"bbbbbbbbbb").unwrap();

    let ran = find(Arc::new(Router::new()), spec(tmp.path())).await;

    assert!(
        ran.groups.is_empty(),
        "same size, different bytes: not duplicates"
    );
}

#[tokio::test]
async fn a_unique_file_is_not_reported() {
    let tmp = TempDir::new().unwrap();
    fs::write(tmp.path().join("a.txt"), b"one of a kind").unwrap();

    let ran = find(Arc::new(Router::new()), spec(tmp.path())).await;

    assert!(ran.groups.is_empty());
}

#[tokio::test]
async fn three_identical_files_are_one_group_of_three() {
    let tmp = TempDir::new().unwrap();
    for name in ["a.txt", "b.txt", "c.txt"] {
        fs::write(tmp.path().join(name), b"same everywhere").unwrap();
    }

    let ran = find(Arc::new(Router::new()), spec(tmp.path())).await;

    assert_eq!(ran.groups.len(), 1);
    assert_eq!(names(&ran.groups[0]), ["a.txt", "b.txt", "c.txt"]);
    assert_eq!(
        ran.report.extra_files, 2,
        "two of the three could be removed"
    );
}

#[tokio::test]
async fn duplicates_are_found_across_subfolders() {
    let tmp = TempDir::new().unwrap();
    fs::write(tmp.path().join("a.txt"), b"buried treasure").unwrap();
    fs::create_dir_all(tmp.path().join("deep/down")).unwrap();
    fs::write(tmp.path().join("deep/down/b.txt"), b"buried treasure").unwrap();

    let ran = find(Arc::new(Router::new()), spec(tmp.path())).await;

    assert_eq!(ran.groups.len(), 1);
    assert_eq!(names(&ran.groups[0]), ["a.txt", "b.txt"]);
}

#[tokio::test]
async fn empty_files_are_excluded_by_default() {
    let tmp = TempDir::new().unwrap();
    fs::write(tmp.path().join("a.txt"), b"").unwrap();
    fs::write(tmp.path().join("b.txt"), b"").unwrap();

    let ran = find(Arc::new(Router::new()), spec(tmp.path())).await;

    assert!(
        ran.groups.is_empty(),
        "empty files aren't meaningfully duplicates of each other"
    );
}

#[tokio::test]
async fn a_mask_narrows_down_what_is_compared() {
    let tmp = TempDir::new().unwrap();
    fs::write(tmp.path().join("a.txt"), b"same content").unwrap();
    fs::write(tmp.path().join("b.log"), b"same content").unwrap();

    let mut only_txt = spec(tmp.path());
    only_txt.masks = Masks::parse("*.txt");
    let ran = find(Arc::new(Router::new()), only_txt).await;

    assert!(
        ran.groups.is_empty(),
        "b.log was never a candidate, so a.txt has nothing to match"
    );
}

#[tokio::test]
async fn hidden_files_are_left_out_unless_asked_for() {
    let tmp = TempDir::new().unwrap();
    fs::write(tmp.path().join(".secret"), b"shhh, identical").unwrap();
    fs::write(tmp.path().join("obvious.txt"), b"shhh, identical").unwrap();
    #[cfg(windows)]
    {
        std::process::Command::new("attrib")
            .arg("+h")
            .arg(tmp.path().join(".secret"))
            .status()
            .unwrap();
    }

    let without = find(Arc::new(Router::new()), spec(tmp.path())).await;
    assert!(
        without.groups.is_empty(),
        "the hidden copy shouldn't count as a match"
    );

    let mut asked = spec(tmp.path());
    asked.include_hidden = true;
    let with = find(Arc::new(Router::new()), asked).await;
    assert_eq!(with.groups.len(), 1);
}

#[tokio::test]
async fn a_folder_it_cannot_read_is_stepped_over_not_fatal() {
    let tmp = TempDir::new().unwrap();
    fs::write(tmp.path().join("a.txt"), b"findable").unwrap();
    fs::write(tmp.path().join("b.txt"), b"findable").unwrap();
    fs::create_dir(tmp.path().join("shut")).unwrap();
    fs::write(tmp.path().join("shut/c.txt"), b"locked away").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(tmp.path().join("shut"), fs::Permissions::from_mode(0o000)).unwrap();
    }

    let ran = find(Arc::new(Router::new()), spec(tmp.path())).await;

    assert_eq!(ran.groups.len(), 1);
    assert_eq!(names(&ran.groups[0]), ["a.txt", "b.txt"]);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(ran.report.unreadable, 1);
        fs::set_permissions(tmp.path().join("shut"), fs::Permissions::from_mode(0o755)).unwrap();
    }
}

#[tokio::test]
async fn a_search_can_be_stopped() {
    let tmp = TempDir::new().unwrap();
    for i in 0..500 {
        fs::write(
            tmp.path().join(format!("file{i:04}.txt")),
            b"the same everywhere",
        )
        .unwrap();
    }

    let (tx, mut rx) = mpsc::unbounded_channel();
    let handle = run::start(Arc::new(Router::new()), spec(tmp.path()), tx);
    handle.cancel();

    let report = loop {
        let event = timeout(Duration::from_secs(20), rx.recv())
            .await
            .expect("hung")
            .expect("vanished");
        if let DuplicateEvent::Finished(report) = event {
            break report;
        }
    };

    assert!(report.cancelled);
}

#[tokio::test]
async fn it_finds_duplicates_inside_an_archive_like_any_other_folder() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("bundle.zip");
    let mut zip = ZipWriter::new(File::create(&path).unwrap());
    let options = SimpleFileOptions::default();
    for name in ["a.txt", "sub/b.txt"] {
        zip.start_file(name, options).unwrap();
        zip.write_all(b"packed twice").unwrap();
    }
    zip.finish().unwrap();

    let inside = vp(&path).enter("zip").unwrap();
    let ran = find(Arc::new(Router::new()), spec_at(inside)).await;

    assert_eq!(ran.groups.len(), 1);
    assert_eq!(names(&ran.groups[0]), ["a.txt", "b.txt"]);
}

fn spec_at(root: VPath) -> DuplicateSpec {
    DuplicateSpec {
        root,
        ..Default::default()
    }
}

#[tokio::test]
async fn an_empty_folder_finds_nothing_and_says_so() {
    let tmp = TempDir::new().unwrap();

    let ran = find(Arc::new(Router::new()), spec(tmp.path())).await;

    assert!(ran.groups.is_empty());
    assert_eq!(ran.report.groups, 0);
    assert_eq!(ran.report.extra_files, 0);
}
