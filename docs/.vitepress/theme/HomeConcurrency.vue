<template>
  <section class="MbxConcurrency" aria-labelledby="mbx-concurrency-title">
    <div class="copy">
      <p class="eyebrow">Parallel CI, one machine</p>
      <h2 id="mbx-concurrency-title">
        Run multiple Cargo builds at the same time.
      </h2>
      <p class="lede">
        Cargo limits only the compilers in its own process. mbx coordinates
        every Cargo process through one CPU and memory budget, and identical
        work already running is compiled once and restored everywhere else.
      </p>
      <div class="proof" aria-label="Measured performance improvement">
        <strong>Up to 44.9%</strong>
        <span>less lint wall time on an xlarge CI runner</span>
      </div>
      <p class="validation">
        Two cold, order-reversed A/B trials compared the same Clippy commands
        sequentially and in parallel.
      </p>
      <div class="links">
        <a class="primary" href="/github-action#parallel-cargo-steps">
          Copy the workflow
        </a>
        <a href="/benchmarks#contention">See the benchmark →</a>
      </div>
    </div>

    <div
      class="diagram"
      aria-label="Two concurrent lint commands coordinated by mbx"
    >
      <div class="job">
        <span class="label">default features</span>
        <code>mbx clippy</code>
      </div>
      <div class="job">
        <span class="label">all features + targets</span>
        <code>mbx clippy --all-features --all-targets</code>
      </div>
      <div class="join" aria-hidden="true">
        <span></span>
        <span></span>
      </div>
      <div class="pool">
        <span class="pool-title">one mbx permit pool</span>
        <span>CPU · memory · in-flight work</span>
      </div>
    </div>
  </section>
</template>

<style scoped>
.MbxConcurrency {
  align-items: center;
  background:
    linear-gradient(135deg, rgb(var(--mbx-amber-rgb) / 0.1), transparent 48%),
    var(--vp-c-bg-soft);
  border: 1px solid var(--vp-c-divider);
  border-radius: 18px;
  display: grid;
  gap: 48px;
  margin: 64px auto 24px;
  max-width: 1152px;
  overflow: hidden;
  padding: 40px;
}

.eyebrow {
  color: var(--vp-c-brand-1);
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
}

.lede {
  color: var(--vp-c-text-2);
  font-size: 17px;
  line-height: 1.65;
  margin: 20px 0 28px;
  max-width: 42rem;
}

.proof {
  align-items: baseline;
  display: flex;
  gap: 14px;
}

.proof strong {
  color: var(--mbx-amber-bright);
  font-family: var(--mbx-display);
  font-size: 38px;
  letter-spacing: -0.04em;
  white-space: nowrap;
}

.proof span,
.validation {
  color: var(--vp-c-text-2);
  line-height: 1.45;
}

.validation {
  font-size: 13px;
  margin: 8px 0 24px;
}

.links {
  align-items: center;
  display: flex;
  flex-wrap: wrap;
  gap: 20px;
}

.links a {
  color: var(--vp-c-brand-1);
  font-weight: 600;
  text-decoration: none;
}

.links .primary {
  background: linear-gradient(
    135deg,
    var(--mbx-amber-bright),
    var(--mbx-amber-deep)
  );
  border-radius: 9px;
  color: var(--mbx-ink);
  padding: 10px 18px;
}

.diagram {
  display: grid;
  grid-template-columns: 1fr;
}

.job,
.pool {
  background: #100c07;
  border: 1px solid rgb(var(--mbx-amber-rgb) / 0.24);
  padding: 17px 18px;
}

.job:first-child {
  border-radius: 12px 12px 0 0;
}

.job:nth-child(2) {
  border-radius: 0 0 12px 12px;
  border-top: 0;
}

.label,
.pool span {
  color: var(--vp-c-text-3);
  display: block;
  font-family: var(--vp-font-family-mono);
  font-size: 11px;
  margin-bottom: 5px;
}

.job code {
  color: var(--mbx-paper);
  font-family: var(--vp-font-family-mono);
  font-size: 12px;
  white-space: nowrap;
}

.join {
  height: 56px;
  margin: 0 auto;
  position: relative;
  width: 64%;
}

.join::after,
.join span {
  background: rgb(var(--mbx-amber-rgb) / 0.5);
  content: "";
  position: absolute;
}

.join span {
  height: 1px;
  top: 17px;
  width: 50%;
}

.join span:first-child {
  left: 0;
  transform: rotate(16deg);
  transform-origin: right;
}

.join span:last-child {
  right: 0;
  transform: rotate(-16deg);
  transform-origin: left;
}

.join::after {
  bottom: 0;
  height: 29px;
  left: 50%;
  width: 1px;
}

.pool {
  border-color: rgb(var(--mbx-teal-rgb) / 0.6);
  border-radius: 12px;
  box-shadow: 0 18px 38px -24px rgb(var(--mbx-teal-rgb) / 0.7);
  text-align: center;
}

.pool .pool-title {
  color: var(--mbx-teal-light);
  font-family: var(--mbx-display);
  font-size: 18px;
  font-weight: 700;
  margin-bottom: 5px;
}

.pool span:last-child {
  margin: 0;
}

@media (min-width: 960px) {
  .MbxConcurrency {
    grid-template-columns: minmax(0, 1.25fr) minmax(340px, 0.75fr);
    padding: 56px;
  }
}

@media (max-width: 639px) {
  .MbxConcurrency {
    border-left: 0;
    border-radius: 0;
    border-right: 0;
    margin-top: 48px;
    padding: 36px 24px;
  }

  .proof {
    align-items: flex-start;
    flex-direction: column;
    gap: 2px;
  }

  .job code {
    font-size: 10px;
  }
}
</style>
