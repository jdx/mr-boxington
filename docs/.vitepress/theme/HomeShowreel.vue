<script setup lang="ts">
import { withBase } from "vitepress";
import { computed, onMounted, onUnmounted, ref, shallowRef } from "vue";
import { data } from "../benchmarks.data";
import type { ScoreHandle } from "./showreel/audio";
import type { ReelFacts } from "./showreel/bible";
import { factsFromBenchmarks } from "./showreel/facts";
import type { Reel } from "./showreel/reel";

// The reel and its score load on mount as their own chunks, so the rest of
// the docs never download them. Only the facts helper is needed up front, for
// the server-rendered chapter descriptions.
type ReelModule = typeof import("./showreel/reel");
type AudioModule = typeof import("./showreel/audio");

/** Backing-store scales tried in turn when frames take too long to draw. */
const SCALES = [2, 1.5, 1, 0.75];
/** Average draw time, over a window of frames, that triggers a step down. */
const BUDGET_MS = 12;
const WINDOW = 30;
/** Median frame interval above which the reel counts as dropping frames. */
const SLOW_FRAME_MS = 20;
/** Seconds between scheduling the score and its first sample. */
const LEAD = 0.05;
/** Quiet time after the last seek before the score restarts under playback. */
const RESTART_MS = 120;
/** Below this CSS width the reel's HUD text is too small to read, so it is dropped. */
const HUD_MIN_WIDTH = 720;
/** How long the reel's bottom HUD row takes to fade, matching the controls. */
const HUD_FADE_MS = 250;

const facts = factsFromBenchmarks(data);
const secs = (n: number) => `${n.toFixed(1)} seconds`;

/** The chart's delta, from the rounded readouts as the Data scene draws it. */
function savedText(c: NonNullable<ReelFacts["commit"]>): string {
  const delta = (Math.round(c.cargo * 10) - Math.round(c.mbx * 10)) / 10;
  return delta > 0 ? `, ${secs(delta)} less` : "";
}

function describeChapters(f: ReelFacts | null) {
  const bench = f?.subject ? `the ${f.subject} benchmark's` : "the benchmark's";
  const warm = f?.warm;
  const commit = f?.commit;
  return [
    {
      label: "Line & fold",
      text: "A point of light draws a flat cardboard net, which folds up into a closed, taped box.",
    },
    {
      label: "Character",
      text: "The box crouches, leaps, lands, and comes to life as Mr Boxington: eyes, brows, mustache, shipping label, monocle, and bow tie.",
    },
    {
      label: "Kinetic type",
      text: "The camera dives into his monocle, where the words reuse matching compilation work across projects, worktrees, and CI slam into place.",
    },
    {
      label: "Particles",
      text: `Compiled crates stream from a project into the box, which sends restored outputs to a worktree and to CI${
        warm ? `, where counters reach ${warm.hits} of ${warm.lookups} cache hits` : ""
      }.`,
    },
    {
      label: "Data",
      text: commit
        ? `A bar chart of ${bench} next-commit build, with the store warmed at the parent commit: Cargo takes ${secs(commit.cargo)} and mbx takes ${secs(commit.mbx)}${savedText(commit)}.`
        : `A bar chart compares Cargo with mbx on ${bench} next-commit build, with the store warmed at the parent commit.`,
    },
    {
      label: "Isometric",
      text: "Cached outputs drop into an isometric store as boxes. A scan sweeps the grid and prunes all but a few.",
    },
    {
      label: "Liquid morph",
      text: "The boxes that were kept melt together into one liquid shape, which takes on Mr Boxington's outline.",
    },
    {
      label: "Logo resolve",
      text: "Mr Boxington returns in full above the name mr boxington, the line A shared cache for Cargo builds, and the address mr-boxington.jdx.dev.",
    },
  ];
}

const described = describeChapters(facts);
const summary =
  "Showreel: Mr Boxington, a cardboard box with a monocle and bow tie, folds into shape and shows matching Cargo build outputs being reused across projects, worktrees, and CI. Chapters are listed below.";

const playerEl = ref<HTMLDivElement | null>(null);
const canvasEl = ref<HTMLCanvasElement | null>(null);
const controlsEl = ref<HTMLDivElement | null>(null);
const reel = shallowRef<Reel | null>(null);
const drawn = ref(false);
const playing = ref(false);
const ended = ref(false);
/** False while the poster stands in for a reel that has not been played. */
const started = ref(false);
const soundOn = ref(false);
const awake = ref(false);
const scrubbing = ref(false);
/** Mirrors the playhead for the template; `now` is the precise value. */
const time = ref(0);
const hoverTime = ref<number | null>(null);

const duration = computed(() => reel.value?.duration ?? 15);
const chapters = computed(() => reel.value?.chapters ?? []);

function chapterAt(t: number): number {
  const cs = chapters.value;
  const i = cs.findIndex((c) => t < c.end);
  return i < 0 ? Math.max(0, cs.length - 1) : i;
}
const chapterLabel = (i: number) => chapters.value[i]?.label ?? described[i]?.label ?? "";
const trim = (n: number) => String(Number(n.toFixed(1)));

const valueText = computed(() => {
  // Whole seconds while playing, so a focused slider is not read out ten
  // times a second.
  const t = playing.value ? Math.floor(time.value) : time.value;
  return `${trim(t)} of ${trim(duration.value)} seconds, ${chapterLabel(chapterAt(time.value))}`;
});

const hoverLabel = computed(() => {
  const t = hoverTime.value;
  if (t === null) return "";
  const i = chapterAt(t);
  return `${String(i + 1).padStart(2, "0")} ${chapterLabel(i)} · ${t.toFixed(1)}`;
});

// Chapter boundaries as gaps cut into the rail.
const railMask = computed(() => {
  const d = duration.value;
  const cuts = chapters.value.slice(1).map((c) => {
    const p = (c.start / d) * 100;
    return `#000 0 calc(${p}% - 1.5px), transparent 0 calc(${p}% + 1.5px)`;
  });
  if (!cuts.length) return undefined;
  const g = `linear-gradient(to right, ${cuts.join(", ")}, #000 0)`;
  return { maskImage: g, WebkitMaskImage: g };
});

const playLabel = computed(() =>
  playing.value ? "Pause" : ended.value ? "Replay showreel" : "Play showreel",
);
const pillText = computed(() =>
  playing.value ? "" : ended.value ? "Replay" : !started.value ? "Play" : "",
);

// Engine state that never reaches the template.
let mod: ReelModule | null = null;
let audioMod: Promise<AudioModule> | null = null;
let ctx: CanvasRenderingContext2D | null = null;
let now = 0;
let raf = 0;
let clockAnchor: { t: number; ts: number } | null = null;
let cssW = 0;
let cssH = 0;
let scaleCap = Infinity;
let costs: number[] = [];
let gaps: number[] = [];
let lastFrame = 0;
let warmup = 0;
let inView = false;
let autoplayed = false;
let autoPaused = false;
let userStarted = false;
let resumeAfterScrub = false;
let wakeTimer = 0;
let reducedMotion = false;
let renderFailed = false;
let disposed = false;
/** The fade of the reel's chapter label and timecode toward hudTarget(). */
const hudFade = { from: 1, to: 1, start: 0, dur: 0 };
/** The row opacity the last frame was drawn with. */
let hudDrawn = 1;
let fadeRaf = 0;
/** Controls sit in a strip under the frame instead of over it. */
let stripLayout = false;
let ac: AudioContext | null = null;
let master: GainNode | null = null;
let score: { handle: ScoreHandle; from: number; when: number } | null = null;
let scoreToken = 0;
let suspendTimer = 0;
/** An idle suspend still settling; a start waits for it before resuming. */
let suspending: Promise<void> | null = null;
/** The context stopped a playing score on its own (a call, a device change). */
let interrupted = false;
let restartTimer = 0;
const cleanups: (() => void)[] = [];

function listen(target: EventTarget, type: string, fn: (e: Event) => void) {
  target.addEventListener(type, fn);
  cleanups.push(() => target.removeEventListener(type, fn));
}

function backingScale(): number {
  return Math.min(window.devicePixelRatio || 1, 2, scaleCap);
}

/** Whether the overlaid controls show, by the same rules as the stylesheet. */
function controlsShown(): boolean {
  if (!playing.value || awake.value || scrubbing.value) return true;
  try {
    const p = playerEl.value;
    return !!(
      controlsEl.value?.matches(":hover") ||
      p?.matches(":focus-visible") ||
      p?.querySelector(":focus-visible")
    );
  } catch {
    // Without :focus-visible the stylesheet drops those rules too.
    return false;
  }
}

/**
 * The reel draws its own chapter label and timecode along the bottom edge,
 * where the overlaid controls go, so that row gives way whenever they show.
 */
function hudTarget(): number {
  return stripLayout || !controlsShown() ? 1 : 0;
}

/** CSS `ease`, cubic-bezier(0.25, 0.1, 0.25, 1), solved for x by Newton's method. */
function cssEase(x: number): number {
  let u = x;
  for (let i = 0; i < 5; i++) u -= (u * (0.75 + u * (u - 0.75)) - x) / (0.75 + u * (3 * u - 1.5));
  u = Math.min(Math.max(u, 0), 1);
  return u * (0.3 + u * (2.4 - 1.7 * u));
}

function hudValue(ts: number): number {
  const f = hudFade;
  const p = f.dur > 0 ? (ts - f.start) / f.dur : 1;
  return p >= 1 ? f.to : f.from + (f.to - f.from) * cssEase(Math.max(p, 0));
}

/**
 * The row's opacity at `ts`. It fades like the controls' opacity transition
 * in reverse, so the two cross over: a change of target starts a new fade from
 * where the row is, shortened as a reversed CSS transition is.
 */
function hudAt(ts: number): number {
  const target = hudTarget();
  if (target !== hudFade.to) {
    const from = hudValue(ts);
    hudFade.from = from;
    hudFade.to = target;
    hudFade.start = ts;
    // The stylesheet drops its transitions for reduced motion.
    hudFade.dur = reducedMotion ? 0 : HUD_FADE_MS * Math.abs(target - from);
  }
  return hudValue(ts);
}

/** Put the row straight at its target, for a change nobody sees. */
function snapHud() {
  const target = hudTarget();
  hudFade.from = target;
  hudFade.to = target;
  hudFade.dur = 0;
}

/** Run the row's fade while paused; playback frames run it otherwise. */
function fadeHud() {
  if (raf || fadeRaf || hudDrawn === hudTarget()) return;
  fadeRaf = requestAnimationFrame((ts) => {
    fadeRaf = 0;
    if (playing.value || hudDrawn === hudTarget() || draw(ts) < 0) return;
    fadeHud();
  });
}

/**
 * Draw the current frame, resizing the backing store first if needed. `ts`
 * is the frame's timestamp, the clock CSS transitions also run on.
 * Returns the time the draw calls took, or -1 when nothing was drawn.
 */
function draw(ts = performance.now()): number {
  const r = reel.value;
  const c = canvasEl.value;
  if (!r || !ctx || !c || cssW <= 0 || cssH <= 0) return -1;
  const s = backingScale();
  const w = Math.max(1, Math.round(cssW * s));
  const h = Math.max(1, Math.round(cssH * s));
  if (c.width !== w || c.height !== h) {
    c.width = w;
    c.height = h;
  }
  const hudBottom = hudAt(ts);
  const t0 = performance.now();
  try {
    r.render(ctx, started.value ? now : mod!.POSTER_TIME, w, h, {
      hud: cssW < HUD_MIN_WIDTH ? 0 : 1,
      hudBottom,
    });
    hudDrawn = hudBottom;
  } catch (err) {
    // A frame that throws leaves its save() calls unbalanced, so a clip or
    // alpha it set would carry into every later frame. Setting the width
    // resets the whole context; ctx.reset() is too new to rely on.
    c.width = w;
    if (!renderFailed) console.error("showreel: frame failed to draw", err);
    renderFailed = true;
    return -1;
  }
  drawn.value = true;
  return performance.now() - t0;
}

/** Step the backing store down when frames run over budget. */
function measure(ms: number, gap: number) {
  if (warmup > 0) {
    warmup--;
    return;
  }
  costs.push(ms);
  gaps.push(gap);
  // About 30 frames, but no more than a second when frames are very slow.
  const span = gaps.reduce((a, b) => a + b, 0);
  if (costs.length < WINDOW && (costs.length < 6 || span < 1000)) return;
  const avg = costs.reduce((a, b) => a + b, 0) / costs.length;
  const sorted = gaps.sort((a, b) => a - b);
  const median = sorted[sorted.length >> 1];
  costs = [];
  gaps = [];
  // With a GPU canvas most of the cost lands after the draw calls return and
  // only shows up as missed frames. A steady 30 Hz cap (low-power mode) keeps
  // its fastest and median intervals equal, so it does not count; anything
  // slower than that does.
  const dropping =
    median > SLOW_FRAME_MS * 2 || (median > SLOW_FRAME_MS && median > sorted[0] * 1.5);
  if (avg <= BUDGET_MS && !dropping) return;
  const current = backingScale();
  const next = SCALES.find((s) => s < current - 0.01);
  if (next === undefined) return;
  // Applied by the next draw, which resizes and paints in one go.
  scaleCap = next;
  warmup = 4;
}

function audioNow(a: AudioContext): number {
  // The context time now leaving the speakers, so the picture matches what
  // is heard rather than what is scheduled.
  const ts = a.getOutputTimestamp?.();
  if (ts?.contextTime && ts.performanceTime) {
    return ts.contextTime + (performance.now() - ts.performanceTime) / 1000;
  }
  return a.currentTime - (a.outputLatency || a.baseLatency || 0);
}

function clockTime(ts: number): number {
  if (score && ac && ac.state === "running") {
    clockAnchor = null;
    // Never step backwards: hold the frame until the sound catches up.
    return Math.max(now, score.from + (audioNow(ac) - score.when));
  }
  if (!clockAnchor) clockAnchor = { t: now, ts };
  return clockAnchor.t + (ts - clockAnchor.ts) / 1000;
}

function frame(ts: number) {
  raf = 0;
  if (!playing.value || !reel.value) return;
  const t = clockTime(ts);
  if (t >= duration.value) {
    now = duration.value;
    finish();
    return;
  }
  now = t;
  time.value = t;
  const cost = draw(ts);
  if (cost >= 0 && lastFrame) measure(cost, ts - lastFrame);
  lastFrame = ts;
  raf = requestAnimationFrame(frame);
}

function play(byUser: boolean) {
  if (!reel.value) return;
  if (ended.value || now >= duration.value - 1e-3) now = 0;
  if (byUser) userStarted = true;
  ended.value = false;
  started.value = true;
  autoPaused = false;
  playing.value = true;
  time.value = now;
  clockAnchor = null;
  costs = [];
  gaps = [];
  lastFrame = 0;
  warmup = 4;
  if (soundOn.value) void startScore(false);
  if (!raf) raf = requestAnimationFrame(frame);
}

function pause() {
  playing.value = false;
  if (raf) cancelAnimationFrame(raf);
  raf = 0;
  stopScore();
  time.value = now;
  // The controls come up on pause; fade the HUD row out from under them.
  fadeHud();
}

/** Hold the last frame; the reel plays once. */
function finish() {
  pause();
  // The HUD has already faded out for the end card.
  if (fadeRaf) cancelAnimationFrame(fadeRaf);
  fadeRaf = 0;
  snapHud();
  ended.value = true;
  draw();
}

function toggle() {
  if (!reel.value) return;
  if (playing.value) {
    userStarted = true;
    autoPaused = false;
    pause();
  } else {
    play(true);
  }
}

function seek(t: number) {
  if (!reel.value) return;
  userStarted = true;
  now = Math.min(Math.max(t, 0), duration.value);
  time.value = now;
  started.value = true;
  ended.value = now >= duration.value;
  if (playing.value) {
    if (ended.value) {
      finish();
      return;
    }
    clockAnchor = null;
    if (soundOn.value) {
      // Silence at once, but reschedule the whole score only once seeking
      // settles; a held arrow key would otherwise start one per repeat. The
      // picture runs on the performance clock in between.
      stopScore();
      restartTimer = window.setTimeout(() => {
        if (playing.value && soundOn.value && !score) void startScore(true);
      }, RESTART_MS);
    }
  } else {
    draw();
  }
}

function seekChapter(dir: 1 | -1) {
  const cs = chapters.value;
  if (!cs.length) return;
  const i = chapterAt(now);
  // Back from inside a chapter goes to its start, as on a disc player.
  if (dir < 0) seek(now - cs[i].start > 0.5 || i === 0 ? cs[i].start : cs[i - 1].start);
  else seek(i + 1 < cs.length ? cs[i + 1].start : duration.value);
}

// Sound

function loadAudio(): Promise<AudioModule> {
  audioMod ??= import("./showreel/audio").catch((err) => {
    // Forget a failed load, so a hover prefetch that failed does not decide
    // the next toggle. Whether a retry refetches is up to the browser:
    // Chromium keeps a failed module fetch for the life of the page.
    audioMod = null;
    throw err;
  });
  return audioMod;
}

/** Fetch the score when the sound control is about to be used, not on every visit. */
function warmAudio() {
  void loadAudio().catch(() => {});
}

/** Sound could not start: show the control as off instead of claiming it is on. */
function soundFailed(err: unknown) {
  console.warn("showreel: sound is unavailable", err);
  soundOn.value = false;
  stopScore();
}

function toggleSound() {
  if (soundOn.value) {
    soundOn.value = false;
    stopScore();
    return;
  }
  // Create or resume the context inside the gesture, or browsers keep it muted.
  try {
    if (!ac) {
      const Ctor =
        window.AudioContext ??
        (window as unknown as { webkitAudioContext?: typeof AudioContext }).webkitAudioContext;
      if (!Ctor) return;
      ac = new Ctor({ latencyHint: "interactive" });
      master = ac.createGain();
      master.connect(ac.destination);
      ac.addEventListener("statechange", onAudioState);
    }
    void ac.resume().catch(() => {});
  } catch (err) {
    console.warn("showreel: sound is unavailable", err);
    return;
  }
  soundOn.value = true;
  if (playing.value) void startScore(true);
}

async function startScore(midPlay: boolean) {
  // Stop the old score now, so a seek never shows a frame on its clock.
  stopScore();
  interrupted = false;
  const token = scoreToken;
  const a = ac;
  if (!a || !master) return;
  let audio: AudioModule;
  try {
    audio = await loadAudio();
    // The context reads "running" until a pending suspend lands, which would
    // then silence the new score. Let it land, then resume.
    if (suspending) await suspending;
    if (a.state !== "running") await a.resume();
  } catch (err) {
    // A newer start or a stop has taken over; leave the control to it.
    if (token === scoreToken) soundFailed(err);
    return;
  }
  if (disposed || token !== scoreToken || !playing.value || !soundOn.value) return;
  const when = a.currentTime + LEAD;
  // Joining a reel already in motion: start the score where the picture will
  // be when the first sample is heard. From a standstill, start exactly where
  // the picture is and let it wait, so the downbeat is not skipped.
  const from = midPlay ? now + LEAD + (a.outputLatency || a.baseLatency || 0) : now;
  if (from >= duration.value) return;
  try {
    score = { handle: audio.playScore(a, master, from, when), from, when };
  } catch (err) {
    soundFailed(err);
  }
}

/** Stop the running score; it fades itself out over a few milliseconds. */
function stopScore() {
  scoreToken++;
  window.clearTimeout(restartTimer);
  score?.handle.stop();
  score = null;
  // Idle the context once the fade is done whenever nothing is playing, so a
  // paused or finished reel with sound on does not keep the audio thread busy.
  // Playing again resumes it (startScore).
  window.clearTimeout(suspendTimer);
  suspendTimer = window.setTimeout(() => {
    const idle = !score && (!soundOn.value || !playing.value);
    if (ac && idle && ac.state === "running") {
      const done: Promise<void> = ac
        .suspend()
        .catch(() => {})
        .finally(() => {
          if (suspending === done) suspending = null;
        });
      suspending = done;
    }
  }, 200);
}

function onAudioState() {
  if (!ac) return;
  // An interrupted context (a call, a device change) loses its place; restart
  // the score from the picture when it comes back. Only a score this listener
  // cut off is restarted here: a resume that startScore() asked for fires this
  // event too, and restarting then would pre-empt that start.
  if (ac.state !== "running") {
    if (score) {
      interrupted = true;
      stopScore();
    }
  } else if (interrupted && playing.value && soundOn.value && !score) {
    interrupted = false;
    void startScore(true);
  }
}

// Visibility

function maybeAutoplay() {
  if (!reel.value || autoplayed || reducedMotion || started.value) return;
  if (!inView || document.hidden) return;
  autoplayed = true;
  play(false);
}

function autoPause() {
  if (!playing.value) return;
  pause();
  autoPaused = true;
}

function maybeResume() {
  if (!autoPaused || !inView || document.hidden || !reel.value) return;
  // An autoplay the viewer never touched does not come back once reduced
  // motion is on.
  if (reducedMotion && !userStarted) return;
  play(false);
}

function onReducedMotion(reduce: boolean) {
  reducedMotion = reduce;
  if (reduce && (playing.value || autoPaused) && !userStarted) {
    // Motion was never asked for: go back to the poster.
    pause();
    now = 0;
    time.value = 0;
    started.value = false;
    autoplayed = false;
    autoPaused = false;
    draw();
  } else if (!reduce) {
    maybeAutoplay();
  }
}

// Input

function onKey(e: KeyboardEvent) {
  if (e.altKey || e.ctrlKey || e.metaKey || !reel.value) return;
  const target = e.target as HTMLElement;
  const onButton = target.tagName === "BUTTON";
  const onRange = target.tagName === "INPUT";
  switch (e.key) {
    case " ":
    case "Enter":
      // Buttons activate themselves on Space and Enter.
      if (onButton || (e.key === "Enter" && onRange)) return;
      // A held key toggles once; its auto-repeats are swallowed so Space
      // still does not scroll the page.
      if (!e.repeat) toggle();
      break;
    case "k":
    case "K":
      if (!e.repeat) toggle();
      break;
    case "m":
    case "M":
      if (!e.repeat) toggleSound();
      break;
    case "ArrowLeft":
      seek(now - 1);
      break;
    case "ArrowRight":
      seek(now + 1);
      break;
    case "ArrowDown":
    case "ArrowUp":
      if (!onRange) return;
      seek(now + (e.key === "ArrowUp" ? 1 : -1));
      break;
    case "PageUp":
      seekChapter(-1);
      break;
    case "PageDown":
      seekChapter(1);
      break;
    case "Home":
      seek(0);
      break;
    case "End":
      seek(duration.value);
      break;
    default:
      return;
  }
  e.preventDefault();
  wake();
}

function onScrubInput(e: Event) {
  seek(Number((e.target as HTMLInputElement).value));
}

function onScrubStart() {
  if (!reel.value) return;
  scrubbing.value = true;
  resumeAfterScrub = playing.value;
  if (playing.value) pause();
}

function onScrubEnd() {
  if (!scrubbing.value) return;
  scrubbing.value = false;
  if (resumeAfterScrub && !ended.value) play(true);
  resumeAfterScrub = false;
}

function onScrubHover(e: PointerEvent) {
  const el = e.currentTarget as HTMLElement;
  const rect = el.getBoundingClientRect();
  const inset = 7;
  const f = Math.min(Math.max((e.clientX - rect.left - inset) / (rect.width - inset * 2), 0), 1);
  hoverTime.value = f * duration.value;
  el.style.setProperty("--hover-x", `${inset + f * (rect.width - inset * 2)}px`);
}

function onStageClick() {
  playerEl.value?.focus({ preventScroll: true });
  toggle();
}

/** Show the controls for a moment after pointer or key activity. */
function wake() {
  awake.value = true;
  window.clearTimeout(wakeTimer);
  wakeTimer = window.setTimeout(() => {
    awake.value = false;
  }, 2500);
}

function rest() {
  window.clearTimeout(wakeTimer);
  awake.value = false;
  hoverTime.value = null;
}

// Lifecycle

async function whenFontsLoad(families: string[]): Promise<void> {
  const fonts = document.fonts;
  if (!fonts?.load) return;
  const loaded = Promise.all(families.map((f) => fonts.load(f))).then(
    () => true,
    () => true,
  );
  const timeout = new Promise<false>((resolve) => window.setTimeout(() => resolve(false), 2500));
  if (await Promise.race([loaded, timeout])) return;
  // Too slow for the first frame: draw with fallbacks, then re-measure.
  void loaded.then(() => {
    if (disposed || !mod) return;
    mod.resetTypeCache();
    if (!playing.value) draw();
  });
}

async function init() {
  let type: typeof import("./showreel/type");
  try {
    [mod, type] = await Promise.all([import("./showreel/reel"), import("./showreel/type")]);
  } catch (err) {
    console.error("showreel: failed to load", err);
    return;
  }
  if (disposed) return;
  await whenFontsLoad([type.font(64, 600, type.DISPLAY), type.font(16, 500, type.MONO)]);
  if (disposed || !canvasEl.value) return;
  mod.resetTypeCache();
  ctx = canvasEl.value.getContext("2d", { alpha: false });
  if (!ctx) return;
  reel.value = mod.createReel(facts);
  snapHud();
  draw();
  maybeAutoplay();
}

onMounted(() => {
  const player = playerEl.value!;
  const canvas = canvasEl.value!;

  const motion = window.matchMedia("(prefers-reduced-motion: reduce)");
  reducedMotion = motion.matches;
  listen(motion, "change", (e) => onReducedMotion((e as MediaQueryListEvent).matches));

  listen(document, "visibilitychange", () => {
    if (document.hidden) {
      autoPause();
      return;
    }
    // Nothing crosses a threshold when a tab comes back, so the observer stays
    // quiet: an autoplay held back while the tab was hidden starts here.
    maybeAutoplay();
    maybeResume();
  });

  const io = new IntersectionObserver(
    (entries) => {
      // A late callback can carry several entries; the last one is current.
      const entry = entries[entries.length - 1];
      const ratio = entry.isIntersecting ? entry.intersectionRatio : 0;
      if (ratio >= 0.5) {
        inView = true;
        maybeAutoplay();
        maybeResume();
      } else if (ratio < 0.2) {
        inView = false;
        autoPause();
      }
    },
    { threshold: [0, 0.2, 0.5] },
  );
  io.observe(player);
  cleanups.push(() => io.disconnect());

  const ro = new ResizeObserver((entries) => {
    const box = entries[entries.length - 1].contentRect;
    cssW = box.width;
    cssH = box.height;
    // Resizing clears the canvas; repaint before the browser does.
    draw();
  });
  ro.observe(canvas);
  cleanups.push(() => ro.disconnect());

  // A move to a screen with another pixel ratio does not resize anything.
  let dpr: MediaQueryList | null = null;
  const onDpr = () => {
    watchDpr();
    if (!playing.value) draw();
  };
  const watchDpr = () => {
    dpr?.removeEventListener("change", onDpr);
    dpr = window.matchMedia(`(resolution: ${window.devicePixelRatio}dppx)`);
    dpr.addEventListener("change", onDpr);
  };
  watchDpr();
  cleanups.push(() => dpr?.removeEventListener("change", onDpr));

  listen(window, "pointerup", onScrubEnd);
  listen(window, "pointercancel", onScrubEnd);

  // The same query as the stylesheet's strip layout.
  const strip = window.matchMedia("(hover: none), (pointer: coarse), (max-width: 639px)");
  stripLayout = strip.matches;
  listen(strip, "change", (e) => {
    stripLayout = (e as MediaQueryListEvent).matches;
    snapHud();
    if (!playing.value) draw();
  });

  void init();
});

onUnmounted(() => {
  disposed = true;
  if (raf) cancelAnimationFrame(raf);
  if (fadeRaf) cancelAnimationFrame(fadeRaf);
  raf = 0;
  fadeRaf = 0;
  window.clearTimeout(wakeTimer);
  window.clearTimeout(suspendTimer);
  window.clearTimeout(restartTimer);
  for (const fn of cleanups.splice(0)) fn();
  score?.handle.stop();
  score = null;
  if (ac) {
    ac.removeEventListener("statechange", onAudioState);
    void ac.close().catch(() => {});
  }
  ac = null;
  master = null;
  ctx = null;
});
</script>

<template>
  <section class="MbxShowreel home-section" aria-label="Showreel">
    <figure>
      <div
        ref="playerEl"
        class="player"
        :class="{
          'is-drawn': drawn,
          'is-ready': reel,
          'is-playing': playing,
          'is-paused': !playing,
          'is-awake': awake || scrubbing,
        }"
        role="group"
        aria-label="Showreel player"
        aria-describedby="mbx-showreel-keys"
        tabindex="-1"
        @keydown="onKey"
        @pointermove="wake"
        @pointerdown="wake"
        @pointerleave="rest"
      >
        <div class="stage">
          <img class="fallback" :src="withBase('/logo.svg')" alt="" width="200" height="200" />
          <canvas ref="canvasEl" role="img" :aria-label="summary" @click="onStageClick" />
          <div class="scrim" aria-hidden="true" />
        </div>
        <ol class="sr-only" aria-label="Showreel chapters">
          <li v-for="(c, i) in described" :key="i">{{ chapterLabel(i) }}: {{ c.text }}</li>
        </ol>
        <p id="mbx-showreel-keys" class="sr-only">
          Space or K plays and pauses, the arrow keys move one second, Page Up and Page Down move
          by chapter, Home and End jump to the start and the end, and M turns sound on or off.
        </p>
        <div ref="controlsEl" class="controls">
          <button
            type="button"
            class="play"
            :class="{ pill: pillText }"
            :aria-label="playLabel"
            aria-keyshortcuts="K Space"
            :title="`${playLabel} (K)`"
            @click="toggle"
          >
            <svg v-if="playing" viewBox="0 0 24 24" aria-hidden="true">
              <path d="M7 5h3.6v14H7zM13.4 5H17v14h-3.6z" />
            </svg>
            <svg v-else-if="ended" viewBox="0 0 24 24" aria-hidden="true">
              <path
                d="M12 4.5V1.8L7.4 5.9l4.6 4V7.2a5.3 5.3 0 1 1-5.3 5.3H4a8 8 0 1 0 8-8z"
              />
            </svg>
            <svg v-else viewBox="0 0 24 24" aria-hidden="true">
              <path d="M8 5.2v13.6a.6.6 0 0 0 .9.5l10.6-6.8a.6.6 0 0 0 0-1L8.9 4.7a.6.6 0 0 0-.9.5z" />
            </svg>
            <span v-if="pillText">{{ pillText }}</span>
          </button>
          <div
            class="scrubber"
            :class="{ hovering: hoverTime !== null }"
            @pointermove="onScrubHover"
            @pointerleave="hoverTime = null"
          >
            <div class="rail" :style="railMask" aria-hidden="true">
              <i
                class="preview"
                :style="{ transform: `scaleX(${(hoverTime ?? 0) / duration})` }"
              />
              <i class="fill" :style="{ transform: `scaleX(${time / duration})` }" />
            </div>
            <input
              type="range"
              min="0"
              :max="duration"
              step="any"
              :value="time"
              aria-label="Seek"
              :aria-valuetext="valueText"
              @input="onScrubInput"
              @change="onScrubEnd"
              @pointerdown="onScrubStart"
            />
            <span class="tip" aria-hidden="true">{{ hoverLabel }}</span>
          </div>
          <span class="time" aria-hidden="true"
            ><span class="elapsed">{{ time.toFixed(1) }}</span
            ><span class="total"> / {{ duration.toFixed(1) }}</span></span
          >
          <button
            type="button"
            class="sound"
            aria-label="Sound"
            :aria-pressed="soundOn"
            aria-keyshortcuts="M"
            title="Sound (M)"
            @click="toggleSound"
            @pointerenter="warmAudio"
            @focus="warmAudio"
          >
            <svg viewBox="0 0 24 24" aria-hidden="true">
              <path d="M3.5 9.2v5.6h3.8l5 4.2V5L7.3 9.2z" />
              <path v-if="soundOn" class="stroke" d="M15.6 8.6a4.8 4.8 0 0 1 0 6.8M18.2 6a8.4 8.4 0 0 1 0 12" />
              <path v-else class="stroke" d="M16 9.5l5 5M21 9.5l-5 5" />
            </svg>
          </button>
        </div>
      </div>
      <figcaption>
        Drawn live in your browser. Build times come from the
        <a :href="withBase('/benchmarks')">benchmarks</a>.
      </figcaption>
    </figure>
  </section>
</template>

<style scoped>
.MbxShowreel {
  padding-top: 8px;
}
figure {
  margin: 0;
}
.player {
  background: #0e0c0a;
  border: 1px solid var(--vp-c-divider);
  border-radius: 12px;
  box-shadow:
    0 30px 80px -40px #000,
    0 14px 32px -22px rgb(0 0 0 / 0.7);
  isolation: isolate;
  position: relative;
}
.player:focus-visible {
  outline: 2px solid var(--mbx-teal-light);
  outline-offset: 4px;
}
/* Exactly 16:9 inside the border; rounded to the border's inner edge. */
.stage {
  aspect-ratio: 16 / 9;
  border-radius: 11px;
  /* Its own stacking context, so Safari clips the canvas to the corners. */
  isolation: isolate;
  overflow: hidden;
  position: relative;
}
/* The logo holds the frame until the reel's first draw, or if it fails to load. */
.fallback {
  height: 31.5%;
  left: 50%;
  position: absolute;
  top: 37.6%;
  transform: translate(-50%, -50%);
  width: auto;
}
/* Hidden once the canvas has faded in over it, so the logo never blinks. */
.player.is-drawn .fallback {
  transition: visibility 0s linear 0.35s;
  visibility: hidden;
}
canvas {
  cursor: pointer;
  display: block;
  height: 100%;
  inset: 0;
  opacity: 0;
  position: absolute;
  transition: opacity 0.35s ease;
  width: 100%;
}
.player.is-drawn canvas {
  opacity: 1;
}
.player.is-playing:not(.is-awake) canvas {
  cursor: none;
}

/*
 * Sized to the frame, not the bar, so the controls stay legible over bright
 * frames. The reel hides its own chapter label and timecode while the
 * controls show, so the scrim only has to carry the controls. The ramp
 * follows a smoothstep, so the band has no hard top edge.
 */
.scrim {
  background: linear-gradient(
    to top,
    rgb(14 12 10 / 0.8) 0,
    rgb(14 12 10 / 0.77) 12.5%,
    rgb(14 12 10 / 0.69) 25%,
    rgb(14 12 10 / 0.57) 37.5%,
    rgb(14 12 10 / 0.43) 50%,
    rgb(14 12 10 / 0.29) 62.5%,
    rgb(14 12 10 / 0.16) 75%,
    rgb(14 12 10 / 0.06) 87.5%,
    rgb(14 12 10 / 0) 100%
  );
  bottom: 0;
  height: 26%;
  left: 0;
  opacity: 0;
  pointer-events: none;
  position: absolute;
  right: 0;
  transition: opacity 0.25s ease;
}
.controls {
  align-items: center;
  bottom: 0;
  display: flex;
  gap: 4px;
  left: 0;
  opacity: 0;
  padding: 0 16px 12px;
  position: absolute;
  right: 0;
  transition: opacity 0.25s ease;
  visibility: hidden;
  z-index: 1;
}
.player.is-ready .controls {
  visibility: visible;
}
.player.is-ready.is-paused .controls,
.player.is-ready.is-paused .scrim,
.player.is-ready.is-awake .controls,
.player.is-ready.is-awake .scrim,
.player.is-ready .controls:hover {
  opacity: 1;
}
/* Own rules: a browser drops a whole selector list that uses a pseudo-class it lacks. */
.player.is-ready:focus-visible .controls,
.player.is-ready:focus-visible .scrim {
  opacity: 1;
}
.player.is-ready:has(.controls:hover) .scrim,
.player.is-ready:has(:focus-visible) .controls,
.player.is-ready:has(:focus-visible) .scrim {
  opacity: 1;
}

button {
  align-items: center;
  border-radius: 8px;
  color: var(--mbx-paper);
  cursor: pointer;
  display: inline-flex;
  flex-shrink: 0;
  height: 44px;
  justify-content: center;
  transition:
    background-color 0.15s ease,
    color 0.15s ease;
  width: 44px;
}
button:hover {
  background: rgb(245 234 214 / 0.1);
  color: var(--mbx-amber-bright);
}
button svg {
  fill: currentColor;
  height: 22px;
  width: 22px;
}
button .stroke {
  fill: none;
  stroke: currentColor;
  stroke-linecap: round;
  stroke-width: 1.8;
}
.play.pill {
  background: var(--mbx-amber);
  color: var(--mbx-ink);
  font-size: 14px;
  font-weight: 700;
  gap: 6px;
  padding: 0 16px 0 12px;
  width: auto;
}
.play.pill:hover {
  background: var(--mbx-amber-bright);
  color: var(--mbx-ink);
}
.play.pill svg {
  height: 18px;
  width: 18px;
}
.sound[aria-pressed="true"] {
  color: var(--mbx-amber);
}

.scrubber {
  flex: 1;
  height: 44px;
  margin: 0 6px;
  min-width: 0;
  position: relative;
}
.rail {
  background: rgb(245 234 214 / 0.36);
  border-radius: 2px;
  height: 4px;
  left: 7px;
  overflow: hidden;
  position: absolute;
  right: 7px;
  top: 50%;
  transform: translateY(-50%);
  transition: height 0.15s ease;
}
.scrubber:hover .rail,
.scrubber:focus-within .rail {
  height: 6px;
}
.rail i {
  inset: 0;
  position: absolute;
  transform-origin: left center;
}
.fill {
  background: var(--mbx-amber);
}
.preview {
  background: rgb(245 234 214 / 0.28);
  opacity: 0;
}
.scrubber.hovering .preview {
  opacity: 1;
}
.scrubber input {
  -webkit-appearance: none;
  appearance: none;
  background: transparent;
  border-radius: 6px;
  cursor: pointer;
  height: 100%;
  inset: 0;
  margin: 0;
  position: absolute;
  width: 100%;
}
.scrubber input:focus-visible {
  outline: 2px solid var(--mbx-teal-light);
  outline-offset: 2px;
}
.scrubber input::-webkit-slider-runnable-track {
  background: transparent;
  height: 44px;
}
.scrubber input::-moz-range-track {
  background: transparent;
}
.scrubber input::-webkit-slider-thumb {
  -webkit-appearance: none;
  appearance: none;
  background: var(--mbx-amber);
  border: 0;
  border-radius: 50%;
  box-shadow: 0 0 0 3px rgb(14 12 10 / 0.45);
  height: 14px;
  margin-top: 15px;
  transform: scale(0.8);
  transition: transform 0.15s ease;
  width: 14px;
}
.scrubber input::-moz-range-thumb {
  background: var(--mbx-amber);
  border: 0;
  border-radius: 50%;
  box-shadow: 0 0 0 3px rgb(14 12 10 / 0.45);
  height: 14px;
  transform: scale(0.8);
  transition: transform 0.15s ease;
  width: 14px;
}
.scrubber:hover input::-webkit-slider-thumb,
.scrubber input:focus-visible::-webkit-slider-thumb {
  transform: scale(1);
}
.scrubber:hover input::-moz-range-thumb,
.scrubber input:focus-visible::-moz-range-thumb {
  transform: scale(1);
}
.tip {
  background: rgb(20 18 15 / 0.94);
  border: 1px solid var(--vp-c-divider);
  border-radius: 6px;
  bottom: 40px;
  color: var(--mbx-paper);
  font-family: var(--vp-font-family-mono);
  font-size: 11px;
  left: var(--hover-x, 0);
  line-height: 1;
  opacity: 0;
  padding: 6px 8px;
  pointer-events: none;
  position: absolute;
  transform: translateX(-50%);
  transition: opacity 0.12s ease;
  white-space: nowrap;
}
.scrubber.hovering .tip {
  opacity: 1;
}
.time {
  color: rgb(245 234 214 / 0.78);
  font-family: var(--vp-font-family-mono);
  font-size: 12px;
  font-variant-numeric: tabular-nums;
  padding: 0 4px;
  white-space: nowrap;
}
.elapsed {
  color: var(--mbx-paper);
  display: inline-block;
  min-width: 4ch;
  text-align: right;
}

figcaption {
  color: var(--vp-c-text-2);
  font-size: 13px;
  line-height: 1.6;
  margin-top: 14px;
}
figcaption a {
  color: var(--vp-c-brand-1);
}
figcaption a:hover {
  text-decoration: underline;
  text-underline-offset: 3px;
}

.sr-only {
  border: 0;
  clip: rect(0 0 0 0);
  clip-path: inset(50%);
  height: 1px;
  margin: -1px;
  overflow: hidden;
  padding: 0;
  position: absolute;
  white-space: nowrap;
  width: 1px;
}

/*
 * Touch screens and narrow windows: the controls sit in a strip under the
 * frame, always shown, so nothing covers the picture or waits on a hover.
 */
@media (hover: none), (pointer: coarse), (max-width: 639px) {
  .stage {
    border-radius: 11px 11px 0 0;
  }
  .scrim {
    display: none;
  }
  .controls {
    border-top: 1px solid var(--vp-c-divider);
    opacity: 1;
    padding: 5px 10px;
    position: relative;
  }
  .controls button:focus-visible,
  .scrubber input:focus-visible {
    outline-offset: 1px;
  }
}
@media (hover: none), (pointer: coarse) {
  .tip {
    display: none;
  }
}
@media (max-width: 639px) {
  .controls {
    gap: 0;
    padding: 5px 6px;
  }
  /* Room for the thumb's halo and the focus ring beside the pill and readout. */
  .scrubber {
    margin: 0 6px;
  }
  .time {
    font-size: 11px;
    padding: 0 2px;
  }
  .play.pill {
    padding: 0 12px 0 10px;
  }
}
/* Phones: the slider already reads out the time, so the total gives way to it. */
@media (max-width: 419px) {
  .total {
    display: none;
  }
}
@media (prefers-reduced-motion: reduce) {
  canvas,
  .scrim,
  .controls,
  .rail,
  .tip,
  button {
    transition: none;
  }
}
</style>
