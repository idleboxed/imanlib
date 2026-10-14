// SPDX-License-Identifier: BSD-3-Clause
//! Independent receiver generations × report IDs. Menu ownership is separate
//! from channel state and is not an emulator player/port assignment.
use super::{invalid, Control, Decoder, Profile, State, MAX_REPORT_BYTES};
use crate::input::{Buttons, ACTIONS};
use crate::input_metrics::Counters;
use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    path::{Path, PathBuf},
    time::Duration,
};

pub struct Info {
    pub generation: PathBuf,
    pub bus: u32,
    pub vendor: u16,
    pub product: u16,
    pub name: String,
    pub descriptor: Vec<u8>,
}
pub trait Device {
    /// Borrow the checked identity and descriptor metadata of this open HID device.
    fn info(&self) -> &Info;
    /// One complete input packet; None means EAGAIN, not disconnection.
    fn read(&mut self, packet: &mut [u8]) -> io::Result<Option<usize>>;
}
pub trait Source {
    /// List profile-matching HID device paths without choosing an input owner.
    fn discover(&mut self) -> io::Result<Vec<PathBuf>>;
    /// Open and validate one read-only HID device; the returned object owns its resources.
    fn open(&mut self, path: &Path) -> io::Result<Box<dyn Device>>;
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ChannelId {
    pub generation: u64,
    pub report_id: u8,
}
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub id: ChannelId,
    pub receiver: PathBuf,
    pub received: bool,
    pub ready: bool,
    pub state: State,
}
#[derive(Default)]
struct Channel {
    state: State,
    received: bool,
    ready: bool,
    pressed: Buttons,
    sample: Buttons,
    fresh_press: bool,
    blocked: bool,
}
impl Channel {
    fn inhibit(&mut self) {
        self.ready = false;
        self.pressed = Buttons::default();
        self.sample = Buttons::default();
        self.fresh_press = false;
    }
}
struct Receiver {
    device: Box<dyn Device>,
    decoder: Decoder,
    generation: u64,
    channels: BTreeMap<u8, Channel>,
}
impl Receiver {
    fn new(device: Box<dyn Device>, profile: &Profile, generation: u64) -> io::Result<Self> {
        let info = device.info();
        if !profile.matches(info.bus, info.vendor, info.product, &info.name) {
            return Err(invalid("HID receiver identity mismatch"));
        }
        let decoder = Decoder::new(profile, &info.descriptor)?;
        let channels = decoder
            .report_ids()
            .map(|id| (id, Channel::default()))
            .collect();
        Ok(Self {
            device,
            decoder,
            generation,
            channels,
        })
    }
    fn poll(&mut self, mut diagnostics: Option<&mut Counters>) -> io::Result<()> {
        for channel in self.channels.values_mut() {
            channel.sample = Buttons::default();
            channel.pressed = Buttons::default();
            channel.fresh_press = false;
        }
        let mut buffer = [0u8; MAX_REPORT_BYTES + 1];
        for _ in 0..256 {
            match self.device.read(&mut buffer) {
                Ok(Some(bytes_read)) => {
                    if let Some(diagnostics_item) = diagnostics.as_deref_mut() {
                        diagnostics_item.events += 1;
                    }
                    let packet = buffer
                        .get(..bytes_read)
                        .ok_or_else(|| invalid("oversized HID packet"))?;
                    let (id, state) = self.decoder.decode(packet)?;
                    let channel = self
                        .channels
                        .get_mut(&id)
                        .expect("decoder channel set is fixed");
                    if channel.ready && !channel.blocked {
                        let previous = channel.state.menu_buttons();
                        let next = state.menu_buttons();
                        for action in ACTIONS {
                            if next.down(action) && !previous.down(action) {
                                channel.pressed.set(action, true);
                            }
                        }
                        // Only profiled menu buttons can request ownership.
                        // Periodic HID reports and axis/hat edges cannot do so.
                        channel.fresh_press |= [
                            Control::West, Control::East, Control::North,
                            Control::South, Control::Back, Control::Start,
                        ].into_iter().any(|control| {
                            state.value(control) != 0 && channel.state.value(control) == 0
                        });
                    }
                    channel.received = true;
                    channel.state = state;
                }
                Ok(None) => {
                    for channel in self.channels.values_mut() {
                        if !channel.ready {
                            // Never replay the initial or over-budget queue. Only a
                            // drained, observed neutral state enables later actions.
                            channel.ready = !channel.blocked && channel.received && channel.state.neutral();
                        } else {
                            let current = channel.state.menu_buttons();
                            channel.sample = current.union(channel.pressed);
                            for (first_action, opposite_action) in [
                                (crate::igui::Action::Left, crate::igui::Action::Right),
                                (crate::igui::Action::Up, crate::igui::Action::Down),
                            ] {
                                if channel.sample.down(first_action) && channel.sample.down(opposite_action) {
                                    channel.sample.set(first_action, current.down(first_action));
                                    channel.sample.set(opposite_action, current.down(opposite_action));
                                }
                            }
                        }
                    }
                    return Ok(());
                }
                Err(error_item) if error_item.kind() == io::ErrorKind::Interrupted => continue,
                Err(error_item) => return Err(error_item),
            }
        }
        if let Some(diagnostics_item) = diagnostics {
            diagnostics_item.poll_limits += 1;
        }
        for channel in self.channels.values_mut() {
            channel.inhibit();
        }
        Ok(())
    }
}

/// Pure construction. Only poll discovers/opens devices, read-only through Source.
pub struct Receivers<S: Source> {
    source: S,
    profiles: Vec<Profile>,
    receivers: BTreeMap<PathBuf, Receiver>,
    owner: Option<ChannelId>,
    fresh_request: bool,
    next_generation: u64,
    next_scan: Duration,
    errors: BTreeSet<String>,
    diagnostics: Option<Counters>,
}
impl<S: Source> Receivers<S> {
    /// Create a receiver for one profile without discovering devices.
    pub fn new(source: S, profile: Profile) -> Self {
        Self::with_profiles(source, vec![profile])
    }
    /// Create a receiver for supplied profiles without acquiring devices.
    pub fn with_profiles(source: S, profiles: Vec<Profile>) -> Self {
        Self {
            source,
            profiles,
            receivers: BTreeMap::new(),
            owner: None,
            fresh_request: false,
            next_generation: 1,
            next_scan: Duration::ZERO,
            errors: BTreeSet::new(),
            diagnostics: None,
        }
    }
    /// Start collecting bounded HID receiver counters.
    pub fn enable_diagnostics(&mut self) {
        self.diagnostics = Some(Counters::default());
    }
    /// Copy counters, returning empty counters when diagnostics are disabled.
    pub fn diagnostics(&self) -> Counters {
        self.diagnostics.clone().unwrap_or_default()
    }
    /// Report whether at least one HID receiver is currently open.
    pub fn connected(&self) -> bool {
        !self.receivers.is_empty()
    }
    /// Return the HID channel currently owning menu input, if any.
    pub fn owner(&self) -> Option<ChannelId> {
        self.owner
    }
    /// Report whether the latest poll observed a new ownership request.
    pub(crate) fn fresh_request(&self) -> bool {
        self.fresh_request
    }
    /// Suppress the current owner's held input until a new neutral baseline is observed.
    pub(crate) fn inhibit_owner(&mut self) {
        if let Some(owner) = self.owner {
            if let Some(receiver) = self.receivers.values_mut().find(|receiver| receiver.generation == owner.generation) {
                receiver.channels.get_mut(&owner.report_id).expect("owner channel exists").inhibit();
            }
        }
    }
    /// Return paths identifying the HID device generations currently held.
    pub(crate) fn generations(&self) -> impl Iterator<Item = &Path> {
        self.receivers.values().map(|receiver| receiver.device.info().generation.as_path())
    }
    /// Copy the current bounded control states for diagnostic consumers.
    pub fn snapshots(&self) -> Vec<Snapshot> {
        self.receivers
            .iter()
            .flat_map(|(path, receiver)| {
                receiver
                    .channels
                    .iter()
                    .map(move |(&report_id, channel)| Snapshot {
                        id: ChannelId {
                            generation: receiver.generation,
                            report_id,
                        },
                        receiver: path.clone(),
                        received: channel.received,
                        ready: channel.ready,
                        state: channel.state,
                    })
            })
            .collect()
    }
    fn error(&mut self, message: String) {
        if let Some(diagnostics_item) = &mut self.diagnostics {
            diagnostics_item.errors += 1;
        }
        if self.errors.len() < 32 && self.errors.insert(message.clone()) {
            eprintln!("IGUI HID: {message}");
        }
    }
    /// Discover and read HID devices, returning buttons from one selected channel.
    pub fn poll(&mut self, now: Duration) -> Buttons {
        self.poll_with_blocked(now, |_| false)
    }
    /// Poll HID receivers while excluding channels whose metadata matches the supplied predicate.
    pub(crate) fn poll_with_blocked(&mut self, now: Duration, blocked: impl Fn(&Info) -> bool) -> Buttons {
        self.fresh_request = false;
        if now >= self.next_scan {
            self.next_scan = now.saturating_add(Duration::from_secs(2));
            match self.source.discover() {
                Err(error_item) => self.error(format!("discovery: {error_item}")),
                Ok(mut paths) => {
                    paths.sort();
                    paths.dedup();
                    for path in paths.into_iter().take(256) {
                        if self.receivers.len() == 8 {
                            break;
                        }
                        if self.receivers.contains_key(&path) {
                            continue;
                        }
                        let result = self.source.open(&path).and_then(|device| {
                            if self
                                .receivers
                                .values()
                                .any(|receiver_item| receiver_item.device.info().generation == device.info().generation)
                            {
                                return Err(invalid("duplicate HID receiver generation"));
                            }
                            let info = device.info();
                            let mut matching = self.profiles.iter().filter(|profile_item| {
                                profile_item.matches(info.bus, info.vendor, info.product, &info.name)
                            });
                            let profile = matching.next().ok_or_else(|| invalid("no matching HID profile"))?;
                            if matching.next().is_some() {
                                return Err(invalid("ambiguous HID profiles"));
                            }
                            Receiver::new(device, profile, self.next_generation)
                        });
                        match result {
                            Ok(receiver) => {
                                if let Some(diagnostics_item) = &mut self.diagnostics {
                                    diagnostics_item.connections += 1;
                                }
                                self.next_generation += 1;
                                eprintln!(
                                    "IGUI HID: {} (generation {}, channels {:?})",
                                    path.display(),
                                    receiver.generation,
                                    receiver.channels.keys().collect::<Vec<_>>()
                                );
                                self.receivers.insert(path, receiver);
                            }
                            Err(error_item) => self.error(format!("{}: {error_item}", path.display())),
                        }
                    }
                }
            }
        }
        let mut failed = Vec::new();
        for (path, receiver) in &mut self.receivers {
            let exclude = blocked(receiver.device.info());
            for channel in receiver.channels.values_mut() {
                if channel.blocked != exclude {
                    // Both blocking and unblocking invalidate old queued input.
                    channel.inhibit();
                    channel.blocked = exclude;
                }
            }
            if let Err(error_item) = receiver.poll(self.diagnostics.as_mut()) {
                failed.push((path.clone(), error_item));
            }
        }
        for (path, error_item) in failed {
            if let Some(diagnostics_item) = &mut self.diagnostics {
                diagnostics_item.disconnects += 1;
            }
            self.receivers.remove(&path);
            self.error(format!("{} disconnected: {error_item}", path.display()));
        }
        if let Some(owner) = self.owner {
            if let Some(receiver) = self
                .receivers
                .values()
                .find(|receiver_item| receiver_item.generation == owner.generation)
            {
                if receiver.channels[&owner.report_id].blocked {
                    self.owner = None;
                }
            } else {
                self.owner = None;
            }
        }
        // Node order then Report ID is the deterministic tie-break, never OR.
        let selected = self.receivers.values().find_map(|receiver| {
            receiver.channels.iter().find(|(_, channel)| {
                !channel.blocked && channel.fresh_press && channel.sample.any()
            }).map(|(&report_id, _)| ChannelId { generation: receiver.generation, report_id })
        });
        if let Some(selected) = selected {
            self.fresh_request = true;
            if self.owner != Some(selected) {
                self.inhibit_owner();
                self.owner = Some(selected);
                self.inhibit_owner();
                return Buttons::default();
            }
        }
        self.owner.and_then(|owner| {
            self.receivers.values().find(|receiver| receiver.generation == owner.generation)
                .map(|receiver| receiver.channels[&owner.report_id].sample)
        }).unwrap_or_default()
    }
}
