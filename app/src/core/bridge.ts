// Thin wrapper over the Tauri commands/events. Every call is a no-op when the
// page is opened in a plain browser, so the island can be iterated on with
// `npm run dev` alone. Add `?mock=1` to the URL (dev server only) and every call
// is answered by dev/mock.ts with realistic fake data instead.
//
// The command and event shapes below are app/CONTRACTS.md §2–§3, verbatim.

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import type { ChatProvider, Settings } from "./state";

export const IS_TAURI =
  typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

type MockModule = typeof import("../../dev/mock");

// `import.meta.env.DEV` is a literal `false` in a production build, so the whole
// expression folds away and the mock never ships.
const mock: Promise<MockModule> | null =
  import.meta.env.DEV && !IS_TAURI && new URLSearchParams(window.location.search).has("mock")
    ? import("../../dev/mock")
    : null;

/** True when the page runs against dev/mock.ts (`npx vite`, `?mock=1`). */
export const IS_MOCK = mock !== null;

/** The mock module, for the dev-only scene hooks in main.ts. Null outside mock mode. */
export function mockModule(): Promise<MockModule> | null {
  return mock;
}

async function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T | null> {
  if (mock) {
    try {
      return await (await mock).mockInvoke<T>(cmd, args);
    } catch (err) {
      console.error(`[zuko:mock] ${cmd} failed`, err);
      return null;
    }
  }
  if (!IS_TAURI) return null;
  try {
    return await invoke<T>(cmd, args);
  } catch (err) {
    console.error(`[zuko] ${cmd} failed`, err);
    return null;
  }
}

/** Same as `call`, but surfaces the error so the UI can show what went wrong. */
async function callOrThrow<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  if (mock) return (await mock).mockInvoke<T>(cmd, args);
  if (!IS_TAURI) throw new Error("not running inside Zuko");
  return invoke<T>(cmd, args);
}

/** Error text as the user should read it: Tauri rejects with bare strings. */
export function errorText(err: unknown): string {
  return String(err instanceof Error ? err.message : err).replace(/^Error:\s*/, "");
}

export interface BootInfo {
  settings: Settings;
  /** Logical screen rect of the monitor the island lives on. */
  screen: { x: number; y: number; width: number; height: number; scale: number };
  version: string;
  hookPath: string;
  /** False where the OS has no global cursor (Wayland): see Island.followPageCursor. */
  cursorPoll: boolean;
}

export const Bridge = {
  boot: () => call<BootInfo>("boot"),

  saveSettings: (settings: Settings) => call<void>("save_settings", { settings }),

  /** Shrink the window down to the invisible wake strip (hidden) or back to full. */
  setCollapsed: (collapsed: boolean) => call<void>("set_collapsed", { collapsed }),

  /**
   * Pushes the island shape in window coordinates. Rust flips click-through from
   * its own cursor poll, so the flag is never a frame behind a click.
   */
  setIslandRect: (x: number, y: number, width: number, height: number) =>
    call<void>("set_island_rect", { x, y, width, height }),

  /** Give the window keyboard focus (chat field) and take it away again. */
  focusWindow: (focused: boolean) => call<void>("focus_window", { focused }),

  reposition: () => call<void>("reposition"),

  openUrl: (url: string) => call<void>("open_url", { url }),

  /** "Open terminal" → opens the folder in VS Code when `code` is on PATH. */
  openInVSCode: (path: string | null) => call<boolean>("open_in_vscode", { path }),

  /**
   * "Open file" on an activity row: VS Code, in the window that has the session folder
   * `cwd` open (`code <cwd> --goto <path>`), or the file's folder in Explorer. False when
   * it did not open in VS Code.
   */
  openFile: (path: string, cwd?: string | null) => call<boolean>("open_file", { path, cwd: cwd ?? null }),

  quit: () => call<void>("quit_app"),

  openSettingsWindow: () => call<void>("open_settings_window"),

  /** Writes to %LOCALAPPDATA%\Zuko\zuko.log, next to the Rust lines. */
  log: (message: string) => call<void>("log_line", { message }),

  /** Tray → Pause. The island stops reacting and hands approvals to the terminal. */
  setPaused: (paused: boolean) => call<void>("set_paused", { paused }),

  // ── Claude Code hooks (legacy installer, kept for the status paths) ───────
  hooksStatus: () => call<HookStatus>("hooks_status"),
  /** Diff to show before anything is written. `install: false` previews removal. */
  hooksPreview: (install: boolean) => callOrThrow<HookPreview>("hooks_preview", { install }),
  /**
   * Writes ~/.claude/settings.json — only ever after an explicit click, and only
   * when the file still matches the preview the user looked at.
   */
  hooksApply: (install: boolean, fingerprint: string) =>
    callOrThrow<string>("hooks_apply", { install, fingerprint }),

  // ── Approvals ─────────────────────────────────────────────────────────────
  /**
   * The user's answer to a PermissionRequest. `elapsedMs` is how long the card
   * was on screen, which Rust uses (with the island) to spot rubber-stamping.
   */
  approvalDecision: (requestId: string, decision: "allow" | "deny", elapsedMs?: number) =>
    call<void>("approval_decision", {
      requestId,
      decision,
      elapsedMs: elapsedMs == null ? undefined : Math.max(0, Math.round(elapsedMs)),
    }),
  /** "The card is up" — until this lands the relay only waits a moment. */
  approvalAck: (requestId: string) => call<void>("approval_ack", { requestId }),
  /** "Nobody can act on this" — Claude Code asks in the terminal right away. */
  approvalDecline: (requestId: string) => call<void>("approval_decline", { requestId }),

  // ── Protection (hooks + gateway + deny rules in ~/.claude/settings.json) ──
  protectionStatus: () => call<ProtectionStatus>("protection_status"),
  /** The exact settings.json diff `protectionApply` would write. Nothing is touched. */
  protectionPreview: (options: InstallOptions) =>
    callOrThrow<HookPreview>("protection_preview", { options }),
  /** Writes the reviewed diff; refused if settings.json changed since the preview. */
  protectionApply: (options: InstallOptions, fingerprint: string) =>
    callOrThrow<string>("protection_apply", { options, fingerprint }),

  // ── Policy ────────────────────────────────────────────────────────────────
  policyGet: () => call<Policy>("policy_get"),
  /** Rejects with a readable message when the policy is invalid. */
  policySet: (policy: Policy) => callOrThrow<void>("policy_set", { policy }),
  /** Saves and returns the default policy. */
  policyReset: () => callOrThrow<Policy>("policy_reset"),

  // ── Local AI (optional Ollama on this machine) ────────────────────────────
  /** Is Ollama reachable, is the model installed. `config`: an unsaved draft. */
  localaiStatus: (config?: LocalAiConfig) => callOrThrow<LocalAiStatus>("localai_status", { config: config ?? null }),
  /** Deep-scans a made-up sentence; never touches the vault. */
  localaiTest: (config?: LocalAiConfig) => callOrThrow<LocalAiTest>("localai_test", { config: config ?? null }),

  // ── Vault (values never reach the webview, except through vaultReveal) ───
  vaultList: () => call<EntryView[]>("vault_list"),
  /** Returns the new key (e.g. `API_KEY_3`). */
  vaultAdd: (value: string, kind: string, label: string) =>
    callOrThrow<string>("vault_add", { value, kind, label }),
  vaultForget: (key: string) => callOrThrow<boolean>("vault_forget", { key }),
  vaultClear: () => callOrThrow<void>("vault_clear"),
  /** The real value. Only ever called from an explicit click on Reveal. */
  vaultReveal: (key: string) => callOrThrow<string | null>("vault_reveal", { key }),
  /** Non-sensitive facts about a stored value (computed locally); null if the key is gone. */
  vaultInsights: (key: string) => callOrThrow<VaultInsight[] | null>("vault_insights", { key }),
  /** Copies the value to the clipboard on the Rust side (it never reaches the webview) and clears it after 30 s if unchanged. False if the key is gone. */
  vaultCopy: (key: string) => callOrThrow<boolean>("vault_copy", { key }),

  // ── Browser bridge (the extension's native messaging host) ────────────────
  /** Is `app.zuko.host` registered for this user's browsers. */
  nativeHostStatus: () => call<NativeHostStatus>("native_host_status"),
  /** Register (true) or unregister (false) the host; the choice sticks across launches. Explicit clicks only. */
  nativeHostSet: (enabled: boolean) => callOrThrow<NativeHostStatus>("native_host_set", { enabled }),

  // ── Activity and audit ────────────────────────────────────────────────────
  /** Newest first. */
  activityRecent: (limit: number) => call<ActivityItem[]>("activity_recent", { limit }),
  auditVerify: () => callOrThrow<AuditVerifyResult>("audit_verify"),
  auditOpenFolder: () => call<void>("audit_open_folder"),

  // ── Documents and clipboard ───────────────────────────────────────────────
  maskText: (text: string) => callOrThrow<MaskTextResult>("mask_text", { text }),
  unmaskText: (text: string) => callOrThrow<UnmaskTextResult>("unmask_text", { text }),
  /** Scans a file and writes `<inbox>/<stem>.zuko.md` with every finding masked. */
  sanitizeFile: (path: string) => callOrThrow<SanitizeResult>("sanitize_file", { path }),
  /** Shows a file in Explorer. */
  revealPath: (path: string) => call<void>("reveal_path", { path }),
  /** Masks the clipboard text in place. */
  clipboardMask: () => callOrThrow<{ count: number }>("clipboard_mask"),
  clipboardUnmask: () => callOrThrow<{ count: number }>("clipboard_unmask"),

  // ── Chat, files, secrets ──────────────────────────────────────────────────
  /** One chat turn with the provider chosen in Settings → Chat. API keys and file bytes never leave Rust. */
  chatSend: (query: string, context: ChatContext | null) =>
    callOrThrow<ChatReply>("chat_send", { query, context }),
  chatReset: () => call<void>("chat_reset"),
  /**
   * The model dropdown for one provider. OpenAI's list is fetched by Rust with the
   * stored key (the key never reaches the webview); Ollama's lists installed models.
   */
  chatModels: (provider: ChatProvider) => callOrThrow<ChatModels>("chat_models", { provider }),
  /** Can the chat work with `provider` (default: the chosen one)? Nothing is sent to a cloud provider. */
  chatStatus: (provider?: ChatProvider) => callOrThrow<ChatStatus>("chat_status", { provider: provider ?? null }),
  /** Copies a dropped file into the inbox. */
  ingestFile: (path: string) => callOrThrow<DroppedFile>("ingest_file", { path }),
  /** Only ever tells you whether a key exists — never its value. */
  secretPresent: (key: string) => call<boolean>("secret_present", { key }),
  secretSet: (key: string, value: string) => callOrThrow<void>("secret_set", { key, value }),
  secretClear: (key: string) => callOrThrow<void>("secret_clear", { key }),
};

// ── Shared shapes ─────────────────────────────────────────────────────────────

export type ChatContext =
  | { kind: "file"; name: string; path: string }
  | { kind: "window"; appName: string; title: string; url?: string };

export interface ChatReply {
  /** The answer, with placeholders restored on this machine. */
  text: string;
  /** What was masked before the turn was sent (keys and labels, never values). */
  masked: { key: string; label: string }[];
}

export interface ChatModels {
  provider: ChatProvider;
  /** Sorted. Claude: Zuko's list; OpenAI: chat models the key can use; Ollama: installed models. */
  models: string[];
  /** The saved model, or (OpenAI) a sensible default when the saved one is not offered any more. */
  selected: string;
  /** Why the list could not be fetched (no key, offline, Ollama not running…). */
  error: string | null;
}

export interface ChatStatus {
  provider: ChatProvider;
  /** "Claude" | "OpenAI" | "Ollama" */
  label: string;
  model: string;
  /** True when the masked conversation leaves this machine. */
  cloud: boolean;
  ready: boolean;
  /** Cloud providers: whether a key is stored (never the key). Null for Ollama. */
  keyPresent: boolean | null;
  /** Ollama only: the local AI endpoint the chat uses, and what /api/tags said. */
  endpoint: string | null;
  reachable: boolean | null;
  modelPresent: boolean | null;
  error: string | null;
  /** e.g. `ollama pull gemma3:4b` */
  hint: string | null;
}

export interface DroppedFile {
  name: string;
  path: string;
  size: number;
}

export interface HookStatus {
  installed: boolean;
  settingsPath: string;
  hookPath: string;
  hookReady: boolean;
}

export interface HookPreview {
  diff: string;
  backup: string;
  settingsPath: string;
  /** Hand back to the apply call so only the reviewed diff is ever written. */
  fingerprint: string;
}

// ── Engine types (zuko-core, camelCase JSON) ──────────────────────────────────

export type Tier = "low" | "medium" | "high" | "critical";
export type Verdict = "allow" | "ask" | "deny" | "defer";
export type RuleVerdict = "allow" | "ask" | "deny";
export type PolicyMode = "enforce" | "monitor";

export const TIERS: readonly Tier[] = ["low", "medium", "high", "critical"];

/** Index into TIERS: low 0 … critical 3. */
export function tierRank(t: Tier): number {
  return TIERS.indexOf(t);
}

export interface InstallOptions {
  /** Zuko's hook entries in ~/.claude/settings.json. */
  hooks: boolean;
  /** env.ANTHROPIC_BASE_URL → Zuko gateway (+ privacy env vars). */
  gateway: boolean;
  /** Mirror policy blocked paths/domains into permissions.deny. */
  denyRules: boolean;
}

export interface ProtectionStatus {
  hooksInstalled: boolean;
  /** Relay exe in place. */
  hookReady: boolean;
  /** settings.json env points at our gateway. */
  gatewayConfigured: boolean;
  gatewayRunning: boolean;
  /** Includes the path token; shown masked in the UI. */
  gatewayUrl: string | null;
  gatewayPort: number;
  denyRulesInstalled: boolean;
  mode: PolicyMode;
  vaultSize: number;
  // The four totals are today's (local date, back to 0 at midnight), counted from the
  // audit log, so they survive a restart and match the activity feed's filters.
  /** Values masked today (each masking counts the distinct values it replaced). */
  maskedTotal: number;
  /** Actions denied (policy or human) and prompts held back today. */
  blockedTotal: number;
  /** Actions Zuko asked about today. */
  askedTotal: number;
  /** Low-risk actions allowed without asking today. */
  autoAllowedTotal: number;
  /** The browser extension was heard from in the last 60 s (it says hello every 25 s while linked). */
  extensionConnected: boolean;
  policyPath: string;
  auditPath: string;
}

/** One browser that can start the native host. */
export interface NativeHostBrowser {
  /** "Chrome" | "Edge" | "Chromium" | "Brave" */
  name: string;
  /** The browser has settings for this user. */
  installed: boolean;
  /** It points at Zuko's host manifest. */
  registered: boolean;
}

/** The browser bridge: `app.zuko.host` registered for the current user. */
export interface NativeHostStatus {
  /** The manifest is right and every browser that should know the host does. */
  registered: boolean;
  /** Zuko registers it at every launch (`Settings.browserBridge`). */
  enabled: boolean;
  browsers: NativeHostBrowser[];
  /** `%LOCALAPPDATA%\Zuko\native-host\app.zuko.host.json` */
  manifestPath: string;
  /** The installed native host the manifest points at. */
  hostPath: string;
  hostPresent: boolean;
  extensionId: string;
  /** Why the last register / unregister failed. */
  error: string | null;
}

export type ActivityVerdict =
  | "allow" | "ask" | "deny" | "defer"
  | "approved" | "denied" | "masked" | "rehydrated" | "blocked_prompt";

export interface ActivityItem {
  /** Unique. */
  id: string;
  /** Unix ms. */
  ts: number;
  sessionId: string;
  /** Last path component of cwd. */
  project: string;
  /** PreToolUse | PermissionRequest | UserPromptSubmit | PostToolUse | Gateway | Browser | File | Chat */
  event: string;
  tool: string;
  /** Masked. */
  summary: string;
  /** One of ActivityVerdict; kept open so a newer app never breaks the feed. */
  verdict: ActivityVerdict | (string & {});
  tier: Tier;
  score: number;
  headline: string;
  /** Policy rule ids + invariant ids. */
  rules: string[];
  /** Vault keys involved. */
  keys: string[];
  /** Local AI explanation (display only), when one arrived for this item. */
  aiExplanation?: string;
  /** Absolute path of the file a Write/Edit/MultiEdit/NotebookEdit call targets ("Open file"). Live items only. */
  path?: string;
  /** The session's working folder, next to `path`: "Open file" opens the file in the VS Code window that has it open. */
  cwd?: string;
}

/** One non-sensitive fact about a vault value (never the value itself). */
export interface VaultInsight {
  label: string;
  text: string;
}

export interface SanitizeFinding {
  key: string;
  kind: string;
  label: string;
  count: number;
}

export interface SanitizeResult {
  name: string;
  inputPath: string;
  /** `<inbox>/<stem>.zuko.md` */
  outputPath: string;
  kind: "text" | "markdown" | "pdf" | "code";
  /** PDFs only. */
  pages: number | null;
  findings: SanitizeFinding[];
  /** First ~1500 chars of the masked output. */
  preview: string;
  /** e.g. "No text layer found (scanned PDF?)" */
  warnings: string[];
  /** What the local AI deep scan added; null when it is off. */
  aiDeepScan: AiDeepScan | null;
}

export interface AiDeepScan {
  /** Values the model found (verified by Zuko) that the patterns had not masked. */
  items: number;
  /** Of those, values the vault had never seen. */
  newItems: number;
  model: string;
  ms: number;
  error: string | null;
}

/** `localAi` in the policy. The local model may only make Zuko stricter. */
export interface LocalAiConfig {
  enabled: boolean;
  /** Loopback only: http://127.0.0.1, http://localhost or http://[::1]. */
  endpoint: string;
  model: string;
  deepScanPrompts: boolean;
  deepScanDocuments: boolean;
  explainRisk: boolean;
  /** The gateway waits up to timeoutMs for the scan of the newest prompt. */
  waitForPromptScan: boolean;
  timeoutMs: number;
}

export interface LocalAiStatus {
  enabled: boolean;
  endpoint: string;
  model: string;
  endpointOk: boolean;
  reachable: boolean;
  modelPresent: boolean;
  models: string[];
  error: string | null;
  /** e.g. `ollama pull gemma3:4b` */
  hint: string | null;
}

export interface AiFinding {
  kind: string;
  value: string;
  label: string;
}

export interface LocalAiTest {
  sample: string;
  findings: AiFinding[];
  ms: number;
  error: string | null;
}

/** `ai-explain`: a local AI explanation for an approval card or a feed item. */
export interface AiExplainEvent {
  requestId: string | null;
  activityId: string | null;
  text: string;
  model: string;
}

export interface RiskFactor {
  id: string;
  weight: number;
  /** One plain-English sentence. */
  text: string;
}

export interface ImpactVector {
  /** reversible | partial | irreversible */
  reversibility: string;
  /** none | project | user | system */
  blastRadius: string;
  /** none | known_host | unknown_host */
  egress: string;
  /** public | internal | secret */
  sensitivity: string;
  privilege: boolean;
  obfuscated: boolean;
}

export type Friction =
  | { type: "none" }
  | { type: "hold"; ms: number }
  | { type: "blocked" };

export interface Violation {
  id: string;
  verdict: string;
  reason: string;
  triggeredBy: number[];
}

/** The `zuko` object on PreToolUse and PermissionRequest hook payloads. */
export interface ZukoHookInfo {
  verdict: Verdict;
  tier: Tier;
  score: number;
  headline: string;
  factors: RiskFactor[];
  vector: ImpactVector;
  reasonUser: string;
  friction: Friction;
  rules: string[];
  violations: Violation[];
  /** Vault keys filled into the input. */
  rehydrated: string[];
}

export interface PrivacyEvent {
  source: "gateway" | "hook" | "chat" | "file" | "browser" | "clipboard";
  direction: "masked" | "rehydrated" | "blocked_prompt";
  count: number;
  keys: string[];
  /** Human labels, same order as keys. */
  labels: string[];
  newKeys: string[];
  sessionId: string | null;
  /** For blocked_prompt: the masked text the user can resend. */
  maskedPrompt: string | null;
}

export type Category = "secret" | "pii" | "custom";

/** Public, value-free view of a vault entry. */
export interface EntryView {
  /** `API_KEY_1` — no braces. */
  key: string;
  kind: string;
  label: string;
  category: Category;
  hint: string | null;
  /** e.g. `sk-p…9fQa` */
  preview: string;
  /** gateway | hook | chat | file | browser | manual */
  source: string;
  /** Unix seconds. */
  created: number;
  /** Unix seconds. */
  lastUsed: number;
  hits: number;
}

export interface MaskReport {
  count: number;
  keys: string[];
  newKeys: string[];
}

export interface MaskTextResult {
  text: string;
  report: MaskReport;
}

export interface UnmaskTextResult {
  text: string;
  keys: string[];
}

export interface AuditVerifyResult {
  ok: boolean;
  count: number;
  error: string | null;
}

export interface DetectorConfig {
  secrets: boolean;
  pii: boolean;
  emails: boolean;
  phones: boolean;
  cards: boolean;
  ibans: boolean;
  ips: boolean;
  nationalIds: boolean;
  genericEntropy: boolean;
  minEntropy: number;
  customTerms: string[];
  allowlist: string[];
}

export interface Policy {
  version: number;
  mode: PolicyMode;
  network: { blocked: string[]; allowed: string[]; unknown: RuleVerdict };
  filesystem: { blockedRead: string[]; blockedWrite: string[]; sensitive: string[] };
  commands: { blocked: string[]; ask: string[]; allowed: string[] };
  tools: { blocked: string[]; ask: string[]; allowedMcp: string[] };
  approvals: {
    autoAllowLowRisk: boolean;
    holdToApproveFrom: Tier;
    blockFrom: Tier;
    holdMs: number;
  };
  privacy: {
    detector: DetectorConfig;
    blockSecretPromptsWithoutGateway: boolean;
    maskToolOutput: boolean;
    secretHosts: Record<string, string[]>;
  };
  localAi: LocalAiConfig;
}

/** Placeholder kinds the detector emits (zuko-core detect::kinds). */
export const VAULT_KINDS = [
  "API_KEY", "TOKEN", "PRIVATE_KEY", "PASSWORD", "SECRET", "CONN_STRING", "JWT",
  "EMAIL", "PHONE", "CARD", "IBAN", "IP", "NID", "TERM",
] as const;

// ── Events ────────────────────────────────────────────────────────────────────

/** A Claude Code hook payload as the island receives it (strings truncated). */
export interface HookEventPayload {
  hook_event_name?: string;
  request_id?: string;
  session_id?: string;
  cwd?: string;
  message?: string;
  /** UserPromptSubmit carries `prompt`; `message` belongs to Notification/Stop. */
  prompt?: string;
  tool_name?: string;
  tool_input?: Record<string, unknown>;
  /** Optional agent tag: lowercase, digits and hyphens, ≤ 24 chars. */
  zuko_agent?: string;
  /** Zuko's verdict, on PreToolUse and PermissionRequest. */
  zuko?: ZukoHookInfo;
}

export interface EventMap {
  cursor: { x: number; y: number };
  tray: string;
  hook: HookEventPayload;
  "screen-changed": null;
  "settings-changed": Settings;
  activity: ActivityItem;
  privacy: PrivacyEvent;
  "protection-changed": ProtectionStatus;
  "ai-explain": AiExplainEvent;
}

export type BridgeEventName = keyof EventMap;

export interface DragDropPayload {
  type: "enter" | "over" | "drop" | "leave";
  paths?: string[];
}

/** Files dragged onto the window. Only reaches us when the window takes the mouse. */
export async function onDragDrop(handler: (e: DragDropPayload) => void) {
  if (!IS_TAURI) return () => {};
  return getCurrentWebview().onDragDropEvent((event) => {
    handler(event.payload as DragDropPayload);
  });
}

export async function onEvent<K extends BridgeEventName>(
  name: K,
  handler: (payload: EventMap[K]) => void,
): Promise<() => void> {
  if (mock) return (await mock).mockListen(name, handler);
  if (!IS_TAURI) return () => {};
  return listen<EventMap[K]>(name, (e) => handler(e.payload));
}
