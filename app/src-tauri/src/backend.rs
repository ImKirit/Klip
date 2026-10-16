//! Everything the UI asks of the machine: reading the clip folder, writing sidecars,
//! running ffmpeg, the recycle bin. No Tauri types in here, so the same code serves the
//! real app and the test backend of the browser preview.
//!
//! One entry point, `Backend::dispatch`, takes a command name and JSON arguments.

use klip_core::{ClipMeta, Config};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read};
use std::os::windows::process::CommandExt;
use std::path::{Component, Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, UNIX_EPOCH};

/// Keeps child processes from flashing a console window.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const DEFAULT_FFMPEG: &str = r"C:\ffmpeg\bin\ffmpeg.exe";
const VIDEO_EXT: [&str; 4] = ["mp4", "mkv", "webm", "mov"];
const IMAGE_EXT: [&str; 3] = ["png", "jpg", "jpeg"];
/// Files that travel with a clip: metadata, cuts, input track, webcam.
const SIDECARS: [&str; 4] = ["klip.json", "edit.json", "inputs.json", "cam.mp4"];
/// Exports land in this subfolder by default, the library does not list it.
const EXPORT_FOLDER: &str = "Exporte";

type R = Result<Value, String>;

fn err<E: std::fmt::Display>(what: &str) -> impl Fn(E) -> String + '_ {
    move |e| format!("{what}: {e}")
}

fn env_dir(name: &str) -> Option<PathBuf> {
    std::env::var_os(name).filter(|v| !v.is_empty()).map(PathBuf::from)
}

/// `%APPDATA%\Klip`, or `KLIP_DATA_DIR` so that tests never touch the real settings.
pub fn data_dir() -> PathBuf {
    env_dir("KLIP_DATA_DIR").unwrap_or_else(klip_core::data_dir)
}

/// Where thumbnails, audio tracks and waveforms go: `Config::cache_dir()` of klip-core,
/// so the app and the recorder agree on the place the user chose. `KLIP_CACHE_DIR` wins
/// over the settings, so a test can never write outside its own folder.
pub fn cache_dir() -> PathBuf {
    env_dir("KLIP_CACHE_DIR").unwrap_or_else(|| Config::load().unwrap_or_default().cache_dir())
}

/// The app keeps one folder per clip in the cache, named by a 16 digit hex key. Other
/// folders in there belong to the recorder (its replay buffer on disk) and are left alone.
fn app_cache_entries() -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(cache_dir()) else { return Vec::new() };
    entries
        .flatten()
        .filter(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            name.len() == 16 && name.chars().all(|c| c.is_ascii_hexdigit()) && e.path().is_dir()
        })
        .map(|e| e.path())
        .collect()
}

fn config_path() -> PathBuf {
    data_dir().join("config.json")
}

fn settings_path() -> PathBuf {
    data_dir().join("app.json")
}

fn read_json(path: &Path) -> Result<Option<Value>, String> {
    match std::fs::read(path) {
        Ok(bytes) => {
            // Tolerate a UTF-8 byte order mark, editors on Windows like to add one.
            let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&bytes);
            serde_json::from_slice(bytes).map(Some).map_err(|e| format!("{}: {e}", path.display()))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

/// Writes through a temporary file and a rename, a crash never leaves half a file.
fn write_json(path: &Path, value: &Value) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(err("create folder"))?;
    }
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".tmp");
    let tmp = path.with_file_name(name);
    std::fs::write(&tmp, serde_json::to_vec_pretty(value).map_err(err("encode"))?).map_err(err("write"))?;
    std::fs::rename(&tmp, path).map_err(err("rename"))
}

/// The config as the recorder sees it: defaults filled in, unknown fields kept as they
/// are so that a newer recorder does not lose settings when the app saves.
fn load_config() -> (Value, bool, Option<String>) {
    let defaults = serde_json::to_value(Config::default()).unwrap_or(Value::Null);
    match read_json(&config_path()) {
        Ok(None) => (defaults, false, None),
        Err(e) => (defaults, true, Some(e)),
        // `Config::load` also brings a file of an older version up to date, exactly as the
        // recorder does (second clip key, accent). The plain parse is the fallback for a
        // file it cannot read but `read_json` can, one with a byte order mark.
        Ok(Some(raw)) => match Config::load().or_else(|_| serde_json::from_value::<Config>(raw.clone()).map_err(|e| e.to_string())) {
            Err(e) => (defaults, true, Some(format!("{}: {e}", config_path().display()))),
            Ok(typed) => {
                let mut merged = serde_json::to_value(typed).unwrap_or(Value::Null);
                if let (Some(m), Some(r)) = (merged.as_object_mut(), raw.as_object()) {
                    for (k, v) in r {
                        m.entry(k.clone()).or_insert_with(|| v.clone());
                    }
                }
                (merged, true, None)
            }
        },
    }
}

pub fn clip_dir() -> PathBuf {
    if let Some(dir) = env_dir("KLIP_CLIP_DIR") {
        return dir;
    }
    let (config, _, _) = load_config();
    match config.get("clip_dir").and_then(Value::as_str) {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => klip_core::default_clip_dir(),
    }
}

fn ffmpeg_path() -> PathBuf {
    let from_settings = read_json(&settings_path())
        .ok()
        .flatten()
        .and_then(|s| s.get("ffmpeg_path").and_then(Value::as_str).map(str::to_string))
        .filter(|s| !s.trim().is_empty());
    match from_settings {
        Some(p) => PathBuf::from(p),
        None if Path::new(DEFAULT_FFMPEG).exists() => PathBuf::from(DEFAULT_FFMPEG),
        None => PathBuf::from("ffmpeg"),
    }
}

fn ffprobe_path() -> PathBuf {
    let ffmpeg = ffmpeg_path();
    match ffmpeg.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir.join("ffprobe.exe"),
        _ => PathBuf::from("ffprobe"),
    }
}

fn tool(path: &Path) -> Command {
    let mut cmd = Command::new(path);
    cmd.creation_flags(CREATE_NO_WINDOW).stdin(Stdio::null());
    cmd
}

fn str_arg<'a>(a: &'a Value, key: &str) -> Result<&'a str, String> {
    a.get(key).and_then(Value::as_str).ok_or(format!("argument '{key}' is missing"))
}

fn list_arg(a: &Value, key: &str) -> Result<Vec<String>, String> {
    let list = a.get(key).and_then(Value::as_array).ok_or(format!("argument '{key}' is missing"))?;
    Ok(list.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
}

fn lower(path: &Path) -> String {
    path.to_string_lossy().replace('/', "\\").to_lowercase()
}

/// True if `path` is absolute, has no `..` and lies inside `root`.
pub fn is_inside(path: &Path, root: &Path) -> bool {
    let clean = path.is_absolute() && !path.components().any(|c| matches!(c, Component::ParentDir));
    let root = lower(root);
    let root = root.trim_end_matches('\\');
    let path = lower(path);
    clean && path.starts_with(root) && path[root.len()..].starts_with('\\')
}

/// Guards every command that changes or removes files: only inside the clip folder.
fn in_clip_dir(path: &str) -> Result<PathBuf, String> {
    let p = PathBuf::from(path);
    if is_inside(&p, &clip_dir()) {
        Ok(p)
    } else {
        Err(format!("{path} is outside the clip folder"))
    }
}

fn sidecar(video: &Path, ext: &str) -> PathBuf {
    video.with_extension(ext)
}

fn fnv(text: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

fn modified_ms(meta: &std::fs::Metadata) -> u64 {
    meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// Cache folder of one clip. The key changes when the file changes, old entries are
/// simply never read again.
fn cache_key(path: &Path, meta: &std::fs::Metadata) -> String {
    fnv(&format!("{}|{}|{}", lower(path), meta.len(), modified_ms(meta)))
}

fn dir_size(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else { return 0 };
    entries
        .flatten()
        .map(|e| match e.metadata() {
            Ok(m) if m.is_dir() => dir_size(&e.path()),
            Ok(m) => m.len(),
            Err(_) => 0,
        })
        .sum()
}

#[derive(Default)]
struct JobState {
    state: &'static str,
    out_ms: u64,
    speed: String,
    log: String,
    code: Option<i32>,
}

struct Job {
    child: Mutex<Child>,
    state: Mutex<JobState>,
    total_ms: u64,
    outputs: Vec<PathBuf>,
    /// Applied once ffmpeg has succeeded, so a half written file never has its final name.
    renames: Vec<(PathBuf, PathBuf)>,
}

impl Job {
    fn remove_outputs(&self) {
        for path in &self.outputs {
            let _ = std::fs::remove_file(path);
        }
    }
}

pub struct Backend {
    jobs: Mutex<HashMap<u64, Arc<Job>>>,
    next_job: AtomicU64,
}

impl Default for Backend {
    fn default() -> Self {
        Self::new()
    }
}

impl Backend {
    pub fn new() -> Self {
        Self { jobs: Mutex::new(HashMap::new()), next_job: AtomicU64::new(1) }
    }

    /// Stops every running ffmpeg. Called when the app exits, nothing may stay behind.
    pub fn kill_all(&self) {
        for job in self.jobs.lock().unwrap().values() {
            let _ = job.child.lock().unwrap().kill();
            if job.state.lock().unwrap().state == "running" {
                job.remove_outputs();
            }
        }
    }

    pub fn dispatch(&self, cmd: &str, a: &Value) -> R {
        match cmd {
            "env" => env_info(),
            "config_load" => {
                let (config, exists, error) = load_config();
                Ok(json!({ "config": config, "exists": exists, "error": error }))
            }
            "config_save" => config_save(a),
            "settings_load" => Ok(read_json(&settings_path())?.unwrap_or_else(|| json!({}))),
            "settings_save" => {
                let settings = a.get("settings").filter(|s| s.is_object()).ok_or("settings must be an object")?;
                write_json(&settings_path(), settings)?;
                Ok(Value::Null)
            }
            "autoclip_load" => autoclip_load(),
            "autoclip_save" => autoclip_save(a),
            "settings_export" => settings_export(a),
            "settings_import" => {
                read_json(Path::new(str_arg(a, "path")?))?.ok_or_else(|| "file not found".to_string())
            }
            "library_scan" => library_scan(),
            "thumb" => thumb(a),
            "meta_write" => meta_write(a),
            "rename" => rename(a),
            "trash" => trash_clips(a),
            "reveal" => reveal(a),
            "clipboard_files" => clipboard_files(a),
            "disk" => disk(a),
            "edit_read" => Ok(read_json(&sidecar(Path::new(str_arg(a, "path")?), "edit.json"))?.unwrap_or(Value::Null)),
            "edit_write" => {
                let video = in_clip_dir(str_arg(a, "path")?)?;
                write_json(&sidecar(&video, "edit.json"), a.get("edit").ok_or("edit is missing")?)?;
                Ok(Value::Null)
            }
            "edit_delete" => {
                let video = in_clip_dir(str_arg(a, "path")?)?;
                match std::fs::remove_file(sidecar(&video, "edit.json")) {
                    Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
                    _ => Ok(Value::Null),
                }
            }
            "probe" => probe(str_arg(a, "path")?),
            "ffmpeg_run" => ffmpeg_run(a),
            "ffmpeg_info" => ffmpeg_info(),
            "job_start" => self.job_start(a),
            "job_status" => self.job_status(a),
            "job_cancel" => self.job_cancel(a),
            "cache_file" => cache_file(a),
            "cache_stats" => Ok(json!({ "bytes": app_cache_entries().iter().map(|p| dir_size(p)).sum::<u64>(), "dir": cache_dir() })),
            "cache_clear" => {
                for entry in app_cache_entries() {
                    std::fs::remove_dir_all(&entry).map_err(err("clear cache"))?;
                }
                Ok(Value::Null)
            }
            "drives" => drives(),
            "peaks" => peaks(a),
            "file_info" => {
                let meta = std::fs::metadata(str_arg(a, "path")?).ok();
                Ok(json!({ "exists": meta.is_some(), "size": meta.map(|m| m.len()).unwrap_or(0) }))
            }
            "unique_path" => unique_path(a),
            "sounds_list" => sounds_list(),
            "sound_data" => sound_data(a),
            "sound_set" => sound_set(a),
            "sound_reset" => sound_reset(a),
            "recorder_status" => recorder_status(),
            "recorder_start" => recorder_start(),
            "cloud_account" => Ok(json!({ "account": klip_cloud::account_fresh() })),
            "cloud_link_start" => {
                let label = format!("Klip on {}", std::env::var("COMPUTERNAME").unwrap_or_else(|_| "a PC".into()));
                klip_cloud::start_link(str_arg(a, "server")?, &label).map(|l| json!(l))
            }
            "cloud_link_poll" => klip_cloud::poll_link(str_arg(a, "server")?, str_arg(a, "device_code")?).map(|p| json!(p)),
            "cloud_sign_out" => klip_cloud::sign_out().map(|_| Value::Null),
            "cloud_upload" => {
                let video = in_clip_dir(str_arg(a, "path")?)?;
                let cfg = Config::load().unwrap_or_default().cloud;
                klip_cloud::upload(&video, &cfg).map(|r| json!(r))
            }
            "open_url" => open_url(str_arg(a, "url")?),
            "recorder_reload" => recorder_reload(),
            "cache_write" => cache_write(a),
            "inputs_read" => Ok(read_json(&sidecar(Path::new(str_arg(a, "path")?), "inputs.json"))?.unwrap_or(Value::Null)),
            "selftest_report" => {
                write_json(&data_dir().join("selftest.json"), a.get("report").unwrap_or(&Value::Null))?;
                Ok(Value::Null)
            }
            other => Err(format!("unknown command '{other}'")),
        }
    }

    fn job_start(&self, a: &Value) -> R {
        let args = list_arg(a, "args")?;
        let total_ms = a.get("total_ms").and_then(Value::as_u64).unwrap_or(0);
        let outputs: Vec<PathBuf> = list_arg(a, "outputs").unwrap_or_default().into_iter().map(PathBuf::from).collect();
        let renames: Vec<(PathBuf, PathBuf)> = a
            .get("renames")
            .and_then(Value::as_array)
            .map(|list| {
                list.iter()
                    .filter_map(|pair| Some((PathBuf::from(pair.get(0)?.as_str()?), PathBuf::from(pair.get(1)?.as_str()?))))
                    .collect()
            })
            .unwrap_or_default();
        for out in &outputs {
            if let Some(dir) = out.parent() {
                std::fs::create_dir_all(dir).map_err(err("create folder"))?;
            }
        }
        let mut child = tool(&ffmpeg_path())
            .args(["-hide_banner", "-nostdin", "-y", "-nostats", "-progress", "pipe:1"])
            .args(&args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("could not start {}: {e}", ffmpeg_path().display()))?;
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let job = Arc::new(Job {
            child: Mutex::new(child),
            state: Mutex::new(JobState { state: "running", ..Default::default() }),
            total_ms,
            outputs,
            renames,
        });
        let id = self.next_job.fetch_add(1, Ordering::Relaxed);
        self.jobs.lock().unwrap().insert(id, job.clone());

        // Progress arrives as key=value lines on stdout.
        if let Some(out) = stdout {
            let job = job.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(out).lines().map_while(Result::ok) {
                    let mut st = job.state.lock().unwrap();
                    if let Some(us) = line.strip_prefix("out_time_us=").and_then(|v| v.trim().parse::<i64>().ok()) {
                        st.out_ms = (us.max(0) / 1000) as u64;
                    } else if let Some(speed) = line.strip_prefix("speed=") {
                        st.speed = speed.trim().to_string();
                    }
                }
            });
        }
        if let Some(errs) = stderr {
            let job = job.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(errs).lines().map_while(Result::ok) {
                    let mut st = job.state.lock().unwrap();
                    st.log.push_str(&line);
                    st.log.push('\n');
                    // Only the tail matters, that is where ffmpeg names the error.
                    if st.log.len() > 6000 {
                        let mut cut = st.log.len() - 4000;
                        while !st.log.is_char_boundary(cut) {
                            cut += 1;
                        }
                        st.log.drain(..cut);
                    }
                }
            });
        }
        let waiter = job.clone();
        std::thread::spawn(move || loop {
            let status = waiter.child.lock().unwrap().try_wait();
            match status {
                Ok(None) => std::thread::sleep(Duration::from_millis(80)),
                Ok(Some(exit)) => {
                    // Give the reader threads a moment to drain the pipes.
                    std::thread::sleep(Duration::from_millis(60));
                    let mut st = waiter.state.lock().unwrap();
                    st.code = exit.code();
                    if st.state == "running" {
                        let renamed = exit.success() && waiter.renames.iter().all(|(from, to)| std::fs::rename(from, to).is_ok());
                        st.state = if renamed { "done" } else { "failed" };
                    }
                    if st.state != "done" {
                        waiter.remove_outputs();
                    }
                    return;
                }
                Err(_) => {
                    waiter.state.lock().unwrap().state = "failed";
                    return;
                }
            }
        });
        Ok(json!({ "id": id }))
    }

    fn job(&self, a: &Value) -> Result<Arc<Job>, String> {
        let id = a.get("id").and_then(Value::as_u64).ok_or("argument 'id' is missing")?;
        self.jobs.lock().unwrap().get(&id).cloned().ok_or(format!("job {id} does not exist"))
    }

    fn job_status(&self, a: &Value) -> R {
        let job = self.job(a)?;
        let st = job.state.lock().unwrap();
        let progress = match (st.state, job.total_ms) {
            ("done", _) => 1.0,
            (_, 0) => 0.0,
            (_, total) => (st.out_ms as f64 / total as f64).min(1.0),
        };
        Ok(json!({
            "state": st.state, "out_ms": st.out_ms, "progress": progress,
            "speed": st.speed, "log": st.log, "code": st.code,
        }))
    }

    fn job_cancel(&self, a: &Value) -> R {
        let job = self.job(a)?;
        {
            let mut st = job.state.lock().unwrap();
            if st.state == "running" {
                st.state = "cancelled";
            }
        }
        let _ = job.child.lock().unwrap().kill();
        Ok(Value::Null)
    }
}

fn env_info() -> R {
    let (config, _, _) = load_config();
    let clips = clip_dir();
    Ok(json!({
        "clip_dir": clips,
        "clip_dir_from_env": env_dir("KLIP_CLIP_DIR").is_some(),
        "default_clip_dir": klip_core::default_clip_dir(),
        "export_dir": clips.join(EXPORT_FOLDER),
        "data_dir": data_dir(),
        "cache_dir": cache_dir(),
        "cache_dir_from_env": env_dir("KLIP_CACHE_DIR").is_some(),
        "default_cache_dir": klip_core::default_cache_dir(),
        "config_path": config_path(),
        "default_ffmpeg": DEFAULT_FFMPEG,
        "ffmpeg": ffmpeg_path(),
        "selftest": std::env::var("KLIP_SELFTEST").ok(),
        "config_version": config.get("version"),
    }))
}

/// Letters of the drives that exist, as "C:". For the choice of a cache drive.
fn drives() -> R {
    let list: Vec<String> = (b'A'..=b'Z').map(|c| format!("{}:", c as char)).filter(|d| Path::new(&format!("{d}\\")).exists()).collect();
    Ok(json!({ "drives": list }))
}

fn config_save(a: &Value) -> R {
    let config = a.get("config").ok_or("config is missing")?;
    // Reject anything the recorder could not read.
    serde_json::from_value::<Config>(config.clone()).map_err(err("config is not valid"))?;
    write_json(&config_path(), config)?;
    Ok(Value::Null)
}

/// Event kinds of `klip_detect::EventKind`, by their serde names. The recorder drops the
/// whole rule file if it meets a name it does not know, so nothing else may be written.
const EVENT_KINDS: [&str; 13] = [
    "kill", "double_kill", "triple_kill", "quadra_kill", "ace", "death", "clutch", "spike_planted", "spike_defused",
    "round_won", "round_lost", "match_won", "match_lost",
];

/// Mirror of `klip_detect::Rule`. The app must not depend on the detector crate. The
/// numbers are f64 here so that 0.6 is written as 0.6, the recorder reads them as f32.
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy)]
struct AutoClipRule {
    enabled: bool,
    before_s: f64,
    after_s: f64,
}

/// Mirror of `klip_detect::RuleSet`, the format of `autoclip.json`.
#[derive(serde::Serialize, serde::Deserialize)]
struct AutoClipRules {
    rules: std::collections::BTreeMap<String, AutoClipRule>,
    #[serde(default = "default_merge_gap")]
    merge_gap_s: f64,
    #[serde(default = "default_max_clip")]
    max_clip_s: f64,
    #[serde(default)]
    min_confidence: f64,
}

fn default_merge_gap() -> f64 {
    2.0
}

fn default_max_clip() -> f64 {
    120.0
}

/// Same values as `impl Default for RuleSet` in klip-detect.
impl Default for AutoClipRules {
    fn default() -> Self {
        let rules = EVENT_KINDS
            .iter()
            .map(|kind| {
                let (enabled, before_s, after_s) = match *kind {
                    "kill" | "double_kill" | "triple_kill" | "quadra_kill" | "ace" => (true, 10.0, 5.0),
                    "clutch" => (true, 20.0, 5.0),
                    "match_won" | "match_lost" => (true, 15.0, 8.0),
                    "death" => (false, 10.0, 3.0),
                    "spike_planted" | "spike_defused" => (false, 10.0, 5.0),
                    _ => (false, 15.0, 5.0),
                };
                (kind.to_string(), AutoClipRule { enabled, before_s, after_s })
            })
            .collect();
        Self { rules, merge_gap_s: default_merge_gap(), max_clip_s: default_max_clip(), min_confidence: 0.0 }
    }
}

impl AutoClipRules {
    /// Rejects what the recorder could not read or what makes no sense as a time.
    fn check(&self) -> Result<(), String> {
        let time = |v: f64| v.is_finite() && (0.0..=3600.0).contains(&v);
        for (kind, rule) in &self.rules {
            if !EVENT_KINDS.contains(&kind.as_str()) {
                return Err(format!("unknown event '{kind}'"));
            }
            if !time(rule.before_s) || !time(rule.after_s) {
                return Err(format!("times of '{kind}' are out of range"));
            }
        }
        if !time(self.merge_gap_s) || !time(self.max_clip_s) {
            return Err("merge_gap_s or max_clip_s is out of range".into());
        }
        if !(self.min_confidence.is_finite() && (0.0..=1.0).contains(&self.min_confidence)) {
            return Err("min_confidence must lie between 0 and 1".into());
        }
        Ok(())
    }
}

fn autoclip_path() -> PathBuf {
    data_dir().join("autoclip.json")
}

/// The rules as the recorder would use them: a missing or unreadable file means the
/// defaults there too, so the defaults are what is shown.
fn autoclip_load() -> R {
    let defaults = serde_json::to_value(AutoClipRules::default()).map_err(err("encode"))?;
    let path = autoclip_path();
    let (rules, exists, error) = match read_json(&path) {
        Ok(None) => (defaults, false, None),
        Err(e) => (defaults, true, Some(e)),
        Ok(Some(raw)) => match serde_json::from_value::<AutoClipRules>(raw).map_err(|e| e.to_string()).and_then(|r| r.check().map(|_| r)) {
            Ok(typed) => (serde_json::to_value(typed).map_err(err("encode"))?, true, None),
            Err(e) => (defaults, true, Some(format!("{}: {e}", path.display()))),
        },
    };
    Ok(json!({ "rules": rules, "exists": exists, "error": error, "path": path }))
}

fn autoclip_save(a: &Value) -> R {
    let rules = a.get("rules").ok_or("rules is missing")?;
    let typed = serde_json::from_value::<AutoClipRules>(rules.clone()).map_err(err("rules are not valid"))?;
    typed.check()?;
    // Written from the typed form, so the file holds exactly the fields the recorder knows.
    write_json(&autoclip_path(), &serde_json::to_value(typed).map_err(err("encode"))?)?;
    Ok(Value::Null)
}

fn settings_export(a: &Value) -> R {
    let (config, _, _) = load_config();
    let app = read_json(&settings_path())?.unwrap_or_else(|| json!({}));
    let bundle = json!({ "klip_settings": 1, "config": config, "app": app });
    write_json(Path::new(str_arg(a, "path")?), &bundle)?;
    Ok(Value::Null)
}

fn scan_dir(dir: &Path, root: &Path, cache: &Path, depth: u32, items: &mut Vec<Value>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = entry.metadata() else { continue };
        let name = entry.file_name().to_string_lossy().to_string();
        if meta.is_dir() {
            let skip = name.starts_with('.') || (depth == 0 && name.eq_ignore_ascii_case(EXPORT_FOLDER));
            if !skip && depth < 6 {
                scan_dir(&path, root, cache, depth + 1, items);
            }
            continue;
        }
        // The webcam of a clip belongs to that clip, it is no clip of its own.
        if name.to_lowercase().ends_with(".cam.mp4") {
            continue;
        }
        let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
        let media = if VIDEO_EXT.contains(&ext.as_str()) {
            "video"
        } else if IMAGE_EXT.contains(&ext.as_str()) {
            "image"
        } else {
            continue;
        };
        // The sidecar is the truth. A broken one is reported, the clip still shows up.
        let (clip_meta, meta_error) = match read_json(&sidecar(&path, "klip.json")) {
            Ok(Some(raw)) => match serde_json::from_value::<ClipMeta>(raw.clone()) {
                Ok(_) => (raw, None),
                Err(e) => (Value::Null, Some(e.to_string())),
            },
            Ok(None) => (Value::Null, None),
            Err(e) => (Value::Null, Some(e)),
        };
        let camera = Some(sidecar(&path, "cam.mp4")).filter(|c| c.exists());
        let key = cache_key(&path, &meta);
        let thumb = cache.join(&key).join("thumb.jpg");
        let folder = path.parent().and_then(|p| p.strip_prefix(root).ok()).map(|p| p.to_string_lossy().to_string());
        items.push(json!({
            "path": path,
            "name": path.file_stem().map(|s| s.to_string_lossy().to_string()),
            "ext": ext,
            "folder": folder.unwrap_or_default(),
            "media": media,
            "size": meta.len(),
            "modified_ms": modified_ms(&meta),
            "meta": clip_meta,
            "meta_error": meta_error,
            "has_edit": sidecar(&path, "edit.json").exists(),
            "has_inputs": sidecar(&path, "inputs.json").exists(),
            "camera": camera,
            "thumb": if thumb.exists() { json!(thumb) } else { Value::Null },
        }));
    }
}

fn library_scan() -> R {
    let root = clip_dir();
    let mut items = Vec::new();
    let exists = root.is_dir();
    if exists {
        // Looked up once: the cache folder comes from the settings file.
        scan_dir(&root, &root, &cache_dir(), 0, &mut items);
    }
    Ok(json!({ "dir": root, "exists": exists, "items": items }))
}

fn thumb(a: &Value) -> R {
    let path = PathBuf::from(str_arg(a, "path")?);
    let meta = std::fs::metadata(&path).map_err(err("thumbnail source"))?;
    let dir = cache_dir().join(cache_key(&path, &meta));
    let out = dir.join("thumb.jpg");
    if !out.exists() {
        std::fs::create_dir_all(&dir).map_err(err("create cache"))?;
        let at = a.get("at_ms").and_then(Value::as_u64).unwrap_or(0) as f64 / 1000.0;
        let tmp = dir.join("thumb.tmp.jpg");
        let status = tool(&ffmpeg_path())
            .args(["-hide_banner", "-loglevel", "error", "-y", "-ss", &format!("{at:.3}"), "-i"])
            .arg(&path)
            .args(["-frames:v", "1", "-vf", "scale=480:-2", "-q:v", "4"])
            .arg(&tmp)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output()
            .map_err(|e| format!("could not start {}: {e}", ffmpeg_path().display()))?;
        if !status.status.success() || !tmp.exists() {
            let _ = std::fs::remove_file(&tmp);
            return Err(format!("no thumbnail: {}", String::from_utf8_lossy(&status.stderr).trim()));
        }
        std::fs::rename(&tmp, &out).map_err(err("store thumbnail"))?;
    }
    Ok(json!({ "path": out }))
}

fn meta_write(a: &Value) -> R {
    let video = in_clip_dir(str_arg(a, "path")?)?;
    let meta = a.get("meta").ok_or("meta is missing")?;
    serde_json::from_value::<ClipMeta>(meta.clone()).map_err(err("sidecar is not valid"))?;
    write_json(&sidecar(&video, "klip.json"), meta)?;
    Ok(Value::Null)
}

fn rename(a: &Value) -> R {
    let video = in_clip_dir(str_arg(a, "path")?)?;
    let name = str_arg(a, "name")?.trim();
    let bad = name.is_empty() || name.ends_with('.') || name.chars().any(|c| "\\/:*?\"<>|".contains(c) || c.is_control());
    if bad {
        return Err("name is empty or contains one of \\ / : * ? \" < > |".into());
    }
    let ext = video.extension().map(|e| e.to_string_lossy().to_string()).unwrap_or_default();
    let target = video.with_file_name(format!("{name}.{ext}"));
    if lower(&target) == lower(&video) {
        return Ok(json!({ "path": video }));
    }
    if target.exists() {
        return Err(format!("{} already exists", target.display()));
    }
    std::fs::rename(&video, &target).map_err(err("rename"))?;
    for ext in SIDECARS {
        let from = sidecar(&video, ext);
        if from.exists() {
            std::fs::rename(&from, sidecar(&target, ext)).map_err(err("rename sidecar"))?;
        }
    }
    Ok(json!({ "path": target }))
}

/// Moves clips and their sidecars to the Windows recycle bin. Nothing is ever deleted
/// for good from here.
fn trash_clips(a: &Value) -> R {
    let mut trashed = Vec::new();
    let mut errors = Vec::new();
    for path in list_arg(a, "paths")? {
        let video = match in_clip_dir(&path) {
            Ok(v) => v,
            Err(e) => {
                errors.push(e);
                continue;
            }
        };
        let mut files = vec![video.clone()];
        files.extend(SIDECARS.iter().map(|ext| sidecar(&video, ext)).filter(|p| p.exists()));
        match trash::delete_all(&files) {
            Ok(()) => trashed.push(path),
            Err(e) => errors.push(format!("{path}: {e}")),
        }
    }
    Ok(json!({ "trashed": trashed, "errors": errors }))
}

fn reveal(a: &Value) -> R {
    let path = PathBuf::from(str_arg(a, "path")?);
    let mut cmd = Command::new("explorer.exe");
    if path.is_dir() {
        cmd.arg(&path);
    } else if path.exists() {
        cmd.raw_arg(format!("/select,\"{}\"", path.display()));
    } else {
        return Err(format!("{} does not exist", path.display()));
    }
    cmd.spawn().map_err(err("open Explorer"))?;
    Ok(Value::Null)
}

/// Puts files on the clipboard the way Explorer does (CF_HDROP), so that Ctrl+V in
/// Discord or a folder pastes the file itself.
fn clipboard_files(a: &Value) -> R {
    use clipboard_win::{formats, Clipboard, Setter};
    let paths = list_arg(a, "paths")?;
    if let Some(missing) = paths.iter().find(|p| !Path::new(p).exists()) {
        return Err(format!("{missing} does not exist"));
    }
    let _clip = Clipboard::new_attempts(10).map_err(err("open clipboard"))?;
    formats::FileList.write_clipboard(&paths).map_err(err("write clipboard"))?;
    Ok(Value::Null)
}

fn disk(a: &Value) -> R {
    let mut path = a.get("path").and_then(Value::as_str).map(PathBuf::from).unwrap_or_else(clip_dir);
    // The clip folder may not exist yet, ask the closest folder that does.
    while !path.exists() {
        match path.parent() {
            Some(parent) => path = parent.to_path_buf(),
            None => break,
        }
    }
    Ok(json!({
        "free": fs4::available_space(&path).map_err(err("free space"))?,
        "total": fs4::total_space(&path).map_err(err("disk size"))?,
    }))
}

fn probe(path: &str) -> R {
    let out = tool(&ffprobe_path())
        .args(["-v", "error", "-print_format", "json", "-show_format", "-show_streams"])
        .arg(path)
        .output()
        .map_err(|e| format!("could not start {}: {e}", ffprobe_path().display()))?;
    if !out.status.success() {
        return Err(format!("ffprobe: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    serde_json::from_slice(&out.stdout).map_err(err("ffprobe output"))
}

/// Runs ffmpeg to the end and returns exit code and messages. For short runs such as the
/// encoder trial, long work goes through jobs.
fn ffmpeg_run(a: &Value) -> R {
    let args = list_arg(a, "args")?;
    let timeout = Duration::from_millis(a.get("timeout_ms").and_then(Value::as_u64).unwrap_or(20_000));
    let mut child = tool(&ffmpeg_path())
        .args(["-hide_banner", "-nostdin", "-y"])
        .args(&args)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not start {}: {e}", ffmpeg_path().display()))?;
    let mut stderr = child.stderr.take();
    let reader = std::thread::spawn(move || {
        let mut text = String::new();
        if let Some(pipe) = stderr.as_mut() {
            let mut bytes = Vec::new();
            let _ = pipe.read_to_end(&mut bytes);
            text = String::from_utf8_lossy(&bytes).to_string();
        }
        text
    });
    let started = Instant::now();
    let code = loop {
        match child.try_wait().map_err(err("wait for ffmpeg"))? {
            Some(status) => break status.code(),
            None if started.elapsed() > timeout => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            None => std::thread::sleep(Duration::from_millis(20)),
        }
    };
    let log = reader.join().unwrap_or_default();
    let tail: String = log.chars().rev().take(1500).collect::<Vec<_>>().into_iter().rev().collect();
    Ok(json!({ "code": code, "log": tail, "ms": started.elapsed().as_millis() as u64 }))
}

fn ffmpeg_info() -> R {
    let path = ffmpeg_path();
    match tool(&path).arg("-version").output() {
        Ok(out) if out.status.success() => {
            let text = String::from_utf8_lossy(&out.stdout).to_string();
            Ok(json!({ "path": path, "ok": true, "version": text.lines().next().unwrap_or_default() }))
        }
        Ok(out) => Ok(json!({ "path": path, "ok": false, "version": String::from_utf8_lossy(&out.stderr).trim() })),
        Err(e) => Ok(json!({ "path": path, "ok": false, "version": e.to_string() })),
    }
}

fn cache_file(a: &Value) -> R {
    let source = PathBuf::from(str_arg(a, "source")?);
    let name = str_arg(a, "name")?;
    if name.contains(['\\', '/', ':']) || name.contains("..") {
        return Err("bad cache file name".into());
    }
    let meta = std::fs::metadata(&source).map_err(err("cache source"))?;
    let dir = cache_dir().join(cache_key(&source, &meta));
    std::fs::create_dir_all(&dir).map_err(err("create cache"))?;
    let path = dir.join(name);
    let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    Ok(json!({ "path": path, "exists": size > 0, "size": size }))
}

/// Reads mono signed 16 bit samples and returns the loudest value of each bucket, 0 to 1.
fn peaks(a: &Value) -> R {
    let bytes = std::fs::read(str_arg(a, "path")?).map_err(err("waveform data"))?;
    let rate = a.get("rate").and_then(Value::as_u64).unwrap_or(4000).max(1) as usize;
    let per_second = a.get("per_second").and_then(Value::as_u64).unwrap_or(50).clamp(1, rate as u64) as usize;
    let bucket = (rate / per_second).max(1);
    let samples = bytes.len() / 2;
    let mut out = Vec::with_capacity(samples / bucket + 1);
    for chunk in bytes[..samples * 2].chunks(bucket * 2) {
        let peak = chunk.chunks_exact(2).map(|s| i16::from_le_bytes([s[0], s[1]]).unsigned_abs()).max().unwrap_or(0);
        out.push((f64::from(peak) / 32768.0 * 1000.0).round() / 1000.0);
    }
    Ok(json!({ "peaks": out, "per_second": rate as f64 / bucket as f64 }))
}

/// A path in `dir` that does not exist yet: `stem.ext`, then `stem_2.ext` and so on.
fn unique_path(a: &Value) -> R {
    let dir = PathBuf::from(str_arg(a, "dir")?);
    let stem = str_arg(a, "stem")?;
    let ext = str_arg(a, "ext")?;
    if !dir.is_absolute() {
        return Err(format!("{} is not an absolute folder", dir.display()));
    }
    std::fs::create_dir_all(&dir).map_err(err("create folder"))?;
    let mut path = dir.join(format!("{stem}.{ext}"));
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("{stem}_{n}.{ext}"));
        n += 1;
    }
    Ok(json!({ "path": path }))
}

/// One request to the recorder over its named pipe: a line of JSON there, a line back.
/// Returns `None` when no recorder listens. A helper thread does the talking, so a
/// recorder that hangs cannot freeze the app.
fn recorder_call(request: Value) -> Option<Value> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let answer = (|| -> Option<Value> {
            let mut pipe = std::fs::OpenOptions::new().read(true).write(true).open(klip_core::PIPE_NAME).ok()?;
            let mut line = serde_json::to_vec(&request).ok()?;
            line.push(b'\n');
            std::io::Write::write_all(&mut pipe, &line).ok()?;
            let mut text = String::new();
            BufReader::new(pipe).read_line(&mut text).ok()?;
            serde_json::from_str(&text).ok()
        })();
        let _ = tx.send(answer);
    });
    rx.recv_timeout(Duration::from_millis(1500)).ok().flatten()
}

fn recorder_status() -> R {
    // For pictures of the app that must not show what runs on this machine right now.
    if env_dir("KLIP_NO_RECORDER").is_some() {
        return Ok(json!({ "connected": false }));
    }
    match recorder_call(json!({ "cmd": "status" })) {
        Some(answer) => Ok(json!({ "connected": true, "status": answer.get("status"), "error": answer.get("error") })),
        None => Ok(json!({ "connected": false })),
    }
}

/// Starts the recorder that sits next to the app, unless one already answers. Without the
/// recorder there is no buffer and no clip, and nobody should have to know that it is a
/// program of its own. Does nothing while the settings live in a test folder.
pub fn recorder_start() -> R {
    if env_dir("KLIP_DATA_DIR").is_some() || env_dir("KLIP_NO_RECORDER").is_some() {
        return Ok(json!({ "started": false, "skipped": true }));
    }
    if recorder_call(json!({ "cmd": "status" })).is_some() {
        return Ok(json!({ "started": false, "running": true }));
    }
    let exe = std::env::current_exe().map_err(err("find the app"))?;
    let recorder = exe.parent().map(|d| d.join("klip-recorder.exe")).filter(|p| p.is_file());
    let Some(recorder) = recorder else {
        return Err("klip-recorder.exe is not next to the app".into());
    };
    Command::new(&recorder).arg("--tray").spawn().map_err(err("start the recorder"))?;
    Ok(json!({ "started": true }))
}

/// Tells the recorder to read `config.json` again. Skipped while the settings live in a
/// test folder: the recorder only knows the real file and must not be poked by tests.
fn recorder_reload() -> R {
    if env_dir("KLIP_DATA_DIR").is_some() {
        return Ok(json!({ "connected": false, "skipped": true }));
    }
    match recorder_call(json!({ "cmd": "reload_config" })) {
        Some(answer) => Ok(json!({ "connected": true, "ok": answer.get("ok"), "error": answer.get("error") })),
        None => Ok(json!({ "connected": false })),
    }
}

/// Stores a small file made by the UI (a filter script, an overlay picture) in the
/// cache folder of a clip and returns its path.
fn cache_write(a: &Value) -> R {
    use base64::Engine;
    let info = cache_file(a)?;
    let path = PathBuf::from(info.get("path").and_then(Value::as_str).unwrap_or_default());
    let bytes = match (a.get("text").and_then(Value::as_str), a.get("base64").and_then(Value::as_str)) {
        (Some(text), _) => text.as_bytes().to_vec(),
        (None, Some(data)) => base64::engine::general_purpose::STANDARD.decode(data).map_err(err("base64"))?,
        _ => return Err("text or base64 is missing".into()),
    };
    std::fs::write(&path, bytes).map_err(err("write cache file"))?;
    Ok(json!({ "path": path }))
}
/// The notice sounds, by the names the overlay knows, with the built in file of each.
const SOUNDS: [(&str, &[u8]); 6] = [
    ("clip", include_bytes!("../../../assets/sounds/clip.wav")),
    ("auto", include_bytes!("../../../assets/sounds/auto.wav")),
    ("rec_start", include_bytes!("../../../assets/sounds/rec_start.wav")),
    ("rec_stop", include_bytes!("../../../assets/sounds/rec_stop.wav")),
    ("shot", include_bytes!("../../../assets/sounds/shot.wav")),
    ("tick", include_bytes!("../../../assets/sounds/tick.wav")),
];

/// The overlay ignores larger files, so the app does not accept them either.
const MAX_SOUND_BYTES: u64 = 2 * 1024 * 1024;

fn sounds_dir() -> PathBuf {
    data_dir().join("sounds")
}

/// The user's own file for a sound. Fails for a name the overlay does not know, so a
/// caller can never reach a path outside the sound folder.
fn own_sound(a: &Value) -> Result<(usize, PathBuf), String> {
    let name = str_arg(a, "name")?;
    let at = SOUNDS.iter().position(|(n, _)| *n == name).ok_or(format!("unknown sound '{name}'"))?;
    Ok((at, sounds_dir().join(format!("{name}.wav"))))
}

fn is_wav(bytes: &[u8]) -> bool {
    bytes.len() > 44 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WAVE"
}

fn sounds_list() -> R {
    let list: Vec<Value> = SOUNDS
        .iter()
        .map(|(name, _)| json!({ "name": name, "custom": sounds_dir().join(format!("{name}.wav")).is_file() }))
        .collect();
    Ok(json!({ "dir": sounds_dir(), "sounds": list }))
}

/// The sound as it will play: the user's own file if there is a usable one, else the
/// built in one. Base64, for the preview in the settings.
fn sound_data(a: &Value) -> R {
    use base64::Engine;
    let (at, path) = own_sound(a)?;
    let own = std::fs::read(&path).ok().filter(|b| is_wav(b) && b.len() as u64 <= MAX_SOUND_BYTES);
    let custom = own.is_some();
    let bytes = own.unwrap_or_else(|| SOUNDS[at].1.to_vec());
    Ok(json!({ "base64": base64::engine::general_purpose::STANDARD.encode(bytes), "custom": custom }))
}

/// Copies a WAV file the user picked into the sound folder. The picked file stays.
fn sound_set(a: &Value) -> R {
    let (_, target) = own_sound(a)?;
    let source = PathBuf::from(str_arg(a, "path")?);
    let size = std::fs::metadata(&source).map_err(err("read the file"))?.len();
    if size > MAX_SOUND_BYTES {
        return Err("the file is larger than 2 MB".into());
    }
    let bytes = std::fs::read(&source).map_err(err("read the file"))?;
    if !is_wav(&bytes) {
        return Err("this is not a WAV file".into());
    }
    std::fs::create_dir_all(sounds_dir()).map_err(err("create the sound folder"))?;
    std::fs::write(&target, bytes).map_err(err("write the sound"))?;
    Ok(json!({ "path": target }))
}

/// Back to the built in sound: removes the copy in the sound folder.
fn sound_reset(a: &Value) -> R {
    let (_, target) = own_sound(a)?;
    match std::fs::remove_file(&target) {
        Ok(()) => Ok(Value::Null),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Value::Null),
        Err(e) => Err(format!("remove the sound: {e}")),
    }
}

/// Opens a web address in the default browser. Only http and https.
fn open_url(url: &str) -> R {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err("only web addresses can be opened".into());
    }
    // explorer.exe drops addresses with a query ("?code=...") on many machines. The URL
    // handler of Windows opens them in the default browser like a click on a link.
    Command::new("rundll32.exe").arg("url.dll,FileProtocolHandler").arg(url).spawn().map_err(err("open the browser"))?;
    Ok(Value::Null)
}

/// Used by the transports to fill in an empty map when a caller sends no arguments.
pub fn empty_args() -> Value {
    Value::Object(Map::new())
}
