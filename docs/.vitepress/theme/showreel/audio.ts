// The reel's soundtrack, synthesized with the Web Audio API: oscillators,
// seeded noise, filters, and envelopes, with no samples and no network. The
// whole score is written in reel seconds and scheduled from any start point,
// so playback, seeking, and offline export hear the same mix. Randomness is
// seeded per event, so a mid-reel start plays the same sounds, on the same
// frames, that full playback plays from there.
//
// Sound design leads: every choreographed accent in the scenes has its own
// sound, tuned to D major where it has a pitch. Music supports it: a build in
// bar 1, a minimal groove with sub bass in bars 2 to 6 that ducks under the
// effects, a breakdown in bar 7 that drops back and rises into a sixteenth of
// silence, and the resolve on bar 8.
//
// Where a scene times its motion with a formula (the tape, the Data bars, the
// cube rain, the prune beam), the formula is rebuilt here and names the scene
// code it mirrors, so the sound lands where the picture does.

import { BAR, BEAT, DURATION, GRID_R, KEEP } from "./bible";
import { cubicBezier, hash, lerp, progress, rng, smoothstep, spring } from "./math";

export interface ScoreHandle {
  /** Fade out over ~30 ms and release every node. */
  stop(): void;
}

/** A sixteenth note. */
const X = BEAT / 4;
/** Global time of beat `n`. */
const bt = (n: number): number => n * BEAT;
/** Equal-tempered pitch of MIDI note `n`. */
const hz = (n: number): number => 440 * 2 ** ((n - 69) / 12);
/** One 60 fps frame, as the scenes count them. */
const FRAME = 1 / 60;
/** The first 60 fps frame at or after reel time `t`, where a scene's event first shows. */
const onFrame = (t: number): number => Math.ceil(t * 60 - 1e-9) / 60;
/** The breath before the resolve: nothing sounds from here to b28. */
const GAP = bt(27.75);

/**
 * DynamicsCompressorNode delays its output by a fixed 6 ms lookahead (Chromium,
 * WebKit, and Gecko share the same compressor kernel). Everything is scheduled
 * that much early so the mix leaves the graph on the frame.
 */
const LATENCY = 0.006;
/** A sound the clock has already passed by at most this much still plays whole, that much late. */
const LATE = 0.06;
/** How far ahead of a live context's clock anything is scheduled. */
const MARGIN = 0.015;
/** ...and the duck curves, which cannot start late without shifting whole. */
const CURVE_MARGIN = 0.06;
/** Seconds of score a live context gets before playScore returns; the rest follows on a timer. */
const FIRST = 0.4;
/** How far ahead of the clock the timer keeps the score built, and how often it runs. */
const AHEAD = 1.2;
const TICK_MS = 250;

// Automation. A curve is a constant or a list of points in reel seconds; each
// point's kind shapes the segment arriving at it. Exponential segments only
// ever run between positive values, and every envelope starts and ends at 0.

type Kind = "lin" | "exp" | "set";
type Pt = readonly [t: number, v: number, kind?: Kind];
type Curve = number | readonly Pt[];

function valueAt(pts: readonly Pt[], t: number): number {
  if (t <= pts[0][0]) return pts[0][1];
  for (let i = 1; i < pts.length; i++) {
    const [t1, v1, k = "lin"] = pts[i];
    if (t < t1) {
      const [t0, v0] = pts[i - 1];
      const u = (t - t0) / (t1 - t0);
      if (k === "set") return v0;
      if (k === "exp" && v0 > 0 && v1 > 0) return v0 * (v1 / v0) ** u;
      return v0 + (v1 - v0) * u;
    }
  }
  return pts[pts.length - 1][1];
}

/** Linear attack to `peak`, then an exponential fall of 80 dB over `len`. */
const perc = (t: number, peak: number, attack: number, len: number): Pt[] => [
  [t, 0],
  [t + attack, peak],
  [t + attack + len, 0.0001, "exp"],
  [t + attack + len + 0.004, 0],
];
/** Rise linearly to `peak` at `tp`, then fall away exponentially to silence at `t1`. */
const ad = (t0: number, tp: number, peak: number, t1: number): Pt[] => [
  [t0, 0],
  [tp, peak],
  [t1, 0.0001, "exp"],
  [t1 + 0.004, 0],
];
/** Attack to `peak`, move to `end` by `t1`, then release over `rel`. */
const hold = (t0: number, attack: number, peak: number, t1: number, end: number, rel: number): Pt[] => [
  [t0, 0],
  [t0 + attack, peak],
  [t1, end],
  [t1 + rel, 0.0001, "exp"],
  [t1 + rel + 0.004, 0],
];
/** Fade in to `a` at `ta`, swell exponentially to `peak` at `tp`, then cut away by `t1`. */
const swell = (t0: number, ta: number, a: number, tp: number, peak: number, t1: number): Pt[] => [
  [t0, 0],
  [ta, a],
  [tp, peak, "exp"],
  [t1, 0.0001, "exp"],
  [t1 + 0.004, 0],
];
/** An exponential sweep from `v0` at `t0` to `v1` at `t1` (pitches, cutoffs). */
const sweep = (t0: number, v0: number, t1: number, v1: number): Pt[] => [
  [t0, v0],
  [t1, v1, "exp"],
];
/** A straight line from `v0` at `t0` to `v1` at `t1` (pans). */
const line = (t0: number, v0: number, t1: number, v1: number): Pt[] => [
  [t0, v0],
  [t1, v1],
];

/** Exponential glide sampled along an easing curve. */
function glide(t0: number, t1: number, v0: number, v1: number, ease: (u: number) => number, n = 8): Pt[] {
  const out: Pt[] = [];
  for (let i = 0; i <= n; i++) {
    const u = i / n;
    out.push([t0 + (t1 - t0) * u, v0 * (v1 / v0) ** ease(u), "exp"]);
  }
  return out;
}
/** Inverse of an ease-out cubic: when a counter easing out passes fraction `p`. */
const outCubicInv = (p: number) => 1 - (1 - p) ** (1 / 3);
/** When a rising function `f` first reaches `v` after `t0` (bisection within `span`). */
function crossing(f: (t: number) => number, v: number, t0: number, span: number): number {
  let lo = t0;
  let hi = t0 + span;
  for (let i = 0; i < 40; i++) {
    const mid = (lo + hi) / 2;
    if (f(mid) >= v) hi = mid;
    else lo = mid;
  }
  return hi;
}

// Buffers and curves shared by every score on one context.

/** A shaper curve, typed as the constructor returns it so any TS DOM lib accepts it. */
const floats = (n: number) => new Float32Array(n);
type Floats = ReturnType<typeof floats>;

/**
 * Buffers are generated the first time a score needs them, so the first call
 * on a fresh context (a click on the sound button) only pays for white noise.
 */
interface Shared {
  noise: Partial<Record<NoiseKind, AudioBuffer>>;
  room: AudioBuffer | null;
  warm: PeriodicWave | null;
  sat: Floats;
  crush: Floats;
  ceiling: Floats;
}
type NoiseKind = "white" | "pink" | "crackle";

const sharedCache = new WeakMap<BaseAudioContext, Shared>();

type Fill = (r: () => number, d: Float32Array) => void;
const NOISE: Record<NoiseKind, readonly [seed: number, fill: Fill]> = {
  white: [
    1,
    (r, d) => {
      for (let i = 0; i < d.length; i++) d[i] = r() * 2 - 1;
    },
  ],
  // Paul Kellet's pink filter: warmer air for whooshes.
  pink: [
    2,
    (r, d) => {
      let b0 = 0;
      let b1 = 0;
      let b2 = 0;
      let peak = 0;
      for (let i = 0; i < d.length; i++) {
        const w = r() * 2 - 1;
        b0 = 0.99765 * b0 + w * 0.099046;
        b1 = 0.963 * b1 + w * 0.2965164;
        b2 = 0.57 * b2 + w * 1.0526913;
        d[i] = b0 + b1 + b2 + w * 0.1848;
        peak = Math.max(peak, Math.abs(d[i]));
      }
      for (let i = 0; i < d.length; i++) d[i] /= peak;
    },
  ],
  // Sparse, ringing clicks: tape adhesive letting go, cardboard crumbling.
  crackle: [
    3,
    (r, d) => {
      for (let i = 0; i < d.length - 16; i++) {
        if (r() < 0.004) {
          const a = (0.3 + 0.7 * r()) * (r() < 0.5 ? -1 : 1);
          for (let k = 0; k < 14; k++) d[i + k] += a * Math.exp(-k / 3) * (k % 2 ? -0.6 : 1);
        }
      }
    },
  ],
};

/** Two seconds of seeded noise, looped by every voice that plays it. */
function noiseOf(ac: BaseAudioContext, sh: Shared, kind: NoiseKind): AudioBuffer {
  let buf = sh.noise[kind];
  if (!buf) {
    const [seed, fill] = NOISE[kind];
    buf = ac.createBuffer(1, Math.floor(ac.sampleRate * 2), ac.sampleRate);
    fill(rng(seed), buf.getChannelData(0));
    sh.noise[kind] = buf;
  }
  return buf;
}

/** Pad wave: harmonics falling faster than a saw's, so it glows instead of buzzing. */
function warmOf(ac: BaseAudioContext, sh: Shared): PeriodicWave {
  if (!sh.warm) {
    const H = 40;
    const real = new Float32Array(H);
    const imag = new Float32Array(H);
    for (let k = 1; k < H; k++) imag[k] = (1 / k) ** 1.45 * (k % 2 ? 1 : 0.7);
    sh.warm = ac.createPeriodicWave(real, imag);
  }
  return sh.warm;
}

function roomOf(ac: BaseAudioContext, sh: Shared): AudioBuffer {
  sh.room ??= roomImpulse(ac);
  return sh.room;
}

/** A warm hall: decorrelated noise with early reflections that darkens as it decays. */
function roomImpulse(ac: BaseAudioContext): AudioBuffer {
  const sr = ac.sampleRate;
  const len = 2.2;
  const n = Math.floor(sr * len);
  const buf = ac.createBuffer(2, n, sr);
  for (let ch = 0; ch < 2; ch++) {
    const r = rng(0x5eed + ch * 977);
    const d = buf.getChannelData(ch);
    // The two exponentials advance by a fixed ratio per sample.
    const kDark = Math.exp(-1 / (0.35 * sr));
    const kDecay = Math.exp(-6.9 / (1.7 * sr));
    const fade0 = Math.floor(sr * (len - 0.25));
    let dark = 1;
    let decay = 1;
    let lp = 0;
    for (let i = 0; i < n; i++) {
      const t = i / sr;
      const pre = t < 0.011 ? 0 : Math.min(1, (t - 0.011) / 0.006);
      const a = 0.06 + 0.7 * dark;
      lp += a * (r() * 2 - 1 - lp);
      const tailOut = i < fade0 ? 1 : 1 - smoothstep(len - 0.25, len, t);
      d[i] = lp * decay * pre * tailOut * (1.2 - a);
      dark *= kDark;
      decay *= kDecay;
    }
    for (let k = 0; k < 14; k++) {
      const i = Math.floor(sr * (0.011 + r() * 0.075));
      d[i] += (r() * 2 - 1) * 0.6 * (1 - k / 16);
    }
  }
  return buf;
}

function shared(ac: BaseAudioContext): Shared {
  let s = sharedCache.get(ac);
  if (s) return s;
  const curve = (n: number, f: (x: number) => number) => {
    const c = floats(n);
    for (let i = 0; i < n; i++) c[i] = f((i / (n - 1)) * 2 - 1);
    return c;
  };
  s = {
    noise: {},
    room: null,
    warm: null,
    sat: curve(2048, (x) => Math.tanh(1.8 * x) / Math.tanh(1.8)),
    crush: curve(2048, (x) => Math.round(x * 5) / 5),
    // Final safety: unity below 0.6, then a smooth knee that never passes
    // 0.88 (-1.1 dBFS). The domain is ±2 so the knee has room.
    ceiling: curve(8192, (x) => {
      const a = Math.abs(x * 2);
      const k = 0.6;
      const c = 0.88;
      return Math.sign(x) * (a <= k ? a : k + (c - k) * Math.tanh((a - k) / (c - k)));
    }),
  };
  sharedCache.set(ac, s);
  return s;
}

// The mix: buses, time mapping, voices.

type Bus = "sfx" | "drums" | "music";

interface VoiceOpts {
  bus?: Bus;
  pan?: Curve;
  /** Reverb send level. */
  send?: number;
  /** Sustained sound: if playback starts inside it, enter already sounding with a short fade-in. */
  hold?: boolean;
}

/** True when `env` still rings 150 ms above -30 dB (of its peak) after `t`. */
function rings(env: readonly Pt[], t: number): boolean {
  let peak = 0;
  for (const p of env) peak = Math.max(peak, p[1]);
  return valueAt(env, t + 0.15) > 0.0316 * peak;
}

/**
 * The score is built in passes, each creating the voices whose first sound
 * falls in its window. Every pass runs the same composition, so a voice is
 * the same whichever pass builds it. The first pass of the first score on a
 * page also records the ducks and the kicks (see Plan).
 */
class Mix {
  readonly sources: AudioScheduledSourceNode[] = [];
  /** Effect accents the music ducks under: time, depth, release. */
  readonly ducks: Dip[] = [];
  /** Groove kicks, for the bass and pad pump. */
  readonly kicks: number[] = [];
  /** This pass builds voices whose first sound falls in [lo, hi). */
  lo = -Infinity;
  hi = Infinity;
  collect = true;
  /** Reel time the context clock has passed (with a margin): nothing can start before it. */
  floor: number;
  private readonly strips = new Map<string, AudioNode>();

  constructor(
    readonly ac: BaseAudioContext,
    readonly sh: Shared,
    readonly from: number,
    readonly when: number,
    readonly buses: Record<Bus, AudioNode>,
    readonly verb: AudioNode,
    /** A real-time context, whose clock keeps running while a pass builds. */
    readonly live = false,
  ) {
    this.floor = from;
  }

  /**
   * Context time of reel time `t`, pinned to a sample frame so any start
   * point lands every source on the same frame as full playback does. The
   * time sits half a frame before it: Chromium rounds a start time up to the
   * next frame, and a time exactly on a frame lands a frame late whenever
   * float error puts it a hair past, which differs between start points and
   * moves sounds that share a noise stream against each other.
   */
  at(t: number): number {
    const sr = this.ac.sampleRate;
    return Math.max(0, (Math.round((this.when + (t - this.from) - LATENCY) * sr) - 0.5) / sr);
  }

  /** The earliest reel time still safe to schedule, `margin` seconds ahead of the context clock. */
  clock(margin: number): number {
    return Math.max(this.from, this.from + this.ac.currentTime + margin + LATENCY - this.when);
  }

  duck(t: number, depth: number, release = 0.15): void {
    if (this.collect) this.ducks.push([t, depth, release]);
  }

  kick(t: number): void {
    if (this.collect) this.kicks.push(t);
  }

  /**
   * A shared channel strip (static pan, reverb send) that voices mix into, so
   * a few dozen panners and send gains serve every voice.
   */
  strip(bus: Bus, pan: number, send: number): AudioNode {
    const p = Math.round(pan * 20) / 20;
    const s = Math.round(send * 50) / 50;
    const key = `${bus}|${p}|${s}`;
    let input = this.strips.get(key);
    if (!input) {
      input = this.ac.createGain();
      let out: AudioNode = input;
      if (p) {
        const sp = this.ac.createStereoPanner();
        sp.pan.value = p;
        out = input.connect(sp);
      }
      out.connect(this.buses[bus]);
      if (s) {
        const g = this.ac.createGain();
        g.gain.value = s;
        out.connect(g).connect(this.verb);
      }
      this.strips.set(key, input);
    }
    return input;
  }

  /**
   * Apply a curve to a param, entering mid-curve when it starts before
   * `enter`, and heard `shift` seconds late.
   */
  set(p: AudioParam, c: Curve, enter = this.from, shift = 0): void {
    if (typeof c === "number") {
      p.value = c;
      return;
    }
    let i = 0;
    if (c[0][0] >= enter) {
      p.setValueAtTime(c[0][1], this.at(c[0][0] + shift));
      i = 1;
    } else {
      while (i < c.length && c[i][0] <= enter) i++;
      p.setValueAtTime(valueAt(c, enter), this.at(enter + shift));
    }
    for (; i < c.length; i++) {
      const [t, v, k = "lin"] = c[i];
      const ct = this.at(t + shift);
      if (k === "exp") p.exponentialRampToValueAtTime(v, ct);
      else if (k === "set") p.setValueAtTime(v, ct);
      else p.linearRampToValueAtTime(v, ct);
    }
  }

  /**
   * A voice whose amplitude follows `env` (reel seconds), or null when it
   * belongs to another pass or cannot sound. A voice the clock has just
   * passed plays whole, a moment late; one that began long before (a
   * mid-reel start, a stalled timer) enters mid-sound with a short fade if
   * it is sustained or still ringing, and is skipped otherwise.
   */
  voice(env: readonly Pt[], o: VoiceOpts = {}): Voice | null {
    const t0 = env[0][0];
    const t1 = env[env.length - 1][0];
    if (t1 <= this.from || t0 >= DURATION) return null;
    const nominal = Math.max(t0, this.from);
    if (nominal < this.lo || nominal >= this.hi) return null;
    // A slow pass (a cold first call, a throttled tab) can fall behind the
    // clock partway through, so a live voice checks it again: a source that
    // starts late while its envelope runs on time would lose its attack.
    const floor = Math.max(this.floor, this.from, this.live ? this.clock(MARGIN) : -Infinity);
    let enter = t0;
    let shift = 0;
    if (t0 < floor) {
      if (t0 >= this.from - 1e-4 && floor - t0 <= LATE) shift = floor - t0;
      else if (o.hold || rings(env, floor)) enter = floor;
      else return null;
    }
    return new Voice(this, t0, t1, enter, shift, env, o);
  }
}

class Voice {
  readonly amp: GainNode;

  constructor(
    readonly m: Mix,
    readonly t0: number,
    readonly t1: number,
    readonly enter: number,
    readonly shift: number,
    env: readonly Pt[],
    o: VoiceOpts,
  ) {
    const ac = m.ac;
    this.amp = ac.createGain();
    this.set(this.amp.gain, env);
    let out: AudioNode = this.amp;
    if (enter > t0) {
      const fade = ac.createGain();
      m.set(fade.gain, line(enter, 0, enter + 0.03, 1), enter);
      out = out.connect(fade);
    }
    // Moving pans get their own panner; static ones share a strip.
    if (typeof o.pan === "object") {
      const p = ac.createStereoPanner();
      this.set(p.pan, o.pan);
      out = out.connect(p);
    }
    out.connect(m.strip(o.bus ?? "sfx", typeof o.pan === "number" ? o.pan : 0, o.send ?? 0));
  }

  set(p: AudioParam, c: Curve): void {
    this.m.set(p, c, this.enter, this.shift);
  }

  private run(src: AudioScheduledSourceNode, until: number, offset?: number, from = this.enter): void {
    const m = this.m;
    const start = m.at(Math.max(from, this.enter) + this.shift);
    if (offset === undefined) src.start(start);
    else (src as AudioBufferSourceNode).start(start, offset);
    src.stop(m.at(Math.max(until, this.enter) + this.shift) + 0.02);
    m.sources.push(src);
  }

  /** A gain stage into `into`; a plain 1 connects straight through. */
  gain(level: Curve, into: AudioNode = this.amp): AudioNode {
    if (level === 1) return into;
    const g = this.m.ac.createGain();
    this.set(g.gain, level);
    g.connect(into);
    return g;
  }

  /** A gain whose level an LFO can modulate (tremolo, flutter). */
  vca(base: number, into: AudioNode = this.amp): GainNode {
    const g = this.m.ac.createGain();
    g.gain.value = base;
    g.connect(into);
    return g;
  }

  osc(
    type: OscillatorType | PeriodicWave,
    freq: Curve,
    level: Curve = 1,
    into: AudioNode = this.amp,
    detune = 0,
  ): OscillatorNode {
    const o = this.m.ac.createOscillator();
    // PeriodicWave is an empty interface, so a string check cannot narrow it away.
    if (typeof type === "string") o.type = type as OscillatorType;
    else o.setPeriodicWave(type);
    this.set(o.frequency, freq);
    if (detune) o.detune.value = detune;
    o.connect(this.gain(level, into));
    this.run(o, this.t1);
    return o;
  }

  noise(kind: NoiseKind, level: Curve = 1, into: AudioNode = this.amp, rate: Curve = 1, until = this.t1): void {
    const s = this.m.ac.createBufferSource();
    s.buffer = noiseOf(this.m.ac, this.m.sh, kind);
    s.loop = true;
    this.set(s.playbackRate, rate);
    s.connect(this.gain(level, into));
    // A fixed read offset per event keeps every start point identical.
    const offset = hash(Math.round(this.t0 * 9973), kind.length) * 1.8;
    this.run(s, Math.min(until, this.t1), offset);
  }

  filter(type: BiquadFilterType, freq: Curve, q: Curve = 0.7, into: AudioNode = this.amp): BiquadFilterNode {
    const f = this.m.ac.createBiquadFilter();
    f.type = type;
    this.set(f.frequency, freq);
    this.set(f.Q, q);
    f.connect(into);
    return f;
  }

  shaper(curve: Floats, into: AudioNode = this.amp): WaveShaperNode {
    const w = this.m.ac.createWaveShaper();
    w.curve = curve;
    w.connect(into);
    return w;
  }

  /**
   * An oscillator modulating `target`. `cycle` gives the first whole-cycle
   * boundary at or after a time, so an LFO that keeps time with the beat can
   * wait for it when the voice enters mid-sound instead of starting off-grid.
   */
  lfo(type: OscillatorType, rate: Curve, depth: Curve, target: AudioParam, cycle?: (t: number) => number): void {
    const o = this.m.ac.createOscillator();
    o.type = type;
    this.set(o.frequency, rate);
    const g = this.m.ac.createGain();
    this.set(g.gain, depth);
    o.connect(g).connect(target);
    this.run(o, this.t1, undefined, cycle && this.enter > this.t0 ? cycle(this.enter) : this.enter);
  }
}

// Sound palette. Lowpass and highpass Q values are resonance in dB, as the
// Web Audio spec defines them; bandpass Q is the usual bandwidth ratio.

/** A small struck tone with a quick inharmonic shimmer on top. */
function ping(m: Mix, t: number, f: number, vel: number, len: number, o: VoiceOpts = {}): void {
  const v = m.voice(perc(t, vel, 0.001, len), o);
  if (!v) return;
  v.osc("sine", f);
  v.osc("sine", f * 2.76, perc(t, 0.25, 0.0005, len * 0.3));
}

/** A tiny mechanical click with a pitched body. */
function tick(m: Mix, t: number, f: number, vel: number, pan = 0, send = 0.06): void {
  const v = m.voice(perc(t, 0.7 * vel, 0.0004, 0.035), { pan, send });
  if (!v) return;
  v.noise("white", 1, v.filter("bandpass", f * 2.2, 2.2), 1, t + 0.05);
  v.osc("sine", f, perc(t, 0.86, 0.0004, 0.018));
}

/** A weighty low hit: a sine that drops in pitch, with a soft noise slap. */
function thump(m: Mix, t: number, vel: number, f0: number, f1: number, len: number, o: VoiceOpts = {}): void {
  const v = m.voice(perc(t, 0.6 * vel, 0.0015, len), o);
  if (!v) return;
  v.osc("sine", [[t, f0], [t + 0.045, f1 * 1.25, "exp"], [t + len, f1, "exp"]]);
  v.noise("white", perc(t, 0.3, 0.0005, 0.035), v.filter("lowpass", 2400, -3), 1, t + 0.05);
}

/** A cork-like pop: a sine that chirps up to its pitch. */
function pop(m: Mix, t: number, f: number, vel: number, pan = 0, send = 0.18): void {
  const v = m.voice(perc(t, vel, 0.0015, 0.17), { pan, send });
  if (!v) return;
  v.osc("sine", [[t, f * 0.42], [t + 0.02, f, "exp"], [t + 0.12, f * 1.015, "exp"]]);
  v.osc("triangle", sweep(t, f * 0.84, t + 0.02, f * 2), perc(t, 0.14, 0.0008, 0.05));
}

/** A glassy bell: struck-bar partials over a bright transient. */
function ding(m: Mix, t: number, f: number, vel: number, pan: number, len = 1.3, send = 0.38): void {
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
function clink(m: Mix, t: number, vel: number, pan: number): void {
  const v = m.voice(perc(t, vel, 0.0004, 0.6), { pan, send: 0.25 });
  if (!v) return;
  const base = hz(98);
  for (const [ratio, a, len] of [[1, 1, 0.6], [1.59, 0.55, 0.35], [2.21, 0.4, 0.22], [2.93, 0.28, 0.14]]) {
    v.osc("sine", base * ratio, perc(t, a, 0.0004, len));
  }
  v.noise("white", perc(t, 0.5, 0.0003, 0.008), v.filter("highpass", 6000, 0), 1, t + 0.02);
}

/** Air: noise through a moving band. Sustained, so a mid-sweep start still hears it. */
function whoosh(m: Mix, env: readonly Pt[], band: Curve, q: number, o: VoiceOpts = {}, kind: NoiseKind = "pink"): void {
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
function bloop(m: Mix, t: number, f: number, vel: number, pan: number, len = 0.22): void {
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
function knock(m: Mix, t: number, f: number, vel: number, pan: number, send = 0.12): void {
  const v = m.voice(perc(t, vel, 0.0008, 0.2), { pan, send });
  if (!v) return;
  v.noise("white", perc(t, 0.8, 0.0005, 0.06), v.filter("bandpass", 1300 + f, 1.3), 1, t + 0.08);
  v.osc("triangle", sweep(t, f * 2, t + 0.02, f), perc(t, 0.7, 0.001, 0.15));
  v.osc("sine", sweep(t, f, t + 0.05, f * 0.5), 0.45);
}

// Drums and bass for the groove.

function kick(m: Mix, t: number, vel: number, filtered = false): void {
  const v = m.voice(perc(t, 0.5 * vel, 0.002, 0.46), { bus: filtered ? "music" : "drums" });
  if (!v) return;
  const into = filtered ? v.filter("lowpass", 160, -3) : v.amp;
  v.osc("sine", [[t, 165], [t + 0.032, 62, "exp"], [t + 0.3, 45, "exp"]], 1, into);
  if (!filtered) v.noise("white", perc(t, 0.22, 0.0004, 0.012), v.filter("highpass", 2500, 0), 1, t + 0.02);
}

function clap(m: Mix, t: number, vel: number): void {
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

function hat(m: Mix, t: number, vel: number): void {
  const v = m.voice(perc(t, 0.28 * vel, 0.0008, 0.05), { bus: "drums", pan: 0.22 });
  if (v) v.noise("white", 1, v.filter("highpass", 7200, 0));
}

/**
 * A run of bass notes as one voice: the oscillators keep their phase from
 * note to note, so repeated notes never cancel, and the envelope dips at each
 * change to articulate it. Notes are [start, end, MIDI note, velocity].
 * `grit` is a band-passed saw at the root that puts the line's third to
 * sixth harmonics (150 to 300 Hz) about 11 dB under the fundamental, so the
 * line survives on laptop and phone speakers that roll off below 200 Hz.
 */
function bassRun(
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

/**
 * One pad chord: two detuned voices spread across the stereo field. `thin`
 * highpasses it and scoops 250 to 400 Hz, so a breakdown chord leaves the
 * low end to the downbeat and the low mids to the liquid.
 */
function pad(
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

// Bar 1, "Line & fold": ignition, the pen, the flood, four folds, the lid, tape.

function ignition(m: Mix, t: number): void {
  // One small point of light: quiet (about -19.5 LUFS over its first 100
  // ms, level with the pen draw that follows), so the lid, the slams, and
  // the resolve stay the big moments and bar 1 builds from here.
  const v = m.voice(perc(t, 0.055, 0.001, 0.8), { send: 0.4 });
  if (v) {
    v.osc("sine", sweep(t, hz(84), t + 0.07, hz(86)));
    v.osc("sine", hz(98), perc(t, 0.2, 0.0005, 0.25));
    v.noise("white", perc(t, 0.8, 0.0003, 0.018), v.filter("bandpass", 5200, 1.2), 1, t + 0.03);
  }
  // A warm bloom that swells under the spark as the point grows.
  const g = m.voice(ad(t, t + 0.09, 0.058, t + 0.6), { hold: true });
  if (g) {
    g.osc("sine", hz(50));
    g.osc("sine", hz(57), 0.5);
  }
}

/**
 * The pens leave on the first 32nd and speed up (scene 1 runs them with an
 * arc-length power of 1.45), so the corner glints crowd together toward the
 * close. The shimmer's band and pitch step up at each corner.
 */
function penDraw(m: Mix, t0: number, t1: number): void {
  const launch = t0 + BEAT / 8;
  const corner = (k: number) => launch + (t1 - launch) * (k / 7) ** (1 / 1.45);
  const band: Pt[] = [[t0, 2600]];
  const pitch: Pt[] = [[t0, hz(88)]];
  const pan: Pt[] = [[t0, 0]];
  const steps = [88, 90, 93, 95, 97, 98, 100];
  for (let k = 1; k <= 7; k++) {
    const t = corner(k);
    band.push([t, 2600 + 700 * k, "exp"]);
    pitch.push([t - 0.004, hz(steps[k - 1]), "exp"], [t, hz(steps[Math.min(6, k)]), "exp"]);
    pan.push([t, 0.5 * Math.sin(k * 2.1)]);
  }
  const v = m.voice(hold(t0, launch - t0, 0.09, t1 - 0.01, 0.28, 0.11), { pan, send: 0.3, hold: true });
  if (v) {
    v.noise("white", 0.9, v.filter("bandpass", band, 5));
    const s = v.osc("sine", pitch, 0.16);
    v.lfo("sine", 19, 16, s.frequency);
  }
  const r = rng(101);
  for (let k = 1; k <= 6; k++) {
    const t = corner(k);
    ping(m, t, hz(steps[k - 1] + 12), 0.035 + 0.01 * k, 0.12, { pan: (r() * 2 - 1) * 0.5, send: 0.3 });
  }
}

function flood(m: Mix, t: number): void {
  // The outline closes: a tick and a ping, then the flood whoosh.
  tick(m, t, 2600, 0.22, 0.1, 0.2);
  ping(m, t, hz(86), 0.12, 0.5, { send: 0.4 });
  const env: Pt[] = [[t, 0], [t + 0.08, 0.3], [t + 0.3, 0.14, "exp"], [t + 0.6, 0.0001, "exp"], [t + 0.604, 0]];
  const band: Pt[] = [[t, 350], [t + 0.14, 3000, "exp"], [t + 0.55, 600, "exp"]];
  whoosh(m, env, band, 0.8, { pan: line(t, -0.25, t + 0.5, 0.25), send: 0.25 });
}

/**
 * The walls wind up together on b1.75 (s1 T_CROUCH) as every crease flares
 * (s1 drawCreases): a scored-card scratch on the flare, well under the fold
 * clacks, then a short inhale into the first fold.
 */
function crouch(m: Mix, t: number, t1: number): void {
  tick(m, t, 3400, 0.18, 0, 0.15);
  const s = m.voice(perc(t, 0.06, 0.0006, 0.025), { pan: 0.1, send: 0.12 });
  if (s) s.noise("white", 1, s.filter("bandpass", sweep(t, 7000, t + 0.025, 3500), 1.6), 1, t + 0.035);
  whoosh(m, swell(t, t + 0.04, 0.01, t1 - 0.006, 0.07, t1 + 0.01), sweep(t, 500, t1, 1600), 1.4, { send: 0.12 });
}

function fold(m: Mix, t: number, i: number): void {
  m.duck(t, 0.15, 0.08);
  // A3 B3 C#4 D4: each wall rises a scale step.
  knock(m, t, hz([57, 59, 61, 62][i]), 0.5, [-0.35, 0.35, -0.18, 0.18][i], 0.14);
}

/**
 * s1 LID: the lid hangs at -25 degrees from 1.305, then whips over on a
 * Hermite from rest to 2700 degrees a second, most of it in the last 60 ms.
 */
function lid(m: Mix, t: number, hang: number): void {
  whoosh(m, swell(hang, t - 0.06, 0.012, t - 0.004, 0.13, t + 0.01), sweep(hang, 350, t, 1700), 1.3, {
    send: 0.08,
    pan: line(hang, 0.1, t, -0.05),
  });
  // The lid slams shut.
  m.duck(t, 0.3, 0.12);
  thump(m, t, 0.85, 140, 50, 0.4);
  const v = m.voice(perc(t, 0.45, 0.0008, 0.12), { send: 0.18 });
  if (v) v.noise("white", 1, v.filter("lowpass", 1800, 0), 1, t + 0.15);
  whoosh(m, ad(t, t + 0.012, 0.16, t + 0.3), 500, 0.8, { send: 0.1, hold: false });
  // The lid hops twice off the rim before it settles.
  knock(m, t + 0.066, hz(50), 0.16, 0.05, 0.06);
  knock(m, t + 0.1, hz(50), 0.07, -0.05, 0.06);
}

/**
 * s1 tapeAt: the tape pulls across the lid on an ease in and out (the
 * scene's cubicBezier(0.3, 0, 0.25, 1)) that comes to rest at the corner at
 * 70% of the time, then presses down the side on an ease out and seats on
 * b3.75. The rip follows the tape's speed: it swells across the lid, stalls
 * at the corner, and tears again down the side.
 */
function tape(m: Mix, t0: number, t1: number): void {
  const pull = cubicBezier(0.3, 0, 0.25, 1);
  const turn = t0 + 0.7 * (t1 - t0);
  // Speed across the lid, sampled from the scene's own curve.
  const speed = (t: number) => pull(progress(t0, turn, t + 0.002)) - pull(progress(t0, turn, t - 0.002));
  let top = 0;
  for (let i = 1; i < 16; i++) top = Math.max(top, speed(lerp(t0, turn, i / 16)));
  const env: Pt[] = [[t0, 0]];
  const rate: Pt[] = [[t0, 0.5]];
  for (let i = 1; i <= 12; i++) {
    const t = lerp(t0, turn, i / 12);
    const s = i === 12 ? 0 : speed(t) / top;
    env.push([t, 0.02 + 0.42 * s ** 0.7]);
    rate.push([t, 0.5 + 2.2 * s]);
  }
  // Down the side: torn off at full speed, slowing on the ease out.
  env.push([turn + 0.004, 0.48], [turn + 0.03, 0.34]);
  env.push([t1 - 0.012, 0.055, "exp"], [t1, 0.0001, "exp"], [t1 + 0.004, 0]);
  rate.push([turn + 0.004, 2.6, "set"], [t1, 0.6, "exp"]);
  const pan: Pt[] = [[t0, -0.45], [turn, 0.3], [t1, 0.45]];
  const v = m.voice(env, { pan, send: 0.12, hold: true });
  if (v) {
    const band: Pt[] = [
      [t0, 1500],
      [turn - 0.03, 3000, "exp"],
      [turn, 1700, "exp"],
      [turn + 0.01, 3200, "exp"],
      [t1, 1400, "exp"],
    ];
    const am = v.vca(0.6, v.filter("bandpass", band, 0.8));
    // Adhesive letting go in a sawtooth rhythm that runs with the speed.
    v.lfo("sawtooth", rate.map(([t, r, k]): Pt => [t, 50 * r, k]), 0.4, am.gain);
    v.noise("crackle", 3, am, rate);
    v.noise("white", 1.2, am);
  }
  // The tape creases over the edge, then seats with a pat.
  tick(m, turn, 1900, 0.18, 0.3, 0.08);
  thump(m, t1, 0.18, 240, 120, 0.08, { pan: 0.4 });
}

// Bar 2, "Character".

function creak(m: Mix, t0: number, t1: number): void {
  const r = rng(202);
  const rate: Pt[] = [];
  for (let t = t0; t <= t1 + 1e-9; t += 0.012) rate.push([t, 24 + 30 * ((t - t0) / (t1 - t0)) + 7 * r()]);
  const v = m.voice(hold(t0, 0.05, 1.2, t1 - 0.02, 2, 0.032), { send: 0.1, hold: true, pan: 0.05 });
  if (!v) return;
  // A slow pulse train rung through two cardboard formants.
  const saw = v.osc("sawtooth", rate, 1, v.filter("bandpass", sweep(t0, 620, t1, 840), 7));
  saw.connect(v.gain(0.7, v.filter("bandpass", sweep(t0, 1450, t1, 1950), 9)));
}

function launch(m: Mix, t: number, land: number): void {
  m.duck(t, 0.2, 0.1);
  const env: Pt[] = [
    [t, 0],
    [t + 0.06, 0.3],
    [t + 0.22, 0.24],
    [land - 0.02, 0.03, "exp"],
    [land, 0.0001, "exp"],
    [land + 0.004, 0],
  ];
  const v = m.voice(env, { pan: line(t, -0.15, land, 0.15), send: 0.18, hold: true });
  if (v) {
    const bp = v.filter("bandpass", [[t, 480], [t + 0.2, 2400, "exp"], [land, 800, "exp"]], 1.4);
    // The spin: one flutter per quarter turn of yaw. s2 spins 374 degrees
    // in the air, fast off the push and slowing linearly (SPIN_K 0.35), so
    // the quarter turns come at 12 a second at takeoff and 5.8 at touchdown.
    const am = v.vca(0.62, bp);
    v.lfo("sine", line(t, 12, land, 5.8), 0.34, am.gain);
    v.noise("pink", 1, am);
  }
  // The stretch: a rising fwip.
  const w = m.voice(perc(t, 0.2, 0.003, 0.22), { send: 0.25 });
  if (w) {
    w.osc("sine", sweep(t, hz(62), t + 0.09, hz(81)));
    w.osc("triangle", sweep(t, hz(74), t + 0.09, hz(93)), 0.12);
  }
}

function land(m: Mix, t: number): void {
  m.duck(t, 0.5, 0.2);
  thump(m, t, 1, 110, 42, 0.5);
  const v = m.voice(perc(t, 0.45, 0.0008, 0.1), { send: 0.15 });
  if (v) v.noise("white", 1, v.filter("lowpass", 1500, 0), 1, t + 0.12);
  // The dust puff.
  whoosh(m, ad(t, t + 0.03, 0.18, t + 0.45), sweep(t, 1200, t + 0.4, 500), 0.8, { send: 0.2, hold: false });
}

function flick(m: Mix, t: number, pan: number, vel = 0.16): void {
  const v = m.voice(perc(t, 2.2 * vel, 0.002, 0.08), { pan, send: 0.12 });
  if (!v) return;
  v.noise("white", 1, v.filter("bandpass", sweep(t, 2200, t + 0.05, 7000), 2.5));
  v.osc("sine", sweep(t, 1400, t + 0.045, 3200), 0.35);
}

/** A jaw-harp spring: a resonant filter and the pitch wobbling together. */
function boing(m: Mix, t: number, vel = 0.3, f = hz(50), len = 0.42): void {
  const v = m.voice(perc(t, vel, 0.004, len), { send: 0.15 });
  if (!v) return;
  const lp = v.filter("lowpass", 900, 9);
  const saw = v.osc("sawtooth", f, 0.5, lp);
  v.lfo("sine", 11, sweep(t, 700, t + len, 20), lp.frequency);
  v.lfo("sine", 11, sweep(t, f * 0.1, t + len, 0.3), saw.frequency);
  const s = v.osc("sine", sweep(t, f * 2.25, t + 0.05, f * 2), 0.35);
  v.lfo("sine", 11, sweep(t, f * 0.17, t + len, 0.5), s.frequency);
}

function stamp(m: Mix, t: number, vel = 1, pan = 0.12): void {
  thump(m, t, 0.6 * vel, 190, 75, 0.22, { pan });
  const v = m.voice(perc(t, 0.4 * vel, 0.0006, 0.07), { pan, send: 0.15 });
  if (v) v.noise("white", 1, v.filter("bandpass", 1500, 0.9), 1, t + 0.1);
  const p = m.voice(perc(t + 0.004, 0.12 * vel, 0.0005, 0.03), { pan: pan + 0.08 });
  if (p) p.noise("white", 1, p.filter("highpass", 5500, 0), 1, t + 0.05);
}

/** Fabric spinning: a sharp first flap, then flaps gated fast, slowing as the spring settles. */
function flutter(m: Mix, t: number, len = 0.2, vel = 0.3): void {
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

/**
 * s2 monocle: flicked up from behind the lid (MONO_UP, 0.19 s before the
 * seat) on an ease out to the top of its arc, it drops into the seat with a
 * clink, swings out once on a 7 Hz pendulum, and is blended back onto the
 * seat by about 64 ms later.
 */
function monocle(m: Mix, seat: number): void {
  const up = seat - 0.19;
  const f = m.voice(perc(up, 0.1, 0.004, 0.07), { pan: 0.2, send: 0.2 });
  if (f) {
    f.noise("pink", 1, f.filter("bandpass", sweep(up, 1600, up + 0.05, 4200), 2.2));
    f.osc("sine", sweep(up, hz(81), up + 0.05, hz(93)), 0.18);
  }
  m.duck(seat, 0.2, 0.12);
  clink(m, seat, 0.2, 0.25);
  const home = seat + 0.064;
  clink(m, home, 0.06, 0.28);
}

function blip(m: Mix, t: number, f: number, vel: number): void {
  const v = m.voice(perc(t, 1.6 * vel, 0.001, 0.035), { send: 0.1 });
  if (v) v.osc("sine", sweep(t, f * 1.3, t + 0.02, f));
}

// Bar 3, "Kinetic type". The iris whomp and the three slams are its loudest
// moments; they sit about 2 LU under the resolve so bar 8 stays the peak.

function dive(m: Mix, t0: number, t1: number): void {
  m.duck(t1, 0.35, 0.15);
  // The lens catches the light as the camera dives in.
  ping(m, t0, hz(98), 0.07, 0.25, { pan: 0.25, send: 0.35 });
  tick(m, t0, 3000, 0.12, 0.25);
  whoosh(m, swell(t0, t0 + 0.05, 0.03, t1 - 0.012, 0.36, t1 + 0.07), sweep(t0, 300, t1, 4200), 1.3, { send: 0.2 });
  // Glass rising an octave as the lens fills the frame.
  const g = m.voice(swell(t0, t0 + 0.05, 0.01, t1 - 0.012, 0.07, t1 + 0.05), { send: 0.3, hold: true });
  if (g) g.osc("sine", sweep(t0, hz(74), t1, hz(86)));
  // The iris whomp.
  const w = m.voice(perc(t1, 0.39, 0.004, 0.45), { send: 0.15 });
  if (w) {
    w.osc("sine", sweep(t1, 95, t1 + 0.3, 38));
    w.noise("pink", perc(t1, 0.3, 0.002, 0.12), w.filter("lowpass", 700, 0), 1, t1 + 0.15);
  }
}

/** The hero line types on with scene 3's own uneven key rhythm. */
function typing(m: Mix, t0: number, t1: number): void {
  const text = "Reuse matching compilation work across";
  const w: number[] = [];
  for (let i = 0; i < text.length; i++) w.push(0.6 + hash(i, 41) * 0.8 + (text[i - 1] === " " ? 0.5 : 0));
  const total = w.reduce((s, x) => s + x, 0);
  const r = rng(303);
  let acc = 0;
  for (let i = 0; i < text.length; i++) {
    const t = t0 + (acc / total) * (t1 - t0);
    acc += w[i];
    const f = 1900 + 700 * r();
    const pan = (r() - 0.5) * 0.3;
    // Every other key, plus each word's first: a fast, even patter.
    const wordStart = i === 0 || text[i - 1] === " ";
    if (i % 2 && !wordStart) continue;
    tick(m, t, f, wordStart ? 0.4 : 0.24, pan);
  }
}

/** Slam 1, "projects,": a woody boom, then the other letters knock in. */
function slamDrop(m: Mix, t: number): void {
  m.duck(t, 0.6, 0.22);
  thump(m, t, 0.6, 125, 40, 0.6);
  const v = m.voice(perc(t, 0.2, 0.001, 0.4), { send: 0.18 });
  if (v) {
    const lp = v.filter("lowpass", sweep(t, 2400, t + 0.25, 500), 2);
    v.osc("triangle", hz(50), 1, lp);
    v.noise("white", perc(t, 0.7, 0.0005, 0.05), lp, 1, t + 0.08);
  }
  // The other letters land on the scene's 1/128-bar stagger.
  const stag = BEAT / 32;
  const climb = [0, 2, 4, 7, 9, 12, 14, 16];
  for (let k = 1; k <= 8; k++) {
    knock(m, t + k * stag, hz(62 + climb[k - 1]), 0.11 * (1 - k / 12), (k % 2 ? -1 : 1) * 0.3, 0.1);
  }
}

/**
 * The git graph under the lockup (s3 T_MAIN): main's first commit pops on
 * b9.25 as "projects," settles, and the line draws out left to right to its
 * second commit just before the branch forks on b9.75.
 */
function gitMain(m: Mix, t: number, tip: number): void {
  pop(m, t, hz(74), 0.13, -0.35, 0.2);
  whoosh(m, ad(t + 0.02, tip - 0.02, 0.05, tip + 0.03), sweep(t + 0.02, 1400, tip, 3200), 3, {
    pan: line(t + 0.02, -0.4, tip, 0.35),
    send: 0.15,
  });
  pop(m, tip, hz(78), 0.08, 0.35, 0.2);
}

/** Slam 2, "worktrees,": a plucked Bm stab over a low body. */
function slamSlide(m: Mix, t: number, branch: number): void {
  // The git branch leaves the main line: a rising zip that meets the hit.
  m.duck(t, 0.55, 0.2);
  whoosh(m, ad(branch, t - 0.004, 0.2, t + 0.07), sweep(branch, 900, t, 4200), 1.6, {
    pan: line(branch, -0.6, t, 0.3),
    send: 0.1,
  });
  const z = m.voice(ad(branch, t - 0.004, 0.05, t + 0.03), { send: 0.2, hold: true, pan: 0.2 });
  if (z) z.osc("triangle", sweep(branch, hz(59), t, hz(83)));
  // Bm triad plucked through a closing filter, wide, over a B1 body.
  for (const side of [-1, 1]) {
    const v = m.voice(perc(t, 0.24, 0.002, 0.45), { pan: side * 0.5, send: 0.22 });
    if (!v) continue;
    const lp = v.filter("lowpass", sweep(t, 6000, t + 0.25, 700), 4);
    for (const n of [59, 62, 66, 71]) v.osc("sawtooth", hz(n), 0.3, lp, side * 9);
  }
  thump(m, t, 0.6, 95, 46, 0.4);
  const b = m.voice(perc(t, 0.16, 0.002, 0.35), { send: 0.08 });
  if (b) {
    b.osc("sine", hz(35));
    b.osc("sine", hz(47), 0.45);
  }
  const s = m.voice(perc(t, 0.45, 0.0005, 0.05), { send: 0.15 });
  if (s) s.noise("white", 1, s.filter("bandpass", 2600, 1.2), 1, t + 0.07);
}

/** "and" glides in on the sixteenth pickup (s3 T_AND). */
function glideAnd(m: Mix, t: number): void {
  whoosh(m, ad(t - 0.02, t + 0.05, 0.08, t + 0.2), sweep(t - 0.02, 2600, t + 0.2, 1100), 1.5, { send: 0.15, pan: 0.1 });
  tick(m, t + 0.05, 2300, 0.08, 0.1, 0.1);
}

/** "CI" stamps giant with an RGB split, then snaps to size: three detuned layers pulling into unison. */
function slamGlitch(m: Mix, t: number, snap: number): void {
  m.duck(t, 0.6, 0.22);
  const f = hz(50);
  for (const [k, pan] of [[-1, -0.8], [0, 0], [1, 0.8]]) {
    const v = m.voice(perc(t, 0.12, 0.0008, 0.32), { pan, send: 0.08 });
    if (!v) continue;
    const cr = v.shaper(m.sh.crush, v.filter("lowpass", sweep(t, 7000, t + 0.3, 900), 1));
    v.osc("sawtooth", [[t, f * (1 + 0.07 * k)], [snap, f * (1 + 0.05 * k), "exp"], [snap + 0.012, f]], 1, cr);
    v.osc("square", [[t, f * 2 * (1 - 0.05 * k)], [snap, f * 2 * (1 - 0.04 * k), "exp"], [snap + 0.012, f * 2]], 0.4, cr);
  }
  thump(m, t, 0.55, 160, 44, 0.5);
  const b = m.voice(perc(t, 0.11, 0.0004, 0.05), { send: 0.1 });
  if (b) b.osc("square", hz(93), 1, b.filter("lowpass", 5000, 0));
  // The snap to size.
  const z = m.voice(perc(snap, 0.28, 0.0004, 0.05), { send: 0.15 });
  if (z) {
    z.noise("white", 1, z.filter("bandpass", sweep(snap, 6000, snap + 0.04, 1800), 2), 1, snap + 0.07);
    z.osc("sine", sweep(snap, 2400, snap + 0.03, 900), 0.4);
  }
}

/** The extra words fall away in fragments, each a falling note. */
function scatter(m: Mix, t: number): void {
  const r = rng(404);
  const scale = [74, 76, 78, 81, 83, 86, 88, 90, 93];
  for (let i = 0; i < 12; i++) {
    const ti = i === 0 ? t : t + i * 0.024 + r() * 0.018;
    const f = hz(scale[Math.floor(r() * scale.length)]);
    const pan = (r() * 2 - 1) * 0.7;
    const v = m.voice(perc(ti, 0.13 * (1 - i / 18), 0.001, 0.24), { pan, send: 0.22 });
    if (!v) continue;
    v.osc("sine", sweep(ti, f, ti + 0.24, f * 0.55));
    v.osc("triangle", sweep(ti, f * 2, ti + 0.1, f * 1.1), perc(ti, 0.12, 0.0005, 0.04));
  }
  whoosh(m, ad(t, t + 0.03, 0.09, t + 0.45), sweep(t, 3200, t + 0.45, 500), 1.5, { send: 0.2 });
}

/** The three names fly on arcs; their whooshes peak as they reach the labels (s3 T_LAND), a frame before b12. */
function flyToNodes(m: Mix, t0: number, land: number): void {
  [-0.65, 0.6, 0.7].forEach((pan, k) => {
    const s = t0 + 0.1 + k * 0.03;
    whoosh(m, ad(s, land, 0.09, land + 0.07), sweep(s, 600, land, 2600 + 400 * k), 2, {
      pan: line(s, 0, land, pan),
      send: 0.15,
    });
  });
}

// Bar 4, "Particles".

function nodes(m: Mix, t: number): void {
  // project, worktree, CI
  const list: [number, number, number][] = [
    [0, 74, -0.65],
    [0.06, 78, 0.55],
    [0.12, 81, 0.7],
  ];
  for (const [d, n, pan] of list) {
    m.duck(t + d, 0.2, 0.08);
    pop(m, t + d, hz(n), 0.3, pan);
  }
}

function boxPop(m: Mix, t: number): void {
  // Sparks spiral in on the spot first: a gathering, inhaled swell.
  m.duck(t, 0.45, 0.18);
  whoosh(m, ad(t - 0.17, t - 0.006, 0.16, t), sweep(t - 0.17, 5000, t, 1200), 2, { send: 0.1 });
  pop(m, t, hz(69), 0.42, 0, 0.2);
  thump(m, t, 0.7, 120, 50, 0.35);
  whoosh(m, ad(t, t + 0.008, 0.2, t + 0.5), sweep(t, 3800, t + 0.45, 450), 2.2, { send: 0.3, hold: false });
}

/** Compiled crates stream from the project node into the box. */
function stream(m: Mix, t0: number, t1: number): void {
  const n = 7;
  const notes = [86, 88, 90, 93, 95, 98, 93];
  for (let i = 0; i < n; i++) {
    const t = t0 + 0.02 + ((t1 - t0) * i) / n;
    ping(m, t, hz(notes[i]), 0.06, 0.12, { pan: -0.6 + i * 0.07, send: 0.2 });
  }
}

function gulp(m: Mix, t: number, i: number): void {
  m.duck(t, 0.12, 0.06);
  const f = hz([50, 52, 54, 57][i]);
  const v = m.voice(perc(t, 0.32, 0.003, 0.12), { send: 0.08 });
  if (v) {
    const lp = v.filter("lowpass", sweep(t, 1600, t + 0.09, 450), 5);
    v.osc("sine", [[t, f * 1.6], [t + 0.028, f * 0.85, "exp"], [t + 0.09, f, "exp"]], 1, lp);
    v.osc("triangle", sweep(t, f * 3.2, t + 0.028, f * 1.7), 0.25, lp);
  }
}

/** The box charges, then sends restored artifacts in three bursts. */
function restore(m: Mix, charge: number, bursts: number[], done: number): void {
  const c = m.voice(ad(charge, bursts[0] - 0.004, 0.14, bursts[0] + 0.01), { send: 0.2, hold: true });
  if (c) {
    const lp = c.filter("lowpass", sweep(charge, 400, bursts[0], 4000), 6);
    c.osc("sawtooth", sweep(charge, hz(62), bursts[0], hz(74)), 0.6, lp);
  }
  const r = rng(505);
  bursts.forEach((b, k) => {
    const vel = [1, 0.7, 0.55][k];
    m.duck(b, 0.3 * vel, 0.2);
    const notes = [[86, 90, 93, 98, 102], [88, 93, 97], [90, 95, 98]][k];
    notes.forEach((n, i) => {
      ping(m, b + (i * X) / 2, hz(n), (0.15 - i * 0.02) * vel, 0.4, { pan: 0.15 + i * 0.12, send: 0.3 });
    });
    whoosh(m, ad(b, b + 0.1, 0.15 * vel, b + 0.45), sweep(b, 1200, b + 0.35, 5200), 1.5, {
      pan: line(b, 0, b + 0.4, 0.65),
      send: 0.25,
    });
    for (let i = 0; i < 4; i++) {
      const ti = b + 0.03 + r() * 0.4;
      const f = 3500 + r() * 4500;
      const pan = 0.3 + r() * 0.55;
      const v = m.voice(perc(ti, (0.03 + 0.02 * r()) * vel, 0.0005, 0.06), { pan, send: 0.35 });
      if (v) v.osc("sine", f);
    }
  });
  // Arrivals: the counters tick up as artifacts land, ending on b15.5.
  const windows = [
    [bursts[0] + 0.26, bursts[1] + 0.2, 6],
    [bursts[1] + 0.24, bursts[2] + 0.17, 4],
    [bursts[2] + 0.2, done - 0.012, 2],
  ];
  let k = 0;
  for (const [a, b, n] of windows) {
    for (let i = 0; i < n; i++) {
      const t = a + (b - a) * outCubicInv(i / n);
      k++;
      tick(m, t, 2000 + k * 70, 0.1, i % 2 ? 0.5 : 0.72);
    }
  }
  // The last hit lands: a chime.
  tick(m, done, 3200, 0.2, 0.6);
  ping(m, done, hz(81), 0.16, 0.45, { pan: 0.5, send: 0.3 });
  ping(m, done + 0.004, hz(86), 0.12, 0.45, { pan: 0.72, send: 0.3 });
  // s4 glintK: Mr Boxington's monocle twinkles 30 ms after the lock.
  ding(m, done + 0.03, hz(98), 0.035, 0, 0.35, 0.35);
}

/** Whip pan: out to the left into the cut, in from the right after it. */
function whip(m: Mix, t: number): void {
  m.duck(t, 0.5, 0.12);
  const out = swell(t - 0.14, t - 0.09, 0.05, t - 0.004, 0.45, t + 0.012);
  whoosh(m, out, sweep(t - 0.14, 900, t, 4800), 0.9, { pan: line(t - 0.14, 0, t, -0.85) }, "white");
  const inn = ad(t - 0.01, t + 0.004, 0.42, t + 0.16);
  whoosh(m, inn, sweep(t - 0.01, 4800, t + 0.14, 900), 0.9, { pan: line(t - 0.01, 0.85, t + 0.14, 0), send: 0.1 }, "white");
  thump(m, t, 0.4, 90, 45, 0.18);
}

// Bar 5, "Data".

/**
 * Cargo lurches forward crate by crate (s5-data STEP_PLAN): steps on b16.5,
 * b16.75, and b17, a fourth two frames after b17.25, then a grind that
 * starts slow, arrives at speed, and clunks home half a frame before b17.75
 * (CARGO_DONE, so the frame nearest the beat shows it home). Each lurch
 * knocks a scale step higher (the fold motif, an octave down) and its
 * readout's drums ratchet over.
 */
function cargoBar(m: Mix): void {
  const steps = [bt(16.5), bt(16.75), bt(17), bt(17.25) + 2 * FRAME];
  steps.forEach((t, i) => {
    knock(m, t, hz([45, 47, 49, 50][i]), 0.3 + 0.04 * i, -0.2, 0.08);
    const s = m.voice(perc(t, 0.12, 0.003, 0.07), { pan: -0.2, send: 0.06 });
    if (s) s.noise("pink", 1, s.filter("bandpass", 800, 1.1));
    for (let k = 0; k < 3; k++) tick(m, t + 0.014 + k * 0.016, 1500 + 110 * i + 60 * k, 0.05, -0.25);
  });
  const g0 = bt(17.25) + 8 * FRAME;
  const home = bt(17.75) - FRAME / 2;
  m.duck(home, 0.2, 0.1);
  const g = m.voice(hold(g0, 0.03, 0.05, home - 0.006, 0.2, 0.012), { pan: -0.15, send: 0.06, hold: true });
  if (g) {
    const bp = g.filter("bandpass", sweep(g0, 450, home, 1300), 1.2);
    g.noise("crackle", 2.5, bp, sweep(g0, 0.45, home, 1.3));
    g.noise("pink", 0.6, bp);
    g.osc("sawtooth", sweep(g0, hz(33), home, hz(38)), 0.3, g.filter("lowpass", 500, 2));
  }
  thump(m, home, 0.45, 140, 55, 0.22, { pan: -0.1 });
  knock(m, home, hz(38), 0.32, -0.15, 0.1);
  tick(m, home, 2200, 0.16, -0.2);
}

/**
 * mbx launches on s5-data's spring (`mbxGrow`: 3.7 Hz, damping 0.66, launch
 * velocity 7.75): its tone rises an octave with the bar, overshoots with it,
 * and settles, while the readout ratchets up evenly. The scene locks the
 * readout half a frame before the bar first reaches its mark (`mbxLock`), so
 * the lock, the photo-finish hairline, and the flash all first show on the
 * frame after that (8.0833, 2.6 ms before b17.25); the lock sounds there.
 */
function mbxBar(m: Mix, t0: number): void {
  const grow = (t: number) => spring(t - t0, 3.7, 0.66, 7.75);
  const lock = onFrame(crossing(grow, 1, t0, 0.2) - FRAME / 2);
  m.duck(lock, 0.3, 0.15);
  pop(m, t0, hz(69), 0.2, 0.05, 0.15);
  tick(m, t0, 2600, 0.14, 0.05);
  const v = m.voice(hold(t0, 0.012, 0.15, lock + 0.1, 0.12, 0.3), { send: 0.22, hold: true, pan: 0.05 });
  if (v) {
    const pitch: Pt[] = [[t0, hz(57)]];
    for (let i = 1; i <= 16; i++) pitch.push([t0 + i * 0.025, hz(57) * 2 ** grow(t0 + i * 0.025), "exp"]);
    const lp = v.filter("lowpass", [[t0, 700], [lock, 4200, "exp"], [lock + 0.4, 1200, "exp"]], 3);
    v.osc("sawtooth", pitch, 0.45, lp, -8);
    v.osc("sawtooth", pitch, 0.45, lp, 8);
  }
  for (let k = 1; k <= 7; k++) {
    const t = crossing(grow, k / 8, t0, 0.2);
    tick(m, t, 2400 + k * 40, 0.06, 0.15);
  }
  thump(m, lock, 0.3, 160, 80, 0.15);
  tick(m, lock, 3000, 0.2, 0.1);
  ping(m, lock, hz(81), 0.17, 0.4, { send: 0.3, pan: 0.1 });
  ping(m, lock + 0.004, hz(86), 0.12, 0.4, { send: 0.3, pan: 0.2 });
}

function delta(m: Mix, t: number, label: number, glint: number): void {
  m.duck(t, 0.3, 0.15);
  tick(m, t, 2800, 0.24, 0.1, 0.12);
  // Short enough to clear the way for the figure's pop a sixteenth later.
  ping(m, t, hz(81), 0.18, 0.28, { send: 0.3 });
  ping(m, t + 0.05, hz(86), 0.2, 0.32, { send: 0.3 });
  // The figure pops in, then glints.
  tick(m, label, 2600, 0.16, 0.15);
  pop(m, label, hz(90), 0.2, 0.15, 0.2);
  ding(m, glint, hz(98), 0.05, 0.2, 0.5, 0.4);
}

/**
 * The chart crouches for 0.11 s, then clears on b19 as one move: the page
 * falls back in perspective while the bar compacts to a square on b19.25,
 * extrudes, and the camera swings round and eases to a stop on the world
 * view 45 ms before b20 (s5 DOLLY), where the cube sits as the handoff.
 * The stop is an ease to rest with nothing landing, so it gets no hit: the
 * drone and the swing fade out on it and leave the downbeat to the kick.
 */
function extrude(m: Mix, clear: number, hit: number, t1: number): void {
  const settle = t1 - 0.045;
  m.duck(clear, 0.25, 0.1);
  m.duck(hit, 0.25, 0.12);
  // The page swipes away and falls back: a bright swipe, then air falling in pitch.
  const sw = m.voice(perc(clear, 0.2, 0.0008, 0.035), { send: 0.1 });
  if (sw) sw.noise("white", 1, sw.filter("highpass", 2800, 0));
  whoosh(m, swell(clear - 0.1, clear - 0.04, 0.03, clear, 0.12, clear + 0.02), sweep(clear - 0.1, 900, clear, 2600), 1.2, {
    send: 0.1,
  });
  whoosh(m, ad(clear, clear + 0.02, 0.3, clear + 0.3), sweep(clear, 5000, clear + 0.28, 600), 0.8, { send: 0.12 }, "white");
  knock(m, hit, hz(50), 0.3, 0, 0.12);
  const v = m.voice(hold(hit, 0.06, 0.08, settle - 0.015, 0.2, 0.04), { send: 0.2, hold: true });
  if (v) {
    const lp = v.filter("lowpass", sweep(hit, 250, settle, 2600), 8);
    const g = sweep(hit, hz(38), settle, hz(50));
    v.osc("sawtooth", g, 0.5, lp, -7);
    v.osc("sawtooth", g, 0.5, lp, 7);
  }
  // The camera swings off the chart and back to the world view.
  const band: Pt[] = [[clear, 700], [clear + 0.25, 1800, "exp"], [settle, 900, "exp"]];
  whoosh(m, ad(clear + 0.05, clear + 0.25, 0.09, settle), band, 1.2, { pan: line(clear, -0.5, settle, 0.5), send: 0.1 });
}

// Bar 6, "Isometric": the grid, the city raining in, the prune, the discs.

interface Cell {
  x: number;
  z: number;
  d: number;
  keep: boolean;
}

let cityCache: Cell[] | null = null;
/**
 * Scene 6's city, rebuilt from its rules (s6-world `world()`): every cell
 * within GRID_R but the center, the four corners, and a few seeded holes
 * toward the rim; kept cells are always filled.
 */
function city(): Cell[] {
  if (cityCache) return cityCache;
  const keep = new Set(KEEP.map(([x, z]) => `${x},${z}`));
  const out: Cell[] = [];
  for (let x = -GRID_R; x <= GRID_R; x++) {
    for (let z = -GRID_R; z <= GRID_R; z++) {
      if (x === 0 && z === 0) continue;
      const k = keep.has(`${x},${z}`);
      const d = Math.hypot(x, z);
      const corner = Math.abs(x) === GRID_R && Math.abs(z) === GRID_R;
      const seed = (x + 16) * 64 + z + 16;
      if (!k && (corner || hash(seed, 3) < 0.07 + 0.3 * smoothstep(3.6, 5.2, d))) continue;
      out.push({ x, z, d, keep: k });
    }
  }
  cityCache = out;
  return out;
}

/**
 * The grid draws out, the center cube stomps on b20.25, and the city rains
 * in: each ring of equal distance lands together, from b20.5 at distance 1
 * to b21.5 at the outermost ring, hypot(GRID_R, GRID_R - 1) (s6 DLAST).
 * Rings climb the D major pentatonic.
 */
function world(m: Mix, t0: number): void {
  whoosh(m, ad(t0, t0 + 0.08, 0.04, t0 + 0.4), sweep(t0, 9000, t0 + 0.3, 4500), 1, { send: 0.3 }, "white");
  const stomp = t0 + X;
  thump(m, stomp, 0.5, 150, 55, 0.3);
  // The stomp's shock ring pops the first ring into being, hot, three frames
  // later (s6 T_POP0): four sparks of light.
  const pop0 = stomp + 0.05;
  [93, 98, 102, 105].forEach((n, i) => {
    ping(m, pop0 + i * 0.006, hz(n), 0.09, 0.16, { pan: [-0.3, 0.3, -0.15, 0.15][i], send: 0.3 });
  });
  const land0 = bt(20.5);
  const land1 = bt(21.5);
  const dlast = Math.hypot(GRID_R, GRID_R - 1);
  const rings = new Map<number, number>();
  for (const c of city()) {
    const key = Math.round(c.d * 1000) / 1000;
    rings.set(key, (rings.get(key) ?? 0) + 1);
  }
  const scale = [62, 64, 66, 69, 71, 74, 76, 78, 81, 83, 86, 88, 90, 93];
  [...rings]
    .sort((a, b) => a[0] - b[0])
    .forEach(([d, count], i) => {
      const t = land0 + ((land1 - land0) * (d - 1)) / (dlast - 1);
      m.duck(t, 0.08, 0.06);
      const weight = Math.min(1, 0.45 + count / 14);
      const pan = i === 0 ? 0 : (i % 2 ? -1 : 1) * Math.min(0.6, 0.1 + 0.04 * i);
      const v = m.voice(perc(t, 0.16 * weight, 0.0008, 0.4), { pan, send: 0.22 });
      if (!v) return;
      // Marimba body and overtone, a cardboard tap, and a squash thud.
      const f = hz(scale[Math.min(i, scale.length - 1)]);
      v.osc("sine", f);
      v.osc("sine", f * 3.99, perc(t, 0.3, 0.0005, 0.05));
      v.noise("white", perc(t, 0.35, 0.0005, 0.03), v.filter("lowpass", 1400, 0), 1, t + 0.05);
      v.osc("sine", sweep(t, 170, t + 0.04, 85), perc(t, 0.7, 0.001, 0.08));
    });
}

/**
 * The prune beam (s6-world beamPos and beamAt) crosses one diagonal row
 * (x + z) every 1/64 bar from b22, so it tags the kept rows on b22.25,
 * b22.5, and b22.75, then accelerates out of the front corner by b22 + 7/8.
 * Each pruned carton rocks and slaps flat SLAP after the beam touches it
 * (0.07 s, shortened by up to 40% on the rows the beam reaches as it speeds
 * up); a row slaps as one, louder the more cartons it holds.
 */
function prune(m: Mix, t0: number): void {
  const speed = 16 / BEAT;
  const fast = t0 + 0.75 * BEAT;
  const end = t0 + (7 / 8) * BEAT;
  // The scanner arms first (s6 ARM0 to T_BEAM0): two pen tips race around
  // the grid's border from the front corner, one each way, speeding up
  // (inQuad) until they meet at the back corner as the beam ignites.
  const arm = t0 - (2 * BEAT - 0.66);
  for (const side of [-1, 1]) {
    const a = m.voice(swell(arm, arm + 0.08, 0.008, t0 - 0.004, 0.07, t0 + 0.006), {
      pan: line(arm, side * 0.55, t0, side * 0.08),
      send: 0.2,
      hold: true,
    });
    if (!a) continue;
    a.osc("triangle", glide(arm, t0, hz(66), hz(90), (u) => u * u), 0.5, undefined, side * 6);
    a.noise("white", 0.5, a.filter("bandpass", glide(arm, t0, 2200, 6500, (u) => u * u), 6));
  }
  // The beam switches on.
  m.duck(t0, 0.15, 0.3);
  tick(m, t0, 3200, 0.3, 0, 0.15);
  const z = m.voice(perc(t0, 0.12, 0.001, 0.06), { send: 0.2 });
  if (z) z.osc("sine", sweep(t0, hz(100), t0 + 0.05, hz(88)));
  // The gantry hums while it sweeps and cuts out as it exits the front corner.
  const v = m.voice(hold(t0, 0.03, 0.11, end - 0.008, 0.13, 0.04), { send: 0.18, hold: true });
  if (v) {
    const am = v.vca(0.6);
    v.lfo("sine", 24, 0.3, am.gain);
    v.noise("white", 0.8, v.filter("bandpass", 3400, 7, am));
    v.osc("sine", sweep(t0, hz(81), end, hz(93)), 0.18, am);
  }
  // It snaps off at the front corner.
  const x = m.voice(perc(end, 0.14, 0.002, 0.09), { send: 0.25, pan: 0.1 });
  if (x) {
    x.noise("white", 1, x.filter("bandpass", sweep(end, 6000, end + 0.08, 2000), 3));
    x.osc("sine", sweep(end, hz(93), end + 0.06, hz(86)), 0.25);
  }
  const cFast = -9 + speed * (fast - t0);
  const k = (9 - cFast - speed * (end - fast)) / (end - fast) ** 2;
  const beamAt = (rank: number): number => {
    const c = rank - 1;
    if (c <= cFast) return t0 + (c + 9) / speed;
    return fast + (-speed + Math.sqrt(speed * speed + 4 * k * (c - cFast))) / (2 * k);
  };
  const rows = new Map<number, { n: number; side: number }>();
  for (const c of city()) {
    if (c.keep) continue;
    const r = rows.get(c.x + c.z) ?? { n: 0, side: 0 };
    r.n++;
    r.side += c.x - c.z;
    rows.set(c.x + c.z, r);
  }
  const r = rng(606);
  for (const [rank, { n, side }] of [...rows].sort((a, b) => a[0] - b[0])) {
    const hit = beamAt(rank);
    const slap = hit + 0.07 * lerp(1, 0.6, progress(fast, end, hit));
    const f = 200 + r() * 160;
    const w = Math.min(1, 0.4 + n / 8);
    const c = m.voice(perc(slap, 0.13 * w, 0.001, 0.16), { pan: Math.max(-0.5, Math.min(0.5, (0.12 * side) / n)), send: 0.1 });
    if (!c) continue;
    c.noise("crackle", 1, c.filter("lowpass", sweep(slap, 3000, slap + 0.14, 500), 0), 1.3);
    c.noise("white", perc(slap, 0.55, 0.0005, 0.02), c.filter("bandpass", 1100, 1.1), 1, slap + 0.04);
    c.osc("sine", sweep(slap, f * 1.8, slap + 0.12, f * 0.6), 0.5);
  }
  // Kept cubes get tagged as the beam reaches their rows.
  const tagged = new Map<number, number>();
  for (const [x, z] of KEEP) tagged.set(x + z, (tagged.get(x + z) ?? 0) + 1);
  const notes: Record<number, number> = { [-4]: 81, 0: 86, 4: 90 };
  for (const [rank, count] of [...tagged].sort((a, b) => a[0] - b[0])) {
    const t = beamAt(rank);
    ping(m, t, hz(notes[rank] ?? 86), 0.1 + 0.02 * count, 0.45, { send: 0.3 });
  }
}

/**
 * The kept cubes wind up under the last of the beam, hop together on b23 and
 * glow (s6 T_BEAM1), turn a quarter, flatten, and round off into the discs
 * just before the bar line (s6 FLAT0 and T_DISC).
 */
function keptHop(m: Mix, t: number, end: number): void {
  m.duck(t, 0.2, 0.15);
  // The flattened carpet drops through the floor like trapdoors, gone in
  // about five frames (s6 sinkDur).
  const d = m.voice(perc(t, 0.22, 0.002, 0.2), { send: 0.12 });
  if (d) {
    d.noise("pink", 1, d.filter("lowpass", sweep(t, 1400, t + 0.12, 180), 2));
    d.osc("sine", sweep(t, 120, t + 0.1, 48), 0.6);
  }
  whoosh(m, swell(t - 0.1, t - 0.06, 0.01, t - 0.004, 0.08, t + 0.01), sweep(t - 0.1, 600, t, 1800), 1.5, { send: 0.1 });
  const h = m.voice(perc(t, 0.2, 0.003, 0.3), { send: 0.2 });
  if (h) {
    h.osc("sine", [[t, hz(50)], [t + 0.1, hz(62), "exp"], [t + 0.3, hz(57), "exp"]]);
    h.osc("triangle", [[t, hz(62)], [t + 0.1, hz(74), "exp"]], perc(t, 0.2, 0.002, 0.08));
  }
  // The glow: a fast strum across the seven cubes, B minor pentatonic over the Bm9 pad.
  [71, 74, 76, 78, 81, 83, 86].forEach((n, i) => {
    const ti = t + i * 0.012;
    const v = m.voice(ad(ti, ti + 0.004, 0.05, ti + 0.55), { pan: (i % 2 ? -1 : 1) * (0.1 + i * 0.06), send: 0.4 });
    if (!v) return;
    v.osc("sine", hz(n));
    v.osc("sine", hz(n) * 2.001, 0.15);
    v.osc("triangle", hz(n), 0.2);
  });
  const flat = t + 0.23;
  // They flatten and round off into the discs.
  whoosh(m, ad(flat - 0.1, end - 0.03, 0.09, end + 0.06), sweep(flat - 0.1, 3500, end, 500), 1.2, { send: 0.3 });
}

// Bar 7, "Liquid morph": the breakdown.

function jelly(m: Mix, t: number): void {
  // The shared splat: a wet slap on top, so the breakdown opens with some
  // top end over the wobble's body.
  const s = m.voice(perc(t, 0.16, 0.001, 0.07), { send: 0.25 });
  if (s) {
    s.noise("pink", 1, s.filter("bandpass", sweep(t, 3200, t + 0.06, 1300), 1.3));
    s.osc("sine", sweep(t, 2400, t + 0.03, 900), perc(t, 0.3, 0.001, 0.03));
  }
  const v = m.voice(ad(t, t + 0.005, 0.1, t + 0.55), { send: 0.3 });
  if (!v) return;
  // The wobble lives in a resonant filter swinging across a saw's overtones
  // (twice per squash, as s7 rings the discs at ~3.5 Hz), so it reads on
  // small speakers; a dip at 330 Hz keeps the body from booming.
  const dip = v.filter("peaking", 330, 1);
  dip.gain.value = -6;
  const lp = v.filter("lowpass", 1000, 8, dip);
  v.lfo("sine", 7, sweep(t, 600, t + 0.55, 30), lp.frequency);
  for (const [type, level] of [["sine", 0.8], ["sawtooth", 0.22]] as const) {
    const o = v.osc(type, hz(55), level, lp);
    v.lfo("sine", 7, sweep(t, 30, t + 0.55, 1), o.frequency);
  }
}

/**
 * s7-morph's anchors: the pull on b24.5; the first pair reaches the core on
 * b25; the core throws out arms (ARM: 0.22 s out, full length 0.1223 s after
 * the bridge) and yanks the second pair in on b25.5; the sides wind up and
 * slam in on b26, throwing up a drop whose thread snaps on b26.25 and which
 * plops back on b26.5 as the blob inhales; the squat on b26.75 that the morph
 * springs out of; and the landing on b27.5 with one rebound. Every tail is
 * gone before the breath at b27.75.
 */
function liquid(
  m: Mix,
  pull: number,
  top: number,
  bottom: number,
  merge: number,
  snap: number,
  inhale: number,
  morph: number,
  land: number,
): void {
  // The discs drift out, then the pull.
  whoosh(m, swell(pull - 0.2, pull, 0.04, top - 0.01, 0.08, top + 0.2), sweep(pull - 0.2, 500, top, 1400), 3, {
    send: 0.35,
  });
  // Bridges snap on the beat: the first pair, the second, then the sides with the merge.
  bloop(m, top, hz(74), 0.13, -0.35);
  bloop(m, top + 0.018, hz(78), 0.11, 0.35);
  const arms = bottom + 0.1223 - 0.22;
  // The core throws out arms and yanks the second pair in.
  whoosh(m, ad(arms, bottom - 0.004, 0.07, bottom + 0.05), sweep(arms, 700, bottom, 2400), 2.5, { send: 0.2 });
  bloop(m, bottom, hz(71), 0.13, -0.3);
  bloop(m, bottom + 0.018, hz(76), 0.11, 0.3);
  const rush = merge - 0.14;
  // The sides rush in and slam: the merge.
  m.duck(merge, 0.3, 0.2);
  const rushEnv = swell(rush, rush + 0.05, 0.02, merge - 0.004, 0.12, merge + 0.02);
  whoosh(m, rushEnv, sweep(rush, 400, merge, 1500), 1.6, { send: 0.15 });
  bloop(m, merge, hz(62), 0.26, 0, 0.4);
  bloop(m, merge + 0.012, hz(69), 0.12, -0.4);
  bloop(m, merge + 0.024, hz(74), 0.1, 0.4);
  const g = m.voice(perc(merge, 0.32, 0.004, 0.4), { send: 0.2 });
  if (g) g.osc("sine", sweep(merge, 90, merge + 0.25, 45));
  // The drop rises out of the top on a thinning thread that snaps, then plops back in.
  // An elastic tink, bright enough to cut through the merge's low tail.
  m.duck(snap, 0.2, 0.08);
  const p = m.voice(perc(snap, 0.15, 0.0008, 0.08), { pan: 0.08, send: 0.25 });
  if (p) {
    p.osc("sine", sweep(snap, hz(86), snap + 0.025, hz(98)));
    p.osc("triangle", sweep(snap, hz(98), snap + 0.02, hz(105)), perc(snap, 0.3, 0.0005, 0.025));
    p.noise("white", perc(snap, 0.45, 0.0004, 0.008), p.filter("bandpass", 4500, 1.5), 1, snap + 0.02);
  }
  // The drop plops back in.
  bloop(m, inhale, hz(86), 0.12, -0.15, 0.16);
  // The blob breathes in on b26.5 and out into the morph.
  const breath: Pt[] = [[merge + 0.05, 0], [inhale, 0.06], [morph, 0.025], [land - 0.05, 0.0001, "exp"], [land - 0.046, 0]];
  whoosh(m, breath, [[merge, 500], [inhale, 1300, "exp"], [land - 0.05, 600, "exp"]], 1.2, { send: 0.3 });
  // The morph: a liquid squelch rising into the silhouette.
  const v = m.voice(ad(morph, land - 0.03, 0.09, land + 0.09), { send: 0.25, hold: true });
  if (v) {
    const lp = v.filter("lowpass", [[morph, 300], [land, 2200, "exp"], [land + 0.09, 900, "exp"]], 12);
    const o = v.osc("triangle", sweep(morph, hz(50), land, hz(62)), 1, lp);
    v.lfo("sine", 4.2, [[morph, 0.5], [land, 14, "exp"], [land + 0.09, 2, "exp"]], o.frequency);
  }
  // It stops dead on b27.5 and rebounds once: s7 rings it as exp(-10 d)
  // sin(2 pi 4.2 d), whose first peak comes atan(2 pi 4.2 / 10) / (2 pi 4.2) later.
  const w = 2 * Math.PI * 4.2;
  const rebound = land + Math.atan(w / 10) / w;
  m.duck(land, 0.25, 0.08);
  thump(m, land, 0.55, 130, 55, 0.09);
  bloop(m, land, hz(62), 0.14, 0, 0.08);
  bloop(m, rebound, hz(69), 0.05, 0, 0.04);
}

/**
 * The build into bar 8: noise and rising fifths swell from b26 under a
 * filtered roll that doubles its rate every half bar (eighths, sixteenths,
 * thirty-seconds), all cutting dead on `gap` for a sixteenth of silence
 * before the downbeat.
 */
function riser(m: Mix, t0: number, gap: number): void {
  const cut = (a: number, peak: number): Pt[] => [
    [t0, 0],
    [t0 + 0.25, a],
    [gap - 0.006, peak, "exp"],
    [gap, 0.0001, "exp"],
    [gap + 0.003, 0],
  ];
  const v = m.voice(cut(0.012, 0.2), { send: 0.05, hold: true });
  if (v) v.noise("white", 1, v.filter("bandpass", sweep(t0, 400, gap, 7000), 1.1));
  const s = m.voice(cut(0.02, 0.16), { send: 0.06, hold: true });
  if (s) {
    // Fifths rising an octave, A over E, into the D of the downbeat.
    const lp = s.filter("lowpass", sweep(t0, 400, gap, 5000), 3);
    for (const det of [-10, 10]) s.osc("sawtooth", sweep(t0, hz(45), gap, hz(57)), 0.5, lp, det);
    s.osc("sawtooth", sweep(t0, hz(52), gap, hz(64)), 0.35, lp);
  }
  const r = m.voice(cut(0.01, 0.2), { send: 0.04, hold: true, pan: 0.05 });
  if (r) {
    // The gate opens every eighth until b27, every sixteenth until b27.5,
    // then every 32nd. Each rate runs whole cycles, so a mid-sound entry can
    // start the gate on the next period boundary and stay on the grid.
    const am = r.vca(0.5, r.filter("bandpass", sweep(t0, 900, gap, 3600), 0.9));
    const period = (t: number) => (t < bt(27) ? 2 * X : t < bt(27.5) ? X : X / 2);
    const cycle = (t: number) => t0 + Math.ceil((t - t0) / period(t) - 1e-9) * period(t);
    r.lfo("square", [[t0, 2 / BEAT], [bt(27), 4 / BEAT, "set"], [bt(27.5), 8 / BEAT, "set"]], 0.5, am.gain, cycle);
    r.noise("white", 1, am);
  }
}

// Bar 8, "Logo resolve".

function resolve(m: Mix, t: number): void {
  m.duck(t, 0.7, 0.4);
  const sub = m.voice(perc(t, 0.42, 0.006, 1.6), { hold: true });
  if (sub) {
    sub.osc("sine", [[t, 120], [t + 0.07, 60, "exp"], [t + 1.5, 36.7, "exp"]]);
    sub.osc("sine", [[t, 240], [t + 0.07, 120, "exp"], [t + 1.5, 73.4, "exp"]], 0.15);
  }
  const k = m.voice(perc(t, 0.27, 0.001, 0.2));
  if (k) {
    k.osc("sine", sweep(t, 220, t + 0.03, 80));
    k.noise("white", perc(t, 0.22, 0.0004, 0.015), k.filter("highpass", 2000, 0), 1, t + 0.03);
  }
  // Dmaj9, wide, through a closing filter.
  const env: Pt[] = [[t, 0], [t + 0.004, 0.14], [t + 0.35, 0.09, "exp"], [t + 1.6, 0.0001, "exp"], [t + 1.604, 0]];
  for (const side of [-1, 1]) {
    const v = m.voice(env, { pan: side * 0.45, send: 0.35, hold: true });
    if (!v) continue;
    const lp = v.filter("lowpass", [[t, 7000], [t + 0.5, 1400, "exp"], [t + 1.6, 700, "exp"]], 1);
    for (const n of [50, 57, 64, 66, 73]) v.osc(warmOf(m.ac, m.sh), hz(n), 0.22, lp, side * 8);
  }
  // A soft crash: pink noise with a cymbal's shape (a bell around 6.5 kHz,
  // rolled off above 10 kHz so the master's air shelf adds no fizz), then
  // the shockwave ring.
  const c = m.voice(perc(t, 0.3, 0.002, 1.4), { send: 0.35, hold: true });
  if (c) {
    const bell = c.filter("peaking", 6500, 1.2, c.filter("lowpass", sweep(t, 11000, t + 1.2, 7500), 0));
    bell.gain.value = 5;
    c.noise("pink", 1, c.filter("highpass", sweep(t, 4200, t + 1.2, 2400), 0, bell));
  }
  whoosh(m, ad(t, t + 0.008, 0.2, t + 0.7), sweep(t, 3000, t + 0.6, 300), 2, { send: 0.3, hold: false });
  clap(m, t, 0.45);
}

function face(m: Mix, t0: number): void {
  // Brows, eyes, mustache, monocle, bow tie, tape, label: one per sixteenth,
  // climbing the D major arpeggio.
  [74, 78, 81, 86, 88, 90, 93].forEach((n, k) => {
    pop(m, t0 + k * X, hz(n), 0.2, (k % 2 ? -1 : 1) * 0.2, 0.25);
  });
  // Small echoes of scene 2 under the pops. s8 seats the monocle and slams
  // the label 4 ms before their sixteenths (MONO_SEAT, LABEL_HIT).
  flick(m, t0, 0, 0.08);
  boing(m, t0 + 2 * X, 0.1, hz(62), 0.25);
  clink(m, t0 + 3 * X - 0.004, 0.12, 0.2);
  flutter(m, t0 + 4 * X, 0.12, 0.12);
  stamp(m, t0 + 6 * X - 0.004, 0.35, 0.1);
  // s8 tapeAt: the tape swipes across the top from 25 ms before its
  // sixteenth, fastest into the cue, and lands softly down the side.
  const tp = t0 + 5 * X;
  const v = m.voice(
    [[tp - 0.025, 0], [tp - 0.021, 0.6], [tp, 0.4], [tp + 0.06, 0.1, "exp"], [tp + 0.19, 0.0001, "exp"], [tp + 0.194, 0]],
    { pan: line(tp - 0.025, -0.3, tp + 0.19, 0.4), send: 0.12, hold: true },
  );
  if (v) {
    const am = v.vca(0.6, v.filter("bandpass", sweep(tp - 0.025, 3200, tp + 0.19, 1500), 0.9));
    v.lfo("sawtooth", sweep(tp - 0.025, 130, tp + 0.19, 30), 0.4, am.gain);
    v.noise("crackle", 2.5, am, sweep(tp - 0.025, 2.6, tp + 0.19, 0.7));
    v.noise("white", 0.8, am);
  }
}

/**
 * s8 leanAt: the close-up snaps back out from 4 ms before b29 on a
 * front-loaded curve, most of the way in its first 100 ms, revealing the
 * lockup below.
 */
function snapOut(m: Mix, t: number): void {
  whoosh(m, ad(t, t + 0.014, 0.16, t + 0.34), sweep(t, 2600, t + 0.3, 420), 1.1, { send: 0.15 });
  const s = m.voice(perc(t, 0.12, 0.004, 0.18), { send: 0.1 });
  if (s) s.osc("sine", sweep(t, 180, t + 0.15, 70));
}

function wordmark(m: Mix, t: number, tag: number, url: number): void {
  whoosh(m, ad(t, t + 0.15, 0.2, t + 0.5), sweep(t, 2000, t + 0.3, 7000), 1.5, { send: 0.35 }, "white");
  // The tagline and the URL tick in.
  tick(m, tag, 2100, 0.06, -0.1, 0.2);
  tick(m, url, 2500, 0.05, 0.1, 0.2);
}

// Groove, bass, and pads.

function groove(m: Mix): void {
  // Kicks and claps per bar (sixteenth indices); claps on 2 and 4 throughout.
  const K = [[], [0, 8], [0, 8], [0, 8, 10], [0, 6, 8], [0, 8, 11]];
  const C = [[], [4, 12], [4, 12], [4, 12], [4, 12], [4, 12]];
  for (let b = 1; b <= 5; b++) {
    const t0 = b * BAR;
    for (const s of K[b]) {
      m.kick(t0 + s * X);
      kick(m, t0 + s * X, s % 8 === 0 ? 1 : 0.7);
    }
    for (const s of C[b]) {
      clap(m, t0 + s * X, 1);
    }
    for (let s = 0; s < 16; s++) {
      if (s % 4 === 2) {
        hat(m, t0 + s * X, 1);
      } else if ((s === 7 || s === 15) && b >= 3 && !(b === 5 && s === 15)) {
        hat(m, t0 + s * X, 0.4);
      }
    }
  }
  // Bar 7: drums out, only a lowpassed heartbeat under the breakdown.
  for (const t of [bt(24), bt(26)]) {
    kick(m, t, 0.5, true);
  }
}

function bassline(m: Mix): void {
  // Roots per bar: D, Bm, G, A, Bm. Octave pickup on the last sixteenth pair.
  const roots = [0, 38, 35, 31, 33, 35];
  const pattern = [[0, 6, 0, 1], [6, 2, 0, 1], [8, 6, 0, 1], [14, 2, 12, 0.7]];
  // The lower the root, the further its harmonics sit from the 210 Hz band
  // that carries them to small speakers, so the low bars get more grit.
  const grit = [0, 0.3, 0.45, 0.9, 0.75, 0.5];
  for (let b = 1; b <= 5; b++) {
    const t0 = b * BAR;
    bassRun(
      m,
      pattern.map(([s, len, iv, vel]): [number, number, number, number] => [t0 + s * X, t0 + (s + len) * X, roots[b] + iv, vel]),
      0.08,
      360,
      grit[b],
    );
  }
  // Breakdown: long, darker notes on G then A, gone before the breath.
  bassRun(m, [[bt(24), bt(26) - 0.02, 31, 1]], 0.02, 220, 0);
  bassRun(m, [[bt(26), GAP - 0.03, 33, 1]], 0.02, 220, 0);
}

function pads(m: Mix): void {
  // Bar 1 builds on A sus: the pad swells and opens as the box assembles,
  // then falls away over the last sixteenth so the drop lands on contrast.
  pad(m, bt(1), BAR - X, [45, 52, 57, 59, 62], 0.1, glide(bt(1), BAR, 300, 2600, (u) => u * u), 0.9, X);
  const chords: [number, number[], number][] = [
    [1, [50, 57, 62, 64, 66], 1500],
    [2, [47, 54, 57, 62, 64], 1400],
    [3, [43, 50, 54, 59, 62], 1500],
    [4, [45, 52, 59, 61, 64], 1700],
    [5, [47, 54, 57, 62, 73], 1800],
  ];
  for (const [b, notes, cutoff] of chords) {
    pad(m, b * BAR, (b + 1) * BAR, notes, 0.07, cutoff);
  }
  // Breakdown: an open Gmaj9, then A sus4 opening toward the breath.
  pad(m, bt(24), bt(26), [43, 50, 59, 66, 69], 0.06, glide(bt(24), bt(26), 900, 2000, (u) => u), 0.15, 0.3, 200);
  pad(m, bt(26), GAP - 0.03, [45, 52, 62, 64, 69], 0.06, glide(bt(26), GAP, 1200, 3200, (u) => u * u), 0.1, 0.024, 200);
  // Bar 8: a soft Dmaj9 afterglow under the logo, gone before the end.
  pad(m, bt(28), 14.45, [50, 57, 62, 64, 66, 73], 0.07, 1300, 0.25, 0.35);
}

function compose(m: Mix): void {
  // Bar 1: no drums, only the build.
  ignition(m, 0);
  penDraw(m, 0, bt(1));
  flood(m, bt(1));
  crouch(m, bt(1.75), bt(2));
  for (let i = 0; i < 4; i++) fold(m, bt(2) + i * X, i);
  lid(m, bt(3), 1.305);
  tape(m, bt(3.25), bt(3.75));
  // Bar 2.
  creak(m, bt(4), bt(4.5));
  launch(m, bt(4.5), bt(5.5));
  land(m, bt(5.5));
  // Eyes pop.
  m.duck(bt(6), 0.15, 0.1);
  pop(m, bt(6), hz(81), 0.3, -0.2);
  pop(m, bt(6) + 0.014, hz(86), 0.26, 0.2);
  // Brows flick.
  flick(m, bt(6.25), -0.15);
  flick(m, bt(6.25) + 0.012, 0.15);
  // The mustache unfurls.
  m.duck(bt(6.5), 0.2, 0.2);
  boing(m, bt(6.5));
  // The label stamps on.
  m.duck(bt(6.75), 0.3, 0.1);
  stamp(m, bt(6.75));
  monocle(m, bt(7));
  // The monocle glints.
  m.duck(bt(7.25), 0.25, 0.2);
  ding(m, bt(7.25), hz(93), 0.2, 0.25, 0.7);
  // The bow tie spins in.
  m.duck(bt(7.5), 0.15, 0.12);
  flutter(m, bt(7.5));
  // A blink.
  blip(m, bt(7.75), hz(93), 0.12);
  blip(m, bt(7.75) + 0.07, hz(97), 0.08);
  // Bar 3.
  dive(m, bt(8), bt(8.5));
  typing(m, bt(8.5), bt(8.9));
  slamDrop(m, bt(9));
  gitMain(m, bt(9.25), bt(9.75) - 0.03);
  slamSlide(m, bt(10), bt(9.75));
  glideAnd(m, bt(10.25));
  slamGlitch(m, bt(10.5), bt(10.5) + 0.05);
  scatter(m, bt(11));
  flyToNodes(m, bt(11), bt(12) - 0.0175);
  // Bar 4.
  nodes(m, bt(12));
  boxPop(m, bt(12.5));
  stream(m, bt(12.5), bt(12.5) + 0.33);
  [bt(13), bt(13.25), bt(13.5), bt(13.75)].forEach((t, i) => gulp(m, t, i));
  restore(m, bt(13.75) + 0.012, [bt(14), bt(14.5), bt(15)], bt(15.5));
  whip(m, bt(16));
  // Bar 5.
  cargoBar(m);
  mbxBar(m, bt(17));
  delta(m, bt(18), bt(18.25), bt(18.5));
  extrude(m, bt(19), bt(19.25), bt(20));
  // Bar 6.
  world(m, bt(20));
  prune(m, bt(22));
  keptHop(m, bt(23), bt(24) - 0.02);
  // Bar 7.
  jelly(m, bt(24));
  liquid(m, bt(24.5), bt(25), bt(25.5), bt(26), bt(26.25), bt(26.5), bt(26.75), bt(27.5));
  riser(m, bt(26), GAP);
  // Bar 8.
  resolve(m, bt(28));
  face(m, bt(28.5));
  snapOut(m, bt(29) - 0.004);
  wordmark(m, bt(29), bt(29.5), bt(29.75));
  // A blink.
  blip(m, bt(30.5), hz(93), 0.07);
  blip(m, bt(30.5) + 0.07, hz(97), 0.05);
  // The final glint.
  ding(m, bt(31), hz(86), 0.2, 0.15, 0.9);
  ding(m, bt(31) + 0.003, hz(93), 0.1, 0.25, 0.7);
  // Music.
  groove(m);
  bassline(m);
  pads(m);
}

type Dip = readonly [t: number, depth: number, release: number];

/** Duck curves are sampled on a fixed grid from reel time 0. */
const DIP_STEP = 0.0025;

/**
 * A gain curve over the whole reel that is the product of dips: each accent
 * pulls the gain down over 8 ms and lets it recover exponentially (to under
 * 1% of its depth after five time constants).
 */
function dips(lists: readonly (readonly [readonly Dip[], number])[]): Floats {
  const n = Math.ceil((DURATION + 0.1) / DIP_STEP) + 1;
  const vals = floats(n).fill(1);
  for (const [list, scale] of lists) {
    for (const [te, d, r] of list) {
      const i0 = Math.max(0, Math.ceil((te - 0.008) / DIP_STEP));
      const i1 = Math.min(n - 1, Math.floor((te + r * 5) / DIP_STEP));
      const k = Math.exp(-DIP_STEP / r);
      let s = 0;
      for (let i = i0; i <= i1; i++) {
        const dt = i * DIP_STEP - te;
        if (dt < 0) vals[i] *= 1 - d * scale * ((dt + 0.008) / 0.008);
        else {
          // The recovery advances by a fixed ratio per step.
          s = s ? s * k : Math.exp(-dt / r);
          vals[i] *= 1 - d * scale * s;
        }
      }
    }
  }
  return vals;
}

/**
 * Play a whole-reel dip curve on `p` from reel time `t0`. Chromium moves a
 * curve whose start has already passed up to the clock, which would put
 * every dip after it late. The audio thread can run a whole device buffer
 * ahead of the clock the page reads, so a live score joins the curve at
 * least CURVE_MARGIN ahead of it, easing in from the resting gain over 10
 * ms; only dips in the first moments of a cold start are lost.
 */
function dipCurve(m: Mix, p: AudioParam, t0: number, vals: Floats): void {
  let k = Math.ceil(t0 / DIP_STEP - 1e-9);
  const late = m.live ? Math.ceil((m.clock(CURVE_MARGIN) - 1e-9) / DIP_STEP) : 0;
  const skip = late > k;
  k = Math.min(vals.length - 2, Math.max(k, late));
  const rest = vals.slice(k);
  if (skip) for (let i = 0; i < Math.min(4, rest.length); i++) rest[i] = 1 + (rest[i] - 1) * (i / 4);
  p.setValueCurveAtTime(rest, m.at(k * DIP_STEP), (rest.length - 1) * DIP_STEP);
}

/**
 * What a score's first pass records never changes between scores (only
 * which voices get built does), so it is worked out once per page, by the
 * first score: the two duck curves.
 */
interface Plan {
  drums: Floats;
  music: Floats;
}
let planCache: Plan | null = null;

function planOf(m: Mix): Plan {
  const pump = m.kicks.map((t): Dip => [t, 0.45, 0.1]);
  return {
    // The groove ducks under the effects; bass and pads also pump with the kick.
    drums: dips([[m.ducks, 0.6]]),
    music: dips([
      [m.ducks, 1],
      [pump, 1],
    ]),
  };
}

/**
 * Schedule the soundtrack into `dest`, starting `from` seconds into the reel
 * at audio-context time `when`. An offline context gets the whole score at
 * once; a live one gets its first moments before this returns and the rest
 * from a timer that stays a little over a second ahead of the clock.
 */
export function playScore(ac: BaseAudioContext, dest: AudioNode, from: number, when: number): ScoreHandle {
  if (!(from < DURATION - 0.03)) return { stop() {} };
  const sh = shared(ac);
  const live = !("startRendering" in ac);
  // Nearly every voice uses white noise; making it now, before any voice
  // checks the clock, keeps that cost out of the first voice that needs it.
  noiseOf(ac, sh, "white");

  // Master: a touch less sub and more air, a glue compressor, makeup, a soft
  // ceiling, the end fade, and the stop() fade.
  const pre = ac.createGain();
  const low = ac.createBiquadFilter();
  low.type = "lowshelf";
  low.frequency.value = 90;
  low.gain.value = -1.5;
  const air = ac.createBiquadFilter();
  air.type = "highshelf";
  air.frequency.value = 7000;
  air.gain.value = 2.5;
  const comp = ac.createDynamicsCompressor();
  comp.threshold.value = -18;
  comp.knee.value = 10;
  comp.ratio.value = 2;
  comp.attack.value = 0.005;
  const makeup = ac.createGain();
  // Half, because the ceiling curve's domain is ±2.
  makeup.gain.value = 0.5 * 1.19;
  const ceiling = ac.createWaveShaper();
  ceiling.curve = sh.ceiling;
  const tail = ac.createGain();
  const kill = ac.createGain();
  pre.connect(low).connect(air).connect(comp).connect(makeup).connect(ceiling).connect(tail).connect(kill).connect(dest);

  const sfx = ac.createGain();
  sfx.connect(pre);
  const drums = ac.createGain();
  drums.gain.value = 0.62;
  const drumDuck = ac.createGain();
  drums.connect(drumDuck).connect(pre);
  const music = ac.createGain();
  music.gain.value = 0.9;
  const musicDuck = ac.createGain();
  music.connect(musicDuck).connect(pre);

  const verb = ac.createGain();
  const conv = ac.createConvolver();
  // Building the impulse costs ~10 ms of main thread; a live score does it
  // from its first timer tick, which still lands well before `when`.
  if (!live) conv.buffer = roomOf(ac, sh);
  const verbLow = ac.createBiquadFilter();
  verbLow.type = "highpass";
  verbLow.frequency.value = 220;
  const verbOut = ac.createGain();
  verbOut.gain.value = 2.5;
  const verbGate = ac.createGain();
  verb.connect(conv).connect(verbLow).connect(verbOut).connect(verbGate).connect(pre);

  const m = new Mix(ac, sh, from, when, { sfx, drums, music }, verb, live);
  let stopped = false;
  let built = -Infinity;
  const build = (to: number): void => {
    m.lo = built;
    m.hi = to >= DURATION ? Infinity : to;
    m.floor = m.clock(live ? MARGIN : 0);
    compose(m);
    m.collect = false;
    built = to;
  };
  // The first sounds go out first. The first score on a page also records
  // the whole score's ducks and kicks in this pass; later ones reuse them.
  m.collect = !planCache;
  build(live ? from + FIRST : DURATION);
  const p = (planCache ??= planOf(m));
  const start = m.clock(live ? MARGIN : 0);

  // A fresh compressor starts clamped down and takes ~200 ms to open; a fast
  // release until just after the first sound lets it settle at once.
  comp.release.value = 0.001;
  comp.release.setValueAtTime(0.2, m.at(start) + 0.03);
  dipCurve(m, drumDuck.gain, start, p.drums);
  dipCurve(m, musicDuck.gain, start, p.music);
  // The breath before the resolve: the room goes quiet with everything else.
  m.set(verbGate.gain, [[GAP - 0.012, 1], [GAP + 0.01, 0.02, "exp"], [bt(28) - 0.004, 0.02], [bt(28), 1]]);
  // Everything, reverb included, is silent before the last frame.
  m.set(tail.gain, [[14.6, 1], [14.86, 0.25, "exp"], [14.965, 0.0002, "exp"], [14.972, 0]]);

  let timer: ReturnType<typeof setTimeout> | undefined;
  const topUp = (): void => {
    timer = undefined;
    if (stopped) return;
    const clock = from + (ac.currentTime - when);
    const to = Math.min(DURATION, Math.max(clock, from) + AHEAD);
    if (to > built) build(to);
    if (built < DURATION) timer = setTimeout(topUp, TICK_MS);
  };
  if (live) {
    timer = setTimeout(() => {
      timer = undefined;
      if (stopped) return;
      conv.buffer = roomOf(ac, sh);
      if (built < DURATION) timer = setTimeout(topUp, 0);
    }, 0);
  }

  return {
    stop() {
      if (stopped) return;
      stopped = true;
      if (timer !== undefined) clearTimeout(timer);
      const now = ac.currentTime;
      kill.gain.cancelScheduledValues(now);
      kill.gain.setValueAtTime(1, now);
      kill.gain.linearRampToValueAtTime(0, now + 0.03);
      for (const s of m.sources) {
        try {
          s.stop(now + 0.035);
        } catch {
          // Never started.
        }
      }
      const release = () => {
        try {
          kill.disconnect();
        } catch {
          // Already disconnected.
        }
      };
      setTimeout(release, 120);
    },
  };
}
