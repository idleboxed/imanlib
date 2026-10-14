// SPDX-License-Identifier: BSD-3-Clause
//! Partial iman IPC stub for standalone IGUI integration tests.
//! Registers the GUI and forwards control commands; not the session manager.
//! No managed bootstrap, game launch, storage policy, observation or process spawning.
use crate::{
    igui::CONTROL_COMMANDS,
    protocol::{Message, Response},
    server::{PollError, Server},
};
use serde_json::json;
use std::{io, path::Path, time::Instant};

/// Test host for IGUI without `--managed`; not a replacement for iman.
pub struct Host {
    server: Server,
    pid: Option<u32>,
    peer: Option<u64>,
    closed: bool,
}
impl Host {
    /// Create a private test endpoint with real monotonic deadlines.
    pub fn bind(parent: &Path) -> io::Result<Self> {
        Self::bind_with_clock(parent, Instant::now)
    }
    /// Create a test endpoint whose routing deadlines follow the supplied clock.
    pub fn bind_with_clock(parent: &Path, clock: impl Fn() -> Instant + Send + Sync + 'static) -> io::Result<Self> {
        Ok(Self {
            server: Server::bind_with_clock(parent, clock)?,
            pid: None,
            peer: None,
            closed: false,
        })
    }
    /// Return this test host's private socket path.
    pub fn path(&self) -> &Path {
        self.server.path()
    }
    /// The launcher explicitly authorizes one PID; an arbitrary client cannot register.
    pub fn expect_gui(&mut self, pid: u32) -> io::Result<()> {
        if self.pid.is_some() {
            return Err(io::Error::other("GUI already expected"));
        }
        self.pid = Some(pid);
        Ok(())
    }
    /// Report whether the registered GUI acknowledged closure.
    pub fn closed(&self) -> bool {
        self.closed
    }
    /// The launcher reports these errors without replaying any GUI actions.
    #[must_use = "IPC errors must be handled"]
    pub fn poll(&mut self) -> Vec<PollError> {
        let result = self.server.poll(self.peer, self.pid);
        let mut errors = result.errors;
        for incoming in result.incoming {
            let request_item = &incoming.request;
            let response = if !request_item.valid() {
                Response::error(
                    request_item.id,
                    "invalid_request",
                    "Namespaced command and object parameters required",
                )
            } else if CONTROL_COMMANDS.contains(&request_item.command.as_str()) {
                if let Some(peer) = self.peer.filter(|_| !self.closed) {
                    match self.server.forward(peer, &incoming) {
                        Ok(()) => continue,
                        Err(error_item) => Response::error(request_item.id, "unavailable", error_item),
                    }
                } else {
                    Response::error(request_item.id, "not_ready", "No active GUI registered")
                }
            } else {
                match request_item.command.as_str() {
                    "igui.attach" if self.pid == Some(incoming.pid) && self.peer.is_none() => {
                        self.peer = Some(incoming.peer);
                        Response::done(request_item.id, json!({}))
                    }
                    "igui.closed" if self.peer == Some(incoming.peer) && !self.closed => {
                        self.closed = true;
                        Response::done(request_item.id, json!({}))
                    }
                    "igui.attach" | "igui.closed" => {
                        Response::error(request_item.id, "forbidden", "Expected the registered GUI process")
                    }
                    "core.capabilities" => Response::done(
                        request_item.id,
                        json!({"commands":CONTROL_COMMANDS,"mode":"gui_only"}),
                    ),
                    _ => Response::error(request_item.id, "unknown_command", "Unknown namespaced command"),
                }
            };
            if let Err(error) = self.server.send(incoming.peer, &Message::Response(response)) {
                errors.push(PollError::new(Some(incoming.peer), "reply", error));
            }
        }
        errors
    }
}
