// SPDX-License-Identifier: BSD-3-Clause
#![cfg(feature = "input")]
#![cfg(target_os = "linux")]
//! Synthetic transports with the recorded HID descriptor, not radio measurements.
mod common;
use {
    imanlib::evdev::linux::Pad,
    imanlib::evdev::linux::Source as EvdevSource,
    imanlib::gamepad::automatic::Automatic,
    imanlib::gamepad::automatic::Backend,
    imanlib::gamepad::profile::Profile,
    imanlib::gamepad::receiver::Receiver,
    imanlib::hid::receiver::Device as HidDevice,
    imanlib::hid::receiver::Info,
    imanlib::hid::receiver::Receivers,
    imanlib::hid::receiver::Source as HidSource,
    imanlib::input::Buttons,
    imanlib::igui::Action,
};
use imanlib::{evdev::{DeviceInfo, Event}, input_metrics::Sample};
use std::{
    cell::RefCell,
    collections::{BTreeMap, VecDeque},
    io,
    path::{Path, PathBuf},
    rc::Rc,
    time::Duration,
};

struct EvdevFixture {
    info: DeviceInfo,
    queue: VecDeque<Event>,
    held: Vec<u16>,
    denied: bool,
    failed: bool,
    interrupted: bool,
    opened: usize,
    closed: usize,
    reads: usize,
    scans: usize,
}
struct EvdevPad {
    fixture: Rc<RefCell<EvdevFixture>>,
    info: DeviceInfo,
}
impl Drop for EvdevPad {
    fn drop(&mut self) { self.fixture.borrow_mut().closed += 1; }
}
impl Pad for EvdevPad {
    fn info(&self) -> &DeviceInfo { &self.info }
    fn read(&mut self) -> io::Result<Option<Event>> {
        let mut fixture = self.fixture.borrow_mut();
        fixture.reads += 1;
        if fixture.failed { return Err(io::Error::other("synthetic evdev disconnect")); }
        if fixture.interrupted { return Err(io::ErrorKind::Interrupted.into()); }
        Ok(fixture.queue.pop_front())
    }
    fn keys_down(&mut self) -> io::Result<Vec<u16>> { Ok(self.fixture.borrow().held.clone()) }
}
struct EvdevDevices(Vec<Rc<RefCell<EvdevFixture>>>);
impl EvdevSource for EvdevDevices {
    fn discover(&mut self) -> io::Result<Vec<PathBuf>> {
        Ok(self.0.iter().rev().map(|fixture| {
            let mut fixture = fixture.borrow_mut();
            fixture.scans += 1;
            PathBuf::from(&fixture.info.path)
        }).collect())
    }
    fn open(&mut self, path: &Path) -> io::Result<Box<dyn Pad>> {
        let fixture = self.0.iter().find(|fixture| Path::new(&fixture.borrow().info.path) == path)
            .ok_or_else(|| io::Error::other("unknown synthetic event path"))?.clone();
        let mut state = fixture.borrow_mut();
        if state.denied { return Err(io::ErrorKind::PermissionDenied.into()); }
        state.opened += 1;
        let info = state.info.clone();
        drop(state);
        Ok(Box::new(EvdevPad { fixture, info }))
    }
}
struct HidFixture {
    generation: PathBuf,
    queue: VecDeque<Vec<u8>>,
    denied: bool,
    failed: bool,
    interrupted: bool,
    opened: usize,
    closed: usize,
    reads: usize,
    scans: usize,
}
struct HidPad {
    fixture: Rc<RefCell<HidFixture>>,
    info: Info,
}
impl Drop for HidPad {
    fn drop(&mut self) { self.fixture.borrow_mut().closed += 1; }
}
impl HidDevice for HidPad {
    fn info(&self) -> &Info { &self.info }
    fn read(&mut self, packet: &mut [u8]) -> io::Result<Option<usize>> {
        let mut fixture = self.fixture.borrow_mut();
        fixture.reads += 1;
        if fixture.failed { return Err(io::Error::other("synthetic hidraw disconnect")); }
        if fixture.interrupted { return Err(io::ErrorKind::Interrupted.into()); }
        Ok(fixture.queue.pop_front().map(|bytes| {
            packet[..bytes.len()].copy_from_slice(&bytes);
            bytes.len()
        }))
    }
}
struct HidDevices(Vec<Rc<RefCell<HidFixture>>>);
impl HidSource for HidDevices {
    fn discover(&mut self) -> io::Result<Vec<PathBuf>> {
        Ok(self.0.iter().enumerate().rev().map(|(index, fixture)| {
            fixture.borrow_mut().scans += 1;
            PathBuf::from(format!("/synthetic/hidraw{index}"))
        }).collect())
    }
    fn open(&mut self, path: &Path) -> io::Result<Box<dyn HidDevice>> {
        let index: usize = path.file_name().unwrap().to_str().unwrap().strip_prefix("hidraw").unwrap().parse().unwrap();
        let fixture = self.0[index].clone();
        let mut state = fixture.borrow_mut();
        if state.denied { return Err(io::ErrorKind::PermissionDenied.into()); }
        state.opened += 1;
        let generation = state.generation.clone();
        drop(state);
        Ok(Box::new(HidPad { fixture, info: Info {
            generation, bus: 3, vendor: 0x2563, product: 0x0555,
            name: "ShanWan Twin USB Joystick".into(),
            descriptor: include_bytes!("fixtures/shanwan-2563-0555.bin").to_vec(),
        } }))
    }
}
type Input = Automatic<EvdevDevices, HidDevices>;
type EvdevStates = Vec<Rc<RefCell<EvdevFixture>>>;
type HidStates = Vec<Rc<RefCell<HidFixture>>>;

fn setup(evdev_count: usize, hid_count: usize) -> (Input, EvdevStates, HidStates) {
    // Intentionally equal VID/PID/names for unrelated devices: only guarded
    // canonical sysfs ancestry is evidence that two nodes represent an alias.
    let evdev: Vec<_> = (0..evdev_count).map(|index| Rc::new(RefCell::new(EvdevFixture {
        info: DeviceInfo {
            path: format!("/synthetic/event{index}"), name: "ShanWan Twin USB Joystick".into(),
            axis_codes: vec![], abs_info: BTreeMap::new(),
            key_codes: vec![21, 28, 30, 45, 48, 54, 103, 105, 106, 108],
            identity: BTreeMap::from([
                ("id/bustype".into(), "0003".into()), ("id/vendor".into(), "2563".into()),
                ("id/product".into(), "0555".into()),
                ("sysfs".into(), format!("/synthetic/devices/ev-pad-{index}/input/input{index}")),
            ]),
        },
        queue: VecDeque::new(), held: vec![], denied: false, failed: false,
        interrupted: false, opened: 0, closed: 0, reads: 0, scans: 0,
    }))).collect();
    let hid: Vec<_> = (0..hid_count).map(|index| Rc::new(RefCell::new(HidFixture {
        generation: PathBuf::from(format!("/synthetic/devices/hid-pad-{index}")),
        queue: VecDeque::new(), denied: false, failed: false, interrupted: false,
        opened: 0, closed: 0, reads: 0, scans: 0,
    }))).collect();
    let mut document: serde_json::Value = serde_json::from_slice(&common::control_bytes(common::BOARD_EVDEV_FILE)).unwrap();
    document["axes"] = serde_json::json!({});
    document["match"]["vendor"] = "2563".into();
    document["match"]["product"] = "0555".into();
    let profile = Profile::parse(&serde_json::to_vec(&document).unwrap()).unwrap();
    let input = Automatic::new(
        Receiver::with_profiles(EvdevDevices(evdev.clone()), vec![profile]),
        Some(Receivers::new(HidDevices(hid.clone()), common::hid_profile())),
    );
    (input, evdev, hid)
}
fn key(fixture: &Rc<RefCell<EvdevFixture>>, code: u16, value: i32) {
    fixture.borrow_mut().queue.extend([
        Event { kind: 1, code, value }, Event { kind: 0, code: 0, value: 0 },
    ]);
}
fn tap(fixture: &Rc<RefCell<EvdevFixture>>, code: u16) { key(fixture, code, 1); key(fixture, code, 0); }
fn report(fixture: &Rc<RefCell<HidFixture>>, id: u8, buttons: u16) {
    fixture.borrow_mut().queue.push_back(vec![
        id, 128, 128, 128, 128, 15 | ((buttons as u8 & 15) << 4), (buttons >> 4) as u8, 0,
    ]);
}
fn hid_tap(fixture: &Rc<RefCell<HidFixture>>, id: u8, buttons: u16) { report(fixture, id, buttons); report(fixture, id, 0); }
fn poll(input: &mut Input, ms: u64) -> Buttons { input.poll(Duration::from_millis(ms)) }
fn prepare(input: &mut Input, fixtures: &[Rc<RefCell<HidFixture>>]) {
    for fixture in fixtures { report(fixture, 1, 0); report(fixture, 2, 0); }
    assert!(!poll(input, 0).any());
    assert!(!poll(input, 1).any());
    assert_eq!(input.owner(), None);
}
fn diagnostics(input: &Input) -> Sample {
    let mut sample = Sample::default();
    input.update_diagnostics(&mut sample);
    assert!(sample.validate());
    sample
}

#[test]
fn independent_hid_and_evdev_pads_take_turns_without_reopening_or_merging() {
    let (mut input, evdev, hid) = setup(1, 1);
    input.enable_diagnostics();
    assert_eq!(evdev[0].borrow().opened + hid[0].borrow().opened, 0);
    prepare(&mut input, &hid);
    tap(&evdev[0], 30);
    assert!(!poll(&mut input, 2).any());
    assert_eq!(input.owner(), Some(Backend::Evdev));
    assert!(!poll(&mut input, 3).any());
    key(&evdev[0], 108, 1);
    assert!(poll(&mut input, 4).down(Action::Down));
    report(&hid[0], 1, 8);

    assert!(!poll(&mut input, 5).any());

    assert_eq!(input.owner(), Some(Backend::Hid));
    assert!(!poll(&mut input, 6).any());
    assert!(diagnostics(&input).evdev_identity.is_none());
    assert_eq!(diagnostics(&input).channels.iter().filter(|channel| channel.owner).count(), 1);
    report(&hid[0], 1, 0);
    assert!(!poll(&mut input, 7).any());
    hid_tap(&hid[0], 1, 2);
    let buttons = poll(&mut input, 8);
    assert!(buttons.down(Action::Favourite));
    assert!(!buttons.down(Action::Down));
    key(&evdev[0], 108, 0);
    assert!(!poll(&mut input, 9).any());
    tap(&evdev[0], 28);
    assert!(!poll(&mut input, 10).any());
    assert_eq!(input.owner(), Some(Backend::Evdev));
    assert!(!poll(&mut input, 11).any());
    tap(&evdev[0], 30);
    assert!(poll(&mut input, 12).down(Action::Activate));
    let sample = diagnostics(&input);
    assert_eq!(sample.evdev_identity.unwrap().path, "/synthetic/event0");
    assert!(sample.channels.iter().all(|channel| !channel.owner));
    assert_eq!(evdev[0].borrow().opened, 1);
    assert_eq!(hid[0].borrow().opened, 1);
    drop(input);
    assert_eq!(evdev[0].borrow().closed, 1);
    assert_eq!(hid[0].borrow().closed, 1);
}
#[test]
fn simultaneous_backend_requests_choose_hid_once_and_consume_the_winning_hold() {
    let (mut input, evdev, hid) = setup(1, 1);
    prepare(&mut input, &hid);
    key(&evdev[0], 30, 1);
    report(&hid[0], 1, 8);

    assert!(!poll(&mut input, 2).any());

    assert_eq!(input.owner(), Some(Backend::Hid));
    key(&evdev[0], 30, 2);
    report(&hid[0], 1, 8);
    assert!(!poll(&mut input, 3).any());
    assert_eq!(input.owner(), Some(Backend::Hid));
    report(&hid[0], 1, 0);
    assert!(!poll(&mut input, 4).any());
    hid_tap(&hid[0], 1, 2);
    let buttons = poll(&mut input, 5);
    assert!(buttons.down(Action::Favourite));
    assert!(!buttons.down(Action::Activate));
}
#[test]
fn a_working_hid_generation_excludes_only_its_exact_evdev_aliases() {
    let (mut input, evdev, hid) = setup(2, 1);
    let root = hid[0].borrow().generation.clone();
    evdev[0].borrow_mut().info.identity.insert("sysfs".into(), root.join("input/input0").to_string_lossy().into());
    evdev[1].borrow_mut().info.identity.insert("sysfs".into(), format!("{}-other/input/input1", root.display()));
    input.enable_diagnostics();

    prepare(&mut input, &hid);

    assert_eq!(evdev[0].borrow().opened, 1);
    assert_eq!(evdev[0].borrow().closed, 1);
    assert_eq!(evdev[0].borrow().reads, 0);
    assert_eq!(evdev[1].borrow().opened, 1);
    assert_eq!(evdev[1].borrow().closed, 0);
    tap(&evdev[0], 30);
    tap(&evdev[1], 30);
    assert!(!poll(&mut input, 2).any());
    assert_eq!(diagnostics(&input).evdev_identity.unwrap().path, "/synthetic/event1");
    assert!(!poll(&mut input, 3).any());
    tap(&evdev[1], 48);
    assert!(poll(&mut input, 4).down(Action::Favourite));
    assert_eq!(evdev[0].borrow().reads, 0);
}
#[test]
fn vid_pid_and_name_without_verified_sysfs_do_not_guess_an_alias() {
    let (mut input, evdev, hid) = setup(1, 1);
    evdev[0].borrow_mut().info.identity.remove("sysfs");

    prepare(&mut input, &hid);

    assert_eq!(evdev[0].borrow().opened, 1);
    assert_eq!(evdev[0].borrow().closed, 0);
    tap(&evdev[0], 30);
    assert!(!poll(&mut input, 2).any());
    assert_eq!(input.owner(), Some(Backend::Evdev));
}
#[test]
fn late_hid_alias_does_not_evict_active_evdev_and_unblocking_discards_old_hid_input() {
    let (mut input, evdev, hid) = setup(1, 2);
    hid[0].borrow_mut().denied = true;
    let root = hid[0].borrow().generation.clone();
    evdev[0].borrow_mut().info.identity.insert("sysfs".into(), root.join("input/input0").to_string_lossy().into());
    prepare(&mut input, &hid);
    tap(&evdev[0], 30);
    assert!(!poll(&mut input, 2).any());
    assert!(!poll(&mut input, 3).any());
    hid[0].borrow_mut().denied = false;

    assert!(!poll(&mut input, 2000).any());

    assert_eq!(input.owner(), Some(Backend::Evdev));
    assert_eq!(evdev[0].borrow().closed, 0);
    report(&hid[0], 1, 8);
    key(&evdev[0], 30, 1);
    assert!(poll(&mut input, 2001).down(Action::Activate));
    assert_eq!(input.owner(), Some(Backend::Evdev));
    assert!(diagnostics(&input).channels.iter().all(|channel| !channel.owner));
    hid_tap(&hid[1], 1, 2);
    assert!(!poll(&mut input, 2002).any());
    assert_eq!(input.owner(), Some(Backend::Hid));
    // Queued taps and an old hold from the blocked alias cannot steal the menu
    // when its representation becomes eligible after the preceding handoff.
    hid_tap(&hid[0], 1, 8);
    report(&hid[0], 1, 8);
    assert!(!poll(&mut input, 2003).any());
    assert_eq!(evdev[0].borrow().closed, 1);
    assert_eq!(diagnostics(&input).channels.iter().find(|channel| channel.owner).unwrap().generation, 1);
    report(&hid[0], 1, 8);
    assert!(!poll(&mut input, 2004).any());
    report(&hid[0], 1, 0);
    assert!(!poll(&mut input, 2005).any());
    hid_tap(&hid[0], 1, 8);
    assert!(!poll(&mut input, 2006).any());
    let owner = diagnostics(&input).channels.into_iter().find(|channel| channel.owner).unwrap();
    assert_eq!(owner.generation, 2);
    assert!(!poll(&mut input, 2007).any());
    hid_tap(&hid[0], 1, 2);
    assert!(poll(&mut input, 2008).down(Action::Favourite));
}
#[test]
fn lost_hid_generation_reopens_evdev_with_snapshot_neutral_and_fresh_press_gates() {
    let (mut input, evdev, hid) = setup(1, 1);
    let root = hid[0].borrow().generation.clone();
    evdev[0].borrow_mut().info.identity.insert("sysfs".into(), root.join("input/input0").to_string_lossy().into());
    prepare(&mut input, &hid);
    hid_tap(&hid[0], 1, 8);
    assert!(!poll(&mut input, 2).any());
    assert!(!poll(&mut input, 3).any());
    hid[0].borrow_mut().failed = true;
    hid[0].borrow_mut().denied = true;

    assert!(!poll(&mut input, 4).any());

    assert_eq!(input.owner(), None);
    evdev[0].borrow_mut().held = vec![30];
    tap(&evdev[0], 28);
    assert!(!poll(&mut input, 2000).any());
    assert!(!poll(&mut input, 2001).any());
    assert_eq!(input.owner(), None);
    key(&evdev[0], 30, 0);
    assert!(!poll(&mut input, 2002).any());
    key(&evdev[0], 30, 1);
    assert!(!poll(&mut input, 2003).any());
    assert_eq!(input.owner(), Some(Backend::Evdev));
    key(&evdev[0], 30, 0);
    assert!(!poll(&mut input, 2004).any());
    tap(&evdev[0], 30);
    assert!(poll(&mut input, 2005).down(Action::Activate));
}
#[test]
fn combined_fd_and_read_budgets_are_bounded_and_drop_releases_both_backends() {
    let (mut input, evdev, hid) = setup(12, 12);
    for fixture in &evdev { fixture.borrow_mut().interrupted = true; }
    for fixture in &hid { fixture.borrow_mut().interrupted = true; }

    assert!(!poll(&mut input, 0).any());

    assert_eq!(input.owner(), None);
    let evdev_opened: usize = evdev.iter().map(|fixture| fixture.borrow().opened).sum();
    let hid_opened: usize = hid.iter().map(|fixture| fixture.borrow().opened).sum();
    assert_eq!((evdev_opened, hid_opened), (8, 8));
    let reads: usize = evdev.iter().map(|fixture| fixture.borrow().reads).sum::<usize>()
        + hid.iter().map(|fixture| fixture.borrow().reads).sum::<usize>();
    assert_eq!(reads, 16 * 256);
    assert!(evdev.iter().all(|fixture| fixture.borrow().scans == 1));
    assert!(hid.iter().all(|fixture| fixture.borrow().scans == 1));
    assert!(!poll(&mut input, 100).any());
    assert!(evdev.iter().all(|fixture| fixture.borrow().scans == 1));
    assert!(hid.iter().all(|fixture| fixture.borrow().scans == 1));
    drop(input);
    assert_eq!(evdev.iter().map(|fixture| fixture.borrow().closed).sum::<usize>(), 8);
    assert_eq!(hid.iter().map(|fixture| fixture.borrow().closed).sum::<usize>(), 8);
}
