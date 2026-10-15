//! Windows Graphics Capture of a whole monitor. Runs entirely outside of any game process.

use crate::gfx::Gfx;
use crate::Res;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::Arc;
use windows::core::{factory, IInspectable, Interface};
use windows::Foundation::TypedEventHandler;
use windows::Graphics::Capture::{
    Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession,
};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Direct3D11::ID3D11Texture2D;
use windows::Win32::Graphics::Gdi::HMONITOR;
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::System::WinRT::Direct3D11::{
    CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess,
};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;

#[derive(Default)]
pub struct CaptureStats {
    /// Frames delivered by Windows, at up to the refresh rate of the monitor.
    pub arrived: AtomicU64,
    /// Frames passed on at the target frame rate.
    pub taken: AtomicU64,
}

pub struct Capture {
    pool: Direct3D11CaptureFramePool,
    session: GraphicsCaptureSession,
    pub stats: Arc<CaptureStats>,
    pub width: u32,
    pub height: u32,
}

impl Drop for Capture {
    fn drop(&mut self) {
        // Closing ends the capture at once. Without it Windows keeps delivering frames
        // until the objects are collected.
        let _ = self.session.Close();
        let _ = self.pool.Close();
    }
}

/// What to capture. A window source records that window only, things lying on top of it
/// (other windows, notifications) are not part of the picture.
#[derive(Clone, Copy)]
pub enum Source {
    Monitor(HMONITOR),
    Window(HWND),
}

/// A capture that is set up but not running yet, so its size is known up front.
pub struct Prepared {
    device: IDirect3DDevice,
    item: GraphicsCaptureItem,
    pub width: u32,
    pub height: u32,
}

pub fn prepare(gfx: &Gfx, source: Source) -> Res<Prepared> {
    unsafe {
        let dxgi: IDXGIDevice = gfx.device.cast()?;
        let inspectable: IInspectable = CreateDirect3D11DeviceFromDXGIDevice(&dxgi)?;
        let device: IDirect3DDevice = inspectable.cast()?;
        let interop = factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()?;
        let item: GraphicsCaptureItem = match source {
            Source::Monitor(m) => interop.CreateForMonitor(m)?,
            Source::Window(w) => interop.CreateForWindow(w)?,
        };
        let size = item.Size()?;
        Ok(Prepared { device, item, width: size.Width as u32, height: size.Height as u32 })
    }
}

/// Starts the prepared capture. `on_frame` gets the BGRA texture and its
/// timestamp in 100 ns units (QueryPerformanceCounter time base), at most `fps` times
/// per second. Windows delivers frames at the refresh rate of the monitor, 300 Hz on the
/// development machine, so everything above the target rate is dropped right here.
pub fn start<F>(prepared: Prepared, fps: u32, capture_cursor: bool, on_frame: F) -> Res<Capture>
where
    F: Fn(&ID3D11Texture2D, i64) + Send + 'static,
{
    unsafe {
        let Prepared { device, item, .. } = prepared;
        let size = item.Size()?;

        let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
            &device,
            DirectXPixelFormat::B8G8R8A8UIntNormalized,
            // One more than the minimum, so a frame still being converted does not make
            // Windows skip the next one.
            3,
            size,
        )?;
        let session = pool.CreateCaptureSession(&item)?;
        let _ = session.SetIsCursorCaptureEnabled(capture_cursor);
        // Only exists from build 20348 on. On Windows 10 the yellow border stays.
        let _ = session.SetIsBorderRequired(false);

        let stats = Arc::new(CaptureStats::default());
        let handler_stats = stats.clone();
        // Frames are snapped to a grid of the target frame rate, anchored at the first frame.
        // The first frame that falls into a new slot is taken, later ones in the same slot
        // are dropped. Anchoring keeps a source that runs at the target rate in the middle
        // of its slots, so timer jitter cannot drop or double a frame.
        let slot_len: i64 = 10_000_000 / fps as i64;
        let anchor = AtomicI64::new(i64::MIN);
        let last_slot = AtomicI64::new(-1);

        pool.FrameArrived(&TypedEventHandler::new(
            move |sender: windows::core::Ref<'_, Direct3D11CaptureFramePool>, _| {
                let Some(pool) = sender.as_ref() else { return Ok(()) };
                // Taking the frame returns the previous one to the pool, even if it is dropped.
                let Ok(frame) = pool.TryGetNextFrame() else { return Ok(()) };
                handler_stats.arrived.fetch_add(1, Ordering::Relaxed);
                let time = frame.SystemRelativeTime()?.Duration;
                if anchor.load(Ordering::Relaxed) == i64::MIN {
                    anchor.store(time, Ordering::Relaxed);
                }
                let base = anchor.load(Ordering::Relaxed);
                let slot = (time - base + slot_len / 2).div_euclid(slot_len);
                if slot <= last_slot.load(Ordering::Relaxed) {
                    return Ok(());
                }
                last_slot.store(slot, Ordering::Relaxed);
                let access: IDirect3DDxgiInterfaceAccess = frame.Surface()?.cast()?;
                let texture: ID3D11Texture2D = access.GetInterface()?;
                handler_stats.taken.fetch_add(1, Ordering::Relaxed);
                on_frame(&texture, base + slot * slot_len);
                Ok(())
            },
        ))?;
        session.StartCapture()?;

        Ok(Capture {
            pool,
            session,
            stats,
            width: size.Width as u32,
            height: size.Height as u32,
        })
    }
}
