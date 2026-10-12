//! Manual recording: packets go straight from the encoder into a file, with pause and
//! markers. Shares the encoder with the replay buffer, so recording adds no encoding work.

use crate::encoder::StreamType;
use crate::mux::Mp4Writer;
use crate::ring::Packet;
use crate::Res;
use std::path::PathBuf;

enum State {
    Running,
    Paused,
    /// Resumed, waiting for the next keyframe. Frames before it could not be decoded
    /// because the frames they depend on were skipped during the pause.
    Resuming,
}

pub struct Recording {
    writer: Mp4Writer,
    pub path: PathBuf,
    state: State,
    /// Source time of the first packet.
    base: i64,
    /// Total length of all pauses, taken out of the timeline.
    paused: i64,
    last_pts: i64,
    /// Audio older than this belongs to a pause or to the time before the recording.
    audio_from: i64,
    ever_paused: bool,
    pub markers: Vec<u64>,
}

impl Recording {
    /// Starts a recording with `head`, the packets since the newest keyframe, so the file
    /// begins at the moment of the key press instead of up to a second later.
    pub fn start(path: PathBuf, stream: &StreamType, audio: &[StreamType], head: &[Packet]) -> Res<Self> {
        let first = head.first().ok_or("no keyframe in the buffer yet")?;
        let mut rec = Self {
            writer: Mp4Writer::create(&path, stream, audio, true)?,
            path,
            state: State::Running,
            base: first.pts,
            paused: 0,
            last_pts: first.pts,
            audio_from: first.pts,
            ever_paused: false,
            markers: Vec::new(),
        };
        for p in head {
            rec.push(p)?;
        }
        Ok(rec)
    }

    pub fn push(&mut self, p: &Packet) -> Res<()> {
        match self.state {
            State::Paused => return Ok(()),
            State::Resuming => {
                if !p.key {
                    return Ok(());
                }
                self.paused += p.pts - self.last_pts - p.duration;
                self.audio_from = p.pts;
                self.state = State::Running;
            }
            State::Running => {}
        }
        // The head start overlaps with packets that arrive right after it.
        if p.pts <= self.last_pts && self.writer.end > 0 {
            return Ok(());
        }
        self.last_pts = p.pts;
        self.writer.write(0, p, p.pts - self.base - self.paused)
    }

    /// Adds one audio packet of `track`. Audio follows the video: it is skipped while
    /// paused and until the first video frame after a pause.
    pub fn push_audio(&mut self, track: usize, p: &Packet) -> Res<()> {
        if !matches!(self.state, State::Running) || p.pts < self.audio_from {
            return Ok(());
        }
        self.writer.write(track + 1, p, p.pts - self.base - self.paused)
    }

    pub fn base(&self) -> i64 {
        self.base
    }

    /// True if the recording was paused at least once, its timeline then has gaps cut out.
    pub fn was_paused(&self) -> bool {
        self.ever_paused
    }

    /// Toggles pause and returns true if the recording is paused afterwards.
    pub fn toggle_pause(&mut self) -> bool {
        self.ever_paused = true;
        self.state = match self.state {
            State::Running => State::Paused,
            State::Paused | State::Resuming => State::Resuming,
        };
        matches!(self.state, State::Paused)
    }

    /// Remembers the current position. Shown in the editor later.
    pub fn mark(&mut self) -> u64 {
        let ms = (self.writer.end / 10_000) as u64;
        self.markers.push(ms);
        ms
    }

    pub fn length_ms(&self) -> u64 {
        (self.writer.end / 10_000) as u64
    }

    pub fn finish(self) -> Res<(PathBuf, u64, Vec<u64>)> {
        let ms = self.writer.finish()?;
        Ok((self.path, ms, self.markers))
    }
}
