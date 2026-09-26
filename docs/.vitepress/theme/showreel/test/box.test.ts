// The logo character's geometry against docs/public/logo.svg, in numbers.
// logo.test.ts compares the pixels.

import assert from "node:assert/strict";
import { test } from "node:test";
import {
  boxSilhouette,
  drawBox,
  FRONT_CAM,
  faceToScreen,
  LID_STEP,
  LOGO_POSE,
  logoCam,
  monocleScreen,
  mouth,
} from "../box";
import { flatCard, FLAT_BAND, tone, View } from "../space";

const close = (a: number, b: number, what: string) => assert.ok(Math.abs(a - b) < 1e-9, `${what}: ${a} vs ${b}`);

test("logoCam draws the box's corners where logo.svg does", () => {
  // In logo units: a 128 px square at the origin.
  const view = new View(logoCam(0, 0, 128));
  const at = (p: [number, number, number]) => view.project(p);
  const want: [[number, number, number], number, number][] = [
    [[-0.5, 0, 0.5], 4, 124],
    [[0.5, 0, 0.5], 124, 124],
    [[-0.5, 0.85, 0.5], 4, 22],
    [[0.5, 0.85, 0.5], 124, 22],
    [[-0.5, 0.85, -0.5], 14, 7],
    [[0.5, 0.85, -0.5], 114, 7],
    // The lid's front edge band is 5 units deep.
    [[0, 0.85 - 5 / 120, 0.5], 64, 27],
  ];
  for (const [p, x, y] of want) {
    close(at(p).x, x, `${p} x`);
    close(at(p).y, y, `${p} y`);
  }
  // The silhouette is the logo's outline, less its rounded bottom corners.
  const hull = boxSilhouette(view, LOGO_POSE).map((p) => [Math.round(p.x * 1e6) / 1e6, Math.round(p.y * 1e6) / 1e6]);
  assert.deepEqual(
    hull.sort((a, b) => a[0] - b[0] || a[1] - b[1]),
    [
      [4, 22],
      [4, 124],
      [14, 7],
      [114, 7],
      [124, 22],
      [124, 124],
    ],
  );
});

test("face coordinates are logo units, and the monocle is logo.svg's", () => {
  const view = new View(logoCam(10, 20, 256));
  const p = faceToScreen(view, LOGO_POSE, 33, 66);
  close(p.x, 10 + 33 * 2, "x");
  close(p.y, 20 + 66 * 2, "y");
  const m = monocleScreen(view, LOGO_POSE);
  close(m.x, 10 + 86 * 2, "monocle x");
  close(m.y, 20 + 62 * 2, "monocle y");
  close(m.r, 24.5 * 2, "monocle r");
});

test("FRONT_CAM is the logo view, 480 px square in the middle of the frame", () => {
  const view = new View(FRONT_CAM);
  close(view.project([-0.5, 0.85, -0.5]).x, 720 + 14 * 3.75, "lid x");
  close(view.project([0.5, 0, 0.5]).y, 300 + 124 * 3.75, "floor y");
  assert.deepEqual([FRONT_CAM.yaw, FRONT_CAM.pitch, FRONT_CAM.persp], [0, 0, 5.5]);
});

test("the flat material lands on the logo's colours from straight ahead", () => {
  const view = new View(FRONT_CAM);
  assert.equal(flatCard(tone(view, [0, 0, 1])), "#e6ad54");
  assert.equal(flatCard(tone(view, [0, 1, 0])), "#f2c479");
  assert.equal(flatCard(tone(view, [0, 0, 1]) - FLAT_BAND), "#cf8f35");
});

test("the lid hinges up a step at a time and opens a mouth", () => {
  const view = new View(logoCam(0, 0, 128));
  const shut = boxSilhouette(view, LOGO_POSE);
  const open = boxSilhouette(view, { ...LOGO_POSE, lid: 4 });
  const top = (h: { y: number }[]) => Math.min(...h.map((p) => p.y));
  // Four steps raise the lid a quarter of the box's width.
  const lid = view.project([0.5, 0.85 + 4 * LID_STEP, -0.5]).y;
  close(top(open), lid, "raised lid");
  assert.ok(top(open) < top(shut) - 20);
  // The mouth sits on the rim, 5 units below the top of the shut lid.
  const m = mouth(view, { ...LOGO_POSE, lid: 4 });
  assert.ok(m.pts.every((p) => p.y > 7 && p.y < 27.001), JSON.stringify(m.pts));
});

test("the inside is drawn only while the lid is open", () => {
  let calls = 0;
  // A context that accepts every call and setting.
  const settings = ["globalAlpha", "fillStyle", "strokeStyle", "lineWidth", "lineCap", "lineJoin"];
  const ctx = new Proxy(
    {},
    {
      get: (_, key) =>
        key === "createLinearGradient" || key === "createRadialGradient"
          ? () => ({ addColorStop() {} })
          : typeof key === "string" && !settings.includes(key)
            ? () => {}
            : 1,
      set: () => true,
    },
  ) as unknown as CanvasRenderingContext2D;
  const inside = () => calls++;
  const view = new View(FRONT_CAM);
  drawBox(ctx, view, { ...LOGO_POSE, face: null, inside });
  assert.equal(calls, 0);
  drawBox(ctx, view, { ...LOGO_POSE, face: null, lid: 2, inside });
  assert.equal(calls, 1);
});
