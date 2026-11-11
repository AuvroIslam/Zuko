import assert from "node:assert/strict";
import { createHash, generateKeyPairSync } from "node:crypto";
import { execFileSync } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { extensionIdFromKey, readExtensionId } from "../scripts/ext-id.mjs";
import { hostManifest, registryCommands, REGISTRY_KEYS } from "../scripts/register-host.mjs";
import { extRoot, repoRoot } from "./helpers.mjs";

const manifest = JSON.parse(readFileSync(join(extRoot, "manifest.json"), "utf8"));

test("manifest: MV3, the four sites only, the requested permissions only", () => {
  assert.equal(manifest.manifest_version, 3);
  assert.deepEqual(manifest.host_permissions, ["https://chatgpt.com/*", "https://chat.openai.com/*", "https://claude.ai/*", "https://chat.deepseek.com/*"]);
  // `alarms`: the heartbeat that relinks a sleeping service worker to the desktop app.
  assert.deepEqual([...manifest.permissions].sort(), ["alarms", "clipboardWrite", "nativeMessaging", "offscreen", "storage"]);
  assert.equal(manifest.background.type, "module");
  assert.equal(manifest.background.service_worker, "sw.js");
  assert.equal(manifest.action.default_popup, "popup.html");
  assert.equal(manifest.content_security_policy.extension_pages, "script-src 'self' 'wasm-unsafe-eval'; object-src 'self'", "no remote code, no eval");
  assert.equal(manifest.externally_connectable, undefined, "no other extension or website can message us");
  assert.equal(manifest.web_accessible_resources, undefined, "nothing is exposed to pages");
});

test("manifest: the net guard runs in the MAIN world and the content script in the isolated one, both at document_start, guard first", () => {
  const [main, isolated] = manifest.content_scripts;
  assert.deepEqual(main.js, ["net-guard.js"]);
  assert.equal(main.world, "MAIN");
  assert.equal(main.run_at, "document_start");
  assert.deepEqual(isolated.js, ["content.js"]);
  assert.ok(isolated.world === undefined || isolated.world === "ISOLATED");
  assert.equal(isolated.run_at, "document_start");
  assert.deepEqual(main.matches, manifest.host_permissions);
  assert.deepEqual(isolated.matches, manifest.host_permissions);
});

test("the pinned key gives a valid extension ID, derived the way Chrome does it", () => {
  const id = readExtensionId();
  assert.match(id, /^[a-p]{32}$/);
  const der = Buffer.from(manifest.key, "base64");
  const digest = createHash("sha256").update(der).digest("hex").slice(0, 32);
  const expected = [...digest].map((c) => String.fromCharCode(97 + parseInt(c, 16))).join("");
  assert.equal(id, expected);
  // The key is a public SPKI key, nothing private.
  assert.equal(der[0], 0x30);
  assert.ok(!JSON.stringify(manifest).includes("PRIVATE"));
  assert.ok(der.length < 400, "a 2048-bit public key, not a private one");
});

test("extensionIdFromKey agrees with an independently generated key", () => {
  const { publicKey } = generateKeyPairSync("rsa", { modulusLength: 2048 });
  const spki = publicKey.export({ type: "spki", format: "der" });
  const id = extensionIdFromKey(spki.toString("base64"));
  assert.match(id, /^[a-p]{32}$/);
  assert.equal(extensionIdFromKey(spki.toString("base64")), id, "deterministic");
});

test("the private key never reaches git", () => {
  assert.match(readFileSync(join(extRoot, ".gitignore"), "utf8"), /^\.keys\/$/m);
  let tracked;
  try {
    tracked = execFileSync("git", ["ls-files", "extension"], { cwd: repoRoot, encoding: "utf8" });
  } catch {
    return; // not a git checkout
  }
  assert.ok(!/\.pem$|\.keys\//m.test(tracked), "no key material is tracked");
  assert.ok(!/node_modules|dist\//.test(tracked), "no build output is tracked");
});

test("icons: PNG at 16, 32, 48 and 128 px", () => {
  for (const size of [16, 32, 48, 128]) {
    const path = join(extRoot, manifest.icons[String(size)]);
    const png = readFileSync(path);
    assert.deepEqual([...png.subarray(0, 8)], [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);
    assert.equal(png.readUInt32BE(16), size);
    assert.equal(png.readUInt32BE(20), size);
  }
});

test("host manifest: one allowed origin (this extension), stdio, the right name", () => {
  const id = readExtensionId();
  const m = hostManifest({ exePath: "C:\\Zuko\\zuko-native-host.exe", extensionId: id });
  assert.deepEqual(m, {
    name: "app.zuko.host",
    description: m.description,
    path: "C:\\Zuko\\zuko-native-host.exe",
    type: "stdio",
    allowed_origins: [`chrome-extension://${id}/`],
  });
  assert.throws(() => hostManifest({ exePath: "x", extensionId: "not-an-id" }), /valid extension ID/);
  assert.throws(() => hostManifest({ exePath: "x", extensionId: "*" }), /valid extension ID/);
});

test("the desktop app registers the host for this extension's ID, under the same name", () => {
  // app/src-tauri/src/nativehost.rs writes the host manifest at every launch; the origin it
  // allows must be the ID this manifest's key pins, or Chrome refuses the connection.
  const src = readFileSync(join(repoRoot, "app", "src-tauri", "src", "nativehost.rs"), "utf8");
  assert.equal(src.match(/pub const EXTENSION_ID: &str = "([a-p]{32})";/)?.[1], readExtensionId());
  assert.match(src, /pub const HOST_NAME: &str = "app\.zuko\.host";/);
  assert.match(src, /const DESCRIPTION: &str = "Zuko: browser extension to desktop app bridge";/);
  assert.equal(hostManifest({ exePath: "x", extensionId: readExtensionId() }).description, "Zuko: browser extension to desktop app bridge");
});

test("registry commands target HKCU for Chrome and Edge only", () => {
  const cmds = registryCommands("C:\\Users\\me\\app.zuko.host.json");
  assert.deepEqual(
    cmds.map((c) => c.text),
    [
      'reg add "HKCU\\Software\\Google\\Chrome\\NativeMessagingHosts\\app.zuko.host" /ve /t REG_SZ /d "C:\\Users\\me\\app.zuko.host.json" /f',
      'reg add "HKCU\\Software\\Microsoft\\Edge\\NativeMessagingHosts\\app.zuko.host" /ve /t REG_SZ /d "C:\\Users\\me\\app.zuko.host.json" /f',
    ],
  );
  assert.ok(REGISTRY_KEYS.every(([, key]) => key.startsWith("HKCU\\")));
  assert.deepEqual(registryCommands("x", true).map((c) => c.argv[0]), ["delete", "delete"]);
});

test("register-host.mjs writes the manifest and prints (never runs) the registry commands without --apply", () => {
  const out = mkdtempSync(join(tmpdir(), "zuko-host-"));
  try {
    const stdout = execFileSync(process.execPath, [join(extRoot, "scripts", "register-host.mjs"), "--out", out, "--exe", join(out, "zuko-native-host.exe")], { encoding: "utf8" });
    const written = JSON.parse(readFileSync(join(out, "app.zuko.host.json"), "utf8"));
    assert.equal(written.allowed_origins[0], `chrome-extension://${readExtensionId()}/`);
    assert.equal(written.path, join(out, "zuko-native-host.exe"));
    if (process.platform === "win32") {
      assert.match(stdout, /reg add "HKCU\\Software\\Google\\Chrome\\NativeMessagingHosts\\app\.zuko\.host"/);
      assert.match(stdout, /reg add "HKCU\\Software\\Microsoft\\Edge\\NativeMessagingHosts\\app\.zuko\.host"/);
      assert.match(stdout, /re-run this script with --apply/);
    }
    assert.match(stdout, /NOT FOUND/, "warns when the host binary is missing");
  } finally {
    rmSync(out, { recursive: true, force: true });
  }
});

test("a built dist/ is self-consistent (skipped until `npm run build`)", (t) => {
  const dist = join(extRoot, "dist");
  if (!existsSync(join(dist, "manifest.json"))) return t.skip("dist/ not built");
  const m = JSON.parse(readFileSync(join(dist, "manifest.json"), "utf8"));
  const files = [m.background.service_worker, m.action.default_popup, ...Object.values(m.icons), ...m.content_scripts.flatMap((c) => c.js), "offscreen.html", "offscreen.js", "popup.js", "pdf.worker.min.mjs"];
  for (const f of files) assert.ok(existsSync(join(dist, f)), `${f} is in dist/`);
  for (const f of ["net-guard.js", "content.js"]) {
    const src = readFileSync(join(dist, f), "utf8");
    assert.ok(!/^\s*(import|export)\s/m.test(src), `${f} is a classic script (no import/export)`);
  }
  assert.ok(!/\beval\s*\(|new Function\(/.test(readFileSync(join(dist, "net-guard.js"), "utf8")), "the page-world script uses no eval");
  assert.match(readFileSync(join(dist, "offscreen.html"), "utf8"), /offscreen\.js/);
});
