// The reel's shared contract: tempo, the timeline, palette, cameras, and the
// exact frame each scene hands to the next. Scenes own everything between
// handoffs; map.ts draws every handoff frame (drawHandoff) and lays out the
// world the middle sections share.

import { type BoxPose, drawBox, drawShadow, FRONT_CAM, LOGO_POSE, logoCam } from "./box";
import type { ReelFacts } from "./facts";
import { DEG, inCubic, progress } from "./math";
import { type Camera, View } from "./space";
import { type SectionId, sec } from "./timeline";
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

// The benchmark claims are read, checked and typed where the published run
// is parsed: facts.ts.
export type { ReelFacts };

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
  /**
   * The lit screen on the frame at `lt`, if there is one: the pane's
   * window, which the reel's vignette leaves out.
   */
  lit?(lt: number): LitRect | null;
}

/**
 * A screen in the frame, logical px: a terminal's window, lit from within.
 * The vignette spares it, so the pixel pane's colours read as the
 * terminal's wherever it stands.
 */
export interface LitRect {
  x: number;
  y: number;
  w: number;
  h: number;
  /** How far the vignette spares it, 0 to 1. */
  alpha: number;
}

export const iso = { yaw: 45 * DEG, pitch: 30 * DEG };

// Handoffs fold → mr-boxington and mr-boxington → what: the logo character
// (box.ts LOGO_POSE, standing on the origin) seen from straight ahead. At the
// first he is shut and taped with no face, centered at FRONT_CAM; by the
// second he wears the logo's face at H2_CAM, eased left for the name card,
// and the push toward his monocle is under way. Both frames are drawn with
// drawLogoBox.

/**
 * @deprecated The old isometric hero view, kept for the scenes that still
 * perform in it. Handoffs use H1_CAM and H2_CAM.
 */
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

/**
 * The logo character on the floor, seen from straight ahead: a soft contact
 * shadow under its base, then the box. drawShadow's floor ellipse lies edge
 * on in the front view, so this one is drawn on the screen under the base.
 */
export function drawLogoBox(ctx: CanvasRenderingContext2D, cam: Camera, pose: BoxPose, shadow = 1): void {
  const view = new View(cam);
  if (shadow > 0) {
    const s = pose.size ?? 1;
    const l = view.project([pose.pos[0] - s / 2, pose.pos[1], pose.pos[2] + s / 2]);
    const r = view.project([pose.pos[0] + s / 2, pose.pos[1], pose.pos[2] + s / 2]);
    const w = r.x - l.x;
    const cx = (l.x + r.x) / 2;
    ctx.save();
    ctx.translate(cx, l.y);
    ctx.scale(1, 0.085);
    const g = ctx.createRadialGradient(0, 0, 0, 0, 0, w * 0.66);
    g.addColorStop(0, `rgba(0,0,0,${0.55 * shadow})`);
    g.addColorStop(0.6, `rgba(0,0,0,${0.3 * shadow})`);
    g.addColorStop(1, "rgba(0,0,0,0)");
    ctx.fillStyle = g;
    ctx.beginPath();
    ctx.arc(0, 0, w * 0.66, 0, Math.PI * 2);
    ctx.fill();
    ctx.restore();
  }
  drawBox(ctx, view, pose);
}

/** fold → mr-boxington: the logo 480 px square in the middle of the frame. */
export const H1_CAM: Camera = FRONT_CAM;
/** fold → mr-boxington: shut and taped in flat logo colours, no face yet. */
export const H1_POSE: BoxPose = { ...LOGO_POSE, face: null };

/**
 * mr-boxington's name-card framing, where the push toward the monocle
 * starts: the logo 440 px square at (250, 250), so the box stands at x
 * 264-676 on y 676 and the right of the frame is free for the name card.
 */
export const H2_CAM: Camera = logoCam(250, 250, 440);
/** mr-boxington → what: the logo exactly, taped, with its face and rosy cheeks. */
export const H2_POSE: BoxPose = { ...LOGO_POSE };

/** The monocle dive runs from mr-boxington b7.75 to what b0.5, across the bar line. */
export const DIVE0 = sec("mr-boxington").beat(7.75);
export const DIVE1 = sec("what").beat(0.5);
/** How far the dive zooms: the lens (logo r 21 inside the rim) then covers the frame. */
export const DIVE_ZOOM = 16;
/**
 * The monocle dive's camera at global time `t`: H2_CAM until DIVE0, then an
 * accelerating zoom (DIVE_ZOOM to the power inCubic) about the one screen
 * point that brings the monocle (logo 86, 62) to the frame's center as the
 * zoom reaches DIVE_ZOOM on DIVE1, held after. Both scenes draw H2_POSE
 * under it, so the dive is one move across the bar line, where the zoom is
 * 1.108.
 */
export function diveCam(t: number): Camera {
  const z = DIVE_ZOOM ** inCubic(progress(DIVE0, DIVE1, t));
  const k = 440 / 128;
  // The zoom's fixed point: the monocle, at (250 + 86k, 250 + 62k), lands
  // on the frame's center.
  const fx = (DIVE_ZOOM * (250 + 86 * k) - W / 2) / (DIVE_ZOOM - 1);
  const fy = (DIVE_ZOOM * (250 + 62 * k) - H / 2) / (DIVE_ZOOM - 1);
  return logoCam(fx + (250 - fx) * z, fy + (250 - fy) * z, 440 * z);
}

// Handoff what → every-checkout: only the three node labels on the
// background, exactly as drawn by drawNodeLabel. They are the labels over
// every-checkout's three terminal cards (map.ts EC_CARDS), clear of the
// captions' band.

/** Each node label's baseline center: 56 px type over its card. */
export const NODES = {
  project: { x: 360, y: 196, label: "project" },
  worktree: { x: 960, y: 196, label: "worktree" },
  ci: { x: 1560, y: 196, label: "CI" },
} as const;
/** The labels' type. The baseline sits `dy` below the NODES point (0: on it). */
export const NODE_LABEL = { dy: 0, size: 56, weight: 600, fill: PALETTE.paper, tracking: -1.1 } as const;

/** The handoff label for a node, centered on its NODES point. */
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
// left over its last WHIP (map.ts whipOut, with fx.smear); the chart arrives
// from the right over the next section's first WHIP. map.ts drawWhip's
// streaks run through both halves and peak on the bar line, whose frame is
// the streaks alone on PALETTE.bg.
export const WHIP = 0.14;

// Handoff next-push → pruned: one plain carton (no face or tape) at the
// center cell of the world grid under WORLD_CAM, with no floor grid yet.

export const WORLD_CAM: Camera = { cx: 960, cy: 600, scale: 70, ...iso, target: [0, 0, 0] };
/** Grid cells run from -GRID_R to GRID_R on x and z. */
export const GRID_R = 4;
export const CUBE = 0.78;
/**
 * A world carton on cell (x, z): the logo character built as a cube, CUBE
 * on every side, faceless, in the flat logo colours with no outline, so it
 * wears the lid band and base band the reel's other cartons (map.ts
 * drawCarton) do and `tape` lays the logo's tab.
 */
export const cellPose = (x: number, z: number, extra: Partial<BoxPose> = {}): BoxPose => ({
  kind: "logo",
  pos: [x, 0, z],
  size: CUBE,
  dims: [1, 1, 1],
  face: null,
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

// Handoff morph → end: the front-facing silhouette of END_POSE under END_CAM
// (boxSilhouette: six corners, plus a seventh point on the right-hand side,
// where the lid's edge meets the wall, that turns by less than a hundredth
// of a pixel) filled flat with PALETTE.amber on PALETTE.night. The end card
// inflates it into the logo.

/**
 * The end card's box: the logo, standing half a box width below the origin
 * as the old hero did, so the card's inflate about the box's middle keeps
 * its place.
 */
export const END_POSE: BoxPose = { ...LOGO_POSE, pos: [0, -0.5, 0] };
/** The lid's top, where END_CAM looks. */
const END_LID = END_POSE.pos[1] + 0.85;

/**
 * The end card's exact logo view of END_POSE: the logo 420 px square at
 * (750, 185), the box at x 763-1157, y 208-592 (logoCam, moved down with
 * the box). END_CAM is derived from it; the end card itself lands smaller,
 * on s8's CARD_SQ, to leave room for its copy under the box.
 */
export const END_LOGO_CAM: Camera = (() => {
  const L = logoCam(750, 185, 420);
  const [x, y, z] = L.target ?? [0, 0, 0];
  return { ...L, target: [x, y + END_POSE.pos[1], z] };
})();

/**
 * The morph's target and the end card's first frame: END_LOGO_CAM's eye
 * turned to look at the middle of the lid, so (cx, cy) is the lid's center
 * on screen (960, 230) rather than the horizon above the box, as a scene
 * placing things by END_CAM's center expects. It is the same front view
 * from the same eye point with the picture plane tipped 7.8° down: the
 * silhouette's top is END_LOGO_CAM's to a pixel, and its base is 9 px
 * narrower and 11 px higher (y 581, not 592), so the sides taper by 1°.
 */
export const END_CAM: Camera = (() => {
  const L = END_LOGO_CAM;
  // The logo's eye: straight out from its target, persp away.
  const eye = [0, (L.target ?? [0, 0, 0])[1], L.persp ?? 0];
  const dy = eye[1] - END_LID;
  const D = Math.hypot(dy, eye[2]);
  const pitch = Math.atan2(dy, eye[2]);
  const view = new View(L);
  const at = view.project([0, END_LID, 0]);
  // Match the lid's front edge: 0.5 either side, 0.5 toward the eye.
  const near = view.project([0.5, END_LID, 0.5]);
  const f = D / (D - 0.5 * Math.cos(pitch));
  const scale = (near.x - at.x) / (0.5 * f);
  return { cx: at.x, cy: at.y, scale, yaw: 0, pitch, persp: D, target: [0, END_LID, 0] };
})();
