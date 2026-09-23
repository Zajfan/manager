//! Browsing inside archives as if they were folders.

mod fs;
pub mod index;
pub mod stream;
pub mod tar;
pub mod zip;

pub use fs::ArchiveFs;

/// The archive format a file name suggests, if we can read it.
///
/// This is what decides whether pressing Enter on a file opens it as a folder,
/// so it deliberately only says yes to formats [`ArchiveFs`] can actually read.
/// The many ZIP-based formats (`.jar`, `.apk`, `.epub`, ...) are all just ZIPs.
pub fn kind_for(name: &str) -> Option<&'static str> {
    let lower = name.to_ascii_lowercase();
    // Two-part extensions have to be checked before the single-part ones,
    // or `backup.tar.gz` would look like a file of type "gz".
    for (suffix, kind) in [(".tar.gz", "tar"), (".tar", "tar"), (".tgz", "tar")] {
        if lower.len() > suffix.len() && lower.ends_with(suffix) {
            return Some(kind);
        }
    }

    // Same rule as `Entry::extension`: a leading dot is a hidden file, not an
    // extension, and a trailing dot is not one either.
    let dot = name.rfind('.')?;
    if dot == 0 || dot == name.len() - 1 {
        return None;
    }
    match &lower[dot + 1..] {
        // Every one of these is a ZIP with a different name on it.
        //
        // Formats that merely happen to be ZIPs inside (`.docx`, `.xlsx`) are
        // left out: someone pressing Enter on a document wants to read it, not
        // to browse its XML. Those belong behind an explicit "open as archive"
        // key, the way Total Commander does it.
        "zip" | "jar" | "war" | "ear" | "apk" | "xpi" | "epub" | "odt" | "ods" | "odp" => {
            Some("zip")
        }
        _ => None,
    }
}

#[cfg(test)]
mod kind_tests {
    use super::kind_for;

    #[test]
    fn zip_files_are_recognised() {
        assert_eq!(kind_for("photos.zip"), Some("zip"));
    }

    #[test]
    fn the_extension_is_not_case_sensitive() {
        // Archives written on Windows often shout.
        assert_eq!(kind_for("PHOTOS.ZIP"), Some("zip"));
        assert_eq!(kind_for("Photos.Zip"), Some("zip"));
    }

    #[test]
    fn formats_that_are_really_zips_are_recognised_too() {
        for name in ["app.jar", "app.apk", "book.epub", "add-on.xpi", "doc.odt"] {
            assert_eq!(kind_for(name), Some("zip"), "{name}");
        }
    }

    #[test]
    fn ordinary_files_are_left_alone() {
        for name in ["notes.txt", "image.jpg", "no-extension", "zip"] {
            assert_eq!(kind_for(name), None, "{name}");
        }
    }

    #[test]
    fn a_leading_dot_is_not_an_extension() {
        // `.zip` is a hidden file called "zip", not an archive.
        assert_eq!(kind_for(".zip"), None);
        assert_eq!(kind_for("trailing."), None);
    }

    #[test]
    fn the_tar_family_is_recognised() {
        for name in ["backup.tar", "backup.tar.gz", "backup.tgz", "BACKUP.TAR.GZ"] {
            assert_eq!(kind_for(name), Some("tar"), "{name}");
        }
    }

    #[test]
    fn a_double_extension_is_not_mistaken_for_its_last_half() {
        // `.gz` on its own is a single compressed file, not an archive.
        assert_eq!(kind_for("notes.gz"), None);
    }

    #[test]
    fn formats_we_cannot_read_yet_are_not_offered() {
        // Opening these would only produce an error, so Enter shouldn't try.
        for name in ["stuff.7z", "stuff.rar", "disk.iso", "backup.tar.bz2"] {
            assert_eq!(kind_for(name), None, "{name}");
        }
    }
}
