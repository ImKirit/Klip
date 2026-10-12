//! Writes encoded packets into an MP4 file. Packets are copied as they are, nothing is
//! encoded again, so saving a clip costs disk time only.

use crate::encoder::StreamType;
use crate::ring::Packet;
use crate::Res;
use std::path::Path;
use windows::core::HSTRING;
use windows::Win32::Media::MediaFoundation::*;

pub struct Mp4Writer {
    writer: IMFSinkWriter,
    /// Stream numbers of the sink writer. Entry 0 is the video, the rest are audio tracks.
    streams: Vec<u32>,
    /// End of the last written video packet in 100 ns units, the length of the file so far.
    pub end: i64,
    /// The newest video packet, kept back until the next one arrives. Only then is it
    /// known how long the picture really stayed on screen. Frames can be a full second
    /// apart while the picture stands still, and a file that claims 1/60 s for each of
    /// them plays back with a collapsed timeline.
    held: Option<(Packet, i64)>,
}
// SAFETY: the sink writer is free threaded, and Mp4Writer is only used behind a Mutex.
unsafe impl Send for Mp4Writer {}

impl Mp4Writer {
    /// `fragmented` writes the index in pieces while recording, so a crash or power cut
    /// leaves a playable file. A plain MP4 keeps its index for the very end.
    pub fn create(path: &Path, video: &StreamType, audio: &[StreamType], fragmented: bool) -> Res<Self> {
        unsafe {
            let attrs = {
                let mut a = None;
                MFCreateAttributes(&mut a, 3)?;
                a.ok_or("no attributes")?
            };
            // Input and output types are identical, the writer must not insert an encoder.
            attrs.SetUINT32(&MF_READWRITE_DISABLE_CONVERTERS, 1)?;
            // By default the writer blocks a stream that runs ahead of the others. Audio is
            // dense and video can be sparse, so that would stall the audio thread, and with
            // it the whole recorder.
            attrs.SetUINT32(&MF_SINK_WRITER_DISABLE_THROTTLING, 1)?;
            if fragmented {
                attrs.SetGUID(&MF_TRANSCODE_CONTAINERTYPE, &MFTranscodeContainerType_FMPEG4)?;
            }
            let writer = MFCreateSinkWriterFromURL(&HSTRING::from(path.as_os_str()), None, &attrs)?;
            let mut streams = Vec::new();
            for stream in std::iter::once(video).chain(audio) {
                let index = writer.AddStream(&stream.0)?;
                writer.SetInputMediaType(index, &stream.0, None)?;
                streams.push(index);
            }
            writer.BeginWriting()?;
            Ok(Self { writer, streams, end: 0, held: None })
        }
    }

    /// Writes one packet of stream `stream` (0 = video) at `pts`, in 100 ns units from the
    /// start of the file.
    pub fn write(&mut self, stream: usize, p: &Packet, pts: i64) -> Res<()> {
        if stream != 0 {
            return self.put(stream, p, pts, p.duration);
        }
        self.end = pts + p.duration;
        if let Some((prev, prev_pts)) = self.held.replace((p.clone(), pts)) {
            self.put(0, &prev, prev_pts, (pts - prev_pts).max(1))?;
        }
        Ok(())
    }

    fn put(&mut self, stream: usize, p: &Packet, pts: i64, duration: i64) -> Res<()> {
        let Some(index) = self.streams.get(stream).copied() else { return Ok(()) };
        unsafe {
            let buffer = MFCreateMemoryBuffer(p.data.len() as u32)?;
            let mut ptr = std::ptr::null_mut();
            buffer.Lock(&mut ptr, None, None)?;
            std::ptr::copy_nonoverlapping(p.data.as_ptr(), ptr, p.data.len());
            buffer.Unlock()?;
            buffer.SetCurrentLength(p.data.len() as u32)?;

            let sample = MFCreateSample()?;
            sample.AddBuffer(&buffer)?;
            sample.SetSampleTime(pts)?;
            if duration > 0 {
                sample.SetSampleDuration(duration)?;
            }
            if p.key {
                sample.SetUINT32(&MFSampleExtension_CleanPoint, 1)?;
            }
            self.writer.WriteSample(index, &sample)?;
        }
        Ok(())
    }

    pub fn finish(mut self) -> Res<u64> {
        if let Some((last, pts)) = self.held.take() {
            self.put(0, &last, pts, last.duration)?;
        }
        unsafe { self.writer.Finalize()? };
        Ok((self.end / 10_000) as u64)
    }
}

/// Writes a finished list of packets as one clip and returns its length in milliseconds.
/// Audio packets outside the stretch of the video are left out.
pub fn write_mp4(path: &Path, video: &StreamType, packets: &[Packet], audio: &[(StreamType, Vec<Packet>)]) -> Res<u64> {
    let first = packets.first().ok_or("the replay buffer is empty")?;
    if !first.key {
        return Err("clip does not start on a keyframe".into());
    }
    let base = first.pts;
    let last = packets.last().unwrap();
    let end = last.pts + last.duration;

    let streams: Vec<StreamType> = audio.iter().map(|(s, _)| s.clone()).collect();
    let mut writer = Mp4Writer::create(path, video, &streams, false)?;

    // One list in time order, so the file is interleaved and players need not seek.
    let mut all: Vec<(i64, usize, &Packet)> = packets.iter().map(|p| (p.pts, 0, p)).collect();
    for (i, (_, track)) in audio.iter().enumerate() {
        all.extend(track.iter().filter(|p| p.pts >= base && p.pts < end).map(|p| (p.pts, i + 1, p)));
    }
    all.sort_by_key(|(pts, stream, _)| (*pts, *stream));
    for (pts, stream, p) in all {
        writer.write(stream, p, pts - base)?;
    }
    writer.finish()
}
