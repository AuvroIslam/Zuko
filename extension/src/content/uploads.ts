// Upload interception, in the isolated world on `window` in the capture phase, so it sees file
// choices, drops and pastes before the site does:
//
//   input[type=file] "change"  -> stop the event, sanitize, put the clean files back with
//                                 DataTransfer, re-dispatch "change" (flagged so we skip it)
//   "drop" / "paste" with files -> same, with a fresh DragEvent / ClipboardEvent
//
// Text-like files (.txt .md .json .csv, code ...) are masked in place. PDFs become
// `name.zuko.md` (text from pdf.js in the offscreen document, then masked); a PDF with no
// text layer is BLOCKED, because it cannot be scanned. Everything else (images, office
// files) passes untouched; the network layer still checks text-like bodies as a last line.

import { buildNote } from "../shared/placeholders.ts";
import { MAX_SCAN_BYTES, isPdf, isScannableText } from "../shared/files.ts";

export interface MaskResult {
  text: string;
  count: number;
  keys: string[];
}

export interface PdfResult {
  ok: boolean;
  error?: string;
  /** True when there is no text layer: the upload must not go through. */
  blocked?: boolean;
  markdown?: string;
  pages?: number;
  warnings?: string[];
  count?: number;
  keys?: string[];
}

export interface UploadDeps {
  /** Full detection + vault masking of a file's text (source "file"). */
  maskText(text: string): Promise<MaskResult>;
  sanitizePdf(base64: string, name: string): Promise<PdfResult>;
  notify(level: "info" | "warn" | "error", text: string): void;
  /** False while Zuko is switched off for this site. */
  enabled?: () => boolean;
  /** Counters / app activity. */
  report(count: number, keys: string[]): void;
  /** File constructor of the page's realm (tests pass Node's). */
  File?: typeof File;
}

export interface Sanitized {
  /** The file to upload in place of the original; null = remove it (blocked). */
  file: File | null;
  count: number;
  keys: string[];
  changed: boolean;
  /** Human message for a toast (empty when nothing to say). */
  message: string;
  level: "info" | "warn" | "error";
}

const OFFICE = /\.(docx?|xlsx?|pptx?|odt|ods|odp|rtf|zip|7z|rar|tar|gz)$/i;

function stem(name: string): string {
  return name.replace(/\.[^.\\/]+$/, "");
}

function toBase64(buf: ArrayBuffer): string {
  const bytes = new Uint8Array(buf);
  let bin = "";
  for (let i = 0; i < bytes.length; i += 0x8000) bin += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  return btoa(bin);
}

function plural(n: number, one: string, many: string): string {
  return `${n} ${n === 1 ? one : many}`;
}

/** The kinds of file Zuko will look inside. */
export function needsSanitizing(file: { name: string; type: string }): boolean {
  return isPdf(file) || isScannableText(file);
}

const isMarkdownish = (name: string) => /\.(txt|md|markdown)$/i.test(name) || !/\.[^.\\/]+$/.test(name);

export async function sanitizeFile(file: File, deps: UploadDeps): Promise<Sanitized> {
  const Ctor = deps.File ?? File;
  const pass = (message = "", level: Sanitized["level"] = "info"): Sanitized => ({ file, count: 0, keys: [], changed: false, message, level });

  if (isPdf(file)) {
    if (file.size > 64 * 1024 * 1024) return pass(`${file.name} is too large for Zuko to scan (64 MB limit). It was not checked.`, "warn");
    const r = await deps.sanitizePdf(toBase64(await file.arrayBuffer()), file.name);
    if (!r.ok) {
      return { file: null, count: 0, keys: [], changed: true, level: "error", message: `Zuko could not read ${file.name} (${r.error ?? "unknown error"}), so it was not uploaded.` };
    }
    if (r.blocked) {
      return {
        file: null,
        count: 0,
        keys: [],
        changed: true,
        level: "error",
        message: `${file.name} has no text layer (a scanned PDF?). Zuko cannot check it for secrets, so it was not uploaded.`,
      };
    }
    const count = r.count ?? 0;
    const keys = r.keys ?? [];
    let md = r.markdown ?? "";
    if (count > 0 && keys.length > 0) md = `${buildNote(keys)}\n\n${md}`;
    const out = new Ctor([md], `${stem(file.name)}.zuko.md`, { type: "text/markdown", lastModified: file.lastModified });
    const warn = (r.warnings ?? []).join(" ");
    return {
      file: out,
      count,
      keys,
      changed: true,
      level: warn ? "warn" : "info",
      message:
        `${file.name} was replaced by a text-only copy (${out.name}, ${plural(r.pages ?? 0, "page", "pages")}). ` +
        (count > 0 ? `${plural(count, "item", "items")} masked.` : "Nothing sensitive found.") +
        (warn ? ` ${warn}` : ""),
    };
  }

  if (isScannableText(file)) {
    if (file.size > MAX_SCAN_BYTES) return pass(`${file.name} is too large for Zuko to scan (16 MB limit). It was not checked.`, "warn");
    let text: string;
    try {
      text = new TextDecoder("utf-8", { fatal: true }).decode(await file.arrayBuffer());
    } catch {
      return pass(`${file.name} is not UTF-8 text, so Zuko could not check it.`, "warn");
    }
    const r = await deps.maskText(text);
    if (r.count === 0 || r.text === text) return pass();
    const body = isMarkdownish(file.name) && r.keys.length > 0 ? `${buildNote(r.keys)}\n\n${r.text}` : r.text;
    const out = new Ctor([body], file.name, { type: file.type || "text/plain", lastModified: file.lastModified });
    return {
      file: out,
      count: r.count,
      keys: r.keys,
      changed: true,
      level: "info",
      message: `${file.name}: ${plural(r.count, "item", "items")} masked before upload.`,
    };
  }

  if (OFFICE.test(file.name)) return pass(`Zuko cannot look inside ${file.name}. Check it yourself before sending.`, "warn");
  return pass();
}

// ---------------------------------------------------------------------------------------
// DOM wiring

export class UploadGuard {
  private readonly win: Window & typeof globalThis;
  private readonly deps: UploadDeps;
  private bypass = 0;
  private started = false;

  constructor(win: Window & typeof globalThis, deps: UploadDeps) {
    this.win = win;
    this.deps = deps;
  }

  start(): void {
    if (this.started) return;
    this.started = true;
    // Capture on window, registered at document_start: ahead of every page listener.
    this.win.addEventListener("change", (e) => this.onChange(e), true);
    this.win.addEventListener("drop", (e) => this.onDrop(e as DragEvent), true);
    this.win.addEventListener("paste", (e) => this.onPaste(e as ClipboardEvent), true);
  }

  /** Sanitizes a list of files and reports to the user. Order is kept; blocked files drop out. */
  async process(files: File[]): Promise<File[]> {
    const names = files.filter(needsSanitizing).map((f) => f.name);
    if (names.length > 0) this.deps.notify("info", `Checking ${names.length === 1 ? names[0] : names.length + " files"} for private data...`);
    const out: File[] = [];
    let totalCount = 0;
    const keys = new Set<string>();
    for (const f of files) {
      let s: Sanitized;
      try {
        s = await sanitizeFile(f, this.deps);
      } catch (e) {
        // Unknown failure while scanning: never let an unchecked text file through silently.
        s = {
          file: needsSanitizing(f) ? null : f,
          count: 0,
          keys: [],
          changed: true,
          level: "error",
          message: `Zuko could not check ${f.name} (${e instanceof Error ? e.message : String(e)}), so it was not uploaded.`,
        };
      }
      if (s.file) out.push(s.file);
      totalCount += s.count;
      s.keys.forEach((k) => keys.add(k));
      if (s.message) this.deps.notify(s.level, s.message);
    }
    if (totalCount > 0 || files.some(needsSanitizing)) this.deps.report(totalCount, [...keys]);
    return out;
  }

  /** Files Zuko cannot look inside: say so, but do not get in the way. */
  private warnOpaque(files: File[]): void {
    for (const f of files) if (OFFICE.test(f.name)) this.deps.notify("warn", `Zuko cannot look inside ${f.name}. Check it yourself before sending.`);
  }

  private toTransfer(files: File[]): DataTransfer {
    const dt = new this.win.DataTransfer();
    for (const f of files) dt.items.add(f);
    return dt;
  }

  private off(): boolean {
    return this.deps.enabled ? !this.deps.enabled() : false;
  }

  private onChange(e: Event): void {
    if (this.off()) return;
    const input = e.target as HTMLInputElement | null;
    if (this.bypass > 0 || !input || input.tagName !== "INPUT" || input.type !== "file" || !input.files?.length) return;
    const files = [...input.files];
    if (!files.some(needsSanitizing)) return this.warnOpaque(files);
    e.stopImmediatePropagation();
    void this.process(files).then((clean) => {
      try {
        input.files = this.toTransfer(clean).files;
      } catch {
        // Cannot replace the selection: fail closed for text-like files.
        input.value = "";
        this.deps.notify("error", "Zuko could not swap the checked file in, so the selection was cleared. Please choose the file again.");
        return;
      }
      this.bypass++;
      try {
        input.dispatchEvent(new this.win.Event("change", { bubbles: true }));
      } finally {
        this.bypass--;
      }
    });
  }

  private onDrop(e: DragEvent): void {
    if (this.off()) return;
    const dt = e.dataTransfer;
    if (this.bypass > 0 || !dt || !dt.files || dt.files.length === 0) return;
    const files = [...dt.files];
    if (!files.some(needsSanitizing)) return this.warnOpaque(files);
    e.preventDefault();
    e.stopImmediatePropagation();
    const target = e.target;
    void this.process(files).then((clean) => {
      if (!(target instanceof this.win.EventTarget)) return;
      this.bypass++;
      try {
        target.dispatchEvent(
          new this.win.DragEvent("drop", {
            bubbles: true,
            cancelable: true,
            dataTransfer: this.toTransfer(clean),
            clientX: e.clientX,
            clientY: e.clientY,
          }),
        );
      } finally {
        this.bypass--;
      }
    });
  }

  private onPaste(e: ClipboardEvent): void {
    if (this.off()) return;
    const dt = e.clipboardData;
    if (this.bypass > 0 || !dt || !dt.files || dt.files.length === 0) return;
    const files = [...dt.files];
    if (!files.some(needsSanitizing)) return this.warnOpaque(files);
    e.preventDefault();
    e.stopImmediatePropagation();
    const target = e.target;
    void this.process(files).then((clean) => {
      if (!(target instanceof this.win.EventTarget)) return;
      this.bypass++;
      try {
        target.dispatchEvent(
          new this.win.ClipboardEvent("paste", { bubbles: true, cancelable: true, clipboardData: this.toTransfer(clean) }),
        );
      } finally {
        this.bypass--;
      }
    });
  }
}
