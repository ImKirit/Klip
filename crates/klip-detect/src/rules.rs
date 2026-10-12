//! Rule model: which events produce clips, and how events become clip jobs.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::event::{Event, EventKind};

/// Setting for one event kind.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct Rule {
    pub enabled: bool,
    /// Seconds kept before the event.
    pub before_s: f32,
    /// Seconds kept after the event.
    pub after_s: f32,
}

impl Rule {
    pub const fn new(enabled: bool, before_s: f32, after_s: f32) -> Rule {
        Rule { enabled, before_s, after_s }
    }
}

/// All rules of one game. Kinds without an entry count as switched off.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct RuleSet {
    pub rules: BTreeMap<EventKind, Rule>,
    /// Clips whose windows are at most this far apart are joined.
    #[serde(default = "default_merge_gap")]
    pub merge_gap_s: f32,
    /// Upper bound for one clip. A longer clip loses its beginning.
    #[serde(default = "default_max_clip")]
    pub max_clip_s: f32,
    /// Events below this confidence are ignored.
    #[serde(default)]
    pub min_confidence: f32,
}

fn default_merge_gap() -> f32 {
    2.0
}

fn default_max_clip() -> f32 {
    120.0
}

impl Default for RuleSet {
    fn default() -> Self {
        let mut rules = BTreeMap::new();
        for kind in EventKind::ALL {
            let rule = match kind {
                EventKind::Kill
                | EventKind::DoubleKill
                | EventKind::TripleKill
                | EventKind::QuadraKill
                | EventKind::Ace => Rule::new(true, 10.0, 5.0),
                EventKind::Clutch => Rule::new(true, 20.0, 5.0),
                EventKind::MatchWon | EventKind::MatchLost => Rule::new(true, 15.0, 8.0),
                EventKind::Death => Rule::new(false, 10.0, 3.0),
                EventKind::SpikePlanted | EventKind::SpikeDefused => Rule::new(false, 10.0, 5.0),
                EventKind::RoundWon | EventKind::RoundLost => Rule::new(false, 15.0, 5.0),
            };
            rules.insert(kind, rule);
        }
        RuleSet { rules, merge_gap_s: default_merge_gap(), max_clip_s: default_max_clip(), min_confidence: 0.0 }
    }
}

impl RuleSet {
    /// Rule for a kind, if it is switched on.
    pub fn active(&self, kind: EventKind) -> Option<Rule> {
        self.rules.get(&kind).copied().filter(|r| r.enabled)
    }

    pub fn set(&mut self, kind: EventKind, rule: Rule) {
        self.rules.insert(kind, rule);
    }

    pub fn from_json(text: &str) -> Result<RuleSet, serde_json::Error> {
        serde_json::from_str(text)
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("rule set serializes")
    }
}

/// One clip to cut out of the replay buffer.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ClipJob {
    pub start_ms: u64,
    pub end_ms: u64,
    /// The most important event, used to name the clip.
    pub label: EventKind,
    /// The events inside the clip after multi kill replacement, ordered by time.
    pub events: Vec<Event>,
}

struct Windowed {
    start: u64,
    end: u64,
    event: Event,
}

fn secs_to_ms(s: f32) -> u64 {
    (s.max(0.0) as f64 * 1000.0).round() as u64
}

/// Applies the rules to one event. A multi kill whose own rule is switched off
/// falls back to the single kill rule and is then treated as a single kill.
fn window_for(event: &Event, rules: &RuleSet) -> Option<Windowed> {
    if event.confidence < rules.min_confidence {
        return None;
    }
    let (rule, event) = match rules.active(event.kind) {
        Some(rule) => (rule, event.clone()),
        None if event.kind.kill_tier() > 1 => {
            let rule = rules.active(EventKind::Kill)?;
            let mut e = event.clone();
            e.kind = EventKind::Kill;
            e.start_ms = None;
            (rule, e)
        }
        None => return None,
    };
    let from = event.start_ms.unwrap_or(event.t_ms).min(event.t_ms);
    Some(Windowed {
        start: from.saturating_sub(secs_to_ms(rule.before_s)),
        end: event.t_ms + secs_to_ms(rule.after_s),
        event,
    })
}

fn finish(group: Vec<Windowed>, rules: &RuleSet) -> ClipJob {
    let mut start = group.iter().map(|w| w.start).min().unwrap_or(0);
    let end = group.iter().map(|w| w.end).max().unwrap_or(0);
    let mut events: Vec<Event> = group.into_iter().map(|w| w.event).collect();
    events.sort_by_key(|e| e.t_ms);

    // A multi kill replaces the kills it is made of: drop every kill family event
    // of a lower tier that happened up to the highest one.
    if let Some(top) = events
        .iter()
        .filter(|e| e.kind.kill_tier() > 1)
        .max_by_key(|e| (e.kind.kill_tier(), e.t_ms))
        .cloned()
    {
        events.retain(|e| {
            let tier = e.kind.kill_tier();
            tier == 0 || tier >= top.kind.kill_tier() || e.t_ms > top.t_ms
        });
    }

    let label = events
        .iter()
        .max_by_key(|e| e.kind.label_priority())
        .map(|e| e.kind)
        .unwrap_or(EventKind::Kill);

    let max = secs_to_ms(rules.max_clip_s);
    if max > 0 && end - start > max {
        start = end - max;
    }
    ClipJob { start_ms: start, end_ms: end, label, events }
}

/// Turns events into clip jobs: applies the rules, joins overlapping windows into
/// one clip and lets a multi kill replace its single kills. Jobs come out ordered
/// by time and never overlap.
pub fn plan_clips(events: &[Event], rules: &RuleSet) -> Vec<ClipJob> {
    let mut windows: Vec<Windowed> = events.iter().filter_map(|e| window_for(e, rules)).collect();
    windows.sort_by_key(|w| (w.start, w.end));
    let gap = secs_to_ms(rules.merge_gap_s);

    let mut jobs = Vec::new();
    let mut group: Vec<Windowed> = Vec::new();
    let mut group_end = 0u64;
    for w in windows {
        if !group.is_empty() && w.start > group_end + gap {
            jobs.push(finish(std::mem::take(&mut group), rules));
        }
        group_end = if group.is_empty() { w.end } else { group_end.max(w.end) };
        group.push(w);
    }
    if !group.is_empty() {
        jobs.push(finish(group, rules));
    }
    jobs
}

/// Streaming form of [`plan_clips`] for the recorder: push events as they arrive,
/// and take the clips whose window has closed.
pub struct ClipPlanner {
    rules: RuleSet,
    pending: Vec<Event>,
}

impl ClipPlanner {
    pub fn new(rules: RuleSet) -> ClipPlanner {
        ClipPlanner { rules, pending: Vec::new() }
    }

    pub fn rules(&self) -> &RuleSet {
        &self.rules
    }

    /// Replaces the rules. Pending events are planned with the new rules.
    pub fn set_rules(&mut self, rules: RuleSet) {
        self.rules = rules;
    }

    pub fn push(&mut self, event: Event) {
        self.pending.push(event);
    }

    /// Clips that can no longer grow: their end plus the merge gap lies before
    /// `now_ms`. Their events leave the planner. Events that match no active rule
    /// are dropped.
    pub fn take_ready(&mut self, now_ms: u64) -> Vec<ClipJob> {
        let gap = secs_to_ms(self.rules.merge_gap_s);
        let jobs = plan_clips(&self.pending, &self.rules);
        let mut ready = Vec::new();
        let mut keep_from = None;
        for job in jobs {
            if job.end_ms + gap < now_ms {
                ready.push(job);
            } else {
                keep_from = Some(job.events.iter().map(|e| e.start_ms.unwrap_or(e.t_ms).min(e.t_ms)).min().unwrap_or(0));
                break;
            }
        }
        match keep_from {
            // Everything from the first open clip on stays pending, in its
            // original form so a later rule change still sees the real kinds.
            Some(t) => self.pending.retain(|e| e.t_ms >= t),
            None => self.pending.clear(),
        }
        ready
    }

    /// Forgets pending events, for example when the game exits.
    pub fn clear(&mut self) {
        self.pending.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(kind: EventKind, t_s: f32) -> Event {
        Event::new(kind, (t_s * 1000.0) as u64, 0.9)
    }

    fn multi(kind: EventKind, start_s: f32, t_s: f32) -> Event {
        Event { kind, t_ms: (t_s * 1000.0) as u64, start_ms: Some((start_s * 1000.0) as u64), confidence: 0.9 }
    }

    #[test]
    fn default_rules_roundtrip_through_json() {
        let rules = RuleSet::default();
        let text = rules.to_json();
        assert!(text.contains("\"double_kill\""));
        assert!(text.contains("\"spike_planted\""));
        assert_eq!(RuleSet::from_json(&text).unwrap(), rules);
    }

    #[test]
    fn missing_optional_fields_get_defaults() {
        let rules = RuleSet::from_json(r#"{"rules":{"kill":{"enabled":true,"before_s":8,"after_s":4}}}"#).unwrap();
        assert_eq!(rules.merge_gap_s, 2.0);
        assert_eq!(rules.max_clip_s, 120.0);
        assert!(rules.active(EventKind::Kill).is_some());
        assert!(rules.active(EventKind::Death).is_none());
    }

    #[test]
    fn single_kill_uses_before_and_after() {
        let jobs = plan_clips(&[ev(EventKind::Kill, 60.0)], &RuleSet::default());
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].start_ms, 50_000);
        assert_eq!(jobs[0].end_ms, 65_000);
        assert_eq!(jobs[0].label, EventKind::Kill);
    }

    #[test]
    fn window_never_starts_before_zero() {
        let jobs = plan_clips(&[ev(EventKind::Kill, 3.0)], &RuleSet::default());
        assert_eq!(jobs[0].start_ms, 0);
    }

    #[test]
    fn disabled_events_make_no_clip() {
        let events = [ev(EventKind::Death, 60.0), ev(EventKind::RoundLost, 70.0)];
        assert!(plan_clips(&events, &RuleSet::default()).is_empty());
    }

    #[test]
    fn low_confidence_is_filtered() {
        let mut rules = RuleSet::default();
        rules.min_confidence = 0.95;
        assert!(plan_clips(&[ev(EventKind::Kill, 60.0)], &rules).is_empty());
    }

    #[test]
    fn overlapping_events_become_one_clip() {
        let mut rules = RuleSet::default();
        rules.set(EventKind::Death, Rule::new(true, 10.0, 3.0));
        let events = [ev(EventKind::Kill, 60.0), ev(EventKind::Death, 68.0)];
        let jobs = plan_clips(&events, &rules);
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].start_ms, 50_000);
        assert_eq!(jobs[0].end_ms, 71_000);
        assert_eq!(jobs[0].label, EventKind::Kill);
        assert_eq!(jobs[0].events.len(), 2);
    }

    #[test]
    fn distant_events_stay_separate() {
        let events = [ev(EventKind::Kill, 60.0), ev(EventKind::Kill, 200.0)];
        let jobs = plan_clips(&events, &RuleSet::default());
        assert_eq!(jobs.len(), 2);
        assert!(jobs[0].end_ms < jobs[1].start_ms);
    }

    #[test]
    fn windows_within_the_merge_gap_are_joined() {
        // First clip ends at 65 s, second starts at 66.5 s: gap 1.5 s is below 2 s.
        let events = [ev(EventKind::Kill, 60.0), ev(EventKind::Kill, 76.5)];
        assert_eq!(plan_clips(&events, &RuleSet::default()).len(), 1);
        let mut rules = RuleSet::default();
        rules.merge_gap_s = 0.0;
        assert_eq!(plan_clips(&events, &rules).len(), 2);
    }

    #[test]
    fn multi_kill_replaces_single_kills() {
        let events = [
            ev(EventKind::Kill, 60.0),
            multi(EventKind::DoubleKill, 60.0, 63.0),
            multi(EventKind::TripleKill, 60.0, 66.0),
        ];
        let jobs = plan_clips(&events, &RuleSet::default());
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].label, EventKind::TripleKill);
        assert_eq!(jobs[0].events.len(), 1);
        assert_eq!(jobs[0].events[0].kind, EventKind::TripleKill);
        assert_eq!(jobs[0].start_ms, 50_000);
        assert_eq!(jobs[0].end_ms, 71_000);
    }

    #[test]
    fn multi_kill_window_reaches_back_to_the_first_kill() {
        // Kills 40 s apart in one round: the double kill clip must still show the first.
        let jobs = plan_clips(&[multi(EventKind::DoubleKill, 60.0, 100.0)], &RuleSet::default());
        assert_eq!(jobs[0].start_ms, 50_000);
        assert_eq!(jobs[0].end_ms, 105_000);
    }

    #[test]
    fn kill_of_the_next_round_survives_replacement() {
        let mut rules = RuleSet::default();
        rules.set(EventKind::Kill, Rule::new(true, 30.0, 5.0));
        let events = [
            ev(EventKind::Kill, 60.0),
            multi(EventKind::DoubleKill, 60.0, 62.0),
            ev(EventKind::Kill, 95.0),
        ];
        let jobs = plan_clips(&events, &rules);
        assert_eq!(jobs.len(), 1);
        let kinds: Vec<EventKind> = jobs[0].events.iter().map(|e| e.kind).collect();
        assert_eq!(kinds, vec![EventKind::DoubleKill, EventKind::Kill]);
        assert_eq!(jobs[0].label, EventKind::DoubleKill);
    }

    #[test]
    fn disabled_multi_kill_falls_back_to_kill_rule() {
        let mut rules = RuleSet::default();
        rules.set(EventKind::DoubleKill, Rule::new(false, 10.0, 5.0));
        let jobs = plan_clips(&[multi(EventKind::DoubleKill, 20.0, 60.0)], &rules);
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].label, EventKind::Kill);
        assert_eq!(jobs[0].start_ms, 50_000);

        rules.set(EventKind::Kill, Rule::new(false, 10.0, 5.0));
        assert!(plan_clips(&[multi(EventKind::DoubleKill, 20.0, 60.0)], &rules).is_empty());
    }

    #[test]
    fn ace_outranks_everything_in_the_label() {
        let mut rules = RuleSet::default();
        rules.set(EventKind::RoundWon, Rule::new(true, 15.0, 5.0));
        let events = [multi(EventKind::Ace, 50.0, 60.0), ev(EventKind::RoundWon, 61.0), ev(EventKind::Clutch, 61.0)];
        let jobs = plan_clips(&events, &rules);
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].label, EventKind::Ace);
    }

    #[test]
    fn overlong_clip_loses_its_beginning() {
        let mut rules = RuleSet::default();
        rules.max_clip_s = 30.0;
        let jobs = plan_clips(&[multi(EventKind::DoubleKill, 10.0, 100.0)], &rules);
        assert_eq!(jobs[0].end_ms, 105_000);
        assert_eq!(jobs[0].start_ms, 75_000);
    }

    #[test]
    fn planner_waits_until_the_window_has_closed() {
        let mut p = ClipPlanner::new(RuleSet::default());
        p.push(ev(EventKind::Kill, 60.0));
        assert!(p.take_ready(64_000).is_empty());
        // 65 s end plus 2 s merge gap.
        assert!(p.take_ready(67_000).is_empty());
        let jobs = p.take_ready(67_001);
        assert_eq!(jobs.len(), 1);
        assert!(p.take_ready(100_000).is_empty());
    }

    #[test]
    fn planner_extends_an_open_clip() {
        let mut p = ClipPlanner::new(RuleSet::default());
        p.push(ev(EventKind::Kill, 60.0));
        assert!(p.take_ready(63_000).is_empty());
        p.push(multi(EventKind::DoubleKill, 60.0, 63.0));
        assert!(p.take_ready(67_500).is_empty());
        let jobs = p.take_ready(70_500);
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].label, EventKind::DoubleKill);
        assert_eq!(jobs[0].events.len(), 1);
    }

    #[test]
    fn planner_releases_closed_clips_and_keeps_open_ones() {
        let mut p = ClipPlanner::new(RuleSet::default());
        p.push(ev(EventKind::Kill, 60.0));
        p.push(ev(EventKind::Kill, 200.0));
        let jobs = p.take_ready(201_000);
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].end_ms, 65_000);
        let jobs = p.take_ready(300_000);
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].end_ms, 205_000);
    }

    #[test]
    fn planner_drops_events_without_a_rule() {
        let mut p = ClipPlanner::new(RuleSet::default());
        p.push(ev(EventKind::Death, 60.0));
        assert!(p.take_ready(61_000).is_empty());
        assert!(p.pending.is_empty());
    }
}
