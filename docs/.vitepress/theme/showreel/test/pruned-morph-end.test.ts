// The last three sections: target/ pruned (scenes/s6-world.ts), the morph
// (s7-morph.ts) and the end card (s8-logo.ts). Their copy is the
// storyboard's, the pruning follows the rules it names and never takes the
// most recent checkout, the end card starts on the morph's silhouette and
// lands on the logo seen square-on, it holds long enough and ends still, and
// the score sounds on the picture's hits.

import assert from "node:assert/strict";
import { test } from "node:test";
import { playScore } from "../audio";
import { BEAT, END_CAM, END_POSE, KEEP, sec } from "../bible";
import { boxSilhouette, logoCam } from "../box";
import { EC_SLABS, TOWER_NAMES } from "../map";
import * as pruned from "../scenes/s6-world";
import * as end from "../scenes/s8-logo";
import { type Camera, View } from "../space";
import { plain, readingTime, wordCount } from "../type";
import { MockContext } from "./mock-audio";

const P = sec("pruned");
const E = sec("end");
const b = (n: number) => n * BEAT;

/** Two cameras frame the same view, to a millionth. */
function sameCam(a: Camera, e: Camera, what: string): void {
  const nums = (c: Camera) => [c.cx, c.cy, c.scale, c.yaw, c.pitch, c.roll ?? 0, c.persp ?? 0, ...(c.target ?? [0, 0, 0])];
  const [x, y] = [nums(a), nums(e)];
  x.forEach((v, i) => assert.ok(Math.abs(v - y[i]) < 1e-6, `${what}: ${JSON.stringify(a)} against ${JSON.stringify(e)}`));
}

test("pruned says where target/ lives, then the three rules", () => {
  const caps = (pruned.scene.captions?.(null) ?? []).map((c) => ({ out: c.out, lines: c.lines.map((l) => [l.in, plain(l.text)]) }));
  assert.deepEqual(caps, [{ out: 4.5, lines: [[0.75, "target/ lives in the cache."]] }]);
  assert.equal(pruned.HEADER, "Pruned after builds:");
  assert.deepEqual(pruned.RULE_TEXT, ["checkout deleted", "unused 30 days", "over budget"]);
  // The header is in on b5 and the rules on b5.25, b6 and b6.75; all leave on b11.5.
  assert.equal(pruned.T_HEADER, b(5));
  assert.deepEqual(pruned.T_RULES, [b(5.25), b(6), b(6.75)]);
  assert.equal(pruned.COPY_OUT, b(11.5));
  // Every rule, and the header, holds for its reading time.
  for (const [text, land] of [[pruned.HEADER, pruned.T_HEADER], ...pruned.RULE_TEXT.map((r, i) => [r, pruned.T_RULES[i]] as const)] as const) {
    assert.ok(pruned.COPY_OUT - land >= readingTime(wordCount(text)), `"${text}" holds ${(pruned.COPY_OUT - land).toFixed(3)} s`);
  }
});

test("the cartons are every-checkout's target directories, and the rules take the right ones", () => {
  const city = pruned.world();
  const named = city.filter((c) => c.name).map((c) => c.name?.name ?? "");
  const known = new Set<string>([...TOWER_NAMES, ...Object.values(EC_SLABS)]);
  assert.ok(named.length >= 4, "several cartons carry checkout names");
  for (const n of named) assert.ok(known.has(n), `${n} is not a checkout from every-checkout`);
  const byName = (n: string) => {
    const c = city.find((x) => x.name?.name === n);
    assert.ok(c, n);
    return c;
  };
  // hk and hk-fix are kept; hk-fix, the most recently used, is scanned last.
  assert.ok(byName("hk").keep && byName("hk-fix").keep);
  for (const c of city) if (c.name && c.name.name !== "hk-fix") assert.ok(c.hit <= byName("hk-fix").hit, `${c.name.name} is scanned after hk-fix`);
  // The keepers are exactly the discs the morph picks up, and none of them folds.
  assert.deepEqual(
    city.filter((c) => c.keep).map((c) => [c.x, c.z]).sort(),
    KEEP.map(([x, z]) => [x, z]).sort(),
  );
  for (const c of city) if (c.keep) assert.equal(c.fold, Infinity);
  // One orphan, whose checkout is deleted, goes flat on the first rule, the
  // stale group on the second, and everything else over budget from the third.
  const rule = (r: number) => city.filter((c) => c.rule === r);
  assert.deepEqual(rule(1).map((c) => c.name?.name), ["hk-pr-812"]);
  assert.ok(rule(2).some((c) => c.name?.name === "hk-old-spike"), "the clock's carton is stale");
  for (const c of rule(1)) assert.ok(Math.abs(pruned.slapAt(c) - pruned.T_RULES[0]) < 1e-9);
  for (const c of rule(2)) assert.ok(Math.abs(pruned.slapAt(c) - pruned.T_RULES[1]) < 1e-9);
  for (const c of rule(3)) {
    assert.ok(pruned.slapAt(c) >= pruned.T_RULES[2] - 1e-9, "nothing goes over budget before its rule");
    assert.ok(pruned.slapAt(c) <= pruned.T_BEAMEND, "every pruned carton is flat before the beam leaves");
  }
  // Least recently used first: over budget, a row the beam reaches later never goes flat earlier.
  const late = rule(3).filter((c) => c.hit >= pruned.T_RULES[2] - pruned.SLAP);
  for (const a of late) for (const c of late) if (a.rank < c.rank) assert.ok(a.fold <= c.fold + 1e-9);
  // The keepers hop after the beam has gone and land as the discs by b11.5.
  assert.ok(pruned.T_HOP > pruned.T_BEAMEND && pruned.T_DISC === b(11.5));
});

test("the end card's copy is the storyboard's", () => {
  assert.equal(end.WORD, "mr boxington");
  assert.equal(end.LINE, "A shared cache for Cargo builds");
  assert.equal(end.INSTALL, "cargo install mbx --locked && mbx setup");
  assert.equal(end.URL_TEXT, "mr-boxington.jdx.dev");
  assert.deepEqual([end.T_WORD, end.T_LINE, end.T_INSTALL, end.T_URL], [b(2.75), b(3.25), b(3.75), b(4)]);
  assert.equal(end.T_BERRY, b(2));
  assert.deepEqual([end.T_GLINT, end.T_BLINK], [b(8), b(10)]);
  // The card is whole from the address on, and holds at least 3.75 s.
  assert.ok(E.len - end.T_URL >= 3.75, `the card holds ${(E.len - end.T_URL).toFixed(3)} s`);
  assert.equal(end.scene.captions, undefined, "the card's lines are its own, not lower-third captions");
});

test("the end card inflates from END_CAM and lands on the logo seen square-on", () => {
  sameCam(end.cardAt(0).cam, END_CAM, "the first frame");
  // The inflate starts from rest on the morph's silhouette.
  const sil = (lt: number) => boxSilhouette(new View(end.cardAt(lt).cam), end.cardAt(lt).pose);
  const start = boxSilhouette(new View(END_CAM), END_POSE);
  sil(0).forEach((p, i) => assert.ok(Math.hypot(p.x - start[i].x, p.y - start[i].y) < 0.5, `corner ${i} jumps on the downbeat`));
  // It lands on the logo's own square: logoCam, moved down with the box.
  const L = logoCam(end.CARD_SQ.x, end.CARD_SQ.y, end.CARD_SQ.size);
  const [x, y, z] = L.target ?? [0, 0, 0];
  sameCam(end.cardAt(E.len).cam, { ...L, target: [x, y + END_POSE.pos[1], z] }, "the card");
  assert.equal(end.cardAt(E.len).cam.pitch, 0, "the tip has straightened out");
  // The box clears the wordmark below it.
  const base = Math.max(...sil(E.len).map((p) => p.y));
  assert.ok(base < end.CARD.word - 128 * 0.75 - 24, `the box's base at y ${base.toFixed(0)} crowds the wordmark`);
});

test("the end card is the new character, taped with its strawberry, and ends on a still frame", () => {
  for (let lt = 0; lt <= E.len; lt += 1 / 60) {
    const { pose } = end.cardAt(lt);
    assert.equal(pose.kind, "logo", `at ${lt.toFixed(3)} the box is not the logo`);
    for (const k of ["brows", "browLift", "bowtie", "bowtieSpin", "label"]) {
      assert.ok(!(pose.face && k in pose.face), `the face sets the cube's ${k}`);
    }
    assert.ok(!pose.label, "no shipping label");
  }
  const last = end.cardAt(E.len - 1 / 120);
  assert.equal(last.pose.tape, 1);
  assert.equal(last.pose.face?.strawberry, 1);
  assert.equal(last.pose.face?.cheeks, 3);
  assert.equal(last.pose.face?.berryLift, 0);
  // From STILL on, nothing in the picture changes.
  assert.ok(end.STILL >= end.T_BLINK + 0.16 && end.STILL < E.len - 0.4);
  assert.deepEqual(end.cardAt(end.STILL), last);
});

test("the score sounds on pruned's and the end card's hits", () => {
  const WHEN = 0.2;
  const ac = new MockContext();
  playScore(ac.context, ac.destination as AudioNode, 0, WHEN);
  const starts = ac.starts().map(({ t }) => t - WHEN);
  const near = (t: number, what: string) =>
    assert.ok(
      starts.some((s) => Math.abs(s - t) < 0.012),
      `nothing sounds at ${what} (${t.toFixed(3)} s)`,
    );
  near(P.at(pruned.T_STOMP), "the stomp");
  near(P.at(pruned.T_CRUMPLE), "the crumple");
  near(P.at(pruned.T_CLOCK), "the clock");
  near(P.at(pruned.T_DAYS), "the month running out");
  near(P.at(pruned.T_PLANE), "the disk budget");
  pruned.T_RULES.forEach((t, i) => near(P.at(t), `rule ${i + 1}`));
  near(P.at(pruned.T_BEAM0), "the beam");
  near(P.at(pruned.T_DROP), "the carpet's drop");
  near(P.at(pruned.T_HOP), "the keepers' hop");
  near(P.at(pruned.T_DISC), "the discs");
  near(E.start, "the resolve");
  near(E.at(end.T_BERRY), "the strawberry");
  near(E.at(end.T_URL), "the address");
  near(E.at(end.T_GLINT), "the glint");
});
