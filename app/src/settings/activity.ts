// Activity: the recent decisions (newest first, live), filterable by verdict,
// plus the tamper-evident audit log check.

import { h, svg, clear } from "../views/dom";
import { Bridge, onEvent, type ActivityItem } from "../core/bridge";
import { activityText, fmtCount, verdictMeta, type VerdictGroup } from "../views/format";
import { tierColor } from "../core/layout";
import { clockTime, feedback, hint, relTime, section, segmented } from "./ui";

type Filter = "all" | VerdictGroup;

const LIMIT = 200;

export function activitySection(initial: ActivityItem[] | null, auditPath: string): HTMLElement {
  const { el, head } = section("activity", "Activity");
  const count = h("span", { class: "count" });
  head.append(count);

  let items = initial ?? [];
  let filter: Filter = "all";
  const list = h("div", { class: "act-list" });
  const empty = hint("Nothing yet. Tool calls, approvals and masked values show up here as they happen.");

  const filters = segmented<Filter>(
    [["all", "All"], ["blocked", "Blocked"], ["asked", "Asked"], ["allowed", "Allowed"], ["privacy", "Privacy"]],
    filter,
    (v) => {
      filter = v;
      render();
    },
  );

  /**
   * Opens the edited file in VS Code, in the window that has the session folder open (or
   * shows its folder); only on Write/Edit rows.
   */
  function openFile(path: string, cwd: string | undefined): HTMLElement {
    return h("button", { class: "small", text: "Open file", title: path, onclick: () => void Bridge.openFile(path, cwd) });
  }

  function itemRow(a: ActivityItem): HTMLElement {
    const v = verdictMeta(a.verdict);
    const where = [a.project, a.event, a.tool].filter(Boolean).join(" · ");
    const rules = a.rules.length ? h("span", { class: "rules", text: a.rules.join(", ") }) : null;
    return h("div", { class: "act-row", title: a.summary },
      h("span", { class: "act-time", title: new Date(a.ts).toLocaleString(), text: clockTime(a.ts) }),
      h("span", { class: "badge", style: `color:${v.color};background:${v.color}1f` },
        svg(v.icon, 9, v.stroke ? { stroke: 3 } : {}), h("span", { text: v.label })),
      h("div", { class: "act-main" },
        h("div", { class: "act-text", text: activityText(a) }),
        a.aiExplanation
          ? h("div", { class: "act-ai" }, h("i", { text: "AI explanation" }), h("span", { text: a.aiExplanation }))
          : null,
        h("div", { class: "act-meta" },
          v.group !== "privacy" ? h("i", { class: "dot", style: `width:6px;height:6px;background:${tierColor(a.tier)}` }) : null,
          v.group !== "privacy" ? h("span", { text: `${a.tier} ${a.score}` }) : null,
          h("span", { text: where }),
          rules,
          a.path ? openFile(a.path, a.cwd) : null,
          a.keys.length ? h("span", { class: "keys", text: a.keys.map((k) => `{{${k}}}`).join(" ") }) : null),
      ),
      h("span", { class: "act-ago", text: relTime(a.ts) }),
    );
  }

  function render() {
    count.textContent = items.length ? fmtCount(items.length) : "";
    const shown = filter === "all" ? items : items.filter((a) => verdictMeta(a.verdict).group === filter);
    clear(list);
    for (const a of shown.slice(0, LIMIT)) list.append(itemRow(a));
    list.style.display = shown.length ? "" : "none";
    empty.style.display = shown.length ? "none" : "";
    empty.textContent = items.length ? "Nothing matches this filter." : "Nothing yet. Tool calls, approvals and masked values show up here as they happen.";
  }

  // Audit log
  const verdict = h("span", { class: "audit-verdict" });
  const fb = feedback();
  const verify = h("button", { text: "Verify audit log" });
  verify.addEventListener("click", async () => {
    verify.disabled = true;
    verdict.className = "audit-verdict";
    verdict.textContent = "Checking…";
    fb.clear();
    try {
      const r = await Bridge.auditVerify();
      verdict.className = r.ok ? "audit-verdict pass" : "audit-verdict fail";
      verdict.textContent = r.ok
        ? `PASS · ${fmtCount(r.count)} receipts, chain intact`
        : `FAIL · ${r.error ?? "the hash chain is broken"}`;
    } catch (e) {
      verdict.textContent = "";
      fb.error(e, "Couldn't verify");
    } finally {
      verify.disabled = false;
    }
  });
  const open = h("button", { text: "Open folder", onclick: () => void Bridge.auditOpenFolder() });

  el.append(
    h("div", { class: "row" }, filters.el),
    list,
    empty,
    h("div", { class: "audit" },
      h("div", { class: "row" }, verify, open, verdict),
      hint("Every decision is appended to a hash-chained log that never contains secret values. Verify recomputes the chain to prove nothing was edited or removed."),
      auditPath ? h("span", { class: "path", text: auditPath }) : null),
    fb.el,
  );
  render();

  void onEvent("activity", (a) => {
    if (items.some((x) => x.id === a.id)) return;
    items = [a, ...items].slice(0, LIMIT);
    render();
  });
  // A local AI explanation for an item already listed (display only).
  void onEvent("ai-explain", (e) => {
    const item = e.activityId ? items.find((x) => x.id === e.activityId) : undefined;
    if (!item) return;
    item.aiExplanation = e.text;
    render();
  });

  return el;
}
