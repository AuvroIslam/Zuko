// The launch greeting: a swirl of fire spins up in the opened island, Zuko
// drops into it and lands with a burst of embers, his eyes ignite, he winds up
// and throws a fire punch — the fireball streaks across the island, bursts at
// the far end and sets the island's edge alight — then he looks pleased with
// himself and shrinks into his spot in the compact island.
//
// One renderer: the greeting poses a BotEngine (fields + `clock`) every frame
// and calls `drawAt()`; the fire comes from the same helpers the engine uses.
// Everything is laid out in a 640×150 reference space, the size of the
// expanded island while the greeting plays.

import { Sound } from "../core/sound";
import { COMPACT_W } from "../core/layout";
import { BotEngine, hexToRGB, type EyeShape, type RGB } from "./engine";
import {
  FIRE, TAU, burst, ember, fireball, flame, glow, punchTrail, rgba, seeded, swirl,
} from "./fire";

// ── Timing (seconds) ──────────────────────────────────────────────────────────

const T = {
  drop0: 0.12,
  land: 0.52,
  boot0: 0.62,
  boot1: 1.08,
  wind: 1.2,
  hit: 1.36,
  launch: 1.34,
  impact: 1.66,
  happy0: 2.05,
  happy1: 2.75,
  content0: 2.9,
  content1: 3.05,
  down0: 3.15,
  down1: 3.45,
  blink2: 3.8,
  end: 4.6,
  autoLeave: 4.9,
  COLLAPSE: 0.34,
};

export const GREETING_END = T.end;

// ── Geometry (640×150) ────────────────────────────────────────────────────────

const C0 = { x: 320, y: 90 };
/** Zuko's half box at full size. */
const R_FULL = 33;
/** Where the compact island draws its bot: botPosition("compact"), diameter 20. */
const EAR = { x: 320 - COMPACT_W / 2 + 40, y: 16, R: 10 };
const CARD = { x: 10, y: 36, w: 620, h: 104 };
const CARD_R = 20;
/** Where the fireball lands: the island's far right end. */
const TARGET = { x: 598, y: 84 };

const PAL = FIRE.ember;

// ── Easing ────────────────────────────────────────────────────────────────────

const E = {
  out: (t: number) => 1 - Math.pow(1 - t, 3),
  easeIn: (t: number) => t * t * t,
  inOut: (t: number) => (t < 0.5 ? 4 * t * t * t : 1 - Math.pow(-2 * t + 2, 3) / 2),
  back: (t: number) => {
    const c1 = 1.70158;
    const c3 = c1 + 1;
    return 1 + c3 * Math.pow(t - 1, 3) + c1 * Math.pow(t - 1, 2);
  },
};

const clamp = (v: number, a: number, b: number) => Math.max(a, Math.min(b, v));
const lerp = (a: number, b: number, t: number) => a + (b - a) * t;
const seg = (t: number, a: number, b: number) => clamp((t - a) / (b - a), 0, 1);
const bump = (t: number, a: number, b: number) => Math.sin(Math.PI * seg(t, a, b));

// ── Pose ──────────────────────────────────────────────────────────────────────

interface Pose {
  R: number; x: number; y: number; sx: number; sy: number; tilt: number;
  eye: EyeShape; open: number; es: number;
  lookX: number; lookY: number;
  /** Eye ignition 0…1, the punch (extension 0…1) and its fist flame. */
  boot: number; punch: number; fist: number;
  halo: number; minis: number; fx: number; card: number;
}

function blinkAt(t: number, tb: number): number {
  const k = seg(t, tb, tb + 0.12);
  return k > 0 && k < 1 ? 1 - Math.sin(Math.PI * k) * 0.94 : 1;
}

function greetPose(t: number): Pose {
  const R = R_FULL;
  const x = C0.x;
  // Falls in from above the island and lands with a squash.
  const fall = E.easeIn(seg(t, T.drop0, T.land));
  let y = lerp(-50, C0.y, fall);
  let sx = 1;
  let sy = 1;
  let tilt = 0;
  if (t < T.land) {
    sy = 1 + 0.12 * fall;
    sx = 1 - 0.06 * fall;
  } else if (t < T.land + 0.4) {
    const k = seg(t, T.land, T.land + 0.4);
    const sq = Math.sin(Math.PI * Math.min(1, k * 2)) * (1 - k);
    sy = 1 - 0.2 * sq;
    sx = 1 + 0.16 * sq;
  }

  // Wind-up, punch, hold, recover.
  let punch = 0;
  let fist = 0;
  if (t >= T.wind) {
    const cock = E.out(seg(t, T.wind, T.hit - 0.08));
    const hit = E.out(seg(t, T.hit - 0.08, T.hit));
    const back = E.inOut(seg(t, T.hit + 0.35, T.hit + 0.7));
    punch = Math.max(0, hit - back) + (hit < 1 ? -0.25 * cock : 0);
    punch = clamp(punch, 0, 1);
    fist = Math.max(0.6 * cock, hit) * (1 - E.inOut(seg(t, T.hit + 0.2, T.hit + 0.75)));
    tilt = (-0.08 * cock + 0.2 * hit) * (1 - back);
    y += -R * 0.04 * hit * (1 - back);
  }
  // A small proud sway after the burst.
  if (t >= T.happy0 && t < T.content1) {
    const w = t - T.happy0;
    const fade = 1 - seg(t, T.content0, T.content1);
    tilt += Math.sin(w * 2 * Math.PI * 0.9 + 0.6) * 0.045 * fade;
    y += Math.sin(w * 2 * Math.PI * 1.8) * 0.8 * fade;
  }
  if (t >= T.content0 && t < T.down1) y += R * 0.08 * Math.sin(Math.PI * seg(t, T.content0, T.down1));

  let eye: EyeShape = "pill";
  if (t >= T.wind && t < T.hit + 0.6) eye = "angry";
  if (t >= T.happy0 && t < T.happy1) eye = "happy";
  if (t >= T.content0 && t < T.content1) eye = "closed";
  if (t >= T.down0 && t < T.down1) eye = "closed";

  // Eyes snap a little wide as they ignite.
  const es = 1 + 0.16 * bump(t, T.boot1 - 0.15, T.boot1 + 0.25);
  const open = Math.min(blinkAt(t, 2.0), blinkAt(t, T.blink2));

  let lookX = 0;
  let lookY = 0;
  if (t < T.land) lookY = -0.4;
  if (t >= T.boot0 && t < T.wind) {
    const k = E.inOut(seg(t, T.boot0, T.boot1));
    lookY = lerp(-0.3, 0.1, k);
  }
  if (t >= T.wind && t < T.happy0) lookX = 0.85 * E.out(seg(t, T.wind, T.hit));
  if (t >= T.happy0 && t < T.content0) {
    const k = E.inOut(seg(t, T.happy0, T.happy0 + 0.3));
    lookX = lerp(0.85, -0.5, k);
    lookY = lerp(0, 0.25, k);
  }
  if (t >= T.content0) {
    const k = E.inOut(seg(t, T.down1, T.down1 + 0.35));
    lookX = lerp(-0.5, 0, k);
    lookY = lerp(0.25, 0, k);
  }

  return {
    R, x, y, sx, sy, tilt,
    eye, open, es,
    lookX, lookY,
    boot: E.inOut(seg(t, T.boot0, T.boot1)),
    punch, fist,
    halo: E.out(seg(t, T.land - 0.1, T.land + 0.4)),
    minis: 0,
    fx: 1,
    card: seg(t, 0.05, 0.3),
  };
}

function smallPose(): Pose {
  return {
    R: EAR.R, x: EAR.x, y: EAR.y,
    sx: 1, sy: 1, tilt: 0,
    eye: "pill", open: 1, es: 1,
    lookX: 0, lookY: 0,
    boot: 1, punch: 0, fist: 0,
    halo: 0.6, minis: 1, fx: 1, card: 0,
  };
}

function pose(t: number, tc: number): Pose {
  if (t < tc) return greetPose(Math.min(t, T.end + 10));
  const a = greetPose(tc);
  const b = smallPose();
  const e = E.inOut(seg(t, tc, tc + T.COLLAPSE));
  const p: Pose = { ...a };
  p.x = lerp(a.x, b.x, e);
  p.y = lerp(a.y, b.y, e);
  p.R = lerp(a.R, b.R, e);
  p.halo = lerp(a.halo, b.halo, e);
  // Cut short mid-ignition: finish lighting up on the way down.
  p.boot = lerp(a.boot, 1, e);
  p.punch = a.punch * (1 - e);
  p.fist = a.fist * (1 - e);
  p.card = a.card * (1 - seg(t, tc, tc + 0.18));
  p.tilt = a.tilt * (1 - e);
  p.sx = lerp(a.sx, 1, e);
  p.sy = lerp(a.sy, 1, e);
  p.es = lerp(a.es, 1, e);
  p.eye = "pill";
  p.open = blinkAt(t, tc + 0.14);
  p.lookX = a.lookX * (1 - e);
  p.lookY = a.lookY * (1 - e);
  p.minis = E.back(seg(t, tc + 0.24, tc + 0.42));
  p.fx = 1 - seg(t, tc, tc + 0.2);
  return p;
}

// ── Effects (seeded, so every launch looks the same) ─────────────────────────

interface Spark { a: number; sp: number; t0: number; life: number; s: number; drift: number }

function sparks(seed: number, n: number, t0: number, spread: number, up: number): Spark[] {
  const rnd = seeded(seed);
  return Array.from({ length: n }, () => ({
    a: -Math.PI / 2 + (rnd() - 0.5) * spread,
    sp: up * (0.6 + rnd() * 0.8),
    t0: t0 + rnd() * 0.2,
    life: 0.5 + rnd() * 0.5,
    s: 1 + rnd() * 1.4,
    drift: (rnd() - 0.5) * 30,
  }));
}

/** Embers kicked up by the landing. */
const LANDING = sparks(11, 16, T.land, 2.6, 60);
/** Sparkles round Zuko while he looks pleased. */
const PROUD = (() => {
  const rnd = seeded(23);
  return Array.from({ length: 5 }, (_, i) => ({
    x: C0.x + (rnd() - 0.5) * 120, y: C0.y - 30 - rnd() * 34, t0: T.happy0 + i * 0.09, s: 3 + rnd() * 2.5,
  }));
})();

/** Total length of the card outline. */
const PERIM = 2 * (CARD.w - 2 * CARD_R) + 2 * (CARD.h - 2 * CARD_R) + TAU * CARD_R;

/**
 * A point on the card outline `s` along it, clockwise from the top-left
 * corner's end, with the outward normal.
 */
function cardPoint(s: number): { x: number; y: number; nx: number; ny: number } {
  const r = CARD_R;
  const w = CARD.w - 2 * r;
  const h = CARD.h - 2 * r;
  const q = (Math.PI * r) / 2;
  let d = ((s % PERIM) + PERIM) % PERIM;
  const L = CARD.x;
  const Tp = CARD.y;
  const Rt = CARD.x + CARD.w;
  const B = CARD.y + CARD.h;
  const corner = (cx: number, cy: number, a0: number, k: number) => {
    const a = a0 + (k / q) * (Math.PI / 2);
    return { x: cx + Math.cos(a) * r, y: cy + Math.sin(a) * r, nx: Math.cos(a), ny: Math.sin(a) };
  };
  if (d < w) return { x: L + r + d, y: Tp, nx: 0, ny: -1 };
  d -= w;
  if (d < q) return corner(Rt - r, Tp + r, -Math.PI / 2, d);
  d -= q;
  if (d < h) return { x: Rt, y: Tp + r + d, nx: 1, ny: 0 };
  d -= h;
  if (d < q) return corner(Rt - r, B - r, 0, d);
  d -= q;
  if (d < w) return { x: Rt - r - d, y: B, nx: 0, ny: 1 };
  d -= w;
  if (d < q) return corner(L + r, B - r, Math.PI / 2, d);
  d -= q;
  if (d < h) return { x: L, y: B - r - d, nx: -1, ny: 0 };
  d -= h;
  return corner(L + r, Tp + r, Math.PI, d);
}

/** Where on the outline the fireball lands (the middle of the right edge). */
const S_IMPACT = (CARD.w - 2 * CARD_R) + (Math.PI * CARD_R) / 2 + (CARD.h / 2 - CARD_R);
/** How long the flame fronts take to run round to the far side. */
const IGNITE_DUR = 0.75;

function drawIgnition(x: CanvasRenderingContext2D, t: number, alpha: number) {
  const k = seg(t, T.impact + 0.05, T.impact + 0.05 + IGNITE_DUR);
  if (k <= 0 || alpha <= 0.01) return;
  const run = E.out(k) * (PERIM / 2);
  const fade = (1 - seg(t, T.impact + 0.6, T.impact + 1.9)) * alpha;
  if (fade <= 0.01) return;

  // The burning edge: the outline from the impact point out to each front.
  x.save();
  x.beginPath();
  x.roundRect(CARD.x, CARD.y, CARD.w, CARD.h, CARD_R);
  x.lineCap = "round";
  for (const [lw, c, a] of [[7, PAL.base, 0.22], [3, PAL.mid, 0.75], [1.2, PAL.tip, 0.95]] as const) {
    x.lineWidth = lw;
    x.strokeStyle = rgba(c, a * fade);
    for (const dir of [1, -1]) {
      x.setLineDash([run, PERIM]);
      x.lineDashOffset = dir > 0 ? -S_IMPACT : -(S_IMPACT - run);
      x.stroke();
    }
  }
  x.restore();

  // Flames riding the two fronts.
  if (k < 1) {
    for (const dir of [1, -1]) {
      const p = cardPoint(S_IMPACT + dir * run);
      const ang = Math.atan2(p.nx, -p.ny);
      glow(x, p.x, p.y, 16, PAL.mid, 0.5 * fade);
      flame(x, p.x, p.y, ang * 0.6, 8, 15 * (1 - k * 0.4), t + 3.1, PAL, dir * 4, fade);
    }
  }
  // A warm wash inside the island as it catches.
  const wash = Math.sin(Math.PI * clamp(k * 1.2, 0, 1)) * 0.16 * fade;
  if (wash > 0.005) {
    x.save();
    x.beginPath();
    x.roundRect(CARD.x, CARD.y, CARD.w, CARD.h, CARD_R);
    x.clip();
    const g = x.createRadialGradient(TARGET.x, TARGET.y, 10, TARGET.x, TARGET.y, 520);
    g.addColorStop(0, rgba(PAL.mid, wash * 1.6));
    g.addColorStop(1, rgba(PAL.base, 0));
    x.fillStyle = g;
    x.fillRect(CARD.x, CARD.y, CARD.w, CARD.h);
    x.restore();
  }
}

/**
 * The swirl Zuko drops into, spinning down as he lands: the far half is drawn
 * behind him, the near half in front.
 */
function drawSwirl(x: CanvasRenderingContext2D, t: number, alpha: number, front: boolean) {
  if (alpha <= 0.01) return;
  for (const [k0, r, sq, oy, sd] of [[0, 1.75, 0.34, 0.35, 0], [0.08, 1.25, 0.4, -0.15, 2]] as const) {
    const k = seg(t, k0, T.land + 0.42);
    if (k <= 0 || k >= 1) continue;
    const cy = C0.y + R_FULL * (oy + 0.25 * (1 - k));
    x.save();
    x.beginPath();
    if (front) x.rect(0, cy, 640, 150);
    else x.rect(0, 0, 640, cy);
    x.clip();
    x.globalAlpha = alpha;
    swirl(x, C0.x, cy, R_FULL * r, k, t + 3.1, PAL, sq, 0, sd);
    x.restore();
  }
}

function drawEffects(x: CanvasRenderingContext2D, t: number, p: Pose, bot: BotEngine) {
  const alpha = p.fx * Math.max(p.card, p.fx < 1 ? 1 : 0);
  if (alpha <= 0.01) return;
  const tt = t + 3.1;

  drawSwirl(x, t, alpha, true);

  // Landing: a flat ring of fire spreading out, and embers kicked up.
  const lk = seg(t, T.land, T.land + 0.6);
  if (lk > 0 && lk < 1) {
    const rx = lerp(R_FULL * 0.8, 200, E.out(lk));
    x.strokeStyle = rgba(PAL.mid, 0.6 * Math.pow(1 - lk, 1.5) * alpha);
    x.lineWidth = lerp(3, 1, lk);
    x.beginPath();
    x.ellipse(C0.x, C0.y + R_FULL * 1.15, rx, rx * 0.16, 0, 0, TAU);
    x.stroke();
  }
  const fy = C0.y + R_FULL;
  for (const s of LANDING) {
    const k = (t - s.t0) / s.life;
    if (k <= 0 || k >= 1) continue;
    const d = s.sp * E.out(k);
    ember(x, C0.x + Math.cos(s.a) * d * 1.6 + s.drift * k * k, fy + Math.sin(s.a) * d + 22 * k * k, s.s, k, PAL);
  }

  // The punch: the trail into the fist, the fireball, its embers, the burst.
  const f = bot.fistPosition(1);
  const tk = seg(t, T.wind + 0.06, T.wind + 0.75);
  if (f && tk > 0 && tk < 1) {
    x.save();
    x.globalAlpha = alpha;
    punchTrail(x, f.x, f.y, f.dx, f.dy, R_FULL * 0.74, tk, tt, PAL, 5);
    x.restore();
  }
  const from = f && t < T.hit + 0.4 ? { x: f.x + f.dx * f.r, y: f.y + f.dy * f.r } : { x: C0.x + R_FULL * 1.7, y: C0.y + R_FULL * 0.36 };
  const fl = T.impact - T.launch;
  const s = 7.5;
  const at = (k: number): [number, number] => [
    lerp(from.x, TARGET.x, k), lerp(from.y, TARGET.y, k) - Math.sin(Math.PI * k) * 14,
  ];
  const rnd = seeded(77);
  for (let i = 0; i < 26; i++) {
    const kk = rnd();
    const life = 0.3 + rnd() * 0.35;
    const vx = (rnd() - 0.5) * 30;
    const vy = -14 - rnd() * 26;
    const r = 1.4 + rnd() * 2;
    const age = (t - (T.launch + kk * fl)) / life;
    if (age <= 0 || age >= 1) continue;
    const [px, py] = at(kk);
    ember(x, px + vx * age * life, py + vy * age * life, r, age, PAL);
  }
  const k = seg(t, T.launch, T.impact);
  if (k > 0 && k < 1) {
    const [px, py] = at(k);
    const [qx, qy] = at(Math.min(1, k + 0.03));
    fireball(x, px, py, Math.atan2(qy - py, qx - px), s * (0.7 + 0.3 * Math.min(1, k * 5)), tt, PAL, 1, 9);
  }
  const bk = seg(t, T.impact, T.impact + 0.55);
  if (bk > 0 && bk < 1) burst(x, TARGET.x, TARGET.y, bk, s * 1.5, tt, PAL, 9);

  // Sparkles while he looks pleased.
  for (const sp of PROUD) {
    const k2 = seg(t, sp.t0, sp.t0 + 0.7);
    if (k2 <= 0 || k2 >= 1) continue;
    const a = Math.sin(Math.PI * k2) * alpha;
    const r = sp.s * (0.6 + 0.4 * Math.sin(Math.PI * k2));
    x.save();
    x.translate(sp.x, sp.y - k2 * 6);
    x.rotate(k2 * 1.2);
    x.fillStyle = rgba(hexToRGB("#FFD166"), a);
    x.beginPath();
    x.moveTo(0, -r);
    x.quadraticCurveTo(r * 0.2, -r * 0.2, r, 0);
    x.quadraticCurveTo(r * 0.2, r * 0.2, 0, r);
    x.quadraticCurveTo(-r * 0.2, r * 0.2, -r, 0);
    x.quadraticCurveTo(-r * 0.2, -r * 0.2, 0, -r);
    x.fill();
    x.restore();
  }
}

const MINI_COLORS = ["#22C55E", "#3E86E0", "#EAB308", "#A78BFA"];

// ── Controller ────────────────────────────────────────────────────────────────

/**
 * Runs the greeting animation on its own canvas. `onComplete` fires once at
 * T.end (or right after the collapse when interrupted) so the FSM can move on.
 */
export class Greeting {
  private startMs = 0;
  private tc = Number.POSITIVE_INFINITY;
  private fired = false;
  private timers: number[] = [];
  private bot = new BotEngine();
  private minis: BotEngine[] = MINI_COLORS.map((c, i) => {
    const m = new BotEngine();
    m.isMini = true;
    m.bodyColor = hexToRGB(c);
    m.flickerSeed = i * 1.7;
    return m;
  });

  onComplete: (() => void) | null = null;

  constructor() {
    this.bot.flickerSeed = 3.1;
    this.bot.snapToState();
  }

  start() {
    this.startMs = performance.now();
    this.tc = Number.POSITIVE_INFINITY;
    this.fired = false;
    this.cancelTimers();
    this.timers.push(
      window.setTimeout(() => Sound.play("greet"), 30),
      window.setTimeout(() => Sound.play("fire"), T.wind * 1000),
      window.setTimeout(() => Sound.play("fireball"), T.launch * 1000),
      window.setTimeout(() => Sound.play("burst"), T.impact * 1000),
      window.setTimeout(() => this.fire(), (T.end + 0.05) * 1000),
    );
  }

  /** Mouse entered the island during the greeting — hold it open. */
  hover() {
    if (this.tc >= T.autoLeave) this.tc = Number.POSITIVE_INFINITY;
  }

  /** Mouse left — collapse from now. */
  interrupt() {
    const t = (performance.now() - this.startMs) / 1000;
    if (!Number.isFinite(this.tc) || this.tc > t) this.tc = t;
    this.cancelTimers();
  }

  get elapsed(): number {
    return (performance.now() - this.startMs) / 1000;
  }

  get done(): boolean {
    return this.fired;
  }

  private fire() {
    if (this.fired) return;
    this.fired = true;
    this.cancelTimers();
    this.onComplete?.();
  }

  private cancelTimers() {
    this.timers.forEach((id) => window.clearTimeout(id));
    this.timers = [];
  }

  draw(x: CanvasRenderingContext2D) {
    const t = this.elapsed;
    if (!this.fired && t >= T.end && this.tc >= T.autoLeave) this.fire();
    this.drawFrame(x, t, this.tc);
  }

  /**
   * Draws the frame at `t` seconds into the greeting, collapsing from `tc`
   * (Infinity = not collapsing). Pure: previews call it with fixed times.
   */
  drawFrame(x: CanvasRenderingContext2D, t: number, tc = Number.POSITIVE_INFINITY) {
    const p = pose(t, tc);
    x.clearRect(0, 0, 640, 150);

    if (p.card > 0) {
      x.save();
      x.globalAlpha = p.card;
      x.beginPath();
      x.roundRect(CARD.x, CARD.y, CARD.w, CARD.h, CARD_R);
      x.fillStyle = "#121318";
      x.fill();
      x.restore();
      drawIgnition(x, t, p.card * p.fx);
    }

    this.drawMinis(x, p.minis, t);
    drawSwirl(x, t, p.fx * Math.max(p.card, p.fx < 1 ? 1 : 0), false);
    this.drawZuko(x, p, t);
    // After Zuko, so the fist and the fire sit on top of him.
    if (p.fx > 0.01) {
      x.save();
      x.beginPath();
      x.rect(0, 0, 640, 150);
      x.clip();
      drawEffects(x, t, p, this.bot);
      this.bot.drawFx(x);
      x.restore();
    }
  }

  private drawZuko(x: CanvasRenderingContext2D, p: Pose, t: number) {
    if (p.R <= 0.6) return;
    const col: RGB = hexToRGB("#FFA63D");

    // A warm halo once he has landed.
    if (p.halo > 0.01) {
      const lit = p.halo * (0.35 + 0.65 * p.boot);
      for (const [rad, a] of [[p.R * 2.4, 0.14], [p.R * 4, 0.05]] as const) {
        const g = x.createRadialGradient(p.x, p.y, 0, p.x, p.y, rad);
        g.addColorStop(0, rgba(col, a * lit));
        g.addColorStop(1, rgba(col, 0));
        x.fillStyle = g;
        x.beginPath();
        x.arc(p.x, p.y, rad, 0, Math.PI * 2);
        x.fill();
      }
    }

    const b = this.bot;
    b.clock = t;
    b.col = b.colT = col;
    b.tint = 0.5;
    b.glow = 1;
    b.boot = p.boot;
    b.ignite = 1;
    b.sx = p.sx;
    b.sy = p.sy;
    b.tilt = p.tilt;
    b.es = p.es;
    b.open = p.open;
    b.yaw = p.lookX * 0.62;
    b.pitch = p.lookY * 0.5;
    b.eyeOverride = p.eye;
    b.punch = p.punch;
    b.punchAngle = -0.12;
    b.fistFire = p.fist;
    b.badge = null;
    b.badgeS = 0;
    b.drawAt(x, p.x, p.y, p.R);
  }

  /** The compact island's mini bots popping in as Zuko lands in its spot. */
  private drawMinis(x: CanvasRenderingContext2D, alpha: number, t: number) {
    if (alpha <= 0.01) return;
    const cx = 320 + COMPACT_W / 2 - 27;
    const cy = 16;
    const sp = 6;
    const offsets: [number, number][] = [[-sp, -sp], [sp, -sp], [-sp, sp], [sp, sp]];
    offsets.forEach(([dx, dy], i) => {
      const m = this.minis[i];
      m.clock = t;
      m.drawAt(x, cx + dx, cy + dy - 0.5, 4.6 * alpha);
    });
  }
}
