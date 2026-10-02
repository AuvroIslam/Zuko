// Vault helpers that the WASM engine does not provide: merging the desktop app's vault with
// the extension's session vault, and the extra forms of a value the tripwire looks for.

import { parseKey } from "./placeholders.ts";
import type { VaultEntry, VaultJson } from "./engine.ts";

/**
 * Union of two vaults. `remote` (the desktop app) wins every key it already uses, because
 * those placeholders may already be in Claude Code sessions. A local entry whose value the
 * app does not have keeps its key when that key is free, and gets a fresh number otherwise
 * (`renamed` lists those: their placeholders in old browser chats will no longer restore).
 */
export function mergeVaults(
  local: VaultJson,
  remote: VaultJson,
): { vault: VaultJson; renamed: Array<{ from: string; to: string }> } {
  const entries: VaultEntry[] = remote.entries.map((e) => ({ ...e }));
  const byValue = new Set(entries.map((e) => e.value));
  const usedKeys = new Set(entries.map((e) => e.key));
  const counters: Record<string, number> = { ...remote.counters };
  const bump = (key: string) => {
    const p = parseKey(key);
    if (p && (counters[p.kind] ?? 0) < p.n) counters[p.kind] = p.n;
  };
  entries.forEach((e) => bump(e.key));

  const renamed: Array<{ from: string; to: string }> = [];
  for (const e of local.entries) {
    if (byValue.has(e.value)) continue;
    let key = e.key;
    if (usedKeys.has(key)) {
      const kind = parseKey(key)?.kind ?? e.kind;
      const next = Math.max(counters[kind] ?? 0, local.counters[kind] ?? 0) + 1;
      key = `${kind}_${next}`;
      renamed.push({ from: e.key, to: key });
    }
    usedKeys.add(key);
    byValue.add(e.value);
    entries.push({ ...e, key });
    bump(key);
  }
  for (const [kind, n] of Object.entries(local.counters)) if ((counters[kind] ?? 0) < n) counters[kind] = n;
  return { vault: { entries, counters }, renamed };
}

// ---------------------------------------------------------------------------------------
// Needles

export interface EncodedNeedle {
  key: string;
  form: "url" | "base64" | "hex";
  text: string;
}

export interface EscapedNeedle {
  key: string;
  /** The value as it appears inside a JSON string. */
  text: string;
}

const MIN_ENCODED_BYTES = 8;

function jsonEscaped(v: string): string {
  return JSON.stringify(v).slice(1, -1);
}

function jsonEscapedAscii(v: string): string {
  let out = "";
  for (const ch of jsonEscaped(v)) {
    if (ch.charCodeAt(0) < 128) out += ch;
    else for (let i = 0; i < ch.length; i++) out += "\\u" + ch.charCodeAt(i).toString(16).padStart(4, "0");
  }
  return out;
}

function toBase64(bytes: Uint8Array): string {
  let s = "";
  for (const b of bytes) s += String.fromCharCode(b);
  return btoa(s).replace(/=+$/, "");
}

/**
 * Substrings of base64(prefix + value + suffix) that depend only on `value`, for the three
 * possible alignments of the value inside a base64 stream. Standard and URL-safe alphabets.
 */
export function base64Needles(value: string): string[] {
  const bytes = new TextEncoder().encode(value);
  if (bytes.length < MIN_ENCODED_BYTES) return [];
  const out = new Set<string>();
  for (let k = 0; k < 3; k++) {
    const padded = new Uint8Array(k + bytes.length);
    padded.set(bytes, k);
    const enc = toBase64(padded);
    const start = [0, 2, 3][k]!;
    const end = (k + bytes.length) % 3 === 0 ? 0 : 1;
    const needle = enc.slice(start, enc.length - end);
    if (needle.length >= 8) {
      out.add(needle);
      out.add(needle.replace(/\+/g, "-").replace(/\//g, "_"));
    }
  }
  return [...out];
}

export function entryEscapedNeedles(e: VaultEntry): EscapedNeedle[] {
  const forms = new Set<string>();
  const esc = jsonEscaped(e.value);
  if (esc !== e.value) forms.add(esc);
  const ascii = jsonEscapedAscii(e.value);
  if (ascii !== e.value) forms.add(ascii);
  return [...forms].map((text) => ({ key: e.key, text }));
}

export function entryEncodedNeedles(e: VaultEntry): EncodedNeedle[] {
  const bytes = new TextEncoder().encode(e.value);
  if (bytes.length < MIN_ENCODED_BYTES) return [];
  const out: EncodedNeedle[] = [];
  const url = encodeURIComponent(e.value);
  if (url !== e.value) out.push({ key: e.key, form: "url", text: url });
  const hex = [...bytes].map((b) => b.toString(16).padStart(2, "0")).join("");
  out.push({ key: e.key, form: "hex", text: hex }, { key: e.key, form: "hex", text: hex.toUpperCase() });
  for (const text of base64Needles(e.value)) out.push({ key: e.key, form: "base64", text });
  return out;
}
