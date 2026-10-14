// SPDX-License-Identifier: BSD-3-Clause
//! Synthetic receiver lifecycle and packet tests with library-owned profiles.
#![cfg(feature = "input")]
#![cfg(target_os = "linux")]
mod common;
use {
    imanlib::evdev::linux::Pad,
    imanlib::evdev::linux::ProfileSource,
    imanlib::evdev::linux::Source,
    imanlib::gamepad::receiver::Receiver,
    imanlib::input::Buttons,
    imanlib::igui::Action::*,
};
use imanlib::evdev::{DeviceInfo, Event};
use std::{
    cell::RefCell,
    collections::{BTreeMap, VecDeque},
    io,
    path::{Path, PathBuf},
    rc::Rc,
    time::Duration,
};

fn info(board: bool) -> DeviceInfo {
    DeviceInfo {
        path: "/synthetic/event0".into(),
        name: "ShanWan Twin USB Joystick".into(),
        axis_codes: vec![0, 1, 2, 5, 16, 17],
        key_codes: if board {
            vec![21, 28, 30, 45, 48, 54, 103, 105, 106, 108]
        } else {
            (288..300).collect()
        },
        abs_info: BTreeMap::from([
            (0, [128, 0, 255, 0, 0, 0]),
            (1, [128, 0, 255, 0, 0, 0]),
            (16, [0, -1, 1, 0, 0, 0]),
            (17, [0, -1, 1, 0, 0, 0]),
        ]),
        identity: BTreeMap::from([
            ("id/bustype".into(), "0003".into()),
            (
                "id/vendor".into(),
                if board { "20bc" } else { "2563" }.into(),
            ),
            (
                "id/product".into(),
                if board { "5500" } else { "0555" }.into(),
            ),
        ]),
    }
}
struct State {
    info: DeviceInfo,
    queue: VecDeque<Event>,
    held: Vec<u16>,
    axes: BTreeMap<u8, i32>,
    opened: usize,
    closed: usize,
    reads: usize,
    snapshots: usize,
    scans: usize,
    failed: bool,
    denied: bool,
    noisy: bool,
    snapshot_error: bool,
}
struct Fixture(Rc<RefCell<State>>);
struct Device {
    info: DeviceInfo,
    state: Rc<RefCell<State>>,
}
impl Drop for Device {
    fn drop(&mut self) {
        self.state.borrow_mut().closed += 1;
    }
}
impl Pad for Device {
    fn info(&self) -> &DeviceInfo {
        &self.info
    }
    fn read(&mut self) -> io::Result<Option<Event>> {
        let mut state_item = self.state.borrow_mut();
        state_item.reads += 1;
        if state_item.failed {
            return Err(io::Error::other("synthetic disconnect"));
        }
        if state_item.noisy {
            return Ok(Some(error(3, 2, 123)));
        }
        Ok(state_item.queue.pop_front())
    }
    fn keys_down(&mut self) -> io::Result<Vec<u16>> {
        let mut state_item = self.state.borrow_mut();
        state_item.snapshots += 1;
        Ok(state_item.held.clone())
    }
    fn axes_state(&mut self) -> io::Result<BTreeMap<u8, i32>> {
        let state_item = self.state.borrow();
        if state_item.snapshot_error {
            return Err(io::Error::other("synthetic snapshot error"));
        }
        Ok(state_item.axes.clone())
    }
}
impl Source for Fixture {
    fn discover(&mut self) -> io::Result<Vec<PathBuf>> {
        self.0.borrow_mut().scans += 1;
        Ok(vec!["/synthetic/event0".into()])
    }
    fn open(&mut self, _: &Path) -> io::Result<Box<dyn Pad>> {
        let mut state_item = self.0.borrow_mut();
        if state_item.denied {
            return Err(io::Error::from(io::ErrorKind::PermissionDenied));
        }
        state_item.opened += 1;
        Ok(Box::new(Device {
            info: state_item.info.clone(),
            state: self.0.clone(),
        }))
    }
}
struct MultiFixture(BTreeMap<PathBuf, Rc<RefCell<State>>>);
impl Source for MultiFixture {
    fn discover(&mut self) -> io::Result<Vec<PathBuf>> {
        for state in self.0.values() {
            state.borrow_mut().scans += 1;
        }
        // Discovery order and duplicate paths must not become owner selection.
        let mut paths: Vec<_> = self.0.keys().rev().cloned().collect();
        paths.extend(self.0.keys().next().cloned());
        Ok(paths)
    }
    fn open(&mut self, path: &Path) -> io::Result<Box<dyn Pad>> {
        let state = self.0.get(path).ok_or_else(|| io::Error::other("unknown synthetic path"))?;
        Fixture(state.clone()).open(path)
    }
}
fn state(board: bool) -> Rc<RefCell<State>> {
    Rc::new(RefCell::new(State {
        info: info(board),
        queue: VecDeque::new(),
        held: vec![],
        axes: BTreeMap::from([(0, 128), (1, 128), (16, 0), (17, 0)]),
        opened: 0,
        closed: 0,
        reads: 0,
        snapshots: 0,
        scans: 0,
        failed: false,
        denied: false,
        noisy: false,
        snapshot_error: false,
    }))
}
fn fixture(board: bool) -> (Rc<RefCell<State>>, Receiver<Fixture>) {
    let state = state(board);
    (state.clone(), Receiver::with_profiles(Fixture(state), common::evdev_profiles()))
}
fn multiple(boards: &[bool]) -> (Vec<Rc<RefCell<State>>>, Receiver<MultiFixture>) {
    let mut devices = BTreeMap::new();
    let mut states = Vec::new();
    for (index, &board) in boards.iter().enumerate() {
        let state = state(board);
        let path = PathBuf::from(format!("/synthetic/event{index}"));
        state.borrow_mut().info.path = path.to_string_lossy().into_owned();
        devices.insert(path, state.clone());
        states.push(state);
    }
    (states, Receiver::with_profiles(MultiFixture(devices), common::evdev_profiles()))
}
fn poll<S: Source>(receiver_item: &mut Receiver<S>) -> Buttons {
    receiver_item.poll(Duration::from_secs(1))
}
fn observe<S: Source>(receiver_item: &mut Receiver<S>) {
    assert!(!poll(receiver_item).any());
    assert!(!poll(receiver_item).any());
    assert!(receiver_item.connected());
}
fn arm<S: Source>(state_item: &Rc<RefCell<State>>, receiver_item: &mut Receiver<S>) {
    observe(receiver_item);
    let code = if state_item.borrow().info.key_codes.contains(&21) { 21 } else { 288 };
    tap(state_item, code);
    assert!(!poll(receiver_item).any());
    assert!(receiver_item.diagnostic_identity().is_some());
    assert!(!poll(receiver_item).any());
}
fn error(kind: u16, code: u16, value: i32) -> Event {
    Event { kind, code, value }
}
fn packet(state_item: &Rc<RefCell<State>>, event: Event) {
    state_item.borrow_mut().queue.extend([event, error(0, 0, 0)]);
}
fn tap(state_item: &Rc<RefCell<State>>, code: u16) {
    packet(state_item, error(1, code, 1));
    packet(state_item, error(1, code, 0));
}

#[test]
fn construction_is_pure_and_selected_owner_is_closed_on_drop() {
    let (state_item, mut receiver_item) = fixture(false);
    assert_eq!(state_item.borrow().opened, 0);
    arm(&state_item, &mut receiver_item);
    receiver_item.poll(Duration::from_secs(100));
    assert_eq!(state_item.borrow().opened, 1);
    assert_eq!(state_item.borrow().scans, 2);
    drop(receiver_item);
    assert_eq!(state_item.borrow().closed, 1);
}

#[test]
fn inactive_first_receiver_does_not_hide_a_fresh_press_from_the_second_identity() {
    let (states, _) = multiple(&[true, true, true, true]);
    for index in [0, 2] {
        let mut state = states[index].borrow_mut();
        state.info.name = "HID 0810:0001".into();
        state.info.identity.insert("id/vendor".into(), "0810".into());
        state.info.identity.insert("id/product".into(), "0001".into());
    }
    let mut profiles = common::evdev_profiles();
    let mut document: serde_json::Value = serde_json::from_slice(
        &common::control_bytes(common::BOARD_EVDEV_FILE)
    ).unwrap();
    document["name"] = "HID 0810:0001".into();
    document["match"]["vendor"] = "0810".into();
    document["match"]["product"] = "0001".into();
    profiles.push(imanlib::gamepad::profile::Profile::parse(
        &serde_json::to_vec(&document).unwrap()
    ).unwrap());
    let devices = states.iter().map(|state| {
        (PathBuf::from(&state.borrow().info.path), state.clone())
    }).collect();
    let mut receiver = Receiver::with_profiles(MultiFixture(devices), profiles);
    receiver.enable_diagnostics();
    assert!(states.iter().all(|state| state.borrow().opened == 0));

    observe(&mut receiver);

    assert!(receiver.diagnostic_identity().is_none());
    assert!(states.iter().all(|state| state.borrow().opened == 1));
    assert_eq!(receiver.diagnostics().connections, 4);
    packet(&states[1], error(1, 28, 1));
    assert!(!poll(&mut receiver).any());
    let identity = receiver.diagnostic_identity().unwrap();
    assert_eq!(identity.path, "/synthetic/event1");
    assert_eq!(identity.vendor.as_deref(), Some("20bc"));
    for index in [0, 2, 3] {
        assert_eq!(states[index].borrow().closed, 0);
    }
    assert!(!poll(&mut receiver).any());
    packet(&states[1], error(1, 28, 0));
    assert!(!poll(&mut receiver).any());
    tap(&states[1], 48);
    assert!(poll(&mut receiver).down(Favourite));

    drop(receiver);

    assert!(states.iter().all(|state| state.borrow().closed == 1));
}

#[test]
fn selecting_poll_and_all_held_controls_are_consumed_until_neutral() {
    let (states, mut receiver) = multiple(&[true, true]);
    observe(&mut receiver);
    packet(&states[1], error(1, 30, 1));
    packet(&states[1], error(3, 16, -1));
    tap(&states[1], 48);

    assert!(!poll(&mut receiver).any());

    assert_eq!(receiver.diagnostic_identity().unwrap().path, "/synthetic/event1");
    assert!(!poll(&mut receiver).any());
    packet(&states[1], error(1, 30, 0));
    tap(&states[1], 54);
    assert!(!poll(&mut receiver).any());
    packet(&states[1], error(3, 16, 0));
    assert!(!poll(&mut receiver).any());
    assert!(!poll(&mut receiver).any());
    tap(&states[1], 48);
    let buttons = poll(&mut receiver);
    assert!(buttons.down(Favourite));
    assert!(!buttons.down(Activate));
    assert!(!buttons.down(Trash));
    assert!(!buttons.down(Left));
}

#[test]
fn initial_hold_and_opening_backlog_cannot_select_an_owner() {
    let (states, mut receiver) = multiple(&[true, true]);
    states[0].borrow_mut().held = vec![30];
    tap(&states[0], 28);
    tap(&states[1], 28);

    observe(&mut receiver);

    assert!(receiver.diagnostic_identity().is_none());
    tap(&states[0], 48);
    assert!(!poll(&mut receiver).any());
    assert!(receiver.diagnostic_identity().is_none());
    tap(&states[1], 28);
    assert!(!poll(&mut receiver).any());
    assert_eq!(receiver.diagnostic_identity().unwrap().path, "/synthetic/event1");
}

#[test]
fn axis_motion_unknown_keys_and_repeats_cannot_select_a_menu_owner() {
    let (states, mut receiver) = multiple(&[true, true]);
    observe(&mut receiver);
    for (code, value, neutral) in [(0, 0, 128), (16, 1, 0)] {
        packet(&states[1], error(3, code, value));
        assert!(!poll(&mut receiver).any());
        assert!(receiver.diagnostic_identity().is_none());
        packet(&states[1], error(3, code, neutral));
        assert!(!poll(&mut receiver).any());
    }
    tap(&states[1], 107);
    packet(&states[1], error(1, 48, 2));
    assert!(!poll(&mut receiver).any());
    assert!(receiver.diagnostic_identity().is_none());
    packet(&states[1], error(1, 48, 1));
    assert!(!poll(&mut receiver).any());
    assert!(receiver.diagnostic_identity().is_none());
    packet(&states[1], error(1, 48, 0));
    assert!(!poll(&mut receiver).any());

    tap(&states[1], 48);

    assert!(!poll(&mut receiver).any());
    assert_eq!(receiver.diagnostic_identity().unwrap().path, "/synthetic/event1");
}

#[test]
fn fresh_other_controller_transfers_without_replug_and_never_merges_states() {
    let (states, mut receiver) = multiple(&[true, true]);
    arm(&states[1], &mut receiver);
    let ignored_reads = states[0].borrow().reads;
    packet(&states[0], error(1, 30, 1));
    packet(&states[0], error(3, 16, -1));
    tap(&states[1], 48);

    let buttons = poll(&mut receiver);

    assert!(!buttons.any());
    assert!(states[0].borrow().reads > ignored_reads);
    assert_eq!(receiver.diagnostic_identity().unwrap().path, "/synthetic/event0");
    assert!(!poll(&mut receiver).any());
    packet(&states[0], error(1, 30, 0));
    packet(&states[0], error(3, 16, 0));
    assert!(!poll(&mut receiver).any());
    tap(&states[0], 45);
    let buttons = poll(&mut receiver);
    assert!(buttons.down(LowerFilters));
    assert!(!buttons.down(Favourite));
    assert!(!buttons.down(Activate));
    assert!(!buttons.down(Left));
    assert!(!receiver.poll(Duration::from_secs(100)).any());
    assert!(receiver.connected());
    assert_eq!(receiver.diagnostic_identity().unwrap().path, "/synthetic/event0");
    assert!(states.iter().all(|state| state.borrow().opened == 1));
    assert!(states.iter().all(|state| state.borrow().scans == 2));
}

#[test]
fn owner_disconnect_does_not_promote_a_stale_hold_and_fresh_other_press_is_required() {
    let (states, mut receiver) = multiple(&[true, true]);
    states[1].borrow_mut().held = vec![48];
    arm(&states[0], &mut receiver);
    states[0].borrow_mut().failed = true;

    assert!(!receiver.poll(Duration::from_secs(4)).any());

    assert!(receiver.connected());
    assert!(receiver.diagnostic_identity().is_none());
    assert_eq!(states[0].borrow().closed, 1);
    assert_eq!(states[1].borrow().closed, 0);
    assert!(!receiver.poll(Duration::from_secs(5)).any());
    assert!(states.iter().all(|state| state.borrow().opened == 1));
    assert!(!receiver.poll(Duration::from_secs(6)).any());
    assert_eq!(states[0].borrow().opened, 2);
    assert_eq!(states[1].borrow().opened, 1);
    assert!(receiver.connected());
    assert!(receiver.diagnostic_identity().is_none());
    tap(&states[1], 28);
    assert!(!poll(&mut receiver).any());
    assert!(receiver.diagnostic_identity().is_none());
    packet(&states[1], error(1, 48, 0));
    assert!(!poll(&mut receiver).any());
    tap(&states[1], 28);
    assert!(!poll(&mut receiver).any());
    assert_eq!(receiver.diagnostic_identity().unwrap().path, "/synthetic/event1");
    assert!(!poll(&mut receiver).any());
    tap(&states[1], 48);
    assert!(poll(&mut receiver).down(Favourite));
}

#[test]
fn ownership_can_move_back_and_forth_between_open_fds_without_new_connections() {
    let (states, mut receiver) = multiple(&[true, true]);
    receiver.enable_diagnostics();
    arm(&states[0], &mut receiver);
    for index in [1, 0, 1, 0] {
        tap(&states[index], 28);
        assert!(!poll(&mut receiver).any());
        assert_eq!(receiver.diagnostic_identity().unwrap().path, format!("/synthetic/event{index}"));
        assert!(!poll(&mut receiver).any());
        tap(&states[index], 48);
        assert!(poll(&mut receiver).down(Favourite));
        assert!(!poll(&mut receiver).any());
    }

    assert_eq!(receiver.diagnostics().connections, 2);
    assert_eq!(receiver.diagnostics().disconnects, 0);
    assert!(states.iter().all(|state| state.borrow().opened == 1 && state.borrow().closed == 0));
}

#[test]
fn new_receiver_is_discovered_while_owner_is_live_but_opening_backlog_does_not_transfer() {
    let (states, mut receiver) = multiple(&[true, true]);
    states[1].borrow_mut().denied = true;
    arm(&states[0], &mut receiver);
    states[1].borrow_mut().denied = false;
    tap(&states[1], 28);

    assert!(!receiver.poll(Duration::from_secs(3)).any());

    assert_eq!(receiver.diagnostic_identity().unwrap().path, "/synthetic/event0");
    assert!(!poll(&mut receiver).any());
    tap(&states[1], 28);
    assert!(!poll(&mut receiver).any());
    assert_eq!(receiver.diagnostic_identity().unwrap().path, "/synthetic/event1");
    assert!(states.iter().all(|state| state.borrow().opened == 1));
}

#[test]
fn incomplete_dropped_and_over_budget_candidate_polls_cannot_select_an_owner() {
    for variant in 0..3 {
        let (states, mut receiver) = multiple(&[true, true]);
        observe(&mut receiver);
        tap(&states[1], 28);
        match variant {
            0 => states[1].borrow_mut().queue.push_back(error(1, 48, 1)),
            1 => packet(&states[1], error(0, 3, 0)),
            _ => {
                let mut state = states[1].borrow_mut();
                state.queue.extend((0..256).map(|_| error(0, 0, 0)));
            }
        }

        assert!(!poll(&mut receiver).any());

        assert!(receiver.diagnostic_identity().is_none());
        packet(&states[1], error(1, 48, 0));
        assert!(!poll(&mut receiver).any());
        assert!(!poll(&mut receiver).any());
        tap(&states[1], 28);
        assert!(!poll(&mut receiver).any());
        assert_eq!(receiver.diagnostic_identity().unwrap().path, "/synthetic/event1");
    }
}

#[test]
fn conflicting_held_directions_are_not_a_safe_neutral_for_selection() {
    let (state, mut receiver) = fixture(true);
    state.borrow_mut().held = vec![103, 108];
    observe(&mut receiver);
    tap(&state, 28);
    assert!(!poll(&mut receiver).any());
    assert!(receiver.diagnostic_identity().is_none());
    packet(&state, error(1, 103, 0));
    packet(&state, error(1, 108, 0));
    assert!(!poll(&mut receiver).any());

    tap(&state, 28);

    assert!(!poll(&mut receiver).any());
    assert!(receiver.diagnostic_identity().is_some());
}

#[test]
fn candidate_pool_and_poll_work_remain_bounded_and_drop_closes_every_fd() {
    let (states, mut receiver) = multiple(&[true; 70]);
    receiver.enable_diagnostics();
    for state in &states {
        state.borrow_mut().noisy = true;
    }

    assert!(!poll(&mut receiver).any());

    assert!(receiver.diagnostic_identity().is_none());
    assert_eq!(states.iter().map(|state| state.borrow().opened).sum::<usize>(), 8);
    assert_eq!(states.iter().map(|state| state.borrow().reads).sum::<usize>(), 8 * 256);
    assert_eq!(receiver.diagnostics().connections, 8);
    assert_eq!(receiver.diagnostics().poll_limits, 8);
    drop(receiver);
    assert_eq!(states.iter().map(|state| state.borrow().closed).sum::<usize>(), 8);
}

#[test]
fn held_controls_and_backlog_at_open_require_neutral_then_a_new_press() {
    for (key, axis) in [(Some(296), None), (None, Some(0)), (None, Some(16))] {
        let (state_item, mut receiver_item) = fixture(false);
        if let Some(code) = key {
            state_item.borrow_mut().held = vec![code];
        }
        if let Some(code) = axis {
            state_item.borrow_mut()
                .axes
                .insert(code, if code == 16 { -1 } else { 0 });
        }
        tap(&state_item, 291);
        observe(&mut receiver_item);
        assert!(receiver_item.diagnostic_identity().is_none());
        assert!(!poll(&mut receiver_item).any());
        if let Some(code) = key {
            packet(&state_item, error(1, code, 0));
        }
        if let Some(code) = axis {
            packet(&state_item, error(3, u16::from(code), if code == 16 { 0 } else { 128 }));
        }
        assert!(!poll(&mut receiver_item).any());
        tap(&state_item, 291);
        assert!(!poll(&mut receiver_item).any());
        assert!(receiver_item.diagnostic_identity().is_some());
        assert!(!poll(&mut receiver_item).any());
        tap(&state_item, 291);
        assert!(poll(&mut receiver_item).down(LowerFilters));
    }
}

#[test]
fn syn_dropped_discards_taps_and_resnapshots_keys_and_axes_after_report() {
    let (state_item, mut receiver_item) = fixture(false);
    arm(&state_item, &mut receiver_item);
    tap(&state_item, 296);
    state_item.borrow_mut().queue.extend([error(0, 3, 0), error(1, 291, 1)]);
    assert!(!poll(&mut receiver_item).any());
    assert_eq!(state_item.borrow().snapshots, 1);
    state_item.borrow_mut().held = vec![289];
    state_item.borrow_mut().axes.insert(16, 1);
    packet(&state_item, error(1, 291, 0));
    assert!(!poll(&mut receiver_item).any());
    assert_eq!(state_item.borrow().snapshots, 2);
    assert!(!poll(&mut receiver_item).any());
    packet(&state_item, error(1, 289, 0));
    packet(&state_item, error(3, 16, 0));
    assert!(!poll(&mut receiver_item).any());
    tap(&state_item, 296);
    assert!(poll(&mut receiver_item).down(Trash));
}

#[test]
fn partial_packet_and_budget_exhaustion_never_publish_old_edges() {
    for noisy in [false, true] {
        let (state_item, mut receiver_item) = fixture(false);
        arm(&state_item, &mut receiver_item);
        if noisy {
            state_item.borrow_mut().noisy = true;
        } else {
            tap(&state_item, 296);
            state_item.borrow_mut().queue.push_back(error(1, 291, 1));
        }
        let before = state_item.borrow().reads;
        assert!(!poll(&mut receiver_item).any());
        assert!(state_item.borrow().reads - before <= 256);
        state_item.borrow_mut().noisy = false;
        packet(&state_item, error(1, 291, 0));
        assert!(!poll(&mut receiver_item).any());
        tap(&state_item, 291);
        assert!(poll(&mut receiver_item).down(LowerFilters));
    }
}

#[test]
fn invalid_identity_capabilities_or_ranges_are_rejected_and_closed() {
    for variant in 0..6 {
        let (state_item, mut receiver_item) = fixture(false);
        match variant {
            0 => {
                state_item.borrow_mut().info.name = "Keyboard".into();
            }
            1 => {
                state_item.borrow_mut()
                    .info
                    .identity
                    .insert("id/product".into(), "0556".into());
            }
            2 => {
                state_item.borrow_mut().info.key_codes.retain(|&code_item| code_item != 296);
            }
            3 => {
                state_item.borrow_mut().info.abs_info.remove(&16);
            }
            4 => {
                state_item.borrow_mut().info.abs_info.get_mut(&0).unwrap()[2] = 0;
            }
            _ => {
                state_item.borrow_mut().info.abs_info.get_mut(&16).unwrap()[1] = 0;
            }
        }
        assert!(!poll(&mut receiver_item).any());
        assert!(!receiver_item.connected());
        assert_eq!(state_item.borrow().opened, state_item.borrow().closed);
    }
}

#[test]
fn bad_event_or_snapshot_fails_closed_not_as_a_synthetic_press() {
    for variant in 0..4 {
        let (state_item, mut receiver_item) = fixture(false);
        arm(&state_item, &mut receiver_item);
        match variant {
            0 => packet(&state_item, error(1, 291, 3)),
            1 => packet(&state_item, error(3, 0, 256)),
            2 => {
                state_item.borrow_mut().snapshot_error = true;
                packet(&state_item, error(0, 3, 0));
            }
            _ => {
                state_item.borrow_mut().axes.remove(&17);
                packet(&state_item, error(0, 3, 0));
            }
        }
        assert!(!poll(&mut receiver_item).any());
        assert!(!receiver_item.connected());
        assert_eq!(state_item.borrow().closed, 1);
    }
}

#[test]
fn reconnect_and_permission_failures_are_rate_limited_and_neutral_gated() {
    let (state_item, mut receiver_item) = fixture(false);
    state_item.borrow_mut().denied = true;
    assert!(!poll(&mut receiver_item).any());
    poll(&mut receiver_item);
    assert_eq!(state_item.borrow().scans, 1);
    state_item.borrow_mut().denied = false;
    receiver_item.poll(Duration::from_secs(3));
    arm(&state_item, &mut receiver_item);
    packet(&state_item, error(1, 291, 1));
    assert!(poll(&mut receiver_item).down(LowerFilters));
    state_item.borrow_mut().failed = true;
    assert!(!receiver_item.poll(Duration::from_secs(4)).any());
    assert!(!receiver_item.connected());
    assert_eq!(state_item.borrow().closed, 1);
    state_item.borrow_mut().failed = false;
    state_item.borrow_mut().held = vec![291];
    assert!(!receiver_item.poll(Duration::from_secs(5)).any());
    assert_eq!(state_item.borrow().opened, 1);
    assert!(!receiver_item.poll(Duration::from_secs(6)).any());
    assert_eq!(state_item.borrow().opened, 2);
    assert!(!poll(&mut receiver_item).any());
    packet(&state_item, error(1, 291, 0));
    assert!(!poll(&mut receiver_item).any());
    tap(&state_item, 291);
    assert!(!poll(&mut receiver_item).any());
    assert!(receiver_item.diagnostic_identity().is_some());
    assert!(!poll(&mut receiver_item).any());
    tap(&state_item, 291);
    assert!(poll(&mut receiver_item).down(LowerFilters));
}

#[test]
fn deadzone_hat_priority_and_conflicting_directions_are_safe() {
    let (state_item, mut receiver_item) = fixture(true);
    arm(&state_item, &mut receiver_item);
    for value_item in [64, 128, 191] {
        packet(&state_item, error(3, 0, value_item));
        assert!(!poll(&mut receiver_item).any());
    }
    packet(&state_item, error(3, 0, 0));
    packet(&state_item, error(3, 16, 1));
    let button = poll(&mut receiver_item);
    assert!(button.down(Right));
    assert!(!button.down(Left));
    packet(&state_item, error(1, 105, 1));
    assert!(!poll(&mut receiver_item).any());
    packet(&state_item, error(3, 0, 128));
    packet(&state_item, error(3, 16, 0));
    packet(&state_item, error(1, 105, 0));
    poll(&mut receiver_item);
}

#[test]
fn production_source_rejects_regular_files_symlinks_and_unrelated_devices() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("event0");
    std::fs::write(&path, [0; 24]).unwrap();
    assert!(ProfileSource::new(common::evdev_profiles()).open(&path).is_err());
    std::fs::remove_file(&path).unwrap();
    std::os::unix::fs::symlink("/dev/null", &path).unwrap();
    assert!(ProfileSource::new(common::evdev_profiles()).open(&path).is_err());
    assert!(ProfileSource::new(common::evdev_profiles()).open(Path::new("/dev/null")).is_err());
}

#[test]
fn custom_json_changes_identity_buttons_and_axis_roles_without_device_specific_code() {
    use imanlib::gamepad::profile::Profile;
    let (state_item, _) = fixture(true);
    let mut document: serde_json::Value = serde_json::from_slice(&common::control_bytes(common::BOARD_EVDEV_FILE)).unwrap();
    document["match"]["vendor"] = serde_json::json!("1234");
    document["name"] = serde_json::json!("Synthetic pad");
    document["buttons"]["west"] = serde_json::json!(21);
    document["buttons"]["north"] = serde_json::json!(45);
    document["axes"]["left_stick_x"] = serde_json::json!(1);
    document["axes"]["left_stick_y"] = serde_json::json!(0);
    let profile = Profile::parse(&serde_json::to_vec(&document).unwrap()).unwrap();
    state_item.borrow_mut()
        .info
        .identity
        .insert("id/vendor".into(), "1234".into());
    state_item.borrow_mut().info.name = "Synthetic pad".into();
    let mut receiver_item = Receiver::with_profiles(Fixture(state_item.clone()), vec![profile]);
    arm(&state_item, &mut receiver_item);

    tap(&state_item, 21);
    assert!(poll(&mut receiver_item).down(LowerFilters));
    assert!(!poll(&mut receiver_item).any());
    packet(&state_item, error(3, 1, 0));
    assert!(poll(&mut receiver_item).down(Left));
}

#[test]
fn digital_only_profile_does_not_require_axes_or_axis_snapshot() {
    use imanlib::gamepad::profile::Profile;
    let (state, _) = fixture(true);
    let mut document: serde_json::Value = serde_json::from_slice(&common::control_bytes(common::BOARD_EVDEV_FILE)).unwrap();
    document["axes"] = serde_json::json!({});
    let profile = Profile::parse(&serde_json::to_vec(&document).unwrap()).unwrap();
    state.borrow_mut().info.axis_codes.clear();
    state.borrow_mut().info.abs_info.clear();
    state.borrow_mut().snapshot_error = true;
    let mut receiver = Receiver::with_profiles(Fixture(state.clone()), vec![profile]);

    arm(&state, &mut receiver);
    tap(&state, 103);

    assert!(poll(&mut receiver).down(Up));
    assert!(receiver.connected());
}

#[test]
fn optional_inverted_stick_pair_navigates_without_a_hat() {
    use imanlib::gamepad::profile::Profile;
    let (state, _) = fixture(true);
    let mut document: serde_json::Value = serde_json::from_slice(&common::control_bytes(common::BOARD_EVDEV_FILE)).unwrap();
    document["axes"] = serde_json::json!({"left_stick_x":{"code":0,"invert":true},"left_stick_y":{"code":1,"invert":true}});
    for key in ["up", "down", "left", "right"] { document["buttons"].as_object_mut().unwrap().remove(key); }
    let profile = Profile::parse(&serde_json::to_vec(&document).unwrap()).unwrap();
    state.borrow_mut().info.axis_codes = vec![0, 1];
    state.borrow_mut().info.abs_info.retain(|&code, _| code < 2);
    state.borrow_mut().axes.retain(|&code, _| code < 2);
    let mut receiver = Receiver::with_profiles(Fixture(state.clone()), vec![profile]);

    arm(&state, &mut receiver);
    packet(&state, error(3, 0, 0));
    assert!(poll(&mut receiver).down(Right));
    packet(&state, error(3, 0, 128));
    packet(&state, error(3, 1, 255));

    assert!(poll(&mut receiver).down(Up));
}

#[test]
fn passive_diagnostics_do_not_open_or_read_again_and_keep_loss_and_disconnect_counts() {
    let (state_item, mut receiver_item) = fixture(true);
    receiver_item.enable_diagnostics();
    assert_eq!(state_item.borrow().opened, 0);
    arm(&state_item, &mut receiver_item);
    packet(&state_item, error(3, 17, -1));
    assert!(poll(&mut receiver_item).down(Up));
    state_item.borrow_mut().queue.extend([error(0, 3, 0), error(0, 0, 0)]);
    assert!(!poll(&mut receiver_item).any());
    let reads = state_item.borrow().reads;
    let snapshot = receiver_item.diagnostics();
    let identity = receiver_item.diagnostic_identity().unwrap();
    assert_eq!(state_item.borrow().reads, reads);
    assert_eq!(snapshot.connections, 1);
    assert_eq!(snapshot.syn_dropped, 1);
    assert_eq!(identity.vendor.as_deref(), Some("20bc"));
    assert!(snapshot
        .controls
        .iter()
        .any(|code_item| code_item.code == 17 && code_item.min == -1));
    state_item.borrow_mut().failed = true;
    poll(&mut receiver_item);
    assert_eq!(receiver_item.diagnostics().disconnects, 1);
    assert_eq!(receiver_item.diagnostics().errors, 1);
    assert_eq!(state_item.borrow().opened, 1);
    assert_eq!(state_item.borrow().closed, 1);
}

#[test]
fn ambiguous_evdev_profiles_do_not_read_or_activate_the_device() {
    let (state, _) = fixture(true);
    let profile = common::evdev_profiles().remove(0);
    let mut receiver = Receiver::with_profiles(Fixture(state.clone()), vec![profile.clone(), profile]);

    assert!(!receiver.poll(Duration::ZERO).any());

    let state = state.borrow();
    assert!(!receiver.connected());
    assert_eq!(state.reads, 0);
    assert_eq!(state.snapshots, 0);
    assert_eq!(state.opened, state.closed);
}

#[test]
fn conflicting_hid_json_does_not_disable_a_unique_evdev_profile() {
    let directory = tempfile::tempdir().unwrap();
    for name in ["hid-a.json", "hid-b.json"] {
        std::fs::write(directory.path().join(name), common::control_bytes(common::HID_FILE)).unwrap();
    }
    std::fs::write(directory.path().join("evdev.json"), common::control_bytes(common::PC_EVDEV_FILE)).unwrap();
    let profiles = imanlib::controls::load(directory.path(), None, None).unwrap();
    assert!(profiles.hid.is_empty());
    assert_eq!(profiles.conflicts.len(), 1);
    let (state, _) = fixture(false);
    let mut receiver = Receiver::with_profiles(Fixture(state.clone()), profiles.evdev);
    arm(&state, &mut receiver);
    tap(&state, 289);

    assert!(poll(&mut receiver).any());
    assert!(receiver.connected());
}
