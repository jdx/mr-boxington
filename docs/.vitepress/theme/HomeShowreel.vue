<script setup lang="ts">
import { withBase } from "vitepress";
import { data } from "../benchmarks.data";
import { data as showreel } from "../showreel.data";
import type { ReelFacts } from "./showreel/bible";
import { factsFromBenchmarks } from "./showreel/facts";
import { SECTIONS, type SectionId } from "./showreel/timeline";

// The reel is rendered to an MP4 by `mise run render:showreel` (the docs deploy
// runs it), so this is a plain video player. Builds without a render leave the
// section out.

const facts = factsFromBenchmarks(data);
const secs = (n: number) => `${n.toFixed(1)} seconds`;

/** The chart's delta, from the raw medians and then rounded, as the chart draws it. */
function savedText(c: NonNullable<ReelFacts["commit"]>): string {
  const delta = Math.round((c.cargo - c.mbx) * 10) / 10;
  return delta > 0 ? `, ${secs(delta)} less` : "";
}

/** What each chapter shows, in the reel's order, with the numbers it draws. */
function describeChapters(f: ReelFacts | null) {
  const bench = f?.subject ? `the ${f.subject} benchmark` : "the benchmark";
  const warm = f?.warm;
  const commit = f?.commit;
  const text: Record<SectionId, string> = {
    fold: "Two pens draw a flat cardboard net. It folds up into a box, the lid slams shut, and a strip of tape runs down the front.",
    "mr-boxington":
      "The box leaps, lands, and comes to life as Mr Boxington: a skeptical eye, a monocle on a brass chain, a handlebar mustache, and rosy cheeks. His name card reads mr boxington, a shared cache for Cargo builds.",
    what: "The camera dives into his monocle, and the line mbx reuses Cargo compiler work across projects, worktrees, and CI slams into place.",
    "every-checkout":
      "Without a shared cache, a project, a worktree, and a CI runner each run cargo build and compile the same crates again, and old target/ directories pile up.",
    "under-cargo-build":
      "You keep typing cargo build. Cargo plans the build, and mbx wraps each rustc call it makes, with no daemon to manage.",
    "first-build":
      "The first build compiles as usual, and mbx stores each output under a key of its inputs, with the checkout's paths replaced by placeholders. Nothing was cached yet, so Mr Boxington finishes taped, without a strawberry.",
    "same-checkout": warm
      ? `Measured in ${bench}: the same checkout with an empty target/ restores ${warm.hits} of ${warm.lookups} compilations from the store in ${secs(warm.seconds)}. Mr Boxington blushes, is taped, and gets a strawberry.`
      : `Measured in ${bench}: the same checkout with an empty target/ restores its compilations from the store instead of recompiling them. Mr Boxington blushes, is taped, and gets a strawberry.`,
    "another-worktree":
      "A new worktree at another path computes the same keys, so its matching crates are restored and only the edited crate compiles.",
    "six-builds":
      "Six builds running at once share one pool of compiler permits. Cache hits never wait, and a compilation two of the builds need runs once.",
    ci: "In CI, pushes to main publish compiled outputs to a remote cache, such as the GitHub Actions cache, a cache server, or an S3 bucket, and pull requests only restore from it.",
    "next-push": commit
      ? `A bar chart of ${bench}'s next push in CI, with the cache holding the previous commit: Cargo alone takes ${secs(commit.cargo)} and Cargo with mbx takes ${secs(commit.mbx)}${savedText(commit)}.`
      : `A bar chart compares Cargo alone with Cargo and mbx on ${bench}'s next push in CI, with the cache holding the previous commit.`,
    pruned:
      "target/ lives in the cache. Cartons of old target directories pile up, and after a build mbx removes those whose checkout is gone or that went unused for 30 days, then the least recently used while the pile is over its disk budget.",
    morph: "The cartons that were kept melt together into one liquid shape, which sets into Mr Boxington's outline.",
    end: "Mr Boxington returns in full with a strawberry beside his tape, above the name mr boxington, the line A shared cache for Cargo builds, the command cargo install mbx --locked && mbx setup, and the address mr-boxington.jdx.dev.",
  };
  return SECTIONS.map(({ id, label }) => ({ label, text: text[id] }));
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
        aria-label="Mr Boxington showreel: how mbx caches Cargo builds. A cardboard box with a skeptical eye, a monocle on a brass chain and a handlebar mustache. Chapters are listed below."
      >
        <!-- Generated from the reel's sections; see showreel/timeline.ts. -->
        <track kind="chapters" srclang="en" label="Chapters" :src="withBase('/showreel-chapters.vtt')" default />
      </video>
      <ol class="sr-only" aria-label="Showreel chapters">
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
