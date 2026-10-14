//! The webcam. Read through Media Foundation, the way every camera app on Windows does it,
//! and handed on as BGRA pictures. The pipeline encodes them into a buffer of their own,
//! so a clip gets the camera as a second file next to it: `<clip>.cam.mp4`.
//!
//! For tests without a camera, `KLIP_CAMERA_TEST=<video file>` feeds a file instead.

use crate::Res;
use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use windows::core::{Interface, PWSTR};
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::{CoInitializeEx, CoTaskMemFree, COINIT_MULTITHREADED};

/// The largest picture asked from the camera. More costs encoder time and disk space for
/// what ends up as a small picture in a corner.
const MAX_HEIGHT: u32 = 720;
pub const FPS: u32 = 30;

/// `MF_SOURCE_READER_FIRST_VIDEO_STREAM` as the u32 the calls take.
const FIRST_VIDEO: u32 = 0xFFFF_FFFC;

fn pack(hi: u32, lo: u32) -> u64 {
    ((hi as u64) << 32) | lo as u64
}

fn unpack(v: u64) -> (u32, u32) {
    ((v >> 32) as u32, v as u32)
}

/// All video capture devices with their names, in the order Windows lists them.
fn devices() -> Res<Vec<(String, IMFActivate)>> {
    unsafe {
        let mut attrs = None;
        MFCreateAttributes(&mut attrs, 1)?;
        let attrs = attrs.ok_or("no attributes")?;
        attrs.SetGUID(&MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE, &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID)?;
        let mut list: *mut Option<IMFActivate> = std::ptr::null_mut();
        let mut count = 0u32;
        MFEnumDeviceSources(&attrs, &mut list, &mut count)?;
        let mut out = Vec::new();
        for i in 0..count as usize {
            let Some(activate) = (*list.add(i)).take() else { continue };
            let mut name = PWSTR::null();
            let mut len = 0u32;
            let text = if activate.GetAllocatedString(&MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME, &mut name, &mut len).is_ok() {
                let t = name.to_string().unwrap_or_default();
                CoTaskMemFree(Some(name.0 as *const _));
                t
            } else {
                format!("Camera {}", i + 1)
            };
            out.push((text, activate));
        }
        if !list.is_null() {
            CoTaskMemFree(Some(list as *const _));
        }
        Ok(out)
    }
}

/// Names of the cameras, for the settings.
pub fn names() -> Vec<String> {
    devices().map(|d| d.into_iter().map(|(n, _)| n).collect()).unwrap_or_default()
}

/// The size the camera is asked for: the largest it offers up to 720 lines, at least
/// 25 frames per second if it can. Falls back to its first format.
fn pick_format(reader: &IMFSourceReader) -> Option<(u32, u32)> {
    let mut best: Option<(u32, u32, bool)> = None;
    for i in 0.. {
        let Ok(t) = (unsafe { reader.GetNativeMediaType(FIRST_VIDEO, i) }) else { break };
        let Ok(size) = (unsafe { t.GetUINT64(&MF_MT_FRAME_SIZE) }) else { continue };
        let (w, h) = unpack(size);
        let fast = unsafe { t.GetUINT64(&MF_MT_FRAME_RATE) }.map(|r| {
            let (n, d) = unpack(r);
            d > 0 && n / d >= 25
        });
        let fast = fast.unwrap_or(false);
        if h == 0 || h > MAX_HEIGHT {
            continue;
        }
        let better = match best {
            None => true,
            Some((bw, bh, bfast)) => (fast && !bfast) || (fast == bfast && w * h > bw * bh),
        };
        if better {
            best = Some((w, h, fast));
        }
    }
    best.map(|(w, h, _)| (w & !1, h & !1))
}

/// A running camera. Stops when dropped or when `stop` is set.
pub struct Feed {
    stop: Arc<AtomicBool>,
    pub name: String,
    pub width: u32,
    pub height: u32,
}

impl Drop for Feed {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

/// Opens the camera whose name contains `wanted` (the first one without a wish) and calls
/// `on_frame` with each picture: BGRA rows top down, width, height, and the time on the
/// clock of the screen capture.
pub fn start<F>(wanted: Option<&str>, stop: Arc<AtomicBool>, on_frame: F) -> Res<Feed>
where
    F: FnMut(&[u8], u32, u32, i64) + Send + 'static,
{
    if let Some(path) = std::env::var_os("KLIP_CAMERA_TEST").filter(|v| !v.is_empty()) {
        return start_test(std::path::PathBuf::from(path), stop, on_frame);
    }
    let mut all = devices()?;
    if all.is_empty() {
        return Err("no camera is connected".into());
    }
    let at = wanted
        .and_then(|w| all.iter().position(|(n, _)| n.to_lowercase().contains(&w.to_lowercase())))
        .unwrap_or(0);
    let (name, activate) = all.swap_remove(at);

    // The device is opened on the reading thread, Media Foundation objects stay there.
    let (tx, rx) = std::sync::mpsc::channel::<Res<(u32, u32)>>();
    let thread_stop = stop.clone();
    let activate = SendActivate(activate);
    std::thread::Builder::new().name("klip-camera".into()).spawn(move || {
        let activate = activate;
        let mut on_frame = on_frame;
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }
        let opened = (|| -> Res<(IMFSourceReader, u32, u32)> {
            unsafe {
                let source: IMFMediaSource = activate.0.ActivateObject()?;
                let mut attrs = None;
                MFCreateAttributes(&mut attrs, 2)?;
                let attrs = attrs.ok_or("no attributes")?;
                // Lets the reader convert whatever the camera sends (MJPG, YUY2, NV12)
                // into plain 32 bit RGB.
                attrs.SetUINT32(&MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, 1)?;
                let reader = MFCreateSourceReaderFromMediaSource(&source, &attrs)?;
                let (w, h) = pick_format(&reader).ok_or("the camera offers no usable picture size")?;
                let out = MFCreateMediaType()?;
                out.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
                out.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)?;
                out.SetUINT64(&MF_MT_FRAME_SIZE, pack(w, h))?;
                out.SetUINT64(&MF_MT_FRAME_RATE, pack(FPS, 1))?;
                reader.SetCurrentMediaType(FIRST_VIDEO, None, &out)?;
                Ok((reader, w, h))
            }
        })();
        let (reader, w, h) = match opened {
            Ok(v) => v,
            Err(e) => {
                let _ = tx.send(Err(e));
                return;
            }
        };
        let _ = tx.send(Ok((w, h)));
        let mut picture = vec![0u8; (w * h * 4) as usize];
        while !thread_stop.load(Ordering::Acquire) {
            let mut flags = 0u32;
            let mut sample = None;
            let read = unsafe { reader.ReadSample(FIRST_VIDEO, 0, None, Some(&mut flags), None, Some(&mut sample)) };
            if read.is_err() || flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 || flags & MF_SOURCE_READERF_ERROR.0 as u32 != 0 {
                eprintln!("klip-recorder: the camera stopped delivering pictures");
                break;
            }
            let Some(sample) = sample else { continue };
            let time = crate::qpc_100ns();
            if copy_rgb32(&sample, w, h, &mut picture).is_ok() {
                on_frame(&picture, w, h, time);
            }
        }
    })?;
    let (width, height) = rx.recv().map_err(|_| "the camera thread ended")??;
    println!("camera: {name}, {width}x{height}");
    Ok(Feed { stop, name, width, height })
}

struct SendActivate(IMFActivate);
// SAFETY: activation objects of capture devices are free threaded.
unsafe impl Send for SendActivate {}

/// Copies one RGB32 sample into `out` as top down BGRA rows.
fn copy_rgb32(sample: &IMFSample, w: u32, h: u32, out: &mut [u8]) -> Res<()> {
    unsafe {
        let buffer = sample.ConvertToContiguousBuffer()?;
        let row = (w * 4) as usize;
        if let Ok(b2) = buffer.cast::<IMF2DBuffer>() {
            let mut first: *mut u8 = std::ptr::null_mut();
            let mut pitch = 0i32;
            b2.Lock2D(&mut first, &mut pitch)?;
            // A negative pitch means the picture is stored bottom up, `first` is still
            // the top row.
            for y in 0..h as usize {
                let src = first.offset(y as isize * pitch as isize);
                std::ptr::copy_nonoverlapping(src, out[y * row..].as_mut_ptr(), row);
            }
            let _ = b2.Unlock2D();
        } else {
            let mut data: *mut u8 = std::ptr::null_mut();
            let mut len = 0u32;
            buffer.Lock(&mut data, None, Some(&mut len))?;
            let n = (len as usize).min(out.len());
            std::ptr::copy_nonoverlapping(data, out.as_mut_ptr(), n);
            let _ = buffer.Unlock();
        }
        Ok(())
    }
}

/// Test input: a video file played in a loop at the camera size, through ffmpeg.
fn start_test<F>(path: std::path::PathBuf, stop: Arc<AtomicBool>, mut on_frame: F) -> Res<Feed>
where
    F: FnMut(&[u8], u32, u32, i64) + Send + 'static,
{
    let (w, h) = (1280u32, 720u32);
    let mut child = Command::new("ffmpeg")
        .args(["-v", "error", "-nostdin", "-re", "-stream_loop", "-1", "-i"])
        .arg(&path)
        .args(["-an", "-vf", &format!("scale={w}:{h},fps={FPS}"), "-pix_fmt", "bgra", "-f", "rawvideo", "-"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("ffmpeg is needed for KLIP_CAMERA_TEST: {e}"))?;
    let mut pipe = child.stdout.take().ok_or("no output from ffmpeg")?;
    let thread_stop = stop.clone();
    std::thread::Builder::new().name("klip-camera-test".into()).spawn(move || {
        let mut picture = vec![0u8; (w * h * 4) as usize];
        while !thread_stop.load(Ordering::Acquire) && pipe.read_exact(&mut picture).is_ok() {
            on_frame(&picture, w, h, crate::qpc_100ns());
        }
        let _ = child.kill();
        let _ = child.wait();
    })?;
    println!("camera: test file {}, {w}x{h}", path.display());
    Ok(Feed { stop, name: "test".into(), width: w, height: h })
}
