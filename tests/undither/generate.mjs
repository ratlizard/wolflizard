// Writes grimoire-reference.bin, the fixture the desktop presenter's undither
// test compares its Metal output with: a synthetic 8-bit frame and what
// grimoire's own undither() makes of it.
//
// grimoire (ratlizard/grimoire) is where the filter comes from, and its
// js/delv-graphics.js is the reference: the presenter runs the browser
// player's GPU version of it, which leaves out stray-colour repair and runs
// one smoothing pass, so the reference is undither() with `stray: 0` and
// `passes: 1`, in stored values (the presenter's `light` off), with the
// game's animated colours (indices 0xE0 to 0xFB) locked.
//
// The frame is made here, from formulas, and is nobody's artwork: a
// checkerboard of two close colours, an ordered dither of a ramp, a
// checkerboard between two ramps of different hue (the kind of dither
// grimoire's notes describe in Cythera's portraits), a random-threshold
// dither of a lit disc, drawn lines and a hard edge that must survive, and a
// checkerboard of an animated colour and an ordinary one.
//
//   node tests/undither/generate.mjs <grimoire checkout>
//
// File layout, little-endian: u32 width, u32 height, 256 u32 palette entries
// as 0xAARRGGBB, width*height index bytes, width*height*4 RGBA bytes of
// grimoire's output.

import { readFileSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const grimoire = process.argv[2];
if (!grimoire) {
  console.error('usage: node tests/undither/generate.mjs <grimoire checkout>');
  process.exit(2);
}
const source = readFileSync(join(grimoire, 'js', 'delv-graphics.js'), 'utf8');
const { undither, UD } = new Function(`${source}\n; return { undither, UD };`)();

const W = 96, H = 64;

// Three 32-entry ramps and a set of animated colours.
const palette = new Uint32Array(256);
const rgb = (r, g, b) => (0xff000000 | (r << 16) | (g << 8) | b) >>> 0;
for (let i = 0; i < 32; i++) {
  palette[i] = rgb(20 + i * 3, 30 + i * 5, 70 + i * 5);        // blue
  palette[32 + i] = rgb(i * 8, i * 8, i * 8);                   // grey
  palette[64 + i] = rgb(40 + i * 6, 25 + i * 4, 10 + i * 2);    // warm brown
}
for (let i = 0xe0; i < 0xfc; i++) palette[i] = rgb(200, 40 + (i - 0xe0) * 6, 60);
palette[0xff] = rgb(0, 0, 0);
palette[0xfe] = rgb(255, 255, 255);

const bayer = [0, 8, 2, 10, 12, 4, 14, 6, 3, 11, 1, 9, 15, 7, 13, 5];
// A fixed hash, so the random-threshold dither is the same on every run.
const hash = (x, y) => {
  let h = (x * 374761393 + y * 668265263) >>> 0;
  h = Math.imul(h ^ (h >>> 13), 1274126177) >>> 0;
  return ((h ^ (h >>> 16)) >>> 0) / 4294967296;
};

const indices = new Uint8Array(W * H);
for (let y = 0; y < H; y++) {
  for (let x = 0; x < W; x++) {
    const tx = x % 32, ty = y % 32, region = (y < 32 ? 0 : 3) + Math.floor(x / 32);
    let index;
    if (region === 0) {
      // A 50% checkerboard of two close blues.
      index = (x + y) % 2 ? 10 : 14;
    } else if (region === 1) {
      // An ordered dither of a grey ramp across the square.
      const v = 4 + tx * 0.5 + bayer[(y % 4) * 4 + (x % 4)] / 16;
      index = 32 + Math.min(31, Math.floor(v));
    } else if (region === 2) {
      // Grey and warm ramps interleaved, lightening down the square.
      const level = 6 + Math.floor(ty / 3);
      index = (x + y) % 2 ? 32 + level : 64 + level;
    } else if (region === 3) {
      // A flat field with a one-pixel line and a hard edge through it.
      index = ty < 20 ? 40 : 70;
      if (tx === 12 || ty === 8) index = 0xff;
      if (tx >= 22 && tx <= 23 && ty < 28) index = 0xfe;
    } else if (region === 4) {
      // A lit disc, random-threshold dithered onto the blue ramp.
      const dx = tx - 12, dy = ty - 12, r2 = (dx * dx + dy * dy) / 196;
      const v = r2 < 1 ? 6 + 22 * Math.sqrt(1 - r2) : 3;
      index = Math.min(31, Math.floor(v + hash(x, y)));
    } else {
      // An animated colour dithered with an ordinary one, and a flat
      // animated patch.
      index = (x + y) % 2 ? 0xe4 : 12;
      if (ty >= 20 && tx >= 20) index = 0xe8;
    }
    indices[y * W + x] = index;
  }
}

const raw = new Uint8ClampedArray(W * H * 4);
const locked = new Uint8Array(W * H);
for (let i = 0; i < W * H; i++) {
  const argb = palette[indices[i]];
  raw[i * 4] = (argb >>> 16) & 0xff;
  raw[i * 4 + 1] = (argb >>> 8) & 0xff;
  raw[i * 4 + 2] = argb & 0xff;
  raw[i * 4 + 3] = 255;
  locked[i] = indices[i] >= 0xe0 && indices[i] < 0xfc ? 1 : 0;
}

const P = Object.assign({}, UD, { stray: 0, passes: 1 });
const out = undither(new Uint8ClampedArray(raw), W, H, P, locked, null).out;

// The fixture has to be able to tell a filter that skipped a stage from one
// that ran it: say how far grimoire without its supersample is from itself.
const P0 = Object.assign({}, P, { upscale: 1, supersample: false });
const out0 = undither(new Uint8ClampedArray(raw), W, H, P0, locked, null).out;
let changed = 0, stageApart = 0;
for (let i = 0; i < W * H; i++) {
  let moved = false, apart = 0;
  for (let c = 0; c < 3; c++) {
    if (out[i * 4 + c] !== raw[i * 4 + c]) moved = true;
    apart = Math.max(apart, Math.abs(out[i * 4 + c] - out0[i * 4 + c]));
  }
  if (moved) changed++;
  if (apart > 2) stageApart++;
}

const header = new Uint32Array([W, H]);
const file = Buffer.concat([
  Buffer.from(header.buffer),
  Buffer.from(palette.buffer),
  Buffer.from(indices),
  Buffer.from(out.buffer, out.byteOffset, out.byteLength),
]);
const target = join(dirname(fileURLToPath(import.meta.url)), 'grimoire-reference.bin');
writeFileSync(target, file);
console.log(`${target}: ${W}x${H}, grimoire changes ${changed} pixels; ` +
  `without its supersample it is ${stageApart} pixels more than 2 away`);
