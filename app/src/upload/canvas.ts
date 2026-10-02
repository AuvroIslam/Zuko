// The upload canvas — port of UploadCanvasView.swift.
//
// While the sequence engine is active this canvas draws the whole island body:
// card, dashed drop frame, drop text, progress bar, the choose card, Zuko and
// the dropped file. Zuko scans the file with fire: he points a burning fist at
// it, a sweep of flame runs down the page and every secret line it passes is
// burned into a placeholder block, then a flame-shield check pops on it. The
// island's own Zuko is hidden for the duration because this canvas poses its
// own BotEngine.

import { State } from "../core/state";
import { BotEngine, LED, shieldPath, type EyeShape, type RGB } from "../character/engine";
import { FIRE, ember, flameRibbon, glow, seeded, taper } from "../character/fire";
import { USC, lerp, progressAt, seg, type UploadEyeShape, type UploadFrame } from "./sequence";

const FONT = 'system-ui, "Segoe UI Variable Text", "Segoe UI", sans-serif';

/** Mirrors the reference `rr()`: a rounded rect, radius clamped to the box. */
function rr(ctx: CanvasRenderingContext2D, x: number, y: number, w: number, h: number, r: number) {
  const rad = Math.max(0, Math.min(r, w / 2, h / 2));
  ctx.beginPath();
  ctx.roundRect(x, y, w, h, rad);
}

const clamp01 = (v: number) => Math.max(0, Math.min(1, v));
const mix3 = (a: RGB, b: RGB, t: number): RGB => [lerp(a[0], b[0], t), lerp(a[1], b[1], t), lerp(a[2], b[2], t)];
const rgba = (c: RGB, a: number) =>
  `rgba(${Math.round(c[0] * 255)},${Math.round(c[1] * 255)},${Math.round(c[2] * 255)},${clamp01(a)})`;
const WHITE: RGB = [1, 1, 1];

/** A bump: 0 → 1 → 0 over [a, a + d]. */
const pulse = (t: number, a: number, d: number) => (t > a && t < a + d ? Math.sin((Math.PI * (t - a)) / d) : 0);

function blinkAt(t: number, tb: number): number {
  const k = seg(t, tb, tb + 0.12);
  return k > 0 && k < 1 ? 1 - Math.sin(Math.PI * k) * 0.94 : 1;
}

const EYE: Record<UploadEyeShape, EyeShape> = { pill: "pill", cup: "cup", content: "happy", wide: "pill" };

/** Document size at scale 1, in island points. */
const DOC_W = 34;
const DOC_H = 42;

/** The sheet's text lines: [y, width] as fractions of the sheet; secrets get burned. */
const DOC_LINES: readonly [number, number, boolean][] = [
  [0.3, 0.42, false], [0.43, 0.64, true], [0.54, 0.64, false], [0.65, 0.5, true], [0.76, 0.38, false],
];

const FIRE_PAL = FIRE.ember;

function text(
  ctx: CanvasRenderingContext2D,
  s: string,
  x: number,
  y: number,
  font: string,
  color: string,
  align: CanvasTextAlign = "left",
) {
  ctx.font = font;
  ctx.fillStyle = color;
  ctx.textAlign = align;
  // SwiftUI's .leading / .center / .trailing anchors are vertically centred.
  ctx.textBaseline = "middle";
  ctx.fillText(s, x, y);
}

export interface UploadCanvasActions {
  /** Primary button — hand the file to the chat. */
  ask(): void;
  /** Secondary button. */
  cancel(): void;
}

export class UploadCanvas {
  /** Wrapper holding the canvas and the two invisible choose buttons. */
  readonly el: HTMLElement;

  private canvas: HTMLCanvasElement;
  private ctx: CanvasRenderingContext2D | null;
  private overlay: HTMLElement;
  private sizedFor = 0;
  private bot = new BotEngine();

  constructor(actions: UploadCanvasActions) {
    this.canvas = document.createElement("canvas");
    this.canvas.id = "upload-canvas";

    // Invisible hit areas at the reference button positions. The labels are
    // painted on the canvas; these only catch the click.
    const mk = (x: number, w: number, onclick: () => void) => {
      const b = document.createElement("button");
      b.className = "upload-hit";
      b.style.left = `${x}px`;
      b.style.top = "113px";
      b.style.width = `${w}px`;
      b.style.height = "26px";
      b.addEventListener("click", onclick);
      return b;
    };
    this.overlay = document.createElement("div");
    this.overlay.id = "upload-overlay";
    this.overlay.append(mk(114, 168, actions.ask), mk(290, 120, actions.cancel));

    this.el = document.createElement("div");
    this.el.id = "upload-layer";
    this.el.append(this.canvas, this.overlay);

    this.ctx = this.canvas.getContext("2d");
    this.bot.flickerSeed = 7.3;
    this.bot.snapToState();
  }

  /** `wallTime` in seconds drives the marching dashes and the flame flicker. */
  draw(f: UploadFrame, wallTime: number) {
    const dpr = Math.min(2, window.devicePixelRatio || 1);
    if (this.sizedFor !== dpr) {
      this.sizedFor = dpr;
      this.canvas.width = Math.round(USC.W * dpr);
      this.canvas.height = Math.round(USC.ISL_H * dpr);
      this.canvas.style.width = `${USC.W}px`;
      this.canvas.style.height = `${USC.ISL_H}px`;
    }
    const ctx = this.ctx;
    if (!ctx) return;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, USC.W, USC.ISL_H);

    this.drawScene(ctx, f, wallTime);

    // The buttons only exist once the choose card has faded in.
    this.overlay.style.display = f.chooseAlpha > 0.5 ? "block" : "none";
  }

  // ── Scene ─────────────────────────────────────────────────────────────────

  private drawScene(ctx: CanvasRenderingContext2D, f: UploadFrame, wallTime: number) {
    // Island background.
    ctx.fillStyle = "#000000";
    ctx.fillRect(0, 0, USC.W, USC.ISL_H);

    // Card.
    ctx.save();
    rr(ctx, USC.CARD_X, USC.CARD_Y, USC.CARD_W, USC.CARD_H, USC.CARD_R);
    ctx.clip();
    ctx.fillStyle = "#0D0E10";
    ctx.fillRect(USC.CARD_X, USC.CARD_Y, USC.CARD_W, USC.CARD_H);

    // Glow fanning up from the bottom edge of the card: warm firelight while
    // Zuko works on the file, green once it is uploading safely.
    if (f.greenWash > 0) {
      const gx = USC.CARD_X + USC.CARD_W / 2;
      const gy = USC.CARD_Y + USC.CARD_H;
      const c = f.pt < USC.T_PROG_START ? "242,138,30" : "40,212,130";
      const g = ctx.createRadialGradient(gx, gy, 0, gx, gy, USC.CARD_H * 1.5);
      g.addColorStop(0, `rgba(${c},${f.greenWash * 0.9})`);
      g.addColorStop(0.55, `rgba(${c},${f.greenWash * 0.3})`);
      g.addColorStop(1, `rgba(${c},0)`);
      ctx.fillStyle = g;
      ctx.fillRect(USC.CARD_X, USC.CARD_Y, USC.CARD_W, USC.CARD_H);
    }
    ctx.restore();

    // Dashed border, marching left to right at ~20 pt/s.
    if (f.zoneAlpha > 0) {
      ctx.save();
      ctx.globalAlpha = f.zoneAlpha;
      ctx.strokeStyle = f.zoneOver ? "rgba(242,138,30,0.55)" : "rgba(255,255,255,0.14)";
      ctx.lineWidth = 1.5;
      ctx.setLineDash([6, 5]);
      ctx.lineDashOffset = -wallTime * 20;
      rr(ctx, USC.CARD_X + 0.75, USC.CARD_Y + 0.75, USC.CARD_W - 1.5, USC.CARD_H - 1.5, USC.CARD_R - 0.5);
      ctx.stroke();
      ctx.restore();
    }

    if (f.zoneAlpha > 0 && f.textAlpha > 0) this.drawDropText(ctx, f);
    if (f.barAlpha > 0 || f.barReveal > 0) this.drawProgressBar(ctx, f);
    if (f.chooseAlpha > 0) this.drawChoose(ctx, f);

    const col = this.ledColor(f);
    this.drawZuko(ctx, f, wallTime, col);
    if (f.fileVisible && f.docAlpha > 0.01) this.drawFile(ctx, f, wallTime, col);
  }

  // ── Drop zone text and chips ──────────────────────────────────────────────

  private drawDropText(ctx: CanvasRenderingContext2D, f: UploadFrame) {
    ctx.save();
    ctx.globalAlpha = f.textAlpha;
    text(ctx, "Drop your files here", USC.TEXT_X, USC.TEXT_Y - 4, `500 13px ${FONT}`, "#D5D7DB");

    let cx = USC.TEXT_X;
    for (const chip of ["PDF", "Images", "Code", "Docs"]) {
      // The macOS port measures chips the same rough way, so the row lines up.
      const w = chip.length * 6.5 + 16;
      ctx.fillStyle = "rgba(255,255,255,0.07)";
      rr(ctx, cx, USC.TEXT_Y + 9, w, 18, 9);
      ctx.fill();
      text(ctx, chip, cx + 8, USC.TEXT_Y + 18, `500 11px ${FONT}`, "#B9BDC4");
      cx += w + 6;
    }
    ctx.restore();
  }

  // ── Progress bar ──────────────────────────────────────────────────────────

  private drawProgressBar(ctx: CanvasRenderingContext2D, f: UploadFrame) {
    ctx.save();
    ctx.globalAlpha = Math.max(f.barAlpha, 0.001);

    const x0 = USC.BAR_X0;
    const x1 = USC.BAR_X1;
    const by = USC.BAR_Y;
    const barLen = (x1 - x0) * f.barReveal;

    const name = State.droppedFile?.name ?? "file";
    text(ctx, `Scanning ${name}`, x0, by - 30, `500 12.5px ${FONT}`, "#A9ADB5");

    if (f.check > 0) {
      ctx.save();
      ctx.translate(x1 - 8, by - 30);
      ctx.scale(f.check, f.check);
      ctx.beginPath();
      ctx.arc(0, 0, 8, 0, Math.PI * 2);
      ctx.fillStyle = "#34D399";
      ctx.fill();
      ctx.beginPath();
      ctx.moveTo(-3.6, 0.2);
      ctx.lineTo(-1, 2.8);
      ctx.lineTo(3.8, -2.6);
      ctx.strokeStyle = "#07130E";
      ctx.lineWidth = 2;
      ctx.lineCap = "round";
      ctx.lineJoin = "round";
      ctx.stroke();
      ctx.restore();
    } else {
      text(ctx, `${Math.round(f.progress * 100)} %`, x1, by - 30, `500 12.5px ${FONT}`, "#A9ADB5", "right");
    }

    // Track.
    if (barLen > 0) {
      ctx.fillStyle = "rgba(255,255,255,0.08)";
      rr(ctx, x0, by - 3, barLen, 6, 3);
      ctx.fill();
    }

    // Fill.
    const fx = lerp(x0, x1, f.progress);
    if (fx > x0 + 1) {
      const flashGreen = `rgb(${Math.round(lerp(52, 110, f.flash))},${Math.round(
        lerp(211, 231, f.flash),
      )},${Math.round(lerp(153, 183, f.flash))})`;
      const g = ctx.createLinearGradient(x0, 0, fx, 0);
      g.addColorStop(0, "#1FA87A");
      g.addColorStop(1, flashGreen);
      ctx.fillStyle = g;
      rr(ctx, x0, by - 3, fx - x0, 6, 3);
      ctx.fill();
    }

    // Glow trail, its length driven by how fast the bar is moving.
    if (f.progress > 0.01 && f.progress < 1) {
      const v =
        (progressAt(f.t + 0.01, USC.T_PROG_START, f.progEnd) -
          progressAt(f.t, USC.T_PROG_START, f.progEnd)) / 0.01;
      const tl = Math.max(8, Math.min(34, 8 + v * 40));
      const g = ctx.createLinearGradient(fx - tl, 0, fx, 0);
      g.addColorStop(0, "rgba(52,212,153,0)");
      g.addColorStop(1, "rgba(110,231,183,0.6)");
      ctx.save();
      ctx.filter = "blur(3px)";
      ctx.fillStyle = g;
      rr(ctx, fx - tl, by - 4, tl, 8, 4);
      ctx.fill();
      ctx.restore();
    }
    ctx.restore();
  }

  // ── Choose card ───────────────────────────────────────────────────────────

  private drawChoose(ctx: CanvasRenderingContext2D, f: UploadFrame) {
    ctx.save();
    ctx.globalAlpha = f.chooseAlpha;
    ctx.translate(0, (1 - f.chooseAlpha) * 4);

    const name = State.droppedFile?.name ?? "file";
    text(ctx, `${name} is ready.`, 114, 80, `600 14px ${FONT}`, "#F5F6F8");
    text(ctx, "What do you want to do with it?", 114, 100, `400 12.5px ${FONT}`, "#9398A1");

    ctx.fillStyle = "#F5F6F8";
    rr(ctx, 114, 113, 168, 26, 13);
    ctx.fill();
    text(ctx, "Ask a question about it", 198, 126, `500 12.5px ${FONT}`, "#0B0C0E", "center");

    ctx.fillStyle = "rgba(255,255,255,0.09)";
    rr(ctx, 290, 113, 120, 26, 13);
    ctx.fill();
    text(ctx, "Cancel", 350, 126, `500 12.5px ${FONT}`, "#F1F2F4", "center");
    ctx.restore();
  }

  // ── Zuko ──────────────────────────────────────────────────────────────────

  /** Amber while scanning; golden once the file has its badge and when the bar completes. */
  private ledColor(f: UploadFrame): RGB {
    const ok = Math.max(clamp01(f.badge) * (1 - seg(f.pt, USC.T_CHEW_END, USC.T_SHRINK_END)), f.flash);
    return mix3(LED.ember, LED.gold, ok);
  }

  /** Zuko's half box: the sequence's `d` is the old body box. */
  private zukoR(f: UploadFrame): number {
    return (f.d / 2) * 0.92;
  }

  /** The fire sweep's height on the page: a preview bob while hovering, one pass after the drop. */
  private sweepY(f: UploadFrame, wallTime: number): number {
    const s = f.docScale;
    const h = DOC_H * s;
    const top = f.docY - h / 2;
    const bot = f.docY + h / 2;
    return f.dropped ? lerp(top + 3 * s, bot - 3 * s, f.scan) : f.docY + Math.sin(wallTime * 4.5) * h * 0.36;
  }

  /** How hard the sweep burns: only after the drop, and only while the page is there. */
  private burn(f: UploadFrame): number {
    return f.dropped && f.fileVisible ? f.beam * f.docAlpha : 0;
  }

  private drawZuko(ctx: CanvasRenderingContext2D, f: UploadFrame, wallTime: number, col: RGB) {
    const b = this.bot;
    const pt = f.pt;
    const R = this.zukoR(f);
    b.clock = wallTime;
    b.col = b.colT = col;
    b.tint = 0.55;
    b.glow = 1;
    b.boot = 1;
    b.ignite = 1;
    b.flare = f.dropped ? Math.max(pulse(pt, USC.T_DROP, 0.6) * 0.7, pulse(pt, USC.T_CHEW1, 0.7)) : 0;
    b.boost = f.dropped ? pulse(pt, USC.T_CHEW1, 0.6) * 0.6 : 0;
    b.morph = Math.min(1, f.morph);
    // While the file hovers his fists burn in the ready stance; after the drop
    // one fist points at the page and throws the sweep.
    b.slotH = f.dropped ? 0 : 0.1 + f.beam * 0.35;
    b.slotHTarget = 0;
    b.isScanning = false;
    const burn = this.burn(f);
    const edge = f.docX - (DOC_W * f.docScale) / 2;
    b.punch = 0.6 * burn;
    b.punchAngle = Math.atan2(this.sweepY(f, wallTime) - (f.y + f.hop + R * 0.45), edge - f.x);
    b.fistFire = burn;
    b.sx = f.sx;
    b.sy = f.sy;
    b.tilt = f.tilt;
    b.yaw = f.lookX * 0.62;
    b.pitch = f.lookY * 0.5;
    b.eyeOverride = EYE[f.eye];
    b.open = Math.min(blinkAt(pt, USC.T_CHEW_END + 0.2), blinkAt(pt, f.growEnd + 0.45));
    b.badge = null;
    b.drawAt(ctx, f.x, f.y + f.hop, R);
    b.drawFx(ctx);
  }

  // ── The file, the fire sweep and the badge ────────────────────────────────

  private drawFile(ctx: CanvasRenderingContext2D, f: UploadFrame, wallTime: number, col: RGB) {
    const s = f.docScale;
    const w = DOC_W * s;
    const h = DOC_H * s;
    const top = f.docY - h / 2;
    const bot = f.docY + h / 2;
    const lineY = this.sweepY(f, wallTime);
    const burn = this.burn(f);
    const preview = f.dropped ? 0 : f.beam * f.docAlpha;

    // The jet: flame from the pointing fist to the page edge at the sweep.
    const side = f.docX >= f.x ? 1 : -1;
    const fist = this.bot.fistPosition(side);
    const ex = f.docX - (side * w) / 2;
    if (fist && burn > 0.02 && side * (ex - fist.x) > 2) {
      const n = 8;
      const sx = fist.x + fist.dx * fist.r;
      const sy = fist.y + fist.dy * fist.r;
      const pts: number[] = [];
      const hw: number[] = [];
      for (let i = 0; i < n; i++) {
        const k = i / (n - 1);
        pts.push(lerp(sx, ex, k), lerp(sy, lineY, k) - Math.sin(Math.PI * k) * 3);
        hw.push((1 + 2.4 * k) * s * (0.8 + 0.2 * burn));
      }
      flameRibbon(ctx, pts, hw, wallTime, FIRE_PAL, burn, 3);
      glow(ctx, ex, lineY, 12 * s, FIRE_PAL.mid, 0.5 * burn);
    }

    ctx.save();
    ctx.globalAlpha = f.docAlpha;
    const burnY = f.dropped && f.scan > 0 ? lineY : null;
    drawDoc(ctx, f.docX, f.docY, s, burnY);

    // Freshly burned lines still smoulder for a moment.
    if (burnY != null) {
      const x0 = f.docX - w / 2;
      for (const [fy, fw, secret] of DOC_LINES) {
        if (!secret) continue;
        const ly = top + h * fy;
        const age = ((burnY - ly) / (h - 6 * s)) * 0.55;
        if (age <= 0 || age >= 0.45) continue;
        const a = 1 - age / 0.45;
        for (let i = 0; i < 3; i++) {
          const fx = x0 + w * (0.2 + (fw * (i + 0.5)) / 3);
          ember(ctx, fx, ly - age * 22 * s - i * 1.5, 1.6 * s, age / 0.45, FIRE_PAL);
        }
        glow(ctx, x0 + w * (0.18 + fw / 2), ly, w * 0.45, FIRE_PAL.mid, 0.35 * a);
      }
    }

    // The sweep itself: a band of fire across the page, licking upwards.
    if (burn > 0.02 && f.scan < 1) {
      ctx.save();
      docPath(ctx, f.docX, f.docY, s);
      ctx.clip();
      ctx.fillStyle = `rgba(242,138,30,${0.12 * burn})`;
      ctx.fillRect(f.docX - w / 2, top, w, lineY - top);
      ctx.restore();
      const n = 9;
      const pts: number[] = [];
      for (let i = 0; i < n; i++) pts.push(lerp(f.docX + w / 2 + 2, f.docX - w / 2 - 2, i / (n - 1)), lineY);
      glow(ctx, f.docX, lineY, w * 0.7, FIRE_PAL.mid, 0.35 * burn);
      flameRibbon(ctx, pts, taper(n, 2.6 * s, 1), wallTime, FIRE_PAL, burn, 8);
      const rnd = seeded(5);
      for (let i = 0; i < 7; i++) {
        const ph = (((wallTime * 1.8 + rnd()) % 1) + 1) % 1;
        const px = f.docX - w / 2 + w * rnd();
        ember(ctx, px, lineY - ph * 14 * s, 1.3 * s, ph, FIRE_PAL);
      }
    } else if (preview > 0.01) {
      // While hovering close: a faint flicker of flame bobbing over the page.
      ctx.save();
      ctx.shadowColor = rgba(col, preview);
      ctx.shadowBlur = 6;
      ctx.fillStyle = rgba(mix3(col, WHITE, 0.35), preview);
      rr(ctx, f.docX - w / 2 - 3, lineY - 0.9, w + 6, 1.8, 0.9);
      ctx.fill();
      ctx.restore();
    }

    if (f.badge > 0.01) drawFlameCheck(ctx, f.docX + w / 2 - 3 * s, bot - 6 * s, 8.5 * s * f.badge, wallTime);
    ctx.restore();
  }
}

// ── Flame-shield check badge ────────────────────────────────────────────────

/** A small flame-orange shield with a check and a flame on top: the file passed Zuko's fire. */
function drawFlameCheck(ctx: CanvasRenderingContext2D, cx: number, cy: number, r: number, t: number) {
  if (r <= 0.3) return;
  ctx.save();
  ctx.translate(cx, cy);
  // The flame peeking over the shield.
  const fh = r * (1.15 + 0.12 * Math.sin(t * 9));
  ctx.fillStyle = "#F05A14";
  ctx.beginPath();
  ctx.moveTo(-r * 0.42, -r * 0.6);
  ctx.quadraticCurveTo(-r * 0.5, -r * 0.6 - fh * 0.6, Math.sin(t * 5) * r * 0.08, -r * 0.6 - fh);
  ctx.quadraticCurveTo(r * 0.5, -r * 0.6 - fh * 0.6, r * 0.42, -r * 0.6);
  ctx.closePath();
  ctx.fill();
  ctx.fillStyle = "#FFC24D";
  ctx.beginPath();
  ctx.moveTo(-r * 0.22, -r * 0.6);
  ctx.quadraticCurveTo(-r * 0.25, -r * 0.6 - fh * 0.4, 0, -r * 0.6 - fh * 0.62);
  ctx.quadraticCurveTo(r * 0.25, -r * 0.6 - fh * 0.4, r * 0.22, -r * 0.6);
  ctx.closePath();
  ctx.fill();

  const p = shieldPath(r);
  ctx.lineWidth = Math.max(1, r * 0.3);
  ctx.lineJoin = "round";
  ctx.strokeStyle = "#2B1D1A";
  ctx.stroke(p);
  const g = ctx.createLinearGradient(0, -r, 0, r);
  g.addColorStop(0, "#FFB347");
  g.addColorStop(1, "#E0561A");
  ctx.fillStyle = g;
  ctx.fill(p);
  ctx.beginPath();
  ctx.moveTo(-r * 0.42, -r * 0.04);
  ctx.lineTo(-r * 0.1, r * 0.28);
  ctx.lineTo(r * 0.46, -r * 0.36);
  ctx.strokeStyle = "#FFF6E6";
  ctx.lineWidth = Math.max(1, r * 0.24);
  ctx.lineCap = "round";
  ctx.lineJoin = "round";
  ctx.stroke();
  ctx.restore();
}

// ── Document icon ───────────────────────────────────────────────────────────

/** The sheet outline with its folded corner, centred on (cx, cy). */
function docPath(ctx: CanvasRenderingContext2D, cx: number, cy: number, sc: number) {
  const w = DOC_W * sc;
  const h = DOC_H * sc;
  const x = cx - w / 2;
  const y = cy - h / 2;
  const fold = 8 * sc;
  const r = 2 * sc;
  ctx.beginPath();
  ctx.moveTo(x + r, y);
  ctx.lineTo(x + w - fold, y);
  ctx.lineTo(x + w, y + fold);
  ctx.lineTo(x + w, y + h - r);
  ctx.quadraticCurveTo(x + w, y + h, x + w - r, y + h);
  ctx.lineTo(x + r, y + h);
  ctx.quadraticCurveTo(x, y + h, x, y + h - r);
  ctx.lineTo(x, y + r);
  ctx.quadraticCurveTo(x, y, x + r, y);
  ctx.closePath();
}

/**
 * The generic sheet with a folded corner and a few lines of text. macOS swaps
 * in the real file icon; Windows has no equivalent reachable from the webview,
 * so this is the shape in every case. Secret lines above `burnY` have been
 * burned into placeholder blocks.
 */
function drawDoc(ctx: CanvasRenderingContext2D, cx: number, cy: number, sc: number, burnY: number | null = null) {
  const w = DOC_W * sc;
  const h = DOC_H * sc;
  const x = cx - w / 2;
  const y = cy - h / 2;
  const fold = 8 * sc;

  ctx.save();
  ctx.shadowColor = "rgba(0,0,0,0.45)";
  ctx.shadowBlur = 8;
  ctx.shadowOffsetY = 3;
  docPath(ctx, cx, cy, sc);
  ctx.fillStyle = "#F4F4F6";
  ctx.fill();
  ctx.restore();

  ctx.beginPath();
  ctx.moveTo(x + w - fold, y);
  ctx.lineTo(x + w - fold, y + fold);
  ctx.lineTo(x + w, y + fold);
  ctx.closePath();
  ctx.fillStyle = "#D5D6DB";
  ctx.fill();

  for (const [fy, fw, secret] of DOC_LINES) {
    const ly = y + h * fy;
    const lh = Math.max(1, h * 0.05);
    if (secret && burnY != null && ly + lh / 2 < burnY) {
      const bh = Math.max(2.6, h * 0.09);
      rr(ctx, x + w * 0.15, ly + lh / 2 - bh / 2, w * (fw + 0.06), bh, bh / 2);
      ctx.fillStyle = "#F28A1E";
      ctx.fill();
      ctx.lineWidth = Math.max(0.6, 0.7 * sc);
      ctx.strokeStyle = "#B33A2E";
      ctx.stroke();
      continue;
    }
    ctx.fillStyle = "#B4B9C3";
    rr(ctx, x + w * 0.18, ly, w * fw, lh, 1);
    ctx.fill();
  }
}
