//! Reading TAR archives, plain or gzipped.
//!
//! A TAR has no table of contents: it is just entries one after another, each
//! with a header saying how long it is. Listing one therefore means walking
//! the whole file. For a plain `.tar` that walk records where each entry's
//! data starts, so reading one afterwards is a seek.
//!
//! A `.tar.gz` can't be seeked at all, so it's decompressed once into a
//! temporary file when it's opened and treated as a plain TAR from then on.
//! The alternative — decompressing from the start for every file — turns
//! extracting an archive into quadratic work.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use tempfile::NamedTempFile;

use crate::archive::index::RawEntry;
use crate::{Error, Result, VPath};

/// The first two bytes of every gzip stream.
const GZIP_MAGIC: [u8; 2] = [0x1f, 0x8b];

/// A TAR archive whose entries have been located.
#[derive(Debug)]
pub struct TarReader {
    source: Source,
    entries: Vec<RawEntry>,
    /// Where each entry's data starts, and how long it is, by slot.
    positions: Vec<(u64, u64)>,
}

/// The seekable, uncompressed TAR stream.
#[derive(Debug)]
enum Source {
    /// A plain `.tar` read straight from disk.
    OnDisk(PathBuf),
    /// A compressed archive, decompressed once. The file is deleted when this
    /// is dropped, so it lives exactly as long as the open archive.
    Unpacked(NamedTempFile),
}

impl TarReader {
    /// Walks the archive at `container`, recording where everything is.
    pub fn open(container: &VPath) -> Result<TarReader> {
        // An archive inside another archive would have to be extracted first.
        let native = container.as_local().ok_or_else(|| Error::Unsupported {
            backend: "tar",
            path: container.clone(),
        })?;
        let file = File::open(native).map_err(|e| Error::from_io(container, e))?;

        let source = if is_gzipped(container, &file)? {
            let mut unpacked = NamedTempFile::new().map_err(|e| Error::from_io(container, e))?;
            let mut decoder = flate2::read::GzDecoder::new(file);
            io::copy(&mut decoder, unpacked.as_file_mut())
                .map_err(|e| unreadable(container, &e))?;
            Source::Unpacked(unpacked)
        } else {
            Source::OnDisk(native.to_path_buf())
        };

        let mut entries = Vec::new();
        let mut positions = Vec::new();
        let stream = source.open().map_err(|e| Error::from_io(container, e))?;
        let mut archive = tar::Archive::new(stream);
        let walk = archive.entries().map_err(|e| unreadable(container, &e))?;
        for item in walk {
            let entry = item.map_err(|e| unreadable(container, &e))?;
            let header = entry.header();
            let kind = header.entry_type();
            // Bookkeeping entries that carry the next entry's long name or
            // extended attributes. The `tar` crate already applies them.
            if kind.is_pax_global_extensions() || kind.is_gnu_longname() || kind.is_gnu_longlink() {
                continue;
            }
            let path = entry.path().map_err(|e| unreadable(container, &e))?;
            let mut name = path.to_string_lossy().into_owned();
            // The index decides what's a folder by the trailing slash, and not
            // every tar writer includes it.
            if kind.is_dir() && !name.ends_with('/') {
                name.push('/');
            }
            let size = entry.size();
            positions.push((entry.raw_file_position(), size));
            entries.push(RawEntry {
                name,
                size,
                modified: header
                    .mtime()
                    .ok()
                    .map(|secs| SystemTime::UNIX_EPOCH + Duration::from_secs(secs)),
                slot: entries.len(),
            });
        }

        Ok(TarReader {
            source,
            entries,
            positions,
        })
    }

    pub fn entries(&self) -> &[RawEntry] {
        &self.entries
    }

    /// Reads one entry, handing its bytes to `emit` a chunk at a time.
    pub fn read_entry(&self, slot: usize, emit: &mut dyn FnMut(Vec<u8>) -> bool) -> io::Result<()> {
        let (offset, size) = *self
            .positions
            .get(slot)
            .ok_or_else(|| io::Error::other("no such entry in the archive"))?;

        let mut stream = self.source.open()?;
        stream.seek(SeekFrom::Start(offset))?;

        let mut buffer = vec![0u8; 64 * 1024];
        let mut left = size;
        while left > 0 {
            // Never read past this entry into the next one's header.
            let want = buffer.len().min(left as usize);
            let read = stream.read(&mut buffer[..want])?;
            if read == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "the archive ends in the middle of this file",
                ));
            }
            left -= read as u64;
            if !emit(buffer[..read].to_vec()) {
                return Ok(());
            }
        }
        Ok(())
    }
}

impl Source {
    /// A fresh handle, positioned at the start.
    fn open(&self) -> io::Result<File> {
        match self {
            Source::OnDisk(path) => File::open(path),
            Source::Unpacked(temp) => temp.reopen(),
        }
    }
}

/// Peeks at the first two bytes without disturbing the file position.
fn is_gzipped(container: &VPath, file: &File) -> Result<bool> {
    let mut magic = [0u8; 2];
    match read_at_start(file, &mut magic) {
        Ok(read) => Ok(read == magic.len() && magic == GZIP_MAGIC),
        Err(e) => Err(Error::from_io(container, e)),
    }
}

fn read_at_start(mut file: &File, buffer: &mut [u8]) -> io::Result<usize> {
    file.seek(SeekFrom::Start(0))?;
    let read = file.read(buffer)?;
    file.seek(SeekFrom::Start(0))?;
    Ok(read)
}

/// Anything that goes wrong while walking a TAR means the file isn't one, or
/// isn't a whole one. Either way the useful thing to say is that it can't be read.
fn unreadable(container: &VPath, err: &io::Error) -> Error {
    Error::Archive {
        path: container.clone(),
        message: err.to_string().into(),
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use tempfile::TempDir;

    use super::*;

    /// Writes a TAR, gzipped if `compress`. A name ending in `/` is a folder.
    fn make_tar(dir: &TempDir, name: &str, entries: &[(&str, &[u8])], compress: bool) -> VPath {
        let path = dir.path().join(name);
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
                        .append_data(&mut header, format!("{folder}/"), io::empty())
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
            let mut encoder =
                flate2::write::GzEncoder::new(&mut out, flate2::Compression::default());
            encoder.write_all(&raw).unwrap();
            encoder.finish().unwrap();
        } else {
            out.write_all(&raw).unwrap();
        }
        VPath::local(path).unwrap()
    }

    fn names(reader: &TarReader) -> Vec<&str> {
        reader.entries().iter().map(|e| e.name.as_str()).collect()
    }

    fn slot_of(reader: &TarReader, name: &str) -> usize {
        reader
            .entries()
            .iter()
            .find(|e| e.name == name)
            .unwrap_or_else(|| panic!("no entry called {name:?} in {:?}", names(reader)))
            .slot
    }

    fn read_all(reader: &TarReader, slot: usize) -> Vec<u8> {
        let mut out = Vec::new();
        reader
            .read_entry(slot, &mut |chunk| {
                out.extend_from_slice(&chunk);
                true
            })
            .unwrap();
        out
    }

    const SAMPLE: &[(&str, &[u8])] = &[
        ("notes.txt", b"hello"),
        ("photos/", b""),
        ("photos/img.jpg", b"JPEGDATA"),
    ];

    #[test]
    fn lists_what_a_tar_contains() {
        let tmp = TempDir::new().unwrap();
        let reader = TarReader::open(&make_tar(&tmp, "stuff.tar", SAMPLE, false)).unwrap();

        assert_eq!(names(&reader), ["notes.txt", "photos/", "photos/img.jpg"]);
    }

    #[test]
    fn folder_entries_keep_their_trailing_slash() {
        // That trailing slash is how the index knows it's a folder.
        let tmp = TempDir::new().unwrap();
        let reader = TarReader::open(&make_tar(&tmp, "stuff.tar", SAMPLE, false)).unwrap();

        let folder = reader
            .entries()
            .iter()
            .find(|e| e.name.contains("photos") && e.name.ends_with('/'));
        assert!(folder.is_some(), "no folder entry in {:?}", names(&reader));
    }

    #[test]
    fn reads_entries_back_exactly() {
        let tmp = TempDir::new().unwrap();
        // Big enough to span several chunks.
        let big: Vec<u8> = (0..200_000).map(|i| (i % 251) as u8).collect();
        let entries: Vec<(&str, &[u8])> = vec![("small.txt", b"hello"), ("big.bin", &big)];
        let reader = TarReader::open(&make_tar(&tmp, "stuff.tar", &entries, false)).unwrap();

        assert_eq!(read_all(&reader, slot_of(&reader, "small.txt")), b"hello");
        assert!(read_all(&reader, slot_of(&reader, "big.bin")) == big);
    }

    #[test]
    fn a_gzipped_tar_reads_the_same_as_a_plain_one() {
        let tmp = TempDir::new().unwrap();
        let big: Vec<u8> = (0..200_000).map(|i| (i % 251) as u8).collect();
        let entries: Vec<(&str, &[u8])> = vec![("notes.txt", b"hello"), ("big.bin", &big)];
        let reader = TarReader::open(&make_tar(&tmp, "stuff.tar.gz", &entries, true)).unwrap();

        assert_eq!(names(&reader), ["notes.txt", "big.bin"]);
        assert_eq!(read_all(&reader, slot_of(&reader, "notes.txt")), b"hello");
        assert!(read_all(&reader, slot_of(&reader, "big.bin")) == big);
    }

    #[test]
    fn reading_the_same_entry_twice_gives_the_same_bytes() {
        // Reads seek rather than continuing where the last one stopped.
        let tmp = TempDir::new().unwrap();
        let reader = TarReader::open(&make_tar(&tmp, "stuff.tar.gz", SAMPLE, true)).unwrap();
        let slot = slot_of(&reader, "notes.txt");

        assert_eq!(read_all(&reader, slot), b"hello");
        assert_eq!(read_all(&reader, slot), b"hello");
    }

    #[test]
    fn sizes_and_times_survive() {
        let tmp = TempDir::new().unwrap();
        let reader = TarReader::open(&make_tar(&tmp, "stuff.tar", SAMPLE, false)).unwrap();

        let notes = &reader.entries()[slot_of(&reader, "notes.txt")];
        assert_eq!(notes.size, 5);
        assert_eq!(
            notes.modified,
            Some(SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000))
        );
    }

    #[test]
    fn an_empty_tar_lists_nothing() {
        let tmp = TempDir::new().unwrap();
        let reader = TarReader::open(&make_tar(&tmp, "empty.tar", &[], false)).unwrap();

        assert!(reader.entries().is_empty());
    }

    #[test]
    fn reading_stops_early_when_nobody_is_listening() {
        let tmp = TempDir::new().unwrap();
        let big: Vec<u8> = vec![7; 500_000];
        let entries: Vec<(&str, &[u8])> = vec![("big.bin", &big)];
        let reader = TarReader::open(&make_tar(&tmp, "stuff.tar", &entries, false)).unwrap();

        let mut chunks = 0;
        reader
            .read_entry(slot_of(&reader, "big.bin"), &mut |_| {
                chunks += 1;
                false
            })
            .unwrap();

        assert_eq!(chunks, 1);
    }

    #[test]
    fn something_that_is_not_a_tar_is_rejected_clearly() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("notatar.tar");
        std::fs::write(&path, b"nowhere near a tar header").unwrap();

        let err = TarReader::open(&VPath::local(&path).unwrap()).unwrap_err();

        assert!(matches!(err, Error::Archive { .. }), "got {err:?}");
    }

    #[test]
    fn a_tar_that_is_not_there_is_a_plain_not_found() {
        let tmp = TempDir::new().unwrap();
        let missing = VPath::local(tmp.path().join("gone.tar")).unwrap();

        let err = TarReader::open(&missing).unwrap_err();

        assert!(matches!(err, Error::NotFound(_)), "got {err:?}");
    }
}
