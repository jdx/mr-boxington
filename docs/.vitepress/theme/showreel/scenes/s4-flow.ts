// Scene 4, "Particles": the cache at work. Compiled crates stream out of the
// project and drop through a slit in Mr Boxington's lid on the sixteenths;
// then he spits restored artifacts back out to a worktree and CI on the
// eighths. Every particle is a closed-form ballistic arc (a quadratic Bézier
// walked at a constant rate is exactly a thrown object), so any frame can be
// drawn on its own.

import {
  BAR,
  BEAT,
  bar,
  drawNodeLabel,
  H,
  HERO_CAM,
  NODE_LABEL,
  NODES,
  PALETTE,
  type ReelFacts,
  type Scene,
  W,
  WHIP,
} from "../bible";
import {
  boxFrame,
  type BoxPose,
  drawBox,
  drawShadow,
  FACE_FULL,
  HERO_POSE,
  panelMatrix,
} from "../box";
import { mix, rgba } from "../color";
import { glow, makeCanvas, roundedRect, shake, smear } from "../fx";
import {
  clamp,
  hash,
  inOutSine,
  keys,
  lerp,
  noise1,
  outCubic,
  outExpo,
  progress,
  pulse,
  rng,
  smoothstep,
  spring,
  swiftIn,
  swiftInOut,
  swiftOut,
  TAU,
  wobble,
} from "../math";
import { applyMatrix, type Camera, type DOMMatrix2D, tone, View } from "../space";
import { drawText, font, layout, MONO } from "../type";

type XY = { x: number; y: number };
type NodeKey = keyof typeof NODES;
type Receiver = "worktree" | "ci";

// Beat grid in local time: b12 is 0.
const B = (n: number): number => n * BEAT;
const T_POP = B(0.5);
const GULPS = [B(1), B(1.25), B(1.5), B(1.75)] as const;
/** The coil before the send starts right after the last swallow. */
const T_CHARGE = GULPS[3] + 0.012;
const T_SEND = B(2);
const BURSTS = [B(2), B(2.5), B(3)] as const;
const T_DONE = B(3.5);
/** The project's build prints Finished right after the last swallow. */
const T_FINISHED = GULPS[3] + 0.04;
const T_WHIP = BAR - WHIP;
const LAST = BAR - 1 / 60;

/** Screen gravity shared by every thrown particle, px/s². */
const G = 9000;

const BOX_X = 960;
const BOX_Y = 566;
const BOX_SCALE = 176;

// Node art. Icon bottoms sit a fixed gap above the handoff labels.
const TERM_W = 312;
const TERM_H = 192;
const BAR_H = 32;
const RACK_W = 304;
const RACK_U = 78;
const RACK_GAP = 16;
const RACK_H = RACK_U * 2 + RACK_GAP;
const ICON_BOTTOM = 62;

interface Card {
  x: number;
  y: number;
  w: number;
  h: number;
  r: number;
}
const cardFor = (key: NodeKey, w: number, h: number, r: number): Card => ({
  x: NODES[key].x - w / 2,
  y: NODES[key].y + ICON_BOTTOM - h,
  w,
  h,
  r,
});
const CARDS: Record<NodeKey, Card> = {
  project: cardFor("project", TERM_W, TERM_H, 14),
  worktree: cardFor("worktree", TERM_W, TERM_H, 14),
  ci: cardFor("ci", RACK_W, RACK_H, 12),
};

// The lid's mouth: a slit along the tape seam, in the top face's unit frame
// (u along the tape, v across it).
const SLIT_U0 = 0.12;
const SLIT_U1 = 0.88;
const SLIT_HW = 0.08;
/** Crates end their arc this far below the seam, inside the box. */
const SINK = 16;

const TAG_NAMES = ["serde", "syn", "tokio", "libc", "regex", "clap"] as const;
const COMPILED = [
  "proc-macro2",
  "unicode-ident",
  "quote",
  "memchr",
  "cfg-if",
  "itoa",
  "serde_json",
  "aho-corasick",
  "regex-syntax",
  "bytes",
  "mio",
  "anyhow",
  "log",
];

// Cube colors, quantized so a frame reuses a few dozen CSS strings.
const SHADES = 16;
function ramp(dark: string, midc: string, light: string): string[] {
  const out: string[] = [];
  for (let i = 0; i <= SHADES; i++) {
    const k = i / SHADES;
    out.push(k < 0.5 ? mix(dark, midc, k * 2) : mix(midc, light, k * 2 - 1));
  }
  return out;
}
let AMBER_RAMP: string[] | null = null;
let GREEN_RAMP: string[] | null = null;
const amberRamp = () => (AMBER_RAMP ??= ramp("#8f5717", "#e2ab51", "#fbe2a6"));
const greenRamp = () => (GREEN_RAMP ??= ramp("#46703f", "#a1ca92", "#e4f5da"));

// A view with the identity basis: the cube normals below are already in
// camera space, so tone() lights them with the hero box's own key light.
const CAM_SPACE = new View({ cx: 0, cy: 0, scale: 1, yaw: 0, pitch: 0 });

// Unit cube: faces as corner indices into CORNERS, with outward normals.
const CORNERS: readonly [number, number, number][] = [
  [-1, -1, -1],
  [1, -1, -1],
  [1, 1, -1],
  [-1, 1, -1],
  [-1, -1, 1],
  [1, -1, 1],
  [1, 1, 1],
  [-1, 1, 1],
];
const FACES: readonly [number[], [number, number, number]][] = [
  [[4, 5, 6, 7], [0, 0, 1]],
  [[1, 0, 3, 2], [0, 0, -1]],
  [[5, 1, 2, 6], [1, 0, 0]],
  [[0, 4, 7, 3], [-1, 0, 0]],
  [[7, 6, 2, 3], [0, 1, 0]],
  [[0, 1, 5, 4], [0, -1, 0]],
];
const px = new Float64Array(8);
const py = new Float64Array(8);

/** A small tumbling cube, orthographic, lit like the hero box. */
function drawCube(
  ctx: CanvasRenderingContext2D,
  x: number,
  y: number,
  size: number,
  a: number,
  b: number,
  shades: string[],
) {
  const ca = Math.cos(a);
  const sa = Math.sin(a);
  const cb = Math.cos(b);
  const sb = Math.sin(b);
  const h = size / 2;
  // Rotate about Y by b, then about X by a.
  const rot = (v: readonly [number, number, number]): [number, number, number] => {
    const x1 = v[0] * cb + v[2] * sb;
    const z1 = -v[0] * sb + v[2] * cb;
    return [x1, v[1] * ca - z1 * sa, v[1] * sa + z1 * ca];
  };
  for (let i = 0; i < 8; i++) {
    const r = rot(CORNERS[i]);
    px[i] = x + r[0] * h;
    py[i] = y - r[1] * h;
  }
  for (const [idx, n] of FACES) {
    const rn = rot(n);
    if (rn[2] <= 0.02) continue;
    const k = Math.round(tone(CAM_SPACE, rn) * SHADES);
    ctx.fillStyle = shades[k];
    ctx.beginPath();
    ctx.moveTo(px[idx[0]], py[idx[0]]);
    ctx.lineTo(px[idx[1]], py[idx[1]]);
    ctx.lineTo(px[idx[2]], py[idx[2]]);
    ctx.lineTo(px[idx[3]], py[idx[3]]);
    ctx.closePath();
    ctx.fill();
  }
}

// Geometry helpers.

/** A point of a plane matrix's local frame on screen. */
const mp = (m: DOMMatrix2D, u: number, v: number): XY => ({
  x: m.a * u + m.c * v + m.e,
  y: m.b * u + m.d * v + m.f,
});

/** Rounded rect as a subpath (fx.roundedRect starts a new path). */
function rrPath(ctx: CanvasRenderingContext2D, c: Card) {
  const { x, y, w, h, r } = c;
  ctx.moveTo(x + r, y);
  ctx.arcTo(x + w, y, x + w, y + h, r);
  ctx.arcTo(x + w, y + h, x, y + h, r);
  ctx.arcTo(x, y + h, x, y, r);
  ctx.arcTo(x, y, x + w, y, r);
  ctx.closePath();
}

function polyPath(ctx: CanvasRenderingContext2D, pts: readonly XY[]) {
  ctx.moveTo(pts[0].x, pts[0].y);
  for (let i = 1; i < pts.length; i++) ctx.lineTo(pts[i].x, pts[i].y);
  ctx.closePath();
}

/** The slit as a lens in screen space, `open` 0..1 of its full width. */
function slitPoly(m: DOMMatrix2D, open: number): XY[] {
  const N = 14;
  const pts: XY[] = [];
  for (let i = 0; i <= N; i++) {
    const w = SLIT_HW * open * Math.sin((Math.PI * i) / N) ** 0.6;
    pts.push(mp(m, lerp(SLIT_U0, SLIT_U1, i / N), 0.5 - w));
  }
  for (let i = N; i >= 0; i--) {
    const w = SLIT_HW * open * Math.sin((Math.PI * i) / N) ** 0.6;
    pts.push(mp(m, lerp(SLIT_U0, SLIT_U1, i / N), 0.5 + w));
  }
  return pts;
}

const lidPoly = (m: DOMMatrix2D): XY[] => [mp(m, 0, 0), mp(m, 1, 0), mp(m, 1, 1), mp(m, 0, 1)];

// Flights: every particle's launch, landing, and ballistic control point.

interface Flight {
  /** Launch and landing times (local). */
  e: number;
  a: number;
  x0: number;
  y0: number;
  x1: number;
  y1: number;
  x2: number;
  y2: number;
  size: number;
  spin: number;
  phase: number;
  seed: number;
  inbound: boolean;
  /** Fraction of the flight at the lid end spent passing through the slit. */
  slit: number;
  tag?: string;
  /** For outbound flights, which node receives it. */
  to?: Receiver;
}

/** A thrown arc from p0 to p2 taking a - e seconds under gravity G. */
function toss(e: number, a: number, x0: number, y0: number, x2: number, y2: number) {
  const T = a - e;
  return { e, a, x0, y0, x2, y2, x1: (x0 + x2) / 2, y1: (y0 + y2) / 2 - (G * T * T) / 4 };
}

/** The slit phase: the last (or first) ~0.04 s of a flight, at most 12% of it. */
const slitShare = (e: number, a: number) => Math.min(0.12, 0.04 / (a - e));

/** The hero's own camera: the logo view, smaller, at frame center. */
const BOX_CAM = (): Camera => ({ ...HERO_CAM, cx: BOX_X, cy: BOX_Y, scale: BOX_SCALE });

let restLidM: DOMMatrix2D | null = null;
/** The lid's plane matrix with the box at rest. */
function restLid(): DOMMatrix2D {
  return (restLidM ??= panelMatrix(new View(BOX_CAM()), boxFrame({ ...HERO_POSE }), "top"));
}

interface Plan {
  inbound: Flight[];
  outbound: Flight[];
  /** Arrival times per receiving node, sorted. */
  arrivals: Record<Receiver, number[]>;
  /** Project log lines: [time, crate]. */
  compiled: [number, string][];
}

const bez = (f: Flight, u: number): XY => {
  const v = 1 - u;
  return {
    x: v * v * f.x0 + 2 * v * u * f.x1 + u * u * f.x2,
    y: v * v * f.y0 + 2 * v * u * f.y1 + u * u * f.y2,
  };
};

// Named-crate tags: a pill to the upper right of the crate, visible over the
// middle of the flight.
const TAG_FONT = () => font(19, 600, MONO);
const TAG_H = 30;
const TAG_DX = 16;
const TAG_DY = -20;
const TAG_SHOW: [number, number, number, number] = [0.05, 0.13, 0.68, 0.8];
const tagWidth = (name: string) => name.length * 11.4 + 24;

/** Gap between two tags' pills at time t (negative when they overlap). */
function tagGap(f: Flight, g: Flight, t: number): number {
  const p = bez(f, (t - f.e) / (f.a - f.e));
  const q = bez(g, (t - g.e) / (g.a - g.e));
  const ax = p.x + TAG_DX;
  const bx = q.x + TAG_DX;
  const dx = Math.max(ax - (bx + tagWidth(g.tag ?? "")), bx - (ax + tagWidth(f.tag ?? "")));
  const dy = Math.abs(p.y - q.y) - TAG_H;
  return Math.max(dx, dy);
}

let plan: Plan | null = null;
function getPlan(): Plan {
  if (plan) return plan;
  const r = rng(4417);
  const lid = restLid();
  const pc = CARDS.project;
  const launchX = pc.x + pc.w - 8;
  const launchY = (k: number) => lerp(pc.y + BAR_H + 24, pc.y + pc.h - 20, k);
  const landing = (u: number): XY => {
    const p = mp(lid, u, 0.5);
    return { x: p.x, y: p.y + SINK };
  };
  const inbound: Flight[] = [];
  const compiled: [number, string][] = [];

  // Named crates first: each takes the launch slot, height, and gulp whose
  // pill stays clearest of the tags already placed, so they never collide.
  const tags: Flight[] = [];
  TAG_NAMES.forEach((name, i) => {
    let best: Flight | null = null;
    let bestScore = -Infinity;
    for (let e = 0.25; e <= 0.575; e += 0.0125) {
      for (let yi = 0; yi < 5; yi++) {
        for (const g of GULPS) {
          const T = g - e;
          if (T < 0.3 || T > 0.46) continue;
          const l = landing(0.36 + 0.28 * ((i * 0.37) % 1));
          const f: Flight = {
            ...toss(e, g - 0.008, launchX, launchY(yi / 4), l.x, l.y),
            size: 15,
            spin: 0.8,
            phase: hash(i, 21) * TAU,
            seed: i,
            inbound: true,
            slit: slitShare(e, g - 0.008),
            tag: name,
          };
          let gap = 60;
          let spread = 1;
          for (const o of tags) {
            spread = Math.min(spread, Math.abs(o.e - e) / 0.05);
            const t0 = Math.max(f.e + TAG_SHOW[0] * (f.a - f.e), o.e + TAG_SHOW[0] * (o.a - o.e));
            const t1 = Math.min(f.e + TAG_SHOW[3] * (f.a - f.e), o.e + TAG_SHOW[3] * (o.a - o.e));
            for (let k = 0; k <= 10 && t1 > t0; k++) {
              gap = Math.min(gap, tagGap(f, o, lerp(t0, t1, k / 10)));
            }
          }
          // Clear of the others first, then spread in time, then a lob.
          const score = Math.min(gap, 24) + 8 * Math.min(spread, 1) + 4 * T;
          if (score > bestScore) {
            bestScore = score;
            best = f;
          }
        }
      }
    }
    if (best) {
      tags.push(best);
      compiled.push([best.e, name]);
    }
  });
  inbound.push(...tags);

  // The stream: launches run from the pop to just before the last gulp, and
  // each crate lands in the packet of a gulp it can reach, so arcs differ in
  // height and the line from the terminal to the lid never breaks.
  const N_IN = 60;
  for (let i = 0; i < N_IN; i++) {
    const e = 0.25 + (i / (N_IN - 1)) * 0.47 + (r() - 0.5) * 0.01;
    const feasible = GULPS.filter((g) => g - e >= 0.1 && g - e <= 0.46);
    const pick = r() < 0.6 ? 0 : Math.min(1, feasible.length - 1);
    let slot: number = feasible.length ? feasible[pick] : GULPS[3];
    // The first few are flat, fast shots so the b13 gulp has a mouthful.
    if (i < 6) slot = GULPS[0];
    const a = slot - 0.004 - r() * 0.018;
    const l = landing(0.3 + 0.4 * r());
    inbound.push({
      ...toss(e, a, launchX - r() * 6, launchY(r()), l.x, l.y),
      size: 8 + r() * 5,
      spin: 0.8 + r() * 1.6,
      phase: r() * TAU,
      seed: inbound.length,
      inbound: true,
      slit: slitShare(e, a),
    });
    if (i % 6 === 2) compiled.push([e, COMPILED[(i / 6) | 0] ?? "log"]);
  }
  compiled.sort((p, q) => p[0] - q[0]);

  // Outbound: three volleys per receiver. The last volley's arrivals spread
  // out as they near b15.5, so the counters tick down into the lock, and the
  // final artifact lands exactly on it.
  const outbound: Flight[] = [];
  const arrivals: Record<Receiver, number[]> = { worktree: [], ci: [] };
  for (const to of ["worktree", "ci"] as const) {
    const dst = CARDS[to];
    const y0 = to === "worktree" ? dst.y + BAR_H + 18 : dst.y + 14;
    const y1 = dst.y + dst.h - 16;
    const mine: Flight[] = [];
    const add = (e: number, a: number) => {
      const s = mp(lid, 0.3 + 0.4 * r(), 0.5);
      mine.push({
        ...toss(e, a, s.x, s.y + SINK, dst.x + 6, lerp(y0, y1, r())),
        size: 8 + r() * 6,
        spin: 0.8 + r() * 1.6,
        phase: r() * TAU,
        seed: 500 + outbound.length + mine.length,
        inbound: false,
        slit: slitShare(e, a),
        to,
      });
    };
    const jit = () => (r() - 0.5) * 0.008;
    for (let i = 0; i < 16; i++) {
      const e = BURSTS[0] + jit();
      add(e, e + 0.24 + 0.2 * r());
    }
    for (let i = 0; i < 14; i++) {
      const e = BURSTS[1] + jit();
      add(e, e + 0.2 + 0.18 * r());
    }
    const n = 14;
    const [a0, ease] = to === "worktree" ? [1.515, 0.55] : [1.5, 0.48];
    for (let j = 0; j < n; j++) {
      const a = lerp(a0, T_DONE, 1 - (1 - j / (n - 1)) ** ease);
      add(Math.min(BURSTS[2] + jit(), a - 0.1), a);
    }
    outbound.push(...mine);
    arrivals[to] = mine.map((f) => f.a).sort((p, q) => p - q);
  }
  plan = { inbound, outbound, arrivals, compiled };
  return plan;
}

// Anticipation: comet sparks spiral in on a tilted plane and collapse into a
// hot point on the frame before the pop.

interface Comet {
  s: number;
  a: number;
  r0: number;
  th0: number;
  turns: number;
  bright: number;
  color: string;
  w: number;
}
let comets: Comet[] | null = null;
function getComets(): Comet[] {
  if (comets) return comets;
  const r = rng(907);
  const N = 22;
  const colors = [PALETTE.amberBright, PALETTE.paper, "#c4862c", PALETTE.amberBright, PALETTE.amber];
  comets = [];
  for (let i = 0; i < N; i++) {
    const a = 0.125 + 0.08 * (i / (N - 1)) + (r() - 0.5) * 0.012;
    comets.push({
      s: Math.max(0.004 + 0.02 * r(), a - 0.12 - r() * 0.08),
      a: Math.min(a, T_POP - 0.03),
      r0: 200 + 240 * r(),
      th0: r() * TAU,
      turns: 0.28 + 0.3 * r(),
      bright: 0.6 + 0.4 * r(),
      color: colors[i % colors.length],
      w: 2.2 + 2.4 * r(),
    });
  }
  return comets;
}

function cometAt(c: Comet, q: number): XY {
  const rr = c.r0 * (1 - q) ** 1.1;
  const th = c.th0 + c.turns * TAU * q ** 1.5;
  return { x: BOX_X + Math.cos(th) * rr, y: BOX_Y + Math.sin(th) * rr * 0.6 };
}

const COMET_N = 18;
const cqx = new Float64Array(COMET_N);
const cqy = new Float64Array(COMET_N);

function gather(ctx: CanvasRenderingContext2D, lt: number) {
  if (lt <= 0 || lt >= T_POP) return;
  const cs = getComets();
  let core = 0;
  ctx.save();
  for (const c of cs) {
    core += smoothstep(c.a - 0.015, c.a + 0.01, lt);
    if (lt <= c.s || lt >= c.a) continue;
    const p = progress(c.s, c.a, lt);
    const ease = (k: number) => Math.max(0, k) ** 2.1;
    const alpha = c.bright * smoothstep(0, 0.3, p);
    // The tail reaches back to where the head was a quarter of the flight
    // ago, so it lengthens as the comet speeds up. One tapered ribbon along
    // the path, one gradient: no joints, no beads.
    const qh = ease(p);
    const qt = ease(p - 0.25);
    for (let k = 0; k < COMET_N; k++) {
      const q = cometAt(c, lerp(qh, qt, k / (COMET_N - 1)));
      cqx[k] = q.x;
      cqy[k] = q.y;
    }
    const hx = cqx[0];
    const hy = cqy[0];
    const ex = cqx[COMET_N - 1];
    const ey = cqy[COMET_N - 1];
    if ((hx - ex) ** 2 + (hy - ey) ** 2 > 4) {
      ctx.beginPath();
      for (let side = 1; side >= -1; side -= 2) {
        for (let j = 0; j < COMET_N; j++) {
          const k = side > 0 ? j : COMET_N - 1 - j;
          const a = Math.max(k - 1, 0);
          const b = Math.min(k + 1, COMET_N - 1);
          let nx = cqy[a] - cqy[b];
          let ny = cqx[b] - cqx[a];
          const l = Math.hypot(nx, ny) || 1;
          const w = side * c.w * 0.55 * (1 - k / (COMET_N - 1)) ** 0.85;
          nx = (nx / l) * w;
          ny = (ny / l) * w;
          if (j === 0 && side > 0) ctx.moveTo(cqx[k] + nx, cqy[k] + ny);
          else ctx.lineTo(cqx[k] + nx, cqy[k] + ny);
        }
      }
      ctx.closePath();
      const g = ctx.createLinearGradient(hx, hy, ex, ey);
      g.addColorStop(0, rgba(PALETTE.paper, alpha));
      g.addColorStop(0.12, rgba(c.color, alpha * 0.9));
      g.addColorStop(0.55, rgba(c.color, alpha * 0.35));
      g.addColorStop(1, rgba(c.color, 0));
      // Additive, so the vortex heats up where the tails converge.
      ctx.globalCompositeOperation = "lighter";
      ctx.fillStyle = g;
      ctx.fill();
      ctx.globalCompositeOperation = "source-over";
    }
    ctx.fillStyle = rgba(PALETTE.paper, alpha);
    ctx.beginPath();
    ctx.arc(hx, hy, c.w * 0.72, 0, TAU);
    ctx.fill();
    glow(ctx, hx, hy, 8 + 5 * c.w, c.color, 0.45 * alpha);
  }
  ctx.restore();
  core /= cs.length;
  // The hot point swells as sparks arrive, then pinches just before the pop.
  const pinch = swiftIn(progress(T_POP - 0.045, T_POP, lt));
  const r = (12 + 40 * core) * (1 - 0.5 * pinch);
  glow(ctx, BOX_X, BOX_Y, r * 2.6, PALETTE.amberBright, 0.2 + 0.6 * core);
  glow(ctx, BOX_X, BOX_Y, r, "#fff4dc", core * (0.7 + 0.3 * pinch));
  if (core > 0.02) {
    ctx.fillStyle = rgba("#fffaf0", clamp(core * 1.5));
    ctx.beginPath();
    ctx.arc(BOX_X, BOX_Y, (2 + 4 * core) * (1 - 0.35 * pinch), 0, TAU);
    ctx.fill();
  }
}

// The pop: a shockwave that leads the box and embers that burn out before b13.

function screenRing(
  ctx: CanvasRenderingContext2D,
  r0: number,
  r1: number,
  p: number,
  color: string,
  width: number,
) {
  if (p <= 0 || p >= 1) return;
  ctx.strokeStyle = rgba(color, (1 - p) ** 1.3);
  ctx.lineWidth = width * (1 - p) ** 1.2 + 0.5;
  ctx.beginPath();
  ctx.arc(BOX_X, BOX_Y, lerp(r0, r1, outExpo(p)), 0, TAU);
  ctx.stroke();
}

function popBehind(ctx: CanvasRenderingContext2D, lt: number) {
  const d = lt - T_POP;
  if (d < 0 || d > 0.3) return;
  glow(ctx, BOX_X, BOX_Y, 320, PALETTE.amberBright, 0.9 * pulse(lt, T_POP, 0.001, 0.05));
  ctx.save();
  screenRing(ctx, 195, 460, progress(T_POP, T_POP + 0.2, lt), PALETTE.amberBright, 6);
  screenRing(ctx, 165, 350, progress(T_POP + 0.012, T_POP + 0.17, lt), PALETTE.paper, 2);
  ctx.restore();
}

/**
 * Debris from the pop: sparks and flecks of cardboard on random headings and
 * speeds under drag and gravity. Sparks are streaks whose length follows their
 * speed; flecks tumble. They sit behind the box and outrun its growth, so they
 * spray out from its silhouette, and all of it is gone before b13.
 */
function popDebris(ctx: CanvasRenderingContext2D, lt: number) {
  const d = lt - T_POP;
  if (d < 0 || d > 0.23) return;
  ctx.save();
  ctx.lineCap = "round";
  const sparkColors = [PALETTE.amberBright, PALETTE.paper, PALETTE.amber];
  const fleckColors = ["#e2ab51", "#c4862c", "#f2c479", "#955c19"];
  for (let i = 0; i < 28; i++) {
    const fleck = i % 3 === 2;
    const life = (fleck ? 0.1 : 0.11) + (fleck ? 0.035 : 0.06) * hash(i, 11);
    if (d >= life) continue;
    const ang = TAU * hash(i, 8);
    const k = (fleck ? 6 : 8) + 5 * hash(i, 10);
    const reach = fleck ? 250 + 200 * hash(i, 9) : 320 + 300 * hash(i, 9);
    const decay = Math.exp(-k * d);
    const dist = 30 + reach * (1 - decay);
    const speed = reach * k * decay;
    // Scraps are heavy: once the burst's drag has spent them they drop out,
    // and they fade as they slow, so none hangs in the air.
    const g = fleck ? 3200 : 800;
    const ca = Math.cos(ang);
    const sa = Math.sin(ang);
    const x = BOX_X + ca * dist;
    const y = BOX_Y + sa * dist * 0.9 + 0.5 * g * d * d;
    const f = 1 - d / life;
    if (fleck) {
      // A tumbling scrap: a quad that spins and flips as it flies.
      const sz = 5 + 6 * hash(i, 12);
      const rot = TAU * hash(i, 13) + d * (14 + 10 * hash(i, 14));
      ctx.save();
      ctx.translate(x, y);
      ctx.rotate(rot);
      ctx.scale(1, Math.cos(d * 30 + i));
      ctx.globalAlpha *= Math.min(1, f * 2.5) * Math.sqrt(decay);
      ctx.fillStyle = fleckColors[i % fleckColors.length];
      ctx.fillRect(-sz / 2, -sz * 0.35, sz, sz * 0.7);
      ctx.restore();
      continue;
    }
    const vx = ca * speed;
    const vy = sa * speed * 0.9 + g * d;
    const vl = Math.hypot(vx, vy) || 1;
    const len = 2 + speed * 0.016;
    const w = (1.4 + 2.2 * hash(i, 15)) * (0.4 + 0.6 * f);
    const col = sparkColors[i % sparkColors.length];
    ctx.strokeStyle = rgba(col, f ** 0.6);
    ctx.lineWidth = w;
    ctx.beginPath();
    ctx.moveTo(x - (vx / vl) * len, y - (vy / vl) * len);
    ctx.lineTo(x, y);
    ctx.stroke();
    ctx.fillStyle = rgba(PALETTE.paper, f);
    ctx.beginPath();
    ctx.arc(x, y, w * 0.75, 0, TAU);
    ctx.fill();
    if (i % 3 === 0) glow(ctx, x, y, 10 + 4 * w, col, 0.5 * f);
  }
  ctx.restore();
}

// Mr Boxington's performance.

interface BoxState {
  s: number;
  squash: number;
  look: [number, number];
  browLift: number;
  blink: number;
  twitch: number;
  glint: number;
  tone: number;
  /** 0..1 build toward the send. */
  build: number;
  /** Screen-space rumble while charging, px. */
  rumble: [number, number];
}

const lookX = keys([
  [T_POP + 0.1, 0],
  [T_POP + 0.2, -5, swiftOut],
  [GULPS[3] + 0.02, -5],
  [T_SEND + 0.05, 4, swiftOut],
  [B(2.9), 4],
  [B(3.3), 0, swiftInOut],
]);
const lookY = keys([
  [T_POP + 0.1, 0],
  [T_POP + 0.2, -3.5, swiftOut],
  [GULPS[3] + 0.02, -2],
  [T_SEND + 0.05, -4, swiftOut],
  [B(2.55), -4],
  [B(2.7), 3.5, swiftOut],
  [B(3.1), 3.5],
  [B(3.3), 0, swiftInOut],
]);
const blinkK = keys([
  [GULPS[3] + 0.01, 0],
  [T_SEND - 0.04, 1, swiftOut],
  [T_SEND, 1],
  [T_SEND + 0.05, 0, swiftOut],
]);
// A twinkle on the rim, gone before the whip.
const glintK = keys([
  [T_DONE, 0],
  [T_DONE + 0.03, 0.5, outCubic],
  [T_DONE + 0.13, 0, inOutSine],
]);

function boxState(lt: number): BoxState {
  // Full size three frames after the pop, a 30% overshoot, settled by b13.
  const s = lt < T_POP ? 0 : spring(lt - T_POP, 4, 0.45, 20);
  // Stretch leads the growth; the squash lands as the overshoot recovers.
  let squash = 1 + 0.25 * wobble(lt, T_POP, 5, 7);
  const build = lt < T_SEND ? progress(T_CHARGE, T_SEND, lt) : 0;
  const calm = lt < T_SEND ? 1 - build : 0;
  let tone = 0;
  // Each swallow bulges the box; amber warmth only while swallowing.
  for (const g of GULPS) {
    squash -= 0.12 * wobble(lt, g, 6, 11) * calm;
    tone += 0.035 * pulse(lt, g, 0.01, 0.05) * calm;
  }
  // Coil into a deep sink before the send, then release into a stretch.
  if (lt < T_SEND) squash -= 0.22 * outCubic(build);
  else squash -= 0.22 * (1 - spring(lt - T_SEND, 3.4, 0.32, 16));
  // Later volleys: a quick coil ahead of the beat and a stretch on it.
  for (const b of BURSTS.slice(1)) squash -= 0.15 * wobble(lt, b - 1 / 12, 6, 9);
  // A satisfied little bounce when every lookup has hit.
  squash -= 0.06 * wobble(lt, T_DONE, 5, 8);
  // Rumble: alternating frame to frame, growing with the charge.
  const fr = Math.floor(lt * 60);
  const amp = 5.5 * build ** 1.2;
  const rumble: [number, number] = [
    amp * (fr % 2 ? 1 : -1) * (0.6 + 0.4 * hash(fr, 71)),
    amp * 0.35 * (hash(fr, 73) * 2 - 1),
  ];
  let browLift =
    (lt >= T_POP ? 9 * (1 - spring(lt - T_POP, 2.6, 0.45)) : 0) +
    (lt >= T_SEND ? 7 * Math.exp(-(lt - T_SEND) / 0.25) : 0) +
    (lt >= T_DONE ? 5 * Math.sin(Math.PI * progress(T_DONE, T_DONE + 0.2, lt)) : 0) -
    3 * swiftIn(build);
  for (const b of BURSTS.slice(1)) browLift += 3 * pulse(lt, b, 0.02, 0.08);
  let twitch = 0;
  for (const g of GULPS) twitch += 0.12 * wobble(lt, g, 8, 12);
  return {
    s,
    squash,
    look: [lookX(lt), lookY(lt)],
    browLift,
    blink: blinkK(lt),
    twitch,
    glint: glintK(lt),
    tone,
    build,
    rumble,
  };
}

function boxView(st: BoxState): View {
  return new View({
    ...BOX_CAM(),
    cx: BOX_X + st.rumble[0],
    cy: BOX_Y + st.rumble[1],
    scale: BOX_SCALE * Math.max(st.s, 0.001),
  });
}

function heroPose(st: BoxState): BoxPose {
  return {
    ...HERO_POSE,
    squash: st.squash,
    toneShift: st.tone,
    face: {
      ...FACE_FULL,
      look: st.look,
      browLift: st.browLift,
      blink: st.blink,
      twitch: st.twitch,
      glint: st.glint,
    },
  };
}

// The mouth: open ahead of each packet and snapped shut on the swallow, cracked
// with green light through the charge, thrown wide on the send.
function slitOpen(lt: number): number {
  let o = 0;
  for (const g of GULPS) {
    const opening = outCubic(progress(g - 0.08, g - 0.035, lt));
    const closing = swiftOut(progress(g, g + 0.045, lt));
    o = Math.max(o, 0.72 * opening * (1 - closing));
  }
  if (lt < T_SEND) return Math.max(o, 0.38 * outCubic(progress(T_CHARGE, T_SEND, lt)));
  let s = lerp(0.38, 1, outExpo(progress(T_SEND, T_SEND + 0.05, lt)));
  for (const b of BURSTS.slice(1)) s += 0.25 * pulse(lt, b, 0.03, 0.06);
  return s * (1 - swiftInOut(progress(BURSTS[2] + 0.07, BURSTS[2] + 0.17, lt)));
}

/** 0..1 green light inside the box. */
function slitLight(lt: number): number {
  if (lt < T_SEND) return outCubic(progress(T_CHARGE, T_SEND, lt));
  return 1 - progress(BURSTS[2] + 0.05, BURSTS[2] + 0.17, lt);
}

function drawSlit(ctx: CanvasRenderingContext2D, m: DOMMatrix2D, slit: XY[], open: number, lit: number) {
  if (open < 0.02) return;
  ctx.save();
  ctx.beginPath();
  polyPath(ctx, slit);
  ctx.fillStyle = mix("#1b1209", "#dff5d2", lit * 0.9);
  ctx.fill();
  // The far flap's cut edge shows as a darker band just inside the far lip.
  ctx.clip();
  ctx.beginPath();
  const N = 14;
  for (let i = 0; i <= N; i++) {
    const w = SLIT_HW * open * Math.sin((Math.PI * i) / N) ** 0.6;
    const p = mp(m, lerp(SLIT_U0, SLIT_U1, i / N), 0.5 - w);
    if (i) ctx.lineTo(p.x, p.y);
    else ctx.moveTo(p.x, p.y);
  }
  for (let i = N; i >= 0; i--) {
    const w = SLIT_HW * open * Math.sin((Math.PI * i) / N) ** 0.6;
    const p = mp(m, lerp(SLIT_U0, SLIT_U1, i / N), 0.5 - w);
    ctx.lineTo(p.x, p.y + 5 * open);
  }
  ctx.closePath();
  ctx.fillStyle = mix("#7a4a14", PALETTE.green, lit * 0.6);
  ctx.fill();
  ctx.restore();
  ctx.save();
  ctx.beginPath();
  polyPath(ctx, slit);
  ctx.strokeStyle = mix("#53350f", PALETTE.green, lit * 0.5);
  ctx.lineWidth = 2;
  ctx.lineJoin = "round";
  ctx.stroke();
  ctx.restore();
}

/**
 * A shockwave on the floor around the box's base: the kick of a send. Every
 * radius clears the footprint (half-diagonal 0.707, 0.8 at the deepest
 * squash), so drawn before the box the far arc is hidden and the near arc
 * passes in front of his base, exactly as a ring on the floor would.
 */
function floorRing(
  ctx: CanvasRenderingContext2D,
  floor: DOMMatrix2D,
  r0: number,
  r1: number,
  p: number,
  width: number,
  alpha: number,
  color: string = PALETTE.green,
) {
  if (p <= 0 || p >= 1) return;
  const a = alpha * (1 - p) ** 1.3;
  const r = lerp(r0, r1, outExpo(p));
  const edge = width * (1 - p) ** 1.1 + 0.006;
  // A crisp leading edge with a soft wake behind it, like a pressure front.
  const outer = r + edge / 2;
  const inner = Math.max(0.01, r - edge / 2 - (0.05 + 0.3 * (1 - p)) * (width / 0.07));
  const k = (r - edge / 2 - inner) / (outer - inner);
  ctx.save();
  applyMatrix(ctx, floor);
  const g = ctx.createRadialGradient(0, 0, inner, 0, 0, outer);
  g.addColorStop(0, rgba(color, 0));
  g.addColorStop(k * 0.96, rgba(color, 0.26 * a));
  g.addColorStop(k, rgba(color, a));
  g.addColorStop(1, rgba(color, a));
  ctx.fillStyle = g;
  ctx.beginPath();
  ctx.arc(0, 0, outer, 0, TAU);
  ctx.arc(0, 0, inner, TAU, 0, true);
  ctx.fill();
  ctx.restore();
}

/** Light pooled on the floor around the base, as an ellipse in the floor plane. */
function floorGlow(ctx: CanvasRenderingContext2D, floor: DOMMatrix2D, r: number, color: string, a: number) {
  if (a <= 0.004) return;
  ctx.save();
  applyMatrix(ctx, floor);
  glow(ctx, 0, 0, r, color, a);
  ctx.restore();
}

// Particles.

function flightAt(f: Flight, t: number, dm: XY): XY {
  const u = clamp((t - f.e) / (f.a - f.e));
  const p = bez(f, u);
  // Track the live mouth (it squashes and rumbles) at the box end of the arc.
  const w = f.inbound ? u * u : (1 - u) * (1 - u);
  // A little seeded flutter across the path, zero at both ends.
  const fl = Math.sin(Math.PI * u) * 4;
  return {
    x: p.x + dm.x * w + noise1(t * 7 + f.seed * 3.1, 7) * fl,
    y: p.y + dm.y * w + noise1(t * 7 + f.seed * 5.7, 9) * fl,
  };
}

const TRAIL_N = 7;
const TRAIL_DT = 0.009;
const tx = new Float64Array(TRAIL_N);
const ty = new Float64Array(TRAIL_N);
const lx = new Float64Array(TRAIL_N);
const ly = new Float64Array(TRAIL_N);
const rx = new Float64Array(TRAIL_N);
const ry = new Float64Array(TRAIL_N);

function drawTrail(
  ctx: CanvasRenderingContext2D,
  f: Flight,
  lt: number,
  dm: XY,
  color: string,
  width: number,
) {
  for (let k = 0; k < TRAIL_N; k++) {
    const p = flightAt(f, clamp(lt - k * TRAIL_DT, f.e, f.a), dm);
    tx[k] = p.x;
    ty[k] = p.y;
  }
  const dx = tx[0] - tx[TRAIL_N - 1];
  const dy = ty[0] - ty[TRAIL_N - 1];
  if (dx * dx + dy * dy < 4) return;
  // Tapered ribbon: widest at the head, a point at the tail.
  for (let k = 0; k < TRAIL_N; k++) {
    const a = Math.max(k - 1, 0);
    const b = Math.min(k + 1, TRAIL_N - 1);
    let nx = -(ty[a] - ty[b]);
    let ny = tx[a] - tx[b];
    const l = Math.hypot(nx, ny) || 1;
    nx /= l;
    ny /= l;
    const w = width * (1 - k / (TRAIL_N - 1)) ** 0.9;
    lx[k] = tx[k] + nx * w;
    ly[k] = ty[k] + ny * w;
    rx[k] = tx[k] - nx * w;
    ry[k] = ty[k] - ny * w;
  }
  const g = ctx.createLinearGradient(tx[0], ty[0], tx[TRAIL_N - 1], ty[TRAIL_N - 1]);
  g.addColorStop(0, rgba(color, 0.55));
  g.addColorStop(1, rgba(color, 0));
  ctx.fillStyle = g;
  ctx.beginPath();
  ctx.moveTo(lx[0], ly[0]);
  for (let k = 1; k < TRAIL_N; k++) ctx.lineTo(lx[k], ly[k]);
  for (let k = TRAIL_N - 1; k >= 0; k--) ctx.lineTo(rx[k], ry[k]);
  ctx.closePath();
  ctx.fill();
}

/** Head scale: leaves at 60% with a spring, shrinks over the last few percent. */
function headScale(f: Flight, lt: number): number {
  const u = (lt - f.e) / (f.a - f.e);
  if (u < 0 || u >= 1) return 0;
  const out = lerp(0.6, 1, spring(lt - f.e, 5, 0.5, 6));
  const into = 1 - swiftIn(progress(f.inbound ? 0.92 : 0.93, 1, u));
  return Math.max(0, out * into);
}

/** Whether a flight is passing through the slit right now. */
function inSlit(f: Flight, lt: number): boolean {
  const u = (lt - f.e) / (f.a - f.e);
  return f.inbound ? u > 1 - f.slit : u < f.slit;
}

function drawGlows(ctx: CanvasRenderingContext2D, live: Flight[], lt: number, dm: XY, color: string) {
  for (const f of live) {
    const s = headScale(f, lt);
    if (s <= 0) continue;
    const p = flightAt(f, lt, dm);
    // A bright launch spark for the first few frames of each flight.
    const spark = 1 - progress(0, 0.07, lt - f.e);
    glow(ctx, p.x, p.y, f.size * (2.4 + 2.6 * spark) * s, color, 0.35 + 0.45 * spark);
  }
}

function drawHeads(
  ctx: CanvasRenderingContext2D,
  live: Flight[],
  lt: number,
  dm: XY,
  shades: string[],
  color: string,
) {
  if (!live.length) return;
  ctx.save();
  ctx.globalCompositeOperation = "lighter";
  for (const f of live) drawTrail(ctx, f, lt, dm, color, f.size * 0.34);
  ctx.restore();
  for (const f of live) {
    const s = headScale(f, lt);
    if (s <= 0) continue;
    const p = flightAt(f, lt, dm);
    const turn = (lt - f.e) * f.spin;
    drawCube(ctx, p.x, p.y, f.size * s, f.phase + turn * TAU, f.phase * 1.7 + turn * 3.3, shades);
  }
}

function drawFlights(
  ctx: CanvasRenderingContext2D,
  flights: Flight[],
  lt: number,
  dm: XY,
  shades: string[],
  color: string,
  clipLid: () => void,
) {
  const open: Flight[] = [];
  const lid: Flight[] = [];
  for (const f of flights) {
    if (lt < f.e || lt > f.a + TRAIL_N * TRAIL_DT) continue;
    (inSlit(f, lt) ? lid : open).push(f);
  }
  drawGlows(ctx, open, lt, dm, color);
  drawGlows(ctx, lid, lt, dm, color);
  drawHeads(ctx, open, lt, dm, shades, color);
  if (lid.length) {
    // Through the slit: hidden by the lid except where the mouth is open.
    ctx.save();
    clipLid();
    drawHeads(ctx, lid, lt, dm, shades, color);
    ctx.restore();
  }
}

function drawTags(ctx: CanvasRenderingContext2D, flights: Flight[], lt: number, dm: XY) {
  const tf = TAG_FONT();
  for (const f of flights) {
    if (!f.tag || lt < f.e || lt > f.a) continue;
    const u = (lt - f.e) / (f.a - f.e);
    const a = progress(TAG_SHOW[0], TAG_SHOW[1], u) * (1 - progress(TAG_SHOW[2], TAG_SHOW[3], u));
    if (a <= 0) continue;
    const pop = spring(lt - f.e - 0.02, 4.5, 0.45, 6);
    const p = flightAt(f, lt, dm);
    const line = layout(ctx, f.tag, tf);
    const w = line.width + 24;
    ctx.save();
    ctx.globalAlpha *= a;
    ctx.translate(p.x + TAG_DX, p.y + TAG_DY);
    ctx.scale(pop, pop);
    roundedRect(ctx, 0, -TAG_H / 2, w, TAG_H, TAG_H / 2);
    ctx.fillStyle = rgba(PALETTE.surface, 0.95);
    ctx.fill();
    ctx.strokeStyle = rgba(PALETTE.amber, 0.9);
    ctx.lineWidth = 1.5;
    ctx.stroke();
    drawText(ctx, f.tag, 12, 6.5, { font: tf, fill: PALETTE.amberBright });
    ctx.restore();
  }
}

// Node icons.

/** Stroke a path revealed from its start to fraction p of its length. */
function revealStroke(ctx: CanvasRenderingContext2D, length: number, p: number) {
  if (p <= 0) return;
  if (p < 1) ctx.setLineDash([length * p, length + 1]);
  ctx.stroke();
  ctx.setLineDash([]);
}

const rrLength = (w: number, h: number, r: number) => 2 * (w + h) - (8 - TAU) * r;

/**
 * A glowing pen tip at fraction p around a rounded rect's outline, starting
 * where roundedRect starts (top edge, after the corner) and running clockwise.
 */
function penTip(
  ctx: CanvasRenderingContext2D,
  x: number,
  y: number,
  w: number,
  h: number,
  r: number,
  p: number,
  color: string,
) {
  if (p <= 0 || p >= 1) return;
  // Straight-cornered walk; the glow hides the difference at the corners.
  let d = p * 2 * (w + h) + r;
  let qx = x;
  let qy = y;
  if (d < w) qx = x + d;
  else if ((d -= w) < h) (qx = x + w), (qy = y + d);
  else if ((d -= h) < w) (qx = x + w - d), (qy = y + h);
  else if ((d -= w) < h) qy = y + h - d;
  else qx = x + d - h;
  const fade = 1 - p * p;
  glow(ctx, qx, qy, 40, color, 0.9 * fade);
  ctx.fillStyle = rgba(PALETTE.paper, fade);
  ctx.beginPath();
  ctx.arc(qx, qy, 3, 0, TAU);
  ctx.fill();
}

interface LogLine {
  t: number;
  draw: (ctx: CanvasRenderingContext2D, x: number, y: number, age: number) => void;
}

const LOG = () => font(17, 500, MONO);
const LOG_B = () => font(17, 700, MONO);
const LOG_LH = 30;
const LOG_VIS = 4;

function terminal(
  ctx: CanvasRenderingContext2D,
  key: "project" | "worktree",
  lt: number,
  start: number,
  lines: LogLine[],
  accent: string,
  activity: number,
  rest = 0,
) {
  const c = CARDS[key];
  const k = progress(start, start + 0.36, lt);
  if (k <= 0) return;
  const grow = swiftOut(k);
  const x = -TERM_W / 2;
  const y = -TERM_H / 2;
  ctx.save();
  ctx.translate(c.x + c.w / 2, c.y + c.h / 2 + 14 * (1 - grow));
  const sc = lerp(0.9, 1, grow);
  ctx.scale(sc, sc);

  if (activity > 0) glow(ctx, 0, 0, 220, accent, 0.14 * activity);

  roundedRect(ctx, x, y, TERM_W, TERM_H, c.r);
  ctx.fillStyle = rgba(PALETTE.surface, 0.96 * clamp(k * 2.4));
  ctx.fill();
  // A finished job steps back: its chrome and log recede once it is done.
  ctx.globalAlpha *= 1 - 0.55 * rest;
  ctx.strokeStyle = mix(PALETTE.divider, accent, 0.25 + 0.75 * activity);
  ctx.lineWidth = 2;
  const tp = outCubic(progress(start, start + 0.3, lt));
  revealStroke(ctx, rrLength(TERM_W, TERM_H, c.r), tp);
  penTip(ctx, x, y, TERM_W, TERM_H, c.r, tp, accent);

  // Title bar: a hairline and three dots.
  const hp = swiftOut(progress(start + 0.08, start + 0.3, lt));
  if (hp > 0) {
    ctx.strokeStyle = rgba(PALETTE.divider, 1);
    ctx.lineWidth = 1.5;
    ctx.beginPath();
    ctx.moveTo(x + 1, y + BAR_H);
    ctx.lineTo(x + 1 + (TERM_W - 2) * hp, y + BAR_H);
    ctx.stroke();
  }
  const dots = [PALETTE.amber, PALETTE.teal, PALETTE.divider];
  dots.forEach((col, i) => {
    const s = spring(lt - start - 0.12 - i * 0.035, 5, 0.45);
    if (s <= 0) return;
    ctx.fillStyle = col;
    ctx.beginPath();
    ctx.arc(x + 20 + i * 18, y + BAR_H / 2, 5.5 * s, 0, TAU);
    ctx.fill();
  });

  // Body: the log, scrolled so the newest line sits at the bottom.
  const top = y + BAR_H + 38;
  ctx.save();
  ctx.beginPath();
  ctx.rect(x + 4, y + BAR_H + 3, TERM_W - 8, TERM_H - BAR_H - 7);
  ctx.clip();
  let scroll = 0;
  for (let i = LOG_VIS; i < lines.length; i++) {
    scroll += swiftOut(progress(lines[i].t, lines[i].t + 0.06, lt));
  }
  for (let i = 0; i < lines.length; i++) {
    const L = lines[i];
    if (lt < L.t) break;
    const ly = top + (i - scroll) * LOG_LH;
    if (ly < top - LOG_LH || ly > top + LOG_VIS * LOG_LH) continue;
    ctx.save();
    ctx.globalAlpha *= clamp((ly - top + LOG_LH * 0.8) / (LOG_LH * 0.8));
    L.draw(ctx, x + 16, ly, lt - L.t);
    ctx.restore();
  }
  ctx.restore();
  ctx.restore();
}

function promptLine(t: number, dur: number, caretUntil: number): LogLine {
  const cmd = "cargo build";
  return {
    t,
    draw(ctx, x, y, age) {
      drawText(ctx, "$", x, y, { font: LOG_B(), fill: PALETTE.amber });
      const n = Math.floor(clamp(age / dur) * cmd.length);
      const line = drawText(ctx, cmd.slice(0, n), x + 20, y, { font: LOG(), fill: PALETTE.text1 });
      // Block caret: solid while typing, then blinking until output starts.
      const now = t + age;
      if (now < caretUntil && (age < dur || Math.floor(now / (BEAT / 2)) % 2 === 0)) {
        ctx.fillStyle = rgba(PALETTE.text1, 0.85);
        ctx.fillRect(x + 22 + line.width, y - 14, 9, 18);
      }
    },
  };
}

function compileLine(t: number, name: string): LogLine {
  return {
    t,
    draw(ctx, x, y, age) {
      ctx.save();
      ctx.globalAlpha *= clamp(age / 0.03);
      drawText(ctx, "Compiling", x, y, { font: LOG_B(), fill: PALETTE.amber });
      drawText(ctx, name, x + 104, y, { font: LOG(), fill: PALETTE.text2 });
      ctx.restore();
    },
  };
}

function checkMark(
  ctx: CanvasRenderingContext2D,
  x: number,
  y: number,
  s: number,
  p: number,
  color: string,
  w = 2,
) {
  if (p <= 0) return;
  ctx.save();
  ctx.strokeStyle = color;
  ctx.lineWidth = w;
  ctx.lineCap = "round";
  ctx.lineJoin = "round";
  ctx.beginPath();
  ctx.moveTo(x - 0.5 * s, y);
  ctx.lineTo(x - 0.15 * s, y + 0.35 * s);
  ctx.lineTo(x + 0.5 * s, y - 0.4 * s);
  revealStroke(ctx, s * 1.45, p);
  ctx.restore();
}

function hitLine(t: number, name: string): LogLine {
  return {
    t,
    draw(ctx, x, y, age) {
      ctx.save();
      ctx.globalAlpha *= clamp(age / 0.03);
      checkMark(ctx, x + 7, y - 6, 12, clamp(age / 0.05), PALETTE.green, 2.4);
      drawText(ctx, "hit", x + 24, y, { font: LOG_B(), fill: PALETTE.green });
      drawText(ctx, name, x + 66, y, { font: LOG(), fill: PALETTE.text2 });
      ctx.restore();
    },
  };
}

function finishedLine(t: number): LogLine {
  return {
    t,
    draw(ctx, x, y, age) {
      ctx.save();
      ctx.globalAlpha *= clamp(age / 0.03);
      drawText(ctx, "Finished", x, y, { font: LOG_B(), fill: PALETTE.green });
      ctx.restore();
    },
  };
}

let projectLog: LogLine[] | null = null;
let worktreeLog: LogLine[] | null = null;
function logs(): { project: LogLine[]; worktree: LogLine[] } {
  if (projectLog && worktreeLog) return { project: projectLog, worktree: worktreeLog };
  const p = getPlan();
  projectLog = [promptLine(0.05, 0.16, T_POP), ...p.compiled.map(([t, n]) => compileLine(t, n))];
  projectLog.push(finishedLine(T_FINISHED));
  // The worktree hits what the project just compiled, in the same order and
  // aligned to the end, so both logs rest on the same crates.
  const order = p.compiled.map(([, n]) => n);
  const arr = p.arrivals.worktree;
  const hits = Math.ceil(arr.length / 4);
  worktreeLog = [promptLine(0.11, 0.16, arr[0])];
  for (let k = 0; k < hits; k++) {
    const name = order[order.length - hits + k] ?? TAG_NAMES[k % TAG_NAMES.length];
    worktreeLog.push(hitLine(arr[k * 4], name));
  }
  worktreeLog.push(finishedLine(T_DONE));
  return { project: projectLog, worktree: worktreeLog };
}

function runner(
  ctx: CanvasRenderingContext2D,
  lt: number,
  start: number,
  activity: number,
  firstHit: number,
) {
  if (lt < start) return;
  const c = CARDS.ci;
  ctx.save();
  if (activity > 0) glow(ctx, c.x + c.w / 2, c.y + c.h / 2, 210, PALETTE.green, 0.14 * activity);
  for (let u = 0; u < 2; u++) {
    const s0 = start + u * 0.07;
    const k = progress(s0, s0 + 0.34, lt);
    if (k <= 0) continue;
    const grow = swiftOut(k);
    const x = c.x;
    const y = c.y + u * (RACK_U + RACK_GAP) + 14 * (1 - grow);
    const mid = y + RACK_U / 2;
    roundedRect(ctx, x, y, RACK_W, RACK_U, c.r);
    ctx.fillStyle = rgba(PALETTE.surface, 0.96 * clamp(k * 2.4));
    ctx.fill();
    ctx.strokeStyle = mix(PALETTE.divider, PALETTE.green, 0.25 + 0.75 * activity);
    ctx.lineWidth = 2;
    const tp = outCubic(progress(s0, s0 + 0.3, lt));
    revealStroke(ctx, rrLength(RACK_W, RACK_U, c.r), tp);
    penTip(ctx, x, y, RACK_W, RACK_U, c.r, tp, PALETTE.tealLight);
    // Vent slots on the right.
    const vp = swiftOut(progress(s0 + 0.1, s0 + 0.3, lt));
    ctx.strokeStyle = rgba(PALETTE.divider, 1);
    ctx.lineWidth = 3.5;
    ctx.lineCap = "round";
    for (let i = 0; i < 4; i++) {
      const vx = x + RACK_W - 32 - i * 16;
      ctx.beginPath();
      ctx.moveTo(vx, mid - 13 * vp);
      ctx.lineTo(vx, mid + 13 * vp);
      ctx.stroke();
    }
    const ls = spring(lt - s0 - 0.16, 5, 0.45);
    if (ls <= 0) continue;
    if (u === 0) {
      // Status light: pending amber blink, green from the first hit on.
      const hit = lt >= firstHit;
      const on = hit ? 1 : 0.35 + 0.65 * (0.5 + 0.5 * Math.cos(((lt - s0) / BEAT) * TAU));
      const col = hit ? PALETTE.green : PALETTE.amber;
      ctx.fillStyle = rgba(col, on);
      ctx.beginPath();
      ctx.arc(x + 30, mid, 8.5 * ls, 0, TAU);
      ctx.fill();
      glow(ctx, x + 30, mid, 34 * ls, col, 0.5 * on);
      drawText(ctx, "runner", x + 54, mid + 6, {
        font: LOG(),
        fill: rgba(PALETTE.text3, clamp((lt - s0 - 0.18) / 0.1)),
      });
    } else {
      // Activity lights flicker as artifacts land.
      const fr = Math.floor(lt * 30);
      for (let i = 0; i < 8; i++) {
        const lit = activity > 0.05 && hash(fr * 7 + i, 91) < 0.3 + 0.7 * activity;
        ctx.fillStyle = lit ? PALETTE.green : rgba(PALETTE.divider, 1);
        ctx.fillRect(x + 24 + i * 17, mid - 4.5 * ls, 9, 9 * ls);
      }
    }
  }
  ctx.restore();
}

// Counters under the receiving nodes: the scene's one fact, set large.

const CNT_H = 52;
/** Pill center below the node label's baseline. */
const CNT_DY = 40;
const CNT_DIGITS = () => font(40, 700, MONO);
const CNT_TOTAL = () => font(26, 500, MONO);
const CNT_UNIT = () => font(19, 500, MONO);

/**
 * The fraction of lookups counted at `lt`. One monotone curve from the first
 * arrival to the lock, so the odometer never holds or jumps: quick while the
 * first volley pours in, then easing down into the final count.
 */
function countDone(first: number, lt: number): number {
  const p = progress(first, T_DONE, lt);
  return 1 - (1 - p) ** 1.9;
}

function counter(
  ctx: CanvasRenderingContext2D,
  key: Receiver,
  lt: number,
  facts: ReelFacts | null,
  done: number,
) {
  const n = NODES[key];
  const s = spring(lt - T_SEND - (key === "ci" ? 0.03 : 0), 4, 0.5);
  if (s <= 0) return;
  const warm = facts?.warm ?? null;
  const locked = lt >= T_DONE;
  const h = CNT_H;
  const cy = n.y + NODE_LABEL.dy + CNT_DY;
  const fin = pulse(lt, T_DONE, 0.008, 0.1);
  const bump = 1 + 0.08 * fin;
  // On the lock the count flashes paper for a few frames, then settles green.
  const flip = locked ? 1 - smoothstep(T_DONE + 0.06, T_DONE + 0.16, lt) : 0;
  const ink = mix(PALETTE.green, PALETTE.paper, flip);

  // Hierarchy: the count is the hero, the total reads second, the unit is a
  // small label. Mono digits keep every width fixed while it rolls.
  const fD = CNT_DIGITS();
  const fT = CNT_TOTAL();
  const fU = CNT_UNIT();
  let digits = "";
  let dim = 0;
  let total = "";
  const unit = warm ? "hits" : "cache hits";
  if (warm) {
    const places = String(warm.lookups).length;
    const count = locked ? warm.hits : Math.min(warm.hits, Math.floor(warm.hits * done));
    const raw = String(count);
    digits = raw.padStart(places, "0");
    dim = count === 0 ? places : places - raw.length;
    total = `/${warm.lookups}`;
  }
  const wD = digits ? layout(ctx, digits, fD).width : 0;
  const wT = total ? layout(ctx, total, fT).width : 0;
  const wU = layout(ctx, unit, fU, 1).width;
  const inner = wD + (total ? 3 + wT : 0) + (digits ? 14 : 0) + wU;
  const w = 60 + inner + 28;

  if (fin > 0.01) glow(ctx, n.x, cy, 120, PALETTE.green, 0.26 * fin);
  ctx.save();
  ctx.translate(n.x, cy);
  ctx.scale(s * bump, s * bump);
  roundedRect(ctx, -w / 2, -h / 2, w, h, h / 2);
  ctx.fillStyle = rgba(PALETTE.surface, 0.96);
  ctx.fill();
  ctx.strokeStyle = rgba(PALETTE.divider, 1);
  ctx.lineWidth = 2;
  ctx.stroke();
  // Progress fills the pill's outline around from the top.
  ctx.strokeStyle = mix(PALETTE.green, "#ffffff", fin * 0.6);
  ctx.lineWidth = 2.5;
  roundedRect(ctx, -w / 2, -h / 2, w, h, h / 2);
  revealStroke(ctx, rrLength(w, h, h / 2), done);
  // A dot that becomes a check when every lookup has hit.
  const ck = progress(T_DONE - 0.006, T_DONE + 0.08, lt);
  const x0 = -w / 2;
  if (ck <= 0) {
    ctx.fillStyle = PALETTE.green;
    ctx.beginPath();
    ctx.arc(x0 + 32, 0, 7, 0, TAU);
    ctx.fill();
  } else {
    checkMark(ctx, x0 + 32, 1, 22, swiftOut(ck), ink, 4);
  }
  const base = 14;
  let x = x0 + 60;
  if (digits) {
    drawText(ctx, digits, x, base, {
      font: fD,
      fill: ink,
      glyph: (_g, i) => (i < dim ? { fill: rgba(PALETTE.green, 0.3) } : {}),
    });
    x += wD + 3;
    drawText(ctx, total, x, base, { font: fT, fill: PALETTE.text2 });
    x += wT + 14;
  }
  drawText(ctx, unit, x, base, { font: fU, tracking: 1, fill: PALETTE.text3 });
  ctx.restore();
}

/** An icon's outline echoing outward, like a ring shaped to the card. */
function ripple(ctx: CanvasRenderingContext2D, c: Card, p: number) {
  if (p <= 0 || p >= 1) return;
  const e = outCubic(p);
  const g = 6 + 28 * e;
  ctx.save();
  roundedRect(ctx, c.x - g, c.y - g, c.w + g * 2, c.h + g * 2, c.r + g);
  ctx.strokeStyle = rgba(PALETTE.green, (1 - p) ** 1.6);
  ctx.lineWidth = 4 * (1 - p) + 1;
  ctx.stroke();
  ctx.restore();
}

// The whole frame's content, drawn once or into the whip's streak buffer.

let restCenter: XY | null = null;

function content(ctx: CanvasRenderingContext2D, lt: number, facts: ReelFacts | null) {
  const p = getPlan();
  const st = boxState(lt);
  const view = boxView(st);
  const pose = heroPose(st);
  const frame = boxFrame(pose);
  const lid = panelMatrix(view, frame, "top");
  const lidC = mp(lid, 0.5, 0.5);
  restCenter ??= mp(restLid(), 0.5, 0.5);
  const dm = { x: lidC.x - restCenter.x, y: lidC.y - restCenter.y };
  const open = slitOpen(lt);
  const lit = slitLight(lt);

  gather(ctx, lt);

  // Nodes.
  const act = (key: Receiver) => {
    let a = 0;
    for (const t of p.arrivals[key]) a += pulse(lt, t, 0.004, 0.06);
    return clamp(a * 0.3 + pulse(lt, T_DONE, 0.01, 0.15));
  };
  let emit = 0;
  for (const f of p.inbound) emit += pulse(lt, f.e, 0.004, 0.05);
  const { project: plog, worktree: wlog } = logs();
  // Once the project has finished, it recedes and hands focus to the receivers.
  const rest = swiftOut(progress(T_FINISHED + 0.08, T_FINISHED + 0.33, lt));
  terminal(ctx, "project", lt, 0, plog, PALETTE.amber, clamp(emit * 0.12), rest);
  terminal(ctx, "worktree", lt, 0.06, wlog, PALETTE.green, act("worktree"));
  runner(ctx, lt, 0.12, act("ci"), p.arrivals.ci[0]);
  drawNodeLabel(ctx, "project", 1 - 0.4 * rest);
  drawNodeLabel(ctx, "worktree");
  drawNodeLabel(ctx, "ci");

  // Counters start rolling on the first arrival and land on the last.
  for (const key of ["worktree", "ci"] as const) {
    counter(ctx, key, lt, facts, countDone(p.arrivals[key][0], lt));
  }

  // Behind the box: the pop, the send's light, and its floor shockwaves.
  popBehind(ctx, lt);
  popDebris(ctx, lt);
  if (lt >= T_SEND - 0.01) {
    glow(ctx, lidC.x, lidC.y, 300, PALETTE.green, 0.75 * pulse(lt, T_SEND, 0.004, 0.09));
    for (const b of BURSTS.slice(1)) {
      glow(ctx, lidC.x, lidC.y, 190, PALETTE.green, 0.45 * pulse(lt, b, 0.004, 0.07));
    }
  }
  if (st.build > 0) {
    glow(ctx, lidC.x, lidC.y, 150 + 130 * st.build, PALETTE.green, 0.6 * st.build ** 0.7);
  }

  if (st.s >= 0.01) {
    const floor = view.planeMatrix(pose.pos, [1, 0, 0], [0, 0, 1]);
    // Seam light spills onto the floor as he charges; each send kicks a ring
    // out along the ground from under him.
    floorGlow(ctx, floor, 1.5 + 0.3 * st.build, PALETTE.green, 0.32 * st.build ** 1.5);
    floorGlow(ctx, floor, 2.2, PALETTE.green, 0.5 * pulse(lt, T_SEND, 0.004, 0.12));
    floorRing(ctx, floor, 0.9, 2.45, progress(T_SEND, T_SEND + 0.34, lt), 0.07, 1);
    floorRing(ctx, floor, 0.86, 2.0, progress(T_SEND + 0.02, T_SEND + 0.26, lt), 0.025, 0.8, PALETTE.paper);
    for (const b of BURSTS.slice(1)) {
      floorGlow(ctx, floor, 1.7, PALETTE.green, 0.3 * pulse(lt, b, 0.004, 0.08));
      floorRing(ctx, floor, 0.9, 1.9, progress(b, b + 0.26, lt), 0.05, 0.8);
    }
    drawShadow(ctx, view, pose.pos, 1, 0, 0.5);
    drawBox(ctx, view, pose);
  }

  // On the lid: the mouth, green light leaking from the seams, small flashes.
  const slit = slitPoly(lid, Math.max(open, 0));
  if (st.s > 0.9) drawSlit(ctx, lid, slit, open, lit);
  if (st.build > 0) {
    const b = st.build ** 0.6;
    ctx.save();
    ctx.globalCompositeOperation = "lighter";
    ctx.lineCap = "round";
    ctx.lineJoin = "round";
    ctx.strokeStyle = rgba(PALETTE.green, 0.9 * b);
    ctx.lineWidth = 3.5;
    const c0 = mp(lid, 0, 1);
    const c1 = mp(lid, 1, 1);
    const c2 = mp(lid, 1, 0);
    ctx.beginPath();
    ctx.moveTo(c0.x, c0.y);
    ctx.lineTo(c1.x, c1.y);
    ctx.lineTo(c2.x, c2.y);
    ctx.stroke();
    ctx.restore();
    for (const [u, v] of [
      [0.5, 1],
      [1, 1],
      [1, 0.5],
    ] as const) {
      const q = mp(lid, u, v);
      glow(ctx, q.x, q.y, 56 * b, PALETTE.green, 0.6 * b);
    }
  }
  if (lit > 0 && open > 0.05) glow(ctx, lidC.x, lidC.y, 30 + 30 * open, PALETTE.green, 0.5 * lit);
  // A small lid-local flash on each volley; nothing larger ever sits on the box.
  for (const b of BURSTS) {
    const k = (b === T_SEND ? 0.85 : 0.5) * pulse(lt, b, 0.004, 0.06);
    glow(ctx, lidC.x, lidC.y, 44, "#eaffdd", k);
  }
  // The swallow: a small absorb spark as each packet drops through.
  for (const g of GULPS) {
    glow(ctx, lidC.x, lidC.y + 4, 40, PALETTE.amberBright, 0.55 * pulse(lt, g, 0.004, 0.045));
  }

  // Flights, which disappear into the cards they enter.
  const lidPts = lidPoly(lid);
  const clipLid = () => {
    ctx.beginPath();
    ctx.rect(-W, -H, 3 * W, 3 * H);
    polyPath(ctx, lidPts);
    polyPath(ctx, slit);
    ctx.clip("evenodd");
  };
  ctx.save();
  ctx.beginPath();
  ctx.rect(-W, -H, 3 * W, 3 * H);
  for (const c of Object.values(CARDS)) rrPath(ctx, c);
  ctx.clip("evenodd");
  drawFlights(ctx, p.inbound, lt, dm, amberRamp(), PALETTE.amberBright, clipLid);
  drawFlights(ctx, p.outbound, lt, dm, greenRamp(), PALETTE.green, clipLid);
  ctx.restore();
  drawTags(ctx, p.inbound, lt, dm);

  // Each crate leaves the project, and each artifact enters its node, with a
  // small flash on the card's edge.
  for (const f of p.inbound) {
    const k = pulse(lt, f.e, 0.003, 0.04);
    if (k > 0.02) glow(ctx, f.x0 + 8, f.y0, 26, PALETTE.amberBright, 0.5 * k);
  }
  for (const f of p.outbound) {
    const k = pulse(lt, f.a, 0.003, 0.05);
    if (k > 0.02) glow(ctx, f.x2 - 6, f.y2, 34, PALETTE.green, 0.7 * k);
  }

  // Finish: both receivers ripple out together.
  const rp = progress(T_DONE, T_DONE + 0.42, lt);
  ripple(ctx, CARDS.worktree, rp);
  ripple(ctx, CARDS.ci, rp);
}

// Whip. The content is drawn once into a buffer squeezed `k` times
// horizontally, which is a cheap k px box blur along the motion once it is
// stretched back, and the smear lays two blurred copies across one frame of
// travel: a continuous streak, never a strobe of sharp copies.
let streakBuf: HTMLCanvasElement | null = null;

function drawStreak(
  ctx: CanvasRenderingContext2D,
  draw: (c: CanvasRenderingContext2D) => void,
  x: number,
  v: number,
) {
  const k = clamp(Math.abs(v) * 0.5, 8, 200);
  streakBuf ??= makeCanvas(W / 8, H);
  const g = streakBuf.getContext("2d")!;
  g.setTransform(1, 0, 0, 1, 0, 0);
  g.clearRect(0, 0, streakBuf.width, streakBuf.height);
  g.setTransform(1 / k, 0, 0, 1, 0, 0);
  draw(g);
  g.setTransform(1, 0, 0, 1, 0, 0);
  const buf = streakBuf;
  smear(ctx, x, v, () => ctx.drawImage(buf, 0, 0, W / k, H, 0, 0, W, H), 2);
}

/**
 * Content offset for the whip: a small wind-up right, then out to the left.
 * The travel is short enough that the receivers are still in shot on the
 * frame before last and smear across the left edge on the last one, so the
 * pan never shows an empty frame before the cut.
 */
function whipX(lt: number): number {
  const wind = 26 * swiftOut(progress(T_DONE + 0.01, T_WHIP, lt));
  const p = progress(T_WHIP, LAST, lt);
  return wind - (wind + 1950) * p ** 1.7;
}

// Speed lines: streaks fixed in the world at a few depths, so they pan with
// the content (faster in front), stretch with its speed, and wrap around off
// screen. They peak on the last frame, where scene 5's own streaks pick up
// the same vocabulary: a hot head on the left and a tail trailing right.
interface Streak {
  y: number;
  x: number;
  par: number;
  len: number;
  w: number;
  color: string;
  a: number;
}
/** Wrap span for streak heads: wider than the frame plus the longest streak. */
const STREAK_SPAN = W + 1900;
let streakField: Streak[] | null = null;
function streaks(): Streak[] {
  if (streakField) return streakField;
  const colors = [PALETTE.paper, PALETTE.amber, PALETTE.green, PALETTE.amberBright, PALETTE.tealLight];
  streakField = [];
  for (let i = 0; i < 40; i++) {
    const fat = hash(i, 61) < 0.2;
    streakField.push({
      y: 140 + hash(i, 41) * 800,
      x: hash(i, 47) * STREAK_SPAN,
      par: 0.7 + hash(i, 53) * 0.7,
      len: (fat ? 480 : 260) + hash(i, 43) * 620,
      w: fat ? 10 + hash(i, 59) * 12 : 1.5 + hash(i, 59) * 3,
      color: colors[Math.floor(hash(i, 67) * colors.length)],
      a: fat ? 0.22 + hash(i, 71) * 0.16 : 0.45 + hash(i, 71) * 0.25,
    });
  }
  return streakField;
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

/**
 * [y, height, color, alpha, parallax, head x on the last frame]: broad soft
 * bands in the colors that just left, set so they cross the frame on the
 * last frame and give it the mass of scene 5's incoming bands.
 */
const BANDS: readonly (readonly [number, number, string, number, number, number])[] = [
  [300, 44, PALETTE.green, 0.32, 1.15, 260],
  [548, 56, PALETTE.amber, 0.4, 0.9, 520],
  [770, 44, PALETTE.green, 0.3, 1.25, 90],
  [196, 60, PALETTE.amberDeep, 0.18, 0.8, 900],
  [900, 54, PALETTE.teal, 0.2, 1.05, 700],
];

function speedLines(ctx: CanvasRenderingContext2D, lt: number, x: number, v: number) {
  const wp = progress(T_WHIP, LAST, lt);
  if (wp <= 0) return;
  // Full strength on the last frame, not faded out before the cut.
  const env = Math.sin((Math.PI / 2) * wp);
  const speed = clamp(Math.abs(v) / 430);
  const margin = STREAK_SPAN - W;
  const xEnd = whipX(LAST);
  ctx.save();
  ctx.globalCompositeOperation = "lighter";
  for (const [y, h, color, a, par, head] of BANDS) {
    const hx = head + par * (x - xEnd);
    const len = 1500 * speed * par;
    if (hx > W || hx + len < 0) continue;
    ctx.globalAlpha = a * env;
    ctx.drawImage(streakSprite(color), hx, y - h / 2, len, h);
  }
  for (const s of streaks()) {
    const len = s.len * s.par * (0.2 + 0.8 * speed);
    const u = (s.x + s.par * x) % STREAK_SPAN;
    const hx = (u < 0 ? u + STREAK_SPAN : u) - margin;
    if (hx > W || hx + len < 0) continue;
    ctx.globalAlpha = s.a * env;
    ctx.drawImage(streakSprite(s.color), hx, s.y - s.w / 2, len, s.w);
  }
  ctx.restore();
}

export const scene: Scene = {
  id: "flow",
  start: bar(3),
  end: bar(4),
  draw(ctx, lt, env) {
    ctx.fillStyle = PALETTE.bg;
    ctx.fillRect(0, 0, env.W, env.H);

    // A slow push toward the box, plus impact shake on the pop and the send.
    const push = 1 + 0.03 * inOutSine(progress(T_POP, T_WHIP, lt));
    const [sx1, sy1] = shake(lt, T_POP, 5, 0.07);
    const [sx2, sy2] = shake(lt, T_SEND, 5, 0.06);
    const draw = (c: CanvasRenderingContext2D) => {
      c.translate(sx1 + sx2, sy1 + sy2);
      c.translate(BOX_X, BOX_Y);
      c.scale(push, push);
      c.translate(-BOX_X, -BOX_Y);
      content(c, lt, env.facts);
    };
    const x = whipX(lt);
    const v = x - whipX(lt - 1 / 60);
    ctx.save();
    // Below ~16 px a frame the motion reads sharp; above it, it streaks.
    if (lt < T_WHIP || Math.abs(v) < 16) {
      ctx.translate(x, 0);
      draw(ctx);
    } else {
      drawStreak(ctx, draw, x, v);
    }
    ctx.restore();

    // Speed lines sell the whip, at full strength into the cut.
    speedLines(ctx, lt, x, v);
  },
};
