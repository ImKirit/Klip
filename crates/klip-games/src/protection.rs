//! The protection list: does a game ship a kernel anti-cheat?
//!
//! Everything here works from the outside. Evidence comes from three places only:
//! file and folder NAMES in the install folder, the registry (is a service installed)
//! and a built in list of process names. No process is opened, no file content inside a
//! game folder is read. The rule from the vault applies throughout: unknown counts as
//! protected, so a missing entry can never unlock anything.

use crate::library::{GameSource, InstalledGame};
use crate::reg;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AntiCheat {
    Vanguard,
    EasyAntiCheat,
    BattlEye,
    Faceit,
    Ricochet,
    EaJavelin,
    GameGuard,
    Xigncode,
    AntiCheatExpert,
    NeacProtect,
    MihoyoProtect,
    Denuvo,
    NexonGameSecurity,
    /// Roblox. User mode, but it fights injection hard, so it is treated the same.
    Hyperion,
    /// Blizzard's own system. The kernel part is not documented, so it is protected.
    Blizzard,
    /// Valve Anti-Cheat with Trusted Mode. No kernel driver, but injection is blocked.
    VacTrustedMode,
}

impl AntiCheat {
    pub fn label(self) -> &'static str {
        match self {
            AntiCheat::Vanguard => "Riot Vanguard",
            AntiCheat::EasyAntiCheat => "Easy Anti-Cheat",
            AntiCheat::BattlEye => "BattlEye",
            AntiCheat::Faceit => "FACEIT AC",
            AntiCheat::Ricochet => "Ricochet",
            AntiCheat::EaJavelin => "EA Javelin",
            AntiCheat::GameGuard => "nProtect GameGuard",
            AntiCheat::Xigncode => "XIGNCODE3",
            AntiCheat::AntiCheatExpert => "Anti-Cheat Expert",
            AntiCheat::NeacProtect => "NEAC Protect",
            AntiCheat::MihoyoProtect => "miHoYo Protect",
            AntiCheat::Denuvo => "Denuvo Anti-Cheat",
            AntiCheat::NexonGameSecurity => "Nexon Game Security",
            AntiCheat::Hyperion => "Hyperion",
            AntiCheat::Blizzard => "Blizzard",
            AntiCheat::VacTrustedMode => "VAC Trusted Mode",
        }
    }
}

/// Why a game counts as protected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Evidence {
    /// The process name is on the built in list. `verified_locally` says whether the name
    /// was checked against a real installation on the development machine; otherwise it
    /// comes from the raw list in the vault (Recherche Anti-Cheat, section 7).
    BuiltInList { verified_locally: bool },
    /// A file or folder name inside the install folder, for example `EasyAntiCheat`.
    InstallMarker { found: String },
    /// A Riot title on a machine where the Vanguard services are installed.
    RiotTitleWithVanguard,
    /// Riot's own product settings name Vanguard as a dependency of this title.
    RiotMetadata,
    /// FACEIT AC is installed and the game is one it guards.
    FaceitInstalled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtectionReason {
    pub anti_cheat: AntiCheat,
    pub evidence: Evidence,
}

impl fmt::Display for ProtectionReason {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.anti_cheat.label())?;
        match &self.evidence {
            Evidence::BuiltInList { verified_locally: true } => write!(f, " (built in list, name verified)"),
            Evidence::BuiltInList { verified_locally: false } => write!(f, " (built in list, name from raw list)"),
            Evidence::InstallMarker { found } => write!(f, " (found '{found}')"),
            Evidence::RiotTitleWithVanguard => write!(f, " (Riot title, Vanguard services installed)"),
            Evidence::RiotMetadata => write!(f, " (Riot product settings list Vanguard)"),
            Evidence::FaceitInstalled => write!(f, " (FACEIT AC installed)"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", content = "reason", rename_all = "snake_case")]
pub enum Protection {
    /// Evidence of an anti-cheat was found. Stage 2 of the overlay (hook) stays locked.
    Protected(ProtectionReason),
    /// The game is on the explicit allow list AND no evidence of an anti-cheat was found.
    KnownUnprotected,
    /// Nothing is known. Treated exactly like `Protected`.
    Unknown,
}

impl Protection {
    /// The only question the rest of Klip may ask before loading anything into a game.
    /// True for explicitly released games only. `Unknown` is deliberately false: a game
    /// Klip has never heard of may well ship an anti-cheat.
    pub fn hook_allowed(&self) -> bool {
        matches!(self, Protection::KnownUnprotected)
    }

    pub fn is_protected(&self) -> bool {
        !self.hook_allowed()
    }
}

impl fmt::Display for Protection {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Protection::Protected(reason) => write!(f, "protected: {reason}"),
            Protection::KnownUnprotected => write!(f, "known unprotected"),
            Protection::Unknown => write!(f, "unknown (treated as protected)"),
        }
    }
}

/// A file or folder name inside an install folder that gives an anti-cheat away.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AntiCheatMarker {
    pub anti_cheat: AntiCheat,
    /// Path of the hit relative to the install folder (or the bare name).
    pub found: String,
}

/// One entry of the built in list.
#[derive(Debug, Clone, Copy)]
pub struct KnownProtected {
    pub exe: &'static str,
    pub game: &'static str,
    pub anti_cheat: AntiCheat,
    /// True if the process name was seen in a real installation on the development
    /// machine (2026-10-10). False: taken from the raw list, name from general knowledge.
    pub verified_locally: bool,
}

const fn known(exe: &'static str, game: &'static str, anti_cheat: AntiCheat, verified_locally: bool) -> KnownProtected {
    KnownProtected { exe, game, anti_cheat, verified_locally }
}

/// Start set taken from the raw list in `Recherche Anti-Cheat`, section 7.2. Entries the
/// raw list marks with (?) or "unbelegt" are left out on purpose. Leaving a game out is
/// harmless because unknown games are protected anyway; the list only adds the reason and
/// lets Klip recognise games from launchers it does not scan.
pub const KNOWN_PROTECTED: &[KnownProtected] = &[
    known("VALORANT.exe", "VALORANT", AntiCheat::Vanguard, true),
    known("VALORANT-Win64-Shipping.exe", "VALORANT", AntiCheat::Vanguard, true),
    known("League of Legends.exe", "League of Legends", AntiCheat::Vanguard, false),
    known("LeagueClient.exe", "League of Legends", AntiCheat::Vanguard, false),
    known("LeagueClientUx.exe", "League of Legends", AntiCheat::Vanguard, false),
    known("FortniteClient-Win64-Shipping.exe", "Fortnite", AntiCheat::EasyAntiCheat, true),
    known("FortniteClient-Win64-Shipping_EAC_EOS.exe", "Fortnite", AntiCheat::EasyAntiCheat, true),
    known("FortniteLauncher.exe", "Fortnite", AntiCheat::EasyAntiCheat, true),
    known("r5apex.exe", "Apex Legends", AntiCheat::EasyAntiCheat, false),
    known("r5apex_dx12.exe", "Apex Legends", AntiCheat::EasyAntiCheat, false),
    known("RustClient.exe", "Rust", AntiCheat::EasyAntiCheat, false),
    known("TslGame.exe", "PUBG: Battlegrounds", AntiCheat::BattlEye, false),
    known("RainbowSix.exe", "Rainbow Six Siege", AntiCheat::BattlEye, false),
    known("RainbowSix_Vulkan.exe", "Rainbow Six Siege", AntiCheat::BattlEye, false),
    known("RainbowSix_BE.exe", "Rainbow Six Siege", AntiCheat::BattlEye, false),
    known("EscapeFromTarkov.exe", "Escape from Tarkov", AntiCheat::BattlEye, false),
    known("EscapeFromTarkov_BE.exe", "Escape from Tarkov", AntiCheat::BattlEye, false),
    known("destiny2.exe", "Destiny 2", AntiCheat::BattlEye, false),
    known("cod.exe", "Call of Duty", AntiCheat::Ricochet, false),
    known("BF2042.exe", "Battlefield 2042", AntiCheat::EaJavelin, false),
    known("bf6.exe", "Battlefield 6", AntiCheat::EaJavelin, false),
    known("FC25.exe", "EA Sports FC 25", AntiCheat::EaJavelin, false),
    known("F1_24.exe", "F1 24", AntiCheat::EaJavelin, false),
    known("F1_25.exe", "F1 25", AntiCheat::EaJavelin, false),
    known("eldenring.exe", "Elden Ring", AntiCheat::EasyAntiCheat, false),
    known("nightreign.exe", "Elden Ring Nightreign", AntiCheat::EasyAntiCheat, true),
    known("armoredcore6.exe", "Armored Core VI", AntiCheat::EasyAntiCheat, false),
    known("start_protected_game.exe", "Easy Anti-Cheat launcher", AntiCheat::EasyAntiCheat, true),
    known("DeadByDaylight-Win64-Shipping.exe", "Dead by Daylight", AntiCheat::EasyAntiCheat, false),
    known("Discovery.exe", "THE FINALS", AntiCheat::EasyAntiCheat, false),
    known("HuntGame.exe", "Hunt: Showdown 1896", AntiCheat::EasyAntiCheat, false),
    known("FallGuys_client_game.exe", "Fall Guys", AntiCheat::EasyAntiCheat, false),
    known("HaloInfinite.exe", "Halo Infinite", AntiCheat::EasyAntiCheat, false),
    known("NewWorld.exe", "New World: Aeternum", AntiCheat::EasyAntiCheat, false),
    known("LOSTARK.exe", "Lost Ark", AntiCheat::EasyAntiCheat, false),
    known("TL.exe", "Throne and Liberty", AntiCheat::EasyAntiCheat, false),
    known("RocketLeague.exe", "Rocket League", AntiCheat::EasyAntiCheat, false),
    known("SoTGame.exe", "Sea of Thieves", AntiCheat::EasyAntiCheat, false),
    known("SquadGame.exe", "Squad", AntiCheat::EasyAntiCheat, false),
    known("HLL-Win64-Shipping.exe", "Hell Let Loose", AntiCheat::EasyAntiCheat, false),
    known("Darktide.exe", "Warhammer 40,000: Darktide", AntiCheat::EasyAntiCheat, false),
    known("VRChat.exe", "VRChat", AntiCheat::EasyAntiCheat, false),
    known("StarCitizen.exe", "Star Citizen", AntiCheat::EasyAntiCheat, false),
    known("ForHonor.exe", "For Honor", AntiCheat::EasyAntiCheat, false),
    known("TheDivision2.exe", "The Division 2", AntiCheat::EasyAntiCheat, false),
    known("NBA2K25.exe", "NBA 2K25", AntiCheat::EasyAntiCheat, false),
    known("M1-Win64-Shipping.exe", "The First Descendant", AntiCheat::NexonGameSecurity, false),
    known("GTA5.exe", "Grand Theft Auto V", AntiCheat::BattlEye, false),
    known("GTA5_Enhanced.exe", "Grand Theft Auto V", AntiCheat::BattlEye, false),
    known("GTA5_BE.exe", "Grand Theft Auto V", AntiCheat::BattlEye, false),
    known("PlayGTAV.exe", "Grand Theft Auto V", AntiCheat::BattlEye, false),
    known("DayZ_x64.exe", "DayZ", AntiCheat::BattlEye, false),
    known("DayZ_BE.exe", "DayZ", AntiCheat::BattlEye, false),
    known("arma3_x64.exe", "Arma 3", AntiCheat::BattlEye, false),
    known("arma3battleye.exe", "Arma 3", AntiCheat::BattlEye, false),
    known("ArmaReforgerSteam.exe", "Arma Reforger", AntiCheat::BattlEye, false),
    known("ArkAscended.exe", "ARK: Survival Ascended", AntiCheat::BattlEye, false),
    known("ArkAscended_BE.exe", "ARK: Survival Ascended", AntiCheat::BattlEye, false),
    known("aces.exe", "War Thunder", AntiCheat::BattlEye, false),
    known("helldivers2.exe", "Helldivers 2", AntiCheat::GameGuard, false),
    known("GenshinImpact.exe", "Genshin Impact", AntiCheat::MihoyoProtect, false),
    known("YuanShen.exe", "Genshin Impact", AntiCheat::MihoyoProtect, false),
    known("StarRail.exe", "Honkai: Star Rail", AntiCheat::MihoyoProtect, false),
    known("ZenlessZoneZero.exe", "Zenless Zone Zero", AntiCheat::MihoyoProtect, false),
    known("Wuthering Waves.exe", "Wuthering Waves", AntiCheat::AntiCheatExpert, false),
    known("DeltaForceClient-Win64-Shipping.exe", "Delta Force", AntiCheat::AntiCheatExpert, false),
    known("NarakaBladepoint.exe", "Naraka: Bladepoint", AntiCheat::NeacProtect, false),
    known("Marvel-Win64-Shipping.exe", "Marvel Rivals", AntiCheat::NeacProtect, false),
    known("ONCE_HUMAN.exe", "Once Human", AntiCheat::NeacProtect, false),
    known("BlackDesert64.exe", "Black Desert Online", AntiCheat::Xigncode, false),
    known("pso2.exe", "Phantasy Star Online 2 New Genesis", AntiCheat::GameGuard, false),
    known("crossfire.exe", "CrossFire", AntiCheat::Xigncode, false),
    known("MapleStory.exe", "MapleStory", AntiCheat::NexonGameSecurity, false),
    known("eFootball.exe", "eFootball", AntiCheat::Denuvo, false),
    known("cs2.exe", "Counter-Strike 2", AntiCheat::VacTrustedMode, false),
    known("Overwatch.exe", "Overwatch 2", AntiCheat::Blizzard, true),
    known("RobloxPlayerBeta.exe", "Roblox", AntiCheat::Hyperion, false),
];

/// Games FACEIT AC guards when it is installed. With FACEIT present the reason for these
/// switches from the game's own system to FACEIT, which is the kernel one.
const FACEIT_GAMES: &[&str] = &["cs2.exe", "csgo.exe"];

/// Built in allow list for stage 2 of the overlay. Empty on purpose: every entry needs its
/// own research on what the game's anti-cheat vendor says (rule 2 in CLAUDE.md) and an
/// owner decision. Until then `hook_allowed()` is false for every game.
pub const HOOK_ALLOWLIST: &[&str] = &[];

/// Looks a process name up in the built in list, ignoring case.
pub fn known_protected(exe_name: &str) -> Option<&'static KnownProtected> {
    KNOWN_PROTECTED.iter().find(|k| k.exe.eq_ignore_ascii_case(exe_name))
}

/// Maps the name of one directory entry to the anti-cheat it gives away. Names only, the
/// entry itself is never opened.
pub fn marker_for_entry(name: &str, is_dir: bool) -> Option<AntiCheat> {
    let lower = name.to_ascii_lowercase();
    if is_dir {
        return match lower.as_str() {
            "easyanticheat" | "easyanticheat_eos" => Some(AntiCheat::EasyAntiCheat),
            "battleye" => Some(AntiCheat::BattlEye),
            "gameguard" => Some(AntiCheat::GameGuard),
            "xigncode" => Some(AntiCheat::Xigncode),
            "anticheatexpert" => Some(AntiCheat::AntiCheatExpert),
            "eaanticheat" => Some(AntiCheat::EaJavelin),
            _ => None,
        };
    }
    match lower.as_str() {
        "start_protected_game.exe" | "easyanticheat_setup.exe" | "easyanticheat_eos_setup.exe" => {
            return Some(AntiCheat::EasyAntiCheat)
        }
        "beservice_x64.exe" | "beclient_x64.dll" => return Some(AntiCheat::BattlEye),
        "eaanticheat.gameservicelauncher.exe" => return Some(AntiCheat::EaJavelin),
        "gameguard.des" | "gamemon.des" | "gamemon64.des" => return Some(AntiCheat::GameGuard),
        "xhunter1.sys" | "x3.xem" => return Some(AntiCheat::Xigncode),
        "hoyokprotect.sys" | "mhyprot2.sys" | "mhyprot3.sys" => return Some(AntiCheat::MihoyoProtect),
        "blackcipher64.aes" => return Some(AntiCheat::NexonGameSecurity),
        _ => {}
    }
    if lower.ends_with("_be.exe") {
        return Some(AntiCheat::BattlEye);
    }
    if lower.ends_with("_eac.exe") || lower.ends_with("_eac_eos.exe") {
        return Some(AntiCheat::EasyAntiCheat);
    }
    None
}

/// Markers next to an exe whose install folder is not known: the folder of the exe and
/// two levels above it, as the research recommends. Reads directory listings only.
pub fn markers_near(exe_path: &Path) -> Vec<AntiCheatMarker> {
    let mut markers = Vec::new();
    let mut dir = exe_path.parent();
    for _ in 0..3 {
        let Some(current) = dir else { break };
        // A drive root or a path without a real parent is not a game folder.
        if current.parent().is_none() {
            break;
        }
        if let Ok(entries) = std::fs::read_dir(current) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
                if let Some(anti_cheat) = marker_for_entry(&name, is_dir) {
                    markers.push(AntiCheatMarker { anti_cheat, found: name });
                }
            }
        }
        dir = current.parent();
    }
    markers
}

/// What is installed on this machine, read once, plus the allow list. Classifies games
/// without touching any process.
#[derive(Debug, Clone)]
pub struct ProtectionDb {
    /// Vanguard is INSTALLED (service entries `vgc` or `vgk`, or its program folder).
    /// Whether it is running does not matter: since Vanguard On-Demand the driver only
    /// loads together with the game.
    pub vanguard_installed: bool,
    pub faceit_installed: bool,
    allow: Vec<String>,
}

impl ProtectionDb {
    /// Reads the machine state from the registry and the program folder.
    pub fn probe() -> Self {
        let service = |name: &str| {
            reg::key_exists(reg::HKEY_LOCAL_MACHINE, &format!("SYSTEM\\CurrentControlSet\\Services\\{name}"))
        };
        let program = |folder: &str| {
            std::env::var_os("ProgramFiles").map(|base| Path::new(&base).join(folder).is_dir()).unwrap_or(false)
        };
        Self {
            vanguard_installed: service("vgc") || service("vgk") || program("Riot Vanguard"),
            faceit_installed: service("FACEITService") || service("FACEIT") || program("FACEIT AC"),
            allow: HOOK_ALLOWLIST.iter().map(|s| s.to_ascii_lowercase()).collect(),
        }
    }

    /// A database with a given machine state instead of the real one, for tests.
    pub fn fixed(vanguard_installed: bool, faceit_installed: bool) -> Self {
        Self { vanguard_installed, faceit_installed, allow: Vec::new() }
    }

    /// Replaces the allow list for stage 2 of the overlay. Not wired to any setting on
    /// purpose, see `HOOK_ALLOWLIST`.
    pub fn with_hook_allowlist<I: IntoIterator<Item = S>, S: AsRef<str>>(mut self, exe_names: I) -> Self {
        self.allow = exe_names.into_iter().map(|s| s.as_ref().to_ascii_lowercase()).collect();
        self
    }

    /// The core decision. Any evidence wins over the allow list, so an allowed game that
    /// later ships an anti-cheat is locked again without an update of Klip.
    pub fn classify(
        &self,
        exe_names: &[&str],
        markers: &[AntiCheatMarker],
        source: Option<GameSource>,
        vanguard_declared: bool,
    ) -> Protection {
        let protected = |anti_cheat, evidence| Protection::Protected(ProtectionReason { anti_cheat, evidence });
        let listed = exe_names.iter().find_map(|name| known_protected(name));

        if self.faceit_installed && exe_names.iter().any(|n| FACEIT_GAMES.iter().any(|g| g.eq_ignore_ascii_case(n))) {
            return protected(AntiCheat::Faceit, Evidence::FaceitInstalled);
        }
        if let Some(entry) = listed {
            return protected(entry.anti_cheat, Evidence::BuiltInList { verified_locally: entry.verified_locally });
        }
        if let Some(marker) = markers.first() {
            return protected(marker.anti_cheat, Evidence::InstallMarker { found: marker.found.clone() });
        }
        if vanguard_declared {
            return protected(AntiCheat::Vanguard, Evidence::RiotMetadata);
        }
        if source == Some(GameSource::Riot) && self.vanguard_installed {
            return protected(AntiCheat::Vanguard, Evidence::RiotTitleWithVanguard);
        }
        if !exe_names.is_empty() && exe_names.iter().all(|n| self.allow.contains(&n.to_ascii_lowercase())) {
            return Protection::KnownUnprotected;
        }
        Protection::Unknown
    }

    /// Classifies a game from the library scan.
    pub fn classify_installed(&self, game: &InstalledGame) -> Protection {
        // Every exe of the install has to be on the allow list for the install as a whole
        // to count as released. The exe that is actually running is judged on its own
        // in `classify_running`.
        let names: Vec<&str> = game.exe_names.iter().map(String::as_str).collect();
        self.classify(&names, &game.markers, Some(game.source), game.vanguard_declared)
    }

    /// Classifies a running program by the path of its exe. `game` is the library entry
    /// the path belongs to, if any. The process itself is not touched.
    pub fn classify_running(&self, exe_path: Option<&Path>, exe_name: &str, game: Option<&InstalledGame>) -> Protection {
        if let Some(game) = game {
            if let Protection::Protected(reason) = self.classify_installed(game) {
                return Protection::Protected(reason);
            }
            return self.classify(&[exe_name], &game.markers, Some(game.source), game.vanguard_declared);
        }
        let markers = exe_path.map(markers_near).unwrap_or_default();
        self.classify(&[exe_name], &markers, None, false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_is_never_hookable() {
        let db = ProtectionDb::fixed(false, false);
        let p = db.classify(&["SomeIndieGame.exe"], &[], Some(GameSource::Steam), false);
        assert_eq!(p, Protection::Unknown);
        assert!(!p.hook_allowed());
        assert!(p.is_protected());
    }

    #[test]
    fn built_in_list_ignores_case() {
        let db = ProtectionDb::fixed(false, false);
        let p = db.classify(&["valorant-win64-shipping.EXE"], &[], None, false);
        assert!(matches!(p, Protection::Protected(ProtectionReason { anti_cheat: AntiCheat::Vanguard, .. })));
        assert!(!p.hook_allowed());
    }

    #[test]
    fn allow_list_only_without_evidence() {
        let db = ProtectionDb::fixed(false, false).with_hook_allowlist(["PEAK.exe", "nightreign.exe"]);
        assert!(db.classify(&["peak.exe"], &[], Some(GameSource::Steam), false).hook_allowed());
        // On the allow list AND on the protected list: evidence wins.
        assert!(!db.classify(&["nightreign.exe"], &[], None, false).hook_allowed());
        // On the allow list, but an anti-cheat folder appeared in the install.
        let marker = AntiCheatMarker { anti_cheat: AntiCheat::EasyAntiCheat, found: "EasyAntiCheat".into() };
        assert!(!db.classify(&["peak.exe"], &[marker], None, false).hook_allowed());
        // An empty name list must never count as "all names allowed".
        assert!(!db.classify(&[], &[], None, false).hook_allowed());
    }

    #[test]
    fn default_allow_list_is_empty() {
        assert!(HOOK_ALLOWLIST.is_empty());
    }

    #[test]
    fn riot_titles_follow_vanguard() {
        let with = ProtectionDb::fixed(true, false);
        let without = ProtectionDb::fixed(false, false);
        let p = with.classify(&["SomeNewRiotGame.exe"], &[], Some(GameSource::Riot), false);
        assert!(matches!(p, Protection::Protected(ProtectionReason { evidence: Evidence::RiotTitleWithVanguard, .. })));
        // Without Vanguard an unlisted Riot title is unknown, which is still locked.
        let p = without.classify(&["SomeNewRiotGame.exe"], &[], Some(GameSource::Riot), false);
        assert_eq!(p, Protection::Unknown);
        let p = without.classify(&["SomeNewRiotGame.exe"], &[], Some(GameSource::Riot), true);
        assert!(matches!(p, Protection::Protected(ProtectionReason { evidence: Evidence::RiotMetadata, .. })));
    }

    #[test]
    fn faceit_takes_over_cs2() {
        let p = ProtectionDb::fixed(false, true).classify(&["cs2.exe"], &[], Some(GameSource::Steam), false);
        assert!(matches!(p, Protection::Protected(ProtectionReason { anti_cheat: AntiCheat::Faceit, .. })));
        let p = ProtectionDb::fixed(false, false).classify(&["cs2.exe"], &[], Some(GameSource::Steam), false);
        assert!(matches!(p, Protection::Protected(ProtectionReason { anti_cheat: AntiCheat::VacTrustedMode, .. })));
    }

    #[test]
    fn marker_names() {
        assert_eq!(marker_for_entry("EasyAntiCheat", true), Some(AntiCheat::EasyAntiCheat));
        assert_eq!(marker_for_entry("easyanticheat", true), Some(AntiCheat::EasyAntiCheat));
        assert_eq!(marker_for_entry("BattlEye", true), Some(AntiCheat::BattlEye));
        assert_eq!(marker_for_entry("start_protected_game.exe", false), Some(AntiCheat::EasyAntiCheat));
        assert_eq!(marker_for_entry("DayZ_BE.exe", false), Some(AntiCheat::BattlEye));
        assert_eq!(marker_for_entry("FortniteClient-Win64-Shipping_EAC_EOS.exe", false), Some(AntiCheat::EasyAntiCheat));
        // A file named like the folder is not a hit, and neither is an ordinary exe.
        assert_eq!(marker_for_entry("EasyAntiCheat", false), None);
        assert_eq!(marker_for_entry("game.exe", false), None);
        assert_eq!(marker_for_entry("Saves", true), None);
    }

    #[test]
    fn list_has_no_duplicate_names() {
        for (i, a) in KNOWN_PROTECTED.iter().enumerate() {
            for b in &KNOWN_PROTECTED[i + 1..] {
                assert!(!a.exe.eq_ignore_ascii_case(b.exe), "{} is listed twice", a.exe);
            }
        }
    }
}
