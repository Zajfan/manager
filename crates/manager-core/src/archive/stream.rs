//! Turning a blocking reader into one the rest of the app can await.
//!
//! Archive libraries hand out plain blocking [`Read`]s, but [`crate::Vfs`]
//! promises an `AsyncRead`. Reading the whole entry into memory first would be
//! simpler, and would also mean a 4 GB file inside a ZIP takes 4 GB of RAM to
//! copy out. So instead a task on the blocking pool pulls the entry through in
//! chunks and hands them over one at a time.

use std::io::{self, Read};
use std::pin::Pin;
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, ReadBuf};
use tokio::sync::mpsc::{self, Receiver};

/// How much is pulled from the archive at a time.
const CHUNK: usize = 64 * 1024;
/// How many chunks may sit in the channel before the reader has to wait. This
/// is what stops a fast archive from filling memory faster than the
/// destination disk can take it.
const BACKLOG: usize = 4;

/// An [`AsyncRead`] fed by a blocking reader on the blocking pool.
pub struct ChunkReader {
    chunks: Receiver<io::Result<Vec<u8>>>,
    /// The chunk being handed out, and how much of it has gone.
    current: Option<(Vec<u8>, usize)>,
}

impl ChunkReader {
    /// Starts a task that pushes chunks out through the reader.
    ///
    /// `work` is handed an `emit` callback; `emit` returns `false` once nobody
    /// is reading any more, which is the signal to stop early. Use this for
    /// sources that can't hand out an owned [`Read`], such as archive entries.
    pub fn pump<F>(work: F) -> ChunkReader
    where
        F: FnOnce(&mut dyn FnMut(Vec<u8>) -> bool) -> io::Result<()> + Send + 'static,
    {
        let (tx, chunks) = mpsc::channel(BACKLOG);
        tokio::task::spawn_blocking(move || {
            let sender = tx.clone();
            // A closed channel means the reader was dropped: stop reading.
            let mut emit = move |chunk: Vec<u8>| sender.blocking_send(Ok(chunk)).is_ok();
            if let Err(failure) = work(&mut emit) {
                let _ = tx.blocking_send(Err(failure));
            }
        });
        ChunkReader {
            chunks,
            current: None,
        }
    }

    /// Starts pumping `source` in the background.
    ///
    /// The pump stops on its own when the `ChunkReader` is dropped, so
    /// abandoning a copy doesn't leave a thread reading a huge file.
    pub fn spawn(mut source: impl Read + Send + 'static) -> ChunkReader {
        ChunkReader::pump(move |emit| {
            let mut buffer = vec![0u8; CHUNK];
            loop {
                match source.read(&mut buffer) {
                    Ok(0) => return Ok(()),
                    Ok(n) if emit(buffer[..n].to_vec()) => {}
                    // Either the source is done or nobody is listening.
                    Ok(_) => return Ok(()),
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                    Err(e) => return Err(e),
                }
            }
        })
    }
}

impl AsyncRead for ChunkReader {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let me = self.get_mut();
        loop {
            if let Some((chunk, used)) = &mut me.current {
                let take = buf.remaining().min(chunk.len() - *used);
                buf.put_slice(&chunk[*used..*used + take]);
                *used += take;
                if *used == chunk.len() {
                    me.current = None;
                }
                return Poll::Ready(Ok(()));
            }
            match me.chunks.poll_recv(cx) {
                Poll::Ready(Some(Ok(chunk))) => me.current = Some((chunk, 0)),
                Poll::Ready(Some(Err(e))) => return Poll::Ready(Err(e)),
                // The pump finished: end of file.
                Poll::Ready(None) => return Poll::Ready(Ok(())),
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use tokio::io::AsyncReadExt;

    use super::*;

    /// Bytes that don't repeat, so a mix-up in chunk order would show.
    fn data(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i % 251) as u8).collect()
    }

    #[tokio::test]
    async fn hands_over_everything_the_source_had() {
        // Several chunks' worth, and deliberately not a whole number of them.
        let original = data(CHUNK * 3 + 17);
        let mut reader = ChunkReader::spawn(Cursor::new(original.clone()));

        let mut got = Vec::new();
        reader.read_to_end(&mut got).await.unwrap();

        assert_eq!(got.len(), original.len());
        assert!(got == original, "the bytes came back in the wrong order");
    }

    #[tokio::test]
    async fn a_pushing_source_is_read_back_in_order() {
        let original = data(CHUNK + 5);
        let copy = original.clone();
        let mut reader = ChunkReader::pump(move |emit| {
            for part in copy.chunks(1000) {
                if !emit(part.to_vec()) {
                    return Ok(());
                }
            }
            Ok(())
        });

        let mut got = Vec::new();
        reader.read_to_end(&mut got).await.unwrap();

        assert!(got == original);
    }

    #[tokio::test]
    async fn a_pushing_source_reports_its_failure() {
        let mut reader = ChunkReader::pump(|emit| {
            emit(b"partial".to_vec());
            Err(io::Error::new(io::ErrorKind::UnexpectedEof, "truncated"))
        });

        let mut got = Vec::new();
        let err = reader.read_to_end(&mut got).await.unwrap_err();

        assert_eq!(err.to_string(), "truncated");
    }

    #[tokio::test]
    async fn a_pushing_source_is_told_to_stop_when_the_reader_is_dropped() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};

        let stopped = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stopped);
        // An endless source: the only way this ever finishes is by being told to.
        let reader = ChunkReader::pump(move |emit| {
            while emit(vec![0u8; 1024]) {}
            flag.store(true, Ordering::SeqCst);
            Ok(())
        });

        drop(reader);

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !stopped.load(Ordering::SeqCst) && std::time::Instant::now() < deadline {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(
            stopped.load(Ordering::SeqCst),
            "the source was never told to stop, so it would read forever"
        );
    }

    #[tokio::test]
    async fn an_empty_source_is_just_end_of_file() {
        let mut reader = ChunkReader::spawn(Cursor::new(Vec::new()));

        let mut got = Vec::new();
        reader.read_to_end(&mut got).await.unwrap();

        assert!(got.is_empty());
    }

    #[tokio::test]
    async fn a_source_that_dribbles_out_one_byte_at_a_time_still_works() {
        struct Dribble(Vec<u8>, usize);
        impl Read for Dribble {
            fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
                if self.1 >= self.0.len() || buf.is_empty() {
                    return Ok(0);
                }
                buf[0] = self.0[self.1];
                self.1 += 1;
                Ok(1)
            }
        }

        let original = data(500);
        let mut reader = ChunkReader::spawn(Dribble(original.clone(), 0));

        let mut got = Vec::new();
        reader.read_to_end(&mut got).await.unwrap();

        assert_eq!(got, original);
    }

    #[tokio::test]
    async fn a_failure_partway_through_reaches_the_reader() {
        struct FailsAfterOneChunk(usize);
        impl Read for FailsAfterOneChunk {
            fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
                if self.0 == 0 {
                    self.0 += 1;
                    buf[..10].fill(b'x');
                    return Ok(10);
                }
                Err(io::Error::new(io::ErrorKind::InvalidData, "corrupt entry"))
            }
        }

        let mut reader = ChunkReader::spawn(FailsAfterOneChunk(0));

        let mut got = Vec::new();
        let err = reader.read_to_end(&mut got).await.unwrap_err();

        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert_eq!(err.to_string(), "corrupt entry");
    }

    #[tokio::test]
    async fn an_interrupted_read_is_retried_rather_than_treated_as_the_end() {
        struct InterruptsOnce(bool);
        impl Read for InterruptsOnce {
            fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
                if !self.0 {
                    self.0 = true;
                    return Err(io::Error::from(io::ErrorKind::Interrupted));
                }
                if buf.is_empty() {
                    return Ok(0);
                }
                buf[0] = b'z';
                Ok(1)
            }
        }

        let mut reader = ChunkReader::spawn(InterruptsOnce(false).take(1));

        let mut got = Vec::new();
        reader.read_to_end(&mut got).await.unwrap();

        assert_eq!(got, b"z");
    }
}
