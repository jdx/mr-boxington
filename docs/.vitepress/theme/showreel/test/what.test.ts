// The "what" section (scenes/s3-type.ts) against the storyboard: its beat
// map, the reading rule for the sentence it spells out, and both handoffs,
// drawn call for call. The frames themselves are checked by eye.

import assert from "node:assert/strict";
import { test } from "node:test";
import { BEAT, DIVE1, type SceneEnv, sec } from "../bible";
import { drawHandoff } from "../map";
import {
  caretOn,
  INHALE,
  KEYS,
  LEAD,
  LOCKUP,
  scene,
  T_AND,
  T_BRANCH,
  T_CI,
  T_DONE,
  T_IRIS,
  T_LAND,
  T_OUT,
  T_P,
  T_PEEL,
  T_W,
} from "../scenes/s3-type";
import { readingTime, wordCount } from "../type";

const S = sec("what");
const b = (n: number) => n * BEAT;
const near = (a: number, x: number, what: string) => assert.ok(Math.abs(a - x) < 1e-9, `${what}: ${a} vs ${x}`);

test("the section keeps the storyboard's beats", () => {
  // The dive covers the frame on b0.5.
  near(T_IRIS, b(0.5), "iris");
  near(S.start + T_IRIS, DIVE1, "the iris on the dive's end");
  assert.ok(T_PEEL > 0 && T_PEEL < T_IRIS, "the glint lets go during the dive");
  near(T_P, b(2.25), "projects,");
  near(T_W, b(3), "worktrees,");
  assert.ok(T_BRANCH < T_W && T_BRANCH > T_P);
  near(T_AND, b(3.75), "and");
  near(T_CI, b(4), "CI.");
  near(T_DONE, b(4.5), "complete");
  near(T_OUT, b(11), "breakup");
  assert.ok(T_LAND > b(11.5) && T_LAND < S.len, "the names land inside the section's last beat");
  near(LOCKUP, S.beat(8), "fallback poster");
});

test("the lead types once, in order, and is in by b2", () => {
  const typed = KEYS.filter((k) => k.i >= 0);
  assert.equal(typed.map((k) => k.ch).join(""), LEAD.join(""));
  assert.equal(KEYS.filter((k) => k.ch === "\n").length, LEAD.length - 1);
  for (let i = 1; i < KEYS.length; i++) assert.ok(KEYS[i].t > KEYS[i - 1].t, `key ${i} lands before key ${i - 1}`);
  assert.ok(KEYS[0].t > T_IRIS, "the first key lands after the pupil opens");
  assert.ok(KEYS[KEYS.length - 1].t <= b(2), "the lead is in by b2");
});

test("the whole sentence holds for its reading time, on both frame grids", () => {
  const sentence = `${LEAD.join(" ")} projects, worktrees, and CI.`;
  const need = readingTime(wordCount(sentence));
  assert.equal(wordCount(sentence), 10);
  // From the frame CI. settles to the first frame of the inhale before the break.
  const leave = T_OUT - INHALE;
  assert.ok(leave - T_DONE >= need, `holds ${leave - T_DONE}, needs ${need}`);
  for (const fps of [60, 120]) {
    const frames = Math.ceil(S.at(leave) * fps - 1e-6) - Math.ceil(S.at(T_DONE) * fps - 1e-6);
    assert.ok(frames / fps >= need - 1e-9, `${fps} fps: holds ${frames / fps}`);
  }
});

test("the caret blinks about once a second and is on for the fallback poster", () => {
  assert.ok(caretOn(LOCKUP - S.start));
  assert.ok(caretOn(b(8.9)));
  assert.ok(!caretOn(b(9.1)));
  assert.ok(caretOn(b(10.1)));
});

// A canvas that writes down what is drawn: every call and every property
// set, with the arguments. Paths are Path2D stand-ins that keep their data.
type Log = string[];
function recorder(log: Log): CanvasRenderingContext2D {
  const gradient = { addColorStop: (o: number, c: string) => log.push(`stop ${o} ${c}`) };
  const target: Record<string, unknown> = {
    measureText: (s: string) => (log.push("measureText()"), {
      width: 30 * Array.from(s).length,
      actualBoundingBoxAscent: 20,
      actualBoundingBoxDescent: 6,
    }),
    createRadialGradient: (...a: number[]) => (log.push(`radial ${a.map((v) => v.toFixed(3))}`), gradient),
    createLinearGradient: (...a: number[]) => (log.push(`linear ${a.map((v) => v.toFixed(3))}`), gradient),
  };
  // Styles read back what was set, as a canvas's do.
  const state: Record<string, unknown> = { globalAlpha: 1, lineWidth: 1, globalCompositeOperation: "source-over" };
  const fmt = (v: unknown) => (typeof v === "number" ? v.toFixed(3) : String(v));
  return new Proxy(target, {
    get(t, key: string) {
      if (key in t) return t[key];
      if (key in state) return state[key];
      return (...args: unknown[]) => log.push(`${key}(${args.map(fmt).join(",")})`);
    },
    set(_t, key: string, value) {
      state[key] = value;
      log.push(`${key}=${fmt(value)}`);
      return true;
    },
  }) as unknown as CanvasRenderingContext2D;
}

class FakePath {
  constructor(readonly d = "") {}
  toString() {
    return `path ${this.d}`;
  }
  arc() {}
  moveTo() {}
  lineTo() {}
  closePath() {}
}

/** What a draw leaves on the canvas: calls and styles, without state bookkeeping. */
function drawn(draw: (ctx: CanvasRenderingContext2D) => void): Log {
  const g = globalThis as { Path2D?: unknown };
  const had = g.Path2D;
  g.Path2D = FakePath;
  try {
    const log: Log = [];
    draw(recorder(log));
    // Measuring text sets a font and reads widths without drawing.
    const kept = log.filter((l) => !/^(save|restore)\(|^(textAlign|textBaseline)=/.test(l));
    return kept.filter((l, i) => !l.startsWith("measureText(") && !(l.startsWith("font=") && kept[i + 1]?.startsWith("measureText(")));
  } finally {
    g.Path2D = had;
  }
}

const env = (t: number): SceneEnv => ({ W: 1920, H: 1080, t, facts: null });

test("the section starts on the monocle dive's bar-line frame", () => {
  const want = drawn((ctx) => drawHandoff(ctx, "mr-boxington|what", env(S.start)));
  const got = drawn((ctx) => scene.draw(ctx, 0, env(S.start)));
  assert.ok(want.length > 20);
  assert.deepEqual(got, want);
});

test("the names land as every-checkout's labels and hold them to the bar line", () => {
  const want = drawn((ctx) => drawHandoff(ctx, "what|every-checkout", env(S.end)));
  for (const lt of [T_LAND, (T_LAND + S.len) / 2, S.len - 1 / 120]) {
    assert.deepEqual(
      drawn((ctx) => scene.draw(ctx, lt, env(S.at(lt)))),
      want,
      `at b${(lt / BEAT).toFixed(3)}`,
    );
  }
});
