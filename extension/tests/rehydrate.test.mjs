import assert from "node:assert/strict";
import { test } from "node:test";
import { Rehydrator } from "../src/content/rehydrate.ts";
import { installCopyHandler, restoreText, writeRestoredClipboard } from "../src/content/clipboard.ts";
import { buildNote, VariantMatcher } from "../src/shared/placeholders.ts";
import { KEY, MAIL, makeWindow, tick } from "./helpers.mjs";

const NOTE = buildNote(["API_KEY_1"]);

/** Values come from a map; `lazy` makes them appear only after load() (like the service worker round trip). */
class MapSource {
  constructor(all, { lazy = false } = {}) {
    this.all = all;
    this.cache = new Map(lazy ? [] : Object.entries(all));
    this.loads = [];
  }
  keys() {
    return new Set(Object.keys(this.all));
  }
  value(k) {
    return this.cache.get(k);
  }
  async load(keys) {
    this.loads.push(keys);
    await tick(1);
    for (const k of keys) if (k in this.all) this.cache.set(k, this.all[k]);
  }
}

const PAGE = `
<main>
  <div data-message-author-role="user"><div class="whitespace-pre-wrap">${NOTE}\n\nMy key is {{API_KEY_1}} and my mail {{EMAIL_1}}</div></div>
  <div data-message-author-role="assistant">
    <div class="markdown">
      <p>Put this in <code>.env</code>, using API_KEY_1 or [EMAIL_1] or {{api_key_1}}:</p>
      <pre><code class="language-bash"><span>OPENAI_API_KEY</span><span>=</span><span>{{</span><span>API_KEY_1</span><span>}}</span>
<span>OWNER=</span><span>{</span><span>{ EMAIL_1 }</span><span>}</span></code></pre>
      <p>Unrelated: HTTP_2, {{TOKEN_9}} and {{ name }} stay.</p>
    </div>
  </div>
</main>
<form>
  <div id="prompt-textarea" contenteditable="true" class="ProseMirror">{{API_KEY_1}} typed by the user</div>
  <textarea>{{API_KEY_1}}</textarea>
</form>
<script>var x = "{{API_KEY_1}}";</script>
<style>.a::after { content: "{{API_KEY_1}}"; }</style>
`;

function setup({ lazy = false, extra } = {}) {
  const win = makeWindow({ html: `<!doctype html><html><body>${PAGE}</body></html>` });
  win.CSS = { highlights: new Map() };
  win.Highlight = class extends Set {};
  const source = new MapSource({ API_KEY_1: KEY, EMAIL_1: MAIL, ...(extra ?? {}) }, { lazy });
  const events = [];
  const toggles = [];
  const r = new Rehydrator({
    doc: win.document,
    site: "chatgpt",
    source,
    delayMs: 5,
    onRestored: (count, keys) => events.push({ count, keys }),
    onToggle: (v) => toggles.push(v),
  });
  return { win, doc: win.document, source, r, events, toggles, highlight: () => win.CSS.highlights.get("zuko-restored") };
}

const text = (el) => el.textContent;
const q = (doc, sel) => doc.querySelector(sel);

/** Structure with every text node blanked: it must be identical before and after rehydration. */
function skeleton(node) {
  if (node.nodeType === 3) return "#text";
  return `<${node.nodeName}${node.id ? "#" + node.id : ""}>${[...node.childNodes].map(skeleton).join("")}</${node.nodeName}>`;
}

test("only nodeValue changes: no element is added, removed or moved", async () => {
  const { win, doc, r } = setup();
  const before = skeleton(doc.body);
  const records = [];
  const obs = new win.MutationObserver((rs) => records.push(...rs));
  obs.observe(doc.body, { childList: true, subtree: true, characterData: true, attributes: true });
  r.start();
  await r.flushNow();
  records.push(...obs.takeRecords());
  obs.disconnect();
  assert.ok(records.length > 0, "something was restored");
  assert.deepEqual([...new Set(records.map((m) => m.type))], ["characterData"], "only text node data changed");
  assert.equal(skeleton(doc.body), before);
  r.stop();
});

test("placeholders in messages become the real values, including mangled forms", async () => {
  const { doc, r } = setup();
  r.start();
  await r.flushNow();
  const user = q(doc, '[data-message-author-role="user"]');
  assert.equal(text(user).trim(), `My key is ${KEY} and my mail ${MAIL}`, "restored, and the Zuko note is hidden");
  const p = q(doc, ".markdown p");
  assert.equal(text(p), `Put this in .env, using ${KEY} or ${MAIL} or ${KEY}:`);
  assert.equal(text(doc.querySelectorAll(".markdown p")[1]), "Unrelated: HTTP_2, {{TOKEN_9}} and {{ name }} stay.");
  r.stop();
});

test("a placeholder split over syntax-highlight spans is restored without touching the spans", async () => {
  const { doc, r } = setup();
  const code = q(doc, "pre code");
  const spans = [...code.querySelectorAll("span")];
  r.start();
  await r.flushNow();
  assert.equal(text(code), `OPENAI_API_KEY=${KEY}\nOWNER=${MAIL}`);
  assert.deepEqual([...code.querySelectorAll("span")], spans, "same span elements, same order");
  // The value sits in the first node of each placeholder; the covered slices are emptied.
  assert.equal(spans[2].textContent, KEY);
  assert.equal(spans[3].textContent, "");
  assert.equal(spans[4].textContent, "");
  r.stop();
});

test("the composer, textareas, scripts and styles are never touched", async () => {
  const { doc, r } = setup();
  r.start();
  await r.flushNow();
  assert.equal(text(q(doc, "#prompt-textarea")), "{{API_KEY_1}} typed by the user");
  assert.equal(q(doc, "textarea").value ?? q(doc, "textarea").textContent, "{{API_KEY_1}}");
  assert.equal(q(doc, "script").textContent, 'var x = "{{API_KEY_1}}";');
  assert.ok(q(doc, "style").textContent.includes("{{API_KEY_1}}"));
  r.stop();
});

test("restored values are highlighted: one range per value, covering exactly the value", async () => {
  const { r, highlight } = setup();
  r.start();
  await r.flushNow();
  const ranges = [...highlight()];
  const covered = ranges.map((x) => x.toString()).sort();
  // user: key + mail; assistant p: key, mail, key; code: key, mail.
  assert.deepEqual(covered, [KEY, KEY, KEY, KEY, MAIL, MAIL, MAIL].sort());
  r.stop();
});

test("Alt+R: toggling shows the exact original masked text, and back", async () => {
  const { doc, r, toggles, highlight } = setup();
  const originals = [...doc.querySelectorAll("main *")].filter((e) => e.children.length === 0).map((e) => e.textContent);
  r.start();
  await r.flushNow();
  const restored = [...doc.querySelectorAll("main *")].filter((e) => e.children.length === 0).map((e) => e.textContent);
  assert.notDeepEqual(restored, originals);

  assert.equal(r.toggle(), false);
  assert.deepEqual([...doc.querySelectorAll("main *")].filter((e) => e.children.length === 0).map((e) => e.textContent), originals, "byte-identical masked view (note included)");
  assert.equal(highlight().size, 0);
  await tick(20); // our own writes must not trigger a re-restore while masked
  assert.deepEqual([...doc.querySelectorAll("main *")].filter((e) => e.children.length === 0).map((e) => e.textContent), originals);

  assert.equal(r.toggle(), true);
  assert.deepEqual([...doc.querySelectorAll("main *")].filter((e) => e.children.length === 0).map((e) => e.textContent), restored);
  assert.equal(highlight().size, 7);
  assert.deepEqual([...highlight()].map((x) => x.toString()).sort(), [KEY, KEY, KEY, KEY, MAIL, MAIL, MAIL].sort(), "ranges were rebuilt after the text came back");
  assert.deepEqual(toggles, [false, true]);
  r.stop();
});

test("streaming: a placeholder arriving token by token is restored once complete; page rewrites are restored again", async () => {
  const { doc, r } = setup();
  const p = doc.createElement("p");
  const node = doc.createTextNode("");
  p.appendChild(node);
  q(doc, '[data-message-author-role="assistant"] .markdown').appendChild(p);
  r.start();
  await r.flushNow();
  for (const chunk of ["Use {", "{API", "_KEY", "_1}", "} now"]) {
    node.nodeValue += chunk; // what React does as tokens stream in
    await r.flushNow();
  }
  assert.equal(node.nodeValue, `Use ${KEY} now`);
  // The page re-renders the same node with its own (masked) text: we restore it again.
  node.nodeValue = "Again {{EMAIL_1}}.";
  await r.flushNow();
  assert.equal(node.nodeValue, `Again ${MAIL}.`);
  // ...and the masked view of that node is what the page wrote.
  r.toggle();
  assert.equal(node.nodeValue, "Again {{EMAIL_1}}.");
  r.stop();
});

test("values load on demand: unresolved keys are fetched once, then restored", async () => {
  const { doc, r, source } = setup({ lazy: true });
  r.start();
  await r.flushNow();
  assert.equal(text(q(doc, ".markdown p")), `Put this in .env, using ${KEY} or ${MAIL} or ${KEY}:`);
  assert.equal(source.loads.length, 1);
  assert.deepEqual([...source.loads[0]].sort(), ["API_KEY_1", "EMAIL_1"]);
  r.stop();
});

test("a key the vault no longer has is left as written", async () => {
  const win = makeWindow({ html: '<!doctype html><html><body><main><div data-message-author-role="assistant"><p>{{API_KEY_1}} and {{EMAIL_1}}</p></div></main></body></html>' });
  const source = new MapSource({ API_KEY_1: KEY, EMAIL_1: MAIL });
  source.load = async () => {}; // the service worker could not resolve EMAIL_1 any more
  source.cache = new Map([["API_KEY_1", KEY]]);
  const r = new Rehydrator({ doc: win.document, site: "chatgpt", source, delayMs: 5 });
  r.start();
  await r.flushNow();
  assert.equal(win.document.querySelector("p").textContent, `${KEY} and {{EMAIL_1}}`);
  r.stop();
});

test("an unrecognised page (no message selectors match) still gets restored in <main>, never in editable regions", async () => {
  const win = makeWindow({ html: "<!doctype html><html><body><main><section><p>reply: {{API_KEY_1}}</p></section></main><div contenteditable=\"true\">{{API_KEY_1}}</div></body></html>" });
  const r = new Rehydrator({ doc: win.document, site: "deepseek", source: new MapSource({ API_KEY_1: KEY }), delayMs: 5 });
  r.start();
  await r.flushNow();
  assert.equal(win.document.querySelector("p").textContent, `reply: ${KEY}`);
  assert.equal(win.document.querySelector("[contenteditable]").textContent, "{{API_KEY_1}}");
  r.stop();
});

test("stop() puts the page back exactly as the site wrote it", async () => {
  const { doc, r } = setup();
  const before = doc.body.innerHTML;
  r.start();
  await r.flushNow();
  assert.notEqual(doc.body.innerHTML, before);
  r.stop();
  assert.equal(doc.body.innerHTML, before);
});

test("refresh() picks up keys that appear later (new vault entries)", async () => {
  const win = makeWindow({ html: '<!doctype html><html><body><main><div data-message-author-role="assistant"><p>mail {{EMAIL_2}}</p></div></main></body></html>' });
  const all = {};
  const source = new MapSource(all);
  const r = new Rehydrator({ doc: win.document, site: "chatgpt", source, delayMs: 5 });
  r.start();
  await r.flushNow();
  assert.equal(win.document.querySelector("p").textContent, "mail {{EMAIL_2}}");
  all.EMAIL_2 = "two@acme.io";
  r.refresh();
  await r.flushNow();
  assert.equal(win.document.querySelector("p").textContent, "mail two@acme.io");
  r.stop();
});

// ---- copy rehydration ----------------------------------------------------------------------

function selectText(win, node) {
  const range = win.document.createRange();
  range.selectNodeContents(node);
  const sel = win.getSelection();
  sel.removeAllRanges();
  sel.addRange(range);
}

test("copy event: selected placeholders are replaced by real values in the clipboard", async () => {
  const win = makeWindow({ html: '<!doctype html><html><body><p id="a">OPENAI_API_KEY={{API_KEY_1}}</p><div contenteditable="true" id="c">{{API_KEY_1}}</div></body></html>' });
  const source = new MapSource({ API_KEY_1: KEY });
  const matcher = new VariantMatcher(["API_KEY_1"]);
  installCopyHandler({ doc: win.document, matcher: () => matcher, source, enabled: () => true });
  const copy = (node) => {
    selectText(win, node);
    const data = {};
    const ev = new win.Event("copy", { bubbles: true, cancelable: true });
    ev.clipboardData = { setData: (t, v) => (data[t] = v) };
    node.dispatchEvent(ev);
    return { data, prevented: ev.defaultPrevented };
  };
  let r = copy(win.document.getElementById("a"));
  assert.deepEqual(r, { data: { "text/plain": `OPENAI_API_KEY=${KEY}` }, prevented: true });
  r = copy(win.document.getElementById("c"));
  assert.deepEqual(r, { data: {}, prevented: false }, "the user's own typing is never rewritten");
  win.document.getElementById("a").textContent = "no placeholder here";
  assert.deepEqual(copy(win.document.getElementById("a")), { data: {}, prevented: false });
});

test("copy button path: values are loaded, restored, escaped for HTML, and written from the isolated world", async () => {
  const win = makeWindow();
  const written = [];
  Object.defineProperty(win.navigator, "clipboard", {
    value: {
      writeText: async (t) => written.push(["text", t]),
      write: async (items) => written.push(["write", items]),
    },
  });
  win.ClipboardItem = class {
    constructor(data) {
      this.data = data;
    }
  };
  win.Blob = class {
    constructor(parts, opts) {
      this.text = parts.join("");
      this.type = opts.type;
    }
  };
  const source = new MapSource({ API_KEY_1: "a<b>&c" }, { lazy: true });
  const env = { doc: win.document, matcher: () => new VariantMatcher(["API_KEY_1"]), source };

  assert.deepEqual(await writeRestoredClipboard({ "text/plain": "k={{API_KEY_1}}" }, env), { written: true });
  assert.deepEqual(written[0], ["text", "k=a<b>&c"]);
  assert.equal(source.loads.length, 1, "the value was fetched on demand");

  assert.deepEqual(await writeRestoredClipboard({ "text/plain": "k={{API_KEY_1}}", "text/html": "<p>k={{API_KEY_1}}</p>" }, env), { written: true });
  const items = written[1][1][0].data;
  assert.equal(items["text/plain"].text, "k=a<b>&c");
  assert.equal(items["text/html"].text, "<p>k=a&lt;b&gt;&amp;c</p>");

  assert.deepEqual(await writeRestoredClipboard({ "text/plain": "nothing 42" }, env), { written: false }, "nothing to restore: the page copies for itself");
  assert.equal(await restoreText("{{EMAIL_9}}", env), null);
});
