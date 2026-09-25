// Scene 2, "Character". The closed box from the fold comes alive: it rocks
// back onto its heel, leaps on a spinning arc to center, lands with a squash,
// and then the face assembles one feature per sixteenth under a push-in,
// until it wears the logo pose for the monocle dive.

import { BEAT, H1_POSE, HERO_CAM, PALETTE, type Scene, sec } from "../bible";
import {
  type BoxFrame,
  type BoxPose,
  boxFrame,
  boxPoint,
  boxSilhouette,
  CREAM,
  drawBox,
  type FaceParams,
  faceToScreen,
  INK,
  monoclePaths,
  monocleScreen,
  OUTLINE_RATIO,
  panelMatrix,
  sparkle,
} from "../box";
import { mix, rgba } from "../color";
import { glow, shake } from "../fx";
import {
  clamp,
  cubicBezier,
  DEG,
  inCubic,
  inOutSine,
  inQuad,
  keys,
  lerp,
  outCubic,
  outQuad,
  progress,
  pulse,
  rng,
  smoothstep,
  spring,
  swiftInOut,
  TAU,
  wobble,
} from "../math";
import {
  applyMatrix,
  type Camera,
  type DOMMatrix2D,
  mixCamera,
  type V3,
  View,
} from "../space";

const S = sec("mr-boxington");

// Accents in local time. The score (score/character.ts) is written to these.
const at = (b: number) => b * BEAT;
export const LAUNCH = at(0.5);
const APEX = at(1);
export const LAND = at(1.5);
export const EYES = at(2);
export const BROWS = at(2.25);
export const MUST = at(2.5);
export const LABEL = at(2.75);
export const MONO = at(3);
export const GLINT = at(3.25);
export const TIE = at(3.5);
export const BLINK = at(3.75);

/** Pops start one frame early with a kick, so the anchor frame already reads. */
const LEAD = 1 / 60;
/** Every residual wobble is blended to rest over this window before the handoff. */
const FIN0 = S.len - 0.095;
const FIN1 = S.len - 0.023;

const FLOOR = H1_POSE.pos[1];
const IMPACT = 0.045;
const S2 = Math.SQRT1_2;

type Pt = { x: number; y: number };

// Camera: a lagging crane with the leap that is home again by touchdown, a
// push toward the face while it assembles, and a pull back to the logo
// framing that finishes before the blink, so the last beat plays on a locked
// camera ahead of the dive.

const PUSH_CAM: Camera = { ...HERO_CAM, scale: 380, target: [0, 0.08, 0.2] };
const PUSH0 = 0.86;
const PUSH1 = 1.41;
const PULL0 = GLINT + 0.02;
const PULL1 = BLINK - 0.025;
const pullEase = cubicBezier(0.45, 0, 0.25, 1);
const CRANE = 40;
const craneAt = keys([
  [LAUNCH, 0],
  [APEX + 0.02, 1, inOutSine],
  [LAND - 0.01, 0, inOutSine],
]);

function cameraAt(lt: number): Camera {
  const push =
    swiftInOut(progress(PUSH0, PUSH1, lt)) * (1 - pullEase(progress(PULL0, PULL1, lt)));
  const cam = mixCamera(HERO_CAM, PUSH_CAM, push);
  cam.cy += CRANE * craneAt(lt);
  return cam;
}

// Body. On the ground it rocks about whichever bottom corner is lowest,
// pinned where it stood, so leaning never slides it. In the air its center
// flies a true parabola and every tilt pivots about that center.

/** Rocking back onto the heel is what carries the center off to the left. */
const ROCK = 27 * DEG;
/** Center rise above the straight line from takeoff to touchdown. */
const LIFT = 0.74;
const LAND_SQUASH = 1.1;

/** Lean toward screen right: rears back onto the heel, throws itself over, rights itself to land. */
const leanAt = keys([
  [0.03, 0],
  [0.2, -ROCK, inOutSine],
  [LAUNCH, -ROCK * 1.04],
  [LAUNCH + 0.1, 5 * DEG, inOutSine],
  [APEX, 9 * DEG, inOutSine],
  [LAND - 0.05, 0, inOutSine],
]);
/** Lean toward the viewer, peaking a little later, so the bank precesses. */
const pitchAt = keys([
  [LAUNCH, 0],
  [APEX + 0.04, 7 * DEG, inOutSine],
  [LAND - 0.05, 0, inOutSine],
]);

const crouch = keys([
  [0, 1],
  [0.19, 0.82, inOutSine],
  [LAUNCH, 0.8],
  [LAUNCH + 0.045, 1.15, outQuad],
  [APEX - 0.02, 0.92, inOutSine],
  [LAND, LAND_SQUASH, inQuad],
]);

function squashAt(lt: number): number {
  if (lt < LAND) return crouch(lt);
  const d = lt - LAND;
  if (d < IMPACT) return lerp(LAND_SQUASH, 0.78, outQuad(d / IMPACT));
  let s = 0.78 + 0.22 * spring(d - IMPACT, 2.8, 0.36);
  // Small body reactions as features arrive.
  s += 0.07 * wobble(lt, EYES - LEAD, 3.2, 7);
  s -= 0.045 * pulse(lt, LABEL, 0.012, 0.022);
  s -= 0.025 * wobble(lt, MONO, 5, 10);
  s += 0.02 * wobble(lt, TIE, 4, 9);
  return s;
}

const WIND = -14 * DEG;
const SPIN_K = 0.35;
const FLIGHT = LAND - LAUNCH;

function yawAt(lt: number): number {
  if (lt < LAUNCH) return WIND * inOutSine(progress(0.02, LAUNCH, lt));
  if (lt < LAND) {
    const p = (lt - LAUNCH) / FLIGHT;
    // Fast off the push, easing toward touchdown but still turning.
    return lerp(WIND, TAU, p + SPIN_K * p * (1 - p));
  }
  // Friction stops the spin at touchdown: it twists past square and recoils.
  const vLand = ((TAU - WIND) * (1 - SPIN_K)) / FLIGHT;
  const f = 3.2;
  return (vLand / (TAU * f)) * wobble(lt, LAND, f, 11);
}

/** Yaw rate in the air, radians per second. */
function spinRate(lt: number): number {
  const p = clamp((lt - LAUNCH) / FLIGHT);
  return ((TAU - WIND) * (1 + SPIN_K * (1 - 2 * p))) / FLIGHT;
}

/** Squash, yaw, and tilts, with the box's bottom center at the origin of the floor. */
function trackPose(lt: number): BoxPose {
  const fin = smoothstep(FIN0, FIN1, lt);
  // Momentum tips it on along the travel as it lands, then it rocks back.
  const lean = leanAt(lt) + 0.07 * wobble(lt, LAND + 0.01, 2.6, 7);
  const pitch = pitchAt(lt);
  // Lean and pitch in world tilts; the head lifts as the eyes open, and the
  // stamp shoves the right panel so the box rocks away and back.
  const tx = S2 * (pitch - lean) - 0.045 * wobble(lt, EYES - LEAD, 2.4, 5);
  const tz = -S2 * (lean + pitch) + 0.06 * wobble(lt, LABEL, 4, 7);
  return {
    ...H1_POSE,
    pos: [0, FLOOR, 0],
    squash: lerp(squashAt(lt), 1, fin),
    yaw: lerp(yawAt(lt), 0, fin),
    tiltX: lerp(tx, 0, fin),
    tiltZ: lerp(tz, 0, fin),
  };
}

const ORIGIN: V3 = [0, 0, 0];

/** The lowest bottom corner, as local [x, z] signs. */
function lowestCorner(f: BoxFrame): [number, number] {
  let best = Infinity;
  let pick: [number, number] = [-1, 1];
  for (const i of [-1, 1]) {
    for (const k of [-1, 1]) {
      const y = boxPoint(f, i, -1, k)[1];
      if (y < best - 1e-9) {
        best = y;
        pick = [i, k];
      }
    }
  }
  return pick;
}

/** Standing: tilted about the lowest bottom corner, which stays where it stood. */
function groundPose(base: BoxPose): BoxPose {
  const tilted = boxFrame({ ...base, pos: ORIGIN });
  const flat = boxFrame({ ...base, pos: ORIGIN, tiltX: 0, tiltZ: 0 });
  const [i, k] = lowestCorner(tilted);
  const t = boxPoint(tilted, i, -1, k);
  const u = boxPoint(flat, i, -1, k);
  return { ...base, pos: [u[0] - t[0], FLOOR - t[1], u[2] - t[2]] };
}

let takeoff: { c: V3; heel: V3 } | null = null;
/** Center and heel at the takeoff frame, where the air arc starts. */
function takeoffState(): { c: V3; heel: V3 } {
  if (takeoff) return takeoff;
  const pose = groundPose(trackPose(LAUNCH));
  const f = boxFrame(pose);
  const [i, k] = lowestCorner(f);
  takeoff = { c: f.c, heel: boxPoint(f, i, -1, k) };
  return takeoff;
}

/** Airborne: the center on a ballistic arc from takeoff to the landing spot. */
function airPose(lt: number, base: BoxPose): BoxPose {
  const p = (lt - LAUNCH) / FLIGHT;
  const c0 = takeoffState().c;
  const y1 = FLOOR + 0.5 * LAND_SQUASH;
  const c: V3 = [
    lerp(c0[0], 0, p),
    lerp(c0[1], y1, p) + LIFT * 4 * p * (1 - p),
    lerp(c0[2], 0, p),
  ];
  const f = boxFrame({ ...base, pos: ORIGIN });
  const pos: V3 = [c[0] - f.c[0], c[1] - f.c[1], c[2] - f.c[2]];
  // The floor still holds the heel through the push-off frames, where the
  // stretch reaches down.
  const [i, k] = lowestCorner(f);
  const under = FLOOR - (pos[1] + boxPoint(f, i, -1, k)[1]);
  if (under > 0) pos[1] += under;
  return { ...base, pos };
}

function bodyPose(lt: number): BoxPose {
  const base = trackPose(lt);
  return lt > LAUNCH && lt < LAND ? airPose(lt, base) : groundPose(base);
}

// Face.

const pop = (lt: number, anchor: number, f: number, z: number, kick = 22) =>
  spring(lt - anchor + LEAD, f, z, kick);

/** Opaque from its first frame a third too big, then slammed flat on the anchor. */
const STAMP = 0.034;
const labelAt = (lt: number) =>
  lt < LABEL - STAMP ? 0 : lerp(0.5, 1, inCubic(progress(LABEL - STAMP, LABEL, lt)));

const blinkAt = keys([
  [BLINK - 0.035, 0],
  [BLINK, 1, inQuad],
  [BLINK + 0.017, 1],
  [BLINK + 0.075, 0, outQuad],
]);

/** A small star on the rim: rises in two frames, a quick "ting" of a decay. */
const GLINT_PEAK = 0.55;
function glintAt(lt: number): number {
  if (lt < GLINT) return GLINT_PEAK * outQuad(progress(GLINT - 0.034, GLINT, lt));
  return GLINT_PEAK * (1 - outQuad(progress(GLINT, GLINT + 0.12, lt)));
}

function browLiftAt(lt: number): number {
  // In low, then snapped up through rest on the anchor.
  let lift = -7 * (1 - spring(lt - (BROWS - LEAD), 5, 0.45, 25));
  // Raised watching the monocle drop; the clink knocks them down, then back.
  if (lt < MONO) lift += 2 * smoothstep(MONO - 0.13, MONO - 0.03, lt);
  else lift -= 4.2 * (1 - spring(lt - MONO, 6, 0.38));
  return lift;
}

// Eyes glance down at the stamp, up at the monocle, and follow it home.
const lookX = keys([
  [LABEL - 0.06, 0],
  [LABEL - 0.025, 4.5, outCubic],
  [LABEL + 0.03, 4.5],
  [LABEL + 0.07, 3, outCubic],
  [MONO - 0.01, 1.5, inOutSine],
  [MONO + 0.06, 0, outCubic],
]);
const lookY = keys([
  [LABEL - 0.06, 0],
  [LABEL - 0.025, 2.5, outCubic],
  [LABEL + 0.03, 2.5],
  [LABEL + 0.07, -5, outCubic],
  [MONO - 0.01, -2, inOutSine],
  [MONO + 0.06, 0, outCubic],
]);

// The bow tie whirls one decelerating turn from the anchor, crosses square
// before the blink still turning, and a stiff spring takes up the overshoot.
const TIE_SPIN = 0.095;
const TIE_K = 0.25;
const TIE_F = 5.5;
function tieSpin(lt: number): number {
  if (lt < TIE + TIE_SPIN) {
    const p = progress(TIE, TIE + TIE_SPIN, lt);
    return -TAU * (1 - (TIE_K * p + (1 - TIE_K) * p * (2 - p)));
  }
  const v = (TAU * TIE_K) / TIE_SPIN;
  return (v / (TAU * TIE_F)) * wobble(lt, TIE + TIE_SPIN, TIE_F, 16);
}

function faceAt(lt: number): FaceParams {
  const fin = smoothstep(FIN0, FIN1, lt);
  const blink = blinkAt(lt);
  return {
    eyes: lerp(pop(lt, EYES, 4.2, 0.42, 30), 1, fin),
    // Scales in four frames early, overshooting as the flick lands.
    brows: lerp(spring(lt - (BROWS - 0.067), 6, 0.5, 20), 1, fin),
    browLift: lerp(browLiftAt(lt) - 3 * blink, 0, fin),
    blink,
    look: [lerp(lookX(lt), 0, fin), lerp(lookY(lt), 0, fin)],
    monocle: lt >= MONO_SWITCH ? 1 : 0,
    glint: glintAt(lt),
    mustache: lerp(pop(lt, MUST, 3.6, 0.42, 30), 1, fin),
    twitch: lerp(0.32 * wobble(lt, MUST + 0.05, 6.5, 6.5), 0, fin),
    // Half size on the anchor frame and most of the way two frames later;
    // the cap keeps the wings on the panel while the spin carries the energy.
    bowtie: lerp(Math.min(pop(lt, TIE, 5, 0.62, 30), 1.04), 1, fin),
    bowtieSpin: lerp(tieSpin(lt), 0, fin),
  };
}

// The monocle is flicked up from behind the lid once the mustache has had
// its boing, hangs at the top of its arc over the stamp, and drops into the
// seat, turning into the face plane.

const MONO_TOP = MONO - 0.14;
/** The monocle is flicked up from behind the lid. */
export const MONO_UP = MONO_TOP - 0.05;
const MONO_SWITCH = MONO + 0.08;
/** Arc relative to the seat, in px at HERO_CAM scale. */
const ARC_X = 125;
const UP_Y = -120;
const TOP_Y = -305;
const REBOUND_LEN = 110;

/** Face-plane matrix with its origin moved to the seated ring center. */
function seatMatrix(view: View, f: BoxFrame): DOMMatrix2D {
  const m = panelMatrix(view, f, "face");
  return { ...m, e: m.a * 18 - m.c * 28 + m.e, f: m.b * 18 - m.d * 28 + m.f };
}

function mulRot(m: DOMMatrix2D, x: number, y: number, rot: number): DOMMatrix2D {
  const c = Math.cos(rot);
  const n = Math.sin(rot);
  return {
    a: m.a * c + m.c * n,
    b: m.b * c + m.d * n,
    c: -m.a * n + m.c * c,
    d: -m.b * n + m.d * c,
    e: m.a * x + m.c * y + m.e,
    f: m.b * x + m.d * y + m.f,
  };
}

function arcAt(lt: number): [number, number] {
  const x = ARC_X * (1 - progress(MONO_UP, MONO, lt));
  const y =
    lt < MONO_TOP
      ? lerp(UP_Y, TOP_Y, outCubic(progress(MONO_UP, MONO_TOP, lt)))
      : TOP_Y * (1 - inQuad(progress(MONO_TOP, MONO, lt)));
  return [x, y];
}

/** Ring-centered transform for the flying monocle, or null when drawBox owns it. */
function monoAt(
  view: View,
  f: BoxFrame,
  lt: number,
): { m: DOMMatrix2D; behind: boolean } | null {
  if (lt < MONO_UP || lt >= MONO_SWITCH) return null;
  const seat = seatMatrix(view, f);
  if (lt >= MONO) {
    // The seat stops it dead: the rim flattens for a frame, one small
    // pendulum rebound, then exactly home.
    const settle = 1 - smoothstep(MONO + 0.045, MONO + 0.075, lt);
    const th = 0.1 * wobble(lt, MONO, 7, 10) * settle;
    const rx = -REBOUND_LEN * Math.sin(th);
    const ry = -REBOUND_LEN * (1 - Math.cos(th));
    const m = mulRot(seat, rx, ry, -th * 0.9);
    const sq = 0.12 * Math.exp(-(lt - MONO) / 0.018) * settle;
    const wx = 1 + sq * 0.5;
    const wy = 1 - sq;
    return { m: { ...m, a: m.a * wx, b: m.b * wx, c: m.c * wy, d: m.d * wy }, behind: false };
  }
  const k = view.cam.scale / HERO_CAM.scale;
  const [px, py] = arcAt(lt);
  const [qx, qy] = arcAt(lt - 1 / 60);
  const vx = px - qx;
  const vy = py - qy;
  const speed = Math.hypot(vx, vy);

  // The seat's linear part as rotation, uniform scale, and a unit shear, so
  // the ring can fly flat to the screen and take on the panel's shear last
  // without ever collapsing.
  const th0 = Math.atan2(seat.b, seat.a);
  const c0 = Math.cos(th0);
  const s0 = Math.sin(th0);
  const sx = Math.hypot(seat.a, seat.b);
  const kap = c0 * seat.c + s0 * seat.d;
  const sy = -s0 * seat.c + c0 * seat.d;
  const unit = Math.sqrt(Math.abs(sx * sy));
  const fall = progress(MONO_TOP, MONO, lt);
  const sh = smoothstep(0.3, 1, fall);
  const ua = lerp(1, sx / unit, sh);
  const uc = lerp(0, kap / unit, sh);
  const ud = lerp(1, sy / unit, sh);
  // Behind the box it is farther away; over the lid it is nearest the lens.
  const grow =
    lt < MONO_TOP
      ? lerp(0.85, 1.22, outQuad(progress(MONO_UP, MONO_TOP, lt)))
      : lerp(1.22, 1, inQuad(fall));
  const scl = unit * grow;
  // One full tumble in flight, finishing square to the seat.
  const rot = th0 - TAU * (1 - progress(MONO_UP, MONO, lt));
  const cr = Math.cos(rot) * scl;
  const sr = Math.sin(rot) * scl;
  let a = cr * ua;
  let b = sr * ua;
  let c = cr * uc - sr * ud;
  let d = sr * uc + cr * ud;
  let e = seat.e + px * k;
  let g = seat.f + py * k;
  // Directional smear on the fastest frames: stretched along the velocity,
  // with the leading edge where the ring really is.
  const sig = 1 + 0.75 * smoothstep(40, 68, speed);
  if (sig > 1.001) {
    const ux = vx / speed;
    const uy = vy / speed;
    const ac = 1 / Math.sqrt(sig);
    const s11 = sig * ux * ux + ac * uy * uy;
    const s12 = (sig - ac) * ux * uy;
    const s22 = sig * uy * uy + ac * ux * ux;
    [a, b, c, d] = [s11 * a + s12 * b, s12 * a + s22 * b, s11 * c + s12 * d, s12 * c + s22 * d];
    const back = (sig - 1) * 19 * scl;
    e -= ux * back;
    g -= uy * back;
  }
  return { m: { a, b, c, d, e, f: g }, behind: lt < MONO_TOP };
}

// The monocle in flight, with box.ts's paths and drawFace's stroke widths,
// so the hand-over to drawBox's seated monocle is invisible.
function drawMonocle(ctx: CanvasRenderingContext2D, m: DOMMatrix2D) {
  const p = monoclePaths();
  ctx.save();
  applyMatrix(ctx, m);
  ctx.translate(-18, 28);
  ctx.lineCap = "round";
  ctx.lineJoin = "round";
  ctx.strokeStyle = INK;
  ctx.lineWidth = 5;
  ctx.stroke(p.ring);
  ctx.strokeStyle = rgba(CREAM, 0.7);
  ctx.lineWidth = 3;
  ctx.stroke(p.glint);
  ctx.strokeStyle = INK;
  ctx.lineWidth = 3.5;
  ctx.stroke(p.chain);
  ctx.fillStyle = INK;
  ctx.fill(p.bead);
  ctx.restore();
}

/** Over the dark background only, a hairline of warm rim light keeps the ink ring legible. */
function monocleRim(ctx: CanvasRenderingContext2D, m: DOMMatrix2D, sil: Pt[]) {
  const s = Math.sqrt(Math.abs(m.a * m.d - m.b * m.c));
  if (s <= 0) return;
  ctx.save();
  clipOutside(ctx, sil);
  applyMatrix(ctx, m);
  ctx.beginPath();
  ctx.arc(0, 0, 19 + 0.75 / s, 0, TAU);
  ctx.strokeStyle = rgba(PALETTE.amberBright, 0.45);
  ctx.lineWidth = 1.5 / s;
  ctx.stroke();
  ctx.beginPath();
  ctx.arc(0, 0, 14 - 0.75 / s, 0, TAU);
  ctx.strokeStyle = rgba(PALETTE.amberBright, 0.22);
  ctx.stroke();
  ctx.restore();
}

// Silhouette helpers: the outline stroke sits half outside the hull, so the
// hull is inflated by that much before anything clips against it.

function inflate(pts: Pt[], w: number): Pt[] {
  const n = pts.length;
  if (n < 3) return pts;
  let area = 0;
  for (let i = 0; i < n; i++) {
    const p = pts[i];
    const q = pts[(i + 1) % n];
    area += p.x * q.y - q.x * p.y;
  }
  const sgn = area > 0 ? 1 : -1;
  const normal = (i: number): [number, number] => {
    const p = pts[i];
    const q = pts[(i + 1) % n];
    const l = Math.hypot(q.x - p.x, q.y - p.y) || 1;
    return [(sgn * (q.y - p.y)) / l, (-sgn * (q.x - p.x)) / l];
  };
  return pts.map((p, i) => {
    const [ax, ay] = normal((i - 1 + n) % n);
    const [bx, by] = normal(i);
    const k = w / Math.max(1 + ax * bx + ay * by, 0.2);
    return { x: p.x + (ax + bx) * k, y: p.y + (ay + by) * k };
  });
}

/** Signed distance from a convex outline, negative inside. */
function outsideBy(pts: Pt[], x: number, y: number): number {
  const n = pts.length;
  let area = 0;
  for (let i = 0; i < n; i++) {
    const p = pts[i];
    const q = pts[(i + 1) % n];
    area += p.x * q.y - q.x * p.y;
  }
  const sgn = area > 0 ? 1 : -1;
  let d = -Infinity;
  for (let i = 0; i < n; i++) {
    const p = pts[i];
    const q = pts[(i + 1) % n];
    const l = Math.hypot(q.x - p.x, q.y - p.y) || 1;
    d = Math.max(d, (sgn * ((q.y - p.y) * (x - p.x) - (q.x - p.x) * (y - p.y))) / l);
  }
  return d;
}

/** Clip to everything outside the silhouette. */
function clipOutside(ctx: CanvasRenderingContext2D, sil: Pt[]) {
  ctx.beginPath();
  ctx.rect(-400, -400, 2720, 1880);
  ctx.moveTo(sil[0].x, sil[0].y);
  for (let i = 1; i < sil.length; i++) ctx.lineTo(sil[i].x, sil[i].y);
  ctx.closePath();
  ctx.clip("evenodd");
}

// Dust: kicked out along the floor from under the base. Each jet is a low
// skirt of domes whose footprints lie in the floor, emitted from a stretch
// of the footprint's edge (a skirt) or fanned from a corner (a skid). The
// domes race out, are held back by drag, and lift only a little. A jet fades
// as one translucent shape while its domes shrink onto their leading edges,
// and it is gone before any dome can come loose.

interface Lobe {
  /** Emission point on the footprint, in half-extents. */
  x: number;
  z: number;
  dx: number;
  dz: number;
  reach: number;
  r: number;
  /** How far it shrinks by the end of its life. */
  shrink: number;
  die: number;
}

interface Jet {
  life: number;
  delay: number;
  lobes: Lobe[];
}

interface JetSpec {
  /** Emission stretch on the footprint, [x, z] in half-extents (a point for a corner). */
  a: [number, number];
  b: [number, number];
  /** Floor direction at a and at b: 135 is screen left, -45 screen right, 45 straight at the viewer. */
  degA: number;
  degB: number;
  n: number;
  reach: number;
  size: number;
  life: number;
  delay?: number;
  /** Bigger domes riding on a skirt, which sink back into it as it erodes. */
  bumps?: number;
}

// Takeoff: squeezed out behind the heel it pushed off from, a small skid
// back along the floor.
const LAUNCH_DUST: JetSpec[] = [
  { a: [0, 0], b: [0, 0], degA: 150, degB: 115, n: 6, reach: 0.3, size: 0.056, life: 0.16 },
];
// Touchdown: a skirt out of both front edges and a skid off each side
// corner, longest to the right where the momentum carries it. Skirt domes
// sit closer than their radius, so the ridge stays whole as it erodes.
const LAND_DUST: JetSpec[] = [
  { a: [1, -1], b: [1, -1], degA: -30, degB: -48, n: 6, reach: 0.4, size: 0.064, life: 0.17 },
  { a: [-1, 1], b: [-1, 1], degA: 120, degB: 140, n: 5, reach: 0.3, size: 0.056, life: 0.16 },
  { a: [-0.9, 1], b: [0.9, 1], degA: 90, degB: 90, n: 19, reach: 0.16, size: 0.05, life: 0.15, delay: 0.006, bumps: 5 },
  { a: [1, 0.9], b: [1, -0.9], degA: 0, degB: 0, n: 19, reach: 0.2, size: 0.052, life: 0.16, delay: 0.006, bumps: 5 },
];
const DUST_TINT = mix(PALETTE.text3, PALETTE.amberShade, 0.35);
const DUST_BASE = mix(DUST_TINT, PALETTE.bg, 0.5);
const DUST_TOP = mix(DUST_TINT, PALETTE.bg, 0.22);
const DUST_ALPHA = 0.72;
const DRAG = 0.04;

const jetCache = new Map<JetSpec[], Jet[]>();
function jets(specs: JetSpec[], seed: number): Jet[] {
  let list = jetCache.get(specs);
  if (list) return list;
  const r = rng(seed);
  list = specs.map((s) => {
    const lobes: Lobe[] = [];
    const corner = s.a[0] === s.b[0] && s.a[1] === s.b[1];
    const along = (u: number) => [lerp(s.a[0], s.b[0], u), lerp(s.a[1], s.b[1], u)];
    for (let i = 0; i < s.n; i++) {
      const u = s.n === 1 ? 0.5 : i / (s.n - 1);
      // A skid is a streak with its biggest dome at the head; a skirt is a
      // ridge, fullest mid-edge.
      const belly = 1 - (2 * u - 1) ** 2;
      const a = (corner ? lerp(s.degA, s.degB, r()) : s.degA) * DEG + (r() - 0.5) * 0.08;
      const [x, z] = along(u);
      lobes.push({
        x,
        z,
        dx: Math.cos(a),
        dz: Math.sin(a),
        reach: s.reach * (corner ? lerp(0.5, 1, u) : lerp(0.85, 1, 0.5 * belly + 0.5 * r())),
        r: s.size * (corner ? lerp(0.65, 1.1, u) : 1) * lerp(0.9, 1.1, r()),
        shrink: 0.3,
        die: lerp(0.92, 1.06, r()),
      });
    }
    // Bumps: spaced along the skirt with jitter, some leading and some
    // lagging, so the crest is irregular.
    const nb = s.bumps ?? 0;
    for (let i = 0; i < nb; i++) {
      const u = (i + 0.5 + (r() - 0.5) * 0.6) / nb;
      const [x, z] = along(u);
      const belly = 1 - (2 * u - 1) ** 2;
      lobes.push({
        x,
        z,
        dx: Math.cos(s.degA * DEG),
        dz: Math.sin(s.degA * DEG),
        reach: s.reach * lerp(0.55, 0.95, r()),
        r: s.size * lerp(1.25, 1.55, r()) * lerp(0.85, 1, belly),
        shrink: 0.6,
        die: lerp(0.95, 1.1, r()),
      });
    }
    return { life: s.life, delay: s.delay ?? 0, lobes };
  });
  jetCache.set(specs, list);
  return list;
}

interface Dome {
  x: number;
  y: number;
  rx: number;
}

interface Puff {
  alpha: number;
  domes: Dome[];
}

function dustPuffs(
  view: View,
  lt: number,
  t0: number,
  specs: JetSpec[],
  seed: number,
  origin: V3,
  half: number,
  out: Puff[],
) {
  const d0 = lt - t0;
  if (d0 < 0 || d0 > 0.4) return;
  for (const jet of jets(specs, seed)) {
    const d = d0 - jet.delay;
    const q = d / jet.life;
    if (q <= 0 || q >= 1) continue;
    // Out from under the edge fast, then held back by drag.
    const travel = (1 - Math.exp(-d / DRAG)) / (1 - Math.exp(-jet.life / DRAG));
    const grow = lerp(0.35, 1, outCubic(clamp(q / 0.3)));
    const domes: Dome[] = [];
    for (const l of jet.lobes) {
      const r0 = l.r * grow;
      // Shrinks onto its leading edge while the jet fades.
      const r = r0 * (1 - l.shrink * smoothstep(0.25, 1, q * l.die));
      const lead = l.reach * travel + r0 * 0.9;
      const dist = lead - r;
      const p = view.project([
        origin[0] + l.x * half + l.dx * dist,
        FLOOR,
        origin[2] + l.z * half + l.dz * dist,
      ]);
      // The crown lifts a little as it rolls out, never off the floor.
      const lift = 0.25 * r0 * smoothstep(0, 0.6, q) * view.cam.scale;
      domes.push({ x: p.x, y: p.y - lift, rx: r * view.cam.scale });
    }
    out.push({ alpha: DUST_ALPHA * (1 - smoothstep(0.4, 0.9, q)), domes });
  }
}

/** A dome: a floor-flat half-ellipse footprint under a low crown. */
function dome(path: Path2D, x: number, y: number, rx: number) {
  path.moveTo(x + rx, y);
  path.ellipse(x, y, rx, rx * 0.5, 0, 0, Math.PI);
  path.ellipse(x, y, rx, rx * 0.72, 0, Math.PI, TAU);
}

function drawPuffs(ctx: CanvasRenderingContext2D, puffs: Puff[]) {
  for (const p of puffs) {
    if (p.alpha <= 0.01 || p.domes.length === 0) continue;
    // One body per jet, so overlapping domes read as a single mass of dust:
    // shade first, then the same body raised a little and clipped to itself
    // for the lit crown, which leaves one shaded band along the floor.
    const body = new Path2D();
    let rx = 0;
    for (const d of p.domes) {
      dome(body, d.x, d.y, d.rx);
      rx += d.rx;
    }
    rx /= p.domes.length;
    ctx.save();
    ctx.globalAlpha *= p.alpha;
    ctx.fillStyle = DUST_BASE;
    ctx.fill(body);
    ctx.clip(body);
    ctx.translate(0, -0.32 * rx);
    ctx.fillStyle = DUST_TOP;
    ctx.fill(body);
    ctx.restore();
  }
}

// Spin lines: clean tapered swooshes around the box at two heights on each
// side, in its (un-spun) horizontal plane, so they tilt with the bank. They
// lead with a round head in the spin direction, stay clear of the outline,
// and retract into their heads as the spin slows.

interface Swoosh {
  /** 0 for the right side, PI for the left. */
  side: number;
  /** Height in half-extents. */
  j: number;
  /** Head angle past the side's extreme, radians, in the spin direction. */
  head: number;
  /** Arc length scale. */
  len: number;
  /** Radius in half-extents. */
  R: number;
  /** Start delay, seconds. */
  delay: number;
}

const SWOOSHES: Swoosh[] = [
  { side: 0, j: 0.5, head: 0.3, len: 1, R: 1.78, delay: 0 },
  { side: 0, j: -0.4, head: 0.05, len: 0.8, R: 1.7, delay: 0.017 },
  { side: Math.PI, j: 0.35, head: 0.2, len: 0.9, R: 1.74, delay: 0.008 },
  { side: Math.PI, j: -0.3, head: 0.4, len: 1, R: 1.82, delay: 0.025 },
];
const SWOOSH_IN = 0.05;
const SWOOSH_OUT0 = 0.1;
const SWOOSH_OUT1 = 0.2;
const SWOOSH_N = 28;
const SWOOSH_COLOR = mix(PALETTE.paper, PALETTE.amberBright, 0.35);

function drawSwooshes(ctx: CanvasRenderingContext2D, view: View, pose: BoxPose, lt: number, sil: Pt[]) {
  const d0 = lt - LAUNCH;
  if (d0 <= 0 || d0 >= SWOOSH_OUT1 + 0.03) return;
  const f = boxFrame({ ...pose, yaw: 0 });
  const k = view.cam.scale / HERO_CAM.scale;
  // Screen-right and toward-viewer axes in the box's tilted floor plane.
  const ur: V3 = [(f.x[0] - f.z[0]) * S2, (f.x[1] - f.z[1]) * S2, (f.x[2] - f.z[2]) * S2];
  const ud: V3 = [(f.x[0] + f.z[0]) * S2, (f.x[1] + f.z[1]) * S2, (f.x[2] + f.z[2]) * S2];
  const spun = yawAt(lt) - WIND;
  const pts: Pt[] = [];
  ctx.save();
  ctx.fillStyle = SWOOSH_COLOR;
  for (const s of SWOOSHES) {
    const d = d0 - s.delay;
    if (d <= 0) continue;
    // Grows out of the head, then the tail catches up with it.
    const grow = outCubic(clamp(d / SWOOSH_IN));
    const keep = 1 - inQuad(progress(SWOOSH_OUT0, SWOOSH_OUT1, d));
    const span = Math.min(spinRate(lt) * 0.05, 1.1) * s.len * grow * keep;
    if (span <= 0.02) continue;
    // Spin runs toward decreasing angle; the head creeps along with it.
    const phiHead = s.side - s.head - 0.08 * spun;
    pts.length = 0;
    for (let n = 0; n <= SWOOSH_N; n++) {
      const phi = phiHead + (span * n) / SWOOSH_N;
      const cr = s.R * Math.cos(phi);
      const sr = s.R * Math.sin(phi);
      const w: V3 = [
        f.c[0] + ur[0] * cr + ud[0] * sr + f.y[0] * s.j,
        f.c[1] + ur[1] * cr + ud[1] * sr + f.y[1] * s.j,
        f.c[2] + ur[2] * cr + ud[2] * sr + f.y[2] * s.j,
      ];
      pts.push(view.project(w));
    }
    // Keep the run from the head that stays clear of the outline.
    let a = 0;
    while (a < pts.length && outsideBy(sil, pts[a].x, pts[a].y) < 8 * k) a++;
    let b = a;
    while (b < pts.length && outsideBy(sil, pts[b].x, pts[b].y) >= 8 * k) b++;
    if (b - a < 3) continue;
    let length = 0;
    for (let n = a + 1; n < b; n++) length += Math.hypot(pts[n].x - pts[n - 1].x, pts[n].y - pts[n - 1].y);
    if (length < 30 * k) continue;
    const w0 = 4.2 * k * lerp(0.75, 1, keep);
    const left: Pt[] = [];
    const right: Pt[] = [];
    for (let n = a; n < b; n++) {
      const p = pts[Math.max(a, n - 1)];
      const q = pts[Math.min(b - 1, n + 1)];
      const dx = q.x - p.x;
      const dy = q.y - p.y;
      const l = Math.hypot(dx, dy) || 1;
      const u = (n - a) / (b - 1 - a);
      const hw = w0 * (1 - u) ** 1.4;
      left.push({ x: pts[n].x - (dy / l) * hw, y: pts[n].y + (dx / l) * hw });
      right.push({ x: pts[n].x + (dy / l) * hw, y: pts[n].y - (dx / l) * hw });
    }
    ctx.beginPath();
    ctx.moveTo(left[0].x, left[0].y);
    for (let n = 1; n < left.length; n++) ctx.lineTo(left[n].x, left[n].y);
    for (let n = right.length - 1; n >= 0; n--) ctx.lineTo(right[n].x, right[n].y);
    ctx.closePath();
    ctx.fill();
    // The head is its own path: sharing the body's would punch it hollow.
    ctx.beginPath();
    ctx.arc(pts[a].x, pts[a].y, w0, 0, TAU);
    ctx.fill();
  }
  ctx.restore();
}

/**
 * Contact shadow: identical to drawShadow at rest (the handoff frame), with
 * a tighter, darker core while the box is in the air.
 */
function contactShadow(
  ctx: CanvasRenderingContext2D,
  view: View,
  at: V3,
  size: number,
  alpha: number,
  core: number,
) {
  if (alpha <= 0.003) return;
  ctx.save();
  applyMatrix(ctx, view.planeMatrix([at[0], FLOOR, at[2]], [size, 0, 0], [0, 0, size]));
  const r = 0.78;
  const g = ctx.createRadialGradient(0, 0, 0, 0, 0, r);
  g.addColorStop(0, `rgba(0,0,0,${alpha})`);
  g.addColorStop(lerp(0.55, 0.32, core), `rgba(0,0,0,${alpha * lerp(0.5, 0.8, core)})`);
  g.addColorStop(1, "rgba(0,0,0,0)");
  ctx.fillStyle = g;
  ctx.beginPath();
  ctx.arc(0, 0, r, 0, Math.PI * 2);
  ctx.fill();
  ctx.restore();
}

// Impact ticks kicked off the label as it stamps down, in the panel's plane.
// They fly out at full weight and retract from the tail until they are gone.
const TICKS: [number, number, number, number][] = [
  [28, 40, -0.8, -0.6],
  [78, 40, 0.8, -0.6],
  [78, 72, 0.8, 0.6],
  [28, 72, -0.8, 0.6],
  [28, 56, -1, 0],
  [78, 56, 1, 0],
];
const TICK_LIFE = 0.08;

function stampTicks(ctx: CanvasRenderingContext2D, view: View, f: BoxFrame, lt: number) {
  const p = progress(LABEL, LABEL + TICK_LIFE, lt);
  if (p <= 0 || p >= 1) return;
  const head = 4 + 12 * outCubic(p);
  const tail = 4 + 12 * inQuad(p);
  if (head - tail < 0.6) return;
  ctx.save();
  applyMatrix(ctx, panelMatrix(view, f, "right"));
  ctx.beginPath();
  for (const [x, y, dx, dy] of TICKS) {
    ctx.moveTo(x + dx * tail, y + dy * tail);
    ctx.lineTo(x + dx * head, y + dy * head);
  }
  ctx.strokeStyle = CREAM;
  ctx.lineCap = "round";
  ctx.lineWidth = 3.4;
  ctx.stroke();
  ctx.restore();
}

// The clink: a flash in the lens and a ring kicked out around it, riding
// with the ring through its rebound.
function clinkRipple(ctx: CanvasRenderingContext2D, m: DOMMatrix2D, lt: number) {
  const p = progress(MONO, MONO + 0.15, lt);
  if (p <= 0 || p >= 1) return;
  ctx.save();
  applyMatrix(ctx, m);
  if (lt < MONO + 0.034) {
    ctx.beginPath();
    ctx.arc(0, 0, 14.5, 0, TAU);
    ctx.fillStyle = rgba(CREAM, 0.45 * (1 - progress(MONO, MONO + 0.034, lt)));
    ctx.fill();
  }
  ctx.beginPath();
  ctx.arc(0, 0, lerp(17, 30, outCubic(p)), 0, TAU);
  ctx.strokeStyle = rgba(CREAM, 0.85 * (1 - p) ** 1.5);
  ctx.lineWidth = 3.5 * (1 - p) + 0.5;
  ctx.stroke();
  ctx.restore();
}

/** Footprint half-extent at the impact squash, where the landing dust leaves from. */
const LAND_HALF = 0.5 / Math.sqrt(0.78);

export const scene: Scene = {
  id: S.id,
  start: S.start,
  end: S.end,
  draw(ctx, lt, env) {
    ctx.fillStyle = PALETTE.bg;
    ctx.fillRect(0, 0, env.W, env.H);

    const view = new View(cameraAt(lt));
    const k = view.cam.scale / HERO_CAM.scale;
    const pose: BoxPose = { ...bodyPose(lt), label: labelAt(lt), face: faceAt(lt) };
    const f = boxFrame(pose);
    const sil = inflate(boxSilhouette(view, pose), (OUTLINE_RATIO / 2) * view.cam.scale + 0.5);
    const [sx, sy] = shake(lt, LAND, 9, 0.07);
    const [tx, ty] = shake(lt, LABEL, 4, 0.06);

    ctx.save();
    ctx.translate(sx + tx, sy + ty);

    // Contact shadow under the center of mass: it tightens and fades as the
    // box rises, but never vanishes.
    const [li, lk] = lowestCorner(f);
    const hN = clamp((boxPoint(f, li, -1, lk)[1] - FLOOR) / 0.9);
    const squash = pose.squash ?? 1;
    const widen = 1 / Math.sqrt(Math.max(squash, 0.05));
    contactShadow(ctx, view, f.c, widen * lerp(1, 0.6, hN), lerp(0.45, 0.3, hN), hN);

    drawBox(ctx, view, pose);

    const puffs: Puff[] = [];
    dustPuffs(view, lt, LAUNCH, LAUNCH_DUST, 21, takeoffState().heel, 0, puffs);
    dustPuffs(view, lt, LAND, LAND_DUST, 7, [0, FLOOR, 0], LAND_HALF, puffs);
    drawPuffs(ctx, puffs);
    drawSwooshes(ctx, view, pose, lt, sil);
    stampTicks(ctx, view, f, lt);

    const mono = monoAt(view, f, lt);
    if (mono) {
      if (mono.behind) {
        ctx.save();
        clipOutside(ctx, sil);
        drawMonocle(ctx, mono.m);
        ctx.restore();
      } else {
        drawMonocle(ctx, mono.m);
      }
      if (lt < MONO) monocleRim(ctx, mono.m, sil);
    }
    clinkRipple(ctx, mono ? mono.m : seatMatrix(view, f), lt);

    // The glint: drawBox draws the star; add a small warm bloom and a
    // smaller echo on the far side of the rim two frames later.
    if (lt >= MONO_SWITCH) {
      const g = glintAt(lt) / GLINT_PEAK;
      if (g > 0) {
        const gp = faceToScreen(view, pose, 7.5, -38);
        glow(ctx, gp.x, gp.y, 45 * k * g, PALETTE.amberBright, 0.18 * g);
      }
      const g2 = glintAt(lt - 2 / 60);
      if (g2 > 0) {
        const r = monocleScreen(view, pose).r;
        const p2 = faceToScreen(view, pose, 29.7, -16.3);
        sparkle(ctx, p2.x, p2.y, r * 1.5 * g2 * 0.3, g2 / GLINT_PEAK, -g2 * 0.6);
      }
    }

    ctx.restore();
  },
};
