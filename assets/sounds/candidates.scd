// Candidates for "clip saved", built in SuperCollider. Rendered without playing anything:
//
//   sclang candidates.scd <output folder>
//
// Four different characters, all soft: a wooden bar, a plucked string, a short breath
// that ends in a drop, and a small glass chord.

(
var out = thisProcess.argv[0] ? ".";
var rate = 44100;
var sounds, renderNext;

var start = { |at| TDelay.ar(Impulse.ar(0), at) };
var startK = { |at| TDelay.kr(Impulse.kr(0), at) };

// A struck wooden bar: a resonator with the overtones of a marimba, hit by a soft mallet.
var bar = { |freq, at, amp = 1|
	var mallet = LPF.ar(Decay2.ar(start.(at), 0.002, 0.012) * PinkNoise.ar(0.6), 2500) + (start.(at) * 0.3);
	Klank.ar(`[[1, 3.93, 9.2], [1, 0.22, 0.05], [0.32, 0.09, 0.04]], mallet, freq) * amp;
};

// A plucked string.
var string = { |freq, at, amp = 1|
	LPF.ar(Pluck.ar(PinkNoise.ar(0.7), startK.(at), 0.02, freq.reciprocal, 0.5, 0.42), 3800) * amp;
};

// A soft glass tone: three sines close together, slow attack.
var glass = { |freq, at, amp = 1|
	var env = EnvGen.kr(Env.perc(0.018, 0.34, curve: -4), startK.(at));
	Mix(SinOsc.ar(freq * [1, 1.003, 2.002], 0, [0.5, 0.5, 0.12])) * env * amp;
};

var room = { |sig, mix = 0.22, size = 0.55|
	FreeVerb2.ar(sig, sig, mix, size, 0.6);
};

var done = { |sig, dur|
	sig * EnvGen.kr(Env.linen(0.001, dur - 0.06, 0.05), doneAction: 2);
};

sounds = [
	[\S1_wood, 0.9, {
		var sig = bar.(659.25, 0.0) + bar.(987.77, 0.095, 0.9);
		Out.ar(0, done.(room.(sig * 0.5, 0.18), 0.9));
	}],
	[\S2_string, 1.0, {
		var sig = string.(392.0, 0.0) + string.(587.33, 0.085, 0.9);
		Out.ar(0, done.(room.(sig, 0.2), 1.0));
	}],
	[\S3_breath, 0.7, {
		// The breath rises for 150 ms, the drop lands at its end.
		var breath = BPF.ar(PinkNoise.ar, EnvGen.kr(Env([350, 2600], [0.16], \exp)), 0.7)
			* EnvGen.kr(Env([0, 0.5, 0], [0.15, 0.04], [3, -3]));
		var drop = SinOsc.ar(EnvGen.kr(Env([480, 480, 880], [0.15, 0.05], \exp)))
			* EnvGen.kr(Env([0, 0, 1, 0], [0.15, 0.004, 0.16], [0, 0, -5]));
		Out.ar(0, done.(room.((breath * 0.7) + (drop * 0.6), 0.2), 0.7));
	}],
	[\S4_glass, 1.0, {
		var sig = glass.(659.25, 0.0) + glass.(987.77, 0.10, 0.9) + glass.(1318.51, 0.10, 0.35);
		Out.ar(0, done.(room.(sig * 0.5, 0.3, 0.7), 1.0));
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
			[dur + 0.1, [\c_set, 0, 0]]
		];
		Score.recordNRT(
			score,
			out +/+ (name ++ ".osc"),
			out +/+ (name ++ ".raw.wav"),
			sampleRate: rate,
			headerFormat: "WAV",
			sampleFormat: "int16",
			options: ServerOptions.new.numOutputBusChannels_(2).numInputBusChannels_(0),
			duration: dur + 0.1,
			action: {
				("KLIP-SOUNDS rendered " ++ name).postln;
				renderNext.(i + 1);
			}
		);
	};
};

renderNext.(0);
)
