// The real zuko-native-host.exe, driven with Chrome's framing over stdio, against a fake app
// on the real named pipe `\\.\pipe\zuko-<SID>`. Windows only; skipped when the host binary is
// not built (`cargo build --release -p zuko-native-host` in app/) or when a real Zuko app is
// already serving the pipe (the test never talks to it).

import assert from "node:assert/strict";
import { execFileSync, spawn } from "node:child_process";
import { existsSync, readdirSync } from "node:fs";
import net from "node:net";
import { join } from "node:path";
import { test } from "node:test";
import { repoRoot } from "./helpers.mjs";

const exe = join(repoRoot, "app", "target", "release", "zuko-native-host.exe");

function sid() {
  const out = execFileSync(join(process.env.SystemRoot ?? "C:\\Windows", "System32", "whoami.exe"), ["/user", "/fo", "csv", "/nh"], { encoding: "utf8" });
  return out.trim().split(",")[1].replaceAll('"', "");
}

function skipReason() {
  if (process.platform !== "win32") return "Windows-only (named pipe)";
  if (!existsSync(exe)) return "host not built: run `cargo build --release -p zuko-native-host` in app/";
  try {
    if (readdirSync("\\\\.\\pipe\\").includes(`zuko-${sid()}`)) return "a real Zuko app is serving the pipe";
  } catch {
    return "cannot enumerate pipes";
  }
  return null;
}

const frame = (obj) => {
  const body = Buffer.from(typeof obj === "string" ? obj : JSON.stringify(obj));
  const head = Buffer.alloc(4);
  head.writeUInt32LE(body.length);
  return Buffer.concat([head, body]);
};

/** Spawns the host; returns { send(obj), next() -> parsed frame, close() -> exit code }. */
function host() {
  const child = spawn(exe, ["chrome-extension://abcdefghijklmnopabcdefghijklmnop/"], { stdio: ["pipe", "pipe", "inherit"] });
  let buf = Buffer.alloc(0);
  const waiting = [];
  child.stdout.on("data", (d) => {
    buf = Buffer.concat([buf, d]);
    pump();
  });
  function pump() {
    while (buf.length >= 4 && waiting.length) {
      const len = buf.readUInt32LE(0);
      if (buf.length < 4 + len) return;
      const body = buf.subarray(4, 4 + len);
      buf = buf.subarray(4 + len);
      waiting.shift()({ raw: body, json: () => JSON.parse(body.toString()), length: len });
    }
  }
  const exited = new Promise((r) => child.on("exit", (code) => r(code)));
  return {
    send: (obj) => child.stdin.write(frame(obj)),
    next: () => new Promise((r) => (waiting.push(r), pump())),
    async close() {
      child.stdin.end();
      return exited;
    },
    kill: () => child.kill(),
  };
}

/** A fake app: handler(line) -> reply line (or null to hang up without answering). */
function fakeApp(handler) {
  const received = [];
  const server = net.createServer((sock) => {
    let data = "";
    sock.on("data", (d) => {
      data += d;
      const nl = data.indexOf("\n");
      if (nl < 0) return;
      const line = data.slice(0, nl);
      received.push(line);
      const reply = handler(line);
      if (reply === null) sock.end();
      else sock.end(reply + "\n");
    });
    sock.on("error", () => {});
  });
  return new Promise((resolve) => server.listen(`\\\\.\\pipe\\zuko-${sid()}`, () => resolve({ received, close: () => new Promise((r) => server.close(r)) })));
}

const reason = skipReason();

test("app not running: every frame still gets exactly one error frame, in order", { skip: reason ?? false }, async () => {
  const h = host();
  h.send({ op: "hello", id: 1 });
  h.send({ op: "vault", id: 2 });
  assert.deepEqual((await h.next()).json(), { ok: false, error: "Zuko desktop app is not running", id: 1 });
  assert.deepEqual((await h.next()).json(), { ok: false, error: "Zuko desktop app is not running", id: 2 });
  assert.equal(await h.close(), 0, "exits cleanly when the browser closes stdin");
});

test("with an app: wraps as ZukoExtension, unwraps {reply}, echoes ids, handles big requests and replies", { skip: reason ?? false }, async () => {
  const app = await fakeApp((line) => {
    const req = JSON.parse(line);
    switch (req.message.op) {
      case "hello":
        return JSON.stringify({ reply: { ok: true, app: "zuko", version: "9.9.9" } });
      case "mask":
        return JSON.stringify({ reply: { ok: true, received: req.message.text.length } });
      case "vault":
        return JSON.stringify({ reply: { ok: true, vault: "v".repeat(1024 * 1024 + 10) } }); // over the 1 MB cap
      case "silent":
        return null;
      default:
        return JSON.stringify({ stdout: null });
    }
  });
  const h = host();
  try {
    h.send({ op: "hello", version: "0.1.1", id: 1 });
    assert.deepEqual((await h.next()).json(), { ok: true, app: "zuko", version: "9.9.9", id: 1 });
    const sent = JSON.parse(app.received[0]);
    assert.equal(sent.hook_event_name, "ZukoExtension");
    assert.deepEqual(sent.message, { op: "hello", version: "0.1.1", id: 1 });

    // 3 MB browser -> host is fine (no practical cap in that direction); the reply is small.
    const big = "a".repeat(3 * 1024 * 1024);
    h.send({ op: "mask", text: big, id: 2 });
    const r = (await h.next()).json();
    assert.equal(r.ok, true, JSON.stringify(r));
    assert.equal(r.received, big.length, "the whole 3 MB request reached the app");
    assert.equal(r.id, 2);

    // A reply over 1 MB is replaced by an error frame, and the host carries on.
    h.send({ op: "vault", id: 3 });
    const tooBig = await h.next();
    assert.ok(tooBig.length < 1024 * 1024);
    assert.equal(tooBig.json().ok, false);
    assert.match(tooBig.json().error, /1 MB/);
    assert.equal(tooBig.json().id, 3);

    // An app that hangs up without answering, and one that answers in the wrong shape.
    h.send({ op: "silent", id: 4 });
    const silent = (await h.next()).json();
    assert.equal(silent.ok, false);
    assert.equal(silent.id, 4);
    h.send({ op: "other", id: 5 });
    assert.equal((await h.next()).json().ok, false);

    // Garbage JSON from the browser is answered, not fatal.
    h.send("{not json");
    assert.equal((await h.next()).json().ok, false);
    h.send({ op: "hello", id: 6 });
    assert.equal((await h.next()).json().id, 6);
  } finally {
    await h.close();
    await app.close();
  }
});
