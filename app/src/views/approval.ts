// The approval card: a PermissionRequest with Zuko's risk verdict on it.
//
// Friction follows the verdict (ZukoHookInfo.friction):
//   none    → Deny / Allow, like Coucou's card.
//   hold    → Allow becomes "Hold to allow": it fills only while pressed and the
//             decision is sent when it is full. Letting go early resets it.
//   blocked → no Allow at all; the reason is shown and Deny is the only answer.
// The rubber-stamp guard (State.rubberStamp) can turn a "none" card into a
// 1.2 s hold after three fast approvals in a row.

import { h, svg, clear, dot } from "./dom";
import { ICONS } from "./icons";
import { agentWho, btn, card, setWash, stack } from "./parts";
import { impactChips, tierLabel } from "./format";
import type { ViewActions, ViewHost } from "./views";
import { tierColor, tierWash } from "../core/layout";
import { CLAUDE_ID, RubberStampGuard, State, type ApprovalInfo } from "../core/state";
import type { ZukoHookInfo } from "../core/bridge";

/** Most factor sentences a card shows. */
const MAX_FACTORS = 3;

export type EffectiveFriction =
  | { type: "none" }
  | { type: "hold"; ms: number; guard: boolean }
  | { type: "blocked" };

/** The verdict's friction, tightened by the rubber-stamp guard. */
export function effectiveFriction(info: ApprovalInfo, guard: RubberStampGuard): EffectiveFriction {
  const z = info.zuko;
  if (!z) return { type: "none" };
  const f = z.friction;
  if (f.type === "blocked") return { type: "blocked" };
  const guarded = guard.applies(z.tier);
  if (f.type === "hold") {
    const ms = Math.max(f.ms, guarded ? RubberStampGuard.HOLD_MS : 0);
    return { type: "hold", ms, guard: guarded && f.ms < RubberStampGuard.HOLD_MS };
  }
  return guarded ? { type: "hold", ms: RubberStampGuard.HOLD_MS, guard: true } : { type: "none" };
}

/** Up to three factor sentences, heaviest first. */
export function topFactors(z: ZukoHookInfo): string[] {
  const seen = new Set<string>();
  return [...z.factors]
    .filter((f) => f.text.trim())
    .sort((a, b) => b.weight - a.weight)
    .map((f) => f.text.trim())
    .filter((t) => {
      const key = t.toLowerCase();
      if (seen.has(key) || key === z.headline.trim().toLowerCase()) return false;
      seen.add(key);
      return true;
    })
    .slice(0, MAX_FACTORS);
}

/** Why a blocked request is blocked, without repeating the tier and headline. */
export function blockedReason(z: ZukoHookInfo): string {
  const reasons = z.violations.map((v) => v.reason.trim()).filter(Boolean);
  if (reasons.length) return reasons.join(" · ");
  const user = z.reasonUser.replace(/^[^A-Za-z]*[A-Z]+(?: RISK)?\s*[—–-]\s*/, "").trim();
  return user || "This crosses one of your policy rules.";
}

/** The note under the factors, if any. */
function noteFor(info: ApprovalInfo, friction: EffectiveFriction): { text: string; kind: "block" | "guard" } | null {
  if (friction.type === "blocked" && info.zuko) {
    return { text: `Zuko blocks this: ${blockedReason(info.zuko)}`, kind: "block" };
  }
  if (friction.type === "hold" && friction.guard) {
    return { text: "You approved the last few in a blink — hold Allow for a moment this time.", kind: "guard" };
  }
  return null;
}

/**
 * Optional lines on the card (factors + note), for the island height. Null when
 * the request has no verdict, which keeps Coucou's compact card.
 */
export function approvalLines(info: ApprovalInfo | null, guard: RubberStampGuard): number | null {
  if (!info?.zuko) return null;
  const friction = effectiveFriction(info, guard);
  // The AI explanation is clamped to two lines.
  return topFactors(info.zuko).length + (noteFor(info, friction) ? 1 : 0) + (info.aiExplanation ? 2 : 0);
}

/** "DELETES the folder build/" → the leading capitalised verb in the tier colour. */
function renderHeadline(el: HTMLElement, text: string, color: string) {
  clear(el);
  const m = /^([A-Z][A-Z'’]+(?: [A-Z][A-Z'’]+)*)(\b[\s\S]*)$/.exec(text);
  if (m) {
    el.append(h("b", { style: `color:${color}`, text: m[1] }), document.createTextNode(m[2]));
  } else {
    el.textContent = text;
  }
}

export function buildApproval(actions: ViewActions): ViewHost {
  const who = h("div", { class: "appr-who" });
  const headline = h("div", { class: "appr-headline" });
  const code = h("div", { class: "code" });
  const factors = h("div", { class: "appr-factors" });
  const chips = h("div", { class: "appr-chips" });
  const note = h("div", { class: "appr-note" });
  // The local AI's plain-English explanation, labelled as such. It arrives after the
  // card is up and only adds text: the headline, friction and buttons stay Zuko's.
  const aiText = h("span");
  const ai = h("div", { class: "appr-ai" }, h("i", { text: "AI explanation" }), aiText);
  ai.style.display = "none";

  // Every button is built once, here. Rebuilding one between a pointer-down and
  // a pointer-up swallows the click, and a hold would lose its press; sync()
  // only shows, hides and relabels.
  const deny = btn("Deny", "secondary", () => decide("deny"), "N");
  const allow = btn("Allow", "primary", () => decide("allow"), "Y");
  const holdFill = h("span", { class: "hold-fill" });
  const holdLabel = h("span", { class: "hold-label", text: "Hold to allow" });
  const holdTime = h("span", { class: "hold-time" });
  const hold = h("button", { class: "btn hold" }, holdFill, holdLabel, holdTime);
  const policyLink = h("button", {
    class: "link-btn appr-link",
    text: "Edit policy…",
    onclick: () => actions.openSettingsWindow(),
  });
  const row = h("div", { class: "actions" }, deny, allow, hold, h("div", { class: "grow" }), policyLink);

  const body = stack(116, 16, who, headline, code, factors, ai, chips, note, row);
  body.classList.add("appr");
  const cardEl = card("amber", body);
  const el = h("div", { class: "view" }, cardEl);

  let currentId = "";
  /** performance.now() when this request's card was first on screen. */
  let shownAt = 0;
  let friction: EffectiveFriction = { type: "none" };
  let holding = false;
  let holdStart = 0;
  let holdDone = false;

  function decide(d: "allow" | "deny") {
    if (!State.pendingApproval) return;
    const elapsed = performance.now() - shownAt;
    cancelHold();
    actions.decide(d, elapsed);
  }

  function startHold() {
    if (friction.type !== "hold" || holding || !State.pendingApproval) return;
    holding = true;
    holdDone = false;
    holdStart = performance.now();
    hold.classList.add("holding");
    actions.keepAlive();
  }

  function cancelHold() {
    holding = false;
    hold.classList.remove("holding");
    holdFill.style.transform = "scaleX(0)";
    if (friction.type === "hold") holdTime.textContent = `${(friction.ms / 1000).toFixed(1)}s`;
  }

  hold.addEventListener("pointerdown", (e) => {
    if (e.button !== 0) return;
    startHold();
  });
  // Letting go, sliding off or the OS stealing the pointer all reset the fill;
  // only a press held to the end decides.
  hold.addEventListener("pointerup", () => cancelHold());
  hold.addEventListener("pointerleave", () => cancelHold());
  hold.addEventListener("pointercancel", () => cancelHold());

  function render(info: ApprovalInfo) {
    const z = info.zuko;
    friction = effectiveFriction(info, State.rubberStamp);
    cancelHold();

    const tier = z?.tier ?? null;
    const color = tier ? tierColor(tier) : "#F5A524";
    setWash(cardEl, tierWash(tier));
    cardEl.dataset.tier = tier ?? "none";

    headline.style.display = z ? "" : "none";
    if (z) renderHeadline(headline, z.headline.trim() || `Wants to use ${info.tool}`, color);

    clear(factors);
    for (const text of z ? topFactors(z) : []) {
      factors.append(h("div", { class: "appr-factor", title: text }, dot(color, 4), h("span", { text })));
    }
    factors.style.display = factors.childElementCount ? "" : "none";

    clear(chips);
    if (z) {
      for (const c of impactChips(z.vector)) {
        chips.append(
          h("span", { class: `impact ${c.level}`, title: c.title },
            c.key ? h("i", { text: c.key }) : null,
            h("span", { text: c.value }),
          ),
        );
      }
      if (z.rehydrated.length) {
        chips.append(h("span", {
          class: "impact info",
          title: "Placeholders Zuko fills with the real values on your machine.",
        }, svg(ICONS.lock, 9), h("span", { text: `fills ${z.rehydrated.length}` })));
      }
    }
    chips.style.display = chips.childElementCount ? "" : "none";

    const n = noteFor(info, friction);
    note.textContent = n?.text ?? "";
    note.className = n ? `appr-note ${n.kind}` : "appr-note";
    note.style.display = n ? "" : "none";

    // Buttons: shown, hidden and relabelled — never rebuilt.
    const blocked = friction.type === "blocked";
    deny.className = blocked ? "btn primary" : "btn secondary";
    allow.style.display = friction.type === "none" ? "" : "none";
    hold.style.display = friction.type === "hold" ? "" : "none";
    policyLink.style.display = blocked ? "" : "none";
    hold.style.setProperty("--fill", color);
    if (friction.type === "hold") holdTime.textContent = `${(friction.ms / 1000).toFixed(1)}s`;
  }

  return {
    el,
    sync() {
      const info = State.pendingApproval;
      clear(who);
      who.append(agentWho(State.task(CLAUDE_ID), "needs permission"));
      const z = info?.zuko;
      if (z) {
        const pill = h("span", {
          class: `tier-pill ${z.tier}`,
          title: `Risk score ${z.score} / 100`,
        }, h("i"), h("span", { text: tierLabel(z.tier) }), h("em", { text: String(z.score) }));
        pill.style.setProperty("--tc", tierColor(z.tier));
        who.append(pill);
      }

      // The whole point of approving here rather than in the terminal: this line
      // is the command, the file path or the URL being authorised, not just the
      // name of the tool asking.
      code.textContent = info?.command || info?.tool || "…";

      if (!info) return;
      const explanation = info.aiExplanation ?? "";
      if (aiText.textContent !== explanation) aiText.textContent = explanation;
      ai.title = explanation;
      ai.style.display = explanation ? "" : "none";
      if (info.requestId !== currentId) {
        currentId = info.requestId;
        // sync() only runs while the view is on screen, so this is the moment the
        // card was first seen — the start of elapsedMs.
        shownAt = performance.now();
        render(info);
      }
    },
    tick() {
      if (!holding || friction.type !== "hold") return;
      // performance.now(), not the frame timestamp: holdStart came from it, and the
      // two clocks can disagree by a frame or more.
      const p = Math.max(0, Math.min(1, (performance.now() - holdStart) / friction.ms));
      holdFill.style.transform = `scaleX(${p})`;
      holdTime.textContent = `${Math.max(0, (friction.ms * (1 - p)) / 1000).toFixed(1)}s`;
      if (p >= 1 && !holdDone) {
        holdDone = true;
        decide("allow");
      }
    },
    animating: () => holding,
  };
}
