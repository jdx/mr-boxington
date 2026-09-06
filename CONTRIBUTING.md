# Contributing

Start with [Discussions](https://github.com/jdx/mr-boxington/discussions) for
questions and proposed changes. For a suspected vulnerability, use the private
reporting process in [SECURITY.md](SECURITY.md).

## Set up the repository

Install [mise](https://mise.jdx.dev), then run from the repository root:

```sh
git submodule update --init --recursive
mise install
mise exec -- rustup target add wasm32-unknown-unknown
mise run build
```

The submodules provide Bats and its assertion helpers. The WebAssembly target
is required by the end-to-end tests. `mise run build` first builds a bootstrap
mbx, then uses it to build the workspace.

## Make a change

Keep the change focused and explain the behavior it improves. Use conventional
commit subjects and pull request titles such as `docs: clarify cache setup` or
`fix: preserve target paths`. See [AGENTS.md](AGENTS.md) for allowed types and
repository rules.

Do not bump crate versions or edit changelogs in ordinary pull requests.
Release-plz generates both. Declare an API break with `!` in the title and
explain it in a `BREAKING CHANGE:` footer; [RELEASING.md](RELEASING.md) describes
the release process.

## Check your work

```sh
mise run format
mise run ci
```

`ci` runs formatting and Clippy checks, builds the workspace, verifies generated
docs and site links, and runs Rust and platform behavioral tests. For a focused
iteration, use `mise run test:cargo` or `mise run test:e2e`. See
[test/README.md](test/README.md) for individual Bats cases,
[benchmarks/README.md](benchmarks/README.md) for performance measurements, and
[fuzz/README.md](fuzz/README.md) for parser fuzzing.

## Work on documentation

The README introduces the project. `docs/` contains the VitePress website:
start at `docs/guide.md` for its reading paths and `docs/index.md` for the
landing page. Navigation is in `docs/.vitepress/config.mts`; components and
styles are in `docs/.vitepress/theme/`.

```sh
mise run docs          # generate reference pages and start VitePress
mise run check:docs    # verify generated pages match their declarations
mise run check:links   # build the site and check internal pages and anchors
```

`docs/cli/` is generated. Edit command help in `crates/mbx/src/cli/` or settings
in `crates/mbx/src/config.rs`, run `mise run render:docs`, and include the result
in the change. Do not hand-edit generated pages.

Lead guides with the task and a runnable example. Keep command reference,
conceptual detail, and troubleshooting easy to find without repeating them
across pages. Preserve published anchors when moving sections, and check the
site on a narrow screen as well as a desktop.

### Social previews

The documentation build generates a 1200×630 PNG for each page title using
`docs/.vitepress/social-images.mjs`, the project logo, and the bundled
[Space Grotesk font](docs/.vitepress/fonts/README.md). Rendering needs no remote
service or system fonts. Hashed image URLs refresh when titles or artwork
change. The build tests the renderer and verifies that every page references
an emitted image.
