// SPDX-License-Identifier: BSD-3-Clause
#![cfg(all(feature = "drm", target_os = "linux"))]

use embedded_graphics::{pixelcolor::Rgb888, prelude::*};
use {
    imanlib::kms::copy_frame,
    imanlib::kms::Backend,
    imanlib::kms::Kms,
    imanlib::surface::Surface,
};
use std::{cell::RefCell, io, rc::Rc};

#[derive(Default)]
struct State {
    calls: Vec<&'static str>,
    failures: Vec<&'static str>,
    master_errno: Option<i32>,
    blank_frames: Vec<bool>,
}
struct Fake(Rc<RefCell<State>>);
impl Fake {
    fn call(&self, name: &'static str) -> io::Result<()> {
        let mut state = self.0.borrow_mut();
        state.calls.push(name);
        if state.failures.contains(&name) {
            Err(io::Error::other(name))
        } else {
            Ok(())
        }
    }
}
impl Backend for Fake {
    fn acquire_master(&mut self) -> io::Result<()> {
        self.call("acquire")?;
        self.0
            .borrow()
            .master_errno
            .map_or(Ok(()), |n| Err(io::Error::from_raw_os_error(n)))
    }
    fn release_master(&mut self) -> io::Result<()> {
        self.call("release")
    }
    fn select_output(&mut self) -> io::Result<()> {
        self.call("select")
    }
    fn create_buffer(&mut self) -> io::Result<()> {
        self.call("create")
    }
    fn add_framebuffer(&mut self) -> io::Result<()> {
        self.call("add")
    }
    fn write_frame(&mut self, frame: &Surface) -> io::Result<()> {
        self.0.borrow_mut().blank_frames.push(frame.pixels().iter().all(|&pixel| pixel == 0));
        self.call("write")
    }
    fn enable(&mut self) -> io::Result<()> {
        self.call("enable")
    }
    fn verify(&mut self) -> io::Result<()> {
        self.call("verify")
    }
    fn restore(&mut self) -> io::Result<()> {
        self.call("restore")
    }
    fn remove_framebuffer(&mut self) -> io::Result<()> {
        self.call("remove")
    }
    fn destroy_buffer(&mut self) -> io::Result<()> {
        self.call("destroy")
    }
}

#[test]
fn session_verifies_before_drawing_and_restores_before_releasing_resources() {
    let state = Rc::new(RefCell::new(State::default()));
    let frame = Surface::new(2, 2);
    let mut display = Kms::start(Fake(state.clone()), &frame, false).unwrap();

    display.present(&frame).unwrap();
    display.finish().unwrap();
    display.finish().unwrap();
    assert!(display
        .present(&frame)
        .unwrap_err()
        .to_string()
        .contains("closed"));
    drop(display);

    assert_eq!(
        state.borrow().calls,
        [
            "acquire", "select", "create", "add", "write", "enable", "verify", "verify", "write",
            "restore", "remove", "destroy", "release"
        ]
    );
}

#[test]
fn every_partial_start_failure_cleans_only_attempted_resources() {
    for (failure, cleanup) in [
        ("acquire", vec![]),
        ("select", vec!["release"]),
        ("create", vec!["destroy", "release"]),
        ("add", vec!["remove", "destroy", "release"]),
        ("write", vec!["remove", "destroy", "release"]),
        ("enable", vec!["restore", "remove", "destroy", "release"]),
        ("verify", vec!["restore", "remove", "destroy", "release"]),
    ] {
        let state = Rc::new(RefCell::new(State {
            failures: vec![failure],
            ..Default::default()
        }));

        assert!(Kms::start(Fake(state.clone()), &Surface::new(2, 2), false).is_err());

        let state = state.borrow();
        let failed = state.calls.iter().position(|c| *c == failure).unwrap();
        assert_eq!(state.calls[failed + 1..], cleanup, "{failure}");
    }
}

#[test]
fn only_explicit_vendor_einval_continues_without_claiming_master() {
    for (errno, allow, success) in [
        (libc::EINVAL, false, false),
        (libc::EINVAL, true, true),
        (libc::EACCES, true, false),
        (libc::EBUSY, true, false),
        (libc::EPERM, true, false),
    ] {
        let state = Rc::new(RefCell::new(State {
            master_errno: Some(errno),
            ..Default::default()
        }));

        let display = Kms::start(Fake(state.clone()), &Surface::new(1, 1), allow);
        assert_eq!(display.is_ok(), success);
        drop(display);

        assert!(!state.borrow().calls.contains(&"release"));
        assert_eq!(state.borrow().calls.contains(&"verify"), success);
    }
}

#[test]
fn cleanup_reports_all_failures_without_repeating_them_on_drop() {
    let state = Rc::new(RefCell::new(State::default()));
    let mut display = Kms::start(Fake(state.clone()), &Surface::new(1, 1), false).unwrap();
    state.borrow_mut().failures = vec!["restore", "remove", "destroy", "release"];

    let error = display.finish().unwrap_err().to_string();
    let count = state.borrow().calls.len();
    drop(display);

    for name in ["restore", "remove", "destroy", "release"] {
        assert!(error.contains(name));
    }
    assert_eq!(state.borrow().calls.len(), count);
}

#[test]
fn changed_scanout_aborts_before_another_frame_write() {
    let state = Rc::new(RefCell::new(State::default()));
    let mut display = Kms::start(Fake(state.clone()), &Surface::new(1, 1), false).unwrap();
    state.borrow_mut().failures = vec!["verify"];

    assert!(display.present(&Surface::new(1, 1)).is_err());
    drop(display);

    assert_eq!(
        state
            .borrow()
            .calls
            .iter()
            .filter(|c| **c == "write")
            .count(),
        1
    );
    assert_eq!(
        &state.borrow().calls[state.borrow().calls.len() - 4..],
        ["restore", "remove", "destroy", "release"]
    );
}

#[test]
fn drop_cleans_an_unfinished_session() {
    let state = Rc::new(RefCell::new(State::default()));

    drop(Kms::start(Fake(state.clone()), &Surface::new(1, 1), false).unwrap());

    assert!(state
        .borrow()
        .calls
        .ends_with(&["restore", "remove", "destroy", "release"]));
}

#[test]
fn idle_checks_do_not_upload_pixels() {
    let state = Rc::new(RefCell::new(State::default()));
    let mut display = Kms::start(Fake(state.clone()), &Surface::new(1, 1), false).unwrap();

    display.check_scanout().unwrap();
    display.check_scanout().unwrap();
    display.finish().unwrap();
    assert!(display.check_scanout().is_err());

    assert_eq!(
        state
            .borrow()
            .calls
            .iter()
            .filter(|c| **c == "write")
            .count(),
        1
    );
    assert_eq!(
        state
            .borrow()
            .calls
            .iter()
            .filter(|c| **c == "verify")
            .count(),
        3
    );
}

#[test]
fn xrgb_rows_preserve_padding_and_extra_mapping_bytes() {
    let mut surface = Surface::new(2, 2);
    surface.clear(Rgb888::new(0x12, 0x34, 0x56)).unwrap();
    let mut destination = [0xaa; 28];

    copy_frame(&surface, &mut destination, 12).unwrap();

    assert_eq!(
        &destination[..8],
        &[0x56, 0x34, 0x12, 0, 0x56, 0x34, 0x12, 0]
    );
    assert_eq!(&destination[12..20], &destination[..8]);
    assert_eq!(&destination[8..12], &[0xaa; 4]);
    assert_eq!(&destination[20..], &[0xaa; 8]);
}

#[test]
fn invalid_pitch_size_and_overflow_fail_before_touching_pixels() {
    let frame = Surface::new(2, 2);
    for (size, pitch) in [(16, 0), (16, 4), (32, 9), (15, 8), (32, usize::MAX - 3)] {
        let mut bytes = vec![0xaa; size];

        let error = copy_frame(&frame, &mut bytes, pitch).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(bytes, vec![0xaa; size]);
    }
}

#[test]
fn prepared_frame_is_uploaded_before_initial_modeset_without_an_extra_present() {
    let state = Rc::new(RefCell::new(State::default()));
    use embedded_graphics::{pixelcolor::Rgb888, prelude::DrawTarget};
    let mut first = Surface::new(1280, 720);
    first.clear(Rgb888::new(10, 20, 30)).unwrap();

    let mut display = Kms::start(Fake(state.clone()), &first, false).unwrap();

    assert_eq!(state.borrow().blank_frames, [false]);
    assert_eq!(state.borrow().calls, ["acquire", "select", "create", "add", "write", "enable", "verify"]);
    display.finish().unwrap();
}
