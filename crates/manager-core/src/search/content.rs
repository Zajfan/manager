//! Looking for text inside files.
//!
//! Files are read a chunk at a time and never held whole, so searching a
//! folder of huge logs costs a fixed amount of memory. The price of that is
//! having to be careful at the seams: a match can straddle two chunks, so each
//! chunk keeps the tail of the one before it.

use tokio::io::{AsyncRead, AsyncReadExt};

use crate::{Error, Result, VPath};

/// How much is read at a time.
const CHUNK: usize = 64 * 1024;

/// The text being looked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Needle {
    bytes: Vec<u8>,
    case_sensitive: bool,
}

impl Needle {
    /// `None` for an empty search, which means "don't search the contents".
    pub fn new(text: &str, case_sensitive: bool) -> Option<Needle> {
        if text.is_empty() {
            return None;
        }
        // Folding case on the bytes rather than the characters: it makes the
        // needle and the file comparable without decoding the file at all, at
        // the price of only ignoring case for ASCII. Anything else has to
        // match exactly.
        let bytes = if case_sensitive {
            text.as_bytes().to_vec()
        } else {
            text.as_bytes().to_ascii_lowercase()
        };
        Some(Needle {
            bytes,
            case_sensitive,
        })
    }

    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    /// Whether the text appears anywhere in `haystack`.
    pub fn is_in(&self, haystack: &[u8]) -> bool {
        if self.bytes.is_empty() || self.bytes.len() > haystack.len() {
            return false;
        }
        let windows = haystack.windows(self.bytes.len());
        if self.case_sensitive {
            windows.into_iter().any(|window| window == self.bytes)
        } else {
            windows
                .into_iter()
                .any(|window| window.eq_ignore_ascii_case(&self.bytes))
        }
    }
}

/// Reads `stream` to the end, or until the text turns up.
pub async fn stream_contains<R>(stream: &mut R, needle: &Needle, path: &VPath) -> Result<bool>
where
    R: AsyncRead + Unpin + ?Sized,
{
    if needle.is_empty() {
        return Ok(false);
    }
    // Carrying this much of the last chunk means a match is found however it
    // falls across the reads, whatever length it is.
    let overlap = needle.len() - 1;
    let mut carried: Vec<u8> = Vec::new();
    let mut chunk = vec![0u8; CHUNK];

    loop {
        let read = stream
            .read(&mut chunk)
            .await
            .map_err(|e| Error::from_io(path, e))?;
        if read == 0 {
            return Ok(false);
        }

        let mut window = Vec::with_capacity(carried.len() + read);
        window.extend_from_slice(&carried);
        window.extend_from_slice(&chunk[..read]);
        if needle.is_in(&window) {
            return Ok(true);
        }

        let keep = window.len().min(overlap);
        carried = window[window.len() - keep..].to_vec();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vp() -> VPath {
        VPath::parse("sftp://test/file").unwrap()
    }

    async fn search(body: &[u8], text: &str, case_sensitive: bool) -> bool {
        let needle = Needle::new(text, case_sensitive).unwrap();
        let mut reader = std::io::Cursor::new(body.to_vec());
        stream_contains(&mut reader, &needle, &vp()).await.unwrap()
    }

    #[test]
    fn an_empty_search_is_no_search_at_all() {
        assert_eq!(Needle::new("", false), None);
    }

    #[test]
    fn it_finds_text_anywhere_in_the_bytes() {
        let needle = Needle::new("needle", true).unwrap();
        assert!(needle.is_in(b"needle at the start"));
        assert!(needle.is_in(b"somewhere in the needle of it"));
        assert!(needle.is_in(b"at the end is the needle"));
        assert!(!needle.is_in(b"nothing like it here"));
    }

    #[test]
    fn case_can_be_ignored_or_insisted_on() {
        let loose = Needle::new("Error", false).unwrap();
        assert!(loose.is_in(b"ERROR: it broke"));
        assert!(loose.is_in(b"error: it broke"));

        let strict = Needle::new("Error", true).unwrap();
        assert!(strict.is_in(b"Error: it broke"));
        assert!(!strict.is_in(b"ERROR: it broke"));
    }

    #[test]
    fn a_needle_longer_than_the_haystack_is_simply_absent() {
        let needle = Needle::new("a very long search string", false).unwrap();
        assert!(!needle.is_in(b"short"));
        assert!(!needle.is_in(b""));
    }

    #[tokio::test]
    async fn it_finds_text_in_a_stream() {
        assert!(search(b"hello there world", "there", true).await);
        assert!(!search(b"hello there world", "absent", true).await);
    }

    #[tokio::test]
    async fn it_finds_text_lying_across_two_reads() {
        // The whole reason for keeping a tail: put the match right on the seam.
        let mut body = vec![b'.'; CHUNK - 3];
        body.extend_from_slice(b"SPLIT-HERE");
        body.extend(vec![b'.'; 100]);

        assert!(search(&body, "SPLIT-HERE", true).await);
    }

    #[tokio::test]
    async fn even_a_search_string_longer_than_a_chunk_is_found() {
        // The tail kept between reads is as long as the text being looked for,
        // so there is no length at which this quietly stops working.
        let needle = "x".repeat(CHUNK + 50);
        let mut body = vec![b'.'; 10];
        body.extend_from_slice(needle.as_bytes());
        body.extend(vec![b'.'; 10]);

        assert!(search(&body, &needle, true).await);
    }

    #[tokio::test]
    async fn it_reads_past_many_chunks_to_find_something_late() {
        let mut body = vec![b'.'; CHUNK * 3];
        body.extend_from_slice(b"finally");

        assert!(search(&body, "finally", true).await);
    }

    #[tokio::test]
    async fn an_empty_file_contains_nothing() {
        assert!(!search(b"", "anything", true).await);
    }
}
