//! Test input: plays a video file into the recording chain instead of capturing the
//! screen. Everything behind the capture runs exactly as in a game (conversion, encoder,
//! region tap, detector, clip planner, auto clip), so recorded footage of a game can prove
//! the chain without the game running. Not used in normal operation.

use crate::capture::CaptureStats;
use crate::gfx::Gfx;
use crate::Res;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};

/// Frames per second the file is fed with. Enough for the detector, which samples at ten,
/// and small enough that the raw frames fit through a pipe in real time.
const FEED_FPS: u32 = 30;

/// Width and height of the first video stream, asked from ffprobe.
pub fn probe(path: &Path) -> Res<(u32, u32)> {
    let out = Command::new("ffprobe")
        .args(["-v", "error", "-select_streams", "v:0", "-show_entries", "stream=width,height", "-of", "csv=p=0"])
        .arg(path)
        .output()
        .map_err(|e| format!("ffprobe is needed for --replay and was not found: {e}"))?;
    let text = String::from_utf8_lossy(&out.stdout);
    let mut parts = text.trim().split(',').filter_map(|p| p.trim().parse::<u32>().ok());
    match (parts.next(), parts.next()) {
        (Some(w), Some(h)) => Ok((w, h)),
        _ => Err(format!("could not read the size of {}", path.display()).into()),
    }
}

struct SendGpu(ID3D11Device, ID3D11DeviceContext);
// SAFETY: the device is free threaded and the context is multithread protected.
unsafe impl Send for SendGpu {}

/// Starts feeding. `on_frame` gets the same arguments as from the live capture. When the
/// file ends, `done` is set.
pub fn start<F>(
    gfx: &Gfx,
    path: &Path,
    width: u32,
    height: u32,
    stop: Arc<AtomicBool>,
    done: Arc<AtomicBool>,
    stats: Arc<CaptureStats>,
    on_frame: F,
) -> Res<()>
where
    F: Fn(&ID3D11Texture2D, i64) + Send + 'static,
{
    // `-re` paces the file in real time, so timestamps and buffer behave as in a live run.
    let mut child = Command::new("ffmpeg")
        .args(["-v", "error", "-nostdin", "-re", "-i"])
        .arg(path)
        .args(["-an", "-vf", &format!("fps={FEED_FPS}"), "-pix_fmt", "bgra", "-f", "rawvideo", "-"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("ffmpeg is needed for --replay and was not found: {e}"))?;
    let mut pipe = child.stdout.take().ok_or("no output from ffmpeg")?;
    let gpu = SendGpu(gfx.device.clone(), gfx.context.clone());

    std::thread::Builder::new().name("klip-replay".into()).spawn(move || {
        let gpu = gpu;
        let desc = D3D11_TEXTURE2D_DESC {
            Width: width,
            Height: height,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D11_USAGE_DEFAULT,
            // The same flags a captured frame has, the converter needs them for its input view.
            BindFlags: (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        let mut texture = None;
        if unsafe { gpu.0.CreateTexture2D(&desc, None, Some(&mut texture)) }.is_err() {
            done.store(true, Ordering::Release);
            return;
        }
        let Some(texture) = texture else { return };
        let mut frame = vec![0u8; width as usize * height as usize * 4];
        while !stop.load(Ordering::Acquire) && pipe.read_exact(&mut frame).is_ok() {
            unsafe {
                gpu.1.UpdateSubresource(&texture, 0, None, frame.as_ptr() as *const _, width * 4, 0);
            }
            stats.arrived.fetch_add(1, Ordering::Relaxed);
            stats.taken.fetch_add(1, Ordering::Relaxed);
            on_frame(&texture, crate::qpc_100ns());
        }
        let _ = child.kill();
        let _ = child.wait();
        done.store(true, Ordering::Release);
    })?;
    Ok(())
}
