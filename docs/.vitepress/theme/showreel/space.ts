// A small 3D camera for the reel. The logo is a one-point perspective from
// straight ahead (box.ts logoCam: yaw 0, pitch 0, the eye 5.5 box widths out
// and 0.75 above the lid); the isometric world is an orthographic view from
// 45° azimuth and 30° elevation. mixCamera flies between the two, and either
// can tilt or change perspective between shots.

import { mix } from "./color";
import { lerp, remap } from "./math";

export type V3 = [number, number, number];

export interface Camera {
  /** Screen position (logical px) of `target`. */
  cx: number;
  cy: number;
  /** Logical px per world unit at the target's depth. */
  scale: number;
  /** Viewer azimuth around +Y, radians. 45° looks at the +X/+Z corner. */
  yaw: number;
  /** Viewer elevation, radians. 30° is the logo; 90° looks straight down. */
  pitch: number;
  /** Screen-space rotation, radians. */
  roll?: number;
  /** Viewer distance in world units for perspective; 0 or absent is orthographic. */
  persp?: number;
  /** World point drawn at (cx, cy). */
  target?: V3;
}

export interface Projected {
  x: number;
  y: number;
  /** Distance toward the viewer; larger is nearer. */
  z: number;
  /** Perspective magnification at this point (1 when orthographic). */
  f: number;
}

export const add = (a: V3, b: V3): V3 => [a[0] + b[0], a[1] + b[1], a[2] + b[2]];
export const sub = (a: V3, b: V3): V3 => [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
export const mul = (a: V3, s: number): V3 => [a[0] * s, a[1] * s, a[2] * s];
export const dot = (a: V3, b: V3): number => a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
export const cross = (a: V3, b: V3): V3 => [
  a[1] * b[2] - a[2] * b[1],
  a[2] * b[0] - a[0] * b[2],
  a[0] * b[1] - a[1] * b[0],
];
export const len = (a: V3): number => Math.hypot(a[0], a[1], a[2]);
export const norm = (a: V3): V3 => {
  const l = len(a) || 1;
  return [a[0] / l, a[1] / l, a[2] / l];
};
export const mix3 = (a: V3, b: V3, t: number): V3 => [
  lerp(a[0], b[0], t),
  lerp(a[1], b[1], t),
  lerp(a[2], b[2], t),
];

/** Rotate `p` around the unit `axis` through `pivot` by `angle` (Rodrigues). */
export function rotateAround(p: V3, pivot: V3, axis: V3, angle: number): V3 {
  const v = sub(p, pivot);
  const c = Math.cos(angle);
  const s = Math.sin(angle);
  const k = axis;
  const kv = cross(k, v);
  const kd = dot(k, v) * (1 - c);
  return [
    pivot[0] + v[0] * c + kv[0] * s + k[0] * kd,
    pivot[1] + v[1] * c + kv[1] * s + k[1] * kd,
    pivot[2] + v[2] * c + kv[2] * s + k[2] * kd,
  ];
}

/** Interpolate every numeric field of two cameras. */
export function mixCamera(a: Camera, b: Camera, t: number): Camera {
  const ta = a.target ?? [0, 0, 0];
  const tb = b.target ?? [0, 0, 0];
  return {
    cx: lerp(a.cx, b.cx, t),
    cy: lerp(a.cy, b.cy, t),
    scale: lerp(a.scale, b.scale, t),
    yaw: lerp(a.yaw, b.yaw, t),
    pitch: lerp(a.pitch, b.pitch, t),
    roll: lerp(a.roll ?? 0, b.roll ?? 0, t),
    // Blend perspective through its inverse so ortho (0) mixes smoothly.
    persp: (() => {
      const ia = a.persp ? 1 / a.persp : 0;
      const ib = b.persp ? 1 / b.persp : 0;
      const i = lerp(ia, ib, t);
      return i > 1e-6 ? 1 / i : 0;
    })(),
    target: mix3(ta, tb, t),
  };
}

/** A camera with its basis precomputed; build one per frame per camera. */
export class View {
  readonly cam: Camera;
  /** Screen right, screen up, and toward-viewer unit vectors in world space. */
  readonly r: V3;
  readonly u: V3;
  readonly v: V3;
  private readonly cr: number;
  private readonly sr: number;
  private readonly t: V3;

  constructor(cam: Camera) {
    this.cam = cam;
    const cy = Math.cos(cam.yaw);
    const sy = Math.sin(cam.yaw);
    const cp = Math.cos(cam.pitch);
    const sp = Math.sin(cam.pitch);
    this.r = [cy, 0, -sy];
    this.u = [-sp * sy, cp, -sp * cy];
    this.v = [cp * sy, sp, cp * cy];
    this.cr = Math.cos(cam.roll ?? 0);
    this.sr = Math.sin(cam.roll ?? 0);
    this.t = cam.target ?? [0, 0, 0];
  }

  project(p: V3): Projected {
    const d0 = p[0] - this.t[0];
    const d1 = p[1] - this.t[1];
    const d2 = p[2] - this.t[2];
    const x = d0 * this.r[0] + d1 * this.r[1] + d2 * this.r[2];
    const y = d0 * this.u[0] + d1 * this.u[1] + d2 * this.u[2];
    const z = d0 * this.v[0] + d1 * this.v[1] + d2 * this.v[2];
    const D = this.cam.persp ?? 0;
    const f = D > 0 ? D / Math.max(D - z, 1e-3) : 1;
    const sx = x * f * this.cam.scale;
    const sy = -y * f * this.cam.scale;
    return {
      x: this.cam.cx + sx * this.cr - sy * this.sr,
      y: this.cam.cy + sx * this.sr + sy * this.cr,
      z,
      f,
    };
  }

  /** Whether a surface with outward `normal` at `at` faces the viewer. */
  facing(normal: V3, at: V3): boolean {
    const D = this.cam.persp ?? 0;
    if (D <= 0) return dot(normal, this.v) > 1e-4;
    const eye = add(this.t, mul(this.v, D));
    return dot(normal, sub(eye, at)) > 1e-4;
  }

  /** Normal expressed in camera space: [right, up, toward viewer]. */
  toCamera(n: V3): V3 {
    return [dot(n, this.r), dot(n, this.u), dot(n, this.v)];
  }

  /**
   * Canvas transform mapping a planar local 2D frame into screen space:
   * local (x, y) lands at `origin + x * xAxis + y * yAxis` in the world.
   * Exact for orthographic cameras and for planes facing a perspective
   * camera square on (the logo's front); otherwise linearized at `origin`.
   */
  planeMatrix(origin: V3, xAxis: V3, yAxis: V3): DOMMatrix2D {
    const o = this.project(origin);
    const px = this.project(add(origin, xAxis));
    const py = this.project(add(origin, yAxis));
    return {
      a: px.x - o.x,
      b: px.y - o.y,
      c: py.x - o.x,
      d: py.y - o.y,
      e: o.x,
      f: o.y,
    };
  }
}

export interface DOMMatrix2D {
  a: number;
  b: number;
  c: number;
  d: number;
  e: number;
  f: number;
}

/** Apply a plane matrix on top of the context's current transform. */
export function applyMatrix(ctx: CanvasRenderingContext2D, m: DOMMatrix2D): void {
  ctx.transform(m.a, m.b, m.c, m.d, m.e, m.f);
}

// Lighting: a key light fixed in camera space (upper left, in front), so the
// top face reads lightest, the left panel mid, and the right panel darkest in
// the isometric view, and faces keep that relationship as objects turn.
const LIGHT: V3 = norm([-0.35, 0.8, 0.5]);

/** Tone of a surface, 0 (the isometric right panel) to 1 (a top face). */
export function tone(view: View, normal: V3): number {
  const n = view.toCamera(normal);
  return remap(dot(n, LIGHT), -0.25, 0.95, 0, 1);
}

// Cardboard gradient stops from the old isometric logo: [light corner, dark
// corner].
const RAMP: [number, string, string][] = [
  [0, "#bd7f26", "#955c19"],
  [0.45, "#e2ab51", "#c4862c"],
  [1, "#f6d693", "#e8b95f"],
];

// The logo character's flat material: one colour per face, no gradient, from
// the same key light. Seen from straight ahead, as in the logo, the front
// lands exactly on the logo's #e6ad54 and the top on #f2c479, and a band
// FLAT_BAND darker (the lid's edge, the base) on #cf8f35. Faces turned from
// that view shade continuously, so a turning box still reads as a solid.
// Seen isometrically, the right-hand wall is #bd7d23, the old logo's; below
// it the material goes on darkening toward the ink, so the bands on that
// wall show and a carton put in shadow keeps its two walls apart.
const AHEAD = new View({ cx: 0, cy: 0, scale: 1, yaw: 0, pitch: 0 });
const TONE_FRONT = tone(AHEAD, [0, 0, 1]);
const TONE_TOP = tone(AHEAD, [0, 1, 0]);
const TONE_ISO_RIGHT = tone(new View({ cx: 0, cy: 0, scale: 1, yaw: Math.PI / 4, pitch: Math.PI / 6 }), [1, 0, 0]);
/** How much darker a deep band is than the face it is painted on. */
export const FLAT_BAND = 0.25;
const FLAT: [number, string][] = [
  [TONE_ISO_RIGHT - 2 * FLAT_BAND, "#8e5f1d"],
  [TONE_ISO_RIGHT - FLAT_BAND, "#a56e20"],
  [TONE_ISO_RIGHT, "#bd7d23"],
  [TONE_FRONT - 2 * FLAT_BAND, "#bd7d23"],
  [TONE_FRONT - FLAT_BAND, "#cf8f35"],
  [TONE_FRONT, "#e6ad54"],
  [TONE_TOP, "#f2c479"],
  [TONE_TOP + FLAT_BAND, "#f8dca6"],
];

/** The flat logo colour of a face of the given tone (see tone()). */
export function flatCard(t: number): string {
  if (t <= FLAT[0][0]) return FLAT[0][1];
  for (let i = 1; i < FLAT.length; i++) {
    const [t1, c1] = FLAT[i];
    // Exact at the stops, so the logo's colours come out as written.
    if (t === t1) return c1;
    if (t < t1) {
      const [t0, c0] = FLAT[i - 1];
      return mix(c0, c1, (t - t0) / (t1 - t0));
    }
  }
  return FLAT[FLAT.length - 1][1];
}

/** Two gradient colors for a cardboard face of the given tone. */
export function cardboard(t: number): [string, string] {
  const k = Math.min(Math.max(t, 0), 1);
  for (let i = 1; i < RAMP.length; i++) {
    if (k <= RAMP[i][0]) {
      const [t0, a0, b0] = RAMP[i - 1];
      const [t1, a1, b1] = RAMP[i];
      const p = (k - t0) / (t1 - t0);
      return [mix(a0, a1, p), mix(b0, b1, p)];
    }
  }
  return [RAMP[2][1], RAMP[2][2]];
}

/** Fill a projected polygon with the logo's diagonal cardboard gradient. */
export function cardboardFill(
  ctx: CanvasRenderingContext2D,
  pts: readonly { x: number; y: number }[],
  t: number,
): CanvasGradient {
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
  const [a, b] = cardboard(t);
  const g = ctx.createLinearGradient(x0, y0, x1, y1);
  g.addColorStop(0, a);
  g.addColorStop(1, b);
  return g;
}

/** Trace a projected polygon as the current path. */
export function polygon(
  ctx: CanvasRenderingContext2D,
  pts: readonly { x: number; y: number }[],
): void {
  ctx.beginPath();
  ctx.moveTo(pts[0].x, pts[0].y);
  for (let i = 1; i < pts.length; i++) ctx.lineTo(pts[i].x, pts[i].y);
  ctx.closePath();
}

/** Convex hull (Andrew's monotone chain) of screen points, clockwise on screen. */
export function hull<T extends { x: number; y: number }>(points: readonly T[]): T[] {
  const p = [...points].sort((a, b) => a.x - b.x || a.y - b.y);
  if (p.length < 3) return p;
  const crossZ = (o: T, a: T, b: T) =>
    (a.x - o.x) * (b.y - o.y) - (a.y - o.y) * (b.x - o.x);
  const lower: T[] = [];
  for (const q of p) {
    while (lower.length >= 2 && crossZ(lower[lower.length - 2], lower[lower.length - 1], q) <= 0)
      lower.pop();
    lower.push(q);
  }
  const upper: T[] = [];
  for (let i = p.length - 1; i >= 0; i--) {
    const q = p[i];
    while (upper.length >= 2 && crossZ(upper[upper.length - 2], upper[upper.length - 1], q) <= 0)
      upper.pop();
    upper.push(q);
  }
  upper.pop();
  lower.pop();
  return lower.concat(upper);
}
