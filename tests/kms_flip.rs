// SPDX-License-Identifier: BSD-3-Clause
//! Synthetic scanout and socket events only; no display, kernel modeset or SD.
#![cfg(all(feature = "drm", target_os = "linux"))]
use {
    imanlib::kms::flip::wait_for_flip,
    imanlib::kms::flip::DoubleBuffer,
    imanlib::kms::flip::FlipIo,
    imanlib::surface::Surface,
};
use std::{
    io::{self, Write},
    os::{
        fd::{AsFd, BorrowedFd},
        unix::net::UnixStream,
    },
    time::{Duration, Instant},
};

#[derive(Default)]
struct Fake {
    calls: Vec<String>,
    visible: usize,
    pending: Option<usize>,
    fail: Option<&'static str>,
    errno: Option<i32>,
    verifies: usize,
    allocated: [bool; 2],
}
impl Fake {
    fn call(&mut self, name: &'static str, slot: usize) -> io::Result<()> {
        self.calls.push(format!("{name}{slot}"));
        if self.fail == Some(name) {
            return Err(self
                .errno
                .map_or_else(|| io::Error::other(name), io::Error::from_raw_os_error));
        }
        Ok(())
    }
}
impl FlipIo for Fake {
    fn verify(&mut self, slot: usize) -> io::Result<()> {
        self.verifies += 1;
        self.call("verify", slot)?;
        if self.fail == Some("verify-after-write") && self.verifies == 2 {
            return Err(io::Error::other("foreign scanout"));
        }
        assert_eq!(slot, self.visible);
        Ok(())
    }
    fn write(&mut self, slot: usize, _: &Surface) -> io::Result<()> {
        assert_ne!(slot, self.visible, "never overwrite visible scanout");
        assert!(
            self.pending.is_none(),
            "never write before previous flip completion"
        );
        self.call("write", slot)?;
        self.allocated[slot] = true;
        Ok(())
    }
    fn queue(&mut self, slot: usize) -> io::Result<()> {
        assert!(self.pending.is_none());
        self.call("queue", slot)?;
        self.pending = Some(slot);
        Ok(())
    }
    fn wait(&mut self) -> io::Result<()> {
        self.call("wait", self.visible)?;
        self.visible = self.pending.take().unwrap();
        Ok(())
    }
    fn restore(&mut self, slot: usize) -> io::Result<()> {
        assert!(self.pending.is_none());
        assert_eq!(slot, self.visible);
        self.call("restore", slot)
    }
    fn discard(&mut self, slot: usize) -> io::Result<()> {
        assert!(self.pending.is_none());
        assert_ne!(slot, self.visible, "never release the displayed buffer");
        self.call("discard", slot)?;
        self.allocated[slot] = false;
        Ok(())
    }
}

#[test]
fn retained_owner_releases_only_hidden_buffer_and_reallocates_on_next_redraw() {
    let mut state = DoubleBuffer::default();
    let mut io = Fake { allocated: [true, false], ..Default::default() };
    let frame = Surface::new(2, 2);

    for _ in 0..3 {
        state.present(&mut io, &frame).unwrap();
        assert_eq!(io.allocated, [true, true]);
        let front = state.front();
        io.calls.clear();

        state.discard_hidden(&mut io).unwrap();

        assert!(io.allocated[front]);
        assert!(!io.allocated[1 - front]);
        assert_eq!(io.calls, [format!("verify{front}"), format!("discard{}", 1 - front), format!("verify{front}")]);
        assert_eq!(state.front(), front);
    }
    state.finish(&mut io).unwrap();
    assert_eq!(io.calls.last().unwrap(), &format!("restore{}", state.front()));
}

#[test]
fn hidden_release_is_blocked_on_pending_failed_foreign_or_retired_scanout() {
    for failure in ["wait", "queue", "write", "verify"] {
        let mut state = DoubleBuffer::default();
        let mut io = Fake { fail: Some(failure), ..Default::default() };
        assert!(state.present(&mut io, &Surface::new(1, 1)).is_err());
        let calls = io.calls.clone();

        assert!(state.discard_hidden(&mut io).is_err());

        assert_eq!(io.calls, calls);
    }
    let mut state = DoubleBuffer::default();
    let mut io = Fake::default();
    state.finish(&mut io).unwrap();
    let calls = io.calls.clone();
    assert!(state.discard_hidden(&mut io).is_err());
    assert_eq!(io.calls, calls);
}

#[test]
fn hidden_release_failure_or_foreign_owner_stops_without_retrying() {
    for failure in ["discard", "verify"] {
        let mut state = DoubleBuffer::default();
        let mut io = Fake { fail: Some(failure), ..Default::default() };

        assert!(state.discard_hidden(&mut io).is_err());
        let calls = io.calls.clone();
        assert!(state.discard_hidden(&mut io).is_err());
        assert!(state.present(&mut io, &Surface::new(1, 1)).is_err());

        assert_eq!(io.calls, calls);
        assert_eq!(calls.contains(&"discard1".into()), failure == "discard");
    }
}

#[test]
fn alternating_frames_reuse_only_after_event_and_restore_the_last_front() {
    let mut state = DoubleBuffer::default();
    let mut io = Fake::default();
    let frame = Surface::new(2, 2);

    state.present(&mut io, &frame).unwrap();
    state.present(&mut io, &frame).unwrap();
    state.present(&mut io, &frame).unwrap();
    state.finish(&mut io).unwrap();
    state.finish(&mut io).unwrap();

    assert_eq!(state.completed(), 3);
    assert_eq!(state.front(), 1);
    assert!(state.retired());
    assert!(state.present(&mut io, &frame).is_err());
    assert_eq!(
        io.calls,
        [
            "verify0", "write1", "verify0", "queue1", "wait0", "verify1", "verify1", "write0",
            "verify1", "queue0", "wait1", "verify0", "verify0", "write1", "verify0", "queue1",
            "wait0", "verify1", "restore1",
        ]
    );
}

#[test]
fn foreign_owner_before_or_during_copy_prevents_submission() {
    for failure in ["verify", "verify-after-write"] {
        let mut state = DoubleBuffer::default();
        let mut io = Fake {
            fail: Some(failure),
            ..Default::default()
        };

        assert!(state.present(&mut io, &Surface::new(1, 1)).is_err());
        let calls = io.calls.clone();
        assert!(state.present(&mut io, &Surface::new(1, 1)).is_err());

        assert_eq!(io.calls, calls);
        assert!(!calls.iter().any(|calls_item| calls_item.starts_with("queue")));
        assert!(!state.retired());
    }
}

#[test]
fn allocation_or_copy_failure_keeps_initial_front_and_allows_checked_restore() {
    let mut state = DoubleBuffer::default();
    let mut io = Fake {
        fail: Some("write"),
        ..Default::default()
    };

    assert!(state.present(&mut io, &Surface::new(1, 1)).is_err());
    state.finish(&mut io).unwrap();

    assert_eq!(state.front(), 0);
    assert!(state.retired());
    assert_eq!(io.calls, ["verify0", "write1", "restore0"]);
}

#[test]
fn rejected_flip_is_not_retried_or_replaced_with_unsynchronized_writes() {
    for errno in [
        libc::EINVAL,
        libc::ENOSYS,
        libc::EBUSY,
        libc::EPERM,
        libc::EIO,
    ] {
        let mut state = DoubleBuffer::default();
        let mut io = Fake {
            fail: Some("queue"),
            errno: Some(errno),
            ..Default::default()
        };

        assert_eq!(
            state
                .present(&mut io, &Surface::new(1, 1))
                .unwrap_err()
                .raw_os_error(),
            Some(errno)
        );
        assert!(state.present(&mut io, &Surface::new(1, 1)).is_err());
        state.finish(&mut io).unwrap();

        assert_eq!(state.completed(), 0);
        assert!(state.retired());
        assert_eq!(
            io.calls,
            ["verify0", "write1", "verify0", "queue1", "restore0"]
        );
    }
}

#[test]
fn timeout_reserves_both_buffers_until_completion_even_if_readback_would_change() {
    let mut state = DoubleBuffer::default();
    let mut io = Fake {
        fail: Some("wait"),
        errno: Some(libc::ETIMEDOUT),
        ..Default::default()
    };

    assert!(state.present(&mut io, &Surface::new(1, 1)).is_err());
    assert!(state.finish(&mut io).is_err());
    assert!(!state.retired());
    assert_eq!(state.front(), 0);
    assert_eq!(io.pending, Some(1));
    assert!(!io.calls.iter().any(|calls_item| calls_item.starts_with("restore")));
    let count = io.calls.len();
    assert!(state.present(&mut io, &Surface::new(1, 1)).is_err());
    assert_eq!(io.calls.len(), count);

    io.fail = None;
    state.finish(&mut io).unwrap();

    assert_eq!(state.front(), 1);
    assert!(state.retired());
    assert_eq!(
        io.calls.iter().filter(|calls_item| calls_item.starts_with("queue")).count(),
        1
    );
    assert_eq!(io.calls.last().unwrap(), "restore1");
}

#[test]
fn interrupted_submission_is_uncertain_and_requires_an_event_before_restore() {
    let mut state = DoubleBuffer::default();
    let mut io = Fake {
        fail: Some("queue"),
        errno: Some(libc::EINTR),
        ..Default::default()
    };

    assert!(state.present(&mut io, &Surface::new(1, 1)).is_err());
    io.fail = Some("wait");
    io.errno = Some(libc::ETIMEDOUT);
    assert!(state.finish(&mut io).is_err());

    assert!(!state.retired());
    assert!(!io.calls.iter().any(|calls_item| calls_item.starts_with("restore")));
    // Simulate a late event from an ioctl accepted despite interruption.
    io.pending = Some(1);
    io.fail = None;
    state.finish(&mut io).unwrap();
    assert_eq!(io.calls.last().unwrap(), "restore1");
}

#[test]
fn failed_restore_never_authorizes_resource_release() {
    let mut state = DoubleBuffer::default();
    let mut io = Fake::default();
    state.present(&mut io, &Surface::new(1, 1)).unwrap();
    io.fail = Some("restore");

    assert!(state.finish(&mut io).is_err());

    assert!(!state.retired());
    assert!(state.ready().is_err());
}

struct Events(UnixStream);
impl AsFd for Events {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.0.as_fd()
    }
}
impl drm::Device for Events {}
impl drm::control::Device for Events {}
fn handle(id: u32) -> drm::control::crtc::Handle {
    std::num::NonZeroU32::new(id).unwrap().into()
}
fn fixture() -> (Events, UnixStream) {
    let (read, write) = UnixStream::pair().unwrap();
    read.set_nonblocking(true).unwrap();
    (Events(read), write)
}
// Synthetic native-endian DRM_EVENT_FLIP_COMPLETE (32 bytes, uapi drm.h).
fn event(crtc_id: u32, user_data: u64) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend(2_u32.to_ne_bytes());
    bytes.extend(32_u32.to_ne_bytes());
    bytes.extend(user_data.to_ne_bytes());
    for number in [1_u32, 2000, 3, crtc_id] {
        bytes.extend(number.to_ne_bytes());
    }
    bytes
}

#[test]
fn event_wait_accepts_matching_modern_and_legacy_crtc_ids() {
    for crtc_id in [59, 0] {
        let (card, mut writer) = fixture();
        writer.write_all(&event(crtc_id, 59)).unwrap();

        wait_for_flip(&card, handle(59), Duration::from_millis(100)).unwrap();
    }
}

#[test]
fn event_wait_ignores_other_crtcs_and_waits_for_our_completion() {
    let (card, mut writer) = fixture();
    let mut events = event(60, 60);
    events.extend(event(59, 59));
    writer.write_all(&events).unwrap();

    wait_for_flip(&card, handle(59), Duration::from_millis(100)).unwrap();
}

#[test]
fn empty_or_unrelated_events_have_a_bounded_timeout() {
    for other in [false, true] {
        let (card, mut writer) = fixture();
        if other {
            writer.write_all(&event(60, 60)).unwrap();
        }
        let start = Instant::now();

        let error = wait_for_flip(&card, handle(59), Duration::from_millis(15)).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(start.elapsed() < Duration::from_secs(2));
    }
}

#[test]
fn closed_event_descriptor_is_an_error_not_a_successful_flip() {
    let (card, writer) = fixture();
    drop(writer);

    let error = wait_for_flip(&card, handle(59), Duration::from_secs(1)).unwrap_err();

    assert!(error.to_string().contains("descriptor closed or failed"));
}

#[cfg(feature = "drm-diagnostics")]
#[test]
fn diagnostic_upload_only_never_queues_or_changes_the_visible_slot() {
    let mut state = DoubleBuffer::default();
    let mut io = Fake::default();
    let frame = Surface::new(2, 2);

    state.write_hidden(&mut io, &frame).unwrap();
    state.write_hidden(&mut io, &frame).unwrap();
    state.finish(&mut io).unwrap();

    assert_eq!(state.completed(), 0);
    assert_eq!(state.front(), 0);
    assert_eq!(
        io.calls,
        ["verify0", "write1", "verify0", "verify0", "write1", "verify0", "restore0"]
    );
    assert!(state.write_hidden(&mut io, &frame).is_err());
}

#[cfg(feature = "drm-diagnostics")]
#[test]
fn diagnostic_upload_refuses_failed_or_pending_presenters_and_lost_ownership() {
    for failure in ["wait", "verify-after-write"] {
        let mut state = DoubleBuffer::default();
        let mut io = Fake {
            fail: Some(failure),
            ..Default::default()
        };
        let frame = Surface::new(1, 1);

        assert!(state.present(&mut io, &frame).is_err());
        let calls = io.calls.clone();
        assert!(state.write_hidden(&mut io, &frame).is_err());

        assert_eq!(io.calls, calls);
    }
}

#[test]
fn event_wait_preserves_raw_sequence_and_timestamp_for_diagnostics() {
    let (card, mut writer) = fixture();
    writer.write_all(&event(0, 59)).unwrap();

    let error_item = imanlib::kms::flip::wait_for_flip_event(&card, handle(59), Duration::from_millis(100))
        .unwrap();

    assert_eq!(error_item.frame, 3);
    assert_eq!(error_item.duration, Duration::from_micros(1_002_000));
    assert_eq!(error_item.crtc, handle(59));
}
