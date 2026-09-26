// Scene 3, "What mbx does". The monocle dive that mr-boxington started
// (bible.ts diveCam) punches through the lens; his pupil opens on a type
// world, and the glint arc peels off the glass to become the caret that
// types the lead. The lockup lands under it with one treatment per word,
// the whole sentence holds long enough to read, then everything but the
// three node names falls away and the names fly to every-checkout's labels.

import {
  BEAT,
  beat,
  DIVE0,
  DIVE1,
  DIVE_ZOOM,
  diveCam,
  drawLogoBox,
  drawNodeLabel,
  H,
  H2_POSE,
  NODE_LABEL,
  NODES,
  PALETTE,
  type Scene,
  sec,
  W,
} from "../bible";
import { faceToScreen, LOGO_FACE, LOGO_INK, logoCam } from "../box";
import { mix, rgba } from "../color";
import { flash, glow, ring, shake } from "../fx";
import {
  clamp,
  cubicBezier,
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
import { type Camera, View } from "../space";
import { font, layout } from "../type";

const S = sec("what");

// Beat map, local seconds. The score (score/type.ts) is written to these.
/** b0.5: the lens covers the frame and the pupil opens on the type world. */
export const T_IRIS = DIVE1 - S.start;
/** The glint arc lets go of the lens and swings down toward the caret. */
export const T_PEEL = beat(0.3);
/** It has become the caret; the first key follows. */
export const T_CARET = beat(0.85);
/** The lead's last key. */
const T_TYPED = beat(1.9);
export const T_P = beat(2.25); // "projects," cascades in, first letter first
export const T_MAIN = beat(2.5); // main draws out under it, commit to commit
export const T_BRANCH = beat(2.75); // a git branch leaves the main line...
export const T_W = beat(3); // ...and lands on its commit: "worktrees," locks
export const T_AND = beat(3.75); // "and" glides in on the sixteenth pickup
export const T_CI = beat(4); // "CI." stamps in giant
export const T_SNAP = T_CI + 0.05; // ...holds three frames, then snaps down
/** b4.5: the sentence is complete and settled; it holds to T_OUT. */
export const T_DONE = beat(4.5);
export const T_OUT = beat(11); // breakup: extras fall, names fly
/** "projects," lands a letter at a time, one per 1/128 note. */
export const DROP_STAG = beat(1 / 32);
/** Main's second commit pops just before the branch forks. */
export const MAIN_TIP = T_BRANCH - 0.03;
// The names reach their labels a frame before the section ends, as the
// score's name whooshes peak: from here on the frame is exactly handoff
// what → every-checkout.
export const T_LAND = S.len - 0.0175;
/** Global time of the lockup's caret-on hold: the storyboard's fallback poster. */
export const LOCKUP = S.beat(8);

/** The lead, typed in two lines; the display lockup below it finishes the sentence. */
export const LEAD = ["mbx reuses Cargo", "compiler work across"] as const;
const LEAD_SIZE = 80;
const LEAD_SPEC = font(LEAD_SIZE, 600);
/** Baseline to baseline. */
const LEAD_PITCH = 94;
const EM = NODE_LABEL.size;
// Display glyphs are always drawn at the label's own font and scaled, so a
// word in flight has the same outlines as the label it lands as.
const SPEC = font(EM, NODE_LABEL.weight);
const SPEC_LIGHT = font(EM, 300);
const TRACK = -0.04 * EM;
const Z0 = 1.75; // camera zoom on the lead while it types
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
  /** The lead's lines, their glyphs, and baselines. */
  lead: Glyph[][];
  yl: number[];
  /** Left edge and width of the block: the lead's second line sets both. */
  lx: number;
  wl: number;
  k1: number;
  k2: number;
  k3: number;
  y1: number;
  y2: number;
  y3: number;
  proj: Glyph[];
  work: Glyph[];
  and: Glyph[];
  ci: Glyph[];
  /** Camera centers while the lockup is still building. */
  c0: number;
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
  const wl = layout(ctx, LEAD[1], LEAD_SPEC).width;
  const lx = (W - wl) / 2;
  const width = (s: string) => layout(ctx, s, SPEC, TRACK).width;
  // Every display line is justified to the lead's second line.
  const k1 = wl / width("projects,");
  const k2 = wl / width("worktrees,");
  const k3 = wl / width("and CI.");
  // Baselines from the font's ascender (0.72 em) and descender (0.21 em).
  const yl0 = 0.72 * LEAD_SIZE;
  const yl1 = yl0 + LEAD_PITCH;
  const y1 = yl1 + 0.21 * LEAD_SIZE + 40 + 0.72 * EM * k1;
  const y2 = y1 + 0.21 * EM * k1 + 10 + 0.72 * EM * k2;
  const y3 = y2 + 54 + 0.7 * EM * k3;
  // Centered on the frame, a little high: the block's weight is at its foot.
  const dy = 532 - y3 / 2;
  const ciRow = row(ctx, "CI.", SPEC, TRACK, 0, k3);
  const lk: Lockup = {
    lead: LEAD.map((l) => row(ctx, l, LEAD_SPEC, 0, lx, 1).glyphs),
    yl: [yl0 + dy, yl1 + dy],
    lx,
    wl,
    k1,
    k2,
    k3,
    y1: y1 + dy,
    y2: y2 + dy,
    y3: y3 + dy,
    proj: row(ctx, "projects,", SPEC, TRACK, lx, k1).glyphs,
    work: row(ctx, "worktrees,", SPEC, TRACK, lx, k2).glyphs,
    and: row(ctx, "and", SPEC_LIGHT, TRACK, lx, k3).glyphs,
    ci: ciRow.glyphs.map((g) => ({ ...g, x: g.x + lx + wl - ciRow.width })),
    c0: 0,
    c1: 0,
    c2: 0,
  };
  const top = dy;
  lk.c0 = (top + lk.yl[1] + 0.21 * LEAD_SIZE) / 2;
  lk.c1 = (top + lk.y1 + 0.21 * EM * k1) / 2;
  lk.c2 = (top + lk.y2 + 30) / 2;
  return lk;
}

// --- The keys ----------------------------------------------------------------

/** One key of the lead: a glyph on a line, or the return between the lines. */
export interface Key {
  ch: string;
  /** Local time it lands. */
  t: number;
  /** Line and glyph index; the return has glyph -1. */
  line: number;
  i: number;
  /** A word's first letter: the score ticks these harder. */
  word: boolean;
}

// The keys land on an uneven hand's rhythm, with a breath after each word
// and a longer one for the return. The score ticks every key it plays from
// this list, so each visible key has its click.
export const KEYS: readonly Key[] = (() => {
  const keys: Omit<Key, "t">[] = [];
  LEAD.forEach((text, line) => {
    if (line > 0) keys.push({ ch: "\n", line, i: -1, word: false });
    Array.from(text).forEach((ch, i) => keys.push({ ch, line, i, word: i === 0 || text[i - 1] === " " }));
  });
  const w = keys.map((k, n) => (k.ch === "\n" ? 2.2 : 0.6 + hash(n, 41) * 0.8 + (k.word ? 0.5 : 0)));
  const total = w.reduce((s, v) => s + v, 0) - w[w.length - 1];
  let acc = 0;
  return keys.map((k, n) => {
    const t = T_CARET + 0.03 + (acc / total) * (T_TYPED - T_CARET - 0.03);
    acc += w[n];
    return { ...k, t };
  });
})();

// --- The type world's camera -------------------------------------------------

/** The type world's camera: frames the lockup as it grows, shakes on hits. */
interface WorldCam {
  cx: number;
  cy: number;
  z: number;
  ox: number;
  oy: number;
}

/** The inhale before the breakup: a few frames, so the hold keeps its reading time. */
export const INHALE = 0.04;

/** The camera's inhale: builds over INHALE, released by the break. */
function swell(lt: number): number {
  return inQuad(progress(T_OUT - INHALE, T_OUT, lt)) * (1 - outCubic(progress(T_OUT, T_OUT + 0.14, lt)));
}

/**
 * fx.shake, eased out before its last steps: those are sub-pixel and would
 * shimmer the type's edges at the start of the hold.
 */
function jolt(lt: number, at: number, amp: number, decay: number): [number, number] {
  const [x, y] = shake(lt, at, amp, decay);
  const k = 1 - smoothstep(at + 2 * decay, at + 3 * decay, lt);
  return [x * k, y * k];
}

function worldCam(lt: number, lk: Lockup): WorldCam {
  // Rushing in as the pupil opens, close on the lead as it types, then
  // pulling back as the first display word lands and reframing as each
  // line joins, to rest with the block centered.
  const open = lerp(0.5, 1, outCubic(progress(T_IRIS - 0.08, T_IRIS + 0.34, lt)));
  const pull = swiftInOut(progress(T_TYPED - 0.06, T_P + 0.01, lt));
  const a = swiftInOut(progress(T_BRANCH - 0.08, T_W + 0.04, lt));
  const b = swiftInOut(progress(T_AND - 0.12, T_CI - 0.005, lt));
  const typed = smoothstep(T_CARET, T_TYPED, lt);
  const mid = lk.lx + lk.wl / 2;
  const cx = lerp(lerp(mid - 50, mid + 50, typed), W / 2, pull);
  const cy = lerp(lk.c0, lerp(lerp(lk.c1, lk.c2, a), H / 2, b), pull);
  // At rest the camera is exactly still: a slow drift would step each line
  // of type a pixel at a time as the canvas snaps its baselines.
  const frame = lerp(Z0 * open, lerp(lerp(Z1, Z2, a), 1, b), pull);
  const z = frame * (1 + 0.02 * swell(lt));
  const s1 = jolt(lt, T_P, 7, 0.06);
  const s2 = jolt(lt, T_W, 4, 0.05);
  const s3 = jolt(lt, T_CI, 14, 0.07);
  const s4 = jolt(lt, T_OUT, 4, 0.04);
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

// --- The caret -----------------------------------------------------------------

// A pill the height of the lead's ascender to its descender: the glint arc,
// straightened.
const CARET_W = 13;
const CARET_H = 76;

/** Keys landed by `lt`. */
function keysIn(lt: number): number {
  let n = 0;
  while (n < KEYS.length && lt >= KEYS[n].t) n++;
  return n;
}

/** The caret's center and size on screen. */
function caretRect(c: WorldCam, lk: Lockup, lt: number) {
  const n = keysIn(lt);
  const last = n > 0 ? KEYS[n - 1] : null;
  let x = lk.lx - 4 - CARET_W / 2;
  let line = 0;
  if (last) {
    line = last.line;
    if (last.i >= 0) {
      const g = lk.lead[line][last.i];
      x = g.x + g.w / 2 + 5 + CARET_W / 2;
    }
  }
  const y = lk.yl[line] + 14 - CARET_H / 2;
  return { x: sx(c, x), y: sy(c, y), w: CARET_W * c.z, h: CARET_H * c.z };
}

/** A caret, or the arc on its way to being one: a round-capped stroke through `pts`. */
function stroke(ctx: CanvasRenderingContext2D, pts: readonly [number, number][], width: number, color: string): void {
  ctx.save();
  ctx.lineCap = "round";
  ctx.lineJoin = "round";
  ctx.lineWidth = width;
  ctx.strokeStyle = color;
  ctx.beginPath();
  pts.forEach(([x, y], i) => (i ? ctx.lineTo(x, y) : ctx.moveTo(x, y)));
  ctx.stroke();
  ctx.restore();
}

/** The caret's pill as a stroke from its top to its bottom, in `n` points. */
function caretPts(r: { x: number; y: number; w: number; h: number }, n: number): [number, number][] {
  const top = r.y - r.h / 2 + r.w / 2;
  const bottom = r.y + r.h / 2 - r.w / 2;
  return Array.from({ length: n }, (_, i) => [r.x, lerp(bottom, top, i / (n - 1))]);
}

// --- The monocle dive ----------------------------------------------------------

/** The logo's square on screen: logo (x, y) is at (x0 + x k, y0 + y k), k = size / 128. */
interface Square {
  x: number;
  y: number;
  size: number;
}

function squareOf(cam: Camera): Square {
  const v = new View(cam);
  const a = faceToScreen(v, H2_POSE, 0, 0);
  const b = faceToScreen(v, H2_POSE, 128, 0);
  return { x: a.x, y: a.y, size: b.x - a.x };
}
const onScreen = (q: Square, x: number, y: number): [number, number] => [
  q.x + (x * q.size) / 128,
  q.y + (y * q.size) / 128,
];

// The dive's zoom runs DIVE_ZOOM ** inCubic(u), so it is fastest as it
// lands on the lens. Past DIVE1 the camera coasts on through the glass at
// that rate, easing off over COAST, so the push never stops dead.
const RATE = (3 * Math.log(DIVE_ZOOM)) / (DIVE1 - DIVE0);
const COAST = 0.05;
let diveEnd: { sq: Square; fixed: [number, number] } | null = null;

/** The dive's last square, and the fixed point its zoom was taken about. */
function landed() {
  if (!diveEnd) {
    const sq = squareOf(diveCam(DIVE1));
    const s0 = squareOf(diveCam(DIVE0));
    const z = sq.size / s0.size;
    diveEnd = { sq, fixed: [(z * s0.x - sq.x) / (z - 1), (z * s0.y - sq.y) / (z - 1)] };
  }
  return diveEnd;
}

/** The logo's square at local time `lt`: diveCam's, then coasting on through the lens. */
function diveSquare(lt: number): Square {
  if (lt <= T_IRIS) return squareOf(diveCam(S.start + lt));
  const { sq, fixed } = landed();
  const d = lt - T_IRIS;
  const z = Math.exp(RATE * COAST * (1 - Math.exp(-d / COAST)));
  // The coast starts about the dive's own fixed point and drifts to the
  // pupil, so the pupil stays in frame as it opens.
  const [px, py] = onScreen(sq, 86, 63);
  const k = smoothstep(0, 0.08, d);
  const fx = lerp(fixed[0], px, k);
  const fy = lerp(fixed[1], py, k);
  return { x: fx + z * (sq.x - fx), y: fy + z * (sq.y - fy), size: z * sq.size };
}

/** The pupil behind the glass, in logo units: it dilates as the dive lands, then opens past the lens. */
function pupilRadius(lt: number): number {
  return 8.5 + 3 * inQuad(progress(0, T_IRIS, lt)) + 60 * inQuad(progress(T_IRIS, T_IRIS + 0.16, lt));
}

// The glint arc, logo.svg's "M69 57a18 18 0 0 1 10-12": 18 units about
// (86.46, 61.38), from 194.1° to 245.5°, drawn 4 units wide.
const ARC_C: [number, number] = [86.46, 61.38];
const ARC_A0 = Math.atan2(57 - ARC_C[1], 69 - ARC_C[0]) + TAU;
const ARC_A1 = Math.atan2(45 - ARC_C[1], 79 - ARC_C[0]) + TAU;
const ARC_N = 24;
/** A reflection does not ride the glass: the arc eases off the dive over this long. */
const LETGO = 0.04;
const MORPH = cubicBezier(0.45, 0, 0.2, 1);

/** The arc's points in screen space at local time `lt`, morphing into the caret at `r`. */
function arcPts(lt: number, r: { x: number; y: number; w: number; h: number }) {
  const d = Math.max(0, lt - T_PEEL);
  const q = diveSquare(T_PEEL + LETGO * (1 - Math.exp(-d / LETGO)));
  const src = Array.from({ length: ARC_N }, (_, i): [number, number] => {
    const a = lerp(ARC_A0, ARC_A1, i / (ARC_N - 1));
    return onScreen(q, ARC_C[0] + 18 * Math.cos(a), ARC_C[1] + 18 * Math.sin(a));
  });
  const dst = caretPts(r, ARC_N);
  const m = MORPH(progress(T_PEEL + 0.02, T_CARET, lt));
  // It swings down in a curve, bowing out to the left of the straight path.
  const bow = Math.sin(Math.PI * m) * 0.18;
  const pts = src.map(([x, y], i): [number, number] => {
    const [u, v] = dst[i];
    const dx = u - x;
    const dy = v - y;
    return [lerp(x, u, m) + dy * bow, lerp(y, v, m) - dx * bow];
  });
  return { pts, width: lerp((4 * q.size) / 128, r.w, m), m };
}

function drawDive(ctx: CanvasRenderingContext2D, lt: number, lk: Lockup): void {
  const q = diveSquare(lt);
  const cam = lt <= T_IRIS ? diveCam(S.start + lt) : logoCam(q.x, q.y, q.size);
  const k = q.size / 128;
  const [px, py] = onScreen(q, 86 + LOGO_FACE.look[0], 63 + LOGO_FACE.look[1]);
  const rp = pupilRadius(lt) * k;
  // Once the pupil holds all four corners, only the world is left.
  const far = Math.max(Math.hypot(px, py), Math.hypot(W - px, py), Math.hypot(px, H - py), Math.hypot(W - px, H - py));
  const c = worldCam(lt, lk);
  const bg = mix(LOGO_INK, PALETTE.bg, smoothstep(T_IRIS - 0.04, T_IRIS + 0.2, lt));
  if (rp < far) {
    ctx.fillStyle = PALETTE.bg;
    ctx.fillRect(0, 0, W, H);
    // The contract frame on the bar line; after it the arc is ours to move.
    const pose = lt < T_PEEL ? H2_POSE : { ...H2_POSE, face: { ...LOGO_FACE, arc: 0 } };
    drawLogoBox(ctx, cam, pose);
    if (lt <= 0) return;
    // The pupil dilates behind the glass, then opens past it on the world.
    ctx.save();
    if (lt < T_IRIS) {
      const [mx, my] = onScreen(q, 86, 62);
      ctx.beginPath();
      ctx.arc(mx, my, 18 * k, 0, TAU);
      ctx.clip();
    }
    ctx.beginPath();
    ctx.arc(px, py, rp, 0, TAU);
    ctx.clip();
    drawWorld(ctx, lt, lk, bg, c);
    ctx.restore();
    // A hot rim as the iris opens.
    const rim = smoothstep(T_IRIS - 0.08, T_IRIS, lt) * (1 - smoothstep(T_IRIS + 0.04, T_IRIS + 0.14, lt));
    if (rim > 0) {
      ctx.save();
      ctx.strokeStyle = rgba(PALETTE.amberBright, 0.8 * rim);
      ctx.lineWidth = 3 + 0.02 * rp;
      ctx.beginPath();
      ctx.arc(px, py, rp, 0, TAU);
      ctx.stroke();
      ctx.restore();
    }
  } else {
    drawWorld(ctx, lt, lk, bg, c);
  }
  if (lt < T_PEEL || lt >= T_CARET) return;
  // The glint arc lets go of the lens and becomes the caret, cooling from
  // the glass's white to amber as it straightens.
  const r = caretRect(c, lk, lt);
  const arc = arcPts(lt, r);
  const mid = arc.pts[ARC_N >> 1];
  glow(ctx, mid[0], mid[1], 120 + 2 * arc.width, PALETTE.amberBright, 0.5 * Math.sin(Math.PI * arc.m));
  stroke(ctx, arc.pts, arc.width, mix("#ffffff", PALETTE.amber, smoothstep(0.35, 1, arc.m)));
}

// --- The type world ------------------------------------------------------------

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
  const ax = lk.lx - 58;
  const ay = lk.y1 + RAIL_DY;
  const by = lk.y2 + RAIL_DY;
  const r = 46;
  const segV = by - r - ay;
  const arc = (r * Math.PI) / 2;
  const x1 = lk.lx + lk.wl + 58;
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
  /** Sideways bow of the arc, px (positive is right). */
  bow: number;
  /** Peak lean mid-flight, radians; negative lifts the right end. */
  bank: number;
  /** Launch after T_OUT, seconds. */
  delay: number;
  /** +1 when the word travels right, so its right end leads. */
  dir: number;
}

// The names are under way two frames after the break and keep real speed
// into the last frames, so they arrive with the score's whooshes at the
// section's end instead of creeping in.
const FLY = cubicBezier(0.3, 0.05, 0.55, 1);
const LAG = 0.0012;
// Motion blur as a 180° shutter: trailing samples over half a frame.
const SHUTTER = 0.5 / 60;
const BLUR_N = 8;

// The names launch on the break, a hair apart, and all land together, each
// on a shallow arc that diverges from the other two: "project" peels off to
// the upper left, "worktree" rises up the middle, bowing right to let it
// pass, and "CI" climbs to the upper right.
function flyers(lk: Lockup): Flyer[] {
  return [
    {
      glyphs: lk.proj.slice(0, 7),
      k: lk.k1,
      y: lk.y1,
      key: "project",
      lift: -90,
      bow: -60,
      bank: -0.16,
      delay: 0,
      dir: -1,
    },
    {
      glyphs: lk.work.slice(0, 8),
      k: lk.k2,
      y: lk.y2,
      key: "worktree",
      lift: 0,
      bow: 150,
      bank: 0.08,
      delay: 0.02,
      dir: 1,
    },
    {
      glyphs: lk.ci.slice(0, 2),
      k: lk.k3,
      y: lk.y3,
      key: "ci",
      lift: -40,
      bow: 60,
      bank: -0.08,
      delay: 0.008,
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
  const cxp = (x0 + a1) / 2 + f.bow;
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
  /** How fast it recedes into the frame; DEPTH by default. */
  depth?: number;
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
const LEAD_FILL = PALETTE.text2;

function fallers(lk: Lockup): Piece[] {
  const out: Piece[] = [];
  // The lead bursts: its letters are knocked up and out, spinning away into
  // depth, and are spent before "project" climbs through where it stood.
  const mid = lk.lx + lk.wl / 2;
  let n = 0;
  lk.lead.forEach((line, l) =>
    line.forEach((g) => {
      const side = (g.x - mid) / (lk.wl / 2);
      const i = n++;
      out.push({
        spec: LEAD_SPEC,
        ch: g.ch,
        w: g.w,
        x: g.x,
        y: lk.yl[l],
        k: 1,
        fill: LEAD_FILL,
        delay: hash(i, 9) * 0.008,
        vx: side * 900 + (hash(i, 3) - 0.5) * 500,
        vy: -(500 + hash(i, 5) * 700) + l * 250,
        spin: (hash(i, 7) - 0.5) * 22,
        gravity: 9000,
        depth: 7,
        hold: 0.015,
        life: 0.1,
      });
    }),
  );
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
    depth = DEPTH,
  ) => out.push({ spec, ch: g.ch, w: g.w, x: g.x, y, k, fill, delay, vx, vy, spin, gravity: G, depth, hold, life });
  const P = PALETTE.paper;
  // Both "s," pairs let go and tumble straight down, so the names pull away
  // from them instead of carrying them along. They are knocked back into
  // depth and spent before "CI" climbs through where they fall.
  heavy(lk.proj[7], lk.y1, lk.k1, SPEC, P, 0, 60, 40, 12, 0.04, 0.12, 5);
  heavy(lk.proj[8], lk.y1, lk.k1, SPEC, P, 0.012, 150, 0, 16, 0.04, 0.12, 5);
  heavy(lk.work[8], lk.y2, lk.k2, SPEC, P, 0.004, -40, 60, -11, 0.03, 0.1, 5);
  heavy(lk.work[9], lk.y2, lk.k2, SPEC, P, 0.014, 60, 20, 15, 0.03, 0.1, 5);
  // "and" lets go letter by letter, a frame apart; the period falls away
  // under CI. Nothing flies below them, so they leave through the bottom of
  // the frame.
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
function fall(p: Pick<Piece, "vx" | "vy" | "spin" | "gravity" | "depth" | "hold" | "life">, d: number) {
  return {
    dx: p.vx * d,
    dy: p.vy * d + 0.5 * p.gravity * d * d,
    rot: p.spin * d,
    s: 1 / (1 + (p.depth ?? DEPTH) * d),
    alpha: 1 - smoothstep(p.hold, p.life, d),
  };
}

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
    { t: T_IRIS, x: lk.lx, y: lk.yl[0] - 30, amp: 0.8 },
    { t: T_P, x: lk.lx + lk.wl / 2, y: lk.y1, amp: 0.55 },
    { t: T_W, x: lk.lx + lk.wl + 58, y: lk.y2, amp: 0.55 },
    { t: T_CI, x: lk.lx + lk.wl - 180, y: lk.y3 - 100, amp: 0.55 },
    { t: T_OUT, x: W / 2, y: H / 2, amp: 0.5 },
  ].filter((w) => lt > w.t && lt < w.t + 0.6);
  // Brighter while the iris opens, so the lens lands on a lit stage.
  const base = lerp(0.3, 0.14, smoothstep(T_IRIS + 0.1, T_IRIS + 0.4, lt));
  const s = 2.2 * c.z;
  ctx.save();
  ctx.fillStyle = PALETTE.text3;
  for (let gx = Math.ceil(x0 / GRID) * GRID; gx <= x1; gx += GRID) {
    const px = sx(c, gx);
    for (let gy = Math.ceil(y0 / GRID) * GRID; gy <= y1; gy += GRID) {
      const py = sy(c, gy);
      let a = base;
      for (const w of waves) {
        const r = (lt - w.t) * 2600;
        const d = Math.hypot(gx - w.x, gy - w.y) - r;
        a += w.amp * Math.exp(-(d * d) / 5000) * (1 - (lt - w.t) / 0.6) * smoothstep(0, 0.02, lt - w.t);
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

/** Whether the caret shows: solid while it types, then on for the first of every two beats (about 1 Hz, and on at b8). */
export function caretOn(lt: number): boolean {
  return lt < T_TYPED || Math.floor(lt / BEAT + 1e-9) % 2 === 0;
}

function drawWorld(ctx: CanvasRenderingContext2D, lt: number, lk: Lockup, bg: string, c: WorldCam): void {
  ctx.fillStyle = bg;
  ctx.fillRect(0, 0, W, H);

  // While CI is giant it owns the frame: everything else all but vanishes
  // for the three-frame hold and comes back on the snap.
  const giant = lt >= T_CI ? 1 - clamp(ciSnap(lt)) : 0;
  const dim = 1 - giant;
  const out = lt >= T_OUT;
  const loose = inQuad(progress(T_OUT - INHALE, T_OUT, lt));

  drawGrid(
    ctx,
    c,
    lk,
    lt,
    smoothstep(T_IRIS - 0.12, T_IRIS + 0.02, lt) * (1 - smoothstep(T_OUT + 0.05, T_OUT + 0.32, lt)) * dim,
  );

  // The iris lands on a lit prompt: a glow that rides the caret, and a
  // double ripple fixed in the world where the caret lands, so the
  // camera's track carries it off to the left. Both rings are spent before
  // the typing gets far.
  if (lt < T_P) {
    const cr = caretRect(c, lk, lt);
    const glowA = smoothstep(T_CARET - 0.12, T_CARET, lt) * (1 - smoothstep(T_TYPED - 0.1, T_P, lt));
    glow(ctx, cr.x, cr.y, 150 * (c.z / Z0), PALETTE.amber, 0.55 * glowA);
    const ex = sx(c, lk.lx - 4 - CARET_W / 2);
    const ey = sy(c, lk.yl[0] + 14 - CARET_H / 2);
    // The rings burst out already clear of the caret, as its click lands.
    const burst = (end: number) => {
      const p = progress(T_CARET, end, lt);
      return p > 0 ? lerp(0.12, 1, p) : 0;
    };
    ring(ctx, ex, ey, 380 * c.z, burst(T_CARET + 0.24), PALETTE.amberBright, 7);
    ring(ctx, ex, ey, 240 * c.z, burst(T_CARET + 0.2), PALETTE.amber, 4);
  }

  // Construction guides: baselines drawn with a pen tip, and the margins.
  ctx.save();
  ctx.lineCap = "round";
  const guides = [lk.yl[0], lk.yl[1], lk.y1, lk.y2, lk.y3];
  guides.forEach((y, i) => {
    const g0 = outCubic(progress(T_OUT + i * 0.012, T_OUT + 0.12 + i * 0.012, lt));
    const pen = progress(T_IRIS + i * 0.05, T_IRIS + 0.4 + i * 0.05, lt);
    const g1 = swiftOut(pen);
    if (g1 <= g0) return;
    const xa = lk.lx - 110;
    const xb = lk.lx + lk.wl + 110;
    // Retracting guides fade as they go, so no stub is left hanging.
    ctx.strokeStyle = rgba(PALETTE.divider, (i < 2 ? 0.6 : 0.95) * dim * (1 - g0));
    ctx.lineWidth = 1.5;
    ctx.beginPath();
    ctx.moveTo(sx(c, lerp(xa, xb, g0)), sy(c, y));
    ctx.lineTo(sx(c, lerp(xa, xb, g1)), sy(c, y));
    ctx.stroke();
    if (i >= 2 && i !== 3) {
      const hitA = pulse(lt, i === 2 ? T_P : T_CI, 0.004, 0.09) * dim;
      if (hitA > 0.01) {
        ctx.strokeStyle = rgba(PALETTE.amberBright, 0.7 * hitA);
        ctx.lineWidth = 2.5;
        ctx.stroke();
      }
    }
    if (g1 < 0.995) {
      const tx = sx(c, lerp(xa, xb, g1));
      glow(ctx, tx, sy(c, y), 26 * c.z, PALETTE.amber, 0.7 * (1 - g1) * smoothstep(0, 0.08, pen));
    }
  });
  for (const [i, x] of [lk.lx, lk.lx + lk.wl].entries()) {
    const g0 = outCubic(progress(T_OUT, T_OUT + 0.12, lt));
    const g1 = swiftOut(progress(T_IRIS + 0.08 + i * 0.06, T_IRIS + 0.48 + i * 0.06, lt));
    if (g1 <= g0) continue;
    const ya = lk.yl[0] - 110;
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

  // "projects,": letters drop in a fast cascade, one per DROP_STAG, and
  // squash on the main line. Drawn before the lead so they fall behind it.
  const FALL = 0.14;
  // Screen boxes of letters still dropping, for the lead's knockout.
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
      const tl = T_P + i * DROP_STAG;
      if (lt < tl - FALL) return;
      const u = progress(tl - FALL, tl, lt);
      let dy = -1100 * (1 - u * u);
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

  // The lead: each key lands warm and a few px low, then settles and cools.
  // The caret leads, solid while typing, then blinks in time.
  const leadDip = 12 * wobble(lt, T_P, 4, 10) * (1 - smoothstep(T_P + 0.4, T_P + 0.6, lt));
  if (!out) {
    for (const key of KEYS) {
      if (lt < key.t || key.i < 0) continue;
      const g = lk.lead[key.line][key.i];
      if (g.ch === " ") continue;
      const d = lt - key.t;
      const rise = 12 * (1 - outCubic(progress(0, 0.07, d))) - 3 * loose;
      const fill = mix(PALETTE.amberBright, LEAD_FILL, smoothstep(0.02, 0.16, d));
      const x = sx(c, g.x);
      const y = sy(c, lk.yl[key.line] + leadDip + rise);
      // A thin knockout only where a dropping letter passes behind.
      const hw = 0.5 * g.w * c.z;
      let over = false;
      for (const [a, b, e, f] of drops) {
        if (x + hw > a && x - hw < e && y + 12 * c.z > b && y - 60 * c.z < f) over = true;
      }
      glyph(ctx, LEAD_SPEC, g.ch, g.w, x, y, c.z, fill, {
        alpha: dim,
        halo: over ? { color: bg, width: 6 * c.z } : undefined,
      });
    }
    // Before T_CARET the caret is still the glint arc (drawDive draws it).
    if (lt >= T_CARET && caretOn(lt)) {
      const r = caretRect(c, lk, lt);
      stroke(ctx, caretPts({ ...r, y: r.y + leadDip * c.z }, 2), r.w, rgba(PALETTE.amber, dim));
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
  // beat, then snaps down beside it.
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
    const caret = { vx: 900, vy: -300, spin: 14, gravity: 9000, depth: 7, hold: 0.015, life: 0.1 };
    const f = fall(caret, Math.max(0, lt - T_OUT - 0.004));
    if (f.alpha > 0) {
      ctx.save();
      ctx.globalAlpha *= f.alpha;
      ctx.translate(cr.x + f.dx, cr.y + f.dy);
      ctx.rotate(f.rot);
      stroke(ctx, caretPts({ x: 0, y: 0, w: cr.w * f.s, h: cr.h * f.s }, 2), cr.w * f.s, PALETTE.amber);
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
  const tip = outBack(2.6)(progress(MAIN_TIP - 0.01, MAIN_TIP + 0.08, lt)) * gone;
  const tipHot = 0.8 * pulse(lt, MAIN_TIP, 0.004, 0.06);
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

/** Snap progress for CI: 0 while giant, overshoots past 1, settles at 1 by b4.5. */
function ciSnap(lt: number): number {
  if (lt < T_SNAP) return 0;
  const a = progress(T_SNAP, T_SNAP + 0.09, lt);
  if (a < 1) return 1.05 * outCubic(a);
  return lerp(1.05, 1, cubicBezier(0.45, 0, 0.55, 1)(progress(T_SNAP + 0.09, T_DONE - 0.01, lt)));
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
    if (lt < T_IRIS + 0.25) drawDive(ctx, lt, lk);
    else drawWorld(ctx, lt, lk, PALETTE.bg, worldCam(lt, lk));
    ctx.restore();
  },
};
