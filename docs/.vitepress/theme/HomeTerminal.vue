<script setup>
import { onMounted, ref } from "vue";

// A sample of the savings pool, with numbers consistent with the transcript
// below (191 hits; the cold build took 3m 41s, the warm one 4.9s). The real
// line is drawn at random per build. Pick one when the demo mounts and keep it
// for the lifetime of the page, just like one invocation of mbx would.
const quips = [
  "mbx[savings]: served 191 compilations from cache; rustc showed up ready to do 3m 36s of work and was sent home",
  "mbx[savings]: 6h 14m of compiling skipped across 87 builds. rustc suspects nothing.",
  "mbx[savings]: 47.0 GiB of build debris binned so far. cargo clean remains unemployed.",
  "mbx[savings]: your deleted worktrees left 41.0 GiB behind. left. past tense.",
  "mbx[savings]: every checkout believes it owns 22.0 GiB of outputs. the disk keeps one copy and says nothing.",
  "mbx[savings]: 4312 compilations on file. the box remembers.",
  "mbx[savings]: rustc believes it compiled everything. it is down 6h 14m across 87 builds. let it believe.",
  "mbx[savings]: 22.0 GiB in every checkout, 22.0 GiB on disk. arithmetic declined to comment.",
];
// Leave the SSR render empty so hydration agrees, then make the one random
// choice in the browser. The line arrives with the rest of the transcript.
const quip = ref("");
const buildsRun = ref(0);

function runBuild() {
  if (buildsRun.value < 2) {
    buildsRun.value += 1;
  }
}
onMounted(() => {
  quip.value = quips[Math.floor(Math.random() * quips.length)];
});
</script>

<template>
  <section class="MbxDemo" aria-label="mbx across two checkouts">
    <div class="window">
      <div class="titlebar">
        <span aria-hidden="true" class="dot"></span>
        <span aria-hidden="true" class="dot"></span>
        <span aria-hidden="true" class="dot"></span>
        <span class="label">two checkouts · one cache</span>
      </div>
      <div
        aria-live="polite"
        class="screen"
        role="log"
      >
        <div v-if="buildsRun === 0" class="line"><span class="path">~/proj</span> <span class="prompt">$</span> <span aria-hidden="true" class="cursor"></span></div>
        <template v-if="buildsRun >= 1">
          <div class="build-output cold-build">
            <div class="line"><span class="path">~/proj</span> <span class="prompt">$</span> <span class="cmd">mbx build</span></div>
            <div class="line"><span class="verb">   Compiling</span> libc v0.2.174</div>
            <div class="line"><span class="verb">   Compiling</span> serde v1.0.219</div>
            <div class="line dim">            … 193 more crates …</div>
            <div class="line"><span class="verb">    Finished</span> `dev` profile [unoptimized + debuginfo] target(s) in <span class="time">3m 41s</span></div>
            <div aria-hidden="true" class="line gap"></div>
            <div class="line"><span class="path">~/proj</span> <span class="prompt">$</span> <span class="cmd">git worktree add ../review &amp;&amp; cd ../review</span></div>
            <div v-if="buildsRun === 1" class="line"><span class="path">~/review</span> <span class="prompt">$</span> <span aria-hidden="true" class="cursor"></span></div>
          </div>
        </template>
        <template v-if="buildsRun >= 2">
          <div class="build-output warm-build">
            <div class="line"><span class="path">~/review</span> <span class="prompt">$</span> <span class="cmd">mbx build</span></div>
            <div class="line"><span class="verb">    Finished</span> `dev` profile [unoptimized + debuginfo] target(s) in <span class="time fast">4.9s</span></div>
            <div class="line cache">mbx[cache]: 191 hits, 3 misses, 187 prefetched; 0 B downloaded, 0 B uploaded, 412.6 MiB stored locally</div>
            <div class="line quip">{{ quip }}</div>
            <div class="line"><span class="path">~/review</span> <span class="prompt">$</span> <span aria-hidden="true" class="cursor"></span></div>
          </div>
        </template>
      </div>
      <div v-if="buildsRun < 2" class="controls">
        <button class="run" type="button" @click="runBuild">run cargo build</button>
        <span class="progress">build {{ buildsRun + 1 }} of 2</span>
      </div>
    </div>
    <p class="caption">The second checkout never compiles what the first already did — locally or in CI.</p>
  </section>
</template>

<style scoped>
.MbxDemo {
  box-sizing: border-box;
  margin: 0 auto;
  max-width: 1280px;
  padding: 16px 24px 8px;
  width: 100%;
}

@media (min-width: 640px) {
  .MbxDemo {
    padding: 24px 48px 8px;
  }
}

@media (min-width: 960px) {
  .MbxDemo {
    padding: 32px 64px 8px;
  }
}

.window {
  background: #100c07;
  border: 1px solid rgb(var(--mbx-amber-rgb) / 0.22);
  border-radius: 14px;
  box-shadow:
    0 30px 70px -30px rgba(0, 0, 0, 0.75),
    0 20px 60px -30px rgb(var(--mbx-amber-rgb) / 0.2);
  overflow: hidden;
}

.titlebar {
  align-items: center;
  background: rgb(var(--mbx-amber-rgb) / 0.07);
  border-bottom: 1px solid rgb(var(--mbx-amber-rgb) / 0.14);
  display: flex;
  gap: 8px;
  padding: 10px 14px;
}

.dot {
  background: rgb(var(--mbx-amber-rgb) / 0.35);
  border-radius: 50%;
  height: 11px;
  width: 11px;
}

.label {
  color: var(--vp-c-text-3);
  font-family: var(--vp-font-family-mono);
  font-size: 12px;
  letter-spacing: 0.04em;
  margin-left: auto;
}

.screen {
  font-family: var(--vp-font-family-mono);
  font-size: 13px;
  line-height: 1.75;
  overflow-x: auto;
  padding: 18px 20px;
}

.line {
  animation: mbx-line-in 0.45s ease both;
  white-space: pre;
}

.build-output .line:nth-child(1) { animation-delay: 0.05s; }
.build-output .line:nth-child(2) { animation-delay: 0.25s; }
.build-output .line:nth-child(3) { animation-delay: 0.4s; }
.build-output .line:nth-child(4) { animation-delay: 0.55s; }
.build-output .line:nth-child(5) { animation-delay: 0.85s; }

.cold-build .line:nth-child(6) { animation-delay: 0.85s; }
.cold-build .line:nth-child(7) { animation-delay: 1.05s; }
.cold-build .line:nth-child(8) { animation-delay: 1.25s; }

.warm-build .line:nth-child(2) { animation-delay: 0.35s; }
.warm-build .line:nth-child(3) { animation-delay: 0.55s; }
.warm-build .line:nth-child(4) { animation-delay: 0.75s; }
.warm-build .line:nth-child(5) { animation-delay: 0.95s; }

.gap {
  height: 0.9em;
}

.path {
  color: var(--mbx-teal-light);
}

.prompt {
  color: var(--vp-c-brand-1);
}

.cmd {
  color: var(--mbx-paper);
  font-weight: 600;
}

.verb {
  color: var(--mbx-cargo-green);
  font-weight: 600;
}

.dim {
  color: var(--vp-c-text-3);
}

.time {
  color: var(--mbx-paper);
  font-weight: 600;
}

.time.fast {
  background: rgb(var(--mbx-amber-rgb) / 0.18);
  border-radius: 4px;
  color: var(--vp-c-brand-1);
  padding: 1px 5px;
}

.cache {
  color: var(--vp-c-brand-1);
}

.quip {
  color: rgb(var(--mbx-amber-rgb) / 0.9);
  min-height: 1.75em;
}

.cursor {
  animation: mbx-blink 1.1s steps(1) infinite;
  background: var(--vp-c-brand-1);
  display: inline-block;
  height: 1.1em;
  vertical-align: text-bottom;
  width: 0.55em;
}

.controls {
  align-items: center;
  border-top: 1px solid rgb(var(--mbx-amber-rgb) / 0.14);
  display: flex;
  flex-direction: column;
  gap: 8px;
  padding: 18px 20px 20px;
}

.run {
  background: linear-gradient(135deg, var(--mbx-amber-bright), var(--mbx-amber-deep));
  border: 0;
  border-radius: 9px;
  box-shadow: 0 12px 28px -12px rgb(var(--mbx-amber-rgb) / 0.75);
  color: var(--mbx-ink);
  cursor: pointer;
  font-family: var(--vp-font-family-mono);
  font-size: 17px;
  font-weight: 700;
  min-height: 48px;
  padding: 12px 28px;
  transition: box-shadow 0.2s ease, filter 0.2s ease, transform 0.2s ease;
}

.run:hover {
  box-shadow: 0 16px 34px -12px rgb(var(--mbx-amber-rgb) / 0.9);
  filter: brightness(1.08);
  transform: translateY(-1px);
}

.run:active {
  transform: translateY(1px);
}

.run:focus-visible {
  outline: 3px solid var(--mbx-teal-light);
  outline-offset: 3px;
}

.progress {
  color: var(--vp-c-text-3);
  font-family: var(--vp-font-family-mono);
  font-size: 11px;
  letter-spacing: 0.08em;
  text-transform: uppercase;
}

.caption {
  color: var(--vp-c-text-2);
  font-size: 14px;
  margin-top: 14px;
  text-align: center;
}

@keyframes mbx-line-in {
  from {
    opacity: 0;
    transform: translateY(4px);
  }
  to {
    opacity: 1;
    transform: translateY(0);
  }
}

@keyframes mbx-blink {
  50% {
    opacity: 0;
  }
}

@media (prefers-reduced-motion: reduce) {
  .line,
  .cursor {
    animation: none;
  }
}
</style>
