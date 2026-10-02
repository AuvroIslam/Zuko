// The decision logic of the service worker, free of chrome.* APIs so it runs in Node tests:
// masking (desktop app first, WASM engine as fallback), the tripwire, rehydration lookups,
// vault persistence and counters.

import { Engine, EngineError, type Finding, type MaskReport, type VaultJson } from "../shared/engine.ts";
import { entryEncodedNeedles, entryEscapedNeedles, mergeAddOnly, mergeVaults, type EncodedNeedle, type EscapedNeedle } from "../shared/vault.ts";
import { canonicalKeys, wrap } from "../shared/placeholders.ts";
import type { SiteId } from "../shared/sites.ts";

export class EngineUnavailable extends Error {
  constructor(message = "engine-unavailable") {
    super(message);
  }
}

/** Where session state lives (chrome.storage.session in the extension, a Map in tests). */
export interface KeyValueStore {
  get(key: string): Promise<unknown>;
  set(key: string, value: unknown): Promise<void>;
}

/** The app's local-AI status (policy op). */
export interface LocalAiStatus {
  enabled: boolean;
  reachable: boolean;
  model: string;
  waitForPromptScan: boolean;
}

/** The app's answer to a deep scan: what it learned (add-only) and its vault as of now. */
export interface DeepScanReply {
  enabled: boolean;
  added: Array<{ key: string; label: string }>;
  vault: VaultJson | null;
}

/** The desktop app, reached through the native messaging host. */
export interface AppLink {
  readonly linked: boolean;
  /** Local-AI status as last read from the app; absent or null: no deep scan. */
  readonly localAi?: LocalAiStatus | null;
  /** Asks the app's local AI for a second look at already-masked text. Null: no answer. */
  deepScan?(text: string, site: string, wait: boolean): Promise<DeepScanReply | null>;
  mask(text: string, site: string): Promise<{ text: string; report: MaskReport } | null>;
  fetchVault(): Promise<VaultJson | null>;
  event(e: { kind: "masked" | "blocked" | "upload"; site: string; count: number; keys: string[] }): void;
}

export interface MaskManyResult {
  texts: string[];
  /** Replacements made by this call. */
  count: number;
  /** Every known placeholder present in the output texts (new or already there). */
  keys: string[];
  newKeys: string[];
  via: "app" | "wasm";
  /** Vault entries the local AI added while this call waited for it. */
  aiAdded: number;
}

export type TripwireResult =
  | { action: "pass"; texts: string[]; count: 0; keys: [] }
  | { action: "masked"; texts: string[]; count: number; keys: string[] }
  | { action: "block"; reason: string; keys: string[] };

export interface ScanSummary {
  count: number;
  items: Array<{ kind: string; label: string }>;
}

export interface SiteStats {
  masked: number;
  blocked: number;
  uploads: number;
  restored: number;
}

const EMPTY_STATS = (): SiteStats => ({ masked: 0, blocked: 0, uploads: 0, restored: 0 });

export class Brain {
  engine: Engine | null = null;
  engineError: string | null = null;
  link: AppLink | null = null;

  private rev = 0;
  private needleRev = -1;
  private escaped: EscapedNeedle[] = [];
  private encoded: EncodedNeedle[] = [];
  private values = new Set<string>();
  private keySet = new Set<string>();
  private stats: Record<string, SiteStats> = {};

  private readonly store: KeyValueStore;
  private readonly now: () => number;

  constructor(store: KeyValueStore, now: () => number = () => Math.floor(Date.now() / 1000)) {
    this.store = store;
    this.now = now;
  }

  // ---- lifecycle -----------------------------------------------------------------------

  /** Attaches the engine and restores the session's detector settings, vault and counters. */
  async attach(engine: Engine | null, error: string | null = null): Promise<void> {
    this.engine = engine;
    this.engineError = engine ? null : (error ?? "engine not loaded");
    this.stats = ((await this.store.get("stats")) as Record<string, SiteStats> | undefined) ?? {};
    if (!engine) return;
    const detector = (await this.store.get("detector")) as Record<string, unknown> | undefined;
    if (detector) {
      try {
        engine.configure(detector);
      } catch {
        // A stale or hand-edited config must not stop the engine: defaults apply.
      }
    }
    const vault = (await this.store.get("vault")) as VaultJson | undefined;
    if (vault) {
      try {
        engine.loadVault(vault);
      } catch {
        // Corrupt session vault: start empty rather than refuse to protect.
      }
    }
    this.rev++;
  }

  private need(): Engine {
    if (!this.engine) throw new EngineUnavailable(this.engineError ?? "engine-unavailable");
    return this.engine;
  }

  get engineLoaded(): boolean {
    return this.engine !== null;
  }

  vaultSize(): number {
    return this.engine ? this.engine.exportVault().entries.length : 0;
  }

  // ---- vault ---------------------------------------------------------------------------

  private async persistVault(): Promise<void> {
    if (!this.engine) return;
    this.rev++;
    await this.store.set("vault", this.engine.exportVault());
  }

  /** Merges the desktop app's vault into the local one. Returns how many local keys were renumbered. */
  async adoptRemoteVault(remote: VaultJson): Promise<number> {
    const e = this.need();
    const { vault, renamed } = mergeVaults(e.exportVault(), remote);
    e.loadVault(vault);
    await this.persistVault();
    return renamed.length;
  }

  async applyDetector(detector: Record<string, unknown> | null): Promise<void> {
    const e = this.need();
    e.configure(detector);
    await this.store.set("detector", detector ?? undefined);
  }

  async clearVault(): Promise<void> {
    const e = this.need();
    e.loadVault(null);
    await this.persistVault();
  }

  private refreshNeedles(): void {
    if (this.needleRev === this.rev) return;
    const entries = this.need().exportVault().entries;
    this.escaped = entries.flatMap(entryEscapedNeedles);
    this.encoded = entries.flatMap(entryEncodedNeedles);
    this.values = new Set(entries.map((e) => e.value));
    this.keySet = new Set(entries.map((e) => e.key));
    this.needleRev = this.rev;
  }

  knownKeys(): string[] {
    return this.need()
      .views()
      .map((v) => v.key);
  }

  /** key -> value/kind/label for the keys that exist; the content script asks only for what it shows. */
  resolve(keys: string[]): Record<string, { value: string; kind: string; label: string }> {
    const entries = new Map(this.need().exportVault().entries.map((e) => [e.key, e]));
    const out: Record<string, { value: string; kind: string; label: string }> = {};
    for (const k of keys) {
      const e = entries.get(k);
      if (e) out[k] = { value: e.value, kind: e.kind, label: e.label };
    }
    return out;
  }

  rehydrate(texts: string[]): { texts: string[]; keys: string[] } {
    const e = this.need();
    const keys = new Set<string>();
    const out = texts.map((t) => {
      const r = e.rehydrate(t);
      r.keys.forEach((k) => keys.add(k));
      return r.text;
    });
    return { texts: out, keys: [...keys] };
  }

  // ---- masking -------------------------------------------------------------------------

  async maskMany(
    texts: string[],
    opts: {
      site: SiteId | string;
      mode: "full" | "known";
      source?: string;
      /** Called with N when the local AI's deep scan added N vault entries (also when it finishes later). */
      onAiAdded?: (n: number) => void;
    },
  ): Promise<MaskManyResult> {
    const e = this.need();
    const out: string[] = [];
    const newKeys = new Set<string>();
    let count = 0;
    let via: "app" | "wasm" = "wasm";
    let touchedVault = false;
    let aiAdded = 0;

    for (const text of texts) {
      if (text === "") {
        out.push(text);
        continue;
      }
      if (opts.mode === "known") {
        const r = e.maskKnown(text);
        out.push(r.text);
        count += r.count;
        continue;
      }
      let report: MaskReport | null = null;
      let masked: string | null = null;
      const link = this.link;
      if (link?.linked) {
        const r = await link.mask(text, String(opts.site)).catch(() => null);
        if (r) {
          masked = r.text;
          report = r.report;
          via = "app";
          if (r.report.newKeys.length > 0) {
            const remote = await link.fetchVault().catch(() => null);
            if (remote) await this.adoptRemoteVault(remote);
          } else if (r.text.includes("{{") && canonicalKeys(r.text).some((k) => !this.hasKey(k))) {
            // Values the app already knew (e.g. learned by its local AI) are placeholders here
            // that this vault has never seen: fetch them so they can be restored.
            const remote = await link.fetchVault().catch(() => null);
            if (remote) await this.adoptRemoteVault(remote);
          }
        }
      }
      if (masked === null || report === null) {
        const r = e.mask(text, opts.source ?? "browser", this.now());
        masked = r.text;
        report = r.report;
        if (r.report.newKeys.length > 0) touchedVault = true;
      }
      count += report.count;
      report.newKeys.forEach((k) => newKeys.add(k));
      const ai = await this.aiPass(masked, String(opts.site), opts);
      masked = ai.text;
      count += ai.count;
      aiAdded += ai.added;
      ai.newKeys.forEach((k) => newKeys.add(k));
      out.push(masked);
    }
    if (touchedVault) await this.persistVault();

    const keys = new Set<string>();
    for (const t of out) if (t.includes("{{")) e.rehydrate(t).keys.forEach((k) => keys.add(k));
    return { texts: out, count, keys: [...keys], newKeys: [...newKeys], via, aiAdded };
  }

  private hasKey(key: string): boolean {
    this.refreshNeedles();
    return this.keySet.has(key);
  }

  /**
   * The desktop app's local-AI deep scan of text that is ALREADY masked (the deterministic
   * pass above is never skipped or changed). It can only add vault entries: merging is
   * add-only. For uploads, and when the app says prompts should wait, the request is held until
   * the scan is back and the text is re-masked with what it learned; otherwise the scan runs in
   * the background and later messages benefit. Any failure leaves the text exactly as it was.
   */
  private async aiPass(
    masked: string,
    site: string,
    opts: { source?: string; onAiAdded?: (n: number) => void },
  ): Promise<{ text: string; count: number; added: number; newKeys: string[] }> {
    const none = { text: masked, count: 0, added: 0, newKeys: [] as string[] };
    const link = this.link;
    const ai = link?.localAi;
    if (!link?.linked || !link.deepScan || !ai?.enabled || !ai.reachable || masked.trim().length < 6) return none;
    const e = this.need();
    const wait = opts.source === "file" || ai.waitForPromptScan;
    const apply = async (r: DeepScanReply | null): Promise<string[]> => {
      if (!r?.enabled || !r.vault || r.vault.entries.length === 0) return [];
      const { vault, added } = mergeAddOnly(e.exportVault(), r.vault);
      if (added.length === 0) return [];
      e.loadVault(vault);
      await this.persistVault();
      return added.map((a) => a.key);
    };
    if (!wait) {
      void link
        .deepScan(masked, site, false)
        .then(apply)
        .then((keys) => {
          if (keys.length > 0) opts.onAiAdded?.(keys.length);
        })
        .catch(() => undefined);
      return none;
    }
    let keys: string[] = [];
    try {
      keys = await apply(await link.deepScan(masked, site, true));
    } catch {
      return none;
    }
    if (keys.length === 0) return none;
    const again = e.maskKnown(masked);
    opts.onAiAdded?.(keys.length);
    return { text: again.text, count: again.count, added: keys.length, newKeys: keys };
  }

  /** What the composer chip shows: how many distinct items would be masked. Never returns values. */
  scan(text: string): ScanSummary {
    const e = this.need();
    this.refreshNeedles();
    const findings: Finding[] = e.scan(text);
    const seen = new Set<string>();
    const items: ScanSummary["items"] = [];
    for (const f of findings) {
      if (this.values.has(f.value) || seen.has(f.value)) continue;
      seen.add(f.value);
      items.push({ kind: f.kind, label: f.label ?? f.kind });
    }
    if (this.values.size > 0) {
      const known = e.maskKnown(text);
      if (known.count > 0) {
        const keys = new Set(e.rehydrate(known.text).keys);
        const views = new Map(e.views().map((v) => [v.key, v]));
        for (const k of keys) items.push({ kind: views.get(k)?.kind ?? k, label: views.get(k)?.label ?? k });
      }
    }
    return { count: items.length, items };
  }

  // ---- tripwire ------------------------------------------------------------------------

  /**
   * Exact-value check for any outgoing text: vault values (raw and JSON-escaped) become
   * placeholders; encoded copies (URL, base64, hex) cannot be fixed in place, so the request
   * is blocked.
   */
  tripwire(texts: string[]): TripwireResult {
    const e = this.need();
    this.refreshNeedles();
    if (this.values.size === 0) return { action: "pass", texts, count: 0, keys: [] };

    const blockedKeys = new Set<string>();
    for (const text of texts) {
      for (const n of this.encoded) if (text.includes(n.text)) blockedKeys.add(n.key);
    }
    if (blockedKeys.size > 0) {
      return {
        action: "block",
        reason: "an encoded copy of a protected value (URL-encoded, base64 or hex) is in this request",
        keys: [...blockedKeys],
      };
    }

    let count = 0;
    const keys = new Set<string>();
    const out = texts.map((text) => {
      let t = text;
      const r = e.maskKnown(t);
      if (r.count > 0) {
        count += r.count;
        t = r.text;
      }
      for (const n of this.escaped) {
        if (t.includes(n.text)) {
          const parts = t.split(n.text);
          count += parts.length - 1;
          t = parts.join(wrap(n.key));
        }
      }
      if (t !== text) e.rehydrate(t).keys.forEach((k) => keys.add(k));
      return t;
    });
    if (count === 0) return { action: "pass", texts, count: 0, keys: [] };
    return { action: "masked", texts: out, count, keys: [...keys] };
  }

  // ---- counters ------------------------------------------------------------------------

  async bump(site: string, field: keyof SiteStats, by = 1): Promise<void> {
    if (by <= 0) return;
    const s = (this.stats[site] ??= EMPTY_STATS());
    s[field] += by;
    await this.store.set("stats", this.stats);
  }

  getStats(): Record<string, SiteStats> {
    return this.stats;
  }
}

export { EngineError };
