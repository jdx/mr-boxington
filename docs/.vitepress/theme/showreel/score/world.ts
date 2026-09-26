// target/ pruned: the first cube stomps and the city of target/ cartons
// rains in, a checkout's folder crumples, a 30-day clock runs out, a second
// wave piles through the disk budget, and the beam sweeps while each rule
// knocks its group flat; the keepers hop and land as the discs.

import type { Section } from "../bible";
import { rng } from "../math";
import {
  ARM0,
  chipPop,
  clockDays,
  FLAT0,
  FLAT1,
  SPIN0,
  SPIN1,
  slapAt,
  T_BEAM0,
  T_BEAMEND,
  T_CLOCK,
  T_CRUMPLE,
  T_CRUSH,
  T_DAYS,
  T_DAYS0,
  T_DISC,
  T_DROP,
  T_HOP,
  T_LIFT,
  T_PLANE,
  T_POP0,
  T_RULES,
  T_STOMP,
  world as city,
} from "../scenes/s6-world";
import type { Part } from ".";
import { ad, crossing, glide, hold, hz, line, type Mix, perc, swell, sweep } from "./mix";
import { A, bassBars, CHORD, chordBars, drumBars, G } from "./grooves";
import { ding, flick, knock, ping, pop, thump, tick, whoosh } from "./sounds";

/**
 * The grid draws out, the center cube stomps on b0.25 (s6 T_STOMP), and the
 * city rains in: each ring of equal distance touches down together, from
 * b0.5 at distance 1 to b2 at the rim (each cell's `land`). Rings climb
 * the D major pentatonic.
 */
function rain(m: Mix, s: Section): void {
  const t0 = s.start;
  whoosh(m, ad(t0, t0 + 0.08, 0.04, t0 + 0.4), sweep(t0, 9000, t0 + 0.3, 4500), 1, { send: 0.3 }, "white");
  const stomp = s.at(T_STOMP);
  thump(m, stomp, 0.5, 150, 55, 0.3);
  // The stomp's shock ring pops the first ring into being, hot, three
  // frames later (s6 T_POP0): four sparks of light.
  const pop0 = s.at(T_POP0);
  [93, 98, 102, 105].forEach((n, i) => {
    ping(m, pop0 + i * 0.006, hz(n), 0.09, 0.16, { pan: [-0.3, 0.3, -0.15, 0.15][i], send: 0.3 });
  });
  // One voice per touchdown time, weighted by how many cartons land on it.
  const rings = new Map<number, { t: number; n: number }>();
  for (const c of city()) {
    if (c.center) continue;
    const key = Math.round(c.land * 1e4);
    const r = rings.get(key) ?? { t: c.land, n: 0 };
    r.n++;
    rings.set(key, r);
  }
  const scale = [62, 64, 66, 69, 71, 74, 76, 78, 81, 83, 86, 88, 90, 93];
  [...rings.values()]
    .sort((a, b) => a.t - b.t)
    .forEach(({ t: lt, n }, i) => {
      const t = s.at(lt);
      m.duck(t, 0.08, 0.06);
      const weight = Math.min(1, 0.45 + n / 14);
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
  // Each checkout's name chip pops onto its carton (s6 chipPop), a soft
  // cork a step above the ring it rides in on.
  let k = 0;
  for (const c of city()) {
    if (!c.name) continue;
    pop(m, s.at(chipPop(c)), hz([81, 83, 86, 88, 90, 93][k % 6]), 0.07, (k % 2 ? -1 : 1) * 0.25, 0.2);
    k++;
  }
}

/**
 * The orphan's folder lifts out of its chip on b2.25 (s6 T_LIFT), is
 * crushed in the air from T_CRUSH, and drops on b2.5 (T_CRUMPLE) as its
 * name is struck through.
 */
function crumple(m: Mix, s: Section): void {
  const lift = s.at(T_LIFT);
  flick(m, lift, -0.2, 0.12);
  // The crush: paper crackle tightening into the ball, loudest as it closes.
  const c0 = s.at(T_CRUSH);
  const c1 = s.at(T_CRUMPLE);
  const v = m.voice(swell(c0, c0 + 0.01, 0.05, c1 - 0.01, 0.2, c1 + 0.06), { pan: -0.2, send: 0.15 });
  if (v) {
    v.noise("crackle", 1, v.filter("bandpass", sweep(c0, 5200, c1, 2200), 1.1), 1.6);
    v.noise("white", 0.35, v.filter("highpass", 6000, 0));
  }
  const r = rng(812);
  for (let i = 0; i < 6; i++) {
    const t = c0 + ((c1 - c0) * (i + r())) / 6;
    const q = m.voice(perc(t, 0.05 + 0.02 * i, 0.0005, 0.03), { pan: -0.2, send: 0.1 });
    if (q) q.noise("white", 1, q.filter("bandpass", 2400 + 2600 * r(), 2.5));
  }
  // The ball closes, and the strike runs through the name.
  knock(m, c1, 260, 0.12, -0.2, 0.08);
  whoosh(m, ad(c1, c1 + 0.03, 0.05, c1 + 0.14), sweep(c1, 1800, c1 + 0.12, 4200), 4, { pan: line(c1, -0.35, c1 + 0.12, 0), send: 0.05 }, "white");
}

/**
 * The 30-day clock pops onto the stale carton on b3 (s6 T_CLOCK), its hand
 * runs a lap from b3.25 to b4 (s6 clockDays) with a tick every three days,
 * and it rings as the month runs out and the dust settles.
 */
function clock(m: Mix, s: Section): void {
  pop(m, s.at(T_CLOCK), hz(79), 0.12, -0.3, 0.2);
  for (let d = 1; d < 10; d++) {
    const t = s.at(crossing(clockDays, d / 10, T_DAYS0, T_DAYS - T_DAYS0));
    tick(m, t, d % 5 === 0 ? 3000 : 2400, 0.1 + 0.012 * d, -0.3, 0.05);
  }
  const done = s.at(T_DAYS);
  ding(m, done, hz(84), 0.1, -0.3, 0.7);
  ding(m, done + 0.06, hz(84), 0.06, -0.25, 0.5);
  // The dust: a soft dry breath.
  whoosh(m, ad(done, done + 0.1, 0.04, done + 0.5), sweep(done, 900, done + 0.5, 300), 1.5, { pan: -0.2, send: 0.2 });
}

/**
 * The disk budget draws on at b3.75 (s6 T_PLANE): its posts rise and its
 * lid traces round both ways. Then the second wave piles through it from
 * b4, every carton a cardboard knock, and the first one through the lid
 * sets off a low warning.
 */
function budget(m: Mix, s: Section): void {
  const p = s.at(T_PLANE);
  const v = m.voice(ad(p, p + 0.12, 0.06, p + 0.3), { send: 0.25 });
  if (v) {
    v.osc("triangle", glide(p, p + 0.13, hz(62), hz(74), (u) => 1 - (1 - u) ** 2));
    v.noise("white", 0.4, v.filter("bandpass", sweep(p + 0.07, 2500, p + 0.26, 6000), 5));
  }
  const r = rng(29);
  const lands: number[] = [];
  for (const c of city()) for (const u of c.ups) lands.push(u.land);
  lands.sort((a, b) => a - b);
  lands.forEach((lt, i) => {
    const t = s.at(lt);
    knock(m, t, 150 + 90 * r(), 0.1 + (i === 0 ? 0.1 : 0.04 * r()), (r() - 0.5) * 0.8, 0.14);
  });
  // Over budget: a low two-note buzz under the pile.
  const o = s.at(lands[0]);
  m.duck(o, 0.12, 0.2);
  const w = m.voice(hold(o, 0.01, 0.07, o + 0.26, 0.03, 0.12), { send: 0.1 });
  if (w) {
    const lp = w.filter("lowpass", 900, 4);
    w.osc("square", [[o, hz(45)], [o + 0.13, hz(45), "set"], [o + 0.131, hz(44), "set"]], 0.5, lp);
    w.osc("sawtooth", hz(57), 0.25, lp);
  }
}

/**
 * The scanner arms around the grid's border from b4.375 (s6 ARM0) and the
 * beam ignites at the back corner on b5, sweeping to the front by b8
 * (T_BEAMEND). Each rule knocks as its line lands (T_RULES: b5.25, b6,
 * b6.75), and its group slaps flat with it: the orphan, the stale cartons,
 * then everything the beam has scanned over budget in one ripple and the
 * rest as the beam reaches them (s6 slapAt); cartons slapping on the same
 * frame share a voice. Each keeper chimes as the beam tags it (its `hit`).
 */
function prune(m: Mix, s: Section): void {
  const t0 = s.at(T_BEAM0);
  const end = s.at(T_BEAMEND);
  // Two pen tips race round the border from the front corner, one each
  // way, speeding up (inQuad) until they meet as the beam ignites.
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
  // The gantry hums while it sweeps and cuts out as it leaves the front corner.
  const v = m.voice(hold(t0, 0.03, 0.11, end - 0.008, 0.13, 0.04), { send: 0.18, hold: true });
  if (v) {
    const am = v.vca(0.6);
    v.lfo("sine", 24, 0.3, am.gain);
    v.noise("white", 0.8, v.filter("bandpass", 3400, 7, am));
    v.osc("sine", sweep(t0, hz(81), end, hz(93)), 0.18, am);
  }
  const x = m.voice(perc(end, 0.14, 0.002, 0.09), { send: 0.25, pan: 0.1 });
  if (x) {
    x.noise("white", 1, x.filter("bandpass", sweep(end, 6000, end + 0.08, 2000), 3));
    x.osc("sine", sweep(end, hz(93), end + 0.06, hz(86)), 0.25);
  }
  // A knock and a thud per rule, climbing, as its line lands.
  T_RULES.forEach((lt, i) => {
    const t = s.at(lt);
    m.duck(t, 0.2, 0.12);
    knock(m, t, [196, 220, 247][i], 0.34, 0, 0.16);
    thump(m, t, 0.35 + 0.1 * i, 120, 50, 0.16);
  });
  // The slaps, grouped by the frame they land on.
  const groups = new Map<number, { t: number; n: number; side: number }>();
  for (const c of city()) {
    if (c.keep) continue;
    const at = slapAt(c);
    const key = Math.round(at * 120);
    const g = groups.get(key) ?? { t: at, n: 0, side: 0 };
    g.n += 1 + c.ups.length;
    g.side += c.x - c.z;
    groups.set(key, g);
  }
  const r = rng(606);
  for (const { t: lt, n, side } of [...groups.values()].sort((a, b) => a.t - b.t)) {
    const slap = s.at(lt);
    const f = 200 + r() * 160;
    const w = Math.min(1, 0.4 + n / 8);
    const c = m.voice(perc(slap, 0.13 * w, 0.001, 0.16), { pan: Math.max(-0.5, Math.min(0.5, (0.12 * side) / n)), send: 0.1 });
    if (!c) continue;
    c.noise("crackle", 1, c.filter("lowpass", sweep(slap, 3000, slap + 0.14, 500), 0), 1.3);
    c.noise("white", perc(slap, 0.55, 0.0005, 0.02), c.filter("bandpass", 1100, 1.1), 1, slap + 0.04);
    c.osc("sine", sweep(slap, f * 1.8, slap + 0.12, f * 0.6), 0.5);
  }
  // Each keeper chimes as the beam tags it, higher toward the front row.
  const tagged = new Map<number, number>();
  for (const c of city()) if (c.keep) tagged.set(c.hit, (tagged.get(c.hit) ?? 0) + 1);
  [...tagged]
    .sort((a, b) => a[0] - b[0])
    .forEach(([at, count], i) => ping(m, s.at(at), hz([81, 86, 90][Math.min(i, 2)]), 0.1 + 0.02 * count, 0.45, { send: 0.3 }));
}

/**
 * On b8 (s6 T_DROP) the flattened carpet drops through the floor like
 * trapdoors, gone in about five frames, and the keepers glow. They hop on
 * b9.25 (T_HOP), turn a quarter (SPIN0 to SPIN1), flatten (FLAT0 to FLAT1)
 * and land as the discs on b11.5 (T_DISC).
 */
function keepers(m: Mix, s: Section): void {
  const t = s.at(T_DROP);
  m.duck(t, 0.2, 0.15);
  const d = m.voice(perc(t, 0.22, 0.002, 0.2), { send: 0.12 });
  if (d) {
    d.noise("pink", 1, d.filter("lowpass", sweep(t, 1400, t + 0.12, 180), 2));
    d.osc("sine", sweep(t, 120, t + 0.1, 48), 0.6);
  }
  whoosh(m, swell(t - 0.1, t - 0.06, 0.01, t - 0.004, 0.08, t + 0.01), sweep(t - 0.1, 600, t, 1800), 1.5, { send: 0.1 });
  // The glow: a fast strum across the seven keepers, B minor pentatonic over the Bm9 pad.
  [71, 74, 76, 78, 81, 83, 86].forEach((n, i) => {
    const ti = t + i * 0.012;
    const v = m.voice(ad(ti, ti + 0.004, 0.05, ti + 0.55), { pan: (i % 2 ? -1 : 1) * (0.1 + i * 0.06), send: 0.4 });
    if (!v) return;
    v.osc("sine", hz(n));
    v.osc("sine", hz(n) * 2.001, 0.15);
    v.osc("triangle", hz(n), 0.2);
  });
  // The hop: a springy rise through the turn.
  const h = s.at(T_HOP);
  const hv = m.voice(perc(h, 0.2, 0.003, 0.3), { send: 0.2 });
  if (hv) {
    hv.osc("sine", [[h, hz(50)], [h + 0.1, hz(62), "exp"], [h + 0.3, hz(57), "exp"]]);
    hv.osc("triangle", [[h, hz(62)], [h + 0.1, hz(74), "exp"]], perc(h, 0.2, 0.002, 0.08));
  }
  whoosh(m, ad(s.at(SPIN0), s.at(SPIN0) + 0.25, 0.05, s.at(SPIN1)), sweep(s.at(SPIN0), 700, s.at(SPIN1), 2400), 2, { send: 0.25 });
  // They flatten and round off into the discs, and land.
  const f0 = s.at(FLAT0);
  const end = s.at(T_DISC);
  whoosh(m, ad(f0 - 0.1, s.at(FLAT1), 0.09, end + 0.06), sweep(f0 - 0.1, 3500, end, 500), 1.2, { send: 0.3 });
  [74, 78, 81].forEach((n, i) => pop(m, end + i * 0.008, hz(n), 0.08, (i - 1) * 0.3, 0.3));
}

export const part: Part = {
  cues(m, s) {
    rain(m, s);
    crumple(m, s);
    clock(m, s);
    budget(m, s);
    prune(m, s);
    keepers(m, s);
  },
  drums: (m, s) => drumBars(m, s, [[[0, 8, 11], [4, 12], [7]]]),
  bass: (m, s) => bassBars(m, s, [[35, 0.5], G, A]),
  pads: (m, s) => chordBars(m, s, [[47, 54, 57, 62, 73], CHORD.G, CHORD.A], 1800),
};
