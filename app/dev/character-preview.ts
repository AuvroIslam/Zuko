// Dev harness: renders Zuko deterministically — every state, every eye glyph,
// the concept-sheet expressions, the fire (punch, fireball flight, flick,
// aura, hover ring), mini bots, the launch greeting and the drop/scan sequence
// at fixed timestamps — so a single headless screenshot shows the whole
// character. Not part of the app bundle.
//
//   /dev/character-preview.html?only=hero,concept,states,eyes,fire,minis,sizes,light,greet,drop,icons
//   &hs=280 sets the hero size.

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
  const hs = Number(params.get("hs") ?? 280);
  for (const [s, t] of [["idle", 0.7], ["approval", 1.9], ["error", 2.6], ["finished", 3.3]] as const) {
    drawBot(row, posed(s, t), hs, s);
  }
}

if (want("concept")) {
  // The concept sheet's emotes: neutral, happy, angry, thinking, excited, sleepy.
  const row = section("Concept sheet expressions");
  const sheet: [string, BotStateName, EyeShape | null, (e: BotEngine) => void][] = [
    ["neutral", "idle", null, () => {}],
    ["happy", "idle", "happy", () => {}],
    ["angry", "error", null, () => {}],
    ["thinking", "thinking", null, () => {}],
    ["excited", "idle", "star", (e) => { e.boost = 0.8; }],
    ["sleepy", "sleeping", null, () => {}],
    ["love", "idle", "heart", (e) => { e.boost = 1; }],
    ["surprised", "idle", "dot", () => {}],
    ["wink", "idle", "wink", () => {}],
    ["dizzy", "dizzy", null, () => {}],
  ];
  sheet.forEach(([label, st, eye, fn], i) => {
    const e = posed(st, 0.6 + i * 0.31);
    if (eye) e.eyeOverride = eye;
    fn(e);
    drawBot(row, e, 150, label);
  });
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
    "spiral", "heart", "star", "tired", "wink", "cup", "angry", "think",
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

/** A fresh engine posed `t` seconds into a punch / flick / swirl from rest. */
function fireFrame(
  kind: "punch" | "flick" | "swirl", t: number, target: { x: number; y: number } | null,
  w: number, h: number, bx: number, by: number, R: number, state: BotStateName = "idle",
) {
  const e = posed(state, 0.9);
  e.clock = 10;
  const scratch = document.createElement("canvas").getContext("2d")!;
  e.drawAt(scratch, bx, by, R);
  if (kind === "punch") e.shootFire(target, { sound: false, kind: state === "error" ? "danger" : "ember" });
  else if (kind === "flick") e.fireFlick(target, { sound: false });
  else e.fireSwirl();
  e.clock = 10 + t;
  return (ctx: CanvasRenderingContext2D) => {
    ctx.clearRect(0, 0, w, h);
    e.drawAt(ctx, bx, by, R);
    e.drawFx(ctx);
  };
}

if (want("fire")) {
  const W = 300, H = 150;
  const target = { x: 270, y: 70 };
  const row = section("Fire punch → fireball → burst (t in s, target at right)");
  // &fs=2 doubles the punch frames to inspect the arm and swirl.
  const fs = Number(params.get("fs") ?? 1);
  const ft = params.get("ft")?.split(",").map(Number) ?? [0.07, 0.14, 0.2, 0.26, 0.32, 0.4, 0.5, 0.62, 0.8];
  for (const t of ft) {
    const ctx = cell(row, W * fs, H * fs, `${t}`);
    ctx.scale(fs, fs);
    fireFrame("punch", t, target, W, H, 60, 92, 30)(ctx);
    ctx.strokeStyle = "rgba(255,255,255,0.15)";
    ctx.strokeRect(target.x - 4, target.y - 4, 8, 8);
  }
  const row2 = section("Block (danger), upward shot, flick, swirl");
  for (const t of [0.24, 0.36, 0.55]) {
    fireFrame("punch", t, { x: 250, y: 30 }, W, H, 60, 92, 30, "error")(cell(row2, W, H, `block ${t}`));
  }
  for (const t of [0.12, 0.3, 0.5]) {
    fireFrame("flick", t, { x: 200, y: 60 }, W, H, 60, 92, 30)(cell(row2, W, H, `flick ${t}`));
  }
  for (const t of [0.2, 0.45]) fireFrame("swirl", t, null, W, H, 150, 88, 30)(cell(row2, W, H, `swirl ${t}`));
  const row3 = section("Aura, hover ring, ready stance, posed punch");
  const au = posed("idle", 1.2); au.setFireAura(true); au.snapToState(); drawBot(row3, au, 150, "aura");
  const au2 = posed("approval", 1.6); au2.setFireAura(true); au2.snapToState(); drawBot(row3, au2, 150, "approval + aura");
  const rg = posed("working", 0.8); rg.setFireRing(true); rg.snapToState(); drawBot(row3, rg, 150, "hover ring");
  const rg2 = posed("working", 2.1); rg2.setFireRing(true); rg2.snapToState(); drawBot(row3, rg2, 104, "ring 104");
  const st = posed("idle", 1.1); st.morph = 1; st.slotH = 0.3; st.slotHTarget = 0.2;
  {
    const ctx = cell(row3, 150, 190, "ready stance");
    st.particleOverhang = 40; st.draw(ctx, 150, 190); st.drawFx(ctx);
  }
  const pp = posed("idle", 1.4); pp.punch = 0.8; pp.punchAngle = -0.2; pp.fistFire = 0.9;
  {
    const ctx = cell(row3, 200, 190, "posed punch");
    pp.drawAt(ctx, 70, 115, 37); pp.drawFx(ctx);
  }
}

if (want("light")) {
  const row = section("On light backgrounds", "light");
  for (const [st, d] of [["idle", 110], ["approval", 62], ["working", 44], ["finished", 28], ["idle", 20]] as const) {
    const e = posed(st, 1.1);
    const w = d / 0.6;
    const ctx = cell(row, w, w + 20, `${st} ${d}`, true);
    e.particleOverhang = 20;
    e.draw(ctx, w, w + 20);
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
  const S = Number(params.get("ds") ?? 0.56);
  const dt = params.get("dt")?.split(",").map(Number);
  const shots = dt ? new Set(dt) : new Set([-0.6, -0.1, 0.05, 0.2, 0.35, 0.5, 0.68, 0.85, 1.05, 1.2, 2.5, 3.8, 4.2, 4.6]);
  const grab = new Map<number, ReturnType<typeof UploadSeq.frame>>();
  // Walk the timeline in small steps (the sequence is a spring simulation).
  const T0 = 100;
  UploadSeq.clock = T0;
  UploadSeq.enterZone(520, 96);
  let dropped = false;
  for (let k = 0; k <= 960; k++) {
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
  // Run `ICON_PREVIEW=node_modules/.icon-preview npm run icons` first for the
  // exact ICO sizes; 128 px comes straight from src-tauri/icons.
  const row = section("App icon (ICO sizes 16, 24, 32, 48, 64 at 1:1; 128 px)", "icons");
  const v = Date.now();
  for (const bg of ["l", "m", "d"]) {
    const tile = document.createElement("div");
    tile.className = `tile ${bg}`;
    for (const size of [16, 24, 32, 48, 64]) {
      const img = document.createElement("img");
      img.src = `/node_modules/.icon-preview/icon-${size}.png?v=${v}`;
      img.width = size;
      img.height = size;
      tile.append(img);
    }
    const big = document.createElement("img");
    big.src = `/src-tauri/icons/128x128.png?v=${v}`;
    big.width = 128;
    big.height = 128;
    tile.append(big);
    row.append(tile);
  }
  const zoom = section("16 and 32 px, zoomed ×6", "icons");
  for (const bg of ["l", "d"]) {
    const tile = document.createElement("div");
    tile.className = `tile ${bg}`;
    for (const size of [16, 24, 32]) {
      const img = document.createElement("img");
      img.src = `/node_modules/.icon-preview/icon-${size}.png?v=${v}`;
      img.width = size * 6;
      img.height = size * 6;
      tile.append(img);
    }
    zoom.append(tile);
  }
}
