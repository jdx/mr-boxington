<template>
  <footer class="EndevFooter">
    <span>MIT License</span>
    <span aria-hidden="true">·</span>
    <span>Copyright © {{ year }}</span>
    <span aria-hidden="true">·</span>
    <a class="EndevFooterLink" href="https://jdx.dev">
      <span aria-hidden="true" class="EndevFooterLogo">j</span>
      <span>jdx.dev</span>
    </a>
  </footer>
</template>

<script setup>
const year = new Date().getFullYear();
</script>

<style scoped>
.EndevFooter { align-items: center; border-top: 1px solid var(--vp-c-divider); color: var(--vp-c-text-2); display: flex; flex-wrap: wrap; font-size: 14px; gap: 8px; justify-content: center; line-height: 28px; margin-top: auto; padding: 22px 24px 26px; text-align: center; }
.EndevFooterLink { align-items: center; color: inherit; display: inline-flex; gap: 8px; text-decoration: none; }
.EndevFooterLink:hover { color: var(--vp-c-brand-1); }
.EndevFooterLogo { align-items: center; background: var(--vp-c-brand-soft); border: 1px solid var(--vp-c-brand-1); border-radius: 8px; color: var(--vp-c-brand-1); display: inline-flex; font-family: Georgia, serif; font-size: 18px; font-weight: 700; height: 28px; justify-content: center; width: 28px; }
</style>
