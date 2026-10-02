// Documents: sanitize a file into `<name>.zuko.md` with every finding masked,
// and a mask / unmask box for any text (answers from web chats, snippets…).

import { h, svg, clear, copyText } from "../views/dom";
import { ICONS } from "../views/icons";
import { Bridge, IS_MOCK, IS_TAURI, onDragDrop, type SanitizeResult } from "../core/bridge";
import { placeholder, plural } from "../views/format";
import { feedback, hint, notice, section, sub } from "./ui";

const ACCEPT = ".txt,.md,.markdown,.pdf,.json,.csv,.log,.env,.yaml,.yml,.toml,.ini,.ts,.tsx,.js,.jsx,.py,.rs,.go,.java,.cs,.rb,.php,.sh,.ps1,.sql";

export function documentsSection(): HTMLElement {
  const { el } = section("documents", "Documents");

  // ── Sanitize a file ─────────────────────────────────────────────────────────
  const picker = h("input", { type: "file", accept: ACCEPT, style: "display:none" }) as HTMLInputElement;
  const drop = h("div", { class: "drop", tabindex: "0", role: "button" },
    svg(ICONS.doc, 18),
    h("div", {}, h("b", { text: "Drop a file here or choose one" }),
      h("span", { class: "hint", text: ".txt, .md, .pdf or code — up to a few MB" })));
  const pathInput = h("input", { type: "text", placeholder: "…or paste a full path", spellcheck: "false", style: "flex:1 1 auto;min-width:0" }) as HTMLInputElement;
  const go = h("button", { class: "primary", text: "Sanitize" });
  const result = h("div", { class: "stack-12" });
  const fb = feedback();

  async function sanitize(path: string) {
    path = path.trim().replace(/^"(.*)"$/, "$1");
    if (!path) return;
    fb.clear();
    clear(result);
    result.append(hint(`Scanning ${path.split(/[\\/]/).pop()}…`));
    go.disabled = true;
    try {
      const r = await Bridge.sanitizeFile(path);
      clear(result);
      result.append(renderResult(r));
    } catch (e) {
      clear(result);
      fb.error(e, "Couldn't sanitize");
    } finally {
      go.disabled = false;
    }
  }

  function renderResult(r: SanitizeResult): HTMLElement {
    const total = r.findings.reduce((n, f) => n + f.count, 0);
    const kind = r.kind === "pdf" ? `PDF${r.pages ? ` · ${plural(r.pages, "page")}` : ""}` : r.kind;
    const show = h("button", { text: "Show file", onclick: () => void Bridge.revealPath(r.outputPath) });
    const copy = h("button", { text: "Copy preview" });
    copy.addEventListener("click", async () => {
      copy.textContent = (await copyText(r.preview)) ? "Copied" : "Copy failed";
      window.setTimeout(() => (copy.textContent = "Copy preview"), 1600);
    });
    return h("div", { class: "result" },
      h("div", { class: "result-head" },
        h("b", { text: r.name }), h("span", { class: "hint", text: kind }),
        h("span", { class: total ? "badge teal" : "badge green", text: total ? `${plural(total, "value")} masked` : "Nothing sensitive found" })),
      r.findings.length
        ? h("div", { class: "findings" }, ...r.findings.map((f) =>
          h("span", { class: "finding", title: f.kind }, h("span", { text: f.label }),
            h("code", { text: placeholder(f.key) }), f.count > 1 ? h("em", { text: `×${f.count}` }) : null)))
        : null,
      ...r.warnings.map((w) => notice("warn", w)),
      h("pre", { class: "preview-box", text: r.preview }),
      h("div", { class: "row" }, show, copy, h("span", { class: "path", text: r.outputPath })),
    );
  }

  drop.addEventListener("click", () => picker.click());
  drop.addEventListener("keydown", (e) => {
    if (e.key === "Enter" || e.key === " ") picker.click();
  });
  picker.addEventListener("change", () => {
    const f = picker.files?.[0];
    picker.value = "";
    if (!f) return;
    // A webview never sees the real path of a picked file; the mock just needs a name.
    const path = (f as File & { path?: string }).path;
    if (path || !IS_TAURI) void sanitize(path || f.name);
    else fb.show("warn", "Drag the file onto this box, or paste its full path.");
  });
  // Plain-browser drops (dev); in the app the webview's own drop event below wins.
  drop.addEventListener("dragover", (e) => {
    e.preventDefault();
    drop.classList.add("over");
  });
  drop.addEventListener("dragleave", () => drop.classList.remove("over"));
  drop.addEventListener("drop", (e) => {
    e.preventDefault();
    drop.classList.remove("over");
    const f = e.dataTransfer?.files?.[0];
    if (f && !IS_TAURI) void sanitize(f.name);
  });
  void onDragDrop((e) => {
    if (e.type === "enter" || e.type === "over") drop.classList.add("over");
    else drop.classList.remove("over");
    if (e.type === "drop" && e.paths?.[0]) {
      document.getElementById("documents")?.scrollIntoView({ behavior: "smooth", block: "start" });
      void sanitize(e.paths[0]);
    }
  });
  go.addEventListener("click", () => void sanitize(pathInput.value));
  pathInput.addEventListener("keydown", (e) => {
    if (e.key === "Enter") void sanitize(pathInput.value);
  });

  // ── Mask / unmask text ──────────────────────────────────────────────────────
  const input = h("textarea", { rows: "5", placeholder: "Paste text with secrets to mask — or an answer with {{PLACEHOLDERS}} to restore.", spellcheck: "false" }) as HTMLTextAreaElement;
  const output = h("textarea", { rows: "5", readonly: true, placeholder: "Result", spellcheck: "false" }) as HTMLTextAreaElement;
  const report = h("span", { class: "hint" });
  const tfb = feedback();
  const mask = h("button", { class: "primary", text: "Mask" });
  const unmask = h("button", { text: "Unmask" });
  const copyOut = h("button", { text: "Copy result" });
  const clipMask = h("button", { class: "small", text: "Mask clipboard" });
  const clipUnmask = h("button", { class: "small", text: "Unmask clipboard" });

  mask.addEventListener("click", async () => {
    if (!input.value) return;
    tfb.clear();
    try {
      const r = await Bridge.maskText(input.value);
      output.value = r.text;
      const keys = r.report.keys.map(placeholder).join(", ");
      report.textContent = r.report.count
        ? `${plural(r.report.count, "value")} masked${keys ? `: ${keys}` : ""}${r.report.newKeys.length ? ` (${r.report.newKeys.length} new)` : ""}`
        : "Nothing to mask.";
    } catch (e) {
      tfb.error(e, "Couldn't mask");
    }
  });
  unmask.addEventListener("click", async () => {
    if (!input.value) return;
    tfb.clear();
    try {
      const r = await Bridge.unmaskText(input.value);
      output.value = r.text;
      report.textContent = r.keys.length ? `Restored ${r.keys.map(placeholder).join(", ")}` : "No known placeholders found.";
    } catch (e) {
      tfb.error(e, "Couldn't unmask");
    }
  });
  copyOut.addEventListener("click", async () => {
    if (!output.value) return;
    copyOut.textContent = (await copyText(output.value)) ? "Copied" : "Copy failed";
    window.setTimeout(() => (copyOut.textContent = "Copy result"), 1600);
  });
  const clip = (fn: () => Promise<{ count: number }>, verb: string) => async () => {
    tfb.clear();
    try {
      const r = await fn();
      tfb.show(r.count ? "ok" : "warn", r.count ? `${verb} ${plural(r.count, "value")} on the clipboard.` : "Nothing to change on the clipboard.");
    } catch (e) {
      tfb.error(e);
    }
  };
  clipMask.addEventListener("click", clip(() => Bridge.clipboardMask(), "Masked"));
  clipUnmask.addEventListener("click", clip(() => Bridge.clipboardUnmask(), "Restored"));

  el.append(
    hint("Share a document without its secrets: Zuko writes a masked copy you can paste or upload anywhere."),
    drop,
    picker,
    h("div", { class: "row" }, pathInput, go),
    result,
    fb.el,
    sub("Mask or unmask text", "Works with any chat app"),
    input,
    h("div", { class: "row" }, mask, unmask, copyOut, h("div", { class: "spacer" }), clipMask, clipUnmask),
    output,
    report,
    tfb.el,
  );

  // Dev only (`?mock=1&demo=1`): fill both tools so a screenshot shows results.
  if (IS_MOCK && new URLSearchParams(window.location.search).has("demo")) {
    void sanitize("C:\\Users\\dev\\Documents\\invoice-march.pdf");
    input.value = "My key is sk-proj-Xk29fLm0aQ7rT1vB3nZ8yW4uE6iO2pS5dG9fQa, mail jamie.rahman@acme.io if it fails.";
    mask.click();
  }
  return el;
}
