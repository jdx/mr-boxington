// Renders the landing-page showreel to docs/public/showreel.mp4, which the
// homepage offers as og:video so link previews that play video (Discord,
// iMessage, Telegram) can play the reel. Every frame is a pure function of
// time, so this is the picture the page draws, with the score rendered
// offline. The docs deploy runs this before building; local builds skip the
// video unless it has been rendered.
//
// Needs ffmpeg on PATH and Playwright's Chromium headless shell
// (`aube exec playwright-core install chromium-headless-shell`), or a
// Chromium-based browser at CHROME_PATH.

import { once } from "node:events";
import { spawn } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { build } from "esbuild";
import { chromium } from "playwright-core";

const here = dirname(fileURLToPath(import.meta.url));
const out = resolve(here, "../public/showreel.mp4");
const WIDTH = 1280;
const HEIGHT = 720;
const FPS = 60;
const DURATION = 15;
// Rendered before the reel starts and trimmed, so the score's compressor
// lookahead can place the first sounds exactly.
const PRE_ROLL = 0.2;
const SAMPLE_RATE = 48000;

// The same published run the benchmarks page and the live reel read, with the
// validity check from benchmarks.data.ts: a failed or unknown run shows no
// numbers.
function benchmarkResults() {
  try {
    const results = JSON.parse(
      readFileSync(resolve(here, "../../benchmarks/results.json"), "utf8"),
    );
    return [1, 2].includes(results.schema) && results.passed ? results : null;
  } catch {
    return null;
  }
}

const bundle = await build({
  stdin: {
    contents: `export { createReel, factsFromBenchmarks, resetTypeCache } from "./theme/showreel/reel.ts";
export { playScore } from "./theme/showreel/audio.ts";`,
    resolveDir: here,
    loader: "ts",
  },
  bundle: true,
  format: "iife",
  globalName: "Showreel",
  target: "es2022",
  write: false,
  logLevel: "error",
});

const browser = await chromium.launch({
  executablePath: process.env.CHROME_PATH || undefined,
});
const work = mkdtempSync(join(tmpdir(), "showreel-"));
try {
  const page = await browser.newPage();
  let pageError = null;
  page.on("pageerror", (err) => {
    pageError ??= err;
  });
  await page.setContent(
    `<canvas id="reel" width="${WIDTH}" height="${HEIGHT}"></canvas>`,
  );
  await page.addScriptTag({ content: bundle.outputFiles[0].text });
  await page.evaluate(
    async ({ font, results }) => {
      const bytes = Uint8Array.from(atob(font), (c) => c.charCodeAt(0));
      const face = new FontFace("Space Grotesk", bytes, { weight: "300 700" });
      document.fonts.add(await face.load());
      await document.fonts.load('500 16px "SFMono-Regular", Consolas, "Liberation Mono", monospace');
      Showreel.resetTypeCache();
      const canvas = document.getElementById("reel");
      window.reel = Showreel.createReel(Showreel.factsFromBenchmarks(results));
      window.ctx = canvas.getContext("2d", { alpha: false });
    },
    {
      font: readFileSync(resolve(here, "fonts/SpaceGrotesk.ttf")).toString("base64"),
      results: benchmarkResults(),
    },
  );

  // The score, as 16-bit stereo PCM.
  const wav = join(work, "score.wav");
  const pcm = await page.evaluate(
    async ({ duration, preRoll, rate }) => {
      const ac = new OfflineAudioContext(2, Math.ceil(rate * (duration + preRoll)), rate);
      Showreel.playScore(ac, ac.destination, 0, preRoll);
      const buffer = await ac.startRendering();
      const skip = Math.round(preRoll * rate);
      const frames = Math.round(duration * rate);
      const left = buffer.getChannelData(0);
      const right = buffer.getChannelData(1);
      const data = new Int16Array(frames * 2);
      for (let i = 0; i < frames; i++) {
        data[i * 2] = Math.max(-1, Math.min(1, left[i + skip])) * 32767;
        data[i * 2 + 1] = Math.max(-1, Math.min(1, right[i + skip])) * 32767;
      }
      let binary = "";
      const bytes = new Uint8Array(data.buffer);
      for (let i = 0; i < bytes.length; i += 0x8000) {
        binary += String.fromCharCode.apply(null, bytes.subarray(i, i + 0x8000));
      }
      return btoa(binary);
    },
    { duration: DURATION, preRoll: PRE_ROLL, rate: SAMPLE_RATE },
  );
  const samples = Buffer.from(pcm, "base64");
  const header = Buffer.alloc(44);
  header.write("RIFF", 0);
  header.writeUInt32LE(36 + samples.length, 4);
  header.write("WAVEfmt ", 8);
  header.writeUInt32LE(16, 16);
  header.writeUInt16LE(1, 20);
  header.writeUInt16LE(2, 22);
  header.writeUInt32LE(SAMPLE_RATE, 24);
  header.writeUInt32LE(SAMPLE_RATE * 4, 28);
  header.writeUInt16LE(4, 32);
  header.writeUInt16LE(16, 34);
  header.write("data", 36);
  header.writeUInt32LE(samples.length, 40);
  writeFileSync(wav, Buffer.concat([header, samples]));

  // 720p60 H.264 High with AAC and the index up front: small (about 2 MB)
  // and playable by every link preview that plays video.
  const ffmpeg = spawn(
    "ffmpeg",
    [
      "-y", "-loglevel", "error",
      "-f", "image2pipe", "-framerate", String(FPS), "-c:v", "png", "-i", "pipe:0",
      "-i", wav,
      "-c:v", "libx264", "-preset", "slow", "-crf", "23",
      "-profile:v", "high", "-level:v", "4.0", "-pix_fmt", "yuv420p",
      "-c:a", "aac", "-b:a", "128k",
      "-movflags", "+faststart", "-shortest",
      out,
    ],
    { stdio: ["pipe", "inherit", "inherit"] },
  );
  const exited = once(ffmpeg, "close");
  for (let i = 0; i < FPS * DURATION; i++) {
    const png = await page.evaluate(
      ({ t, w, h }) => {
        window.reel.render(window.ctx, t, w, h);
        return document.getElementById("reel").toDataURL("image/png");
      },
      { t: i / FPS, w: WIDTH, h: HEIGHT },
    );
    const frame = Buffer.from(png.slice(png.indexOf(",") + 1), "base64");
    if (!ffmpeg.stdin.write(frame)) await once(ffmpeg.stdin, "drain");
  }
  ffmpeg.stdin.end();
  const [code] = await exited;
  if (pageError) throw pageError;
  if (code !== 0) throw new Error(`ffmpeg exited with ${code}`);
  console.log(`Rendered ${out}`);
} finally {
  await browser.close();
  rmSync(work, { recursive: true, force: true });
}
