import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { defineConfig } from "vitepress";

const configDir = dirname(fileURLToPath(import.meta.url));
const cargoToml = readFileSync(resolve(configDir, "../../Cargo.toml"), "utf8");
const versionMatch = cargoToml.match(
  /^\[workspace\.package\][\s\S]*?^\s*version\s*=\s*"([^"]+)"/m,
);
const latestVersion = versionMatch?.[1] ?? "0.0.0";

export default defineConfig({
  title: "mr boxington",
  description: "A build cache for Rust projects",
  lang: "en-US",
  lastUpdated: true,
  appearance: "force-dark",
  cleanUrls: true,
  sitemap: {
    hostname: "https://mr-boxington.jdx.dev",
  },
  themeConfig: {
    logo: "/logo.svg",
    nav: [
      { text: "Get Started", link: "/getting-started" },
      { text: "Configuration", link: "/configuration" },
      { text: "GitHub Actions", link: "/github-actions" },
      { text: "CLI", link: "/cli" },
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
          { text: "Cache results", link: "/cache-results" },
          { text: "Limits", link: "/limits" },
        ],
      },
      {
        text: "Reference",
        items: [{ text: "CLI", link: "/cli" }],
      },
    ],
    outline: { level: [2, 3] },
    socialLinks: [
      { icon: "github", link: "https://github.com/jdx/mr-boxington" },
      { icon: "discord", link: "https://discord.gg/UBa7pJUN7Z" },
    ],
    editLink: {
      pattern: "https://github.com/jdx/mr-boxington/edit/main/docs/:path",
      text: "Edit this page on GitHub",
    },
    search: { provider: "local" },
    footer: false,
  },
  head: [
    ["link", { rel: "icon", href: "/favicon.svg", type: "image/svg+xml" }],
    ["link", { rel: "alternate icon", href: "/favicon.png", type: "image/png" }],
    ["meta", { name: "theme-color", content: "#d69b42" }],
    ["meta", { property: "og:type", content: "website" }],
    ["meta", { property: "og:site_name", content: "mr boxington" }],
    ["meta", { property: "og:title", content: "mr boxington" }],
    [
      "meta",
      { property: "og:description", content: "A build cache for Rust projects" },
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
