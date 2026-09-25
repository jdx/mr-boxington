// Mr Boxington as a real 3D cube. The face, tape, and label reuse the path
// data from docs/public/logo.svg, drawn in each panel's own plane, so the
// logo pose (HERO_POSE under HERO_CAM) reproduces the logo exactly.

import { rgba } from "./color";
import { roundedRect } from "./fx";
import { clamp, lerp } from "./math";
import {
  add,
  applyMatrix,
  cardboardFill,
  hull,
  mul,
  polygon,
  type Projected,
  rotateAround,
  tone,
  type V3,
  View,
} from "./space";

export const INK = "#2a3f44";
export const CREAM = "#f6ecd2";
export const OUTLINE = "#53350f";
export const TIE = "#35555c";
export const KNOT = "#e9b650";
export const TAPE = "#efd8a4";
export const TAPE_SIDE = "#d8b96e";
export const TAPE_EDGE = "#a5772b";
export const LABEL = "#f6ecd2";
export const LABEL_EDGE = "#8a5215";
export const LABEL_LINES = "#b08d4d";

// Logo proportions that scenes drawing their own cardboard must match.
/** Outline width per unit of box edge (8 px on the logo's 147 px edge). */
export const OUTLINE_RATIO = 8 / 147;
/** Half-width of the tape band across the top, in the box's [-1, 1] units. */
export const TAPE_HALF = 0.23;
/** How far the tape's end runs down the right panel, as a share of the edge. */
export const TAPE_DROP = 26 / 128;

/** Face features, each animatable. Omitted fields default to fully shown. */
export interface FaceParams {
  /** 0..1 brows drawn in. */
  brows?: number;
  /** Brow raise in face px (the logo face is ~104 px wide); positive is up. */
  browLift?: number;
  /** 0..1 eyes popped in. */
  eyes?: number;
  /** 0..1 eyelid closure. */
  blink?: number;
  /** Eye offset in face px. */
  look?: [number, number];
  /** 0..1 monocle travel: 0 is up and away, 1 is seated on the right eye. */
  monocle?: number;
  /** 0..1 sparkle on the monocle rim. */
  glint?: number;
  /** 0..1 mustache grown in from the center. */
  mustache?: number;
  /** Mustache wiggle, radians per half. */
  twitch?: number;
  /** 0..1 bow tie scaled in. */
  bowtie?: number;
  /** Bow tie rotation, radians. */
  bowtieSpin?: number;
}

export const FACE_FULL: Readonly<Required<FaceParams>> = {
  brows: 1,
  browLift: 0,
  eyes: 1,
  blink: 0,
  look: [0, 0],
  monocle: 1,
  glint: 0,
  mustache: 1,
  twitch: 0,
  bowtie: 1,
  bowtieSpin: 0,
};

export interface BoxPose {
  /** World position of the center of the bottom face. */
  pos: V3;
  /** Edge length in world units. */
  size?: number;
  /** Turn about the box's own vertical axis, radians. */
  yaw?: number;
  /** Tumble about the box center: around world X, then world Z, radians. */
  tiltX?: number;
  tiltZ?: number;
  /** Vertical scale about the bottom face; width compensates to keep volume. */
  squash?: number;
  /** 0..1 packing tape laid across the top and down the right panel. */
  tape?: number;
  /** 0..1 shipping label stamped onto the right panel. */
  label?: number;
  /** Face features on the left panel; null for a plain box. */
  face?: FaceParams | null;
  /** Outline width multiplier; 1 matches the logo. */
  outline?: number;
  /** Overall opacity. */
  alpha?: number;
  /** Shift every face's tone, -1..1. */
  toneShift?: number;
  /**
   * Draw a single flat color silhouette instead of shaded panels (for
   * morph handoffs). The value is a CSS color.
   */
  flat?: string;
}

/** The logo's resting pose: tape, label, and the full face. */
export const HERO_POSE: Readonly<BoxPose> = {
  pos: [0, -0.5, 0],
  size: 1,
  tape: 1,
  label: 1,
  face: FACE_FULL,
};

let paths: Record<string, Path2D> | null = null;
function P(): Record<string, Path2D> {
  if (paths) return paths;
  const circle = (x: number, y: number, r: number) => {
    const p = new Path2D();
    p.arc(x, y, r, 0, Math.PI * 2);
    return p;
  };
  paths = {
    browL: new Path2D("M-38 -52q11 -7 21 -2"),
    browR: new Path2D("M6 -52q11 -7 21 -2"),
    eyeL: circle(-26, -28, 8),
    eyeR: circle(18, -28, 8),
    hiL: circle(-28.5, -30.5, 2.6),
    hiR: circle(15.5, -30.5, 2.6),
    ring: circle(18, -28, 16.5),
    glint: new Path2D("M7.1 -33.1A12 12 0 0 1 12 -38.4"),
    chain: new Path2D("M33 -16.5Q35.5 -9 34 -3"),
    bead: circle(33.8, -0.5, 3),
    mustR: new Path2D(
      "M-4 2 C0 -4 8 -6 14 -3 C20 -.5 23 -2.5 24.5 -7 C28 -2 27 5 21 7.5 C13 11 1 11 -4 8 Z",
    ),
    mustL: new Path2D(
      "M-4 2 C-8 -4 -16 -6 -22 -3 C-28 -.5 -31 -2.5 -32.5 -7 C-36 -2 -35 5 -29 7.5 C-21 11 -9 11 -4 8 Z",
    ),
    wingL: new Path2D(
      "M-12 34 -34 22.5c-3.6-1.9-8 .7-8 4.8v13.4c0 4.1 4.4 6.7 8 4.8L-12 34Z",
    ),
    wingR: new Path2D(
      "M4 34 26 22.5c3.6-1.9 8 .7 8 4.8v13.4c0 4.1-4.4 6.7-8 4.8L4 34Z",
    ),
    labelLines: new Path2D("M35 50h26M35 58h32M35 66h20"),
  };
  return paths;
}

/** The box's local frame after pose transforms. */
export interface BoxFrame {
  /** Box center. */
  c: V3;
  /** Half-extent vectors along the box's local x, y, z (already scaled). */
  x: V3;
  y: V3;
  z: V3;
  size: number;
  squash: number;
}

export function boxFrame(pose: BoxPose): BoxFrame {
  const s = pose.size ?? 1;
  const sq = pose.squash ?? 1;
  const wide = 1 / Math.sqrt(Math.max(sq, 0.05));
  const h = s / 2;
  let x: V3 = [h * wide, 0, 0];
  let y: V3 = [0, h * sq, 0];
  let z: V3 = [0, 0, h * wide];
  const yaw = pose.yaw ?? 0;
  if (yaw) {
    const o: V3 = [0, 0, 0];
    x = rotateAround(x, o, [0, 1, 0], yaw);
    z = rotateAround(z, o, [0, 1, 0], yaw);
  }
  let c: V3 = add(pose.pos, [0, h * sq, 0]);
  const tx = pose.tiltX ?? 0;
  const tz = pose.tiltZ ?? 0;
  if (tx || tz) {
    // Tilt about the bottom center so a leaning box stays planted.
    const pivot = pose.pos;
    const o: V3 = [0, 0, 0];
    const turn = (v: V3, axis: V3, a: number) => rotateAround(v, o, axis, a);
    if (tx) {
      x = turn(x, [1, 0, 0], tx);
      y = turn(y, [1, 0, 0], tx);
      z = turn(z, [1, 0, 0], tx);
      c = rotateAround(c, pivot, [1, 0, 0], tx);
    }
    if (tz) {
      x = turn(x, [0, 0, 1], tz);
      y = turn(y, [0, 0, 1], tz);
      z = turn(z, [0, 0, 1], tz);
      c = rotateAround(c, pivot, [0, 0, 1], tz);
    }
  }
  return { c, x, y, z, size: s, squash: sq };
}

/** Point in the box from local coordinates in [-1, 1]^3. */
export function boxPoint(f: BoxFrame, lx: number, ly: number, lz: number): V3 {
  return add(f.c, add(mul(f.x, lx), add(mul(f.y, ly), mul(f.z, lz))));
}

export function boxCorners(pose: BoxPose): V3[] {
  const f = boxFrame(pose);
  const out: V3[] = [];
  for (const i of [-1, 1]) for (const j of [-1, 1]) for (const k of [-1, 1]) out.push(boxPoint(f, i, j, k));
  return out;
}

/** Screen-space outline of the box (convex hull of its projected corners). */
export function boxSilhouette(view: View, pose: BoxPose): Projected[] {
  return hull(boxCorners(pose).map((p) => view.project(p)));
}

type PanelId = "top" | "bottom" | "face" | "right" | "back" | "left";

interface Panel {
  id: PanelId;
  /** Local-coordinate corners, wound consistently. */
  corners: [number, number, number][];
  normal: (f: BoxFrame) => V3;
}

const unit = (v: V3): V3 => {
  const l = Math.hypot(v[0], v[1], v[2]) || 1;
  return [v[0] / l, v[1] / l, v[2] / l];
};

const PANELS: Panel[] = [
  { id: "top", corners: [[-1, 1, -1], [1, 1, -1], [1, 1, 1], [-1, 1, 1]], normal: (f) => unit(f.y) },
  { id: "bottom", corners: [[-1, -1, -1], [-1, -1, 1], [1, -1, 1], [1, -1, -1]], normal: (f) => unit(mul(f.y, -1)) },
  // The logo's left panel faces +z and carries the face.
  { id: "face", corners: [[-1, 1, 1], [1, 1, 1], [1, -1, 1], [-1, -1, 1]], normal: (f) => unit(f.z) },
  { id: "back", corners: [[1, 1, -1], [-1, 1, -1], [-1, -1, -1], [1, -1, -1]], normal: (f) => unit(mul(f.z, -1)) },
  // The logo's right panel faces +x and carries the label and tape end.
  { id: "right", corners: [[1, 1, 1], [1, 1, -1], [1, -1, -1], [1, -1, 1]], normal: (f) => unit(f.x) },
  { id: "left", corners: [[-1, 1, -1], [-1, 1, 1], [-1, -1, 1], [-1, -1, -1]], normal: (f) => unit(mul(f.x, -1)) },
];

/**
 * Plane matrix for a panel's decoration space, in the logo's own units:
 * face and right panels use the SVG's sheared local frames (104 px across,
 * 128 px down per edge); the top uses unit coordinates (u along x, v along z).
 */
export function panelMatrix(view: View, f: BoxFrame, id: "face" | "right" | "top") {
  if (id === "face") {
    // logo: translate(108,182) matrix(1 .5 0 1 0 0), origin at the panel center
    const o = boxPoint(f, 0, 0, 1);
    return view.planeMatrix(o, mul(f.x, 2 / 104), mul(f.y, -2 / 128));
  }
  if (id === "right") {
    // logo: translate(160,144) matrix(1 -.5 0 1 0 0), origin at the top front corner
    const o = boxPoint(f, 1, 1, 1);
    return view.planeMatrix(o, mul(f.z, -2 / 104), mul(f.y, -2 / 128));
  }
  const o = boxPoint(f, -1, 1, -1);
  return view.planeMatrix(o, mul(f.x, 2), mul(f.z, 2));
}

/** Screen position of a point in the face panel's logo coordinates. */
export function faceToScreen(view: View, pose: BoxPose, lx: number, ly: number): Projected {
  const f = boxFrame(pose);
  return view.project(
    add(boxPoint(f, 0, 0, 1), add(mul(f.x, (2 * lx) / 104), mul(f.y, (-2 * ly) / 128))),
  );
}

/** The seated monocle's center and approximate screen radius. */
export function monocleScreen(view: View, pose: BoxPose): { x: number; y: number; r: number } {
  const c = faceToScreen(view, pose, 18, -28);
  const e = faceToScreen(view, pose, 34.5, -28);
  return { x: c.x, y: c.y, r: Math.hypot(e.x - c.x, e.y - c.y) };
}

/** Draw a four-point sparkle, unsheared, in screen space. */
export function sparkle(
  ctx: CanvasRenderingContext2D,
  x: number,
  y: number,
  size: number,
  alpha: number,
  rot = 0,
  color = "#fff8e6",
): void {
  if (alpha <= 0 || size <= 0) return;
  ctx.save();
  ctx.translate(x, y);
  ctx.rotate(rot);
  ctx.globalAlpha *= clamp(alpha);
  const g = ctx.createRadialGradient(0, 0, 0, 0, 0, size * 1.4);
  g.addColorStop(0, rgba(color, 0.9));
  g.addColorStop(0.25, rgba(color, 0.35));
  g.addColorStop(1, rgba(color, 0));
  ctx.fillStyle = g;
  ctx.beginPath();
  ctx.arc(0, 0, size * 1.4, 0, Math.PI * 2);
  ctx.fill();
  ctx.fillStyle = color;
  ctx.beginPath();
  const w = size * 0.13;
  ctx.moveTo(0, -size);
  ctx.quadraticCurveTo(w, -w, size, 0);
  ctx.quadraticCurveTo(w, w, 0, size);
  ctx.quadraticCurveTo(-w, w, -size, 0);
  ctx.quadraticCurveTo(-w, -w, 0, -size);
  ctx.fill();
  ctx.restore();
}

function drawFace(ctx: CanvasRenderingContext2D, face: FaceParams): void {
  const p = P();
  const v = { ...FACE_FULL, ...face };
  ctx.lineCap = "round";
  ctx.lineJoin = "round";

  // Bow tie, spun and scaled about its knot.
  if (v.bowtie > 0) {
    ctx.save();
    ctx.translate(-4, 34);
    ctx.rotate(v.bowtieSpin);
    ctx.scale(v.bowtie, v.bowtie);
    ctx.translate(4, -34);
    ctx.fillStyle = TIE;
    ctx.strokeStyle = INK;
    ctx.lineWidth = 4;
    ctx.fill(p.wingL);
    ctx.stroke(p.wingL);
    ctx.fill(p.wingR);
    ctx.stroke(p.wingR);
    roundedRect(ctx, -11, 26, 14, 16, 3.5);
    ctx.fillStyle = KNOT;
    ctx.fill();
    ctx.stroke();
    ctx.restore();
  }

  // Mustache halves grow from the center and twitch independently.
  if (v.mustache > 0) {
    ctx.fillStyle = INK;
    for (const [path, dir] of [
      [p.mustL, -1],
      [p.mustR, 1],
    ] as const) {
      ctx.save();
      ctx.translate(-4, 5);
      ctx.rotate(-dir * v.twitch);
      ctx.scale(v.mustache, lerp(0.6, 1, v.mustache));
      ctx.translate(4, -5);
      ctx.fill(path);
      ctx.restore();
    }
  }

  // Eyes, with blink as a vertical squash toward a closed line.
  if (v.eyes > 0) {
    const [lx, ly] = v.look;
    for (const [eye, hi, cx] of [
      [p.eyeL, p.hiL, -26],
      [p.eyeR, p.hiR, 18],
    ] as const) {
      ctx.save();
      ctx.translate(cx + lx, -28 + ly);
      const open = 1 - clamp(v.blink);
      ctx.scale(v.eyes, v.eyes * Math.max(open, 0.001));
      ctx.translate(-cx, 28);
      ctx.fillStyle = INK;
      ctx.fill(eye);
      if (open > 0.35) {
        ctx.fillStyle = CREAM;
        ctx.fill(hi);
      }
      ctx.restore();
      if (open < 0.2 && v.eyes > 0.5) {
        ctx.strokeStyle = INK;
        ctx.lineWidth = 4;
        ctx.beginPath();
        ctx.moveTo(cx + lx - 8 * v.eyes, -27 + ly);
        ctx.quadraticCurveTo(cx + lx, -23 + ly, cx + lx + 8 * v.eyes, -27 + ly);
        ctx.stroke();
      }
    }
  }

  // Brows draw in from their inner ends and lift together.
  if (v.brows > 0) {
    ctx.strokeStyle = INK;
    ctx.lineWidth = 5.5;
    for (const [path, x] of [
      [p.browL, -28],
      [p.browR, 16],
    ] as const) {
      ctx.save();
      ctx.translate(x, -54 - v.browLift);
      ctx.scale(v.brows, v.brows);
      ctx.translate(-x, 54);
      ctx.stroke(path);
      ctx.restore();
    }
  }

  // Monocle: travels in from up and to the right with a swing, then seats.
  if (v.monocle > 0) {
    const away = 1 - clamp(v.monocle, 0, 1.2);
    ctx.save();
    ctx.translate(18 + away * 46, -28 - away * 70);
    ctx.rotate(away * -1.1);
    const s = lerp(1, 1.35, clamp(away));
    ctx.scale(s, s);
    ctx.translate(-18, 28);
    ctx.globalAlpha *= clamp(v.monocle * 3);
    ctx.strokeStyle = INK;
    ctx.lineWidth = 5;
    ctx.stroke(p.ring);
    ctx.strokeStyle = rgba(CREAM, lerp(0.7, 1, clamp(v.glint)));
    ctx.lineWidth = 3;
    ctx.stroke(p.glint);
    ctx.strokeStyle = INK;
    ctx.lineWidth = 3.5;
    ctx.stroke(p.chain);
    ctx.fillStyle = INK;
    ctx.fill(p.bead);
    ctx.restore();
  }
}

function drawTape(ctx: CanvasRenderingContext2D, view: View, f: BoxFrame, amount: number, facing: Set<PanelId>) {
  if (amount <= 0) return;
  // Across the top: logo tape spans x 0..1 at z 0.385..0.615 in unit coords.
  const along = clamp(amount / 0.8);
  if (facing.has("top")) {
    ctx.save();
    applyMatrix(ctx, panelMatrix(view, f, "top"));
    ctx.beginPath();
    ctx.rect(0, 0.385, along, 0.23);
    ctx.fillStyle = TAPE;
    ctx.fill();
    ctx.restore();
    // Stroke in screen space so its width does not shear.
    const corners = [
      boxPoint(f, -1, 1, -TAPE_HALF),
      boxPoint(f, -1 + 2 * along, 1, -TAPE_HALF),
      boxPoint(f, -1 + 2 * along, 1, TAPE_HALF),
      boxPoint(f, -1, 1, TAPE_HALF),
    ].map((p) => view.project(p));
    ctx.save();
    polygon(ctx, corners);
    ctx.strokeStyle = TAPE_EDGE;
    ctx.lineJoin = "round";
    ctx.lineWidth = (OUTLINE_RATIO / 2) * f.size * view.cam.scale * corners[0].f;
    ctx.stroke();
    ctx.restore();
  }
  // Down the right panel: 26 of 128 px.
  const down = clamp((amount - 0.8) / 0.2);
  if (down > 0 && facing.has("right")) {
    const d = TAPE_DROP * 2 * down;
    const corners = [
      boxPoint(f, 1, 1, TAPE_HALF),
      boxPoint(f, 1, 1, -TAPE_HALF),
      boxPoint(f, 1, 1 - d, -TAPE_HALF),
      boxPoint(f, 1, 1 - d, TAPE_HALF),
    ].map((p) => view.project(p));
    ctx.save();
    polygon(ctx, corners);
    ctx.fillStyle = TAPE_SIDE;
    ctx.fill();
    ctx.strokeStyle = TAPE_EDGE;
    ctx.lineJoin = "round";
    ctx.lineWidth = (OUTLINE_RATIO / 2) * f.size * view.cam.scale * corners[0].f;
    ctx.stroke();
    ctx.restore();
  }
}

/**
 * The monocle's paths in the face panel's logo coordinates, for scenes that
 * fly it in themselves before handing it back to drawBox.
 */
export function monoclePaths(): { ring: Path2D; glint: Path2D; chain: Path2D; bead: Path2D } {
  const p = P();
  return { ring: p.ring, glint: p.glint, chain: p.chain, bead: p.bead };
}

/** The shipping label at rest, in the right panel's logo coordinates. */
export function drawLabelArt(ctx: CanvasRenderingContext2D): void {
  roundedRect(ctx, 28, 40, 50, 32, 3);
  ctx.lineJoin = "round";
  ctx.fillStyle = LABEL;
  ctx.fill();
  ctx.strokeStyle = LABEL_EDGE;
  ctx.lineWidth = 4;
  ctx.stroke();
  ctx.strokeStyle = LABEL_LINES;
  ctx.lineWidth = 3.5;
  ctx.lineCap = "round";
  ctx.stroke(P().labelLines);
}

function drawLabel(ctx: CanvasRenderingContext2D, amount: number) {
  if (amount <= 0) return;
  // Stamp: scales down onto the panel from slightly large.
  const s = lerp(1.6, 1, clamp(amount));
  ctx.save();
  ctx.globalAlpha *= clamp(amount * 2);
  if (s !== 1) {
    ctx.translate(53, 56);
    ctx.scale(s, s);
    ctx.translate(-53, -56);
  }
  drawLabelArt(ctx);
  ctx.restore();
}

/** Draw Mr Boxington (or a plain box) in the given pose. */
export function drawBox(ctx: CanvasRenderingContext2D, view: View, pose: BoxPose): void {
  const alpha = pose.alpha ?? 1;
  if (alpha <= 0) return;
  const f = boxFrame(pose);
  ctx.save();
  ctx.globalAlpha *= alpha;

  if (pose.flat) {
    polygon(ctx, boxSilhouette(view, pose));
    ctx.fillStyle = pose.flat;
    ctx.fill();
    ctx.restore();
    return;
  }

  const lw = OUTLINE_RATIO * f.size * view.cam.scale * (pose.outline ?? 1);
  const visible: { id: PanelId; pts: Projected[]; depth: number; n: V3 }[] = [];
  for (const panel of PANELS) {
    const n = panel.normal(f);
    const world = panel.corners.map(([a, b, c]) => boxPoint(f, a, b, c));
    const center = boxPoint(f, ...(panel.corners.reduce(
      (acc, c) => [acc[0] + c[0] / 4, acc[1] + c[1] / 4, acc[2] + c[2] / 4],
      [0, 0, 0],
    ) as [number, number, number]));
    if (!view.facing(n, center)) continue;
    const pts = world.map((p) => view.project(p));
    visible.push({ id: panel.id, pts, depth: view.project(center).z, n });
  }
  visible.sort((a, b) => a.depth - b.depth);
  const facing = new Set(visible.map((v) => v.id));

  // Same order as the logo: each panel filled and outlined, then the tape,
  // label, and face on top.
  ctx.lineJoin = "round";
  ctx.strokeStyle = OUTLINE;
  for (const panel of visible) {
    polygon(ctx, panel.pts);
    ctx.fillStyle = cardboardFill(ctx, panel.pts, tone(view, panel.n) + (pose.toneShift ?? 0));
    ctx.fill();
    if (lw > 0) {
      ctx.lineWidth = lw * panel.pts[0].f;
      ctx.stroke();
    }
  }
  drawTape(ctx, view, f, pose.tape ?? 0, facing);
  if (facing.has("right") && (pose.label ?? 0) > 0) {
    ctx.save();
    applyMatrix(ctx, panelMatrix(view, f, "right"));
    drawLabel(ctx, pose.label ?? 0);
    ctx.restore();
  }
  if (pose.face && facing.has("face")) {
    ctx.save();
    applyMatrix(ctx, panelMatrix(view, f, "face"));
    drawFace(ctx, pose.face);
    ctx.restore();
    const glint = pose.face.glint ?? 0;
    if (glint > 0 && (pose.face.monocle ?? 1) >= 0.98) {
      const g = faceToScreen(view, pose, 7.5, -38);
      const r = monocleScreen(view, pose).r;
      sparkle(ctx, g.x, g.y, r * 1.5 * glint, glint, glint * 0.6);
    }
  }
  ctx.restore();
}

/** Soft contact shadow on the floor plane y = pose.pos[1] - lift. */
export function drawShadow(
  ctx: CanvasRenderingContext2D,
  view: View,
  pos: V3,
  size: number,
  lift: number,
  alpha = 0.45,
  floorY = pos[1] - lift,
): void {
  const spread = 1 + lift * 0.9;
  const a = alpha / (1 + lift * 2.5);
  if (a <= 0.003) return;
  const o: V3 = [pos[0], floorY, pos[2]];
  ctx.save();
  applyMatrix(ctx, view.planeMatrix(o, [size, 0, 0], [0, 0, size]));
  const r = 0.78 * spread;
  const g = ctx.createRadialGradient(0, 0, 0, 0, 0, r);
  g.addColorStop(0, `rgba(0,0,0,${a})`);
  g.addColorStop(0.55, `rgba(0,0,0,${a * 0.5})`);
  g.addColorStop(1, "rgba(0,0,0,0)");
  ctx.fillStyle = g;
  ctx.beginPath();
  ctx.arc(0, 0, r, 0, Math.PI * 2);
  ctx.fill();
  ctx.restore();
}
