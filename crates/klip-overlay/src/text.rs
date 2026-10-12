//! Every text the overlay brings along itself, in both languages.
//!
//! Titles and details of notices are not in here: the caller hands those over as
//! finished text. English is the default.

use crate::state::Lang;

/// One entry per text. The table below has one row per entry and one column per language.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum T {
    BufferActive,
    Recording,
    Paused,
    Ready,
    DragToMove,
    SaveClip,
    StartRecording,
    StopRecording,
    Screenshot,
    MicOn,
    MicOff,
    Recent,
    NoClips,
    OpenLibrary,
    Settings,
    ClosesOverlay,
    ClipSaved,
}

impl T {
    #[cfg(test)]
    pub const ALL: [T; 17] = [
        T::BufferActive,
        T::Recording,
        T::Paused,
        T::Ready,
        T::DragToMove,
        T::SaveClip,
        T::StartRecording,
        T::StopRecording,
        T::Screenshot,
        T::MicOn,
        T::MicOff,
        T::Recent,
        T::NoClips,
        T::OpenLibrary,
        T::Settings,
        T::ClosesOverlay,
        T::ClipSaved,
    ];
}

/// (English, German).
fn row(t: T) -> (&'static str, &'static str) {
    match t {
        T::BufferActive => ("Buffer active", "Puffer aktiv"),
        T::Recording => ("Recording", "Aufnahme läuft"),
        T::Paused => ("Paused", "Pausiert"),
        T::Ready => ("Ready", "Bereit"),
        T::DragToMove => ("Drag to move", "Ziehen zum Verschieben"),
        T::SaveClip => ("Save clip", "Clip speichern"),
        T::StartRecording => ("Start recording", "Aufnahme starten"),
        T::StopRecording => ("Stop recording", "Aufnahme stoppen"),
        T::Screenshot => ("Screenshot", "Screenshot"),
        T::MicOn => ("Microphone on", "Mikrofon an"),
        T::MicOff => ("Microphone off", "Mikrofon aus"),
        T::Recent => ("Recent", "Zuletzt"),
        T::NoClips => ("No clips yet", "Noch keine Clips"),
        T::OpenLibrary => ("Open library", "Bibliothek"),
        T::Settings => ("Settings", "Einstellungen"),
        T::ClosesOverlay => ("closes the overlay", "schließt das Overlay"),
        T::ClipSaved => ("Clip saved", "Clip gespeichert"),
    }
}

pub(crate) fn tr(lang: Lang, t: T) -> &'static str {
    let (en, de) = row(t);
    match lang {
        Lang::En => en,
        Lang::De => de,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_text_exists_in_both_languages() {
        for t in T::ALL {
            let (en, de) = (tr(Lang::En, t), tr(Lang::De, t));
            assert!(!en.is_empty() && !de.is_empty(), "{t:?}");
            assert!(en.is_ascii(), "{t:?}: the English text is plain ASCII");
            // No dashes used as punctuation, in neither language.
            for text in [en, de] {
                assert!(!text.contains('\u{2014}') && !text.contains('\u{2013}'), "{t:?}");
            }
        }
        assert_eq!(tr(Lang::default(), T::SaveClip), "Save clip");
        assert_eq!(tr(Lang::De, T::SaveClip), "Clip speichern");
    }
}
