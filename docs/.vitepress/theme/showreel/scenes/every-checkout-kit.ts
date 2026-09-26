// Small pieces every-checkout and under-cargo-build share on top of the map
// kit (map.ts): a terminal's scrolling log drawn exactly as a pane draws its
// lines, a slab tossed through the air, the dust a landing kicks up, and
// sparks gathering on the spot where something is about to pop in. All of
// it is a pure function of its arguments.

import { PALETTE } from "../bible";
import { rgba } from "../color";
import { glow } from "../fx";
import { curveAt, drawSlab, type PaneLayout, type PaneState, type Pt, SLAB_H } from "../map";
import { clamp, hash, lerp, progress, TAU } from "../math";
import { drawText, font, layout, MONO } from "../type";

/** One row of a log: `Compiling <crate>`, where it sits and how far it has faded in. */
export interface LogRow {
  crate: string;
  /** Line slot under the prompt, fractional while it scrolls; below 0 it has scrolled away. */
  slot: number;
  alpha: number;
  /** Still rising into place, px. */
  dy?: number;
}

/**
 * A pane's `Compiling` rows at fractional slots, painted with the same calls
 * drawPane makes for its `compile` lines, so a scene can scroll a log and
 * hand the settled rows back to drawPane without a pixel changing. Rows are
 * clipped under the prompt, where they scroll away. Draw it after drawPane,
 * on a pane standing fully open.
 */
export function drawLog(ctx: CanvasRenderingContext2D, p: PaneState, L: PaneLayout, rows: readonly LogRow[]): void {
  const a = p.alpha ?? 1;
  if (a <= 0) return;
  const text = p.text ?? 56;
  const bold = font(text, 700, MONO);
  const head = "Compiling ";
  const top = L.prompt.y + text * 0.28;
  ctx.save();
  ctx.globalAlpha *= a * (1 - 0.6 * (p.dim ?? 0));
  ctx.beginPath();
  ctx.rect(L.window.x, top, L.window.w, L.window.y + L.window.h - top);
  ctx.clip();
  for (const r of rows) {
    const la = r.alpha * clamp(1 + r.slot);
    if (la <= 0) continue;
    const y = L.line.y + r.slot * L.lineStep + (r.dy ?? 0);
    ctx.save();
    ctx.globalAlpha *= la;
    drawText(ctx, head, L.line.x, y, { font: bold, fill: PALETTE.amber });
    drawText(ctx, r.crate, L.line.x + layout(ctx, head, bold).width, y, { font: font(text, 500, MONO), fill: PALETTE.text2 });
    ctx.restore();
  }
  ctx.restore();
}

/** The width of `Compiling <crate>` in a pane's log. */
export function rowWidth(ctx: CanvasRenderingContext2D, crate: string, text = 56): number {
  return layout(ctx, "Compiling ", font(text, 700, MONO)).width + layout(ctx, crate, font(text, 500, MONO)).width;
}

/**
 * The point `u` (0..1 of its length) along a thread through `pts`, sagging
 * between them as map.ts drawThread draws it.
 */
export function threadPoint(pts: readonly Pt[], u: number): Pt {
  const path: Pt[] = [];
  for (let i = 0; i < pts.length - 1; i++) {
    const s = pts[i];
    const e = pts[i + 1];
    const c = { x: (s.x + e.x) / 2, y: (s.y + e.y) / 2 + Math.hypot(e.x - s.x, e.y - s.y) * 0.08 };
    for (let j = i === 0 ? 0 : 1; j <= 16; j++) path.push(curveAt({ a: s, c, b: e }, j / 16));
  }
  const acc = [0];
  for (let i = 1; i < path.length; i++) acc.push(acc[i - 1] + Math.hypot(path[i].x - path[i - 1].x, path[i].y - path[i - 1].y));
  const L = acc[acc.length - 1] * clamp(u);
  for (let i = 1; i < path.length; i++) {
    if (acc[i] >= L) {
      const f = (L - acc[i - 1]) / (acc[i] - acc[i - 1] || 1);
      return { x: lerp(path[i - 1].x, path[i].x, f), y: lerp(path[i - 1].y, path[i].y, f) };
    }
  }
  return path[path.length - 1];
}

/** A slab's resting place: its top left, width and turn (map.ts towerSlabs, or a pane's slab). */
export interface SlabAt {
  x: number;
  y: number;
  w: number;
  rot: number;
}

/**
 * A slab thrown from `a` to `b` at `u` (0..1 of its flight, time-linear): it
 * moves across at an even speed and rises and falls on a parabola peaking
 * near `apex` (the top's y), tumbling a little and taking its new width on
 * the way.
 */
export function tossed(a: SlabAt, b: SlabAt, u: number, apex: number, tumble = 0.35): SlabAt {
  // A Bézier control for y that puts the parabola's peak about at `apex`.
  const c = 2 * apex - (a.y + b.y) / 2;
  const v = 1 - u;
  return {
    x: lerp(a.x, b.x, u),
    y: v * v * a.y + 2 * u * v * c + u * u * b.y,
    w: lerp(a.w, b.w, u * u * (3 - 2 * u)),
    rot: lerp(a.rot, b.rot, u) + tumble * Math.sin(Math.PI * u),
  };
}

/**
 * A slab drawn squashed about its base's center (`squash` > 0 flattens it),
 * as it takes a landing.
 */
export function drawSquashedSlab(
  ctx: CanvasRenderingContext2D,
  s: SlabAt,
  name: string,
  squash: number,
  o: { alpha?: number; tag?: boolean; lit?: number } = {},
): void {
  ctx.save();
  if (squash) {
    const cx = s.x + s.w / 2;
    const base = s.y + SLAB_H;
    ctx.translate(cx, base);
    ctx.scale(1 + 0.6 * squash, 1 - squash);
    ctx.translate(-cx, -base);
  }
  drawSlab(ctx, s.x, s.y, s.w, name, { rot: s.rot, ...o });
  ctx.restore();
}

/**
 * The dust a landing kicks up at `at`: flecks of cardboard thrown out
 * sideways from a floor contact `w` wide centered on (x, y), arcing and
 * fading. `seed` varies the throw.
 */
export function dust(ctx: CanvasRenderingContext2D, x: number, y: number, w: number, t: number, at: number, seed: number, amount = 1): void {
  const d = t - at;
  const life = 0.55;
  if (d <= 0 || d >= life || amount <= 0) return;
  const n = Math.round(14 * amount);
  ctx.save();
  for (let i = 0; i < n; i++) {
    const side = i % 2 ? 1 : -1;
    const k = hash(i, seed);
    const sx = x + side * (w / 2) * (0.55 + 0.45 * hash(i, seed + 1));
    const vx = side * (160 + 260 * k);
    const vy = -(90 + 200 * hash(i, seed + 2));
    const px = sx + vx * d * (1 - d / (2 * life));
    const py = y - 4 + vy * d + 900 * d * d;
    if (py > y + 2) continue;
    const fade = 1 - d / life;
    const r = (2 + 3.5 * hash(i, seed + 3)) * (0.6 + 0.4 * fade);
    ctx.fillStyle = rgba(i % 3 ? "#8a7350" : PALETTE.amberBright, 0.75 * fade * fade);
    ctx.beginPath();
    ctx.arc(px, py, r, 0, TAU);
    ctx.fill();
  }
  // A low puff along the floor.
  const puff = Math.sin(Math.PI * clamp(d / 0.35)) * amount;
  if (puff > 0) {
    ctx.save();
    ctx.translate(x, y);
    ctx.scale(1, 0.22);
    const r = w * (0.55 + 0.35 * clamp(d / 0.35));
    const g = ctx.createRadialGradient(0, 0, 0, 0, 0, r);
    g.addColorStop(0, rgba("#b39870", 0.18 * puff));
    g.addColorStop(1, rgba("#b39870", 0));
    ctx.fillStyle = g;
    ctx.beginPath();
    ctx.arc(0, 0, r, 0, TAU);
    ctx.fill();
    ctx.restore();
  }
  ctx.restore();
}

/**
 * Sparks spiralling in on (x, y) from about `r` px out, arriving at `at`
 * after `lead` seconds: the gather before something pops in there.
 */
export function gather(ctx: CanvasRenderingContext2D, x: number, y: number, r: number, t: number, at: number, lead: number, seed = 7): void {
  const u = progress(at - lead, at, t);
  if (u <= 0 || u >= 1) return;
  const n = 10;
  ctx.save();
  for (let i = 0; i < n; i++) {
    const delay = 0.25 * hash(i, seed);
    const k = progress(delay, 1, u);
    if (k <= 0) continue;
    const e = k * k;
    const a0 = (i / n) * TAU + hash(i, seed + 1);
    const rr = r * (0.7 + 0.5 * hash(i, seed + 2)) * (1 - e);
    const ang = a0 + 2.2 * e;
    const px = x + Math.cos(ang) * rr;
    const py = y + Math.sin(ang) * rr * 0.8;
    // A short tail back along the spiral.
    const tail = 0.12;
    const e0 = Math.max(0, k - tail) ** 2;
    const qx = x + Math.cos(a0 + 2.2 * e0) * r * (0.7 + 0.5 * hash(i, seed + 2)) * (1 - e0);
    const qy = y + Math.sin(a0 + 2.2 * e0) * r * (0.7 + 0.5 * hash(i, seed + 2)) * (1 - e0) * 0.8;
    const col = i % 3 ? PALETTE.amberBright : PALETTE.paper;
    ctx.strokeStyle = rgba(col, 0.55 * Math.min(1, k * 3));
    ctx.lineWidth = 3;
    ctx.lineCap = "round";
    ctx.beginPath();
    ctx.moveTo(qx, qy);
    ctx.lineTo(px, py);
    ctx.stroke();
    glow(ctx, px, py, 26, col, 0.5 * Math.min(1, k * 3));
  }
  ctx.restore();
}
