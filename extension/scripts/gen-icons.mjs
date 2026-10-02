// Draws Zuko's face into the extension's toolbar icons (PNG 16/32/48/128): the navy
// shield, dark glass visor with two teal LED eyes, and the ember flame on top. The
// rasteriser and PNG encoder are the ones from app/scripts/gen-icons.mjs (same repo,
// same character), with no dependencies beyond node:zlib.
//
//   node scripts/gen-icons.mjs

import { deflateSync } from "node:zlib";
import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const OUT = join(dirname(fileURLToPath(import.meta.url)), "..", "icons");

// ── Palette ───────────────────────────────────────────────────────────────────

const hex = (h) => [parseInt(h.slice(1, 3), 16), parseInt(h.slice(3, 5), 16), parseInt(h.slice(5, 7), 16)];
const RIM = hex("#04060C");
const BODY_TOP = hex("#323D62");
const BODY_BOTTOM = hex("#121829");
const RIM_LIGHT = hex("#8FA6E0");
const VISOR = hex("#070A12");
const VISOR_TOP = hex("#18213A");
const LED = hex("#2EE6C5");
const LED_CORE = hex("#D9FFF6");
const FIRE_ROOT = hex("#F0520F");
const FIRE_BASE = hex("#FF7A1A");
const FIRE_TIP = hex("#FFD166");
const CORE_BASE = hex("#FFAE3D");
const CORE_TIP = hex("#FFF3C8");

const SS = 4; // supersampling factor

const lerp = (a, b, t) => a + (b - a) * t;
const clamp = (v, a, b) => Math.max(a, Math.min(b, v));
const mix = (a, b, t) => [0, 1, 2].map((i) => lerp(a[i], b[i], clamp(t, 0, 1)));

// ── Geometry (mirrors SHIELD and shieldPath() in src/character/engine.ts) ─────

const SHIELD = {
  halfW: 1.0, top: -0.86, corner: 0.36, bulge: 0.035, flankY: 0.24,
  taperX: 0.58, taperY: 0.74, tipHalf: 0.16, tipY: 0.98,
};

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
const quad = (p0, p1, p2, n, out) => {
  for (let i = 1; i <= n; i++) {
    const t = i / n;
    const u = 1 - t;
    out.push([u * u * p0[0] + 2 * u * t * p1[0] + t * t * p2[0], u * u * p0[1] + 2 * u * t * p1[1] + t * t * p2[1]]);
  }
};

/** The shield outline as a polygon, centred on the origin, half width R. */
function shieldPoly(R) {
  const S = SHIELD;
  const a = S.halfW * R, top = S.top * R, rc = S.corner * R, bulge = S.bulge * R;
  const flankY = S.flankY * R, taperX = S.taperX * R, taperY = S.taperY * R;
  const tipHalf = S.tipHalf * R, tipY = S.tipY * R;
  const d = ((tipY - taperY) * tipHalf) / (2 * taperX - tipHalf);
  const tipStartY = tipY - d, tipCtlY = tipY + d;
  const ex = a - rc, k = rc * 0.55, tx = 0.6 * ex, ty = 1.33 * bulge;
  const tl = Math.hypot(tx, ty) || 1;
  const p = [[-ex, top]];
  const N = 24;
  cubic([-ex, top], [-0.4 * ex, top - ty], [0.4 * ex, top - ty], [ex, top], N, p);
  cubic([ex, top], [ex + (tx / tl) * k, top + (ty / tl) * k], [a, top + rc - k], [a, top + rc], N, p);
  cubic([a, top + rc], [a, flankY], [taperX, taperY], [tipHalf, tipStartY], N * 2, p);
  quad([tipHalf, tipStartY], [0, tipCtlY], [-tipHalf, tipStartY], N, p);
  cubic([-tipHalf, tipStartY], [-taperX, taperY], [-a, flankY], [-a, top + rc], N * 2, p);
  cubic([-a, top + rc], [-a, top + rc - k], [-ex - (tx / tl) * k, top + (ty / tl) * k], [-ex, top], N, p);
  p.pop(); // the last point repeats the first
  return p;
}

/** One flame tongue (same curve as tonguePath in the engine). */
function tonguePoly(bx, by, w, h, tipX) {
  const p = [[bx - w / 2, by]];
  cubic([bx - w / 2, by], [bx - w * 0.55, by - h * 0.42], [tipX - w * 0.18, by - h * 0.72], [tipX, by - h], 20, p);
  cubic([tipX, by - h], [tipX + w * 0.2, by - h * 0.7], [bx + w * 0.55, by - h * 0.4], [bx + w / 2, by], 20, p);
  p.pop();
  return p;
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

// ── Zuko ──────────────────────────────────────────────────────────────────────

function renderZuko(size) {
  const px = new Uint8Array(size * size * 4);
  const tiny = size <= 24;
  const small = size <= 48;

  // Proportions: small icons drop detail and grow the face so it still reads.
  const flameH = tiny ? 0.62 : 0.78;
  const R = size * (tiny ? 0.41 : 0.375);
  const rim = Math.max(tiny ? 0.85 : 1, R * 0.07);
  const topY = SHIELD.top * R - flameH * R; // top of the flame
  const botY = SHIELD.tipY * R;
  const cx = size / 2;
  const cy = size / 2 - (topY + botY) / 2;

  const body = shieldPoly(R);
  const inner = offsetPoly(body, -Math.max(0.6, R * 0.09));

  // Flame tongues, rooted under the top of the head.
  const baseY = SHIELD.top * R + R * 0.2;
  const W = R * (tiny ? 0.72 : 0.62);
  const H = R * flameH + R * 0.2;
  const outer = tiny
    ? [tonguePoly(0, baseY, W, H, 0.02 * H)]
    : [
        tonguePoly(-0.34 * W, baseY, 0.56 * W, 0.62 * H, -0.34 * W - 0.26 * 0.62 * H),
        tonguePoly(0.36 * W, baseY, 0.52 * W, 0.54 * H, 0.36 * W + 0.28 * 0.54 * H),
        tonguePoly(0, baseY, 0.84 * W, H, 0.03 * H),
      ];
  const core = tonguePoly(0.02 * W, baseY, (tiny ? 0.5 : 0.44) * W, (tiny ? 0.62 : 0.56) * H, 0.02 * W);

  const inRim = rasterUnion([offsetPoly(body, rim), ...outer.map((p) => offsetPoly(p, rim))], size, cx, cy);
  const inBody = rasterPoly([body], size, cx, cy);
  const inInner = rasterPoly([inner], size, cx, cy);
  const inFlame = rasterUnion(outer, size, cx, cy);
  const inCore = rasterPoly([core], size, cx, cy);

  // Visor and LEDs (local coordinates, analytic).
  const vcy = -0.2 * R;
  const vhw = (tiny ? 0.84 : 0.8) * R;
  const vhh = (tiny ? 0.34 : small ? 0.3 : 0.27) * R;
  const vr = Math.min(vhh, 0.24 * R);
  const ew = (tiny ? 0.25 : 0.19) * R; // half width
  const eh = (tiny ? 0.15 : 0.1) * R; // half height
  const esp = (tiny ? 0.4 : 0.36) * R;
  const ey = vcy + 0.02 * R;
  const glowR = R * 0.22;

  const flameTop = baseY - H;
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
          let col = RIM;
          if (inBody(gx, gy)) {
            const t = (ly - SHIELD.top * R) / ((SHIELD.tipY - SHIELD.top) * R);
            col = mix(BODY_TOP, BODY_BOTTOM, t);
            // Cool rim light on the inner edge, strongest top-left.
            if (!inInner(gx, gy)) {
              const k = clamp(0.75 - (lx / R) * 0.25 - t * 0.7, 0.08, 0.75);
              col = mix(col, RIM_LIGHT, k * (tiny ? 0.55 : 0.7));
            }
            // Visor.
            const dv = sdRoundBox(lx, ly, 0, vcy, vhw, vhh, vr);
            if (dv < 0) {
              const vt = (ly - (vcy - vhh)) / (2 * vhh);
              col = mix(VISOR_TOP, VISOR, clamp(vt * 2.2, 0, 1));
              // LED eyes with a soft bloom.
              let glow = 0;
              let led = 0;
              let hot = 0;
              for (const sd of [-1, 1]) {
                const de = sdRoundBox(lx, ly, sd * esp, ey, ew, eh, eh);
                if (de < 0) {
                  led = 1;
                  if (sdRoundBox(lx, ly, sd * esp, ey, ew * 0.6, eh * 0.4, eh * 0.4) < 0 && !tiny) hot = 1;
                }
                glow = Math.max(glow, Math.exp(-Math.pow(Math.max(0, de) / glowR, 2)));
              }
              col = mix(col, LED, glow * 0.45);
              if (led) col = hot ? LED_CORE : mix(LED, LED_CORE, tiny ? 0.15 : 0.25);
            } else if (dv < Math.max(0.7, R * 0.05)) {
              col = mix(col, RIM, 0.75); // bezel
            }
          } else if (inFlame(gx, gy)) {
            const t = clamp((baseY - ly) / (baseY - flameTop), 0, 1);
            col = t < 0.3 ? mix(FIRE_ROOT, FIRE_BASE, t / 0.3) : mix(FIRE_BASE, FIRE_TIP, (t - 0.3) / 0.7);
            if (inCore(gx, gy)) col = mix(CORE_BASE, CORE_TIP, t * 1.4);
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

// ── Go ────────────────────────────────────────────────────────────────────────

mkdirSync(OUT, { recursive: true });
for (const size of [16, 32, 48, 128]) {
  const data = encodePNG(size, renderZuko(size));
  writeFileSync(join(OUT, `icon-${size}.png`), data);
  console.log(`icons/icon-${size}.png — ${data.length} bytes`);
}
