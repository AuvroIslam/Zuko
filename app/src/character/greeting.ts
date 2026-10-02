// The launch greeting: Zuko rises into the opened island, its visor boots with
// a left → right scan line, the LEDs light up, the ember flame ignites, it has
// a quick look around, then it shrinks into its spot in the compact island.
//
// One renderer: the greeting poses a BotEngine (fields + `clock`) every frame
// and calls `drawAt()`. Everything is laid out in a 640×150 reference space,
// the size of the expanded island while the greeting plays.

import { Sound } from "../core/sound";
import { COMPACT_W } from "../core/layout";
import { BotEngine, LED, hexToRGB, type EyeShape, type RGB } from "./engine";

// ── Timing (seconds) ──────────────────────────────────────────────────────────

const T = {
  rise: 0.45,
  boot0: 0.6,
  boot1: 1.2,
  dip0: 1.25,
  ignite: 1.36,
  pop1: 1.52,
  happy0: 1.62,
  happy1: 2.3,
  content0: 2.45,
  content1: 2.6,
  badge: 2.72,
  down0: 2.85,
  down1: 3.2,
  blink2: 3.8,
  tint0: 3.85,
  tint1: 4.15,
  end: 4.6,
  autoLeave: 4.9,
  COLLAPSE: 0.34,
};

export const GREETING_END = T.end;

// ── Geometry (640×150) ────────────────────────────────────────────────────────

const C0 = { x: 320, y: 92 };
/** Zuko's half width at full size; the body is 2R across. */
const R_FULL = 31;
/** Where the compact island draws its bot: botPosition("compact"), diameter 20. */
const EAR = { x: 320 - COMPACT_W / 2 + 40, y: 16, R: 10 };
const CARD = { x: 10, y: 36, w: 620, h: 104 };
const CARD_R = 20;

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
const mix3 = (a: RGB, b: RGB, t: number): RGB => [lerp(a[0], b[0], t), lerp(a[1], b[1], t), lerp(a[2], b[2], t)];
const rgba = (c: RGB, a: number) =>
  `rgba(${Math.round(c[0] * 255)},${Math.round(c[1] * 255)},${Math.round(c[2] * 255)},${clamp(a, 0, 1)})`;

// ── Pose ──────────────────────────────────────────────────────────────────────

interface Pose {
  R: number; x: number; y: number; sx: number; sy: number; tilt: number;
  eye: EyeShape; open: number; es: number;
  lookX: number; lookY: number;
  /** Visor boot 0…1, flame ignition 0…1(+overshoot), flame flare 0…1. */
  boot: number; ignite: number; flare: number;
  /** LED colour: 1 = intro blue, 0 = idle teal. */
  blue: number;
  badge: number; halo: number; minis: number; fx: number; card: number;
}

function blinkAt(t: number, tb: number): number {
  const k = seg(t, tb, tb + 0.12);
  return k > 0 && k < 1 ? 1 - Math.sin(Math.PI * k) * 0.94 : 1;
}

function greetPose(t: number): Pose {
  const gg = E.back(seg(t, 0.02, T.rise));
  const R = lerp(2, R_FULL, gg);
  const x = C0.x;
  let y = lerp(16, C0.y, E.out(seg(t, 0.02, T.rise)));
  let sx = 1;
  let sy = 1;
  let tilt = 0;

  // Anticipation dip, then the pop as the flame catches.
  if (t >= T.dip0 && t < T.pop1) {
    const k = Math.sin(Math.PI * seg(t, T.dip0, T.pop1));
    const up = seg(t, T.ignite, T.pop1);
    y += R * 0.2 * k * (1 - up) - R * 0.12 * k * up;
    sy = 1 - 0.07 * k * (1 - up) + 0.05 * k * up;
    sx = 1 + 0.05 * k * (1 - up) - 0.03 * k * up;
  }
  // A small proud sway while it looks around.
  if (t >= T.pop1 && t < T.content1) {
    const w = t - T.pop1;
    const fade = 1 - seg(t, T.content0, T.content1);
    tilt = Math.sin(w * 2 * Math.PI * 0.9 + 0.6) * 0.045 * fade;
    y += Math.sin(w * 2 * Math.PI * 1.8) * 0.8 * fade;
  }
  if (t >= T.content0 && t < T.down1) {
    y += R * 0.1 * Math.sin(Math.PI * seg(t, T.content0, T.down1));
  }

  let eye: EyeShape = "pill";
  if (t >= T.happy0 && t < T.happy1) eye = "happy";
  if (t >= T.content0 && t < T.content1) eye = "closed";
  if (t >= T.down0 && t < T.down1) eye = "closed";

  // Eyes snap a little wide as they power on.
  const es = 1 + 0.18 * Math.sin(Math.PI * seg(t, T.boot1 - 0.15, T.boot1 + 0.25));
  const open = Math.min(blinkAt(t, 2.42), blinkAt(t, T.blink2));

  let lookX = 0;
  let lookY = 0;
  if (t >= T.boot1 && t < T.dip0) lookY = -0.15;
  if (t >= T.pop1 && t < T.content0) {
    const k = E.inOut(seg(t, T.pop1, T.pop1 + 0.25));
    const back = E.inOut(seg(t, 2.05, 2.3));
    lookX = lerp(0, 0.75, k) * (1 - back) + -0.7 * back;
    lookY = lerp(0, -0.35, k) * (1 - back) + 0.25 * back;
  }
  if (t >= T.content0) {
    const k = E.inOut(seg(t, T.down1, T.down1 + 0.35));
    lookX = lerp(-0.7, 0, k);
    lookY = lerp(0.25, 0, k);
  }

  return {
    R, x, y, sx, sy, tilt,
    eye, open, es,
    lookX, lookY,
    boot: E.inOut(seg(t, T.boot0, T.boot1)),
    ignite: E.back(seg(t, T.ignite, T.ignite + 0.3)),
    flare: t < T.ignite ? 0 : E.out(seg(t, T.ignite, T.ignite + 0.12)) * (1 - E.inOut(seg(t, T.ignite + 0.12, T.ignite + 1.1))),
    blue: 1 - E.inOut(seg(t, T.tint0, T.tint1)),
    badge: E.back(seg(t, T.badge, T.badge + 0.28)) * (1 - E.inOut(seg(t, T.tint0, T.tint1))),
    halo: E.out(seg(t, 0.3, 0.7)),
    minis: 0,
    fx: 1,
    card: seg(t, 0.18, 0.45),
  };
}

function smallPose(): Pose {
  return {
    R: EAR.R, x: EAR.x, y: EAR.y,
    sx: 1, sy: 1, tilt: 0,
    eye: "pill", open: 1, es: 1,
    lookX: 0, lookY: 0,
    boot: 1, ignite: 1, flare: 0, blue: 0,
    badge: 0, halo: 0.6, minis: 1, fx: 1, card: 0,
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
  p.badge = lerp(a.badge, b.badge, e);
  p.blue = lerp(a.blue, b.blue, e);
  p.halo = lerp(a.halo, b.halo, e);
  // Cut short mid-boot: finish booting on the way down.
  p.boot = lerp(a.boot, 1, e);
  p.ignite = lerp(a.ignite, 1, e);
  p.flare = a.flare * (1 - e);
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

const SPARKS: Spark[] = (() => {
  let seed = 11;
  const rnd = () => {
    seed = (Math.imul(seed, 1103515245) + 12345) & 0x7fffffff;
    return seed / 0x7fffffff;
  };
  return Array.from({ length: 14 }, () => ({
    a: -Math.PI / 2 + (rnd() - 0.5) * 1.5,
    sp: 38 + rnd() * 46,
    t0: T.ignite + rnd() * 0.35,
    life: 0.55 + rnd() * 0.5,
    s: 0.9 + rnd() * 1.1,
    drift: (rnd() - 0.5) * 30,
  }));
})();

/** Shield pulses: thin rings that leave Zuko as the visor boots and the flame catches. */
const PULSES = [
  { t0: T.boot0 + 0.05, col: LED.blue, a: 0.55 },
  { t0: T.boot1 - 0.05, col: LED.blue, a: 0.45 },
  { t0: T.ignite, col: hexToRGB("#FF9A3C"), a: 0.5 },
];

const EMBER_HOT = hexToRGB("#FFD166");
const EMBER = hexToRGB("#FF7A1A");

function drawEffects(x: CanvasRenderingContext2D, t: number, p: Pose) {
  const alpha = p.fx * Math.max(p.card, p.fx < 1 ? 1 : 0);
  if (alpha <= 0.01) return;

  // A faint card-wide scan line echoing the visor boot.
  const sweep = seg(t, T.boot0, T.boot1);
  if (sweep > 0 && sweep < 1) {
    const sxp = lerp(CARD.x, CARD.x + CARD.w, E.inOut(sweep));
    const fade = Math.sin(Math.PI * sweep) * alpha;
    const g = x.createLinearGradient(sxp - 120, 0, sxp, 0);
    g.addColorStop(0, rgba(LED.blue, 0));
    g.addColorStop(1, rgba(LED.blue, 0.1 * fade));
    x.fillStyle = g;
    x.fillRect(sxp - 120, CARD.y, 120, CARD.h);
    x.fillStyle = rgba(LED.blue, 0.35 * fade);
    x.fillRect(sxp - 0.75, CARD.y, 1.5, CARD.h);
  }

  for (const pu of PULSES) {
    const k = seg(t, pu.t0, pu.t0 + 1.1);
    if (k <= 0 || k >= 1) continue;
    const rx = lerp(R_FULL * 0.9, 360, E.out(k));
    const a = pu.a * Math.pow(1 - k, 1.6) * alpha;
    x.strokeStyle = rgba(pu.col, a);
    x.lineWidth = lerp(2, 1, k);
    x.beginPath();
    x.ellipse(C0.x, C0.y, rx, rx * 0.36, 0, 0, Math.PI * 2);
    x.stroke();
  }

  // Embers thrown up as the flame ignites.
  const fy = C0.y - R_FULL * 0.95;
  for (const s of SPARKS) {
    const k = (t - s.t0) / s.life;
    if (k <= 0 || k >= 1) continue;
    const d = s.sp * E.out(k);
    const px = C0.x + Math.cos(s.a) * d + s.drift * k * k;
    const py = fy + Math.sin(s.a) * d + 18 * k * k;
    x.fillStyle = rgba(mix3(EMBER_HOT, EMBER, k), (1 - k) * alpha);
    x.beginPath();
    x.arc(px, py, s.s * (1 - k * 0.5), 0, Math.PI * 2);
    x.fill();
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
      window.setTimeout(() => Sound.play("greet"), T.boot0 * 1000),
      window.setTimeout(() => Sound.play("blip"), T.badge * 1000),
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

      x.save();
      x.beginPath();
      x.roundRect(CARD.x, CARD.y, CARD.w, CARD.h, CARD_R);
      x.clip();
      drawEffects(x, t, p);
      x.restore();
    } else if (Number.isFinite(tc) && t >= tc) {
      drawEffects(x, t, p);
    }

    this.drawMinis(x, p.minis, t);
    this.drawZuko(x, p, t);
  }

  private drawZuko(x: CanvasRenderingContext2D, p: Pose, t: number) {
    if (p.R <= 0.6) return;
    const col = mix3(LED.teal, LED.blue, p.blue);

    // Soft halo in the LED colour, warming as the flame catches.
    if (p.halo > 0.01) {
      const lit = p.halo * (0.35 + 0.65 * p.boot);
      for (const [rad, a] of [[p.R * 2.4, 0.16], [p.R * 4, 0.06]] as const) {
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
    b.flameLevel = 0.85;
    b.boot = p.boot;
    b.ignite = p.ignite;
    b.flare = p.flare;
    b.sx = p.sx;
    b.sy = p.sy;
    b.tilt = p.tilt;
    b.es = p.es;
    b.open = p.open;
    b.yaw = p.lookX * 0.62;
    b.pitch = p.lookY * 0.5;
    b.eyeOverride = p.eye;
    b.badge = p.badge > 0.01 ? { kind: "dots", color: col } : null;
    b.badgeS = p.badge;
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
