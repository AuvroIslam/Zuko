// PDF text extraction -> markdown ("## Page N" sections). Pure over a pdf.js module so the
// offscreen document (browser build) and the tests (Node build) share it.
//
// Text only, by design: no OCR, no images. A PDF with no text layer reports `hasText: false`
// and the caller blocks the upload, because nothing in it could be checked.

import type { PdfExtract } from "../shared/protocol.ts";

/** The slice of pdf.js we use. */
export interface PdfLib {
  getDocument(src: Record<string, unknown>): { promise: Promise<PdfDoc>; destroy?: () => Promise<void> };
}
interface PdfDoc {
  numPages: number;
  getPage(n: number): Promise<{ getTextContent(): Promise<{ items: PdfItem[] }>; cleanup?: () => void }>;
  destroy?: () => Promise<void>;
}
interface PdfItem {
  str?: string;
  hasEOL?: boolean;
  width?: number;
  height?: number;
  transform?: number[];
}

export const MAX_PAGES = 500;
export const MAX_CHARS = 8_000_000;

/** Joins one page's text items into lines, keeping words apart and lines on separate rows. */
export function pageText(items: PdfItem[]): string {
  let out = "";
  let lastY: number | null = null;
  let lastEndX: number | null = null;
  for (const it of items) {
    const str = it.str ?? "";
    if (!str && !it.hasEOL) continue;
    const x = it.transform?.[4] ?? 0;
    const y = it.transform?.[5] ?? 0;
    const h = it.height ?? 0;
    if (lastY !== null && Math.abs(y - lastY) > Math.max(2, h * 0.5)) {
      if (!out.endsWith("\n")) out += "\n";
      lastEndX = null;
    } else if (lastEndX !== null && x - lastEndX > h * 0.25 && !out.endsWith(" ") && !out.endsWith("\n") && !str.startsWith(" ")) {
      out += " ";
    }
    out += str;
    lastY = y;
    lastEndX = x + (it.width ?? 0);
    if (it.hasEOL) {
      out += "\n";
      lastY = null;
      lastEndX = null;
    }
  }
  return out.replace(/[ \t]+\n/g, "\n").replace(/\n{3,}/g, "\n\n").trim();
}

export async function extractPdfText(pdfjs: PdfLib, data: Uint8Array, limits: { maxPages?: number; maxChars?: number } = {}): Promise<PdfExtract> {
  const maxPages = limits.maxPages ?? MAX_PAGES;
  const maxChars = limits.maxChars ?? MAX_CHARS;
  const warnings: string[] = [];
  const task = pdfjs.getDocument({
    data,
    isEvalSupported: false, // no eval/Function in extension pages
    useSystemFonts: false,
    disableFontFace: true,
    enableXfa: false,
    verbosity: 0,
  });
  let doc: PdfDoc;
  try {
    doc = await task.promise;
  } catch (e) {
    const name = (e as { name?: string })?.name ?? "";
    const msg = name === "PasswordException" ? "the PDF is password protected" : e instanceof Error ? e.message : String(e);
    return { ok: false, error: msg, pages: 0, markdown: "", hasText: false, warnings };
  }

  try {
    const total = doc.numPages;
    const count = Math.min(total, maxPages);
    if (total > count) warnings.push(`Only the first ${count} of ${total} pages were read.`);
    const sections: string[] = [];
    const empty: number[] = [];
    let chars = 0;
    let hasText = false;
    for (let n = 1; n <= count; n++) {
      const page = await doc.getPage(n);
      const content = await page.getTextContent();
      page.cleanup?.();
      const text = pageText(content.items);
      if (text) hasText = true;
      else empty.push(n);
      chars += text.length;
      sections.push(`## Page ${n}\n\n${text || "_(no text on this page)_"}`);
      if (chars > maxChars) {
        warnings.push(`The text was cut after page ${n} (too long).`);
        break;
      }
    }
    if (hasText && empty.length > 0) {
      const shown = empty.slice(0, 8).join(", ") + (empty.length > 8 ? ", ..." : "");
      warnings.push(`No text layer on page${empty.length === 1 ? "" : "s"} ${shown}: scanned pages are not checked.`);
    }
    if (!hasText) warnings.push("No text layer found (scanned PDF?). Zuko does not do OCR.");
    return { ok: true, pages: total, markdown: sections.join("\n\n") + "\n", hasText, warnings };
  } finally {
    await doc.destroy?.().catch(() => undefined);
  }
}
