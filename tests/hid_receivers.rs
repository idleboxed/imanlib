// SPDX-License-Identifier: BSD-3-Clause
#![cfg(feature = "input")]
mod common;
use {
    imanlib::hid::receiver::Device,
    imanlib::hid::receiver::Info,
    imanlib::hid::receiver::Receivers,
    imanlib::hid::receiver::Source,
    imanlib::hid::Control,
    imanlib::igui::Action,
};
use std::{
    cell::RefCell,
    collections::{BTreeMap, VecDeque},
    io,
    path::{Path, PathBuf},
    rc::Rc,
    time::Duration,
};

#[derive(Default)]
struct Fixture {
    identity: Option<(u16, u16, String)>,
    queue: VecDeque<Vec<u8>>,
    reads: usize,
    opened: usize,
    closed: usize,
    denied: bool,
    failed: bool,
    interrupted: bool,
}
struct Pad {
    state: Rc<RefCell<Fixture>>,
    info: Info,
}
impl Drop for Pad {
    fn drop(&mut self) {
        self.state.borrow_mut().closed += 1;
    }
}
impl Device for Pad {
    fn info(&self) -> &Info {
        &self.info
    }
    fn read(&mut self, packet: &mut [u8]) -> io::Result<Option<usize>> {
        let mut s = self.state.borrow_mut();
        s.reads += 1;
        if s.failed {
            return Err(io::Error::other("synthetic disconnection"));
        }
        if s.interrupted {
            return Err(io::ErrorKind::Interrupted.into());
        }
        Ok(s.queue.pop_front().map(|p| {
            packet[..p.len()].copy_from_slice(&p);
            p.len()
        }))
    }
}
struct Devices {
    fixtures: Vec<Rc<RefCell<Fixture>>>,
}
impl Source for Devices {
    fn discover(&mut self) -> io::Result<Vec<PathBuf>> {
        Ok((0..self.fixtures.len())
            .map(|i| PathBuf::from(format!("/synthetic/hidraw{i}")))
            .collect())
    }
    fn open(&mut self, path: &Path) -> io::Result<Box<dyn Device>> {
        let i: usize = path
            .to_str()
            .unwrap()
            .trim_start_matches("/synthetic/hidraw")
            .parse()
            .unwrap();
        let state = self.fixtures[i].clone();
        if state.borrow().denied {
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        state.borrow_mut().opened += 1;
        let (vendor, product, name) = state.borrow().identity.clone()
            .unwrap_or((0x2563, 0x0555, "ShanWan Twin USB Joystick".into()));
        Ok(Box::new(Pad {
            state,
            info: Info {
                generation: path.into(),
                bus: 3,
                vendor,
                product,
                name,
                descriptor: include_bytes!("fixtures/shanwan-2563-0555.bin").to_vec(),
            },
        }))
    }
}
fn send(f: &Rc<RefCell<Fixture>>, id: u8, buttons: u16) {
    f.borrow_mut().queue.push_back(vec![
        id,
        128,
        128,
        128,
        128,
        15 | ((buttons as u8 & 15) << 4),
        (buttons >> 4) as u8,
        0,
    ]);
}
fn setup(n: usize) -> (Receivers<Devices>, Vec<Rc<RefCell<Fixture>>>) {
    let fixtures: Vec<_> = (0..n)
        .map(|_| Rc::new(RefCell::new(Fixture::default())))
        .collect();
    let receivers = Receivers::new(
        Devices {
            fixtures: fixtures.clone(),
        },
        common::hid_profile(),
    );
    (receivers, fixtures)
}
fn arm(r: &mut Receivers<Devices>, f: &[Rc<RefCell<Fixture>>]) {
    for pad in f {
        send(pad, 1, 0);
        send(pad, 2, 0);
    }
    assert!(!r.poll(Duration::ZERO).any());
    assert!(!r.poll(Duration::from_millis(1)).any());
    assert!(r.owner().is_none());
    send(&f[0], 1, 8);
    send(&f[0], 1, 0);
    assert!(!r.poll(Duration::from_millis(2)).any());
    assert!(r.owner().is_some());
    assert!(!r.poll(Duration::from_millis(3)).any());
}
#[test]
fn construction_is_pure_and_two_receivers_have_four_independent_channels() {
    let (mut r, f) = setup(2);
    assert!(f.iter().all(|s| s.borrow().opened == 0));
    arm(&mut r, &f);
    let owner = r.owner().unwrap();
    assert_eq!(owner.report_id, 1);
    send(&f[0], 1, 8);
    send(&f[0], 2, 2);
    send(&f[1], 1, 1);
    send(&f[1], 2, 4);

    let buttons = r.poll(Duration::from_millis(2));
    assert!(buttons.down(Action::LowerFilters));
    assert!(!buttons.down(Action::Favourite));
    let snapshots = r.snapshots();
    assert_eq!(snapshots.len(), 4);
    assert_eq!(snapshots[0].state.value(Control::West), 1);
    assert_eq!(snapshots[1].state.value(Control::East), 1);
    assert_eq!(snapshots[2].state.value(Control::North), 1);
    assert_eq!(snapshots[3].state.value(Control::South), 1);
    assert_ne!(snapshots[0].id, snapshots[2].id);
    assert_eq!(r.owner(), Some(owner));
    drop(r);
    assert!(f.iter().all(|s| s.borrow().closed == 1));
}
#[test]
fn taps_are_preserved_but_initial_held_buttons_require_release() {
    let (mut r, f) = setup(1);
    send(&f[0], 1, 8);
    assert!(!r.poll(Duration::ZERO).any());
    assert!(!r.poll(Duration::from_millis(1)).any());
    send(&f[0], 1, 0);
    assert!(!r.poll(Duration::from_millis(2)).any());
    send(&f[0], 1, 8);
    send(&f[0], 1, 0);
    assert!(!r.poll(Duration::from_millis(3)).any());
    assert!(!r.poll(Duration::from_millis(4)).any());
    send(&f[0], 1, 8);
    send(&f[0], 1, 0);
    assert!(r.poll(Duration::from_millis(5)).down(Action::LowerFilters));
}
#[test]
fn late_channel_is_not_armed_by_another_channels_neutral() {
    let (mut r, f) = setup(1);
    send(&f[0], 1, 0);
    r.poll(Duration::ZERO);
    r.poll(Duration::from_millis(1));
    send(&f[0], 2, 8);
    r.poll(Duration::from_millis(2));
    assert!(!r.snapshots()[1].ready);
    send(&f[0], 2, 0);
    r.poll(Duration::from_millis(3));
    assert!(r.snapshots()[1].ready);
}
#[test]
fn malformed_packet_closes_receiver_and_reassignment_needs_neutral() {
    let (mut r, f) = setup(2);
    arm(&mut r, &f);
    send(&f[1], 1, 8);
    f[0].borrow_mut().queue.push_back(vec![1]);
    assert!(!r.poll(Duration::from_millis(2)).any());
    assert_eq!(f[0].borrow().closed, 1);
    assert!(!r.poll(Duration::from_millis(3)).any());
    send(&f[1], 1, 0);
    assert!(!r.poll(Duration::from_millis(4)).any());
    send(&f[1], 1, 8);
    assert!(r.poll(Duration::from_millis(5)).down(Action::LowerFilters));
}
#[test]
fn reconnect_reusing_same_node_gets_new_generation_and_no_old_press() {
    let (mut r, f) = setup(1);
    arm(&mut r, &f);
    let old = r.owner().unwrap();
    f[0].borrow_mut().failed = true;
    r.poll(Duration::from_millis(2));
    assert!(!r.connected());
    f[0].borrow_mut().failed = false;
    send(&f[0], 1, 8);
    assert!(!r.poll(Duration::from_secs(2)).any());
    assert!(r.owner().is_none());
    assert_eq!(f[0].borrow().opened, 2);
    assert!(!r.poll(Duration::from_secs(3)).any());
    send(&f[0], 1, 0);
    assert!(!r.poll(Duration::from_millis(3001)).any());
    send(&f[0], 1, 8);
    assert!(!r.poll(Duration::from_millis(3002)).any());
    assert_ne!(r.owner().unwrap(), old);
}
#[test]
fn denied_receiver_does_not_block_another_or_retry_each_frame() {
    let (mut r, f) = setup(2);
    f[0].borrow_mut().denied = true;
    arm(&mut r, &f[1..]);
    send(&f[1], 1, 8);
    assert!(r.poll(Duration::from_millis(2)).down(Action::LowerFilters));
    assert_eq!(r.snapshots().len(), 2);
    assert_eq!(f[0].borrow().opened, 0);
}
#[test]
fn flood_and_interrupts_are_bounded_and_discard_partial_actions() {
    let (mut r, f) = setup(1);
    arm(&mut r, &f);
    for _ in 0..300 {
        send(&f[0], 1, 8);
    }
    let reads = f[0].borrow().reads;
    assert!(!r.poll(Duration::from_millis(2)).any());
    assert_eq!(f[0].borrow().reads - reads, 256);
    send(&f[0], 1, 0);
    assert!(!r.poll(Duration::from_millis(3)).any());
    send(&f[0], 1, 8);
    assert!(r.poll(Duration::from_millis(4)).down(Action::LowerFilters));
    f[0].borrow_mut().interrupted = true;
    let reads = f[0].borrow().reads;
    assert!(!r.poll(Duration::from_millis(5)).any());
    assert_eq!(f[0].borrow().reads - reads, 256);
}
#[test]
fn fresh_press_moves_ownership_between_report_ids_without_reopening_or_merging() {
    let (mut receivers, fixtures) = setup(1);
    receivers.enable_diagnostics();
    arm(&mut receivers, &fixtures);
    for id in [2, 1, 2, 1] {
        send(&fixtures[0], id, 8);
        assert!(!receivers.poll(Duration::from_millis(10)).any());
        assert_eq!(receivers.owner().unwrap().report_id, id);
        // Periodic repeats of the selecting hold do not become actions.
        send(&fixtures[0], id, 8);
        assert!(!receivers.poll(Duration::from_millis(11)).any());
        send(&fixtures[0], id, 0);
        assert!(!receivers.poll(Duration::from_millis(12)).any());
        send(&fixtures[0], id, 2);
        send(&fixtures[0], id, 0);
        let buttons = receivers.poll(Duration::from_millis(13));
        assert!(buttons.down(Action::Favourite));
        assert!(!buttons.down(Action::LowerFilters));
        assert!(!receivers.poll(Duration::from_millis(14)).any());
    }

    assert_eq!(receivers.diagnostics().connections, 1);
    assert_eq!(fixtures[0].borrow().opened, 1);
    assert_eq!(fixtures[0].borrow().closed, 0);
}
#[test]
fn inactive_first_hid_channel_cannot_hide_fresh_input_from_another_receiver() {
    let (mut receivers, fixtures) = setup(2);
    for fixture in &fixtures {
        send(fixture, 1, 0);
        send(fixture, 2, 0);
    }
    assert!(!receivers.poll(Duration::ZERO).any());
    assert!(receivers.owner().is_none());
    send(&fixtures[1], 2, 8);

    assert!(!receivers.poll(Duration::from_millis(1)).any());

    assert_eq!(receivers.owner().unwrap(), receivers.snapshots()[3].id);
    send(&fixtures[1], 2, 0);
    assert!(!receivers.poll(Duration::from_millis(2)).any());
    send(&fixtures[1], 2, 4);
    assert!(receivers.poll(Duration::from_millis(3)).down(Action::Activate));
}
#[test]
fn old_hold_axis_motion_and_non_menu_buttons_cannot_transfer_hid_ownership() {
    let (mut receivers, fixtures) = setup(1);
    send(&fixtures[0], 2, 8);
    arm(&mut receivers, &fixtures[..1]);
    // arm() supplied a neutral for both channels; establish and transfer away
    // from a real hold, then verify repeated old reports cannot take it back.
    send(&fixtures[0], 2, 8);
    assert!(!receivers.poll(Duration::from_millis(4)).any());
    assert!(!receivers.poll(Duration::from_micros(4500)).any());
    send(&fixtures[0], 1, 2);
    assert!(!receivers.poll(Duration::from_millis(5)).any());
    let owner = receivers.owner().unwrap();
    assert_eq!(owner.report_id, 1);
    send(&fixtures[0], 2, 8);
    assert!(!receivers.poll(Duration::from_millis(6)).any());
    assert_eq!(receivers.owner(), Some(owner));
    send(&fixtures[0], 1, 0);
    send(&fixtures[0], 2, 0);
    assert!(!receivers.poll(Duration::from_millis(7)).any());
    // A complete axis-only packet moves the stick but cannot request ownership.
    let mut axis = vec![2, 0, 128, 128, 128, 15, 0, 0];
    fixtures[0].borrow_mut().queue.push_back(axis.clone());
    assert!(!receivers.poll(Duration::from_millis(8)).any());
    assert_eq!(receivers.owner(), Some(owner));
    axis[1] = 128;
    fixtures[0].borrow_mut().queue.push_back(axis);
    send(&fixtures[0], 2, 16); // Shoulder: profiled, but no menu action.
    assert!(!receivers.poll(Duration::from_millis(9)).any());
    assert_eq!(receivers.owner(), Some(owner));
}
#[test]
fn hid_receiver_pool_and_reads_are_bounded_and_drop_closes_all_opened_fds() {
    let (mut receivers, fixtures) = setup(12);
    for fixture in &fixtures {
        fixture.borrow_mut().interrupted = true;
    }

    assert!(!receivers.poll(Duration::ZERO).any());

    assert!(receivers.owner().is_none());
    assert_eq!(fixtures.iter().map(|fixture| fixture.borrow().opened).sum::<usize>(), 8);
    assert_eq!(fixtures.iter().map(|fixture| fixture.borrow().reads).sum::<usize>(), 8 * 256);
    drop(receivers);
    assert_eq!(fixtures.iter().map(|fixture| fixture.borrow().closed).sum::<usize>(), 8);
}
#[test]
fn capture_replay_preserves_all_four_start_sequences() {
    let decoder = imanlib::hid::Decoder::new(
        &common::hid_profile(),
        include_bytes!("fixtures/shanwan-2563-0555.bin"),
    )
    .unwrap();
    let mut states = BTreeMap::new();
    let mut starts = BTreeMap::new();
    let mut all_west = false;
    for line in include_str!("fixtures/shanwan-four-pads.jsonl").lines() {
        let v: serde_json::Value = serde_json::from_str(line).unwrap();
        if v["event"] != "state" {
            continue;
        }
        let bytes = v["report_hex"]
            .as_str()
            .unwrap()
            .as_bytes()
            .chunks(2)
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect::<Vec<_>>();
        let (id, state) = decoder.decode(&bytes).unwrap();
        let key = (v["node"].as_str().unwrap().to_owned(), id);
        let old: imanlib::hid::State = states.insert(key.clone(), state).unwrap_or_default();
        if state.value(Control::Start) == 1 && old.value(Control::Start) == 0 {
            *starts.entry(key).or_insert(0) += 1;
        }
        all_west |= states.len() == 4 && states.values().all(|s| s.value(Control::West) == 1);
    }
    assert_eq!(
        starts,
        BTreeMap::from([
            (("hidraw3".to_owned(), 1), 1),
            (("hidraw3".to_owned(), 2), 3),
            (("hidraw4".to_owned(), 1), 2),
            (("hidraw4".to_owned(), 2), 4),
        ])
    );
    assert!(all_west);
}
#[cfg(target_os = "linux")]
#[test]
fn linux_source_refuses_non_hidraw_paths_without_opening_devices() {
    let mut source = imanlib::hid::linux::LinuxSource::new(vec![common::hid_profile()]);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("hidraw0");
    std::fs::write(&path, b"fixture").unwrap();
    assert!(source.open(&path).is_err());
    assert!(source.open(Path::new("/dev/null")).is_err());
}

#[test]
fn passive_hid_diagnostics_count_packets_without_merging_channels_or_reading_snapshots_again() {
    let (mut r, f) = setup(2);
    r.enable_diagnostics();
    assert!(f.iter().all(|s| s.borrow().opened == 0));
    arm(&mut r, &f);
    send(&f[0], 1, 8);
    send(&f[1], 2, 2);
    assert!(r.poll(Duration::from_millis(2)).down(Action::LowerFilters));
    let reads = f[0].borrow().reads;
    let report = r.diagnostics();
    assert_eq!(report.connections, 2);
    assert_eq!(report.events, 8);
    assert_eq!(r.snapshots().len(), 4);
    assert_eq!(f[0].borrow().reads, reads);
    f[0].borrow_mut().failed = true;
    r.poll(Duration::from_millis(3));
    assert_eq!(r.diagnostics().disconnects, 1);
    assert_eq!(r.diagnostics().errors, 1);
}

#[test]
fn different_json_profiles_keep_one_owner_and_decode_their_own_bindings() {
    let mut value: serde_json::Value = serde_json::from_slice(&common::control_bytes(common::HID_FILE)).unwrap();
    value["name"] = "Synthetic second model".into();
    value["match"]["vendor"] = "1234".into();
    let south = value["controls"]["south"].clone();
    value["controls"]["south"] = value["controls"]["east"].clone();
    value["controls"]["east"] = south;
    let alternate = imanlib::hid::Profile::parse(&serde_json::to_vec(&value).unwrap()).unwrap();
    let fixtures: Vec<_> = (0..2).map(|_| Rc::new(RefCell::new(Fixture::default()))).collect();
    fixtures[1].borrow_mut().identity = Some((0x1234, 0x0555, "Synthetic second model".into()));
    let mut receivers = Receivers::with_profiles(
        Devices { fixtures: fixtures.clone() }, vec![alternate, common::hid_profile()],
    );
    arm(&mut receivers, &fixtures);
    let owner = receivers.owner();
    send(&fixtures[0], 1, 2);
    send(&fixtures[1], 1, 2);

    let buttons = receivers.poll(Duration::from_millis(2));

    assert!(buttons.down(Action::Favourite));
    assert!(!buttons.down(Action::Activate));
    assert_eq!(receivers.owner(), owner);
    let snapshots = receivers.snapshots();
    assert_eq!(snapshots[0].state.value(Control::East), 1);
    assert_eq!(snapshots[2].state.value(Control::South), 1);
    fixtures[0].borrow_mut().failed = true;
    assert!(!receivers.poll(Duration::from_millis(3)).any());
    send(&fixtures[1], 1, 0);
    assert!(!receivers.poll(Duration::from_millis(4)).any());
    send(&fixtures[1], 1, 2);
    assert!(!receivers.poll(Duration::from_millis(5)).any());
    send(&fixtures[1], 1, 0);
    assert!(!receivers.poll(Duration::from_millis(6)).any());
    send(&fixtures[1], 1, 2);
    assert!(receivers.poll(Duration::from_millis(7)).down(Action::Activate));
}

#[test]
fn receiver_refuses_ambiguous_profiles_even_outside_directory_loader() {
    let fixture = Rc::new(RefCell::new(Fixture::default()));
    let mut receivers = Receivers::with_profiles(
        Devices { fixtures: vec![fixture.clone()] },
        vec![common::hid_profile(), common::hid_profile()],
    );
    assert!(!receivers.poll(Duration::ZERO).any());
    assert!(!receivers.connected());
    assert_eq!(fixture.borrow().closed, 1);
}

#[test]
fn conflicting_evdev_json_does_not_disable_a_unique_hid_profile() {
    let directory = tempfile::tempdir().unwrap();
    for name in ["evdev-a.json", "evdev-b.json"] {
        std::fs::write(directory.path().join(name), common::control_bytes(common::PC_EVDEV_FILE)).unwrap();
    }
    std::fs::write(directory.path().join("hid.json"), common::control_bytes(common::HID_FILE)).unwrap();
    let profiles = imanlib::controls::load(directory.path(), None, None).unwrap();
    assert!(profiles.evdev.is_empty());
    assert_eq!(profiles.conflicts.len(), 1);
    let fixture = Rc::new(RefCell::new(Fixture::default()));
    let mut receivers = Receivers::with_profiles(Devices { fixtures: vec![fixture.clone()] }, profiles.hid);
    arm(&mut receivers, &[fixture.clone()]);
    send(&fixture, 1, 8);

    assert!(receivers.poll(Duration::from_millis(2)).down(Action::LowerFilters));
    assert!(receivers.connected());
}
