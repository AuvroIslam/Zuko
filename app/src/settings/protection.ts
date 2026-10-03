// Protection: what Zuko has written into ~/.claude/settings.json (hooks,
// gateway env, deny rules), whether it is all working, and the counters.
// Nothing is written without a reviewed diff and the fingerprint that came
// with it.

import { h, clear } from "../views/dom";
import { Bridge, IS_MOCK, onEvent, type InstallOptions, type ProtectionStatus } from "../core/bridge";
import { fmtCount, maskGatewayUrl } from "../views/format";
import { AMBER, feedback, hint, renderDiff, section, setDot, statusDot, tiles, toggle, type DotState } from "./ui";

const EMPTY: ProtectionStatus = {
  hooksInstalled: false, hookReady: false, gatewayConfigured: false, gatewayRunning: false,
  gatewayUrl: null, gatewayPort: 0, denyRulesInstalled: false, mode: "enforce", vaultSize: 0,
  maskedTotal: 0, blockedTotal: 0, askedTotal: 0, autoAllowedTotal: 0, extensionConnected: false,
  policyPath: "", auditPath: "",
};

function overall(p: ProtectionStatus): DotState {
  if (!p.hookReady || !p.hooksInstalled) return "err";
  if (p.gatewayConfigured && !p.gatewayRunning) return "err";
  if (!p.gatewayConfigured || p.mode === "monitor") return "warn";
  return "ok";
}

export function protectionSection(initial: ProtectionStatus | null): HTMLElement {
  let status = initial ?? EMPTY;
  const { el, head } = section("protection", "Protection");
  const headDot = statusDot(overall(status));
  head.prepend(headDot);

  const statusList = h("div", { class: "status-list" });
  const counters = h("div", {});
  const body = h("div", { class: "stack-12" });
  const fb = feedback();

  const options: InstallOptions = {
    hooks: status.hooksInstalled || !initial,
    gateway: status.gatewayConfigured,
    denyRules: status.denyRulesInstalled,
  };

  function statusRow(state: DotState, label: string, detail: string, title?: string): HTMLElement {
    return h("div", { class: "status-row", title: title ?? "" }, statusDot(state), h("b", { text: label }), h("span", { text: detail }));
  }

  function renderStatus() {
    const p = status;
    setDot(headDot, overall(p));
    clear(statusList);
    statusList.append(
      statusRow(p.hooksInstalled ? "ok" : "err", "Hooks", p.hooksInstalled ? "Installed in Claude Code" : "Not installed"),
      statusRow(p.hookReady ? "ok" : "err", "Relay", p.hookReady ? "zuko-hook.exe in place" : "zuko-hook.exe missing — reinstall Zuko"),
      statusRow(
        !p.gatewayConfigured ? "off" : p.gatewayRunning ? "ok" : "err",
        "Gateway",
        !p.gatewayConfigured
          ? "Not configured · hooks-only mode"
          : p.gatewayRunning
            ? `Running · ${maskGatewayUrl(p.gatewayUrl)}`
            : "Configured but not running — Claude Code can't reach the API",
      ),
      statusRow(p.denyRulesInstalled ? "ok" : "off", "Deny rules",
        p.denyRulesInstalled ? "Mirrored into permissions.deny" : "Not mirrored"),
      statusRow(p.mode === "monitor" ? "warn" : "ok", "Mode",
        p.mode === "monitor" ? "Monitor — logs everything, blocks nothing" : "Enforce"),
    );
    clear(counters);
    counters.append(
      tiles([
        [fmtCount(p.maskedTotal), "masked today", "#2dd4bf"],
        [fmtCount(p.blockedTotal), "blocked today", "#f4505e"],
        [fmtCount(p.askedTotal), "asked today", AMBER],
        [fmtCount(p.autoAllowedTotal), "auto-allowed today"],
        [fmtCount(p.vaultSize), "in vault"],
      ]),
      hint("Today's counts start at midnight and are read back from the audit log, so a restart keeps them. They match the Activity filters for today."),
    );
  }

  function draw() {
    clear(body);
    fb.clear();
    const opt = (key: keyof InstallOptions, label: string, detail: string) =>
      h("div", { class: "option" },
        toggle(options[key], (v) => { options[key] = v; }, label),
        h("div", {}, h("b", { text: label }), h("span", { class: "hint", text: detail })),
      );
    const review = h("button", { class: "primary", text: "Review changes…" });
    if (!status.hookReady && initial) {
      review.disabled = true;
      review.title = "The relay isn't installed yet.";
    }
    review.addEventListener("click", () => void preview());
    body.append(
      h("div", { class: "options" },
        opt("hooks", "Hooks", "Policy, risk scoring and approvals on every tool call."),
        opt("gateway", "Gateway", "Route Claude Code through Zuko so secrets are masked before they leave."),
        opt("denyRules", "Deny rules", "Copy blocked paths and domains into Claude Code's own deny list — enforced even if Zuko is off."),
      ),
      hint("Gateway mode masks everything Claude Code sends — prompts, @files and tool output. Hooks-only mode still enforces your policy and approvals, but blocks prompts with secrets instead of masking them. Restart Claude Code after turning the gateway on or off."),
      h("div", { class: "row" }, review),
    );
  }

  async function preview() {
    fb.clear();
    let p;
    try {
      p = await Bridge.protectionPreview({ ...options });
    } catch (e) {
      // An unreadable settings.json stops here rather than being overwritten.
      fb.error(e, "Can't prepare the change");
      return;
    }
    const chosen = { ...options };
    clear(body);
    const apply = h("button", { class: "primary", text: "Apply" });
    const cancel = h("button", { text: "Cancel", onclick: () => draw() });
    apply.addEventListener("click", async () => {
      apply.disabled = true;
      try {
        const backup = await Bridge.protectionApply(chosen, p.fingerprint);
        draw();
        fb.show("ok", `Applied. Backup saved as ${backup}.${chosen.gateway !== status.gatewayConfigured ? " Restart Claude Code to pick up the gateway change." : ""}`);
        const fresh = await Bridge.protectionStatus();
        if (fresh) {
          status = fresh;
          renderStatus();
        }
      } catch (e) {
        apply.disabled = false;
        fb.error(e, "Not written");
      }
    });
    body.append(
      hint(p.diff.trim()
        ? "Exactly what will change in your settings.json. Your own hooks and settings are left alone."
        : "Nothing to change — settings.json already matches."),
      // The gateway URL carries its access token: never paint it.
      renderDiff(p.diff.replace(/(\/t\/)[A-Za-z0-9_-]{6,}/g, "$1••••••") || "(no changes)"),
      h("div", { class: "row" }, h("span", { class: "path", text: `Backup → ${p.backup}` })),
      h("div", { class: "row" }, apply, cancel),
    );
  }

  renderStatus();
  draw();
  // Dev only (`?mock=1&demo=1`): open the diff so a screenshot shows it.
  if (IS_MOCK && new URLSearchParams(window.location.search).has("demo")) void preview();
  el.append(
    statusList,
    counters,
    body,
    fb.el,
  );

  void onEvent("protection-changed", (p) => {
    status = p;
    renderStatus();
  });

  return el;
}
