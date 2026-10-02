# Zuko browser extension

Masks API keys, secrets and personal data **before they leave your browser** for ChatGPT, claude.ai and
DeepSeek, and puts the real values back on your screen. It runs the same `zuko-core` engine as the
desktop app (compiled to WASM) and can share one vault with it, so a key you mask in Claude Code is
the same `{{API_KEY_1}}` in a web chat.

Extension ID (pinned by the `key` in `manifest.json`): `cbdnjagdcfchakoejiahgeclappplcba`

## What it protects

| Layer | What happens |
|---|---|
| **Network (enforcement)** | A MAIN-world script at `document_start` wraps `fetch`, `XMLHttpRequest`, `WebSocket.send`, `navigator.sendBeacon` and the clipboard writers. Known prompt endpoints are rewritten: ChatGPT `messages[].content.parts[]` (text and multimodal text), claude.ai `prompt` and `attachments[].extracted_content`, DeepSeek `prompt` (completion and edit). A one-line note tells the model what the placeholders mean. |
| **Tripwire** | Every other outgoing body (any host, any endpoint) is checked for exact vault values. Raw and JSON-escaped copies are replaced by their placeholder; URL-encoded, base64 and hex copies, and values in a cross-origin URL, cannot be fixed in place, so the request is **blocked** and you are told. |
| **Fail-safe** | If the engine cannot be reached and a prompt or upload contains something that looks like a live credential (private key, `sk-...`, `AKIA...`, `ghp_...`, JWT, ...), the request is **blocked**, not sent. Ordinary prompts still go through. |
| **Display** | Placeholders in the chat are turned back into real values by changing only `Text.nodeValue` (no element is added or moved, so React stays happy). Restored values are highlighted with the CSS Custom Highlight API. **Alt+R** flips between real values and what the AI sees. Mangled placeholders (`[API_KEY_1]`, `{{api_key_1}}`, `API KEY 1`, ...) are matched too, but only for keys that exist in the vault. |
| **Copy** | Copy buttons and Ctrl+C give you the real value (restored in the isolated world, so real values never enter the page's JavaScript). |
| **Files** | `.txt .md .json .csv`, code and `.env` uploads (file picker, drop, paste) are masked in place. PDFs become `name.zuko.md` (text from pdf.js in an offscreen document, `## Page N` sections, then masked); a PDF with no text layer is **blocked**. |
| **Composer chip** | "Zuko: 2 items will be masked" while you type (advisory; masking itself happens on send). |

Per-site switches and session counters are in the toolbar popup. Nothing is sent anywhere except through
the optional link to your own desktop app.

## Build

Needs Node 22.13+ (24 recommended) and, for the engine, Rust with the `wasm32-unknown-unknown` target.

```powershell
# 1. the engine (once, or whenever zuko-core changes)
cd app
cargo build -p zuko-core --target wasm32-unknown-unknown --release

# 2. the extension
cd ..\extension
npm install
npm run build        # -> extension/dist  (add -- --min to minify our own code)
npm test             # typecheck + 128 tests against the real WASM engine
```

`npm run build` copies `app/target/wasm32-unknown-unknown/release/zuko_core.wasm` into `dist/`. If it is
missing the build still succeeds but prints a loud warning, and the service worker logs
`ENGINE NOT LOADED` at runtime (the popup shows it too): without the engine Zuko can only block obvious
secrets. Use `ZUKO_WASM=path\to\zuko_core.wasm npm run build` for a different build.

## Load unpacked

Chrome: `chrome://extensions`, enable **Developer mode**, **Load unpacked**, choose `extension/dist`.
Edge: `edge://extensions`, enable **Developer mode**, **Load unpacked**, choose `extension/dist`.

The manifest `key` pins the ID above, so the native host registration below stays valid across rebuilds.
After rebuilding, press the reload button on the extension card and reload open chat tabs.

## Link the desktop app (optional)

The extension talks to the Zuko desktop app through the native messaging host `app.zuko.host`
(`app/native-host`, a small Rust binary: Chrome frames on stdio, one JSON line on `\\.\pipe\zuko-<SID>`).

```powershell
cd app
cargo build --release -p zuko-native-host      # -> app\target\release\zuko-native-host.exe
cd ..\extension
node scripts/register-host.mjs                 # writes the host manifest, prints the registry commands
node scripts/register-host.mjs --apply         # ...and runs them (HKCU only: Chrome + Edge)
node scripts/register-host.mjs --uninstall --apply   # remove again
```

Without `--apply` the script only writes `%LOCALAPPDATA%\Zuko\native-host\app.zuko.host.json` (override
with `--out`) and **prints** the two `reg add` commands. The host manifest allows exactly one origin,
this extension. Start the Zuko app, then the popup shows **Desktop app: linked**. When linked:

* new values are masked by the app (one vault shared with Claude Code); the extension keeps a copy in
  `chrome.storage.session` (memory only, never on disk) so it keeps working if the app closes;
* the app's detector settings (custom terms, allowlist, ...) are applied;
* blocks and uploads appear in the app's activity feed.

If the app is closed or the host is not registered, Zuko works alone with its own session vault.
Values masked while the app was closed stay in the extension's session vault; when the app comes back its
numbering wins, and a clashing local key is renumbered.

## Architecture

```
page (MAIN world)  net-guard.js ──MessageChannel──▶ content.js (isolated) ──runtime.sendMessage──▶ sw.js
  fetch/XHR/WS/beacon/clipboard                      bridge, toasts, chip,                          zuko_core.wasm + vault
  known-endpoint rewrite, tripwire                   DOM rehydration, uploads                       ├─ offscreen.html (pdf.js)
                                                                                                    └─ connectNative ─▶ zuko-native-host ─▶ pipe ─▶ app
```

* `src/main-world/` is small on purpose: it runs under the page's CSP, has no `chrome.*`, and only
  intercepts and asks. The MessageChannel is created by the isolated script at `document_start`; the
  guard accepts one port, once, and the isolated side stops offering ports once the guard says hello.
* `src/background/` holds the engine, the vault and the policy. `brain.ts` has no `chrome.*` calls, so
  it is tested directly.
* `src/content/` has the per-site selectors (`sites.ts`, with generic fallbacks), toasts, rehydration,
  copy handling and upload interception.
* Selectors only affect display. Enforcement does not read the DOM, so a site redesign breaks the chip
  or the restored text, not protection.

## Limits (be honest with yourself about these)

* **Detection is pattern-based.** Unknown secret formats and free-form names can slip through (add
  custom terms in the desktop app). No ML or NER.
* **Private APIs move.** Paths and body shapes are matched with regexes and anything unknown still goes
  through the tripwire, but a new endpoint that carries a prompt in a new shape is only protected for
  values already in the vault, until the rewriter is updated.
* **Blind spots:** requests made from the site's own Web Workers or service worker never touch the
  patched `fetch`; voice mode, images (no OCR), the desktop and mobile apps are out of scope; a site
  that calls `showOpenFilePicker` is not intercepted at the file dialog (the network layer still checks
  text-like bodies). Streams (`ReadableStream` bodies) and files over 16 MB cannot be scanned and pass
  unchanged.
* **Anti-bot code** could fingerprint a patched `fetch`, and could one day verify request bodies.
* **Information loss:** questions that depend on the real value ("is this key valid?") cannot be
  answered from a placeholder. Models sometimes drop or rewrite placeholders; tolerant matching helps
  but does not guarantee a restore.
* **PDFs:** text only, no OCR, output is markdown (not a redacted PDF). Scanned PDFs are blocked.
  Office files and archives are passed with a warning (Zuko cannot look inside them).
* **Trust:** this extension can read and rewrite your prompts on those four sites (that is how it
  works). It loads no remote code, requests only those host permissions, and is open source.
* Restored values exist in the page's DOM while shown; anything the page later reads from the DOM and
  sends is caught by the tripwire. Alt+R hides them again.

## Development notes

* `npm test` runs `tsc --noEmit` first, then `node --test` over `tests/`. The tests load the **real**
  `zuko_core.wasm` in Node (like `app/core/wasm/smoke.mjs`), the real pdf.js, jsdom pages, a fake
  `chrome`, and for the native host the real `zuko-native-host.exe` against a fake app on the real
  named pipe (skipped when the exe is not built or a real Zuko app owns the pipe).
* TypeScript is erasable-syntax only (`erasableSyntaxOnly`), so Node runs `src/**/*.ts` directly in tests.
* `scripts/gen-key.mjs` creates the manifest key and keeps the private key in `.keys/` (git-ignored).
  Never commit it. `--force` replaces the key (the ID changes: re-run `register-host.mjs`).
* `scripts/gen-icons.mjs` redraws the toolbar icons (the chibi Zuko head, ported from `app/scripts/gen-icons.mjs`).

## Credits

Approach informed by two open-source projects (ideas only, no code copied): Redacto (Apache-2.0), for
running a Rust/WASM engine in an MV3 extension and for tolerant placeholder matching; and Better-DeepSeek
(MIT, EdgeTypE), for patching `fetch`/XHR in the page's own world. pdf.js (Apache-2.0) is bundled for PDF
text extraction.

## Fan tribute

Zuko's on-screen character is an unofficial fan tribute to Prince Zuko from Avatar:
The Last Airbender (© Viacom International / Nickelodeon); Zuko the app is not
affiliated with or endorsed by them.
