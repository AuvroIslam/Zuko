// Copy rehydration in the isolated world:
//  * `copy` event (selection copied with Ctrl+C or the context menu): if the selection still
//    contains placeholders whose values are cached, the clipboard gets the real values;
//  * `clipboard` requests from the page's patched writeText/write (copy buttons): restore the
//    values here and write the clipboard from this world, so real values never pass through
//    the page's JavaScript.
// If anything fails the page's own copy goes ahead (with placeholders), which is safe.

import { replaceVariants, type VariantMatcher } from "../shared/placeholders.ts";
import { isEditable } from "./sites.ts";
import type { ValueSource } from "./rehydrate.ts";

const escapeHtml = (s: string) => s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");

export interface CopyEnv {
  doc: Document;
  matcher: () => VariantMatcher;
  source: ValueSource;
  enabled: () => boolean;
}

/** Restores values in `text`, loading what is not cached yet. Returns null when nothing changes. */
export async function restoreText(text: string, env: Pick<CopyEnv, "matcher" | "source">, html = false): Promise<string | null> {
  const matcher = env.matcher();
  if (matcher.size === 0 || !matcher.mightContain(text)) return null;
  const keys = matcher.keysIn(text);
  if (keys.length === 0) return null;
  await env.source.load(keys);
  const esc = html ? escapeHtml : (s: string) => s;
  const r = replaceVariants(text, matcher, (k) => {
    const v = env.source.value(k);
    return v === undefined ? undefined : esc(v);
  });
  return r.count > 0 ? r.text : null;
}

/** Answers a page `clipboard` request. `written: false` tells the page to do its own copy. */
export async function writeRestoredClipboard(items: Record<string, string>, env: Pick<CopyEnv, "matcher" | "source" | "doc">): Promise<{ written: boolean }> {
  const out: Record<string, string> = {};
  let changed = false;
  for (const [type, text] of Object.entries(items)) {
    const restored = await restoreText(text, env, type === "text/html");
    out[type] = restored ?? text;
    if (restored !== null) changed = true;
  }
  if (!changed) return { written: false };
  const nav = env.doc.defaultView?.navigator;
  const win = env.doc.defaultView as any;
  try {
    if (Object.keys(out).length === 1 && out["text/plain"] !== undefined) {
      await nav!.clipboard.writeText(out["text/plain"]);
    } else {
      const data: Record<string, Blob> = {};
      for (const [type, text] of Object.entries(out)) data[type] = new win.Blob([text], { type });
      await nav!.clipboard.write([new win.ClipboardItem(data)]);
    }
    return { written: true };
  } catch {
    return { written: false };
  }
}

export function installCopyHandler(env: CopyEnv): void {
  env.doc.addEventListener(
    "copy",
    (e) => {
      if (!env.enabled()) return;
      const sel = env.doc.getSelection();
      const text = sel?.toString();
      if (!text || !e.clipboardData) return;
      const anchor = sel?.anchorNode instanceof Element ? sel.anchorNode : (sel?.anchorNode?.parentElement ?? null);
      for (let p: Element | null = anchor; p; p = p.parentElement) if (isEditable(p)) return; // the user's own typing
      const matcher = env.matcher();
      if (matcher.size === 0 || !matcher.mightContain(text)) return;
      // Synchronous only: copy events cannot wait. Values appear here once the DOM pass loaded them.
      const r = replaceVariants(text, matcher, (k) => env.source.value(k));
      if (r.count === 0) return;
      e.clipboardData.setData("text/plain", r.text);
      e.preventDefault();
    },
    true,
  );
}
