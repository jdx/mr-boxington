<script setup lang="ts">
import { withBase } from "vitepress";
import { data } from "../benchmarks.data";
import { data as showreel } from "../showreel.data";
import type { ReelFacts } from "./showreel/bible";
import { factsFromBenchmarks } from "./showreel/facts";

// The reel is rendered to an MP4 by `mise run render:showreel` (the docs deploy
// runs it), so this is a plain video player. Builds without a render leave the
// section out.

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
</script>

<template>
  <section v-if="showreel" class="MbxShowreel home-section" aria-label="Showreel">
    <figure>
      <!-- No autoplay, and nothing downloads until someone presses play. -->
      <video
        :src="withBase(showreel.src)"
        :poster="withBase(showreel.poster)"
        width="1920"
        height="1080"
        controls
        playsinline
        preload="none"
        aria-label="Showreel: Mr Boxington, a cardboard box with a monocle and bow tie, folds into shape and shows matching Cargo build outputs being reused across projects, worktrees, and CI. Chapters are listed below."
        aria-describedby="mbx-showreel-chapters"
      />
      <ol id="mbx-showreel-chapters" class="sr-only">
        <li v-for="c in described" :key="c.label">{{ c.label }}: {{ c.text }}</li>
      </ol>
      <figcaption>
        Build times come from the
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
video {
  aspect-ratio: 16 / 9;
  background: #0e0c0a;
  border: 1px solid var(--vp-c-divider);
  border-radius: 12px;
  box-shadow:
    0 30px 80px -40px #000,
    0 14px 32px -22px rgb(0 0 0 / 0.7);
  display: block;
  height: auto;
  width: 100%;
}
video:focus-visible {
  outline: 2px solid var(--mbx-teal-light);
  outline-offset: 4px;
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
</style>
