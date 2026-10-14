// SPDX-License-Identifier: BSD-3-Clause
//! Legacy KMS session with checked routing, scanout ownership and restoration.
//! No atomic KMS, EGL, libdrm, modesetting daemon, mount or persistent writes.

use std::io;

use crate::surface::Surface;

#[cfg(feature = "drm-diagnostics")]
pub mod diagnostic;
pub mod flip;
pub mod format;
mod linux;
pub mod output;
pub use linux::LinuxDrm;

/// Hardware boundary. Cleanup methods must tolerate an unsuccessful creation
/// attempt; implementations retain handles as soon as the kernel returns them.
pub trait Backend {
    /// Request DRM master on the held display device.
    fn acquire_master(&mut self) -> io::Result<()>;
    /// Release DRM master acquired by this session.
    fn release_master(&mut self) -> io::Result<()>;
    /// Choose and validate an exclusive display route.
    fn select_output(&mut self) -> io::Result<()>;
    /// Allocate the checked scanout buffer for the selected route.
    fn create_buffer(&mut self) -> io::Result<()>;
    /// Register the allocated buffer as a DRM framebuffer.
    fn add_framebuffer(&mut self) -> io::Result<()>;
    /// Write a prepared surface into the owned display buffers.
    fn write_frame(&mut self, surface: &Surface) -> io::Result<()>;
    /// Enable the selected route with the registered framebuffer.
    fn enable(&mut self) -> io::Result<()>;
    /// Verify that the selected route still scans out this session's framebuffer.
    fn verify(&mut self) -> io::Result<()>;
    /// Restore only if the selected CRTC still belongs to this session.
    fn restore(&mut self) -> io::Result<()>;
    /// Remove the framebuffer after checked route restoration.
    fn remove_framebuffer(&mut self) -> io::Result<()>;
    /// Release the allocated display buffer after its framebuffer is removed.
    fn destroy_buffer(&mut self) -> io::Result<()>;
}

pub struct Kms<B: Backend> {
    backend: B,
    master: bool,
    buffer: bool,
    framebuffer: bool,
    modeset: bool,
    closed: bool,
}

impl<B: Backend> Kms<B> {
    /// Acquire a checked KMS route and prepare its first frame, cleaning up if initialization fails.
    pub fn start(backend: B, first: &Surface, allow_vendor_einval: bool) -> io::Result<Self> {
        let mut session = Self {
            backend,
            master: false,
            buffer: false,
            framebuffer: false,
            modeset: false,
            closed: false,
        };
        match session.backend.acquire_master() {
            Ok(()) => session.master = true,
            Err(error) if allow_vendor_einval && error.raw_os_error() == Some(libc::EINVAL) => {
                eprintln!("IGUI DRM: SET_MASTER returned EINVAL; master NOT acquired (explicit vendor mode)");
            }
            Err(error) => return Err(error),
        }
        session.backend.select_output()?;
        session.buffer = true;
        session.backend.create_buffer()?;
        session.framebuffer = true;
        session.backend.add_framebuffer()?;
        session.backend.write_frame(first)?;
        // Also inspect ownership after an interrupted/failed modeset attempt.
        session.modeset = true;
        session.backend.enable()?;
        session.backend.verify()?;
        eprintln!(
            "IGUI DRM: scanout verified; master_acquired={}",
            session.master
        );
        Ok(session)
    }

    /// Verify scanout ownership and submit a prepared frame.
    pub fn present(&mut self, surface: &Surface) -> io::Result<()> {
        self.check_scanout()?;
        self.backend.write_frame(surface)
    }

    /// Reject a closed session or a scanout route that no longer belongs to it.
    pub fn check_scanout(&mut self) -> io::Result<()> {
        if self.closed {
            return Err(io::Error::other("DRM session is closed"));
        }
        // Do not keep drawing into an orphaned FB if another process took over.
        self.backend.verify()
    }

    /// Attempts every cleanup step once and reports all failures. No hidden retry.
    pub fn finish(&mut self) -> io::Result<()> {
        if self.closed {
            return Ok(());
        }
        self.closed = true;
        let mut errors = Vec::new();
        for (needed, operation, result) in [
            (
                self.modeset,
                "restore",
                if self.modeset {
                    self.backend.restore()
                } else {
                    Ok(())
                },
            ),
            (
                self.framebuffer,
                "remove-fb",
                if self.framebuffer {
                    self.backend.remove_framebuffer()
                } else {
                    Ok(())
                },
            ),
            (
                self.buffer,
                "destroy-buffer",
                if self.buffer {
                    self.backend.destroy_buffer()
                } else {
                    Ok(())
                },
            ),
            (
                self.master,
                "release-master",
                if self.master {
                    self.backend.release_master()
                } else {
                    Ok(())
                },
            ),
        ] {
            if let Err(error) = result {
                debug_assert!(needed);
                errors.push(format!("{operation}: {error}"));
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(io::Error::other(errors.join("; ")))
        }
    }
}

impl Kms<LinuxDrm> {
    /// Return available KMS route and ownership information without changing the display.
    pub fn diagnostic_state(&self) -> Option<crate::display_metrics::KmsState> {
        self.backend.diagnostic_state(self.master)
    }
}

impl<B: Backend> Drop for Kms<B> {
    fn drop(&mut self) {
        if let Err(error) = self.finish() {
            eprintln!("IGUI DRM cleanup failed: {error}");
        }
    }
}

/// XRGB8888 little-endian, with row padding supplied by the kernel. Validate
/// all dimensions before touching the destination, including arithmetic overflow.
pub fn copy_frame(surface: &Surface, destination: &mut [u8], pitch: usize) -> io::Result<()> {
    format::Format::Xrgb8888.copy_frame(surface, destination, pitch)
}
