// The link to the Zuko desktop app through the native messaging host `app.zuko.host`
// (app/native-host, CONTRACTS.md section 4). Everything here is optional: when the host is not
// registered or the app is closed, `linked` stays false and the extension works alone.
//
// The host answers strictly one reply per request and echoes our `id`, so a reply that
// arrives after a timeout can never be mistaken for the answer to a later request.

import type { AppLink } from "./brain.ts";
import type { MaskReport, VaultJson } from "../shared/engine.ts";

export const HOST_NAME = "app.zuko.host";

interface Pending {
  resolve: (reply: any | null) => void;
  timer: ReturnType<typeof setTimeout>;
}

const RETRY_AFTER_MS = 20_000;

export class NativeLink implements AppLink {
  private port: chrome.runtime.Port | null = null;
  private pending = new Map<number, Pending>();
  private nextId = 1;
  private connecting: Promise<boolean> | null = null;
  private lastAttempt = 0;

  linked = false;
  appVersion: string | null = null;
  lastError: string | null = null;
  /** Called after every successful link (the service worker syncs policy and vault here). */
  onLinked: (() => Promise<void> | void) | null = null;
  onChange: (() => void) | null = null;

  private readonly version: string;
  private readonly host: string;

  constructor(version: string, host = HOST_NAME) {
    this.version = version;
    this.host = host;
  }

  /** Connects if not linked. Throttled: a missing host is retried at most every 20 s unless `force`. */
  connect(force = false): Promise<boolean> {
    if (this.linked) return Promise.resolve(true);
    if (this.connecting) return this.connecting;
    if (!force && Date.now() - this.lastAttempt < RETRY_AFTER_MS) return Promise.resolve(false);
    this.lastAttempt = Date.now();
    this.connecting = this.doConnect().finally(() => {
      this.connecting = null;
    });
    return this.connecting;
  }

  /** Fire-and-forget retry used on hot paths. */
  maybeReconnect(): void {
    if (!this.linked && !this.connecting && Date.now() - this.lastAttempt >= RETRY_AFTER_MS) void this.connect();
  }

  private async doConnect(): Promise<boolean> {
    let port: chrome.runtime.Port;
    try {
      port = chrome.runtime.connectNative(this.host);
    } catch (e) {
      this.lastError = e instanceof Error ? e.message : String(e);
      return false;
    }
    this.port = port;
    port.onMessage.addListener((m) => this.onMessage(m));
    port.onDisconnect.addListener(() => this.onDisconnect(port));

    const hello = await this.rpc({ op: "hello", version: this.version }, 2500);
    if (hello?.ok === true) {
      this.linked = true;
      this.appVersion = typeof hello.version === "string" ? hello.version : null;
      this.lastError = null;
      this.onChange?.();
      try {
        await this.onLinked?.();
      } catch (e) {
        console.warn("[Zuko] syncing with the desktop app failed:", e);
      }
      return true;
    }
    if (!this.lastError) this.lastError = typeof hello?.error === "string" ? hello.error : "the desktop app did not answer";
    // Not linked: let the host process (and the service worker's keep-alive) go.
    try {
      port.disconnect();
    } catch {
      /* already gone */
    }
    this.cleanup(port);
    return false;
  }

  private onMessage(m: any): void {
    const id = typeof m?.id === "number" ? m.id : null;
    if (id === null) return;
    const p = this.pending.get(id);
    if (!p) return; // answered too late: its caller already gave up
    clearTimeout(p.timer);
    this.pending.delete(id);
    p.resolve(m);
  }

  private onDisconnect(port: chrome.runtime.Port): void {
    // Reading lastError marks it handled; without this Chrome logs an "unchecked" warning.
    const err = chrome.runtime.lastError?.message;
    if (err) this.lastError = err;
    this.cleanup(port);
  }

  private cleanup(port: chrome.runtime.Port): void {
    if (this.port !== port) return;
    this.port = null;
    const wasLinked = this.linked;
    this.linked = false;
    for (const p of this.pending.values()) {
      clearTimeout(p.timer);
      p.resolve(null);
    }
    this.pending.clear();
    if (wasLinked) this.onChange?.();
  }

  /** One request, one reply (or null on timeout / disconnect). */
  private rpc(msg: Record<string, unknown>, timeoutMs = 5000): Promise<any | null> {
    const port = this.port;
    if (!port) return Promise.resolve(null);
    const id = this.nextId++;
    return new Promise((resolve) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        resolve(null);
      }, timeoutMs);
      this.pending.set(id, { resolve, timer });
      try {
        port.postMessage({ ...msg, id });
      } catch {
        clearTimeout(timer);
        this.pending.delete(id);
        resolve(null);
      }
    });
  }

  private async request(msg: Record<string, unknown>, timeoutMs?: number): Promise<any | null> {
    if (!this.linked) return null;
    const r = await this.rpc(msg, timeoutMs);
    return r?.ok === true ? r : null;
  }

  async mask(text: string, site: string): Promise<{ text: string; report: MaskReport } | null> {
    const r = await this.request({ op: "mask", text, site }, 20_000);
    if (!r || typeof r.text !== "string" || !r.report) return null;
    return { text: r.text, report: r.report as MaskReport };
  }

  async fetchVault(): Promise<VaultJson | null> {
    const r = await this.request({ op: "vault" }, 8000);
    if (!r) return null;
    let v = r.vault;
    if (typeof v === "string") {
      try {
        v = JSON.parse(v);
      } catch {
        return null;
      }
    }
    if (!v || typeof v !== "object") return null;
    return { entries: Array.isArray(v.entries) ? v.entries : [], counters: v.counters ?? {} };
  }

  async fetchPolicy(): Promise<Record<string, unknown> | null> {
    const r = await this.request({ op: "policy" }, 5000);
    return r && r.detector && typeof r.detector === "object" ? (r.detector as Record<string, unknown>) : null;
  }

  event(e: { kind: "masked" | "blocked" | "upload"; site: string; count: number; keys: string[] }): void {
    if (!this.linked || !this.port) return;
    void this.rpc({ op: "event", ...e }, 3000);
  }

  disconnect(): void {
    const port = this.port;
    if (!port) return;
    try {
      port.disconnect();
    } catch {
      /* already gone */
    }
    this.cleanup(port);
  }
}
