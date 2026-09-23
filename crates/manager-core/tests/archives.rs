//! Browsing and reading archives through the `Vfs` interface, and copying out
//! of them with the ordinary job engine.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::Write;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use manager_core::jobs::{self, JobEvent, JobSpec, Outcome};
use manager_core::{EntryKind, Error, Router, VPath, Vfs};
use tempfile::TempDir;
use tokio::io::AsyncReadExt;
use tokio::sync::mpsc;
use tokio::time::timeout;
use zip::write::{SimpleFileOptions, ZipWriter};

/// Writes a ZIP. Entry names ending in `/` become folder entries.
fn make_zip(dir: &Path, name: &str, entries: &[(&str, &[u8])]) -> VPath {
    let path = dir.join(name);
    let mut zip = ZipWriter::new(File::create(&path).unwrap());
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for (entry, contents) in entries {
        if let Some(folder) = entry.strip_suffix('/') {
            zip.add_directory(folder, options).unwrap();
        } else {
            zip.start_file(*entry, options).unwrap();
            zip.write_all(contents).unwrap();
        }
    }
    zip.finish().unwrap();
    VPath::local(path).unwrap()
}

/// Writes a TAR, gzipped if `compress`. A name ending in `/` is a folder.
fn make_tar(dir: &Path, name: &str, entries: &[(&str, &[u8])], compress: bool) -> VPath {
    let path = dir.join(name);
    let mut raw = Vec::new();
    {
        let mut builder = tar::Builder::new(&mut raw);
        for (entry, contents) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_mtime(1_700_000_000);
            header.set_mode(0o644);
            if let Some(folder) = entry.strip_suffix('/') {
                header.set_entry_type(tar::EntryType::Directory);
                header.set_size(0);
                header.set_cksum();
                builder
                    .append_data(&mut header, format!("{folder}/"), std::io::empty())
                    .unwrap();
            } else {
                header.set_size(contents.len() as u64);
                header.set_cksum();
                builder.append_data(&mut header, entry, *contents).unwrap();
            }
        }
        builder.finish().unwrap();
    }
    let mut out = File::create(&path).unwrap();
    if compress {
        let mut encoder = flate2::write::GzEncoder::new(&mut out, flate2::Compression::default());
        encoder.write_all(&raw).unwrap();
        encoder.finish().unwrap();
    } else {
        out.write_all(&raw).unwrap();
    }
    VPath::local(path).unwrap()
}

const SAMPLE: &[(&str, &[u8])] = &[
    ("notes.txt", b"hello"),
    ("photos/", b""),
    ("photos/img.jpg", b"JPEGDATA"),
    ("photos/2024/deep.txt", b"buried"),
];

fn sample_zip(dir: &Path) -> VPath {
    make_zip(dir, "stuff.zip", SAMPLE)
}

async fn names(vfs: &Router, dir: &VPath) -> Vec<String> {
    let mut names: Vec<String> = vfs
        .list(dir)
        .await
        .unwrap()
        .into_iter()
        .map(|e| e.name)
        .collect();
    names.sort();
    names
}

/// Every file under `dir`: relative path → contents.
fn tree(dir: &Path) -> BTreeMap<String, Option<String>> {
    let mut out = BTreeMap::new();
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, Option<String>>) {
        for item in fs::read_dir(dir).unwrap() {
            let path = item.unwrap().path();
            let rel = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            if path.is_dir() {
                out.insert(rel, None);
                walk(root, &path, out);
            } else {
                out.insert(rel, fs::read_to_string(&path).ok());
            }
        }
    }
    walk(dir, dir, &mut out);
    out
}

// -------------------------------------------------------------------------------------------
// Browsing
// -------------------------------------------------------------------------------------------

#[tokio::test]
async fn a_zip_opens_like_a_folder() {
    let tmp = TempDir::new().unwrap();
    let root = sample_zip(tmp.path()).enter("zip").unwrap();

    assert_eq!(names(&Router::new(), &root).await, ["notes.txt", "photos"]);
}

#[tokio::test]
async fn folders_inside_a_zip_open_too() {
    let tmp = TempDir::new().unwrap();
    let vfs = Router::new();
    let photos = sample_zip(tmp.path())
        .enter("zip")
        .unwrap()
        .join("photos")
        .unwrap();

    assert_eq!(names(&vfs, &photos).await, ["2024", "img.jpg"]);
    assert_eq!(
        names(&vfs, &photos.join("2024").unwrap()).await,
        ["deep.txt"]
    );
}

#[tokio::test]
async fn entries_inside_a_zip_carry_their_details() {
    let tmp = TempDir::new().unwrap();
    let root = sample_zip(tmp.path()).enter("zip").unwrap();

    let entries = Router::new().list(&root).await.unwrap();
    let notes = entries.iter().find(|e| e.name == "notes.txt").unwrap();
    let photos = entries.iter().find(|e| e.name == "photos").unwrap();

    assert_eq!(notes.kind, EntryKind::File);
    assert_eq!(notes.size, 5);
    assert_eq!(notes.path, root.join("notes.txt").unwrap());
    assert!(
        notes.permissions.readonly,
        "nothing in an archive is writable"
    );
    assert_eq!(photos.kind, EntryKind::Dir);
}

#[tokio::test]
async fn the_root_of_a_zip_is_a_folder() {
    let tmp = TempDir::new().unwrap();
    let root = sample_zip(tmp.path()).enter("zip").unwrap();

    let entry = Router::new().stat(&root).await.unwrap();

    assert_eq!(entry.kind, EntryKind::Dir);
    assert_eq!(entry.name, "stuff.zip");
}

#[tokio::test]
async fn a_file_inside_a_zip_can_be_described() {
    let tmp = TempDir::new().unwrap();
    let file = sample_zip(tmp.path())
        .enter("zip")
        .unwrap()
        .join("notes.txt")
        .unwrap();

    let entry = Router::new().stat(&file).await.unwrap();

    assert_eq!(entry.kind, EntryKind::File);
    assert_eq!(entry.size, 5);
}

#[tokio::test]
async fn something_that_is_not_in_the_zip_is_not_found() {
    let tmp = TempDir::new().unwrap();
    let root = sample_zip(tmp.path()).enter("zip").unwrap();
    let vfs = Router::new();

    let missing = vfs.list(&root.join("nope").unwrap()).await.unwrap_err();
    assert!(matches!(missing, Error::NotFound(_)), "got {missing:?}");

    let not_a_folder = vfs
        .list(&root.join("notes.txt").unwrap())
        .await
        .unwrap_err();
    assert!(
        matches!(not_a_folder, Error::NotADirectory(_)),
        "got {not_a_folder:?}"
    );
}

#[tokio::test]
async fn reading_a_file_out_of_a_zip_gives_its_contents() {
    let tmp = TempDir::new().unwrap();
    let file = sample_zip(tmp.path())
        .enter("zip")
        .unwrap()
        .join("notes.txt")
        .unwrap();

    let mut reader = Router::new().open_read(&file).await.unwrap();
    let mut got = String::new();
    reader.read_to_string(&mut got).await.unwrap();

    assert_eq!(got, "hello");
}

#[tokio::test]
async fn an_archive_refuses_to_be_written_to() {
    let tmp = TempDir::new().unwrap();
    let root = sample_zip(tmp.path()).enter("zip").unwrap();
    let vfs = Router::new();
    let target = root.join("new").unwrap();

    let refusals = [
        vfs.create_dir(&target).await.err(),
        vfs.open_write(&target).await.err(),
        vfs.remove_file(&root.join("notes.txt").unwrap())
            .await
            .err(),
        vfs.remove_dir(&root.join("photos").unwrap()).await.err(),
    ];
    for err in refusals {
        let err = err.expect("writing into an archive should fail, not succeed");
        assert!(
            matches!(err, Error::InvalidOperation(_) | Error::Unsupported { .. }),
            "expected a clear refusal, got {err:?}"
        );
        assert!(
            err.to_string().contains("archive"),
            "the message should explain why: {err}"
        );
    }
}

// -------------------------------------------------------------------------------------------
// The same, through a different format
// -------------------------------------------------------------------------------------------

#[tokio::test]
async fn a_tar_opens_like_a_folder() {
    let tmp = TempDir::new().unwrap();
    let root = make_tar(tmp.path(), "stuff.tar", SAMPLE, false)
        .enter("tar")
        .unwrap();

    assert_eq!(names(&Router::new(), &root).await, ["notes.txt", "photos"]);
}

#[tokio::test]
async fn a_gzipped_tar_opens_like_a_folder_too() {
    let tmp = TempDir::new().unwrap();
    let vfs = Router::new();
    let root = make_tar(tmp.path(), "stuff.tar.gz", SAMPLE, true)
        .enter("tar")
        .unwrap();

    assert_eq!(names(&vfs, &root).await, ["notes.txt", "photos"]);
    assert_eq!(
        names(&vfs, &root.join("photos").unwrap()).await,
        ["2024", "img.jpg"]
    );
}

#[tokio::test]
async fn a_whole_folder_can_be_copied_out_of_a_gzipped_tar() {
    let tmp = TempDir::new().unwrap();
    let out = tmp.path().join("out");
    fs::create_dir(&out).unwrap();
    let photos = make_tar(tmp.path(), "stuff.tar.gz", SAMPLE, true)
        .enter("tar")
        .unwrap()
        .join("photos")
        .unwrap();

    let outcome = run(
        Arc::new(Router::new()),
        JobSpec::Copy {
            sources: vec![photos],
            dest: VPath::local(&out).unwrap(),
        },
    )
    .await;

    assert_eq!(outcome, Outcome::Completed);
    assert_eq!(
        tree(&out),
        BTreeMap::from([
            ("photos".to_string(), None),
            ("photos/2024".to_string(), None),
            (
                "photos/2024/deep.txt".to_string(),
                Some("buried".to_string())
            ),
            ("photos/img.jpg".to_string(), Some("JPEGDATA".to_string())),
        ])
    );
}

#[tokio::test]
async fn a_window_can_be_read_from_a_file_inside_an_archive() {
    // The viewer scrolls with this, so it has to work in archives too, even
    // though a ZIP entry can only be reached by decompressing up to it.
    let tmp = TempDir::new().unwrap();
    let body: Vec<u8> = (0..100_000).map(|i| (i % 251) as u8).collect();
    let zip = make_zip(tmp.path(), "big.zip", &[("big.bin", &body)]);
    let inside = zip.enter("zip").unwrap().join("big.bin").unwrap();

    let window = Router::new()
        .read_window(&inside, 99_990, 10)
        .await
        .unwrap();

    assert_eq!(window, &body[99_990..]);
}

// -------------------------------------------------------------------------------------------
// Routing
// -------------------------------------------------------------------------------------------

#[tokio::test]
async fn an_archive_reports_being_unwatchable_in_a_way_the_ui_can_recognise() {
    // The UI stays quiet about places that can never be watched, instead of
    // complaining every time you open one, so it has to be able to tell this
    // apart from a watch that genuinely failed.
    let tmp = TempDir::new().unwrap();
    let root = sample_zip(tmp.path()).enter("zip").unwrap();
    let sink: manager_core::WatchSink = Arc::new(|_| {});

    let err = Router::new().watch(&root, sink).await.unwrap_err();

    assert!(matches!(err, Error::Unsupported { .. }), "got {err:?}");
}

#[tokio::test]
async fn plain_paths_still_go_to_the_local_disk() {
    let tmp = TempDir::new().unwrap();
    fs::write(tmp.path().join("ordinary.txt"), b"hi").unwrap();

    let entries = Router::new()
        .list(&VPath::local(tmp.path()).unwrap())
        .await
        .unwrap();

    assert!(entries.iter().any(|e| e.name == "ordinary.txt"));
}

#[tokio::test]
async fn an_archive_format_we_do_not_know_is_refused() {
    let tmp = TempDir::new().unwrap();
    let odd = sample_zip(tmp.path()).enter("rar").unwrap();

    let err = Router::new().list(&odd).await.unwrap_err();

    assert!(matches!(err, Error::Unsupported { .. }), "got {err:?}");
}

// -------------------------------------------------------------------------------------------
// The payoff: the existing job engine copies out of an archive unchanged
// -------------------------------------------------------------------------------------------

async fn run(vfs: Arc<dyn Vfs>, spec: JobSpec) -> Outcome {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let _handle = jobs::start(vfs, spec, tx);
    // These tests all copy into an empty folder, so the only event a healthy
    // job can produce is the final report.
    let event = timeout(Duration::from_secs(20), rx.recv())
        .await
        .expect("job hung")
        .expect("job vanished");
    match event {
        JobEvent::Finished(report) => report.outcome,
        JobEvent::Conflict(q) => panic!("unexpected conflict on {}", q.source.path),
        JobEvent::Error(q) => panic!("unexpected error: {}", q.error),
    }
}

#[tokio::test]
async fn one_file_can_be_copied_out_of_a_zip() {
    let tmp = TempDir::new().unwrap();
    let out = tmp.path().join("out");
    fs::create_dir(&out).unwrap();
    let notes = sample_zip(tmp.path())
        .enter("zip")
        .unwrap()
        .join("notes.txt")
        .unwrap();

    let outcome = run(
        Arc::new(Router::new()),
        JobSpec::Copy {
            sources: vec![notes],
            dest: VPath::local(&out).unwrap(),
        },
    )
    .await;

    assert_eq!(outcome, Outcome::Completed);
    assert_eq!(fs::read_to_string(out.join("notes.txt")).unwrap(), "hello");
}

#[tokio::test]
async fn a_whole_folder_can_be_copied_out_of_a_zip() {
    let tmp = TempDir::new().unwrap();
    let out = tmp.path().join("out");
    fs::create_dir(&out).unwrap();
    let photos = sample_zip(tmp.path())
        .enter("zip")
        .unwrap()
        .join("photos")
        .unwrap();

    let outcome = run(
        Arc::new(Router::new()),
        JobSpec::Copy {
            sources: vec![photos],
            dest: VPath::local(&out).unwrap(),
        },
    )
    .await;

    assert_eq!(outcome, Outcome::Completed);
    assert_eq!(
        tree(&out),
        BTreeMap::from([
            ("photos".to_string(), None),
            ("photos/2024".to_string(), None),
            (
                "photos/2024/deep.txt".to_string(),
                Some("buried".to_string())
            ),
            ("photos/img.jpg".to_string(), Some("JPEGDATA".to_string())),
        ])
    );
}

#[tokio::test]
async fn a_big_file_survives_the_trip_out_of_a_zip() {
    let tmp = TempDir::new().unwrap();
    let out = tmp.path().join("out");
    fs::create_dir(&out).unwrap();
    // Bigger than one chunk, so the streaming path is really exercised.
    let big: Vec<u8> = (0..1_000_000).map(|i| (i % 251) as u8).collect();
    let zip = make_zip(tmp.path(), "big.zip", &[("big.bin", &big)]);
    let inside = zip.enter("zip").unwrap().join("big.bin").unwrap();

    let outcome = run(
        Arc::new(Router::new()),
        JobSpec::Copy {
            sources: vec![inside],
            dest: VPath::local(&out).unwrap(),
        },
    )
    .await;

    assert_eq!(outcome, Outcome::Completed);
    assert!(fs::read(out.join("big.bin")).unwrap() == big);
}
