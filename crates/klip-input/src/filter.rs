//! The one gate between raw input and the track.
//!
//! Nothing reaches [`Track`] except through [`Processor::handle`], and that function is
//! where the keylogger rules are enforced. It is plain code without any Windows call, so
//! the tests can feed it every case directly.

use crate::layout::{KeyId, MouseButton, MouseConfig, OverlayLayout};
use crate::track::{Event, EventKind, Held, InputClip, Track};

/// Length of one movement window in 100 ns units (50 ms). Shorter windows would let the
/// track describe the mouse path more finely than an overlay arrow ever needs.
pub const MOVE_WINDOW: i64 = 500_000;

/// What the capture thread reports. Deliberately poorer than a raw input record: there
/// is no virtual key and no character in it, so text cannot get past this type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RawEvent {
    Key { make: u16, e0: bool, e1: bool, down: bool },
    Button { button: MouseButton, down: bool },
    Wheel { delta: i16, horizontal: bool },
    /// Relative travel since the last report. Absolute pointers are never reported.
    Move { dx: i32, dy: i32 },
}

/// The keys and mouse parts that may be stored. Built from a layout and from nothing
/// else, so the track can never hold a key the overlay does not show.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Allowlist {
    keys: Vec<KeyId>,
    mouse: MouseConfig,
}

impl Allowlist {
    /// Allows nothing.
    pub fn none() -> Self {
        Self::default()
    }

    pub fn from_layout(layout: &OverlayLayout) -> Self {
        let mut keys: Vec<KeyId> = layout.visible_keys().iter().map(|k| k.id).collect();
        keys.sort_unstable();
        keys.dedup();
        Allowlist { keys, mouse: layout.mouse }
    }

    pub fn key(&self, id: KeyId) -> bool {
        self.keys.binary_search(&id).is_ok()
    }

    pub fn mouse(&self) -> MouseConfig {
        self.mouse
    }

    /// False when no key is in scope, the keyboard then does not need to be listened to.
    pub fn needs_keyboard(&self) -> bool {
        !self.keys.is_empty()
    }

    pub fn needs_mouse(&self) -> bool {
        self.mouse.any()
    }
}

/// Totals only. There is on purpose no count per key and no record of what was dropped:
/// a dropped event leaves nothing behind but `discarded` going up by one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// Every event handed to the processor.
    pub received: u64,
    /// Events written to the track (a movement window counts once).
    pub stored: u64,
    /// Events rejected by a rule: capture not active, paused, or not in the layout.
    pub discarded: u64,
    /// Allowed events that carry no news: auto repeat of a held key, a release without
    /// a press, movement reports folded into a window.
    pub folded: u64,
}

struct Window {
    start: i64,
    dx: i64,
    dy: i64,
}

pub struct Processor {
    allow: Allowlist,
    game_active: bool,
    paused: bool,
    track: Track,
    stats: Stats,
    window: Option<Window>,
}

impl Processor {
    /// Starts with capture off: nothing is stored until `set_game_active(true, ..)`.
    pub fn new(allow: Allowlist, retention: i64) -> Self {
        Processor {
            allow,
            game_active: false,
            paused: false,
            track: Track::new(retention),
            stats: Stats::default(),
            window: None,
        }
    }

    /// True only while a game is in the foreground and capture is not paused.
    pub fn capturing(&self) -> bool {
        self.game_active && !self.paused
    }

    pub fn allowlist(&self) -> &Allowlist {
        &self.allow
    }

    pub fn stats(&self) -> Stats {
        self.stats
    }

    pub fn track(&self) -> &Track {
        &self.track
    }

    pub fn set_retention(&mut self, retention: i64) {
        self.track.set_retention(retention);
    }

    /// The "a recognised game is in the foreground" switch, set from outside.
    pub fn set_game_active(&mut self, active: bool, t: i64) {
        self.switch(t, |p| p.game_active = active);
    }

    pub fn set_paused(&mut self, paused: bool, t: i64) {
        self.switch(t, |p| p.paused = paused);
    }

    pub fn set_allowlist(&mut self, allow: Allowlist, t: i64) {
        self.release_all(t);
        self.allow = allow;
    }

    fn switch(&mut self, t: i64, change: impl FnOnce(&mut Self)) {
        let was = self.capturing();
        change(self);
        if was && !self.capturing() {
            self.release_all(t);
        }
    }

    /// Ends every held key and button at `t`. Once capture stops, the real releases are
    /// discarded like everything else, and without this the track would show those keys
    /// as held for the rest of the buffer.
    fn release_all(&mut self, t: i64) {
        self.flush_motion();
        let Held { keys, buttons } = self.track.held().clone();
        for id in keys {
            self.track.push(Event { t, kind: EventKind::Key { id, down: false } });
        }
        for button in buttons {
            self.track.push(Event { t, kind: EventKind::Button { button, down: false } });
        }
    }

    /// The only way into the track. `t` is the event time in 100 ns units.
    pub fn handle(&mut self, t: i64, event: RawEvent) {
        self.stats.received += 1;
        // Rule: nothing outside a game, nothing while paused. This comes first, so in
        // that state not even the scope check looks at the key.
        if !self.capturing() {
            self.stats.discarded += 1;
            return;
        }
        let kind = match event {
            RawEvent::Key { make, e0, e1, down } => {
                // Rule: only keys of the layout. Anything else ends here, uncounted per
                // key and unlogged.
                let Some(id) = KeyId::from_scancode(make, e0, e1).filter(|id| self.allow.key(*id)) else {
                    self.stats.discarded += 1;
                    return;
                };
                if self.track.held().key(id) == down {
                    self.stats.folded += 1;
                    return;
                }
                EventKind::Key { id, down }
            }
            RawEvent::Button { button, down } => {
                if !self.allow.mouse.button(button) {
                    self.stats.discarded += 1;
                    return;
                }
                if self.track.held().button(button) == down {
                    self.stats.folded += 1;
                    return;
                }
                EventKind::Button { button, down }
            }
            RawEvent::Wheel { delta, horizontal } => {
                if !self.allow.mouse.wheel {
                    self.stats.discarded += 1;
                    return;
                }
                EventKind::Wheel { delta, horizontal }
            }
            RawEvent::Move { dx, dy } => {
                if !self.allow.mouse.movement {
                    self.stats.discarded += 1;
                    return;
                }
                self.add_motion(t, dx, dy);
                return;
            }
        };
        self.track.push(Event { t, kind });
        self.stats.stored += 1;
    }

    /// Rule: movement is kept as a sum per window, never as single reports and never as
    /// a position.
    fn add_motion(&mut self, t: i64, dx: i32, dy: i32) {
        if self.window.as_ref().is_some_and(|w| t - w.start >= MOVE_WINDOW) {
            self.flush_motion();
        }
        let w = self.window.get_or_insert(Window { start: t, dx: 0, dy: 0 });
        w.dx += dx as i64;
        w.dy += dy as i64;
        self.stats.folded += 1;
    }

    /// Writes the open movement window, if any. Called when the next window starts and
    /// before anything reads the track, so a reader never misses the newest movement.
    pub fn flush_motion(&mut self) {
        let Some(w) = self.window.take() else { return };
        let (dx, dy) = (w.dx as f64, w.dy as f64);
        let amount = (dx * dx + dy * dy).sqrt().round();
        if amount < 1.0 {
            return;
        }
        // Eight sectors of 45 degrees. Raw input counts y downwards, so the angle runs
        // clockwise on screen.
        let dir = ((dy.atan2(dx) / std::f64::consts::FRAC_PI_4).round() as i32).rem_euclid(8) as u8;
        self.track.push(Event { t: w.start, kind: EventKind::Move { dir, amount: amount.min(u16::MAX as f64) as u16 } });
        self.stats.stored += 1;
    }

    /// Events from `t0` to `t1` as a clip, with the keys held at `t0`.
    pub fn slice(&mut self, t0: i64, t1: i64) -> InputClip {
        self.flush_motion();
        self.track.slice(t0, t1, MOVE_WINDOW)
    }

    /// Frees memory while no input arrives. `handle` trims on its own.
    pub fn trim(&mut self, now: i64) {
        self.track.trim(now);
    }

    /// Forgets the whole track, for "delete what was captured".
    pub fn clear(&mut self) {
        self.window = None;
        self.track.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{LayoutKind, Preset, Scope};

    const SEC: i64 = 10_000_000;
    const MS: i64 = 10_000;
    const W: u16 = 0x11;
    const ENTER: u16 = 0x1C;

    fn key(make: u16, down: bool) -> RawEvent {
        RawEvent::Key { make, e0: false, e1: false, down }
    }

    fn left_hand() -> OverlayLayout {
        OverlayLayout::preset(LayoutKind::IsoQwertz, Preset::LeftHandShooter)
    }

    fn running(layout: &OverlayLayout) -> Processor {
        let mut p = Processor::new(Allowlist::from_layout(layout), 60 * SEC);
        p.set_game_active(true, 0);
        p
    }

    fn all(p: &mut Processor) -> InputClip {
        p.slice(0, 1000 * SEC)
    }

    #[test]
    fn allowlist_is_exactly_the_visible_keys() {
        let layout = left_hand();
        let allow = Allowlist::from_layout(&layout);
        let visible: Vec<KeyId> = layout.visible_keys().iter().map(|k| k.id).collect();
        for code in 0..=u16::MAX {
            assert_eq!(allow.key(KeyId(code)), visible.contains(&KeyId(code)), "scancode id {code:#x}");
        }
        assert!(!Allowlist::none().needs_keyboard() && !Allowlist::none().needs_mouse());
    }

    #[test]
    fn key_outside_the_scope_is_never_stored() {
        let layout = left_hand();
        let allow = Allowlist::from_layout(&layout);
        let mut p = running(&layout);
        // Every scancode there is, plain, E0 and E1, pressed and released.
        let mut sent = 0;
        for make in 0..=0x1FFu16 {
            for (e0, e1) in [(false, false), (true, false), (false, true)] {
                for down in [true, false] {
                    p.handle(SEC + sent, RawEvent::Key { make, e0, e1, down });
                    sent += 1;
                }
            }
        }
        let clip = all(&mut p);
        assert!(!clip.events.is_empty());
        for e in &clip.events {
            match e.kind {
                EventKind::Key { id, .. } => assert!(allow.key(id), "{id:?} leaked into the track"),
                other => panic!("unexpected {other:?}"),
            }
        }
        // 26 keys of the template, press and release each.
        assert_eq!(clip.events.len(), 52);
        assert_eq!(p.stats(), Stats { received: sent as u64, stored: 52, discarded: sent as u64 - 52, folded: 0 });
    }

    #[test]
    fn a_dropped_key_leaves_no_trace() {
        let mut p = running(&left_hand());
        fn dump(p: &mut Processor) -> String {
            let clip = all(p);
            format!("{:?} {:?} {:?}", p.track(), p.allowlist(), clip)
        }
        let before = dump(&mut p);
        p.handle(SEC, key(ENTER, true));
        p.handle(SEC + 1, key(ENTER, false));
        // The whole observable state is the same as before, except for the two totals.
        assert_eq!(dump(&mut p), before);
        assert_eq!(p.stats(), Stats { received: 2, stored: 0, discarded: 2, folded: 0 });
    }

    #[test]
    fn right_ctrl_is_not_left_ctrl() {
        // Same make code, told apart only by the E0 flag. The template has left Ctrl only.
        let mut p = running(&left_hand());
        p.handle(SEC, RawEvent::Key { make: 0x1D, e0: true, e1: false, down: true });
        assert!(all(&mut p).events.is_empty());
        p.handle(SEC, RawEvent::Key { make: 0x1D, e0: false, e1: false, down: true });
        assert_eq!(all(&mut p).events.len(), 1);
    }

    #[test]
    fn inactive_stores_nothing() {
        let layout = left_hand();
        let mut p = Processor::new(Allowlist::from_layout(&layout), 60 * SEC);
        assert!(!p.capturing());
        p.handle(SEC, key(W, true));
        p.handle(SEC, RawEvent::Button { button: MouseButton::Left, down: true });
        p.handle(SEC, RawEvent::Wheel { delta: 120, horizontal: false });
        p.handle(SEC, RawEvent::Move { dx: 50, dy: 0 });
        assert!(p.track().is_empty() && p.track().held().is_empty());
        assert_eq!(p.stats(), Stats { received: 4, stored: 0, discarded: 4, folded: 0 });

        p.set_game_active(true, 2 * SEC);
        p.handle(2 * SEC, key(W, true));
        assert_eq!(p.track().len(), 1);
        p.set_game_active(false, 3 * SEC);
        p.handle(4 * SEC, key(0x1E, true));
        assert_eq!(p.stats().stored, 1);
    }

    #[test]
    fn pause_stores_nothing() {
        let mut p = running(&left_hand());
        p.set_paused(true, SEC);
        assert!(!p.capturing());
        p.handle(2 * SEC, key(W, true));
        p.handle(2 * SEC, RawEvent::Button { button: MouseButton::Left, down: true });
        assert!(p.track().is_empty());
        assert_eq!(p.stats().discarded, 2);
        p.set_paused(false, 3 * SEC);
        p.handle(3 * SEC, key(W, true));
        assert_eq!(p.track().len(), 1);
        // Paused and no game: unpausing alone must not start capture.
        p.set_game_active(false, 4 * SEC);
        p.set_paused(true, 4 * SEC);
        p.set_paused(false, 5 * SEC);
        assert!(!p.capturing());
    }

    #[test]
    fn stopping_releases_held_keys_in_the_track() {
        let mut p = running(&left_hand());
        p.handle(SEC, key(W, true));
        p.handle(SEC, RawEvent::Button { button: MouseButton::Right, down: true });
        p.set_paused(true, 2 * SEC);
        // The real release arrives while paused and is discarded.
        p.handle(3 * SEC, key(W, false));
        let clip = all(&mut p);
        assert!(clip.state_at(1500).held.key(KeyId(W)));
        assert!(clip.state_at(2000).held.is_empty());
        // A release without a press after resuming is not news.
        p.set_paused(false, 4 * SEC);
        p.handle(4 * SEC, key(W, false));
        assert_eq!(all(&mut p).events.len(), 4);
    }

    #[test]
    fn auto_repeat_is_stored_once() {
        let mut p = running(&left_hand());
        for i in 0..30 {
            p.handle(SEC + i * 33 * MS, key(W, true));
        }
        p.handle(3 * SEC, key(W, false));
        assert_eq!(all(&mut p).events.len(), 2);
        assert_eq!(p.stats().folded, 29);
    }

    #[test]
    fn mouse_parts_follow_the_layout() {
        let mut layout = left_hand();
        layout.mouse = MouseConfig { left: true, ..Default::default() };
        let mut p = running(&layout);
        p.handle(SEC, RawEvent::Button { button: MouseButton::Left, down: true });
        p.handle(SEC, RawEvent::Button { button: MouseButton::Side1, down: true });
        p.handle(SEC, RawEvent::Wheel { delta: 120, horizontal: false });
        p.handle(SEC, RawEvent::Move { dx: 9, dy: 9 });
        let clip = all(&mut p);
        assert_eq!(clip.events.len(), 1);
        assert_eq!(clip.events[0].kind, EventKind::Button { button: MouseButton::Left, down: true });
        assert_eq!(p.stats().discarded, 3);
    }

    #[test]
    fn movement_is_one_coarse_event_per_window() {
        let mut layout = left_hand();
        layout.mouse.movement = true;
        let mut p = running(&layout);
        // 1000 reports per second for 200 ms, straight to the right, then 100 ms down.
        for i in 0..200 {
            p.handle(SEC + i * MS, RawEvent::Move { dx: 3, dy: 0 });
        }
        for i in 200..300 {
            p.handle(SEC + i * MS, RawEvent::Move { dx: 0, dy: 2 });
        }
        let clip = all(&mut p);
        assert_eq!(clip.events.len(), 6);
        assert_eq!(clip.move_window_ms, 50);
        for (n, e) in clip.events.iter().enumerate() {
            assert_eq!(e.t_ms, 1000 + n as u32 * 50);
            let expected = if n < 4 { EventKind::Move { dir: 0, amount: 150 } } else { EventKind::Move { dir: 2, amount: 100 } };
            assert_eq!(e.kind, expected);
        }
        assert_eq!(p.stats(), Stats { received: 300, stored: 6, discarded: 0, folded: 300 });
    }

    #[test]
    fn movement_directions() {
        let mut layout = left_hand();
        layout.mouse.movement = true;
        let cases = [(10, 0, 0), (10, 10, 1), (0, 10, 2), (-10, 10, 3), (-10, 0, 4), (-10, -10, 5), (0, -10, 6), (10, -10, 7), (10, 3, 0)];
        for (dx, dy, dir) in cases {
            let mut p = running(&layout);
            p.handle(SEC, RawEvent::Move { dx, dy });
            match all(&mut p).events[0].kind {
                EventKind::Move { dir: got, .. } => assert_eq!(got, dir, "({dx}, {dy})"),
                other => panic!("unexpected {other:?}"),
            }
        }
        // Travel that cancels out is no movement.
        let mut p = running(&layout);
        p.handle(SEC, RawEvent::Move { dx: 5, dy: 0 });
        p.handle(SEC + MS, RawEvent::Move { dx: -5, dy: 0 });
        assert!(all(&mut p).events.is_empty());
    }

    #[test]
    fn changing_the_layout_narrows_what_is_stored() {
        let mut p = running(&left_hand());
        p.handle(SEC, key(0x02, true));
        let wasd = OverlayLayout::new("wasd", LayoutKind::IsoQwertz, Scope::Preset { preset: Preset::Wasd });
        p.set_allowlist(Allowlist::from_layout(&wasd), 2 * SEC);
        // The key that is no longer in scope was released by the switch and stays out.
        p.handle(3 * SEC, key(0x02, false));
        p.handle(3 * SEC, key(0x02, true));
        p.handle(3 * SEC, key(W, true));
        let clip = all(&mut p);
        assert_eq!(clip.events.len(), 3);
        assert_eq!(clip.state_at(5000).held.keys, vec![KeyId(W)]);
    }

    #[test]
    fn empty_scope_stores_no_key_at_all() {
        let layout = OverlayLayout::new("none", LayoutKind::AnsiQwerty, Scope::Cut { fraction: 0.0 });
        let mut p = running(&layout);
        for make in 1..0x80 {
            p.handle(SEC, key(make, true));
        }
        assert!(p.track().is_empty());
    }
}
