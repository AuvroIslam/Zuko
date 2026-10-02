// Zuko — the guardian character, drawn with Canvas 2D.
//
// Zuko is a small shield-shaped guardian: a deep navy body, a dark glass visor
// holding two LED eyes that glow in the state colour, and an ember flame tuft
// that flickers on top of its head. Every expression is an LED glyph inside the
// visor. The tween, emote and particle machinery comes from the MIT-licensed
// BotEngine port the app started from; everything that is drawn is Zuko's own.
//
// One renderer serves the whole app: the island bot and the mini bots tick a
// BotEngine, while the launch greeting and the drop sequence pose one directly
// (fields + `clock`) and call `drawAt()`.

import { Ease, clamp, lerp, type EaseFn } from "../core/anim";
import { Sound } from "../core/sound";
import type { BotEmoteName, BotStateName } from "../core/layout";

// ── Types ─────────────────────────────────────────────────────────────────────

export type EyeShape =
  | "pill" | "wide" | "dot" | "line" | "flat" | "happy" | "closed"
  | "spiral" | "heart" | "star" | "tired" | "wink" | "cup";

export type BadgeKind = "dots" | "bang" | "question" | "dot";

export interface Badge {
  kind: BadgeKind;
  color: RGB;
}

export type RGB = readonly [number, number, number]; // components 0…1

export type TweenKey = readonly [target: number, durationMs: number, ease: EaseFn];

/** Which palette the flame burns with. */
export type FireKind = "ember" | "danger" | "success";

interface Tween {
  prop: PropKey;
  keys: TweenKey[];
  index: number;
  from: number;
  startMs: number;
  onComplete?: () => void;
}

type PropKey =
  | "yaw" | "pitch" | "roll" | "tilt" | "open" | "sx" | "sy" | "oy" | "ox"
  | "tint" | "morph" | "boost" | "flare" | "boot" | "es" | "badgeS";

interface BotStateCfg {
  color: RGB;
  /** How much LED light spills onto the body and tints the rim, 0…1. */
  tint: number;
  eye: EyeShape;
  badge: Badge | null;
  bounces: boolean;
  scans: boolean;
  breathes: boolean;
  zz: boolean;
  sweat: boolean;
  look: readonly [number, number] | null;
  tilt: number;
  /** LED brightness, 0…1. */
  glow: number;
  /** Resting flame size; 1 is a normal flame. */
  flame: number;
  fire: FireKind;
}

interface Particle {
  type: "heart" | "star" | "spark" | "sweat" | "z";
  x: number; y: number; vx: number; vy: number;
  age: number; life: number; rot: number; size: number;
}

/** Face metrics for one draw, in canvas units. */
interface Face {
  /** Visor centre y, half width, height, corner radius and bottom sag. */
  vcy: number; vhw: number; vh: number; vr: number; sag: number;
  /** Eye bar width, height and centre offset from the middle. */
  ew: number; eh: number; esp: number;
}

// ── Palette ───────────────────────────────────────────────────────────────────

export function hexToRGB(hex: string): RGB {
  const h = hex.replace("#", "");
  const v = parseInt(h, 16);
  return [((v >> 16) & 255) / 255, ((v >> 8) & 255) / 255, (v & 255) / 255];
}

/** LED colours. The first five are the brand states; the rest fill in. */
export const LED = {
  teal: hexToRGB("#2EE6C5"), // idle, working
  amber: hexToRGB("#FFB020"), // attention, approval
  red: hexToRGB("#FF4D5E"), // danger, error
  green: hexToRGB("#4ADE80"), // success, finished
  blue: hexToRGB("#60A5FA"), // thinking, intro
  sky: hexToRGB("#38BDF8"), // searching
  orange: hexToRGB("#FF9A3C"), // rate limited
  violet: hexToRGB("#A78BFA"), // dizzy
} as const;

const BODY_TOP = hexToRGB("#2A3350");
const BODY_BOTTOM = hexToRGB("#121829");
const RIM: RGB = [0.66, 0.76, 1];
const WHITE: RGB = [1, 1, 1];
const BLACK: RGB = [0, 0, 0];
const HEART = hexToRGB("#FF5C7A");
const STAR = hexToRGB("#FFD166");

interface FirePalette { root: RGB; base: RGB; tip: RGB; coreBase: RGB; coreTip: RGB }

/** Ember is the resting flame: orange at the root, yellow at the tips. */
const FIRE: Record<FireKind, FirePalette> = {
  ember: {
    root: hexToRGB("#F0520F"), base: hexToRGB("#FF7A1A"), tip: hexToRGB("#FFD166"),
    coreBase: hexToRGB("#FFAE3D"), coreTip: hexToRGB("#FFF3C8"),
  },
  danger: {
    root: hexToRGB("#D9231C"), base: hexToRGB("#FF3D2E"), tip: hexToRGB("#FF9A3C"),
    coreBase: hexToRGB("#FF6B3A"), coreTip: hexToRGB("#FFD2A0"),
  },
  success: {
    root: hexToRGB("#FF7A1A"), base: hexToRGB("#FFA030"), tip: hexToRGB("#FFF1A6"),
    coreBase: hexToRGB("#FFD166"), coreTip: hexToRGB("#FFFFF2"),
  },
};

// ── Geometry (units of R, half the nominal body diameter) ─────────────────────

/**
 * The shield outline: a flat-ish top with generous corners, upright flanks that
 * taper to a softly rounded point. scripts/gen-icons.mjs mirrors these numbers.
 */
export const SHIELD = {
  halfW: 1.0,
  top: -0.86,
  corner: 0.36,
  bulge: 0.035,
  /** y of the first flank control point: keeps the sides upright for a while. */
  flankY: 0.24,
  /** Second flank control point, where the taper turns into the point. */
  taperX: 0.58,
  taperY: 0.74,
  /** Half width where the rounded point begins, and its lowest y. */
  tipHalf: 0.16,
  tipY: 0.98,
} as const;

/** Shield parameters at full `morph`: a rounded badge base, for the scan stance. */
const SHIELD_BOXED = { taperX: 0.94, taperY: 0.6, tipHalf: 0.62, tipY: 0.86 } as const;

/**
 * Zuko's body outline, centred on the origin, `R` = half its width. `morph`
 * broadens the point into a rounded base (the drag-over scan stance).
 */
export function shieldPath(R: number, morph = 0): Path2D {
  const m = clamp(morph, 0, 1);
  const a = SHIELD.halfW * R;
  const top = SHIELD.top * R;
  const rc = SHIELD.corner * R;
  const bulge = SHIELD.bulge * R;
  const flankY = SHIELD.flankY * R;
  const taperX = lerp(SHIELD.taperX, SHIELD_BOXED.taperX, m) * R;
  const taperY = lerp(SHIELD.taperY, SHIELD_BOXED.taperY, m) * R;
  const tipHalf = lerp(SHIELD.tipHalf, SHIELD_BOXED.tipHalf, m) * R;
  const tipY = lerp(SHIELD.tipY, SHIELD_BOXED.tipY, m) * R;
  // Where the flank hands over to the point, chosen so the two curves meet
  // with the same tangent (no kink at the tip).
  const d = ((tipY - taperY) * tipHalf) / (2 * taperX - tipHalf);
  const tipStartY = tipY - d;
  const tipCtlY = tipY + d;

  // The top is a shallow arch; the corner leaves along the arch's own tangent.
  const ex = a - rc;
  const k = rc * 0.55;
  const tx = 0.6 * ex;
  const ty = 1.33 * bulge;
  const tl = Math.hypot(tx, ty) || 1;

  const p = new Path2D();
  p.moveTo(-ex, top);
  p.bezierCurveTo(-0.4 * ex, top - ty, 0.4 * ex, top - ty, ex, top);
  p.bezierCurveTo(ex + (tx / tl) * k, top + (ty / tl) * k, a, top + rc - k, a, top + rc);
  p.bezierCurveTo(a, flankY, taperX, taperY, tipHalf, tipStartY);
  p.quadraticCurveTo(0, tipCtlY, -tipHalf, tipStartY);
  p.bezierCurveTo(-taperX, taperY, -a, flankY, -a, top + rc);
  p.bezierCurveTo(-a, top + rc - k, -ex - (tx / tl) * k, top + (ty / tl) * k, -ex, top);
  p.closePath();
  return p;
}

function visorPath(f: Face): Path2D {
  const l = -f.vhw;
  const r = f.vhw;
  const t = f.vcy - f.vh / 2;
  const b = f.vcy + f.vh / 2;
  const rad = f.vr;
  const p = new Path2D();
  p.moveTo(l + rad, t);
  p.lineTo(r - rad, t);
  p.quadraticCurveTo(r, t, r, t + rad);
  p.lineTo(r, b - rad);
  p.quadraticCurveTo(r, b, r - rad, b);
  p.quadraticCurveTo(0, b + 2 * f.sag, l + rad, b);
  p.quadraticCurveTo(l, b, l, b - rad);
  p.lineTo(l, t + rad);
  p.quadraticCurveTo(l, t, l + rad, t);
  p.closePath();
  return p;
}

interface Tongue { dx: number; w: number; h: number; f: number; ph: number; lean: number; core: boolean }

/** Flame tongues: two leaning flanks, the tall centre, and a hot inner core. */
const TONGUES: readonly Tongue[] = [
  { dx: -0.34, w: 0.56, h: 0.62, f: 7.3, ph: 0.4, lean: -0.28, core: false },
  { dx: 0.36, w: 0.52, h: 0.54, f: 8.1, ph: 2.2, lean: 0.3, core: false },
  { dx: 0.0, w: 0.84, h: 1.0, f: 6.1, ph: 3.7, lean: 0.02, core: false },
  { dx: 0.02, w: 0.44, h: 0.56, f: 9.4, ph: 5.1, lean: 0, core: true },
];
const MINI_TONGUES: readonly Tongue[] = [TONGUES[2], TONGUES[3]];

// ── States ────────────────────────────────────────────────────────────────────

const base = {
  bounces: false, scans: false, breathes: false, zz: false, sweat: false,
  look: null, tilt: 0, glow: 1, flame: 0.85, fire: "ember" as FireKind,
};

export const BOT_STATES: Record<BotStateName, BotStateCfg> = {
  idle: { ...base, color: LED.teal, tint: 0.35, eye: "pill", badge: null },
  working: { ...base, color: LED.teal, tint: 0.6, eye: "pill", badge: { kind: "dots", color: LED.teal }, flame: 0.95 },
  thinking: { ...base, color: LED.blue, tint: 0.6, eye: "pill", badge: { kind: "dots", color: LED.blue }, look: [0.55, 0.55] },
  searching: { ...base, color: LED.sky, tint: 0.6, eye: "pill", badge: { kind: "dots", color: LED.sky }, scans: true, flame: 0.95 },
  approval: { ...base, color: LED.amber, tint: 0.8, eye: "wide", badge: { kind: "bang", color: LED.amber }, bounces: true, flame: 1.3 },
  question: { ...base, color: LED.amber, tint: 0.7, eye: "pill", badge: { kind: "question", color: LED.amber }, tilt: 0.17, flame: 1.1 },
  error: { ...base, color: LED.red, tint: 0.8, eye: "flat", badge: { kind: "dot", color: LED.red }, flame: 1.2, fire: "danger" },
  finished: { ...base, color: LED.green, tint: 0.5, eye: "happy", badge: { kind: "dot", color: LED.green }, flame: 1.15, fire: "success" },
  ratelimit: { ...base, color: LED.orange, tint: 0.65, eye: "tired", badge: { kind: "dot", color: LED.orange }, sweat: true, flame: 0.7 },
  sleeping: { ...base, color: LED.teal, tint: 0.2, eye: "closed", badge: null, breathes: true, zz: true, glow: 0.45, flame: 0.4 },
  dizzy: { ...base, color: LED.violet, tint: 0.6, eye: "spiral", badge: null },
};

/** State → sound, as in BotStateCfg.sound. */
export const STATE_SOUND: Partial<Record<BotStateName, string>> = {
  working: "work", thinking: "think", searching: "search", approval: "approval",
  question: "question", error: "error", finished: "finish", ratelimit: "rate",
  sleeping: "sleep", dizzy: "dizzy",
};

const EMOTE_EYE: Record<BotEmoteName, EyeShape> = {
  love: "heart", surprised: "dot", proud: "star", wink: "wink",
  yawn: "tired", happy: "happy", annoyed: "line",
};

// ── Small helpers ─────────────────────────────────────────────────────────────

const now = () => performance.now() / 1000;

const rgba = (c: RGB, a = 1) =>
  `rgba(${Math.round(c[0] * 255)},${Math.round(c[1] * 255)},${Math.round(c[2] * 255)},${clamp(a, 0, 1)})`;

const mix3 = (a: RGB, b: RGB, t: number): RGB => [
  lerp(a[0], b[0], t), lerp(a[1], b[1], t), lerp(a[2], b[2], t),
];

const smoothstep = (t: number) => {
  const k = clamp(t, 0, 1);
  return k * k * (3 - 2 * k);
};

function mixFire(a: FirePalette, b: FirePalette, t: number): FirePalette {
  if (t <= 0.001) return a;
  return {
    root: mix3(a.root, b.root, t), base: mix3(a.base, b.base, t), tip: mix3(a.tip, b.tip, t),
    coreBase: mix3(a.coreBase, b.coreBase, t), coreTip: mix3(a.coreTip, b.coreTip, t),
  };
}

function roundRectPath(x: CanvasRenderingContext2D, X: number, Y: number, W: number, H: number, R: number) {
  const r = Math.max(0, Math.min(R, W / 2, H / 2));
  x.beginPath();
  x.moveTo(X + r, Y);
  x.arcTo(X + W, Y, X + W, Y + H, r);
  x.arcTo(X + W, Y + H, X, Y + H, r);
  x.arcTo(X, Y + H, X, Y, r);
  x.arcTo(X, Y, X + W, Y, r);
  x.closePath();
}

/** A rounded bar centred on the origin. */
function barPath(x: CanvasRenderingContext2D, w: number, h: number) {
  roundRectPath(x, -w / 2, -h / 2, w, h, Math.min(w, h) / 2);
}

function heartPath(x: CanvasRenderingContext2D, s: number) {
  x.beginPath();
  x.moveTo(0, s * 0.38);
  x.bezierCurveTo(-s * 1.05, -s * 0.15, -s * 0.5, -s * 0.95, 0, -s * 0.38);
  x.bezierCurveTo(s * 0.5, -s * 0.95, s * 1.05, -s * 0.15, 0, s * 0.38);
  x.closePath();
}

function starPath(x: CanvasRenderingContext2D, ro: number, ri: number) {
  x.beginPath();
  for (let i = 0; i < 10; i++) {
    const r = i % 2 ? ri : ro;
    const a = -Math.PI / 2 + (i * Math.PI) / 5;
    x.lineTo(Math.cos(a) * r, Math.sin(a) * r);
  }
  x.closePath();
}

/** One flame tongue: a teardrop rising from a flat base at `by`. */
function tonguePath(x: CanvasRenderingContext2D, bx: number, by: number, w: number, h: number, tipX: number) {
  x.beginPath();
  x.moveTo(bx - w / 2, by);
  x.bezierCurveTo(bx - w * 0.55, by - h * 0.42, tipX - w * 0.18, by - h * 0.72, tipX, by - h);
  x.bezierCurveTo(tipX + w * 0.2, by - h * 0.7, bx + w * 0.55, by - h * 0.4, bx + w / 2, by);
  x.closePath();
}

const FONT = `system-ui, "Segoe UI Variable Text", "Segoe UI", sans-serif`;

// ── Engine ────────────────────────────────────────────────────────────────────

export class BotEngine {
  isMini = false;
  /** Solid body colour for mini bots / integration pills (null = Zuko's navy). */
  bodyColor: RGB | null = null;

  // Animated state
  yaw = 0; pitch = 0; roll = 0; tilt = 0; open = 1;
  sx = 1; sy = 1; oy = 0; ox = 0;
  tint = BOT_STATES.idle.tint; morph = 0; es = 1; badgeS = 0;
  /** Emote glow: brighter LEDs and a livelier flame (love, proud, happy). */
  boost = 0;
  /** Transient flame flare-up on alerts, 0…1. */
  flare = 0;
  /** Visor boot progress: 0 = dark glass, 1 = booted. The scan line runs left → right. */
  boot = 1;
  /** Flame ignition, 0 = unlit … 1 = burning. Multiplies the flame size. */
  ignite = 1;

  /** Smoothed resting flame size and LED brightness; both follow the state. */
  flameLevel = BOT_STATES.idle.flame;
  glow = 1;
  /** Smoothed flame palette mix towards danger (error) and success (finished). */
  dangerMix = 0;
  successMix = 0;

  /**
   * Animation clock in seconds for posed drawing (greeting, drop sequence,
   * previews). Null = the wall clock. Only the flame flicker and the looping
   * glyphs read it; tweens always run on the wall clock.
   */
  clock: number | null = null;

  // Targets
  tgYaw = 0; tgPitch = 0; tgTilt = 0; tgSy = 1; tgSx = 1; tgEs = 1;

  /** Extra canvas height above the body so hearts can fly out without clipping. */
  particleOverhang = 0;

  /**
   * Scanner intensity spring (fraction of R) for the drag-over scan stance. The
   * island drives the target; `gulp()` kicks it for the scan pulse on a drop.
   */
  slotH = 0; slotHTarget = 0; slotHVel = 0;
  /** True for a moment after a scan pulse: the LEDs show a pleased "^ ^". */
  isScanning = false;

  col: RGB = LED.teal;
  colT: RGB = LED.teal;

  state: BotStateName = "idle";
  cfg: BotStateCfg = BOT_STATES.idle;

  eyeOverride: EyeShape | null = null;
  eyeOverrideUntil = 0;
  permanentEye: EyeShape | null = null;
  permanentEmote: BotEmoteName | null = null;
  miniNextBehavior = 0;

  badge: Badge | null = null;
  private badgeKey = "none";
  private badgeToken = 0;

  private tweens = new Map<PropKey, Tween>();
  private locks = new Set<PropKey>();
  private particles: Particle[] = [];

  lookX = 0;
  lookY = 0;

  lastTime = now();
  private t0 = now() - Math.random() * 5;
  /** Per-instance phase so neighbouring mini bots never flicker in step. */
  private readonly flickerSeed = Math.random() * 100;
  private nextBlink = now() + 1.5 + Math.random() * 2;
  private greetToken = 0;
  private lastAmbient = 0;
  private slapTimes: number[] = [];
  private miniLookTarget = { x: 0, y: 0 };
  private miniLookNextTime = 0;

  /** Fired when three slaps land inside 1.7 s (→ dizzy + confused view). */
  onDizzy: (() => void) | null = null;

  // ── Public API ──────────────────────────────────────────────────────────────

  setState(next: BotStateName, force = false) {
    if (this.state === next && !force) return;
    const prev = this.state;
    this.state = next;
    this.cfg = BOT_STATES[next];
    this.colT = this.cfg.color;
    if (!this.locks.has("tint")) this.tint = this.cfg.tint;
    if (!this.locks.has("tilt")) this.tgTilt = this.cfg.tilt;
    this.setBadge(this.cfg.badge);

    switch (next) {
      case "finished":
        this.doRoll(950, 1);
        this.flareUp(0.6);
        setTimeout(() => this.emit("spark", 5), 500);
        break;
      case "error":
        this.anim("ox", [
          [0.08, 50, Ease.out], [-0.08, 70, Ease.inOut],
          [0.05, 70, Ease.inOut], [0, 90, Ease.out],
        ]);
        this.flareUp(1);
        break;
      case "approval":
        this.anim("oy", [[-0.2, 150, Ease.out], [0, 300, Ease.back]]);
        this.flareUp(1);
        break;
      case "dizzy":
        this.doRoll(1300, 2);
        break;
      case "question":
        this.blink();
        this.flareUp(0.6);
        break;
      case "ratelimit":
        this.emit("sweat", 1);
        break;
      default:
        if (prev !== "idle" || next !== "idle") this.blink();
    }
  }

  setBadge(b: Badge | null) {
    const key = b ? `${b.kind}-${b.color.join(",")}` : "none";
    if (key === this.badgeKey) return;
    this.badgeKey = key;
    const tok = ++this.badgeToken;
    this.anim("badgeS", [[0, 90, Ease.inOut]]);
    setTimeout(() => {
      if (tok !== this.badgeToken) return;
      this.badge = b;
      if (b) this.anim("badgeS", [[1, 280, Ease.back]]);
    }, 100);
  }

  /**
   * Jumps every smoothed value to the current state's resting pose, with no
   * tween in flight. For posed drawing and previews; the island never needs it.
   */
  snapToState() {
    this.tweens.clear();
    this.locks.clear();
    this.col = this.colT = this.cfg.color;
    this.tint = this.cfg.tint;
    this.tilt = this.tgTilt = this.cfg.tilt;
    this.flameLevel = this.cfg.flame;
    this.glow = this.cfg.glow;
    this.dangerMix = this.cfg.fire === "danger" ? 1 : 0;
    this.successMix = this.cfg.fire === "success" ? 1 : 0;
    this.badgeToken++;
    this.badge = this.cfg.badge;
    this.badgeKey = this.badge ? `${this.badge.kind}-${this.badge.color.join(",")}` : "none";
    this.badgeS = this.badge ? 1 : 0;
    this.roll = 0; this.open = 1; this.oy = 0; this.ox = 0;
    this.sx = this.tgSx = 1; this.sy = this.tgSy = 1;
    this.boot = 1; this.flare = 0; this.boost = 0;
  }

  blink() {
    if (this.locks.has("open")) return;
    this.anim("open", [[0.06, 70, Ease.inOut], [1, 130, Ease.out]]);
  }

  squash() {
    this.anim("sy", [[0.78, 70, Ease.out], [1.1, 130, Ease.out], [1, 170, Ease.inOut]]);
    this.anim("sx", [[1.16, 70, Ease.out], [0.95, 130, Ease.out], [1, 170, Ease.inOut]]);
  }

  /** Scan pulse on a drop: the visor beam flashes, then the LEDs go "^ ^". */
  gulp() {
    this.slotHTarget = 0.42;
    setTimeout(() => {
      this.slotHTarget = 0;
      this.isScanning = true;
      setTimeout(() => { this.isScanning = false; }, 800);
    }, 460);
    this.anim("sy", [[0.82, 80, Ease.out], [1.12, 130, Ease.out], [1, 220, Ease.back]]);
    this.anim("sx", [[1.18, 80, Ease.out], [0.94, 130, Ease.out], [1, 220, Ease.back]]);
    this.flareUp(0.8);
    this.blink();
  }

  slap() {
    this.interruptGreet();
    if (this.state === "dizzy") return;
    const t = now();
    this.slapTimes = this.slapTimes.filter((s) => t - s < 1.7);
    this.slapTimes.push(t);
    Sound.play("slap");
    this.squash();
    if (this.slapTimes.length >= 3) {
      this.slapTimes = [];
      this.onDizzy?.();
    } else {
      this.eyeOverride = "line";
      this.eyeOverrideUntil = t + 0.8;
      setTimeout(() => Sound.play("annoyed"), 60);
    }
  }

  doRoll(durationMs: number, turns: number) {
    this.roll = 0;
    this.anim("roll", [[Math.PI * 2 * turns, durationMs, Ease.inOut]], () => { this.roll = 0; });
  }

  /** Flame flare-up: jumps to `amount`, then dies back down. */
  flareUp(amount = 1) {
    this.anim("flare", [[amount, 140, Ease.out], [0, 900, Ease.inOut]]);
  }

  /**
   * Boot-up hello: the visor scan line sweeps left → right, the LEDs light as
   * it passes, then the flame flares and Zuko looks pleased.
   */
  greet() {
    const t = now();
    const tok = ++this.greetToken;
    this.boot = 0;
    this.anim("boot", [[0, 120, Ease.lin], [1, 560, Ease.inOut]]);
    this.anim("oy", [[-0.06, 220, Ease.out], [0.0, 220, Ease.back]]);
    Sound.play("greet");

    setTimeout(() => {
      if (this.greetToken !== tok) return;
      this.flareUp(1);
      this.anim("sy", [[0.95, 100, Ease.out], [1.0, 260, Ease.back]]);
      this.anim("sx", [[1.04, 100, Ease.out], [1.0, 260, Ease.back]]);
      this.eyeOverride = "happy";
      this.eyeOverrideUntil = t + 1.8;
    }, 720);
    setTimeout(() => { if (this.greetToken === tok) this.blink(); }, 1900);
  }

  interruptGreet() {
    if (this.boot >= 0.999 && !this.tweens.has("boot")) return;
    this.greetToken++;
    this.tweens.delete("boot");
    this.locks.delete("boot");
    this.boot = 1;
  }

  setPermanentEmote(emote: BotEmoteName | null) {
    this.permanentEmote = emote;
    if (emote === "wink") {
      this.miniNextBehavior = now() + 0.8 + Math.random() * 1.7;
      return;
    }
    this.permanentEye = emote ? EMOTE_EYE[emote] : null;
    if (this.permanentEye) {
      this.eyeOverride = this.permanentEye;
      this.eyeOverrideUntil = Number.POSITIVE_INFINITY;
    } else if (this.eyeOverrideUntil === Number.POSITIVE_INFINITY) {
      this.eyeOverride = null;
      this.eyeOverrideUntil = 0;
    }
    this.miniNextBehavior = now() + 0.8 + Math.random() * 1.7;
  }

  triggerEmote(emote: BotEmoteName, duration = 1.8) {
    const t = now();
    this.eyeOverride = EMOTE_EYE[emote];
    this.eyeOverrideUntil = t + duration;

    switch (emote) {
      case "love":
        this.anim("boost", [
          [1, 300, Ease.out], [1, (duration - 0.6) * 1000, Ease.lin], [0, 300, Ease.inOut],
        ]);
        this.emit("heart", 4);
        this.anim("oy", [[-0.1, 160, Ease.out], [0, 300, Ease.back]]);
        break;
      case "surprised":
        this.anim("oy", [[-0.3, 140, Ease.out], [0, 380, Ease.back]]);
        this.anim("es", [[1.25, 120, Ease.out], [1, 500, Ease.inOut]]);
        this.flareUp(0.9);
        break;
      case "proud":
        this.emit("star", 5);
        this.anim("tilt", [
          [-0.14, 220, Ease.out], [-0.14, (duration - 0.5) * 1000, Ease.lin], [0, 280, Ease.inOut],
        ]);
        this.anim("boost", [
          [0.7, 250, Ease.out], [0.7, (duration - 0.5) * 1000, Ease.lin], [0, 300, Ease.inOut],
        ]);
        this.flareUp(0.7);
        break;
      case "wink":
        this.anim("tilt", [
          [0.12, 160, Ease.out], [0.12, (duration - 0.4) * 1000, Ease.lin], [0, 240, Ease.inOut],
        ]);
        break;
      case "yawn":
        this.anim("sy", [[1.12, 500, Ease.inOut], [1, 500, Ease.inOut]]);
        this.anim("sx", [[0.94, 500, Ease.inOut], [1, 500, Ease.inOut]]);
        setTimeout(() => { this.eyeOverride = "closed"; this.emit("z", 2); }, 700);
        break;
      case "happy":
        this.anim("boost", [[0.6, 200, Ease.out], [0, 600, Ease.inOut]]);
        break;
      case "annoyed":
        this.eyeOverride = "line";
        this.eyeOverrideUntil = t + 0.8;
        setTimeout(() => Sound.play("annoyed"), 60);
        break;
    }
  }

  emit(type: Particle["type"], count: number) {
    for (let i = 0; i < count; i++) {
      const isZ = type === "z";
      this.particles.push({
        type,
        x: (Math.random() - 0.5) * 0.9 + (isZ ? 0.55 : 0),
        y: -0.7 - Math.random() * 0.2,
        vx: (Math.random() - 0.5) * 0.35 + (isZ ? 0.18 : 0),
        vy: -(0.45 + Math.random() * 0.35),
        age: -i * 0.14,
        life: 1.3 + Math.random() * 0.5,
        rot: Math.random() * Math.PI * 2,
        size: 0.15 + Math.random() * 0.08,
      });
    }
  }

  animateMorph(target: number, durationMs?: number) {
    const dur = durationMs ?? (target > 0.5 ? 550 : 650);
    this.anim("morph", [[target, dur, Ease.inOut]]);
  }

  resetMorph() {
    this.tweens.delete("morph");
    this.locks.delete("morph");
    this.morph = 0;
  }

  /** The flame is burning, so it flickers on every frame. */
  private get flameLit(): boolean {
    return this.ignite * (this.flameLevel + this.flare) > 0.02;
  }

  /** Anything other than the ambient flame flicker is still moving. */
  private get moving(): boolean {
    return (
      this.tweens.size > 0 ||
      this.particles.length > 0 ||
      this.cfg.bounces || this.cfg.scans || this.cfg.breathes || this.cfg.zz || this.cfg.sweat ||
      this.isMini ||
      Math.abs(this.tgYaw - this.yaw) > 0.002 ||
      Math.abs(this.tgPitch - this.pitch) > 0.002 ||
      Math.abs(this.tgTilt - this.tilt) > 0.002 ||
      Math.abs(this.tgSy - this.sy) > 0.002 ||
      Math.abs(this.tgSx - this.sx) > 0.002 ||
      Math.abs(this.tgEs - this.es) > 0.002 ||
      Math.abs(this.cfg.flame - this.flameLevel) > 0.004 ||
      Math.abs(this.cfg.glow - this.glow) > 0.004 ||
      this.slotH > 0.001 || Math.abs(this.slotHVel) > 0.001 ||
      Math.abs(this.col[0] - this.colT[0]) > 0.003 ||
      Math.abs(this.col[1] - this.colT[1]) > 0.003 ||
      Math.abs(this.col[2] - this.colT[2]) > 0.003
    );
  }

  /**
   * True while anything is still moving — lets the island stop its RAF loop.
   * A lit flame flickers continuously, so a burning Zuko is always busy; see
   * `ambientOnly` for a caller that wants to drop its frame rate meanwhile.
   */
  get busy(): boolean {
    return this.flameLit || this.moving;
  }

  /** True when the only thing animating is the flame flicker. */
  get ambientOnly(): boolean {
    return this.flameLit && !this.moving;
  }

  // ── Tweens ──────────────────────────────────────────────────────────────────

  anim(prop: PropKey, keys: TweenKey[], onComplete?: () => void) {
    this.tweens.set(prop, {
      prop, keys, index: 0, from: this[prop], startMs: performance.now(), onComplete,
    });
    this.locks.add(prop);
  }

  // ── Update ──────────────────────────────────────────────────────────────────

  update(dt: number) {
    const n = now();
    const nowMs = performance.now();

    for (const tw of [...this.tweens.values()]) {
      const k = tw.keys[tw.index];
      const p = Math.min(1, Math.max(0, (nowMs - tw.startMs) / k[1]));
      this[tw.prop] = tw.from + (k[0] - tw.from) * k[2](p);
      if (p >= 1) {
        tw.from = k[0];
        tw.index += 1;
        tw.startMs = nowMs;
        if (tw.index >= tw.keys.length) {
          this.tweens.delete(tw.prop);
          this.locks.delete(tw.prop);
          tw.onComplete?.();
        }
      }
    }

    const t = n - this.t0;
    let ty = this.lookX * 0.62;
    let tp = this.lookY * 0.5;

    if (this.cfg.look) {
      ty = ty * 0.35 + this.cfg.look[0] * 0.55;
      tp = tp * 0.3 + this.cfg.look[1] * 0.5;
    }
    if (this.cfg.scans) {
      ty = Math.sin(t * 2.6) * 0.6;
      tp = -0.06;
    }
    if (this.state === "sleeping") { ty = 0; tp = -0.14; }
    if (this.state === "dizzy") { ty = Math.sin(t * 9) * 0.25; }

    // Mini bots never follow the mouse — they wander.
    if (this.isMini && !this.cfg.look && !this.cfg.scans && this.state !== "sleeping" && this.state !== "dizzy") {
      if (n > this.miniLookNextTime) {
        this.miniLookTarget = {
          x: -0.88 + Math.random() * 1.76,
          y: -0.55 + Math.random() * 1.0,
        };
        this.miniLookNextTime = n + 0.5 + Math.random() * 1.5;
      }
      ty = this.miniLookTarget.x * 0.62;
      tp = this.miniLookTarget.y * 0.5;
    }

    this.tgYaw = ty;
    this.tgPitch = tp;
    this.tgTilt = this.cfg.tilt;

    const bounce = this.cfg.bounces ? -Math.abs(Math.sin(t * 5.2)) * 0.07 : 0;
    const kGen = 1 - Math.pow(0.0008, dt);
    if (!this.locks.has("oy")) this.oy += (bounce - this.oy) * kGen;

    if (this.cfg.breathes) {
      const amp = this.isMini ? 0.07 : 0.035;
      this.tgSy = 1 + Math.sin(t * 1.8) * amp;
      this.tgSx = 1 - Math.sin(t * 1.8) * amp * 0.57;
    } else if (this.isMini) {
      this.tgSy = 1 + Math.sin(t * 2.2) * 0.04;
      this.tgSx = 1 - Math.sin(t * 2.2) * 0.02;
    } else {
      this.tgSy = 1;
      this.tgSx = 1;
    }

    if (this.isMini && n > this.miniNextBehavior) this.doMiniBehaviorLoop();

    const kLook = 1 - Math.pow(0.0025, dt);
    if (!this.locks.has("yaw")) this.yaw += (this.tgYaw - this.yaw) * kLook;
    if (!this.locks.has("pitch")) this.pitch += (this.tgPitch - this.pitch) * kLook;
    if (!this.locks.has("tilt")) this.tilt += (this.tgTilt - this.tilt) * kGen;
    if (!this.locks.has("sy")) this.sy += (this.tgSy - this.sy) * kGen;
    if (!this.locks.has("sx")) this.sx += (this.tgSx - this.sx) * kGen;
    if (!this.locks.has("es")) this.es += (this.tgEs - this.es) * kGen;

    this.col = mix3(this.col, this.colT, 1 - Math.pow(0.002, dt));

    // Flame and LEDs ease towards the state over about half a second.
    const kSlow = 1 - Math.pow(0.004, dt);
    this.flameLevel += (this.cfg.flame - this.flameLevel) * kSlow;
    this.glow += (this.cfg.glow - this.glow) * kSlow;
    this.dangerMix += ((this.cfg.fire === "danger" ? 1 : 0) - this.dangerMix) * kSlow;
    this.successMix += ((this.cfg.fire === "success" ? 1 : 0) - this.successMix) * kSlow;

    if (n > this.nextBlink) {
      if (this.state !== "sleeping" && this.state !== "dizzy") {
        this.blink();
        if (Math.random() < 0.22) setTimeout(() => this.blink(), 230);
      }
      this.nextBlink = n + 2.2 + Math.random() * 3.2;
    }

    if (this.eyeOverride && n > this.eyeOverrideUntil) {
      this.eyeOverride = this.permanentEye;
      if (this.permanentEye) this.eyeOverrideUntil = Number.POSITIVE_INFINITY;
    }

    if (n - this.lastAmbient > 1.3) {
      this.lastAmbient = n;
      if (this.cfg.zz) this.emit("z", 1);
      if (!this.isMini && this.cfg.sweat && Math.random() < 0.5) this.emit("sweat", 1);
    }

    for (const p of this.particles) p.age += dt;
    this.particles = this.particles.filter((p) => p.age < p.life);

    // Scanner spring — ω₀ = 2π/0.25, ζ = 0.6
    const omega = (2 * Math.PI) / 0.25;
    const zeta = 0.6;
    const acc = omega * omega * (this.slotHTarget - this.slotH) - 2 * zeta * omega * this.slotHVel;
    this.slotHVel += acc * dt;
    this.slotH = Math.max(0, this.slotH + this.slotHVel * dt);

    this.lastTime = n;
  }

  private doMiniBehaviorLoop() {
    const n = now();
    switch (this.permanentEmote) {
      case "happy":
        if (this.locks.has("oy")) { this.miniNextBehavior = n + 0.4; return; }
        this.anim("oy", [[-0.3, 120, Ease.out], [0.03, 200, Ease.inOut], [0, 160, Ease.back]]);
        this.anim("sy", [[0.82, 80, Ease.out], [1.18, 130, Ease.out], [0.88, 160, Ease.inOut], [1, 200, Ease.back]]);
        this.anim("sx", [[1.15, 80, Ease.out], [0.88, 130, Ease.out], [1.06, 160, Ease.inOut], [1, 200, Ease.back]]);
        this.miniNextBehavior = n + 2.2 + Math.random() * 1.2;
        break;
      case "annoyed":
        if (this.locks.has("yaw")) { this.miniNextBehavior = n + 0.5; return; }
        this.anim("yaw", [
          [-0.65, 50, Ease.out], [0.65, 90, Ease.inOut], [-0.5, 80, Ease.inOut],
          [0.4, 75, Ease.inOut], [-0.2, 70, Ease.inOut], [0, 140, Ease.out],
        ]);
        this.miniNextBehavior = n + 3.0 + Math.random() * 2.5;
        break;
      case "wink":
        this.eyeOverride = "wink";
        this.eyeOverrideUntil = n + 0.55;
        this.anim("tilt", [[0.13, 100, Ease.out], [0.13, 320, Ease.lin], [0, 200, Ease.inOut]]);
        this.miniNextBehavior = n + 2.2 + Math.random() * 2.0;
        break;
      case "love":
        this.emit("heart", 2);
        this.anim("tilt", [[-0.1, 180, Ease.out], [0.1, 340, Ease.inOut], [0, 220, Ease.inOut]]);
        this.miniNextBehavior = n + 2.6 + Math.random() * 1.5;
        break;
      default:
        this.miniNextBehavior = n + 3.0 + Math.random() * 2.0;
    }
  }

  // ── Draw ────────────────────────────────────────────────────────────────────

  /**
   * Draws flame, body, visor, eyes, badge and particles into a canvas of
   * `w`×`h` CSS pixels (the caller has already applied the DPR transform). The
   * body is `0.6 × w` across, centred in the bottom `w`×`w` square.
   */
  draw(x: CanvasRenderingContext2D, W: number, H: number) {
    const R = W * 0.3;
    this.render(x, W / 2 + this.ox * R, H / 2 + this.particleOverhang / 2 + this.oy * R + R * 0.06, R);
  }

  /**
   * Draws Zuko laid out around (`cx`, `cy`) — the same point the island places
   * its bot canvas on — with a body `2R` across. Used by posed renderers.
   */
  drawAt(x: CanvasRenderingContext2D, cx: number, cy: number, R: number) {
    this.render(x, cx + this.ox * R, cy + this.oy * R + R * 0.06, R);
  }

  private render(x: CanvasRenderingContext2D, cx: number, cy: number, R: number) {
    if (R <= 0.5) return;
    const t = (this.clock ?? now()) + this.flickerSeed;
    // shadowBlur ignores the transform, so glow radii are scaled by hand.
    const m = x.getTransform();
    const px = Math.hypot(m.a, m.b) || 1;
    const face = this.face(R);

    x.save();
    x.translate(cx, cy);
    if (this.tilt !== 0) x.rotate(this.tilt);
    x.scale(this.sx, this.sy);
    this.drawFlame(x, R, t);
    const body = shieldPath(R, this.morph);
    this.drawBody(x, body, R, face);
    this.drawVisor(x, R, face, t, px);
    x.restore();

    if (this.badge && this.badgeS > 0.01 && this.morph < 0.25) {
      this.drawBadge(x, this.badge, R, cx, cy, t);
    }
    this.drawParticles(x, R, cx, cy);
  }

  /** Visor and eye sizes. Small Zukos get a taller visor and bolder LEDs. */
  private face(R: number): Face {
    const s = Math.max(clamp((18 - R) / 11, 0, 1), this.isMini ? 0.8 : 0);
    const m = clamp(this.morph, 0, 1);
    const vh = lerp(lerp(0.46, 0.64, s), 0.6, m) * R;
    return {
      vcy: lerp(-0.22, -0.2, s) * R,
      vhw: lerp(0.76, 0.82, s) * R,
      vh,
      vr: Math.min(vh / 2, lerp(0.2, 0.26, s) * R),
      sag: lerp(0.05, 0.025, s) * R,
      ew: lerp(0.3, 0.36, s) * R * this.es,
      eh: lerp(0.155, 0.24, s) * R * this.es,
      esp: lerp(0.33, 0.37, s) * R,
    };
  }

  private flamePalette(): FirePalette {
    return mixFire(mixFire(FIRE.ember, FIRE.danger, this.dangerMix), FIRE.success, this.successMix);
  }

  /** The ember tuft, drawn first so the head hides its roots. */
  private drawFlame(x: CanvasRenderingContext2D, R: number, t: number) {
    const lvl = this.ignite * (this.flameLevel + this.flare * 0.5 + this.boost * 0.2);
    if (lvl < 0.02) return;
    const pal = this.flamePalette();
    const baseY = SHIELD.top * R + R * 0.16;
    // Small Zukos get a proportionally bigger flame so it still reads.
    const small = 1 + 0.4 * clamp((22 - R) / 12, 0, 1);
    const H = R * 0.7 * lvl * small;
    const W = R * 0.5 * clamp(0.65 + 0.35 * lvl, 0.65, 1.15) * small;
    const speed = 0.8 + 0.5 * clamp(lvl, 0, 1.6);

    x.save();
    if (lvl < 0.3) x.globalAlpha = lvl / 0.3;
    if (!this.isMini) {
      const gy = baseY - H * 0.45;
      const gr = H * 1.05 + R * 0.12;
      const g = x.createRadialGradient(0, gy, 0, 0, gy, gr);
      g.addColorStop(0, rgba(pal.base, 0.34));
      g.addColorStop(1, rgba(pal.base, 0));
      x.fillStyle = g;
      x.beginPath();
      x.arc(0, gy, gr, 0, Math.PI * 2);
      x.fill();
    }
    for (const tg of this.isMini ? MINI_TONGUES : TONGUES) {
      const flick = 1 +
        0.14 * Math.sin(t * tg.f * speed + tg.ph) +
        0.07 * Math.sin(t * tg.f * 1.73 * speed + tg.ph * 2.1);
      const hh = H * tg.h * flick;
      const bx = tg.dx * W;
      const tipX = bx + (tg.lean + Math.sin(t * tg.f * 0.57 * speed + tg.ph * 1.3) * 0.1) * hh;
      const g = x.createLinearGradient(0, baseY, 0, baseY - hh);
      if (tg.core) {
        g.addColorStop(0, rgba(pal.coreBase, 0.95));
        g.addColorStop(1, rgba(pal.coreTip, 0.9));
      } else {
        g.addColorStop(0, rgba(pal.root));
        g.addColorStop(0.3, rgba(pal.base));
        g.addColorStop(1, rgba(pal.tip));
      }
      x.fillStyle = g;
      tonguePath(x, bx, baseY, tg.w * W, hh, tipX);
      x.fill();
    }
    x.restore();
  }

  private drawBody(x: CanvasRenderingContext2D, body: Path2D, R: number, f: Face) {
    if (this.bodyColor && this.isMini) {
      // Mini bots: flat solid fill — the visor and LEDs carry the face.
      x.fillStyle = rgba(this.bodyColor, 1);
      x.fill(body);
      return;
    }
    const top = SHIELD.top * R;
    const tip = SHIELD.tipY * R;
    const [cTop, cBot] = this.bodyColor
      ? [mix3(this.bodyColor, WHITE, 0.08), mix3(this.bodyColor, BLACK, 0.5)]
      : [BODY_TOP, BODY_BOTTOM];
    const g = x.createLinearGradient(0, top, 0, tip);
    g.addColorStop(0, rgba(cTop));
    g.addColorStop(1, rgba(cBot));
    x.fillStyle = g;
    x.fill(body);

    x.save();
    x.clip(body);

    // LED light spilling out of the visor onto the face.
    const spill = this.tint * this.glow * this.boot;
    if (spill > 0.01) {
      const sg = x.createRadialGradient(0, f.vcy, f.vh * 0.3, 0, f.vcy, R * 1.25);
      sg.addColorStop(0, rgba(this.col, 0.13 * spill));
      sg.addColorStop(1, rgba(this.col, 0));
      x.fillStyle = sg;
      x.fill(body);
    }

    // Shade towards the point.
    const sh = x.createLinearGradient(0, R * 0.1, 0, tip);
    sh.addColorStop(0, "rgba(0,0,0,0)");
    sh.addColorStop(1, "rgba(0,0,0,0.3)");
    x.fillStyle = sh;
    x.fill(body);

    // Soft highlight across the top.
    x.save();
    x.translate(-R * 0.18, top + R * 0.2);
    x.scale(1, 0.5);
    const hl = x.createRadialGradient(0, 0, 0, 0, 0, R * 0.8);
    hl.addColorStop(0, "rgba(220,232,255,0.11)");
    hl.addColorStop(1, "rgba(255,255,255,0)");
    x.fillStyle = hl;
    x.beginPath();
    x.arc(0, 0, R * 0.8, 0, Math.PI * 2);
    x.fill();
    x.restore();

    // Cool rim light: the inner half of a wide stroke, brightest top-left.
    const rim = mix3(RIM, this.col, 0.3 * this.tint);
    const rg = x.createLinearGradient(-R, top, R * 0.7, tip);
    rg.addColorStop(0, rgba(rim, 0.5));
    rg.addColorStop(0.55, rgba(rim, 0.15));
    rg.addColorStop(1, rgba(rim, 0.06));
    x.lineWidth = Math.max(1.2, R * 0.1);
    x.strokeStyle = rg;
    x.stroke(body);
    x.restore();

    // A crisp dark edge so the navy still reads on a light background.
    x.lineWidth = Math.max(0.6, R * 0.025);
    x.strokeStyle = "rgba(3,5,10,0.85)";
    x.stroke(body);
  }

  private drawVisor(x: CanvasRenderingContext2D, R: number, f: Face, t: number, px: number) {
    const vp = visorPath(f);
    const top = f.vcy - f.vh / 2;
    const bot = f.vcy + f.vh / 2 + f.sag;

    // Bezel, then the glass.
    x.lineWidth = Math.max(0.8, R * 0.06);
    x.strokeStyle = "rgba(2,3,8,0.9)";
    x.stroke(vp);
    const g = x.createLinearGradient(0, top, 0, bot);
    g.addColorStop(0, "#121A2C");
    g.addColorStop(0.45, "#070A12");
    g.addColorStop(1, "#04060B");
    x.fillStyle = g;
    x.fill(vp);

    x.save();
    x.clip(vp);

    // LED light filling the glass.
    const lit = this.glow * smoothstep(this.boot * 1.6) * (1 + this.boost * 0.4);
    if (lit > 0.01) {
      const sg = x.createRadialGradient(0, f.vcy, 0, 0, f.vcy, f.vhw * 1.05);
      sg.addColorStop(0, rgba(this.col, 0.2 * lit));
      sg.addColorStop(1, rgba(this.col, 0));
      x.fillStyle = sg;
      x.fillRect(-f.vhw, top, f.vhw * 2, bot - top);
    }

    // Faint scanlines, only where there are pixels to spare.
    if (R > 16 && !this.isMini) {
      x.fillStyle = "rgba(160,190,255,0.035)";
      const step = Math.max(1.5, R * 0.06);
      const lh = Math.max(0.5, R * 0.016);
      for (let y = top + step / 2; y < bot; y += step) x.fillRect(-f.vhw, y, f.vhw * 2, lh);
    }

    if (this.cfg.scans) this.drawSearchColumn(x, R, f);
    this.drawEyes(x, R, f, t, px);
    if (this.morph > 0.05) this.drawScanner(x, R, f, t, px);
    if (this.boot < 1) this.drawBootLine(x, R, f, px);

    // Reflection: a diagonal sheen that drifts against the gaze.
    const dx = -this.yaw * R * 0.08;
    x.fillStyle = "rgba(190,215,255,0.075)";
    x.beginPath();
    x.moveTo(dx - f.vhw * 0.56, top);
    x.lineTo(dx - f.vhw * 0.3, top);
    x.lineTo(dx - f.vhw * 0.6, bot);
    x.lineTo(dx - f.vhw * 0.86, bot);
    x.closePath();
    x.fill();
    const lw = Math.max(0.5, R * 0.022);
    x.strokeStyle = "rgba(200,220,255,0.18)";
    x.lineWidth = lw;
    x.beginPath();
    x.moveTo(-f.vhw + f.vr, top + lw);
    x.lineTo(f.vhw - f.vr, top + lw);
    x.stroke();
    x.restore();
  }

  /** Two LEDs. They drift towards the cursor and never leave the visor. */
  private drawEyes(x: CanvasRenderingContext2D, R: number, f: Face, t: number, px: number) {
    let shape: EyeShape = this.eyeOverride ?? this.cfg.eye;
    if (this.morph > 0.5) {
      if (this.isScanning) shape = "happy";
      else if (this.slotHTarget > 0.05 || this.slotH > 0.1) shape = "cup";
    }

    const yawN = clamp(this.yaw / 0.62, -1.3, 1.3);
    const pitchN = clamp(this.pitch / 0.5, -1.3, 1.3);
    const maxDx = Math.max(0, f.vhw - f.esp - f.ew * 0.62);
    const dx = clamp(yawN * R * 0.2, -maxDx, maxDx);
    let dy = -pitchN * f.vh * 0.18;
    // A roll scrolls the LEDs up out of the visor and back in from below.
    if (this.roll !== 0) {
      const turns = this.roll / (Math.PI * 2);
      const frac = turns - Math.floor(turns);
      dy += (frac < 0.5 ? -frac : 1 - frac) * 2 * (f.vh + f.eh);
    }

    const usesOpen = shape === "pill" || shape === "wide" || shape === "cup" || shape === "wink";
    const blinkDim = usesOpen ? 0.4 + 0.6 * clamp(this.open, 0, 1) : 1;
    const bootX = lerp(-f.vhw, f.vhw, this.boot);

    for (const sd of [-1, 1]) {
      const ex = sd * f.esp + dx;
      const on = this.boot >= 1 ? 1 : smoothstep((bootX - ex + f.ew * 0.5) / f.ew);
      if (on <= 0.01) continue;
      const persp = 1 + sd * clamp(this.yaw, -0.7, 0.7) * 0.16;
      x.save();
      x.translate(ex, f.vcy + dy + f.sag * 0.3);
      x.scale(persp, 1);
      this.drawEyeShape(x, shape, f.ew, f.eh, sd, t, R * 0.28 * px, on * this.glow * blinkDim);
      x.restore();
    }
  }

  /** One LED glyph: a glowing body pass, then a hot white-ish core. */
  private drawEyeShape(
    x: CanvasRenderingContext2D, shape: EyeShape,
    w: number, h: number, sd: number, t: number, blur: number, a: number,
  ) {
    const col = shape === "heart" ? HEART : shape === "star" ? STAR : this.col;
    const glowA = clamp(a * (1 + this.boost * 0.3), 0, 1);
    x.lineCap = "round";
    x.lineJoin = "round";
    for (const core of [false, true]) {
      x.save();
      if (!core) {
        x.shadowColor = rgba(col, 0.85 * glowA);
        x.shadowBlur = blur * (1 + this.boost * 0.5);
      }
      const c = core ? mix3(col, WHITE, 0.72) : mix3(col, WHITE, 0.12);
      x.fillStyle = rgba(c, core ? 0.8 * glowA : glowA);
      x.strokeStyle = x.fillStyle;
      this.eyeGlyph(x, shape, w, h, sd, t, core);
      x.restore();
    }
  }

  private eyeGlyph(
    x: CanvasRenderingContext2D, shape: EyeShape,
    w: number, h: number, sd: number, t: number, core: boolean,
  ) {
    // Filled glyphs shrink for the core; stroked glyphs thin out.
    const fill = (draw: () => void, csx = 0.62, csy = 0.4) => {
      if (core) x.scale(csx, csy);
      draw();
      x.fill();
    };
    const stroke = (lw: number, draw: () => void) => {
      x.lineWidth = core ? lw * 0.38 : lw;
      draw();
      x.stroke();
    };
    switch (shape) {
      case "wide":
        this.eyeGlyph(x, "pill", w * 1.12, h * 1.7, sd, t, core);
        break;
      case "pill": {
        const hh = Math.max(h * this.open, h * 0.18);
        fill(() => barPath(x, w, hh));
        break;
      }
      case "dot":
        fill(() => {
          x.beginPath();
          x.arc(0, 0, Math.min(h * 0.62, w * 0.3), 0, Math.PI * 2);
        }, 0.5, 0.5);
        break;
      case "line":
        x.rotate(-sd * 0.22);
        fill(() => barPath(x, w * 1.05, Math.max(h * 0.36, 1)), 0.7, 0.4);
        break;
      case "flat":
        fill(() => barPath(x, w * 1.15, Math.max(h * 0.42, 1)), 0.75, 0.4);
        break;
      case "happy":
        stroke(h * 0.55, () => {
          x.beginPath();
          x.arc(0, h * 0.62, w * 0.46, Math.PI * 1.12, Math.PI * 1.88);
        });
        break;
      case "closed":
        stroke(h * 0.42, () => {
          x.beginPath();
          x.arc(0, -h * 0.55, w * 0.46, Math.PI * 0.14, Math.PI * 0.86);
        });
        break;
      case "spiral": {
        const rMax = Math.min(w * 0.52, h * 1.3);
        stroke(Math.max(h * 0.24, 0.8), () => {
          x.beginPath();
          for (let a = 0; a < 4.4 * Math.PI; a += 0.2) {
            const r = rMax * (0.12 + (a / (4.4 * Math.PI)) * 0.88);
            const aa = a + t * 9 * sd;
            if (a === 0) x.moveTo(Math.cos(aa) * r, Math.sin(aa) * r);
            else x.lineTo(Math.cos(aa) * r, Math.sin(aa) * r);
          }
        });
        break;
      }
      case "heart":
        fill(() => heartPath(x, Math.min(w * 0.62, h * 1.45)), 0.5, 0.5);
        break;
      case "star":
        x.rotate(t * 1.5 * sd);
        fill(() => {
          const ro = Math.min(w * 0.56, h * 1.4);
          starPath(x, ro, ro * 0.45);
        }, 0.5, 0.5);
        break;
      case "tired":
        // A drooping lid over half a bar.
        x.save();
        x.translate(0, h * 0.2);
        fill(() => barPath(x, w, h * 0.5));
        x.restore();
        if (!core) {
          x.globalAlpha = 0.55;
          barPath(x, w * 1.12, Math.max(h * 0.16, 0.8));
          x.translate(0, -h * 0.28);
          x.fill();
        }
        break;
      case "wink":
        if (sd < 0) this.eyeGlyph(x, "pill", w, h, sd, t, core);
        else this.eyeGlyph(x, "happy", w, h, sd, t, core);
        break;
      case "cup": {
        // Flat top, rounded bottom — eager, while the scanner is up.
        const hh = Math.max(h * 1.45 * this.open, h * 0.3);
        const cr = Math.min(w / 2, hh / 2);
        fill(() => {
          x.beginPath();
          x.moveTo(-w / 2, -hh / 2);
          x.lineTo(w / 2, -hh / 2);
          x.lineTo(w / 2, hh / 2 - cr);
          x.quadraticCurveTo(w / 2, hh / 2, w / 2 - cr, hh / 2);
          x.lineTo(-w / 2 + cr, hh / 2);
          x.quadraticCurveTo(-w / 2, hh / 2, -w / 2, hh / 2 - cr);
          x.closePath();
        });
        break;
      }
    }
  }

  /** Searching: a faint column of light travels with the LEDs. */
  private drawSearchColumn(x: CanvasRenderingContext2D, R: number, f: Face) {
    const cxs = clamp((this.yaw / 0.62) * R * 0.2, -f.vhw, f.vhw);
    const w = f.esp * 2 + f.ew;
    const g = x.createLinearGradient(cxs - w / 2, 0, cxs + w / 2, 0);
    g.addColorStop(0, rgba(this.col, 0));
    g.addColorStop(0.5, rgba(this.col, 0.14 * this.glow));
    g.addColorStop(1, rgba(this.col, 0));
    x.fillStyle = g;
    x.fillRect(cxs - w / 2, f.vcy - f.vh, w, f.vh * 2);
  }

  /** Drag-over scan stance: a bright bar sweeps the visor back and forth. */
  private drawScanner(x: CanvasRenderingContext2D, R: number, f: Face, t: number, px: number) {
    const a = clamp(this.morph, 0, 1) * clamp(0.35 + this.slotH * 1.6, 0, 1);
    const phase = t * 4.2;
    const sx = Math.sin(phase) * f.vhw * 0.82;
    const dir = Math.cos(phase) >= 0 ? 1 : -1;
    const trail = f.vhw * 0.4;
    const top = f.vcy - f.vh / 2;
    const h = f.vh + f.sag * 2;
    const g = x.createLinearGradient(sx - dir * trail, 0, sx, 0);
    g.addColorStop(0, rgba(this.col, 0));
    g.addColorStop(1, rgba(this.col, 0.35 * a));
    x.fillStyle = g;
    x.fillRect(Math.min(sx, sx - dir * trail), top, trail, h);
    x.save();
    x.shadowColor = rgba(this.col, a);
    x.shadowBlur = R * 0.3 * px;
    x.fillStyle = rgba(mix3(this.col, WHITE, 0.55), a);
    x.fillRect(sx - R * 0.02, top, R * 0.04, h);
    x.restore();
  }

  /** Visor boot: a scan line crosses the glass, lighting it as it goes. */
  private drawBootLine(x: CanvasRenderingContext2D, R: number, f: Face, px: number) {
    if (this.boot <= 0) return;
    const bx = lerp(-f.vhw, f.vhw, this.boot);
    const a = clamp(Math.min(this.boot / 0.06, (1 - this.boot) / 0.1), 0, 1);
    const top = f.vcy - f.vh / 2;
    const h = f.vh + f.sag * 2;
    const trail = f.vhw * 0.7;
    const g = x.createLinearGradient(bx - trail, 0, bx, 0);
    g.addColorStop(0, rgba(this.col, 0));
    g.addColorStop(1, rgba(this.col, 0.3 * a));
    x.fillStyle = g;
    x.fillRect(bx - trail, top, trail, h);
    x.save();
    x.shadowColor = rgba(this.col, a);
    x.shadowBlur = R * 0.35 * px;
    x.fillStyle = rgba(mix3(this.col, WHITE, 0.65), a);
    x.fillRect(bx - Math.max(0.5, R * 0.02), top, Math.max(1, R * 0.04), h);
    x.restore();
  }

  private drawBadge(x: CanvasRenderingContext2D, badge: Badge, R: number, cx: number, cy: number, t: number) {
    const bs = this.badgeS * (this.isMini ? 1.25 : 1);
    const bx = cx - R * 0.74 * this.sx;
    const by = cy - R * 0.74 * this.sy;

    x.save();
    x.translate(bx, by);
    x.scale(bs, bs);
    const col = rgba(badge.color);

    if (badge.kind === "dots") {
      if (this.isMini) {
        const phase = (t * 2.4) % 1;
        const dotR = R * 0.22 * (1 + 0.25 * Math.sin(phase * Math.PI * 2));
        x.fillStyle = "#000";
        x.beginPath();
        x.arc(0, 0, R * 0.2, 0, Math.PI * 2);
        x.fill();
        x.fillStyle = col;
        x.beginPath();
        x.arc(0, 0, dotR, 0, Math.PI * 2);
        x.fill();
      } else {
        const pw = R * 0.72;
        const ph = R * 0.36;
        roundRectPath(x, -pw / 2 - R * 0.05, -ph / 2 - R * 0.05, pw + R * 0.1, ph + R * 0.1, ph / 2 + R * 0.05);
        x.fillStyle = "#000";
        x.fill();
        roundRectPath(x, -pw / 2, -ph / 2, pw, ph, ph / 2);
        x.fillStyle = col;
        x.fill();
        for (let i = 0; i < 3; i++) {
          const phase = (((t * 2.4 - i * 0.22) % 1) + 1) % 1;
          const dotR = R * 0.055 * (1 + 0.4 * Math.max(0, Math.sin(phase * Math.PI * 2)));
          x.fillStyle = "#0A0E18";
          x.beginPath();
          x.arc((i - 1) * R * 0.18, 0, dotR, 0, Math.PI * 2);
          x.fill();
        }
      }
    } else if (badge.kind === "bang" || badge.kind === "question") {
      x.fillStyle = "#000";
      x.beginPath();
      x.arc(0, 0, R * 0.3, 0, Math.PI * 2);
      x.fill();
      x.fillStyle = col;
      x.beginPath();
      x.arc(0, 0, R * 0.23, 0, Math.PI * 2);
      x.fill();
      if (!this.isMini) {
        x.fillStyle = "#0A0E18";
        x.font = `900 ${R * 0.32}px ${FONT}`;
        x.textAlign = "center";
        x.textBaseline = "middle";
        x.fillText(badge.kind === "bang" ? "!" : "?", 0, R * 0.02);
      }
    } else {
      x.fillStyle = "#000";
      x.beginPath();
      x.arc(0, 0, R * 0.2, 0, Math.PI * 2);
      x.fill();
      x.fillStyle = col;
      x.beginPath();
      x.arc(0, 0, R * 0.135, 0, Math.PI * 2);
      x.fill();
    }
    x.restore();
  }

  private drawParticles(x: CanvasRenderingContext2D, R: number, cx: number, cy: number) {
    for (const p of this.particles) {
      if (p.age <= 0) continue;
      const k = p.age / p.life;
      const a = k < 0.2 ? k / 0.2 : 1 - (k - 0.2) / 0.8;
      const px = cx + (p.x + p.vx * p.age) * R * 1.3;
      const py = cy + (p.y + p.vy * p.age) * R * 1.3;
      const sz = R * p.size * (1 + k * 0.4);

      x.save();
      x.translate(px, py);
      x.globalAlpha = Math.min(1, Math.max(0, a));
      switch (p.type) {
        case "heart":
          x.rotate(Math.sin(p.age * 6) * 0.3);
          x.fillStyle = rgba(HEART);
          heartPath(x, sz);
          x.fill();
          break;
        case "star":
          x.rotate(p.rot + p.age * 2);
          x.fillStyle = rgba(STAR);
          starPath(x, sz, sz * 0.45);
          x.fill();
          break;
        case "spark":
          // Embers thrown off by the flame.
          x.rotate(p.rot);
          x.fillStyle = "#FFE29A";
          starPath(x, sz * 0.8, sz * 0.18);
          x.fill();
          break;
        case "sweat":
          x.fillStyle = "#7CC7FF";
          x.beginPath();
          x.moveTo(0, -sz);
          x.quadraticCurveTo(sz * 0.8, sz * 0.2, 0, sz * 0.6);
          x.quadraticCurveTo(-sz * 0.8, sz * 0.2, 0, -sz);
          x.fill();
          break;
        case "z":
          x.fillStyle = "rgb(190,214,240)";
          x.font = `700 ${sz * 1.9}px ${FONT}`;
          x.textAlign = "center";
          x.textBaseline = "middle";
          x.fillText("z", 0, 0);
          break;
      }
      x.restore();
    }
  }
}
