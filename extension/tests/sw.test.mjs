// The service worker end to end against a fake `chrome` (storage, messaging, offscreen, native
// port) with the REAL WASM engine, the real pdf.js and a real engine standing in for the app.

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import * as pdfjs from "pdfjs-dist/legacy/build/pdf.mjs";
import { extractPdfText } from "../src/offscreen/pdf-text.ts";
import { AWS, KEY, MAIL, realEngine, tick, wasmPath } from "./helpers.mjs";
import { makePdf } from "./pdf-fixtures.mjs";

const EXT_ID = "zukotestextensionid";
const wasmBytes = readFileSync(wasmPath());

// ---- the fake browser -----------------------------------------------------------------------

const sessionData = new Map();
const localData = new Map();
const messageListeners = [];
const changeListeners = [];
const toasts = [];
const appEvents = [];
let offscreenOpen = false;
let nativeFactory = () => failingPort("Specified native messaging host not found.");

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
function send(message, sender) {
  return new Promise((resolve, reject) => {
    const handled = messageListeners.some((l) => l(message, sender, resolve) === true);
    if (!handled) reject(new Error("no listener took the message"));
  });
}
const page = { id: EXT_ID, url: "https://chatgpt.com/c/abc", tab: { id: 7 }, frameId: 0 };
const popup = { id: EXT_ID, url: `chrome-extension://${EXT_ID}/popup.html` };

// ---- tests (they share one service worker, so they run in order) -----------------------------

test("boots with the real engine; no native host registered is a quiet 'not linked'", async () => {
  await tick(20); // the first connection attempt is still in flight at boot
  const s = await send({ type: "status" }, popup);
  assert.equal(s.ok, true);
  assert.equal(s.engine, true);
  assert.equal(s.linked, false);
  assert.match(s.linkError, /not found/);
  assert.equal(s.version, "9.9.9");
  assert.deepEqual(s.prefs.sites, { chatgpt: true, claude: true, deepseek: true });
});

test("only the extension's own pages and content scripts on the four sites are served", async () => {
  const bad = [
    { id: "someotherextension", url: "https://chatgpt.com/", tab: { id: 1 } },
    { id: EXT_ID, url: "https://evil.example/", tab: { id: 1 } },
    { id: EXT_ID, url: "https://chatgpt.com/" }, // no tab: not a content script
    {},
  ];
  for (const sender of bad) {
    const r = await send({ type: "resolve", keys: ["API_KEY_1"] }, sender);
    assert.equal(r.ok, false, JSON.stringify(sender));
  }
  // Content scripts cannot use UI-only operations.
  for (const msg of [{ type: "status" }, { type: "setSite", site: "chatgpt", enabled: false }, { type: "clearVault" }, { type: "relink" }]) {
    assert.equal((await send(msg, page)).ok, false, msg.type);
  }
  // ...and pages (popup) cannot pretend to be a site for page-only operations.
  assert.equal((await send({ type: "maskMany", site: "chatgpt", texts: ["x"], mode: "full" }, popup)).ok, false);
});

test("maskMany from a content script: masks, counts, persists to session storage, publishes a sync state", async () => {
  const r = await send({ type: "maskMany", site: "chatgpt", texts: [`key ${KEY} mail ${MAIL}`], mode: "full" }, page);
  assert.equal(r.ok, true);
  assert.deepEqual(r.texts, ["key {{API_KEY_1}} mail {{EMAIL_1}}"]);
  assert.equal(r.via, "wasm");
  assert.deepEqual(r.newKeys, ["API_KEY_1", "EMAIL_1"]);

  assert.equal(sessionData.get("zuko.vault").entries.length, 2, "vault mirrored to storage.session (memory only)");
  assert.ok(!localData.has("zuko.vault"), "never to disk");
  const sync = localData.get("zuko.sync");
  assert.deepEqual({ vaultSize: sync.vaultSize, engine: sync.engine, linked: sync.linked }, { vaultSize: 2, engine: true, linked: false });
  assert.ok(!JSON.stringify([...localData.values()]).includes(KEY), "no value ever reaches storage.local");

  const s = await send({ type: "status" }, popup);
  assert.equal(s.total.masked, 2);
  assert.equal(s.stats.chatgpt.masked, 2);
  assert.equal(s.vaultSize, 2);
});

test("resolve / known / rehydrate / scan / tripwire answer from the vault", async () => {
  assert.deepEqual((await send({ type: "known" }, page)).keys, ["API_KEY_1", "EMAIL_1"]);
  const v = await send({ type: "resolve", keys: ["API_KEY_1", "NOPE_1"] }, page);
  assert.deepEqual(Object.keys(v.values), ["API_KEY_1"]);
  assert.equal(v.values.API_KEY_1.value, KEY);
  assert.deepEqual((await send({ type: "rehydrate", texts: ["{{EMAIL_1}}"] }, page)).texts, [MAIL]);
  const scan = await send({ type: "scan", text: `${AWS} and ${KEY}` }, page);
  assert.equal(scan.count, 2);
  assert.ok(!JSON.stringify(scan).includes(AWS));
  const t = await send({ type: "tripwire", site: "claude", texts: [`leak ${KEY}`] }, { ...page, url: "https://claude.ai/new" });
  assert.equal(t.action, "masked");
  assert.deepEqual(t.texts, ["leak {{API_KEY_1}}"]);
  const blocked = await send({ type: "tripwire", site: "claude", texts: [Buffer.from(KEY).toString("base64")] }, page);
  assert.equal(blocked.action, "block");
  const s = await send({ type: "status" }, popup);
  assert.equal(s.stats.claude.masked, 1);
  assert.equal(s.stats.claude.blocked, 1);
});

test("per-site switch: the popup flips it, the state reply and storage follow", async () => {
  assert.equal((await send({ type: "state", site: "deepseek" }, page)).enabled, true);
  const r = await send({ type: "setSite", site: "deepseek", enabled: false }, popup);
  assert.equal(r.prefs.sites.deepseek, false);
  assert.equal(localData.get("zuko.prefs").sites.deepseek, false);
  assert.equal((await send({ type: "state", site: "deepseek" }, page)).enabled, false);
  assert.equal((await send({ type: "state", site: "chatgpt" }, page)).enabled, true);
  await send({ type: "setSite", site: "deepseek", enabled: true }, popup);
});

test("PDF upload: offscreen text extraction, masking, counters; a scanned PDF is reported as blocked", async () => {
  const b64 = (lines) => Buffer.from(makePdf(lines)).toString("base64");
  const ok = await send({ type: "sanitize-pdf", site: "chatgpt", name: "r.pdf", base64: b64([[`aws ${AWS}`, "other text"]]) }, page);
  assert.equal(ok.ok, true);
  assert.equal(ok.blocked, false);
  assert.equal(ok.markdown, "## Page 1\n\naws {{API_KEY_2}}\nother text\n");
  assert.equal(ok.count, 1);
  assert.equal(offscreenOpen, true, "the offscreen document was created on demand");

  const scanned = await send({ type: "sanitize-pdf", site: "chatgpt", name: "s.pdf", base64: b64([[]]) }, page);
  assert.equal(scanned.ok, true);
  assert.equal(scanned.blocked, true);
  assert.ok(scanned.warnings.some((w) => /No text layer/.test(w)));

  await send({ type: "event", site: "chatgpt", kind: "upload", count: 1, keys: [] }, page);
  assert.equal((await send({ type: "status" }, popup)).stats.chatgpt.uploads, 1);
});

test("toasts from sub-frames are relayed to the top frame of the same tab", async () => {
  const r = await send({ type: "toast", level: "error", text: "blocked something" }, { ...page, frameId: 3 });
  assert.equal(r.ok, true);
  assert.deepEqual(toasts.at(-1), { tabId: 7, msg: { type: "toast", level: "error", text: "blocked something" }, opts: { frameId: 0 } });
});

test("the desktop app: link, policy and vault sync, app-first masking, event reporting, graceful loss", async () => {
  const appEngine = await realEngine();
  appEngine.mask("someone.else@acme-corp.io"); // the app already holds EMAIL_1 for a different value
  const appPolicy = { customTerms: ["Project Falcon"] };
  let appDown = false;
  let port;
  nativeFactory = () => {
    port = appPort(async (m) => {
      if (appDown) return { ok: false, error: "Zuko desktop app is not running" };
      switch (m.op) {
        case "hello":
          return { ok: true, app: "zuko", version: "1.2.3" };
        case "policy":
          return { ok: true, detector: appPolicy };
        case "vault":
          return { ok: true, vault: appEngine.exportVault() };
        case "mask": {
          const r = appEngine.mask(m.text, "browser");
          return { ok: true, text: r.text, report: r.report };
        }
        case "event":
          appEvents.push(m);
          return { ok: true };
      }
      return { ok: false, error: "unknown op" };
    });
    return port;
  };

  const linked = await send({ type: "relink" }, popup);
  assert.equal(linked.linked, true);
  let s = await send({ type: "status" }, popup);
  assert.equal(s.linked, true);
  assert.equal(s.appVersion, "1.2.3");

  // Vault merge: the app keeps EMAIL_1; our own email moved to a free number.
  const keys = (await send({ type: "known" }, page)).keys.sort();
  assert.deepEqual(keys, ["API_KEY_1", "API_KEY_2", "EMAIL_1", "EMAIL_2"]);
  assert.equal((await send({ type: "resolve", keys: ["EMAIL_1"] }, page)).values.EMAIL_1.value, "someone.else@acme-corp.io");
  assert.equal((await send({ type: "resolve", keys: ["EMAIL_2"] }, page)).values.EMAIL_2.value, MAIL);

  // The app's detector settings were applied: the custom term is now detected locally too.
  assert.equal((await send({ type: "scan", text: "status of project falcon" }, page)).count, 1);

  // New values go through the app (one shared vault), and our engine learns the app's entries.
  const r = await send({ type: "maskMany", site: "claude", texts: ["call +8801712345678 now"], mode: "full" }, { ...page, url: "https://claude.ai/new" });
  assert.equal(r.via, "app");
  assert.match(r.texts[0], /^call \{\{PHONE_\d+\}\} now$/);
  assert.equal((await send({ type: "resolve", keys: r.keys }, page)).values[r.keys[0]].value, "+8801712345678");
  assert.equal(appEvents.filter((e) => e.kind === "masked").length, 0, "the app saw this mask itself: no duplicate event");

  // Blocks and uploads are reported to the app's activity feed.
  const t = await send({ type: "tripwire", site: "chatgpt", texts: [Buffer.from(KEY).toString("base64")] }, page);
  assert.equal(t.action, "block");
  await tick(20);
  assert.ok(appEvents.some((e) => e.op === "event" && e.kind === "blocked" && e.site === "chatgpt"));

  // The app goes away mid-session: masking carries on locally.
  port.dropConnection();
  await tick(5);
  s = await send({ type: "status" }, popup);
  assert.equal(s.linked, false);
  const local = await send({ type: "maskMany", site: "chatgpt", texts: [`again ${KEY}`], mode: "full" }, page);
  assert.equal(local.via, "wasm");
  assert.equal(local.texts[0], "again {{API_KEY_1}}");
});

test("an app that answers 'not running' to hello is a clean 'not linked' and releases the port", async () => {
  let disconnected = false;
  nativeFactory = () => {
    const p = appPort(async () => ({ ok: false, error: "Zuko desktop app is not running" }));
    const orig = p.disconnect;
    p.disconnect = () => {
      disconnected = true;
      orig();
    };
    return p;
  };
  const r = await send({ type: "relink" }, popup);
  assert.equal(r.linked, false);
  assert.match(r.error, /not running/);
  assert.equal(disconnected, true, "no idle native port keeping the worker alive");
});

test("clearing the session vault forgets every value", async () => {
  assert.equal((await send({ type: "clearVault" }, popup)).ok, true);
  assert.deepEqual((await send({ type: "known" }, page)).keys, []);
  assert.equal((await send({ type: "status" }, popup)).vaultSize, 0);
  assert.equal(sessionData.get("zuko.vault").entries.length, 0);
});
