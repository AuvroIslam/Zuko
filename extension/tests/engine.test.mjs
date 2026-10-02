import assert from "node:assert/strict";
import { test } from "node:test";
import { WasmEngine } from "../src/shared/engine.ts";
import { AWS, CARD, KEY, MAIL, realEngine, wasmPath } from "./helpers.mjs";
import { readFileSync } from "node:fs";

test("the WASM module needs no imports and loads through the C-ABI client", async () => {
  const module = await WebAssembly.compile(readFileSync(wasmPath()));
  assert.deepEqual(WebAssembly.Module.imports(module), []);
  const engine = await WasmEngine.load(module);
  assert.equal(engine.call({ op: "scan", text: "nothing here" }).ok, true);
});

test("mask / rehydrate round trip through the typed client", async () => {
  const e = await realEngine();
  const prompt = `key ${KEY}, mail ${MAIL}, card ${CARD}`;
  const m = e.mask(prompt);
  assert.equal(m.text, "key {{API_KEY_1}}, mail {{EMAIL_1}}, card {{CARD_1}}");
  assert.deepEqual(m.report.keys, ["API_KEY_1", "EMAIL_1", "CARD_1"]);
  assert.equal(e.rehydrate(m.text).text, prompt);
  assert.equal(e.mask(m.text).report.count, 0, "masking is idempotent");
});

test("vault export/load round trip keeps counters", async () => {
  const a = await realEngine();
  a.mask(`${KEY} ${MAIL}`);
  const vault = a.exportVault();
  assert.equal(vault.entries.length, 2);
  const b = await realEngine();
  assert.equal(b.loadVault(vault), 2);
  assert.equal(b.rehydrate("{{API_KEY_1}} {{EMAIL_1}}").text, `${KEY} ${MAIL}`);
  assert.equal(b.mask("x other@acme-corp.io").text, "x {{EMAIL_2}}");
});

test("a second engine instance has its own vault", async () => {
  const a = await realEngine();
  const b = await realEngine();
  a.mask(KEY);
  assert.equal(b.exportVault().entries.length, 0);
});

test("errors surface as exceptions with the engine's message", async () => {
  const e = await realEngine();
  assert.throws(() => e.transport.call({ op: "mask" }) && e.rehydrate(5), /./);
  assert.equal(e.transport.call({ op: "explode" }).ok, false);
  assert.throws(() => e.loadVault(42), /vault/i);
});

test("detects the secret shapes the browser cares about", async () => {
  const e = await realEngine();
  const kinds = e.scan(`aws ${AWS} openai ${KEY} mail ${MAIL}`).map((f) => f.kind);
  assert.ok(kinds.includes("API_KEY"));
  assert.ok(kinds.includes("EMAIL"));
});
