// Mr Boxington as the terminal draws him beside a build: a port of
// crates/mbx/src/cli/mascot.rs, layer for layer and rule for rule, so the
// reel's pixel build view shows the real mascot. test/sprite.test.ts reads
// mascot.rs and its golden sprites and fails if this file drifts from them.
//
// The sprite is an 18x18 pixel canvas, shown in a terminal as 18x9 cells of
// half blocks. A frame is `sprite(poseAt(inputs))`. Every rule is integer
// arithmetic on whole milliseconds and counts, and the only state carried
// between frames is the lid the previous frame showed (lidAt folds it for the
// reel, whose frames are pure functions of time).

export const WIDTH = 18;
export const HEIGHT = 9;
/** Canvas pixels per side. Cell row `r` shows pixel rows `2r` and `2r + 1`. */
export const SIZE = 18;

export type Rgb = readonly [number, number, number];
/** Palette keys, one per pixel, `.` where the terminal shows through. */
export type Canvas = string[][];

/** Each palette key's colour. Any other key is a transparent pixel. */
export const PALETTE: Readonly<Record<string, Rgb>> = {
  A: [230, 173, 84], // amber: the front panel
  B: [242, 196, 121], // bright: the lid's top face
  D: [207, 143, 53], // deep: the lid's front edge, the base, the open top's rim ends
  S: [189, 125, 35], // shade: the monocle chain
  K: [32, 25, 14], // ink: eyelid lines, pupils, ring, mustache
  W: [245, 234, 214], // paper: the bare eye
  L: [214, 232, 232], // lens
  G: [255, 255, 255], // glint
  T: [247, 228, 184], // packing tape
  H: [84, 56, 22], // the inside of the box, under a raised lid
  "1": [230, 150, 92], // cheeks, hit share below 1/3
  "2": [229, 134, 99], // cheeks, hit share below 2/3
  "3": [228, 122, 104], // cheeks, hit share 2/3 and up (the logo's rose)
  R: [214, 62, 70], // strawberry
  Y: [247, 216, 138], // strawberry seed
  V: [106, 168, 79], // strawberry leaf
};

/** A palette key's colour, or null for a transparent pixel. */
export function color(key: string): Rgb | null {
  return Object.hasOwn(PALETTE, key) ? PALETTE[key] : null;
}

/**
 * Pixel art stamped onto the canvas: `rows[j]`'s `i`th key lands on pixel
 * `(x + i, y + j)`, and `.` leaves the pixel alone.
 */
export interface Layer {
  x: number;
  y: number;
  rows: readonly string[];
}

const at = (x: number, y: number, rows: readonly string[]): Layer => ({ x, y, rows });

/** The layer's opaque pixels as `[x, y, key]`, clipped to the canvas. */
export function pixels(layer: Layer): [number, number, string][] {
  const out: [number, number, string][] = [];
  layer.rows.forEach((row, j) => {
    for (let i = 0; i < row.length; i++) {
      const [x, y] = [layer.x + i, layer.y + j];
      if (row[i] !== "." && x < SIZE && y < SIZE) out.push([x, y, row[i]]);
    }
  });
  return out;
}

export const BODY = at(1, 6, [
  "AAAAAAAAAAAAAAAA",
  "AAAAAAAAAAAAAAAA",
  "AAAAAAAAAAAAAAAA",
  "AAAAAAAAAAAAAAAA",
  "AAAAAAAAAAAAAAAA",
  "AAAAAAAAAAAAAAAA",
  "AAAAAAAAAAAAAAAA",
  "AAAAAAAAAAAAAAAA",
  "AAAAAAAAAAAAAAAA",
  "AAAAAAAAAAAAAAAA",
  "AAAAAAAAAAAAAAAA",
  "DDDDDDDDDDDDDDDD",
]);
/** The open top of the box, drawn whenever the lid is not shut on it. */
export const OPENING = at(1, 5, ["DHHHHHHHHHHHHHHD"]);
/**
 * The lid, shut. While the build runs, the left end stays hinged to the box
 * and the right end rises by `lid` pixels (stampLid). Its front edge
 * overhangs the box.
 */
export const LID = at(0, 4, [
  "..BBBBBBBBBBBBBB..",
  "DDDDDDDDDDDDDDDDDD",
]);
/** Success only: tape over the lid and a tab onto the front. */
export const TAPE = at(8, 4, [
  "TT",
  "TT",
  "TT",
]);
/**
 * Failure only: the lid knocked askew, shoved left with its left end down
 * off the edge of the box and its right end up over the open top.
 */
export const KNOCKED_LID = at(0, 3, [
  "..........BBBB....",
  "....BBBBBBDDDDDD..",
  "BBBBDDDDDD........",
  "DDDD..............",
]);

/**
 * The bare eye: the white, then the pupil, then the eyelid line over both.
 * The eyelid is exactly as wide as the white.
 */
export const EYE_WHITE = at(3, 9, [
  "WWWW",
  "WWWW",
  ".WW.",
]);
/** The skeptic's flat eyelid rests on the pupil, as in the logo. */
export const EYELID = at(3, 8, ["KKKK"]);
/** Testing drops the eyelid a row, onto the white's top row. */
export const EYELID_SQUINT = at(3, 9, ["KKKK"]);
/**
 * A blink: this line and no white or pupil. It shares a row with
 * MONOCLE_SHUT, so the two shut eyes are one level line.
 */
export const EYE_SHUT = at(3, 9, ["KKKK"]);
/** Failure only: both eyes bare and glum, lids sloping down to the outer corners. */
export const GLUM_LEFT = at(2, 8, [
  "...KK",
  ".KKWW",
  ".WWWW",
  ".WKKW",
  "..KK.",
]);
export const GLUM_RIGHT = at(10, 8, [
  "KK...",
  "WWKK.",
  "WWWW.",
  "WKKW.",
  ".KK..",
]);
export const PUPIL: readonly string[] = [
  "KK",
  "KK",
];
export const PUPIL_DOWN: readonly string[] = ["KK"];

export const RING = at(8, 6, [
  "..KKKK..",
  ".KLLLLK.",
  "KLLLLLLK",
  "KLLLLLLK",
  "KLLLLLLK",
  ".KLLLLK.",
  "..KKKK..",
]);
/**
 * The one highlight that is always on: inside the lens a pixel in from the
 * ring's top-left chamfer, with lens between it and the tape tab.
 */
export const PARKED_GLINT = at(10, 8, ["G"]);
/** While looking left, put the highlight across the lens from the pupil. */
export const PARKED_GLINT_RIGHT = at(13, 8, ["G"]);
/** The eye behind the monocle, blinking. */
export const MONOCLE_SHUT = at(9, 9, ["KKKKKK"]);
/**
 * Failure only: popped out and hanging by its dotted chain against the box's
 * right side, the whole ring on the amber so it reads on dark terminals too.
 */
export const DROPPED = at(13, 7, [
  "..S.",
  "....",
  "..S.",
  "....",
  "..S.",
  ".KK.",
  "KGLK",
  "KLLK",
  ".KK.",
]);

/**
 * Painted in the blush level's colour. Mirrored about the box's centre line,
 * each just outside a curled mustache tip.
 */
export const CHEEKS: readonly [Layer, Layer] = [at(1, 13, ["cc"]), at(15, 13, ["cc"])];

/**
 * A handlebar: a centre hump above two lobes whose outer ends curl up.
 * Twelve pixels wide, so it centres on the box.
 */
export const MUSTACHE = at(3, 14, [
  "K...KKKK...K",
  "KKKKK..KKKKK",
  ".KKK....KKK.",
]);
/** Failure only. Eleven wide: the dropped monocle hangs where a twelfth column would go. */
export const MUSTACHE_DROOP = at(3, 14, [
  "...KKKKK...",
  ".KKKK.KKKK.",
  "KK.......KK",
]);

/** Success with hits: the keepsake, a strawberry beside the tape. Its tip rests on the lid's top face. */
export const STRAWBERRY = at(4, 0, [
  "..V.",
  "VVVV",
  "RYRR",
  "RRYR",
  ".RR.",
]);

export const LID_MAX = 4;
/** The drawn lid's right edge descends at most this far per drawn frame. */
export const LID_STEP_PX = 1;
/** The Compiling list for [0, 4200), the progress bar for [4200, 5400), and you for [5400, 7000). */
export const GAZE_LOOP_MS = 7000;
export const GAZE_LIST_UNTIL_MS = 4200;
export const GAZE_BAR_UNTIL_MS = 5400;
/** One blink per window, starting `500 + hash32(window) % 2800` ms into it. */
export const BLINK_WINDOW_MS = 4300;
export const BLINK_EARLIEST_MS = 500;
export const BLINK_SPREAD_MS = 2800;
export const BLINK_MS = 160;
/** The glint's clock: band positions 0..3 show for 100 ms each, then the lens rests for 240 ms. */
export const GLINT_CYCLE_MS = 640;
export const GLINT_STEP_MS = 100;
export const GLINT_POSITIONS = 4;
export const GLINT_SWEEP_MS = GLINT_STEP_MS * GLINT_POSITIONS;
export const GLINT_REST_MS = GLINT_CYCLE_MS - GLINT_SWEEP_MS;

/** A 32-bit integer hash of a blink window's index. */
export function hash32(n: number): number {
  let v = (Math.imul(n, 2_654_435_761) + 12_345) >>> 0;
  v = (v ^ (v >>> 15)) >>> 0;
  v = Math.imul(v, 2_246_822_519) >>> 0;
  return (v ^ (v >>> 13)) >>> 0;
}

export function blinking(ms: number): boolean {
  // Truncating the window index is the hash's own wrapping.
  const start = BLINK_EARLIEST_MS + (hash32(Math.floor(ms / BLINK_WINDOW_MS) >>> 0) % BLINK_SPREAD_MS);
  const m = ms % BLINK_WINDOW_MS;
  return start <= m && m < start + BLINK_MS;
}

export type Eye = "skeptic" | "squint" | "shut";
/** Left toward the Compiling list, down toward the progress bar, or straight out at you. */
export type Gaze = "list" | "bar" | "you";

export function gazeAt(ms: number): Gaze {
  const cycle = ms % GAZE_LOOP_MS;
  if (cycle < GAZE_LIST_UNTIL_MS) return "list";
  if (cycle < GAZE_BAR_UNTIL_MS) return "bar";
  return "you";
}

/**
 * The lid's target: `ceil(4 * (1 - min(p / 0.9, 1)))` with `p = done / total`,
 * in integers.
 *
 * 4 px at the start, then 3, 2 and 1 from 22.5%, 45% and 67.5%, and shut
 * from 90%. An unknown or zero total holds the lid fully open. Exact for
 * counts below 2^49.
 */
export function lidOffset(done: number, total: number | null): number {
  if (total === null || total <= 0) return LID_MAX;
  const n = 9 * total;
  const d = Math.min(10 * Math.min(done, total), n);
  return Math.floor((LID_MAX * (n - d) + n - 1) / n);
}

/**
 * The lid to draw, given the lid the previous drawn frame showed: it descends
 * one pixel per drawn frame toward the target, and takes a higher target at
 * once.
 */
export function stepLid(shown: number, target: number): number {
  return target < shown ? shown - LID_STEP_PX : target;
}

/** Frames of the reel's build view per second of reel time, for lidAt. */
export const LID_FPS = 30;

/**
 * The lid shown on drawn frame `frame` (from 0), stepLid folded over every
 * frame's target from LID_MAX. The reel counts frames at LID_FPS of its own
 * time, so its 60 and 120 fps renders show the same lid.
 */
export function lidAt(frame: number, target: (frame: number) => number): number {
  let shown = LID_MAX;
  for (let i = 0; i <= frame; i++) shown = stepLid(shown, target(i));
  return shown;
}

/**
 * The lens band position 0..3, or null for a matte lens.
 *
 * The band is keyed off the clock, so a warm build sweeps continuously at a
 * steady pace. A cycle's sweep shows only when the last hit landed no
 * earlier than the rest just before it. The lens is lit only while
 * `sinceHitMs < 640`.
 */
export function glintAt(ms: number, sinceHitMs: number | null): number | null {
  if (sinceHitMs === null) return null;
  const phase = ms % GLINT_CYCLE_MS;
  if (phase >= GLINT_SWEEP_MS || sinceHitMs > phase + GLINT_REST_MS) return null;
  return Math.floor(phase / GLINT_STEP_MS);
}

/** 0 before the first hit, then 1 to 3 as `hits / (hits + misses)` crosses 1/3 and 2/3. */
export function cheekLevel(hits: number, misses: number): number {
  if (hits === 0) return 0;
  const share = Math.floor((3 * hits) / (hits + misses));
  return 1 + Math.min(share, 2);
}

/** Everything a frame depends on, as whole numbers. */
export interface Inputs {
  /** Milliseconds since the build started. */
  ms: number;
  /** Milliseconds since the hit count last rose; null before the first hit. */
  sinceHitMs: number | null;
  /** Cargo's units finished, never test counts. */
  done: number;
  /** Cargo's units expected; null or 0 while unknown. */
  total: number | null;
  /** The lid the previous drawn frame showed; LID_MAX before the first. */
  lidShown: number;
  hits: number;
  misses: number;
  testing: boolean;
  /** null while the outcome is unsettled. */
  ok: boolean | null;
}

/**
 * The bare eye's pupil and the monocle's for a gaze. The downward glance uses
 * a shorter pupil so lens glass remains between it and the ring.
 */
export const PUPILS: Readonly<Record<Gaze, readonly [Layer, Layer]>> = {
  list: [at(3, 9, PUPIL), at(10, 9, PUPIL)],
  bar: [at(3, 10, PUPIL_DOWN), at(10, 10, PUPIL_DOWN)],
  you: [at(4, 9, PUPIL), at(11, 9, PUPIL)],
};

/** What a frame shows. A finished failure is the default with `failed` set. */
export interface Pose {
  /** Pixels the lid's right edge rises above its closed position, 0 to LID_MAX. */
  lid: number;
  /** Tape over the shut lid, on success only. */
  taped: boolean;
  /** The knocked lid, glum eyes, drooping mustache and dropped monocle. */
  failed: boolean;
  eye: Eye;
  gaze: Gaze;
  /** The lens band position 0..3, or null for a matte lens. */
  glint: number | null;
  /** The blush level, 0 to 3. */
  cheeks: number;
  strawberry: boolean;
}

export const DEFAULT_POSE: Readonly<Pose> = {
  lid: 0,
  taped: false,
  failed: false,
  eye: "skeptic",
  gaze: "you",
  glint: null,
  cheeks: 0,
  strawberry: false,
};

/**
 * Finished frames ignore the clock and the previous lid. While running, only
 * the eyes and the glint follow the clock.
 */
export function poseAt(inputs: Inputs): Pose {
  const cheeks = cheekLevel(inputs.hits, inputs.misses);
  if (inputs.ok === false) return { ...DEFAULT_POSE, failed: true };
  if (inputs.ok === true) return { ...DEFAULT_POSE, taped: true, cheeks, strawberry: inputs.hits > 0 };
  return {
    ...DEFAULT_POSE,
    lid: stepLid(inputs.lidShown, lidOffset(inputs.done, inputs.total)),
    eye: blinking(inputs.ms) ? "shut" : inputs.testing ? "squint" : "skeptic",
    gaze: gazeAt(inputs.ms),
    glint: glintAt(inputs.ms, inputs.sinceHitMs),
    cheeks,
  };
}

function stamp(canvas: Canvas, layer: Layer): void {
  for (const [x, y, key] of pixels(layer)) canvas[y][x] = key;
}

function stampLid(canvas: Canvas, rise: number): void {
  if (rise === 0) {
    stamp(canvas, LID);
    return;
  }
  // Hinge the left end at the box. Each column drops by at most one pixel
  // when `rise` drops by one, so jumpy Cargo progress still reads as closing.
  let previousY = LID.y + 1;
  for (let x = 0; x < LID.rows[1].length; x++) {
    const frontY = LID.y + 1 - Math.floor((rise * x) / (SIZE - 1));
    canvas[frontY][x] = "D";
    if (frontY < previousY) canvas[previousY][x] = "D";
    if (x >= 2 && x < SIZE - 2) canvas[frontY - 1][x] = "B";
    previousY = frontY;
  }
}

/**
 * Turn the lens pixels on diagonals `2p + 1` and `2p + 2` into glint. Only
 * pixels still showing lens light up: never the pupil, the ring or the shut
 * line.
 */
function lightBand(canvas: Canvas, position: number): void {
  const band = [2 * position + 1, 2 * position + 2];
  for (const [x, y, key] of pixels(RING)) {
    if (key !== "L" || canvas[y][x] !== "L") continue;
    // The lens diagonal, 1 at the top-left corner to 8 at the bottom-right.
    if (band.includes(x + y - RING.x - RING.y - 2)) canvas[y][x] = "G";
  }
}

/** The pose's pixels as palette keys, row by row. */
export function sprite(pose: Pose): Canvas {
  const canvas: Canvas = Array.from({ length: SIZE }, () => Array<string>(SIZE).fill("."));
  stamp(canvas, BODY);
  if (pose.failed) {
    for (const layer of [OPENING, KNOCKED_LID, GLUM_LEFT, GLUM_RIGHT, MUSTACHE_DROOP, DROPPED]) {
      stamp(canvas, layer);
    }
    return canvas;
  }
  if (pose.lid > 0) stamp(canvas, OPENING);
  stampLid(canvas, pose.lid);
  if (pose.taped) stamp(canvas, TAPE);
  if (pose.cheeks > 0) {
    for (const [x, y] of CHEEKS.flatMap(pixels)) canvas[y][x] = String(pose.cheeks);
  }
  const [eyePupil, lensPupil] = PUPILS[pose.gaze];
  const [eyelid, monocle] =
    pose.eye === "skeptic"
      ? [EYELID, lensPupil]
      : pose.eye === "squint"
        ? [EYELID_SQUINT, lensPupil]
        : [EYE_SHUT, MONOCLE_SHUT];
  if (pose.eye !== "shut") {
    stamp(canvas, EYE_WHITE);
    stamp(canvas, eyePupil);
  }
  stamp(canvas, eyelid);
  stamp(canvas, RING);
  stamp(canvas, monocle);
  stamp(canvas, pose.gaze === "list" || pose.gaze === "bar" ? PARKED_GLINT_RIGHT : PARKED_GLINT);
  if (pose.glint !== null) lightBand(canvas, pose.glint);
  stamp(canvas, MUSTACHE);
  if (pose.strawberry) stamp(canvas, STRAWBERRY);
  return canvas;
}

/** A terminal cell: a glyph from ` `, `▀` and `▄`, and its colours. A space never carries fg. */
export interface Cell {
  glyph: " " | "▀" | "▄";
  fg: Rgb | null;
  bg: Rgb | null;
}

export function cells(canvas: Canvas): Cell[][] {
  return Array.from({ length: HEIGHT }, (_, r) =>
    Array.from({ length: WIDTH }, (_, x): Cell => {
      const [top, bottom] = [canvas[2 * r][x], canvas[2 * r + 1][x]];
      const [upper, lower] = [color(top), color(bottom)];
      if (!upper && !lower) return { glyph: " ", fg: null, bg: null };
      if (upper && top === bottom) return { glyph: " ", fg: null, bg: upper };
      if (upper && !lower) return { glyph: "▀", fg: upper, bg: null };
      if (!upper && lower) return { glyph: "▄", fg: lower, bg: null };
      return { glyph: "▀", fg: upper, bg: lower };
    }),
  );
}

export function draw(pose: Pose): Cell[][] {
  return cells(sprite(pose));
}

/**
 * Paint the sprite with its top-left at (x, y), each of its pixels
 * `pixelSize` across, snapped to whole device pixels (nearest neighbour, no
 * antialiasing) under a transform that scales and translates but does not
 * rotate. Transparent pixels are left alone.
 */
export function drawSprite(
  ctx: CanvasRenderingContext2D,
  pose: Pose,
  x: number,
  y: number,
  pixelSize: number,
): void {
  const m = ctx.getTransform();
  const px = Math.max(1, Math.round(pixelSize * m.a));
  const x0 = Math.round(m.a * x + m.e);
  const y0 = Math.round(m.d * y + m.f);
  const canvas = sprite(pose);
  ctx.save();
  ctx.setTransform(1, 0, 0, 1, 0, 0);
  canvas.forEach((row, j) => {
    // One rectangle per run of a colour along the row.
    for (let i = 0; i < SIZE; ) {
      let n = 1;
      while (i + n < SIZE && row[i + n] === row[i]) n++;
      const c = color(row[i]);
      if (c) {
        ctx.fillStyle = `rgb(${c[0]},${c[1]},${c[2]})`;
        ctx.fillRect(x0 + i * px, y0 + j * px, n * px, px);
      }
      i += n;
    }
  });
  ctx.restore();
}
