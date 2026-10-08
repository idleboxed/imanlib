// SPDX-License-Identifier: BSD-3-Clause
//! Controller profiles and read-only receiver arbitration, without GUI state.
pub mod profile;
#[cfg(target_os = "linux")]
pub mod automatic;
#[cfg(target_os = "linux")]
pub mod receiver;
