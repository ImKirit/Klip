//! Regions of the captured frame, input buffers and the small image helpers the
//! detectors work on. Nothing in here touches a window or a process.

use serde::{Deserialize, Serialize};

/// Horizontal anchor of a relative rectangle.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AnchorX {
    Left,
    Center,
    Right,
}

/// Rectangle relative to the frame. Every length is a fraction of the frame
/// HEIGHT, because game HUDs scale with the height and stick to an edge.
///
/// * `Left`: the left edge sits `dx` right of the left frame edge.
/// * `Right`: the right edge sits `dx` left of the right frame edge.
/// * `Center`: the left edge sits `dx` right of the horizontal centre (negative
///   values go left).
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct RelRect {
    pub anchor: AnchorX,
    pub dx: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// Rectangle in pixels of the full frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PixelRect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

impl RelRect {
    /// Resolves to pixels and clamps to the frame. `x_stretch` scales horizontal
    /// lengths (1.0 for an unstretched HUD).
    pub fn resolve(&self, frame_w: u32, frame_h: u32, x_stretch: f32) -> PixelRect {
        let fw = frame_w as f32;
        let fh = frame_h as f32;
        let k = fh * x_stretch;
        let w = self.w * k;
        let left = match self.anchor {
            AnchorX::Left => self.dx * k,
            AnchorX::Right => fw - self.dx * k - w,
            AnchorX::Center => fw / 2.0 + self.dx * k,
        };
        let x0 = left.round().clamp(0.0, fw - 1.0);
        let x1 = (left + w).round().clamp(x0 + 1.0, fw);
        let y0 = (self.y * fh).round().clamp(0.0, fh - 1.0);
        let y1 = ((self.y + self.h) * fh).round().clamp(y0 + 1.0, fh);
        PixelRect { x: x0 as u32, y: y0 as u32, w: (x1 - x0) as u32, h: (y1 - y0) as u32 }
    }
}

/// What a detector asks the recorder for.
#[derive(Clone, Debug, PartialEq)]
pub struct RegionRequest {
    /// Index the recorder echoes back in [`RegionImage::id`].
    pub id: u32,
    pub name: &'static str,
    pub rect: RelRect,
    /// Horizontal stretch to pass to [`RelRect::resolve`].
    pub x_stretch: f32,
    /// Wanted sampling rate in Hz.
    pub rate_hz: f32,
    /// Height the detector scales the full frame to internally. Informational:
    /// a recorder that can downscale on the GPU may deliver that size instead of
    /// full resolution.
    pub work_frame_height: u32,
}

impl RegionRequest {
    pub fn pixel_rect(&self, frame_w: u32, frame_h: u32) -> PixelRect {
        self.rect.resolve(frame_w, frame_h, self.x_stretch)
    }
}

/// A cropped region as BGRA, 8 bit per channel, top row first.
#[derive(Clone, Copy, Debug)]
pub struct RegionImage<'a> {
    pub id: u32,
    pub data: &'a [u8],
    pub width: u32,
    pub height: u32,
    /// Bytes from one row to the next, at least `width * 4`.
    pub stride: usize,
}

impl<'a> RegionImage<'a> {
    /// Borrows a region out of a full BGRA frame without copying.
    pub fn from_frame(id: u32, frame: &'a [u8], frame_w: u32, frame_h: u32, rect: PixelRect) -> Option<RegionImage<'a>> {
        if rect.w == 0 || rect.h == 0 || rect.x + rect.w > frame_w || rect.y + rect.h > frame_h {
            return None;
        }
        let stride = frame_w as usize * 4;
        if frame.len() < stride * frame_h as usize {
            return None;
        }
        let start = rect.y as usize * stride + rect.x as usize * 4;
        Some(RegionImage { id, data: &frame[start..], width: rect.w, height: rect.h, stride })
    }

    fn is_valid(&self) -> bool {
        let w4 = self.width as usize * 4;
        self.width > 0
            && self.height > 0
            && self.stride >= w4
            && self.data.len() >= self.stride * (self.height as usize - 1) + w4
    }
}

/// One sampling step: the regions a detector asked for, cut from the same frame.
#[derive(Clone, Copy, Debug)]
pub struct Sample<'a> {
    pub t_ms: u64,
    /// Size of the full captured frame the regions were cut from.
    pub frame_width: u32,
    pub frame_height: u32,
    pub regions: &'a [RegionImage<'a>],
}

impl<'a> Sample<'a> {
    pub fn region(&self, id: u32) -> Option<&RegionImage<'a>> {
        self.regions.iter().find(|r| r.id == id)
    }
}

/// Small owned RGB image the detectors analyse.
#[derive(Clone, Debug, Default)]
pub struct RgbImage {
    pub width: usize,
    pub height: usize,
    /// R, G, B per pixel, row by row.
    pub data: Vec<u8>,
}

impl RgbImage {
    #[inline]
    pub fn px(&self, x: usize, y: usize) -> [u8; 3] {
        let i = (y * self.width + x) * 3;
        [self.data[i], self.data[i + 1], self.data[i + 2]]
    }
}

/// Resamples a BGRA region into `out` with a box filter (area average when
/// shrinking, nearest neighbour when enlarging). Returns false for a malformed
/// input, `out` is then left empty.
pub fn resample_bgra(src: &RegionImage<'_>, dst_w: usize, dst_h: usize, out: &mut RgbImage) -> bool {
    out.width = 0;
    out.height = 0;
    out.data.clear();
    if !src.is_valid() || dst_w == 0 || dst_h == 0 {
        return false;
    }
    let sw = src.width as usize;
    let sh = src.height as usize;
    out.width = dst_w;
    out.height = dst_h;
    out.data.resize(dst_w * dst_h * 3, 0);

    if sw >= dst_w && sh >= dst_h {
        // Shrinking: walk the source once, every source pixel adds to exactly
        // one target pixel.
        let col_of: Vec<u32> = (0..sw).map(|x| (x * dst_w / sw) as u32).collect();
        let mut col_count = vec![0u32; dst_w];
        for &c in &col_of {
            col_count[c as usize] += 1;
        }
        let mut acc = vec![[0u32; 3]; dst_w];
        let mut sy = 0;
        for y in 0..dst_h {
            acc.iter_mut().for_each(|a| *a = [0; 3]);
            let mut rows = 0u32;
            while sy < sh && sy * dst_h / sh == y {
                let line = &src.data[sy * src.stride..sy * src.stride + sw * 4];
                for (p, &c) in line.chunks_exact(4).zip(&col_of) {
                    let a = &mut acc[c as usize];
                    a[0] += p[2] as u32;
                    a[1] += p[1] as u32;
                    a[2] += p[0] as u32;
                }
                rows += 1;
                sy += 1;
            }
            let orow = &mut out.data[y * dst_w * 3..(y + 1) * dst_w * 3];
            for ((o, a), &n) in orow.chunks_exact_mut(3).zip(&acc).zip(&col_count) {
                let n = (n * rows).max(1);
                o[0] = (a[0] / n) as u8;
                o[1] = (a[1] / n) as u8;
                o[2] = (a[2] / n) as u8;
            }
        }
        return true;
    }

    // Column spans are the same for every row.
    let spans: Vec<(usize, usize)> = (0..dst_w)
        .map(|x| {
            let a = x * sw / dst_w;
            let b = ((x + 1) * sw / dst_w).max(a + 1).min(sw);
            (a.min(sw - 1), b)
        })
        .collect();

    for y in 0..dst_h {
        let y0 = (y * sh / dst_h).min(sh - 1);
        let y1 = ((y + 1) * sh / dst_h).max(y0 + 1).min(sh);
        let rows = y1 - y0;
        let orow = y * dst_w * 3;
        for (x, &(x0, x1)) in spans.iter().enumerate() {
            let mut b = 0u32;
            let mut g = 0u32;
            let mut r = 0u32;
            for sy in y0..y1 {
                let line = &src.data[sy * src.stride + x0 * 4..sy * src.stride + x1 * 4];
                for p in line.chunks_exact(4) {
                    b += p[0] as u32;
                    g += p[1] as u32;
                    r += p[2] as u32;
                }
            }
            let n = (rows * (x1 - x0)) as u32;
            let o = orow + x * 3;
            out.data[o] = (r / n) as u8;
            out.data[o + 1] = (g / n) as u8;
            out.data[o + 2] = (b / n) as u8;
        }
    }
    true
}

/// Converts a rectangle of an NV12 frame (BT.709, limited range) to BGRA so it can
/// be handed to a detector as a [`RegionImage`]. `out` receives `rect.w * rect.h * 4`
/// bytes. Returns false if the planes are too small for the rectangle.
pub fn nv12_region_to_bgra(
    y_plane: &[u8],
    y_stride: usize,
    uv_plane: &[u8],
    uv_stride: usize,
    rect: PixelRect,
    out: &mut Vec<u8>,
) -> bool {
    out.clear();
    let (x0, y0, w, h) = (rect.x as usize, rect.y as usize, rect.w as usize, rect.h as usize);
    if w == 0 || h == 0 {
        return false;
    }
    let last_y = y0 + h - 1;
    if y_plane.len() < last_y * y_stride + x0 + w {
        return false;
    }
    if uv_plane.len() < (last_y / 2) * uv_stride + ((x0 + w - 1) / 2) * 2 + 2 {
        return false;
    }
    out.reserve(w * h * 4);
    for yy in y0..y0 + h {
        let yrow = &y_plane[yy * y_stride..];
        let uvrow = &uv_plane[(yy / 2) * uv_stride..];
        for xx in x0..x0 + w {
            let c = yrow[xx] as i32 - 16;
            let u = uvrow[(xx / 2) * 2] as i32 - 128;
            let v = uvrow[(xx / 2) * 2 + 1] as i32 - 128;
            // BT.709 limited range, fixed point with 10 fractional bits.
            let r = (1192 * c + 1836 * v + 512) >> 10;
            let g = (1192 * c - 218 * u - 546 * v + 512) >> 10;
            let b = (1192 * c + 2163 * u + 512) >> 10;
            out.push(b.clamp(0, 255) as u8);
            out.push(g.clamp(0, 255) as u8);
            out.push(r.clamp(0, 255) as u8);
            out.push(255);
        }
    }
    true
}

/// Hue in degrees (0 to 360), saturation and value (0 to 1) of an RGB pixel.
#[inline]
pub fn rgb_to_hsv(rgb: [u8; 3]) -> (f32, f32, f32) {
    let r = rgb[0] as f32;
    let g = rgb[1] as f32;
    let b = rgb[2] as f32;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    if max <= 0.0 {
        return (0.0, 0.0, 0.0);
    }
    let s = d / max;
    let v = max / 255.0;
    if d <= 0.0 {
        return (0.0, 0.0, v);
    }
    let mut h = if max == r {
        (g - b) / d
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    } * 60.0;
    if h < 0.0 {
        h += 360.0;
    }
    (h, s, v)
}

/// Colour range in HSV. `h_min > h_max` means the range wraps around 360 degrees.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct HsvRange {
    pub h_min: f32,
    pub h_max: f32,
    pub s_min: f32,
    pub s_max: f32,
    pub v_min: f32,
    pub v_max: f32,
}

impl HsvRange {
    #[inline]
    pub fn contains(&self, h: f32, s: f32, v: f32) -> bool {
        let hue_ok = if self.h_min <= self.h_max {
            h >= self.h_min && h <= self.h_max
        } else {
            h >= self.h_min || h <= self.h_max
        };
        hue_ok && s >= self.s_min && s <= self.s_max && v >= self.v_min && v <= self.v_max
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rel_rect_right_anchor_resolves_on_1080p() {
        let r = RelRect { anchor: AnchorX::Right, dx: 0.0, y: 0.0796, w: 0.7130, h: 0.2315 };
        let p = r.resolve(1920, 1080, 1.0);
        assert_eq!(p.x + p.w, 1920);
        assert_eq!(p.w, 770);
        assert_eq!(p.y, 86);
        assert_eq!(p.h, 250);
    }

    #[test]
    fn rel_rect_center_anchor_is_symmetric() {
        let r = RelRect { anchor: AnchorX::Center, dx: -0.1185, y: 0.125, w: 0.237, h: 0.1315 };
        let p = r.resolve(2560, 1440, 1.0);
        let mid = p.x as f32 + p.w as f32 / 2.0;
        assert!((mid - 1280.0).abs() <= 1.0);
    }

    #[test]
    fn rel_rect_is_clamped_to_the_frame() {
        let r = RelRect { anchor: AnchorX::Right, dx: 0.0, y: 0.9, w: 3.0, h: 0.5 };
        let p = r.resolve(640, 480, 1.0);
        assert_eq!(p.x, 0);
        assert_eq!(p.w, 640);
        assert_eq!(p.y + p.h, 480);
    }

    #[test]
    fn resample_averages_blocks() {
        // 4x2 image: left half black, right half white.
        let mut data = Vec::new();
        for _ in 0..2 {
            for x in 0..4 {
                let v = if x < 2 { 0 } else { 255 };
                data.extend_from_slice(&[v, v, v, 255]);
            }
        }
        let src = RegionImage { id: 0, data: &data, width: 4, height: 2, stride: 16 };
        let mut out = RgbImage::default();
        assert!(resample_bgra(&src, 2, 1, &mut out));
        assert_eq!(out.px(0, 0), [0, 0, 0]);
        assert_eq!(out.px(1, 0), [255, 255, 255]);
    }

    #[test]
    fn resample_rejects_short_buffers() {
        let data = [0u8; 8];
        let src = RegionImage { id: 0, data: &data, width: 4, height: 2, stride: 16 };
        let mut out = RgbImage::default();
        assert!(!resample_bgra(&src, 2, 1, &mut out));
        assert!(out.data.is_empty());
    }

    #[test]
    fn region_from_frame_respects_stride() {
        let (w, h) = (8u32, 4u32);
        let mut frame = vec![0u8; (w * h * 4) as usize];
        // Mark pixel (5, 2) red in BGRA.
        let i = (2 * w as usize + 5) * 4;
        frame[i + 2] = 200;
        let r = RegionImage::from_frame(7, &frame, w, h, PixelRect { x: 4, y: 1, w: 3, h: 3 }).unwrap();
        let mut out = RgbImage::default();
        assert!(resample_bgra(&r, 3, 3, &mut out));
        assert_eq!(out.px(1, 1), [200, 0, 0]);
        assert_eq!(out.px(0, 0), [0, 0, 0]);
        assert!(RegionImage::from_frame(7, &frame, w, h, PixelRect { x: 6, y: 0, w: 3, h: 1 }).is_none());
    }

    #[test]
    fn nv12_grey_and_colour() {
        // 2x2 frame, mid grey: Y=126 gives about 128 in full range.
        let y = [126u8; 4];
        let uv = [128u8, 128];
        let mut out = Vec::new();
        assert!(nv12_region_to_bgra(&y, 2, &uv, 2, PixelRect { x: 0, y: 0, w: 2, h: 2 }, &mut out));
        assert_eq!(out.len(), 16);
        assert!((out[0] as i32 - 128).abs() <= 1 && out[0] == out[1] && out[1] == out[2]);
        // Strong V pushes red up and green down.
        let uv = [128u8, 220];
        assert!(nv12_region_to_bgra(&y, 2, &uv, 2, PixelRect { x: 0, y: 0, w: 2, h: 2 }, &mut out));
        assert!(out[2] > out[1] && out[2] > out[0]);
        assert!(!nv12_region_to_bgra(&y, 2, &uv, 2, PixelRect { x: 1, y: 1, w: 2, h: 2 }, &mut out));
    }

    #[test]
    fn hsv_of_measured_colours() {
        let (h, s, v) = rgb_to_hsv([231, 228, 112]);
        assert!((h - 58.5).abs() < 1.5 && s > 0.45 && v > 0.85);
        let (h, _, _) = rgb_to_hsv([94, 177, 164]);
        assert!((h - 170.0).abs() < 3.0);
        let wrap = HsvRange { h_min: 350.0, h_max: 26.0, s_min: 0.2, s_max: 1.0, v_min: 0.3, v_max: 1.0 };
        let (h, s, v) = rgb_to_hsv([237, 104, 84]);
        assert!(wrap.contains(h, s, v));
        assert!(wrap.contains(355.0, 0.5, 0.5));
        assert!(!wrap.contains(100.0, 0.5, 0.5));
    }
}
