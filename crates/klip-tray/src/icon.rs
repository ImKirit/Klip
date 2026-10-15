//! Tray icons drawn in code: the Klip. logo, a white bar and a triangle pointing back on a
//! dark square. The triangle shows the state. No image files, so nothing can go missing.

use windows::Win32::UI::WindowsAndMessaging::{CreateIcon, HICON};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    /// The triangle while the buffer runs: the violet of the logo (`#8B5CFF`).
    pub const ACCENT: Rgb = Rgb(0x8B, 0x5C, 0xFF);
    /// The triangle while a recording runs (`#FF3B4E`).
    pub const RECORDING: Rgb = Rgb(0xFF, 0x3B, 0x4E);
    /// Nothing runs: a grey triangle.
    pub const IDLE: Rgb = Rgb(0x6E, 0x6B, 0x80);
    /// The square behind the mark (`#16141F`).
    pub const BACK: Rgb = Rgb(0x16, 0x14, 0x1F);
}

/// Samples per pixel edge for smooth edges.
const SAMPLES: u32 = 4;

// The logo in units of a 512 square, as in assets/logo/klip-logo.svg.
const CORNER: f32 = 120.0;
const BAR: (f32, f32, f32, f32, f32) = (130.0, 112.0, 200.0, 400.0, 35.0);
/// The triangle with its rounded corners, as a polygon.
const TRIANGLE: [(f32, f32); 9] = [
    (236.0, 256.0),
    (360.0, 128.0),
    (380.0, 118.0),
    (398.0, 126.0),
    (404.0, 146.0),
    (404.0, 366.0),
    (398.0, 386.0),
    (380.0, 394.0),
    (360.0, 384.0),
];

fn in_rounded_rect(x: f32, y: f32, l: f32, t: f32, r: f32, b: f32, radius: f32) -> bool {
    if x < l || x > r || y < t || y > b {
        return false;
    }
    let cx = x.clamp(l + radius, r - radius);
    let cy = y.clamp(t + radius, b - radius);
    (x - cx).powi(2) + (y - cy).powi(2) <= radius * radius
}

fn in_triangle(x: f32, y: f32) -> bool {
    let mut inside = false;
    let mut j = TRIANGLE.len() - 1;
    for i in 0..TRIANGLE.len() {
        let (xi, yi) = TRIANGLE[i];
        let (xj, yj) = TRIANGLE[j];
        if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// The logo as `size` x `size` pixels, four bytes each in the order blue, green, red,
/// alpha, alpha not premultiplied. `state` colours the triangle.
pub fn dot_pixels(size: u32, state: Rgb) -> Vec<u8> {
    let mut pixels = vec![0u8; (size * size * 4) as usize];
    let unit = 512.0 / size as f32;
    let n = SAMPLES * SAMPLES;
    for y in 0..size {
        for x in 0..size {
            let (mut r, mut g, mut b, mut covered) = (0u32, 0u32, 0u32, 0u32);
            for sy in 0..SAMPLES {
                for sx in 0..SAMPLES {
                    let px = (x as f32 + (sx as f32 + 0.5) / SAMPLES as f32) * unit;
                    let py = (y as f32 + (sy as f32 + 0.5) / SAMPLES as f32) * unit;
                    if !in_rounded_rect(px, py, 0.0, 0.0, 512.0, 512.0, CORNER) {
                        continue;
                    }
                    covered += 1;
                    let c = if in_rounded_rect(px, py, BAR.0, BAR.1, BAR.2, BAR.3, BAR.4) {
                        Rgb(0xFF, 0xFF, 0xFF)
                    } else if in_triangle(px, py) {
                        state
                    } else {
                        Rgb::BACK
                    };
                    r += c.0 as u32;
                    g += c.1 as u32;
                    b += c.2 as u32;
                }
            }
            let i = ((y * size + x) * 4) as usize;
            if covered > 0 {
                pixels[i] = (b / covered) as u8;
                pixels[i + 1] = (g / covered) as u8;
                pixels[i + 2] = (r / covered) as u8;
                pixels[i + 3] = (covered * 255 / n) as u8;
            }
        }
    }
    pixels
}

/// Builds a Windows icon from the dot. The caller owns the handle and destroys it.
pub(crate) fn create_dot_icon(size: u32, color: Rgb) -> windows::core::Result<HICON> {
    let pixels = dot_pixels(size, color);
    // The AND mask is one bit per pixel in rows padded to 16 bits. With real alpha in
    // the colour data Windows ignores it, but a set bit for every fully transparent pixel
    // keeps the icon correct on paths that do not.
    let stride = size.div_ceil(16) as usize * 2;
    let mut mask = vec![0u8; stride * size as usize];
    for y in 0..size as usize {
        for x in 0..size as usize {
            if pixels[(y * size as usize + x) * 4 + 3] == 0 {
                mask[y * stride + x / 8] |= 0x80 >> (x % 8);
            }
        }
    }
    unsafe { CreateIcon(None, size as i32, size as i32, 1, 32, mask.as_ptr(), pixels.as_ptr()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logo_has_clear_corners_a_white_bar_and_a_state_triangle() {
        let size = 32;
        let pixels = dot_pixels(size, Rgb::RECORDING);
        let at = |x: u32, y: u32| {
            let i = ((y * size + x) * 4) as usize;
            (pixels[i + 2], pixels[i + 1], pixels[i], pixels[i + 3])
        };
        assert_eq!(at(0, 0).3, 0);
        assert_eq!(at(31, 31).3, 0);
        // The middle of the bar is white, the triangle red, the square dark.
        assert_eq!(at(10, 16), (0xFF, 0xFF, 0xFF, 255));
        assert_eq!(at(22, 16), (0xFF, 0x3B, 0x4E, 255));
        assert_eq!(at(16, 3), (0x16, 0x14, 0x1F, 255));
        // Another state, another triangle.
        let idle = dot_pixels(size, Rgb::IDLE);
        let i = ((16 * size + 22) * 4) as usize;
        assert_eq!((idle[i + 2], idle[i + 1], idle[i]), (0x6E, 0x6B, 0x80));
    }

    #[test]
    fn icon_handles_are_created_for_common_sizes() {
        for size in [16, 20, 24, 32] {
            let icon = create_dot_icon(size, Rgb::RECORDING).expect("CreateIcon");
            assert!(!icon.is_invalid());
            unsafe { windows::Win32::UI::WindowsAndMessaging::DestroyIcon(icon).unwrap() };
        }
    }
}
