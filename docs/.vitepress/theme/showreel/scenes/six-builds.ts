// Section 9, "Six builds at once". The benchmark's six real jobs (check,
// clippy and test --no-run, each with the default features and with
// --all-features --all-targets) start together on one machine. The
// another-worktree pane cracks into six tiles, which carry off its picture
// and round into job cards, and all six type their command at once, bobbing
// out of step like six drum loops. Before any compiler starts, the permit
// rail slams down between them and the loops snap onto one groove.
// Compilations fall onto the rail as amber sparks: each takes a permit slot
// while rustc runs, and once the pool is full the rest queue above it. Cache
// hits come straight out of Mr Boxington's monocle as green sparks and pass
// through the rail without stopping. One pair is singled out: `cargo check`
// and `cargo clippy` need the same `syn` compilation, so clippy's waits
// beside check's and takes its result, green, when it lands (`ran once`).
// Then the rail, full, tips over: its 32 permits slide down it into a bar,
// `scheduler on`, and the run with the scheduler off shoots out beside it.
// At the end the six cards fly together into the ci section's two runners as
// the frame closes in around Mr Boxington.
//
// The peaks are the contention benchmark's (facts.contention): each run's
// most compilers seen at once. Without them no bar or figure is drawn, the
// rail keeps cycling, and the first caption holds to b11.5. Every flight is
// planned once (plan()), so a frame is a pure function of time.

import { BEAT, type LitRect, PALETTE, type Scene, type SceneEnv, sec } from "../bible";
import { boxFrame, boxPoint, drawStrawberry, LOGO_FACE, type LogoFace } from "../box";
import { mix, rgba } from "../color";
import { type ContentionFact, medianOf, type ReelFacts } from "../facts";
import { glow, ring, roundedRect, shake } from "../fx";
import {
  AW_END,
  boxCam,
  boxMonocle,
  boxPose,
  chipHeight,
  type Curve,
  curveAt,
  drawFrame,
  drawLabel,
  drawMapBox,
  drawPane,
  drawRail,
  drawRunner,
  drawSlab,
  drawSpark,
  drawTag,
  KIT,
  lerpRect,
  MACHINE,
  type PaneState,
  type Pt,
  type RailState,
  railSlots,
  type Rect,
  SB_END,
} from "../map";
import { clamp, inQuad, lerp, progress, rng, smoothstep, spring, swiftIn, swiftInOut, swiftOut, TAU, wobble } from "../math";
import { View } from "../space";
import { glintAt } from "../sprite";
import { type Caption, drawText, font, layout, MONO } from "../type";

const S = sec("six-builds");

/** Local time of beat `n` of the section. */
const b = (n: number): number => n * BEAT;

// The beats, in section-local seconds. The score (score/six-builds.ts)
// places its cues on these.

/** The pane cracks into six job cards. */
export const T_SPLIT = 0.02;
/** The strawberry hops off: six new builds have started. */
export const T_HOP = b(0.25);
/** All six commands type together. */
export const T_TYPE = b(0.25);
export const T_TYPED = b(0.75);
/** The permit rail starts to fall, and lands. */
export const T_DROP = b(0.75);
export const T_SLAM = b(1);
/** The first compilations leave the jobs. */
export const T_FLOW = b(1.125);
/** check sends `syn`, it takes a permit, clippy sends the same `syn`, and the one compilation lands for both. */
export const T_SYN_CHECK = b(2.5);
export const T_SYN_DOCK = b(2.75);
export const T_SYN_CLIPPY = b(3);
export const T_SYN_WAIT = b(3.25);
export const T_SYN_DONE = b(3.75);
/** The rail tips into the two bars (with the contention peaks only). */
export const T_TIP = b(6.5);
export const T_POUR = b(6.75);
export const T_BARS = b(7);
/** A light runs along the bars in the hold. */
export const T_SHINE = b(9);
/** The cards fly together into ci's two runners. */
export const T_MERGE = b(11.5);

/**
 * One sentence, a word per 1/32 note from b0.875: the second line starts to
 * rise as the first lands. Landing the first line at b1.25 lets the pair
 * hold all seven words for 2.25 s even when it leaves at b6.25.
 */
const CAPTION_1: Caption["lines"] = [
  { in: 1.25, text: "Six builds share" },
  { in: 1.75, text: "one pool of permits." },
];

/**
 * The figure line needs the peaks; without them the first caption holds.
 * Its six words rise on the lower row from the tip, so the first caption
 * wipes a quarter beat before it and is gone from that row when they start.
 */
function captions(facts: ReelFacts | null): readonly Caption[] {
  const c = contention(facts);
  if (!c) return [{ out: 11.5, lines: CAPTION_1 }];
  return [
    { out: 6.25, lines: CAPTION_1 },
    { out: 11.75, lines: [{ in: 7.25, text: `${c.scheduled} compilers at peak, not ${c.unscheduled}.` }] },
  ];
}

/** The contention peaks, when the run backs them. */
const contention = (facts: ReelFacts | null): ContentionFact | null => facts?.contention ?? null;

// Layout, map px at HOME. Mr Boxington stays at MACHINE.box, where
// another-worktree leaves him and ci picks him up.

/** `one machine`: the whole frame, Mr Boxington and the six jobs inside it. */
const FRAME: Rect = { x: 36, y: 52, w: 1848, h: 684 };
const BOX = MACHINE.box;
const CHIP_H = chipHeight(40);
/** The job cards: a column per command, a row per feature set. */
const COLS = [
  { x: 696, w: 308, cmd: "cargo check" },
  { x: 1018, w: 332, cmd: "cargo clippy" },
  { x: 1364, w: 500, cmd: "cargo test --no-run" },
] as const;
const ROWS = [
  { y: 130, label: "default", mono: false },
  { y: 254, label: "--all-features --all-targets", mono: true },
] as const;
interface Job {
  row: number;
  col: number;
  rect: Rect;
  cmd: string;
}
const JOBS: readonly Job[] = ROWS.flatMap((r, row) =>
  COLS.map((c, col) => ({ row, col, cmd: c.cmd, rect: { x: c.x, y: r.y, w: c.w, h: CHIP_H } })),
);
const CHECK = 0;
const CLIPPY = 1;
/** Where a job's sparks leave and land: its card's bottom edge. */
const port = (j: number): Pt => ({ x: JOBS[j].rect.x + JOBS[j].rect.w / 2, y: JOBS[j].rect.y + CHIP_H });

/** The machine's one pool of compiler permits (the benchmark ran with 32). */
const RAIL: RailState = { x: 696, y: 420, w: 1168, slots: 32 };
const SLOTS = railSlots(RAIL);
/** Waiting compilations line up above the rail from its right end. */
const QUEUE_Y = RAIL.y - 54;
const QUEUE_GAP = 20;
const queueX = (rank: number): number => RAIL.x + RAIL.w - 18 - rank * QUEUE_GAP;
/** The permit `syn` gets, between check's card and clippy's. */
const SYN_SLOT = 8;

/** The bars, when the peaks are published: the longer one spans BAR_MAX. */
const BAR_X = RAIL.x;
const BAR_MAX = 1000;
const BAR_H = 40;
const ON_Y = RAIL.y;
const OFF_Y = RAIL.y + 120;

// Flight times, seconds.
/** A compilation from its job down to the rail. */
const FLY = 0.16;
/** From the queue into a free slot. */
const DOCK = 0.08;
/** A compiled output back up to its job. */
const RETURN = 0.2;
/** A hit from the monocle to its job. */
const HIT_FLY = 0.36;
/** The last compilations and hits leave the jobs (the rail keeps cycling without the peaks). */
const EMIT_END = b(10.5);
const HIT_END = b(10);

// The plan: every compilation's flight and permit, and every hit, decided
// once from a seeded generator, so each frame only looks them up.

interface Compile {
  job: number;
  /** Leaves its job. */
  emit: number;
  /** Reaches the rail, or the back of the queue. */
  arrive: number;
  /** Given a permit: it leaves the queue for its slot. */
  grant: number;
  /** rustc finishes; the permit frees. */
  release: number;
  slot: number;
  /** Where it leaves its card. */
  from: Pt;
  /** Its place in line when it arrives (0 is next). */
  rank: number;
}
interface Hit {
  job: number;
  emit: number;
}

/** The pair that shares one compilation, planned by hand: check's `syn` takes a slot, clippy's waits on it. */
const SYN: Compile = {
  job: CHECK,
  emit: T_SYN_CHECK,
  arrive: T_SYN_DOCK,
  grant: T_SYN_DOCK,
  release: T_SYN_DONE,
  slot: SYN_SLOT,
  from: port(CHECK),
  rank: 0,
};
/** Where clippy's `syn` waits: above the slot its twin holds. */
const SYN_WAIT: Pt = { x: SLOTS[SYN_SLOT].x, y: QUEUE_Y };

function plan(): { compiles: Compile[]; hits: Hit[] } {
  const r = rng(90210);
  // Every job has a wave of independent crates ready at once, then a
  // steady trickle as Cargo's graph unlocks more.
  const emits: { job: number; t: number; dx: number }[] = [];
  JOBS.forEach((_, job) => {
    let t = T_FLOW + job * 0.021;
    for (let k = 0; k < 8; k++) {
      emits.push({ job, t, dx: (r() - 0.5) * 0.6 });
      t += 0.028 + r() * 0.04;
    }
    t += 0.2 + r() * 0.15;
    while (t < EMIT_END) {
      emits.push({ job, t, dx: (r() - 0.5) * 0.6 });
      t += 0.2 + r() * 0.26;
    }
  });
  emits.sort((a, b2) => a.t - b2.t);
  // One pool, first come first served: a compilation takes the free permit
  // nearest where it lands, or waits in line for the next one to free.
  const free = SLOTS.map(() => 0);
  const busy = (s: number, a: number, z: number) => s === SYN_SLOT && a < SYN.release && z > SYN.grant - DOCK;
  const compiles: Compile[] = [];
  let lastGrant = 0;
  for (const e of emits) {
    const job = JOBS[e.job];
    const from = { x: port(e.job).x + e.dx * job.rect.w * 0.5, y: port(e.job).y };
    const arrive = e.t + FLY;
    const dur = 0.55 + r() * 1.05;
    let grant = Math.max(arrive, lastGrant);
    let slot = -1;
    for (let guard = 0; guard < 400 && slot < 0; guard++) {
      let best = Infinity;
      SLOTS.forEach((p, s) => {
        if (free[s] > grant + 1e-9 || busy(s, grant, grant + dur)) return;
        const d = Math.abs(p.x - from.x);
        if (d < best) {
          best = d;
          slot = s;
        }
      });
      if (slot < 0) {
        const next = Math.min(...free.filter((f, s) => f > grant && !busy(s, f, f + dur)));
        grant = Number.isFinite(next) ? next : grant + 0.05;
      }
    }
    lastGrant = grant;
    const release = grant + (grant > arrive ? DOCK : 0) + dur;
    free[slot] = release;
    compiles.push({ job: e.job, emit: e.t, arrive, grant, release, slot, from, rank: 0 });
  }
  // Each one's place in line when it arrives.
  for (const c of compiles) {
    c.rank = compiles.filter((o) => o !== c && o.arrive < c.arrive && o.grant > c.arrive).length;
  }
  // Hits start once other jobs have filled the store, and grow.
  const hits: Hit[] = [];
  let t = b(2.1);
  while (t < HIT_END) {
    hits.push({ job: Math.floor(r() * JOBS.length), emit: t });
    t += 0.1 + r() * (t < b(4) ? 0.34 : 0.22);
  }
  // clippy's `syn`, restored from check's compilation.
  hits.push({ job: CLIPPY, emit: T_SYN_DONE });
  hits.sort((a, b2) => a.emit - b2.emit);
  return { compiles, hits };
}

const PLAN = plan();
/**
 * Every permit taken before the rail tips, for the score's clicks: the
 * moment each compilation drops into its slot, and the slot's x.
 */
export const DOCKS: readonly { at: number; x: number }[] = [...PLAN.compiles, SYN]
  .map((c) => ({ at: c.grant + (c.grant > c.arrive ? DOCK : 0), x: SLOTS[c.slot].x }))
  .filter((d) => d.at < T_TIP)
  .sort((a, z) => a.at - z.at);
/** Every compilation's arrival, permit and release, and its slot, for the tests. */
export const SCHEDULE: readonly Readonly<Pick<Compile, "job" | "emit" | "arrive" | "grant" | "release" | "slot">>[] = [
  ...PLAN.compiles,
  SYN,
].map(({ job, emit, arrive, grant, release, slot }) => ({ job, emit, arrive, grant, release, slot }));
/** Every hit leaving the monocle, for the score's glints. */
export const HITS: readonly number[] = PLAN.hits.map((h) => h.emit).filter((t) => t < T_TIP);

/**
 * The six loops before the slam: each job's card hops on its own half-beat
 * cycle, out of step with the others, from the first keystroke to the slam.
 */
export const LOOP = BEAT / 2;
export const LOOP_PHASE: readonly number[] = [0, 0.37, 0.71, 0.18, 0.55, 0.9];
/** Where job j's loop is in its cycle at `t` (0 on its hit), before the slam. */
const loopAt = (j: number, t: number): number => ((((t - T_TYPE) / LOOP + LOOP_PHASE[j]) % 1) + 1) % 1;
/** Job j's loop hits between the first keystroke and the slam, local seconds. */
export function loopHits(j: number): number[] {
  const out: number[] = [];
  for (let k = 0; ; k++) {
    const t = T_TYPE + (k - LOOP_PHASE[j]) * LOOP;
    if (t >= T_SLAM - 1e-9) return out;
    if (t >= T_TYPE - 1e-9) out.push(t);
  }
}

// Drawing helpers.

/** A spark sitting still: a waiting compilation. */
function dot(ctx: CanvasRenderingContext2D, x: number, y: number, color: string, size: number, alpha = 1): void {
  if (alpha <= 0) return;
  ctx.save();
  ctx.globalAlpha *= alpha;
  glow(ctx, x, y, size * 5, color, 0.55);
  ctx.fillStyle = mix(color, "#ffffff", 0.45);
  ctx.beginPath();
  ctx.arc(x, y, size, 0, TAU);
  ctx.fill();
  ctx.restore();
}

/** A straight-ish drop from a to b that leaves heading down. */
const drop = (a: Pt, z: Pt): Curve => ({ a, c: { x: a.x, y: lerp(a.y, z.y, 0.7) }, b: z });
/** A short hop between two points, bowed up. */
const hop = (a: Pt, z: Pt): Curve => ({ a, c: { x: (a.x + z.x) / 2, y: Math.min(a.y, z.y) - 26 }, b: z });

/** How far along the queue compilation c is at `t`: its rank, easing forward as those ahead leave. */
function rankAt(c: Compile, t: number): number {
  let n = 0;
  for (const o of PLAN.compiles) {
    if (o === c || o.arrive >= c.arrive || o.grant <= c.arrive) continue;
    n += 1 - swiftOut(progress(o.grant, o.grant + 0.12, t));
  }
  return n;
}

/** How much of each permit is held at `t`, 0..1: lit while its compilation runs. */
function heldAt(t: number): number[] {
  const held = SLOTS.map(() => 0);
  for (const c of [...PLAN.compiles, SYN]) {
    const on = c.grant + (c.grant > c.arrive ? DOCK : 0);
    if (t < on || t >= c.release + 0.1) continue;
    held[c.slot] = Math.max(held[c.slot], Math.min(progress(on, on + 0.04, t), 1 - progress(c.release, c.release + 0.1, t)));
  }
  return held;
}

interface CardStyle {
  alpha: number;
  /** Characters typed. */
  typed: number;
  cursor: boolean;
  /** 0..1 lit green by a hit, amber by a compiled output landing, or amber for the syn pair. */
  green: number;
  amber: number;
  edge: number;
  scale: number;
  /** 0..1 how much of the command shows: it goes first as the cards fly together. */
  ink: number;
}

/** A job card: a pill with its command in mono at 40 px, a block cursor while it types. */
function drawCard(ctx: CanvasRenderingContext2D, r: Rect, cmd: string, s: CardStyle): void {
  if (s.alpha <= 0) return;
  ctx.save();
  ctx.globalAlpha *= s.alpha;
  ctx.translate(r.x + r.w / 2, r.y + r.h / 2);
  ctx.scale(s.scale, s.scale);
  if (s.green > 0) glow(ctx, 0, 0, r.w * 0.6, PALETTE.green, 0.4 * s.green);
  if (s.amber > 0) glow(ctx, 0, 0, r.w * 0.6, PALETTE.amber, 0.35 * s.amber);
  roundedRect(ctx, -r.w / 2, -r.h / 2, r.w, r.h, r.h / 2);
  const tint = s.green >= s.amber ? PALETTE.green : PALETTE.amber;
  ctx.fillStyle = mix(PALETTE.surface, tint, 0.18 * Math.max(s.green, s.amber, s.edge * 0.6));
  ctx.fill();
  ctx.lineWidth = 2.5;
  ctx.strokeStyle = mix(PALETTE.divider, s.edge > 0 ? PALETTE.amber : tint, Math.max(s.green, s.amber, s.edge));
  ctx.stroke();
  if (s.ink <= 0) {
    ctx.restore();
    return;
  }
  ctx.globalAlpha *= s.ink;
  const spec = font(40, 500, MONO);
  const shown = cmd.slice(0, Math.floor(s.typed));
  const w = drawText(ctx, shown, -r.w / 2 + 22, 14, { font: spec, fill: PALETTE.text1 }).width;
  if (s.cursor) {
    ctx.fillStyle = rgba(PALETTE.text1, 0.85);
    ctx.fillRect(-r.w / 2 + 22 + w + 3, -16, 22, 36);
  }
  ctx.restore();
}

/** The six tiles the pane cracks into, in its window: a 3 x 2 grid, in JOBS order. */
const WINDOW: Rect = { x: MACHINE.pane.x + 10, y: MACHINE.pane.y, w: MACHINE.pane.w - 20, h: MACHINE.pane.h - 62 + 6 };
const tile = (j: number): Rect => ({
  x: WINDOW.x + (JOBS[j].col * WINDOW.w) / 3,
  y: WINDOW.y + (JOBS[j].row * WINDOW.h) / 2,
  w: WINDOW.w / 3,
  h: WINDOW.h / 2,
});
/** When each tile leaves for its card, a 1/128 note apart, all gone before the first keystroke. */
export const TILES: readonly number[] = [0, 2, 4, 1, 3, 5].map((k) => T_SPLIT + 0.015 + (k * BEAT) / 32);
const tileAt = (j: number): number => TILES[j];

/** The picture the pane cracks: another-worktree's window, without its slab. */
const PICTURE: PaneState = { ...(AW_END.pane as PaneState), rect: WINDOW, slab: null };
/** How dark the picture has gone under the crack's seams, and how it fades on a tile (by its k). */
const CRACK_DIM = 0.35;
const fade = (k: number): number => CRACK_DIM + (1 - CRACK_DIM) * smoothstep(0, 0.45, k);

/**
 * A tile on its way from the pane to its card: its piece of the pane's
 * picture riding with it and fading as it rounds into a pill, its seam
 * amber at first.
 */
function drawTile(ctx: CanvasRenderingContext2D, j: number, r: Rect, k: number, alpha: number): void {
  if (alpha <= 0) return;
  const src = tile(j);
  const fill = mix(KIT.window, PALETTE.surface, clamp(k));
  const shape = () => roundedRect(ctx, r.x, r.y, r.w, r.h, lerp(6, r.h / 2, clamp(k)));
  ctx.save();
  ctx.globalAlpha *= alpha;
  shape();
  ctx.fillStyle = fill;
  ctx.fill();
  const dim = fade(k);
  if (dim < 1) {
    ctx.save();
    ctx.clip();
    ctx.translate(r.x, r.y);
    ctx.scale(r.w / src.w, r.h / src.h);
    ctx.translate(-src.x, -src.y);
    drawPane(ctx, PICTURE);
    ctx.restore();
    // drawPane leaves its own path behind.
    shape();
    ctx.fillStyle = rgba(fill, dim);
    ctx.fill();
  }
  ctx.lineWidth = 2.5;
  ctx.strokeStyle = mix(PALETTE.amberBright, PALETTE.divider, smoothstep(0, 0.6, k));
  ctx.stroke();
  ctx.restore();
}

/** The pane as another-worktree leaves it, cracking: its seams light amber and its picture dims, then the tiles leave. */
function drawCrack(ctx: CanvasRenderingContext2D, lt: number): void {
  const pane = AW_END.pane as PaneState;
  // The slab sinks into the floor as the window leaves it.
  const sink = swiftIn(progress(0.02, 0.26, lt));
  ctx.save();
  ctx.beginPath();
  ctx.rect(0, 0, 1920, BOX.y + 6);
  ctx.clip();
  drawSlab(ctx, pane.rect.x, BOX.y - 62 + sink * 70, pane.rect.w, pane.slab ?? "", { alpha: 1 - sink * 0.6 });
  ctx.restore();
  if (lt >= TILES[0]) return;
  // The window itself, then the seams lighting up across it.
  drawPane(ctx, PICTURE);
  const k = progress(T_SPLIT - 0.004, TILES[0], lt);
  if (k <= 0) return;
  ctx.save();
  // Every tile's own outline, as the tiles draw them when they leave.
  for (let j = 0; j < JOBS.length; j++) {
    const r = tile(j);
    roundedRect(ctx, r.x, r.y, r.w, r.h, 6);
    ctx.fillStyle = rgba(KIT.window, CRACK_DIM * k);
    ctx.fill();
    ctx.strokeStyle = rgba(PALETTE.amberBright, k);
    ctx.lineWidth = 2.5;
    ctx.stroke();
  }
  ctx.restore();
}

// Mr Boxington.

/** The pose's face at lt: where he looks, the glint from hits, a flinch at the slam, one blink. */
function face(lt: number, peaks: boolean): LogoFace {
  // Eyes on the jobs, then the rail, then the bars; home to the logo's by the end.
  const toJobs = swiftOut(progress(T_HOP, T_HOP + 0.25, lt));
  const toRail = swiftOut(progress(T_SLAM - 0.05, T_SLAM + 0.15, lt));
  const toBars = peaks ? swiftOut(progress(T_BARS, T_BARS + 0.3, lt)) : 0;
  const home = swiftInOut(progress(T_MERGE, S.len - 0.04, lt));
  let look: [number, number] = [lerp(0, 3, toJobs), lerp(0, -2, toJobs)];
  look = [look[0], lerp(look[1], 1, toRail)];
  look = [lerp(look[0], 2, toBars), lerp(look[1], 2, toBars)];
  look = [lerp(look[0], 0, home), lerp(look[1], 0, home)];
  // The mascot's sweep while hits leave the monocle (sprite.ts glintAt).
  let since: number | null = null;
  for (const h of PLAN.hits) if (h.emit <= lt && (peaks ? h.emit < T_TIP : true)) since = lt - h.emit;
  const band = lt >= T_MERGE ? null : glintAt(Math.floor((lt - T_FLOW) * 1000), since === null ? null : Math.floor(since * 1000));
  const blinkAt = b(9.5);
  const blink = lt >= blinkAt && lt < blinkAt + 0.16 ? 1 : 0;
  // The star on the rim for the shared result, restored from the store.
  const glint = lt >= T_SYN_DONE && lt < T_SYN_DONE + 0.6 ? Math.exp(-(lt - T_SYN_DONE) / 0.12) : 0;
  // A wince at the slam; with the peaks, a satisfied squint at the bars.
  const wince = smoothstep(T_SLAM, T_SLAM + 0.05, lt) * (1 - smoothstep(T_SLAM + 0.2, T_SLAM + 0.4, lt));
  const smug = peaks ? smoothstep(b(8), b(8.5), lt) * (1 - smoothstep(b(10.75), b(11.25), lt)) : 0;
  const twitch = lt < b(5) ? 0.22 * wobble(lt, T_SLAM, 5, 7) + 0.18 * wobble(lt, T_SYN_DONE, 5, 8) : 0;
  return {
    ...LOGO_FACE,
    look,
    sweep: band === null ? null : (band + 0.5) / 4,
    glint,
    eyelid: 0.7 * wince + 0.45 * smug,
    twitch,
    blink,
    strawberry: lt < T_HOP ? 1 : 0,
  };
}

/** The strawberry hops off his lid as the six builds start, and is gone. */
function drawBerryHop(ctx: CanvasRenderingContext2D, lt: number): void {
  const p = progress(T_HOP, T_HOP + 0.42, lt);
  if (lt < T_HOP || p >= 1) return;
  // Its seat, as box.ts draws it: lid local (-0.42, 0), upright.
  const view = new View(boxCam(BOX));
  const f = boxFrame(boxPose({ tape: 1 }));
  const seat = view.project(boxPoint(f, -0.42, 1, 0));
  const width = 0.22 * view.cam.scale * seat.f;
  // Up and back over his left shoulder, spinning, shrinking away.
  const x = seat.x - 150 * p;
  const y = seat.y - 190 * p + 260 * p * p;
  const k = 1 - smoothstep(0.55, 1, p);
  drawStrawberry(ctx, x, y, width * (1 - 0.35 * p) * k, -2.4 * p);
}

// The bars.

/**
 * A peak's bar, ruled into one cell per compiler at `unit` px each (the
 * scheduler-on bar is the rail's 32 permits, packed), with a light running
 * along it at `shine` (0..1, none outside).
 */
function drawBar(ctx: CanvasRenderingContext2D, x: number, y: number, w: number, unit: number, color: string, alpha: number, shine: number): void {
  if (alpha <= 0 || w <= 0) return;
  ctx.save();
  ctx.globalAlpha *= alpha;
  roundedRect(ctx, x, y - BAR_H / 2, w, BAR_H, 8);
  ctx.fillStyle = color;
  ctx.fill();
  ctx.save();
  ctx.clip();
  ctx.fillStyle = rgba("#ffffff", 0.12);
  ctx.fillRect(x + 4, y - BAR_H / 2 + 4, Math.max(0, w - 8), 5);
  ctx.fillStyle = rgba(PALETTE.ink, 0.28);
  for (let cx = x + unit; cx < x + w - 1; cx += unit) ctx.fillRect(cx - 0.75, y - BAR_H / 2 + 10, 1.5, BAR_H - 14);
  if (shine > 0 && shine < 1) {
    const sx = lerp(x - 80, x + BAR_MAX + 80, shine);
    const g = ctx.createLinearGradient(sx - 70, 0, sx + 70, 0);
    g.addColorStop(0, rgba("#ffffff", 0));
    g.addColorStop(0.5, rgba("#fff4dc", 0.45));
    g.addColorStop(1, rgba("#ffffff", 0));
    ctx.fillStyle = g;
    ctx.fillRect(sx - 70, y - BAR_H / 2, 140, BAR_H);
  }
  ctx.restore();
  ctx.restore();
}

/** A word run rising in: a label or a figure landing at `at`. */
function riseIn(ctx: CanvasRenderingContext2D, text: string, x: number, y: number, spec: string, fill: string, t: number, at: number, out: number): void {
  const p = progress(at, at + 0.18, t);
  const q = progress(out, out + 0.12, t);
  if (p <= 0 || q >= 1) return;
  ctx.save();
  ctx.globalAlpha *= clamp(p / 0.6) * (1 - q);
  drawText(ctx, text, x, y + 18 * (1 - swiftOut(p)) + 14 * q, { font: spec, fill });
  ctx.restore();
}

// The frame.

/** The frame's label: `one machine`, retyped `your machine` as it closes in on him at the end. */
function frameLabel(lt: number): string {
  const steps = Math.floor(progress(T_MERGE, T_MERGE + b(0.35), lt) * 7 + 1e-9);
  if (steps <= 3) return `${"one".slice(0, 3 - steps)} machine`.trimStart();
  return `${"your".slice(0, steps - 3)} machine`;
}

function draw(ctx: CanvasRenderingContext2D, lt: number, env: SceneEnv): void {
  const peaks = contention(env.facts);
  const t = lt;
  // How far into the finish: the cards to the runners, the frame to Mr Boxington's.
  const merge = swiftInOut(progress(T_MERGE + 0.04, S.len - 1 / 60, t));
  // The chart, or the cycling rail, clears first and fast, so the cards fly on their own.
  const leave = swiftOut(progress(T_MERGE, T_MERGE + 0.14, t));
  const tipped = peaks ? progress(T_TIP, T_TIP + 0.08, t) : 0;

  ctx.save();
  const [sx, sy] = shake(t, T_SLAM, 9, 0.09);
  ctx.translate(sx, sy);

  // `one machine`, drawn on around everything, then closing in.
  const frame = lerpRect(FRAME, SB_END.frame?.rect ?? FRAME, merge);
  drawFrame(ctx, { rect: frame, label: frameLabel(t), draw: swiftOut(progress(0.05, b(1.5), t)) });

  if (t < 0.3) drawCrack(ctx, t);

  // Mr Boxington: taped, rosy; the strawberry goes as the builds start.
  const pose = boxPose({ tape: 1, face: face(t, peaks !== null) });
  drawMapBox(ctx, { spot: BOX, pose });
  drawBerryHop(ctx, t);

  // The rail slams in, cycles, and (with the peaks) tips into the bars.
  const railIn = t >= T_DROP;
  const dropBy = t < T_SLAM ? 640 * (1 - inQuad(progress(T_DROP, T_SLAM, t))) : 16 * Math.max(0, wobble(t, T_SLAM, 5, 12));
  const railGone = peaks ? progress(T_BARS + 0.14, T_BARS + 0.2, t) : leave;
  // With the peaks the pool is full as it tips: every permit lights.
  const full = peaks ? smoothstep(T_TIP - 0.15, T_TIP, t) : 0;
  const held = heldAt(t).map((h) => Math.max(h, full));
  if (railIn && railGone < 1) {
    drawRailTipping(ctx, t, peaks, held, dropBy, 1 - railGone);
    if (t >= T_SLAM && t < T_SLAM + 0.5) {
      const p = progress(T_SLAM, T_SLAM + 0.5, t);
      ring(ctx, RAIL.x + 20, RAIL.y, 90, p, PALETTE.amberBright, 5);
      ring(ctx, RAIL.x + RAIL.w - 20, RAIL.y, 90, p, PALETTE.amberBright, 5);
      glow(ctx, RAIL.x + RAIL.w / 2, RAIL.y, RAIL.w * 0.55, PALETTE.amberBright, 0.5 * (1 - p) ** 2);
    }
  }

  // The flights, under the cards, so the back row's leave from behind the front row.
  const sparks = peaks ? 1 - tipped : 1 - leave;
  if (sparks > 0) drawFlights(ctx, t, sparks);

  // The job cards: tiles out of the pane, typing, lit by what lands on them.
  drawCards(ctx, t, peaks !== null, merge);

  // The pair sharing `syn`, drawn over the cards.
  if (sparks > 0) drawSyn(ctx, t, env.facts, sparks);

  // Notes under the rail while it cycles.
  const notesOut = peaks ? T_TIP - 0.12 : T_MERGE;
  riseIn(ctx, "cache hits never wait", RAIL.x, RAIL.y + 128, font(40, 600), PALETTE.green, t, b(2.25), notesOut);
  const right = RAIL.x + RAIL.w;
  const note1 = "permits: one pool for every build";
  const note2 = "on the machine, weighted by memory";
  const spec40 = font(40, 600);
  riseIn(ctx, note1, right - layout(ctx, note1, spec40).width, RAIL.y + 128, spec40, PALETTE.text2, t, b(1.5), notesOut);
  riseIn(ctx, note2, right - layout(ctx, note2, spec40).width, RAIL.y + 176, spec40, PALETTE.text2, t, b(1.625), notesOut);

  if (peaks && env.facts) drawBars(ctx, t, peaks, env.facts);

  // The finish: the cards fly into the runners, which fade up in their place.
  if (merge > 0) {
    for (const r of SB_END.runners ?? []) drawRunner(ctx, { ...r, alpha: smoothstep(0.35, 1, merge) });
  }
  ctx.restore();
}

/** The rail: dropped in, lit by what it holds, and with the peaks tipping so its permits pour into the bar. */
function drawRailTipping(
  ctx: CanvasRenderingContext2D,
  t: number,
  peaks: ContentionFact | null,
  held: readonly number[],
  dropBy: number,
  alpha: number,
): void {
  if (!peaks || t < T_TIP) {
    drawRail(ctx, { ...RAIL, held: (i) => held[i], drop: dropBy, alpha });
    return;
  }
  // Full at the tip; a dip of its right end, then it tips the other way and
  // the permits slide down to its left end, packed into the scheduler-on bar.
  const onW = (BAR_MAX * peaks.scheduled) / peaks.unscheduled;
  const dip = Math.sin(Math.PI * progress(T_TIP, T_POUR, t)) * 0.035;
  const lift = -0.075 * swiftOut(progress(T_POUR, T_POUR + 0.1, t)) * (1 - swiftInOut(progress(T_BARS - 0.04, T_BARS + 0.1, t)));
  const angle = dip + lift;
  ctx.save();
  ctx.translate(RAIL.x, RAIL.y);
  ctx.rotate(angle);
  ctx.translate(-RAIL.x, -RAIL.y);
  const track = 1 - progress(T_BARS - 0.05, T_BARS + 0.1, t);
  drawRail(ctx, { ...RAIL, held: () => 0, alpha: alpha * track });
  const pitch = (RAIL.w - 24) / RAIL.slots;
  const size = Math.min(pitch - 5, 24);
  SLOTS.forEach((p, i) => {
    const slide = inQuad(progress(T_POUR + 0.02 + (i / SLOTS.length) * 0.05, T_BARS + 0.02, t));
    const packed = BAR_X + (onW * (i + 0.5)) / SLOTS.length;
    const x = lerp(p.x, packed, slide);
    const s = lerp(size, Math.max(onW / SLOTS.length, 4), slide);
    ctx.fillStyle = PALETTE.amber;
    roundedRect(ctx, x - s / 2, RAIL.y - size / 2, s, size, Math.min(5, s / 2));
    ctx.fill();
  });
  glow(ctx, BAR_X + onW / 2, RAIL.y, 160, PALETTE.amber, 0.3 * (1 - track));
  ctx.restore();
}

/** Every compilation and hit in flight, waiting, or coming home, at `t`. */
function drawFlights(ctx: CanvasRenderingContext2D, t: number, alpha: number): void {
  ctx.save();
  ctx.globalAlpha *= alpha;
  const amber = PALETTE.amberBright;
  for (const c of PLAN.compiles) {
    if (t < c.emit || t > c.release + RETURN) continue;
    const slot = { x: SLOTS[c.slot].x, y: RAIL.y };
    const waits = c.grant > c.arrive + 1e-6;
    if (t < c.arrive) {
      // Falling from its card to the rail or the back of the line.
      const to = waits ? { x: queueX(c.rank), y: QUEUE_Y } : slot;
      drawSpark(ctx, drop(c.from, to), inQuad(progress(c.emit, c.arrive, t)), { color: amber, size: 6, trail: 0.35 });
    } else if (t < c.grant) {
      // In line, shuffling forward as permits free.
      const x = queueX(rankAt(c, t));
      dot(ctx, x, QUEUE_Y + Math.sin((t + c.emit) * 9) * 2, amber, 5.5, 0.95);
    } else if (waits && t < c.grant + DOCK) {
      const from = { x: queueX(rankAt(c, c.grant)), y: QUEUE_Y };
      drawSpark(ctx, hop(from, slot), swiftOut(progress(c.grant, c.grant + DOCK, t)), { color: amber, size: 6, trail: 0.3 });
    } else if (t >= c.release) {
      // Compiled: the output goes back up to its job, faint.
      const home = port(c.job);
      drawSpark(ctx, hop(slot, { x: home.x + (c.from.x - home.x) * 0.5, y: home.y }), swiftOut(progress(c.release, c.release + RETURN, t)), {
        color: PALETTE.amber,
        size: 3.5,
        trail: 0.25,
        alpha: 0.7,
      });
    }
  }
  // Hits: green, straight out of the monocle, through the rail without stopping.
  const mono = boxMonocle(BOX, boxPose({ tape: 1 }));
  for (const h of PLAN.hits) {
    if (h.emit === T_SYN_DONE) continue;
    if (t < h.emit || t > h.emit + HIT_FLY) continue;
    const to = port(h.job);
    const k: Curve = { a: { x: mono.x + mono.r * 0.8, y: mono.y - 8 }, c: { x: to.x, y: RAIL.y + 100 }, b: to };
    const p = progress(h.emit, h.emit + HIT_FLY, t);
    drawSpark(ctx, k, 1 - (1 - p) ** 2, { color: PALETTE.green, size: 6.5, trail: 0.3 });
  }
  ctx.restore();
}

/** check's `syn` in its slot, clippy's waiting over it, and the result going to both. */
function drawSyn(ctx: CanvasRenderingContext2D, t: number, facts: ReelFacts | null, alpha: number): void {
  if (t < T_SYN_CHECK || t > T_SYN_DONE + b(5)) return;
  ctx.save();
  ctx.globalAlpha *= alpha;
  const hk = facts?.subject === "hk";
  const slot = { x: SLOTS[SYN_SLOT].x, y: RAIL.y };
  const tagOut = progress(T_SYN_DONE, T_SYN_DONE + 0.1, t);
  const tag = (p: Pt, a = 1) => {
    if (hk && a > 0) drawTag(ctx, p.x + 10, p.y - 14, "syn", { size: 30, tone: "amber", rot: -0.32, alpha: a * (1 - tagOut) });
  };
  // check's, down into its permit.
  if (t < T_SYN_DOCK) {
    const k = drop(port(CHECK), slot);
    const u = inQuad(progress(T_SYN_CHECK, T_SYN_DOCK, t));
    drawSpark(ctx, k, u, { color: PALETTE.amberBright, size: 9, trail: 0.4 });
    tag(curveAt(k, u));
  } else if (t < T_SYN_DONE) {
    // Compiling: the slot burns brighter than the rest, its tag hung under the rail.
    glow(ctx, slot.x, slot.y, 70, PALETTE.amberBright, 0.45 + 0.15 * Math.sin(t * 30));
    const swing = 0.25 * wobble(t, T_SYN_DOCK, 2.5, 5);
    if (hk) drawTag(ctx, slot.x, slot.y + 24, "syn", { size: 30, tone: "amber", rot: 0.35 + swing, alpha: 1 - tagOut });
  }
  // clippy's, the same compilation: it waits over the slot, tied to it.
  if (t >= T_SYN_CLIPPY && t < T_SYN_DONE) {
    const k = drop(port(CLIPPY), SYN_WAIT);
    const u = inQuad(progress(T_SYN_CLIPPY, T_SYN_WAIT, t));
    const p = t < T_SYN_WAIT ? curveAt(k, u) : { x: SYN_WAIT.x, y: SYN_WAIT.y + Math.sin(t * 10) * 3 };
    if (t < T_SYN_WAIT) drawSpark(ctx, k, u, { color: PALETTE.amberBright, size: 9, trail: 0.4 });
    else {
      ctx.save();
      ctx.strokeStyle = rgba(PALETTE.amberBright, 0.7);
      ctx.lineWidth = 3;
      ctx.setLineDash([6, 6]);
      ctx.lineDashOffset = -t * 60;
      ctx.beginPath();
      ctx.moveTo(p.x, p.y + 10);
      ctx.lineTo(slot.x, slot.y - 16);
      ctx.stroke();
      ctx.restore();
      dot(ctx, p.x, p.y, PALETTE.amberBright, 8);
    }
    tag(p, t < T_SYN_WAIT ? 1 : 1);
  }
  // Done once: compiled for check, restored for clippy.
  if (t >= T_SYN_DONE) {
    const p = progress(T_SYN_DONE, T_SYN_DONE + 0.4, t);
    ring(ctx, slot.x, slot.y, 56, p, PALETTE.amberBright, 5);
    const u = swiftOut(progress(T_SYN_DONE, T_SYN_DONE + b(0.5), t));
    if (u < 1) {
      drawSpark(ctx, hop(slot, port(CHECK)), u, { color: PALETTE.amberBright, size: 8, trail: 0.35 });
      drawSpark(ctx, hop(SYN_WAIT, port(CLIPPY)), u, { color: PALETTE.green, size: 8, trail: 0.35 });
    }
    // `ran once`, where clippy's waited.
    const s = spring(t - T_SYN_DONE - 0.04, 4.5, 0.5);
    const out = progress(T_TIP - 0.12, T_TIP, t);
    if (s > 0 && out < 1) {
      ctx.save();
      ctx.globalAlpha *= 1 - out;
      ctx.translate(slot.x, QUEUE_Y + 12);
      ctx.scale(s, s);
      drawText(ctx, "ran once", 0, 0, { font: font(56, 600), fill: PALETTE.paper, align: "center", tracking: -1.1 });
      ctx.restore();
    }
  }
  ctx.restore();
}

/** The six job cards at `t`: out of the pane, typing, then lit by what lands on them; at the end, into the runners. */
function drawCards(ctx: CanvasRenderingContext2D, t: number, peaks: boolean, merge: number): void {
  // Before the rail lands each hops to its own loop; after, all on the beat.
  const runners = SB_END.runners ?? [];
  // With the peaks the cards step back for the chart, and forward again to leave.
  const dim = peaks ? 0.6 * smoothstep(T_TIP, T_BARS, t) * (1 - smoothstep(T_MERGE, T_MERGE + 0.1, t)) : 0;
  if (t < TILES[0]) return;
  JOBS.forEach((job, j) => {
    // Each tile sits in the pane until it leaves, then springs to its card.
    const at = tileAt(j);
    const k = t < at ? 0 : spring(t - at, 3.6, 0.62);
    let r = lerpRect(tile(j), job.rect, k);
    if (merge > 0) r = lerpRect(r, runners[job.row].rect, merge);
    const asCard = smoothstep(0.35, 0.8, k);
    if (asCard < 1) drawTile(ctx, j, r, k, 1);
    if (asCard <= 0) return;
    // The loops: out of step until the slam, then together.
    const loop = t < T_SLAM ? loopAt(j, t) : ((t - T_SLAM) / BEAT) % 1;
    const bob = t < T_TYPE || t > T_TIP ? 0 : Math.exp(-loop * 9) * (t < T_SLAM ? 12 : 5);
    r = { ...r, y: r.y - bob };
    let green = 0;
    let amber = 0;
    // With the peaks the jobs fade back once the rail tips: nothing lands on them after.
    const quiet = peaks ? T_TIP : Infinity;
    for (const h of PLAN.hits) {
      const land = h.emit + HIT_FLY;
      if (h.job === j && h.emit < quiet && t >= land && t < land + 0.3) green = Math.max(green, 1 - (t - land) / 0.3);
    }
    if (j === CLIPPY && t >= T_SYN_DONE + b(0.5)) green = Math.max(green, 1 - progress(T_SYN_DONE + b(0.5), T_SYN_DONE + b(0.5) + 0.4, t));
    for (const c of PLAN.compiles) {
      const land = c.release + RETURN;
      if (c.job === j && c.release < quiet && t >= land && t < land + 0.2) amber = Math.max(amber, 0.6 * (1 - (t - land) / 0.2));
    }
    const pair = (j === CHECK || j === CLIPPY) && t >= T_SYN_CHECK - 0.1 ? 1 - progress(T_SYN_DONE + 0.6, T_SYN_DONE + 0.9, t) : 0;
    drawCard(ctx, r, job.cmd, {
      alpha: asCard * (1 - dim) * (1 - smoothstep(0.3, 0.9, merge)),
      typed: job.cmd.length * progress(T_TYPE, T_TYPED, t),
      cursor: t >= T_TYPE && t < T_SLAM && loopAt(j, t) < 0.5,
      green,
      amber,
      edge: clamp(pair),
      scale: 1,
      ink: 1 - smoothstep(0, 0.22, merge),
    });
  });
  // The rows' labels, over their cards.
  const labelA = smoothstep(T_TYPE, T_TYPED, t) * (1 - (peaks ? 0.6 * smoothstep(T_TIP, T_BARS, t) : 0)) * (1 - smoothstep(0, 0.4, merge));
  ROWS.forEach((row) => {
    drawLabel(ctx, { text: row.label, x: COLS[0].x + 4, y: row.y - 14, size: 40, mono: row.mono, fill: PALETTE.text3, alpha: labelA });
  });
}

/** The two peaks, scheduler on and off, from the rail's pour to the finish. */
function drawBars(ctx: CanvasRenderingContext2D, t: number, c: ContentionFact, facts: ReelFacts): void {
  if (t < T_BARS - 0.02) return;
  // At the finish the bars run back into their origin as they fade.
  const out = swiftOut(progress(T_MERGE, T_MERGE + 0.14, t));
  const a = 1 - out;
  if (a <= 0) return;
  const onW = ((BAR_MAX * c.scheduled) / c.unscheduled) * (1 - 0.8 * out);
  const offW = BAR_MAX * spring(t - T_BARS - 0.06, 2.6, 0.62) * (1 - 0.8 * out);
  ctx.save();
  ctx.globalAlpha *= a;
  // scheduler on: the rail's permits, packed.
  // One cell per compiler on both; a light runs along them halfway through the hold.
  const unit = (BAR_MAX / c.unscheduled) * (1 - 0.8 * out);
  const shine = progress(T_SHINE, T_SHINE + b(1.25), t);
  drawBar(ctx, BAR_X, ON_Y, onW, unit, PALETTE.amber, progress(T_BARS + 0.08, T_BARS + 0.14, t), shine);
  drawBar(ctx, BAR_X, OFF_Y, offW, unit, PALETTE.amberDeep, 1, progress(T_SHINE + b(0.25), T_SHINE + b(1.5), t));
  if (offW > 0) glow(ctx, BAR_X + offW, OFF_Y, 90, PALETTE.amberBright, 0.5 * (1 - progress(T_BARS + 0.1, T_BARS + 0.5, t)));
  const figure = font(56, 700, MONO);
  riseIn(ctx, String(c.scheduled), BAR_X + onW + 22, ON_Y + 20, figure, PALETTE.paper, t, T_BARS + 0.06, Infinity);
  riseIn(ctx, String(c.unscheduled), BAR_X + BAR_MAX * (1 - 0.8 * out) + 22, OFF_Y + 20, figure, PALETTE.paper, t, T_BARS + 0.36, Infinity);
  const label = font(40, 600);
  riseIn(ctx, "scheduler on", BAR_X, ON_Y - 38, label, PALETTE.text2, t, T_BARS + 0.1, Infinity);
  riseIn(ctx, "scheduler off", BAR_X, OFF_Y - 38, label, PALETTE.text2, t, T_BARS + 0.22, Infinity);
  const source = [
    facts.subject ? `${facts.subject} benchmark` : "benchmark",
    "six CI jobs on one runner",
    medianOf(c.trials),
  ]
    .filter(Boolean)
    .join(" · ");
  riseIn(ctx, source, BAR_X, OFF_Y + 104, label, PALETTE.text3, t, T_BARS + 0.6, Infinity);
  ctx.restore();
}

/** When the last tile's piece of the picture has faded out on it (fade, at k 0.45). */
const PICTURE_GONE = Math.max(...TILES) + 0.06;

/** The pane's window, lit as another-worktree leaves it, until its tiles have carried the picture off. */
function lit(lt: number): LitRect | null {
  if (lt >= PICTURE_GONE) return null;
  return { ...WINDOW, alpha: 1 - smoothstep(T_SPLIT, PICTURE_GONE, lt) };
}

export const scene: Scene = {
  id: S.id,
  start: S.start,
  end: S.end,
  draw(ctx, lt, env) {
    draw(ctx, lt, env);
  },
  captions,
  lit,
};
