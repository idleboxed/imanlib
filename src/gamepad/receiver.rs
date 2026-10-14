// SPDX-License-Identifier: BSD-3-Clause
//! One evdev owner with declarative, narrowly identified profiles. No HID writes,
//! grabs, driver changes, or dependence on RetroArch at runtime.
use super::profile::{AxisBinding, Profile};
use {
    crate::evdev::linux::Pad,
    crate::evdev::linux::Source,
    crate::input::Buttons,
    crate::input::ACTIONS,
    crate::igui::Action,
};
use {
    crate::evdev::DeviceInfo,
    crate::evdev::Event,
    crate::input_metrics::Counters,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    path::{Component, Path, PathBuf},
    time::Duration,
};

const CONTROLLER_LIMIT: usize = 8;

/// Report whether evdev metadata refers to the same physical generation as a HID path.
pub(crate) fn belongs_to_hid_generation(info: &DeviceInfo, generation: &Path) -> bool {
    let Some(sysfs) = info.identity.get("sysfs") else { return false; };
    let sysfs = Path::new(sysfs);
    // The production sources pin both canonical paths to guarded, opened FDs.
    // Compare complete path components, never VID/PID or a shared name.
    sysfs.is_absolute() && generation.is_absolute()
        && sysfs.components().all(|part| !matches!(part, Component::ParentDir | Component::CurDir))
        && generation.components().all(|part| !matches!(part, Component::ParentDir | Component::CurDir))
        && sysfs.starts_with(generation)
}

struct Controls {
    keys: BTreeMap<u16, Action>,
    bindings: [Option<AxisBinding>; 4],
    ranges: BTreeMap<u8, (i32, i32)>,
    down: BTreeSet<u16>,
    axes: BTreeMap<u8, i32>,
}
impl Controls {
    fn new(info: &DeviceInfo, profiles: &[Profile]) -> io::Result<Self> {
        info.validate().map_err(io::Error::other)?;
        let mut matching = profiles.iter().filter(|profile_item| profile_item.matches_info(info));
        let profile = matching.next()
            .ok_or_else(|| io::Error::other("unsupported receiver identity"))?;
        if matching.next().is_some() {
            return Err(io::Error::other("ambiguous evdev profiles"));
        }
        let keys = profile.key_map();
        let bindings = profile.axis_bindings();
        if !keys.keys().all(|code| info.key_codes.contains(code)) {
            return Err(io::Error::other(
                "receiver key capabilities do not match profile",
            ));
        }
        let mut ranges = BTreeMap::new();
        for (position, binding) in bindings.iter().enumerate() {
            let Some(binding) = binding else { continue; };
            let code = binding.code();
            let range = info
                .abs_info
                .get(&code)
                .ok_or_else(|| io::Error::other("receiver axis missing"))?;
            let (min, max) = (range[1], range[2]);
            if min >= max || (position >= 2 && (min, max) != (-1, 1)) {
                return Err(io::Error::other("invalid receiver axis range"));
            }
            ranges.insert(code, (min, max));
        }
        Ok(Self {
            keys,
            bindings,
            ranges,
            down: BTreeSet::new(),
            axes: BTreeMap::new(),
        })
    }
    fn event(&mut self, event: Event) -> io::Result<()> {
        match event.kind {
            1 if self.keys.contains_key(&event.code) => match event.value {
                0 => {
                    self.down.remove(&event.code);
                }
                1 | 2 => {
                    self.down.insert(event.code);
                }
                _ => return Err(io::Error::other("invalid key value")),
            },
            3 if event.code < 64 => {
                let code = event.code as u8;
                if let Some(&(min, max)) = self.ranges.get(&code) {
                    if !(min..=max).contains(&event.value) {
                        return Err(io::Error::other("axis value outside advertised range"));
                    }
                    self.axes.insert(code, event.value);
                }
            }
            _ => {}
        }
        Ok(())
    }
    fn snapshot(&mut self, pad: &mut dyn Pad) -> io::Result<()> {
        let keys = pad.keys_down()?;
        let axes = if self.ranges.is_empty() { BTreeMap::new() } else { pad.axes_state()? };
        if !self.ranges.keys().all(|code| axes.contains_key(code)) {
            return Err(io::Error::other("incomplete axis snapshot"));
        }
        self.down.clear();
        self.axes.clear();
        for code in keys {
            self.event(Event {
                kind: 1,
                code,
                value: 1,
            })?;
        }
        for (code, value) in axes {
            self.event(Event {
                kind: 3,
                code: u16::from(code),
                value,
            })?;
        }
        Ok(())
    }
    fn axis(&self, binding: Option<AxisBinding>, hat: bool) -> i32 {
        let Some(binding) = binding else { return 0; };
        let code = binding.code();
        let (min, max) = self.ranges[&code];
        let value = i64::from(self.axes[&code]);
        // Central half of a stick's advertised range is neutral; hats are exact.
        let delta = (value - i64::from(min)) * 4;
        let span = i64::from(max) - i64::from(min);
        let direction = if hat { value as i32 } else if delta < span {
            -1
        } else if delta > span * 3 {
            1
        } else {
            0
        };
        if binding.invert() { -direction } else { direction }
    }
    fn buttons(&self) -> Buttons {
        let mut result = Buttons::default();
        for code in &self.down {
            if let Some(&action) = self.keys.get(code) {
                result.set(action, true);
            }
        }
        for (stick, hat, negative, positive) in [
            (
                self.bindings[0],
                self.bindings[2],
                Action::Left,
                Action::Right,
            ),
            (
                self.bindings[1],
                self.bindings[3],
                Action::Up,
                Action::Down,
            ),
        ] {
            let value = if self.axis(hat, true) != 0 {
                self.axis(hat, true)
            } else {
                self.axis(stick, false)
            };
            if value < 0 {
                result.set(negative, true);
            }
            if value > 0 {
                result.set(positive, true);
            }
            // Conflicting digital/analog directions must not choose an arbitrary one.
            if result.down(negative) && result.down(positive) {
                result.set(negative, false);
                result.set(positive, false);
            }
        }
        result
    }
    fn neutral(&self) -> bool {
        // Conflicting directions can cancel in buttons(), but held controls are
        // never a neutral baseline for acquiring or resuming menu input.
        self.down.is_empty()
            && self.bindings.iter().enumerate().all(|(position, &binding)| {
                self.axis(binding, position >= 2) == 0
            })
    }
}

struct Controller {
    pad: Box<dyn Pad>,
    controls: Controls,
    snapshot_needed: bool,
    dropped: bool,
    complete: bool,
    waiting: bool,
    current: Buttons,
    pressed: Buttons,
    packet_press: bool,
    fresh_press: bool,
}
impl Controller {
    fn new(pad: Box<dyn Pad>, profiles: &[Profile]) -> io::Result<Self> {
        let controls = Controls::new(pad.info(), profiles)?;
        Ok(Self {
            pad,
            controls,
            snapshot_needed: true,
            dropped: false,
            complete: true,
            waiting: true,
            current: Buttons::default(),
            pressed: Buttons::default(),
            packet_press: false,
            fresh_press: false,
        })
    }
    fn inhibit(&mut self) {
        self.waiting = true;
        self.pressed = Buttons::default();
        self.packet_press = false;
        self.fresh_press = false;
    }
    fn commit(&mut self) {
        let buttons = self.controls.buttons();
        if !self.waiting {
            for action in ACTIONS {
                if buttons.down(action) && !self.current.down(action) {
                    self.pressed.set(action, true);
                }
            }
        }
        self.current = buttons;
        self.fresh_press |= self.packet_press;
        self.packet_press = false;
    }
    fn poll(&mut self, mut diagnostics: Option<&mut Counters>) -> io::Result<Buttons> {
        self.fresh_press = false;
        // A busy or incomplete queue can never publish a partial action packet.
        for _ in 0..256 {
            match self.pad.read() {
                Ok(Some(event)) => {
                    if let Some(diagnostics_item) = diagnostics.as_deref_mut() {
                        diagnostics_item.event(event.kind, event.code, event.value);
                    }
                    if event.kind == 0 && event.code == 3 {
                        // SYN_DROPPED
                        self.inhibit();
                        self.snapshot_needed = true;
                        self.dropped = true;
                        self.complete = false;
                    } else if event.kind == 0 && event.code == 0 {
                        // SYN_REPORT
                        self.complete = true;
                        self.dropped = false;
                        if !self.snapshot_needed {
                            self.commit();
                        }
                    } else {
                        self.complete = false;
                        if !self.snapshot_needed {
                            if !self.waiting && event.kind == 1 && event.value == 1
                                && self.controls.keys.contains_key(&event.code)
                                && !self.controls.down.contains(&event.code)
                            {
                                self.packet_press = true;
                            }
                            self.controls.event(event)?;
                        }
                    }
                }
                Ok(None) => {
                    if self.dropped || !self.complete {
                        self.inhibit();
                        return Ok(Buttons::default());
                    }
                    if self.snapshot_needed {
                        self.controls.snapshot(self.pad.as_mut())?;
                        self.current = self.controls.buttons();
                        self.snapshot_needed = false;
                        // Even a neutral snapshot is not permission to replay the old queue.
                        return Ok(Buttons::default());
                    }
                    if self.waiting {
                        if self.controls.neutral() {
                            self.waiting = false;
                        }
                        self.pressed = Buttons::default();
                        return Ok(Buttons::default());
                    }
                    let mut sample = self.current.union(self.pressed);
                    for (axis_item, opposite_action) in [(Action::Left, Action::Right), (Action::Up, Action::Down)] {
                        if sample.down(axis_item) && sample.down(opposite_action) {
                            // Coalesced taps must not reintroduce opposing directions.
                            sample.set(axis_item, self.current.down(axis_item));
                            sample.set(opposite_action, self.current.down(opposite_action));
                        }
                    }
                    self.pressed = Buttons::default();
                    return Ok(sample);
                }
                Err(error_item) if error_item.kind() == io::ErrorKind::Interrupted => continue,
                Err(error_item) => return Err(error_item),
            }
        }
        if let Some(diagnostics_item) = diagnostics {
            diagnostics_item.poll_limits += 1;
        }
        self.inhibit();
        Ok(Buttons::default())
    }
}

/// Observes a bounded set of compatible nodes. A fresh profiled key press
/// selects or transfers one owner, consuming that poll and initial holds.
/// Repeats and axis motion cannot transfer ownership.
/// Constructing this object never opens a device.
pub struct Receiver<S: Source> {
    source: S,
    profiles: Vec<Profile>,
    controllers: BTreeMap<PathBuf, Controller>,
    owner: Option<PathBuf>,
    fresh_request: bool,
    next_scan: Duration,
    errors: BTreeSet<String>,
    diagnostics: Option<Counters>,
}
impl<S: Source> Receiver<S> {
    /// Create a receiver with its source and profiles; device acquisition happens during polling.
    pub fn with_profiles(source: S, profiles: Vec<Profile>) -> Self {
        Self {
            source,
            profiles,
            controllers: BTreeMap::new(),
            owner: None,
            fresh_request: false,
            next_scan: Duration::ZERO,
            errors: BTreeSet::new(),
            diagnostics: None,
        }
    }
    /// Start collecting bounded receiver counters.
    pub fn enable_diagnostics(&mut self) {
        self.diagnostics = Some(Counters::default());
    }
    /// Copy receiver counters, returning empty counters when diagnostics are disabled.
    pub fn diagnostics(&self) -> Counters {
        self.diagnostics.clone().unwrap_or_default()
    }
    /// Return bounded diagnostic identity for the current evdev input owner.
    pub fn diagnostic_identity(&self) -> Option<crate::input_metrics::Identity> {
        let info = self.owner_info()?;
        Some(crate::input_metrics::Identity {
            path: info.path.clone(),
            name: info.name.clone(),
            vendor: info
                .identity
                .get("id/vendor")
                .filter(|text| text.len() <= 64)
                .cloned(),
            product: info
                .identity
                .get("id/product")
                .filter(|text| text.len() <= 64)
                .cloned(),
            axes: info
                .abs_info
                .iter()
                .take(8)
                .map(|(&code, axis_item)| crate::input_metrics::AxisRange {
                    code,
                    min: axis_item[1],
                    max: axis_item[2],
                })
                .collect(),
            axes_omitted: info.abs_info.len().saturating_sub(8),
        })
    }
    /// An open evdev backend, including candidates not yet owning the menu.
    /// Opening a node does not establish radio activity or select an identity.
    pub fn connected(&self) -> bool {
        !self.controllers.is_empty()
    }
    /// Return metadata for the controller currently owning menu input.
    pub(crate) fn owner_info(&self) -> Option<&DeviceInfo> {
        Some(self.controllers.get(self.owner.as_ref()?)?.pad.info())
    }
    /// Report whether the latest poll observed a new request for input ownership.
    pub(crate) fn fresh_request(&self) -> bool {
        self.fresh_request
    }
    /// Suppress the current owner's held input until a new neutral baseline is observed.
    pub(crate) fn inhibit_owner(&mut self) {
        if let Some(controller) = self.owner.as_ref().and_then(|path| self.controllers.get_mut(path)) {
            controller.inhibit();
        }
    }
    fn error(&mut self, message: String) {
        if let Some(diagnostics_item) = &mut self.diagnostics {
            diagnostics_item.errors += 1;
        }
        if self.errors.len() < 32 && self.errors.insert(message.clone()) {
            eprintln!("IGUI receiver: {message}");
        }
    }
    /// Discover and poll permitted devices, returning buttons from a single input owner.
    pub fn poll(&mut self, now: Duration) -> Buttons {
        self.poll_with_exclusions(now, &[])
    }
    /// Poll evdev while blocking device aliases of the supplied HID generations.
    pub(crate) fn poll_with_exclusions(&mut self, now: Duration, hid_generations: &[PathBuf]) -> Buttons {
        self.fresh_request = false;
        self.controllers.retain(|_, controller| {
            !hid_generations.iter().any(|generation| belongs_to_hid_generation(controller.pad.info(), generation))
        });
        if self.owner.as_ref().is_some_and(|path| !self.controllers.contains_key(path)) {
            self.owner = None;
        }
        if now >= self.next_scan {
            self.next_scan = now.saturating_add(Duration::from_secs(2));
            match self.source.discover() {
                Err(error_item) => self.error(format!("discovery: {error_item}")),
                Ok(mut paths) => {
                    paths.sort();
                    paths.dedup();
                    for path in paths.into_iter().take(64) {
                        if self.controllers.len() == CONTROLLER_LIMIT {
                            break;
                        }
                        if self.controllers.contains_key(&path) {
                            continue;
                        }
                        match self
                            .source
                            .open(&path)
                            .and_then(|pad| {
                                if hid_generations.iter().any(|generation| belongs_to_hid_generation(pad.info(), generation)) {
                                    return Ok(None);
                                }
                                Controller::new(pad, &self.profiles).map(Some)
                            })
                        {
                            Ok(Some(controller)) => {
                                if let Some(diagnostics_item) = &mut self.diagnostics {
                                    diagnostics_item.connections += 1;
                                }
                                eprintln!(
                                    "IGUI receiver: {} ({:?}:{:?}, awaiting fresh key press)",
                                    path.display(),
                                    controller.pad.info().identity.get("id/vendor"),
                                    controller.pad.info().identity.get("id/product")
                                );
                                self.controllers.insert(path, controller);
                            }
                            Ok(None) => {}
                            Err(error_item) => self.error(format!("{}: {error_item}", path.display())),
                        }
                    }
                }
            }
        }
        let mut failed = Vec::new();
        let mut selected = None;
        let mut owner_buttons = Buttons::default();
        for (path, controller) in &mut self.controllers {
            match controller.poll(self.diagnostics.as_mut()) {
                Ok(buttons) => {
                    if self.owner.as_ref() == Some(path) {
                        owner_buttons = buttons;
                    }
                    if selected.is_none() && controller.fresh_press && buttons.any() {
                        selected = Some(path.clone());
                    }
                }
                Err(error_item) => failed.push((path.clone(), error_item)),
            }
        }
        for (path, error_item) in failed {
            if let Some(diagnostics_item) = &mut self.diagnostics {
                diagnostics_item.disconnects += 1;
            }
            self.controllers.remove(&path);
            if self.owner.as_ref() == Some(&path) {
                self.owner = None;
                owner_buttons = Buttons::default();
            }
            self.next_scan = now.saturating_add(Duration::from_secs(2));
            self.error(format!("{} disconnected: {error_item}", path.display()));
        }
        if let Some(path) = selected {
            self.fresh_request = true;
            if self.owner.as_ref() != Some(&path) {
                // Selection is not a menu action. Discard the complete selecting
                // poll and require release before any new action from this owner.
                self.inhibit_owner();
                self.owner = Some(path);
                self.inhibit_owner();
                self.errors.clear();
                return Buttons::default();
            }
        }
        owner_buttons
    }
}
