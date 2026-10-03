// The link to the Zuko desktop app through the native messaging host `app.zuko.host`
// (app/native-host, CONTRACTS.md section 4). Everything here is optional: when the host is not
// registered or the app is closed, `linked` stays false and the extension works alone.
//
// The host answers strictly one reply per request and echoes our `id`, so a reply that
// arrives after a timeout can never be mistaken for the answer to a later request.
//
// Heartbeat: the app shows the extension as connected only while it heard from it in the
// last minute, so a linked extension says `hello` every 25 s. The open native port keeps
// the service worker alive meanwhile (Chrome 105+), so a plain timer is enough here; the
// service worker's alarm (sw.ts) covers the time it is asleep, unlinked. A heartbeat the
// host answers with an error (the app was closed) drops the link, and the next reconnect
// re-syncs policy and vault when the app is back.

import type { AppLink, DeepScanReply, LocalAiStatus } from "./brain.ts";
import type { MaskReport, VaultJson } from "../shared/engine.ts";

export const HOST_NAME = "app.zuko.host";

interface Pending {
  resolve: (reply: any | null) => void;
  timer: ReturnType<typeof setTimeout>;
}

const RETRY_AFTER_MS = 20_000;
/** Under half of the app's 60 s "connected" window, so one late beat never drops it. */
export const HEARTBEAT_MS = 25_000;
/** Heartbeats that may go unanswered in a row (a busy app) before the link counts as dead. */
const MISSED_BEATS = 2;

export class NativeLink implements AppLink {
  private port: chrome.runtime.Port | null = null;
  private pending = new Map<number, Pending>();
  private nextId = 1;
  private connecting: Promise<boolean> | null = null;
  private lastAttempt = 0;
  private beatTimer: ReturnType<typeof setInterval> | null = null;
  private missed = 0;

  linked = false;
  /** The app's local-AI status as of the last policy read (null: unknown / older app). */
  localAi: LocalAiStatus | null = null;
  private policyAt = 0;
  appVersion: string | null = null;
  lastError: string | null = null;
  /** Called after every successful link (the service worker syncs policy and vault here). */
  onLinked: (() => Promise<void> | void) | null = null;
  onChange: (() => void) | null = null;

  private readonly version: string;
  private readonly host: string;
  private readonly heartbeatMs: number;
  private readonly beatTimeoutMs: number;
  private readonly retryAfterMs: number;

  /** `timing` shortens the heartbeat, its answer deadline and the retry throttle (tests). */
  constructor(
    version: string,
    host = HOST_NAME,
    timing: { heartbeatMs?: number; beatTimeoutMs?: number; retryAfterMs?: number } = {},
  ) {
    this.version = version;
    this.host = host;
    this.heartbeatMs = timing.heartbeatMs ?? HEARTBEAT_MS;
    this.beatTimeoutMs = timing.beatTimeoutMs ?? 5000;
    this.retryAfterMs = timing.retryAfterMs ?? RETRY_AFTER_MS;
  }

  /**
   * The periodic check (the service worker's alarm calls it too): a linked extension says
   * hello so the app keeps showing it as connected; an unlinked one tries to link again
   * (throttled like every other retry).
   */
  async tick(): Promise<void> {
    if (!this.linked) {
      await this.connect();
      return;
    }
    const port = this.port;
    const r = await this.rpc({ op: "hello", version: this.version }, this.beatTimeoutMs);
    if (!this.linked || this.port !== port) return; // the link changed meanwhile
    if (r?.ok === true) {
      this.missed = 0;
      if (typeof r.version === "string") this.appVersion = r.version;
      return;
    }
    // The host answered for an app that is gone, or nobody answered twice in a row.
    if (r !== null || ++this.missed >= MISSED_BEATS) {
      if (typeof r?.error === "string") this.lastError = r.error;
      this.disconnect();
    }
  }

  private startHeartbeat(): void {
    this.stopHeartbeat();
    this.missed = 0;
    const timer = setInterval(() => void this.tick(), this.heartbeatMs);
    // Node (the tests) would otherwise stay up for the timer; a browser timer has no unref.
    (timer as unknown as { unref?: () => void }).unref?.();
    this.beatTimer = timer;
  }

  private stopHeartbeat(): void {
    if (this.beatTimer !== null) clearInterval(this.beatTimer);
    this.beatTimer = null;
  }

  /**
   * Connects if not linked. Throttled: a missing host is retried at most every 20 s unless
   * `force` (the heartbeat alarm in sw.ts fires every 30 s, so each one gets its attempt).
   */
  connect(force = false): Promise<boolean> {
    if (this.linked) return Promise.resolve(true);
    if (this.connecting) return this.connecting;
    if (!force && Date.now() - this.lastAttempt < this.retryAfterMs) return Promise.resolve(false);
    this.lastAttempt = Date.now();
    this.connecting = this.doConnect().finally(() => {
      this.connecting = null;
    });
    return this.connecting;
  }

  /** Fire-and-forget retry used on hot paths. */
  maybeReconnect(): void {
    if (!this.linked && !this.connecting && Date.now() - this.lastAttempt >= this.retryAfterMs) void this.connect();
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
      this.startHeartbeat();
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
    this.stopHeartbeat();
    const wasLinked = this.linked;
    this.linked = false;
    this.localAi = null;
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
    const r = await this.request({ op: "policy" }, 6000);
    this.policyAt = Date.now();
    if (r) {
      const a = r.localAi;
      this.localAi =
        a && typeof a === "object"
          ? {
              enabled: a.enabled === true,
              reachable: a.reachable === true,
              model: typeof a.model === "string" ? a.model : "",
              waitForPromptScan: a.waitForPromptScan === true,
            }
          : null; // an older app: no local AI
    }
    return r && r.detector && typeof r.detector === "object" ? (r.detector as Record<string, unknown>) : null;
  }

  /** Re-reads the app's local-AI status when the last read is older than `maxAgeMs`. */
  async refreshLocalAi(maxAgeMs = 30_000): Promise<void> {
    if (!this.linked || Date.now() - this.policyAt < maxAgeMs) return;
    this.policyAt = Date.now(); // one refresh at a time
    await this.fetchPolicy();
  }

  async deepScan(text: string, site: string, wait: boolean): Promise<DeepScanReply | null> {
    // The app bounds a waited scan by its own timeout; the host gives up after 45 s.
    const r = await this.request({ op: "deepScan", text, site, wait }, wait ? 44_000 : 6000);
    if (!r) return null;
    if (r.enabled !== true) {
      if (this.localAi) this.localAi = { ...this.localAi, enabled: false };
      return { enabled: false, added: [], vault: null };
    }
    let v = r.vault;
    if (typeof v === "string") {
      try {
        v = JSON.parse(v);
      } catch {
        v = null;
      }
    }
    const vault: VaultJson | null =
      v && typeof v === "object" ? { entries: Array.isArray(v.entries) ? v.entries : [], counters: v.counters ?? {} } : null;
    const added = Array.isArray(r.added)
      ? r.added.filter((a: any) => a && typeof a.key === "string").map((a: any) => ({ key: a.key as string, label: String(a.label ?? "") }))
      : [];
    return { enabled: true, added, vault };
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
