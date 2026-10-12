//! Parser tests against small files modelled on the real ones from the development machine.

use klip_games::library::{
    parse_app_manifest, parse_epic_manifest, parse_library_folders, parse_riot_installs, parse_riot_product_settings,
};
use klip_games::vdf;
use std::path::PathBuf;

#[test]
fn library_folders_new_layout() {
    let folders = parse_library_folders(include_str!("data/libraryfolders_new.vdf")).unwrap();
    assert_eq!(
        folders,
        vec![PathBuf::from("C:\\Program Files (x86)\\Steam"), PathBuf::from("D:\\Games\\SteamLibrary")]
    );
}

#[test]
fn library_folders_old_layout() {
    let folders = parse_library_folders(include_str!("data/libraryfolders_old.vdf")).unwrap();
    // The numeric keys are folders, the two statistics keys are not.
    assert_eq!(folders, vec![PathBuf::from("D:\\SteamLibrary"), PathBuf::from("E:\\Games\\Steam")]);
}

#[test]
fn library_folders_rejects_other_files() {
    assert!(parse_library_folders(include_str!("data/appmanifest_1222140.acf")).is_err());
    assert!(parse_library_folders("\"libraryfolders\" {").is_err());
}

#[test]
fn app_manifest() {
    let app = parse_app_manifest(include_str!("data/appmanifest_1222140.acf")).unwrap();
    assert_eq!(app.appid, "1222140");
    assert_eq!(app.name, "Detroit: Become Human");
    assert_eq!(app.installdir, "Detroit Become Human");
    assert_eq!(app.state_flags, Some(4));
}

#[test]
fn app_manifest_with_escapes_and_non_ascii() {
    let app = parse_app_manifest(include_str!("data/appmanifest_quotes.acf")).unwrap();
    assert_eq!(app.name, "Say \"Hi\": The Game\u{ae}");
    assert_eq!(app.installdir, "Dale&Dawson");
    assert_eq!(app.state_flags, Some(518));
}

#[test]
fn app_manifest_missing_fields() {
    assert!(parse_app_manifest("\"AppState\" { \"appid\" \"1\" }").is_err());
    assert!(parse_app_manifest("").is_err());
}

#[test]
fn nested_blocks_survive() {
    let root = vdf::parse(include_str!("data/appmanifest_1222140.acf")).unwrap();
    let state = root.get("AppState").unwrap();
    assert_eq!(state.get("InstalledDepots").unwrap().get("1222141").unwrap().str("size"), Some("63077578235"));
    assert_eq!(state.get("userconfig").unwrap().str("language"), Some("german"));
}

#[test]
fn epic_manifest() {
    let app = parse_epic_manifest(include_str!("data/fortnite.item")).unwrap();
    assert_eq!(app.app_name, "Fortnite");
    assert_eq!(app.display_name, "Fortnite");
    assert_eq!(app.install_location, PathBuf::from("D:\\Games\\Fortnite\\Fortnite"));
    assert_eq!(app.launch_executable, "FortniteGame/Binaries/Win64/FortniteBootstrapper.exe");
    assert!(app.is_game && !app.is_dlc && !app.incomplete);
}

#[test]
fn epic_manifest_engine_and_dlc_are_not_games() {
    let engine = parse_epic_manifest(include_str!("data/engine.item")).unwrap();
    assert!(!engine.is_game);
    assert!(!engine.is_dlc);

    let dlc = parse_epic_manifest(include_str!("data/dlc.item")).unwrap();
    assert!(dlc.is_dlc && dlc.incomplete);
    // No DisplayName in the file: the internal name stands in.
    assert_eq!(dlc.display_name, "abc123dlc");
}

#[test]
fn epic_manifest_broken() {
    assert!(parse_epic_manifest("{").is_err());
    assert!(parse_epic_manifest("{\"AppName\": \"x\"}").is_err());
    assert!(parse_epic_manifest("{\"InstallLocation\": \"\"}").is_err());
}

#[test]
fn riot_product_settings() {
    let s = parse_riot_product_settings(include_str!("data/valorant.live.product_settings.yaml"));
    assert_eq!(s.install_full_path, Some(PathBuf::from("D:/Games/Valorant/Riot Games/VALORANT/live")));
    assert_eq!(s.install_root, Some(PathBuf::from("D:/Games/Valorant/Riot Games")));
    assert!(s.vanguard);
}

#[test]
fn riot_vanguard_flag_only_counts_under_dependencies() {
    let s = parse_riot_product_settings(include_str!("data/no_vanguard.product_settings.yaml"));
    assert_eq!(s.install_full_path, Some(PathBuf::from("C:/Riot Games/LoR/live")));
    assert!(!s.vanguard);
}

#[test]
fn riot_installs() {
    let paths = parse_riot_installs(include_str!("data/RiotClientInstalls.json")).unwrap();
    assert_eq!(paths, vec![PathBuf::from("D:/Games/Valorant/Riot Games/VALORANT/live/")]);
    assert_eq!(parse_riot_installs("{}").unwrap(), Vec::<PathBuf>::new());
    assert!(parse_riot_installs("not json").is_err());
}
