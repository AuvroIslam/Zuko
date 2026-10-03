// NativeLink's heartbeat: a linked extension keeps saying hello so the desktop app keeps
// showing it as connected, and a link whose app went away is dropped and picked up again.
// Short timings stand in for the real 25 s beat and 20 s retry throttle.

import assert from "node:assert/strict";
import { test } from "node:test";
import { NativeLink, HEARTBEAT_MS } from "../src/background/native.ts";
import { tick } from "./helpers.mjs";

/** What `connectNative` returns next: a port whose far end is `handler(message)`. */
let factory = null;
globalThis.chrome = {
  runtime: {
    lastError: undefined,
    connectNative: () => factory(),
  },
};

/** A port to a fake host + app. `handler` returns the reply, or undefined for silence. */
function hostPort(handler) {
  const onMsg = [];
  const onDisc = [];
  let gone = false;
  const port = {
    sent: [],
    postMessage(m) {
      if (gone) throw new Error("Attempting to use a disconnected port object");
      port.sent.push(m);
      setTimeout(() => {
        const reply = handler(m);
        if (reply && !gone) onMsg.forEach((l) => l({ ...reply, id: m.id }));
      }, 0);
    },
    disconnect() {
      gone = true;
    },
    get gone() {
      return gone;
    },
    onMessage: { addListener: (l) => onMsg.push(l) },
    onDisconnect: { addListener: (l) => onDisc.push(l) },
  };
  return port;
}

const hellos = (port) => port.sent.filter((m) => m.op === "hello").length;

test("the real heartbeat fits twice into the app's 60 s 'connected' window", () => {
  assert.ok(HEARTBEAT_MS <= 30_000 && HEARTBEAT_MS >= 10_000, `${HEARTBEAT_MS}`);
});

test("a linked extension says hello on every beat, and stops once the link is gone", async () => {
  let port;
  factory = () => (port = hostPort(() => ({ ok: true, app: "zuko", version: "1.0.0" })));
  const link = new NativeLink("9.9.9", "app.zuko.host", { heartbeatMs: 40 });
  assert.equal(await link.connect(true), true);
  assert.equal(hellos(port), 1, "the hello that links");
  await tick(190);
  assert.ok(hellos(port) >= 4, `${hellos(port)} hellos`);
  assert.ok(port.sent.every((m) => m.op !== "hello" || m.version === "9.9.9"), "each beat says who is calling");
  assert.equal(link.linked, true);

  link.disconnect();
  const after = hellos(port);
  await tick(150);
  assert.equal(hellos(port), after, "no beats without a link");
});

test("a beat the host answers for a closed app drops the link; the next tick links again", async () => {
  let appUp = true;
  let port;
  factory = () => (port = hostPort((m) => (appUp ? { ok: true, app: "zuko", version: "1.0.0" } : m.op === "hello" ? { ok: false, error: "Zuko desktop app is not running" } : undefined)));
  const link = new NativeLink("9.9.9", "app.zuko.host", { heartbeatMs: 40, beatTimeoutMs: 30, retryAfterMs: 0 });
  let changes = 0;
  link.onChange = () => (changes += 1);
  let synced = 0;
  link.onLinked = () => (synced += 1);
  assert.equal(await link.connect(true), true);
  assert.deepEqual([changes, synced], [1, 1]);

  appUp = false;
  await tick(120);
  assert.equal(link.linked, false, "the link is dropped");
  assert.match(link.lastError ?? "", /not running/);
  assert.equal(port.gone, true, "and the host process released");
  assert.equal(changes, 2);

  // The app is back: the next tick (the service worker's alarm) links and re-syncs.
  appUp = true;
  await link.tick();
  assert.equal(link.linked, true);
  assert.equal(synced, 2, "policy and vault are fetched again");
  link.disconnect();
});

test("one unanswered beat is forgiven, two in a row drop the link", async () => {
  let answer = true;
  factory = () => hostPort(() => (answer ? { ok: true, app: "zuko", version: "1.0.0" } : undefined));
  // Beats driven by hand: the timer is far off, each unanswered beat gives up after 30 ms.
  const link = new NativeLink("9.9.9", "app.zuko.host", { heartbeatMs: 60_000, beatTimeoutMs: 30 });
  assert.equal(await link.connect(true), true);
  answer = false;
  await link.tick();
  assert.equal(link.linked, true, "one late answer is not a dead app");
  answer = true;
  await link.tick();
  assert.equal(link.linked, true);
  answer = false;
  await link.tick();
  await link.tick();
  assert.equal(link.linked, false, "two silent beats in a row are");
});

test("without a registered host, ticks retry quietly and respect the throttle", async () => {
  let attempts = 0;
  factory = () => {
    attempts += 1;
    const disc = [];
    const port = {
      postMessage() {},
      disconnect() {},
      onMessage: { addListener() {} },
      onDisconnect: { addListener: (l) => disc.push(l) },
    };
    setTimeout(() => {
      globalThis.chrome.runtime.lastError = { message: "Specified native messaging host not found." };
      disc.forEach((l) => l());
      globalThis.chrome.runtime.lastError = undefined;
    }, 0);
    return port;
  };
  const link = new NativeLink("9.9.9", "app.zuko.host", { heartbeatMs: 40, retryAfterMs: 60_000 });
  assert.equal(await link.connect(true), false);
  assert.match(link.lastError, /not found/);
  await link.tick();
  await link.tick();
  assert.equal(attempts, 1, "inside the throttle window nothing is retried");
});
