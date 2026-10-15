//! DirectWrite fonts with a fallback chain.
//!
//! The design asks for Archivo and JetBrains Mono. They are
//! not Windows fonts, so each role is looked up in three places: the font files the
//! caller hands over (a private collection, nothing is installed), then the fonts
//! installed on the system, then Segoe UI or Consolas. Nothing is ever fetched from the
//! network.

use std::path::{Path, PathBuf};
use windows::core::{w, Interface, BOOL, HSTRING, PCWSTR};
use windows::Win32::Graphics::DirectWrite::{
    DWriteCreateFactory, IDWriteFactory5, IDWriteFontCollection, IDWriteFontFile, IDWriteInMemoryFontFileLoader,
    IDWriteTextLayout, IDWriteTextLayout1, DWRITE_FACTORY_TYPE_SHARED,
    DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT, DWRITE_TEXT_METRICS, DWRITE_TEXT_RANGE, DWRITE_TRIMMING,
    DWRITE_TRIMMING_GRANULARITY_CHARACTER, DWRITE_WORD_WRAPPING_NO_WRAP,
};

/// Which family each role ended up with. For logs and for the honest report.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FontReport {
    pub display: String,
    pub ui: String,
    pub mono: String,
    /// Font files that DirectWrite accepted.
    pub files_loaded: usize,
    /// Font files that were handed over but could not be loaded.
    pub files_failed: usize,
    /// Why the first failed file failed, empty when none did.
    pub first_error: String,
    /// Family names found in the handed over files.
    pub families: Vec<String>,
}

/// The font files directly inside `dir` (ttf, otf, ttc, woff, woff2), sorted. Not
/// recursive. A missing folder gives an empty list.
pub fn font_files_in(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| ["ttf", "otf", "ttc", "woff", "woff2"].contains(&e.to_ascii_lowercase().as_str()))
        })
        .collect();
    files.sort();
    files
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Role {
    /// Wordmark and headings.
    Display,
    /// Interface text.
    Ui,
    /// Time codes, numbers, key caps.
    Mono,
}

struct Face {
    /// Family name, nul terminated UTF-16.
    name: Vec<u16>,
    /// `None` is the system collection.
    collection: Option<IDWriteFontCollection>,
    label: String,
}

pub(crate) struct Fonts {
    factory: IDWriteFactory5,
    display: Face,
    ui: Face,
    mono: Face,
    report: FontReport,
    /// Holds unpacked WOFF data. Registered with the factory, unregistered on drop.
    memory: Option<IDWriteInMemoryFontFileLoader>,
}

impl Drop for Fonts {
    fn drop(&mut self) {
        if let Some(loader) = &self.memory {
            unsafe {
                let _ = self.factory.UnregisterFontFileLoader(loader);
            }
        }
    }
}

/// Unpacks a WOFF or WOFF2 container and hands the plain font to the memory loader,
/// which keeps its own copy of the bytes.
unsafe fn unpack(
    factory: &IDWriteFactory5,
    loader: &IDWriteInMemoryFontFileLoader,
    bytes: &[u8],
) -> windows::core::Result<IDWriteFontFile> {
    let data = bytes.as_ptr() as *const core::ffi::c_void;
    let kind = factory.AnalyzeContainerType(data, bytes.len() as u32);
    let stream = factory.UnpackFontFile(kind, data, bytes.len() as u32)?;
    let size = stream.GetFileSize()?;
    let (mut start, mut context) = (std::ptr::null_mut(), std::ptr::null_mut());
    stream.ReadFileFragment(&mut start, 0, size, &mut context)?;
    let file = loader.CreateInMemoryFontFileReference(factory, start, size as u32, None);
    stream.ReleaseFileFragment(context);
    file
}

/// One laid out line of text.
pub(crate) struct Text {
    pub layout: Option<IDWriteTextLayout>,
    pub w: f32,
    pub h: f32,
}

fn has_family(collection: &IDWriteFontCollection, name: &str) -> bool {
    let (mut index, mut exists) = (0u32, BOOL(0));
    unsafe { collection.FindFamilyName(&HSTRING::from(name), &mut index, &mut exists).is_ok() && exists.as_bool() }
}

impl Fonts {
    pub fn new(files: &[PathBuf]) -> Result<Fonts, String> {
        unsafe {
            let factory: IDWriteFactory5 =
                DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).map_err(|e| format!("DirectWrite: {e}"))?;
            let mut report = FontReport::default();

            let mut memory: Option<IDWriteInMemoryFontFileLoader> = None;
            let mut custom: Option<IDWriteFontCollection> = None;
            if !files.is_empty() {
                if let Ok(builder) = factory.CreateFontSetBuilder() {
                    for path in files {
                        // DirectWrite wants a plain absolute path: no "..", no "\\?\".
                        let full = std::fs::canonicalize(path).unwrap_or_else(|_| path.clone());
                        let text = full.to_string_lossy();
                        let plain = text.strip_prefix(r"\\?\").unwrap_or(&text);
                        let packed = full
                            .extension()
                            .and_then(|e| e.to_str())
                            .is_some_and(|e| e.eq_ignore_ascii_case("woff") || e.eq_ignore_ascii_case("woff2"));
                        let file = if packed {
                            // The font set builder does not read WOFF containers from a
                            // path, they have to be unpacked into memory first.
                            if memory.is_none() {
                                memory = factory
                                    .CreateInMemoryFontFileLoader()
                                    .and_then(|l| factory.RegisterFontFileLoader(&l).map(|_| l))
                                    .ok();
                            }
                            match (&memory, std::fs::read(&full)) {
                                (Some(loader), Ok(bytes)) => unpack(&factory, loader, &bytes),
                                (_, Err(e)) => Err(windows::core::Error::from(e)),
                                (None, _) => Err(windows::core::Error::empty()),
                            }
                        } else {
                            factory.CreateFontFileReference(&HSTRING::from(plain), None)
                        };
                        match file.and_then(|file| builder.AddFontFile(&file)) {
                            Ok(()) => report.files_loaded += 1,
                            Err(e) => {
                                report.files_failed += 1;
                                if report.first_error.is_empty() {
                                    report.first_error = format!("{plain}: {e}");
                                }
                            }
                        }
                    }
                    if report.files_loaded > 0 {
                        custom = builder
                            .CreateFontSet()
                            .and_then(|set| factory.CreateFontCollectionFromFontSet(&set))
                            .and_then(|c| c.cast::<IDWriteFontCollection>())
                            .ok();
                    }
                }
            }

            if let Some(c) = &custom {
                for i in 0..c.GetFontFamilyCount() {
                    let name = c.GetFontFamily(i).and_then(|f| f.GetFamilyNames()).and_then(|names| {
                        let mut buffer = vec![0u16; names.GetStringLength(0)? as usize + 1];
                        names.GetString(0, &mut buffer)?;
                        Ok(String::from_utf16_lossy(&buffer[..buffer.len() - 1]))
                    });
                    report.families.extend(name.ok());
                }
            }
            let mut system1 = None;
            let _ = factory.GetSystemFontCollection(false, &mut system1, false);
            let system: Option<IDWriteFontCollection> = system1.and_then(|c| c.cast().ok());

            let families = report.families.clone();
            let pick = |wanted: &str, fallback: &str| -> Face {
                let face = |name: &str, collection: Option<IDWriteFontCollection>, from: &str| Face {
                    name: name.encode_utf16().chain(std::iter::once(0)).collect(),
                    collection,
                    label: format!("{name} ({from})"),
                };
                if let Some(c) = custom.as_ref().filter(|c| has_family(c, wanted)) {
                    return face(wanted, Some(c.clone()), "font file");
                }
                // Static cuts often come out under a longer family name: "Archivo SemiBold",
                // "Bricolage Grotesque 96pt". Accept that as the family at a word boundary,
                // but never a different width such as "Archivo Condensed".
                let longer = |f: &&String| {
                    f.strip_prefix(wanted).is_some_and(|rest| {
                        rest.starts_with(' ') && !["Condensed", "Expanded", "Narrow", "Black Condensed"].iter().any(|w| rest[1..].starts_with(w))
                    })
                };
                if let (Some(c), Some(long)) = (custom.as_ref(), families.iter().find(longer)) {
                    return face(long, Some(c.clone()), "font file");
                }
                if system.as_ref().is_some_and(|c| has_family(c, wanted)) {
                    return face(wanted, None, "installed");
                }
                face(fallback, None, "fallback")
            };
            let display = pick("Archivo", "Segoe UI");
            let ui = pick("Archivo", "Segoe UI");
            let mono = pick("JetBrains Mono", "Consolas");
            report.display = display.label.clone();
            report.ui = ui.label.clone();
            report.mono = mono.label.clone();
            Ok(Fonts { factory, display, ui, mono, report, memory })
        }
    }

    pub fn report(&self) -> &FontReport {
        &self.report
    }

    /// Lays out one line. With `max_w` the text is cut with an ellipsis at that width.
    /// `tracking` is extra space after every character in pixels.
    pub fn text(&self, text: &str, role: Role, weight: i32, size: f32, max_w: Option<f32>, tracking: f32) -> Text {
        let face = match role {
            Role::Display => &self.display,
            Role::Ui => &self.ui,
            Role::Mono => &self.mono,
        };
        let build = || -> windows::core::Result<Text> {
            unsafe {
                let format = self.factory.CreateTextFormat(
                    PCWSTR(face.name.as_ptr()),
                    face.collection.as_ref(),
                    DWRITE_FONT_WEIGHT(weight),
                    DWRITE_FONT_STYLE_NORMAL,
                    DWRITE_FONT_STRETCH_NORMAL,
                    size.max(1.0),
                    w!("en-us"),
                )?;
                format.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
                if max_w.is_some() {
                    let sign = self.factory.CreateEllipsisTrimmingSign(&format)?;
                    let trimming = DWRITE_TRIMMING {
                        granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER,
                        delimiter: 0,
                        delimiterCount: 0,
                    };
                    format.SetTrimming(&trimming, &sign)?;
                }
                let wide: Vec<u16> = text.encode_utf16().collect();
                let layout = self.factory.CreateTextLayout(&wide, &format, max_w.unwrap_or(8192.0).max(1.0), 8192.0)?;
                let spaced = tracking > 0.0 && !wide.is_empty();
                if spaced {
                    let range = DWRITE_TEXT_RANGE { startPosition: 0, length: wide.len() as u32 };
                    layout.cast::<IDWriteTextLayout1>()?.SetCharacterSpacing(0.0, tracking, 0.0, range)?;
                }
                let mut m = DWRITE_TEXT_METRICS::default();
                layout.GetMetrics(&mut m)?;
                // The space after the last character is not part of the visible width.
                let w = if spaced { (m.width - tracking).max(0.0) } else { m.width };
                Ok(Text { layout: Some(layout), w: w.ceil(), h: m.height.ceil() })
            }
        };
        build().unwrap_or(Text { layout: None, w: 0.0, h: 0.0 })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn falls_back_without_font_files() {
        let fonts = Fonts::new(&[]).unwrap();
        let r = fonts.report();
        assert_eq!(r.files_loaded, 0);
        // Whatever is installed, every role resolves to some family.
        assert!(!r.display.is_empty() && !r.ui.is_empty() && !r.mono.is_empty());
        let t = fonts.text("Clip saved", Role::Ui, 600, 14.0, None, 0.0);
        assert!(t.w > 40.0 && t.h > 10.0);
    }

    #[test]
    fn a_missing_file_is_counted_not_fatal() {
        let fonts = Fonts::new(&[PathBuf::from(r"C:\does\not\exist\nothing.ttf")]).unwrap();
        assert_eq!(fonts.report().files_loaded, 0);
        assert!(fonts.report().ui.ends_with("(fallback)") || fonts.report().ui.ends_with("(installed)"));
    }

    #[test]
    fn trimming_keeps_text_inside_the_width() {
        let fonts = Fonts::new(&[]).unwrap();
        let long = "Ein sehr langer Cliptitel, der niemals in die Zeile passt";
        assert!(fonts.text(long, Role::Ui, 400, 13.0, None, 0.0).w > 200.0);
        assert!(fonts.text(long, Role::Ui, 400, 13.0, Some(120.0), 0.0).w <= 120.0);
    }

    #[test]
    fn tracking_widens_the_line() {
        let fonts = Fonts::new(&[]).unwrap();
        let plain = fonts.text("DRAG TO MOVE", Role::Mono, 500, 10.0, None, 0.0);
        let spaced = fonts.text("DRAG TO MOVE", Role::Mono, 500, 10.0, None, 1.2);
        // Eleven gaps of 1.2 pixels, the one after the last letter does not count.
        assert!(spaced.w > plain.w + 10.0 && spaced.w < plain.w + 16.0, "{} {}", plain.w, spaced.w);
    }

    #[test]
    fn font_files_in_a_missing_folder() {
        assert!(font_files_in(Path::new(r"C:\does\not\exist")).is_empty());
    }
}
