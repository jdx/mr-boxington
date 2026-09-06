<template>
  <div class="MbxShowcase">
    <section class="home-section practical" aria-labelledby="practical-title">
      <div class="section-heading">
        <p class="home-eyebrow">A little housekeeping goes a long way</p>
        <h2 id="practical-title">Room for the work<br />you are doing now.</h2>
      </div>
      <div class="feature-columns">
        <article>
          <span class="feature-number" aria-hidden="true">02</span>
          <h3>A cache that cleans up.</h3>
          <p>
            Managed targets go when their checkout does, or when age and size
            limits call for it. Preview the cleanup before running it yourself.
          </p>
          <code class="feature-command">mbx gc --dry-run</code>
          <a class="home-text-link" href="/managed-targets"
            >Understand the disk budget <span aria-hidden="true">→</span></a
          >
        </article>
        <article>
          <span class="feature-number" aria-hidden="true">03</span>
          <h3>Builds that share the machine.</h3>
          <p>
            Tests, Clippy, and other worktrees draw from one CPU and memory
            budget. Identical compilations already in flight can finish once and
            serve both builds.
          </p>
          <code class="feature-command"
            >CARGO_TARGET_DIR=target/clippy mbx clippy</code
          >
          <p class="feature-note">
            Give each command its own target directory.
          </p>
          <a class="home-text-link" href="/scheduling"
            >Set up parallel builds <span aria-hidden="true">→</span></a
          >
        </article>
        <article>
          <span class="feature-number" aria-hidden="true">04</span>
          <h3>Something to bring to CI.</h3>
          <p>
            Warm runners with GitHub Actions cache, a cache server, or an
            S3-compatible bucket. Keep remote writers limited to trusted builds.
          </p>
          <code class="feature-command">jdx/mr-boxington-action@v1</code>
          <a class="home-text-link" href="/github-action"
            >Add the GitHub Action <span aria-hidden="true">→</span></a
          >
        </article>
      </div>
    </section>

    <section class="home-section proof" aria-labelledby="proof-title">
      <div>
        <p class="home-eyebrow">Measured, with the context attached</p>
        <h2 id="proof-title">See what a warm cache changes.</h2>
        <p>
          Compare warm builds, the next commit, local edits, and parallel jobs.
          The benchmark pages include the workload, trial ranges, and
          limitations.
        </p>
      </div>
      <a class="proof-link" href="/benchmarks"
        >Explore the benchmarks <span aria-hidden="true">↗</span></a
      >
    </section>

    <section class="home-section reading" aria-labelledby="reading-title">
      <div class="section-heading">
        <p class="home-eyebrow">Open the manual</p>
        <h2 id="reading-title">Make it part of your day.</h2>
      </div>
      <div class="reading-links">
        <a href="/cookbook/local-development"
          ><span>At your desk</span
          ><strong>Editors, watchers &amp; worktrees</strong
          ><span aria-hidden="true">↗</span></a
        >
        <a href="/configuration"
          ><span>On your terms</span
          ><strong>Settings, budgets &amp; build policy</strong
          ><span aria-hidden="true">↗</span></a
        >
        <a href="/troubleshooting"
          ><span>When you need an answer</span
          ><strong>Understand a build result</strong
          ><span aria-hidden="true">↗</span></a
        >
      </div>
      <div class="home-closing">
        <span>Built on Cargo. Inspired by the caches that came before.</span>
        <a href="/acknowledgements"
          >Meet the influences <span aria-hidden="true">→</span></a
        >
        <a href="https://jdx.dev/posts/2026-09-05-introducing-mr-boxington/"
          >Read the announcement <span aria-hidden="true">↗</span></a
        >
      </div>
    </section>
  </div>
</template>

<style scoped>
.practical {
  border-top: 1px solid var(--vp-c-divider);
  padding-bottom: 72px;
  padding-top: 64px;
}
.section-heading {
  margin-bottom: 36px;
}
.feature-columns {
  display: grid;
  gap: 32px;
  grid-template-columns: repeat(3, minmax(0, 1fr));
}
article {
  border-top: 1px solid var(--vp-c-divider);
  display: flex;
  flex-direction: column;
  padding-top: 20px;
}
.feature-number {
  color: var(--vp-c-brand-1);
  font-family: var(--vp-font-family-mono);
  font-size: 12px;
  margin-bottom: 22px;
}
h3 {
  font-family: var(--mbx-display);
  font-size: 22px;
  letter-spacing: -0.025em;
  line-height: 1.3;
  margin-bottom: 14px;
}
article > p {
  color: var(--vp-c-text-2);
  font-size: 15px;
  line-height: 1.8;
}
.feature-command {
  color: var(--mbx-paper);
  display: block;
  font-family: var(--vp-font-family-mono);
  font-size: 11px;
  margin: 22px 0;
  overflow-wrap: anywhere;
}
article .feature-note {
  font-size: 12px;
  margin: -14px 0 20px;
}
article > a {
  margin-top: auto;
}
.proof {
  align-items: center;
  background: var(--vp-c-bg-soft);
  border: 1px solid var(--vp-c-divider);
  border-radius: 12px;
  display: grid;
  gap: 40px;
  grid-template-columns: minmax(0, 1fr) auto;
  padding: 36px 40px;
}
.proof h2 {
  font-size: 30px;
}
.proof p:not(.home-eyebrow) {
  color: var(--vp-c-text-2);
  font-size: 15px;
  line-height: 1.8;
  margin-top: 16px;
  max-width: 620px;
}
.proof-link {
  border-bottom: 1px solid var(--vp-c-brand-1);
  color: var(--vp-c-brand-1);
  font-size: 14px;
  padding: 12px 0;
}
.proof-link span {
  margin-left: 12px;
}
.reading {
  padding-bottom: 32px;
  padding-top: 80px;
}
.reading-links {
  display: grid;
  grid-template-columns: repeat(3, minmax(0, 1fr));
}
.reading-links a {
  border-bottom: 1px solid var(--vp-c-divider);
  border-top: 1px solid var(--vp-c-divider);
  display: grid;
  gap: 12px;
  grid-template-columns: 1fr auto;
  padding: 24px 20px;
  transition: background 0.2s;
}
.reading-links a + a {
  border-left: 1px solid var(--vp-c-divider);
}
.reading-links a:hover {
  background: var(--vp-c-brand-soft);
}
.reading-links a > span:first-child {
  color: var(--vp-c-text-2);
  font-size: 12px;
  grid-column: 1 / -1;
}
.reading-links strong {
  font-size: 15px;
  font-weight: 500;
}
.reading-links a > span:last-child {
  color: var(--vp-c-brand-1);
}
.home-closing {
  color: var(--vp-c-text-2);
  display: flex;
  flex-wrap: wrap;
  font-size: 12px;
  gap: 12px 24px;
  line-height: 1.8;
  padding: 32px 0;
}
.home-closing > span {
  flex-basis: 100%;
}
.home-closing a {
  color: var(--vp-c-brand-1);
}
.home-closing a:hover {
  text-decoration: underline;
}
@media (max-width: 800px) {
  .feature-columns,
  .reading-links {
    gap: 28px;
    grid-template-columns: 1fr;
  }
  .reading-links {
    gap: 0;
  }
  .reading-links a + a {
    border-left: 0;
    border-top: 0;
  }
  .proof {
    grid-template-columns: 1fr;
    gap: 20px;
    padding: 28px;
  }
  .proof-link {
    justify-self: start;
  }
}
</style>
