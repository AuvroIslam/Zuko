// Protection surface cards, shown in the overview's left card when a pill is
// focused and nothing live is running on it: Claude Code, Gateway, Browser and
// Policy. They read only from State.protection and State.activity.

import { h, svg, dot } from "./dom";
import { activityText, fmtCount, timeAgo, verdictMeta } from "./format";
import type { ViewActions } from "./views";
import {
  BROWSER_ID, CLAUDE_ID, GATEWAY_ID, POLICY_ID, State, type AgentTask,
} from "../core/state";
import type { ActivityItem, ProtectionStatus } from "../core/bridge";

const GREEN = "#22C55E";
const RED = "#F4505E";
const AMBER = "#F5A524";
const GREY = "#5F646D";

function head(color: string, name: string, kind: string): HTMLElement {
  return h("div", { class: "sf-head" }, dot(color, 7), h("b", { text: name }), h("span", { text: kind }));
}

function status(color: string, text: string, title?: string): HTMLElement {
  return h("div", { class: "sf-status", title: title ?? text }, dot(color, 5), h("span", { text }));
}

/** A row of big numbers with a small label under each. */
function tiles(...items: [value: number | string, label: string, color?: string][]): HTMLElement {
  return h(
    "div",
    { class: "sf-tiles" },
    ...items.map(([value, label, color]) =>
      h(
        "div",
        { class: "sf-tile" },
        h("b", { style: color ? `color:${color}` : undefined, text: typeof value === "number" ? fmtCount(value) : value }),
        h("span", { text: label }),
      ),
    ),
  );
}

/** One activity line: verdict icon, text, time. */
function activityRow(item: ActivityItem): HTMLElement {
  const v = verdictMeta(item.verdict);
  return h(
    "div",
    { class: "sf-row", title: `${v.label} · ${activityText(item)}` },
    h("i", { class: "sf-row-icon", style: `color:${v.color}` }, svg(v.icon, 8, v.stroke ? { stroke: 3 } : {})),
    h("span", { class: "sf-row-text", text: activityText(item) }),
    h("span", { class: "sf-row-ago", text: timeAgo(item.ts) }),
  );
}

function links(...items: [label: string, onClick: () => void][]): HTMLElement {
  return h(
    "div",
    { class: "sf-actions" },
    ...items.map(([label, onClick]) => h("button", { class: "link-btn", text: label, onclick: onClick })),
  );
}

const lastWhere = (pred: (a: ActivityItem) => boolean) => State.activity.find(pred) ?? null;

// ── Cards ─────────────────────────────────────────────────────────────────────

function claudeCard(task: AgentTask, p: ProtectionStatus | null, actions: ViewActions): HTMLElement {
  const installed = p?.hooksInstalled ?? State.settings.hooksInstalled;
  const line = !p
    ? status(GREY, "Waiting for Zuko…")
    : !installed
      ? status(RED, "Hooks not installed")
      : !p.hookReady
        ? status(AMBER, "Relay missing — reinstall Zuko")
        : status(p.mode === "monitor" ? AMBER : GREEN, p.mode === "monitor" ? "Watching · monitor mode" : "Guarding every tool call");
  return h(
    "div",
    { class: "sf-card" },
    // The counters are today's (back to 0 at midnight); three "… today" labels do not fit.
    head(task.color, task.name, "Firewall · today"),
    line,
    tiles(
      [p?.askedTotal ?? 0, "asked", AMBER],
      [p?.blockedTotal ?? 0, "blocked", RED],
      [p?.autoAllowedTotal ?? 0, "auto-allowed"],
    ),
    links(
      ["Open VS Code", () => actions.openProject()],
      [installed ? "Settings…" : "Install…", () => actions.openSettingsWindow()],
    ),
  );
}

function gatewayCard(task: AgentTask, p: ProtectionStatus | null, actions: ViewActions): HTMLElement {
  const line = !p
    ? status(GREY, "Waiting for Zuko…")
    : !p.gatewayConfigured
      ? status(GREY, "Off · hooks-only mode")
      : p.gatewayRunning
        ? status(GREEN, `On · 127.0.0.1:${p.gatewayPort}`)
        : status(RED, "Down — Claude Code can't reach the API", "The gateway is configured in Claude Code but isn't running.");
  return h(
    "div",
    { class: "sf-card" },
    head(task.color, task.name, "Privacy proxy"),
    line,
    tiles(
      [p?.maskedTotal ?? 0, "masked today", "#2DD4BF"],
      [p?.vaultSize ?? 0, "in vault"],
    ),
    links([p?.gatewayConfigured ? "Settings…" : "Turn on…", () => actions.openSettingsWindow()]),
  );
}

function browserCard(task: AgentTask, p: ProtectionStatus | null, actions: ViewActions): HTMLElement {
  const last = lastWhere((a) => a.event === "Browser");
  const line = !p
    ? status(GREY, "Waiting for Zuko…")
    : p.extensionConnected
      ? status(GREEN, "Extension connected")
      : status(GREY, "Extension not connected");
  return h(
    "div",
    { class: "sf-card" },
    head(task.color, task.name, "Web chats"),
    line,
    last
      ? h("div", { class: "sf-rows" }, activityRow(last))
      : h("div", { class: "sf-empty", text: "ChatGPT, claude.ai and DeepSeek prompts are masked before they're sent." }),
    links([p?.extensionConnected ? "Settings…" : "Set it up…", () => actions.openSettingsWindow()]),
  );
}

function policyCard(task: AgentTask, p: ProtectionStatus | null, actions: ViewActions): HTMLElement {
  const last = lastWhere((a) => verdictMeta(a.verdict).group === "blocked");
  const monitor = p?.mode === "monitor";
  return h(
    "div",
    { class: "sf-card" },
    head(task.color, task.name, monitor ? "Monitor mode · today" : "Enforced · today"),
    tiles(
      [p?.blockedTotal ?? 0, monitor ? "would block" : "blocked", RED],
      [p?.askedTotal ?? 0, "asked", AMBER],
      [p?.autoAllowedTotal ?? 0, "auto-allowed"],
    ),
    last
      ? h("div", { class: "sf-rows" }, activityRow(last))
      : h("div", { class: "sf-empty", text: "Nothing blocked yet." }),
    links(["Edit policy…", () => actions.openSettingsWindow()]),
  );
}

function agentCard(task: AgentTask): HTMLElement {
  return h(
    "div",
    { class: "sf-card" },
    head(task.color, task.name, "Agent"),
    status(GREY, "Waiting for activity"),
  );
}

/** Everything the card shows, so the overview only rebuilds it when it changed. */
export function surfaceCardKey(task: AgentTask): string {
  const p = State.protection;
  const lastId = State.activity[0]?.id ?? "";
  return JSON.stringify([
    task.id, task.name, task.state, p, task.id === BROWSER_ID || task.id === POLICY_ID ? lastId : "",
    State.settings.hooksInstalled,
  ]);
}

export function renderSurfaceCard(task: AgentTask, actions: ViewActions): HTMLElement {
  const p = State.protection;
  switch (task.id) {
    case CLAUDE_ID:
      return claudeCard(task, p, actions);
    case GATEWAY_ID:
      return gatewayCard(task, p, actions);
    case BROWSER_ID:
      return browserCard(task, p, actions);
    case POLICY_ID:
      return policyCard(task, p, actions);
    default:
      return agentCard(task);
  }
}
