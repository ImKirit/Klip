//! Renders the overlay into a PNG.
//!
//! The live windows are excluded from screen capture, so a screenshot can never show
//! them. This draws the same scenes with the same view code into a WIC bitmap on a plain
//! background instead. It proves what the views draw, not what the composition shows.

use crate::fonts::{FontReport, Fonts};
use crate::gfx::{color, with_offset_and_opacity, Gfx};
use crate::scene::{resolve, ToastItem};
use crate::state::{Action, Element, OverlayState, Toast};
use crate::theme::Rgb;
use std::path::{Path, PathBuf};
use std::time::Instant;
use windows::core::HSTRING;
use windows::Win32::Foundation::GENERIC_WRITE;
use windows::Win32::Graphics::Direct2D::Common::{D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_PIXEL_FORMAT};
use windows::Win32::Graphics::Direct2D::{
    D2D1CreateFactory, ID2D1Factory, D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_RENDER_TARGET_PROPERTIES,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_ContainerFormatPng, GUID_WICPixelFormat32bppBGRA, GUID_WICPixelFormat32bppPBGRA,
    IWICImagingFactory, WICBitmapCacheOnLoad, WICBitmapEncoderNoCache, WICRect,
};
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED};

#[derive(Clone, Debug)]
pub struct SnapshotOptions {
    /// Size of the stand in for the monitor.
    pub width: u32,
    pub height: u32,
    pub background: Rgb,
    /// Cut the picture down to what was drawn plus `padding`, enlarged by `zoom`.
    pub crop: bool,
    pub padding: u32,
    /// Whole factor by which every element is enlarged. Only sensible with `crop`.
    pub zoom: u32,
    pub font_files: Vec<PathBuf>,
    /// The toast to show, as if it were on screen right now.
    pub toast: Option<Toast>,
    /// More toasts, stacked below `toast` in this order. At most four are drawn in all.
    pub toasts: Vec<Toast>,
    /// Panel button drawn in its hover look.
    pub hover: Option<Action>,
    /// Restrict the picture to these elements. Empty means all that the state shows.
    pub only: Vec<Element>,
}

impl Default for SnapshotOptions {
    fn default() -> Self {
        SnapshotOptions {
            width: 1920,
            height: 1080,
            background: Rgb(0x26, 0x2A, 0x2E),
            crop: false,
            padding: 24,
            zoom: 1,
            font_files: Vec::new(),
            toast: None,
            toasts: Vec::new(),
            hover: None,
            only: Vec::new(),
        }
    }
}

/// Draws `state` as the overlay would show it and writes a PNG. Returns which fonts
/// were used.
pub fn render_png(state: &OverlayState, options: &SnapshotOptions, path: &Path) -> Result<FontReport, String> {
    let fonts = Fonts::new(&options.font_files)?;
    render(state, options, path, &fonts).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(fonts.report().clone())
}

fn render(state: &OverlayState, o: &SnapshotOptions, path: &Path, fonts: &Fonts) -> windows::core::Result<()> {
    let zoom = o.zoom.clamp(1, 4);
    let (cw, ch) = (o.width.clamp(16, 8192), o.height.clamp(16, 8192));
    unsafe {
        // WIC is COM. A thread that is already initialized another way is fine too.
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let wic: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
        let bitmap = wic.CreateBitmap(cw, ch, &GUID_WICPixelFormat32bppPBGRA, WICBitmapCacheOnLoad)?;
        let factory: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
        let props = D2D1_RENDER_TARGET_PROPERTIES {
            pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
            dpiX: 96.0,
            dpiY: 96.0,
            ..Default::default()
        };
        let rt = factory.CreateWicBitmapRenderTarget(&bitmap, &props)?;

        // The canvas plays the monitor. With zoom the monitor shrinks, the pixels stay.
        let area = (cw as f32 / zoom as f32, ch as f32 / zoom as f32);
        let k = (o.height as f32 / 1080.0).clamp(0.25, 8.0);
        let now = Instant::now();
        let stack: Vec<ToastItem> = o
            .toast
            .iter()
            .chain(o.toasts.iter())
            .take(crate::state::MAX_TOASTS)
            .map(|t| ToastItem { title: t.title.clone(), detail: t.detail.clone(), kind: t.kind, alpha: 1.0 })
            .collect();
        let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, 0.0f32, 0.0f32);

        rt.BeginDraw();
        rt.Clear(Some(&color(o.background, 1.0)));
        {
            let painter = Gfx::on(fonts, &rt)?;
            for element in Element::ALL {
                if !o.only.is_empty() && !o.only.contains(&element) {
                    continue;
                }
                let hover = if element == Element::Panel { o.hover } else { None };
                let Some(r) = resolve(state, element, &stack, hover, now, k) else { continue };
                let scale = r.scale * zoom as f32;
                let size = r.scene.view(&painter.measuring(), scale, &mut Vec::new());
                let unzoomed = (size.0 / zoom as f32, size.1 / zoom as f32);
                let (ox, oy) = r.place.origin(area, unzoomed, k);
                let (ox, oy) = (ox * zoom as f32, oy * zoom as f32);
                with_offset_and_opacity(&rt, ox, oy, r.opacity, || {
                    r.scene.view(&painter, scale, &mut Vec::new());
                });
                x0 = x0.min(ox);
                y0 = y0.min(oy);
                x1 = x1.max(ox + size.0);
                y1 = y1.max(oy + size.1);
            }
        }
        rt.EndDraw(None, None)?;

        let pad = o.padding as f32;
        let rect = if o.crop && x1 > x0 {
            let (l, t) = ((x0 - pad).max(0.0) as i32, (y0 - pad).max(0.0) as i32);
            let (r, b) = ((x1 + pad).min(cw as f32) as i32, (y1 + pad).min(ch as f32) as i32);
            WICRect { X: l, Y: t, Width: r - l, Height: b - t }
        } else {
            WICRect { X: 0, Y: 0, Width: cw as i32, Height: ch as i32 }
        };

        let stream = wic.CreateStream()?;
        stream.InitializeFromFilename(&HSTRING::from(path.as_os_str()), GENERIC_WRITE.0)?;
        let encoder = wic.CreateEncoder(&GUID_ContainerFormatPng, std::ptr::null())?;
        encoder.Initialize(&stream, WICBitmapEncoderNoCache)?;
        let mut frame = None;
        encoder.CreateNewFrame(&mut frame, std::ptr::null_mut())?;
        let frame = frame.ok_or_else(windows::core::Error::empty)?;
        frame.Initialize(None)?;
        frame.SetSize(rect.Width as u32, rect.Height as u32)?;
        let mut format = GUID_WICPixelFormat32bppBGRA;
        frame.SetPixelFormat(&mut format)?;
        frame.WriteSource(&bitmap, &rect)?;
        frame.Commit()?;
        encoder.Commit()?;
    }
    Ok(())
}
