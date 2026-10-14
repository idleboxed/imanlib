// SPDX-License-Identifier: BSD-3-Clause
//! Bounded evdev diagnostics. Stream numbers are NOT player numbers.
use {
    crate::evdev::find_retroarch_button_index,
    crate::evdev::DeviceInfo,
    crate::evdev::Event,
};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

mod guide;
#[cfg(target_os = "linux")]
pub mod linux;
mod render;
pub use guide::{Guide, Key, Stage, ACTIONS};
pub use render::draw;

pub const MAX_STREAMS: usize = 8;
pub const MAX_CONNECTIONS: u32 = 32;
pub const LOG_BYTES: usize = 40 * 1024;
/// Session safety limit only. Individual prompts never advance on a timer.
pub const END_MS: u64 = 540_000;

#[derive(Clone, Copy, Debug)]
pub struct RawObservation {
    pub generation: u32,
    pub event: Event,
    pub ms: u64,
    pub retroarch_button: Option<usize>,
}

#[derive(Debug, Serialize)]
pub struct ControlStats {
    pub kind: u16,
    pub code: u16,
    pub count: u64,
    pub min: i32,
    pub max: i32,
    pub first: i32,
    pub last: i32,
    pub first_ms: u64,
    pub last_ms: u64,
}
#[derive(Default, Debug, Serialize)]
pub struct Evidence {
    pub events: u64,
    pub invalid_events: u64,
    pub controls_omitted: u64,
    pub controls: BTreeMap<u32, ControlStats>,
}
pub struct Stream {
    pub generation: u32,
    pub info: DeviceInfo,
    pub events: u64,
    pub resyncs: u64,
    pub last: Option<Event>,
    pub evidence: Evidence,
    ready: bool,
    down: BTreeSet<u16>,
    presses: BTreeMap<u16, u64>,
    taps: Vec<Key>,
}
impl Stream {
    /// Return the currently held raw key codes for this stream.
    pub fn buttons_down(&self) -> Vec<u16> {
        self.down.iter().copied().collect()
    }
    fn clear_actions(&mut self) {
        self.presses.clear();
        self.taps.clear();
    }
}
pub struct Probe {
    pub streams: Vec<Option<Stream>>,
    pub stage: Stage,
    pub guide: Guide,
    pub connections: u32,
    pub max_connected: usize,
    pub max_overlap: usize,
    pub overlap_generations: Vec<u32>,
    pub incomplete_polls: u64,
    pub sample_complete: bool,
    pub recent: VecDeque<RawObservation>,
    capture_recent: bool,
    finished_at: Option<u64>,
    records: Vec<Value>,
    diagnostics: crate::input_metrics::Counters,
}
impl Default for Probe {
    fn default() -> Self {
        Self {
            streams: (0..MAX_STREAMS).map(|_| None).collect(),
            stage: Stage::Prepare,
            guide: Guide::default(),
            connections: 0,
            max_connected: 0,
            max_overlap: 0,
            overlap_generations: Vec::new(),
            incomplete_polls: 0,
            sample_complete: false,
            recent: VecDeque::new(),
            capture_recent: false,
            finished_at: None,
            diagnostics: Default::default(),
            records: vec![
                json!({"event":"probe_start","format":"IGUI-INPUT-2","interface":"evdev","session_limit_ms":END_MS,"manual_steps":true,"streams_are_not_players":true,"mapping_written":false}),
            ],
        }
    }
}
impl Probe {
    /// Interactive menu mode has its own guide; the standalone contract stays unchanged.
    pub fn input_test() -> Self {
        let mut probe = Self::default();
        probe.capture_recent = true;
        probe.records.clear();
        probe
    }
    /// Register checked device metadata and return an available bounded stream slot.
    pub fn connect(&mut self, info: DeviceInfo, ms: u64) -> Result<usize, &'static str> {
        info.validate()?;
        if self.connections >= MAX_CONNECTIONS {
            return Err("diagnostic connection limit reached");
        }
        if self
            .streams
            .iter()
            .flatten()
            .any(|stream| stream.info.path == info.path)
        {
            return Err("duplicate diagnostic device path");
        }
        let slot = self
            .streams
            .iter()
            .position(Option::is_none)
            .ok_or("diagnostic stream limit reached")?;
        self.connections += 1;
        self.diagnostics.connections += 1;
        self.records.push(
            json!({"event":"device","ms":ms,"slot":slot,"generation":self.connections,"info":info}),
        );
        self.streams[slot] = Some(Stream {
            generation: self.connections,
            info,
            events: 0,
            resyncs: 0,
            last: None,
            evidence: Evidence::default(),
            ready: false,
            down: BTreeSet::new(),
            presses: BTreeMap::new(),
            taps: Vec::new(),
        });
        self.max_connected = self
            .max_connected
            .max(self.streams.iter().flatten().count());
        Ok(slot)
    }
    fn evidence(&mut self, slot: usize, ms: u64) {
        if let Some(stream) = &mut self.streams[slot] {
            if stream.evidence.events != 0 || stream.evidence.invalid_events != 0 {
                self.records.push(json!({"event":"evidence","stage":self.stage,"end_ms":ms,"generation":stream.generation,"data":std::mem::take(&mut stream.evidence)}));
            }
        }
    }
    fn transition(&mut self, stage: Stage, ms: u64) {
        if stage == self.stage {
            return;
        }
        for slot in 0..MAX_STREAMS {
            self.evidence(slot, ms);
        }
        for stream in self.streams.iter_mut().flatten() {
            stream.clear_actions();
        }
        self.stage = stage;
        if stage == Stage::Finish {
            self.finished_at = Some(ms);
        }
        self.records
            .push(json!({"event":"stage","ms":ms,"stage":stage}));
    }
    /// Record the stream's evidence and invalidate ownership when its device disappears.
    pub fn disconnect(&mut self, slot: usize, ms: u64, reason: &str) {
        self.evidence(slot, ms);
        if let Some(stream) = self.streams[slot].take() {
            self.diagnostics.disconnects += 1;
            self.records.push(json!({"event":"disconnect","ms":ms,"generation":stream.generation,"reason":reason.chars().take(160).collect::<String>(),"events":stream.events}));
            if self.stage == Stage::Finish {
                return;
            }
            if self.guide.disconnected(stream.generation) {
                self.transition(Stage::Prepare, ms);
            } else if let Stage::Button { player, .. } = self.stage {
                if self.guide.players[usize::from(player - 1)] == Some(stream.generation) {
                    self.guide.players[usize::from(player - 1)] = None;
                    self.transition(Stage::Player { player }, ms);
                }
            }
        }
    }
    /// Discard stale control state and record why the stream must establish a new baseline.
    pub fn resync(&mut self, slot: usize, ms: u64, reason: &str) {
        if reason == "SYN_DROPPED" {
            self.diagnostics.syn_dropped += 1;
        }
        if let Some(stream) = &mut self.streams[slot] {
            if stream.ready {
                stream.resyncs += 1;
                self.records.push(
                    json!({"event":"resync","ms":ms,"generation":stream.generation,"reason":reason}),
                );
            }
            stream.ready = false;
            stream.down.clear();
            stream.clear_actions();
        }
        self.guide.clear_candidate();
    }
    /// Validate held keys and establish the stream's starting state without inventing pressed edges.
    pub fn baseline(&mut self, slot: usize, keys: &[u16], ms: u64) -> Result<(), &'static str> {
        let stream = self.streams[slot].as_mut().ok_or("missing stream")?;
        if keys.len() > 768 || keys.iter().any(|key_item| !stream.info.key_codes.contains(key_item)) {
            return Err("invalid evdev key state");
        }
        stream.down = keys.iter().copied().collect();
        stream.clear_actions();
        stream.ready = true;
        self.records.push(json!({"event":"key_state","ms":ms,"generation":stream.generation,"keys_down":keys,"not_a_press":true}));
        Ok(())
    }
    /// Apply one supported event to a ready stream and accumulate bounded evidence.
    pub fn event(&mut self, slot: usize, event: Event, ms: u64) {
        let Some(stream) = self.streams[slot].as_mut() else {
            return;
        };
        if !stream.ready || !matches!(event.kind, 1 | 3) {
            return;
        }
        self.diagnostics.event(event.kind, event.code, event.value);
        if (event.kind == 1
            && (!stream.info.key_codes.contains(&event.code) || !matches!(event.value, 0..=2)))
            || (event.kind == 3
                && (event.code >= 64 || !stream.info.axis_codes.contains(&(event.code as u8))))
        {
            self.diagnostics.errors += 1;
            stream.evidence.invalid_events += 1;
            self.resync(slot, ms, "invalid event");
            return;
        }
        stream.last = Some(event);
        if self.capture_recent {
            if self.recent.len() == 6 {
                self.recent.pop_front();
            }
            self.recent.push_back(RawObservation {
                generation: stream.generation, event, ms,
                retroarch_button: (event.kind == 1)
                    .then(|| find_retroarch_button_index(&stream.info.key_codes, event.code)).flatten(),
            });
            self.records.push(json!({"event":"raw","ms":ms,"generation":stream.generation,
                "kind":event.kind,"code":event.code,"value":event.value,
                "fresh_press":event.kind == 1 && event.value == 1 && !stream.down.contains(&event.code),
                "retroarch_button":self.recent.back().and_then(|sample| sample.retroarch_button),
                "operator":event.kind == 1 && self.guide.operator == Some(Key {generation:stream.generation,code:event.code})}));
        }
        stream.events += 1;
        stream.evidence.events += 1;
        if event.kind == 1 {
            match event.value {
                1 if stream.down.insert(event.code) => {
                    stream.presses.insert(event.code, ms);
                }
                0 => {
                    stream.down.remove(&event.code);
                    if stream.presses.remove(&event.code).is_some() {
                        if stream.taps.len() < 16 {
                            stream.taps.push(Key {
                                generation: stream.generation,
                                code: event.code,
                            });
                        } else {
                            self.resync(slot, ms, "tap backlog");
                            return;
                        }
                    }
                }
                _ => {}
            }
        }
        let key = u32::from(event.kind) * 65536 + u32::from(event.code);
        if !stream.evidence.controls.contains_key(&key) && stream.evidence.controls.len() == 32 {
            stream.evidence.controls_omitted += 1;
            return;
        }
        let control_stats = stream.evidence.controls.entry(key).or_insert(ControlStats {
            kind: event.kind,
            code: event.code,
            count: 0,
            min: event.value,
            max: event.value,
            first: event.value,
            last: event.value,
            first_ms: ms,
            last_ms: ms,
        });
        control_stats.count += 1;
        control_stats.min = control_stats.min.min(event.value);
        control_stats.max = control_stats.max.max(event.value);
        control_stats.last = event.value;
        control_stats.last_ms = ms;
    }
    /// Report whether the selected stream has a usable baseline.
    pub fn ready(&self, slot: usize) -> bool {
        self.streams[slot].as_ref().is_some_and(|stream| stream.ready)
    }
    /// Record an exhausted read budget and require the stream to resynchronize.
    pub fn incomplete(&mut self, slot: usize, ms: u64) {
        self.incomplete_polls += 1;
        self.resync(slot, ms, "read budget exceeded");
    }
    /// Discard incomplete sample actions before the next observation.
    pub fn cancel_sample(&mut self) {
        self.sample_complete = false;
        for stream in self.streams.iter_mut().flatten() {
            stream.clear_actions();
        }
        self.guide.clear_candidate();
    }
    /// Only complete, drained samples may confirm a step or simultaneous holds.
    pub fn sampled(&mut self, ms: u64) {
        self.sample_complete = true;
        let mut taps = Vec::new();
        for stream in self.streams.iter_mut().flatten() {
            taps.append(&mut stream.taps);
        }
        if self.capture_recent {
            return;
        }
        if self.stage != Stage::Finish
            && self.guide.operator.is_some_and(|key_item| {
                self.streams.iter().flatten().any(|stream| {
                    stream.generation == key_item.generation
                        && stream.presses
                            .get(&key_item.code)
                            .is_some_and(|&start| ms.saturating_sub(start) >= 2000)
                })
            })
        {
            self.records
                .push(json!({"event":"operator_finish","ms":ms,"reason":"start_held"}));
            self.transition(Stage::Finish, ms);
            return;
        }
        if self.stage == Stage::Together {
            let held: Vec<_> = self
                .streams
                .iter()
                .flatten()
                .filter(|stream| {
                    stream.presses.keys().any(|&code| {
                        self.guide.operator
                            != Some(Key {
                                generation: stream.generation,
                                code,
                            })
                    })
                })
                .map(|stream| stream.generation)
                .collect();
            if held.len() > self.max_overlap {
                self.max_overlap = held.len();
                self.overlap_generations = held;
            }
        }
        let (next, mut records) = self.guide.sample(self.stage, &taps);
        for record in &mut records {
            record["ms"] = json!(ms);
        }
        self.records.extend(records);
        self.transition(next, ms);
    }
    /// Report whether the probe's final display interval has elapsed.
    pub fn done(&self, ms: u64) -> bool {
        self.finished_at
            .is_some_and(|start| ms.saturating_sub(start) >= 2000)
    }
    /// Drain accumulated probe records for the caller to persist or display.
    pub fn take_records(&mut self) -> Vec<Value> {
        std::mem::take(&mut self.records)
    }
    /// Copy bounded input counters and stream state into the shared report format.
    pub fn diagnostic_report(&self, headless: bool) -> crate::input_metrics::Sample {
        crate::input_metrics::Sample {
            evdev: self.diagnostics.clone(),
            headless,
            evdev_connected: self.streams.iter().any(Option::is_some),
            probe: Some(crate::input_metrics::ProbeSummary {
                max_connected: self.max_connected,
                max_overlap: self.max_overlap,
                incomplete_polls: self.incomplete_polls,
            }),
            ..Default::default()
        }
    }
    /// Record final stream evidence and return a summary without claiming physical-player verification.
    pub fn finish(&mut self, ms: u64, reason: &str) -> Value {
        for slot in 0..MAX_STREAMS {
            self.evidence(slot, ms);
        }
        json!({"event":"probe_end","ms":ms,"reason":reason,"connections":self.connections,"max_connected_streams":self.max_connected,"max_simultaneous_button_streams":self.max_overlap,"overlap_generations":self.overlap_generations,"incomplete_polls":self.incomplete_polls,"operator":self.guide.operator,"player_generations":self.guide.players,"operator_confirmed_steps":self.guide.observations,"physical_players_verified":false,"mapping_written":false})
    }
}

/// Leave room in BOX2's 64 KiB log for DRM, supervisor and the final summary.
#[derive(Default)]
pub struct LogBudget {
    pub bytes: usize,
    pub omitted: usize,
}
impl LogBudget {
    /// Encode a probe log record while enforcing the total output-byte budget.
    pub fn encode(&mut self, record: &Value) -> Option<String> {
        let line = format!("IGUI input: {record}\n");
        if self.bytes + line.len() > LOG_BYTES {
            self.omitted += 1;
            None
        } else {
            self.bytes += line.len();
            Some(line)
        }
    }
}
