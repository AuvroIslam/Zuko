// Smoke test for the zuko-core WASM build (no dependencies).
//
//   cargo build -p zuko-core --target wasm32-unknown-unknown --release
//   node core/wasm/smoke.mjs [path/to/zuko_core.wasm]
//
// Loads the module with an empty import object, drives every documented op through the
// C-ABI (zuko_alloc / zuko_call / zuko_free) and checks the JSON replies.

import { readFileSync, statSync } from "node:fs";
import { fileURLToPath } from "node:url";
import path from "node:path";
import assert from "node:assert/strict";

const here = path.dirname(fileURLToPath(import.meta.url));
const wasmPath =
  process.argv[2] ?? path.resolve(here, "../../target/wasm32-unknown-unknown/release/zuko_core.wasm");
const bytes = readFileSync(wasmPath);
const module = await WebAssembly.compile(bytes);
assert.deepEqual(WebAssembly.Module.imports(module), [], "module must not need imports");
const instance = await WebAssembly.instantiate(module, {});
const { memory, zuko_alloc, zuko_free, zuko_call } = instance.exports;
for (const f of [zuko_alloc, zuko_free, zuko_call]) assert.equal(typeof f, "function");

const enc = new TextEncoder();
const dec = new TextDecoder("utf-8", { fatal: true });

/** Sends raw bytes, returns the parsed JSON reply. */
function callBytes(input) {
  const ptr = zuko_alloc(input.length);
  new Uint8Array(memory.buffer, ptr, input.length).set(input);
  const packed = zuko_call(ptr, input.length);
  zuko_free(ptr, input.length);
  assert.equal(typeof packed, "bigint");
  const outPtr = Number(packed >> 32n);
  const outLen = Number(packed & 0xffffffffn);
  const out = dec.decode(new Uint8Array(memory.buffer, outPtr, outLen).slice());
  zuko_free(outPtr, outLen);
  return JSON.parse(out);
}
const call = (req) => callBytes(enc.encode(JSON.stringify(req)));

let checks = 0;
function ok(reply, what) {
  assert.equal(reply.ok, true, `${what}: ${JSON.stringify(reply)}`);
  checks++;
  return reply;
}
function fails(reply, what) {
  assert.equal(reply.ok, false, `${what} should fail`);
  assert.equal(typeof reply.error, "string");
  checks++;
}

// Fake but correctly shaped values.
const KEY = "sk-proj-ZukoFake0123456789abcdefghijklmnopqrstuvwx";
const MAIL = "rahim.uddin@gmail.com";

// --- default state: scan / mask work before any configure
let r = ok(call({ op: "scan", text: `key ${KEY} mail ${MAIL}` }), "scan (default config)");
assert.deepEqual(r.findings.map((f) => f.kind), ["API_KEY", "EMAIL"]);
assert.equal(r.findings[0].rule, "openai-api-key");
assert.equal(r.findings[1].hint, "Gmail address");
assert.equal(r.findings[0].value, KEY);

// --- configure
ok(call({ op: "configure", detector: { customTerms: ["Project Falcon"], ips: true } }), "configure");
r = ok(call({ op: "scan", text: "project falcon runs on 52.95.110.1 and 10.0.0.1" }), "scan (configured)");
assert.deepEqual(r.findings.map((f) => [f.kind, f.value]), [
  ["TERM", "project falcon"],
  ["IP", "52.95.110.1"],
]);
fails(call({ op: "configure", detector: { ips: "yes" } }), "configure with a bad type");
ok(call({ op: "configure", detector: {} }), "configure defaults");

// --- mask / rehydrate / maskKnown
const prompt = `Use ${KEY} and mail ${MAIL}; card 4242 4242 4242 4242; ফোন 01712345678`;
r = ok(call({ op: "mask", text: prompt, source: "browser", now: 1700000000 }), "mask");
assert.equal(r.text, "Use {{API_KEY_1}} and mail {{EMAIL_1}}; card {{CARD_1}}; ফোন {{PHONE_1}}");
assert.equal(r.report.count, 4);
assert.deepEqual(r.report.keys, ["API_KEY_1", "EMAIL_1", "CARD_1", "PHONE_1"]);
assert.deepEqual(r.report.newKeys, r.report.keys);
const masked = r.text;
r = ok(call({ op: "mask", text: masked }), "mask is idempotent");
assert.equal(r.text, masked);
assert.equal(r.report.count, 0);
r = ok(call({ op: "rehydrate", text: `${masked} {{NOPE_1}} {{ EMAIL_1 }}` }), "rehydrate");
assert.equal(r.text, `${prompt} {{NOPE_1}} ${MAIL}`);
assert.deepEqual(r.keys, ["API_KEY_1", "EMAIL_1", "CARD_1", "PHONE_1", "EMAIL_1"]);
r = ok(call({ op: "maskKnown", text: `reply: ${KEY} / ${MAIL} / other@acme.io` }), "maskKnown");
assert.equal(r.text, "reply: {{API_KEY_1}} / {{EMAIL_1}} / other@acme.io");
assert.equal(r.count, 2);
fails(call({ op: "mask" }), "mask without text");
fails(call({ op: "rehydrate", text: 5 }), "rehydrate with a non-string");

// --- legend
r = ok(call({ op: "legend", keys: ["CARD_1", "API_KEY_1", "NOPE_1"] }), "legend");
assert.ok(r.text.startsWith("Privacy note from Zuko:"));
assert.ok(r.text.endsWith("- {{API_KEY_1}}: OpenAI API key (project-scoped)\n- {{CARD_1}}: Visa card ending 4242"), r.text);
assert.equal(ok(call({ op: "legend", keys: [] }), "empty legend").text, "");
fails(call({ op: "legend", keys: "API_KEY_1" }), "legend with bad keys");

// --- views never contain values
r = ok(call({ op: "views" }), "views");
assert.equal(r.entries.length, 4);
assert.deepEqual(r.entries.map((e) => e.key), ["API_KEY_1", "EMAIL_1", "CARD_1", "PHONE_1"]);
assert.ok(!JSON.stringify(r.entries).includes(KEY) && !JSON.stringify(r.entries).includes(MAIL));
assert.equal(r.entries[0].preview, "sk-p…uvwx");
assert.equal(r.entries[0].source, "browser");

// --- exportVault / loadVault round trip (object and string forms)
const exported = ok(call({ op: "exportVault" }), "exportVault").vault;
assert.equal(exported.entries.length, 4);
assert.equal(ok(call({ op: "loadVault", vault: null }), "loadVault empty").size, 0);
assert.equal(ok(call({ op: "rehydrate", text: masked }), "rehydrate after reset").text, masked);
assert.equal(ok(call({ op: "loadVault", vault: exported }), "loadVault object").size, 4);
assert.equal(ok(call({ op: "rehydrate", text: masked }), "rehydrate after load").text, prompt);
assert.equal(ok(call({ op: "loadVault", vault: JSON.stringify(exported) }), "loadVault string").size, 4);
r = ok(call({ op: "mask", text: `new ${"bob.fake@acme-corp.io"}` }), "counters continue after load");
assert.equal(r.text, "new {{EMAIL_2}}");
fails(call({ op: "loadVault", vault: "{not json" }), "loadVault bad JSON");
fails(call({ op: "loadVault", vault: 42 }), "loadVault bad type");

// --- protocol errors
fails(callBytes(enc.encode("{not json")), "invalid JSON");
fails(callBytes(new Uint8Array([0xff, 0xfe, 0x7b])), "invalid UTF-8");
fails(callBytes(new Uint8Array(0)), "empty request");
fails(call({ nope: 1 }), "missing op");
fails(call({ op: "explode" }), "unknown op");
zuko_free(zuko_alloc(0), 0);

// --- memory: many calls must not leak
const before = memory.buffer.byteLength;
const big = (`token ${KEY} for ${MAIL}, আমার ঢাকা ব্যাংক; ` + "lorem ipsum dolor sit amet ".repeat(20) + "\n").repeat(400);
const t0 = performance.now();
for (let i = 0; i < 20; i++) {
  const m = ok(call({ op: "mask", text: big }), "big mask");
  assert.equal(ok(call({ op: "rehydrate", text: m.text }), "big rehydrate").text, big);
}
const ms = (performance.now() - t0) / 20;
for (let i = 0; i < 2000; i++) call({ op: "scan", text: `x ${i} ${MAIL}` });
const after = memory.buffer.byteLength;
assert.ok(after - before <= 16 * 1024 * 1024, `memory grew from ${before} to ${after}`);

const size = statSync(wasmPath).size;
console.log(
  `zuko_core.wasm OK: ${checks} checks, size ${(size / 1024 / 1024).toFixed(2)} MiB (${size} bytes), ` +
    `mask+rehydrate of ${(big.length / 1024).toFixed(0)} KiB in ${ms.toFixed(1)} ms, memory ${before} -> ${after} bytes`
);
assert.ok(size < 2 * 1024 * 1024, "wasm must stay under 2 MiB");
