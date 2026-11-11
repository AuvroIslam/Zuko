// What happens to one outgoing request body: known prompt endpoints get their text fields
// masked with full detection; uploads of text files likewise; every other body goes through
// the tripwire (exact vault values only). Pure of patching: the wrappers in guard-core.ts
// call `guard()` and act on the outcome.
//
// Fail-safe rule: when the engine cannot be reached, a request to a prompt or upload endpoint
// that carries a high-confidence secret is BLOCKED; everything else passes unchanged (the
// tripwire cannot work without the vault, and breaking the site would not protect anything).

import { isPromptEndpoint, isUploadEndpoint, type SiteId } from "../shared/sites.ts";
import { MAX_SCAN_BYTES, isTextLikeName, isTextLikeType } from "../shared/files.ts";
import type { BridgeReply } from "../shared/protocol.ts";
import { BridgeUnavailable, type PageBridge } from "./bridge.ts";
import { findHighConfidenceSecret } from "./fallback.ts";
import { rewriteJson, rewriterFor, type MaskOutcome } from "./rewriters.ts";

export type GuardWindow = Window & typeof globalThis;

export interface GuardRequest {
  url: string;
  method: string;
  body: unknown;
  contentType?: string | null;
}

export type GuardOutcome =
  | { action: "pass" }
  | { action: "replace"; body: BodyInit }
  | { action: "block"; reason: string };

const PASS: GuardOutcome = { action: "pass" };

interface Ctx {
  site: SiteId;
  known: boolean;
  upload: boolean;
  url: URL;
  contentType: string;
}

interface MaskedReply extends BridgeReply {
  texts: string[];
  keys: string[];
  count: number;
}

type TripReply = BridgeReply & (
  | { action: "pass" }
  | { action: "masked"; texts: string[]; count: number; keys: string[] }
  | { action: "block"; reason: string; keys: string[] }
);

export class Pipeline {
  private readonly win: GuardWindow;
  private readonly site: SiteId;
  private readonly bridge: PageBridge;
  /** Tells the user something (toast, or a minimal banner when the content script is gone). */
  private readonly notify: (level: "info" | "warn" | "error", text: string) => void;

  constructor(
    win: GuardWindow,
    site: SiteId,
    bridge: PageBridge,
    notify?: (level: "info" | "warn" | "error", text: string) => void,
  ) {
    this.win = win;
    this.site = site;
    this.bridge = bridge;
    this.notify = notify ?? ((level, text) => void bridge.post({ type: "notify", level, text }));
  }

  // ---- cheap synchronous decisions (the wrappers use these to stay out of the way) ------

  private vaultMayHaveValues(): boolean {
    const s = this.bridge.state;
    return !s.known || s.vaultSize > 0;
  }

  private parse(url: string): URL | null {
    try {
      const u = new URL(url, this.win.location.href);
      return u.protocol === "http:" || u.protocol === "https:" ? u : null;
    } catch {
      return null;
    }
  }

  /** True when a request needs the async guard at all. */
  needsGuard(url: string, method: string, body: unknown): boolean {
    if (!this.bridge.state.enabled) return false;
    const u = this.parse(url);
    if (!u) return false;
    const m = method.toUpperCase();
    const hasBody = body !== null && body !== undefined && m !== "GET" && m !== "HEAD";
    if (hasBody && (isPromptEndpoint(this.site, u, m) || isUploadEndpoint(this.site, u, m))) return true;
    if (!this.vaultMayHaveValues()) return false;
    if (hasBody) return true;
    return u.origin !== this.win.location.origin && u.search !== "";
  }

  // ---- the guard ------------------------------------------------------------------------

  async guard(req: GuardRequest): Promise<GuardOutcome> {
    if (!this.bridge.state.enabled) return PASS;
    const url = this.parse(req.url);
    if (!url) return PASS;
    const method = req.method.toUpperCase();
    const ctx: Ctx = {
      site: this.site,
      known: isPromptEndpoint(this.site, url, method),
      upload: isUploadEndpoint(this.site, url, method),
      url,
      contentType: (req.contentType ?? "").toLowerCase(),
    };

    // Values in a cross-origin URL cannot be rewritten safely: block.
    if (url.origin !== this.win.location.origin && url.search !== "" && this.vaultMayHaveValues()) {
      const r = await this.tripwire([url.href, safeDecode(url.href)]);
      if (r?.action === "block") return this.block(r.reason, url);
      if (r?.action === "masked") return this.block("a protected value is in the request URL", url);
    }

    const hasBody = req.body !== null && req.body !== undefined && method !== "GET" && method !== "HEAD";
    if (!hasBody) return PASS;
    if (!ctx.known && !ctx.upload && !this.vaultMayHaveValues()) return PASS;
    return this.processBody(req.body, ctx);
  }

  private async processBody(body: unknown, ctx: Ctx): Promise<GuardOutcome> {
    const w = this.win;
    if (typeof body === "string") return this.processString(body, ctx, (s) => s);
    if (body instanceof w.URLSearchParams) return this.processForm(body, ctx);
    if (body instanceof w.FormData) return this.processFormData(body, ctx);
    if (body instanceof w.Blob) return this.processBlob(body, ctx);
    if (body instanceof w.ArrayBuffer || ArrayBuffer.isView(body)) {
      const bytes = body instanceof w.ArrayBuffer ? new Uint8Array(body) : new Uint8Array((body as ArrayBufferView).buffer, (body as ArrayBufferView).byteOffset, (body as ArrayBufferView).byteLength);
      if (bytes.byteLength > MAX_SCAN_BYTES) return PASS;
      let text: string;
      try {
        text = new TextDecoder("utf-8", { fatal: true }).decode(bytes);
      } catch {
        return PASS; // binary: nothing we can match textually
      }
      return this.processString(text, ctx, (s) => new TextEncoder().encode(s) as unknown as BodyInit);
    }
    return PASS; // ReadableStream, Document, ...: cannot be inspected without consuming it
  }

  // ---- text bodies ----------------------------------------------------------------------

  private async processString(text: string, ctx: Ctx, wrap: (s: string) => BodyInit): Promise<GuardOutcome> {
    if (text.length > MAX_SCAN_BYTES) return PASS;
    let cur = text;
    let count = 0;

    if (ctx.known) {
      let rewritten = false;
      try {
        const rw = await rewriteJson(this.site, cur, (texts) => this.maskFields(texts, "full"));
        if (rw) {
          rewritten = true;
          cur = rw.body;
          count += rw.count;
        }
      } catch {
        return this.failSafe(text, ctx);
      }
      if (!rewritten && !ctx.contentType.includes("json") && !looksLikeJson(text)) {
        // A prompt endpoint with a body we do not understand: mask it as plain text.
        try {
          const m = await this.maskFields([cur], "full");
          cur = m.texts[0]!;
          count += m.count;
        } catch {
          return this.failSafe(text, ctx);
        }
      }
    } else if (ctx.upload && isTextUpload(ctx)) {
      try {
        const m = await this.maskFields([cur], "full", "file");
        cur = m.texts[0]!;
        count += m.count;
      } catch {
        return this.failSafe(text, ctx);
      }
    }

    // The tripwire runs on whatever will actually be sent.
    if (this.vaultMayHaveValues() || count > 0) {
      const t = await this.tripwire([cur]);
      if (t?.action === "block") return this.block(t.reason, ctx.url);
      if (t?.action === "masked") {
        cur = t.texts[0]!;
        count += t.count;
      }
    }
    return cur === text ? PASS : { action: "replace", body: wrap(cur) };
  }

  private async processForm(params: URLSearchParams, ctx: Ctx): Promise<GuardOutcome> {
    if (!this.vaultMayHaveValues()) return PASS;
    const entries = [...params.entries()];
    const texts = entries.flatMap(([k, v]) => [k, v]);
    const t = await this.tripwire(texts);
    if (t?.action === "block") return this.block(t.reason, ctx.url);
    if (t?.action !== "masked") return PASS;
    const next = new this.win.URLSearchParams();
    for (let i = 0; i < entries.length; i++) next.append(t.texts[i * 2]!, t.texts[i * 2 + 1]!);
    return { action: "replace", body: next };
  }

  private async processFormData(fd: FormData, ctx: Ctx): Promise<GuardOutcome> {
    const w = this.win;
    const entries = [...fd.entries()] as Array<[string, string | File]>;
    const next: Array<[string, string | File]> = entries.map(([k, v]) => [k, v]);
    let changed = false;

    const strIdx = entries.flatMap(([, v], i) => (typeof v === "string" ? [i] : []));
    if (strIdx.length > 0 && this.vaultMayHaveValues()) {
      const t = await this.tripwire(strIdx.map((i) => entries[i]![1] as string));
      if (t?.action === "block") return this.block(t.reason, ctx.url);
      if (t?.action === "masked") {
        strIdx.forEach((i, j) => (next[i] = [entries[i]![0], t.texts[j]!]));
        changed = true;
      }
    }
    for (let i = 0; i < entries.length; i++) {
      const v = entries[i]![1];
      if (typeof v === "string" || !isTextBlob(v)) continue;
      const out = await this.processBlob(v, ctx);
      if (out.action === "block") return out;
      if (out.action === "replace") {
        next[i] = [entries[i]![0], out.body as unknown as File];
        changed = true;
      }
    }
    if (!changed) return PASS;
    const rebuilt = new w.FormData();
    for (const [k, v] of next) {
      if (typeof v === "string") rebuilt.append(k, v);
      else rebuilt.append(k, v, (v as File).name ?? "blob");
    }
    return { action: "replace", body: rebuilt };
  }

  private async processBlob(blob: Blob, ctx: Ctx): Promise<GuardOutcome> {
    if (blob.size > MAX_SCAN_BYTES || !isTextBlob(blob)) return PASS;
    let text: string;
    try {
      // Strict decoding: a binary file with no type must never be rewritten through a lossy decode.
      text = new TextDecoder("utf-8", { fatal: true }).decode(await blob.arrayBuffer());
    } catch {
      return PASS;
    }
    const w = this.win;
    const file = blob as File;
    return this.processString(text, ctx, (s) =>
      typeof file.name === "string"
        ? new w.File([s], file.name, { type: blob.type, lastModified: file.lastModified })
        : new w.Blob([s], { type: blob.type }),
    );
  }

  // ---- bridge calls ---------------------------------------------------------------------

  private async maskFields(texts: string[], mode: "full" | "known", source?: string): Promise<MaskOutcome> {
    const r = await this.bridge.request<MaskedReply>({ type: "maskMany", texts, mode, source });
    if (r.keys.length > 0 || r.count > 0) this.bridge.state.vaultSize = Math.max(this.bridge.state.vaultSize, 1);
    return { texts: r.texts, keys: r.keys, count: r.count };
  }

  /** The tripwire; null when the engine cannot be reached (the caller then lets the request through). */
  private async tripwire(texts: string[]): Promise<TripReply | null> {
    try {
      return await this.bridge.request<TripReply>({ type: "tripwire", texts });
    } catch (e) {
      if (e instanceof BridgeUnavailable) return null;
      throw e;
    }
  }

  // ---- outcomes -------------------------------------------------------------------------

  private block(reason: string, url: URL): GuardOutcome {
    const where = url.hostname === this.win.location.hostname ? "this site" : url.hostname;
    this.notify("error", `Zuko blocked a request to ${where}: ${reason}.`);
    return { action: "block", reason };
  }

  /** Engine unreachable: block only what is obviously a live credential. */
  private failSafe(raw: string, ctx: Ctx): GuardOutcome {
    const texts: string[] = [];
    try {
      const body = JSON.parse(raw);
      for (const f of rewriterFor(this.site).fields(body)) texts.push(f.text);
    } catch {
      texts.push(raw);
    }
    for (const t of texts) {
      const label = findHighConfidenceSecret(t);
      if (label) {
        this.bridge.post({ type: "event", kind: "blocked", count: 1, keys: [] });
        return this.block(
          `the Zuko engine is not available and this message contains what looks like a ${label}. Nothing was sent. Reload the extension or the page and try again`,
          ctx.url,
        );
      }
    }
    return PASS;
  }
}

function safeDecode(s: string): string {
  try {
    return decodeURIComponent(s.replace(/\+/g, " "));
  } catch {
    return s;
  }
}

function looksLikeJson(s: string): boolean {
  const t = s.trimStart();
  return t.startsWith("{") || t.startsWith("[");
}

function isTextBlob(b: Blob): boolean {
  const name = (b as File).name;
  return isTextLikeType(b.type) || (typeof name === "string" && isTextLikeName(name)) || (b.type === "" && b.size <= 2 * 1024 * 1024);
}

function isTextUpload(ctx: Ctx): boolean {
  return ctx.contentType === "" || isTextLikeType(ctx.contentType) || ctx.contentType.includes("octet-stream");
}
