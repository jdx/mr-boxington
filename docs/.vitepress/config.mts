import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { defineConfig } from "vitepress";
import { tabsMarkdownPlugin } from "vitepress-plugin-tabs";

const configDir = dirname(fileURLToPath(import.meta.url));
const cargoToml = readFileSync(resolve(configDir, "../../Cargo.toml"), "utf8");
const versionMatch = cargoToml.match(
  /^\[workspace\.package\][\s\S]*?^\s*version\s*=\s*"([^"]+)"/m,
);
const latestVersion = versionMatch?.[1] ?? "0.0.0";

export default defineConfig({
  title: "mr boxington",
  description: "A drop-in Cargo wrapper that shares compiled work and prunes target/ automatically",
  lang: "en-US",
  lastUpdated: true,
  appearance: "force-dark",
  cleanUrls: true,
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
      { text: "Get Started", link: "/getting-started" },
      { text: "Configuration", link: "/configuration" },
      { text: "GitHub Actions", link: "/github-actions" },
      { text: "CLI", link: "/cli/" },
      {
        text: `v${latestVersion}`,
        link: "https://github.com/jdx/mr-boxington/releases",
      },
    ],
    sidebar: [
      {
        text: "Getting started",
        items: [
          { text: "Introduction", link: "/" },
          { text: "Install and run", link: "/getting-started" },
        ],
      },
      {
        text: "Use mbx",
        items: [
          { text: "Configuration", link: "/configuration" },
          { text: "GitHub Actions", link: "/github-actions" },
          { text: "Remote cache", link: "/remote-cache" },
          { text: "Managed targets", link: "/managed-targets" },
        ],
      },
      {
        text: "Understand mbx",
        items: [
          { text: "How it works", link: "/how-it-works" },
          { text: "How mbx compares", link: "/compared" },
          { text: "Stability", link: "/stability" },
          { text: "Protocol compatibility", link: "/protocol-compatibility" },
          { text: "Cache results", link: "/cache-results" },
          { text: "Limits", link: "/limits" },
        ],
      },
      {
        text: "Reference",
        items: [
          { text: "CLI overview", link: "/cli/" },
          { text: "doctor", link: "/cli/doctor" },
          { text: "explain", link: "/cli/explain" },
          { text: "gc", link: "/cli/gc" },
          {
            text: "cache",
            link: "/cli/cache",
            collapsed: true,
            items: [
              { text: "dir", link: "/cli/cache/dir" },
              { text: "stats", link: "/cli/cache/stats" },
              { text: "projects", link: "/cli/cache/projects" },
              { text: "largest", link: "/cli/cache/largest" },
              { text: "verify", link: "/cli/cache/verify" },
              { text: "remove", link: "/cli/cache/remove" },
            ],
          },
          { text: "prefetch", link: "/cli/prefetch" },
          { text: "Settings", link: "/configuration#settings" },
        ],
      },
    ],
    outline: { level: [2, 3] },
    socialLinks: [
      { icon: "github", link: "https://github.com/jdx/mr-boxington" },
      { icon: "discord", link: "https://discord.gg/UBa7pJUN7Z" },
    ],
    editLink: {
      pattern: ({ filePath }) =>
        `https://github.com/jdx/mr-boxington/edit/main/docs/${filePath}`,
      text: "Edit this page on GitHub",
    },
    search: { provider: "local" },
    footer: false,
  },
  head: [
    ["link", { rel: "icon", href: "/favicon.svg", type: "image/svg+xml" }],
    ["link", { rel: "alternate icon", href: "/favicon.png", type: "image/png" }],
    ["link", { rel: "preconnect", href: "https://fonts.googleapis.com" }],
    [
      "link",
      { rel: "preconnect", href: "https://fonts.gstatic.com", crossorigin: "" },
    ],
    [
      "link",
      {
        rel: "stylesheet",
        href: "https://fonts.googleapis.com/css2?family=Space+Grotesk:wght@500;600;700&family=JetBrains+Mono:wght@400;600&display=swap",
      },
    ],
    ["meta", { name: "theme-color", content: "#d69b42" }],
    ["meta", { property: "og:type", content: "website" }],
    ["meta", { property: "og:site_name", content: "mr boxington" }],
    ["meta", { property: "og:title", content: "mr boxington" }],
    [
      "meta",
      { property: "og:description", content: "A drop-in Cargo wrapper that shares compiled work and prunes target/ automatically" },
    ],
    [
      "meta",
      {
        property: "og:image",
        content: "https://mr-boxington.jdx.dev/og.png",
      },
    ],
    ["meta", { property: "og:image:width", content: "1200" }],
    ["meta", { property: "og:image:height", content: "630" }],
    ["meta", { name: "twitter:card", content: "summary_large_image" }],
  ],
});
