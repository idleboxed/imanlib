// SPDX-License-Identifier: BSD-3-Clause
//! Owner-reported display measurements. No device access or clock-domain assumptions.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Statistics {
    pub requests: u64,
    pub unchanged: u64,
    pub full: u64,
    pub dialog: u64,
    pub left: u64,
    pub preview: u64,
    pub metadata: u64,
    pub telemetry: u64,
    pub render_us_sum: u128,
    pub render_us_max: u128,
    pub full_cost: Timing,
    pub partial_cost: Timing,
    pub dialog_cost: Timing,
    pub telemetry_only_cost: Timing,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Timing {
    pub count: u64,
    pub sum_us: u128,
    pub max_us: u128,
}
impl Timing {
    pub fn record(&mut self, us: u128) {
        self.count += 1;
        self.sum_us += us;
        self.max_us = self.max_us.max(us);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventStamp {
    pub sequence: u32,
    /// Raw DRM event timestamp in microseconds; its clock domain is not assumed.
    pub timestamp_us: u64,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct Timings {
    pub render_us: Option<u64>,
    pub map_us: Option<u64>,
    pub copy_us: Option<u64>,
    pub unmap_us: Option<u64>,
    pub submit_us: Option<u64>,
    pub wait_us: Option<u64>,
    pub event: Option<EventStamp>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Metric {
    pub count: u64,
    pub min: u64,
    pub max: u64,
    pub sum: u64,
}
impl Metric {
    pub fn record(&mut self, value: Option<u64>) {
        if let Some(value) = value {
            self.min = if self.count == 0 {
                value
            } else {
                self.min.min(value)
            };
            self.max = self.max.max(value);
            self.sum = self.sum.saturating_add(value);
            self.count += 1;
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Summary {
    pub steps: u64,
    pub metrics: [Metric; 6],
    pub first: Option<EventStamp>,
    pub last: Option<EventStamp>,
    pub sequence_delta: Metric,
    pub timestamp_delta_us: Metric,
    pub timestamp_regressions: u64,
}
impl Summary {
    pub fn record(&mut self, t: Timings) {
        self.steps += 1;
        for (metric, value) in self.metrics.iter_mut().zip([
            t.render_us,
            t.map_us,
            t.copy_us,
            t.unmap_us,
            t.submit_us,
            t.wait_us,
        ]) {
            metric.record(value);
        }
        if let Some(event) = t.event {
            if let Some(last) = self.last {
                self.sequence_delta
                    .record(Some(u64::from(event.sequence.wrapping_sub(last.sequence))));
                let delta = event.timestamp_us.checked_sub(last.timestamp_us);
                self.timestamp_regressions += u64::from(delta.is_none());
                self.timestamp_delta_us.record(delta);
            }
            self.first.get_or_insert(event);
            self.last = Some(event);
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Backend {
    Headless,
    Desktop,
    Drm,
    KmsProbe,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct KmsState {
    pub connector: u32,
    pub encoder: u32,
    pub crtc: u32,
    pub width: u16,
    pub height: u16,
    pub refresh_hz: u32,
    pub completed_flips: u64,
    pub master_acquired: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Sample {
    pub backend: Backend,
    pub render: Option<Statistics>,
    pub kms: Option<KmsState>,
    /// Ordinary KMS present/check cost excludes the UI's 33 ms rate-limit sleep.
    pub present: Option<Timing>,
    pub idle_check: Option<Timing>,
    /// Only the explicitly launched five-phase probe supplies map/copy/event details.
    pub probe: Option<Summary>,
    pub cleanup_ok: Option<bool>,
    pub operation_ok: Option<bool>,
}
impl Sample {
    pub fn validate(&self) -> bool {
        self.kms.as_ref().is_none_or(|k| {
            k.connector != 0 && k.encoder != 0 && k.crtc != 0 && k.width != 0 && k.height != 0
        }) && match self.backend {
            Backend::Drm => self.kms.is_some() && self.probe.is_none(),
            Backend::KmsProbe => self.probe.is_some() && self.render.is_none(),
            Backend::Headless | Backend::Desktop => self.kms.is_none() && self.probe.is_none(),
        }
    }
}
