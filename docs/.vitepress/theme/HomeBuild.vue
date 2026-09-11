<script setup lang="ts">
import { withBase } from "vitepress";
import { ref } from "vue";

const paused = ref(false);
</script>

<template>
  <section class="MbxBuild" aria-labelledby="mbx-build-title">
    <div class="heading">
      <p class="eyebrow">While Cargo does its work</p>
      <h2 id="mbx-build-title">A little company for the compile.</h2>
    </div>
    <figure>
      <picture>
        <source
          media="(prefers-reduced-motion: reduce)"
          :srcset="withBase('/screenshots/cargo-pretty.png')"
        />
        <img
          :src="withBase(`/screenshots/cargo-pretty.${paused ? 'png' : 'gif'}`)"
          alt="Animated Boxington mascot beside mbx build output showing live and completed crates"
          width="1080"
          height="550"
          loading="lazy"
          decoding="async"
        />
      </picture>
      <figcaption>
        Build output during a rebuild.
        <button type="button" :aria-pressed="paused" @click="paused = !paused">
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
