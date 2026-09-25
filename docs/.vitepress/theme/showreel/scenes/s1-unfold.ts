// Scene 1, "Line & fold": two pens draw a flat cardboard net from one corner
// and meet at a corner of the base; the net turns to card in straight wipes
// radiating from that meeting point, the walls crouch together, then fold up
// on sixteenths into the closed, taped box that handoff 1 → 2 expects. The
// net is real geometry: six quads hinged in world space and drawn through the
// same camera and outline as drawBox, so the switch to drawBox once the lid
// is shut is invisible.

import { bar, beat, drawStagedBox, H1_POSE, HERO_CAM, PALETTE, type Scene } from "../bible";
import { boxFrame, boxPoint, drawShadow, OUTLINE, OUTLINE_RATIO, sparkle } from "../box";
import { mix, mixRGB, type RGB, rgba } from "../color";
import { glow, makeCanvas, shake } from "../fx";
import {
  clamp,
  cubicBezier,
  DEG,
  hash,
  lerp,
  outCubic,
  progress,
  pulse,
  smoothstep,
  swiftInOut,
  TAU,
  wobble,
} from "../math";
import {
  type Camera,
  cardboard,
  cardboardFill,
  mix3,
  polygon,
  type Projected,
  rotateAround,
  tone,
  type V3,
  View,
} from "../space";

// Beat map (local seconds; scene 1 starts at 0).
const T_LAUNCH = beat(0.125); // the pens leave on the first 32nd
const T_CLOSE = beat(1); // the pens meet and the outline closes
const T_FLOODED = 0.8; // every panel has turned to card
const T_CROUCH = beat(1.75); // the walls wind up together
const FOLDS = [beat(2), beat(2.25), beat(2.5), beat(2.75)];
const T_SLAM = beat(3);
const T_TAPE0 = beat(3.25);
const T_TAPE1 = beat(3.75);
/** From here the box is closed and drawBox renders it. */
const T_BOXED = 1.515;
const CAM_END = 1.72;
/** Each wall's rise. Longer than the gap between folds, so they cascade. */
const LEAD = 0.22;

const Q = Math.PI / 2;
const FLOOR = -0.5;
const R2 = Math.SQRT1_2;

type XZ = readonly [number, number];
type WallKey = "left" | "face" | "right" | "back";
type PanelKey = "base" | WallKey | "lid";
const onFloor = ([x, z]: XZ): V3 => [x, FLOOR, z];
const mixXZ = (a: XZ, b: XZ, t: number): XZ => [lerp(a[0], b[0], t), lerp(a[1], b[1], t)];

// Two pens leave the far corner of the left wall in opposite directions and
// meet at the base corner where the flood starts; each path is seven edges.
const PATH_A: XZ[] = [
  [-1.5, 0.5], [-1.5, -0.5], [-0.5, -0.5], [-0.5, -1.5],
  [-0.5, -2.5], [0.5, -2.5], [0.5, -1.5], [0.5, -0.5],
];
const PATH_B: XZ[] = [
  [-1.5, 0.5], [-0.5, 0.5], [-0.5, 1.5], [0.5, 1.5],
  [0.5, 0.5], [1.5, 0.5], [1.5, -0.5], [0.5, -0.5],
];
const IGNITE: V3 = onFloor(PATH_A[0]);
const CLOSE_AT: XZ = PATH_A[7];
const PEN_GAMMA = 1.45;
const penS = (lt: number) => 7 * progress(T_LAUNCH, T_CLOSE, lt) ** PEN_GAMMA;
const penT = (s: number) => T_LAUNCH + (T_CLOSE - T_LAUNCH) * (s / 7) ** (1 / PEN_GAMMA);

function pathAt(path: XZ[], s: number): XZ {
  const i = Math.min(Math.floor(s), path.length - 2);
  const f = clamp(s - i, 0, 1);
  return mixXZ(path[i], path[i + 1], f);
}

/** Floor points along `path` from arc length s0 to s1, vertices included. */
function pathSpan(path: XZ[], s0: number, s1: number): XZ[] {
  const out: XZ[] = [pathAt(path, s0)];
  for (let k = Math.floor(s0) + 1; k < s1; k++) out.push(path[k]);
  out.push(pathAt(path, s1));
  return out;
}

/** Path vertices where the pen actually turns (sparks fly off these). */
function turnsOf(path: XZ[]): number[] {
  const out: number[] = [];
  for (let k = 1; k < path.length - 1; k++) {
    const [a, b, c] = [path[k - 1], path[k], path[k + 1]];
    if (Math.abs((b[0] - a[0]) * (c[1] - b[1]) - (b[1] - a[1]) * (c[0] - b[0])) > 1e-6) out.push(k);
  }
  return out;
}
const TURNS_A = turnsOf(PATH_A);
const TURNS_B = turnsOf(PATH_B);

// Walls fold in a ring around the base, far wall first, lid-carrier last.
const WALLS: { key: WallKey; o: XZ; at: number }[] = [
  { key: "left", o: [-1, 0], at: FOLDS[0] },
  { key: "face", o: [0, 1], at: FOLDS[1] },
  { key: "right", o: [1, 0], at: FOLDS[2] },
  { key: "back", o: [0, -1], at: FOLDS[3] },
];
const WALL: Record<WallKey, (typeof WALLS)[number]> = {
  left: WALLS[0],
  face: WALLS[1],
  right: WALLS[2],
  back: WALLS[3],
};
const BASE_QUAD: XZ[] = [[-0.5, -0.5], [0.5, -0.5], [0.5, 0.5], [-0.5, 0.5]];
const LID_PIVOT: V3 = [0, FLOOR, -1.5];

/** An arm's flat quad from `k0` to `k1` units out along `o`: hinge edge first. */
function armQuad(o: XZ, k0: number, k1: number): XZ[] {
  const [ox, oz] = o;
  const c = (k: number, side: number): XZ => [k * ox - side * oz, k * oz + side * ox];
  return [c(k0, 0.5), c(k0, -0.5), c(k1, -0.5), c(k1, 0.5)];
}

// Hinge creases, each drawn from the end a pen reaches first. `on` is the
// panel whose surface carries the crease.
const CREASES: { a: XZ; b: XZ; s: number; wall: WallKey | "lid"; on: PanelKey }[] = [
  { a: [-0.5, 0.5], b: [-0.5, -0.5], s: 1, wall: "left", on: "base" },
  { a: [-0.5, 0.5], b: [0.5, 0.5], s: 1, wall: "face", on: "base" },
  { a: [-0.5, -0.5], b: [0.5, -0.5], s: 2, wall: "back", on: "base" },
  { a: [-0.5, -1.5], b: [0.5, -1.5], s: 3, wall: "lid", on: "back" },
  { a: [0.5, 0.5], b: [0.5, -0.5], s: 4, wall: "right", on: "base" },
];

// Hinge angles. Positive folds a wall up and in; the lid's angle is relative
// to the back wall that carries it.
interface Angles {
  left: number;
  face: number;
  right: number;
  back: number;
  lid: number;
}

/** Wind-up: every wall dips a few degrees below the floor on b1.75. */
const crouch = (lt: number): number => -6 * DEG * outCubic(progress(0.72, T_CROUCH, lt));
/** Ease-in that dips once more before it commits; arrives at slope 4. */
const rise = (u: number): number => 2 * u * u * u - u * u;

function foldAngle(lt: number, at: number): number {
  if (lt < at) return lerp(crouch(lt), Q, rise(progress(at - LEAD, at, lt)));
  // Bounce inward only, and settle before the next wall lands, so a wall
  // never leans out and opens a gap at its neighbour's corner.
  const d = lt - at;
  if (d < 0.072) return Q + 11 * DEG * Math.sin((Math.PI * d) / 0.072);
  if (d < 0.114) return Q + 2 * DEG * Math.sin((Math.PI * (d - 0.072)) / 0.042);
  return Q;
}

// Velocity-continuous keys: [time, value, velocity per second].
type HKey = readonly [time: number, value: number, velocity: number];
function hermite(frames: readonly HKey[], t: number): number {
  if (t <= frames[0][0]) return frames[0][1];
  for (let i = 1; i < frames.length; i++) {
    const [t1, p1, m1] = frames[i];
    if (t > t1) continue;
    const [t0, p0, m0] = frames[i - 1];
    const h = t1 - t0;
    const s = (t - t0) / h;
    const s2 = s * s;
    const s3 = s2 * s;
    return (
      (2 * s3 - 3 * s2 + 1) * p0 +
      (s3 - 2 * s2 + s) * h * m0 +
      (-2 * s3 + 3 * s2) * p1 +
      (s3 - s2) * h * m1
    );
  }
  return frames[frames.length - 1][1];
}

// The lid lags back as its wall swings up, eases into the peak just after
// the wall lands, hangs, then whips over and slams on b3.
const LID: HKey[] = [
  [FOLDS[3] - LEAD, 0, 0],
  [1.26, -16, -200],
  [1.305, -25, 0],
  [T_SLAM, 90, 2700],
];
function lidAngle(lt: number): number {
  if (lt < T_SLAM) return hermite(LID, lt) * DEG;
  const d = lt - T_SLAM;
  // Two quick hops off the rim, small enough that its outline stays seated.
  if (d < 0.066) return Q - 3.2 * DEG * Math.sin((Math.PI * d) / 0.066);
  if (d < 0.1) return Q - 0.8 * DEG * Math.sin((Math.PI * (d - 0.066)) / 0.034);
  return Q;
}

function anglesAt(lt: number): Angles {
  return {
    left: foldAngle(lt, FOLDS[0]),
    face: foldAngle(lt, FOLDS[1]),
    right: foldAngle(lt, FOLDS[2]),
    back: foldAngle(lt, FOLDS[3]),
    lid: lidAngle(lt),
  };
}

/** Where a flat point (or, with `vector`, a direction) on a panel is now. */
function carry(a: Angles, key: PanelKey, p: V3, vector = false): V3 {
  if (key === "base") return p;
  const w = WALL[key === "lid" ? "back" : key];
  const axis: V3 = [-w.o[1], 0, w.o[0]];
  const O: V3 = [0, 0, 0];
  const q = key === "lid" ? rotateAround(p, vector ? O : LID_PIVOT, axis, a.lid) : p;
  return rotateAround(q, vector ? O : [0.5 * w.o[0], FLOOR, 0.5 * w.o[1]], axis, a[w.key]);
}

/** A two-frame brightening of the lid as it seats, then nothing. */
const slamFlash = (lt: number): number =>
  lt < T_SLAM ? 0 : (1 - progress(T_SLAM, T_SLAM + 0.05, lt)) ** 2;

/**
 * The corner whose perspective scale sets a panel's outline width: the one
 * drawBox uses for the same face once the box is closed, so widths match.
 */
const widthCorner = (key: PanelKey) => (key === "lid" ? 1 : key === "base" ? 0 : 3);

const hingeA = (angle: number) => smoothstep(8 * DEG, 30 * DEG, Math.abs(angle));
const DOWN: V3 = [0, -1, 0];

interface Panel {
  key: PanelKey;
  pts: V3[];
  /** Outer (printed) side normal. */
  n: V3;
  /** Outline alpha per edge i → i+1. */
  edges: number[];
  /** Warm rim along the free edge while the panel is in flight. */
  rim: number;
  /** Tone boost (the lid's flash as it seats). */
  lift: number;
  /** 0..1 darkening of the kraft side as the walls close around it. */
  shade: number;
}

function buildPanels(a: Angles, lt: number, squash: number): Panel[] {
  // The inside of the box falls into shadow as the walls close around it,
  // so the floor never flashes light between the rim's outlines.
  let up = 0;
  for (const w of WALLS) up += smoothstep(30 * DEG, 90 * DEG, a[w.key]) / 4;
  const enclosed = smoothstep(0.2, 1, up);
  const panels: Panel[] = [
    {
      key: "base",
      pts: BASE_QUAD.map(onFloor),
      n: DOWN,
      edges: [hingeA(a.back), hingeA(a.right), hingeA(a.face), hingeA(a.left)],
      rim: 0,
      lift: 0,
      shade: 0.85 * enclosed,
    },
  ];
  for (const w of WALLS) {
    const ang = a[w.key];
    panels.push({
      key: w.key,
      pts: armQuad(w.o, 0.5, 1.5).map((p) => carry(a, w.key, onFloor(p))),
      n: carry(a, w.key, DOWN, true),
      edges: [hingeA(ang), 1, w.key === "back" ? hingeA(a.lid) : 1, 1],
      rim: smoothstep(10 * DEG, 35 * DEG, ang) * (1 - smoothstep(w.at, w.at + 0.08, lt)),
      lift: 0,
      shade: 0.4 * enclosed,
    });
  }
  panels.push({
    key: "lid",
    pts: armQuad(WALL.back.o, 1.5, 2.5).map((p) => carry(a, "lid", onFloor(p))),
    n: carry(a, "lid", DOWN, true),
    edges: [hingeA(a.lid), 1, 1, 1],
    rim: smoothstep(FOLDS[3] - 0.05, FOLDS[3], lt) * (1 - smoothstep(T_SLAM - 0.03, T_SLAM, lt)),
    lift: 0.3 * slamFlash(lt),
    shade: 0,
  });
  if (squash !== 1) {
    const wide = 1 / Math.sqrt(squash);
    for (const p of panels) {
      p.pts = p.pts.map(([x, y, z]) => [x * wide, FLOOR + (y - FLOOR) * squash, z * wide]);
    }
  }
  return panels;
}

// Flood: one clock `D` (distance travelled by the front) drives every panel.
// The base wipes diagonally away from the corner where the pens met. An arm
// floods only where that diagonal, carried on across its hinge, has passed
// and where its own straight wipe out from the hinge has too, so card never
// shows beyond a black gap: the front crosses each crease as one diagonal and
// straightens as it travels out. The lid carries on from its wall.
interface Wipe {
  key: PanelKey;
  quad: XZ[];
  o: XZ;
  dir: XZ;
  d0: number;
}
const BASE_DIR: XZ = [-R2, R2];
const reach = (p: XZ) => (p[0] - CLOSE_AT[0]) * BASE_DIR[0] + (p[1] - CLOSE_AT[1]) * BASE_DIR[1];
const along = (w: Wipe, p: XZ) => (p[0] - w.o[0]) * w.dir[0] + (p[1] - w.o[1]) * w.dir[1];
/**
 * How far the diagonal runs past a hinge's nearer end before the arm's own
 * straight wipe starts. Arms the diagonal reaches along their length need
 * √2 − 1 for the front to finish straight; the two that meet at the pens'
 * corner start almost with the base.
 */
const armLag = (o: XZ) => (o[0] * BASE_DIR[0] + o[1] * BASE_DIR[1] > 0 ? Math.SQRT2 - 1 : 0.2);
const ARM_WIPES: Wipe[] = WALLS.map((w) => {
  const quad = armQuad(w.o, 0.5, 1.5);
  const d0 = Math.min(reach(quad[0]), reach(quad[1])) + armLag(w.o);
  return { key: w.key, quad, o: quad[0], dir: w.o, d0 };
});
const WIPES: Wipe[] = [
  { key: "base", quad: BASE_QUAD, o: CLOSE_AT, dir: BASE_DIR, d0: 0 },
  ...ARM_WIPES,
  {
    key: "lid",
    quad: armQuad(WALL.back.o, 1.5, 2.5),
    o: [0, -1.5],
    dir: WALL.back.o,
    d0: ARM_WIPES[3].d0 + 1,
  },
];
const WIPE = Object.fromEntries(WIPES.map((w) => [w.key, w])) as Record<PanelKey, Wipe>;

/** Front travel at which a flat point on panel `key` turns to card. */
function dpOf(key: PanelKey, p: XZ): number {
  const w = WIPE[key];
  return Math.max(w.d0 + along(w, p), reach(p));
}

/** When each panel's front appears and when it has swept the whole panel. */
const SPAN = Object.fromEntries(
  WIPES.map((w) => {
    let lo = Infinity;
    let hi = -Infinity;
    const [a, b, , d] = w.quad;
    for (let i = 0; i <= 20; i++) {
      for (let j = 0; j <= 20; j++) {
        const p: XZ = [
          a[0] + ((b[0] - a[0]) * i) / 20 + ((d[0] - a[0]) * j) / 20,
          a[1] + ((b[1] - a[1]) * i) / 20 + ((d[1] - a[1]) * j) / 20,
        ];
        const dp = dpOf(w.key, p);
        lo = Math.min(lo, dp);
        hi = Math.max(hi, dp);
      }
    }
    return [w.key, { lo, hi }];
  }),
) as Record<PanelKey, { lo: number; hi: number }>;
/** The front's travel when every edge has fully turned dark. */
const D_END = Math.max(...WIPES.map((w) => SPAN[w.key].hi)) + 0.12;
const FLOOD_EASE = cubicBezier(0.25, 0.45, 0.6, 1);
const floodD = (lt: number): number =>
  lt < T_CLOSE ? -1 : D_END * FLOOD_EASE(progress(T_CLOSE, T_FLOODED, lt));

/** A flat polygon whose edge i → i+1 is a moving front when `front[i]`. */
interface Patch {
  pts: XZ[];
  front: boolean[];
}

/** Keep the part of `patch` where `f` ≤ 0; the new edge along f = 0 is a front. */
function clipPatch(patch: Patch, f: (p: XZ) => number): Patch {
  const pts: XZ[] = [];
  const front: boolean[] = [];
  const n = patch.pts.length;
  for (let i = 0; i < n; i++) {
    const p = patch.pts[i];
    const q = patch.pts[(i + 1) % n];
    const fp = f(p);
    const fq = f(q);
    if (fp <= 0) {
      pts.push(p);
      front.push(patch.front[i]);
      if (fq > 0) {
        // Leaving: the edge from here to where it comes back runs along the cut.
        pts.push(mixXZ(p, q, fp / (fp - fq)));
        front.push(true);
      }
    } else if (fq < 0) {
      pts.push(mixXZ(p, q, fp / (fp - fq)));
      front.push(patch.front[i]);
    }
  }
  return { pts, front };
}

/** The part of panel `key` the front has turned to card at travel `D`. */
function floodedPart(key: PanelKey, D: number): Patch {
  const w = WIPE[key];
  let patch: Patch = { pts: [...w.quad], front: w.quad.map(() => false) };
  if (D < SPAN[key].hi) {
    patch = clipPatch(patch, (p) => reach(p) - D);
    if (key !== "base") patch = clipPatch(patch, (p) => w.d0 + along(w, p) - D);
  }
  return patch;
}

const inRect = (q: readonly XZ[], p: XZ) => {
  const xs = q.map((v) => v[0]);
  const zs = q.map((v) => v[1]);
  const e = 1e-6;
  return (
    p[0] >= Math.min(...xs) - e &&
    p[0] <= Math.max(...xs) + e &&
    p[1] >= Math.min(...zs) - e &&
    p[1] <= Math.max(...zs) + e
  );
};
/** The arm each pen segment outlines. */
const OWNERS: PanelKey[][] = [PATH_A, PATH_B].map((path) =>
  path.slice(0, -1).map((p, i) => {
    const m = mixXZ(p, path[i + 1], 0.5);
    return WIPES.find((w) => w.key !== "base" && inRect(w.quad, m))?.key ?? "base";
  }),
);

// Kraft for the unprinted inner side: the logo ramp, duller, and kept well
// above the background so a far wall's inner face reads while it stands.
const KRAFT: [number, string, string][] = [
  [0, "#7a5222", "#6b4719"],
  [0.5, "#a0702f", "#8c6026"],
  [1, "#c3955a", "#ad8046"],
];

const KRAFT_SHADOW = "#2c1c08";

function kraftFill(
  ctx: CanvasRenderingContext2D,
  pts: Projected[],
  t: number,
  shade = 0,
): CanvasGradient {
  const k = clamp(t);
  const i = k <= 0.5 ? 1 : 2;
  const [t0, a0, b0] = KRAFT[i - 1];
  const [t1, a1, b1] = KRAFT[i];
  const p = (k - t0) / (t1 - t0);
  let x0 = Infinity;
  let y0 = Infinity;
  let x1 = -Infinity;
  let y1 = -Infinity;
  for (const q of pts) {
    x0 = Math.min(x0, q.x);
    y0 = Math.min(y0, q.y);
    x1 = Math.max(x1, q.x);
    y1 = Math.max(y1, q.y);
  }
  const g = ctx.createLinearGradient(x0, y0, x1, y1);
  g.addColorStop(0, mix(mixRGB(a0, a1, p), KRAFT_SHADOW, shade));
  g.addColorStop(1, mix(mixRGB(b0, b1, p), KRAFT_SHADOW, shade));
  return g;
}

const center = (pts: V3[]): V3 => {
  const c: V3 = [0, 0, 0];
  for (const p of pts) {
    c[0] += p[0] / pts.length;
    c[1] += p[1] / pts.length;
    c[2] += p[2] / pts.length;
  }
  return c;
};

/** Printed cardboard when the outer side faces the lens, kraft otherwise. */
function panelFill(
  ctx: CanvasRenderingContext2D,
  view: View,
  pts: V3[],
  n: V3,
  sp: Projected[],
  lift = 0,
  shade = 0,
): CanvasGradient {
  return view.facing(n, center(pts))
    ? cardboardFill(ctx, sp, tone(view, n) + lift)
    : kraftFill(ctx, sp, tone(view, [-n[0], -n[1], -n[2]]) + lift, shade);
}

/** One flat colour for a panel's visible side (the middle of its gradient). */
function panelTint(view: View, pts: V3[], n: V3, shade = 0): RGB {
  if (view.facing(n, center(pts))) {
    const [c0, c1] = cardboard(tone(view, n));
    return mixRGB(c0, c1, 0.5);
  }
  const k = clamp(tone(view, [-n[0], -n[1], -n[2]]));
  const i = k <= 0.5 ? 1 : 2;
  const [t0, a0, b0] = KRAFT[i - 1];
  const [t1, a1, b1] = KRAFT[i];
  const mid = mixRGB(mixRGB(a0, a1, (k - t0) / (t1 - t0)), mixRGB(b0, b1, (k - t0) / (t1 - t0)), 0.5);
  return mixRGB(mid, KRAFT_SHADOW, shade);
}

interface Smear {
  key: PanelKey;
  /** The band the free edge swept: its two ends' arcs and the edge now and then. */
  pts: V3[];
  /** Middles of the free edge now and at the shutter's opening. */
  now: V3;
  then: V3;
  alpha: number;
}

// Motion blur: the area a panel's free edge swept while the shutter was open,
// as one shape with its corners' arcs, filled in the panel's own colour and
// fading back along the motion. Impacts stay crisp: the shutter never opens
// before the moment a panel lands, so the landing frame has no smear.
const SHUTTER = 0.016;
const ARC_STEPS = 4;
function motionSmears(lt: number, a: Angles, squash: number): Smear[] {
  const lands = (k: WallKey | "lid"): number[] =>
    k === "lid" ? [FOLDS[3], T_SLAM] : [WALL[k].at];
  const opened = (k: WallKey | "lid"): number => {
    let t0 = lt - SHUTTER;
    for (const at of lands(k)) if (lt >= at) t0 = Math.max(t0, at);
    return t0;
  };
  const turn = (x: Angles, k: WallKey | "lid") => (k === "lid" ? x.back + x.lid : x[k]);
  const out: Smear[] = [];
  for (const k of ["left", "face", "right", "back", "lid"] as const) {
    const t0 = opened(k);
    if (t0 >= lt - 1e-4) continue;
    const w = smoothstep(0.12, 0.3, Math.abs(turn(a, k) - turn(anglesAt(t0), k)));
    if (w <= 0) continue;
    const at: V3[][] = [];
    for (let j = 0; j <= ARC_STEPS; j++) {
      const t = lerp(lt, t0, j / ARC_STEPS);
      at.push(buildPanels(j === 0 ? a : anglesAt(t), t, squash).find((p) => p.key === k)!.pts);
    }
    const old = at[ARC_STEPS];
    // The free edge now, one corner's arc back, the free edge then, and the
    // other corner's arc forward again.
    const pts: V3[] = [];
    for (let j = 0; j <= ARC_STEPS; j++) pts.push(at[j][3]);
    for (let j = ARC_STEPS; j >= 0; j--) pts.push(at[j][2]);
    out.push({
      key: k,
      pts,
      now: mix3(at[0][2], at[0][3], 0.5),
      then: mix3(old[2], old[3], 0.5),
      alpha: 0.45 * w,
    });
  }
  return out;
}

/** Painter's order: the base first (it is the floor), then far to near. */
function paintOrder(view: View, panels: Panel[]): Panel[] {
  return panels
    .map((p) => ({ p, depth: p.key === "base" ? -Infinity : view.project(center(p.pts)).z }))
    .sort((a, b) => a.depth - b.depth)
    .map((x) => x.p);
}

function drawPanels(
  ctx: CanvasRenderingContext2D,
  view: View,
  panels: Panel[],
  smears: Smear[],
  decorate: (key: PanelKey) => void,
): void {
  const lw = OUTLINE_RATIO * view.cam.scale;
  const items = paintOrder(view, panels).map((p) => ({ p, sp: p.pts.map((q) => view.project(q)) }));
  ctx.save();
  ctx.lineJoin = "round";
  ctx.lineCap = "round";
  for (const { p, sp } of items) {
    const sm = smears.find((m) => m.key === p.key);
    if (sm) {
      const a = view.project(sm.now);
      const b = view.project(sm.then);
      if (Math.hypot(b.x - a.x, b.y - a.y) > 1) {
        const tint = panelTint(view, p.pts, p.n, p.shade);
        const g = ctx.createLinearGradient(a.x, a.y, b.x, b.y);
        g.addColorStop(0, rgba(tint, sm.alpha));
        g.addColorStop(1, rgba(tint, 0));
        polygon(ctx, sm.pts.map((q) => view.project(q)));
        ctx.fillStyle = g;
        ctx.fill();
      }
    }
    polygon(ctx, sp);
    const fill = panelFill(ctx, view, p.pts, p.n, sp, p.lift, p.shade);
    ctx.fillStyle = fill;
    ctx.fill();
    if (p.key === "base" || p.key === "back") {
      // Cover the hairline seams along the hinges it shares.
      ctx.strokeStyle = fill;
      ctx.lineWidth = 1;
      ctx.stroke();
    }
    decorate(p.key);
    ctx.strokeStyle = OUTLINE;
    const width = lw * sp[widthCorner(p.key)].f;
    ctx.lineWidth = width;
    ctx.beginPath();
    let any = false;
    for (let i = 0; i < 4; i++) {
      if (p.edges[i] < 0.999) continue;
      const a = sp[i];
      const b = sp[(i + 1) % 4];
      ctx.moveTo(a.x, a.y);
      ctx.lineTo(b.x, b.y);
      any = true;
    }
    if (any) ctx.stroke();
    for (let i = 0; i < 4; i++) {
      const e = p.edges[i];
      if (e >= 0.999 || e <= 0.001) continue;
      const a = sp[i];
      const b = sp[(i + 1) % 4];
      ctx.save();
      ctx.globalAlpha *= e;
      ctx.beginPath();
      ctx.moveTo(a.x, a.y);
      ctx.lineTo(b.x, b.y);
      ctx.stroke();
      ctx.restore();
    }
    if (p.rim > 0) drawRim(ctx, sp, width, p.rim);
  }
  ctx.restore();
}

/**
 * Light catching a panel's free edge in flight: a thin bevel on the card just
 * inside the outline, tapered at both ends so it never ends in a hard cap.
 */
function drawRim(ctx: CanvasRenderingContext2D, sp: Projected[], width: number, rim: number): void {
  const a = sp[2];
  const b = sp[3];
  const cx = (sp[0].x + sp[1].x + sp[2].x + sp[3].x) / 4;
  const cy = (sp[0].y + sp[1].y + sp[2].y + sp[3].y) / 4;
  const len = Math.hypot(b.x - a.x, b.y - a.y);
  if (len < 1) return;
  let nx = -(b.y - a.y) / len;
  let ny = (b.x - a.x) / len;
  if (nx * (cx - (a.x + b.x) / 2) + ny * (cy - (a.y + b.y) / 2) < 0) {
    nx = -nx;
    ny = -ny;
  }
  // Inward, and in from each corner by the outline's half width.
  const off = width / 2 + 1;
  const tx = ((b.x - a.x) / len) * (width / 2);
  const ty = ((b.y - a.y) / len) * (width / 2);
  const x0 = a.x + nx * off + tx;
  const y0 = a.y + ny * off + ty;
  const x1 = b.x + nx * off - tx;
  const y1 = b.y + ny * off - ty;
  const g = ctx.createLinearGradient(x0, y0, x1, y1);
  const c = PALETTE.amberBright;
  g.addColorStop(0, rgba(c, 0));
  g.addColorStop(0.25, rgba(c, 0.7 * rim));
  g.addColorStop(0.75, rgba(c, 0.7 * rim));
  g.addColorStop(1, rgba(c, 0));
  ctx.save();
  ctx.globalCompositeOperation = "lighter";
  ctx.strokeStyle = g;
  ctx.lineWidth = 2.2;
  ctx.lineCap = "butt";
  ctx.beginPath();
  ctx.moveTo(x0, y0);
  ctx.lineTo(x1, y1);
  ctx.stroke();
  ctx.restore();
}

/**
 * The flood's card, with a hot band and a crisp paper line at each front.
 * Creases are drawn in the same order as drawPanels draws them, so the
 * hand-over to drawPanels is seamless.
 */
function drawFlood(
  ctx: CanvasRenderingContext2D,
  view: View,
  D: number,
  a: Angles,
  panels: Panel[],
  decorate: (key: PanelKey) => void,
): void {
  for (const w of paintOrder(view, panels).map((p) => WIPE[p.key])) {
    ctx.save();
    floodPatch(ctx, view, D, a, panels, w);
    ctx.restore();
    decorate(w.key);
  }
}

/** One panel's flooded part, and its front while it is moving. */
function floodPatch(
  ctx: CanvasRenderingContext2D,
  view: View,
  D: number,
  a: Angles,
  panels: Panel[],
  w: Wipe,
): void {
  const span = SPAN[w.key];
  if (D <= span.lo) return;
  const panel = panels.find((p) => p.key === w.key)!;
  const full = panel.pts.map((q) => view.project(q));
  const fill = panelFill(ctx, view, panel.pts, panel.n, full);
  const { pts, front } = floodedPart(w.key, D);
  if (pts.length < 3) return;
  const at = (p: XZ) => view.project(carry(a, w.key, onFloor(p)));
  const sp = pts.map(at);
  polygon(ctx, sp);
  ctx.fillStyle = fill;
  ctx.fill();
  if (w.key === "base" || w.key === "back") {
    // Cover the hairline seams along the hinges it shares.
    ctx.strokeStyle = fill;
    ctx.lineWidth = 1;
    ctx.stroke();
  }
  const on = smoothstep(span.lo, span.lo + 0.05, D) * (1 - smoothstep(span.hi - 0.12, span.hi, D));
  if (on <= 0 || !front.some(Boolean)) return;
  // The front as one path, so a kinked front strokes without doubling up.
  const frontPath = () => {
    ctx.beginPath();
    for (let i = 0; i < sp.length; i++) {
      if (!front[i]) continue;
      const q = sp[(i + 1) % sp.length];
      ctx.moveTo(sp[i].x, sp[i].y);
      ctx.lineTo(q.x, q.y);
    }
  };
  ctx.save();
  ctx.globalCompositeOperation = "lighter";
  // Heat behind each front edge, fading back into the card. Where a kinked
  // front's two bands overlap the corner runs a little hotter.
  let cx = 0;
  let cz = 0;
  for (const p of pts) {
    cx += p[0] / pts.length;
    cz += p[1] / pts.length;
  }
  for (let i = 0; i < pts.length; i++) {
    if (!front[i]) continue;
    const p = pts[i];
    const q = pts[(i + 1) % pts.length];
    const len = Math.hypot(q[0] - p[0], q[1] - p[1]);
    if (len < 1e-4) continue;
    let nx = -(q[1] - p[1]) / len;
    let nz = (q[0] - p[0]) / len;
    const m: XZ = [(p[0] + q[0]) / 2, (p[1] + q[1]) / 2];
    if ((cx - m[0]) * nx + (cz - m[1]) * nz < 0) {
      nx = -nx;
      nz = -nz;
    }
    const g0 = at(m);
    const g1 = at([m[0] + nx * 0.28, m[1] + nz * 0.28]);
    const g = ctx.createLinearGradient(g0.x, g0.y, g1.x, g1.y);
    g.addColorStop(0, rgba(PALETTE.amberBright, 0.5 * on));
    g.addColorStop(0.4, rgba(PALETTE.amberBright, 0.16 * on));
    g.addColorStop(1, rgba(PALETTE.amberBright, 0));
    polygon(ctx, sp);
    ctx.fillStyle = g;
    ctx.fill();
  }
  ctx.lineCap = "round";
  ctx.lineJoin = "round";
  frontPath();
  ctx.strokeStyle = rgba(PALETTE.paper, 0.95 * on);
  ctx.lineWidth = 3;
  ctx.stroke();
  ctx.restore();
}

// The outline as short pieces, each knowing how far the flood has turned it
// from pen light into the dark cardboard outline. Pieces run from pen B's tip
// back to the start corner and on along pen A, so the start corner is a join.
interface Piece {
  p0: Projected;
  p1: Projected;
  key: PanelKey;
  k: number;
}
const PIECES = 6;

function outlinePieces(view: View, a: Angles, s: number, D: number): Piece[] {
  const out: Piece[] = [];
  const add = (pi: number, u0: number, u1: number) => {
    const path = pi === 0 ? PATH_A : PATH_B;
    const key = OWNERS[pi][Math.min(Math.floor(Math.min(u0, u1)), 6)];
    const kRaw = D < 0 ? 0 : smoothstep(0, 0.1, D - dpOf(key, pathAt(path, (u0 + u1) / 2)));
    out.push({
      p0: view.project(carry(a, key, onFloor(pathAt(path, u0)))),
      p1: view.project(carry(a, key, onFloor(pathAt(path, u1)))),
      key,
      k: Math.round(kRaw * 10) / 10,
    });
  };
  const pieces = (i: number) => {
    const L = Math.min(i + 1, s) - i;
    return { L, n: Math.max(1, Math.ceil(L * PIECES)) };
  };
  for (let i = Math.min(6, Math.ceil(s) - 1); i >= 0; i--) {
    const { L, n } = pieces(i);
    for (let j = n; j > 0; j--) add(1, i + (L * j) / n, i + (L * (j - 1)) / n);
  }
  for (let i = 0; i < 7 && i < s; i++) {
    const { L, n } = pieces(i);
    for (let j = 0; j < n; j++) add(0, i + (L * j) / n, i + (L * (j + 1)) / n);
  }
  return out;
}

/** Runs of consecutive pieces that share a style; a loop's ends may merge. */
function runs(pieces: Piece[], same: (a: Piece, b: Piece) => boolean, loop: boolean) {
  const out: Piece[][] = [];
  for (const pc of pieces) {
    const last = out[out.length - 1];
    if (last && same(last[0], pc)) last.push(pc);
    else out.push([pc]);
  }
  if (loop && out.length > 1 && same(out[out.length - 1][0], out[0][0])) {
    out[0] = out.pop()!.concat(out[0]);
  }
  return out.map((run) => ({ run, closed: loop && out.length === 1 }));
}

function strokeRun(ctx: CanvasRenderingContext2D, run: Piece[], closed: boolean): void {
  ctx.beginPath();
  ctx.moveTo(run[0].p0.x, run[0].p0.y);
  for (const pc of run) ctx.lineTo(pc.p1.x, pc.p1.y);
  if (closed) ctx.closePath();
  ctx.stroke();
}

function strokeFloor(ctx: CanvasRenderingContext2D, view: View, pts: XZ[]): void {
  ctx.beginPath();
  pts.forEach((p, i) => {
    const q = view.project(onFloor(p));
    if (i === 0) ctx.moveTo(q.x, q.y);
    else ctx.lineTo(q.x, q.y);
  });
  ctx.stroke();
}

function drawOutline(
  ctx: CanvasRenderingContext2D,
  view: View,
  lt: number,
  D: number,
  a: Angles,
  widthOf: (key: PanelKey) => number,
): void {
  if (lt <= T_LAUNCH) return;
  const s = penS(lt);
  const pieces = outlinePieces(view, a, s, D);
  if (!pieces.length) return;
  const loop = s >= 7;
  const burst = lt >= T_CLOSE ? Math.exp(-(lt - T_CLOSE) / 0.07) : 0;
  ctx.save();
  ctx.lineJoin = "round";
  // The dark outline swells in behind the front...
  ctx.lineCap = "round";
  ctx.strokeStyle = OUTLINE;
  for (const { run, closed } of runs(pieces, (p, q) => p.key === q.key && p.k === q.k, loop)) {
    if (run[0].k <= 0) continue;
    ctx.lineWidth = widthOf(run[0].key) * run[0].k;
    strokeRun(ctx, run, closed);
  }
  // ...along the same centreline the pen light is fading from.
  ctx.globalCompositeOperation = "lighter";
  ctx.lineCap = "butt";
  for (const { run, closed } of runs(pieces, (p, q) => p.k === q.k, loop)) {
    const l = 1 - run[0].k;
    if (l <= 0) continue;
    // Soft falloff under the hot core: nine nested bands, each adding a
    // little, so the light fades out without visible steps.
    ctx.strokeStyle = rgba(PALETTE.amber, 0.036 * (1 + 1.2 * burst) * l);
    for (let i = 8; i >= 0; i--) {
      ctx.lineWidth = 5.5 * 1.25 ** i;
      strokeRun(ctx, run, closed);
    }
    ctx.strokeStyle = rgba(PALETTE.amberBright, (0.85 + 0.15 * burst) * l);
    ctx.lineWidth = 3.8 + 2 * burst;
    strokeRun(ctx, run, closed);
  }
  // White-hot tail cooling behind each tip.
  if (lt < T_CLOSE + 0.1) {
    ctx.lineCap = "round";
    const L = 1.1;
    const N = 8;
    const heat = 1 - progress(T_CLOSE, T_CLOSE + 0.1, lt);
    for (const path of [PATH_A, PATH_B]) {
      for (let i = 0; i < N; i++) {
        const s1 = s - (L * i) / N;
        const s0 = s - (L * (i + 1)) / N;
        if (s1 <= 0) break;
        const k = 1 - i / N;
        ctx.strokeStyle = rgba(PALETTE.paper, 0.7 * k * heat);
        ctx.lineWidth = 2 + 4 * k;
        strokeFloor(ctx, view, pathSpan(path, Math.max(0, s0), s1));
      }
    }
  }
  ctx.restore();
}

// Creases are dashed by hand in world units, so each dash can cross-fade
// from pen light to scored card as the flood reaches it, and the pattern
// never shifts when perspective changes.
const DASH = 0.085;
const GAP = 0.06;
function drawCreases(
  ctx: CanvasRenderingContext2D,
  view: View,
  lt: number,
  D: number,
  a: Angles,
  on: PanelKey | null,
): void {
  const S = view.cam.scale;
  const score = pulse(lt, T_CROUCH, 0.03, 0.07);
  ctx.save();
  ctx.lineCap = "butt";
  for (const c of CREASES) {
    if (on && c.on !== on) continue;
    const drawn = outCubic(progress(penT(c.s) + 0.015, penT(c.s) + 0.2, lt));
    if (drawn <= 0) continue;
    const fold =
      c.wall === "lid" ? Math.max(Math.abs(a.back), Math.abs(a.lid)) : Math.abs(a[c.wall]);
    const fade = 1 - hingeA(fold);
    if (fade <= 0) continue;
    for (let t0 = 0; t0 < drawn; t0 += DASH + GAP) {
      const t1 = Math.min(t0 + DASH, drawn);
      const pa = view.project(carry(a, c.on, onFloor(mixXZ(c.a, c.b, t0))));
      const pb = view.project(carry(a, c.on, onFloor(mixXZ(c.a, c.b, t1))));
      const k = D < 0 ? 0 : smoothstep(0, 0.1, D - dpOf(c.on, mixXZ(c.a, c.b, (t0 + t1) / 2)));
      ctx.beginPath();
      ctx.moveTo(pa.x, pa.y);
      ctx.lineTo(pb.x, pb.y);
      if (k > 0) {
        ctx.globalCompositeOperation = "source-over";
        ctx.strokeStyle = rgba(OUTLINE, 0.7 * k * fade);
        ctx.lineWidth = 0.022 * S * pa.f;
        ctx.stroke();
      }
      ctx.globalCompositeOperation = "lighter";
      const light = 0.8 * (1 - k) + 0.9 * score;
      if (light > 0) {
        ctx.strokeStyle = rgba(k < 1 ? PALETTE.amber : PALETTE.amberBright, light * fade);
        ctx.lineWidth = 2.5 + 1.5 * score;
        ctx.stroke();
      }
    }
  }
  ctx.restore();
}

// Camera: a close-up on the spark pulls back and spins to frame the net
// top-down, holds that blueprint view while the net floods with only a slight
// lean, then swings up and around with the folds, rising to give the upright
// lid headroom. Hermite keys keep velocity continuous across the phases.
const PITCH: HKey[] = [
  [0, 90, 0],
  [0.5, 90, 0],
  [0.86, 81, -45],
  [FOLDS[0], 76, -95],
  [FOLDS[2], 50, -95],
  [FOLDS[3], 40, -60],
  [1.5, 32.5, -18],
  [CAM_END, 30, 0],
];
// The net spins in while it is drawn and is already near the logo's 45° by
// the first fold, so every wall folds diagonally to the lens and reads.
const YAW: HKey[] = [
  [0, 125, -220],
  [T_CLOSE, 62, -40],
  [FOLDS[0], 52, -15],
  [1.3, 46.5, -8],
  [CAM_END, 45, 0],
];
const SCALE: HKey[] = [
  [0, 600, -600],
  [T_LAUNCH, 540, -1900],
  [0.3, 250, -300],
  [0.6, 212, 0],
  [FOLDS[0], 216, 25],
  [1.35, 256, 60],
  [CAM_END, 280, 0],
];
/** Target rise in world units: frames the box low while the lid stands. */
const LIFT: HKey[] = [
  [0.95, 0, 0],
  [1.3, 0.24, 0],
  [CAM_END, 0, 0],
];

/**
 * The floor point under the centre of the screen-aligned bounding box of
 * everything the pens have drawn by `s`, for a camera at `yaw` (treated as
 * orthographic, which is close enough to frame by). Pitch only scales screen
 * y, so it does not move the box's centre.
 */
function drawnCenter(s: number, yaw: number): XZ {
  const c = Math.cos(yaw);
  const n = Math.sin(yaw);
  let x0 = Infinity;
  let x1 = -Infinity;
  let y0 = Infinity;
  let y1 = -Infinity;
  const take = ([x, z]: XZ) => {
    const X = x * c - z * n;
    const Y = x * n + z * c;
    x0 = Math.min(x0, X);
    x1 = Math.max(x1, X);
    y0 = Math.min(y0, Y);
    y1 = Math.max(y1, Y);
  };
  for (const path of [PATH_A, PATH_B]) {
    for (let k = 0; k <= Math.min(Math.floor(s), 7); k++) take(path[k]);
    take(pathAt(path, s));
  }
  const X = (x0 + x1) / 2;
  const Y = (y0 + y1) / 2;
  return [c * X + n * Y, -n * X + c * Y];
}

// The operator keeps the drawing centred: the target follows the drawn
// outline's centre through a window that looks back further than ahead, so
// the lens leaves the spark gently and never snaps when the pens turn.
const FRAME_BACK = 0.2;
const FRAME_AHEAD = 0.06;
const FRAME_N = 24;
function framedXZ(lt: number, yaw: number): XZ {
  let x = 0;
  let z = 0;
  let wsum = 0;
  for (let i = 0; i <= FRAME_N; i++) {
    const u = i / FRAME_N;
    const tau = lt - FRAME_BACK + (FRAME_BACK + FRAME_AHEAD) * u;
    // Triangular weight peaking at `lt`.
    const w = (tau <= lt ? 1 - (lt - tau) / FRAME_BACK : 1 - (tau - lt) / FRAME_AHEAD) + 1e-3;
    const c = drawnCenter(penS(tau), yaw);
    x += c[0] * w;
    z += c[1] * w;
    wsum += w;
  }
  return [x / wsum, z / wsum];
}

function cameraAt(lt: number): Camera {
  if (lt >= CAM_END) return HERO_CAM;
  const home = swiftInOut(progress(0.8, CAM_END, lt));
  const ip = lerp(0.2, 0, smoothstep(FOLDS[0] - 0.1, CAM_END, lt));
  const yaw = hermite(YAW, lt) * DEG;
  const pitch = hermite(PITCH, lt) * DEG;
  const target = mix3(onFloor(framedXZ(lt, yaw)), [0, 0, 0], home);
  target[1] += hermite(LIFT, lt);
  return {
    cx: HERO_CAM.cx,
    cy: HERO_CAM.cy,
    scale: hermite(SCALE, lt),
    yaw,
    pitch,
    roll: 0,
    persp: ip > 1e-6 ? 1 / ip : 0,
    target,
  };
}

const TAPE_PULL = cubicBezier(0.3, 0, 0.25, 1);
/** Tape: a smooth pull across the lid, then a quick press down the side. */
function tapeAt(lt: number): number {
  const u = progress(T_TAPE0, T_TAPE1, lt);
  const knee = 0.7;
  if (u < knee) return 0.8 * TAPE_PULL(u / knee);
  return 0.8 + 0.2 * outCubic((u - knee) / (1 - knee));
}

/** Squash on the lid slam, recovered well before the handoff. */
function slamSquash(lt: number): number {
  const w = 1 - smoothstep(1.62, 1.74, lt);
  return 1 - 0.07 * wobble(lt, T_SLAM, 3.6, 8) * w;
}

// Sparks: a few flicked off the pen tips where they turn, and an uneven spray
// out of the corner where the pens collide. Each slides on the floor.
interface Spark {
  t0: number;
  at: XZ;
  dir: XZ;
  speed: number;
  life: number;
  /** Streak length, as seconds of travel behind the head. */
  tail: number;
  width: number;
  /** Distance out from `at` where it is born, so it never reads as a spoke. */
  r0: number;
}
let sparks: Spark[] | null = null;
function allSparks(): Spark[] {
  if (sparks) return sparks;
  const out: Spark[] = [];
  let seed = 1;
  for (const [path, turns] of [
    [PATH_A, TURNS_A],
    [PATH_B, TURNS_B],
  ] as const) {
    for (const k of turns) {
      const a = path[k - 1];
      const b = path[k];
      const base = Math.atan2(b[1] - a[1], b[0] - a[0]);
      // One or two fat sparks flung on along the old heading: big enough to
      // read at landing-page size.
      const n = 1 + Math.floor(hash(seed, 1) * 1.7);
      for (let j = 0; j < n; j++, seed++) {
        const ang = base + (hash(seed, 3) - 0.5) * 1.1;
        out.push({
          t0: penT(k),
          at: b,
          dir: [Math.cos(ang), Math.sin(ang)],
          speed: 3 + hash(seed, 5) * 2.2,
          life: 0.1 + hash(seed, 7) * 0.05,
          tail: 0.03,
          width: 5 + hash(seed, 9) * 1.5,
          r0: 0.03,
        });
      }
    }
  }
  // Away from the base, between the two arms that met.
  const away = Math.atan2(-1, 1);
  for (let j = 0; j < 10; j++) {
    const long = j < 3;
    const ang = away + (hash(j, 31) - 0.5) * (long ? 0.9 : 2.7);
    out.push({
      t0: T_CLOSE,
      at: CLOSE_AT,
      dir: [Math.cos(ang), Math.sin(ang)],
      speed: long ? 10 + hash(j, 33) * 4 : 3 + hash(j, 35) * 4,
      life: long ? 0.12 + hash(j, 37) * 0.05 : 0.06 + hash(j, 39) * 0.07,
      tail: long ? 0.03 : 0.012 + hash(j, 41) * 0.01,
      width: long ? 3.6 : 2.5 + hash(j, 43) * 2,
      r0: 0.05 + hash(j, 45) * 0.06,
    });
  }
  sparks = out;
  return out;
}

function sparkPos(s: Spark, d: number): V3 {
  const k = 7;
  const travel = s.r0 + (s.speed * (1 - Math.exp(-k * d))) / k;
  return onFloor([s.at[0] + s.dir[0] * travel, s.at[1] + s.dir[1] * travel]);
}

function drawSparks(ctx: CanvasRenderingContext2D, view: View, lt: number): void {
  ctx.save();
  ctx.globalCompositeOperation = "lighter";
  for (const s of allSparks()) {
    const d = lt - s.t0;
    if (d <= 0 || d >= s.life) continue;
    const k = 1 - d / s.life;
    const h = view.project(sparkPos(s, d));
    const t = view.project(sparkPos(s, Math.max(0, d - s.tail)));
    const len = Math.hypot(h.x - t.x, h.y - t.y);
    if (len > 0.5) {
      // Tapered streak: full width at the head, a point at the tail.
      const w = (s.width * (0.35 + 0.65 * k)) / 2;
      const nx = (-(h.y - t.y) / len) * w;
      const ny = ((h.x - t.x) / len) * w;
      ctx.fillStyle = rgba(PALETTE.amberBright, k);
      ctx.beginPath();
      ctx.moveTo(h.x + nx, h.y + ny);
      ctx.lineTo(t.x, t.y);
      ctx.lineTo(h.x - nx, h.y - ny);
      ctx.arc(h.x, h.y, w, Math.atan2(-ny, -nx), Math.atan2(ny, nx));
      ctx.fill();
    }
    glow(ctx, h.x, h.y, 5 + 9 * k, PALETTE.paper, 0.9 * k);
  }
  ctx.restore();
}

/** An anamorphic lens streak through a light. */
function streak(
  ctx: CanvasRenderingContext2D,
  x: number,
  y: number,
  len: number,
  alpha: number,
): void {
  if (alpha <= 0 || len <= 0) return;
  ctx.save();
  ctx.translate(x, y);
  ctx.scale(1, 0.045);
  glow(ctx, 0, 0, len, PALETTE.amberBright, alpha);
  ctx.restore();
}

/** A two-axis flare: the corner ping as a pen turns. */
function crossFlare(ctx: CanvasRenderingContext2D, x: number, y: number, size: number, alpha: number): void {
  if (alpha <= 0.01) return;
  glow(ctx, x, y, size * 0.55, PALETTE.paper, alpha);
  ctx.save();
  ctx.translate(x, y);
  ctx.scale(1, 0.09);
  glow(ctx, 0, 0, size * 1.7, PALETTE.amberBright, alpha);
  ctx.restore();
  ctx.save();
  ctx.translate(x, y);
  ctx.scale(0.09, 1);
  glow(ctx, 0, 0, size * 1.1, PALETTE.amberBright, alpha);
  ctx.restore();
}

function drawLights(ctx: CanvasRenderingContext2D, view: View, lt: number): void {
  const s = penS(lt);
  // Pen tips, kicking brighter each time they turn a corner.
  if (lt < T_CLOSE) {
    const ign = progress(0, 0.05, lt);
    for (const [path, turns] of [
      [PATH_A, TURNS_A],
      [PATH_B, TURNS_B],
    ] as const) {
      const q = view.project(onFloor(pathAt(path, s)));
      let kick = 0;
      for (const k of turns) kick += pulse(lt, penT(k), 0.006, 0.03);
      const r =
        (lt < T_LAUNCH ? 80 * outCubic(ign) : lerp(80, 64, progress(T_LAUNCH, 0.14, lt))) *
        (1 + 0.55 * kick);
      glow(ctx, q.x, q.y, r, PALETTE.amber, 0.9);
      glow(ctx, q.x, q.y, r * 0.32, PALETTE.paper, 1);
      // A flare pinned to each corner the pen has just turned.
      for (const k of turns) {
        const f = pulse(lt, penT(k), 0.004, 0.02);
        if (f <= 0.01) continue;
        const c = view.project(onFloor(path[k]));
        crossFlare(ctx, c.x, c.y, 56, f);
      }
    }
    // Ignition flare, strongest on the first frames.
    const q = view.project(IGNITE);
    const flare = lt < T_LAUNCH ? outCubic(ign) : Math.exp(-(lt - T_LAUNCH) / 0.08);
    glow(ctx, q.x, q.y, 190 * flare, PALETTE.amber, 0.55 * flare);
    streak(ctx, q.x, q.y, 520 * flare, 0.7 * flare);
  }
  // Closure: a flash and a lens streak where the pens meet.
  if (lt >= T_CLOSE && lt < T_CLOSE + 0.45) {
    const q = view.project(onFloor(CLOSE_AT));
    const d = lt - T_CLOSE;
    const k = Math.exp(-d / 0.07);
    glow(ctx, q.x, q.y, 110 + 140 * (1 - k), PALETTE.amberBright, k);
    glow(ctx, q.x, q.y, 40, PALETTE.paper, k);
    streak(ctx, q.x, q.y, 760 * (0.55 + 0.45 * k), 0.85 * k);
  }
}

/**
 * Once the flood has reached the lid's tip, a glint runs back down the net's
 * long axis and crosses the base on b1.75, as the creases score.
 */
const SHEEN_EASE = cubicBezier(0.35, 0, 0.55, 1);
function drawSheen(
  ctx: CanvasRenderingContext2D,
  view: View,
  lt: number,
  panels: Panel[],
  W: number,
  H: number,
): void {
  const u = progress(0.7, 0.93, lt);
  if (u <= 0 || u >= 1) return;
  const zc = lerp(-3, 2, SHEEN_EASE(u));
  const a = view.project([0, FLOOR, zc - 0.55]);
  const b = view.project([0, FLOOR, zc + 0.55]);
  const g = ctx.createLinearGradient(a.x, a.y, b.x, b.y);
  const k = 0.26 * Math.sin(Math.PI * u);
  g.addColorStop(0, rgba(PALETTE.amberBright, 0));
  g.addColorStop(0.5, rgba(PALETTE.amberBright, k));
  g.addColorStop(1, rgba(PALETTE.amberBright, 0));
  ctx.save();
  ctx.beginPath();
  for (const p of panels) {
    p.pts.forEach((q, i) => {
      const s = view.project(q);
      if (i === 0) ctx.moveTo(s.x, s.y);
      else ctx.lineTo(s.x, s.y);
    });
    ctx.closePath();
  }
  ctx.clip("nonzero");
  ctx.globalCompositeOperation = "lighter";
  ctx.fillStyle = g;
  ctx.fillRect(0, 0, W, H);
  ctx.restore();
}

/** The lid's seams light up for the two frames it seats. */
function drawSlamSeams(ctx: CanvasRenderingContext2D, view: View, lt: number, panels: Panel[]): void {
  const f = slamFlash(lt);
  if (f <= 0) return;
  const lid = panels.find((p) => p.key === "lid")!;
  ctx.save();
  ctx.globalCompositeOperation = "lighter";
  ctx.lineJoin = "round";
  polygon(
    ctx,
    lid.pts.map((q) => view.project(q)),
  );
  ctx.strokeStyle = rgba(PALETTE.amberBright, 0.6 * f);
  ctx.lineWidth = 8;
  ctx.stroke();
  ctx.strokeStyle = rgba(PALETTE.paper, 0.85 * f);
  ctx.lineWidth = 2.2;
  ctx.stroke();
  ctx.restore();
}

// Dust squeezed out of the seams when the lid slams: small warm puffs pushed
// out fast from all four top edges, and hard chips of card flicked up and
// falling. All of it is gone by ~1.55.
let puff: HTMLCanvasElement | null = null;
function puffSprite(): HTMLCanvasElement {
  if (puff) return puff;
  puff = makeCanvas(64, 64);
  const g = puff.getContext("2d")!;
  const grad = g.createRadialGradient(32, 32, 0, 32, 32, 32);
  grad.addColorStop(0, mix(PALETTE.amber, PALETTE.paper, 0.3));
  grad.addColorStop(0.45, mix(PALETTE.amber, PALETTE.paper, 0.3, 0.5));
  grad.addColorStop(1, mix(PALETTE.amber, PALETTE.paper, 0.3, 0));
  g.fillStyle = grad;
  g.fillRect(0, 0, 64, 64);
  return puff;
}

const SIDES: XZ[] = [
  [0, 1],
  [1, 0],
  [0, -1],
  [-1, 0],
];

function drawDust(ctx: CanvasRenderingContext2D, view: View, lt: number): void {
  const d = lt - T_SLAM;
  if (d <= 0 || d > 0.145) return;
  const sprite = puffSprite();
  const S = view.cam.scale;
  const base = ctx.globalAlpha;
  ctx.save();
  // Puffs are light kicked up off the seams, so they add.
  ctx.globalCompositeOperation = "lighter";
  for (let i = 0; i < 18; i++) {
    const life = 0.05 + hash(i, 21) * 0.05;
    if (d > life) continue;
    const o = SIDES[i % 4];
    const u = hash(i, 23) * 1.1 - 0.55;
    const sp = 3.2 + hash(i, 25) * 2.4;
    const travel = (sp * (1 - Math.exp(-18 * d))) / 18;
    const spread = u * (1 + travel);
    const p = view.project([
      o[0] * (0.5 + travel) - o[1] * spread,
      0.48 + travel * 0.2,
      o[1] * (0.5 + travel) + o[0] * spread,
    ]);
    const k = 1 - d / life;
    const r = (0.012 + 0.04 * (1 - k)) * S * (0.7 + hash(i, 27) * 0.6);
    ctx.globalAlpha = base * 0.5 * k ** 1.5;
    ctx.drawImage(sprite, p.x - r, p.y - r, r * 2, r * 2);
  }
  ctx.globalCompositeOperation = "source-over";
  for (let i = 0; i < 14; i++) {
    const life = 0.09 + hash(i, 41) * 0.05;
    if (d > life) continue;
    const o = SIDES[(i + 1) % 4];
    const u = hash(i, 43) * 1.0 - 0.5;
    const sp = 3 + hash(i, 45) * 2.6;
    const out = (sp * (1 - Math.exp(-10 * d))) / 10;
    const y = 0.5 + (1.3 + hash(i, 47)) * d - 9 * d * d;
    const p = view.project([
      o[0] * (0.5 + out) - o[1] * u,
      y,
      o[1] * (0.5 + out) + o[0] * u,
    ]);
    const k = 1 - d / life;
    const sz = 2.2 + hash(i, 49) * 2.6;
    ctx.globalAlpha = base * k;
    ctx.fillStyle = hash(i, 51) < 0.5 ? PALETTE.amberBright : PALETTE.amberDeep;
    ctx.save();
    ctx.translate(p.x, p.y);
    ctx.rotate(hash(i, 53) * TAU + d * 40 * (hash(i, 55) - 0.5));
    ctx.fillRect(-sz, -sz * 0.45, sz * 2, sz * 0.9);
    ctx.restore();
  }
  ctx.restore();
}

export const scene: Scene = {
  id: "unfold",
  start: bar(0),
  end: bar(1),
  draw(ctx, lt, env) {
    ctx.fillStyle = mix(PALETTE.night, PALETTE.bg, smoothstep(0.5, 1.4, lt));
    ctx.fillRect(0, 0, env.W, env.H);
    const cam = cameraAt(lt);
    const view = new View(cam);
    const [sx, sy] = shake(lt, T_SLAM, 7, 0.045);
    const squash = slamSquash(lt);

    ctx.save();
    ctx.translate(sx, sy);
    if (lt >= T_BOXED) {
      const tape = tapeAt(lt);
      const pose = { ...H1_POSE, tape, squash };
      drawStagedBox(ctx, cam, pose);
      // A glint rides the tape's leading edge across the lid, then holds on
      // the crease where the tape wraps over, so the end stays readable as
      // it presses down the side.
      const f = boxFrame(pose);
      const along = clamp(tape / 0.8);
      const q = view.project(boxPoint(f, -1 + 2 * along, 1, 0));
      const fadeIn = smoothstep(T_TAPE0, T_TAPE0 + 0.035, lt);
      const live = fadeIn * (1 - smoothstep(T_TAPE1 + 0.01, T_TAPE1 + 0.08, lt));
      if (live > 0) {
        // The light swells as the tape presses home on the beat; the star
        // stays small so the tape end reads as it seats.
        const pop = pulse(lt, T_TAPE1, 0.03, 0.03);
        glow(ctx, q.x, q.y, 52 * (1 + 0.9 * pop), PALETTE.amberBright, (0.45 + 0.15 * pop) * live);
        sparkle(ctx, q.x, q.y, 17 * live * (1 + 0.2 * pop), live, lt * 3);
      }
      drawDust(ctx, view, lt);
      ctx.restore();
      return;
    }

    const a = anglesAt(lt);
    const panels = buildPanels(a, lt, squash);
    drawShadow(ctx, view, H1_POSE.pos, 1, 0, 0.45 * smoothstep(0.95, 1.45, lt));

    if (lt < T_FLOODED) {
      const D = floodD(lt);
      const lw = OUTLINE_RATIO * view.cam.scale;
      const widthOf = (key: PanelKey) =>
        lw * view.project(panels.find((p) => p.key === key)!.pts[widthCorner(key)]).f;
      drawFlood(ctx, view, D, a, panels, (key) => drawCreases(ctx, view, lt, D, a, key));
      drawOutline(ctx, view, lt, D, a, widthOf);
      drawSheen(ctx, view, lt, panels, env.W, env.H);
    } else {
      drawPanels(ctx, view, panels, motionSmears(lt, a, squash), (key) =>
        drawCreases(ctx, view, lt, Infinity, a, key),
      );
      drawSheen(ctx, view, lt, panels, env.W, env.H);
      drawSlamSeams(ctx, view, lt, panels);
    }
    drawLights(ctx, view, lt);
    drawSparks(ctx, view, lt);
    drawDust(ctx, view, lt);
    ctx.restore();
  },
};
