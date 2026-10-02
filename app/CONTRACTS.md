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
The relay waits for a reply for `PreToolUse`, `PostToolUse`, `UserPromptSubmit`,
`SessionStart` (budget 1.5 s each) and `PermissionRequest` (two-phase: an `{"ack":true}`
line within 800 ms, then the decision within 108 s). All other events are fire-and-forget.

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

New commands (JS argument names are camelCase):

| Command | Args | Returns |
|---|---|---|
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
| `activity_recent` | `limit: number` | `ActivityItem[]` (newest first) |
| `audit_verify` | — | `{ ok: boolean, count: number, error: string \| null }` |
| `audit_open_folder` | — | `void` |
| `mask_text` | `text: string` | `{ text: string, report: MaskReport }` |
| `unmask_text` | `text: string` | `{ text: string, keys: string[] }` |
| `sanitize_file` | `path: string` | `SanitizeResult` |
| `reveal_path` | `path: string` | `void` (shows a file in Explorer) |
| `clipboard_mask` | — | `{ count: number }` (masks the clipboard text in place) |
| `clipboard_unmask` | — | `{ count: number }` |

`approval_decision` gains an optional `elapsedMs: number` (time the card was on screen),
used for rubber-stamp detection.

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
  maskedTotal: number;         // values masked since launch
  blockedTotal: number;        // actions denied since launch
  askedTotal: number;
  autoAllowedTotal: number;
  extensionConnected: boolean; // a native-host connection was seen in the last 60 s
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
}
```

## 3. Events (Rust → frontend)

| Event | Target | Payload |
|---|---|---|
| `hook` | island | the hook payload (UI copy: strings truncated to 2000 chars) + `request_id` for PermissionRequest + **`zuko`** (below) |
| `activity` | all windows | `ActivityItem` |
| `privacy` | all windows | `PrivacyEvent` |
| `protection-changed` | all windows | `ProtectionStatus` |
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

Extension → app messages (`{"op": …}`):
- `{"op":"hello","version":…}` → `{"ok":true,"app":"zuko","version":…}`
- `{"op":"mask","text":…,"site":…}` → `{"ok":true,"text":…,"report":MaskReport}`
- `{"op":"rehydrate","text":…}` → `{"ok":true,"text":…,"keys":[…]}`
- `{"op":"vault"}` → `{"ok":true,"vault":<Vault JSON>}` (the full vault, so the extension's
  WASM engine can work offline; kept in `chrome.storage.session`)
- `{"op":"policy"}` → `{"ok":true,"detector":DetectorConfig}`
- `{"op":"event","kind":"masked"|"blocked"|"upload","site":…,"count":…,"keys":[…]}` → `{"ok":true}`
  (shown in the app's activity feed and privacy toasts)

## 5. Files on disk

| Path | Content |
|---|---|
| `%APPDATA%\Zuko\settings.json` | UI settings (`Settings`) |
| `%APPDATA%\Zuko\policy.json` | `Policy` (pretty JSON) |
| `%LOCALAPPDATA%\Zuko\vault.bin` | vault JSON encrypted with XChaCha20-Poly1305; 32-byte key in the OS keyring under `vault-key` |
| `%LOCALAPPDATA%\Zuko\audit\audit.jsonl` | hash-chained `Receipt` lines |
| `%LOCALAPPDATA%\Zuko\gateway.json` | `{ "port": n, "token": "…", "upstream": "https://api.anthropic.com" }` |
| `%LOCALAPPDATA%\Zuko\bin\zuko-hook.exe` | relay |
| `%LOCALAPPDATA%\Zuko\inbox\` | dropped files and sanitized copies |
