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
        aria-label="Terminal transcript: the first checkout compiles every crate in 3 minutes 41 seconds; a fresh worktree finishes in 4.9 seconds with 191 cache hits"
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
  border: 1px solid rgba(230, 173, 84, 0.22);
  border-radius: 14px;
  box-shadow:
    0 30px 70px -30px rgba(0, 0, 0, 0.75),
    0 20px 60px -30px rgba(230, 173, 84, 0.2);
  overflow: hidden;
}

.titlebar {
  align-items: center;
  background: rgba(230, 173, 84, 0.07);
  border-bottom: 1px solid rgba(230, 173, 84, 0.14);
  display: flex;
  gap: 8px;
  padding: 10px 14px;
}

.dot {
  background: rgba(230, 173, 84, 0.35);
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

.gap {
  height: 0.9em;
}

.path {
  color: #7fa6ad;
}

.prompt {
  color: var(--vp-c-brand-1);
}

.cmd {
  color: #f5ead6;
  font-weight: 600;
}

.verb {
  color: #86c07e;
  font-weight: 600;
}

.dim {
  color: var(--vp-c-text-3);
}

.time {
  color: #f5ead6;
  font-weight: 600;
}

.time.fast {
  background: rgba(230, 173, 84, 0.18);
  border-radius: 4px;
  color: var(--vp-c-brand-1);
  padding: 1px 5px;
}

.cache {
  color: var(--vp-c-brand-1);
}

.cursor {
  animation: mbx-line-in 0.35s ease 2.6s both, mbx-blink 1.1s steps(1) 2.95s infinite;
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
