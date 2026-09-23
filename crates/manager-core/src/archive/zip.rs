//! Reading ZIP files.

use std::fs::File;
use std::io::{self, Read};
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use crate::archive::index::RawEntry;
use crate::{Error, Result, VPath};

/// A ZIP file on the local disk, with its table of contents already read.
///
/// A ZIP keeps a directory of its contents at the end of the file, so listing
/// is cheap and doesn't involve decompressing anything.
#[derive(Debug)]
pub struct ZipReader {
    path: PathBuf,
    entries: Vec<RawEntry>,
}

impl ZipReader {
    /// Reads the table of contents of the ZIP at `container`.
    pub fn open(container: &VPath) -> Result<ZipReader> {
        let path = local_path(container)?;
        let mut archive = open_archive(container, &path)?;

        let mut entries = Vec::with_capacity(archive.len());
        for slot in 0..archive.len() {
            // `by_index_raw` skips setting up decryption and decompression,
            // so listing works even for entries we couldn't read.
            let entry = archive
                .by_index_raw(slot)
                .map_err(|e| archive_error(container, e))?;
            entries.push(RawEntry {
                name: entry.name().to_string(),
                size: entry.size(),
                modified: entry.last_modified().and_then(to_system_time),
                slot,
            });
        }

        Ok(ZipReader { path, entries })
    }

    pub fn entries(&self) -> &[RawEntry] {
        &self.entries
    }

    /// Reads one entry, handing its bytes to `emit` a chunk at a time,
    /// decompressing as it goes.
    ///
    /// It pushes rather than returning a reader because every reader the `zip`
    /// crate hands out borrows the archive, so an owned one can't be expressed
    /// without unsafe self-reference. `emit` returning `false` means nobody is
    /// listening any more and reading stops.
    pub fn read_entry(&self, slot: usize, emit: &mut dyn FnMut(Vec<u8>) -> bool) -> io::Result<()> {
        // The archive is reopened per entry: the reader has to own it, and the
        // central directory is at the end of the file, so this costs one extra
        // parse of the table of contents per file. Worth revisiting if
        // extracting large archives turns out to be slow.
        let file = File::open(&self.path)?;
        let mut archive = zip::ZipArchive::new(file).map_err(io::Error::other)?;
        let mut entry = archive.by_index(slot).map_err(io::Error::other)?;

        let mut buffer = vec![0u8; 64 * 1024];
        loop {
            let read = entry.read(&mut buffer)?;
            if read == 0 || !emit(buffer[..read].to_vec()) {
                return Ok(());
            }
        }
    }
}

/// A ZIP timestamp as an instant.
///
/// ZIP inherited MS-DOS dates: two-second resolution, and no time zone at all.
/// Like most tools, we read them as UTC, because there is nothing better to do.
fn to_system_time(stamp: zip::DateTime) -> Option<SystemTime> {
    let days = days_from_civil(
        stamp.year().into(),
        stamp.month().into(),
        stamp.day().into(),
    );
    let seconds = days * 86_400
        + i64::from(stamp.hour()) * 3_600
        + i64::from(stamp.minute()) * 60
        + i64::from(stamp.second());
    let offset = Duration::from_secs(seconds.unsigned_abs());
    if seconds >= 0 {
        SystemTime::UNIX_EPOCH.checked_add(offset)
    } else {
        SystemTime::UNIX_EPOCH.checked_sub(offset)
    }
}

/// Days from 1970-01-01 to the given date, negative before it.
///
/// This is Howard Hinnant's `days_from_civil`: it shifts the year to start in
/// March so that the leap day lands at the end, which makes the whole thing
/// work out without any table of month lengths. `era` is a 400-year cycle, the
/// period after which the Gregorian calendar repeats exactly.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400; // 0..=399
    let day_of_year = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

fn local_path(container: &VPath) -> Result<PathBuf> {
    // An archive inside another archive, or on a server, has to be fetched
    // before it can be opened; nothing does that yet.
    let path = container.as_local().ok_or_else(|| Error::Unsupported {
        backend: "zip",
        path: container.clone(),
    })?;
    Ok(path.to_path_buf())
}

fn open_archive(container: &VPath, path: &PathBuf) -> Result<zip::ZipArchive<File>> {
    let file = File::open(path).map_err(|e| Error::from_io(container, e))?;
    zip::ZipArchive::new(file).map_err(|e| archive_error(container, e))
}

fn archive_error(container: &VPath, err: zip::result::ZipError) -> Error {
    match err {
        zip::result::ZipError::Io(source) => Error::from_io(container, source),
        other => Error::Archive {
            path: container.clone(),
            message: other.to_string().into(),
        },
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use tempfile::TempDir;
    use zip::write::{SimpleFileOptions, ZipWriter};

    use super::*;

    /// Writes a ZIP and returns its path. Each entry is `(name, contents)`;
    /// a name ending in `/` becomes a folder entry.
    fn make_zip(dir: &TempDir, name: &str, entries: &[(&str, &[u8])]) -> VPath {
        let path = dir.path().join(name);
        let mut zip = ZipWriter::new(File::create(&path).unwrap());
        let options =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
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

    fn names(reader: &ZipReader) -> Vec<&str> {
        reader.entries().iter().map(|e| e.name.as_str()).collect()
    }

    fn read_all(reader: &ZipReader, slot: usize) -> Vec<u8> {
        let mut out = Vec::new();
        reader
            .read_entry(slot, &mut |chunk| {
                out.extend_from_slice(&chunk);
                true
            })
            .unwrap();
        out
    }

    #[test]
    fn lists_what_the_zip_contains() {
        let tmp = TempDir::new().unwrap();
        let zip = make_zip(
            &tmp,
            "stuff.zip",
            &[
                ("notes.txt", b"hello"),
                ("photos/", b""),
                ("photos/img.jpg", b"JPEGDATA"),
            ],
        );

        let reader = ZipReader::open(&zip).unwrap();

        assert_eq!(names(&reader), ["notes.txt", "photos/", "photos/img.jpg"]);
        assert_eq!(reader.entries()[0].size, 5);
        assert_eq!(reader.entries()[2].size, 8);
    }

    #[test]
    fn reads_an_entry_back_exactly() {
        let tmp = TempDir::new().unwrap();
        // Big enough that it is really compressed and really streamed.
        let big: Vec<u8> = (0..200_000).map(|i| (i % 251) as u8).collect();
        let zip = make_zip(
            &tmp,
            "stuff.zip",
            &[("small.txt", b"hello"), ("big.bin", &big)],
        );

        let reader = ZipReader::open(&zip).unwrap();
        let slot_of = |name: &str| {
            reader
                .entries()
                .iter()
                .find(|e| e.name == name)
                .unwrap()
                .slot
        };

        assert_eq!(read_all(&reader, slot_of("small.txt")), b"hello");
        assert_eq!(read_all(&reader, slot_of("big.bin")), big);
    }

    #[test]
    fn an_empty_zip_lists_nothing() {
        let tmp = TempDir::new().unwrap();
        let zip = make_zip(&tmp, "empty.zip", &[]);

        let reader = ZipReader::open(&zip).unwrap();

        assert!(reader.entries().is_empty());
    }

    #[test]
    fn stored_timestamps_come_back() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("dated.zip");
        let mut zip = ZipWriter::new(File::create(&path).unwrap());
        // ZIP keeps MS-DOS dates: two-second resolution, no time zone.
        let stamp = zip::DateTime::from_date_and_time(2024, 3, 17, 14, 30, 10).unwrap();
        zip.start_file(
            "dated.txt",
            SimpleFileOptions::default().last_modified_time(stamp),
        )
        .unwrap();
        zip.write_all(b"x").unwrap();
        zip.finish().unwrap();

        let reader = ZipReader::open(&VPath::local(&path).unwrap()).unwrap();

        // 2024-03-17 14:30:10 UTC
        let expected = SystemTime::UNIX_EPOCH + Duration::from_secs(1_710_685_810);
        assert_eq!(reader.entries()[0].modified, Some(expected));
    }

    #[test]
    fn reading_stops_early_when_nobody_is_listening() {
        let tmp = TempDir::new().unwrap();
        let big: Vec<u8> = (0..500_000).map(|i| (i % 251) as u8).collect();
        let zip = make_zip(&tmp, "stuff.zip", &[("big.bin", &big)]);
        let reader = ZipReader::open(&zip).unwrap();

        let mut chunks = 0;
        reader
            .read_entry(0, &mut |_| {
                chunks += 1;
                false // give up immediately
            })
            .unwrap();

        assert_eq!(chunks, 1, "it should stop after the first refused chunk");
    }

    #[test]
    fn ms_dos_dates_convert_to_the_right_instant() {
        let cases = [
            // (y, m, d, h, min, s, seconds since the epoch)
            (1980, 1, 1, 0, 0, 0, 315_532_800),
            (2000, 1, 1, 0, 0, 0, 946_684_800),
            (2024, 2, 29, 12, 0, 0, 1_709_208_000), // a leap day
            (2024, 3, 17, 14, 30, 10, 1_710_685_810),
            (2099, 12, 31, 23, 59, 58, 4_102_444_798),
        ];

        for (y, m, d, h, min, s, expected) in cases {
            let stamp = zip::DateTime::from_date_and_time(y, m, d, h, min, s).unwrap();
            assert_eq!(
                to_system_time(stamp),
                Some(SystemTime::UNIX_EPOCH + Duration::from_secs(expected)),
                "{y}-{m:02}-{d:02} {h:02}:{min:02}:{s:02}"
            );
        }
    }

    #[test]
    fn something_that_is_not_a_zip_is_rejected_clearly() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("notazip.zip");
        std::fs::write(&path, b"this is just some text").unwrap();

        let err = ZipReader::open(&VPath::local(&path).unwrap()).unwrap_err();

        assert!(
            matches!(err, Error::Archive { .. }),
            "expected an archive error, got {err:?}"
        );
    }

    #[test]
    fn a_zip_that_is_not_there_is_a_plain_not_found() {
        let tmp = TempDir::new().unwrap();
        let missing = VPath::local(tmp.path().join("gone.zip")).unwrap();

        let err = ZipReader::open(&missing).unwrap_err();

        assert!(matches!(err, Error::NotFound(_)), "got {err:?}");
    }

    #[test]
    fn a_zip_inside_an_archive_is_not_supported_yet() {
        // `VPath` can already address it; this backend can only open real files.
        let tmp = TempDir::new().unwrap();
        let outer = make_zip(&tmp, "outer.zip", &[("inner.zip", b"")]);
        let nested = outer.enter("zip").unwrap().join("inner.zip").unwrap();

        let err = ZipReader::open(&nested).unwrap_err();

        assert!(matches!(err, Error::Unsupported { .. }), "got {err:?}");
    }
}
