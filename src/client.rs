// SPDX-License-Identifier: BSD-3-Clause
use crate::{
    protocol::{Message, Outcome, Request, Response, REQUEST_LIMIT, RESPONSE_LIMIT},
    transport::Channel,
};
use serde_json::Value;
use std::{
    collections::VecDeque,
    io,
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::{ffi::OsStrExt, net::UnixStream},
    },
    path::Path,
    thread,
    time::{Duration, Instant},
};

pub struct Client {
    channel: Channel,
    next: u64,
    events: VecDeque<String>,
    requests: VecDeque<Request>,
    last_request: u64,
}
impl Client {
    pub fn connect(path: &Path) -> io::Result<Self> {
        let bytes = path.as_os_str().as_bytes();
        let mut address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
        if bytes.is_empty() || bytes.len() >= address.sun_path.len() || bytes.contains(&0) {
            return Err(io::Error::other("invalid Unix socket path"));
        }
        address.sun_family = libc::AF_UNIX as _;
        for (target, source) in address.sun_path.iter_mut().zip(bytes) {
            *target = *source as _;
        }
        let raw = unsafe {
            libc::socket(
                libc::AF_UNIX,
                libc::SOCK_STREAM | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC,
                0,
            )
        };
        if raw < 0 {
            return Err(io::Error::last_os_error());
        }
        let fd = unsafe { OwnedFd::from_raw_fd(raw) };
        let length = (std::mem::offset_of!(libc::sockaddr_un, sun_path) + bytes.len() + 1)
            as libc::socklen_t;
        // A full local backlog returns EAGAIN instead of blocking. Never retry.
        if unsafe {
            libc::connect(
                fd.as_raw_fd(),
                (&address as *const libc::sockaddr_un).cast(),
                length,
            )
        } != 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            channel: Channel::new(UnixStream::from(fd), RESPONSE_LIMIT)?,
            next: 1,
            events: VecDeque::new(),
            requests: VecDeque::new(),
            last_request: 0,
        })
    }
    pub fn send(&mut self, command: &str, params: Value) -> io::Result<u64> {
        let request = Request::new(self.next, command, params);
        if !request.valid() {
            return Err(io::Error::other("invalid IPC command or parameters"));
        }
        self.next = self
            .next
            .checked_add(1)
            .ok_or_else(|| io::Error::other("IPC request IDs exhausted"))?;
        self.channel
            .queue(&Message::Request(request), REQUEST_LIMIT)?;
        Ok(self.next - 1)
    }
    pub fn poll(&mut self) -> io::Result<Option<Response>> {
        self.channel.flush()?;
        match self.channel.receive_frame::<Message>()? {
            None => Ok(None),
            Some((Message::Response(r), _)) => Ok(Some(r)),
            Some((Message::Request(r), size))
                if r.valid()
                    && r.id > self.last_request
                    && self.requests.len() < 4
                    && size <= REQUEST_LIMIT =>
            {
                self.last_request = r.id;
                self.requests.push_back(r);
                Ok(None)
            }
            Some((Message::Event { event, params }, _))
                if event == "igui.close" && params.is_object() && self.events.len() < 4 =>
            {
                self.events.push_back(event);
                Ok(None)
            }
            _ => Err(io::Error::other("unexpected IPC message")),
        }
    }
    pub fn take_request(&mut self) -> Option<Request> {
        self.requests.pop_front()
    }
    pub fn reply(&mut self, response: Response) -> io::Result<()> {
        self.channel
            .queue(&Message::Response(response), RESPONSE_LIMIT)
    }
    pub fn close_requested(&mut self) -> bool {
        self.events.pop_front().is_some()
    }
    /// Bounded blocking convenience for bootstrap/CLI, not for the GUI frame loop.
    pub fn call(&mut self, command: &str, params: Value) -> io::Result<Response> {
        let id = self.send(command, params)?;
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(response) = self.poll()? {
                if response.id != id {
                    return Err(io::Error::other("IPC response ID mismatch"));
                }
                return Ok(response);
            }
            if Instant::now() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "IPC request timed out; not retried",
                ));
            }
            thread::sleep(Duration::from_millis(2));
        }
    }
    pub fn value(&mut self, command: &str, params: Value) -> io::Result<Value> {
        match self.call(command, params)?.outcome {
            Outcome::Done { result } => Ok(result),
            Outcome::Error { error } => Err(io::Error::other(error)),
            Outcome::Accepted { .. } => Err(io::Error::other("unexpected asynchronous operation")),
        }
    }
}
