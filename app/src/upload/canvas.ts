// The upload canvas — port of UploadCanvasView.swift.
//
// While the sequence engine is active this canvas draws the whole island body:
// card, dashed drop frame, drop text, progress bar, the choose card, Zuko and
// the dropped file. Zuko scans the file: a beam from its visor sweeps the page
// top to bottom, then a shield-check badge pops on it. The island's own Zuko is
// hidden for the duration because this canvas poses its own BotEngine.

import { State } from "../core/state";
import { BotEngine, LED, shieldPath, type EyeShape, type RGB } from "../character/engine";
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

const EYE: Record<UploadEyeShape, EyeShape> = { pill: "pill", cup: "cup", content: "happy", wide: "wide" };

/** Document size at scale 1, in island points. */
const DOC_W = 34;
const DOC_H = 42;

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

    // Green glow, fanning up from the bottom edge of the card.
    if (f.greenWash > 0) {
      const gx = USC.CARD_X + USC.CARD_W / 2;
      const gy = USC.CARD_Y + USC.CARD_H;
      const g = ctx.createRadialGradient(gx, gy, 0, gx, gy, USC.CARD_H * 1.5);
      g.addColorStop(0, `rgba(40,212,130,${f.greenWash * 0.9})`);
      g.addColorStop(0.55, `rgba(40,212,130,${f.greenWash * 0.3})`);
      g.addColorStop(1, "rgba(40,212,130,0)");
      ctx.fillStyle = g;
      ctx.fillRect(USC.CARD_X, USC.CARD_Y, USC.CARD_W, USC.CARD_H);
    }
    ctx.restore();

    // Dashed border, marching left to right at ~20 pt/s.
    if (f.zoneAlpha > 0) {
      ctx.save();
      ctx.globalAlpha = f.zoneAlpha;
      ctx.strokeStyle = f.zoneOver ? "rgba(46,230,197,0.5)" : "rgba(255,255,255,0.14)";
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

  /** Teal while scanning; green once the file has its badge and when the bar completes. */
  private ledColor(f: UploadFrame): RGB {
    const ok = Math.max(clamp01(f.badge) * (1 - seg(f.pt, USC.T_CHEW_END, USC.T_SHRINK_END)), f.flash);
    return mix3(LED.teal, LED.green, ok);
  }

  /** Zuko's half width: the sequence's `d` is the old body box. */
  private zukoR(f: UploadFrame): number {
    return (f.d / 2) * 0.92;
  }

  private drawZuko(ctx: CanvasRenderingContext2D, f: UploadFrame, wallTime: number, col: RGB) {
    const b = this.bot;
    const pt = f.pt;
    b.clock = wallTime;
    b.col = b.colT = col;
    b.tint = 0.55;
    b.glow = 1;
    b.boot = 1;
    b.ignite = 1;
    b.flameLevel = 0.9;
    b.flare = f.dropped ? Math.max(pulse(pt, USC.T_DROP, 0.6) * 0.7, pulse(pt, USC.T_CHEW1, 0.7)) : 0;
    b.boost = f.dropped ? pulse(pt, USC.T_CHEW1, 0.6) * 0.6 : 0;
    b.morph = Math.min(1, f.morph);
    // The visor's own sweep runs while the file hovers; after the drop the
    // beam leaves the visor instead.
    b.slotH = f.dropped ? 0 : 0.1 + f.beam * 0.35;
    b.slotHTarget = 0;
    b.isScanning = false;
    b.sx = f.sx;
    b.sy = f.sy;
    b.tilt = f.tilt;
    b.yaw = f.lookX * 0.62;
    b.pitch = f.lookY * 0.5;
    b.eyeOverride = EYE[f.eye];
    b.open = Math.min(blinkAt(pt, USC.T_CHEW_END + 0.2), blinkAt(pt, f.growEnd + 0.45));
    b.badge = null;
    b.drawAt(ctx, f.x, f.y + f.hop, this.zukoR(f));
  }

  // ── The file, the scan beam and the badge ─────────────────────────────────

  private drawFile(ctx: CanvasRenderingContext2D, f: UploadFrame, wallTime: number, col: RGB) {
    const s = f.docScale;
    const w = DOC_W * s;
    const h = DOC_H * s;
    const top = f.docY - h / 2;
    const bot = f.docY + h / 2;
    const R = this.zukoR(f);

    // Scan line: a slow preview bob while hovering, one top → bottom sweep after the drop.
    const lineY = f.dropped
      ? lerp(top + 3 * s, bot - 3 * s, f.scan)
      : f.docY + Math.sin(wallTime * 4.5) * h * 0.36;

    // The beam leaves the visor edge facing the file, once the two are apart.
    const side = f.docX >= f.x ? 1 : -1;
    const ax = f.x + side * 0.66 * R * f.sx;
    const ay = f.y + f.hop - 0.15 * R * f.sy;
    const edge = f.docX - (side * w) / 2;
    const beam = f.beam * f.docAlpha * clamp01((side * (edge - ax) - 4) / 10);

    if (beam > 0.01) {
      ctx.save();
      const g = ctx.createLinearGradient(ax, 0, edge, 0);
      g.addColorStop(0, rgba(col, 0.32 * beam));
      g.addColorStop(1, rgba(col, 0.07 * beam));
      ctx.fillStyle = g;
      ctx.beginPath();
      ctx.moveTo(ax, ay - 1.5);
      ctx.lineTo(edge, top);
      ctx.lineTo(edge, bot);
      ctx.lineTo(ax, ay + 1.5);
      ctx.closePath();
      ctx.fill();
      ctx.strokeStyle = rgba(mix3(col, WHITE, 0.4), 0.7 * beam);
      ctx.lineWidth = 1;
      ctx.beginPath();
      ctx.moveTo(ax, ay);
      ctx.lineTo(edge, lineY);
      ctx.stroke();
      ctx.restore();
    }

    ctx.save();
    ctx.globalAlpha = f.docAlpha;
    drawDoc(ctx, f.docX, f.docY, s);

    if (beam > 0.01) {
      // The part of the page already scanned takes a faint tint.
      if (f.dropped && f.scan > 0) {
        ctx.save();
        docPath(ctx, f.docX, f.docY, s);
        ctx.clip();
        ctx.fillStyle = rgba(col, 0.18 * beam);
        ctx.fillRect(f.docX - w / 2, top, w, lineY - top);
        ctx.restore();
      }
      ctx.save();
      ctx.shadowColor = rgba(col, beam);
      ctx.shadowBlur = 6;
      ctx.fillStyle = rgba(mix3(col, WHITE, 0.35), beam);
      rr(ctx, f.docX - w / 2 - 3, lineY - 0.9, w + 6, 1.8, 0.9);
      ctx.fill();
      ctx.restore();
    }

    if (f.badge > 0.01) drawShieldCheck(ctx, f.docX + w / 2 - 3 * s, bot - 6 * s, 8.5 * s * f.badge);
    ctx.restore();
  }
}

// ── Shield-check badge ──────────────────────────────────────────────────────

/** A small shield with a check mark: the file passed Zuko's scan. */
function drawShieldCheck(ctx: CanvasRenderingContext2D, cx: number, cy: number, r: number) {
  if (r <= 0.3) return;
  ctx.save();
  ctx.translate(cx, cy);
  const p = shieldPath(r);
  ctx.lineWidth = Math.max(1, r * 0.34);
  ctx.lineJoin = "round";
  ctx.strokeStyle = "#0B0F17";
  ctx.stroke(p);
  const g = ctx.createLinearGradient(0, -r, 0, r);
  g.addColorStop(0, "#6EF0A0");
  g.addColorStop(1, "#22B35E");
  ctx.fillStyle = g;
  ctx.fill(p);
  ctx.beginPath();
  ctx.moveTo(-r * 0.42, -r * 0.04);
  ctx.lineTo(-r * 0.1, r * 0.28);
  ctx.lineTo(r * 0.46, -r * 0.36);
  ctx.strokeStyle = "#062012";
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
 * so this is the shape in every case.
 */
function drawDoc(ctx: CanvasRenderingContext2D, cx: number, cy: number, sc: number) {
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

  ctx.fillStyle = "#B4B9C3";
  const lines: [number, number][] = [[0.3, 0.42], [0.43, 0.64], [0.54, 0.64], [0.65, 0.64], [0.76, 0.38]];
  for (const [fy, fw] of lines) {
    rr(ctx, x + w * 0.18, y + h * fy, w * fw, Math.max(1, h * 0.05), 1);
    ctx.fill();
  }
}
