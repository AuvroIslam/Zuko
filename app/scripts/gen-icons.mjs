// Draws Zuko's head into the PNG/ICO set Tauri needs: the glossy cream sphere,
// two glowing amber almond eyes, the scar round his left eye (the viewer's
// right), and the black topknot in its red hair-tie, all inside a dark outline
// so it reads on light and dark taskbars alike. Small sizes drop detail and
// chunk up the hair and eyes. No dependencies: the icons are rasterised here
// and encoded with node:zlib, so the app icon stays "drawn in code" like the
// character itself (src/character/engine.ts).
//
//   node scripts/gen-icons.mjs

import { deflateSync } from "node:zlib";
import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const OUT = join(dirname(fileURLToPath(import.meta.url)), "..", "src-tauri", "icons");

// ── Palette (the concept sheet's) ─────────────────────────────────────────────

const hex = (h) => [parseInt(h.slice(1, 3), 16), parseInt(h.slice(3, 5), 16), parseInt(h.slice(5, 7), 16)];
const OUTLINE = hex("#1E1310");
const WHITE = hex("#FFFFFF");
const CREAM = hex("#F7F3EE");
const BEIGE = hex("#E8D5C4");
const SHADE = hex("#CBAE96");
const ORANGE = hex("#F28A1E");
const EYE_CORE = hex("#FFF3CF");
const EYE_MID = hex("#FFB347");
const EYE_EDGE = hex("#B5400E");
const SCAR = hex("#A83226");
const SCAR_EDGE = hex("#7A2016");
const HAIR = hex("#33221E");
const HAIR_RIM = hex("#8A5A45");
const TIE_DARK = hex("#6E1F18");
const TIE = hex("#B33A2E");
const TIE_LIGHT = hex("#D4523F");
const GOLD = hex("#E0A030");

const SS = 4; // supersampling factor

const lerp = (a, b, t) => a + (b - a) * t;
const clamp = (v, a, b) => Math.max(a, Math.min(b, v));
const mix = (a, b, t) => [0, 1, 2].map((i) => lerp(a[i], b[i], clamp(t, 0, 1)));
const smooth = (a, b, v) => {
  const k = clamp((v - a) / (b - a), 0, 1);
  return k * k * (3 - 2 * k);
};

// ── Geometry helpers ──────────────────────────────────────────────────────────

const cubic = (p0, p1, p2, p3, n, out) => {
  for (let i = 1; i <= n; i++) {
    const t = i / n;
    const u = 1 - t;
    out.push([
      u * u * u * p0[0] + 3 * u * u * t * p1[0] + 3 * u * t * t * p2[0] + t * t * t * p3[0],
      u * u * u * p0[1] + 3 * u * u * t * p1[1] + 3 * u * t * t * p2[1] + t * t * t * p3[1],
    ]);
  }
};

const cubicAt = (p, s) => {
  const u = 1 - s;
  const a = u * u * u, b = 3 * u * u * s, c = 3 * u * s * s, d = s * s * s;
  return [a * p[0] + b * p[2] + c * p[4] + d * p[6], a * p[1] + b * p[3] + c * p[5] + d * p[7]];
};

/** A tapered lock of hair along a cubic centre line (same profile as the engine). */
function lockPoly(p, w, qx, qy, r, n = 24) {
  const left = [];
  const right = [];
  let tip = [0, 0];
  for (let i = 0; i <= n; i++) {
    const s = i / n;
    const [lx, ly] = cubicAt(p, s);
    const [ax, ay] = cubicAt(p, Math.min(1, s + 0.02));
    const [bx, by] = cubicAt(p, Math.max(0, s - 0.02));
    let tx = ax - bx, ty = ay - by;
    const l = Math.hypot(tx, ty) || 1;
    tx /= l;
    ty /= l;
    const ww = w * r * (0.55 + 0.75 * Math.sin(Math.PI * s * 0.85)) * (1 - s * s * s);
    const X = qx + lx * r;
    const Y = qy + ly * r;
    left.push([X - ty * ww, Y + tx * ww]);
    right.push([X + ty * ww, Y - tx * ww]);
    tip = [X, Y];
  }
  return [...left, tip, ...right.reverse()];
}

/** The almond eye (right eye; mirrored for the left): an oval with its top cut along a chord. */
function almondPoly(cx, cy, w, h, cut, sd) {
  const a0 = (cut[1] * Math.PI) / 180;
  const a1 = (cut[0] * Math.PI) / 180;
  const p = [];
  const N = 40;
  for (let i = 0; i <= N; i++) {
    const a = a0 + ((a1 - a0) * i) / N;
    p.push([cx + sd * Math.cos(a) * (w / 2), cy + Math.sin(a) * (h / 2)]);
  }
  return p;
}

/** The scar outline (engine.ts SCAR), in head radii round its centre. */
function scarPoly(cx, cy, s) {
  const p = [[-0.2, 0.2]];
  const segs = [
    [[-0.32, 0.05], [-0.3, -0.2], [-0.12, -0.28]],
    [[-0.05, -0.31], [0.0, -0.29], [0.04, -0.37]],
    [[0.08, -0.3], [0.13, -0.29], [0.2, -0.36]],
    [[0.22, -0.28], [0.28, -0.24], [0.31, -0.18]],
    [[0.38, -0.02], [0.33, 0.16], [0.18, 0.24]],
    [[0.06, 0.3], [-0.1, 0.29], [-0.2, 0.2]],
  ];
  let last = p[0];
  for (const [c1, c2, e] of segs) {
    cubic(last, c1, c2, e, 10, p);
    last = e;
  }
  p.pop();
  return p.map(([x, y]) => [cx + x * s, cy + y * s]);
}

/** Pushes every vertex `d` along its outward normal (polygons are smooth enough for this). */
function offsetPoly(poly, d) {
  const n = poly.length;
  let area = 0;
  for (let i = 0; i < n; i++) {
    const [x0, y0] = poly[i];
    const [x1, y1] = poly[(i + 1) % n];
    area += x0 * y1 - x1 * y0;
  }
  const s = area > 0 ? 1 : -1;
  return poly.map((p, i) => {
    const a = poly[(i - 1 + n) % n];
    const b = poly[(i + 1) % n];
    let nx = (b[1] - a[1]) * s;
    let ny = -(b[0] - a[0]) * s;
    const l = Math.hypot(nx, ny) || 1;
    nx /= l;
    ny /= l;
    return [p[0] + nx * d, p[1] + ny * d];
  });
}

/** Even-odd inside test with crossings precomputed per subsample row. */
function rasterPoly(polys, size, ox, oy) {
  const rows = new Array(size * SS);
  for (let r = 0; r < size * SS; r++) {
    const y = (r + 0.5) / SS - oy;
    const xs = [];
    for (const poly of polys) {
      for (let i = 0; i < poly.length; i++) {
        const [x0, y0] = poly[i];
        const [x1, y1] = poly[(i + 1) % poly.length];
        if ((y0 <= y && y1 > y) || (y1 <= y && y0 > y)) xs.push(x0 + ((y - y0) / (y1 - y0)) * (x1 - x0) + ox);
      }
    }
    rows[r] = xs.sort((a, b) => a - b);
  }
  return (sx, sy) => {
    const xs = rows[sy];
    const x = (sx + 0.5) / SS;
    let c = 0;
    for (const v of xs) if (v < x) c++;
    return (c & 1) === 1;
  };
}

/** Union test: one rasteriser per polygon (overlapping polygons would cancel under even-odd). */
function rasterUnion(polys, size, ox, oy) {
  const testers = polys.map((p) => rasterPoly([p], size, ox, oy));
  return (sx, sy) => testers.some((t) => t(sx, sy));
}

/** Signed distance to a rounded box centred on (cx, cy). */
function sdRoundBox(x, y, cx, cy, hw, hh, r) {
  const qx = Math.abs(x - cx) - hw + r;
  const qy = Math.abs(y - cy) - hh + r;
  return Math.hypot(Math.max(qx, 0), Math.max(qy, 0)) + Math.min(Math.max(qx, qy), 0) - r;
}

const circlePoly = (cx, cy, r, n = 96) =>
  Array.from({ length: n }, (_, i) => [cx + Math.cos((i / n) * Math.PI * 2) * r, cy + Math.sin((i / n) * Math.PI * 2) * r]);

// ── Zuko's head ───────────────────────────────────────────────────────────────

/** Hair locks for the icon: the engine's, compressed so the head can stay big. */
const LOCKS = [
  { p: [0, 0, 0.05, -0.42, 0.76, -0.52, 1.0, 0.5], w: 0.21, min: 0 },
  { p: [0, 0, -0.06, -0.34, 0.34, -0.5, 0.7, -0.38], w: 0.15, min: 32 },
  { p: [0.02, -0.02, 0.5, -0.2, 0.92, -0.26, 1.16, 0.02], w: 0.11, min: 32 },
  { p: [0.02, 0, 0.42, -0.12, 0.86, 0.06, 0.92, 0.66], w: 0.09, min: 48 },
];

function renderZuko(size) {
  const px = new Uint8Array(size * size * 4);
  const tiny = size <= 24;
  const small = size <= 48;

  // Layout: fit the whole silhouette — head, tie and hair — into the square,
  // so the head is as big as the topknot allows.
  const lockSet = LOCKS.filter((l) => size >= l.min);
  const wk = tiny ? 1.45 : small ? 1.15 : 1;
  const tieTopU = tiny ? -1.24 : -1.3;
  let bx0 = -1, bx1 = 1, by0 = tieTopU, by1 = 1;
  for (const l of lockSet) {
    for (const [px, py] of lockPoly(l.p, l.w * wk, 0, tieTopU + 0.02, 1)) {
      bx0 = Math.min(bx0, px); bx1 = Math.max(bx1, px);
      by0 = Math.min(by0, py); by1 = Math.max(by1, py);
    }
  }
  const rimU = tiny ? 0.11 : 0.085;
  const fit = size / (Math.max(bx1 - bx0, by1 - by0) + 2 * rimU + 2 / size);
  const r = fit;
  const rim = Math.max(tiny ? 0.9 : 1, r * 0.075);
  const cx = size / 2 - ((bx0 + bx1) / 2) * r;
  const cy = size / 2 - ((by0 + by1) / 2) * r;

  // Hair-tie and the hair spilling out of it (relative to the head centre).
  const tieTop = tieTopU * r;
  const tieBot = -0.92 * r;
  const tieHW = (tiny ? 0.19 : small ? 0.14 : 0.12) * r;
  const locks = lockSet.map((l) => lockPoly(l.p, l.w * wk, 0, tieTop + 0.02 * r, r));
  const tiePoly = [
    [-tieHW, tieTop], [tieHW, tieTop], [tieHW, tieBot], [-tieHW, tieBot],
  ];
  const head = circlePoly(0, 0, r);

  const inRim = rasterUnion(
    [offsetPoly(head, rim), offsetPoly(tiePoly, rim), ...locks.map((p) => offsetPoly(p, rim))],
    size, cx, cy,
  );
  const inHair = rasterUnion(locks, size, cx, cy);
  const inHairCore = rasterUnion(locks.map((p) => offsetPoly(p, -Math.max(0.5, r * 0.05))), size, cx, cy);

  // Eyes and scar.
  const [ew, eh, esp, ey] = tiny ? [0.36, 0.5, 0.4, 0.08] : small ? [0.33, 0.46, 0.38, 0.07] : [0.3, 0.42, 0.37, 0.07];
  const cut = [196, -52];
  const eyes = [-1, 1].map((sd) => almondPoly(sd * esp * r, ey * r, ew * r, eh * r, cut, sd));
  const inEye = rasterUnion(eyes, size, cx, cy);
  const inEyeCore = rasterUnion(eyes.map((p) => offsetPoly(p, -Math.max(0.5, ew * r * 0.12))), size, cx, cy);
  const scar = tiny ? null : scarPoly(esp * r + 0.05 * r, ey * r - 0.07 * r, 1.32 * r);
  const inScar = scar ? rasterPoly([scar], size, cx, cy) : () => false;
  const inScarCore = scar ? rasterPoly([offsetPoly(scar, -Math.max(0.5, r * 0.035))], size, cx, cy) : () => false;

  for (let y = 0; y < size; y++) {
    for (let x = 0; x < size; x++) {
      let acc = [0, 0, 0];
      let alpha = 0;
      for (let sy = 0; sy < SS; sy++) {
        for (let sx = 0; sx < SS; sx++) {
          const gx = x * SS + sx;
          const gy = y * SS + sy;
          if (!inRim(gx, gy)) continue;
          const lx = (gx + 0.5) / SS - cx;
          const ly = (gy + 0.5) / SS - cy;
          const dc = Math.hypot(lx, ly);
          let col = OUTLINE;

          if (dc < r) {
            // The glossy sphere: lit from the top left, a warm rim lower right.
            const t = Math.hypot(lx + 0.36 * r, ly + 0.42 * r) / (1.3 * r);
            col = t < 0.45 ? mix(WHITE, CREAM, t / 0.45) : t < 0.82 ? mix(CREAM, BEIGE, (t - 0.45) / 0.37) : mix(BEIGE, SHADE, (t - 0.82) / 0.18);
            const warm = smooth(0.78, 1, dc / r) * clamp((lx + ly) / (1.2 * r), 0, 1);
            col = mix(col, ORANGE, warm * 0.4);
            // Specular highlight.
            const hx = lx + 0.4 * r;
            const hy = ly + 0.5 * r;
            const ca = Math.cos(0.65), sa = Math.sin(0.65);
            const ex = (hx * ca - hy * sa) / (0.3 * r);
            const eyy = (hx * sa + hy * ca) / (0.17 * r);
            const e2 = ex * ex + eyy * eyy;
            if (e2 < 1) col = mix(col, WHITE, (1 - e2) * 0.9);
            // The scar round the right eye.
            if (inScar(gx, gy)) col = mix(col, inScarCore(gx, gy) ? SCAR : SCAR_EDGE, inScarCore(gx, gy) ? 0.55 : 0.6);
            // Glow spilling round the eyes.
            for (const sd of [-1, 1]) {
              const dx = (lx - sd * esp * r) / (ew * r);
              const dy = (ly - ey * r) / (eh * r);
              const d = Math.hypot(dx, dy);
              col = mix(col, EYE_MID, Math.exp(-Math.pow(Math.max(0, d - 0.4) / 0.42, 2)) * 0.5);
            }
            if (inEye(gx, gy)) {
              if (!inEyeCore(gx, gy)) col = EYE_EDGE;
              else {
                let best = 9;
                for (const sd of [-1, 1]) {
                  const dx = (lx - sd * esp * r + sd * 0.03 * r) / (ew * r * 0.5);
                  const dy = (ly - ey * r - 0.06 * r) / (eh * r * 0.5);
                  best = Math.min(best, Math.hypot(dx, dy));
                }
                col = best < 0.42 ? mix(EYE_CORE, EYE_MID, best / 0.42) : mix(EYE_MID, ORANGE, (best - 0.42) / 0.58);
              }
            }
          } else if (dc < r + rim * 0.55 && !inHair(gx, gy)) {
            col = OUTLINE;
          }

          // The topknot sits on the crown, over the head's edge.
          const dTie = sdRoundBox(lx, ly, 0, (tieTop + tieBot) / 2, tieHW, (tieBot - tieTop) / 2, tieHW * 0.4);
          if (dTie < 0) {
            const k = (lx + tieHW) / (2 * tieHW);
            col = k < 0.32 ? mix(TIE_DARK, TIE_LIGHT, k / 0.32) : mix(TIE_LIGHT, TIE_DARK, (k - 0.32) / 0.68 * 0.85);
            if (!tiny && !small) {
              const b1 = Math.abs(ly - (tieTop + 0.055 * r));
              const b2 = Math.abs(ly - (tieBot - 0.07 * r));
              if (Math.min(b1, b2) < Math.max(0.5, 0.025 * r)) col = GOLD;
            }
            if (dTie > -Math.max(0.6, r * 0.03)) col = mix(col, OUTLINE, 0.7);
          } else if (dc >= r * 0.86 && Math.hypot(lx / (0.18 * r), (ly + 0.965 * r) / (0.075 * r)) < 1) {
            col = HAIR;
          } else if (inHair(gx, gy) && (dc >= r || ly < -0.9 * r)) {
            col = inHairCore(gx, gy) ? HAIR : mix(HAIR, HAIR_RIM, tiny ? 0.5 : 0.8);
          }

          acc = acc.map((v, i) => v + col[i]);
          alpha++;
        }
      }
      if (alpha === 0) continue;
      const o = (y * size + x) * 4;
      px[o] = Math.round(acc[0] / alpha);
      px[o + 1] = Math.round(acc[1] / alpha);
      px[o + 2] = Math.round(acc[2] / alpha);
      px[o + 3] = Math.round((alpha / (SS * SS)) * 255);
    }
  }
  return px;
}

// ── PNG ───────────────────────────────────────────────────────────────────────

const CRC_TABLE = (() => {
  const t = new Uint32Array(256);
  for (let n = 0; n < 256; n++) {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    t[n] = c >>> 0;
  }
  return t;
})();

function crc32(buf) {
  let c = 0xffffffff;
  for (const b of buf) c = CRC_TABLE[(c ^ b) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}

function chunk(type, data) {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length);
  const body = Buffer.concat([Buffer.from(type, "ascii"), data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(body));
  return Buffer.concat([len, body, crc]);
}

function encodePNG(size, rgba) {
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(size, 0);
  ihdr.writeUInt32BE(size, 4);
  ihdr[8] = 8; // bit depth
  ihdr[9] = 6; // RGBA
  const raw = Buffer.alloc(size * (size * 4 + 1));
  for (let y = 0; y < size; y++) {
    raw[y * (size * 4 + 1)] = 0; // filter: none
    Buffer.from(rgba.buffer, y * size * 4, size * 4).copy(raw, y * (size * 4 + 1) + 1);
  }
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk("IHDR", ihdr),
    chunk("IDAT", deflateSync(raw, { level: 9 })),
    chunk("IEND", Buffer.alloc(0)),
  ]);
}

// ── ICO (PNG-in-ICO, Vista and later) ─────────────────────────────────────────

function encodeICO(entries) {
  const header = Buffer.alloc(6);
  header.writeUInt16LE(0, 0);
  header.writeUInt16LE(1, 2);
  header.writeUInt16LE(entries.length, 4);
  const dir = Buffer.alloc(16 * entries.length);
  let offset = header.length + dir.length;
  entries.forEach((e, i) => {
    const o = i * 16;
    dir[o] = e.size >= 256 ? 0 : e.size;
    dir[o + 1] = e.size >= 256 ? 0 : e.size;
    dir[o + 2] = 0;
    dir[o + 3] = 0;
    dir.writeUInt16LE(1, o + 4);
    dir.writeUInt16LE(32, o + 6);
    dir.writeUInt32LE(e.png.length, o + 8);
    dir.writeUInt32LE(offset, o + 12);
    offset += e.png.length;
  });
  return Buffer.concat([header, dir, ...entries.map((e) => e.png)]);
}

// ── Go ────────────────────────────────────────────────────────────────────────

mkdirSync(OUT, { recursive: true });

const png = (size) => encodePNG(size, renderZuko(size));

const files = {
  "32x32.png": png(32),
  "128x128.png": png(128),
  "128x128@2x.png": png(256),
  "icon.png": png(512),
};
for (const [name, data] of Object.entries(files)) {
  writeFileSync(join(OUT, name), data);
  console.log(`${name} — ${data.length} bytes`);
}

const ico = encodeICO([16, 24, 32, 48, 64, 128, 256].map((size) => ({ size, png: png(size) })));
writeFileSync(join(OUT, "icon.ico"), ico);
console.log(`icon.ico — ${ico.length} bytes`);

// Optional: ICON_PREVIEW=<dir> also writes every ICO size as a PNG for review.
if (process.env.ICON_PREVIEW) {
  mkdirSync(process.env.ICON_PREVIEW, { recursive: true });
  for (const size of [16, 24, 32, 48, 64]) writeFileSync(join(process.env.ICON_PREVIEW, `icon-${size}.png`), png(size));
}
