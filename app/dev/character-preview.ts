// Dev harness: renders Zuko deterministically — every state, every LED glyph,
// mini bots, the launch greeting and the drop/scan sequence at fixed
// timestamps — so a single headless screenshot shows the whole character.
// Not part of the app bundle.
//
//   /dev/character-preview.html?only=states,eyes,minis,sizes,greet,drop,icons

import { BOT_STATES, BotEngine, hexToRGB, type EyeShape } from "../src/character/engine";
import { Greeting } from "../src/character/greeting";
import { UploadCanvas } from "../src/upload/canvas";
import { UploadSeq } from "../src/upload/sequence";
import { State } from "../src/core/state";
import type { BotStateName } from "../src/core/layout";

const root = document.getElementById("root")!;
const params = new URLSearchParams(location.search);
const only = params.get("only")?.split(",") ?? null;
const want = (s: string) => !only || only.includes(s);
const dpr = Math.max(1, window.devicePixelRatio || 1);

function section(title: string, cls = ""): HTMLElement {
  const h = document.createElement("h2");
  h.textContent = title;
  const row = document.createElement("div");
  row.className = `row ${cls}`;
  root.append(h, row);
  return row;
}

function cell(parent: HTMLElement, w: number, h: number, label: string, light = false) {
  const fig = document.createElement("figure");
  const c = document.createElement("canvas");
  c.width = Math.round(w * dpr);
  c.height = Math.round(h * dpr);
  c.style.width = `${w}px`;
  c.style.height = `${h}px`;
  if (light) c.className = "light";
  const cap = document.createElement("figcaption");
  cap.textContent = label;
  fig.append(c, cap);
  parent.append(fig);
  const ctx = c.getContext("2d")!;
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  return ctx;
}

/** A posed engine: resting pose of `state`, flame frozen at `clock`. */
function posed(state: BotStateName, clock = 1.37): BotEngine {
  const e = new BotEngine();
  e.setState(state, true);
  e.snapToState();
  e.clock = clock;
  return e;
}

const OVERHANG = 40;
function drawBot(parent: HTMLElement, e: BotEngine, size: number, label: string) {
  const ctx = cell(parent, size, size + OVERHANG, label);
  e.particleOverhang = OVERHANG;
  e.draw(ctx, size, size + OVERHANG);
}

const STATES = Object.keys(BOT_STATES) as BotStateName[];

if (want("hero")) {
  const row = section("Hero");
  for (const [s, t] of [["idle", 0.7], ["approval", 1.9], ["error", 2.6], ["finished", 3.3]] as const) {
    drawBot(row, posed(s, t), 280, s);
  }
}

if (want("states")) {
  const row = section("States (resting pose)");
  STATES.forEach((s, i) => drawBot(row, posed(s, 0.4 + i * 0.29), 104, s));
  const row2 = section("Gaze + flare");
  const looks: [string, number, number][] = [
    ["look L", -0.62, 0], ["look R", 0.62, 0], ["look up", 0, -0.5], ["look down", 0, 0.5],
  ];
  for (const [label, yaw, pitch] of looks) {
    const e = posed("idle");
    e.yaw = yaw; e.pitch = pitch;
    drawBot(row2, e, 104, label);
  }
  const fl = posed("approval"); fl.flare = 1; drawBot(row2, fl, 104, "approval flare");
  const er = posed("error"); er.flare = 0.8; drawBot(row2, er, 104, "error flare");
  const bl = posed("idle"); bl.open = 0.1; drawBot(row2, bl, 104, "blink");
  const bo = posed("idle"); bo.boot = 0.45; bo.ignite = 0; drawBot(row2, bo, 104, "boot 0.45");
  const sc = posed("idle"); sc.morph = 1; sc.slotH = 0.3; sc.slotHTarget = 0.2; drawBot(row2, sc, 104, "scan stance");
  const lv = posed("idle"); lv.boost = 1; lv.eyeOverride = "heart"; drawBot(row2, lv, 104, "love boost");
}

if (want("eyes")) {
  const row = section("LED glyphs");
  const eyes: EyeShape[] = [
    "pill", "wide", "dot", "line", "flat", "happy", "closed",
    "spiral", "heart", "star", "tired", "wink", "cup",
  ];
  eyes.forEach((shape, i) => {
    const e = posed(i % 3 === 0 ? "idle" : i % 3 === 1 ? "thinking" : "approval", 0.9 + i * 0.13);
    e.eyeOverride = shape;
    e.badge = null;
    drawBot(row, e, 84, shape);
  });
}

if (want("sizes")) {
  const row = section("Sizes (body diameter)");
  for (const d of [14, 20, 28, 44, 62, 110]) {
    const e = posed("idle", 2.1);
    const w = d / 0.6;
    const ctx = cell(row, w, w + 16, `${d}px`);
    e.particleOverhang = 16;
    e.draw(ctx, w, w + 16);
  }
}

if (want("minis")) {
  const colors = ["#22C55E", "#EAB308", "#60A5FA", "#E879F9", "#E86A6A", "#3E86E0"];
  const states: BotStateName[] = ["working", "approval", "thinking", "finished", "error", "sleeping"];
  const row = section("Mini bots — real size (13 px body) and ×4");
  colors.forEach((c, i) => {
    const e = posed(states[i], 0.5 + i * 0.4);
    e.isMini = true;
    e.bodyColor = hexToRGB(c);
    const w = 13 / 0.6;
    const ctx = cell(row, w, w, "");
    e.draw(ctx, w, w);
  });
  colors.forEach((c, i) => {
    const e = posed(states[i], 0.5 + i * 0.4);
    e.isMini = true;
    e.bodyColor = hexToRGB(c);
    const w = 88;
    const ctx = cell(row, w, w, `${states[i]}`);
    e.draw(ctx, w, w);
  });
}

if (want("greet")) {
  const row = section("Greeting (t in s; c = collapsing)");
  const g = new Greeting();
  const S = Number(params.get("gs") ?? 0.56);
  const ft = params.get("gt")?.split(",").map(Number);
  const frames: [number, number][] = ft ? ft.map((t) => [t, Infinity]) : [
    [0.15, Infinity], [0.45, Infinity], [0.8, Infinity], [1.05, Infinity],
    [1.45, Infinity], [1.9, Infinity], [2.75, Infinity], [3.5, Infinity],
    [4.3, Infinity], [5.0, 4.75], [5.2, 4.75],
  ];
  for (const [t, tc] of frames) {
    const ctx = cell(row, 640 * S, 150 * S, Number.isFinite(tc) ? `c ${t}` : `${t}`);
    ctx.scale(S, S);
    g.drawFrame(ctx, t, tc);
  }
}

if (want("drop")) {
  const row = section("Drop → scan → upload (t from drop, s)");
  State.droppedFile = { name: "notes.md", path: "C:/tmp/notes.md" };
  const S = 0.56;
  const shots = new Set([-0.6, -0.1, 0.08, 0.2, 0.32, 0.45, 0.6, 0.78, 1.05, 1.3, 2.6, 4.0, 4.35]);
  const grab = new Map<number, ReturnType<typeof UploadSeq.frame>>();
  // Walk the timeline in small steps (the sequence is a spring simulation).
  const T0 = 100;
  UploadSeq.clock = T0;
  UploadSeq.enterZone(520, 96);
  let dropped = false;
  for (let k = 0; k <= 900; k++) {
    const t = k / 120; // since entry
    UploadSeq.clock = T0 + t;
    if (!dropped) UploadSeq.updateCursor(520 - Math.min(1, t / 0.9) * 360, 96);
    if (!dropped && t >= 1.4) { dropped = true; UploadSeq.performDrop(2.4); }
    const rel = Math.round((t - 1.4) * 1000) / 1000;
    const f = UploadSeq.frame();
    for (const s of shots) if (Math.abs(rel - s) < 1 / 240 && !grab.has(s)) grab.set(s, f);
  }
  UploadSeq.deactivate();
  UploadSeq.clock = null;
  for (const s of [...shots].sort((a, b) => a - b)) {
    const f = grab.get(s);
    if (!f) continue;
    const fig = document.createElement("figure");
    const uc = new UploadCanvas({ ask() {}, cancel() {} });
    const c = uc.el.querySelector("canvas")!;
    uc.draw(f, 3 + s);
    c.style.width = `${640 * S}px`;
    c.style.height = `${176 * S}px`;
    const cap = document.createElement("figcaption");
    cap.textContent = `${s}`;
    fig.append(c, cap);
    row.append(fig);
  }
}

if (want("icons")) {
  const row = section("App icon", "icons");
  for (const bg of ["l", "m", "d"]) {
    const tile = document.createElement("div");
    tile.className = `tile ${bg}`;
    for (const [file, px] of [["32x32.png", 16], ["32x32.png", 24], ["32x32.png", 32], ["128x128.png", 48], ["128x128.png", 128]] as const) {
      const img = document.createElement("img");
      img.src = `/src-tauri/icons/${file}?v=${Date.now()}`;
      img.width = px;
      img.height = px;
      tile.append(img);
    }
    row.append(tile);
  }
}
