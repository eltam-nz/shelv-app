#!/usr/bin/env node
/**
 * Verifies every palette colour against WCAG 2.1 contrast minimums.
 *
 * Pastels on a dark background are easy to get wrong: they look fine to a
 * designer with a good monitor and fail for everyone else. This computes the
 * ratios rather than trusting the eye, and runs in CI so a colour cannot be
 * added or tweaked into being unreadable.
 *
 * Thresholds (WCAG 2.1):
 *   - 4.5:1 for body text (1.4.3)
 *   - 3.0:1 for a boundary that identifies a control or its state (1.4.11),
 *     which covers input borders and the focus ring
 *   - 1.3:1 for a purely decorative divider. 1.4.11 does not reach these —
 *     the table is delineated by spacing and content, not by its gridlines —
 *     but an invisible divider is still a bug, so there is a floor.
 *
 * Reads the tokens out of src/styles/theme.css, so the check and the
 * stylesheet cannot drift apart.
 *
 * Run: node scripts/check-contrast.mjs
 */

import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const css = readFileSync(join(root, "src/styles/theme.css"), "utf8");

/** Parses `--token: #rrggbb;` and `--token: rgb(r g b / a);` declarations. */
function parseTokens(source) {
  const tokens = new Map();
  const re = /--([\w-]+)\s*:\s*([^;]+);/g;
  let match;
  while ((match = re.exec(source)) !== null) {
    tokens.set(match[1], match[2].trim());
  }
  return tokens;
}

function parseHex(value) {
  const m = /^#([0-9a-f]{6})$/i.exec(value.trim());
  if (!m) return null;
  const n = parseInt(m[1], 16);
  return [(n >> 16) & 0xff, (n >> 8) & 0xff, n & 0xff];
}

/** Parses `rgb(r g b / a)` into a colour plus alpha. */
function parseRgba(value) {
  const m = /^rgb\(\s*(\d+)\s+(\d+)\s+(\d+)\s*\/\s*([\d.]+)\s*\)$/i.exec(value.trim());
  if (!m) return null;
  return { rgb: [Number(m[1]), Number(m[2]), Number(m[3])], alpha: Number(m[4]) };
}

/** Composites a translucent colour over an opaque one. */
function over(fg, alpha, bg) {
  return fg.map((c, i) => Math.round(c * alpha + bg[i] * (1 - alpha)));
}

/** WCAG relative luminance. */
function luminance([r, g, b]) {
  const channel = (v) => {
    const s = v / 255;
    return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
  };
  return 0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b);
}

function contrast(a, b) {
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return (hi + 0.05) / (lo + 0.05);
}

const tokens = parseTokens(css);

function colour(name) {
  const raw = tokens.get(name);
  if (raw === undefined) throw new Error(`theme.css has no --${name}`);
  const hex = parseHex(raw);
  if (hex) return hex;
  throw new Error(`--${name} is not a plain hex colour: ${raw}`);
}

function translucent(name) {
  const raw = tokens.get(name);
  if (raw === undefined) throw new Error(`theme.css has no --${name}`);
  const rgba = parseRgba(raw);
  if (rgba) return rgba;
  throw new Error(`--${name} is not an rgb(r g b / a) colour: ${raw}`);
}

const bg = colour("bg");
const surface = colour("surface");
const surfaceRaised = colour("surface-raised");

/** Accent names shared by tags and status colours. */
const ACCENTS = ["blue", "mint", "rose", "amber", "lilac", "teal", "peach", "sage"];

const checks = [];

// Body text on each background.
for (const [name, on] of [
  ["bg", bg],
  ["surface", surface],
  ["surface-raised", surfaceRaised],
]) {
  checks.push({ label: `fg on ${name}`, fg: colour("fg"), bg: on, min: 4.5 });
  checks.push({ label: `fg-muted on ${name}`, fg: colour("fg-muted"), bg: on, min: 4.5 });
}

// Dividers only need to be visible; control boundaries and focus must clear
// 3:1 because they identify a component and its state.
checks.push({
  label: "border (divider) on surface",
  fg: colour("border"),
  bg: surface,
  min: 1.3,
});
checks.push({
  label: "border-strong (control boundary) on surface",
  fg: colour("border-strong"),
  bg: surface,
  min: 3.0,
});
checks.push({
  label: "focus ring on surface",
  fg: colour("focus"),
  bg: surface,
  min: 3.0,
});
checks.push({ label: "focus ring on bg", fg: colour("focus"), bg, min: 3.0 });

// Each accent is used as chip text over a translucent chip fill of the same
// hue, which itself sits on a surface. Check the real composited pair, not
// the accent against the bare background — that would flatter it.
for (const name of ACCENTS) {
  const fg = colour(`accent-${name}`);
  const fill = translucent(`accent-${name}-fill`);
  const chipBg = over(fill.rgb, fill.alpha, surface);
  checks.push({ label: `accent-${name} text on its own chip`, fg, bg: chipBg, min: 4.5 });
  checks.push({ label: `accent-${name} text on surface`, fg, bg: surface, min: 4.5 });
}

let failed = 0;
const rows = [];
for (const check of checks) {
  const ratio = contrast(check.fg, check.bg);
  const pass = ratio >= check.min;
  if (!pass) failed += 1;
  rows.push(
    `${pass ? "  ok  " : " FAIL "} ${ratio.toFixed(2).padStart(5)}:1 ` +
      `(min ${check.min.toFixed(1)})  ${check.label}`,
  );
}

console.log(rows.join("\n"));

if (failed > 0) {
  console.error(
    `\n${String(failed)} of ${String(checks.length)} contrast checks failed. ` +
      `Lighten the accent or darken the surface in src/styles/theme.css.`,
  );
  process.exit(1);
}

console.log(`\nall ${String(checks.length)} contrast checks pass`);
