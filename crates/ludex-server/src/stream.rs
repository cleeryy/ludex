//! Bounded bridge between the synchronous core tar writer and an async HTTP
//! body.
//!
//! `stream_archive_as_tar` writes to a `std::io::Write`; the HTTP body is polled on
//! a Tokio worker. The adapter below copies bytes into a small bounded channel
//! from a blocking thread, so a slow client applies backpressure instead of
//! buffering the archive in memory. When the client disconnects the receiver is
//! dropped, `blocking_send` fails and the archive task stops.

use std::io::{self, Write};
use std::path::PathBuf;

use axum::body::Body;
use bytes::Bytes;
use ludex_core::archive::stream_archive_as_tar;
use tokio::sync::mpsc;

/// Size of each chunk sent to the HTTP body.
const CHUNK_SIZE: usize = 64 * 1024;
/// Number of chunks that may wait in the channel (about 512 KiB).
const CHANNEL_CAPACITY: usize = 8;

/// Builds a streaming tar body for a ZIP or 7z archive.
///
/// `expected_bytes` must be the exact tar size returned by
/// `ludex_core::archive::archive_tar_size`; it is used for the `Content-Length`
/// header and verified against the core report while streaming.
pub fn archive_tar_body(path: PathBuf, expected_bytes: u64) -> Body {
    let (sender, receiver) = mpsc::channel::<Result<Bytes, io::Error>>(CHANNEL_CAPACITY);

    tokio::task::spawn_blocking(move || {
        let error_sender = sender.clone();
        let mut writer = ChannelWriter::new(sender);
        let result = stream_archive_as_tar(&path, &mut writer);
        let flush_result = writer.flush();
        drop(writer);

        let failure = match (result, flush_result) {
            (Ok(report), Ok(())) if report.tar_bytes == expected_bytes => None,
            (Ok(report), Ok(())) => Some(format!(
                "tar size mismatch: expected {expected_bytes} bytes, wrote {}",
                report.tar_bytes
            )),
            (Err(error), _) => Some(format!("archive stream failed: {error}")),
            (Ok(_), Err(error)) => Some(format!("tar flush failed: {error}")),
        };

        if let Some(message) = failure {
            // The receiver is gone when the client disconnected; that is the
            // expected way this task stops.
            let _ = error_sender.blocking_send(Err(io::Error::other(message)));
        }
    });

    let stream = futures_util::stream::unfold(receiver, |mut receiver| async move {
        receiver.recv().await.map(|item| (item, receiver))
    });
    Body::from_stream(stream)
}

/// `std::io::Write` implementation that forwards fixed-size chunks to a Tokio
/// channel. Only used from `spawn_blocking`.
struct ChannelWriter {
    sender: mpsc::Sender<Result<Bytes, io::Error>>,
    buffer: Vec<u8>,
}

impl ChannelWriter {
    fn new(sender: mpsc::Sender<Result<Bytes, io::Error>>) -> Self {
        Self {
            sender,
            buffer: Vec::with_capacity(CHUNK_SIZE),
        }
    }

    fn send_buffer(&mut self) -> io::Result<()> {
        if self.buffer.is_empty() {
            return Ok(());
        }
        let chunk = std::mem::replace(&mut self.buffer, Vec::with_capacity(CHUNK_SIZE));
        self.sender
            .blocking_send(Ok(Bytes::from(chunk)))
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "client disconnected"))
    }
}

impl Write for ChannelWriter {
    fn write(&mut self, mut input: &[u8]) -> io::Result<usize> {
        let total = input.len();
        while !input.is_empty() {
            let room = CHUNK_SIZE.saturating_sub(self.buffer.len());
            let take = room.min(input.len());
            self.buffer.extend_from_slice(&input[..take]);
            input = &input[take..];
            if self.buffer.len() >= CHUNK_SIZE {
                self.send_buffer()?;
            }
        }
        Ok(total)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.send_buffer()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writer_chunks_and_flushes() {
        let (sender, mut receiver) = mpsc::channel(4);
        let mut writer = ChannelWriter::new(sender);
        let payload = vec![7_u8; CHUNK_SIZE + 123];
        writer.write_all(&payload).expect("write");
        writer.flush().expect("flush");
        drop(writer);

        let mut collected = Vec::new();
        while let Some(item) = receiver.blocking_recv() {
            collected.extend_from_slice(&item.expect("chunk"));
        }
        assert_eq!(collected, payload);
    }

    #[test]
    fn writer_stops_when_receiver_is_dropped() {
        let (sender, receiver) = mpsc::channel(1);
        drop(receiver);
        let mut writer = ChannelWriter::new(sender);
        let error = writer
            .write_all(&vec![1_u8; CHUNK_SIZE * 2])
            .expect_err("broken pipe");
        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
    }
}
