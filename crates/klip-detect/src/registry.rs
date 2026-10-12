//! Maps an executable name to the detector for that game.

use crate::detector::Detector;
use crate::valorant::ValorantDetector;

/// Creates a fresh detector.
pub type DetectorFactory = fn() -> Box<dyn Detector>;

struct Entry {
    id: &'static str,
    exe_names: Vec<String>,
    factory: DetectorFactory,
}

/// Known detectors. The recorder only passes the file name of the foreground
/// game, the registry never looks at a process itself.
pub struct Registry {
    entries: Vec<Entry>,
}

impl Registry {
    /// Empty registry.
    pub fn new() -> Registry {
        Registry { entries: Vec::new() }
    }

    /// Registry with every detector that ships with Klip.
    pub fn builtin() -> Registry {
        let mut r = Registry::new();
        r.register("valorant", &["VALORANT-Win64-Shipping.exe"], || Box::new(ValorantDetector::new()));
        r
    }

    /// Adds a detector. A later registration for the same exe name wins.
    pub fn register(&mut self, id: &'static str, exe_names: &[&str], factory: DetectorFactory) {
        self.entries.push(Entry {
            id,
            exe_names: exe_names.iter().map(|s| s.to_ascii_lowercase()).collect(),
            factory,
        });
    }

    /// Identifier of the detector for an exe name or path, without creating it.
    pub fn id_for_exe(&self, exe: &str) -> Option<&'static str> {
        self.find(exe).map(|e| e.id)
    }

    /// Creates the detector for an exe name or full path. Matching ignores case
    /// and any directory part.
    pub fn for_exe(&self, exe: &str) -> Option<Box<dyn Detector>> {
        self.find(exe).map(|e| (e.factory)())
    }

    /// Identifiers of all registered detectors.
    pub fn ids(&self) -> Vec<&'static str> {
        self.entries.iter().map(|e| e.id).collect()
    }

    fn find(&self, exe: &str) -> Option<&Entry> {
        let name = exe.rsplit(['\\', '/']).next().unwrap_or(exe).to_ascii_lowercase();
        self.entries.iter().rev().find(|e| e.exe_names.iter().any(|n| *n == name))
    }
}

impl Default for Registry {
    fn default() -> Self {
        Registry::builtin()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detector::SourceKind;
    use crate::event::{Event, EventKind};
    use crate::frame::{RegionRequest, Sample};

    struct FakeGsi {
        queue: Vec<Event>,
    }

    impl Detector for FakeGsi {
        fn id(&self) -> &'static str {
            "fake_gsi"
        }
        fn display_name(&self) -> &'static str {
            "Fake GSI"
        }
        fn source(&self) -> SourceKind {
            SourceKind::External
        }
        fn supported_events(&self) -> &'static [EventKind] {
            &[EventKind::Kill]
        }
        fn regions(&self) -> Vec<RegionRequest> {
            Vec::new()
        }
        fn process(&mut self, _sample: &Sample<'_>) -> Vec<Event> {
            Vec::new()
        }
        fn poll(&mut self, _now_ms: u64) -> Vec<Event> {
            std::mem::take(&mut self.queue)
        }
        fn reset(&mut self) {
            self.queue.clear();
        }
    }

    #[test]
    fn builtin_finds_valorant_by_name_and_path() {
        let r = Registry::builtin();
        assert_eq!(r.id_for_exe("VALORANT-Win64-Shipping.exe"), Some("valorant"));
        assert_eq!(r.id_for_exe("valorant-win64-shipping.EXE"), Some("valorant"));
        let d = r.for_exe(r"C:\Riot Games\VALORANT\live\ShooterGame\Binaries\Win64\VALORANT-Win64-Shipping.exe");
        assert_eq!(d.unwrap().id(), "valorant");
        assert!(r.for_exe("notepad.exe").is_none());
        assert!(r.for_exe("VALORANT.exe").is_none());
    }

    #[test]
    fn external_detectors_fit_the_interface() {
        let mut r = Registry::new();
        r.register("fake_gsi", &["cs2.exe"], || {
            Box::new(FakeGsi { queue: vec![Event::new(EventKind::Kill, 1000, 1.0)] })
        });
        let mut d = r.for_exe("CS2.EXE").unwrap();
        assert_eq!(d.source(), SourceKind::External);
        assert!(d.regions().is_empty());
        assert_eq!(d.poll(2000).len(), 1);
        assert!(d.poll(3000).is_empty());
        assert_eq!(r.ids(), vec!["fake_gsi"]);
    }
}
