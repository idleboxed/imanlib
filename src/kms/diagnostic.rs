// SPDX-License-Identifier: BSD-3-Clause
//! Explicit probe operations; not enabled in the regular GUI/manager build.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operation {
    UploadOnly,
    FlipOnly,
    UploadAndFlip,
}

pub use crate::display_metrics::{EventStamp, Timings};
