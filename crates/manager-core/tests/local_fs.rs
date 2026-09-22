//! Tests that run `LocalFs` against real, throwaway directories on disk.

use std::fs;
use std::path::Path;
use std::time::{Duration, SystemTime};

use manager_core::{
    Entry, EntryKind, Error, LocalFs, SortKey, SortOrder, SortSpec, VPath, Vfs, sort_entries,
};
use tempfile::TempDir;

fn vp(path: impl AsRef<Path>) -> VPath {
    VPath::local(path).unwrap()
}

fn write(dir: &Path, name: &str, bytes: usize) {
    fs::write(dir.join(name), vec![b'x'; bytes]).unwrap();
}

async fn list_sorted(dir: &Path, spec: SortSpec) -> Vec<Entry> {
    let mut entries = LocalFs.list(&vp(dir)).await.unwrap();
    sort_entries(&mut entries, spec);
    entries
}

fn names(entries: &[Entry]) -> Vec<&str> {
    entries.iter().map(|e| e.name.as_str()).collect()
}

fn find<'a>(entries: &'a [Entry], name: &str) -> &'a Entry {
    entries.iter().find(|e| e.name == name).unwrap()
}

#[tokio::test]
async fn lists_files_and_folders_with_metadata() {
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "hello.txt", 5);
    fs::create_dir(tmp.path().join("photos")).unwrap();

    let entries = LocalFs.list(&vp(tmp.path())).await.unwrap();
    assert_eq!(entries.len(), 2);

    let file = find(&entries, "hello.txt");
    assert_eq!(file.kind, EntryKind::File);
    assert_eq!(file.size, 5);
    assert_eq!(
        file.path.as_local(),
        Some(tmp.path().join("hello.txt").as_path())
    );
    assert_eq!(file.path, vp(tmp.path()).join("hello.txt").unwrap());
    assert_eq!(file.extension(), Some("txt"));
    assert!(file.modified.is_some());

    let dir = find(&entries, "photos");
    assert_eq!(dir.kind, EntryKind::Dir);
    assert_eq!(dir.size, 0);
    assert_eq!(dir.extension(), None);
}

#[tokio::test]
async fn empty_folder_lists_nothing() {
    let tmp = TempDir::new().unwrap();
    assert!(LocalFs.list(&vp(tmp.path())).await.unwrap().is_empty());
}

#[tokio::test]
async fn missing_folder_is_not_found() {
    let tmp = TempDir::new().unwrap();
    let err = LocalFs
        .list(&vp(tmp.path().join("nope")))
        .await
        .unwrap_err();
    assert!(matches!(err, Error::NotFound(_)), "got {err:?}");
}

#[tokio::test]
async fn listing_a_file_is_an_error() {
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "file.txt", 1);
    let err = LocalFs
        .list(&vp(tmp.path().join("file.txt")))
        .await
        .unwrap_err();
    assert!(
        matches!(err, Error::NotADirectory(_) | Error::Io { .. }),
        "got {err:?}"
    );
}

#[tokio::test]
async fn stat_single_file_and_root() {
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "a.bin", 42);

    let entry = LocalFs.stat(&vp(tmp.path().join("a.bin"))).await.unwrap();
    assert_eq!(entry.name, "a.bin");
    assert_eq!(entry.size, 42);

    let root = if cfg!(windows) { "C:\\" } else { "/" };
    let root_entry = LocalFs.stat(&vp(Path::new(root))).await.unwrap();
    assert_eq!(root_entry.kind, EntryKind::Dir);
    assert_eq!(root_entry.name, root);
}

#[tokio::test]
async fn names_and_extensions() {
    let tmp = TempDir::new().unwrap();
    for name in ["archive.tar.gz", ".bashrc", "README"] {
        write(tmp.path(), name, 1);
    }
    // Windows silently strips trailing dots from file names, so only test this elsewhere.
    if cfg!(not(windows)) {
        write(tmp.path(), "trailing.", 1);
    }
    let entries = LocalFs.list(&vp(tmp.path())).await.unwrap();

    assert_eq!(find(&entries, "archive.tar.gz").extension(), Some("gz"));
    assert_eq!(find(&entries, "archive.tar.gz").stem(), "archive.tar");
    assert_eq!(find(&entries, ".bashrc").extension(), None);
    assert_eq!(find(&entries, "README").extension(), None);
    if cfg!(not(windows)) {
        assert_eq!(find(&entries, "trailing.").extension(), None);
    }
}

#[tokio::test]
async fn sort_by_name_puts_folders_first_in_natural_order() {
    let tmp = TempDir::new().unwrap();
    for name in ["img10.png", "img2.png", "Zebra.txt", "apple.txt"] {
        write(tmp.path(), name, 1);
    }
    fs::create_dir(tmp.path().join("zz_folder")).unwrap();
    fs::create_dir(tmp.path().join("Docs")).unwrap();

    let entries = list_sorted(tmp.path(), SortSpec::default()).await;
    assert_eq!(
        names(&entries),
        [
            "Docs",
            "zz_folder",
            "apple.txt",
            "img2.png",
            "img10.png",
            "Zebra.txt"
        ]
    );
}

#[tokio::test]
async fn descending_keeps_folders_on_top() {
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "a.txt", 1);
    write(tmp.path(), "b.txt", 1);
    fs::create_dir(tmp.path().join("dir1")).unwrap();
    fs::create_dir(tmp.path().join("dir2")).unwrap();

    let spec = SortSpec {
        order: SortOrder::Descending,
        ..SortSpec::default()
    };
    let entries = list_sorted(tmp.path(), spec).await;
    assert_eq!(names(&entries), ["dir2", "dir1", "b.txt", "a.txt"]);
}

#[tokio::test]
async fn sort_by_size_orders_files_and_names_folders() {
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "big", 300);
    write(tmp.path(), "small", 1);
    write(tmp.path(), "medium", 20);
    fs::create_dir(tmp.path().join("b_dir")).unwrap();
    fs::create_dir(tmp.path().join("a_dir")).unwrap();

    let spec = SortSpec {
        key: SortKey::Size,
        ..SortSpec::default()
    };
    let entries = list_sorted(tmp.path(), spec).await;
    assert_eq!(
        names(&entries),
        ["a_dir", "b_dir", "small", "medium", "big"]
    );
}

#[tokio::test]
async fn sort_by_extension() {
    let tmp = TempDir::new().unwrap();
    for name in ["b.txt", "a.zip", "c.md", "noext", "a.txt"] {
        write(tmp.path(), name, 1);
    }
    let spec = SortSpec {
        key: SortKey::Extension,
        ..SortSpec::default()
    };
    let entries = list_sorted(tmp.path(), spec).await;
    assert_eq!(
        names(&entries),
        ["noext", "c.md", "a.txt", "b.txt", "a.zip"]
    );
}

#[tokio::test]
async fn sort_by_modified_time() {
    let tmp = TempDir::new().unwrap();
    let base = SystemTime::now() - Duration::from_secs(3600);
    for (name, age) in [("new", 1), ("old", 100), ("mid", 50)] {
        write(tmp.path(), name, 1);
        let file = fs::File::options()
            .write(true)
            .open(tmp.path().join(name))
            .unwrap();
        file.set_modified(base - Duration::from_secs(age)).unwrap();
    }
    let spec = SortSpec {
        key: SortKey::Modified,
        ..SortSpec::default()
    };
    let entries = list_sorted(tmp.path(), spec).await;
    assert_eq!(names(&entries), ["old", "mid", "new"]);
}

#[cfg(unix)]
#[tokio::test]
async fn dotfiles_are_hidden_on_unix() {
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), ".secret", 1);
    write(tmp.path(), "visible", 1);
    let entries = LocalFs.list(&vp(tmp.path())).await.unwrap();
    assert!(find(&entries, ".secret").hidden);
    assert!(!find(&entries, "visible").hidden);
}

#[cfg(unix)]
#[tokio::test]
async fn unix_permissions_are_reported() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "script.sh", 1);
    let path = tmp.path().join("script.sh");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o555)).unwrap();

    let entry = LocalFs.stat(&vp(path)).await.unwrap();
    assert_eq!(entry.permissions.unix_mode, Some(0o555));
    assert!(entry.permissions.readonly);
}

#[cfg(unix)]
#[tokio::test]
async fn symlinks_report_their_target() {
    use manager_core::LinkTarget;
    use std::os::unix::fs::symlink;

    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "real.txt", 7);
    fs::create_dir(tmp.path().join("real_dir")).unwrap();
    symlink(tmp.path().join("real.txt"), tmp.path().join("link_to_file")).unwrap();
    symlink(tmp.path().join("real_dir"), tmp.path().join("link_to_dir")).unwrap();
    symlink(tmp.path().join("gone"), tmp.path().join("broken")).unwrap();

    let entries = LocalFs.list(&vp(tmp.path())).await.unwrap();

    let to_file = find(&entries, "link_to_file");
    assert_eq!(
        to_file.kind,
        EntryKind::Symlink {
            target: Some(LinkTarget::File)
        }
    );
    assert_eq!(to_file.size, 7, "size comes from the target");
    assert_eq!(
        to_file.link_target.as_deref(),
        Some(tmp.path().join("real.txt").as_path())
    );

    let to_dir = find(&entries, "link_to_dir");
    assert!(
        to_dir.kind.is_dir_like(),
        "a link to a folder can be entered"
    );

    let broken = find(&entries, "broken");
    assert_eq!(broken.kind, EntryKind::Symlink { target: None });

    // Links to folders sort with the folders.
    let mut sorted = entries.clone();
    sort_entries(&mut sorted, SortSpec::default());
    assert_eq!(
        names(&sorted),
        [
            "link_to_dir",
            "real_dir",
            "broken",
            "link_to_file",
            "real.txt"
        ]
    );
}

#[tokio::test]
async fn create_dir_makes_a_folder_once() {
    let tmp = TempDir::new().unwrap();
    let new_dir = vp(tmp.path()).join("new folder").unwrap();

    LocalFs.create_dir(&new_dir).await.unwrap();
    assert!(tmp.path().join("new folder").is_dir());

    let err = LocalFs.create_dir(&new_dir).await.unwrap_err();
    assert!(matches!(err, Error::AlreadyExists(_)), "got {err:?}");
}

#[tokio::test]
async fn local_fs_refuses_paths_it_cannot_open() {
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "photos.zip", 1);
    let inside_zip = vp(tmp.path().join("photos.zip")).enter("zip").unwrap();
    let remote = VPath::parse("sftp://me@host/home").unwrap();

    for path in [inside_zip, remote] {
        let err = LocalFs.list(&path).await.unwrap_err();
        assert!(matches!(err, Error::Unsupported { .. }), "got {err:?}");
    }
}
