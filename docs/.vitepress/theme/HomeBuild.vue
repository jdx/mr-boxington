<script setup lang="ts">
import { withBase } from "vitepress";
import { ref } from "vue";

const paused = ref(false);
</script>

<template>
  <section class="MbxBuild" aria-labelledby="mbx-build-title">
    <div class="heading">
      <h2 id="mbx-build-title">Follow build progress</h2>
      <p>
        In an interactive terminal, <code>build</code>, <code>check</code>,
        <code>clippy</code>, <code>run</code>, and <code>test</code> show each
        compiling crate with a timer, label finished crates with their cache
        outcome, and color the progress bar by outcome. This applies to
        <code>mbx build</code> and to <code>cargo build</code> set up to use
        mbx. In CI, or with <code>display = "plain"</code>
        (<code>MBX_DISPLAY=plain</code>), mbx shows Cargo's own output instead.
      </p>
    </div>
    <figure>
      <picture>
        <source
          media="(prefers-reduced-motion: reduce)"
          :srcset="withBase('/screenshots/cargo-pretty.png')"
        />
        <img
          :src="withBase(`/screenshots/cargo-pretty.${paused ? 'png' : 'gif'}`)"
          alt="mbx build output with live crates and timers, finished crates marked hit or miss, a progress bar split into cache hits and misses, and estimated compiler time saved, beside the Mr Boxington mascot"
          width="1080"
          height="550"
          loading="lazy"
          decoding="async"
        />
      </picture>
      <figcaption>
        <code>mbx build</code> during a partly cached rebuild.
        <button type="button" @click="paused = !paused">
          {{ paused ? 'Play animation' : 'Pause animation' }}
        </button>
      </figcaption>
    </figure>
  </section>
</template>

<style scoped>
.MbxBuild {
  margin: 64px auto 0;
  max-width: 1152px;
  padding: 0 32px;
}
.heading {
  max-width: 700px;
  margin-bottom: 24px;
}
.eyebrow {
  color: var(--mbx-teal-light);
  font-family: var(--vp-font-family-mono);
  font-size: 13px;
  font-weight: 600;
}
h2 {
  color: var(--vp-c-text-1);
  font-family: var(--mbx-display);
  font-size: clamp(28px, 4vw, 40px);
  line-height: 1.15;
  margin: 12px 0;
}
figure { margin: 0; }
img {
  display: block;
  width: 100%;
  height: auto;
  border-radius: 15px;
}
figcaption {
  margin-top: 12px;
  color: var(--vp-c-text-2);
  font-size: 13px;
}
button {
  margin-left: 12px;
  color: var(--vp-c-brand-1);
  text-decoration: underline;
}
@media (max-width: 640px) {
  .MbxBuild { padding: 0 24px; }
}
@media (prefers-reduced-motion: reduce) {
  button { display: none; }
}
</style>
