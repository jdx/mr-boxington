// Liquid morph: the kept discs turn to jelly, spiral together into one
// metaball blob, breathe, and set into the end silhouette. The surface is a
// real implicit field (summed compact kernels) sampled on a coarse grid,
// contoured with marching squares, resampled, and drawn as a smooth path.

import { bar, beat, END_CAM, END_POSE, keptDiscs, PALETTE, type Scene } from "../bible";
import { boxSilhouette } from "../box";
import { mix, rgba } from "../color";
import { glow } from "../fx";
import {
  clamp,
  cubicBezier,
  hash,
  inOutSine,
  keys,
  lerp,
  noise1,
  progress,
  smoothstep,
  swiftOut,
  TAU,
  wobble,
} from "../math";
import { View } from "../space";

const T0 = bar(6);
/** Local time of global beat `n`. */
const at = (n: number): number => beat(n) - T0;

// Anchors, local seconds.
const PULL = at(24.5);
const FIRST = at(25);
const SECOND = at(25.5);
const MERGE = at(26);
const SNAP = at(26.25);
const INHALE = at(26.5);
const MORPH = at(26.75);
const LAND = at(27.5);
/** Every residual wobble is gone by here, so the last frames are exact. */
const SETTLED = LAND + 0.2;

/**
 * Kernel reach in radii. Tight, so a body only feels a neighbour close by:
 * facing surfaces reach out as rounded tips and neck into a bridge instead
 * of lifting flat across their whole side.
 */
const K = 1.5;
const ISO = (1 - 1 / (K * K)) ** 3;
/** Field grid spacing, logical px. */
const STEP = 4;

interface Pt {
  x: number;
  y: number;
}

/** A local bulge on a ball's rim: `a` of its radius toward angle `th`, `k` sharpness. */
interface Tip {
  cx: number;
  cy: number;
  a: number;
  k: number;
}

/** A metaball with low-order shape modes and local tips: r(θ) = r (1 + a(θ)). */
interface Ball {
  x: number;
  y: number;
  r: number;
  c1: number;
  s1: number;
  c2: number;
  s2: number;
  c3: number;
  s3: number;
  tips?: Tip[];
}

/** A capsule kernel, tapering from r0 at (x0, y0) to r1 at (x1, y1): arms and threads. */
interface Seg {
  x0: number;
  y0: number;
  x1: number;
  y1: number;
  r0: number;
  r1: number;
}

const ball = (x: number, y: number, r: number): Ball => ({
  x,
  y,
  r,
  c1: 0,
  s1: 0,
  c2: 0,
  s2: 0,
  c3: 0,
  s3: 0,
});

/** Add an elongation `e` along angle `dir` (negative flattens across it). */
function stretch(b: Ball, e: number, dir: number): void {
  b.c2 += e * Math.cos(2 * dir);
  b.s2 += e * Math.sin(2 * dir);
}

/** Add a one-sided lean `e` toward angle `dir` (an egg, or a teardrop with stretch). */
function lean(b: Ball, e: number, dir: number): void {
  b.c1 += e * Math.cos(dir);
  b.s1 += e * Math.sin(dir);
}

interface Layout {
  discs: { x: number; y: number; r: number }[];
  /** Start attractor: the center disc. */
  a0: Pt;
  /** End silhouette, clockwise on screen, and its center. */
  hex: Pt[];
  hc: Pt;
}

let cached: Layout | null = null;
function layout(): Layout {
  if (cached) return cached;
  const discs = keptDiscs();
  const hex = boxSilhouette(new View(END_CAM), END_POSE).map((p) => ({ x: p.x, y: p.y }));
  let hx = 0;
  let hy = 0;
  for (const p of hex) {
    hx += p.x / hex.length;
    hy += p.y / hex.length;
  }
  cached = { discs, a0: { x: discs[0].x, y: discs[0].y }, hex, hc: { x: hx, y: hy } };
  return cached;
}

// Pull schedule for the six outer discs (KEEP order 1..6). They join in
// point-symmetric pairs through the core, so the blob grows as a bar the
// swirl turns into a pinwheel, never as lobes on one side. Each pair joins
// with its own gesture: the first pair reaches for the core (b25), the core
// throws out arms and yanks the second pair in (b25.5), and the sides slam
// in on b26 hard enough to throw up a drop.
type Gesture = "reach" | "grab" | "slam";
interface Pull {
  gesture: Gesture;
  /** When the bridge forms. */
  contact: number;
  /** Radius (from the attractor) at the bridge; tuned so it snaps on the beat. */
  rc: number;
  /** Seconds from the bridge to resting inside the blob. */
  slurp: number;
  /** Approach ease power: higher waits longer, then rushes. */
  p: number;
  /** Outward wind-up before the pull, px. */
  antic: number;
}
const PULLS: Record<number, Pull> = {
  // Top left and bottom right.
  4: { gesture: "reach", contact: FIRST, rc: 112.9, slurp: 0.19, p: 1.9, antic: 18 },
  3: { gesture: "reach", contact: FIRST, rc: 112.9, slurp: 0.21, p: 1.9, antic: 18 },
  // Top right and bottom left.
  6: { gesture: "grab", contact: SECOND, rc: 150, slurp: 0.13, p: 1.5, antic: 18 },
  5: { gesture: "grab", contact: SECOND, rc: 150, slurp: 0.2, p: 1.5, antic: 18 },
  // Left and right.
  1: { gesture: "slam", contact: MERGE, rc: 152.7, slurp: 0.08, p: 4.5, antic: 40 },
  2: { gesture: "slam", contact: MERGE, rc: 152.7, slurp: 0.08, p: 4.5, antic: 40 },
};
/**
 * The core's arms for the grab: seconds to shoot out, tip radius, and how
 * long after the bridge they would reach full length (tuned so the bridge
 * lands on the beat while the arm is still travelling).
 */
const ARM = { out: 0.22, r: 15, lead: 0.1223 };
/** Ring radius the absorbed discs churn on inside the blob. */
const REST = 20;
/**
 * The core takes on each absorbed disc's volume (area grows by CORE_TAKE of
 * a disc per disc) while the disc itself thins out inside it, so the blob
 * neither deflates nor bulges with lumps as it settles.
 */
const CORE_TAKE = 1;
const DISC_THIN = 0.7;

/** Radius of an outer disc from the attractor. */
function radial(i: number, rho0: number, t: number): number {
  const s = PULLS[i];
  if (t <= PULL) return rho0 + s.antic * inOutSine(progress(0.04, PULL, t));
  const r0 = rho0 + s.antic;
  const T1 = s.contact - PULL;
  if (t <= s.contact) return lerp(r0, s.rc, ((t - PULL) / T1) ** s.p);
  // Hermite from the contact speed to rest: the bridge accelerates it in.
  const v = (s.p * (s.rc - r0)) / T1;
  const u = clamp((t - s.contact) / s.slurp);
  const u2 = u * u;
  const u3 = u2 * u;
  return (
    (2 * u3 - 3 * u2 + 1) * s.rc + (u3 - 2 * u2 + u) * s.slurp * v + (-2 * u3 + 3 * u2) * REST
  );
}

/** 0..1 how far disc `i` has been absorbed. */
const absorbed = (i: number, t: number): number =>
  i === 0 ? 1 : inOutSine(progress(PULLS[i].contact, PULLS[i].contact + PULLS[i].slurp, t));

/**
 * Radius of the core. It swells a little ahead of each slurp, so the last
 * of a disc sinks under its surface.
 */
function coreR(t: number): number {
  const L = layout();
  let taken = 0;
  for (let k = 1; k < L.discs.length; k++) taken += absorbed(k, t) ** 0.6;
  return L.discs[0].r * Math.sqrt(1 + CORE_TAKE * taken);
}

const riseEase = cubicBezier(0.4, 0, 0.45, 1);
const swirlEase = cubicBezier(0.5, 0, 0.45, 1);

/** The point everything is drawn toward: it floats up to the end silhouette. */
const riseY = (t: number, a0y: number, hcy: number): number =>
  lerp(a0y, hcy, riseEase(progress(PULL, LAND, t)));

/** A slight sway so the float up is an arc, back on center by the landing. */
const swayX = (t: number): number =>
  24 * Math.sin(Math.PI * inOutSine(progress(PULL, LAND, t)));

/** Global spiral: the pull carries a slow clockwise swirl. */
const swirl = (t: number): number => 1.15 * swirlEase(progress(0, LAND, t));

/** Direction of outer disc `i` from the attractor: absorbed discs turn with the blob. */
function discAngle(i: number, t: number): number {
  const L = layout();
  const d = L.discs[i];
  return Math.atan2(d.y - L.a0.y, d.x - L.a0.x) + swirl(t) * (0.4 + 0.6 * absorbed(i, t));
}

/**
 * Camera push about the blob. It creeps in with the pull, lands on the b26
 * slam and holds while the blob breathes and starts to set, then snaps back
 * out and stops dead on b27.5, so the silhouette's rebound reads as impact.
 */
const zoom = keys([
  [0, 1],
  [MERGE, 1.6, cubicBezier(0.55, 0, 0.3, 1)],
  [LAND - 0.22, 1.68, inOutSine],
  [LAND, 1, cubicBezier(0.5, 0, 0.75, 0.5)],
]);

// The drop the b26 slam throws up. It rises inside the blob, breaks the
// surface as a ball on a thread that thins until it snaps on b26.25, flies
// a ballistic arc, and plops back in on b26.5 as the blob inhales to meet
// it. Positions are relative to the blob center, along `dir`.
const DROP = {
  r: 25,
  /** Launch direction: square to the sides' slam, which the swirl has turned. */
  dir: -Math.PI / 2 + 0.2,
  t0: MERGE + 0.03,
  ts: SNAP,
  tc: INHALE,
  /** Thread length (core rim to drop) when it snaps, px. */
  gap: 26,
  /** Launch speed at the snap, px/s. */
  vs: 1200,
  /** Gravity, px/s², tuned so the drop meets the blob on tc. */
  g: 22126,
  /** Thread radius when it snaps, px. */
  thin: 2.6,
};
/** Seconds after rejoining that the drop has fully dissolved into the blob. */
const DISSOLVE = 0.1;

function dropAt(t: number): Pt {
  const D = DROP;
  const ux = Math.cos(D.dir);
  const uy = Math.sin(D.dir);
  const ds = coreR(D.ts) + D.r + D.gap;
  if (t <= D.ts) {
    const d0 = coreR(D.t0) - D.r - 4;
    const u = progress(D.t0, D.ts, t);
    // Accelerates out of the blob and leaves at the launch speed.
    const m = clamp((D.vs * (D.ts - D.t0)) / (ds - d0), 0, 3);
    const d = lerp(d0, ds, (3 - m) * u * u + (m - 2) * u * u * u);
    return { x: ux * d, y: uy * d };
  }
  const s = t - D.ts;
  const d = ds + D.vs * s;
  return { x: ux * d, y: uy * d + 0.5 * D.g * s * s };
}

/** The b26 slam squeezes the blob from the sides: a vertical stretch that springs back. */
const slam = (t: number): number => 0.085 * wobble(t, MERGE, 3.4, 7);

/**
 * The breath, as extra vertical stretch: in on b26.5 (rising to catch the
 * drop), out into a squat on b26.75, and the morph springs up out of it.
 */
const breath = keys([
  [MERGE + 0.12, 0],
  [INHALE, 0.095, inOutSine],
  [MORPH, -0.075, inOutSine],
  [MORPH + 0.17, 0.04, inOutSine],
  [LAND - 0.03, 0, inOutSine],
]);

/** Liquid shading, flat at both handoffs; it drains away gently after the landing. */
const shade = (t: number): number =>
  swiftOut(progress(0.04, 0.45, t)) * (1 - smoothstep(LAND - 0.02, LAND + 0.14, t));

// Splashes: every join (a disc bridging on its beat, the drop plopping back)
// flares light at the contact point and sends a bump running both ways
// around the blob's rim.
interface Splash {
  t: number;
  /** Direction of the contact from the blob center at time `t`. */
  angle: (t: number) => number;
  /** Distance of the contact point from the blob center. */
  dist: number;
  amp: number;
}
let splashCache: Splash[] | null = null;
function splashes(): Splash[] {
  if (splashCache) return splashCache;
  const L = layout();
  const out: Splash[] = [];
  for (const k of Object.keys(PULLS)) {
    const i = Number(k);
    const s = PULLS[i];
    out.push({
      t: s.contact,
      angle: (t) => discAngle(i, t),
      dist: s.rc - L.discs[i].r * (s.gesture === "grab" ? 0.5 : 0.9),
      amp: s.gesture === "slam" ? 0.07 : 0.05,
    });
  }
  const p = dropAt(DROP.tc);
  const th = Math.atan2(p.y, p.x);
  out.push({ t: DROP.tc, angle: () => th, dist: Math.hypot(p.x, p.y) - DROP.r, amp: 0.06 });
  splashCache = out;
  return out;
}

interface Frame {
  balls: Ball[];
  segs: Seg[];
  /** Blob center (attractor). */
  a: Pt;
  /** Core radius. */
  R: number;
}

function motion(t: number): Frame {
  const L = layout();
  const a: Pt = { x: L.a0.x + swayX(t), y: riseY(t, L.a0.y, L.hc.y) };
  const R = coreR(t);
  const balls: Ball[] = [];
  const segs: Seg[] = [];
  const h = 1 / 240;
  L.discs.forEach((d, i) => {
    // Jelly: the b24 impulse rings each disc's 2- and 3-lobe modes. One
    // shared sideways splat axis, so the group reads as a single landing.
    const delay = i === 0 ? 0 : 0.03 * hash(i, 3);
    const f2 = 3.3 + 0.4 * hash(i, 5);
    const j2 = 0.17 * wobble(t, delay, f2, 3.2);
    const j3 = 0.018 * wobble(t, delay + 0.04, f2 * 1.45, 4);
    const o3 = TAU * hash(i, 9);
    const b = ball(d.x, d.y, d.r);
    b.c2 = j2;
    b.c3 = j3 * Math.cos(3 * o3);
    b.s3 = j3 * Math.sin(3 * o3);
    if (i === 0) {
      b.x = a.x;
      b.y = a.y;
      b.r = R;
      balls.push(b);
      return;
    }
    const s = PULLS[i];
    const tc = s.contact;
    const ab = absorbed(i, t);
    const rho0 = Math.hypot(d.x - L.a0.x, d.y - L.a0.y);
    // Once inside, each disc churns gently: lava, not a lump.
    const lump = 1 + 0.12 * ab * noise1(t * 2.2 + i * 3.7, i);
    const rho = radial(i, rho0, t) * lump;
    const speed = (radial(i, rho0, t - h) - radial(i, rho0, t + h)) / (2 * h);
    const phi = discAngle(i, t);
    const ux = Math.cos(phi);
    const uy = Math.sin(phi);
    b.x = a.x + rho * ux;
    b.y = a.y + rho * uy;
    b.r *= 1 - DISC_THIN * ab;
    // Stretch along the pull and lean into it, by speed and by the blob's
    // growing tide, until absorbed. Capped so the ends stay round; the
    // yanked and slammed discs stretch further.
    const tide = 0.06 * smoothstep(PULL, tc, t);
    const cap = s.gesture === "reach" ? 0.12 : 0.17;
    const e = Math.min(cap, clamp(speed / 1300, -0.05, 0.17) + tide) * (1 - ab);
    // Once bridged its inner end is drawn down the neck as the blob sucks
    // it in; the outer end stays round, so it never reads as a horn.
    const suck = Math.sin(Math.PI * ab);
    const toward = phi + Math.PI;
    stretch(b, e + 0.05 * suck, toward);
    lean(b, 0.4 * e + 0.28 * suck, toward);
    if (s.gesture === "reach") {
      // The first pair reach for the core: a rounded bulge (tip radius about
      // half the disc's) draws out over the frames before the bridge and
      // relaxes once the neck has formed.
      const g = smoothstep(tc - 0.16, tc - 0.01, t) * (1 - smoothstep(tc + 0.02, tc + 0.16, t));
      if (g > 0) b.tips = [{ cx: -ux, cy: -uy, a: 0.4 * g, k: 6.5 }];
    } else if (s.gesture === "grab" && ab < 1) {
      // The core shoots an arm at the disc; the arm lands on the beat, then
      // hauls the disc in, a long neck until the last moment.
      const t1 = tc + ARM.lead;
      const k = inOutSine(progress(t1 - ARM.out, t1, t));
      if (k > 0) {
        const fade = 1 - smoothstep(0.5, 0.95, ab);
        const len = lerp(R * 0.7, rho - b.r * 0.35, k);
        segs.push({
          x0: a.x + ux * R * 0.5,
          y0: a.y + uy * R * 0.5,
          x1: a.x + ux * len,
          y1: a.y + uy * len,
          r0: R * 0.3 * fade,
          r1: ARM.r * fade,
        });
      }
    }
    balls.push(b);
  });

  // The drop.
  const D = DROP;
  const ux = Math.cos(D.dir);
  const uy = Math.sin(D.dir);
  if (t > D.t0 - 0.04 && t < D.tc + DISSOLVE) {
    const p = dropAt(t);
    const q = dropAt(t + h);
    const o = dropAt(t - h);
    const vx = (q.x - o.x) / (2 * h);
    const vy = (q.y - o.y) / (2 * h);
    const sp = Math.hypot(vx, vy);
    const dir = Math.atan2(vy, vx);
    // Born inside the blob, so it swells there before breaking out; after
    // the plop it sinks and dissolves inside.
    const size =
      smoothstep(D.t0 - 0.04, D.t0 + 0.03, t) * (1 - smoothstep(D.tc + 0.02, D.tc + DISSOLVE, t));
    const db = ball(a.x + p.x, a.y + p.y, D.r * size);
    // Stretched along its path by speed, ringing once from the snap. It
    // stays round or long until contact and flattens only after the plop.
    const fly = Math.min(0.12, sp / 9000) + (t > D.ts ? 0.07 * wobble(t, D.ts, 7, 16) : 0);
    const flat = 0.28 * Math.sin(Math.PI * progress(D.tc, D.tc + 0.06, t));
    stretch(db, fly - flat, dir);
    balls.push(db);
    const base = R - 12;
    if (t < D.ts) {
      // The thread: thins fast, then lingers thin, so it is a ball on a
      // string (under 10 px on screen) when it snaps.
      const u = progress(D.t0 + 0.02, D.ts, t);
      const rt = (D.thin + (0.6 * D.r - D.thin) * (1 - u) ** 2) * size;
      segs.push({
        x0: a.x + ux * base,
        y0: a.y + uy * base,
        x1: a.x + p.x,
        y1: a.y + p.y,
        r0: rt * 1.4,
        r1: rt,
      });
      // The core heaves up under it, broad and low, and settles before the snap.
      const heave = Math.sin(Math.PI * progress(D.t0, D.ts - 0.03, t));
      if (heave > 0) (balls[0].tips ??= []).push({ cx: ux, cy: uy, a: 0.07 * heave, k: 2.2 });
    } else if (t < D.ts + 0.1) {
      // Snapped: the root recoils into the core and the tail snaps up into
      // the drop along the thread's line, each fattening a little as it
      // shortens and ending tucked inside its body.
      const d = t - D.ts;
      const k = (1 - progress(0, 0.06, d)) ** 2;
      const rs = (D.thin + 4 * (1 - k)) * (1 - smoothstep(0.045, 0.09, d));
      const root = R - rs * (1 - k) ** 2 + 0.36 * D.gap * k;
      segs.push({
        x0: a.x + ux * base,
        y0: a.y + uy * base,
        x1: a.x + ux * root,
        y1: a.y + uy * root,
        r0: rs * 1.4,
        r1: rs,
      });
      const tail = D.r * size + 0.3 * D.gap * k - 2 * rs * (1 - k) ** 2;
      segs.push({
        x0: a.x + p.x,
        y0: a.y + p.y,
        x1: a.x + p.x - ux * tail,
        y1: a.y + p.y - uy * tail,
        r0: rs * 1.2,
        r1: rs,
      });
    }
    // The blob swells up to meet the drop on its way back down.
    const reach =
      smoothstep(D.tc - 0.12, D.tc - 0.005, t) * (1 - smoothstep(D.tc + 0.02, D.tc + 0.16, t));
    if (reach > 0) {
      const pc = dropAt(D.tc);
      const l = Math.hypot(pc.x, pc.y);
      (balls[0].tips ??= []).push({ cx: pc.x / l, cy: pc.y / l, a: 0.1 * reach, k: 5 });
    }
  }
  return { balls, segs, a, R };
}

// Field and marching squares.

// Marching-squares segments per corner code (TL 8, TR 4, BR 2, BL 1) as
// [from, to] edge pairs (0 top, 1 right, 2 bottom, 3 left), wound with the
// inside on the left. The saddles 5 and 10 split unless the cell center is
// inside, in which case SADDLE_JOINED bridges them.
const SEGS: readonly (readonly number[])[] = [
  [], [2, 3], [1, 2], [1, 3], [0, 1], [0, 1, 2, 3], [0, 2], [0, 3],
  [3, 0], [2, 0], [3, 0, 1, 2], [1, 0], [3, 1], [2, 1], [3, 2], [],
];
const SADDLE_JOINED: Record<number, readonly number[]> = { 5: [0, 3, 2, 1], 10: [1, 0, 3, 2] };

/** Signed field minus the iso level on a grid, contoured into closed loops. */
function contour(balls: Ball[], segs: Seg[]): Pt[][] {
  let minX = Infinity;
  let minY = Infinity;
  let maxX = -Infinity;
  let maxY = -Infinity;
  for (const g of segs) {
    const rr = K * Math.max(g.r0, g.r1);
    minX = Math.min(minX, g.x0 - rr, g.x1 - rr);
    minY = Math.min(minY, g.y0 - rr, g.y1 - rr);
    maxX = Math.max(maxX, g.x0 + rr, g.x1 + rr);
    maxY = Math.max(maxY, g.y0 + rr, g.y1 + rr);
  }
  const reach: number[] = [];
  for (const b of balls) {
    // |a(θ)| is bounded by the sum of its mode amplitudes.
    let amax =
      Math.abs(b.c1) + Math.abs(b.s1) +
      Math.abs(b.c2) + Math.abs(b.s2) +
      Math.abs(b.c3) + Math.abs(b.s3);
    if (b.tips) for (const q of b.tips) amax += q.a;
    const rr = K * b.r * (1 + amax);
    reach.push(rr);
    minX = Math.min(minX, b.x - rr);
    minY = Math.min(minY, b.y - rr);
    maxX = Math.max(maxX, b.x + rr);
    maxY = Math.max(maxY, b.y + rr);
  }
  const x0 = Math.floor(minX / STEP) * STEP - STEP;
  const y0 = Math.floor(minY / STEP) * STEP - STEP;
  const nx = Math.ceil((maxX - x0) / STEP) + 2;
  const ny = Math.ceil((maxY - y0) / STEP) + 2;
  const f = new Float32Array(nx * ny).fill(-ISO);

  balls.forEach((b, k) => {
    if (b.r < 0.5) return;
    const rr = reach[k];
    const R2 = (K * b.r) ** 2;
    const tips = b.tips ?? [];
    const deform = b.c1 || b.s1 || b.c2 || b.s2 || b.c3 || b.s3 || tips.length;
    const i0 = Math.max(0, Math.floor((b.x - rr - x0) / STEP));
    const i1 = Math.min(nx - 1, Math.ceil((b.x + rr - x0) / STEP));
    const j0 = Math.max(0, Math.floor((b.y - rr - y0) / STEP));
    const j1 = Math.min(ny - 1, Math.ceil((b.y + rr - y0) / STEP));
    const rr2 = rr * rr;
    for (let j = j0; j <= j1; j++) {
      const dy = y0 + j * STEP - b.y;
      for (let i = i0; i <= i1; i++) {
        const dx = x0 + i * STEP - b.x;
        const d2 = dx * dx + dy * dy;
        if (d2 >= rr2) continue;
        let sc = 1;
        if (deform && d2 > 1e-9) {
          const d = Math.sqrt(d2);
          const c = dx / d;
          const s = dy / d;
          sc +=
            b.c1 * c +
            b.s1 * s +
            b.c2 * (c * c - s * s) +
            b.s2 * 2 * s * c +
            b.c3 * c * (4 * c * c - 3) +
            b.s3 * s * (3 - 4 * s * s);
          // Von Mises bumps: smooth, rounded, and local.
          for (const q of tips) sc += q.a * Math.exp(q.k * (c * q.cx + s * q.cy - 1));
        }
        const w = 1 - d2 / (sc * sc * R2);
        if (w > 0) f[j * nx + i] += w * w * w;
      }
    }
  });

  // Capsules: the same kernel about the nearest point of a segment, with
  // the radius tapering along it.
  for (const g of segs) {
    const rm = K * Math.max(g.r0, g.r1);
    if (rm < 0.5) continue;
    const ex = g.x1 - g.x0;
    const ey = g.y1 - g.y0;
    const el2 = ex * ex + ey * ey || 1e-9;
    const i0 = Math.max(0, Math.floor((Math.min(g.x0, g.x1) - rm - x0) / STEP));
    const i1 = Math.min(nx - 1, Math.ceil((Math.max(g.x0, g.x1) + rm - x0) / STEP));
    const j0 = Math.max(0, Math.floor((Math.min(g.y0, g.y1) - rm - y0) / STEP));
    const j1 = Math.min(ny - 1, Math.ceil((Math.max(g.y0, g.y1) + rm - y0) / STEP));
    for (let j = j0; j <= j1; j++) {
      const py = y0 + j * STEP - g.y0;
      for (let i = i0; i <= i1; i++) {
        const px = x0 + i * STEP - g.x0;
        const u = clamp((px * ex + py * ey) / el2);
        const dx = px - u * ex;
        const dy = py - u * ey;
        const rr = K * (g.r0 + (g.r1 - g.r0) * u);
        if (rr <= 0) continue;
        const w = 1 - (dx * dx + dy * dy) / (rr * rr);
        if (w > 0) f[j * nx + i] += w * w * w;
      }
    }
  }

  // Oriented segments with the inside on the left (on screen). Edge ids:
  // 2 * sample for the horizontal edge to its right, +1 for the vertical
  // edge below it.
  const next = new Int32Array(2 * nx * ny).fill(-1);
  const starts: number[] = [];
  for (let j = 0; j < ny - 1; j++) {
    for (let i = 0; i < nx - 1; i++) {
      const k = j * nx + i;
      const va = f[k];
      const vb = f[k + 1];
      const vc = f[k + nx + 1];
      const vd = f[k + nx];
      const code = (va > 0 ? 8 : 0) | (vb > 0 ? 4 : 0) | (vc > 0 ? 2 : 0) | (vd > 0 ? 1 : 0);
      if (code === 0 || code === 15) continue;
      const segs =
        (code === 5 || code === 10) && va + vb + vc + vd > 0 ? SADDLE_JOINED[code] : SEGS[code];
      const edge = [2 * k, 2 * (k + 1) + 1, 2 * (k + nx), 2 * k + 1];
      for (let q = 0; q < segs.length; q += 2) {
        next[edge[segs[q]]] = edge[segs[q + 1]];
        starts.push(edge[segs[q]]);
      }
    }
  }

  const point = (e: number): Pt => {
    const k = e >> 1;
    const i = k % nx;
    const j = (k - i) / nx;
    const v0 = f[k];
    if ((e & 1) === 0) {
      const u = v0 / (v0 - f[k + 1]);
      return { x: x0 + (i + u) * STEP, y: y0 + j * STEP };
    }
    const u = v0 / (v0 - f[k + nx]);
    return { x: x0 + i * STEP, y: y0 + (j + u) * STEP };
  };

  const loops: Pt[][] = [];
  for (const s of starts) {
    if (next[s] < 0) continue;
    const loop: Pt[] = [];
    let e = s;
    while (e >= 0 && next[e] >= 0) {
      loop.push(point(e));
      const n = next[e];
      next[e] = -1;
      e = n;
    }
    if (loop.length >= 3) loops.push(loop);
  }
  return loops;
}

/** Resample a closed polyline at even arc-length spacing. */
function resample(loop: Pt[], spacing: number): Pt[] {
  const n = loop.length;
  let total = 0;
  for (let i = 0; i < n; i++) {
    const a = loop[i];
    const b = loop[(i + 1) % n];
    total += Math.hypot(b.x - a.x, b.y - a.y);
  }
  const m = Math.max(8, Math.round(total / spacing));
  const step = total / m;
  const out: Pt[] = [];
  let i = 0;
  let acc = 0;
  let segLen = Math.hypot(loop[1 % n].x - loop[0].x, loop[1 % n].y - loop[0].y);
  for (let k = 0; k < m; k++) {
    const target = k * step;
    while (acc + segLen < target && i < n - 1) {
      acc += segLen;
      i++;
      const a = loop[i];
      const b = loop[(i + 1) % n];
      segLen = Math.hypot(b.x - a.x, b.y - a.y);
    }
    const a = loop[i];
    const b = loop[(i + 1) % n];
    const u = segLen > 0 ? clamp((target - acc) / segLen) : 0;
    out.push({ x: lerp(a.x, b.x, u), y: lerp(a.y, b.y, u) });
  }
  return out;
}

const area = (loop: Pt[]): number => {
  let s = 0;
  for (let i = 0, n = loop.length; i < n; i++) {
    const a = loop[i];
    const b = loop[(i + 1) % n];
    s += a.x * b.y - b.x * a.y;
  }
  return s / 2;
};

function centroid(loop: Pt[]): Pt {
  let cx = 0;
  let cy = 0;
  let s = 0;
  for (let i = 0, n = loop.length; i < n; i++) {
    const a = loop[i];
    const b = loop[(i + 1) % n];
    const c = a.x * b.y - b.x * a.y;
    s += c;
    cx += (a.x + b.x) * c;
    cy += (a.y + b.y) * c;
  }
  return s === 0 ? loop[0] : { x: cx / (3 * s), y: cy / (3 * s) };
}

/** Whether `p` lies inside a closed polyline (even-odd). */
function inside(loop: Pt[], p: Pt): boolean {
  let c = false;
  for (let i = 0, j = loop.length - 1; i < loop.length; j = i++) {
    const a = loop[i];
    const b = loop[j];
    if (a.y > p.y !== b.y > p.y && p.x < ((b.x - a.x) * (p.y - a.y)) / (b.y - a.y) + a.x) c = !c;
  }
  return c;
}

/** Farthest distance from `c` along direction `th` to a closed polyline. */
function rayDist(loop: Pt[], c: Pt, th: number): number {
  const dx = Math.cos(th);
  const dy = Math.sin(th);
  let best = 0;
  for (let i = 0, n = loop.length; i < n; i++) {
    const a = loop[i];
    const b = loop[(i + 1) % n];
    const ex = b.x - a.x;
    const ey = b.y - a.y;
    const den = dx * ey - dy * ex;
    if (Math.abs(den) < 1e-9) continue;
    const ax = a.x - c.x;
    const ay = a.y - c.y;
    const s = (ax * ey - ay * ex) / den;
    const u = (ax * dy - ay * dx) / den;
    if (s > 0 && u >= 0 && u <= 1 && s > best) best = s;
  }
  return best;
}

/**
 * Jelly ring-down once the silhouette lands, as an affine mode of the exact
 * hex so its edges stay straight: the camera stops dead, so the silhouette
 * carries on shrinking a little and squashes down, then springs back up
 * past rest and settles. Starts from rest, gone by SETTLED.
 */
function jellyHex(t: number): Pt[] {
  const L = layout();
  const d = t - LAND;
  const env = Math.exp(-d * 10) * (1 - smoothstep(LAND + 0.1, SETTLED, t));
  const s = 1 - 0.03 * env * Math.sin(TAU * 4.2 * d);
  const e = -0.032 * env * Math.sin(TAU * 4.8 * d);
  const rot = 0.02 * env * Math.sin(TAU * 4.2 * d);
  const sx = s / (1 + e);
  const sy = s * (1 + e);
  const c = Math.cos(rot);
  const sn = Math.sin(rot);
  const { hc } = L;
  return denseHex().map((p) => {
    const dx = (p.x - hc.x) * sx;
    const dy = (p.y - hc.y) * sy;
    return { x: hc.x + dx * c - dy * sn, y: hc.y + dx * sn + dy * c };
  });
}

let denseCache: Pt[] | null = null;
/**
 * The silhouette with every edge subdivided (corners kept exact) and wound
 * like the field contours, so the rim shading sees a dense loop.
 */
function denseHex(): Pt[] {
  if (denseCache) return denseCache;
  const hex = layout().hex;
  const out: Pt[] = [];
  for (let i = hex.length - 1; i >= 0; i--) {
    const a = hex[i];
    const b = hex[(i - 1 + hex.length) % hex.length];
    const n = Math.max(1, Math.round(Math.hypot(b.x - a.x, b.y - a.y) / 5));
    for (let k = 0; k < n; k++) out.push({ x: lerp(a.x, b.x, k / n), y: lerp(a.y, b.y, k / n) });
  }
  denseCache = out;
  return out;
}

/**
 * The silhouette with corners rounded by `rad` px (inset, then grown back by
 * arcs), so the liquid firms up into the box and the corners sharpen last.
 */
function roundedHex(hex: Pt[], rad: number): Pt[] {
  if (rad < 0.25) return hex;
  const n = hex.length;
  // Outward edge normals (the silhouette winds clockwise on screen).
  const nrm = hex.map((a, i) => {
    const b = hex[(i + 1) % n];
    const l = Math.hypot(b.x - a.x, b.y - a.y);
    return { x: (b.y - a.y) / l, y: -(b.x - a.x) / l };
  });
  const out: Pt[] = [];
  for (let i = 0; i < n; i++) {
    const n0 = nrm[(i - 1 + n) % n];
    const n1 = nrm[i];
    // Inset corner: moved along the bisector so both edges sit `rad` in.
    const k = rad / (1 + n0.x * n1.x + n0.y * n1.y);
    const q = { x: hex[i].x - (n0.x + n1.x) * k, y: hex[i].y - (n0.y + n1.y) * k };
    const a0 = Math.atan2(n0.y, n0.x);
    let a1 = Math.atan2(n1.y, n1.x);
    while (a1 < a0) a1 += TAU;
    for (let j = 0; j <= 6; j++) {
      const a = lerp(a0, a1, j / 6);
      out.push({ x: q.x + rad * Math.cos(a), y: q.y + rad * Math.sin(a) });
    }
  }
  return out;
}

/**
 * Corner radius of the forming silhouette: soft while the liquid flows in,
 * then snapped sharp over the last frames so the landing is an event.
 */
function cornerRadius(t: number): number {
  if (t >= LAND) return 0;
  const soft = lerp(48, 30, smoothstep(MORPH, LAND - 0.05, t));
  const snap = progress(LAND - 0.05, LAND, t);
  return soft * (1 - snap * snap);
}

/** Uniform ray samples around the blob, low-passed so a thin feature can never become a wedge. */
const RAYS = 144;
function blobRadii(blob: Pt[], c: Pt): Float64Array {
  const raw = new Float64Array(RAYS);
  for (let k = 0; k < RAYS; k++) raw[k] = rayDist(blob, c, (k / RAYS) * TAU - Math.PI);
  const out = new Float64Array(RAYS);
  const wts = [0.06, 0.12, 0.2, 0.24, 0.2, 0.12, 0.06];
  for (let k = 0; k < RAYS; k++) {
    let s = 0;
    for (let j = -3; j <= 3; j++) s += wts[j + 3] * raw[(k + j + RAYS) % RAYS];
    out[k] = s;
  }
  return out;
}

function sampleRadii(r: Float64Array, th: number): number {
  const x = ((th + Math.PI) / TAU) * RAYS;
  const i = Math.floor(x);
  const f = x - i;
  const a = r[((i % RAYS) + RAYS) % RAYS];
  const b = r[(((i + 1) % RAYS) + RAYS) % RAYS];
  return a + (b - a) * f;
}

/** Blob → silhouette by radial interpolation about moving centers. */
function morphLoop(blob: Pt[], t: number, m: number): Pt[] {
  const L = layout();
  const cb = centroid(blob);
  const hc = L.hc;
  // The silhouette forms twisted along the swirl, arrives still turning,
  // and rings square.
  const u = progress(MORPH, LAND, t);
  const rot = -0.4 * (2 * u ** 3 - 3 * u * u + 1) + (u ** 3 - u * u) * (LAND - MORPH) * 1.1;
  const angles: number[] = [];
  for (let k = 0; k < RAYS; k++) angles.push((k / RAYS) * TAU - Math.PI);
  for (const p of L.hex) angles.push(Math.atan2(p.y - hc.y, p.x - hc.x) + rot);
  const wrap = (a: number) => a - TAU * Math.floor((a + Math.PI) / TAU);
  const sorted = angles.map(wrap).sort((a, b) => a - b);
  const mc = clamp(m);
  const cx = lerp(cb.x, hc.x, mc);
  const cy = lerp(cb.y, hc.y, mc);
  const target = roundedHex(L.hex, cornerRadius(t));
  const rb = blobRadii(blob, cb);
  const out: Pt[] = [];
  for (const th of sorted) {
    const rh = rayDist(target, hc, th - rot);
    const r = rh + (1 - m) * (sampleRadii(rb, th) - rh);
    out.push({ x: cx + r * Math.cos(th), y: cy + r * Math.sin(th) });
  }
  // Rising angle runs clockwise on screen; wind it like the field contours.
  return out.reverse();
}

/** Morph weight: builds out of the exhale and lands on b27.5. */
const morphWeight = (t: number): number => (t <= MORPH ? 0 : progress(MORPH, LAND, t) ** 1.6);

// Drawing.

/** A contour ready to draw; morphed loops keep their corners sharp. */
interface Loop {
  pts: Pt[];
  sharp: boolean;
}

function trace(ctx: CanvasRenderingContext2D, loop: Loop): void {
  const pts = loop.pts;
  const n = pts.length;
  if (loop.sharp) {
    ctx.moveTo(pts[0].x, pts[0].y);
    for (let i = 1; i < n; i++) ctx.lineTo(pts[i].x, pts[i].y);
    ctx.closePath();
    return;
  }
  const last = pts[n - 1];
  ctx.moveTo((last.x + pts[0].x) / 2, (last.y + pts[0].y) / 2);
  for (let i = 0; i < n; i++) {
    const p = pts[i];
    const q = pts[(i + 1) % n];
    ctx.quadraticCurveTo(p.x, p.y, (p.x + q.x) / 2, (p.y + q.y) / 2);
  }
  ctx.closePath();
}

/** Outward unit normals of a loop wound with the inside on the left. */
function normals(loop: Pt[]): Pt[] {
  const n = loop.length;
  const out: Pt[] = [];
  for (let i = 0; i < n; i++) {
    const a = loop[(i - 1 + n) % n];
    const b = loop[(i + 1) % n];
    const tx = b.x - a.x;
    const ty = b.y - a.y;
    const l = Math.hypot(tx, ty) || 1;
    out.push({ x: -ty / l, y: tx / l });
  }
  return out;
}

/**
 * 0..1 per vertex: 1 where the rim is straight or bulges, 0 where it is concave,
 * from the turn of the rim over a few samples either side.
 */
function convexity(loop: Pt[]): number[] {
  const n = loop.length;
  const sgn = Math.sign(area(loop)) || 1;
  const out: number[] = [];
  for (let i = 0; i < n; i++) {
    const a = loop[(i - 3 + n) % n];
    const b = loop[i];
    const c = loop[(i + 3) % n];
    const cr = (b.x - a.x) * (c.y - b.y) - (b.y - a.y) * (c.x - b.x);
    const l = Math.hypot(b.x - a.x, b.y - a.y) * Math.hypot(c.x - b.x, c.y - b.y) || 1;
    out.push(smoothstep(-0.06, -0.01, (sgn * cr) / l));
  }
  // Smoothed along the rim, so a highlight tapers off into a saddle rather
  // than breaking into slivers.
  const soft: number[] = [];
  for (let i = 0; i < n; i++) {
    let m = 0;
    for (let j = -4; j <= 4; j++) m += out[(i + j + n) % n];
    soft.push((m / 9) ** 2);
  }
  return soft;
}

/**
 * Miter offsets: moving vertex `i` in by `d * m[i]` moves both adjacent
 * edges in by `d`, so an inset band stays parallel through sharp corners.
 */
function miters(loop: Pt[]): Pt[] {
  const n = loop.length;
  const en: Pt[] = [];
  for (let i = 0; i < n; i++) {
    const a = loop[i];
    const b = loop[(i + 1) % n];
    const l = Math.hypot(b.x - a.x, b.y - a.y) || 1;
    en.push({ x: -(b.y - a.y) / l, y: (b.x - a.x) / l });
  }
  const out: Pt[] = [];
  for (let i = 0; i < n; i++) {
    const n0 = en[(i - 1 + n) % n];
    const n1 = en[i];
    const k = 1 / Math.max(0.3, 1 + n0.x * n1.x + n0.y * n1.y);
    out.push({ x: (n0.x + n1.x) * k, y: (n0.y + n1.y) * k });
  }
  return out;
}

/**
 * A crescent hugging the inside of the rim wherever the surface faces `dir`:
 * reads as a specular or refracted rim on a liquid.
 */
function crescent(
  ctx: CanvasRenderingContext2D,
  loop: Pt[],
  nrm: Pt[],
  mit: Pt[],
  convex: number[],
  dir: Pt,
  lo: number,
  inset: number,
  width: number,
): void {
  const n = loop.length;
  // Lit where the rim faces `dir` and bulges out: a concave saddle between
  // two lobes would not catch a highlight along its rim.
  const w = nrm.map((q, i) => smoothstep(lo, 1, q.x * dir.x + q.y * dir.y) * convex[i]);
  // Start the walk at a point outside the lit arc so runs are contiguous.
  let s = 0;
  for (let i = 0; i < n; i++) {
    if (w[i] === 0) {
      s = i;
      break;
    }
  }
  let run: number[] = [];
  const flush = () => {
    if (run.length >= 6) {
      ctx.beginPath();
      for (let k = 0; k < run.length; k++) {
        const i = run[k];
        const x = loop[i].x - mit[i].x * inset;
        const y = loop[i].y - mit[i].y * inset;
        if (k === 0) ctx.moveTo(x, y);
        else ctx.lineTo(x, y);
      }
      // Tapered to a point at both ends, so a straight lit edge carries a
      // sliver of light rather than a square-cut bar.
      const last = run.length - 1;
      for (let k = last; k >= 0; k--) {
        const i = run[k];
        const f = k / last;
        const d = inset + width * w[i] * smoothstep(0, 0.2, f) * smoothstep(0, 0.2, 1 - f);
        ctx.lineTo(loop[i].x - mit[i].x * d, loop[i].y - mit[i].y * d);
      }
      ctx.closePath();
      ctx.fill();
    }
    run = [];
  };
  for (let k = 0; k <= n; k++) {
    const i = (s + k) % n;
    if (k < n && w[i] > 0) run.push(i);
    else flush();
  }
}

const LIGHT = { x: -0.55, y: -0.835 };
const RIM = { x: 0.62, y: 0.785 };

interface Spot {
  x: number;
  y: number;
  nx: number;
  ny: number;
  a: number;
}

/**
 * Where a second, smaller glint sits: the light-weighted mean of the rim,
 * which is stable on round bodies and centers on a flat lit edge. Fades out
 * when the lit rim splits across two lobes.
 */
function glintSpot(pts: Pt[], nrm: Pt[], req: number): Spot | null {
  let sw = 0;
  let sx = 0;
  let sy = 0;
  let snx = 0;
  let sny = 0;
  const ws = nrm.map((q) => Math.max(0, q.x * LIGHT.x + q.y * LIGHT.y) ** 16);
  for (let i = 0; i < pts.length; i++) {
    const w = ws[i];
    sw += w;
    sx += w * pts[i].x;
    sy += w * pts[i].y;
    snx += w * nrm[i].x;
    sny += w * nrm[i].y;
  }
  if (sw < 1e-6) return null;
  const mx = sx / sw;
  const my = sy / sw;
  let v = 0;
  for (let i = 0; i < pts.length; i++) v += ws[i] * ((pts[i].x - mx) ** 2 + (pts[i].y - my) ** 2);
  const spread = Math.sqrt(v / sw);
  const nl = Math.hypot(snx, sny) || 1;
  return {
    x: mx,
    y: my,
    nx: snx / nl,
    ny: sny / nl,
    a: 1 - smoothstep(0.3 * req, 0.5 * req, spread),
  };
}

/** Light where a join happens, lit inside the liquid. */
interface Flare {
  x: number;
  y: number;
  a: number;
}

/** Warm cream between bright amber and paper: light on the liquid that stays in the palette. */
const FLARE = mix(PALETTE.amberBright, PALETTE.paper, 0.5, 1);
const FLARE_CLEAR = mix(PALETTE.amberBright, PALETTE.paper, 0.5, 0);

/**
 * Draw the liquid; `firm` (0..1) is how far it has set, which retires the
 * round-body glint, and `flares` light it from inside at each join.
 */
function drawLiquid(
  ctx: CanvasRenderingContext2D,
  loops: Loop[],
  s: number,
  firm: number,
  flares: Flare[] = [],
): void {
  if (s <= 0.001) {
    ctx.beginPath();
    for (const l of loops) trace(ctx, l);
    ctx.fillStyle = PALETTE.amber;
    ctx.fill();
    return;
  }
  for (const loop of loops) {
    const pts = loop.pts;
    const req = Math.sqrt(Math.abs(area(pts)) / Math.PI);
    let x0 = Infinity;
    let y0 = Infinity;
    let x1 = -Infinity;
    let y1 = -Infinity;
    for (const p of pts) {
      x0 = Math.min(x0, p.x);
      y0 = Math.min(y0, p.y);
      x1 = Math.max(x1, p.x);
      y1 = Math.max(y1, p.y);
    }
    ctx.save();
    ctx.beginPath();
    trace(ctx, loop);
    // Lit from the upper left, deepening toward the lower right.
    const g = ctx.createLinearGradient(x0, y0, x1, y1);
    g.addColorStop(0, mix(PALETTE.amber, PALETTE.amberBright, 0.7 * s));
    g.addColorStop(0.5, PALETTE.amber);
    g.addColorStop(1, mix(PALETTE.amber, PALETTE.amberShade, 0.75 * s));
    ctx.fillStyle = g;
    ctx.fill();
    ctx.clip();
    // Light passing through a deep body pools toward the lower right, as in
    // honey; small drops are too thin to show it.
    const c = centroid(pts);
    const deep = 0.22 * s * smoothstep(40, 95, req);
    const hx = c.x + RIM.x * req * 0.35;
    const hy = c.y + RIM.y * req * 0.35;
    glow(ctx, hx, hy, req * 1.15, PALETTE.amberBright, deep);
    // Join flares, painted over (not added to) the amber so they never
    // clip to a lemon yellow.
    const alpha = ctx.globalAlpha;
    for (const f of flares) {
      const r = 64;
      if (f.x < x0 - r || f.x > x1 + r || f.y < y0 - r || f.y > y1 + r) continue;
      const fg = ctx.createRadialGradient(f.x, f.y, 0, f.x, f.y, r);
      fg.addColorStop(0, FLARE);
      fg.addColorStop(1, FLARE_CLEAR);
      ctx.globalAlpha = alpha * 0.7 * f.a;
      ctx.fillStyle = fg;
      ctx.fillRect(f.x - r, f.y - r, 2 * r, 2 * r);
    }
    ctx.globalAlpha = alpha;
    // A soft, darker meniscus just inside the edge: three stacked bands.
    ctx.lineJoin = "round";
    ctx.strokeStyle = rgba(PALETTE.amberShade, 0.075 * s);
    for (const k of [0.56, 0.32, 0.14]) {
      ctx.lineWidth = Math.min(k * 60, req * k);
      ctx.stroke();
    }
    const nrm = normals(pts);
    const mit = miters(pts);
    const cvx = convexity(pts);
    // Warm light refracted through the body, catching the lower right rim.
    ctx.fillStyle = rgba(PALETTE.amberBright, 0.6 * s);
    crescent(ctx, pts, nrm, mit, cvx, RIM, 0.35, Math.min(3, req * 0.06), Math.min(5, req * 0.1));
    // Specular band, upper left. On the forming box, only the edges that
    // truly face the light carry it.
    ctx.fillStyle = rgba(PALETTE.paper, 0.85 * s);
    const inset = Math.min(9, req * 0.16);
    const width = Math.min(9, req * 0.15);
    crescent(ctx, pts, nrm, mit, cvx, LIGHT, loop.sharp ? 0.6 : 0.45, inset, width);
    // A small second glint just under the band, on round bodies big enough
    // to hold it; it goes as the liquid sets flat.
    const round = 1 - smoothstep(0.12, 0.4, firm);
    if (req > 30 && round > 0) {
      const gs = glintSpot(pts, nrm, req);
      if (gs && gs.a > 0.01) {
        const off = inset + width + Math.min(11, req * 0.11);
        ctx.fillStyle = rgba(PALETTE.paper, 0.85 * s * gs.a * round);
        ctx.beginPath();
        ctx.arc(gs.x - gs.nx * off, gs.y - gs.ny * off, Math.min(4.5, req * 0.04), 0, TAU);
        ctx.fill();
      }
    }
    ctx.restore();
  }
}

export const scene: Scene = {
  id: "morph",
  start: bar(6),
  end: bar(7),
  draw(ctx, lt, env) {
    const t = lt;
    const L = layout();
    // bg → night, so the end card's black arrives already.
    const dark = smoothstep(0.2, 1.55, t);
    ctx.fillStyle = dark >= 1 ? PALETTE.night : mix(PALETTE.bg, PALETTE.night, dark);
    ctx.fillRect(0, 0, env.W, env.H);

    if (t >= SETTLED) {
      ctx.beginPath();
      ctx.moveTo(L.hex[0].x, L.hex[0].y);
      for (const p of L.hex) ctx.lineTo(p.x, p.y);
      ctx.closePath();
      ctx.fillStyle = PALETTE.amber;
      ctx.fill();
      return;
    }

    const s = shade(t);
    if (t >= LAND) {
      // Landed: the exact silhouette rings down as a jelly while the gloss drains.
      glow(ctx, L.hc.x, L.hc.y, 380, PALETTE.amber, 0.1 * s);
      drawLiquid(ctx, [{ pts: jellyHex(t), sharp: true }], s, 1);
      return;
    }

    const fr = motion(t);
    const z = zoom(t);
    ctx.save();
    ctx.translate(fr.a.x, fr.a.y);
    ctx.scale(z, z);
    ctx.translate(-fr.a.x, -fr.a.y);

    // Warm light the liquid throws onto the stage, swelling on the merge.
    const halo = s * (0.1 + (t >= MERGE ? 0.1 * Math.exp(-(t - MERGE) * 4) : 0));
    glow(ctx, fr.a.x, fr.a.y, 380, PALETTE.amber, halo);

    // The slam, the breath, a slow travelling surface wave, and the splash
    // bumps act on the contour about the blob center.
    const sy = 1 + slam(t) + breath(t);
    const swell = 1 + 0.3 * breath(t);
    const kx = swell / Math.sqrt(sy);
    const ky = swell * sy;
    // The wave swells with the breath, a beat behind it.
    const wave =
      (0.035 * smoothstep(SECOND, MERGE + 0.1, t) + 0.25 * Math.abs(breath(t - 0.08))) *
      (1 - smoothstep(LAND - 0.14, LAND - 0.03, t));
    const ph = t * 7;
    const sp = splashes();
    const live = sp.filter((e) => t > e.t && t < e.t + 0.8);
    const raw = contour(fr.balls, fr.segs);
    let main = -1;
    raw.forEach((l, i) => {
      if (main < 0 && inside(l, fr.a)) main = i;
    });
    // The wave and bumps ride the core's rim only; anything reaching out
    // past it (lobes, arms, the drop and its thread) keeps its own shape.
    const near = fr.R * 1.15;
    const far = fr.R * 1.6;
    const loops: Loop[] = raw.map((l) => {
      let pts = resample(l, 5);
      pts = pts.map((p) => {
        const dx = p.x - fr.a.x;
        const dy = p.y - fr.a.y;
        const th = Math.atan2(dy, dx);
        const rim = 1 - smoothstep(near, far, Math.hypot(dx, dy));
        let k = 0;
        if (wave > 0) k += wave * (Math.sin(3 * th - ph) + 0.6 * Math.sin(2 * th + ph * 0.7 + 1));
        for (const e of live) {
          const d = t - e.t;
          // Rises as the two halves part, so the join itself never peaks.
          const amp = e.amp * smoothstep(0.01, 0.09, d) * Math.exp(-d * 4.5);
          const c = e.angle(t);
          const run = 6.5 * d;
          for (const side of [run, -run]) {
            let q = th - c - side;
            q -= TAU * Math.round(q / TAU);
            k += amp * Math.exp(-(q * q) / 0.18);
          }
        }
        const f = 1 + k * rim;
        return { x: fr.a.x + dx * kx * f, y: fr.a.y + dy * ky * f };
      });
      return { pts, sharp: false };
    });
    if (main < 0) {
      main = 0;
      for (let i = 1; i < loops.length; i++)
        if (Math.abs(area(loops[i].pts)) > Math.abs(area(loops[main].pts))) main = i;
    }

    // Only the blob morphs.
    const m = morphWeight(t);
    if (m > 0 && loops.length) loops[main] = { pts: morphLoop(loops[main].pts, t, m), sharp: true };

    // Light flares where each join happens: a bloom spilling onto the
    // stage behind the liquid, and a hot spot inside it.
    const flares: Flare[] = [];
    for (const e of sp) {
      const d = t - e.t;
      if (d < -0.02 || d > 0.2) continue;
      const k =
        progress(-0.02, 0, d) * Math.exp(-Math.max(0, d) / 0.06) * (1 - progress(0.1, 0.2, d));
      const c = e.angle(t);
      // Placed like the contour: it rides the slam and the breath.
      const gx = fr.a.x + Math.cos(c) * e.dist * kx;
      const gy = fr.a.y + Math.sin(c) * e.dist * ky;
      glow(ctx, gx, gy, 90, PALETTE.amber, 0.55 * k * s);
      flares.push({ x: gx, y: gy, a: k * s });
    }
    drawLiquid(ctx, loops, s, m, flares);
    ctx.restore();
  },
};
