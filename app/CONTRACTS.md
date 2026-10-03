# Zuko internal contracts

The interfaces between Zuko's parts. Change a contract here first, then both sides.
Engine types (`Policy`, `Decision`, `RiskReport`, `EntryView`, `Finding`, `MaskReport`…)
are defined in `core/src/*.rs` and serialize as camelCase JSON.

---

## 1. Relay ⇄ app (named pipe `\\.\pipe\zuko-<SID>`, Unix socket `$XDG_RUNTIME_DIR/zuko.sock`)

**Request (hook → app):** one UTF-8 JSON object terminated by `\n`, at most 16 MiB.
It is the hook payload from Claude Code, **untruncated** (including `tool_response` and
`transcript_path`), plus fields the relay adds:

| Field | Meaning |
|---|---|
| `hook_event_name` | always set (from the payload or argv) |
| `zuko_wants_reply` | `true` if the relay will wait for an answer |
| `zuko_env` | `{ "anthropicBaseUrl": <ANTHROPIC_BASE_URL or null>, "home": <home dir> }` from the relay's environment (Claude Code passes its env to hooks) |
| `term_program`, `wt_session`, `term_session_id`, `vscode_pid`, `session_pid` | terminal context (unchanged from Coucou) |
| `zuko_agent` | from `--agent <name>` (third-party agents) |

**Reply (app → hook):** one JSON object terminated by `\n`:
```json
{ "stdout": { ...hook output object... } }   // printed verbatim by the relay
{ "stdout": null }                            // print nothing ("no opinion")
```
For `hook_event_name: "ZukoExtension"` (native host), the reply is `{"reply": <message>}`.
The relay waits for a reply for `PreToolUse`, `PostToolUse`, `UserPromptSubmit`,
`SessionStart` (budget 1.5 s each) and `PermissionRequest` (budget 110 s). For
`PermissionRequest` the app keeps Coucou's acknowledgement logic internally: if the island
does not confirm the card within 800 ms, the app replies `{"stdout": null}` at once and
Claude Code asks in the terminal; otherwise it replies when the human decides (≤ 108 s).
Each reply is one line; the relay prints `stdout` verbatim. All other events are
fire-and-forget (the app closes the connection without replying).

**App unreachable / timeout:** the relay runs `zuko_core::guard::decide` itself with
`%APPDATA%\Zuko\policy.json` (defaults if missing), no ledger, no vault:
- `PreToolUse`: Deny → print deny. Ask → print ask with reason. Allow/Defer → print nothing
  (the relay never auto-allows on its own).
- `UserPromptSubmit`: if the prompt contains secrets (detector) → block with
  `suppressOriginalPrompt`, telling the user Zuko is not running.
- Everything else: print nothing.

## 2. Tauri commands (frontend → Rust)

Existing Coucou commands keep their names (`boot`, `save_settings`, `set_collapsed`,
`set_island_rect`, `focus_window`, `reposition`, `open_url`, `open_in_vscode`, `quit_app`,
`set_paused`, `hooks_status`, `hooks_preview`, `hooks_apply`, `approval_decision`,
`approval_ack`, `approval_decline`, `log_line`, `chat_send`, `chat_reset`, `ingest_file`,
`secret_present`, `secret_set`, `secret_clear`, `open_settings_window`). Removed:
`refresh_integration`, `open_n8n`.

`save_settings` never changes `islandOffset` (a window that has not heard of the last drag
must not move the island back); only the two commands below do.

New commands (JS argument names are camelCase):

| Command | Args | Returns |
|---|---|---|
| `island_drag` | `phase: "start" \| "move" \| "end", dx: number` | `void` (the island follows a press-and-drag on its header or empty area: `dx` = logical px the pointer moved since the press, from `screenX`; Rust moves the window along the top edge, clamped to the display; "end" saves `Settings.islandOffset` and emits `settings-changed`) |
| `reset_island_position` | — | `void` (back to the top centre; saved, `settings-changed`) |
| `protection_status` | — | `ProtectionStatus` |
| `protection_preview` | `options: InstallOptions` | `HookPreview` (same shape as `hooks_preview`) |
| `protection_apply` | `options: InstallOptions, fingerprint: string` | `string` (backup path) |
| `policy_get` | — | `Policy` |
| `policy_set` | `policy: Policy` | `void` (error string on invalid) |
| `policy_reset` | — | `Policy` (the defaults, saved) |
| `vault_list` | — | `EntryView[]` |
| `vault_add` | `value: string, kind: string, label: string` | `string` (key) |
| `vault_forget` | `key: string` | `boolean` |
| `vault_clear` | — | `void` |
| `vault_reveal` | `key: string` | `string \| null` (explicit user click only) |
| `vault_insights` | `key: string` | `{ label: string, text: string }[] \| null` (local, non-sensitive facts: never the value or more than its last four digits; display only) |
| `vault_copy` | `key: string` | `boolean` (explicit click only; the Rust side copies the value to the clipboard, so it never reaches the webview, and clears it after 30 s only if the clipboard still holds it; audit receipt event `VaultCopy`, keys only) |
| `open_file` | `path: string, cwd: string \| null` | `boolean` (existing absolute file only. VS Code on PATH: `code <cwd> --goto <path>` when `cwd` is an existing absolute folder that contains the file, so the window already running that session gets it; otherwise `code --reuse-window --goto <path>`. Each value its own argument, no shell. Without VS Code: shows the folder. True when VS Code opened it) |
| `native_host_status` | — | `NativeHostStatus` (the browser bridge: is `app.zuko.host` registered for this user) |
| `native_host_set` | `enabled: boolean` | `NativeHostStatus` (explicit click only: registers or unregisters now and stores `Settings.browserBridge`, so an unregistered bridge stays off across launches; audited; a failure comes back in `error`) |
| `activity_recent` | `limit: number` | `ActivityItem[]` (newest first) |
| `audit_verify` | — | `{ ok: boolean, count: number, error: string \| null }` |
| `audit_open_folder` | — | `void` |
| `mask_text` | `text: string` | `{ text: string, report: MaskReport }` |
| `unmask_text` | `text: string` | `{ text: string, keys: string[] }` |
| `sanitize_file` | `path: string` | `SanitizeResult` |
| `reveal_path` | `path: string` | `void` (shows a file in Explorer) |
| `clipboard_mask` | — | `{ count: number }` (masks the clipboard text in place) |
| `clipboard_unmask` | — | `{ count: number }` |
| `localai_status` | `config: LocalAiConfig \| null` (unsaved draft; null = saved policy) | `LocalAiStatus` |
| `localai_test` | `config: LocalAiConfig \| null` | `LocalAiTest` (deep scan of a made-up sentence; never touches the vault) |
| `chat_models` | `provider: ChatProvider` | `ChatModels` (the Settings → Chat dropdown; OpenAI's list is fetched in Rust with the stored key, Ollama's from `/api/tags` on the loopback endpoint) |
| `chat_status` | `provider: ChatProvider \| null` (null = the chosen one) | `ChatStatus` (key presence for the cloud providers, nothing is sent; Ollama reachability and model) |

`approval_decision` gains an optional `elapsedMs: number` (time the card was on screen),
used for rubber-stamp detection.

`chat_send` (`query: string, context: ChatContext | null` → `ChatReply`) talks to the
provider chosen in `Settings.chatProvider` with that provider's model. Same privacy
pipeline for every provider: the message, window context and attached text/PDF text are
masked with the engine (source `"chat"`), the placeholder legend is appended to the
system prompt, the reply is rehydrated locally, a `privacy` event is emitted, images and
scanned PDFs are refused. A conversation belongs to one provider: the first turn with
another provider starts a new one (the island clears its bubbles when the provider
changes). `secret_present` / `secret_set` / `secret_clear` accept `"anthropic-api-key"`
and `"openai-api-key"` (presence only, never the value; the vault key is out of reach).

### Types
```ts
type Tier = "low" | "medium" | "high" | "critical";
type Verdict = "allow" | "ask" | "deny" | "defer";

interface InstallOptions {
  hooks: boolean;        // Zuko's hook entries in ~/.claude/settings.json
  gateway: boolean;      // env.ANTHROPIC_BASE_URL → Zuko gateway (+ privacy env vars)
  denyRules: boolean;    // mirror policy blocked paths/domains into permissions.deny
}

interface ProtectionStatus {
  hooksInstalled: boolean;
  hookReady: boolean;          // relay exe in place
  gatewayConfigured: boolean;  // settings.json env points at our gateway
  gatewayRunning: boolean;
  gatewayUrl: string | null;   // includes the path token; shown masked in UI
  gatewayPort: number;
  denyRulesInstalled: boolean;
  mode: "enforce" | "monitor";
  vaultSize: number;
  // The four totals are TODAY's (local date; they start again at 0 at midnight). The audit
  // log counts every receipt as it is written and, at startup, today's receipts already in
  // the log, so a restart keeps them. Groups follow the activity feed's filters:
  maskedTotal: number;         // values masked today: each "masked" receipt counts its distinct keys
                               // (a browser report without known keys: the count it reported)
  blockedTotal: number;        // denied by policy (deny), by a human (denied_by_user), prompts held back (blocked_prompt)
  askedTotal: number;          // PreToolUse "ask"
  autoAllowedTotal: number;    // PreToolUse "allow"
  extensionConnected: boolean; // the extension was heard from in the last 60 s (it says hello every 25 s while linked)
  policyPath: string;
  auditPath: string;
}

interface ActivityItem {
  id: string;            // unique
  ts: number;            // unix ms
  sessionId: string;
  project: string;       // last path component of cwd
  event: string;         // PreToolUse | PermissionRequest | UserPromptSubmit | PostToolUse | Gateway | Browser | File | Chat
  tool: string;
  summary: string;       // masked
  verdict: string;       // allow | ask | deny | defer | approved | denied | masked | rehydrated | blocked_prompt
  tier: Tier;
  score: number;
  headline: string;
  rules: string[];       // policy rule ids + invariant ids
  keys: string[];        // vault keys involved
  aiExplanation?: string; // local AI text, attached when `ai-explain` arrives (display only)
  path?: string;         // absolute file path of a Write/Edit/MultiEdit/NotebookEdit call ("Open file"); live items only, never file contents
  cwd?: string;          // the session's absolute working folder, only next to `path` (passed to open_file)
}

interface SanitizeResult {
  name: string;
  inputPath: string;
  outputPath: string;    // <inbox>/<stem>.zuko.md
  kind: "text" | "markdown" | "pdf" | "code";
  pages: number | null;  // PDFs
  findings: { key: string; kind: string; label: string; count: number }[];
  preview: string;       // first ~1500 chars of the masked output
  warnings: string[];    // e.g. "No text layer found (scanned PDF?)"
  aiDeepScan: AiDeepScan | null;  // null when the local AI is off for documents
}

interface AiDeepScan {   // shown as "AI deep scan: +N items"
  items: number;         // verified values the model found that the patterns had not masked
  newItems: number;      // of those, values the vault had never seen
  model: string;
  ms: number;
  error: string | null;  // timeout / unusable answer / partial coverage (also in warnings)
}

// Policy.localAi (serde default: absent in old files = disabled). Global only: a
// project's .zuko/policy.json cannot change it.
interface LocalAiConfig {
  enabled: boolean;            // default false
  endpoint: string;            // default "http://127.0.0.1:11434"; loopback only (127.0.0.1, localhost, [::1])
  model: string;               // default "gemma3:4b"
  deepScanPrompts: boolean;    // default true
  deepScanDocuments: boolean;  // default true
  explainRisk: boolean;        // default true
  waitForPromptScan: boolean;  // default false: gateway waits ≤ timeoutMs for the newest prompt's scan
  timeoutMs: number;           // default 20000, 500..120000
}

interface LocalAiStatus {
  enabled: boolean; endpoint: string; model: string;
  endpointOk: boolean;         // loopback check passed (otherwise nothing is sent)
  reachable: boolean;          // GET /api/tags answered
  modelPresent: boolean;
  models: string[];            // installed models
  error: string | null;
  hint: string | null;         // e.g. "ollama pull gemma3:4b"
}

interface LocalAiTest {
  sample: string;
  findings: { kind: string; value: string; label: string }[];  // each verified as an exact substring
  ms: number;
  error: string | null;
}

// The island chat. Claude and OpenAI get masked text; Ollama runs on this machine.
type ChatProvider = "anthropic" | "openai" | "ollama";

// Settings (settings.json) chat fields; serde defaults, so older files load unchanged.
// An unknown chatProvider (e.g. written by a newer build) reads as "ollama": nothing
// leaves the machine and the other preferences survive.
interface ChatSettings {
  chatProvider: ChatProvider;  // default "anthropic"
  model: string;               // the Claude model (field name predates the others), default "claude-opus-5"
  openaiModel: string;         // default "gpt-5-mini"
  ollamaModel: string;         // default "gemma3:4b"; endpoint = policy localAi.endpoint (loopback only),
                               // used whether or not localAi.enabled is on
}

// Settings (settings.json), serde defaults, so older files load unchanged.
interface PlacementAndBridgeSettings {
  browserBridge: boolean;  // default true: Zuko registers the browser bridge at launch
  islandOffset: number;    // default 0: the island centre's distance from the display centre, as a
                           // fraction of the display width (negative = left); clamped to the display
                           // when applied (launch, display change, drag), shared by the wake strip
}

// The browser bridge (native messaging host `app.zuko.host`, HKCU / ~/.config only).
interface NativeHostStatus {
  registered: boolean;     // the manifest is right and every browser that should know the host does
  enabled: boolean;        // Settings.browserBridge
  browsers: { name: string; installed: boolean; registered: boolean }[];  // Chrome, Edge, Chromium, Brave
  manifestPath: string;    // <local data>\native-host\app.zuko.host.json
  hostPath: string;        // the installed zuko-native-host the manifest points at
  hostPresent: boolean;
  extensionId: string;     // cbdnjagdcfchakoejiahgeclappplcba, the only allowed origin
  error: string | null;    // why the last register / unregister failed
}

interface ChatReply {
  text: string;                               // placeholders restored on this machine
  masked: { key: string; label: string }[];   // what was masked in this turn (never values)
}

interface ChatModels {
  provider: ChatProvider;
  models: string[];        // sorted. OpenAI: GET /v1/models filtered to chat models (gpt-*, chatgpt-*, o<digit>*,
                           // minus image/audio/realtime/tts/transcribe/embedding/moderation/search/instruct/codex/-pro)
  selected: string;        // the saved model; OpenAI: a sensible default when the saved one is not offered
  error: string | null;    // no key, offline, 401, Ollama not running…
}

interface ChatStatus {
  provider: ChatProvider;
  label: string;           // "Claude" | "OpenAI" | "Ollama"
  model: string;
  cloud: boolean;          // the masked conversation leaves this machine
  ready: boolean;
  keyPresent: boolean | null;    // cloud providers only
  endpoint: string | null;       // Ollama only
  reachable: boolean | null;     // Ollama only
  modelPresent: boolean | null;  // Ollama only
  error: string | null;
  hint: string | null;           // e.g. "ollama pull gemma3:4b"
}
```

**Local AI rule:** the local model may only make Zuko stricter. Deterministic masking and
the guard run first and stay authoritative; the model can add vault entries (masked by the
normal passes from then on) and explanation text, never remove a mask or change a verdict.
Its answers are strict JSON, every finding is verified as an exact substring of the text it
was shown, labels are Zuko's own, and failures fall back to the deterministic result.

## 3. Events (Rust → frontend)

| Event | Target | Payload |
|---|---|---|
| `hook` | island | the hook payload (UI copy: strings truncated to 2000 chars) + `request_id` for PermissionRequest + **`zuko`** (below) |
| `activity` | all windows | `ActivityItem` |
| `privacy` | all windows | `PrivacyEvent` |
| `protection-changed` | all windows | `ProtectionStatus` — after any change to what is protected, after decisions and maskings (throttled: at most 4 a second, the last change of a burst always followed by one), when the extension connects or falls silent, and at midnight |
| `ai-explain` | all windows | `{ requestId: string \| null, activityId: string \| null, text: string, model: string }` — a local AI explanation for an approval card (`requestId`) or a feed item (`activityId`); display only, labelled "AI explanation" |
| `settings-focused` | island | `null` — the settings window got the focus: the island folds back to compact unless an approval card is waiting. (When it is shown, the settings window is also moved below the island's current bottom on the island's display, within the work area, so the island never covers its title bar.) |
| `settings-changed`, `tray`, `screen-changed`, `cursor` | unchanged | unchanged |

The `zuko` object on `hook` payloads for `PreToolUse` and `PermissionRequest`:
```ts
interface ZukoHookInfo {
  verdict: Verdict;
  tier: Tier;
  score: number;
  headline: string;
  factors: { id: string; weight: number; text: string }[];
  vector: { reversibility: string; blastRadius: string; egress: string; sensitivity: string; privilege: boolean; obfuscated: boolean };
  reasonUser: string;
  friction: { type: "none" } | { type: "hold"; ms: number } | { type: "blocked" };
  rules: string[];
  violations: { id: string; verdict: string; reason: string; triggeredBy: number[] }[];
  rehydrated: string[];   // vault keys filled into the input
}

interface PrivacyEvent {
  source: "gateway" | "hook" | "chat" | "file" | "browser" | "clipboard";
  direction: "masked" | "rehydrated" | "blocked_prompt";
  count: number;
  keys: string[];
  labels: string[];        // human labels, same order as keys
  newKeys: string[];
  sessionId: string | null;
  maskedPrompt: string | null;  // for blocked_prompt: the masked text the user can resend
}
```

## 4. Native messaging (browser extension ⇄ `zuko-native-host` ⇄ pipe)

Host manifest name: `app.zuko.host`. Chrome/Edge frame: 4-byte little-endian length +
UTF-8 JSON. The host forwards each message to the app over the same pipe as a request
with `hook_event_name: "ZukoExtension"` and relays the app's reply.

Registration: the app registers the host for the current user at every launch (unless
`Settings.browserBridge` is off, or the data folder is redirected for development). It
writes `<local data>\native-host\app.zuko.host.json` (`name`, `description`, `path` = the
installed `bin\zuko-native-host.exe`, `type: "stdio"`, `allowed_origins:
["chrome-extension://cbdnjagdcfchakoejiahgeclappplcba/"]`, identical to
`extension/scripts/register-host.mjs`) and, on Windows, the default value of
`HKCU\Software\Google\Chrome\NativeMessagingHosts\app.zuko.host` and
`HKCU\Software\Microsoft\Edge\NativeMessagingHosts\app.zuko.host` (plus Chromium and
Brave when installed) = that file's path; on Linux a copy of the manifest in each installed
browser's `~/.config/<browser>/NativeMessagingHosts/`. Only what differs is written.

Heartbeat: while linked the extension sends `{"op":"hello"}` every 25 s (a `chrome.alarms`
alarm wakes a sleeping service worker every 30 s to reconnect when not linked). The app
counts the extension as connected while it heard from it in the last 60 s and sends
`protection-changed` when that flips.

Extension → app messages (`{"op": …}`):
- `{"op":"hello","version":…}` → `{"ok":true,"app":"zuko","version":…}`
- `{"op":"mask","text":…,"site":…}` → `{"ok":true,"text":…,"report":MaskReport}`
- `{"op":"rehydrate","text":…}` → `{"ok":true,"text":…,"keys":[…]}`
- `{"op":"vault"}` → `{"ok":true,"vault":<Vault JSON>}` (the full vault, so the extension's
  WASM engine can work offline; kept in `chrome.storage.session`)
- `{"op":"policy"}` → `{"ok":true,"detector":DetectorConfig,"localAi":{"enabled","reachable","model","waitForPromptScan"}}`
  (`reachable` is a live Ollama probe, only made when enabled)
- `{"op":"deepScan","text":…,"site":…,"wait":bool}` — the local-AI second look at text the extension
  already masked. Local AI off → `{"ok":true,"enabled":false,"added":[]}` at once. `wait:true` →
  scan now (bounded by `localAi.timeoutMs`; long text is chunked) and reply
  `{"ok":true,"enabled":true,"added":[{"key","label"}],"vault":<Vault JSON>}` with the entries it
  learned; `wait:false` → queue the scan, reply at once with `added:[]`, `queued:true` and the vault
  as known now. ADD-ONLY: the app only interns entries and the extension merges only adds, so the
  AI can make Zuko stricter, never looser. The model sees text after deterministic masking only;
  loopback-only, cached and concurrency-limited like every other local-AI call.
- `{"op":"event","kind":"masked"|"blocked"|"upload","site":…,"count":…,"keys":[…]}` → `{"ok":true}`
  (shown in the app's activity feed and privacy toasts)

## 5. Files on disk

| Path | Content |
|---|---|
| `%APPDATA%\Zuko\settings.json` | UI settings (`Settings`, including the chat fields above; never a key) |
| OS keyring, service `app.zuko.desktop` | `anthropic-api-key`, `openai-api-key` (the island chat; the UI may set, clear and ask presence, never read), `vault-key` (internal only) |
| `%APPDATA%\Zuko\policy.json` | `Policy` (pretty JSON) |
| `%LOCALAPPDATA%\Zuko\vault.bin` | vault JSON encrypted with XChaCha20-Poly1305; 32-byte key in the OS keyring under `vault-key` |
| `%LOCALAPPDATA%\Zuko\audit\audit.jsonl` | hash-chained `Receipt` lines |
| `%LOCALAPPDATA%\Zuko\gateway.json` | `{ "port": n, "token": "…", "upstream": "https://api.anthropic.com" }` |
| `%LOCALAPPDATA%\Zuko\bin\zuko-hook.exe` | relay |
| `%LOCALAPPDATA%\Zuko\bin\zuko-native-host.exe` | the browser extension's native host |
| `%LOCALAPPDATA%\Zuko\native-host\app.zuko.host.json` | its host manifest (section 4), named by `HKCU\Software\{Google\Chrome,Microsoft\Edge,…}\NativeMessagingHosts\app.zuko.host` |
| `%LOCALAPPDATA%\Zuko\inbox\` | dropped files and sanitized copies |
