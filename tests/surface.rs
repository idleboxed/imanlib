// SPDX-License-Identifier: BSD-3-Clause
#![cfg(feature = "graphics")]

use embedded_graphics::{pixelcolor::Rgb888, prelude::*, primitives::Rectangle};
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
fn surface_clips_primitives_and_preserves_contiguous_source_alignment() {
    let mut surface = Surface::new(3, 2);
    surface
        .fill_solid(
            &Rectangle::new(Point::new(-2, -1), Size::new(4, 3)),
            Rgb888::RED,
        )
        .unwrap();
    assert_eq!(
        surface.pixels(),
        &[0xff0000, 0xff0000, 0, 0xff0000, 0xff0000, 0]
    );
    surface
        .fill_solid(
            &Rectangle::new(Point::new(200, 300), Size::new(4, 3)),
            Rgb888::BLUE,
        )
        .unwrap();
    surface
        .fill_contiguous(
            &Rectangle::new(Point::new(-1, 0), Size::new(3, 2)),
            [
                Rgb888::RED,
                Rgb888::GREEN,
                Rgb888::BLUE,
                Rgb888::BLUE,
                Rgb888::WHITE,
                Rgb888::BLACK,
            ],
        )
        .unwrap();
    assert_eq!(surface.pixels(), &[0x00ff00, 0x0000ff, 0, 0xffffff, 0, 0]);
    assert_eq!(surface.rgb_bytes().len(), 18);
}

#[test]
fn packed_blit_clips_every_edge_and_leaves_other_pixels_untouched() {
    let mut source = Surface::new(3, 2);
    source
        .fill_contiguous(
            &source.bounding_box(),
            [
                Rgb888::RED,
                Rgb888::GREEN,
                Rgb888::BLUE,
                Rgb888::WHITE,
                Rgb888::BLACK,
                Rgb888::YELLOW,
            ],
        )
        .unwrap();
    for position in [
        Point::new(1, 1),
        Point::new(-1, -1),
        Point::new(3, 2),
        Point::new(9, 9),
        Point::new(-9, 0),
        Point::new(i32::MIN, 0),
        Point::new(i32::MAX, 0),
        Point::new(i32::MIN, i32::MIN),
        Point::new(i32::MAX, i32::MAX),
    ] {
        let mut dest = Surface::new(4, 3);
        dest.clear(Rgb888::MAGENTA).unwrap();
        dest.blit(&source, position);
        for position_y in 0..3i32 {
            for position_x in 0..4i32 {
                let sx = i64::from(position_x) - i64::from(position.x);
                let sy = i64::from(position_y) - i64::from(position.y);
                let expected = if (0..3).contains(&sx) && (0..2).contains(&sy) {
                    source.pixels()[sy as usize * 3 + sx as usize]
                } else {
                    0xff00ff
                };
                assert_eq!(dest.pixels()[position_y as usize * 4 + position_x as usize], expected);
            }
        }
    }
}
