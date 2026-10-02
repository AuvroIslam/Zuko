// Shared by gen-key.mjs, register-host.mjs and the tests: extension ID from the manifest key.
// ID = first 16 bytes of SHA-256(SPKI DER public key), each hex digit mapped 0-f -> a-p.

import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

export const root = join(dirname(fileURLToPath(import.meta.url)), "..");
export const manifestPath = join(root, "manifest.json");

/** SPKI DER bytes (base64, as stored in the manifest "key") -> 32-char extension ID. */
export function extensionIdFromKey(publicKeyBase64) {
  const digest = createHash("sha256").update(Buffer.from(publicKeyBase64, "base64")).digest();
  return [...digest.subarray(0, 16)]
    .map((b) => String.fromCharCode(97 + (b >> 4)) + String.fromCharCode(97 + (b & 15)))
    .join("");
}

/** The pinned extension ID, or null when manifest.json has no "key" yet. */
export function readExtensionId() {
  const manifest = JSON.parse(readFileSync(manifestPath, "utf8"));
  return manifest.key ? extensionIdFromKey(manifest.key) : null;
}
