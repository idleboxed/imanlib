// SPDX-License-Identifier: BSD-3-Clause
#![cfg(feature = "graphics")]

use embedded_graphics::{pixelcolor::Rgb888, prelude::*};
use imanlib::surface::Surface;

#[test]
fn clipped_blit_preserves_source_offsets_and_outside_pixels() {
    let mut source = Surface::new(2, 2);
    source.draw_iter([
        Pixel(Point::new(0, 0), Rgb888::new(1, 2, 3)),
        Pixel(Point::new(1, 0), Rgb888::new(4, 5, 6)),
        Pixel(Point::new(0, 1), Rgb888::new(7, 8, 9)),
        Pixel(Point::new(1, 1), Rgb888::new(10, 11, 12)),
    ]).unwrap();
    let mut destination = Surface::new(2, 2);
    destination.clear(Rgb888::new(20, 30, 40)).unwrap();

    destination.blit(&source, Point::new(-1, 0));

    assert_eq!(destination.pixels(), [0x040506, 0x141e28, 0x0a0b0c, 0x141e28]);
    assert_eq!(destination.rgb_bytes(), [4, 5, 6, 20, 30, 40, 10, 11, 12, 20, 30, 40]);
}

#[test]
fn fully_offscreen_extreme_coordinates_do_not_change_the_surface() {
    let source = Surface::new(2, 2);
    let mut destination = Surface::new(2, 2);
    destination.clear(Rgb888::new(20, 30, 40)).unwrap();

    destination.blit(&source, Point::new(i32::MIN, i32::MIN));
    destination.blit(&source, Point::new(i32::MAX, i32::MAX));

    assert_eq!(destination.pixels(), [0x141e28; 4]);
}
