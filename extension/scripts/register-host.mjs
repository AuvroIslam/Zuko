// Registers the Zuko native messaging host (`app.zuko.host`) so the extension can talk to the
// desktop app.
//
//   node scripts/register-host.mjs                 write the host manifest, print the registry commands
//   node scripts/register-host.mjs --apply         ...and run them (HKCU only, Chrome + Edge)
//   node scripts/register-host.mjs --uninstall     print the commands that remove the registration
//   node scripts/register-host.mjs --uninstall --apply
//
// Options: --exe <path>   host binary (default app/target/release/zuko-native-host[.exe])
//          --out <dir>    where to write app.zuko.host.json (default %LOCALAPPDATA%\Zuko\native-host)
//          --id <id>      extension ID (default: derived from the "key" in manifest.json)
//
// Nothing outside the manifest file and (only with --apply) HKCU\Software\...\NativeMessagingHosts
// is ever touched. The host manifest allows exactly one origin: this extension's ID.

import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, writeFileSync, copyFileSync } from "node:fs";
import { homedir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { readExtensionId, root } from "./ext-id.mjs";

export const HOST_NAME = "app.zuko.host";

const args = process.argv.slice(2);
const flag = (name) => args.includes(name);
const value = (name) => {
  const i = args.indexOf(name);
  return i >= 0 ? args[i + 1] : undefined;
};

/** The host manifest object (exported for tests). */
export function hostManifest({ exePath, extensionId }) {
  if (!/^[a-p]{32}$/.test(extensionId)) throw new Error(`"${extensionId}" is not a valid extension ID (32 letters a-p)`);
  return {
    name: HOST_NAME,
    description: "Zuko: browser extension to desktop app bridge",
    path: exePath,
    type: "stdio",
    allowed_origins: [`chrome-extension://${extensionId}/`],
  };
}

export const REGISTRY_KEYS = [
  ["Chrome", `HKCU\\Software\\Google\\Chrome\\NativeMessagingHosts\\${HOST_NAME}`],
  ["Edge", `HKCU\\Software\\Microsoft\\Edge\\NativeMessagingHosts\\${HOST_NAME}`],
];

/** The `reg` command lines for a manifest path (exported for tests). */
export function registryCommands(manifestPath, uninstall = false) {
  return REGISTRY_KEYS.map(([browser, key]) => ({
    browser,
    key,
    argv: uninstall ? ["delete", key, "/f"] : ["add", key, "/ve", "/t", "REG_SZ", "/d", manifestPath, "/f"],
    text: uninstall ? `reg delete "${key}" /f` : `reg add "${key}" /ve /t REG_SZ /d "${manifestPath}" /f`,
  }));
}

function main() {
  const win = process.platform === "win32";
  const extensionId = value("--id") ?? readExtensionId();
  if (!extensionId) {
    console.error("manifest.json has no \"key\" yet. Run `node scripts/gen-key.mjs` first (or pass --id).");
    process.exit(1);
  }
  const exeName = win ? "zuko-native-host.exe" : "zuko-native-host";
  // Zuko installs the host next to its relay, and only accepts extension messages
  // from there; the build output is the fallback for development.
  const installed = win
    ? join(process.env.LOCALAPPDATA ?? join(homedir(), "AppData", "Local"), "Zuko", "bin", exeName)
    : join(homedir(), ".local", "share", "zuko", "bin", exeName);
  const exePath = resolve(value("--exe") ?? (existsSync(installed) ? installed : join(root, "..", "app", "target", "release", exeName)));
  const outDir = resolve(
    value("--out") ?? (win ? join(process.env.LOCALAPPDATA ?? join(homedir(), "AppData", "Local"), "Zuko", "native-host") : join(homedir(), ".local", "share", "zuko", "native-host")),
  );
  const manifestPath = join(outDir, `${HOST_NAME}.json`);
  const uninstall = flag("--uninstall");
  const apply = flag("--apply");

  console.log(`Extension ID : ${extensionId}`);
  console.log(`Host binary  : ${exePath}${existsSync(exePath) ? "" : "   (NOT FOUND: run `cargo build --release -p zuko-native-host` in app/)"}`);
  console.log(`Host manifest: ${manifestPath}\n`);

  if (!uninstall) {
    mkdirSync(outDir, { recursive: true });
    writeFileSync(manifestPath, JSON.stringify(hostManifest({ exePath, extensionId }), null, 2) + "\n");
    console.log(`Wrote ${manifestPath}\n`);
  }

  if (win) {
    const cmds = registryCommands(manifestPath, uninstall);
    console.log(apply ? `Applying (${uninstall ? "removing" : "registering"}) for the current user:` : "Run these in a terminal (current user only), or re-run this script with --apply:");
    for (const c of cmds) {
      console.log(`  ${c.text}`);
      if (apply) {
        try {
          execFileSync("reg", c.argv, { stdio: "pipe" });
          console.log(`    ok (${c.browser})`);
        } catch (e) {
          console.log(`    ${c.browser}: ${String(e.stderr ?? e.message).trim() || "failed"}`);
        }
      }
    }
  } else {
    // Linux and macOS: the manifest is copied into each browser's NativeMessagingHosts directory.
    const dirs =
      process.platform === "darwin"
        ? [
            join(homedir(), "Library/Application Support/Google/Chrome/NativeMessagingHosts"),
            join(homedir(), "Library/Application Support/Microsoft Edge/NativeMessagingHosts"),
          ]
        : [join(homedir(), ".config/google-chrome/NativeMessagingHosts"), join(homedir(), ".config/microsoft-edge/NativeMessagingHosts"), join(homedir(), ".config/chromium/NativeMessagingHosts")];
    console.log(apply ? "Copying the manifest for the current user:" : "Run these (or re-run with --apply):");
    for (const d of dirs) {
      const target = join(d, `${HOST_NAME}.json`);
      console.log(uninstall ? `  rm "${target}"` : `  mkdir -p "${d}" && cp "${manifestPath}" "${target}"`);
      if (apply && !uninstall) {
        mkdirSync(d, { recursive: true });
        copyFileSync(manifestPath, target);
      }
    }
  }

  console.log("\nThen reload the extension. The popup shows \"Desktop app: linked\" once the Zuko app is running.");
}

// Run only as a script, not when the tests import the helpers above.
if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) main();
