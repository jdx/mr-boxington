// Composites the scenes, their captions, and the reel's finishing passes.
// `render` is a pure function of time, so playback, scrubbing, and offline
// export agree.

import { CHAPTERS, DURATION, H, PALETTE, type ReelFacts, type Scene, sec, W } from "./bible";
import { grain, vignette } from "./fx";
import { clamp } from "./math";
import { scenes } from "./scenes";
import { drawCaptions, timeCaptions } from "./type";

/**
 * The video's poster frame: the end of "Another worktree", where the box is
 * taped with its strawberry and the caption says what the landing page's
 * hero does not.
 */
export const POSTER_TIME = sec("another-worktree").beat(11);

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

export interface ReelOptions {
  /** Skip captions, grain, and vignette (for comparing raw scene frames). */
  raw?: boolean;
}

export function createReel(facts: ReelFacts | null, options: ReelOptions = {}): Reel {
  // Captions can depend on the numbers, so they are placed once per reel.
  const captions = scenes.flatMap((s) => timeCaptions(sec(s.id), s.captions?.(facts) ?? []));
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
        // Over the vignette, so a caption reads the same at the frame's edge;
        // under the grain, so it sits in the picture.
        drawCaptions(ctx, t, captions);
        grain(ctx, W, H, t, 0.07);
      }
      // Guard against a scene leaving the transform or blend mode dirty.
      ctx.setTransform(1, 0, 0, 1, 0, 0);
      ctx.globalAlpha = 1;
      ctx.globalCompositeOperation = "source-over";
    },
  };
}

// Re-exported for the video renderer.
export { factsFromBenchmarks } from "./facts";
export { resetTypeCache } from "./type";
