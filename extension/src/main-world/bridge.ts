// Page-side end of the MessageChannel to the content script.
//
// Handshake (both scripts run at document_start, before any page script):
//  * the net guard listens for ONE `message` whose payload is `{__zuko:"port"}` and carries a
//    port, takes it, stops the event so nobody after us sees it, and stops listening;
//  * then it announces itself with a `zuko:main-ready` event. The content script answers by
//    posting the port (it also posts once at its own start, which covers the other load order).
// The content script hands out a port at most until the guard says hello over it, so a page
// script that fires `zuko:main-ready` later gets nothing.

import type { BridgeReply, BridgeRequest, BridgeState } from "../shared/protocol.ts";

export const PORT_TAG = "port";
export const READY_EVENT = "zuko:main-ready";

export class BridgeUnavailable extends Error {
  constructor(message = "bridge unavailable") {
    super(message);
  }
}

export interface PageBridge {
  state: BridgeState & { known: boolean };
  /** Request/response. Rejects with BridgeUnavailable on timeout or when the other side reports an engine error. */
  request<T extends BridgeReply = BridgeReply>(msg: DistributiveOmit<Extract<BridgeRequest, { id: number }>, "id">, timeoutMs?: number): Promise<T>;
  /** Fire and forget. */
  post(msg: Extract<BridgeRequest, { type: "notify" | "event" }>): boolean;
  connected(): boolean;
  /** Called when state changes. */
  onState(cb: () => void): void;
}

type DistributiveOmit<T, K extends keyof any> = T extends unknown ? Omit<T, K> : never;

const DEFAULT_TIMEOUT_MS = 8000;

export function createPageBridge(win: Window & typeof globalThis): PageBridge {
  let port: MessagePort | null = null;
  let nextId = 1;
  const pending = new Map<number, { resolve: (r: any) => void; reject: (e: Error) => void; timer: ReturnType<typeof setTimeout> }>();
  const waiting: Array<() => void> = [];
  const stateListeners: Array<() => void> = [];

  const bridge: PageBridge = {
    // Until the first state arrives we assume protection is on and the vault may be non-empty.
    state: { type: "state", enabled: true, vaultSize: 0, engine: true, known: false },
    request(msg, timeoutMs = DEFAULT_TIMEOUT_MS) {
      return new Promise((resolve, reject) => {
        const id = nextId++;
        const timer = setTimeout(() => {
          pending.delete(id);
          reject(new BridgeUnavailable("timed out"));
        }, timeoutMs);
        pending.set(id, { resolve, reject, timer });
        const send = () => port?.postMessage({ ...msg, id });
        if (port) send();
        else waiting.push(send);
      });
    },
    post(msg) {
      if (!port) return false;
      port.postMessage(msg);
      return true;
    },
    connected: () => port !== null,
    onState: (cb) => void stateListeners.push(cb),
  };

  const onPortMessage = (e: MessageEvent) => {
    const m = e.data as (BridgeReply & { type?: string }) | BridgeState | null;
    if (!m || typeof m !== "object") return;
    if ((m as BridgeState).type === "state") {
      const s = m as BridgeState;
      bridge.state = { ...s, known: true };
      stateListeners.forEach((cb) => cb());
      return;
    }
    const reply = m as BridgeReply;
    const p = pending.get(reply.id);
    if (!p) return;
    clearTimeout(p.timer);
    pending.delete(reply.id);
    if (reply.ok) p.resolve(reply);
    else p.reject(new BridgeUnavailable(String(reply.error ?? "request failed")));
  };

  const onWindowMessage = (e: MessageEvent) => {
    const d = e.data as { __zuko?: string } | null;
    if (port || e.source !== win || !d || typeof d !== "object" || d.__zuko !== PORT_TAG || !e.ports[0]) return;
    e.stopImmediatePropagation();
    port = e.ports[0];
    port.onmessage = onPortMessage;
    win.removeEventListener("message", onWindowMessage, true);
    port.postMessage({ type: "hello" });
    waiting.splice(0).forEach((send) => send());
  };

  win.addEventListener("message", onWindowMessage, true);
  win.dispatchEvent(new win.CustomEvent(READY_EVENT));
  return bridge;
}
