//! Screenshots: one captured frame is read back from the GPU and written as PNG.
//! This is the only place where a picture is copied into RAM, and only on request.

use crate::Res;
use std::path::Path;
use windows::core::HSTRING;
use windows::Win32::Foundation::GENERIC_WRITE;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED};

pub struct Pixels {
    pub width: u32,
    pub height: u32,
    /// BGRA, top row first, `width * 4` bytes per row.
    pub data: Vec<u8>,
}

/// Copies the texture into RAM. Runs on the capture thread, so it does nothing but copy.
pub fn read_back(device: &ID3D11Device, context: &ID3D11DeviceContext, bgra: &ID3D11Texture2D) -> Res<Pixels> {
    unsafe {
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        bgra.GetDesc(&mut desc);
        desc.Usage = D3D11_USAGE_STAGING;
        desc.BindFlags = 0;
        desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
        desc.MiscFlags = 0;
        let mut staging = None;
        device.CreateTexture2D(&desc, None, Some(&mut staging))?;
        let staging = staging.ok_or("no staging texture")?;
        context.CopyResource(&staging, bgra);

        let mut map = D3D11_MAPPED_SUBRESOURCE::default();
        context.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut map))?;
        let (w, h) = (desc.Width as usize, desc.Height as usize);
        let mut data = vec![0u8; w * h * 4];
        for y in 0..h {
            let src = std::slice::from_raw_parts((map.pData as *const u8).add(y * map.RowPitch as usize), w * 4);
            data[y * w * 4..(y + 1) * w * 4].copy_from_slice(src);
        }
        context.Unmap(&staging, 0);
        // The alpha channel of a captured frame carries no meaning, a PNG would show it as transparency.
        for px in data.chunks_exact_mut(4) {
            px[3] = 255;
        }
        Ok(Pixels { width: desc.Width, height: desc.Height, data })
    }
}

/// Encodes with the PNG codec that ships with Windows. Called on a worker thread.
pub fn write_png(path: &Path, pixels: &Pixels) -> Res<()> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let factory: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
        let stream = factory.CreateStream()?;
        stream.InitializeFromFilename(&HSTRING::from(path.as_os_str()), GENERIC_WRITE.0)?;
        let encoder = factory.CreateEncoder(&GUID_ContainerFormatPng, std::ptr::null())?;
        encoder.Initialize(&stream, WICBitmapEncoderNoCache)?;
        let mut frame = None;
        encoder.CreateNewFrame(&mut frame, std::ptr::null_mut())?;
        let frame = frame.ok_or("no PNG frame")?;
        frame.Initialize(None)?;
        frame.SetSize(pixels.width, pixels.height)?;
        let mut format = GUID_WICPixelFormat32bppBGRA;
        frame.SetPixelFormat(&mut format)?;
        if format != GUID_WICPixelFormat32bppBGRA {
            return Err("the PNG encoder refused BGRA".into());
        }
        frame.WritePixels(pixels.height, pixels.width * 4, &pixels.data)?;
        frame.Commit()?;
        encoder.Commit()?;
        Ok(())
    }
}
