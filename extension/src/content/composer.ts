// The composer chip: "Zuko: 2 items will be masked", shown while the user types. Advisory
// only (it never edits the box): the actual masking happens in the network layer when the
// message is sent, so it also covers edits, regenerate and anything typed elsewhere.

import { composerText, findComposer } from "./sites.ts";
import type { SiteId } from "../shared/sites.ts";
import type { Overlay } from "./toast.ts";

export interface ScanReply {
  ok: boolean;
  count?: number;
  items?: Array<{ kind: string; label: string }>;
  error?: string;
}

export class ComposerChip {
  private readonly doc: Document;
  private readonly site: SiteId;
  private readonly overlay: Overlay;
  private readonly scan: (text: string) => Promise<ScanReply>;
  private composer: HTMLElement | null = null;
  private timer: ReturnType<typeof setTimeout> | null = null;
  private seq = 0;
  private enabled = true;
  private offlineShown = false;

  constructor(doc: Document, site: SiteId, overlay: Overlay, scan: (text: string) => Promise<ScanReply>) {
    this.doc = doc;
    this.site = site;
    this.overlay = overlay;
    this.scan = scan;
  }

  start(): void {
    const win = this.doc.defaultView!;
    this.doc.addEventListener("input", (e) => this.onInput(e), true);
    const reposition = () => this.composer && this.composer.isConnected && this.schedule(60);
    win.addEventListener("resize", reposition, { passive: true });
    win.addEventListener("scroll", reposition, { passive: true, capture: true });
  }

  setEnabled(on: boolean): void {
    this.enabled = on;
    if (!on) this.overlay.chip(null);
    else this.schedule(0);
  }

  private onInput(e: Event): void {
    if (!this.enabled) return;
    const c = findComposer(this.doc, this.site, e.target);
    if (!c) return;
    this.composer = c;
    this.schedule(350);
  }

  private schedule(ms: number): void {
    if (this.timer) clearTimeout(this.timer);
    this.timer = setTimeout(() => void this.update(), ms);
  }

  async update(): Promise<void> {
    this.timer = null;
    const c = this.composer;
    if (!this.enabled || !c || !c.isConnected) {
      this.overlay.chip(null);
      return;
    }
    const text = composerText(c);
    if (text.trim().length < 6 || text.length > 400_000) {
      this.overlay.chip(null);
      return;
    }
    const mine = ++this.seq;
    const r = await this.scan(text);
    if (mine !== this.seq) return; // a newer keystroke superseded this scan
    const rect = c.getBoundingClientRect();
    if (!r.ok) {
      // Engine offline: say so once per page, not on every keystroke.
      if (!this.offlineShown) {
        this.offlineShown = true;
        this.overlay.chip("Zuko engine offline: secrets will be blocked", "warn", rect);
      }
      return;
    }
    this.offlineShown = false;
    const n = r.count ?? 0;
    if (n === 0) {
      this.overlay.chip(null);
      return;
    }
    const kinds = [...new Set((r.items ?? []).map((i) => i.label))].slice(0, 5).join(", ");
    this.overlay.chip(`Zuko: ${n} ${n === 1 ? "item" : "items"} will be masked`, "info", rect, kinds);
  }
}
