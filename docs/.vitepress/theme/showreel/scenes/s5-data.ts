// Scene 5, "Data": the published next-commit benchmark as a bar chart. The
// chart whips in from the right in layers, cargo lurches forward crate by
// crate while mbx springs to its mark beside it, and a dimension line
// measures the time saved. Then the page falls away in perspective while the
// camera swings around the mbx bar, which compacts into a square and pops out
// of the page as the cube that opens the isometric world.

import {
  bar,
  BEAT,
  CUBE,
  drawStagedBox,
  H5_POSE,
  PALETTE,
  type ReelFacts,
  type Scene,
  WHIP,
  WORLD_CAM,
} from "../bible";
import { drawShadow, OUTLINE, OUTLINE_RATIO } from "../box";
import { mix, rgba } from "../color";
import { glow, makeCanvas, smear } from "../fx";
import {
  clamp,
  cubicBezier,
  hash,
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
import { drawText, font, layout, MONO } from "../type";

/** Local time of global beat `n` (this scene starts on beat 16). */
const b = (n: number): number => (n - 16) * BEAT;
const FRAME = 1 / 60;

const T = {
  cargo: b(16.5),
  mbx: b(17),
  cargoLand: b(17.75),
  delta: b(18),
  label: b(18.25),
  glint: b(18.5),
  clear: b(19),
  hit: b(19.25),
  end: b(20),
};

// Layout in chart px, which equal screen px while the camera faces the chart.
// The zero line is the spine: the title aligns to it and the row labels hang
// to its left.
const AX0 = 380;
const AXLEN = 1120;
const TH = 96;
const ROW_CARGO = 518;
const ROW_MBX = 668;
const AXIS_Y = 768;
const GRID_TOP = ROW_CARGO - TH / 2 - 24;
const TITLE_Y = 280;
const SUB_Y = 322;
const BRK_Y = 424;
const NUM_SIZE = 68;
const NUM_GAP = 26;
const LABEL_SIZE = 34;
const TICK_SIZE = 24;
const DELTA_SIZE = 84;

// Whip-in: content starts fully off the right edge.
const WHIP_D = 1800;

interface Model {
  cargo: number;
  mbx: number;
  max: number;
  step: number;
  /** Real numbers available: readouts, tick labels, and the delta. */
  numbers: boolean;
  subject: string | null;
  cargoText: string;
  mbxText: string;
  deltaText: string;
  /** Where each of cargo's steps brings its bar to rest, as fractions of its value. */
  steps: number[];
  /** The same rests for the readout, as fractions of the figure it shows. */
  readSteps: number[];
}

function niceStep(raw: number): number {
  const p = 10 ** Math.floor(Math.log10(raw));
  for (const m of [1, 2, 2.5, 5, 10]) if (m * p >= raw) return m * p;
  return 10 * p;
}

/** A step that rests exactly on mbx's mark. */
const MEET = -1;

// Cargo lurches forward crate by crate on the sixteenths. Its third step
// comes to rest on mbx's mark just before mbx locks there on b17.25, so at
// the lock both tips are flush: mbx is done, cargo is halfway. Then a bigger
// step and a last grind that clunks home on b17.75. [start, end, fraction].
const STEP_PLAN: readonly (readonly [number, number, number])[] = [
  [b(16.5), b(16.5) + 6 * FRAME, 0.13],
  [b(16.75), b(16.75) + 6 * FRAME, 0.28],
  [b(17), b(17) + 6 * FRAME, MEET],
  [b(17.25) + 2 * FRAME, b(17.25) + 8 * FRAME, 0.725],
  // Ends half a frame early so the frame nearest the beat shows it home.
  [b(17.25) + 8 * FRAME, T.cargoLand - FRAME / 2, 1],
];
const CARGO_DONE = T.cargoLand - FRAME / 2;
/** The last step starts slow and arrives at speed, so it stops with a clunk. */
const grind = cubicBezier(0.4, 0, 0.8, 0.8);

let modelCache: { facts: ReelFacts | null; m: Model } | null = null;
function model(facts: ReelFacts | null): Model {
  if (modelCache && modelCache.facts === facts) return modelCache.m;
  const c = facts?.commit ?? null;
  let m: Model;
  if (c && c.cargo > 0 && c.mbx > 0) {
    const hi = Math.max(c.cargo, c.mbx);
    const step = niceStep(hi / 4);
    const max = Math.ceil((hi * 1.02) / step) * step;
    const cargoText = c.cargo.toFixed(1);
    const mbxText = c.mbx.toFixed(1);
    // The saving is the difference of the figures on screen, so it adds up
    // for anyone who subtracts them (it is within rounding of the raw one).
    const tenths = Math.round(Number(cargoText) * 10) - Math.round(Number(mbxText) * 10);
    const shown = Number(cargoText);
    // Only meet mbx on the way when it is the shorter bar.
    const meet = c.mbx < c.cargo;
    // Readout rests sit on whole tenths so the figure is crisp between steps.
    const readSteps = STEP_PLAN.map(([, , f0]) => {
      if (f0 === MEET && meet) return Number(mbxText) / shown;
      const f = f0 === MEET ? 0.455 : f0;
      return f >= 1 ? 1 : Math.max(0.1, Math.round(f * shown * 10) / 10) / shown;
    });
    m = {
      cargo: c.cargo,
      mbx: c.mbx,
      max,
      step,
      numbers: true,
      subject: facts?.subject || null,
      cargoText,
      mbxText,
      // The time-saved annotation only makes sense when mbx is faster.
      deltaText: tenths > 0 ? (tenths / 10).toFixed(1) : "",
      // The bar meets mbx's tip exactly; its readout meets mbx's figure.
      steps: STEP_PLAN.map(([, , f], i) => (f === MEET && meet ? c.mbx / c.cargo : readSteps[i])),
      readSteps,
    };
  } else {
    // No published numbers: the same moves, unlabeled, and no ratio either.
    // A longer cargo bar would be an unsourced speedup claim, so both bars
    // stop at one length (mbx's usual mark, which keeps the camera path).
    m = {
      cargo: 0.46,
      mbx: 0.46,
      max: 1,
      step: 0.25,
      numbers: false,
      subject: facts?.subject || null,
      cargoText: "",
      mbxText: "",
      deltaText: "",
      steps: STEP_PLAN.map(([, , f]) => (f === MEET ? 0.455 : f)),
      readSteps: [],
    };
  }
  modelCache = { facts, m };
  return m;
}

const pxPer = (m: Model): number => AXLEN / m.max;
const cargoEnd = (m: Model): number => AX0 + m.cargo * pxPer(m);
const mbxEnd = (m: Model): number => AX0 + m.mbx * pxPer(m);

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

/**
 * mbx launches with speed, reaches its mark on the frame nearest b17.25 (a
 * hair early, so that frame shows the tips flush), and overshoots by about 7%
 * before settling. The launch is tuned so the figure counts up evenly, one or
 * two units a frame: 1.3, 3.0, 4.8, 6.4, 7.7, 8.6, then 9.2 on the beat.
 */
const mbxGrow = (t: number): number => spring(t - T.mbx, 3.7, 0.66, 7.75);
let mbxCross = -1;
/** When the mbx bar first reaches its value. */
function mbxCrossing(): number {
  if (mbxCross >= 0) return mbxCross;
  let lo = T.mbx;
  let hi = T.mbx + 0.5;
  for (let t = T.mbx; t < T.mbx + 0.5; t += 1 / 240) {
    if (mbxGrow(t) >= 1) {
      hi = t;
      break;
    }
    lo = t;
  }
  for (let i = 0; i < 30; i++) {
    const mid = (lo + hi) / 2;
    if (mbxGrow(mid) >= 1) hi = mid;
    else lo = mid;
  }
  mbxCross = hi;
  return hi;
}
/** The readout locks half a frame before the bar's crossing, so the lock frame is crisp. */
const mbxLock = (): number => mbxCrossing() - FRAME / 2;

// Whip layers. The title leads; the plot and then the row labels trail it
// with more overshoot and a longer, dragging settle.

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
  y0: TITLE_Y - 40,
  y1: TITLE_Y + 14,
  x: whipTrack(0, 10, 0.16, 0),
  buf: null,
  maxGain: 2.5,
};
const L_SUB: Layer = {
  y0: SUB_Y - 30,
  y1: SUB_Y + 12,
  x: whipTrack(FRAME, 14, 0.18, 0),
  buf: null,
  maxGain: 2.5,
};
const L_PLOT: Layer = {
  y0: GRID_TOP - 8,
  y1: AXIS_Y + 60,
  x: whipTrack(2 * FRAME, 20, 0.2, 0.15),
  buf: null,
  maxGain: 1.5,
};
const L_LABELS: Layer = {
  y0: ROW_CARGO - 44,
  y1: ROW_MBX + 40,
  x: whipTrack(3 * FRAME, 28, 0.26, 0.2),
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
  const vel = off - L.x(t - FRAME);
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

// Speed lines: scene 4 leaves on a dense field of streaks, so the arrival
// picks the same vocabulary up at full strength on its first drawn frame and
// lets it die away as the layers brake. Each line is a light streak with a
// bright head on the left and a tail that shortens as the whip slows.
interface Streak {
  y: number;
  x: number;
  len: number;
  w: number;
  par: number;
  color: string;
  a: number;
}
let streaks: Streak[] | null = null;
function streakField(): Streak[] {
  if (streaks) return streaks;
  const colors = [PALETTE.paper, PALETTE.amber, PALETTE.green, PALETTE.tealLight, PALETTE.amberBright];
  streaks = [];
  for (let i = 0; i < 32; i++) {
    const fat = hash(i, 97) < 0.22;
    streaks.push({
      y: 150 + hash(i, 71) * 780,
      x: -400 + hash(i, 79) * 2400,
      len: (fat ? 500 : 260) + hash(i, 73) * 700,
      w: fat ? 10 + hash(i, 89) * 14 : 1.5 + hash(i, 89) * 3,
      par: 0.7 + hash(i, 83) * 0.9,
      color: colors[Math.floor(hash(i, 101) * colors.length)],
      a: fat ? 0.28 + hash(i, 103) * 0.2 : 0.5 + hash(i, 103) * 0.25,
    });
  }
  return streaks;
}

let streakSprites: Map<string, HTMLCanvasElement> | null = null;
/** A streak of `color`: hot at the left end, trailing off to the right, soft top and bottom. */
function streakSprite(color: string): HTMLCanvasElement {
  streakSprites ??= new Map();
  let c = streakSprites.get(color);
  if (!c) {
    c = makeCanvas(128, 16);
    const g = c.getContext("2d")!;
    const gx = g.createLinearGradient(0, 0, 128, 0);
    gx.addColorStop(0, rgba(color, 0));
    gx.addColorStop(0.04, rgba(color, 1));
    gx.addColorStop(0.3, rgba(color, 0.55));
    gx.addColorStop(1, rgba(color, 0));
    g.fillStyle = gx;
    g.fillRect(0, 0, 128, 16);
    // Round the profile: fade the top and bottom rows.
    g.globalCompositeOperation = "destination-in";
    const gy = g.createLinearGradient(0, 0, 0, 16);
    gy.addColorStop(0, "rgba(0,0,0,0)");
    gy.addColorStop(0.35, "rgba(0,0,0,1)");
    gy.addColorStop(0.65, "rgba(0,0,0,1)");
    gy.addColorStop(1, "rgba(0,0,0,0)");
    g.fillStyle = gy;
    g.fillRect(0, 0, 128, 16);
    streakSprites.set(color, c);
  }
  return c;
}

/** [y, height, color, alpha, head offset]: soft bands at the chart's rows. */
const BANDS: readonly (readonly [number, number, string, number, number])[] = [
  [TITLE_Y - 10, 34, PALETTE.paper, 0.2, 0],
  [ROW_CARGO, TH * 1.2, PALETTE.teal, 0.42, 90],
  [ROW_MBX, TH * 1.2, PALETTE.amber, 0.42, 140],
  [AXIS_Y, 22, PALETTE.paper, 0.16, 40],
  [880, 60, PALETTE.green, 0.14, 300],
  [190, 70, PALETTE.amberDeep, 0.14, 420],
];

/** The whip's travel still to come, 1 at the cut and 0 as the title lands. */
const whipLeft = (t: number): number => 1 - outQuart(progress(0, WHIP, t));

function speedLines(ctx: CanvasRenderingContext2D, t: number): void {
  // Frame 0 is the handoff (empty); full strength from the first drawn frame.
  const env = t < FRAME * 0.5 ? 0 : 1 - smoothstep(2 * FRAME, WHIP * 0.75, t);
  if (env <= 0) return;
  const left = whipLeft(t);
  ctx.save();
  ctx.globalCompositeOperation = "lighter";
  // Broad bands at the chart's own rows: the bars' colors race through
  // where the bars will stand, and give the whip the mass of scene 4's exit.
  for (const [y, h, color, a, lead] of BANDS) {
    const x = AX0 + lead + 700 * left;
    const len = 380 + 1800 * left;
    if (x > 1920) continue;
    ctx.globalAlpha = a * env;
    ctx.drawImage(streakSprite(color), x, y - h / 2, len, h);
  }
  for (const s of streakField()) {
    // Heads sweep left and brake with the layers; tails shrink with speed.
    const x = s.x + 800 * s.par * left - 160 * s.par;
    const len = s.len * (0.25 + 0.75 * left) * s.par;
    if (x > 1920 || x + len < 0) continue;
    ctx.globalAlpha = s.a * env;
    ctx.drawImage(streakSprite(s.color), x, s.y - s.w / 2, len, s.w);
  }
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
  const frame = (x: number, y: number) =>
    view.planeMatrix(w(x, y), [1 / S0, 0, 0], [0, -1 / S0, 0]);
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
 * digits per frame. `low` marks the lowest drum, the only one that spins.
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
 * chart (x, y). `prev` is the value one frame earlier (null when locked),
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

interface Rect {
  x0: number;
  y0: number;
  x1: number;
  y1: number;
}

/**
 * Padded boxes around the readouts, knocked out of the grid lines. `open`
 * closes the gaps as the readouts fade, so the grid heals rather than pops.
 */
function readoutBoxes(ctx: CanvasRenderingContext2D, m: Model, t: number, open = 1): Rect[] {
  if (!m.numbers || open <= 0) return [];
  const out: Rect[] = [];
  const box = (text: string, x: number, row: number) => {
    const y = row + NUM_SIZE * 0.36;
    const c = y + (DN - UP) / 2;
    const h = ((UP + DN) / 2 + 8) * open;
    out.push({ x0: x - 12, x1: x + readoutWidth(ctx, text) + 12, y0: c - h, y1: c + h });
  };
  if (t >= T.cargo) {
    const v = cargoValue(m, t);
    box(m.cargoText, readoutX(ctx, m.cargoText, v * Number(m.cargoText), cargoTip(m, t)), ROW_CARGO);
  }
  if (t >= T.mbx) box(m.mbxText, mbxTip(m, t) + NUM_GAP, ROW_MBX);
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
/** Chart px of the cube's front-view center: the square at the mbx tip. */
function pivot(m: Model): { x: number; y: number } {
  return { x: mbxEnd(m) - TH / 2, y: ROW_MBX };
}

/** Slow push-in while the chart plays, about the frame center. */
const push = (t: number): number => lerp(1, 1.04, inOutSine(progress(0.05, T.clear + 0.1, t)));

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

// b19 to b20: the page falls back while the bar compacts on b19.25, the
// camera orbits in perspective while the square extrudes, then the lens
// flattens and dollies back to the world view. Everything starts on b19, the
// score's swipe, and the page keeps falling until the cube settles.
const ROT = [T.clear, T.end - 0.07] as const;
const MOVE = [T.clear, T.end - 0.1] as const;
const DOLLY = [T.hit + 0.02, T.end - 0.045] as const;
const EXTRUDE = [T.hit - 0.02, T.end - 0.075] as const;
/** The cube settles into the world view here (the score's soft seat). */
const SETTLE = DOLLY[1];
// Weighted late, so the swing and the pull-back are still visibly braking
// through the last frames rather than parked a tenth of a second early.
const rotEase = cubicBezier(0.45, 0, 0.4, 1);
const dollyEase = cubicBezier(0.5, 0, 0.6, 1);
/** From here on the frame is exactly the handoff. */
const T_REST = T.end - 0.03;
/** Viewer distance, world units, at the orbit's widest lens. */
const PERSP = 7;

/** Perspective opening on b19: a kick on the first frame, full by +0.16 s. */
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
  0.07 * outCubic(progress(T.clear, T.clear + 3 * FRAME, t)) +
  0.93 * fallEase(progress(T.clear, SETTLE, t));

/**
 * How far the page has fallen back behind the cube, world units. Chosen so
 * its apparent size, not its depth, follows the fall's curve: depth alone
 * would spend its whole last half barely changing on screen.
 */
const recede = (t: number): number => PERSP * (1 / lerp(1, PAGE_FAR, fallAt(t)) - 1);

// The page clears as one move, in depth order: its words go first, on the
// swipe, then the bars, and the grid stays longest as the reference the orbit
// turns against, falling away under the cube until it lands.
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

function drawSlab(
  ctx: CanvasRenderingContext2D,
  view: View,
  lo: V3,
  hi: V3,
  look: number,
): void {
  const c: V3 = [(lo[0] + hi[0]) / 2, (lo[1] + hi[1]) / 2, (lo[2] + hi[2]) / 2];
  const h: V3 = [(hi[0] - lo[0]) / 2, (hi[1] - lo[1]) / 2, (hi[2] - lo[2]) / 2];
  const vis: { pts: Projected[]; depth: number; n: V3 }[] = [];
  for (const face of FACES) {
    const at: V3 = [c[0] + face.n[0] * h[0], c[1] + face.n[1] * h[1], c[2] + face.n[2] * h[2]];
    if (!view.facing(face.n, at)) continue;
    const pts = face.c.map(([x, y, z]) =>
      view.project([c[0] + x * h[0], c[1] + y * h[1], c[2] + z * h[2]]),
    );
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

// Chart chrome: title, caption, row labels, grid, axis, and tick labels.

function drawTitle(ctx: CanvasRenderingContext2D, P: Plane, m: Model, a: number): void {
  if (a <= 0) return;
  const tf = font(30, 500, MONO);
  ctx.save();
  ctx.globalAlpha *= a;
  P.at(ctx, AX0, TITLE_Y);
  let x = 0;
  if (m.subject) {
    drawText(ctx, m.subject, x, 0, { font: tf, fill: PALETTE.amber });
    x += layout(ctx, `${m.subject} `, tf).width;
    drawText(ctx, "·", x, 0, { font: tf, fill: PALETTE.text3 });
    x += layout(ctx, "· ", tf).width;
  }
  drawText(ctx, "next commit", x, 0, { font: tf, fill: PALETTE.text1 });
  ctx.restore();
}

function drawCaption(ctx: CanvasRenderingContext2D, P: Plane, a: number): void {
  if (a <= 0) return;
  ctx.save();
  ctx.globalAlpha *= a;
  P.at(ctx, AX0, SUB_Y);
  drawText(ctx, "build time, store warmed at the parent commit", 0, 0, {
    font: font(24, 400),
    fill: PALETTE.text3,
  });
  ctx.restore();
}

function drawRowLabels(ctx: CanvasRenderingContext2D, P: Plane, t: number, a: number): void {
  if (a <= 0) return;
  const lf = font(LABEL_SIZE, 600);
  for (const [label, y, fill, at, amp] of [
    ["cargo", ROW_CARGO, PALETTE.text2, CARGO_DONE, 0.05],
    ["mbx", ROW_MBX, PALETTE.amber, mbxLock(), 0.1],
  ] as const) {
    // Each label kicks when its bar lands: a small secondary beat.
    const kick = pulse(t, at, 0.015, 0.09);
    ctx.save();
    ctx.globalAlpha *= a;
    P.at(ctx, AX0 - 28, y);
    ctx.scale(1 + amp * kick, 1 + amp * kick);
    drawText(ctx, label, 0, 12, {
      font: lf,
      tracking: -0.6,
      align: "right",
      fill: kick > 0.05 ? mix(fill, PALETTE.paper, kick * 0.7) : fill,
    });
    ctx.restore();
  }
}

/** A vertical grid line from y0 up to y1, broken around `knock` boxes. */
function gridLine(
  ctx: CanvasRenderingContext2D,
  P: Plane,
  x: number,
  y0: number,
  y1: number,
  knock: Rect[],
): void {
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

function drawPlot(
  ctx: CanvasRenderingContext2D,
  P: Plane,
  m: Model,
  t: number,
  knock: Rect[],
  fade: Fade,
): void {
  // Grid lines rise from the axis on a stagger.
  const ticks = Math.round(m.max / m.step);
  const tf = font(TICK_SIZE, 500);
  ctx.save();
  const a0 = ctx.globalAlpha;
  ctx.lineCap = "butt";
  for (let i = 0; i <= ticks; i++) {
    const x = AX0 + (i * m.step * AXLEN) / m.max;
    const grow = swiftOut(progress(0.1 + i * 0.035, 0.38 + i * 0.035, t));
    const top = lerp(AXIS_Y, GRID_TOP, grow);
    const k = P.k(x, AXIS_Y);
    if (top < AXIS_Y - 0.5 && fade.line > 0) {
      ctx.globalAlpha = a0 * fade.line;
      ctx.strokeStyle =
        i === 0 ? rgba(PALETTE.text3, 0.8) : mix(PALETTE.divider, PALETTE.text3, 0.5 * fade.fall, 0.95);
      ctx.lineWidth = (i === 0 ? 2 : 1.5) * k;
      // The zero line is the spine: never broken.
      gridLine(ctx, P, x, AXIS_Y, top, i === 0 ? [] : knock);
    }
    // Tick marks hang below the axis.
    if (fade.line > 0) {
      ctx.globalAlpha = a0 * fade.line;
      ctx.strokeStyle = rgba(PALETTE.text3, 0.8);
      ctx.lineWidth = 2 * k;
      ctx.beginPath();
      seg(ctx, P, x, AXIS_Y, x, AXIS_Y + 10);
      ctx.stroke();
    }
    if (m.numbers && fade.text > 0) {
      const v = i * m.step;
      const txt = Number.isInteger(v) ? String(v) : v.toFixed(1);
      ctx.save();
      ctx.globalAlpha = a0 * fade.text;
      P.at(ctx, x, AXIS_Y + 44);
      drawText(ctx, txt, 0, 0, { font: tf, align: "center", fill: PALETTE.text3 });
      if (i === ticks) {
        const w = layout(ctx, txt, tf).width;
        drawText(ctx, "s", w / 2 + 6, 0, { font: font(TICK_SIZE * 0.8, 500), fill: PALETTE.text3 });
      }
      ctx.restore();
    }
  }
  // Axis baseline.
  if (fade.line > 0) {
    ctx.globalAlpha = a0 * fade.line;
    ctx.strokeStyle = rgba(PALETTE.text3, 0.8);
    ctx.lineWidth = 2 * P.k(AX0, AXIS_Y);
    ctx.beginPath();
    seg(ctx, P, AX0 - 14, AXIS_Y, AX0 + AXLEN + 14, AXIS_Y);
    ctx.stroke();
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
function tipLight(
  ctx: CanvasRenderingContext2D,
  P: Plane,
  x: number,
  row: number,
  a: number,
  color: string,
): void {
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
function landFlare(
  ctx: CanvasRenderingContext2D,
  P: Plane,
  x: number,
  row: number,
  k: number,
  color: string,
): void {
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
 * instant, from the top of the cargo bar to the foot of the mbx bar.
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
 * Cargo's value, 0..1, as its readout shows it: each lurch settles in about
 * four frames so the figure rests crisp between steps, and it locks when the
 * last crate lands.
 */
const cargoValue = (m: Model, t: number): number =>
  t >= CARGO_DONE ? 1 : cargoGrow(m.readSteps, t, 1.5);
/** Chart x of the cargo bar's tip, with a small recoil after the clunk. */
const cargoTip = (m: Model, t: number): number =>
  AX0 + (cargoEnd(m) - AX0) * cargoGrow(m.steps, t) + 6 * wobble(t, CARGO_DONE, 6, 16);

function drawCargo(ctx: CanvasRenderingContext2D, P: Plane, m: Model, t: number, fade: Fade): void {
  if (t < T.cargo) return;
  const tip = cargoTip(m, t);
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
    const v = cargoTip(m, t) - cargoTip(m, t - FRAME);
    tipLight(ctx, P, tip, ROW_CARGO, clamp(v / 70) * 0.8, PALETTE.tealLight);
    // A short, muted flare when the last crate lands on b17.75.
    landFlare(ctx, P, tip, ROW_CARGO, flash(t, CARGO_DONE, 0.04) * 0.6, PALETTE.tealLight);
  }
  // The readout rides the tip.
  // Full strength from its first frame: a half-faded figure reads as a ghost.
  const na = progress(T.cargo, T.cargo + 0.01, t) * fade.text;
  if (!m.numbers || na <= 0) return;
  const target = Number(m.cargoText);
  const v = cargoValue(m, t);
  const locked = t >= CARGO_DONE;
  drawOdometer(
    ctx,
    P,
    m.cargoText,
    v * target,
    locked ? null : cargoValue(m, t - FRAME) * target,
    readoutX(ctx, m.cargoText, v * target, tip),
    ROW_CARGO + NUM_SIZE * 0.36,
    PALETTE.text1,
    na,
    0,
    pulse(t, CARGO_DONE, 0.005, 0.06) * 0.6,
  );
}

/** Chart x of the mbx bar's tip. */
const mbxTip = (m: Model, t: number): number => AX0 + (mbxEnd(m) - AX0) * mbxGrow(t);

function drawMbxReadout(ctx: CanvasRenderingContext2D, P: Plane, m: Model, t: number, fade: Fade): void {
  if (!m.numbers || t < T.mbx) return;
  const lock = mbxLock();
  const target = Number(m.mbxText);
  const locked = t >= lock;
  const a = progress(T.mbx, T.mbx + 0.01, t) * fade.text;
  // Mechanical settle once locked: the lowest drum nudges past and back,
  // a tenth of a digit at most, never fast enough to blur.
  const settle = 0.2 * wobble(t, lock, 5, 13);
  drawOdometer(
    ctx,
    P,
    m.mbxText,
    locked ? target : clamp(mbxGrow(t)) * target,
    locked ? null : clamp(mbxGrow(t - FRAME)) * target,
    mbxTip(m, t) + NUM_GAP,
    ROW_MBX + NUM_SIZE * 0.36,
    PALETTE.amberBright,
    a,
    settle,
    pulse(t, lock, 0.005, 0.08),
  );
}

function drawMbxFx(ctx: CanvasRenderingContext2D, P: Plane, m: Model, t: number): void {
  if (t < T.mbx || t >= T.clear) return;
  const tip = mbxTip(m, t);
  const v = tip - mbxTip(m, t - FRAME);
  tipLight(ctx, P, tip, ROW_MBX, clamp(v / 50), PALETTE.amberBright);
  // The lock: a flare at the live tip, 3–4 frames, and the photo-finish
  // line, gone before mbx's overshoot or cargo's next step can reach it.
  landFlare(ctx, P, tip, ROW_MBX, flash(t, mbxLock(), 0.04), PALETTE.amberBright);
  const pf = flash(t, mbxLock(), 0.016);
  if (m.mbx < m.cargo && pf > 0.25) photoFinish(ctx, P, mbxEnd(m), pf);
  // A light sweep across the bar, entering it on b18.5. Cream laid over the
  // amber, never added, so no channel clips and the hue stays amber.
  const gp = progress(T.glint - 0.04, T.glint + 0.22, t);
  if (gp > 0 && gp < 1) {
    const x0 = AX0;
    const x1 = mbxEnd(m);
    const cx = lerp(x0 - 60, x1 + 160, inOutSine(gp));
    ctx.save();
    ctx.beginPath();
    ctx.rect(x0, ROW_MBX - TH / 2, x1 - x0, TH);
    ctx.clip();
    ctx.transform(1, 0, -0.5, 1, 0.5 * ROW_MBX, 0);
    const g = ctx.createLinearGradient(cx - 100, 0, cx + 100, 0);
    g.addColorStop(0, rgba(PALETTE.paper, 0));
    g.addColorStop(0.5, rgba(PALETTE.paper, 0.24));
    g.addColorStop(1, rgba(PALETTE.paper, 0));
    ctx.fillStyle = g;
    ctx.fillRect(cx - 100, ROW_MBX - TH / 2, 200, TH);
    ctx.restore();
  }
}

// The delta figure pops on b18.25: launched two frames early on a stiff
// spring so it is at full size on the beat, then overshoots.
const DELTA_POP = (t: number): number => spring(t - (T.label - 2 * FRAME), 5, 0.45, 40);

function drawDelta(ctx: CanvasRenderingContext2D, P: Plane, m: Model, t: number, fade: Fade): void {
  if (!m.deltaText || t < T.delta) return;
  const xa = mbxEnd(m);
  const xb = cargoEnd(m);
  const green = PALETTE.green;
  ctx.save();
  ctx.lineCap = "round";

  // The saved stretch of the cargo bar is hatched out, left to right.
  const hatch = outQuart(progress(T.delta + 0.05, T.delta + 0.26, t));
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
    const crawl = ((t - T.delta) * 36) % 20;
    ctx.beginPath();
    for (let x = xa - TH + crawl - 20; x < xa + w + 20; x += 20) seg(ctx, P, x, top + TH, x + TH, top);
    ctx.stroke();
    ctx.restore();
  }

  // Extension lines rise from each bar's end, then the dimension line spans
  // them. On b19 they draw back up into the dimension line and leave with the
  // words, so nothing points at the bar once it has gone.
  if (fade.text > 0) {
    ctx.save();
    ctx.globalAlpha *= fade.text;
    ctx.strokeStyle = green;
    const k = P.k(xa, BRK_Y);
    ctx.lineWidth = 2.5 * k;
    const back = swiftInOut(progress(T.clear - 0.03, T.clear + 0.1, t));
    const ext = (x: number, from: number, e: number) => {
      if (e <= 0 || back >= 1) return;
      const foot = lerp(from, BRK_Y - 14, back);
      ctx.save();
      ctx.setLineDash([7 * k, 7 * k]);
      ctx.beginPath();
      seg(ctx, P, x, foot, x, lerp(from, BRK_Y - 14, e));
      ctx.stroke();
      ctx.restore();
    };
    // The mbx line stands on the bar's top, which dips as the bar crouches.
    const mbxTop = ROW_MBX - TH / 2 + TH * CROUCH * crouchAt(t);
    ext(xa, mbxTop - 8, swiftOut(progress(T.delta, T.delta + 0.14, t)));
    ext(xb, ROW_CARGO - TH / 2 - 8, swiftOut(progress(T.delta + 0.04, T.delta + 0.15, t)));
    const span = swiftInOut(progress(T.delta + 0.04, T.delta + 0.19, t));
    if (span > 0) {
      ctx.beginPath();
      seg(ctx, P, xa, BRK_Y, lerp(xa, xb, span), BRK_Y);
      seg(ctx, P, xa, BRK_Y - 12, xa, BRK_Y + 12);
      if (span > 0.98) seg(ctx, P, xb, BRK_Y - 12, xb, BRK_Y + 12);
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
    const w = layout(ctx, m.deltaText, f, track).width;
    const uf = font(DELTA_SIZE * 0.5, 500);
    const uw = layout(ctx, "s", uf).width;
    const mw = DELTA_SIZE * 0.4;
    const gap = DELTA_SIZE * 0.1;
    const ugap = DELTA_SIZE * 0.12;
    const total = mw + gap + w + ugap + uw;
    ctx.save();
    ctx.globalAlpha *= clamp(pop * 3) * fade.text;
    P.at(ctx, (xa + xb) / 2, BRK_Y - 24);
    ctx.scale(s, s);
    const x0 = -total / 2;
    ctx.fillStyle = green;
    ctx.fillRect(x0, -DELTA_SIZE * 0.34, mw, DELTA_SIZE * 0.075);
    drawText(ctx, m.deltaText, x0 + mw + gap, 0, { font: f, tracking: track, fill: green });
    drawText(ctx, "s", x0 + mw + gap + w + ugap, 0, { font: uf, fill: rgba(green, 0.8) });
    ctx.restore();
  }
  ctx.restore();
}

/** 0..1 anticipation before the collapse: the mbx bar crouches into the floor. */
const crouchAt = keys([
  [T.clear - 0.11, 0],
  [T.clear, 1, swiftOut],
  // The release snaps on b19, with the swipe.
  [T.hit - 0.02, 0, outCubic],
]);
const CROUCH = 0.11;

/** The mbx bar, from chart bar through square to cube, in world space. */
function drawMbxBox(ctx: CanvasRenderingContext2D, view: View, m: Model, t: number): void {
  if (t < T.mbx) return;
  const p = pivot(m);
  const wx = (sx: number) => (sx - p.x) / S0;
  // Collapse: the zero end rushes to the tip and the bar compacts to a square.
  const tip = mbxTip(m, t);
  const left = collapseLeft(m, t);
  // Anticipation: the bar crouches into the floor before it compacts, then
  // an impact squash where the square forms, recovering through the swing.
  const crouch = crouchAt(t);
  const sq = 0.14 * wobble(t, T.hit, 4, 9) * (1 - progress(EXTRUDE[1] - 0.1, EXTRUDE[1], t));
  const cx = wx((tip + left) / 2);
  const hw = ((tip - left) / S0 / 2) * (1 - sq);
  const hgt = CUBE * (1 + sq * 0.9) * (1 - CROUCH * crouch);
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
  collapseTrail(ctx, view, m, t, wx(left), hgt);
  drawSlab(ctx, view, [cx - hw, 0, -CUBE / 2], [cx + hw, hgt, -CUBE / 2 + depth], look);
}

/**
 * Where the collapsing bar's zero end is, chart px: a latch-release jolt on
 * b19 (visible on its first frame), then the rush into b19.25.
 */
function collapseLeft(m: Model, t: number): number {
  const c =
    0.07 * outCubic(progress(T.clear, T.clear + 2.5 * FRAME, t)) +
    0.93 * inCubic(progress(T.clear, T.hit, t));
  return lerp(AX0, mbxTip(m, t) - TH, c);
}

/**
 * The zero end covers most of its travel in the two frames before b19.25, so
 * it drags a smear of the ground it just crossed, and on impact that smear
 * breaks into three speed ticks that are reeled in behind the square.
 */
function collapseTrail(
  ctx: CanvasRenderingContext2D,
  view: View,
  m: Model,
  t: number,
  left: number,
  hgt: number,
): void {
  if (t < T.clear || t > T.hit + 0.12) return;
  const p = pivot(m);
  const wx = (sx: number) => (sx - p.x) / S0;
  const z = -CUBE / 2;
  ctx.save();
  if (t < T.hit) {
    // About a frame and a half of travel, fading back toward where the end
    // was; only once the end is really moving, or it reads as a shadow.
    const from = wx(collapseLeft(m, t - FRAME * 1.5));
    const run = left - from;
    if (run > 16 / S0) {
      const a = view.project([from, hgt / 2, z]);
      const c = view.project([left, hgt / 2, z]);
      const g = ctx.createLinearGradient(a.x, a.y, c.x, c.y);
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
      const c = view.project([left - gap - reach * r, hgt * h, z]);
      ctx.moveTo(a.x, a.y);
      ctx.lineTo(c.x, c.y);
    }
    ctx.stroke();
  }
  ctx.restore();
}

export const scene: Scene = {
  id: "data",
  start: bar(4),
  end: bar(5),
  draw(ctx, lt, env) {
    const t = lt;
    ctx.fillStyle = PALETTE.bg;
    ctx.fillRect(0, 0, env.W, env.H);
    if (t >= T_REST) {
      drawStagedBox(ctx, WORLD_CAM, H5_POSE);
      return;
    }
    const m = model(env.facts);
    const cam = camAt(m, t);
    const view = new View(cam);

    if (t >= T.clear) {
      // The page falls back behind the cube, every element projected.
      const P = projPlane(new View(pageCam(cam, t)), m, -CUBE / 2 - recede(t));
      const fade = pageFade(t);
      ctx.save();
      drawPlot(ctx, P, m, t, readoutBoxes(ctx, m, t, fade.text), fade);
      drawTitle(ctx, P, m, fade.text);
      drawCaption(ctx, P, fade.text);
      drawRowLabels(ctx, P, t, fade.text);
      drawCargo(ctx, P, m, t, fade);
      drawDelta(ctx, P, m, t, fade);
      drawMbxReadout(ctx, P, m, t, fade);
      ctx.restore();
      drawMbxBox(ctx, view, m, t);
      return;
    }

    speedLines(ctx, t);
    // Behind the box: the chart plane, its layers whipping in on a stagger.
    ctx.save();
    enterChart(ctx, view, m);
    const knock = readoutBoxes(ctx, m, t);
    const plotOff = L_PLOT.x(t);
    whipLayer(ctx, L_PLOT, t, (g) => drawPlot(g, FLAT, m, t, knock, OPAQUE));
    whipLayer(ctx, L_TITLE, t, (g) => drawTitle(g, FLAT, m, 1));
    whipLayer(ctx, L_SUB, t, (g) => drawCaption(g, FLAT, 1));
    whipLayer(ctx, L_LABELS, t, (g) => drawRowLabels(g, FLAT, t, 1));
    // The bars and their readouts ride the plot's layer.
    ctx.translate(plotOff, 0);
    drawCargo(ctx, FLAT, m, t, OPAQUE);
    drawDelta(ctx, FLAT, m, t, OPAQUE);
    ctx.restore();

    drawMbxBox(ctx, view, m, t);

    // Light and the readout ride on top of the bar.
    ctx.save();
    enterChart(ctx, view, m);
    drawMbxFx(ctx, FLAT, m, t);
    drawMbxReadout(ctx, FLAT, m, t, OPAQUE);
    ctx.restore();
  },
};
