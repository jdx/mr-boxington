<script setup lang="ts">
import { ref } from "vue";

// An illustration of cache behavior, deliberately independent of benchmark data.
const warm = ref(false);
const crates = ["libc", "serde", "your-app"];
</script>

<template>
  <section class="MbxDemo home-section" aria-labelledby="demo-title">
    <div class="demo-copy">
      <p class="home-eyebrow">01 / Build here. Reuse there.</p>
      <h2 id="demo-title">A new worktree.<br />A head start.</h2>
      <p>
        Build once, then restore matching work from another checkout or CI.
      </p>
      <a class="home-text-link" href="/how-it-works#portable-keys"
        >How the cache travels <span aria-hidden="true">→</span></a
      >
    </div>
    <div class="terminal">
      <div class="titlebar">
        <span class="terminal-dots" aria-hidden="true">● ● ●</span>
        <span>two worktrees / one store</span>
        <span class="demo-label">illustration</span>
      </div>
      <div
        class="scenario-controls"
        role="group"
        aria-label="Choose a build scenario"
      >
        <button type="button" :aria-pressed="!warm" @click="warm = false">
          First build
        </button>
        <button type="button" :aria-pressed="warm" @click="warm = true">
          New worktree
        </button>
      </div>
      <div class="screen" role="status" aria-live="polite" aria-atomic="true">
        <p class="command">
          <span class="path">{{ warm ? "~/review" : "~/project" }}</span>
          <span class="prompt">$</span> mbx build
        </p>
        <div v-for="crate in crates" :key="crate" class="compiler-row">
          <span class="outcome" :class="{ restored: warm }">{{
            warm ? "Restored" : "Compiling"
          }}</span>
          <span>{{ crate }}</span>
          <span class="row-note">{{
            warm ? "cache hit" : "stored for later"
          }}</span>
        </div>
        <p class="result">
          {{
            warm
              ? "Matching work, ready to use."
              : "Freshly compiled. Safely tucked away."
          }}
        </p>
      </div>
      <p class="terminal-note">
        {{
          warm
            ? "Matching inputs can reuse outputs. Changed or unsupported work still compiles."
            : "A cold cache fills as you build. Cargo still skips any work it already considers fresh."
        }}
      </p>
    </div>
  </section>
</template>

<style scoped>
.MbxDemo {
  align-items: center;
  display: grid;
  gap: 48px;
  grid-template-columns: minmax(0, 0.85fr) minmax(0, 1.15fr);
  padding-bottom: 72px;
  padding-top: 56px;
}
.demo-copy > p:not(.home-eyebrow) {
  color: var(--vp-c-text-2);
  font-size: 17px;
  line-height: 1.8;
  margin: 20px 0 24px;
  max-width: 390px;
}
.terminal {
  background: #11100d;
  border: 1px solid var(--vp-c-divider);
  border-radius: 12px;
  box-shadow: 0 18px 48px -24px #000;
  min-width: 0;
}
.titlebar {
  align-items: center;
  border-bottom: 1px solid var(--vp-c-divider);
  color: var(--vp-c-text-2);
  display: flex;
  flex-wrap: wrap;
  font-family: var(--vp-font-family-mono);
  font-size: 10px;
  gap: 12px;
  padding: 14px 18px;
}
.terminal-dots {
  color: #766444;
  letter-spacing: 2px;
}
.demo-label {
  color: var(--mbx-teal-light);
  margin-left: auto;
}
.scenario-controls {
  display: flex;
  gap: 4px;
  padding: 18px 20px 0;
}
.scenario-controls button {
  border: 1px solid transparent;
  border-radius: 5px;
  color: var(--vp-c-text-2);
  cursor: pointer;
  font-size: 12px;
  min-height: 44px;
  padding: 8px 12px;
}
.scenario-controls button[aria-pressed="true"] {
  background: var(--vp-c-brand-soft);
  border-color: var(--vp-c-divider);
  color: var(--vp-c-brand-1);
}
.scenario-controls button:hover {
  color: var(--vp-c-text-1);
}
.screen {
  font-family: var(--vp-font-family-mono);
  font-size: 12px;
  line-height: 1.8;
  padding: 22px 24px 16px;
}
.command {
  color: var(--mbx-paper);
  margin-bottom: 22px;
}
.path {
  color: var(--mbx-teal-light);
}
.prompt {
  color: var(--vp-c-brand-1);
  margin: 0 6px;
}
.compiler-row {
  align-items: baseline;
  display: grid;
  gap: 12px;
  grid-template-columns: 9ch 1fr auto;
  padding: 7px 0;
}
.outcome {
  color: var(--vp-c-brand-1);
}
.outcome.restored {
  color: var(--mbx-cargo-green);
}
.row-note {
  color: var(--vp-c-text-3);
  font-size: 10px;
}
.result {
  border-top: 1px dashed var(--vp-c-divider);
  color: var(--mbx-paper);
  margin-top: 22px;
  padding-top: 16px;
}
.terminal-note {
  border-top: 1px solid var(--vp-c-divider);
  color: var(--vp-c-text-2);
  font-size: 12px;
  line-height: 1.7;
  min-height: 68px;
  padding: 14px 24px;
}
@media (max-width: 800px) {
  .MbxDemo {
    gap: 28px;
    grid-template-columns: 1fr;
  }
  .demo-copy > p:not(.home-eyebrow) {
    max-width: 540px;
  }
}
@media (max-width: 400px) {
  .screen {
    padding-left: 16px;
    padding-right: 16px;
  }
  .row-note {
    display: none;
  }
  .compiler-row {
    grid-template-columns: 9ch 1fr;
  }
  .titlebar {
    gap: 8px;
  }
  .terminal-dots {
    display: none;
  }
}
</style>
