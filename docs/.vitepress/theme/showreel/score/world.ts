// Isometric: the grid, the city raining in ring by ring, the prune beam, and
// the kept cubes turning into discs.

import { KEEP, type Section } from "../bible";
import { rng } from "../math";
import {
  ARM0,
  beamAt,
  DLAST,
  SLAP,
  T_BEAM0,
  T_BEAM1,
  T_BEAMEND,
  T_DISC,
  T_LAND0,
  T_LAND1,
  T_POP0,
  T_STOMP,
  world as city,
} from "../scenes/s6-world";
import type { Part } from ".";
import { ad, glide, hold, hz, line, type Mix, perc, swell, sweep } from "./mix";
import { A, bassBars, CHORD, chordBars, drumBars, G } from "./grooves";
import { ping, thump, tick, whoosh } from "./sounds";

/**
 * The grid draws out, the center cube stomps on b0.25 (s6 T_STOMP), and the
 * city rains in: each ring of equal distance lands together, from b0.5 at
 * distance 1 to b1.5 at the outermost ring (s6 T_LAND0, T_LAND1, DLAST).
 * Rings climb the D major pentatonic.
 */
function world(m: Mix, s: Section): void {
  const t0 = s.start;
  whoosh(m, ad(t0, t0 + 0.08, 0.04, t0 + 0.4), sweep(t0, 9000, t0 + 0.3, 4500), 1, { send: 0.3 }, "white");
  const stomp = s.at(T_STOMP);
  thump(m, stomp, 0.5, 150, 55, 0.3);
  // The stomp's shock ring pops the first ring into being, hot, three frames
  // later (s6 T_POP0): four sparks of light.
  const pop0 = s.at(T_POP0);
  [93, 98, 102, 105].forEach((n, i) => {
    ping(m, pop0 + i * 0.006, hz(n), 0.09, 0.16, { pan: [-0.3, 0.3, -0.15, 0.15][i], send: 0.3 });
  });
  const land0 = s.at(T_LAND0);
  const land1 = s.at(T_LAND1);
  const dlast = DLAST;
  const rings = new Map<number, number>();
  for (const c of city()) {
    if (c.center) continue;
    const key = Math.round(c.d * 1000) / 1000;
    rings.set(key, (rings.get(key) ?? 0) + 1);
  }
  const scale = [62, 64, 66, 69, 71, 74, 76, 78, 81, 83, 86, 88, 90, 93];
  [...rings]
    .sort((a, b) => a[0] - b[0])
    .forEach(([d, count], i) => {
      const t = land0 + ((land1 - land0) * (d - 1)) / (dlast - 1);
      m.duck(t, 0.08, 0.06);
      const weight = Math.min(1, 0.45 + count / 14);
      const pan = i === 0 ? 0 : (i % 2 ? -1 : 1) * Math.min(0.6, 0.1 + 0.04 * i);
      const v = m.voice(perc(t, 0.16 * weight, 0.0008, 0.4), { pan, send: 0.22 });
      if (!v) return;
      // Marimba body and overtone, a cardboard tap, and a squash thud.
      const f = hz(scale[Math.min(i, scale.length - 1)]);
      v.osc("sine", f);
      v.osc("sine", f * 3.99, perc(t, 0.3, 0.0005, 0.05));
      v.noise("white", perc(t, 0.35, 0.0005, 0.03), v.filter("lowpass", 1400, 0), 1, t + 0.05);
      v.osc("sine", sweep(t, 170, t + 0.04, 85), perc(t, 0.7, 0.001, 0.08));
    });
}

/**
 * The prune beam (s6-world beamPos and beamAt) crosses one diagonal row
 * (x + z) every 1/64 bar from b2, so it tags the kept rows on b2.25, b2.5,
 * and b2.75, then accelerates out of the front corner by b2 + 7/8. Each
 * pruned carton rocks and slaps flat SLAP after the beam touches it (0.07
 * s, shortened by up to 40% on the rows the beam reaches as it speeds up); a
 * row slaps as one, louder the more cartons it holds.
 */
function prune(m: Mix, s: Section): void {
  const t0 = s.at(T_BEAM0);
  const end = s.at(T_BEAMEND);
  // The scanner arms first (s6 ARM0 to T_BEAM0): two pen tips race around
  // the grid's border from the front corner, one each way, speeding up
  // (inQuad) until they meet at the back corner as the beam ignites.
  const arm = s.at(ARM0);
  for (const side of [-1, 1]) {
    const a = m.voice(swell(arm, arm + 0.08, 0.008, t0 - 0.004, 0.07, t0 + 0.006), {
      pan: line(arm, side * 0.55, t0, side * 0.08),
      send: 0.2,
      hold: true,
    });
    if (!a) continue;
    a.osc("triangle", glide(arm, t0, hz(66), hz(90), (u) => u * u), 0.5, undefined, side * 6);
    a.noise("white", 0.5, a.filter("bandpass", glide(arm, t0, 2200, 6500, (u) => u * u), 6));
  }
  // The beam switches on.
  m.duck(t0, 0.15, 0.3);
  tick(m, t0, 3200, 0.3, 0, 0.15);
  const z = m.voice(perc(t0, 0.12, 0.001, 0.06), { send: 0.2 });
  if (z) z.osc("sine", sweep(t0, hz(100), t0 + 0.05, hz(88)));
  // The gantry hums while it sweeps and cuts out as it exits the front corner.
  const v = m.voice(hold(t0, 0.03, 0.11, end - 0.008, 0.13, 0.04), { send: 0.18, hold: true });
  if (v) {
    const am = v.vca(0.6);
    v.lfo("sine", 24, 0.3, am.gain);
    v.noise("white", 0.8, v.filter("bandpass", 3400, 7, am));
    v.osc("sine", sweep(t0, hz(81), end, hz(93)), 0.18, am);
  }
  // It snaps off at the front corner.
  const x = m.voice(perc(end, 0.14, 0.002, 0.09), { send: 0.25, pan: 0.1 });
  if (x) {
    x.noise("white", 1, x.filter("bandpass", sweep(end, 6000, end + 0.08, 2000), 3));
    x.osc("sine", sweep(end, hz(93), end + 0.06, hz(86)), 0.25);
  }
  // A row's cartons share the beam's touch and so slap as one.
  const rows = new Map<number, { n: number; side: number; slap: number }>();
  for (const c of city()) {
    if (c.keep) continue;
    const r = rows.get(c.rank) ?? { n: 0, side: 0, slap: s.at(c.hit + SLAP * c.fs) };
    r.n++;
    r.side += c.x - c.z;
    rows.set(c.rank, r);
  }
  const r = rng(606);
  for (const [, { n, side, slap }] of [...rows].sort((a, b) => a[0] - b[0])) {
    const f = 200 + r() * 160;
    const w = Math.min(1, 0.4 + n / 8);
    const c = m.voice(perc(slap, 0.13 * w, 0.001, 0.16), { pan: Math.max(-0.5, Math.min(0.5, (0.12 * side) / n)), send: 0.1 });
    if (!c) continue;
    c.noise("crackle", 1, c.filter("lowpass", sweep(slap, 3000, slap + 0.14, 500), 0), 1.3);
    c.noise("white", perc(slap, 0.55, 0.0005, 0.02), c.filter("bandpass", 1100, 1.1), 1, slap + 0.04);
    c.osc("sine", sweep(slap, f * 1.8, slap + 0.12, f * 0.6), 0.5);
  }
  // Kept cubes get tagged as the beam reaches their rows.
  const tagged = new Map<number, number>();
  for (const [x, z] of KEEP) tagged.set(x + z, (tagged.get(x + z) ?? 0) + 1);
  const notes: Record<number, number> = { [-4]: 81, 0: 86, 4: 90 };
  for (const [rank, count] of [...tagged].sort((a, b) => a[0] - b[0])) {
    const t = s.at(beamAt(rank));
    ping(m, t, hz(notes[rank] ?? 86), 0.1 + 0.02 * count, 0.45, { send: 0.3 });
  }
}

/**
 * The kept cubes wind up under the last of the beam, hop together on b3 and
 * glow (s6 T_BEAM1), turn a quarter, flatten, and round off into the discs
 * just before the bar line (s6 FLAT0 and T_DISC).
 */
function keptHop(m: Mix, t: number, end: number): void {
  m.duck(t, 0.2, 0.15);
  // The flattened carpet drops through the floor like trapdoors, gone in
  // about five frames (s6 sinkDur).
  const d = m.voice(perc(t, 0.22, 0.002, 0.2), { send: 0.12 });
  if (d) {
    d.noise("pink", 1, d.filter("lowpass", sweep(t, 1400, t + 0.12, 180), 2));
    d.osc("sine", sweep(t, 120, t + 0.1, 48), 0.6);
  }
  whoosh(m, swell(t - 0.1, t - 0.06, 0.01, t - 0.004, 0.08, t + 0.01), sweep(t - 0.1, 600, t, 1800), 1.5, { send: 0.1 });
  const h = m.voice(perc(t, 0.2, 0.003, 0.3), { send: 0.2 });
  if (h) {
    h.osc("sine", [[t, hz(50)], [t + 0.1, hz(62), "exp"], [t + 0.3, hz(57), "exp"]]);
    h.osc("triangle", [[t, hz(62)], [t + 0.1, hz(74), "exp"]], perc(t, 0.2, 0.002, 0.08));
  }
  // The glow: a fast strum across the seven cubes, B minor pentatonic over the Bm9 pad.
  [71, 74, 76, 78, 81, 83, 86].forEach((n, i) => {
    const ti = t + i * 0.012;
    const v = m.voice(ad(ti, ti + 0.004, 0.05, ti + 0.55), { pan: (i % 2 ? -1 : 1) * (0.1 + i * 0.06), send: 0.4 });
    if (!v) return;
    v.osc("sine", hz(n));
    v.osc("sine", hz(n) * 2.001, 0.15);
    v.osc("triangle", hz(n), 0.2);
  });
  const flat = t + 0.23;
  // They flatten and round off into the discs.
  whoosh(m, ad(flat - 0.1, end - 0.03, 0.09, end + 0.06), sweep(flat - 0.1, 3500, end, 500), 1.2, { send: 0.3 });
}

export const part: Part = {
  cues(m, s) {
    world(m, s);
    prune(m, s);
    keptHop(m, s.at(T_BEAM1), s.at(T_DISC));
  },
  drums: (m, s) => drumBars(m, s, [[[0, 8, 11], [4, 12], [7]]]),
  bass: (m, s) => bassBars(m, s, [[35, 0.5], G, A]),
  pads: (m, s) => chordBars(m, s, [[47, 54, 57, 62, 73], CHORD.G, CHORD.A], 1800),
};
