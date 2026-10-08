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
        if let Some(c) = self
            .controls
            .iter_mut()
            .find(|c| (c.kind, c.code) == (kind, code))
        {
            c.count = c.count.saturating_add(1);
            c.min = c.min.min(value);
            c.max = c.max.max(value);
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
    pub fn record(&mut self, current: [bool; LOGICAL_EDGE_COUNT]) {
        for (index, down) in current.into_iter().enumerate() {
            if down && !self.previous[index] {
                self.counts[index] = self.counts[index].saturating_add(1);
            }
        }
        self.previous = current;
    }

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
    pub fn validate(&self) -> bool {
        self.evdev_identity.as_ref().is_none_or(|d| {
            d.path.len() <= 256
                && d.name.len() <= 256
                && d.vendor.as_ref().is_none_or(|s| s.len() <= 64)
                && d.product.as_ref().is_none_or(|s| s.len() <= 64)
                && d.axes.len() <= 8
                && d.axes.iter().all(|a| a.code < 64 && a.min <= a.max)
        }) && self.channels.len() <= CHANNEL_LIMIT
            && self.channels.iter().all(|c| c.generation != 0)
            && [&self.evdev, &self.hid, &self.joydev].into_iter().all(|s| {
                s.controls.len() <= CONTROL_LIMIT
                    && s.controls
                        .iter()
                        .all(|c| matches!(c.kind, 1 | 3) && c.min <= c.max && c.count > 0)
            })
            && self
                .probe
                .as_ref()
                .is_none_or(|p| p.max_connected <= 8 && p.max_overlap <= 8)
    }
}
