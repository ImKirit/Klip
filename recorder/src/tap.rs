//! Reads small rectangles out of a captured frame for the event detectors. The frame is
//! the one that gets encoded anyway, so detection needs no second capture. Only the
//! requested rectangles are copied into RAM, a few hundred kilobytes a few times a second.

use crate::Res;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};

/// A rectangle in pixels of the captured frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

/// Pixels of one rectangle: BGRA, top row first, `w * 4` bytes per row.
pub struct Region {
    pub rect: Rect,
    pub data: Vec<u8>,
}

pub struct RegionTap {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    /// Reused between reads, recreated only when a larger rectangle is asked for.
    staging: Option<(ID3D11Texture2D, u32, u32)>,
}
// SAFETY: the device is free threaded, the context is multithread protected, and the tap
// is used from the capture thread only.
unsafe impl Send for RegionTap {}

impl RegionTap {
    pub fn new(device: &ID3D11Device, context: &ID3D11DeviceContext) -> Self {
        Self { device: device.clone(), context: context.clone(), staging: None }
    }

    /// Copies `rect` of `frame` into RAM. The rectangle is clipped to the frame.
    pub fn read(&mut self, frame: &ID3D11Texture2D, rect: Rect) -> Res<Region> {
        unsafe {
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            frame.GetDesc(&mut desc);
            let x = rect.x.min(desc.Width.saturating_sub(1));
            let y = rect.y.min(desc.Height.saturating_sub(1));
            let w = rect.w.min(desc.Width - x).max(1);
            let h = rect.h.min(desc.Height - y).max(1);

            if !self.staging.as_ref().is_some_and(|(_, sw, sh)| *sw >= w && *sh >= h) {
                let td = D3D11_TEXTURE2D_DESC {
                    Width: w,
                    Height: h,
                    MipLevels: 1,
                    ArraySize: 1,
                    Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                    Usage: D3D11_USAGE_STAGING,
                    BindFlags: 0,
                    CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                    MiscFlags: 0,
                };
                let mut tex = None;
                self.device.CreateTexture2D(&td, None, Some(&mut tex))?;
                self.staging = Some((tex.ok_or("no staging texture")?, w, h));
            }
            let (staging, _, _) = self.staging.as_ref().unwrap();

            let source = D3D11_BOX { left: x, top: y, front: 0, right: x + w, bottom: y + h, back: 1 };
            self.context.CopySubresourceRegion(staging, 0, 0, 0, 0, frame, 0, Some(&source));
            let mut map = D3D11_MAPPED_SUBRESOURCE::default();
            self.context.Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut map))?;
            let (wb, rows) = (w as usize * 4, h as usize);
            let mut data = vec![0u8; wb * rows];
            for row in 0..rows {
                let src = std::slice::from_raw_parts((map.pData as *const u8).add(row * map.RowPitch as usize), wb);
                data[row * wb..(row + 1) * wb].copy_from_slice(src);
            }
            self.context.Unmap(staging, 0);
            Ok(Region { rect: Rect { x, y, w, h }, data })
        }
    }
}
