// Shared test helpers: the REAL zuko_core.wasm loaded in Node (like app/core/wasm/smoke.mjs),
// a Brain wired to it, jsdom windows patched with Node's fetch-era classes, and bridge doubles.

import { existsSync, readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { JSDOM } from "jsdom";
import { Brain } from "../src/background/brain.ts";
import { Engine, WasmEngine } from "../src/shared/engine.ts";
import { BridgeUnavailable } from "../src/main-world/bridge.ts";

const here = dirname(fileURLToPath(import.meta.url));
export const extRoot = resolve(here, "..");
export const repoRoot = resolve(extRoot, "..");

// Fake but correctly shaped values (the same ones the engine's own smoke test uses).
export const KEY = "sk-proj-ZukoFake0123456789abcdefghijklmnopqrstuvwx";
export const MAIL = "rahim.uddin@gmail.com";
export const CARD = "4242 4242 4242 4242";
export const AWS = "AKIAZ7VRSQ4XNWPLM3KD";

export function wasmPath() {
  const candidates = [
    process.env.ZUKO_WASM,
    join(repoRoot, "app", "target", "wasm32-unknown-unknown", "release", "zuko_core.wasm"),
    join(extRoot, "dist", "zuko_core.wasm"),
  ].filter(Boolean);
  const found = candidates.find((p) => existsSync(p));
  if (!found) {
    throw new Error(
      "zuko_core.wasm not found. Build it first: `cd app && cargo build -p zuko-core --target wasm32-unknown-unknown --release` " +
        `(looked in: ${candidates.join(", ")})`,
    );
  }
  return found;
}

let compiled;
async function module() {
  compiled ??= await WebAssembly.compile(readFileSync(wasmPath()));
  return compiled;
}

/** A fresh engine instance (its own empty vault) over the real WASM. */
export async function realEngine() {
  return new Engine(await WasmEngine.load(await module()));
}

export function memoryStore(initial = {}) {
  const data = new Map(Object.entries(initial));
  return {
    data,
    async get(k) {
      return data.has(k) ? structuredClone(data.get(k)) : undefined;
    },
    async set(k, v) {
      if (v === undefined) data.delete(k);
      else data.set(k, structuredClone(v));
    },
  };
}

export async function realBrain({ store = memoryStore(), link = null } = {}) {
  const brain = new Brain(store, () => 1_700_000_000);
  await brain.attach(await realEngine());
  brain.link = link;
  return brain;
}

/** A jsdom window with Node's Blob/File/FormData/Request/... so the net guard sees real bodies. */
export function makeWindow({ url = "https://chatgpt.com/", html = "<!doctype html><html><body></body></html>" } = {}) {
  const dom = new JSDOM(html, { url, pretendToBeVisual: true });
  const w = dom.window;
  const globals = { Blob, File, FormData, URLSearchParams, URL, Request, Response, Headers, TextEncoder, TextDecoder, ArrayBuffer, ReadableStream };
  for (const [k, v] of Object.entries(globals)) Object.defineProperty(w, k, { value: v, configurable: true, writable: true });
  w.queueMicrotask = queueMicrotask;
  w.__dom = dom;
  return w;
}

/** The page-side bridge, answering straight from a Brain (no MessageChannel). */
export class DirectBridge {
  constructor(brain, { enabled = true, site = "chatgpt" } = {}) {
    this.brain = brain;
    this.site = site;
    this.enabled = enabled;
    this.posts = [];
    this.requests = [];
  }
  get state() {
    return { type: "state", enabled: this.enabled, vaultSize: this.brain.vaultSize(), engine: true, known: true };
  }
  connected() {
    return true;
  }
  onState() {}
  post(msg) {
    this.posts.push(msg);
    return true;
  }
  async request(msg) {
    this.requests.push(msg);
    switch (msg.type) {
      case "maskMany":
        return { ok: true, ...(await this.brain.maskMany(msg.texts, { site: this.site, mode: msg.mode, source: msg.source })) };
      case "tripwire":
        return { ok: true, ...this.brain.tripwire(msg.texts) };
      default:
        throw new BridgeUnavailable(`unsupported in test: ${msg.type}`);
    }
  }
}

/** A bridge whose engine never answers: every request fails like a dead service worker. */
export class DeadBridge {
  constructor() {
    this.posts = [];
    this.requests = [];
    this.state = { type: "state", enabled: true, vaultSize: 0, engine: false, known: false };
  }
  connected() {
    return true;
  }
  onState() {}
  post(msg) {
    this.posts.push(msg);
    return true;
  }
  async request(msg) {
    this.requests.push(msg);
    throw new BridgeUnavailable("engine-unavailable");
  }
}

/** Install a recording fetch on a window; returns the list of calls. */
export function recordFetch(win) {
  const calls = [];
  win.fetch = async (input, init) => {
    calls.push({ input, init });
    return new Response("ok");
  };
  return calls;
}

export async function bodyText(call) {
  const b = call.init?.body;
  if (typeof b === "string") return b;
  if (b instanceof Blob) return await b.text();
  if (b instanceof URLSearchParams) return b.toString();
  if (b instanceof Uint8Array) return new TextDecoder().decode(b);
  if (call.input instanceof Request) return await call.input.clone().text();
  return b;
}

export const tick = (ms = 0) => new Promise((r) => setTimeout(r, ms));
