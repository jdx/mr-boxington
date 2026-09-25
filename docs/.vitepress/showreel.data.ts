import { createHash } from "node:crypto";
import { existsSync, readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const configDir = dirname(fileURLToPath(import.meta.url));
const videoPath = resolve(configDir, "../public/showreel.mp4");

export interface ShowreelFiles {
  /** Site-relative URL of the rendered MP4. */
  src: string;
  /** Site-relative URL of its poster frame. */
  poster: string;
}

/**
 * The showreel rendered by `mise run render:showreel`, which the docs deploy
 * runs before building, or null when this build has no render. The landing
 * page plays it and the homepage offers it as og:video.
 */
export function showreelFiles(): ShowreelFiles | null {
  if (!existsSync(videoPath)) return null;
  // Versioned so browsers and link previews that cached an earlier render
  // fetch the new one. The poster is rendered with the video.
  const version = createHash("sha256")
    .update(readFileSync(videoPath))
    .digest("hex")
    .slice(0, 12);
  return {
    src: `/showreel.mp4?v=${version}`,
    poster: `/showreel-poster.jpg?v=${version}`,
  };
}

export declare const data: ShowreelFiles | null;

export default {
  // Pick up a render made while the dev server is running.
  watch: [videoPath],
  load: showreelFiles,
};
