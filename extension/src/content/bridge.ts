// Isolated-world end of the MessageChannel to the page's net guard (see main-world/bridge.ts
// for the handshake). Requests from the page are forwarded to handlers (which talk to the
// service worker); replies carry only masked text, never real values.

import type { BridgeReply, BridgeRequest, BridgeState } from "../shared/protocol.ts";

export const PORT_TAG = "port";
export const READY_EVENT = "zuko:main-ready";

export interface BridgeHandlers {
  request(msg: Extract<BridgeRequest, { id: number }>): Promise<Record<string, unknown>>;
  notify(level: "info" | "warn" | "error", text: string): void;
  event(kind: "masked" | "blocked" | "upload", count: number, keys: string[]): void;
}

export class ContentBridge {
  private readonly win: Window & typeof globalThis;
  private readonly handlers: BridgeHandlers;
  private port: MessagePort | null = null;
  private offered: MessagePort | null = null;
  private state: BridgeState = { type: "state", enabled: true, vaultSize: 0, engine: true };

  constructor(win: Window & typeof globalThis, handlers: BridgeHandlers) {
    this.win = win;
    this.handlers = handlers;
  }

  /** Offers a port to the page's net guard (whichever script loaded first). */
  start(): void {
    this.win.addEventListener(READY_EVENT, () => {
      // Only until the guard has said hello: after that a page script firing the event gets nothing.
      if (!this.port) this.offer();
    });
    this.offer();
  }

  pushState(state: Omit<BridgeState, "type">): void {
    this.state = { type: "state", ...state };
    this.port?.postMessage(this.state);
  }

  private offer(): void {
    this.offered?.close();
    const ch = new this.win.MessageChannel();
    ch.port1.onmessage = (e) => this.onMessage(ch.port1, e);
    this.offered = ch.port1;
    this.win.postMessage({ __zuko: PORT_TAG }, "*", [ch.port2]);
  }

  private onMessage(from: MessagePort, e: MessageEvent): void {
    const m = e.data as { type?: string; id?: number } | null;
    if (!m || typeof m !== "object") return;
    if (!this.port) {
      if (m.type !== "hello") return;
      this.port = from;
      this.offered = null;
      this.port.postMessage(this.state);
      return;
    }
    if (from !== this.port) return;
    const req = m as BridgeRequest;
    switch (req.type) {
      case "notify":
        if (typeof req.text === "string") this.handlers.notify(req.level === "error" || req.level === "warn" ? req.level : "info", req.text.slice(0, 600));
        return;
      case "event":
        this.handlers.event(req.kind, Number(req.count) || 0, Array.isArray(req.keys) ? req.keys.map(String) : []);
        return;
      case "maskMany":
      case "tripwire":
      case "clipboard": {
        const id = req.id;
        this.handlers
          .request(req)
          .then(
            (r): BridgeReply => ({ id, ok: true, ...r }),
            (err): BridgeReply => ({ id, ok: false, error: err instanceof Error ? err.message : String(err) }),
          )
          .then((reply) => this.port?.postMessage(reply));
        return;
      }
    }
  }
}
