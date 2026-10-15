//! The recording chain as one unit that can be started and stopped: capture, conversion,
//! encoder, audio, replay buffer, input track. "Buffer only while a game runs" means
//! this unit exists only for that time, and at idle the recorder holds no GPU resources.

use crate::audio;
use crate::camera;
use crate::capture::{self, Capture, CaptureStats};
use crate::encoder::{Encoder, EncoderConfig, MfEncoder, StreamType};
use crate::gfx;
use crate::mux;
use crate::record::Recording;
use crate::ring::Ring;
use crate::shot;
use crate::{free_bytes, new_path, qpc_100ns, stamp, CpuMeter, Res};
use klip_core::{CameraTrack, ClipEvent, ClipKind, ClipMeta, Config, HotkeyAction, RecorderStatus, VideoCodec, SIDECAR_VERSION};
use klip_input::{InputCapture, InputClip, LayoutKind, OverlayLayout, Preset};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Direct3D11::{ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D};
use windows::Win32::Graphics::Gdi::{MonitorFromWindow, HMONITOR, MONITOR_DEFAULTTONEAREST};

/// A recording stops by itself below this much free space, so the file can still be closed.
const MIN_FREE_BYTES: u64 = 2 << 30;

/// The game the current footage belongs to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GameInfo {
    /// Key of the game for profiles, from the game catalog.
    pub id: String,
    pub name: String,
    pub exe_name: String,
}

#[derive(Clone, Copy)]
pub enum SourcePick {
    Primary,
    Monitor(usize),
    /// Monitor handle as a number, the monitor a game window is on.
    MonitorHandle(isize),
    /// Window handle as a number.
    Window(isize),
    /// Every monitor of the main graphics card, side by side in one picture.
    All,
}

/// The widest picture the hardware encoders take (NVENC, Quick Sync and AMF all stop at
/// 4096 for H.264). A wider desktop is scaled down to fit.
const MAX_ENCODE_WIDTH: u32 = 4096;

/// Called with path and length in seconds after the chain saved a clip on its own.
pub type Notify = Arc<dyn Fn(PathBuf, u32) + Send + Sync>;

pub struct Options {
    pub cfg: Config,
    pub out_dir: PathBuf,
    pub source: SourcePick,
    pub notify: Option<Notify>,
    /// Test input: feed this video file instead of capturing the screen.
    pub replay: Option<PathBuf>,
}

/// What the capture thread hands to the detector thread.
enum DetMsg {
    /// The game changed. `None` means no game, detection rests.
    Game(Option<String>),
    /// Cut outs of one frame: time in milliseconds, frame size, and the regions by id.
    Frame(u64, u32, u32, Vec<(u32, crate::tap::Region)>),
    /// Test mode only: pretend the detector just reported a kill.
    TestEvent,
    /// The source ended (replay): write every clip that is still being planned.
    Flush,
}

/// Replay buffers and the running recording. Fed by the encoder and audio threads, read
/// when a clip is saved. One lock keeps a recording and its head start consistent.
struct Shared {
    ring: Ring,
    /// One buffer per audio track, same length as the video buffer.
    audio: Vec<Ring>,
    recording: Option<Recording>,
    /// The webcam: its buffer and, during a manual recording, its own file.
    cam: Option<Ring>,
    cam_recording: Option<Recording>,
}

/// The webcam from picture to packet: upload, conversion, an encoder of its own.
struct CamChain {
    texture: ID3D11Texture2D,
    context: ID3D11DeviceContext,
    converter: Mutex<gfx::Converter>,
    enc: Arc<dyn Encoder>,
    width: u32,
    height: u32,
}

// SAFETY: the device context is multithread protected, the converter sits behind a lock.
unsafe impl Send for CamChain {}
unsafe impl Sync for CamChain {}

impl CamChain {
    fn new(gfx: &gfx::Gfx, width: u32, height: u32, shared: Arc<Mutex<Shared>>) -> Res<Self> {
        let desc = windows::Win32::Graphics::Direct3D11::D3D11_TEXTURE2D_DESC {
            Width: width,
            Height: height,
            MipLevels: 1,
            ArraySize: 1,
            Format: windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: windows::Win32::Graphics::Direct3D11::D3D11_USAGE_DEFAULT,
            // The converter needs the same flags a captured frame has.
            BindFlags: (windows::Win32::Graphics::Direct3D11::D3D11_BIND_RENDER_TARGET.0
                | windows::Win32::Graphics::Direct3D11::D3D11_BIND_SHADER_RESOURCE.0) as u32,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        let mut texture = None;
        unsafe { gfx.device.CreateTexture2D(&desc, None, Some(&mut texture))? };
        let texture = texture.ok_or("CreateTexture2D returned nothing")?;
        let converter = gfx::Converter::new(gfx, width, height, width, height, camera::FPS, None)?;
        // H.264 always: the camera file must play everywhere, and its size is small.
        let cfg = EncoderConfig { width, height, fps: camera::FPS, bitrate_kbps: 3000, codec: VideoCodec::H264, gop_seconds: 2 };
        let enc: Arc<dyn Encoder> = Arc::new(MfEncoder::new(gfx, cfg, move |p| {
            let mut s = shared.lock().unwrap();
            if let Some(rec) = &mut s.cam_recording {
                if let Err(e) = rec.push(&p) {
                    eprintln!("klip-recorder: camera recording lost a frame: {e}");
                }
            }
            if let Some(ring) = &mut s.cam {
                ring.push(p);
            }
        })?);
        Ok(Self { texture, context: gfx.context.clone(), converter: Mutex::new(converter), enc, width, height })
    }

    fn push(&self, bgra: &[u8], width: u32, height: u32, time: i64) {
        if width != self.width || height != self.height {
            return;
        }
        unsafe { self.context.UpdateSubresource(&self.texture, 0, None, bgra.as_ptr() as *const _, width * 4, 0) };
        let nv12 = self.converter.lock().unwrap().convert(&self.texture);
        if let Ok(nv12) = nv12 {
            let _ = self.enc.submit(&nv12, time, 10_000_000 / camera::FPS as i64);
        }
    }
}

pub struct Pipeline {
    shared: Arc<Mutex<Shared>>,
    enc: Arc<dyn Encoder>,
    enc_cfg: EncoderConfig,
    out_dir: PathBuf,
    want_shot: Arc<AtomicBool>,
    /// Audio tracks in file order. The list grows while programs start to play sound.
    audio_tracks: Option<Arc<Mutex<Vec<audio::TrackInfo>>>>,
    game_exe: Option<audio::GameExe>,
    mic_muted: Option<Arc<AtomicBool>>,
    /// Layout of the input overlay, and whether a recognised game has the focus.
    layout: OverlayLayout,
    foreground: AtomicBool,
    input_paused: AtomicBool,
    buffer_seconds: u32,
    max_recording_ms: u64,
    /// Path of the clip or recording written last, shown by the app.
    last_saved: Mutex<Option<String>>,
    game: Arc<Mutex<Option<GameInfo>>>,
    input: Option<InputCapture>,
    stop: Arc<AtomicBool>,
    /// One capture per monitor that is recorded, empty for a replay.
    capture: Mutex<Vec<Capture>>,
    cam_chain: Arc<OnceLock<CamChain>>,
    /// The running camera. Dropping it ends the reading thread.
    camera: Mutex<Option<camera::Feed>>,
    cap_stats: Arc<CaptureStats>,
    replay_done: Arc<AtomicBool>,
    detector: Option<std::sync::mpsc::Sender<DetMsg>>,
    notify: Option<Notify>,
    dropped: Arc<AtomicU64>,
    counters: Mutex<(u64, u64, u64, Instant)>,
}
// SAFETY: every member is either free threaded (Media Foundation, WinRT capture objects,
// a multithread protected D3D context) or sits behind a Mutex.
unsafe impl Send for Pipeline {}
unsafe impl Sync for Pipeline {}

impl Pipeline {
    pub fn start(opts: Options) -> Res<Arc<Self>> {
        let (gfx, source) = match opts.source {
            SourcePick::Primary => {
                let g = gfx::create(gfx::MonitorPick::Primary)?;
                let m = g.monitor;
                (g, capture::Source::Monitor(m))
            }
            SourcePick::Monitor(i) => {
                let g = gfx::create(gfx::MonitorPick::Index(i))?;
                let m = g.monitor;
                (g, capture::Source::Monitor(m))
            }
            SourcePick::MonitorHandle(h) => {
                let g = gfx::create(gfx::MonitorPick::Handle(HMONITOR(h as *mut _)))?;
                let m = g.monitor;
                (g, capture::Source::Monitor(m))
            }
            SourcePick::Window(h) => {
                let hwnd = HWND(h as *mut _);
                let monitor = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
                (gfx::create(gfx::MonitorPick::Handle(monitor))?, capture::Source::Window(hwnd))
            }
            SourcePick::All => {
                let g = gfx::create(gfx::MonitorPick::Primary)?;
                let m = g.monitor;
                (g, capture::Source::Monitor(m))
            }
        };
        println!("adapter: {}", gfx.adapter_name);
        // All monitors: those on the adapter of the device, the others cannot be copied
        // onto the same texture.
        let multi = match opts.source {
            SourcePick::All if opts.replay.is_none() => {
                let (here, elsewhere): (Vec<_>, Vec<_>) =
                    gfx::monitors()?.into_iter().partition(|m| m.adapter == gfx.adapter_name);
                for m in &elsewhere {
                    println!("monitors: one on {} is left out, it is on another graphics card", m.adapter);
                }
                Some(here)
            }
            _ => None,
        };
        let replay = match &opts.replay {
            Some(path) => Some((path.clone(), crate::replay::probe(path)?)),
            None => None,
        };
        let mut prepared = if replay.is_none() && multi.is_none() { Some(capture::prepare(&gfx, source)?) } else { None };
        let (src_w, src_h) = match (&prepared, &replay, &multi) {
            (Some(p), _, _) => (p.width, p.height),
            (None, Some((_, size)), _) => *size,
            (None, None, Some(list)) => {
                let (w, h, _) = crate::composite::layout(list);
                println!("monitors: {} side by side", list.len());
                (w, h)
            }
            (None, None, None) => unreachable!(),
        };
        println!("source: {src_w}x{src_h}");
        // A minimised window reports a tiny size that no encoder accepts.
        if src_w < 160 || src_h < 120 {
            return Err(format!("the source is only {src_w}x{src_h}, is the window minimised?").into());
        }

        let cfg = &opts.cfg;
        // Optional downscale on the GPU. Encoders want even dimensions.
        // A region narrows the picture to a part of the source. It is clipped to the
        // source so a stale setting from another monitor cannot break the start.
        let crop = cfg.region.and_then(|[x, y, w, h]| {
            let (x, y) = (x.min(src_w.saturating_sub(2)), y.min(src_h.saturating_sub(2)));
            let (w, h) = (w.min(src_w - x) & !1, h.min(src_h - y) & !1);
            (w >= 64 && h >= 64).then_some([x, y, w, h])
        });
        let (pic_w, pic_h) = crop.map(|c| (c[2], c[3])).unwrap_or((src_w, src_h));
        if crop.is_some() {
            println!("region: {pic_w}x{pic_h}");
        }
        let (out_w, out_h) = match cfg.height {
            Some(h) if h < pic_h => {
                let h = h & !1;
                ((pic_w as u64 * h as u64 / pic_h as u64) as u32 & !1, h)
            }
            _ => (pic_w & !1, pic_h & !1),
        };
        let (out_w, out_h) = if out_w > MAX_ENCODE_WIDTH {
            (MAX_ENCODE_WIDTH, (out_h as u64 * MAX_ENCODE_WIDTH as u64 / out_w as u64) as u32 & !1)
        } else {
            (out_w, out_h)
        };
        let enc_cfg = EncoderConfig {
            width: out_w,
            height: out_h,
            fps: cfg.fps,
            bitrate_kbps: cfg.bitrate_kbps,
            codec: cfg.codec,
            gop_seconds: 2,
        };

        let ring = if cfg.buffer_on_disk {
            Ring::on_disk(cfg.buffer_seconds, cfg.cache_dir().join("buffer"))?
        } else {
            Ring::in_memory(cfg.buffer_seconds)
        };
        println!(
            "buffer: {} s {}",
            cfg.buffer_seconds,
            if ring.is_on_disk() { "on disk".to_string() } else { "in RAM".to_string() }
        );
        let shared = Arc::new(Mutex::new(Shared { ring, audio: Vec::new(), recording: None, cam: None, cam_recording: None }));
        let dropped = Arc::new(AtomicU64::new(0));
        let stop = Arc::new(AtomicBool::new(false));

        let sink = shared.clone();
        let enc: Arc<dyn Encoder> = Arc::new(MfEncoder::new(&gfx, enc_cfg, move |p| {
            let mut s = sink.lock().unwrap();
            if let Some(rec) = &mut s.recording {
                if let Err(e) = rec.push(&p) {
                    eprintln!("klip-recorder: recording lost a frame: {e}");
                }
            }
            s.ring.push(p);
        })?);
        println!("encoder: {} ({:?}, {} kbit/s, {} fps)", enc.name(), enc_cfg.codec, enc_cfg.bitrate_kbps, enc_cfg.fps);

        let (audio_tracks, game_exe, mic_muted) = if cfg.audio.enabled {
            let sink = shared.clone();
            // Tracks can appear at any time, each one gets its buffer with its first packet.
            let (on_disk, seconds, buffer_dir) = (cfg.buffer_on_disk, cfg.buffer_seconds, cfg.cache_dir().join("buffer"));
            let started = audio::start(&cfg.audio, cfg.buffer_seconds, stop.clone(), move |track, p| {
                let mut s = sink.lock().unwrap();
                while s.audio.len() <= track {
                    let dir = buffer_dir.join(format!("audio{}", s.audio.len()));
                    let ring = if on_disk { Ring::on_disk(seconds, dir).ok() } else { None };
                    s.audio.push(ring.unwrap_or_else(|| Ring::in_memory(seconds)));
                }
                if let Some(rec) = &mut s.recording {
                    if let Err(e) = rec.push_audio(track, &p) {
                        eprintln!("klip-recorder: recording lost audio: {e}");
                    }
                }
                s.audio[track].push(p);
            })?;
            let names: Vec<String> = started.tracks.lock().unwrap().iter().map(|t| t.name.clone()).collect();
            println!(
                "audio: {}{}",
                names.join(", "),
                if cfg.audio.mode == klip_core::AudioMode::All { ", plus every program that plays sound" } else { "" }
            );
            (Some(started.tracks), Some(started.game), Some(started.mic_muted))
        } else {
            println!("audio: off");
            (None, None, None)
        };

        // Key and mouse button states for the input overlay. Starts switched off: it only
        // stores anything while `set_game` says a recognised game has the focus.
        let layout = load_layout(cfg.input.layout.as_deref());
        let input = if cfg.input.enabled {
            match InputCapture::start(&layout, Duration::from_secs(cfg.buffer_seconds as u64 + 5)) {
                Ok(c) => {
                    println!("input: layout '{}' ready, waiting for a game", layout.name);
                    Some(c)
                }
                Err(e) => {
                    eprintln!("klip-recorder: input capture not available: {e}");
                    None
                }
            }
        } else {
            None
        };

        let converter = Mutex::new(gfx::Converter::new(&gfx, src_w, src_h, out_w, out_h, enc_cfg.fps, crop)?);
        let frame_duration = 10_000_000 / enc_cfg.fps as i64;
        // The newest converted frame and its time, kept for the heartbeat below.
        let last_frame: Arc<Mutex<Option<(SendTexture, i64)>>> = Arc::new(Mutex::new(None));
        let want_shot = Arc::new(AtomicBool::new(false));
        let game: Arc<Mutex<Option<GameInfo>>> = Arc::new(Mutex::new(None));

        // Event detection for auto clips. The capture thread only cuts the requested
        // regions out of the frame, a worker thread looks at them.
        let det_regions: Arc<Mutex<Vec<klip_detect::RegionRequest>>> = Arc::new(Mutex::new(Vec::new()));
        let (det_tx, det_rx) = std::sync::mpsc::channel::<DetMsg>();
        // The detectors know the picture of one monitor. On the wide picture of all
        // monitors their regions would point at the wrong places.
        let cap_det = (cfg.auto_clips && multi.is_none()).then(|| (det_regions.clone(), det_tx.clone()));
        if cfg.auto_clips && multi.is_some() {
            println!("detect: auto clips need a single monitor, they rest while all are recorded");
        }
        // Tap and time of the last sample. Behind a lock because the capture callback is shared.
        let tap = Mutex::new((crate::tap::RegionTap::new(&gfx.device, &gfx.context), 0i64));

        let cap_enc = enc.clone();
        let cap_dropped = dropped.clone();
        let cap_last = last_frame.clone();
        let cap_shot = want_shot.clone();
        let cap_gpu = SendGpu(gfx.device.clone(), gfx.context.clone());
        let cap_out = opts.out_dir.clone();
        let cap_game = game.clone();
        let on_frame = move |texture: &ID3D11Texture2D, time: i64| {
            if cap_shot.swap(false, Ordering::AcqRel) {
                let name = cap_game.lock().unwrap().as_ref().map(|g| g.name.clone());
                take_screenshot(&cap_gpu, texture, &game_dir(&cap_out, name.as_deref()), name);
            }
            if let Some((regions, tx)) = &cap_det {
                let regions = regions.lock().unwrap();
                let rate = regions.iter().map(|r| r.rate_hz).fold(0.0f32, f32::max);
                let mut tap = tap.lock().unwrap();
                if rate > 0.0 && time - tap.1 >= (10_000_000.0 / rate) as i64 {
                    tap.1 = time;
                    let cut: Vec<_> = regions
                        .iter()
                        .filter_map(|r| {
                            let p = r.pixel_rect(src_w, src_h);
                            let rect = crate::tap::Rect { x: p.x, y: p.y, w: p.w, h: p.h };
                            tap.0.read(texture, rect).ok().map(|region| (r.id, region))
                        })
                        .collect();
                    let _ = tx.send(DetMsg::Frame((time / 10_000) as u64, src_w, src_h, cut));
                }
            }
            let mut last = cap_last.lock().unwrap();
            // A heartbeat frame may carry a later time than a frame that was already on its way.
            if last.as_ref().is_some_and(|(_, t)| time <= *t) {
                return;
            }
            let result = converter
                .lock()
                .unwrap()
                .convert(texture)
                .and_then(|nv12| cap_enc.submit(&nv12, time, frame_duration).map(|kept| (nv12, kept)));
            match result {
                Ok((nv12, kept)) => {
                    if !kept {
                        cap_dropped.fetch_add(1, Ordering::Relaxed);
                    }
                    *last = Some((SendTexture(nv12), time));
                }
                Err(e) => {
                    cap_dropped.fetch_add(1, Ordering::Relaxed);
                    eprintln!("klip-recorder: frame lost: {e}");
                }
            }
        };
        let replay_done = Arc::new(AtomicBool::new(false));
        let (cap, cap_stats) = match (&replay, prepared.take(), &multi) {
            (Some((path, _)), _, _) => {
                let stats = Arc::new(CaptureStats::default());
                crate::replay::start(&gfx, path, src_w, src_h, stop.clone(), replay_done.clone(), stats.clone(), on_frame)?;
                println!("replay: {} at {src_w}x{src_h}, encoding {out_w}x{out_h}", path.display());
                (Vec::new(), stats)
            }
            (None, Some(prepared), _) => {
                let cap = capture::start(prepared, enc_cfg.fps, cfg.capture_cursor, on_frame)?;
                println!("capture: {}x{} running, encoding {}x{}", cap.width, cap.height, out_w, out_h);
                let stats = cap.stats.clone();
                (vec![cap], stats)
            }
            (None, None, Some(list)) => {
                let all = crate::composite::start(&gfx, list, enc_cfg.fps, cfg.capture_cursor, on_frame)?;
                println!("capture: {} monitors as {}x{} running, encoding {}x{}", all.captures.len(), all.width, all.height, out_w, out_h);
                (all.captures, all.stats)
            }
            (None, None, None) => unreachable!(),
        };

        // Heartbeat: Windows delivers no frames while the picture stands still. Without this
        // a clip cut from a quiet stretch would hold a single frame. Once a second the last
        // picture is encoded again, which costs next to nothing because nothing changed.
        {
            let enc = enc.clone();
            let stop = stop.clone();
            std::thread::spawn(move || {
                while !stop.load(Ordering::Acquire) {
                    std::thread::sleep(Duration::from_millis(250));
                    let now = qpc_100ns();
                    let mut last = last_frame.lock().unwrap();
                    let Some((texture, time)) = last.as_mut() else { continue };
                    if now - *time < 10_000_000 {
                        continue;
                    }
                    // Each of these is a keyframe, so clips and resumed recordings can start
                    // within a second instead of within a minute.
                    enc.force_keyframe();
                    if enc.submit(&texture.0, now, frame_duration).is_ok() {
                        *time = now;
                    }
                }
                *last_frame.lock().unwrap() = None;
            });
        }

        // The webcam, if wanted. It gets its own converter, encoder and buffer, and every
        // clip a camera file of its own. A camera that does not open costs the clip nothing.
        let cam_chain: Arc<OnceLock<CamChain>> = Arc::new(OnceLock::new());
        let camera = if cfg.camera.enabled && opts.replay.is_none() {
            let chain = cam_chain.clone();
            let started = camera::start(cfg.camera.device.as_deref(), Arc::new(AtomicBool::new(false)), move |bgra, w, h, time| {
                if let Some(c) = chain.get() {
                    c.push(bgra, w, h, time);
                }
            });
            match started.and_then(|feed| {
                let c = CamChain::new(&gfx, feed.width, feed.height, shared.clone())?;
                Ok((feed, c))
            }) {
                Ok((feed, c)) => {
                    let ring = if cfg.buffer_on_disk {
                        Ring::on_disk(cfg.buffer_seconds, cfg.cache_dir().join("buffer").join("camera")).unwrap_or_else(|_| Ring::in_memory(cfg.buffer_seconds))
                    } else {
                        Ring::in_memory(cfg.buffer_seconds)
                    };
                    shared.lock().unwrap().cam = Some(ring);
                    let _ = cam_chain.set(c);
                    Some(feed)
                }
                Err(e) => {
                    eprintln!("klip-recorder: camera not available: {e}");
                    None
                }
            }
        } else {
            None
        };

        let detect = cfg.auto_clips;
        let this = Arc::new(Self {
            detector: detect.then_some(det_tx),
            notify: opts.notify,
            shared,
            enc,
            enc_cfg,
            out_dir: opts.out_dir,
            want_shot,
            audio_tracks,
            game_exe,
            mic_muted,
            layout,
            foreground: AtomicBool::new(false),
            input_paused: AtomicBool::new(false),
            buffer_seconds: cfg.buffer_seconds,
            max_recording_ms: cfg.max_recording_minutes as u64 * 60_000,
            last_saved: Mutex::new(None),
            game,
            input,
            stop,
            cap_stats,
            capture: Mutex::new(cap),
            cam_chain,
            camera: Mutex::new(camera),
            replay_done,
            dropped,
            counters: Mutex::new((0, 0, 0, Instant::now())),
        });

        if detect {
            let weak = Arc::downgrade(&this);
            std::thread::Builder::new().name("klip-detect".into()).spawn(move || {
                let registry = klip_detect::Registry::builtin();
                let rules: klip_detect::RuleSet = std::fs::read(klip_core::data_dir().join("autoclip.json"))
                    .ok()
                    .and_then(|b| serde_json::from_slice(&b).ok())
                    .unwrap_or_default();
                let mut planner = klip_detect::ClipPlanner::new(rules);
                let mut detector: Option<Box<dyn klip_detect::Detector>> = None;
                let (mut last_t, mut samples, mut busy) = (0u64, 0u64, Duration::ZERO);
                for msg in det_rx {
                    match msg {
                        DetMsg::Flush => {
                            for job in planner.take_ready(u64::MAX) {
                                let Some(p) = weak.upgrade() else { return };
                                if let Err(e) = p.save_auto_clip(&job) {
                                    eprintln!("klip-recorder: auto clip not saved: {e}");
                                }
                            }
                        }
                        DetMsg::TestEvent => {
                            println!(
                                "detect: test event at {last_t} ms, {samples} samples so far, {:.2} ms each",
                                busy.as_secs_f64() * 1000.0 / samples.max(1) as f64
                            );
                            planner.push(klip_detect::Event::new(klip_detect::EventKind::Kill, last_t, 1.0));
                        }
                        DetMsg::Game(exe) => {
                            detector = exe.and_then(|e| registry.for_exe(&e));
                            planner.clear();
                            *det_regions.lock().unwrap() = detector.as_ref().map(|d| d.regions()).unwrap_or_default();
                            if let Some(d) = &detector {
                                println!("detect: {} active", d.display_name());
                            }
                        }
                        DetMsg::Frame(t_ms, w, h, cut) => {
                            let Some(det) = &mut detector else { continue };
                            let images: Vec<_> = cut
                                .iter()
                                .map(|(id, r)| klip_detect::RegionImage {
                                    id: *id,
                                    data: &r.data,
                                    width: r.rect.w,
                                    height: r.rect.h,
                                    stride: r.rect.w as usize * 4,
                                })
                                .collect();
                            let sample = klip_detect::Sample { t_ms, frame_width: w, frame_height: h, regions: &images };
                            let started = Instant::now();
                            let events = det.process(&sample);
                            busy += started.elapsed();
                            samples += 1;
                            last_t = t_ms;
                            for event in events {
                                println!("detect: {} at {} ms", event.kind.id(), event.t_ms);
                                planner.push(event);
                            }
                            for job in planner.take_ready(t_ms) {
                                let Some(p) = weak.upgrade() else { return };
                                if let Err(e) = p.save_auto_clip(&job) {
                                    eprintln!("klip-recorder: auto clip not saved: {e}");
                                }
                            }
                        }
                    }
                }
            })?;
        }
        Ok(this)
    }

    /// Writes the stretch a detector asked for as a clip of its own. The events go into
    /// the sidecar file and nowhere else: nothing about them is shown during a match.
    fn save_auto_clip(&self, job: &klip_detect::ClipJob) -> Res<()> {
        let (from, to) = (job.start_ms as i64 * 10_000, job.end_ms as i64 * 10_000);
        let (packets, audio) = {
            let mut s = self.shared.lock().unwrap();
            let packets = s.ring.range(from, to);
            let first = packets.first().map(|p| p.pts).unwrap_or(0);
            let audio: Vec<_> = self
                .audio_streams()
                .into_iter()
                .zip(s.audio.iter_mut().map(|ring| ring.since(first)))
                .collect();
            (packets, audio)
        };
        let first = packets.first().ok_or("the stretch is no longer in the buffer")?.pts;
        let stream = self.enc.stream_type().ok_or("the encoder has not produced a frame yet")?;
        let path = new_path(&self.dir(), &format!("_{}", job.label.id()), "mp4")?;
        let ms = mux::write_mp4(&path, &stream, &packets, &audio)?;
        let events = job
            .events
            .iter()
            .map(|e| ClipEvent {
                at_ms: (e.t_ms as i64 - first / 10_000).max(0) as u64,
                kind: e.kind.id().to_string(),
                label: None,
            })
            .collect();
        let mut meta = self.meta(ClipKind::AutoClip, ms, events);
        meta.reason = Some(job.label.id().to_string());
        meta.camera = self.save_camera(&path, first, first + ms as i64 * 10_000);
        meta.write_next_to(&path)?;
        self.write_inputs(&path, first, first + ms as i64 * 10_000);
        *self.last_saved.lock().unwrap() = Some(path.display().to_string());
        println!("AUTO saved={} duration={:.2}s event={}", path.display(), ms as f64 / 1000.0, job.label.id());
        if let Some(notify) = &self.notify {
            notify(path, (ms / 1000) as u32);
        }
        Ok(())
    }

    /// Tells the chain which game the footage belongs to and whether it has the focus.
    /// Input is only stored while a recognised game is in the foreground.
    pub fn set_game(&self, game: Option<GameInfo>, foreground: bool) {
        if let Some(exe) = &self.game_exe {
            *exe.lock().unwrap() = game.as_ref().map(|g| g.exe_name.clone());
        }
        if let Some(tx) = &self.detector {
            let changed = self.game.lock().unwrap().as_ref().map(|g| &g.exe_name) != game.as_ref().map(|g| &g.exe_name);
            if changed {
                let _ = tx.send(DetMsg::Game(game.as_ref().map(|g| g.exe_name.clone())));
            }
        }
        if let Some(input) = &self.input {
            input.set_game_active(game.is_some() && foreground);
        }
        self.foreground.store(game.is_some() && foreground, Ordering::Release);
        *self.game.lock().unwrap() = game;
    }

    /// Test mode only: feeds a made up kill into the auto clip planner.
    pub fn inject_test_event(&self) {
        if let Some(tx) = &self.detector {
            let _ = tx.send(DetMsg::TestEvent);
        }
    }

    /// Layout and currently held keys for the live input overlay. `None` while no
    /// recognised game has the focus, so nothing is ever shown outside of games.
    pub fn live_input(&self) -> Option<(OverlayLayout, klip_input::Held)> {
        let input = self.input.as_ref()?;
        self.foreground.load(Ordering::Acquire).then(|| (self.layout.clone(), input.held()))
    }

    /// Mutes or unmutes the microphone track. Returns true if it is muted afterwards.
    pub fn toggle_mic(&self) -> bool {
        match &self.mic_muted {
            Some(m) => !m.fetch_xor(true, Ordering::AcqRel),
            None => false,
        }
    }

    pub fn mic_muted(&self) -> bool {
        self.mic_muted.as_ref().is_some_and(|m| m.load(Ordering::Acquire))
    }

    pub fn game(&self) -> Option<GameInfo> {
        self.game.lock().unwrap().clone()
    }

    /// Writes the auto clips that are still waiting for footage after their event.
    pub fn flush_auto_clips(&self) {
        if let Some(tx) = &self.detector {
            let _ = tx.send(DetMsg::Flush);
        }
    }

    /// True once a replayed file has been fed completely.
    pub fn replay_finished(&self) -> bool {
        self.replay_done.load(Ordering::Acquire)
    }

    fn audio_names(&self) -> Vec<String> {
        self.audio_tracks.as_ref().map(|t| t.lock().unwrap().iter().map(|t| t.name.clone()).collect()).unwrap_or_default()
    }

    fn audio_streams(&self) -> Vec<StreamType> {
        self.audio_tracks.as_ref().map(|t| t.lock().unwrap().iter().map(|t| t.stream.clone()).collect()).unwrap_or_default()
    }

    pub fn is_recording(&self) -> bool {
        self.shared.lock().unwrap().recording.is_some()
    }

    fn dir(&self) -> PathBuf {
        game_dir(&self.out_dir, self.game().as_ref().map(|g| g.name.as_str()))
    }

    /// Writes the input track next to `video` for the stretch `from..to` on the capture
    /// clock. Nothing is written when no game was in the foreground during that time.
    fn write_inputs(&self, video: &Path, from: i64, to: i64) {
        let Some(input) = &self.input else { return };
        let clip = input.slice(from, to);
        if clip.events.is_empty() && clip.held.keys.is_empty() && clip.held.buttons.is_empty() {
            return;
        }
        if let Err(e) = clip.write(&InputClip::path_for(video)) {
            eprintln!("klip-recorder: input track not written: {e}");
        }
    }

    pub fn perform(&self, action: &HotkeyAction) -> Res<()> {
        match action {
            HotkeyAction::Clip { seconds } => {
                let started = Instant::now();
                let (packets, audio) = {
                    let mut s = self.shared.lock().unwrap();
                    // In RAM this only copies reference counts, the encoder is not held up.
                    let packets = s.ring.last(*seconds);
                    let from = packets.first().map(|p| p.pts).unwrap_or(0);
                    let audio: Vec<_> = self
                        .audio_streams()
                        .into_iter()
                        .zip(s.audio.iter_mut().map(|ring| ring.since(from)))
                        .collect();
                    (packets, audio)
                };
                let stream = self.enc.stream_type().ok_or("the encoder has not produced a frame yet")?;
                let path = new_path(&self.dir(), "", "mp4")?;
                let ms = mux::write_mp4(&path, &stream, &packets, &audio)?;
                let mut meta = self.meta(ClipKind::Clip, ms, Vec::new());
                if let (Some(first), Some(last)) = (packets.first(), packets.last()) {
                    meta.camera = self.save_camera(&path, first.pts, last.pts + last.duration);
                }
                meta.write_next_to(&path)?;
                if let (Some(first), Some(last)) = (packets.first(), packets.last()) {
                    self.write_inputs(&path, first.pts, last.pts + last.duration);
                }
                *self.last_saved.lock().unwrap() = Some(path.display().to_string());
                println!(
                    "CLIP saved={} duration={:.2}s write={:.0}ms",
                    path.display(),
                    ms as f64 / 1000.0,
                    started.elapsed().as_secs_f64() * 1000.0
                );
            }
            HotkeyAction::RecordToggle => {
                let mut s = self.shared.lock().unwrap();
                if let Some(rec) = s.recording.take() {
                    let cam_rec = s.cam_recording.take();
                    drop(s);
                    let (from, paused) = (rec.base(), rec.was_paused());
                    let (path, ms, markers) = rec.finish()?;
                    let events = markers
                        .into_iter()
                        .map(|at_ms| ClipEvent { at_ms, kind: "marker".into(), label: None })
                        .collect();
                    let mut meta = self.meta(ClipKind::Recording, ms, events);
                    if let Some(cam) = cam_rec {
                        let offset_ms = (cam.base() - from) / 10_000;
                        match cam.finish() {
                            Ok((cam_path, _, _)) => {
                                meta.camera = cam_path
                                    .file_name()
                                    .map(|n| CameraTrack { file: n.to_string_lossy().into_owned(), offset_ms });
                            }
                            Err(e) => eprintln!("klip-recorder: camera file not closed: {e}"),
                        }
                    }
                    meta.write_next_to(&path)?;
                    // A pause shifts the video against the clock the inputs are stamped
                    // with. Rather than write a track that is off, write none.
                    if !paused {
                        self.write_inputs(&path, from, from + ms as i64 * 10_000);
                    }
                    if let Some(input) = &self.input {
                        input.set_retention(Duration::from_secs(self.buffer_seconds as u64 + 5));
                    }
                    *self.last_saved.lock().unwrap() = Some(path.display().to_string());
                    println!("REC stopped={} duration={:.2}s", path.display(), ms as f64 / 1000.0);
                } else {
                    let dir = self.dir();
                    std::fs::create_dir_all(&dir)?;
                    if free_bytes(&dir).is_some_and(|free| free < MIN_FREE_BYTES) {
                        return Err("not enough free disk space to record".into());
                    }
                    let stream = self.enc.stream_type().ok_or("the encoder has not produced a frame yet")?;
                    let head = s.ring.since_last_keyframe();
                    let path = new_path(&dir, "_rec", "mp4")?;
                    let mut rec = Recording::start(path.clone(), &stream, &self.audio_streams(), &head)?;
                    let from = rec.base();
                    for (track, ring) in s.audio.iter_mut().enumerate() {
                        for p in ring.since(from) {
                            rec.push_audio(track, &p)?;
                        }
                    }
                    s.recording = Some(rec);
                    // The camera gets a file of its own, started from its newest keyframe.
                    if let Some(stream) = self.cam_chain.get().and_then(|c| c.enc.stream_type()) {
                        let head = s.cam.as_mut().map(Ring::since_last_keyframe).unwrap_or_default();
                        match Recording::start(ClipMeta::camera_path(&path), &stream, &[], &head) {
                            Ok(cam) => s.cam_recording = Some(cam),
                            Err(e) => eprintln!("klip-recorder: camera not recorded: {e}"),
                        }
                    }
                    // The input track must reach back to the start of the recording.
                    if let Some(input) = &self.input {
                        let longest = if self.max_recording_ms > 0 { self.max_recording_ms / 1000 } else { 24 * 3600 };
                        input.set_retention(Duration::from_secs(longest + 60));
                    }
                    println!("REC started={}", path.display());
                }
            }
            HotkeyAction::RecordPause => {
                let mut s = self.shared.lock().unwrap();
                if let Some(cam) = &mut s.cam_recording {
                    cam.toggle_pause();
                }
                if let Some(rec) = &mut s.recording {
                    let paused = rec.toggle_pause();
                    if !paused {
                        self.enc.force_keyframe();
                        if let Some(c) = self.cam_chain.get() {
                            c.enc.force_keyframe();
                        }
                    }
                    println!("REC {}", if paused { "paused" } else { "resumed" });
                }
            }
            HotkeyAction::Marker => {
                if let Some(rec) = &mut self.shared.lock().unwrap().recording {
                    println!("REC marker at {:.2}s", rec.mark() as f64 / 1000.0);
                }
            }
            // Picked up by the capture thread with the next frame.
            HotkeyAction::Screenshot => self.want_shot.store(true, Ordering::Release),
            // The panel belongs to the overlay, the main loop handles it.
            HotkeyAction::Overlay => {}
            HotkeyAction::InputPause => {
                if let Some(input) = &self.input {
                    let paused = !self.input_paused.fetch_xor(true, Ordering::AcqRel);
                    input.set_paused(paused);
                    println!("input: {}", if paused { "paused" } else { "capturing again" });
                }
            }
        }
        Ok(())
    }

    /// True if a running recording has to end now: disk nearly full or time limit reached.
    pub fn recording_must_stop(&self) -> Option<&'static str> {
        let ms = self.shared.lock().unwrap().recording.as_ref().map(|r| r.length_ms())?;
        if free_bytes(&self.out_dir).is_some_and(|free| free < MIN_FREE_BYTES) {
            return Some("the disk is nearly full");
        }
        (self.max_recording_ms > 0 && ms >= self.max_recording_ms).then_some("time limit reached")
    }

    pub fn status(&self) -> RecorderStatus {
        let s = self.shared.lock().unwrap();
        RecorderStatus {
            version: env!("CARGO_PKG_VERSION").to_string(),
            buffer_seconds: self.buffer_seconds,
            buffered_seconds: s.ring.span_seconds(),
            recording: s.recording.is_some(),
            recording_ms: s.recording.as_ref().map(|r| r.length_ms()).unwrap_or(0),
            encoder: self.enc.name().to_string(),
            width: self.enc_cfg.width,
            height: self.enc_cfg.height,
            fps: self.enc_cfg.fps,
            audio_tracks: self.audio_names(),
            last_saved: self.last_saved.lock().unwrap().clone(),
            game: self.game().map(|g| g.name),
            hotkey_problems: Vec::new(),
            monitors: Vec::new(),
            cameras: Vec::new(),
            camera: None,
            cloud_error: None,
            cloud_uploaded: 0,
        }
    }

    /// One line of measurements since the last call.
    pub fn stat_line(&self, meter: &mut CpuMeter) -> String {
        let cpu = meter.percent();
        let (ws, private) = crate::memory_mb();
        let arrived = self.cap_stats.arrived.load(Ordering::Relaxed);
        let taken = self.cap_stats.taken.load(Ordering::Relaxed);
        let out = self.enc.frames_out();
        let mut c = self.counters.lock().unwrap();
        let secs = c.3.elapsed().as_secs_f64().max(0.001);
        let (bytes, span, rec_ms) = {
            let s = self.shared.lock().unwrap();
            let audio: usize = s.audio.iter().map(|r| r.bytes()).sum();
            (s.ring.bytes() + audio, s.ring.span_seconds(), s.recording.as_ref().map(|r| r.length_ms()))
        };
        let line = format!(
            "STAT cpu={:.2}% ws={:.0}MB private={:.0}MB source_fps={:.1} taken_fps={:.1} encoded_fps={:.1} dropped={} buffer={:.1}MB/{:.1}s{}",
            cpu,
            ws,
            private,
            (arrived - c.0) as f64 / secs,
            (taken - c.1) as f64 / secs,
            (out - c.2) as f64 / secs,
            self.dropped.load(Ordering::Relaxed),
            bytes as f64 / 1_048_576.0,
            span,
            rec_ms.map(|ms| format!(" rec={:.1}s", ms as f64 / 1000.0)).unwrap_or_default(),
        );
        *c = (arrived, taken, out, Instant::now());
        line
    }

    /// Ends everything: closes a running recording properly, stops capture, encoder and
    /// audio, and removes the buffer files.
    pub fn stop(&self) {
        if self.is_recording() {
            if let Err(e) = self.perform(&HotkeyAction::RecordToggle) {
                eprintln!("klip-recorder: recording not closed: {e}");
            }
        }
        self.capture.lock().unwrap().clear();
        self.camera.lock().unwrap().take();
        self.stop.store(true, Ordering::Release);
        self.enc.shutdown();
        if let Some(c) = self.cam_chain.get() {
            c.enc.shutdown();
        }
        if let Some(input) = &self.input {
            input.set_game_active(false);
            input.clear();
        }
        let mut s = self.shared.lock().unwrap();
        s.ring.discard();
        s.audio.iter_mut().for_each(Ring::discard);
        if let Some(cam) = &mut s.cam {
            cam.discard();
        }
    }

    /// Writes the camera for the stretch `from..=to` next to `video`. `None` without a
    /// camera or when it has nothing for that stretch.
    fn save_camera(&self, video: &Path, from: i64, to: i64) -> Option<CameraTrack> {
        let stream = self.cam_chain.get()?.enc.stream_type()?;
        let packets = self.shared.lock().unwrap().cam.as_mut()?.range(from, to);
        let first = packets.first()?.pts;
        let path = ClipMeta::camera_path(video);
        match mux::write_mp4(&path, &stream, &packets, &[]) {
            Ok(_) => Some(CameraTrack {
                file: path.file_name()?.to_string_lossy().into_owned(),
                offset_ms: (first - from) / 10_000,
            }),
            Err(e) => {
                eprintln!("klip-recorder: camera file not written: {e}");
                None
            }
        }
    }

    /// Name of the camera that is running.
    pub fn camera_name(&self) -> Option<String> {
        self.camera.lock().unwrap().as_ref().map(|f| f.name.clone())
    }

    fn meta(&self, kind: ClipKind, duration_ms: u64, events: Vec<ClipEvent>) -> ClipMeta {
        ClipMeta {
            version: SIDECAR_VERSION,
            kind,
            created: stamp('T', ':'),
            game: self.game().map(|g| g.name),
            reason: None,
            duration_ms,
            width: self.enc_cfg.width,
            height: self.enc_cfg.height,
            fps: self.enc_cfg.fps,
            codec: self.enc_cfg.codec,
            encoder: self.enc.name().to_string(),
            audio_tracks: self.audio_names(),
            events,
            favorite: false,
            tags: Vec::new(),
            camera: None,
            cloud: None,
        }
    }
}

/// The layout named in the settings from `%APPDATA%\Klip\layouts.json`, or the built in
/// left hand layout. Only the keys of this layout are ever stored.
fn load_layout(name: Option<&str>) -> OverlayLayout {
    let fallback = || OverlayLayout::preset(LayoutKind::IsoQwertz, Preset::LeftHandShooter);
    let Some(name) = name else { return fallback() };
    let path = klip_core::data_dir().join("layouts.json");
    match klip_input::LayoutLibrary::load(&path) {
        Ok(library) => library.get(name).cloned().unwrap_or_else(fallback),
        Err(_) => fallback(),
    }
}

/// Clips are sorted into one folder per game. Characters Windows forbids in names are dropped.
fn game_dir(base: &Path, game: Option<&str>) -> PathBuf {
    match game {
        Some(name) => {
            let clean: String = name.chars().filter(|c| !r#"<>:"/\|?*"#.contains(*c) && !c.is_control()).collect();
            let clean = clean.trim().trim_end_matches('.');
            if clean.is_empty() { base.to_path_buf() } else { base.join(clean) }
        }
        None => base.to_path_buf(),
    }
}

/// Reads the frame back on the capture thread and leaves encoding to a worker.
fn take_screenshot(gpu: &SendGpu, texture: &ID3D11Texture2D, out_dir: &Path, game: Option<String>) {
    let pixels = match shot::read_back(&gpu.0, &gpu.1, texture) {
        Ok(p) => p,
        Err(e) => return eprintln!("klip-recorder: screenshot failed: {e}"),
    };
    let out_dir = out_dir.to_path_buf();
    std::thread::spawn(move || {
        let result = new_path(&out_dir, "", "png").and_then(|path| {
            shot::write_png(&path, &pixels)?;
            ClipMeta {
                version: SIDECAR_VERSION,
                kind: ClipKind::Screenshot,
                created: stamp('T', ':'),
                game,
                reason: None,
                duration_ms: 0,
                width: pixels.width,
                height: pixels.height,
                fps: 0,
                codec: VideoCodec::H264,
                encoder: String::new(),
                audio_tracks: Vec::new(),
                events: Vec::new(),
                favorite: false,
                tags: Vec::new(),
                camera: None,
                cloud: None,
            }
            .write_next_to(&path)?;
            Ok(path)
        });
        match result {
            Ok(path) => println!("SHOT saved={}", path.display()),
            Err(e) => eprintln!("klip-recorder: screenshot failed: {e}"),
        }
    });
}

struct SendTexture(ID3D11Texture2D);
// SAFETY: the device is multithread protected and the texture is only handed to the encoder.
unsafe impl Send for SendTexture {}

struct SendGpu(ID3D11Device, ID3D11DeviceContext);
// SAFETY: the device is free threaded and the context is multithread protected.
unsafe impl Send for SendGpu {}
