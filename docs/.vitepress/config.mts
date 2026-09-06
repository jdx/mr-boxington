import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { defineConfig } from "vitepress";
import { tabsMarkdownPlugin } from "vitepress-plugin-tabs";
import { socialCard, writeSocialCard } from "./social-images.mjs";

const configDir = dirname(fileURLToPath(import.meta.url));
const cargoToml = readFileSync(
  resolve(configDir, "../../crates/mbx/Cargo.toml"),
  "utf8",
);
const versionMatch = cargoToml.match(
  /^\[package\][\s\S]*?^\s*version\s*=\s*"([^"]+)"/m,
);
if (!versionMatch) {
  throw new Error("could not read the mbx version from crates/mbx/Cargo.toml");
}
const latestVersion = versionMatch[1];
const siteUrl = "https://mr-boxington.jdx.dev";

export default defineConfig({
  title: "mr boxington",
  description:
    "Reuse Cargo compiler work across worktrees and CI, manage build storage, and run parallel builds with a shared CPU and memory budget.",
  lang: "en-US",
  lastUpdated: true,
  appearance: "force-dark",
  cleanUrls: true,
  // The configuration reference is embedded into /configuration with an
  // @include rather than shipped as its own page.
  srcExclude: ["cli/configuration.md"],
  rewrites: {
    "cli/cache.md": "cli/cache/index.md",
  },
  sitemap: {
    hostname: "https://mr-boxington.jdx.dev",
  },
  markdown: {
    config(md) {
      md.use(tabsMarkdownPlugin);
    },
  },
  themeConfig: {
    logo: "/logo.svg",
    nav: [
      {
        text: "Docs",
        link: "/guide",
        activeMatch:
          "^/(guide|getting-started|installation|setup|cookbook/local-development|scheduling|incremental|linkers|managed-targets|standalone-builds|tui|stats|troubleshooting|cache-results|how-it-works|limits|compared|faq|acknowledgements)",
      },
      {
        text: "CI & sharing",
        link: "/github-action",
        activeMatch:
          "^/(github-action|remote-cache|cache-server|cookbook/(fork-prs|migrate))",
      },
      { text: "Benchmarks", link: "/benchmarks" },
      {
        text: "Reference",
        link: "/cli/",
        activeMatch: "^/(cli|configuration|protocol-compatibility|stability)",
      },
      {
        text: `v${latestVersion}`,
        link: "https://github.com/jdx/mr-boxington/releases",
      },
    ],
    sidebar: [
      {
        text: "Start here",
        items: [
          { text: "Documentation", link: "/guide" },
          { text: "Get started", link: "/getting-started" },
          { text: "Installation", link: "/installation" },
          { text: "Cargo & editor setup", link: "/setup" },
        ],
      },
      {
        text: "Build locally",
        items: [
          { text: "Local development", link: "/cookbook/local-development" },
          { text: "Managed targets", link: "/managed-targets" },
          { text: "Parallel builds", link: "/scheduling" },
          { text: "Incremental builds", link: "/incremental" },
          { text: "Managed linkers", link: "/linkers" },
          { text: "Watching builds", link: "/tui" },
          { text: "Standalone C and C++", link: "/standalone-builds" },
        ],
      },
      {
        text: "CI & sharing",
        items: [
          { text: "GitHub Action", link: "/github-action" },
          { text: "Migrate an existing cache", link: "/cookbook/migrate" },
          { text: "Fork pull requests", link: "/cookbook/fork-prs" },
          { text: "Remote cache", link: "/remote-cache" },
          { text: "Cache server", link: "/cache-server" },
        ],
      },
      {
        text: "Understand & troubleshoot",
        items: [
          { text: "Troubleshooting", link: "/troubleshooting" },
          { text: "Cache results", link: "/cache-results" },
          { text: "Savings and statistics", link: "/stats" },
          { text: "How it works", link: "/how-it-works" },
          { text: "Caching limits", link: "/limits" },
          { text: "How mbx compares", link: "/compared" },
          { text: "Benchmarks", link: "/benchmarks" },
          { text: "FAQ", link: "/faq" },
        ],
      },
      {
        text: "Reference",
        items: [
          { text: "Configuration", link: "/configuration" },
          {
            text: "CLI commands",
            link: "/cli/",
            collapsed: true,
            items: [
              { text: "setup", link: "/cli/setup" },
              { text: "completion", link: "/cli/completion" },
              { text: "doctor", link: "/cli/doctor" },
              { text: "explain", link: "/cli/explain" },
              { text: "clean", link: "/cli/clean" },
              { text: "gc", link: "/cli/gc" },
              { text: "tui", link: "/cli/tui" },
              { text: "stats", link: "/cli/stats" },
              { text: "prefetch", link: "/cli/prefetch" },
              { text: "exec", link: "/cli/exec" },
              {
                text: "cache",
                link: "/cli/cache/",
                collapsed: true,
                items: [
                  { text: "dir", link: "/cli/cache/dir" },
                  { text: "stats", link: "/cli/cache/stats" },
                  { text: "projects", link: "/cli/cache/projects" },
                  { text: "largest", link: "/cli/cache/largest" },
                  { text: "verify", link: "/cli/cache/verify" },
                  { text: "trace", link: "/cli/cache/trace" },
                  { text: "export", link: "/cli/cache/export" },
                  { text: "import", link: "/cli/cache/import" },
                  { text: "remove", link: "/cli/cache/remove" },
                ],
              },
            ],
          },
          { text: "Stability", link: "/stability" },
          { text: "Protocol compatibility", link: "/protocol-compatibility" },
          { text: "Acknowledgements", link: "/acknowledgements" },
        ],
      },
    ],
    outline: { level: [2, 3] },
    socialLinks: [
      { icon: "github", link: "https://github.com/jdx/mr-boxington" },
      { icon: "discord", link: "https://discord.gg/UBa7pJUN7Z" },
    ],
    editLink: {
      pattern: ({ filePath }) => {
        const command = filePath.split("/")[1]?.replace(/\.md$/, "");
        const module =
          command === "index" || command === "completion" ? "mod" : command;
        const source = filePath.startsWith("cli/")
          ? `crates/mbx/src/cli/${module}.rs`
          : `docs/${filePath}`;
        return `https://github.com/jdx/mr-boxington/edit/main/${source}`;
      },
      text: "Edit this page on GitHub",
    },
    search: { provider: "local" },
    footer: false,
  },
  head: [
    [
      "script",
      {
        async: "",
        src: "https://www.googletagmanager.com/gtag/js?id=G-0MDX8ZJYFY",
      },
    ],
    [
      "script",
      {},
      `window.dataLayer = window.dataLayer || [];
function gtag(){dataLayer.push(arguments);}
gtag('js', new Date());
gtag('config', 'G-0MDX8ZJYFY');`,
    ],
    ["link", { rel: "icon", href: "/favicon.svg", type: "image/svg+xml" }],
    [
      "link",
      { rel: "icon", href: "/favicon.png", type: "image/png", sizes: "64x64" },
    ],
    ["link", { rel: "apple-touch-icon", href: "/favicon.png" }],
    ["link", { rel: "manifest", href: "/site.webmanifest" }],
    ["meta", { name: "theme-color", content: "#191713" }],
    ["meta", { property: "og:type", content: "website" }],
    ["meta", { property: "og:site_name", content: "mr boxington" }],
    ["meta", { property: "og:locale", content: "en_US" }],
    ["meta", { property: "og:image:width", content: "1200" }],
    ["meta", { property: "og:image:height", content: "630" }],
    ["meta", { name: "twitter:card", content: "summary_large_image" }],
    ["meta", { name: "twitter:site", content: "@jdxcode" }],
  ],
  transformHead({ pageData, title, description, siteConfig }) {
    const heading = pageData.title || "mr boxington";
    const card = socialCard(heading);
    writeSocialCard(siteConfig.outDir, card);
    const image = new URL(card.path, `${siteUrl}/`).toString();
    const imageAlt = `${heading} — mr boxington docs`;
    const url = new URL(
      pageData.relativePath.replace(/index\.md$/, "").replace(/\.md$/, ""),
      `${siteUrl}/`,
    ).toString();

    return [
      ["link", { rel: "canonical", href: url }],
      ["meta", { property: "og:url", content: url }],
      ["meta", { property: "og:image", content: image }],
      ["meta", { property: "og:image:alt", content: imageAlt }],
      ["meta", { name: "twitter:image", content: image }],
      ["meta", { name: "twitter:image:alt", content: imageAlt }],
      ["meta", { property: "og:title", content: title }],
      ["meta", { property: "og:description", content: description }],
      ["meta", { name: "twitter:title", content: title }],
      ["meta", { name: "twitter:description", content: description }],
      [
        "script",
        { type: "application/ld+json" },
        JSON.stringify({
          "@context": "https://schema.org",
          "@type": "WebPage",
          name: title,
          description,
          url,
          isPartOf: { "@type": "WebSite", name: "mr boxington", url: siteUrl },
        }),
      ],
    ];
  },
});
