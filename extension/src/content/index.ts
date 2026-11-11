// Content script (ISOLATED world, document_start): the bridge between the page's net guard
// and the service worker, plus the on-page features: composer chip, DOM rehydration, copy
// rehydration, upload interception and toasts. Everything on-page degrades silently if the
// site's DOM changes; protection never depends on it (that is the net guard's job).

import { ContentBridge } from "./bridge.ts";
import { ComposerChip } from "./composer.ts";
import { installCopyHandler, writeRestoredClipboard } from "./clipboard.ts";
import { Rehydrator } from "./rehydrate.ts";
import { Overlay } from "./toast.ts";
import { UploadGuard, type PdfResult } from "./uploads.ts";
import { SwValues, type Send } from "./values.ts";
import { VariantMatcher } from "../shared/placeholders.ts";
import { STORAGE_PREFS, STORAGE_SYNC, type Prefs, type SyncState } from "../shared/protocol.ts";
import { siteForHost, type SiteId } from "../shared/sites.ts";

const site = siteForHost(location.hostname);
if (site && !(window as any).__zukoContent) {
  Object.defineProperty(window, "__zukoContent", { value: true, enumerable: false });
  main(site);
}

function main(site: SiteId): void {
  const win = window as Window & typeof globalThis;
  const isTop = win === win.top;
  let overlay: Overlay | null = null;
  const ui = () => (overlay ??= new Overlay(document));

  let enabled = true;
  let vaultSize = 0;
  let engine = true;
  let invalidatedShown = false;

  // ---- talking to the service worker ----------------------------------------------------

  const notify = (level: "info" | "warn" | "error", text: string, ttl?: number) => {
    if (isTop) ui().show({ level, text, ttl });
    else void send({ type: "toast", level, text });
  };

  const send: Send = async (msg) => {
    try {
      if (!chrome.runtime?.id) throw new Error("Extension context invalidated.");
      const reply = await chrome.runtime.sendMessage(msg);
      return reply ?? { ok: false, error: "engine-unavailable" };
    } catch (e) {
      if (!invalidatedShown && /context invalidated|Receiving end does not exist/i.test(String((e as Error)?.message ?? e))) {
        invalidatedShown = true;
        notify(
          "warn",
          "Zuko was reloaded or updated. Reload this page to restore full protection. Until then, messages with obvious secrets are blocked.",
          0,
        );
      }
      return { ok: false, error: "engine-unavailable" };
    }
  };

  const kindsOf = (keys: string[]) => [...new Set(keys.map((k) => k.replace(/_\d+$/, "").replace(/_/g, " ").toLowerCase()))].slice(0, 4).join(", ");

  // ---- bridge to the page ---------------------------------------------------------------

  const values = new SwValues(send);
  let rehydrator: Rehydrator | null = null;
  let chip: ComposerChip | null = null;

  const bridge = new ContentBridge(win, {
    async request(req) {
      switch (req.type) {
        case "maskMany": {
          const r = await send({ type: "maskMany", site, texts: req.texts, mode: req.mode, source: req.source });
          if (!r.ok) throw new Error(r.error ?? "engine-unavailable");
          if (r.count > 0) {
            vaultSize = Math.max(vaultSize, 1);
            if (req.mode === "full" && !req.source) {
              notify("info", `Masked ${r.count} ${r.count === 1 ? "item" : "items"} before sending${r.keys.length ? ` (${kindsOf(r.keys)})` : ""}.`, 4500);
            }
          }
          return { texts: r.texts, count: r.count, keys: r.keys, newKeys: r.newKeys };
        }
        case "tripwire": {
          const r = await send({ type: "tripwire", site, texts: req.texts, url: req.url });
          if (!r.ok) throw new Error(r.error ?? "engine-unavailable");
          if (r.action === "masked") notify("warn", `Removed ${r.count} protected ${r.count === 1 ? "value" : "values"} from an outgoing request.`, 6000);
          return r;
        }
        case "clipboard":
          return writeRestoredClipboard(req.items, {
            doc: document,
            matcher: () => rehydrator?.variantMatcher ?? EMPTY_MATCHER(),
            source: values,
          });
      }
    },
    notify: (level, text) => notify(level, text),
    event: (kind, count, keys) => void send({ type: "event", site, kind, count, keys }),
  });
  bridge.start(); // right now: the page's guard may already be waiting

  const pushState = () => bridge.pushState({ enabled, vaultSize, engine });

  // ---- state: per-site switch and vault changes -----------------------------------------

  const applyEnabled = () => {
    if (!isTop) return;
    if (rehydrator) {
      if (enabled) rehydrator.start();
      else rehydrator.stop();
    }
    if (!enabled) overlay?.setSwitch({ visible: false, revealed: true, aiName: AI_NAME[site], pick: () => {} });
    chip?.setEnabled(enabled);
  };

  const onVaultChanged = async () => {
    const changed = await values.refreshKeys();
    if (changed) rehydrator?.refresh();
  };

  void send({ type: "state", site }).then((r) => {
    if (!r.ok) {
      engine = false;
      pushState();
      return;
    }
    enabled = r.enabled !== false;
    engine = r.engine === true;
    vaultSize = Number(r.vaultSize) || 0;
    pushState();
    applyEnabled();
  });

  try {
    chrome.storage.onChanged.addListener((changes, area) => {
      if (area !== "local") return;
      const prefs = changes[STORAGE_PREFS]?.newValue as Prefs | undefined;
      if (prefs) {
        enabled = prefs.sites?.[site] !== false;
        pushState();
        applyEnabled();
      }
      const sync = changes[STORAGE_SYNC]?.newValue as SyncState | undefined;
      if (sync) {
        vaultSize = sync.vaultSize;
        engine = sync.engine;
        pushState();
        if (isTop) void onVaultChanged();
      }
    });
    chrome.runtime.onMessage.addListener((msg) => {
      if (msg?.type === "toast" && isTop && typeof msg.text === "string") ui().show({ level: msg.level === "error" || msg.level === "warn" ? msg.level : "info", text: msg.text });
      return false;
    });
  } catch {
    /* context already invalidated: the net guard falls back to its own scan */
  }

  // ---- uploads (every frame, registered now so we run before the page's listeners) -------

  new UploadGuard(win, {
    enabled: () => enabled,
    maskText: async (text) => {
      const r = await send({ type: "maskMany", site, texts: [text], mode: "full", source: "file" });
      if (!r.ok) throw new Error(r.error ?? "engine-unavailable");
      return { text: r.texts[0], count: r.count, keys: r.keys };
    },
    sanitizePdf: async (base64, name) => (await send({ type: "sanitize-pdf", site, name, base64 })) as PdfResult,
    notify: (level, text) => notify(level, text),
    report: (count, keys) => void send({ type: "event", site, kind: "upload", count, keys }),
  }).start();

  // ---- on-page features (top frame only) --------------------------------------------------

  if (!isTop) return;

  const onReady = (fn: () => void) => {
    if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", fn, { once: true });
    else fn();
  };

  onReady(() => {
    let restoredPending = 0;
    let restoredTimer: ReturnType<typeof setTimeout> | null = null;
    rehydrator = new Rehydrator({
      doc: document,
      site,
      source: values,
      onRestored: (count) => {
        restoredPending += count;
        restoredTimer ??= setTimeout(() => {
          void send({ type: "event", site, kind: "restored", count: restoredPending, keys: [] });
          restoredPending = 0;
          restoredTimer = null;
        }, 2500);
      },
      onToggle: (revealed) => {
        paintSwitch();
        notify(
          "info",
          revealed
            ? "Your view: real values, on this computer only."
            : `What ${AI_NAME[site]} got: the highlighted placeholders are all it ever saw of your data.`,
          3500,
        );
      },
      onTracked: () => paintSwitch(),
    });
    const paintSwitch = () =>
      ui().setSwitch({
        visible: enabled && !!rehydrator?.hasRestored,
        revealed: rehydrator?.isRevealed ?? true,
        aiName: AI_NAME[site],
        pick: (revealed) => {
          if (rehydrator && rehydrator.isRevealed !== revealed) rehydrator.toggle();
        },
      });

    chip = new ComposerChip(document, site, ui(), (text) => send({ type: "scan", text }));
    chip.start();
    chip.setEnabled(enabled);

    installCopyHandler({ doc: document, matcher: () => rehydrator!.variantMatcher, source: values, enabled: () => enabled });

    win.addEventListener(
      "keydown",
      (e) => {
        if (e.altKey && !e.ctrlKey && !e.metaKey && !e.shiftKey && e.code === "KeyR" && enabled) {
          e.preventDefault();
          e.stopImmediatePropagation();
          rehydrator?.toggle();
        }
      },
      true,
    );

    void values.refreshKeys().then(() => {
      rehydrator!.refresh();
      if (enabled) rehydrator!.start();
    });
  });
}

const EMPTY_MATCHER = () => new VariantMatcher([]);

/** What each site's assistant is called on the view switch. */
const AI_NAME: Record<SiteId, string> = { chatgpt: "ChatGPT", claude: "Claude", deepseek: "DeepSeek" };
