//! Tray icons drawn in code: the Klip. logo, a K of round strokes on a violet square,
//! whose dot shows the state. No image files, so nothing can go missing next to the exe.

use windows::Win32::UI::WindowsAndMessaging::{CreateIcon, HICON};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    /// The dot while the buffer runs: white, like the K.
    pub const ACCENT: Rgb = Rgb(0xFF, 0xFF, 0xFF);
    /// The dot while a recording runs (`#FF3B4E`).
    pub const RECORDING: Rgb = Rgb(0xFF, 0x3B, 0x4E);
    /// Nothing runs: the logo without its dot.
    pub const IDLE: Rgb = Rgb(0x9A, 0x99, 0x8F);
    /// The square behind the K (`#8B5CFF`).
    pub const LOGO: Rgb = Rgb(0x8B, 0x5C, 0xFF);
}

/// Samples per pixel edge for smooth edges.
const SAMPLES: u32 = 4;

// The logo in units of a 512 square, as in assets/logo/klip-logo.svg.
const CORNER: f32 = 120.0;
const STROKE: f32 = 33.0;
const SEGMENTS: [((f32, f32), (f32, f32)); 3] = [((158.0, 142.0), (158.0, 370.0)), ((320.0, 142.0), (180.0, 266.0)), ((226.0, 228.0), (322.0, 370.0))];
const DOT: (f32, f32, f32) = (392.0, 370.0, 33.0);

fn in_rounded_square(x: f32, y: f32) -> bool {
    if !(0.0..=512.0).contains(&x) || !(0.0..=512.0).contains(&y) {
        return false;
    }
    let cx = x.clamp(CORNER, 512.0 - CORNER);
    let cy = y.clamp(CORNER, 512.0 - CORNER);
    (x - cx).powi(2) + (y - cy).powi(2) <= CORNER * CORNER
}

fn near_segment(x: f32, y: f32, a: (f32, f32), b: (f32, f32)) -> bool {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let t = (((x - a.0) * dx + (y - a.1) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
    let (px, py) = (a.0 + t * dx, a.1 + t * dy);
    (x - px).powi(2) + (y - py).powi(2) <= STROKE * STROKE
}

/// The logo as `size` x `size` pixels, four bytes each in the order blue, green, red,
/// alpha, alpha not premultiplied. `dot` colours the dot, `Rgb::IDLE` leaves it out.
pub fn dot_pixels(size: u32, dot: Rgb) -> Vec<u8> {
    let mut pixels = vec![0u8; (size * size * 4) as usize];
    let unit = 512.0 / size as f32;
    let n = (SAMPLES * SAMPLES) as u32;
    for y in 0..size {
        for x in 0..size {
            let (mut r, mut g, mut b, mut covered) = (0u32, 0u32, 0u32, 0u32);
            for sy in 0..SAMPLES {
                for sx in 0..SAMPLES {
                    let px = (x as f32 + (sx as f32 + 0.5) / SAMPLES as f32) * unit;
                    let py = (y as f32 + (sy as f32 + 0.5) / SAMPLES as f32) * unit;
                    if !in_rounded_square(px, py) {
                        continue;
                    }
                    covered += 1;
                    let on_dot = dot != Rgb::IDLE && (px - DOT.0).powi(2) + (py - DOT.1).powi(2) <= DOT.2 * DOT.2;
                    let c = if on_dot {
                        dot
                    } else if SEGMENTS.iter().any(|(a, b)| near_segment(px, py, *a, *b)) {
                        Rgb(0xFF, 0xFF, 0xFF)
                    } else {
                        Rgb::LOGO
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
    fn logo_has_clear_corners_a_white_k_and_a_coloured_dot() {
        let size = 32;
        let pixels = dot_pixels(size, Rgb::RECORDING);
        let at = |x: u32, y: u32| {
            let i = ((y * size + x) * 4) as usize;
            (pixels[i + 2], pixels[i + 1], pixels[i], pixels[i + 3])
        };
        assert_eq!(at(0, 0).3, 0);
        assert_eq!(at(31, 31).3, 0);
        // The middle of the stem of the K is white, the dot red, the square violet.
        assert_eq!(at(9, 16), (0xFF, 0xFF, 0xFF, 255));
        assert_eq!(at(24, 23), (0xFF, 0x3B, 0x4E, 255));
        assert_eq!(at(16, 4), (0x8B, 0x5C, 0xFF, 255));
        // Without a state the dot is gone, the square shows through.
        let idle = dot_pixels(size, Rgb::IDLE);
        let i = ((23 * size + 24) * 4) as usize;
        assert_eq!((idle[i + 2], idle[i + 1], idle[i]), (0x8B, 0x5C, 0xFF));
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
