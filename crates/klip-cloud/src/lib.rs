//! Klip Cloud from the PC: link this PC to a Discord account on a Klip Cloud server,
//! upload clips, and keep the access token safe.
//!
//! The token never sits in a file in plain text: Windows encrypts it for the signed in
//! user (DPAPI), so another account on the same PC, or a copy of the file, cannot use it.
//! Nothing here talks to any server but the one in the settings.

use base64::Engine;
use klip_core::{data_dir, ClipMeta, CloudConfig, CloudRef};
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

pub type Res<T> = Result<T, String>;

/// The HTTP client Klip uses everywhere: Windows TLS, a Klip user agent, sane timeouts.
pub fn http() -> ureq::Agent {
    agent()
}

fn agent() -> ureq::Agent {
    let tls = native_tls::TlsConnector::new().map(Arc::new);
    let mut builder = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(60))
        .user_agent(concat!("Klip/", env!("CARGO_PKG_VERSION")));
    if let Ok(tls) = tls {
        builder = builder.tls_connector(tls);
    }
    builder.build()
}

fn url(server: &str, path: &str) -> String {
    format!("{}{}", server.trim_end_matches('/'), path)
}

/// The message of the server, or a sentence for a failed connection.
fn explain(error: ureq::Error) -> String {
    match error {
        ureq::Error::Status(code, res) => {
            let body: serde_json::Value = res.into_json().unwrap_or_default();
            body.get("error").and_then(|e| e.as_str()).map(str::to_string).unwrap_or(format!("the server answered {code}"))
        }
        ureq::Error::Transport(t) => format!("the server cannot be reached: {t}"),
    }
}

// ---- the token -------------------------------------------------------------------

#[derive(Serialize, Deserialize)]
struct Stored {
    server: String,
    name: String,
    /// Address of the Discord profile picture.
    #[serde(default)]
    avatar: Option<String>,
    /// The token, encrypted by Windows for this user, in base64.
    token: String,
}

fn token_file() -> PathBuf {
    data_dir().join("cloud.json")
}

fn protect(data: &[u8], encrypt: bool) -> Res<Vec<u8>> {
    use windows::Win32::Foundation::{HLOCAL, LocalFree};
    use windows::Win32::Security::Cryptography::{CryptProtectData, CryptUnprotectData, CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN};
    let input = CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_ptr() as *mut u8 };
    let mut output = CRYPT_INTEGER_BLOB::default();
    unsafe {
        let done = if encrypt {
            CryptProtectData(&input, None, None, None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut output)
        } else {
            CryptUnprotectData(&input, None, None, None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut output)
        };
        done.map_err(|e| format!("Windows could not {} the sign in: {e}", if encrypt { "store" } else { "read" }))?;
        let bytes = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        let _ = LocalFree(Some(HLOCAL(output.pbData as *mut _)));
        Ok(bytes)
    }
}

/// Who is signed in on which server, if anyone. The token itself stays inside.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Account {
    pub server: String,
    pub name: String,
    pub avatar: Option<String>,
    /// Clips counted for the leaderboard, and whether the account shows up there. Only
    /// known after a look at the server.
    pub clips_made: Option<u64>,
    pub on_board: Option<bool>,
}

fn stored() -> Option<Stored> {
    serde_json::from_slice(&std::fs::read(token_file()).ok()?).ok()
}

pub fn account() -> Option<Account> {
    let s = stored()?;
    Some(Account { server: s.server, name: s.name, avatar: s.avatar, clips_made: None, on_board: None })
}

/// Asks the server for the current name and picture, a changed Discord profile shows up
/// in Klip this way. Falls back to what is stored when the server cannot be reached.
pub fn account_fresh() -> Option<Account> {
    let mut s = stored()?;
    let (mut clips_made, mut on_board) = (None, None);
    if let Ok(token) = token_for(&s.server) {
        let res = agent().get(&url(&s.server, "/api/me")).set("Authorization", &format!("Bearer {token}")).call();
        if let Ok(body) = res.map_err(explain).and_then(|r| r.into_json::<serde_json::Value>().map_err(|e| e.to_string())) {
            if let Some(user) = body.get("user").filter(|u| u.is_object()) {
                let name = user.get("name").and_then(|n| n.as_str()).unwrap_or(&s.name).to_string();
                let avatar = user.get("avatar").and_then(|a| a.as_str()).map(str::to_string);
                clips_made = user.get("clips_made").and_then(|c| c.as_u64());
                on_board = user.get("on_board").and_then(|b| b.as_bool());
                if name != s.name || avatar != s.avatar {
                    s.name = name;
                    s.avatar = avatar;
                    let _ = std::fs::write(token_file(), serde_json::to_vec_pretty(&s).unwrap_or_default());
                }
            }
        }
    }
    Some(Account { server: s.server, name: s.name, avatar: s.avatar, clips_made, on_board })
}

/// Adds `clips` to the count of the signed in account on the leaderboard. Only the number
/// leaves the PC, never a clip.
pub fn report_clips(clips: u32) -> Res<()> {
    let s = stored().ok_or("this PC is not signed in to Klip Cloud")?;
    let token = token_for(&s.server)?;
    agent()
        .post(&url(&s.server, "/api/stats"))
        .set("Authorization", &format!("Bearer {token}"))
        .send_json(serde_json::json!({ "clips": clips }))
        .map_err(explain)?;
    Ok(())
}

/// Shows or hides the signed in account on the public leaderboard.
pub fn set_on_board(show: bool) -> Res<()> {
    let s = stored().ok_or("this PC is not signed in to Klip Cloud")?;
    let token = token_for(&s.server)?;
    agent()
        .put(&url(&s.server, "/api/me/board"))
        .set("Authorization", &format!("Bearer {token}"))
        .send_json(serde_json::json!({ "show": show }))
        .map_err(explain)?;
    Ok(())
}

/// The token for `server`, if this PC is linked to it.
fn token_for(server: &str) -> Res<String> {
    let bytes = std::fs::read(token_file()).map_err(|_| "this PC is not signed in to Klip Cloud".to_string())?;
    let stored: Stored = serde_json::from_slice(&bytes).map_err(|_| "the sign in file is damaged, sign in again".to_string())?;
    if stored.server.trim_end_matches('/') != server.trim_end_matches('/') {
        return Err("this PC is signed in to another server, sign in again".into());
    }
    let sealed = base64::engine::general_purpose::STANDARD.decode(stored.token).map_err(|e| e.to_string())?;
    String::from_utf8(protect(&sealed, false)?).map_err(|e| e.to_string())
}

fn save_token(server: &str, name: &str, avatar: Option<String>, token: &str) -> Res<()> {
    let sealed = protect(token.as_bytes(), true)?;
    let stored = Stored { server: server.to_string(), name: name.to_string(), avatar, token: base64::engine::general_purpose::STANDARD.encode(sealed) };
    std::fs::create_dir_all(data_dir()).map_err(|e| e.to_string())?;
    std::fs::write(token_file(), serde_json::to_vec_pretty(&stored).unwrap_or_default()).map_err(|e| e.to_string())
}

/// Signs this PC out. The server keeps the device in its list until it is removed there.
pub fn sign_out() -> Res<()> {
    match std::fs::remove_file(token_file()) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

// ---- linking this PC -------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Link {
    pub device_code: String,
    pub user_code: String,
    pub verify_url: String,
    pub interval: u64,
    pub expires_in: u64,
}

/// Asks the server for a code. The user opens `verify_url`, signs in with Discord and
/// confirms the code, then `poll_link` gets the token.
pub fn start_link(server: &str, label: &str) -> Res<Link> {
    agent()
        .post(&url(server, "/api/device"))
        .send_json(serde_json::json!({ "label": label }))
        .map_err(explain)?
        .into_json()
        .map_err(|e| e.to_string())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Poll {
    Pending,
    Done { name: String, avatar: Option<String> },
}

pub fn poll_link(server: &str, device_code: &str) -> Res<Poll> {
    let res = agent().post(&url(server, "/api/device/token")).send_json(serde_json::json!({ "device_code": device_code })).map_err(explain)?;
    if res.status() == 202 {
        return Ok(Poll::Pending);
    }
    let body: serde_json::Value = res.into_json().map_err(|e| e.to_string())?;
    let token = body.get("token").and_then(|t| t.as_str()).ok_or("the server sent no token")?;
    let name = body.pointer("/user/name").and_then(|n| n.as_str()).unwrap_or("").to_string();
    let avatar = body.pointer("/user/avatar").and_then(|a| a.as_str()).map(str::to_string);
    save_token(server, &name, avatar.clone(), token)?;
    Ok(Poll::Done { name, avatar })
}

// ---- uploading -------------------------------------------------------------------

/// A JPEG of the clip for the web page, taken with ffmpeg if it is there. Without it the
/// clip goes up without a picture.
fn thumbnail(video: &Path) -> Option<Vec<u8>> {
    use std::os::windows::process::CommandExt;
    let app: serde_json::Value = std::fs::read(data_dir().join("app.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    let chosen = app.get("ffmpeg_path").and_then(|p| p.as_str()).filter(|p| !p.is_empty()).map(PathBuf::from);
    let ffmpeg = chosen.unwrap_or_else(|| PathBuf::from(r"C:\ffmpeg\bin\ffmpeg.exe"));
    let ffmpeg = if ffmpeg.exists() { ffmpeg } else { PathBuf::from("ffmpeg") };
    let out = std::process::Command::new(ffmpeg)
        .args(["-v", "error", "-ss", "1", "-i"])
        .arg(video)
        .args(["-frames:v", "1", "-vf", "scale=640:-2", "-q:v", "5", "-f", "mjpeg", "-"])
        .creation_flags(0x0800_0000)
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    (out.status.success() && !out.stdout.is_empty()).then_some(out.stdout)
}

/// Uploads one clip and returns its place in the cloud. The sidecar of the clip records
/// it, so the same clip is not sent twice. With `keep_local` off the clip then goes to
/// the recycle bin, never deleted for good.
pub fn upload(video: &Path, cfg: &CloudConfig) -> Res<CloudRef> {
    let token = token_for(&cfg.server)?;
    let sidecar = ClipMeta::sidecar_path(video);
    let meta: Option<ClipMeta> = std::fs::read(&sidecar).ok().and_then(|b| serde_json::from_slice(&b).ok());
    if let Some(done) = meta.as_ref().and_then(|m| m.cloud.clone()) {
        if done.server == cfg.server {
            return Ok(done);
        }
    }
    let name = video.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "Clip".into());
    let header = meta
        .as_ref()
        .map(|m| {
            serde_json::json!({
                "kind": m.kind, "game": m.game, "reason": m.reason, "created": m.created,
                "duration_ms": m.duration_ms, "width": m.width, "height": m.height,
            })
        })
        .unwrap_or_else(|| serde_json::json!({}));
    let file = std::fs::File::open(video).map_err(|e| format!("cannot read {}: {e}", video.display()))?;
    let size = file.metadata().map_err(|e| e.to_string())?.len();
    let res = agent()
        .put(&url(&cfg.server, "/api/clips"))
        .timeout(Duration::from_secs(3600))
        .set("Authorization", &format!("Bearer {token}"))
        .set("Content-Type", "video/mp4")
        .set("Content-Length", &size.to_string())
        .set("X-Klip-Name", &ascii_header(&name))
        .set("X-Klip-Meta", &base64::engine::general_purpose::STANDARD.encode(header.to_string()))
        .send(file)
        .map_err(explain)?;
    let body: serde_json::Value = res.into_json().map_err(|e| e.to_string())?;
    let id = body.get("id").and_then(|i| i.as_str()).ok_or("the server sent no id")?.to_string();

    if let Some(jpeg) = thumbnail(video) {
        let _ = agent()
            .put(&url(&cfg.server, &format!("/api/clips/{id}/thumb")))
            .set("Authorization", &format!("Bearer {token}"))
            .set("Content-Type", "image/jpeg")
            .send_bytes(&jpeg);
    }

    let done = CloudRef { server: cfg.server.clone(), id, uploaded: now_rfc3339() };
    if let Some(mut m) = meta {
        m.cloud = Some(done.clone());
        let _ = m.write_next_to(video);
    }
    if !cfg.keep_local {
        let mut files = vec![video.to_path_buf()];
        for ext in ["klip.json", "edit.json", "inputs.json", "cam.mp4"] {
            let p = video.with_extension(ext);
            if p.exists() {
                files.push(p);
            }
        }
        recycle(&files)?;
    }
    Ok(done)
}

/// HTTP headers carry ASCII. Other letters go as percent escapes, the server decodes them.
fn ascii_header(text: &str) -> String {
    let mut out = String::new();
    for b in text.bytes() {
        if b.is_ascii_graphic() || b == b' ' {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn now_rfc3339() -> String {
    // The sidecar already has a clock helper in the recorder; here seconds since 1970 are
    // enough for sorting, written in the RFC 3339 shape the rest of Klip reads.
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let days = secs / 86_400;
    let (y, m, d) = civil(days as i64);
    let rem = secs % 86_400;
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

/// Days since 1970 as a calendar date (Howard Hinnant's algorithm).
fn civil(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Moves files to the recycle bin of Windows. They can be restored from there.
pub fn recycle(files: &[PathBuf]) -> Res<()> {
    use windows::core::PCWSTR;
    use windows::Win32::UI::Shell::{SHFileOperationW, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT, FO_DELETE, SHFILEOPSTRUCTW};
    if files.is_empty() {
        return Ok(());
    }
    // A list of paths, each ended by a zero, the whole list by another zero.
    let mut list: Vec<u16> = Vec::new();
    for f in files {
        list.extend(f.as_os_str().to_string_lossy().encode_utf16());
        list.push(0);
    }
    list.push(0);
    let mut op = SHFILEOPSTRUCTW {
        wFunc: FO_DELETE,
        pFrom: PCWSTR(list.as_ptr()),
        fFlags: (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_NOERRORUI | FOF_SILENT).0 as u16,
        ..Default::default()
    };
    let code = unsafe { SHFileOperationW(&mut op) };
    if code != 0 {
        return Err(format!("the local copy could not go to the recycle bin (code {code})"));
    }
    Ok(())
}

/// Reads a whole response body as text, for small answers.
pub fn read_text(mut r: impl Read) -> String {
    let mut s = String::new();
    let _ = r.read_to_string(&mut s);
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calendar_dates() {
        assert_eq!(civil(0), (1970, 1, 1));
        assert_eq!(civil(20_376), (2025, 10, 15));
        assert_eq!(civil(11_016), (2000, 2, 29));
    }

    #[test]
    fn headers_stay_ascii() {
        assert_eq!(ascii_header("Ace on Ascent"), "Ace on Ascent");
        assert_eq!(ascii_header("Grün"), "Gr%C3%BCn");
    }

    #[test]
    fn the_token_survives_windows_encryption() {
        let sealed = protect(b"secret-token", true).unwrap();
        assert_ne!(sealed, b"secret-token");
        assert_eq!(protect(&sealed, false).unwrap(), b"secret-token");
    }
}
