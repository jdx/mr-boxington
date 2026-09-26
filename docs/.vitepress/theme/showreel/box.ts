// Mr Boxington in 3D, two ways.
//
// The logo character (`kind: "logo"`) is docs/public/logo.svg's box built as
// a solid: 1 wide, 0.85 tall and 1 deep, with the lid a slab of its own that
// hinges up at its left end in the terminal mascot's steps and opens a mouth.
// It is painted in flat logo colours with no outline. The front art is the
// logo's own paths in logo units, drawn in the front panel's plane, and the
// art on the lid comes from projected corners, so LOGO_POSE under logoCam()
// reproduces the logo (test/logo.test.ts compares the two pixel by pixel) and
// any other camera sees the same box in the round.
//
// The cube (`kind: "cube"`, the default) is the old isometric character and
// the reel's plain cartons. Its face, bow tie, and label are deprecated and
// go once the scenes have moved to the logo character.

import { mix, rgba } from "./color";
import { roundedRect } from "./fx";
import { clamp, lerp, smoothstep, TAU } from "./math";
import {
  add,
  applyMatrix,
  type Camera,
  dot,
  cardboard,
  cardboardFill,
  FLAT_BAND,
  flatCard,
  hull,
  len,
  mul,
  norm,
  polygon,
  type Projected,
  rotateAround,
  sub,
  tone,
  type V3,
  View,
} from "./space";

/** @deprecated The cube's face ink; the logo character uses LOGO_INK. */
export const INK = "#2a3f44";
/** @deprecated The cube's eye highlight; the logo character's eye is PAPER. */
export const CREAM = "#f6ecd2";
export const OUTLINE = "#53350f";
/** @deprecated The bow tie goes with the cube's face. */
export const TIE = "#35555c";
/** @deprecated The bow tie goes with the cube's face. */
export const KNOT = "#e9b650";
export const TAPE = "#efd8a4";
export const TAPE_SIDE = "#d8b96e";
export const TAPE_EDGE = "#a5772b";
/** @deprecated No carton wears a shipping label in the new reel. */
export const LABEL = "#f6ecd2";
/** @deprecated No carton wears a shipping label in the new reel. */
export const LABEL_EDGE = "#8a5215";
/** @deprecated No carton wears a shipping label in the new reel. */
export const LABEL_LINES = "#b08d4d";

// The logo character's paint, from docs/public/logo.svg. The terminal
// mascot's palette (sprite.ts) uses the same colours.
export const LOGO_INK = "#20190e";
/** The bare eye. */
export const PAPER = "#f5ead6";
export const LENS = "#d6e8e8";
const LENS_RING = "#9bbbc0";
const GLINT = "#ffffff";
/** The monocle's brass chain. */
export const CHAIN = "#bd7d23";
export const LOGO_TAPE = "#f7e4b8";
/** The inside of the box, under a raised lid. */
export const INSIDE = "#543816";
/** Cheek colours for blush levels 1 to 3; 3 is the logo's rose. */
export const CHEEKS = ["#e6965c", "#e58663", "#e47a68"] as const;
export const BERRY = "#d63e46";
const SEED = "#f7d88a";
const LEAF = "#6aa84f";

// Logo proportions that scenes drawing their own cardboard must match.
/** Outline width per unit of box edge (8 px on the logo's 147 px edge). */
export const OUTLINE_RATIO = 8 / 147;
/** Half-width of the tape band across the top, in the box's [-1, 1] units. */
export const TAPE_HALF = 0.23;
/** How far the tape's end runs down the right panel, as a share of the edge. */
export const TAPE_DROP = 26 / 128;

/**
 * Face features, each animatable. Omitted fields take the character's
 * defaults: LOGO_FACE on the logo character, FACE_FULL on the cube.
 * Coordinates are logo.svg units on the logo character (the front is 120
 * across) and the cube's own face px (104 across).
 */
export interface FaceParams {
  /**
   * 0..1 eyes popped in. On the logo character: the bare eye, scaled up from
   * its center, and the pupil behind the lens.
   */
  eyes?: number;
  /** 0..1 eyes shut: the logo's shut the way the mascot's do, to one level line. */
  blink?: number;
  /** Pupil offset, both eyes. */
  look?: [number, number];
  /**
   * The cube: 0..1 travel, from up and away to seated on the right eye. The
   * logo character: 0..1 popped in, scaled up about the lens (overshoot past
   * 1 is fine), with the lens, rim, inner ring, pupil, and glint arc.
   */
  monocle?: number;
  /** 0..1 a star sparkle on the monocle's rim. */
  glint?: number;
  /** 0..1 mustache grown in from the center. */
  mustache?: number;
  /** Mustache wiggle, radians per half; positive lifts both tips. */
  twitch?: number;

  // The logo character only.
  /**
   * 0..1 the skeptic's flat eyelid lowered toward a squint (the mascot drops
   * it a row while tests run). 0 is the logo.
   */
  eyelid?: number;
  /** Monocle and chain swung about the chain's anchor on the box, radians. */
  swing?: number;
  /** 0..1 the lens glass tinted in (1 is the logo's pale lens). */
  lens?: number;
  /** 0..1 the glint arc on the lens drawn on from its lower end. */
  arc?: number;
  /**
   * The mascot's sweeping glint: a bright band crossing the lens from top
   * left (0) to bottom right (1); null or outside [0, 1] is a matte lens.
   * sprite.ts's band position p is sweep (p + 0.5) / 4.
   */
  sweep?: number | null;
  /** 0..1 the chain's dots drawn out from the ring. */
  chain?: number;
  /**
   * Blush: 0 none, then the mascot's levels 1 to 3 (fractions blend them).
   * Between 0 and 1 the cheeks also scale in.
   */
  cheeks?: number;
  /** 0..1 the strawberry keepsake, scaled up on its seat on the lid. */
  strawberry?: number;
  /** The strawberry's height above its seat, in logo units, for drops and hops. */
  berryLift?: number;

  /** @deprecated The cube's face: 0..1 brows drawn in. */
  brows?: number;
  /** @deprecated The cube's face: brow raise in face px; positive is up. */
  browLift?: number;
  /** @deprecated The cube's face: 0..1 bow tie scaled in. */
  bowtie?: number;
  /** @deprecated The cube's face: bow tie rotation, radians. */
  bowtieSpin?: number;
}

/** Every feature the logo character has. */
export type LogoFace = Required<Omit<FaceParams, "brows" | "browLift" | "bowtie" | "bowtieSpin">>;

/** The logo's face: every feature in, cheeks at the rose, no strawberry. */
export const LOGO_FACE: Readonly<LogoFace> = {
  eyes: 1,
  blink: 0,
  look: [0, 0],
  monocle: 1,
  glint: 0,
  mustache: 1,
  twitch: 0,
  eyelid: 0,
  swing: 0,
  lens: 1,
  arc: 1,
  sweep: null,
  chain: 1,
  cheeks: 3,
  strawberry: 0,
  berryLift: 0,
};

/** @deprecated The cube's full face; the logo character's is LOGO_FACE. */
export const FACE_FULL: Readonly<Required<FaceParams>> = {
  ...LOGO_FACE,
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
  /** "cube" (the default): the old isometric character and plain cartons. */
  kind?: "cube" | "logo";
  /** World position of the center of the bottom face. */
  pos: V3;
  /** Width in world units. */
  size?: number;
  /**
   * Width, height, and depth as multiples of size. The logo character's
   * default is LOGO_DIMS, the cube's [1, 1, 1].
   */
  dims?: V3;
  /** Turn about the box's own vertical axis, radians. */
  yaw?: number;
  /** Tumble about the box center: around world X, then world Z, radians. */
  tiltX?: number;
  tiltZ?: number;
  /** Vertical scale about the bottom face; width compensates to keep volume. */
  squash?: number;
  /**
   * The cube: 0..1 packing tape laid across the top and down the right
   * panel. The logo character: 0..1 its tab laid over the lid from the back
   * (to TAPE_TOP), then down the front.
   */
  tape?: number;
  /** @deprecated The cube: 0..1 shipping label stamped onto the right panel. */
  label?: number;
  /** Face features on the front (the cube's left panel); null for a plain box. */
  face?: FaceParams | null;
  /** Outline width multiplier; 1 matches the cube's logo, and the logo character has none. */
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

  // The logo character only.
  /**
   * How far the lid's right end rises above the box, in the terminal
   * mascot's steps (LID_STEP each; sprite.ts LID_MAX is 4), its left end
   * hinged on the rim as mascot.rs's stamp_lid draws it. 0 is shut.
   * Fractions glide.
   */
  lid?: number;
  /** The lid swung open about its back edge, radians. */
  lidTilt?: number;
  /** 0 the flat logo colours, 1 the cube's shaded cardboard gradient. */
  shade?: number;
  /**
   * Drawn in the open mouth: over the dark inside, under the walls and the
   * lid, so a carton dropped into the gap disappears into the box. Called
   * only while the lid is open.
   */
  inside?: (ctx: CanvasRenderingContext2D) => void;
}

/** @deprecated The cube's logo pose; the logo character's is LOGO_POSE. */
export const HERO_POSE: Readonly<BoxPose> = {
  pos: [0, -0.5, 0],
  size: 1,
  tape: 1,
  label: 1,
  face: FACE_FULL,
};

/** logo.svg units per box width: the front panel spans x 4..124. */
const LOGO_UNITS = 120;
/** The logo character: width, height to the top of the lid, and depth. */
export const LOGO_DIMS: Readonly<V3> = [1, 0.85, 1];
/** The lid slab's thickness in box widths: the logo's 5-unit front edge. */
export const LID_THICK = 5 / LOGO_UNITS;
/** One lid step in box widths: a pixel of the 16-pixel-wide terminal mascot. */
export const LID_STEP = 1 / 16;
/** The share of `tape` spent laying the tab over the lid; the rest runs down the front. */
export const TAPE_TOP = 0.5;

/** The logo: taped shut, every feature in, rosy cheeks, no strawberry. */
export const LOGO_POSE: Readonly<BoxPose> = {
  kind: "logo",
  pos: [0, 0, 0],
  size: 1,
  tape: 1,
  face: LOGO_FACE,
};

/** How far the logo's eye is from the box's center, in box widths. */
const EYE_DIST = 5.5;
/** How far above the lid the logo's eye is, in box widths. */
const EYE_RISE = 0.75;

/**
 * The logo's own camera, placed so LOGO_POSE (a size 1 box standing on the
 * origin) draws exactly as logo.svg would in the square at (x, y) with side
 * `size` px. It is a one-point perspective from straight ahead (yaw 0, pitch
 * 0) with the eye 5.5 box widths out and 0.75 above the lid: that makes the
 * lid's far edge 100/120 of its near one and its top face 15 logo units deep.
 * The horizon, and so (cx, cy), sits at logo y -68.
 */
export function logoCam(x: number, y: number, size: number): Camera {
  const k = size / 128;
  const near = EYE_DIST / (EYE_DIST - LOGO_DIMS[2] / 2);
  return {
    cx: x + 64 * k,
    cy: y + (22 - EYE_RISE * LOGO_UNITS) * k,
    scale: (LOGO_UNITS / near) * k,
    yaw: 0,
    pitch: 0,
    persp: EYE_DIST,
    target: [0, LOGO_DIMS[1] + EYE_RISE, 0],
  };
}

/** The logo view with the logo 480 px square in the middle of the frame. */
export const FRONT_CAM: Camera = logoCam(720, 300, 480);

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
  /** Width, height, and depth as multiples of size. */
  dims: Readonly<V3>;
}

const CUBE_DIMS: Readonly<V3> = [1, 1, 1];

export function boxFrame(pose: BoxPose): BoxFrame {
  const s = pose.size ?? 1;
  const sq = pose.squash ?? 1;
  const dims = pose.dims ?? (pose.kind === "logo" ? LOGO_DIMS : CUBE_DIMS);
  const wide = 1 / Math.sqrt(Math.max(sq, 0.05));
  const h = s / 2;
  let x: V3 = [h * dims[0] * wide, 0, 0];
  let y: V3 = [0, h * dims[1] * sq, 0];
  let z: V3 = [0, 0, h * dims[2] * wide];
  const yaw = pose.yaw ?? 0;
  if (yaw) {
    const o: V3 = [0, 0, 0];
    x = rotateAround(x, o, [0, 1, 0], yaw);
    z = rotateAround(z, o, [0, 1, 0], yaw);
  }
  let c: V3 = add(pose.pos, [0, h * dims[1] * sq, 0]);
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
  return { c, x, y, z, size: s, squash: sq, dims };
}

/** Point in the box from local coordinates in [-1, 1]^3. */
export function boxPoint(f: BoxFrame, lx: number, ly: number, lz: number): V3 {
  return add(f.c, add(mul(f.x, lx), add(mul(f.y, ly), mul(f.z, lz))));
}

/** The box's corners; the logo character's include its lid wherever it is. */
export function boxCorners(pose: BoxPose): V3[] {
  if (pose.kind === "logo") return logoCorners(logoBox(pose));
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
 * Plane matrix for a cube panel's decoration space, in the old logo's own
 * units: face and right panels use the SVG's sheared local frames (104 px
 * across, 128 px down per edge); the top uses unit coordinates (u along x, v
 * along z). The logo character's front plane is frontMatrix().
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

/**
 * Screen position of a point on the face: logo.svg units on the logo
 * character's front, the old face px on the cube.
 */
export function faceToScreen(view: View, pose: BoxPose, lx: number, ly: number): Projected {
  if (pose.kind === "logo") return view.project(frontPoint(logoBox(pose), lx, ly));
  const f = boxFrame(pose);
  return view.project(
    add(boxPoint(f, 0, 0, 1), add(mul(f.x, (2 * lx) / 104), mul(f.y, (-2 * ly) / 128))),
  );
}

/**
 * The seated monocle's center and screen radius: to the middle of the logo
 * character's rim (logo (86, 62), r 24.5), or the cube's ring.
 */
export function monocleScreen(view: View, pose: BoxPose): { x: number; y: number; r: number } {
  const [cx, cy, r] = pose.kind === "logo" ? MONOCLE : [18, -28, 16.5];
  const c = faceToScreen(view, pose, cx, cy);
  const e = faceToScreen(view, pose, cx + r, cy);
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
 * @deprecated The cube's monocle paths in its face panel's coordinates, for
 * scenes that fly it in themselves before handing it back to drawBox.
 */
export function monoclePaths(): { ring: Path2D; glint: Path2D; chain: Path2D; bead: Path2D } {
  const p = P();
  return { ring: p.ring, glint: p.glint, chain: p.chain, bead: p.bead };
}

/** @deprecated The shipping label at rest, in the cube's right panel coordinates. */
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
  if (pose.kind === "logo") {
    drawLogoBox(ctx, view, pose);
    return;
  }
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

// The logo character. Its parts in the box's local [-1, 1] coordinates: the
// body from the floor (y -1) to the rim, the lid slab from the rim to the top
// (y 1), turned `hinge` about its left bottom edge and swung `tilt` about its
// back edge. The front art is in logo.svg units: x 4..124 across the front,
// y 22 at the top of the lid's front edge down to 124 at the floor.

/** The monocle: center and rim radius in logo units. */
const MONOCLE: readonly [number, number, number] = [86, 62, 24.5];
/** Where the eye closes: the mascot's shut line, a row below its eyelid. */
const SHUT_Y = 63.5;
/** How far the eyelid drops in a full squint, in logo units. */
const SQUINT = 7;
/** The rim's cardboard, inset from the walls, in box widths. */
const WALL = 0.025;
/** The tape tab's half-width at the lid's back and front edges, in local x. */
const TAB_BACK = 0.14;
const TAB_FRONT = 2 / 15;
/** The strawberry's seat on the lid, in local x and z, and its width in box widths. */
const BERRY_SEAT: readonly [number, number] = [-0.42, 0];
const BERRY_WIDTH = 0.22;

let logoPaths: Record<string, Path2D> | null = null;
/** logo.svg's own paths, verbatim, in logo units. */
function LP(): Record<string, Path2D> {
  if (logoPaths) return logoPaths;
  logoPaths = {
    cheekL: new Path2D("M7.5 94a6.5 4.5 0 1 0 13 0a6.5 4.5 0 1 0-13 0z"),
    cheekR: new Path2D("M99.5 94a6.5 4.5 0 1 0 13 0a6.5 4.5 0 1 0-13 0z"),
    white: new Path2D("M20.5 56.5h25A14 14 0 1 1 20.5 56.5z"),
    arc: new Path2D("M69 57a18 18 0 0 1 10-12"),
    chain: new Path2D("M110 72c6 6 10 14 10 24"),
    mustache: new Path2D(
      "M60 99C54 93 42 93 35 99C31 103 25 102 23 95C19 104 26 111 36 110C45 109 53 106 60 106C67 106 75 109 84 110C94 111 101 104 97 95C95 102 89 103 85 99C78 93 66 93 60 99z",
    ),
  };
  return logoPaths;
}

/** The glint arc's length, for drawing it on: 18 units of radius over its chord of 2√61. */
const ARC_LEN = 36 * Math.asin(Math.sqrt(61) / 18);
/** The chain's dots, `.1 4.6` along its curve: six fit. */
const CHAIN_DOTS = 6;

interface LogoBox {
  f: BoxFrame;
  /** World vectors per logo unit: rightward along the front, and up. */
  ex: V3;
  up: V3;
  /** Local y of the rim, where the shut lid's slab begins. */
  rim: number;
  /** The lid's turn on its hinge and its swing, radians. */
  hinge: number;
  tilt: number;
  shut: boolean;
  /** The hinge, along the lid's left bottom edge: its middle and its axis. */
  hingePivot: V3;
  hingeAxis: V3;
  /** The lid's axis of swing, through its back bottom edge. */
  pivot: V3;
  axis: V3;
}

/**
 * The lid's turn on its left-hand hinge that raises its right end, top and
 * all, `rise` lid widths: sin a + t (cos a - 1) = rise for a lid `t` widths
 * thick.
 */
function hingeAngle(rise: number, t: number): number {
  if (rise <= 0) return 0;
  return Math.asin(Math.min((rise + t) / Math.hypot(1, t), 1)) - Math.atan(t);
}

function logoBox(pose: BoxPose): LogoBox {
  const f = boxFrame(pose);
  const [w, h] = f.dims;
  // f.x is half the width, 60 logo units; f.y half the height, 60 h / w.
  const ex = mul(f.x, 2 / LOGO_UNITS);
  const up = mul(f.y, (2 * w) / (LOGO_UNITS * h));
  const rim = 1 - (2 * LID_THICK * w) / h;
  // The lid's width and thickness as it stands, squash and all.
  const width = 2 * len(f.x);
  const thick = (1 - rim) * len(f.y);
  const hinge = hingeAngle(Math.max(pose.lid ?? 0, 0) * LID_STEP, thick / width);
  const tilt = pose.lidTilt ?? 0;
  return {
    f,
    ex,
    up,
    rim,
    hinge,
    tilt,
    shut: hinge === 0 && tilt === 0,
    hingePivot: boxPoint(f, -1, rim, 0),
    // Turning about the depth axis carries the right end up.
    hingeAxis: norm(f.z),
    pivot: boxPoint(f, 0, rim, -1),
    axis: norm(f.x),
  };
}

/** A point on the lid slab from local coordinates at rest (y from the rim to 1). */
function lidPoint(b: LogoBox, lx: number, ly: number, lz: number): V3 {
  let p = boxPoint(b.f, lx, ly, lz);
  if (b.hinge) p = rotateAround(p, b.hingePivot, b.hingeAxis, b.hinge);
  return b.tilt ? rotateAround(p, b.pivot, b.axis, -b.tilt) : p;
}

/** A direction on the lid, turned with it. */
function lidVector(b: LogoBox, v: V3): V3 {
  const o: V3 = [0, 0, 0];
  const hinged = b.hinge ? rotateAround(v, o, b.hingeAxis, b.hinge) : v;
  return b.tilt ? rotateAround(hinged, o, b.axis, -b.tilt) : hinged;
}

/** A point on the front plane (local z 1) from logo coordinates. */
function frontPoint(b: LogoBox, x: number, y: number): V3 {
  return add(boxPoint(b.f, 0, -1, 1), add(mul(b.ex, x - 64), mul(b.up, 124 - y)));
}

function logoCorners(b: LogoBox): V3[] {
  const out: V3[] = [];
  for (const i of [-1, 1]) {
    for (const k of [-1, 1]) {
      out.push(boxPoint(b.f, i, -1, k), boxPoint(b.f, i, b.rim, k));
      out.push(lidPoint(b, i, b.rim, k), lidPoint(b, i, 1, k));
    }
  }
  return out;
}

/**
 * Canvas transform for the logo character's front plane in logo units:
 * exact whenever the front faces the camera square on, and otherwise
 * linearized at logo point (x, y), so draw each feature about its own center.
 */
export function frontMatrix(view: View, pose: BoxPose, x = 64, y = 73) {
  return featureMatrix(view, logoBox(pose), x, y);
}

function featureMatrix(view: View, b: LogoBox, x: number, y: number) {
  const m = view.planeMatrix(frontPoint(b, x, y), b.ex, mul(b.up, -1));
  // Logo coordinates in, rather than offsets from (x, y).
  return { ...m, e: m.e - m.a * x - m.c * y, f: m.f - m.b * x - m.d * y };
}

/**
 * The open mouth: the rim's inner outline on screen, and its middle, where a
 * carton aims to drop into the box.
 */
export function mouth(view: View, pose: BoxPose): { pts: Projected[]; x: number; y: number } {
  const b = logoBox(pose);
  const [w, , d] = b.f.dims;
  const ix = 1 - (2 * WALL) / w;
  const iz = 1 - (2 * WALL) / d;
  const pts = [
    [-ix, -iz],
    [ix, -iz],
    [ix, iz],
    [-ix, iz],
  ].map(([x, z]) => view.project(boxPoint(b.f, x, b.rim, z)));
  const c = view.project(boxPoint(b.f, 0, b.rim, 0));
  return { pts, x: c.x, y: c.y };
}

/**
 * The logo character's walls, each seen from outside: its bottom corners,
 * left then right, in local x and z.
 */
const WALLS: { id: PanelId; a: [number, number]; b: [number, number]; normal: (f: BoxFrame) => V3 }[] = [
  { id: "face", a: [-1, 1], b: [1, 1], normal: (f) => norm(f.z) },
  { id: "right", a: [1, 1], b: [1, -1], normal: (f) => norm(f.x) },
  { id: "back", a: [1, -1], b: [-1, -1], normal: (f) => norm(mul(f.z, -1)) },
  { id: "left", a: [-1, -1], b: [-1, 1], normal: (f) => norm(mul(f.x, -1)) },
];

/** Quarter-circle corners in logo units, as points (y down). */
function corner(out: [number, number][], cx: number, cy: number, r: number, a0: number): void {
  const n = 12;
  for (let i = 0; i <= n; i++) {
    const a = a0 + (i / n) * (Math.PI / 2);
    out.push([cx + r * Math.cos(a), cy + r * Math.sin(a)]);
  }
}

/** The logo's rounded bottom corners, in logo units. */
const FOOT = 3;
/** How far a neighbouring wall reaches from their corner on screen, in logo units, before the feet there are square. */
const JOIN = 24;

/**
 * A wall's outline from logo height `top` to the floor, `span` logo units
 * wide, with rounded bottom corners of radius `ra` on its left and `rb` on its
 * right (the logo's FOOT). With top 118 it is the base band.
 */
function wallOutline(top: number, span: number, ra = FOOT, rb = FOOT): [number, number][] {
  const pts: [number, number][] = [
    [0, top],
    [span, top],
  ];
  corner(pts, span - rb, 124 - rb, rb, 0);
  corner(pts, ra, 124 - ra, ra, Math.PI / 2);
  return pts;
}

/**
 * Trace a wall's outline (wallOutline, in logo units from the wall's bottom
 * left corner `origin`) as the current path. Where the wall's plane maps to
 * the screen exactly (an orthographic camera, or the wall square on to a
 * perspective one, as the logo's front is) its corners are true arcs, as
 * logo.svg draws them; otherwise projected points. Returns those points.
 */
function traceWall(
  ctx: CanvasRenderingContext2D,
  view: View,
  origin: V3,
  along: V3,
  up: V3,
  n: V3,
  top: number,
  span: number,
  ra = FOOT,
  rb = FOOT,
): Projected[] {
  const pts = wallOutline(top, span, ra, rb).map(([x, y]) =>
    view.project(add(origin, add(mul(along, x), mul(up, 124 - y)))),
  );
  if (view.cam.persp && dot(n, view.v) < 1 - 1e-9) {
    polygon(ctx, pts);
    return pts;
  }
  ctx.save();
  applyMatrix(ctx, view.planeMatrix(add(origin, mul(up, 124)), along, mul(up, -1)));
  ctx.beginPath();
  ctx.moveTo(0, top);
  ctx.lineTo(span, top);
  ctx.arc(span - rb, 124 - rb, rb, 0, Math.PI / 2);
  ctx.arc(ra, 124 - ra, ra, Math.PI / 2, Math.PI);
  ctx.closePath();
  ctx.restore();
  return pts;
}

/**
 * Fill the current path, a face of tone `t`: flat logo colour, or part or
 * all of the way to the cube's cardboard gradient across `pts`.
 */
function fillFace(
  ctx: CanvasRenderingContext2D,
  pts: readonly Projected[],
  t: number,
  shade: number,
): void {
  const k = clamp(shade);
  if (k <= 0) {
    ctx.fillStyle = flatCard(t);
  } else if (k >= 1) {
    ctx.fillStyle = cardboardFill(ctx, pts, t);
  } else {
    const flat = flatCard(t);
    const [a, c] = cardboard(t);
    // cardboardFill's gradient line, its stops part way from the flat colour.
    const xs = pts.map((p) => p.x);
    const ys = pts.map((p) => p.y);
    const m = ctx.createLinearGradient(Math.min(...xs), Math.min(...ys), Math.max(...xs), Math.max(...ys));
    m.addColorStop(0, mix(flat, a, k));
    m.addColorStop(1, mix(flat, c, k));
    ctx.fillStyle = m;
  }
  ctx.fill();
}

function drawLogoBox(ctx: CanvasRenderingContext2D, view: View, pose: BoxPose): void {
  const alpha = pose.alpha ?? 1;
  if (alpha <= 0) return;
  ctx.save();
  ctx.globalAlpha *= alpha;
  if (pose.flat) {
    polygon(ctx, boxSilhouette(view, pose));
    ctx.fillStyle = pose.flat;
    ctx.fill();
    ctx.restore();
    return;
  }
  const b = logoBox(pose);
  const { f } = b;
  const [w, h] = f.dims;
  const shift = pose.toneShift ?? 0;
  const shade = pose.shade ?? 0;
  const lw = OUTLINE_RATIO * f.size * view.cam.scale * (pose.outline ?? 0);
  const outline = (pts: readonly Projected[]) => {
    if (lw <= 0) return;
    ctx.strokeStyle = OUTLINE;
    ctx.lineJoin = "round";
    ctx.lineWidth = lw * pts[0].f;
    ctx.stroke();
  };
  const face = (pts: readonly Projected[], t: number) => {
    polygon(ctx, pts);
    fillFace(ctx, pts, t, shade);
    outline(pts);
  };
  // Where two walls, or two of the lid's edges, meet and both show, the one
  // painted first is carried a hair under the other along the edge from `a`
  // to `c`, in its own fill, so the background does not show through where
  // their antialiased edges meet. Never wider than either face reaches from
  // the edge (`width`, on screen), so it stays inside the box as one turns
  // out of view.
  const seam = (a: V3, c: V3, width: number) => {
    if (width <= 0) return;
    const [p, q] = [view.project(a), view.project(c)];
    ctx.beginPath();
    ctx.moveTo(p.x, p.y);
    ctx.lineTo(q.x, q.y);
    ctx.strokeStyle = ctx.fillStyle;
    ctx.lineWidth = Math.min(1, width);
    ctx.lineCap = "butt";
    ctx.stroke();
  };
  /** How far `far` lies on screen from the line through `a` and `c`: a face's reach from its edge, 0 edge on. */
  const reach = (a: V3, c: V3, far: V3) => {
    const [p, q, r] = [view.project(a), view.project(c), view.project(far)];
    const [ex, ey] = [q.x - p.x, q.y - p.y];
    const l = Math.hypot(ex, ey);
    return l > 1e-9 ? Math.abs(ex * (r.y - p.y) - ey * (r.x - p.x)) / l : 0;
  };
  // The walls in WALLS order, which is the order they are painted: each
  // wall's right-hand neighbour is the next, its left-hand one the last.
  const walls = WALLS.map((wall) => {
    const pa = boxPoint(f, wall.a[0], -1, wall.a[1]);
    const pb = boxPoint(f, wall.b[0], -1, wall.b[1]);
    const n = wall.normal(f);
    return { ...wall, pa, pb, n, shown: view.facing(n, add(mul(add(pa, pb), 0.5), f.y)) };
  });
  /** The lid edges painted after edge `i` and beside it: its right-hand (side 0) then left-hand neighbour, where shown. */
  const later = (i: number, shown: (j: number) => boolean) =>
    [(i + 1) % 4, (i + 3) % 4].map((j, side) => ({ j, side })).filter(({ j }) => j > i && shown(j));
  /** Each wall's gradient, which its base band and lid edge share. */
  const wallPts: Partial<Record<PanelId, Projected[]>> = {};
  // Logo heights: the lid's top and the rim.
  const yTop = 124 - (LOGO_UNITS * h) / w;
  const yRim = yTop + LID_THICK * LOGO_UNITS;

  // The open mouth, then whatever is dropping into it.
  const topN = norm(f.y);
  if (!b.shut && view.facing(topN, boxPoint(f, 0, b.rim, 0))) {
    const t = tone(view, topN) + shift;
    face(
      [
        [-1, -1],
        [1, -1],
        [1, 1],
        [-1, 1],
      ].map(([x, z]) => view.project(boxPoint(f, x, b.rim, z))),
      t - FLAT_BAND,
    );
    polygon(ctx, mouth(view, pose).pts);
    ctx.fillStyle = INSIDE;
    ctx.fill();
    pose.inside?.(ctx);
  }

  // The lid slab: its top, carrying the tape's tab, and its edges (the
  // logo's front band).
  const tape = pose.tape ?? 0;
  const lidN = lidVector(b, topN);
  const lidFaces: { pts: V3[]; n: V3; top: boolean; wall?: number }[] = [
    {
      pts: [
        lidPoint(b, -1, 1, -1),
        lidPoint(b, 1, 1, -1),
        lidPoint(b, 1, 1, 1),
        lidPoint(b, -1, 1, 1),
      ],
      n: lidN,
      top: true,
    },
    {
      pts: [
        lidPoint(b, -1, b.rim, 1),
        lidPoint(b, 1, b.rim, 1),
        lidPoint(b, 1, b.rim, -1),
        lidPoint(b, -1, b.rim, -1),
      ],
      n: mul(lidN, -1),
      top: false,
    },
  ];
  WALLS.forEach((wall, i) => {
    const [ax, az] = wall.a;
    const [bx, bz] = wall.b;
    lidFaces.push({
      pts: [
        lidPoint(b, ax, 1, az),
        lidPoint(b, bx, 1, bz),
        lidPoint(b, bx, b.rim, bz),
        lidPoint(b, ax, b.rim, az),
      ],
      n: lidVector(b, wall.normal(f)),
      top: false,
      wall: i,
    });
  });
  const drawLid = (top: boolean) => {
    const shown = lidFaces.map(
      (lf) => lf.top === top && view.facing(lf.n, mul(lf.pts.reduce((s, p) => add(s, p), [0, 0, 0] as V3), 0.25)),
    );
    lidFaces.forEach((lf, at) => {
      if (!shown[at]) return;
      const pts = lf.pts.map((p) => view.project(p));
      polygon(ctx, pts);
      const t = tone(view, lf.n) + shift - (top ? 0 : FLAT_BAND);
      // An edge band shades as its wall does, one step darker.
      fillFace(ctx, (lf.wall !== undefined && wallPts[WALLS[lf.wall].id]) || pts, t, shade);
      if (lf.wall !== undefined) {
        const i = lf.wall;
        for (const { j, side } of later(i, (j) => shown[2 + j])) {
          const [a, c] = side === 0 ? [lf.pts[1], lf.pts[2]] : [lf.pts[0], lf.pts[3]];
          // Each edge's far end along the lid, on its top.
          const mine = side === 0 ? lf.pts[0] : lf.pts[1];
          const theirs = lidFaces[2 + j].pts[side === 0 ? 1 : 0];
          seam(a, c, Math.min(reach(a, c, mine), reach(a, c, theirs)));
        }
        if (lw > 0) polygon(ctx, pts);
      }
      outline(pts);
      if (top && tape > 0) {
        // The tab, laid from the back edge toward the front, from projected
        // corners; slightly wider at the back, as the logo draws it.
        const k = clamp(tape / TAPE_TOP);
        const z1 = lerp(-1, 1, k);
        const half = lerp(TAB_BACK, TAB_FRONT, k);
        polygon(
          ctx,
          [
            lidPoint(b, -TAB_BACK, 1, -1),
            lidPoint(b, TAB_BACK, 1, -1),
            lidPoint(b, half, 1, z1),
            lidPoint(b, -half, 1, z1),
          ].map((p) => view.project(p)),
        );
        ctx.fillStyle = LOGO_TAPE;
        ctx.fill();
      }
    });
  };

  // Shut, the box is one convex solid whose faces never overlap on screen,
  // so it paints in the logo's order: the lid's top, the walls, the edges.
  // Open, the lid comes last, over the mouth and the body.
  if (b.shut) drawLid(true);

  // The body. A shut box's walls run up under the lid's front edge, as the
  // logo's front panel does, so no seam shows where they meet.
  const bottomN = mul(topN, -1);
  if (view.facing(bottomN, boxPoint(f, 0, -1, 0))) {
    face(
      [
        [-1, 1],
        [1, 1],
        [1, -1],
        [-1, -1],
      ].map(([x, z]) => view.project(boxPoint(f, x, -1, z))),
      tone(view, bottomN) + shift - FLAT_BAND,
    );
  }
  let front = false;
  const top = b.shut ? yTop : yRim;
  walls.forEach((wall, i) => {
    if (!wall.shown) return;
    const { pa, pb, n } = wall;
    if (wall.id === "face") front = true;
    // The wall's width in logo units, which keep their size on every wall.
    const span = len(sub(pb, pa)) / len(b.ex);
    const along = mul(sub(pb, pa), 1 / span);
    const t = tone(view, n) + shift;
    const up = (p: V3, y: number) => add(p, mul(b.up, 124 - y));
    // Its corners with its right-hand and left-hand neighbours. A foot beside
    // a neighbour that shows squares off as the neighbour turns into view,
    // square once the neighbour reaches JOIN logo units from the corner on
    // screen, so no notch opens between two rounded feet. With no neighbour
    // showing, as in the logo seen square on, both feet are round.
    const corners = [
      { j: (i + 1) % 4, at: pb, mine: pa, theirs: walls[(i + 1) % 4].pb },
      { j: (i + 3) % 4, at: pa, mine: pb, theirs: walls[(i + 3) % 4].pa },
    ].map(({ j, at, mine, theirs }) => {
      if (!walls[j].shown) return { j, at, r: FOOT, width: 0 };
      const head = up(at, 0);
      const [p, q] = [view.project(at), view.project(head)];
      const unit = Math.hypot(q.x - p.x, q.y - p.y) / 124;
      const near = reach(at, head, theirs);
      return {
        j,
        at,
        r: FOOT * (1 - smoothstep(0, JOIN * unit, near)),
        width: Math.min(near, reach(at, head, mine)),
      };
    });
    const [rb, ra] = [corners[0].r, corners[1].r];
    const seams = corners.filter(({ j }) => j > i && walls[j].shown);
    const pts = traceWall(ctx, view, pa, along, b.up, n, top, span, ra, rb);
    fillFace(ctx, pts, t, shade);
    for (const c of seams) seam(up(c.at, top), up(c.at, 118), c.width);
    if (lw > 0 && seams.length) traceWall(ctx, view, pa, along, b.up, n, top, span, ra, rb);
    outline(pts);
    // The base band takes its wall's gradient, so it reads as the wall's
    // foot, one step darker, rather than a strip shaded on its own.
    traceWall(ctx, view, pa, along, b.up, n, 118, span, ra, rb);
    fillFace(ctx, pts, t - FLAT_BAND, shade);
    for (const c of seams) seam(up(c.at, 118), up(c.at, 124 - c.r), c.width);
    wallPts[wall.id] = pts;
  });

  if (!b.shut) drawLid(true);
  drawLid(false);

  if (front) {
    // The tape down the front: over the lid's edge and onto the body, in one
    // piece while the lid is shut.
    const down = clamp((tape - TAPE_TOP) / (1 - TAPE_TOP));
    if (down > 0) {
      const fill = (pts: V3[]) => {
        polygon(ctx, pts.map((p) => view.project(p)));
        ctx.fill();
      };
      const [x0, x1] = [64 - TAB_FRONT * 60, 64 + TAB_FRONT * 60];
      const strip = (from: number, to: number) =>
        fill([
          frontPoint(b, x0, from),
          frontPoint(b, x1, from),
          frontPoint(b, x1, to),
          frontPoint(b, x0, to),
        ]);
      const y1 = yTop + 14 * down;
      ctx.fillStyle = LOGO_TAPE;
      if (b.shut) {
        strip(yTop, y1);
      } else {
        const lo = lerp(1, b.rim, clamp((y1 - yTop) / (yRim - yTop)));
        fill([
          lidPoint(b, -TAB_FRONT, 1, 1),
          lidPoint(b, TAB_FRONT, 1, 1),
          lidPoint(b, TAB_FRONT, lo, 1),
          lidPoint(b, -TAB_FRONT, lo, 1),
        ]);
        if (y1 > yRim) strip(yRim, y1);
      }
    }
    if (pose.face) drawLogoFace(ctx, view, b, pose.face);
  }
  if (pose.face) drawLogoBerry(ctx, view, b, pose.face);
  ctx.restore();
}

function cheekColor(level: number): string {
  if (level <= 1) return CHEEKS[0];
  if (level >= 3) return CHEEKS[2];
  const i = Math.floor(level);
  if (i === level) return CHEEKS[i - 1];
  return mix(CHEEKS[i - 1], CHEEKS[i], level - i);
}

/**
 * The face, in the logo's own paint order: cheeks, the bare eye, the
 * monocle, its chain, then the mustache. Each feature is drawn in the front
 * plane linearized at its own center.
 */
function drawLogoFace(ctx: CanvasRenderingContext2D, view: View, b: LogoBox, face: FaceParams): void {
  const p = LP();
  const v = { ...LOGO_FACE, ...face };
  const at = (x: number, y: number) => {
    ctx.save();
    applyMatrix(ctx, featureMatrix(view, b, x, y));
  };
  const about = (x: number, y: number, sx: number, sy: number) => {
    if (sx === 1 && sy === 1) return;
    ctx.translate(x, y);
    ctx.scale(sx, sy);
    ctx.translate(-x, -y);
  };
  const [lookX, lookY] = v.look;
  ctx.lineCap = "round";

  // Cheeks, scaled in and warmed through the mascot's three levels.
  if (v.cheeks > 0) {
    const k = clamp(v.cheeks);
    for (const [path, x] of [
      [p.cheekL, 14],
      [p.cheekR, 106],
    ] as const) {
      at(x, 94);
      about(x, 94, k, k);
      ctx.fillStyle = cheekColor(v.cheeks);
      ctx.fill(path);
      ctx.restore();
    }
  }

  // The bare eye: the white and the pupil under the flat eyelid. A blink
  // squashes both toward the shut line as the eyelid comes down to it.
  const shut = clamp(v.blink);
  if (v.eyes > 0) {
    at(33, SHUT_Y);
    ctx.globalAlpha *= clamp(v.eyes * 3);
    about(33, SHUT_Y, v.eyes, v.eyes);
    const lidY = 56.5 + SQUINT * clamp(v.eyelid);
    const open = 1 - shut;
    if (open > 0.001) {
      ctx.save();
      about(33, SHUT_Y, 1, open);
      if (lidY > 56.5) {
        ctx.beginPath();
        ctx.rect(10, lidY, 46, 30);
        ctx.clip();
      }
      ctx.fillStyle = PAPER;
      ctx.fill(p.white);
      ctx.clip(p.white);
      ctx.beginPath();
      ctx.arc(33 + lookX, 66 + lookY, 6.5, 0, TAU);
      ctx.fillStyle = LOGO_INK;
      ctx.fill();
      ctx.restore();
    }
    const y = SHUT_Y - (SHUT_Y - lidY) * open;
    ctx.beginPath();
    ctx.moveTo(18, y);
    ctx.lineTo(48, y);
    ctx.strokeStyle = LOGO_INK;
    ctx.lineWidth = 3.5;
    ctx.stroke();
    ctx.restore();
  }

  // The monocle, swung on its chain about the chain's anchor.
  const [mx, my, mr] = MONOCLE;
  if (v.monocle > 0 || v.chain > 0) {
    at(mx, my);
    if (v.swing) {
      ctx.translate(120, 96);
      ctx.rotate(v.swing);
      ctx.translate(-120, -96);
    }
    if (v.monocle > 0) {
      ctx.save();
      ctx.globalAlpha *= clamp(v.monocle * 3);
      about(mx, my, v.monocle, v.monocle);
      ctx.beginPath();
      ctx.arc(mx, my, mr, 0, TAU);
      if (v.lens > 0) {
        ctx.fillStyle = v.lens >= 1 ? LENS : rgba(LENS, v.lens);
        ctx.fill();
      }
      ctx.strokeStyle = LOGO_INK;
      ctx.lineWidth = 7;
      ctx.stroke();
      // The mascot's sweep: a band of light across the glass.
      const sweep = v.sweep;
      if (sweep !== null && sweep >= 0 && sweep <= 1) {
        ctx.save();
        ctx.beginPath();
        ctx.arc(mx, my, 18, 0, TAU);
        ctx.clip();
        ctx.translate(mx, my);
        ctx.rotate(Math.PI / 4);
        ctx.fillStyle = rgba(GLINT, 0.9);
        ctx.fillRect(lerp(-24, 24, sweep) - 4, -mr, 8, 2 * mr);
        ctx.restore();
      }
      ctx.beginPath();
      ctx.arc(mx, my, 19.5, 0, TAU);
      ctx.strokeStyle = LENS_RING;
      ctx.lineWidth = 3;
      ctx.stroke();
      // The eye behind the glass, shut to a line across the lens in a blink.
      if (v.eyes > 0 && shut < 1) {
        ctx.save();
        ctx.beginPath();
        ctx.arc(mx, my, 18, 0, TAU);
        ctx.clip();
        about(86 + lookX, SHUT_Y + lookY, v.eyes, v.eyes * (1 - shut));
        ctx.beginPath();
        ctx.arc(86 + lookX, 63 + lookY, 8.5, 0, TAU);
        ctx.fillStyle = LOGO_INK;
        ctx.fill();
        ctx.restore();
      }
      if (v.eyes > 0 && shut > 0) {
        // Level with the bare eye's shut line, and hidden behind the
        // squashing pupil until the eye is nearly shut.
        const half = lerp(6.75, 14.25, smoothstep(0.5, 1, shut)) * v.eyes;
        ctx.save();
        ctx.globalAlpha *= smoothstep(0.3, 0.7, shut);
        ctx.beginPath();
        ctx.moveTo(86 - half, SHUT_Y);
        ctx.lineTo(86 + half, SHUT_Y);
        ctx.strokeStyle = LOGO_INK;
        ctx.lineWidth = 3.5;
        ctx.stroke();
        ctx.restore();
      }
      if (v.arc > 0) {
        if (v.arc < 1) ctx.setLineDash([ARC_LEN * v.arc, ARC_LEN]);
        ctx.strokeStyle = GLINT;
        ctx.lineWidth = 4;
        ctx.stroke(p.arc);
        ctx.setLineDash([]);
      }
      ctx.restore();
    }
    // The chain's dots, from the ring out to the box's side.
    const dots = Math.round(clamp(v.chain) * CHAIN_DOTS);
    if (dots > 0) {
      const dash = [];
      for (let i = 0; i < dots; i++) dash.push(0.1, 4.6);
      if (dots < CHAIN_DOTS) dash[dash.length - 1] = 99;
      ctx.setLineDash(dash);
      ctx.strokeStyle = CHAIN;
      ctx.lineWidth = 3;
      ctx.stroke(p.chain);
      ctx.setLineDash([]);
    }
    ctx.restore();
  }

  // The mustache grows from the middle; twitching turns its halves apart.
  if (v.mustache > 0) {
    at(60, 101);
    about(60, 101, v.mustache, lerp(0.6, 1, v.mustache));
    ctx.fillStyle = LOGO_INK;
    if (!v.twitch) {
      ctx.fill(p.mustache);
    } else {
      for (const dir of [-1, 1]) {
        ctx.save();
        // The halves overlap a little so no seam opens between them.
        ctx.beginPath();
        ctx.rect(dir < 0 ? 0 : 59.5, 80, 60.5, 40);
        ctx.clip();
        ctx.translate(60, 101);
        ctx.rotate(-dir * v.twitch);
        ctx.translate(-60, -101);
        ctx.fill(p.mustache);
        ctx.restore();
      }
    }
    ctx.restore();
  }

  // A star on the rim, the glint the reel plays on hits.
  if (v.glint > 0 && v.monocle >= 0.98) {
    const g = view.project(frontPoint(b, 72, 49));
    const e = view.project(frontPoint(b, 72 + mr, 49));
    sparkle(ctx, g.x, g.y, 0.75 * Math.hypot(e.x - g.x, e.y - g.y) * v.glint, v.glint, v.glint * 0.6);
  }
}

/** The strawberry on its seat on the lid, beside the tape, standing up off the lid. */
function drawLogoBerry(ctx: CanvasRenderingContext2D, view: View, b: LogoBox, face: FaceParams): void {
  const k = face.strawberry ?? 0;
  if (k <= 0) return;
  const upDir = lidVector(b, b.up);
  const seat = add(lidPoint(b, BERRY_SEAT[0], 1, BERRY_SEAT[1]), mul(upDir, face.berryLift ?? 0));
  const p = view.project(seat);
  const q = view.project(add(seat, mul(upDir, 10)));
  const width = BERRY_WIDTH * b.f.size * b.f.dims[0] * view.cam.scale * p.f * k;
  ctx.save();
  ctx.globalAlpha *= clamp(k * 3);
  drawStrawberry(ctx, p.x, p.y, width, Math.atan2(q.x - p.x, p.y - q.y));
  ctx.restore();
}

let berryPaths: { body: Path2D; seeds: Path2D; leaves: Path2D } | null = null;

/**
 * The strawberry keepsake the mascot earns on a build that hit the cache,
 * upright in screen space: its tip at (x, y), `width` px across its
 * shoulders, turned `rot` radians about the tip.
 */
export function drawStrawberry(
  ctx: CanvasRenderingContext2D,
  x: number,
  y: number,
  width: number,
  rot = 0,
): void {
  if (width <= 0) return;
  // Drawn 24 units wide with the tip at the origin, standing up (-y).
  berryPaths ??= (() => {
    const seeds = new Path2D();
    for (const [sx, sy] of [
      [-7, -16],
      [-2, -17.5],
      [3.5, -17],
      [8, -14.5],
      [-4.5, -10.5],
      [0.5, -11],
      [5.5, -9.5],
      [-1.5, -5],
      [2.5, -5.5],
    ]) {
      seeds.moveTo(sx + 0.9, sy);
      seeds.ellipse(sx, sy, 0.9, 1.3, 0, 0, TAU);
    }
    return {
      body: new Path2D(
        "M0 0C-3.5 0-11.5-7-12-15C-12.3-20.5-8-23.5-4-23C-2-22.8-1-22.3 0-22.3C1-22.3 2-22.8 4-23C8-23.5 12.3-20.5 12-15C11.5-7 3.5 0 0 0Z",
      ),
      seeds,
      leaves: new Path2D(
        "M0-21.5L-10.5-21.5L-5-24.5L-8-29L-1.8-26L0-29.5L1.8-26L8-29L5-24.5L10.5-21.5ZM-.9-26C-.9-29 0-31.5 2-33L3-32C1.4-30.5 .9-28.5 .9-26Z",
      ),
    };
  })();
  ctx.save();
  ctx.translate(x, y);
  ctx.rotate(rot);
  const s = width / 24;
  ctx.scale(s, s);
  ctx.fillStyle = BERRY;
  ctx.fill(berryPaths.body);
  ctx.fillStyle = SEED;
  ctx.fill(berryPaths.seeds);
  ctx.fillStyle = LEAF;
  ctx.fill(berryPaths.leaves);
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
