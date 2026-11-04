#!/usr/bin/env node
// Lorevia's vector originals. Glyphs are outlined so rendering needs no fonts.
// Wordmark typeface: Geist, Copyright 2024 The Geist Project Authors (SIL OFL 1.1).
// Run npm --prefix web run build:logo to regenerate all brand assets.
import { readFile, writeFile } from "node:fs/promises";
import { createRequire } from "node:module";
import path from "node:path";
import { fileURLToPath } from "node:url";

const require = createRequire(import.meta.url);
const parseFont = require("next/dist/compiled/@next/font/dist/fontkit").default;
const font = parseFont(await readFile(require.resolve("next/dist/compiled/@vercel/og/Geist-Regular.ttf")));
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../assets/figs/logo");
const run = font.layout("Lorevia");
const tracking = -12;
const scale = 0.1;
const minX = run.glyphs[0].bbox.minX;
const width = Math.ceil((run.positions.reduce((sum, p) => sum + p.xAdvance + tracking, 0) - minX) * scale);
let cursor = -minX;
const letters = run.glyphs.map((glyph, index) => {
  const svg = `<path transform="translate(${cursor} 0)" d="${glyph.path.toSVG()}"/>`;
  cursor += run.positions[index].xAdvance + tracking;
  return svg;
}).join("");

function mark(ink, accent) {
  // An L that opens into a page: a steady foundation and room to explore.
  return `<path fill="${ink}" d="M30 18C24.5 18 20 22.5 20 28V85C20 99.4 28.6 108 43 108H100C105.5 108 110 103.5 110 98V88H46C42.7 88 40 85.3 40 82V28C40 22.5 35.5 18 30 18Z"/><path fill="${accent}" d="M58 70C58 43 77 23 108 18C107 46 88 66 58 70Z"/>`;
}
function word(ink) {
  return `<g transform="translate(2 76) scale(${scale} -${scale})" fill="${ink}" stroke="${ink}" stroke-width="12" stroke-linejoin="round">${letters}</g>`;
}
function svg(w, h, body) {
  return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${w} ${h}" width="${w}" height="${h}" role="img" aria-label="Lorevia">\n  ${body}\n</svg>\n`;
}
for (const [color, ink, accent] of [["color", "#3274e8", "#58b6a5"], ["black", "#111113", "#111113"]]) {
  await writeFile(path.join(root, `${color}-logo.svg`), svg(128, 128, mark(ink, accent)));
  await writeFile(path.join(root, `${color}-word.svg`), svg(width + 4, 88, word(ink)));
  await writeFile(path.join(root, `${color}-full.svg`), svg(width + 150, 128,
    `${mark(ink, accent)}<g transform="translate(146 20)">${word(ink)}</g>`));
}
console.log("Generated Lorevia vector mark, wordmark, and lockup (color + monochrome).");
