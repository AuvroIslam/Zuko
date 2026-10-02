# Zuko — build plan

> A privacy firewall and guardian for AI coding agents and AI chats.
> It sits between you, your agent (Claude Code in the terminal or VS Code) and the
> cloud model. It masks your secrets before they leave your machine and checks every
> action the agent tries. It also stops you from approving dangerous things on reflex.

Zuko is built on the MIT-licensed source code of
[Coucou](https://github.com/Louis-CFM/coucou), a Tauri 2 desktop companion that
already shows Claude Code sessions and relays permission requests. We keep its
plumbing (hook relay, named pipe, island UI, settings window, keyring and
installer) and replace its brand, character, sounds and service integrations with
Zuko's security features. See `NOTICE.md` for what was and was not imported.

---

## 1. The problems

| # | Problem | What it looks like today |
|---|---|---|
| 1 | **Approval fatigue** | The agent asks permission 40 times an hour. We press *Yes* without reading. A `rm -rf`, a `curl` to an unknown host or an edit to `~/.ssh` gets approved like an `ls`. |
| 2 | **Secrets leak into the chat** | "My API key is `sk-proj-abc…`, paste it in `.env`." That key now sits on a cloud provider's servers, in logs and in the transcript. Reading `.env`, a stack trace with a token or a customer CSV leaks the same way. |
| 3 | **No real boundaries** | We can't say "never visit pastebin.com" or "never read `D:\Personal`" and have it enforced and monitored. |
| 4 | **Useful answers need the data** | We share the secret because the answer seems to need it. We want the same answer *without* the provider ever seeing the value. This applies to plain text, `.md` and `.pdf` files, and web chats (ChatGPT, claude.ai, DeepSeek). |

---

## 2. What Zuko does

### Pillar A: Privacy Shield (problems 2 and 4)
- **Detect** secrets and PII locally. Secrets: API keys for OpenAI, Anthropic, AWS, GitHub, Stripe, Google, Slack and others, plus JWTs, private keys, connection strings, passwords and high-entropy tokens. PII: emails, phone numbers, cards (Luhn check), IBAN (mod-97 check) and IPs. You can add your own terms too, such as client names or project codenames.
- **Mask** each value with a stable, typed placeholder: `{{API_KEY_1}}`, `{{EMAIL_2}}`. The same value always gets the same placeholder, so the conversation stays consistent and prompt caching keeps working.
- **Tell the model what the placeholder means without revealing it.** A short legend is attached, for example *"API_KEY_1 = an OpenAI API key; CARD_1 = a Visa card ending 4242; PHONE_1 = a Bangladeshi mobile number."* The model can then answer as if it had the data.
- **Rehydrate locally.** The model writes `OPENAI_API_KEY={{API_KEY_1}}`; Zuko swaps the real value in *on your machine*. Your `.env` gets the real key, the answer on screen shows the real key, and the provider only ever saw the placeholder.

### Pillar B: Agent Firewall (problem 3)
- **A policy you control:** blocked and allowed domains, blocked read and write paths, blocked commands and blocked tools (MCP included). There is a global policy plus an optional per-project `.zuko/policy.json`.
- **Enforced on every tool call** through Claude Code's `PreToolUse` hook. This covers Read, Write, Edit, Glob, Grep, WebFetch, WebSearch, Bash, PowerShell and MCP tools. A blocked call is denied with a reason the agent can read and adapt to.
- **Defense in depth:** blocked paths and domains are also written into Claude Code's own `permissions.deny` rules, which Claude Code enforces even if Zuko is not running.
- **Taint-aware chain rules,** inspired by ChainBreak. Example: if the session read `.env` and now tries to `curl` a host *with that secret's value (or its base64/hex/URL encoding) in the command*, the call is blocked. If the value is absent, Zuko asks. This avoids the "one read taints the whole session forever" false positives.
- **Self-protection:** the agent cannot edit Zuko's policy, Claude Code's settings, the hook binary, or kill Zuko.
- **Monitoring:** a live activity feed and a tamper-evident audit log. The log is append-only JSONL with SHA-256 hash chaining and a `verify` command, and it never stores secret values.

### Pillar C: Approval Guard (problem 1)
- **Risk scoring per action.** An impact vector covers reversibility, blast radius, network egress, data sensitivity, privilege and obfuscation. It produces a tier (low, medium, high, critical) and a **plain-English headline** that leads with the consequence: *"DELETES the folder `build/` and everything in it"* or *"SENDS data to `webhook.site` (unknown host)"*.
- **Friction proportional to risk:**
  - **Low:** auto-approved (configurable), so you stop seeing prompts that don't matter.
  - **Medium:** a normal approval card.
  - **High:** *hold-to-approve*. The Allow button only fills while held, after the explanation has been on screen.
  - **Critical / policy violation:** blocked, with the reason.
- **Rubber-stamp detection:** if you approve risky actions faster than you could read them, Zuko slows you down.
- **"UNKNOWN is not SAFE":** commands Zuko can't analyse (`eval`, `iex`, `-EncodedCommand`, `base64 -d | sh`) are always asked about, never auto-approved.

---

## 3. What is actually possible (verified against Claude Code docs, Oct 2026)

| Goal | Possible? | How |
|---|---|---|
| Rewrite your typed prompt before the model sees it | **Not with settings hooks** (`UserPromptSubmit` can only block or add context). **Yes with the Zuko Gateway.** | Local proxy set via `ANTHROPIC_BASE_URL` masks every `/v1/messages` request body. |
| Gateway works with a Claude Pro/Max login (OAuth) | **Yes** | Set only `ANTHROPIC_BASE_URL`, forward `anthropic-beta` and auth headers untouched. |
| `env` in `~/.claude/settings.json` reaches the VS Code extension | **Yes** | The extension spawns Claude Code with that env. |
| Model writes `{{API_KEY_1}}`, the real value lands in `.env` | **Yes** | The gateway rehydrates `tool_use` input in the response stream. In hooks-only mode, `PreToolUse.updatedInput` does it. |
| Mask `@file` attachments, IDE selections, file reads and command output | **Gateway: yes, all of it** (it sees every outbound byte). Hooks-only: Bash output yes (`PostToolUse.updatedToolOutput`); `@file` no. | Gateway |
| Show real values on screen while the model only has placeholders | **Yes** in gateway mode (text deltas are rehydrated). Hooks-only: CLI only, via `MessageDisplay`. | Gateway |
| Block domains and folders | **Yes** for Claude's tools. Shell commands are parsed on a best-effort basis. | `PreToolUse` deny plus `permissions.deny` |
| Risk explanation inside Claude Code's own prompt | **Yes** | `PreToolUse` `ask` + `permissionDecisionReason` (shown to the user, not the model). |
| Auto-approve low-risk actions | **Yes** | `PreToolUse` `allow` (never overrides a deny rule). |
| OS-level sandbox on native Windows | **No** | Claude Code's sandbox needs macOS, Linux or WSL2. A child process (e.g. a malicious `npm` postinstall) can still reach the network; this is out of scope. |
| Enforcement that survives Zuko crashing | Partly | `permissions.deny` rules: yes. Hooks fail open by Claude Code's design, so the relay carries a local, stateless copy of the policy engine. |
| Web chats (ChatGPT, claude.ai, DeepSeek) | **Yes**, with a browser extension | Wraps `fetch`/XHR in the page to mask outgoing prompts, plus an exact-match tripwire on every outgoing body; DOM-only rehydration of answers; sanitized file uploads. |
| PDF redaction | **Text, yes.** Layout-preserving PDF redaction, no. | Extract text, mask it, output `name.zuko.md`. Black boxes drawn on a PDF don't remove the text, and MuPDF is AGPL. |
| "Same answer as if I shared it" | **Mostly** | Placeholders plus a legend preserve meaning. Answers that depend on the *actual value* ("is this key valid?", "what's the checksum of this card?") cannot be preserved, and we say so. |

### Two modes
1. **Gateway mode (recommended, complete).** `ANTHROPIC_BASE_URL=http://127.0.0.1:<port>/t/<token>`. Nothing sensitive leaves the machine. It fails *closed*: if Zuko is off, Claude Code can't reach the API, and a `UserPromptSubmit` check explains why instead of showing a cryptic network error. A tray toggle turns it on and off (Claude Code reads `env` at session start, so a toggle applies to new sessions).
2. **Hooks-only mode (fallback).** No proxy. Prompts containing secrets are blocked, with the original text suppressed, and a masked version is copied for you to resend. Tool inputs are rehydrated by `PreToolUse`, Bash output is masked by `PostToolUse`, and the firewall and approval guard work fully.

---

## 4. Example: "This is my api key (abcdefg), paste it in .env"

```
You type ──► Claude Code ──► Zuko Gateway (127.0.0.1)
                               │  detects "abcdefg" as API key
                               │  vault: abcdefg → {{API_KEY_1}}
                               │  request body becomes:
                               │   "This is my api key ({{API_KEY_1}}), paste it in .env"
                               │   + legend "API_KEY_1 = an API key; reproduce verbatim"
                               ▼
                          Anthropic API  (never sees abcdefg)
                               │  streams back: text "Done — added it to .env"
                               │  + tool_use Write{.env, "OPENAI_API_KEY={{API_KEY_1}}"}
                               ▼
                         Zuko Gateway rehydrates the stream locally
                               │  Write{.env, "OPENAI_API_KEY=abcdefg"}
                               ▼
Claude Code ──► PreToolUse hook ──► Zuko firewall: policy ok, risk low ──► allow
           ──► .env written with the real key
Next turn: Claude Code sends history containing "abcdefg" ──► gateway re-masks it
           to the exact same {{API_KEY_1}} (deterministic), so the provider still sees only the placeholder.
```

Rules that keep this correct:
- **User and tool_result content** gets full detection plus vault matching.
- **Assistant content** gets *vault-value replacement only* (the exact inverse of rehydration), so history round-trips byte-for-byte and thinking signatures stay valid.
- **Thinking blocks** are never touched in either direction.
- **Placeholders split across SSE chunks** are buffered; values inserted into `input_json_delta` are JSON-escaped.

---

## 5. Architecture

```
┌────────────────────────── your machine ───────────────────────────┐
│                                                                    │
│  Claude Code (CLI / VS Code)                                       │
│     │ hooks (PreToolUse, PostToolUse, UserPromptSubmit,            │
│     │        PermissionRequest, SessionStart, …)                   │
│     ▼                                                              │
│  zuko-hook.exe ──named pipe \\.\pipe\zuko-<SID>──► Zuko app (Tauri)│
│   (local stateless policy fallback                 ├ Guard: policy │
│    if the app is down)                             │   risk, taint │
│                                                    ├ Vault (encrypted,
│     │ ANTHROPIC_BASE_URL                           │   key in Credential Manager)
│     ▼                                              ├ Audit log (hash chain)
│  Zuko Gateway 127.0.0.1 ◄──────── part of app ─────┤ Island UI + Settings
│     │ masked traffic only                          └ File sanitizer (txt/md/pdf)
│     ▼                                                    ▲
│  api.anthropic.com                                       │ native messaging
│                                                          │ (zuko-native-host.exe)
│  Browser ── Zuko extension (MV3) ────────────────────────┘
│   chatgpt.com · claude.ai · chat.deepseek.com
│   same engine compiled to WASM
└────────────────────────────────────────────────────────────────────┘
```

### Components and repo layout
```
app/                          Tauri 2 desktop app (Windows primary, Linux works)
  core/                       zuko-core — pure Rust, no I/O, no C deps; also builds to WASM
    detect.rs                 secret + PII rules, Luhn/IBAN/entropy validators, custom terms
    placeholder.rs            {{KIND_N}} format, parser, tolerant matcher
    vault.rs                  value ↔ placeholder mapping, kinds, safe hints, (de)serialize
    mask.rs                   mask/rehydrate text and JSON, legend builder
    stream.rs                 incremental rehydrator for chunked text / JSON-string fragments
    anthropic.rs              Messages API request masking + SSE response rehydration
    shell.rs                  Bash/PowerShell command analyzer
    action.rs                 Claude Code tool call → normalized Action (paths, hosts, commands)
    policy.rs                 policy model, path globs, domain match, evaluation
    risk.rs                   impact vector, score, tier, plain-English headline
    taint.rs                  per-session ledger + chain invariants
    guard.rs                  policy + risk + taint → Decision
    hookio.rs                 Claude Code hook output builders
    audit.rs                  hash-chained receipt records + verifier
    wasm.rs                   C-ABI exports for the browser extension
  hook/                       zuko-hook relay (spawned by Claude Code per event)
  native-host/                zuko-native-host: browser extension ⇄ app bridge (stdio ⇄ pipe)
  src-tauri/                  the app: pipe server, guard state, gateway, vault store,
                              audit store, policy store, hook installer, file sanitizer, chat
  src/                        island + settings UI (TypeScript), Zuko character, synth sounds
extension/                    MV3 browser extension (TypeScript + WASM core)
plan.md  NOTICE.md  LICENSE  README.md
```

### Hook events Zuko installs
| Event | Zuko uses it for | Waits for an answer? |
|---|---|---|
| `SessionStart` | open a taint ledger; inject the placeholder convention as `additionalContext` | yes (fast) |
| `UserPromptSubmit` | gateway on: count what will be masked and notify. Gateway off: block secret-bearing prompts and offer a masked copy. Gateway down: explain. | yes |
| `PreToolUse` | policy, risk, taint invariants, self-protection, placeholder rehydration → allow / ask / deny / no opinion | yes (≤1.5 s, then local fallback) |
| `PermissionRequest` | island approval card with risk, explanation and friction | yes (≤110 s) |
| `PostToolUse` | update taint ledger (secrets seen, sensitive reads, untrusted input); mask Bash output in hooks-only mode | yes (fast) |
| `Stop`, `SessionEnd`, `Notification`, `SubagentStart/Stop`, `PostToolUseFailure` | activity feed, session summary | no |

### Relay ⇄ app protocol
- Hook to app: one JSON line with the **full** payload (no truncation for firewall events; up to 8 MiB).
- App to hook: one JSON line, `{"stdout": <object or null>}`, printed verbatim. An empty reply means "no opinion".
- App unreachable: the relay loads `%APPDATA%\Zuko\policy.json` and runs the same `zuko-core` guard without state. Policy violations are still denied, risky actions become `ask`, everything else passes.

---

## 6. Policy format (`%APPDATA%\Zuko\policy.json`, optional `<project>/.zuko/policy.json`)
```json
{
  "version": 1,
  "mode": "enforce",
  "network":    { "blocked": ["pastebin.com", "webhook.site", "*.ngrok.io", "transfer.sh"],
                  "allowed": ["github.com", "registry.npmjs.org", "pypi.org"],
                  "unknown": "allow" },
  "filesystem": { "blockedRead":  ["~/.ssh/**", "~/.aws/**", "D:/Personal/**"],
                  "blockedWrite": ["C:/Windows/**"],
                  "sensitive":    ["**/.env", "**/.env.*", "**/*.pem", "**/id_rsa*", "**/credentials*"] },
  "commands":   { "blocked": ["git push --force*", "rm -rf /*", "format *"],
                  "ask":     ["npm publish*", "git push*"] },
  "tools":      { "blocked": [], "ask": ["mcp__*"] },
  "approvals":  { "autoAllowLowRisk": true, "holdToApproveFrom": "high", "blockFrom": "critical" },
  "privacy":    { "maskSecrets": true, "maskPii": true, "customTerms": ["Project Falcon"],
                  "allowlist": ["example.com", "localhost", "127.0.0.1"] }
}
```
Evaluation order: **self-protection → deny → taint invariants → ask → risk tier → allow**. In `"mode": "monitor"`, everything is logged and nothing is blocked.

---

## 7. Risk model
- Score from 0 to 100 built from impact-vector factors. Each factor has a plain-English sentence.
  - **Irreversible:** `rm -rf`, `Remove-Item -Recurse`, `del /s`, `git reset --hard`, `git clean -fd`, `push --force`, `DROP TABLE`, `format`.
  - **Blast radius:** writes outside the project, system paths, home dotfiles, global installs.
  - **Egress:** curl, wget, Invoke-WebRequest/RestMethod, nc, scp, ssh, ftp, git push, npm publish, WebFetch to unknown hosts.
  - **Data sensitivity:** sensitive paths, detector hits, secrets in arguments.
  - **Privilege:** sudo, runas, Set-ExecutionPolicy, reg add, schtasks, chmod 777.
  - **Supply chain:** package installs, `curl | sh`.
  - **Obfuscation / unknown:** eval, iex, encoded commands, `base64 -d | sh`, unparseable commands.
- Tiers: low (< 25) · medium (25–49) · high (50–79) · critical (≥ 80, or any policy or invariant hit).

## 8. Chain invariants (deterministic; no LLM in the decision path)
| Id | Fires when | Verdict |
|---|---|---|
| `SECRET_EGRESS` | egress action carries a vaulted or seen secret (raw, base64, hex or URL-encoded) | deny |
| `TAINTED_EGRESS` | session read sensitive data and now egresses to an unknown host | ask |
| `UNTRUSTED_EXEC` | content from WebFetch or a downloaded file is piped to a shell, or `curl … \| sh` | deny |
| `SELF_PROTECT` | touches Zuko's policy/vault/binaries, `~/.claude/settings*.json`, or kills Zuko | deny |
| `UNKNOWN_TOOL` | first use of an MCP tool not in the allowed list | ask |
| `UNANALYZABLE` | command can't be parsed confidently | ask |

---

## 9. Web chats: browser extension
- **Network layer (enforcement):** a MAIN-world script at `document_start` wraps `fetch`, `XMLHttpRequest`, `WebSocket.send` and `sendBeacon`. Known prompt fields are rewritten:
  - ChatGPT: `messages[].content.parts`.
  - claude.ai: `prompt` and `attachments[].extracted_content`.
  - DeepSeek: `prompt`.
- **Tripwire:** every outgoing body on any endpoint is checked for exact vault values (and their encodings) and masked or blocked.
- **Display:** a MutationObserver rewrites only text-node values, turning placeholders back into real values. Restored values are highlighted with the CSS Custom Highlight API, and Alt+R toggles them. Copy buttons are patched so copied code carries real values.
- **Files:** `.txt`, `.md`, code and `.pdf` uploads are intercepted (input `change`, drop, paste). PDFs go through pdf.js in an offscreen document and are replaced with a sanitized `name.zuko.md`.
- **Engine:** the same `zuko-core` compiled to WASM runs in the service worker. The vault lives in `chrome.storage.session` (memory only), and is shared with the desktop app through native messaging when the app is running.

## 10. Documents (.txt, .md, .pdf) in the desktop app
- Drop a file on the Zuko island. Zuko scans it, lists the findings and writes a sanitized copy (`name.zuko.md`), which you can paste or upload anywhere.
- **Restore:** paste an answer that contains placeholders and Zuko rehydrates it from the vault. Hotkeys mask and unmask the clipboard, so this works with *any* chat app.
- The built-in chat with Claude always goes through the masker.

### 10b. Local AI (optional, off by default)
An on-device model in Ollama (default `gemma3:4b` at `http://127.0.0.1:11434`) catches what
patterns cannot — person names, street addresses, organisation names, internal hostnames
and tokens with no known format — and explains risky actions in plain English. Config:
`localAi` in the global policy (`enabled`, `endpoint`, `model`, `deepScanPrompts`,
`deepScanDocuments`, `explainRisk`, `waitForPromptScan`, `timeoutMs`); a project policy
cannot change it. Settings → Local AI shows Ollama's status (with the exact
`ollama pull <model>` when the model is missing), the installed models, and a Test button.

**Core security rule: the LLM may only make Zuko STRICTER, never looser.**
- Deterministic detection and the guard always run first and stay authoritative. The model
  can ADD findings to mask (interned into the vault, so the normal `mask_known` /
  `mask_text` passes mask them everywhere from then on), ADD an ask or raise a tier (pure
  helpers `stricter_verdict` / `stricter_tier`, monotone by construction) and ADD
  explanation text. It can never remove a mask, lower a verdict, turn deny/ask into allow,
  or hold up the fast path. Explanations are display text that no decision reads.
- Its output is untrusted: strict JSON only; every finding must be an exact substring of
  the text it was shown (checked in `zuko_core::localai::parse_deep_scan`), kinds come from
  a fixed list, labels are Zuko's own (a model label could carry the value into the cloud
  legend), short/odd values are rejected. Garbage, refusals and timeouts are ignored: the
  deterministic result stands. An explanation that plays the risk down ("this is safe") is
  discarded.
- Loopback only: the endpoint must be `127.0.0.1`, `localhost` (pinned to 127.0.0.1) or
  `[::1]`, checked on save and again on every call; the client ignores proxies and
  refuses redirects.
- The model never sees a known secret: deep scans get the text after deterministic masking
  (on a throwaway vault copy), explanations are built only from vault-masked facts
  (`ExplainInput::masked` is the only constructor).
- Bounded: at most 2 calls in flight, a hash-keyed cache, per-call `timeoutMs`.

Where it runs: documents wait (bounded) for a chunked deep scan before the `.zuko.md` is
written (`SanitizeResult.aiDeepScan`: "AI deep scan: +N items"); the island chat waits for
the scan of the message; hook `UserPromptSubmit` and the gateway scan the newest user text
in the background; ask/deny decisions and approval cards get an `ai-explain` event.
**Limitation:** in background mode the very first send of a new name can leave before its
scan finishes; every later request masks it. The gateway option "wait for AI scan on
prompts" closes the gap by holding each prompt up to `timeoutMs`. Measured on the dev
machine (CPU, gemma3:4b): about 4 s for a repeated prompt, 7–10 s for a new short prompt,
plus about 11 s the first time the model loads (Zuko pre-loads it when the feature is on),
so slower machines should raise `timeoutMs` or use a smaller model.

---

## 11. Rebrand (required by Coucou's asset license)
- Name **Zuko** everywhere: product name, bundle id `app.zuko.desktop`, `zuko.exe`, `zuko-hook.exe`, pipe `zuko-<SID>`, `%APPDATA%\Zuko`, keyring service `app.zuko.desktop`.
- **The character, Zuko:** an unofficial chibi fan tribute to Prince Zuko from *Avatar: The Last Airbender*, drawn in code (`app/src/character/engine.ts`, fire in `fire.ts`). A big glossy cream sphere of a head (~70 % of the figure) with glowing amber almond eyes, the reddish scar round his left eye (the viewer's right), a black topknot in a red hair-tie whose ponytail swings on a spring, and a small Fire Nation tunic: maroon with a gold V collar, red neck scarf, gold-knotted belt, stubby sleeves with gold cuffs and cream fists with dark tips. Palette from the concept sheet: #F7F3EE, #E8D5C4, #F28A1E, #B33A2E, #2B1D1A. The eye glow takes the state colour (amber idle/working, yellow-amber pulsing for approvals, red-orange with a flame aura for errors, gold and happy when finished); expressions include determined, happy, angry, thinking, excited, sleepy, plus heart, star, wink, dizzy and the rest. Mini bots in the pills are simplified heads tinted by the pill colour. Distinct from Coucou's Mochi (no plain white ball with pill eyes, no blush or wave).
- **He firebends:** `shootFire(target)` (fire punch: the arm extends, the fist ignites, flame tongues wrap into it and a fireball flies to the target with an ember trail and bursts), `fireFlick(target)`, `setFireAura(on)`, `setFireRing(on)` and `fireSwirl()`. Fire that leaves the bot's box is drawn on a full-window effects canvas (`#fx-canvas`, pointer-events none); effects only keep the island at full frame rate while they animate. Triggers: a firewall block (PreToolUse `zuko.verdict` "deny" or a deny activity item) fire-punches the blocked line; a masked secret gets a flick at the privacy notice; a high/critical approval card keeps an aura simmering; an agent at work hovers him on a ring of fire; a triple-click throws a punch towards the click (six quick clicks still make him dizzy); the launch greeting lands him in a fire swirl and his fireball sets the island's edge alight; the drop scan burns secret lines into placeholder blocks with a sweep of fire.
- New icon drawn in code (`scripts/gen-icons.mjs`) and new **synthesized** sounds (WebAudio, no audio files).
- Coucou's service integrations (Stripe, n8n, GitHub, Vercel, Resend, Notion, Cal.com) are removed. Their pills become Zuko's protection surfaces: Claude Code, Gateway, Browser, Policy.

---

## 12. Build phases

| Phase | Deliverable | Status |
|---|---|---|
| 0 | Repo, toolchain (Rust GNU + MinGW), research, this plan | done |
| 1 | Rebrand identifiers, remove integrations, `zuko-core` skeleton with fixed APIs | done |
| 2 | `zuko-core`: detection, vault, masking, streaming, Anthropic transforms | done: 51 secret rules + PII, 0 false positives on the fixture corpus, 1 MB scanned in about 50 ms |
| 3 | `zuko-core`: shell analyzer, actions, policy, risk, taint, guard, hook I/O, audit | done: 51-scenario benchmark, 100% detection, 0% false blocks, about 0.3 ms per decision |
| 4 | App: request/response pipe, guard state, relay fallback, installer (hooks, gateway env, deny rules), vault and audit stores | done |
| 5 | App: Zuko Gateway (proxy, SSE rehydration, token auth, upstream chaining) | done |
| 6 | UI: Zuko character, synth sounds, icon, risk approval card with hold-to-approve, activity feed, privacy notices, settings | done |
| 7 | Documents: sanitizer (txt/md/pdf/code), clipboard mask/unmask (Ctrl+Alt+M / U), masked chat | done |
| 8 | Browser extension + WASM engine + native host | done (128 tests); still needs a manual check in a real Chrome/Edge on the live sites |
| 9 | Tests and real end-to-end runs | done (see below) |
| 10 | Security review and fixes, README | done for the pipe trust boundary (only Zuko's own binaries may talk to the app); further review welcome |
| 11 | Optional local AI (Ollama): deep scans and risk explanations, stricter-only (§10b) | done: core validators + mock-Ollama app tests; real `gemma3:4b` smoke test passes (`cargo test -p zuko --lib real_gemma -- --ignored`) |

### Verified end to end (real Claude Code CLI, real app, fake secrets)
1. **Gateway mode:** a prompt carrying `sk-proj-…` reached the provider as `{{API_KEY_1}}`. The agent wrote `OPENAI_API_KEY={{API_KEY_1}}`, and `.env` on disk got the real key. The audit log shows `Gateway masked [API_KEY_1]`, then `PreToolUse Write allow`.
2. **Firewall:** `curl https://pastebin.com/...` was denied by `PreToolUse` (`network.blocked:pastebin.com`). The agent was told why and adapted.
3. **Hooks-only mode:** a prompt carrying a Stripe key was blocked before it was sent, and a masked copy (`{{API_KEY_2}}`) was offered for resending.
4. **Relay without the app:** the stateless fallback still denies blocked domains, `rm -rf ~`, reads of `~/.ssh` and edits to Claude Code's settings, in about 13–33 ms.

### Still to do / manual checks
- Load the extension in a real Chrome or Edge and check ChatGPT, claude.ai and DeepSeek. Site DOMs and endpoints drift.
- Build and test the NSIS installer (`npm run pack`), and do a Linux build.
- An "Always allow" from the island is treated as a single allow; it could write a Claude Code permission rule (`updatedPermissions`).
- PostToolUse output too large to mask within the 1.5 s budget passes through unmasked (gateway mode still masks it upstream).

## 13. Testing strategy
- **Unit tests** (`cargo test -p zuko-core`): detector corpus (true and false positives), mask→rehydrate round trips, streaming rehydration split at *every* byte position, policy glob and domain matching on Windows paths, shell analysis cases.
- **Benchmark suite:** scripted attack, safe and near-miss scenarios. Reports detection rate, false-block rate, ask rate and latency.
- **Gateway tests:** a local mock Anthropic server that streams SSE. Verifies the upstream only ever receives placeholders and the client receives real values.
- **End to end:** real `claude -p` runs pointed at the gateway with an isolated `--settings` file and *fake* secrets. They check that the request on the wire is masked and that the file on disk has the real value. Your own `~/.claude/settings.json` is never touched by tests.
- **Extension:** unit tests for body rewriters and the tripwire against the WASM engine in Node, then manual checks on the three sites.

## 14. Honest limitations
- Detection is pattern-based. Unknown secret formats and free-form names can slip through, and custom terms help. The optional local AI (§10b) catches many names and addresses, but it is best effort, only ever adds masks, and in background mode can miss the first send of a new value.
- Shell parsing is best effort. Obfuscated commands are treated as unknown, which means Zuko asks.
- No OS sandbox on native Windows, so processes the agent starts can do things Zuko never sees.
- Hooks fail open by Claude Code's design. Gateway mode plus `permissions.deny` rules are the hard floor.
- The web extension depends on sites' private APIs and DOM, which change. Enforcement sits at the network layer so DOM drift breaks display, not protection.
- PDFs: text only (no images or OCR); output is markdown, not a redacted PDF.
- Some Claude Code traffic skips any gateway. `/bug`, `/feedback`, `/share` and the session survey upload transcripts straight to Anthropic, and in gateway mode the local transcript holds rehydrated values. Zuko sets `DISABLE_BUG_COMMAND=1` and `DISABLE_ERROR_REPORTING=1` when it installs the gateway and tells you why.
- A trusted project's `.claude/settings.json` can set its own `ANTHROPIC_BASE_URL` and route around the gateway. The relay reports the session's real base URL on every event, and Zuko warns when a session is not using the gateway. Only managed (admin) settings can fully prevent the override.
- Rehydration is a privileged sink. Zuko fills real values automatically only into local file writes. Shell commands get them through `PreToolUse` only when they have no network egress. WebFetch and MCP calls never get them. This stops a prompt injection from using Zuko itself to send a secret out.

## 15. Dev setup (Windows)
- Rust (installed: GNU host, `stable-x86_64-pc-windows-gnu`) plus MinGW-w64 (WinLibs) for C deps and `windres`. Add `%USERPROFILE%\.cargo\bin` to PATH.
- Node 20+.
- `cd app && npm install && npm run tauri dev`
- Optional MSVC route (official Tauri path): VS 2022 Build Tools with "Desktop development with C++", then `rustup default stable-x86_64-pc-windows-msvc`.

## 16. Git conventions
- Commit messages: at most 7 words. No co-author trailers.
- `coucou/`, `comp_rules.md` and `inspiration.md` are local references and are git-ignored.
