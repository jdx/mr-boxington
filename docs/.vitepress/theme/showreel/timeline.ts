// The reel's clock: the tempo and the timeline of sections. It imports
// nothing, so the landing page can read the chapters without the drawing
// code. bible.ts re-exports all of it.

/** 128 BPM puts a 4/4 bar in exactly 1.875 seconds. */
export const BPM = 128;
export const BEAT = 60 / BPM;
export const BAR = BEAT * 4;
export const beat = (n: number): number => n * BEAT;
export const bar = (n: number): number => n * BAR;

/**
 * The timeline: every section in order, in whole bars. The reel's length,
 * the chapters, each scene's span, and where the score places its cues all
 * come from here, so lengthening or inserting a section moves everything
 * after it. Labels are what a viewer sees in the player's chapter menu and
 * the page's chapter list.
 */
export const SECTIONS = [
  { id: "fold", label: "Fold", bars: 1 },
  { id: "mr-boxington", label: "Mr Boxington", bars: 2 },
  { id: "what", label: "What mbx does", bars: 3 },
  { id: "every-checkout", label: "Every checkout compiles again", bars: 3 },
  { id: "under-cargo-build", label: "Under cargo build", bars: 2 },
  { id: "first-build", label: "First build fills the store", bars: 3 },
  { id: "same-checkout", label: "Same checkout, empty target/", bars: 2 },
  { id: "another-worktree", label: "Another worktree", bars: 3 },
  { id: "six-builds", label: "Six builds at once", bars: 3 },
  { id: "ci", label: "CI", bars: 3 },
  { id: "next-push", label: "Next push, measured", bars: 3 },
  { id: "pruned", label: "target/ pruned", bars: 3 },
  { id: "morph", label: "Morph", bars: 1 },
  { id: "end", label: "End card", bars: 3 },
] as const satisfies readonly { id: string; label: string; bars: number }[];

export type SectionId = (typeof SECTIONS)[number]["id"];

/** One section on the reel's clock. Times are global seconds. */
export interface Section {
  id: SectionId;
  label: string;
  bars: number;
  start: number;
  /** The frame at `end` belongs to the next section. */
  end: number;
  /** Length in seconds. */
  len: number;
  /** Global time of local time `lt`, seconds into the section. */
  at(lt: number): number;
  /** Global time of beat `n` of the section; beat 0 is its first downbeat. */
  beat(n: number): number;
  /** Global time of bar `n` of the section. */
  bar(n: number): number;
}

const TIMELINE = new Map<SectionId, Section>();
{
  // Counted in whole bars and beats, so every boundary is exact.
  let first = 0;
  for (const { id, label, bars } of SECTIONS) {
    const b0 = first;
    TIMELINE.set(id, {
      id,
      label,
      bars,
      start: bar(b0),
      end: bar(b0 + bars),
      len: bar(bars),
      at: (lt) => bar(b0) + lt,
      beat: (n) => beat(b0 * 4 + n),
      bar: (n) => bar(b0 + n),
    });
    first += bars;
  }
}

/** Where section `id` sits on the reel's clock. */
export function sec(id: SectionId): Section {
  const s = TIMELINE.get(id);
  if (!s) throw new Error(`no section "${id}"`);
  return s;
}

/** The whole reel: every section, end to end. */
export const DURATION = bar(SECTIONS.reduce((n, s) => n + s.bars, 0));

/** One chapter per section, for players and the page's chapter list. */
export const CHAPTERS = SECTIONS.map(({ id }) => {
  const { label, start, end } = sec(id);
  return { id, label, start, end };
});

/** `h:mm:ss.fff`, a WebVTT timestamp. Section bounds are whole milliseconds. */
function vttTime(s: number): string {
  const ms = Math.round(s * 1000);
  const pad = (n: number, w = 2) => String(n).padStart(w, "0");
  return `${pad(Math.floor(ms / 3_600_000))}:${pad(Math.floor(ms / 60_000) % 60)}:${pad(Math.floor(ms / 1000) % 60)}.${pad(ms % 1000, 3)}`;
}

/**
 * The chapters as a WebVTT chapters track, one cue per section, identified
 * by its id. docs/public/showreel-chapters.vtt is this text; a test keeps it
 * current.
 */
export function chaptersVtt(): string {
  const cues = CHAPTERS.map((c) => `${c.id}\n${vttTime(c.start)} --> ${vttTime(c.end)}\n${c.label}\n`);
  return ["WEBVTT\n", ...cues].join("\n");
}
