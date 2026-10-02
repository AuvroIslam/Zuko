// Policy: the rules Zuko enforces on every tool call, the approval friction
// and what the privacy shield looks for. Edits go into a draft; Save sends
// the whole policy to Rust, which validates it again.

import { h, clear } from "../views/dom";
import { Bridge, TIERS, tierRank, type Policy, type PolicyMode, type RuleVerdict, type Tier } from "../core/bridge";
import { chipEditor, confirmButton, feedback, hint, section, segmented, select, sub, toggle } from "./ui";

// ── Validation ────────────────────────────────────────────────────────────────

const DOMAIN = /^(\*\.)?([a-z0-9]([a-z0-9-]*[a-z0-9])?\.)+[a-z]{2,}$|^localhost$|^\d{1,3}(\.\d{1,3}){3}$/i;

function validDomain(v: string): string | null {
  if (/^[a-z]+:\/\//i.test(v)) return "Just the host — drop the http:// part.";
  if (/[/\s]/.test(v)) return "A domain has no path or spaces (e.g. pastebin.com or *.ngrok.io).";
  return DOMAIN.test(v) ? null : `"${v}" doesn't look like a domain.`;
}

function validPath(v: string): string | null {
  if (v.length > 400) return "That path is too long.";
  if (/[\n\r"<>|]/.test(v)) return "Paths can't contain quotes, <, > or |.";
  return null;
}

function validCommand(v: string): string | null {
  return v.length > 300 ? "Keep command patterns under 300 characters." : null;
}

function validTool(v: string): string | null {
  return /^[A-Za-z0-9_*.-]+$/.test(v) ? null : "Tool names use letters, digits, _ . - and * (e.g. mcp__github__*).";
}

function validTerm(v: string): string | null {
  return v.length < 3 ? "Custom terms need at least 3 characters." : null;
}

/** Problems with the draft as a whole; empty when it can be saved. */
function validate(p: Policy): string[] {
  const errs: string[] = [];
  const hold = p.approvals.holdMs;
  if (!Number.isFinite(hold) || hold < 300 || hold > 10_000) errs.push("Hold duration must be between 300 and 10,000 ms.");
  if (tierRank(p.approvals.blockFrom) < tierRank(p.approvals.holdToApproveFrom)) {
    errs.push("“Block from” can't be a lower tier than “Hold to approve from”.");
  }
  const both = p.network.blocked.filter((d) => p.network.allowed.includes(d));
  if (both.length) errs.push(`${both.join(", ")} is both blocked and allowed.`);
  return errs;
}

// ── Section ───────────────────────────────────────────────────────────────────

const TIER_OPTIONS: [Tier, string][] = TIERS.map((t) => [t, t[0].toUpperCase() + t.slice(1)]);

export function policySection(initial: Policy | null): HTMLElement {
  const { el } = section("policy", "Policy");
  if (!initial) {
    el.append(hint("The policy is loaded from Zuko. Open this window from the running app to edit it."));
    return el;
  }

  let saved: Policy = structuredClone(initial);
  let draft: Policy = structuredClone(initial);

  const body = h("div", { class: "stack-12" });
  const errors = h("div", { class: "field-error block" });
  const dirtyLabel = h("span", { class: "hint" });
  const fb = feedback();
  const save = h("button", { class: "primary", text: "Save policy" });
  const revert = h("button", { text: "Discard changes" });
  const reset = confirmButton("Reset to defaults", "Click again to reset", async () => {
    try {
      const fresh = await Bridge.policyReset();
      saved = structuredClone(fresh);
      draft = structuredClone(fresh);
      build();
      fb.show("ok", "Policy reset to the defaults and saved.");
    } catch (e) {
      fb.error(e, "Couldn't reset");
    }
  });

  function changed() {
    const dirty = JSON.stringify(draft) !== JSON.stringify(saved);
    const errs = validate(draft);
    errors.textContent = errs.join(" ");
    errors.style.display = errs.length ? "" : "none";
    save.disabled = !dirty || errs.length > 0;
    revert.disabled = !dirty;
    dirtyLabel.textContent = dirty ? "Unsaved changes" : "";
    if (dirty) fb.clear();
  }

  save.addEventListener("click", async () => {
    save.disabled = true;
    try {
      await Bridge.policySet(draft);
      saved = structuredClone(draft);
      fb.show("ok", "Saved. Applies from the next tool call.");
    } catch (e) {
      fb.error(e, "Not saved");
    }
    changed();
  });
  revert.addEventListener("click", () => {
    draft = structuredClone(saved);
    build();
  });

  /** A labelled chip editor bound to a list in the draft. */
  function list(
    label: string,
    get: (p: Policy) => string[],
    put: (p: Policy, v: string[]) => void,
    placeholder: string,
    validateOne: (v: string) => string | null,
    help?: string,
  ): HTMLElement {
    const editor = chipEditor({
      values: get(draft),
      placeholder,
      validate: validateOne,
      onChange: (v) => {
        put(draft, v);
        changed();
      },
    });
    return h("div", { class: "field" },
      h("div", { class: "field-label" }, h("label", { text: label }), help ? h("span", { class: "hint", text: help }) : null),
      editor.el);
  }

  function toggleRow(label: string, on: boolean, set: (v: boolean) => void, help?: string): HTMLElement {
    return h("div", { class: "option" },
      toggle(on, (v) => { set(v); changed(); }, label),
      h("div", {}, h("b", { text: label }), help ? h("span", { class: "hint", text: help }) : null));
  }

  function build() {
    clear(body);
    const d = draft;

    const mode = segmented<PolicyMode>([["enforce", "Enforce"], ["monitor", "Monitor"]], d.mode, (v) => {
      d.mode = v;
      changed();
    });

    const holdMs = h("input", { type: "number", min: "300", max: "10000", step: "100", value: String(d.approvals.holdMs), style: "width:92px" }) as HTMLInputElement;
    holdMs.addEventListener("input", () => {
      d.approvals.holdMs = Number(holdMs.value);
      changed();
    });

    const det = d.privacy.detector;
    const detector = (key: keyof typeof det, label: string) =>
      h("label", { class: "check" },
        toggle(Boolean(det[key]), (v) => { (det as unknown as Record<string, boolean>)[key] = v; changed(); }, label),
        h("span", { text: label }));

    body.append(
      h("div", { class: "row" }, h("label", { text: "Mode" }), mode.el,
        h("span", { class: "hint", text: "Monitor logs every decision but blocks nothing." })),

      sub("Network"),
      list("Blocked domains", (p) => p.network.blocked, (p, v) => { p.network.blocked = v; },
        "pastebin.com, *.ngrok.io", validDomain),
      list("Allowed domains", (p) => p.network.allowed, (p, v) => { p.network.allowed = v; },
        "github.com", validDomain, "Known hosts lower the risk score."),
      h("div", { class: "row" }, h("label", { text: "Unknown hosts" }),
        select<RuleVerdict>([["allow", "Allow (scored)"], ["ask", "Ask"], ["deny", "Block"]], d.network.unknown, (v) => {
          d.network.unknown = v;
          changed();
        })),

      sub("Files"),
      list("Blocked from reading", (p) => p.filesystem.blockedRead, (p, v) => { p.filesystem.blockedRead = v; },
        "~/.ssh/**", validPath),
      list("Blocked from writing", (p) => p.filesystem.blockedWrite, (p, v) => { p.filesystem.blockedWrite = v; },
        "C:/Windows/**", validPath),
      list("Sensitive files", (p) => p.filesystem.sensitive, (p, v) => { p.filesystem.sensitive = v; },
        "**/.env", validPath, "Reading these taints the session: sending data out afterwards is asked or blocked."),

      sub("Commands and tools"),
      list("Blocked commands", (p) => p.commands.blocked, (p, v) => { p.commands.blocked = v; },
        "git push --force*", validCommand),
      list("Always ask for", (p) => p.commands.ask, (p, v) => { p.commands.ask = v; },
        "npm publish*", validCommand),
      list("Blocked tools", (p) => p.tools.blocked, (p, v) => { p.tools.blocked = v; },
        "mcp__browser__*", validTool),
      list("Ask before tools", (p) => p.tools.ask, (p, v) => { p.tools.ask = v; },
        "mcp__*", validTool),

      sub("Approvals"),
      toggleRow("Auto-allow low risk", d.approvals.autoAllowLowRisk, (v) => { d.approvals.autoAllowLowRisk = v; },
        "Low-risk actions go through without a prompt. Unknown commands are never low risk."),
      h("div", { class: "row" }, h("label", { text: "Hold to approve from" }),
        select(TIER_OPTIONS, d.approvals.holdToApproveFrom, (v) => { d.approvals.holdToApproveFrom = v; changed(); })),
      h("div", { class: "row" }, h("label", { text: "Block from" }),
        select(TIER_OPTIONS, d.approvals.blockFrom, (v) => { d.approvals.blockFrom = v; changed(); })),
      h("div", { class: "row" }, h("label", { text: "Hold duration" }), holdMs, h("span", { class: "hint", text: "ms" })),

      sub("Privacy"),
      h("div", { class: "checks" },
        detector("secrets", "Secrets & API keys"),
        detector("genericEntropy", "Random-looking tokens"),
        detector("pii", "Personal data"),
        detector("emails", "Emails"),
        detector("phones", "Phone numbers"),
        detector("cards", "Card numbers"),
        detector("ibans", "IBANs"),
        detector("nationalIds", "National IDs"),
        detector("ips", "Public IPs"),
      ),
      list("Custom terms", (p) => p.privacy.detector.customTerms, (p, v) => { p.privacy.detector.customTerms = v; },
        "Project Falcon", validTerm, "Client names, codenames — masked like secrets."),
      list("Never mask", (p) => p.privacy.detector.allowlist, (p, v) => { p.privacy.detector.allowlist = v; },
        "example.com", (v) => (v.length ? null : "Empty value.")),
      toggleRow("Block secret prompts without the gateway", d.privacy.blockSecretPromptsWithoutGateway,
        (v) => { d.privacy.blockSecretPromptsWithoutGateway = v; },
        "Hooks-only mode can't rewrite a prompt, so it holds it back and offers a masked copy."),
      toggleRow("Mask command output", d.privacy.maskToolOutput, (v) => { d.privacy.maskToolOutput = v; },
        "Secrets printed by Bash are masked before Claude reads them."),
    );
    changed();
  }

  build();
  el.append(
    body,
    errors,
    h("div", { class: "row sticky-actions" }, save, revert, dirtyLabel, h("div", { class: "spacer" }), reset),
    fb.el,
  );
  changed();
  return el;
}
