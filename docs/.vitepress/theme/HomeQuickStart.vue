<script setup lang="ts">
import { ref } from "vue";

const command =
  'mise use --global --postinstall "mbx setup --yes" mr-boxington';
const status = ref("");

async function copyCommand() {
  try {
    await navigator.clipboard.writeText(command);
    status.value = "Copied";
  } catch {
    status.value = "Select the command to copy it.";
  }
}
</script>

<template>
  <section class="MbxQuickStart" aria-label="Install with mise">
    <div class="install-command">
      <code>{{ command }}</code>
      <button
        type="button"
        aria-label="Copy mise installation command"
        @click="copyCommand"
      >
        Copy
      </button>
    </div>
    <p class="install-note">
      <span>Linux · macOS · Windows</span>
      <a href="/installation"
        >More install options <span aria-hidden="true">↗</span></a
      >
    </p>
    <p class="copy-status" role="status">{{ status }}</p>
  </section>
</template>

<style scoped>
.MbxQuickStart {
  margin-top: 28px;
  max-width: 580px;
}
.install-command {
  align-items: center;
  background: var(--vp-code-block-bg);
  border: 1px solid var(--vp-c-divider);
  border-radius: 8px;
  display: flex;
  gap: 12px;
  padding: 10px 10px 10px 16px;
  text-align: left;
}
code {
  color: var(--vp-c-text-1);
  font-family: var(--vp-font-family-mono);
  font-size: 12px;
  line-height: 1.8;
  overflow-wrap: anywhere;
  user-select: all;
}
button {
  background: var(--vp-c-bg-soft);
  border: 1px solid var(--vp-c-divider);
  border-radius: 5px;
  color: var(--vp-c-brand-1);
  cursor: pointer;
  flex-shrink: 0;
  font-size: 12px;
  min-height: 44px;
  padding: 0 12px;
}
button:hover {
  border-color: var(--vp-c-brand-1);
}
.install-note {
  color: var(--vp-c-text-2);
  display: flex;
  flex-wrap: wrap;
  font-size: 12px;
  gap: 8px 20px;
  justify-content: space-between;
  line-height: 1.6;
  margin-top: 12px;
}
a {
  color: var(--vp-c-brand-1);
}
a:hover {
  text-decoration: underline;
}
.copy-status {
  color: var(--vp-c-brand-1);
  font-size: 12px;
  min-height: 20px;
  text-align: left;
}
@media (max-width: 959px) {
  .MbxQuickStart {
    margin-left: auto;
    margin-right: auto;
  }
}
</style>
