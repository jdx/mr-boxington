interface BannerData {
  id: string;
  enabled: boolean;
  message: string;
  link?: string;
  linkText?: string;
  expires?: string;
}

const dismissedKey = "jdx-banner-dismissed";

function isCurrent(banner: BannerData): boolean {
  if (!banner.enabled) return false;
  if (!banner.expires) return true;
  const expiration = Date.parse(banner.expires);
  return Number.isNaN(expiration) || Date.now() < expiration;
}

function isHttpUrl(value: string): boolean {
  try {
    const url = new URL(value);
    return url.protocol === "https:" || url.protocol === "http:";
  } catch {
    return false;
  }
}

export function initBanner(): void {
  if (typeof window === "undefined") return;

  window.addEventListener("DOMContentLoaded", async () => {
    const controller = new AbortController();
    const timeout = window.setTimeout(() => controller.abort(), 5000);
    try {
      const response = await fetch("https://jdx.dev/banner.json", {
        signal: controller.signal,
      });
      if (!response.ok) return;
      const banner = (await response.json()) as BannerData;
      if (!banner?.id || !isCurrent(banner)) return;
      if (localStorage.getItem(dismissedKey) === banner.id) return;

      const element = document.createElement("div");
      element.className = "jdx-banner";
      element.setAttribute("role", "region");
      element.setAttribute("aria-label", "Site announcement");

      const message = document.createElement("span");
      message.textContent = banner.message;
      element.appendChild(message);

      if (banner.link && isHttpUrl(banner.link)) {
        const link = document.createElement("a");
        link.href = banner.link;
        link.rel = "noopener";
        link.target = "_blank";
        link.textContent = banner.linkText || "Learn more";
        element.appendChild(link);
      }

      const dismiss = document.createElement("button");
      dismiss.type = "button";
      dismiss.setAttribute("aria-label", "Dismiss announcement");
      dismiss.textContent = "×";
      dismiss.addEventListener("click", () => {
        localStorage.setItem(dismissedKey, banner.id);
        element.remove();
        document.documentElement.style.removeProperty("--vp-layout-top-height");
      });
      element.appendChild(dismiss);
      document.body.prepend(element);

      const syncHeight = () => {
        document.documentElement.style.setProperty(
          "--vp-layout-top-height",
          `${element.offsetHeight}px`,
        );
      };
      new ResizeObserver(syncHeight).observe(element);
      syncHeight();
    } catch {
      // Announcements should never prevent the documentation from loading.
    } finally {
      window.clearTimeout(timeout);
    }
  });
}
