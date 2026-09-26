// Section 5, "Under cargo build". The camera moves into your machine: the
// frame grows past the screen, CI slides away, and the tower's top slab,
// hk, hops across and opens into a time-lapse terminal while the rest of the
// pile slides out of sight behind it. `cargo build` types and the real build
// view comes up. Cargo's plan of hk's dependencies unfolds between the
// terminal and Mr Boxington, and each crate fires its `rustc` call through
// the gap under his hovering lid, syn's amber and thicker. Then the camera
// pushes in on him and syn (map.ts UC_END), where first-build compiles it.

import { BEAT, PALETTE, type Scene, type SceneEnv, sec } from "../bible";
import { rgba } from "../color";
import { glow, ring } from "../fx";
import {
  applyMapCam,
  type BoxSpot,
  boxPose,
  type ChipState,
  type Curve,
  cursorOn,
  drawArrow,
  drawBuildView,
  drawChip,
  drawSpark,
  drawFrame,
  drawHandoff,
  drawLabel,
  drawMapBox,
  drawPane,
  drawTower,
  EC_END,
  FOLLOW,
  HOME,
  IDLE_FACE,
  MACHINE,
  type MapCam,
  mixMapCam,
  OVERVIEW,
  type PaneState,
  PLAN,
  planChip,
  type Pt,
  lerpRect,
  RUNNING_POSE,
  TERM,
  TOWER_NAMES,
  towerSlabs,
  typedChars,
  UC_END,
} from "../map";
import { clamp, inCubic, inOutCubic, lerp, outCubic, progress, pulse, smoothstep, swiftInOut, swiftOut, wobble } from "../math";
import type { Pose } from "../sprite";
import { type Caption, DETAIL, drawWords, font, MONO, wordStyle } from "../type";
import { drawSquashedSlab, dust, type SlabAt, tossed } from "./every-checkout-kit";

const S = sec("under-cargo-build");
const b = (n: number): number => n * BEAT;

// Beat map, local seconds. The score (score/under-cargo-build.ts) is written to these.

/** The frame grows past the screen and CI slides off. */
export const T_INTO = [b(0), b(0.4)] as const;
/** The hk slab hops off the tower onto the terminal's spot... */
export const T_HOP = [b(0), b(0.25)] as const;
/** ...and the terminal swings up from it. */
export const T_OPEN = [b(0.25), b(0.5)] as const;
/** `cargo build` types; on Enter the build view comes up. */
export const T_TYPE = [b(0.25), b(0.75)] as const;
export const T_RUN = b(0.75);
/** Cargo's plan unfolds, one crate per eighth. */
export const T_CHIP = PLAN.map((_, i) => b(1 + 0.5 * i));
/** Each crate's `rustc` call lands in the gap under the lid on the eighths from b2.5, syn's on the downbeat... */
export const T_HIT = PLAN.map((_, i) => b(2.5 + 0.5 * i));
/** ...after flying for FLY. */
export const FLY = b(0.3);
export const SYN = PLAN.indexOf("syn");
/** The camera pushes in on Mr Boxington and syn, and the rest of the plan dims. */
export const T_PUSH = [b(6), b(7.75)] as const;
/** From here the frame is exactly UC_END. */
const SETTLED = b(7.8);

const CAPTIONS: readonly Caption[] = [
  {
    out: 7.75,
    lines: [
      { in: 1.25, text: "Keep typing `cargo build`." },
      { in: 1.75, text: "mbx wraps each `rustc` call." },
    ],
  },
];

/** Desktop detail at 40 px, each landing on its beat and wiping on its own. */
const NOTES = [
  { text: "after setup", x: 1560, y: 240, in: 1, out: 6, fill: PALETTE.text3, align: "left" },
  { text: "Cargo plans the build", x: MACHINE.chips.x + MACHINE.chips.w, y: 128, in: 1.5, out: 6, fill: PALETTE.text2, align: "right" },
  { text: "no daemon: the cache agent starts and stops with each command", x: 90, y: 64, in: 3.5, out: 6.25, fill: PALETTE.text3, align: "left" },
] as const;
const WRAPPER = { x: MACHINE.box.x, y: 200, in: 2.5, out: 6 };
const NOTE_STYLE = { ...DETAIL, fill: PALETTE.text3 };

const OLD = TOWER_NAMES.slice(0, 5);
const TOP = towerSlabs({ ...OVERVIEW.tower, names: TOWER_NAMES })[5];
const PANE = UC_END.pane!;
const SPOT: BoxSpot = MACHINE.box;
/** Where the arrows all aim: the dark of the open mouth, a little apart. */
const MOUTH: Pt = { x: 500, y: 352 };
/** The gap under the lid, at its right end: every arrow threads it. */
const GAP: Pt = { x: 636, y: 308 };

/** Crate i's `rustc` call: from beside its chip, through the gap, into the mouth. */
function arrowCurve(i: number): Curve {
  const c = planChip(i);
  const a = { x: c.x - 10, y: c.y + 28 };
  const e = { x: MOUTH.x + 18 * ((i % 3) - 1), y: MOUTH.y + 4 * (i % 2) };
  // A quadratic through the gap at its midpoint.
  return { a, b: e, c: { x: 2 * GAP.x - (a.x + e.x) / 2, y: 2 * GAP.y - (a.y + e.y) / 2 } };
}

/** The map camera at `lt`. */
function camAt(lt: number): MapCam {
  return mixMapCam(HOME, FOLLOW, inOutCubic(progress(T_PUSH[0], T_PUSH[1], lt)));
}

/** The pixel mascot's pose: the running one, blinking once with the big box. */
const BLINK = [b(5.1), b(5.1) + 0.16] as const;
function spritePose(lt: number): Pose {
  return lt >= BLINK[0] && lt < BLINK[1] ? { ...RUNNING_POSE, eye: "shut" } : RUNNING_POSE;
}

/** The terminal: its slab hopping over from the tower, then the window swinging up, the prompt, the build view. */
function drawTerminal(ctx: CanvasRenderingContext2D, lt: number, t: number): void {
  const land: SlabAt = { x: PANE.rect.x, y: PANE.rect.y + PANE.rect.h - 62, w: PANE.rect.w, rot: 0 };
  if (lt < T_HOP[1]) {
    const u = progress(T_HOP[0], T_HOP[1], lt);
    // High enough to clear the pile sliding away under it; it stretches to the terminal's width only as it comes down.
    const p = tossed({ x: TOP.x, y: TOP.y, w: OVERVIEW.tower.w, rot: TOP.rot }, land, u, 245, 0.06);
    drawSquashedSlab(ctx, { ...p, w: lerp(OVERVIEW.tower.w, land.w, smoothstep(0.5, 1, u)) }, "hk", 0);
    return;
  }
  const open = swiftOut(progress(T_OPEN[0], T_OPEN[1], lt));
  const typing = lt < T_TYPE[1];
  const p: PaneState = {
    ...PANE,
    open,
    prompt: { text: "cargo build", shown: typedChars("cargo build", lt, T_TYPE[0], T_TYPE[1] - T_TYPE[0]), cursor: typing && cursorOn(t) },
    view: null,
  };
  // The slab takes the landing; the window overshoots a little as it stands.
  const over = 0.05 * wobble(lt, T_OPEN[0] + b(0.18), 3.2, 8);
  const squash = 0.18 * pulse(lt, T_HOP[1], 0.001, 0.06);
  const r = PANE.rect;
  ctx.save();
  if (over || squash) {
    ctx.translate(r.x + r.w / 2, r.y + r.h);
    ctx.scale(1 + 0.3 * squash, (1 + over) * (1 - squash));
    ctx.translate(-r.x - r.w / 2, -r.y - r.h);
  }
  const L = drawPane(ctx, p);
  if (lt >= T_RUN) {
    // Over the pane, as drawPane would paint its `view`, so it comes up on
    // Enter under a flash and hands over to drawPane unchanged.
    drawBuildView(ctx, L, { ...PANE.view!, pose: spritePose(lt) });
    // Enter: the view comes up with a flash of the terminal's light.
    const f = pulse(lt, T_RUN, 0.001, 0.09);
    if (f > 0.01) glow(ctx, L.sprite.x + L.sprite.w / 2, L.sprite.y + L.sprite.h / 2, 420, TERM.text, 0.3 * f);
  }
  ctx.restore();
}

/** The plan: each chip slides out of the terminal's edge into its place, springing; syn turns amber as it fires. */
function drawPlan(ctx: CanvasRenderingContext2D, lt: number): void {
  const dimAll = smoothstep(T_PUSH[0], T_PUSH[0] + b(0.5), lt);
  PLAN.forEach((_, i) => {
    const at = T_CHIP[i];
    if (lt < at) return;
    const u = progress(at, at + b(0.3), lt);
    const fire = T_HIT[i] - FLY;
    const syn = i === SYN;
    const amber = syn && lt >= fire;
    const base = planChip(i, amber ? "amber" : "neutral");
    const chip: ChipState = {
      ...base,
      x: base.x + 70 * (1 - swiftOut(u)),
      alpha: clamp(u * 3),
      scale: 1 + 0.12 * wobble(lt, at + b(0.1), 4, 9) + (syn ? 0.1 : 0.05) * pulse(lt, fire, 0.01, 0.12),
      lit: pulse(lt, fire, 0.01, 0.15) + (syn ? 0.6 * pulse(lt, T_HIT[i], 0.01, 0.35) : 0),
    };
    if (!syn && dimAll > 0) {
      // Dimming is a crossfade to the dim chip UC_END holds.
      drawChip(ctx, { ...chip, alpha: (chip.alpha ?? 1) * (1 - dimAll) });
      drawChip(ctx, { ...planChip(i, "dim"), alpha: dimAll });
      return;
    }
    drawChip(ctx, chip);
  });
}

/** The `rustc` calls, drawn inside the box's mouth pass so the lid and walls cover what goes behind them. */
function drawCalls(ctx: CanvasRenderingContext2D, lt: number): void {
  PLAN.forEach((_, i) => {
    const hit = T_HIT[i];
    const fire = hit - FLY;
    if (lt < fire || lt > hit + b(0.4)) return;
    const k = arrowCurve(i);
    const syn = i === SYN;
    const to = outCubic(progress(fire, hit, lt));
    const from = inCubic(progress(hit, hit + b(0.35), lt));
    drawArrow(ctx, k, {
      from,
      to,
      width: syn ? 10 : 5.5,
      color: syn ? PALETTE.amber : PALETTE.text2,
      head: syn ? 36 : 22,
      glow: syn ? 1 : 0.35,
    });
    // syn's call carries a spark at its head.
    if (syn && to < 1) drawSpark(ctx, k, to, { color: PALETTE.amberBright, size: 9, trail: 0.3 });
  });
}

/** Mr Boxington: lid hovering and knocked up a little by each call that lands, eyes on the plan. */
function boxAt(lt: number): { spot: BoxSpot; lid: number; look: [number, number]; blink: number } {
  let lid = 4;
  PLAN.forEach((_, i) => {
    lid += (i === SYN ? 1.1 : 0.45) * pulse(lt, T_HIT[i] + 0.012, 0.012, 0.07);
  });
  // A glance at the terminal as it opens, then to the plan.
  const toPane = smoothstep(T_OPEN[0], T_OPEN[0] + b(0.2), lt) * (1 - smoothstep(T_CHIP[0], T_CHIP[0] + b(0.25), lt));
  const toList = smoothstep(T_CHIP[0], T_CHIP[0] + b(0.25), lt);
  const look: [number, number] = [4 * toPane + 3 * toList, -1 * toList];
  const blink = lt >= BLINK[0] && lt < BLINK[1] ? 1 : 0;
  return { spot: SPOT, lid, look, blink };
}

/** A flash in the mouth as each call lands: amber for syn, the terminal's light for the rest. */
function drawHits(ctx: CanvasRenderingContext2D, lt: number): void {
  PLAN.forEach((_, i) => {
    const f = pulse(lt, T_HIT[i], 0.005, i === SYN ? 0.2 : 0.09);
    if (f < 0.01) return;
    const color = i === SYN ? PALETTE.amberBright : PALETTE.paper;
    glow(ctx, MOUTH.x, MOUTH.y - 6, i === SYN ? 300 : 170, color, (i === SYN ? 0.8 : 0.45) * f);
  });
  const syn = T_HIT[SYN];
  ring(ctx, MOUTH.x, MOUTH.y, 260, progress(syn, syn + 0.4, lt), PALETTE.amberBright, 6);
}

/** The 40 px notes, and RUSTC_WRAPPER over the lid with a tick down to the gap. */
function drawNotes(ctx: CanvasRenderingContext2D, lt: number): void {
  for (const n of NOTES) {
    drawWords(ctx, n.text, n.x, n.y, { ...NOTE_STYLE, fill: n.fill }, lt, b(n.in), b(n.out), n.align);
  }
  const style = { ...wordStyle(40, PALETTE.amber), font: font(40, 600, MONO) };
  drawWords(ctx, "RUSTC_WRAPPER", WRAPPER.x, WRAPPER.y, style, lt, b(WRAPPER.in), b(WRAPPER.out), "center");
  const tick = progress(b(WRAPPER.in), b(WRAPPER.in + 0.25), lt) * (1 - progress(b(WRAPPER.out), b(WRAPPER.out + 0.25), lt));
  if (tick > 0) {
    ctx.save();
    ctx.strokeStyle = rgba(PALETTE.amber, 0.7 * tick);
    ctx.lineWidth = 3;
    ctx.beginPath();
    ctx.moveTo(WRAPPER.x, WRAPPER.y + 12);
    ctx.lineTo(WRAPPER.x, WRAPPER.y + 12 + 22 * tick);
    ctx.stroke();
    ctx.restore();
  }
}

function draw(ctx: CanvasRenderingContext2D, lt: number, env: SceneEnv): void {
  if (lt < 1e-9) {
    drawHandoff(ctx, "every-checkout|under-cargo-build", env);
    return;
  }
  if (lt >= SETTLED) {
    drawHandoff(ctx, "under-cargo-build|first-build", env);
    return;
  }
  ctx.fillStyle = PALETTE.bg;
  ctx.fillRect(0, 0, env.W, env.H);
  ctx.save();
  applyMapCam(ctx, camAt(lt));

  // Into the machine: the frame grows past the screen's edges.
  const into = inCubic(progress(T_INTO[0], T_INTO[1], lt));
  drawFrame(ctx, { rect: lerpRect(OVERVIEW.frame, MACHINE.frame, into), label: "your machine" });

  // The rest of the pile slides off into the dark behind where the terminal stands (UC_END parks it there, hidden).
  const slide = progress(0, b(0.3), lt);
  drawTower(ctx, {
    x: lerp(OVERVIEW.tower.x, MACHINE.tower.x, swiftInOut(slide)),
    base: MACHINE.tower.base,
    w: MACHINE.tower.w,
    names: OLD,
    alpha: 1 - smoothstep(0.2, 0.9, slide),
  });

  // CI slides away to the right.
  const away = inCubic(progress(T_INTO[0], b(0.3), lt));
  if (away < 1) {
    const ci = EC_END.cards![0];
    ctx.save();
    ctx.translate(900 * away, 0);
    ctx.globalAlpha *= 1 - away;
    drawPane(ctx, ci);
    for (const l of EC_END.labels ?? []) drawLabel(ctx, l);
    ctx.restore();
  }

  drawTerminal(ctx, lt, env.t);
  drawPlan(ctx, lt);
  const m = boxAt(lt);
  drawMapBox(ctx, {
    spot: m.spot,
    pose: boxPose({ lid: m.lid, face: { ...IDLE_FACE, look: m.look, blink: m.blink }, inside: (c) => drawCalls(c, lt) }),
  });
  drawHits(ctx, lt);
  drawNotes(ctx, lt);
  dust(ctx, PANE.rect.x + PANE.rect.w / 2, PANE.rect.y + PANE.rect.h, PANE.rect.w, lt, T_HOP[1], 80, 0.8);
  ctx.restore();
}

export const scene: Scene = {
  id: S.id,
  start: S.start,
  end: S.end,
  draw,
  captions: () => CAPTIONS,
};
