// Claude Code hook events → island state, plus Zuko's own events (activity,
// privacy, protection status).
// Port of HookServer.processEvent / processPermissionRequest from the macOS app.
// Difference from macOS: no terminal filter. On Windows the hook fires from any
// terminal (Windows Terminal, VS Code, PowerShell…) and all of them are handled.

import { Bridge, onEvent, type HookEventPayload, type PrivacyEvent, type ZukoHookInfo } from "../core/bridge";
import { Sound } from "../core/sound";
import { BROWSER_ID, CLAUDE_ID, CLAUDE_NAME, GATEWAY_ID, POLICY_ID, State } from "../core/state";
import type { Island } from "./island";

/** Clears the approval card if no decision was made before the hook gave up. */
let pendingTimeout: number | null = null;
/** Clears the Policy pill's red badge a little after a block. */
let policyBadgeTimer: number | null = null;

/** Same rule as HookServer.validateAgent on macOS. "claude" is reserved. */
function validateAgent(raw: string | undefined): string | null {
  if (!raw || raw.length > 24 || raw === "claude") return null;
  if (!/^[a-z0-9-]+$/.test(raw)) return null;
  return raw;
}

const FALLBACK_COLORS = ["#22C55E", "#EAB308", "#60A5FA", "#E879F9"];

function agentColor(name: string): string {
  let h = 0;
  for (let i = 0; i < name.length; i++) {
    h = (Math.imul(31, h) + name.charCodeAt(i)) | 0;
  }
  return FALLBACK_COLORS[Math.abs(h) % FALLBACK_COLORS.length];
}

function lastPathComponent(p: string): string {
  const cleaned = p.replace(/[\\/]+$/, "");
  const idx = Math.max(cleaned.lastIndexOf("\\"), cleaned.lastIndexOf("/"));
  return idx >= 0 ? cleaned.slice(idx + 1) : cleaned;
}

/** Ticker verbs per tool. */
const TOOL_LABELS: Record<string, string> = {
  Bash: "Runs",
  Read: "Reads",
  Write: "Writes",
  Edit: "Edits",
  Glob: "Finds",
  Grep: "Searches",
  WebSearch: "Web search",
  WebFetch: "Fetches",
  TodoWrite: "Tasks",
  Task: "Agent",
  LS: "Lists",
  MultiEdit: "Edits",
  NotebookEdit: "Notebook",
  PowerShell: "Runs",
};

function stepLabel(tool: string, input: Record<string, unknown>): string {
  const label = TOOL_LABELS[tool] ?? tool;
  const str = (k: string) => (typeof input[k] === "string" ? (input[k] as string) : null);
  const cmd = str("command");
  if (cmd) return `${label} · ${cmd.slice(0, 40)}`;
  const path = str("path");
  if (path) return `${label} · ${lastPathComponent(path)}`;
  const file = str("file_path");
  if (file) return `${label} · ${lastPathComponent(file)}`;
  const url = str("url");
  if (url) return `${label} · ${url.replace(/^https?:\/\//, "").slice(0, 40)}`;
  const query = str("query");
  if (query) return `${label} · ${query.slice(0, 40)}`;
  return label;
}

/**
 * The ticker line for a PreToolUse that carries Zuko's verdict: a block or a
 * question leads with what Zuko did, everything else is the plain step label.
 */
function zukoStepLabel(tool: string, input: Record<string, unknown>, z: ZukoHookInfo | undefined): string {
  const plain = stepLabel(tool, input);
  if (!z) return plain;
  const headline = z.headline.trim();
  switch (z.verdict) {
    case "deny":
      return `Blocked · ${headline || plain}`;
    case "ask":
      return `Asks · ${headline || plain}`;
    default:
      return z.rehydrated.length ? `${plain} · filled ${z.rehydrated.length}` : plain;
  }
}

/**
 * What the Allow button actually authorises. Approving "Write" tells you nothing
 * — approving `Write · C:\…\.env` tells you everything, and the difference is
 * the whole point of approving from the island rather than blind.
 *
 * Ordered by how specific the field is, so an unfamiliar tool still shows
 * whatever identifying string it carries instead of falling back to its name.
 */
const APPROVAL_FIELDS = [
  "command", // Bash, PowerShell
  "file_path", // Write, Edit, MultiEdit, NotebookEdit
  "path", // Read, LS
  "url", // WebFetch
  "query", // WebSearch
  "pattern", // Glob, Grep
  "prompt", // Task
] as const;

function approvalTarget(tool: string, input: Record<string, unknown>): string {
  for (const field of APPROVAL_FIELDS) {
    const value = input[field];
    if (typeof value === "string" && value.trim()) {
      return `${tool} · ${value.trim()}`;
    }
  }
  return tool;
}

function upsert(projectName: string, cwd: string, sessionId?: string) {
  const t = State.task(CLAUDE_ID);
  if (!t) return;
  t.name = projectName;
  if (cwd) t.sessionCwd = cwd;
  // "Open terminal" asks Rust for the terminal this session runs in, by this id.
  if (sessionId) t.sessionId = sessionId;
}

function clearSession() {
  const t = State.task(CLAUDE_ID);
  if (!t) return;
  t.steps = [];
  t.stepIndex = 0;
  t.name = CLAUDE_NAME;
  t.pillBadge = null;
}

/** A red badge on the Policy pill, gone again after a few seconds. */
function flagPolicy() {
  State.setPillBadge(POLICY_ID, "denied");
  if (policyBadgeTimer != null) window.clearTimeout(policyBadgeTimer);
  policyBadgeTimer = window.setTimeout(() => {
    policyBadgeTimer = null;
    if (State.task(POLICY_ID)?.pillBadge === "denied") State.setPillBadge(POLICY_ID, null);
  }, 8000);
}

export function registerHookHandlers(island: Island) {
  void onEvent("hook", (payload) => handleHook(island, payload));
}

/** Zuko's own events: the activity feed, privacy notices and protection status. */
export function registerZukoHandlers(island: Island) {
  void onEvent("activity", (item) => {
    const fresh = !State.activity.some((a) => a.id === item.id);
    State.pushActivity(item);
    // A firewall block: Zuko throws a fireball at it (the PreToolUse hook may
    // have reported the same block a moment ago; the island throws one punch).
    if (fresh && item.verdict === "deny" && !State.paused) island.fireBlock();
  });
  void onEvent("protection-changed", (status) => State.setProtection(status));
  // The local AI's explanation of a card or a feed item, whenever it is ready.
  void onEvent("ai-explain", (e) => State.applyExplanation(e));
  void onEvent("privacy", (event) => handlePrivacy(island, event));
}

/** Loads the status and the recent feed once at boot. */
export async function seedZukoState() {
  const [status, recent] = await Promise.all([
    Bridge.protectionStatus(),
    Bridge.activityRecent(200),
  ]);
  if (status) State.setProtection(status);
  if (recent) State.seedActivity(recent);
}

/** The last notice put on screen, to keep a busy gateway from re-alerting. */
let lastNotice = { sig: "", at: 0 };
const NOTICE_REPEAT_MS = 60_000;

function handlePrivacy(island: Island, event: PrivacyEvent) {
  if (State.paused) return;
  // Restores are routine and happen on every answer in gateway mode.
  if (event.direction === "rehydrated") return;
  const pill = event.source === "browser" ? BROWSER_ID
    : event.source === "gateway" ? GATEWAY_ID
      : event.source === "hook" ? CLAUDE_ID : null;
  if (pill && State.focusId !== pill) State.setPillBadge(pill, "masked");

  // The gateway re-masks the whole history on every turn: the same values a
  // minute later are a badge, not a new notice. New values and blocked prompts
  // always get one.
  const sig = `${event.direction}|${[...event.keys].sort().join(",")}`;
  const now = Date.now();
  const repeat = event.direction === "masked" && event.newKeys.length === 0 &&
    sig === lastNotice.sig && now - lastNotice.at < NOTICE_REPEAT_MS;
  if (repeat) {
    State.notify();
    return;
  }
  lastNotice = { sig, at: now };
  State.privacyNotice = event;
  Sound.play(event.direction === "blocked_prompt" ? "error" : "blip");
  island.showPrivacy();
  // Masked: Zuko burns the secret into a placeholder with a puff of fire.
  if (event.direction === "masked" && !State.pendingApproval) island.fireFlickAtPrivacy();
  State.notify();
}

function handleHook(island: Island, payload: HookEventPayload) {
  if (State.paused) {
    // Silence here used to cost Claude Code nearly two minutes: the relay waited
    // for a decision from an island that had already decided not to look. Say so,
    // and the terminal takes the question immediately.
    if (payload.request_id) void Bridge.approvalDecline(payload.request_id);
    return;
  }

  const name = payload.hook_event_name ?? "";
  const cwd = payload.cwd ?? "";
  const sessionId = payload.session_id ?? "";
  const projectName = lastPathComponent(cwd) || "Session";

  // Route to the right pill. Valid zuko_agent → dynamic "agent_<name>" pill.
  // "claude" is reserved; absent or invalid → Claude Code pill unchanged.
  const validAgent = validateAgent(payload.zuko_agent);
  const agentId = validAgent ? `agent_${validAgent}` : CLAUDE_ID;
  const isExternalAgent = validAgent !== null;

  const focused = State.focusId === agentId;

  /** Alerts force the island open; work events only reveal the compact island. */
  const surface = (view: Parameters<Island["alert"]>[0], isAlert: boolean) => {
    if (State.mode === "expanded") {
      if (isAlert) island.setView(view);
    } else if (isAlert) {
      island.alert(view);
    } else if (State.mode === "hidden") {
      island.reveal();
    }
  };

  /** Ensure the agent pill exists (no-op for Claude Code). */
  const ensurePill = () => {
    if (isExternalAgent) {
      State.upsertExternalAgent(agentId, validAgent!, agentColor(validAgent!));
      const agent = State.task(agentId);
      if (agent) {
        if (cwd) agent.sessionCwd = cwd;
        if (sessionId) agent.sessionId = sessionId;
      }
    } else {
      upsert(projectName, cwd, sessionId);
    }
  };

  switch (name) {
    case "SessionStart":
      ensurePill();
      surface("overview", false);
      Sound.play("work");
      break;

    case "UserPromptSubmit": {
      ensurePill();
      State.updateTask(agentId, "thinking");
      // The field is `prompt`; reading `message` meant this step was always blank.
      const asked = payload.prompt ?? payload.message;
      if (asked) State.appendStep(agentId, asked.slice(0, 60));
      surface("overview", false);
      break;
    }

    case "PreToolUse": {
      ensurePill();
      const tool = payload.tool_name ?? "Tool";
      const z = payload.zuko;
      State.appendStep(agentId, zukoStepLabel(tool, payload.tool_input ?? {}, z));
      if (z?.verdict === "deny") {
        // Zuko said no: the agent reads the reason and adapts; the human gets a
        // red badge on the pill and on Policy, not an alert.
        State.updateTask(agentId, "error");
        if (!focused) State.setPillBadge(agentId, "denied");
        flagPolicy();
        Sound.play("error");
        // ...and Zuko fire-punches the blocked line on the island.
        island.fireBlock();
        window.setTimeout(() => {
          if (State.task(agentId)?.state === "error") State.updateTask(agentId, "working");
        }, 1600);
      } else {
        State.updateTask(agentId, "working");
      }
      surface("overview", false);
      break;
    }

    case "PostToolUse":
      State.updateTask(agentId, "working");
      break;

    case "PostToolUseFailure":
      State.updateTask(agentId, "working");
      State.appendStep(agentId, "Failed");
      break;

    case "Notification": {
      const message = payload.message ?? "";
      const lower = message.toLowerCase();
      if (lower.includes("rate limit") || lower.includes("usage limit") || lower.includes("limit reached")) {
        State.updateTask(agentId, "ratelimit");
        Sound.play("rate");
      } else if (message.endsWith("?")) {
        State.updateTask(agentId, "question");
        State.appendStep(agentId, message);
      }
      break;
    }

    case "Stop":
      State.updateTask(agentId, "finished");
      if (payload.message) State.appendStep(agentId, payload.message.slice(0, 60));
      Sound.play("finish");
      if (focused) surface("finished", true);
      else State.setPillBadge(agentId, "finished");
      window.setTimeout(() => {
        if (isExternalAgent) {
          State.removeTask(agentId);
        } else {
          State.updateTask(agentId, "idle");
          State.setPillBadge(agentId, null);
        }
      }, 5200);
      break;

    case "StopFailure":
      State.updateTask(agentId, "error");
      Sound.play("error");
      if (focused) surface("error", true);
      else State.setPillBadge(agentId, "error");
      break;

    case "SessionEnd":
      if (isExternalAgent) {
        State.removeTask(agentId);
      } else {
        State.updateTask(agentId, "idle");
        clearSession();
      }
      break;

    case "SubagentStart":
      State.appendStep(agentId, "+ subagent");
      break;

    case "SubagentStop":
      State.appendStep(agentId, "Subagent done");
      break;

    case "PermissionRequest": {
      // External agents do not get an approval card — showing one would look like
      // a Claude Code request. Decline immediately so the agent re-asks in its
      // terminal.
      if (isExternalAgent) {
        if (payload.request_id) void Bridge.approvalDecline(payload.request_id);
        break;
      }

      const requestId = payload.request_id ?? "";
      // One card, one request. A second one must never quietly replace the first
      // — that would leave a human staring at request B while request A waits for
      // a decision nobody can give. Hand it straight back to the terminal.
      if (State.pendingApproval && State.pendingApproval.requestId !== requestId) {
        if (requestId) void Bridge.approvalDecline(requestId);
        break;
      }
      upsert(projectName, cwd, sessionId);
      if (pendingTimeout != null) window.clearTimeout(pendingTimeout);
      const tool = payload.tool_name ?? "Tool";
      const input = payload.tool_input ?? {};
      State.pendingApproval = {
        requestId,
        sessionId: payload.session_id ?? "",
        tool,
        command: approvalTarget(tool, input),
        zuko: payload.zuko ?? null,
        receivedAt: performance.now(),
        aiExplanation: null,
      };
      // The relay's short ack window closes in 800 ms; everything below this
      // line is synchronous, so the card really is up by the time it lands.
      if (requestId) void Bridge.approvalAck(requestId);
      State.updateTask(CLAUDE_ID, "approval");
      State.isPinned = true;
      Sound.play("approval");
      // A risk card always takes the island: the other pills are Zuko's own
      // surfaces, so there is no other session's view to protect.
      State.focusId = CLAUDE_ID;
      State.setPillBadge(CLAUDE_ID, null);
      island.alert("approval");
      // Zuko answers within 108 s or not at all; after that the terminal has
      // taken over and the card would be lying.
      pendingTimeout = window.setTimeout(() => {
        pendingTimeout = null;
        if (!State.pendingApproval) return;
        State.pendingApproval = null;
        State.isPinned = false;
        island.dropPin();
        State.updateTask(CLAUDE_ID, "working");
        State.setPillBadge(CLAUDE_ID, null);
        if (State.view === "approval") island.leaveAlert();
        State.notify();
      }, 110_000);
      break;
    }

    default:
      break;
  }
  State.notify();
}
