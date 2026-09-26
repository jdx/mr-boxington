// Section 10, "CI". Mr Boxington hops left into a corner of his `your
// machine` frame, which dims: CI happens elsewhere. A remote cache drops in
// over the two runners six-builds left, and its backends land beside it
// (the GitHub Actions cache, a cache server, an S3 bucket). The `push to
// main` runner throws its compiled outputs up to the remote's shelf, amber
// in flight and stored on landing. The `pull request` runner gets green
// copies down, restored, without a glint: in the Action's default archive
// mode Cargo reuses the restored target/ directly, so no mbx hit happens.
// Its own compile, thrown up, bounces off the read-only line over it.
// Each runner's rack lights blink amber while it compiles and green while
// it restores, and in the caption's hold a light runs along the shelf.
// The section ends on the whip pan into the chart (map.ts whipOut and
// drawWhip), still moving on its last frame.
//
// Nothing here depends on the benchmark facts.

import { BEAT, PALETTE, type Scene, type SceneEnv, sec, WHIP } from "../bible";
import { LOGO_FACE, type LogoFace } from "../box";
import { rgba } from "../color";
import { glow, ring, roundedRect } from "../fx";
import {
  arc,
  boxPose,
  type BoxSpot,
  chipRect,
  CI,
  type Curve,
  curveAt,
  drawCarton,
  drawChip,
  drawFrame,
  drawMapBox,
  drawReadOnly,
  drawRemote,
  drawRunner,
  drawWhip,
  drawWhipOut,
  FLOOR,
  lerpRect,
  lerpSpot,
  MACHINE,
  popIn,
  type Pt,
  type Rect,
  remoteSlots,
  type RunnerState,
  SB_END,
  typedChars,
  WHIP_AT,
  WHIP_WIND,
} from "../map";
import { hash, inQuad, lerp, progress, pulse, smoothstep, swiftIn, swiftInOut, swiftOut, TAU, wobble } from "../math";
import { type Caption, drawText, font, MONO } from "../type";

const S = sec("ci");

/** Local time of beat `n` of the section. */
const b = (n: number): number => n * BEAT;

// The beats, in section-local seconds. The score (score/ci.ts) places its
// cues on these.

/** Mr Boxington crouches, leaps, and lands in his corner. */
export const T_CROUCH = 0;
export const T_LEAP = b(0.25);
export const T_LAND = b(0.875);
/** The remote falls in, and lands. */
export const T_DROP = b(0.5);
export const T_REMOTE = b(1);
/** Its backends pop in, the last one landed by b1.5. */
export const CHIPS = [b(0.875), b(1.125), b(1.375)] as const;
/** `uses: jdx/mr-boxington-action@v1` types. */
export const T_USES = b(1.5);
/** push to main: each compiled output leaves the runner on a sixteenth, and lands on the shelf. */
export const UPLOADS = Array.from({ length: 8 }, (_, k) => b(1.5 + k * 0.25));
export const UPLOAD_FLY = 0.34;
/** The read-only line draws across the pull request's approach. */
export const T_LOCK = b(4);
/** pull request: each restored copy leaves the shelf, and lands on the runner. */
export const RESTORES = [4.5, 4.875, 5.25, 5.625, 6.625, 6.875, 7.125].map(b);
export const RESTORE_FLY = 0.3;
/** Its own compile: thrown up, stopped by the read-only line, back down. */
export const T_THROW = b(6);
export const T_BOUNCE = b(6.25);
export const T_BACK = b(6.625);
/** Under the caption's hold: a light runs along the shelf. */
export const T_SHIMMER = b(9);
/** The wind-up before the whip (map.ts), section-local. */
export const T_WIND = WHIP_AT - WHIP - WHIP_WIND - S.start;

const CAPTIONS: readonly Caption[] = [
  {
    out: 11.75,
    lines: [
      { in: 3.5, text: "In CI, pushes to main publish." },
      { in: 5, text: "Pull requests only restore." },
    ],
  },
];

// Layout, map px at HOME.

/** Where he lands: the kit's corner of the frame. */
const HOP = CI.hop;
/** The remote over the runners, its backends stacked to its left, right-aligned. */
const REMOTE: Rect = { x: 1200, y: 40, w: 680, h: 240 };
const SLOTS = remoteSlots(REMOTE);
const BACKENDS = ["GitHub Actions cache", "cache server", "S3 bucket"] as const;
const CHIP_RIGHT = REMOTE.x - 24;
const CHIP_Y = [40, 126, 212] as const;
/** The runners settle a little lower than six-builds leaves them, to make room over them. */
const RUNNER_TOP = 470;
const RUNNERS = (SB_END.runners ?? []).map((r) => ({ ...r, to: { ...r.rect, y: RUNNER_TOP, h: FLOOR - RUNNER_TOP } }));
/** Where outputs leave the push runner and land on the pull request runner. */
const PUSH_PORT: Pt = { x: 1340, y: RUNNER_TOP };
const PR_PORT: Pt = { x: 1660, y: RUNNER_TOP };
const LOCK = { x0: 1440, x1: 1880, y: 350 };
const USES = { x: 532, y: 440, text: "uses: jdx/mr-boxington-action@v1" };
const CARTON = 44;
/** The floor's dust. */
const DUST = "#9a8a6c";

/** His spot through the hop: a crouch, an arc left, a squashed landing. */
function hop(lt: number): { spot: BoxSpot; squash: number; air: number } {
  const p = progress(T_LEAP, T_LAND, lt);
  const along = swiftInOut(p);
  const spot = lerpSpot(MACHINE.box, HOP.box, along);
  const air = Math.sin(Math.PI * p);
  // From rest on the bar line: the crouch eases in, and springs open over the first frames of the leap.
  const crouch = 1 - 0.12 * (lt < T_LEAP ? smoothstep(T_CROUCH, T_LEAP, lt) : 1 - swiftOut(progress(T_LEAP, T_LEAP + 0.05, lt)));
  const stretch = lt >= T_LEAP && lt < T_LAND ? 1 + 0.1 * Math.sin(Math.PI * Math.min(1, p * 2)) : 1;
  const land = lt >= T_LAND ? 1 - 0.16 * Math.exp(-(lt - T_LAND) / 0.09) * Math.cos((lt - T_LAND) * 26) : 1;
  return { spot: { ...spot, y: FLOOR - 150 * air }, squash: crouch * stretch * land, air };
}

/** Where he looks: at the uploads, then down at the pull request, and a blink in the hold. */
function face(lt: number): LogoFace {
  const up = swiftOut(progress(T_LAND, T_LAND + 0.3, lt));
  const pr = swiftInOut(progress(RESTORES[0] - 0.2, RESTORES[0], lt));
  const home = swiftInOut(progress(b(9.5), b(10), lt));
  const look: [number, number] = [lerp(lerp(0, 3, up), 3, pr) * (1 - home) + 1 * home, lerp(lerp(0, -3, up), 0, pr) * (1 - home)];
  const blink = lt >= b(9) && lt < b(9) + 0.16 ? 1 : 0;
  // A wince as the pull request's compile hits the line.
  const wince = smoothstep(T_BOUNCE, T_BOUNCE + 0.04, lt) * (1 - smoothstep(T_BOUNCE + 0.2, T_BOUNCE + 0.4, lt));
  return { ...LOGO_FACE, look, blink, eyelid: 0.6 * wince, twitch: 0.2 * wobble(lt, T_LAND, 5, 7) };
}

/** Dust kicked out both ways along the floor from a landing `age` seconds ago. */
function dust(ctx: CanvasRenderingContext2D, x: number, y: number, w: number, age: number): void {
  if (age < 0 || age > 0.45) return;
  const p = age / 0.45;
  ctx.save();
  for (let i = 0; i < 10; i++) {
    const side = i % 2 ? 1 : -1;
    const k = 0.35 + 0.65 * ((i * 0.37) % 1);
    const px = x + side * (w * 0.45 + w * 0.5 * k * swiftOut(p));
    const py = y - 6 - 26 * k * swiftOut(p) * (1 - p * 0.5);
    ctx.fillStyle = rgba(DUST, 0.3 * (1 - p) ** 1.5);
    ctx.beginPath();
    ctx.arc(px, py, 6 + 18 * k * swiftOut(p), 0, TAU);
    ctx.fill();
  }
  ctx.restore();
}

/** A carton in flight with two fading copies behind it along its curve. */
function flying(
  ctx: CanvasRenderingContext2D,
  k: Curve,
  u: number,
  lag: number,
  size: number,
  kind: "compiled" | "restored",
  o: { rot?: number; alpha?: number; lit?: number } = {},
): void {
  for (const [i, a] of [[2, 0.18], [1, 0.35]] as const) {
    const q = Math.max(0, u - lag * i);
    if (q >= u) continue;
    const at = curveAt(k, q);
    drawCarton(ctx, at.x, at.y, size, kind, { rot: o.rot, alpha: a * (o.alpha ?? 1) });
  }
  const at = curveAt(k, u);
  drawCarton(ctx, at.x, at.y, size, kind, o);
}

/** An upload in flight: from the push runner up to its shelf slot, tumbling. */
function upload(k: number): Curve {
  const s = SLOTS[k];
  return arc(PUSH_PORT, { x: s.x, y: s.y }, 0.28);
}

/** A restore in flight: a copy peels off its slot and drops to the pull request runner. */
function restore(k: number): Curve {
  const s = SLOTS[k];
  return { a: { x: s.x, y: s.y }, c: { x: lerp(s.x, PR_PORT.x, 0.25), y: s.y - 90 }, b: PR_PORT };
}

/**
 * A runner's rack lights while it works, over drawRunner's dark ones (its
 * geometry): amber while it compiles, green while it restores, blinking at
 * `act` 0..1.
 */
function rackLights(ctx: CanvasRenderingContext2D, r: Rect, act: number, color: string, t: number, seed: number): void {
  if (act <= 0.05) return;
  const uh = (r.h - 116) / 2;
  const fr = Math.floor(t * 30);
  ctx.save();
  ctx.fillStyle = color;
  for (let u = 0; u < 2; u++) {
    const mid = r.y + 96 + u * (uh + 8) + uh / 2;
    for (let i = 0; i < 8; i++) {
      if (hash(fr * 7 + i + u * 31, seed) < 0.3 + 0.7 * act) ctx.fillRect(r.x + 44 + i * 22, mid - 6, 12, 12);
    }
  }
  ctx.restore();
}

/** A light running along the remote's shelf, once, at `p` 0..1. */
function shimmer(ctx: CanvasRenderingContext2D, p: number): void {
  if (p <= 0 || p >= 1) return;
  const x = lerp(REMOTE.x - 120, REMOTE.x + REMOTE.w + 120, swiftInOut(p));
  ctx.save();
  roundedRect(ctx, REMOTE.x, REMOTE.y, REMOTE.w, REMOTE.h, 24);
  ctx.clip();
  ctx.globalCompositeOperation = "lighter";
  const g = ctx.createLinearGradient(x - 110, 0, x + 110, 0);
  g.addColorStop(0, rgba(PALETTE.paper, 0));
  g.addColorStop(0.5, rgba(PALETTE.paper, 0.14 * Math.sin(Math.PI * p)));
  g.addColorStop(1, rgba(PALETTE.paper, 0));
  ctx.fillStyle = g;
  ctx.fillRect(x - 110, REMOTE.y, 220, REMOTE.h);
  ctx.restore();
}

function content(ctx: CanvasRenderingContext2D, lt: number, t: number): void {
  // The frame closes in on his corner and dims.
  const close = swiftInOut(progress(T_LEAP, T_LAND + 0.05, lt));
  const frame = lerpRect(CI.start.frame, HOP.frame, close);
  drawFrame(ctx, { rect: frame, label: "your machine", dim: smoothstep(b(0.5), b(1.5), lt) });

  const h = hop(lt);
  drawMapBox(ctx, { spot: h.spot, pose: boxPose({ tape: 1, face: face(lt), squash: h.squash }), shadow: 1 - 0.7 * h.air });
  dust(ctx, HOP.box.x, FLOOR, HOP.box.w, lt - T_LAND);

  // The remote falls in and bounces; its shelf fills as uploads land.
  if (lt >= T_DROP) {
    const fall = lt < T_REMOTE ? -380 * (1 - inQuad(progress(T_DROP, T_REMOTE, lt))) : -16 * Math.max(0, wobble(lt, T_REMOTE, 4, 10));
    const landed = UPLOADS.filter((u) => lt >= u + UPLOAD_FLY).length;
    let lit = pulse(lt, T_REMOTE, 0.01, 0.2);
    for (const u of UPLOADS) lit = Math.max(lit, 0.7 * pulse(lt, u + UPLOAD_FLY, 0.005, 0.12));
    drawRemote(ctx, { rect: { ...REMOTE, y: REMOTE.y + fall }, fill: landed / SLOTS.length, lit });
    shimmer(ctx, progress(T_SHIMMER, T_SHIMMER + b(1.5), lt));
    if (lt >= T_REMOTE && lt < T_REMOTE + 0.5) {
      const p = progress(T_REMOTE, T_REMOTE + 0.5, lt);
      glow(ctx, REMOTE.x + REMOTE.w / 2, REMOTE.y + REMOTE.h, 260, PALETTE.paper, 0.25 * (1 - p) ** 2);
    }
  }

  // Its backends, any one of which it can be.
  BACKENDS.forEach((text, i) => {
    const s = popIn(lt, CHIPS[i]);
    if (s <= 0) return;
    const w = chipRect(ctx, { text, x: 0, y: 0, size: 56, mono: false }).w;
    drawChip(ctx, { text, x: CHIP_RIGHT - w, y: CHIP_Y[i], size: 56, mono: false, scale: s, lit: pulse(lt, CHIPS[i] + 0.08, 0.01, 0.2) });
  });

  // The Action both runners use.
  const shown = typedChars(USES.text, lt, T_USES, b(0.75));
  if (shown > 0) drawText(ctx, USES.text.slice(0, shown), USES.x, USES.y, { font: font(40, 500, MONO), fill: PALETTE.text3 });

  // The runners settle lower, busy while they build.
  const settle = swiftInOut(progress(T_LEAP, T_LAND + 0.1, lt));
  RUNNERS.forEach((r, i) => {
    const busy = i === 0 ? [UPLOADS[0], b(3.75)] : [RESTORES[0], b(8)];
    const blocked = i === 1 && lt >= T_BOUNCE && lt < T_BOUNCE + 0.22;
    const led: RunnerState["led"] = blocked ? "blocked" : lt >= busy[1] ? "ok" : lt >= busy[0] ? "busy" : "idle";
    const activity = lt >= busy[0] && lt < busy[1] ? 1 : lt >= busy[1] ? 0.12 : 0;
    const rect = lerpRect(r.rect, r.to, settle);
    drawRunner(ctx, { rect, label: r.label, led, t, activity: 0 });
    // Amber while main compiles; green while the pull request restores, amber for its own compile.
    const own = i === 1 && lt >= T_THROW - 0.2 && lt < T_BACK;
    rackLights(ctx, rect, activity, i === 0 || own ? PALETTE.amber : PALETTE.green, t, 91 + i);
  });

  // The read-only line over the pull request runner.
  const lock = smoothstep(T_LOCK, T_LOCK + 0.15, lt);
  if (lock > 0) {
    const flash = pulse(lt, T_BOUNCE, 0.005, 0.18);
    const shakeX = 5 * wobble(lt, T_BOUNCE, 9, 12);
    drawReadOnly(ctx, LOCK.x0 + shakeX, LOCK.x1 + shakeX, LOCK.y - 20 * (1 - lock), lock, flash);
  }

  // Uploads: amber up, tumbling, stored on the shelf.
  UPLOADS.forEach((u, k) => {
    const p = progress(u, u + UPLOAD_FLY, lt);
    if (lt < u || p >= 1) return;
    const size = lerp(70, CARTON, swiftIn(p));
    flying(ctx, upload(k), p, 0.07, size, "compiled", { rot: (1 - swiftOut(p)) * TAU * 0.5 * (k % 2 ? 1 : -1), lit: 0.5 });
  });
  // Restores: green copies down, no glint.
  RESTORES.forEach((r, k) => {
    const p = progress(r, r + RESTORE_FLY, lt);
    const after = lt - (r + RESTORE_FLY);
    if (lt < r || after > 0.14) return;
    if (p < 1) {
      flying(ctx, restore(k), inQuad(p), 0.06, lerp(CARTON, 70, p), "restored", { alpha: smoothstep(0, 0.1, p) });
    } else {
      // Into the runner: squashed on landing, gone.
      const q = after / 0.14;
      drawCarton(ctx, PR_PORT.x, PR_PORT.y, 70, "restored", { sy: 1 - 0.6 * q, sx: 1 + 0.2 * q, alpha: 1 - q });
    }
  });
  // The pull request's own compile: up, stopped, back down.
  if (lt >= T_THROW && lt < T_BACK + 0.14) {
    const top = LOCK.y + 8;
    const size = 56;
    const hit = top + size * 0.92;
    let y: number;
    let sy = 1;
    if (lt < T_BOUNCE) y = lerp(PR_PORT.y, hit, swiftOut(progress(T_THROW, T_BOUNCE, lt)));
    else if (lt < T_BACK) {
      const q = progress(T_BOUNCE, T_BACK, lt);
      y = lerp(hit, PR_PORT.y, inQuad(q));
      sy = 1 - 0.25 * Math.exp(-(lt - T_BOUNCE) / 0.05);
    } else y = PR_PORT.y;
    const gone = progress(T_BACK, T_BACK + 0.14, lt);
    drawCarton(ctx, PR_PORT.x - 80, y, size, "compiled", { sy: sy * (1 - 0.5 * gone), alpha: 1 - gone, rot: 0.2 * wobble(lt, T_BOUNCE, 4, 8) });
    if (lt >= T_BOUNCE) ring(ctx, PR_PORT.x - 80, top, 70, progress(T_BOUNCE, T_BOUNCE + 0.35, lt), PALETTE.paper, 4);
  }
}

function draw(ctx: CanvasRenderingContext2D, lt: number, env: SceneEnv): void {
  // The whip pan: winding up, then smeared out to the left past the bar
  // line, the streaks over it (map.ts); the chart arrives on them.
  if (lt >= T_WIND) {
    drawWhipOut(ctx, env.t, () => content(ctx, lt, env.t));
    drawWhip(ctx, env.t);
    return;
  }
  content(ctx, lt, env.t);
}

export const scene: Scene = {
  id: S.id,
  start: S.start,
  end: S.end,
  draw,
  captions: () => CAPTIONS,
};

