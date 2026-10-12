"""Builds the sounds of Klip from scratch. Standard library only, nothing is played.

    python make.py variants      three candidates for "clip saved" into ./variants
    python make.py               the whole family into this folder

The character: soft and round, no metal, no noise. Every sound is made of "drops": a
pure tone whose pitch slides up quickly while it fades, like a bubble. A weak second
harmonic gives it body, a low pass takes the edge off.
"""

import array
import math
import struct
import sys
from pathlib import Path

RATE = 44100
PEAK = 0.5  # of full scale, the overlay scales further down with its volume


def drop(start, f0, f1, glide=0.05, attack=0.005, decay=0.11, level=1.0, harm=0.12):
    """One drop: (start in s, samples). Pitch goes from f0 to f1 within `glide`."""
    n = int((attack + decay * 6) * RATE)
    out, phase = [], 0.0
    for i in range(n):
        t = i / RATE
        k = min(1.0, t / glide)
        freq = f0 * (f1 / f0) ** (1 - (1 - k) ** 2)  # fast at first, then settling
        phase += 2 * math.pi * freq / RATE
        env = min(1.0, t / attack) * math.exp(-max(0.0, t - attack) / decay)
        out.append((math.sin(phase) + harm * math.sin(2 * phase)) * env * level)
    return start, out


def mix(parts, lowpass=3200.0):
    total = max(int(s * RATE) + len(x) for s, x in parts)
    buf = [0.0] * total
    for s, x in parts:
        at = int(s * RATE)
        for i, v in enumerate(x):
            buf[at + i] += v
    # One pole low pass, twice, for a round top end.
    a = 1 - math.exp(-2 * math.pi * lowpass / RATE)
    for _ in range(2):
        y = 0.0
        for i, v in enumerate(buf):
            y += a * (v - y)
            buf[i] = y
    # Cut the tail where it is inaudible, fade the last 15 ms.
    peak = max(abs(v) for v in buf)
    end = max(i for i, v in enumerate(buf) if abs(v) > peak * 0.02) + 1
    buf = buf[:end]
    fade = int(0.015 * RATE)
    return [v / peak * PEAK * min(1.0, (len(buf) - 1 - i) / fade) for i, v in enumerate(buf)]


def write(path: Path, samples):
    data = array.array("h", (int(round(v * 32767)) for v in samples)).tobytes()
    header = b"RIFF" + struct.pack("<I", 36 + len(data)) + b"WAVEfmt " + struct.pack(
        "<IHHIIHH", 16, 1, 1, RATE, RATE * 2, 2, 16
    ) + b"data" + struct.pack("<I", len(data))
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(header + data)
    print(f"{path.name}: {len(samples) * 1000 // RATE} ms, {len(data) // 1024} KB")


# Candidates for "clip saved".
VARIANTS = {
    # A: one soft pop.
    "A-pop": [drop(0, 420, 780, glide=0.06, decay=0.07)],
    # B: two pops, the second one higher. The closest to a "saved" gesture.
    "B-double-pop": [drop(0, 440, 660, decay=0.055), drop(0.085, 587, 988, decay=0.075, level=0.95)],
    # C: two soft mallet notes without a slide, a little longer.
    "C-mallet": [
        drop(0, 523.25, 523.25, attack=0.003, decay=0.10, harm=0.25),
        drop(0.10, 783.99, 783.99, attack=0.003, decay=0.14, harm=0.2, level=0.9),
    ],
}

FAMILY = {
    "clip": VARIANTS["B-double-pop"],
    # Automatic clip: three pops, rising.
    "auto": [
        drop(0, 440, 660, decay=0.05),
        drop(0.075, 587, 880, decay=0.055, level=0.95),
        drop(0.15, 784, 1175, decay=0.085, level=0.9),
    ],
    # Recording started: one longer drop that rises far.
    "rec_start": [drop(0, 300, 620, glide=0.12, decay=0.12)],
    # Recording saved: two pops, falling.
    "rec_stop": [drop(0, 740, 620, decay=0.055), drop(0.09, 520, 390, glide=0.08, decay=0.09)],
    # Screenshot: one very short high pop.
    "shot": [drop(0, 900, 1250, glide=0.025, decay=0.035), drop(0.045, 1250, 1500, glide=0.02, decay=0.03, level=0.5)],
    # Marker, a switch: the smallest one.
    "tick": [drop(0, 700, 860, glide=0.02, decay=0.03, level=0.8)],
}

if __name__ == "__main__":
    here = Path(__file__).parent
    if sys.argv[1:] == ["variants"]:
        for name, parts in VARIANTS.items():
            write(here / "variants" / f"clip-{name}.wav", mix(parts))
    else:
        for name, parts in FAMILY.items():
            write(here / f"{name}.wav", mix(parts))
