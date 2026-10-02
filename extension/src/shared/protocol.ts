// Messages between the extension's parts. Three hops:
//
//   page (MAIN world net guard) --MessagePort--> content script --runtime.sendMessage--> service worker
//
// The first hop uses `BridgeRequest` / `BridgeReply` (ids, no chrome.* in the page world).
// The second hop carries the same operations as `SwRequest` with the site added.

import type { SiteId } from "./sites.ts";

export type MaskMode = "full" | "known";

/** Page -> content script (over the MessageChannel). */
export type BridgeRequest =
  | { id: number; type: "maskMany"; texts: string[]; mode: MaskMode; source?: string }
  | { id: number; type: "tripwire"; texts: string[]; url?: string }
  | { id: number; type: "clipboard"; items: Record<string, string> }
  | { type: "notify"; level: "info" | "warn" | "error"; text: string }
  | { type: "event"; kind: "masked" | "blocked" | "upload"; count: number; keys: string[] };

export interface BridgeReply {
  id: number;
  ok: boolean;
  error?: string;
  [k: string]: unknown;
}

/** Content script -> page. */
export interface BridgeState {
  type: "state";
  enabled: boolean;
  /** Vault entries known to the engine (0 lets the net guard skip tripwire round trips). */
  vaultSize: number;
  engine: boolean;
}

/** Content script / popup / offscreen -> service worker. */
export type SwRequest =
  | { type: "maskMany"; site: SiteId; texts: string[]; mode: MaskMode; source?: string }
  | { type: "tripwire"; site: SiteId; texts: string[]; url?: string }
  | { type: "rehydrate"; texts: string[] }
  | { type: "resolve"; keys: string[] }
  | { type: "known" }
  | { type: "scan"; text: string }
  | { type: "event"; site: SiteId; kind: "masked" | "blocked" | "upload" | "restored"; count: number; keys: string[] }
  | { type: "toast"; level: "info" | "warn" | "error"; text: string }
  | { type: "sanitize-pdf"; site: SiteId; name: string; base64: string }
  | { type: "state"; site: SiteId }
  | { type: "status" }
  | { type: "setSite"; site: SiteId; enabled: boolean }
  | { type: "clearVault" }
  | { type: "relink" };

export interface Prefs {
  sites: Record<SiteId, boolean>;
}

export const DEFAULT_PREFS: Prefs = { sites: { chatgpt: true, claude: true, deepseek: true } };

/** Non-sensitive state mirrored into chrome.storage.local so content scripts can watch it. */
export interface SyncState {
  rev: number;
  vaultSize: number;
  engine: boolean;
  linked: boolean;
}

export const STORAGE_PREFS = "zuko.prefs";
export const STORAGE_SYNC = "zuko.sync";

/** Reply of the offscreen PDF extractor. */
export interface PdfExtract {
  ok: boolean;
  error?: string;
  pages: number;
  markdown: string;
  /** False when no page has a text layer (scanned PDF). */
  hasText: boolean;
  warnings: string[];
}
