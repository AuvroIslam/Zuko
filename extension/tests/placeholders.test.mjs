import assert from "node:assert/strict";
import { test } from "node:test";
import { VariantMatcher, buildNote, canonicalKeys, hasNote, parseKey, replaceVariants, stripNote, withNote } from "../src/shared/placeholders.ts";

const KEYS = ["API_KEY_1", "EMAIL_1", "EMAIL_12", "PERSON_2", "CARD_1"];
const M = new VariantMatcher(KEYS);
const VALUES = { API_KEY_1: "sk-REAL", EMAIL_1: "a@b.io", EMAIL_12: "twelve@b.io", PERSON_2: "Rahim", CARD_1: "4242" };
const restore = (t) => replaceVariants(t, M, (k) => VALUES[k]).text;

test("canonical and spaced placeholders are restored", () => {
  assert.equal(restore("use {{API_KEY_1}} now"), "use sk-REAL now");
  assert.equal(restore("use {{ API_KEY_1 }} now"), "use sk-REAL now");
});

test("tolerates the manglings models produce", () => {
  const cases = {
    "[API_KEY_1]": "sk-REAL", // single square brackets
    "{API_KEY_1}": "sk-REAL", // single braces
    "[[API_KEY_1]]": "sk-REAL",
    "{{api_key_1}}": "sk-REAL", // lower case, braces present
    "{{Api Key 1}}": "sk-REAL", // spaces instead of underscores
    "{{APIKEY_1}}": "sk-REAL", // dropped separator
    "{{API_KEY1}}": "sk-REAL", // dropped _ before the index
    "{{API_KEY_1": "sk-REAL", // closing bracket lost
    "API_KEY_1}}": "sk-REAL", // opening bracket lost
    "API_KEY_1": "sk-REAL", // brackets dropped entirely (upper case)
    "API KEY 1": "sk-REAL", // brackets dropped, spaces
  };
  for (const [mangled, want] of Object.entries(cases)) assert.equal(restore(`x ${mangled} y`), `x ${want} y`, mangled);
});

test("only known keys are touched: HTTP_2, SHA_256, unknown kinds and numbers stay", () => {
  for (const t of ["HTTP_2 is fine", "SHA_256 digest", "{{ name }} template", "{{EMAIL_9}}", "{{TOKEN_1}}", "PERSON_3", "EMAIL_0"]) {
    assert.equal(restore(t), t);
  }
});

test("bare lower-case and identifier-embedded forms are NOT restored", () => {
  for (const t of ["api_key_1 = 5", "MY_API_KEY_1", "API_KEY_1abc", "xAPI_KEY_1", "GMAILEMAIL_1", "EMAIL_1x"]) assert.equal(restore(t), t, t);
});

test("EMAIL_12 and EMAIL_1 do not collide", () => {
  assert.equal(restore("{{EMAIL_12}} and {{EMAIL_1}}"), "twelve@b.io and a@b.io");
  assert.equal(restore("EMAIL 12, EMAIL_1."), "twelve@b.io, a@b.io.");
});

test("values with $ & special characters are inserted literally", () => {
  const m = new VariantMatcher(["API_KEY_1"]);
  const r = replaceVariants("k={{API_KEY_1}}", m, () => "a$&b$1\\n");
  assert.equal(r.text, "k=a$&b$1\\n");
  assert.deepEqual(r.ranges, [{ start: 2, end: 2 + "a$&b$1\\n".length, key: "API_KEY_1" }]);
});

test("placeholders without a value are left exactly as written", () => {
  const r = replaceVariants("{{API_KEY_1}} {{EMAIL_1}}", M, (k) => (k === "EMAIL_1" ? "x@y.z" : undefined));
  assert.equal(r.text, "{{API_KEY_1}} x@y.z");
  assert.equal(r.count, 1);
});

test("an empty matcher finds nothing and costs nothing", () => {
  const m = new VariantMatcher([]);
  assert.equal(m.size, 0);
  assert.deepEqual(m.find("{{API_KEY_1}}"), []);
});

test("parseKey follows the engine's format", () => {
  assert.deepEqual(parseKey("API_KEY_12"), { kind: "API_KEY", n: 12 });
  assert.deepEqual(parseKey("X_1"), { kind: "X", n: 1 });
  for (const bad of ["api_key_1", "API_KEY_0", "API_KEY_01", "API_KEY", "_1", "1_1", "{{API_KEY_1}}"]) assert.equal(parseKey(bad), null, bad);
});

test("the note is one line, mentions every placeholder, and strips cleanly", () => {
  const note = buildNote(["API_KEY_1", "EMAIL_1", "API_KEY_1"]);
  assert.ok(!note.includes("\n"));
  assert.ok(note.includes("{{API_KEY_1}}") && note.includes("{{EMAIL_1}}"));
  assert.ok(note.startsWith("[Zuko privacy note:") && note.endsWith("]"));
  assert.equal(note.slice(1, -1).includes("]"), false);
  const text = withNote("hello {{API_KEY_1}}", ["API_KEY_1"]);
  assert.ok(hasNote(text));
  assert.equal(stripNote(text), "hello {{API_KEY_1}}");
  // idempotent: re-noting replaces the old note instead of stacking
  assert.equal(withNote(text, ["API_KEY_1"]).split("[Zuko privacy note:").length, 2);
  assert.equal(withNote("plain", []), "plain");
});

test("canonicalKeys lists well-formed placeholders only", () => {
  assert.deepEqual(canonicalKeys("a {{API_KEY_1}} b {{ EMAIL_2 }} c {{nope}} {{X_0}}"), ["API_KEY_1", "EMAIL_2"]);
});
