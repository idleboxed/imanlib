// SPDX-License-Identifier: BSD-3-Clause
//! Local RPC and shared display/input primitives: no launcher or SD policy.
pub mod client;
#[cfg(feature = "input")]
mod controller_profile;
#[cfg(feature = "input")]
pub mod controls;
pub mod display_metrics;
pub mod evdev;
pub mod filesystem;
#[cfg(feature = "input")]
pub mod gamepad;
#[cfg(feature = "input")]
pub mod hid;
pub mod igui;
pub mod igui_iman_stub;
pub mod input;
pub mod input_metrics;
#[cfg(feature = "input")]
pub mod input_probe;
#[cfg(all(feature = "drm", target_os = "linux"))]
pub mod kms;
pub mod metrics;
pub mod protocol;
pub mod server;
#[cfg(feature = "graphics")]
pub mod surface;
pub mod transport;
