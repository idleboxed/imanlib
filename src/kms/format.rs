// SPDX-License-Identifier: BSD-3-Clause
//! Pixel packing is independent of DRM ownership, routing and page flips.
//! The GUI selects its scanout format at build time; explicit callers keep control.
use std::io;

use drm::buffer::DrmFourcc;

use crate::surface::Surface;

#[cfg(feature = "rgb565")]
mod rgb565;
mod xrgb8888;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Format {
    #[default]
    Xrgb8888,
    #[cfg(feature = "rgb565")]
    Rgb565,
}

/// Consistent parameters for CREATE_DUMB and legacy ADDFB.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Registration {
    pub fourcc: DrmFourcc,
    pub depth: u32,
    pub bits_per_pixel: u32,
}

impl Format {
    /// Build-time scanout selection: RGB565 when enabled, otherwise XRGB8888.
    pub fn configured() -> Self {
        #[cfg(feature = "rgb565")]
        {
            Self::Rgb565
        }
        #[cfg(not(feature = "rgb565"))]
        {
            Self::Xrgb8888
        }
    }

    /// API support, not a promise that the selected driver/plane accepts it.
    pub fn registration(self) -> io::Result<Registration> {
        match self {
            Self::Xrgb8888 => Ok(xrgb8888::REGISTRATION),
            #[cfg(feature = "rgb565")]
            Self::Rgb565 => Ok(rgb565::REGISTRATION),
        }
    }

    /// Return the storage width of the selected DRM pixel format.
    pub fn bits_per_pixel(self) -> usize {
        match self {
            Self::Xrgb8888 => 32,
            #[cfg(feature = "rgb565")]
            Self::Rgb565 => 16,
        }
    }

    /// Row size without driver padding; both formats use whole-byte pixels.
    pub fn row_bytes(self, width: usize) -> io::Result<usize> {
        if width == 0 {
            return Err(invalid_layout());
        }
        width
            .checked_mul(self.bits_per_pixel() / 8)
            .ok_or_else(invalid_layout)
    }

    /// Require a row pitch large enough for the selected width and pixel format.
    pub(super) fn validate_pitch(self, width: usize, pitch: usize) -> io::Result<usize> {
        let row_bytes = self.row_bytes(width)?;
        let alignment = self.bits_per_pixel() / 8;
        if pitch < row_bytes || pitch % alignment != 0 {
            return Err(invalid_layout());
        }
        Ok(row_bytes)
    }

    /// Validate the whole mapping before writing. No extra frame allocation;
    /// padding and any bytes beyond the image remain untouched.
    pub fn copy_frame(self, surface: &Surface, destination: &mut [u8], pitch: usize) -> io::Result<()> {
        let row_bytes = self.validate_pitch(surface.width() as usize, pitch)?;
        let required = pitch.checked_mul(surface.height() as usize);
        if required.is_none_or(|bytes| bytes > destination.len()) {
            return Err(invalid_layout());
        }
        for (source, row) in surface
            .pixels()
            .chunks_exact(surface.width() as usize)
            .zip(destination.chunks_exact_mut(pitch))
        {
            let row = &mut row[..row_bytes];
            match self {
                Self::Xrgb8888 => xrgb8888::write_row(source, row),
                #[cfg(feature = "rgb565")]
                Self::Rgb565 => rgb565::write_row(source, row),
            }
        }
        Ok(())
    }
}

fn invalid_layout() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, "invalid DRM mapping or pitch")
}
