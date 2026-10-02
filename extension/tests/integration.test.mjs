// Everything together, in one process: the page's net guard (MAIN world entry), the content
// script (isolated world entry) over a real MessageChannel handshake, the service worker on a
// fake chrome, the real WASM engine, and a jsdom page. Uses the real entry points
// (src/main-world/net-guard.ts, src/content/index.ts), so the wiring in them is exercised too.

import assert from "node:assert/strict";
import { after, test } from "node:test";
import { page, popup, send } from "./fake-chrome.mjs";
import { AWS, KEY, MAIL, makeWindow, recordFetch, tick } from "./helpers.mjs";
import { hasNote, stripNote } from "../src/shared/placeholders.ts";

const channels = [];
after(() => {
  for (const c of channels) {
    c.port1.close();
    c.port2.close();
  }
});

const PAGE = `
<main>
  <div data-message-author-role="assistant"><p id="reply">nothing yet</p></div>
</main>
<form><div id="prompt-textarea" contenteditable="true" class="ProseMirror"></div></form>
<input type="file" id="f">`;

// ---- the page ----------------------------------------------------------------------------------

const win = makeWindow({ url: "https://chatgpt.com/", html: `<!doctype html><html><body>${PAGE}</body></html>` });
win.MessageChannel = function () {
  const c = new MessageChannel();
  channels.push(c);
  return c;
};
// jsdom's postMessage ignores transferred ports; deliver them like a browser does.
win.postMessage = (data, _origin, ports = []) => {
  queueMicrotask(() => {
    const ev = new win.Event("message");
    ev.data = data;
    ev.source = win;
    ev.ports = ports;
    win.dispatchEvent(ev);
  });
};
win.DataTransfer = class {
  constructor() {
    this.files = [];
    this.items = { add: (f) => this.files.push(f) };
  }
};
const calls = recordFetch(win);
win.XMLHttpRequest.prototype.send = function () {};
Object.assign(globalThis, { window: win, document: win.document, location: win.location });

// chrome.runtime.sendMessage: the content script's calls go to the service worker; the service
// worker's own calls (to the offscreen document) keep their fake.
const offscreenSend = chrome.runtime.sendMessage;
let swAlive = true;
chrome.runtime.sendMessage = async (msg) => {
  if (msg?.target === "offscreen") return offscreenSend(msg);
  if (!swAlive) throw new Error("Could not establish connection. Receiving end does not exist.");
  return send(msg, page);
};

// MAIN world first, then the isolated world, as the manifest orders them.
await import("../src/main-world/net-guard.ts");
await import("../src/content/index.ts");
await tick(60);

const CONV = "https://chatgpt.com/backend-api/f/conversation";
const conv = (text) => JSON.stringify({ action: "next", messages: [{ id: "m1", author: { role: "user" }, content: { content_type: "text", parts: [text] } }] });
const reply = () => win.document.getElementById("reply");

test("a prompt is masked on its way out, and the vault is shared with the service worker", async () => {
  await win.fetch(CONV, { method: "POST", body: conv(`my key is ${KEY} and my mail ${MAIL}`) });
  const sent = JSON.parse(calls.at(-1).init.body).messages[0].content.parts[0];
  assert.ok(hasNote(sent));
  assert.equal(stripNote(sent), "my key is {{API_KEY_1}} and my mail {{EMAIL_1}}");
  assert.ok(win.document.querySelector("zuko-overlay"), "the user was told (toast host exists)");
});

test("an answer that mentions the placeholders shows the real values on screen", async () => {
  reply().firstChild.nodeValue = "Put OPENAI_API_KEY={{API_KEY_1}} in .env and mail {{ EMAIL_1 }}.";
  await tick(400); // debounce + value lookup through the service worker
  assert.equal(reply().textContent, `Put OPENAI_API_KEY=${KEY} in .env and mail ${MAIL}.`);
});

test("Alt+R shows the masked text and back", async () => {
  const press = () => {
    const e = new win.Event("keydown", { bubbles: true, cancelable: true });
    Object.assign(e, { altKey: true, ctrlKey: false, metaKey: false, shiftKey: false, code: "KeyR", key: "r" });
    win.dispatchEvent(e);
    return e;
  };
  const e = press();
  assert.equal(e.defaultPrevented, true);
  assert.equal(reply().textContent, "Put OPENAI_API_KEY={{API_KEY_1}} in .env and mail {{ EMAIL_1 }}.");
  press();
  assert.equal(reply().textContent, `Put OPENAI_API_KEY=${KEY} in .env and mail ${MAIL}.`);
});

test("a file chosen in the upload dialog is sanitized before the page sees it", async () => {
  const input = win.document.getElementById("f");
  const seen = [];
  win.document.addEventListener("change", (e) => seen.push(e.target.files.map((f) => f.name)));
  Object.defineProperty(input, "files", { value: [new File([`AWS_KEY=${AWS}\n`], "creds.env")], configurable: true, writable: true });
  input.dispatchEvent(new win.Event("change", { bubbles: true }));
  assert.deepEqual(seen, []);
  await tick(150);
  assert.deepEqual(seen, [["creds.env"]]);
  assert.ok(!(await input.files[0].text()).includes(AWS));
  assert.match(await input.files[0].text(), /^AWS_KEY=\{\{API_KEY_\d+\}\}\n$/);
});

test("switching the site off in the popup: nothing is masked, the page shows what the site wrote; on again, all is back", async () => {
  await send({ type: "setSite", site: "chatgpt", enabled: false }, popup);
  await tick(50);
  const body = conv(`plain ${AWS}`);
  const init = { method: "POST", body };
  await win.fetch(CONV, init);
  assert.equal(calls.at(-1).init, init, "untouched while switched off");
  assert.equal(reply().textContent, "Put OPENAI_API_KEY={{API_KEY_1}} in .env and mail {{ EMAIL_1 }}.", "DOM restored to the site's own text");

  await send({ type: "setSite", site: "chatgpt", enabled: true }, popup);
  await tick(400);
  assert.equal(reply().textContent, `Put OPENAI_API_KEY=${KEY} in .env and mail ${MAIL}.`);
  await win.fetch(CONV, { method: "POST", body: conv(`again ${AWS}`) });
  assert.ok(!calls.at(-1).init.body.includes(AWS));
});

test("the service worker dies: a prompt with an obvious secret is blocked, an ordinary one still goes", async () => {
  swAlive = false;
  const before = calls.length;
  await assert.rejects(win.fetch(CONV, { method: "POST", body: conv(`use ${KEY}`) }), /Zuko blocked/);
  assert.equal(calls.length, before, "nothing was sent");
  const init = { method: "POST", body: conv("How do I reverse a list?") };
  await win.fetch(CONV, init);
  assert.equal(calls.at(-1).init, init);
  swAlive = true;
});
