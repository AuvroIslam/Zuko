// Toolbar popup: is the engine loaded, is the desktop app linked, per-site switches and what
// Zuko did this session. Reads everything from the service worker; shows no vault values.

import { SITE_IDS, SITE_LABELS, type SiteId } from "../shared/sites.ts";

interface Status {
  ok: boolean;
  version: string;
  engine: boolean;
  engineError: string | null;
  linked: boolean;
  appVersion: string | null;
  linkError: string | null;
  vaultSize: number;
  prefs: { sites: Record<SiteId, boolean> };
  total: { masked: number; blocked: number; uploads: number; restored: number };
}

const SITE_HOST: Record<SiteId, string> = {
  chatgpt: "chatgpt.com, chat.openai.com",
  claude: "claude.ai",
  deepseek: "chat.deepseek.com",
};

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;

async function ask<T = any>(msg: Record<string, unknown>): Promise<T | null> {
  try {
    return (await chrome.runtime.sendMessage(msg)) as T;
  } catch {
    return null;
  }
}

function setStatus(dot: string, value: string, level: "ok" | "warn" | "bad", text: string): void {
  $(dot).className = `dot ${level}`;
  $(value).textContent = text;
}

function renderSites(prefs: Status["prefs"]): void {
  const host = $("sites");
  if (host.childElementCount === SITE_IDS.length) {
    for (const id of SITE_IDS) (document.getElementById(`sw-${id}`) as HTMLInputElement).checked = prefs.sites[id] !== false;
    return;
  }
  for (const id of SITE_IDS) {
    const label = document.createElement("label");
    label.className = "site";
    const name = document.createElement("span");
    name.className = "name";
    name.textContent = SITE_LABELS[id];
    const small = document.createElement("span");
    small.className = "host";
    small.textContent = SITE_HOST[id];
    name.appendChild(small);
    const sw = document.createElement("span");
    sw.className = "switch";
    const input = document.createElement("input");
    input.type = "checkbox";
    input.id = `sw-${id}`;
    input.checked = prefs.sites[id] !== false;
    input.setAttribute("aria-label", `Protect ${SITE_LABELS[id]}`);
    input.addEventListener("change", () => void ask({ type: "setSite", site: id, enabled: input.checked }));
    const track = document.createElement("span");
    track.className = "track";
    const thumb = document.createElement("span");
    thumb.className = "thumb";
    sw.append(input, track, thumb);
    label.append(name, sw);
    host.appendChild(label);
  }
}

function render(s: Status): void {
  $("version").textContent = `v${s.version} · privacy guard for AI chats`;

  if (s.engine) setStatus("dot-engine", "val-engine", "ok", "loaded");
  else setStatus("dot-engine", "val-engine", "bad", "not loaded");

  const relink = $("btn-relink");
  if (s.linked) {
    setStatus("dot-desktop", "val-desktop", "ok", s.appVersion ? `linked (v${s.appVersion})` : "linked");
    relink.hidden = true;
  } else {
    setStatus("dot-desktop", "val-desktop", "warn", "not linked");
    relink.hidden = false;
  }

  const hint = $("hint");
  if (!s.engine) {
    hint.hidden = false;
    hint.textContent = `The privacy engine is missing, so Zuko can only block obvious secrets. ${s.engineError ?? ""} Build zuko_core.wasm and rebuild the extension (see the README).`.trim();
  } else if (!s.linked) {
    hint.hidden = false;
    hint.textContent =
      "Zuko works on its own. Link the desktop app to share one vault with Claude Code" +
      (s.linkError ? ` (${s.linkError.replace(/\.$/, "")}).` : ".");
  } else {
    hint.hidden = true;
  }

  renderSites(s.prefs);
  $("n-masked").textContent = String(s.total.masked);
  $("n-blocked").textContent = String(s.total.blocked);
  $("n-uploads").textContent = String(s.total.uploads);
  $("n-restored").textContent = String(s.total.restored);
  $("n-vault").textContent = String(s.vaultSize);
}

async function refresh(): Promise<void> {
  const s = await ask<Status>({ type: "status" });
  if (s?.ok) render(s);
  else {
    setStatus("dot-engine", "val-engine", "bad", "no answer");
    setStatus("dot-desktop", "val-desktop", "warn", "unknown");
  }
}

$("btn-relink").addEventListener("click", async () => {
  $("val-desktop").textContent = "connecting...";
  await ask({ type: "relink" });
  await refresh();
});

$("btn-clear").addEventListener("click", async () => {
  if (!confirm("Forget every value Zuko masked in this browser session? Placeholders already in your chats will stop being restored.")) return;
  await ask({ type: "clearVault" });
  await refresh();
});

void refresh();
setInterval(() => void refresh(), 2000);
