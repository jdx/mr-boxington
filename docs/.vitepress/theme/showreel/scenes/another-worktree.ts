// Section 8, "Another worktree": the benchmark card folds away into hk's
// target/ slab, which goes back on the pile as hk-fix's slab comes off it and
// opens an `illustration` terminal. `cargo build` starts a new build: the
// tape rips, the lid pops up and flings the strawberry off. The new build's
// key for `syn` rolls while `~/src/hk-fix/target` rewrites to `${target}`,
// comes out the same as the stored one and clicks onto it; the box reels it
// in and `syn` shoots out into hk-fix's target/, the first hit. The rest
// restore in green bursts on the eighths while the lid steps down, then
// `hk (edited)` compiles in an amber ring and closes the bar with its one
// amber cell. Taped, a strawberry, and the hold: from b11 the frame is
// map.ts AW_END, the reel's poster.
//
// The pixel pane is an illustration, not a benchmark, so it shows no counts:
// 28 units, one per cell of the bar, 27 restored and hk compiled.

import { BEAT, type LitRect, PALETTE, type Scene, sec } from "../bible";
import { type BoxPose, drawStrawberry, sparkle } from "../box";
import { rgba } from "../color";
import { glow, shake } from "../fx";
import {
  AW_END,
  BAR_CELLS,
  CAPTION_TOP,
  boxCam,
  boxFromSprite,
  type BuildPlan,
  type BuildView,
  buildAt,
  type CartonKind,
  chipHeight,
  type Curve,
  cursorOn,
  curveAt,
  drawCarton,
  drawChip,
  drawFrame,
  drawLabel,
  drawMapBox,
  drawPane,
  drawSlab,
  drawSpark,
  drawTag,
  drawTower,
  drawWorld,
  paneLayout,
  paneLit,
  FLOOR,
  MACHINE,
  type PaneState,
  type Pt,
  SC_END,
  SLAB_H,
  towerSlabs,
  typedChars,
  type Unit,
  WARM_FACE,
} from "../map";
import { clamp, hash, lerp, outBack, progress, pulse, spring, swiftIn, swiftInOut, swiftOut, TAU, wobble } from "../math";
import { View } from "../space";
import { LID_FPS } from "../sprite";
import { type Caption, drawText, drawWords, font, layout, MONO, wordStyle } from "../type";

const S = sec("another-worktree");

/** Local time of beat `n` of the section. */
const B = (n: number): number => n * BEAT;

// Accents, local seconds. The score (score/another-worktree.ts) places its
// cues on these.

/** The benchmark card folds down into hk's slab. */
export const FOLD = [0, B(0.375)] as const;
/** hk's slab goes back on the pile and hk-fix's comes down off it. */
export const SWAP = [B(0.375), B(0.875)] as const;
/** hk-fix's terminal opens up out of its slab. */
export const OPEN = [B(0.875), B(1.125)] as const;
/** `cargo build` types. */
export const TYPE = [B(0.9375), B(1.1875)] as const;
/** Enter: the build starts and the tape rips off. */
export const T_START = B(1.25);
/** The tape's rip, seconds. */
export const RIP = 0.06;
/** The lid pops up to hover and flings the strawberry off. */
export const T_LID = T_START + RIP;
/** The new key's tag pops up, rolling its digest, with the target path under it. */
export const T_CHIP = B(1.375);
/** The stored key's tag rises out of the box on its string. */
export const T_STORED = B(1.75);
/** `~/src/hk-fix/target` rewrites to `${target}`, as "same keys." lands. */
export const REWRITE = [B(1.875), B(2.375)] as const;
/** The rewritten path is taken up into the new key, and its digest settles. */
export const T_TAKE = B(2.5);
export const T_DIGEST = B(2.75);
/** The new tag lands on the stored one: the click. */
export const T_CLICK = B(3);
/** The box reels the matched tag in and `syn` shoots out. */
export const T_SHOOT = B(3.125);
/** The restores burst out on the eighths. */
export const BURSTS = [4, 4.5, 5, 5.5, 6, 6.5].map(B);
/** hk (edited) starts compiling. */
export const T_HK = B(7);
/** Its carton leaves for the slab. */
export const T_HK_SHOOT = B(8.25);
/** The tape goes on. */
export const T_TAPE = B(9.5);
/** The tape's run, seconds. */
export const TAPE_RUN = 0.2;
/** The strawberry lands. */
export const T_BERRY = B(10);
/** From here the frame is AW_END. */
export const T_REST = B(11);

/** Seconds a restored carton is in the air. */
const FLIGHT = 0.3;
/** How many cartons each burst throws: 26, with syn and hk 28, one per cell of the bar. */
const BURST_SIZES = [4, 4, 5, 4, 5, 4] as const;

// Where things are, map px (the map stays at HOME throughout).

const BOX = MACHINE.box;
/** A slab's place: top left, width and turn. */
interface SlabAt {
  x: number;
  y: number;
  w: number;
  rot: number;
}
/** The pane's slab: hk's at the start, hk-fix's from the swap on. */
const PANE_SLAB: SlabAt = { x: MACHINE.pane.x, y: FLOOR - SLAB_H, w: MACHINE.pane.w, rot: 0 };
/** The top of the pile, where hk-fix's slab waits and hk's goes. */
const PILE_TOP: SlabAt = { ...towerSlabs(AW_END.tower!)[4], w: MACHINE.tower.w };
/** Cartons sink into the slab where its front face starts, under the window's edge. */
const SLAB_IN = PANE_SLAB.y + 12;
/** The box's rim, where the shut lid's front edge ends (logo y 27 under boxCam at MACHINE.box). */
const RIM = 386;
/** One lid step on screen, px: LID_STEP of the box's 376 px. */
const LID_PX = 23.5;
/** Where the stored and the new key's tags hang: their holes. */
const STORED_TAG: Pt = { x: 672, y: 236 };
const NEW_TAG: Pt = { x: 672, y: 392 };
/** The target path chip's top left, and its type: small enough to fit between the box and the pane. */
const PATH_CHIP: Pt = { x: 638, y: 464 };
const PATH_SIZE = 32;
/** hk's carton while it compiles: its base's center. */
const HK_AT: Pt = { x: 850, y: 612 };
const KEY = "9e1f";
const TAG_SIZE = 40;

/** The middle of the gap between the rim and the hovering lid at lid `lid`, where cartons come out. */
const gapY = (lid: number): number => RIM - (lid * LID_PX) / 2;

// The build the pane shows, planned on the reel's clock. Every carton is a
// unit; a unit is done when its carton lands in hk-fix's target/.

interface Flight {
  /** Local seconds. */
  launch: number;
  land: number;
  /** Where it comes out of the box, and where it sinks into the slab. */
  from: Pt;
  to: Pt;
  /** How far over the straight line the throw arcs, px. */
  lift: number;
  size: number;
  kind: CartonKind;
  /** Turns over the flight, radians. */
  spin: number;
  label?: string;
}

/** syn, shot out once the box has its key. */
export const SYN: Flight = {
  launch: T_SHOOT,
  land: B(3.5),
  from: { x: 560, y: gapY(4) + 36 },
  to: { x: 1112, y: SLAB_IN },
  lift: 300,
  size: 100,
  kind: "restored",
  spin: -0.5,
  label: "syn",
};

/**
 * The restores, burst by burst: each burst fans out of the gap under the
 * lid, high and low, and pours into the near end of hk-fix's slab, under
 * the bar it fills and clear of the terminal's text. Where each lands is
 * fixed by a hash.
 */
export const RESTORES: readonly Flight[] = BURSTS.flatMap((at, k) =>
  Array.from({ length: BURST_SIZES[k] }, (_, i): Flight => {
    const n = k * 8 + i;
    const launch = at + i * 0.022;
    const slot = (i + 0.5 + (hash(n, 71) - 0.5) * 0.6) / BURST_SIZES[k];
    const size = 50 + hash(n, 83) * 14;
    return {
      launch,
      land: launch + FLIGHT,
      // The gap's height is filled in once the lid is known (flights, below).
      from: { x: 520 + hash(n, 73) * 80, y: 0 },
      to: { x: lerp(1078, 1150, slot), y: SLAB_IN },
      lift: 200 + (i / (BURST_SIZES[k] - 1)) * 200 + hash(n, 79) * 40,
      size,
      kind: "restored",
      spin: (hash(n, 89) - 0.5) * 2.4,
    };
  }),
);

/** hk, compiled: it hops from where it compiled into the slab. */
export const HK: Flight = {
  launch: T_HK_SHOOT,
  land: B(8.5),
  from: HK_AT,
  to: { x: 1120, y: SLAB_IN },
  lift: 110,
  size: 112,
  kind: "compiled",
  spin: 0.35,
  label: "hk",
};

const UNITS: readonly Unit[] = [
  { at: S.at(SYN.land), outcome: "hit" },
  ...RESTORES.map((f): Unit => ({ at: S.at(f.land), outcome: "hit" })),
  { at: S.at(HK.land), outcome: "miss" },
];

/** hk-fix's build, as the pixel pane and the big box show it. */
export const PLAN: BuildPlan = {
  start: S.at(T_START),
  total: UNITS.length,
  units: UNITS,
  finish: S.at(T_TAPE),
  gaze: "list",
};

/** The restores come out of the gap under the lid as it stands when they launch. */
const flights: readonly Flight[] = [
  SYN,
  ...RESTORES.map((f) => ({ ...f, from: { x: f.from.x, y: gapY(buildAt(PLAN, S.at(f.launch)).pose.lid) + f.size * 0.35 } })),
  HK,
];

/**
 * A throw's path for the carton's base: an arc over the line from where it
 * leaves, its peak past halfway, on down through the slab's edge until the
 * whole carton is in, at `land`.
 */
function path(f: Flight): Curve {
  const a = f.from;
  const b = { x: f.to.x, y: f.to.y + f.size * 0.92 };
  return { a, b, c: { x: lerp(a.x, b.x, 0.55), y: Math.min(a.y, f.to.y) - f.lift } };
}

// The box.

/** The strawberry on the shut lid, as box.ts seats it: its tip, width and turn under his camera. */
const SEAT = (() => {
  const view = new View(boxCam(BOX));
  const p = view.project([-0.21, 0.85, 0]);
  const q = view.project([-0.21, 0.85 + 1 / 12, 0]);
  // box.ts: BERRY_WIDTH (0.22 box widths) at the seat's depth.
  return { x: p.x, y: p.y, w: 0.22 * view.cam.scale * p.f, rot: Math.atan2(q.x - p.x, p.y - q.y) };
})();

/** The strawberry flung off by the lid: up and away over his left shoulder, turning, out of the frame. */
const HOP = 0.36;
/** Its fall back onto the lid at the end, seconds: 45 logo units (141 px) under gravity. */
export const BERRY_FALL = 0.16;
function flungBerry(lt: number): { x: number; y: number; w: number; rot: number } | null {
  const u = (lt - T_LID) / HOP;
  if (u < 0 || u >= 1) return null;
  // Thrown up and left, slowing as it climbs; gone off the top by the end.
  return { x: SEAT.x - 430 * u, y: SEAT.y - 720 * u + 260 * u * u, w: SEAT.w, rot: SEAT.rot - 4.2 * u };
}

/** The finishing tape at `lt`: 0 to 1, over the lid to TAPE_TOP (0.5), then down the front. */
export const tapeAt = (lt: number): number => swiftOut(progress(T_TAPE, T_TAPE + TAPE_RUN, lt));

/**
 * Local seconds each time the lid steps down, after it has popped up: the
 * pixel mascot's steps (lidAt at LID_FPS), which the big box's lid follows.
 */
export function lidSteps(): number[] {
  const out: number[] = [];
  const frames = Math.ceil((T_TAPE - T_START) * LID_FPS);
  let last = 4;
  for (let f = 0; f <= frames; f++) {
    const lt = T_START + (f + 0.5) / LID_FPS;
    const lid = buildAt(PLAN, S.at(lt)).pose.lid;
    if (lid < last) out.push(T_START + f / LID_FPS);
    last = lid;
  }
  return out;
}

/** The lid's hover: popped up to 4 on the start, then stepping down with the pixel mascot's. */
function lidAt(lt: number, view: BuildView): number {
  if (lt < T_LID) return 0;
  const pop = 4 * spring(lt - T_LID, 6, 0.42);
  return lt < T_LID + 0.25 ? pop : view.pose.lid;
}

/**
 * A wobble from `at` that dies out completely within `dur` seconds, so the
 * hold before AW_END is exactly still.
 */
function jolt(lt: number, at: number, freq: number, decay: number, dur = 0.4): number {
  const d = lt - at;
  return d < 0 || d >= dur ? 0 : wobble(lt, at, freq, decay) * (1 - d / dur) ** 2;
}

/** Every burst's little jolt: its recoil in his body. */
function recoil(lt: number): number {
  let s = 0;
  for (const at of BURSTS) s += jolt(lt, at, 7, 14);
  s += 0.6 * jolt(lt, T_SHOOT, 7, 14);
  s += 0.5 * jolt(lt, T_TAPE + TAPE_RUN, 6, 12);
  return s;
}

function boxAt(lt: number, view: BuildView): BoxPose {
  if (lt < T_START) {
    // Still the benchmark's finish, eyes following the swap.
    const look = swiftInOut(progress(FOLD[1], SWAP[1], lt));
    return { ...SC_END.box!.pose, face: { ...WARM_FACE, look: [3 * look, 0] } };
  }
  const done = lt >= T_TAPE;
  const pose = boxFromSprite(view.pose);
  const face = pose.face ?? {};
  // The rip: the tape peels back off the front, then the lid.
  const tape = lt < T_LID ? 1 - progress(T_START, T_LID, lt) : tapeAt(lt);
  // The blush drops with the new build and comes back rose on the first hit.
  const firstHit = SYN.land;
  const cheeks = lt < firstHit ? 3 * (1 - progress(T_START, T_START + 0.12, lt)) : 3 * clamp(spring(lt - firstHit, 5, 0.5), 0, 1.2) ;
  // The strawberry rides the lid until it pops, then comes back at the end:
  // it drops onto its seat under gravity and bounces once.
  const drop = progress(T_BERRY - BERRY_FALL, T_BERRY, lt);
  const berry = lt < T_LID ? 1 : progress(T_BERRY - BERRY_FALL, T_BERRY - BERRY_FALL + 0.05, lt);
  const bounce = lt < T_LID ? 0 : lt < T_BERRY ? 45 * (1 - drop * drop) : 4 * Math.max(0, jolt(lt, T_BERRY, 4, 9));
  // Eyes back out at the viewer as the build finishes.
  const look = face.look ?? [0, 0];
  const back = swiftInOut(progress(T_TAPE, T_TAPE + 0.2, lt));
  // A monocle star on the click; the sweep follows the hits.
  const star = pulse(lt, T_CLICK + 0.02, 0.02, 0.09);
  return {
    ...pose,
    lid: done ? 0 : lidAt(lt, view),
    tape,
    squash: 1 - 0.035 * recoil(lt),
    face: {
      ...face,
      cheeks: done ? 3 : cheeks,
      look: done ? [look[0] * (1 - back), look[1] * (1 - back)] : look,
      strawberry: berry,
      berryLift: bounce,
      glint: star,
      twitch: 0.12 * jolt(lt, T_CLICK, 5, 9, 0.5) + 0.08 * jolt(lt, HK.land, 5, 9, 0.5),
    },
  };
}

// The pane.

/** The pixel pane's build view, with the strawberry held back until the big box's lands. */
function viewAt(lt: number): BuildView {
  const v = buildAt(PLAN, S.at(lt));
  const done = lt >= T_TAPE;
  return {
    ...v,
    pose: { ...v.pose, strawberry: v.pose.strawberry && lt >= T_BERRY },
    status: done
      ? { text: "✓ Built", tone: "ok" }
      : lt >= T_HK && lt < HK.land
        ? { text: "Compiling hk", tone: "text" }
        : { text: "Compiling", tone: "dim" },
  };
}

const HK_FIX_PANE: PaneState = { ...AW_END.pane!, view: null, prompt: { text: "cargo build" } };

/** A slab on its way between two places, swinging out `bulge` px and nearer by `grow`. */
function flyingSlab(
  ctx: CanvasRenderingContext2D,
  from: SlabAt,
  to: SlabAt,
  k: number,
  name: string,
  bulge: number,
  grow: number,
): void {
  const arc = Math.sin(Math.PI * k);
  const w = lerp(from.w, to.w, k);
  const x = lerp(from.x, to.x, k) + bulge * arc;
  const y = lerp(from.y, to.y, k) - 40 * arc;
  const s = 1 + grow * arc;
  ctx.save();
  ctx.translate(x + w / 2, y + SLAB_H / 2);
  ctx.scale(s, s);
  ctx.translate(-x - w / 2, -y - SLAB_H / 2);
  drawSlab(ctx, x, y, w, name, { rot: lerp(from.rot, to.rot, k) - 0.1 * arc * Math.sign(bulge) });
  ctx.restore();
}

/** The machine behind the box: the pile, the swap, and the pane in whatever state it is in. */
function drawMachine(ctx: CanvasRenderingContext2D, lt: number, view: BuildView): void {
  drawFrame(ctx, AW_END.frame!);
  if (lt < SWAP[0]) {
    drawTower(ctx, SC_END.tower!);
    drawPane(ctx, { ...SC_END.pane!, open: 1 - swiftIn(progress(FOLD[0], FOLD[1], lt)) });
    return;
  }
  if (lt < SWAP[1]) {
    const k = swiftInOut(progress(SWAP[0], SWAP[1], lt));
    // hk swings up past the pile's left onto its top; hk-fix swings out
    // nearer, past its right, on its way down.
    drawTower(ctx, { ...AW_END.tower!, count: 4 });
    flyingSlab(ctx, PANE_SLAB, PILE_TOP, k, "hk", -240, -0.14);
    flyingSlab(ctx, PILE_TOP, PANE_SLAB, k, "hk-fix", 200, 0.12);
    return;
  }
  drawTower(ctx, AW_END.tower!);
  const typed = typedChars("cargo build", lt, TYPE[0], TYPE[1] - TYPE[0]);
  const running = lt >= T_START;
  drawPane(ctx, {
    ...HK_FIX_PANE,
    open: swiftOut(progress(OPEN[0], OPEN[1], lt)),
    prompt: { text: "cargo build", shown: typed, cursor: !running && (lt < TYPE[1] || cursorOn(S.at(lt))) },
    view: running ? view : null,
  });
}

/** The pane's bar lighting up: hk's amber cell as it lands, then a gleam along it as the build finishes. */
function drawBarLight(ctx: CanvasRenderingContext2D, lt: number): void {
  const bar = paneLayout(AW_END.pane!).bar;
  const cell = bar.w / BAR_CELLS;
  const lit = pulse(lt, HK.land, 0.01, 0.12);
  if (lit > 0) glow(ctx, bar.x + bar.w - cell / 2, bar.y + bar.h / 2, 70, PALETTE.amberBright, lit);
  const u = progress(T_TAPE, T_TAPE + 0.3, lt);
  if (u <= 0 || u >= 1) return;
  const x = lerp(bar.x - 60, bar.x + bar.w + 60, swiftInOut(u));
  ctx.save();
  ctx.beginPath();
  ctx.rect(bar.x, bar.y, bar.w, bar.h);
  ctx.clip();
  const g = ctx.createLinearGradient(x - 60, 0, x + 60, 0);
  g.addColorStop(0, "rgba(255,255,255,0)");
  g.addColorStop(0.5, "rgba(255,255,255,0.55)");
  g.addColorStop(1, "rgba(255,255,255,0)");
  ctx.fillStyle = g;
  ctx.fillRect(x - 60, bar.y, 120, bar.h);
  ctx.restore();
}

// The key: the stored tag, the new one, and the path it is built from.

const HEX = "0123456789abcdef";

/** The new key's digest: rolling at 30 Hz until each digit settles on the stored one's. */
function digest(lt: number): string {
  const frame = Math.floor(S.at(lt) * 30);
  return Array.from(KEY, (ch, i) => (lt >= T_DIGEST - (3 - i) * 0.035 ? ch : HEX[Math.floor(hash(frame * 5 + i, 97) * 16)])).join("");
}

// The rewrite, in fractions of REWRITE: each of the prefix's glyphs turns
// over in turn, `target` closes up on the placeholder, and `}` snaps on.
const PREFIX = "~/src/hk-fix/";
const flipStart = (i: number): number => (0.27 * i) / PREFIX.length;
const FLIP_LEN = 0.18;
const SLIDE = [0.3, 0.8] as const;
const CLOSE = 0.7;
/** Local seconds each glyph of `~/src/hk-fix/` turns over, halfway through its flip. */
export const FLIPS = Array.from(PREFIX, (_, i) => lerp(REWRITE[0], REWRITE[1], flipStart(i) + FLIP_LEN / 2));
/** Local seconds the `}` snaps on. */
export const T_CLOSE = lerp(REWRITE[0], REWRITE[1], CLOSE);

/**
 * A mono chip whose `~/src/hk-fix/` flips to `${` as `target` slides left
 * and `}` pops on. It scales about the middle of its full width, so a pop
 * never pushes it into the box or the pane.
 */
function drawPathChip(ctx: CanvasRenderingContext2D, x: number, y: number, k: number, scale: number, alpha: number): void {
  const spec = font(PATH_SIZE, 500, MONO);
  const cw = layout(ctx, "M", spec).width;
  const pad = PATH_SIZE * 0.55;
  const from = PREFIX;
  const core = "target";
  const slide = swiftInOut(progress(SLIDE[0], SLIDE[1], k));
  const close = outBack(2.2)(progress(CLOSE, 1, k));
  const lead = lerp(from.length, 2, slide);
  const w = (lead + core.length + close) * cw + 2 * pad;
  const h = chipHeight(PATH_SIZE);
  const lit = pulse(k, CLOSE + 0.02, 0.05, 0.12);
  ctx.save();
  ctx.globalAlpha *= alpha;
  const mid = ((from.length + core.length) * cw) / 2 + pad;
  ctx.translate(x + mid, y + h / 2);
  ctx.scale(scale, scale);
  ctx.translate(-mid, 0);
  drawChip(ctx, { text: "", x: 0, y: -h / 2, w, size: PATH_SIZE, tone: "neutral", lit });
  const base = h * 0.25;
  const glyph = (ch: string, gx: number, sy: number, a: number, fill: string) => {
    if (sy <= 0.02 || a <= 0) return;
    ctx.save();
    ctx.globalAlpha *= a;
    ctx.translate(gx + cw / 2, base - PATH_SIZE * 0.35);
    ctx.scale(1, sy);
    drawText(ctx, ch, 0, PATH_SIZE * 0.35, { font: spec, fill, align: "center" });
    ctx.restore();
  };
  // The checkout's own prefix: its first two letters flip over into `${`,
  // the rest fold away as `target` closes up on them.
  for (let i = 0; i < from.length; i++) {
    const p = progress(flipStart(i), flipStart(i) + FLIP_LEN, k);
    const gx = pad + i * cw * (i < 2 ? 1 : 1 - slide);
    if (i < 2) {
      glyph(from[i], gx, Math.abs(Math.cos(Math.PI * Math.min(p, 0.5))), p < 0.5 ? 1 : 0, PALETTE.text1);
      glyph("${"[i], gx, Math.abs(Math.cos(Math.PI * Math.max(p, 0.5))), p >= 0.5 ? 1 : 0, PALETTE.amberBright);
    } else {
      glyph(from[i], gx, 1 - p, 1 - slide, PALETTE.text1);
    }
  }
  const cx = pad + lead * cw;
  for (let i = 0; i < core.length; i++) glyph(core[i], cx + i * cw, 1, 1, slide > 0.5 ? PALETTE.amberBright : PALETTE.text1);
  glyph("}", cx + core.length * cw, clamp(close, 0, 1.3), clamp(close * 2), PALETTE.amberBright);
  ctx.restore();
}

/** The two tags and the path, from the new key's first roll to the stored tag reeled back in. */
function drawKey(ctx: CanvasRenderingContext2D, lt: number): void {
  if (lt < T_CHIP || lt > T_SHOOT + 0.05) return;
  const mouthPt = { x: 596, y: gapY(4) };
  const reel = swiftIn(progress(T_CLICK + 0.03, T_SHOOT, lt));
  // The stored tag rises out of the box on its string, then is reeled back in.
  const rise = spring(lt - T_STORED, 4.2, 0.55);
  if (rise > 0) {
    const hole = {
      x: lerp(lerp(mouthPt.x, STORED_TAG.x, rise), mouthPt.x, reel),
      y: lerp(lerp(mouthPt.y, STORED_TAG.y, rise), mouthPt.y, reel),
    };
    const matched = lt >= T_CLICK;
    const punch = 1 + 0.18 * pulse(lt, T_CLICK, 0.01, 0.08);
    ctx.save();
    ctx.translate(hole.x, hole.y);
    ctx.scale(punch * (1 - 0.7 * reel), punch * (1 - 0.7 * reel));
    if (matched) glow(ctx, 130, 0, 260, PALETTE.green, 0.8 * pulse(lt, T_CLICK, 0.01, 0.12));
    drawTag(ctx, 0, 0, `key ${KEY}…`, {
      size: TAG_SIZE,
      tone: matched ? "green" : "paper",
      rot: 0.05 * jolt(lt, T_STORED + 0.1, 3, 5, 0.8),
      string: { x: (mouthPt.x - hole.x) / (punch * (1 - 0.7 * reel)), y: (mouthPt.y - hole.y) / (punch * (1 - 0.7 * reel)) },
      alpha: clamp(rise * 3) * (1 - progress(0.7, 1, reel)),
    });
    ctx.restore();
  }
  if (lt >= T_CLICK) {
    sparkle(ctx, STORED_TAG.x + 250, STORED_TAG.y - 24, 34 * pulse(lt, T_CLICK, 0.01, 0.1), 1, 0.3);
    return;
  }
  // The new key: it pops up rolling, takes in the rewritten path, settles,
  // and slides up onto the stored one.
  const pop = spring(lt - T_CHIP, 5, 0.5);
  const slide = swiftIn(progress(T_DIGEST, T_CLICK, lt));
  const x = NEW_TAG.x;
  const y = lerp(NEW_TAG.y, STORED_TAG.y, slide);
  ctx.save();
  ctx.translate(x, y);
  ctx.scale(pop, pop);
  drawTag(ctx, 0, 0, `key ${digest(lt)}…`, { size: TAG_SIZE, tone: "paper", alpha: clamp(pop * 3) });
  ctx.restore();
  // The path under it, rewritten, then drawn up into it.
  const take = swiftIn(progress(T_TAKE, T_DIGEST, lt));
  if (take < 1) {
    const cp = spring(lt - T_CHIP - 0.04, 5, 0.7);
    const py = lerp(PATH_CHIP.y, NEW_TAG.y - chipHeight(PATH_SIZE) / 2, take);
    drawPathChip(ctx, PATH_CHIP.x, py, progress(REWRITE[0], REWRITE[1], lt), cp * (1 - 0.8 * take), clamp(cp * 3) * (1 - take));
  }
}

// Cartons in the air.

function drawFlight(ctx: CanvasRenderingContext2D, f: Flight, lt: number): void {
  if (lt < f.launch || lt >= f.land) return;
  const u = progress(f.launch, f.land, lt);
  const k = path(f);
  const p = curveAt(k, u);
  const color = f.kind === "compiled" ? PALETTE.amber : PALETTE.green;
  // A trail along the path of its middle.
  const mid: Curve = {
    a: { x: k.a.x, y: k.a.y - f.size * 0.4 },
    c: { x: k.c.x, y: k.c.y - f.size * 0.4 },
    b: { x: k.b.x, y: k.b.y - f.size * 0.4 },
  };
  drawSpark(ctx, mid, u, { color, size: f.size * 0.09, trail: 0.22, alpha: 0.55 });
  // Out of the gap small, full size in flight, and down into the slab.
  const grow = f === HK ? 1 : lerp(0.3, 1, swiftOut(progress(0, 0.14, u)));
  ctx.save();
  ctx.beginPath();
  ctx.rect(0, 0, 1920, f.to.y);
  ctx.clip();
  drawCarton(ctx, p.x, p.y, f.size * grow, f.kind, {
    rot: f.spin * u,
    label: p.y < f.to.y ? f.label : undefined,
  });
  ctx.restore();
}

/** Where a carton lands: a flash along the slab's edge and a spray of sparks up off it. */
function drawLanding(ctx: CanvasRenderingContext2D, f: Flight, lt: number): void {
  const d = lt - f.land;
  if (d < -0.02 || d > 0.35) return;
  const color = f.kind === "compiled" ? PALETTE.amberBright : PALETTE.green;
  const big = f.size / 60;
  const flash = pulse(lt, f.land, 0.02, 0.08);
  const spark = f.kind === "compiled" ? "#fbe3b0" : "#d4ebc8";
  glow(ctx, f.to.x, f.to.y + 8, 80 * big, color, flash);
  ctx.save();
  ctx.fillStyle = rgba(spark, 0.9 * flash);
  ctx.fillRect(f.to.x - 44 * big, f.to.y - 3, 88 * big, 5);
  const p = clamp(d / 0.35);
  if (d > 0) {
    for (let i = 0; i < 6; i++) {
      const seed = Math.round(f.land * 1000) + i * 17;
      const a = -Math.PI * (0.2 + 0.6 * hash(seed, 131));
      const r = (40 + 50 * hash(seed, 137)) * big * swiftOut(p);
      const sx = f.to.x + Math.cos(a) * r;
      const sy = f.to.y + Math.sin(a) * r + 70 * p * p;
      glow(ctx, sx, sy, 16 * (1 - p) + 4, color, (1 - p) ** 1.2);
      ctx.fillStyle = rgba(spark, (1 - p) ** 1.2);
      ctx.beginPath();
      ctx.arc(sx, sy, 3 * (1 - p) + 1, 0, TAU);
      ctx.fill();
    }
  }
  ctx.restore();
}

/** A shock ring on the floor round the box: a burst of restores leaving. */
function floorRing(ctx: CanvasRenderingContext2D, p: number, color: string): void {
  if (p <= 0 || p >= 1) return;
  const e = swiftOut(p);
  const rx = 150 + 180 * e;
  ctx.save();
  ctx.strokeStyle = rgba(color, 0.7 * (1 - p) ** 1.5);
  ctx.lineWidth = 5 * (1 - p) + 1;
  ctx.beginPath();
  ctx.ellipse(BOX.x, FLOOR, rx, rx * 0.12, 0, 0, TAU);
  ctx.stroke();
  ctx.restore();
}

// hk (edited), compiling.

/** hk's compile ring, round the carton's middle. */
const HK_RING = { x: HK_AT.x, y: HK_AT.y - HK.size * 0.42, r: 104 };
/** Seconds the ring takes to burst and fade once hk leaves it. */
const RELEASE = 0.16;

/** The ring letting go as hk leaves it: a full amber circle bursting out and fading. */
function drawRelease(ctx: CanvasRenderingContext2D, lt: number): void {
  const p = progress(HK.launch, HK.launch + RELEASE, lt);
  if (p <= 0 || p >= 1) return;
  const e = swiftOut(p);
  glow(ctx, HK_RING.x, HK_RING.y, 150 + 60 * e, PALETTE.amber, 0.6 * (1 - p));
  ctx.save();
  ctx.strokeStyle = rgba(PALETTE.amberBright, (1 - p) ** 1.5);
  ctx.lineWidth = 8 * (1 - p) + 1;
  ctx.beginPath();
  ctx.arc(HK_RING.x, HK_RING.y, HK_RING.r * (1 + 0.45 * e), 0, TAU);
  ctx.stroke();
  ctx.restore();
}

function drawCompile(ctx: CanvasRenderingContext2D, lt: number): void {
  drawRelease(ctx, lt);
  if (lt < T_HK - 0.02 || lt >= HK.launch) return;
  const pop = spring(lt - T_HK, 4.5, 0.5);
  const p = progress(T_HK, HK.launch, lt);
  const frame = Math.floor(S.at(lt) * 30);
  const cy = HK_RING.y;
  const r = HK_RING.r;
  // rustc at work: a glow that breathes, the ring filling round the carton,
  // and a crackle of short arcs off its edge.
  glow(ctx, HK_AT.x, cy, 150, PALETTE.amber, (0.45 + 0.25 * hash(frame, 101)) * clamp(pop));
  ctx.save();
  ctx.lineCap = "round";
  ctx.strokeStyle = rgba(PALETTE.amber, 0.25 * clamp(pop));
  ctx.lineWidth = 8;
  ctx.beginPath();
  ctx.arc(HK_AT.x, cy, r * pop, 0, TAU);
  ctx.stroke();
  ctx.strokeStyle = PALETTE.amberBright;
  ctx.lineWidth = 8;
  ctx.beginPath();
  ctx.arc(HK_AT.x, cy, r * pop, -Math.PI / 2, -Math.PI / 2 + TAU * swiftInOut(p));
  ctx.stroke();
  ctx.lineWidth = 3;
  for (let i = 0; i < 6; i++) {
    if (hash(frame * 7 + i, 103) < 0.45) continue;
    const a0 = hash(frame * 7 + i, 107) * TAU;
    ctx.strokeStyle = rgba(PALETTE.amberBright, 0.9);
    ctx.beginPath();
    for (let j = 0; j <= 4; j++) {
      const a = a0 + j * 0.09;
      const rr = r * pop + (j % 2 ? 10 : -4) + hash(frame + j, 109 + i) * 12;
      if (j === 0) ctx.moveTo(HK_AT.x + Math.cos(a) * rr, cy + Math.sin(a) * rr);
      else ctx.lineTo(HK_AT.x + Math.cos(a) * rr, cy + Math.sin(a) * rr);
    }
    ctx.stroke();
  }
  ctx.restore();
  // The carton comes together as it compiles: a jitter that dies away.
  const jit = (1 - p) * 3;
  drawCarton(ctx, HK_AT.x + (hash(frame, 113) - 0.5) * jit, HK_AT.y, HK.size * pop, "compiled", {
    label: "hk",
    rot: 0.03 * Math.sin(lt * 40) * (1 - p),
  });
  drawLabel(ctx, {
    text: "hk (edited)",
    x: HK_AT.x,
    y: cy - r - 26,
    size: 56,
    mono: true,
    align: "center",
    fill: PALETTE.amberBright,
    alpha: clamp(pop * 2),
  });
}

/** The label stays where hk compiled and lifts away as the ring lets go, clear of the pane. */
function drawHkLabel(ctx: CanvasRenderingContext2D, lt: number): void {
  const u = progress(HK.launch, HK.launch + RELEASE, lt);
  if (u <= 0 || u >= 1) return;
  drawLabel(ctx, {
    text: "hk (edited)",
    x: HK_AT.x,
    y: HK_RING.y - HK_RING.r - 26 - 24 * swiftOut(u),
    size: 56,
    mono: true,
    align: "center",
    fill: PALETTE.amberBright,
    alpha: (1 - u) ** 2,
  });
}

// The copy beside the diagram.

// How a restore lands, for a desktop viewer: along the top, once syn is in,
// and gone before the hold.
const DETAIL = wordStyle(40, PALETTE.text2);
const DETAIL_TEXT = "restored by reflink or hard link where the filesystem allows, copied otherwise";
const DETAIL_IN = B(5.25);
const DETAIL_OUT = B(10.25);

const CAPTIONS: readonly Caption[] = [
  {
    out: 6.25,
    lines: [
      { in: 1.5, text: "New worktree, new path," },
      { in: 2, text: "same keys." },
    ],
  },
  {
    // A sixteenth after the storyboard's b6.75, so the first word rises
    // only once the last caption's wipe has cleared.
    out: 11.75,
    lines: [
      { in: 7, text: "Matching crates restore." },
      { in: 8.5, text: "Your edit compiles." },
    ],
  },
];

function draw(ctx: CanvasRenderingContext2D, lt: number): void {
  ctx.fillStyle = PALETTE.bg;
  ctx.fillRect(0, 0, 1920, 1080);
  if (lt >= T_REST) {
    // The poster, and the handoff to six-builds.
    drawWorld(ctx, AW_END);
    return;
  }
  const view = viewAt(lt);
  const [sx, sy] = shake(lt, T_CLICK, 5, 0.05);
  ctx.save();
  // Nothing strays into the captions' band.
  ctx.beginPath();
  ctx.rect(0, 0, 1920, CAPTION_TOP);
  ctx.clip();
  ctx.translate(sx, sy);
  const punch = 1 + 0.025 * pulse(lt, T_CLICK, 0.015, 0.1);
  ctx.translate(STORED_TAG.x + 130, STORED_TAG.y);
  ctx.scale(punch, punch);
  ctx.translate(-STORED_TAG.x - 130, -STORED_TAG.y);
  drawMachine(ctx, lt, view);
  // The rings go down on the floor behind him.
  floorRing(ctx, progress(SYN.launch, SYN.launch + 0.5, lt), PALETTE.green);
  for (const at of BURSTS) floorRing(ctx, progress(at, at + 0.5, lt), PALETTE.green);
  const pose = boxAt(lt, view);
  drawMapBox(ctx, { spot: BOX, pose });
  const berry = flungBerry(lt);
  if (berry) drawStrawberry(ctx, berry.x, berry.y, berry.w, berry.rot);
  // A flash in the mouth as each burst leaves.
  for (const at of [SYN.launch, ...BURSTS]) {
    glow(ctx, 560, gapY(view.pose.lid) + 10, 150, PALETTE.green, 0.8 * pulse(lt, at, 0.01, 0.08));
  }
  drawKey(ctx, lt);
  drawCompile(ctx, lt);
  for (const f of flights) drawLanding(ctx, f, lt);
  for (const f of flights) drawFlight(ctx, f, lt);
  drawHkLabel(ctx, lt);
  drawBarLight(ctx, lt);
  drawWords(ctx, DETAIL_TEXT, 960, 64, DETAIL, lt, DETAIL_IN, DETAIL_OUT, "center");
  ctx.restore();
}

/** The pane's window, lit: the benchmark card's as it folds down, then hk-fix's as it opens up. */
function lit(lt: number): LitRect | null {
  if (lt < SWAP[0]) return paneLit(SC_END.pane!, undefined, 1 - swiftIn(progress(FOLD[0], FOLD[1], lt)));
  if (lt < SWAP[1]) return null;
  return paneLit(AW_END.pane!, undefined, swiftOut(progress(OPEN[0], OPEN[1], lt)));
}

export const scene: Scene = {
  id: S.id,
  start: S.start,
  end: S.end,
  draw(ctx, lt) {
    draw(ctx, lt);
  },
  lit,
  captions: () => CAPTIONS,
};
