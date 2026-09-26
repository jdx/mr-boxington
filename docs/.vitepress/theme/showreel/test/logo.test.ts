// The logo character in its logo pose, drawn by drawBox in Chromium, against
// docs/public/logo.svg drawn by the same browser at the same size. The two
// may differ only where antialiasing does: along edges, and by a little.
// Skipped when no Chromium is installed (`aube exec playwright-core install
// chromium-headless-shell`, or a Chromium at CHROME_PATH), unless
// SHOWREEL_REQUIRE_CHROMIUM is set: CI sets it whenever it installs one, so a
// broken install fails here instead of skipping the check.

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { join } from "node:path";
import { test } from "node:test";
import { REPO, SHOWREEL } from "./repo";

/** Largest difference an edge pixel may show, in 8-bit levels of premultiplied colour or alpha. */
const EDGE = 32;
/** Largest difference a pixel away from every edge may show. */
const INSIDE = 1;

// From docs/, where the browser and bundler are installed. Loaded at run
// time, not bundled into the test.
const load = createRequire(join(REPO, "docs/package.json"));

test("drawBox(LOGO_POSE) reproduces logo.svg", async (t) => {
  const { chromium } = load("playwright-core") as typeof import("playwright-core");
  const { build } = load("esbuild") as typeof import("esbuild");
  let browser: import("playwright-core").Browser;
  try {
    browser = await chromium.launch({ executablePath: process.env.CHROME_PATH || undefined });
  } catch (err) {
    if (process.env.SHOWREEL_REQUIRE_CHROMIUM) throw err;
    t.skip(`no Chromium to draw in: ${String(err).split("\n")[0]}`);
    return;
  }
  try {
    const bundle = await build({
      stdin: {
        contents: `export { drawBox, FRONT_CAM, LOGO_POSE, logoCam } from "./box"; export { View } from "./space";`,
        resolveDir: SHOWREEL,
        loader: "ts",
      },
      bundle: true,
      format: "iife",
      globalName: "Box",
      target: "es2022",
      write: false,
      logLevel: "error",
    });
    const page = await browser.newPage();
    await page.setContent("<body></body>");
    await page.addScriptTag({ content: bundle.outputFiles[0].text });
    const svg = readFileSync(join(REPO, "docs/public/logo.svg"), "utf8");
    // FRONT_CAM (the logo 480 px square at (720, 300) of the frame), then
    // logoCam at whole and fractional pixels per logo unit.
    for (const [size, frame] of [[480, true], [128, false], [300, false], [512, false]] as const) {
      const r = await page.evaluate(
        async ({ svg, size, frame }) => {
          const img = new Image();
          img.src = `data:image/svg+xml;base64,${btoa(svg)}`;
          await img.decode();
          const canvas = (w: number, h: number) => {
            const c = document.createElement("canvas");
            c.width = w;
            c.height = h;
            return c.getContext("2d") as CanvasRenderingContext2D;
          };
          const [w, h, x, y] = frame ? [1920, 1080, 720, 300] : [size, size, 0, 0];
          const want = canvas(w, h);
          want.drawImage(img, x, y, size, size);
          const got = canvas(w, h);
          const cam = frame ? Box.FRONT_CAM : Box.logoCam(0, 0, size);
          Box.drawBox(got, new Box.View(cam), Box.LOGO_POSE);
          const A = want.getImageData(0, 0, w, h).data;
          const B = got.getImageData(0, 0, w, h).data;
          // Premultiplied, so a transparent pixel's colour does not count.
          const diff = (i: number) => {
            let d = Math.abs(A[i + 3] - B[i + 3]);
            for (let c = 0; c < 3; c++) d = Math.max(d, Math.abs((A[i + c] * A[i + 3] - B[i + c] * B[i + 3]) / 255));
            return d;
          };
          // Away from every edge: the reference's 3x3 neighbourhood is one colour.
          const inside = (x: number, y: number) => {
            for (let dy = -1; dy <= 1; dy++)
              for (let dx = -1; dx <= 1; dx++) {
                const [X, Y] = [x + dx, y + dy];
                if (X < 0 || Y < 0 || X >= w || Y >= h) continue;
                const j = (Y * w + X) * 4;
                for (let c = 0; c < 4; c++) if (A[j + c] !== A[(y * w + x) * 4 + c]) return false;
              }
            return true;
          };
          let edge = 0;
          let interior = 0;
          let over = 0;
          let sq = 0;
          for (let py = 0; py < h; py++)
            for (let px = 0; px < w; px++) {
              const d = diff((py * w + px) * 4);
              sq += d * d;
              if (d > 2) over++;
              if (inside(px, py)) interior = Math.max(interior, d);
              else edge = Math.max(edge, d);
            }
          const mse = sq / (w * h);
          return { edge, interior, over, psnr: mse ? 10 * Math.log10((255 * 255) / mse) : Infinity };
        },
        { svg, size, frame },
      );
      const what = `${size} px${frame ? " (FRONT_CAM)" : ""}`;
      t.diagnostic(
        `${what}: edge max ${r.edge.toFixed(1)}, interior max ${r.interior.toFixed(1)}, ` +
          `${r.over} px over 2 levels, PSNR ${r.psnr.toFixed(1)} dB`,
      );
      assert.ok(r.interior <= INSIDE, `${what}: a pixel away from any edge differs by ${r.interior}`);
      assert.ok(r.edge <= EDGE, `${what}: an edge pixel differs by ${r.edge}`);
    }
  } finally {
    await browser.close();
  }
});

declare const Box: typeof import("../box") & typeof import("../space");
