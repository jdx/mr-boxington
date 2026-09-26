// Scene 14, "End card": the flat amber silhouette the morph set inflates
// into the logo box on the downbeat, and the camera pushes in while his face
// pops back on, one feature per sixteenth: the eye under its flat eyelid,
// the mustache, the monocle on its chain, a glint, the rosy cheeks. The tape
// runs over the lid and a strawberry lands beside its tab, as it does on a
// build that hit the cache: a nod to the original cardboard Mr. Boxington.
// The camera snaps back as the wordmark, the line, the install command, and
// the address arrive below him. He glints once and blinks once, and the card
// ends on a still frame.

import { BEAT, drawLogoBox, END_CAM, END_POSE, PALETTE, type Scene, sec } from "../bible";
import { type BoxPose, boxSilhouette, type FaceParams, faceToScreen, LOGO_FACE, logoCam, sparkle } from "../box";
import { mix, rgba } from "../color";
import { flash, glow, makeCanvas, roundedRect, shake } from "../fx";
import {
  clamp,
  cubicBezier,
  hash,
  inOutSine,
  inQuad,
  keys,
  lerp,
  outCubic,
  outQuad,
  progress,
  pulse,
  smoothstep,
  spring,
  swiftOut,
  TAU,
  wobble,
} from "../math";
import { type Camera, mixCamera, polygon, type Projected, View } from "../space";
import { CAPTION, drawWords, entrance, font, layout, MONO, wordStyle } from "../type";

const S = sec("end");

/** Local time of beat `n` of the section. */
const b = (n: number): number => n * BEAT;

// Accents, local seconds. The score (score/logo.ts) is written to these.
/** The face, one feature per sixteenth. */
export const T_EYE = b(0.5);
export const T_MUST = b(0.75);
export const T_MONO = b(1);
export const T_SHINE = b(1.25);
export const T_BLUSH = b(1.5);
/** The tape runs over the lid and down the front, fastest into its cue. */
export const T_TAPE = b(1.75);
/** The strawberry lands on its seat beside the tape's tab. */
export const T_BERRY = b(2);
/** The camera snaps back from the close-up to the card. */
export const T_SNAP = b(2.25);
/** The card's lines land: the wordmark, the line, the command, the address. */
export const T_WORD = b(2.75);
export const T_LINE = b(3.25);
export const T_INSTALL = b(3.75);
export const T_URL = b(4);
/** One glint across the monocle, and one blink. */
export const T_GLINT = b(8);
export const T_BLINK = b(10);
/** Nothing moves from here: the card ends on a still frame. */
export const STILL = b(10.75);

/** Pops start one frame early with a kick, so the anchor frame already reads. */
const LEAD = 1 / 60;
const pop = (lt: number, anchor: number, f: number, z: number, kick = 22) =>
  spring(lt - anchor + LEAD, f, z, kick);

// The camera. The morph's silhouette is END_CAM, the logo view of END_POSE
// tipped to look at the lid; the card is the logo's own untipped view,
// described (as box.ts logoCam is) by the square the logo would fill on
// screen. The tip straightens out as the camera pushes in on the face, and
// the snap back lands on the card's square.

/** The logo's square on screen: its top left corner and side, px. */
interface Square {
  x: number;
  y: number;
  size: number;
}
/** END_CAM's framing as a square (bible.ts END_LOGO_CAM). */
const START: Square = { x: 750, y: 185, size: 420 };
/** The push-in holds the face's middle (logo 64, 73) still. */
const zoomed = (sq: Square, size: number): Square => ({
  x: sq.x + (64 * (sq.size - size)) / 128,
  y: sq.y + (73 * (sq.size - size)) / 128,
  size,
});
/** The close-up: 680 px, the face's middle brought down to (960, 570). */
const PUSHED: Square = { x: 960 - (64 * 680) / 128, y: 570 - (73 * 680) / 128, size: 680 };
/** The card: the box above the lockup, its base at y 474. */
export const CARD_SQ: Square = { x: 960 - 170, y: 474 - (124 * 340) / 128, size: 340 };
const pushIn = cubicBezier(0.3, 0, 0.2, 1);
/** The snap back: most of the way in its first frames, then a long settle. */
const snapBack = cubicBezier(0.1, 0.8, 0.2, 1);
export const SNAP_DUR = 0.52;

function squareAt(lt: number): Square {
  const p = pushIn(progress(0.03, b(0.75), lt));
  const creep = 0.04 * inOutSine(progress(b(0.75), T_SNAP, lt));
  const near: Square = { x: lerp(START.x, PUSHED.x, p), y: lerp(START.y, PUSHED.y, p), size: lerp(START.size, PUSHED.size, p) };
  const from = zoomed(near, near.size * (1 + creep));
  const k = snapBack(progress(T_SNAP, T_SNAP + SNAP_DUR, lt));
  return {
    x: lerp(from.x, CARD_SQ.x, k),
    y: lerp(from.y, CARD_SQ.y, k),
    size: lerp(from.size, CARD_SQ.size, k),
  };
}

/** The logo view of END_POSE (standing half a box width down) filling square `sq`. */
function squareCam(sq: Square): Camera {
  const L = logoCam(sq.x, sq.y, sq.size);
  const [x, y, z] = L.target ?? [0, 0, 0];
  return { ...L, target: [x, y + END_POSE.pos[1], z] };
}

/** The tip straightens out over the push. */
const untip = (lt: number): number => smoothstep(0, b(0.9), lt);

function cameraAt(lt: number): Camera {
  return mixCamera(END_CAM, squareCam(squareAt(lt)), untip(lt));
}

// The box. It inflates about its middle on the downbeat and springs back,
// and takes a small squash from each feature landing and from the berry.
const MID = END_POSE.pos[1] + 0.425;
const sizeAt = (lt: number): number => 1 + 0.12 * wobble(lt, 0, 2.6, 6.5);

function squashAt(lt: number): number {
  let s = 1 + 0.05 * wobble(lt, 0.05, 3.2, 8);
  s -= 0.02 * pulse(lt, T_EYE, 0.02, 0.06);
  s -= 0.018 * pulse(lt, T_MUST, 0.02, 0.06);
  s -= 0.02 * pulse(lt, T_MONO, 0.015, 0.05);
  s -= 0.015 * pulse(lt, T_BLUSH, 0.015, 0.06);
  s -= 0.025 * pulse(lt, T_TAPE + 0.03, 0.02, 0.07);
  s -= 0.05 * pulse(lt, T_BERRY, 0.01, 0.07);
  return s;
}

/** Tape: laid from the lid's back edge, fastest into the cue, landing soft down the front. */
export const TAPE_LEAD = 0.05;
const tapeAt = keys([
  [T_TAPE - TAPE_LEAD, 0],
  [T_TAPE + 0.02, 0.62, inQuad],
  [T_TAPE + 0.2, 1, swiftOut],
]);

// The strawberry drops from above the frame onto its seat on the lid,
// landing on its cue, and bounces twice. Heights in logo units.
const BERRY_DROP = 260;
const BERRY_G = 5200;
const BERRY_FALL = Math.sqrt((2 * BERRY_DROP) / BERRY_G);
/** How much of the landing speed each bounce keeps. */
const BERRY_BOUNCE = [0.26, 0.09] as const;
function berryLift(lt: number): number {
  const d = lt - T_BERRY;
  if (d < 0) return 0.5 * BERRY_G * d * d;
  let s = d;
  const v = BERRY_G * BERRY_FALL;
  for (const e of BERRY_BOUNCE) {
    const u = v * e;
    const th = (2 * u) / BERRY_G;
    if (s < th) return u * s - 0.5 * BERRY_G * s * s;
    s -= th;
  }
  return 0;
}
/** When the strawberry touches its seat again after each bounce, local seconds. */
export function berryBounces(): number[] {
  const v = BERRY_G * BERRY_FALL;
  let t = T_BERRY;
  return BERRY_BOUNCE.map((e) => (t += (2 * v * e) / BERRY_G));
}

/** The eye opens as it pops in: its eyelid snaps up to the skeptic's line. */
const openAt = (lt: number) => 1 - outQuad(progress(T_EYE - LEAD, T_EYE + 0.045, lt));
/** One 160 ms blink: both eyes shut to one level line, hold, open. */
const blinkAt = keys([
  [T_BLINK, 0],
  [T_BLINK + 0.035, 1, inQuad],
  [T_BLINK + 0.115, 1],
  [T_BLINK + 0.16, 0, outQuad],
]);
/** He glances down at his name as it lands, and back up for the glint. */
const glance = (lt: number): number =>
  swiftOut(progress(T_WORD - 0.12, T_WORD + 0.1, lt)) * (1 - inOutSine(progress(T_GLINT - 0.3, T_GLINT - 0.06, lt)));

/** The mascot's glint: the band sweeps the lens over four 100 ms steps. */
const SWEEP_MS = 0.4;
function sweepAt(lt: number): number | null {
  for (const at of [T_SHINE, T_GLINT]) {
    const d = lt - at + 0.05;
    if (d >= 0 && d < SWEEP_MS) return d / SWEEP_MS;
  }
  return null;
}

function faceAt(lt: number): FaceParams | null {
  if (lt < T_EYE - LEAD) return null;
  const [lx, ly] = [2.5 * glance(lt), 3.5 * glance(lt)];
  return {
    ...LOGO_FACE,
    eyes: pop(lt, T_EYE, 5, 0.45, 25),
    blink: Math.max(openAt(lt), blinkAt(lt)),
    look: [lx, ly],
    mustache: lt < T_MUST - LEAD ? 0 : pop(lt, T_MUST, 4.5, 0.45, 25),
    twitch: 0.3 * wobble(lt, T_MUST + 0.04, 6.5, 6.5) + 0.12 * wobble(lt, T_GLINT, 7, 9),
    monocle: lt < T_MONO - LEAD ? 0 : pop(lt, T_MONO, 6, 0.55, 20),
    chain: smoothstep(T_MONO - LEAD, T_MONO + 0.09, lt),
    swing: -0.12 * wobble(lt, T_MONO + 0.02, 3.4, 8),
    arc: outCubic(progress(T_MONO, T_SHINE, lt)),
    sweep: sweepAt(lt),
    glint: 0.9 * pulse(lt, T_SHINE, 0.02, 0.09) + 0.9 * pulse(lt, T_GLINT, 0.03, 0.12),
    cheeks: lt < T_BLUSH - LEAD ? 0 : lt < T_BLUSH + 0.03 ? outCubic(progress(T_BLUSH - LEAD, T_BLUSH + 0.03, lt)) : 1 + 2 * smoothstep(T_BLUSH + 0.03, T_BLUSH + 0.2, lt),
    strawberry: lt < T_BERRY - BERRY_FALL ? 0 : 1,
    berryLift: berryLift(lt),
  };
}

function poseAt(lt: number): BoxPose {
  const t = Math.min(lt, STILL);
  const size = sizeAt(t);
  const squash = squashAt(t);
  return {
    ...END_POSE,
    // Inflate about the middle, so the pop reads as volume, not a hop.
    pos: [0, MID - 0.425 * size * squash, 0],
    size,
    squash,
    tape: tapeAt(t),
    face: faceAt(t),
    toneShift: 0.3 * pulse(t, 0.02, 0.02, 0.08),
  };
}

// The inflate: on the first frames the flat amber silhouette gives way to
// the box's own flat colours behind a warm front sweeping down from the top
// left, with a hot band riding the front.
const REVEAL = [0.004, 0.11] as const;
const SWEEP_DIR: readonly [number, number] = [0.45, 0.893];

function drawReveal(ctx: CanvasRenderingContext2D, sil: readonly Projected[], lt: number): void {
  const q = progress(REVEAL[0], REVEAL[1], lt);
  if (q >= 1) return;
  const [dx, dy] = SWEEP_DIR;
  let m0 = Infinity;
  let m1 = -Infinity;
  for (const p of sil) {
    const d = p.x * dx + p.y * dy;
    m0 = Math.min(m0, d);
    m1 = Math.max(m1, d);
  }
  const band = 40;
  const s = lerp(m0 - band, m1 + band, inOutSine(q));
  ctx.save();
  polygon(ctx, sil);
  ctx.clip();
  const g = ctx.createLinearGradient(dx * (s - band), dy * (s - band), dx * (s + band), dy * (s + band));
  g.addColorStop(0, rgba(PALETTE.amber, 0));
  g.addColorStop(1, rgba(PALETTE.amber, 1));
  ctx.fillStyle = g;
  ctx.fillRect(0, 0, 1920, 1080);
  if (q > 0) {
    const h = ctx.createLinearGradient(dx * (s - 60), dy * (s - 60), dx * (s + 60), dy * (s + 60));
    const heat = 0.55 * Math.sin(Math.PI * q);
    h.addColorStop(0, rgba(PALETTE.amberBright, 0));
    h.addColorStop(0.55, rgba(PALETTE.paper, heat));
    h.addColorStop(1, rgba(PALETTE.amberBright, 0));
    ctx.globalCompositeOperation = "lighter";
    ctx.fillStyle = h;
    ctx.fillRect(0, 0, 1920, 1080);
  }
  ctx.restore();
}

/**
 * A shockwave along the floor from the box's base: a flat ellipse, its back
 * half drawn behind the box and its front half over it.
 */
function floorRing(
  ctx: CanvasRenderingContext2D,
  base: { x: number; y: number; w: number },
  p: number,
  reach: number,
  width: number,
  alpha: number,
  half: "back" | "front",
): void {
  if (p <= 0 || p >= 1) return;
  const e = 1 - (1 - p) ** 3;
  const r = lerp(0.55, reach, e) * base.w;
  ctx.save();
  ctx.beginPath();
  if (half === "back") ctx.ellipse(base.x, base.y, r, r * 0.11, 0, Math.PI, TAU);
  else ctx.ellipse(base.x, base.y, r, r * 0.11, 0, 0, Math.PI);
  ctx.strokeStyle = rgba(PALETTE.amberBright, alpha * (1 - p) ** 1.3);
  ctx.lineWidth = lerp(width, 1.6, p);
  ctx.lineCap = "round";
  ctx.stroke();
  ctx.restore();
}

function drawRings(ctx: CanvasRenderingContext2D, base: { x: number; y: number; w: number }, lt: number, half: "back" | "front"): void {
  floorRing(ctx, base, progress(0.008, 0.3, lt), 2.3, 8, 0.85, half);
  // A faint, quicker echo inside it.
  floorRing(ctx, base, progress(0.04, 0.24, lt), 1.6, 3, 0.4, half);
}

// Sparks thrown off by the inflate: from the silhouette's upper edges on
// parabolic arcs, gone before the face starts to assemble.
interface Spark {
  t0: number;
  life: number;
  x: number;
  y: number;
  vx: number;
  vy: number;
  w: number;
}
const GRAVITY = 3400;
let sparks: Spark[] | null = null;
function getSparks(): Spark[] {
  if (sparks) return sparks;
  sparks = [];
  const sil = boxSilhouette(new View(END_CAM), END_POSE);
  let cx = 0;
  let cy = 0;
  for (const p of sil) {
    cx += p.x / sil.length;
    cy += p.y / sil.length;
  }
  const edges: [Projected, Projected][] = [];
  for (let i = 0; i < sil.length; i++) {
    const p = sil[i];
    const q = sil[(i + 1) % sil.length];
    if ((p.y + q.y) / 2 < cy + 60) edges.push([p, q]);
  }
  for (let i = 0; i < 18; i++) {
    const h = (k: number) => hash(i, 900 + k);
    const [p, q] = edges[Math.floor(h(1) * edges.length)];
    const u = 0.1 + 0.8 * h(2);
    const x = lerp(p.x, q.x, u);
    const y = lerp(p.y, q.y, u);
    const out = Math.atan2(y - cy, x - cx);
    const a = lerp(out, -Math.PI / 2, 0.35) + (h(3) - 0.5) * 0.9;
    const speed = 380 + h(4) ** 1.4 * 700;
    sparks.push({
      t0: 0.006 + h(5) * 0.025,
      life: 0.16 + h(6) * 0.14,
      x,
      y,
      vx: Math.cos(a) * speed,
      vy: Math.sin(a) * speed,
      w: 1.4 + h(7) * 2.2,
    });
  }
  return sparks;
}

function drawSparks(ctx: CanvasRenderingContext2D, lt: number): void {
  if (lt > 0.35) return;
  ctx.save();
  ctx.lineCap = "round";
  for (const s of getSparks()) {
    const d = lt - s.t0;
    if (d <= 0 || d >= s.life) continue;
    const age = d / s.life;
    const a = 1 - age * age;
    const at = (k: number): [number, number] => {
      const dd = Math.max(0, d - k * 0.014);
      return [s.x + s.vx * dd, s.y + s.vy * dd + 0.5 * GRAVITY * dd * dd];
    };
    ctx.beginPath();
    for (let k = 3; k >= 0; k--) {
      const [x, y] = at(k);
      if (k === 3) ctx.moveTo(x, y);
      else ctx.lineTo(x, y);
    }
    ctx.strokeStyle = mix(PALETTE.paper, PALETTE.amberBright, smoothstep(0, 0.5, age), a);
    ctx.lineWidth = s.w * lerp(1, 0.45, age);
    ctx.stroke();
    const [x, y] = at(0);
    glow(ctx, x, y, 7 + s.w * 3, PALETTE.amber, 0.45 * a);
  }
  ctx.restore();
}

// A big soft backlight. glow()'s 128 px sprite bands when stretched this far,
// so this one has its own larger sprite with a smooth falloff.
let backSprite: HTMLCanvasElement | null = null;
function backlight(ctx: CanvasRenderingContext2D, x: number, y: number, r: number, alpha: number): void {
  if (alpha <= 0) return;
  if (!backSprite) {
    const n = 512;
    backSprite = makeCanvas(n, n);
    const g = backSprite.getContext("2d")!;
    const grad = g.createRadialGradient(n / 2, n / 2, 0, n / 2, n / 2, n / 2);
    for (let i = 0; i <= 16; i++) {
      const k = i / 16;
      grad.addColorStop(k, rgba(PALETTE.amberDeep, Math.exp(-4.5 * k * k) * (1 - k)));
    }
    g.fillStyle = grad;
    g.fillRect(0, 0, n, n);
  }
  ctx.save();
  ctx.globalCompositeOperation = "lighter";
  ctx.globalAlpha *= clamp(alpha);
  ctx.drawImage(backSprite, x - r, y - r, r * 2, r * 2);
  ctx.restore();
}

/**
 * Zoom smear for the snap back: the silhouette at sub-frame steps back along
 * the move, faint and behind the box, so the fastest frames read as one
 * continuous pull instead of a jump.
 */
function drawSnapSmear(ctx: CanvasRenderingContext2D, lt: number): void {
  if (lt < T_SNAP || lt > T_SNAP + 0.16) return;
  const size = (t: number) => squareAt(t).size;
  const speed = (size(lt - 1 / 60) - size(lt)) / size(lt);
  const a = 0.32 * clamp(speed * 4);
  if (a < 0.01) return;
  ctx.save();
  ctx.fillStyle = PALETTE.amberDeep;
  const base = ctx.globalAlpha;
  const n = 20;
  for (let j = 1; j <= n; j++) {
    const tj = lt - (j / n) * (1.4 / 60);
    if (tj < T_SNAP) break;
    polygon(ctx, boxSilhouette(new View(cameraAt(tj)), poseAt(tj)));
    ctx.globalAlpha = base * a * 0.36 * (1 - j / (n + 1));
    ctx.fill();
  }
  ctx.restore();
}

/** The strawberry's landing: a sparkle and a puff where it meets the lid. */
function drawBerryLanding(ctx: CanvasRenderingContext2D, view: View, pose: BoxPose, lt: number): void {
  const d = lt - T_BERRY;
  if (d < 0 || d > 0.3) return;
  // The seat, on the lid's top: lid local (-0.42, 0), which is logo (39, 14.5).
  const p = faceToScreen(view, pose, 39, 14.5);
  const k = Math.exp(-d / 0.07);
  glow(ctx, p.x, p.y, 90 * (0.5 + d * 3), PALETTE.amberBright, 0.5 * k);
  sparkle(ctx, p.x + 38, p.y - 60, 26 * k, k, d * 4);
}

// The card: the wordmark, the line, the install command, and the address,
// centred under the box, arriving as the camera snaps back. Nothing on it
// moves once the address has landed but the caret, which goes out with the
// glint.
const CX = 960;
export const CARD = { word: 614, line: 732, install: 834, url: 936 } as const;
export const WORD = "mr boxington";
/** The wordmark, 128 px, as on the name card. */
const WORD_STYLE = wordStyle(128);
/** When the wordmark's first word starts to rise. */
export const WORD_IN = entrance(WORD, T_WORD);
export const LINE = "A shared cache for Cargo builds";
export const INSTALL = "cargo install mbx --locked && mbx setup";
const INSTALL_FONT = font(56, 500, MONO);
export const URL_TEXT = "mr-boxington.jdx.dev";
const URL_STYLE = wordStyle(72, PALETTE.amberBright);

// The command types on, a key per few milliseconds, each landing warm and
// cooling to amber, with a caret that blinks on the beat until the glint.
export const INSTALL_KEYS: readonly number[] = (() => {
  const out: number[] = [];
  const n = INSTALL.length;
  let t = 0;
  for (let i = 0; i < n; i++) {
    out.push(t);
    t += 0.009 + hash(i, 61) * 0.006;
  }
  // The last key lands on T_INSTALL.
  return out.map((k) => T_INSTALL - out[n - 1] + k);
})();
const CARET_OFF = T_GLINT;

function drawInstall(ctx: CanvasRenderingContext2D, lt: number): void {
  if (lt < INSTALL_KEYS[0]) return;
  const line = layout(ctx, INSTALL, INSTALL_FONT);
  const x0 = CX - line.width / 2;
  let typed = 0;
  ctx.save();
  ctx.font = INSTALL_FONT;
  ctx.textAlign = "left";
  ctx.textBaseline = "alphabetic";
  line.glyphs.forEach((g, i) => {
    const d = lt - INSTALL_KEYS[i];
    if (d < 0) return;
    typed = i + 1;
    if (g.ch === " ") return;
    const s = 1 + 0.25 * Math.exp(-d * 40);
    ctx.save();
    ctx.translate(x0 + g.x + g.w / 2, CARD.install);
    ctx.scale(s, s);
    ctx.fillStyle = mix(PALETTE.paper, PALETTE.amber, smoothstep(0, 0.12, d));
    ctx.fillText(g.ch, -g.w / 2, 0);
    ctx.restore();
  });
  // Solid while typing, then on for the first half of each beat.
  const done = typed === INSTALL.length;
  if (lt < CARET_OFF && (!done || (lt / BEAT) % 1 < 0.5)) {
    const last = line.glyphs[typed - 1];
    const x = x0 + last.x + last.w + 6;
    roundedRect(ctx, x, CARD.install - 40, 26, 50, 3);
    ctx.fillStyle = PALETTE.amber;
    ctx.fill();
  }
  ctx.restore();
}

/** The card's lines, set as the reel's captions and the name card are (type.ts drawWords). */
function drawCard(ctx: CanvasRenderingContext2D, lt: number): void {
  drawWords(ctx, WORD, CX, CARD.word, WORD_STYLE, lt, T_WORD, Infinity, "center");
  drawWords(ctx, LINE, CX, CARD.line, CAPTION, lt, T_LINE, Infinity, "center");
  drawInstall(ctx, lt);
  drawWords(ctx, URL_TEXT, CX, CARD.url, URL_STYLE, lt, T_URL, Infinity, "center");
}

export const scene: Scene = {
  id: S.id,
  start: S.start,
  end: S.end,
  draw(ctx, lt, env) {
    ctx.fillStyle = PALETTE.night;
    ctx.fillRect(0, 0, env.W, env.H);

    // Handoff morph → end: exactly the flat silhouette on the first frame.
    if (lt <= 0) {
      polygon(ctx, boxSilhouette(new View(END_CAM), END_POSE));
      ctx.fillStyle = PALETTE.amber;
      ctx.fill();
      return;
    }
    const t = Math.min(lt, STILL);

    // Night lifts to the site's stage colour as the card settles.
    ctx.fillStyle = mix(PALETTE.night, PALETTE.bg, smoothstep(0.3, 1.6, t));
    ctx.fillRect(0, 0, env.W, env.H);

    const cam = cameraAt(t);
    const view = new View(cam);
    const pose = poseAt(t);
    const sil = boxSilhouette(view, pose);
    const l = view.project([pose.pos[0] - 0.5 * (pose.size ?? 1), pose.pos[1], pose.pos[2] + 0.5 * (pose.size ?? 1)]);
    const r = view.project([pose.pos[0] + 0.5 * (pose.size ?? 1), pose.pos[1], pose.pos[2] + 0.5 * (pose.size ?? 1)]);
    const base = { x: (l.x + r.x) / 2, y: l.y, w: r.x - l.x };
    const mid = view.project([0, MID, 0.5]);

    ctx.save();
    const [sx, sy] = shake(t, 0.001, 7, 0.05);
    ctx.translate(sx, sy);

    // Warm backlight that lingers, and a tight hot core on the hit.
    backlight(ctx, mid.x, mid.y + 20, 1.5 * base.w, 0.22 * progress(0, 0.35, t));
    const hit = pulse(t, 1 / 60, 1 / 60, 0.03);
    glow(ctx, mid.x, mid.y, 360, PALETTE.amberBright, 0.75 * hit);
    glow(ctx, mid.x, mid.y, 170, PALETTE.paper, 0.45 * hit);

    drawSnapSmear(ctx, t);
    drawRings(ctx, base, t, "back");
    drawSparks(ctx, t);
    drawLogoBox(ctx, cam, pose, 0.9 * progress(0.02, 0.2, t));
    drawReveal(ctx, sil, t);
    // Exposure pop on the box alone, so the stage stays night.
    const exposure = 0.2 * pulse(t, 1 / 60, 1 / 60, 0.035);
    if (exposure > 0.01) {
      ctx.save();
      polygon(ctx, sil);
      ctx.globalCompositeOperation = "lighter";
      ctx.fillStyle = rgba(PALETTE.amberBright, exposure);
      ctx.fill();
      ctx.restore();
    }
    drawRings(ctx, base, t, "front");
    drawBerryLanding(ctx, view, pose, t);
    ctx.restore();

    drawCard(ctx, t);
    flash(ctx, env.W, env.H, 0.04 * pulse(t, 1 / 60, 1 / 60, 0.03), PALETTE.amberBright);
  },
};

/** For tests: the box's pose and camera at local time `lt`. */
export const cardAt = (lt: number): { pose: BoxPose; cam: Camera } => ({ pose: poseAt(lt), cam: cameraAt(lt) });
