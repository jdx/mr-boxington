// Scene 3, "Kinetic type". The camera dives through Mr Boxington's monocle;
// his pupil dilates into an iris that opens on a type world, and the eye's
// highlight becomes the caret that types the hero line. The line builds into
// a justified lockup with one treatment per word, then everything but the
// three node names falls away and the names fly to the flow scene's labels.

import {
  beat,
  drawNodeLabel,
  drawStagedBox,
  H,
  H2_POSE,
  HERO_CAM,
  NODE_LABEL,
  NODES,
  PALETTE,
  type Scene,
  sec,
  W,
} from "../bible";
import { boxFrame, CREAM, faceToScreen, INK, panelMatrix } from "../box";
import { mix, rgba } from "../color";
import { flash, glow, ring, roundedRect, shake } from "../fx";
import {
  clamp,
  cubicBezier,
  DEG,
  hash,
  inQuad,
  lerp,
  outBack,
  outCubic,
  progress,
  pulse,
  smoothstep,
  swiftInOut,
  swiftOut,
  TAU,
  wobble,
} from "../math";
import { add, applyMatrix, type Camera, mul, type V3, View } from "../space";
import { font, layout, MONO } from "../type";

const S = sec("type");

// Beat map, local seconds. The score (score/type.ts) is written to these.
export const T_IRIS = beat(0.5); // the iris has filled the frame; the first key lands
const T_TYPED = beat(0.9); // hero line fully typed
export const T_P = beat(1); // "projects," lands
export const T_MAIN = beat(1.25); // main draws out under it, commit to commit
export const T_BRANCH = beat(1.75); // a git branch leaves the main line...
export const T_W = beat(2); // ...and lands on its commit: "worktrees," locks
export const T_AND = beat(2.25); // "and" glides in on the sixteenth pickup
export const T_CI = beat(2.5); // "CI." stamps in giant
export const T_SNAP = T_CI + 0.05; // ...holds three frames, then snaps down
export const T_OUT = beat(3); // breakup: extras fall, names fly
/** Main's second commit pops just before the branch forks. */
export const MAIN_TIP = T_BRANCH - 0.03;

/** Global time of the finished lockup's last frame before the breakup; the reel's poster. */
export const LOCKUP = S.start + T_OUT - 0.06;
// The names reach their labels a frame before the section ends, as the
// score's name whooshes peak: from here on the frame is exactly handoff 3 -> 4.
export const T_LAND = S.len - 0.0175;

export const HERO = "Reuse matching compilation work across";
const WL = 940; // lockup width; every display line is justified to it
const LX = (W - WL) / 2;
const EM = NODE_LABEL.size;
// Display glyphs are always drawn at the label's own font and scaled, so a
// word in flight has the same outlines as the label it lands as.
const SPEC = font(EM, NODE_LABEL.weight);
const SPEC_LIGHT = font(EM, 300);
const SPEC_MONO = font(28, 500, MONO);
const TRACK = -0.04 * EM;
const Z0 = 2; // camera zoom on the typed line
const Z1 = 1.12; // ...with the first display line
const Z2 = 1.05; // ...with two

interface Glyph {
  ch: string;
  /** World x of the glyph's center. */
  x: number;
  /** Advance at the spec's own size. */
  w: number;
}

interface Lockup {
  k1: number;
  k2: number;
  k3: number;
  ym: number;
  y1: number;
  y2: number;
  y3: number;
  mono: Glyph[];
  monoW: number;
  proj: Glyph[];
  work: Glyph[];
  and: Glyph[];
  ci: Glyph[];
  /** Camera centers while the lockup is still building. */
  c1: number;
  c2: number;
}

function row(
  ctx: CanvasRenderingContext2D,
  text: string,
  spec: string,
  tracking: number,
  x0: number,
  k: number,
) {
  const line = layout(ctx, text, spec, tracking);
  return {
    glyphs: line.glyphs.map((g) => ({ ch: g.ch, x: x0 + (g.x + g.w / 2) * k, w: g.w })),
    width: line.width * k,
  };
}

// Rebuilt every frame from type.ts's cache, which is reset once web fonts
// load, so the lockup never keeps fallback-font metrics.
function lockup(ctx: CanvasRenderingContext2D): Lockup {
  const width = (s: string) => layout(ctx, s, SPEC, TRACK).width;
  const k1 = WL / width("projects,");
  const k2 = WL / width("worktrees,");
  const k3 = WL / width("and CI.");
  // Baselines from the font's ascender (0.72 em) and descender (0.21 em).
  const ym = 20;
  const y1 = ym + 44 + 0.72 * EM * k1;
  const y2 = y1 + 0.21 * EM * k1 + 12 + 0.72 * EM * k2;
  const y3 = y2 + 58 + 0.7 * EM * k3;
  const dy = 532 - y3 / 2;
  const ciRow = row(ctx, "CI.", SPEC, TRACK, 0, k3);
  const mono = row(ctx, HERO, SPEC_MONO, 0, LX, 1);
  const lk: Lockup = {
    k1,
    k2,
    k3,
    ym: ym + dy,
    y1: y1 + dy,
    y2: y2 + dy,
    y3: y3 + dy,
    mono: mono.glyphs,
    monoW: mono.width,
    proj: row(ctx, "projects,", SPEC, TRACK, LX, k1).glyphs,
    work: row(ctx, "worktrees,", SPEC, TRACK, LX, k2).glyphs,
    and: row(ctx, "and", SPEC_LIGHT, TRACK, LX, k3).glyphs,
    ci: ciRow.glyphs.map((g) => ({ ...g, x: g.x + LX + WL - ciRow.width })),
    c1: 0,
    c2: 0,
  };
  lk.c1 = (lk.ym - 22 + lk.y1 + 0.21 * EM * k1) / 2;
  lk.c2 = (lk.ym - 22 + lk.y2 + 40) / 2;
  return lk;
}

/** The type world's camera: frames the lockup as it grows, shakes on hits. */
interface WorldCam {
  cx: number;
  cy: number;
  z: number;
  ox: number;
  oy: number;
}

/** Inhale before the breakup: builds over three frames, released by the break. */
function swell(lt: number): number {
  return inQuad(progress(T_OUT - 0.055, T_OUT, lt)) * (1 - outCubic(progress(T_OUT, T_OUT + 0.14, lt)));
}

function worldCam(lt: number, lk: Lockup): WorldCam {
  // Close on the caret as the iris opens, pan along the typed line, then
  // pull back as the first word lands and reframe as each line joins.
  const pull = swiftInOut(progress(0.37, T_P - 0.004, lt));
  const a = swiftInOut(progress(0.7, 0.93, lt));
  const b = swiftInOut(progress(1.0, T_CI - 0.005, lt));
  const dive = lerp(0.6, 1, outCubic(progress(0, 0.36, lt)));
  const track = smoothstep(T_IRIS - 0.04, T_TYPED + 0.03, lt);
  const cx = lerp(lerp(LX + 60, LX + lk.monoW / 2, track), W / 2, pull);
  const cy = lerp(lk.ym - 8, lerp(lerp(lk.c1, lk.c2, a), H / 2, b), pull);
  // A slow push keeps the holds alive between hits.
  const frame = lerp(Z0 * dive, lerp(lerp(Z1, Z2, a), 1, b), pull);
  const z = frame * (1 + 0.012 * lt) * (1 + 0.022 * swell(lt));
  const s1 = shake(lt, T_P, 7, 0.06);
  const s2 = shake(lt, T_W, 4, 0.05);
  const s3 = shake(lt, T_CI, 14, 0.07);
  const s4 = shake(lt, T_OUT, 4, 0.04);
  return {
    cx,
    cy,
    z,
    ox: s1[0] + s2[0] + s3[0] + s4[0],
    oy: s1[1] + s2[1] + s3[1] + s4[1],
  };
}
const sx = (c: WorldCam, x: number) => (x - c.cx) * c.z + W / 2 + c.ox;
const sy = (c: WorldCam, y: number) => (y - c.cy) * c.z + H / 2 + c.oy;

interface GlyphOpts {
  sx?: number;
  sy?: number;
  rot?: number;
  alpha?: number;
  /** Knock out what lies behind with a background-colored stroke, `width` screen px across. */
  halo?: { color: string; width: number };
  /**
   * Ink skip: inside the screen band y0..y1, erase `gap` px either side of
   * the glyph, horizontally only, so a rail breaks cleanly around a
   * descender without nicking the bowls that sit just above it.
   */
  skip?: { color: string; y0: number; y1: number; gap: number };
}

/** One glyph with its baseline center at (x, y), scaled by k. */
function glyph(
  ctx: CanvasRenderingContext2D,
  spec: string,
  ch: string,
  w: number,
  x: number,
  y: number,
  k: number,
  fill: string,
  o: GlyphOpts = {},
): void {
  const a = o.alpha ?? 1;
  if (ch === " " || a <= 0.003 || k <= 0) return;
  const kx = k * (o.sx ?? 1);
  const ky = k * (o.sy ?? 1);
  if (o.skip) {
    const s = o.skip;
    ctx.save();
    ctx.beginPath();
    ctx.rect(0, s.y0, W, s.y1 - s.y0);
    ctx.clip();
    ctx.font = spec;
    ctx.fillStyle = s.color;
    ctx.globalAlpha *= a;
    for (let i = -2; i <= 2; i++) {
      ctx.save();
      ctx.translate(x + (s.gap * i) / 2, y);
      if (o.rot) ctx.rotate(o.rot);
      ctx.scale(kx, ky);
      ctx.fillText(ch, -w / 2, 0);
      ctx.restore();
    }
    ctx.restore();
  }
  if (o.halo) {
    ctx.save();
    ctx.translate(x, y);
    if (o.rot) ctx.rotate(o.rot);
    ctx.scale(kx, ky);
    ctx.font = spec;
    ctx.lineJoin = "round";
    ctx.strokeStyle = o.halo.color;
    ctx.lineWidth = o.halo.width / Math.max(kx, ky);
    ctx.globalAlpha *= a;
    ctx.strokeText(ch, -w / 2, 0);
    ctx.restore();
  }
  ctx.save();
  ctx.translate(x, y);
  if (o.rot) ctx.rotate(o.rot);
  ctx.scale(kx, ky);
  ctx.globalAlpha *= a;
  ctx.font = spec;
  ctx.fillStyle = fill;
  ctx.fillText(ch, -w / 2, 0);
  ctx.restore();
}

// Key times for the hero line. The score ticks these, so every visible key
// has its click.
export const KEY_T: readonly number[] = (() => {
  const w: number[] = [];
  for (let i = 0; i < HERO.length; i++) {
    w.push(0.6 + hash(i, 41) * 0.8 + (HERO[i - 1] === " " ? 0.5 : 0));
  }
  const total = w.reduce((s, v) => s + v, 0);
  let acc = 0;
  return w.map((v) => {
    const t = T_IRIS + (acc / total) * (T_TYPED - T_IRIS);
    acc += v;
    return t;
  });
})();

const CARET_W = 15;
const CARET_H = 29;

/** Caret rectangle in screen space (center and size). */
function caretRect(c: WorldCam, lk: Lockup, lt: number) {
  let n = 0;
  while (n < KEY_T.length && lt >= KEY_T[n]) n++;
  const last = lk.mono[Math.max(0, n - 1)];
  const x = n === 0 ? LX + 1 : last.x + last.w / 2 + 3;
  return {
    x: sx(c, x + CARET_W / 2),
    y: sy(c, lk.ym - 21 + CARET_H / 2),
    w: CARET_W * c.z,
    h: CARET_H * c.z,
  };
}

function fillCaret(
  ctx: CanvasRenderingContext2D,
  x: number,
  y: number,
  w: number,
  h: number,
  r: number,
  fill: string,
): void {
  roundedRect(ctx, x - w / 2, y - h / 2, w, h, r);
  ctx.fillStyle = fill;
  ctx.fill();
}

// --- The monocle dive ------------------------------------------------------

const MONOCLE_FACE: [number, number] = [18, -28];
let monocleWorld: V3 | null = null;
function monocleAt(): V3 {
  if (!monocleWorld) {
    const f = boxFrame(H2_POSE);
    const c = add(f.c, f.z);
    const [u, v] = MONOCLE_FACE;
    monocleWorld = add(c, add(mul(f.x, (2 * u) / 104), mul(f.y, (-2 * v) / 128)));
  }
  return monocleWorld;
}

const Z_DIVE = 36;
function diveCam(lt: number): Camera {
  const p = progress(0, T_IRIS, lt);
  const m = monocleAt();
  const m0 = new View(HERO_CAM).project(m);
  const move = cubicBezier(0.4, 0, 0.2, 1)(progress(0, 0.75, p));
  const turn = cubicBezier(0.45, 0, 0.25, 1)(progress(0, 0.95, p));
  // Exponential zoom that keeps accelerating until it punches through.
  const zoom = Z_DIVE ** (p ** 1.7);
  return {
    cx: lerp(m0.x, W / 2, move),
    cy: lerp(m0.y, H / 2, move),
    scale: HERO_CAM.scale * zoom,
    yaw: lerp(HERO_CAM.yaw, 0, turn),
    pitch: lerp(HERO_CAM.pitch, 0, turn),
    roll: -14 * DEG * turn,
    target: m,
  };
}

function drawDive(ctx: CanvasRenderingContext2D, lt: number, lk: Lockup): void {
  const p = progress(0, T_IRIS, lt);
  const cam = diveCam(lt);
  ctx.fillStyle = PALETTE.bg;
  ctx.fillRect(0, 0, W, H);
  drawStagedBox(ctx, cam, H2_POSE);
  if (lt <= 0) return;
  const view = new View(cam);
  const m = panelMatrix(view, boxFrame(H2_POSE), "face");
  const [fx, fy] = MONOCLE_FACE;
  // Trace a circle in the face plane; restoring the transform keeps the path.
  const circle = (r: number) => {
    ctx.save();
    applyMatrix(ctx, m);
    ctx.beginPath();
    ctx.arc(fx, fy, r, 0, TAU);
    ctx.restore();
  };
  // The pupil dilates out to the monocle ring and opens on the type world.
  const rp = lerp(8, 14.4, swiftOut(progress(0.05, 0.8, p)));
  const rim = lerp(0.3, 1.2, smoothstep(0, 0.4, p));
  circle(rp);
  ctx.fillStyle = INK;
  ctx.fill();
  ctx.save();
  circle(rp - rim);
  ctx.clip();
  drawWorld(ctx, lt, lk, mix(INK, PALETTE.bg, smoothstep(0.05, 0.55, p)));
  ctx.restore();

  // The lens glint stays on the glass as we pass through it.
  const shine = 0.9 * (1 - smoothstep(0.5, 0.85, p));
  if (shine > 0) {
    ctx.save();
    applyMatrix(ctx, m);
    ctx.beginPath();
    ctx.arc(fx, fy, 12, 205 * DEG, 240 * DEG);
    ctx.restore();
    ctx.save();
    ctx.lineCap = "round";
    ctx.strokeStyle = rgba(CREAM, shine);
    ctx.lineWidth = (3 * cam.scale) / 104;
    ctx.stroke();
    ctx.restore();
  }

  // The eye's highlight rides the pupil, then peels off to become the caret.
  const q = swiftInOut(progress(0.45, 1, p));
  const hl = faceToScreen(view, H2_POSE, 15.5, -30.5);
  const he = faceToScreen(view, H2_POSE, 18.1, -30.5);
  const hr = Math.min(Math.hypot(he.x - hl.x, he.y - hl.y), 20);
  const car = caretRect(worldCam(lt, lk), lk, lt);
  fillCaret(
    ctx,
    lerp(hl.x, car.x, q),
    lerp(hl.y, car.y, q),
    lerp(hr * 2, car.w, q),
    lerp(hr * 2, car.h, q),
    lerp(hr, 2, q),
    mix(CREAM, PALETTE.amber, q),
  );
}

// --- The type world --------------------------------------------------------

// The rails run a little under the baselines, so the letters stand on them
// and only descenders cross (with an ink skip).
const RAIL_DY = 7;
const BRANCH_W = 3.5;

/**
 * Git graph for "worktrees,": main runs under the "projects," baseline
 * between two commits in the margins; a branch forks off the first, turns
 * down the margin, and runs under the "worktrees," baseline to its own
 * commit on the right.
 */
function branchGeom(lk: Lockup) {
  const ax = LX - 58;
  const ay = lk.y1 + RAIL_DY;
  const by = lk.y2 + RAIL_DY;
  const r = 46;
  const segV = by - r - ay;
  const arc = (r * Math.PI) / 2;
  const x1 = LX + WL + 58;
  const segH = x1 - (ax + r);
  const total = segV + arc + segH;
  const sAt = (x: number) => segV + arc + (x - (ax + r));
  const pt = (s: number): [number, number] => {
    if (s <= segV) return [ax, ay + s];
    if (s <= segV + arc) {
      const a = Math.PI - (s - segV) / r;
      return [ax + r + Math.cos(a) * r, by - r + Math.sin(a) * r];
    }
    return [ax + r + (s - segV - arc), by];
  };
  return { ax, ay, by, x1, total, sAt, pt };
}
type Branch = ReturnType<typeof branchGeom>;

// The head accelerates out of the fork and hits the end commit on the beat.
const HEAD_A = 0.3;
const headEase = (u: number) => HEAD_A * u + (1 - HEAD_A) * u * u;
function branchS(lt: number, g: Branch): number {
  return g.total * headEase(progress(T_BRANCH, T_W, lt));
}
/** Time the branch head passes `s` (inverse of headEase). */
function branchTime(s: number, g: Branch): number {
  const f = clamp(s / g.total);
  const b = HEAD_A;
  const a = 1 - HEAD_A;
  const u = (-b + Math.sqrt(b * b + 4 * a * f)) / (2 * a);
  return lerp(T_BRANCH, T_W, u);
}

/** A git commit: hollow ring, or filled while it flashes. */
function commit(
  ctx: CanvasRenderingContext2D,
  x: number,
  y: number,
  r: number,
  lw: number,
  hot: number,
  bg: string,
  alpha: number,
): void {
  if (r <= 0.1 || alpha <= 0) return;
  ctx.save();
  ctx.globalAlpha *= alpha;
  ctx.beginPath();
  ctx.arc(x, y, r, 0, TAU);
  ctx.fillStyle = hot > 0 ? mix(bg, PALETTE.amberBright, hot) : bg;
  ctx.fill();
  ctx.strokeStyle = hot > 0 ? mix(PALETTE.tealLight, PALETTE.amberBright, hot) : PALETTE.tealLight;
  ctx.lineWidth = lw;
  ctx.stroke();
  ctx.restore();
}

interface Flyer {
  glyphs: Glyph[];
  k: number;
  y: number;
  key: keyof typeof NODES;
  /** Arc height above the chord's midpoint, px (negative is up). */
  lift: number;
  /** Peak lean mid-flight, radians; negative lifts the right end. */
  bank: number;
  /** Launch after T_OUT, seconds. */
  delay: number;
  /** +1 when the word travels right, so its right end leads. */
  dir: number;
}

// The names are under way two frames after the break and keep real speed
// into the last frames: about 90% of the way at 1.75 s and 97% at 1.80 s,
// so they arrive with the score's whooshes at the section's end instead of
// creeping in.
const FLY = cubicBezier(0.3, 0.05, 0.55, 1);
const LAG = 0.0012;
// Motion blur as a 180° shutter: trailing samples over half a frame.
const SHUTTER = 0.5 / 60;
const BLUR_N = 8;

// The names launch on the break, a hair apart, and all land together, each
// on a shallow arc that diverges from the other two.
function flyers(lk: Lockup): Flyer[] {
  return [
    {
      glyphs: lk.proj.slice(0, 7),
      k: lk.k1,
      y: lk.y1,
      key: "project",
      lift: -60,
      bank: -0.16,
      delay: 0,
      dir: -1,
    },
    {
      glyphs: lk.work.slice(0, 8),
      k: lk.k2,
      y: lk.y2,
      key: "worktree",
      lift: -70,
      bank: -0.1,
      delay: 0.008,
      dir: 1,
    },
    {
      glyphs: lk.ci.slice(0, 2),
      k: lk.k3,
      y: lk.y3,
      key: "ci",
      lift: -40,
      bank: -0.08,
      delay: 0.004,
      dir: 1,
    },
  ];
}

const bez = (a: number, b: number, c: number, t: number) =>
  (1 - t) ** 2 * a + 2 * (1 - t) * t * b + t * t * c;

/** A node name in flight: one arc for the word, glyphs spaced along it. */
function drawFlyer(ctx: CanvasRenderingContext2D, c: WorldCam, f: Flyer, lt: number): void {
  const n = NODES[f.key];
  const label = layout(ctx, n.label, SPEC, NODE_LABEL.tracking);
  const lx0 = n.x - label.width / 2;
  const ly = n.y + NODE_LABEL.dy;
  const count = f.glyphs.length;
  const e0 = label.glyphs[0];
  const eN = label.glyphs[count - 1];
  const a0 = (f.glyphs[0].x + f.glyphs[count - 1].x) / 2;
  const a1 = lx0 + (e0.x + e0.w / 2 + eN.x + eN.w / 2) / 2;
  const x0 = sx(c, a0);
  const y0 = sy(c, f.y);
  const cxp = (x0 + a1) / 2;
  const cyp = (y0 + ly) / 2 + f.lift;
  const k0 = f.k * c.z;
  /** Glyph j's baseline center, scale, and lean at time `at`. */
  const place = (j: number, at: number) => {
    const g = f.glyphs[j];
    const order = f.dir > 0 ? count - 1 - j : j;
    const t0 = T_OUT + f.delay + order * LAG;
    const p = FLY(progress(t0, T_LAND, at));
    const k = Math.exp(lerp(Math.log(k0), 0, p));
    const e = label.glyphs[j];
    // Offset from the word's anchor, in label-font units.
    const off = lerp((g.x - a0) / f.k, lx0 + e.x + e.w / 2 - a1, p);
    const rot = f.bank * Math.sin(Math.PI * clamp(p));
    return {
      x: bez(x0, cxp, a1, p) + off * k * Math.cos(rot),
      y: bez(y0, cyp, ly, p) + off * k * Math.sin(rot),
      k,
      rot,
      p,
    };
  };
  const tint = (p: number) =>
    f.key === "ci" ? mix(PALETTE.amber, PALETTE.paper, smoothstep(0.15, 0.85, p)) : PALETTE.paper;
  // Directional motion blur: fainter copies at the shutter's earlier
  // positions, at the current size and lean, behind the word, only while it
  // moves more than a few px a frame.
  const cur = f.glyphs.map((_, j) => place(j, lt));
  const was = place(0, lt - SHUTTER);
  if (Math.hypot(cur[0].x - was.x, cur[0].y - was.y) > 3) {
    for (let s = BLUR_N; s >= 1; s--) {
      const at = lt - (SHUTTER * s) / BLUR_N;
      const a = 0.26 * (1 - s / (BLUR_N + 1));
      f.glyphs.forEach((g, j) => {
        const q = place(j, at);
        const n = cur[j];
        glyph(ctx, SPEC, g.ch, g.w, q.x, q.y, n.k, tint(n.p), { rot: n.rot, alpha: a });
      });
    }
  }
  f.glyphs.forEach((g, j) => {
    const q = cur[j];
    glyph(ctx, SPEC, g.ch, g.w, q.x, q.y, q.k, tint(q.p), { rot: q.rot });
  });
}

interface Piece {
  spec: string;
  ch: string;
  w: number;
  x: number;
  y: number;
  k: number;
  fill: string;
  delay: number;
  /** Launch velocity (px/s) and spin (rad/s). */
  vx: number;
  vy: number;
  spin: number;
  gravity: number;
  /** Seconds at full strength after letting go, then the fade's end. */
  hold: number;
  life: number;
}

// Pieces get a small knock and then gravity takes them: they stay at full
// strength while the drop gets going and spin as they fall. Pieces whose
// drop crosses a flight corridor fade once they are clearly falling; "and"
// and the period fall straight out of the bottom of the frame.
const DEPTH = 1.2;
const G = 12000;

function fallers(lk: Lockup): Piece[] {
  const out: Piece[] = [];
  // The hero line crumbles: a small hop off its guide, then a fast drop.
  const mid = LX + lk.monoW / 2;
  lk.mono.forEach((g, i) => {
    const side = (g.x - mid) / (lk.monoW / 2);
    out.push({
      spec: SPEC_MONO,
      ch: g.ch,
      w: g.w,
      x: g.x,
      y: lk.ym,
      k: 1,
      fill: PALETTE.text2,
      delay: hash(i, 9) * 0.008,
      vx: side * 160 + (hash(i, 3) - 0.5) * 180,
      vy: -(80 + hash(i, 5) * 160),
      spin: (hash(i, 7) - 0.5) * 18,
      gravity: 14000,
      hold: 0.05,
      life: 0.19,
    });
  });
  const heavy = (
    g: Glyph,
    y: number,
    k: number,
    spec: string,
    fill: string,
    delay: number,
    vx: number,
    vy: number,
    spin: number,
    hold: number,
    life: number,
  ) => out.push({ spec, ch: g.ch, w: g.w, x: g.x, y, k, fill, delay, vx, vy, spin, gravity: G, hold, life });
  const P = PALETTE.paper;
  // Both "s," pairs sit where "worktree" is headed. They just let go and
  // tumble straight down, so the names pull away from them instead of
  // carrying them along; they cross behind "worktree" and CI already turned
  // sideways, and fade once the fall reads.
  heavy(lk.proj[7], lk.y1, lk.k1, SPEC, P, 0, 60, 40, 12, 0.067, 0.19);
  heavy(lk.proj[8], lk.y1, lk.k1, SPEC, P, 0.012, 150, 0, 16, 0.067, 0.19);
  heavy(lk.work[8], lk.y2, lk.k2, SPEC, P, 0.004, -40, 60, -11, 0.067, 0.19);
  heavy(lk.work[9], lk.y2, lk.k2, SPEC, P, 0.014, 60, 20, 15, 0.067, 0.19);
  // "and" lets go letter by letter, a frame apart; the
  // period falls away under CI. Nothing flies below them, so they leave
  // through the bottom of the frame.
  const andV: [number, number, number][] = [
    [-70, -130, -3.5],
    [-15, -110, 2.5],
    [45, -120, 4],
  ];
  lk.and.forEach((g, j) =>
    heavy(g, lk.y3, lk.k3, SPEC_LIGHT, PALETTE.text3, 0.014 * j, andV[j][0], andV[j][1], andV[j][2], 0.24, 0.32),
  );
  heavy(lk.ci[2], lk.y3, lk.k3, SPEC, PALETTE.amber, 0.006, -40, -90, -7, 0.24, 0.32);
  return out;
}

/** Screen offset, spin, depth scale, and fade of a piece `d` s after it lets go. */
function fall(p: Pick<Piece, "vx" | "vy" | "spin" | "gravity" | "hold" | "life">, d: number) {
  return {
    dx: p.vx * d,
    dy: p.vy * d + 0.5 * p.gravity * d * d,
    rot: p.spin * d,
    s: 1 / (1 + DEPTH * d),
    alpha: 1 - smoothstep(p.hold, p.life, d),
  };
}

// The HUD, padded: the title row with its rolling chapter label at top left
// and the bar counter at top right. The grid stays out of them.
const HUD_RECTS: [number, number, number, number][] = [
  [36, 20, 660, 104],
  [1716, 30, 1884, 95],
];

const GRID = 40;
/** A faint dot grid: makes the type camera's moves legible; hits ripple it. */
function drawGrid(
  ctx: CanvasRenderingContext2D,
  c: WorldCam,
  lk: Lockup,
  lt: number,
  alpha: number,
): void {
  if (alpha <= 0) return;
  const x0 = c.cx - W / 2 / c.z;
  const x1 = c.cx + W / 2 / c.z;
  const y0 = c.cy - H / 2 / c.z;
  const y1 = c.cy + H / 2 / c.z;
  const waves = [
    { t: 0.1, x: LX + 8, y: lk.ym - 6, amp: 0.8 },
    { t: T_P, x: LX + WL / 2, y: lk.y1, amp: 0.55 },
    { t: T_W, x: LX + WL + 58, y: lk.y2, amp: 0.55 },
    { t: T_CI, x: LX + WL - 180, y: lk.y3 - 100, amp: 0.55 },
    { t: T_OUT, x: W / 2, y: H / 2, amp: 0.5 },
  ].filter((w) => lt > w.t && lt < w.t + 0.6);
  // Brighter while the iris opens, so the lens lands on a lit stage.
  const base = lerp(0.3, 0.16, smoothstep(T_IRIS, T_IRIS + 0.25, lt));
  const s = 2.2 * c.z;
  ctx.save();
  ctx.fillStyle = PALETTE.text3;
  for (let gx = Math.ceil(x0 / GRID) * GRID; gx <= x1; gx += GRID) {
    const px = sx(c, gx);
    for (let gy = Math.ceil(y0 / GRID) * GRID; gy <= y1; gy += GRID) {
      const py = sy(c, gy);
      let hud = false;
      for (const [a, b, e, f] of HUD_RECTS) if (px > a && px < e && py > b && py < f) hud = true;
      if (hud) continue;
      let a = base;
      for (const w of waves) {
        const r = (lt - w.t) * 2600;
        const d = Math.hypot(gx - w.x, gy - w.y) - r;
        a += w.amp * Math.exp(-(d * d) / 5000) * (1 - (lt - w.t) / 0.6);
      }
      // Fall off toward the frame edges.
      const ex = (px - W / 2) / (W / 2);
      const ey = (py - H / 2) / (H / 2);
      const edge = 1 - 0.65 * smoothstep(0.55, 1.05, Math.hypot(ex, ey * 0.9));
      ctx.globalAlpha = alpha * edge * Math.min(a, 0.8);
      ctx.fillRect(px - s / 2, py - s / 2, s, s);
    }
  }
  ctx.restore();
}

function drawWorld(ctx: CanvasRenderingContext2D, lt: number, lk: Lockup, bg: string): void {
  const c = worldCam(lt, lk);
  ctx.fillStyle = bg;
  ctx.fillRect(0, 0, W, H);

  // While CI is giant it owns the frame: everything else all but vanishes
  // for the three-frame hold and comes back on the snap.
  const giant = lt >= T_CI ? 1 - clamp(ciSnap(lt)) : 0;
  const dim = 1 - giant;
  const out = lt >= T_OUT;
  const loose = inQuad(progress(T_OUT - 0.055, T_OUT, lt));

  drawGrid(
    ctx,
    c,
    lk,
    lt,
    smoothstep(0.02, 0.16, lt) * (1 - smoothstep(T_OUT + 0.05, T_OUT + 0.32, lt)) * dim,
  );

  // The iris lands on a lit prompt: a glow that rides the caret, and a
  // double ripple fixed in the world where the caret sat before the first
  // key, so the camera's track carries it off to the left. Both rings are
  // spent before the typing gets far.
  if (lt < T_P) {
    const cr = caretRect(c, lk, lt);
    const glowA = smoothstep(0.1, 0.2, lt) * (1 - smoothstep(T_TYPED - 0.05, T_P, lt));
    glow(ctx, cr.x, cr.y, 150 * (c.z / Z0), PALETTE.amber, 0.55 * glowA);
    const ex = sx(c, LX + 1 + CARET_W / 2);
    const ey = sy(c, lk.ym - 21 + CARET_H / 2);
    ring(ctx, ex, ey, 380 * c.z, progress(0.12, 0.35, lt), PALETTE.amberBright, 7);
    ring(ctx, ex, ey, 260 * c.z, progress(0.17, 0.35, lt), PALETTE.amber, 4);
  }

  // Construction guides: baselines drawn with a pen tip, and the margins.
  ctx.save();
  ctx.lineCap = "round";
  const guides = [lk.ym, lk.y1, lk.y2, lk.y3];
  guides.forEach((y, i) => {
    const g0 = outCubic(progress(T_OUT + i * 0.012, T_OUT + 0.12 + i * 0.012, lt));
    const g1 = swiftOut(progress(0.04 + i * 0.05, 0.4 + i * 0.05, lt));
    if (g1 <= g0) return;
    const xa = LX - 110;
    const xb = LX + WL + 110;
    // Retracting guides fade as they go, so no stub is left hanging.
    ctx.strokeStyle = rgba(PALETTE.divider, (i === 0 ? 0.6 : 0.95) * dim * (1 - g0));
    ctx.lineWidth = 1.5;
    ctx.beginPath();
    ctx.moveTo(sx(c, lerp(xa, xb, g0)), sy(c, y));
    ctx.lineTo(sx(c, lerp(xa, xb, g1)), sy(c, y));
    ctx.stroke();
    if (i > 0 && i !== 2) {
      const hitA = pulse(lt, [T_P, T_W, T_CI][i - 1], 0.004, 0.09) * dim;
      if (hitA > 0.01) {
        ctx.strokeStyle = rgba(PALETTE.amberBright, 0.7 * hitA);
        ctx.lineWidth = 2.5;
        ctx.stroke();
      }
    }
    if (g1 < 0.995) {
      const tx = sx(c, lerp(xa, xb, g1));
      glow(ctx, tx, sy(c, y), 26 * c.z, PALETTE.amber, 0.7 * (1 - g1));
    }
  });
  for (const [i, x] of [LX, LX + WL].entries()) {
    const g0 = outCubic(progress(T_OUT, T_OUT + 0.12, lt));
    const g1 = swiftOut(progress(0.12 + i * 0.06, 0.52 + i * 0.06, lt));
    if (g1 <= g0) continue;
    const ya = lk.ym - 70;
    const yb = lk.y3 + 50;
    ctx.strokeStyle = rgba(PALETTE.divider, 0.6 * dim * (1 - g0));
    ctx.lineWidth = 1.5;
    ctx.beginPath();
    ctx.moveTo(sx(c, x), sy(c, lerp(ya, yb, g0)));
    ctx.lineTo(sx(c, x), sy(c, lerp(ya, yb, g1)));
    ctx.stroke();
  }
  ctx.restore();

  const br = branchGeom(lk);
  drawBranch(ctx, c, br, lt, bg, dim);

  // "projects,": letters drop in a fast cascade, one per 128th note, and
  // squash on the main line. Drawn before the kicker so they fall behind it.
  const STAG = beat(1 / 32);
  const FALL = 0.14;
  // Screen boxes of letters still dropping, for the kicker's knockout.
  const drops: [number, number, number, number][] = [];
  // Descenders skip the main line's ink where they cross it.
  const railBand = (y: number) => {
    const ry = sy(c, y + RAIL_DY);
    const hw = (BRANCH_W / 2 + 1.5) * c.z;
    return { color: bg, y0: ry - hw, y1: ry + hw, gap: 7 * c.z };
  };
  const skip1 = railBand(lk.y1);
  if (!out) {
    lk.proj.forEach((g, i) => {
      const tl = T_P + i * STAG;
      if (lt < tl - FALL) return;
      const u = progress(tl - FALL, tl, lt);
      let dy = -950 * (1 - u * u);
      let sxk = 1;
      let syk = 1;
      let rot = (hash(i, 7) - 0.5) * 0.7 * (1 - u);
      if (u < 1) {
        syk = 1 + 0.4 * u;
        sxk = 1 - 0.15 * u;
        const k = lk.k1 * c.z;
        const x = sx(c, g.x);
        const y = sy(c, lk.y1) + dy * c.z;
        const hw = 0.5 * g.w * k * sxk + 0.1 * EM * k;
        drops.push([x - hw, y - 0.8 * EM * k * syk, x + hw, y + 0.25 * EM * k * syk]);
      } else {
        const d = lt - tl;
        const sq = 0.18 * Math.exp(-d * 16) * Math.cos(TAU * 5 * d);
        syk = 1 - sq;
        sxk = 1 + sq * 0.4;
        dy = 0;
        rot = 0;
      }
      if (i >= 7) {
        dy -= 5 * loose;
        rot += (hash(i, 19) - 0.5) * 0.14 * loose;
      }
      const desc = lt >= T_MAIN && (g.ch === "p" || g.ch === "j" || g.ch === ",");
      glyph(ctx, SPEC, g.ch, g.w, sx(c, g.x), sy(c, lk.y1) + dy * c.z, lk.k1 * c.z, PALETTE.paper, {
        sx: sxk,
        sy: syk,
        rot,
        alpha: dim,
        skip: desc ? skip1 : undefined,
      });
    });
  }

  // The hero line: each key lands warm and a few px low, then settles and
  // cools. The caret leads, solid while typing, then blinks in time.
  const monoDip = 10 * wobble(lt, T_P, 4, 10);
  if (!out) {
    lk.mono.forEach((g, i) => {
      const t = KEY_T[i];
      if (lt < t || g.ch === " ") return;
      const d = lt - t;
      const rise = 6 * (1 - outCubic(progress(0, 0.07, d))) - 3 * loose;
      const fill = mix(PALETTE.amberBright, PALETTE.text2, smoothstep(0.02, 0.14, d));
      const x = sx(c, g.x);
      const y = sy(c, lk.ym + monoDip + rise);
      // A thin knockout only where a dropping letter passes behind.
      const hw = 0.5 * g.w * c.z;
      let over = false;
      for (const [a, b, e, f] of drops) {
        if (x + hw > a && x - hw < e && y + 7 * c.z > b && y - 22 * c.z < f) over = true;
      }
      glyph(ctx, SPEC_MONO, g.ch, g.w, x, y, c.z, fill, {
        alpha: dim,
        halo: over ? { color: bg, width: 4.5 * c.z } : undefined,
      });
    });
    if (lt >= T_IRIS) {
      const on = lt < T_P || (lt / beat(1)) % 1 < 0.5;
      if (on) {
        const r = caretRect(c, lk, lt);
        fillCaret(ctx, r.x, r.y + monoDip * c.z, r.w, r.h, 2, rgba(PALETTE.amber, dim));
      }
    }
  }

  // "worktrees,": letters sprout up out of the branch as its head races past.
  if (!out && lt >= T_BRANCH) {
    const maskY = sy(c, br.by);
    const RISE = 0.1;
    const hide = 0.78 * EM * lk.k2;
    lk.work.forEach((g, j) => {
      const t0 = branchTime(br.sAt(g.x - 0.2 * g.w * lk.k2), br) - 0.01;
      if (lt < t0) return;
      const u = progress(t0, t0 + RISE, lt);
      let dy = hide * (1 - outBack(2.2)(u));
      let rot = -0.28 * (1 - outCubic(u)) * (j % 2 ? -1 : 1);
      if (j >= 8) {
        dy -= 5 * loose;
        rot += (hash(j, 23) - 0.5) * 0.14 * loose;
      }
      const clip = dy > 0.5;
      if (clip) {
        ctx.save();
        ctx.beginPath();
        ctx.rect(0, 0, W, maskY);
        ctx.clip();
      }
      // The comma's tail skips the branch's ink, like the descenders above.
      const tail = !clip && g.ch === ",";
      glyph(ctx, SPEC, g.ch, g.w, sx(c, g.x), sy(c, lk.y2 + dy), lk.k2 * c.z, PALETTE.paper, {
        rot,
        alpha: dim,
        skip: tail ? railBand(lk.y2) : undefined,
      });
      if (clip) ctx.restore();
    });
  }

  // "and" glides in on the pickup; CI stamps in giant with a split on the
  // half beat, then snaps down beside it.
  if (!out && lt >= T_AND) {
    lk.and.forEach((g, j) => {
      const u = swiftOut(progress(T_AND + j * 0.025, T_AND + 0.2 + j * 0.025, lt));
      if (u <= 0) return;
      const rot = (hash(j, 29) - 0.5) * 0.14 * loose;
      const x = sx(c, g.x - 90 * (1 - u));
      const y = sy(c, lk.y3 - 5 * loose);
      glyph(ctx, SPEC_LIGHT, g.ch, g.w, x, y, lk.k3 * c.z, PALETTE.text3, { alpha: u * dim, rot });
    });
  }
  if (!out && lt >= T_CI) drawCI(ctx, c, lk, lt, loose);

  // Breakup: extras are knocked back and fall away; the names fly home.
  if (out) {
    for (const p of fallers(lk)) {
      const d = Math.max(0, lt - T_OUT - p.delay);
      const f = fall(p, d);
      if (f.alpha <= 0) continue;
      glyph(ctx, p.spec, p.ch, p.w, sx(c, p.x) + f.dx, sy(c, p.y) + f.dy, p.k * c.z * f.s, p.fill, {
        rot: f.rot,
        alpha: f.alpha,
      });
    }
    // The caret goes with its line.
    const cr = caretRect(c, lk, lt);
    const caret = { vx: 240, vy: -200, spin: 9, gravity: 14000, hold: 0.05, life: 0.19 };
    const f = fall(caret, Math.max(0, lt - T_OUT - 0.004));
    if (f.alpha > 0) {
      ctx.save();
      ctx.globalAlpha *= f.alpha;
      ctx.translate(cr.x + f.dx, cr.y + f.dy);
      ctx.rotate(f.rot);
      fillCaret(ctx, 0, 0, cr.w * f.s, cr.h * f.s, 2, PALETTE.amber);
      ctx.restore();
    }
    for (const fl of flyers(lk)) drawFlyer(ctx, c, fl, lt);
  }

  if (lt >= T_CI - 0.01) flash(ctx, W, H, 0.1 * pulse(lt, T_CI, 0.001, 0.05), PALETTE.amberBright);
}

/** Pen tip: a hot dot with its light. */
function penTip(ctx: CanvasRenderingContext2D, x: number, y: number, z: number, a: number): void {
  glow(ctx, x, y, 80 * z, PALETTE.amber, 0.85 * a);
  ctx.beginPath();
  ctx.arc(x, y, 8 * z, 0, TAU);
  ctx.fillStyle = rgba(PALETTE.amberBright, a);
  ctx.fill();
}

/** Main, the branch, their commits, and the racing heads. */
function drawBranch(
  ctx: CanvasRenderingContext2D,
  c: WorldCam,
  br: Branch,
  lt: number,
  bg: string,
  dim: number,
): void {
  const head = branchS(lt, br);
  const retract = outCubic(progress(T_OUT, T_OUT + 0.14, lt));
  const gone = 1 - outCubic(progress(T_OUT, T_OUT + 0.1, lt));
  const lw = 4 * c.z;
  ctx.save();
  ctx.lineCap = "round";
  ctx.lineJoin = "round";
  // Main: the commit in the left margin pops as "projects," settles, and
  // the line draws out behind the word to a second commit on the right.
  const fork = outBack(2.2)(progress(T_MAIN - 0.02, T_MAIN + 0.09, lt)) * gone;
  const mainP = swiftInOut(progress(T_MAIN + 0.02, T_BRANCH - 0.03, lt));
  if (mainP > 0) {
    const m0 = lerp(br.ax, br.x1, retract);
    const m1 = lerp(br.ax, br.x1, mainP);
    if (m1 > m0) {
      ctx.beginPath();
      ctx.moveTo(sx(c, m0), sy(c, br.ay));
      ctx.lineTo(sx(c, m1), sy(c, br.ay));
      ctx.strokeStyle = rgba(PALETTE.teal, dim * (1 - retract));
      ctx.lineWidth = BRANCH_W * c.z;
      ctx.stroke();
    }
    if (mainP < 1) penTip(ctx, sx(c, m1), sy(c, br.ay), c.z, 0.8);
  }
  const tipAt = MAIN_TIP;
  const tip = outBack(2.6)(progress(tipAt - 0.01, tipAt + 0.08, lt)) * gone;
  const tipHot = 0.8 * pulse(lt, tipAt, 0.004, 0.06);
  commit(ctx, sx(c, br.x1), sy(c, br.ay), 11 * c.z * tip, lw, tipHot, bg, dim);
  if (head > 0) {
    const tail = br.total * retract;
    if (head > tail) {
      ctx.beginPath();
      const n = Math.max(2, Math.ceil((head - tail) / 12));
      for (let i = 0; i <= n; i++) {
        const [x, y] = br.pt(lerp(tail, head, i / n));
        if (i === 0) ctx.moveTo(sx(c, x), sy(c, y));
        else ctx.lineTo(sx(c, x), sy(c, y));
      }
      ctx.strokeStyle = rgba(PALETTE.teal, dim * (1 - retract));
      ctx.lineWidth = BRANCH_W * c.z;
      ctx.stroke();
    }
    if (lt < T_W) {
      const [hx, hy] = br.pt(head);
      penTip(ctx, sx(c, hx), sy(c, hy), c.z, 1);
    }
  }
  // The fork commit flares as the branch leaves it.
  const flare = 0.7 * pulse(lt, T_BRANCH, 0.02, 0.07);
  commit(ctx, sx(c, br.ax), sy(c, br.ay), 11 * c.z * fork, lw, flare, bg, dim);
  // The head lands in the branch's own commit on the beat: a hot flash that
  // cools to a ring.
  const end = outBack(2.6)(progress(T_W - 0.012, T_W + 0.09, lt)) * gone;
  if (end > 0) {
    const ex = sx(c, br.x1);
    const ey = sy(c, br.by);
    const hot = 1 - smoothstep(T_W, T_W + 0.22, lt);
    glow(ctx, ex, ey, 150 * c.z, PALETTE.amberBright, 0.9 * pulse(lt, T_W, 0.004, 0.09) * dim);
    ring(ctx, ex, ey, 150 * c.z, progress(T_W, T_W + 0.26, lt), PALETTE.amberBright, 5);
    commit(ctx, ex, ey, 12 * c.z * end, lw, hot, bg, dim);
  }
  ctx.restore();
}

/** Snap progress for CI: 0 while giant, overshoots past 1, settles at 1. */
function ciSnap(lt: number): number {
  if (lt < T_SNAP) return 0;
  const a = progress(T_SNAP, T_SNAP + 0.09, lt);
  if (a < 1) return 1.05 * outCubic(a);
  return lerp(1.05, 1, cubicBezier(0.45, 0, 0.55, 1)(progress(T_SNAP + 0.09, T_SNAP + 0.2, lt)));
}

const GIANT = 3.1;
function drawCI(
  ctx: CanvasRenderingContext2D,
  c: WorldCam,
  lk: Lockup,
  lt: number,
  loose: number,
): void {
  const sp = ciSnap(lt);
  const press = 1 + 0.12 * (1 - outCubic(progress(T_CI, T_SNAP, lt)));
  const g = Math.exp(lerp(Math.log(GIANT * press), 0, sp));
  // The group's rest center; the giant pose is centered on the frame.
  const gx = (lk.ci[0].x - lk.ci[0].w * lk.k3 * 0.5 + lk.ci[2].x + lk.ci[2].w * lk.k3 * 0.5) / 2;
  const gy = lk.y3 - 0.36 * EM * lk.k3;
  const rx = sx(c, gx);
  const ry = sy(c, gy);
  const cx = lerp(W / 2, rx, sp);
  const cy = lerp(H / 2, ry, sp);
  const k = lk.k3 * c.z * g;
  const split = 30 * (1 - smoothstep(T_CI, T_SNAP + 0.12, lt)) ** 1.5;
  const f = Math.floor(lt * 60);
  const jx = (hash(f, 11) - 0.5) * split * 0.5;
  const jy = (hash(f, 13) - 0.5) * split * 0.3;
  const place = (gl: Glyph, i: number, ox: number, oy: number, fill: string, alpha = 1) => {
    const x = cx + (sx(c, gl.x) - rx) * g + ox;
    let y = cy + (sy(c, lk.y3) - ry) * g + oy;
    let rot = 0;
    if (i === 2) {
      y -= 5 * loose;
      rot = 0.1 * loose;
    }
    glyph(ctx, SPEC, gl.ch, gl.w, x, y, k, fill, { alpha, rot });
  };
  // Hot paper core cooling to amber; the split pulls a teal and an amber
  // fringe off either side, both from the palette, behind an opaque core.
  const core = mix(PALETTE.paper, PALETTE.amber, smoothstep(T_SNAP, T_SNAP + 0.14, lt));
  if (split > 0.4) {
    const amt = clamp(split / 12) ** 0.7;
    lk.ci.forEach((gl, i) => place(gl, i, -split - jx, -jy, PALETTE.tealLight, 0.9 * amt));
    lk.ci.forEach((gl, i) => place(gl, i, split + jx, jy, PALETTE.amberDeep, 0.9 * amt));
  }
  lk.ci.forEach((gl, i) => place(gl, i, 0, 0, core));
}

export const scene: Scene = {
  id: S.id,
  start: S.start,
  end: S.end,
  draw(ctx, lt) {
    ctx.save();
    ctx.textAlign = "left";
    ctx.textBaseline = "alphabetic";
    if (lt >= T_LAND) {
      ctx.fillStyle = PALETTE.bg;
      ctx.fillRect(0, 0, W, H);
      for (const k of ["project", "worktree", "ci"] as const) drawNodeLabel(ctx, k);
      ctx.restore();
      return;
    }
    const lk = lockup(ctx);
    if (lt < T_IRIS) drawDive(ctx, lt, lk);
    else drawWorld(ctx, lt, lk, PALETTE.bg);
    ctx.restore();
  },
};
