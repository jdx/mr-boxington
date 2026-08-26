import DefaultTheme from "vitepress/theme";
import type { Theme } from "vitepress";
import { enhanceAppWithTabs } from "vitepress-plugin-tabs/client";
import { h, onMounted, onUnmounted } from "vue";
import { data as starsData } from "../stars.data";
import EndevFooter from "./EndevFooter.vue";
import EndevSponsors from "./EndevSponsors.vue";
import HomeTerminal from "./HomeTerminal.vue";
import { initBanner } from "./banner";
import "./custom.css";

export default {
  extends: DefaultTheme,
  Layout() {
    return h(DefaultTheme.Layout, null, {
      "home-hero-after": () => h(HomeTerminal),
      "layout-bottom": () => [h(EndevSponsors), h(EndevFooter)],
    });
  },
  enhanceApp({ app }) {
    enhanceAppWithTabs(app);
    initBanner();
  },
  setup() {
    let observer: MutationObserver | undefined;
    onMounted(() => {
      const addStarCount = () => {
        if (!starsData.stars) return false;

        const githubLinks = document.querySelectorAll(
          '.VPSocialLinks a[href*="github.com/jdx/mr-boxington"]',
        );
        githubLinks.forEach((githubLink) => {
          if (!githubLink.querySelector(".star-count")) {
            const starBadge = document.createElement("span");
            starBadge.className = "star-count";
            starBadge.title = "GitHub Stars";
            const glyph = document.createElement("span");
            glyph.className = "star-glyph";
            glyph.textContent = "★";
            glyph.setAttribute("aria-hidden", "true");
            starBadge.append(glyph, starsData.stars);
            githubLink.appendChild(starBadge);
          }
        });
        return (
          githubLinks.length > 0 &&
          Array.from(githubLinks).every((link) =>
            link.querySelector(".star-count"),
          )
        );
      };

      addStarCount();
      observer = new MutationObserver(() => {
        addStarCount();
      });
      observer.observe(document.querySelector(".VPNav") || document.body, {
        childList: true,
        subtree: true,
      });
    });
    onUnmounted(() => observer?.disconnect());
  },
} satisfies Theme;
