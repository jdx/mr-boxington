<template>
  <div class="MbxShowcase">
    <section class="card" aria-labelledby="mbx-targets-title">
      <div class="copy">
        <p class="eyebrow">Every worktree, one store</p>
        <h2 id="mbx-targets-title">
          Every worktree starts warm, and cleans up after itself.
        </h2>
        <p class="lede">
          Workspace, registry, toolchain, and sysroot paths become stable
          placeholders before they enter a key, so a crate compiled in one
          checkout is restored in the next — each keeping its own
          <code>target/</code>, with no shared Cargo lock to serialize them.
          mbx parks those directories under its cache root and collects one
          when its checkout is gone, when it sits unused for 30 days, or when
          they outgrow their budget.
        </p>
        <div class="proof" aria-label="Cache keys contain no absolute paths">
          <strong>No absolute paths</strong>
          <span>
            enter a cache key, so a second checkout restores instead of
            rebuilding
          </span>
        </div>
        <p class="validation">
          A benchmark builds one checkout and times another, and the run is
          thrown away unless that second checkout reports hits and restored
          files. Budgets scale with the disk — 10% for managed target
          directories, 5% for the store — and the directory you are working in
          is never the one collected to meet them.
        </p>
        <div class="links">
          <a class="primary" href="/how-it-works#portable-keys">
            See how the keys travel
          </a>
          <a href="/managed-targets#collection">What gets collected →</a>
        </div>
      </div>

      <div
        class="diagram"
        aria-label="Three checkouts sharing one content-addressed store"
      >
        <div class="job">
          <span class="label">~/src/app · compiled here</span>
          <code>target -> targets/v1/1f9c2ab7</code>
        </div>
        <div class="job">
          <span class="label">~/src/app-hotfix · starts warm</span>
          <code>target -> targets/v1/8a20d941</code>
        </div>
        <div class="job dim last">
          <span class="label">deleted checkout · collected</span>
          <code>targets/v1/3c5f0e2b</code>
        </div>
        <div class="join" aria-hidden="true">
          <span></span>
          <span></span>
        </div>
        <div class="pool">
          <span class="pool-title">one shared store</span>
          <span>compiled once · reflinked into each target/</span>
        </div>
      </div>
    </section>

    <section class="card reverse" aria-labelledby="mbx-concurrency-title">
      <div class="copy">
        <p class="eyebrow">Parallel CI, one machine</p>
        <h2 id="mbx-concurrency-title">
          Run multiple Cargo builds at the same time.
        </h2>
        <p class="lede">
          Start several Cargo commands at once — two lint configurations, say,
          or tests alongside clippy. Left alone, each one tries to fill the
          machine, and together they overload it. mbx gives them a single
          shared CPU and memory budget instead, so they run in parallel
          without oversubscribing the machine. We saw mise's lint job finish
          up to 45% sooner this way.
        </p>
        <div class="links">
          <a class="primary" href="/github-action#parallel-cargo-steps">
            Copy the workflow
          </a>
          <a href="/configuration#machine-wide-compile-scheduling">
            Tune the budget →
          </a>
        </div>
      </div>

      <div
        class="diagram"
        aria-label="Two Cargo lint builds sharing one CPU and memory budget"
      >
        <div class="job">
          <span class="label">default features</span>
          <code>mbx clippy</code>
        </div>
        <div class="job last">
          <span class="label">all features + targets</span>
          <code>mbx clippy --all-features --all-targets</code>
        </div>
        <div class="join" aria-hidden="true">
          <span></span>
          <span></span>
        </div>
        <div class="pool">
          <span class="pool-title">one shared budget</span>
          <span>CPU · memory</span>
        </div>
      </div>
    </section>

    <section class="card" aria-labelledby="mbx-remote-title">
      <div class="copy">
        <p class="eyebrow">Every runner, one cache</p>
        <h2 id="mbx-remote-title">CI runners and teammates start warm.</h2>
        <p class="lede">
          Point mbx at a cache server or any S3-compatible bucket and
          ephemeral runners download only the rustc actions their build
          needs. On GitHub Actions there is nothing to host: the default
          backend rides the Actions cache and warms fork pull requests from
          caches built on main.
        </p>
        <div class="proof" aria-label="Write policy">
          <strong>PRs never publish</strong>
          <span>pull requests restore and cannot write, forks included</span>
        </div>
        <p class="validation">
          Only protected-branch builds may write. The client enforces it and
          your server should too — defense in depth, not an access-control
          boundary.
        </p>
        <div class="links">
          <a class="primary" href="/github-action">Set up the GitHub Action</a>
          <a href="/remote-cache">Bring a server or a bucket →</a>
        </div>
      </div>

      <div
        class="diagram"
        aria-label="A CI build and a laptop sharing one remote cache"
      >
        <div class="job">
          <span class="label">CI on main · read-write</span>
          <code>mbx test --workspace</code>
        </div>
        <div class="job last">
          <span class="label">teammate's laptop · read-only</span>
          <code>mbx build</code>
        </div>
        <div class="join" aria-hidden="true">
          <span></span>
          <span></span>
        </div>
        <div class="pool">
          <span class="pool-title">one remote cache</span>
          <span>mbx cache server · any S3-compatible bucket</span>
        </div>
      </div>
    </section>
  </div>
</template>

<style scoped>
.MbxShowcase {
  display: grid;
  gap: 24px;
  margin: 64px auto 24px;
  max-width: 1152px;
}

.card {
  align-items: center;
  background:
    linear-gradient(135deg, rgb(var(--mbx-amber-rgb) / 0.1), transparent 48%),
    var(--vp-c-bg-soft);
  border: 1px solid var(--vp-c-divider);
  border-radius: 18px;
  display: grid;
  gap: 48px;
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

.lede code,
.validation code {
  font-family: var(--vp-font-family-mono);
  font-size: 0.9em;
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

.job.last {
  border-radius: 0 0 12px 12px;
}

.job + .job {
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

.job.dim code {
  color: var(--vp-c-text-3);
  text-decoration: line-through;
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
  .card {
    grid-template-columns: minmax(0, 1.25fr) minmax(340px, 0.75fr);
    padding: 56px;
  }

  .card.reverse {
    grid-template-columns: minmax(340px, 0.75fr) minmax(0, 1.25fr);
  }

  .card.reverse .copy {
    order: 2;
  }

  .card.reverse .diagram {
    order: 1;
  }
}

@media (max-width: 639px) {
  .card {
    border-left: 0;
    border-radius: 0;
    border-right: 0;
    padding: 36px 24px;
  }

  .MbxShowcase {
    margin-top: 48px;
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
