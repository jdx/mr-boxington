// The map's layout and every section's handoff: a contract for all thirteen
// bar lines, each on its bar line, the map's actors clear of the captions'
// band, and the shared moves (the monocle dive, the whip) continuous where
// they cross a bar line. The frames themselves are checked by eye.

import assert from "node:assert/strict";
import { test } from "node:test";
import { BEAT, DIVE0, DIVE1, diveCam, H, H2_CAM, H2_POSE, NODES, SECTIONS, type SectionId, sec, W, WHIP } from "../bible";
import { monocleScreen } from "../box";
import {
  BAR_CELLS,
  BOUNDARIES,
  boxFromSprite,
  buildAt,
  CAPTION_TOP,
  CI,
  EC_CARDS,
  FLOOR,
  HANDOFFS,
  handoffIn,
  handoffOut,
  MACHINE,
  mapToScreen,
  OVERVIEW,
  paneLayout,
  PLAN,
  planChip,
  type Rect,
  segments,
  type Unit,
  whipIn,
  whipOut,
  WHIP_AT,
  WHIP_WIND,
  worldBounds,
} from "../map";
import { POSTER_TIME } from "../reel";
import { scenes } from "../scenes";
import { View } from "../space";

const bottom = (r: Rect) => r.y + r.h;

test("every bar line has a handoff, in order, on its bar line", () => {
  assert.equal(BOUNDARIES.length, SECTIONS.length - 1);
  assert.equal(Object.keys(HANDOFFS).length, 13);
  BOUNDARIES.forEach((id, i) => {
    const h = HANDOFFS[id];
    assert.ok(h, `no handoff for ${id}`);
    assert.equal(h.from, SECTIONS[i].id);
    assert.equal(h.to, SECTIONS[i + 1].id);
    assert.equal(h.t, sec(h.to).start);
    assert.equal(h.t, sec(h.from).end);
    assert.ok(h.note.length > 20, `${id} says what is on its frame`);
  });
  for (const { id } of SECTIONS) {
    const i = SECTIONS.findIndex((s) => s.id === id);
    assert.equal(handoffIn(id)?.to ?? null, i === 0 ? null : id);
    assert.equal(handoffOut(id)?.from ?? null, i === SECTIONS.length - 1 ? null : id);
  }
});

test("the map's handoff frames keep the captions' band clear", () => {
  for (const h of Object.values(HANDOFFS)) {
    if (!h.world) continue;
    for (const { actor, rect } of worldBounds(h.world)) {
      if (actor === "frame") {
        // A dashed outline: its bottom edge above the band, or the whole
        // frame grown past the screen.
        const off = rect.x <= 0 && rect.x + rect.w >= W && rect.y <= 0 && bottom(rect) >= H;
        assert.ok(bottom(rect) <= CAPTION_TOP || off, `${h.id}: the frame's bottom edge at y ${bottom(rect)}`);
        continue;
      }
      const offScreen = rect.x >= W || rect.x + rect.w <= 0;
      assert.ok(offScreen || bottom(rect) <= CAPTION_TOP, `${h.id}: ${actor} reaches y ${bottom(rect).toFixed(0)}`);
    }
  }
});

test("every station stands above the captions' band", () => {
  const rects: [string, Rect][] = [
    ...Object.entries(EC_CARDS),
    ["overview frame", OVERVIEW.frame],
    ["pane", MACHINE.pane],
    ["ci frame", CI.start.frame],
    ["hop frame", CI.hop.frame],
    ["remote", CI.remote],
    ...Object.entries(CI.runners),
    ...PLAN.map((_, i): [string, Rect] => {
      const c = planChip(i);
      return [c.text, { x: c.x, y: c.y, w: c.w ?? 0, h: 56 }];
    }),
  ];
  for (const [name, r] of rects) assert.ok(bottom(r) <= CAPTION_TOP, `${name} reaches y ${bottom(r)}`);
  for (const spot of [MACHINE.box, CI.start.box, CI.hop.box]) assert.ok(spot.y <= FLOOR);
  for (const n of Object.values(NODES)) assert.ok(n.y + 16 <= CAPTION_TOP, `${n.label}'s label`);
  // The pane's slab, the box and the tower all stand on the one floor.
  assert.equal(bottom(MACHINE.pane), FLOOR);
  assert.equal(MACHINE.box.y, FLOOR);
  assert.equal(MACHINE.tower.base, FLOOR);
});

test("the machine's frame stays off screen, even pushed in", () => {
  const f = MACHINE.frame;
  const cam = HANDOFFS["under-cargo-build|first-build"].world?.cam;
  assert.ok(cam);
  const a = mapToScreen(cam, { x: f.x, y: f.y });
  const b = mapToScreen(cam, { x: f.x + f.w, y: f.y + f.h });
  assert.ok(a.x < 0 && a.y < 0 && b.x > W && b.y > H);
});

test("the monocle dive starts on H2_CAM and lands on the monocle", () => {
  assert.deepEqual(diveCam(DIVE0 - 1), H2_CAM);
  assert.deepEqual(diveCam(DIVE0), H2_CAM);
  // One move across the bar line: no jump between neighbouring frames.
  const t = sec("what").start;
  const at = (u: number) => diveCam(u).scale;
  const step = Math.abs(at(t + 1 / 120) - at(t)) / at(t);
  assert.ok(step < 0.05, `the zoom jumps ${step} across the bar line`);
  assert.ok(Math.abs(at(t) / H2_CAM.scale - 1.108) < 0.01, "the dive is a sixteenth in on the bar line");
  const m = monocleScreen(new View(diveCam(DIVE1)), H2_POSE);
  assert.ok(Math.abs(m.x - W / 2) < 1 && Math.abs(m.y - H / 2) < 1, `the monocle ends at ${m.x}, ${m.y}`);
  assert.ok(m.r * (21 / 24.5) > Math.hypot(W, H) / 2, "the lens covers the frame");
});

test("the whip leaves ci in shot until the bar line and brings the chart in after it", () => {
  assert.equal(whipOut(WHIP_AT - WHIP - WHIP_WIND), 0);
  // The frame before the bar line still shows ci's right edge.
  const last = whipOut(WHIP_AT - 1 / 120);
  assert.ok(last > -W && last < -W / 2, `the last ci frame is at ${last}`);
  assert.ok(whipOut(WHIP_AT) <= -W);
  assert.equal(whipIn(WHIP_AT), 1800);
  assert.equal(whipIn(WHIP_AT + WHIP), 0);
});

test("segments() is model.rs's", () => {
  const mix = { hits: 7, misses: 2, bypasses: 1, unconsulted: 0 };
  assert.deepEqual(segments(mix, 20), [14, 4, 2]);
  assert.deepEqual(segments({ hits: 0, misses: 0, bypasses: 0, unconsulted: 0 }, 28), [0, 0, 28]);
  for (let w = 0; w <= 80; w++) {
    const s = segments(mix, w);
    assert.equal(s[0] + s[1] + s[2], w);
  }
});

test("buildAt runs the mascot's rules: the lid steps down, a hit build ends taped with a strawberry", () => {
  const units: Unit[] = Array.from({ length: 20 }, (_, i) => ({ at: 1.05 + i * 0.05, outcome: "hit" }));
  const plan = { start: 1, total: 20, units, finish: 2.5 };
  const first = buildAt(plan, 1);
  assert.equal(first.pose.lid, 4);
  assert.equal(first.pose.cheeks, 0);
  assert.equal(first.filled, 0);
  // The lid never rises while the build runs, and is shut before the end.
  let lid = 4;
  for (let t = 1; t < 2.5; t += 1 / 120) {
    const v = buildAt(plan, t);
    assert.ok(v.pose.lid <= lid, `the lid rose at ${t}`);
    lid = v.pose.lid;
  }
  assert.equal(lid, 0);
  const warm = buildAt(plan, 2.5);
  assert.ok(warm.pose.taped && warm.pose.strawberry);
  assert.equal(warm.pose.cheeks, 3);
  assert.equal(warm.filled, BAR_CELLS);
  // The same build with no hits finishes cold: taped, matte, no strawberry.
  const cold = buildAt({ ...plan, units: units.map((u) => ({ ...u, outcome: "other" as const })) }, 2.5);
  assert.ok(cold.pose.taped && !cold.pose.strawberry);
  assert.equal(cold.pose.cheeks, 0);
  assert.deepEqual(segments(cold.mix, BAR_CELLS), [0, 0, BAR_CELLS]);
  // The big box mirrors it.
  const box = boxFromSprite(warm.pose);
  assert.equal(box.tape, 1);
  assert.equal(box.lid, 0);
  assert.equal(box.face?.strawberry, 1);
  // Pure: the same time gives the same frame, in any order.
  assert.deepEqual(buildAt(plan, 1.7), buildAt(plan, 1.7));
});

test("the map's sections start and end on map states or shared moves", () => {
  // every-checkout starts from the node labels and ci leaves on the whip;
  // every bar line between them is a map state.
  const middle = ["every-checkout", "under-cargo-build", "first-build", "same-checkout", "another-worktree", "six-builds", "ci"] as const;
  for (const id of middle.slice(1)) assert.ok(handoffIn(id)?.world, `${id} starts from a map state`);
  for (const id of middle.slice(0, -1)) assert.ok(handoffOut(id)?.world, `${id} ends on a map state`);
  assert.equal(handoffOut("ci")?.meet, "motion");
  assert.equal(handoffIn("every-checkout")?.id, "what|every-checkout");
  // The poster, a beat before another-worktree ends, is its end state.
  assert.equal(sec("another-worktree").end - sec("another-worktree").beat(11), BEAT);
});

test("the pane's window is lit where it stands, the same on both sides of its bar lines, and in full on the poster", () => {
  const scene = (id: SectionId) => scenes.find((s) => s.id === id);
  const lit = (id: SectionId, lt: number) => scene(id)?.lit?.(lt) ?? null;
  const pane: SectionId[] = ["under-cargo-build", "first-build", "same-checkout", "another-worktree", "six-builds"];
  for (const s of SECTIONS) if (!pane.includes(s.id)) assert.equal(scene(s.id)?.lit, undefined, `${s.id} has no pane to light`);
  for (let i = 0; i + 1 < pane.length; i++) {
    const a = lit(pane[i], sec(pane[i]).len - 1e-9);
    const b = lit(pane[i + 1], 0);
    assert.ok(a && b, `${pane[i]}|${pane[i + 1]}`);
    for (const k of ["x", "y", "w", "h", "alpha"] as const) assert.ok(Math.abs(a[k] - b[k]) < 1e-6, `${pane[i]}|${pane[i + 1]} ${k}: ${a[k]} vs ${b[k]}`);
  }
  // Nothing is lit before the terminal swings up, and the six tiles take it with them.
  assert.equal(lit("under-cargo-build", 0), null);
  assert.equal(lit("six-builds", sec("six-builds").len / 2), null);
  const poster = lit("another-worktree", POSTER_TIME - sec("another-worktree").start);
  const aw = HANDOFFS["another-worktree|six-builds"].world?.pane;
  assert.ok(aw);
  assert.deepEqual(poster, { ...paneLayout(aw).window, alpha: 1 });
});
