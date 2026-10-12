//! Library scan: which games are installed, according to the launchers' own manifests.
//!
//! Sources are Steam, Epic, Riot and the Xbox app. Everything is read from the launchers'
//! bookkeeping files, never from a running process. Inside a game's install folder only
//! file and folder NAMES are listed (to learn the exe names and to spot an anti-cheat),
//! no file there is opened. That is rule 4 of the anti-cheat rules in the vault.

use crate::protection::{marker_for_entry, AntiCheatMarker};
use crate::{reg, vdf};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GameSource {
    Steam,
    Epic,
    Riot,
    Xbox,
    /// Added by the user.
    Manual,
    /// Not found by a scan, recognised by its process name from the built in list.
    KnownList,
}

impl GameSource {
    pub fn label(self) -> &'static str {
        match self {
            GameSource::Steam => "Steam",
            GameSource::Epic => "Epic",
            GameSource::Riot => "Riot",
            GameSource::Xbox => "Xbox",
            GameSource::Manual => "Manual",
            GameSource::KnownList => "Known list",
        }
    }
}

/// One installed game as a launcher reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstalledGame {
    /// Stable key, for example `steam:1222140`, `epic:Fortnite`, `riot:valorant.live`.
    /// Profiles are stored under this key.
    pub id: String,
    pub name: String,
    pub install_dir: PathBuf,
    /// Names of the exe files found in the install folder, helpers such as crash
    /// reporters filtered out. The launch exe of the manifest comes first if known.
    pub exe_names: Vec<String>,
    pub source: GameSource,
    /// Anti-cheat folders and files seen while listing the install folder.
    #[serde(default)]
    pub markers: Vec<AntiCheatMarker>,
    /// Riot only: the product settings name Vanguard as a dependency.
    #[serde(default)]
    pub vanguard_declared: bool,
}

#[derive(Debug, Clone, Default)]
pub struct ScanResult {
    pub games: Vec<InstalledGame>,
    /// Files that exist but could not be read or parsed. A launcher that is simply not
    /// installed is not a problem and does not show up here.
    pub problems: Vec<String>,
}

// ---------------------------------------------------------------------------------------
// Paths

/// Lower case, backslashes, no trailing separator. Windows paths compare without case and
/// launchers mix both kinds of slashes. No file system access, so no `canonicalize`.
pub fn normalize_path(path: &Path) -> String {
    let mut s = path.to_string_lossy().replace('/', "\\").to_lowercase();
    while s.ends_with('\\') && s.len() > 3 {
        s.pop();
    }
    s
}

/// True if `child` (normalized) is `parent` or lies below it. Compares whole path
/// components, so `D:\Games\Portal 2` is not below `D:\Games\Portal`.
pub fn is_under(child: &str, parent: &str) -> bool {
    if parent.is_empty() || !child.starts_with(parent) {
        return false;
    }
    child.len() == parent.len() || parent.ends_with('\\') || child.as_bytes()[parent.len()] == b'\\'
}

// ---------------------------------------------------------------------------------------
// Install folder listing

/// How deep below the install folder exe files are looked for. Unreal games keep theirs
/// at `<Game>\Binaries\Win64`, with the anti-cheat folder one level further down.
const MAX_DEPTH: usize = 4;
/// Upper bound for one install so that a huge mod folder cannot stall the scan.
const MAX_DIRS: usize = 3000;

/// Helper programs that live in game folders but are never the game.
fn is_helper_exe(lower: &str) -> bool {
    const PARTS: &[&str] = &[
        "crash", "unins", "setup", "redist", "install", "easyanticheat", "beservice", "webhelper", "errorreport",
        "bugreport", "dxwebsetup", "cefprocess", "subprocess", "blizzardbrowser", "blizzarderror", "updater",
    ];
    PARTS.iter().any(|part| lower.contains(part))
}

/// Lists exe names and anti-cheat markers below `dir`. Directory listings only.
pub fn list_install_dir(dir: &Path) -> (Vec<String>, Vec<AntiCheatMarker>) {
    let mut exes: Vec<String> = Vec::new();
    let mut markers: Vec<AntiCheatMarker> = Vec::new();
    let mut queue = vec![(dir.to_path_buf(), 0usize)];
    let mut visited = 0usize;
    while let Some((current, depth)) = queue.pop() {
        visited += 1;
        if visited > MAX_DIRS {
            break;
        }
        let Ok(entries) = std::fs::read_dir(&current) else { continue };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else { continue };
            // Junctions and symlinks could lead out of the install folder or in circles.
            if kind.is_symlink() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if let Some(anti_cheat) = marker_for_entry(&name, kind.is_dir()) {
                let found = entry.path().strip_prefix(dir).map(|p| p.display().to_string()).unwrap_or(name.clone());
                if !markers.iter().any(|m| m.anti_cheat == anti_cheat) {
                    markers.push(AntiCheatMarker { anti_cheat, found });
                }
                if kind.is_dir() {
                    continue;
                }
            }
            if kind.is_dir() {
                if depth < MAX_DEPTH {
                    queue.push((entry.path(), depth + 1));
                }
            } else if name.to_ascii_lowercase().ends_with(".exe") {
                let lower = name.to_ascii_lowercase();
                // start_protected_game.exe is the Easy Anti-Cheat launcher: it stays in the
                // list because it is the process that owns the window for a moment.
                let keep = !is_helper_exe(&lower) || lower == "start_protected_game.exe";
                if keep && !exes.iter().any(|e| e.eq_ignore_ascii_case(&name)) {
                    exes.push(name);
                }
            }
        }
    }
    exes.sort_by_key(|e| e.to_lowercase());
    (exes, markers)
}

fn finish_game(id: String, name: String, install_dir: PathBuf, source: GameSource, launch_exe: Option<&str>) -> InstalledGame {
    let (mut exe_names, markers) = list_install_dir(&install_dir);
    if let Some(launch) = launch_exe.filter(|l| !l.is_empty()) {
        exe_names.retain(|e| !e.eq_ignore_ascii_case(launch));
        exe_names.insert(0, launch.to_string());
    }
    InstalledGame { id, name, install_dir, exe_names, source, markers, vanguard_declared: false }
}

fn read_text(path: &Path, problems: &mut Vec<String>) -> Option<String> {
    match std::fs::read(path) {
        Ok(bytes) => Some(String::from_utf8_lossy(&bytes).into_owned()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => {
            problems.push(format!("{}: {e}", path.display()));
            None
        }
    }
}

// ---------------------------------------------------------------------------------------
// Steam

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SteamApp {
    pub appid: String,
    pub name: String,
    /// Folder name below `steamapps\common`.
    pub installdir: String,
    /// Undocumented by Valve. 4 is commonly "fully installed". Kept for display only,
    /// the scan decides by whether the folder exists.
    pub state_flags: Option<u32>,
}

/// Reads the library folders out of `libraryfolders.vdf`. Handles the old layout
/// (number with the path as value) and the new one (number with a block holding `path`).
pub fn parse_library_folders(text: &str) -> Result<Vec<PathBuf>, String> {
    let root = vdf::parse(text)?;
    let folders = root.get("libraryfolders").ok_or("no 'libraryfolders' block")?;
    let mut paths = Vec::new();
    for (key, value) in folders.entries() {
        if key.parse::<u32>().is_err() {
            continue;
        }
        let path = match value {
            vdf::Vdf::Str(path) => Some(path.as_str()),
            block => block.str("path"),
        };
        if let Some(path) = path.filter(|p| !p.is_empty()) {
            paths.push(PathBuf::from(path));
        }
    }
    Ok(paths)
}

/// Reads one `appmanifest_<appid>.acf`.
pub fn parse_app_manifest(text: &str) -> Result<SteamApp, String> {
    let root = vdf::parse(text)?;
    let state = root.get("AppState").ok_or("no 'AppState' block")?;
    let field = |key: &str| state.str(key).map(str::to_string).ok_or(format!("no '{key}'"));
    Ok(SteamApp {
        appid: field("appid")?,
        name: field("name")?,
        installdir: field("installdir")?,
        state_flags: state.str("StateFlags").and_then(|s| s.parse().ok()),
    })
}

/// Steam entries that are tools, not games. 228980 is "Steamworks Common Redistributables".
const STEAM_NOT_A_GAME: &[&str] = &["228980"];

/// The Steam folder from the registry, with the default path as a last resort.
pub fn steam_root() -> Option<PathBuf> {
    let candidates = [
        reg::read_string(reg::HKEY_CURRENT_USER, "Software\\Valve\\Steam", "SteamPath"),
        reg::read_string(reg::HKEY_LOCAL_MACHINE, "SOFTWARE\\WOW6432Node\\Valve\\Steam", "InstallPath"),
        std::env::var("ProgramFiles(x86)").ok().map(|p| format!("{p}\\Steam")),
    ];
    candidates.into_iter().flatten().map(PathBuf::from).find(|p| p.join("steamapps").is_dir())
}

pub fn scan_steam(problems: &mut Vec<String>) -> Vec<InstalledGame> {
    let Some(root) = steam_root() else { return Vec::new() };
    let mut libraries = vec![root.clone()];
    let index = root.join("steamapps").join("libraryfolders.vdf");
    if let Some(text) = read_text(&index, problems) {
        match parse_library_folders(&text) {
            Ok(paths) => libraries.extend(paths),
            Err(e) => problems.push(format!("{}: {e}", index.display())),
        }
    }
    let mut seen: Vec<String> = Vec::new();
    libraries.retain(|lib| {
        let key = normalize_path(lib);
        let fresh = !seen.contains(&key);
        seen.push(key);
        fresh
    });

    let mut games = Vec::new();
    for library in libraries {
        let steamapps = library.join("steamapps");
        let Ok(entries) = std::fs::read_dir(&steamapps) else { continue };
        for entry in entries.flatten() {
            let file = entry.file_name().to_string_lossy().to_lowercase();
            if !file.starts_with("appmanifest_") || !file.ends_with(".acf") {
                continue;
            }
            let Some(text) = read_text(&entry.path(), problems) else { continue };
            let app = match parse_app_manifest(&text) {
                Ok(app) => app,
                Err(e) => {
                    problems.push(format!("{}: {e}", entry.path().display()));
                    continue;
                }
            };
            if STEAM_NOT_A_GAME.contains(&app.appid.as_str()) {
                continue;
            }
            let install_dir = steamapps.join("common").join(&app.installdir);
            // A manifest can outlive its folder (interrupted uninstall, unplugged drive).
            if !install_dir.is_dir() {
                continue;
            }
            games.push(finish_game(format!("steam:{}", app.appid), app.name, install_dir, GameSource::Steam, None));
        }
    }
    games
}

// ---------------------------------------------------------------------------------------
// Epic

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EpicApp {
    pub app_name: String,
    pub display_name: String,
    pub install_location: PathBuf,
    /// Relative to `install_location`, may be empty.
    pub launch_executable: String,
    pub is_game: bool,
    pub incomplete: bool,
    /// True for add-ons that belong to another entry.
    pub is_dlc: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct EpicItem {
    #[serde(default)]
    app_name: String,
    #[serde(default)]
    display_name: String,
    install_location: String,
    #[serde(default)]
    launch_executable: String,
    #[serde(default)]
    app_categories: Vec<String>,
    #[serde(default)]
    main_game_app_name: String,
    #[serde(default, rename = "bIsIncompleteInstall")]
    incomplete: bool,
}

/// Reads one Epic `*.item` manifest (JSON). There is no official schema, so every field
/// except the install location is optional.
pub fn parse_epic_manifest(text: &str) -> Result<EpicApp, String> {
    let item: EpicItem = serde_json::from_str(text).map_err(|e| e.to_string())?;
    if item.install_location.is_empty() {
        return Err("empty InstallLocation".into());
    }
    let display_name = if item.display_name.is_empty() { item.app_name.clone() } else { item.display_name };
    Ok(EpicApp {
        // Unreal Engine itself is listed with the category "engines". No categories at
        // all is an old manifest, those were games.
        is_game: item.app_categories.is_empty() || item.app_categories.iter().any(|c| c == "games"),
        is_dlc: !item.main_game_app_name.is_empty() && item.main_game_app_name != item.app_name,
        app_name: item.app_name,
        display_name,
        install_location: PathBuf::from(item.install_location),
        launch_executable: item.launch_executable,
        incomplete: item.incomplete,
    })
}

fn program_data() -> PathBuf {
    std::env::var_os("ProgramData").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("C:\\ProgramData"))
}

pub fn scan_epic(problems: &mut Vec<String>) -> Vec<InstalledGame> {
    let dir = program_data().join("Epic").join("EpicGamesLauncher").join("Data").join("Manifests");
    let Ok(entries) = std::fs::read_dir(&dir) else { return Vec::new() };
    let mut games = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.extension().is_some_and(|e| e.eq_ignore_ascii_case("item")) {
            continue;
        }
        let Some(text) = read_text(&path, problems) else { continue };
        let app = match parse_epic_manifest(&text) {
            Ok(app) => app,
            Err(e) => {
                problems.push(format!("{}: {e}", path.display()));
                continue;
            }
        };
        if !app.is_game || app.is_dlc || app.incomplete || !app.install_location.is_dir() {
            continue;
        }
        let launch = Path::new(&app.launch_executable).file_name().map(|n| n.to_string_lossy().into_owned());
        let id = format!("epic:{}", app.app_name);
        games.push(finish_game(id, app.display_name, app.install_location, GameSource::Epic, launch.as_deref()));
    }
    games
}

// ---------------------------------------------------------------------------------------
// Riot

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RiotProductSettings {
    pub install_full_path: Option<PathBuf>,
    pub install_root: Option<PathBuf>,
    /// `dependencies: vanguard: true`
    pub vanguard: bool,
}

/// Reads the few keys Klip needs out of a `*.product_settings.yaml`. The file is flat
/// enough that a line scan does it, a YAML crate would be a dependency for three keys.
pub fn parse_riot_product_settings(text: &str) -> RiotProductSettings {
    let mut settings = RiotProductSettings::default();
    let mut section = String::new();
    for line in text.lines() {
        let Some((key, value)) = line.split_once(':') else { continue };
        let indented = key.starts_with([' ', '\t']);
        let key = key.trim();
        let value = value.trim().trim_matches('"');
        if !indented {
            section = key.to_string();
            match key {
                "product_install_full_path" if !value.is_empty() => settings.install_full_path = Some(value.into()),
                "product_install_root" if !value.is_empty() => settings.install_root = Some(value.into()),
                _ => {}
            }
        } else if section == "dependencies" && key == "vanguard" && value == "true" {
            settings.vanguard = true;
        }
    }
    settings
}

/// Install folders named in `RiotClientInstalls.json` (`associated_client`).
pub fn parse_riot_installs(text: &str) -> Result<Vec<PathBuf>, String> {
    let value: serde_json::Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let Some(clients) = value.get("associated_client").and_then(|v| v.as_object()) else {
        return Ok(Vec::new());
    };
    Ok(clients.keys().map(PathBuf::from).collect())
}

/// Display names for Riot's internal product ids. Unknown ids are shown as they are.
fn riot_product_name(product: &str) -> String {
    match product {
        "valorant" => "VALORANT",
        "league_of_legends" => "League of Legends",
        "bacon" => "Legends of Runeterra",
        "lion" => "2XKO",
        other => return other.replace('_', " "),
    }
    .to_string()
}

pub fn scan_riot(problems: &mut Vec<String>) -> Vec<InstalledGame> {
    let base = program_data().join("Riot Games");
    let mut games: Vec<InstalledGame> = Vec::new();

    // Only `*.product_settings.yaml` is opened here. The lockfiles in the same folders
    // hold the port and password of the local Riot client API, which Klip must never use
    // (red in the anti-cheat rules), so they are not even read.
    if let Ok(entries) = std::fs::read_dir(base.join("Metadata")) {
        for entry in entries.flatten() {
            let folder = entry.file_name().to_string_lossy().into_owned();
            let yaml = entry.path().join(format!("{folder}.product_settings.yaml"));
            let Some(text) = read_text(&yaml, problems) else { continue };
            let settings = parse_riot_product_settings(&text);
            let Some(install_dir) = settings.install_full_path.filter(|p| p.is_dir()) else { continue };
            // Riot writes forward slashes, the rest of Klip shows and compares backslashes.
            let install_dir = PathBuf::from(install_dir.to_string_lossy().replace('/', "\\"));
            let (product, patchline) = folder.split_once('.').unwrap_or((folder.as_str(), "live"));
            let mut name = riot_product_name(product);
            if patchline != "live" {
                name.push_str(&format!(" ({})", patchline.to_uppercase()));
            }
            let mut game = finish_game(format!("riot:{folder}"), name, install_dir, GameSource::Riot, None);
            game.vanguard_declared = settings.vanguard;
            games.push(game);
        }
    }

    let installs = base.join("RiotClientInstalls.json");
    if let Some(text) = read_text(&installs, problems) {
        match parse_riot_installs(&text) {
            Ok(paths) => {
                for path in paths {
                    let key = normalize_path(&path);
                    if !path.is_dir() || games.iter().any(|g| normalize_path(&g.install_dir) == key) {
                        continue;
                    }
                    // `...\VALORANT\live\`: the folder above the patch line is the product.
                    let shown = path.to_string_lossy().replace('/', "\\");
                    let install_dir = PathBuf::from(shown.trim_end_matches('\\'));
                    let name = install_dir
                        .parent()
                        .and_then(|p| p.file_name())
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| key.clone());
                    games.push(finish_game(format!("riot:{key}"), name, install_dir, GameSource::Riot, None));
                }
            }
            Err(e) => problems.push(format!("{}: {e}", installs.display())),
        }
    }
    games
}

// ---------------------------------------------------------------------------------------
// Xbox app

/// Reads a `.GamingRoot` file from the root of a drive: the magic `RGBX`, a version and
/// the folder for Xbox app games on that drive as UTF-16, relative to the drive root.
pub fn parse_gaming_root(bytes: &[u8]) -> Option<String> {
    let rest = bytes.strip_prefix(b"RGBX")?.get(4..)?;
    let units: Vec<u16> =
        rest.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).take_while(|&u| u != 0).collect();
    let folder = String::from_utf16(&units).ok()?;
    let folder = folder.trim_matches(['\\', '/']).to_string();
    // The folder is meant to be a plain name on the same drive. Anything that tries to
    // climb out of it is not a file the Xbox app wrote.
    (!folder.is_empty() && !folder.contains("..") && !folder.contains(':')).then_some(folder)
}

fn fixed_drives() -> Vec<PathBuf> {
    use windows::core::HSTRING;
    use windows::Win32::Storage::FileSystem::{GetDriveTypeW, GetLogicalDrives};
    // DRIVE_FIXED. Network and removable drives are skipped: probing them can hang for
    // seconds, and a recorder must never wake a sleeping network share.
    const DRIVE_FIXED: u32 = 3;
    let mask = unsafe { GetLogicalDrives() };
    (0..26u32)
        .filter(|bit| mask & (1 << bit) != 0)
        .map(|bit| format!("{}:\\", (b'A' + bit as u8) as char))
        .filter(|root| unsafe { GetDriveTypeW(&HSTRING::from(root.as_str())) } == DRIVE_FIXED)
        .map(PathBuf::from)
        .collect()
}

/// Games installed through the Xbox app into a folder the user can see (`XboxGames`).
/// Each game sits in `<folder>\<Name>\Content`. Classic Store packages under
/// `WindowsApps` are not covered: that folder is locked down and reading the package
/// list needs WinRT calls this crate avoids. Such games are still found at runtime.
pub fn scan_xbox(_problems: &mut Vec<String>) -> Vec<InstalledGame> {
    let mut games = Vec::new();
    for drive in fixed_drives() {
        let Ok(bytes) = std::fs::read(drive.join(".GamingRoot")) else { continue };
        let Some(folder) = parse_gaming_root(&bytes) else { continue };
        let Ok(entries) = std::fs::read_dir(drive.join(&folder)) else { continue };
        for entry in entries.flatten() {
            let content = entry.path().join("Content");
            if !content.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            games.push(finish_game(format!("xbox:{}", name.to_lowercase()), name, content, GameSource::Xbox, None));
        }
    }
    games
}

/// Runs every scan. Takes a moment (directory listings of every install folder), so call
/// it at start and on request, not in a loop.
pub fn scan_all() -> ScanResult {
    let mut result = ScanResult::default();
    result.games.extend(scan_steam(&mut result.problems));
    result.games.extend(scan_epic(&mut result.problems));
    result.games.extend(scan_riot(&mut result.problems));
    result.games.extend(scan_xbox(&mut result.problems));
    result.games.sort_by_key(|g| g.name.to_lowercase());
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_matching_respects_components() {
        let parent = normalize_path(Path::new("D:/Games/SteamLibrary/steamapps/common/Portal/"));
        assert_eq!(parent, "d:\\games\\steamlibrary\\steamapps\\common\\portal");
        let inside = normalize_path(Path::new("D:\\Games\\SteamLibrary\\steamapps\\common\\Portal\\hl2.exe"));
        let sibling = normalize_path(Path::new("D:\\Games\\SteamLibrary\\steamapps\\common\\Portal 2\\portal2.exe"));
        assert!(is_under(&inside, &parent));
        assert!(is_under(&parent, &parent));
        assert!(!is_under(&sibling, &parent));
        assert!(!is_under(&inside, ""));
    }

    #[test]
    fn helper_exes_are_filtered() {
        assert!(is_helper_exe("unitycrashhandler64.exe"));
        assert!(is_helper_exe("crashreportclient.exe"));
        assert!(is_helper_exe("easyanticheat_eos_setup.exe"));
        assert!(!is_helper_exe("peak.exe"));
        assert!(!is_helper_exe("valorant-win64-shipping.exe"));
    }

    #[test]
    fn gaming_root() {
        let mut bytes = b"RGBX\x01\x00\x00\x00".to_vec();
        for unit in "XboxGames".encode_utf16().chain([0]) {
            bytes.extend(unit.to_le_bytes());
        }
        assert_eq!(parse_gaming_root(&bytes).as_deref(), Some("XboxGames"));
        assert_eq!(parse_gaming_root(b"nope"), None);
        assert_eq!(parse_gaming_root(b"RGBX\x01\x00\x00\x00"), None);
        let mut evil = b"RGBX\x01\x00\x00\x00".to_vec();
        for unit in "..\\Windows".encode_utf16().chain([0]) {
            evil.extend(unit.to_le_bytes());
        }
        assert_eq!(parse_gaming_root(&evil), None);
    }
}
