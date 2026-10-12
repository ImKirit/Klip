//! Audio: every source becomes its own track. Sources are read through WASAPI, per
//! program with process loopback. That API is served by the Windows audio engine, no
//! handle to the other process is opened and nothing is loaded into it.
//!
//! One thread reads all sources and puts them on a common sample grid that is anchored to
//! the clock of the video timestamps. Because every track advances by exactly the same
//! number of samples per step, the tracks can never drift apart, and silence (for which
//! process loopback delivers nothing at all) is simply filled with zeros.
//!
//! By default every program that plays sound gets a track by itself: the thread watches
//! the audio sessions of the output device and opens a new track for each new program.

use crate::encoder::StreamType;
use crate::ring::Packet;
use crate::Res;
use klip_core::{AudioConfig, AudioMode};
use std::collections::{HashMap, VecDeque};
use std::mem::ManuallyDrop;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use wasapi::{
    AudioCaptureClient, AudioClient, DeviceEnumerator, Direction, SampleType, SessionState, StreamMode, WaveFormat,
};
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};

const RATE: i64 = 48_000;
const CHANNELS: usize = 2;
/// Bytes per sample frame, 16 bit stereo.
const FRAME: usize = 2 * CHANNELS;
/// Samples per AAC frame.
const BLOCK: usize = 1024;
/// The grid runs this far behind real time, which gives every source time to deliver.
const LATENCY_FRAMES: i64 = RATE * 60 / 1000;
/// A source that has piled up more than this is ahead of the clock and gets trimmed.
const MAX_BACKLOG: usize = (RATE as usize * 250 / 1000) * FRAME;
const AAC_BYTES_PER_SECOND: u32 = 20_000;
/// Upper limit of tracks in one file. Players and editors get unwieldy beyond this, and
/// every track costs an encoder.
const MAX_TRACKS: usize = 16;
/// Programs that play sound but are never worth a track of their own.
const IGNORED: &[&str] = &[
    "klip-recorder.exe",
    "klip.exe",
    "audiodg.exe",
    "shellexperiencehost.exe",
    // Recorders of the graphics driver hold a session that only mirrors the system sound.
    "nvcontainer.exe",
    "nvidia share.exe",
    "nvidia overlay.exe",
    "nvidia broadcast.exe",
    "amdrsserv.exe",
];

/// The exe that feeds the track "Game". Can change while the recorder runs.
pub type GameExe = Arc<Mutex<Option<String>>>;

#[derive(Clone)]
pub struct TrackInfo {
    pub name: String,
    pub stream: StreamType,
}

pub struct Audio {
    pub game: GameExe,
    /// While set, the microphone track and its share of the mix are silent.
    pub mic_muted: Arc<AtomicBool>,
    /// All tracks in file order. Track 0 is the finished mix, which is what players that
    /// only know one audio track will play. Grows while programs start to play sound.
    pub tracks: Arc<Mutex<Vec<TrackInfo>>>,
}

/// One capture stream and the samples it has delivered but the grid has not consumed yet.
struct Source {
    _client: AudioClient,
    capture: AudioCaptureClient,
    fifo: VecDeque<u8>,
}

impl Source {
    fn open(client: Res<AudioClient>) -> Res<Self> {
        let mut client = client?;
        let format = WaveFormat::new(16, 16, &SampleType::Int, RATE as usize, CHANNELS, None);
        let mode = StreamMode::PollingShared { autoconvert: true, buffer_duration_hns: 2_000_000 };
        client.initialize_client(&format, &Direction::Capture, &mode).map_err(|e| e.to_string())?;
        let capture = client.get_audiocaptureclient().map_err(|e| e.to_string())?;
        client.start_stream().map_err(|e| e.to_string())?;
        Ok(Self { _client: client, capture, fifo: VecDeque::new() })
    }

    fn system() -> Res<Self> {
        Self::open(default_device(&Direction::Render))
    }

    fn microphone() -> Res<Self> {
        Self::open(default_device(&Direction::Capture))
    }

    fn process(pid: u32) -> Res<Self> {
        Self::open(AudioClient::new_application_loopback_client(pid, true).map_err(|e| e.to_string().into()))
    }

    /// Moves everything the device has ready into the fifo. An error means the device or
    /// the process is gone.
    fn drain(&mut self) -> Res<()> {
        loop {
            match self.capture.get_next_packet_size().map_err(|e| e.to_string())? {
                Some(n) if n > 0 => {
                    self.capture.read_from_device_to_deque(&mut self.fifo).map_err(|e| e.to_string())?;
                }
                _ => break,
            }
        }
        if self.fifo.len() > MAX_BACKLOG {
            let excess = (self.fifo.len() - MAX_BACKLOG / 2) / FRAME * FRAME;
            self.fifo.drain(..excess);
        }
        Ok(())
    }

    /// Adds the next block of this source onto `out`. Missing samples count as silence.
    fn mix_into(&mut self, out: &mut [i16]) {
        for sample in out.iter_mut() {
            let (Some(lo), Some(hi)) = (self.fifo.pop_front(), self.fifo.pop_front()) else { break };
            *sample = sample.saturating_add(i16::from_le_bytes([lo, hi]));
        }
    }
}

fn default_device(direction: &Direction) -> Res<AudioClient> {
    let device = DeviceEnumerator::new()
        .and_then(|e| e.get_default_device(direction))
        .map_err(|e| e.to_string())?;
    Ok(device.get_iaudioclient().map_err(|e| e.to_string())?)
}

enum Kind {
    /// Sum of the system track and the microphone.
    Mix,
    System(Option<Source>),
    Microphone(Option<Source>),
    /// All running programs with one of these exe names, one source per process tree.
    Apps { exes: Vec<String>, sources: HashMap<u32, Source> },
    /// The game in the foreground. The track always exists so that every file has the
    /// same layout, it is silent while no game is known.
    Game { exe: GameExe, current: Option<String>, sources: HashMap<u32, Source> },
}

struct Track {
    kind: Kind,
    encoder: Aac,
}
// SAFETY: Media Foundation objects are free threaded, and the WASAPI clients are created
// and used on the audio thread only (the track list moves there before any is opened).
unsafe impl Send for Track {}

/// Starts the audio thread. `on_packet` receives the track index and one AAC frame,
/// timestamps are on the same clock as the video. `keep_seconds` is the length of the
/// replay buffer: a track that appears later is filled with silence that far back, so
/// all tracks of a clip have the same length.
pub fn start<F>(cfg: &AudioConfig, keep_seconds: u32, stop: Arc<AtomicBool>, mut on_packet: F) -> Res<Audio>
where
    F: FnMut(usize, Packet) + Send + 'static,
{
    let game: GameExe = Arc::new(Mutex::new(None));
    let mut plan: Vec<(String, Kind)> = vec![("Mix".into(), Kind::Mix), ("System".into(), Kind::System(None))];
    if cfg.microphone {
        plan.push(("Microphone".into(), Kind::Microphone(None)));
    }
    plan.push(("Game".into(), Kind::Game { exe: game.clone(), current: None, sources: HashMap::new() }));
    for app in &cfg.apps {
        let exes = app.exe.iter().map(|e| e.to_lowercase()).collect();
        plan.push((app.name.clone(), Kind::Apps { exes, sources: HashMap::new() }));
    }

    let mic_muted = Arc::new(AtomicBool::new(false));
    let muted = mic_muted.clone();
    let mut tracks = Vec::new();
    let mut infos = Vec::new();
    for (name, kind) in plan {
        let (encoder, stream) = Aac::new()?;
        infos.push(TrackInfo { name, stream });
        tracks.push(Track { kind, encoder });
    }
    let shared = Arc::new(Mutex::new(infos));
    let list = shared.clone();
    let discover = cfg.mode == AudioMode::All;
    let keep_frames = keep_seconds as i64 * RATE;

    std::thread::Builder::new().name("klip-audio".into()).spawn(move || {
        let _ = wasapi::initialize_mta();
        let anchor = crate::qpc_100ns();
        let mut emitted: i64 = 0;
        let mut last_scan = Instant::now() - Duration::from_secs(60);
        let mut blocks: Vec<[i16; BLOCK * CHANNELS]> = vec![[0; BLOCK * CHANNELS]; tracks.len()];
        let mut out = Vec::new();
        let block_100ns = BLOCK as i64 * 10_000_000 / RATE;
        while !stop.load(Ordering::Acquire) {
            // One AAC frame lasts 21 ms, waking up more often than that only costs CPU.
            std::thread::sleep(Duration::from_millis(20));

            // The game changed: its old sources belong to another program now.
            for t in tracks.iter_mut() {
                if let Kind::Game { exe, current, sources } = &mut t.kind {
                    let wanted = exe.lock().unwrap().as_ref().map(|e| e.to_lowercase());
                    if wanted != *current {
                        *current = wanted;
                        sources.clear();
                        last_scan = Instant::now() - Duration::from_secs(60);
                    }
                }
            }

            // Devices and programs come and go. Reopen what is missing every few seconds.
            if last_scan.elapsed() >= Duration::from_secs(3) {
                last_scan = Instant::now();
                let processes = process_list();

                // A program that started to play sound and has no track yet gets one.
                if discover && tracks.len() < MAX_TRACKS {
                    for exe in playing_programs(&processes) {
                        let known = tracks.iter().any(|t| match &t.kind {
                            Kind::Apps { exes, .. } => exes.contains(&exe),
                            Kind::Game { current, .. } => current.as_deref() == Some(exe.as_str()),
                            _ => false,
                        });
                        if known || tracks.len() >= MAX_TRACKS {
                            continue;
                        }
                        let Ok((encoder, stream)) = Aac::new() else { continue };
                        let index = tracks.len();
                        let name = track_name(&exe);
                        println!("audio: new track '{name}' for {exe}");
                        // Silence back to the start of the buffer, so this track is as long
                        // as the others in every clip cut from now on.
                        if let Some(frame) = encoder.settled_silence() {
                            let first = (emitted - keep_frames).max(0) / BLOCK as i64 * BLOCK as i64;
                            let mut at = first;
                            while at < emitted {
                                let pts = anchor + at * 10_000_000 / RATE;
                                on_packet(index, Packet { data: frame.clone(), pts, duration: block_100ns, key: true });
                                at += BLOCK as i64;
                            }
                        }
                        list.lock().unwrap().push(TrackInfo { name, stream });
                        tracks.push(Track { kind: Kind::Apps { exes: vec![exe], sources: HashMap::new() }, encoder });
                        blocks.push([0; BLOCK * CHANNELS]);
                    }
                }

                for t in tracks.iter_mut() {
                    match &mut t.kind {
                        Kind::System(s @ None) => *s = report("system", Source::system()),
                        Kind::Microphone(s @ None) => *s = report("microphone", Source::microphone()),
                        Kind::Game { current: None, .. } => {}
                        Kind::Game { current: Some(exe), sources, .. } => {
                            open_missing(&processes, std::slice::from_ref(exe), sources)
                        }
                        Kind::Apps { exes, sources } => open_missing(&processes, exes, sources),
                        _ => {}
                    }
                }
            }

            for t in tracks.iter_mut() {
                match &mut t.kind {
                    Kind::System(s) | Kind::Microphone(s) => {
                        if s.as_mut().is_some_and(|src| src.drain().is_err()) {
                            *s = None;
                        }
                    }
                    Kind::Apps { sources, .. } | Kind::Game { sources, .. } => {
                        sources.retain(|_, src| src.drain().is_ok())
                    }
                    Kind::Mix => {}
                }
            }

            let target = (crate::qpc_100ns() - anchor) * RATE / 10_000_000 - LATENCY_FRAMES;
            while target - emitted >= BLOCK as i64 {
                for (t, block) in tracks.iter_mut().zip(blocks.iter_mut()) {
                    block.fill(0);
                    match &mut t.kind {
                        Kind::Microphone(Some(s)) if muted.load(Ordering::Relaxed) => s.fifo.clear(),
                        Kind::System(Some(s)) | Kind::Microphone(Some(s)) => s.mix_into(block),
                        Kind::Apps { sources, .. } | Kind::Game { sources, .. } => {
                            sources.values_mut().for_each(|s| s.mix_into(block))
                        }
                        _ => {}
                    }
                }
                // The mix is everything the user hears plus the microphone. The system track
                // already contains every program, so program tracks are not added again.
                let mut mix = [0i16; BLOCK * CHANNELS];
                for (t, block) in tracks.iter().zip(blocks.iter()) {
                    if matches!(t.kind, Kind::System(_) | Kind::Microphone(_)) {
                        for (m, s) in mix.iter_mut().zip(block.iter()) {
                            *m = m.saturating_add(*s);
                        }
                    }
                }
                blocks[0] = mix;

                let pts = anchor + emitted * 10_000_000 / RATE;
                for (i, (t, block)) in tracks.iter().zip(blocks.iter()).enumerate() {
                    out.clear();
                    if let Err(e) = t.encoder.encode(block, pts, block_100ns, &mut out) {
                        eprintln!("klip-recorder: audio track {i} lost a block: {e}");
                    }
                    for p in out.drain(..) {
                        on_packet(i, p);
                    }
                }
                emitted += BLOCK as i64;
            }
        }
    })?;

    Ok(Audio { game, mic_muted, tracks: shared })
}

/// Opens a source for every matching process tree that has none yet.
fn open_missing(processes: &[(u32, u32, String)], exes: &[String], sources: &mut HashMap<u32, Source>) {
    for pid in root_processes(processes, exes) {
        if !sources.contains_key(&pid) {
            if let Ok(source) = Source::process(pid) {
                sources.insert(pid, source);
            }
        }
    }
}

/// Exe names of the programs that have an active audio session on the output device.
/// This asks the Windows audio engine which programs play sound, nothing else.
fn playing_programs(processes: &[(u32, u32, String)]) -> Vec<String> {
    let mut found = Vec::new();
    let Ok(device) = DeviceEnumerator::new().and_then(|e| e.get_default_device(&Direction::Render)) else {
        return found;
    };
    let Ok(sessions) = device.get_iaudiosessionmanager().and_then(|m| m.get_audiosessionenumerator()) else {
        return found;
    };
    for i in 0..sessions.get_count().unwrap_or(0) {
        let Ok(session) = sessions.get_session(i) else { continue };
        if !matches!(session.get_state(), Ok(SessionState::Active)) {
            continue;
        }
        // Windows calls a session active as soon as a program holds a stream open, even
        // if it never plays anything. Only a program that is audible right now gets a
        // track, otherwise every background service would end up in every clip.
        let audible = session.get_audiometerinformation().and_then(|m| m.get_peak_value()).is_ok_and(|peak| peak > 0.00002);
        if !audible {
            continue;
        }
        let Ok(pid) = session.get_process_id() else { continue };
        let Some((_, _, exe)) = processes.iter().find(|(p, _, _)| *p == pid && pid != 0) else { continue };
        if !IGNORED.contains(&exe.as_str()) && !found.contains(exe) {
            found.push(exe.clone());
        }
    }
    found
}

/// "spotify.exe" becomes "Spotify".
fn track_name(exe: &str) -> String {
    let stem = exe.strip_suffix(".exe").unwrap_or(exe);
    let mut chars = stem.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => exe.to_string(),
    }
}

/// Says once per attempt whether a device could be opened. Failures are normal (no
/// microphone plugged in) and are retried on the next scan.
fn report(what: &str, source: Res<Source>) -> Option<Source> {
    match source {
        Ok(s) => {
            println!("audio: {what} opened");
            Some(s)
        }
        Err(e) => {
            eprintln!("klip-recorder: audio {what} not available: {e}");
            None
        }
    }
}

/// The AAC encoder that ships with Windows. Runs on the CPU, one frame costs microseconds.
struct Aac {
    mft: IMFTransform,
    out_size: u32,
    /// Silent blocks fed to the encoder in a row.
    silent_run: std::cell::Cell<u32>,
    /// What the encoder puts out for silence once it has only seen silence for a while.
    silence: std::cell::RefCell<Option<Arc<[u8]>>>,
}

/// After this many silent blocks the encoder holds no trace of earlier sound.
const SILENT_SETTLE: u32 = 4;

impl Aac {
    fn new() -> Res<(Self, StreamType)> {
        unsafe {
            let input = MFT_REGISTER_TYPE_INFO { guidMajorType: MFMediaType_Audio, guidSubtype: MFAudioFormat_PCM };
            let output = MFT_REGISTER_TYPE_INFO { guidMajorType: MFMediaType_Audio, guidSubtype: MFAudioFormat_AAC };
            let mut list: *mut Option<IMFActivate> = std::ptr::null_mut();
            let mut count = 0u32;
            MFTEnumEx(
                MFT_CATEGORY_AUDIO_ENCODER,
                MFT_ENUM_FLAG_SYNCMFT | MFT_ENUM_FLAG_SORTANDFILTER,
                Some(&input),
                Some(&output),
                &mut list,
                &mut count,
            )?;
            let mut mft: Option<IMFTransform> = None;
            for i in 0..count as usize {
                if let Some(activate) = (*list.add(i)).take() {
                    if mft.is_none() {
                        mft = activate.ActivateObject().ok();
                    }
                }
            }
            CoTaskMemFree(Some(list as *const _));
            let mft = mft.ok_or("no AAC encoder found")?;

            let pcm = MFCreateMediaType()?;
            pcm.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
            pcm.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM)?;
            pcm.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
            pcm.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, RATE as u32)?;
            pcm.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, CHANNELS as u32)?;
            pcm.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, FRAME as u32)?;
            pcm.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, RATE as u32 * FRAME as u32)?;
            mft.SetInputType(0, &pcm, 0)?;

            let aac = MFCreateMediaType()?;
            aac.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
            aac.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_AAC)?;
            aac.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
            aac.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, RATE as u32)?;
            aac.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, CHANNELS as u32)?;
            aac.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, AAC_BYTES_PER_SECOND)?;
            // Raw AAC frames without ADTS headers, which is what MP4 stores.
            aac.SetUINT32(&MF_MT_AAC_PAYLOAD_TYPE, 0)?;
            mft.SetOutputType(0, &aac, 0)?;

            // The type the encoder reports back carries the decoder configuration the muxer needs.
            let stream = StreamType(mft.GetOutputCurrentType(0)?);
            let out_size = mft.GetOutputStreamInfo(0)?.cbSize.max(8192);
            mft.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)?;
            mft.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)?;
            Ok((Self { mft, out_size, silent_run: Default::default(), silence: Default::default() }, stream))
        }
    }

    /// Runs the fresh encoder on silence until it has settled and returns the frame it
    /// produces for silence. Used to fill a new track backwards.
    fn settled_silence(&self) -> Option<Arc<[u8]>> {
        let zeros = [0i16; BLOCK * CHANNELS];
        let mut scratch = Vec::new();
        for _ in 0..SILENT_SETTLE + 2 {
            scratch.clear();
            self.encode(&zeros, 0, 0, &mut scratch).ok()?;
        }
        self.silence.borrow().clone()
    }

    /// Most tracks are silent most of the time. A settled silent track reuses the frame
    /// the encoder produced for silence. The encoder itself is left in its silent state,
    /// which is exactly where it would be had it encoded every block.
    fn encode(&self, pcm: &[i16], pts: i64, duration: i64, out: &mut Vec<Packet>) -> Res<()> {
        let silent = pcm.iter().all(|s| *s == 0);
        if !silent {
            self.silent_run.set(0);
        } else if self.silent_run.get() >= SILENT_SETTLE {
            if let Some(frame) = self.silence.borrow().as_ref() {
                out.push(Packet { data: frame.clone(), pts, duration, key: true });
                return Ok(());
            }
        }
        let before = out.len();
        let result = self.encode_block(pcm, pts, duration, out);
        if silent {
            self.silent_run.set(self.silent_run.get() + 1);
            if self.silent_run.get() >= SILENT_SETTLE && out.len() == before + 1 {
                *self.silence.borrow_mut() = Some(out[before].data.clone());
            }
        }
        result
    }

    fn encode_block(&self, pcm: &[i16], pts: i64, duration: i64, out: &mut Vec<Packet>) -> Res<()> {
        unsafe {
            let bytes = std::mem::size_of_val(pcm);
            let buffer = MFCreateMemoryBuffer(bytes as u32)?;
            let mut ptr = std::ptr::null_mut();
            buffer.Lock(&mut ptr, None, None)?;
            std::ptr::copy_nonoverlapping(pcm.as_ptr() as *const u8, ptr, bytes);
            buffer.Unlock()?;
            buffer.SetCurrentLength(bytes as u32)?;
            let sample = MFCreateSample()?;
            sample.AddBuffer(&buffer)?;
            sample.SetSampleTime(pts)?;
            sample.SetSampleDuration(duration)?;
            self.mft.ProcessInput(0, &sample, 0)?;

            loop {
                let holder = MFCreateSample()?;
                holder.AddBuffer(&MFCreateMemoryBuffer(self.out_size)?)?;
                let mut data = [MFT_OUTPUT_DATA_BUFFER {
                    dwStreamID: 0,
                    pSample: ManuallyDrop::new(Some(holder)),
                    dwStatus: 0,
                    pEvents: ManuallyDrop::new(None),
                }];
                let mut status = 0u32;
                let result = self.mft.ProcessOutput(0, &mut data, &mut status);
                let produced = ManuallyDrop::take(&mut data[0].pSample);
                ManuallyDrop::drop(&mut data[0].pEvents);
                match result {
                    Err(e) if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => return Ok(()),
                    Err(e) => return Err(e.into()),
                    Ok(()) => {}
                }
                let Some(produced) = produced else { return Ok(()) };
                let buf = produced.ConvertToContiguousBuffer()?;
                let mut p = std::ptr::null_mut();
                let mut len = 0u32;
                buf.Lock(&mut p, None, Some(&mut len))?;
                let bytes: Arc<[u8]> = Arc::from(std::slice::from_raw_parts(p, len as usize));
                buf.Unlock()?;
                // The time of the input block is used on purpose: the encoder numbers its
                // output by what it was fed, which goes wrong once silent blocks are skipped.
                out.push(Packet { data: bytes, pts, duration, key: true });
            }
        }
    }
}

/// Process id, parent id and exe name in lower case of every running process. Reads the
/// process table only, no process is opened.
fn process_list() -> Vec<(u32, u32, String)> {
    let mut all = Vec::new();
    unsafe {
        let Ok(snapshot) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return all };
        let mut entry = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        if Process32FirstW(snapshot, &mut entry).is_ok() {
            loop {
                let len = entry.szExeFile.iter().position(|c| *c == 0).unwrap_or(entry.szExeFile.len());
                let name = String::from_utf16_lossy(&entry.szExeFile[..len]).to_lowercase();
                all.push((entry.th32ProcessID, entry.th32ParentProcessID, name));
                if Process32NextW(snapshot, &mut entry).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snapshot);
    }
    all
}

/// Programs like Discord and browsers consist of many processes. The root of each tree is
/// enough, because process loopback includes the children.
fn root_processes(all: &[(u32, u32, String)], exes: &[String]) -> Vec<u32> {
    all.iter()
        .filter(|(_, _, name)| exes.contains(name))
        .filter(|(_, parent, name)| !all.iter().any(|(pid, _, pname)| pid == parent && pname == name))
        .map(|(pid, _, _)| *pid)
        .collect()
}
