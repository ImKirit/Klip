//! All monitors in one picture. Every monitor gets a capture of its own, each frame is
//! copied to its place on one large texture (the desktop as Windows lays it out), and that
//! texture goes on to conversion and encoder like the picture of a single monitor.
//!
//! Only monitors on the adapter of the device take part: a texture cannot be copied from
//! one graphics card to another without a detour through system memory.

use crate::capture::{self, Capture, CaptureStats};
use crate::gfx::{Gfx, MonitorInfo};
use crate::Res;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use windows::Win32::Graphics::Direct3D11::{
    ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D, D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_BOX,
    D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};

/// Where each monitor lands on the canvas: its desktop rectangle moved so that the
/// leftmost and topmost monitor edges are at 0. Returns the canvas size too.
pub fn layout(monitors: &[MonitorInfo]) -> (u32, u32, Vec<(i32, i32)>) {
    let left = monitors.iter().map(|m| m.rect.left).min().unwrap_or(0);
    let top = monitors.iter().map(|m| m.rect.top).min().unwrap_or(0);
    let right = monitors.iter().map(|m| m.rect.right).max().unwrap_or(0);
    let bottom = monitors.iter().map(|m| m.rect.bottom).max().unwrap_or(0);
    let places = monitors.iter().map(|m| (m.rect.left - left, m.rect.top - top)).collect();
    // Encoders want even sizes. The extra column or row stays black.
    (((right - left) as u32 + 1) & !1, ((bottom - top) as u32 + 1) & !1, places)
}

struct Canvas {
    texture: ID3D11Texture2D,
    context: ID3D11DeviceContext,
    width: u32,
    height: u32,
}

// SAFETY: the device context is multithread protected (see gfx::create), the texture is a
// plain GPU resource. Both are only used through that protected context.
unsafe impl Send for Canvas {}
unsafe impl Sync for Canvas {}

impl Canvas {
    fn new(device: &ID3D11Device, context: &ID3D11DeviceContext, width: u32, height: u32) -> Res<Self> {
        let desc = D3D11_TEXTURE2D_DESC {
            Width: width,
            Height: height,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        // A new default texture is all zeros: black, which is right for the gaps between
        // monitors of different sizes.
        let mut texture = None;
        unsafe { device.CreateTexture2D(&desc, None, Some(&mut texture))? };
        Ok(Self { texture: texture.ok_or("CreateTexture2D returned nothing")?, context: context.clone(), width, height })
    }

    /// Copies a captured frame to its place, cut to the canvas.
    fn paste(&self, frame: &ID3D11Texture2D, at: (i32, i32), size: (u32, u32)) {
        let (x, y) = (at.0.max(0) as u32, at.1.max(0) as u32);
        if x >= self.width || y >= self.height {
            return;
        }
        let w = size.0.min(self.width - x);
        let h = size.1.min(self.height - y);
        // The frame texture of a capture can be larger than the monitor, only the monitor
        // part is copied.
        let mut fd = D3D11_TEXTURE2D_DESC::default();
        unsafe { frame.GetDesc(&mut fd) };
        let (w, h) = (w.min(fd.Width), h.min(fd.Height));
        let source = D3D11_BOX { left: 0, top: 0, front: 0, right: w, bottom: h, back: 1 };
        unsafe { self.context.CopySubresourceRegion(&self.texture, 0, x, y, 0, frame, 0, Some(&source)) };
    }
}

pub struct Composite {
    /// Kept alive for as long as the picture is wanted. Dropping them ends the captures.
    pub captures: Vec<Capture>,
    pub stats: Arc<CaptureStats>,
    pub width: u32,
    pub height: u32,
}

/// Starts one capture per monitor and calls `on_frame` with the whole canvas at most
/// `fps` times per second, whenever any monitor showed something new. Times are on the
/// grid of the target frame rate, like those of a single capture.
pub fn start<F>(gfx: &Gfx, monitors: &[MonitorInfo], fps: u32, cursor: bool, on_frame: F) -> Res<Composite>
where
    F: Fn(&ID3D11Texture2D, i64) + Send + Sync + 'static,
{
    if monitors.is_empty() {
        return Err("no monitor to record".into());
    }
    let (width, height, places) = layout(monitors);
    let canvas = Arc::new(Canvas::new(&gfx.device, &gfx.context, width, height)?);
    let stats = Arc::new(CaptureStats::default());
    let on_frame = Arc::new(on_frame);
    // One grid for all monitors, so two monitors never hand on two frames for one slot.
    let slot_len: i64 = 10_000_000 / fps.max(1) as i64;
    let anchor = Arc::new(AtomicI64::new(i64::MIN));
    let last_slot = Arc::new(AtomicI64::new(-1));

    let mut captures = Vec::with_capacity(monitors.len());
    for (m, at) in monitors.iter().zip(places) {
        let prepared = capture::prepare(gfx, capture::Source::Monitor(m.monitor))?;
        let size = (prepared.width, prepared.height);
        let (canvas, stats, on_frame) = (canvas.clone(), stats.clone(), on_frame.clone());
        let (anchor, last_slot) = (anchor.clone(), last_slot.clone());
        let cap = capture::start(prepared, fps, cursor, move |frame, time| {
            canvas.paste(frame, at, size);
            stats.arrived.fetch_add(1, Ordering::Relaxed);
            if anchor.load(Ordering::Relaxed) == i64::MIN {
                anchor.store(time, Ordering::Relaxed);
            }
            let base = anchor.load(Ordering::Relaxed);
            let slot = (time - base + slot_len / 2).div_euclid(slot_len);
            // Several capture threads race for a slot, only one may win it.
            let mut seen = last_slot.load(Ordering::Relaxed);
            loop {
                if slot <= seen {
                    return;
                }
                match last_slot.compare_exchange(seen, slot, Ordering::AcqRel, Ordering::Relaxed) {
                    Ok(_) => break,
                    Err(now) => seen = now,
                }
            }
            stats.taken.fetch_add(1, Ordering::Relaxed);
            on_frame(&canvas.texture, base + slot * slot_len);
        })?;
        captures.push(cap);
    }
    Ok(Composite { captures, stats, width, height })
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Foundation::RECT;
    use windows::Win32::Graphics::Gdi::HMONITOR;

    fn mon(left: i32, top: i32, w: i32, h: i32) -> MonitorInfo {
        MonitorInfo {
            adapter: "test".into(),
            monitor: HMONITOR(std::ptr::null_mut()),
            rect: RECT { left, top, right: left + w, bottom: top + h },
        }
    }

    #[test]
    fn side_by_side() {
        let (w, h, at) = layout(&[mon(0, 0, 1920, 1080), mon(1920, 0, 1920, 1080)]);
        assert_eq!((w, h), (3840, 1080));
        assert_eq!(at, vec![(0, 0), (1920, 0)]);
    }

    #[test]
    fn a_monitor_left_of_and_above_the_main_one() {
        let (w, h, at) = layout(&[mon(0, 0, 2560, 1440), mon(-1080, -300, 1080, 1920)]);
        assert_eq!((w, h), (3640, 1920));
        assert_eq!(at, vec![(1080, 300), (0, 0)]);
    }

    #[test]
    fn odd_sizes_become_even() {
        let (w, h, _) = layout(&[mon(0, 0, 1366, 767)]);
        assert_eq!((w, h), (1366, 768));
    }
}
