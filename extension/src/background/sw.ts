// Zuko service worker: the engine (zuko_core.wasm), the session vault, the link to the
// desktop app, the offscreen PDF extractor and the per-site switches.
//
// MV3 service workers are killed after ~30 s idle (and live as long as a native port is
// open), so nothing important lives only in memory: the vault is mirrored to
// chrome.storage.session (memory-backed, never written to disk) after every change and the
// engine reloads from it on the next wake-up.

import { Brain, EngineUnavailable, type KeyValueStore } from "./brain.ts";
import { NativeLink } from "./native.ts";
import { Engine, WasmEngine } from "../shared/engine.ts";
import { DEFAULT_PREFS, STORAGE_PREFS, STORAGE_SYNC, type PdfExtract, type Prefs, type SwRequest, type SyncState } from "../shared/protocol.ts";
import { SITE_IDS, siteForHost, type SiteId } from "../shared/sites.ts";

const sessionStore: KeyValueStore = {
  async get(key) {
    return (await chrome.storage.session.get(`zuko.${key}`))[`zuko.${key}`];
  },
  async set(key, value) {
    if (value === undefined) await chrome.storage.session.remove(`zuko.${key}`);
    else await chrome.storage.session.set({ [`zuko.${key}`]: value });
  },
};

const brain = new Brain(sessionStore);
const link = new NativeLink(chrome.runtime.getManifest().version);
brain.link = link;

let syncRev = 0;
let prefs: Prefs = structuredClone(DEFAULT_PREFS);

/** Mirrors non-sensitive state to storage.local; content scripts watch it. */
async function publishSync(): Promise<void> {
  const state: SyncState = { rev: ++syncRev, vaultSize: brain.vaultSize(), engine: brain.engineLoaded, linked: link.linked };
  await chrome.storage.local.set({ [STORAGE_SYNC]: state });
}

async function loadPrefs(): Promise<void> {
  const stored = (await chrome.storage.local.get(STORAGE_PREFS))[STORAGE_PREFS] as Partial<Prefs> | undefined;
  prefs = { sites: { ...DEFAULT_PREFS.sites, ...(stored?.sites ?? {}) } };
}

async function loadEngine(): Promise<void> {
  try {
    const res = await fetch(chrome.runtime.getURL("zuko_core.wasm"));
    if (!res.ok) throw new Error(`zuko_core.wasm is missing from the extension (HTTP ${res.status})`);
    const engine = new Engine(await WasmEngine.load(await res.arrayBuffer()));
    await brain.attach(engine);
  } catch (e) {
    const msg = e instanceof Error ? e.message : String(e);
    console.error(
      "[Zuko] ENGINE NOT LOADED: " +
        msg +
        "\nBuild it with `cargo build -p zuko-core --target wasm32-unknown-unknown --release` in app/, then `npm run build` in extension/. " +
        "Until then Zuko BLOCKS requests that contain obvious secrets and cannot mask anything else.",
    );
    await brain.attach(null, msg);
  }
}

let linking: Promise<boolean> = Promise.resolve(false);

link.onChange = () => void publishSync();
link.onLinked = async () => {
  if (!brain.engineLoaded) return;
  const detector = await link.fetchPolicy();
  if (detector) await brain.applyDetector(detector);
  const vault = await link.fetchVault();
  if (vault) await brain.adoptRemoteVault(vault);
  await publishSync();
};

const ready: Promise<void> = (async () => {
  await loadPrefs();
  await loadEngine();
  linking = link.connect(true);
  await publishSync();
})();

// ---- helpers ---------------------------------------------------------------------------

const isExtensionPage = (s: chrome.runtime.MessageSender) =>
  s.id === chrome.runtime.id && typeof s.url === "string" && s.url.startsWith(chrome.runtime.getURL(""));

const isContentScript = (s: chrome.runtime.MessageSender) => {
  if (s.id !== chrome.runtime.id || !s.tab || typeof s.url !== "string") return false;
  try {
    return siteForHost(new URL(s.url).hostname) !== null;
  } catch {
    return false;
  }
};

const safeSite = (v: unknown): SiteId => (SITE_IDS.includes(v as SiteId) ? (v as SiteId) : "chatgpt");

async function report(site: SiteId, kind: "masked" | "blocked" | "upload", count: number, keys: string[], viaApp = false): Promise<void> {
  if (kind === "masked") await brain.bump(site, "masked", count);
  else if (kind === "blocked") await brain.bump(site, "blocked", Math.max(1, count));
  else await brain.bump(site, "uploads", 1);
  if (!viaApp) link.event({ kind, site, count, keys });
}

/** "AI deep scan: +N items" in the tab that sent the text (top frame: only it can show UI). */
function aiToast(tabId: number | undefined, n: number): void {
  void publishSync();
  if (tabId === undefined || n <= 0) return;
  const text = `AI deep scan: +${n} ${n === 1 ? "item" : "items"}`;
  void chrome.tabs.sendMessage(tabId, { type: "toast", level: "info", text }, { frameId: 0 }).catch(() => undefined);
}

let offscreenOpen: Promise<void> | null = null;
async function ensureOffscreen(): Promise<void> {
  const existing = await chrome.runtime.getContexts({ contextTypes: ["OFFSCREEN_DOCUMENT"] });
  if (existing.length > 0) return;
  offscreenOpen ??= chrome.offscreen
    .createDocument({
      url: "offscreen.html",
      reasons: ["WORKERS"],
      justification: "Extract text from PDFs locally with pdf.js so they can be sanitized before upload",
    })
    .catch((e) => {
      // Another wake-up of this worker already created it.
      if (!String(e?.message ?? e).includes("single offscreen")) throw e;
    })
    .finally(() => {
      offscreenOpen = null;
    });
  await offscreenOpen;
}

async function sanitizePdf(site: SiteId, base64: string, tabId?: number): Promise<Record<string, unknown>> {
  await ensureOffscreen();
  const r = (await chrome.runtime.sendMessage({ target: "offscreen", type: "pdf-extract", base64 })) as PdfExtract | undefined;
  if (!r || !r.ok) return { ok: false, error: r?.error ?? "the PDF reader did not answer" };
  if (!r.hasText) return { ok: true, blocked: true, pages: r.pages, warnings: r.warnings };
  link.maybeReconnect();
  const masked = await brain.maskMany([r.markdown], { site, mode: "full", source: "file", onAiAdded: (n) => aiToast(tabId, n) });
  await report(site, "masked", masked.count, masked.keys, masked.via === "app");
  await publishSync();
  return { ok: true, blocked: false, markdown: masked.texts[0], pages: r.pages, warnings: r.warnings, count: masked.count, keys: masked.keys };
}

async function status() {
  // The first connection attempt may still be in flight (a missing host answers a tick
  // later); report its outcome, not "not linked" for a link that is still being tried.
  await Promise.race([linking, new Promise((r) => setTimeout(r, 3000))]);
  await link.refreshLocalAi(5000).catch(() => undefined);
  const stats = brain.getStats();
  const total = { masked: 0, blocked: 0, uploads: 0, restored: 0 };
  for (const s of Object.values(stats)) {
    total.masked += s.masked;
    total.blocked += s.blocked;
    total.uploads += s.uploads;
    total.restored += s.restored;
  }
  return {
    ok: true,
    version: chrome.runtime.getManifest().version,
    engine: brain.engineLoaded,
    engineError: brain.engineError,
    linked: link.linked,
    appVersion: link.appVersion,
    localAi: link.linked ? link.localAi : null,
    linkError: link.lastError,
    vaultSize: brain.vaultSize(),
    prefs,
    stats,
    total,
  };
}

// ---- message router ----------------------------------------------------------------------

async function handle(msg: SwRequest, sender: chrome.runtime.MessageSender): Promise<unknown> {
  await ready;
  const fromPage = isContentScript(sender);
  const fromUi = isExtensionPage(sender);
  if (!fromPage && !fromUi) return { ok: false, error: "unauthorized sender" };

  switch (msg.type) {
    case "maskMany": {
      if (!fromPage) break;
      await Promise.race([linking, new Promise((r) => setTimeout(r, 1500))]);
      link.maybeReconnect();
      void link.refreshLocalAi().catch(() => undefined);
      const site = safeSite(msg.site);
      const tabId = sender.tab?.id;
      const r = await brain.maskMany(msg.texts, { site, mode: msg.mode, source: msg.source, onAiAdded: (n) => aiToast(tabId, n) });
      if (msg.mode === "full") {
        await report(site, "masked", r.count, r.keys, r.via === "app");
        if (r.newKeys.length > 0) await publishSync();
      }
      return { ok: true, ...r };
    }
    case "tripwire": {
      if (!fromPage) break;
      const site = safeSite(msg.site);
      const r = brain.tripwire(msg.texts);
      if (r.action === "masked") {
        await report(site, "masked", r.count, r.keys);
      } else if (r.action === "block") {
        await report(site, "blocked", 1, r.keys);
      }
      return { ok: true, ...r };
    }
    case "rehydrate":
      return { ok: true, ...brain.rehydrate(msg.texts) };
    case "resolve":
      return { ok: true, values: brain.resolve(msg.keys) };
    case "known":
      return { ok: true, keys: brain.knownKeys() };
    case "scan":
      return { ok: true, ...brain.scan(msg.text) };
    case "event": {
      if (!fromPage) break;
      const site = safeSite(msg.site);
      if (msg.kind === "restored") await brain.bump(site, "restored", msg.count);
      else await report(site, msg.kind, msg.count, msg.keys);
      return { ok: true };
    }
    case "toast": {
      // Frames other than the top one cannot show UI; relay to the top frame of their tab.
      const tabId = sender.tab?.id;
      if (!fromPage || tabId === undefined) break;
      await chrome.tabs.sendMessage(tabId, { type: "toast", level: msg.level, text: msg.text }, { frameId: 0 }).catch(() => undefined);
      return { ok: true };
    }
    case "sanitize-pdf":
      if (!fromPage) break;
      return sanitizePdf(safeSite(msg.site), msg.base64, sender.tab?.id);
    case "state": {
      const site = safeSite(msg.site);
      return { ok: true, enabled: prefs.sites[site] !== false, engine: brain.engineLoaded, linked: link.linked, vaultSize: brain.vaultSize() };
    }
    case "status":
      if (!fromUi) break;
      return status();
    case "setSite": {
      if (!fromUi) break;
      const site = safeSite(msg.site);
      prefs = { sites: { ...prefs.sites, [site]: msg.enabled === true } };
      await chrome.storage.local.set({ [STORAGE_PREFS]: prefs });
      return { ok: true, prefs };
    }
    case "clearVault": {
      if (!fromUi) break;
      await brain.clearVault();
      await publishSync();
      return { ok: true };
    }
    case "relink": {
      if (!fromUi) break;
      const ok = await link.connect(true);
      await publishSync();
      return { ok: true, linked: ok, error: link.lastError };
    }
  }
  return { ok: false, error: "unauthorized or unknown request" };
}

chrome.runtime.onMessage.addListener((message, sender, sendResponse) => {
  // Messages for the offscreen document are not ours.
  if (!message || typeof message !== "object" || message.target === "offscreen") return false;
  handle(message as SwRequest, sender).then(sendResponse, (e) => {
    const unavailable = e instanceof EngineUnavailable;
    if (!unavailable) console.error("[Zuko] request failed:", e);
    sendResponse({ ok: false, error: unavailable ? "engine-unavailable" : e instanceof Error ? e.message : String(e) });
  });
  return true; // answer asynchronously
});

chrome.storage.onChanged.addListener((changes, area) => {
  if (area === "local" && changes[STORAGE_PREFS]?.newValue) prefs = changes[STORAGE_PREFS].newValue as Prefs;
});

chrome.runtime.onStartup.addListener(() => void ready);
chrome.runtime.onInstalled.addListener(() => void ready);
