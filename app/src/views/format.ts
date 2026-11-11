// Wording and colours shared by the island and the settings window: verdicts,
// tiers, impact chips, privacy notices and relative times. Pure functions only —
// no State, no DOM — so the settings bundle can use them without the island.

import type { ActivityItem, ImpactVector, PrivacyEvent, Tier } from "../core/bridge";
import { ICONS } from "./icons";

/** "just now", "4m", "2h", "3d" — same shape as the Swift `timeAgo`. */
export function timeAgo(ms: number, now = Date.now()): string {
  const diff = (now - ms) / 1000;
  if (!Number.isFinite(diff)) return "";
  if (diff < 45) return "just now";
  if (diff < 3600) return `${Math.max(1, Math.round(diff / 60))}m`;
  if (diff < 86400) return `${Math.floor(diff / 3600)}h`;
  return `${Math.floor(diff / 86400)}d`;
}

/** `2 secrets`, `1 secret`. */
export function plural(n: number, word: string, many = `${word}s`): string {
  return `${n} ${n === 1 ? word : many}`;
}

/** Thousands separators without pulling in Intl options everywhere. */
export function fmtCount(n: number): string {
  return n.toLocaleString("en-US");
}

// ── Tiers ─────────────────────────────────────────────────────────────────────

export function tierLabel(tier: Tier): string {
  return tier.toUpperCase();
}

// ── Verdicts ──────────────────────────────────────────────────────────────────

export type VerdictGroup = "blocked" | "asked" | "allowed" | "privacy" | "other";

export interface VerdictMeta {
  label: string;
  color: string;
  icon: string;
  /** Draw the icon as a stroke (true) or a fill. */
  stroke: boolean;
  group: VerdictGroup;
}

const GREEN = "#34D399";
const AMBER = "#F5A524";
const RED = "#F4505E";
const TEAL = "#2DD4BF";
const BLUE = "#60A5FA";
const GREY = "#6B7079";

/** How each ActivityItem verdict reads: label, colour, icon and filter group. */
export function verdictMeta(verdict: string): VerdictMeta {
  switch (verdict) {
    case "allow":
      return { label: "Allowed", color: GREEN, icon: ICONS.check, stroke: true, group: "allowed" };
    case "approved":
      return { label: "Approved", color: GREEN, icon: ICONS.check, stroke: true, group: "allowed" };
    case "ask":
      return { label: "Asked", color: AMBER, icon: ICONS.bang, stroke: false, group: "asked" };
    case "deny":
      return { label: "Blocked", color: RED, icon: ICONS.nosign, stroke: true, group: "blocked" };
    case "denied":
      return { label: "Denied", color: RED, icon: ICONS.xmark, stroke: false, group: "blocked" };
    case "blocked_prompt":
      return { label: "Prompt blocked", color: RED, icon: ICONS.nosign, stroke: true, group: "blocked" };
    case "masked":
      return { label: "Masked", color: TEAL, icon: ICONS.shield, stroke: false, group: "privacy" };
    case "rehydrated":
      return { label: "Restored", color: BLUE, icon: ICONS.restore, stroke: true, group: "privacy" };
    case "defer":
      return { label: "No opinion", color: GREY, icon: ICONS.ellipsis, stroke: false, group: "allowed" };
    default:
      return { label: verdict || "Event", color: GREY, icon: ICONS.ellipsis, stroke: false, group: "other" };
  }
}

/** The line to show for an activity item: the headline, else the summary. */
export function activityText(item: ActivityItem): string {
  return item.headline.trim() || item.summary.trim() || `${item.event} · ${item.tool}`.trim();
}

// ── Impact chips ──────────────────────────────────────────────────────────────

export type ChipLevel = "ok" | "warn" | "bad";

export interface ImpactChip {
  /** Dimension, shown dim: "Undo", "Scope", "Network", "Data". */
  key: string;
  value: string;
  level: ChipLevel;
  title: string;
}

/** The impact vector as chips: undo, scope, network and data, plus red flags. */
export function impactChips(v: ImpactVector): ImpactChip[] {
  const chips: ImpactChip[] = [];

  const undo: Record<string, [string, ChipLevel, string]> = {
    reversible: ["easy", "ok", "This can be undone."],
    partial: ["partly", "warn", "Only part of this can be undone."],
    irreversible: ["no", "bad", "This cannot be undone."],
  };
  const [uv, ul, ut] = undo[v.reversibility] ?? [v.reversibility, "warn", "Reversibility unknown."];
  chips.push({ key: "Undo", value: uv, level: ul, title: ut });

  const scope: Record<string, [string, ChipLevel, string]> = {
    none: ["none", "ok", "Touches no files."],
    project: ["project", "ok", "Stays inside the project folder."],
    user: ["your files", "warn", "Reaches files outside the project, in your user folder."],
    system: ["system", "bad", "Reaches system locations."],
  };
  const [sv, sl, st] = scope[v.blastRadius] ?? [v.blastRadius, "warn", "Blast radius unknown."];
  chips.push({ key: "Scope", value: sv, level: sl, title: st });

  const net: Record<string, [string, ChipLevel, string]> = {
    none: ["none", "ok", "No network access."],
    known_host: ["known host", "ok", "Talks to a host you allow."],
    unknown_host: ["unknown host", "bad", "Talks to a host Zuko doesn't know."],
  };
  const [nv, nl, nt] = net[v.egress] ?? [v.egress, "warn", "Network use unknown."];
  chips.push({ key: "Network", value: nv, level: nl, title: nt });

  const data: Record<string, [string, ChipLevel, string]> = {
    public: ["public", "ok", "No sensitive data involved."],
    internal: ["internal", "warn", "Involves internal project data."],
    secret: ["secret", "bad", "Involves secrets or sensitive files."],
  };
  const [dv, dl, dt] = data[v.sensitivity] ?? [v.sensitivity, "warn", "Sensitivity unknown."];
  chips.push({ key: "Data", value: dv, level: dl, title: dt });

  if (v.privilege) {
    chips.push({ key: "", value: "Admin rights", level: "bad", title: "Asks for elevated privileges." });
  }
  if (v.obfuscated) {
    chips.push({
      key: "", value: "Obfuscated", level: "bad",
      title: "Zuko can't read what this really does — unknown is not safe.",
    });
  }
  return chips;
}

// ── Privacy notices ───────────────────────────────────────────────────────────

const SOURCE_LABELS: Record<PrivacyEvent["source"], string> = {
  gateway: "Gateway",
  hook: "Claude Code",
  chat: "Chat",
  file: "File",
  browser: "Browser",
  clipboard: "Clipboard",
};

export function privacySourceLabel(source: string): string {
  return SOURCE_LABELS[source as PrivacyEvent["source"]] ?? source;
}

/** Distinct labels, first-seen order: ["OpenAI API key", "Email address"]. */
export function distinctLabels(e: PrivacyEvent): string[] {
  const out: string[] = [];
  for (const l of e.labels) {
    const t = l.trim();
    if (t && !out.includes(t)) out.push(t);
  }
  return out;
}

/** Title and detail line for a privacy notice. */
export function privacyText(e: PrivacyEvent): { title: string; detail: string } {
  const labels = distinctLabels(e);
  const what = labels.length > 3 ? `${labels.slice(0, 3).join(", ")} +${labels.length - 3}` : labels.join(", ");
  const n = Math.max(e.count, e.keys.length, 1);
  switch (e.direction) {
    case "blocked_prompt":
      return {
        title: "Blocked a prompt containing a secret",
        detail: e.maskedPrompt
          ? `Masked copy ready${what ? ` · ${what}` : ""}`
          : what || "Nothing was sent.",
      };
    case "rehydrated":
      return {
        title: `Restored ${plural(n, "value")} on your machine`,
        detail: what || "The model only ever saw placeholders.",
      };
    default:
      return {
        title: `Masked ${plural(n, "secret")} before ${n === 1 ? "it" : "they"} left`,
        detail: what || "Replaced with placeholders.",
      };
  }
}

// ── Misc ──────────────────────────────────────────────────────────────────────

/** `http://127.0.0.1:8787/t/abcdef…` → `http://127.0.0.1:8787/t/••••••`. */
export function maskGatewayUrl(url: string | null): string {
  if (!url) return "—";
  return url.replace(/(\/t\/)[^/?#]+/, "$1••••••");
}

/** `API_KEY_1` → `{{API_KEY_1}}`. */
export function placeholder(key: string): string {
  return `{{${key}}}`;
}
