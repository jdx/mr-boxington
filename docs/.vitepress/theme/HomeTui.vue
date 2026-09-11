<script setup lang="ts">
import { ref } from "vue";
import { withBase } from "vitepress";

const screens = [
  {
    name: "Live",
    image: "/screenshots/tui-live.png",
    alt: "mbx Live dashboard with build activity, hit and miss graphs, store capacity, and compilation savings",
    description: "Follow every build, watch cache traffic, and spot the biggest wins.",
  },
  {
    name: "Insights",
    image: "/screenshots/tui-insights.png",
    alt: "mbx Insights dashboard with cache outcome bars, action durations, bypass reasons, and savings rankings",
    description: "See what missed, what was bypassed, and which crates cost the most.",
  },
  {
    name: "Store",
    image: "/screenshots/tui-store.png",
    alt: "mbx Store dashboard with lifetime savings, automatic pruning totals, and estimated workspace sharing",
    description: "See how much work and cleanup mbx has done since it started counting.",
  },
];
const selected = ref(screens[0]);
</script>

<template>
  <section class="MbxTui" aria-labelledby="mbx-tui-title">
    <div class="heading">
      <p class="eyebrow">Your cache, in plain sight</p>
      <h2 id="mbx-tui-title">Watch the work you don’t have to do.</h2>
    </div>
    <div class="screen-picker" role="group" aria-label="Choose a dashboard screenshot">
      <button
        v-for="screen in screens"
        :key="screen.name"
        type="button"
        :aria-pressed="selected.name === screen.name"
        aria-controls="mbx-tui-screenshot"
        @click="selected = screen"
      >
        {{ screen.name }}
      </button>
      <span>Example build data</span>
    </div>
    <figure id="mbx-tui-screenshot">
      <a
        :href="withBase(selected.image)"
        target="_blank"
        rel="noopener"
        :aria-label="`Open the ${selected.name} screenshot at full size`"
      >
        <img
          :src="withBase(selected.image)"
          :alt="selected.alt"
          width="1504"
          height="1104"
          loading="lazy"
          decoding="async"
        />
      </a>
      <figcaption>
        <span aria-live="polite">{{ selected.description }}</span>
        <a :href="withBase(selected.image)" target="_blank" rel="noopener">
          View full size <span aria-hidden="true">↗</span>
        </a>
      </figcaption>
    </figure>
    <div class="links">
      <a :href="withBase('/tui')">Explore the dashboard <span aria-hidden="true">→</span></a>
    </div>
  </section>
</template>

<style scoped>
.MbxTui {
  background:
    radial-gradient(ellipse at top right, rgb(var(--mbx-teal-rgb) / 0.12), transparent 65%),
    var(--vp-c-bg-soft);
  border: 1px solid var(--vp-c-divider);
  border-radius: 18px;
  margin: 64px auto 0;
  max-width: 1152px;
  padding: 40px;
}

.heading { margin-bottom: 28px; }

.eyebrow {
  color: var(--mbx-teal-light);
  font-family: var(--vp-font-family-mono);
  font-size: 13px;
  font-weight: 600;
  letter-spacing: 0.08em;
  margin: 0 0 12px;
  text-transform: uppercase;
}

h2 {
  color: var(--vp-c-text-1);
  font-family: var(--mbx-display);
  font-size: clamp(30px, 4vw, 44px);
  letter-spacing: -0.035em;
  line-height: 1.08;
  margin: 0;
  max-width: 18ch;
}

.screen-picker,
.links {
  align-items: center;
  display: flex;
  flex-wrap: wrap;
  gap: 12px;
}

.screen-picker {
  margin-bottom: 16px;
}

button {
  border: 1px solid var(--vp-c-divider);
  border-radius: 8px;
  color: var(--vp-c-text-2);
  font-family: var(--vp-font-family-mono);
  padding: 9px 18px;
}

button:hover,
button[aria-pressed="true"] {
  background: rgb(var(--mbx-teal-rgb) / 0.12);
  border-color: var(--mbx-teal-light);
  color: var(--mbx-teal-light);
}

button:focus-visible,
a:focus-visible {
  outline: 2px solid var(--vp-c-brand-1);
  outline-offset: 4px;
}

.screen-picker span {
  color: var(--vp-c-text-3);
  font-size: 13px;
  margin-left: auto;
}

figure {
  margin: 0;
}

figure a {
  border: 1px solid var(--vp-c-divider);
  border-radius: 12px;
  display: block;
  overflow: hidden;
}

img {
  display: block;
  height: auto;
  width: 100%;
}

figcaption {
  color: var(--vp-c-text-2);
  display: flex;
  flex-wrap: wrap;
  font-size: 14px;
  gap: 8px 20px;
  justify-content: space-between;
  line-height: 1.6;
  margin-top: 14px;
  min-height: 3.2em;
}

figcaption a {
  border: 0;
  border-radius: 0;
  color: var(--vp-c-brand-1);
  white-space: nowrap;
}

.links {
  gap: 12px 28px;
  margin-top: 12px;
}

.links a {
  color: var(--vp-c-brand-1);
  font-weight: 600;
}

@media (max-width: 639px) {
  .MbxTui {
    border-left: 0;
    border-radius: 0;
    border-right: 0;
    margin-top: 48px;
    padding: 32px 24px;
  }

  button {
    padding: 9px 14px;
  }

  .screen-picker span {
    flex-basis: 100%;
    margin-left: 0;
  }
}
</style>
