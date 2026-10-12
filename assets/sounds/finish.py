"""Trims and levels the WAV files SuperCollider rendered, and prints what is in them.

    python finish.py <folder with *.raw.wav> <output folder>

Standard library only. Every file comes out as 16 bit mono 44.1 kHz with a plain 44 byte
header, silence cut off at both ends, a short fade at the end and the same peak level.
"""

import array
import math
import struct
import sys
import wave
from pathlib import Path

PEAK = 0.5  # of full scale. The overlay scales this down further with its volume.
FLOOR = 0.002  # below this a sample counts as silence


def read(path: Path) -> tuple[list[float], int]:
    with wave.open(str(path), "rb") as w:
        assert w.getsampwidth() == 2, "16 bit expected"
        rate, channels = w.getframerate(), w.getnchannels()
        raw = array.array("h", w.readframes(w.getnframes()))
    mono = [sum(raw[i : i + channels]) / channels / 32768.0 for i in range(0, len(raw), channels)]
    return mono, rate


def main() -> None:
    src, dst = Path(sys.argv[1]), Path(sys.argv[2])
    dst.mkdir(parents=True, exist_ok=True)
    for path in sorted(src.glob("*.raw.wav")):
        samples, rate = read(path)
        loud = [i for i, s in enumerate(samples) if abs(s) > FLOOR]
        if not loud:
            raise SystemExit(f"{path.name} is silent")
        # Keep two milliseconds before the first sound so the attack is not cut.
        samples = samples[max(0, loud[0] - rate // 500) : loud[-1] + 1]
        peak = max(abs(s) for s in samples)
        fade = rate // 100
        out = array.array("h")
        for i, s in enumerate(samples):
            tail = min(1.0, (len(samples) - 1 - i) / fade)
            head = min(1.0, i / (rate // 1000))
            out.append(int(round(s / peak * PEAK * tail * head * 32767)))
        name = path.name.replace(".raw.wav", ".wav")
        data = out.tobytes()
        header = b"RIFF" + struct.pack("<I", 36 + len(data)) + b"WAVEfmt " + struct.pack(
            "<IHHIIHH", 16, 1, 1, rate, rate * 2, 2, 16
        ) + b"data" + struct.pack("<I", len(data))
        (dst / name).write_bytes(header + data)

        # What is in the file, for a check without ears: length, level, and the loudest
        # frequency of the first and of the last third.
        rms = math.sqrt(sum(s * s for s in out) / len(out)) / 32767

        def loudest(part: array.array) -> int:
            best, best_power = 0, 0.0
            for f in range(100, 4000, 20):
                re = sum(part[n] * math.cos(2 * math.pi * f * n / rate) for n in range(0, len(part), 4))
                im = sum(part[n] * math.sin(2 * math.pi * f * n / rate) for n in range(0, len(part), 4))
                if re * re + im * im > best_power:
                    best, best_power = f, re * re + im * im
            return best

        third = len(out) // 3
        print(
            f"{name}: {len(out) * 1000 // rate} ms, {rate} Hz, rms {rms:.3f}, "
            f"starts around {loudest(out[:third])} Hz, ends around {loudest(out[-third:])} Hz, {len(data) // 1024} KB"
        )


if __name__ == "__main__":
    main()
