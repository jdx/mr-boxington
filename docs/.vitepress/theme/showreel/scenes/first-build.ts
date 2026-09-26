// Section 6, "First build fills the store". The store is empty, so rustc
// compiles as usual: an amber ring races round `syn`'s chip beside the
// pane's `Compiling syn`, and its output pops out as a carton that arcs in
// under the raised end of Mr Boxington's lid, a gulp. Then a cutaway: a hole opens in his front and the camera
// pushes in on the store inside him, where `syn`'s carton lands on an empty
// shelf and the inputs of its key snap onto the wall: the crate's sources,
// rustc, features, profile and RUSTFLAGS, and the two paths in its rustc
// call, which flip letter by letter to `${cargo_registry}` and `${target}`.
// The key stamps onto the carton's tag. The camera backs out, the rest of
// the plan streams in under the lid, and the lid steps down four times with
// the terminal's own mascot while its real bar fills grey: a cold store
// mostly reports "not looked up". The build finishes taped, with no blush
// and no strawberry, the mascot's real cold finish, and no fanfare.
//
// The section starts on under-cargo-build's frame (map.ts UC_END: pushed in
// on the whole machine, the pane and its mascot in), holds it until the push
// into the store, and settles onto FB_END at home. Nothing here depends on
// the benchmark's numbers except the text of two chips.

import { BEAT, PALETTE, type Scene, type SceneEnv, sec } from "../bible";
import { type BoxPose, boxFrame, boxPoint, frontMatrix, LID_THICK, LOGO_DIMS } from "../box";
import { mix, rgba } from "../color";
import type { ReelFacts } from "../facts";
import { glow, ring, roundedRect } from "../fx";
import {
  applyMapCam,
  boxCam,
  boxFromSprite,
  type BoxSpot,
  type BuildPlan,
  type BuildView,
  buildAt,
  CAPTION_TOP,
  type ChipState,
  chipRect,
  type Curve,
  curveAt,
  drawArrow,
  drawCarton,
  drawChip,
  drawFrame,
  drawMapBox,
  drawPane,
  drawSpark,
  drawTag,
  drawTower,
  FB_END,
  FLOOR,
  HOME,
  KIT,
  MACHINE,
  type MapCam,
  mapToScreen,
  PLAN,
  planChip,
  type Pt,
  UC_END,
  type Unit,
} from "../map";
import { clamp, cubicBezier, hash, inQuad, lerp, progress, smoothstep, swiftIn, swiftInOut, swiftOut } from "../math";
import { polygon, type Projected, View } from "../space";
import { type Caption, drawWords, font, layout, MONO, wordStyle } from "../type";
import { bouncyLid, bump, jolt, land, lidSteps, tapeAt } from "./first-build-kit";

const S = sec("first-build");

/** Local time of beat `n` of the section. */
const b = (n: number): number => n * BEAT;
/** Section-local time of global time `t`. */
const local = (t: number): number => t - S.start;

// The beat map, section-local seconds. The score (score/first-build.ts)
// places its cues on these.

/** The pane's status names the crate the ring is compiling. */
export const T_STATUS = b(0.25);
/** The amber ring runs round `syn`'s chip, twice, speeding up. */
export const T_RING0 = 0;
/** It closes: the chip collapses and the carton pops out of it. */
export const T_RING1 = b(2);
/** The carton leaves on its arc. */
export const T_THROW = b(2.25);
/** It drops in under the lid: the gulp. */
export const T_GULP = b(3.25);
/** A hole opens in his front, from the middle out. */
export const T_OPEN0 = b(4);
export const T_OPEN1 = b(4.5);
/** The camera pushes in on the store. */
export const T_PUSH0 = b(4.25);
export const T_PUSH1 = b(5.25);
/** Inside, `syn`'s carton lands on the shelf, and its tag swings out. */
export const T_SHELF = b(4.5);
export const T_TAG = b(5);
/** The two paths in its rustc call snap onto the wall, then the key's other inputs, on eighths. */
export const T_PATHS = [b(5.25), b(5.5)] as const;
export const T_CHIPS = [b(5.75), b(6.25), b(6.75), b(7.25), b(7.75)] as const;
/** Each path flips to its placeholder, letter by letter. */
export const T_REWRITE = [
  [b(7.5), b(8.5)],
  [b(8), b(9)],
] as const;
/**
 * A thread shoots from each input into the tag, top to bottom, and the key
 * stamps on as the last one lands: the key is made of them.
 */
export const T_HASH = b(8.5);
export const THREAD = b(0.3);
export const T_THREADS = Array.from({ length: 7 }, (_, i) => T_HASH + b(0.07) * i + THREAD);
export const T_STAMP = b(9.25);
/** The camera backs out, and the hole in his front shuts as it goes. */
export const T_PULL0 = b(9.5);
export const T_PULL1 = b(10.5);
export const T_CLOSE0 = b(9.85);
export const T_CLOSE1 = b(10.2);
/** The rest of the plan streams in under the lid. */
export const T_STREAM0 = b(10);
export const T_STREAM1 = b(11.1);
/** The build finishes; the lid is taped. */
export const T_FINISH = b(11.5);
export const T_TAPE0 = b(11.5);
export const T_TAPE1 = b(11.875);

const SPOT: BoxSpot = MACHINE.box;
const SYN = PLAN.indexOf("syn");

// The build, in reel time. Its counts are the reel's own, not a benchmark's:
// the bar's share is all a viewer reads, and a cold store answers every
// unit "not looked up", so the bar fills grey.

/** Units in the plan. The lid steps at 9, 18, 27 and 36 done. */
const TOTAL = 40;
/** Units that land in the stream, after `syn`; the last few finish under a shut lid. */
const STREAMED = 35;
/** Seconds a streamed carton flies. */
const FLY = 0.24;
/** The plan's other named crates, in PLAN order, and the streamed unit each one is. */
const NAMED = PLAN.map((name, i) => ({ name, i })).filter((c) => c.name !== "syn");
const NAMED_UNIT = [5, 10, 15, 20, 25, 30, 35];

const streamAt = (k: number): number => S.at(lerp(T_STREAM0, T_STREAM1, (k - 1) / (STREAMED - 1)));

export const BUILD: BuildPlan = {
  start: S.start,
  total: TOTAL,
  units: [
    { at: S.at(T_GULP), outcome: "other" },
    ...Array.from({ length: STREAMED }, (_, i): Unit => ({ at: streamAt(i + 1), outcome: "other" })),
    ...Array.from({ length: TOTAL - 1 - STREAMED }, (_, i): Unit => ({ at: S.at(b(11.2 + 0.06 * i)), outcome: "other" })),
  ],
  finish: S.at(T_FINISH),
};

/** Global times the lid steps down, with the terminal's mascot: the score's four clicks. */
export const LID_STEPS = lidSteps(BUILD, S.end);
/** When each of the plan's other named crates drops in, section-local seconds: the score's quick gulps. */
export const T_NAMED = NAMED_UNIT.map((k) => streamAt(k) - S.start);

// Cameras.

/**
 * Where the camera looks into the store: pushed in until his front's border
 * runs down the frame's edges, left of where the captions start, with the
 * rim just under the top.
 */
const INSIDE: Readonly<MapCam> = { cx: 440, cy: 494.8, zoom: 4.6 };
/**
 * under-cargo-build's push-in, which this section starts on and holds while
 * `syn` compiles: the whole machine, so the pane's `Compiling syn` and its
 * mascot read beside the ring.
 */
const FOLLOW: MapCam = UC_END.cam ?? HOME;

/**
 * A move from `a` to `b` zooming about the one map point that stays put on
 * screen, so it reads as a push rather than a pan: the zoom eased by `e` in
 * log space.
 */
function zoomCam(a: MapCam, b: MapCam, e: number): MapCam {
  if (e <= 0) return a;
  if (e >= 1) return b;
  const fx = (b.cx * b.zoom - a.cx * a.zoom) / (b.zoom - a.zoom);
  const fy = (b.cy * b.zoom - a.cy * a.zoom) / (b.zoom - a.zoom);
  const z = a.zoom * (b.zoom / a.zoom) ** e;
  return { cx: fx - ((fx - a.cx) * a.zoom) / z, cy: fy - ((fy - a.cy) * a.zoom) / z, zoom: z };
}

/** Backing out: off the mark in a few frames, then a long settle home. */
const PULL_EASE = cubicBezier(0.35, 0, 0.12, 1);

/** The camera at `lt`: at FOLLOW's rest, then the push in and the long way home. */
function camAt(lt: number): MapCam {
  if (lt < T_PULL0) return zoomCam(FOLLOW, INSIDE, swiftInOut(progress(T_PUSH0, T_PUSH1, lt)));
  return zoomCam(INSIDE, HOME, PULL_EASE(progress(T_PULL0, T_PULL1, lt)));
}

/** How far the rest of the map has dimmed around the cutaway: gone before the push has gone far, back as it ends. */
const spotAt = (lt: number): number =>
  0.95 * (smoothstep(T_PUSH0, T_PUSH0 + b(0.6), lt) - smoothstep(T_PULL0, T_PULL0 + b(0.75), lt));

/** How far the hole in his front is open, 0..1: out from the middle of his face, then shut again the same way. */
const openAt = (lt: number): number =>
  swiftOut(progress(T_OPEN0, T_OPEN1, lt)) * (1 - swiftIn(progress(T_CLOSE0, T_CLOSE1, lt)));

// Cargo's plan: the chips under-cargo-build left, `syn`'s amber.

/** `syn`'s chip, and the ring's track round it. */
const SYN_CHIP = planChip(SYN, "amber");
const SYN_CENTER: Pt = { x: SYN_CHIP.x + (SYN_CHIP.w ?? 0) / 2, y: SYN_CHIP.y + 28 };

/** A point `u` (0..1, clockwise from the top middle) round a rounded rect, with its outward normal. */
function around(x: number, y: number, w: number, h: number, r: number, u: number): { p: Pt; n: Pt } {
  const straight = [w - 2 * r, h - 2 * r];
  const quarter = (Math.PI * r) / 2;
  const total = 2 * straight[0] + 2 * straight[1] + 4 * quarter;
  let d = (((u % 1) + 1) % 1) * total;
  // Top edge's right half, then round clockwise.
  const runs: [number, (s: number) => { p: Pt; n: Pt }][] = [
    [straight[0] / 2, (s) => ({ p: { x: x + w / 2 + s, y }, n: { x: 0, y: -1 } })],
    [quarter, (s) => arcPt(x + w - r, y + r, -Math.PI / 2 + s / r)],
    [straight[1], (s) => ({ p: { x: x + w, y: y + r + s }, n: { x: 1, y: 0 } })],
    [quarter, (s) => arcPt(x + w - r, y + h - r, s / r)],
    [straight[0], (s) => ({ p: { x: x + w - r - s, y: y + h }, n: { x: 0, y: 1 } })],
    [quarter, (s) => arcPt(x + r, y + h - r, Math.PI / 2 + s / r)],
    [straight[1], (s) => ({ p: { x, y: y + h - r - s }, n: { x: -1, y: 0 } })],
    [quarter, (s) => arcPt(x + r, y + r, Math.PI + s / r)],
    [straight[0] / 2, (s) => ({ p: { x: x + r + s, y }, n: { x: 0, y: -1 } })],
  ];
  for (const [len, at] of runs) {
    if (d <= len) return at(d);
    d -= len;
  }
  return runs[0][1](0);
  function arcPt(cx: number, cy: number, a: number) {
    return { p: { x: cx + r * Math.cos(a), y: cy + r * Math.sin(a) }, n: { x: Math.cos(a), y: Math.sin(a) } };
  }
}

/** The ring's head: laps of the chip done at `lt`, speeding up. */
const RING_LAPS = 2;
const ringAt = (lt: number): number => RING_LAPS * inQuad(progress(T_RING0, T_RING1, lt)) ** 0.8;

function drawRing(ctx: CanvasRenderingContext2D, lt: number): void {
  if (lt <= T_RING0 || lt > T_RING1 + b(1)) return;
  const c = SYN_CHIP;
  const w = c.w ?? 320;
  const h = 56;
  const pad = 7;
  const box = [c.x - pad, c.y - pad, w + 2 * pad, h + 2 * pad, h / 2 + pad] as const;
  const laps = ringAt(lt);
  // It catches on the downbeat, and flares up over the first few frames.
  const catches = clamp((lt - T_RING0) / 0.1);
  if (lt <= T_RING1) {
    // The track so far, dim, then the hot trail behind the head.
    const lit = clamp(laps);
    ctx.save();
    ctx.globalAlpha *= catches;
    ctx.lineCap = "round";
    ctx.strokeStyle = rgba(PALETTE.amber, 0.35 + 0.35 * clamp(laps - 1));
    ctx.lineWidth = 5;
    ctx.beginPath();
    const n = 64;
    for (let i = 0; i <= n * lit; i++) {
      const { p } = around(...box, (i / n) * lit);
      if (i === 0) ctx.moveTo(p.x, p.y);
      else ctx.lineTo(p.x, p.y);
    }
    ctx.stroke();
    const trail = 0.22 + 0.12 * clamp(laps - 1);
    for (let i = 16; i >= 1; i--) {
      const a = around(...box, laps - (trail * i) / 16);
      const e = around(...box, laps - (trail * (i - 1)) / 16);
      const k = 1 - i / 17;
      ctx.strokeStyle = rgba(mix(PALETTE.amber, "#fff3d6", k), 0.2 + 0.8 * k);
      ctx.lineWidth = 4 + 7 * k;
      ctx.beginPath();
      ctx.moveTo(a.p.x, a.p.y);
      ctx.lineTo(e.p.x, e.p.y);
      ctx.stroke();
    }
    const head = around(...box, laps).p;
    glow(ctx, head.x, head.y, 70, PALETTE.amberBright, 0.9);
    ctx.restore();
    // Sparks thrown off the head: the crackle.
    for (let j = 0; j < 60; j++) {
      const born = lerp(T_RING0 + 0.02, T_RING1, j / 60);
      const life = 0.2 + 0.2 * hash(j, 71);
      const age = lt - born;
      if (age <= 0 || age >= life) continue;
      const from = around(...box, ringAt(born));
      const spin = (hash(j, 73) - 0.5) * 1.4;
      const dir = { x: from.n.x * Math.cos(spin) - from.n.y * Math.sin(spin), y: from.n.x * Math.sin(spin) + from.n.y * Math.cos(spin) };
      const v = 260 + 320 * hash(j, 79);
      const at = (s: number): Pt => ({ x: from.p.x + dir.x * v * s, y: from.p.y + dir.y * v * s + 900 * s * s });
      const q0 = at(Math.max(0, age - 0.03));
      const q1 = at(age);
      ctx.strokeStyle = rgba(PALETTE.amberBright, 1 - age / life);
      ctx.lineWidth = 3;
      ctx.lineCap = "round";
      ctx.beginPath();
      ctx.moveTo(q0.x, q0.y);
      ctx.lineTo(q1.x, q1.y);
      ctx.stroke();
    }
  }
  // It closes: a shockwave off the chip and a flash.
  const k = progress(T_RING1, T_RING1 + 0.4, lt);
  if (k > 0 && k < 1) {
    ring(ctx, SYN_CENTER.x, SYN_CENTER.y, 300, k, PALETTE.amberBright, 10);
    glow(ctx, SYN_CENTER.x, SYN_CENTER.y, 260, PALETTE.amber, 0.8 * (1 - k) ** 2);
  }
}

/** The chip a named crate's carton leaves from, lighting amber just before, then collapsing. */
function chipAt(i: number, lt: number): ChipState | null {
  if (i === SYN) {
    // Heating as the ring runs, then collapsing into its carton.
    const heat = clamp(ringAt(lt) / RING_LAPS);
    const gone = progress(T_RING1, T_RING1 + 0.09, lt);
    if (gone >= 1) return null;
    return planChip(i, "amber", {
      ...(heat > 0 ? { lit: 0.3 * clamp((lt - T_RING0) / 0.1) + 0.7 * heat } : {}),
      ...(gone > 0 ? { scale: 1 - swiftIn(gone) } : {}),
    });
  }
  const named = NAMED.findIndex((c) => c.i === i);
  const launch = local(streamAt(NAMED_UNIT[named])) - FLY;
  const warm = progress(launch - b(0.3), launch, lt);
  const gone = progress(launch, launch + 0.08, lt);
  if (gone >= 1) return null;
  if (warm <= 0) return planChip(i, "dim");
  return planChip(i, "amber", { lit: warm, ...(gone > 0 ? { scale: 1 - swiftIn(gone) } : {}) });
}

// Flights into the box, drawn inside it: under the lid and behind the front.

export interface Flight {
  curve: Curve;
  /** Global launch and landing. */
  t0: number;
  t1: number;
  size: number;
  spin: number;
  label?: string;
}

/** A throw from `a` that passes through `gap`, under the lid's right edge, and drops deep into the box at `x`. */
function intoMouth(a: Pt, gap: Pt, x: number): Curve {
  const end: Pt = { x, y: 480 };
  // The control that puts the curve's midpoint on the gap.
  return { a, b: end, c: { x: 2 * gap.x - (a.x + end.x) / 2, y: 2 * gap.y - (a.y + end.y) / 2 } };
}

export const SYN_FLIGHT: Flight = {
  curve: { a: { x: SYN_CENTER.x, y: SYN_CENTER.y - 8 }, c: { x: 650, y: 236 }, b: { x: 452, y: 470 } },
  t0: S.at(T_THROW),
  t1: S.at(T_GULP),
  size: 62,
  spin: 0.9,
  label: "syn",
};

/**
 * The lid's underside as the stream goes in, map px: it steps down with the
 * build, so each carton aims for the middle of the gap it will meet.
 */
const gapMid = (t: number): number => (292 + (4 - bouncyLid(LID_STEPS, t)) * (376 / 16) + 386) / 2;

/**
 * The stream after the cutaway: the named crates from their chips, and
 * smaller cartons for some of the rest from the plan around them (most units
 * go in unseen).
 */
export const STREAM: readonly Flight[] = Array.from({ length: STREAMED }, (_, i) => i + 1).flatMap((k): Flight[] => {
  const named = NAMED_UNIT.indexOf(k);
  if (named < 0 && k % 4 === 3) return [];
  const t1 = streamAt(k);
  const size = named >= 0 ? 44 : lerp(20, 28, hash(k, 313));
  const from: Pt =
    named >= 0
      ? { x: MACHINE.chips.x + 34, y: planChip(NAMED[named].i).y + 44 }
      : { x: lerp(700, 980, hash(k, 301)), y: lerp(200, 650, hash(k, 303)) };
  const fly = FLY * lerp(0.85, 1.15, hash(k, 311));
  const gap = { x: lerp(600, 650, hash(k, 319)), y: gapMid(t1 - fly / 2) + size * 0.4 };
  return [{ curve: intoMouth(from, gap, lerp(300, 540, hash(k, 307))), t0: t1 - fly, t1, size, spin: (hash(k, 317) - 0.5) * 1.6 }];
});

function drawFlights(ctx: CanvasRenderingContext2D, t: number): void {
  for (const f of [SYN_FLIGHT, ...STREAM]) {
    if (t < f.t0 || t >= f.t1 + 0.05) continue;
    const u = progress(f.t0, f.t1, t);
    const p = curveAt(f.curve, u);
    // The stream's cartons pop out of the plan as they leave; syn's has already popped.
    const grow = f === SYN_FLIGHT ? 1 : land(t, f.t0, 0.07, 0.2);
    drawCarton(ctx, p.x, p.y, f.size * grow, "compiled", {
      rot: f.spin * (u - 0.3),
      sy: 1 + 0.12 * Math.sin(Math.PI * u),
      sx: 1 - 0.08 * Math.sin(Math.PI * u),
      label: f.label,
    });
  }
}

/** `syn`'s carton popping out of its chip and hovering for the throw. */
function drawSynPop(ctx: CanvasRenderingContext2D, lt: number): void {
  if (lt < T_RING1 || lt >= T_THROW) return;
  const k = land(lt, T_RING1, 0.2, 0.3);
  const rise = swiftOut(progress(T_RING1, T_THROW, lt));
  const a = SYN_FLIGHT.curve.a;
  drawCarton(ctx, a.x, lerp(a.y + 30, a.y, rise), SYN_FLIGHT.size * k, "compiled", {
    rot: -0.27 * rise,
    label: k > 0.8 ? "syn" : undefined,
    lit: 1 - rise,
  });
}

// Mr Boxington.

/** His eye, map px, for aiming his look. */
const EYE: Pt = { x: 343, y: 508 };

function boxAt(lt: number, view: BuildView, inside: ((ctx: CanvasRenderingContext2D) => void) | undefined): BoxPose {
  const t = S.at(lt);
  // The chomp: the lid comes down on the carton as it drops in, and he squashes.
  const chomp = bump(lt, T_GULP - 0.07, 0.3);
  const squash = 1 - 0.07 * chomp + 0.025 * jolt(lt, T_GULP + 0.2, 0.4, 5);
  const lid = bouncyLid(LID_STEPS, t) - 1.5 * chomp;
  const tape = tapeAt(lt, T_TAPE0, T_TAPE1);
  // His eyes follow the carton from its chip into his mouth, then go back
  // to the plan; the rest of the time they are the terminal's mascot's.
  const follow = bump(lt, T_RING1 - b(0.25), T_GULP - T_RING1 + b(0.5));
  let look: [number, number] | undefined;
  if (follow > 0) {
    const f = lt < T_THROW ? SYN_FLIGHT.curve.a : curveAt(SYN_FLIGHT.curve, progress(T_THROW, T_GULP, lt));
    const d = Math.hypot(f.x - EYE.x, f.y - 60 - EYE.y) || 1;
    const aim: [number, number] = [(4 * (f.x - EYE.x)) / d, (4 * (f.y - 60 - EYE.y)) / d];
    const rest = boxFromSprite(view.pose).face?.look ?? [0, 0];
    look = [lerp(rest[0], aim[0], follow), lerp(rest[1], aim[1], follow)];
  }
  const pose = boxFromSprite(view.pose, {
    ...(look ? { look } : {}),
    ...(chomp > 0 ? { eyelid: chomp, twitch: 0.12 * chomp } : {}),
  });
  return {
    ...pose,
    lid: view.pose.taped ? 0 : lid,
    tape,
    ...(squash !== 1 ? { squash } : {}),
    ...(inside ? { inside } : {}),
  };
}

// The cutaway: a hole in his front, and the store behind it.

/** The front, logo units: x 4-124, from the rim (27) to the floor (124). */
const RIM_Y = 27;
/** The hole at its widest: the front less a 3-unit border, cut through the base. */
const HOLE = { x0: 7, x1: 121, y0: 30, y1: 132 };

// The store's layout, in px as they show when the camera is INSIDE: `syn`'s
// carton on the shelf at the left with its tag, the inputs stacked on the
// wall at the right, and empty slots on the shelf under them.

const SHELF_Y = 670;
const SHELF_X = [239, 1681] as const;
const CARTON_X = 440;
const CARTON_SIZE = 190;
const SLOTS = [960, 1130, 1300, 1470];
const TAG_AT: Pt = { x: 548, y: 580 };
/** The inputs: three rows of key chips at 40 px, then the two paths at 56 px. */
const LEFT = 850;
const ROWS = [150, 220, 290, 374, 466];
/** Which row each key chip sits in. */
const KEY_ROW = [0, 1, 1, 2, 2];

/** Map px from the store's px. */
function toMap(ctx: CanvasRenderingContext2D): void {
  ctx.translate(INSIDE.cx - 960 / INSIDE.zoom, INSIDE.cy - 540 / INSIDE.zoom);
  ctx.scale(1 / INSIDE.zoom, 1 / INSIDE.zoom);
}

/** The hole's outline, map px, `open` of the way out from the middle of his face. */
function holePath(ctx: CanvasRenderingContext2D, m: DOMMatrix2DLike, open: number): void {
  const cx = (HOLE.x0 + HOLE.x1) / 2;
  const cy = (HOLE.y0 + HOLE.y1) / 2;
  const hw = ((HOLE.x1 - HOLE.x0) / 2) * open;
  const hh = ((HOLE.y1 - HOLE.y0) / 2) * open;
  ctx.save();
  ctx.transform(m.a, m.b, m.c, m.d, m.e, m.f);
  roundedRect(ctx, cx - hw, cy - hh, 2 * hw, 2 * hh, 7 * open);
  ctx.restore();
}
type DOMMatrix2DLike = { a: number; b: number; c: number; d: number; e: number; f: number };

/** The store's walls and floor, drawn from the box's own corners so the push-in stays true. */
function drawWalls(ctx: CanvasRenderingContext2D, view: View, pose: BoxPose): void {
  const f = boxFrame(pose);
  const rim = 1 - (2 * LID_THICK * LOGO_DIMS[0]) / LOGO_DIMS[1];
  const P = (x: number, y: number, z: number): Projected => view.project(boxPoint(f, x, y, z));
  const quad = (pts: Projected[], fill: string | CanvasGradient) => {
    polygon(ctx, pts);
    ctx.fillStyle = fill;
    ctx.fill();
  };
  quad([P(-1, -1, 1), P(1, -1, 1), P(1, -1, -1), P(-1, -1, -1)], "#4c3417");
  quad([P(-1, -1, 1), P(-1, -1, -1), P(-1, rim, -1), P(-1, rim, 1)], "#3a2811");
  quad([P(1, -1, 1), P(1, -1, -1), P(1, rim, -1), P(1, rim, 1)], "#402c13");
  const top = P(0, rim, -1);
  const bottom = P(0, -1, -1);
  const g = ctx.createLinearGradient(0, top.y, 0, bottom.y);
  g.addColorStop(0, "#6a4b24");
  g.addColorStop(1, "#553b1b");
  quad([P(-1, -1, -1), P(1, -1, -1), P(1, rim, -1), P(-1, rim, -1)], g);
  // Light falls in from under the lid; the floor's back edge sits in shadow.
  const shade = ctx.createLinearGradient(0, bottom.y - 40, 0, bottom.y + 30);
  shade.addColorStop(0, "rgba(0,0,0,0)");
  shade.addColorStop(1, "rgba(0,0,0,0.28)");
  quad([P(-1, -1, 0.2), P(1, -1, 0.2), P(1, 0, -1), P(-1, 0, -1)], shade);
}

function drawShelf(ctx: CanvasRenderingContext2D): void {
  const [x0, x1] = SHELF_X;
  ctx.fillStyle = "rgba(0,0,0,0.25)";
  ctx.fillRect(x0, SHELF_Y + 36, x1 - x0, 16);
  ctx.fillStyle = KIT.slabTop;
  ctx.beginPath();
  ctx.moveTo(x0 + 14, SHELF_Y - 14);
  ctx.lineTo(x1 - 14, SHELF_Y - 14);
  ctx.lineTo(x1, SHELF_Y);
  ctx.lineTo(x0, SHELF_Y);
  ctx.closePath();
  ctx.fill();
  ctx.fillStyle = KIT.slabFront;
  ctx.fillRect(x0, SHELF_Y, x1 - x0, 30);
  ctx.fillStyle = KIT.slabBand;
  ctx.fillRect(x0, SHELF_Y + 24, x1 - x0, 6);
  // The empty store: slots waiting for cartons.
  ctx.save();
  ctx.strokeStyle = rgba(PALETTE.paper, 0.3);
  ctx.lineWidth = 3;
  ctx.setLineDash([10, 9]);
  for (const x of SLOTS) {
    roundedRect(ctx, x - 62, SHELF_Y - 104, 124, 100, 8);
    ctx.stroke();
  }
  ctx.restore();
}

/** One input chip snapping onto the wall at `at`: a drop, a pop, and a flash. */
function snap(lt: number, at: number): { alpha: number; dy: number; lit: number; scale: number } | null {
  if (lt < at) return null;
  const k = land(lt, at, 0.16, 0.2);
  return { alpha: clamp((lt - at) / 0.04), dy: -46 * (1 - k), lit: Math.exp(-(lt - at) / 0.18), scale: 0.9 + 0.1 * k };
}

/** A path chip's two texts, the second with its placeholder marked. */
interface PathChip {
  from: string;
  to: string;
  /** The placeholder's span in `to`. */
  mark: readonly [number, number];
}
const PATHS: readonly PathChip[] = [
  { from: "~/.cargo/registry/…", to: "${cargo_registry}/…", mark: [0, 17] },
  { from: "~/src/hk/target", to: "${target}", mark: [0, 9] },
];

/** How long one character takes to flip over, seconds. */
const FLIP = 0.07;

/** When character `i` of path chip `k` starts to flip, section-local seconds. */
function flipAt(k: number, i: number): number {
  const p = PATHS[k];
  const n = Math.max(p.from.length, p.to.length);
  const [t0, t1] = T_REWRITE[k];
  return t0 + (i * (t1 - t0 - FLIP)) / (n - 1);
}

/** Every character flip that changes a character, per path: the score's letter clicks. */
export const FLIPS: readonly (readonly number[])[] = PATHS.map((p, k) =>
  Array.from({ length: Math.max(p.from.length, p.to.length) }, (_, i) => i)
    .filter((i) => p.from[i] !== p.to[i])
    .map((i) => flipAt(k, i)),
);

/**
 * A path chip at 56 px whose characters flip over, split-flap fashion, from
 * `from` to `to` one after another between `t0` and `t1`, the chip easing
 * to its new width as they go.
 */
function drawPathChip(ctx: CanvasRenderingContext2D, k: number, x: number, y: number, lt: number, lit: number): void {
  const p = PATHS[k];
  const [t0, t1] = T_REWRITE[k];
  const size = 56;
  const spec = font(size, 500, MONO);
  const adv = layout(ctx, "M", spec).width;
  const n = Math.max(p.from.length, p.to.length);
  // The chip hugs the characters showing: the new text up to the flip, the
  // old one's tail after it, easing in as the tail flips away.
  let shown = 0;
  for (let i = 0; i < n; i++) {
    const f = (lt - flipAt(k, i)) / FLIP;
    const ch = f >= 1 ? p.to[i] : p.from[i];
    if (ch !== undefined && ch !== " ") shown = i + 1;
    else if (f > 0 && f < 1) shown = Math.max(shown, i + 1 - f);
  }
  const w = Math.round(shown * adv + size * 1.1);
  const q = progress(t0, t1, lt);
  const flipping = q > 0 && q < 1 ? 1 : 0;
  const r = drawChip(ctx, { text: "", x, y, w, size, tone: "neutral", lit: Math.max(lit, 0.6 * flipping) });
  const x0 = r.x + size * 0.55;
  const base = r.y + r.h / 2 + size * 0.35;
  ctx.save();
  ctx.font = spec;
  for (let i = 0; i < n; i++) {
    const f = (lt - flipAt(k, i)) / FLIP;
    const done = f >= 0.5;
    const ch = (done ? p.to[i] : p.from[i]) ?? " ";
    if (ch === " ") continue;
    const marked = done && i >= p.mark[0] && i < p.mark[1];
    const sy = f > 0 && f < 1 ? Math.abs(Math.cos(Math.PI * f)) : 1;
    ctx.fillStyle = f > 0 && f < 1.6 ? PALETTE.amberBright : marked ? PALETTE.tealLight : PALETTE.text1;
    ctx.save();
    ctx.translate(x0 + i * adv, base - size * 0.35);
    ctx.scale(1, sy);
    ctx.fillText(ch, 0, size * 0.35);
    ctx.restore();
  }
  ctx.restore();
}

function drawStore(ctx: CanvasRenderingContext2D, lt: number, facts: ReelFacts | null): void {
  drawShelf(ctx);

  // `syn`'s carton drops onto the shelf, and lights up as its key stamps on.
  if (lt >= T_SHELF - 0.16) {
    const fall = progress(T_SHELF - 0.16, T_SHELF, lt);
    const squash = jolt(lt, T_SHELF, 0.3, 4.5) * 0.12 + jolt(lt, T_STAMP, 0.3, 5) * 0.06;
    const y = SHELF_Y - 1100 * (1 - inQuad(fall)) * (fall < 1 ? 1 : 0);
    const lit = lt >= T_STAMP ? Math.exp(-(lt - T_STAMP) / 0.2) : 0;
    drawCarton(ctx, CARTON_X, y, CARTON_SIZE, "compiled", { sy: 1 - squash, sx: 1 + squash * 0.6, label: "syn", lit });
  }

  // The inputs, snapping onto the wall: the key chips in three rows, then
  // the two paths.
  const toolchain = facts?.toolchain;
  const keys = [
    facts?.subject === "hk" ? "syn 2.0.119 sources" : "syn sources",
    toolchain ? `rustc ${toolchain}` : "rustc",
    "features",
    "profile",
    "RUSTFLAGS",
  ];
  const rowX = ROWS.map(() => LEFT);
  const chips: ChipState[] = keys.map((text, i) => {
    const row = KEY_ROW[i];
    const c: ChipState = { text, x: rowX[row], y: ROWS[row], size: 40 };
    rowX[row] += chipRect(ctx, c).w + 16;
    return c;
  });
  // Where each input's thread leaves from, in thread order: inside its
  // chip's left end, so it comes out from under the chip.
  const ends: Pt[] = [
    ...chips.map((c) => ({ x: c.x + 20, y: c.y + 28 })),
    ...PATHS.map((_, i) => ({ x: LEFT + 28, y: ROWS[3 + i] + 39 })),
  ];
  // Each chip lights as its thread leaves, and all of them as the key stamps on.
  const hot = (i: number) =>
    Math.max(bump(lt, T_THREADS[i] - THREAD - 0.02, THREAD + 0.12), bump(lt, T_STAMP - 0.03, 0.32));

  // The threads, under the chips and the tag.
  ends.forEach((a, i) => drawKeyThread(ctx, keyThread(a), lt, i));

  chips.forEach((c, i) => {
    const s = snap(lt, T_CHIPS[i]);
    if (s) drawChip(ctx, { ...c, y: c.y + s.dy, alpha: s.alpha, lit: Math.max(s.lit, hot(i)), scale: s.scale });
  });
  PATHS.forEach((_, i) => {
    const s = snap(lt, T_PATHS[i]);
    if (!s) return;
    const y = ROWS[3 + i];
    ctx.save();
    ctx.globalAlpha *= s.alpha;
    const cx = LEFT + 300;
    const cy = y + 39 + s.dy;
    ctx.translate(cx, cy);
    ctx.scale(s.scale, s.scale);
    ctx.translate(-cx, -cy);
    drawPathChip(ctx, i, LEFT, y + s.dy, lt, Math.max(s.lit, hot(keys.length + i)));
    ctx.restore();
  });

  // The tag: out on its string from b5, blank while the threads run into
  // it, twitching as each lands, until the key stamps on.
  if (lt >= T_TAG) {
    const swing = land(lt, T_TAG, 0.35, 0.4);
    const twitch = T_THREADS.reduce((k, at) => k + jolt(lt, at, 0.22, 7) * 0.05, 0);
    const knot: Pt = { x: CARTON_X + CARTON_SIZE * 0.36, y: SHELF_Y - CARTON_SIZE * 0.84 };
    const slam = lt >= T_STAMP ? 1 + 0.35 * (1 - land(lt, T_STAMP, 0.18, 0.3)) : 1;
    ctx.save();
    ctx.translate(TAG_AT.x, TAG_AT.y);
    ctx.scale(slam, slam);
    ctx.rotate(-1.2 * (1 - swing) + twitch);
    ctx.translate(-TAG_AT.x, -TAG_AT.y);
    const stamped = lt >= T_STAMP;
    drawTag(ctx, TAG_AT.x, TAG_AT.y, stamped ? "key 9e1f…" : "key ····", {
      tone: "paper",
      string: { x: knot.x, y: knot.y },
      alpha: clamp((lt - T_TAG) / 0.05),
    });
    ctx.restore();
    const flash = progress(T_STAMP, T_STAMP + 0.35, lt);
    if (flash > 0 && flash < 1) {
      ring(ctx, KEY_AT.x, KEY_AT.y, 240, flash, PALETTE.paper, 8);
      glow(ctx, KEY_AT.x, KEY_AT.y, 220, PALETTE.amberBright, 0.7 * (1 - flash) ** 2);
    }
  }

  // Each thread's spark, riding in ahead of it, over everything.
  ends.forEach((a, i) => {
    const u = progress(T_THREADS[i] - THREAD, T_THREADS[i], lt);
    if (u > 0 && u < 1) drawSpark(ctx, keyThread(a), threadEase(u), { color: PALETTE.amberBright, size: 7, trail: 0.35 });
  });
}

/** Where the threads meet: the tag's blank, where the key is stamped. */
const KEY_AT: Pt = { x: TAG_AT.x + 190, y: TAG_AT.y };

/** An input's thread: out of its chip's left end, sagging down and left into the tag. */
function keyThread(a: Pt): Curve {
  return { a, c: { x: KEY_AT.x + 10, y: a.y + 40 }, b: KEY_AT };
}

const threadEase = (u: number): number => swiftInOut(u) * 0.7 + u * 0.3;

/**
 * Thread `i` drawing on into the tag, hot while it runs, flashing as the key
 * stamps on and then reeled in after it, gone a quarter-second later.
 */
function drawKeyThread(ctx: CanvasRenderingContext2D, k: Curve, lt: number, i: number): void {
  const t1 = T_THREADS[i];
  const to = threadEase(progress(t1 - THREAD, t1, lt));
  const from = swiftIn(progress(T_STAMP, T_STAMP + 0.24, lt));
  if (to <= 0 || from >= 1) return;
  const flash = bump(lt, T_STAMP - 0.04, 0.2);
  const color = mix(PALETTE.amber, PALETTE.paper, Math.max(flash, 0.5 * bump(lt, t1 - 0.04, 0.14)));
  drawArrow(ctx, k, { from, to, width: 4 + 2 * flash, color, head: 0, alpha: 0.9 });
}

function drawCutaway(ctx: CanvasRenderingContext2D, lt: number, pose: BoxPose, facts: ReelFacts | null): void {
  const open = openAt(lt);
  if (open <= 0) return;
  const view = new View(boxCam(SPOT));
  const m = frontMatrix(view, pose);
  ctx.save();
  // Inside the front wall only.
  ctx.save();
  ctx.transform(m.a, m.b, m.c, m.d, m.e, m.f);
  ctx.beginPath();
  ctx.moveTo(4, RIM_Y);
  ctx.lineTo(124, RIM_Y);
  ctx.arc(121, 121, 3, 0, Math.PI / 2);
  ctx.arc(7, 121, 3, Math.PI / 2, Math.PI);
  ctx.closePath();
  ctx.restore();
  ctx.clip();
  holePath(ctx, m, open);
  ctx.save();
  ctx.clip();
  drawWalls(ctx, view, pose);
  ctx.save();
  toMap(ctx);
  drawStore(ctx, lt, facts);
  ctx.restore();
  ctx.restore();
  // The cut edge: the card's thickness, lit along the top.
  holePath(ctx, m, open);
  ctx.lineWidth = 3;
  ctx.strokeStyle = "#8a5a1d";
  ctx.stroke();
  holePath(ctx, m, open);
  ctx.lineWidth = 1;
  ctx.strokeStyle = rgba("#ffe2a8", 0.8);
  ctx.stroke();
  ctx.restore();
}

// The terminal.

function paneView(lt: number): BuildView {
  const t = S.at(lt);
  const v = buildAt(BUILD, t);
  if (t >= (BUILD.finish ?? Infinity)) return { ...v, status: { text: "✓ Built", tone: "ok" } };
  if (lt < T_STATUS) return { ...v, status: { text: "Compiling", tone: "dim" } };
  // The last crate that went into the box, or syn while the ring runs.
  let name: string = "syn";
  NAMED.forEach((c, k) => {
    if (local(streamAt(NAMED_UNIT[k])) - FLY <= lt) name = c.name;
  });
  return { ...v, status: { text: `Compiling ${name}`, tone: "text" } };
}

// The header over him: the scenario, gone once the build is taped.

const HEADER = { ...wordStyle(40, PALETTE.text2), font: font(40, 500) };

function draw(ctx: CanvasRenderingContext2D, lt: number, env: SceneEnv): void {
  const facts = env.facts;
  const t = env.t;
  ctx.fillStyle = PALETTE.bg;
  ctx.fillRect(0, 0, env.W, env.H);
  const cam = camAt(lt);
  const view = paneView(lt);
  const flights = [SYN_FLIGHT, ...STREAM].some((f) => t >= f.t0 && t < f.t1 + 0.05);
  const pose = boxAt(lt, view, flights ? (c) => drawFlights(c, t) : undefined);

  ctx.save();
  applyMapCam(ctx, cam);
  if (FB_END.frame) drawFrame(ctx, FB_END.frame);
  if (FB_END.tower) drawTower(ctx, FB_END.tower);
  if (FB_END.pane) drawPane(ctx, { ...FB_END.pane, view });
  for (let i = 0; i < PLAN.length; i++) {
    const c = chipAt(i, lt);
    if (c) drawChip(ctx, c);
  }
  drawRing(ctx, lt);
  const spot = spotAt(lt);
  if (spot > 0) {
    ctx.fillStyle = rgba(PALETTE.bg, spot);
    ctx.fillRect(cam.cx - 1200 / cam.zoom, cam.cy - 700 / cam.zoom, 2400 / cam.zoom, 1400 / cam.zoom);
  }
  drawMapBox(ctx, { spot: SPOT, pose });
  drawCutaway(ctx, lt, pose, facts);
  drawSynPop(ctx, lt);
  drawWords(ctx, "first build · empty store", SPOT.x - 20, 187, HEADER, t, S.at(b(0.75)), S.at(b(11.25)), "center");
  ctx.restore();
  drawScrim(ctx, env, scrimAt(cam));
}

/**
 * How dark the captions' band is kept while the camera is in close: his
 * front sweeps down through it as the camera pushes in and backs out, and
 * inside him it is the store's floor. None while the floor the map stands
 * on is above the band, as it is at FOLLOW and HOME.
 */
const scrimAt = (cam: MapCam): number => smoothstep(CAPTION_TOP - 20, CAPTION_TOP + 50, mapToScreen(cam, { x: 0, y: FLOOR }).y);

/** A shadow over the captions' band, fading in from just under the shelf. */
function drawScrim(ctx: CanvasRenderingContext2D, env: SceneEnv, k: number): void {
  if (k <= 0) return;
  const top = CAPTION_TOP - 28;
  const g = ctx.createLinearGradient(0, top, 0, top + 56);
  g.addColorStop(0, rgba(PALETTE.bg, 0));
  g.addColorStop(1, rgba(PALETTE.bg, 0.86 * k));
  ctx.fillStyle = g;
  ctx.fillRect(0, top, env.W, env.H - top);
}

const CAPTIONS: readonly Caption[] = [
  {
    out: 5.5,
    lines: [
      { in: 0.75, text: "First build: `rustc` compiles," },
      { in: 1, text: "mbx stores." },
    ],
  },
  {
    out: 11.75,
    lines: [
      { in: 6.75, text: "Keyed by inputs," },
      { in: 7, text: "not by checkout path." },
    ],
  },
];

export const scene: Scene = {
  id: S.id,
  start: S.start,
  end: S.end,
  draw,
  captions: () => CAPTIONS,
};

// Kept for the tests: the box's pose and the pane on any frame.
export const frameState = (lt: number) => {
  const view = paneView(lt);
  return { cam: camAt(lt), view, pose: boxAt(lt, view, undefined), chips: PLAN.map((_, i) => chipAt(i, lt)) };
};
