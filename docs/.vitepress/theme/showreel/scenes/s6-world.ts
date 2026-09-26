// Scene 12, "target/ pruned": the store as a small world of cartons, each a
// checkout's target/ directory. The first cube stomps and the city rains in
// on the ripple while the caption says where target/ lives. Then the rules
// set up: a checkout's folder crumples, a carton wears a 30-day clock, and a
// second wave piles through a dashed disk-budget plane. A scan beam sweeps
// the city over three beats and each group goes flat as its rule lands:
// the orphan, the stale ones, then everything over budget, least recently
// used first. hk-fix, the most recent, is never taken. The survivors hop
// and flatten into the amber discs the liquid morph picks up.

import {
  BEAT,
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
  OUTLINE,
  OUTLINE_RATIO,
  sparkle,
  TAPE,
  TAPE_DROP,
  TAPE_EDGE,
  TAPE_HALF,
  TAPE_SIDE,
} from "../box";
import { mix, mixRGB, type RGB, rgb, rgba } from "../color";
import { glow, makeCanvas, roundedRect } from "../fx";
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
  spring,
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
  mixCamera,
  mix3,
  mul,
  polygon,
  type Projected,
  tone,
  type V3,
  View,
} from "../space";
import { drawText, drawWords, entrance, font, layout, MONO, WIPE, type WordStyle, wordStyle } from "../type";

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
/** Dust on a carton nobody has built in for 30 days. */
const DUST_GREY = mix(PALETTE.text3, PALETTE.bg, 0.35);

const S = sec("pruned");
/** Local time of beat `n` of the section. */
const b = (n: number): number => n * BEAT;

// Anchors, local seconds. The score (score/world.ts) is written to these.
export const T_STOMP = b(0.25); // the first cube stomps, the ripple leaves
export const T_LAND0 = b(0.5); // the first ring touches down
export const T_LAND1 = b(2); // the rim lands
/** The checkout's folder lifts out of its chip, crumples, and falls. */
export const T_LIFT = b(2.25);
/** It is crushed in the air from here to T_CRUMPLE, then drops. */
export const T_CRUSH = T_LIFT + 0.09;
export const T_CRUMPLE = b(2.5);
/** The 30-day clock pops onto a carton and its hand runs out the month. */
export const T_CLOCK = b(3);
export const T_DAYS0 = b(3.25);
export const T_DAYS = b(4);
/** The disk-budget plane draws on; the second wave piles through it. */
export const T_PLANE = b(3.75);
export const T_PILE0 = b(4);
export const T_PILE1 = b(4.75);
/** The scanner arms around the border and ignites at the back corner. */
export const ARM0 = b(4.375);
export const T_BEAM0 = b(5);
/** Each rule's line lands and its group slaps flat on the beat. */
export const T_RULES = [b(5.25), b(6), b(6.75)] as const;
export const T_BEAMEND = b(8);
/** The flattened carpet drops through the floor; the keepers glow. */
export const T_DROP = b(8);
/** The keepers hop, turn, and land as the discs. */
export const T_HOP = b(9.25);
export const T_DISC = b(11.5);

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

// The scan beam sweeps the diagonal x + z = c from behind the back corner
// on b5 to past the front corner on b8 at one speed, six rows a beat. It
// crosses the back row as the orphan, a row in, starts to fold and reaches
// it as it slaps flat on b5.25; it has crossed every row by b7.6, so the
// last carton is flat by b8.
const V_BEAM = 6 / BEAT;
const C_BEAM0 = -8 - V_BEAM * (T_RULES[0] - 0.07 - T_BEAM0);
const beamPos = (t: number): number => C_BEAM0 + V_BEAM * (t - T_BEAM0);
/** When the beam reaches a row: one unit early, as it meets the cubes' back corners. */
export const beamAt = (rank: number): number => T_BEAM0 + (rank - 1 - C_BEAM0) / V_BEAM;

// The checkouts named in every-checkout, each the chip over its carton:
// the two kept (hk, and hk-fix, the most recently used), the orphan whose
// checkout is deleted, the stale one, and two that go for the budget. The
// chip sits `dx`, `dy` px from the top of its carton.
interface Named {
  name: string;
  cell: readonly [number, number];
  dx: number;
  dy: number;
}
const NAMED: readonly Named[] = [
  { name: "hk", cell: [-2, 2], dx: -176, dy: 22 },
  { name: "hk-fix", cell: [3, 1], dx: 164, dy: -36 },
  { name: "hk-pr-812", cell: [-3, -3], dx: 92, dy: -78 },
  { name: "hk-old-spike", cell: [-4, 1], dx: -128, dy: -64 },
  { name: "hk-bisect", cell: [2, -4], dx: 100, dy: -66 },
  { name: "hk-try-2", cell: [0, 4], dx: -150, dy: 56 },
];
/** Rule 2's group: the stale carton wearing the clock, and the others nobody used either. */
const STALE: readonly (readonly [number, number])[] = [
  [-4, 1],
  [-3, -4],
  [0, -4],
  [-4, -1],
];
/**
 * Where the second wave lands, and how many cartons high it stacks each
 * cell: a mound of cartons that go for the budget, heaped round hk.
 */
const PILE: readonly (readonly [x: number, z: number, extra: number])[] = [
  [1, 0, 2],
  [0, 1, 2],
  [-1, 0, 2],
  [0, -1, 2],
  [1, 1, 2],
  [-1, -1, 1],
  [1, -1, 1],
  [-1, 1, 1],
  [2, 0, 1],
  [0, 2, 1],
  [-2, 0, 1],
  [0, -2, 1],
  [2, 1, 1],
  [1, 2, 1],
  [-1, -2, 1],
  [2, -1, 1],
  [-2, 1, 1],
];

/** Which rule prunes a carton: 0 kept, 1 its checkout is gone, 2 unused 30 days, 3 over budget. */
type Rule = 0 | 1 | 2 | 3;

/** A carton stacked on another by the second wave. */
interface Upper {
  spawn: number;
  land: number;
  drop: number;
  yaw: number;
  /** A little off the carton under it, so the pile reads as tossed. */
  ox: number;
  oz: number;
  ex: V3;
  ez: V3;
  tape: boolean;
}

interface Cell {
  x: number;
  z: number;
  /** Distance from the center cell; each ring of equal distance lands together. */
  d: number;
  rank: number;
  rule: Rule;
  keep: boolean;
  center: boolean;
  /** First ring: lands on b0.5 with the camera bump. */
  first: boolean;
  /** Pop-in and touchdown times, and the drop height between them. */
  spawn: number;
  land: number;
  drop: number;
  /** When the beam reaches it: a keeper is tagged, anything else condemned. */
  hit: number;
  /** When it starts to fold flat (Infinity for a keeper); it slaps SLAP later. */
  fold: number;
  /** A few degrees of yaw so the block reads as stacked crates, not tiles. */
  yaw: number;
  ex: V3;
  ez: V3;
  /** Fold direction, away from the viewer onto floor the beam has cleared. */
  dir: V3;
  tape: boolean;
  seed: number;
  name: Named | null;
  /** The second wave's cartons stacked on this one, bottom up. */
  ups: Upper[];
}

const key = (x: number, z: number) => `${x},${z}`;

let cells: Cell[] | null = null;
/** The city: every cell that gets a carton, the center cube included. */
export function world(): Cell[] {
  if (cells) return cells;
  const keep = new Set(KEEP.map(([x, z]) => key(x, z)));
  const named = new Map(NAMED.map((n) => [key(...n.cell), n]));
  const stale = new Set(STALE.map(([x, z]) => key(x, z)));
  const pile = new Map(PILE.map(([x, z, n], i) => [key(x, z), [i, n] as const]));
  const out: Cell[] = [];
  for (let x = -GRID_R; x <= GRID_R; x++) {
    for (let z = -GRID_R; z <= GRID_R; z++) {
      const k = keep.has(key(x, z));
      const name = named.get(key(x, z)) ?? null;
      const seed = (x + 16) * 64 + z + 16;
      const d = Math.hypot(x, z);
      const center = x === 0 && z === 0;
      // A few holes, mostly toward the rim, so the block reads as a city
      // rather than a slab; the four corners stay open to soften the diamond.
      const corner = Math.abs(x) === GRID_R && Math.abs(z) === GRID_R;
      const forced = k || center || name || stale.has(key(x, z)) || pile.has(key(x, z));
      if (!forced && (corner || hash(seed, 3) < 0.07 + 0.3 * smoothstep(3.6, 5.2, d))) continue;
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
      const rank = x + z;
      const rule: Rule = k ? 0 : name?.name === "hk-pr-812" ? 1 : stale.has(key(x, z)) ? 2 : 3;
      const hit = beamAt(rank);
      const ups: Upper[] = [];
      for (let k = 0; k < (pile.get(key(x, z))?.[1] ?? 0); k++) {
        const s = seed * 4 + k;
        const uyaw = (hash(s, 73) - 0.5) * 18 * DEG;
        ups.push({
          spawn: 0,
          land: 0,
          drop: 2.6 + 0.8 * hash(s, 75),
          yaw: uyaw,
          ox: (hash(s, 77) - 0.5) * 0.12,
          oz: (hash(s, 79) - 0.5) * 0.12,
          ex: [Math.cos(yaw + uyaw), 0, -Math.sin(yaw + uyaw)],
          ez: [Math.sin(yaw + uyaw), 0, Math.cos(yaw + uyaw)],
          tape: hash(s, 81) < 0.4,
        });
      }
      const cell: Cell = {
        x,
        z,
        d,
        rank,
        rule,
        keep: k,
        center,
        first: !center && d === 1,
        spawn: center ? -1 : Math.max(T_POP0, land - Math.sqrt((2 * drop) / G)),
        land,
        drop,
        hit,
        fold: Infinity,
        yaw,
        ex,
        ez,
        dir: mul(alongX ? ex : ez, -1),
        tape: !center && !k && hash(seed, 5) < 0.34,
        seed,
        name,
        ups,
      };
      out.push(cell);
    }
  }
  // The second wave: every second layer's carton lands in a shuffled
  // order from b4, then the third layer's from half a beat later, all down
  // by b4.75.
  for (const layer of [0, 1]) {
    const wave = out
      .filter((c) => c.ups.length > layer)
      .sort((a, bb) => hash(a.seed, 71 + layer) - hash(bb.seed, 71 + layer));
    const [t0, t1] = layer === 0 ? [T_PILE0, T_PILE0 + b(0.5)] : [T_PILE0 + b(0.3), T_PILE1 - 0.02];
    wave.forEach((c, i) => {
      const u = c.ups[layer];
      u.land = lerp(t0, t1, i / Math.max(1, wave.length - 1));
      // Never before the carton it lands on.
      if (layer > 0) u.land = Math.max(u.land, c.ups[layer - 1].land + 0.1);
      u.spawn = u.land - Math.sqrt((2 * u.drop) / G);
    });
  }
  // Folds. The orphan and the stale ones go flat as their rules land; over
  // budget, everything the beam has already scanned falls in one ripple
  // from the back corner as its rule lands, and the rest as the beam
  // reaches them.
  const cut = beamPos(T_RULES[2] - SLAP);
  const scanned = out.filter((c) => c.rule === 3 && c.hit < T_RULES[2] - SLAP);
  const r0 = Math.min(...scanned.map((c) => c.rank));
  for (const c of out) {
    if (c.rule === 1) c.fold = T_RULES[0] - SLAP;
    else if (c.rule === 2) c.fold = T_RULES[1] - SLAP;
    else if (c.rule === 3) {
      c.fold =
        c.hit < T_RULES[2] - SLAP
          ? T_RULES[2] - SLAP + RIPPLE * progress(r0, cut, c.rank) + 0.012 * hash(c.seed, 43)
          : c.hit;
    }
  }
  cells = out;
  return out;
}

/** The over-budget ripple runs this long from the back corner to the beam. */
const RIPPLE = 0.13;

/** A small vertical camera kick (px) on the stomp and on the first touchdown. */
function bump(t: number): number {
  let y = 0;
  for (const [at, amp] of [
    [T_STOMP, 2.5],
    [T_LAND0, 4],
    [T_RULES[2], 3],
  ]) {
    const u = t - at;
    if (u >= 0 && u < 0.3) y += amp * Math.exp(-u / 0.045) * Math.cos(TAU * 8 * u);
  }
  return y;
}

// The camera. It starts on the cube the chart dropped, cranes up and back
// while the city rains in, so the whole pile sits above the captions' band,
// and drifts round a few degrees while the rules play out. It settles back
// onto WORLD_CAM while the keepers hop, before the discs land.
const CITY: Camera = {
  ...WORLD_CAM,
  cy: 470,
  scale: 72,
  yaw: WORLD_CAM.yaw - 7 * DEG,
  pitch: WORLD_CAM.pitch - 3 * DEG,
};
const toCity = keys([
  [0, 0],
  [b(0.35), 0],
  [b(1.9), 1, inOutSine],
  [b(8.5), 1],
  [b(10.75), 0, inOutCubic],
]);
/** A slow drift across the hold, so the city never parks. */
const drift = keys([
  [b(1.9), 0],
  [b(8.5), 1, inOutSine],
]);
function camera(t: number): Camera {
  const k = toCity(t);
  const bmp = bump(t);
  if (k <= 0 && bmp === 0) return WORLD_CAM;
  const cam = mixCamera(WORLD_CAM, CITY, k);
  return { ...cam, yaw: cam.yaw + 4 * DEG * drift(t) * k, cy: cam.cy + bmp };
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

/** b8: the floor flashes as the carpet drops through it. */
const floorFlash = (t: number): number => (t >= T_DROP ? Math.exp(-(t - T_DROP) / 0.08) : 0);

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
const GRID_OUT = [T_STOMP, T_STOMP + 0.55] as const;
const GRID_BACK = [T_DROP - 0.02, T_DROP + b(1.5)] as const;
function drawGrid(ctx: CanvasRenderingContext2D, view: View, t: number): void {
  const out = 7 * swiftOut(progress(GRID_OUT[0], GRID_OUT[1], t));
  const back = 7 * (1 - inOutCubic(progress(GRID_BACK[0], GRID_BACK[1], t)));
  const R = Math.min(out, back);
  const fade = 1 - smoothstep(T_DROP + 0.05, GRID_BACK[1], t);
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
      const bb = -EDGE + j;
      const s = clamp((R - Math.hypot(a, bb)) * 1.5);
      if (s <= 0) continue;
      const p = view.project([a, 0, bb]);
      const r = 2.2 * s;
      ctx.fillRect(p.x - r, p.y - r * 0.5, r * 2, r);
    }
  }
  ctx.restore();

  const tipA = (1 - progress(GRID_OUT[0] + 0.3, GRID_OUT[1], t)) * fade;
  if (tipA > 0 && t < GRID_BACK[0]) {
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
/** Kept cubes hop on T_HOP and land as discs on T_DISC. */
const HOP = 1.25;
const hopSquash = keys([
  [T_HOP, 0.78],
  [T_HOP + 0.05, 1.16, outQuad],
  [T_HOP + 0.62, 1, inOutSine],
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
  // The carton a second-wave carton lands on takes the blow.
  if (c.ups.length) squash *= blow(c.ups[0], t);
  if (c.keep) {
    if (t >= c.hit && t < T_DROP) {
      // Tagged by the beam: a quick perk upward.
      const u = t - c.hit;
      squash *= 1 + 0.14 * Math.exp(-9 * u) * Math.sin(TAU * 3.6 * u);
    }
    if (t >= T_DROP && t < T_HOP) {
      // A bounce as the carpet drops away, then the wind-up for the hop.
      const u = t - T_DROP;
      squash *= 1 + 0.1 * Math.exp(-8 * u) * Math.sin(TAU * 3 * u);
      squash *= lerp(1, 0.78, inOutSine(progress(T_HOP - 0.16, T_HOP, t)));
    } else if (t >= T_HOP) {
      const s = progress(T_HOP, T_DISC, t);
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
      toneShift: flare,
    }),
    lift,
    speed,
    spark,
  };
}

/** A carton's squash as the one above lands on it. */
const blow = (above: Upper, t: number): number =>
  t < above.land ? 1 : 1 - 0.16 * Math.exp(-11 * (t - above.land)) * Math.cos(TAU * 5 * (t - above.land));

/**
 * Level `k` of the second wave on cell `c` (0 is the carton on the base),
 * standing on `lower` or still falling onto it.
 */
function standingUp(c: Cell, k: number, lower: Standing, t: number): Standing | null {
  const u0 = c.ups[k];
  if (!u0 || t < u0.spawn) return null;
  const seat = lower.lift + CUBE * (lower.pose.squash ?? 1);
  let lift = 0;
  let squash = 1;
  let speed = 0;
  let spark = 0;
  let size = CUBE;
  let flare = 0;
  let tiltX = 0;
  let tiltZ = 0;
  if (t < u0.land) {
    const u = t - u0.spawn;
    const fall = u / (u0.land - u0.spawn);
    lift = Math.max(0, u0.drop - 0.5 * G * u * u);
    speed = G * u;
    squash = 1 + 0.3 * fall * fall;
    size = CUBE * Math.max(0.02, popScale(progress(0, POP, u)));
    spark = 1 - progress(0, 0.09, u);
    flare = 0.3 * (1 - smoothstep(0, 0.08, u));
    const w = (1 - fall) * (1 - fall);
    tiltX = (hash(c.seed * 4 + k, 57) - 0.5) * 0.6 * w;
    tiltZ = (hash(c.seed * 4 + k, 58) - 0.5) * 0.6 * w;
  } else {
    const u = t - u0.land;
    squash = 1 - 0.3 * Math.exp(-9 * u) * Math.cos(TAU * 4.2 * u);
    flare = 0.25 * Math.exp(-u / 0.06);
  }
  const next = c.ups[k + 1];
  if (next) squash *= blow(next, t);
  return {
    pose: cellPose(c.x, c.z, {
      pos: [c.x + u0.ox, seat + lift, c.z + u0.oz],
      size,
      squash,
      yaw: c.yaw + u0.yaw,
      tiltX,
      tiltZ,
      tape: u0.tape ? 1 : 0,
      toneShift: flare,
    }),
    lift: seat + lift,
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

// A pruned carton gets knocked flat: it darkens, rocks toward the viewer,
// falls back onto cleared floor, and slaps down with a bounce. A stack goes
// over as one column. The flattened cartons lie as a carpet until b8, when
// the whole carpet drops through the floor at once.
const ROCK = 0.025;
/** A carton slaps flat this long after it starts to fold. */
export const SLAP = 0.07;
const SETTLE = 0.1;
function foldAngle(u: number): number {
  if (u < ROCK) return -0.12 * outQuad(u / ROCK);
  if (u < SLAP) return lerp(-0.12, Math.PI / 2, inQuad((u - ROCK) / (SLAP - ROCK)));
  return Math.PI / 2 - 0.1 * Math.sin(Math.PI * progress(SLAP, SETTLE, u));
}
/** When carton `c` slaps flat, or Infinity for a keeper. */
export const slapAt = (c: Cell): number => c.fold + SLAP;
const foldU = (c: Cell, t: number): number => t - c.fold;
const folding = (c: Cell, t: number): boolean => !c.keep && t >= c.fold;
const isFlat = (c: Cell, t: number): boolean => !c.keep && foldU(c, t) >= SLAP;

// The b8 drop: every flat carton slides straight down into its own
// footprint, as if through a trapdoor, and is gone in about five frames.
const SINK_DEPTH = 0.75;
const sinkDur = (c: Cell): number => 0.055 + 0.025 * hash(c.seed, 41);
const sinkP = (c: Cell, t: number): number => progress(T_DROP, T_DROP + sinkDur(c), t);

/** 0..1 how dusty a stale carton has gone as the month runs out. */
const dustAt = (c: Cell, t: number): number => (c.rule === 2 ? smoothstep(T_DAYS0 + 0.1, T_DAYS, t) : 0);

function drawFold(ctx: CanvasRenderingContext2D, view: View, c: Cell, t: number): void {
  const u = foldU(c, t);
  const sink = inQuad(sinkP(c, t));
  if (sink >= 1) return;
  const phi = foldAngle(u);
  const h = HALF;
  const d = c.dir;
  const sp = Math.sin(phi);
  const cp = Math.cos(phi);
  // Vertical edges lean along d; the tops stay level (a shear, like a
  // knocked-down box). Each level of a stack rides the one under it.
  const w: V3 = [d[0] * sp * 2 * h, cp * 2 * h, d[2] * sp * 2 * h];
  const levels: { ex: V3; ez: V3; ox: number; oz: number; tape: boolean }[] = [
    { ex: c.ex, ez: c.ez, ox: 0, oz: 0, tape: c.tape },
  ];
  for (const up of c.ups) if (t >= up.land) levels.push({ ex: up.ex, ez: up.ez, ox: up.ox, oz: up.oz, tape: up.tape });
  // A wall's normal tilts with the shear when it faces along d.
  const wallN = (e: V3): V3 => {
    const s = e[0] * d[0] + e[2] * d[2];
    return [e[0] - s * d[0] * (1 - cp), -s * sp, e[2] - s * d[2] * (1 - cp)];
  };
  const condemned = -lerp(BACKLIT, 0.36, smoothstep(0, 0.02, u));
  const flat = smoothstep(0.03, SLAP + 0.01, u);
  const dim = condemned * (1 - 0.5 * flat);
  const lw = LW * view.cam.scale;
  const corners: Projected[] = [];
  const boxes = levels.map((L, k) => {
    const P = (lx: number, lz: number, ly: number): V3 => [
      c.x + L.ox + (L.ex[0] * lx + L.ez[0] * lz) * h + w[0] * (k + ly),
      w[1] * (k + ly),
      c.z + L.oz + (L.ex[2] * lx + L.ez[2] * lz) * h + w[2] * (k + ly),
    ];
    for (const a of [-1, 1]) for (const bb of [-1, 1]) corners.push(view.project(P(a, bb, 0)), view.project(P(a, bb, 1)));
    const nx = mul(L.ex, -1);
    const nz = mul(L.ez, -1);
    return {
      P,
      L,
      mid: P(0, 0, 0.5),
      faces: [
        { id: "top", q: [P(-1, -1, 1), P(1, -1, 1), P(1, 1, 1), P(-1, 1, 1)], n: [0, 1, 0] as V3 },
        { id: "x+", q: [P(1, -1, 0), P(1, 1, 0), P(1, 1, 1), P(1, -1, 1)], n: wallN(L.ex) },
        { id: "x-", q: [P(-1, -1, 0), P(-1, 1, 0), P(-1, 1, 1), P(-1, -1, 1)], n: wallN(nx) },
        { id: "z+", q: [P(-1, 1, 0), P(1, 1, 0), P(1, 1, 1), P(-1, 1, 1)], n: wallN(L.ez) },
        { id: "z-", q: [P(-1, -1, 0), P(1, -1, 0), P(1, -1, 1), P(-1, -1, 1)], n: wallN(nz) },
      ],
    };
  });
  const outline = hull(corners);

  const paint = () => {
    ctx.lineJoin = "round";
    ctx.strokeStyle = OUTLINE;
    ctx.lineWidth = lw;
    for (const bx of boxes) {
      const shown = new Set<string>();
      for (const f of bx.faces) {
        if (!view.facing(f.n, bx.mid)) continue;
        shown.add(f.id);
        const pts = f.q.map((q) => view.project(q));
        polygon(ctx, pts);
        ctx.fillStyle = cardboardFill(ctx, pts, tone(view, f.n) + dim);
        ctx.fill();
        ctx.stroke();
      }
      if (bx.L.tape) {
        // Tape across the top, and its end down the +x panel, as drawBox lays it.
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
        const hw = TAPE_HALF;
        const P = bx.P;
        if (shown.has("top")) strip([P(-1, -hw, 1), P(1, -hw, 1), P(1, hw, 1), P(-1, hw, 1)], TAPE);
        const drop = 1 - TAPE_DROP;
        if (shown.has("x+"))
          strip([P(1, hw, 1), P(1, -hw, 1), P(1, -hw, drop), P(1, hw, drop)], TAPE_SIDE);
      }
    }
    const dust = dustAt(c, t);
    if (dust > 0) {
      polygon(ctx, outline);
      ctx.fillStyle = rgba(DUST_GREY, 0.5 * dust);
      ctx.fill();
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
// and under everything standing, and gone by the b8 drop.
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
  const u = (t - slapAt(c)) / DUST;
  if (u <= 0 || u >= 1) return;
  const a = 0.34 * (1 - u) ** 1.5 * (1 - smoothstep(T_DROP - 0.03, T_DROP + 0.02, t));
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
  const reach = 1 + 0.6 * c.ups.length;
  ctx.save();
  floorMatrix(ctx, view, c.x, c.z);
  ctx.globalAlpha *= a;
  for (let i = 0; i < PUFFS.length; i++) {
    const [al, ac, da, dc] = PUFFS[i];
    const s = c.seed * 8 + i;
    // Floor-local axes: along d, and across it.
    const ox = (d[0] * al * reach - d[2] * ac) * HALF;
    const oz = (d[2] * al * reach + d[0] * ac) * HALF;
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
    (1 - smoothstep(T_BEAMEND - 0.03, T_BEAMEND + 0.02, t))
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
  const bb = view.project([c / 2 - 1.1, 0, c / 2 - 1.1]);
  const g = ctx.createLinearGradient(a.x, a.y, bb.x, bb.y);
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

// The beam is a gantry: a laser bar above the pile, carried on two posts
// that ride the floor rails, with a faint light sheet between the bar and
// the floor line. The bar sits above every roof, so it is drawn over the
// whole city; the posts and sheet stand in the rows' painter order.
const BAR_Y = 3 * CUBE + 0.16;

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

/** 1 as each rule lands, gone in about two frames: the beam flares with it. */
function beamPulse(t: number): number {
  let p = 0;
  for (const at of T_RULES) {
    const u = t - at;
    if (u >= 0 && u < 0.2) p = Math.max(p, Math.exp(-u / 0.03));
  }
  return p;
}

function strokeLine(
  ctx: CanvasRenderingContext2D,
  a: { x: number; y: number },
  bb: { x: number; y: number },
  passes: readonly (readonly [width: number, alpha: number, color: string | RGB])[],
): void {
  for (const [wd, al, col] of passes) {
    if (al <= 0) continue;
    ctx.strokeStyle = rgba(col, al);
    ctx.lineWidth = wd;
    ctx.beginPath();
    ctx.moveTo(a.x, a.y);
    ctx.lineTo(bb.x, bb.y);
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

// Before the sweep the scanner arms: two pen tips race around the grid's
// border from the front corner, trailing comet tails, and meet at the back
// corner on b5, where the beam ignites.
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
  const fade = 1 - smoothstep(T_BEAM0 + 0.05, T_BEAMEND, t);
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
  const bp = view.project(B);
  const flare =
    t < T_BEAM0 ? smoothstep(T_BEAM0 - 0.04, T_BEAM0, t) : Math.exp(-(t - T_BEAM0) / 0.09);
  if (flare > 0.01) glow(ctx, bp.x, bp.y, 140, PALETTE.tealLight, 0.95 * flare);
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

/** 0..1 a scanned carton waiting for its rule: dark, with an amber blink on the touch. */
function condemnedAt(c: Cell, t: number): number {
  if (c.keep || t < c.hit) return 0;
  return smoothstep(c.hit, c.hit + 0.04, t);
}

// Light from the curtain behind a standing cube: a rim light along the
// top's back edges, the only visible edges that face the beam.
function drawLit(ctx: CanvasRenderingContext2D, view: View, pose: BoxPose, a: number, color: RGB = RIM): void {
  if (a <= 0.01) return;
  const f = boxFrame(pose);
  const back = view.project(boxPoint(f, -1, 1, -1));
  const l = view.project(boxPoint(f, -1, 1, 1));
  const r = view.project(boxPoint(f, 1, 1, -1));
  ctx.save();
  ctx.globalCompositeOperation = "lighter";
  ctx.strokeStyle = rgba(color, 0.9 * a);
  ctx.lineWidth = 2.5;
  ctx.lineCap = "round";
  ctx.beginPath();
  ctx.moveTo(l.x, l.y);
  ctx.lineTo(back.x, back.y);
  ctx.lineTo(r.x, r.y);
  ctx.stroke();
  ctx.restore();
}

/** A translucent coat over a pose's silhouette: dust, or the dark of a condemned carton. */
function coat(ctx: CanvasRenderingContext2D, view: View, pose: BoxPose, color: string, a: number): void {
  if (a <= 0.005) return;
  polygon(ctx, boxSilhouette(view, pose));
  ctx.fillStyle = rgba(color, a);
  ctx.fill();
}

// Kept cubes: after the glow they hop, turn a quarter, flatten to amber, and
// round off into discs, arriving with zero velocity on T_DISC.
export const SPIN0 = T_HOP + 0.02;
export const SPIN1 = T_HOP + 0.62;
export const FLAT0 = T_DISC - 0.34;
export const FLAT1 = T_DISC - 0.16;
const M0 = T_DISC - 0.3;
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
    const bb = pts[(i + 1) % pts.length];
    const ex = bb.x - a.x;
    const ey = bb.y - a.y;
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

/** 0..1 the kept state (green mark and reticle), from the beam's touch to the drop. */
function keptTag(c: Cell, t: number): number {
  return smoothstep(c.hit - 0.002, c.hit + 0.01, t) * (1 - smoothstep(T_DROP, T_DROP + 0.08, t));
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
  const on = smoothstep(T_DROP - 0.02, T_DROP + 0.02, t);
  // The halo builds as the carpet drops away rather than flaring over it.
  const glowA = smoothstep(T_DROP, T_DROP + 0.1, t) * (1 - smoothstep(FLAT0 - 0.1, FLAT1, t));
  if (glowA > 0) glow(ctx, center.x, center.y, 160 * scaleK, PALETTE.amber, 0.6 * glowA);

  const spin = (Math.PI / 2) * swiftInOut(progress(SPIN0, SPIN1, t));
  const flat = smoothstep(FLAT0, FLAT1, t);
  const m = inOutCubic(progress(M0, M1, t));
  const bright = on * (0.1 + 0.22 * Math.exp(-Math.max(0, t - T_DROP) / 0.08)) * (1 - flat);
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

  // Kept state: the top face marked green from the beam's touch until b8.
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
    // A bright green hit on the touch, settling to the held mark.
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
// touches it, hold, and burst off as the carpet drops.
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
  const rel = swiftOut(progress(T_DROP, T_DROP + 0.08, t));
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
  // A glint on the tag. It only adds light and fades by shrinking, so it
  // never goes grey over the dark floor.
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
    const g = smoothstep(T_DROP + 0.02, T_DROP + 0.1, t) * (1 - smoothstep(FLAT0 - 0.2, FLAT0, t));
    if (g > 0) {
      ctx.save();
      floorMatrix(ctx, view, c.x, c.z);
      glow(ctx, 0, 0, 1.4, PALETTE.amber, 0.45 * g);
      ctx.restore();
    }
  }
}

// The disk budget: a dashed box over the grid, its lid at a height the first
// layer sits under and the second wave's mound pierces. Its posts rise from
// the grid's corners, the lid traces round, and it turns amber while the
// pile is over it; it calms as the pruning brings the pile back under, and
// fades with the grid.
const PLANE_Y = 1.18;
/** 0..1 the pile is over budget: from the first carton landing through the lid to the last one folding. */
function overBudget(list: Cell[], t: number): number {
  let first = Infinity;
  let last = -Infinity;
  for (const c of list) {
    if (!c.ups.length) continue;
    first = Math.min(first, c.ups[0].land - 0.03);
    last = Math.max(last, slapAt(c));
  }
  return smoothstep(first, first + 0.04, t) * (1 - smoothstep(last - 0.02, last + 0.1, t));
}
const planeFade = (t: number): number => 1 - smoothstep(T_DROP + 0.1, T_DROP + b(1.25), t);
/** The posts rise, then the lid traces round from the left corner both ways. */
const postsUp = (t: number): number => swiftOut(progress(T_PLANE, T_PLANE + 0.13, t));
const lidDrawn = (t: number): number => swiftOut(progress(T_PLANE + 0.07, T_PLANE + 0.26, t));

/** The box's corners at height y: left, back, right, front. */
function boxRing(view: View, y: number): Projected[] {
  return [
    view.project([-EDGE, y, EDGE]),
    view.project([-EDGE, y, -EDGE]),
    view.project([EDGE, y, -EDGE]),
    view.project([EDGE, y, EDGE]),
  ];
}

/** The part of a pose above the lid, as a screen polygon, or null. */
function abovePlane(view: View, pose: BoxPose): Projected[] | null {
  const f = boxFrame(pose);
  const pts: Projected[] = [];
  const lo = f.c[1] - f.y[1];
  const hi = f.c[1] + f.y[1];
  if (hi <= PLANE_Y) return null;
  const from = clamp((PLANE_Y - f.c[1]) / f.y[1], -1, 1);
  for (const x of [-1, 1]) for (const z of [-1, 1]) for (const y of [lo >= PLANE_Y ? -1 : from, 1]) pts.push(view.project(boxPoint(f, x, y, z)));
  return hull(pts);
}

function budgetStroke(ctx: CanvasRenderingContext2D, color: string, t: number): void {
  ctx.setLineDash([16, 11]);
  ctx.lineDashOffset = -t * 40;
  ctx.lineWidth = 3;
  ctx.lineCap = "round";
  ctx.strokeStyle = color;
}

function budgetColor(list: Cell[], t: number): { over: number; flash: number; edge: string } {
  const over = overBudget(list, t);
  const flash = over * Math.exp(-Math.max(0, t - T_PILE0) / 0.14);
  return { over, flash, edge: rgba(mix(PALETTE.paper, PALETTE.amberBright, over), (0.8 + 0.2 * flash) * planeFade(t)) };
}

/** The back post, behind the whole city. */
function drawBudgetBack(ctx: CanvasRenderingContext2D, view: View, list: Cell[], t: number): void {
  const k = postsUp(t);
  if (k <= 0 || planeFade(t) <= 0) return;
  const a = view.project([-EDGE, 0, -EDGE]);
  const top = view.project([-EDGE, PLANE_Y * k, -EDGE]);
  ctx.save();
  budgetStroke(ctx, budgetColor(list, t).edge, t);
  ctx.beginPath();
  ctx.moveTo(a.x, a.y);
  ctx.lineTo(top.x, top.y);
  ctx.stroke();
  ctx.restore();
}

/** The lid, its tint, the other three posts, and the label, over the city. */
function drawBudget(ctx: CanvasRenderingContext2D, view: View, list: Cell[], ups: Map<Cell, Standing[]>, t: number): void {
  const k = postsUp(t);
  const fade = planeFade(t);
  if (k <= 0 || fade <= 0) return;
  const { over, flash, edge } = budgetColor(list, t);
  const floor = boxRing(view, 0);
  const lid = boxRing(view, PLANE_Y);
  ctx.save();
  // Everything the mound pushes up through the lid stays in front of it.
  ctx.beginPath();
  ctx.rect(-10, -10, 1940, 1100);
  for (const stack of ups.values()) {
    for (const st of stack) {
      const top = abovePlane(view, st.pose);
      if (!top) continue;
      ctx.moveTo(top[0].x, top[0].y);
      for (let i = 1; i < top.length; i++) ctx.lineTo(top[i].x, top[i].y);
      ctx.closePath();
    }
  }
  ctx.clip("evenodd");
  const drawn = lidDrawn(t);
  if (drawn > 0) {
    // The lid's glass, amber while the pile is over it.
    polygon(ctx, lid);
    ctx.fillStyle = rgba(mix(PALETTE.tealLight, PALETTE.amber, over), (0.05 + 0.06 * over + 0.22 * flash) * fade * smoothstep(0.5, 1, drawn));
    ctx.fill();
    // Its border, drawing on both ways from the left corner.
    budgetStroke(ctx, edge, t);
    ctx.beginPath();
    for (const path of [
      [lid[0], lid[1], lid[2]],
      [lid[0], lid[3], lid[2]],
    ]) {
      const lens = [Math.hypot(path[1].x - path[0].x, path[1].y - path[0].y), Math.hypot(path[2].x - path[1].x, path[2].y - path[1].y)];
      const shown = drawn * (lens[0] + lens[1]);
      ctx.moveTo(path[0].x, path[0].y);
      if (shown <= lens[0]) ctx.lineTo(lerp(path[0].x, path[1].x, shown / lens[0]), lerp(path[0].y, path[1].y, shown / lens[0]));
      else {
        ctx.lineTo(path[1].x, path[1].y);
        const q = (shown - lens[0]) / lens[1];
        ctx.lineTo(lerp(path[1].x, path[2].x, q), lerp(path[1].y, path[2].y, q));
      }
    }
    ctx.stroke();
  }
  // The near posts.
  budgetStroke(ctx, edge, t);
  ctx.beginPath();
  for (const i of [0, 2, 3]) {
    ctx.moveTo(floor[i].x, floor[i].y);
    ctx.lineTo(lerp(floor[i].x, lid[i].x, k), lerp(floor[i].y, lid[i].y, k));
  }
  ctx.stroke();
  ctx.restore();
  // The label, off the left post.
  const lab = popIn(t, T_PLANE + b(0.25)) * fade;
  if (lab > 0.01) {
    const p = floor[0];
    const q = lid[0];
    ctx.save();
    ctx.globalAlpha *= clamp(lab);
    drawText(ctx, "disk budget", p.x - 24, lerp(p.y, q.y, 0.5) + 14, {
      font: font(40, 600),
      align: "right",
      fill: mix(PALETTE.paper, PALETTE.amberBright, over),
    });
    ctx.restore();
  }
}

/** Where a stacked carton crosses the plane: a hot line round its near walls. */
function drawCrossing(ctx: CanvasRenderingContext2D, view: View, st: Standing, a: number): void {
  if (a <= 0.01) return;
  const f = boxFrame(st.pose);
  const lo = f.c[1] - f.y[1];
  const hi = f.c[1] + f.y[1];
  if (lo >= PLANE_Y || hi <= PLANE_Y) return;
  const y = (PLANE_Y - f.c[1]) / f.y[1];
  const ring = [
    boxPoint(f, -1, y, 1),
    boxPoint(f, 1, y, 1),
    boxPoint(f, 1, y, -1),
    boxPoint(f, -1, y, -1),
  ].map((p) => view.project(p));
  // Only the corner nearest the viewer and its two edges show.
  let near = 0;
  for (let i = 1; i < 4; i++) if (ring[i].y > ring[near].y) near = i;
  const pa = ring[(near + 3) % 4];
  const pb = ring[near];
  const pc = ring[(near + 1) % 4];
  ctx.save();
  ctx.lineCap = "round";
  ctx.lineJoin = "round";
  for (const [w, al, col] of [
    [9, 0.25, PALETTE.amber],
    [3, 0.95, PALETTE.amberBright],
  ] as const) {
    ctx.strokeStyle = rgba(col, al * a);
    ctx.lineWidth = w;
    ctx.beginPath();
    ctx.moveTo(pa.x, pa.y);
    ctx.lineTo(pb.x, pb.y);
    ctx.lineTo(pc.x, pc.y);
    ctx.stroke();
  }
  ctx.restore();
}

// Name chips: each named carton's checkout, a folder and its name over the
// carton on a pin. They pop in as their cartons land. A condemned one drops
// with its carton; the kept ones turn green when the beam tags them.
const CHIP = { size: 40, h: 56, pad: 18, icon: 34, gap: 12 } as const;
const CHIP_FONT = font(CHIP.size, 500, MONO);

/** A manila folder. */
const FOLDER = "#d8b36a";

/** A folder: the checkout. Drawn centered on (0, 0), `w` px wide. */
function folderGlyph(ctx: CanvasRenderingContext2D, w: number, color = FOLDER): void {
  const h = w * 0.78;
  const tab = w * 0.42;
  ctx.fillStyle = mix(color, PALETTE.ink, 0.25);
  roundedRect(ctx, -w / 2, -h / 2, tab, h * 0.3, w * 0.07);
  ctx.fill();
  ctx.fillStyle = color;
  roundedRect(ctx, -w / 2, -h / 2 + h * 0.17, w, h * 0.83, w * 0.08);
  ctx.fill();
}

/** When a named carton's chip pops in, local seconds. */
export function chipPop(c: Cell): number {
  return c.center ? T_LAND0 + 0.02 : c.land + 0.05;
}

/** Where a carton's props hang: the carton standing, or as it stood when it began to fold. */
const anchor = (c: Cell, t: number): Standing | undefined => standing(c, Math.min(t, c.fold - 1e-4)) ?? undefined;

function drawChips(ctx: CanvasRenderingContext2D, view: View, list: Cell[], t: number): void {
  for (const c of list) {
    const n = c.name;
    if (!n) continue;
    const pop = spring(t - chipPop(c), 5, 0.55);
    if (pop <= 0.001) continue;
    // A condemned chip falls away with its carton; the kept ones go as the
    // keepers hop.
    const gone = c.keep ? smoothstep(T_HOP - 0.1, T_HOP + 0.1, t) : progress(c.fold, c.fold + 0.14, t);
    if (gone >= 1) continue;
    const st = anchor(c, t);
    const top = st ? view.project([c.x, st.lift + CUBE * (st.pose.squash ?? 1), c.z]) : view.project([c.x, 0, c.z]);
    const w = CHIP.pad * 2 + CHIP.icon + CHIP.gap + measure(ctx, n.name);
    const cx = top.x + n.dx;
    const cy = top.y + n.dy + 50 * inQuad(gone);
    const kept = c.keep ? smoothstep(c.hit, c.hit + 0.05, t) : 0;
    const orphan = c.rule === 1 ? smoothstep(T_CRUMPLE, T_CRUMPLE + 0.1, t) : 0;
    ctx.save();
    ctx.globalAlpha *= 1 - gone;
    // The pin, from the carton to the chip's nearest edge: its underside,
    // or its end when the chip hangs beside the carton.
    const beside = top.y < cy + CHIP.h / 2 + 8;
    const ax = beside ? cx + Math.sign(top.x - cx) * (w / 2) * pop : clamp(top.x, cx - w / 2 + 24, cx + w / 2 - 24);
    const ay = beside ? cy : cy + (CHIP.h / 2) * pop;
    ctx.strokeStyle = rgba(PALETTE.paper, 0.55 * clamp(pop));
    ctx.lineWidth = 2;
    ctx.beginPath();
    ctx.moveTo(top.x, top.y - 4);
    ctx.lineTo(ax, ay);
    ctx.stroke();
    ctx.fillStyle = rgba(PALETTE.paper, 0.8 * clamp(pop));
    ctx.beginPath();
    ctx.arc(top.x, top.y - 4, 3.5, 0, TAU);
    ctx.fill();
    ctx.translate(cx, cy);
    ctx.scale(pop, pop);
    if (kept > 0) glow(ctx, 0, 0, w * 0.7, PALETTE.green, 0.4 * kept * Math.exp(-Math.max(0, t - c.hit) / 0.2));
    roundedRect(ctx, -w / 2, -CHIP.h / 2, w, CHIP.h, CHIP.h / 2);
    ctx.fillStyle = mix(rgba(PALETTE.surface, 0.96), mix(PALETTE.surface, PALETTE.green, 0.18), kept);
    ctx.fill();
    ctx.lineWidth = 2.5;
    ctx.strokeStyle = mix(mix(PALETTE.divider, PALETTE.green, kept), PALETTE.divider, orphan);
    ctx.stroke();
    // The folder, unless it has lifted out to crumple.
    const ix = -w / 2 + CHIP.pad + CHIP.icon / 2;
    if (c.rule !== 1 || t < T_LIFT) {
      ctx.save();
      ctx.translate(ix, 0);
      folderGlyph(ctx, CHIP.icon, kept > 0 ? mix(FOLDER, PALETTE.green, kept) : undefined);
      ctx.restore();
    } else {
      // Where it was: a dashed empty outline.
      ctx.save();
      ctx.setLineDash([4, 4]);
      ctx.strokeStyle = rgba(PALETTE.text3, 0.7 * orphan);
      ctx.lineWidth = 2;
      roundedRect(ctx, ix - CHIP.icon / 2, -CHIP.icon * 0.39 + CHIP.icon * 0.13, CHIP.icon, CHIP.icon * 0.65, 3);
      ctx.stroke();
      ctx.restore();
    }
    const tx = ix + CHIP.icon / 2 + CHIP.gap;
    const fill = mix(mix(PALETTE.text1, PALETTE.green, kept), PALETTE.text3, orphan);
    drawText(ctx, n.name, tx, CHIP.size * 0.35, { font: CHIP_FONT, fill });
    if (orphan > 0) {
      // Struck through: its checkout is gone.
      const len = measure(ctx, n.name) * swiftOut(progress(T_CRUMPLE, T_CRUMPLE + 0.12, t));
      ctx.strokeStyle = rgba(PALETTE.text3, 0.95);
      ctx.lineWidth = 3;
      ctx.beginPath();
      ctx.moveTo(tx - 2, -2);
      ctx.lineTo(tx - 2 + len + 4, -2);
      ctx.stroke();
    }
    ctx.restore();
  }
}

const measure = (ctx: CanvasRenderingContext2D, text: string): number => layout(ctx, text, CHIP_FONT).width;

// The orphan's folder: it lifts out of the chip, crumples into a ball, and
// drops onto its carton's roof, bouncing twice.
/** The paper ball's radius, and the folder's width while it hangs over the chip, px. */
const BALL_R = 21;
const LIFTED = 62;
/**
 * The folder's outline pulled into a jagged ball as `k` runs 0 to 1: rays
 * from its center, each easing from the folder's edge to the ball's.
 */
function crumpledPts(k: number, w: number): { x: number; y: number }[] {
  const out: { x: number; y: number }[] = [];
  const N = 30;
  const hw = w / 2;
  const hh = (w * 0.78) / 2;
  for (let i = 0; i < N; i++) {
    const a = (i / N) * TAU;
    const c = Math.cos(a);
    const s = Math.sin(a);
    const box = 1 / Math.max(Math.abs(c) / hw, Math.abs(s) / hh);
    // Alternate rays in and out, so the crush reads as crinkled paper.
    const ball = BALL_R * (i % 2 ? 0.78 + 0.18 * hash(i, 91) : 1 + 0.14 * hash(i, 92));
    const r = lerp(box, ball, k) * (1 + 0.25 * Math.sin(Math.PI * k) * (hash(i, 93) - 0.5));
    out.push({ x: c * r, y: s * r });
  }
  return out;
}

function drawCrumple(ctx: CanvasRenderingContext2D, view: View, c: Cell, st: Standing | undefined, t: number): void {
  if (t < T_LIFT) return;
  const n = c.name;
  if (!n || !st) return;
  // The chip's folder slot, where it starts.
  const top = view.project([c.x, st.lift + CUBE * (st.pose.squash ?? 1), c.z]);
  const w = CHIP.pad * 2 + CHIP.icon + CHIP.gap + measure(ctx, n.name);
  const x0 = top.x + n.dx - w / 2 + CHIP.pad + CHIP.icon / 2;
  const y0 = top.y + n.dy;
  // Up and out of the chip, growing, then crushed in the air.
  const lift = swiftOut(progress(T_LIFT, T_LIFT + 0.12, t));
  const crush = inOutCubic(progress(T_CRUSH, T_CRUMPLE, t));
  const size = lerp(CHIP.icon, LIFTED, lift);
  const hover = { x: x0 - 30 * lift, y: y0 - 74 * lift };
  // Then it drops onto the front of its carton's roof and bounces twice.
  const floor = view.project([c.x - 0.1, st.lift + CUBE * (st.pose.squash ?? 1), c.z + 0.2]);
  let x = hover.x;
  let y = hover.y;
  let rot = -0.35 * crush + 0.06 * Math.sin(TAU * 9 * (t - T_LIFT)) * Math.sin(Math.PI * crush);
  let alpha = 1;
  if (t > T_CRUMPLE) {
    const u = t - T_CRUMPLE;
    const drop = Math.max(10, floor.y - BALL_R - hover.y);
    const g = 5200;
    const t1 = Math.sqrt((2 * drop) / g);
    const v1 = g * t1;
    let yy = 0.5 * g * u * u;
    if (u >= t1) {
      let s = u - t1;
      yy = drop;
      for (const e of [0.36, 0.14]) {
        const v = v1 * e;
        const th = (2 * v) / g;
        if (s < th) {
          yy = drop - (v * s - 0.5 * g * s * s);
          break;
        }
        s -= th;
      }
    }
    const roll = outQuad(clamp(u / (t1 + 0.3)));
    x = lerp(hover.x, floor.x, roll);
    y = hover.y + yy;
    // It rolls as it travels, and comes to rest with it.
    rot += 3.6 * roll;
    alpha = 1 - smoothstep(0.45, 0.7, u);
  }
  if (alpha <= 0) return;
  ctx.save();
  ctx.globalAlpha *= alpha;
  ctx.translate(x, y);
  ctx.rotate(rot);
  if (crush <= 0) {
    folderGlyph(ctx, size);
  } else {
    polygon(ctx, crumpledPts(crush, size));
    ctx.fillStyle = mix(FOLDER, "#c29a55", crush);
    ctx.fill();
    ctx.strokeStyle = rgba(PALETTE.ink, 0.5 * crush);
    ctx.lineWidth = 1.5;
    ctx.stroke();
    // Creases.
    ctx.strokeStyle = rgba(PALETTE.ink, 0.4 * crush);
    ctx.beginPath();
    for (let i = 0; i < 5; i++) {
      const a0 = hash(i, 95) * TAU;
      const r0 = BALL_R * 0.8 * crush;
      ctx.moveTo(Math.cos(a0) * r0, Math.sin(a0) * r0);
      ctx.lineTo(Math.cos(a0 + 2.4) * r0 * 0.35, Math.sin(a0 + 2.4) * r0 * 0.35);
    }
    ctx.stroke();
  }
  ctx.restore();
}

// The 30-day clock the stale carton wears on its front: it pops on, its
// hand runs a lap while the dial fills amber, one tick a day, and it
// flashes as the month runs out and its carton gathers dust.
function drawClock(ctx: CanvasRenderingContext2D, view: View, c: Cell, st: Standing | undefined, t: number): void {
  if (t < T_CLOCK - 0.05 || !st) return;
  const gone = progress(c.fold, c.fold + 0.1, t);
  if (gone >= 1) return;
  const pop = spring(t - T_CLOCK + 0.03, 5, 0.5);
  if (pop <= 0.001) return;
  const f = boxFrame(st.pose);
  // The carton's front left panel faces the viewer; the clock hangs on it.
  const p = view.project(boxPoint(f, 0, 0.1, 1));
  const R = 34;
  const days = clockDays(t);
  const done = t >= T_DAYS ? Math.exp(-(t - T_DAYS) / 0.12) : 0;
  ctx.save();
  ctx.globalAlpha *= 1 - gone;
  ctx.translate(p.x - 6, p.y + 4);
  ctx.scale(pop, pop);
  if (done > 0.01) glow(ctx, 0, 0, R * 2.6, PALETTE.amber, 0.7 * done);
  ctx.beginPath();
  ctx.arc(0, 0, R, 0, TAU);
  ctx.fillStyle = PALETTE.paper;
  ctx.fill();
  // The month so far, as an amber wedge.
  if (days > 0) {
    ctx.beginPath();
    ctx.moveTo(0, 0);
    ctx.arc(0, 0, R - 5, -Math.PI / 2, -Math.PI / 2 + TAU * days);
    ctx.closePath();
    ctx.fillStyle = rgba(PALETTE.amber, 0.55 + 0.45 * done);
    ctx.fill();
  }
  ctx.lineWidth = 4;
  ctx.strokeStyle = PALETTE.ink;
  ctx.beginPath();
  ctx.arc(0, 0, R, 0, TAU);
  ctx.stroke();
  // A tick a day.
  ctx.lineWidth = 1.5;
  ctx.beginPath();
  for (let i = 0; i < 30; i++) {
    const a = (i / 30) * TAU;
    const r0 = i % 5 === 0 ? R - 9 : R - 6;
    ctx.moveTo(Math.cos(a) * r0, Math.sin(a) * r0);
    ctx.lineTo(Math.cos(a) * (R - 3), Math.sin(a) * (R - 3));
  }
  ctx.stroke();
  // The hand.
  const a = -Math.PI / 2 + TAU * days;
  ctx.lineCap = "round";
  ctx.lineWidth = 4;
  ctx.beginPath();
  ctx.moveTo(0, 0);
  ctx.lineTo(Math.cos(a) * (R - 8), Math.sin(a) * (R - 8));
  ctx.stroke();
  ctx.beginPath();
  ctx.arc(0, 0, 3.5, 0, TAU);
  ctx.fillStyle = PALETTE.ink;
  ctx.fill();
  ctx.restore();
}

/** 0..1 of the month the clock's hand has run. */
export const clockDays = (t: number): number => inOutSine(progress(T_DAYS0, T_DAYS, t));

/** 1 when a popIn spring has landed; 0 before `at`. */
const popIn = (t: number, at: number): number => spring(t - at, 4.5, 0.5);

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

// The copy the scene sets besides its caption: the link that puts target/
// in the cache, above the caption; then the rules. The header takes the
// captions' upper line and the three rules share the lower one, landing as
// their groups go flat; the least-recently-used note sits over the last.
const DETAIL_MONO: WordStyle = {
  font: font(40, 500, MONO),
  fill: PALETTE.text3,
  code: { font: font(40, 500, MONO), fill: PALETTE.amber },
  rise: 11,
};
const DETAIL: WordStyle = { ...wordStyle(40, PALETTE.text3) };
const RULE = wordStyle(64);
const LINK = "target -> <cache root>/targets/v1/<checkout digest>";
export const HEADER = "Pruned after builds:";
export const RULE_TEXT = ["checkout deleted", "unused 30 days", "over budget"] as const;
/** The header lands on b5, a beat before the first rule; everything leaves on b11.5. */
export const T_HEADER = b(5);
export const COPY_OUT = b(11.5);
const ROW = [832, 936] as const;
const RULE_GAP = 72;

/**
 * Clip to what a row's exit wipe has left: one wipe runs left to right over
 * a sixteenth from `out` across everything on the row, as a caption's does.
 * False once it has passed.
 */
function rowWipe(ctx: CanvasRenderingContext2D, t: number, out: number, width: number): boolean {
  const k = progress(out, out + WIPE, t);
  if (k >= 1) return false;
  if (k > 0) {
    ctx.beginPath();
    ctx.rect(160 - 8 + k * (width + 16), -1e5, 2e5, 2e5);
    ctx.clip();
  }
  return true;
}

function drawCopy(ctx: CanvasRenderingContext2D, t: number): void {
  drawWords(ctx, LINK, 160, 842, DETAIL_MONO, t, b(1.75), b(4.5));
  if (t < b(4.5)) return;
  // Lay out the rules' row: each rule, with a dot between.
  const xs: number[] = [];
  let x = 160;
  for (const text of RULE_TEXT) {
    xs.push(x);
    x += drawWords(ctx, text, 0, 0, RULE, -Infinity, 0) + RULE_GAP;
  }
  const end = x - RULE_GAP;
  ctx.save();
  if (rowWipe(ctx, t, COPY_OUT, end - 160)) {
    drawWords(ctx, HEADER, 160, ROW[0], RULE, t, T_HEADER);
    drawWords(ctx, "least recently used first", end, ROW[0], DETAIL, t, b(7.25), Infinity, "right");
  }
  ctx.restore();
  ctx.save();
  if (rowWipe(ctx, t, COPY_OUT, end - 160)) {
    RULE_TEXT.forEach((text, i) => {
      // Each rule lands amber as its group goes flat, then cools to paper.
      const hot = t >= T_RULES[i] - 0.06 ? Math.exp(-Math.max(0, t - T_RULES[i]) / 0.35) : 0;
      const style = hot > 0.01 ? wordStyle(64, mix(PALETTE.paper, PALETTE.amberBright, hot)) : RULE;
      drawWords(ctx, text, xs[i], ROW[1], style, t, T_RULES[i]);
      if (i === 0) return;
      // A dot before each later rule, as that rule starts to land.
      const a = smoothstep(0, 0.06, t - entrance(text, T_RULES[i]));
      if (a <= 0) return;
      ctx.fillStyle = rgba(PALETTE.text3, a);
      ctx.beginPath();
      ctx.arc(xs[i] - RULE_GAP / 2, ROW[1] - 20, 5.5, 0, TAU);
      ctx.fill();
    });
  }
  ctx.restore();
  drawWords(ctx, "at most once an hour · budgets scale with the disk", 160, 118, DETAIL, t, b(8), COPY_OUT);
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
      drawCopy(ctx, t);
      return;
    }
    const view = new View(camera(t));
    const list = world();

    drawGrid(ctx, view, t);
    drawFloorFx(ctx, view, list, t);
    drawBeamFloor(ctx, view, t);
    drawScanFrame(ctx, view, t);

    const depth = (c: Cell) => c.x * view.v[0] + c.z * view.v[2];
    const sorted = [...list].sort((a, bb) => depth(a) - depth(bb));

    drawBudgetBack(ctx, view, list, t);
    // The carpet: cartons already slapped flat lie on the floor under
    // everything that stands, with their dust over them.
    for (const c of sorted) if (isFlat(c, t)) drawFold(ctx, view, c, t);
    for (const c of list) if (!c.keep && t >= c.fold) drawDust(ctx, view, c, t);
    drawFloorFlash(ctx, view, t);

    // Contact shadows under everything that stands, falls, or is folding.
    const states = new Map<Cell, Standing>();
    const ups = new Map<Cell, Standing[]>();
    for (const c of list) {
      if (folding(c, t)) {
        shadow(ctx, view, c.x, c.z, 0, 0.45 * (1 - smoothstep(0.01, SLAP, foldU(c, t))));
        continue;
      }
      const st = standing(c, t);
      if (!st) continue;
      states.set(c, st);
      const stack: Standing[] = [];
      for (let k = 0, below = st; k < c.ups.length; k++) {
        const up = standingUp(c, k, below, t);
        if (!up) break;
        stack.push(up);
        below = up;
      }
      if (stack.length) ups.set(c, stack);
      let a = 0.45 * clamp(st.pose.size! / CUBE);
      if (c.keep) a *= 1 - smoothstep(FLAT0 - 0.25, FLAT0, t);
      shadow(ctx, view, c.x, c.z, st.lift, a);
    }

    // Painter's order, split by the beam's plane so the light sheet and the
    // gantry posts sit between the rows it has crossed and the rows in front.
    const drawStanding = (c: Cell, st: Standing, lower: boolean) => {
      // Backlit by the approaching curtain: the faces toward the viewer go
      // dark under a bright rim, until the scanner's verdict.
      const lit = beamLight(c, t);
      const doom = condemnedAt(c, t);
      let pose = st.pose;
      if (lit > 0 && t < c.hit) pose = { ...pose, toneShift: (pose.toneShift ?? 0) - BACKLIT * lit };
      if (doom > 0) pose = { ...pose, toneShift: (pose.toneShift ?? 0) - 0.42 * doom };
      drawSmear(ctx, view, { ...st, pose });
      if (c.keep && lower) drawKept(ctx, view, c, { ...st, pose }, t);
      else drawBox(ctx, view, pose);
      if (lower) coat(ctx, view, pose, DUST_GREY, 0.5 * dustAt(c, t));
      // The scan's touch: an amber blink round the carton it condemns.
      const blink = doom > 0 ? Math.exp(-(t - c.hit) / 0.06) : 0;
      drawLit(ctx, view, pose, Math.max(lit * (t < c.hit ? 1 : 0), tipLight(c, t)));
      if (blink > 0.02) drawLit(ctx, view, pose, blink, rgb(PALETTE.amberBright));
      if (st.spark > 0) {
        const p = view.project([pose.pos[0], st.lift + HALF, pose.pos[2]]);
        glow(ctx, p.x, p.y, 42 * st.spark + 14, PALETTE.amberBright, 0.9 * st.spark);
      }
    };
    const drawCell = (c: Cell) => {
      if (folding(c, t)) {
        if (!isFlat(c, t)) drawFold(ctx, view, c, t);
        return;
      }
      const st = states.get(c);
      if (!st) return;
      drawStanding(c, st, true);
      for (const up of ups.get(c) ?? []) drawStanding(c, up, false);
    };
    if (beamLevel(t) > 0) {
      const c0 = beamPos(t);
      for (const c of sorted) if (c.rank < c0) drawCell(c);
      drawBeamCurtain(ctx, view, t);
      for (const c of sorted) if (c.rank >= c0) drawCell(c);
    } else {
      for (const c of sorted) drawCell(c);
    }

    // The budget box's lid over the first layer, and the hot lines where
    // the pile crosses it.
    drawBudget(ctx, view, list, ups, t);
    const over = overBudget(list, t) * planeFade(t);
    for (const stack of ups.values()) for (const st of stack) drawCrossing(ctx, view, st, over);
    drawBeamBar(ctx, view, t);

    drawTipGlows(ctx, view, t);
    // Scanner reticles sit over the scene like a HUD.
    if (t > T_BEAM0 && t < T_DROP + 0.12)
      for (const c of list) {
        const st = c.keep ? states.get(c) : undefined;
        if (st) drawReticle(ctx, view, c, st, t);
      }
    // The rules' props, then the names.
    for (const c of list) {
      if (c.rule === 2 && c.name) drawClock(ctx, view, c, anchor(c, t), t);
    }
    drawChips(ctx, view, list, t);
    for (const c of list) if (c.rule === 1) drawCrumple(ctx, view, c, anchor(c, t), t);
    drawCopy(ctx, t);
  },
  captions: () => [{ out: 4.5, lines: [{ in: 0.75, text: "`target/` lives in the cache." }] }],
};
