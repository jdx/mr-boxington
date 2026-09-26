// Section 7, "Same checkout, empty target/". Measured on hk: with the store
// warm and target/ emptied, every compilation came back. The terminal flips
// over into the benchmark's card, the build view redrawn from its counts
// with no prompt or command, because the harness ran it on a CI runner. Mr
// Boxington tears his tape off and lifts his lid for the new build; the bar
// fills green over a fixed 2.5 beats while restored outputs spark out of him
// into hk's target/, the monocle's glint sweeps, the cheeks jump to rose on
// the first hit, the lid steps down, he is taped, and a strawberry lands.
//
// The section starts on first-build's cold finish (map.ts FB_END) and ends
// on the card with every label gone (SC_END). The numbers change only the
// text: the fill, the glint and the lid run on fixed beats, and without a
// warm fact the card shows no label, counter or source line, and the caption
// claims no figure.

import { BEAT, PALETTE, type Scene, type SceneEnv, sec } from "../bible";
import { mix, rgba } from "../color";
import { medianOf, type ReelFacts, tenths } from "../facts";
import { glow, makeCanvas, shake } from "../fx";
import {
  applyMapCam,
  boxFromSprite,
  type BuildPlan,
  type BuildView,
  buildAt,
  type Curve,
  drawFrame,
  drawMapBox,
  drawPane,
  drawSlab,
  drawSpark,
  drawTower,
  FB_END,
  MACHINE,
  type PaneState,
  paneLayout,
  type Rect,
  SC_END,
  TERM,
} from "../map";
import { clamp, hash, lerp, progress, swiftIn, swiftInOut } from "../math";
import { LID_MAX } from "../sprite";
import { type Caption, drawWords, font, layout, LABEL, MONO, type WordStyle, wordStyle } from "../type";
import { bouncyLid, bump, doneAt, evenUnits, glintSweeps, jolt, land, lidSteps, tapeAt } from "./first-build-kit";

const S = sec("same-checkout");

/** Local time of beat `n` of the section. */
const b = (n: number): number => n * BEAT;

// The beat map, section-local seconds. The score (score/same-checkout.ts)
// places its cues on these.

/** The terminal flips over, edge on at the half, into the benchmark card. */
export const T_FLIP0 = 0;
export const T_FLIP1 = b(0.5);
/** The old build's tape tears off, and the lid springs up for the new one. */
export const T_RIP0 = b(0.06);
export const T_RIP1 = b(0.25);
export const T_LIFT = b(0.25);
/** The card's label lands. */
export const T_LABEL = b(0.5);
/** The bar fills, a unit at a time, over a fixed 2.5 beats. */
export const T_FILL0 = b(0.75);
export const T_FILL1 = b(3.25);
/** The build finishes: taped. */
export const T_FINISH = b(3.25);
export const T_TAPE0 = b(3.25);
export const T_TAPE1 = b(3.625);
/** The strawberry lands on the lid. */
export const T_BERRY = b(4);
/** One blink in the hold. */
export const T_BLINK = b(6);
/** The card's labels wipe away, then the caption, so the card is bare on the bar line. */
export const T_OUT = b(7.5);

// The warm build, in reel time: a fixed count, every unit a hit. The
// benchmark's counts only change the counter's text.

/** Units in the plan. The lid steps at 14, 27, 41 and 54 done, the last just shut before the tape. */
const TOTAL = 60;

export const BUILD: BuildPlan = {
  start: S.at(b(0.25)),
  total: TOTAL,
  units: evenUnits(TOTAL, S.at(T_FILL0), S.at(T_FILL1), "hit"),
  finish: S.at(T_FINISH),
};

/** Global times the lid steps down, and the glint's sweeps begin: the score's clicks and tings. */
export const LID_STEPS = lidSteps(BUILD, S.end);
export const SWEEPS = glintSweeps(BUILD, S.end);
/** The first hit, where the cheeks jump to rose. */
export const T_BLUSH = BUILD.units[0].at - S.start;

// The copy the card carries, from the facts, and none without a warm fact.

/** The card's label, the counter's final figure, and the source line. */
export function cardCopy(facts: ReelFacts | null): { label: string | null; hits: number | null; source: string | null } {
  const warm = facts?.warm;
  if (!warm) return { label: null, hits: null, source: null };
  const version = facts?.versions?.mbx;
  const source = [
    "Linux CI runner",
    "warm store, same checkout, target/ emptied",
    version ? `mbx ${version}` : null,
    medianOf(warm.trials),
    "build view redrawn from these counts",
  ]
    .filter((s): s is string => s !== null)
    .join(" · ");
  return { label: facts.subject ? `${facts.subject} benchmark` : "benchmark", hits: warm.hits, source };
}

/** The caption's second line: the warm run's result, or a line that claims no number. */
export function restored(facts: ReelFacts | null): string {
  const warm = facts?.warm;
  return warm ? `${warm.hits} of ${warm.lookups} restored, ${tenths(warm.seconds)} s.` : "restored, not recompiled.";
}

// The terminal and the card.

/** The pane's window, where the flip happens, and its slab, which stays put. */
const OLD_PANE = FB_END.pane as PaneState;
const CARD = SC_END.pane as PaneState;
const WINDOW = paneLayout(CARD).window;

/** The card at `t`: the warm build's view, the strawberry held back until it lands. */
function cardView(t: number): BuildView {
  const v = buildAt(BUILD, t);
  const pose = t < S.at(T_BERRY) ? { ...v.pose, strawberry: false } : v.pose;
  return { pose, filled: v.filled, mix: v.mix };
}

let flipCanvas: HTMLCanvasElement | null = null;

/** How far the far edge of the turning card recedes: 0 flat, larger deeper. */
const DEPTH = 0.2;
/** Vertical strips the turning card is drawn in, for its perspective. */
const STRIPS = 64;

/**
 * The window turned `turn` radians about its upright middle (0 facing us,
 * pi/2 edge on), in perspective: drawn once flat on an offscreen canvas so
 * the pixel mascot keeps its square pixels, then laid back in upright strips
 * that shrink toward the far edge, and shaded as it turns from the light.
 * `mirror` puts the far edge on the left, for the face that turns in, and
 * `print` draws whatever else is on that face, in map px.
 */
function drawFlipped(
  ctx: CanvasRenderingContext2D,
  pane: PaneState,
  turn: number,
  mirror: boolean,
  lift: number,
  print?: (o: CanvasRenderingContext2D) => void,
): void {
  const m = ctx.getTransform();
  const pad = 4;
  const x0 = Math.floor(m.a * WINDOW.x + m.e) - pad;
  const y0 = Math.floor(m.d * WINDOW.y + m.f) - pad;
  const w = Math.ceil(m.a * WINDOW.w) + 2 * pad;
  const h = Math.ceil(m.d * WINDOW.h) + 2 * pad;
  flipCanvas ??= makeCanvas(w, h);
  if (flipCanvas.width !== w || flipCanvas.height !== h) {
    flipCanvas.width = w;
    flipCanvas.height = h;
  }
  const o = flipCanvas.getContext("2d")!;
  o.setTransform(1, 0, 0, 1, 0, 0);
  o.clearRect(0, 0, w, h);
  o.setTransform(m.a, 0, 0, m.d, m.e - x0, m.f - y0);
  drawPane(o, { ...pane, rect: WINDOW, slab: null });
  print?.(o);
  const cos = Math.cos(turn);
  const sin = Math.sin(turn) * (mirror ? -1 : 1);
  // A point u half-widths right of the middle lands at x(u), scaled by k(u).
  const k = (u: number) => 1 / (1 + DEPTH * u * sin);
  const x = (u: number) => ((u * cos * w) / 2) * k(u);
  const cx = x0 + w / 2;
  const cy = y0 + h / 2 - lift * m.d;
  ctx.save();
  ctx.setTransform(1, 0, 0, 1, 0, 0);
  ctx.imageSmoothingEnabled = true;
  for (let i = 0; i < STRIPS; i++) {
    const u0 = -1 + (2 * i) / STRIPS;
    const u1 = u0 + 2 / STRIPS;
    const xa = cx + x(u0);
    const xb = cx + x(u1);
    const sh = h * k((u0 + u1) / 2);
    // Overlap each strip a hair so no seam shows between them.
    const dx = Math.min(xa, xb) - 0.35;
    ctx.drawImage(flipCanvas, (i * w) / STRIPS, 0, w / STRIPS, h, dx, cy - sh / 2, Math.abs(xb - xa) + 0.7, sh);
  }
  // Turned away from the light, it darkens.
  const l = cx + x(-1);
  const r = cx + x(1);
  const tl = h * k(-1);
  const tr = h * k(1);
  ctx.fillStyle = rgba("#000000", 0.55 * (1 - Math.abs(cos)));
  ctx.beginPath();
  ctx.moveTo(l, cy - tl / 2 + pad);
  ctx.lineTo(r, cy - tr / 2 + pad);
  ctx.lineTo(r, cy + tr / 2 - pad);
  ctx.lineTo(l, cy + tl / 2 - pad);
  ctx.closePath();
  ctx.fill();
  ctx.restore();
}

/** The terminal, the flip, and the card with its copy, which turns in with it. */
function drawTerminal(ctx: CanvasRenderingContext2D, lt: number, t: number, facts: ReelFacts | null): void {
  const card: PaneState = { ...CARD, view: cardView(t) };
  const p = progress(T_FLIP0, T_FLIP1, lt);
  if (p <= 0) {
    drawPane(ctx, OLD_PANE);
    return;
  }
  if (p >= 1) {
    drawPane(ctx, card);
    drawCardCopy(ctx, lt, t, facts);
    return;
  }
  const slab = paneLayout(CARD).slab as Rect;
  drawSlab(ctx, slab.x, slab.y, slab.w, CARD.slab ?? "", { alpha: 1 });
  // Over and back, gathering speed into the edge-on moment and settling out
  // of it, with a little hop there.
  const turn = swiftInOut(p) * Math.PI;
  const lift = 26 * Math.sin(turn);
  if (turn < Math.PI / 2) drawFlipped(ctx, OLD_PANE, turn, false, lift);
  else drawFlipped(ctx, card, Math.PI - turn, true, lift, (o) => drawCardCopy(o, lt, t, facts));
}

// The copy on the card and over him.

const COUNTER: WordStyle = { ...wordStyle(40, TERM.green), font: font(40, 500, MONO) };
const SOURCE: WordStyle = { ...wordStyle(40, PALETTE.text3), font: font(40, 500) };
/** The source line's column over him, and its widest line. */
const SOURCE_X = 160;
const SOURCE_Y = 84;
const SOURCE_W = 860;

/** `text` broken into lines no wider than `width` in `style`. */
function wrap(ctx: CanvasRenderingContext2D, text: string, style: WordStyle, width: number): string[] {
  const lines: string[] = [];
  let line = "";
  for (const word of text.split(" ")) {
    const next = line ? `${line} ${word}` : word;
    if (line && layout(ctx, next, style.font).width > width) {
      lines.push(line);
      line = word;
    } else {
      line = next;
    }
  }
  if (line) lines.push(line);
  return lines;
}

/** The card's label and its hit counter, rolling with the bar. */
function drawCardCopy(ctx: CanvasRenderingContext2D, lt: number, t: number, facts: ReelFacts | null): void {
  const copy = cardCopy(facts);
  const L = paneLayout(CARD);
  const out = S.at(T_OUT);
  if (copy.label) drawWords(ctx, copy.label, L.prompt.x, L.prompt.y, LABEL, t, S.at(T_LABEL), out);
  if (copy.hits !== null && lt >= T_FILL0 - b(0.25)) {
    const n = Math.round((copy.hits * doneAt(BUILD.units, t)) / TOTAL);
    drawWords(ctx, `${n} hits`, L.counts.x, L.counts.y, COUNTER, t, S.at(T_FILL0), out);
  }
}

/** The source line over him. */
function drawSource(ctx: CanvasRenderingContext2D, t: number, facts: ReelFacts | null): void {
  const copy = cardCopy(facts);
  const out = S.at(T_OUT);
  if (copy.source) {
    wrap(ctx, copy.source, SOURCE, SOURCE_W).forEach((line, i) => {
      drawWords(ctx, line, SOURCE_X, SOURCE_Y + i * 48, SOURCE, t, S.at(b(1.25 + 0.25 * i)), out);
    });
  }
}

// Mr Boxington, and what he restores.

const SPOT = MACHINE.box;
/** His cheeks and the gap under his lid, map px. */
const CHEEKS = [
  { x: 283, y: 596 },
  { x: 572, y: 596 },
];

/** A restored output: a green spark from the gap under his lid down into hk's target/. */
export const SPARKS: readonly { curve: Curve; t0: number; t1: number }[] = BUILD.units
  .filter((_, i) => i % 2 === 0)
  .map((u, i) => {
    const t0 = u.at;
    const a = { x: lerp(560, 630, hash(i, 401)), y: lerp(318, 350, hash(i, 403)) };
    const end = { x: lerp(1120, 1760, hash(i, 407)), y: lerp(652, 676, hash(i, 409)) };
    return { curve: { a, c: { x: lerp(820, 960, hash(i, 411)), y: lerp(600, 660, hash(i, 413)) }, b: end }, t0, t1: t0 + lerp(0.28, 0.4, hash(i, 419)) };
  });

function drawSparks(ctx: CanvasRenderingContext2D, t: number): void {
  for (const s of SPARKS) {
    const u = progress(s.t0, s.t1, t);
    if (u <= 0 || u >= 1) continue;
    drawSpark(ctx, s.curve, swiftIn(u) * 0.35 + u * 0.65, { color: PALETTE.green, size: 6, trail: 0.22, alpha: Math.min(1, (1 - u) * 5) });
  }
}

function boxAt(lt: number, view: BuildView) {
  const t = S.at(lt);
  // The old tape tears off, then the lid springs up to the running hover;
  // from there it steps down with the card's mascot.
  const rip = 1 - swiftIn(progress(T_RIP0, T_RIP1, lt));
  const lid = view.pose.taped ? 0 : LID_MAX * land(lt, T_LIFT, 0.22, 0.3) - (LID_MAX - bouncyLid(LID_STEPS, t));
  const tape = lt < T_FINISH ? rip : tapeAt(lt, T_TAPE0, T_TAPE1);
  // The glint crosses the lens continuously on the mascot's clock, and a
  // star catches the rim as each sweep begins.
  let sweep: number | null = null;
  if (view.pose.glint !== null) sweep = (((t - BUILD.start) * 1000) % 640) / 400;
  const star = SWEEPS.reduce((k, s) => Math.max(k, bump(t, s.at, 0.26)), 0);
  // The strawberry drops onto the lid and bounces once.
  const fall = progress(T_BERRY - 0.12, T_BERRY, lt);
  const berry = lt < T_BERRY - 0.12 ? 0 : 1;
  const lift = 32 * (1 - fall * fall) + 5 * Math.abs(jolt(lt, T_BERRY, 0.3, 3.4));
  const blink = bump(lt, T_BLINK, 0.16);
  const pose = boxFromSprite(view.pose, {
    sweep,
    glint: star,
    strawberry: berry * clamp(0.3 + fall),
    berryLift: berry > 0 ? lift : 0,
    ...(blink > 0 ? { blink } : {}),
  });
  // Before the new build, the old one's finish: taped, looking at you.
  const before = lt < b(0.25);
  const face = before ? { ...(FB_END.box?.pose.face ?? {}) } : pose.face;
  const squash = 1 - 0.05 * bump(lt, T_BERRY, 0.2) - 0.04 * bump(lt, T_LIFT, 0.2);
  return {
    ...pose,
    face,
    lid: Math.max(lid, 0),
    tape,
    ...(squash !== 1 ? { squash } : {}),
  };
}

function draw(ctx: CanvasRenderingContext2D, lt: number, env: SceneEnv): void {
  const facts = env.facts;
  const t = env.t;
  ctx.fillStyle = PALETTE.bg;
  ctx.fillRect(0, 0, env.W, env.H);
  const view = cardView(t);
  const pose = boxAt(lt, view);
  const [sx, sy] = [shake(lt, T_TAPE1, 5, 0.06), shake(lt, T_BERRY, 5, 0.06)].reduce(([a, b], [c, d]) => [a + c, b + d], [0, 0]);

  ctx.save();
  if (sx || sy) ctx.translate(sx, sy);
  applyMapCam(ctx, SC_END.cam ?? { cx: 960, cy: 540, zoom: 1 });
  if (SC_END.frame) drawFrame(ctx, SC_END.frame);
  // The old target/ slabs wait behind the pane; the flip would show them.
  const flipping = lt > T_FLIP0 && lt < T_FLIP1;
  if (SC_END.tower && !flipping) drawTower(ctx, SC_END.tower);
  drawTerminal(ctx, lt, t, facts);
  // His blush lands with a warm bloom.
  const blush = bump(lt, T_BLUSH, 0.3);
  if (blush > 0) for (const c of CHEEKS) glow(ctx, c.x, c.y, 90, mix(PALETTE.amber, "#e47a68", 0.7), 0.5 * blush);
  drawMapBox(ctx, { spot: SPOT, pose });
  drawSparks(ctx, t);
  drawSource(ctx, t, facts);
  ctx.restore();
}

export const scene: Scene = {
  id: S.id,
  start: S.start,
  end: S.end,
  draw,
  captions: (facts: ReelFacts | null): readonly Caption[] => [
    {
      out: 7.75,
      lines: [
        { in: 0.75, text: "Same checkout, empty `target/`:" },
        { in: 3.25, text: restored(facts) },
      ],
    },
  ],
};

/** For the tests: the box's pose and the card's view on any frame. */
export const frameState = (lt: number) => {
  const view = cardView(S.at(lt));
  return { view, pose: boxAt(lt, view) };
};
