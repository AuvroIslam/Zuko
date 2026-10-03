// App state — mirror of AppState.swift (the parts the island needs), plus what
// Zuko adds on top: the protection status, the activity feed, the privacy notice
// and the rubber-stamp guard.

import type { BotEmoteName, BotStateName, IslandMode, IslandViewName } from "./layout";
import type { EyeShape } from "../character/engine";
import {
  tierRank,
  type ActivityItem, type PrivacyEvent, type ProtectionStatus, type Tier, type ZukoHookInfo,
} from "./bridge";

export type AgentSource = "claudeCode" | "zuko" | "agent";
export type PillBadge = "approval" | "finished" | "error" | "denied" | "masked";

export interface AgentTask {
  id: string;
  name: string;
  color: string;
  state: BotStateName;
  stepIndex: number;
  steps: string[];
  source: AgentSource;
  /** One of Zuko's protection surfaces (always present), as opposed to an external agent. */
  isSurface: boolean;
  emote?: BotEmoteName | null;
  miniEye?: EyeShape | null;
  pillBadge?: PillBadge | null;
  /** Short status shown on the right of the pill ("on", "3"…). */
  pillMeta?: string | null;
  /** Tooltip for the pill. */
  pillTitle?: string | null;
  sessionCwd?: string | null;
}

export interface ApprovalInfo {
  requestId: string;
  sessionId: string;
  tool: string;
  command: string;
  /** Zuko's risk verdict; null when the payload came without one (older relay). */
  zuko: ZukoHookInfo | null;
  /** performance.now() when the request arrived. */
  receivedAt: number;
  /** Local AI explanation (`ai-explain`), when one arrives. Display only. */
  aiExplanation: string | null;
}

export interface ChatMessage {
  id: number;
  role: "user" | "assistant";
  content: string;
}

export type PromptContext =
  | { kind: "window"; appName: string; title: string; url?: string }
  | { kind: "file"; name: string; path?: string };

export interface ResultItem {
  label: string;
  detail: string;
  url?: string;
}

export interface SearchResult {
  title: string;
  items: ResultItem[];
  note?: string;
}

// ── Protection surfaces (the pills) ───────────────────────────────────────────

export const CLAUDE_ID = "surface_claude";
export const GATEWAY_ID = "surface_gateway";
export const BROWSER_ID = "surface_browser";
export const POLICY_ID = "surface_policy";

export const CLAUDE_NAME = "Claude Code";

const surface = (id: string, name: string, color: string, source: AgentSource): AgentTask => ({
  id, name, color, state: "idle", stepIndex: 0, steps: [], source, isSurface: true,
});

/** The four places Zuko protects, in pill order. */
export const SURFACES: readonly AgentTask[] = [
  surface(CLAUDE_ID, CLAUDE_NAME, "#E8875B", "claudeCode"),
  surface(GATEWAY_ID, "Gateway", "#2DD4BF", "zuko"),
  surface(BROWSER_ID, "Browser", "#60A5FA", "zuko"),
  surface(POLICY_ID, "Policy", "#A78BFA", "zuko"),
];

/** How many activity items the island keeps. */
export const ACTIVITY_LIMIT = 200;

// ── Rubber-stamp guard ────────────────────────────────────────────────────────

/**
 * Spots approvals given faster than anyone can read them. Three medium-or-higher
 * requests approved in under 800 ms in a row arm the guard; the next
 * medium-or-higher approval then needs Allow held for 1.2 s, after which the
 * guard disarms. A deny or a considered (slow) approval breaks the streak; low
 * risk approvals neither count nor break it.
 */
export class RubberStampGuard {
  static readonly FAST_MS = 800;
  static readonly STREAK = 3;
  static readonly HOLD_MS = 1200;

  streak = 0;
  armed = false;

  /** True when a request of this tier must be held because of the guard. */
  applies(tier: Tier | null): boolean {
    return this.armed && tier != null && tierRank(tier) >= tierRank("medium");
  }

  /** Records one answer. Returns true when this answer armed the guard. */
  record(tier: Tier | null, decision: "allow" | "deny", elapsedMs: number): boolean {
    if (decision === "deny") {
      this.streak = 0;
      return false;
    }
    if (tier == null || tierRank(tier) < tierRank("medium")) return false;
    if (this.armed) {
      // This approval went through the enforced hold.
      this.armed = false;
      this.streak = 0;
      return false;
    }
    if (elapsedMs >= RubberStampGuard.FAST_MS) {
      this.streak = 0;
      return false;
    }
    this.streak += 1;
    if (this.streak < RubberStampGuard.STREAK) return false;
    this.streak = 0;
    this.armed = true;
    return true;
  }
}

// ── Settings ──────────────────────────────────────────────────────────────────

export interface Settings {
  soundEnabled: boolean;
  soundVolume: number;
  autoCloseInterval: number;
  absenceInterval: number;
  screen: "primary" | "cursor";
  autostart: boolean;
  hooksInstalled: boolean;
  /** Who answers the island chat. */
  chatProvider: ChatProvider;
  /** Claude model used by the chat (the Anthropic provider; the field predates the others). */
  model: string;
  /** OpenAI model used by the chat. */
  openaiModel: string;
  /** Ollama model used by the chat (independent of the local AI's scan model). */
  ollamaModel: string;
}

/** Who answers the island chat: Claude and OpenAI get masked text, Ollama stays on this PC. */
export type ChatProvider = "anthropic" | "openai" | "ollama";

export const CHAT_PROVIDERS: readonly ChatProvider[] = ["anthropic", "openai", "ollama"];

/** The name the user reads. */
export const CHAT_PROVIDER_LABEL: Record<ChatProvider, string> = {
  anthropic: "Claude",
  openai: "OpenAI",
  ollama: "Ollama",
};

/** Settings field holding each provider's model. */
export const CHAT_MODEL_FIELD = {
  anthropic: "model",
  openai: "openaiModel",
  ollama: "ollamaModel",
} as const satisfies Record<ChatProvider, keyof Settings>;

export const DEFAULT_SETTINGS: Settings = {
  soundEnabled: true,
  soundVolume: 0.12,
  autoCloseInterval: 15,
  absenceInterval: 180,
  screen: "primary",
  autostart: false,
  hooksInstalled: false,
  chatProvider: "anthropic",
  model: "claude-opus-5",
  openaiModel: "gpt-5-mini",
  ollamaModel: "gemma3:4b",
};

/** The model the chat uses with `provider` (the default when the field is blank). */
export function chatModel(s: Settings, provider: ChatProvider = s.chatProvider): string {
  const field = CHAT_MODEL_FIELD[provider];
  return s[field]?.trim() || DEFAULT_SETTINGS[field];
}

type Listener = () => void;

class AppState {
  mode: IslandMode = "hidden";
  view: IslandViewName = "overview";

  tasks: AgentTask[] = [];
  focusId: string | null = null;

  stateOverride: BotStateName | null = null;

  /** Cursor in logical screen pixels, origin top-left (like AppState.mousePosition). */
  mouse = { x: 0, y: 0 };
  /** Cursor relative to the island's top-left corner. */
  mouseInIsland = { x: 0, y: 0 };

  isPinned = false;
  paused = false;

  uploadProgress = 0;
  uploadDuration = 2.4;
  fileDragOver = false;

  promptContext: PromptContext | null = null;
  droppedFile: { name: string; path: string } | null = null;
  noteMessage: string | null = null;
  searchResult: SearchResult | null = null;
  chatHistory: ChatMessage[] = [];
  pendingApproval: ApprovalInfo | null = null;

  /** Last status from Rust; null until the first answer (or outside Tauri). */
  protection: ProtectionStatus | null = null;
  /** Newest first, at most ACTIVITY_LIMIT. */
  activity: ActivityItem[] = [];
  /** The privacy notice on screen (or waiting behind an approval card). */
  privacyNotice: PrivacyEvent | null = null;
  rubberStamp = new RubberStampGuard();

  lastActivity = performance.now();

  settings: Settings = { ...DEFAULT_SETTINGS };

  private listeners = new Set<Listener>();

  subscribe(fn: Listener): () => void {
    this.listeners.add(fn);
    return () => this.listeners.delete(fn);
  }

  /** Marks the UI dirty; the island re-renders on the next frame. */
  notify() {
    for (const fn of this.listeners) fn();
  }

  get focusTask(): AgentTask | null {
    return this.tasks.find((t) => t.id === this.focusId) ?? this.tasks[0] ?? null;
  }

  get effectiveState(): BotStateName {
    return this.stateOverride ?? this.focusTask?.state ?? "idle";
  }

  get otherTasks(): AgentTask[] {
    return this.tasks.filter((t) => t.id !== this.focusId);
  }

  task(id: string): AgentTask | null {
    return this.tasks.find((t) => t.id === id) ?? null;
  }

  setFocus(id: string) {
    const t = this.task(id);
    if (!t) return;
    this.focusId = id;
    t.pillBadge = null;
    this.notify();
  }

  updateTask(id: string, state: BotStateName) {
    const t = this.task(id);
    if (!t) return;
    t.state = state;
    this.notify();
  }

  appendStep(id: string, step: string) {
    const t = this.task(id);
    if (!t) return;
    t.steps.push(step);
    if (t.steps.length > 20) t.steps.shift();
    t.stepIndex = t.steps.length - 1;
    this.notify();
  }

  setPillBadge(id: string, badge: PillBadge | null) {
    const t = this.task(id);
    if (!t) return;
    t.pillBadge = badge;
    this.notify();
  }

  /** Creates the surface pills once; Claude Code first and focused. */
  loadSurfaces() {
    for (const proto of SURFACES) {
      if (!this.task(proto.id)) this.tasks.push({ ...proto, steps: [] });
    }
    this.sortTasks();
    if (!this.focusId) this.focusId = CLAUDE_ID;
    this.applyProtection();
    this.notify();
  }

  /**
   * Order: Claude Code, then external agent_* pills (so a live agent is in the
   * visible slice), then the other surfaces in declaration order.
   */
  private sortTasks() {
    const order = SURFACES.map((t) => t.id);
    const rank = (t: AgentTask) =>
      t.id === CLAUDE_ID ? -2 : t.id.startsWith("agent_") ? -1 : order.indexOf(t.id);
    this.tasks.sort((a, b) => rank(a) - rank(b));
  }

  removeTask(id: string) {
    const idx = this.tasks.findIndex((t) => t.id === id);
    if (idx < 0) return;
    this.tasks.splice(idx, 1);
    if (this.focusId === id) this.focusId = this.tasks[0]?.id ?? CLAUDE_ID;
    this.notify();
  }

  /** Creates a dynamic agent_ pill on first event; no-ops if it already exists.
   *  Inserted right after Claude Code so it appears in the visible slice. */
  upsertExternalAgent(id: string, name: string, color: string) {
    if (this.task(id)) return;
    const at = this.tasks.findIndex((t) => t.id === CLAUDE_ID) + 1;
    this.tasks.splice(at, 0, {
      id, name, color,
      state: "idle", stepIndex: 0, steps: [],
      source: "agent", isSurface: false,
    });
    if (!this.focusId) this.focusId = id;
    this.notify();
  }

  /** New status from Rust: store it and restate the surface pills. */
  setProtection(status: ProtectionStatus | null) {
    this.protection = status;
    this.applyProtection();
    this.notify();
  }

  /**
   * Derives each surface pill's resting state and meta text from the protection
   * status. A pill busy showing something (a finished session, a fresh block)
   * keeps its state; only resting pills are restated.
   */
  private applyProtection() {
    const p = this.protection;
    const resting = (t: AgentTask) =>
      t.state === "idle" || t.state === "sleeping" || t.state === "error";

    const gateway = this.task(GATEWAY_ID);
    if (gateway) {
      const on = !!p?.gatewayConfigured;
      gateway.pillMeta = on ? (p!.gatewayRunning ? `${p!.maskedTotal}` : "down") : "off";
      gateway.pillTitle = !p
        ? "Gateway status unknown"
        : !on
          ? "Gateway off — hooks-only mode"
          : p.gatewayRunning
            ? `Gateway on · ${p.maskedTotal} value${p.maskedTotal === 1 ? "" : "s"} masked since launch`
            : "Gateway configured but not running";
      if (resting(gateway)) gateway.state = !on ? "sleeping" : p!.gatewayRunning ? "idle" : "error";
    }

    const browser = this.task(BROWSER_ID);
    if (browser) {
      const on = !!p?.extensionConnected;
      browser.pillMeta = on ? "on" : "off";
      browser.pillTitle = on ? "Browser extension connected" : "Browser extension not connected";
      if (resting(browser)) browser.state = on ? "idle" : "sleeping";
    }

    const policy = this.task(POLICY_ID);
    if (policy) {
      const blocked = p?.blockedTotal ?? 0;
      policy.pillMeta = p?.mode === "monitor" ? "monitor" : `${blocked}`;
      policy.pillTitle = p?.mode === "monitor"
        ? "Monitor mode — nothing is blocked, everything is logged"
        : `${blocked} action${blocked === 1 ? "" : "s"} blocked since launch`;
      if (resting(policy)) policy.state = "idle";
    }

    const claude = this.task(CLAUDE_ID);
    if (claude) {
      const asked = p?.askedTotal ?? 0;
      claude.pillMeta = p && !p.hooksInstalled ? "off" : asked > 0 ? `${asked}` : null;
      claude.pillTitle = p && !p.hooksInstalled
        ? "Hooks not installed"
        : `${asked} approval${asked === 1 ? "" : "s"} asked since launch`;
    }
  }

  /** Adds an activity item to the front of the feed (ignoring repeats). */
  pushActivity(item: ActivityItem) {
    if (this.activity.some((a) => a.id === item.id)) return;
    this.activity.unshift(item);
    if (this.activity.length > ACTIVITY_LIMIT) this.activity.length = ACTIVITY_LIMIT;
    this.notify();
  }

  /** A local AI explanation arrived (`ai-explain`): attach it to its card or feed item. */
  applyExplanation(e: { requestId: string | null; activityId: string | null; text: string }) {
    let hit = false;
    if (e.requestId && this.pendingApproval?.requestId === e.requestId) {
      this.pendingApproval = { ...this.pendingApproval, aiExplanation: e.text };
      hit = true;
    }
    const item = e.activityId ? this.activity.find((a) => a.id === e.activityId) : undefined;
    if (item) {
      item.aiExplanation = e.text;
      hit = true;
    }
    if (hit) this.notify();
  }

  /** Replaces the feed with a newest-first list from Rust, keeping anything newer. */
  seedActivity(items: ActivityItem[]) {
    const seen = new Set<string>();
    const merged = [...this.activity, ...items].filter((a) => {
      if (seen.has(a.id)) return false;
      seen.add(a.id);
      return true;
    });
    merged.sort((a, b) => b.ts - a.ts);
    this.activity = merged.slice(0, ACTIVITY_LIMIT);
    this.notify();
  }

  defaultView(): IslandViewName {
    return this.tasks.length === 0 ? "empty" : "overview";
  }
}

export const State = new AppState();
