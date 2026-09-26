// A stand-in for a section whose scene is not built yet: the section's name
// and what the viewer should learn from it, the copy the storyboard puts on
// screen, and a beat ruler with a playhead and the caption spans, so the
// section's captions and timing can be reviewed in place. Each new
// section's scene file is one of these until its real scene replaces it.
//
// A stub starts on the handoff frame it inherits (map.ts handoffIn) and
// ends on the one it owes the next section (handoffOut), so the reel stays
// seamless on every bar line while real scenes replace stubs one by one:
// it fades from the first to its placeholder over a beat and a half, fades
// the second in over the second-to-last beat, and holds it for the last.
// A section that leaves on the whip whips its placeholder out instead.

import { BEAT, PALETTE, type Scene, type SceneEnv, SECTIONS, type SectionId, sec, WHIP } from "../bible";
import { rgba } from "../color";
import { roundedRect } from "../fx";
import { drawWhip, drawWhipOut, handoffIn, handoffOut, WHIP_AT, WHIP_WIND } from "../map";
import { smoothstep } from "../math";
import { type Caption, DETAIL, drawText, drawWords, entrance, font, LABEL, layout, MONO, type WordStyle } from "../type";

/** Copy on screen besides the captions, in section-local beats. */
export interface StubCopy {
  text: string;
  /** Beat its last word lands on. */
  in: number;
  /** Beat it starts to wipe away; it holds to the end without one. */
  out?: number;
  style?: WordStyle;
}

export interface Stub {
  id: SectionId;
  /** What the viewer learns, from the storyboard. */
  learns: string;
  captions?: Scene["captions"];
  copy?: (env: SceneEnv) => readonly StubCopy[];
  /** @deprecated Every stub now starts on its handoff; ignored. */
  nodesIn?: boolean;
  /** @deprecated A stub owing the whip whips out on its own; ignored. */
  whipOut?: boolean;
}

// The picture area: everything above the captions' band.
const FX = 160;
const FY = 72;
const FW = 1600;
const FH = 640;
const PAD = 48;
const RULER_Y = FY + FH - 76;

/** Beats the stub takes to leave its first handoff frame. */
const FADE_IN = 1.5;

/** Break `text` into lines no wider than `width` in `spec`. */
function wrap(ctx: CanvasRenderingContext2D, text: string, spec: string, width: number): string[] {
  const lines: string[] = [];
  let line = "";
  for (const word of text.split(" ")) {
    const next = line ? `${line} ${word}` : word;
    if (line && layout(ctx, next, spec).width > width) {
      lines.push(line);
      line = word;
    } else {
      line = next;
    }
  }
  if (line) lines.push(line);
  return lines;
}

/** The section's beats, bar numbers on the reel's count, the caption spans, and the playhead. */
function ruler(ctx: CanvasRenderingContext2D, stub: Stub, lt: number, env: SceneEnv): void {
  const s = sec(stub.id);
  const beats = s.bars * 4;
  const x0 = FX + PAD;
  const pb = (FW - 2 * PAD) / beats;
  const firstBar = Math.round(s.start / (4 * BEAT)) + 1;
  ctx.fillStyle = rgba(PALETTE.paper, 0.3);
  for (let k = 0; k <= beats; k++) {
    const onBar = k % 4 === 0;
    ctx.fillRect(x0 + k * pb - 1, RULER_Y - (onBar ? 18 : 8), 2, onBar ? 36 : 16);
    if (onBar && k < beats) {
      drawText(ctx, `bar ${firstBar + k / 4}`, x0 + k * pb + 8, RULER_Y + 40, {
        font: font(20, 500, MONO),
        fill: PALETTE.text3,
      });
    }
  }
  // Each caption line from its first word to the start of its wipe, the
  // landing marked.
  const caps: readonly Caption[] = stub.captions?.(env.facts) ?? [];
  for (const c of caps) {
    c.lines.forEach((l, i) => {
      const a = entrance(l.text, l.in * BEAT) / BEAT;
      const y = RULER_Y - 44 - (c.lines.length - 1 - i) * 12;
      ctx.fillStyle = rgba(PALETTE.paper, 0.35);
      ctx.fillRect(x0 + a * pb, y, (c.out - a) * pb, 6);
      ctx.fillStyle = PALETTE.paper;
      ctx.fillRect(x0 + l.in * pb - 1, y - 3, 3, 12);
    });
  }
  ctx.fillStyle = PALETTE.amber;
  ctx.fillRect(x0 + (lt / BEAT) * pb - 1.5, RULER_Y - 64, 3, 88);
  drawText(ctx, `b ${(lt / BEAT).toFixed(2)} / ${beats}`, FX + FW - PAD, FY + 76, {
    font: font(28, 500, MONO),
    fill: PALETTE.amber,
    align: "right",
  });
}

function placeholder(ctx: CanvasRenderingContext2D, stub: Stub, lt: number, env: SceneEnv): void {
  const n = SECTIONS.findIndex((s) => s.id === stub.id) + 1;
  ctx.save();
  ctx.strokeStyle = rgba(PALETTE.paper, 0.22);
  ctx.lineWidth = 2;
  ctx.setLineDash([12, 10]);
  roundedRect(ctx, FX, FY, FW, FH, 20);
  ctx.stroke();
  ctx.restore();

  const top = FY + 76;
  drawText(ctx, String(n).padStart(2, "0"), FX + PAD, top, { font: font(40, 600, MONO), fill: PALETTE.amber });
  drawText(ctx, sec(stub.id).label, FX + PAD + 76, top, { font: LABEL.font, fill: PALETTE.paper });
  drawText(ctx, "placeholder", FX + PAD + 76, top + 44, { font: font(24, 500, MONO), fill: PALETTE.text3 });
  const spec = DETAIL.font;
  wrap(ctx, stub.learns, spec, FW - 2 * PAD).forEach((line, i) => {
    drawText(ctx, line, FX + PAD, top + 120 + i * 52, { font: spec, fill: PALETTE.text2 });
  });

  const copy = stub.copy?.(env) ?? [];
  copy.forEach((c, i) => {
    drawWords(ctx, c.text, FX + PAD, top + 280 + i * 76, c.style ?? LABEL, lt, c.in * BEAT, (c.out ?? Infinity) * BEAT);
  });
  ruler(ctx, stub, lt, env);
}

export function stubScene(stub: Stub): Scene {
  const S = sec(stub.id);
  const from = handoffIn(stub.id);
  const to = handoffOut(stub.id);
  const whips = to?.id === "ci|next-push";
  return {
    id: S.id,
    start: S.start,
    end: S.end,
    draw(ctx, lt, env) {
      const beats = S.len / BEAT;
      const fill = () => {
        ctx.fillStyle = PALETTE.bg;
        ctx.fillRect(0, 0, env.W, env.H);
      };
      // The body: the inherited frame giving way to the placeholder.
      const body = () => {
        const k = from ? smoothstep(0, FADE_IN * BEAT, lt) : 1;
        if (from && k < 1) from.draw(ctx, env);
        if (k <= 0) return;
        ctx.save();
        ctx.globalAlpha = k;
        if (k < 1) fill();
        placeholder(ctx, stub, lt, env);
        ctx.restore();
      };
      fill();
      if (whips && env.t >= WHIP_AT - WHIP - WHIP_WIND) {
        // Smeared copies of the placeholder, then the speed lines over them.
        drawWhipOut(ctx, env.t, body);
        drawWhip(ctx, env.t);
        return;
      }
      body();
      // The owed frame fades in over the second-to-last beat and holds.
      const b = to ? smoothstep((beats - 2) * BEAT, (beats - 1) * BEAT, lt) : 0;
      if (to && b > 0) {
        ctx.save();
        ctx.globalAlpha = b;
        to.draw(ctx, env);
        ctx.restore();
      }
    },
    captions: stub.captions,
  };
}
