// Chat: who answers the island chat, with which model, and with which key.
//
// Claude and OpenAI get masked text: secrets and personal data are replaced by
// placeholders before anything is sent and restored on this machine. Ollama runs on
// this computer, so nothing leaves it. Every line here says which is which.
//
// Keys go straight to the OS keyring through Rust: this page can only ask whether one
// is stored, never read it back. OpenAI's model list is fetched by Rust with the stored
// key, and only when OpenAI is the chosen provider (or on Refresh / after saving a key);
// Ollama's comes from the local AI endpoint, which Rust only accepts as loopback.

import { h, clear, copyText } from "../views/dom";
import { Bridge, type ChatModels, type ChatStatus } from "../core/bridge";
import { CHAT_MODEL_FIELD, CHAT_PROVIDER_LABEL, chatModel, type ChatProvider, type Settings } from "../core/state";
import { feedback, hint, segmented, setDot, statusDot, type DotState } from "./ui";

/** Read and write the settings owned by main.ts (it saves and tells the island). */
export interface SettingsStore {
  get(): Settings;
  update(patch: Partial<Settings>): void;
}

type CloudProvider = Exclude<ChatProvider, "ollama">;

const KEY_NAME: Record<CloudProvider, string> = {
  anthropic: "anthropic-api-key",
  openai: "openai-api-key",
};

const CLAUDE_MODELS: [string, string][] = [
  ["claude-opus-5", "Claude Opus 5"],
  ["claude-sonnet-5", "Claude Sonnet 5"],
  ["claude-haiku-4-5", "Claude Haiku 4.5"],
];

const CHOICES: [ChatProvider, string][] = [
  ["anthropic", "Claude"],
  ["openai", "OpenAI"],
  ["ollama", "Local (Ollama)"],
];

/** The honest line for each provider: where the words go. */
export const WHERE: Record<ChatProvider, string> = {
  anthropic: "Masked text is sent to Anthropic. Secrets and personal data become placeholders first and are restored on this PC.",
  openai: "Masked text is sent to OpenAI. Secrets and personal data become placeholders first and are restored on this PC.",
  ollama: "Stays on this PC: Ollama answers from this computer and nothing is sent anywhere. No key needed.",
};

/** One provider's block: a sub-heading with its own dot and an "In use" badge. */
function block(provider: ChatProvider, ...children: (Node | null)[]) {
  const dot = statusDot("off");
  const badge = h("span", { class: "badge green", text: "In use" });
  const el = h("div", { class: "chat-provider", "data-provider": provider },
    h("div", { class: "subhead" }, h("h3", {}, dot, h("span", { text: CHOICES.find(([p]) => p === provider)![1] })), badge),
    hint(WHERE[provider]),
    ...children);
  return {
    el,
    dot,
    setActive(on: boolean) {
      badge.style.display = on ? "" : "none";
      el.classList.toggle("active", on);
    },
  };
}

/** Password field + Save key + Remove for one cloud provider's key. */
function keyRow(provider: CloudProvider, onChange: (present: boolean) => void) {
  const fb = feedback();
  const field = h("input", {
    type: "password", style: "flex:1 1 auto;min-width:0", autocomplete: "off", spellcheck: "false",
    "aria-label": `${CHAT_PROVIDER_LABEL[provider]} API key`,
  }) as HTMLInputElement;
  const saveBtn = h("button", { class: "primary", text: "Save key" });
  const clearBtn = h("button", { class: "danger", text: "Remove" });
  const state = h("span", { class: "hint" });

  function paint(present: boolean) {
    field.placeholder = present ? "••••••••••••  (stored)" : provider === "anthropic" ? "sk-ant-..." : "sk-...";
    clearBtn.style.display = present ? "" : "none";
    state.textContent = present
      ? "Stored in the Windows Credential Manager, never on disk. Zuko never shows it again."
      : provider === "anthropic"
        ? "Only the island's chat needs one. Claude Code keeps using its own login."
        : "Your own OpenAI API key, from platform.openai.com.";
    onChange(present);
  }

  async function refresh() {
    paint((await Bridge.secretPresent(KEY_NAME[provider])) ?? false);
  }

  saveBtn.addEventListener("click", async () => {
    const value = field.value.trim();
    if (!value) return;
    try {
      await Bridge.secretSet(KEY_NAME[provider], value);
      field.value = "";
      fb.show("ok", "Saved.");
      await refresh();
    } catch (err) {
      fb.error(err, "Couldn't save");
    }
  });
  clearBtn.addEventListener("click", async () => {
    try {
      await Bridge.secretClear(KEY_NAME[provider]);
      fb.show("ok", "Key removed.");
      await refresh();
    } catch (err) {
      fb.error(err, "Couldn't remove");
    }
  });

  return {
    el: h("div", { class: "chat-key" },
      h("div", { class: "row" }, h("label", { text: "API key" }), field, saveBtn, clearBtn),
      state,
      fb.el),
    paint,
  };
}

/** A model dropdown: `models` (all offered), plus the saved one marked when missing. */
function fillSelect(select: HTMLSelectElement, models: string[], current: string, missingLabel: string, labels?: Map<string, string>) {
  clear(select);
  const all = models.includes(current) ? models : [current, ...models];
  for (const m of all) {
    const text = labels?.get(m) ?? m;
    select.append(h("option", { value: m, text: models.includes(m) || !models.length ? text : `${text} (${missingLabel})` }));
  }
  select.value = current;
}

export function chatSection(store: SettingsStore, initialKeys: Record<CloudProvider, boolean>, onProvider: (p: ChatProvider) => void): HTMLElement {
  const headDot = statusDot("off");
  const summary = h("span", { class: "hint" });
  const head = h("h2", {}, headDot, h("span", { text: "Chat" }));
  const el = h("section", { id: "chat" }, head);

  const keys = { ...initialKeys };
  const provider = () => store.get().chatProvider ?? "anthropic";
  const dots: Record<ChatProvider, DotState> = { anthropic: "off", openai: "off", ollama: "off" };

  // ── Claude ──────────────────────────────────────────────────────────────────
  const claudeModel = h("select", { "aria-label": "Claude model" }) as HTMLSelectElement;
  fillSelect(claudeModel, CLAUDE_MODELS.map(([id]) => id), chatModel(store.get(), "anthropic"), "custom", new Map(CLAUDE_MODELS));
  claudeModel.addEventListener("change", () => store.update({ [CHAT_MODEL_FIELD.anthropic]: claudeModel.value }));
  const claudeKey = keyRow("anthropic", (present) => {
    keys.anthropic = present;
    paintCloud("anthropic");
  });
  const claude = block("anthropic", claudeKey.el, h("div", { class: "row" }, h("label", { text: "Model" }), claudeModel));

  // ── OpenAI ──────────────────────────────────────────────────────────────────
  const openaiModel = h("select", { "aria-label": "OpenAI model" }) as HTMLSelectElement;
  const openaiNote = h("div", { class: "hint" });
  const openaiRefresh = h("button", { text: "Refresh" });
  let openaiLoaded = false;
  fillSelect(openaiModel, [], chatModel(store.get(), "openai"), "saved");
  openaiModel.addEventListener("change", () => store.update({ [CHAT_MODEL_FIELD.openai]: openaiModel.value }));
  const openaiKey = keyRow("openai", (present) => {
    const added = present && !keys.openai;
    keys.openai = present;
    paintCloud("openai");
    // A newly saved key is checked at once by listing the models it can use.
    if (added || (present && provider() === "openai" && !openaiLoaded)) void loadOpenAiModels();
    if (!present) {
      openaiLoaded = false;
      fillSelect(openaiModel, [], chatModel(store.get(), "openai"), "saved");
      openaiNote.textContent = "Add a key to see the models it can use.";
    }
  });

  async function loadOpenAiModels() {
    openaiNote.textContent = "Asking OpenAI which models this key can use…";
    openaiRefresh.disabled = true;
    try {
      paintOpenAiModels(await Bridge.chatModels("openai"));
    } catch (err) {
      openaiNote.textContent = String(err).replace(/^Error:\s*/, "");
    } finally {
      openaiRefresh.disabled = false;
    }
  }

  function paintOpenAiModels(r: ChatModels) {
    const saved = chatModel(store.get(), "openai");
    if (r.error) {
      openaiNote.textContent = r.error;
      dots.openai = provider() === "openai" ? "warn" : dots.openai;
      fillSelect(openaiModel, [], saved, "saved");
      paintHead();
      return;
    }
    openaiLoaded = true;
    fillSelect(openaiModel, r.models, r.selected, "not offered");
    if (r.selected !== saved) {
      store.update({ [CHAT_MODEL_FIELD.openai]: r.selected });
      openaiNote.textContent = `${saved} is not offered for this key any more, so the chat now uses ${r.selected}.`;
    } else {
      openaiNote.textContent = `${r.models.length} chat models available with this key.`;
    }
  }
  openaiRefresh.addEventListener("click", () => {
    if (!keys.openai) {
      openaiNote.textContent = "Add a key to see the models it can use.";
      return;
    }
    void loadOpenAiModels();
  });
  const openai = block("openai", openaiKey.el,
    h("div", { class: "row" }, h("label", { text: "Model" }), openaiModel, openaiRefresh),
    openaiNote);

  // ── Ollama ──────────────────────────────────────────────────────────────────
  const ollamaModel = h("select", { "aria-label": "Ollama model" }) as HTMLSelectElement;
  const ollamaStatus = h("div", { class: "hint" });
  const pullRow = h("div", { class: "row" });
  pullRow.style.display = "none";
  const ollamaRefresh = h("button", { text: "Refresh" });
  fillSelect(ollamaModel, [], chatModel(store.get(), "ollama"), "not installed");
  ollamaModel.addEventListener("change", () => {
    store.update({ [CHAT_MODEL_FIELD.ollama]: ollamaModel.value });
    void refreshOllama();
  });
  ollamaRefresh.addEventListener("click", () => void refreshOllama());

  async function refreshOllama() {
    ollamaStatus.textContent = "Checking Ollama…";
    ollamaRefresh.disabled = true;
    try {
      const [models, status] = await Promise.all([Bridge.chatModels("ollama"), Bridge.chatStatus("ollama")]);
      fillSelect(ollamaModel, models.models, chatModel(store.get(), "ollama"), "not installed");
      paintOllama(status);
    } catch (err) {
      ollamaStatus.textContent = String(err).replace(/^Error:\s*/, "");
      dots.ollama = "err";
      paintBlocks();
    } finally {
      ollamaRefresh.disabled = false;
    }
  }

  function paintOllama(s: ChatStatus) {
    clear(pullRow);
    pullRow.style.display = "none";
    const at = s.endpoint ? ` at ${s.endpoint}` : "";
    if (s.ready) {
      dots.ollama = "ok";
      ollamaStatus.textContent = `Ollama is running${at} with ${s.model}.`;
    } else if (s.reachable && !s.modelPresent) {
      dots.ollama = "warn";
      ollamaStatus.textContent = `Ollama is running, but ${s.model} is not installed. Run this in a terminal:`;
      const cmd = s.hint ?? `ollama pull ${s.model}`;
      const copy = h("button", { class: "small", text: "Copy" });
      copy.addEventListener("click", async () => {
        copy.textContent = (await copyText(cmd)) ? "Copied" : "Copy failed";
        window.setTimeout(() => (copy.textContent = "Copy"), 1400);
      });
      pullRow.append(h("code", { text: cmd }), copy);
      pullRow.style.display = "";
    } else {
      dots.ollama = provider() === "ollama" ? "err" : "off";
      ollamaStatus.textContent = `${s.error ?? "Ollama is not reachable."} ${s.hint ?? ""}`.trim();
    }
    paintBlocks();
  }
  const ollama = block("ollama",
    ollamaStatus,
    pullRow,
    h("div", { class: "row" }, h("label", { text: "Model" }), ollamaModel, ollamaRefresh),
    hint("Uses the endpoint from Local AI (loopback only). The chat works whether or not the local AI scans are switched on."));

  // ── Provider choice ─────────────────────────────────────────────────────────
  const blocks = { anthropic: claude, openai, ollama };

  function paintCloud(p: CloudProvider) {
    dots[p] = keys[p] ? "ok" : provider() === p ? "err" : "off";
    paintBlocks();
  }

  function paintBlocks() {
    for (const p of Object.keys(blocks) as ChatProvider[]) {
      blocks[p].setActive(p === provider());
      setDot(blocks[p].dot, dots[p]);
    }
    paintHead();
  }

  function paintHead() {
    const p = provider();
    const label = CHAT_PROVIDER_LABEL[p];
    const model = chatModel(store.get(), p);
    setDot(headDot, dots[p] === "off" ? "warn" : dots[p]);
    const problem = p === "ollama"
      ? dots.ollama === "ok" ? "" : " It isn't ready yet: see below."
      : keys[p] ? "" : ` Add your ${label} API key below.`;
    summary.textContent = `${label} answers the island chat with ${model}.${problem}`;
  }

  const choice = segmented(CHOICES, provider(), (p) => {
    store.update({ chatProvider: p });
    if (p !== "ollama") paintCloud(p);
    if (p === "openai" && keys.openai && !openaiLoaded) void loadOpenAiModels();
    if (p === "ollama") void refreshOllama();
    paintBlocks();
    onProvider(p);
  });

  el.append(
    summary,
    h("div", { class: "row" }, h("label", { text: "Provider" }), choice.el),
    hint("Switching the provider starts a new conversation, so nothing said to one is replayed to another."),
    claude.el,
    openai.el,
    ollama.el,
  );

  claudeKey.paint(keys.anthropic);
  openaiKey.paint(keys.openai);
  if (!keys.openai) openaiNote.textContent = "Add a key to see the models it can use.";
  else if (provider() !== "openai") openaiNote.textContent = "Press Refresh to list the models this key can use.";
  paintBlocks();
  void refreshOllama();
  return el;
}
