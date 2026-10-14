// SPDX-License-Identifier: BSD-3-Clause
//! Passive owner-side input counters, never a second input reader.
use serde::{Deserialize, Serialize};

pub const CONTROL_LIMIT: usize = 8;
pub const CHANNEL_LIMIT: usize = 16;
pub const LOGICAL_EDGE_COUNT: usize = 10;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ControlRange {
    pub kind: u16,
    pub code: u16,
    pub count: u64,
    pub min: i32,
    pub max: i32,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Counters {
    pub connections: u64,
    pub disconnects: u64,
    pub errors: u64,
    pub events: u64,
    pub syn_dropped: u64,
    pub poll_limits: u64,
    pub controls_omitted: u64,
    pub controls: Vec<ControlRange>,
}
impl Counters {
    /// Observed raw values, including those subsequently rejected by the decoder.
    pub fn event(&mut self, kind: u16, code: u16, value: i32) {
        self.events = self.events.saturating_add(1);
        if kind == 0 && code == 3 {
            self.syn_dropped = self.syn_dropped.saturating_add(1);
        }
        if !matches!(kind, 1 | 3) {
            return;
        }
        if let Some(control) = self
            .controls
            .iter_mut()
            .find(|control| (control.kind, control.code) == (kind, code))
        {
            control.count = control.count.saturating_add(1);
            control.min = control.min.min(value);
            control.max = control.max.max(value);
        } else if self.controls.len() < CONTROL_LIMIT {
            self.controls.push(ControlRange {
                kind,
                code,
                count: 1,
                min: value,
                max: value,
            });
        } else {
            self.controls_omitted = self.controls_omitted.saturating_add(1);
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Channel {
    pub generation: u64,
    pub report_id: u8,
    pub received: bool,
    pub ready: bool,
    pub owner: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ProbeSummary {
    pub max_connected: usize,
    pub max_overlap: usize,
    pub incomplete_polls: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AxisRange {
    pub code: u8,
    pub min: i32,
    pub max: i32,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Identity {
    pub path: String,
    pub name: String,
    pub vendor: Option<String>,
    pub product: Option<String>,
    pub axes: Vec<AxisRange>,
    pub axes_omitted: usize,
}

/// Sampled rising edges in order: up, down, left, right, Y, X, A, B, Select, Start.
pub type LogicalEdges = [u64; LOGICAL_EDGE_COUNT];

/// A live owner's sampled states.
#[derive(Default)]
pub struct LogicalEdgeSampler {
    previous: [bool; LOGICAL_EDGE_COUNT],
    counts: [u64; LOGICAL_EDGE_COUNT],
}
impl LogicalEdgeSampler {
    /// Count new pressed edges without counting held buttons repeatedly.
    pub fn record(&mut self, current: [bool; LOGICAL_EDGE_COUNT]) {
        for (index, down) in current.into_iter().enumerate() {
            if down && !self.previous[index] {
                self.counts[index] = self.counts[index].saturating_add(1);
            }
        }
        self.previous = current;
    }

    /// Copy the accumulated logical button-edge counts.
    pub fn snapshot(&self) -> LogicalEdges {
        self.counts
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Sample {
    pub evdev: Counters,
    pub hid: Counters,
    pub joydev: Counters,
    pub polls: u64,
    /// Sampled rising edges, not physical press counts or NavigationInput repeats.
    pub logical_edges: LogicalEdges,
    pub channels: Vec<Channel>,
    pub channels_omitted: usize,
    pub evdev_connected: bool,
    pub evdev_identity: Option<Identity>,
    pub joydev_connected: bool,
    pub headless: bool,
    pub probe: Option<ProbeSummary>,
}
impl Sample {
    /// Report whether device identities, capability lists and input counters fit the diagnostic contract.
    pub fn validate(&self) -> bool {
        self.evdev_identity.as_ref().is_none_or(|identity| {
            identity.path.len() <= 256
                && identity.name.len() <= 256
                && identity.vendor.as_ref().is_none_or(|sample| sample.len() <= 64)
                && identity.product.as_ref().is_none_or(|sample| sample.len() <= 64)
                && identity.axes.len() <= 8
                && identity.axes.iter().all(|axis| axis.code < 64 && axis.min <= axis.max)
        }) && self.channels.len() <= CHANNEL_LIMIT
            && self.channels.iter().all(|control| control.generation != 0)
            && [&self.evdev, &self.hid, &self.joydev].into_iter().all(|sample| {
                sample.controls.len() <= CONTROL_LIMIT
                    && sample.controls
                        .iter()
                        .all(|control| matches!(control.kind, 1 | 3) && control.min <= control.max && control.count > 0)
            })
            && self
                .probe
                .as_ref()
                .is_none_or(|probe_item| probe_item.max_connected <= 8 && probe_item.max_overlap <= 8)
    }
}
