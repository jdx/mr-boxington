// Glyph-level text layout for kinetic type. Positions come from measuring
// prefixes, so kerning survives when letters are animated one by one.

export const DISPLAY = '"Space Grotesk", "Avenir Next", "Segoe UI", sans-serif';
export const MONO = '"SFMono-Regular", Consolas, "Liberation Mono", monospace';

export const font = (size: number, weight = 600, family = DISPLAY): string =>
  `${weight} ${size}px ${family}`;

export interface Glyph {
  ch: string;
  /** Left edge relative to the start of the line. */
  x: number;
  /** Advance width. */
  w: number;
}

export interface Line {
  glyphs: Glyph[];
  width: number;
}

const cache = new Map<string, Line>();

/** Forget measurements; call once web fonts finish loading. */
export function resetTypeCache(): void {
  cache.clear();
}

/** Measure a line glyph by glyph. `tracking` is extra px after each glyph. */
export function layout(
  ctx: CanvasRenderingContext2D,
  text: string,
  fontSpec: string,
  tracking = 0,
): Line {
  const key = `${fontSpec}|${tracking}|${text}`;
  let line = cache.get(key);
  if (line) return line;
  // Animated sizes or tracking create a key per frame; keep the cache bounded.
  if (cache.size > 4000) cache.clear();
  ctx.save();
  ctx.font = fontSpec;
  const chars = Array.from(text);
  const glyphs: Glyph[] = [];
  let prefix = "";
  for (let i = 0; i < chars.length; i++) {
    const x = ctx.measureText(prefix).width + i * tracking;
    const w = ctx.measureText(chars[i]).width;
    glyphs.push({ ch: chars[i], x, w });
    prefix += chars[i];
  }
  const width = ctx.measureText(text).width + Math.max(0, chars.length - 1) * tracking;
  ctx.restore();
  line = { glyphs, width };
  cache.set(key, line);
  return line;
}

export interface GlyphStyle {
  dx?: number;
  dy?: number;
  /** Rotation about the glyph's baseline center, radians. */
  rot?: number;
  sx?: number;
  sy?: number;
  alpha?: number;
  fill?: string;
}

export interface TextOptions {
  font: string;
  tracking?: number;
  align?: "left" | "center" | "right";
  fill?: string;
  /** Per-glyph animation; return null to skip a glyph. */
  glyph?: (g: Glyph, i: number, n: number) => GlyphStyle | null;
}

/** Draw a line at baseline `y`, optionally animating each glyph. */
export function drawText(
  ctx: CanvasRenderingContext2D,
  text: string,
  x: number,
  y: number,
  opts: TextOptions,
): Line {
  const line = layout(ctx, text, opts.font, opts.tracking ?? 0);
  const align = opts.align ?? "left";
  const x0 = align === "left" ? x : align === "center" ? x - line.width / 2 : x - line.width;
  ctx.save();
  ctx.font = opts.font;
  ctx.textAlign = "left";
  ctx.textBaseline = "alphabetic";
  ctx.fillStyle = opts.fill ?? "#f5ead6";
  if (!opts.glyph && !opts.tracking) {
    ctx.fillText(text, x0, y);
    ctx.restore();
    return line;
  }
  const n = line.glyphs.length;
  line.glyphs.forEach((g, i) => {
    const s = opts.glyph ? opts.glyph(g, i, n) : {};
    if (!s || g.ch === " ") return;
    const a = s.alpha ?? 1;
    if (a <= 0) return;
    ctx.save();
    ctx.globalAlpha *= a;
    if (s.fill) ctx.fillStyle = s.fill;
    const cx = x0 + g.x + g.w / 2 + (s.dx ?? 0);
    ctx.translate(cx, y + (s.dy ?? 0));
    if (s.rot) ctx.rotate(s.rot);
    if (s.sx !== undefined || s.sy !== undefined) ctx.scale(s.sx ?? 1, s.sy ?? 1);
    ctx.fillText(g.ch, -g.w / 2, 0);
    ctx.restore();
  });
  ctx.restore();
  return line;
}
