// Local AI: an optional model running in Ollama on this machine, used to find what
// the patterns miss (names, addresses, organisations, internal hostnames) and to
// explain risky actions in plain English.
//
// It may only make Zuko stricter: it adds masks and explanation text, never removes
// a mask or changes a verdict, and Rust only ever talks to a loopback endpoint.
// Saving fetches the current policy first and replaces only `localAi`, so this
// section never overwrites a change made in the Policy section (and vice versa).

import { h, clear, copyText } from "../views/dom";
import { Bridge, type LocalAiConfig, type LocalAiStatus, type Policy } from "../core/bridge";
import { feedback, hint, section, setDot, statusDot, sub, toggle } from "./ui";

const LOOPBACK = /^https?:\/\/(127\.0\.0\.1|localhost|\[::1\])(:\d{1,5})?\/?$/i;

function validEndpoint(v: string): string | null {
  return LOOPBACK.test(v.trim())
    ? null
    : "Local only: http://127.0.0.1:11434, http://localhost:11434 or http://[::1]:11434.";
}

export function localAiSection(initial: Policy | null): HTMLElement {
  const { el, head } = section("localai", "Local AI");
  if (!initial) {
    el.append(hint("Open this window from the running app to set up the local AI."));
    return el;
  }

  let saved: LocalAiConfig = structuredClone(initial.localAi);
  const draft: LocalAiConfig = structuredClone(initial.localAi);

  const dot = statusDot("off");
  head.prepend(dot);
  const statusLine = h("span", { class: "hint" });
  const pullRow = h("div", { class: "row" });
  const fb = feedback();

  // ── Fields ──────────────────────────────────────────────────────────────────
  const endpoint = h("input", {
    type: "text", value: draft.endpoint, spellcheck: "false", autocomplete: "off",
    style: "flex:1 1 auto;min-width:0",
  }) as HTMLInputElement;
  const endpointErr = h("span", { class: "field-error" });
  endpoint.addEventListener("input", () => {
    draft.endpoint = endpoint.value.trim();
    changed();
  });
  endpoint.addEventListener("change", () => void refresh());

  const model = h("select", {}) as HTMLSelectElement;
  model.addEventListener("change", () => {
    draft.model = model.value;
    changed();
    void refresh();
  });
  function fillModels(models: string[]) {
    clear(model);
    const all = models.includes(draft.model) ? models : [draft.model, ...models];
    for (const m of all) model.append(h("option", { value: m, text: models.includes(m) ? m : `${m} (not installed)` }));
    model.value = draft.model;
  }
  fillModels([]);

  const timeout = h("input", {
    type: "number", min: "0.5", max: "120", step: "0.5",
    value: String(draft.timeoutMs / 1000), style: "width:72px",
  }) as HTMLInputElement;
  timeout.addEventListener("input", () => {
    draft.timeoutMs = Math.round(Number(timeout.value) * 1000);
    changed();
  });

  const option = (label: string, key: keyof LocalAiConfig, help: string) =>
    h("div", { class: "option" },
      toggle(Boolean(draft[key]), (v) => { (draft as unknown as Record<string, boolean>)[key] = v; changed(); }, label),
      h("div", {}, h("b", { text: label }), h("span", { class: "hint", text: help })));

  // ── Save ────────────────────────────────────────────────────────────────────
  const save = h("button", { class: "primary", text: "Save" });
  const dirtyLabel = h("span", { class: "hint" });

  function problems(): string | null {
    const e = validEndpoint(draft.endpoint);
    if (e) return e;
    if (!Number.isFinite(draft.timeoutMs) || draft.timeoutMs < 500 || draft.timeoutMs > 120_000) {
      return "The timeout must be between 0.5 and 120 seconds.";
    }
    return null;
  }

  function changed() {
    const dirty = JSON.stringify(draft) !== JSON.stringify(saved);
    const err = problems();
    endpointErr.textContent = validEndpoint(draft.endpoint) ?? "";
    save.disabled = !dirty || err !== null;
    dirtyLabel.textContent = dirty ? (err ?? "Unsaved changes") : "";
    if (dirty) fb.clear();
  }

  save.addEventListener("click", async () => {
    save.disabled = true;
    try {
      // Only `localAi` changes: start from the policy as it is now.
      const current = await Bridge.policyGet();
      if (!current) throw new Error("Zuko is not running");
      await Bridge.policySet({ ...current, localAi: structuredClone(draft) });
      saved = structuredClone(draft);
      fb.show("ok", draft.enabled ? "Saved. The local AI is on." : "Saved. The local AI is off.");
      void refresh();
    } catch (e) {
      fb.error(e, "Not saved");
    }
    changed();
  });

  // ── Status ──────────────────────────────────────────────────────────────────
  function paint(s: LocalAiStatus | null) {
    clear(pullRow);
    pullRow.style.display = "none";
    if (!s) {
      setDot(dot, "off");
      statusLine.textContent = "Status unavailable.";
      return;
    }
    fillModels(s.models);
    if (!s.endpointOk) {
      setDot(dot, "err");
      statusLine.textContent = s.error ?? "That endpoint is not allowed.";
    } else if (!s.reachable) {
      setDot(dot, draft.enabled ? "err" : "off");
      statusLine.textContent = `${s.error ?? "Ollama is not reachable."} ${s.hint ?? ""}`.trim();
    } else if (!s.modelPresent) {
      setDot(dot, "warn");
      statusLine.textContent = `Ollama is running, but ${s.model} is not installed. Run this in a terminal:`;
      const cmd = s.hint ?? `ollama pull ${s.model}`;
      const copy = h("button", { class: "small", text: "Copy" });
      copy.addEventListener("click", async () => {
        copy.textContent = (await copyText(cmd)) ? "Copied" : "Copy failed";
        window.setTimeout(() => (copy.textContent = "Copy"), 1400);
      });
      pullRow.append(h("code", { text: cmd }), copy);
      pullRow.style.display = "";
    } else {
      setDot(dot, saved.enabled ? "ok" : "off");
      statusLine.textContent = saved.enabled
        ? `Ollama is running with ${s.model}. The local AI is on.`
        : `Ollama is running with ${s.model}. Turn the local AI on to use it.`;
    }
  }

  async function refresh() {
    if (validEndpoint(draft.endpoint)) {
      paint({ ...emptyStatus(draft), error: validEndpoint(draft.endpoint) });
      return;
    }
    statusLine.textContent = "Checking Ollama…";
    try {
      paint(await Bridge.localaiStatus(draft));
    } catch (e) {
      paint(null);
      fb.error(e, "Status check failed");
    }
  }

  // ── Test ────────────────────────────────────────────────────────────────────
  const testBtn = h("button", { text: "Test" });
  const testOut = h("div", { class: "localai-test" });
  testBtn.addEventListener("click", async () => {
    testBtn.disabled = true;
    clear(testOut);
    testOut.append(hint("Scanning a made-up sentence… (the first call loads the model and can take a while)"));
    try {
      const r = await Bridge.localaiTest(draft);
      clear(testOut);
      testOut.append(h("div", { class: "hint", text: `Sample: “${r.sample}”` }));
      if (r.error) {
        testOut.append(h("div", { class: "field-error block", text: `${r.error} (after ${(r.ms / 1000).toFixed(1)} s)` }));
      } else if (!r.findings.length) {
        testOut.append(hint(`No findings (${(r.ms / 1000).toFixed(1)} s). Try a larger model.`));
      } else {
        testOut.append(
          h("div", { class: "hint", text: `Found ${r.findings.length} in ${(r.ms / 1000).toFixed(1)} s — each one verified to be in the text:` }),
          h("ul", { class: "steps" }, ...r.findings.map((f) => h("li", {}, h("b", { text: f.label }), ` · ${f.value}`))),
        );
      }
    } catch (e) {
      clear(testOut);
      fb.error(e, "Test failed");
    } finally {
      testBtn.disabled = false;
    }
  });

  const refreshBtn = h("button", { text: "Refresh", onclick: () => void refresh() });

  el.append(
    statusLine,
    pullRow,
    hint("Runs a model in Ollama on this computer to catch what patterns miss — names, street addresses, organisations, internal hostnames — and to explain risky actions in plain English. Nothing leaves your machine: only a local address is accepted."),
    hint("It can only make Zuko stricter. It adds masks and explanations; it never removes a mask, never changes a verdict, and its answers are checked word for word against your text."),
    h("div", { class: "option" },
      toggle(draft.enabled, (v) => { draft.enabled = v; changed(); }, "Use local AI"),
      h("div", {}, h("b", { text: "Use local AI" }), h("span", { class: "hint", text: "Off by default. Everything else in Zuko works the same without it." }))),
    h("div", { class: "row" }, h("label", { text: "Endpoint" }), endpoint, refreshBtn),
    endpointErr,
    h("div", { class: "row" }, h("label", { text: "Model" }), model, h("span", { class: "hint", text: "From Ollama’s installed models" })),
    h("div", { class: "row" }, h("label", { text: "Timeout" }), timeout, h("span", { class: "hint", text: "seconds per call; slower answers are ignored" })),
    sub("Uses"),
    h("div", { class: "options" },
      option("Deep scan prompts", "deepScanPrompts",
        "Scans what you type in the background. A new name is masked from the next request on: the very first send can go out before the scan finishes."),
      option("Wait for AI scan on prompts", "waitForPromptScan",
        "The gateway holds each prompt until the scan is done (up to the timeout), so even the first mention is masked. Adds the model’s delay to every prompt."),
      option("Deep scan documents", "deepScanDocuments",
        "Dropped files are scanned before the sanitized copy is written."),
      option("Explain risky actions", "explainRisk",
        "Adds a short “AI explanation” to approval cards and blocked actions. Zuko’s own verdict and headline stay as they are."),
    ),
    h("div", { class: "row" }, testBtn, h("span", { class: "hint", text: "Scans a sample sentence with a fake name and address. Nothing is saved." })),
    testOut,
    h("div", { class: "row sticky-actions" }, save, dirtyLabel),
    fb.el,
  );
  changed();
  void refresh();
  return el;
}

function emptyStatus(c: LocalAiConfig): LocalAiStatus {
  return {
    enabled: c.enabled, endpoint: c.endpoint, model: c.model, endpointOk: false, reachable: false,
    modelPresent: false, models: [], error: null, hint: null,
  };
}
