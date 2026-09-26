<script setup lang="ts">
import { withBase } from "vitepress";
import { onMounted, ref } from "vue";
import { data } from "../benchmarks.data";
import { data as showreel } from "../showreel.data";
import { describeChapters } from "./showreel/describe";
import { factsFromBenchmarks } from "./showreel/facts";

// The reel is rendered to MP4 files by `mise run render:showreel` (the docs
// deploy runs it), so this is a plain video player. Builds without a render
// leave the section out.

const facts = factsFromBenchmarks(data);
const described = describeChapters(facts);

// The page is served with the 60 fps file, which plays everywhere. Once it is
// mounted, and before anyone presses play, it switches to the 120 fps file if
// the browser says it decodes that smoothly and power-efficiently (in
// practice, in hardware). This tests the decoder, not the display, so a
// capable 60 Hz screen gets the larger file too. Nothing downloads until play.
const player = ref<HTMLVideoElement>();
const src = ref(showreel?.src ?? "");
onMounted(async () => {
  const video120 = showreel?.video120;
  if (!video120 || !navigator.mediaCapabilities) return;
  try {
    const { smooth, powerEfficient } = await navigator.mediaCapabilities.decodingInfo({
      type: "file",
      video: {
        // H.264 High at level 5.1, as the renderer encodes it.
        contentType: 'video/mp4; codecs="avc1.640033"',
        width: 1920,
        height: 1080,
        framerate: 120,
        bitrate: video120.bitrate,
      },
    });
    // Someone who already pressed play keeps the file that is playing.
    const idle = player.value?.paused && player.value.readyState === HTMLMediaElement.HAVE_NOTHING;
    if (smooth && powerEfficient && idle) src.value = video120.src;
  } catch {
    // Older browsers reject the query; they keep the 60 fps file.
  }
});
</script>

<template>
  <section v-if="showreel" class="MbxShowreel home-section" aria-label="Showreel">
    <figure>
      <!-- No autoplay, and nothing downloads until someone presses play. -->
      <video
        ref="player"
        :src="withBase(src)"
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
