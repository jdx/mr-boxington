// The fold and Mr Boxington against the storyboard (Appendix B): the fold
// settles on its handoff before the bar line, the face pops on sixteenths,
// and the name card holds long enough to read, clear of the box and the
// captions' band, and has wiped away by the bar line where the dive crosses
// into "what". The frames themselves are checked by eye.

import assert from "node:assert/strict";
import { test } from "node:test";
import { BEAT, DIVE0, sec } from "../bible";
import { CAPTION_TOP } from "../map";
import { FOLDS, T_REST, T_SLAM, T_TAPE0, T_TAPE1 } from "../scenes/s1-unfold";
import {
  BLINK,
  BLUSH,
  CARD,
  CARD_LANDS,
  CHAIN0,
  chainDotAt,
  EYE,
  GLINT,
  LAND,
  LINE,
  MONO,
  MUST,
  NAME,
  PUSH,
  TAGLINE,
  WORDMARK,
} from "../scenes/s2-character";
import { readingTime, WIPE, wordCount } from "../type";

const FOLD = sec("fold");
const FACE = sec("mr-boxington");
const frameAt = (t: number, fps: number) => Math.ceil(t * fps - 1e-6);

test("the fold folds on sixteenths, tapes, and rests on its handoff before the bar line", () => {
  FOLDS.forEach((t, i) => assert.equal(t, (2 + i / 4) * BEAT));
  assert.equal(T_SLAM, 3 * BEAT);
  assert.equal(T_TAPE0, 3.25 * BEAT);
  assert.equal(T_TAPE1, 3.75 * BEAT);
  assert.ok(T_REST > T_TAPE1, "the tape seats before the rest");
  for (const fps of [60, 120]) {
    // At least one whole frame of the handoff frame before the bar line.
    assert.ok(frameAt(T_REST, fps) < frameAt(FOLD.len, fps), `no rest frame at ${fps} fps`);
  }
});

test("he lands on b1.25 and his face pops in on sixteenths from b1.5", () => {
  assert.equal(LAND, 1.25 * BEAT);
  assert.deepEqual([EYE, MUST, MONO, GLINT, BLUSH], [1.5, 1.75, 2, 2.25, 2.5].map((b) => b * BEAT));
  assert.equal(BLINK, 5.5 * BEAT);
  assert.equal(PUSH, DIVE0 - FACE.start);
  assert.equal(PUSH, 7.75 * BEAT);
});

test("the monocle's chain pays out from its anchor before the monocle clinks home", () => {
  let last = -Infinity;
  for (let i = 5; i >= 0; i--) {
    const t = chainDotAt(i);
    assert.ok(t >= CHAIN0 && t < MONO, `dot ${i} at ${t}`);
    assert.ok(t > last, "the anchor end first");
    last = t;
  }
});

test("the name card lands on b2.75 and b3, holds to read, and wipes by the bar line", () => {
  assert.equal(CARD_LANDS[0], WORDMARK);
  assert.equal(WORDMARK, 2.75 * BEAT);
  assert.equal(TAGLINE, 3 * BEAT);
  assert.ok(CARD_LANDS[1] <= TAGLINE && CARD_LANDS[2] === TAGLINE);
  const lines = [NAME, ...LINE];
  for (const fps of [60, 120]) {
    lines.forEach((text, i) => {
      const held = (frameAt(PUSH, fps) - frameAt(CARD_LANDS[i], fps)) / fps;
      assert.ok(held >= readingTime(wordCount(text)), `"${text}" holds ${held} s at ${fps} fps`);
    });
    // The line as a whole, from its first half landing.
    const words = wordCount(LINE.join(" "));
    const held = (frameAt(PUSH, fps) - frameAt(Math.min(CARD_LANDS[1], CARD_LANDS[2]), fps)) / fps;
    assert.ok(held >= readingTime(words), `the line holds ${held} s at ${fps} fps`);
  }
  assert.ok(PUSH + WIPE <= FACE.len + 1e-9, "the card has wiped by the bar line");
  assert.equal(LINE.join(" "), "A shared cache for Cargo builds.");
  assert.equal(NAME, "mr boxington");
});

test("the name card stands right of the box and above the captions' band", () => {
  // H2_CAM draws the box at x 264-676.
  assert.ok(CARD.x > 676 + 60);
  // The line's lower baseline plus a descender at 88 px.
  assert.ok(CARD.line[1] + 0.25 * 88 < CAPTION_TOP);
  assert.ok(CARD.name < CARD.line[0] && CARD.line[0] < CARD.line[1]);
});
