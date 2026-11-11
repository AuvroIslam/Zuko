// Zuko — the on-screen character, drawn with Canvas 2D.
//
// A chibi fan tribute to Prince Zuko (Avatar: The Last Airbender): a big glossy
// sphere of a head with glowing amber almond eyes, the scar round his left eye
// (the viewer's right), a black topknot in a red hair-tie whose ponytail swings
// when he moves, and a little Fire Nation tunic with stubby arms and cream
// fists. And he firebends: `shootFire()` throws a fire punch and a fireball,
// `fireFlick()` puffs a flame off the fist, `setFireAura()` makes flames simmer
// round his head and `setFireRing()` floats him on a ring of fire.
//
// The tween, emote and particle machinery comes from the MIT-licensed BotEngine
// port the app started from; everything drawn is Zuko's own. One renderer
// serves the whole app: the island bot and the mini bots tick a BotEngine,
// while the launch greeting and the drop sequence pose one directly (fields +
// `clock`) and call `drawAt()`. Fire that leaves the bot's box is drawn by
// `drawFx()` onto whatever layer the caller provides (the island has a
// full-window effects canvas; posed renderers pass their own context).

import { Ease, clamp, lerp, type EaseFn } from "../core/anim";
import { Sound } from "../core/sound";
import type { BotEmoteName, BotStateName } from "../core/layout";
import {
  FIRE, TAU, burst, easeInOut, easeOut, ember, fireball, flame, glow, hexToRGB, mix3, mixFire, tearPath,
  punchTrail, rgba, seeded, swirl, type FireKind, type FirePalette, type RGB,
} from "./fire";

export { FIRE, hexToRGB, type FireKind, type RGB } from "./fire";

// ── Types ─────────────────────────────────────────────────────────────────────

export type EyeShape =
  | "pill" | "wide" | "dot" | "line" | "flat" | "happy" | "closed"
  | "spiral" | "heart" | "star" | "tired" | "wink" | "cup"
  | "angry" | "think";

export type BadgeKind = "dots" | "bang" | "question" | "dot";

export interface Badge {
  kind: BadgeKind;
  color: RGB;
}

export type TweenKey = readonly [target: number, durationMs: number, ease: EaseFn];

type Mouth = "none" | "smile" | "grin" | "frown" | "wavy" | "o" | "flat" | "grit";

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
  /** Eye glow colour. */
  color: RGB;
  /** How much of the eye glow warms the head's rim, 0…1. */
  tint: number;
  eye: EyeShape;
  mouth: Mouth;
  badge: Badge | null;
  bounces: boolean;
  scans: boolean;
  breathes: boolean;
  zz: boolean;
  sweat: boolean;
  look: readonly [number, number] | null;
  tilt: number;
  /** Eye brightness, 0…1. */
  glow: number;
  /** Resting flame aura round the head, 0…1. */
  aura: number;
  fire: FireKind;
  /** The eye glow pulses (waiting on the human). */
  pulse: boolean;
}

interface Particle {
  type: "heart" | "star" | "spark" | "sweat" | "z";
  x: number; y: number; vx: number; vy: number;
  age: number; life: number; rot: number; size: number;
}

/** A point in effects-layer coordinates (CSS pixels). */
export interface FxPoint { x: number; y: number }

export interface FireOptions {
  /** Fire colour; defaults to the everyday ember (red for blocks). */
  kind?: FireKind;
  /** Seconds before the punch starts. */
  delay?: number;
  /** Without a target: direction (radians, 0 = right) and range in pixels. */
  angle?: number;
  distance?: number;
  /** Called when the fireball (or flick) lands. */
  onImpact?: () => void;
  /** Play the fire sounds (default true). */
  sound?: boolean;
}

interface FxItem {
  kind: "punch" | "flick" | "swirl";
  t0: number;
  dur: number;
  side: -1 | 1;
  ang: number;
  target: FxPoint | null;
  dist: number;
  flight: number;
  pal: FireKind;
  seed: number;
  onImpact?: () => void;
  sound: boolean;
  /** Bit flags: 1 launch sound, 2 impact. */
  fired: number;
  /** Where the fireball left the fist (set on the first frame after launch). */
  from: FxPoint | null;
}

/** The punching arm's pose for one frame. */
interface PunchPose {
  active: boolean;
  side: -1 | 1;
  ang: number;
  /** 0 = arm at rest, 1 = aimed along `ang`. */
  aim: number;
  /** Arm length multiplier (1 = rest). */
  ext: number;
  /** Flame on the fist, 0…1. */
  fire: number;
  /** Body rotation and horizontal lunge (units of R). */
  lean: number;
  lunge: number;
  angry: boolean;
  aura: number;
  kind: FireKind;
}

/** A fist in effects-layer coordinates, with its arm direction and radius. */
export interface Fist { x: number; y: number; dx: number; dy: number; r: number }

// ── Palette ───────────────────────────────────────────────────────────────────

/** Eye glow colours. */
export const LED = {
  ember: hexToRGB("#FFA63D"), // idle, working
  amber: hexToRGB("#FFC93C"), // approval, question
  red: hexToRGB("#FF5533"), // error, danger
  gold: hexToRGB("#FFD866"), // finished
  sun: hexToRGB("#FFB95E"), // thinking, searching
  orange: hexToRGB("#FF8A2A"), // rate limited
  rose: hexToRGB("#FF7A8A"), // dizzy
  dim: hexToRGB("#E88F3A"), // sleeping
} as const;

const WHITE: RGB = [1, 1, 1];
const BLACK: RGB = [0, 0, 0];
const CREAM = hexToRGB("#F7F3EE");
const BEIGE = hexToRGB("#E8D5C4");
const SHADE = hexToRGB("#CBAE96");
const INK = hexToRGB("#2B1D1A");
const UNLIT = hexToRGB("#4A2C22");
const EYE_EDGE = hexToRGB("#B5400E");
const HEART = hexToRGB("#FF5C7A");
const STAR = hexToRGB("#FFD166");
const INK_S = "#2B1D1A";
const GOLD = "#E0A030";

const FONT = `system-ui, "Segoe UI Variable Text", "Segoe UI", sans-serif`;

// ── Geometry ──────────────────────────────────────────────────────────────────

/** Level of detail: 0 = mini / tiny, 1 = small, 2 = full. */
type Tier = 0 | 1 | 2;

/** Eye metrics per tier, in head radii: width, height, spacing, height offset. */
const EYES: Record<Tier, readonly [number, number, number, number]> = {
  0: [0.35, 0.47, 0.39, 0.09],
  1: [0.32, 0.44, 0.38, 0.07],
  2: [0.31, 0.44, 0.37, 0.07],
};

/** Ponytail locks: a cubic centre line from the top of the hair-tie, in head radii. */
const LOCKS: readonly { p: readonly number[]; w: number; tier: Tier; flex: number }[] = [
  // One glossy swoosh made of a few locks that rise out of the tie, arc over to
  // the right and fall behind the head, ending in staggered points.
  { p: [0, 0, 0.05, -0.55, 0.78, -0.66, 1.02, 0.38], w: 0.21, tier: 0, flex: 1 },
  { p: [0, 0, -0.02, -0.62, 0.52, -0.84, 0.88, -0.36], w: 0.15, tier: 1, flex: 0.75 },
  { p: [0.02, 0, 0.2, -0.42, 0.86, -0.5, 1.14, 0.1], w: 0.13, tier: 1, flex: 1.1 },
  { p: [0.02, 0, 0.36, -0.24, 0.86, -0.2, 0.9, 0.62], w: 0.1, tier: 1, flex: 1.25 },
  // A little flick forward off the top.
  { p: [0, -0.02, -0.18, -0.26, -0.14, -0.5, 0.1, -0.6], w: 0.09, tier: 1, flex: 0.5 },
  // A wisp curling off the end of the main tail.
  { p: [0.94, 0.06, 1.16, 0.2, 1.22, 0.46, 1.06, 0.8], w: 0.055, tier: 2, flex: 1.4 },
];

/** The scar, in head radii round its centre (just up and out from the right eye). */
const SCAR = (() => {
  const p = new Path2D();
  p.moveTo(-0.2, 0.2);
  p.bezierCurveTo(-0.32, 0.05, -0.3, -0.2, -0.12, -0.28);
  p.bezierCurveTo(-0.05, -0.31, 0.0, -0.29, 0.04, -0.37);
  p.bezierCurveTo(0.08, -0.3, 0.13, -0.29, 0.2, -0.36);
  p.bezierCurveTo(0.22, -0.28, 0.28, -0.24, 0.31, -0.18);
  p.bezierCurveTo(0.38, -0.02, 0.33, 0.16, 0.18, 0.24);
  p.bezierCurveTo(0.06, 0.3, -0.1, 0.29, -0.2, 0.2);
  p.closePath();
  return p;
})();

/** A lighter brush stroke inside the scar. */
const SCAR_STREAK = (() => {
  const p = new Path2D();
  p.moveTo(-0.15, -0.1);
  p.bezierCurveTo(-0.08, -0.22, 0.12, -0.25, 0.22, -0.16);
  p.bezierCurveTo(0.1, -0.18, -0.03, -0.13, -0.15, -0.1);
  p.closePath();
  return p;
})();

/**
 * A heater-shield outline, centred on the origin, `R` = half its width. Kept
 * for badges (the drop sequence's "scanned" mark); `morph` rounds the point.
 */
export const SHIELD = {
  halfW: 1.0, top: -0.86, corner: 0.36, bulge: 0.035, flankY: 0.24,
  taperX: 0.58, taperY: 0.74, tipHalf: 0.16, tipY: 0.98,
} as const;

export function shieldPath(R: number, morph = 0): Path2D {
  const m = clamp(morph, 0, 1);
  const a = SHIELD.halfW * R;
  const top = SHIELD.top * R;
  const rc = SHIELD.corner * R;
  const flankY = SHIELD.flankY * R;
  const taperX = lerp(SHIELD.taperX, 0.86, m) * R;
  const taperY = lerp(SHIELD.taperY, 0.66, m) * R;
  const tipHalf = lerp(SHIELD.tipHalf, 0.34, m) * R;
  const tipY = lerp(SHIELD.tipY, 0.93, m) * R;
  const d = ((tipY - taperY) * tipHalf) / (2 * taperX - tipHalf);
  const p = new Path2D();
  p.moveTo(-a + rc, top);
  p.lineTo(a - rc, top);
  p.quadraticCurveTo(a, top, a, top + rc);
  p.bezierCurveTo(a, flankY, taperX, taperY, tipHalf, tipY - d);
  p.quadraticCurveTo(0, tipY + d, -tipHalf, tipY - d);
  p.bezierCurveTo(-taperX, taperY, -a, flankY, -a, top + rc);
  p.quadraticCurveTo(-a, top, -a + rc, top);
  p.closePath();
  return p;
}

// ── States ────────────────────────────────────────────────────────────────────

const base = {
  bounces: false, scans: false, breathes: false, zz: false, sweat: false,
  look: null, tilt: 0, glow: 1, aura: 0, fire: "ember" as FireKind, pulse: false, mouth: "none" as Mouth,
};

export const BOT_STATES: Record<BotStateName, BotStateCfg> = {
  idle: { ...base, color: LED.ember, tint: 0.35, eye: "pill", badge: null },
  working: { ...base, color: LED.ember, tint: 0.55, eye: "pill", badge: { kind: "dots", color: LED.ember } },
  thinking: { ...base, color: LED.sun, tint: 0.5, eye: "think", mouth: "wavy", badge: { kind: "question", color: LED.sun }, look: [0.5, 0.55] },
  searching: { ...base, color: LED.sun, tint: 0.55, eye: "pill", badge: { kind: "dots", color: LED.sun }, scans: true },
  approval: { ...base, color: LED.amber, tint: 0.8, eye: "wide", badge: { kind: "bang", color: LED.amber }, bounces: true, pulse: true },
  question: { ...base, color: LED.amber, tint: 0.7, eye: "pill", badge: { kind: "question", color: LED.amber }, tilt: 0.15, pulse: true },
  error: { ...base, color: LED.red, tint: 0.85, eye: "angry", mouth: "frown", badge: { kind: "dot", color: LED.red }, aura: 0.85, fire: "danger" },
  finished: { ...base, color: LED.gold, tint: 0.6, eye: "happy", mouth: "grin", badge: { kind: "dot", color: LED.gold }, fire: "success" },
  ratelimit: { ...base, color: LED.orange, tint: 0.6, eye: "tired", mouth: "flat", badge: { kind: "dot", color: LED.orange }, sweat: true },
  sleeping: { ...base, color: LED.dim, tint: 0.2, eye: "closed", badge: null, breathes: true, zz: true, glow: 0.5 },
  dizzy: { ...base, color: LED.rose, tint: 0.5, eye: "spiral", mouth: "wavy", badge: null },
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

/** The mouth an eye override brings with it. */
const EYE_MOUTH: Partial<Record<EyeShape, Mouth>> = {
  happy: "smile", heart: "smile", star: "grin", wink: "smile", dot: "o", line: "frown",
  angry: "frown", flat: "frown", spiral: "wavy", think: "wavy", tired: "flat",
};

// Punch timeline (seconds from the start of the punch).
const P_WIND = 0.14;
const P_HIT = 0.24;
const P_LAUNCH = 0.22;
const P_HOLD = 0.55;
const P_BACK = 0.85;

// ── Small helpers ─────────────────────────────────────────────────────────────

const now = () => performance.now() / 1000;

const smoothstep = (t: number) => {
  const k = clamp(t, 0, 1);
  return k * k * (3 - 2 * k);
};

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

function heartPath(x: CanvasRenderingContext2D, s: number) {
  x.beginPath();
  x.moveTo(0, s * 0.38);
  x.bezierCurveTo(-s * 1.05, -s * 0.15, -s * 0.5, -s * 0.95, 0, -s * 0.38);
  x.bezierCurveTo(s * 0.5, -s * 0.95, s * 1.05, -s * 0.15, 0, s * 0.38);
  x.closePath();
}

/** A four-point sparkle (the concept sheet's "excited" stars). */
function sparklePath(x: CanvasRenderingContext2D, r: number) {
  const k = r * 0.22;
  x.beginPath();
  x.moveTo(0, -r);
  x.quadraticCurveTo(k, -k, r, 0);
  x.quadraticCurveTo(k, k, 0, r);
  x.quadraticCurveTo(-k, k, -r, 0);
  x.quadraticCurveTo(-k, -k, 0, -r);
  x.closePath();
}

/** Where the lid cuts each almond: inner and outer angles on the ellipse (degrees). */
const CUT = {
  pill: [193, -47], angry: [186, -28], wide: [228, -100], cup: [214, -34], soft: [216, -84],
} as const;

/**
 * The almond eye of the right eye (the left one is drawn mirrored): an oval
 * with its top sliced off along a chord that drops towards the nose — from
 * `cut[0]` (inner side) to `cut[1]` (top, outer side), angles in degrees.
 */
function almondPath(x: CanvasRenderingContext2D, w: number, h: number, cut: readonly [number, number]) {
  const a0 = (cut[1] * Math.PI) / 180;
  const a1 = (cut[0] * Math.PI) / 180;
  const rx = w / 2;
  const ry = h / 2;
  const ix = Math.cos(a1) * rx;
  const iy = Math.sin(a1) * ry;
  const ox = Math.cos(a0) * rx;
  const oy = Math.sin(a0) * ry;
  x.beginPath();
  x.ellipse(0, 0, rx, ry, 0, a0, a1);
  // The lid: a chord bowed very slightly outwards.
  const mx = (ix + ox) / 2;
  const my = (iy + oy) / 2;
  x.quadraticCurveTo(mx + (oy - iy) * 0.08, my - (ox - ix) * 0.08, ox, oy);
  x.closePath();
}

/** Samples a cubic (flat [x0,y0,…,x3,y3]) at s. */
function cubicAt(p: readonly number[], s: number): [number, number] {
  const u = 1 - s;
  const a = u * u * u;
  const b = 3 * u * u * s;
  const c = 3 * u * s * s;
  const d = s * s * s;
  return [a * p[0] + b * p[2] + c * p[4] + d * p[6], a * p[1] + b * p[3] + c * p[5] + d * p[7]];
}

// ── Engine ────────────────────────────────────────────────────────────────────

export class BotEngine {
  isMini = false;
  /** Head colour for mini bots / integration pills (null = Zuko's own white). */
  bodyColor: RGB | null = null;

  // Animated state
  yaw = 0; pitch = 0; roll = 0; tilt = 0; open = 1;
  sx = 1; sy = 1; oy = 0; ox = 0;
  tint = BOT_STATES.idle.tint; morph = 0; es = 1; badgeS = 0;
  /** Emote glow: brighter eyes (love, proud, happy). */
  boost = 0;
  /** Transient flare-up on alerts: hotter eyes and a burst of aura, 0…1. */
  flare = 0;
  /** Eye ignition: 0 = dark eyes, 1 = glowing. The left eye lights first. */
  boot = 1;
  /** Multiplies every flame on the body (aura, ring); 0 = no fire at all. */
  ignite = 1;

  /** Smoothed aura level round the head, and the hover ring under the feet. */
  flameLevel = 0;
  ringLevel = 0;
  /** Smoothed eye brightness; follows the state. */
  glow = 1;
  /** Smoothed fire palette mix towards danger (error) and success (finished). */
  dangerMix = 0;
  successMix = 0;

  /**
   * Animation clock in seconds for posed drawing (greeting, drop sequence,
   * previews). Null = the wall clock. The fire, the hair sway and the looping
   * glyphs read it; tweens always run on the wall clock.
   */
  clock: number | null = null;

  // Targets
  tgYaw = 0; tgPitch = 0; tgTilt = 0; tgSy = 1; tgSx = 1; tgEs = 1;

  /** Extra canvas height above the body so hearts can fly out without clipping. */
  particleOverhang = 0;

  /**
   * Ready-stance spring (the drag-over scan): fists up and burning. The island
   * drives the target; `gulp()` kicks it for the pulse on a drop.
   */
  slotH = 0; slotHTarget = 0; slotHVel = 0;
  /** True for a moment after a scan pulse: the eyes show a pleased "^ ^". */
  isScanning = false;

  // Posed punch (greeting, drop sequence, previews): extension 0…1, direction
  // in radians (0 = right) and the flame on that fist 0…1.
  punch = 0;
  punchAngle = 0;
  fistFire = 0;

  /** Where this bot's canvas sits on the effects layer (CSS px). */
  fxOrigin: FxPoint = { x: 0, y: 0 };

  col: RGB = LED.ember;
  colT: RGB = LED.ember;

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

  /** Blink now and then (dev stills turn it off). */
  autoBlink = true;

  lookX = 0;
  lookY = 0;

  lastTime = now();
  private t0 = now() - Math.random() * 5;
  /** Per-instance phase so neighbouring mini bots never flicker in step. */
  flickerSeed = Math.random() * 100;
  private nextBlink = now() + 1.5 + Math.random() * 2;
  private greetToken = 0;
  private lastAmbient = 0;
  private slapTimes: number[] = [];
  private miniLookTarget = { x: 0, y: 0 };
  private miniLookNextTime = 0;

  // Fire
  private auraOn = false;
  private ringOn = false;
  private fx: FxItem[] = [];
  private fxSeed = 1;
  private lastFxT0 = 0;
  /** Effects-layer positions from the last draw: figure origin, fists. */
  private anchor: { x: number; y: number; R: number } | null = null;
  private fists: [Fist | null, Fist | null] = [null, null];

  // Ponytail spring (radians) and the head motion that drives it.
  hairA = 0;
  private hairV = 0;
  private prevHeadX = 0;
  private prevHeadY = 0;

  /** Fired when the slaps pile up (six inside 2.2 s → dizzy + confused view). */
  onDizzy: (() => void) | null = null;
  /** Fired on a fast triple slap (the island answers with a fire punch). */
  onTripleSlap: (() => void) | null = null;

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
        this.anim("oy", [[-0.35, 300, Ease.out], [0, 450, Ease.back]]);
        this.flareUp(0.6);
        setTimeout(() => this.emit("star", 4), 450);
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
        this.flareUp(0.8);
        break;
      case "dizzy":
        this.doRoll(1300, 2);
        break;
      case "question":
        this.blink();
        this.flareUp(0.4);
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
    this.flameLevel = Math.max(this.cfg.aura, this.auraOn ? 1 : 0);
    this.ringLevel = this.ringOn ? 1 : 0;
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
    this.hairA = 0; this.hairV = 0;
  }

  blink() {
    if (this.locks.has("open")) return;
    this.anim("open", [[0.06, 70, Ease.inOut], [1, 130, Ease.out]]);
  }

  squash() {
    this.anim("sy", [[0.8, 70, Ease.out], [1.1, 130, Ease.out], [1, 170, Ease.inOut]]);
    this.anim("sx", [[1.14, 70, Ease.out], [0.95, 130, Ease.out], [1, 170, Ease.inOut]]);
  }

  /** The pulse on a drop: fists flare, then the eyes go "^ ^". */
  gulp() {
    this.slotHTarget = 0.42;
    setTimeout(() => {
      this.slotHTarget = 0;
      this.isScanning = true;
      setTimeout(() => { this.isScanning = false; }, 800);
    }, 460);
    this.anim("sy", [[0.84, 80, Ease.out], [1.1, 130, Ease.out], [1, 220, Ease.back]]);
    this.anim("sx", [[1.15, 80, Ease.out], [0.95, 130, Ease.out], [1, 220, Ease.back]]);
    this.flareUp(0.6);
    this.blink();
  }

  /**
   * A click on Zuko. One or two: annoyed. A fast triple: `onTripleSlap` (the
   * island throws a fire punch). Keep going to six and he gets dizzy.
   */
  slap() {
    this.interruptGreet();
    if (this.state === "dizzy") return;
    const t = now();
    this.slapTimes = this.slapTimes.filter((s) => t - s < 2.2);
    this.slapTimes.push(t);
    Sound.play("slap");
    this.squash();
    const n = this.slapTimes.length;
    if (n >= 6 || (n >= 3 && !this.onTripleSlap)) {
      this.slapTimes = [];
      this.onDizzy?.();
      return;
    }
    if (n === 3 && t - this.slapTimes[0] < 1.2) {
      this.onTripleSlap?.();
      return;
    }
    this.eyeOverride = "line";
    this.eyeOverrideUntil = t + 0.8;
    setTimeout(() => Sound.play("annoyed"), 60);
  }

  /** A flip: the whole figure turns `turns` times round its middle. */
  doRoll(durationMs: number, turns: number) {
    this.roll = 0;
    this.anim("roll", [[Math.PI * 2 * turns, durationMs, Ease.inOut]], () => { this.roll = 0; });
  }

  /** Flare-up: hotter eyes and a burst of aura that dies back down. */
  flareUp(amount = 1) {
    this.anim("flare", [[amount, 140, Ease.out], [0, 900, Ease.inOut]]);
  }

  /** Hello: the eyes ignite, a swirl of fire, then a pleased look. */
  greet() {
    const t = now();
    const tok = ++this.greetToken;
    this.boot = 0;
    this.anim("boot", [[0, 120, Ease.lin], [1, 560, Ease.inOut]]);
    this.anim("oy", [[-0.06, 220, Ease.out], [0.0, 220, Ease.back]]);
    Sound.play("greet");
    this.fireSwirl();

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
        this.anim("es", [[1.22, 120, Ease.out], [1, 500, Ease.inOut]]);
        this.flareUp(0.7);
        break;
      case "proud":
        this.emit("star", 5);
        this.anim("tilt", [
          [-0.14, 220, Ease.out], [-0.14, (duration - 0.5) * 1000, Ease.lin], [0, 280, Ease.inOut],
        ]);
        this.anim("boost", [
          [0.7, 250, Ease.out], [0.7, (duration - 0.5) * 1000, Ease.lin], [0, 300, Ease.inOut],
        ]);
        this.flareUp(0.5);
        break;
      case "wink":
        this.anim("tilt", [
          [0.12, 160, Ease.out], [0.12, (duration - 0.4) * 1000, Ease.lin], [0, 240, Ease.inOut],
        ]);
        break;
      case "yawn":
        this.anim("sy", [[1.1, 500, Ease.inOut], [1, 500, Ease.inOut]]);
        this.anim("sx", [[0.95, 500, Ease.inOut], [1, 500, Ease.inOut]]);
        setTimeout(() => { this.eyeOverride = "closed"; this.emit("z", 2); }, 700);
        break;
      case "happy":
        this.anim("boost", [[0.6, 200, Ease.out], [0, 600, Ease.inOut]]);
        this.anim("oy", [[-0.12, 140, Ease.out], [0, 320, Ease.back]]);
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

  /** The drag-over "ready" stance: fists up, eyes eager. */
  animateMorph(target: number, durationMs?: number) {
    const dur = durationMs ?? (target > 0.5 ? 450 : 550);
    this.anim("morph", [[target, dur, Ease.inOut]]);
  }

  resetMorph() {
    this.tweens.delete("morph");
    this.locks.delete("morph");
    this.morph = 0;
  }

  // ── Fire ────────────────────────────────────────────────────────────────────

  /** The fire clock: `clock` when posed or frozen, else the wall clock. */
  private fxTime(): number {
    return this.clock ?? now();
  }

  /**
   * Fire punch: the arm cocks back, the fist ignites, the punch lands with a
   * swirl of flame and a fireball flies to `target` (effects-layer pixels),
   * leaving an ember trail, and bursts into sparks there. Without a target the
   * fireball flies `opts.distance` towards `opts.angle`.
   */
  shootFire(target: FxPoint | null, opts: FireOptions = {}) {
    const a = this.anchor;
    let ang = opts.angle ?? 0;
    let dist = opts.distance ?? (a ? a.R * 5 : 140);
    if (target && a) {
      ang = Math.atan2(target.y - (a.y - a.R * 0.1), target.x - a.x);
      dist = Math.hypot(target.x - a.x, target.y - a.y);
    }
    const flight = clamp(dist / 640, 0.16, 0.5);
    const t0 = this.fxTime() + (opts.delay ?? 0);
    this.lastFxT0 = t0;
    this.fx.push({
      kind: "punch", t0, dur: Math.max(P_BACK + 0.1, P_LAUNCH + flight + 0.55),
      side: Math.cos(ang) >= 0 ? 1 : -1, ang, target, dist, flight,
      pal: opts.kind ?? "ember", seed: this.fxSeed++, onImpact: opts.onImpact,
      sound: opts.sound ?? true, fired: 0, from: null,
    });
    this.interruptGreet();
    if (opts.sound ?? true) Sound.play("fire");
  }

  /** A quick puff of flame off the fist, towards `target` if given. */
  fireFlick(target: FxPoint | null = null, opts: FireOptions = {}) {
    const a = this.anchor;
    let ang = opts.angle ?? -0.35;
    let dist = opts.distance ?? (a ? a.R * 1.2 : 30);
    if (target && a) {
      ang = Math.atan2(target.y - a.y, target.x - a.x);
      dist = Math.hypot(target.x - a.x, target.y - a.y);
    }
    const t0 = this.fxTime() + (opts.delay ?? 0);
    this.lastFxT0 = t0;
    this.fx.push({
      kind: "flick", t0, dur: 0.8, side: Math.cos(ang) >= 0 ? 1 : -1, ang, target, dist,
      flight: 0.28, pal: opts.kind ?? "ember", seed: this.fxSeed++, onImpact: opts.onImpact,
      sound: opts.sound ?? true, fired: 0, from: null,
    });
    if (opts.sound ?? true) Sound.play("fire");
  }

  /** A swirl of fire round the whole body (landings, the hello). */
  fireSwirl(opts: FireOptions = {}) {
    const t0 = this.fxTime() + (opts.delay ?? 0);
    this.lastFxT0 = t0;
    this.fx.push({
      kind: "swirl", t0, dur: 0.95, side: 1, ang: 0, target: null, dist: 0, flight: 0,
      pal: opts.kind ?? "ember", seed: this.fxSeed++, sound: false, fired: 0, from: null,
    });
  }

  /** Flames simmering round the head (a risky card waiting, anger). */
  setFireAura(on: boolean) {
    this.auraOn = on;
  }

  /** Hovering on a ring of fire (an agent at work). */
  setFireRing(on: boolean) {
    this.ringOn = on;
  }

  /** True while any fire effect is on screen (the effects layer must be redrawn). */
  get fxActive(): boolean {
    return this.fx.length > 0 || this.fistFireLevel(1) > 0.02 || this.fistFireLevel(-1) > 0.02;
  }

  /**
   * A fist on the effects layer as of the last draw (side −1 = viewer's left),
   * with its arm direction and radius; null for tiny figures without arms.
   */
  fistPosition(side: -1 | 1): Readonly<Fist> | null {
    return this.fists[side < 0 ? 0 : 1];
  }

  /** The figure's origin on the effects layer as of the last draw, and its R. */
  get fxAnchor(): Readonly<{ x: number; y: number; R: number }> | null {
    return this.anchor;
  }

  /** Dev only: holds the fire at `at` seconds into the last effect started. */
  freezeFx(at: number) {
    this.clock = this.lastFxT0 + at;
  }

  // ── Activity ────────────────────────────────────────────────────────────────

  /** Anything other than the ambient hair sway and flame flicker is moving. */
  private get moving(): boolean {
    return (
      this.tweens.size > 0 ||
      this.particles.length > 0 ||
      this.fx.length > 0 ||
      this.cfg.bounces || this.cfg.scans || this.cfg.breathes || this.cfg.zz || this.cfg.sweat ||
      this.isMini ||
      Math.abs(this.tgYaw - this.yaw) > 0.002 ||
      Math.abs(this.tgPitch - this.pitch) > 0.002 ||
      Math.abs(this.tgTilt - this.tilt) > 0.002 ||
      Math.abs(this.tgSy - this.sy) > 0.002 ||
      Math.abs(this.tgSx - this.sx) > 0.002 ||
      Math.abs(this.tgEs - this.es) > 0.002 ||
      Math.abs(this.auraTarget - this.flameLevel) > 0.004 ||
      Math.abs((this.ringOn ? 1 : 0) - this.ringLevel) > 0.004 ||
      Math.abs(this.cfg.glow - this.glow) > 0.004 ||
      Math.abs(this.hairV) > 0.03 ||
      this.slotH > 0.001 || Math.abs(this.slotHVel) > 0.001 ||
      Math.abs(this.col[0] - this.colT[0]) > 0.003 ||
      Math.abs(this.col[1] - this.colT[1]) > 0.003 ||
      Math.abs(this.col[2] - this.colT[2]) > 0.003
    );
  }

  /** The ponytail sways and the eyes shimmer as long as Zuko is awake at all. */
  private get ambient(): boolean {
    return this.glow > 0.01;
  }

  /**
   * True while anything is moving — lets the island stop its RAF loop. Zuko is
   * never quite still (hair, eye shimmer, simmering flames), so see
   * `ambientOnly` for a caller that wants to drop its frame rate meanwhile.
   */
  get busy(): boolean {
    return this.ambient || this.moving;
  }

  /** True when only the ambient sway and flicker are animating (~15 fps is plenty). */
  get ambientOnly(): boolean {
    return this.ambient && !this.moving;
  }

  private get auraTarget(): number {
    return Math.max(this.cfg.aura, this.auraOn ? 1 : 0);
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
      const amp = this.isMini ? 0.06 : 0.03;
      this.tgSy = 1 + Math.sin(t * 1.8) * amp;
      this.tgSx = 1 - Math.sin(t * 1.8) * amp * 0.57;
    } else if (this.isMini) {
      this.tgSy = 1 + Math.sin(t * 2.2) * 0.035;
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

    // Eyes, aura and ring ease towards the state over about half a second.
    const kSlow = 1 - Math.pow(0.004, dt);
    this.glow += (this.cfg.glow - this.glow) * kSlow;
    this.flameLevel += (this.auraTarget - this.flameLevel) * kSlow;
    this.ringLevel += ((this.ringOn ? 1 : 0) - this.ringLevel) * kSlow;
    this.dangerMix += ((this.cfg.fire === "danger" ? 1 : 0) - this.dangerMix) * kSlow;
    this.successMix += ((this.cfg.fire === "success" ? 1 : 0) - this.successMix) * kSlow;

    if (this.autoBlink && n > this.nextBlink) {
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

    // Ready-stance spring — ω₀ = 2π/0.25, ζ = 0.6
    const omega = (2 * Math.PI) / 0.25;
    const zeta = 0.6;
    const acc = omega * omega * (this.slotHTarget - this.slotH) - 2 * zeta * omega * this.slotHVel;
    this.slotHVel += acc * dt;
    this.slotH = Math.max(0, this.slotH + this.slotHVel * dt);

    this.updateFire(dt);
    this.lastTime = n;
  }

  /** Fire sounds and impacts, the ponytail spring, and expiring effects. */
  private updateFire(dt: number) {
    const ft = this.fxTime();
    for (const it of this.fx) {
      const e = ft - it.t0;
      if (it.kind === "punch") {
        if (!(it.fired & 1) && e >= P_LAUNCH - 0.02) {
          it.fired |= 1;
          if (it.sound) Sound.play("fireball");
        }
        if (!(it.fired & 2) && e >= P_LAUNCH + it.flight) {
          it.fired |= 2;
          if (it.sound) Sound.play("burst");
          it.onImpact?.();
        }
      } else if (it.kind === "flick" && !(it.fired & 2) && e >= 0.14 + it.flight) {
        it.fired |= 2;
        it.onImpact?.();
      }
    }
    if (this.fx.length) this.fx = this.fx.filter((it) => ft - it.t0 < it.dur);

    // The ponytail lags behind the head: a damped spring driven by how fast the
    // head moves sideways (and a little by hops).
    if (dt > 0) {
      const pz = this.punchPose(ft);
      const hx = this.ox + this.tilt * 0.8 + pz.lunge + pz.lean * 0.8;
      const hy = this.oy;
      const vx = (hx - this.prevHeadX) / dt;
      const vy = (hy - this.prevHeadY) / dt;
      this.prevHeadX = hx;
      this.prevHeadY = hy;
      const a = -90 * this.hairA - 9 * this.hairV - clamp(vx, -8, 8) * 4 + clamp(vy, -8, 8) * 1.5;
      this.hairV += a * Math.min(dt, 0.033);
      this.hairA = clamp(this.hairA + this.hairV * Math.min(dt, 0.033), -0.7, 0.7);
      if (Math.abs(this.hairA) < 0.0005 && Math.abs(this.hairV) < 0.005) {
        this.hairA = 0;
        this.hairV = 0;
      }
    }
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

  // ── Pose ────────────────────────────────────────────────────────────────────

  /** The punching arm this frame: from a live punch/flick, else the posed fields. */
  private punchPose(ft: number): PunchPose {
    for (let i = this.fx.length - 1; i >= 0; i--) {
      const it = this.fx[i];
      if (it.kind === "swirl") continue;
      const e = ft - it.t0;
      if (e < 0 || e > P_BACK + 0.15) continue;
      const dirX = Math.cos(it.ang);
      let aim = 0, ext = 1, fire = 0, lean = 0, lunge = 0;
      if (it.kind === "punch") {
        if (e < P_WIND) {
          const k = easeOut(e / P_WIND);
          aim = k * 0.55; ext = lerp(1, 0.6, k); lean = -0.1 * k; lunge = -0.04 * k; fire = 0.6 * k;
        } else if (e < P_HIT) {
          const k = easeOut((e - P_WIND) / (P_HIT - P_WIND));
          aim = lerp(0.55, 1, k); ext = lerp(0.6, 2.5, k); lean = lerp(-0.1, 0.14, k);
          lunge = lerp(-0.04, 0.13, k); fire = lerp(0.6, 1, k);
        } else if (e < P_HOLD) {
          const k = (e - P_HIT) / (P_HOLD - P_HIT);
          aim = 1; ext = 2.5 - 0.15 * Math.sin(Math.PI * k); lean = 0.14; lunge = 0.13; fire = 1 - 0.55 * k;
        } else {
          const k = easeInOut((e - P_HOLD) / (P_BACK - P_HOLD));
          aim = 1 - k; ext = lerp(2.5, 1, k); lean = 0.14 * (1 - k); lunge = 0.13 * (1 - k); fire = 0.45 * (1 - k);
        }
      } else {
        // Flick: a short jab and a puff.
        const k1 = easeOut(e / 0.12);
        const k2 = easeInOut((e - 0.35) / 0.3);
        aim = Math.min(1, k1) * (1 - k2);
        ext = 1 + 0.9 * Math.min(1, k1) * (1 - k2);
        fire = Math.sin(Math.PI * clamp((e - 0.04) / 0.5, 0, 1));
        lean = 0.06 * aim;
        lunge = 0.04 * aim;
      }
      return {
        active: true, side: it.side, ang: it.ang, aim, ext, fire,
        lean: lean * dirX, lunge: lunge * dirX, angry: it.kind === "punch" && e < 0.95,
        aura: it.kind === "punch" ? 0.55 * Math.sin(Math.PI * clamp(e / 0.8, 0, 1)) : 0,
        kind: it.pal,
      };
    }
    const p = clamp(this.punch, 0, 1);
    return {
      active: p > 0.001, side: Math.cos(this.punchAngle) >= 0 ? 1 : -1, ang: this.punchAngle,
      aim: Math.min(1, p * 1.6), ext: lerp(1, 2.5, p), fire: this.fistFire,
      lean: 0, lunge: 0, angry: false, aura: 0, kind: "ember",
    };
  }

  private firePalette(kind?: FireKind): FirePalette {
    const own = mixFire(mixFire(FIRE.ember, FIRE.danger, this.dangerMix), FIRE.success, this.successMix);
    return kind && kind !== "ember" ? FIRE[kind] : own;
  }

  /** Flame on a fist, 0…1: the ready stance, a punch, or the posed field. */
  private fistFireLevel(sd: number): number {
    const stance = clamp(this.morph, 0, 1) * clamp(this.slotH * 3, 0, 1);
    const pz = this.punchPose(this.fxTime());
    const punching = pz.active && pz.side === sd ? pz.fire : 0;
    return Math.max(stance, punching);
  }

  private currentEye(pz: PunchPose): EyeShape {
    let shape: EyeShape = this.eyeOverride ?? this.cfg.eye;
    if (this.morph > 0.5) {
      if (this.isScanning) shape = "happy";
      else if (this.slotHTarget > 0.05 || this.slotH > 0.1) shape = "cup";
    }
    if (pz.angry && this.state !== "dizzy") shape = "angry";
    return shape;
  }

  private currentMouth(shape: EyeShape, pz: PunchPose): Mouth {
    if (pz.angry && this.state !== "dizzy") return "grit";
    if (this.eyeOverride || shape !== this.cfg.eye) return EYE_MOUTH[shape] ?? "none";
    return this.cfg.mouth;
  }

  // ── Draw ────────────────────────────────────────────────────────────────────

  /**
   * Draws Zuko into a canvas of `w`×`h` CSS pixels (the caller has already
   * applied the DPR transform). The nominal body box is `0.6 × w` across,
   * centred in the bottom `w`×`w` square.
   */
  draw(x: CanvasRenderingContext2D, W: number, H: number) {
    const R = W * 0.3;
    this.render(x, W / 2 + this.ox * R, H / 2 + this.particleOverhang / 2 + this.oy * R + R * 0.06, R);
  }

  /**
   * Draws Zuko laid out around (`cx`, `cy`) — the same point the island places
   * its bot canvas on — with a nominal body box `2R` across. Posed renderers.
   */
  drawAt(x: CanvasRenderingContext2D, cx: number, cy: number, R: number) {
    this.render(x, cx + this.ox * R, cy + this.oy * R + R * 0.06, R);
  }

  private render(x: CanvasRenderingContext2D, cx: number, cy: number, R: number) {
    if (R <= 0.5) return;
    const t = (this.clock ?? now()) + this.flickerSeed;
    const ft = this.fxTime();
    const pz = this.punchPose(ft);
    const tier: Tier = this.isMini || R < 8 ? 0 : R < 18 ? 1 : 2;
    const u = R * (tier === 0 ? 0.88 : 0.74);
    const hy = tier === 0 ? R * 0.12 : -R * 0.08;
    const pal = this.firePalette(pz.active ? pz.kind : undefined);
    const ring = tier > 0 ? this.ringLevel * this.ignite : 0;
    const lift = ring > 0.001 ? (-0.15 * R + Math.sin(t * 2.4) * 0.035 * R) * ring : 0;

    const baseM = x.getTransform();
    x.save();
    x.translate(cx + pz.lunge * R, cy);
    const rot = this.tilt + pz.lean;
    if (rot) x.rotate(rot);
    const pivot = hy + 1.5 * u;
    x.translate(0, pivot);
    x.scale(this.sx, this.sy);
    x.translate(0, -pivot);

    if (ring > 0.01) this.drawRing(x, u, hy, t, pal, ring, false);
    x.save();
    if (lift) x.translate(0, lift);
    if (this.roll) {
      const py = hy + 0.35 * u;
      x.translate(0, py);
      x.rotate(this.roll);
      x.translate(0, -py);
    }
    this.drawFigure(x, u, hy, t, tier, pz, pal, baseM);
    x.restore();
    if (ring > 0.01) this.drawRing(x, u, hy, t, pal, ring, true);
    x.restore();

    // Where the figure sits on the effects layer.
    this.anchor = { x: this.fxOrigin.x + cx, y: this.fxOrigin.y + cy, R };

    if (this.badge && this.badgeS > 0.01 && this.morph < 0.25) {
      this.drawBadge(x, this.badge, R, cx, cy, t);
    }
    this.drawParticles(x, R, cx, cy);
  }

  private drawFigure(
    x: CanvasRenderingContext2D, u: number, hy: number, t: number,
    tier: Tier, pz: PunchPose, pal: FirePalette, baseM: DOMMatrix,
  ) {
    const yawN = clamp(this.yaw / 0.62, -1.3, 1.3);
    const pitchN = clamp(this.pitch / 0.5, -1.3, 1.3);
    const fx = yawN * 0.17 * u;
    const fy = -pitchN * 0.13 * u;
    const aura = tier > 0
      ? clamp(Math.max(this.flameLevel, this.flare * 0.55, pz.aura), 0, 1.3) * this.ignite
      : 0;

    if (aura > 0.01) this.drawAura(x, u, hy, t, pal, aura, tier);
    this.drawHair(x, u, fx * 0.35, hy, t, tier);
    if (tier > 0) {
      this.drawBody(x, u, hy, t, tier, pz, baseM);
    } else {
      this.fists = [null, null];
      if (!this.isMini) this.drawCollar(x, u, hy);
    }
    this.drawHead(x, u, hy, tier);
    this.drawFace(x, u, hy, fx, fy, t, tier, pz, yawN);
    this.drawTie(x, u, fx * 0.35, hy, tier);
  }

  // ── Hair ────────────────────────────────────────────────────────────────────

  /** The ponytail, behind the head: a few tapered locks that sway as one. */
  private drawHair(x: CanvasRenderingContext2D, u: number, ox: number, hy: number, t: number, tier: Tier) {
    const qx = ox;
    const qy = hy - 1.28 * u;
    const sway = this.hairA + 0.055 * Math.sin(t * 1.3) + 0.025 * Math.sin(t * 2.9 + 1);
    const N = tier === 2 ? 12 : 8;
    const left: number[] = [];
    const right: number[] = [];
    // Near-black hair with a warm rim, so it still reads on the black island.
    const hg = x.createLinearGradient(qx, qy - 0.8 * u, qx + 1.1 * u, qy + 0.7 * u);
    hg.addColorStop(0, "#4A322B");
    hg.addColorStop(0.5, "#33221E");
    hg.addColorStop(1, "#24170F");
    // The locks are outlined as one silhouette (all strokes, then all fills),
    // so the rim traces the outside of the swoosh, not every strand.
    const shapes: Path2D[] = [];
    for (const lock of LOCKS) {
      if (lock.tier > tier) continue;
      left.length = 0;
      right.length = 0;
      let px = 0, py = 0;
      for (let i = 0; i <= N; i++) {
        const s = i / N;
        const [lx, ly] = cubicAt(lock.p, s);
        // Bend: points further down the lock turn further round the root.
        const a = sway * lock.flex * Math.pow(s, 1.4);
        const ca = Math.cos(a);
        const sa = Math.sin(a);
        const X = qx + (lx * ca - ly * sa) * u;
        const Y = qy + (lx * sa + ly * ca) * u;
        const [nx2, ny2] = cubicAt(lock.p, Math.min(1, s + 0.02));
        const [nx1, ny1] = cubicAt(lock.p, Math.max(0, s - 0.02));
        let tx = (nx2 - nx1) * ca - (ny2 - ny1) * sa;
        let ty = (nx2 - nx1) * sa + (ny2 - ny1) * ca;
        const l = Math.hypot(tx, ty) || 1;
        tx /= l;
        ty /= l;
        const w = lock.w * (tier === 0 ? 1.45 : 1) * u * (0.55 + 0.75 * Math.sin(Math.PI * s * 0.85)) * (1 - s * s * s);
        left.push(X - ty * w, Y + tx * w);
        right.push(X + ty * w, Y - tx * w);
        px = X;
        py = Y;
      }
      const path = new Path2D();
      path.moveTo(left[0], left[1]);
      for (let i = 2; i < left.length; i += 2) path.lineTo(left[i], left[i + 1]);
      path.lineTo(px, py);
      for (let i = right.length - 2; i >= 0; i -= 2) path.lineTo(right[i], right[i + 1]);
      path.closePath();
      shapes.push(path);
    }
    x.lineJoin = "round";
    x.lineWidth = Math.max(0.8, (tier === 0 ? 0.06 : 0.045) * u);
    x.strokeStyle = "rgba(160,104,80,0.8)";
    for (const p of shapes) x.stroke(p);
    x.fillStyle = hg;
    for (const p of shapes) x.fill(p);
    // Glossy sheen along the main lock and the top one.
    if (tier === 2) {
      const lock = LOCKS[0];
      x.strokeStyle = "rgba(150,100,80,0.6)";
      x.lineWidth = Math.max(0.6, 0.035 * u);
      x.lineCap = "round";
      x.beginPath();
      for (let i = 1; i <= 7; i++) {
        const s = 0.06 + (i / 7) * 0.5;
        const [lx, ly] = cubicAt(lock.p, s);
        const a = sway * lock.flex * Math.pow(s, 1.4);
        const X = qx + (lx * Math.cos(a) - ly * Math.sin(a)) * u - 0.05 * u;
        const Y = qy + (lx * Math.sin(a) + ly * Math.cos(a)) * u + 0.02 * u;
        if (i === 1) x.moveTo(X, Y);
        else x.lineTo(X, Y);
      }
      x.stroke();
    }
  }

  /** The red hair-tie standing on the crown, with the gathered hair under it. */
  private drawTie(x: CanvasRenderingContext2D, u: number, ox: number, hy: number, tier: Tier) {
    const top = hy - 1.3 * u;
    const bot = hy - 0.93 * u;
    const hw = (tier === 0 ? 0.19 : 0.115) * u;
    const lw = Math.max(0.6, (tier === 0 ? 0.05 : 0.03) * u);

    x.fillStyle = INK_S;
    x.beginPath();
    x.ellipse(ox, hy - 0.965 * u, (tier === 0 ? 0.2 : 0.17) * u, 0.07 * u, 0, 0, TAU);
    x.fill();

    const g = x.createLinearGradient(ox - hw, 0, ox + hw, 0);
    g.addColorStop(0, "#7A2219");
    g.addColorStop(0.32, "#D4523F");
    g.addColorStop(0.6, "#B33A2E");
    g.addColorStop(1, "#5E1A14");
    roundRectPath(x, ox - hw, top, hw * 2, bot - top, hw * 0.4);
    x.fillStyle = g;
    x.fill();
    if (tier > 0) {
      x.fillStyle = GOLD;
      const bh = Math.max(0.6, 0.036 * u);
      x.fillRect(ox - hw, top + 0.055 * u - bh / 2, hw * 2, bh);
      x.fillRect(ox - hw, bot - 0.07 * u - bh / 2, hw * 2, bh);
    }
    x.lineWidth = lw;
    x.strokeStyle = tier === 0 ? "rgba(43,29,26,0.5)" : "rgba(43,29,26,0.85)";
    roundRectPath(x, ox - hw, top, hw * 2, bot - top, hw * 0.4);
    x.stroke();
  }

  // ── Head ────────────────────────────────────────────────────────────────────

  private drawHead(x: CanvasRenderingContext2D, u: number, hy: number, tier: Tier) {
    const tinted = this.isMini && this.bodyColor ? this.bodyColor : null;
    const cTop = tinted ? mix3(tinted, WHITE, 0.55) : WHITE;
    const cMid = tinted ? mix3(tinted, WHITE, 0.12) : CREAM;
    const cEdge = tinted ? mix3(tinted, BLACK, 0.18) : BEIGE;
    const cRim = tinted ? mix3(tinted, BLACK, 0.38) : SHADE;
    const g = x.createRadialGradient(-0.36 * u, hy - 0.42 * u, 0.04 * u, -0.08 * u, hy - 0.06 * u, 1.12 * u);
    g.addColorStop(0, rgba(cTop));
    g.addColorStop(0.42, rgba(cMid));
    g.addColorStop(0.8, rgba(cEdge));
    g.addColorStop(1, rgba(cRim));
    x.fillStyle = g;
    x.beginPath();
    x.arc(0, hy, u, 0, TAU);
    x.fill();

    if (tier > 0) {
      x.save();
      x.beginPath();
      x.arc(0, hy, u, 0, TAU);
      x.clip();
      // Warm rim light from the fire and the eyes, lower right.
      const warm = clamp(this.tint * this.glow + this.flameLevel * 0.5 + this.flare * 0.3, 0, 1.2);
      const rim = mix3(this.col, hexToRGB("#F28A1E"), 0.5);
      const rg = x.createLinearGradient(-u, hy - u, u, hy + u);
      rg.addColorStop(0, rgba(rim, 0));
      rg.addColorStop(0.55, rgba(rim, 0.08 * warm));
      rg.addColorStop(1, rgba(rim, 0.42 * warm));
      x.lineWidth = 0.2 * u;
      x.strokeStyle = rg;
      x.beginPath();
      x.arc(0, hy, u, 0, TAU);
      x.stroke();
      x.restore();
    }

    // Specular highlight, top-left.
    x.save();
    x.translate(-0.4 * u, hy - 0.5 * u);
    x.rotate(-0.65);
    x.scale(1, 0.56);
    const hg = x.createRadialGradient(0, 0, 0, 0, 0, 0.3 * u);
    hg.addColorStop(0, "rgba(255,255,255,0.95)");
    hg.addColorStop(1, "rgba(255,255,255,0)");
    x.fillStyle = hg;
    x.beginPath();
    x.arc(0, 0, 0.3 * u, 0, TAU);
    x.fill();
    if (tier === 2) {
      x.fillStyle = "rgba(255,255,255,0.9)";
      x.beginPath();
      x.arc(-0.06 * u, 0, 0.08 * u, 0, TAU);
      x.fill();
    }
    x.restore();

    x.lineWidth = Math.max(0.6, (tier === 0 ? 0.075 : tier === 1 ? 0.05 : 0.032) * u);
    x.strokeStyle = tinted ? rgba(mix3(tinted, BLACK, 0.62), 0.85) : "rgba(43,29,26,0.6)";
    x.beginPath();
    x.arc(0, hy, u, 0, TAU);
    x.stroke();
  }

  // ── Face ────────────────────────────────────────────────────────────────────

  private drawFace(
    x: CanvasRenderingContext2D, u: number, hy: number, fx: number, fy: number,
    t: number, tier: Tier, pz: PunchPose, yawN: number,
  ) {
    const shape = this.currentEye(pz);
    const [ew0, eh0, esp, eyy] = EYES[tier];
    const ew = ew0 * u * this.es;
    const eh = eh0 * u * this.es;
    const cyE = hy + fy + eyy * u;

    x.save();
    x.beginPath();
    x.arc(0, hy, u, 0, TAU);
    x.clip();

    // The scar sits round the right eye (Zuko's left).
    if (!this.isMini) {
      const sx = fx + esp * u * (1 - 0.08 * yawN);
      x.save();
      x.translate(sx + 0.05 * u, cyE - 0.07 * u);
      x.scale(u * 1.32, u * 1.32);
      x.fillStyle = "rgba(168,50,38,0.55)";
      x.fill(SCAR);
      if (tier > 0) {
        x.lineWidth = Math.max(0.5 / u, 0.026);
        x.strokeStyle = "rgba(122,32,22,0.5)";
        x.stroke(SCAR);
      }
      if (tier === 2) {
        x.fillStyle = "rgba(255,214,200,0.22)";
        x.fill(SCAR_STREAK);
      }
      x.restore();
    }

    let lit = this.glow * (1 + this.boost * 0.3 + this.flare * 0.25);
    if (this.cfg.pulse) lit *= 0.82 + 0.22 * Math.sin(t * 6.3);
    else lit *= 0.97 + 0.03 * Math.sin(t * 3.1);
    for (const sd of [-1, 1] as const) {
      const on = this.boot >= 1 ? 1 : smoothstep((this.boot - (sd < 0 ? 0 : 0.3)) / 0.55);
      const persp = 1 - 0.2 * clamp(sd * yawN, 0, 1);
      const ex = fx + sd * esp * u * (1 - 0.08 * sd * yawN);
      this.drawEye(x, shape, ex, cyE, ew * persp, eh, sd, t, lit * on, on, tier);
    }

    if (tier > 0 || !this.isMini) this.drawMouth(x, this.currentMouth(shape, pz), fx, hy + fy + 0.52 * u, u, tier);
    x.restore();
  }

  /**
   * One eye at (`ex`, `ey`). Glyphs are defined for the right eye and mirrored
   * for the left. `a` is the glow (0 = unlit dark eye), `on` the ignition.
   */
  private drawEye(
    x: CanvasRenderingContext2D, shape: EyeShape, ex: number, ey: number,
    w: number, h: number, sd: -1 | 1, t: number, a: number, on: number, tier: Tier,
  ) {
    const col0 = shape === "heart" ? HEART : shape === "star" ? STAR : this.col;
    const col = mix3(UNLIT, col0, clamp(on, 0, 1));
    const lit = clamp(a, 0, 1.4);
    const mini = this.isMini && this.bodyColor != null;

    x.save();
    x.translate(ex, ey);
    // Bloom: the glow spilling onto the head.
    if (lit > 0.03 && !mini && shape !== "closed") {
      glow(x, 0, h * 0.05, h * (tier === 0 ? 0.95 : 1.1), col0, 0.42 * Math.min(1, lit));
    }
    x.scale(sd, 1);

    const edge = rgba(mix3(EYE_EDGE, UNLIT, 1 - clamp(on, 0, 1)), 0.9);
    const lw = Math.max(0.55, w * (tier === 0 ? 0.12 : 0.08));

    /** Fills the current path with the glowing eye gradient and an edge. */
    const fillEye = (hh: number) => {
      if (mini) {
        x.fillStyle = INK_S;
        x.fill();
        return;
      }
      const g = x.createRadialGradient(-w * 0.06, hh * 0.14, 0, 0, hh * 0.05, Math.max(w, hh) * 0.66);
      g.addColorStop(0, rgba(mix3(col, WHITE, 0.85 * Math.min(1, lit))));
      g.addColorStop(0.42, rgba(mix3(col, WHITE, 0.32 * Math.min(1, lit))));
      g.addColorStop(0.82, rgba(col));
      g.addColorStop(1, rgba(mix3(col, EYE_EDGE, 0.5)));
      x.fillStyle = g;
      x.fill();
      x.lineWidth = lw;
      x.strokeStyle = edge;
      x.stroke();
    };
    /** A glowing stroke: dark edge, colour, hot core. */
    const strokeEye = (width: number, dim = 1) => {
      x.lineCap = "round";
      x.lineJoin = "round";
      if (mini) {
        x.lineWidth = width;
        x.strokeStyle = INK_S;
        x.stroke();
        return;
      }
      x.lineWidth = width + lw * 1.3;
      x.strokeStyle = edge;
      x.stroke();
      x.lineWidth = width;
      x.strokeStyle = rgba(mix3(mix3(col, INK, 1 - dim), WHITE, 0.15));
      x.stroke();
      if (width > 1.6 && dim > 0.6) {
        x.lineWidth = width * 0.38;
        x.strokeStyle = rgba(mix3(col, WHITE, 0.75), Math.min(1, lit));
        x.stroke();
      }
    };
    const almond = (ww: number, hh: number, cut: readonly [number, number], blink = true) => {
      x.save();
      if (blink && this.open < 0.999) {
        x.translate(0, hh * 0.2);
        x.scale(1, Math.max(this.open, 0.08));
        x.translate(0, -hh * 0.2);
      }
      almondPath(x, ww, hh, cut);
      fillEye(hh);
      if (tier === 2 && lit > 0.5 && !mini && this.open > 0.5) {
        x.fillStyle = "rgba(255,255,255,0.85)";
        x.beginPath();
        x.arc(ww * 0.12, hh * 0.02, ww * 0.09, 0, TAU);
        x.fill();
      }
      x.restore();
    };
    const lid = (y: number, ww: number) => {
      if (tier === 0) return;
      x.strokeStyle = INK_S;
      x.lineWidth = Math.max(0.6, ww * 0.09);
      x.lineCap = "round";
      x.beginPath();
      x.moveTo(-ww * 0.55, y + ww * 0.06);
      x.quadraticCurveTo(0, y - ww * 0.12, ww * 0.55, y - ww * 0.04);
      x.stroke();
    };

    switch (shape) {
      case "pill":
        almond(w, h, CUT.pill);
        break;
      case "wide":
        almond(w * 1.1, h * 1.08, CUT.wide);
        break;
      case "angry":
        almond(w * 1.04, h * 0.86, CUT.angry);
        break;
      case "cup":
        almond(w * 1.06, h * 1.02, CUT.cup);
        break;
      case "think":
        if (sd > 0) {
          x.translate(0, h * 0.14);
          almond(w, h * 0.42, CUT.pill, false);
          lid(-h * 0.24, w);
        } else {
          x.translate(0, -h * 0.04);
          almond(w * 1.04, h * 1.04, CUT.soft);
        }
        break;
      case "tired":
        x.translate(0, h * 0.16);
        almond(w, h * 0.56, CUT.cup, false);
        lid(-h * 0.33, w);
        break;
      case "happy":
        x.beginPath();
        x.moveTo(-w * 0.55, h * 0.18);
        x.quadraticCurveTo(0, -h * 0.5, w * 0.55, h * 0.18);
        strokeEye(Math.max(1, w * 0.36));
        break;
      case "closed":
        x.beginPath();
        x.moveTo(-w * 0.55, -h * 0.02);
        x.quadraticCurveTo(0, h * 0.36, w * 0.55, -h * 0.02);
        strokeEye(Math.max(0.9, w * 0.24), 0.55);
        break;
      case "line":
        x.rotate(-0.36);
        x.beginPath();
        x.moveTo(-w * 0.5, 0);
        x.lineTo(w * 0.5, 0);
        strokeEye(Math.max(1, h * 0.26));
        break;
      case "flat":
        x.rotate(-0.22);
        x.beginPath();
        x.moveTo(-w * 0.55, 0);
        x.lineTo(w * 0.55, 0);
        strokeEye(Math.max(1, h * 0.32));
        break;
      case "dot":
        x.beginPath();
        x.arc(0, 0, Math.min(w, h) * 0.36, 0, TAU);
        fillEye(h * 0.6);
        break;
      case "spiral": {
        const rMax = Math.min(w * 0.6, h * 0.55);
        x.beginPath();
        for (let ang = 0; ang < 4.4 * Math.PI; ang += 0.25) {
          const r = rMax * (0.1 + (ang / (4.4 * Math.PI)) * 0.9);
          const aa = ang + t * 9;
          if (ang === 0) x.moveTo(Math.cos(aa) * r, Math.sin(aa) * r);
          else x.lineTo(Math.cos(aa) * r, Math.sin(aa) * r);
        }
        strokeEye(Math.max(0.8, w * 0.16));
        break;
      }
      case "heart":
        x.translate(0, h * 0.06);
        heartPath(x, Math.min(w * 0.72, h * 0.62));
        fillEye(h);
        break;
      case "star":
        x.rotate(t * 1.5);
        sparklePath(x, Math.min(w * 0.75, h * 0.62));
        fillEye(h);
        break;
      case "wink":
        if (sd < 0) {
          almond(w, h, CUT.pill);
        } else {
          x.beginPath();
          x.moveTo(-w * 0.55, h * 0.18);
          x.quadraticCurveTo(0, -h * 0.5, w * 0.55, h * 0.18);
          strokeEye(Math.max(1, w * 0.36));
        }
        break;
    }
    x.restore();
  }

  private drawMouth(x: CanvasRenderingContext2D, m: Mouth, mx: number, my: number, u: number, tier: Tier) {
    if (m === "none") return;
    if (tier === 0 && m !== "grin" && m !== "o") return;
    const w = 0.09 * u;
    x.save();
    x.strokeStyle = INK_S;
    x.fillStyle = INK_S;
    x.lineWidth = Math.max(0.7, 0.042 * u);
    x.lineCap = "round";
    x.lineJoin = "round";
    x.beginPath();
    switch (m) {
      case "smile":
        x.moveTo(mx - w, my - 0.01 * u);
        x.quadraticCurveTo(mx, my + 0.09 * u, mx + w, my - 0.01 * u);
        x.stroke();
        break;
      case "grin":
        x.moveTo(mx - w * 1.15, my - 0.03 * u);
        x.quadraticCurveTo(mx, my + 0.19 * u, mx + w * 1.15, my - 0.03 * u);
        x.closePath();
        x.fill();
        if (tier === 2) {
          x.fillStyle = "#E0675A";
          x.beginPath();
          x.ellipse(mx, my + 0.055 * u, 0.045 * u, 0.022 * u, 0, 0, TAU);
          x.fill();
        }
        break;
      case "frown":
        x.moveTo(mx - w * 0.85, my + 0.045 * u);
        x.quadraticCurveTo(mx, my - 0.04 * u, mx + w * 0.85, my + 0.045 * u);
        x.stroke();
        break;
      case "wavy":
        x.moveTo(mx - w, my);
        x.quadraticCurveTo(mx - w / 2, my - 0.045 * u, mx, my);
        x.quadraticCurveTo(mx + w / 2, my + 0.045 * u, mx + w, my);
        x.stroke();
        break;
      case "o":
        x.ellipse(mx, my + 0.02 * u, 0.045 * u, 0.058 * u, 0, 0, TAU);
        x.fill();
        break;
      case "flat":
        x.moveTo(mx - w * 0.75, my + 0.01 * u);
        x.lineTo(mx + w * 0.75, my + 0.01 * u);
        x.stroke();
        break;
      case "grit":
        // Gritted teeth, as in the reference pose.
        roundRectPath(x, mx - w * 1.1, my - 0.035 * u, w * 2.2, 0.085 * u, 0.03 * u);
        x.fill();
        if (tier === 2) {
          x.strokeStyle = "rgba(255,250,240,0.95)";
          x.lineWidth = Math.max(0.5, 0.022 * u);
          x.beginPath();
          x.moveTo(mx - w * 0.85, my + 0.007 * u);
          x.lineTo(mx + w * 0.85, my + 0.007 * u);
          x.stroke();
        }
        break;
    }
    x.restore();
  }

  // ── Body ────────────────────────────────────────────────────────────────────

  /** Legs, tunic, belt, collar, scarf and arms — everything under the head. */
  private drawBody(
    x: CanvasRenderingContext2D, u: number, hy: number, t: number, tier: Tier,
    pz: PunchPose, baseM: DOMMatrix,
  ) {
    const X = (v: number) => v * u;
    const Y = (v: number) => hy + v * u;
    const lw = Math.max(0.6, 0.03 * u);

    // Legs and boots.
    x.fillStyle = INK_S;
    for (const sd of [-1, 1]) {
      roundRectPath(x, X(sd * 0.2 - 0.1), Y(1.2), X(0.2), X(0.3), X(0.08));
      x.fill();
      x.beginPath();
      x.ellipse(X(sd * 0.22), Y(1.48), X(0.135), X(0.075), 0, 0, TAU);
      x.fillStyle = "#3D2822";
      x.fill();
      x.fillStyle = INK_S;
    }

    // Tunic.
    const tunic = new Path2D();
    tunic.moveTo(X(-0.56), Y(0.6));
    tunic.bezierCurveTo(X(-0.66), Y(0.66), X(-0.75), Y(0.74), X(-0.75), Y(0.88));
    tunic.bezierCurveTo(X(-0.75), Y(1.06), X(-0.7), Y(1.22), X(-0.64), Y(1.36));
    tunic.quadraticCurveTo(X(0), Y(1.45), X(0.64), Y(1.36));
    tunic.bezierCurveTo(X(0.7), Y(1.22), X(0.75), Y(1.06), X(0.75), Y(0.88));
    tunic.bezierCurveTo(X(0.75), Y(0.74), X(0.66), Y(0.66), X(0.56), Y(0.6));
    tunic.closePath();
    const tg = x.createLinearGradient(0, Y(0.6), 0, Y(1.42));
    tg.addColorStop(0, "#A3352A");
    tg.addColorStop(0.5, "#7A2219");
    tg.addColorStop(1, "#541712");
    x.fillStyle = tg;
    x.fill(tunic);

    x.save();
    x.clip(tunic);
    if (tier === 2) {
      // Side shading.
      const sg = x.createLinearGradient(X(-0.75), 0, X(0.75), 0);
      sg.addColorStop(0, "rgba(30,6,4,0.35)");
      sg.addColorStop(0.35, "rgba(30,6,4,0)");
      sg.addColorStop(0.7, "rgba(30,6,4,0)");
      sg.addColorStop(1, "rgba(30,6,4,0.4)");
      x.fillStyle = sg;
      x.fill(tunic);
      // Hem trim and the front split below the belt.
      x.strokeStyle = GOLD;
      x.lineWidth = Math.max(0.6, 0.045 * u);
      x.beginPath();
      x.moveTo(X(-0.66), Y(1.32));
      x.quadraticCurveTo(X(0), Y(1.41), X(0.66), Y(1.32));
      x.stroke();
      x.beginPath();
      x.moveTo(X(0), Y(1.28));
      x.lineTo(X(0), Y(1.42));
      x.stroke();
    }
    // Belt.
    x.fillStyle = "#C98A3A";
    x.fillRect(X(-0.8), Y(1.18), X(1.6), X(0.09));
    if (tier === 2) {
      x.fillStyle = "rgba(70,30,12,0.45)";
      x.fillRect(X(-0.8), Y(1.18), X(1.6), Math.max(0.5, X(0.018)));
      x.fillRect(X(-0.8), Y(1.27) - Math.max(0.5, X(0.018)), X(1.6), Math.max(0.5, X(0.018)));
    }
    x.restore();

    // Belt knot and its tails, swaying with the hair.
    const sway = this.hairA * 0.5 + 0.04 * Math.sin(t * 1.3);
    if (tier === 2) {
      x.fillStyle = "#D9963A";
      x.strokeStyle = "rgba(90,40,14,0.8)";
      x.lineWidth = lw;
      for (const [dx, len, a] of [[0.035, 0.2, 0.18], [-0.035, 0.17, -0.1]] as const) {
        x.save();
        x.translate(X(dx), Y(1.25));
        x.rotate(a + sway);
        x.beginPath();
        x.moveTo(-X(0.035), 0);
        x.lineTo(X(0.035), 0);
        x.lineTo(X(0.045), X(len));
        x.lineTo(X(0), X(len - 0.03));
        x.lineTo(-X(0.045), X(len));
        x.closePath();
        x.fill();
        x.stroke();
        x.restore();
      }
    }
    roundRectPath(x, X(-0.075), Y(1.17), X(0.15), X(0.12), X(0.035));
    x.fillStyle = "#E8A93A";
    x.fill();
    if (tier === 2) {
      x.strokeStyle = "rgba(110,60,20,0.9)";
      x.lineWidth = lw;
      x.stroke();
    }

    // The V collar: darker inner layer edged with gold.
    x.beginPath();
    x.moveTo(X(-0.46), Y(0.96));
    x.lineTo(X(0), Y(1.19));
    x.lineTo(X(0.46), Y(0.96));
    x.closePath();
    x.fillStyle = "#5E1A14";
    x.fill();
    x.strokeStyle = GOLD;
    x.lineWidth = Math.max(0.7, (tier === 2 ? 0.065 : 0.08) * u);
    x.lineJoin = "round";
    x.beginPath();
    x.moveTo(X(-0.5), Y(0.94));
    x.lineTo(X(0), Y(1.19));
    x.lineTo(X(0.5), Y(0.94));
    x.stroke();

    // The neck scarf, hugging the underside of the head.
    x.beginPath();
    x.arc(0, hy, 1.13 * u, 0.17 * Math.PI, 0.83 * Math.PI);
    x.arc(0, hy, 0.88 * u, 0.83 * Math.PI, 0.17 * Math.PI, true);
    x.closePath();
    const sc = x.createLinearGradient(0, Y(0.85), 0, Y(1.13));
    sc.addColorStop(0, "#D04A3B");
    sc.addColorStop(1, "#8E2A22");
    x.fillStyle = sc;
    x.fill();

    // Arms: the punching one last so it reads over the other.
    const order: (-1 | 1)[] = pz.active && pz.side < 0 ? [1, -1] : [-1, 1];
    for (const sd of order) this.drawArm(x, u, hy, sd, t, tier, pz, baseM);
  }

  /** One stubby arm: a dark-red sleeve, a gold cuff and a cream fist with a dark tip. */
  private drawArm(
    x: CanvasRenderingContext2D, u: number, hy: number, sd: -1 | 1, t: number,
    tier: Tier, pz: PunchPose, baseM: DOMMatrix,
  ) {
    const m = clamp(this.morph, 0, 1);
    let ang = lerp(0.5, 2.3, m) + Math.sin(t * 1.1 + sd) * 0.025;
    let L = lerp(0.38, 0.34, m);
    let dx = sd * Math.sin(ang);
    let dy = Math.cos(ang);
    if (pz.active && pz.side === sd && pz.aim > 0) {
      const px = Math.cos(pz.ang);
      const py = Math.sin(pz.ang);
      dx = lerp(dx, px, pz.aim);
      dy = lerp(dy, py, pz.aim);
      const l = Math.hypot(dx, dy) || 1;
      dx /= l;
      dy /= l;
      L = 0.42 * pz.ext;
      ang = 0;
    }
    const sx = sd * 0.66 * u;
    const sy = hy + 0.78 * u;
    const fr = (tier === 2 ? 0.16 : 0.18) * u;
    const sw = (tier === 2 ? 0.25 : 0.29) * u;
    const fx = sx + dx * L * u;
    const fy = sy + dy * L * u;
    const wx = fx - dx * fr * 0.6;
    const wy = fy - dy * fr * 0.6;

    x.lineCap = "round";
    x.strokeStyle = "#3A120E";
    x.lineWidth = sw + Math.max(1, 0.05 * u);
    x.beginPath();
    x.moveTo(sx, sy);
    x.lineTo(wx, wy);
    x.stroke();
    x.strokeStyle = "#8E2A22";
    x.lineWidth = sw;
    x.stroke();
    if (tier === 2) {
      x.strokeStyle = "rgba(220,110,90,0.35)";
      x.lineWidth = sw * 0.28;
      x.beginPath();
      x.moveTo(sx - dy * sw * 0.18, sy + dx * sw * 0.18 - sw * 0.12);
      x.lineTo(wx - dy * sw * 0.18, wy + dx * sw * 0.18 - sw * 0.12);
      x.stroke();
    }
    // Cuff.
    x.lineCap = "butt";
    x.strokeStyle = GOLD;
    x.lineWidth = sw * 1.06;
    x.beginPath();
    x.moveTo(wx - dx * 0.07 * u, wy - dy * 0.07 * u);
    x.lineTo(wx + dx * 0.01 * u, wy + dy * 0.01 * u);
    x.stroke();

    // Fist.
    const g = x.createRadialGradient(fx - fr * 0.4, fy - fr * 0.45, fr * 0.1, fx, fy, fr * 1.1);
    g.addColorStop(0, "#FFFFFF");
    g.addColorStop(0.6, rgba(CREAM));
    g.addColorStop(1, rgba(BEIGE));
    x.fillStyle = g;
    x.beginPath();
    x.arc(fx, fy, fr, 0, TAU);
    x.fill();
    x.save();
    x.clip();
    x.fillStyle = INK_S;
    x.beginPath();
    x.arc(fx + dx * fr * 0.95, fy + dy * fr * 0.95, fr * 0.62, 0, TAU);
    x.fill();
    x.restore();
    x.lineWidth = Math.max(0.5, 0.03 * u);
    x.strokeStyle = "rgba(43,29,26,0.7)";
    x.beginPath();
    x.arc(fx, fy, fr, 0, TAU);
    x.stroke();

    // Remember the fist on the effects layer.
    const mtx = baseM.inverse().multiply(x.getTransform());
    const p = mtx.transformPoint(new DOMPoint(fx, fy));
    let vx = mtx.a * dx + mtx.c * dy;
    let vy = mtx.b * dx + mtx.d * dy;
    const vl = Math.hypot(vx, vy) || 1;
    vx /= vl;
    vy /= vl;
    this.fists[sd < 0 ? 0 : 1] = {
      x: this.fxOrigin.x + p.x, y: this.fxOrigin.y + p.y, dx: vx, dy: vy,
      r: fr * Math.hypot(mtx.a, mtx.b),
    };
  }

  /** Tiny Zukos: just a red collar under the head. */
  private drawCollar(x: CanvasRenderingContext2D, u: number, hy: number) {
    x.beginPath();
    x.arc(0, hy, 1.16 * u, 0.2 * Math.PI, 0.8 * Math.PI);
    x.arc(0, hy, 0.85 * u, 0.8 * Math.PI, 0.2 * Math.PI, true);
    x.closePath();
    x.fillStyle = "#B33A2E";
    x.fill();
  }

  // ── Body fire ───────────────────────────────────────────────────────────────

  /** Flames simmering round the head, drawn behind it. */
  private drawAura(x: CanvasRenderingContext2D, u: number, hy: number, t: number, pal: FirePalette, a: number, tier: Tier) {
    glow(x, 0, hy - 0.05 * u, 1.75 * u, pal.mid, 0.3 * Math.min(1, a));
    const N = tier === 2 ? 11 : 7;
    for (let i = 0; i < N; i++) {
      const s = i / (N - 1);
      const phi = -Math.PI * 1.1 + s * Math.PI * 1.2;
      const c = Math.cos(phi);
      const sn = Math.sin(phi);
      let vx = c * 0.6;
      let vy = sn * 0.6 - 0.8;
      const l = Math.hypot(vx, vy) || 1;
      vx /= l;
      vy /= l;
      const side = Math.abs(c);
      const h = u * (0.3 + 0.36 * side + 0.07 * Math.sin(i * 2.3)) * a;
      flame(x, c * 0.86 * u, hy + sn * 0.86 * u, Math.atan2(vx, -vy), u * 0.42, h, t, pal, i * 1.7, Math.min(1, a * 1.6));
    }
  }

  /** The hover ring: a ring of fire under the feet, back half then front half. */
  private drawRing(x: CanvasRenderingContext2D, u: number, hy: number, t: number, pal: FirePalette, r: number, front: boolean) {
    const cy = hy + 1.64 * u;
    const rx = 0.98 * u;
    const ry = 0.25 * u;
    if (!front) {
      x.save();
      x.translate(0, cy);
      x.scale(1, ry / rx);
      glow(x, 0, 0, rx * 1.4, pal.mid, 0.32 * r);
      x.restore();
    }
    const a0 = front ? 0 : Math.PI;
    x.lineCap = "round";
    x.strokeStyle = rgba(pal.base, 0.85 * r);
    x.lineWidth = Math.max(1, 0.11 * u);
    x.beginPath();
    x.ellipse(0, cy, rx, ry, 0, a0, a0 + Math.PI);
    x.stroke();
    x.strokeStyle = rgba(pal.tip, 0.9 * r);
    x.lineWidth = Math.max(0.6, 0.045 * u);
    x.stroke();
    const N = 9;
    for (let i = 0; i < N; i++) {
      const th = (i / N) * TAU + t * 1.1;
      const s = Math.sin(th);
      if (s >= 0 !== front) continue;
      const depth = 0.72 + 0.28 * s;
      const h = u * (0.3 + 0.1 * Math.sin(i * 1.7 + t * 3)) * depth * r;
      flame(x, Math.cos(th) * rx, cy + s * ry, -0.35 * Math.cos(th), u * 0.26 * depth, h, t, pal, i * 2.3, Math.min(1, r * 1.5));
    }
  }

  // ── Effects layer ───────────────────────────────────────────────────────────

  /**
   * Draws the fire that leaves the bot's box — fist flames, punch swirls,
   * fireballs, ember trails and impact bursts — in effects-layer coordinates.
   * Call after `draw`/`drawAt` (they record where the fists are).
   */
  drawFx(x: CanvasRenderingContext2D) {
    const ft = this.fxTime();
    const t = ft + this.flickerSeed;
    const a = this.anchor;
    if (!a) return;
    const pal = this.firePalette();

    for (const sd of [-1, 1] as const) {
      const f = this.fists[sd < 0 ? 0 : 1];
      if (!f) continue;
      const lvl = this.fistFireLevel(sd);
      if (lvl <= 0.02) continue;
      const pz = this.punchPose(ft);
      const fp = pz.active && pz.side === sd ? this.firePalette(pz.kind) : pal;
      let vx = f.dx * 0.5;
      let vy = f.dy * 0.5 - 0.85;
      const l = Math.hypot(vx, vy) || 1;
      vx /= l;
      vy /= l;
      glow(x, f.x, f.y, f.r * 3.4 * lvl, fp.mid, 0.45 * lvl);
      flame(x, f.x + f.dx * f.r * 0.2, f.y + f.dy * f.r * 0.2, Math.atan2(vx, -vy),
        f.r * 1.9 * (0.6 + 0.4 * lvl), f.r * 3.8 * lvl, t, fp, sd * 3, Math.min(1, lvl * 2));
    }

    for (const it of this.fx) {
      const e = ft - it.t0;
      if (e < 0 || e > it.dur) continue;
      const fp = FIRE[it.pal];
      if (it.kind === "swirl") {
        const k = e / it.dur;
        const R = a.R;
        swirl(x, a.x, a.y + R * 0.35 - k * R * 0.5, R * 1.3, k, t, fp, 0.38, 0, it.seed);
        swirl(x, a.x, a.y - R * 0.1 - k * R * 0.4, R * 1.0, clamp(k * 1.15 - 0.1, 0, 1), t, fp, 0.42, 0, it.seed + 2);
        continue;
      }
      const f = this.fists[it.side < 0 ? 0 : 1];
      const u = a.R * 0.74;
      const start: FxPoint = f
        ? { x: f.x + f.dx * f.r, y: f.y + f.dy * f.r }
        : { x: a.x + Math.cos(it.ang) * u, y: a.y + Math.sin(it.ang) * u };
      if (it.kind === "punch") this.drawPunchFx(x, it, e, t, fp, start, u, f);
      else this.drawFlickFx(x, it, e, t, fp, start, u);
    }
  }

  private drawPunchFx(
    x: CanvasRenderingContext2D, it: FxItem, e: number, t: number, pal: FirePalette,
    start: FxPoint, u: number, f: Fist | null,
  ) {
    // The swirl wrapping the fist as the punch lands.
    const sk = (e - 0.1) / 0.62;
    if (f && sk > 0 && sk < 1) punchTrail(x, f.x, f.y, f.dx, f.dy, u, sk, t, pal, it.seed);
    if (e < P_LAUNCH) return;
    if (!it.from) it.from = start;
    const from = it.from;
    const to = it.target ?? { x: from.x + Math.cos(it.ang) * it.dist, y: from.y + Math.sin(it.ang) * it.dist };
    const dist = Math.hypot(to.x - from.x, to.y - from.y);
    const arc = Math.min(20, dist * 0.09, u * 1.5);
    const s = clamp(u * 0.36, 3.2, 10);
    const at = (k: number): [number, number] => [
      lerp(from.x, to.x, k), lerp(from.y, to.y, k) - Math.sin(Math.PI * k) * arc,
    ];

    // Ember trail.
    const rnd = seeded(it.seed * 31 + 7);
    for (let i = 0; i < 18; i++) {
      const kk = rnd();
      const life = 0.25 + rnd() * 0.3;
      const vx = (rnd() - 0.5) * 34;
      const vy = -12 - rnd() * 26;
      const r = s * (0.22 + rnd() * 0.25);
      const age = (e - (P_LAUNCH + kk * it.flight)) / life;
      if (age <= 0 || age >= 1) continue;
      const [px, py] = at(kk);
      ember(x, px + vx * age * life, py + vy * age * life, r, age, pal);
    }

    const k = (e - P_LAUNCH) / it.flight;
    if (k < 1) {
      const [px, py] = at(k);
      const [qx, qy] = at(Math.min(1, k + 0.03));
      const grow = 0.65 + 0.35 * Math.min(1, k * 5);
      fireball(x, px, py, Math.atan2(qy - py, qx - px), s * grow, t, pal, 1, it.seed);
    }
    const bk = (e - P_LAUNCH - it.flight) / 0.5;
    if (bk > 0 && bk < 1) burst(x, to.x, to.y, bk, s * 1.3, t, pal, it.seed);
  }

  private drawFlickFx(
    x: CanvasRenderingContext2D, it: FxItem, e: number, t: number, pal: FirePalette,
    start: FxPoint, u: number,
  ) {
    const s = clamp(u * 0.22, 2.2, 6);
    const to = it.target ?? { x: start.x + Math.cos(it.ang) * u * 1.4, y: start.y + Math.sin(it.ang) * u * 1.4 };
    const k = (e - 0.14) / it.flight;
    if (k > 0 && k < 1) {
      if (!it.from) it.from = start;
      const from = it.from;
      const px = lerp(from.x, to.x, easeOut(k));
      const py = lerp(from.y, to.y, easeOut(k)) - Math.sin(Math.PI * k) * 6;
      const fade = it.target ? 1 : 1 - k;
      fireball(x, px, py, Math.atan2(to.y - from.y, to.x - from.x), s * (1 - 0.3 * k), t, pal, fade, it.seed);
    }
    if (it.target) {
      const bk = (e - 0.14 - it.flight) / 0.4;
      if (bk > 0 && bk < 1) burst(x, to.x, to.y, bk, s * 0.9, t, pal, it.seed);
    }
  }

  // ── Badge and particles ─────────────────────────────────────────────────────

  private drawBadge(x: CanvasRenderingContext2D, badge: Badge, R: number, cx: number, cy: number, t: number) {
    const bs = this.badgeS * (this.isMini ? 1.25 : 1);
    const bx = cx - R * 0.8 * this.sx;
    const by = cy - R * 0.8 * this.sy;

    x.save();
    x.translate(bx, by);
    x.scale(bs, bs);
    const col = rgba(badge.color);

    if (badge.kind === "dots") {
      if (this.isMini) {
        const phase = (t * 2.4) % 1;
        const dotR = R * 0.22 * (1 + 0.25 * Math.sin(phase * Math.PI * 2));
        x.fillStyle = "#1A100C";
        x.beginPath();
        x.arc(0, 0, R * 0.2, 0, Math.PI * 2);
        x.fill();
        x.fillStyle = col;
        x.beginPath();
        x.arc(0, 0, dotR, 0, Math.PI * 2);
        x.fill();
      } else {
        const pw = R * 0.72;
        const ph = R * 0.34;
        roundRectPath(x, -pw / 2 - R * 0.05, -ph / 2 - R * 0.05, pw + R * 0.1, ph + R * 0.1, ph / 2 + R * 0.05);
        x.fillStyle = "#1A100C";
        x.fill();
        for (let i = 0; i < 3; i++) {
          const phase = (((t * 2.4 - i * 0.22) % 1) + 1) % 1;
          const pulse = Math.max(0, Math.sin(phase * Math.PI * 2));
          const dotR = R * 0.06 * (1 + 0.45 * pulse);
          x.fillStyle = rgba(mix3(badge.color, WHITE, 0.3 * pulse));
          x.beginPath();
          x.arc((i - 1) * R * 0.2, 0, dotR, 0, Math.PI * 2);
          x.fill();
        }
      }
    } else if (badge.kind === "question" && !this.isMini) {
      // A hand-lettered "?" floating by the head, as on the concept sheet.
      x.rotate(-0.12 + Math.sin(t * 2.2) * 0.06);
      x.font = `900 ${R * 0.62}px ${FONT}`;
      x.textAlign = "center";
      x.textBaseline = "middle";
      x.lineJoin = "round";
      x.lineWidth = Math.max(1.5, R * 0.12);
      x.strokeStyle = "#1A100C";
      x.strokeText("?", 0, 0);
      x.fillStyle = col;
      x.fillText("?", 0, 0);
    } else if (badge.kind === "bang" || badge.kind === "question") {
      x.fillStyle = "#1A100C";
      x.beginPath();
      x.arc(0, 0, R * 0.3, 0, Math.PI * 2);
      x.fill();
      x.fillStyle = col;
      x.beginPath();
      x.arc(0, 0, R * 0.23, 0, Math.PI * 2);
      x.fill();
      if (!this.isMini) {
        x.fillStyle = "#1A100C";
        x.font = `900 ${R * 0.32}px ${FONT}`;
        x.textAlign = "center";
        x.textBaseline = "middle";
        x.fillText(badge.kind === "bang" ? "!" : "?", 0, R * 0.02);
      }
    } else if (this.isMini) {
      x.fillStyle = "#1A100C";
      x.beginPath();
      x.arc(0, 0, R * 0.2, 0, Math.PI * 2);
      x.fill();
      x.fillStyle = col;
      x.beginPath();
      x.arc(0, 0, R * 0.135, 0, Math.PI * 2);
      x.fill();
    } else {
      // A little flame in the state colour.
      const s = R * 0.24;
      x.translate(0, s * 0.35);
      x.rotate(Math.sin(t * 3.3) * 0.08);
      tearPath(x, s * 0.95, s * 1.45 * (1 + 0.06 * Math.sin(t * 9)), s * 0.12);
      x.lineWidth = Math.max(1.2, R * 0.08);
      x.lineJoin = "round";
      x.strokeStyle = "#1A100C";
      x.stroke();
      x.fillStyle = col;
      x.fill();
      tearPath(x, s * 0.45, s * 0.7, s * 0.05);
      x.fillStyle = rgba(mix3(badge.color, WHITE, 0.6));
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
          x.rotate(p.rot * 0.2 + p.age * 1.2);
          x.fillStyle = rgba(STAR);
          sparklePath(x, sz * 1.1);
          x.fill();
          break;
        case "spark":
          x.fillStyle = "rgba(242,138,30,0.35)";
          x.beginPath();
          x.arc(0, 0, sz * 0.9, 0, Math.PI * 2);
          x.fill();
          x.fillStyle = "#FFE29A";
          x.beginPath();
          x.arc(0, 0, sz * 0.4, 0, Math.PI * 2);
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
          x.fillStyle = "rgb(240,226,206)";
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
