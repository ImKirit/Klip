//! Replay buffer: encoded packets, trimmed by age. Nothing is decoded here. The packets
//! live in RAM or, for long buffers, in segment files on disk with only an index in RAM.

use std::collections::VecDeque;
use std::fs::File;
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Clone)]
pub struct Packet {
    pub data: Arc<[u8]>,
    /// Presentation time in 100 ns units.
    pub pts: i64,
    pub duration: i64,
    pub key: bool,
}

struct Entry {
    pts: i64,
    duration: i64,
    key: bool,
    len: usize,
    /// Payload when the buffer lives in RAM.
    data: Option<Arc<[u8]>>,
    /// Segment number and offset when it lives on disk.
    segment: u64,
    offset: u64,
}

/// Segment files of the disk buffer. A segment always starts on a keyframe, so dropping
/// whole segments from the front never leaves undecodable frames behind.
struct Disk {
    dir: PathBuf,
    segment: u64,
    offset: u64,
    started: i64,
    writer: Option<BufWriter<File>>,
}

const SEGMENT_LEN: i64 = 5 * 10_000_000;

impl Disk {
    fn path(&self, segment: u64) -> PathBuf {
        self.dir.join(format!("{segment:08}.klipbuf"))
    }
}

pub struct Ring {
    entries: VecDeque<Entry>,
    bytes: usize,
    keep: i64,
    disk: Option<Disk>,
}

impl Ring {
    pub fn in_memory(keep_seconds: u32) -> Self {
        Self { entries: VecDeque::new(), bytes: 0, keep: keep_seconds as i64 * 10_000_000, disk: None }
    }

    /// Buffer in `dir`. Leftover segments from an earlier run are removed first.
    pub fn on_disk(keep_seconds: u32, dir: PathBuf) -> std::io::Result<Self> {
        std::fs::create_dir_all(&dir)?;
        for entry in std::fs::read_dir(&dir)? {
            let path = entry?.path();
            if path.extension().is_some_and(|e| e == "klipbuf") {
                let _ = std::fs::remove_file(path);
            }
        }
        let mut ring = Self::in_memory(keep_seconds);
        ring.disk = Some(Disk { dir, segment: 0, offset: 0, started: i64::MIN, writer: None });
        Ok(ring)
    }

    pub fn is_on_disk(&self) -> bool {
        self.disk.is_some()
    }

    pub fn push(&mut self, p: Packet) {
        let mut entry = Entry {
            pts: p.pts,
            duration: p.duration,
            key: p.key,
            len: p.data.len(),
            data: None,
            segment: 0,
            offset: 0,
        };
        match &mut self.disk {
            None => entry.data = Some(p.data.clone()),
            Some(disk) => {
                if p.key && (disk.writer.is_none() || p.pts - disk.started >= SEGMENT_LEN) {
                    if let Some(mut old) = disk.writer.take() {
                        let _ = old.flush();
                        disk.segment += 1;
                    }
                    disk.offset = 0;
                    disk.started = p.pts;
                    disk.writer = File::create(disk.path(disk.segment)).ok().map(BufWriter::new);
                }
                // Until the first keyframe there is no segment, such packets could not be decoded anyway.
                let Some(writer) = &mut disk.writer else { return };
                if writer.write_all(&p.data).is_err() {
                    return;
                }
                entry.segment = disk.segment;
                entry.offset = disk.offset;
                disk.offset += p.data.len() as u64;
            }
        }
        self.bytes += entry.len;
        let newest = entry.pts;
        self.entries.push_back(entry);

        // Drop whole groups of pictures only: the buffer must always start on a keyframe,
        // otherwise the oldest frames could not be decoded.
        loop {
            let Some(second_key) = self.entries.iter().skip(1).position(|e| e.key) else { break };
            let second_key = second_key + 1;
            if newest - self.entries[second_key].pts < self.keep {
                break;
            }
            for _ in 0..second_key {
                if let Some(old) = self.entries.pop_front() {
                    self.bytes -= old.len;
                }
            }
        }
        if let (Some(disk), Some(front)) = (&mut self.disk, self.entries.front()) {
            // Segments are numbered without gaps, everything below the oldest entry is unused.
            let mut segment = front.segment;
            while segment > 0 {
                segment -= 1;
                if std::fs::remove_file(disk.path(segment)).is_err() {
                    break;
                }
            }
        }
    }

    /// The last `seconds`, extended backwards to the keyframe at or before the start.
    pub fn last(&mut self, seconds: u32) -> Vec<Packet> {
        let Some(newest) = self.entries.back() else { return Vec::new() };
        let from = newest.pts - seconds as i64 * 10_000_000;
        let mut start = None;
        for (i, e) in self.entries.iter().enumerate() {
            if e.key && (e.pts <= from || start.is_none()) {
                start = Some(i);
            }
        }
        self.read_from(start.unwrap_or(0))
    }

    /// Everything since the newest keyframe, the head start of a manual recording.
    pub fn since_last_keyframe(&mut self) -> Vec<Packet> {
        match self.entries.iter().rposition(|e| e.key) {
            Some(i) => self.read_from(i),
            None => Vec::new(),
        }
    }

    /// Everything from pts on. Used to fetch the audio that belongs to a stretch of video.
    pub fn since(&mut self, pts: i64) -> Vec<Packet> {
        match self.entries.iter().position(|e| e.pts >= pts) {
            Some(i) => self.read_from(i),
            None => Vec::new(),
        }
    }

    /// The stretch `from..=to`, extended backwards to the keyframe at or before `from`.
    pub fn range(&mut self, from: i64, to: i64) -> Vec<Packet> {
        let mut start = None;
        for (i, e) in self.entries.iter().enumerate() {
            if e.key && (e.pts <= from || start.is_none()) {
                start = Some(i);
            }
        }
        let Some(start) = start else { return Vec::new() };
        let mut packets = self.read_from(start);
        packets.retain(|p| p.pts <= to);
        packets
    }

    fn read_from(&mut self, start: usize) -> Vec<Packet> {
        if let Some(disk) = &mut self.disk {
            if let Some(w) = &mut disk.writer {
                let _ = w.flush();
            }
        }
        let mut open: Option<(u64, File)> = None;
        let mut out = Vec::with_capacity(self.entries.len() - start);
        for e in self.entries.iter().skip(start) {
            let data: Arc<[u8]> = match (&e.data, &self.disk) {
                (Some(d), _) => d.clone(),
                (None, Some(disk)) => {
                    if open.as_ref().map(|(s, _)| *s) != Some(e.segment) {
                        open = File::open(disk.path(e.segment)).ok().map(|f| (e.segment, f));
                    }
                    let Some((_, file)) = &mut open else { continue };
                    let mut buf = vec![0u8; e.len];
                    if file.seek(SeekFrom::Start(e.offset)).is_err() || file.read_exact(&mut buf).is_err() {
                        continue;
                    }
                    Arc::from(buf)
                }
                (None, None) => continue,
            };
            out.push(Packet { data, pts: e.pts, duration: e.duration, key: e.key });
        }
        out
    }

    /// Removes the buffer files. Called on shutdown.
    pub fn discard(&mut self) {
        self.entries.clear();
        self.bytes = 0;
        if let Some(disk) = &mut self.disk {
            disk.writer = None;
            for segment in 0..=disk.segment {
                let _ = std::fs::remove_file(disk.path(segment));
            }
        }
    }

    pub fn bytes(&self) -> usize {
        self.bytes
    }

    pub fn span_seconds(&self) -> f64 {
        match (self.entries.front(), self.entries.back()) {
            (Some(a), Some(b)) => (b.pts - a.pts) as f64 / 10_000_000.0,
            _ => 0.0,
        }
    }
}
