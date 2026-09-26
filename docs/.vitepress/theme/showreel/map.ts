// The middle of the reel as one world, and every section's handoff.
//
// every-checkout through ci play on one map: Mr Boxington (the logo
// character, box.ts) with his lid raised on its hinge, the terminal panes
// that stand on their checkouts' target/ slabs, the pixel build view with
// the real terminal mascot (sprite.ts) over the real `Build / cache` bar, the
// tower of old target/ slabs, the dashed `your machine` frame, and on the CI
// side the runners and the remote cache with its backend chips. This file
// lays them out, draws them (the kit), and fixes the exact frame on every bar
// line (HANDOFFS), which the scene before ends on and the scene after starts
// from. At 120 fps every bar line is a displayed frame.
//
// Coordinates are the reel's 1920x1080 logical px at the HOME camera. Every
// actor stands on FLOOR (y 690) or above, clear of the captions' band (y
// 755-965), so a scene can leave the map up under any caption. Every
// function is a pure function of its arguments: no state between frames.

import {
  BEAT,
  drawLogoBox,
  drawNodeLabel,
  drawStagedBox,
  diveCam,
  END_CAM,
  END_POSE,
  H,
  H1_CAM,
  H1_POSE,
  H2_POSE,
  H5_POSE,
  keptDiscs,
  NODES,
  PALETTE,
  type SceneEnv,
  SECTIONS,
  type SectionId,
  sec,
  W,
  WHIP,
  WORLD_CAM,
} from "./bible";
import { type BoxPose, boxSilhouette, LOGO_FACE, LOGO_POSE, type LogoFace, logoCam, monocleScreen, mouth } from "./box";
import { mix, rgba } from "./color";
import { glow, makeCanvas, roundedRect, smear } from "./fx";
import { clamp, hash, lerp, outQuart, progress, spring, swiftOut, TAU } from "./math";
import { type Camera, polygon, View } from "./space";
import {
  cheekLevel,
  DEFAULT_POSE,
  drawSprite,
  type Gaze,
  LID_FPS,
  lidAt,
  lidOffset,
  poseAt,
  type Pose,
  SIZE as SPRITE_SIZE,
} from "./sprite";
import { DISPLAY, drawText, font, layout, MONO } from "./type";

// Geometry.

export interface Pt {
  x: number;
  y: number;
}
export interface Rect {
  x: number;
  y: number;
  w: number;
  h: number;
}

/** Everything a scene leaves up while a caption is up stays above this y. */
export const CAPTION_TOP = 740;
/** The ground line every map actor stands on. */
export const FLOOR = 690;

const lerpPt = (a: Pt, b: Pt, k: number): Pt => ({ x: lerp(a.x, b.x, k), y: lerp(a.y, b.y, k) });
/** A rect tweened toward another. */
export const lerpRect = (a: Rect, b: Rect, k: number): Rect => ({
  x: lerp(a.x, b.x, k),
  y: lerp(a.y, b.y, k),
  w: lerp(a.w, b.w, k),
  h: lerp(a.h, b.h, k),
});

// Motion shared by the map's actors, on the caller's clock (seconds).

/** An actor popping in at `at`: its scale, a spring from 0 that overshoots a little. */
export const popIn = (t: number, at: number, freq = 4.5, damping = 0.5): number => spring(t - at, freq, damping);

/** How many characters of `text` are typed at `t`, typing from `at` for `dur`. */
export const typedChars = (text: string, t: number, at: number, dur: number): number =>
  Math.floor(text.length * progress(at, at + dur, t) + 1e-9);

/** A terminal's block cursor, blinking with the beat: on for the first half of each (global `t`). */
export const cursorOn = (t: number): boolean => Math.floor(t / (BEAT / 2)) % 2 === 0;

// The map camera: a 2D pan and zoom over the map. The map point (cx, cy) is
// drawn at the frame's center, `zoom` times its HOME size. Everything on the
// map, the 3D box and the pixel sprite included, is drawn under it.

export interface MapCam {
  cx: number;
  cy: number;
  zoom: number;
}
export const HOME: Readonly<MapCam> = { cx: W / 2, cy: H / 2, zoom: 1 };

/** Apply `cam` on top of the context's transform; draw the map in map px after. */
export function applyMapCam(ctx: CanvasRenderingContext2D, cam: MapCam): void {
  ctx.translate(W / 2, H / 2);
  ctx.scale(cam.zoom, cam.zoom);
  ctx.translate(-cam.cx, -cam.cy);
}

/** Where a map point lands on the screen under `cam`. */
export function mapToScreen(cam: MapCam, p: Pt): Pt {
  return { x: W / 2 + (p.x - cam.cx) * cam.zoom, y: H / 2 + (p.y - cam.cy) * cam.zoom };
}

/** A camera move: the zoom eased in log space, so a push feels even. */
export function mixMapCam(a: MapCam, b: MapCam, k: number): MapCam {
  return { cx: lerp(a.cx, b.cx, k), cy: lerp(a.cy, b.cy, k), zoom: a.zoom * (b.zoom / a.zoom) ** k };
}

// Mr Boxington on the map: box.ts's logo character under his own
// one-point camera (logoCam), so wherever he stands he is the logo seen
// square on. A spot is his base's center and his width in map px.

export interface BoxSpot {
  /** Center of the base, map px. */
  x: number;
  /** The floor under him. */
  y: number;
  /** Box width, map px (the logo's 120 units). */
  w: number;
}

/** His camera: logo.svg's square placed so the base's center sits on the spot. */
export function boxCam(spot: BoxSpot): Camera {
  const size = (spot.w * 128) / 120;
  const k = size / 128;
  return logoCam(spot.x - 64 * k, spot.y - 124 * k, size);
}

/**
 * The pose on the map: the logo character with `extra` over it (tape 0 and
 * the logo's face unless given; null for no face). Position and size come
 * from the spot, so they are fixed here.
 */
export function boxPose(extra: Partial<BoxPose> = {}): BoxPose {
  return {
    ...LOGO_POSE,
    tape: 0,
    ...extra,
    face: extra.face === undefined ? LOGO_FACE : extra.face,
    kind: "logo",
    pos: [0, 0, 0],
    size: 1,
  };
}

/** A spot tweened toward another. */
export const lerpSpot = (a: BoxSpot, b: BoxSpot, k: number): BoxSpot => ({
  x: lerp(a.x, b.x, k),
  y: lerp(a.y, b.y, k),
  w: lerp(a.w, b.w, k),
});

/** The face before any cache hit: the logo's, without the blush. */
export const IDLE_FACE: Readonly<LogoFace> = { ...LOGO_FACE, cheeks: 0 };
/** A cold build's finish: matte, no blush, no strawberry. Same as IDLE_FACE. */
export const COLD_FACE: Readonly<LogoFace> = IDLE_FACE;
/** A build that hit the cache, finished: rose cheeks and the strawberry. */
export const WARM_FACE: Readonly<LogoFace> = { ...LOGO_FACE, cheeks: 3, strawberry: 1 };

/** Where a carton aims to drop into him: the middle of his open mouth, map px. */
export function boxMouth(spot: BoxSpot, pose: BoxPose): Pt {
  const m = mouth(new View(boxCam(spot)), pose);
  return { x: m.x, y: m.y };
}

/** His monocle's center and radius, map px. */
export function boxMonocle(spot: BoxSpot, pose: BoxPose): { x: number; y: number; r: number } {
  return monocleScreen(new View(boxCam(spot)), pose);
}

// The pixel build view: the terminal's `Build / cache` bar and its mascot.

/** The bar's width in cells (view.rs BAR_WIDTH). */
export const BAR_CELLS = 28;

/** The cache counters the bar is drawn from (model.rs CacheMix). */
export interface CacheMix {
  hits: number;
  misses: number;
  bypasses: number;
  /** "not looked up". */
  unconsulted: number;
}
export const NO_MIX: Readonly<CacheMix> = { hits: 0, misses: 0, bypasses: 0, unconsulted: 0 };

/**
 * model.rs segments(): green hits, then amber misses, then grey for
 * bypassed and not looked up, as whole cells that always add up to `width`.
 * No counts yet is all grey.
 */
export function segments(mix: CacheMix, width: number): [number, number, number] {
  const total = mix.hits + mix.misses + mix.bypasses + mix.unconsulted;
  if (total === 0) return [0, 0, width];
  const hitEnd = Math.floor((mix.hits * width) / total);
  const missEnd = Math.floor(((mix.hits + mix.misses) * width) / total);
  return [hitEnd, missEnd - hitEnd, width - missEnd];
}

/** What the pixel build view shows. */
export interface BuildView {
  /** The mascot (sprite.ts). */
  pose: Pose;
  /** Filled cells, 0 to BAR_CELLS: units done, or the whole bar once built. */
  filled: number;
  mix: CacheMix;
  /** The line beside the mascot: `Compiling syn`, `✓ Built`. */
  status?: { text: string; tone: "dim" | "text" | "ok" } | null;
  /** A second line under it, e.g. `354 hits`. */
  counts?: string | null;
}

/** One unit of a build finishing: when, and how the cache answered. */
export interface Unit {
  /** Global seconds. */
  at: number;
  outcome: "hit" | "miss" | "other";
}

/**
 * A build for the pixel view, planned in reel time. Units land at their own
 * times; from `finish` the build is over and `ok`.
 */
export interface BuildPlan {
  /** Global seconds the build starts: the mascot's clock and its lid start here. */
  start: number;
  /** Cargo's units expected. */
  total: number;
  units: readonly Unit[];
  /** Global seconds the finished frame first shows; null while it runs. */
  finish?: number | null;
  /** How it ends (default true). */
  ok?: boolean;
  /** Pin the mascot's gaze (the real one wanders on a 7 s loop). */
  gaze?: Gaze;
  /** Suppress the mascot's blinks (they fall on a hash of the clock). */
  noBlink?: boolean;
}

/**
 * The pixel build view at global time `t`, by mascot.rs's rules: the lid
 * steps down toward lidOffset one pixel per 1/LID_FPS s (lidAt), the lens
 * glints after hits (glintAt), the cheeks follow the hit share, and a
 * finished build is taped, with a strawberry if it hit. The bar fills with
 * the units done, split by segments().
 */
export function buildAt(plan: BuildPlan, t: number): BuildView {
  const mixAt = (time: number): CacheMix & { done: number; lastHit: number | null } => {
    const m = { ...NO_MIX, done: 0, lastHit: null as number | null };
    for (const u of plan.units) {
      if (u.at > time) continue;
      m.done++;
      if (u.outcome === "hit") {
        m.hits++;
        m.lastHit = m.lastHit === null ? u.at : Math.max(m.lastHit, u.at);
      } else if (u.outcome === "miss") m.misses++;
      else m.unconsulted++;
    }
    return m;
  };
  const m = mixAt(t);
  const finished = plan.finish != null && t >= plan.finish;
  const ms = Math.max(0, Math.floor((t - plan.start) * 1000));
  const frame = Math.max(0, Math.floor((t - plan.start) * LID_FPS));
  const lidShown = lidAt(frame, (f) => lidOffset(mixAt(plan.start + f / LID_FPS).done, plan.total));
  let pose = poseAt({
    ms,
    sinceHitMs: m.lastHit === null ? null : Math.max(0, Math.floor((t - m.lastHit) * 1000)),
    done: m.done,
    total: plan.total,
    lidShown,
    hits: m.hits,
    misses: m.misses,
    testing: false,
    ok: finished ? (plan.ok ?? true) : null,
  });
  if (!finished) {
    if (plan.gaze) pose = { ...pose, gaze: plan.gaze };
    if (plan.noBlink && pose.eye === "shut") pose = { ...pose, eye: "skeptic" };
  }
  const filled = finished && (plan.ok ?? true) ? BAR_CELLS : Math.floor((BAR_CELLS * Math.min(m.done, plan.total)) / plan.total);
  return { pose, filled, mix: { hits: m.hits, misses: m.misses, bypasses: m.bypasses, unconsulted: m.unconsulted } };
}

/**
 * Where the big box's pupils look for the mascot's gaze, logo units. A gaze
 * names what he watches, not a way to look: in the terminal the Compiling
 * list and the bar stand to the mascot's left (view.rs), so the pixel mascot
 * looks left and down and left, but on the map Cargo's plan and the pane
 * stand to the big box's right, so he looks right and down and right.
 */
const LOOK: Readonly<Record<Gaze, [number, number]>> = { list: [3, -1], bar: [2, 3], you: [0, 0] };

/**
 * The big box mirroring the pixel mascot: the same lid, hinged at its left
 * end and raised at its right by the sprite's steps (box.ts `lid`), the
 * tape, blush, strawberry and glint (the sprite's band p is sweep (p + 0.5) /
 * 4), the squint and the blink.
 */
export function boxFromSprite(pose: Pose, face: Partial<LogoFace> = {}): BoxPose {
  return boxPose({
    lid: pose.taped ? 0 : pose.lid,
    tape: pose.taped ? 1 : 0,
    face: {
      ...LOGO_FACE,
      cheeks: pose.cheeks,
      strawberry: pose.strawberry ? 1 : 0,
      sweep: pose.glint === null ? null : (pose.glint + 0.5) / 4,
      eyelid: pose.eye === "squint" ? 1 : 0,
      blink: pose.eye === "shut" ? 1 : 0,
      look: LOOK[pose.gaze],
      ...face,
    },
  });
}

/** The running mascot at a build's start: lid up, no blush, eyes on the list. */
export const RUNNING_POSE: Readonly<Pose> = { ...DEFAULT_POSE, lid: 4, gaze: "list" };
/** A finished build's mascot (poseAt with ok): taped, blush by the hit share, a strawberry if it hit. */
export function doneSprite(hits: number, misses: number): Pose {
  return { ...DEFAULT_POSE, taped: true, cheeks: cheekLevel(hits, misses), strawberry: hits > 0 };
}

// Paint.

/** The kit's colours beside bible.ts PALETTE. */
export const KIT = {
  /** A terminal's body and its title bar. */
  window: "#1a1713",
  chrome: "#25211a",
  edge: PALETTE.divider,
  /** A checkout's target/ slab: tired cardboard, not the amber of fresh output. */
  slabTop: "#7b6645",
  slabFront: "#5e4c33",
  slabBand: "#4b3d2a",
  /** The frame's dashes. */
  dash: "rgba(245,234,214,0.38)",
} as const;

/** The terminal's own colours (norimel's palette) that the pixel pane keeps. */
export const TERM = {
  green: "#a6e3a1",
  yellow: "#f9e2af",
  subtext0: "#a6adc8",
  surface1: "#45475a",
  text: "#cdd6f4",
} as const;

/**
 * The `Build / cache` bar's paint: the terminal's green, grey and light
 * shade, and the reel's amber for misses. The terminal's pale yellow reads
 * as beige beside the green once the frame is small, and the storyboard's
 * one missed crate has to read as amber, the colour of compiling.
 */
export const BAR = {
  hit: TERM.green,
  miss: PALETTE.amber,
  other: TERM.subtext0,
  empty: TERM.surface1,
} as const;

/** A carton's paint: amber compiled by rustc, green restored, tan stored in a cache. */
export const CARTON = {
  compiled: { top: "#f2c479", front: "#e6ad54", band: "#cf8f35", tape: "#f7e4b8" },
  restored: { top: "#d4ebc8", front: "#a1ca92", band: "#7fae70", tape: "#eef7e9" },
  stored: { top: "#a18a66", front: "#85704f", band: "#6d5c41", tape: "#bba684" },
} as const;
export type CartonKind = keyof typeof CARTON;

// Kit: text.

export interface LabelState {
  text: string;
  x: number;
  /** Baseline. */
  y: number;
  /** px; 56 for labels a phone viewer needs, 40 for desktop detail. */
  size?: number;
  mono?: boolean;
  align?: "left" | "center" | "right";
  fill?: string;
  alpha?: number;
}

export function drawLabel(ctx: CanvasRenderingContext2D, l: LabelState): number {
  const a = l.alpha ?? 1;
  if (a <= 0) return 0;
  const size = l.size ?? 56;
  ctx.save();
  ctx.globalAlpha *= a;
  const w = drawText(ctx, l.text, l.x, l.y, {
    font: font(size, l.mono ? 500 : 600, l.mono ? MONO : DISPLAY),
    tracking: l.mono ? 0 : -size / 50,
    align: l.align ?? "left",
    fill: l.fill ?? PALETTE.paper,
  }).width;
  ctx.restore();
  return w;
}

// Kit: chips and tags.

export type Tone = "neutral" | "amber" | "green" | "dim";
const TONES: Record<Tone, { fill: string; edge: string; text: string }> = {
  neutral: { fill: rgba(PALETTE.surface, 0.96), edge: PALETTE.divider, text: PALETTE.text1 },
  amber: { fill: mix(PALETTE.surface, PALETTE.amber, 0.16), edge: PALETTE.amber, text: PALETTE.amberBright },
  green: { fill: mix(PALETTE.surface, PALETTE.green, 0.16), edge: PALETTE.green, text: PALETTE.green },
  dim: { fill: rgba(PALETTE.surface, 0.7), edge: rgba(PALETTE.divider, 0.8), text: PALETTE.text3 },
};

export interface ChipState {
  text: string;
  /** Top left. */
  x: number;
  y: number;
  /** Fixed width; the text's own width plus padding without one. */
  w?: number;
  /** Text px (40 desktop detail, 56 a phone label); the chip is 1.4 times as tall. */
  size?: number;
  mono?: boolean;
  tone?: Tone;
  alpha?: number;
  /** 0..1 a halo in the tone's colour. */
  lit?: number;
  /** Pop scale about the chip's center (springs). */
  scale?: number;
}

/** A chip's height for its text size. */
export const chipHeight = (size = 40): number => Math.round(size * 1.4);

/** Measure a chip without drawing it. */
export function chipRect(ctx: CanvasRenderingContext2D, c: ChipState): Rect {
  const size = c.size ?? 40;
  const mono = c.mono ?? true;
  const tw = layout(ctx, c.text, font(size, mono ? 500 : 600, mono ? MONO : DISPLAY)).width;
  return { x: c.x, y: c.y, w: c.w ?? Math.round(tw + size * 1.1), h: chipHeight(size) };
}

/** A rounded chip: a crate, a backend, a path. Returns its rect. */
export function drawChip(ctx: CanvasRenderingContext2D, c: ChipState): Rect {
  const r = chipRect(ctx, c);
  const a = c.alpha ?? 1;
  const s = c.scale ?? 1;
  if (a <= 0 || s <= 0) return r;
  const size = c.size ?? 40;
  const mono = c.mono ?? true;
  const tone = TONES[c.tone ?? "neutral"];
  ctx.save();
  ctx.globalAlpha *= a;
  ctx.translate(r.x + r.w / 2, r.y + r.h / 2);
  ctx.scale(s, s);
  if ((c.lit ?? 0) > 0) glow(ctx, 0, 0, r.w * 0.7, tone.edge, 0.35 * (c.lit ?? 0));
  roundedRect(ctx, -r.w / 2, -r.h / 2, r.w, r.h, r.h / 2);
  ctx.fillStyle = tone.fill;
  ctx.fill();
  ctx.lineWidth = 2.5;
  ctx.strokeStyle = tone.edge;
  ctx.stroke();
  drawText(ctx, c.text, -r.w / 2 + size * 0.55, size * 0.35, {
    font: font(size, mono ? 500 : 600, mono ? MONO : DISPLAY),
    fill: tone.text,
  });
  ctx.restore();
  return r;
}

export interface TagOptions {
  size?: number;
  mono?: boolean;
  /** paper: ink on a paper tag (the default); amber and green: coloured card. */
  tone?: "paper" | "amber" | "green";
  /** Turn about the point, radians. */
  rot?: number;
  alpha?: number;
  /** Where its string runs to, if anywhere. */
  string?: Pt;
}

/**
 * A key or label tag, a luggage tag hung from its point at (x, y) and
 * running right: `key 9e1f…`, `hk (edited)`. Returns its width.
 */
export function drawTag(ctx: CanvasRenderingContext2D, x: number, y: number, text: string, o: TagOptions = {}): number {
  const size = o.size ?? 40;
  const mono = o.mono ?? true;
  const spec = font(size, mono ? 600 : 600, mono ? MONO : DISPLAY);
  const tw = layout(ctx, text, spec).width;
  const h = Math.round(size * 1.5);
  const nose = h * 0.42;
  const w = tw + size * 0.9 + nose;
  const a = o.alpha ?? 1;
  if (a <= 0) return w;
  const paint =
    o.tone === "amber"
      ? { fill: PALETTE.amber, text: PALETTE.ink, hole: PALETTE.amberShade }
      : o.tone === "green"
        ? { fill: PALETTE.green, text: PALETTE.ink, hole: "#6f9a62" }
        : { fill: PALETTE.paper, text: PALETTE.ink, hole: PALETTE.text3 };
  ctx.save();
  ctx.globalAlpha *= a;
  if (o.string) {
    ctx.strokeStyle = rgba(PALETTE.paper, 0.6);
    ctx.lineWidth = 2;
    ctx.beginPath();
    ctx.moveTo(x + nose * 0.6, y);
    ctx.quadraticCurveTo((x + o.string.x) / 2, Math.max(y, o.string.y) + 30, o.string.x, o.string.y);
    ctx.stroke();
  }
  ctx.translate(x, y);
  if (o.rot) ctx.rotate(o.rot);
  const r = 6;
  ctx.beginPath();
  ctx.moveTo(0, 0);
  ctx.lineTo(nose, -h / 2);
  ctx.lineTo(w - r, -h / 2);
  ctx.arcTo(w, -h / 2, w, -h / 2 + r, r);
  ctx.lineTo(w, h / 2 - r);
  ctx.arcTo(w, h / 2, w - r, h / 2, r);
  ctx.lineTo(nose, h / 2);
  ctx.closePath();
  ctx.fillStyle = paint.fill;
  ctx.fill();
  ctx.fillStyle = paint.hole;
  ctx.beginPath();
  ctx.arc(nose * 0.62, 0, h * 0.09, 0, TAU);
  ctx.fill();
  drawText(ctx, text, nose + size * 0.4, size * 0.35, { font: spec, fill: paint.text });
  ctx.restore();
  return w;
}

// Kit: cartons, a crate's compiled output.

export interface CartonOptions {
  /** Turn about its center, radians. */
  rot?: number;
  /** Squash and stretch about its base. */
  sx?: number;
  sy?: number;
  alpha?: number;
  /** A crate name: on its front when it is big enough, else under it. */
  label?: string;
  /** 0..1 a halo, for a landing or a hit. */
  lit?: number;
}

/**
 * A carton standing with its base's center at (x, y), `size` px wide: a
 * small box in the logo's flat, front-facing drawing (front, lid top in
 * one-point perspective, lid band, a tape tab). Amber is compiled by rustc,
 * green restored from the cache, tan stored.
 */
export function drawCarton(
  ctx: CanvasRenderingContext2D,
  x: number,
  y: number,
  size: number,
  kind: CartonKind,
  o: CartonOptions = {},
): void {
  const a = o.alpha ?? 1;
  if (a <= 0 || size <= 0) return;
  const p = CARTON[kind];
  const w = size;
  const fh = size * 0.78;
  const td = size * 0.14;
  const inset = size * 0.08;
  ctx.save();
  ctx.globalAlpha *= a;
  ctx.translate(x, y);
  if (o.sx !== undefined || o.sy !== undefined) ctx.scale(o.sx ?? 1, o.sy ?? 1);
  if (o.rot) {
    ctx.translate(0, -fh / 2);
    ctx.rotate(o.rot);
    ctx.translate(0, fh / 2);
  }
  if ((o.lit ?? 0) > 0) glow(ctx, 0, -fh / 2, size * 1.3, p.front, 0.5 * (o.lit ?? 0));
  // The lid's top, narrower at the back.
  ctx.fillStyle = p.top;
  ctx.beginPath();
  ctx.moveTo(-w / 2 + inset, -fh - td);
  ctx.lineTo(w / 2 - inset, -fh - td);
  ctx.lineTo(w / 2, -fh);
  ctx.lineTo(-w / 2, -fh);
  ctx.closePath();
  ctx.fill();
  // The front, with the logo's rounded base corners, its lid band and base band.
  roundedBottom(ctx, -w / 2, -fh, w, fh, size * 0.03);
  ctx.fillStyle = p.front;
  ctx.fill();
  ctx.fillStyle = p.band;
  ctx.fillRect(-w / 2, -fh, w, size * 0.05);
  ctx.fillRect(-w / 2, -size * 0.05, w, size * 0.05);
  // The tape tab over the lid and down the front.
  const tw = size * 0.14;
  ctx.fillStyle = p.tape;
  ctx.beginPath();
  ctx.moveTo(-tw / 2 + 1, -fh - td);
  ctx.lineTo(tw / 2 - 1, -fh - td);
  ctx.lineTo(tw / 2, -fh);
  ctx.lineTo(-tw / 2, -fh);
  ctx.closePath();
  ctx.fill();
  ctx.fillRect(-tw / 2, -fh, tw, size * 0.13);
  if (o.label) {
    if (size >= 110) {
      drawText(ctx, o.label, 0, -fh * 0.34, { font: font(Math.round(size * 0.2), 600, MONO), fill: PALETTE.ink, align: "center" });
    } else {
      drawText(ctx, o.label, 0, 40, { font: font(32, 500, MONO), fill: PALETTE.text2, align: "center" });
    }
  }
  ctx.restore();
}

/** A rect with only its bottom corners rounded, as a path. */
function roundedBottom(ctx: CanvasRenderingContext2D, x: number, y: number, w: number, h: number, r: number): void {
  ctx.beginPath();
  ctx.moveTo(x, y);
  ctx.lineTo(x + w, y);
  ctx.lineTo(x + w, y + h - r);
  ctx.arcTo(x + w, y + h, x + w - r, y + h, r);
  ctx.lineTo(x + r, y + h);
  ctx.arcTo(x, y + h, x, y + h - r, r);
  ctx.closePath();
}

// Kit: arrows, sparks, and thread.

/** A quadratic Bézier: a thrown arc or a gentle bend. */
export interface Curve {
  a: Pt;
  c: Pt;
  b: Pt;
}

/** An arc from a to b bulging `lift` of its length to the left of travel (up for a rightward throw). */
export function arc(a: Pt, b: Pt, lift = 0.25): Curve {
  const dx = b.x - a.x;
  const dy = b.y - a.y;
  const flip = dx < 0 ? -1 : 1;
  return { a, b, c: { x: (a.x + b.x) / 2 + dy * lift * flip, y: (a.y + b.y) / 2 - Math.abs(dx) * lift } };
}

export function curveAt(k: Curve, u: number): Pt {
  const v = 1 - u;
  return {
    x: v * v * k.a.x + 2 * v * u * k.c.x + u * u * k.b.x,
    y: v * v * k.a.y + 2 * v * u * k.c.y + u * u * k.b.y,
  };
}

/** The direction of travel at u, not normalized. */
export function curveTangent(k: Curve, u: number): Pt {
  return {
    x: 2 * (1 - u) * (k.c.x - k.a.x) + 2 * u * (k.b.x - k.c.x),
    y: 2 * (1 - u) * (k.c.y - k.a.y) + 2 * u * (k.b.y - k.c.y),
  };
}

export interface ArrowOptions {
  /** The drawn stretch of the curve, 0..1; the head rides `to`. */
  from?: number;
  to?: number;
  width?: number;
  color?: string;
  /** Head length px; 0 for none. */
  head?: number;
  alpha?: number;
  /** 0..1 a glow under the stroke. */
  glow?: number;
}

/**
 * An arrow along a curve, drawn on from `from` to `to`: a `rustc` call from
 * Cargo's plan to the box, an upload to the remote.
 */
export function drawArrow(ctx: CanvasRenderingContext2D, k: Curve, o: ArrowOptions = {}): void {
  const u0 = clamp(o.from ?? 0);
  const u1 = clamp(o.to ?? 1);
  const a = o.alpha ?? 1;
  if (u1 <= u0 || a <= 0) return;
  const width = o.width ?? 4;
  const color = o.color ?? PALETTE.paper;
  const head = o.head ?? width * 4.5;
  const n = 24;
  const end = curveAt(k, u1);
  const tan = curveTangent(k, u1);
  const tl = Math.hypot(tan.x, tan.y) || 1;
  const dir = { x: tan.x / tl, y: tan.y / tl };
  ctx.save();
  ctx.globalAlpha *= a;
  if ((o.glow ?? 0) > 0) glow(ctx, end.x, end.y, width * 10, color, 0.6 * (o.glow ?? 0));
  ctx.strokeStyle = color;
  ctx.lineWidth = width;
  ctx.lineCap = "round";
  ctx.lineJoin = "round";
  ctx.beginPath();
  for (let i = 0; i <= n; i++) {
    const p = curveAt(k, lerp(u0, u1, i / n));
    // Stop the shaft under the head so the tip stays sharp.
    const q = i === n && head > 0 ? { x: p.x - dir.x * head * 0.6, y: p.y - dir.y * head * 0.6 } : p;
    if (i === 0) ctx.moveTo(q.x, q.y);
    else ctx.lineTo(q.x, q.y);
  }
  ctx.stroke();
  if (head > 0) {
    const nx = -dir.y;
    const ny = dir.x;
    ctx.fillStyle = color;
    ctx.beginPath();
    ctx.moveTo(end.x, end.y);
    ctx.lineTo(end.x - dir.x * head + nx * head * 0.55, end.y - dir.y * head + ny * head * 0.55);
    ctx.lineTo(end.x - dir.x * head * 0.7, end.y - dir.y * head * 0.7);
    ctx.lineTo(end.x - dir.x * head - nx * head * 0.55, end.y - dir.y * head - ny * head * 0.55);
    ctx.closePath();
    ctx.fill();
  }
  ctx.restore();
}

export interface SparkOptions {
  color?: string;
  /** Head radius px. */
  size?: number;
  /** Tail length along the curve, in u. */
  trail?: number;
  alpha?: number;
}

/** A spark travelling a curve, its head at u with a fading tail: a permit request, a hit. */
export function drawSpark(ctx: CanvasRenderingContext2D, k: Curve, u: number, o: SparkOptions = {}): void {
  const a = o.alpha ?? 1;
  if (a <= 0 || u < 0 || u > 1) return;
  const color = o.color ?? PALETTE.amberBright;
  const size = o.size ?? 7;
  const trail = o.trail ?? 0.18;
  const head = curveAt(k, u);
  ctx.save();
  ctx.globalAlpha *= a;
  glow(ctx, head.x, head.y, size * 6, color, 0.7);
  const n = 10;
  ctx.lineCap = "round";
  for (let i = n; i >= 1; i--) {
    const p0 = curveAt(k, Math.max(0, u - (trail * i) / n));
    const p1 = curveAt(k, Math.max(0, u - (trail * (i - 1)) / n));
    ctx.strokeStyle = rgba(color, 0.9 * (1 - i / (n + 1)));
    ctx.lineWidth = size * 1.6 * (1 - i / (n + 1));
    ctx.beginPath();
    ctx.moveTo(p0.x, p0.y);
    ctx.lineTo(p1.x, p1.y);
    ctx.stroke();
  }
  ctx.fillStyle = mix(color, "#ffffff", 0.6);
  ctx.beginPath();
  ctx.arc(head.x, head.y, size, 0, TAU);
  ctx.fill();
  ctx.restore();
}

export interface ThreadOptions {
  color?: string;
  width?: number;
  alpha?: number;
}

/**
 * A thread stitched through `pts` in order, drawn on to `p` (0..1 of its
 * length): a running stitch with a knot at each point it has reached. It
 * ties rows that repeat, e.g. every card's `Compiling syn`.
 */
export function drawThread(ctx: CanvasRenderingContext2D, pts: readonly Pt[], p: number, o: ThreadOptions = {}): void {
  const a = o.alpha ?? 1;
  if (a <= 0 || p <= 0 || pts.length < 2) return;
  const color = o.color ?? PALETTE.amber;
  const width = o.width ?? 5;
  // A gentle sag between points, sampled into a polyline with its lengths.
  const path: Pt[] = [];
  for (let i = 0; i < pts.length - 1; i++) {
    const s = pts[i];
    const e = pts[i + 1];
    const c = { x: (s.x + e.x) / 2, y: (s.y + e.y) / 2 + Math.hypot(e.x - s.x, e.y - s.y) * 0.08 };
    for (let j = i === 0 ? 0 : 1; j <= 16; j++) path.push(curveAt({ a: s, c, b: e }, j / 16));
  }
  const acc = [0];
  for (let i = 1; i < path.length; i++) acc.push(acc[i - 1] + Math.hypot(path[i].x - path[i - 1].x, path[i].y - path[i - 1].y));
  const L = acc[acc.length - 1] * clamp(p);
  ctx.save();
  ctx.globalAlpha *= a;
  ctx.strokeStyle = color;
  ctx.lineWidth = width;
  ctx.lineCap = "round";
  ctx.setLineDash([width * 3.4, width * 2]);
  ctx.beginPath();
  ctx.moveTo(path[0].x, path[0].y);
  let tip = path[0];
  for (let i = 1; i < path.length; i++) {
    if (acc[i] >= L) {
      const f = (L - acc[i - 1]) / (acc[i] - acc[i - 1] || 1);
      tip = lerpPt(path[i - 1], path[i], f);
      ctx.lineTo(tip.x, tip.y);
      break;
    }
    tip = path[i];
    ctx.lineTo(tip.x, tip.y);
  }
  ctx.stroke();
  ctx.setLineDash([]);
  // Knots where the thread has passed through a row.
  pts.forEach((q, i) => {
    const at = i === 0 ? 0 : acc[i * 16];
    if (at > L + 1e-6) return;
    ctx.fillStyle = color;
    ctx.beginPath();
    ctx.arc(q.x, q.y, width * 1.5, 0, TAU);
    ctx.fill();
  });
  if (p < 1) glow(ctx, tip.x, tip.y, width * 9, color, 0.7);
  ctx.restore();
}

// Kit: terminal panes, which stand on their checkouts' target/ slabs.

/** A slab: a 12 px top face and a 50 px front. */
export const SLAB_H = 62;
const SLAB_TOP = 12;
/** A window's title bar. */
export const TITLE_H = 64;

export interface SlabOptions {
  alpha?: number;
  /** Turn about its center, radians. */
  rot?: number;
  /** Show the `target/` tag on its right (default true). */
  tag?: boolean;
  /** 0..1 a warm light on it, for a slab being picked. */
  lit?: number;
}

/**
 * A checkout's target/ directory as a slab of tired cardboard, its top left
 * at (x, y), `w` wide and SLAB_H tall: the name at 40 px, `target/` at the
 * right.
 */
export function drawSlab(ctx: CanvasRenderingContext2D, x: number, y: number, w: number, name: string, o: SlabOptions = {}): void {
  const a = o.alpha ?? 1;
  if (a <= 0) return;
  ctx.save();
  ctx.globalAlpha *= a;
  if (o.rot) {
    ctx.translate(x + w / 2, y + SLAB_H / 2);
    ctx.rotate(o.rot);
    ctx.translate(-x - w / 2, -y - SLAB_H / 2);
  }
  const inset = 10;
  const lit = o.lit ?? 0;
  ctx.fillStyle = mix(KIT.slabTop, PALETTE.amberBright, 0.35 * lit);
  ctx.beginPath();
  ctx.moveTo(x + inset, y);
  ctx.lineTo(x + w - inset, y);
  ctx.lineTo(x + w, y + SLAB_TOP);
  ctx.lineTo(x, y + SLAB_TOP);
  ctx.closePath();
  ctx.fill();
  roundedBottom(ctx, x, y + SLAB_TOP, w, SLAB_H - SLAB_TOP, 4);
  ctx.fillStyle = mix(KIT.slabFront, PALETTE.amber, 0.3 * lit);
  ctx.fill();
  ctx.fillStyle = KIT.slabBand;
  ctx.fillRect(x, y + SLAB_H - 6, w, 6);
  drawText(ctx, name, x + 20, y + SLAB_TOP + 37, { font: font(40, 600), fill: PALETTE.paper, tracking: -0.6 });
  if (o.tag ?? true) {
    drawText(ctx, "target/", x + w - 18, y + SLAB_TOP + 33, { font: font(26, 500, MONO), fill: rgba(PALETTE.paper, 0.55), align: "right" });
  }
  ctx.restore();
}

export interface TowerState {
  /** Left edge and floor of the bottom slab. */
  x: number;
  base: number;
  w: number;
  /** Slab names from the bottom up. */
  names: readonly string[];
  /** How many slabs stand, from the bottom; a fraction drops the next one in from above. */
  count?: number;
  alpha?: number;
  /** How far each slab drifts right of the one below, px: the lean. */
  lean?: number;
}

/**
 * Each slab's top left and turn in a tower, bottom up: the bottom one flat
 * on the floor, the rest leaning and jittered by a fixed amount each.
 */
export function towerSlabs(t: TowerState): { x: number; y: number; rot: number }[] {
  const lean = t.lean ?? 5;
  return t.names.map((_, i) => ({
    x: t.x + lean * i + (i ? (hash(i, 811) - 0.5) * 14 : 0),
    y: t.base - SLAB_H - i * (SLAB_H - SLAB_TOP),
    rot: i ? (hash(i, 823) - 0.5) * 0.035 + i * 0.004 : 0,
  }));
}

/** The leaning tower of old target/ directories. */
export function drawTower(ctx: CanvasRenderingContext2D, t: TowerState): void {
  const a = t.alpha ?? 1;
  if (a <= 0) return;
  const count = t.count ?? t.names.length;
  const slabs = towerSlabs(t);
  slabs.forEach((s, i) => {
    const k = count - i;
    if (k <= 0) return;
    // The one coming in falls the last 80 px onto the stack.
    const fall = k >= 1 ? 0 : 80 * (1 - k) ** 2;
    drawSlab(ctx, s.x, s.y - fall, t.w, t.names[i], { alpha: a * clamp(k * 3), rot: s.rot });
  });
}

export type Badge = "time-lapse" | "illustration" | "benchmark";
const BADGE_TONE: Record<Badge, string> = {
  "time-lapse": PALETTE.amber,
  illustration: PALETTE.tealLight,
  benchmark: PALETTE.green,
};

/** A terminal line under the prompt, in mono at the pane's text size. */
export interface TermLine {
  text: string;
  /** compile: `Compiling` in amber then the crate; ok: green; dim: muted. */
  tone?: "compile" | "ok" | "dim" | "text";
  alpha?: number;
  /**
   * Its line slot under the prompt (default its index), fractional while a
   * log scrolls. Lines are clipped under the prompt, and one scrolled above
   * slot 0 fades as it goes.
   */
  slot?: number;
  /** Still rising into its slot, px. */
  dy?: number;
}

export interface Prompt {
  /** The command after `$ `. */
  text: string;
  /** Characters typed so far (fractions round down); all of it when omitted. */
  shown?: number;
  /** Show the block cursor (the caller blinks it). */
  cursor?: boolean;
}

export interface PaneState {
  /** The whole pane, window and slab: its bottom stands on FLOOR. */
  rect: Rect;
  /** The window's title, 40 px mono: `~/src/hk`, `runner`. */
  title?: string;
  badge?: Badge | null;
  /** The checkout's slab under the window; null for a pane with none (a CI runner). */
  slab?: string | null;
  /** 0 folded down into its slab, 1 standing open. */
  open?: number;
  alpha?: number;
  /** 0..1 dimmed back. */
  dim?: number;
  prompt?: Prompt | null;
  lines?: readonly TermLine[];
  view?: BuildView | null;
  /** The prompt's and lines' mono px (default 56). */
  text?: number;
  /** 0..1 a halo in `glowColor` (default amber) behind the window. */
  glow?: number;
  glowColor?: string;
}

/** Where things are inside a pane, map px. */
export interface PaneLayout {
  window: Rect;
  body: Rect;
  slab: Rect | null;
  /** The prompt's baseline start, and the first line's (the rest step by `lineStep`). */
  prompt: Pt;
  line: Pt;
  lineStep: number;
  /** The build view: the mascot, the text beside it, the bar. */
  sprite: Rect;
  /** Sprite pixel size. */
  px: number;
  status: Pt;
  counts: Pt;
  barLabel: Pt;
  bar: Rect;
}

/**
 * A pane's layout. The build view is laid out for PANE's 800 px and scales
 * with the window's width. As view.rs draws it, the mascot stands at the
 * right and its status and counts read at the left; the bar runs the width
 * of the pane under both.
 */
export function paneLayout(p: PaneState): PaneLayout {
  const { x, y, w, h } = p.rect;
  const hasSlab = p.slab != null;
  const inset = hasSlab ? 10 : 0;
  const wh = hasSlab ? h - SLAB_H + 6 : h;
  const window = { x: x + inset, y, w: w - 2 * inset, h: wh };
  const body = { x: window.x, y: y + TITLE_H, w: window.w, h: wh - TITLE_H };
  const text = p.text ?? 56;
  const s = window.w / 780;
  const pad = 32 * s;
  const prompt = { x: body.x + pad, y: body.y + text * 1.36 };
  const px = Math.max(2, Math.round(12 * s));
  const side = SPRITE_SIZE * px;
  const sprite = { x: body.x + body.w - pad - side, y: body.y + 108 * s, w: side, h: side };
  const barLabel = { x: body.x + pad, y: sprite.y + sprite.h + 50 * s };
  return {
    window,
    body,
    slab: hasSlab ? { x, y: y + h - SLAB_H, w, h: SLAB_H } : null,
    prompt,
    line: { x: prompt.x, y: prompt.y + text * 1.36 },
    lineStep: text * 1.36,
    sprite,
    px,
    status: { x: body.x + pad, y: sprite.y + 64 * s },
    counts: { x: body.x + pad, y: sprite.y + 124 * s },
    barLabel,
    bar: { x: body.x + pad, y: barLabel.y + 16 * s, w: body.w - 2 * pad, h: 44 * s },
  };
}

function drawBadge(ctx: CanvasRenderingContext2D, right: number, cy: number, badge: Badge): void {
  const spec = font(30, 600);
  const tw = layout(ctx, badge, spec).width;
  const w = tw + 36;
  const h = 44;
  const color = BADGE_TONE[badge];
  roundedRect(ctx, right - w, cy - h / 2, w, h, h / 2);
  ctx.fillStyle = mix(KIT.chrome, color, 0.14);
  ctx.fill();
  ctx.lineWidth = 2;
  ctx.strokeStyle = rgba(color, 0.85);
  ctx.stroke();
  drawText(ctx, badge, right - w / 2, cy + 10, { font: spec, fill: color, align: "center" });
}

/** The real bar: the filled cells split by segments(), the rest in light shade. */
export function drawCacheBar(ctx: CanvasRenderingContext2D, r: Rect, filled: number, mixed: CacheMix): void {
  const n = clamp(Math.floor(filled), 0, BAR_CELLS);
  const [hits, misses, rest] = segments(mixed, n);
  const cell = r.w / BAR_CELLS;
  const run = (from: number, cells: number, color: string) => {
    if (cells <= 0) return;
    ctx.fillStyle = color;
    ctx.fillRect(r.x + from * cell, r.y, cells * cell, r.h);
  };
  // ░ in the terminal: the empty cells, a dim shade with a fine grain of dots.
  run(n, BAR_CELLS - n, rgba(BAR.empty, 0.55));
  if (n < BAR_CELLS) {
    ctx.fillStyle = rgba(BAR.empty, 0.9);
    const dot = Math.max(2, Math.round(cell / 6));
    for (let i = n; i < BAR_CELLS; i++) {
      for (let j = 0; j < 3; j++) {
        for (let k = 0; k < 4; k++) {
          ctx.fillRect(r.x + i * cell + (k + (j % 2) * 0.5) * (cell / 4), r.y + (j + 0.3) * (r.h / 3), dot, dot);
        }
      }
    }
  }
  run(0, hits, BAR.hit);
  run(hits, misses, BAR.miss);
  run(hits + misses, rest, BAR.other);
}

/**
 * The pixel build view inside a pane's body: the mascot, its status, the
 * bar. drawPane draws it from `view`; a scene can draw it over the pane
 * itself, at the layout drawPane returns.
 */
export function drawBuildView(ctx: CanvasRenderingContext2D, L: PaneLayout, v: BuildView): void {
  const s = L.window.w / 780;
  drawSprite(ctx, v.pose, L.sprite.x, L.sprite.y, L.px);
  if (v.status) {
    const fill = v.status.tone === "ok" ? TERM.green : v.status.tone === "dim" ? PALETTE.text3 : PALETTE.text1;
    // A long crate name shrinks the line to fit beside the mascot, where
    // view.rs's content column ends.
    const size = Math.round(44 * s);
    const room = L.sprite.x - 32 * s - L.status.x;
    const wide = layout(ctx, v.status.text, font(size, 700, MONO)).width;
    const fit = wide > room ? Math.floor((size * room) / wide) : size;
    drawText(ctx, v.status.text, L.status.x, L.status.y, { font: font(fit, 700, MONO), fill });
  }
  if (v.counts) {
    drawText(ctx, v.counts, L.counts.x, L.counts.y, { font: font(Math.round(40 * s), 500, MONO), fill: TERM.green });
  }
  drawText(ctx, "Build / cache", L.barLabel.x, L.barLabel.y, { font: font(Math.round(30 * s), 500, MONO), fill: PALETTE.text3 });
  drawCacheBar(ctx, L.bar, v.filled, v.mix);
}

/**
 * A terminal pane: a window with three dots, its title and badge, a prompt
 * with a typed command, mono lines or the pixel build view, standing on its
 * checkout's target/ slab. Returns its layout.
 */
export function drawPane(ctx: CanvasRenderingContext2D, p: PaneState): PaneLayout {
  const L = paneLayout(p);
  const a = p.alpha ?? 1;
  if (a <= 0) return L;
  const open = clamp(p.open ?? 1);
  const dim = p.dim ?? 0;
  ctx.save();
  ctx.globalAlpha *= a * (1 - 0.6 * dim);
  if (L.slab) drawSlab(ctx, L.slab.x, L.slab.y, L.slab.w, p.slab ?? "", { alpha: 1 });
  if (open > 0) {
    const win = L.window;
    const bottom = win.y + win.h;
    ctx.save();
    // Folding down is a flap on a hinge at its base: squashed toward it,
    // and darker as it turns away from the light.
    ctx.translate(0, bottom);
    ctx.scale(1, open);
    ctx.translate(0, -bottom);
    if ((p.glow ?? 0) > 0) glow(ctx, win.x + win.w / 2, win.y + win.h / 2, win.w * 0.75, p.glowColor ?? PALETTE.amber, 0.25 * (p.glow ?? 0));
    roundedRect(ctx, win.x, win.y, win.w, win.h, 18);
    ctx.fillStyle = KIT.window;
    ctx.fill();
    ctx.save();
    ctx.clip();
    ctx.fillStyle = KIT.chrome;
    ctx.fillRect(win.x, win.y, win.w, TITLE_H);
    ctx.fillStyle = KIT.edge;
    ctx.fillRect(win.x, win.y + TITLE_H - 2, win.w, 2);
    [PALETTE.amber, PALETTE.teal, PALETTE.divider].forEach((c, i) => {
      ctx.fillStyle = i === 2 ? "#4a4235" : c;
      ctx.beginPath();
      ctx.arc(win.x + 32 + i * 28, win.y + TITLE_H / 2, 8, 0, TAU);
      ctx.fill();
    });
    if (p.title) {
      drawText(ctx, p.title, win.x + win.w / 2, win.y + TITLE_H / 2 + 13, {
        font: font(40, 500, MONO),
        fill: PALETTE.text3,
        align: "center",
      });
    }
    if (p.badge) drawBadge(ctx, win.x + win.w - 20, win.y + TITLE_H / 2, p.badge);
    const text = p.text ?? 56;
    if (p.prompt) {
      const spec = font(text, 600, MONO);
      const dollar = drawText(ctx, "$", L.prompt.x, L.prompt.y, { font: spec, fill: PALETTE.amber }).width;
      const cx = L.prompt.x + dollar + layout(ctx, " ", spec).width;
      const n = Math.floor(clamp(p.prompt.shown ?? Infinity, 0, p.prompt.text.length));
      const typed = drawText(ctx, p.prompt.text.slice(0, n), cx, L.prompt.y, { font: font(text, 500, MONO), fill: PALETTE.text1 }).width;
      if (p.prompt.cursor) {
        ctx.fillStyle = rgba(PALETTE.text1, 0.85);
        ctx.fillRect(cx + typed + 4, L.prompt.y - text * 0.78, text * 0.56, text * 0.95);
      }
    }
    if (p.lines?.length) {
      // The log scrolls away under the prompt.
      const top = L.prompt.y + text * 0.28;
      ctx.save();
      ctx.beginPath();
      ctx.rect(win.x, top, win.w, bottom - top);
      ctx.clip();
      p.lines.forEach((l, i) => {
        const slot = l.slot ?? i;
        const la = (l.alpha ?? 1) * clamp(1 + slot);
        if (la <= 0) return;
        const y = L.line.y + slot * L.lineStep + (l.dy ?? 0);
        ctx.save();
        ctx.globalAlpha *= la;
        if (l.tone === "compile") {
          const bold = font(text, 700, MONO);
          const head = "Compiling ";
          drawText(ctx, head, L.line.x, y, { font: bold, fill: PALETTE.amber });
          drawText(ctx, l.text, L.line.x + layout(ctx, head, bold).width, y, { font: font(text, 500, MONO), fill: PALETTE.text2 });
        } else {
          const fill = l.tone === "ok" ? TERM.green : l.tone === "dim" ? PALETTE.text3 : PALETTE.text1;
          drawText(ctx, l.text, L.line.x, y, { font: font(text, l.tone === "ok" ? 700 : 500, MONO), fill });
        }
        ctx.restore();
      });
      ctx.restore();
    }
    if (p.view) drawBuildView(ctx, L, p.view);
    ctx.restore();
    roundedRect(ctx, win.x, win.y, win.w, win.h, 18);
    ctx.lineWidth = 2.5;
    ctx.strokeStyle = KIT.edge;
    ctx.stroke();
    if (open < 1) {
      roundedRect(ctx, win.x, win.y, win.w, win.h, 18);
      ctx.fillStyle = rgba("#000000", 0.5 * (1 - open));
      ctx.fill();
    }
    ctx.restore();
  }
  ctx.restore();
  return L;
}

// Kit: the dashed frame.

export interface FrameState {
  rect: Rect;
  /** On the top edge at 56 px: `your machine`, `one machine`. */
  label: string;
  alpha?: number;
  /** 0..1 drawn on from beside the label, clockwise. */
  draw?: number;
  /** 0..1 dimmed back. */
  dim?: number;
}

const FRAME_R = 28;
const LABEL_SIZE = 56;

/** A dashed frame around a machine, labelled on its top edge. */
export function drawFrame(ctx: CanvasRenderingContext2D, f: FrameState): void {
  const a = (f.alpha ?? 1) * (1 - 0.55 * (f.dim ?? 0));
  const d = clamp(f.draw ?? 1);
  if (a <= 0 || d <= 0) return;
  const { x, y, w, h } = f.rect;
  const spec = font(LABEL_SIZE, 600);
  const lw = layout(ctx, f.label, spec, -1.1).width;
  const lx = x + 56;
  // The perimeter from the label's right, clockwise, back to its left.
  const pts: Pt[] = [];
  const corner = (cx: number, cy: number, a0: number) => {
    for (let i = 0; i <= 8; i++) {
      const t = a0 + (i / 8) * (Math.PI / 2);
      pts.push({ x: cx + FRAME_R * Math.cos(t), y: cy + FRAME_R * Math.sin(t) });
    }
  };
  pts.push({ x: lx + lw + 20, y });
  corner(x + w - FRAME_R, y + FRAME_R, -Math.PI / 2);
  corner(x + w - FRAME_R, y + h - FRAME_R, 0);
  corner(x + FRAME_R, y + h - FRAME_R, Math.PI / 2);
  corner(x + FRAME_R, y + FRAME_R, Math.PI);
  pts.push({ x: lx - 20, y });
  let total = 0;
  for (let i = 1; i < pts.length; i++) total += Math.hypot(pts[i].x - pts[i - 1].x, pts[i].y - pts[i - 1].y);
  ctx.save();
  ctx.globalAlpha *= a;
  ctx.strokeStyle = KIT.dash;
  ctx.lineWidth = 3;
  ctx.setLineDash([16, 12]);
  ctx.lineCap = "round";
  ctx.beginPath();
  ctx.moveTo(pts[0].x, pts[0].y);
  let run = 0;
  const L = total * d;
  for (let i = 1; i < pts.length; i++) {
    const seg = Math.hypot(pts[i].x - pts[i - 1].x, pts[i].y - pts[i - 1].y);
    if (run + seg >= L) {
      const q = lerpPt(pts[i - 1], pts[i], (L - run) / (seg || 1));
      ctx.lineTo(q.x, q.y);
      break;
    }
    run += seg;
    ctx.lineTo(pts[i].x, pts[i].y);
  }
  ctx.stroke();
  ctx.setLineDash([]);
  ctx.globalAlpha *= clamp(d * 3);
  drawText(ctx, f.label, lx, y + 20, { font: spec, fill: PALETTE.paper, tracking: -1.1 });
  ctx.restore();
}

// Kit: the CI side.

export interface RunnerState {
  rect: Rect;
  /** 56 px: `push to main`, `pull request`. */
  label: string;
  alpha?: number;
  dim?: number;
  /** Its status light: idle grey, busy amber (blinking on `t`), ok green, blocked red. */
  led?: "idle" | "busy" | "ok" | "blocked";
  /** Global seconds, for the busy blink and the activity lights. */
  t?: number;
  /** 0..1 how busy its activity lights are. */
  activity?: number;
  /**
   * The lit activity lights' colour: green (the default) while it restores,
   * amber while it compiles, as everywhere on the map.
   */
  lights?: string;
  /** Sets the lights' flicker, so racks side by side blink apart (default 91). */
  seed?: number;
}

/** Where cartons leave and land on a runner: the middle of its top edge. */
export const runnerPort = (r: Rect): Pt => ({ x: r.x + r.w / 2, y: r.y });

/** A CI runner: a rack with a label, a status light, vents and activity lights. */
export function drawRunner(ctx: CanvasRenderingContext2D, r: RunnerState): void {
  const a = (r.alpha ?? 1) * (1 - 0.6 * (r.dim ?? 0));
  if (a <= 0) return;
  const { x, y, w, h } = r.rect;
  const t = r.t ?? 0;
  ctx.save();
  ctx.globalAlpha *= a;
  roundedRect(ctx, x, y, w, h, 18);
  ctx.fillStyle = KIT.window;
  ctx.fill();
  ctx.lineWidth = 2.5;
  ctx.strokeStyle = KIT.edge;
  ctx.stroke();
  drawText(ctx, r.label, x + 28, y + 68, { font: font(56, 600), fill: PALETTE.paper, tracking: -1.1 });
  const led = r.led ?? "idle";
  const col = led === "ok" ? PALETTE.green : led === "busy" ? PALETTE.amber : led === "blocked" ? "#e0625a" : "#4a4235";
  const on = led === "busy" ? 0.45 + 0.55 * (0.5 + 0.5 * Math.cos((t / BEAT) * TAU)) : 1;
  const lx = x + w - 40;
  const ly = y + 50;
  ctx.fillStyle = rgba(col, on);
  ctx.beginPath();
  ctx.arc(lx, ly, 10, 0, TAU);
  ctx.fill();
  if (led !== "idle") glow(ctx, lx, ly, 40, col, 0.5 * on);
  // Two rack units under the label.
  const uh = (h - 116) / 2;
  const act = r.activity ?? 0;
  const fr = Math.floor(t * 30);
  for (let u = 0; u < 2; u++) {
    const uy = y + 96 + u * (uh + 8);
    roundedRect(ctx, x + 20, uy, w - 40, uh, 10);
    ctx.fillStyle = PALETTE.surface;
    ctx.fill();
    const mid = uy + uh / 2;
    ctx.strokeStyle = PALETTE.divider;
    ctx.lineWidth = 4;
    ctx.lineCap = "round";
    for (let i = 0; i < 4; i++) {
      const vx = x + w - 52 - i * 18;
      ctx.beginPath();
      ctx.moveTo(vx, mid - uh * 0.25);
      ctx.lineTo(vx, mid + uh * 0.25);
      ctx.stroke();
    }
    for (let i = 0; i < 8; i++) {
      const lit = act > 0.05 && hash(fr * 7 + i + u * 31, r.seed ?? 91) < 0.3 + 0.7 * act;
      ctx.fillStyle = lit ? (r.lights ?? PALETTE.green) : "#3a342a";
      ctx.fillRect(x + 44 + i * 22, mid - 6, 12, 12);
    }
  }
  ctx.restore();
}

export interface RemoteState {
  rect: Rect;
  alpha?: number;
  /** 0..1 of its shelf holding cartons. */
  fill?: number;
  /** 0..1 a glow as something lands. */
  lit?: number;
}

/** The remote's shelf: cells for stored cartons, map px, in fill order. */
export function remoteSlots(r: Rect): Pt[] {
  const cols = 7;
  const rows = 2;
  const out: Pt[] = [];
  const cw = (r.w - 80) / cols;
  for (let j = rows - 1; j >= 0; j--) {
    for (let i = 0; i < cols; i++) out.push({ x: r.x + 40 + (i + 0.5) * cw, y: r.y + 100 + (j + 1) * 58 });
  }
  return out;
}

/** The remote cache: a card labelled `remote cache` with a shelf of stored cartons. */
export function drawRemote(ctx: CanvasRenderingContext2D, r: RemoteState): void {
  const a = r.alpha ?? 1;
  if (a <= 0) return;
  const { x, y, w, h } = r.rect;
  ctx.save();
  ctx.globalAlpha *= a;
  if ((r.lit ?? 0) > 0) glow(ctx, x + w / 2, y + h / 2, w * 0.7, PALETTE.amberBright, 0.3 * (r.lit ?? 0));
  roundedRect(ctx, x, y, w, h, 24);
  ctx.fillStyle = KIT.window;
  ctx.fill();
  ctx.lineWidth = 2.5;
  ctx.strokeStyle = KIT.edge;
  ctx.stroke();
  // A cloud glyph, then the name.
  ctx.fillStyle = PALETTE.tealLight;
  ctx.beginPath();
  const gx = x + 50;
  const gy = y + 56;
  ctx.arc(gx, gy, 14, Math.PI * 0.5, Math.PI * 1.5);
  ctx.arc(gx + 18, gy - 12, 18, Math.PI, Math.PI * 1.85);
  ctx.arc(gx + 40, gy, 14, Math.PI * 1.5, Math.PI * 0.5);
  ctx.closePath();
  ctx.fill();
  drawText(ctx, "remote cache", x + 124, y + 70, { font: font(40, 600), fill: PALETTE.paper, tracking: -0.6 });
  const slots = remoteSlots(r.rect);
  const n = Math.round(clamp(r.fill ?? 0) * slots.length);
  slots.forEach((s, i) => {
    if (i < n) {
      drawCarton(ctx, s.x, s.y, 44, "stored");
    } else {
      ctx.strokeStyle = rgba(PALETTE.divider, 0.9);
      ctx.lineWidth = 2;
      ctx.setLineDash([5, 5]);
      ctx.strokeRect(s.x - 22, s.y - 34, 44, 34);
      ctx.setLineDash([]);
    }
  });
  ctx.restore();
}

/**
 * The read-only line a pull request's writes bounce off: a barrier from x0
 * to x1 at y, a padlock at its left, `read-only` (40 px) over its right.
 * `flash` 0..1 lights it as something hits it.
 */
export function drawReadOnly(ctx: CanvasRenderingContext2D, x0: number, x1: number, y: number, alpha = 1, flash = 0): void {
  if (alpha <= 0) return;
  const color = mix(PALETTE.text2, PALETTE.paper, flash);
  ctx.save();
  ctx.globalAlpha *= alpha;
  if (flash > 0) glow(ctx, (x0 + x1) / 2, y, (x1 - x0) * 0.4, PALETTE.paper, 0.25 * flash);
  ctx.strokeStyle = color;
  ctx.lineWidth = 3;
  ctx.beginPath();
  ctx.moveTo(x0 + 40, y - 7);
  ctx.lineTo(x1, y - 7);
  ctx.moveTo(x0 + 40, y + 7);
  ctx.lineTo(x1, y + 7);
  ctx.stroke();
  // Hatching between the rails, like a barrier.
  ctx.lineWidth = 3;
  ctx.beginPath();
  for (let x = x0 + 48; x < x1 - 6; x += 18) {
    ctx.moveTo(x, y + 7);
    ctx.lineTo(x + 10, y - 7);
  }
  ctx.stroke();
  // The padlock: a shackle over a body.
  ctx.lineWidth = 4;
  ctx.beginPath();
  ctx.arc(x0 + 16, y - 8, 8, Math.PI, 0);
  ctx.stroke();
  ctx.fillStyle = color;
  roundedRect(ctx, x0 + 3, y - 8, 26, 20, 4);
  ctx.fill();
  drawText(ctx, "read-only", x1, y - 22, { font: font(40, 600), fill: color, align: "right", tracking: -0.6 });
  ctx.restore();
}

// Kit: the permit rail.

export interface RailState {
  /** Left end of the rail and its center line. */
  x: number;
  y: number;
  w: number;
  /** Permits on the rail. */
  slots: number;
  /** 0..1 how full permit i is. */
  held?: (i: number) => number;
  alpha?: number;
  /** Slam-in drop, px above its place. */
  drop?: number;
}

/** Each permit's center on a rail. */
export function railSlots(r: RailState): Pt[] {
  const pitch = (r.w - 24) / r.slots;
  return Array.from({ length: r.slots }, (_, i) => ({ x: r.x + 12 + (i + 0.5) * pitch, y: r.y - (r.drop ?? 0) }));
}

/** The machine's one pool of compiler permits, a rail of slots every build queues for. */
export function drawRail(ctx: CanvasRenderingContext2D, r: RailState): void {
  const a = r.alpha ?? 1;
  if (a <= 0) return;
  const y = r.y - (r.drop ?? 0);
  const h = 40;
  ctx.save();
  ctx.globalAlpha *= a;
  roundedRect(ctx, r.x, y - h / 2, r.w, h, h / 2);
  ctx.fillStyle = PALETTE.surface;
  ctx.fill();
  ctx.lineWidth = 2.5;
  ctx.strokeStyle = PALETTE.text3;
  ctx.stroke();
  const pitch = (r.w - 24) / r.slots;
  const s = Math.min(pitch - 5, 24);
  for (const [i, p] of railSlots(r).entries()) {
    const k = clamp(r.held?.(i) ?? 0);
    roundedRect(ctx, p.x - s / 2, y - s / 2, s, s, 5);
    ctx.fillStyle = k > 0 ? mix("#3a342a", PALETTE.amber, k) : "#3a342a";
    ctx.fill();
    if (k > 0.5) glow(ctx, p.x, y, s * 1.6, PALETTE.amber, 0.35 * k);
  }
  ctx.restore();
}

// Mr Boxington on the map.

export interface BoxState {
  spot: BoxSpot;
  pose: BoxPose;
  alpha?: number;
  /** 0..1 his contact shadow (default 1). */
  shadow?: number;
}

export function drawMapBox(ctx: CanvasRenderingContext2D, b: BoxState): void {
  const a = b.alpha ?? 1;
  if (a <= 0) return;
  ctx.save();
  ctx.globalAlpha *= a;
  drawLogoBox(ctx, boxCam(b.spot), b.pose, b.shadow ?? 1);
  ctx.restore();
}

// The world: every persistent actor of the map in one state, drawn back to
// front. A scene can draw a whole state (a handoff, or its own tween between
// two) and its own flights on top, or draw actors one by one with the kit.

export interface WorldState {
  cam?: MapCam;
  frame?: FrameState | null;
  tower?: TowerState | null;
  /** Terminal cards (every-checkout's three, CI's dimmed one). */
  cards?: readonly PaneState[];
  /** The pane on the right of the machine. */
  pane?: PaneState | null;
  chips?: readonly ChipState[];
  remote?: RemoteState | null;
  runners?: readonly RunnerState[];
  box?: BoxState | null;
  labels?: readonly LabelState[];
}

/** Draw a world state under its camera: frame, tower, cards, pane, chips, remote, runners, box, labels. */
export function drawWorld(ctx: CanvasRenderingContext2D, s: WorldState): void {
  ctx.save();
  applyMapCam(ctx, s.cam ?? HOME);
  if (s.frame) drawFrame(ctx, s.frame);
  if (s.tower) drawTower(ctx, s.tower);
  for (const c of s.cards ?? []) drawPane(ctx, c);
  if (s.pane) drawPane(ctx, s.pane);
  for (const c of s.chips ?? []) drawChip(ctx, c);
  if (s.remote) drawRemote(ctx, s.remote);
  for (const r of s.runners ?? []) drawRunner(ctx, r);
  if (s.box) drawMapBox(ctx, s.box);
  for (const l of s.labels ?? []) drawLabel(ctx, l);
  ctx.restore();
}

/**
 * Where a world state's actors land on the screen, under its camera: a
 * conservative box per actor (the box's raised lid and strawberry included,
 * a chip without a width estimated), for checking what a frame keeps clear.
 */
export function worldBounds(s: WorldState): { actor: string; rect: Rect }[] {
  const cam = s.cam ?? HOME;
  const out: { actor: string; rect: Rect }[] = [];
  const add = (actor: string, r: Rect) => {
    const a = mapToScreen(cam, { x: r.x, y: r.y });
    out.push({ actor, rect: { x: a.x, y: a.y, w: r.w * cam.zoom, h: r.h * cam.zoom } });
  };
  if (s.frame) add("frame", s.frame.rect);
  if (s.tower) {
    const slabs = towerSlabs(s.tower);
    const x0 = Math.min(...slabs.map((q) => q.x)) - 8;
    const x1 = Math.max(...slabs.map((q) => q.x)) + s.tower.w + 8;
    const y0 = Math.min(...slabs.map((q) => q.y)) - 8;
    add("tower", { x: x0, y: y0, w: x1 - x0, h: s.tower.base - y0 });
  }
  for (const c of s.cards ?? []) add("card", c.rect);
  if (s.pane) add("pane", s.pane.rect);
  for (const c of s.chips ?? []) {
    const size = c.size ?? 40;
    add(`chip ${c.text}`, { x: c.x, y: c.y, w: c.w ?? c.text.length * size * 0.62 + size * 1.1, h: chipHeight(size) });
  }
  if (s.remote) add("remote", s.remote.rect);
  for (const r of s.runners ?? []) add(`runner ${r.label}`, r.rect);
  if (s.box) {
    const { spot, pose } = s.box;
    const berry = (pose.face?.strawberry ?? 0) > 0 ? 0.32 : 0;
    const top = spot.w * (117 / 120 + (pose.lid ?? 0) / 16 + berry);
    // The contact shadow reaches a little below the base.
    add("box", { x: spot.x - spot.w * 0.66, y: spot.y - top, w: spot.w * 1.32, h: top + spot.w * 0.06 });
  }
  for (const l of s.labels ?? []) {
    const size = l.size ?? 56;
    add(`label ${l.text}`, { x: l.x - l.text.length * size * 0.6, y: l.y - size, w: l.text.length * size * 1.2, h: size * 1.25 });
  }
  return out;
}

// The map's layout. Every rect is map px at HOME; all of it sits above
// CAPTION_TOP.

/** every-checkout's three terminals, labelled from NODES: local cards stand on slabs, CI's has none. */
export const EC_CARDS = {
  project: { x: 90, y: 232, w: 540, h: FLOOR - 232 },
  worktree: { x: 690, y: 232, w: 540, h: FLOOR - 232 },
  ci: { x: 1290, y: 232, w: 540, h: FLOOR - SLAB_H + 6 - 232 },
} as const satisfies Record<keyof typeof NODES, Rect>;
/** Their titles (40 px) and slab names. */
export const EC_TITLES = { project: "~/src/hk", worktree: "~/src/hk-fix", ci: "runner" } as const;
export const EC_SLABS = { project: "hk", worktree: "hk-fix" } as const;

/** The old target/ directories, then the two checkouts that fold onto them, bottom up. */
export const TOWER_NAMES = ["hk-try-2", "hk-pr-812", "hk-old-spike", "hk-bisect", "hk-fix", "hk"] as const;

/** A target/ slab's width in the tower (the pane's slab is the pane's width). */
export const SLAB_W = 400;

/** The machine, from under-cargo-build to another-worktree. */
export const MACHINE = {
  /**
   * `your machine` grown past the frame's edges: inside the machine the
   * frame is off screen, and its bottom edge never crosses the captions'
   * band, even pushed in.
   */
  frame: { x: -400, y: -300, w: 2720, h: 1680 },
  /** Mr Boxington, left: x 252-628, the lid's top at y 324 shut, its right end at 230 raised 4. */
  box: { x: 440, y: FLOOR, w: 376 },
  /** The pane, right: its window x 1050-1830, y 100-634, on its slab to FLOOR. */
  pane: { x: 1040, y: 100, w: 800, h: FLOOR - 100 },
  /** Cargo's plan between them: eight chips 320 x 56 every 64 px from y 150. */
  chips: { x: 680, y: 150, w: 320, pitch: 64 },
  /** The rest of the tower waits behind the pane, hidden. */
  tower: { x: 1240, base: FLOOR, w: SLAB_W },
} as const;

/** hk's crates in Cargo's plan, in the order under-cargo-build shows them. */
export const PLAN = ["libc", "proc-macro2", "quote", "syn", "serde", "tokio", "libgit2-sys", "hk"] as const;
/** The chip for crate i of PLAN. */
export const planChip = (i: number, tone: Tone = "neutral", extra: Partial<ChipState> = {}): ChipState => ({
  text: PLAN[i],
  x: MACHINE.chips.x,
  y: MACHINE.chips.y + i * MACHINE.chips.pitch,
  w: MACHINE.chips.w,
  tone,
  ...extra,
});

/** every-checkout's end: the frame is drawn around the box and the tower; CI waits outside, dimmed. */
export const OVERVIEW = {
  frame: { x: 36, y: 52, w: 1224, h: 684 },
  box: MACHINE.box,
  tower: { x: 800, base: FLOOR, w: SLAB_W },
  ci: EC_CARDS.ci,
} as const;

/** The CI side. `start` is six-builds → ci; the rest is where ci's own actors go. */
export const CI = {
  start: {
    frame: { x: 36, y: 52, w: 844, h: 684 },
    box: MACHINE.box,
  },
  runners: {
    push: { x: 940, y: 430, w: 440, h: FLOOR - 430 },
    pr: { x: 1440, y: 430, w: 440, h: FLOOR - 430 },
  },
  /** After the hop: Mr Boxington and his dimmed frame at the left. */
  hop: {
    frame: { x: 36, y: 52, w: 464, h: 684 },
    box: { x: 268, y: FLOOR, w: 300 },
  },
  /**
   * Suggested: the remote over the runners (its shelf's bottom at y 314),
   * its backend chips (56 px, at most 630 wide) stacked to its left, and
   * the read-only line across the pull request runner's approach.
   */
  remote: { x: 1180, y: 64, w: 700, h: 250 },
  backends: [
    { text: "GitHub Actions cache", x: 500, y: 70 },
    { text: "cache server", x: 500, y: 162 },
    { text: "S3 bucket", x: 500, y: 254 },
  ],
  readOnly: { x0: 1440, x1: 1880, y: 376 },
} as const;

// The streaks of the whip from ci into next-push.

let streakSprites: Map<string, HTMLCanvasElement> | null = null;
/** A streak of `color`: hot at its left end, trailing off right, soft top and bottom. */
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

/** The bar line the whip crosses. */
export const WHIP_AT = sec("ci").end;
/** The wind-up before the whip, seconds. */
export const WHIP_WIND = 0.25;

/**
 * The outgoing ci content's x offset at global time `t`: a 26 px wind-up
 * right over the WHIP_WIND before the whip, then out to the left over the
 * last WHIP, accelerating, still in shot on the frame before the bar line.
 */
export function whipOut(t: number): number {
  const wind = 26 * swiftOut(progress(WHIP_AT - WHIP - WHIP_WIND, WHIP_AT - WHIP, t));
  return wind - (wind + 1970) * progress(WHIP_AT - WHIP, WHIP_AT, t) ** 1.7;
}

/** The incoming next-push content's x offset: from 1800 px right on the bar line to rest after WHIP. */
export function whipIn(t: number): number {
  return 1800 * (1 - outQuart(progress(WHIP_AT, WHIP_AT + WHIP, t)));
}

/** Draw content whipping out: smeared along its motion at `t` (the ci scene's last WHIP). */
export function drawWhipOut(ctx: CanvasRenderingContext2D, t: number, draw: () => void): void {
  const x = whipOut(t);
  const v = x - whipOut(t - 1 / 120);
  smear(ctx, x, v, draw, 6);
}

const WHIP_BANDS: readonly (readonly [number, number, string, number, number])[] = [
  [230, 56, PALETTE.amberDeep, 0.22, 0.85],
  [372, 46, PALETTE.green, 0.3, 1.15],
  [520, 60, PALETTE.amber, 0.38, 0.95],
  [650, 44, PALETTE.green, 0.26, 1.25],
  [860, 54, PALETTE.teal, 0.2, 1.05],
];

/**
 * The whip's speed lines at global time `t`: they build over ci's last
 * WHIP, peak on the bar line (whose frame is these streaks alone on
 * PALETTE.bg) and die out over next-push's first WHIP, racing left the
 * whole time. Both scenes draw it over their content.
 */
export function drawWhip(ctx: CanvasRenderingContext2D, t: number): void {
  const q = (t - WHIP_AT) / WHIP;
  if (q <= -1 || q >= 1) return;
  const env = (1 - Math.abs(q)) ** 0.7;
  // Travel: fastest on the bar line.
  const g = 0.5 + 0.5 * Math.sin((Math.PI / 2) * q);
  ctx.save();
  ctx.globalCompositeOperation = "lighter";
  for (const [i, [y, h, color, a, par]] of WHIP_BANDS.entries()) {
    const len = 1700 * env * par;
    const x = 1500 - 2600 * g * par + hash(i, 5) * 400;
    if (x > W || x + len < 0) continue;
    ctx.globalAlpha = a * env;
    ctx.drawImage(streakSprite(color), x, y - h / 2, len, h);
  }
  const colors = [PALETTE.paper, PALETTE.amber, PALETTE.green, PALETTE.amberBright, PALETTE.tealLight];
  const span = W + 1900;
  for (let i = 0; i < 44; i++) {
    const fat = hash(i, 61) < 0.2;
    const par = 0.7 + hash(i, 53) * 0.7;
    const len = ((fat ? 480 : 260) + hash(i, 43) * 620) * par * (0.2 + 0.8 * env);
    const u = (hash(i, 47) * span - 3200 * g * par) % span;
    const x = (u < 0 ? u + span : u) - (span - W);
    if (x > W || x + len < 0) continue;
    const y = 110 + hash(i, 41) * 860;
    const w = fat ? 10 + hash(i, 59) * 12 : 1.5 + hash(i, 59) * 3;
    ctx.globalAlpha = (fat ? 0.22 + hash(i, 71) * 0.16 : 0.45 + hash(i, 71) * 0.25) * env;
    ctx.drawImage(streakSprite(colors[Math.floor(hash(i, 67) * colors.length)]), x, y - w / 2, len, w);
  }
  ctx.restore();
}

// Handoffs: the exact frame on every bar line.

type Adjacent<T extends readonly { id: string }[]> = T extends readonly [
  infer A extends { id: string },
  infer B extends { id: string },
  ...infer R extends { id: string }[],
]
  ? [`${A["id"]}|${B["id"]}`, ...Adjacent<[B, ...R]>]
  : [];
/** A bar line between two sections: `"<from>|<to>"`. */
export type BoundaryId = Adjacent<typeof SECTIONS>[number];

/** Every boundary in order, from fold|mr-boxington to morph|end. */
export const BOUNDARIES = SECTIONS.slice(1).map((s, i) => `${SECTIONS[i].id}|${s.id}` as BoundaryId);

export interface Handoff {
  id: BoundaryId;
  from: SectionId;
  to: SectionId;
  /** The bar line, global seconds: `sec(to).start`. */
  t: number;
  /**
   * hold: both scenes rest on this frame (the outgoing one settles onto it
   * by its last frame, the incoming one starts from it at rest). motion: a
   * shared move crosses the bar line (diveCam, drawWhip); both scenes follow
   * the named function through it.
   */
  meet: "hold" | "motion";
  /** What is on the frame, in words. */
  note: string;
  /** The map state on the frame, for the boundaries on the map. */
  world?: WorldState;
  /** Paint the whole frame. */
  draw(ctx: CanvasRenderingContext2D, env: SceneEnv): void;
}

const bg = (ctx: CanvasRenderingContext2D, env: SceneEnv, color: string = PALETTE.bg) => {
  ctx.fillStyle = color;
  ctx.fillRect(0, 0, env.W, env.H);
};

const worldHandoff = (id: BoundaryId, meet: Handoff["meet"], note: string, world: WorldState): Omit<Handoff, "from" | "to" | "t"> => ({
  id,
  meet,
  note,
  world,
  draw(ctx, env) {
    bg(ctx, env);
    drawWorld(ctx, world);
  },
});

/** The pane on the machine, as it stands between the builds. */
const machinePane = (extra: Partial<PaneState>): PaneState => ({
  rect: MACHINE.pane,
  title: "~/src/hk",
  badge: "time-lapse",
  slab: "hk",
  prompt: { text: "cargo build" },
  ...extra,
});
const machineFrame: FrameState = { rect: MACHINE.frame, label: "your machine" };
const parkedTower = (names: readonly string[]): TowerState => ({ ...MACHINE.tower, names });

/** every-checkout → under-cargo-build. */
export const EC_END: WorldState = {
  cam: HOME,
  frame: { rect: OVERVIEW.frame, label: "your machine" },
  tower: { ...OVERVIEW.tower, names: TOWER_NAMES },
  cards: [
    {
      rect: EC_CARDS.ci,
      title: EC_TITLES.ci,
      slab: null,
      prompt: { text: "cargo build" },
      lines: [{ text: "syn", tone: "compile" }],
      dim: 1,
    },
  ],
  box: { spot: MACHINE.box, pose: boxPose({ lid: 4, face: IDLE_FACE }) },
  labels: [{ text: NODES.ci.label, x: NODES.ci.x, y: NODES.ci.y, align: "center", alpha: 0.4 }],
};

/**
 * under-cargo-build → first-build: pushed in on the machine as far as it
 * goes with all of it in, his contact shadow (x 192) to the pane's slab
 * (x 1840) about 45 px in from the frame's edges. The pane's top sits 49 px
 * under the frame's and his shadow ends above the captions' band, so syn's
 * chip, the pane's `Compiling syn` and its mascot all read at once.
 * first-build holds it until its push into the store.
 */
export const FOLLOW: Readonly<MapCam> = { cx: 1016, cy: 542, zoom: 1.11 };
export const UC_END: WorldState = {
  cam: FOLLOW,
  frame: machineFrame,
  tower: parkedTower(TOWER_NAMES.slice(0, 5)),
  pane: machinePane({
    view: { pose: RUNNING_POSE, filled: 0, mix: NO_MIX, status: { text: "Compiling", tone: "dim" } },
  }),
  chips: PLAN.map((name, i) => planChip(i, name === "syn" ? "amber" : "dim")),
  box: { spot: MACHINE.box, pose: boxPose({ lid: 4, face: { ...IDLE_FACE, look: LOOK.list } }) },
};

/** first-build → same-checkout: the cold finish. */
export const FB_END: WorldState = {
  cam: HOME,
  frame: machineFrame,
  tower: parkedTower(TOWER_NAMES.slice(0, 5)),
  pane: machinePane({
    view: {
      pose: doneSprite(0, 0),
      filled: BAR_CELLS,
      mix: { ...NO_MIX, unconsulted: 1 },
      status: { text: "✓ Built", tone: "ok" },
    },
  }),
  box: { spot: MACHINE.box, pose: boxPose({ tape: 1, face: COLD_FACE }) },
};

/** same-checkout → another-worktree: the benchmark card, every label gone. */
export const SC_END: WorldState = {
  cam: HOME,
  frame: machineFrame,
  tower: parkedTower(TOWER_NAMES.slice(0, 5)),
  pane: machinePane({
    title: "",
    badge: null,
    prompt: null,
    view: { pose: doneSprite(1, 0), filled: BAR_CELLS, mix: { ...NO_MIX, hits: 1 } },
  }),
  box: { spot: MACHINE.box, pose: boxPose({ tape: 1, face: WARM_FACE }) },
};

/** another-worktree → six-builds: the poster, without its caption. */
export const AW_END: WorldState = {
  cam: HOME,
  frame: machineFrame,
  tower: parkedTower([...TOWER_NAMES.slice(0, 4), "hk"]),
  pane: machinePane({
    title: "~/src/hk-fix",
    badge: "illustration",
    slab: "hk-fix",
    view: {
      pose: doneSprite(27, 1),
      filled: BAR_CELLS,
      mix: { ...NO_MIX, hits: 27, misses: 1 },
      status: { text: "✓ Built", tone: "ok" },
    },
  }),
  box: { spot: MACHINE.box, pose: boxPose({ tape: 1, face: WARM_FACE }) },
};

/** six-builds → ci: Mr Boxington in his frame at the left, the two runners outside it. */
export const SB_END: WorldState = {
  cam: HOME,
  frame: { rect: CI.start.frame, label: "your machine" },
  runners: [
    { rect: CI.runners.push, label: "push to main" },
    { rect: CI.runners.pr, label: "pull request" },
  ],
  box: { spot: CI.start.box, pose: boxPose({ tape: 1, face: LOGO_FACE }) },
};

const HANDOFF_LIST: Omit<Handoff, "from" | "to" | "t">[] = [
  {
    id: "fold|mr-boxington",
    meet: "hold",
    note: "The logo box shut and taped, no face, flat colours (H1_POSE) at FRONT_CAM (H1_CAM), with its contact shadow, on PALETTE.bg.",
    draw(ctx, env) {
      bg(ctx, env);
      drawLogoBox(ctx, H1_CAM, H1_POSE);
    },
  },
  {
    id: "mr-boxington|what",
    meet: "motion",
    note: "The monocle dive, a sixteenth in: H2_POSE (the logo) under diveCam(t), zoom 1.108 from H2_CAM; the name card has wiped away.",
    draw(ctx, env) {
      bg(ctx, env);
      drawLogoBox(ctx, diveCam(sec("what").start), H2_POSE);
    },
  },
  {
    id: "what|every-checkout",
    meet: "hold",
    note: "Only the three node labels (drawNodeLabel, 56 px) at NODES, over where every-checkout's cards stand.",
    draw(ctx, env) {
      bg(ctx, env);
      drawNodeLabel(ctx, "project");
      drawNodeLabel(ctx, "worktree");
      drawNodeLabel(ctx, "ci");
    },
  },
  worldHandoff(
    "every-checkout|under-cargo-build",
    "hold",
    "The `your machine` frame around the box (lid hinged up 4 at its right end, no blush) and the leaning tower of six target/ slabs, hk on top; CI's card dimmed outside it with its label.",
    EC_END,
  ),
  worldHandoff(
    "under-cargo-build|first-build",
    "hold",
    "Pushed in (FOLLOW) on the whole machine: the box, lid hinged up 4, eyes toward the plan; Cargo's plan with syn's chip amber and the rest dimmed; the time-lapse pane, `Compiling` over an empty bar, its mascot at the pane's right, all in frame.",
    UC_END,
  ),
  worldHandoff(
    "first-build|same-checkout",
    "hold",
    "The cold finish: the box taped, no blush, no strawberry; the pane's mascot taped and matte over a full grey bar, `✓ Built`.",
    FB_END,
  ),
  worldHandoff(
    "same-checkout|another-worktree",
    "hold",
    "The benchmark card with no title, badge or text: the mascot taped, rosy, with its strawberry over a full green bar; the box the same.",
    SC_END,
  ),
  worldHandoff(
    "another-worktree|six-builds",
    "hold",
    "The poster without its caption: the box taped, rosy, with the strawberry; hk-fix's illustration pane, its mascot the same over a green bar with one amber cell, `✓ Built`.",
    AW_END,
  ),
  worldHandoff(
    "six-builds|ci",
    "hold",
    "The box (the logo, taped) in a narrowed `your machine` frame; the `push to main` and `pull request` runners outside it, idle.",
    SB_END,
  ),
  {
    id: "ci|next-push",
    meet: "motion",
    note: "The whip's peak: drawWhip's streaks alone on PALETTE.bg; ci's content is off to the left (whipOut), the chart still off to the right (whipIn).",
    draw(ctx, env) {
      bg(ctx, env);
      drawWhip(ctx, WHIP_AT);
    },
  },
  {
    id: "next-push|pruned",
    meet: "hold",
    note: "One plain cube, H5_POSE under WORLD_CAM, with its shadow; no grid.",
    draw(ctx, env) {
      bg(ctx, env);
      drawStagedBox(ctx, WORLD_CAM, H5_POSE);
    },
  },
  {
    id: "pruned|morph",
    meet: "hold",
    note: "The kept directories as flat PALETTE.amber discs (keptDiscs), nothing else.",
    draw(ctx, env) {
      bg(ctx, env);
      ctx.fillStyle = PALETTE.amber;
      for (const d of keptDiscs()) {
        ctx.beginPath();
        ctx.arc(d.x, d.y, d.r, 0, TAU);
        ctx.fill();
      }
    },
  },
  {
    id: "morph|end",
    meet: "hold",
    note: "The front-facing silhouette of END_POSE under END_CAM, flat PALETTE.amber on PALETTE.night.",
    draw(ctx, env) {
      bg(ctx, env, PALETTE.night);
      polygon(ctx, boxSilhouette(new View(END_CAM), END_POSE));
      ctx.fillStyle = PALETTE.amber;
      ctx.fill();
    },
  },
];

/** Every handoff by its boundary. */
export const HANDOFFS = Object.fromEntries(
  HANDOFF_LIST.map((h) => {
    const [from, to] = h.id.split("|") as [SectionId, SectionId];
    return [h.id, { ...h, from, to, t: sec(to).start }];
  }),
) as Record<BoundaryId, Handoff>;

/** Paint boundary `id`'s bar-line frame. */
export function drawHandoff(ctx: CanvasRenderingContext2D, id: BoundaryId, env: SceneEnv): void {
  HANDOFFS[id].draw(ctx, env);
}

/** The handoff a section starts from, or null for the first. */
export function handoffIn(id: SectionId): Handoff | null {
  return Object.values(HANDOFFS).find((h) => h.to === id) ?? null;
}

/** The handoff a section ends on, or null for the last. */
export function handoffOut(id: SectionId): Handoff | null {
  return Object.values(HANDOFFS).find((h) => h.from === id) ?? null;
}
