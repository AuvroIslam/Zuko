// Minimal ambient typings for the extension APIs Zuko uses (no @types/chrome dependency).
// Intentionally loose: only what the code calls, so a typo in a method name still fails.

declare namespace chrome {
  namespace runtime {
    const id: string;
    const lastError: { message?: string } | undefined;
    interface MessageSender {
      id?: string;
      url?: string;
      origin?: string;
      frameId?: number;
      tab?: { id?: number; url?: string };
    }
    function getURL(path: string): string;
    function sendMessage(message: unknown): Promise<any>;
    function connectNative(application: string): Port;
    function getContexts(filter: { contextTypes: string[] }): Promise<unknown[]>;
    interface Port {
      name: string;
      postMessage(message: unknown): void;
      disconnect(): void;
      onMessage: { addListener(cb: (message: any) => void): void };
      onDisconnect: { addListener(cb: () => void): void };
    }
    const onMessage: {
      addListener(
        cb: (message: any, sender: MessageSender, sendResponse: (response?: unknown) => void) => boolean | void,
      ): void;
    };
    const onInstalled: { addListener(cb: (details: { reason: string }) => void): void };
    const onStartup: { addListener(cb: () => void): void };
    function getManifest(): { version: string };
  }
  namespace storage {
    interface Area {
      get(keys?: string | string[] | null): Promise<Record<string, any>>;
      set(items: Record<string, unknown>): Promise<void>;
      remove(keys: string | string[]): Promise<void>;
    }
    const session: Area;
    const local: Area;
    const onChanged: {
      addListener(cb: (changes: Record<string, { oldValue?: any; newValue?: any }>, area: string) => void): void;
    };
  }
  namespace alarms {
    interface Alarm {
      name: string;
      scheduledTime: number;
      periodInMinutes?: number;
    }
    function create(name: string, info: { periodInMinutes?: number; delayInMinutes?: number }): Promise<void>;
    function get(name: string): Promise<Alarm | undefined>;
    const onAlarm: { addListener(cb: (alarm: Alarm) => void): void };
  }
  namespace offscreen {
    function createDocument(params: { url: string; reasons: string[]; justification: string }): Promise<void>;
  }
  namespace tabs {
    function sendMessage(tabId: number, message: unknown, options?: { frameId?: number }): Promise<any>;
    function query(info: { active?: boolean; currentWindow?: boolean }): Promise<Array<{ id?: number; url?: string }>>;
  }
}
