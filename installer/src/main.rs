//! Setup program for Klip. One file that carries the program files, copies them into the
//! user's program folder, adds a start menu entry and an entry under "Installed apps",
//! and can take all of it away again. Installs per user, so it never asks for admin rights.
//!
//!   klip-setup.exe                  install, with questions
//!   klip-setup.exe /S               install without questions
//!   klip-setup.exe --dir <folder>   install somewhere else
//!   klip-setup.exe --no-shortcut --no-registry --no-start   for tests
//!   uninstall.exe --uninstall [/S]  remove (settings and clips stay)

#![windows_subsystem = "windows"]

use std::io::Write;
use std::path::{Path, PathBuf};
use windows::core::{Interface, HSTRING, PCWSTR};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, IPersistFile, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
};
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteTreeW, RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_WRITE,
    REG_DWORD, REG_OPTION_NON_VOLATILE, REG_SZ,
};
use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
use windows::Win32::UI::WindowsAndMessaging::{
    MessageBoxW, IDYES, MB_ICONINFORMATION, MB_ICONQUESTION, MB_ICONWARNING, MB_OK, MB_YESNO, MESSAGEBOX_STYLE,
};

include!(concat!(env!("OUT_DIR"), "/files.rs"));

const UNINSTALL_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\Klip";
const RECORDER: &str = "klip-recorder.exe";
const APP: &str = "Klip.exe";

struct Options {
    silent: bool,
    uninstall: bool,
    dir: Option<PathBuf>,
    shortcut: bool,
    registry: bool,
    start: bool,
}

fn main() {
    let mut o = Options { silent: false, uninstall: false, dir: None, shortcut: true, registry: true, start: true };
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "/S" | "--silent" => o.silent = true,
            "--uninstall" => o.uninstall = true,
            "--dir" => o.dir = args.next().map(PathBuf::from),
            "--no-shortcut" => o.shortcut = false,
            "--no-registry" => o.registry = false,
            "--no-start" => o.start = false,
            _ => {}
        }
    }
    let result = if o.uninstall { uninstall(&o) } else { install(&o) };
    if let Err(e) = result {
        if !o.silent {
            message(&format!("Das hat nicht geklappt:\n\n{e}"), MB_OK | MB_ICONWARNING);
        }
        std::process::exit(1);
    }
}

fn default_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_default();
    base.join("Programs").join("Klip")
}

fn install(o: &Options) -> Result<(), String> {
    if FILES.is_empty() {
        return Err("Diese Setup-Datei enthält keine Programmdateien.".into());
    }
    let dir = o.dir.clone().unwrap_or_else(default_dir);
    if !o.silent {
        let text = format!(
            "Klip. installieren?\n\nOrdner: {}\n\nInstalliert wird nur für dieses Benutzerkonto. Einstellungen und Clips einer früheren Version bleiben erhalten.",
            dir.display()
        );
        if message(&text, MB_YESNO | MB_ICONQUESTION) != IDYES.0 {
            return Ok(());
        }
    }

    // A running recorder holds its own exe open, ask it to leave first.
    stop_recorder();
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    for (rel, bytes) in FILES {
        let target = dir.join(rel);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        write_with_retry(&target, bytes)?;
    }
    // The setup doubles as the uninstaller.
    let uninstaller = dir.join("uninstall.exe");
    if let Ok(me) = std::env::current_exe() {
        if me != uninstaller {
            let _ = std::fs::copy(me, &uninstaller);
        }
    }

    let main_exe = if dir.join(APP).exists() { dir.join(APP) } else { dir.join(RECORDER) };
    if o.shortcut {
        if let Err(e) = create_shortcut(&shortcut_path(), &main_exe, &dir) {
            eprintln!("shortcut: {e}");
        }
    }
    if o.registry {
        register(&dir, &uninstaller, &main_exe)?;
    }

    let recorder = dir.join(RECORDER);
    if !o.silent && recorder.exists() {
        let ask = "Soll Klip. mit Windows starten?\n\nDer Recorder sitzt dann im Infobereich und puffert, sobald ein Spiel läuft. Das lässt sich jederzeit über das Symbol im Infobereich ändern.";
        if message(ask, MB_YESNO | MB_ICONQUESTION) == IDYES.0 {
            let _ = klip_tray::autostart::enable_with_args(&recorder, "--tray");
        }
    }
    if o.start && recorder.exists() {
        let _ = std::process::Command::new(&recorder).arg("--tray").current_dir(&dir).spawn();
    }
    if !o.silent {
        message(
            "Klip. ist installiert.\n\nDas Symbol sitzt im Infobereich neben der Uhr. Mit Alt+F10 speicherst du die letzten 60 Sekunden.",
            MB_OK | MB_ICONINFORMATION,
        );
    }
    Ok(())
}

fn uninstall(o: &Options) -> Result<(), String> {
    let me = std::env::current_exe().map_err(|e| e.to_string())?;
    let dir = o.dir.clone().or_else(|| me.parent().map(Path::to_path_buf)).ok_or("kein Ordner")?;
    // Refuse to delete a folder that is not a Klip installation.
    if !dir.join(RECORDER).exists() && !dir.join(APP).exists() {
        return Err(format!("In {} liegt kein Klip.", dir.display()));
    }
    if !o.silent {
        let text = "Klip. entfernen?\n\nDeine Clips und Einstellungen bleiben erhalten.";
        if message(text, MB_YESNO | MB_ICONQUESTION) != IDYES.0 {
            return Ok(());
        }
    }
    stop_recorder();
    if klip_tray::autostart::points_to(&dir.join(RECORDER)) {
        let _ = klip_tray::autostart::disable();
    }
    if o.shortcut {
        let _ = std::fs::remove_file(shortcut_path());
    }
    if o.registry {
        unsafe {
            let _ = RegDeleteTreeW(HKEY_CURRENT_USER, &HSTRING::from(UNINSTALL_KEY));
        }
    }
    for (rel, _) in FILES {
        let _ = std::fs::remove_file(dir.join(rel));
    }
    // Leftover empty folders of the payload, deepest first.
    let mut folders: Vec<PathBuf> = FILES
        .iter()
        .filter_map(|(rel, _)| Path::new(rel).parent().map(|p| dir.join(p)))
        .filter(|p| *p != dir)
        .collect();
    folders.sort_by_key(|p| std::cmp::Reverse(p.components().count()));
    folders.dedup();
    folders.iter().for_each(|f| {
        let _ = std::fs::remove_dir(f);
    });

    // A running program cannot delete itself. A detached shell waits a moment, removes
    // the uninstaller and then the folder, which only goes away if it is empty.
    let script = format!(
        "ping -n 3 127.0.0.1 >nul & del /q \"{}\" & rmdir \"{}\"",
        dir.join("uninstall.exe").display(),
        dir.display()
    );
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let _ = std::process::Command::new("cmd.exe")
        // cmd has its own quoting rules, the line must reach it untouched.
        .raw_arg(format!("/c {script}"))
        .creation_flags(CREATE_NO_WINDOW)
        .current_dir(std::env::temp_dir())
        .spawn();
    if !o.silent {
        message("Klip. wurde entfernt.", MB_OK | MB_ICONINFORMATION);
    }
    Ok(())
}

/// Asks a running recorder to leave through its pipe and gives it a moment to close files.
fn stop_recorder() {
    let Ok(mut pipe) = std::fs::OpenOptions::new().read(true).write(true).open(klip_core::PIPE_NAME) else { return };
    let _ = pipe.write_all(b"{\"cmd\":\"quit\"}\n");
    drop(pipe);
    std::thread::sleep(std::time::Duration::from_secs(2));
}

/// Writing fails while the old file is still open, for example in the second the
/// recorder needs to shut down.
fn write_with_retry(target: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut last = String::new();
    for _ in 0..10 {
        match std::fs::write(target, bytes) {
            Ok(()) => return Ok(()),
            Err(e) => last = e.to_string(),
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    Err(format!("{}: {last}", target.display()))
}

fn shortcut_path() -> PathBuf {
    let base = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_default();
    base.join(r"Microsoft\Windows\Start Menu\Programs").join("Klip.lnk")
}

fn create_shortcut(link: &Path, target: &Path, workdir: &Path) -> Result<(), String> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let shell: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).map_err(|e| e.to_string())?;
        shell.SetPath(&HSTRING::from(target.as_os_str())).map_err(|e| e.to_string())?;
        shell.SetWorkingDirectory(&HSTRING::from(workdir.as_os_str())).map_err(|e| e.to_string())?;
        shell.SetDescription(&HSTRING::from("Klip. Clips und Aufnahmen")).map_err(|e| e.to_string())?;
        let file: IPersistFile = shell.cast().map_err(|e| e.to_string())?;
        file.Save(&HSTRING::from(link.as_os_str()), true).map_err(|e| e.to_string())
    }
}

/// The entry under "Installed apps" in the Windows settings.
fn register(dir: &Path, uninstaller: &Path, icon: &Path) -> Result<(), String> {
    unsafe {
        let mut key = HKEY::default();
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            &HSTRING::from(UNINSTALL_KEY),
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            None,
            &mut key,
            None,
        )
        .ok()
        .map_err(|e| e.to_string())?;
        let text = |name: &str, value: &str| {
            let wide: Vec<u16> = value.encode_utf16().chain(Some(0)).collect();
            let bytes = std::slice::from_raw_parts(wide.as_ptr() as *const u8, wide.len() * 2);
            let _ = RegSetValueExW(key, &HSTRING::from(name), None, REG_SZ, Some(bytes));
        };
        let number = |name: &str, value: u32| {
            let _ = RegSetValueExW(key, &HSTRING::from(name), None, REG_DWORD, Some(&value.to_le_bytes()));
        };
        text("DisplayName", "Klip.");
        text("DisplayVersion", env!("CARGO_PKG_VERSION"));
        text("Publisher", "Kirit");
        text("InstallLocation", &dir.display().to_string());
        text("DisplayIcon", &icon.display().to_string());
        text("UninstallString", &format!("\"{}\" --uninstall", uninstaller.display()));
        text("QuietUninstallString", &format!("\"{}\" --uninstall /S", uninstaller.display()));
        number("NoModify", 1);
        number("NoRepair", 1);
        number("EstimatedSize", (FILES.iter().map(|(_, b)| b.len()).sum::<usize>() / 1024) as u32);
        let _ = RegCloseKey(key);
    }
    Ok(())
}

fn message(text: &str, style: MESSAGEBOX_STYLE) -> i32 {
    unsafe { MessageBoxW(None, &HSTRING::from(text), &HSTRING::from("Klip."), style).0 }
}
