"""Candidates for "clip saved" from three tools. Nothing is played.

    python candidates.py <raw folder> <output folder> [path to sclang] [path to ffmpeg]

  P*  written here, sample by sample (standard library only)
  S*  SuperCollider, see candidates.scd
  F*  ffmpeg's own generators and effects

Every file ends up trimmed, faded and at the same peak, 16 bit stereo 44.1 kHz.
"""

import array
import math
import random
import struct
import subprocess
import sys
import wave
from pathlib import Path

RATE = 44100
PEAK = 0.5


# ---- own synthesis ---------------------------------------------------------------

def place(parts):
    total = max(int(s * RATE) + len(x) for s, x in parts)
    buf = [0.0] * total
    for s, x in parts:
        at = int(s * RATE)
        for i, v in enumerate(x):
            buf[at + i] += v
    return buf


def lowpass(buf, cutoff, passes=2):
    a = 1 - math.exp(-2 * math.pi * cutoff / RATE)
    for _ in range(passes):
        y = 0.0
        for i, v in enumerate(buf):
            y += a * (v - y)
            buf[i] = y
    return buf


def reverb(buf, mix=0.2, tail=0.35):
    """A small room after Schroeder: four combs, two all passes. Returns (left, right)."""
    dry = buf + [0.0] * int(tail * RATE)

    def side(shift):
        wet = [0.0] * len(dry)
        for delay, gain in ((1116, 0.77), (1188, 0.76), (1277, 0.75), (1356, 0.74)):
            d = delay + shift
            line = [0.0] * len(dry)
            for i, v in enumerate(dry):
                line[i] = v + (gain * line[i - d] if i >= d else 0.0)
            for i, v in enumerate(line):
                wet[i] += v * 0.25
        for d in (225 + shift, 556 + shift):
            y = [0.0] * len(wet)
            for i, v in enumerate(wet):
                past_in = wet[i - d] if i >= d else 0.0
                past_out = y[i - d] if i >= d else 0.0
                y[i] = -0.5 * v + past_in + 0.5 * past_out
            wet = y
        lowpass(wet, 4500, 1)
        return [d_ * (1 - mix) + w * mix for d_, w in zip(dry, wet)]

    return side(0), side(23)


def partials(freq, spec, attack=0.004):
    """An additive tone. spec: (ratio, level, decay in s) per partial."""
    n = int((attack + max(d for _, _, d in spec) * 6) * RATE)
    out = []
    for i in range(n):
        t = i / RATE
        rise = min(1.0, t / attack)
        out.append(rise * sum(l * math.exp(-t / d) * math.sin(2 * math.pi * freq * r * t) for r, l, d in spec))
    return out


def drop(f0, f1, glide=0.05, attack=0.006, decay=0.09):
    n = int((attack + decay * 6) * RATE)
    out, phase = [], 0.0
    for i in range(n):
        t = i / RATE
        k = min(1.0, t / glide)
        phase += 2 * math.pi * f0 * (f1 / f0) ** (1 - (1 - k) ** 2) / RATE
        out.append((math.sin(phase) + 0.1 * math.sin(2 * phase)) * min(1.0, t / attack) * math.exp(-max(0, t - attack) / decay))
    return out


def pluck(freq, seconds=0.5, damp=0.994, seed=1):
    """A plucked string after Karplus and Strong, started with a softened burst."""
    rng = random.Random(seed)
    n = int(RATE / freq)
    line = lowpass([rng.uniform(-1, 1) for _ in range(n)], 2500, 1)
    out = []
    for i in range(int(seconds * RATE)):
        v = line[i % n]
        line[i % n] = damp * 0.5 * (v + line[(i + 1) % n])
        out.append(v)
    return out


GLASS = [(1, 1.0, 0.12), (2, 0.25, 0.07), (3, 0.08, 0.04)]
BELL = [(1, 1.0, 0.16), (2.76, 0.18, 0.06), (5.4, 0.06, 0.03)]

OWN = {
    # Two soft glass notes a fifth apart.
    "P1_glass": lambda: reverb(lowpass(place([(0, partials(659.25, GLASS)), (0.095, partials(987.77, GLASS))]), 5000), 0.18),
    # One low drop, round, with a little room.
    "P2_drop": lambda: reverb(lowpass(place([(0, drop(300, 620, glide=0.07, decay=0.08))]), 2800), 0.22),
    # Two plucked notes like a kalimba.
    "P3_kalimba": lambda: reverb(lowpass(place([(0, pluck(523.25, seed=3)), (0.09, pluck(783.99, seed=5))]), 4200), 0.15),
    # A quick strum of three notes, a major chord.
    "P4_strum": lambda: reverb(
        lowpass(place([(0, partials(523.25, BELL)), (0.045, partials(659.25, BELL)), (0.09, partials(783.99, BELL))]), 4500), 0.22
    ),
}


# ---- ffmpeg ----------------------------------------------------------------------

def note(freq, at, decay, harm=0.25):
    t = f"(t-{at})"
    return (
        f"gte(t,{at})*(sin(2*PI*{freq}*{t})+{harm}*sin(2*PI*{freq * 2}*{t}))"
        f"*exp(-{t}*{decay})*min(1,{t}*250)"
    )


FFMPEG = {
    # Two pure notes with a short echo.
    "F1_echo": (f"aevalsrc='{note(880, 0, 20, 0.1)}+0.9*{note(1318.51, 0.09, 16, 0.1)}':d=0.6:s={RATE}",
                "lowpass=f=5000,aecho=0.8:0.6:55|110:0.22|0.1"),
    # A chirp: the pitch runs up while the tone fades.
    "F2_chirp": (f"aevalsrc='sin(2*PI*(380*t+1900*t*t))*exp(-t*20)*min(1,t*250)':d=0.4:s={RATE}",
                 "lowpass=f=3000,aecho=0.8:0.5:40:0.18"),
    # Two warm notes with a slow chorus.
    "F3_warm": (f"aevalsrc='{note(523.25, 0, 13, 0.3)}+0.9*{note(783.99, 0.1, 11, 0.3)}':d=0.7:s={RATE}",
                "lowpass=f=4000,chorus=0.7:0.9:35:0.3:0.25:2"),
    # A soft low knock with a small note on top.
    "F4_knock": (f"aevalsrc='sin(2*PI*(170*t-300*t*t))*exp(-t*38)*min(1,t*400)+0.6*{note(1174.66, 0.03, 18, 0.15)}':d=0.5:s={RATE}",
                 "lowpass=f=4500,aecho=0.8:0.5:70:0.15"),
}


# ---- finishing -------------------------------------------------------------------

def read(path):
    with wave.open(str(path), "rb") as w:
        assert w.getsampwidth() == 2
        ch = w.getnchannels()
        raw = array.array("h", w.readframes(w.getnframes()))
    left = [raw[i] / 32768 for i in range(0, len(raw), ch)]
    right = [raw[i + ch - 1] / 32768 for i in range(0, len(raw), ch)]
    return left, right


def finish(path, left, right):
    peak = max(max(abs(v) for v in left), max(abs(v) for v in right))
    if peak == 0:
        raise SystemExit(f"{path.name} is silent")
    loud = [i for i in range(len(left)) if max(abs(left[i]), abs(right[i])) > peak * 0.015]
    a, b = max(0, loud[0] - RATE // 500), loud[-1] + 1
    left, right = left[a:b], right[a:b]
    fade_out, fade_in = int(0.02 * RATE), RATE // 1000
    out = array.array("h")
    for i in range(len(left)):
        g = PEAK / peak * min(1.0, (len(left) - 1 - i) / fade_out) * min(1.0, i / fade_in)
        out.append(int(round(left[i] * g * 32767)))
        out.append(int(round(right[i] * g * 32767)))
    data = out.tobytes()
    header = b"RIFF" + struct.pack("<I", 36 + len(data)) + b"WAVEfmt " + struct.pack(
        "<IHHIIHH", 16, 1, 2, RATE, RATE * 4, 4, 16
    ) + b"data" + struct.pack("<I", len(data))
    path.write_bytes(header + data)
    rms = math.sqrt(sum(v * v for v in out) / len(out)) / 32767
    print(f"{path.name}: {len(left) * 1000 // RATE} ms, rms {rms:.3f}, {len(data) // 1024} KB")


def main():
    raw, dst = Path(sys.argv[1]), Path(sys.argv[2])
    sclang = sys.argv[3] if len(sys.argv) > 3 else "sclang"
    ffmpeg = sys.argv[4] if len(sys.argv) > 4 else "ffmpeg"
    raw.mkdir(parents=True, exist_ok=True)
    dst.mkdir(parents=True, exist_ok=True)

    for name, build in OWN.items():
        left, right = build()
        finish(dst / f"{name}.wav", left, right)

    here = Path(__file__).parent
    done = subprocess.run([sclang, str(here / "candidates.scd"), raw.as_posix()], capture_output=True, text=True, timeout=180)
    if "KLIP-SOUNDS done" not in done.stdout:
        print(done.stdout[-3000:])
        raise SystemExit("SuperCollider did not finish")
    for path in sorted(raw.glob("S*.raw.wav")):
        finish(dst / path.name.replace(".raw", ""), *read(path))

    for name, (source, effects) in FFMPEG.items():
        tmp = raw / f"{name}.raw.wav"
        cmd = [ffmpeg, "-y", "-v", "error", "-f", "lavfi", "-i", source, "-af", effects + ",apad=pad_dur=0.4",
               "-ac", "2", "-ar", str(RATE), "-c:a", "pcm_s16le", str(tmp)]
        done = subprocess.run(cmd, capture_output=True, text=True, timeout=60)
        if done.returncode != 0:
            raise SystemExit(f"{name}: {done.stderr}")
        finish(dst / f"{name}.wav", *read(tmp))


if __name__ == "__main__":
    main()
