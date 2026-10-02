import assert from "node:assert/strict";
import { test } from "node:test";
import { mergeAddOnly, mergeVaults, base64Needles } from "../src/shared/vault.ts";
import { AWS, KEY, MAIL, memoryStore, realBrain, realEngine } from "./helpers.mjs";

const site = "chatgpt";

test("maskMany masks with full detection and reports the placeholders present", async () => {
  const brain = await realBrain();
  const r = await brain.maskMany([`use ${KEY} please`, "nothing here", ""], { site, mode: "full" });
  assert.deepEqual(r.texts, ["use {{API_KEY_1}} please", "nothing here", ""]);
  assert.equal(r.count, 1);
  assert.deepEqual(r.keys, ["API_KEY_1"]);
  assert.deepEqual(r.newKeys, ["API_KEY_1"]);
  assert.equal(r.via, "wasm");
  // The same value always gets the same placeholder, in any later text.
  const again = await brain.maskMany([`again ${KEY}`], { site, mode: "full" });
  assert.equal(again.texts[0], "again {{API_KEY_1}}");
  assert.deepEqual(again.newKeys, []);
});

test("known mode masks vault values only (no new detections)", async () => {
  const brain = await realBrain();
  await brain.maskMany([KEY], { site, mode: "full" });
  const r = await brain.maskMany([`${KEY} and ${MAIL}`], { site, mode: "known" });
  assert.equal(r.texts[0], `{{API_KEY_1}} and ${MAIL}`);
});

test("the vault is persisted to the session store and restored by a new Brain", async () => {
  const store = memoryStore();
  const a = await realBrain({ store });
  await a.maskMany([`${KEY} ${MAIL}`], { site, mode: "full" });
  assert.equal(store.data.get("vault").entries.length, 2);
  const b = await realBrain({ store }); // a service worker wake-up
  assert.equal(b.vaultSize(), 2);
  assert.deepEqual(b.rehydrate(["x {{API_KEY_1}}"]).texts, [`x ${KEY}`]);
});

test("resolve returns values only for keys that exist; known lists keys without values", async () => {
  const brain = await realBrain();
  await brain.maskMany([`${KEY} ${MAIL}`], { site, mode: "full" });
  const r = brain.resolve(["API_KEY_1", "NOPE_1"]);
  assert.deepEqual(Object.keys(r), ["API_KEY_1"]);
  assert.equal(r.API_KEY_1.value, KEY);
  assert.deepEqual(brain.knownKeys(), ["API_KEY_1", "EMAIL_1"]);
  assert.ok(!JSON.stringify(brain.knownKeys()).includes(KEY));
});

test("scan counts distinct items, never returns values, and counts known vault values", async () => {
  const brain = await realBrain();
  let r = brain.scan(`${KEY} ${KEY} and ${MAIL}`);
  assert.equal(r.count, 2);
  assert.ok(!JSON.stringify(r).includes(KEY) && !JSON.stringify(r).includes(MAIL));
  await brain.maskMany([KEY], { site, mode: "full" });
  r = brain.scan(`${KEY} and ${MAIL}`);
  assert.equal(r.count, 2, "one known value plus one new detection");
  assert.equal(brain.scan("nothing sensitive").count, 0);
});

test("the desktop app is used first and its vault is adopted", async () => {
  const appEngine = await realEngine(); // plays the app's own vault
  const calls = [];
  const link = {
    linked: true,
    async mask(text, s) {
      calls.push(["mask", s]);
      const r = appEngine.mask(text, "browser");
      return { text: r.text, report: r.report };
    },
    async fetchVault() {
      calls.push(["vault"]);
      return appEngine.exportVault();
    },
    event() {},
  };
  const brain = await realBrain({ link });
  const r = await brain.maskMany([`k ${KEY}`], { site, mode: "full" });
  assert.equal(r.via, "app");
  assert.equal(r.texts[0], "k {{API_KEY_1}}");
  assert.deepEqual(calls, [["mask", "chatgpt"], ["vault"]]);
  // The local engine now knows the app's entry, so tripwire and rehydration work offline.
  assert.equal(brain.rehydrate(["{{API_KEY_1}}"]).texts[0], KEY);
  assert.equal(brain.tripwire([`leak ${KEY}`]).action, "masked");
});

test("when the app fails mid-request the local engine masks instead", async () => {
  const link = { linked: true, async mask() { return null; }, async fetchVault() { return null; }, event() {} };
  const brain = await realBrain({ link });
  const r = await brain.maskMany([`k ${KEY}`], { site, mode: "full" });
  assert.equal(r.via, "wasm");
  assert.equal(r.texts[0], "k {{API_KEY_1}}");
});

test("mergeVaults: the app wins its keys, local-only values get fresh numbers on conflict", async () => {
  const local = await realEngine();
  local.mask(`${MAIL} ${KEY}`); // EMAIL_1 and API_KEY_1 locally
  const remote = await realEngine();
  remote.mask("other@acme-corp.io"); // EMAIL_1 in the app is a different value
  const { vault, renamed } = mergeVaults(local.exportVault(), remote.exportVault());
  const byKey = Object.fromEntries(vault.entries.map((e) => [e.key, e.value]));
  assert.equal(byKey.EMAIL_1, "other@acme-corp.io");
  assert.equal(byKey.EMAIL_2, MAIL, "the local email moved to a free number");
  assert.equal(byKey.API_KEY_1, KEY, "no conflict: key kept");
  assert.deepEqual(renamed, [{ from: "EMAIL_1", to: "EMAIL_2" }]);
  // Loadable, and the engine keeps counting from there.
  const e = await realEngine();
  e.loadVault(vault);
  assert.equal(e.mask("new third@acme-corp.io").text, "new {{EMAIL_3}}");
  // Same value on both sides: single entry, the app's key.
  const dup = mergeVaults(local.exportVault(), local.exportVault());
  assert.equal(dup.vault.entries.length, 2);
  assert.deepEqual(dup.renamed, []);
});

test("base64 needles match the value at every alignment inside a larger stream", () => {
  const value = "sk-live-Zq8x_Fake-Value-12345";
  const needles = base64Needles(value);
  for (const prefix of ["", "a", "ab", "abc", "abcd", "token="]) {
    for (const suffix of ["", "!", "&x=1", ";;"]) {
      const b64 = Buffer.from(prefix + value + suffix).toString("base64");
      assert.ok(needles.some((n) => b64.includes(n)), `prefix=${JSON.stringify(prefix)} suffix=${JSON.stringify(suffix)}`);
      const urlsafe = Buffer.from(prefix + value + suffix).toString("base64url");
      assert.ok(needles.some((n) => urlsafe.includes(n)), `urlsafe ${prefix}|${suffix}`);
    }
  }
  assert.deepEqual(base64Needles("short"), []);
});

test("counters accumulate per site", async () => {
  const brain = await realBrain();
  await brain.bump("claude", "masked", 3);
  await brain.bump("claude", "blocked", 1);
  await brain.bump("chatgpt", "uploads", 1);
  assert.deepEqual(brain.getStats().claude, { masked: 3, blocked: 1, uploads: 0, restored: 0 });
  assert.equal(brain.getStats().chatgpt.uploads, 1);
});

test("without an engine every operation says engine-unavailable", async () => {
  const { Brain, EngineUnavailable } = await import("../src/background/brain.ts");
  const brain = new Brain(memoryStore());
  await brain.attach(null, "wasm missing");
  assert.equal(brain.engineLoaded, false);
  assert.equal(brain.vaultSize(), 0);
  await assert.rejects(() => brain.maskMany(["x"], { site, mode: "full" }), EngineUnavailable);
  assert.throws(() => brain.tripwire(["x"]), EngineUnavailable);
  assert.throws(() => brain.scan("x"), EngineUnavailable);
  void AWS;
});

// ---- the app's local-AI deep scan ------------------------------------------------------------

const entry = (key, value, kind, label) => ({ key, value, kind, label, category: "pii", hint: null, hits: 0, created: 1, lastUsed: 1, source: "local-ai" });

/** A linked app whose local AI "finds" `Rahim Uddin` as NAME_1; records what it was asked. */
function aiLink({ localAi = { enabled: true, reachable: true, model: "gemma3:4b", waitForPromptScan: false }, reply, calls = [] } = {}) {
  return {
    linked: true,
    localAi,
    calls,
    async mask() {
      return null; // the local engine masks
    },
    async fetchVault() {
      return null;
    },
    event() {},
    async deepScan(text, siteId, wait) {
      calls.push({ text, site: siteId, wait });
      if (reply === null) return null;
      return (
        reply ?? {
          enabled: true,
          added: [{ key: "NAME_1", label: "Person name" }],
          vault: { entries: [entry("NAME_1", "Rahim Uddin", "NAME", "Person name")], counters: { NAME: 1 } },
        }
      );
    },
  };
}

const tickMs = (ms = 5) => new Promise((r) => setTimeout(r, ms));

test("AI deep scan, uploads wait: the masked text is scanned, then re-masked with what the AI learned", async () => {
  const link = aiLink();
  const brain = await realBrain({ link });
  const added = [];
  const r = await brain.maskMany([`Rahim Uddin sent ${KEY}`], { site, mode: "full", source: "file", onAiAdded: (n) => added.push(n) });
  assert.equal(r.texts[0], "{{NAME_1}} sent {{API_KEY_1}}");
  assert.equal(r.aiAdded, 1);
  assert.deepEqual(added, [1]);
  assert.ok(r.newKeys.includes("NAME_1"));
  assert.equal(r.count, 2);
  assert.deepEqual(link.calls, [{ text: `Rahim Uddin sent {{API_KEY_1}}`, site: "chatgpt", wait: true }]);
  assert.ok(!JSON.stringify(link.calls).includes(KEY), "the app is only ever sent masked text");
  // Later messages are masked by the session vault, no round trip needed.
  assert.equal((await brain.maskMany(["hi Rahim Uddin"], { site, mode: "known" })).texts[0], "hi {{NAME_1}}");
});

test("AI deep scan, prompts: sent in the background by default, merged for later messages", async () => {
  const link = aiLink();
  const brain = await realBrain({ link });
  const added = [];
  const r = await brain.maskMany(["tell Rahim Uddin hello"], { site, mode: "full", onAiAdded: (n) => added.push(n) });
  assert.equal(r.texts[0], "tell Rahim Uddin hello", "the first send is not held back");
  assert.equal(r.aiAdded, 0);
  assert.equal(link.calls[0].wait, false);
  await tickMs();
  assert.deepEqual(added, [1]);
  const next = await brain.maskMany(["tell Rahim Uddin goodbye"], { site, mode: "full" });
  assert.equal(next.texts[0], "tell {{NAME_1}} goodbye");
});

test("AI deep scan, prompts wait when the app says so", async () => {
  const link = aiLink({ localAi: { enabled: true, reachable: true, model: "gemma3:4b", waitForPromptScan: true } });
  const brain = await realBrain({ link });
  const r = await brain.maskMany(["tell Rahim Uddin hello"], { site, mode: "full" });
  assert.equal(r.texts[0], "tell {{NAME_1}} hello");
  assert.equal(link.calls[0].wait, true);
});

test("AI deep scan: off, unreachable, no answer, 'disabled' replies and known mode change nothing", async () => {
  const text = "tell Rahim Uddin hello";
  for (const localAi of [null, { enabled: false, reachable: true, model: "m", waitForPromptScan: true }, { enabled: true, reachable: false, model: "m", waitForPromptScan: true }]) {
    const link = aiLink({ localAi });
    const brain = await realBrain({ link });
    assert.equal((await brain.maskMany([text], { site, mode: "full", source: "file" })).texts[0], text);
    assert.equal(link.calls.length, 0);
  }
  const waiting = { enabled: true, reachable: true, model: "m", waitForPromptScan: true };
  for (const reply of [null, { enabled: false, added: [], vault: null }, { enabled: true, added: [], vault: { entries: [], counters: {} } }]) {
    const link = aiLink({ localAi: waiting, reply });
    const brain = await realBrain({ link });
    const r = await brain.maskMany([text], { site, mode: "full", source: "file" });
    assert.equal(r.texts[0], text);
    assert.equal(r.aiAdded, 0);
    assert.equal(brain.vaultSize(), 0);
  }
  const link = aiLink({ localAi: waiting });
  const brain = await realBrain({ link });
  await brain.maskMany([text], { site, mode: "known" });
  assert.equal(link.calls.length, 0);
  // A link that throws is the same as no answer.
  const boom = aiLink({ localAi: waiting });
  boom.deepScan = async () => {
    throw new Error("host gone");
  };
  assert.equal((await (await realBrain({ link: boom })).maskMany([text], { site, mode: "full", source: "file" })).texts[0], text);
});

test("mergeAddOnly never drops or renames a local entry", () => {
  const local = { entries: [entry("NAME_1", "Alice Roy", "NAME", "Person name"), entry("API_KEY_1", "k-1", "API_KEY", "Key")], counters: { NAME: 1, API_KEY: 1 } };
  const remote = {
    entries: [
      entry("NAME_1", "Rahim Uddin", "NAME", "Person name"), // same key, different value
      entry("API_KEY_1", "k-1", "API_KEY", "Key"), // already known by value
      entry("ADDRESS_1", "House 12, Road 5", "ADDRESS", "Street address"),
    ],
    counters: { NAME: 1, API_KEY: 1, ADDRESS: 1 },
  };
  const { vault, added } = mergeAddOnly(local, remote);
  assert.deepEqual(vault.entries.slice(0, 2), local.entries, "local entries untouched and first");
  assert.deepEqual(added, [{ key: "NAME_2", label: "Person name" }, { key: "ADDRESS_1", label: "Street address" }]);
  assert.equal(vault.entries.length, 4);
  assert.deepEqual(vault.counters, { NAME: 2, API_KEY: 1, ADDRESS: 1 });
  assert.equal(mergeAddOnly(vault, { entries: [], counters: {} }).added.length, 0);
});

test("placeholders the app learned before are fetched so they can be restored", async () => {
  const link = aiLink();
  link.mask = async (t) => ({ text: t.replace("Rahim Uddin", "{{NAME_1}}"), report: { count: 1, newKeys: [], keys: ["NAME_1"] } });
  let fetched = 0;
  link.fetchVault = async () => {
    fetched++;
    return { entries: [entry("NAME_1", "Rahim Uddin", "NAME", "Person name")], counters: { NAME: 1 } };
  };
  const brain = await realBrain({ link });
  const r = await brain.maskMany(["hi Rahim Uddin"], { site, mode: "full" });
  assert.equal(r.via, "app");
  assert.equal(fetched, 1);
  assert.equal(brain.resolve(["NAME_1"]).NAME_1.value, "Rahim Uddin");
});
