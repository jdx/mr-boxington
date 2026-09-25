// Scene 6, "Isometric": the store as a small world of boxes. A grid draws
// out, the center cube stomps and the city pops into being on the ripple, a
// scan beam tags the keepers and knocks every other carton flat, and the
// survivors turn into the flat amber discs the liquid-morph scene picks up.

import {
  BEAT,
  beat,
  CUBE,
  cellPose,
  drawStagedBox,
  GRID_R,
  H5_POSE,
  KEEP,
  keptDiscs,
  PALETTE,
  type Scene,
  sec,
  WORLD_CAM,
} from "../bible";
import {
  type BoxPose,
  boxFrame,
  boxPoint,
  boxSilhouette,
  drawBox,
  drawLabelArt,
  OUTLINE,
  OUTLINE_RATIO,
  sparkle,
  TAPE,
  TAPE_DROP,
  TAPE_EDGE,
  TAPE_HALF,
  TAPE_SIDE,
} from "../box";
import { mixRGB, type RGB, rgb, rgba } from "../color";
import { glow, makeCanvas } from "../fx";
import {
  clamp,
  DEG,
  hash,
  inOutCubic,
  inOutSine,
  inQuad,
  keys,
  lerp,
  outBack,
  outCubic,
  outQuad,
  progress,
  smoothstep,
  swiftInOut,
  swiftOut,
  TAU,
} from "../math";
import {
  add,
  applyMatrix,
  type Camera,
  cardboardFill,
  hull,
  mix3,
  mul,
  polygon,
  sub,
  tone,
  type V3,
  View,
} from "../space";

// Scanner and hot-edge tints, derived from the palette so they follow it.
const hex = (c: RGB): string =>
  `#${c.map((v) => Math.round(v).toString(16).padStart(2, "0")).join("")}`;
/** The beam's white-hot core: teal light pushed toward paper. */
const BEAM_CORE = mixRGB(PALETTE.tealLight, PALETTE.paper, 0.6);
/** Rim light the curtain throws on roof edges. */
const RIM = mixRGB(PALETTE.tealLight, PALETTE.paper, 0.45);
/** The stomp ring's leading edge. */
const SHOCK_EDGE = mixRGB(PALETTE.amberBright, PALETTE.paper, 0.6);
/** Keeper glint: green pushed toward paper. */
const GLINT = hex(mixRGB(PALETTE.green, PALETTE.paper, 0.6));

const S = sec("world");

// Beat-locked anchors, local seconds. The score (score/world.ts) is written
// to these.
export const T_STOMP = beat(0.25); // center cube stomps, the ripple leaves
export const T_LAND0 = beat(0.5); // b0.5: first ring touches down
export const T_LAND1 = beat(1.5); // b1.5: the rim lands
export const T_BEAM0 = beat(2); // b2: beam ignites at the back corner
export const T_BEAM1 = beat(3); // b3: prune done, kept cubes glow and hop
export const T_DISC = S.len - 0.02; // exact discs for the final frames

const HALF = CUBE / 2;
const EDGE = GRID_R + 0.5;
// The folding carton matches drawBox's outline weight and tape proportions.
const LW = OUTLINE_RATIO * CUBE; // drawBox's outline, in world units
/** The outermost occupied ring (the four corner cells stay empty). */
export const DLAST = Math.hypot(GRID_R, GRID_R - 1);

// The rain: every cube pops into being at rest above its cell and drops
// under one gravity. The stomp's shock ring passes under the first ring
// three frames after the hit, pops it into being just above the floor, and
// it lands on b0.5.
export const T_POP0 = T_STOMP + 0.05;
const POP = 0.06;
const DROP0 = 0.45;
const DROP1 = 3.4;
const G = (2 * DROP0) / (T_LAND0 - T_POP0) ** 2;
const popScale = outBack(2.2);

// The beam sweeps the diagonal x + z = c from the back corner (-9) on b2,
// crossing one row per 1/64 bar, so it touches the kept rows (-4, 0, 4) on
// b2.25, b2.5, and b2.75. After the last keeper it accelerates out of the
// front corner, so every carton is flat by b3.
const V_BEAM = 16 / BEAT;
export const T_FAST = T_BEAM0 + 0.75 * BEAT;
export const T_BEAMEND = T_BEAM0 + (7 / 8) * BEAT;
const C_FAST = -9 + V_BEAM * (T_FAST - T_BEAM0);
const K_BEAM = (9 - C_FAST - V_BEAM * (T_BEAMEND - T_FAST)) / (T_BEAMEND - T_FAST) ** 2;
function beamPos(t: number): number {
  if (t <= T_FAST) return -9 + V_BEAM * (t - T_BEAM0);
  const u = Math.min(t, T_BEAMEND) - T_FAST;
  return C_FAST + V_BEAM * u + K_BEAM * u * u;
}
/** When the beam reaches a row: one unit early, as it meets the cubes' back corners. */
export function beamAt(rank: number): number {
  const c = rank - 1;
  if (c <= C_FAST) return T_BEAM0 + (c + 9) / V_BEAM;
  const d = c - C_FAST;
  return T_FAST + (-V_BEAM + Math.sqrt(V_BEAM * V_BEAM + 4 * K_BEAM * d)) / (2 * K_BEAM);
}

interface Cell {
  x: number;
  z: number;
  /** Distance from the center cell; each ring of equal distance lands together. */
  d: number;
  rank: number;
  keep: boolean;
  center: boolean;
  /** First ring: lands on b0.5 with the camera bump. */
  first: boolean;
  /** Pop-in and touchdown times, and the drop height between them. */
  spawn: number;
  land: number;
  drop: number;
  /** When the beam reaches it: kept cubes get tagged, the rest fold flat. */
  hit: number;
  /** Fold duration scale: the rows the accelerating beam reaches fold faster. */
  fs: number;
  /** A few degrees of yaw so the block reads as stacked crates, not tiles. */
  yaw: number;
  ex: V3;
  ez: V3;
  /** Fold direction, away from the viewer onto floor the beam has cleared. */
  dir: V3;
  tape: boolean;
  label: boolean;
  seed: number;
}

let cells: Cell[] | null = null;
/** The city: every cell that gets a carton, the center cube included. */
export function world(): Cell[] {
  if (cells) return cells;
  const key = (x: number, z: number) => `${x},${z}`;
  const keep = new Set(KEEP.map(([x, z]) => key(x, z)));
  const out: Cell[] = [];
  for (let x = -GRID_R; x <= GRID_R; x++) {
    for (let z = -GRID_R; z <= GRID_R; z++) {
      const k = keep.has(key(x, z));
      const seed = (x + 16) * 64 + z + 16;
      const d = Math.hypot(x, z);
      const center = x === 0 && z === 0;
      // A few holes, mostly toward the rim, so the block reads as a city
      // rather than a slab; the four corners stay open to soften the diamond.
      const corner = Math.abs(x) === GRID_R && Math.abs(z) === GRID_R;
      if (!k && !center && (corner || hash(seed, 3) < 0.07 + 0.3 * smoothstep(3.6, 5.2, d)))
        continue;
      const f = center ? 0 : (d - 1) / (DLAST - 1);
      const land = center ? -1 : lerp(T_LAND0, T_LAND1, f);
      const vary = d > 1 ? lerp(0.88, 1.12, hash(seed, 13)) : 1;
      let drop = center ? 0 : lerp(DROP0, DROP1, f) * vary;
      // Nothing behind the first ring may pop in before it lands on b0.5,
      // so that landing is the one that sets off the next wave.
      if (d > 1) drop = Math.min(drop, 0.5 * G * (land - T_LAND0) ** 2);
      const yaw = center || k ? 0 : (hash(seed, 31) - 0.5) * 8 * DEG;
      const ex: V3 = [Math.cos(yaw), 0, -Math.sin(yaw)];
      const ez: V3 = [Math.sin(yaw), 0, Math.cos(yaw)];
      // Fold back (-x or -z), but never onto a kept neighbor.
      let alongX = hash(seed, 9) < 0.5;
      if (keep.has(alongX ? key(x - 1, z) : key(x, z - 1))) alongX = !alongX;
      out.push({
        x,
        z,
        d,
        rank: x + z,
        keep: k,
        center,
        first: !center && d === 1,
        spawn: center ? -1 : Math.max(T_POP0, land - Math.sqrt((2 * drop) / G)),
        land,
        drop,
        hit: beamAt(x + z),
        fs: lerp(1, 0.6, progress(T_FAST, T_BEAMEND, beamAt(x + z))),
        yaw,
        ex,
        ez,
        dir: mul(alongX ? ex : ez, -1),
        tape: !center && !k && hash(seed, 5) < 0.34,
        label: !center && !k && hash(seed, 17) < 0.13,
        seed,
      });
    }
  }
  cells = out;
  return out;
}

/** A small vertical camera kick (px) on the stomp and on the first touchdown. */
function bump(t: number): number {
  let y = 0;
  for (const [at, amp] of [
    [T_STOMP, 2.5],
    [T_LAND0, 4],
  ]) {
    const u = t - at;
    if (u >= 0 && u < 0.3) y += amp * Math.exp(-u / 0.045) * Math.cos(TAU * 8 * u);
  }
  return y;
}

// The camera cranes up and orbits a little while the city builds, then
// settles back onto WORLD_CAM before the discs form.
const sway = keys([
  [0, 0],
  [0.86, 1, inOutSine],
  [1.6, 0, inOutCubic],
]);
function camera(t: number): Camera {
  const s = sway(t);
  const b = bump(t);
  if (s <= 0 && b === 0) return WORLD_CAM;
  return {
    ...WORLD_CAM,
    yaw: WORLD_CAM.yaw - 12 * DEG * s,
    pitch: WORLD_CAM.pitch + 9 * DEG * s,
    scale: WORLD_CAM.scale * (1 + 0.14 * s),
    cy: WORLD_CAM.cy - 10 * s + b,
  };
}

/** The stomp's shock ring (world units): fast, it pops the first ring into being. */
const SHOCK = 0.32;
const shockRadius = (u: number): number => lerp(0.42, 2.6, outCubic(progress(0, SHOCK, u)));
/** The landing front: it reaches each ring as that ring touches down. */
const frontRadius = (t: number): number =>
  1 + ((t - T_LAND0) / (T_LAND1 - T_LAND0)) * (DLAST - 1);

function floorMatrix(ctx: CanvasRenderingContext2D, view: View, x: number, z: number): void {
  applyMatrix(ctx, view.planeMatrix([x, 0, z], [1, 0, 0], [0, 0, 1]));
}

/** b3: the floor flashes as the carpet drops through it. */
const floorFlash = (t: number): number => (t >= T_BEAM1 ? Math.exp(-(t - T_BEAM1) / 0.08) : 0);

/** The flash's wash, laid over the carpet and under everything standing. */
function drawFloorFlash(ctx: CanvasRenderingContext2D, view: View, t: number): void {
  const flash = floorFlash(t);
  if (flash <= 0.01) return;
  ctx.save();
  ctx.globalCompositeOperation = "lighter";
  polygon(ctx, [
    view.project([-EDGE, 0, -EDGE]),
    view.project([EDGE, 0, -EDGE]),
    view.project([EDGE, 0, EDGE]),
    view.project([-EDGE, 0, EDGE]),
  ]);
  ctx.fillStyle = rgba(PALETTE.tealLight, 0.16 * flash);
  ctx.fill();
  ctx.restore();
}

// Floor grid: every line grows out from its point nearest the center, with a
// bright pen tip while it travels, then retracts to the center at the end.
function drawGrid(ctx: CanvasRenderingContext2D, view: View, t: number): void {
  const out = 7 * swiftOut(progress(0, 0.55, t));
  const back = 7 * (1 - inOutCubic(progress(T_BEAM1 - 0.02, 1.72, t)));
  const R = Math.min(out, back);
  const fade = 1 - smoothstep(T_BEAM1 + 0.05, 1.72, t);
  if (R <= 0 || fade <= 0) return;
  const flash = floorFlash(t);
  const tips: [number, number][] = [];
  ctx.save();
  ctx.lineCap = "round";
  ctx.strokeStyle = rgba(PALETTE.tealLight, (0.2 + 0.5 * flash) * fade);
  ctx.lineWidth = 1.5;
  ctx.beginPath();
  for (let i = 0; i <= GRID_R * 2 + 1; i++) {
    const a = -EDGE + i;
    if (Math.abs(a) >= R) continue;
    const L = Math.min(EDGE, Math.sqrt(R * R - a * a));
    for (const along of [0, 1]) {
      const p0 = view.project(along ? [-L, 0, a] : [a, 0, -L]);
      const p1 = view.project(along ? [L, 0, a] : [a, 0, L]);
      ctx.moveTo(p0.x, p0.y);
      ctx.lineTo(p1.x, p1.y);
      if (L < EDGE) tips.push([p0.x, p0.y], [p1.x, p1.y]);
    }
  }
  ctx.stroke();

  // Cell corners as small ticks, popping as the reveal passes them.
  ctx.fillStyle = rgba(PALETTE.paper, 0.26 * fade);
  for (let i = 0; i <= GRID_R * 2 + 1; i++) {
    for (let j = 0; j <= GRID_R * 2 + 1; j++) {
      const a = -EDGE + i;
      const b = -EDGE + j;
      const s = clamp((R - Math.hypot(a, b)) * 1.5);
      if (s <= 0) continue;
      const p = view.project([a, 0, b]);
      const r = 2.2 * s;
      ctx.fillRect(p.x - r, p.y - r * 0.5, r * 2, r);
    }
  }
  ctx.restore();

  const tipA = (1 - progress(0.3, 0.55, t)) * fade;
  if (tipA > 0) {
    ctx.save();
    ctx.fillStyle = rgba(PALETTE.amberBright, 0.9 * tipA);
    for (const [x, y] of tips) {
      ctx.beginPath();
      ctx.arc(x, y, 2.4, 0, TAU);
      ctx.fill();
    }
    ctx.restore();
    for (let i = 0; i < tips.length; i += 3)
      glow(ctx, tips[i][0], tips[i][1], 16, PALETTE.amber, 0.35 * tipA);
  }
}

// drawShadow's contact shadow from one cached sprite: seventy radial
// gradients a frame are the scene's biggest cost otherwise.
let shadowSprite: HTMLCanvasElement | null = null;
function shadow(
  ctx: CanvasRenderingContext2D,
  view: View,
  x: number,
  z: number,
  lift: number,
  alpha: number,
): void {
  const a = alpha / (1 + lift * 2.5);
  if (a <= 0.003) return;
  if (!shadowSprite) {
    shadowSprite = makeCanvas(128, 128);
    const g = shadowSprite.getContext("2d")!;
    const grad = g.createRadialGradient(64, 64, 0, 64, 64, 64);
    grad.addColorStop(0, "rgba(0,0,0,1)");
    grad.addColorStop(0.55, "rgba(0,0,0,0.5)");
    grad.addColorStop(1, "rgba(0,0,0,0)");
    g.fillStyle = grad;
    g.fillRect(0, 0, 128, 128);
  }
  const r = 0.78 * (1 + lift * 0.9);
  ctx.save();
  applyMatrix(ctx, view.planeMatrix([x, 0, z], [CUBE, 0, 0], [0, 0, CUBE]));
  ctx.globalAlpha *= a;
  ctx.drawImage(shadowSprite, -r, -r, r * 2, r * 2);
  ctx.restore();
}

// A standing (or falling) cube at time t, or null before it appears.
interface Standing {
  pose: BoxPose;
  lift: number;
  /** Fall speed, world units per second, for the motion smear. */
  speed: number;
  /** 0..1 spawn spark. */
  spark: number;
}
const stompSquash = keys([
  [0, 1],
  [0.035, 0.8, inOutSine],
  [0.052, 1.14, outQuad],
  [0.085, 1, inOutSine],
  [T_STOMP, 1.08, inQuad],
]);
/** Kept cubes hop on b3 and land as discs on the bar line. */
const HOP = 1.05;
const hopSquash = keys([
  [T_BEAM1, 0.8],
  [T_BEAM1 + 0.05, 1.16, outQuad],
  [1.62, 1, inOutSine],
]);
function standing(c: Cell, t: number): Standing | null {
  let lift = 0;
  let squash = 1;
  let size = CUBE;
  let speed = 0;
  let spark = 0;
  let tiltX = 0;
  let tiltZ = 0;
  let flare = 0;
  if (c.center) {
    // Wind up, hop, and stomp on b0.25: the hit that sends the ripple.
    if (t < T_STOMP) {
      squash = stompSquash(t);
      const s = progress(0.035, T_STOMP, t);
      lift = 0.55 * 4 * s * (1 - s);
    } else {
      const u = t - T_STOMP;
      squash = 1 - 0.25 * Math.exp(-9 * u) * Math.cos(TAU * 4 * u);
      flare = 0.3 * Math.exp(-u / 0.06);
    }
  } else if (t < c.land) {
    if (t < c.spawn) return null;
    // Pops in at rest, full opacity, hot, and falls under gravity; the
    // stretch follows speed.
    const u = t - c.spawn;
    const fall = (t - c.spawn) / (c.land - c.spawn);
    lift = Math.max(0, c.drop - 0.5 * G * u * u);
    speed = G * u;
    squash = 1 + 0.3 * fall * fall;
    size = CUBE * Math.max(0.02, popScale(progress(0, POP, u)));
    spark = 1 - progress(0, 0.09, u);
    flare = 0.32 * (1 - smoothstep(0, 0.08, u));
    const w = (1 - fall) * (1 - fall);
    tiltX = (hash(c.seed, 7) - 0.5) * 0.45 * w;
    tiltZ = (hash(c.seed, 8) - 0.5) * 0.45 * w;
  } else {
    const u = t - c.land;
    squash = 1 - 0.3 * Math.exp(-9 * u) * Math.cos(TAU * 4.2 * u);
    flare = (c.first ? 0.4 : 0.18) * Math.exp(-u / 0.06);
    // The short second-ring drops land before their pop has settled.
    size = CUBE * popScale(progress(0, POP, t - c.spawn));
  }
  if (c.keep) {
    if (t >= c.hit && t < T_BEAM1) {
      // Tagged by the beam: a quick perk upward.
      const u = t - c.hit;
      squash *= 1 + 0.14 * Math.exp(-9 * u) * Math.sin(TAU * 3.6 * u);
    }
    // Wind up under the last of the beam, then hop on b3.
    if (t < T_BEAM1) squash *= lerp(1, 0.8, inOutSine(progress(T_BEAM1 - 0.1, T_BEAM1, t)));
    else {
      const s = progress(T_BEAM1, T_DISC, t);
      lift = HOP * 4 * s * (1 - s);
      squash = hopSquash(t);
    }
  }
  return {
    pose: cellPose(c.x, c.z, {
      pos: [c.x, lift, c.z],
      size,
      squash,
      yaw: c.yaw,
      tiltX,
      tiltZ,
      tape: c.tape ? 1 : 0,
      label: c.label ? 1 : 0,
      toneShift: flare,
    }),
    lift,
    speed,
    spark,
  };
}

function bounds(pts: readonly { x: number; y: number }[]): [number, number, number, number] {
  let x0 = Infinity;
  let y0 = Infinity;
  let x1 = -Infinity;
  let y1 = -Infinity;
  for (const p of pts) {
    if (p.x < x0) x0 = p.x;
    if (p.y < y0) y0 = p.y;
    if (p.x > x1) x1 = p.x;
    if (p.y > y1) y1 = p.y;
  }
  return [x0, y0, x1, y1];
}

// Motion blur: the hull of the cube's silhouette now and where it was about a
// frame ago (never more than a cube height back), filled translucent kraft
// that fades toward the old position. It reads as the cube's own motion, not
// as a light.
const BLUR_DT = 1.3 / 60;
function drawSmear(ctx: CanvasRenderingContext2D, view: View, st: Standing): void {
  const back = Math.min(st.speed * BLUR_DT, CUBE);
  if (back * view.cam.scale < 6) return;
  const [x, y, z] = st.pose.pos;
  const now = boxSilhouette(view, st.pose);
  const was = boxSilhouette(view, { ...st.pose, pos: [x, y + back, z] });
  const [, y0] = bounds(was);
  const [, , , y1] = bounds(now);
  const g = ctx.createLinearGradient(0, y0, 0, y1);
  g.addColorStop(0, rgba(PALETTE.amber, 0));
  g.addColorStop(0.7, rgba(PALETTE.amber, 0.2));
  g.addColorStop(1, rgba(PALETTE.amber, 0.3));
  polygon(ctx, hull([...now, ...was]));
  ctx.fillStyle = g;
  ctx.fill();
}

// A pruned carton gets knocked flat: it darkens as the beam reaches it,
// rocks toward the viewer, falls back onto the cleared row, and slaps down
// with a bounce. The flattened cartons lie behind the beam as a carpet until
// b3, when the whole carpet drops through the floor at once.
const ROCK = 0.025;
/** A carton slaps flat this long after the beam reaches it, in its fold time. */
export const SLAP = 0.07;
const SETTLE = 0.1;
function foldAngle(u: number): number {
  if (u < ROCK) return -0.12 * outQuad(u / ROCK);
  if (u < SLAP) return lerp(-0.12, Math.PI / 2, inQuad((u - ROCK) / (SLAP - ROCK)));
  return Math.PI / 2 - 0.1 * Math.sin(Math.PI * progress(SLAP, SETTLE, u));
}
/** Fold progress in the carton's own (tail-compressed) time. */
const foldU = (c: Cell, t: number): number => (t - c.hit) / c.fs;
const isFlat = (c: Cell, t: number): boolean => !c.keep && foldU(c, t) >= SLAP;

// The b3 drop: every flat carton slides straight down into its own
// footprint, as if through a trapdoor, and is gone in about five frames.
const SINK_DEPTH = 0.75;
const sinkDur = (c: Cell): number => 0.055 + 0.025 * hash(c.seed, 41);
const sinkP = (c: Cell, t: number): number => progress(T_BEAM1, T_BEAM1 + sinkDur(c), t);

function drawFold(ctx: CanvasRenderingContext2D, view: View, c: Cell, t: number): void {
  const u = foldU(c, t);
  const sink = inQuad(sinkP(c, t));
  if (sink >= 1) return;
  const phi = foldAngle(u);
  const h = HALF;
  const { ex, ez } = c;
  const d = c.dir;
  const sp = Math.sin(phi);
  const cp = Math.cos(phi);
  // Vertical edges lean along d; the top stays level (a shear, like a
  // knocked-down box).
  const w: V3 = [d[0] * sp * 2 * h, cp * 2 * h, d[2] * sp * 2 * h];
  const P = (lx: number, lz: number, ly: number): V3 => [
    c.x + (ex[0] * lx + ez[0] * lz) * h + w[0] * ly,
    w[1] * ly,
    c.z + (ex[2] * lx + ez[2] * lz) * h + w[2] * ly,
  ];
  // A wall's normal tilts with the shear when it faces along d.
  const wallN = (e: V3): V3 => {
    const s = e[0] * d[0] + e[2] * d[2];
    return [e[0] - s * d[0] * (1 - cp), -s * sp, e[2] - s * d[2] * (1 - cp)];
  };
  const nx = mul(ex, -1);
  const nz = mul(ez, -1);
  const faces: { q: V3[]; n: V3; id: string }[] = [
    { id: "top", q: [P(-1, -1, 1), P(1, -1, 1), P(1, 1, 1), P(-1, 1, 1)], n: [0, 1, 0] },
    { id: "x+", q: [P(1, -1, 0), P(1, 1, 0), P(1, 1, 1), P(1, -1, 1)], n: wallN(ex) },
    { id: "x-", q: [P(-1, -1, 0), P(-1, 1, 0), P(-1, 1, 1), P(-1, -1, 1)], n: wallN(nx) },
    { id: "z+", q: [P(-1, 1, 0), P(1, 1, 0), P(1, 1, 1), P(-1, 1, 1)], n: wallN(ez) },
    { id: "z-", q: [P(-1, -1, 0), P(1, -1, 0), P(1, -1, 1), P(-1, -1, 1)], n: wallN(nz) },
  ];
  const mid = P(0, 0, 0.5);
  // Condemned: already backlit dark as the beam arrives (BACKLIT), darker
  // as it passes, and it stays dim once flat so the keepers own the frame.
  const flat = smoothstep(0.03, SLAP + 0.01, u);
  const dim = -lerp(BACKLIT, 0.36, smoothstep(0, 0.02, u)) * (1 - 0.5 * flat);
  const lw = LW * view.cam.scale;
  const outline = hull(
    [-1, 1]
      .flatMap((a) => [-1, 1].flatMap((b) => [P(a, b, 0), P(a, b, 1)]))
      .map((q) => view.project(q)),
  );

  const paint = () => {
    ctx.lineJoin = "round";
    ctx.strokeStyle = OUTLINE;
    ctx.lineWidth = lw;
    const shown = new Set<string>();
    for (const f of faces) {
      if (!view.facing(f.n, mid)) continue;
      shown.add(f.id);
      const pts = f.q.map((q) => view.project(q));
      polygon(ctx, pts);
      ctx.fillStyle = cardboardFill(ctx, pts, tone(view, f.n) + dim);
      ctx.fill();
      ctx.stroke();
    }
    const strip = (q: V3[], fill: string) => {
      polygon(
        ctx,
        q.map((p) => view.project(p)),
      );
      ctx.fillStyle = fill;
      ctx.fill();
      ctx.save();
      ctx.lineWidth = lw / 2;
      ctx.strokeStyle = TAPE_EDGE;
      ctx.stroke();
      ctx.restore();
    };
    if (c.tape) {
      // Tape across the top, and its end down the +x panel, as drawBox lays it.
      const hw = TAPE_HALF;
      if (shown.has("top")) strip([P(-1, -hw, 1), P(1, -hw, 1), P(1, hw, 1), P(-1, hw, 1)], TAPE);
      const drop = 1 - TAPE_DROP;
      if (shown.has("x+"))
        strip([P(1, hw, 1), P(1, -hw, 1), P(1, -hw, drop), P(1, hw, drop)], TAPE_SIDE);
    }
    if (c.label && shown.has("x+")) {
      // The shipping label rides the +x panel in the logo's own units.
      const o = P(1, 1, 1);
      const ax = mul(sub(P(1, -1, 1), o), 1 / 104);
      const ay = mul(sub(P(1, 1, 0), o), 1 / 128);
      ctx.save();
      applyMatrix(ctx, view.planeMatrix(o, ax, ay));
      drawLabelArt(ctx);
      ctx.restore();
    }
    if (flat > 0) {
      polygon(ctx, outline);
      ctx.fillStyle = rgba(PALETTE.bg, 0.42 * flat);
      ctx.fill();
    }
  };

  ctx.save();
  if (sink <= 0) {
    paint();
    ctx.restore();
    return;
  }
  // Through the floor: the carton's flat outline becomes a hole, and the
  // carton slides down inside it, darkening as it goes; the trapdoor shuts
  // behind it over the last few frames.
  const o = view.project([0, 0, 0]);
  const dn = view.project([0, -SINK_DEPTH * sink, 0]);
  const shut = 1 - smoothstep(0.55, 1, sink);
  polygon(ctx, outline);
  ctx.fillStyle = rgba(PALETTE.night, 0.85 * shut);
  ctx.fill();
  ctx.clip();
  ctx.translate(dn.x - o.x, dn.y - o.y);
  ctx.globalAlpha *= shut;
  paint();
  polygon(ctx, outline);
  ctx.fillStyle = rgba(PALETTE.night, 0.85 * smoothstep(0, 0.7, sink));
  ctx.fill();
  ctx.restore();
}

// Dust where a carton slaps down: air squeezed out from under it throws a
// few soft kraft-grey puffs out along the floor, flat in the floor plane,
// from its long sides and far end. Drawn in the floor pass, over the carpet
// and under everything standing, and gone by the b3 drop.
let dustSprite: HTMLCanvasElement | null = null;
const DUST_TINT = mixRGB(PALETTE.amber, PALETTE.text2, 0.55);
const DUST = 0.13;
/** Puff origins from the cell center (along d and across it, in half cube widths) and headings. */
const PUFFS: readonly [along: number, across: number, dirAlong: number, dirAcross: number][] = [
  [-0.4, 1, 0.25, 1],
  [1.4, 1, 0.4, 1],
  [-0.4, -1, 0.25, -1],
  [1.4, -1, 0.4, -1],
  [3, 0.45, 1, 0.35],
  [3, -0.45, 1, -0.35],
];
function drawDust(ctx: CanvasRenderingContext2D, view: View, c: Cell, t: number): void {
  const u = (t - (c.hit + SLAP * c.fs)) / DUST;
  if (u <= 0 || u >= 1) return;
  const a = 0.34 * (1 - u) ** 1.5 * (1 - smoothstep(T_BEAM1 - 0.03, T_BEAM1 + 0.02, t));
  if (a <= 0.01) return;
  if (!dustSprite) {
    dustSprite = makeCanvas(64, 64);
    const g = dustSprite.getContext("2d")!;
    const grad = g.createRadialGradient(32, 32, 0, 32, 32, 32);
    grad.addColorStop(0, rgba(DUST_TINT, 1));
    grad.addColorStop(0.45, rgba(DUST_TINT, 0.45));
    grad.addColorStop(1, rgba(DUST_TINT, 0));
    g.fillStyle = grad;
    g.fillRect(0, 0, 64, 64);
  }
  const d = c.dir;
  const e = swiftOut(u);
  ctx.save();
  floorMatrix(ctx, view, c.x, c.z);
  ctx.globalAlpha *= a;
  for (let i = 0; i < PUFFS.length; i++) {
    const [al, ac, da, dc] = PUFFS[i];
    const s = c.seed * 8 + i;
    // Floor-local axes: along d, and across it.
    const ox = (d[0] * al - d[2] * ac) * HALF;
    const oz = (d[2] * al + d[0] * ac) * HALF;
    const dl = Math.hypot(da, dc);
    const dx = (d[0] * da - d[2] * dc) / dl;
    const dz = (d[2] * da + d[0] * dc) / dl;
    const travel = (0.12 + 0.3 * hash(s, 22)) * e;
    const r = lerp(0.1, 0.26 + 0.1 * hash(s, 24), e);
    ctx.save();
    ctx.translate(ox + dx * travel, oz + dz * travel);
    ctx.rotate(Math.atan2(dz, dx));
    ctx.drawImage(dustSprite, -r * 1.5, -r * 0.8, r * 3, r * 1.6);
    ctx.restore();
  }
  ctx.restore();
}

// The scan beam: a bright line on the floor riding the grid's border, with a
// light curtain rising from it, sweeping from the back corner to the front.
function beamLevel(t: number): number {
  return (
    smoothstep(T_BEAM0 - 0.001, T_BEAM0 + 0.025, t) *
    (1 - smoothstep(T_BEAMEND - 0.015, T_BEAMEND + 0.035, t))
  );
}

function beamSegment(c: number): [V3, V3] | null {
  const x1 = Math.max(-EDGE, c - EDGE);
  const x2 = Math.min(EDGE, c + EDGE);
  if (x2 - x1 < 0.02) return null;
  return [
    [x1, 0, c - x1],
    [x2, 0, c - x2],
  ];
}

function gridClip(ctx: CanvasRenderingContext2D, view: View): void {
  polygon(ctx, [
    view.project([-EDGE, 0, -EDGE]),
    view.project([EDGE, 0, -EDGE]),
    view.project([EDGE, 0, EDGE]),
    view.project([-EDGE, 0, EDGE]),
  ]);
  ctx.clip();
}

function drawBeamFloor(ctx: CanvasRenderingContext2D, view: View, t: number): void {
  const lvl = beamLevel(t);
  if (lvl <= 0) return;
  const c = beamPos(t);
  // A cool wash over the floor the beam has just crossed.
  ctx.save();
  gridClip(ctx, view);
  const a = view.project([c / 2, 0, c / 2]);
  const b = view.project([c / 2 - 1.1, 0, c / 2 - 1.1]);
  const g = ctx.createLinearGradient(a.x, a.y, b.x, b.y);
  g.addColorStop(0, rgba(PALETTE.tealLight, 0.22 * lvl));
  g.addColorStop(1, rgba(PALETTE.tealLight, 0));
  ctx.globalCompositeOperation = "lighter";
  ctx.fillStyle = g;
  const W = 12;
  polygon(ctx, [
    view.project([c / 2 - W, 0, c / 2 + W]),
    view.project([c / 2 + W, 0, c / 2 - W]),
    view.project([c / 2 + W - 1.2, 0, c / 2 - W - 1.2]),
    view.project([c / 2 - W - 1.2, 0, c / 2 + W - 1.2]),
  ]);
  ctx.fill();
  ctx.restore();
}

// The beam is a gantry: a laser bar just above roof height, carried on two
// short posts that ride the floor rails, with a faint light sheet between
// the bar and the floor line. The bar sits above every roof, so it is drawn
// over the whole city; the posts and sheet stand in the rows' painter order.
const BAR_Y = CUBE + 0.08;

let curtain: HTMLCanvasElement | null = null;
function curtainSprite(): HTMLCanvasElement {
  if (curtain) return curtain;
  const W = 128;
  const H = 64;
  curtain = makeCanvas(W, H);
  const g = curtain.getContext("2d")!;
  const img = g.createImageData(W, H);
  const lit = rgb(PALETTE.tealLight);
  const body = rgb(PALETTE.teal);
  for (let y = 0; y < H; y++) {
    const v = 1 - y / (H - 1); // 0 at the floor, 1 at the bar
    // Bright where it meets the floor and hangs under the bar, faint between.
    const a = 0.42 * Math.exp(-v / 0.1) + 0.06 + 0.3 * Math.exp(-(1 - v) / 0.07);
    const k = smoothstep(0, 0.3, v) * smoothstep(1, 0.7, v);
    for (let x = 0; x < W; x++) {
      const u = x / (W - 1);
      const e = smoothstep(0, 0.06, u) * smoothstep(1, 0.94, u);
      const i = (y * W + x) * 4;
      img.data[i] = lerp(lit[0], body[0], k);
      img.data[i + 1] = lerp(lit[1], body[1], k);
      img.data[i + 2] = lerp(lit[2], body[2], k);
      img.data[i + 3] = 255 * a * e;
    }
  }
  g.putImageData(img, 0, 0);
  return curtain;
}

/** 1 on each keeper sixteenth (b2.25, b2.5, b2.75), gone in about two frames. */
function beamPulse(t: number): number {
  let p = 0;
  for (let k = 1; k <= 3; k++) {
    const u = t - (T_BEAM0 + (k * BEAT) / 4);
    if (u >= 0 && u < 0.15) p = Math.max(p, Math.exp(-u / 0.022));
  }
  return p;
}

function strokeLine(
  ctx: CanvasRenderingContext2D,
  a: { x: number; y: number },
  b: { x: number; y: number },
  passes: readonly (readonly [width: number, alpha: number, color: string | RGB])[],
): void {
  for (const [wd, al, col] of passes) {
    if (al <= 0) continue;
    ctx.strokeStyle = rgba(col, al);
    ctx.lineWidth = wd;
    ctx.beginPath();
    ctx.moveTo(a.x, a.y);
    ctx.lineTo(b.x, b.y);
    ctx.stroke();
  }
}

/** The sheet, floor line, and posts, drawn between the rows. */
function drawBeamCurtain(ctx: CanvasRenderingContext2D, view: View, t: number): void {
  const lvl = beamLevel(t);
  if (lvl <= 0) return;
  const seg = beamSegment(beamPos(t));
  if (!seg) return;
  const [A, B] = seg;
  const pa = view.project(A);
  const pb = view.project(B);
  const ta = view.project(add(A, [0, BAR_Y, 0]));
  const tb = view.project(add(B, [0, BAR_Y, 0]));
  ctx.save();
  ctx.globalCompositeOperation = "lighter";
  // One cached sprite mapped onto the sheet's screen parallelogram.
  const sprite = curtainSprite();
  ctx.save();
  ctx.transform(
    (tb.x - ta.x) / sprite.width,
    (tb.y - ta.y) / sprite.width,
    (pa.x - ta.x) / sprite.height,
    (pa.y - ta.y) / sprite.height,
    ta.x,
    ta.y,
  );
  ctx.globalAlpha *= lvl;
  ctx.drawImage(sprite, 0, 0);
  ctx.restore();
  ctx.lineCap = "round";
  strokeLine(ctx, pa, pb, [
    [22, 0.06 * lvl, PALETTE.tealLight],
    [8, 0.18 * lvl, PALETTE.tealLight],
    [2.5, 0.8 * lvl, BEAM_CORE],
  ]);
  for (const [f, top] of [
    [pa, ta],
    [pb, tb],
  ]) {
    strokeLine(ctx, f, top, [
      [10, 0.12 * lvl, PALETTE.tealLight],
      [3, 0.9 * lvl, BEAM_CORE],
    ]);
  }
  ctx.restore();
  // The feet ride the border the arming tips traced.
  glow(ctx, pa.x, pa.y, 44, PALETTE.tealLight, 0.7 * lvl);
  glow(ctx, pb.x, pb.y, 44, PALETTE.tealLight, 0.7 * lvl);
}

/** The laser bar over the rooftops, drawn over the whole city. */
function drawBeamBar(ctx: CanvasRenderingContext2D, view: View, t: number): void {
  const lvl = beamLevel(t);
  if (lvl <= 0) return;
  const seg = beamSegment(beamPos(t));
  if (!seg) return;
  const [A, B] = seg;
  const ta = view.project(add(A, [0, BAR_Y, 0]));
  const tb = view.project(add(B, [0, BAR_Y, 0]));
  const p = beamPulse(t);
  ctx.save();
  ctx.globalCompositeOperation = "lighter";
  ctx.lineCap = "round";
  strokeLine(ctx, ta, tb, [
    [34 + 18 * p, (0.07 + 0.1 * p) * lvl, PALETTE.tealLight],
    [13 + 5 * p, (0.22 + 0.2 * p) * lvl, PALETTE.tealLight],
    [4 + 2.5 * p, 0.95 * lvl, BEAM_CORE],
  ]);
  ctx.fillStyle = rgba(BEAM_CORE, lvl);
  for (const q of [ta, tb]) {
    ctx.beginPath();
    ctx.arc(q.x, q.y, 4.5 + 2 * p, 0, TAU);
    ctx.fill();
  }
  ctx.restore();
  for (const q of [ta, tb])
    glow(ctx, q.x, q.y, 50 + 30 * p, PALETTE.tealLight, (0.8 + 0.2 * p) * lvl);
}

// During the hold the scanner arms: two pen tips race around the grid's
// border from the front corner, trailing comet tails, and meet at the back
// corner on b2, where the beam ignites.
export const ARM0 = 0.66;
const LEG = 2 * EDGE;
const F_CORNER: V3 = [EDGE, 0, EDGE];
const B_CORNER: V3 = [-EDGE, 0, -EDGE];
const SIDES: V3[] = [
  [-EDGE, 0, EDGE],
  [EDGE, 0, -EDGE],
];
const armLen = (t: number): number => inQuad(progress(ARM0, T_BEAM0, t)) * 2 * LEG;
const armLead = (t: number): number => 1 - smoothstep(T_BEAM0 - 0.005, T_BEAM0 + 0.06, t);
/** A point `s` along one arming path: front corner, around a side corner, to the back. */
const armAt = (side: V3, s: number): V3 =>
  s <= LEG
    ? mix3(F_CORNER, side, s / LEG)
    : mix3(side, B_CORNER, Math.min(1, (s - LEG) / LEG));

/** 0..1 rim light on a cube from an arming tip running along the border behind it. */
function tipLight(c: Cell, t: number): number {
  const len = armLen(t);
  const lead = armLead(t);
  if (len <= LEG || lead <= 0) return 0;
  let best = 0;
  for (const side of SIDES) {
    const p = armAt(side, len);
    const d = Math.hypot(p[0] - c.x, p[2] - c.z);
    best = Math.max(best, smoothstep(1.9, 0.6, d));
  }
  return best * lead;
}

/** The tips' heads light the rooftops once they run behind the city. */
function drawTipGlows(ctx: CanvasRenderingContext2D, view: View, t: number): void {
  const len = armLen(t);
  const lead = armLead(t);
  if (len <= LEG || lead <= 0) return;
  const a = smoothstep(LEG, LEG + 1, len) * lead;
  for (const side of SIDES) {
    const p = view.project(armAt(side, len));
    glow(ctx, p.x, p.y, 60, PALETTE.tealLight, 0.55 * a);
  }
}

function drawScanFrame(ctx: CanvasRenderingContext2D, view: View, t: number): void {
  const len = armLen(t);
  const fade = 1 - smoothstep(T_BEAM0 + 0.05, T_BEAM1, t);
  if (len <= 0 || fade <= 0) return;
  const F = F_CORNER;
  const B = B_CORNER;
  const p = len / (2 * LEG);
  const lead = armLead(t);
  ctx.save();
  ctx.lineCap = "round";
  ctx.lineJoin = "round";
  for (const side of SIDES) {
    const at = (s: number): V3 => armAt(side, s);
    const path: V3[] = [F];
    if (len > LEG) path.push(side);
    path.push(at(len));
    const sp = path.map((q) => view.project(q));
    ctx.strokeStyle = rgba(PALETTE.tealLight, 0.55 * fade);
    ctx.lineWidth = 3;
    ctx.beginPath();
    ctx.moveTo(sp[0].x, sp[0].y);
    for (let i = 1; i < sp.length; i++) ctx.lineTo(sp[i].x, sp[i].y);
    ctx.stroke();
    if (lead <= 0) continue;
    // Comet tail: the last stretch of the trace, brighter and thicker
    // toward the tip.
    const tail = 2.6 * (0.4 + 0.6 * p);
    const N = 10;
    let prev = view.project(at(Math.max(0, len - tail)));
    for (let i = 1; i <= N; i++) {
      const q = view.project(at(Math.max(0, len - tail * (1 - i / N))));
      const k = i / N;
      ctx.strokeStyle = rgba(BEAM_CORE, 0.85 * k * k * lead);
      ctx.lineWidth = 2 + 4 * k;
      ctx.beginPath();
      ctx.moveTo(prev.x, prev.y);
      ctx.lineTo(q.x, q.y);
      ctx.stroke();
      prev = q;
    }
    ctx.fillStyle = rgba(BEAM_CORE, lead);
    ctx.beginPath();
    ctx.arc(prev.x, prev.y, 4.5, 0, TAU);
    ctx.fill();
    glow(ctx, prev.x, prev.y, 44, PALETTE.tealLight, 0.9 * lead);
  }
  ctx.restore();
  // Ignition flare where the tips meet, and a smaller one where the beam
  // leaves through the front corner.
  const b = view.project(B);
  const flare =
    t < T_BEAM0 ? smoothstep(T_BEAM0 - 0.04, T_BEAM0, t) : Math.exp(-(t - T_BEAM0) / 0.09);
  if (flare > 0.01) glow(ctx, b.x, b.y, 140, PALETTE.tealLight, 0.95 * flare);
  const f = view.project(F);
  const exit =
    t < T_BEAMEND ? smoothstep(T_BEAMEND - 0.03, T_BEAMEND, t) : Math.exp(-(t - T_BEAMEND) / 0.06);
  if (exit > 0.01) glow(ctx, f.x, f.y, 90, PALETTE.tealLight, 0.7 * exit);
}

/** How far the curtain's backlight darkens a cube's front faces. */
const BACKLIT = 0.22;

/** 0..1 how strongly the approaching curtain lights a standing cube. */
function beamLight(c: Cell, t: number): number {
  const lvl = beamLevel(t);
  if (lvl <= 0) return 0;
  const ahead = c.rank - beamPos(t);
  return lvl * smoothstep(3.2, 1, ahead) * smoothstep(-0.6, 1, ahead);
}

// Light from the curtain behind a standing cube: a rim light along the
// top's back edges, the only visible edges that face the beam.
function drawLit(ctx: CanvasRenderingContext2D, view: View, pose: BoxPose, a: number): void {
  if (a <= 0.01) return;
  const f = boxFrame(pose);
  const back = view.project(boxPoint(f, -1, 1, -1));
  const l = view.project(boxPoint(f, -1, 1, 1));
  const r = view.project(boxPoint(f, 1, 1, -1));
  ctx.save();
  ctx.globalCompositeOperation = "lighter";
  ctx.strokeStyle = rgba(RIM, 0.9 * a);
  ctx.lineWidth = 2.5;
  ctx.lineCap = "round";
  ctx.beginPath();
  ctx.moveTo(l.x, l.y);
  ctx.lineTo(back.x, back.y);
  ctx.lineTo(r.x, r.y);
  ctx.stroke();
  ctx.restore();
}

// Kept cubes: after the glow they turn a quarter, flatten to amber, and
// round off into discs, arriving with zero velocity on the bar line.
const SPIN0 = T_BEAM1 + 0.02;
const SPIN1 = 1.8;
const FLAT0 = 1.58;
const FLAT1 = 1.74;
const M0 = 1.6;
const M1 = T_DISC;

function rayHull(
  pts: readonly { x: number; y: number }[],
  cx: number,
  cy: number,
  ang: number,
): number {
  const dx = Math.cos(ang);
  const dy = Math.sin(ang);
  let best = Infinity;
  for (let i = 0; i < pts.length; i++) {
    const a = pts[i];
    const b = pts[(i + 1) % pts.length];
    const ex = b.x - a.x;
    const ey = b.y - a.y;
    const den = dx * ey - dy * ex;
    if (Math.abs(den) < 1e-9) continue;
    const ax = a.x - cx;
    const ay = a.y - cy;
    const s = (ax * ey - ay * ex) / den;
    const v = (ax * dy - ay * dx) / den;
    if (s > 0 && v >= -1e-6 && v <= 1 + 1e-6 && s < best) best = s;
  }
  return best === Infinity ? 0 : best;
}

/** 0..1 the kept state (green mark and reticle), from the beam's touch to b3. */
function keptTag(c: Cell, t: number): number {
  return smoothstep(c.hit - 0.002, c.hit + 0.01, t) * (1 - smoothstep(T_BEAM1, T_BEAM1 + 0.08, t));
}

function drawKept(
  ctx: CanvasRenderingContext2D,
  view: View,
  c: Cell,
  st: Standing,
  t: number,
): void {
  const scaleK = view.cam.scale / WORLD_CAM.scale;
  const center = view.project([c.x, st.lift + HALF * (st.pose.squash ?? 1), c.z]);
  const on = smoothstep(T_BEAM1 - 0.02, T_BEAM1 + 0.02, t);
  // The halo builds as the carpet drops away rather than flaring over it.
  const glowA = smoothstep(T_BEAM1, T_BEAM1 + 0.1, t) * (1 - smoothstep(1.56, 1.8, t));
  if (glowA > 0) glow(ctx, center.x, center.y, 160 * scaleK, PALETTE.amber, 0.6 * glowA);

  const spin = (Math.PI / 2) * swiftInOut(progress(SPIN0, SPIN1, t));
  const flat = smoothstep(FLAT0, FLAT1, t);
  const m = inOutCubic(progress(M0, M1, t));
  const bright = on * (0.1 + 0.22 * Math.exp(-Math.max(0, t - T_BEAM1) / 0.08)) * (1 - flat);
  const pose: BoxPose = {
    ...st.pose,
    yaw: spin,
    toneShift: (st.pose.toneShift ?? 0) + bright,
    outline: 1 - flat,
    tape: 0,
  };
  const hullPts = boxSilhouette(view, pose);
  if (m <= 0) {
    drawBox(ctx, view, pose);
    if (flat > 0) {
      polygon(ctx, hullPts);
      ctx.fillStyle = rgba(PALETTE.amber, flat);
      ctx.fill();
    }
  } else {
    // Resample the silhouette by angle and blend each ray toward the disc,
    // breathing in slightly on the way so the landing has some give.
    const R = CUBE * WORLD_CAM.scale * 0.72 * scaleK;
    const breathe = 1 - 0.07 * Math.sin(Math.PI * m);
    const N = 56;
    const pts: { x: number; y: number }[] = [];
    for (let i = 0; i < N; i++) {
      const ang = (i / N) * TAU;
      const r = lerp(rayHull(hullPts, center.x, center.y, ang), R, m) * breathe;
      pts.push({ x: center.x + Math.cos(ang) * r, y: center.y + Math.sin(ang) * r });
    }
    if (flat < 1) {
      ctx.save();
      polygon(ctx, pts);
      ctx.clip();
      drawBox(ctx, view, pose);
      ctx.restore();
    }
    polygon(ctx, pts);
    ctx.fillStyle = rgba(PALETTE.amber, flat);
    ctx.fill();
  }

  // Kept state: the top face marked green from the beam's touch until b3.
  const tag = keptTag(c, t);
  if (tag > 0.01) {
    const f = boxFrame(pose);
    const top = [
      boxPoint(f, -1, 1, -1),
      boxPoint(f, 1, 1, -1),
      boxPoint(f, 1, 1, 1),
      boxPoint(f, -1, 1, 1),
    ].map((p) => view.project(p));
    ctx.save();
    polygon(ctx, top);
    // A bright green hit on the sixteenth, settling to the held mark.
    const hit = Math.exp(-Math.max(0, t - c.hit) / 0.07);
    ctx.fillStyle = rgba(PALETTE.green, (0.42 + 0.45 * hit) * tag);
    ctx.fill();
    ctx.lineJoin = "round";
    ctx.strokeStyle = rgba(PALETTE.green, tag);
    ctx.lineWidth = 3.5 * scaleK;
    ctx.stroke();
    ctx.restore();
  }
}

// Scanner lock: corner brackets that snap onto a kept cube when the beam
// touches it, hold through the wind-up, and burst off on b3.
function drawReticle(
  ctx: CanvasRenderingContext2D,
  view: View,
  c: Cell,
  st: Standing,
  t: number,
): void {
  const a = keptTag(c, t);
  if (a <= 0.01) return;
  const [x0, y0, x1, y1] = bounds(boxSilhouette(view, st.pose));
  const cx = (x0 + x1) / 2;
  const cy = (y0 + y1) / 2;
  const rel = swiftOut(progress(T_BEAM1, T_BEAM1 + 0.08, t));
  const s = lerp(1.45, 1, swiftOut(progress(c.hit, c.hit + 0.1, t))) * lerp(1, 1.25, rel);
  const hw = ((x1 - x0) / 2 + 3) * s;
  const hh = ((y1 - y0) / 2 + 3) * s;
  const arm = Math.min(hw, hh) * 0.3;
  ctx.save();
  ctx.lineCap = "square";
  ctx.lineJoin = "miter";
  ctx.beginPath();
  for (const [sx, sy] of [
    [-1, -1],
    [1, -1],
    [1, 1],
    [-1, 1],
  ]) {
    const px = cx + sx * hw;
    const py = cy + sy * hh;
    ctx.moveTo(px - sx * arm, py);
    ctx.lineTo(px, py);
    ctx.lineTo(px, py - sy * arm);
  }
  ctx.strokeStyle = rgba(PALETTE.bg, 0.55 * a);
  ctx.lineWidth = 7;
  ctx.stroke();
  ctx.strokeStyle = rgba(PALETTE.green, a);
  ctx.lineWidth = 3.5;
  ctx.stroke();
  ctx.restore();
  // A glint on the tag, one per sixteenth. It only adds light and fades by
  // shrinking, so it never goes grey over the dark floor.
  const glint = t >= c.hit ? Math.exp(-(t - c.hit) / 0.1) : 0;
  if (glint > 0.03) {
    const sc = view.cam.scale / WORLD_CAM.scale;
    ctx.save();
    ctx.globalCompositeOperation = "lighter";
    const size = 22 * sc * glint;
    sparkle(ctx, cx + hw, cy - hh, size, a * Math.min(1, glint * 2), (t - c.hit) * 3, GLINT);
    ctx.restore();
  }
}

function drawFloorFx(ctx: CanvasRenderingContext2D, view: View, list: Cell[], t: number): void {
  // The stomp: a floor flash, and a ripple with a hot leading edge that
  // rides the landing front out to the rim.
  if (t >= T_STOMP) {
    const u = t - T_STOMP;
    ctx.save();
    floorMatrix(ctx, view, 0, 0);
    glow(ctx, 0, 0, 2.1, PALETTE.amber, 0.9 * Math.exp(-u / 0.07));
    ctx.globalCompositeOperation = "lighter";
    // A ring is a white-hot leading edge with light trailing inside it:
    // brightest just behind the edge, falling to nothing `band` units in.
    const ringAt = (R: number, a: number, band: number) => {
      const r0 = Math.max(0, R - band);
      const g = ctx.createRadialGradient(0, 0, r0, 0, 0, R);
      g.addColorStop(0, rgba(PALETTE.amber, 0));
      g.addColorStop(0.45, rgba(PALETTE.amber, 0.1 * a));
      g.addColorStop(0.88, rgba(PALETTE.amberBright, 0.45 * a));
      g.addColorStop(1, rgba(PALETTE.amberBright, 0.7 * a));
      ctx.fillStyle = g;
      ctx.beginPath();
      ctx.arc(0, 0, R, 0, TAU);
      ctx.arc(0, 0, r0, 0, TAU, true);
      ctx.fill();
      ctx.strokeStyle = rgba(SHOCK_EDGE, 0.95 * a);
      ctx.lineWidth = Math.min(0.05, 0.02 + band * 0.1);
      ctx.beginPath();
      ctx.arc(0, 0, R, 0, TAU);
      ctx.stroke();
    };
    // Shock: the band is widest while the ring is fast and thins as it
    // slows, so it visibly spends itself.
    const p = progress(0, SHOCK, u);
    if (p < 1) ringAt(shockRadius(u), (1 - p) ** 1.3, 0.06 + 0.34 * (1 - p) ** 2);
    // The landing front: a faint ring each touchdown rides on.
    if (t >= T_LAND0) {
      const R = frontRadius(t);
      const a =
        0.4 *
        smoothstep(T_LAND0, T_LAND0 + 0.06, t) *
        (1 - smoothstep(DLAST - 0.5, DLAST + 1.2, R));
      if (a > 0.01) ringAt(R, a, 0.3);
    }
    ctx.restore();
  }
  for (const c of list) {
    // Touchdown flash on the floor; the first ring lands as one accent.
    if (c.land > 0) {
      const u = t - c.land;
      if (u >= 0 && u < 0.45) {
        ctx.save();
        floorMatrix(ctx, view, c.x, c.z);
        const a = (c.first ? 0.9 : 0.5) * Math.exp(-u / 0.09);
        glow(ctx, 0, 0, c.first ? 1.3 : 1, PALETTE.amber, a);
        ctx.restore();
      }
    }
    if (!c.keep) continue;
    // Warm pool of light under the kept cubes as they glow.
    const g = smoothstep(T_BEAM1 + 0.02, T_BEAM1 + 0.1, t) * (1 - smoothstep(1.5, 1.72, t));
    if (g > 0) {
      ctx.save();
      floorMatrix(ctx, view, c.x, c.z);
      glow(ctx, 0, 0, 1.4, PALETTE.amber, 0.45 * g);
      ctx.restore();
    }
  }
}

let discs: { x: number; y: number; r: number }[] | null = null;
function drawDiscs(ctx: CanvasRenderingContext2D): void {
  discs ??= keptDiscs();
  ctx.fillStyle = PALETTE.amber;
  for (const d of discs) {
    ctx.beginPath();
    ctx.arc(d.x, d.y, d.r, 0, Math.PI * 2);
    ctx.fill();
  }
}

export const scene: Scene = {
  id: S.id,
  start: S.start,
  end: S.end,
  draw(ctx, lt, env) {
    const t = lt;
    ctx.fillStyle = PALETTE.bg;
    ctx.fillRect(0, 0, env.W, env.H);
    if (t <= 0) {
      drawStagedBox(ctx, WORLD_CAM, H5_POSE);
      return;
    }
    if (t >= T_DISC) {
      drawDiscs(ctx);
      return;
    }
    const view = new View(camera(t));
    const list = world();

    drawGrid(ctx, view, t);
    drawFloorFx(ctx, view, list, t);
    drawBeamFloor(ctx, view, t);
    drawScanFrame(ctx, view, t);

    const depth = (c: Cell) => c.x * view.v[0] + c.z * view.v[2];
    const sorted = [...list].sort((a, b) => depth(a) - depth(b));

    // The carpet: cartons already slapped flat lie on the floor under
    // everything that stands, with their dust over them.
    for (const c of sorted) if (isFlat(c, t)) drawFold(ctx, view, c, t);
    for (const c of list) if (!c.keep && t >= c.hit) drawDust(ctx, view, c, t);
    drawFloorFlash(ctx, view, t);

    // Contact shadows under everything that stands, falls, or is folding.
    const states = new Map<Cell, Standing>();
    for (const c of list) {
      if (!c.keep && t >= c.hit) {
        shadow(ctx, view, c.x, c.z, 0, 0.45 * (1 - smoothstep(0.01, SLAP, foldU(c, t))));
        continue;
      }
      const st = standing(c, t);
      if (!st) continue;
      states.set(c, st);
      let a = 0.45 * clamp(st.pose.size! / CUBE);
      if (c.keep) a *= 1 - smoothstep(1.45, 1.66, t);
      shadow(ctx, view, c.x, c.z, st.lift, a);
    }

    // Painter's order, split by the beam's plane so the light sheet and the
    // gantry posts sit between the rows it has crossed and the rows in front.
    const drawCell = (c: Cell) => {
      if (!c.keep && t >= c.hit) {
        if (!isFlat(c, t)) drawFold(ctx, view, c, t);
        return;
      }
      let st = states.get(c);
      if (!st) return;
      // Backlit by the approaching curtain: the faces toward the viewer go
      // dark under a bright rim, until the scanner's verdict.
      const lit = beamLight(c, t);
      if (lit > 0 && t < c.hit)
        st = { ...st, pose: { ...st.pose, toneShift: (st.pose.toneShift ?? 0) - BACKLIT * lit } };
      drawSmear(ctx, view, st);
      if (c.keep) drawKept(ctx, view, c, st, t);
      else drawBox(ctx, view, st.pose);
      drawLit(ctx, view, st.pose, Math.max(lit, tipLight(c, t)));
      if (st.spark > 0) {
        const p = view.project([c.x, st.lift + HALF, c.z]);
        glow(ctx, p.x, p.y, 42 * st.spark + 14, PALETTE.amberBright, 0.9 * st.spark);
      }
    };
    if (beamLevel(t) > 0) {
      const c0 = beamPos(t);
      for (const c of sorted) if (c.rank < c0) drawCell(c);
      drawBeamCurtain(ctx, view, t);
      for (const c of sorted) if (c.rank >= c0) drawCell(c);
      drawBeamBar(ctx, view, t);
    } else {
      for (const c of sorted) drawCell(c);
    }

    drawTipGlows(ctx, view, t);
    // Scanner reticles sit over the scene like a HUD.
    if (t > T_BEAM0 && t < T_BEAM1 + 0.12)
      for (const c of list) {
        const st = c.keep ? states.get(c) : undefined;
        if (st) drawReticle(ctx, view, c, st, t);
      }
  },
};
