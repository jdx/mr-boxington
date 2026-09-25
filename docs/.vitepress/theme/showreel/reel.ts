// Composites the eight scenes and the reel's finishing passes. `render` is a
// pure function of time, so playback, scrubbing, and offline export agree.

import { BEAT, CHAPTERS, DURATION, H, PALETTE, type ReelFacts, type Scene, W } from "./bible";
import { rgba } from "./color";
import { grain, vignette } from "./fx";
import { clamp, progress, swiftOut } from "./math";
import { scenes } from "./scenes";
import { LOCKUP } from "./scenes/s3-type";
import { drawText, font, MONO } from "./type";

/**
 * The video's poster frame: the kinetic-type lockup, which says what the reel
 * is about without repeating the landing page's hero.
 */
export const POSTER_TIME = LOCKUP;

export interface Reel {
  duration: number;
  chapters: { id: string; label: string; start: number; end: number }[];
  /** Draw the frame at `t` seconds into a canvas `pw` × `ph` device pixels. */
  render(ctx: CanvasRenderingContext2D, t: number, pw: number, ph: number): void;
}

function sceneAt(t: number): Scene {
  for (const s of scenes) if (t < s.end) return s;
  return scenes[scenes.length - 1];
}

const pad = (n: number, w = 2) => String(Math.floor(n)).padStart(w, "0");

/** Showreel chrome: title, bar counter, chapter label, and timecode. */
function hud(ctx: CanvasRenderingContext2D, t: number): void {
  const alpha =
    progress(0.35, 0.8, t) * (1 - progress(DURATION - 1.7, DURATION - 1.2, t));
  if (alpha <= 0) return;
  ctx.save();
  ctx.globalAlpha = alpha;
  const m = 56;
  const small = font(15, 500, MONO);

  drawText(ctx, "MR BOXINGTON  /  SHOWREEL", m, m + 12, {
    font: small,
    tracking: 2.6,
    fill: rgba(PALETTE.paper, 0.5),
  });

  // Eight bar cells; the current one pulses on every beat.
  const cell = 11;
  const gap = 7;
  const x0 = W - m - 8 * cell - 7 * gap;
  const current = Math.min(7, Math.floor(t / (BEAT * 4)));
  const beatPulse = 1 - (t / BEAT - Math.floor(t / BEAT));
  for (let i = 0; i < 8; i++) {
    const x = x0 + i * (cell + gap);
    const y = m + 1;
    if (i < current) {
      ctx.fillStyle = rgba(PALETTE.amber, 0.55);
      ctx.fillRect(x, y, cell, cell);
    } else if (i === current) {
      ctx.fillStyle = rgba(PALETTE.amberBright, 0.55 + 0.45 * beatPulse ** 2);
      const grow = 2 * beatPulse ** 3;
      ctx.fillRect(x - grow, y - grow, cell + grow * 2, cell + grow * 2);
    } else {
      ctx.strokeStyle = rgba(PALETTE.paper, 0.25);
      ctx.lineWidth = 1.5;
      ctx.strokeRect(x + 0.75, y + 0.75, cell - 1.5, cell - 1.5);
    }
  }

  // Chapter label rolls over on each bar line.
  const idx = CHAPTERS.findIndex((c) => t < c.end);
  const i = idx < 0 ? CHAPTERS.length - 1 : idx;
  const since = t - CHAPTERS[i].start;
  const roll = swiftOut(progress(0, 0.32, since));
  const labelY = H - m;
  ctx.save();
  ctx.beginPath();
  ctx.rect(m - 4, labelY - 30, 520, 42);
  ctx.clip();
  const drawChapter = (k: number, dy: number, a: number) => {
    if (k < 0 || a <= 0) return;
    const c = CHAPTERS[k];
    drawText(ctx, pad(k + 1), m, labelY + dy, {
      font: font(15, 600, MONO),
      tracking: 1,
      fill: rgba(PALETTE.amber, a),
    });
    drawText(ctx, c.label.toUpperCase(), m + 44, labelY + dy, {
      font: small,
      tracking: 2.6,
      fill: rgba(PALETTE.paper, 0.72 * a),
    });
  };
  drawChapter(i - 1, -34 * roll, 1 - roll);
  drawChapter(i, 34 * (1 - roll), roll);
  ctx.restore();

  // SMPTE-style timecode at 24 frames per second.
  const f = Math.floor((t % 1) * 24);
  drawText(ctx, `00:00:${pad(t)}:${pad(f)}`, W - m, labelY, {
    font: small,
    tracking: 1.6,
    align: "right",
    fill: rgba(PALETTE.paper, 0.5),
  });
  ctx.restore();
}

export interface ReelOptions {
  /** Skip grain, vignette, and HUD (for comparing raw scene frames). */
  raw?: boolean;
}

export function createReel(facts: ReelFacts | null, options: ReelOptions = {}): Reel {
  return {
    duration: DURATION,
    chapters: CHAPTERS,
    render(ctx, time, pw, ph) {
      const t = clamp(time, 0, DURATION - 1e-6);
      ctx.setTransform(pw / W, 0, 0, ph / H, 0, 0);
      ctx.globalAlpha = 1;
      ctx.globalCompositeOperation = "source-over";
      ctx.fillStyle = PALETTE.bg;
      ctx.fillRect(0, 0, W, H);
      const s = sceneAt(t);
      ctx.save();
      s.draw(ctx, t - s.start, { W, H, t, facts });
      ctx.restore();
      if (!options.raw) {
        vignette(ctx, W, H, 0.5);
        grain(ctx, W, H, t, 0.07);
        hud(ctx, t);
      }
      // Guard against a scene leaving the transform or blend mode dirty.
      ctx.setTransform(1, 0, 0, 1, 0, 0);
      ctx.globalAlpha = 1;
      ctx.globalCompositeOperation = "source-over";
    },
  };
}

// Re-exported for the player and export harness.
export { factsFromBenchmarks } from "./facts";
export { resetTypeCache } from "./type";
