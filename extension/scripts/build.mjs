// Builds the extension into extension/dist (load that folder unpacked).
//
//   node scripts/build.mjs          bundle, copy static files, pdf.js worker and zuko_core.wasm
//   node scripts/build.mjs --min    also minify our own code (default: readable, easier to audit)
//   ZUKO_WASM=path\to\zuko_core.wasm node scripts/build.mjs     use a different engine build
//
// The WASM engine is built separately:
//   cd app && cargo build -p zuko-core --target wasm32-unknown-unknown --release
// Its absence is NOT a build failure (so the UI can be worked on without Rust), but it is
// reported loudly here and by the service worker at runtime.

import { build } from "esbuild";
import { copyFileSync, existsSync, mkdirSync, readFileSync, readdirSync, rmSync, statSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const dist = join(root, "dist");
const minify = process.argv.includes("--min");
const wasmSource = process.env.ZUKO_WASM
  ? resolve(process.env.ZUKO_WASM)
  : resolve(root, "..", "app", "target", "wasm32-unknown-unknown", "release", "zuko_core.wasm");

rmSync(dist, { recursive: true, force: true });
mkdirSync(dist, { recursive: true });

const common = {
  bundle: true,
  target: "chrome116",
  minify,
  legalComments: "none",
  logLevel: "warning",
  charset: "utf8",
};

// MAIN-world and isolated content scripts are classic scripts: IIFE, no imports at runtime.
await build({ ...common, entryPoints: { "net-guard": "src/main-world/net-guard.ts", content: "src/content/index.ts" }, format: "iife", outdir: dist, absWorkingDir: root });
// Extension pages and the module service worker: ES modules.
await build({
  ...common,
  entryPoints: { sw: "src/background/sw.ts", offscreen: "src/offscreen/offscreen.ts", popup: "src/popup/popup.ts" },
  format: "esm",
  outdir: dist,
  absWorkingDir: root,
});

// Static files.
const copy = (from, to) => {
  mkdirSync(dirname(to), { recursive: true });
  copyFileSync(from, to);
};
copy(join(root, "manifest.json"), join(dist, "manifest.json"));
for (const f of readdirSync(join(root, "static"))) copy(join(root, "static", f), join(dist, f));
for (const f of readdirSync(join(root, "icons"))) if (f.endsWith(".png")) copy(join(root, "icons", f), join(dist, "icons", f));

// pdf.js worker (loaded by relative path from offscreen.html; no remote code).
const worker = join(root, "node_modules", "pdfjs-dist", "build", "pdf.worker.min.mjs");
if (!existsSync(worker)) throw new Error("pdfjs-dist is not installed: run `npm install` in extension/");
copy(worker, join(dist, "pdf.worker.min.mjs"));

// The engine.
let wasmOk = false;
if (existsSync(wasmSource)) {
  copy(wasmSource, join(dist, "zuko_core.wasm"));
  wasmOk = true;
} else {
  const bar = "!".repeat(78);
  console.error(
    `\n${bar}\n!! zuko_core.wasm NOT FOUND at:\n!!   ${wasmSource}\n!! The extension was built WITHOUT its privacy engine. It will only block\n!! obvious secrets and cannot mask anything. Build the engine, then rebuild:\n!!   cd app && cargo build -p zuko-core --target wasm32-unknown-unknown --release\n!!   cd ../extension && npm run build\n${bar}\n`,
  );
}

// Every file the manifest points at must exist.
const manifest = JSON.parse(readFileSync(join(dist, "manifest.json"), "utf8"));
const refs = new Set([
  manifest.background?.service_worker,
  manifest.action?.default_popup,
  ...Object.values(manifest.icons ?? {}),
  ...Object.values(manifest.action?.default_icon ?? {}),
  ...(manifest.content_scripts ?? []).flatMap((c) => c.js ?? []),
  "offscreen.html",
  "pdf.worker.min.mjs",
]);
const missing = [...refs].filter((f) => f && !existsSync(join(dist, f)));
if (missing.length) throw new Error(`manifest references missing files: ${missing.join(", ")}`);

const kb = (f) => `${(statSync(join(dist, f)).size / 1024).toFixed(0)} KB`;
console.log(
  `built extension/dist (${minify ? "minified" : "unminified"}): ` +
    ["net-guard.js", "content.js", "sw.js", "offscreen.js", "popup.js", "pdf.worker.min.mjs", ...(wasmOk ? ["zuko_core.wasm"] : [])]
      .map((f) => `${f} ${kb(f)}`)
      .join(", "),
);
writeFileSync(join(dist, ".built"), new Date().toISOString());
