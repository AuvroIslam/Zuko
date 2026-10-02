// Firebending — the fire Zuko throws, drawn with Canvas 2D.
//
// Pure helpers with no state: every flame is a function of a clock `t` and a
// seed, so the island, the launch greeting, the drop sequence and the dev
// preview all draw the same fire, and a frozen clock gives a frozen frame. The
// look is cel-shaded like the concept sheet: a red-orange outer tongue, an
// orange middle and a yellow core, plus glowing embers. Nothing allocates per
// frame beyond a gradient or two, so a few dozen flames stay cheap.

// ── Colour ────────────────────────────────────────────────────────────────────

export type RGB = readonly [number, number, number]; // components 0…1

export function hexToRGB(hex: string): RGB {
  const h = hex.replace("#", "");
  const v = parseInt(h, 16);
  return [((v >> 16) & 255) / 255, ((v >> 8) & 255) / 255, (v & 255) / 255];
}

const clamp01 = (v: number) => Math.max(0, Math.min(1, v));

export const rgba = (c: RGB, a = 1) =>
  `rgba(${Math.round(c[0] * 255)},${Math.round(c[1] * 255)},${Math.round(c[2] * 255)},${clamp01(a)})`;

export const mix3 = (a: RGB, b: RGB, t: number): RGB => [
  a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t,
];

/** Which fire burns: the everyday ember, angry red, or the golden "done" fire. */
export type FireKind = "ember" | "danger" | "success";

/** Outer → inner: deep shadow, outer tongue, middle tongue, tip glow, white-hot core. */
export interface FirePalette { deep: RGB; base: RGB; mid: RGB; tip: RGB; core: RGB }

export const FIRE: Record<FireKind, FirePalette> = {
  ember: {
    deep: hexToRGB("#B33A2E"), base: hexToRGB("#EE5A17"), mid: hexToRGB("#F28A1E"),
    tip: hexToRGB("#FFC24D"), core: hexToRGB("#FFF3CF"),
  },
  danger: {
    deep: hexToRGB("#8E1A12"), base: hexToRGB("#E4321C"), mid: hexToRGB("#FF5A2A"),
    tip: hexToRGB("#FF9F45"), core: hexToRGB("#FFE0B8"),
  },
  success: {
    deep: hexToRGB("#C2561A"), base: hexToRGB("#F2801E"), mid: hexToRGB("#FFB347"),
    tip: hexToRGB("#FFE08A"), core: hexToRGB("#FFFBEA"),
  },
};

export function mixFire(a: FirePalette, b: FirePalette, t: number): FirePalette {
  if (t <= 0.001) return a;
  if (t >= 0.999) return b;
  return {
    deep: mix3(a.deep, b.deep, t), base: mix3(a.base, b.base, t), mid: mix3(a.mid, b.mid, t),
    tip: mix3(a.tip, b.tip, t), core: mix3(a.core, b.core, t),
  };
}

/** Small deterministic PRNG (mulberry32): the same seed throws the same embers. */
export function seeded(seed: number): () => number {
  let a = (seed * 2654435761) >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

export const TAU = Math.PI * 2;
export const easeOut = (t: number) => 1 - Math.pow(1 - clamp01(t), 3);
export const easeInOut = (t: number) => {
  const k = clamp01(t);
  return k < 0.5 ? 4 * k * k * k : 1 - Math.pow(-2 * k + 2, 3) / 2;
};

// ── Shapes ────────────────────────────────────────────────────────────────────

/**
 * A flame tongue: a teardrop with a round bottom centred on the origin, its
 * tip at (`tipX`, −`h`). Round-bottomed so it works rooted on a surface and
 * floating free.
 */
export function tearPath(x: CanvasRenderingContext2D, w: number, h: number, tipX: number) {
  const r = w / 2;
  x.beginPath();
  x.moveTo(-r, 0);
  x.bezierCurveTo(-r, r * 0.9, r, r * 0.9, r, 0);
  x.bezierCurveTo(r * 1.05, -h * 0.38, tipX + r * 0.42, -h * 0.66, tipX, -h);
  x.bezierCurveTo(tipX - r * 0.5, -h * 0.64, -r * 1.08, -h * 0.4, -r, 0);
  x.closePath();
}

/**
 * One flame: three cel layers rising from (`px`, `py`) towards `ang` (radians
 * from straight up, clockwise). `w` and `h` size the outer tongue.
 */
export function flame(
  x: CanvasRenderingContext2D, px: number, py: number, ang: number,
  w: number, h: number, t: number, pal: FirePalette, seed = 0, alpha = 1,
) {
  if (h < 0.4 || alpha <= 0.01) return;
  x.save();
  x.translate(px, py);
  if (ang) x.rotate(ang);
  if (alpha < 1) x.globalAlpha *= alpha;
  const f = 1 + 0.17 * Math.sin(t * 9.3 + seed * 1.7) + 0.08 * Math.sin(t * 15.1 + seed * 3.1);
  const lean = 0.16 * Math.sin(t * 5.7 + seed * 2.3) + 0.06 * Math.sin(t * 12.7 + seed);
  x.fillStyle = rgba(pal.base);
  tearPath(x, w, h * f, lean * h * f);
  x.fill();
  const f2 = 1 + (f - 1) * 0.6;
  x.fillStyle = rgba(pal.mid);
  tearPath(x, w * 0.7, h * 0.74 * f2, lean * h * 0.6);
  x.fill();
  if (w > 2.2) {
    x.fillStyle = rgba(pal.tip);
    tearPath(x, w * 0.4, h * 0.46 * f2, lean * h * 0.3);
    x.fill();
  }
  x.restore();
}

/** A soft round glow. `a` is the centre alpha. */
export function glow(x: CanvasRenderingContext2D, px: number, py: number, r: number, c: RGB, a: number) {
  if (r <= 0.5 || a <= 0.01) return;
  const g = x.createRadialGradient(px, py, 0, px, py, r);
  g.addColorStop(0, rgba(c, a));
  g.addColorStop(0.45, rgba(c, a * 0.45));
  g.addColorStop(1, rgba(c, 0));
  x.fillStyle = g;
  x.beginPath();
  x.arc(px, py, r, 0, TAU);
  x.fill();
}

/** A glowing ember; `k` is its age 0…1 (white-hot → yellow → red, shrinking). */
export function ember(x: CanvasRenderingContext2D, px: number, py: number, r: number, k: number, pal: FirePalette) {
  if (k <= 0 || k >= 1 || r <= 0.15) return;
  const c = k < 0.35 ? mix3(pal.core, pal.tip, k / 0.35) : mix3(pal.tip, pal.base, (k - 0.35) / 0.65);
  const a = 1 - k * k;
  const rr = r * (1 - 0.45 * k);
  if (rr > 1.4) {
    x.fillStyle = rgba(pal.mid, 0.28 * a);
    x.beginPath();
    x.arc(px, py, rr * 2.1, 0, TAU);
    x.fill();
  }
  x.fillStyle = rgba(c, a);
  x.beginPath();
  x.arc(px, py, rr, 0, TAU);
  x.fill();
}

/**
 * A band of fire along a sampled centre line: `pts` is a flat [x0, y0, x1, …]
 * list, `hw` the half width at each point. Licks flicker out of the left-hand
 * edge (relative to the direction of travel). Three cel layers.
 */
export function flameRibbon(
  x: CanvasRenderingContext2D, pts: number[], hw: number[], t: number,
  pal: FirePalette, alpha = 1, seed = 0,
) {
  const n = pts.length / 2;
  if (n < 2 || alpha <= 0.01) return;
  x.save();
  if (alpha < 1) x.globalAlpha *= alpha;
  const layers: [number, RGB, number][] = [[1, pal.base, 0.6], [0.62, pal.mid, 0.4], [0.3, pal.tip, 0.2]];
  for (const [s, col, lick] of layers) {
    x.beginPath();
    for (let pass = 0; pass < 2; pass++) {
      for (let j = 0; j < n; j++) {
        const i = pass === 0 ? j : n - 1 - j;
        const ia = Math.max(0, i - 1);
        const ib = Math.min(n - 1, i + 1);
        let tx = pts[ib * 2] - pts[ia * 2];
        let ty = pts[ib * 2 + 1] - pts[ia * 2 + 1];
        const l = Math.hypot(tx, ty) || 1;
        tx /= l;
        ty /= l;
        // Left-hand normal of the direction of travel.
        const nx = ty;
        const ny = -tx;
        const w = hw[i] * s;
        let off: number;
        if (pass === 0) {
          const k = Math.max(0, Math.sin(i * 1.9 + t * 13 + seed * 2.1));
          off = w * (1 + lick * k * k);
        } else {
          off = -w * 0.55;
        }
        const px = pts[i * 2] + nx * off;
        const py = pts[i * 2 + 1] + ny * off;
        if (pass === 0 && j === 0) x.moveTo(px, py);
        else x.lineTo(px, py);
      }
    }
    x.closePath();
    x.fillStyle = rgba(col);
    x.fill();
  }
  x.restore();
}

/**
 * A fireball heading towards `ang` (radians, 0 = +x): a glow, a flickering
 * tail of three tongues trailing behind, and a white-hot head of radius ~`s`.
 */
export function fireball(
  x: CanvasRenderingContext2D, px: number, py: number, ang: number, s: number,
  t: number, pal: FirePalette, alpha = 1, seed = 0,
) {
  if (s <= 0.3 || alpha <= 0.01) return;
  glow(x, px, py, s * 3, pal.mid, 0.42 * alpha);
  x.save();
  x.translate(px, py);
  x.rotate(ang - Math.PI / 2); // the tear's "up" now trails behind the ball
  if (alpha < 1) x.globalAlpha *= alpha;
  const f = 1 + 0.18 * Math.sin(t * 21 + seed) + 0.1 * Math.sin(t * 33 + seed * 2);
  const wob = 0.18 * Math.sin(t * 17 + seed * 1.3);
  const layers: [number, number, RGB][] = [
    [2.1, 3.4, pal.base], [1.55, 2.5, pal.mid], [1.05, 1.6, pal.tip],
  ];
  for (const [wk, hk, c] of layers) {
    x.fillStyle = rgba(c);
    tearPath(x, s * wk, s * hk * f, wob * s * hk);
    x.fill();
  }
  x.fillStyle = rgba(pal.core);
  x.beginPath();
  x.arc(0, s * 0.18, s * 0.52, 0, TAU);
  x.fill();
  x.restore();
}

/**
 * The impact: a white flash, a ring of fire, flame petals and sparks flying
 * out. `k` is the progress 0…1, `s` the size (about the fireball radius).
 */
export function burst(
  x: CanvasRenderingContext2D, px: number, py: number, k: number, s: number,
  t: number, pal: FirePalette, seed = 0,
) {
  if (k <= 0 || k >= 1 || s <= 0.3) return;
  const rnd = seeded(seed + 17);
  const out = easeOut(k);
  const fade = 1 - k;

  glow(x, px, py, s * (2.2 + 3 * out), pal.mid, 0.55 * fade * fade);
  if (k < 0.35) glow(x, px, py, s * (1.1 + 1.2 * out), pal.core, 0.9 * (1 - k / 0.35));

  // Ring.
  x.strokeStyle = rgba(pal.tip, 0.8 * fade);
  x.lineWidth = Math.max(0.6, s * 0.5 * fade);
  x.beginPath();
  x.arc(px, py, s * (0.9 + 2.6 * out), 0, TAU);
  x.stroke();

  // Flame petals leaning outwards.
  const petals = 7;
  for (let i = 0; i < petals; i++) {
    const a = (i / petals) * TAU + rnd() * 0.5;
    const d = s * (0.5 + 1.5 * out);
    const h = s * (1.9 - 1.2 * k) * (0.7 + rnd() * 0.5);
    flame(x, px + Math.sin(a) * d, py - Math.cos(a) * d, a, s * 0.9, h, t, pal, i + seed, fade);
  }

  // Sparks: short streaks thrown outwards.
  x.lineCap = "round";
  for (let i = 0; i < 12; i++) {
    const a = rnd() * TAU;
    const sp = 0.6 + rnd() * 0.8;
    const d = s * (0.8 + 4.2 * out * sp);
    const len = s * (0.9 * fade + 0.2);
    const sx = px + Math.cos(a) * d;
    const sy = py + Math.sin(a) * d + s * 1.2 * k * k;
    x.strokeStyle = rgba(mix3(pal.core, pal.tip, k), fade);
    x.lineWidth = Math.max(0.6, s * 0.18 * (1 - k * 0.6));
    x.beginPath();
    x.moveTo(sx - Math.cos(a) * len, sy - Math.sin(a) * len);
    x.lineTo(sx, sy);
    x.stroke();
  }
}

/** Samples an elliptical arc into a flat point list (for flameRibbon). */
export function arcPoints(
  cx: number, cy: number, rx: number, ry: number, a0: number, a1: number, n: number, rot = 0,
): number[] {
  const out: number[] = [];
  const cr = Math.cos(rot);
  const sr = Math.sin(rot);
  for (let i = 0; i < n; i++) {
    const a = a0 + ((a1 - a0) * i) / (n - 1);
    const ex = Math.cos(a) * rx;
    const ey = Math.sin(a) * ry;
    out.push(cx + ex * cr - ey * sr, cy + ex * sr + ey * cr);
  }
  return out;
}

/**
 * Half widths that swell and taper to points at both ends; `bias` > 1 moves
 * the thickest part towards the end of the list (the leading edge).
 */
export function taper(n: number, w: number, bias = 1.6): number[] {
  const out: number[] = [];
  for (let i = 0; i < n; i++) {
    const s = Math.pow(i / (n - 1), 1 / bias);
    out.push(w * Math.pow(Math.sin(Math.PI * Math.min(0.98, Math.max(0.02, s))), 0.7));
  }
  return out;
}

/**
 * The fire punch's tail: two tongues of flame that sweep in from behind the
 * shoulder, over and under the arm, and wrap into the fist at (`fx`, `fy`)
 * punching along (`dx`, `dy`). `u` is the size (head radius), `k` 0…1 the
 * progress: the tongues grow in, burn, then fade.
 */
export function punchTrail(
  x: CanvasRenderingContext2D, fx: number, fy: number, dx: number, dy: number, u: number,
  k: number, t: number, pal: FirePalette, seed = 0,
) {
  if (k <= 0 || k >= 1) return;
  const grow = easeOut(Math.min(1, k / 0.32));
  const fade = 1 - easeInOut((k - 0.4) / 0.6);
  const px = -dy;
  const py = dx;
  const n = 14;
  for (const side of [1, -1]) {
    const sc = side > 0 ? 1 : 0.78;
    const p0x = fx - dx * 1.7 * u * sc + px * side * 0.5 * u * sc;
    const p0y = fy - dy * 1.7 * u * sc + py * side * 0.5 * u * sc;
    const p1x = fx - dx * 0.55 * u + px * side * 0.85 * u * sc;
    const p1y = fy - dy * 0.55 * u + py * side * 0.85 * u * sc;
    const p2x = fx + dx * 0.2 * u;
    const p2y = fy + dy * 0.2 * u;
    const s0 = 1 - grow;
    const pts: number[] = [];
    for (let i = 0; i < n; i++) {
      const s = s0 + ((1 - s0) * i) / (n - 1);
      const a = (1 - s) * (1 - s);
      const b = 2 * (1 - s) * s;
      const c = s * s;
      pts.push(a * p0x + b * p1x + c * p2x, a * p0y + b * p1y + c * p2y);
    }
    let hw = taper(n, 0.26 * u * sc, 0.45);
    // flameRibbon licks out of the left-hand edge: run the lower tongue
    // backwards so its licks point away from the arm too.
    if (side < 0) {
      const rev: number[] = [];
      for (let i = n - 1; i >= 0; i--) rev.push(pts[i * 2], pts[i * 2 + 1]);
      pts.splice(0, pts.length, ...rev);
      hw = hw.slice().reverse();
    }
    flameRibbon(x, pts, hw, t, pal, fade, seed + side * 5);
  }
}

/**
 * A swirl of fire: two crescents of flame chasing each other round
 * (`cx`, `cy`). `k` 0…1 drives the sweep and the fade; `squash` flattens it
 * into a ring seen from the side.
 */
export function swirl(
  x: CanvasRenderingContext2D, cx: number, cy: number, r: number, k: number,
  t: number, pal: FirePalette, squash = 1, rot = 0, seed = 0,
) {
  if (k <= 0 || k >= 1 || r <= 1) return;
  const a = Math.min(1, k / 0.15) * (1 - easeInOut((k - 0.55) / 0.45));
  const spin = easeOut(k) * TAU * 1.1 + seed;
  for (let j = 0; j < 2; j++) {
    const a0 = spin + j * Math.PI;
    const sweep = Math.PI * (0.7 + 0.5 * Math.min(1, k * 2.5));
    const rr = r * (0.75 + 0.35 * easeOut(k)) * (j ? 0.82 : 1);
    const n = 14;
    const pts = arcPoints(cx, cy, rr, rr * squash, a0, a0 + sweep, n, rot);
    flameRibbon(x, pts, taper(n, r * 0.2 * (j ? 0.75 : 1)), t, pal, a, seed + j * 3);
  }
}
