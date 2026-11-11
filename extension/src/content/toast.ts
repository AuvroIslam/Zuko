// Toasts and the composer chip, drawn inside a closed shadow root so the site's CSS cannot
// touch them and ours cannot touch the site. Styles use a constructable stylesheet (allowed
// under strict page CSPs, unlike inline <style>), with a <style> fallback where it is missing.

import { mascotSvg } from "../shared/mascot.ts";

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
  /** Flicker a tiny flame by the avatar. Defaults to on for warn/error and for "masked" notes. */
  flame?: boolean;
}

const CSS = `
:host { all: initial; }
.layer { position: fixed; z-index: 2147483647; right: 16px; bottom: 16px; display: flex; flex-direction: column; gap: 8px; align-items: flex-end; pointer-events: none; font: 13px/1.4 system-ui, -apple-system, "Segoe UI", sans-serif; }
.toast { pointer-events: auto; max-width: 380px; color: #f6ece2; background: #1a100d; border: 1px solid #4a2d25; border-left: 4px solid #ffb347; border-radius: 10px; padding: 10px 12px; box-shadow: 0 8px 28px rgba(0,0,0,.45); display: flex; gap: 10px; align-items: flex-start; animation: in .16s ease-out; }
.toast.warn { border-left-color: #e0a030; }
.toast.error { border-left-color: #e5503f; }
.toast .msg { flex: 1; white-space: pre-wrap; word-break: break-word; }
.toast .brand { font-weight: 700; color: #ffb347; margin-right: 6px; }
.toast.warn .brand { color: #e0a030; }
.toast.error .brand { color: #e5503f; }
.toast button { all: unset; cursor: pointer; color: #ffb347; font-weight: 600; padding: 2px 6px; border-radius: 6px; }
.toast button:hover { background: rgba(255,179,71,.16); }
.toast .x { color: #b59a88; font-weight: 400; }
.chip { position: fixed; pointer-events: auto; display: none; align-items: center; gap: 6px; color: #f6ece2; background: #1a100d; border: 1px solid #4a2d25; border-radius: 999px; padding: 3px 10px 3px 8px; font: 12px/1.4 system-ui, -apple-system, "Segoe UI", sans-serif; box-shadow: 0 4px 16px rgba(0,0,0,.35); cursor: default; }
.chip.on { display: inline-flex; }
.chip .dot { width: 8px; height: 8px; border-radius: 50%; background: #ffb347; box-shadow: 0 0 8px #ffb347; }
.chip.warn .dot { background: #e0a030; box-shadow: 0 0 8px #e0a030; }
.switch { position: fixed; right: 18px; bottom: 120px; pointer-events: auto; display: none; align-items: center; gap: 2px; padding: 3px; background: #1a100d; border: 1px solid #4a2d25; border-radius: 999px; box-shadow: 0 6px 22px rgba(0,0,0,.4); font: 600 12px/1.2 system-ui, -apple-system, "Segoe UI", sans-serif; }
.switch.on { display: inline-flex; }
.switch .tag { color: #ffb347; padding: 0 6px 0 8px; }
.switch button { all: unset; cursor: pointer; color: #b59a88; padding: 5px 10px; border-radius: 999px; }
.switch button:hover { color: #f6ece2; }
.switch button.sel { color: #1a100d; background: #2ee6c5; }
.switch button.sel.ai { background: #ffb347; }
@keyframes in { from { opacity: 0; transform: translateY(6px); } to { opacity: 1; transform: none; } }
.avatar { position: relative; flex: none; width: 26px; height: 26px; margin-top: -1px; }
.avatar svg { display: block; width: 26px; height: 26px; }
.flame { position: absolute; right: -5px; top: -7px; width: 9px; height: 13px; background: radial-gradient(ellipse at 50% 75%, #fff3cf 0 18%, #ffb347 40%, #e5503f 85%); transform-origin: 50% 100%; animation: flick .9s ease-in-out infinite alternate; clip-path: polygon(50% 0, 78% 38%, 100% 70%, 82% 100%, 18% 100%, 0 70%, 24% 36%); }
@keyframes flick { 0% { transform: scale(1, 1) rotate(-4deg); opacity: .9; } 50% { transform: scale(.88, 1.15) rotate(3deg); opacity: 1; } 100% { transform: scale(1.05, .92) rotate(-2deg); opacity: .85; } }
@media (prefers-reduced-motion: reduce) { .toast, .flame { animation: none; } }
`;

export class Overlay {
  private readonly doc: Document;
  private readonly host: HTMLElement;
  private readonly layer: HTMLElement;
  private readonly chipEl: HTMLElement;
  private readonly chipText: HTMLElement;
  private mounted = false;
  private readonly switchEl: HTMLElement;
  private readonly yoursBtn: HTMLButtonElement;
  private readonly aiBtn: HTMLButtonElement;
  private pick: (revealed: boolean) => void = () => {};

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
    // "Your view | What ChatGPT got": the page in real values, or exactly as the AI has it.
    this.switchEl = doc.createElement("div");
    this.switchEl.className = "switch";
    const tag = doc.createElement("span");
    tag.className = "tag";
    tag.textContent = "Zuko";
    this.yoursBtn = doc.createElement("button");
    this.yoursBtn.type = "button";
    this.yoursBtn.textContent = "Your view";
    this.yoursBtn.title = "Real values, restored on this computer only";
    this.yoursBtn.addEventListener("click", () => this.pick(true));
    this.aiBtn = doc.createElement("button");
    this.aiBtn.type = "button";
    this.aiBtn.addEventListener("click", () => this.pick(false));
    this.switchEl.append(tag, this.yoursBtn, this.aiBtn);
    root.append(this.layer, this.chipEl, this.switchEl);
  }

  /**
   * The view switch: shown once something on the page was restored. `revealed` is the
   * current view (true = real values); `pick` is called with the one the user chose.
   */
  setSwitch(opts: { visible: boolean; revealed: boolean; aiName: string; pick: (revealed: boolean) => void }): void {
    this.mount();
    this.pick = opts.pick;
    this.aiBtn.textContent = `What ${opts.aiName} got`;
    this.aiBtn.title = `Your messages and its replies exactly as ${opts.aiName} has them: placeholders, never your values (Alt+R)`;
    this.switchEl.classList.toggle("on", opts.visible);
    this.yoursBtn.classList.toggle("sel", opts.revealed);
    this.aiBtn.classList.toggle("sel", !opts.revealed);
    this.aiBtn.classList.toggle("ai", !opts.revealed);
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
    const avatar = this.doc.createElement("div");
    avatar.className = "avatar";
    avatar.appendChild(mascotSvg(this.doc));
    if (opts.flame ?? (level !== "info" || /mask/i.test(opts.text))) {
      const flame = this.doc.createElement("span");
      flame.className = "flame";
      avatar.appendChild(flame);
    }
    el.appendChild(avatar);
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
