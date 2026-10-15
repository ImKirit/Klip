//! D3D11 device on the adapter that drives the captured monitor, and the GPU side
//! BGRA to NV12 conversion. Frames never leave the GPU here.

use crate::Res;
use std::collections::HashMap;
use std::mem::ManuallyDrop;
use windows::core::Interface;
use windows::Win32::Foundation::{HANDLE, HMODULE, RECT};
use windows::Win32::System::Threading::GetCurrentProcess;
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_11_1,
};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT_NV12, DXGI_RATIONAL, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIAdapter1, IDXGIDevice, IDXGIFactory1};
use windows::Win32::Graphics::Gdi::HMONITOR;

pub struct Gfx {
    pub device: ID3D11Device,
    pub context: ID3D11DeviceContext,
    pub adapter_name: String,
    pub monitor: HMONITOR,
}

/// Which monitor the device is created for.
pub enum MonitorPick {
    Primary,
    /// Position in the list printed by `--list`, primary first.
    Index(usize),
    /// The monitor a captured window is on.
    Handle(HMONITOR),
}

pub struct MonitorInfo {
    pub adapter: String,
    pub monitor: HMONITOR,
    pub rect: RECT,
}

/// All monitors of all adapters, the primary one first.
pub fn monitors() -> Res<Vec<MonitorInfo>> {
    Ok(enumerate()?.into_iter().map(|(_, info)| info).collect())
}

fn enumerate() -> Res<Vec<(IDXGIAdapter1, MonitorInfo)>> {
    unsafe {
        let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
        let mut all = Vec::new();
        let mut i = 0;
        while let Ok(adapter) = factory.EnumAdapters1(i) {
            i += 1;
            let name = {
                let d = adapter.GetDesc1()?;
                let len = d.Description.iter().position(|c| *c == 0).unwrap_or(d.Description.len());
                String::from_utf16_lossy(&d.Description[..len])
            };
            let mut j = 0;
            while let Ok(output) = adapter.EnumOutputs(j) {
                j += 1;
                let od = output.GetDesc()?;
                all.push((
                    adapter.clone(),
                    MonitorInfo { adapter: name.clone(), monitor: od.Monitor, rect: od.DesktopCoordinates },
                ));
            }
        }
        // The primary monitor always has its top left corner at the desktop origin.
        all.sort_by_key(|(_, m)| !(m.rect.left == 0 && m.rect.top == 0));
        Ok(all)
    }
}

/// Creates the device on the adapter the chosen monitor is attached to.
/// On hybrid systems the wrong adapter makes capture or the hardware encoder fail.
pub fn create(pick: MonitorPick) -> Res<Gfx> {
    unsafe {
        let mut all = enumerate()?;
        let index = match pick {
            MonitorPick::Primary => 0,
            MonitorPick::Index(i) => i,
            MonitorPick::Handle(h) => all.iter().position(|(_, m)| m.monitor == h).unwrap_or(0),
        };
        if index >= all.len() {
            return Err(format!("monitor {index} does not exist, there are {}", all.len()).into());
        }
        let (adapter, info) = all.swap_remove(index);
        let (monitor, adapter_name) = (info.monitor, info.adapter);

        let mut device = None;
        let mut context = None;
        D3D11CreateDevice(
            &adapter,
            D3D_DRIVER_TYPE_UNKNOWN,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
            Some(&[D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_11_0]),
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut context),
        )?;
        let device = device.ok_or("D3D11CreateDevice returned no device")?;
        let context = context.ok_or("D3D11CreateDevice returned no context")?;

        // Capture callback, encoder and video processor touch the context from different threads.
        let mt: ID3D11Multithread = context.cast()?;
        let _ = mt.SetMultithreadProtected(true);
        raise_gpu_priority(&device);

        Ok(Gfx {
            device,
            context,
            adapter_name,
            monitor,
        })
    }
}

/// A game that keeps the GPU at its limit makes capture and encoder wait for their turn:
/// the game runs smoothly, the clip stutters. Like OBS, Klip asks the GPU scheduler to
/// prefer its own work. This touches only Klip's process, never the game.
fn raise_gpu_priority(device: &ID3D11Device) {
    #[link(name = "gdi32")]
    extern "system" {
        fn D3DKMTSetProcessSchedulingPriorityClass(process: HANDLE, class: i32) -> i32;
    }
    const D3DKMT_SCHEDULINGPRIORITYCLASS_HIGH: i32 = 4;
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| unsafe {
        let status = D3DKMTSetProcessSchedulingPriorityClass(GetCurrentProcess(), D3DKMT_SCHEDULINGPRIORITYCLASS_HIGH);
        if status != 0 {
            eprintln!("klip-recorder: GPU priority not raised (status {status:#x})");
        }
    });
    unsafe {
        if let Ok(dxgi) = device.cast::<IDXGIDevice>() {
            let _ = dxgi.SetGPUThreadPriority(7);
        }
    }
}

/// Number of NV12 textures handed to the encoder in rotation. The async encoder keeps a
/// texture for a few frames and up to three more wait in front of it, so the pool must be
/// deeper than both together.
const POOL: usize = 12;

pub struct Converter {
    video_device: ID3D11VideoDevice,
    video_context: ID3D11VideoContext,
    enumerator: ID3D11VideoProcessorEnumerator,
    processor: ID3D11VideoProcessor,
    outputs: Vec<(ID3D11Texture2D, ID3D11VideoProcessorOutputView)>,
    next: usize,
    // Capture hands out a small fixed set of textures, so views are created once per texture.
    inputs: HashMap<usize, ID3D11VideoProcessorInputView>,
}

// SAFETY: the device context is multithread protected and Converter is only used behind a Mutex.
unsafe impl Send for Converter {}

impl Converter {
    /// `crop` is the part of the input to convert: left, top, width, height.
    pub fn new(
        gfx: &Gfx,
        in_w: u32,
        in_h: u32,
        out_w: u32,
        out_h: u32,
        fps: u32,
        crop: Option<[u32; 4]>,
    ) -> Res<Self> {
        unsafe {
            let video_device: ID3D11VideoDevice = gfx.device.cast()?;
            let video_context: ID3D11VideoContext = gfx.context.cast()?;
            let rate = DXGI_RATIONAL { Numerator: fps, Denominator: 1 };
            let desc = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
                InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
                InputFrameRate: rate,
                InputWidth: in_w,
                InputHeight: in_h,
                OutputFrameRate: rate,
                OutputWidth: out_w,
                OutputHeight: out_h,
                Usage: D3D11_VIDEO_USAGE_PLAYBACK_NORMAL,
            };
            let enumerator = video_device.CreateVideoProcessorEnumerator(&desc)?;
            let processor = video_device.CreateVideoProcessor(&enumerator, 0)?;

            // The defaults are BT.601 full range. Screen content is full range RGB in,
            // BT.709 limited range out, which is what H.264 and HEVC players assume.
            let input_space = D3D11_VIDEO_PROCESSOR_COLOR_SPACE { _bitfield: 0 };
            // bit 2: YCbCr_Matrix = BT.709, bits 4..5: Nominal_Range = 16 to 235
            let output_space = D3D11_VIDEO_PROCESSOR_COLOR_SPACE { _bitfield: (1 << 2) | (1 << 4) };
            video_context.VideoProcessorSetStreamColorSpace(&processor, 0, &input_space);
            video_context.VideoProcessorSetOutputColorSpace(&processor, &output_space);
            if let Some([x, y, w, h]) = crop {
                let rect = RECT { left: x as i32, top: y as i32, right: (x + w) as i32, bottom: (y + h) as i32 };
                video_context.VideoProcessorSetStreamSourceRect(&processor, 0, true, Some(&rect));
            }
            video_context.VideoProcessorSetStreamFrameFormat(
                &processor,
                0,
                D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
            );
            // Driver side "enhancements" (denoise, edge enhancement) would alter the picture.
            video_context.VideoProcessorSetStreamAutoProcessingMode(&processor, 0, false);

            let mut outputs = Vec::with_capacity(POOL);
            for _ in 0..POOL {
                let td = D3D11_TEXTURE2D_DESC {
                    Width: out_w,
                    Height: out_h,
                    MipLevels: 1,
                    ArraySize: 1,
                    Format: DXGI_FORMAT_NV12,
                    SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                    Usage: D3D11_USAGE_DEFAULT,
                    BindFlags: (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32,
                    CPUAccessFlags: 0,
                    MiscFlags: 0,
                };
                let mut tex = None;
                gfx.device.CreateTexture2D(&td, None, Some(&mut tex))?;
                let tex: ID3D11Texture2D = tex.ok_or("CreateTexture2D returned nothing")?;
                let vd = D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
                    ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
                    Anonymous: D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0 {
                        Texture2D: D3D11_TEX2D_VPOV { MipSlice: 0 },
                    },
                };
                let mut view = None;
                video_device.CreateVideoProcessorOutputView(&tex, &enumerator, &vd, Some(&mut view))?;
                outputs.push((tex, view.ok_or("no output view")?));
            }

            Ok(Self {
                video_device,
                video_context,
                enumerator,
                processor,
                outputs,
                next: 0,
                inputs: HashMap::new(),
            })
        }
    }

    /// Converts one BGRA capture texture and returns the NV12 texture holding the result.
    pub fn convert(&mut self, bgra: &ID3D11Texture2D) -> Res<ID3D11Texture2D> {
        unsafe {
            let key = bgra.as_raw() as usize;
            if !self.inputs.contains_key(&key) {
                if self.inputs.len() > 16 {
                    // The capture pool was recreated (size change), old views are stale.
                    self.inputs.clear();
                }
                let vd = D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC {
                    FourCC: 0,
                    ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D,
                    Anonymous: D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0 {
                        Texture2D: D3D11_TEX2D_VPIV { MipSlice: 0, ArraySlice: 0 },
                    },
                };
                let mut view = None;
                self.video_device.CreateVideoProcessorInputView(
                    bgra,
                    &self.enumerator,
                    &vd,
                    Some(&mut view),
                )?;
                self.inputs.insert(key, view.ok_or("no input view")?);
            }
            let input = self.inputs[&key].clone();
            let (tex, out_view) = &self.outputs[self.next];
            self.next = (self.next + 1) % self.outputs.len();

            let mut stream = D3D11_VIDEO_PROCESSOR_STREAM {
                Enable: true.into(),
                pInputSurface: ManuallyDrop::new(Some(input)),
                ..Default::default()
            };
            let result = self.video_context.VideoProcessorBlt(
                &self.processor,
                out_view,
                0,
                std::slice::from_ref(&stream),
            );
            ManuallyDrop::drop(&mut stream.pInputSurface);
            result?;
            Ok(tex.clone())
        }
    }
}
