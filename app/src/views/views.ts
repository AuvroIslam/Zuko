// Island views — DOM ports of IslandViewContent.swift. Paddings, font sizes,
// colours and wording are copied from the Swift views so both platforms read
// identically. Zuko's own views live next door: approval.ts (the risk card),
// activity.ts (feed + privacy notice) and surfaces.ts (protection cards).

import { h, svg, clear, dot } from "./dom";
import { ICONS } from "./icons";
import { Ticker } from "./ticker";
import { agentWho, btn, card, stack } from "./parts";
import { buildApproval } from "./approval";
import { buildActivity, buildPrivacy } from "./activity";
import { renderSurfaceCard, surfaceCardKey } from "./surfaces";
import { CLAUDE_ID, CLAUDE_NAME, State, type AgentTask, type PillBadge } from "../core/state";
import type { IslandViewName } from "../core/layout";
import { createMiniBot, pruneMiniBots } from "../character/minibots";
import { buildPrompt } from "./chat";
import { buildChoose, buildUpload, buildUploading } from "./upload";

export interface ViewActions {
  setView(v: IslandViewName): void;
  collapse(): void;
  setFocus(id: string): void;
  /** Raises the terminal the focused session runs in (the folder, if it is gone). */
  openTerminal(): void;
  /** Opens the focused session's folder in the editor. */
  openProject(): void;
  /** The ↗ button: opens whatever the focused pill points at. */
  openTarget(): void;
  openUrl(url: string): void;
  /** Answers the pending approval; `elapsedMs` is how long the card was on screen. */
  decide(d: "allow" | "deny", elapsedMs: number): void;
  /** Wakes the frame loop (a view animating from its own tick). */
  keepAlive(): void;
  /** Closes the privacy notice. */
  dismissPrivacy(): void;
  toggleSound(): void;
  setVolume(v: number): void;
  setAutoClose(seconds: number): void;
  openSettingsWindow(): void;
  blip(): void;
}

export interface ViewHost {
  el: HTMLElement;
  sync(): void;
  /** Called when the view becomes active, for views with a text field. */
  focus?(): void;
  /** Called every frame while the view is on screen. */
  tick?(nowMs: number): void;
  /** True while tick() has something to animate: keeps the frame loop awake. */
  animating?(): boolean;
}

// ── Header ────────────────────────────────────────────────────────────────────

export function buildHeader(actions: ViewActions): ViewHost {
  const tabHome = h("button", { class: "tab", title: "Overview", onclick: () => go("overview") }, svg(ICONS.house, 13));
  const tabFeed = h("button", { class: "tab", title: "Activity", onclick: () => go("activity") }, svg(ICONS.list, 13));
  const tabChat = h("button", { class: "tab", title: "Ask", onclick: () => go("prompt") }, svg(ICONS.bubble, 13));
  const tabDrop = h("button", { class: "tab", title: "Drop", onclick: () => go("upload") }, svg(ICONS.plus, 13));

  const gearBtn = h("button", { title: "Settings", onclick: () => go("settings") }, svg(ICONS.gear, 14));
  const soundBtn = h("button", { title: "Mute", onclick: () => actions.toggleSound() }, svg(ICONS.speakerOn, 14));

  function go(v: IslandViewName) {
    actions.blip();
    actions.setView(v);
  }

  const el = h(
    "div",
    { id: "header" },
    h("div", { class: "tabs" }, tabHome, tabFeed, tabChat, tabDrop),
    h("div", { class: "header-actions" }, gearBtn, soundBtn),
  );

  let gearFilled: boolean | null = null;
  let soundOn: boolean | null = null;

  return {
    el,
    sync() {
      const v = State.view;
      tabHome.classList.toggle("on", v === "overview" || v === "empty");
      tabFeed.classList.toggle("on", v === "activity");
      tabChat.classList.toggle("on", v === "prompt");
      tabDrop.classList.toggle("on", v === "upload");
      gearBtn.classList.toggle("on", v === "settings");
      if (gearFilled !== (v === "settings")) {
        gearFilled = v === "settings";
        clear(gearBtn);
        gearBtn.append(svg(gearFilled ? ICONS.gearFill : ICONS.gear, 14));
      }
      if (soundOn !== State.settings.soundEnabled) {
        soundOn = State.settings.soundEnabled;
        clear(soundBtn);
        soundBtn.append(svg(soundOn ? ICONS.speakerOn : ICONS.speakerOff, 14));
      }
      el.style.opacity = v === "confused" ? "0" : "1";
    },
  };
}

// ── Overview ──────────────────────────────────────────────────────────────────

function buildOverview(actions: ViewActions): ViewHost {
  const ticker = new Ticker();
  const who = h("div", { class: "who" });
  const tickerBody = h("div", { class: "card-body" }, who, ticker.el);
  const leftBody = h("div", { class: "left-body" });
  const jump = h(
    "button",
    { class: "icon-btn jump", title: "Open", onclick: () => actions.openTarget() },
    svg(ICONS.arrowUpRight, 8),
  );
  const left = card(null, leftBody, jump);
  const pills = h("div", { class: "pills" });
  const right = card(null, pills);

  const el = h("div", { class: "view overview" },
    h("div", { class: "left" }, left),
    h("div", { class: "right" }, right),
  );

  let pillKey = "";
  let lastFocus: string | null = null;
  let mode: "ticker" | "card" | null = null;
  let cardKey = "";

  return {
    el,
    tick(nowMs: number) {
      if (mode === "ticker") ticker.tick(nowMs);
    },
    sync() {
      const task = State.focusTask;
      if (task?.id !== lastFocus) {
        lastFocus = task?.id ?? null;
        cardKey = "";
        mode = null;
      }

      // A live Claude Code session (or agent) keeps the ticker; a resting
      // surface shows its protection card.
      const live = !!task && !task.isSurface && task.steps.length > 0;
      const sessionActive =
        live || (task?.id === CLAUDE_ID && (task.state !== "idle" || task.steps.length > 0));

      if (task && sessionActive) {
        if (mode !== "ticker") {
          clear(leftBody);
          leftBody.append(tickerBody);
          mode = "ticker";
          cardKey = "";
        }
        clear(who);
        who.append(
          dot(task.color, 7),
          h("span", { class: "name", text: task.name }),
          h("span", { class: "tool", text: task.source === "agent" ? "Agent" : CLAUDE_NAME }),
        );
        if (task.steps.length > 1) {
          who.append(h("span", {
            class: "count",
            text: `${Math.min(task.stepIndex + 1, task.steps.length)}/${task.steps.length}`,
          }));
        }
        ticker.sync(task);
      } else if (task) {
        const key = surfaceCardKey(task);
        if (key !== cardKey) {
          cardKey = key;
          mode = "card";
          clear(leftBody);
          leftBody.append(renderSurfaceCard(task, actions));
        }
      }

      const others = State.otherTasks.slice(0, 4);
      const key = others
        .map((t) => `${t.id}:${t.pillBadge ?? ""}:${t.pillMeta ?? ""}:${t.pillTitle ?? ""}`)
        .join("|");
      if (key !== pillKey) {
        pillKey = key;
        clear(pills);
        for (const t of others) pills.append(buildPill(t, actions));
        pruneMiniBots();
      }
    },
  };
}

const BADGE_COLORS: Record<PillBadge, string> = {
  approval: "#F5A524",
  finished: "#22C55E",
  error: "#F4505E",
  denied: "#F4505E",
  masked: "#2DD4BF",
};

const BADGE_ICONS: Record<PillBadge, { path: string; stroke: number }> = {
  approval: { path: ICONS.bang, stroke: 0 },
  finished: { path: ICONS.check, stroke: 3 },
  error: { path: ICONS.xmark, stroke: 0 },
  denied: { path: ICONS.xmark, stroke: 0 },
  masked: { path: ICONS.lock, stroke: 0 },
};

function buildPill(task: AgentTask, actions: ViewActions): HTMLElement {
  const label = task.id === CLAUDE_ID ? CLAUDE_NAME : task.name;
  const canvas = createMiniBot(task, 24);
  const lbl = h("span", { class: "lbl", text: label });
  const pill = h(
    "div",
    { class: "pill", title: task.pillTitle ?? label, onclick: () => actions.setFocus(task.id) },
    canvas,
    lbl,
  );
  if (task.pillMeta) {
    const off = task.pillMeta === "off" || task.pillMeta === "down";
    pill.append(h("span", { class: off ? "pill-meta off" : "pill-meta", text: task.pillMeta }));
    pill.classList.add("has-meta");
  }
  pill.style.borderColor = `${task.color}24`;
  pill.addEventListener("mouseenter", () => {
    pill.style.background = `${task.color}2e`;
    pill.style.borderColor = `${task.color}8c`;
    pill.style.boxShadow = `0 2px 10px ${task.color}59`;
    lbl.style.color = lighten(task.color, 0.3);
  });
  pill.addEventListener("mouseleave", () => {
    pill.style.background = "";
    pill.style.borderColor = `${task.color}24`;
    pill.style.boxShadow = "";
    lbl.style.color = "";
  });

  if (task.pillBadge) {
    const color = BADGE_COLORS[task.pillBadge];
    const icon = BADGE_ICONS[task.pillBadge];
    const inner = h("i", { style: `background:${color}` }, svg(icon.path, 6, { stroke: icon.stroke }));
    const badge = h("div", { class: "pill-badge" }, inner);
    badge.style.boxShadow = `0 0 4px ${color}99`;
    pill.append(badge);
  }
  return pill;
}

function lighten(hex: string, amount: number): string {
  const v = parseInt(hex.replace("#", ""), 16);
  const c = [(v >> 16) & 255, (v >> 8) & 255, v & 255].map((x) =>
    Math.min(255, Math.round(x + amount * 255)),
  );
  return `rgb(${c[0]},${c[1]},${c[2]})`;
}

// ── Empty ─────────────────────────────────────────────────────────────────────

function buildEmpty(actions: ViewActions): ViewHost {
  const body = h(
    "div",
    { class: "stack", style: "padding:0 18px 0 118px;flex-direction:row;align-items:center;gap:16px" },
    h(
      "div",
      { style: "display:flex;flex-direction:column;gap:5px" },
      h("div", { class: "title", text: "Nothing running right now." }),
      h("div", { class: "sub", text: "Drop a file to sanitize it, or ask me anything." }),
    ),
    h("div", { class: "grow" }),
    btn("Ask Claude", "primary", () => actions.setView("prompt")),
  );
  return { el: h("div", { class: "view" }, card(null, body)), sync() {} };
}

// ── Question ──────────────────────────────────────────────────────────────────

function buildQuestion(): ViewHost {
  const who = h("div");
  const title = h("div", { class: "title" });
  const row = h("div", { class: "actions" },
    h("div", { class: "sub", text: "Answer in your terminal — Zuko can't reply for you." }));
  const el = h("div", { class: "view" }, card("cyan", stack(116, 16, who, title, row)));
  return {
    el,
    sync() {
      clear(who);
      who.append(agentWho(State.focusTask, "Claude Code is asking a question"));
      title.textContent = State.focusTask?.steps.at(-1) ?? "Claude needs an answer.";
    },
  };
}

// ── Error ─────────────────────────────────────────────────────────────────────

function buildError(actions: ViewActions): ViewHost {
  const who = h("div");
  const title = h("div", { class: "title", text: "Session stopped on an error." });
  const detail = h("div", { class: "detail" });
  const row = h("div", { class: "actions" },
    btn("Open terminal", "primary", () => actions.openTerminal()),
    btn("OK", "secondary", () => actions.setView(State.defaultView())),
  );
  const el = h("div", { class: "view" }, card("red", stack(116, 16, who, title, detail, row)));
  return {
    el,
    sync() {
      const task = State.focusTask;
      clear(who);
      who.append(agentWho(task, task?.source === "agent" ? "Agent" : CLAUDE_NAME));
      detail.textContent = task?.steps.at(-1) ?? "No detail available.";
    },
  };
}

// ── Finished ──────────────────────────────────────────────────────────────────

function buildFinished(actions: ViewActions): ViewHost {
  const who = h("div");
  const title = h("div", { class: "title" });
  const row = h("div", { class: "actions" },
    btn("Open terminal", "primary", () => actions.openTerminal()),
    btn("OK", "secondary", () => actions.collapse()),
  );
  const el = h("div", { class: "view" }, card("green", stack(116, 16, who, title, row)));
  return {
    el,
    sync() {
      clear(who);
      who.append(agentWho(State.focusTask, "Claude Code finished"));
      title.textContent = State.focusTask?.steps.at(-1) ?? "Session finished";
    },
  };
}

// ── Confused ──────────────────────────────────────────────────────────────────

function buildConfused(): ViewHost {
  const body = h(
    "div",
    { class: "stack", style: "padding:0 18px 0 128px" },
    h("div", { class: "title", text: "Too many hits at once." }),
    h("div", { class: "sub", text: "Give me a sec — back on watch in three seconds." }),
  );
  return { el: h("div", { class: "view" }, card("pink", body)), sync() {} };
}

// ── Note ──────────────────────────────────────────────────────────────────────

function buildNote(): ViewHost {
  const title = h("div", { class: "title note-text" });
  const el = h("div", { class: "view" }, card(null, h("div", { class: "stack", style: "padding:0 22px 0 108px" }, title)));
  return {
    el,
    sync() {
      title.textContent = State.noteMessage ?? "";
    },
  };
}

// ── In-island settings ────────────────────────────────────────────────────────

function buildSettings(actions: ViewActions): ViewHost {
  const soundSwitch = h("button", { class: "switch", onclick: () => actions.toggleSound() });
  const volume = h("input", {
    type: "range", min: "0", max: "0.2", step: "0.005",
    oninput: (e: Event) => actions.setVolume(Number((e.target as HTMLInputElement).value)),
  }) as HTMLInputElement;
  const autoLabel = h("span", {});
  const segButtons = [10, 15, 30].map((s) =>
    h("button", { onclick: () => actions.setAutoClose(s) }, `${s}s`),
  );
  const hooksBadge = h("span", { class: "status-badge" });
  const gatewayBadge = h("span", { class: "status-badge" });
  const modeBadge = h("span", { class: "status-badge" });

  const rows = h(
    "div",
    { class: "settings-rows" },
    h("div", { class: "settings-row" }, soundSwitch, h("span", { text: "Sound" }), volume),
    h(
      "div",
      { class: "settings-row" },
      svg(ICONS.timer, 12),
      autoLabel,
      h("div", { class: "seg" }, ...segButtons),
    ),
    h(
      "div",
      { class: "settings-row", style: "gap:14px" },
      hooksBadge,
      gatewayBadge,
      modeBadge,
      h("div", { class: "grow" }),
      h("button", {
        class: "link-btn",
        style: "color:#8e939c;font-size:11.5px",
        text: "Settings…",
        onclick: () => actions.openSettingsWindow(),
      }),
    ),
  );

  const el = h("div", { class: "view" },
    card(null, h("div", { class: "stack", style: "padding:14px 16px 14px 84px" }, rows)));

  return {
    el,
    sync() {
      const s = State.settings;
      const p = State.protection;
      soundSwitch.classList.toggle("on", s.soundEnabled);
      volume.value = String(s.soundVolume);
      volume.style.opacity = s.soundEnabled ? "1" : "0.4";
      autoLabel.textContent = `Auto-close · ${Math.round(s.autoCloseInterval)}s`;
      segButtons.forEach((b, i) => b.classList.toggle("on", s.autoCloseInterval === [10, 15, 30][i]));
      const hooks = p?.hooksInstalled ?? s.hooksInstalled;
      clear(hooksBadge);
      hooksBadge.append(dot(hooks ? "#22C55E" : "#F4505E", 6), h("span", { text: "Hooks" }));
      clear(gatewayBadge);
      gatewayBadge.append(
        dot(!p?.gatewayConfigured ? "#5F646D" : p.gatewayRunning ? "#22C55E" : "#F4505E", 6),
        h("span", { text: "Gateway" }),
      );
      clear(modeBadge);
      const monitor = p?.mode === "monitor";
      modeBadge.append(dot(monitor ? "#F5A524" : "#22C55E", 6), h("span", { text: monitor ? "Monitor" : "Enforce" }));
    },
  };
}

// ── Placeholders ──────────────────────────────────────────────────────────────

function buildPlaceholder(title: string, sub: string): ViewHost {
  const body = h(
    "div",
    { class: "stack", style: "padding:0 18px 0 118px" },
    h("div", { class: "title", text: title }),
    h("div", { class: "sub", text: sub }),
  );
  return { el: h("div", { class: "view" }, card(null, body)), sync() {} };
}

// ── Registry ──────────────────────────────────────────────────────────────────

export function buildViews(
  actions: ViewActions,
  onChatHeightChange: () => void,
): Map<IslandViewName, ViewHost> {
  const map = new Map<IslandViewName, ViewHost>();
  map.set("overview", buildOverview(actions));
  map.set("empty", buildEmpty(actions));
  map.set("approval", buildApproval(actions));
  map.set("question", buildQuestion());
  map.set("error", buildError(actions));
  map.set("finished", buildFinished(actions));
  map.set("confused", buildConfused());
  map.set("note", buildNote());
  map.set("settings", buildSettings(actions));
  map.set("activity", buildActivity(actions));
  map.set("privacy", buildPrivacy(actions, onChatHeightChange));
  map.set("prompt", buildPrompt(onChatHeightChange));
  map.set("upload", buildUpload());
  map.set("uploading", buildUploading());
  map.set("choose", buildChoose(actions));
  // Not in the Windows v1: sending a file by email, window attach + web result.
  map.set("mail", buildPlaceholder("Sending by email isn't in this version.", ""));
  map.set("searching", buildPlaceholder("Claude is searching…", ""));
  map.set("result", buildPlaceholder("Result", ""));
  return map;
}
