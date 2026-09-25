// Verify the built HTML references real, page-specific PNG previews, and that
// only the homepage offers the rendered showreel as og:video.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { existsSync, readFileSync, readdirSync } from "node:fs";
import { resolve, join, relative } from "node:path";

const root = resolve(process.argv[2] || ".vitepress/dist");
const metaTags = (html) =>
  [...html.matchAll(/<meta\b[^>]*>/g)].map(([tag]) =>
    Object.fromEntries(
      [...tag.matchAll(/([\w:-]+)=(?:"([^"]*)"|'([^']*)'|([^\s>]+))/g)].map(
        ([, name, quoted, single, bare]) => [name, quoted ?? single ?? bare],
      ),
    ),
  );
const meta = (html, key) => {
  const matches = metaTags(html).filter(
    (tag) => tag.property === key || tag.name === key,
  );
  assert.equal(matches.length, 1, `Expected one ${key} tag`);
  return matches[0].content;
};
// Present only when `mise run render:showreel` ran before the build.
const videoFile = join(root, "showreel.mp4");
const video = existsSync(videoFile) ? readFileSync(videoFile) : null;
const walk = (dir) =>
  readdirSync(dir, { withFileTypes: true }).flatMap((entry) =>
    entry.isDirectory() ? walk(join(dir, entry.name)) : [join(dir, entry.name)],
  );
let posts = 0;
const images = new Set();
for (const file of walk(root).filter((file) => file.endsWith(".html"))) {
  const html = readFileSync(file, "utf8");
  assert.equal(meta(html, "og:title"), meta(html, "twitter:title"));
  assert.equal(meta(html, "og:description"), meta(html, "twitter:description"));
  const image = meta(html, "og:image");
  assert.equal(meta(html, "twitter:image"), image);
  assert.equal(meta(html, "twitter:image:alt"), meta(html, "og:image:alt"));
  assert.match(image, /^https:\/\//);
  assert.notEqual(new URL(image).pathname, "/og.png");
  const png = readFileSync(join(root, new URL(image).pathname));
  assert.deepEqual([...png.subarray(0, 8)], [137, 80, 78, 71, 13, 10, 26, 10]);
  assert.equal(png.readUInt32BE(16), 1200);
  assert.equal(png.readUInt32BE(20), 630);
  images.add(image);

  const videoTags = metaTags(html).filter((tag) =>
    tag.property?.startsWith("og:video"),
  );
  if (video && relative(root, file) === "index.html") {
    assert.equal(meta(html, "og:type"), "video.other");
    const url = meta(html, "og:video");
    assert.equal(meta(html, "og:video:secure_url"), url);
    assert.equal(meta(html, "og:video:type"), "video/mp4");
    assert.equal(meta(html, "og:video:width"), "1280");
    assert.equal(meta(html, "og:video:height"), "720");
    assert.match(url, /^https:\/\//);
    assert.equal(new URL(url).pathname, "/showreel.mp4");
    // The version must change with the file, or previews keep a stale render.
    const version = createHash("sha256").update(video).digest("hex");
    assert.equal(new URL(url).searchParams.get("v"), version.slice(0, 12));
    assert.equal(video.toString("latin1", 4, 8), "ftyp", "showreel.mp4 is not an MP4");
  } else {
    assert.equal(meta(html, "og:type"), "website");
    assert.equal(videoTags.length, 0, `Unexpected og:video tags in ${file}`);
  }
  posts++;
}
assert.ok(posts > 0, "No built pages found");
assert.ok(images.size > 1, "Pages should have distinct images");
console.log(
  `Checked images and social metadata for ${posts} documentation pages.`,
);
