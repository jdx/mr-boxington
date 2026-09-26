// Renders the showreel to docs/public/showreel-120.mp4 and showreel.mp4 (the
// same reel at 120 and 60 fps), with its poster frame in
// docs/public/showreel-poster.jpg. The landing page plays the 120 fps file
// where the browser can decode it smoothly and the 60 fps file elsewhere, and
// the homepage offers the 60 fps file as og:video so link previews that play
// video (Discord, iMessage, Telegram) can play it too. Every frame of the reel
// is a pure function of time; the score is rendered offline. The docs deploy
// runs this before building; local builds leave the showreel out unless it
// has been rendered.
//
// Needs ffmpeg on PATH and Playwright's Chromium headless shell
// (`aube exec playwright-core install chromium-headless-shell`), or a
// Chromium-based browser at CHROME_PATH.

import { once } from "node:events";
import { spawn } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { availableParallelism, tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { build } from "esbuild";
import { chromium } from "playwright-core";

const here = dirname(fileURLToPath(import.meta.url));
const poster = resolve(here, "../public/showreel-poster.jpg");
// Written outside public/ and renamed over the outputs only once ffmpeg
// succeeds, so a failed or interrupted render never leaves a partial file
// for the build to publish. The cache directory is on the same filesystem
// (so the rename is atomic) and is never deployed.
const staging = resolve(here, "cache/showreel");
const posterPartial = join(staging, "showreel-poster.jpg");
const WIDTH = 1920;
const HEIGHT = 1080;
// One pass renders every frame at 120 fps. Every 60 fps frame is also a
// 120 fps frame (i/60 == 2i/120), so the 60 fps file takes every other one.
// 1080p120 needs H.264 level 5.1; the 60 fps file, which link previews play,
// stays at 4.2.
const FPS = 120;
const VIDEOS = [
  { name: "showreel-120.mp4", fps: 120, level: "5.1" },
  { name: "showreel.mp4", fps: 60, level: "4.2" },
];
// Frames do not depend on each other, so pages render them side by side. On
// a 4-vCPU runner x264 is the floor: two pages render about 12% faster than
// one, and a third adds nothing. Bigger machines get up to four.
const PAGES = Math.max(1, Math.min(4, availableParallelism() >> 1));
// Frames each page has queued, so one page hands back a frame while it draws
// the next.
const DEPTH = 2;
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
    contents: `export { createReel, factsFromBenchmarks, POSTER_TIME, resetTypeCache } from "./theme/showreel/reel.ts";
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

mkdirSync(staging, { recursive: true });
const started = performance.now();
const browser = await chromium.launch({
  executablePath: process.env.CHROME_PATH || undefined,
});
const work = mkdtempSync(join(tmpdir(), "showreel-"));
const encoders = VIDEOS.map((video) => ({
  ...video,
  out: resolve(here, "../public", video.name),
  partial: join(staging, video.name),
}));
try {
  let pageError = null;
  const font = readFileSync(resolve(here, "fonts/SpaceGrotesk.ttf")).toString("base64");
  const results = benchmarkResults();
  const pages = await Promise.all(
    Array.from({ length: PAGES }, async () => {
      const page = await browser.newPage();
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
        { font, results },
      );
      return page;
    }),
  );
  const [page] = pages;
  // The reel's own length (bible.ts), so video and score follow its timing.
  const duration = await page.evaluate(() => window.reel.duration);

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
    { duration, preRoll: PRE_ROLL, rate: SAMPLE_RATE },
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

  // 1080p H.264 High with AAC and the index up front: sharp on the landing
  // page and, at 60 fps, playable by every link preview that plays video.
  // Frames arrive as JPEG, which is BT.601 YCbCr; the tag makes players
  // decode it with the same matrix.
  for (const encoder of encoders) {
    const ffmpeg = spawn(
      "ffmpeg",
      [
        "-y", "-loglevel", "error",
        "-f", "image2pipe", "-framerate", String(encoder.fps), "-c:v", "mjpeg", "-i", "pipe:0",
        "-i", wav,
        "-c:v", "libx264", "-preset", "slow", "-crf", "23",
        "-profile:v", "high", "-level:v", encoder.level, "-pix_fmt", "yuv420p",
        "-colorspace", "bt470bg",
        "-c:a", "aac", "-b:a", "128k",
        "-movflags", "+faststart", "-shortest",
        encoder.partial,
      ],
      { stdio: ["pipe", "inherit", "inherit"] },
    );
    encoder.ffmpeg = ffmpeg;
    // Fail here, inside the try, if ffmpeg cannot start at all (not on PATH).
    await once(ffmpeg, "spawn");
    encoder.exited = once(ffmpeg, "close");
    // If ffmpeg dies mid-stream, the next frame rethrows its broken pipe; keep
    // that error and the pending close from escaping the try as unhandled.
    encoder.exited.catch(() => {});
    encoder.pipeError = null;
    ffmpeg.stdin.on("error", (err) => {
      encoder.pipeError ??= err;
    });
  }

  // JPEG at 0.95 rather than PNG: the grain makes every PNG about 2.5 MB, and
  // encoding and moving one takes about three times as long. Pages finish
  // out of order, so frames are held until the ones before them are written.
  const total = Math.round(FPS * duration);
  const capture = (i) =>
    pages[i % pages.length].evaluate(
      ({ t, w, h }) => {
        window.reel.render(window.ctx, t, w, h);
        return document.getElementById("reel").toDataURL("image/jpeg", 0.95);
      },
      { t: i / FPS, w: WIDTH, h: HEIGHT },
    );
  const pending = new Map();
  let queued = 0;
  for (let i = 0; i < total; i++) {
    for (; queued < Math.min(total, i + pages.length * DEPTH); queued++) {
      const frame = capture(queued);
      // Awaited in order below; until then a failure must not go unhandled.
      frame.catch(() => {});
      pending.set(queued, frame);
    }
    const jpeg = await pending.get(i);
    pending.delete(i);
    const frame = Buffer.from(jpeg.slice(jpeg.indexOf(",") + 1), "base64");
    const drained = [];
    for (const encoder of encoders) {
      if (i % (FPS / encoder.fps)) continue;
      if (encoder.pipeError) throw encoder.pipeError;
      if (!encoder.ffmpeg.stdin.write(frame)) drained.push(once(encoder.ffmpeg.stdin, "drain"));
    }
    await Promise.all(drained);
    if ((i + 1) % (FPS * 10) === 0) {
      const elapsed = (performance.now() - started) / 1000;
      console.log(`Rendered ${i + 1} of ${total} frames in ${elapsed.toFixed(0)} s`);
    }
  }
  for (const encoder of encoders) encoder.ffmpeg.stdin.end();
  for (const encoder of encoders) {
    const [code] = await encoder.exited;
    if (code !== 0) throw new Error(`ffmpeg exited with ${code} for ${encoder.name}`);
  }
  if (pageError) throw pageError;

  // The poster the player shows until someone presses play.
  const jpeg = await page.evaluate(
    ({ w, h }) => {
      window.reel.render(window.ctx, Showreel.POSTER_TIME, w, h);
      return document.getElementById("reel").toDataURL("image/jpeg", 0.9);
    },
    { w: WIDTH, h: HEIGHT },
  );
  writeFileSync(posterPartial, Buffer.from(jpeg.slice(jpeg.indexOf(",") + 1), "base64"));
  if (pageError) throw pageError;
  for (const encoder of encoders) renameSync(encoder.partial, encoder.out);
  renameSync(posterPartial, poster);
  const elapsed = (performance.now() - started) / 1000;
  console.log(
    `Rendered ${encoders.map((encoder) => encoder.out).join(", ")} and ${poster} in ${elapsed.toFixed(0)} s`,
  );
} finally {
  // An encoder still running here was cut off by an error; its output is
  // discarded, so stop it without letting it finish the file.
  for (const encoder of encoders) encoder.ffmpeg?.kill("SIGKILL");
  await browser.close();
  rmSync(work, { recursive: true, force: true });
  for (const encoder of encoders) rmSync(encoder.partial, { force: true });
  rmSync(posterPartial, { force: true });
}
