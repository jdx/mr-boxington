<script setup>
import { onMounted, onUnmounted, ref } from "vue";

// A sample of the savings pool, with numbers consistent with the transcript
// below (191 hits; the cold build took 3m 41s, the warm one 4.9s). The real
// line is drawn at random per build; rotating here is how a static page shows
// that it does not repeat itself.
const quips = [
  "mbx: served 191 compilations from cache; rustc showed up ready to do 3m 36s of work and was sent home",
  "mbx: 6h 14m of compiling skipped across 87 builds. rustc suspects nothing.",
  "mbx: 47.0 GiB of build debris binned so far. cargo clean remains unemployed.",
  "mbx: your deleted worktrees left 41.0 GiB behind. left. past tense.",
  "mbx: every checkout believes it owns 22.0 GiB of outputs. the disk keeps one copy and says nothing.",
  "mbx: 4312 compilations on file. the box remembers.",
  "mbx: rustc believes it compiled everything. it is down 6h 14m across 87 builds. let it believe.",
  "mbx: 22.0 GiB in every checkout, 22.0 GiB on disk. arithmetic declined to comment.",
];
// Server and first client render must agree, so rotation starts on mount --
// and not at all for someone who asked for reduced motion, matching the
// entrance animations this component already suppresses for them.
const quip = ref(quips[0]);
let at = 0;
let timer;
onMounted(() => {
  if (window.matchMedia("(prefers-reduced-motion: reduce)").matches) {
    return;
  }
  timer = setInterval(() => {
    at = (at + 1) % quips.length;
    quip.value = quips[at];
  }, 4000);
});
onUnmounted(() => clearInterval(timer));
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
        aria-label="Terminal transcript: the first checkout compiles every crate in 3 minutes 41 seconds; a fresh worktree finishes in 4.9 seconds with 191 cache hits, and mbx adds one deadpan line about what the cache has saved over time"
        class="screen"
        role="img"
      >
        <div class="line"><span class="path">~/proj</span> <span class="prompt">$</span> <span class="cmd">mbx build</span></div>
        <div class="line"><span class="verb">   Compiling</span> libc v0.2.174</div>
        <div class="line"><span class="verb">   Compiling</span> serde v1.0.219</div>
        <div class="line dim">            … 193 more crates …</div>
        <div class="line"><span class="verb">    Finished</span> `dev` profile [unoptimized + debuginfo] target(s) in <span class="time">3m 41s</span></div>
        <div aria-hidden="true" class="line gap"></div>
        <div class="line"><span class="path">~/proj</span> <span class="prompt">$</span> <span class="cmd">git worktree add ../review &amp;&amp; cd ../review</span></div>
        <div class="line"><span class="path">~/review</span> <span class="prompt">$</span> <span class="cmd">mbx build</span></div>
        <div class="line"><span class="verb">    Finished</span> `dev` profile [unoptimized + debuginfo] target(s) in <span class="time fast">4.9s</span></div>
        <div class="line cache">cache: 191 hits, 3 misses, 187 prefetched; 0 B downloaded, 0 B uploaded, 412.6 MiB stored locally</div>
        <div class="line quip">{{ quip }}</div>
        <div class="line"><span class="path">~/review</span> <span class="prompt">$</span> <span aria-hidden="true" class="cursor"></span></div>
      </div>
    </div>
    <p class="caption">The second checkout never compiles what the first already did — locally or in CI.</p>
  </section>
</template>

<style scoped>
.MbxDemo {
  margin: 0 auto;
  max-width: 852px;
  padding: 16px 24px 8px;
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
  animation: mbx-line-in 0.35s ease both;
  white-space: pre;
}

.line:nth-child(1) { animation-delay: 0.15s; }
.line:nth-child(2) { animation-delay: 0.45s; }
.line:nth-child(3) { animation-delay: 0.6s; }
.line:nth-child(4) { animation-delay: 0.75s; }
.line:nth-child(5) { animation-delay: 1.15s; }
.line:nth-child(6) { animation-delay: 1.15s; }
.line:nth-child(7) { animation-delay: 1.45s; }
.line:nth-child(8) { animation-delay: 1.85s; }
.line:nth-child(9) { animation-delay: 2.15s; }
.line:nth-child(10) { animation-delay: 2.35s; }
.line:nth-child(11) { animation-delay: 2.6s; }
.line:nth-child(12) { animation-delay: 2.85s; }

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
}

.cursor {
  animation: mbx-line-in 0.35s ease 2.85s both, mbx-blink 1.1s steps(1) 3.2s infinite;
  background: var(--vp-c-brand-1);
  display: inline-block;
  height: 1.1em;
  vertical-align: text-bottom;
  width: 0.55em;
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
