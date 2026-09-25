// What the landing page says each chapter of the reel shows, for readers who
// cannot watch it: HomeShowreel.vue lists these under the player. The
// figures come from the facts and helpers the scenes draw with, and fall back
// with them, so the page never states a number the video does not show
// (test/describe.test.ts holds them to that).

import { delta, type ReelFacts, tenths } from "./facts";
import { SECTIONS, type SectionId } from "./timeline";

const secs = (n: number) => `${tenths(n)} seconds`;

/** What each chapter shows, in the reel's order, with the numbers it draws. */
export function describeChapters(f: ReelFacts | null): { id: SectionId; label: string; text: string }[] {
  const bench = f?.subject ? `the ${f.subject} benchmark` : "the benchmark";
  const warm = f?.warm;
  const commit = f?.commit;
  const peaks = f?.contention;
  const saved = commit && delta(commit);
  const text: Record<SectionId, string> = {
    fold: "Two pens draw a flat cardboard net. It folds up into a box, the lid slams shut, and a strip of tape runs down the front.",
    "mr-boxington":
      "The box leaps, lands, and comes to life as Mr Boxington: a skeptical eye, a monocle on a brass chain, a handlebar mustache, and rosy cheeks. His name card reads mr boxington, a shared cache for Cargo builds.",
    what: "The camera dives into his monocle, and the line mbx reuses Cargo compiler work across projects, worktrees, and CI slams into place.",
    "every-checkout":
      "Without a shared cache, a project, a worktree, and a CI runner each run cargo build and compile the same crates again, and old target/ directories pile up.",
    "under-cargo-build":
      "You keep typing cargo build. Cargo plans the build, and mbx wraps each rustc call it makes, with no daemon to manage.",
    "first-build":
      "The first build compiles as usual, and mbx stores each output under a key of its inputs, with the checkout's paths replaced by placeholders. Nothing was cached yet, so Mr Boxington finishes taped, without a strawberry.",
    "same-checkout": warm
      ? `Measured in ${bench}: the same checkout with an empty target/ restores ${warm.hits} of ${warm.lookups} compilations from the store in ${secs(warm.seconds)}. Mr Boxington blushes, is taped, and gets a strawberry.`
      : `Measured in ${bench}: the same checkout with an empty target/ restores its compilations from the store instead of recompiling them. Mr Boxington blushes, is taped, and gets a strawberry.`,
    "another-worktree":
      "A new worktree at another path computes the same keys, so its matching crates are restored and only the edited crate compiles.",
    "six-builds":
      "Six builds running at once share one pool of compiler permits. Cache hits never wait, and a compilation two of the builds need runs once." +
      (peaks
        ? ` In ${bench}, six CI jobs on one runner peaked at ${peaks.scheduled} compilers running at once with the scheduler on, against ${peaks.unscheduled} with it off.`
        : ""),
    ci: "In CI, pushes to main publish compiled outputs to a remote cache, such as the GitHub Actions cache, a cache server, or an S3 bucket, and pull requests only restore from it.",
    "next-push": commit
      ? `A bar chart of ${bench}'s next push in CI, with the cache holding the previous commit: Cargo alone takes ${secs(commit.cargo)} and Cargo with mbx takes ${secs(commit.mbx)}${saved ? `, ${secs(saved)} less` : ""}, with ${commit.hits} of ${commit.lookups} compilations restored${commit.misses > 0 ? ` and ${commit.misses} compiled` : ""}.`
      : "CI builds the next push, with the cache holding the previous commit. The measured times are on the benchmarks page, mr-boxington.jdx.dev/benchmarks.",
    pruned:
      "target/ lives in the cache. Cartons of old target directories pile up, and after a build mbx removes those whose checkout is gone or that went unused for 30 days, then the least recently used while the pile is over its disk budget.",
    morph: "The cartons that were kept melt together into one liquid shape, which sets into Mr Boxington's outline.",
    end: "Mr Boxington returns in full with a strawberry beside his tape, above the name mr boxington, the line A shared cache for Cargo builds, the command cargo install mbx --locked && mbx setup, and the address mr-boxington.jdx.dev.",
  };
  return SECTIONS.map(({ id, label }) => ({ id, label, text: text[id] }));
}
