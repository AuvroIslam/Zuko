# Zuko

**A privacy firewall and guardian for AI coding agents and AI chats.**

Zuko sits between you, your coding agent (Claude Code in the terminal or in VS Code)
and the cloud model. It:

- masks your secrets and personal data **before they leave your machine**, and puts
  them back locally;
- checks every action the agent tries against **your policy** and blocks what you
  have ruled out;
- explains each risky action in plain English and makes you **slow down for the
  dangerous ones**, so "Allow" stops being a reflex.

It lives at the top of your screen as a small guardian, Zuko, who lights up when an
agent needs you. He is a chibi fan tribute to Prince Zuko from *Avatar: The Last
Airbender* — glowing amber eyes, scar and topknot — and he throws fireballs at the
things he blocks.

---

## What it does

### 1. Your secrets never reach the model, but your work still gets done
You type: *"My API key is `sk-proj-…`, paste it in .env."*

- The model receives *"My API key is `{{API_KEY_1}}`, paste it in .env"* plus a note
  saying `API_KEY_1` is an OpenAI API key.
- The model answers by writing `OPENAI_API_KEY={{API_KEY_1}}` to `.env`.
- Zuko swaps the real key back in **on your machine**: the file gets the real key,
  the reply you read shows the real key, and the provider only ever saw the
  placeholder.

This covers your prompts, files the agent reads, command output, `@file`
attachments and IDE selections: everything Claude Code sends goes through Zuko's
local gateway. Detection covers 50+ secret formats (OpenAI, Anthropic, AWS,
GitHub, Stripe, Google, Slack, JWTs, private keys, connection strings, passwords…)
plus emails, phone numbers (including Bangladesh `+880`), cards (Luhn-checked),
IBANs and your own custom terms. The same value always gets the same placeholder,
so conversations stay consistent.

### 2. You decide what the agent may do
- Block websites (`pastebin.com`, `*.ngrok.io`), folders (`~/.ssh`, `D:\Personal`),
  commands (`git push --force*`) and tools (MCP servers).
- Rules apply to every tool call, including shell commands, which Zuko parses for
  network access, deletes, privilege changes and obfuscation.
- **Chain rules** catch multi-step leaks. For example, if the session read `.env`
  and a later `curl` carries that secret (raw, base64, hex or URL-encoded), the call
  is blocked and the reason cites the step that read it.
- **Self-protection:** the agent cannot edit Zuko's policy, Claude Code's settings,
  or kill Zuko.
- Blocked paths and domains can also be mirrored into Claude Code's own deny rules,
  which hold even when Zuko is not running.
- Every decision is written to a **tamper-evident audit log** (hash-chained, with a
  one-click verify). The log never contains secret values.

### 3. Approvals that make you look
- Each action gets a **risk tier** and a headline that leads with the consequence:
  *"DELETES the folders dist/ and .cache/ and everything in them"*.
- **Low risk** is approved automatically (you can turn that off). **High risk**
  needs a *hold-to-approve*. **Critical** or policy-violating actions are blocked.
- If you approve risky things faster than anyone could read them, Zuko notices and
  slows you down.

### 4. Documents and web chats
- **Drop a `.txt`, `.md`, `.pdf` or code file** into Zuko to get a masked copy that
  is safe to paste or upload anywhere.
- **Ctrl+Alt+M** masks the clipboard and **Ctrl+Alt+U** restores it, so this works
  with any chat app.
- The **browser extension** does the same on ChatGPT, claude.ai and DeepSeek. It
  masks prompts and uploaded files before they are sent, restores the values in the
  answers you read, and fixes copy buttons so copied code carries the real values.

---

## Install and run (Windows)

Requirements: [Rust](https://rustup.rs) (the GNU toolchain works; MSVC also works),
MinGW-w64 if you use the GNU toolchain, and Node 20+.

```powershell
cd app
npm install
npm run tauri dev        # development build
npm run pack             # installer in app/release/
```

Then open **Settings → Protection**, choose what to install, review the exact diff
Zuko will apply to `%USERPROFILE%\.claude\settings.json` (a dated backup is taken
first), and click **Apply**:

| Option | What it does |
|---|---|
| **Hooks** | Policy, risk scoring and approvals on every tool call. |
| **Gateway** | Routes Claude Code through Zuko (`ANTHROPIC_BASE_URL`), so nothing sensitive leaves the machine. Works with a Claude Pro/Max login and with API keys. Restart Claude Code afterwards. |
| **Deny rules** | Mirrors your blocked paths and domains into Claude Code's own `permissions.deny`. |

Uninstalling restores your settings exactly, including a previous
`ANTHROPIC_BASE_URL`.

The browser extension is in [`extension/`](extension/README.md).

## How it works

```
Claude Code (CLI / VS Code)
   │ hooks ──► zuko-hook.exe ──named pipe──► Zuko app ── policy · risk · taint · vault · audit
   │                                            │
   │ ANTHROPIC_BASE_URL ──► Zuko gateway (127.0.0.1) ──masked──► api.anthropic.com
   │
Browser ── Zuko extension (same engine, compiled to WASM) ──native messaging──► Zuko app
```

- **`app/core`** (`zuko-core`): the engine, in pure Rust with no I/O. It covers
  detection, the placeholder vault, masking and streaming rehydration, the shell
  analyzer, policy, risk, taint invariants, decisions and audit receipts. The same
  crate runs in the app, in the hook relay and, as WASM, in the browser.
- **`app/src-tauri`**: the desktop app. It holds the pipe server, the gateway proxy,
  the encrypted vault (XChaCha20-Poly1305, key in Windows Credential Manager), the
  audit log, the hook installer, the document sanitizer and the masked chat.
- **`app/hook`**: the relay Claude Code runs on each hook event. If the app is
  down, the relay still enforces your policy on its own.
- **`app/src`**: the island and settings UI.
- **`extension/`**: the MV3 extension. **`app/native-host`**: its bridge to the app.

Design notes and the full feature/feasibility analysis are in [`plan.md`](plan.md).
The interfaces between parts are in [`app/CONTRACTS.md`](app/CONTRACTS.md).

## Tests

```powershell
cd app
cargo test -p zuko-core      # engine: detection fixtures, streaming split tests, 51-scenario attack benchmark
cargo test -p zuko --lib     # app: gateway (mock upstream), vault store, audit log, sanitizer, chat
node core/wasm/smoke.mjs     # the WASM engine (after: cargo build -p zuko-core --target wasm32-unknown-unknown --release)
```

The attack benchmark runs 51 scenarios: exfiltration chains, prompt-injection
`curl | sh`, encoded secret egress, self-protection, safe workflows and near
misses. It reports detection rate, false-block rate and decision latency.

## Honest limits

- Detection is pattern-based. Unknown secret formats and free-form names can slip
  through, and custom terms help.
- Shell analysis is best effort. Anything Zuko can't analyse is asked about, never
  auto-approved.
- There is no OS sandbox on native Windows, so programs the agent starts (for
  example an `npm` postinstall script) are outside what Zuko can see.
- Claude Code fails hooks *open* by design. Gateway mode and deny rules are the hard
  floor; the relay enforces your policy even when the app is closed.
- Answers that depend on a secret's *actual value* ("is this key valid?") can't be
  given without sharing it.

## Credits

Zuko is built on the MIT-licensed source code of
[Coucou](https://github.com/Louis-CFM/coucou) by Louis Raillé. Coucou's name,
character, icon and sounds are not part of Zuko; see [`NOTICE.md`](NOTICE.md).
The secret rules are derived from [gitleaks](https://github.com/gitleaks/gitleaks)
(MIT); see `app/core/THIRD_PARTY.md`.
