// Toasts and the composer chip, drawn inside a closed shadow root so the site's CSS cannot
// touch them and ours cannot touch the site. Styles use a constructable stylesheet (allowed
// under strict page CSPs, unlike inline <style>), with a <style> fallback where it is missing.

export type ToastLevel = "info" | "warn" | "error";

export interface ToastAction {
  label: string;
  run: () => void;
}

export interface ToastOptions {
  level?: ToastLevel;
  text: string;
  actions?: ToastAction[];
  /** Milliseconds; 0 keeps it until dismissed. */
  ttl?: number;
}

const CSS = `
:host { all: initial; }
.layer { position: fixed; z-index: 2147483647; right: 16px; bottom: 16px; display: flex; flex-direction: column; gap: 8px; align-items: flex-end; pointer-events: none; font: 13px/1.4 system-ui, -apple-system, "Segoe UI", sans-serif; }
.toast { pointer-events: auto; max-width: 380px; color: #e9edf8; background: #0f1530; border: 1px solid #26335f; border-left: 4px solid #2ee6c5; border-radius: 10px; padding: 10px 12px; box-shadow: 0 8px 28px rgba(0,0,0,.45); display: flex; gap: 10px; align-items: flex-start; animation: in .16s ease-out; }
.toast.warn { border-left-color: #ffb13d; }
.toast.error { border-left-color: #ff5a4d; }
.toast .msg { flex: 1; white-space: pre-wrap; word-break: break-word; }
.toast .brand { font-weight: 700; color: #2ee6c5; margin-right: 6px; }
.toast.warn .brand { color: #ffb13d; }
.toast.error .brand { color: #ff5a4d; }
.toast button { all: unset; cursor: pointer; color: #2ee6c5; font-weight: 600; padding: 2px 6px; border-radius: 6px; }
.toast button:hover { background: rgba(46,230,197,.14); }
.toast .x { color: #8fa0d0; font-weight: 400; }
.chip { position: fixed; pointer-events: auto; display: none; align-items: center; gap: 6px; color: #e9edf8; background: #0f1530; border: 1px solid #26335f; border-radius: 999px; padding: 3px 10px 3px 8px; font: 12px/1.4 system-ui, -apple-system, "Segoe UI", sans-serif; box-shadow: 0 4px 16px rgba(0,0,0,.35); cursor: default; }
.chip.on { display: inline-flex; }
.chip .dot { width: 8px; height: 8px; border-radius: 50%; background: #2ee6c5; box-shadow: 0 0 8px #2ee6c5; }
.chip.warn .dot { background: #ffb13d; box-shadow: 0 0 8px #ffb13d; }
@keyframes in { from { opacity: 0; transform: translateY(6px); } to { opacity: 1; transform: none; } }
@media (prefers-reduced-motion: reduce) { .toast { animation: none; } }
`;

export class Overlay {
  private readonly doc: Document;
  private readonly host: HTMLElement;
  private readonly layer: HTMLElement;
  private readonly chipEl: HTMLElement;
  private readonly chipText: HTMLElement;
  private mounted = false;

  constructor(doc: Document) {
    this.doc = doc;
    this.host = doc.createElement("zuko-overlay");
    const root = this.host.attachShadow({ mode: "closed" });
    const win = doc.defaultView as (Window & typeof globalThis) | null;
    let styled = false;
    try {
      if (win && "adoptedStyleSheets" in root && typeof win.CSSStyleSheet === "function") {
        const sheet = new win.CSSStyleSheet();
        sheet.replaceSync(CSS);
        (root as ShadowRoot).adoptedStyleSheets = [sheet];
        styled = true;
      }
    } catch {
      styled = false;
    }
    if (!styled) {
      const style = doc.createElement("style");
      style.textContent = CSS;
      root.appendChild(style);
    }
    this.layer = doc.createElement("div");
    this.layer.className = "layer";
    this.layer.setAttribute("role", "status");
    this.layer.setAttribute("aria-live", "polite");
    this.chipEl = doc.createElement("div");
    this.chipEl.className = "chip";
    const dot = doc.createElement("span");
    dot.className = "dot";
    this.chipText = doc.createElement("span");
    this.chipEl.append(dot, this.chipText);
    root.append(this.layer, this.chipEl);
  }

  private mount(): void {
    if (this.mounted && this.host.isConnected) return;
    (this.doc.documentElement ?? this.doc.body).appendChild(this.host);
    this.mounted = true;
  }

  show(opts: ToastOptions): () => void {
    this.mount();
    const level = opts.level ?? "info";
    const el = this.doc.createElement("div");
    el.className = `toast ${level}`;
    const msg = this.doc.createElement("div");
    msg.className = "msg";
    const brand = this.doc.createElement("span");
    brand.className = "brand";
    brand.textContent = "Zuko";
    msg.append(brand, this.doc.createTextNode(opts.text));
    el.appendChild(msg);
    const close = () => el.remove();
    for (const a of opts.actions ?? []) {
      const b = this.doc.createElement("button");
      b.type = "button";
      b.textContent = a.label;
      b.addEventListener("click", () => {
        a.run();
        close();
      });
      el.appendChild(b);
    }
    const x = this.doc.createElement("button");
    x.type = "button";
    x.className = "x";
    x.textContent = "×";
    x.setAttribute("aria-label", "Dismiss");
    x.addEventListener("click", close);
    el.appendChild(x);
    this.layer.appendChild(el);
    while (this.layer.children.length > 4) this.layer.firstElementChild?.remove();
    const ttl = opts.ttl ?? (level === "error" ? 14000 : 6000);
    if (ttl > 0) this.doc.defaultView?.setTimeout(close, ttl);
    return close;
  }

  /** The composer chip: text, level and anchor rectangle; `null` text hides it. */
  chip(text: string | null, level: "info" | "warn" = "info", anchor?: DOMRect | null, title?: string): void {
    if (text === null) {
      this.chipEl.classList.remove("on");
      return;
    }
    this.mount();
    this.chipText.textContent = text;
    this.chipEl.title = title ?? "";
    this.chipEl.classList.toggle("warn", level === "warn");
    this.chipEl.classList.add("on");
    if (anchor) {
      const win = this.doc.defaultView!;
      const w = this.chipEl.offsetWidth || 180;
      const left = Math.max(8, Math.min(win.innerWidth - w - 8, anchor.right - w));
      const top = Math.max(8, anchor.top - 30);
      this.chipEl.style.left = `${left}px`;
      this.chipEl.style.top = `${top}px`;
    }
  }
}
