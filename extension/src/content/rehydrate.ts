// DOM rehydration: placeholders in the displayed chat become the real values again, on the
// user's screen only. The site's own state keeps holding placeholders, so nothing it sends
// back later can carry a real value (and the tripwire covers the rest).
//
// Rules that keep this safe on React pages (the Google-Translate lesson):
//  * ONLY `Text.nodeValue` is ever assigned. No element is inserted, removed or moved.
//  * A placeholder split across several text nodes (syntax-highlighted code puts `{{`,
//    `API_KEY_1` and `}}` in separate spans) is restored by putting the value in the first
//    node and emptying the covered slices of the others, again via nodeValue.
//  * Restored values are marked with the CSS Custom Highlight API (no wrapper elements).
//  * Alt+R flips every restored node back to the masked text and forth.
//  * Matching is tolerant (dropped brackets, lower case, space for `_`) but only for keys the
//    vault knows (see shared/placeholders.ts).

import { NOTE_PREFIX, VariantMatcher } from "../shared/placeholders.ts";
import type { SiteId } from "../shared/sites.ts";
import { isEditable, rehydrationRoots } from "./sites.ts";

/** Where values come from. The real one asks the service worker; tests pass a map. */
export interface ValueSource {
  /** Keys that exist (no values). */
  keys(): Set<string>;
  /** Cached value, if already loaded. */
  value(key: string): string | undefined;
  /** Loads values for keys into the cache. */
  load(keys: string[]): Promise<void>;
}

export interface RehydratorOptions {
  doc: Document;
  site: SiteId;
  source: ValueSource;
  /** Called with the number of values restored by a batch. */
  onRestored?: (count: number, keys: string[]) => void;
  onToggle?: (revealed: boolean) => void;
  /** Debounce for mutation bursts (ms). */
  delayMs?: number;
  /** How long text must sit unchanged before a placeholder at its very end is restored (ms). */
  settleMs?: number;
}

interface NodeRecord {
  /** The text as the page wrote it (placeholders). */
  masked: string;
  /** The text with real values. */
  restored: string;
  /** Where restored values sit inside `restored`. */
  spans: Array<[number, number]>;
  ranges: Range[];
}

interface NodeEdit {
  from: number;
  to: number;
  value: string;
  key?: string;
}

const BLOCK_TAGS = new Set([
  "ADDRESS", "ARTICLE", "ASIDE", "BLOCKQUOTE", "BODY", "DD", "DETAILS", "DIV", "DL", "DT", "FIELDSET", "FIGCAPTION", "FIGURE",
  "FOOTER", "FORM", "H1", "H2", "H3", "H4", "H5", "H6", "HEADER", "LI", "MAIN", "NAV", "OL", "P", "PRE", "SECTION", "TABLE",
  "TBODY", "TD", "TH", "THEAD", "TR", "UL",
]);
const SKIP_TAGS = new Set(["SCRIPT", "STYLE", "NOSCRIPT", "TEXTAREA", "INPUT", "SELECT", "OPTION", "TEMPLATE", "ZUKO-OVERLAY"]);

const NOTE_RE = /^\s*\[Zuko privacy note:[^\]]*\]\s*/;
const SHOW_TEXT = 4;
const HIGHLIGHT_NAME = "zuko-restored";

export class Rehydrator {
  private readonly doc: Document;
  private readonly win: Window & typeof globalThis;
  private readonly site: SiteId;
  private readonly source: ValueSource;
  private readonly onRestored: RehydratorOptions["onRestored"];
  private readonly onToggle: RehydratorOptions["onToggle"];
  private readonly delayMs: number;
  private readonly settleMs: number;

  private matcher: VariantMatcher;
  private records = new WeakMap<Text, NodeRecord>();
  private tracked = new Set<WeakRef<Text>>();
  private dirty = new Set<Node>();
  private timer: ReturnType<typeof setTimeout> | null = null;
  /** Blocks whose text ends in a placeholder that may still be growing (streaming). */
  private deferred = new Set<Element>();
  private settleTimer: ReturnType<typeof setTimeout> | null = null;
  private observer: MutationObserver | null = null;
  private highlight: any = null;
  private revealed = true;
  private running = false;

  constructor(opts: RehydratorOptions) {
    this.doc = opts.doc;
    this.win = opts.doc.defaultView as Window & typeof globalThis;
    this.site = opts.site;
    this.source = opts.source;
    this.onRestored = opts.onRestored;
    this.onToggle = opts.onToggle;
    this.delayMs = opts.delayMs ?? 60;
    this.settleMs = opts.settleMs ?? 800;
    this.matcher = new VariantMatcher(this.source.keys());
  }

  get isRevealed(): boolean {
    return this.revealed;
  }

  /** The matcher the copy handler shares. */
  get variantMatcher(): VariantMatcher {
    return this.matcher;
  }

  /** For tests. */
  get highlightedRangeCount(): number {
    return this.highlight ? this.highlight.size : 0;
  }

  start(): void {
    if (this.running) return;
    this.running = true;
    this.setupHighlight();
    const body = this.doc.body ?? this.doc.documentElement;
    this.observer = new this.win.MutationObserver((records) => {
      for (const r of records) {
        if (r.type === "characterData") this.dirty.add(r.target);
        else r.addedNodes.forEach((n) => this.dirty.add(n));
      }
      this.schedule();
    });
    this.observer.observe(body, { childList: true, subtree: true, characterData: true });
    this.dirty.add(body);
    this.schedule();
  }

  stop(): void {
    this.running = false;
    this.observer?.disconnect();
    this.observer = null;
    if (this.timer) clearTimeout(this.timer);
    if (this.settleTimer) clearTimeout(this.settleTimer);
    this.timer = null;
    this.settleTimer = null;
    this.deferred.clear();
    this.revert();
  }

  /** The vault changed: rebuild the matcher and look at everything again. */
  refresh(): void {
    this.matcher = new VariantMatcher(this.source.keys());
    if (!this.running) return;
    this.dirty.add(this.doc.body ?? this.doc.documentElement);
    this.schedule();
  }

  /** Alt+R: masked text <-> real values. Returns the new state (true = real values shown). */
  toggle(): boolean {
    this.revealed = !this.revealed;
    for (const ref of [...this.tracked]) {
      const node = ref.deref();
      const rec = node ? this.records.get(node) : undefined;
      if (!node || !rec || !node.isConnected) {
        this.tracked.delete(ref);
        continue;
      }
      node.nodeValue = this.revealed ? rec.restored : rec.masked;
    }
    if (this.revealed) this.paintAll();
    else this.highlight?.clear();
    this.onToggle?.(this.revealed);
    return this.revealed;
  }

  /** Processes everything pending right now (tests and the first paint). */
  async flushNow(opts: { settle?: boolean } = {}): Promise<void> {
    if (this.timer) clearTimeout(this.timer);
    this.timer = null;
    await this.flush();
    if (opts.settle) await this.settle();
  }

  /** Restores placeholders that sat at the very end of text which has since stopped changing. */
  private async settle(): Promise<void> {
    if (this.settleTimer) clearTimeout(this.settleTimer);
    this.settleTimer = null;
    if (!this.revealed || this.deferred.size === 0) return;
    const blocks = [...this.deferred];
    this.deferred.clear();
    const skipCache = new Map<Element, boolean>();
    let restored = 0;
    const keys = new Set<string>();
    const need = new Set<string>();
    for (const b of blocks) {
      if (!b.isConnected) continue;
      const r = this.processBlock(b, need, skipCache, true);
      restored += r.count;
      r.keys.forEach((k) => keys.add(k));
    }
    if (need.size > 0) {
      await this.source.load([...need]).catch(() => undefined);
      for (const b of blocks) {
        if (!b.isConnected) continue;
        const r = this.processBlock(b, new Set(), skipCache, true);
        restored += r.count;
        r.keys.forEach((k) => keys.add(k));
      }
    }
    if (restored > 0) this.onRestored?.(restored, [...keys]);
  }

  // ---- scheduling -----------------------------------------------------------------------

  private schedule(): void {
    if (this.timer || !this.running) return;
    this.timer = setTimeout(() => {
      this.timer = null;
      void this.flush();
    }, this.delayMs);
  }

  private async flush(): Promise<void> {
    if (!this.revealed) {
      this.dirty.clear();
      return;
    }
    const dirty = [...this.dirty];
    this.dirty.clear();
    const blocks = new Set<Element>();
    for (const n of dirty) {
      if (!n.isConnected) continue;
      if (n.nodeType === 3) {
        const parent = n.parentElement;
        if (!parent || rehydrationRoots(this.doc, this.site, parent).length === 0) continue;
        const b = this.blockOf(n);
        if (b) blocks.add(b);
      } else if (n.nodeType === 1) {
        for (const root of rehydrationRoots(this.doc, this.site, n as Element)) this.blocksUnder(root, blocks);
      }
    }

    const skipCache = new Map<Element, boolean>();
    const need = new Set<string>();
    const retry: Element[] = [];
    let restored = 0;
    const keys = new Set<string>();
    for (const b of blocks) {
      if (!b.isConnected) continue;
      const r = this.processBlock(b, need, skipCache, false);
      restored += r.count;
      r.keys.forEach((k) => keys.add(k));
      if (r.unresolved) retry.push(b);
      if (r.deferred) this.deferred.add(b);
    }
    if (restored > 0) this.onRestored?.(restored, [...keys]);
    if (this.deferred.size > 0) {
      if (this.settleTimer) clearTimeout(this.settleTimer);
      this.settleTimer = setTimeout(() => void this.settle(), this.settleMs);
    }

    if (need.size > 0) {
      await this.source.load([...need]).catch(() => undefined);
      if (!this.revealed) return;
      let again = 0;
      const more = new Set<string>();
      for (const b of retry) {
        if (!b.isConnected) continue;
        const r = this.processBlock(b, new Set(), skipCache, false);
        again += r.count;
        if (r.deferred) this.deferred.add(b);
        r.keys.forEach((k) => more.add(k));
      }
      if (again > 0) this.onRestored?.(again, [...more]);
    }
  }

  // ---- finding text ---------------------------------------------------------------------

  private blockOf(node: Node): Element | null {
    let el: Element | null = node.parentElement;
    while (el && !BLOCK_TAGS.has(el.tagName)) el = el.parentElement;
    return el;
  }

  private skipped(el: Element, cache: Map<Element, boolean>): boolean {
    const hit = cache.get(el);
    if (hit !== undefined) return hit;
    const result = SKIP_TAGS.has(el.tagName) || isEditable(el) || (el.parentElement ? this.skipped(el.parentElement, cache) : false);
    cache.set(el, result);
    return result;
  }

  /** Adds the block-level elements that hold text under `root` (and `root` if it is one). */
  private blocksUnder(root: Node, out: Set<Element>): void {
    const walker = this.doc.createTreeWalker(root, SHOW_TEXT);
    for (let n = walker.nextNode(); n; n = walker.nextNode()) {
      if (!n.nodeValue) continue;
      const b = this.blockOf(n);
      if (b) out.add(b);
    }
  }

  /** Restores every run of text under a block. */
  private processBlock(
    block: Element,
    need: Set<string>,
    skipCache: Map<Element, boolean>,
    final: boolean,
  ): { count: number; keys: string[]; unresolved: boolean; deferred: boolean } {
    // A run = consecutive text nodes sharing the same nearest block ancestor.
    const runs: Text[][] = [];
    let current: Text[] = [];
    let currentBlock: Element | null = null;
    const walker = this.doc.createTreeWalker(block, SHOW_TEXT);
    for (let n = walker.nextNode(); n; n = walker.nextNode()) {
      const t = n as Text;
      const parent = t.parentElement;
      if (!parent || this.skipped(parent, skipCache)) continue;
      const b = this.blockOf(t);
      if (b !== currentBlock) {
        if (current.length) runs.push(current);
        current = [];
        currentBlock = b;
      }
      current.push(t);
    }
    if (current.length) runs.push(current);

    let count = 0;
    const keys: string[] = [];
    let unresolved = false;
    let deferred = false;
    for (const run of runs) {
      const r = this.processRun(run, need, final);
      count += r.count;
      keys.push(...r.keys);
      unresolved ||= r.unresolved;
      deferred ||= r.deferred;
    }
    return { count, keys, unresolved, deferred };
  }

  // ---- restoring ------------------------------------------------------------------------

  private processRun(nodes: Text[], need: Set<string>, final: boolean): { count: number; keys: string[]; unresolved: boolean; deferred: boolean } {
    const texts = nodes.map((n) => n.nodeValue ?? "");
    const offsets: number[] = [];
    let joined = "";
    for (const t of texts) {
      offsets.push(joined.length);
      joined += t;
    }
    const hasNote = joined.includes(NOTE_PREFIX) && NOTE_RE.test(joined);
    const mightHave = this.matcher.size > 0 && this.matcher.mightContain(joined);
    if (!hasNote && !mightHave) return { count: 0, keys: [], unresolved: false, deferred: false };

    // Global edits over the joined text, non-overlapping, in order.
    const edits: Array<{ start: number; end: number; value: string; key?: string }> = [];
    let unresolved = false;
    let deferred = false;
    if (hasNote) edits.push({ start: 0, end: NOTE_RE.exec(joined)![0].length, value: "" });
    if (mightHave) {
      for (const v of this.matcher.find(joined)) {
        // Streaming: text that ends mid-placeholder may still grow ("{{API_KEY_1" -> "{{API_KEY_1}}",
        // "API_KEY_1" -> "API_KEY_12"). Wait for it to settle instead of restoring too early.
        if (!final && !v.complete && v.end === joined.length) {
          deferred = true;
          continue;
        }
        const value = this.source.value(v.key);
        if (value === undefined) {
          need.add(v.key);
          unresolved = true;
          continue;
        }
        if (edits.some((e) => v.start < e.end && v.end > e.start)) continue;
        edits.push({ start: v.start, end: v.end, value, key: v.key });
      }
    }
    if (edits.length === 0) return { count: 0, keys: [], unresolved, deferred };
    edits.sort((a, b) => a.start - b.start);

    // Split the global edits into per-node edits: the first node a match touches gets the
    // value, every other node loses just its covered slice.
    const nodeOf = (pos: number) => {
      let i = offsets.length - 1;
      while (i > 0 && offsets[i]! > pos) i--;
      return i;
    };
    const perNode: NodeEdit[][] = nodes.map(() => []);
    for (const e of edits) {
      const first = nodeOf(e.start);
      const last = nodeOf(Math.max(e.start, e.end - 1));
      for (let i = first; i <= last; i++) {
        const nodeStart = offsets[i]!;
        perNode[i]!.push({
          from: Math.max(e.start, nodeStart) - nodeStart,
          to: Math.min(e.end, nodeStart + texts[i]!.length) - nodeStart,
          value: i === first ? e.value : "",
          key: i === first ? e.key : undefined,
        });
      }
    }

    const keys: string[] = [];
    nodes.forEach((node, i) => {
      const list = perNode[i]!;
      if (list.length === 0) return;
      const original = texts[i]!;
      let out = "";
      let pos = 0;
      const spans: Array<[number, number]> = [];
      for (const e of list) {
        out += original.slice(pos, e.from);
        const start = out.length;
        out += e.value;
        if (e.key && e.value) {
          spans.push([start, out.length]);
          keys.push(e.key);
        }
        pos = e.to;
      }
      out += original.slice(pos);
      if (out === original) return;
      const prev = this.records.get(node);
      // A node restored in two passes keeps its first masked text; a node the page rewrote
      // since our last edit starts over.
      const masked = prev && original === prev.restored ? prev.masked : original;
      if (prev) this.unpaint(prev);
      const rec: NodeRecord = { masked, restored: out, spans, ranges: [] };
      node.nodeValue = out;
      this.records.set(node, rec);
      this.tracked.add(new WeakRef(node));
      this.paint(node, rec);
    });
    return { count: keys.length, keys, unresolved, deferred };
  }

  // ---- highlighting ---------------------------------------------------------------------

  private setupHighlight(): void {
    const win = this.win as any;
    if (this.highlight || !win.CSS?.highlights || typeof win.Highlight !== "function") return;
    this.highlight = new win.Highlight();
    win.CSS.highlights.set(HIGHLIGHT_NAME, this.highlight);
    try {
      if (typeof win.CSSStyleSheet === "function" && "adoptedStyleSheets" in this.doc) {
        const sheet = new win.CSSStyleSheet();
        sheet.replaceSync(
          `::highlight(${HIGHLIGHT_NAME}) { background-color: rgba(46, 230, 197, 0.28); color: inherit; text-decoration: underline dotted rgba(46, 230, 197, 0.9); }`,
        );
        (this.doc as any).adoptedStyleSheets = [...(this.doc as any).adoptedStyleSheets, sheet];
      }
    } catch {
      /* highlight without style still works, just invisible */
    }
  }

  private unpaint(rec: NodeRecord): void {
    for (const r of rec.ranges) this.highlight?.delete(r);
    rec.ranges = [];
  }

  private paint(node: Text, rec: NodeRecord): void {
    if (!this.highlight) return;
    this.unpaint(rec);
    for (const [start, end] of rec.spans) {
      try {
        const range = this.doc.createRange();
        range.setStart(node, start);
        range.setEnd(node, end);
        this.highlight.add(range);
        rec.ranges.push(range);
      } catch {
        /* the node changed under us: skip this highlight */
      }
    }
  }

  private paintAll(): void {
    if (!this.highlight) return;
    this.highlight.clear();
    for (const ref of this.tracked) {
      const node = ref.deref();
      const rec = node ? this.records.get(node) : undefined;
      if (node && rec) this.paint(node, rec);
    }
  }

  /** Puts every node back (Zuko turned off for this site). */
  private revert(): void {
    for (const ref of this.tracked) {
      const node = ref.deref();
      const rec = node ? this.records.get(node) : undefined;
      if (node && rec && node.isConnected) node.nodeValue = rec.masked;
    }
    this.tracked.clear();
    this.highlight?.clear();
  }
}
