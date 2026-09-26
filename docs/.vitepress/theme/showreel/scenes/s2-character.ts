// Scene 2, "Mr Boxington". The box the fold shut rocks onto a corner, leaps
// with a full turn and lands in a puff of dust. Then his face pops in, one
// feature per sixteenth: the eye under its flat eyelid, the mustache, the
// monocle on its brass chain, the glint arc on its lens and the rosy cheeks.
// The camera eases him left and his name card rises beside him; he holds the
// logo pose, glances at the card, blinks once, and the push toward his
// monocle begins (bible.ts diveCam), the dive that "what" finishes.
//
// Everything is seen from the logo's own camera (box.ts logoCam), so the
// first frame is the fold's handoff (H1_CAM, H1_POSE) and the pose he holds
// is the logo exactly (H2_POSE).

import { BEAT, DIVE0, diveCam, drawLogoBox, H1_POSE, H2_CAM, H2_POSE, PALETTE, type Scene, sec } from "../bible";
import {
  type BoxFrame,
  type BoxPose,
  boxFrame,
  boxPoint,
  boxSilhouette,
  CHAIN,
  CHEEKS,
  type FaceParams,
  FRONT_CAM,
  faceToScreen,
  frontMatrix,
  LENS,
  LOGO_FACE,
  logoCam,
} from "../box";
import { mix, rgba } from "../color";
import { glow, shake } from "../fx";
import {
  clamp,
  cubicBezier,
  DEG,
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
  TAU,
  wobble,
} from "../math";
import { applyMatrix, type Camera, type V3, View } from "../space";
import { CAPTION, drawWords, wordStyle } from "../type";

const S = sec("mr-boxington");

// Accents in local time. The score (score/character.ts) is written to these.
const b = (n: number) => n * BEAT;
export const LAUNCH = b(0.5);
const APEX = b(0.875);
export const LAND = b(1.25);
/** The eye pops open under its flat eyelid. */
export const EYE = b(1.5);
/** The mustache. */
export const MUST = b(1.75);
/** The monocle's chain pays out from its anchor, one dot at a time... */
export const CHAIN0 = b(1.875);
/** ...and the monocle clinks onto its end. */
export const MONO = b(2);
/** The glint arc draws across the lens. */
export const GLINT = b(2.25);
/** The cheeks swell in, and the camera starts to ease him left. */
export const BLUSH = b(2.5);
/** The name card: the wordmark is in, then the line. */
export const WORDMARK = b(2.75);
export const TAGLINE = b(3);
/** He glances at his name as it lands. */
const GLANCE = b(2.75);
/** One 160 ms blink. */
export const BLINK = b(5.5);
/** The card wipes away and the push toward the monocle begins. */
export const PUSH = DIVE0 - S.start;

/** Pops start one frame early with a kick, so the anchor frame already reads. */
const LEAD = 1 / 60;
/** Every residual wobble is blended to rest over this window, before the card holds. */
const FIN0 = b(3.25);
const FIN1 = b(3.75);

const FLOOR = H1_POSE.pos[1];
const IMPACT = 0.045;

type Pt = { x: number; y: number };

// Camera: the logo's own view throughout, so a move is the logo's square
// sliding and scaling on the screen. It rises a little with the leap, pushes
// in on the face while it assembles, and from BLUSH eases from there to the
// name card's framing (H2_CAM), mostly there as the line lands. From PUSH it
// is the dive.

/** The logo's square on screen (box.ts logoCam): its top left corner and side, px. */
interface Square {
  x: number;
  y: number;
  size: number;
}
/** FRONT_CAM's square, where the fold left him, and H2_CAM's, beside the card. */
const FRONT: Square = { x: 720, y: 300, size: 480 };
const BESIDE: Square = { x: 250, y: 250, size: 440 };
/** The push-in holds the box's middle (logo 64, 73) still. */
const zoomed = (sq: Square, size: number): Square => ({
  x: sq.x + (64 * (sq.size - size)) / 128,
  y: sq.y + (73 * (sq.size - size)) / 128,
  size,
});
const PUSHED = zoomed(FRONT, 536);
const pushIn = cubicBezier(0.4, 0, 0.35, 1);
/** The camera has eased him left, and the card stopped beside him, by b3.25: the line then holds still to read. */
export const EASE1 = b(3.25);
const easeLeft = cubicBezier(0.35, 0, 0.1, 1);
/** Crane: the frame rises with the leap, px, and is home by touchdown. */
const CRANE = 36;
const craneAt = keys([
  [LAUNCH, 0],
  [APEX + 0.02, 1, inOutSine],
  [LAND - 0.01, 0, inOutSine],
]);

function squareAt(lt: number): Square {
  const p = pushIn(progress(LAND + 0.03, BLUSH, lt));
  const k = easeLeft(progress(BLUSH, EASE1, lt));
  const from = zoomed(FRONT, lerp(FRONT.size, PUSHED.size, p));
  return {
    x: lerp(from.x, BESIDE.x, k),
    y: lerp(from.y, BESIDE.y, k) + CRANE * craneAt(lt),
    size: lerp(from.size, BESIDE.size, k),
  };
}

function cameraAt(lt: number): Camera {
  if (lt >= PUSH) return diveCam(S.start + lt);
  if (lt >= EASE1) return H2_CAM;
  if (lt <= LAUNCH) return FRONT_CAM;
  const sq = squareAt(lt);
  return logoCam(sq.x, sq.y, sq.size);
}

// Body. On the ground he rocks about whichever bottom corner is lowest,
// pinned where it stood, so leaning never slides him. In the air his middle
// flies a true parabola and every tilt pivots about it.

/** Rocking onto the left heel winds him up. */
const ROCK = 15 * DEG;
/** Height of the leap's middle above the straight line from takeoff to touchdown. */
const LIFT = 0.62;
const LAND_SQUASH = 1.08;

/** Lean toward screen left (tiltZ): up onto the heel, thrown back over, righted to land. */
const leanAt = keys([
  [0.02, 0],
  [0.19, ROCK, inOutSine],
  [LAUNCH, ROCK * 1.05],
  [LAUNCH + 0.1, -7 * DEG, inOutSine],
  [APEX, -4 * DEG, inOutSine],
  [LAND - 0.05, 0, inOutSine],
]);
/** Lean away from the viewer, peaking a little later, so the bank precesses. */
const pitchAt = keys([
  [LAUNCH, 0],
  [APEX + 0.04, -9 * DEG, inOutSine],
  [LAND - 0.05, 0, inOutSine],
]);

const crouch = keys([
  [0.02, 1],
  [0.19, 0.84, inOutSine],
  [LAUNCH, 0.8],
  [LAUNCH + 0.045, 1.14, outQuad],
  [APEX - 0.02, 0.94, inOutSine],
  [LAND, LAND_SQUASH, inQuad],
]);

/** Small body reactions as each feature lands. */
function reactions(lt: number): number {
  let s = 0.05 * wobble(lt, EYE - LEAD, 3.2, 8);
  s -= 0.03 * wobble(lt, MUST, 4.5, 9);
  s -= 0.035 * pulse(lt, MONO, 0.012, 0.03);
  s += 0.025 * wobble(lt, BLUSH, 3.6, 8);
  return s;
}

function squashAt(lt: number): number {
  if (lt < LAND) return crouch(lt);
  const d = lt - LAND;
  if (d < IMPACT) return lerp(LAND_SQUASH, 0.78, outQuad(d / IMPACT));
  return 0.78 + 0.22 * spring(d - IMPACT, 2.8, 0.36) + reactions(lt);
}

/** A wind-up turn against the spin while he rocks, radians. */
const WIND = -16 * DEG;
const SPIN_K = 0.35;
const FLIGHT = LAND - LAUNCH;

function yawAt(lt: number): number {
  if (lt < LAUNCH) return WIND * inOutSine(progress(0.02, LAUNCH, lt));
  if (lt < LAND) {
    const p = (lt - LAUNCH) / FLIGHT;
    // Fast off the push, easing toward touchdown but still turning.
    return lerp(WIND, TAU, p + SPIN_K * p * (1 - p));
  }
  // Friction stops the spin at touchdown: he twists past square and recoils.
  const vLand = ((TAU - WIND) * (1 - SPIN_K)) / FLIGHT;
  const f = 3.2;
  return (vLand / (TAU * f)) * wobble(lt, LAND, f, 11);
}

/** Yaw rate in the air, radians per second. */
function spinRate(lt: number): number {
  const p = clamp((lt - LAUNCH) / FLIGHT);
  return ((TAU - WIND) * (1 + SPIN_K * (1 - 2 * p))) / FLIGHT;
}
/** The spin in quarter turns a second at takeoff and at touchdown, for the score's flutter. */
export const SPIN_QUARTERS: readonly [number, number] = [spinRate(LAUNCH) / (TAU / 4), spinRate(LAND) / (TAU / 4)];

/** Squash, yaw, and tilts, with the box's bottom middle on the floor's origin. */
function trackPose(lt: number): BoxPose {
  const fin = smoothstep(FIN0, FIN1, lt);
  // Momentum tips him on along the travel as he lands, then he rocks back.
  const lean = leanAt(lt) + 0.06 * wobble(lt, LAND + 0.01, 2.6, 7);
  return {
    ...H1_POSE,
    pos: [0, FLOOR, 0],
    squash: lerp(squashAt(lt), 1, fin),
    yaw: lerp(yawAt(lt), 0, fin),
    tiltX: lerp(pitchAt(lt), 0, fin),
    tiltZ: lerp(lean, 0, fin),
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
  if (!base.tiltX && !base.tiltZ) return base;
  const tilted = boxFrame({ ...base, pos: ORIGIN });
  const flat = boxFrame({ ...base, pos: ORIGIN, tiltX: 0, tiltZ: 0 });
  const [i, k] = lowestCorner(tilted);
  const t = boxPoint(tilted, i, -1, k);
  const u = boxPoint(flat, i, -1, k);
  return { ...base, pos: [u[0] - t[0], FLOOR - t[1], u[2] - t[2]] };
}

let takeoff: { c: V3; heel: V3 } | null = null;
/** Middle and heel at the takeoff frame, where the air arc starts. */
function takeoffState(): { c: V3; heel: V3 } {
  if (takeoff) return takeoff;
  const pose = groundPose(trackPose(LAUNCH));
  const f = boxFrame(pose);
  const [i, k] = lowestCorner(f);
  takeoff = { c: f.c, heel: boxPoint(f, i, -1, k) };
  return takeoff;
}

/** Airborne: the middle on a ballistic arc from takeoff to the landing spot. */
function airPose(lt: number, base: BoxPose): BoxPose {
  const p = (lt - LAUNCH) / FLIGHT;
  const c0 = takeoffState().c;
  const f = boxFrame({ ...base, pos: ORIGIN });
  const y1 = FLOOR + (boxFrame({ ...H1_POSE, squash: LAND_SQUASH }).c[1] - H1_POSE.pos[1]);
  const c: V3 = [lerp(c0[0], 0, p), lerp(c0[1], y1, p) + LIFT * 4 * p * (1 - p), lerp(c0[2], 0, p)];
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

/** The eye opens as it pops in: its eyelid snaps up to the skeptic's line. */
const openAt = (lt: number) => 1 - outQuad(progress(EYE - LEAD, EYE + 0.045, lt));

/** One 160 ms blink: both eyes shut to one level line, hold, open. */
const blinkAt = keys([
  [BLINK, 0],
  [BLINK + 0.035, 1, inQuad],
  [BLINK + 0.115, 1],
  [BLINK + 0.16, 0, outQuad],
]);

/** He looks across at his name as it lands, then back at you well before the blink. */
const lookX = keys([
  [GLANCE - 0.02, 0],
  [GLANCE + 0.07, 3.4, outCubic],
  [b(4.25), 3.4],
  [b(4.25) + 0.1, 0, outCubic],
]);
const lookY = keys([
  [GLANCE - 0.02, 0],
  [GLANCE + 0.07, 0.8, outCubic],
  [b(4.25), 0.8],
  [b(4.25) + 0.1, 0, outCubic],
]);

/** The glint arc draws on from its lower end over a few frames. */
const arcAt = (lt: number) => outQuad(progress(GLINT - LEAD, GLINT + 0.05, lt));

/**
 * Cheeks: scaled in by the anchor (box.ts reads 0..1 as the scale), then
 * warmed through the mascot's three levels to the logo's rose.
 */
function cheeksAt(lt: number): number {
  if (lt < BLUSH + 0.03) return outCubic(progress(BLUSH - LEAD - 0.02, BLUSH + 0.03, lt));
  return 1 + 2 * smoothstep(BLUSH + 0.03, BLUSH + 0.2, lt);
}

function faceAt(lt: number): FaceParams | null {
  if (lt < EYE - LEAD) return null;
  const fin = smoothstep(FIN0, FIN1, lt);
  const settle = (v: number, rest: number) => lerp(v, rest, fin);
  return {
    ...LOGO_FACE,
    eyes: settle(pop(lt, EYE, 5, 0.45, 25), 1),
    blink: Math.max(openAt(lt), blinkAt(lt)),
    look: [lookX(lt), lookY(lt)],
    mustache: lt < MUST - LEAD ? 0 : settle(pop(lt, MUST, 4.5, 0.45, 25), 1),
    // The mustache's flourish: both tips curl up and wave out.
    twitch: settle(0.3 * wobble(lt, MUST + 0.04, 6.5, 6.5), 0),
    monocle: lt < MONO - LEAD ? 0 : settle(pop(lt, MONO, 6, 0.55, 20), 1),
    // The chain: our dots while they pay out, drawBox's from the clink.
    chain: lt < MONO - LEAD ? 0 : 1,
    // The clink knocks the monocle swinging on its chain.
    swing: settle(-0.12 * wobble(lt, MONO, 3.4, 8), 0),
    arc: arcAt(lt),
    cheeks: lt < BLUSH - LEAD - 0.02 ? 0 : cheeksAt(lt),
  };
}

// The chain pays out from its anchor on the box's side toward the ring, one
// dot every few frames, drawn as drawBox draws them (logo.svg's dotted path,
// dots 4.7 units apart from the ring end), so the hand-over at the clink is
// invisible.

const CHAIN_PATH: readonly [number, number][] = [
  [110, 72],
  [116, 78],
  [120, 86],
  [120, 96],
];
const CHAIN_DOTS = 6;
const CHAIN_PITCH = 4.7;
/** A point on the chain's curve at parameter u. */
function chainAt(u: number): [number, number] {
  const [p0, p1, p2, p3] = CHAIN_PATH;
  const v = 1 - u;
  const w = [v * v * v, 3 * v * v * u, 3 * v * u * u, u * u * u];
  return [
    w[0] * p0[0] + w[1] * p1[0] + w[2] * p2[0] + w[3] * p3[0],
    w[0] * p0[1] + w[1] * p1[1] + w[2] * p2[1] + w[3] * p3[1],
  ];
}
let chainDots: [number, number][] | null = null;
/** The dots' centres, ring end first, by arc length along the curve. */
function dotsOnChain(): [number, number][] {
  if (chainDots) return chainDots;
  const out: [number, number][] = [];
  let s = 0;
  let prev = chainAt(0);
  let next = 0;
  for (let i = 1; i <= 2000 && out.length < CHAIN_DOTS; i++) {
    const p = chainAt(i / 2000);
    const step = Math.hypot(p[0] - prev[0], p[1] - prev[1]);
    while (out.length < CHAIN_DOTS && s + step >= next) {
      const k = step > 0 ? (next - s) / step : 0;
      out.push([lerp(prev[0], p[0], k), lerp(prev[1], p[1], k)]);
      next += CHAIN_PITCH;
    }
    s += step;
    prev = p;
  }
  chainDots = out;
  return out;
}
/** When chain dot `i` (0 at the ring) rattles out: the anchor end first. */
export const chainDotAt = (i: number): number =>
  CHAIN0 + ((CHAIN_DOTS - 1 - i) / CHAIN_DOTS) * (MONO - LEAD - CHAIN0);

function drawChainOut(ctx: CanvasRenderingContext2D, view: View, pose: BoxPose, lt: number): void {
  if (lt < CHAIN0 || lt >= MONO - LEAD) return;
  ctx.save();
  applyMatrix(ctx, frontMatrix(view, pose, 115, 84));
  ctx.fillStyle = CHAIN;
  dotsOnChain().forEach(([x, y], i) => {
    const d = lt - chainDotAt(i);
    if (d < 0) return;
    // Each dot drops in a touch large and settles, a link catching.
    const r = 1.5 * (1 + 0.5 * Math.exp(-d / 0.012));
    ctx.beginPath();
    ctx.arc(x, y, r, 0, TAU);
    ctx.fill();
  });
  ctx.restore();
}

// Accents on the features: a ring knocked out around the lens as the
// monocle clinks home, light running along the glint arc as it draws, and a
// warm bloom as the cheeks swell.

function clinkRipple(ctx: CanvasRenderingContext2D, view: View, pose: BoxPose, lt: number): void {
  const p = progress(MONO, MONO + 0.16, lt);
  if (p <= 0 || p >= 1) return;
  const c = faceToScreen(view, pose, 86, 62);
  const e = faceToScreen(view, pose, 86 + 24.5, 62);
  const r = Math.hypot(e.x - c.x, e.y - c.y);
  ctx.save();
  if (lt < MONO + 0.034) {
    ctx.beginPath();
    ctx.arc(c.x, c.y, r * 0.8, 0, TAU);
    ctx.fillStyle = rgba(PALETTE.paper, 0.4 * (1 - progress(MONO, MONO + 0.034, lt)));
    ctx.fill();
  }
  ctx.beginPath();
  ctx.arc(c.x, c.y, r * lerp(1.1, 1.75, outCubic(p)), 0, TAU);
  ctx.strokeStyle = rgba(PALETTE.paper, 0.8 * (1 - p) ** 1.5);
  ctx.lineWidth = r * (0.13 * (1 - p) + 0.02);
  ctx.stroke();
  ctx.restore();
}

/** The arc's lower end, its middle, and its upper end, in logo units (logo.svg `M69 57a18 18 0 0 1 10-12`). */
const ARC_PTS: readonly [number, number][] = [
  [69, 57],
  [72.3, 49.8],
  [79, 45],
];

function glintShimmer(ctx: CanvasRenderingContext2D, view: View, pose: BoxPose, lt: number): void {
  const d = lt - (GLINT - LEAD);
  if (d < 0 || d > 0.3) return;
  const scale = view.cam.scale / FRONT_CAM.scale;
  // Light rides the arc's head as it draws, then blooms and fades on the lens.
  const u = arcAt(lt);
  const seg = u < 0.5 ? 0 : 1;
  const k = u < 0.5 ? u / 0.5 : (u - 0.5) / 0.5;
  const [x0, y0] = ARC_PTS[seg];
  const [x1, y1] = ARC_PTS[seg + 1];
  const head = faceToScreen(view, pose, lerp(x0, x1, k), lerp(y0, y1, k));
  const fade = 1 - smoothstep(0.05, 0.3, d);
  glow(ctx, head.x, head.y, 90 * scale * (0.6 + 0.4 * u), "#ffffff", 0.55 * fade);
  const mid = faceToScreen(view, pose, 86, 62);
  glow(ctx, mid.x, mid.y, 170 * scale, LENS, 0.28 * fade * smoothstep(0, 0.05, d));
}

function cheekBloom(ctx: CanvasRenderingContext2D, view: View, pose: BoxPose, lt: number): void {
  const d = lt - BLUSH;
  if (d < -LEAD || d > 0.35) return;
  const k = pulse(lt, BLUSH + 0.04, 0.05, 0.08);
  if (k <= 0.01) return;
  const scale = view.cam.scale / FRONT_CAM.scale;
  for (const x of [14, 106]) {
    const p = faceToScreen(view, pose, x, 94);
    glow(ctx, p.x, p.y, 70 * scale * (1 + 0.4 * smoothstep(0, 0.2, d)), CHEEKS[2], 0.45 * k);
  }
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
  /** Floor direction at a and at b: 0 is screen right, 90 straight at the viewer, 180 screen left. */
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

// Takeoff: squeezed out from under the heel he pushed off from, a small skid
// out along the floor.
const LAUNCH_DUST: JetSpec[] = [
  { a: [0, 0], b: [0, 0], degA: 185, degB: 160, n: 5, reach: 0.3, size: 0.045, life: 0.15 },
];
// Touchdown: squeezed out sideways from under the base, which is how the
// lens sees dust along the floor: a skid off each front corner, longest to
// the right where the spin carries it, and a smaller one from further back
// on each side.
const LAND_DUST: JetSpec[] = [
  { a: [1, 1], b: [1, 1], degA: -8, degB: 25, n: 8, reach: 0.55, size: 0.066, life: 0.2 },
  { a: [-1, 1], b: [-1, 1], degA: 188, degB: 155, n: 7, reach: 0.45, size: 0.06, life: 0.19 },
  { a: [1, 0.2], b: [1, 0.2], degA: -15, degB: 5, n: 5, reach: 0.4, size: 0.05, life: 0.17, delay: 0.01 },
  { a: [-1, 0.2], b: [-1, 0.2], degA: 195, degB: 175, n: 5, reach: 0.34, size: 0.048, life: 0.16, delay: 0.01 },
];
const DUST_TINT = mix(PALETTE.text3, PALETTE.amberShade, 0.25);
const DUST_BASE = mix(DUST_TINT, PALETTE.bg, 0.4);
const DUST_TOP = mix(DUST_TINT, PALETTE.paper, 0.1);
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
      const lift = 0.35 * r0 * smoothstep(0, 0.6, q) * view.cam.scale * p.f;
      domes.push({ x: p.x, y: p.y - lift, rx: r * view.cam.scale * p.f });
    }
    out.push({ alpha: DUST_ALPHA * (1 - smoothstep(0.4, 0.9, q)), domes });
  }
}

/** A dome: a floor-flat half-ellipse footprint under a low crown. */
function dome(path: Path2D, x: number, y: number, rx: number) {
  path.moveTo(x + rx, y);
  path.ellipse(x, y, rx, rx * 0.25, 0, 0, Math.PI);
  path.ellipse(x, y, rx, rx * 0.85, 0, Math.PI, TAU);
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
// lead with a round head in the spin direction, stay clear of the box, and
// retract into their heads as the spin slows.

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
  { side: 0, j: 0.5, head: 0.3, len: 1, R: 1.7, delay: 0 },
  { side: 0, j: -0.45, head: 0.05, len: 0.8, R: 1.62, delay: 0.017 },
  { side: Math.PI, j: 0.35, head: 0.2, len: 0.9, R: 1.66, delay: 0.008 },
  { side: Math.PI, j: -0.3, head: 0.4, len: 1, R: 1.74, delay: 0.025 },
];
const SWOOSH_IN = 0.05;
const SWOOSH_OUT0 = 0.12;
const SWOOSH_OUT1 = 0.24;
const SWOOSH_N = 28;
const SWOOSH_COLOR = mix(PALETTE.paper, PALETTE.amberBright, 0.35);

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

function drawSwooshes(ctx: CanvasRenderingContext2D, view: View, pose: BoxPose, lt: number) {
  const d0 = lt - LAUNCH;
  if (d0 <= 0 || d0 >= SWOOSH_OUT1 + 0.03) return;
  const sil = boxSilhouette(view, pose);
  const f = boxFrame({ ...pose, yaw: 0 });
  const k = view.cam.scale / FRONT_CAM.scale;
  // Screen right and toward the viewer, in the box's tilted floor plane.
  const ur = f.x;
  const ud = f.z;
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
    // The front turns toward screen right, so the spin runs toward
    // decreasing angle; the head creeps along with it.
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
    // Keep the run from the head that stays clear of the box.
    let a = 0;
    while (a < pts.length && outsideBy(sil, pts[a].x, pts[a].y) < 10 * k) a++;
    let e = a;
    while (e < pts.length && outsideBy(sil, pts[e].x, pts[e].y) >= 10 * k) e++;
    if (e - a < 3) continue;
    let length = 0;
    for (let n = a + 1; n < e; n++) length += Math.hypot(pts[n].x - pts[n - 1].x, pts[n].y - pts[n - 1].y);
    if (length < 30 * k) continue;
    const w0 = 5 * k * lerp(0.75, 1, keep);
    const left: Pt[] = [];
    const right: Pt[] = [];
    for (let n = a; n < e; n++) {
      const p = pts[Math.max(a, n - 1)];
      const q = pts[Math.min(e - 1, n + 1)];
      const dx = q.x - p.x;
      const dy = q.y - p.y;
      const l = Math.hypot(dx, dy) || 1;
      const u = (n - a) / (e - 1 - a);
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
 * Contact shadow under the box's middle, on the floor under its front
 * edge: at rest exactly the one drawLogoBox draws, and tighter and fainter
 * the higher he is.
 */
function contactShadow(ctx: CanvasRenderingContext2D, view: View, pose: BoxPose): void {
  const f = boxFrame(pose);
  const [li, lk] = lowestCorner(f);
  const h = clamp((boxPoint(f, li, -1, lk)[1] - FLOOR) / 0.9);
  const wide = (pose.size ?? 1) / Math.sqrt(Math.max(pose.squash ?? 1, 0.05)) * lerp(1, 0.6, h);
  const l = view.project([f.c[0] - wide / 2, FLOOR, 0.5]);
  const r = view.project([f.c[0] + wide / 2, FLOOR, 0.5]);
  const w = r.x - l.x;
  const a = lerp(1, 0.55, h);
  ctx.save();
  ctx.translate((l.x + r.x) / 2, l.y);
  ctx.scale(1, 0.085);
  const g = ctx.createRadialGradient(0, 0, 0, 0, 0, w * 0.66);
  g.addColorStop(0, `rgba(0,0,0,${0.55 * a})`);
  g.addColorStop(0.6, `rgba(0,0,0,${0.3 * a})`);
  g.addColorStop(1, "rgba(0,0,0,0)");
  ctx.fillStyle = g;
  ctx.beginPath();
  ctx.arc(0, 0, w * 0.66, 0, Math.PI * 2);
  ctx.fill();
  ctx.restore();
}

// The name card, in the free right of the H2_CAM frame: the wordmark over
// the line, set as the reel's captions are (type.ts drawWords: a word per
// 1/32 note, rising; a sixteenth's wipe out), and centred on the box's
// height. It leaves as the push begins, and has wiped by the bar line.

export const NAME = "mr boxington";
export const LINE: readonly [string, string] = ["A shared cache", "for Cargo builds."];
/** The wordmark, 128 px. */
const NAME_STYLE = wordStyle(128);
/** The card's left edge, and its baselines: the wordmark's, then the line's two. */
export const CARD = { x: 772, name: 392, line: [540, 644] } as const;
/**
 * When each of the card's lines lands: the wordmark, then both halves of the
 * line together, so the words cascade down the card a 1/32 note apart.
 */
export const CARD_LANDS: readonly [number, number, number] = [WORDMARK, TAGLINE, TAGLINE];

/**
 * The card rides beside him while the camera eases him left: it keeps its
 * place relative to the box's right edge, so it arrives with him, and has
 * stopped where H2_CAM frames it by the time the line is read.
 */
function cardShift(lt: number): number {
  if (lt >= EASE1) return 0;
  const right = (sq: Square) => sq.x + (124 * sq.size) / 128;
  return right(squareAt(lt)) - right(BESIDE);
}

function drawCard(ctx: CanvasRenderingContext2D, lt: number): void {
  const x = CARD.x + cardShift(lt);
  drawWords(ctx, NAME, x, CARD.name, NAME_STYLE, lt, CARD_LANDS[0], PUSH);
  drawWords(ctx, LINE[0], x, CARD.line[0], CAPTION, lt, CARD_LANDS[1], PUSH);
  drawWords(ctx, LINE[1], x, CARD.line[1], CAPTION, lt, CARD_LANDS[2], PUSH);
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
    const cam = cameraAt(lt);
    if (lt >= FIN1 && lt >= b(4.25) + 0.1 && (lt < BLINK || lt >= BLINK + 0.16)) {
      // At rest in the logo pose: the handoff's own drawing, under the card.
      drawLogoBox(ctx, cam, H2_POSE);
      drawCard(ctx, lt);
      return;
    }

    const view = new View(cam);
    const face = faceAt(lt);
    const pose: BoxPose = { ...bodyPose(lt), tape: 1, face };
    const [sx, sy] = shake(lt, LAND, 9, 0.07);

    ctx.save();
    ctx.translate(sx, sy);
    contactShadow(ctx, view, pose);
    drawLogoBox(ctx, cam, pose, 0);
    drawChainOut(ctx, view, pose, lt);
    const puffs: Puff[] = [];
    dustPuffs(view, lt, LAUNCH, LAUNCH_DUST, 21, takeoffState().heel, 0, puffs);
    dustPuffs(view, lt, LAND, LAND_DUST, 7, [0, FLOOR, 0], LAND_HALF, puffs);
    drawPuffs(ctx, puffs);
    drawSwooshes(ctx, view, pose, lt);
    if (face) {
      clinkRipple(ctx, view, pose, lt);
      glintShimmer(ctx, view, pose, lt);
      cheekBloom(ctx, view, pose, lt);
    }
    ctx.restore();
    drawCard(ctx, lt);
  },
};
