//! Video encoding. The rest of the recorder only knows the `Encoder` trait, so further
//! backends (NVENC direct, FFmpeg libraries) can be added without touching capture or the
//! replay buffer. The first backend uses the hardware encoder MFTs that ship with the
//! graphics driver, which keeps it in step with the installed driver version.

use crate::gfx::Gfx;
use crate::ring::Packet;
use crate::Res;
use klip_core::VideoCodec;
use std::mem::ManuallyDrop;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use windows::core::{Interface, GUID, PWSTR};
use windows::Win32::Graphics::Direct3D11::ID3D11Texture2D;
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::{CoInitializeEx, CoTaskMemFree, COINIT_MULTITHREADED};

#[derive(Clone, Copy, Debug)]
pub struct EncoderConfig {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub bitrate_kbps: u32,
    pub codec: VideoCodec,
    pub gop_seconds: u32,
}

pub trait Encoder: Send + Sync {
    fn name(&self) -> &str;
    /// Hands over one NV12 texture. Times are in 100 ns units. If the encoder is busy the
    /// frame waits in a queue of depth one. Returns false if that pushed out an older
    /// waiting frame, which is then lost.
    fn submit(&self, nv12: &ID3D11Texture2D, pts: i64, duration: i64) -> Res<bool>;
    /// Stream description for the muxer, available once the first packet came out.
    fn stream_type(&self) -> Option<StreamType>;
    fn frames_out(&self) -> u64;
    /// Makes the next frame a keyframe. Needed where something has to start decoding at
    /// a known point: after a pause, and while the picture stands still, when frames are
    /// a second apart and the regular keyframe would be a minute away.
    fn force_keyframe(&self);
    /// Stops the encoder and frees the hardware session. Nothing may be submitted afterwards.
    fn shutdown(&self);
}

/// Media Foundation media type of the encoded stream.
#[derive(Clone)]
pub struct StreamType(pub IMFMediaType);
// SAFETY: Media Foundation objects are free threaded.
unsafe impl Send for StreamType {}
unsafe impl Sync for StreamType {}

struct Inner {
    mft: IMFTransform,
    codec: Option<ICodecAPI>,
    name: String,
    feed: Mutex<Feed>,
    stopping: std::sync::atomic::AtomicBool,
    frames_out: AtomicU64,
    stream_type: Mutex<Option<StreamType>>,
    provides_samples: bool,
    out_buffer_size: u32,
    // The transform holds a pointer to the manager, it must outlive the transform.
    _manager: IMFDXGIDeviceManager,
}
// SAFETY: asynchronous MFTs are free threaded by contract.
unsafe impl Send for Inner {}
unsafe impl Sync for Inner {}

/// How many frames the transform has asked for, and the one frame waiting for a request.
/// Both live under one lock so a request and a waiting frame can never miss each other.
struct Feed {
    need: i32,
    pending: Option<IMFSample>,
}

pub struct MfEncoder {
    inner: Arc<Inner>,
}

fn pack(hi: u32, lo: u32) -> u64 {
    ((hi as u64) << 32) | lo as u64
}

/// Marks a stream as BT.709 limited range, matching what the converter produces. Without
/// the tags players have to guess and may show washed out or crushed colors.
unsafe fn tag_bt709(t: &IMFMediaType) {
    let _ = t.SetUINT32(&MF_MT_VIDEO_PRIMARIES, MFVideoPrimaries_BT709.0 as u32);
    let _ = t.SetUINT32(&MF_MT_TRANSFER_FUNCTION, MFVideoTransFunc_709.0 as u32);
    let _ = t.SetUINT32(&MF_MT_YUV_MATRIX, MFVideoTransferMatrix_BT709.0 as u32);
    let _ = t.SetUINT32(&MF_MT_VIDEO_NOMINAL_RANGE, MFNominalRange_16_235.0 as u32);
}

fn subtype(codec: VideoCodec) -> GUID {
    match codec {
        VideoCodec::H264 => MFVideoFormat_H264,
        VideoCodec::Hevc => MFVideoFormat_HEVC,
        VideoCodec::Av1 => MFVideoFormat_AV1,
    }
}

impl MfEncoder {
    pub fn new<F>(gfx: &Gfx, cfg: EncoderConfig, on_packet: F) -> Res<Self>
    where
        F: FnMut(Packet) + Send + 'static,
    {
        unsafe {
            let mut token = 0u32;
            let mut manager = None;
            MFCreateDXGIDeviceManager(&mut token, &mut manager)?;
            let manager = manager.ok_or("no DXGI device manager")?;
            manager.ResetDevice(&gfx.device, token)?;

            let (mft, name) = find_hardware_encoder(cfg.codec, &gfx.adapter_name)?;

            let attrs = mft.GetAttributes()?;
            // Hardware encoders are asynchronous and refuse to work until unlocked.
            attrs.SetUINT32(&MF_TRANSFORM_ASYNC_UNLOCK, 1)?;
            let _ = attrs.SetUINT32(&MF_LOW_LATENCY, 1);
            mft.ProcessMessage(MFT_MESSAGE_SET_D3D_MANAGER, manager.as_raw() as usize)?;

            // Encoders require the output type first.
            let out = MFCreateMediaType()?;
            out.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            out.SetGUID(&MF_MT_SUBTYPE, &subtype(cfg.codec))?;
            out.SetUINT32(&MF_MT_AVG_BITRATE, cfg.bitrate_kbps * 1000)?;
            out.SetUINT64(&MF_MT_FRAME_SIZE, pack(cfg.width, cfg.height))?;
            out.SetUINT64(&MF_MT_FRAME_RATE, pack(cfg.fps, 1))?;
            out.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1))?;
            out.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
            if cfg.codec == VideoCodec::H264 {
                out.SetUINT32(&MF_MT_MPEG2_PROFILE, eAVEncH264VProfile_High.0 as u32)?;
            }
            let _ = out.SetUINT32(&MF_MT_MAX_KEYFRAME_SPACING, cfg.fps * cfg.gop_seconds);
            tag_bt709(&out);
            mft.SetOutputType(0, &out, 0)?;

            let inp = MFCreateMediaType()?;
            tag_bt709(&inp);
            inp.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            inp.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)?;
            inp.SetUINT64(&MF_MT_FRAME_SIZE, pack(cfg.width, cfg.height))?;
            inp.SetUINT64(&MF_MT_FRAME_RATE, pack(cfg.fps, 1))?;
            inp.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1))?;
            inp.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
            mft.SetInputType(0, &inp, 0)?;

            // Vendors differ in which of these they accept, so every call is best effort.
            let codec = mft.cast::<ICodecAPI>().ok();
            if let Some(api) = &codec {
                let set = |key: &GUID, value: u32| {
                    let v = windows::Win32::System::Variant::VARIANT::from(value);
                    let _ = api.SetValue(key, &v);
                };
                set(&CODECAPI_AVEncCommonRateControlMode, eAVEncCommonRateControlMode_CBR.0 as u32);
                set(&CODECAPI_AVEncCommonMeanBitRate, cfg.bitrate_kbps * 1000);
                set(&CODECAPI_AVEncMPVGOPSize, cfg.fps * cfg.gop_seconds);
                // B frames add latency and reorder packets, a replay buffer gains nothing from them.
                set(&CODECAPI_AVEncMPVDefaultBPictureCount, 0);
            }

            let info = mft.GetOutputStreamInfo(0)?;
            let provides_samples = info.dwFlags
                & (MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 | MFT_OUTPUT_STREAM_CAN_PROVIDE_SAMPLES.0) as u32
                != 0;

            let inner = Arc::new(Inner {
                mft,
                codec,
                name,
                feed: Mutex::new(Feed { need: 0, pending: None }),
                stopping: std::sync::atomic::AtomicBool::new(false),
                frames_out: AtomicU64::new(0),
                stream_type: Mutex::new(None),
                provides_samples,
                out_buffer_size: info.cbSize.max(1 << 20),
                _manager: manager,
            });

            inner.mft.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)?;
            inner.mft.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)?;

            let worker = inner.clone();
            std::thread::Builder::new()
                .name("klip-encoder-events".into())
                .spawn(move || {
                    if let Err(e) = event_loop(worker.clone(), on_packet) {
                        // Shutting the transform down is what ends this loop on purpose.
                        if !worker.stopping.load(Ordering::Acquire) {
                            eprintln!("klip-recorder: encoder stopped: {e}");
                        }
                    }
                })?;

            Ok(Self { inner })
        }
    }
}

impl Encoder for MfEncoder {
    fn name(&self) -> &str {
        &self.inner.name
    }

    fn submit(&self, nv12: &ID3D11Texture2D, pts: i64, duration: i64) -> Res<bool> {
        unsafe {
            let buffer = MFCreateDXGISurfaceBuffer(&ID3D11Texture2D::IID, nv12, 0, false)?;
            if let Ok(b2d) = buffer.cast::<IMF2DBuffer>() {
                if let Ok(len) = b2d.GetContiguousLength() {
                    let _ = buffer.SetCurrentLength(len);
                }
            }
            let sample = MFCreateSample()?;
            sample.AddBuffer(&buffer)?;
            sample.SetSampleTime(pts)?;
            sample.SetSampleDuration(duration)?;
            if self.inner.stopping.load(Ordering::Acquire) {
                return Ok(true);
            }
            let mut feed = self.inner.feed.lock().unwrap();
            if feed.need > 0 {
                feed.need -= 1;
                self.inner.mft.ProcessInput(0, &sample, 0)?;
                Ok(true)
            } else {
                Ok(feed.pending.replace(sample).is_none())
            }
        }
    }

    fn stream_type(&self) -> Option<StreamType> {
        self.inner.stream_type.lock().unwrap().clone()
    }

    fn frames_out(&self) -> u64 {
        self.inner.frames_out.load(Ordering::Relaxed)
    }

    fn shutdown(&self) {
        self.inner.stopping.store(true, Ordering::Release);
        self.inner.feed.lock().unwrap().pending = None;
        unsafe {
            if let Ok(s) = self.inner.mft.cast::<IMFShutdown>() {
                let _ = s.Shutdown();
            }
        }
    }

    fn force_keyframe(&self) {
        if let Some(api) = &self.inner.codec {
            let v = windows::Win32::System::Variant::VARIANT::from(1u32);
            unsafe {
                let _ = api.SetValue(&CODECAPI_AVEncVideoForceKeyFrame, &v);
            }
        }
    }
}

/// Picks the hardware encoder that belongs to the adapter the frames live on. A texture
/// from the NVIDIA card cannot be fed to the Intel encoder without a copy through RAM.
unsafe fn find_hardware_encoder(codec: VideoCodec, adapter: &str) -> Res<(IMFTransform, String)> {
    let input = MFT_REGISTER_TYPE_INFO { guidMajorType: MFMediaType_Video, guidSubtype: MFVideoFormat_NV12 };
    let output = MFT_REGISTER_TYPE_INFO { guidMajorType: MFMediaType_Video, guidSubtype: subtype(codec) };
    let mut list: *mut Option<IMFActivate> = std::ptr::null_mut();
    let mut count = 0u32;
    MFTEnumEx(
        MFT_CATEGORY_VIDEO_ENCODER,
        MFT_ENUM_FLAG_HARDWARE | MFT_ENUM_FLAG_SORTANDFILTER,
        Some(&input),
        Some(&output),
        &mut list,
        &mut count,
    )?;
    let vendor = ["NVIDIA", "Intel", "AMD"]
        .into_iter()
        .find(|v| adapter.to_uppercase().contains(&v.to_uppercase()))
        .unwrap_or("");

    let mut candidates = Vec::new();
    for i in 0..count as usize {
        if let Some(activate) = (*list.add(i)).take() {
            let mut name = PWSTR::null();
            let mut len = 0u32;
            let friendly = if activate.GetAllocatedString(&MFT_FRIENDLY_NAME_Attribute, &mut name, &mut len).is_ok() {
                let s = name.to_string().unwrap_or_default();
                CoTaskMemFree(Some(name.0 as *const _));
                s
            } else {
                String::from("unknown encoder")
            };
            candidates.push((activate, friendly));
        }
    }
    CoTaskMemFree(Some(list as *const _));

    let names: Vec<String> = candidates.iter().map(|c| c.1.clone()).collect();
    let pick = candidates
        .iter()
        .position(|c| !vendor.is_empty() && c.1.to_uppercase().contains(&vendor.to_uppercase()))
        .or(if candidates.is_empty() { None } else { Some(0) })
        .ok_or_else(|| format!("no hardware encoder for {codec:?} found"))?;
    let (activate, friendly) = &candidates[pick];
    let mft: IMFTransform = activate.ActivateObject().map_err(|e| {
        format!("could not start '{friendly}' (available: {names:?}): {e}")
    })?;
    Ok((mft, friendly.clone()))
}

fn event_loop<F: FnMut(Packet)>(inner: Arc<Inner>, mut on_packet: F) -> Res<()> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let events: IMFMediaEventGenerator = inner.mft.cast()?;
        loop {
            let event = events.GetEvent(MEDIA_EVENT_GENERATOR_GET_EVENT_FLAGS(0))?;
            let kind = event.GetType()?;
            if kind == METransformNeedInput.0 as u32 {
                let mut feed = inner.feed.lock().unwrap();
                match feed.pending.take() {
                    Some(sample) => inner.mft.ProcessInput(0, &sample, 0)?,
                    None => feed.need += 1,
                }
            } else if kind == METransformHaveOutput.0 as u32 {
                if let Some(packet) = take_output(&inner)? {
                    inner.frames_out.fetch_add(1, Ordering::Relaxed);
                    on_packet(packet);
                }
            }
        }
    }
}

unsafe fn take_output(inner: &Inner) -> Res<Option<Packet>> {
    let mut out = [MFT_OUTPUT_DATA_BUFFER {
        dwStreamID: 0,
        pSample: ManuallyDrop::new(None),
        dwStatus: 0,
        pEvents: ManuallyDrop::new(None),
    }];
    if !inner.provides_samples {
        let sample = MFCreateSample()?;
        sample.AddBuffer(&MFCreateMemoryBuffer(inner.out_buffer_size)?)?;
        out[0].pSample = ManuallyDrop::new(Some(sample));
    }
    let mut status = 0u32;
    let result = inner.mft.ProcessOutput(0, &mut out, &mut status);
    let sample = ManuallyDrop::take(&mut out[0].pSample);
    ManuallyDrop::drop(&mut out[0].pEvents);

    if let Err(e) = result {
        if e.code() == MF_E_TRANSFORM_STREAM_CHANGE {
            let t = inner.mft.GetOutputAvailableType(0, 0)?;
            inner.mft.SetOutputType(0, &t, 0)?;
            *inner.stream_type.lock().unwrap() = None;
            return Ok(None);
        }
        if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT {
            return Ok(None);
        }
        return Err(e.into());
    }
    let Some(sample) = sample else { return Ok(None) };

    {
        let mut st = inner.stream_type.lock().unwrap();
        if st.is_none() {
            *st = Some(StreamType(inner.mft.GetOutputCurrentType(0)?));
        }
    }

    let buffer = sample.ConvertToContiguousBuffer()?;
    let mut ptr = std::ptr::null_mut();
    let mut len = 0u32;
    buffer.Lock(&mut ptr, None, Some(&mut len))?;
    let data: Arc<[u8]> = Arc::from(std::slice::from_raw_parts(ptr, len as usize));
    buffer.Unlock()?;

    Ok(Some(Packet {
        data,
        pts: sample.GetSampleTime().unwrap_or(0),
        duration: sample.GetSampleDuration().unwrap_or(0),
        key: sample.GetUINT32(&MFSampleExtension_CleanPoint).unwrap_or(0) != 0,
    }))
}
