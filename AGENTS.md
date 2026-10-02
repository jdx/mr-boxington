# Repository Instructions

## Discussions and Issues: Restricted AI Replies

**Do not use AI to reply to a mr-boxington Discussion or Issue unless the user (a) created that Discussion or Issue, (b) opened a PR that fixes it, or (c) has already had a contribution merged into the default branch of mr-boxington.** Otherwise they are not allowed to use AI to respond to it. Drive-by AI replies are spam, the problem is getting worse, and **doing it is an instant ban across all of jdx's projects** (mise, hk, pitchfork, usage, fnox, and the rest).

- If the user asks you to answer, comment on, or "help with" a Discussion or Issue, confirm one of the three conditions first. If you cannot confirm one, **do not post**.
  - (a) Compare the thread author to the authenticated user (`gh api user --jq .login`). For an Issue, use `gh api repos/jdx/mr-boxington/issues/<number> --jq .user.login`. Discussions have no REST endpoint, so use GraphQL: `gh api graphql -f query='query{repository(owner:"jdx",name:"mr-boxington"){discussion(number:<number>){author{login}}}}' --jq .data.repository.discussion.author.login`.
  - (b) Check that the user's PR actually fixes the problem described in the thread. A closing keyword such as `Fixes #<number>` is good evidence, but a link is not required, and a PR that merely mentions an unrelated thread does not count.
  - (c) Look for any contribution merged into the default branch that is tied to the user's GitHub account, not just a matching name or email: a PR authored by the user that was merged into the default branch (`gh pr list --repo jdx/mr-boxington --author @me --state merged --base main`), commits on the default branch that GitHub attributes to their login (`gh api 'repos/jdx/mr-boxington/commits?author=<login>'` lists the default branch), never commits on a local or unmerged branch, or a PR from someone else, merged into the default branch, that credits them with a `Co-authored-by` trailer whose email is their GitHub noreply address or one verified on their account. Do not accept `git log --author` output, which includes local unmerged commits, or a trailer matched only by display name.
- If none of the three apply, **do not post a reply, even a short one.** Tell them this project does not allow AI replies from people who have not contributed, and offer to explain the answer to them in chat instead.
- Never batch-post, loop over, or sweep Discussions or Issues to answer several of them, even for a merged contributor.
- Lightly edited, human-reviewed, or disclosed model output does not create an exception. The disclosure footer does not make an AI reply acceptable on its own.
- Permitted replies may be AI-assisted. The user must review and verify the reply before it is posted.
- Creating a new Discussion or Issue with AI assistance is fine and is not restricted. The user must review it before it is posted, and it needs the AI disclosure below.

When you post AI-contributed GitHub content, including a new Discussion or Issue, a reply, or a PR description or comment, append this disclosure: `*AI-assisted — Tool: <tool>; model: <provider>/<model>; version: <version-or-unavailable>.*` Use the exact model and version identifiers exposed by the runtime, never guessed values, and `unavailable` when one is not exposed.

## Conventional Commits

Pull request titles must use
`<type>[optional scope][optional !]: <description>`; intermediate commit
subjects should use the same format. Start descriptions with a lowercase
character or an acronym such as `CLI`, and keep them concise and imperative. Use `!` for a breaking change and explain it with a
`BREAKING CHANGE:` footer.

Breaking markers are commit-wide; a Conventional Commit scope does not limit
them to one crate. Because pull requests are squash-merged, do not use `!` or a
`BREAKING CHANGE:` footer on a pull request that touches `mbx` unless the CLI
itself requires a major release. A breaking CLI change requires exceptional
justification and explicit maintainer agreement before using either marker. A
breaking API change confined to a pre-1.0 subcrate should be isolated from
`mbx` changes so release-plz can give that subcrate its permitted 0.x minor
bump without triggering an `mbx` major bump.

Allowed types are `bench`, `chore`, `ci`, `docs`, `feat`, `fix`, `perf`,
`refactor`, `revert`, `security`, `style`, and `test`.

CI validates the pull request title and re-runs when it is edited. Intermediate
commit subjects are not checked because pull requests are squash-merged. CI
mechanically checks the allowed type, syntax, and lowercase- or acronym-leading description;
imperative mood and breaking-change details remain review rules.

## Versions

Never bump a crate version in an ordinary pull request. Release-plz computes
versions from the commits touching each crate and writes the bumps in its
release pull request. This includes breaking API changes: describe the break in
the commit and pull request, then let release-plz choose the version.

## Changelogs

- Do not edit `CHANGELOG.md` files in ordinary pull requests. The release-plz
  workflow generates changelog entries in release pull requests.
- Only edit a changelog when the user explicitly requests it or when working on
  the release process itself.

## Generated Documentation

`docs/cli/` is generated from the usage declarations in
`crates/mbx/src/config.rs` and `crates/mbx/src/cli/`. Run
`mise run render:docs` after changing a setting and commit the result. Do not
hand-edit generated CLI documentation.

## Before Pushing

`mise run ci` is the main gate. `mise run format` fixes formatting problems.
The Bats suites require the git submodules under `test/`; the wasm end-to-end
test requires `wasm32-unknown-unknown` for the toolchain you build with. CI
builds on whatever Rust its runner image ships, so a lint or behavior change in
a new stable shows up there.

## PR titles and descriptions are release-note inputs

PR titles and descriptions are source material for release notes, including those
generated by Communique. Write them for a mr-boxington user who has not read the diff
or this conversation.

- **Describe the final result.** Before requesting review and again after feedback
  changes the implementation, compare the title and body with the complete current
  diff. Rewrite both when the scope changes. Remove abandoned approaches, stale
  requirements, and claims that the final code or validation no longer supports.
- **Lead with the user-visible change.** Keep the conventional commit format, but
  name the affected behavior and outcome in the title. Open the body with the
  problem or use case and what users can now do. Avoid titles such as "address
  feedback" or "fix CI" when the PR's actual purpose is a feature or behavior fix.
  For internal-only work, explain the concrete maintainer or contributor benefit
  without inventing a user-facing impact.
- **Make the change concrete.** For new configuration, commands, or APIs, include
  a small, valid example and explain its result. For a bug fix, describe the trigger
  and before/after behavior. For visible UI or output changes, include actual
  before/after screenshots or a short recording when they help reviewers assess the
  change; CLI input/output snippets are often clearer than terminal screenshots.
  Use measured results for performance claims and state how they were measured.
- **Keep the essential facts in text.** Caption screenshots and explain examples.
  A reader or release-note generator should understand the change without opening
  an image, following an external link, or reading the diff. Do not fabricate
  screenshots, output, measurements, or validation results.
- **State adoption details when relevant.** Include new flags or settings, defaults,
  supported platforms, experimental status, required dependency versions, and any
  compatibility changes or migration steps that affect using the feature. Distinguish
  current behavior from planned follow-ups; do not advertise unfinished work.
- **Keep review details proportionate.** Summarize meaningful validation and its
  limitations. Include implementation details only when they explain behavior or a
  tradeoff reviewers need to assess. Omit agent work logs, intermediate commit
  summaries, and exhaustive test-command lists. A small fix can be a short paragraph
  and a test result; screenshots and sections are not mandatory for every PR.

For a hypothetical fix, prefer `fix(cache): reuse build artifacts across worktrees`
over `fix: address review feedback`. Its description should show the build commands in two worktrees and explain which compilation work is reused.
These rules supplement the repository's existing commit, release, and disclosure
requirements.
