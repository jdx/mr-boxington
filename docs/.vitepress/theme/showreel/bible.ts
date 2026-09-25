// The reel's shared contract: tempo, the timeline, palette, cameras, and the
// exact frame each scene hands to the next. Scenes own everything between
// handoffs.

import { type BoxPose, drawBox, drawShadow, FACE_FULL, HERO_POSE } from "./box";
import { DEG } from "./math";
import { type Camera, View } from "./space";
import type { SectionId } from "./timeline";
import { type Caption, drawText, font } from "./type";

/** Logical frame size. Scenes draw in these units at any output resolution. */
export const W = 1920;
export const H = 1080;

// The clock lives in timeline.ts, which imports nothing, so the landing page
// can read the chapters without pulling in the drawing code.
export {
  BAR,
  BEAT,
  BPM,
  bar,
  beat,
  CHAPTERS,
  DURATION,
  type Section,
  type SectionId,
  SECTIONS,
  sec,
} from "./timeline";

export const PALETTE = {
  /** Deepest background, used behind the fold and the end card. */
  night: "#0e0c0a",
  /** Default stage background (docs --vp-c-bg-alt). */
  bg: "#14120f",
  /** Raised surfaces (docs --vp-c-bg-soft). */
  surface: "#211e18",
  divider: "#3d362b",
  amber: "#e6ad54",
  amberBright: "#f2c479",
  amberDeep: "#cf8f35",
  amberShade: "#bd7d23",
  teal: "#6f8e93",
  tealLight: "#9bbbc0",
  green: "#a1ca92",
  paper: "#f5ead6",
  text1: "#eee8dd",
  text2: "#c0b7a6",
  text3: "#a79b86",
  ink: "#20190e",
  outline: "#53350f",
} as const;

export interface ReelFacts {
  /** Benchmark subject, e.g. "hk". */
  subject: string;
  /** Warm build: every lookup restored from the store. */
  warm: { hits: number; lookups: number; seconds: number } | null;
  /** Next-commit build: plain Cargo against mbx with a store warmed at the parent. */
  commit: { cargo: number; mbx: number } | null;
}

export interface SceneEnv {
  W: number;
  H: number;
  /** Global time in seconds. */
  t: number;
  facts: ReelFacts | null;
}

export interface Scene {
  id: SectionId;
  /**
   * Global start and end, seconds: its section's, from `sec(id)`. The frame
   * at `end` belongs to the next scene.
   */
  start: number;
  end: number;
  /** Draw one frame. `lt` is local time, `t - start`. Paint the whole frame. */
  draw(ctx: CanvasRenderingContext2D, lt: number, env: SceneEnv): void;
  /**
   * The section's must-read captions, in its local beats. The reel draws
   * them over the scene in the lower third (type.ts), so a scene lists them
   * here and keeps that band quiet while they are up.
   */
  captions?(facts: ReelFacts | null): readonly Caption[];
}

export const iso = { yaw: 45 * DEG, pitch: 30 * DEG };

// Handoffs fold → mr-boxington and mr-boxington → what: Mr Boxington centered
// in the logo pose. At the first the box is closed and taped with no face or
// label; by the second it wears HERO_POSE.

export const HERO_CAM: Camera = { cx: 960, cy: 540, scale: 280, ...iso, target: [0, 0, 0] };
/** A box standing on the floor: its contact shadow, then the box. */
export function drawStagedBox(
  ctx: CanvasRenderingContext2D,
  cam: Camera,
  pose: BoxPose,
): void {
  const view = new View(cam);
  drawShadow(ctx, view, pose.pos, pose.size ?? 1, 0);
  drawBox(ctx, view, pose);
}

export const H1_POSE: BoxPose = { ...HERO_POSE, label: 0, face: null };
export const H2_POSE: BoxPose = { ...HERO_POSE, face: { ...FACE_FULL } };

// Handoff what → every-checkout: only three labels on the background,
// exactly as drawn by drawNodeLabel, at the node positions the next section
// builds on.

export const NODES = {
  project: { x: 330, y: 520, label: "project" },
  worktree: { x: 1590, y: 330, label: "worktree" },
  ci: { x: 1590, y: 750, label: "CI" },
} as const;
export const NODE_LABEL = { dy: 118, size: 40, weight: 600, fill: PALETTE.paper, tracking: -0.8 } as const;

/** The handoff label for a node, centered below it. */
export function drawNodeLabel(
  ctx: CanvasRenderingContext2D,
  key: keyof typeof NODES,
  alpha = 1,
): void {
  const n = NODES[key];
  if (alpha <= 0) return;
  ctx.save();
  ctx.globalAlpha *= alpha;
  drawText(ctx, n.label, n.x, n.y + NODE_LABEL.dy, {
    font: font(NODE_LABEL.size, NODE_LABEL.weight),
    tracking: NODE_LABEL.tracking,
    align: "center",
    fill: NODE_LABEL.fill,
  });
  ctx.restore();
}

// Handoff ci → next-push: a whip pan. The CI section's content leaves to the
// left over its last 0.14 s using fx.smear; the chart arrives from the right
// over its first 0.14 s the same way. The background is PALETTE.bg on both
// sides.
export const WHIP = 0.14;

// Handoff next-push → pruned: one plain cube (no face, tape, or label) at the
// center cell of the world grid under WORLD_CAM, with no floor grid yet.

export const WORLD_CAM: Camera = { cx: 960, cy: 600, scale: 70, ...iso, target: [0, 0, 0] };
/** Grid cells run from -GRID_R to GRID_R on x and z. */
export const GRID_R = 4;
export const CUBE = 0.78;
export const cellPose = (x: number, z: number, extra: Partial<BoxPose> = {}): BoxPose => ({
  pos: [x, 0, z],
  size: CUBE,
  ...extra,
});
export const H5_POSE: BoxPose = cellPose(0, 0);

// Handoff pruned → morph: the boxes that survive pruning, each shown as a
// flat PALETTE.amber disc (keptDiscs) on PALETTE.bg, nothing else.

export const KEEP: readonly [number, number][] = [
  [0, 0],
  [2, -2],
  [-2, 2],
  [3, 1],
  [-3, -1],
  [1, 3],
  [-1, -3],
];

export function keptDiscs(): { x: number; y: number; r: number }[] {
  const view = new View(WORLD_CAM);
  return KEEP.map(([x, z]) => {
    const p = view.project([x, CUBE / 2, z]);
    return { x: p.x, y: p.y, r: CUBE * WORLD_CAM.scale * 0.72 };
  });
}

// Handoff morph → end: the silhouette of END_POSE under END_CAM
// (boxSilhouette) filled flat with PALETTE.amber on PALETTE.night.

export const END_CAM: Camera = { cx: 960, cy: 410, scale: 185, ...iso, target: [0, 0, 0] };
export const END_POSE: BoxPose = { ...HERO_POSE };
