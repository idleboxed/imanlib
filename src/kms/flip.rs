// SPDX-License-Identifier: BSD-3-Clause
//! Two-buffer presentation policy. A completed event, not GETCRTC, permits reuse.
use std::{
    io,
    os::fd::AsRawFd,
    time::{Duration, Instant},
};

use drm::control::{crtc, Device, Event};

use crate::surface::Surface;

/// I/O boundary for the real presenter and device-free fault-injection tests.
/// Slots are always 0 or 1. `write` may allocate the inactive slot lazily.
pub trait FlipIo {
    fn verify(&mut self, slot: usize) -> io::Result<()>;
    fn write(&mut self, slot: usize, frame: &Surface) -> io::Result<()>;
    /// Queue exactly once, with EVENT and without ASYNC. No implicit retries.
    fn queue(&mut self, slot: usize) -> io::Result<()>;
    fn wait(&mut self) -> io::Result<()>;
    /// Restore only our current framebuffer, or accept an unchanged initial CRTC.
    fn restore(&mut self, slot: usize) -> io::Result<()>;
    /// Release only the inactive slot selected by the checked presenter.
    fn discard(&mut self, _slot: usize) -> io::Result<()> {
        Err(io::Error::new(io::ErrorKind::Unsupported, "inactive buffer release unavailable"))
    }
}

/// Slot 0 contains the initial frame installed by SETCRTC before presentation.
#[derive(Default)]
pub struct DoubleBuffer {
    front: usize,
    pending: Option<usize>,
    failed: bool,
    retired: bool,
    completed: u64,
}

impl DoubleBuffer {
    pub fn front(&self) -> usize {
        self.front
    }

    pub fn retired(&self) -> bool {
        self.retired
    }

    pub fn completed(&self) -> u64 {
        self.completed
    }

    pub fn ready(&self) -> io::Result<()> {
        if self.failed || self.pending.is_some() || self.retired {
            return Err(io::Error::other(
                "DRM presenter is failed, pending or retired",
            ));
        }
        Ok(())
    }

    pub fn present(&mut self, io: &mut impl FlipIo, frame: &Surface) -> io::Result<()> {
        self.ready()?;
        let result = (|| {
            io.verify(self.front)?;
            let back = 1 - self.front;
            io.write(back, frame)?;
            // Rendering can take time. Recheck before submitting to a changed CRTC.
            io.verify(self.front)?;
            if let Err(error) = io.queue(back) {
                // An interrupted ioctl is not proof that nothing was queued.
                // Keep both buffers reserved; cleanup may only drain its event.
                if error.kind() == io::ErrorKind::Interrupted {
                    self.pending = Some(back);
                }
                return Err(error);
            }
            self.pending = Some(back);
            self.complete_pending(io)?;
            io.verify(self.front)
        })();
        if result.is_err() {
            self.failed = true;
        }
        result
    }

    /// A retained owner can keep just its visible buffer while a child runs.
    /// Never free either slot after a failed or still-pending page flip.
    pub fn discard_hidden(&mut self, io: &mut impl FlipIo) -> io::Result<()> {
        self.ready()?;
        let result = (|| {
            io.verify(self.front)?;
            io.discard(1 - self.front)?;
            io.verify(self.front)
        })();
        if result.is_err() {
            self.failed = true;
        }
        result
    }

    /// Diagnostic upload without submitting a flip. The visible slot is unchanged.
    #[cfg(feature = "drm-diagnostics")]
    pub fn write_hidden(&mut self, io: &mut impl FlipIo, frame: &Surface) -> io::Result<()> {
        self.ready()?;
        let result = (|| {
            io.verify(self.front)?;
            io.write(1 - self.front, frame)?;
            io.verify(self.front)
        })();
        if result.is_err() {
            self.failed = true;
        }
        result
    }

    fn complete_pending(&mut self, io: &mut impl FlipIo) -> io::Result<()> {
        if let Some(back) = self.pending {
            io.wait()?;
            self.front = back;
            self.pending = None;
            self.completed += 1;
        }
        Ok(())
    }

    /// Cleanup may wait once more for an already queued event, never resubmit.
    /// On timeout/failure keep resources reserved until the DRM FD is closed.
    pub fn finish(&mut self, io: &mut impl FlipIo) -> io::Result<()> {
        if self.retired {
            return Ok(());
        }
        self.failed = true;
        self.complete_pending(io)?;
        io.restore(self.front)?;
        self.retired = true;
        Ok(())
    }
}

/// Bounded wait on the already open nonblocking DRM FD. The drm crate supports
/// old kernels' zero crtc_id via the CRTC ID it puts in page_flip.user_data.
/// Used with regular socket event fixtures in tests, never a real GPU there.
pub fn wait_for_flip(card: &impl Device, crtc: crtc::Handle, timeout: Duration) -> io::Result<()> {
    wait_for_flip_event(card, crtc, timeout).map(|_| ())
}

/// The UAPI timestamp is absolute in the device's event clock, not a wait duration.
/// Do not compare it with userspace Instant without verifying the clock domain.
pub fn wait_for_flip_event(
    card: &impl Device,
    crtc: crtc::Handle,
    timeout: Duration,
) -> io::Result<drm::control::PageFlipEvent> {
    let started = Instant::now();
    loop {
        let remaining = timeout.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "DRM page flip timed out",
            ));
        }
        let mut fd = libc::pollfd {
            fd: card.as_fd().as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        let millis = remaining
            .as_millis()
            .saturating_add(1)
            .min(i32::MAX as u128) as i32;
        // SAFETY: one initialized pollfd, borrowed descriptor, bounded timeout.
        let result = unsafe { libc::poll(&mut fd, 1, millis) };
        if result < 0 {
            return Err(io::Error::last_os_error());
        }
        if fd.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
            return Err(io::Error::other("DRM event descriptor closed or failed"));
        }
        if result == 0 {
            continue;
        }
        match card.receive_events() {
            Ok(events) => {
                if let Some(event) = events.into_iter().find_map(|e| match e {
                    Event::PageFlip(e) if e.crtc == crtc => Some(e),
                    _ => None,
                }) {
                    return Ok(event);
                }
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
            Err(error) => return Err(error),
        }
    }
}
