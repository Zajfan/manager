//! A tiny `ls` built on manager-core, to see step 1 working before the TUI exists.
//!
//! cargo run -p manager-core --example ls -- [PATH] [--sort name|ext|size|date] [--desc] [--all]

use std::time::UNIX_EPOCH;

use manager_core::{
    Entry, EntryKind, LocalFs, SortKey, SortOrder, SortSpec, VPath, Vfs, sort_entries,
};

#[tokio::main]
async fn main() {
    let mut path = String::from(".");
    let mut spec = SortSpec::default();
    let mut show_hidden = false;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--desc" => spec.order = SortOrder::Descending,
            "--all" | "-a" => show_hidden = true,
            "--sort" => {
                spec.key = match args.next().as_deref() {
                    Some("ext") => SortKey::Extension,
                    Some("size") => SortKey::Size,
                    Some("date") => SortKey::Modified,
                    _ => SortKey::Name,
                }
            }
            other => path = other.to_string(),
        }
    }

    let entries = match VPath::parse(&path) {
        Ok(path) => LocalFs.list(&path).await,
        Err(err) => Err(err),
    };
    let mut entries = match entries {
        Ok(entries) => entries,
        Err(err) => {
            eprintln!("error: {err}");
            std::process::exit(1);
        }
    };
    entries.retain(|e| show_hidden || !e.hidden);
    sort_entries(&mut entries, spec);

    for entry in &entries {
        println!(
            "{:<6} {:>12} {:>12}  {}",
            kind_label(entry),
            size_label(entry),
            mtime(entry),
            display(entry)
        );
    }
    println!("\n{} entries", entries.len());
}

fn kind_label(e: &Entry) -> &'static str {
    match e.kind {
        EntryKind::File => "file",
        EntryKind::Dir => "dir",
        EntryKind::Symlink { target: None } => "broken",
        EntryKind::Symlink { .. } => "link",
        EntryKind::Other => "other",
    }
}

fn size_label(e: &Entry) -> String {
    if e.kind.is_dir_like() {
        "<DIR>".into()
    } else {
        e.size.to_string()
    }
}

/// Seconds since 1970. Pretty dates come later with a proper date library.
fn mtime(e: &Entry) -> String {
    e.modified
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs().to_string())
        .unwrap_or_else(|| "-".into())
}

fn display(e: &Entry) -> String {
    match &e.link_target {
        Some(target) => format!("{} -> {}", e.name, target.display()),
        None => e.name.clone(),
    }
}
