// Building blocks every island view shares: the card with its wash, buttons,
// the "who" row and the padded stack that clears Zuko on the left.

import { h, dot } from "./dom";
import { washRGBA, type Wash } from "../core/layout";
import type { AgentTask } from "../core/state";

export function card(wash: Wash, ...children: (Node | string)[]): HTMLElement {
  const el = h("div", { class: wash ? "card wash" : "card" }, ...children);
  if (wash) el.style.setProperty("--wash", washRGBA(wash));
  return el;
}

/** Changes a card's wash in place (the card must have been built with one). */
export function setWash(cardEl: HTMLElement, wash: Wash) {
  cardEl.classList.toggle("wash", wash != null);
  cardEl.style.setProperty("--wash", washRGBA(wash));
}

export function btn(
  label: string,
  kind: "primary" | "secondary",
  onClick: () => void,
  kbd?: string,
): HTMLButtonElement {
  return h(
    "button",
    { class: `btn ${kind}`, onclick: onClick },
    h("span", { class: "btn-label", text: label }),
    kbd ? h("span", { class: "kbd", text: kbd }) : null,
  );
}

/** AgentWho — coloured dot + task name + grey label. */
export function agentWho(task: AgentTask | null, label: string): HTMLElement {
  const row = h("div", { class: "who-row" });
  if (task) {
    row.append(dot(task.color, 8), h("span", { class: "n", text: task.name }));
  }
  row.append(h("span", { class: "who-label", text: label }));
  return row;
}

export function stack(padLeft: number, padRight: number, ...children: Node[]): HTMLElement {
  const el = h("div", { class: "stack" }, ...children);
  el.style.padding = `4px ${padRight}px 4px ${padLeft}px`;
  return el;
}
