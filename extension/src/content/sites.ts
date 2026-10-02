// Per-site DOM selectors, kept in one table with generic fallbacks. Sites ship two DOM
// contracts at once and rename classes monthly (research: ChatGPT serves a ProseMirror build
// and a plain-textarea build side by side), so every list holds several candidates and the
// generic list is always tried last. A miss only costs display polish (chip, restored text):
// protection sits in the network layer and does not read any of these.

import type { SiteId } from "../shared/sites.ts";

export interface SelectorConfig {
  /** Where the user types. */
  composer: string[];
  /** Containers of chat messages (user and assistant) that may show placeholders. */
  messages: string[];
  /** The user's own messages (the privacy note is hidden there). */
  userMessages: string[];
}

export const GENERIC: SelectorConfig = {
  composer: ["textarea", '[contenteditable="true"][role="textbox"]', '[contenteditable="true"]', '[contenteditable="plaintext-only"]'],
  messages: ["[data-message-author-role]", "article", '[role="article"]'],
  userMessages: ['[data-message-author-role="user"]'],
};

export const SELECTORS: Record<SiteId, SelectorConfig> = {
  chatgpt: {
    composer: [
      "#prompt-textarea",
      "form div.ProseMirror[contenteditable=true]",
      "div.ProseMirror[data-composer-markdown]",
      "[data-mobile-composer-prompt]",
      "form textarea[name=prompt]",
    ],
    messages: [
      "[data-message-author-role]",
      "li[data-message-role]",
      "[data-turn-key]",
      '[data-testid^="conversation-turn"]',
      "h4[data-conversation-role]",
    ],
    userMessages: ['[data-message-author-role="user"]', 'li[data-message-role="user"]', 'h4[data-conversation-role="user"]'],
  },
  claude: {
    composer: ['fieldset [contenteditable="true"]', '[contenteditable="true"][role="textbox"]', ".ProseMirror[contenteditable=true]"],
    messages: ['[data-testid="user-message"]', ".font-claude-message", ".font-claude-response", "[data-is-streaming]"],
    userMessages: ['[data-testid="user-message"]'],
  },
  deepseek: {
    composer: ["textarea#chat-input", ".ds-textarea textarea", "textarea"],
    messages: [".ds-markdown", ".ds-message", '[class*="message"]'],
    userMessages: [".ds-message:not(:has(.ds-markdown))", '[class*="user-message"]'],
  },
};

function selectorsFor(site: SiteId, key: keyof SelectorConfig): string[] {
  return [...SELECTORS[site][key], ...GENERIC[key]];
}

/** querySelector over a list that may contain selectors the browser rejects. */
function safeQuery(root: ParentNode, selector: string): Element | null {
  try {
    return root.querySelector(selector);
  } catch {
    return null;
  }
}

function safeQueryAll(root: ParentNode, selector: string): Element[] {
  try {
    return [...root.querySelectorAll(selector)];
  } catch {
    return [];
  }
}

export function isEditable(el: Element | null): el is HTMLElement {
  if (!el) return false;
  const tag = el.tagName;
  if (tag === "TEXTAREA") return true;
  if (tag === "INPUT") return /^(text|search|url|email|tel)?$/.test((el as HTMLInputElement).type);
  const ce = (el as HTMLElement).getAttribute?.("contenteditable");
  return ce === "" || ce === "true" || ce === "plaintext-only";
}

/** The composer the event came from, or the first one on the page. */
export function findComposer(doc: Document, site: SiteId, from?: EventTarget | null): HTMLElement | null {
  const sels = selectorsFor(site, "composer");
  if (from instanceof (doc.defaultView?.Element ?? Element)) {
    const el = from as Element;
    for (const s of sels) {
      try {
        const hit = el.closest(s);
        if (hit && isEditable(hit)) return hit;
      } catch {
        /* skip invalid selector */
      }
    }
    return null;
  }
  for (const s of sels) {
    const el = safeQuery(doc, s);
    if (el && isEditable(el)) return el;
  }
  return null;
}

export function composerText(el: HTMLElement): string {
  if (el.tagName === "TEXTAREA" || el.tagName === "INPUT") return (el as HTMLTextAreaElement).value;
  return el.innerText ?? el.textContent ?? "";
}

/** Message containers inside `root` (or the roots themselves when `root` is a message). */
export function messageContainers(doc: Document, site: SiteId, root: ParentNode): Element[] {
  const sels = selectorsFor(site, "messages");
  const found = new Set<Element>();
  for (const s of sels) for (const el of safeQueryAll(root, s)) found.add(el);
  if (root instanceof (doc.defaultView?.Element ?? Element)) {
    for (const s of sels) {
      try {
        const up = (root as Element).closest(s);
        if (up) found.add(up);
      } catch {
        /* skip */
      }
    }
  }
  return [...found];
}

export function isUserMessage(site: SiteId, el: Element): boolean {
  return selectorsFor(site, "userMessages").some((s) => {
    try {
      return el.matches(s) || el.closest(s) !== null;
    } catch {
      return false;
    }
  });
}

/**
 * Where to look for text to restore: the message containers when the selectors still match,
 * else the main area of the page (editable regions are always skipped by the walker).
 */
export function rehydrationRoots(doc: Document, site: SiteId, dirty: ParentNode): Node[] {
  const msgs = messageContainers(doc, site, dirty);
  if (msgs.length > 0) return msgs;
  const anyMessage = selectorsFor(site, "messages").some((s) => safeQuery(doc, s) !== null);
  if (anyMessage) return []; // messages exist but the dirty node is outside them (sidebar, toolbar)
  return [safeQuery(doc, "main") ?? safeQuery(doc, '[role="main"]') ?? doc.body].filter(Boolean) as Node[];
}
