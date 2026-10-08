// SPDX-License-Identifier: BSD-3-Clause
//! Local socket ownership and bounded peers; no application command handlers.
use crate::{
    filesystem::read_filesystem,
    protocol::{Message, Request, Response, REQUEST_LIMIT, RESPONSE_LIMIT},
    transport::Channel,
};
use std::{
    fmt,
    fs::{File, OpenOptions},
    io,
    os::{
        fd::AsRawFd,
        unix::{
            fs::{OpenOptionsExt, PermissionsExt},
            net::UnixListener,
        },
    },
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use tempfile::TempDir;

struct Peer {
    id: u64,
    pid: u32,
    channel: Channel,
    last_id: u64,
    active: Instant,
}
pub struct Incoming {
    pub peer: u64,
    pub pid: u32,
    pub request: Request,
}

#[derive(Debug)]
pub struct PollError {
    pub peer: Option<u64>,
    pub operation: &'static str,
    pub error: io::Error,
}
impl PollError {
    pub fn new(peer: Option<u64>, operation: &'static str, error: io::Error) -> Self {
        Self { peer, operation, error }
    }
}
impl fmt::Display for PollError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(peer) = self.peer {
            write!(formatter, "IPC peer {peer} {} failed: {}", self.operation, self.error)
        } else {
            write!(formatter, "IPC {} failed: {}", self.operation, self.error)
        }
    }
}
impl std::error::Error for PollError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.error)
    }
}

#[derive(Default)]
#[must_use = "IPC requests and errors must be handled"]
pub struct Poll {
    pub incoming: Vec<Incoming>,
    pub errors: Vec<PollError>,
}

struct Forwarded {
    id: u64,
    target: u64,
    origin: u64,
    original_id: u64,
    started: Instant,
}
pub struct Server {
    forwarded: Vec<Forwarded>,
    next_request: u64,
    listener: UnixListener,
    peers: Vec<Peer>,
    next_peer: u64,
    path: PathBuf,
    // Anchored TempDir cleanup must happen while the parent FD is still open.
    _directory: TempDir,
    _parent: File,
    _socket_directory: File,
}
impl Server {
    pub fn bind(parent: &Path) -> io::Result<Self> {
        if !parent.is_absolute() {
            return Err(io::Error::other("IPC parent must be absolute"));
        }
        let parent_fd = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(parent)?;
        if !read_filesystem(&parent_fd)?.is_tmpfs {
            return Err(io::Error::other("IPC requires tmpfs; no disk fallback"));
        }
        let directory = tempfile::Builder::new()
            .prefix("ipc-session-")
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir_in(format!("/proc/self/fd/{}", parent_fd.as_raw_fd()))?;
        let dir = File::open(directory.path())?;
        let path = parent
            .join(directory.path().file_name().unwrap())
            .join("control.sock");
        let listener =
            UnixListener::bind(format!("/proc/self/fd/{}/control.sock", dir.as_raw_fd()))?;
        listener.set_nonblocking(true)?;
        Ok(Self {
            forwarded: Vec::new(),
            next_request: 1,
            listener,
            peers: Vec::new(),
            next_peer: 1,
            path,
            _directory: directory,
            _parent: parent_fd,
            _socket_directory: dir,
        })
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn connected(&self, id: u64) -> bool {
        self.peers.iter().any(|p| p.id == id)
    }
    pub fn send(&mut self, id: u64, message: &Message) -> io::Result<()> {
        let peer = self
            .peers
            .iter_mut()
            .find(|p| p.id == id)
            .ok_or_else(|| io::Error::other("IPC peer gone"))?;
        peer.channel.queue(message, RESPONSE_LIMIT)
    }
    /// Forward a validated request; only the selected service may answer it.
    /// The origin receives the service's actual outcome, not an enqueue acknowledgement.
    pub fn forward(&mut self, target: u64, incoming: &Incoming) -> io::Result<()> {
        if !incoming.request.valid() || target == incoming.peer {
            return Err(io::Error::other("invalid forwarded request"));
        }
        if self.forwarded.len() >= 4 || self.forwarded.iter().any(|f| f.origin == incoming.peer) {
            return Err(io::Error::other("IPC forwarding queue is full"));
        }
        let id = self.next_request;
        self.next_request = id
            .checked_add(1)
            .ok_or_else(|| io::Error::other("IPC request IDs exhausted"))?;
        let mut request = incoming.request.clone();
        request.id = id;
        self.send(target, &Message::Request(request))?;
        self.forwarded.push(Forwarded {
            id,
            target,
            origin: incoming.peer,
            original_id: incoming.request.id,
            started: Instant::now(),
        });
        Ok(())
    }
    /// Peer read/write failures close only that connection; the owner handles diagnostics.
    pub fn poll(&mut self, service_peer: Option<u64>, service_pid: Option<u32>) -> Poll {
        let mut result = Poll::default();
        // At most eight accepts and eight requests per host tick.
        for _ in 0..8 {
            let stream = match self.listener.accept() {
                Ok((stream, _)) => stream,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => {
                    result.errors.push(PollError::new(None, "accept", error));
                    break;
                }
            };
            if self.peers.len() >= 8 {
                result.errors.push(PollError::new(None, "accept", io::Error::new(
                    io::ErrorKind::ConnectionRefused, "IPC connection limit exceeded",
                )));
                continue;
            }
            let mut cred = std::mem::MaybeUninit::<libc::ucred>::uninit();
            let mut length = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
            // SAFETY: the kernel writes into a correctly sized ucred buffer.
            if unsafe {
                libc::getsockopt(
                    stream.as_raw_fd(),
                    libc::SOL_SOCKET,
                    libc::SO_PEERCRED,
                    cred.as_mut_ptr().cast(),
                    &mut length,
                )
            } != 0 {
                result.errors.push(PollError::new(None, "credentials", io::Error::last_os_error()));
                continue;
            }
            if length as usize != std::mem::size_of::<libc::ucred>() {
                result.errors.push(PollError::new(None, "credentials", io::Error::new(
                    io::ErrorKind::InvalidData, "Invalid IPC peer credentials length",
                )));
                continue;
            }
            let cred = unsafe { cred.assume_init() };
            if cred.uid != unsafe { libc::geteuid() } || cred.pid <= 0 {
                result.errors.push(PollError::new(None, "credentials", io::Error::new(
                    io::ErrorKind::PermissionDenied, "IPC peer credentials rejected",
                )));
                continue;
            }
            if self.peers.len() >= 7 && service_pid != Some(cred.pid as u32) {
                result.errors.push(PollError::new(None, "accept", io::Error::new(
                    io::ErrorKind::ConnectionRefused, "IPC final connection slot is reserved for GUI",
                )));
                continue;
            }
            match Channel::new(
                stream,
                if service_pid == Some(cred.pid as u32) {
                    RESPONSE_LIMIT
                } else {
                    REQUEST_LIMIT
                },
            ) {
                Ok(channel) => {
                    self.peers.push(Peer {
                        id: self.next_peer,
                        pid: cred.pid as u32,
                        channel,
                        last_id: 0,
                        active: Instant::now(),
                    });
                    self.next_peer += 1;
                }
                Err(error) => result.errors.push(PollError::new(None, "configure", error)),
            }
        }
        let mut replies = Vec::new();
        self.peers.retain_mut(|peer| {
            if service_peer != Some(peer.id) && peer.active.elapsed() > Duration::from_secs(30) {
                return false;
            }
            if let Err(error) = peer.channel.flush() {
                result.errors.push(PollError::new(Some(peer.id), "write", error));
                return false;
            }
            if peer.channel.pending_output() {
                return true;
            }
            match peer.channel.receive_frame::<Message>() {
                Ok(Some((Message::Request(request), size))) => {
                    if size > REQUEST_LIMIT {
                        result.errors.push(PollError::new(
                            Some(peer.id),
                            "request",
                            io::Error::other("IPC request limit exceeded"),
                        ));
                        return false;
                    }
                    peer.active = Instant::now();
                    if request.id <= peer.last_id {
                        let response = Message::Response(Response::error(
                            request.id,
                            "duplicate_id",
                            "Request IDs must increase on each connection; no replay",
                        ));
                        return match peer.channel.queue(&response, RESPONSE_LIMIT) {
                            Ok(()) => true,
                            Err(error) => {
                                result.errors.push(PollError::new(Some(peer.id), "reply", error));
                                false
                            }
                        };
                    }
                    peer.last_id = request.id;
                    result.incoming.push(Incoming {
                        peer: peer.id,
                        pid: peer.pid,
                        request,
                    });
                    true
                }
                Ok(Some((Message::Response(response), _))) if service_peer == Some(peer.id) => {
                    replies.push((peer.id, response));
                    true
                }
                Ok(None) => true,
                Ok(Some(_)) => {
                    result.errors.push(PollError::new(Some(peer.id), "receive", io::Error::new(
                        io::ErrorKind::InvalidData, "Unexpected IPC message",
                    )));
                    false
                }
                Err(error) => {
                    // Closing an idle connection is normal; a truncated frame is not.
                    if error.kind() != io::ErrorKind::UnexpectedEof || peer.channel.has_partial_input() {
                        result.errors.push(PollError::new(Some(peer.id), "read", error));
                    }
                    false
                }
            }
        });
        for (peer, mut response) in replies {
            if let Some(index) = self
                .forwarded
                .iter()
                .position(|f| f.target == peer && f.id == response.id)
            {
                let forwarded = self.forwarded.remove(index);
                response.id = forwarded.original_id;
                if let Err(error) = self.send(forwarded.origin, &Message::Response(response)) {
                    result.errors.push(PollError::new(Some(forwarded.origin), "reply", error));
                }
            } else {
                result.errors.push(PollError::new(Some(peer), "response", io::Error::new(
                    io::ErrorKind::InvalidData, "Unsolicited IPC service response",
                )));
                self.peers.retain(|connected| connected.id != peer);
            }
        }
        for index in (0..self.forwarded.len()).rev() {
            let f = &self.forwarded[index];
            if !self.connected(f.target)
                || !self.connected(f.origin)
                || f.started.elapsed() > Duration::from_secs(4)
            {
                let f = self.forwarded.remove(index);
                if let Err(error) = self.send(
                    f.origin,
                    &Message::Response(Response::error(
                        f.original_id,
                        "delivery_unknown",
                        "Service disconnected or timed out; action may have been applied; no retry",
                    )),
                ) {
                    result.errors.push(PollError::new(Some(f.origin), "reply", error));
                }
            }
        }
        result
    }
}
