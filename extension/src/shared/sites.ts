// Which sites Zuko protects and which of their endpoints carry prompts or uploads.
// Shared by the MAIN-world net guard (small, no DOM), the content script and the service
// worker. Paths are matched with regexes and anything unknown still goes through the
// tripwire, so an endpoint that moves degrades to "vault values only", not to nothing.

export type SiteId = "chatgpt" | "claude" | "deepseek";

export const SITE_IDS: readonly SiteId[] = ["chatgpt", "claude", "deepseek"];

export const SITE_LABELS: Record<SiteId, string> = {
  chatgpt: "ChatGPT",
  claude: "Claude",
  deepseek: "DeepSeek",
};

const HOSTS: Record<string, SiteId> = {
  "chatgpt.com": "chatgpt",
  "chat.openai.com": "chatgpt",
  "claude.ai": "claude",
  "chat.deepseek.com": "deepseek",
};

/** The site for a hostname, or null if Zuko does not run there. */
export function siteForHost(host: string): SiteId | null {
  return HOSTS[host.toLowerCase()] ?? null;
}

export interface Endpoints {
  /** Requests whose JSON body holds the user's prompt (rewritten with full detection). */
  prompt: RegExp[];
  /** Requests that upload a file (text-like bodies get full detection). */
  upload: Array<(url: URL, method: string) => boolean>;
}

export const ENDPOINTS: Record<SiteId, Endpoints> = {
  chatgpt: {
    // /backend-api/conversation, /backend-api/f/conversation, /backend-anon/f/conversation,
    // and the .../prepare call that runs first.
    prompt: [/^\/(?:backend-api|backend-anon)\/(?:f\/)?conversation(?:\/prepare)?\/?$/],
    upload: [
      (u, m) => m !== "GET" && /^\/backend-api\/files(?:\/|$)/.test(u.pathname),
      // Blob storage PUT after /backend-api/files hands out an upload URL.
      (u, m) => m === "PUT" && /(?:blob\.core\.windows\.net|oaiusercontent\.com|openaiusercontent\.com)$/.test(u.hostname),
    ],
  },
  claude: {
    prompt: [/^\/api\/organizations\/[^/]+\/chat_conversations\/[^/]+\/(?:completion|retry_completion)\/?$/],
    upload: [
      (u, m) => m !== "GET" && /^\/api\/(?:organizations\/[^/]+\/)?(?:upload|convert_document)\/?$/.test(u.pathname),
      (u, m) => m !== "GET" && /^\/api\/organizations\/[^/]+\/(?:upload|files|convert_document)(?:\/|$)/.test(u.pathname),
    ],
  },
  deepseek: {
    prompt: [/^\/api\/v0\/chat\/(?:completion|edit_message)\/?$/],
    upload: [(u, m) => m !== "GET" && /^\/api\/v0\/file\/upload_file\/?$/.test(u.pathname)],
  },
};

export function isPromptEndpoint(site: SiteId, url: URL, method: string): boolean {
  return method !== "GET" && method !== "HEAD" && ENDPOINTS[site].prompt.some((re) => re.test(url.pathname));
}

export function isUploadEndpoint(site: SiteId, url: URL, method: string): boolean {
  return ENDPOINTS[site].upload.some((f) => f(url, method.toUpperCase()));
}
