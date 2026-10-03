<template>
  <a class="hero-song" href="/song">
    <span class="hero-song-play" aria-hidden="true">▶</span>
    <span class="hero-song-text"
      >New:
      <strong class="hero-song-title">Pray Silence for Mister Boxington</strong
      ><span class="hero-song-extra">, the theme song and music video</span
      ><span class="hero-song-short">the theme song</span></span
    >
    <span class="hero-song-arrow" aria-hidden="true">→</span>
  </a>
</template>

<style scoped>
/* Announcement chip above the hero name, linking to the theme song page. */
.hero-song {
  display: inline-flex;
  align-items: center;
  gap: 10px;
  max-width: 100%;
  margin-bottom: 20px;
  padding: 5px 14px 5px 5px;
  border: 1px solid var(--vp-c-divider);
  border-radius: 999px;
  color: var(--vp-c-text-2);
  background: var(--vp-c-bg-soft);
  font-size: 0.875rem;
  line-height: 1.4;
  text-decoration: none;
  transition:
    background-color 0.15s,
    border-color 0.15s,
    color 0.15s;
}
.hero-song strong {
  color: var(--vp-c-text-1);
  font-weight: 600;
}
.hero-song:hover {
  color: var(--vp-c-text-1);
  border-color: var(--vp-c-brand-1);
  background: var(--vp-c-brand-soft);
}
.hero-song-play {
  display: inline-flex;
  flex: none;
  align-items: center;
  justify-content: center;
  width: 26px;
  height: 26px;
  padding-left: 2px;
  border-radius: 50%;
  color: var(--mbx-ink);
  background: var(--mbx-amber);
  font-size: 0.625rem;
}
.hero-song-text {
  min-width: 0;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.hero-song-short {
  display: none;
}
.hero-song-arrow {
  flex: none;
  color: var(--vp-c-brand-1);
  transition: transform 0.15s;
}
.hero-song:hover .hero-song-arrow {
  transform: translateX(2px);
}
/* On narrow screens the title alone would wrap; say what it is instead. */
@media (max-width: 639px) {
  .hero-song-title,
  .hero-song-extra {
    display: none;
  }
  .hero-song-short {
    display: inline;
  }
}
</style>
