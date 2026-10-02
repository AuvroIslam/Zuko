import assert from "node:assert/strict";
import { test } from "node:test";
import * as pdfjs from "pdfjs-dist/legacy/build/pdf.mjs";
import { UploadGuard, needsSanitizing, sanitizeFile } from "../src/content/uploads.ts";
import { extractPdfText, pageText } from "../src/offscreen/pdf-text.ts";
import { hasNote, stripNote } from "../src/shared/placeholders.ts";
import { AWS, KEY, MAIL, makeWindow, realBrain, tick } from "./helpers.mjs";
import { makePdf } from "./pdf-fixtures.mjs";

/** UploadDeps wired to the real engine (and the real pdf.js for PDFs). */
async function deps(overrides = {}) {
  const brain = await realBrain();
  const log = { notices: [], reports: [] };
  const base = {
    brain,
    log,
    async maskText(text) {
      const r = await brain.maskMany([text], { site: "chatgpt", mode: "full", source: "file" });
      return { text: r.texts[0], count: r.count, keys: r.keys };
    },
    async sanitizePdf(base64) {
      const ex = await extractPdfText(pdfjs, new Uint8Array(Buffer.from(base64, "base64")));
      if (!ex.ok) return { ok: false, error: ex.error };
      if (!ex.hasText) return { ok: true, blocked: true, pages: ex.pages, warnings: ex.warnings };
      const m = await brain.maskMany([ex.markdown], { site: "chatgpt", mode: "full", source: "file" });
      return { ok: true, blocked: false, markdown: m.texts[0], pages: ex.pages, warnings: ex.warnings, count: m.count, keys: m.keys };
    },
    notify: (level, text) => log.notices.push([level, text]),
    report: (count, keys) => log.reports.push([count, keys]),
  };
  return { ...base, ...overrides };
}

const file = (parts, name, type = "") => new File(Array.isArray(parts) ? parts : [parts], name, { type });
const read = (f) => f.text();

// ---- text-like files -----------------------------------------------------------------------

test(".env, .json, .csv and code files are masked in place, keeping name and type", async () => {
  const d = await deps();
  const env = await sanitizeFile(file(`OPENAI_API_KEY=${KEY}\nDEBUG=1\n`, ".env"), d);
  assert.equal(await read(env.file), "OPENAI_API_KEY={{API_KEY_1}}\nDEBUG=1\n");
  assert.equal(env.file.name, ".env");
  assert.equal(env.count, 1);
  assert.equal(env.changed, true);

  const jsonSrc = JSON.stringify({ service: "billing", contact: MAIL, aws: { key: AWS }, n: 3 }, null, 2);
  const j = await sanitizeFile(file(jsonSrc, "config.json", "application/json"), d);
  const parsed = JSON.parse(await read(j.file));
  assert.deepEqual(parsed, { service: "billing", contact: "{{EMAIL_1}}", aws: { key: "{{API_KEY_2}}" }, n: 3 }, "still valid JSON, structure intact");
  assert.equal(j.file.type, "application/json");

  const csv = await sanitizeFile(file(`name,email,plan\nRahim,${MAIL},pro\nKarim,${MAIL},free\n`, "users.csv", "text/csv"), d);
  assert.equal(await read(csv.file), "name,email,plan\nRahim,{{EMAIL_1}},pro\nKarim,{{EMAIL_1}},free\n");

  const py = await sanitizeFile(file(`import os\nAPI = "${KEY}"\nprint(API)\n`, "run.py", "text/x-python"), d);
  assert.equal(await read(py.file), 'import os\nAPI = "{{API_KEY_3}}"\nprint(API)\n'.replace("API_KEY_3", "API_KEY_1"));
});

test(".txt and .md get the one-line note on top; structured files do not", async () => {
  const d = await deps();
  for (const name of ["notes.txt", "README.md", "NOTES"]) {
    const r = await sanitizeFile(file(`token ${KEY}`, name), d);
    const out = await read(r.file);
    assert.ok(hasNote(out), name);
    assert.equal(stripNote(out), "token {{API_KEY_1}}");
  }
  const j = await sanitizeFile(file(JSON.stringify({ k: KEY }), "x.json"), d);
  assert.ok(!hasNote(await read(j.file)));
});

test("a clean file is passed through as the very same File object", async () => {
  const d = await deps();
  const f = file("just notes about sorting algorithms", "notes.txt", "text/plain");
  const r = await sanitizeFile(f, d);
  assert.equal(r.file, f);
  assert.equal(r.changed, false);
  assert.equal(r.message, "");
});

test("files Zuko cannot read are passed with a warning, never silently altered", async () => {
  const d = await deps();
  const bin = new File([new Uint8Array([0xff, 0xfe, 0xfd, 0x80])], "weird.txt", { type: "text/plain" });
  let r = await sanitizeFile(bin, d);
  assert.equal(r.file, bin);
  assert.equal(r.level, "warn");
  assert.match(r.message, /not UTF-8/);

  const docx = file("PK\u0003\u0004", "report.docx");
  r = await sanitizeFile(docx, d);
  assert.equal(r.file, docx);
  assert.match(r.message, /cannot look inside report\.docx/);

  const png = file(new Uint8Array([0x89, 0x50, 0x4e, 0x47]), "shot.png", "image/png");
  r = await sanitizeFile(png, d);
  assert.deepEqual([r.file, r.message], [png, ""]);
  assert.equal(needsSanitizing(png), false);
  assert.equal(needsSanitizing(file("", "a.pdf", "application/pdf")), true);
  assert.equal(needsSanitizing(file("", "a.ts")), true);
});

// ---- PDF ------------------------------------------------------------------------------------

test("a text PDF becomes name.zuko.md: '## Page N' sections, masked, with the note on top", async () => {
  const d = await deps();
  const pdf = makePdf([
    ["Quarterly report", `Contact: ${MAIL}`],
    ["Deploy credentials", `AWS access key ${AWS}`, "Do not share."],
  ]);
  const r = await sanitizeFile(new File([pdf], "Report Q3.pdf", { type: "application/pdf" }), d);
  assert.equal(r.file.name, "Report Q3.zuko.md");
  assert.equal(r.file.type, "text/markdown");
  const md = await read(r.file);
  assert.ok(hasNote(md));
  const body = stripNote(md);
  assert.match(body, /^## Page 1\n\nQuarterly report\nContact: \{\{EMAIL_1\}\}\n\n## Page 2\n\nDeploy credentials\nAWS access key \{\{API_KEY_1\}\}\nDo not share\.\n$/);
  assert.ok(!md.includes(AWS) && !md.includes(MAIL));
  assert.equal(r.count, 2);
  assert.match(r.message, /text-only copy/);
  assert.match(r.message, /2 items masked/);
});

test("a PDF with nothing sensitive is still replaced by its text-only copy (so hidden content is dropped)", async () => {
  const d = await deps();
  const r = await sanitizeFile(new File([makePdf([["Hello world"]])], "hello.pdf", { type: "application/pdf" }), d);
  assert.equal(r.file.name, "hello.zuko.md");
  assert.equal(r.count, 0);
  assert.match(await read(r.file), /## Page 1\n\nHello world/);
  assert.match(r.message, /Nothing sensitive found/);
});

test("a scanned PDF (no text layer) is blocked, not uploaded", async () => {
  const d = await deps();
  const r = await sanitizeFile(new File([makePdf([[], []])], "scan.pdf", { type: "application/pdf" }), d);
  assert.equal(r.file, null);
  assert.equal(r.level, "error");
  assert.match(r.message, /no text layer/);
});

test("pages without text in an otherwise readable PDF are named in a warning", async () => {
  const ex = await extractPdfText(pdfjs, makePdf([[], ["text on page two"], []]));
  assert.equal(ex.ok, true);
  assert.equal(ex.hasText, true);
  assert.equal(ex.pages, 3);
  assert.match(ex.markdown, /## Page 1\n\n_\(no text on this page\)_/);
  assert.ok(ex.warnings.some((w) => /pages 1, 3/.test(w)));
});

test("a damaged PDF is removed from the upload with an explanation", async () => {
  const d = await deps();
  const r = await sanitizeFile(new File([new TextEncoder().encode("%PDF-1.4\nthis is not a pdf")], "broken.pdf", { type: "application/pdf" }), d);
  assert.equal(r.file, null);
  assert.match(r.message, /could not read broken\.pdf/);
});

test("pdf text joins words and keeps lines apart", () => {
  const t = pageText([
    { str: "Hello", transform: [1, 0, 0, 1, 10, 100], width: 25, height: 10 },
    { str: "world", transform: [1, 0, 0, 1, 40, 100], width: 25, height: 10 },
    { str: "next", transform: [1, 0, 0, 1, 10, 80], width: 20, height: 10 },
    { str: "line", transform: [1, 0, 0, 1, 31, 80], width: 18, height: 10, hasEOL: true },
    { str: "last", transform: [1, 0, 0, 1, 10, 60], width: 20, height: 10 },
  ]);
  assert.equal(t, "Hello world\nnext line\nlast");
});

// ---- the DOM wiring --------------------------------------------------------------------------

function wiring(d) {
  const win = makeWindow();
  win.DataTransfer = class {
    constructor() {
      this.files = [];
      this.items = { add: (f) => this.files.push(f) };
    }
  };
  win.DragEvent = class extends win.Event {
    constructor(type, init = {}) {
      super(type, init);
      this.dataTransfer = init.dataTransfer;
    }
  };
  win.ClipboardEvent = class extends win.Event {
    constructor(type, init = {}) {
      super(type, init);
      this.clipboardData = init.clipboardData;
    }
  };
  const guard = new UploadGuard(win, d);
  guard.start(); // before the page registers anything, like at document_start
  return win;
}

test("input[type=file] change: the page only ever sees the sanitized selection", async () => {
  const d = await deps();
  const win = wiring(d);
  const input = win.document.createElement("input");
  input.type = "file";
  win.document.body.appendChild(input);
  const seen = [];
  win.document.addEventListener("change", (e) => seen.push([...e.target.files].map((f) => f.name)));
  Object.defineProperty(input, "files", { value: [file(`k=${KEY}`, "a.txt"), file("hi", "b.png", "image/png")], configurable: true, writable: true });
  input.dispatchEvent(new win.Event("change", { bubbles: true }));
  assert.deepEqual(seen, [], "the original event was stopped before the page");
  await tick(50);
  assert.deepEqual(seen, [["a.txt", "b.png"]], "re-dispatched once, with the clean files");
  assert.ok(hasNote(await read(input.files[0])));
  assert.ok(!(await read(input.files[0])).includes(KEY));
  assert.deepEqual(d.log.reports, [[1, ["API_KEY_1"]]]);
  assert.ok(d.log.notices.some(([l, t]) => l === "info" && /Checking a\.txt/.test(t)));
});

test("a scanned PDF selection leaves the input empty; the page still gets a change event", async () => {
  const d = await deps();
  const win = wiring(d);
  const input = win.document.createElement("input");
  input.type = "file";
  win.document.body.appendChild(input);
  const seen = [];
  win.document.addEventListener("change", (e) => seen.push(e.target.files.length));
  Object.defineProperty(input, "files", { value: [new File([makePdf([[]])], "scan.pdf", { type: "application/pdf" })], configurable: true, writable: true });
  input.dispatchEvent(new win.Event("change", { bubbles: true }));
  await tick(80);
  assert.deepEqual(seen, [0]);
  assert.ok(d.log.notices.some(([l, t]) => l === "error" && /no text layer/.test(t)));
});

test("drop and paste with files are re-dispatched with sanitized files; the originals are cancelled", async () => {
  const d = await deps();
  const win = wiring(d);
  const zone = win.document.createElement("div");
  win.document.body.appendChild(zone);
  const dropped = [];
  const pasted = [];
  zone.addEventListener("drop", (e) => dropped.push([...e.dataTransfer.files]));
  zone.addEventListener("paste", (e) => pasted.push([...e.clipboardData.files]));

  const drop = new win.Event("drop", { bubbles: true, cancelable: true });
  drop.dataTransfer = { files: [file(`mail ${MAIL}`, "m.md")] };
  zone.dispatchEvent(drop);
  assert.equal(drop.defaultPrevented, true);
  assert.equal(dropped.length, 0);
  await tick(50);
  assert.equal(dropped.length, 1);
  assert.ok(!(await read(dropped[0][0])).includes(MAIL));

  const paste = new win.Event("paste", { bubbles: true, cancelable: true });
  paste.clipboardData = { files: [file(`k ${AWS}`, "p.txt")] };
  zone.dispatchEvent(paste);
  assert.equal(paste.defaultPrevented, true);
  await tick(50);
  assert.equal(pasted.length, 1);
  assert.ok(!(await read(pasted[0][0])).includes(AWS));
});

test("images and other non-text files are not intercepted at all", async () => {
  const d = await deps();
  const win = wiring(d);
  const zone = win.document.createElement("div");
  win.document.body.appendChild(zone);
  const seen = [];
  zone.addEventListener("drop", (e) => seen.push(e));
  const drop = new win.Event("drop", { bubbles: true, cancelable: true });
  drop.dataTransfer = { files: [file("x", "a.png", "image/png")] };
  zone.dispatchEvent(drop);
  assert.equal(seen.length, 1);
  assert.equal(seen[0], drop, "the very same event reached the page");
  assert.equal(drop.defaultPrevented, false);
});

test("engine failure while checking a text file: the file is dropped from the upload, never sent unchecked", async () => {
  const d = await deps({
    async maskText() {
      throw new Error("engine-unavailable");
    },
  });
  const win = wiring(d);
  const input = win.document.createElement("input");
  input.type = "file";
  win.document.body.appendChild(input);
  const seen = [];
  win.document.addEventListener("change", (e) => seen.push(e.target.files.length));
  Object.defineProperty(input, "files", { value: [file(`k=${KEY}`, "secrets.env.txt")], configurable: true, writable: true });
  input.dispatchEvent(new win.Event("change", { bubbles: true }));
  await tick(50);
  assert.deepEqual(seen, [0]);
  assert.ok(d.log.notices.some(([l, t]) => l === "error" && /could not check secrets\.env\.txt/.test(t)));
});

test("switched off for the site: the guard stays out of the way", async () => {
  const d = await deps({ enabled: () => false });
  const win = wiring(d);
  const zone = win.document.createElement("div");
  win.document.body.appendChild(zone);
  const seen = [];
  zone.addEventListener("drop", (e) => seen.push(e));
  const drop = new win.Event("drop", { bubbles: true, cancelable: true });
  drop.dataTransfer = { files: [file(`k=${KEY}`, "a.txt")] };
  zone.dispatchEvent(drop);
  assert.equal(seen[0], drop);
});
