// Value lookups for the content script. Real values never live here permanently: the cache
// holds only the values of placeholders that actually appeared on this page, fetched from the
// service worker on demand, and is dropped when the vault changes.

import type { ValueSource } from "./rehydrate.ts";

export type Send = (msg: Record<string, unknown>) => Promise<any>;

export class SwValues implements ValueSource {
  private known = new Set<string>();
  private cache = new Map<string, string>();
  private readonly send: Send;

  constructor(send: Send) {
    this.send = send;
  }

  keys(): Set<string> {
    return this.known;
  }

  value(key: string): string | undefined {
    return this.cache.get(key);
  }

  async load(keys: string[]): Promise<void> {
    const need = keys.filter((k) => !this.cache.has(k));
    if (need.length === 0) return;
    const r = await this.send({ type: "resolve", keys: need });
    if (!r?.ok) return;
    for (const [k, v] of Object.entries(r.values as Record<string, { value: string }>)) this.cache.set(k, v.value);
  }

  /** Re-reads the list of known keys. Returns true when it changed. */
  async refreshKeys(): Promise<boolean> {
    const r = await this.send({ type: "known" });
    if (!r?.ok) return false;
    const next = new Set<string>(r.keys as string[]);
    const changed = next.size !== this.known.size || [...next].some((k) => !this.known.has(k));
    this.known = next;
    for (const k of [...this.cache.keys()]) if (!next.has(k)) this.cache.delete(k);
    return changed;
  }
}
