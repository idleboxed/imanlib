// SPDX-License-Identifier: BSD-3-Clause
//! Nonblocking, length-prefixed JSON. One frame per poll, bounded output queue.
use serde::{de::DeserializeOwned, Serialize};
use std::{
    collections::VecDeque,
    io::{self, Read, Write},
    os::unix::net::UnixStream,
    time::{Duration, Instant},
};

pub const IO_TIMEOUT: Duration = Duration::from_secs(5);

struct FrameWriter {
    bytes: Vec<u8>,
    limit: usize,
}
impl Write for FrameWriter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        if buffer.len() > self.limit - (self.bytes.len() - 4) {
            return Err(io::Error::other("IPC output limit exceeded"));
        }
        let end = self.bytes.len() + buffer.len();
        if end > self.bytes.capacity() {
            let capacity = self.bytes.capacity().saturating_mul(2)
                .max(end).min(self.limit + 4);
            self.bytes.try_reserve_exact(capacity - self.bytes.len())
                .map_err(io::Error::other)?;
        }
        self.bytes.extend_from_slice(buffer);
        Ok(buffer.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub struct Channel {
    stream: UnixStream,
    limit: usize,
    header: [u8; 4],
    header_used: usize,
    body: Vec<u8>,
    body_used: usize,
    input_started: Option<Instant>,
    output: VecDeque<Vec<u8>>,
    sent: usize,
    output_started: Option<Instant>,
}
impl Channel {
    /// Configure a Unix stream for nonblocking, bounded framed JSON transport.
    pub fn new(stream: UnixStream, limit: usize) -> io::Result<Self> {
        stream.set_nonblocking(true)?;
        Ok(Self {
            stream,
            limit,
            header: [0; 4],
            header_used: 0,
            body: Vec::new(),
            body_used: 0,
            input_started: None,
            output: VecDeque::new(),
            sent: 0,
            output_started: None,
        })
    }
    /// Borrow the held Unix stream without transferring its ownership.
    pub fn stream(&self) -> &UnixStream {
        &self.stream
    }
    /// Report whether any queued frame still needs writing.
    pub fn pending_output(&self) -> bool {
        !self.output.is_empty()
    }
    /// Report whether a frame read has begun but has not completed.
    pub(crate) fn has_partial_input(&self) -> bool {
        self.input_started.is_some()
    }
    /// Serialize and queue one bounded frame, rejecting an oversized frame or full output queue.
    pub fn queue(&mut self, value: &impl Serialize, limit: usize) -> io::Result<()> {
        if self.output.len() >= 4 {
            return Err(io::Error::other("IPC output limit exceeded"));
        }
        let mut frame = FrameWriter {
            bytes: vec![0; 4],
            limit: limit.min(u32::MAX as usize).min(usize::MAX - 4),
        };
        serde_json::to_writer(&mut frame, value)?;
        let size = frame.bytes.len() - 4;
        if size == 0 {
            return Err(io::Error::other("IPC output limit exceeded"));
        }
        frame.bytes[..4].copy_from_slice(&(size as u32).to_be_bytes());
        self.output.push_back(frame.bytes);
        self.output_started.get_or_insert_with(Instant::now);
        Ok(())
    }
    /// Write as much queued output as possible without blocking; report timeout and I/O failures.
    pub fn flush(&mut self) -> io::Result<()> {
        if self
            .output_started
            .is_some_and(|timestamp| timestamp.elapsed() > IO_TIMEOUT)
        {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "IPC writer stalled",
            ));
        }
        if let Some(front) = self.output.front() {
            match self.stream.write(&front[self.sent..]) {
                Ok(0) => return Err(io::Error::new(io::ErrorKind::WriteZero, "IPC peer closed")),
                Ok(bytes_processed) => self.sent += bytes_processed,
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                    ) =>
                {
                    return Ok(())
                }
                Err(error) => return Err(error),
            }
            if self.sent == front.len() {
                self.output.pop_front();
                self.sent = 0;
                self.output_started = (!self.output.is_empty()).then(Instant::now);
            }
        }
        Ok(())
    }
    /// Read and decode one complete bounded frame, or return None until more bytes arrive.
    pub fn receive<T: DeserializeOwned>(&mut self) -> io::Result<Option<T>> {
        self.receive_frame().map(|incoming| incoming.map(|(value, _)| value))
    }
    /// Retain the wire size so ignored JSON fields still count against message limits.
    pub(crate) fn receive_frame<T: DeserializeOwned>(&mut self) -> io::Result<Option<(T, usize)>> {
        if self.input_started.is_some_and(|timestamp| timestamp.elapsed() > IO_TIMEOUT) {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "IPC partial frame timed out",
            ));
        }
        // Header and body each get at most one read; a chatty peer cannot monopolize a tick.
        for _ in 0..2 {
            let target = if self.header_used < 4 {
                &mut self.header[self.header_used..]
            } else {
                &mut self.body[self.body_used..]
            };
            match self.stream.read(target) {
                Ok(0) => {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "IPC peer disconnected",
                    ))
                }
                Ok(bytes_processed) => {
                    self.input_started.get_or_insert_with(Instant::now);
                    if self.header_used < 4 {
                        self.header_used += bytes_processed;
                        if self.header_used == 4 {
                            let size = u32::from_be_bytes(self.header) as usize;
                            if size == 0 || size > self.limit {
                                return Err(io::Error::other("IPC input limit exceeded"));
                            }
                            self.body.resize(size, 0);
                        }
                    } else {
                        self.body_used += bytes_processed;
                        if self.body_used == self.body.len() {
                            let size = self.body.len();
                            // Deserializer diagnostics can include untrusted field values.
                            let result = serde_json::from_slice(&self.body).map_err(|_| {
                                io::Error::new(io::ErrorKind::InvalidData, "Invalid IPC JSON message")
                            });
                            self.header_used = 0;
                            self.body_used = 0;
                            self.body.clear();
                            self.input_started = None;
                            return result.map(|value| Some((value, size)));
                        }
                    }
                }
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                    ) =>
                {
                    return Ok(None)
                }
                Err(error) => return Err(error),
            }
        }
        Ok(None)
    }
}
