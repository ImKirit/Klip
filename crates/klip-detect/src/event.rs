//! Events a detector can report.

use serde::{Deserialize, Serialize};

/// Kind of a game event. The serialized form (snake_case) is the stable identifier
/// used in settings files, clip metadata and truth files.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum EventKind {
    Kill,
    DoubleKill,
    TripleKill,
    QuadraKill,
    Ace,
    Death,
    Clutch,
    SpikePlanted,
    SpikeDefused,
    RoundWon,
    RoundLost,
    MatchWon,
    MatchLost,
}

impl EventKind {
    /// All kinds in a fixed order.
    pub const ALL: [EventKind; 13] = [
        EventKind::Kill,
        EventKind::DoubleKill,
        EventKind::TripleKill,
        EventKind::QuadraKill,
        EventKind::Ace,
        EventKind::Death,
        EventKind::Clutch,
        EventKind::SpikePlanted,
        EventKind::SpikeDefused,
        EventKind::RoundWon,
        EventKind::RoundLost,
        EventKind::MatchWon,
        EventKind::MatchLost,
    ];

    /// Stable identifier, identical to the serde name.
    pub fn id(self) -> &'static str {
        match self {
            EventKind::Kill => "kill",
            EventKind::DoubleKill => "double_kill",
            EventKind::TripleKill => "triple_kill",
            EventKind::QuadraKill => "quadra_kill",
            EventKind::Ace => "ace",
            EventKind::Death => "death",
            EventKind::Clutch => "clutch",
            EventKind::SpikePlanted => "spike_planted",
            EventKind::SpikeDefused => "spike_defused",
            EventKind::RoundWon => "round_won",
            EventKind::RoundLost => "round_lost",
            EventKind::MatchWon => "match_won",
            EventKind::MatchLost => "match_lost",
        }
    }

    /// Parses a stable identifier.
    pub fn from_id(id: &str) -> Option<EventKind> {
        EventKind::ALL.iter().copied().find(|k| k.id() == id)
    }

    /// Position in the kill family: 1 for a single kill up to 5 for an ace, 0 for
    /// everything else.
    pub fn kill_tier(self) -> u8 {
        match self {
            EventKind::Kill => 1,
            EventKind::DoubleKill => 2,
            EventKind::TripleKill => 3,
            EventKind::QuadraKill => 4,
            EventKind::Ace => 5,
            _ => 0,
        }
    }

    /// Kill family member for the n-th kill of a round. Kills beyond the fifth
    /// (possible after a resurrection) count as single kills again.
    pub fn for_kill_count(n: u32) -> EventKind {
        match n {
            2 => EventKind::DoubleKill,
            3 => EventKind::TripleKill,
            4 => EventKind::QuadraKill,
            5 => EventKind::Ace,
            _ => EventKind::Kill,
        }
    }

    /// Rank used to name a clip that contains several events. Higher wins.
    pub fn label_priority(self) -> u8 {
        match self {
            EventKind::Ace => 13,
            EventKind::MatchWon => 12,
            EventKind::Clutch => 11,
            EventKind::QuadraKill => 10,
            EventKind::TripleKill => 9,
            EventKind::DoubleKill => 8,
            EventKind::MatchLost => 7,
            EventKind::SpikeDefused => 6,
            EventKind::SpikePlanted => 5,
            EventKind::Kill => 4,
            EventKind::RoundWon => 3,
            EventKind::Death => 2,
            EventKind::RoundLost => 1,
        }
    }
}

/// One detected event.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Event {
    pub kind: EventKind,
    /// Time of the event in milliseconds on the clock of the samples.
    pub t_ms: u64,
    /// For events that close a sequence (multi kills): time of the first element.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_ms: Option<u64>,
    /// 0.0 to 1.0. How sure the detector is, not a measured probability.
    pub confidence: f32,
}

impl Event {
    pub fn new(kind: EventKind, t_ms: u64, confidence: f32) -> Event {
        Event { kind, t_ms, start_ms: None, confidence }
    }
}
