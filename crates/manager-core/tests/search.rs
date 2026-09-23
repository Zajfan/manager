//! Searching real folder trees, and real archives.

use std::fs::{self, File};
use std::io::Write;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use manager_core::search::{Masks, Needle, SearchEvent, SearchReport, SearchSpec, run};
use manager_core::{Router, VPath, Vfs};
use tempfile::TempDir;
use tokio::sync::mpsc;
use tokio::time::timeout;
use zip::write::{SimpleFileOptions, ZipWriter};

fn vp(path: impl AsRef<Path>) -> VPath {
    VPath::local(path).unwrap()
}

/// Marks a file or folder hidden.
///
/// A leading dot is enough on Unix, but on Windows "hidden" is a real file
/// attribute and the dot means nothing, so the fixture has to set it for the
/// test to be asking the same question on both.
fn make_hidden(path: &Path) {
    #[cfg(windows)]
    {
        let status = std::process::Command::new("attrib")
            .arg("+h")
            .arg(path)
            .status()
            .expect("couldn't run attrib");
        assert!(status.success(), "attrib +h failed on {}", path.display());
    }
    #[cfg(not(windows))]
    let _ = path;
}

/// Builds a little tree to search through.
fn sample_tree(root: &Path) {
    fs::create_dir_all(root.join("src")).unwrap();
    fs::create_dir_all(root.join("docs/old")).unwrap();
    fs::create_dir_all(root.join(".git")).unwrap();
    fs::write(root.join("README.md"), b"the readme, mentions Widgets").unwrap();
    fs::write(root.join("src/main.rs"), b"fn main() { widgets(); }").unwrap();
    fs::write(root.join("src/lib.rs"), b"pub fn nothing() {}").unwrap();
    fs::write(root.join("docs/guide.md"), b"a guide about WIDGETS").unwrap();
    fs::write(root.join("docs/old/ancient.md"), b"very old").unwrap();
    fs::write(root.join(".hidden.md"), b"secret widgets").unwrap();
    fs::write(root.join(".git/config"), b"widgets").unwrap();
    make_hidden(&root.join(".hidden.md"));
    make_hidden(&root.join(".git"));
}

struct Ran {
    names: Vec<String>,
    report: SearchReport,
}

async fn search(vfs: Arc<dyn Vfs>, spec: SearchSpec) -> Ran {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let _handle = run::start(vfs, spec, tx);
    let mut names = Vec::new();
    loop {
        let event = timeout(Duration::from_secs(20), rx.recv())
            .await
            .expect("the search hung")
            .expect("the search vanished without a report");
        match event {
            SearchEvent::Found(entry) => names.push(entry.name.clone()),
            SearchEvent::Finished(report) => {
                names.sort();
                return Ran { names, report };
            }
        }
    }
}

fn spec(root: &Path, masks: &str) -> SearchSpec {
    SearchSpec {
        root: vp(root),
        masks: Masks::parse(masks),
        needle: None,
        include_hidden: false,
    }
}

#[tokio::test]
async fn it_finds_files_by_mask_all_the_way_down() {
    let tmp = TempDir::new().unwrap();
    sample_tree(tmp.path());

    let ran = search(Arc::new(Router::new()), spec(tmp.path(), "*.md")).await;

    assert_eq!(ran.names, ["README.md", "ancient.md", "guide.md"]);
    assert_eq!(ran.report.found, 3);
    assert!(!ran.report.cancelled);
}

#[tokio::test]
async fn folders_match_masks_too() {
    let tmp = TempDir::new().unwrap();
    sample_tree(tmp.path());

    let ran = search(Arc::new(Router::new()), spec(tmp.path(), "doc*")).await;

    assert_eq!(ran.names, ["docs"]);
}

#[tokio::test]
async fn hidden_things_are_left_alone_unless_asked_for() {
    let tmp = TempDir::new().unwrap();
    sample_tree(tmp.path());

    let without = search(Arc::new(Router::new()), spec(tmp.path(), "*")).await;
    assert!(!without.names.contains(&".hidden.md".to_string()));
    assert!(
        !without.names.contains(&"config".to_string()),
        "and it doesn't go into hidden folders either"
    );

    let mut asked = spec(tmp.path(), "*");
    asked.include_hidden = true;
    let with = search(Arc::new(Router::new()), asked).await;
    assert!(with.names.contains(&".hidden.md".to_string()));
    assert!(with.names.contains(&"config".to_string()));
}

#[tokio::test]
async fn text_inside_the_file_narrows_it_down() {
    let tmp = TempDir::new().unwrap();
    sample_tree(tmp.path());
    let mut spec = spec(tmp.path(), "*.md");
    spec.needle = Needle::new("widgets", false);

    let ran = search(Arc::new(Router::new()), spec).await;

    assert_eq!(
        ran.names,
        ["README.md", "guide.md"],
        "ignoring case, and not the one that doesn't mention them"
    );
}

#[tokio::test]
async fn searching_the_text_is_case_sensitive_when_asked() {
    let tmp = TempDir::new().unwrap();
    sample_tree(tmp.path());
    let mut spec = spec(tmp.path(), "*.md");
    spec.needle = Needle::new("WIDGETS", true);

    let ran = search(Arc::new(Router::new()), spec).await;

    assert_eq!(ran.names, ["guide.md"]);
}

#[tokio::test]
async fn a_folder_it_cannot_read_is_stepped_over_not_fatal() {
    let tmp = TempDir::new().unwrap();
    sample_tree(tmp.path());
    let shut = tmp.path().join("shut");
    fs::create_dir(&shut).unwrap();
    fs::write(shut.join("inside.md"), b"x").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&shut, fs::Permissions::from_mode(0o000)).unwrap();
    }

    let ran = search(Arc::new(Router::new()), spec(tmp.path(), "*.md")).await;

    // The rest of the tree is still found.
    assert!(ran.names.contains(&"README.md".to_string()));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(ran.report.unreadable, 1);
        // Put it back so the temporary folder can be cleaned up.
        fs::set_permissions(&shut, fs::Permissions::from_mode(0o755)).unwrap();
    }
}

#[tokio::test]
async fn a_search_can_be_stopped() {
    let tmp = TempDir::new().unwrap();
    // Enough files that cancelling lands somewhere in the middle.
    for i in 0..2000 {
        fs::write(tmp.path().join(format!("file{i:04}.md")), b"x").unwrap();
    }

    let (tx, mut rx) = mpsc::unbounded_channel();
    let handle = run::start(Arc::new(Router::new()), spec(tmp.path(), "*.md"), tx);
    handle.cancel();

    let report = loop {
        let event = timeout(Duration::from_secs(20), rx.recv())
            .await
            .expect("the search hung")
            .expect("the search vanished");
        if let SearchEvent::Finished(report) = event {
            break report;
        }
    };

    assert!(report.cancelled);
    assert!(report.found < 2000, "it stopped early: {}", report.found);
}

#[tokio::test]
async fn it_searches_inside_archives_like_any_other_folder() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("bundle.zip");
    let mut zip = ZipWriter::new(File::create(&path).unwrap());
    let options = SimpleFileOptions::default();
    for (name, body) in [
        ("readme.md", &b"mentions widgets"[..]),
        ("deep/notes.md", &b"nothing here"[..]),
        ("deep/other.txt", &b"widgets again"[..]),
    ] {
        zip.start_file(name, options).unwrap();
        zip.write_all(body).unwrap();
    }
    zip.finish().unwrap();

    let inside = vp(&path).enter("zip").unwrap();
    let mut spec = SearchSpec {
        root: inside,
        masks: Masks::parse("*"),
        needle: Needle::new("widgets", false),
        include_hidden: false,
    };
    spec.masks = Masks::parse("*");

    let ran = search(Arc::new(Router::new()), spec).await;

    assert_eq!(ran.names, ["other.txt", "readme.md"]);
}

#[tokio::test]
async fn searching_an_empty_folder_finds_nothing_and_says_so() {
    let tmp = TempDir::new().unwrap();

    let ran = search(Arc::new(Router::new()), spec(tmp.path(), "*")).await;

    assert!(ran.names.is_empty());
    assert_eq!(ran.report.found, 0);
    assert_eq!(ran.report.scanned, 0);
}
