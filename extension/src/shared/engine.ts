// Client for the zuko-core WASM C-ABI (app/core/src/wasm.rs).
//
// Protocol: JSON in, JSON out through linear memory. `zuko_alloc(len)` hands out a buffer,
// `zuko_call(ptr, len)` takes a UTF-8 JSON request and returns `(out_ptr << 32) | out_len`,
// and both the request and the reply buffers are released with `zuko_free(ptr, len)`.
// Runs in the service worker and in Node (tests); never in the page.

export interface Finding {
  kind: string;
  rule: string;
  value: string;
  start?: number;
  end?: number;
  label?: string;
  hint?: string | null;
  [k: string]: unknown;
}

export interface MaskReport {
  count: number;
  keys: string[];
  newKeys: string[];
}

export interface EntryView {
  key: string;
  kind: string;
  label: string;
  hint: string | null;
  preview: string;
  source: string;
  hits: number;
  [k: string]: unknown;
}

export interface VaultEntry {
  key: string;
  value: string;
  kind: string;
  label: string;
  category?: unknown;
  hint?: string | null;
  source?: string;
  created?: number;
  lastUsed?: number;
  hits?: number;
  [k: string]: unknown;
}

export interface VaultJson {
  entries: VaultEntry[];
  counters: Record<string, number>;
}

/** The one capability everything else builds on: send a request object, get a reply. */
export interface EngineTransport {
  call(request: Record<string, unknown>): any;
}

export class EngineError extends Error {}

interface ZukoExports {
  memory: WebAssembly.Memory;
  zuko_alloc(len: number): number;
  zuko_free(ptr: number, len: number): void;
  zuko_call(ptr: number, len: number): bigint;
}

const enc = new TextEncoder();
const dec = new TextDecoder("utf-8", { fatal: true });

/** The real engine: zuko_core.wasm instantiated with an empty import object. */
export class WasmEngine implements EngineTransport {
  private readonly x: ZukoExports;

  private constructor(x: ZukoExports) {
    this.x = x;
  }

  static async load(source: BufferSource | WebAssembly.Module | Promise<Response>): Promise<WasmEngine> {
    let instance: WebAssembly.Instance;
    if (source instanceof WebAssembly.Module) {
      instance = await WebAssembly.instantiate(source, {});
    } else if (source instanceof Promise) {
      instance = (await WebAssembly.instantiateStreaming(source, {})).instance;
    } else {
      instance = (await WebAssembly.instantiate(source, {})).instance;
    }
    const x = instance.exports as unknown as ZukoExports;
    for (const name of ["memory", "zuko_alloc", "zuko_free", "zuko_call"] as const) {
      if (!(name in x)) throw new EngineError(`zuko_core.wasm lacks the "${name}" export`);
    }
    return new WasmEngine(x);
  }

  call(request: Record<string, unknown>): any {
    const input = enc.encode(JSON.stringify(request));
    const { memory, zuko_alloc, zuko_free, zuko_call } = this.x;
    const ptr = zuko_alloc(input.length);
    new Uint8Array(memory.buffer, ptr, input.length).set(input); // read memory.buffer after alloc: it may have grown
    const packed = zuko_call(ptr, input.length);
    zuko_free(ptr, input.length);
    const outPtr = Number(packed >> 32n);
    const outLen = Number(packed & 0xffffffffn);
    const text = dec.decode(new Uint8Array(memory.buffer, outPtr, outLen).slice());
    zuko_free(outPtr, outLen);
    return JSON.parse(text);
  }
}

/** Typed helpers over any transport (the real engine or a test double). */
export class Engine {
  readonly transport: EngineTransport;

  constructor(transport: EngineTransport) {
    this.transport = transport;
  }

  private ok(req: Record<string, unknown>): any {
    const r = this.transport.call(req);
    if (!r || r.ok !== true) throw new EngineError(String(r?.error ?? `engine op ${String(req.op)} failed`));
    return r;
  }

  configure(detector: Record<string, unknown> | null): void {
    this.ok(detector ? { op: "configure", detector } : { op: "configure" });
  }
  loadVault(vault: VaultJson | null): number {
    return this.ok({ op: "loadVault", vault }).size as number;
  }
  exportVault(): VaultJson {
    const v = this.ok({ op: "exportVault" }).vault as Partial<VaultJson>;
    return { entries: v.entries ?? [], counters: v.counters ?? {} };
  }
  scan(text: string): Finding[] {
    return this.ok({ op: "scan", text }).findings as Finding[];
  }
  mask(text: string, source = "browser", now = Math.floor(Date.now() / 1000)): { text: string; report: MaskReport } {
    const r = this.ok({ op: "mask", text, source, now });
    return { text: r.text, report: r.report };
  }
  maskKnown(text: string): { text: string; count: number } {
    const r = this.ok({ op: "maskKnown", text });
    return { text: r.text, count: r.count };
  }
  rehydrate(text: string): { text: string; keys: string[] } {
    const r = this.ok({ op: "rehydrate", text });
    return { text: r.text, keys: r.keys };
  }
  legend(keys: string[]): string {
    return this.ok({ op: "legend", keys }).text as string;
  }
  views(): EntryView[] {
    return this.ok({ op: "views" }).entries as EntryView[];
  }
}
