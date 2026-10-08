// SPDX-License-Identifier: BSD-3-Clause
//! Declarative, read-only HID gamepads. No USB setup, writes or player assignment.
mod descriptor;
#[cfg(target_os = "linux")]
pub mod linux;
mod profile;
pub mod receiver;

pub use descriptor::{Control, Decoder, State};
pub use profile::Profile;

pub const MAX_DESCRIPTOR_BYTES: usize = 4096;
pub const MAX_REPORT_BYTES: usize = 512;

fn invalid(message: impl Into<String>) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, message.into())
}
