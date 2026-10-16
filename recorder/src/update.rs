//! Automatic updates from GitHub Releases.
//!
//! A while after the start and then every six hours the recorder asks GitHub for the
//! newest release of ImKirit/Klip. Is it newer than this build, `Klip-Setup.exe` is
//! downloaded and checked against the SHA256 line of the release text. Installed is only
//! at a calm moment: no game running, no recording, the app closed. The setup then runs
//! silently into the folder Klip lives in, stops this recorder through its pipe and
//! starts the new one.
//!
//! Only an installed copy updates itself (an `uninstall.exe` lies next to it). A build
//! started from the source folder never replaces itself.

use crate::Res;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

const LATEST: &str = "https://api.github.com/repos/ImKirit/Klip/releases/latest";
const ASSET: &str = "Klip-Setup.exe";
/// A setup larger than this is not Klip.
const MAX_BYTES: u64 = 300 * 1024 * 1024;

/// An update that is downloaded and checked, ready to install.
#[derive(Debug, Clone)]
pub struct Ready {
    pub version: String,
    pub setup: PathBuf,
}

/// "v0.2.0" or "0.2.0-beta.1" as numbers. The part after a dash is ignored.
pub fn parse_version(text: &str) -> Option<(u64, u64, u64)> {
    let core = text.trim().trim_start_matches(['v', 'V']).split(['-', '+']).next()?;
    let mut parts = core.split('.').map(|p| p.parse::<u64>().ok());
    let v = (parts.next()??, parts.next().flatten().unwrap_or(0), parts.next().flatten().unwrap_or(0));
    Some(v)
}

pub fn is_newer(remote: &str, local: &str) -> bool {
    matches!((parse_version(remote), parse_version(local)), (Some(r), Some(l)) if r > l)
}

/// The SHA256 the release text names: a line with "SHA256" and 64 hex digits.
pub fn checksum_in(body: &str) -> Option<String> {
    body.lines().filter(|l| l.to_ascii_uppercase().contains("SHA256")).find_map(|l| {
        l.split(|c: char| !c.is_ascii_hexdigit()).find(|w| w.len() == 64).map(|w| w.to_ascii_uppercase())
    })
}

/// The folder this recorder was installed into, if it is an installation.
pub fn install_dir() -> Option<PathBuf> {
    let dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    dir.join("uninstall.exe").exists().then_some(dir)
}

/// Asks GitHub and downloads a newer setup into `cache`. `Ok(None)` means up to date.
pub fn check(cache: &Path) -> Res<Option<Ready>> {
    let url = std::env::var("KLIP_UPDATE_URL").unwrap_or_else(|_| LATEST.to_string());
    let agent = klip_cloud::http();
    let release: serde_json::Value = agent
        .get(&url)
        .set("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| format!("the release list cannot be read: {e}"))?
        .into_json()
        .map_err(|e| e.to_string())?;
    let tag = release.get("tag_name").and_then(|t| t.as_str()).ok_or("the release has no tag")?;
    if !is_newer(tag, env!("CARGO_PKG_VERSION")) {
        return Ok(None);
    }
    let version = tag.trim_start_matches(['v', 'V']).to_string();
    let body = release.get("body").and_then(|b| b.as_str()).unwrap_or("");
    let want = checksum_in(body).ok_or("the release names no SHA256, not installing it")?;
    let asset = release
        .get("assets")
        .and_then(|a| a.as_array())
        .and_then(|list| list.iter().find(|a| a.get("name").and_then(|n| n.as_str()) == Some(ASSET)))
        .ok_or("the release carries no Klip-Setup.exe")?;
    let link = asset.get("browser_download_url").and_then(|u| u.as_str()).ok_or("the setup has no address")?;

    std::fs::create_dir_all(cache).map_err(|e| e.to_string())?;
    let setup = cache.join(format!("Klip-Setup-{version}.exe"));
    // Downloaded before and still fine: done.
    if std::fs::read(&setup).is_ok_and(|b| sha256_hex(&b) == want) {
        return Ok(Some(Ready { version, setup }));
    }
    let res = agent.get(link).timeout(Duration::from_secs(600)).call().map_err(|e| format!("the setup cannot be downloaded: {e}"))?;
    let mut bytes = Vec::new();
    res.into_reader().take(MAX_BYTES + 1).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err("the setup is far too large, not installing it".into());
    }
    let got = sha256_hex(&bytes);
    if got != want {
        return Err(format!("the setup does not match its SHA256 ({got} instead of {want}), not installing it").into());
    }
    std::fs::write(&setup, &bytes).map_err(|e| e.to_string())?;
    // Older downloads are of no use any more.
    if let Ok(entries) = std::fs::read_dir(cache) {
        for e in entries.flatten() {
            if e.path() != setup {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    Ok(Some(Ready { version, setup }))
}

/// Starts the setup silently into `dir`. It stops this recorder and starts the new one.
/// NSIS wants the folder as the last argument, as `/D=` and without quotes.
pub fn install(ready: &Ready, dir: &Path) -> Res<()> {
    use std::os::windows::process::CommandExt;
    std::process::Command::new(&ready.setup)
        .raw_arg(format!("/S /D={}", dir.display()))
        .current_dir(std::env::temp_dir())
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("the setup does not start: {e}").into())
}

/// True while the app (`Klip.exe`) runs: its files cannot be replaced then.
pub fn app_running() -> bool {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
    };
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return false };
        let mut entry = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut found = false;
        let mut ok = Process32FirstW(snap, &mut entry).is_ok();
        while ok {
            let len = entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(entry.szExeFile.len());
            if String::from_utf16_lossy(&entry.szExeFile[..len]).eq_ignore_ascii_case("Klip.exe") {
                found = true;
                break;
            }
            ok = Process32NextW(snap, &mut entry).is_ok();
        }
        let _ = CloseHandle(snap);
        found
    }
}

/// SHA256 of `data` in capitals, through the hash provider of Windows.
pub fn sha256_hex(data: &[u8]) -> String {
    use windows::Win32::Security::Cryptography::{BCryptHash, BCRYPT_SHA256_ALG_HANDLE};
    let mut out = [0u8; 32];
    let status = unsafe { BCryptHash(BCRYPT_SHA256_ALG_HANDLE, None, data, &mut out) };
    if status.is_err() {
        return String::new();
    }
    out.iter().map(|b| format!("{b:02X}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert_eq!(parse_version("v0.1.0"), Some((0, 1, 0)));
        assert_eq!(parse_version("1.0.0-beta.1"), Some((1, 0, 0)));
        assert_eq!(parse_version("v2"), Some((2, 0, 0)));
        assert!(is_newer("v0.2.0", "0.1.0"));
        assert!(is_newer("v0.1.1", "0.1.0"));
        assert!(!is_newer("v0.1.0", "0.1.0"));
        assert!(!is_newer("v0.0.9", "0.1.0"));
        assert!(!is_newer("nightly", "0.1.0"));
    }

    #[test]
    fn checksum_from_release_text() {
        let body = "Text\n\n**Installing:** run it.\n\nSHA256 `AFCA88582EA1E021C86140A0BEBB72AD0A053CBEF59E314E59BA9676127662C6`";
        assert_eq!(checksum_in(body).as_deref(), Some("AFCA88582EA1E021C86140A0BEBB72AD0A053CBEF59E314E59BA9676127662C6"));
        assert_eq!(checksum_in("no hash here"), None);
    }

    #[test]
    fn hashing_matches_the_known_value() {
        assert_eq!(sha256_hex(b"abc"), "BA7816BF8F01CFEA414140DE5DAE2223B00361A396177A9CB410FF61F20015AD");
    }
}
