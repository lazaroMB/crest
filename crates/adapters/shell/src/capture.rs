//! Bounded capture of a process output stream.

use std::io;

use tokio::io::{AsyncRead, AsyncReadExt};

/// Bytes kept from each of stdout and stderr; the rest is read and dropped
/// so the process never blocks on a full pipe.
pub const MAX_CAPTURE_BYTES: usize = 1024 * 1024;

/// What has been read so far from one stream, at most `limit` bytes.
#[derive(Debug)]
pub struct Capture {
    bytes: Vec<u8>,
    limit: usize,
    truncated: bool,
}

impl Capture {
    /// An empty capture keeping up to `limit` bytes.
    pub fn new(limit: usize) -> Self {
        Self {
            bytes: Vec::new(),
            limit,
            truncated: false,
        }
    }

    /// Whether bytes beyond the limit were dropped.
    pub fn truncated(&self) -> bool {
        self.truncated
    }

    /// The kept bytes, decoded lossily as UTF-8. A multi-byte character cut
    /// by the limit is dropped instead of turned into a replacement
    /// character.
    pub fn text(&self) -> String {
        let bytes = if self.truncated {
            match std::str::from_utf8(&self.bytes) {
                Err(err) if err.error_len().is_none() => &self.bytes[..err.valid_up_to()],
                _ => &self.bytes,
            }
        } else {
            &self.bytes
        };
        String::from_utf8_lossy(bytes).into_owned()
    }

    fn push(&mut self, chunk: &[u8]) {
        let room = self.limit.saturating_sub(self.bytes.len());
        if chunk.len() > room {
            self.truncated = true;
        }
        self.bytes
            .extend_from_slice(&chunk[..chunk.len().min(room)]);
    }

    /// Reads `reader` to the end. Works chunk by chunk, so if this future is
    /// dropped (on timeout) the bytes read so far stay in the capture.
    pub async fn read_from<R: AsyncRead + Unpin>(&mut self, reader: Option<R>) -> io::Result<()> {
        let Some(mut reader) = reader else {
            return Ok(());
        };
        let mut chunk = [0u8; 8192];
        loop {
            let n = reader.read(&mut chunk).await?;
            if n == 0 {
                return Ok(());
            }
            self.push(&chunk[..n]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn keeps_everything_below_the_limit() {
        let mut capture = Capture::new(10);
        capture.read_from(Some(&b"hello"[..])).await.unwrap();
        assert_eq!(capture.text(), "hello");
        assert!(!capture.truncated());
    }

    #[tokio::test]
    async fn exactly_the_limit_is_not_truncated() {
        let mut capture = Capture::new(5);
        capture.read_from(Some(&b"hello"[..])).await.unwrap();
        assert_eq!(capture.text(), "hello");
        assert!(!capture.truncated());
    }

    #[tokio::test]
    async fn drops_bytes_beyond_the_limit() {
        let mut capture = Capture::new(3);
        capture.read_from(Some(&b"hello"[..])).await.unwrap();
        assert_eq!(capture.text(), "hel");
        assert!(capture.truncated());
    }

    #[tokio::test]
    async fn does_not_split_a_character() {
        let mut capture = Capture::new(2);
        capture.read_from(Some("añb".as_bytes())).await.unwrap();
        assert_eq!(capture.text(), "a");
        assert!(capture.truncated());
    }
}
