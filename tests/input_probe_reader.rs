// SPDX-License-Identifier: BSD-3-Clause
#![cfg(feature = "input")]
#![cfg(target_os = "linux")]
mod common;
use {
    imanlib::input_probe::linux::usb_inventory,
    imanlib::input_probe::linux::ProfileSource,
    imanlib::input_probe::linux::Pad,
    imanlib::input_probe::linux::Reader,
    imanlib::input_probe::linux::Source,
    imanlib::input_probe::linux::EVENT_BYTES,
    imanlib::input_probe::Probe,
    imanlib::input_probe::Stage,
};
use imanlib::evdev::{DeviceInfo, Event};
use std::{
    cell::RefCell,
    collections::{BTreeMap, VecDeque},
    io,
    path::{Path, PathBuf},
    rc::Rc,
};
#[derive(Default)]
struct State {
    queues: [VecDeque<Event>; 4],
    opened: Vec<usize>,
    closed: Vec<usize>,
    reads: [usize; 4],
    snapshots: [usize; 4],
    held: Vec<u16>,
    noisy: bool,
    failed: bool,
    denied: bool,
    state_error: bool,
}
struct Fixture(Rc<RefCell<State>>);
struct Device {
    index: usize,
    info: DeviceInfo,
    state: Rc<RefCell<State>>,
}
impl Drop for Device {
    fn drop(&mut self) {
        self.state.borrow_mut().closed.push(self.index);
    }
}
fn event(kind: u16, code: u16, value: i32) -> Event {
    Event { kind, code, value }
}
impl Pad for Device {
    fn info(&self) -> &DeviceInfo {
        &self.info
    }
    fn read(&mut self) -> io::Result<Option<Event>> {
        let mut state_item = self.state.borrow_mut();
        state_item.reads[self.index] += 1;
        if state_item.failed && self.index == 0 {
            return Err(io::Error::other("synthetic disconnected"));
        }
        if state_item.noisy && self.index == 0 {
            return Ok(Some(event(3, 1, 123)));
        }
        Ok(state_item.queues[self.index].pop_front())
    }
    fn keys_down(&mut self) -> io::Result<Vec<u16>> {
        let mut state_item = self.state.borrow_mut();
        state_item.snapshots[self.index] += 1;
        if state_item.state_error {
            return Err(io::Error::other("synthetic state failed"));
        }
        Ok(state_item.held.clone())
    }
}
impl Source for Fixture {
    fn discover(&mut self) -> io::Result<Vec<PathBuf>> {
        Ok((0..4)
            .map(|index_item| PathBuf::from(format!("/synthetic/event{index_item}")))
            .collect())
    }
    fn open(&mut self, path: &Path) -> io::Result<Box<dyn Pad>> {
        let index_item = path
            .to_str()
            .unwrap()
            .chars()
            .last()
            .unwrap()
            .to_digit(10)
            .unwrap() as usize;
        if self.0.borrow().denied {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "synthetic permission",
            ));
        }
        self.0.borrow_mut().opened.push(index_item);
        Ok(Box::new(Device {
            index: index_item,
            state: self.0.clone(),
            info: DeviceInfo {
                path: path.to_string_lossy().into_owned(),
                name: "Synthetic receiver".into(),
                axis_codes: vec![0, 1],
                key_codes: vec![21, 28, 103],
                abs_info: BTreeMap::new(),
                identity: BTreeMap::new(),
            },
        }))
    }
}
fn fixture() -> (Rc<RefCell<State>>, Reader<Fixture>, Probe) {
    let state_item = Rc::new(RefCell::new(State::default()));
    (state_item.clone(), Reader::new(Fixture(state_item)), Probe::default())
}
fn tap(state_item: &Rc<RefCell<State>>, slot: usize, code: u16) {
    state_item.borrow_mut().queues[slot].extend([event(1, code, 1), event(0, 0, 0), event(1, code, 0), event(0, 0, 0)]);
}
#[test]
fn kernel_abi_is_native_ulong_not_userspace_timeval() {
    assert_eq!(
        EVENT_BYTES,
        if cfg!(target_pointer_width = "32") {
            16
        } else {
            24
        }
    );
}
#[test]
fn no_acquisition_until_poll_four_devices_are_sampled_and_fds_are_released() {
    let (state_item, mut reader, mut probe) = fixture();
    assert!(state_item.borrow().opened.is_empty());
    reader.poll(&mut probe, 0);
    assert_eq!(state_item.borrow().snapshots, [1; 4]);
    for index_item in 0..4 {
        tap(&state_item, index_item, 21);
    }
    reader.poll(&mut probe, 10);
    assert_eq!(state_item.borrow().opened, vec![0, 1, 2, 3]);
    assert!(probe.streams.iter().flatten().all(|state_item| state_item.events == 2));
    drop(reader);
    assert_eq!(state_item.borrow().closed, vec![0, 1, 2, 3]);
}
#[test]
fn opening_discards_old_queue_and_does_not_treat_current_held_keys_as_presses() {
    let (state_item, mut reader, mut probe) = fixture();
    tap(&state_item, 0, 28);
    state_item.borrow_mut().held = vec![28];
    reader.poll(&mut probe, 0);
    assert_eq!(probe.streams[0].as_ref().unwrap().events, 0);
    assert!(probe.guide.operator.is_none());
    state_item.borrow_mut().queues[0].extend([event(1, 28, 0), event(0, 0, 0)]);
    reader.poll(&mut probe, 10);
    assert!(probe.guide.operator.is_none());
    tap(&state_item, 0, 28);
    reader.poll(&mut probe, 20);
    assert!(probe.guide.operator.is_none());
    tap(&state_item, 0, 28);
    reader.poll(&mut probe, 30);
    assert_eq!(probe.stage, Stage::Player { player: 1 });
}
#[test]
fn a_noisy_device_cannot_starve_other_streams_or_advance_the_guide() {
    let (state_item, mut reader, mut probe) = fixture();
    reader.poll(&mut probe, 0);
    state_item.borrow_mut().noisy = true;
    for index_item in 0..4 {
        tap(&state_item, index_item, 28);
    }
    reader.poll(&mut probe, 10);
    assert_eq!(state_item.borrow().reads, [257, 6, 6, 6]);
    assert_eq!(probe.streams[3].as_ref().unwrap().events, 2);
    assert_eq!(probe.incomplete_polls, 1);
    assert!(probe.guide.operator.is_none());
    assert_eq!(probe.max_overlap, 0);
}
#[test]
fn syn_dropped_discards_until_syn_report_and_queries_current_state() {
    let (state_item, mut reader, mut probe) = fixture();
    reader.poll(&mut probe, 0);
    tap(&state_item, 0, 28);
    reader.poll(&mut probe, 10);
    tap(&state_item, 0, 28);
    state_item.borrow_mut().queues[0].extend([event(0, 3, 0), event(1, 103, 1), event(1, 103, 0)]);
    reader.poll(&mut probe, 20);
    assert!(probe.guide.operator.is_none());
    assert_eq!(state_item.borrow().snapshots[0], 1);
    assert!(!probe.ready(0));
    state_item.borrow_mut().queues[0].push_back(event(0, 0, 0));
    reader.poll(&mut probe, 30);
    assert_eq!(state_item.borrow().snapshots[0], 2);
    assert!(probe.ready(0));
    tap(&state_item, 0, 28);
    reader.poll(&mut probe, 40);
    assert!(probe.guide.operator.is_none());
    tap(&state_item, 0, 28);
    reader.poll(&mut probe, 50);
    assert!(probe.guide.operator.is_some());
}
#[test]
fn partial_packet_or_another_unfinished_device_does_not_confirm() {
    let (state_item, mut reader, mut probe) = fixture();
    reader.poll(&mut probe, 0);
    tap(&state_item, 0, 28);
    reader.poll(&mut probe, 10);
    tap(&state_item, 0, 28);
    state_item.borrow_mut().queues[1].push_back(event(3, 0, 255));
    reader.poll(&mut probe, 20);
    assert!(probe.guide.operator.is_none());
    state_item.borrow_mut().queues[1].push_back(event(0, 0, 0));
    reader.poll(&mut probe, 30);
    assert!(probe.guide.operator.is_none());
}
#[test]
fn disconnect_and_failed_snapshot_close_fds_and_reopen_with_new_generation() {
    let (state_item, mut reader, mut probe) = fixture();
    reader.poll(&mut probe, 0);
    state_item.borrow_mut().failed = true;
    reader.poll(&mut probe, 10);
    assert_eq!(state_item.borrow().closed, vec![0]);
    assert!(probe.streams[0].is_none());
    state_item.borrow_mut().failed = false;
    reader.poll(&mut probe, 2000);
    assert_eq!(probe.streams[0].as_ref().unwrap().generation, 5);
    state_item.borrow_mut().state_error = true;
    state_item.borrow_mut().queues[0].extend([event(0, 3, 0), event(0, 0, 0)]);
    reader.poll(&mut probe, 2010);
    assert!(probe.streams[0].is_none());
    assert_eq!(state_item.borrow().closed, vec![0, 0]);
}
#[test]
fn permission_failures_are_deduplicated_without_relaxing_access() {
    let (state_item, mut reader, mut probe) = fixture();
    state_item.borrow_mut().denied = true;
    for ms in [0, 2000, 4000] {
        reader.poll(&mut probe, ms);
    }
    assert_eq!(probe.connections, 0);
    assert_eq!(
        probe.take_records()
            .iter()
            .filter(|reader| reader["event"] == "device_error")
            .count(),
        4
    );
}
#[test]
fn production_source_refuses_regular_files_symlinks_and_unrelated_char_devices() {
    let root = tempfile::tempdir().unwrap();
    let probe = root.path().join("event0");
    std::fs::write(&probe, b"not an event device").unwrap();
    let mut source = ProfileSource::new(vec![common::evdev_profiles().remove(0)]);
    assert!(source
        .open(&probe)
        .err()
        .unwrap()
        .to_string()
        .contains("character device"));
    let link = root.path().join("event1");
    std::os::unix::fs::symlink(&probe, &link).unwrap();
    assert!(source.open(&link).is_err());
    assert!(source.open(Path::new("/dev/null")).is_err());
    assert_eq!(std::fs::read(probe).unwrap(), b"not an event device");
}
#[test]
fn usb_inventory_is_bounded_omits_serials_and_reports_missing_fields() {
    let root = tempfile::tempdir().unwrap();
    for index_item in 0..18 {
        let directory = root.path().join(format!("1-{index_item}"));
        std::fs::create_dir(&directory).unwrap();
        std::fs::write(directory.join("idVendor"), "20bc\n").unwrap();
        std::fs::write(directory.join("serial"), "private\n").unwrap();
    }
    let directory = root.path().join("1-1:1.0");
    std::fs::create_dir(&directory).unwrap();
    std::fs::write(directory.join("idVendor"), "20bc").unwrap();
    let value = usb_inventory(root.path()).unwrap();
    assert_eq!(value["devices"].as_array().unwrap().len(), 16);
    assert_eq!(value["truncated"], true);
    assert!(!value.to_string().contains("private"));
    assert!(!value.to_string().contains("1-1:1.0"));
    assert!(value.to_string().contains("ERROR:"));
}
