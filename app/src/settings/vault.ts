// Vault: every value Zuko has replaced with a placeholder. Real values stay in
// Rust; this table only shows previews, and Reveal is an explicit click that
// hides itself again after 10 seconds. Inspect shows a few non-sensitive facts
// (brand, country, vendor, expiry...) computed in Rust; Copy value puts the value on
// the clipboard from Rust, so it never reaches this window, and Rust clears it again
// after 30 seconds.

import { h, clear } from "../views/dom";
import { Bridge, VAULT_KINDS, type EntryView } from "../core/bridge";
import { placeholder, privacySourceLabel } from "../views/format";
import { confirmButton, feedback, hint, relTime, section, select } from "./ui";

const REVEAL_MS = 10_000;
const COPY_CLEAR_S = 30;

export function vaultSection(initial: EntryView[] | null): HTMLElement {
  const { el, head } = section("vault", "Vault");
  const count = h("span", { class: "count" });
  head.append(count);

  let entries = initial ?? [];
  const tbody = h("tbody");
  const table = h("table", { class: "table vault" },
    h("thead", {}, h("tr", {},
      h("th", { text: "Placeholder · label" }), h("th", { text: "Preview" }),
      h("th", { text: "Source" }), h("th", { class: "num", text: "Hits" }), h("th", { text: "Used" }), h("th"))),
    tbody);
  const empty = hint("Nothing in the vault yet. Values appear here the first time Zuko masks them.");
  const fb = feedback();
  /** Keys revealed right now → their hide timers. */
  const revealed = new Map<string, number>();

  async function reload() {
    entries = (await Bridge.vaultList()) ?? [];
    render();
  }

  function render() {
    for (const t of revealed.values()) window.clearTimeout(t);
    revealed.clear();
    count.textContent = entries.length ? String(entries.length) : "";
    clear(tbody);
    table.style.display = entries.length ? "" : "none";
    empty.style.display = entries.length ? "none" : "";
    forgetAll.style.display = entries.length ? "" : "none";
    for (const e of entries) tbody.append(...entryRows(e));
  }

  /** The entry's row, plus a hidden row below it for the Inspect panel. */
  function entryRows(e: EntryView): HTMLElement[] {
    const preview = h("code", { class: "preview", text: e.preview });
    const reveal = h("button", { class: "small", text: "Reveal" });
    const copy = h("button", { class: "small", text: "Copy value", title: `Copies to the clipboard; cleared after ${COPY_CLEAR_S} s` });
    const inspect = h("button", { class: "small", text: "Inspect" });
    const forget = h("button", { class: "small danger", text: "Forget" });
    const facts = h("dl", { class: "facts" });
    const panel = h("tr", { class: "inspect-row" },
      h("td", { colspan: "6" }, h("div", { class: "inspect" }, facts)));
    panel.hidden = true;

    inspect.addEventListener("click", async () => {
      if (!panel.hidden) {
        panel.hidden = true;
        inspect.textContent = "Inspect";
        return;
      }
      try {
        const rows = await Bridge.vaultInsights(e.key);
        if (rows == null) {
          fb.show("warn", `${placeholder(e.key)} is no longer in the vault.`);
          return;
        }
        clear(facts);
        for (const r of rows) facts.append(h("dt", { text: r.label }), h("dd", { text: r.text }));
        panel.hidden = false;
        inspect.textContent = "Close";
      } catch (err) {
        fb.error(err, "Couldn't inspect");
      }
    });

    copy.addEventListener("click", async () => {
      try {
        if (!(await Bridge.vaultCopy(e.key))) {
          fb.show("warn", `${placeholder(e.key)} is no longer in the vault.`);
          return;
        }
        fb.show("ok", `Copied ${placeholder(e.key)} — clipboard clears in ${COPY_CLEAR_S} s`);
      } catch (err) {
        fb.error(err, "Couldn't copy");
      }
    });

    const hide = () => {
      const t = revealed.get(e.key);
      if (t != null) window.clearTimeout(t);
      revealed.delete(e.key);
      preview.textContent = e.preview;
      preview.classList.remove("revealed");
      reveal.textContent = "Reveal";
    };

    reveal.addEventListener("click", async () => {
      if (revealed.has(e.key)) {
        hide();
        return;
      }
      try {
        const value = await Bridge.vaultReveal(e.key);
        if (value == null) {
          fb.show("warn", `${placeholder(e.key)} is no longer in the vault.`);
          return;
        }
        preview.textContent = value;
        preview.classList.add("revealed");
        reveal.textContent = "Hide";
        revealed.set(e.key, window.setTimeout(hide, REVEAL_MS));
      } catch (err) {
        fb.error(err, "Couldn't reveal");
      }
    });

    forget.addEventListener("click", async () => {
      try {
        await Bridge.vaultForget(e.key);
        entries = entries.filter((x) => x.key !== e.key);
        render();
        fb.show("ok", `Forgot ${placeholder(e.key)}. It will get a new placeholder if it shows up again.`);
      } catch (err) {
        fb.error(err, "Couldn't forget");
      }
    });

    const row = h("tr", {},
      h("td", { class: "what" },
        h("code", { class: "ph", text: placeholder(e.key) }),
        h("div", { class: "hint" }, h("span", { class: "lbl", text: e.label }), e.hint ? ` · ${e.hint}` : "")),
      h("td", {}, preview),
      h("td", { class: "dim nowrap", text: privacySourceLabel(e.source === "manual" ? "Added" : e.source) }),
      h("td", { class: "num", text: String(e.hits) }),
      h("td", { class: "dim nowrap", text: relTime(e.lastUsed * 1000) }),
      h("td", { class: "actions" }, h("div", { class: "btns" }, inspect, reveal, copy, forget)),
    );
    return [row, panel];
  }

  const forgetAll = confirmButton("Forget all", "Click again to forget everything", async () => {
    try {
      await Bridge.vaultClear();
      entries = [];
      render();
      fb.show("ok", "Vault cleared. Placeholders already sent can no longer be restored.");
    } catch (err) {
      fb.error(err, "Couldn't clear");
    }
  });

  // Add a value by hand: it is masked from now on, everywhere.
  const value = h("input", { type: "password", placeholder: "Value to protect", autocomplete: "off", spellcheck: "false", style: "flex:1 1 130px;min-width:0" }) as HTMLInputElement;
  let kind = "SECRET";
  const kindSelect = select(VAULT_KINDS.map((k) => [k, k] as [string, string]), kind, (v) => { kind = v; });
  const label = h("input", { type: "text", placeholder: "Label (optional)", style: "flex:1 1 90px;min-width:0" }) as HTMLInputElement;
  const add = h("button", { class: "primary", text: "Add value" });
  add.addEventListener("click", async () => {
    const v = value.value;
    if (!v.trim()) {
      fb.show("warn", "Type or paste the value first.");
      return;
    }
    try {
      const key = await Bridge.vaultAdd(v, kind, label.value.trim());
      value.value = "";
      label.value = "";
      await reload();
      fb.show("ok", `Added as ${placeholder(key)}. It is masked everywhere from now on.`);
    } catch (err) {
      fb.error(err, "Couldn't add");
    }
  });
  value.addEventListener("keydown", (e) => {
    if (e.key === "Enter") add.click();
  });

  el.append(
    hint("Each value Zuko masks gets a stable placeholder. The model only ever sees the placeholder; your machine fills the real value back in."),
    h("div", { class: "table-wrap" }, table),
    empty,
    h("div", { class: "row add-row" }, value, kindSelect, label, add),
    h("div", { class: "row" }, h("div", { class: "spacer" }), forgetAll),
    fb.el,
  );
  render();
  return el;
}
