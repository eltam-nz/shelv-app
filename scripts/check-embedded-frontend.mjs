#!/usr/bin/env node
/**
 * Asserts that a built Shelv binary carries the frontend inside it.
 *
 * A plain `cargo build --release` leaves tauri's `custom-protocol` feature
 * off. Tauri's own build script reads that as "this is a dev build", so the
 * binary embeds nothing and points the webview at `devUrl` instead. It links,
 * runs, and opens a window reading "localhost refused to connect" on every
 * machine without a dev server attached — which is every machine a release
 * artifact is downloaded onto. Only the Tauri CLI turns the feature on.
 *
 * Nothing about that failure is visible at build time, so it is checked here:
 * a production build contains the hashed asset names from dist/index.html,
 * and a dev build contains none of them.
 */

import { readFileSync } from "node:fs";

const binaryPath = process.argv[2];
if (binaryPath === undefined) {
  console.error("usage: check-embedded-frontend.mjs <path-to-binary>");
  process.exit(2);
}

const html = readFileSync("dist/index.html", "utf8");
// Vite emits content-hashed names, so these are specific to this build and
// cannot match by coincidence.
const assets = [...html.matchAll(/\/assets\/([A-Za-z0-9._-]+)/g)].map((m) => m[1]);

if (assets.length === 0) {
  console.error(
    "dist/index.html references no hashed assets, so this check would pass\n" +
      "vacuously. Run `pnpm build` first, or fix this script if the bundler\n" +
      "output format changed.",
  );
  process.exit(2);
}

// latin1 maps every byte to one character, so a substring search over the
// binary is exact and never mangles a match at a chunk boundary.
const binary = readFileSync(binaryPath, "latin1");
const missing = assets.filter((asset) => !binary.includes(asset));

if (missing.length > 0) {
  console.error(
    `${binaryPath} does not embed the frontend.\n\n` +
      `Missing: ${missing.join(", ")}\n\n` +
      'This binary will show "localhost refused to connect" when run.\n' +
      "Build it with `pnpm tauri build --no-bundle`, not `cargo build`.",
  );
  process.exit(1);
}

console.log(
  `${binaryPath} embeds the frontend (${String(assets.length)} assets checked).`,
);
