//! Global hotkeys through RegisterHotKey. That API lives entirely in this process: no
//! hook, nothing is loaded into other programs, and only the registered combinations are
//! ever reported.

use klip_core::{HotkeyAction, HotkeyBinding};
use windows::Win32::UI::Input::KeyboardAndMouse::*;

/// Parses "Alt+Shift+F10" into modifier flags and a virtual key code.
pub fn parse(keys: &str) -> Result<(HOT_KEY_MODIFIERS, u32), String> {
    let mut mods = MOD_NOREPEAT;
    let mut key = None;
    for part in keys.split('+').map(str::trim) {
        let upper = part.to_uppercase();
        match upper.as_str() {
            "ALT" => mods |= MOD_ALT,
            "CTRL" | "STRG" | "CONTROL" => mods |= MOD_CONTROL,
            "SHIFT" | "UMSCHALT" => mods |= MOD_SHIFT,
            "WIN" => mods |= MOD_WIN,
            name => {
                if key.is_some() {
                    return Err(format!("'{keys}' names more than one key"));
                }
                key = Some(key_code(name).ok_or(format!("'{part}' in '{keys}' is not a known key"))?);
            }
        }
    }
    Ok((mods, key.ok_or(format!("'{keys}' has no key"))?))
}

fn key_code(name: &str) -> Option<u32> {
    if let Some(n) = name.strip_prefix('F').and_then(|n| n.parse::<u32>().ok()) {
        return (1..=24).contains(&n).then(|| VK_F1.0 as u32 + n - 1);
    }
    if name.len() == 1 {
        let c = name.as_bytes()[0];
        if c.is_ascii_uppercase() || c.is_ascii_digit() {
            return Some(c as u32);
        }
    }
    let vk = match name {
        "SPACE" | "LEERTASTE" => VK_SPACE,
        "TAB" => VK_TAB,
        "INSERT" | "EINFG" => VK_INSERT,
        "DELETE" | "ENTF" => VK_DELETE,
        "HOME" | "POS1" => VK_HOME,
        "END" | "ENDE" => VK_END,
        "PAGEUP" => VK_PRIOR,
        "PAGEDOWN" => VK_NEXT,
        "PAUSE" => VK_PAUSE,
        "PRINT" | "DRUCK" => VK_SNAPSHOT,
        "SCROLL" | "ROLLEN" => VK_SCROLL,
        _ => return None,
    };
    Some(vk.0 as u32)
}

/// Registers every binding and returns the actions by hotkey id. Problems (unknown key,
/// same combination twice, taken by another program) are reported, the rest still works.
pub fn register(bindings: &[HotkeyBinding]) -> (Vec<HotkeyAction>, Vec<String>) {
    let mut actions = Vec::new();
    let mut problems = Vec::new();
    let mut seen: Vec<(u32, u32, String)> = Vec::new();
    for b in bindings {
        let (mods, key) = match parse(&b.keys) {
            Ok(v) => v,
            Err(e) => {
                problems.push(e);
                continue;
            }
        };
        if let Some((_, _, other)) = seen.iter().find(|(m, k, _)| *m == mods.0 && *k == key) {
            problems.push(format!("'{}' is assigned twice (also as '{other}')", b.keys));
            continue;
        }
        let id = actions.len() as i32 + 1;
        match unsafe { RegisterHotKey(None, id, mods, key) } {
            Ok(()) => {
                seen.push((mods.0, key, b.keys.clone()));
                actions.push(b.action.clone());
            }
            Err(_) => problems.push(format!("'{}' is already taken by another program", b.keys)),
        }
    }
    (actions, problems)
}
