//! Tray icons drawn in code: one dot, the dot of the word mark "Klip.", in the colour of
//! the state. No image files, so nothing can go missing next to the exe.

use windows::Win32::UI::WindowsAndMessaging::{CreateIcon, HICON};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    /// Accent of the design, the status dot for "buffer active" (`#C8F03C`).
    pub const ACCENT: Rgb = Rgb(0xC8, 0xF0, 0x3C);
    /// Reserved for "recording" in the design (`#FF4B3A`).
    pub const RECORDING: Rgb = Rgb(0xFF, 0x4B, 0x3A);
    /// "Text gedämpft" of the design (`#9A998F`), readable on a dark and a light taskbar.
    pub const IDLE: Rgb = Rgb(0x9A, 0x99, 0x8F);
}

/// Share of the icon the dot fills. Leaves the margin other tray icons have.
const DOT_DIAMETER: f32 = 0.72;
/// Samples per pixel edge for the smooth rim.
const SAMPLES: u32 = 4;

/// The dot as `size` x `size` pixels, four bytes each in the order blue, green, red,
/// alpha, alpha not premultiplied. Flat colour, no gradient and no glow (design rule).
pub fn dot_pixels(size: u32, color: Rgb) -> Vec<u8> {
    let mut pixels = vec![0u8; (size * size * 4) as usize];
    let center = size as f32 / 2.0;
    let radius = size as f32 * DOT_DIAMETER / 2.0;
    for y in 0..size {
        for x in 0..size {
            let mut inside = 0;
            for sy in 0..SAMPLES {
                for sx in 0..SAMPLES {
                    let px = x as f32 + (sx as f32 + 0.5) / SAMPLES as f32 - center;
                    let py = y as f32 + (sy as f32 + 0.5) / SAMPLES as f32 - center;
                    if px * px + py * py <= radius * radius {
                        inside += 1;
                    }
                }
            }
            let i = ((y * size + x) * 4) as usize;
            let alpha = (inside * 255 / (SAMPLES * SAMPLES)) as u8;
            pixels[i] = color.2;
            pixels[i + 1] = color.1;
            pixels[i + 2] = color.0;
            pixels[i + 3] = alpha;
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
    fn dot_is_centered_flat_and_has_clear_corners() {
        let size = 16;
        let pixels = dot_pixels(size, Rgb::ACCENT);
        let at = |x: u32, y: u32| {
            let i = ((y * size + x) * 4) as usize;
            (pixels[i + 2], pixels[i + 1], pixels[i], pixels[i + 3])
        };
        assert_eq!(at(8, 8), (0xC8, 0xF0, 0x3C, 255));
        assert_eq!(at(0, 0).3, 0);
        assert_eq!(at(15, 15).3, 0);
        // Symmetric, so the row order Windows expects cannot turn it upside down.
        for y in 0..size {
            for x in 0..size {
                assert_eq!(at(x, y).3, at(size - 1 - x, size - 1 - y).3);
            }
        }
        // One flat colour everywhere, only the alpha varies.
        assert!(pixels.as_chunks::<4>().0.iter().all(|p| p[..3] == [0x3C, 0xF0, 0xC8]));
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
