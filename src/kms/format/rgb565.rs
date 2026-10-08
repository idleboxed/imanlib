// SPDX-License-Identifier: BSD-3-Clause
use super::{DrmFourcc, Registration};

pub(super) const REGISTRATION: Registration = Registration {
    fourcc: DrmFourcc::Rgb565,
    depth: 16,
    bits_per_pixel: 16,
};

pub(super) fn write_row(source: &[u32], destination: &mut [u8]) {
    for (pixel, bytes) in source.iter().zip(destination.chunks_exact_mut(2)) {
        let packed =
            (((pixel >> 8) & 0xf800) | ((pixel >> 5) & 0x07e0) | ((pixel >> 3) & 0x001f)) as u16;
        bytes.copy_from_slice(&packed.to_le_bytes());
    }
}
