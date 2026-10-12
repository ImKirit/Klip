//! The input track: a time trimmed ring in memory, clips cut from it, and the
//! `<name>.inputs.json` file next to a video.
//!
//! Times in the ring are 100 ns units on the QueryPerformanceCounter clock, the clock of
//! the recorder's video timestamps. Times in a clip are milliseconds from the clip start.

use crate::layout::{KeyId, MouseButton};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};

/// Version written into `.inputs.json`. Readers refuse anything newer.
pub const FORMAT_VERSION: u32 = 1;

/// 100 ns units per millisecond.
const MS: i64 = 10_000;

/// How long a wheel step stays visible in a snapshot. A wheel step has no duration of
/// its own, so the renderer needs some window in which it counts as "scrolling now".
const WHEEL_SHOW_MS: u32 = 150;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventKind {
    Key { id: KeyId, down: bool },
    Button { button: MouseButton, down: bool },
    /// One wheel report. 120 is one notch, positive is away from the user (or right).
    Wheel { delta: i16, horizontal: bool },
    /// Mouse travel summed over one window that starts at the event time. `dir` counts
    /// in steps of 45 degrees clockwise on screen: 0 right, 2 down, 4 left, 6 up.
    /// `amount` is the length of the summed travel in mouse counts.
    Move { dir: u8, amount: u16 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Event {
    pub t: i64,
    pub kind: EventKind,
}

/// The keys and buttons that are down at one moment, both sorted.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Held {
    #[serde(default)]
    pub keys: Vec<KeyId>,
    #[serde(default)]
    pub buttons: Vec<MouseButton>,
}

fn set<T: Ord + Copy>(list: &mut Vec<T>, item: T, on: bool) {
    match (list.binary_search(&item), on) {
        (Err(at), true) => list.insert(at, item),
        (Ok(at), false) => {
            list.remove(at);
        }
        _ => {}
    }
}

impl Held {
    pub fn apply(&mut self, kind: &EventKind) {
        match *kind {
            EventKind::Key { id, down } => set(&mut self.keys, id, down),
            EventKind::Button { button, down } => set(&mut self.buttons, button, down),
            EventKind::Wheel { .. } | EventKind::Move { .. } => {}
        }
    }

    pub fn key(&self, id: KeyId) -> bool {
        self.keys.binary_search(&id).is_ok()
    }

    pub fn button(&self, button: MouseButton) -> bool {
        self.buttons.binary_search(&button).is_ok()
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty() && self.buttons.is_empty()
    }
}

/// Ring of events that forgets everything older than `retention`.
///
/// Forgetting is not just dropping: a key that went down before the cut and is still
/// held would otherwise look released. So every dropped event is folded into `base`, the
/// held state at the start of what is kept.
#[derive(Debug)]
pub struct Track {
    events: VecDeque<Event>,
    base: Held,
    now: Held,
    retention: i64,
}

impl Track {
    /// `retention` in 100 ns units, normally the length of the replay buffer.
    pub fn new(retention: i64) -> Self {
        Track { events: VecDeque::new(), base: Held::default(), now: Held::default(), retention: retention.max(0) }
    }

    pub fn set_retention(&mut self, retention: i64) {
        self.retention = retention.max(0);
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// What is held after the newest event.
    pub fn held(&self) -> &Held {
        &self.now
    }

    pub fn push(&mut self, event: Event) {
        self.now.apply(&event.kind);
        // Movement windows are stamped with their start and written at their end, so an
        // event can arrive a little out of order. Walk back to keep the ring sorted.
        let mut at = self.events.len();
        while at > 0 && self.events[at - 1].t > event.t {
            at -= 1;
        }
        self.events.insert(at, event);
        if let Some(newest) = self.events.back().map(|e| e.t) {
            self.trim(newest);
        }
    }

    /// Drops everything older than `now - retention`. `push` does this on its own, call
    /// it from a timer only to free memory while no input arrives.
    pub fn trim(&mut self, now: i64) {
        let cutoff = now - self.retention;
        while self.events.front().is_some_and(|e| e.t < cutoff) {
            let old = self.events.pop_front().unwrap();
            self.base.apply(&old.kind);
        }
    }

    pub fn clear(&mut self) {
        self.events.clear();
        self.base = Held::default();
        self.now = Held::default();
    }

    /// Held state just before `t`: events at exactly `t` are not applied yet.
    pub fn state_at(&self, t: i64) -> Held {
        let mut held = self.base.clone();
        for e in self.events.iter().take_while(|e| e.t < t) {
            held.apply(&e.kind);
        }
        held
    }

    /// The events from `t0` to `t1` (both included) as a clip with times relative to
    /// `t0`, plus what was already held at `t0`.
    pub fn slice(&self, t0: i64, t1: i64, move_window: i64) -> InputClip {
        let events = self
            .events
            .iter()
            .filter(|e| e.t >= t0 && e.t <= t1)
            .map(|e| ClipEvent { t_ms: ((e.t - t0) / MS) as u32, kind: e.kind })
            .collect();
        InputClip {
            version: FORMAT_VERSION,
            duration_ms: ((t1 - t0).max(0) / MS) as u32,
            move_window_ms: (move_window / MS) as u32,
            held: self.state_at(t0),
            events,
        }
    }

    /// Presses per second over the `window` that ends at `t`: keys with `keys` true,
    /// mouse buttons otherwise. For the live widget.
    pub fn rate_at(&self, t: i64, window: i64, keys: bool) -> f32 {
        let hits = self.events.iter().filter(|e| e.t > t - window && e.t <= t && is_press(&e.kind, keys)).count();
        per_second(hits, window as f64 / 10_000_000.0)
    }
}

fn is_press(kind: &EventKind, keys: bool) -> bool {
    match kind {
        EventKind::Key { down, .. } => keys && *down,
        EventKind::Button { down, .. } => !keys && *down,
        _ => false,
    }
}

fn per_second(hits: usize, seconds: f64) -> f32 {
    if seconds <= 0.0 {
        return 0.0;
    }
    (hits as f64 / seconds) as f32
}

/// One event of a clip.
///
/// In the file this is a four element array `[t_ms, type, a, b]`, because a clip can
/// hold thousands of them and field names would triple the size:
/// `"k"` key (a scancode id, b 1 down or 0 up), `"b"` button (a index 0 left, 1 right,
/// 2 middle, 3 and 4 side, b 1 or 0), `"w"` wheel (a delta, b 1 if horizontal),
/// `"m"` movement (a direction 0 to 7, b amount).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "Wire", into = "Wire")]
pub struct ClipEvent {
    pub t_ms: u32,
    pub kind: EventKind,
}

type Wire = (u32, String, i32, i32);

impl From<ClipEvent> for Wire {
    fn from(e: ClipEvent) -> Wire {
        let (tag, a, b) = match e.kind {
            EventKind::Key { id, down } => ("k", id.0 as i32, down as i32),
            EventKind::Button { button, down } => ("b", button.index() as i32, down as i32),
            EventKind::Wheel { delta, horizontal } => ("w", delta as i32, horizontal as i32),
            EventKind::Move { dir, amount } => ("m", dir as i32, amount as i32),
        };
        (e.t_ms, tag.to_string(), a, b)
    }
}

impl TryFrom<Wire> for ClipEvent {
    type Error = String;

    fn try_from((t_ms, tag, a, b): Wire) -> Result<Self, String> {
        let bad = || format!("bad input event [{t_ms}, \"{tag}\", {a}, {b}]");
        let kind = match tag.as_str() {
            "k" => EventKind::Key { id: KeyId(u16::try_from(a).map_err(|_| bad())?), down: b != 0 },
            "b" => EventKind::Button {
                button: u8::try_from(a).ok().and_then(MouseButton::from_index).ok_or_else(bad)?,
                down: b != 0,
            },
            "w" => EventKind::Wheel { delta: i16::try_from(a).map_err(|_| bad())?, horizontal: b != 0 },
            "m" => EventKind::Move {
                dir: u8::try_from(a).ok().filter(|d| *d < 8).ok_or_else(bad)?,
                amount: u16::try_from(b).map_err(|_| bad())?,
            },
            _ => return Err(bad()),
        };
        Ok(ClipEvent { t_ms, kind })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Motion {
    pub dir: u8,
    pub amount: u16,
}

/// Everything a renderer needs to draw the overlay for one moment.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub held: Held,
    /// Sum of the vertical wheel steps of the last 150 ms, 0 when the wheel is still.
    pub wheel: i32,
    /// The movement window that covers this moment, if the mouse moved.
    pub motion: Option<Motion>,
}

/// The content of one `.inputs.json`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputClip {
    pub version: u32,
    pub duration_ms: u32,
    /// Length of one movement window, needed to know how long an `"m"` event lasts.
    pub move_window_ms: u32,
    /// Keys and buttons already down when the clip starts.
    pub held: Held,
    pub events: Vec<ClipEvent>,
}

impl InputClip {
    /// State at `t_ms` from the clip start. Events at exactly `t_ms` are included, so a
    /// press shows on the frame it happens on.
    pub fn state_at(&self, t_ms: u32) -> Snapshot {
        let mut snap = Snapshot { held: self.held.clone(), ..Default::default() };
        for e in self.events.iter().take_while(|e| e.t_ms <= t_ms) {
            snap.held.apply(&e.kind);
            match e.kind {
                EventKind::Wheel { delta, horizontal: false } if t_ms - e.t_ms < WHEEL_SHOW_MS => {
                    snap.wheel += delta as i32;
                }
                EventKind::Move { dir, amount } if t_ms - e.t_ms < self.move_window_ms => {
                    snap.motion = Some(Motion { dir, amount });
                }
                _ => {}
            }
        }
        snap
    }

    /// Key presses per second over the `window_ms` that end at `t_ms`.
    pub fn kps_at(&self, t_ms: u32, window_ms: u32) -> f32 {
        self.rate_at(t_ms, window_ms, true)
    }

    /// Mouse clicks per second over the `window_ms` that end at `t_ms`.
    pub fn cps_at(&self, t_ms: u32, window_ms: u32) -> f32 {
        self.rate_at(t_ms, window_ms, false)
    }

    fn rate_at(&self, t_ms: u32, window_ms: u32, keys: bool) -> f32 {
        let (t, w) = (t_ms as i64, window_ms as i64);
        let hits =
            self.events.iter().filter(|e| (e.t_ms as i64) > t - w && (e.t_ms as i64) <= t && is_press(&e.kind, keys));
        per_second(hits.count(), window_ms as f64 / 1000.0)
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("an input clip always serializes")
    }

    pub fn from_json(text: &str) -> Result<Self, String> {
        // Look at the version first: a newer file may not even parse as this struct, and
        // "newer than this build" is the message the user needs then.
        #[derive(Deserialize)]
        struct Probe {
            version: u32,
        }
        let probe: Probe = serde_json::from_str(text).map_err(|e| e.to_string())?;
        if probe.version > FORMAT_VERSION {
            return Err(format!("input track version {} is newer than this build ({FORMAT_VERSION})", probe.version));
        }
        serde_json::from_str(text).map_err(|e| e.to_string())
    }

    /// `clip.mp4` gives `clip.inputs.json`.
    pub fn path_for(video: &Path) -> PathBuf {
        video.with_extension("inputs.json")
    }

    pub fn write(&self, path: &Path) -> Result<(), String> {
        std::fs::write(path, self.to_json()).map_err(|e| format!("{}: {e}", path.display()))
    }

    pub fn read(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Self::from_json(&text).map_err(|e| format!("{}: {e}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEC: i64 = 10_000_000;
    const W: KeyId = KeyId(0x11);
    const A: KeyId = KeyId(0x1E);

    fn key(t: i64, id: KeyId, down: bool) -> Event {
        Event { t, kind: EventKind::Key { id, down } }
    }

    fn button(t: i64, button: MouseButton, down: bool) -> Event {
        Event { t, kind: EventKind::Button { button, down } }
    }

    /// W held from 1 s to 5 s, A tapped at 3 s, left click at 4 s.
    fn sample() -> Track {
        let mut track = Track::new(60 * SEC);
        for e in [
            key(SEC, W, true),
            key(3 * SEC, A, true),
            key(3 * SEC + SEC / 10, A, false),
            button(4 * SEC, MouseButton::Left, true),
            button(4 * SEC + SEC / 20, MouseButton::Left, false),
            key(5 * SEC, W, false),
        ] {
            track.push(e);
        }
        track
    }

    #[test]
    fn slice_carries_the_state_at_its_start() {
        let clip = sample().slice(2 * SEC, 6 * SEC, 50 * MS);
        // W went down a second before the clip starts and must show as held from frame 0.
        assert_eq!(clip.held.keys, vec![W]);
        assert!(clip.held.buttons.is_empty());
        assert_eq!(clip.duration_ms, 4000);
        assert_eq!(clip.events.len(), 5);
        assert_eq!(clip.events[0], ClipEvent { t_ms: 1000, kind: EventKind::Key { id: A, down: true } });
        assert_eq!(clip.events[4], ClipEvent { t_ms: 3000, kind: EventKind::Key { id: W, down: false } });

        let early = sample().slice(0, SEC / 2, 50 * MS);
        assert!(early.held.is_empty() && early.events.is_empty());
    }

    #[test]
    fn slice_includes_both_ends() {
        let clip = sample().slice(SEC, 5 * SEC, 50 * MS);
        // The press at exactly t0 is an event of the clip, not part of the start state.
        assert!(clip.held.is_empty());
        assert_eq!(clip.events.first().unwrap().t_ms, 0);
        assert_eq!(clip.events.last().unwrap().t_ms, 4000);
    }

    #[test]
    fn state_at_a_time() {
        let track = sample();
        assert!(track.state_at(SEC / 2).is_empty());
        assert_eq!(track.state_at(2 * SEC).keys, vec![W]);
        assert_eq!(track.state_at(3 * SEC + 1).keys, vec![W, A]);
        assert_eq!(track.state_at(4 * SEC + 1).buttons, vec![MouseButton::Left]);
        assert!(track.state_at(9 * SEC).is_empty());
        assert!(track.held().is_empty());

        let clip = track.slice(2 * SEC, 6 * SEC, 50 * MS);
        assert_eq!(clip.state_at(0).held.keys, vec![W]);
        assert_eq!(clip.state_at(999).held.keys, vec![W]);
        assert_eq!(clip.state_at(1000).held.keys, vec![W, A]);
        assert_eq!(clip.state_at(1100).held.keys, vec![W]);
        assert!(clip.state_at(2020).held.button(MouseButton::Left));
        assert!(!clip.state_at(2050).held.button(MouseButton::Left));
        assert!(clip.state_at(3000).held.is_empty());
    }

    #[test]
    fn snapshot_shows_wheel_and_motion_only_for_a_moment() {
        let mut track = Track::new(60 * SEC);
        track.push(Event { t: SEC, kind: EventKind::Wheel { delta: 120, horizontal: false } });
        track.push(Event { t: SEC + 10 * MS, kind: EventKind::Wheel { delta: 120, horizontal: false } });
        track.push(Event { t: 2 * SEC, kind: EventKind::Move { dir: 2, amount: 40 } });
        let clip = track.slice(0, 3 * SEC, 50 * MS);
        assert_eq!(clip.state_at(999).wheel, 0);
        assert_eq!(clip.state_at(1020).wheel, 240);
        assert_eq!(clip.state_at(1200).wheel, 0);
        assert_eq!(clip.state_at(1999).motion, None);
        assert_eq!(clip.state_at(2049).motion, Some(Motion { dir: 2, amount: 40 }));
        assert_eq!(clip.state_at(2050).motion, None);
    }

    #[test]
    fn trimming_by_time_keeps_what_is_still_held() {
        let mut track = Track::new(10 * SEC);
        track.push(key(SEC, W, true));
        track.push(key(2 * SEC, A, true));
        track.push(key(3 * SEC, A, false));
        assert_eq!(track.len(), 3);
        // 20 s later the three old events are outside the 10 s window.
        track.push(button(21 * SEC, MouseButton::Right, true));
        assert_eq!(track.len(), 1);
        // W was never released, so a clip from the trimmed ring still starts with W held.
        let clip = track.slice(15 * SEC, 22 * SEC, 50 * MS);
        assert_eq!(clip.held.keys, vec![W]);
        assert_eq!(clip.events.len(), 1);
        assert_eq!(track.held().keys, vec![W]);

        // The timer path trims without a new event.
        track.trim(40 * SEC);
        assert!(track.is_empty());
        assert_eq!(track.state_at(40 * SEC).buttons, vec![MouseButton::Right]);

        // A shorter retention takes effect on the next push.
        let mut track = sample();
        track.set_retention(SEC);
        track.push(key(6 * SEC, A, true));
        assert_eq!(track.len(), 2);
    }

    #[test]
    fn late_events_are_sorted_in() {
        let mut track = Track::new(60 * SEC);
        track.push(key(2 * SEC, W, true));
        track.push(Event { t: SEC, kind: EventKind::Move { dir: 0, amount: 5 } });
        track.push(key(3 * SEC, W, false));
        let times: Vec<u32> = track.slice(0, 4 * SEC, 50 * MS).events.iter().map(|e| e.t_ms).collect();
        assert_eq!(times, vec![1000, 2000, 3000]);
    }

    #[test]
    fn json_roundtrip_and_exact_format() {
        let mut track = sample();
        track.push(Event { t: 5 * SEC + SEC / 2, kind: EventKind::Wheel { delta: -120, horizontal: false } });
        track.push(Event { t: 5 * SEC + SEC / 2, kind: EventKind::Wheel { delta: 120, horizontal: true } });
        track.push(Event { t: 5 * SEC + SEC / 2, kind: EventKind::Move { dir: 7, amount: 312 } });
        track.push(button(5 * SEC + SEC / 2, MouseButton::Side2, true));
        let clip = track.slice(2 * SEC, 6 * SEC, 50 * MS);
        let text = clip.to_json();
        assert_eq!(InputClip::from_json(&text).unwrap(), clip);
        // The format is a contract with the TypeScript app, so pin it down literally.
        assert_eq!(
            text,
            concat!(
                r#"{"version":1,"duration_ms":4000,"move_window_ms":50,"held":{"keys":[17],"buttons":[]},"#,
                r#""events":[[1000,"k",30,1],[1100,"k",30,0],[2000,"b",0,1],[2050,"b",0,0],[3000,"k",17,0],"#,
                r#"[3500,"w",-120,0],[3500,"w",120,1],[3500,"m",7,312],[3500,"b",4,1]]}"#
            )
        );
    }

    #[test]
    fn file_roundtrip() {
        // Inside target/, so the test writes nothing outside the project.
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/klip-input-tests");
        std::fs::create_dir_all(&dir).unwrap();
        let path = InputClip::path_for(&dir.join("roundtrip.mp4"));
        assert_eq!(path.file_name().unwrap(), "roundtrip.inputs.json");
        let clip = sample().slice(2 * SEC, 6 * SEC, 50 * MS);
        clip.write(&path).unwrap();
        assert_eq!(InputClip::read(&path).unwrap(), clip);
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn reader_rejects_newer_versions_and_broken_events() {
        let newer = r#"{"version":2,"something":"else"}"#;
        assert!(InputClip::from_json(newer).unwrap_err().contains("newer"));
        let head = r#"{"version":1,"duration_ms":10,"move_window_ms":50,"held":{"keys":[],"buttons":[]},"events":"#;
        for bad in [r#"[[0,"x",1,1]]"#, r#"[[0,"b",9,1]]"#, r#"[[0,"m",8,1]]"#, r#"[[0,"k",70000,1]]"#] {
            assert!(InputClip::from_json(&format!("{head}{bad}}}")).is_err(), "{bad}");
        }
        assert!(InputClip::from_json(&format!("{head}[[0,\"k\",17,1]]}}")).is_ok());
    }

    #[test]
    fn kps_and_cps() {
        let mut track = Track::new(60 * SEC);
        // Eight key presses and three clicks inside one second, releases in between.
        for i in 0..8 {
            track.push(key(SEC + i * SEC / 10, W, true));
            track.push(key(SEC + i * SEC / 10 + SEC / 50, W, false));
        }
        for i in 0..3 {
            track.push(button(SEC + i * SEC / 5, MouseButton::Left, true));
            track.push(button(SEC + i * SEC / 5 + SEC / 50, MouseButton::Left, false));
        }
        let clip = track.slice(0, 4 * SEC, 50 * MS);
        assert_eq!(clip.kps_at(1999, 1000), 8.0);
        assert_eq!(clip.cps_at(1999, 1000), 3.0);
        // Half a second window holds the last four presses (at 1.4 to 1.7 s), as a rate.
        assert_eq!(clip.kps_at(1800, 500), 8.0);
        assert_eq!(clip.kps_at(1250, 500), 6.0);
        // Before and long after: nothing. Releases never count.
        assert_eq!(clip.kps_at(900, 1000), 0.0);
        assert_eq!(clip.kps_at(3500, 1000), 0.0);
        assert_eq!(clip.kps_at(1000, 0), 0.0);
        // The live path on the ring agrees with the clip.
        assert_eq!(track.rate_at(2 * SEC - 1, SEC, true), 8.0);
        assert_eq!(track.rate_at(2 * SEC - 1, SEC, false), 3.0);
    }
}
