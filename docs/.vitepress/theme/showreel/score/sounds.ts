// The score's sound palette: struck, plucked, and blown voices shared by
// every section, the drums and bass the groove is built from, and the pads.

import { type Curve, hold, hz, line, type Mix, type NoiseKind, perc, type Pt, sweep, type VoiceOpts, warmOf, X } from "./mix";

// Sound palette. Lowpass and highpass Q values are resonance in dB, as the
// Web Audio spec defines them; bandpass Q is the usual bandwidth ratio.

/** A small struck tone with a quick inharmonic shimmer on top. */
export function ping(m: Mix, t: number, f: number, vel: number, len: number, o: VoiceOpts = {}): void {
  const v = m.voice(perc(t, vel, 0.001, len), o);
  if (!v) return;
  v.osc("sine", f);
  v.osc("sine", f * 2.76, perc(t, 0.25, 0.0005, len * 0.3));
}

/** A tiny mechanical click with a pitched body. */
export function tick(m: Mix, t: number, f: number, vel: number, pan = 0, send = 0.06): void {
  const v = m.voice(perc(t, 0.7 * vel, 0.0004, 0.035), { pan, send });
  if (!v) return;
  v.noise("white", 1, v.filter("bandpass", f * 2.2, 2.2), 1, t + 0.05);
  v.osc("sine", f, perc(t, 0.86, 0.0004, 0.018));
}

/** A weighty low hit: a sine that drops in pitch, with a soft noise slap. */
export function thump(m: Mix, t: number, vel: number, f0: number, f1: number, len: number, o: VoiceOpts = {}): void {
  const v = m.voice(perc(t, 0.6 * vel, 0.0015, len), o);
  if (!v) return;
  v.osc("sine", [[t, f0], [t + 0.045, f1 * 1.25, "exp"], [t + len, f1, "exp"]]);
  v.noise("white", perc(t, 0.3, 0.0005, 0.035), v.filter("lowpass", 2400, -3), 1, t + 0.05);
}

/** A cork-like pop: a sine that chirps up to its pitch. */
export function pop(m: Mix, t: number, f: number, vel: number, pan = 0, send = 0.18): void {
  const v = m.voice(perc(t, vel, 0.0015, 0.17), { pan, send });
  if (!v) return;
  v.osc("sine", [[t, f * 0.42], [t + 0.02, f, "exp"], [t + 0.12, f * 1.015, "exp"]]);
  v.osc("triangle", sweep(t, f * 0.84, t + 0.02, f * 2), perc(t, 0.14, 0.0008, 0.05));
}

/** A glassy bell: struck-bar partials over a bright transient. */
export function ding(m: Mix, t: number, f: number, vel: number, pan: number, len = 1.3, send = 0.38): void {
  const v = m.voice(perc(t, vel, 0.0008, len), { pan, send });
  if (!v) return;
  const partials = [[1, 1, 1], [2, 0.3, 0.55], [2.76, 0.2, 0.35], [5.4, 0.1, 0.18], [8.93, 0.05, 0.1]];
  for (const [ratio, a, k] of partials) {
    if (f * ratio > 16000) continue;
    v.osc("sine", f * ratio, k === 1 ? a : perc(t, a, 0.0005, len * k));
  }
  v.noise("white", perc(t, 0.12, 0.0005, 0.06), v.filter("highpass", 7000, 0), 1, t + 0.08);
}

/** Glass on glass: inharmonic partials, short and bright. */
export function clink(m: Mix, t: number, vel: number, pan: number): void {
  const v = m.voice(perc(t, vel, 0.0004, 0.6), { pan, send: 0.25 });
  if (!v) return;
  const base = hz(98);
  for (const [ratio, a, len] of [[1, 1, 0.6], [1.59, 0.55, 0.35], [2.21, 0.4, 0.22], [2.93, 0.28, 0.14]]) {
    v.osc("sine", base * ratio, perc(t, a, 0.0004, len));
  }
  v.noise("white", perc(t, 0.5, 0.0003, 0.008), v.filter("highpass", 6000, 0), 1, t + 0.02);
}

/** Air: noise through a moving band. Sustained, so a mid-sweep start still hears it. */
export function whoosh(m: Mix, env: readonly Pt[], band: Curve, q: number, o: VoiceOpts = {}, kind: NoiseKind = "pink"): void {
  const v = m.voice(env, { hold: true, ...o });
  if (v) v.noise(kind, 1, v.filter("bandpass", band, q));
}

/**
 * A liquid bloop: a sine that chirps up into its pitch, with an octave ring
 * and overtones rung through a resonant filter that opens over them. The
 * chirp is short and the upper layers carry the surface, so a run of bloops
 * reads as liquid rather than a low-mid hum: a plink rising through 1 to 3
 * kHz as the bubble breaks keeps it wet on small speakers.
 */
export function bloop(m: Mix, t: number, f: number, vel: number, pan: number, len = 0.22): void {
  const v = m.voice(perc(t, 1.2 * vel, 0.005, len), { pan, send: 0.3 });
  if (!v) return;
  const rise = sweep(t, f * 0.7, t + 0.025, f);
  v.osc("sine", rise, 0.7);
  v.osc("sine", sweep(t, f * 1.4, t + 0.025, f * 2), perc(t, 0.45, 0.003, len * 0.45));
  // The formant opens as the bubble surfaces and closes as it rings out.
  const lp = v.filter("lowpass", [[t, f * 1.2], [t + 0.04, f * 8, "exp"], [t + len, f * 2, "exp"]], 10);
  v.osc("sawtooth", rise, 0.32, lp);
  const top = Math.min(3200, f * 4.2);
  v.osc("sine", sweep(t + 0.004, top * 0.55, t + 0.03, top), perc(t + 0.004, 0.5, 0.002, 0.06));
  v.osc("triangle", sweep(t + 0.004, top * 0.8, t + 0.03, top * 1.3), perc(t + 0.004, 0.14, 0.002, 0.035));
}

/** A cardboard knock with a pitched hollow body. */
export function knock(m: Mix, t: number, f: number, vel: number, pan: number, send = 0.12): void {
  const v = m.voice(perc(t, vel, 0.0008, 0.2), { pan, send });
  if (!v) return;
  v.noise("white", perc(t, 0.8, 0.0005, 0.06), v.filter("bandpass", 1300 + f, 1.3), 1, t + 0.08);
  v.osc("triangle", sweep(t, f * 2, t + 0.02, f), perc(t, 0.7, 0.001, 0.15));
  v.osc("sine", sweep(t, f, t + 0.05, f * 0.5), 0.45);
}

// Drums and bass for the groove.

export function kick(m: Mix, t: number, vel: number, filtered = false): void {
  const v = m.voice(perc(t, 0.5 * vel, 0.002, 0.46), { bus: filtered ? "music" : "drums" });
  if (!v) return;
  const into = filtered ? v.filter("lowpass", 160, -3) : v.amp;
  v.osc("sine", [[t, 165], [t + 0.032, 62, "exp"], [t + 0.3, 45, "exp"]], 1, into);
  if (!filtered) v.noise("white", perc(t, 0.22, 0.0004, 0.012), v.filter("highpass", 2500, 0), 1, t + 0.02);
}

export function clap(m: Mix, t: number, vel: number): void {
  // Three hands a few milliseconds apart, then the room.
  const env: Pt[] = [
    [t, 0],
    [t + 0.0008, 0.8 * vel],
    [t + 0.009, 0.15 * vel, "exp"],
    [t + 0.0098, 0.75 * vel],
    [t + 0.018, 0.15 * vel, "exp"],
    [t + 0.0188, 0.9 * vel],
    [t + 0.05, 0.28 * vel, "exp"],
    [t + 0.2, 0.0001, "exp"],
    [t + 0.204, 0],
  ];
  const v = m.voice(env, { bus: "drums", send: 0.2, pan: -0.05 });
  if (!v) return;
  v.noise("white", 1.6, v.filter("highpass", 700, 0, v.filter("bandpass", 1500, 1.1)));
  v.osc("sine", 1900, perc(t, 0.3, 0.0005, 0.012));
}

export function hat(m: Mix, t: number, vel: number): void {
  const v = m.voice(perc(t, 0.28 * vel, 0.0008, 0.05), { bus: "drums", pan: 0.22 });
  if (v) v.noise("white", 1, v.filter("highpass", 7200, 0));
}

/**
 * One bar of the groove from `t0`, in sixteenths: kicks and claps where
 * given, a hat on every off-beat eighth, and soft ghost hats where given.
 * The kicks also pump the bass and pads.
 */
export function grooveBar(
  m: Mix,
  t0: number,
  kicks: readonly number[],
  claps: readonly number[],
  ghosts: readonly number[] = [],
): void {
  for (const s of kicks) {
    m.kick(t0 + s * X);
    kick(m, t0 + s * X, s % 8 === 0 ? 1 : 0.7);
  }
  for (const s of claps) {
    clap(m, t0 + s * X, 1);
  }
  for (let s = 0; s < 16; s++) {
    if (s % 4 === 2) {
      hat(m, t0 + s * X, 1);
    } else if (ghosts.includes(s)) {
      hat(m, t0 + s * X, 0.4);
    }
  }
}

/**
 * A run of bass notes as one voice: the oscillators keep their phase from
 * note to note, so repeated notes never cancel, and the envelope dips at each
 * change to articulate it. Notes are [start, end, MIDI note, velocity].
 * `grit` is a band-passed saw at the root that puts the line's third to
 * sixth harmonics (150 to 300 Hz) about 11 dB under the fundamental, so the
 * line survives on laptop and phone speakers that roll off below 200 Hz.
 */
export function bassRun(
  m: Mix,
  notes: readonly (readonly [number, number, number, number])[],
  level = 0.08,
  cutoff = 360,
  grit = 0.3,
): void {
  const env: Pt[] = [];
  const freq: Pt[] = [];
  notes.forEach(([a, b, n, vel], i) => {
    const peak = level * vel;
    if (i === 0) {
      env.push([a, 0]);
      freq.push([a, hz(n)]);
    } else {
      env.push([a, 0.3 * peak]);
      freq.push([a, hz(n), "set"]);
    }
    env.push([a + 0.012, peak], [b - 0.02, 0.82 * peak]);
  });
  const end = notes[notes.length - 1][1];
  env.push([end, 0.0001, "exp"], [end + 0.004, 0]);
  const v = m.voice(env, { bus: "music", hold: true });
  if (!v) return;
  const lp = v.filter("lowpass", cutoff, -3);
  const sat = v.shaper(m.sh.sat, lp);
  v.osc("sine", freq, 0.7, sat);
  v.osc(
    "triangle",
    freq.map(([t, f, k]): Pt => [t, f * 2, k]),
    0.12,
    sat,
  );
  if (grit) v.osc("sawtooth", freq, grit, v.filter("bandpass", 210, 1.1, v.filter("lowpass", 420, -3)));
}

/** The groove's bass line, in sixteenths: [start, length, interval, velocity]. */
const BASS: readonly (readonly [number, number, number, number])[] = [
  [0, 6, 0, 1],
  [6, 2, 0, 1],
  [8, 6, 0, 1],
  [14, 2, 12, 0.7],
];

/**
 * One bar of the bass line from `t0` on MIDI note `root`, with an octave
 * pickup on the last sixteenth pair. The lower the root, the further its
 * harmonics sit from the 210 Hz band that carries them to small speakers,
 * so low roots want more `grit`.
 */
export function bassBar(m: Mix, t0: number, root: number, grit: number): void {
  bassRun(
    m,
    BASS.map(([s, len, iv, vel]): [number, number, number, number] => [t0 + s * X, t0 + (s + len) * X, root + iv, vel]),
    0.08,
    360,
    grit,
  );
}

/**
 * One pad chord: two detuned voices spread across the stereo field. `thin`
 * highpasses it and scoops 250 to 400 Hz, so a breakdown chord leaves the
 * low end to the downbeat and the low mids to the liquid.
 */
export function pad(
  m: Mix,
  t0: number,
  t1: number,
  notes: number[],
  level: number,
  cutoff: Curve,
  attack = 0.08,
  rel = 0.3,
  thin = 0,
): void {
  const each = level / notes.length;
  for (const side of [-1, 1]) {
    const v = m.voice(hold(t0, attack, each, t1, 0.9 * each, rel), { bus: "music", pan: side * 0.4, send: 0.3, hold: true });
    if (!v) continue;
    let into: AudioNode = v.amp;
    if (thin) {
      const scoop = v.filter("peaking", 320, 1, v.amp);
      scoop.gain.value = -5;
      into = v.filter("highpass", thin, -1, scoop);
    }
    const lp = v.filter("lowpass", cutoff, -1, into);
    for (const n of notes) v.osc(warmOf(m.ac, m.sh), hz(n), 1, lp, side * 7);
  }
}

// Voices the character and the end card share.

export function flick(m: Mix, t: number, pan: number, vel = 0.16): void {
  const v = m.voice(perc(t, 2.2 * vel, 0.002, 0.08), { pan, send: 0.12 });
  if (!v) return;
  v.noise("white", 1, v.filter("bandpass", sweep(t, 2200, t + 0.05, 7000), 2.5));
  v.osc("sine", sweep(t, 1400, t + 0.045, 3200), 0.35);
}

/** A jaw-harp spring: a resonant filter and the pitch wobbling together. */
export function boing(m: Mix, t: number, vel = 0.3, f = hz(50), len = 0.42): void {
  const v = m.voice(perc(t, vel, 0.004, len), { send: 0.15 });
  if (!v) return;
  const lp = v.filter("lowpass", 900, 9);
  const saw = v.osc("sawtooth", f, 0.5, lp);
  v.lfo("sine", 11, sweep(t, 700, t + len, 20), lp.frequency);
  v.lfo("sine", 11, sweep(t, f * 0.1, t + len, 0.3), saw.frequency);
  const s = v.osc("sine", sweep(t, f * 2.25, t + 0.05, f * 2), 0.35);
  v.lfo("sine", 11, sweep(t, f * 0.17, t + len, 0.5), s.frequency);
}

export function stamp(m: Mix, t: number, vel = 1, pan = 0.12): void {
  thump(m, t, 0.6 * vel, 190, 75, 0.22, { pan });
  const v = m.voice(perc(t, 0.4 * vel, 0.0006, 0.07), { pan, send: 0.15 });
  if (v) v.noise("white", 1, v.filter("bandpass", 1500, 0.9), 1, t + 0.1);
  const p = m.voice(perc(t + 0.004, 0.12 * vel, 0.0005, 0.03), { pan: pan + 0.08 });
  if (p) p.noise("white", 1, p.filter("highpass", 5500, 0), 1, t + 0.05);
}

/** Fabric spinning: a sharp first flap, then flaps gated fast, slowing as the spring settles. */
export function flutter(m: Mix, t: number, len = 0.2, vel = 0.3): void {
  const snap = m.voice(perc(t, 3 * vel, 0.0006, 0.03), { send: 0.12, pan: -0.2 });
  if (snap) {
    snap.noise("white", 1, snap.filter("bandpass", 2400, 0.9), 1, t + 0.04);
    snap.osc("sine", sweep(t, 900, t + 0.025, 380), 0.35);
  }
  const v = m.voice(hold(t + 0.004, 0.012, vel * 4, t + len * 0.5, vel * 3, len * 0.5), {
    send: 0.12,
    pan: line(t, -0.2, t + len, 0.2),
  });
  if (!v) return;
  const bp = v.filter("bandpass", sweep(t, 1600, t + len, 900), 1.4);
  const am = v.vca(0.5, bp);
  v.lfo("square", sweep(t, 34, t + len, 12), 0.45, am.gain);
  v.noise("pink", 1, am);
}

export function blip(m: Mix, t: number, f: number, vel: number): void {
  const v = m.voice(perc(t, 1.6 * vel, 0.001, 0.035), { send: 0.1 });
  if (v) v.osc("sine", sweep(t, f * 1.3, t + 0.02, f));
}
