// Offscreen document: runs pdf.js (workers and the DOM are not available in the service
// worker) and answers `pdf-extract` requests from it with markdown text.
// pdf.js is bundled here; its worker is copied next to offscreen.html by the build and loaded
// by relative path, so no remote code is ever used.

import * as pdfjs from "pdfjs-dist/build/pdf.min.mjs";
import { extractPdfText, type PdfLib } from "./pdf-text.ts";
import type { PdfExtract } from "../shared/protocol.ts";

(pdfjs as any).GlobalWorkerOptions.workerSrc = "pdf.worker.min.mjs";

function fromBase64(b64: string): Uint8Array {
  const bin = atob(b64);
  const bytes = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
  return bytes;
}

chrome.runtime.onMessage.addListener((msg, sender, sendResponse) => {
  if (!msg || msg.target !== "offscreen" || msg.type !== "pdf-extract") return false;
  if (sender.id !== chrome.runtime.id) return false;
  extractPdfText(pdfjs as unknown as PdfLib, fromBase64(String(msg.base64))).then(sendResponse, (e): void =>
    sendResponse({ ok: false, error: e instanceof Error ? e.message : String(e), pages: 0, markdown: "", hasText: false, warnings: [] } satisfies PdfExtract),
  );
  return true;
});
