// Small building blocks for the settings window. No framework: each helper
// returns an element and keeps its own state.

import { h, clear } from "../views/dom";
import { errorText } from "../core/bridge";

export const GREEN = "#22c55e";
export const RED = "#f4505e";
export const AMBER = "#f5a524";
export const GREY = "#5f646d";

export type DotState = "ok" | "warn" | "err" | "off";

const DOT_COLORS: Record<DotState, string> = { ok: GREEN, warn: AMBER, err: RED, off: GREY };

export function toggle(on: boolean, onChange: (v: boolean) => void, label?: string): HTMLButtonElement {
  const el = h("button", { class: on ? "switch on" : "switch", "aria-pressed": String(on), "aria-label": label });
  el.addEventListener("click", () => {
    const next = !el.classList.contains("on");
    el.classList.toggle("on", next);
    el.setAttribute("aria-pressed", String(next));
    onChange(next);
  });
  return el;
}

export function statusDot(state: boolean | DotState): HTMLElement {
  const s: DotState = state === true ? "ok" : state === false ? "err" : state;
  return h("i", { class: "dot", style: `background:${DOT_COLORS[s]}` });
}

export function setDot(dot: HTMLElement, state: boolean | DotState) {
  const s: DotState = state === true ? "ok" : state === false ? "err" : state;
  dot.style.background = DOT_COLORS[s];
}

export function renderDiff(text: string): HTMLElement {
  const box = h("div", { class: "diff" });
  for (const line of text.split("\n")) {
    const cls = line.startsWith("+") && !line.startsWith("+++") ? "add"
      : line.startsWith("-") && !line.startsWith("---") ? "del" : "ctx";
    box.append(h("div", { class: cls, text: line }));
  }
  return box;
}

export function notice(kind: "ok" | "err" | "warn", text: string): HTMLElement {
  return h("div", { class: `notice ${kind}`, text });
}

/** A feedback slot: show(kind, text) replaces whatever was there. */
export function feedback(): { el: HTMLElement; show(kind: "ok" | "err" | "warn", text: string): void; error(e: unknown, prefix?: string): void; clear(): void } {
  const el = h("div", { class: "feedback" });
  return {
    el,
    show(kind, text) {
      clear(el);
      el.append(notice(kind, text));
    },
    error(e, prefix) {
      clear(el);
      el.append(notice("err", prefix ? `${prefix}: ${errorText(e)}` : errorText(e)));
    },
    clear() {
      clear(el);
    },
  };
}

/** `<section id>` with its heading; the nav links to the id. */
export function section(id: string, title: string, ...children: (Node | null)[]): { el: HTMLElement; head: HTMLElement } {
  const head = h("h2", {}, h("span", { text: title }));
  const el = h("section", { id }, head, ...children);
  return { el, head };
}

export function row(label: string, ...controls: (Node | string | null)[]): HTMLElement {
  return h("div", { class: "row" }, h("label", { text: label }), ...controls);
}

export function sub(title: string, hint?: string): HTMLElement {
  return h("div", { class: "subhead" }, h("h3", { text: title }), hint ? h("span", { class: "hint", text: hint }) : null);
}

export function hint(text: string): HTMLElement {
  return h("div", { class: "hint", text });
}

/** Segmented control. */
export function segmented<T extends string>(
  options: [T, string][],
  value: T,
  onChange: (v: T) => void,
): { el: HTMLElement; set(v: T): void } {
  const buttons = options.map(([v, label]) => {
    const b = h("button", { class: v === value ? "on" : "", text: label });
    b.addEventListener("click", () => {
      set(v);
      onChange(v);
    });
    return b;
  });
  function set(v: T) {
    buttons.forEach((b, i) => b.classList.toggle("on", options[i][0] === v));
  }
  return { el: h("div", { class: "seg" }, ...buttons), set };
}

export function select<T extends string>(options: [T, string][], value: T, onChange: (v: T) => void): HTMLSelectElement {
  const el = h("select", {}) as HTMLSelectElement;
  for (const [v, label] of options) el.append(h("option", { value: v, text: label }));
  el.value = value;
  el.addEventListener("change", () => onChange(el.value as T));
  return el;
}

/**
 * A button that asks once more before doing something destructive: the first
 * click arms it ("Click again to …"), the second runs it, and it disarms by
 * itself after a few seconds.
 */
export function confirmButton(label: string, armedLabel: string, run: () => void | Promise<void>, cls = "danger"): HTMLButtonElement {
  const b = h("button", { class: cls, text: label });
  let armed = false;
  let timer: number | null = null;
  const disarm = () => {
    armed = false;
    b.textContent = label;
    b.classList.remove("armed");
    if (timer != null) window.clearTimeout(timer);
    timer = null;
  };
  b.addEventListener("click", async () => {
    if (!armed) {
      armed = true;
      b.textContent = armedLabel;
      b.classList.add("armed");
      timer = window.setTimeout(disarm, 4000);
      return;
    }
    disarm();
    b.disabled = true;
    try {
      await run();
    } finally {
      b.disabled = false;
    }
  });
  return b;
}

/** A number of tiles: big value, small label. */
export function tiles(items: [value: string, label: string, color?: string][]): HTMLElement {
  return h(
    "div",
    { class: "tiles" },
    ...items.map(([v, l, c]) => h("div", { class: "tile" }, h("b", { style: c ? `color:${c}` : undefined, text: v }), h("span", { text: l }))),
  );
}

// ── Chip list editor ──────────────────────────────────────────────────────────

export interface ChipEditorOptions {
  values: string[];
  placeholder: string;
  /** Returns an error message, or null when the value is fine. */
  validate?: (v: string) => string | null;
  /** Normalises a typed value before validation (trim by default). */
  normalize?: (v: string) => string;
  onChange: (values: string[]) => void;
  mono?: boolean;
}

/**
 * Editable list of strings: chips with ×, plus an input that adds on Enter or
 * comma. Invalid input stays in the field with the reason underneath.
 */
export function chipEditor(opts: ChipEditorOptions): { el: HTMLElement; set(values: string[]): void } {
  let values = [...opts.values];
  const chips = h("div", { class: opts.mono === false ? "chips" : "chips mono" });
  const input = h("input", { type: "text", placeholder: opts.placeholder, spellcheck: "false", autocomplete: "off" }) as HTMLInputElement;
  const add = h("button", { class: "small", text: "Add" });
  const error = h("div", { class: "field-error" });
  const el = h("div", { class: "chip-editor" }, chips, h("div", { class: "chip-input" }, input, add), error);

  function render() {
    clear(chips);
    if (!values.length) chips.append(h("span", { class: "chips-empty", text: "None" }));
    values.forEach((v, i) => {
      const x = h("button", { class: "chip-x", title: `Remove ${v}`, "aria-label": `Remove ${v}`, text: "×" });
      x.addEventListener("click", () => {
        values = values.filter((_, j) => j !== i);
        render();
        opts.onChange([...values]);
      });
      chips.append(h("span", { class: "chip" }, h("span", { text: v }), x));
    });
  }

  function commit() {
    const parts = input.value.split(/[,\n]/).map((p) => (opts.normalize ?? ((s: string) => s.trim()))(p)).filter(Boolean);
    if (!parts.length) return;
    for (const p of parts) {
      if (values.includes(p)) {
        error.textContent = `"${p}" is already in the list.`;
        return;
      }
      const msg = opts.validate?.(p) ?? null;
      if (msg) {
        error.textContent = msg;
        input.classList.add("invalid");
        return;
      }
    }
    values = [...values, ...parts];
    input.value = "";
    error.textContent = "";
    input.classList.remove("invalid");
    render();
    opts.onChange([...values]);
  }

  input.addEventListener("keydown", (e) => {
    if (e.key === "Enter" || e.key === ",") {
      e.preventDefault();
      commit();
    }
  });
  input.addEventListener("input", () => {
    if (!input.value) {
      error.textContent = "";
      input.classList.remove("invalid");
    }
  });
  add.addEventListener("click", commit);
  render();

  return {
    el,
    set(next) {
      values = [...next];
      error.textContent = "";
      input.value = "";
      render();
    },
  };
}

/** "2m ago", "3h ago", "Oct 2". `ms` is unix milliseconds. */
export function relTime(ms: number): string {
  if (!ms) return "—";
  const diff = (Date.now() - ms) / 1000;
  if (diff < 45) return "just now";
  if (diff < 3600) return `${Math.max(1, Math.round(diff / 60))}m ago`;
  if (diff < 86400) return `${Math.floor(diff / 3600)}h ago`;
  if (diff < 86400 * 7) return `${Math.floor(diff / 86400)}d ago`;
  return new Date(ms).toLocaleDateString("en-US", { month: "short", day: "numeric" });
}

export function clockTime(ms: number): string {
  return new Date(ms).toLocaleTimeString("en-GB", { hour: "2-digit", minute: "2-digit" });
}
