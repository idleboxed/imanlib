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
fn e(kind: u16, code: u16, value: i32) -> Event {
    Event { kind, code, value }
}
impl Pad for Device {
    fn info(&self) -> &DeviceInfo {
        &self.info
    }
    fn read(&mut self) -> io::Result<Option<Event>> {
        let mut s = self.state.borrow_mut();
        s.reads[self.index] += 1;
        if s.failed && self.index == 0 {
            return Err(io::Error::other("synthetic disconnected"));
        }
        if s.noisy && self.index == 0 {
            return Ok(Some(e(3, 1, 123)));
        }
        Ok(s.queues[self.index].pop_front())
    }
    fn keys_down(&mut self) -> io::Result<Vec<u16>> {
        let mut s = self.state.borrow_mut();
        s.snapshots[self.index] += 1;
        if s.state_error {
            return Err(io::Error::other("synthetic state failed"));
        }
        Ok(s.held.clone())
    }
}
impl Source for Fixture {
    fn discover(&mut self) -> io::Result<Vec<PathBuf>> {
        Ok((0..4)
            .map(|i| PathBuf::from(format!("/synthetic/event{i}")))
            .collect())
    }
    fn open(&mut self, path: &Path) -> io::Result<Box<dyn Pad>> {
        let i = path
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
        self.0.borrow_mut().opened.push(i);
        Ok(Box::new(Device {
            index: i,
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
    let s = Rc::new(RefCell::new(State::default()));
    (s.clone(), Reader::new(Fixture(s)), Probe::default())
}
fn tap(s: &Rc<RefCell<State>>, slot: usize, code: u16) {
    s.borrow_mut().queues[slot].extend([e(1, code, 1), e(0, 0, 0), e(1, code, 0), e(0, 0, 0)]);
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
    let (s, mut r, mut p) = fixture();
    assert!(s.borrow().opened.is_empty());
    r.poll(&mut p, 0);
    assert_eq!(s.borrow().snapshots, [1; 4]);
    for i in 0..4 {
        tap(&s, i, 21);
    }
    r.poll(&mut p, 10);
    assert_eq!(s.borrow().opened, vec![0, 1, 2, 3]);
    assert!(p.streams.iter().flatten().all(|s| s.events == 2));
    drop(r);
    assert_eq!(s.borrow().closed, vec![0, 1, 2, 3]);
}
#[test]
fn opening_discards_old_queue_and_does_not_treat_current_held_keys_as_presses() {
    let (s, mut r, mut p) = fixture();
    tap(&s, 0, 28);
    s.borrow_mut().held = vec![28];
    r.poll(&mut p, 0);
    assert_eq!(p.streams[0].as_ref().unwrap().events, 0);
    assert!(p.guide.operator.is_none());
    s.borrow_mut().queues[0].extend([e(1, 28, 0), e(0, 0, 0)]);
    r.poll(&mut p, 10);
    assert!(p.guide.operator.is_none());
    tap(&s, 0, 28);
    r.poll(&mut p, 20);
    assert!(p.guide.operator.is_none());
    tap(&s, 0, 28);
    r.poll(&mut p, 30);
    assert_eq!(p.stage, Stage::Player { player: 1 });
}
#[test]
fn a_noisy_device_cannot_starve_other_streams_or_advance_the_guide() {
    let (s, mut r, mut p) = fixture();
    r.poll(&mut p, 0);
    s.borrow_mut().noisy = true;
    for i in 0..4 {
        tap(&s, i, 28);
    }
    r.poll(&mut p, 10);
    assert_eq!(s.borrow().reads, [257, 6, 6, 6]);
    assert_eq!(p.streams[3].as_ref().unwrap().events, 2);
    assert_eq!(p.incomplete_polls, 1);
    assert!(p.guide.operator.is_none());
    assert_eq!(p.max_overlap, 0);
}
#[test]
fn syn_dropped_discards_until_syn_report_and_queries_current_state() {
    let (s, mut r, mut p) = fixture();
    r.poll(&mut p, 0);
    tap(&s, 0, 28);
    r.poll(&mut p, 10);
    tap(&s, 0, 28);
    s.borrow_mut().queues[0].extend([e(0, 3, 0), e(1, 103, 1), e(1, 103, 0)]);
    r.poll(&mut p, 20);
    assert!(p.guide.operator.is_none());
    assert_eq!(s.borrow().snapshots[0], 1);
    assert!(!p.ready(0));
    s.borrow_mut().queues[0].push_back(e(0, 0, 0));
    r.poll(&mut p, 30);
    assert_eq!(s.borrow().snapshots[0], 2);
    assert!(p.ready(0));
    tap(&s, 0, 28);
    r.poll(&mut p, 40);
    assert!(p.guide.operator.is_none());
    tap(&s, 0, 28);
    r.poll(&mut p, 50);
    assert!(p.guide.operator.is_some());
}
#[test]
fn partial_packet_or_another_unfinished_device_does_not_confirm() {
    let (s, mut r, mut p) = fixture();
    r.poll(&mut p, 0);
    tap(&s, 0, 28);
    r.poll(&mut p, 10);
    tap(&s, 0, 28);
    s.borrow_mut().queues[1].push_back(e(3, 0, 255));
    r.poll(&mut p, 20);
    assert!(p.guide.operator.is_none());
    s.borrow_mut().queues[1].push_back(e(0, 0, 0));
    r.poll(&mut p, 30);
    assert!(p.guide.operator.is_none());
}
#[test]
fn disconnect_and_failed_snapshot_close_fds_and_reopen_with_new_generation() {
    let (s, mut r, mut p) = fixture();
    r.poll(&mut p, 0);
    s.borrow_mut().failed = true;
    r.poll(&mut p, 10);
    assert_eq!(s.borrow().closed, vec![0]);
    assert!(p.streams[0].is_none());
    s.borrow_mut().failed = false;
    r.poll(&mut p, 2000);
    assert_eq!(p.streams[0].as_ref().unwrap().generation, 5);
    s.borrow_mut().state_error = true;
    s.borrow_mut().queues[0].extend([e(0, 3, 0), e(0, 0, 0)]);
    r.poll(&mut p, 2010);
    assert!(p.streams[0].is_none());
    assert_eq!(s.borrow().closed, vec![0, 0]);
}
#[test]
fn permission_failures_are_deduplicated_without_relaxing_access() {
    let (s, mut r, mut p) = fixture();
    s.borrow_mut().denied = true;
    for ms in [0, 2000, 4000] {
        r.poll(&mut p, ms);
    }
    assert_eq!(p.connections, 0);
    assert_eq!(
        p.take_records()
            .iter()
            .filter(|r| r["event"] == "device_error")
            .count(),
        4
    );
}
#[test]
fn production_source_refuses_regular_files_symlinks_and_unrelated_char_devices() {
    let root = tempfile::tempdir().unwrap();
    let p = root.path().join("event0");
    std::fs::write(&p, b"not an event device").unwrap();
    let mut source = ProfileSource::new(vec![common::evdev_profiles().remove(0)]);
    assert!(source
        .open(&p)
        .err()
        .unwrap()
        .to_string()
        .contains("character device"));
    let link = root.path().join("event1");
    std::os::unix::fs::symlink(&p, &link).unwrap();
    assert!(source.open(&link).is_err());
    assert!(source.open(Path::new("/dev/null")).is_err());
    assert_eq!(std::fs::read(p).unwrap(), b"not an event device");
}
#[test]
fn usb_inventory_is_bounded_omits_serials_and_reports_missing_fields() {
    let root = tempfile::tempdir().unwrap();
    for i in 0..18 {
        let d = root.path().join(format!("1-{i}"));
        std::fs::create_dir(&d).unwrap();
        std::fs::write(d.join("idVendor"), "20bc\n").unwrap();
        std::fs::write(d.join("serial"), "private\n").unwrap();
    }
    let d = root.path().join("1-1:1.0");
    std::fs::create_dir(&d).unwrap();
    std::fs::write(d.join("idVendor"), "20bc").unwrap();
    let value = usb_inventory(root.path()).unwrap();
    assert_eq!(value["devices"].as_array().unwrap().len(), 16);
    assert_eq!(value["truncated"], true);
    assert!(!value.to_string().contains("private"));
    assert!(!value.to_string().contains("1-1:1.0"));
    assert!(value.to_string().contains("ERROR:"));
}
