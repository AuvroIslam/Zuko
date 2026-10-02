// Installs the network patches in the page world: fetch, XMLHttpRequest, WebSocket.send,
// navigator.sendBeacon and the clipboard writers. Everything async goes through `Pipeline`;
// these wrappers only keep the original calling conventions intact (XHR.send and
// sendBeacon are synchronous for the page, WebSocket.send keeps its order).
//
// Principles:
//  * a bug in the guard must never break the site: on an unexpected error the request goes
//    out unchanged (the fail-safe secret scan inside Pipeline still runs for prompt endpoints);
//  * requests the guard has no reason to look at (no vault values, not a prompt/upload
//    endpoint) are passed straight to the original function, synchronously;
//  * only the page's own bytes are rewritten; headers and URLs are never touched.
//
// Ideas for the XHR/fetch wrapping come from Better-DeepSeek (MIT, EdgeTypE).

import type { BridgeReply } from "../shared/protocol.ts";
import type { SiteId } from "../shared/sites.ts";
import { createPageBridge, type PageBridge } from "./bridge.ts";
import { Pipeline, type GuardOutcome, type GuardWindow } from "./pipeline.ts";

export interface NetGuard {
  pipeline: Pipeline;
  bridge: PageBridge;
  /** Restores every patched function (tests). */
  uninstall(): void;
}

export function installNetGuard(win: GuardWindow, site: SiteId, bridge: PageBridge = createPageBridge(win)): NetGuard {
  const undo: Array<() => void> = [];
  const patch = <T extends object, K extends keyof T>(target: T | undefined, key: K, make: (orig: T[K]) => T[K]) => {
    if (!target || typeof target[key] !== "function") return;
    const orig = target[key];
    const wrapped = make(orig);
    try {
      Object.defineProperty(wrapped, "name", { value: (orig as any).name });
      Object.defineProperty(wrapped, "length", { value: (orig as any).length });
    } catch {
      /* cosmetic only */
    }
    target[key] = wrapped;
    undo.push(() => {
      target[key] = orig;
    });
  };

  const notify = (level: "info" | "warn" | "error", text: string) => {
    if (!bridge.post({ type: "notify", level, text })) fallbackBanner(win, text);
  };
  const pipeline = new Pipeline(win, site, bridge, notify);

  const blockedError = (reason: string) => new win.TypeError(`Zuko blocked this request: ${reason}`);

  // ---- fetch ---------------------------------------------------------------------------

  patch(win as any, "fetch", (origFetch: typeof fetch) => {
    const call = (input: RequestInfo | URL, init?: RequestInit) => origFetch.call(win, input, init);

    async function guarded(input: RequestInfo | URL, init: RequestInit | undefined, method: string, url: string): Promise<Response> {
      const req = input instanceof win.Request ? input : null;
      let body: unknown = init?.body;
      let contentType = headerOf(init?.headers ?? req?.headers, "content-type");
      try {
        if (body === undefined && req && req.body !== null && method !== "GET" && method !== "HEAD") {
          // The body is a stream: read a clone, leave the original untouched for the pass case.
          body = await req.clone().arrayBuffer();
          contentType ??= req.headers.get("content-type");
        }
        const out: GuardOutcome = await pipeline.guard({ url, method, body, contentType });
        if (out.action === "block") throw blockedError(out.reason);
        if (out.action === "replace") {
          if (req && init?.body === undefined) return origFetch.call(win, new win.Request(req, { body: out.body }));
          return origFetch.call(win, input, { ...init, body: out.body });
        }
      } catch (e) {
        if (e instanceof win.TypeError && String((e as Error).message).startsWith("Zuko blocked")) throw e;
        console.warn("[Zuko] guard error, request sent unchanged:", e);
      }
      return call(input, init);
    }

    return function zukoFetch(input: RequestInfo | URL, init?: RequestInit): Promise<Response> {
      try {
        const req = input instanceof win.Request ? input : null;
        const method = String(init?.method ?? req?.method ?? "GET").toUpperCase();
        const url = req ? req.url : String(input instanceof win.URL ? input.href : input);
        const hasBody = init?.body !== undefined ? init.body !== null : req ? req.body !== null : false;
        if (pipeline.needsGuard(url, method, hasBody ? (init?.body ?? req) : null)) return guarded(input, init, method, url);
      } catch (e) {
        console.warn("[Zuko] guard error, request sent unchanged:", e);
      }
      return call(input, init);
    } as typeof fetch;
  });

  // ---- XMLHttpRequest ------------------------------------------------------------------

  interface XhrMeta {
    method: string;
    url: string;
    contentType: string | null;
    pending: boolean;
    cancelled: boolean;
  }
  const xhrMeta = new WeakMap<XMLHttpRequest, XhrMeta>();
  const XHR = win.XMLHttpRequest?.prototype;

  patch(XHR, "open", (orig) =>
    function open(this: XMLHttpRequest, method: string, url: string | URL, ...rest: unknown[]) {
      xhrMeta.set(this, { method: String(method), url: String(url), contentType: null, pending: false, cancelled: false });
      return (orig as any).call(this, method, url, ...rest);
    } as typeof XHR.open);

  patch(XHR, "setRequestHeader", (orig) =>
    function setRequestHeader(this: XMLHttpRequest, name: string, value: string) {
      const m = xhrMeta.get(this);
      if (m && String(name).toLowerCase() === "content-type") m.contentType = String(value);
      return orig.call(this, name, value);
    } as typeof XHR.setRequestHeader);

  patch(XHR, "abort", (orig) =>
    function abort(this: XMLHttpRequest) {
      const m = xhrMeta.get(this);
      if (m?.pending) {
        m.cancelled = true;
        m.pending = false;
        orig.call(this);
        queueMicrotask(() => fire(this, ["abort", "loadend"]));
        return;
      }
      return orig.call(this);
    } as typeof XHR.abort);

  patch(XHR, "send", (origSend) =>
    function send(this: XMLHttpRequest, body?: Document | XMLHttpRequestBodyInit | null) {
      const m = xhrMeta.get(this);
      try {
        if (!m || !pipeline.needsGuard(m.url, m.method, body ?? null)) return origSend.call(this, body);
      } catch {
        return origSend.call(this, body);
      }
      m.pending = true;
      pipeline.guard({ url: m.url, method: m.method, body, contentType: m.contentType }).then(
        (out) => {
          if (m.cancelled || xhrMeta.get(this) !== m) return;
          m.pending = false;
          if (out.action === "block") {
            (XHR.abort as () => void).call(this); // back to UNSENT, then report a network error
            fire(this, ["error", "loadend"]);
            return;
          }
          origSend.call(this, out.action === "replace" ? (out.body as XMLHttpRequestBodyInit) : body);
        },
        (e) => {
          if (m.cancelled || xhrMeta.get(this) !== m) return;
          m.pending = false;
          console.warn("[Zuko] guard error, request sent unchanged:", e);
          origSend.call(this, body);
        },
      );
    } as typeof XHR.send);

  function fire(xhr: XMLHttpRequest, types: string[]): void {
    for (const t of types) {
      try {
        xhr.dispatchEvent(new win.ProgressEvent(t));
      } catch {
        /* a page handler threw: not ours */
      }
    }
  }

  // ---- WebSocket -----------------------------------------------------------------------

  const wsQueue = new WeakMap<WebSocket, { tail: Promise<void>; pending: number }>();
  patch(win.WebSocket?.prototype, "send", (orig) =>
    function send(this: WebSocket, data: string | ArrayBufferLike | Blob | ArrayBufferView) {
      const httpUrl = String(this.url).replace(/^ws/i, "http");
      const q = wsQueue.get(this);
      let need = false;
      try {
        need = (q?.pending ?? 0) > 0 || pipeline.needsGuard(httpUrl, "POST", data);
      } catch {
        need = false;
      }
      if (!need) return orig.call(this, data as any);
      const state = q ?? { tail: Promise.resolve(), pending: 0 };
      wsQueue.set(this, state);
      state.pending++;
      state.tail = state.tail
        .then(async () => {
          let out: GuardOutcome = { action: "pass" };
          try {
            out = await pipeline.guard({ url: httpUrl, method: "POST", body: data });
          } catch (e) {
            console.warn("[Zuko] guard error, message sent unchanged:", e);
          }
          if (out.action === "block") return;
          if (this.readyState !== 1) return;
          orig.call(this, (out.action === "replace" ? out.body : data) as any);
        })
        .catch(() => undefined)
        .finally(() => {
          state.pending--;
        });
    } as typeof WebSocket.prototype.send);

  // ---- sendBeacon ----------------------------------------------------------------------

  patch(win.Navigator?.prototype, "sendBeacon", (orig) =>
    function sendBeacon(this: Navigator, url: string | URL, data?: BodyInit | null) {
      let need = false;
      try {
        need = pipeline.needsGuard(String(url), "POST", data ?? null);
      } catch {
        need = false;
      }
      if (!need) return orig.call(this, url, data);
      const self = this;
      pipeline.guard({ url: String(url), method: "POST", body: data }).then(
        (out) => {
          if (out.action === "block") return;
          orig.call(self, url, out.action === "replace" ? out.body : data);
        },
        () => void orig.call(self, url, data),
      );
      return true;
    } as typeof Navigator.prototype.sendBeacon);

  // ---- clipboard: copy buttons must carry real values ----------------------------------
  // The page hands us text with placeholders; the content script (isolated world) restores
  // the values and writes the clipboard itself, so real values never enter the page world.

  const Clip = (win as any).Clipboard?.prototype as Clipboard | undefined;
  patch(Clip, "writeText", (orig) =>
    async function writeText(this: Clipboard, text: string) {
      try {
        if (bridge.state.enabled && typeof text === "string" && /\d/.test(text) && bridge.connected()) {
          const r = await bridge.request<BridgeReply & { written?: boolean }>({ type: "clipboard", items: { "text/plain": text } }, 3000);
          if (r.written) return;
        }
      } catch {
        /* fall through to the page's own copy */
      }
      return orig.call(this, text);
    } as Clipboard["writeText"]);

  patch(Clip, "write", (orig) =>
    async function write(this: Clipboard, items: ClipboardItems) {
      try {
        if (bridge.state.enabled && bridge.connected()) {
          const texts: Record<string, string> = {};
          for (const item of items) {
            for (const type of ["text/plain", "text/html"]) {
              if (item.types.includes(type)) texts[type] = await (await item.getType(type)).text();
            }
          }
          if (Object.keys(texts).length > 0 && Object.values(texts).some((t) => /\d/.test(t))) {
            const r = await bridge.request<BridgeReply & { written?: boolean }>({ type: "clipboard", items: texts }, 3000);
            if (r.written) return;
          }
        }
      } catch {
        /* fall through */
      }
      return orig.call(this, items);
    } as Clipboard["write"]);

  return {
    pipeline,
    bridge,
    uninstall() {
      undo.reverse().forEach((f) => f());
    },
  };
}

function headerOf(headers: HeadersInit | undefined | null, name: string): string | null {
  if (!headers) return null;
  if (typeof (headers as Headers).get === "function") return (headers as Headers).get(name);
  if (Array.isArray(headers)) return headers.find(([k]) => k.toLowerCase() === name)?.[1] ?? null;
  const rec = headers as Record<string, string>;
  const key = Object.keys(rec).find((k) => k.toLowerCase() === name);
  return key ? rec[key]! : null;
}

/** Used only when the content script cannot be reached: a plain closed-shadow banner. */
function fallbackBanner(win: GuardWindow, text: string): void {
  try {
    const doc = win.document;
    const host = doc.createElement("div");
    host.style.cssText = "all:initial;position:fixed;z-index:2147483647;right:16px;bottom:16px;";
    const root = host.attachShadow({ mode: "closed" });
    const box = doc.createElement("div");
    box.textContent = text;
    box.style.cssText =
      "font:13px/1.4 system-ui,sans-serif;color:#fff;background:#0f1530;border:1px solid #2ee6c5;border-left:4px solid #ff7a1a;border-radius:8px;padding:10px 14px;max-width:360px;box-shadow:0 6px 24px rgba(0,0,0,.4)";
    root.appendChild(box);
    (doc.body ?? doc.documentElement).appendChild(host);
    win.setTimeout(() => host.remove(), 12000);
  } catch {
    /* nothing left to try */
  }
}
