// Section 4, "Every checkout compiles again". The three node labels the
// lockup left behind grow terminal cards: the project, a worktree of it and
// a CI runner run `cargo build` a beat apart and step through the same
// crates, and an amber thread stitches their repeated `Compiling syn` rows.
// Then the local cards fold down into their target/ slabs, which are tossed
// onto a leaning tower of old ones as it piles up, the dashed `your machine`
// frame draws around it, and the CI card dims outside. The camera leans in
// on the pile while the caption holds and pulls back as Mr Boxington pops in
// beside it with his lid raised, the frame under-cargo-build starts from
// (map.ts EC_END).

import { BEAT, drawNodeLabel, NODES, PALETTE, type Scene, type SceneEnv, sec } from "../bible";
import { glow, ring } from "../fx";
import {
  applyMapCam,
  boxPose,
  cursorOn,
  drawFrame,
  drawHandoff,
  drawLabel,
  drawMapBox,
  drawPane,
  drawThread,
  EC_CARDS,
  EC_SLABS,
  EC_TITLES,
  HOME,
  IDLE_FACE,
  MACHINE,
  type MapCam,
  mixMapCam,
  OVERVIEW,
  type PaneState,
  paneLayout,
  type Pt,
  popIn,
  SLAB_H,
  type TermLine,
  TOWER_NAMES,
  towerSlabs,
  typedChars,
} from "../map";
import { clamp, inOutSine, inQuad, outCubic, progress, pulse, smoothstep, spring, swiftInOut, swiftOut, wobble } from "../math";
import type { Caption } from "../type";
import { drawSquashedSlab, dust, gather, rowWidth, type SlabAt, threadPoint, tossed } from "./every-checkout-kit";

const S = sec("every-checkout");
const b = (n: number): number => n * BEAT;

/** The cards, left to right, by their node. */
export const CARDS = ["project", "worktree", "ci"] as const;
type Card = (typeof CARDS)[number];
type Local = Exclude<Card, "ci">;

// Beat map, local seconds. The score (score/every-checkout.ts) is written to these.

/** Each card stands up from its slab on its beat, over STAND, and is up at T_STOOD... */
export const T_CARD = [b(0), b(1), b(2)] as const;
const STAND = b(0.3);
export const T_STOOD = T_CARD.map((t) => t + STAND);
/** ...and its `cargo build` types from here over TYPE. */
export const T_TYPE = [b(0.25), b(1.25), b(2.25)] as const;
export const TYPE = b(0.5);
/**
 * The crates each build reaches, one row per eighth once the command is in.
 * Short names only: `Compiling <crate>` has to fit a card at 56 px.
 */
export const ROWS = ["libc", "syn"] as const;
export const ROW_STEP = b(0.5);
/** When card `card`'s row `row` lands. */
export const rowAt = (card: number, row: number): number => T_TYPE[card] + TYPE + row * ROW_STEP;
/** The amber thread runs through the three `Compiling syn` rows... */
export const T_THREAD = [b(3), b(4)] as const;
/** ...and a spark runs it again on each beat while the caption holds. */
export const T_SPARKS = [b(4.25), b(5.25)] as const;
const SPARK_RUN = b(0.75);
/** The local cards fold down and slam shut on their slabs. */
export const T_FOLD = b(6);
export const T_SHUT = b(6.25);
/** The old target/ slabs drop onto the tower on 32nds, bottom first. */
export const T_DROPS = [b(6.5), b(6.625), b(6.75), b(6.875)] as const;
const DROP = b(0.2);
/** The thrown hk-fix and hk slabs land on top. */
export const T_LANDS = [b(7), b(7.25)] as const;
/** The `your machine` frame draws on around the tower. */
export const T_FRAME = [b(6.25), b(7.25)] as const;
/** The CI card dims outside it, its log scrolling up to leave `syn`. */
export const T_DIM = [b(6.25), b(7)] as const;
/** The camera leans in on the pile while the caption holds, then pulls back for the pop. */
export const T_LEAN = [b(7.5), b(10.5)] as const;
export const T_PULL = [b(10.75), b(11.4)] as const;
/** The pile teeters once. */
export const T_TEETER = b(9);
/** Sparks gather, and Mr Boxington pops in with his lid raised. */
export const T_POP = b(11.5);
export const GATHER = b(0.6);
/** From here the frame is exactly EC_END. */
const SETTLED = b(11.9);

const CAPTIONS: readonly Caption[] = [
  {
    out: 6,
    lines: [
      { in: 1, text: "Every checkout compiles" },
      { in: 1.25, text: "the same crates again." },
    ],
  },
  { out: 11.25, lines: [{ in: 7, text: "Old `target/` directories pile up." }] },
];

const TOWER = { ...OVERVIEW.tower, names: TOWER_NAMES };
const SLOTS = towerSlabs(TOWER);
/** Where each local card's slab ends up in the tower: hk-fix fifth, hk on top. */
const SLOT_OF: Record<Local, number> = { project: 5, worktree: 4 };
/** How high each is thrown (the top's y at mid-flight), and which way it tumbles. */
const THROW: Record<Local, { apex: number; tumble: number }> = {
  project: { apex: 150, tumble: 0.12 },
  worktree: { apex: 214, tumble: -0.09 },
};
/**
 * Leaning in on the pile: close enough that its names read on a phone, with
 * the tower's foot and the frame's bottom edge still above the captions.
 */
const PILE_CAM: MapCam = { cx: 1010, cy: 612, zoom: 1.6 };

/** The map camera at `lt`. */
function camAt(lt: number): MapCam {
  const lean = inOutSine(progress(T_LEAN[0], T_LEAN[1], lt));
  const pull = swiftInOut(progress(T_PULL[0], T_PULL[1], lt));
  return mixMapCam(HOME, PILE_CAM, lean * (1 - pull));
}

/** Card `k`'s pane at local time `lt`, before its log. */
function cardPane(k: number, lt: number, t: number): PaneState {
  const id = CARDS[k];
  const up = progress(T_CARD[k], T_CARD[k] + STAND, lt);
  const fold = id === "ci" ? 0 : inQuad(progress(T_FOLD, T_SHUT, lt));
  const typing = lt >= T_TYPE[k] && lt < T_TYPE[k] + TYPE;
  return {
    rect: EC_CARDS[id],
    title: EC_TITLES[id],
    slab: id === "ci" ? null : EC_SLABS[id],
    alpha: clamp(up * 5),
    open: outCubic(up) * (1 - fold),
    prompt: { text: "cargo build", shown: typedChars("cargo build", lt, T_TYPE[k], TYPE), cursor: typing && cursorOn(t) },
    dim: id === "ci" ? smoothstep(T_DIM[0], T_DIM[1], lt) : 0,
  };
}

/** Card `k`'s log: each row rises in on its eighth; CI's scrolls up as it dims, leaving `syn` on top. */
function cardRows(k: number, lt: number): TermLine[] {
  const rise = b(0.2);
  const scroll = CARDS[k] === "ci" ? swiftOut(progress(T_DIM[0] + b(0.25), T_DIM[0] + b(0.5), lt)) : 0;
  return ROWS.map((crate, j) => {
    const p = progress(rowAt(k, j), rowAt(k, j) + rise, lt);
    return { text: crate, tone: "compile", slot: j - scroll, alpha: clamp(p * 2.5), dy: 22 * (1 - swiftOut(p)) };
  });
}

/**
 * When the thread, drawing on over T_THREAD, reaches each card's `Compiling
 * syn` row, local seconds, for the score's stitches. By x along the thread
 * (its sag adds little), with the row as wide as 13 mono cells.
 */
export const STITCHES: readonly number[] = (() => {
  const xs = CARDS.map((id) => paneLayout({ rect: EC_CARDS[id], slab: id === "ci" ? null : EC_SLABS[id] }).line.x - 4);
  const end = xs[2] + 13 * 0.6 * 56 + 8;
  // inOutSine's inverse: where the eased progress passes each stitch's share of the run.
  return xs.map((x) => T_THREAD[0] + (T_THREAD[1] - T_THREAD[0]) * (Math.acos(1 - 2 * ((x - xs[0]) / (end - xs[0]))) / Math.PI));
})();

/** The thread's stitches: under each card's `Compiling syn`, from its first letter to its last. */
function threadPoints(ctx: CanvasRenderingContext2D): Pt[] {
  const w = rowWidth(ctx, "syn");
  return CARDS.flatMap((id) => {
    const L = paneLayout({ rect: EC_CARDS[id], slab: id === "ci" ? null : EC_SLABS[id] });
    const y = L.line.y + L.lineStep + 18;
    return [
      { x: L.line.x - 4, y },
      { x: L.line.x + w + 4, y },
    ];
  });
}

/** A local card's slab as it lies on the floor under the card. */
function cardSlab(id: Local): SlabAt {
  const r = EC_CARDS[id];
  return { x: r.x, y: r.y + r.h - SLAB_H, w: r.w, rot: 0 };
}

/** The tower at `lt`: the old slabs dropping in, the thrown ones once landed, jolting and teetering about its foot. */
function drawPile(ctx: CanvasRenderingContext2D, lt: number): void {
  let sway = 0.032 * wobble(lt, T_TEETER, 1.4, 2.4);
  for (const at of T_LANDS) sway += 0.012 * wobble(lt, at, 3, 6);
  sway *= 1 - smoothstep(b(10.5), b(11.5), lt);
  ctx.save();
  if (sway) {
    ctx.translate(TOWER.x + TOWER.w / 2, TOWER.base);
    ctx.rotate(sway);
    ctx.translate(-TOWER.x - TOWER.w / 2, -TOWER.base);
  }
  T_DROPS.forEach((at, i) => {
    const u = progress(at - DROP, at, lt);
    if (u <= 0) return;
    const s = SLOTS[i];
    const squash = lt >= at ? 0.12 * pulse(lt, at, 0.001, 0.05) : 0;
    drawSquashedSlab(ctx, { ...s, y: s.y - 70 * (1 - inQuad(u)), w: TOWER.w }, TOWER.names[i], squash, { alpha: clamp(u * 3) });
  });
  (["worktree", "project"] as const).forEach((id, n) => {
    const at = T_LANDS[n];
    if (lt < at) return;
    const i = SLOT_OF[id];
    drawSquashedSlab(ctx, { ...SLOTS[i], w: TOWER.w }, TOWER.names[i], 0.16 * pulse(lt, at, 0.001, 0.06));
  });
  ctx.restore();
}

/** The local cards' slabs in the air, from the floor under their shut cards to the top of the pile. */
function drawThrows(ctx: CanvasRenderingContext2D, lt: number): void {
  (["worktree", "project"] as const).forEach((id, n) => {
    const at = T_LANDS[n];
    if (lt < T_SHUT || lt >= at) return;
    const i = SLOT_OF[id];
    const u = progress(T_SHUT, at, lt);
    const p = tossed(cardSlab(id), { ...SLOTS[i], w: TOWER.w }, u, THROW[id].apex, THROW[id].tumble);
    // Stretched along the throw as it leaves the floor.
    drawSquashedSlab(ctx, p, TOWER.names[i], -0.1 * pulse(lt, T_SHUT, 0.001, 0.07));
  });
}

/** Card `k`, its log and its `Compiling syn` flare. */
function drawCard(ctx: CanvasRenderingContext2D, k: number, lt: number, t: number): void {
  const id = CARDS[k];
  if (lt < T_CARD[k] || (id !== "ci" && lt >= T_SHUT)) return;
  const p: PaneState = { ...cardPane(k, lt, t), lines: cardRows(k, lt) };
  // Standing up, it overshoots a little about its foot.
  const r = p.rect;
  const over = 0.06 * wobble(lt, T_CARD[k] + STAND * 0.8, 3.5, 9);
  ctx.save();
  if (over) {
    ctx.translate(r.x + r.w / 2, r.y + r.h);
    ctx.scale(1, 1 + over);
    ctx.translate(-r.x - r.w / 2, -r.y - r.h);
  }
  const L = drawPane(ctx, p);
  const flare = (p.open ?? 1) < 1 ? 0 : pulse(lt, rowAt(k, 1), 0.02, 0.16);
  if (flare > 0.01) glow(ctx, L.line.x + 190, L.line.y + L.lineStep - 18, 260, PALETTE.amber, 0.55 * flare);
  ctx.restore();
}

/** The node labels: the local ones drop away as their cards fold, CI's dims with its card. */
function drawNodes(ctx: CanvasRenderingContext2D, lt: number): void {
  CARDS.forEach((id, k) => {
    const n = NODES[id];
    const bump = lt > T_CARD[k] ? 0.09 * wobble(lt, T_CARD[k] + STAND * 0.5, 4, 10) : 0;
    const gone = id === "ci" ? 0 : progress(T_FOLD, T_FOLD + b(0.3), lt);
    if (gone >= 1) return;
    ctx.save();
    ctx.translate(n.x, n.y + 60 * inQuad(gone));
    ctx.scale(1 + bump, 1 + bump);
    ctx.translate(-n.x, -n.y);
    if (id !== "ci") drawNodeLabel(ctx, id, 1 - gone);
    else if (lt < T_DIM[0]) drawNodeLabel(ctx, id);
    // Dimmed, it is the label EC_END keeps.
    else drawLabel(ctx, { text: n.label, x: n.x, y: n.y, align: "center", alpha: 1 - 0.6 * smoothstep(T_DIM[0], T_DIM[1], lt) });
    ctx.restore();
  });
}

/** The thread through the `Compiling syn` rows, then a spark running it on each beat of the hold. */
function drawStitches(ctx: CanvasRenderingContext2D, lt: number): void {
  const p = inOutSine(progress(T_THREAD[0], T_THREAD[1], lt));
  const a = 1 - smoothstep(T_FOLD, T_FOLD + b(0.2), lt);
  if (p <= 0 || a <= 0) return;
  const pts = threadPoints(ctx);
  drawThread(ctx, pts, p, { width: 7, alpha: a });
  for (const at of T_SPARKS) {
    const u = progress(at, at + SPARK_RUN, lt);
    if (u <= 0 || u >= 1) continue;
    const q = threadPoint(pts, inOutSine(u));
    const fade = Math.sin(Math.PI * u);
    glow(ctx, q.x, q.y, 70, PALETTE.amberBright, 0.8 * fade * a);
    glow(ctx, q.x, q.y, 22, "#fff4dc", 0.9 * fade * a);
  }
}

/** Mr Boxington popping in on his spot: a spring about his foot, the lid springing up on its hinge. */
function drawPop(ctx: CanvasRenderingContext2D, lt: number): void {
  const spot = MACHINE.box;
  const mid = spot.y - spot.w * 0.55;
  gather(ctx, spot.x, mid, 340, lt, T_POP, GATHER);
  if (lt < T_POP) return;
  // Every overshoot dies out before the handoff.
  const settle = 1 - smoothstep(T_POP + b(0.15), SETTLED, lt);
  const s = 1 + (popIn(lt, T_POP, 4.2, 0.42) - 1) * settle;
  const lid = 4 + 2.6 * (1 - spring(lt - T_POP - 0.03, 4.5, 0.4)) * settle;
  ring(ctx, spot.x, mid, 430, progress(T_POP, T_POP + 0.45, lt), PALETTE.amberBright, 7);
  glow(ctx, spot.x, mid, 400, PALETTE.amber, 0.45 * pulse(lt, T_POP, 0.005, 0.12));
  drawMapBox(ctx, { spot: { ...spot, w: spot.w * Math.max(0, s) }, pose: boxPose({ lid, face: IDLE_FACE }), shadow: clamp(s) });
  dust(ctx, spot.x, spot.y, spot.w, lt, T_POP + 0.03, 70, 0.9);
}

function draw(ctx: CanvasRenderingContext2D, lt: number, env: SceneEnv): void {
  if (lt >= SETTLED) {
    drawHandoff(ctx, "every-checkout|under-cargo-build", env);
    return;
  }
  ctx.fillStyle = PALETTE.bg;
  ctx.fillRect(0, 0, env.W, env.H);
  ctx.save();
  applyMapCam(ctx, camAt(lt));

  const fd = inOutSine(progress(T_FRAME[0], T_FRAME[1], lt));
  if (fd > 0) drawFrame(ctx, { rect: OVERVIEW.frame, label: "your machine", draw: fd });
  if (lt >= T_DROPS[0] - DROP) drawPile(ctx, lt);
  for (let k = 0; k < CARDS.length; k++) drawCard(ctx, k, lt, env.t);
  drawStitches(ctx, lt);
  drawNodes(ctx, lt);

  // Landings kick up dust.
  (["project", "worktree"] as const).forEach((id, i) => {
    const r = EC_CARDS[id];
    dust(ctx, r.x + r.w / 2, r.y + r.h, r.w * 0.8, lt, T_STOOD[i], 50 + i, 0.45);
    dust(ctx, r.x + r.w / 2, r.y + r.h, r.w, lt, T_SHUT, 40 + i, 0.8);
  });
  dust(ctx, TOWER.x + TOWER.w / 2, TOWER.base, TOWER.w, lt, T_DROPS[0], 60, 0.9);

  drawThrows(ctx, lt);
  drawPop(ctx, lt);
  ctx.restore();
}

export const scene: Scene = {
  id: S.id,
  start: S.start,
  end: S.end,
  draw,
  captions: () => CAPTIONS,
};
