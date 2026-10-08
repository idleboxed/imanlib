// SPDX-License-Identifier: BSD-3-Clause
//! Local RPC and shared bounded metric readers: no launcher, observer or SD writer.
pub mod client;
pub mod display_metrics;
pub mod evdev;
pub mod filesystem;
pub mod igui;
pub mod igui_iman_stub;
pub mod input_metrics;
pub mod metrics;
pub mod protocol;
pub mod server;
pub mod transport;
