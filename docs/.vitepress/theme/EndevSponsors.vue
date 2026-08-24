<template>
  <section v-if="sponsors.length" class="EndevSponsors" aria-label="Sponsors">
    <span class="EndevSponsorsTitle">sponsors</span>
    <a v-for="sponsor in sponsors" :key="sponsor.url" class="EndevSponsorsLogo" :href="sponsor.url" rel="noopener noreferrer sponsored" target="_blank">
      <img :alt="sponsor.name" :src="sponsor.logo" loading="lazy" />
    </a>
    <a class="EndevSponsorsCta" href="https://jdx.dev/sponsors.html">View all sponsors</a>
  </section>
</template>

<script setup>
import { onMounted, ref } from "vue";

const sponsors = ref([]);
const footerTiers = new Set(["title", "premier", "partner"]);

onMounted(async () => {
  const controller = new AbortController();
  const timeout = window.setTimeout(() => controller.abort(), 5000);
  try {
    const response = await fetch("https://jdx.dev/sponsors.json", { signal: controller.signal });
    if (!response.ok) return;
    const payload = await response.json();
    sponsors.value = (Array.isArray(payload?.sponsors) ? payload.sponsors : []).filter(
      (sponsor) => sponsor && footerTiers.has(sponsor.tier) && typeof sponsor.name === "string" && /^https?:\/\//.test(sponsor.url) && /^https?:\/\//.test(sponsor.logo),
    );
  } catch {
    sponsors.value = [];
  } finally {
    window.clearTimeout(timeout);
  }
});
</script>

<style scoped>
.EndevSponsors { align-items: center; display: flex; flex-wrap: wrap; gap: 12px; justify-content: center; margin: 0 auto; max-width: 1000px; padding: 24px; }
.EndevSponsorsTitle { color: var(--vp-c-text-2); font-size: 13px; font-weight: 600; text-transform: uppercase; }
.EndevSponsorsLogo { align-items: center; border: 1px solid var(--vp-c-divider); border-radius: 8px; display: inline-flex; height: 40px; justify-content: center; padding: 8px 12px; }
.EndevSponsorsLogo:hover { border-color: var(--vp-c-brand-1); }
.EndevSponsorsLogo img { display: block; height: 22px; max-width: 120px; object-fit: contain; width: auto; }
.EndevSponsorsCta { color: var(--vp-c-text-2); font-size: 13px; text-decoration: none; }
.EndevSponsorsCta:hover { color: var(--vp-c-brand-1); }
</style>
