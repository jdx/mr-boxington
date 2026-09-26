// Section 11, "Next push, measured": the published next-commit benchmark as
// a bar chart. The chart arrives on the ci section's whip (map.ts drawWhip),
// its layers trailing one another in from the right. Cargo alone and Cargo
// with mbx leave the zero line together: Cargo lurches forward crate by
// crate while mbx springs to its mark, and at that instant both tips are
// flush, because Cargo has run just as long. A strip of mbx's lookups slides
// in under its bar, one tile per compilation with the compiled ones amber,
// and the hatched delta lands. The finished chart holds long enough to read.
// Then the page falls away in perspective while the camera swings around the
// mbx bar, which compacts into a square and pops out of the page as the cube
// that opens the isometric world.
//
// Every figure comes from the published run (facts.ts). Without a commit
// fact, which is only published when the two tools' runs are separated(),
// no bar is drawn and the chart gives way to a card pointing at the
// benchmarks page.

import { BEAT, CUBE, PALETTE, type Scene, sec, WHIP, WORLD_CAM } from "../bible";
import { drawShadow, OUTLINE, OUTLINE_RATIO } from "../box";
import { mix, rgba } from "../color";
import { type CommitFact, delta, medianOf, type ReelFacts, tenths } from "../facts";
import { glow, makeCanvas, smear } from "../fx";
import { drawHandoff, drawWhip } from "../map";
import {
  clamp,
  cubicBezier,
  inCubic,
  inOutSine,
  type Key,
  keys,
  lerp,
  outCubic,
  outQuart,
  progress,
  pulse,
  smoothstep,
  spring,
  swiftInOut,
  swiftOut,
  wobble,
} from "../math";
import {
  applyMatrix,
  type Camera,
  cardboardFill,
  polygon,
  type Projected,
  tone,
  type V3,
  View,
} from "../space";
import { CAPTION, drawText, drawWords, font, layout, LABEL, type WordStyle } from "../type";

const S = sec("next-push");

/** Local time of beat `n` of the section. */
const b = (n: number): number => n * BEAT;
/**
 * One frame of the 120 fps render, which holds every 60 fps frame. Where a
 * move must be home "on the frame nearest the beat", it is timed to this.
 */
const FRAME = 1 / 120;
/**
 * The shutter for motion smears and rolling drums: one 60 fps frame of
 * travel, so the 60 fps file shows each streak joining the next.
 */
const SHUTTER = 1 / 60;

/**
 * The run's facts. Every env.facts comes from factsFromBenchmarks, whose
 * ReelFacts bible.ts still declares as its older three-field subset.
 */
const facts = (env: { facts: unknown }): ReelFacts | null => env.facts as ReelFacts | null;

// Beat map, local seconds. The score (score/data.ts) is written to these.
//
//   b0-b0.3   the whip in (the ci part's whoosh), layers trailing
//   b0.5      Cargo and mbx leave the zero line together
//   b1.25     mbx locks on its mark, flush with Cargo's tip (mbxLock)
//   b1.5-b2   the tile strip slides in under mbx, its compile lands last
//   b2.25     Cargo's last crate clunks home (CARGO_DONE)
//   b2.5      the delta's lines and hatch; its figure lands on b2.75
//   b2.75-b11 the finished chart holds (3.87 s); a glint crosses the mbx
//             bar on b3, and a light runs the strip from b7
//   b11-b12   the page falls away and the mbx bar becomes the cube

export const T = {
  race: b(0.5),
  lock: b(1.25),
  strip: b(1.5),
  amber: b(2),
  cargoLand: b(2.25),
  delta: b(2.5),
  label: b(2.75),
  glint: b(3),
  shimmer: b(7),
  clear: b(11),
  hit: b(11.25),
  end: S.len,
};
/** How long the green tiles take to sweep in, left to right. */
export const STRIP_SWEEP = b(0.375);

// Layout in chart px, which equal screen px while the camera faces the
// chart. The zero line is the spine: the title, the row labels, and the
// strip all hang from it.
const AX0 = 160;
const AXLEN = 1320;
const TH = 96;
const TITLE_Y = 186;
const SUB_Y = [248, 298] as const;
const ROW_CARGO = 552;
const ROW_MBX = 772;
/** A row label's baseline above its bar's top edge. */
const LABEL_GAP = 26;
const LABEL_SIZE = 56;
/** The delta's dimension line, above the Cargo bar. */
const BRK_Y = ROW_CARGO - TH / 2 - 42;
const GRID_TOP = ROW_CARGO - TH / 2 - 18;
const GRID_BOT = ROW_MBX + TH / 2 + 12;
const NUM_SIZE = 120;
const NUM_GAP = 30;
const DELTA_SIZE = 120;
/** The tile strip's top edge, under the mbx bar. */
const STRIP_Y = ROW_MBX + TH / 2 + 26;
/** The tile pitch the strip aims for: big enough for one amber tile to read. */
const STRIP_PITCH = 13.5;
/** At most this many rows, so the strip stays a strip. */
const STRIP_ROWS = 9;

// The card that stands in for the chart: 88 px, on the chart's rows.
// bench-refresh.yml checks weekly but reruns only when a newer mbx release
// is out, so the card says "for each release", not "every week".
export const CARD = ["Benchmarks, rerun for each release:", "mr-boxington.jdx.dev/benchmarks"] as const;
/** The chart's title, 88 px. */
export const TITLE = "CI builds the next push";
const CARD_Y = [560, 668] as const;
/** Where the card's cube pops out of the page, chart px. */
const CARD_PIVOT = { x: 960, y: 820 };

/** Chart layers whip in from here, as map.ts whipIn has them on the bar line. */
const WHIP_D = 1800;

/** The measured chart, from the commit fact. */
interface Chart {
  cargo: number;
  mbx: number;
  max: number;
  step: number;
  cargoText: string;
  mbxText: string;
  /** The saving, from the raw medians; "" when mbx was not a printable tenth faster. */
  deltaText: string;
  /** The annotation: restored, then compiled (facts.annotation, in two colours). */
  note: readonly [restored: string, compiled: string | null];
  lookups: number;
  misses: number;
  /** Where each of Cargo's steps brings its bar to rest, as fractions of its value. */
  steps: number[];
  /** The same rests for the readout, as fractions of the figure it shows. */
  readSteps: number[];
}

interface Model {
  chart: Chart | null;
  /** The 40 px subtitle, one or two lines. */
  sub: string[];
}

function niceStep(raw: number): number {
  const p = 10 ** Math.floor(Math.log10(raw));
  for (const m of [1, 2, 2.5, 5, 10]) if (m * p >= raw) return m * p;
  return 10 * p;
}

/** A step that rests exactly on mbx's mark. */
const MEET = -1;

// Cargo lurches forward crate by crate on the eighths. Its third step comes
// to rest on mbx's mark just before mbx locks there on b1.25: both started
// on b0.5, so at the instant mbx is done Cargo has run exactly as long, and
// the tips are flush. It waits there a sixteenth, lurches on while the strip
// slides in, and a last grind clunks home on b2.25. [start, end, fraction].
const LURCH = 0.1;
export const STEP_PLAN: readonly (readonly [number, number, number])[] = [
  [b(0.5), b(0.5) + LURCH, 0.14],
  [b(0.75), b(0.75) + LURCH, 0.3],
  [b(1), b(1) + LURCH, MEET],
  [b(1.5), b(1.5) + LURCH, 0.64],
  [b(1.75), b(1.75) + LURCH, 0.8],
  // Ends half a frame early so the frame nearest the beat shows it home.
  [b(2), T.cargoLand - FRAME / 2, 1],
];
export const CARGO_DONE = T.cargoLand - FRAME / 2;
/** The last step starts slow and arrives at speed, so it stops with a clunk. */
const grind = cubicBezier(0.4, 0, 0.8, 0.8);

/** The subtitle: the subject, the scenario in plain words, and the runs. */
export function subtitle(f: ReelFacts | null): string[] {
  const setup = "the cache holds the previous commit";
  const runner = "Linux CI runner";
  const median = f?.commit ? medianOf(f.commit.trials) : null;
  if (f?.subject === "hk") {
    return ["hk, a mid-size Rust CLI with C dependencies", [setup, runner, median].filter(Boolean).join(" · ")];
  }
  return [[f?.subject || null, setup].filter(Boolean).join(" · "), [runner, median].filter(Boolean).join(" · ")];
}

/** The annotation under the mbx bar, split where its colour changes. */
export function noteParts(c: CommitFact): readonly [string, string | null] {
  return [`${c.hits} of ${c.lookups} restored`, c.misses > 0 ? `${c.misses} compiled` : null];
}

/** The delta's figure, drawn after a minus; "" when there is none to claim. */
export function deltaText(c: CommitFact): string {
  const d = delta(c);
  return d === null ? "" : tenths(d);
}

let modelCache: { facts: ReelFacts | null; m: Model } | null = null;
function model(f: ReelFacts | null): Model {
  if (modelCache && modelCache.facts === f) return modelCache.m;
  const c = f?.commit ?? null;
  let chart: Chart | null = null;
  if (c) {
    const hi = Math.max(c.cargo, c.mbx);
    const step = niceStep(hi / 4);
    const max = Math.ceil((hi * 1.02) / step) * step;
    const cargoText = tenths(c.cargo);
    const mbxText = tenths(c.mbx);
    const shown = Number(cargoText);
    // Only meet mbx on the way when it is the shorter bar.
    const meet = c.mbx < c.cargo;
    // Readout rests sit on whole tenths so the figure is crisp between steps.
    const readSteps = STEP_PLAN.map(([, , f0]) => {
      if (f0 === MEET && meet) return Number(mbxText) / shown;
      const fr = f0 === MEET ? 0.47 : f0;
      return fr >= 1 ? 1 : Math.max(0.1, Math.round(fr * shown * 10) / 10) / shown;
    });
    chart = {
      cargo: c.cargo,
      mbx: c.mbx,
      max,
      step,
      cargoText,
      mbxText,
      deltaText: deltaText(c),
      note: noteParts(c),
      lookups: c.lookups,
      misses: c.misses,
      // The bar meets mbx's tip exactly; its readout meets mbx's figure.
      steps: STEP_PLAN.map(([, , fr], i) => (fr === MEET && meet ? c.mbx / c.cargo : readSteps[i])),
      readSteps,
    };
  }
  const m: Model = { chart, sub: subtitle(f) };
  modelCache = { facts: f, m };
  return m;
}

const pxPer = (c: Chart): number => AXLEN / c.max;
const cargoEnd = (c: Chart): number => AX0 + c.cargo * pxPer(c);
const mbxEnd = (c: Chart): number => AX0 + c.mbx * pxPer(c);

// Growth curves, 0..1 of each bar's value.

/** `quick` > 1 settles each lurch sooner (the readout's drums lead the bar). */
function cargoGrow(steps: readonly number[], t: number, quick = 1): number {
  let g = 0;
  let from = 0;
  for (let i = 0; i < STEP_PLAN.length; i++) {
    const [s, e0] = STEP_PLAN[i];
    if (t < s) break;
    const last = i === STEP_PLAN.length - 1;
    const e = last ? e0 : s + (e0 - s) / quick;
    g = lerp(from, steps[i], (last ? grind : swiftOut)(progress(s, e, t)));
    from = steps[i];
  }
  return g;
}

// mbx's spring is today's at a third of the speed: it launches with speed,
// reaches its mark half a frame before b1.25 (so the frame nearest the beat
// shows the tips flush), overshoots by about 7%, and settles while the strip
// slides in.
const SPRING = [3.7, 0.66, 7.75] as const;
const springAt = (u: number): number => spring(u, ...SPRING);
/** Seconds the spring takes to first reach its mark at full speed. */
const FIRST = (() => {
  let lo = 0;
  let hi = 0.5;
  for (let i = 0; i < 50; i++) {
    const mid = (lo + hi) / 2;
    if (springAt(mid) >= 1) hi = mid;
    else lo = mid;
  }
  return hi;
})();
/** When the mbx bar first reaches its value. */
const MBX_CROSS = T.lock - FRAME / 2;
const SLOW = FIRST / (MBX_CROSS - T.race);
export const mbxGrow = (t: number): number => springAt((t - T.race) * SLOW);
/** The readout locks half a frame before the bar's crossing, so the lock frame is crisp. */
export const mbxLock = (): number => MBX_CROSS - FRAME / 2;

// Whip layers. The title leads on map.ts whipIn's own track; the subtitle,
// the plot, and then the row labels trail it with more overshoot and a
// longer, dragging settle.

interface Layer {
  /** Chart-px rows this layer occupies, for its streak buffer. */
  y0: number;
  y1: number;
  x: (t: number) => number;
  buf: HTMLCanvasElement | null;
  /** Most brightness gain for the streak: thin type needs more than bold. */
  maxGain: number;
  /** The streak with its brightness gain applied. */
  gain?: HTMLCanvasElement;
}

function whipTrack(delay: number, over: number, settle: number, drag: number): (t: number) => number {
  const k: Key[] = [
    [delay, WHIP_D],
    [delay + WHIP, -over, outQuart],
  ];
  if (drag > 0) k.push([delay + WHIP + settle * 0.45, over * drag, inOutSine]);
  k.push([delay + WHIP + settle, 0, inOutSine]);
  return keys(k);
}

const L_TITLE: Layer = {
  y0: TITLE_Y - 84,
  y1: TITLE_Y + 28,
  x: whipTrack(0, 12, 0.16, 0),
  buf: null,
  maxGain: 1.8,
};
const L_SUB: Layer = {
  y0: SUB_Y[0] - 38,
  y1: SUB_Y[1] + 14,
  x: whipTrack(SHUTTER, 16, 0.18, 0),
  buf: null,
  maxGain: 2.5,
};
const L_PLOT: Layer = {
  y0: GRID_TOP - 8,
  y1: GRID_BOT + 8,
  x: whipTrack(2 * SHUTTER, 22, 0.2, 0.15),
  buf: null,
  maxGain: 1.5,
};
const L_LABELS: Layer = {
  y0: ROW_CARGO - TH / 2 - LABEL_GAP - 56,
  y1: ROW_MBX - TH / 2 - LABEL_GAP + 20,
  x: whipTrack(3 * SHUTTER, 30, 0.26, 0.2),
  buf: null,
  maxGain: 1.4,
};

/**
 * Whip blur. Drawing into a buffer squeezed `k` times horizontally and
 * stretching it back is a cheap horizontal box blur of about `k` px.
 */
function whipLayer(
  ctx: CanvasRenderingContext2D,
  L: Layer,
  t: number,
  draw: (g: CanvasRenderingContext2D) => void,
): void {
  const off = L.x(t);
  // Still entirely off the right edge (its trail extends further right).
  if (off > 1700) return;
  const vel = off - L.x(t - SHUTTER);
  // Only the whip itself streaks. The overshoot and drag are slow enough to
  // draw once, sharp: a streak there would soften text that has landed.
  if (Math.abs(vel) < 20) {
    ctx.save();
    ctx.translate(off, 0);
    draw(ctx);
    ctx.restore();
    return;
  }
  const k = clamp(Math.abs(vel) * 0.2, 3, 40);
  const cw = 1920 / 3;
  const h = L.y1 - L.y0;
  if (!L.buf) L.buf = makeCanvas(cw, h);
  const buf = L.buf;
  const g = buf.getContext("2d");
  if (!g) return;
  g.setTransform(1, 0, 0, 1, 0, 0);
  g.clearRect(0, 0, cw, h);
  g.setTransform(1 / k, 0, 0, 1, 0, -L.y0);
  g.translate(off, 0);
  draw(g);
  g.setTransform(1, 0, 0, 1, 0, 0);
  // Squeezing spreads thin type over k px and dims it; a matching gain,
  // added up in the small buffer, keeps the streak as bright as the type.
  const gain = clamp(k / 12, 1, L.maxGain);
  let src = buf;
  if (gain > 1.01) {
    if (!L.gain) L.gain = makeCanvas(cw, h);
    const o = L.gain.getContext("2d");
    if (o) {
      o.globalCompositeOperation = "source-over";
      o.globalAlpha = 1;
      o.clearRect(0, 0, cw, h);
      o.globalCompositeOperation = "lighter";
      for (let left = gain; left > 0.01; left -= 1) {
        o.globalAlpha = Math.min(1, left);
        o.drawImage(buf, 0, 0);
      }
      o.globalCompositeOperation = "source-over";
      o.globalAlpha = 1;
      src = L.gain;
    }
  }
  const n = Math.abs(vel) > 60 ? 4 : 3;
  ctx.save();
  ctx.globalCompositeOperation = "lighter";
  smear(ctx, 0, Math.max(-360, vel), () => ctx.drawImage(src, 0, 0, 1920 / k, h, 0, L.y0, 1920, h), n);
  ctx.restore();
}

// The chart plane. While the camera faces it the whole chart is one affine
// transform; once the page falls away in perspective every element is
// projected on its own, so the page really recedes rather than shearing.

interface Plane {
  /** Chart px to the context's current coordinates. */
  pt(x: number, y: number): { x: number; y: number };
  /** Append a local frame at chart (x, y), in chart px, to the context. */
  at(ctx: CanvasRenderingContext2D, x: number, y: number): void;
  /** Context px per chart px near (x, y), for stroke widths. */
  k(x: number, y: number): number;
}

const FLAT: Plane = {
  pt: (x, y) => ({ x, y }),
  at: (ctx, x, y) => ctx.translate(x, y),
  k: () => 1,
};

function projPlane(view: View, m: Model, z: number): Plane {
  const p = pivot(m);
  const w = (x: number, y: number): V3 => [(x - p.x) / S0, CUBE / 2 - (y - p.y) / S0, z];
  const frame = (x: number, y: number) => view.planeMatrix(w(x, y), [1 / S0, 0, 0], [0, -1 / S0, 0]);
  return {
    pt: (x, y) => view.project(w(x, y)),
    at: (ctx, x, y) => applyMatrix(ctx, frame(x, y)),
    k: (x, y) => {
      const f = frame(x, y);
      return Math.sqrt(Math.abs(f.a * f.d - f.b * f.c));
    },
  };
}

function quad(ctx: CanvasRenderingContext2D, P: Plane, x: number, y: number, w: number, h: number): void {
  const a = P.pt(x, y);
  const b2 = P.pt(x + w, y);
  const c = P.pt(x + w, y + h);
  const d = P.pt(x, y + h);
  ctx.beginPath();
  ctx.moveTo(a.x, a.y);
  ctx.lineTo(b2.x, b2.y);
  ctx.lineTo(c.x, c.y);
  ctx.lineTo(d.x, d.y);
  ctx.closePath();
}

function seg(ctx: CanvasRenderingContext2D, P: Plane, x0: number, y0: number, x1: number, y1: number): void {
  const a = P.pt(x0, y0);
  const c = P.pt(x1, y1);
  ctx.moveTo(a.x, a.y);
  ctx.lineTo(c.x, c.y);
}

/** Opacity of the page's text, bars, and grid as it falls away. */
interface Fade {
  text: number;
  bar: number;
  line: number;
  /** 0..1 how far the page has fallen: the grid brightens as a spatial cue. */
  fall: number;
}
const OPAQUE: Fade = { text: 1, bar: 1, line: 1, fall: 0 };

// Odometer readout. Each drum is drawn once per frame at its exact position,
// and every frame must read. The upper drums only turn while the lowest one
// carries, so they are always drawn sharp, the next digit sliding through the
// window. The lowest drum is sharp while slow, lightly blurred at a walk, and
// once it spins faster than the eye can follow it ticks: a crisp digit from
// the value each frame with one faint ghost, like a strobed counter wheel.

const NUM_FONT = font(NUM_SIZE, 600);
/** Drum pitch, and the visible band above and below the baseline. */
const LH = NUM_SIZE;
const UP = 0.82 * NUM_SIZE;
const DN = 0.12 * NUM_SIZE;
/** Digit center above the baseline; the band is centered on it. */
const MID = 0.35 * NUM_SIZE;

function drumPositions(text: string, value: number): number[] {
  const dot = text.indexOf(".");
  const decimals = dot < 0 ? 0 : text.length - dot - 1;
  const digits = text.replace(".", "").length;
  const pos: number[] = new Array(digits);
  // Round away float dust so a settled drum sits exactly on its digit.
  pos[0] = Math.round(Math.max(0, value) * 10 ** decimals * 1e4) / 1e4;
  for (let k = 1; k < digits; k++) {
    const lower = pos[k - 1];
    const m = lower - Math.floor(lower / 10) * 10;
    pos[k] = Math.floor(lower / 10) + Math.max(0, m - 9);
  }
  return pos;
}

function numMetrics(ctx: CanvasRenderingContext2D): { slot: number; dot: number; unit: number } {
  let slot = 0;
  for (const g of layout(ctx, "0123456789", NUM_FONT).glyphs) slot = Math.max(slot, g.w);
  return {
    slot: slot * 0.96,
    dot: layout(ctx, ".", NUM_FONT).width * 0.9,
    unit: NUM_SIZE * 0.14 + layout(ctx, "s", font(NUM_SIZE * 0.5, 500)).width,
  };
}

/** A readout's width in chart px, digits through the unit. */
function readoutWidth(ctx: CanvasRenderingContext2D, text: string): number {
  const nm = numMetrics(ctx);
  let w = nm.unit;
  for (const ch of text) w += ch === "." ? nm.dot : nm.slot;
  return w;
}

/** Leading drums (tens and up) hide their zero; returns how much of it shows. */
function leadVisible(text: string, value: number): number {
  const dot = text.indexOf(".");
  const intDigits = dot < 0 ? text.length : dot;
  if (intDigits < 2) return 1;
  const pos = drumPositions(text, value);
  return clamp(pos[pos.length - 1]);
}

// Digit strip for the lowest drum at a walk: 8, 9, 0…9, 0, 1, 2, so any
// window and its blur stay inside. The blur is short (about a fifth of the
// pitch) so a digit's upright strokes keep the sharp glyph's full brightness.
const STRIP_N = 15;
const BLUR_R = 0.2 * LH;
const BLUR_TAPS = 7;
const strips = new Map<string, HTMLCanvasElement>();
function strip(color: string, slot: number): HTMLCanvasElement {
  // Keyed by the measured slot so a late web-font swap rebuilds them.
  const key = `${color}|${slot.toFixed(2)}`;
  const hit = strips.get(key);
  if (hit) return hit;
  if (strips.size > 12) strips.clear();
  const w = Math.ceil(slot) + 8;
  const h = STRIP_N * LH;
  const sharp = makeCanvas(w, h);
  const g = sharp.getContext("2d")!;
  g.font = NUM_FONT;
  g.textAlign = "center";
  g.textBaseline = "alphabetic";
  g.fillStyle = color;
  for (let i = 0; i < STRIP_N; i++) g.fillText(String((i + 8) % 10), w / 2, i * LH + LH / 2 + MID);
  // Vertical box blur by summing shifted copies ("lighter" adds exactly).
  const out = makeCanvas(w, h);
  const o = out.getContext("2d")!;
  o.globalCompositeOperation = "lighter";
  o.globalAlpha = 1 / BLUR_TAPS;
  for (let i = 0; i < BLUR_TAPS; i++) o.drawImage(sharp, 0, lerp(-BLUR_R, BLUR_R, i / (BLUR_TAPS - 1)));
  strips.set(key, out);
  return out;
}

/** Background-colored falloff at a turning drum's top and bottom edges, clear of the glyphs. */
let drumFade: HTMLCanvasElement | null = null;
function fadeSprite(): HTMLCanvasElement {
  if (drumFade) return drumFade;
  drumFade = makeCanvas(4, 64);
  const g = drumFade.getContext("2d")!;
  const grad = g.createLinearGradient(0, 0, 0, 64);
  grad.addColorStop(0, rgba(PALETTE.bg, 1));
  grad.addColorStop(0.13, rgba(PALETTE.bg, 0));
  grad.addColorStop(0.87, rgba(PALETTE.bg, 0));
  grad.addColorStop(1, rgba(PALETTE.bg, 1));
  g.fillStyle = grad;
  g.fillRect(0, 0, 4, 64);
  return drumFade;
}

/** A digit on a drum at `d` (any integer), skipped when it is a hidden leading zero. */
function drumDigit(ctx: CanvasRenderingContext2D, d: number, cx: number, y: number, lead: boolean): void {
  const digit = ((d % 10) + 10) % 10;
  if (lead && digit === 0) return;
  ctx.fillText(String(digit), cx, y);
}

/**
 * One drum centered at `cx`, baseline `y`, at position `q` turning `v`
 * digits per shutter. `low` marks the lowest drum, the only one that spins.
 */
function drawDrum(
  ctx: CanvasRenderingContext2D,
  cx: number,
  y: number,
  q: number,
  v: number,
  color: string,
  lead: boolean,
  slot: number,
  low: boolean,
): void {
  const a0 = ctx.globalAlpha;
  // Strobe: a crisp digit from the value, with a faint trail below it.
  const strobe = low ? smoothstep(0.45, 0.75, v) : smoothstep(1.3, 1.7, v);
  const roll = 1 - strobe;
  const blur = low ? roll * clamp((v - 0.12) / 0.23) : 0;
  const sharp = roll - blur;
  if (strobe > 0.004) {
    const d = Math.round(q);
    // The lowest drum trails a short smear below: the wheel turns upward.
    if (low) {
      ctx.globalAlpha = a0 * strobe * 0.16;
      drumDigit(ctx, d, cx, y + 0.17 * LH, lead);
    }
    ctx.globalAlpha = a0 * strobe;
    drumDigit(ctx, d, cx, y, lead);
  }
  if (sharp > 0.004) {
    const d0 = Math.floor(q);
    const fr = q - d0;
    ctx.globalAlpha = a0 * sharp;
    drumDigit(ctx, d0, cx, y - fr * LH, lead);
    if (fr > 0.01) drumDigit(ctx, d0 + 1, cx, y + (1 - fr) * LH, lead);
  }
  const band = UP + DN;
  if (blur > 0.004) {
    // Only the unleaded lowest drum blurs, so the strip needs no blank zero.
    const img = strip(color, slot);
    const qm = ((q % 10) + 10) % 10;
    const src = (qm + 2.5) * LH - (UP - MID);
    ctx.globalAlpha = a0 * blur;
    ctx.drawImage(img, 0, src, img.width, band, cx - img.width / 2, y - UP, img.width, band);
  }
  // Rolling digits fall off toward the window's edges, like a drum's curve.
  const edge = clamp(v * 4) * roll;
  if (edge > 0.01) {
    ctx.globalAlpha = a0 * edge;
    ctx.drawImage(fadeSprite(), cx - slot / 2 - 3, y - UP, slot + 6, band);
  }
  ctx.globalAlpha = a0;
}

/**
 * Draws `text` (its final reading) rolled to `value`, with its left edge at
 * chart (x, y). `prev` is the value one shutter earlier (null when locked),
 * `settle` a small extra turn of the lowest drum, `pop` a lock accent.
 */
function drawOdometer(
  ctx: CanvasRenderingContext2D,
  P: Plane,
  text: string,
  value: number,
  prev: number | null,
  x: number,
  y: number,
  color: string,
  alpha: number,
  settle: number,
  pop: number,
): void {
  if (alpha <= 0 || !text) return;
  const nm = numMetrics(ctx);
  const pos = drumPositions(text, value);
  const before = prev === null ? pos : drumPositions(text, prev);
  const n = pos.length;
  const dot = text.indexOf(".");
  const intDigits = dot < 0 ? text.length : dot;
  // While the lowest drum ticks, the drums above read the same rounded
  // figure, so every frame shows one whole number rather than a carry.
  const tick = smoothstep(0.45, 0.75, Math.abs(pos[0] - before[0]));
  if (tick > 0) {
    const scale = 10 ** (dot < 0 ? 0 : text.length - dot - 1);
    const rounded = drumPositions(text, Math.round(value * scale) / scale);
    for (let k = 1; k < n; k++) pos[k] = lerp(pos[k], rounded[k], tick);
  }
  let width = 0;
  for (const ch of text) width += ch === "." ? nm.dot : nm.slot;
  ctx.save();
  P.at(ctx, x, y);
  ctx.globalAlpha *= clamp(alpha);
  if (pop > 0.001) {
    const s = 1 + 0.06 * pop;
    ctx.translate(0, -MID);
    ctx.scale(s, s);
    ctx.translate(0, MID);
  }
  const fill = pop > 0.01 ? mix(color, PALETTE.paper, 0.75 * pop) : color;
  ctx.font = NUM_FONT;
  ctx.textAlign = "center";
  ctx.textBaseline = "alphabetic";
  ctx.fillStyle = fill;
  // One clip band: digits roll out through its edges.
  ctx.save();
  ctx.beginPath();
  ctx.rect(-nm.slot - 4, -UP, width + nm.slot + 8, UP + DN);
  ctx.clip();
  let cx = 0;
  let col = n - 1;
  for (const ch of text) {
    if (ch === ".") {
      ctx.fillText(".", cx + nm.dot / 2, 0);
      cx += nm.dot;
      continue;
    }
    const lead = col === n - 1 && intDigits >= 2;
    const q = pos[col] + (col === 0 ? settle : 0);
    const v = Math.abs(pos[col] - before[col]);
    drawDrum(ctx, cx + nm.slot / 2, 0, q, v, fill, lead, nm.slot, col === 0);
    cx += nm.slot;
    col--;
  }
  ctx.restore();
  drawText(ctx, "s", cx + NUM_SIZE * 0.14, 0, {
    font: font(NUM_SIZE * 0.5, 500),
    fill: PALETTE.text3,
  });
  ctx.restore();
}

/** Where a readout starts: after the tip, pulled left while its tens are hidden. */
function readoutX(ctx: CanvasRenderingContext2D, text: string, value: number, tip: number): number {
  return tip + NUM_GAP - numMetrics(ctx).slot * (1 - leadVisible(text, value));
}

interface Box {
  x0: number;
  y0: number;
  x1: number;
  y1: number;
}

// Row labels, 56 px, hung over their bars from the zero line.

const LABEL_FONT = font(LABEL_SIZE, 600);
const labelY = (row: number): number => row - TH / 2 - LABEL_GAP;
/** Each label's row, its runs of text and colour, and how hard it kicks when its bar lands. */
const LABELS: readonly { row: number; parts: readonly (readonly [string, string])[]; amp: number }[] = [
  { row: ROW_CARGO, parts: [["Cargo, no cache", PALETTE.text2]], amp: 0.04 },
  {
    row: ROW_MBX,
    parts: [
      ["Cargo + ", PALETTE.text1],
      ["mbx", PALETTE.amber],
    ],
    amp: 0.07,
  },
];

/** A row label's padded box, which lines running past it break around. */
function labelBox(ctx: CanvasRenderingContext2D, L: (typeof LABELS)[number]): Box {
  const w = L.parts.reduce((s, [text]) => s + layout(ctx, text, LABEL_FONT).width, 0);
  const y = labelY(L.row);
  return { x0: AX0 - 10, x1: AX0 + w + 14, y0: y - 48, y1: y + 18 };
}

/**
 * Padded boxes knocked out of the grid lines: the readouts, which `open`
 * closes as they fade so the grid heals rather than pops, and the labels.
 */
function knockBoxes(ctx: CanvasRenderingContext2D, c: Chart, t: number, open = 1): Box[] {
  const out = LABELS.map((L) => labelBox(ctx, L));
  if (open <= 0) return out;
  const box = (text: string, x: number, row: number) => {
    const y = row + NUM_SIZE * 0.36;
    const mid = y + (DN - UP) / 2;
    const h = ((UP + DN) / 2 + 8) * open;
    out.push({ x0: x - 12, x1: x + readoutWidth(ctx, text) + 12, y0: mid - h, y1: mid + h });
  };
  if (t >= T.race) {
    const v = cargoValue(c, t);
    box(c.cargoText, readoutX(ctx, c.cargoText, v * Number(c.cargoText), cargoTip(c, t)), ROW_CARGO);
    box(c.mbxText, mbxTip(c, t) + NUM_GAP, ROW_MBX);
  }
  return out;
}

// Camera. The chart is a plane in the world (z = -CUBE/2) laid out so that
// the front camera shows it at exactly chart px. The mbx bar is a real box on
// that plane, so it can pop out toward the viewer as the handoff cube while
// the camera orbits the cube's center to WORLD_CAM.

const CENTER: V3 = [0, CUBE / 2, 0];
/** WORLD_CAM re-aimed at the cube's center: projects identically. */
const WORLD_C: Camera = {
  ...WORLD_CAM,
  cy: WORLD_CAM.cy - (CUBE / 2) * Math.cos(WORLD_CAM.pitch) * WORLD_CAM.scale,
  target: CENTER,
};
const S0 = TH / CUBE;
/** Chart px of the cube's front-view center: the square at the mbx tip, or the card's. */
function pivot(m: Model): { x: number; y: number } {
  return m.chart ? { x: mbxEnd(m.chart) - TH / 2, y: ROW_MBX } : CARD_PIVOT;
}

/** Slow push-in while the chart plays and holds, about the frame center. */
const push = (t: number): number => lerp(1, 1.045, inOutSine(progress(0.05, T.clear + 0.1, t)));

function frontCam(m: Model, t: number): Camera {
  const z = push(t);
  const p = pivot(m);
  return {
    cx: 960 + (p.x - 960) * z,
    cy: 540 + (p.y - 540) * z,
    scale: S0 * z,
    yaw: 0,
    pitch: 0,
    target: CENTER,
  };
}

// b11 to b12: the page falls back while the bar compacts on b11.25, the
// camera orbits in perspective while the square extrudes, then the lens
// flattens and dollies back to the world view. Everything starts on b11, the
// score's swipe, and the page keeps falling until the cube settles.
const ROT = [T.clear, T.end - 0.07] as const;
const MOVE = [T.clear, T.end - 0.1] as const;
const DOLLY = [T.hit + 0.02, T.end - 0.045] as const;
const EXTRUDE = [T.hit - 0.02, T.end - 0.075] as const;
/** The cube settles into the world view here (the score's soft seat). */
export const SETTLE = DOLLY[1];
// Weighted late, so the swing and the pull-back are still visibly braking
// through the last frames rather than parked a tenth of a second early.
const rotEase = cubicBezier(0.45, 0, 0.4, 1);
const dollyEase = cubicBezier(0.5, 0, 0.6, 1);
/** From here on the frame is exactly the handoff. */
const T_REST = T.end - 0.03;
/** Viewer distance, world units, at the orbit's widest lens. */
const PERSP = 7;

/** Perspective opening on b11: a kick on the first frame, full by +0.16 s. */
const lensIn = (t: number): number => outCubic(progress(T.clear, T.clear + 0.16, t));

function camAt(m: Model, t: number): Camera {
  const F = frontCam(m, t);
  if (t <= T.clear) return F;
  const r = rotEase(progress(ROT[0], ROT[1], t));
  const p = rotEase(progress(MOVE[0], MOVE[1], t));
  const d = dollyEase(progress(DOLLY[0], DOLLY[1], t));
  // Perspective blends through its inverse: in with the fall, flat again by the rest.
  const lens = lensIn(t) * (1 - swiftInOut(progress(T_REST - 0.2, T_REST - 0.02, t)));
  return {
    cx: lerp(F.cx, WORLD_C.cx, p),
    cy: lerp(F.cy, WORLD_C.cy, p) - Math.sin(Math.PI * p) * 30,
    scale: lerp(F.scale, WORLD_C.scale, d),
    yaw: lerp(0, WORLD_C.yaw, r),
    pitch: lerp(0, WORLD_C.pitch, r),
    persp: lens > 1e-4 ? PERSP / lens : 0,
    target: CENTER,
  };
}

/**
 * The page's camera: the cube's, but its lens never flattens. The page is
 * far behind the cube by then, and an orthographic lens would blow it back
 * up to full size just as it should be vanishing into the distance.
 */
function pageCam(cam: Camera, t: number): Camera {
  const lens = lensIn(t);
  return { ...cam, persp: lens > 1e-4 ? PERSP / lens : 0 };
}

/** Apparent size the page shrinks to as it falls away, under the full lens. */
const PAGE_FAR = 0.42;
/** A shove on the swipe, then a long fall that lands with the cube. */
const fallEase = cubicBezier(0.45, 0, 0.55, 1);
const fallAt = (t: number): number =>
  0.07 * outCubic(progress(T.clear, T.clear + 3 * SHUTTER, t)) + 0.93 * fallEase(progress(T.clear, SETTLE, t));

/**
 * How far the page has fallen back behind the cube, world units. Chosen so
 * its apparent size, not its depth, follows the fall's curve: depth alone
 * would spend its whole last half barely changing on screen.
 */
const recede = (t: number): number => PERSP * (1 / lerp(1, PAGE_FAR, fallAt(t)) - 1);

// The page clears as one move, in depth order: its words go first, on the
// swipe, then the bars and tiles, and the grid stays longest as the
// reference the orbit turns against, falling away under the cube until it
// lands.
function pageFade(t: number): Fade {
  // Fades that hold, then go: the page is still there, dimming with
  // distance, until the frame the cube lands.
  const out = (a: number, b2: number, k: number) => 1 - progress(a, b2, t) ** k;
  return {
    text: 1 - outCubic(progress(T.clear, T.clear + 0.14, t)),
    bar: out(T.clear + 0.04, T_REST - 0.02, 1.3),
    line: out(T.clear + 0.1, T_REST - 0.02, 2),
    fall: smoothstep(T.clear, T.clear + 0.15, t),
  };
}

/** Chart px to world: on the plane z = -CUBE/2, with the pivot at the cube. */
function enterChart(ctx: CanvasRenderingContext2D, view: View, m: Model): void {
  const p = pivot(m);
  const o: V3 = [-p.x / S0, CUBE / 2 + p.y / S0, -CUBE / 2];
  applyMatrix(ctx, view.planeMatrix(o, [1 / S0, 0, 0], [0, -1 / S0, 0]));
}

// A shaded cuboid: flat amber at `look` 0, the handoff cube's cardboard at 1.
// Same panel order, fills, and outline weight as drawBox.

const LW = OUTLINE_RATIO * CUBE;
const FACES: { n: V3; c: [number, number, number][] }[] = [
  { n: [0, 1, 0], c: [[-1, 1, -1], [1, 1, -1], [1, 1, 1], [-1, 1, 1]] },
  { n: [0, -1, 0], c: [[-1, -1, -1], [-1, -1, 1], [1, -1, 1], [1, -1, -1]] },
  { n: [0, 0, 1], c: [[-1, 1, 1], [1, 1, 1], [1, -1, 1], [-1, -1, 1]] },
  { n: [0, 0, -1], c: [[1, 1, -1], [-1, 1, -1], [-1, -1, -1], [1, -1, -1]] },
  { n: [1, 0, 0], c: [[1, 1, 1], [1, 1, -1], [1, -1, -1], [1, -1, 1]] },
  { n: [-1, 0, 0], c: [[-1, 1, -1], [-1, 1, 1], [-1, -1, 1], [-1, -1, -1]] },
];

function drawSlab(ctx: CanvasRenderingContext2D, view: View, lo: V3, hi: V3, look: number): void {
  const c: V3 = [(lo[0] + hi[0]) / 2, (lo[1] + hi[1]) / 2, (lo[2] + hi[2]) / 2];
  const h: V3 = [(hi[0] - lo[0]) / 2, (hi[1] - lo[1]) / 2, (hi[2] - lo[2]) / 2];
  const vis: { pts: Projected[]; depth: number; n: V3 }[] = [];
  for (const face of FACES) {
    const at: V3 = [c[0] + face.n[0] * h[0], c[1] + face.n[1] * h[1], c[2] + face.n[2] * h[2]];
    if (!view.facing(face.n, at)) continue;
    const pts = face.c.map(([x, y, z]) => view.project([c[0] + x * h[0], c[1] + y * h[1], c[2] + z * h[2]]));
    vis.push({ pts, depth: view.project(at).z, n: face.n });
  }
  vis.sort((p, q) => p.depth - q.depth);
  ctx.save();
  const a0 = ctx.globalAlpha;
  ctx.lineJoin = "round";
  ctx.strokeStyle = OUTLINE;
  for (const face of vis) {
    polygon(ctx, face.pts);
    if (look > 0) {
      ctx.fillStyle = cardboardFill(ctx, face.pts, tone(view, face.n));
      ctx.fill();
    }
    if (look < 1) {
      ctx.fillStyle = rgba(PALETTE.amber, 1 - look);
      ctx.fill();
    }
    if (look > 0) {
      ctx.globalAlpha = a0 * look;
      ctx.lineWidth = LW * view.cam.scale * face.pts[0].f;
      ctx.stroke();
      ctx.globalAlpha = a0;
    }
  }
  ctx.restore();
}

// The page's words: title, subtitle, row labels, and the card.

function drawTitle(ctx: CanvasRenderingContext2D, P: Plane, a: number): void {
  if (a <= 0) return;
  ctx.save();
  ctx.globalAlpha *= a;
  P.at(ctx, AX0, TITLE_Y);
  drawText(ctx, TITLE, 0, 0, { font: CAPTION.font, fill: PALETTE.paper });
  ctx.restore();
}

function drawSubtitle(ctx: CanvasRenderingContext2D, P: Plane, m: Model, a: number): void {
  if (a <= 0) return;
  const f = font(40, 500);
  m.sub.forEach((line, i) => {
    ctx.save();
    ctx.globalAlpha *= a;
    P.at(ctx, AX0, SUB_Y[i]);
    drawText(ctx, line, 0, 0, { font: f, fill: PALETTE.text3 });
    ctx.restore();
  });
}

function drawRowLabels(ctx: CanvasRenderingContext2D, P: Plane, t: number, a: number): void {
  if (a <= 0) return;
  for (const [i, L] of LABELS.entries()) {
    // Each label kicks when its bar lands: a small secondary beat.
    const kick = pulse(t, i === 0 ? CARGO_DONE : mbxLock(), 0.015, 0.09);
    ctx.save();
    ctx.globalAlpha *= a;
    P.at(ctx, AX0, labelY(L.row));
    ctx.scale(1 + L.amp * kick, 1 + L.amp * kick);
    let x = 0;
    for (const [text, fill] of L.parts) {
      x += drawText(ctx, text, x, 0, {
        font: LABEL_FONT,
        fill: kick > 0.05 ? mix(fill, PALETTE.paper, kick * 0.7) : fill,
      }).width;
    }
    ctx.restore();
  }
}

/** Words on the page with the reel's rise, at chart (x, y). */
function pageWords(
  ctx: CanvasRenderingContext2D,
  P: Plane,
  text: string,
  x: number,
  y: number,
  style: WordStyle,
  t: number,
  land: number,
  a: number,
): number {
  if (a <= 0) return 0;
  ctx.save();
  ctx.globalAlpha *= a;
  P.at(ctx, x, y);
  const w = drawWords(ctx, text, 0, 0, style, t, land);
  ctx.restore();
  return w;
}

const URL_STYLE: WordStyle = { ...CAPTION, fill: PALETTE.amber };

/**
 * The card that stands in for the chart: its first line rises in with the
 * race's lurches, the address lands on the lock, and a rule draws under it
 * with the delta's cue.
 */
function drawCard(ctx: CanvasRenderingContext2D, P: Plane, t: number, a: number): void {
  if (a <= 0) return;
  pageWords(ctx, P, CARD[0], AX0, CARD_Y[0], CAPTION, t, b(1), a);
  const w = pageWords(ctx, P, CARD[1], AX0, CARD_Y[1], URL_STYLE, t, T.lock, a);
  const u = swiftOut(progress(T.delta, T.delta + 0.3, t));
  if (u <= 0) return;
  ctx.save();
  ctx.globalAlpha *= a;
  ctx.fillStyle = rgba(PALETTE.amber, 0.7);
  quad(ctx, P, AX0, CARD_Y[1] + 26, w * u, 6);
  ctx.fill();
  ctx.restore();
}

/** A vertical grid line from y0 up to y1, broken around `knock` boxes. */
function gridLine(ctx: CanvasRenderingContext2D, P: Plane, x: number, y0: number, y1: number, knock: Box[]): void {
  let runs: [number, number][] = [[y1, y0]];
  for (const r of knock) {
    if (x < r.x0 || x > r.x1) continue;
    const next: [number, number][] = [];
    for (const [a, c] of runs) {
      if (r.y1 <= a || r.y0 >= c) {
        next.push([a, c]);
        continue;
      }
      if (r.y0 > a) next.push([a, r.y0]);
      if (r.y1 < c) next.push([r.y1, c]);
    }
    runs = next;
  }
  ctx.beginPath();
  for (const [a, c] of runs) seg(ctx, P, x, a, x, c);
  ctx.stroke();
}

/** The grid: a line per step rising on a stagger, the zero line brightest. */
function drawPlot(ctx: CanvasRenderingContext2D, P: Plane, c: Chart, t: number, knock: Box[], fade: Fade): void {
  if (fade.line <= 0) return;
  const ticks = Math.round(c.max / c.step);
  ctx.save();
  const a0 = ctx.globalAlpha;
  ctx.lineCap = "butt";
  for (let i = 0; i <= ticks; i++) {
    const x = AX0 + (i * c.step * AXLEN) / c.max;
    const grow = swiftOut(progress(0.1 + i * 0.035, 0.38 + i * 0.035, t));
    const top = lerp(GRID_BOT, GRID_TOP, grow);
    if (top >= GRID_BOT - 0.5) continue;
    ctx.globalAlpha = a0 * fade.line;
    ctx.strokeStyle = i === 0 ? rgba(PALETTE.text3, 0.8) : mix(PALETTE.divider, PALETTE.text3, 0.5 * fade.fall, 0.95);
    ctx.lineWidth = (i === 0 ? 2 : 1.5) * P.k(x, GRID_BOT);
    // The zero line is the spine: never broken.
    gridLine(ctx, P, x, GRID_BOT, top, i === 0 ? [] : knock);
  }
  ctx.restore();
}

// Bars, readouts, and the delta annotation, all in chart px.

let rampCache: Map<string, HTMLCanvasElement> | null = null;
/** Horizontal light ramp, clear at the left and brightest at the right edge. */
function ramp(color: string): HTMLCanvasElement {
  rampCache ??= new Map();
  let c = rampCache.get(color);
  if (!c) {
    c = makeCanvas(64, 4);
    const g = c.getContext("2d")!;
    const grad = g.createLinearGradient(0, 0, 64, 0);
    grad.addColorStop(0, rgba(color, 0));
    grad.addColorStop(0.7, rgba(color, 0.14));
    grad.addColorStop(1, rgba(color, 0.45));
    g.fillStyle = grad;
    g.fillRect(0, 0, 64, 4);
    rampCache.set(color, c);
  }
  return c;
}

/**
 * Light at a growing bar's leading edge: a hot edge line, light pooled in the
 * bar behind it, and a small bloom, all kept inside the bar's band.
 */
function tipLight(ctx: CanvasRenderingContext2D, P: Plane, x: number, row: number, a: number, color: string): void {
  if (a <= 0.01 || x - AX0 < 1) return;
  ctx.save();
  P.at(ctx, x, row);
  // Only inside the bar: a short bar clips the ramp at the zero line.
  const back = Math.max(AX0 - x, -160);
  ctx.beginPath();
  ctx.rect(back, -TH / 2, -back, TH);
  ctx.clip();
  ctx.globalCompositeOperation = "lighter";
  ctx.globalAlpha *= clamp(a);
  ctx.drawImage(ramp(color), -150, -TH / 2, 150, TH);
  glow(ctx, -6, 0, 58, color, 0.3);
  ctx.fillStyle = rgba(PALETTE.paper, 0.85);
  ctx.fillRect(-3.5, -TH / 2, 3.5, TH);
  ctx.restore();
}

/**
 * Landing flare at a bar's live tip: a hot edge line a little taller than
 * the bar and a bloom inside the bar, both on one short envelope `k`.
 */
function landFlare(ctx: CanvasRenderingContext2D, P: Plane, x: number, row: number, k: number, color: string): void {
  if (k <= 0.01) return;
  ctx.save();
  P.at(ctx, x, row);
  ctx.globalCompositeOperation = "lighter";
  ctx.save();
  ctx.beginPath();
  ctx.rect(Math.max(AX0 - x, -120), -TH / 2, Math.min(x - AX0, 120), TH);
  ctx.clip();
  glow(ctx, -4, 0, 64, color, 0.45 * k);
  ctx.restore();
  ctx.fillStyle = rgba(color, 0.9 * k);
  const hh = TH * (0.5 + 0.12 * (1 - k));
  ctx.fillRect(-1.5, -hh, 3, hh * 2);
  ctx.restore();
}

/** A flash that is full on the first frame at or after `at`, then decays. */
const flash = (t: number, at: number, decay: number): number =>
  t < at ? 0 : Math.exp(-Math.max(0, t - at - FRAME) / decay);

/**
 * Photo finish on mbx's lock: a hairline through both tips, flush at that
 * instant, from the top of the Cargo bar to the foot of the mbx bar.
 */
function photoFinish(ctx: CanvasRenderingContext2D, P: Plane, x: number, k: number): void {
  if (k <= 0.01) return;
  ctx.save();
  ctx.globalCompositeOperation = "lighter";
  const top = ROW_CARGO - TH / 2 - 10;
  const foot = ROW_MBX + TH / 2 + 10;
  // Full height on the lock, closing toward the gap between the bars as it fades.
  const open = 0.6 + 0.4 * k;
  const mid = (top + foot) / 2;
  ctx.strokeStyle = rgba(PALETTE.paper, 0.85 * k);
  ctx.lineWidth = 2 * P.k(x, mid);
  ctx.beginPath();
  seg(ctx, P, x, lerp(mid, top, open), x, lerp(mid, foot, open));
  ctx.stroke();
  ctx.restore();
}

/**
 * Cargo's value, 0..1, as its readout shows it: each lurch settles a little
 * sooner than the bar so the figure rests crisp between steps, and it locks
 * when the last crate lands.
 */
const cargoValue = (c: Chart, t: number): number => (t >= CARGO_DONE ? 1 : cargoGrow(c.readSteps, t, 1.5));
/** Chart x of the Cargo bar's tip, with a small recoil after the clunk. */
const cargoTip = (c: Chart, t: number): number =>
  AX0 + (cargoEnd(c) - AX0) * cargoGrow(c.steps, t) + 6 * wobble(t, CARGO_DONE, 6, 16);

function drawCargo(ctx: CanvasRenderingContext2D, P: Plane, c: Chart, t: number, fade: Fade): void {
  if (t < T.race) return;
  const tip = cargoTip(c, t);
  const len = tip - AX0;
  if (len > 0.5 && fade.bar > 0) {
    ctx.save();
    ctx.globalAlpha *= fade.bar;
    ctx.fillStyle = PALETTE.teal;
    quad(ctx, P, AX0, ROW_CARGO - TH / 2, len, TH);
    ctx.fill();
    ctx.restore();
  }
  if (t < T.clear) {
    // Leading-edge light while a crate is moving.
    const v = cargoTip(c, t) - cargoTip(c, t - SHUTTER);
    tipLight(ctx, P, tip, ROW_CARGO, clamp(v / 70) * 0.8, PALETTE.tealLight);
    // A short, muted flare when the last crate lands.
    landFlare(ctx, P, tip, ROW_CARGO, flash(t, CARGO_DONE, 0.05) * 0.6, PALETTE.tealLight);
  }
  // The readout rides the tip, at full strength from its first frame: a
  // half-faded figure reads as a ghost.
  const na = progress(T.race, T.race + 0.01, t) * fade.text;
  if (na <= 0) return;
  const target = Number(c.cargoText);
  const v = cargoValue(c, t);
  const locked = t >= CARGO_DONE;
  drawOdometer(
    ctx,
    P,
    c.cargoText,
    v * target,
    locked ? null : cargoValue(c, t - SHUTTER) * target,
    readoutX(ctx, c.cargoText, v * target, tip),
    ROW_CARGO + NUM_SIZE * 0.36,
    PALETTE.text1,
    na,
    0,
    pulse(t, CARGO_DONE, 0.005, 0.07) * 0.6,
  );
}

/** Chart x of the mbx bar's tip. */
const mbxTip = (c: Chart, t: number): number => AX0 + (mbxEnd(c) - AX0) * mbxGrow(t);

function drawMbxReadout(ctx: CanvasRenderingContext2D, P: Plane, c: Chart, t: number, fade: Fade): void {
  if (t < T.race) return;
  const lock = mbxLock();
  const target = Number(c.mbxText);
  const locked = t >= lock;
  const a = progress(T.race, T.race + 0.01, t) * fade.text;
  // Mechanical settle once locked: the lowest drum nudges past and back,
  // a tenth of a digit at most, never fast enough to blur.
  const settle = 0.2 * wobble(t, lock, 4, 9);
  drawOdometer(
    ctx,
    P,
    c.mbxText,
    locked ? target : clamp(mbxGrow(t)) * target,
    locked ? null : clamp(mbxGrow(t - SHUTTER)) * target,
    mbxTip(c, t) + NUM_GAP,
    ROW_MBX + NUM_SIZE * 0.36,
    PALETTE.amberBright,
    a,
    settle,
    pulse(t, lock, 0.005, 0.1),
  );
}

function drawMbxFx(ctx: CanvasRenderingContext2D, P: Plane, c: Chart, t: number): void {
  if (t < T.race || t >= T.clear) return;
  const tip = mbxTip(c, t);
  const v = tip - mbxTip(c, t - SHUTTER);
  tipLight(ctx, P, tip, ROW_MBX, clamp(v / 50), PALETTE.amberBright);
  // The lock: a flare at the live tip and the photo-finish line, gone
  // before mbx's overshoot or Cargo's next step can reach it.
  landFlare(ctx, P, tip, ROW_MBX, flash(t, mbxLock(), 0.05), PALETTE.amberBright);
  const pf = flash(t, mbxLock(), 0.03);
  if (c.mbx < c.cargo && pf > 0.2) photoFinish(ctx, P, mbxEnd(c), pf);
  // A light sweep across the bar as the hold begins. Cream laid over the
  // amber, never added, so no channel clips and the hue stays amber.
  const gp = progress(T.glint - 0.04, T.glint + 0.34, t);
  if (gp > 0 && gp < 1) {
    const x0 = AX0;
    const x1 = mbxEnd(c);
    const cx = lerp(x0 - 60, x1 + 160, inOutSine(gp));
    ctx.save();
    ctx.beginPath();
    ctx.rect(x0, ROW_MBX - TH / 2, x1 - x0, TH);
    ctx.clip();
    ctx.transform(1, 0, -0.5, 1, 0.5 * ROW_MBX, 0);
    const g = ctx.createLinearGradient(cx - 100, 0, cx + 100, 0);
    g.addColorStop(0, rgba(PALETTE.paper, 0));
    g.addColorStop(0.5, rgba(PALETTE.paper, 0.26));
    g.addColorStop(1, rgba(PALETTE.paper, 0));
    ctx.fillStyle = g;
    ctx.fillRect(cx - 100, ROW_MBX - TH / 2, 200, TH);
    ctx.restore();
  }
}

// The tile strip: one tile per compilation mbx looked up, in columns under
// its bar, filled column by column. The restored ones sweep in green from
// the left, dropping out from under the bar; the compiled ones, last in a
// build (the edited crate and whatever depends on it), land amber on b2 with
// the annotation's last word. Halfway through the hold a light runs along
// the strip and the compile flares again as it passes.

interface StripLayout {
  rows: number;
  cols: number;
  pitch: number;
  tile: number;
  /** The strip's right edge and bottom edge, chart px. */
  x1: number;
  y1: number;
}

/** As many rows as a pitch of about STRIP_PITCH needs to fit the strip under the bar. */
function stripLayout(c: Chart): StripLayout {
  const len = mbxEnd(c) - AX0;
  const rows = clamp(Math.ceil(c.lookups / Math.max(1, Math.floor(len / STRIP_PITCH))), 1, STRIP_ROWS);
  const cols = Math.ceil(c.lookups / rows);
  const pitch = clamp(len / cols, 4, 18);
  const tile = pitch - Math.max(1.5, pitch * 0.2);
  return { rows, cols, pitch, tile, x1: AX0 + cols * pitch, y1: STRIP_Y + rows * pitch };
}

/** When tile `i` lands: the green ones on the sweep, the amber ones on b2. */
function tileAt(c: Chart, L: StripLayout, i: number): number {
  const green = c.lookups - c.misses;
  if (i >= green) return T.amber + (i - green) * 0.006;
  const last = Math.max(1, Math.ceil(green / L.rows) - 1);
  return T.strip + (STRIP_SWEEP * Math.floor(i / L.rows)) / last + (i % L.rows) * 0.004;
}

/** How long a tile takes to drop into place. */
const DROP = 0.09;
/** The hold's light takes this long to run the strip. */
export const SHIMMER = b(1);
/** How bright the hold's light makes the tile in column `col`, 0..1. */
function shimmerAt(L: StripLayout, col: number, t: number): number {
  const d = (t - (T.shimmer + (SHIMMER * col) / L.cols)) / 0.08;
  return Math.abs(d) > 3 ? 0 : Math.exp(-d * d);
}

function drawStrip(ctx: CanvasRenderingContext2D, P: Plane, c: Chart, t: number, a: number): void {
  if (t < T.strip || a <= 0) return;
  const L = stripLayout(c);
  const green = c.lookups - c.misses;
  ctx.save();
  ctx.globalAlpha *= a;
  const a0 = ctx.globalAlpha;
  // Green tiles at rest share one path, so the hold fills the strip once.
  const rest = new Path2D();
  const add = (x: number, y: number, w: number) => {
    const q = [P.pt(x, y), P.pt(x + w, y), P.pt(x + w, y + w), P.pt(x, y + w)];
    rest.moveTo(q[0].x, q[0].y);
    for (let k = 1; k < 4; k++) rest.lineTo(q[k].x, q[k].y);
    rest.closePath();
  };
  for (let i = 0; i < c.lookups; i++) {
    const at = tileAt(c, L, i);
    if (t < at) continue;
    const u = progress(at, at + DROP, t);
    const amber = i >= green;
    const col = Math.floor(i / L.rows);
    const row = i % L.rows;
    const x = AX0 + col * L.pitch;
    const y = STRIP_Y + row * L.pitch;
    // Each tile lands hot and cools to its colour; the hold's light warms
    // it again as it passes.
    const hot = Math.max(1 - progress(at, at + 0.14, t), 0.8 * shimmerAt(L, col, t));
    if (!amber && u >= 1 && hot <= 0.02) {
      add(x, y, L.tile);
      continue;
    }
    const base = amber ? PALETTE.amber : PALETTE.green;
    ctx.globalAlpha = a0 * clamp(u / 0.35);
    ctx.fillStyle = hot > 0.02 ? mix(base, PALETTE.paper, 0.7 * hot) : base;
    if (amber) {
      // The compile pops in past its size and settles.
      const size = L.tile * spring(t - at, 5, 0.35);
      quad(ctx, P, x + (L.tile - size) / 2, y + (L.tile - size) / 2, size, size);
    } else {
      quad(ctx, P, x, y - (1 - swiftOut(u)) * L.pitch * 2.2, L.tile, L.tile);
    }
    ctx.fill();
  }
  ctx.globalAlpha = a0;
  ctx.fillStyle = PALETTE.green;
  ctx.fill(rest);
  ctx.restore();
  // The compiles glow amber: a burst as they land and again as the hold's
  // light passes, and an ember between.
  if (c.misses > 0 && t >= T.amber) {
    for (let i = green; i < c.lookups; i++) {
      const col = Math.floor(i / L.rows);
      const cx = AX0 + col * L.pitch + L.tile / 2;
      const cy = STRIP_Y + (i % L.rows) * L.pitch + L.tile / 2;
      const p = P.pt(cx, cy);
      const k = P.k(cx, cy);
      const burst = flash(t, tileAt(c, L, i), 0.12) + 0.7 * flash(t, T.shimmer + SHIMMER, 0.14);
      glow(ctx, p.x, p.y, (30 + 60 * burst) * k, PALETTE.amber, (a * (0.5 + 0.6 * burst)) / Math.sqrt(c.misses));
    }
  }
}

/**
 * The annotation beside the strip, lined up under mbx's readout: restored
 * in green, compiled in amber, its words rising one per 1/32 note to land on
 * b2 with the amber tile. It goes under the strip when there is no room.
 */
function drawNote(ctx: CanvasRenderingContext2D, P: Plane, c: Chart, t: number, a: number): void {
  if (a <= 0) return;
  const L = stripLayout(c);
  const [restored, compiled] = c.note;
  const first = compiled ? `${restored},` : restored;
  const space = layout(ctx, " ", LABEL.font).width;
  const w0 = layout(ctx, first, LABEL.font).width;
  const total = w0 + (compiled ? space + layout(ctx, compiled, LABEL.font).width : 0);
  const x0 = Math.max(L.x1, mbxEnd(c)) + NUM_GAP;
  const beside = x0 + total <= 1800;
  const x = beside ? x0 : AX0;
  const y = beside ? (STRIP_Y + L.y1) / 2 + 20 : L.y1 + 64;
  const green: WordStyle = { ...LABEL, fill: PALETTE.green };
  const amber: WordStyle = { ...LABEL, fill: PALETTE.amber };
  // The two runs share one line's timing: the compiled words land last, on b2.
  const tail = compiled ? compiled.split(" ").length : 0;
  pageWords(ctx, P, first, x, y, green, t, T.amber - (tail * BEAT) / 8, a);
  if (compiled) pageWords(ctx, P, compiled, x + w0 + space, y, amber, t, T.amber, a);
}

// The delta figure pops on b2.75: launched two frames early on a stiff
// spring so it is at full size on the beat, then overshoots.
const DELTA_POP = (t: number): number => spring(t - (T.label - 2 * SHUTTER), 5, 0.45, 40);

function drawDelta(ctx: CanvasRenderingContext2D, P: Plane, c: Chart, t: number, fade: Fade): void {
  if (!c.deltaText || t < T.delta) return;
  const xa = mbxEnd(c);
  const xb = cargoEnd(c);
  const green = PALETTE.green;
  ctx.save();
  ctx.lineCap = "round";

  // The saved stretch of the Cargo bar is hatched out, left to right.
  const hatch = outQuart(progress(T.delta + 0.05, T.delta + 0.3, t));
  if (hatch > 0 && fade.bar > 0) {
    const top = ROW_CARGO - TH / 2;
    const w = (xb - xa) * hatch;
    ctx.save();
    ctx.globalAlpha *= fade.bar;
    quad(ctx, P, xa, top, w, TH);
    ctx.clip();
    ctx.fillStyle = rgba(PALETTE.bg, 0.45);
    ctx.fill();
    ctx.strokeStyle = rgba(PALETTE.tealLight, 0.42);
    ctx.lineWidth = 3 * P.k(xa, ROW_CARGO);
    ctx.lineCap = "butt";
    const crawl = ((t - T.delta) * 30) % 20;
    ctx.beginPath();
    for (let x = xa - TH + crawl - 20; x < xa + w + 20; x += 20) seg(ctx, P, x, top + TH, x + TH, top);
    ctx.stroke();
    ctx.restore();
  }

  // Extension lines rise from each bar's end, then the dimension line spans
  // them. On b11 they draw back up into the dimension line and leave with
  // the words, so nothing points at the bar once it has gone. Where mbx's
  // bar is short enough for them to reach the row labels, they break around
  // them, as a drawing's dimension lines do.
  if (fade.text > 0) {
    ctx.save();
    const boxes = LABELS.map((L) => labelBox(ctx, L)).filter((lb) => xa < lb.x1);
    if (boxes.length) {
      ctx.beginPath();
      ctx.rect(-1e5, -1e5, 2e5, 2e5);
      for (const lb of boxes) {
        const q = [P.pt(lb.x0, lb.y0), P.pt(lb.x0, lb.y1), P.pt(lb.x1, lb.y1), P.pt(lb.x1, lb.y0)];
        ctx.moveTo(q[0].x, q[0].y);
        for (const p of q.slice(1)) ctx.lineTo(p.x, p.y);
        ctx.closePath();
      }
      ctx.clip("evenodd");
    }
    ctx.globalAlpha *= fade.text;
    ctx.strokeStyle = green;
    const k = P.k(xa, BRK_Y);
    ctx.lineWidth = 3 * k;
    const back = swiftInOut(progress(T.clear - 0.03, T.clear + 0.1, t));
    const ext = (x: number, from: number, e: number) => {
      if (e <= 0 || back >= 1) return;
      const foot = lerp(from, BRK_Y - 14, back);
      ctx.save();
      ctx.setLineDash([8 * k, 8 * k]);
      ctx.beginPath();
      seg(ctx, P, x, foot, x, lerp(from, BRK_Y - 14, e));
      ctx.stroke();
      ctx.restore();
    };
    // The mbx line stands on the bar's top, which dips as the bar crouches.
    const mbxTop = ROW_MBX - TH / 2 + TH * CROUCH * crouchAt(t);
    ext(xa, mbxTop - 8, swiftOut(progress(T.delta, T.delta + 0.16, t)));
    ext(xb, ROW_CARGO - TH / 2 - 8, swiftOut(progress(T.delta + 0.04, T.delta + 0.17, t)));
    const span = swiftInOut(progress(T.delta + 0.04, T.delta + 0.22, t));
    if (span > 0) {
      ctx.beginPath();
      seg(ctx, P, xa, BRK_Y, lerp(xa, xb, span), BRK_Y);
      seg(ctx, P, xa, BRK_Y - 14, xa, BRK_Y + 14);
      if (span > 0.98) seg(ctx, P, xb, BRK_Y - 14, xb, BRK_Y + 14);
      ctx.stroke();
    }
    ctx.restore();
  }

  // The figure, with a drawn minus (the font's is a hyphen) and a small unit.
  const pop = DELTA_POP(t);
  if (pop > 0 && fade.text > 0) {
    const s = lerp(0.7, 1, pop);
    const f = font(DELTA_SIZE, 600);
    const track = -0.04 * DELTA_SIZE;
    const w = layout(ctx, c.deltaText, f, track).width;
    const uf = font(DELTA_SIZE * 0.5, 500);
    const uw = layout(ctx, "s", uf).width;
    const mw = DELTA_SIZE * 0.4;
    const gap = DELTA_SIZE * 0.1;
    const ugap = DELTA_SIZE * 0.12;
    const total = mw + gap + w + ugap + uw;
    ctx.save();
    ctx.globalAlpha *= clamp(pop * 3) * fade.text;
    P.at(ctx, (xa + xb) / 2, BRK_Y - 26);
    ctx.scale(s, s);
    const x0 = -total / 2;
    ctx.fillStyle = green;
    ctx.fillRect(x0, -DELTA_SIZE * 0.34, mw, DELTA_SIZE * 0.075);
    drawText(ctx, c.deltaText, x0 + mw + gap, 0, { font: f, tracking: track, fill: green });
    drawText(ctx, "s", x0 + mw + gap + w + ugap, 0, { font: uf, fill: rgba(green, 0.8) });
    ctx.restore();
  }
  ctx.restore();
}

/** 0..1 anticipation before the collapse: the mbx bar crouches into the floor. */
const crouchAt = keys([
  [T.clear - 0.11, 0],
  [T.clear, 1, swiftOut],
  // The release snaps on b11, with the swipe.
  [T.hit - 0.02, 0, outCubic],
]);
const CROUCH = 0.11;

/**
 * The mbx bar, from chart bar through square to cube, in world space. With
 * the card there is no bar: its square pops out of the page on b11.
 */
function drawMbxBox(ctx: CanvasRenderingContext2D, view: View, m: Model, t: number): void {
  const c = m.chart;
  if (t < (c ? T.race : T.clear)) return;
  const p = pivot(m);
  const wx = (sx: number) => (sx - p.x) / S0;
  // Collapse: the zero end rushes to the tip and the bar compacts to a square.
  const tip = c ? mbxTip(c, t) : p.x + TH / 2;
  const left = c ? collapseLeft(c, t) : p.x - TH / 2;
  const grow = c ? 1 : spring(t - T.clear, 5, 0.5);
  // Anticipation: the bar crouches into the floor before it compacts, then
  // an impact squash where the square forms, recovering through the swing.
  const crouch = c ? crouchAt(t) : 0;
  const sq = 0.14 * wobble(t, T.hit, 4, 9) * (1 - progress(EXTRUDE[1] - 0.1, EXTRUDE[1], t));
  const cx = wx((tip + left) / 2);
  const hw = ((tip - left) / S0 / 2) * (1 - sq) * grow;
  const hgt = CUBE * (1 + sq * 0.9) * (1 - CROUCH * crouch) * grow;
  // The square pops out of the chart's plane toward the viewer.
  const depth =
    CUBE *
    keys([
      [EXTRUDE[0], 0],
      [EXTRUDE[1] - 0.09, 1.08, outCubic],
      [EXTRUDE[1], 1, inOutSine],
    ])(t);
  const look = swiftInOut(progress(T.clear + 0.02, T.hit + 0.04, t));
  // The contact shadow opens as the camera rises over the floor.
  const sh = progress(T.hit, DOLLY[1], t);
  if (sh > 0) drawShadow(ctx, view, [cx, 0, 0], CUBE, 0, 0.45 * sh);
  if (c) collapseTrail(ctx, view, c, t, wx(left), hgt);
  const y0 = (CUBE - hgt) / 2 * (c ? 0 : 1);
  drawSlab(ctx, view, [cx - hw, y0, -CUBE / 2], [cx + hw, y0 + hgt, -CUBE / 2 + depth], look);
}

/**
 * Where the collapsing bar's zero end is, chart px: a latch-release jolt on
 * b11 (visible on its first frame), then the rush into b11.25.
 */
function collapseLeft(c: Chart, t: number): number {
  const k =
    0.07 * outCubic(progress(T.clear, T.clear + 2.5 * SHUTTER, t)) + 0.93 * inCubic(progress(T.clear, T.hit, t));
  return lerp(AX0, mbxTip(c, t) - TH, k);
}

/**
 * The zero end covers most of its travel in the frames before b11.25, so it
 * drags a smear of the ground it just crossed, and on impact that smear
 * breaks into three speed ticks that are reeled in behind the square.
 */
function collapseTrail(ctx: CanvasRenderingContext2D, view: View, c: Chart, t: number, left: number, hgt: number): void {
  if (t < T.clear || t > T.hit + 0.12) return;
  const z = -CUBE / 2;
  const p = mbxEnd(c) - TH / 2;
  const wx = (sx: number) => (sx - p) / S0;
  ctx.save();
  if (t < T.hit) {
    // About a frame and a half of travel, fading back toward where the end
    // was; only once the end is really moving, or it reads as a shadow.
    const from = wx(collapseLeft(c, t - SHUTTER * 1.5));
    const run = left - from;
    if (run > 16 / S0) {
      const a = view.project([from, hgt / 2, z]);
      const e = view.project([left, hgt / 2, z]);
      const g = ctx.createLinearGradient(a.x, a.y, e.x, e.y);
      const k = clamp((run * S0 - 16) / 60);
      g.addColorStop(0, rgba(PALETTE.amber, 0));
      g.addColorStop(0.7, rgba(PALETTE.amber, 0.22 * k));
      g.addColorStop(1, rgba(PALETTE.amber, 0.6 * k));
      ctx.fillStyle = g;
      polygon(ctx, [
        view.project([from, hgt * 0.04, z]),
        view.project([left, 0, z]),
        view.project([left, hgt, z]),
        view.project([from, hgt * 0.96, z]),
      ]);
      ctx.fill();
    }
  } else {
    const u = progress(T.hit, T.hit + 0.12, t);
    const reach = (TH * 1.3 * (1 - outCubic(u))) / S0;
    ctx.lineCap = "round";
    ctx.strokeStyle = rgba(PALETTE.amberBright, 0.8 * (1 - u));
    ctx.lineWidth = (3 * view.cam.scale * view.project([left, 0, z]).f) / S0;
    ctx.beginPath();
    for (const [h, r] of [
      [0.22, 0.7],
      [0.5, 1],
      [0.78, 0.55],
    ] as const) {
      const gap = 10 / S0;
      const a = view.project([left - gap, hgt * h, z]);
      const e = view.project([left - gap - reach * r, hgt * h, z]);
      ctx.moveTo(a.x, a.y);
      ctx.lineTo(e.x, e.y);
    }
    ctx.stroke();
  }
  ctx.restore();
}

/** Everything on the page, under plane `P`, in back-to-front order. */
function drawPage(ctx: CanvasRenderingContext2D, P: Plane, m: Model, t: number, fade: Fade, knock: Box[]): void {
  const c = m.chart;
  if (c) {
    drawPlot(ctx, P, c, t, knock, fade);
    drawRowLabels(ctx, P, t, fade.text);
  }
  drawTitle(ctx, P, fade.text);
  drawSubtitle(ctx, P, m, fade.text);
  if (!c) {
    drawCard(ctx, P, t, fade.text);
    return;
  }
  drawCargo(ctx, P, c, t, fade);
  drawDelta(ctx, P, c, t, fade);
  // The strip leaves soon after the words: a green slab falling away under
  // the cube would read as its footing.
  drawStrip(ctx, P, c, t, Math.min(fade.bar, 1 - outCubic(progress(T.clear, T.clear + 0.22, t))));
  drawNote(ctx, P, c, t, fade.text);
  drawMbxReadout(ctx, P, c, t, fade);
}

export const scene: Scene = {
  id: S.id,
  start: S.start,
  end: S.end,
  draw(ctx, lt, env) {
    const t = lt;
    if (t >= T_REST) {
      drawHandoff(ctx, "next-push|pruned", env);
      return;
    }
    ctx.fillStyle = PALETTE.bg;
    ctx.fillRect(0, 0, env.W, env.H);
    const m = model(facts(env));
    const c = m.chart;
    const cam = camAt(m, t);
    const view = new View(cam);

    if (t >= T.clear) {
      // The page falls back behind the cube, every element projected.
      const P = projPlane(new View(pageCam(cam, t)), m, -CUBE / 2 - recede(t));
      const fade = pageFade(t);
      drawPage(ctx, P, m, t, fade, c ? knockBoxes(ctx, c, t, fade.text) : []);
      drawMbxBox(ctx, view, m, t);
      return;
    }

    // Behind the box: the chart plane, its layers whipping in on a stagger.
    ctx.save();
    enterChart(ctx, view, m);
    const plotOff = L_PLOT.x(t);
    whipLayer(ctx, L_TITLE, t, (g) => drawTitle(g, FLAT, 1));
    whipLayer(ctx, L_SUB, t, (g) => drawSubtitle(g, FLAT, m, 1));
    if (c) {
      const knock = knockBoxes(ctx, c, t);
      whipLayer(ctx, L_PLOT, t, (g) => drawPlot(g, FLAT, c, t, knock, OPAQUE));
      whipLayer(ctx, L_LABELS, t, (g) => drawRowLabels(g, FLAT, t, 1));
      // The bars, their readouts, and the strip ride the plot's layer.
      ctx.translate(plotOff, 0);
      drawCargo(ctx, FLAT, c, t, OPAQUE);
      drawDelta(ctx, FLAT, c, t, OPAQUE);
      drawStrip(ctx, FLAT, c, t, 1);
      drawNote(ctx, FLAT, c, t, 1);
    } else {
      drawCard(ctx, FLAT, t, 1);
    }
    ctx.restore();

    // The mbx bar rides the plot's layer too, still braking as the race
    // starts, so its zero end stays on the spine.
    drawMbxBox(ctx, new View({ ...cam, cx: cam.cx + plotOff * push(t) }), m, t);

    // Light and the readout ride on top of the bar.
    if (c) {
      ctx.save();
      enterChart(ctx, view, m);
      ctx.translate(plotOff, 0);
      drawMbxFx(ctx, FLAT, c, t);
      drawMbxReadout(ctx, FLAT, c, t, OPAQUE);
      ctx.restore();
    }

    // The whip's speed lines, over everything, until they die out.
    if (t < WHIP) drawWhip(ctx, env.t);
  },
};
