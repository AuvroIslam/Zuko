// Zuko's island views: the live activity feed and the privacy notice.
//
// The feed reads State.activity (newest first, ring buffer of ACTIVITY_LIMIT)
// and only rebuilds its rows when the list or the filter changed. The privacy
// notice shows State.privacyNotice: what was masked before it left, or a
// blocked prompt with a masked copy ready to paste.

import { h, svg, clear, copyText } from "./dom";
import { ICONS } from "./icons";
import { btn, card, setWash, stack } from "./parts";
import {
  activityText, distinctLabels, fmtCount, placeholder, privacySourceLabel, privacyText,
  timeAgo, verdictMeta, type VerdictGroup,
} from "./format";
import type { ViewActions, ViewHost } from "./views";
import { tierColor } from "../core/layout";
import { State } from "../core/state";
import type { ActivityItem } from "../core/bridge";

// ── Activity feed ─────────────────────────────────────────────────────────────

type Filter = "all" | VerdictGroup;

const FILTERS: [Filter, string][] = [
  ["all", "All"],
  ["blocked", "Blocked"],
  ["asked", "Asked"],
  ["privacy", "Privacy"],
];

/** Rows rendered at most; the rest is one scroll away in the settings window. */
const MAX_ROWS = 60;

function feedRow(item: ActivityItem): HTMLElement {
  const v = verdictMeta(item.verdict);
  const text = activityText(item);
  const where = [item.project, item.tool].filter(Boolean).join(" · ");
  const tier = item.tier !== "low" && v.group !== "privacy"
    ? h("i", { class: "feed-tier", style: `background:${tierColor(item.tier)}`, title: `${item.tier} risk · ${item.score}` })
    : null;
  return h(
    "div",
    {
      class: "feed-row",
      title: `${v.label} · ${text}${where ? `\n${where}` : ""}${item.aiExplanation ? `\nAI explanation: ${item.aiExplanation}` : ""}`,
    },
    h("i", { class: "feed-icon", style: `color:${v.color}` }, svg(v.icon, 9, v.stroke ? { stroke: 3 } : {})),
    h("span", { class: "feed-text", text }),
    tier,
    h("span", { class: "feed-where", text: where }),
    h("span", { class: "feed-ago", text: timeAgo(item.ts) }),
  );
}

export function buildActivity(actions: ViewActions): ViewHost {
  let filter: Filter = "all";
  let listKey = "";

  const chips = FILTERS.map(([f, label]) =>
    h("button", {
      class: "feed-filter",
      onclick: () => {
        filter = f;
        actions.blip();
        State.notify();
      },
    }, label),
  );
  const count = h("span", { class: "feed-count" });
  const head = h("div", { class: "feed-head" },
    h("b", { text: "Activity" }), count, h("div", { class: "grow" }), ...chips);
  const list = h("div", { class: "feed-list" });
  const empty = h("div", { class: "feed-empty" });

  const statBlocked = h("b", { style: "color:#F4505E" });
  const statAsked = h("b", { style: "color:#F5A524" });
  const statMasked = h("b", { style: "color:#2DD4BF" });
  const stats = h("div", { class: "feed-stats" },
    h("div", {}, statBlocked, h("span", { text: "blocked" })),
    h("div", {}, statAsked, h("span", { text: "asked" })),
    h("div", {}, statMasked, h("span", { text: "masked" })),
    h("button", { class: "link-btn feed-more", text: "More…", onclick: () => actions.openSettingsWindow() }),
  );

  const body = h("div", { class: "feed" }, head, list, empty);
  const el = h("div", { class: "view" }, card(null, stats, body));

  return {
    el,
    sync() {
      const p = State.protection;
      statBlocked.textContent = fmtCount(p?.blockedTotal ?? 0);
      statAsked.textContent = fmtCount(p?.askedTotal ?? 0);
      statMasked.textContent = fmtCount(p?.maskedTotal ?? 0);
      chips.forEach((c, i) => c.classList.toggle("on", FILTERS[i][0] === filter));

      const items = filter === "all"
        ? State.activity
        : State.activity.filter((a) => verdictMeta(a.verdict).group === filter);
      const key = `${filter}|${items.length}|${items[0]?.id ?? ""}|${Math.floor(Date.now() / 30_000)}`;
      if (key === listKey) return;
      listKey = key;

      count.textContent = State.activity.length ? fmtCount(State.activity.length) : "";
      clear(list);
      for (const item of items.slice(0, MAX_ROWS)) list.append(feedRow(item));
      const none = items.length === 0;
      empty.style.display = none ? "" : "none";
      list.style.display = none ? "none" : "";
      empty.textContent = State.activity.length === 0
        ? "Nothing yet. Every tool call, approval and masked value shows up here."
        : "Nothing in this filter.";
    },
  };
}

// ── Privacy notice ────────────────────────────────────────────────────────────

export function buildPrivacy(actions: ViewActions): ViewHost {
  const icon = h("i", { class: "pv-icon" }, svg(ICONS.shield, 10));
  const source = h("span", { class: "n" });
  const sub = h("span", { class: "who-label" });
  const who = h("div", { class: "who-row" }, icon, source, sub);
  const title = h("div", { class: "title pv-title" });
  const labels = h("div", { class: "pv-labels" });
  const masked = h("div", { class: "code pv-masked" });

  // Built once; sync() only relabels and hides.
  const copyLabel = h("span", { class: "btn-label", text: "Copy masked prompt" });
  const copy = h("button", { class: "btn primary" }, svg(ICONS.copy, 11), copyLabel);
  const ok = btn("OK", "secondary", () => actions.dismissPrivacy());
  const feed = h("button", { class: "link-btn pv-link", text: "Activity…", onclick: () => actions.setView("activity") });
  const row = h("div", { class: "actions" }, copy, ok, h("div", { class: "grow" }), feed);

  const cardEl = card("cyan", stack(116, 18, who, title, labels, masked, row));
  const el = h("div", { class: "view" }, cardEl);

  let copiedTimer: number | null = null;
  copy.addEventListener("click", async () => {
    const text = State.privacyNotice?.maskedPrompt;
    if (!text) return;
    const done = await copyText(text);
    copyLabel.textContent = done ? "Copied — paste it back" : "Copy failed";
    if (copiedTimer != null) window.clearTimeout(copiedTimer);
    copiedTimer = window.setTimeout(() => {
      copiedTimer = null;
      copyLabel.textContent = "Copy masked prompt";
    }, 2200);
  });

  let shown: unknown = null;

  return {
    el,
    sync() {
      const e = State.privacyNotice;
      if (e === shown) return;
      shown = e;
      if (!e) return;
      const blocked = e.direction === "blocked_prompt";
      const t = privacyText(e);
      setWash(cardEl, blocked ? "red" : e.direction === "rehydrated" ? "indigo" : "cyan");
      icon.style.color = blocked ? "#F4505E" : e.direction === "rehydrated" ? "#818CF8" : "#2DD4BF";
      source.textContent = privacySourceLabel(e.source);
      sub.textContent = blocked ? "prompt held back" : e.direction === "rehydrated" ? "restored locally" : "privacy shield";
      title.textContent = blocked ? "Blocked a prompt containing a secret — masked copy ready" : `${t.title}:`;

      clear(labels);
      const names = distinctLabels(e);
      names.slice(0, 4).forEach((name) => {
        const i = e.labels.findIndex((l) => l.trim() === name);
        const key = i >= 0 ? e.keys[i] : undefined;
        labels.append(h("span", { class: "pv-chip", title: key ? placeholder(key) : name },
          h("span", { text: name }), key ? h("em", { text: placeholder(key) }) : null));
      });
      if (names.length > 4) labels.append(h("span", { class: "pv-chip more", text: `+${names.length - 4}` }));
      if (!blocked && !names.length) labels.append(h("span", { class: "pv-chip", text: t.detail }));
      labels.style.display = labels.childElementCount ? "" : "none";
      labels.classList.toggle("blocked", blocked);

      masked.textContent = e.maskedPrompt ?? "";
      masked.style.display = blocked && e.maskedPrompt ? "" : "none";
      copy.style.display = blocked && e.maskedPrompt ? "" : "none";
      copyLabel.textContent = "Copy masked prompt";
      ok.className = blocked && e.maskedPrompt ? "btn secondary" : "btn primary";
    },
  };
}
