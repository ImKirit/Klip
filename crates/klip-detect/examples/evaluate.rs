//! Runs a detector over a video file and compares the result with a hand made
//! truth file.
//!
//! ```text
//! cargo run --release -p klip-detect --example evaluate -- <video> <truth.json>
//!     [--scale WxH]        scale the video before detection (simulation)
//!     [--x-stretch F]      profile value hud_x_stretch for this run
//!     [--rate HZ]          sampling rate, default from the profile
//!     [--tolerance S]      match window in seconds, default 1.5
//!     [--profile FILE]     Valorant profile to use instead of the embedded one
//!     [--ffmpeg PATH]      default C:\ffmpeg\bin\ffmpeg.exe
//!     [--debug]            print what the detector saw in every sample
//! ```
//!
//! Frames are read as raw BGRA from an ffmpeg child process. The regions are cut
//! out of the full frame the same way the recorder will do it.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;

use klip_detect::valorant::{ValorantDetector, ValorantProfile};
use klip_detect::{plan_clips, Detector, Event, EventKind, RegionImage, Rule, RuleSet, Sample};
use serde::Deserialize;

#[derive(Deserialize)]
struct TruthFile {
    #[serde(default)]
    notes: String,
    events: Vec<TruthEvent>,
}

#[derive(Deserialize)]
struct TruthEvent {
    /// Seconds from the start of the video.
    t: f64,
    kind: EventKind,
}

struct Args {
    video: PathBuf,
    truth: PathBuf,
    scale: Option<(u32, u32)>,
    x_stretch: Option<f32>,
    rate: Option<f32>,
    tolerance: f64,
    profile: Option<PathBuf>,
    ffmpeg: PathBuf,
    debug: bool,
}

fn parse_size(text: &str) -> Option<(u32, u32)> {
    let (w, h) = text.split_once(['x', 'X'])?;
    Some((w.parse().ok()?, h.parse().ok()?))
}

fn parse_args() -> Result<Args, String> {
    let mut positional = Vec::new();
    let mut args = Args {
        video: PathBuf::new(),
        truth: PathBuf::new(),
        scale: None,
        x_stretch: None,
        rate: None,
        tolerance: 1.5,
        profile: None,
        ffmpeg: PathBuf::from(r"C:\ffmpeg\bin\ffmpeg.exe"),
        debug: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        let mut value = |name: &str| it.next().ok_or(format!("{name} needs a value"));
        match a.as_str() {
            "--scale" => args.scale = Some(parse_size(&value("--scale")?).ok_or("--scale wants WxH")?),
            "--x-stretch" => args.x_stretch = Some(value("--x-stretch")?.parse().map_err(|_| "--x-stretch wants a number")?),
            "--rate" => args.rate = Some(value("--rate")?.parse().map_err(|_| "--rate wants a number")?),
            "--tolerance" => args.tolerance = value("--tolerance")?.parse().map_err(|_| "--tolerance wants a number")?,
            "--profile" => args.profile = Some(PathBuf::from(value("--profile")?)),
            "--ffmpeg" => args.ffmpeg = PathBuf::from(value("--ffmpeg")?),
            "--debug" => args.debug = true,
            other if other.starts_with("--") => return Err(format!("unknown option {other}")),
            _ => positional.push(a),
        }
    }
    if positional.len() != 2 {
        return Err("usage: evaluate <video> <truth.json> [options]".into());
    }
    args.video = PathBuf::from(&positional[0]);
    args.truth = PathBuf::from(&positional[1]);
    Ok(args)
}

fn probe_size(ffmpeg: &Path, video: &Path) -> Result<(u32, u32), String> {
    let ffprobe = ffmpeg.with_file_name(if cfg!(windows) { "ffprobe.exe" } else { "ffprobe" });
    let out = Command::new(&ffprobe)
        .args(["-v", "error", "-select_streams", "v:0", "-show_entries", "stream=width,height", "-of", "csv=p=0:s=x"])
        .arg(video)
        .output()
        .map_err(|e| format!("cannot run {}: {e}", ffprobe.display()))?;
    let text = String::from_utf8_lossy(&out.stdout);
    parse_size(text.trim()).ok_or(format!("ffprobe gave no size for {}", video.display()))
}

struct Score {
    truth: usize,
    found: usize,
    hits: usize,
}

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args = parse_args()?;
    let truth_text = std::fs::read_to_string(&args.truth).map_err(|e| format!("{}: {e}", args.truth.display()))?;
    let truth: TruthFile = serde_json::from_str(truth_text.trim_start_matches('\u{feff}')).map_err(|e| format!("{}: {e}", args.truth.display()))?;

    let mut profile = match &args.profile {
        Some(path) => {
            let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
            ValorantProfile::from_json(&text).map_err(|e| format!("{}: {e}", path.display()))?
        }
        None => ValorantProfile::builtin(),
    };
    if let Some(s) = args.x_stretch {
        profile.hud_x_stretch = s;
    }
    let rate = args.rate.unwrap_or(profile.sample_rate_hz);
    let profile_name = profile.name.clone();
    let mut det = ValorantDetector::with_profile(profile);

    let source = probe_size(&args.ffmpeg, &args.video)?;
    let (w, h) = args.scale.unwrap_or(source);
    let mut filter = format!("fps={rate}");
    if args.scale.is_some() {
        filter.push_str(&format!(",scale={w}:{h}:flags=bilinear"));
    }
    let mut child = Command::new(&args.ffmpeg)
        .args(["-v", "error", "-i"])
        .arg(&args.video)
        .args(["-vf", &filter, "-f", "rawvideo", "-pix_fmt", "bgra", "-"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot run {}: {e}", args.ffmpeg.display()))?;
    let mut pipe = child.stdout.take().ok_or("no ffmpeg output")?;

    let requests = det.regions();
    let rects: Vec<_> = requests.iter().map(|r| (r.id, r.name, r.pixel_rect(w, h))).collect();

    println!("video      {}", args.video.display());
    println!("source     {}x{}, analysed as {}x{} at {} Hz", source.0, source.1, w, h, rate);
    println!("profile    {profile_name}, hud_x_stretch {}", det.profile().hud_x_stretch);
    for (id, name, r) in &rects {
        println!("region {id}   {name}: x {} y {} w {} h {}", r.x, r.y, r.w, r.h);
    }
    if !truth.notes.is_empty() {
        println!("truth      {}", truth.notes);
    }

    let mut frame = vec![0u8; w as usize * h as usize * 4];
    let mut found: Vec<Event> = Vec::new();
    let mut micros: Vec<f64> = Vec::new();
    let mut index = 0u64;
    loop {
        if pipe.read_exact(&mut frame).is_err() {
            break;
        }
        let t_ms = (index as f64 * 1000.0 / rate as f64).round() as u64;
        let regions: Vec<RegionImage<'_>> =
            rects.iter().filter_map(|(id, _, r)| RegionImage::from_frame(*id, &frame, w, h, *r)).collect();
        let sample = Sample { t_ms, frame_width: w, frame_height: h, regions: &regions };
        let start = Instant::now();
        let events = det.process(&sample);
        micros.push(start.elapsed().as_secs_f64() * 1e6);
        if args.debug {
            let d = det.diagnostics();
            let b = d.round_banner;
            if !d.feed.is_empty() || b.corners > 0 || d.kill_banner_white > 0.01 || !events.is_empty() {
                println!(
                    "  {:7.2}s feed {:?} | round banner white {:.2} ally {:.2} enemy {:.2} corners {} | kill banner {:.3} | {:?}",
                    t_ms as f64 / 1000.0,
                    d.feed.iter().map(|o| format!("{:?} x{:.3} y{:.3} s{:.2}{}", o.kind, o.x, o.y, o.score, if o.anchored { " anchored" } else { "" })).collect::<Vec<_>>(),
                    b.white,
                    b.ally,
                    b.enemy,
                    b.corners,
                    d.kill_banner_white,
                    events.iter().map(|e| e.kind.id()).collect::<Vec<_>>()
                );
            }
        }
        found.extend(events);
        index += 1;
    }
    let _ = child.wait();
    if index == 0 {
        return Err("ffmpeg delivered no frames".into());
    }
    let end_ms = (index as f64 * 1000.0 / rate as f64) as u64;
    found.extend(det.poll(end_ms + 60_000));
    found.sort_by_key(|e| e.t_ms);

    // Match: every truth event may be used once, nearest detection of the same kind.
    let mut used = vec![false; found.len()];
    let mut scores: BTreeMap<EventKind, Score> = BTreeMap::new();
    for kind in det.supported_events() {
        scores.insert(*kind, Score { truth: 0, found: 0, hits: 0 });
    }
    for e in &found {
        scores.entry(e.kind).or_insert(Score { truth: 0, found: 0, hits: 0 }).found += 1;
    }
    println!();
    println!("truth events");
    for t in &truth.events {
        let s = scores.entry(t.kind).or_insert(Score { truth: 0, found: 0, hits: 0 });
        s.truth += 1;
        let best = found
            .iter()
            .enumerate()
            .filter(|(i, e)| !used[*i] && e.kind == t.kind)
            .map(|(i, e)| (i, (e.t_ms as f64 / 1000.0 - t.t).abs()))
            .filter(|(_, d)| *d <= args.tolerance)
            .min_by(|a, b| a.1.total_cmp(&b.1));
        match best {
            Some((i, _)) => {
                used[i] = true;
                s.hits += 1;
                let e = &found[i];
                println!(
                    "  {:7.2}s {:<14} found at {:7.2}s (offset {:+.2}s, confidence {:.2})",
                    t.t,
                    t.kind.id(),
                    e.t_ms as f64 / 1000.0,
                    e.t_ms as f64 / 1000.0 - t.t,
                    e.confidence
                );
            }
            None => println!("  {:7.2}s {:<14} MISSED", t.t, t.kind.id()),
        }
    }
    let false_alarms: Vec<&Event> = found.iter().enumerate().filter(|(i, _)| !used[*i]).map(|(_, e)| e).collect();
    println!("false alarms");
    if false_alarms.is_empty() {
        println!("  none");
    }
    for e in &false_alarms {
        println!("  {:7.2}s {:<14} confidence {:.2}", e.t_ms as f64 / 1000.0, e.kind.id(), e.confidence);
    }

    println!();
    println!("{:<14} {:>5} {:>5} {:>5} {:>6} {:>6} {:>9} {:>7}", "event", "truth", "found", "hits", "false", "missed", "precision", "recall");
    for (kind, s) in &scores {
        if s.truth == 0 && s.found == 0 {
            continue;
        }
        let ratio = |a: usize, b: usize| if b == 0 { "n/a".to_string() } else { format!("{:.2}", a as f64 / b as f64) };
        println!(
            "{:<14} {:>5} {:>5} {:>5} {:>6} {:>6} {:>9} {:>7}",
            kind.id(),
            s.truth,
            s.found,
            s.hits,
            s.found - s.hits,
            s.truth - s.hits,
            ratio(s.hits, s.found),
            ratio(s.hits, s.truth)
        );
    }

    micros.sort_by(|a, b| a.total_cmp(b));
    let mean = micros.iter().sum::<f64>() / micros.len() as f64;
    let pick = |q: f64| micros[((micros.len() - 1) as f64 * q).round() as usize];
    println!();
    println!(
        "time per sample over {} samples: mean {:.0} us, median {:.0} us, p95 {:.0} us, max {:.0} us ({})",
        micros.len(),
        mean,
        pick(0.5),
        pick(0.95),
        pick(1.0),
        if cfg!(debug_assertions) { "DEBUG build, not representative" } else { "release build" }
    );

    // Quiet timing: ffmpeg has exited, the last frame is processed again and again.
    let regions: Vec<RegionImage<'_>> =
        rects.iter().filter_map(|(id, _, r)| RegionImage::from_frame(*id, &frame, w, h, *r)).collect();
    let mut quiet: Vec<f64> = Vec::new();
    for i in 0..300u64 {
        let sample = Sample { t_ms: end_ms + 100_000 + i * 100, frame_width: w, frame_height: h, regions: &regions };
        let start = Instant::now();
        let _ = det.process(&sample);
        quiet.push(start.elapsed().as_secs_f64() * 1e6);
    }
    quiet.sort_by(|a, b| a.total_cmp(b));
    println!(
        "time per sample without ffmpeg running, last frame 300 times: median {:.0} us, p95 {:.0} us, max {:.0} us",
        quiet[150], quiet[285], quiet[299]
    );

    // Show what the rule model would cut, with every supported event switched on.
    let mut rules = RuleSet::default();
    for kind in det.supported_events() {
        let r = rules.rules.get(kind).copied().unwrap_or(Rule::new(true, 10.0, 5.0));
        rules.set(*kind, Rule { enabled: true, ..r });
    }
    println!();
    println!("clip jobs with every supported event switched on");
    for job in plan_clips(&found, &rules) {
        println!(
            "  {:7.2}s to {:7.2}s  {:<12} [{}]",
            job.start_ms as f64 / 1000.0,
            job.end_ms as f64 / 1000.0,
            job.label.id(),
            job.events.iter().map(|e| e.kind.id()).collect::<Vec<_>>().join(", ")
        );
    }
    Ok(())
}
