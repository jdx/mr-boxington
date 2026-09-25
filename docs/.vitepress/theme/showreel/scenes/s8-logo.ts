// Scene 8, "Logo resolve": the flat amber silhouette from the morph inflates
// into the shaded box on the downbeat, the camera pushes into a close-up of
// the face while it assembles on sixteenths, snaps back on b29 as the
// wordmark lockup rises in below, and the card settles into the poster frame.

import { BEAT, bar, END_CAM, END_POSE, PALETTE, type Scene } from "../bible";
import {
  type BoxPose,
  boxFrame,
  boxPoint,
  boxSilhouette,
  CREAM,
  drawBox,
  drawLabelArt,
  drawShadow,
  type FaceParams,
  faceToScreen,
  INK,
  monoclePaths,
  OUTLINE,
  OUTLINE_RATIO,
  panelMatrix,
  sparkle,
} from "../box";
import { mix, rgba } from "../color";
import { flash, glow, makeCanvas, roundedRect, shake } from "../fx";
import {
  clamp,
  cubicBezier,
  hash,
  inCubic,
  inOutSine,
  inQuad,
  keys,
  lerp,
  outCubic,
  outQuad,
  outSine,
  progress,
  pulse,
  smoothstep,
  spring,
  swiftInOut,
  swiftOut,
  TAU,
  wobble,
} from "../math";
import { applyMatrix, polygon, type Projected, View } from "../space";
import { DISPLAY, drawText, font, layout, MONO } from "../type";

/** Local time of beat `n` of this bar (b28 is 0). */
const b = (n: number): number => n * BEAT;

// Accents, local seconds. The soundtrack is written to these.
const T_BROWS = b(0.5);
const T_EYES = b(0.75);
const T_MUST = b(1);
const T_MONO = b(1.25);
const T_BOW = b(1.5);
const T_TAPE = b(1.75);
const T_LABEL = b(2);
const T_BLINK = b(2.5);
const T_WORD = b(1);
const T_TAG = b(1.5);
const T_URL = b(1.75);
const T_GLINT = b(3);
const END = bar(1);

// The box sits where END_CAM puts it; the lockup is centered below.
const CX = 960;
const BOX_Y = END_CAM.cy;
const WORD = "mr boxington";
const WORD_Y = 712;
const WORD_FONT = font(100, 600, DISPLAY);
const WORD_TRACK = -4;
const TAG = "A shared cache for Cargo builds";
const TAG_Y = 786;
const TAG_FONT = font(34, 400, DISPLAY);
const URL_TEXT = "mr-boxington.jdx.dev";
const URL_Y = 840;
const URL_FONT = font(22, 500, MONO);
const URL_TRACK = 1.5;

// Camera: a close-up on the face for the brows and eyes, then a snap back on
// b29 that reveals the lockup. Under the orthographic END_CAM a zoom is a 2D
// scale, so the move is a canvas transform that carries the point between
// the eyes from where END_CAM draws it to frame center at LEAN_Z. The snap is
// front-loaded so the box has shrunk clear of the wordmark before the first
// letter surfaces, then it settles over the next beat.
const LEAN_Z = 2.4;
const LEAN_AT: readonly [number, number] = [960, 540];
const FOCUS: readonly [number, number] = [-4, -28];
const PUSH = cubicBezier(0.45, 0, 0.1, 1);
const PULL = cubicBezier(0.05, 0.75, 0.15, 1);
const T_PULL = T_MUST - 0.004;
const PULL_DUR = 0.58;
function leanAt(lt: number): number {
  // A quick push after the hit, then a slow creep so the close-up never parks.
  const push =
    0.93 * PUSH(progress(0.035, 0.215, lt)) + 0.07 * inOutSine(progress(0.12, T_PULL, lt));
  return push * (1 - PULL(progress(T_PULL, T_PULL + PULL_DUR, lt)));
}
/** The world layer's transform at `lt`: a uniform zoom `z` plus offset. */
function leanTransform(lt: number): [ex: number, ey: number, z: number] {
  const w = leanAt(lt);
  const z = lerp(1, LEAN_Z, w);
  const F = faceFocus();
  return [lerp(F.x, LEAN_AT[0], w) - z * F.x, lerp(F.y, LEAN_AT[1], w) - z * F.y, z];
}
let focus: { x: number; y: number } | null = null;
function faceFocus(): { x: number; y: number } {
  if (!focus) {
    const p = faceToScreen(new View(END_CAM), END_POSE, FOCUS[0], FOCUS[1]);
    focus = { x: p.x, y: p.y };
  }
  return focus;
}

/** Box size: a hard inflate pop on the downbeat that springs back. */
const sizeAt = (lt: number): number => 1 + 0.1 * wobble(lt, 0, 2.6, 6.5);

/** Small squash pulses as features land, so the box reacts to each. */
function squashAt(lt: number): number {
  let s = 1 + 0.05 * wobble(lt, 0.05, 3.2, 8);
  s -= 0.02 * pulse(lt, T_EYES + 0.01, 0.02, 0.06);
  s -= 0.018 * pulse(lt, T_MUST + 0.01, 0.02, 0.06);
  s -= 0.02 * pulse(lt, MONO_SEAT, 0.015, 0.05);
  s -= 0.03 * pulse(lt, LABEL_HIT, 0.015, 0.07);
  return s;
}

/** Hold breath: rises from rest at b31 to the top of a breath on the last frame. */
function floatAt(lt: number): number {
  if (lt <= T_GLINT) return 0;
  const k = (lt - T_GLINT) / (END - T_GLINT);
  return 0.011 * (0.5 - 0.5 * Math.cos(Math.PI * k));
}

/**
 * A spring that is `at` of the way grown on its cue: it starts just early
 * enough that the pop reads on the beat and its overshoot lands a few frames
 * after, instead of the whole move trailing the sound.
 */
function popper(freq: number, damping: number, at = 0.6): (lt: number, cue: number) => number {
  let lo = 0;
  let hi = 0.3;
  for (let i = 0; i < 40; i++) {
    const m = (lo + hi) / 2;
    if (spring(m, freq, damping) < at) lo = m;
    else hi = m;
  }
  const lead = lo;
  return (lt, cue) => spring(lt - cue + lead, freq, damping);
}
const popBrows = popper(6, 0.45);
const popEyes = popper(6, 0.35);
const popMustache = popper(5.5, 0.45);

// Bow tie: a fast, well-damped half turn, with the scale held down while
// the wings point toward the panel's edges, so no frame of the spin crosses
// the outline. The wings reach 38.5 face px from the knot along the tie and
// 12.5 across it; inside the outline the knot has 23.5 px of room below it
// and 41 px to the side (a hair inside the outline). Both springs start
// 0.045 s early, so on the cue the tie stands upright at about 0.58 scale and
// the rest of the turn carries it home.
const BOW_LEAD = 0.045;
function bowtieAt(lt: number): [number, number] {
  const d = lt - T_BOW + BOW_LEAD;
  const spin = -Math.PI * (1 - spring(d, 5, 0.7));
  const sin = Math.abs(Math.sin(spin));
  const cos = Math.abs(Math.cos(spin));
  const fit = Math.min(23.5 / (38.5 * sin + 12.5 * cos), 41 / (38.5 * cos + 12.5 * sin));
  return [Math.min(spring(d, 5.5, 0.5), fit), spin];
}

function faceAt(lt: number): FaceParams {
  // Glance down at the wordmark as it lands, back to camera for the glint.
  const glance =
    swiftInOut(progress(T_WORD + 0.14, T_WORD + 0.34, lt)) *
    (1 - swiftInOut(progress(T_GLINT - 0.22, T_GLINT - 0.05, lt)));
  const [bowtie, bowtieSpin] = bowtieAt(lt);

  return {
    brows: popBrows(lt, T_BROWS),
    // The brows sit a few px under the panel's top edge, so their acting goes
    // down, not up: a dip as they pop in (while the overshoot makes them
    // tall), a squint that clamps the monocle as it seats, and only a hair of
    // lift for the glint.
    browLift:
      -2.2 * wobble(lt, T_BROWS + 0.01, 3.5, 8) -
      2.6 * (spring(lt - MONO_SEAT, 5, 0.55) - spring(lt - MONO_SEAT - 0.2, 2.4, 0.8)) +
      0.9 * pulse(lt, T_GLINT - 0.02, 0.06, 0.18),
    eyes: popEyes(lt, T_EYES),
    // Lids shut on the first blip of b30.5 and are opening on the second.
    blink: blinkAt(lt),
    look: [2.5 * glance, 3.2 * glance],
    mustache: popMustache(lt, T_MUST),
    twitch: 0.22 * wobble(lt, T_MUST + 0.02, 5.5, 6) + 0.12 * wobble(lt, T_GLINT, 7, 9),
    // drawBox takes the monocle back once this scene's drop has settled.
    monocle: lt < MONO_SEAT + MONO_OUT ? 0 : 1,
    glint: glintAt(lt),
    bowtie,
    bowtieSpin,
  };
}

const glintAt = keys([
  [T_GLINT - 0.035, 0],
  [T_GLINT, 0.7, outQuad],
  [T_GLINT + 0.34, 0, inOutSine],
]);

const blinkAt = keys([
  [T_BLINK - 0.04, 0],
  [T_BLINK - 0.004, 1, inQuad],
  [T_BLINK + 0.05, 1],
  [T_BLINK + 0.13, 0, outQuad],
]);

// Tape: the swipe's fastest stretch ends on the cue frame, then it wraps down
// the right panel on a long soft landing.
const tapeAt = keys([
  [T_TAPE - 0.025, 0],
  [T_TAPE + 0.2, 1, swiftOut],
]);

function poseAt(lt: number): BoxPose {
  const size = sizeAt(lt);
  // Inflate about the box center so the pop reads as volume, not a hop.
  return {
    ...END_POSE,
    pos: [0, -0.5 * size + floatAt(lt), 0],
    size,
    squash: squashAt(lt),
    tape: tapeAt(lt),
    // drawBox takes the label back once this scene's stamp has settled.
    label: lt < LABEL_HIT + LABEL_OUT ? 0 : 1,
    face: faceAt(lt),
    toneShift: 0.3 * pulse(lt, 0.02, 0.02, 0.08),
  };
}

type C3 = [number, number, number];
const TOP: C3[] = [[-1, 1, -1], [1, 1, -1], [1, 1, 1], [-1, 1, 1]];
const FACE: C3[] = [[-1, 1, 1], [1, 1, 1], [1, -1, 1], [-1, -1, 1]];
const RIGHT: C3[] = [[1, 1, 1], [1, 1, -1], [1, -1, -1], [1, -1, 1]];

// Light floods each visible panel in key-light order (top, face, right), a
// soft front sweeping down and slightly right across it, so the flat
// silhouette becomes dimensional one plane at a time.
const REVEAL: { corners: C3[]; t0: number }[] = [
  { corners: TOP, t0: 0.012 },
  { corners: FACE, t0: 0.08 },
  { corners: RIGHT, t0: 0.148 },
];
const REVEAL_DUR = 0.075;
const SWEEP: readonly [number, number] = [0.45, 0.893];

function drawReveal(ctx: CanvasRenderingContext2D, view: View, pose: BoxPose, lt: number): void {
  const f = boxFrame(pose);
  const [dx, dy] = SWEEP;
  for (const panel of REVEAL) {
    const q = progress(panel.t0, panel.t0 + REVEAL_DUR, lt);
    if (q >= 1) continue;
    const pts = panel.corners.map((c) => view.project(boxPoint(f, c[0], c[1], c[2])));
    let m0 = Infinity;
    let m1 = -Infinity;
    for (const p of pts) {
      const d = p.x * dx + p.y * dy;
      if (d < m0) m0 = d;
      if (d > m1) m1 = d;
    }
    const band = 26;
    const s = lerp(m0 - band, m1 + band, inOutSine(q));
    ctx.save();
    const g = ctx.createLinearGradient(dx * (s - band), dy * (s - band), dx * (s + band), dy * (s + band));
    g.addColorStop(0, rgba(PALETTE.amber, 0));
    g.addColorStop(1, rgba(PALETTE.amber, 1));
    polygon(ctx, pts);
    ctx.fillStyle = g;
    ctx.fill();
    // A hairline of the same fill closes the antialiased seams between panels.
    ctx.strokeStyle = g;
    ctx.lineWidth = 2;
    ctx.stroke();
    if (q > 0) {
      // The light front itself: a warm band riding the edge of the reveal.
      ctx.clip();
      const h = ctx.createLinearGradient(dx * (s - 44), dy * (s - 44), dx * (s + 44), dy * (s + 44));
      const heat = 0.5 * Math.sin(Math.PI * q);
      h.addColorStop(0, rgba(PALETTE.amberBright, 0));
      h.addColorStop(0.55, rgba(PALETTE.paper, heat));
      h.addColorStop(1, rgba(PALETTE.amberBright, 0));
      ctx.globalCompositeOperation = "lighter";
      ctx.fillStyle = h;
      ctx.fill();
    }
    ctx.restore();
  }
}

// Outline draw-on: the three inner edges shoot out of the near corner, then
// the silhouette edges run on from their ends until they meet. The hit frame
// itself is the clean hot silhouette; the first edges show on the next frame
// already long enough to read as three spokes rather than a blot of caps.
const EDGES_A: [C3, C3][] = [
  [[1, 1, 1], [-1, 1, 1]],
  [[1, 1, 1], [1, 1, -1]],
  [[1, 1, 1], [1, -1, 1]],
];
const EDGES_B: [C3, C3][] = [
  [[-1, 1, 1], [-1, 1, -1]],
  [[-1, 1, 1], [-1, -1, 1]],
  [[1, 1, -1], [-1, 1, -1]],
  [[1, 1, -1], [1, -1, -1]],
  [[1, -1, 1], [1, -1, -1]],
  [[1, -1, 1], [-1, -1, 1]],
];
const DRAW_ON = [0.02, 0.21] as const;

function drawOutlineOn(ctx: CanvasRenderingContext2D, view: View, pose: BoxPose, p: number): void {
  if (p <= 0) return;
  const f = boxFrame(pose);
  const P = (c: C3): Projected => view.project(boxPoint(f, c[0], c[1], c[2]));
  const a = clamp(p / 0.45);
  const bb = clamp((p - 0.45) / 0.55);
  const tips: [number, number, number][] = [];
  ctx.save();
  ctx.strokeStyle = OUTLINE;
  // The same width as drawBox's outline, which box.ts does not export.
  ctx.lineWidth = OUTLINE_RATIO * f.size * view.cam.scale;
  ctx.lineCap = "round";
  ctx.beginPath();
  for (const [edges, k] of [
    [EDGES_A, a],
    [EDGES_B, bb],
  ] as const) {
    if (k <= 0) continue;
    for (const [s, e] of edges) {
      const ps = P(s);
      const pe = P(e);
      const x = lerp(ps.x, pe.x, k);
      const y = lerp(ps.y, pe.y, k);
      ctx.moveTo(ps.x, ps.y);
      ctx.lineTo(x, y);
      // Tips brighten as their edge grows, so they never pile up at a corner.
      if (k < 1) tips.push([x, y, clamp(k * 4) * clamp((1 - k) * 6)]);
    }
  }
  ctx.stroke();
  ctx.restore();
  // Hot pen tips on the growing ends.
  for (const [x, y, a] of tips) glow(ctx, x, y, 26, PALETTE.amberBright, 0.8 * a);
}

/** The box, inflating out of the flat silhouette during the first frames. */
function drawHero(ctx: CanvasRenderingContext2D, view: View, pose: BoxPose, lt: number): void {
  const drawn = progress(DRAW_ON[0], DRAW_ON[1], lt);
  if (drawn >= 1) {
    drawBox(ctx, view, pose);
    return;
  }
  drawBox(ctx, view, { ...pose, outline: 0 });
  drawReveal(ctx, view, pose, lt);
  drawOutlineOn(ctx, view, pose, outCubic(drawn));
}

// The monocle drops in from in front of the box: it falls on a short arc,
// shrinking from 1.8x as it nears the panel while its shadow slides in under
// it and sharpens, seats on the cue frame, and rebounds off the eye as the
// chain swings through. drawBox's own monocle path travels out of the face
// panel without depth, so this scene draws it (same paths and strokes as
// box.ts) until the chain has settled, then hands back to drawBox exactly.
const MONO_SEAT = T_MONO - 0.004;
const MONO_FALL = 0.075;
const MONO_OUT = 0.5;
/** Where the chain meets the ring: the monocle turns about it and the chain hangs from it. */
const JOINT: readonly [number, number] = [33, -16.5];
const RING: readonly [number, number] = [18, -28];

interface MonoclePose {
  /** Offset of the joint from its seat, face px. */
  dx: number;
  dy: number;
  /** Ring turn about the joint, radians. */
  rot: number;
  scale: number;
  /** Chain swing about the joint, relative to the ring, radians. */
  chain: number;
  /** Height above the panel, 0 (seated) to 1 (first frame). */
  h: number;
}

function monoclePose(lt: number): MonoclePose | null {
  if (lt < MONO_SEAT - MONO_FALL || lt >= MONO_SEAT + MONO_OUT) return null;
  if (lt < MONO_SEAT) {
    // Tossed in: it enters already moving and its height drops ever faster
    // into the seat; the chain trails further behind the faster it goes.
    const k = progress(MONO_SEAT - MONO_FALL, MONO_SEAT, lt);
    const h = 1 - (0.3 * k + 0.7 * k * k);
    return { dx: 24 * (1 - k), dy: -36 * h, rot: 0.55 * h, scale: 1 + 0.8 * h, chain: -0.7 * k, h };
  }
  // Seated: a small hop back off the eye, and the chain swings through. The
  // tail is windowed to exactly zero so the handback to drawBox is invisible.
  const d = lt - MONO_SEAT;
  const settle = 1 - smoothstep(MONO_OUT - 0.2, MONO_OUT, d);
  const hop = Math.abs(wobble(lt, MONO_SEAT, 6, 16)) * settle;
  return {
    dx: 0,
    dy: -2 * hop,
    rot: 0.05 * wobble(lt, MONO_SEAT, 5, 12) * settle,
    scale: 1 + 0.06 * hop,
    chain: -0.7 * Math.exp(-8 * d) * Math.cos(TAU * 3 * d) * settle,
    h: 0.075 * hop,
  };
}

function drawMonocle(ctx: CanvasRenderingContext2D, view: View, pose: BoxPose, lt: number): void {
  const m = monoclePose(lt);
  if (!m) return;
  const f = boxFrame(pose);
  const glint = pose.face?.glint ?? 0;
  const paths = monoclePaths();
  ctx.save();
  applyMatrix(ctx, panelMatrix(view, f, "face"));

  // Its shadow on the face panel: offset down and right, away from the key
  // light, soft and spread while it is high, converging under it as it lands.
  const sa = 0.42 * smoothstep(0, 0.05, m.h) * (1 - 0.45 * m.h);
  if (sa > 0.005) {
    ctx.save();
    // Inside the panel's outline (its half width is about 4 face px).
    ctx.beginPath();
    ctx.rect(-48, -59.5, 96, 119);
    ctx.clip();
    const sx = RING[0] + 0.5 * m.dx + 7 * m.h;
    const sy = RING[1] + 0.5 * m.dy + 10 * m.h;
    const soft = lerp(0.8, 4.5, m.h);
    const r0 = 16.5;
    const hw = 2.5 + soft;
    const outer = (r0 + hw * 1.6) * (1 + 0.25 * m.h);
    const g = ctx.createRadialGradient(sx, sy, 0, sx, sy, outer);
    for (let i = 0; i <= 12; i++) {
      const r = (i / 12) * outer;
      const u = (r / (1 + 0.25 * m.h) - r0) / hw;
      g.addColorStop(i / 12, rgba(PALETTE.ink, sa * Math.exp(-u * u * u * u)));
    }
    ctx.fillStyle = g;
    ctx.fillRect(sx - outer, sy - outer, outer * 2, outer * 2);
    ctx.restore();
  }

  ctx.lineCap = "round";
  ctx.lineJoin = "round";
  ctx.translate(JOINT[0] + m.dx, JOINT[1] + m.dy);
  ctx.rotate(m.rot);
  ctx.scale(m.scale, m.scale);
  ctx.translate(-JOINT[0], -JOINT[1]);
  ctx.strokeStyle = INK;
  ctx.lineWidth = 5;
  ctx.stroke(paths.ring);
  ctx.strokeStyle = rgba(CREAM, lerp(0.7, 1, clamp(glint)));
  ctx.lineWidth = 3;
  ctx.stroke(paths.glint);
  ctx.translate(JOINT[0], JOINT[1]);
  ctx.rotate(m.chain);
  ctx.translate(-JOINT[0], -JOINT[1]);
  ctx.strokeStyle = INK;
  ctx.lineWidth = 3.5;
  ctx.stroke(paths.chain);
  ctx.fillStyle = INK;
  ctx.fill(paths.bead);
  ctx.restore();
}

/** A small clink where the rim meets the eye, on the seat frame. */
function drawClink(ctx: CanvasRenderingContext2D, view: View, pose: BoxPose, lt: number): void {
  const k = pulse(lt, MONO_SEAT, 0.006, 0.035);
  if (k < 0.02) return;
  const p = faceToScreen(view, pose, 2.5, -24);
  sparkle(ctx, p.x, p.y, 15 * k, k, 0.3);
}

// Shipping label: this scene stamps it itself (same shape and strokes as
// box.ts) so it can hang over the panel with a soft shadow, slam flat on the
// cue frame, and throw a puff of dust, then hands back to drawBox once the
// rebound has died out.
const LABEL_HIT = T_LABEL - 0.004;
const LABEL_HOVER = 0.055;
const LABEL_OUT = 0.4;
const LABEL_C: readonly [number, number] = [53, 56];

/** Label scale about its center and its height over the panel (0 is flat). */
function labelPose(lt: number): { s: number; h: number } | null {
  if (lt < LABEL_HIT - LABEL_HOVER || lt >= LABEL_HIT + LABEL_OUT) return null;
  if (lt < LABEL_HIT) {
    // Opaque from its first frame at 1.3x, then accelerating onto the panel.
    const s = 1.3 - 0.3 * inCubic(progress(LABEL_HIT - LABEL_HOVER, LABEL_HIT, lt));
    return { s, h: (s - 1) / 0.3 };
  }
  const settle = 1 - smoothstep(LABEL_OUT - 0.15, LABEL_OUT, lt - LABEL_HIT);
  const hop = Math.abs(wobble(lt, LABEL_HIT, 7, 14)) * settle;
  return { s: 1 + 0.03 * hop, h: 0.1 * hop };
}

function drawLabelStamp(ctx: CanvasRenderingContext2D, view: View, pose: BoxPose, lt: number): void {
  const lp = labelPose(lt);
  if (!lp) return;
  const f = boxFrame(pose);
  ctx.save();
  applyMatrix(ctx, panelMatrix(view, f, "right"));
  const [cx, cy] = LABEL_C;
  ctx.save();
  ctx.beginPath();
  ctx.rect(2, 2, 100, 124);
  ctx.clip();
  // Soft shadow while it hangs over the panel: stacked, growing rounded rects.
  if (lp.h > 0.01) {
    const blur = lerp(1, 7, lp.h);
    const ox = 5 * lp.h;
    const oy = 7 * lp.h;
    const w = 50 * lp.s;
    const hgt = 32 * lp.s;
    ctx.fillStyle = rgba(PALETTE.ink, 0.12 * smoothstep(0, 0.1, lp.h));
    for (let i = 0; i < 5; i++) {
      const e = (blur * i) / 4;
      roundedRect(ctx, cx - w / 2 + ox - e, cy - hgt / 2 + oy - e, w + 2 * e, hgt + 2 * e, 3 + e);
      ctx.fill();
    }
  }
  // Contact: the panel darkens around the label for a beat as air is pushed out.
  const press = pulse(lt, LABEL_HIT, 0.004, 0.04);
  if (press > 0.02) {
    ctx.fillStyle = rgba(PALETTE.ink, 0.18 * press);
    for (let i = 0; i < 3; i++) {
      const e = 2 + 3 * i;
      roundedRect(ctx, 28 - e, 40 - e, 50 + 2 * e, 32 + 2 * e, 3 + e);
      ctx.fill();
    }
  }
  ctx.restore();

  ctx.translate(cx, cy);
  ctx.scale(lp.s, lp.s);
  ctx.translate(-cx, -cy);
  drawLabelArt(ctx);
  ctx.restore();
}

// A soft dust puff, cached: one sprite drawn per mote.
let puffSprite: HTMLCanvasElement | null = null;
function puff(): HTMLCanvasElement {
  if (puffSprite) return puffSprite;
  const n = 64;
  puffSprite = makeCanvas(n, n);
  const g = puffSprite.getContext("2d")!;
  const grad = g.createRadialGradient(n / 2, n / 2, 0, n / 2, n / 2, n / 2);
  for (let i = 0; i <= 8; i++) {
    const k = i / 8;
    grad.addColorStop(k, mix(PALETTE.paper, PALETTE.amberBright, 0.35, Math.exp(-4 * k * k) * (1 - k)));
  }
  g.fillStyle = grad;
  g.fillRect(0, 0, n, n);
  return puffSprite;
}

/** Cardboard dust squeezed out from under the label's edges as it lands. */
function drawDust(ctx: CanvasRenderingContext2D, view: View, pose: BoxPose, lt: number): void {
  const d = lt - LABEL_HIT;
  if (d <= 0 || d > 0.24) return;
  const f = boxFrame(pose);
  const sprite = puff();
  ctx.save();
  const base = ctx.globalAlpha;
  applyMatrix(ctx, panelMatrix(view, f, "right"));
  for (let i = 0; i < 9; i++) {
    const h = (k: number) => hash(i, 700 + k);
    const life = 0.16 + 0.1 * h(1);
    const age = d / life;
    if (age >= 1) continue;
    // A point on the label's edge, blown straight out from it: most of the
    // air escapes along the long edges.
    const side = i % 5;
    const u = 0.15 + 0.7 * h(2);
    const [x0, y0, nx, ny] =
      side === 0 || side === 3
        ? [28 + 50 * u, 72, 0, 1]
        : side === 1
          ? [78, 40 + 32 * u, 1, 0]
          : side === 2
            ? [28, 40 + 32 * u, -1, 0]
            : [28 + 50 * u, 40, 0, -1];
    const reach = (8 + 12 * h(3)) * (1 - Math.exp(-age * 5));
    const x = x0 + nx * reach + (h(4) - 0.5) * 6 * age;
    const y = y0 + ny * reach - 4 * age * age;
    const r = (7 + 6 * h(5)) * (0.6 + 1.1 * age);
    ctx.globalAlpha = base * 0.6 * (1 - age) ** 1.6;
    ctx.drawImage(sprite, x - r, y - r, r * 2, r * 2);
  }
  ctx.restore();
}

/**
 * Zoom smear for the snap back: the silhouette at sub-frame steps back along
 * the move, faint and behind the box, so the fastest frames read as one
 * continuous pull instead of a jump. It scales with the zoom speed.
 */
function drawSnapSmear(ctx: CanvasRenderingContext2D, view: View, pose: BoxPose, lt: number): void {
  if (lt < T_PULL || lt > T_PULL + 0.14) return;
  const z = leanTransform(lt)[2];
  const speed = (leanTransform(lt - 1 / 60)[2] - z) / z;
  const a = 0.3 * clamp(speed * 4);
  if (a < 0.01) return;
  const sil = boxSilhouette(view, pose);
  ctx.save();
  // Blurred cardboard and outline average out to a darker amber.
  ctx.fillStyle = PALETTE.amberDeep;
  const base = ctx.globalAlpha;
  const n = 24;
  for (let j = 1; j <= n; j++) {
    // Samples from before the move would all stack on one spot as a hard copy.
    const tj = lt - (j / n) * (1.4 / 60);
    if (tj < T_PULL) break;
    const [ex, ey, zj] = leanTransform(tj);
    ctx.save();
    ctx.transform(zj, 0, 0, zj, ex, ey);
    polygon(ctx, sil);
    ctx.globalAlpha = base * a * 0.36 * (1 - j / (n + 1));
    ctx.fill();
    ctx.restore();
  }
  ctx.restore();
}

/**
 * A shockwave travelling out along the floor from the box's footprint. Under
 * the orthographic camera a floor circle is an exact ellipse; the back half
 * is drawn behind the box and the front half over it. Widths are screen px.
 */
function floorRing(
  ctx: CanvasRenderingContext2D,
  view: View,
  y: number,
  p: number,
  maxR: number,
  width: number,
  alpha: number,
  half: "back" | "front",
  zoom: number,
): void {
  if (p <= 0 || p >= 1) return;
  const e = 1 - (1 - p) ** 3;
  const r = lerp(0.74, maxR, e) * view.cam.scale;
  const c = view.project([0, y, 0]);
  ctx.save();
  ctx.beginPath();
  if (half === "back") ctx.ellipse(c.x, c.y, r, r * Math.sin(view.cam.pitch), 0, Math.PI, TAU);
  else ctx.ellipse(c.x, c.y, r, r * Math.sin(view.cam.pitch), 0, 0, Math.PI);
  ctx.strokeStyle = rgba(PALETTE.amberBright, alpha * (1 - p) ** 1.3);
  ctx.lineWidth = lerp(width, 1.6, p) / zoom;
  ctx.lineCap = "round";
  ctx.stroke();
  ctx.restore();
}

function drawRings(
  ctx: CanvasRenderingContext2D,
  view: View,
  y: number,
  lt: number,
  half: "back" | "front",
  zoom: number,
): void {
  floorRing(ctx, view, y, progress(0.008, 0.25, lt), 2, 7, 0.85, half, zoom);
  // A faint, quicker echo inside it.
  floorRing(ctx, view, y, progress(0.04, 0.2, lt), 1.45, 3, 0.4, half, zoom);
}

// Sparks thrown off by the inflate. Air sparks leave the upper silhouette
// edges on parabolic arcs; floor sparks skid out from the footprint along
// the floor. All of them are gone before the face starts to assemble.
interface Spark {
  floor: boolean;
  t0: number;
  life: number;
  /** Air: launch point and velocity in px and px/s. Floor: angle, start and travel in world units. */
  x: number;
  y: number;
  vx: number;
  vy: number;
  w: number;
}
const GRAVITY = 3400;
const FLOOR_DRAG = 0.07;
let sparks: Spark[] | null = null;
function getSparks(): Spark[] {
  if (sparks) return sparks;
  sparks = [];
  const view = new View(END_CAM);
  const sil = boxSilhouette(view, { ...END_POSE, size: 1.02, pos: [0, -0.51, 0] });
  // Upper edges only: the four edges whose outward normal points up or sideways.
  const cx = CX;
  const cy = BOX_Y;
  const edges: [Projected, Projected][] = [];
  for (let i = 0; i < sil.length; i++) {
    const p = sil[i];
    const q = sil[(i + 1) % sil.length];
    if ((p.y + q.y) / 2 < cy + 40) edges.push([p, q]);
  }
  for (let i = 0; i < 15; i++) {
    const h = (k: number) => hash(i, 900 + k);
    const [p, q] = edges[Math.floor(h(1) * edges.length)];
    const u = 0.15 + 0.7 * h(2);
    const x = lerp(p.x, q.x, u);
    const y = lerp(p.y, q.y, u);
    // Outward from the center, lifted, with a clustered spread.
    const out = Math.atan2(y - cy, x - cx);
    const a = lerp(out, -Math.PI / 2, 0.35) + (h(3) - 0.5) * 0.9;
    const speed = 330 + h(4) ** 1.4 * 640;
    sparks.push({
      floor: false,
      t0: 0.006 + h(5) * 0.025,
      life: 0.15 + h(6) * 0.13,
      x,
      y,
      vx: Math.cos(a) * speed,
      vy: Math.sin(a) * speed,
      w: 1.2 + h(7) * 2,
    });
  }
  for (let i = 0; i < 8; i++) {
    const h = (k: number) => hash(i, 950 + k);
    // Clustered angles around the floor, biased toward the camera.
    const a = (i / 8) * TAU + (h(1) - 0.5) * 0.7 + 0.785;
    const r0 = 0.52 / Math.max(Math.abs(Math.cos(a)), Math.abs(Math.sin(a)));
    sparks.push({
      floor: true,
      t0: 0.004 + h(2) * 0.02,
      life: 0.14 + h(3) * 0.1,
      x: a,
      y: r0,
      vx: 0.7 + h(4) * 1.3,
      vy: 0,
      w: 1.4 + h(5) * 1.6,
    });
  }
  return sparks;
}

function sparkAt(s: Spark, view: View, d: number, floorY: number): [number, number] {
  if (!s.floor) return [s.x + s.vx * d, s.y + s.vy * d + 0.5 * GRAVITY * d * d];
  const r = s.y + s.vx * (1 - Math.exp(-d / FLOOR_DRAG));
  const p = view.project([Math.cos(s.x) * r, floorY, Math.sin(s.x) * r]);
  return [p.x, p.y];
}

function drawSparks(ctx: CanvasRenderingContext2D, view: View, lt: number, floorY: number): void {
  if (lt > 0.33) return;
  ctx.save();
  ctx.lineCap = "round";
  ctx.lineJoin = "round";
  for (const s of getSparks()) {
    const d = lt - s.t0;
    if (d <= 0 || d >= s.life) continue;
    const age = d / s.life;
    const a = 1 - age * age;
    // The trail samples the spark's own path, so it bends along the arc.
    ctx.beginPath();
    for (let k = 3; k >= 0; k--) {
      const [x, y] = sparkAt(s, view, Math.max(0, d - k * 0.014), floorY);
      if (k === 3) ctx.moveTo(x, y);
      else ctx.lineTo(x, y);
    }
    ctx.strokeStyle = mix(PALETTE.paper, PALETTE.amberBright, smoothstep(0, 0.5, age), a);
    ctx.lineWidth = s.w * lerp(1, 0.45, age);
    ctx.stroke();
    const [x, y] = sparkAt(s, view, d, floorY);
    glow(ctx, x, y, 7 + s.w * 3, PALETTE.amber, 0.45 * a);
  }
  ctx.restore();
}

// A big soft backlight. glow()'s 128 px sprite bands when stretched this far,
// so this one has its own larger sprite with a smooth falloff.
let backSprite: HTMLCanvasElement | null = null;
function backlight(ctx: CanvasRenderingContext2D, x: number, y: number, r: number, alpha: number): void {
  if (alpha <= 0) return;
  if (!backSprite) {
    const n = 512;
    backSprite = makeCanvas(n, n);
    const g = backSprite.getContext("2d")!;
    const grad = g.createRadialGradient(n / 2, n / 2, 0, n / 2, n / 2, n / 2);
    for (let i = 0; i <= 16; i++) {
      const k = i / 16;
      grad.addColorStop(k, rgba(PALETTE.amberDeep, Math.exp(-4.5 * k * k) * (1 - k)));
    }
    g.fillStyle = grad;
    g.fillRect(0, 0, n, n);
  }
  ctx.save();
  ctx.globalCompositeOperation = "lighter";
  ctx.globalAlpha *= clamp(alpha);
  ctx.drawImage(backSprite, x - r, y - r, r * 2, r * 2);
  ctx.restore();
}

/** The final glint: drawBox's star plus a small cross star, a bloom, and a short streak. */
function drawGlint(ctx: CanvasRenderingContext2D, view: View, pose: BoxPose, lt: number): void {
  const gl = 0.7 * pulse(lt, T_GLINT, 0.035, 0.11);
  if (gl < 0.01) return;
  const g = faceToScreen(view, pose, 7.5, -38);
  glow(ctx, g.x, g.y, 50 * gl, PALETTE.amberBright, 0.4 * gl);
  ctx.save();
  ctx.translate(g.x, g.y);
  ctx.scale(6, 0.08);
  glow(ctx, 0, 0, 42 * gl, PALETTE.paper, 0.7 * gl);
  ctx.restore();
  // Offset 45° from drawBox's own star so the two read as one eight-point glint.
  sparkle(ctx, g.x, g.y, 22 * gl, gl, 0.6 * gl + Math.PI / 4);
}

// Glyphs whose first ink would be a lone dot above the x-height surface a
// beat later than their neighbours, so the word never starts as a speck.
const TITTLE_LAG = 0.03;

function drawWordmark(ctx: CanvasRenderingContext2D, lt: number): void {
  if (lt < T_WORD) return;
  const line = layout(ctx, WORD, WORD_FONT, WORD_TRACK);
  const n = line.glyphs.length;
  const mid = (n - 1) / 2;
  // Tracking tightens over the rest of the bar and lands on the last frame.
  const extra = 16 * (1 - outCubic(progress(T_WORD, END, lt)));
  ctx.save();
  ctx.beginPath();
  ctx.rect(0, WORD_Y - 104, 1920, 134);
  ctx.clip();
  drawText(ctx, WORD, CX, WORD_Y, {
    font: WORD_FONT,
    tracking: WORD_TRACK,
    align: "center",
    fill: PALETTE.paper,
    glyph: (g, i) => {
      // Center out, each letter leaning away from the middle as it rises.
      const lag = g.ch === "i" || g.ch === "j" ? TITTLE_LAG : 0;
      const t0 = T_WORD + (Math.abs(i - mid) - 0.5) * 0.032 + lag;
      const e = swiftOut(progress(t0, t0 + 0.42, lt));
      if (e <= 0) return null;
      return {
        dx: (i - mid) * extra,
        dy: (1 - e) * 140,
        rot: (1 - e) * 0.08 * Math.sign(i - mid),
      };
    },
  });
  ctx.restore();
}

// Word index of each glyph of the tagline, for a per-word cascade.
const TAG_WORDS = (() => {
  const out: number[] = [];
  let w = 0;
  for (const ch of TAG) {
    out.push(w);
    if (ch === " ") w++;
  }
  return out;
})();

function drawTagline(ctx: CanvasRenderingContext2D, lt: number): void {
  if (lt < T_TAG) return;
  ctx.save();
  ctx.beginPath();
  ctx.rect(0, TAG_Y - 40, 1920, 54);
  ctx.clip();
  drawText(ctx, TAG, CX, TAG_Y, {
    font: TAG_FONT,
    tracking: -0.5,
    align: "center",
    fill: PALETTE.text2,
    glyph: (_g, i) => {
      const t0 = T_TAG + TAG_WORDS[i] * 0.035;
      const e = swiftOut(progress(t0, t0 + 0.4, lt));
      if (e <= 0) return null;
      return { dy: (1 - e) * 46, alpha: clamp(e * 1.6) };
    },
  });
  ctx.restore();
}

// The URL types on with a caret, calling back the typed line in scene 3:
// each key lands warm and cools, and the caret blinks on the beat until the
// glint, then goes out so the poster is clean.
const URL_KEYS: number[] = (() => {
  const out: number[] = [];
  let t = T_URL;
  for (let i = 0; i < URL_TEXT.length; i++) {
    out.push(t);
    t += 0.0095 + hash(i, 61) * 0.007;
  }
  return out;
})();
const CARET_OFF = T_GLINT + b(0.5);

function drawUrl(ctx: CanvasRenderingContext2D, lt: number): void {
  if (lt < T_URL) return;
  const line = layout(ctx, URL_TEXT, URL_FONT, URL_TRACK);
  const x0 = CX - line.width / 2;
  let typed = 0;
  ctx.save();
  ctx.font = URL_FONT;
  ctx.textAlign = "left";
  ctx.textBaseline = "alphabetic";
  line.glyphs.forEach((g, i) => {
    const d = lt - URL_KEYS[i];
    if (d < 0) return;
    typed = i + 1;
    const pop = 1 + 0.3 * Math.exp(-d * 40);
    ctx.save();
    ctx.translate(x0 + g.x + g.w / 2, URL_Y);
    ctx.scale(pop, pop);
    ctx.fillStyle = mix(PALETTE.amberBright, PALETTE.text3, smoothstep(0, 0.1, d));
    ctx.fillText(g.ch, -g.w / 2, 0);
    ctx.restore();
  });
  // Solid while typing, then on for the first half of each beat.
  const done = typed === URL_TEXT.length;
  const on = lt < CARET_OFF && (!done || (lt / BEAT) % 1 < 0.5);
  if (on) {
    const last = line.glyphs[typed - 1];
    const x = typed === 0 ? x0 : x0 + last.x + last.w + 3;
    roundedRect(ctx, x, URL_Y - 17, 11, 22, 1.5);
    ctx.fillStyle = PALETTE.amber;
    ctx.fill();
  }
  ctx.restore();
}

export const scene: Scene = {
  id: "logo",
  start: bar(7),
  end: bar(8),
  draw(ctx, lt, env) {
    ctx.fillStyle = PALETTE.night;
    ctx.fillRect(0, 0, env.W, env.H);
    const view = new View(END_CAM);

    // Handoff 7 → 8: exactly the flat silhouette on the first frame.
    if (lt <= 0) {
      polygon(ctx, boxSilhouette(view, END_POSE));
      ctx.fillStyle = PALETTE.amber;
      ctx.fill();
      return;
    }

    // Night lifts to the site's stage color as the card settles.
    ctx.fillStyle = mix(PALETTE.night, PALETTE.bg, smoothstep(0.3, 1.6, lt));
    ctx.fillRect(0, 0, env.W, env.H);

    const pose = poseAt(lt);
    const size = pose.size ?? 1;
    const floorY = -0.5 * size;
    const float = floatAt(lt);

    ctx.save();
    // A slow push on the whole card that comes to rest on the last frame.
    const push = 1 + 0.025 * outSine(progress(0, END, lt));
    ctx.translate(960, 540);
    ctx.scale(push, push);
    ctx.translate(-960, -540);
    const [sx, sy] = shake(lt, 0.001, 7, 0.05);
    ctx.translate(sx, sy);

    drawSnapSmear(ctx, view, pose, lt);

    // World layer: the close-up on the face.
    ctx.save();
    const [ex, ey, z] = leanTransform(lt);
    ctx.transform(z, 0, 0, z, ex, ey);

    // Warm backlight that lingers, and a tight hot core on the hit.
    backlight(ctx, CX, BOX_Y + 20, 560, 0.2 * progress(0, 0.35, lt));
    const hit = pulse(lt, 1 / 60, 1 / 60, 0.03);
    glow(ctx, CX, BOX_Y, 360, PALETTE.amberBright, 0.75 * hit);
    glow(ctx, CX, BOX_Y, 170, PALETTE.paper, 0.45 * hit);

    drawRings(ctx, view, floorY, lt, "back", z);
    drawSparks(ctx, view, lt, floorY);
    // The shadow stays on the floor and softens a touch as the box floats.
    drawShadow(ctx, view, pose.pos, size, float * 3, 0.5 * progress(0.03, 0.3, lt), floorY);
    drawHero(ctx, view, pose, lt);
    drawLabelStamp(ctx, view, pose, lt);
    drawDust(ctx, view, pose, lt);
    drawMonocle(ctx, view, pose, lt);
    // Exposure pop on the box alone, so the stage stays night.
    const pop = 0.2 * pulse(lt, 1 / 60, 1 / 60, 0.035);
    if (pop > 0.01) {
      ctx.save();
      polygon(ctx, boxSilhouette(view, pose));
      ctx.globalCompositeOperation = "lighter";
      ctx.fillStyle = rgba(PALETTE.amberBright, pop);
      ctx.fill();
      ctx.restore();
    }
    drawRings(ctx, view, floorY, lt, "front", z);
    drawClink(ctx, view, pose, lt);
    drawGlint(ctx, view, pose, lt);
    ctx.restore();

    drawWordmark(ctx, lt);
    drawTagline(ctx, lt);
    drawUrl(ctx, lt);
    ctx.restore();

    flash(ctx, env.W, env.H, 0.04 * pulse(lt, 1 / 60, 1 / 60, 0.03), PALETTE.amberBright);
  },
};
