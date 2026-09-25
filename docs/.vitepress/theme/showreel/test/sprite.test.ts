// sprite.ts is a port of crates/mbx/src/cli/mascot.rs. These tests read the
// Rust source and its golden sprites and fail when the two drift apart: a
// change to the terminal mascot has to be ported to the reel's pixel pane.

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { test } from "node:test";
import * as Box from "../box";
import * as Sprite from "../sprite";
import {
  blinking,
  cells,
  cheekLevel,
  color,
  DEFAULT_POSE,
  drawSprite,
  type Eye,
  type Gaze,
  gazeAt,
  glintAt,
  hash32,
  type Inputs,
  type Layer,
  LID_MAX,
  lidAt,
  lidOffset,
  PALETTE,
  PUPILS,
  type Pose,
  poseAt,
  sprite,
  stepLid,
} from "../sprite";
import { REPO } from "./repo";

const rust = readFileSync(join(REPO, "crates/mbx/src/cli/mascot.rs"), "utf8");
const rustTests = readFileSync(join(REPO, "crates/mbx/src/cli/mascot_tests.rs"), "utf8");

const strings = (src: string) => [...src.matchAll(/"([^"]*)"/g)].map((m) => m[1]);
const num = (s: string) => Number(s.replace(/_/g, ""));
const rows = (canvas: string[][]) => canvas.map((r) => r.join(""));

/** A running frame's inputs, the lid caught up with a known total of 60 (mascot_tests.rs). */
const running: Inputs = {
  ms: 0,
  sinceHitMs: null,
  done: 0,
  total: 60,
  lidShown: LID_MAX,
  hits: 0,
  misses: 0,
  testing: false,
  ok: null,
};

test("the palette is mascot.rs's", () => {
  const arms = [...rust.matchAll(/b'(.)' => \((\d+), (\d+), (\d+)\)/g)];
  assert.ok(arms.length >= 16, "found color()'s match arms");
  const theirs = Object.fromEntries(arms.map((m) => [m[1], [num(m[2]), num(m[3]), num(m[4])]]));
  assert.deepEqual({ ...PALETTE }, theirs);
  assert.equal(color("."), null);
  assert.equal(color("c"), null);
});

test("every layer is mascot.rs's, pixel for pixel", () => {
  const layers = [...rust.matchAll(/const (\w+): Layer = Layer::at\((\d+), (\d+), &\[([^\]]*)\]\);/g)];
  assert.ok(layers.length >= 18, "found the layers");
  const mine = Sprite as unknown as Record<string, Layer | undefined>;
  for (const [, name, x, y, body] of layers) {
    assert.deepEqual(mine[name], { x: num(x), y: num(y), rows: strings(body) }, name);
  }
  const cheeks = rust.match(/const CHEEKS: \[Layer; 2\] = \[([^;]*)\];/);
  assert.ok(cheeks, "found CHEEKS");
  const theirCheeks = [...cheeks[1].matchAll(/Layer::at\((\d+), (\d+), &\[([^\]]*)\]\)/g)].map(
    ([, x, y, body]) => ({ x: num(x), y: num(y), rows: strings(body) }),
  );
  assert.deepEqual([...Sprite.CHEEKS], theirCheeks);
  const pupil = rust.match(/const PUPIL: &\[&str\] = &\[([^\]]*)\];/);
  assert.ok(pupil, "found PUPIL");
  assert.deepEqual([...Sprite.PUPIL], strings(pupil[1]));
});

test("the constants are mascot.rs's", () => {
  const consts = new Map<string, number>();
  for (const [, name, expr] of rust.matchAll(/const (\w+): (?:u8|u128|usize) = ([^;]+);/g)) {
    // Literals, or one product or difference of earlier constants.
    const term = (s: string) => {
      const v = /^[\d_]+$/.test(s) ? num(s) : consts.get(s);
      assert.ok(v !== undefined, `${name}: ${s}`);
      return v;
    };
    const m = expr.trim().match(/^(\w+)(?: ([*-]) (\w+))?$/);
    assert.ok(m, `${name} = ${expr}`);
    const v = m[2] === "*" ? term(m[1]) * term(m[3]) : m[2] === "-" ? term(m[1]) - term(m[3]) : term(m[1]);
    consts.set(name, v);
  }
  assert.ok(consts.size >= 15, "found the constants");
  const mine = Sprite as unknown as Record<string, unknown>;
  for (const [name, value] of consts) assert.equal(mine[name], value, name);
});

test("the gaze's pupils are mascot.rs's", () => {
  const theirs = Object.fromEntries(
    [...rust.matchAll(/Self::(List|Bar|You) => \[\((\d+), (\d+)\), \((\d+), (\d+)\)\]/g)].map((m) => [
      m[1].toLowerCase(),
      [
        [num(m[2]), num(m[3])],
        [num(m[4]), num(m[5])],
      ],
    ]),
  );
  assert.deepEqual(PUPILS, theirs);
});

test("mascot_tests.rs's golden sprites", () => {
  const golden = new Map(
    [...rustTests.matchAll(/rows\(&sprite\((\w+)(?:\(\))?\)\),\s*\[([^\]]*)\]/g)].map((m) => [m[1], strings(m[2])]),
  );
  assert.deepEqual([...golden.keys()], ["key_pose", "start", "testing", "failed"]);
  const poses: Record<string, Pose> = {
    key_pose: { ...DEFAULT_POSE, taped: true, cheeks: 3 },
    start: poseAt({ ...running, total: null }),
    testing: poseAt({
      ...running,
      ms: 4600,
      sinceHitMs: 50,
      done: 45,
      lidShown: 2,
      hits: 2,
      misses: 8,
      testing: true,
    }),
    failed: poseAt({
      ...running,
      ms: 9000,
      sinceHitMs: 5000,
      done: 24,
      lidShown: 2,
      hits: 10,
      misses: 14,
      ok: false,
    }),
  };
  for (const [name, expected] of golden) assert.deepEqual(rows(sprite(poses[name])), expected, name);
  assert.deepEqual(poses.testing, {
    ...DEFAULT_POSE,
    lid: 1,
    eye: "squint",
    gaze: "bar",
    glint: 1,
    cheeks: 1,
  });
  const blinkRow = rustTests.match(/assert_eq!\(&blink\[9\], b"([^"]+)"\)/);
  assert.ok(blinkRow, "found the blink row");
  assert.equal(rows(sprite({ ...DEFAULT_POSE, eye: "shut" }))[9], blinkRow[1]);
});

test("hash32 is mascot.rs's, from the numbers in its source", () => {
  const body = rust.match(/fn hash32\(n: u32\) -> u32 \{([^}]*)\}/);
  assert.ok(body, "found hash32");
  const [mulA, add, shiftA, mulB, shiftB] = [...body[1].matchAll(/[\d_]{2,}/g)].map((m) => num(m[0]));
  const theirs = (n: number) => {
    let v = Number((BigInt(n) * BigInt(mulA) + BigInt(add)) % 2n ** 32n);
    v = (v ^ (v >>> shiftA)) >>> 0;
    v = Number((BigInt(v) * BigInt(mulB)) % 2n ** 32n);
    return (v ^ (v >>> shiftB)) >>> 0;
  };
  for (let n = 0; n < 5000; n++) assert.equal(hash32(n), theirs(n), `${n}`);
  for (const n of [2 ** 31, 2 ** 32 - 1, 123_456_789]) assert.equal(hash32(n), theirs(n), `${n}`);
  const golden = rustTests.match(/\[hash32\(0\), hash32\(1\), hash32\(2\)\],\s*\[([\d_, ]+)\]/);
  assert.ok(golden, "found the hash goldens");
  assert.deepEqual([0, 1, 2].map(hash32), golden[1].split(",").map(num));
});

test("mascot_tests.rs's rules: blinks, gaze, lid, cheeks, glint", () => {
  const starts = rustTests.match(/assert_eq!\(start, \[([\d, ]+)\]\[window as usize\]\)/);
  assert.ok(starts, "found the blink starts");
  const want = starts[1].split(",").map(num);
  const { BLINK_WINDOW_MS: span, BLINK_MS } = Sprite;
  for (let window = 0; window < 200; window++) {
    const shut = [];
    for (let ms = window * span; ms < (window + 1) * span; ms++) if (blinking(ms)) shut.push(ms);
    assert.equal(shut.length, BLINK_MS, `${window}`);
    assert.equal(shut[BLINK_MS - 1] - shut[0], BLINK_MS - 1, `${window}`);
    if (window < want.length) assert.equal(shut[0] - window * span, want[window]);
  }
  const gazes = rustTests.match(/\[([\d, ]+)\]\.map\(gaze_at\),\s*\[([^\]]+)\]/);
  assert.ok(gazes, "found the gaze table");
  assert.deepEqual(
    gazes[1].split(",").map(num).map(gazeAt),
    [...gazes[2].matchAll(/Gaze::(\w+)/g)].map((m) => m[1].toLowerCase()),
  );
  const lidTables = [...rustTests.matchAll(/at\((\d+), &\[([\d, ]+)\]\),\s*\[([\d, ]+)\]/g)];
  assert.ok(lidTables.length >= 3, "found the lid tables");
  for (const [, total, dones, targets] of lidTables) {
    assert.deepEqual(
      dones.split(",").map((d) => lidOffset(num(d), num(total))),
      targets.split(",").map(num),
      `total ${total}`,
    );
  }
  for (const [, done, total, target] of rustTests.matchAll(/lid_offset\((\d+), Some\((\d+)\)\), (\d+)\)/g)) {
    assert.equal(lidOffset(num(done), num(total)), num(target));
  }
  assert.equal(lidOffset(5, null), LID_MAX);
  assert.equal(lidOffset(5, 0), LID_MAX);
  for (let shown = 0; shown <= LID_MAX; shown++) {
    for (let target = 0; target <= LID_MAX; target++) {
      assert.equal(stepLid(shown, target), target < shown ? shown - 1 : target);
    }
  }
  const cheeks = rustTests.match(/\[([\d, ]+)\]\.map\(\|hits\| cheek_level\(hits, (\d+) - hits\)\),\s*\[([\d, ]+)\]/);
  assert.ok(cheeks, "found the cheek table");
  assert.deepEqual(
    cheeks[1].split(",").map((h) => cheekLevel(num(h), num(cheeks[2]) - num(h))),
    cheeks[3].split(",").map(num),
  );
  assert.deepEqual([cheekLevel(0, 0), cheekLevel(0, 50)], [0, 0]);
  const { GLINT_CYCLE_MS, GLINT_SWEEP_MS, GLINT_STEP_MS } = Sprite;
  for (let ms = 0; ms < 30_000; ms += 7) {
    const phase = ms % GLINT_CYCLE_MS;
    assert.equal(glintAt(ms, 0), phase < GLINT_SWEEP_MS ? Math.floor(phase / GLINT_STEP_MS) : null, `${ms}`);
    assert.equal(glintAt(ms, null), null);
  }
});

test("only a build that hit the cache earns the strawberry", () => {
  for (const [hits, ok, strawberry] of [
    [0, true, false],
    [1, true, true],
    [1, null, false],
    [1, false, false],
  ] as const) {
    const pose = poseAt({ ...running, sinceHitMs: hits > 0 ? 5000 : null, done: 60, hits, misses: 10, ok });
    assert.equal(pose.strawberry, strawberry);
    assert.equal(sprite(pose).flat().includes("R"), strawberry);
  }
});

test("lidAt steps the lid down a pixel per frame and up at once", () => {
  const target = (i: number) => (i < 5 ? 4 : i < 6 ? 0 : 2);
  assert.deepEqual(
    Array.from({ length: 10 }, (_, i) => lidAt(i, target)),
    [4, 4, 4, 4, 4, 3, 2, 2, 2, 2],
  );
  assert.equal(lidAt(3, () => 0), 0);
});

function everyPose(): Pose[] {
  const poses: Pose[] = [];
  for (let lid = 0; lid <= LID_MAX; lid++)
    for (const eye of ["skeptic", "squint", "shut"] as Eye[])
      for (const gaze of ["list", "bar", "you"] as Gaze[])
        for (const glint of [null, 0, 1, 2, 3])
          for (let cheeks = 0; cheeks <= 3; cheeks++) poses.push({ ...DEFAULT_POSE, lid, eye, gaze, glint, cheeks });
  for (let cheeks = 0; cheeks <= 3; cheeks++)
    for (const strawberry of [false, true]) poses.push({ ...DEFAULT_POSE, taped: true, cheeks, strawberry });
  poses.push({ ...DEFAULT_POSE, failed: true });
  return poses;
}

test("every cell shows its two pixels with half blocks", () => {
  for (const pose of everyPose()) {
    const canvas = sprite(pose);
    for (const key of canvas.flat()) assert.ok(key === "." || color(key), JSON.stringify(pose));
    const grid = cells(canvas);
    assert.deepEqual([grid.length, grid[0].length], [9, 18]);
    grid.forEach((row, r) =>
      row.forEach((cell, x) => {
        const shown =
          cell.glyph === " " && !cell.fg
            ? [cell.bg, cell.bg]
            : cell.glyph === "▀" && cell.fg
              ? [cell.fg, cell.bg]
              : cell.glyph === "▄" && cell.fg
                ? [cell.bg, cell.fg]
                : assert.fail(`${JSON.stringify(cell)} at (${x}, ${r})`);
        assert.deepEqual(shown, [color(canvas[2 * r][x]), color(canvas[2 * r + 1][x])]);
      }),
    );
  }
});

test("drawSprite paints whole device pixels", () => {
  const rects: number[][] = [];
  const ctx = {
    getTransform: () => ({ a: 1.5, b: 0, c: 0, d: 1.5, e: 0.3, f: 10.6 }),
    save() {},
    restore() {},
    setTransform() {},
    fillStyle: "",
    fillRect: (...r: number[]) => rects.push(r),
  } as unknown as CanvasRenderingContext2D;
  drawSprite(ctx, { ...DEFAULT_POSE, taped: true, cheeks: 3 }, 100.2, 50.1, 4);
  assert.ok(rects.length > 0);
  for (const r of rects) assert.ok(r.every(Number.isInteger), `${r}`);
  // 4 px at 1.5x is 6 device px, from (150.6, 85.75) rounded.
  const top = Math.min(...rects.map((r) => r[1]));
  assert.equal(top, 86 + 4 * 6);
  assert.ok(rects.every((r) => r[3] === 6 && (r[0] - 151) % 6 === 0));
});

test("the vector character wears the mascot's colours", () => {
  const hex = (key: string) => `#${PALETTE[key].map((c) => c.toString(16).padStart(2, "0")).join("")}`;
  assert.equal(Box.LOGO_INK, hex("K"));
  assert.equal(Box.PAPER, hex("W"));
  assert.equal(Box.LENS, hex("L"));
  assert.equal(Box.CHAIN, hex("S"));
  assert.equal(Box.LOGO_TAPE, hex("T"));
  assert.equal(Box.INSIDE, hex("H"));
  assert.equal(Box.BERRY, hex("R"));
  assert.deepEqual([...Box.CHEEKS], ["1", "2", "3"].map(hex));
});
