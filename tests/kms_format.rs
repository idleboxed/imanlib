// SPDX-License-Identifier: BSD-3-Clause
#![cfg(all(feature = "drm", target_os = "linux"))]

use embedded_graphics::{pixelcolor::Rgb888, prelude::*};
use {
    imanlib::kms::copy_frame,
    imanlib::kms::format::Format,
    imanlib::kms::LinuxDrm,
    imanlib::surface::Surface,
};
use std::io;

fn formats() -> Vec<Format> {
    vec![
        Format::Xrgb8888,
        #[cfg(feature = "rgb565")]
        Format::Rgb565,
    ]
}

#[test]
fn explicit_default_and_legacy_copy_stay_xrgb8888_with_optional_formats_enabled() {
    assert_eq!(Format::default(), Format::Xrgb8888);
    let registration = Format::default().registration().unwrap();
    assert_eq!(registration.fourcc, drm::buffer::DrmFourcc::Xrgb8888);
    assert_eq!((registration.depth, registration.bits_per_pixel), (24, 32));
    let mut frame = Surface::new(1, 1);
    frame.clear(Rgb888::new(0x12, 0x34, 0x56)).unwrap();
    let mut old = [0xaa; 8];
    let mut explicit = old;

    copy_frame(&frame, &mut old, 8).unwrap();
    Format::default().copy_frame(&frame, &mut explicit, 8).unwrap();

    assert_eq!(old, [0x56, 0x34, 0x12, 0, 0xaa, 0xaa, 0xaa, 0xaa]);
    assert_eq!(explicit, old);
}

#[test]
fn packed_rows_preserve_padding_and_bytes_after_the_image() {
    let mut frame = Surface::new(9, 2);
    frame.clear(Rgb888::WHITE).unwrap();
    for format in formats() {
        let row = format.row_bytes(9).unwrap();
        let pitch = row + 4;
        let mut bytes = vec![0xa5; pitch * 3];

        format.copy_frame(&frame, &mut bytes, pitch).unwrap();

        assert_eq!(&bytes[row..pitch], &[0xa5; 4]);
        assert_eq!(&bytes[pitch + row..pitch * 2], &[0xa5; 4]);
        assert!(bytes[pitch * 2..].iter().all(|byte| *byte == 0xa5));
        assert_eq!(&bytes[..row], &bytes[pitch..pitch + row]);
    }
}

#[test]
fn invalid_mappings_are_rejected_without_partial_writes() {
    let frame = Surface::new(9, 2);
    for format in formats() {
        let row = format.row_bytes(9).unwrap();
        let overflow_pitch = usize::MAX & !3;
        for (pitch, length) in [
            (row, row * 2 - 1), (row - 1, row * 2), (0, row * 2), (overflow_pitch, 64),
        ] {
            let mut bytes = vec![0xa5; length];

            let error = format.copy_frame(&frame, &mut bytes, pitch).unwrap_err();

            assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
            assert!(bytes.iter().all(|byte| *byte == 0xa5));
        }
    }
}

#[test]
fn colour_formats_reject_non_pixel_aligned_pitch() {
    let frame = Surface::new(1, 1);
    for format in formats() {
        let mut bytes = [0xa5; 16];
        let pitch = format.row_bytes(1).unwrap() + 1;

        let error = format.copy_frame(&frame, &mut bytes, pitch).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(bytes, [0xa5; 16]);
    }
}

#[test]
fn row_size_rejects_zero_and_overflow() {
    for format in formats() {
        for width in [0, usize::MAX] {
            assert_eq!(
                format.row_bytes(width).unwrap_err().kind(),
                io::ErrorKind::InvalidInput,
            );
        }
    }
}

#[test]
fn explicit_formats_keep_primary_node_guards() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("card0");
    std::fs::write(&path, b"unchanged fixture").unwrap();
    for format in formats() {
        let error = LinuxDrm::open_with_format(&path, None, format).err().unwrap();

        assert!(error.to_string().contains("unexpected DRM primary device"));
        assert_eq!(std::fs::read(&path).unwrap(), b"unchanged fixture");
    }
}

#[cfg(feature = "rgb565")]
#[test]
fn rgb565_uses_matching_16_bit_allocation_and_registration() {
    let registration = Format::Rgb565.registration().unwrap();

    assert_eq!(registration.fourcc, drm::buffer::DrmFourcc::Rgb565);
    assert_eq!((registration.depth, registration.bits_per_pixel), (16, 16));
    assert_eq!(Format::Rgb565.row_bytes(1280).unwrap() * 720, 1_843_200);
    assert_eq!(Format::Xrgb8888.row_bytes(1280).unwrap() * 720, 3_686_400);
}

#[cfg(feature = "rgb565")]
#[test]
fn rgb565_all_colours_round_trip_with_little_endian_storage() {
    let mut frame = Surface::new(256, 256);
    frame.draw_iter((0..=u16::MAX).map(|value| {
        let colour = Rgb888::new(
            ((value >> 11) << 3) as u8,
            (((value >> 5) & 63) << 2) as u8,
            ((value & 31) << 3) as u8,
        );
        Pixel(Point::new(i32::from(value % 256), i32::from(value / 256)), colour)
    })).unwrap();
    let mut bytes = vec![0; 256 * 256 * 2];

    Format::Rgb565.copy_frame(&frame, &mut bytes, 512).unwrap();

    for (expected, bytes) in (0..=u16::MAX).zip(bytes.chunks_exact(2)) {
        assert_eq!(bytes, expected.to_le_bytes());
    }
}

#[cfg(feature = "rgb565")]
#[test]
fn rgb565_discards_low_colour_bits_without_channel_bleed() {
    let mut frame = Surface::new(2, 1);
    frame.draw_iter([
        Pixel(Point::new(0, 0), Rgb888::new(7, 3, 7)),
        Pixel(Point::new(1, 0), Rgb888::WHITE),
    ]).unwrap();
    let mut bytes = [0xaa; 4];

    Format::Rgb565.copy_frame(&frame, &mut bytes, 4).unwrap();

    assert_eq!(bytes, [0, 0, 0xff, 0xff]);
}

#[test]
fn gui_scanout_follows_build_features() {
    #[cfg(feature = "rgb565")]
    assert_eq!(Format::configured(), Format::Rgb565);
    #[cfg(not(feature = "rgb565"))]
    assert_eq!(Format::configured(), Format::Xrgb8888);
}

#[test]
fn row_size_checks_byte_overflow_without_intermediate_bit_overflow() {
    for format in formats() {
        let pixel_bytes = format.bits_per_pixel() / 8;
        let max_width = usize::MAX / pixel_bytes;

        assert_eq!(format.row_bytes(max_width).unwrap(), max_width * pixel_bytes);
        assert_eq!(format.row_bytes(max_width + 1).unwrap_err().kind(), io::ErrorKind::InvalidInput);
    }
}
