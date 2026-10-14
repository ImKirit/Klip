//! The manual list (programs the user added or excluded) and the catalog that answers
//! "is this exe a game, and which one?" from the scan, the manual list and the built in
//! lists. Pure data and string comparison, no process is touched here.

use crate::library::{is_under, normalize_path, GameSource, InstalledGame};
use crate::protection::{known_protected, Protection, ProtectionDb};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// A program the user declared to be a game.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManualGame {
    pub name: String,
    /// File name of the exe, for example `Minecraft.exe`.
    pub exe_name: String,
    /// Full path if the user picked a file. With a path only that exact file matches,
    /// without one every exe of that name does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exe_path: Option<PathBuf>,
}

impl ManualGame {
    pub fn id(&self) -> String {
        format!("manual:{}", self.exe_name.to_lowercase())
    }
}

/// What the user added and excluded by hand. Part of the settings, written by the app.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ManualList {
    pub added: Vec<ManualGame>,
    /// Exe names (`chrome.exe`) or full paths. Excluded programs are never a game, not
    /// even if a launcher lists them. Also holds the answer "no" to "is this a game?".
    pub excluded: Vec<String>,
}

impl ManualList {
    /// Adds a game, replacing an earlier entry for the same exe, and lifts an exclusion
    /// of that exe: the newest decision of the user wins.
    pub fn add(&mut self, game: ManualGame) {
        self.include(&game.exe_name);
        if let Some(path) = &game.exe_path {
            self.include(&path.to_string_lossy());
        }
        self.added.retain(|g| !g.exe_name.eq_ignore_ascii_case(&game.exe_name));
        self.added.push(game);
    }

    pub fn remove(&mut self, exe_name: &str) {
        self.added.retain(|g| !g.exe_name.eq_ignore_ascii_case(exe_name));
    }

    /// Excludes an exe name or a full path.
    pub fn exclude(&mut self, entry: &str) {
        if !self.excluded.iter().any(|e| same_entry(e, entry)) {
            self.excluded.push(entry.to_string());
        }
    }

    /// Lifts an exclusion.
    pub fn include(&mut self, entry: &str) {
        self.excluded.retain(|e| !same_entry(e, entry));
    }

    pub fn is_excluded(&self, exe_path: Option<&Path>, exe_name: &str) -> bool {
        let path = exe_path.map(normalize_path);
        self.excluded.iter().any(|entry| {
            if entry.contains(['\\', '/']) {
                path.as_deref() == Some(normalize_path(Path::new(entry)).as_str())
            } else {
                entry.eq_ignore_ascii_case(exe_name)
            }
        })
    }

    pub fn find_added(&self, exe_path: Option<&Path>, exe_name: &str) -> Option<&ManualGame> {
        let path = exe_path.map(normalize_path);
        self.added.iter().find(|game| match &game.exe_path {
            Some(expected) => path.as_deref() == Some(normalize_path(expected).as_str()),
            None => game.exe_name.eq_ignore_ascii_case(exe_name),
        })
    }
}

fn same_entry(a: &str, b: &str) -> bool {
    normalize_path(Path::new(a)) == normalize_path(Path::new(b))
}

/// Programs that are never a game: browsers, players, launchers, remote desktop, the shell.
/// Without this list the fullscreen heuristic would report a film or a video call, and
/// recording that by accident is exactly what Klip must not do.
const NOT_A_GAME: &[&str] = &[
    "explorer.exe", "dwm.exe", "searchhost.exe", "startmenuexperiencehost.exe", "shellexperiencehost.exe",
    "lockapp.exe", "applicationframehost.exe", "textinputhost.exe", "taskmgr.exe", "systemsettings.exe",
    "chrome.exe", "msedge.exe", "firefox.exe", "opera.exe", "opera_gx.exe", "brave.exe", "vivaldi.exe", "arc.exe",
    "vlc.exe", "mpv.exe", "mpc-hc64.exe", "mpc-be64.exe", "wmplayer.exe", "video.ui.exe", "microsoft.media.player.exe",
    "potplayermini64.exe", "spotify.exe",
    "steam.exe", "steamwebhelper.exe", "epicgameslauncher.exe", "riotclientservices.exe", "riot client.exe",
    "riotclientux.exe", "battle.net.exe", "eadesktop.exe", "upc.exe", "galaxyclient.exe", "xboxpcapp.exe",
    "mstsc.exe", "teamviewer.exe", "anydesk.exe", "parsecd.exe", "moonlight.exe", "vmconnect.exe",
    "discord.exe", "zoom.exe", "ms-teams.exe", "teams.exe", "slack.exe",
    "obs64.exe", "powerpnt.exe", "code.exe", "devenv.exe", "windowsterminal.exe", "claude.exe",
    // Desktop toys and wallpapers from the game stores: they run all day and would put
    // their name on every clip.
    "bongocat.exe", "bongo cat.exe", "wallpaper32.exe", "wallpaper64.exe", "desktopmate.exe",
    "klip.exe", "klip-recorder.exe",
];

pub fn is_builtin_non_game(exe_name: &str) -> bool {
    NOT_A_GAME.iter().any(|n| n.eq_ignore_ascii_case(exe_name))
}

/// A program recognised as a game.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameMatch {
    /// Key for profiles, see `InstalledGame::id`.
    pub id: String,
    pub name: String,
    pub source: GameSource,
    pub protection: Protection,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Identity {
    Game(GameMatch),
    /// Excluded by the user or on the built in list of programs that are never games.
    Excluded,
    /// Not known. The protection is still worked out (always locked for unknown programs).
    Unknown(Protection),
}

/// Everything needed to recognise a running program. Cheap to clone and to send to the
/// watcher thread.
#[derive(Debug, Clone)]
pub struct GameCatalog {
    games: Vec<InstalledGame>,
    /// Normalized install folders, same order as `games`.
    dirs: Vec<String>,
    manual: ManualList,
    protection: ProtectionDb,
}

impl GameCatalog {
    pub fn new(games: Vec<InstalledGame>, manual: ManualList, protection: ProtectionDb) -> Self {
        let dirs = games.iter().map(|g| normalize_path(&g.install_dir)).collect();
        Self { games, dirs, manual, protection }
    }

    /// Scans the libraries and probes the machine. Returns the catalog and the problems
    /// of the scan.
    pub fn scan(manual: ManualList) -> (Self, Vec<String>) {
        let result = crate::library::scan_all();
        (Self::new(result.games, manual, ProtectionDb::probe()), result.problems)
    }

    pub fn games(&self) -> &[InstalledGame] {
        &self.games
    }

    pub fn manual(&self) -> &ManualList {
        &self.manual
    }

    pub fn protection_db(&self) -> &ProtectionDb {
        &self.protection
    }

    /// Protection of an installed game, without it having to run.
    pub fn protection_of(&self, game: &InstalledGame) -> Protection {
        self.protection.classify_installed(game)
    }

    /// The installed game whose folder contains this path. With nested folders the
    /// innermost one wins.
    pub fn game_for_path(&self, exe_path: &Path) -> Option<&InstalledGame> {
        let path = normalize_path(exe_path);
        (0..self.games.len())
            .filter(|&i| is_under(&path, &self.dirs[i]))
            .max_by_key(|&i| self.dirs[i].len())
            .map(|i| &self.games[i])
    }

    /// True if a process of this name could be a game Klip knows: an exe of an installed
    /// game, a manual entry or a name from the built in list. Lets callers that walk the
    /// process list ask for the path of likely games only instead of opening a handle
    /// to every process on the machine.
    pub fn knows_exe_name(&self, exe_name: &str) -> bool {
        self.games.iter().any(|g| g.exe_names.iter().any(|e| e.eq_ignore_ascii_case(exe_name)))
            || self.manual.added.iter().any(|g| g.exe_name.eq_ignore_ascii_case(exe_name))
            || known_protected(exe_name).is_some()
    }

    /// Decides what a program is. Order: the user's word first, then the launchers'
    /// manifests, then the built in lists.
    pub fn identify(&self, exe_path: Option<&Path>, exe_name: &str) -> Identity {
        if self.manual.is_excluded(exe_path, exe_name) {
            return Identity::Excluded;
        }
        let installed = exe_path.and_then(|p| self.game_for_path(p));
        if let Some(added) = self.manual.find_added(exe_path, exe_name) {
            return Identity::Game(GameMatch {
                id: added.id(),
                name: added.name.clone(),
                source: GameSource::Manual,
                protection: self.protection.classify_running(exe_path, exe_name, installed),
            });
        }
        if let Some(game) = installed {
            return Identity::Game(GameMatch {
                id: game.id.clone(),
                name: game.name.clone(),
                source: game.source,
                protection: self.protection.classify_running(exe_path, exe_name, Some(game)),
            });
        }
        if is_builtin_non_game(exe_name) {
            return Identity::Excluded;
        }
        let protection = self.protection.classify_running(exe_path, exe_name, None);
        if let Some(known) = known_protected(exe_name) {
            return Identity::Game(GameMatch {
                id: format!("known:{}", known.game.to_lowercase()),
                name: known.game.to_string(),
                source: GameSource::KnownList,
                protection,
            });
        }
        Identity::Unknown(protection)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn game(id: &str, name: &str, dir: &str, exes: &[&str]) -> InstalledGame {
        InstalledGame {
            id: id.into(),
            name: name.into(),
            install_dir: dir.into(),
            exe_names: exes.iter().map(|e| e.to_string()).collect(),
            source: GameSource::Steam,
            markers: Vec::new(),
            vanguard_declared: false,
        }
    }

    fn catalog(manual: ManualList) -> GameCatalog {
        let games = vec![
            game("steam:400", "Portal", "D:\\Lib\\steamapps\\common\\Portal", &["hl2.exe"]),
            game("steam:620", "Portal 2", "D:\\Lib\\steamapps\\common\\Portal 2", &["portal2.exe"]),
        ];
        GameCatalog::new(games, manual, ProtectionDb::fixed(false, false))
    }

    #[test]
    fn install_folder_decides() {
        let c = catalog(ManualList::default());
        let hit = c.identify(Some(Path::new("d:/lib/steamapps/common/portal 2/portal2.exe")), "portal2.exe");
        let Identity::Game(m) = hit else { panic!("expected a game") };
        assert_eq!(m.id, "steam:620");
        assert!(!m.protection.hook_allowed());
        assert!(matches!(c.identify(Some(Path::new("C:\\Tools\\thing.exe")), "thing.exe"), Identity::Unknown(_)));
    }

    #[test]
    fn user_decisions_win() {
        let mut manual = ManualList::default();
        manual.exclude("hl2.exe");
        manual.add(ManualGame { name: "My Tool".into(), exe_name: "thing.exe".into(), exe_path: None });
        let c = catalog(manual);
        assert_eq!(c.identify(Some(Path::new("D:\\Lib\\steamapps\\common\\Portal\\hl2.exe")), "hl2.exe"), Identity::Excluded);
        let Identity::Game(m) = c.identify(Some(Path::new("C:\\Tools\\thing.exe")), "THING.EXE") else { panic!() };
        assert_eq!((m.id.as_str(), m.source), ("manual:thing.exe", GameSource::Manual));
        // A manually added program is still unknown to the protection list, so locked.
        assert!(!m.protection.hook_allowed());
    }

    #[test]
    fn adding_lifts_the_exclusion() {
        let mut manual = ManualList::default();
        manual.exclude("C:/Tools/thing.exe");
        assert!(manual.is_excluded(Some(Path::new("c:\\tools\\THING.exe")), "thing.exe"));
        assert!(!manual.is_excluded(None, "thing.exe"));
        manual.add(ManualGame { name: "T".into(), exe_name: "thing.exe".into(), exe_path: Some("C:\\Tools\\thing.exe".into()) });
        assert!(manual.excluded.is_empty());
        assert!(manual.find_added(Some(Path::new("C:\\Other\\thing.exe")), "thing.exe").is_none());
        assert!(manual.find_added(Some(Path::new("C:\\Tools\\thing.exe")), "thing.exe").is_some());
    }

    #[test]
    fn browsers_are_never_games_but_listed_games_are() {
        let c = catalog(ManualList::default());
        assert_eq!(c.identify(Some(Path::new("C:\\Program Files\\Google\\Chrome\\chrome.exe")), "chrome.exe"), Identity::Excluded);
        // Path refused by Windows: the name alone still finds a game from the built in list.
        let Identity::Game(m) = c.identify(None, "VALORANT-Win64-Shipping.exe") else { panic!() };
        assert_eq!(m.source, GameSource::KnownList);
        assert!(matches!(m.protection, Protection::Protected(_)));
    }

    #[test]
    fn manual_list_round_trips_through_json() {
        let mut manual = ManualList::default();
        manual.add(ManualGame { name: "A".into(), exe_name: "a.exe".into(), exe_path: None });
        manual.exclude("b.exe");
        let json = serde_json::to_string(&manual).unwrap();
        assert_eq!(serde_json::from_str::<ManualList>(&json).unwrap(), manual);
        assert_eq!(serde_json::from_str::<ManualList>("{}").unwrap(), ManualList::default());
    }
}
