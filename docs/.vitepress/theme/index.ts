import DefaultTheme from "vitepress/theme";
import type { Theme } from "vitepress";
import { h } from "vue";
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
  enhanceApp() {
    initBanner();
  },
} satisfies Theme;
