// SPDX-License-Identifier: BSD-3-Clause
use std::convert::Infallible;

use embedded_graphics::{pixelcolor::Rgb888, prelude::*, primitives::Rectangle};

/// Logical canvas used by the manager and GUI scanout.
pub const WIDTH: u32 = 1280;
pub const HEIGHT: u32 = 720;

/// CPU surface shared by the desktop adapter and headless visual checks.
/// A board-specific DrawTarget can replace this without changing navigation.
pub struct Surface {
    width: u32,
    height: u32,
    pixels: Vec<u32>,
}

impl Surface {
    pub fn new(width: u32, height: u32) -> Self {
        assert!(width > 0 && height > 0 && width <= 4096 && height <= 4096);
        Self {
            width,
            height,
            pixels: vec![0; width as usize * height as usize],
        }
    }
    pub fn pixels(&self) -> &[u32] {
        &self.pixels
    }
    pub fn width(&self) -> u32 {
        self.width
    }
    pub fn height(&self) -> u32 {
        self.height
    }
    /// Copy an already prepared packed image, clipping both source and destination.
    /// Row copies avoid re-scaling, colour conversion and per-pixel bounds checks.
    pub fn blit(&mut self, source: &Surface, position: Point) {
        // Widen before adding dimensions: a completely offscreen i32 endpoint
        // must be a no-op, not overflow in Rectangle::bottom_right().
        let x = i64::from(position.x);
        let y = i64::from(position.y);
        let left = x.max(0);
        let top = y.max(0);
        let right = (x + i64::from(source.width)).min(i64::from(self.width));
        let bottom = (y + i64::from(source.height)).min(i64::from(self.height));
        if left >= right || top >= bottom {
            return;
        }
        let width = (right - left) as usize;
        for row in top..bottom {
            let from = (row - y) as usize * source.width as usize + (left - x) as usize;
            let to = row as usize * self.width as usize + left as usize;
            self.pixels[to..to + width].copy_from_slice(&source.pixels[from..from + width]);
        }
    }

    pub fn rgb_bytes(&self) -> Vec<u8> {
        self.pixels
            .iter()
            .flat_map(|p| [(p >> 16) as u8, (p >> 8) as u8, *p as u8])
            .collect()
    }
}

fn pack(color: Rgb888) -> u32 {
    (u32::from(color.r()) << 16) | (u32::from(color.g()) << 8) | u32::from(color.b())
}

impl OriginDimensions for Surface {
    fn size(&self) -> Size {
        Size::new(self.width, self.height)
    }
}

impl DrawTarget for Surface {
    type Color = Rgb888;
    type Error = Infallible;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Rgb888>>,
    {
        for Pixel(point, color) in pixels {
            if point.x >= 0
                && point.y >= 0
                && point.x < self.width as i32
                && point.y < self.height as i32
            {
                self.pixels[point.y as usize * self.width as usize + point.x as usize] =
                    pack(color);
            }
        }
        Ok(())
    }

    fn clear(&mut self, color: Rgb888) -> Result<(), Self::Error> {
        self.pixels.fill(pack(color));
        Ok(())
    }

    fn fill_solid(&mut self, area: &Rectangle, color: Rgb888) -> Result<(), Self::Error> {
        let clipped = area.intersection(&self.bounding_box());
        let value = pack(color);
        for row in clipped.rows() {
            let start = row as usize * self.width as usize + clipped.top_left.x as usize;
            self.pixels[start..start + clipped.size.width as usize].fill(value);
        }
        Ok(())
    }

    fn fill_contiguous<I>(&mut self, area: &Rectangle, colors: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Rgb888>,
    {
        // Consume source pixels even when clipped, preserving their row alignment.
        self.draw_iter(
            area.points()
                .zip(colors)
                .map(|(point, color)| Pixel(point, color)),
        )
    }
}
