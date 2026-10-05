# Zuko desktop app: developer notes

See the [project README](../README.md) for what Zuko is and how to use it, and
[`CONTRACTS.md`](CONTRACTS.md) for the interfaces between the parts.

## Build

```powershell
npm install
npm run tauri dev      # live-reloading development build
npm run pack           # installer in release/
```

Windows library tests need an extra Common Controls manifest. Enable it only
for the library test command, then clear it before running or building the app:

```powershell
$env:ZUKO_TEST_MANIFEST = "1"
try { cargo test -p zuko --lib } finally { Remove-Item Env:ZUKO_TEST_MANIFEST }
```

`npm run dev` serves the front end in an ordinary browser. Add `?mock=1` to see the
island and the settings window with fake data, without Tauri:

- `index.html?mock=1&scene=approval-high` — island scenes: `overview`, `activity`,
  `approval-{low,medium,high,critical}`, `privacy`, `privacy-blocked`, …
- `settings.html?mock=1`
- `dev/character-preview.html` — every state, expression and size of Zuko.

`cargo run --bin zuko-gateway` runs the gateway headless with an in-memory engine
(set `ZUKO_DATA_DIR` to a temp folder; `ZUKO_GATEWAY_DUMP=<file>` records exactly
what was sent upstream).

## Layout

```
core/         zuko-core: the engine (pure Rust, also builds to WASM)
hook/         zuko-hook.exe, the Claude Code relay (with a stateless policy fallback)
native-host/  zuko-native-host.exe, the browser extension bridge
src-tauri/    the app: pipe server, gateway, vault, audit, installer, sanitizer, chat
src/          island and settings UI (TypeScript, no framework)
  character/  Zuko, drawn in Canvas 2D
  island/     state machine, hook events
  views/      island views (approval card, activity, privacy notices)
  settings/   the settings window
scripts/      icon generator, packaging
```

## Data on disk

| Path | Content |
|---|---|
| `%APPDATA%\Zuko\settings.json` | UI preferences |
| `%APPDATA%\Zuko\policy.json` | your policy (a project can add `.zuko/policy.json`, which can only tighten it) |
| `%LOCALAPPDATA%\Zuko\vault.bin` | the placeholder vault, encrypted; key in Credential Manager |
| `%LOCALAPPDATA%\Zuko\audit\audit.jsonl` | hash-chained audit receipts |
| `%LOCALAPPDATA%\Zuko\gateway.json` | gateway port, token and upstream |
| `%LOCALAPPDATA%\Zuko\zuko.log` | diagnostics (never contains secret values) |

Tests and the dev gateway honour `ZUKO_DATA_DIR` and `ZUKO_CONFIG_DIR` so they never
touch these.

## Linux

Everything platform-specific lives in `src-tauri/src/platform/` and
`hook/src/unix.rs`; the Linux build of the original app still applies (WebKitGTK,
gtk-layer-shell, Secret Service for keys, a Unix socket at
`$XDG_RUNTIME_DIR/zuko.sock` for the relay).
