// The captions' reading rules (the storyboard's final-timing.py): every
// must-read line holds long enough to read, two-line captions hold all their
// words, and captions never share the screen. Checked for every scene's
// captions, with the benchmark numbers and without them.

import assert from "node:assert/strict";
import { test } from "node:test";
import { BEAT, type ReelFacts, type SectionId, sec } from "../bible";
import { scenes } from "../scenes";
import { type Caption, entrance, plain, readingTime, timeCaptions, WIPE, WORD, wordCount } from "../type";
import { hk } from "./published";

const FACTS: ReelFacts = hk();

/** Every scene's captions under each set of facts, in timeline order. */
function everyCaption(): { name: string; id: SectionId; caps: readonly Caption[] }[] {
  return [FACTS, null].flatMap((facts) =>
    scenes.map((s) => ({ name: `${s.id} (${facts ? "facts" : "no facts"})`, id: s.id, caps: s.captions?.(facts) ?? [] })),
  );
}

/** The first frame at or after `t` on a `fps` grid, as a frame number. */
const frameAt = (t: number, fps: number): number => Math.ceil(t * fps - 1e-6);

/**
 * Seconds a line is fully in on a `fps` grid: from the first frame that
 * shows its last word landed to the first frame of its wipe.
 */
function heldOnFrames(landed: number, out: number, fps: number): number {
  return (frameAt(out, fps) - frameAt(landed, fps)) / fps;
}

test("the storyboard's captions are on screen", () => {
  const n = everyCaption().reduce((k, c) => k + c.caps.length, 0);
  assert.ok(n >= 2 * 11, `only ${n} captions`);
});

test("captions have one or two lines of at most 36 characters, landing in order", () => {
  for (const { name, id, caps } of everyCaption()) {
    const beats = sec(id).bars * 4;
    for (const c of caps) {
      assert.ok(c.lines.length >= 1 && c.lines.length <= 2, `${name}: ${c.lines.length} lines`);
      c.lines.forEach((l, i) => {
        assert.ok(plain(l.text).length <= 36, `${name}: "${plain(l.text)}" is too long`);
        assert.equal((l.text.match(/`/g) ?? []).length % 2, 0, `${name}: unclosed code in "${l.text}"`);
        assert.ok(l.in < c.out, `${name}: "${l.text}" leaves before it lands`);
        if (i > 0) assert.ok(l.in >= c.lines[i - 1].in, `${name}: "${l.text}" lands before the line above it`);
        // The first word starts inside the section.
        assert.ok(entrance(l.text, l.in * BEAT) >= -1e-9, `${name}: "${l.text}" starts before its section`);
      });
      // The wipe finishes inside the section.
      assert.ok(c.out * BEAT + WIPE <= beats * BEAT + 1e-9, `${name}: the wipe at b${c.out} runs past the section`);
    }
  }
});

test("every line holds for words / 4 + 0.5 s, on the beat grid and on both frame grids", () => {
  for (const { name, id, caps } of everyCaption()) {
    const s = sec(id);
    for (const c of caps) {
      for (const l of c.lines) {
        const need = readingTime(wordCount(l.text));
        const landed = s.beat(l.in);
        const out = s.beat(c.out);
        assert.ok(out - landed >= need - 1e-9, `${name}: "${plain(l.text)}" holds ${(out - landed).toFixed(3)} s, needs ${need}`);
        for (const fps of [60, 120]) {
          const held = heldOnFrames(landed, out, fps);
          assert.ok(held >= need - 1e-9, `${name} at ${fps} fps: "${plain(l.text)}" holds ${held.toFixed(4)} s, needs ${need}`);
        }
      }
    }
  }
});

test("a two-line caption holds all its words from the moment its first line lands", () => {
  for (const { name, id, caps } of everyCaption()) {
    const s = sec(id);
    for (const c of caps) {
      const words = c.lines.reduce((n, l) => n + wordCount(l.text), 0);
      const need = readingTime(words);
      const first = s.beat(Math.min(...c.lines.map((l) => l.in)));
      const out = s.beat(c.out);
      const text = c.lines.map((l) => plain(l.text)).join(" / ");
      assert.ok(out - first >= need - 1e-9, `${name}: "${text}" holds ${(out - first).toFixed(3)} s, needs ${need}`);
      for (const fps of [60, 120]) {
        const held = heldOnFrames(first, out, fps);
        assert.ok(held >= need - 1e-9, `${name} at ${fps} fps: "${text}" holds ${held.toFixed(4)} s, needs ${need}`);
      }
    }
  }
});

test("captions never overlap: the next lands at least half a beat after the last starts to leave", () => {
  for (const { name, caps } of everyCaption()) {
    const sorted = [...caps].sort((a, b) => a.lines[0].in - b.lines[0].in);
    for (let i = 1; i < sorted.length; i++) {
      const prev = sorted[i - 1];
      const next = sorted[i];
      const lands = Math.min(...next.lines.map((l) => l.in));
      assert.ok(lands >= prev.out + 0.5, `${name}: a caption lands on b${lands}, the last leaves on b${prev.out}`);
    }
  }
  // On the reel's clock no caption's first word rises before the last
  // caption starts to wipe, so a new line never lands on one still whole.
  for (const facts of [FACTS, null]) {
    const timed = scenes
      .flatMap((s) => timeCaptions(sec(s.id), s.captions?.(facts) ?? []))
      .sort((a, b) => a.start - b.start);
    for (let i = 1; i < timed.length; i++) {
      const text = timed[i].lines.map((l) => plain(l.text)).join(" / ");
      assert.ok(timed[i].start >= timed[i - 1].out - 1e-9, `"${text}" starts before the caption above it leaves`);
    }
  }
});

test("words land one per 1/32 note and leave over a sixteenth", () => {
  assert.equal(WORD, BEAT / 8);
  assert.equal(WIPE, BEAT / 4);
  // Code is in backticks and each run of it is one or more words.
  assert.equal(entrance("Keep typing `cargo build`.", 1), 1 - 4 * WORD);
  assert.equal(entrance("Same checkout, empty `target/`:", 1), 1 - 4 * WORD);
});

test("words are counted as final-timing.py counts them", () => {
  assert.equal(wordCount("Keep typing `cargo build`."), 4);
  assert.equal(wordCount("cargo install mbx --locked && mbx setup"), 6);
  assert.equal(wordCount("354 of 354 restored, 1.3 s."), 6);
  assert.equal(wordCount("CI builds the next push"), 5);
  assert.equal(readingTime(4), 1.5);
});

test("the numbers come from the facts, with a line that claims none without them", () => {
  const same = scenes.find((s) => s.id === "same-checkout");
  const text = (facts: ReelFacts | null) =>
    (same?.captions?.(facts) ?? []).flatMap((c) => c.lines.map((l) => plain(l.text))).join(" / ");
  assert.equal(text(FACTS), "Same checkout, empty target/: / 354 of 354 restored, 1.3 s.");
  assert.equal(text(null), "Same checkout, empty target/: / restored, not recompiled.");
});
