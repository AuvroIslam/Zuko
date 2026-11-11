// A fake `chrome` (storage, messaging, offscreen, native port) plus the real service worker
// running on top of it, for the service-worker and integration tests. Importing this file
// installs globalThis.chrome and loads src/background/sw.ts exactly once per process.

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import * as pdfjs from "pdfjs-dist/legacy/build/pdf.mjs";
import { extractPdfText } from "../src/offscreen/pdf-text.ts";
import { wasmPath } from "./helpers.mjs";

export const EXT_ID = "zukotestextensionid";
const wasmBytes = readFileSync(wasmPath());

export const sessionData = new Map();
export const localData = new Map();
export const messageListeners = [];
export const changeListeners = [];
export const toasts = [];
export const appEvents = [];
/** Alarms the service worker created, by name. */
export const alarms = new Map();
const alarmListeners = [];
/** Fires an alarm the way Chrome does when its time comes. */
export const fireAlarm = (name) => alarmListeners.forEach((l) => l({ name, scheduledTime: Date.now() }));
let offscreenOpen = false;
export const offscreen = {
  get open() {
    return offscreenOpen;
  },
};
let nativeFactory = () => failingPort("Specified native messaging host not found.");
/** Chooses what chrome.runtime.connectNative returns next. */
export const setNative = (f) => {
  nativeFactory = f;
};
export { appPort, failingPort };

function area(map, name) {
  const pick = (keys) => (keys == null ? [...map.keys()] : Array.isArray(keys) ? keys : [keys]);
  return {
    async get(keys) {
      return Object.fromEntries(pick(keys).filter((k) => map.has(k)).map((k) => [k, structuredClone(map.get(k))]));
    },
    async set(items) {
      const changes = {};
      for (const [k, v] of Object.entries(items)) {
        changes[k] = { oldValue: map.get(k), newValue: structuredClone(v) };
        map.set(k, structuredClone(v));
      }
      changeListeners.forEach((l) => l(changes, name));
    },
    async remove(keys) {
      for (const k of pick(keys)) map.delete(k);
    },
  };
}

function failingPort(message) {
  const disc = [];
  let gone = false;
  const port = {
    name: "app.zuko.host",
    postMessage() {
      if (gone) throw new Error("Attempting to use a disconnected port object");
    },
    disconnect() {
      gone = true;
    },
    onMessage: { addListener() {} },
    onDisconnect: { addListener: (l) => disc.push(l) },
  };
  setTimeout(() => {
    gone = true;
    globalThis.chrome.runtime.lastError = { message };
    disc.forEach((l) => l());
    globalThis.chrome.runtime.lastError = undefined;
  }, 0);
  return port;
}

/** A port whose far end is `handler(message) -> reply | undefined`, like the native host + app. */
function appPort(handler) {
  const msg = [];
  const disc = [];
  let gone = false;
  const port = {
    name: "app.zuko.host",
    postMessage(m) {
      if (gone) throw new Error("Attempting to use a disconnected port object");
      setTimeout(async () => {
        const reply = await handler(m);
        if (reply && !gone) msg.forEach((l) => l({ ...reply, id: m.id }));
      }, 0);
    },
    disconnect() {
      gone = true;
    },
    onMessage: { addListener: (l) => msg.push(l) },
    onDisconnect: { addListener: (l) => disc.push(l) },
    // test hooks
    dropConnection() {
      gone = true;
      disc.forEach((l) => l());
    },
  };
  return port;
}

globalThis.chrome = {
  runtime: {
    id: EXT_ID,
    lastError: undefined,
    getURL: (p) => `chrome-extension://${EXT_ID}/${p}`,
    getManifest: () => ({ version: "9.9.9" }),
    onMessage: { addListener: (l) => messageListeners.push(l) },
    onInstalled: { addListener() {} },
    onStartup: { addListener() {} },
    connectNative: (name) => nativeFactory(name),
    getContexts: async () => (offscreenOpen ? [{}] : []),
    // The service worker only sends to the offscreen document: run the real extractor.
    sendMessage: async (msg) => {
      assert.equal(msg.target, "offscreen");
      const bytes = new Uint8Array(Buffer.from(msg.base64, "base64"));
      return extractPdfText(pdfjs, bytes);
    },
  },
  storage: {
    session: area(sessionData, "session"),
    local: area(localData, "local"),
    onChanged: { addListener: (l) => changeListeners.push(l) },
  },
  offscreen: {
    createDocument: async () => {
      offscreenOpen = true;
    },
  },
  alarms: {
    create: async (name, info) => {
      alarms.set(name, { name, scheduledTime: Date.now(), ...info });
    },
    get: async (name) => alarms.get(name),
    onAlarm: { addListener: (l) => alarmListeners.push(l) },
  },
  tabs: {
    sendMessage: async (tabId, msg, opts) => {
      toasts.push({ tabId, msg, opts });
    },
  },
};

const origFetch = globalThis.fetch;
globalThis.fetch = async (url, ...rest) =>
  String(url).endsWith("zuko_core.wasm") ? new Response(wasmBytes, { headers: { "content-type": "application/wasm" } }) : origFetch(url, ...rest);

await import("../src/background/sw.ts");

/** Sends a message the way chrome would and returns the reply. */
export function send(message, sender) {
  return new Promise((resolve, reject) => {
    const handled = messageListeners.some((l) => l(message, sender, resolve) === true);
    if (!handled) reject(new Error("no listener took the message"));
  });
}
export const page = { id: EXT_ID, url: "https://chatgpt.com/c/abc", tab: { id: 7 }, frameId: 0 };
export const popup = { id: EXT_ID, url: `chrome-extension://${EXT_ID}/popup.html` };
