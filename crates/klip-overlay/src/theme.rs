//! Colors of the dark palette from the design note, version 2. No gradients, no glow, no shadows.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    /// Parses "#RRGGBB". Anything else gives `None`.
    pub fn parse(text: &str) -> Option<Rgb> {
        let hex = text.strip_prefix('#')?;
        if hex.len() != 6 || !hex.is_ascii() {
            return None;
        }
        let n = u32::from_str_radix(hex, 16).ok()?;
        Some(Rgb((n >> 16) as u8, (n >> 8) as u8, n as u8))
    }
}

pub const GROUND: Rgb = Rgb(0x0A, 0x0A, 0x0F);
pub const SURFACE_1: Rgb = Rgb(0x11, 0x11, 0x19);
pub const SURFACE_2: Rgb = Rgb(0x19, 0x19, 0x24);
pub const SURFACE_3: Rgb = Rgb(0x23, 0x23, 0x33);
pub const LINE: Rgb = Rgb(0x2A, 0x2A, 0x3C);
pub const TEXT: Rgb = Rgb(0xED, 0xED, 0xF7);
pub const TEXT_MUTED: Rgb = Rgb(0x90, 0x90, 0xAB);
pub const TEXT_FAINT: Rgb = Rgb(0x5B, 0x5B, 0x74);
/// The one accent, violet by default. The user can change it, so the state carries its
/// own copy.
pub const ACCENT: Rgb = Rgb(0x8B, 0x5C, 0xFF);
/// The second preset of the design.
pub const ACCENT_BLUE: Rgb = Rgb(0x3B, 0x82, 0xFF);
pub const ON_ACCENT: Rgb = Rgb(0xFF, 0xFF, 0xFF);
/// Only for "recording is running".
pub const RECORDING: Rgb = Rgb(0xFF, 0x3B, 0x4E);
/// Conflicts and the muted microphone.
pub const WARNING: Rgb = Rgb(0xFF, 0xB0, 0x20);
/// The microphone is on.
pub const OK: Rgb = Rgb(0x3D, 0xDC, 0x97);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_hex() {
        assert_eq!(Rgb::parse("#8B5CFF"), Some(ACCENT));
        assert_eq!(Rgb::parse("#8b5cff"), Some(ACCENT));
        assert_eq!(Rgb::parse("8B5CFF"), None);
        assert_eq!(Rgb::parse("#8B5CF"), None);
        assert_eq!(Rgb::parse("#GGGGGG"), None);
        assert_eq!(Rgb::parse("#äöü"), None);
    }
}
