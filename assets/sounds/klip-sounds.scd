// The sounds of Klip, built in SuperCollider and rendered without playing anything.
//
//   sclang klip-sounds.scd <output folder>
//
// Each sound is one SynthDef. The server runs in non real time mode and writes a WAV,
// no audio device is opened. `finish.py` trims and levels the files afterwards.
//
// The family: a short bright pluck (two operator FM) over a soft low knock. Every sound
// uses the same two building blocks, so they belong together, and each one moves in its
// own direction: up for "saved" and "started", down for "stopped".

(
var out = thisProcess.argv[0] ? ".";
var rate = 44100;
var sounds, renderNext;

// A bell like pluck. `at` is its start in seconds, `bright` the depth of the modulation.
var pluck = { |freq, at, len = 0.22, bright = 1.4, ratio = 2|
	var trig = TDelay.kr(Impulse.kr(0), at);
	var index = EnvGen.kr(Env.perc(0.001, len * 0.4), trig) * bright;
	PMOsc.ar(freq, freq * ratio, index) * EnvGen.kr(Env.perc(0.003, len, curve: -5), trig);
};

// The low knock under the first note.
var knock = { |from = 170, to = 70, at = 0, len = 0.09|
	var trig = TDelay.kr(Impulse.kr(0), at);
	SinOsc.ar(EnvGen.kr(Env([from, from, to], [0, len], \exp), trig)) * EnvGen.kr(Env.perc(0.001, len), trig);
};

// A tiny click of filtered noise that gives the attack its edge.
var tick = { |at = 0, cut = 6000, len = 0.012|
	var trig = TDelay.kr(Impulse.kr(0), at);
	HPF.ar(WhiteNoise.ar, cut) * EnvGen.kr(Env.perc(0.0005, len), trig);
};

// Ends every sound: a short tail of air, a soft ceiling, and silence at both ends.
var finish = { |sig, dur|
	var wet = sig + (AllpassN.ar(sig, 0.05, [0.019, 0.031], 0.18).sum * 0.22);
	(wet * EnvGen.kr(Env.linen(0.001, dur - 0.04, 0.035), doneAction: 2) * 0.6).tanh;
};

sounds = [
	// Clip saved: two notes a fourth apart, rising.
	[\clip, 0.46, {
		var sig = (pluck.(783.99, 0.0) * 0.5) + (pluck.(1046.5, 0.075, 0.30, 1.7) * 0.55)
			+ (knock.() * 0.45) + (tick.() * 0.2);
		Out.ar(0, finish.(sig, 0.46));
	}],
	// Automatic clip: the same two notes with a third one on top, a little brighter.
	[\auto, 0.60, {
		var sig = (pluck.(783.99, 0.0, 0.18) * 0.45) + (pluck.(1046.5, 0.065, 0.2, 1.6) * 0.45)
			+ (pluck.(1567.98, 0.13, 0.38, 2.0) * 0.5)
			+ (knock.() * 0.45) + (tick.() * 0.2) + (tick.(0.13, 9000, 0.03) * 0.08);
		Out.ar(0, finish.(sig, 0.60));
	}],
	// Recording started: one low note that slides up, firm.
	[\rec_start, 0.40, {
		var glide = EnvGen.kr(Env([440, 440, 659.25], [0, 0.11], \exp));
		var body = PMOsc.ar(glide, glide * 2, EnvGen.kr(Env.perc(0.001, 0.12)) * 1.2)
			* EnvGen.kr(Env.perc(0.004, 0.30, curve: -4));
		var sig = (body * 0.6) + (knock.(200, 80) * 0.55) + (tick.() * 0.18);
		Out.ar(0, finish.(sig, 0.40));
	}],
	// Recording stopped: the same two notes the other way round, falling, softer.
	[\rec_stop, 0.44, {
		var sig = (pluck.(659.25, 0.0, 0.2, 1.1) * 0.5) + (pluck.(440, 0.085, 0.3, 0.9) * 0.55)
			+ (knock.(140, 60, 0.085, 0.11) * 0.45) + (tick.() * 0.14);
		Out.ar(0, finish.(sig, 0.44));
	}],
	// Screenshot: a shutter, two quick clicks and a short high ping.
	[\shot, 0.26, {
		var sig = (tick.(0.0, 2500, 0.02) * 0.5) + (tick.(0.045, 4000, 0.03) * 0.4)
			+ (pluck.(1975.53, 0.045, 0.12, 0.8, 3) * 0.22) + (knock.(240, 120, 0, 0.04) * 0.3);
		Out.ar(0, finish.(sig, 0.26));
	}],
	// Everything small (marker, a switch): one short note, quiet.
	[\tick, 0.20, {
		var sig = (pluck.(1318.51, 0.0, 0.11, 0.9) * 0.4) + (tick.(0, 7000, 0.008) * 0.12);
		Out.ar(0, finish.(sig, 0.20));
	}]
];

renderNext = { |i|
	if(i >= sounds.size) {
		"KLIP-SOUNDS done".postln;
		0.exit;
	} {
		var name = sounds[i][0], dur = sounds[i][1];
		var def = SynthDef(name, sounds[i][2]);
		var score = [
			[0.0, [\d_recv, def.asBytes]],
			[0.02, [\s_new, name, 1000, 0, 0]],
			[dur + 0.12, [\c_set, 0, 0]]
		];
		var options = ServerOptions.new.numOutputBusChannels_(1).numInputBusChannels_(0);
		Score.recordNRT(
			score,
			out +/+ (name ++ ".osc"),
			out +/+ (name ++ ".raw.wav"),
			sampleRate: rate,
			headerFormat: "WAV",
			sampleFormat: "int16",
			options: options,
			duration: dur + 0.12,
			action: {
				("KLIP-SOUNDS rendered " ++ name).postln;
				renderNext.(i + 1);
			}
		);
	};
};

renderNext.(0);
)
