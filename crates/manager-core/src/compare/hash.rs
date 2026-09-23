//! Hashing a file's contents, to tell "different" from "merely disturbed".
//!
//! A file's size and modified time are a cheap first check, but they can lie:
//! copying a file can leave its size the same and change nothing, or a build
//! tool can touch a file's timestamp without changing a byte of it. Hashing
//! the actual bytes with BLAKE3 (fast, and not the format this app is trying
//! to be compatible with anyone else's checksums for) is what settles it for
//! sure, at the cost of reading both files in full.

use crate::vfs::ReadStream;
use crate::{Error, Result, VPath};

/// How much is read at a time.
const CHUNK: usize = 256 * 1024;

/// Hashes the whole of a stream. Reads it to the end but holds none of it.
pub async fn hash_stream(mut stream: ReadStream, path: &VPath) -> Result<blake3::Hash> {
    use tokio::io::AsyncReadExt as _;

    let mut hasher = blake3::Hasher::new();
    let mut buffer = vec![0u8; CHUNK];
    loop {
        let read = stream
            .read(&mut buffer)
            .await
            .map_err(|e| Error::from_io(path, e))?;
        if read == 0 {
            return Ok(hasher.finalize());
        }
        hasher.update(&buffer[..read]);
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    fn stream(bytes: &[u8]) -> ReadStream {
        Box::new(Cursor::new(bytes.to_vec()))
    }

    fn vp() -> VPath {
        VPath::parse("sftp://test/file").unwrap()
    }

    #[tokio::test]
    async fn the_same_bytes_hash_the_same() {
        let a = hash_stream(stream(b"hello, world"), &vp()).await.unwrap();
        let b = hash_stream(stream(b"hello, world"), &vp()).await.unwrap();
        assert_eq!(a, b);
    }

    #[tokio::test]
    async fn different_bytes_hash_differently() {
        let a = hash_stream(stream(b"hello, world"), &vp()).await.unwrap();
        let b = hash_stream(stream(b"hello, world!"), &vp()).await.unwrap();
        assert_ne!(a, b);
    }

    #[tokio::test]
    async fn an_empty_file_still_hashes_to_something() {
        let hash = hash_stream(stream(b""), &vp()).await.unwrap();
        assert_eq!(hash, blake3::hash(b""));
    }

    #[tokio::test]
    async fn it_matches_the_reference_hash_for_bytes_bigger_than_one_chunk() {
        // Pins the chunking down: hashing has to see the whole file as one
        // stream of bytes, not chunk-by-chunk with something reset in between.
        let big: Vec<u8> = (0..CHUNK * 3 + 17).map(|i| (i % 251) as u8).collect();
        let hash = hash_stream(stream(&big), &vp()).await.unwrap();
        assert_eq!(hash, blake3::hash(&big));
    }
}
