// Pins the extension ID. Generates an RSA-2048 keypair, writes ONLY the public key into
// manifest.json ("key"), and prints the extension ID that Chrome and Edge derive from it.
// The private key goes to extension/.keys/ (git-ignored); loading unpacked does not need
// it, it is only for packing a .crx with the same ID later.
//
//   node scripts/gen-key.mjs            create a key if the manifest has none, print the ID
//   node scripts/gen-key.mjs --force    replace the key (the ID changes: re-register the host)

import { generateKeyPairSync } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { extensionIdFromKey, manifestPath, root } from "./ext-id.mjs";

const force = process.argv.includes("--force");
const keysDir = join(root, ".keys");
const privatePath = join(keysDir, "zuko-extension.pem");
const manifest = JSON.parse(readFileSync(manifestPath, "utf8"));

if (manifest.key && !force) {
  console.log("manifest.json already has a key (use --force to replace it).");
  console.log(`Extension ID: ${extensionIdFromKey(manifest.key)}`);
  console.log(existsSync(privatePath) ? `Private key: ${privatePath}` : "Private key: not on this machine (fine for loading unpacked).");
  process.exit(0);
}

const { publicKey, privateKey } = generateKeyPairSync("rsa", { modulusLength: 2048 });
const spki = publicKey.export({ type: "spki", format: "der" }).toString("base64");
mkdirSync(keysDir, { recursive: true });
writeFileSync(privatePath, privateKey.export({ type: "pkcs8", format: "pem" }), { mode: 0o600 });

manifest.key = spki;
writeFileSync(manifestPath, JSON.stringify(manifest, null, 2) + "\n");

console.log(`Wrote the public key to manifest.json and the private key to ${privatePath}`);
console.log(`Extension ID: ${extensionIdFromKey(spki)}`);
console.log("Never commit extension/.keys/. If you changed the key, re-run scripts/register-host.mjs.");
