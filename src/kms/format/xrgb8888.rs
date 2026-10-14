// SPDX-License-Identifier: BSD-3-Clause
use super::{DrmFourcc, Registration};

pub(super) const REGISTRATION: Registration = Registration {
    fourcc: DrmFourcc::Xrgb8888,
    depth: 24,
    bits_per_pixel: 32,
};

/// Pack one checked source row into little-endian XRGB8888 output.
pub(super) fn write_row(source: &[u32], destination: &mut [u8]) {
    for (pixel, bytes) in source.iter().zip(destination.chunks_exact_mut(4)) {
        bytes.copy_from_slice(&pixel.to_le_bytes());
    }
}
