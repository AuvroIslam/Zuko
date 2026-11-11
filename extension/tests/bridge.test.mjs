import assert from "node:assert/strict";
import { afterEach, test } from "node:test";
import { BridgeUnavailable, createPageBridge, READY_EVENT } from "../src/main-world/bridge.ts";
import { ContentBridge } from "../src/content/bridge.ts";
import { tick } from "./helpers.mjs";

// Open MessagePorts keep Node alive: close every channel a test created.
const channels = [];
afterEach(() => {
  for (const c of channels.splice(0)) {
    c.port1.close();
    c.port2.close();
  }
});
function tracked() {
  const c = new MessageChannel();
  channels.push(c);
  return c;
}

/** The smallest window the two bridges need: events, postMessage with ports, MessageChannel. */
function fakeWindow() {
  const win = new EventTarget();
  win.CustomEvent = CustomEvent;
  win.MessageChannel = function () {
    return tracked();
  };
  win.location = { origin: "https://chatgpt.com" };
  win.postMessage = (data, _origin, ports = []) => {
    queueMicrotask(() => {
      const ev = new Event("message");
      ev.data = data;
      ev.source = win;
      ev.ports = ports;
      win.dispatchEvent(ev);
    });
  };
  return win;
}

const handlers = (log = []) => ({
  async request(req) {
    log.push(req);
    if (req.type === "maskMany") return { texts: req.texts.map((t) => t.toUpperCase()), count: 1, keys: [], newKeys: [] };
    throw new Error("nope");
  },
  notify: (level, text) => log.push({ notify: level, text }),
  event: (kind, count) => log.push({ event: kind, count }),
});

test("handshake works when the page guard loads first", async () => {
  const win = fakeWindow();
  const page = createPageBridge(win);
  const log = [];
  const content = new ContentBridge(win, handlers(log));
  content.start();
  await tick(5);
  assert.equal(page.connected(), true);
  const r = await page.request({ type: "maskMany", texts: ["abc"], mode: "full" });
  assert.deepEqual(r.texts, ["ABC"]);
});

test("handshake works when the content script loads first (its first offer is lost, the ready event re-offers)", async () => {
  const win = fakeWindow();
  const content = new ContentBridge(win, handlers());
  content.start();
  await tick(5); // the offer went nowhere: nobody listens yet
  const page = createPageBridge(win);
  await tick(5);
  assert.equal(page.connected(), true);
  assert.equal((await page.request({ type: "maskMany", texts: ["x"], mode: "known" })).ok, true);
});

test("requests made before the port exists are queued and answered once it arrives", async () => {
  const win = fakeWindow();
  const page = createPageBridge(win);
  const pending = page.request({ type: "maskMany", texts: ["queued"], mode: "full" });
  await tick(5);
  new ContentBridge(win, handlers()).start();
  assert.deepEqual((await pending).texts, ["QUEUED"]);
});

test("a page script cannot get a port after the guard said hello, and cannot replace the guard's port", async () => {
  const win = fakeWindow();
  const page = createPageBridge(win);
  new ContentBridge(win, handlers()).start();
  await tick(5);
  assert.equal(page.connected(), true);

  // An attacker listens for any message carrying a port, then fires the ready event.
  const stolen = [];
  win.addEventListener("message", (e) => e.ports?.length && stolen.push(e.ports[0]));
  win.dispatchEvent(new CustomEvent(READY_EVENT));
  await tick(10);
  assert.equal(stolen.length, 0, "no new port is handed out");

  // ...or posts its own port, hoping the guard adopts it.
  const evil = tracked();
  const got = [];
  evil.port1.onmessage = (e) => got.push(e.data);
  win.postMessage({ __zuko: "port" }, "*", [evil.port2]);
  await tick(10);
  assert.deepEqual(got, [], "the guard keeps talking to the original port only");
  assert.equal((await page.request({ type: "maskMany", texts: ["ok"], mode: "full" })).ok, true);
  evil.port1.close();
});

test("state pushed by the content script reaches the page; before that the page assumes protection is on", async () => {
  const win = fakeWindow();
  const page = createPageBridge(win);
  assert.equal(page.state.enabled, true);
  assert.equal(page.state.known, false);
  const content = new ContentBridge(win, handlers());
  content.start();
  content.pushState({ enabled: false, vaultSize: 3, engine: true });
  await tick(5);
  assert.deepEqual({ enabled: page.state.enabled, vaultSize: page.state.vaultSize, known: page.state.known }, { enabled: false, vaultSize: 3, known: true });
  content.pushState({ enabled: true, vaultSize: 4, engine: false });
  await tick(5);
  assert.equal(page.state.vaultSize, 4);
  assert.equal(page.state.engine, false);
});

test("handler failures and timeouts reject with BridgeUnavailable", async () => {
  const win = fakeWindow();
  const page = createPageBridge(win);
  new ContentBridge(win, handlers()).start();
  await tick(5);
  await assert.rejects(page.request({ type: "tripwire", texts: ["x"] }), BridgeUnavailable); // handler throws
  const lonely = createPageBridge(fakeWindow()); // no content script at all
  await assert.rejects(lonely.request({ type: "maskMany", texts: ["x"], mode: "full" }, 30), BridgeUnavailable);
});

test("notify and event messages reach the handlers", async () => {
  const win = fakeWindow();
  const page = createPageBridge(win);
  const log = [];
  new ContentBridge(win, handlers(log)).start();
  await tick(5);
  assert.equal(page.post({ type: "notify", level: "error", text: "blocked" }), true);
  assert.equal(page.post({ type: "event", kind: "blocked", count: 2, keys: ["A_1"] }), true);
  await tick(5);
  assert.deepEqual(log, [{ notify: "error", text: "blocked" }, { event: "blocked", count: 2 }]);
  assert.equal(createPageBridge(fakeWindow()).post({ type: "notify", level: "info", text: "x" }), false, "no port yet");
});
