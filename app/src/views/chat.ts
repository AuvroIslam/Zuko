// Chat view — DOM port of PromptView / ChatBubble / TypingDotsView from
// IslandViewContent.swift.

import { h, svg, clear, withPlaceholders } from "./dom";
import { ICONS } from "./icons";
import { Bridge, type ChatContext, type ChatModels } from "../core/bridge";
import { Sound } from "../core/sound";
import {
  CHAT_MODEL_FIELD, CHAT_PROVIDERS, CHAT_PROVIDER_LABEL, State, chatModel, type ChatMessage, type ChatProvider,
  type SentRecord,
} from "../core/state";
import type { ViewHost } from "./views";

let nextId = 1;

function bubble(message: ChatMessage): HTMLElement {
  if (message.role === "user") {
    const row = h(
      "div",
      { class: "chat-row user" },
      h("div", { class: "bubble", text: message.content }),
    );
    return message.sent ? h("div", { class: "chat-turn" }, row, sentBlock(message.sent)) : row;
  }
  return h("div", { class: "chat-row" }, h("div", { class: "reply", text: message.content }));
}

/**
 * Under a sent message: what the model actually received. The message as it left, and
 * the dropped file (first message only) behind a toggle, masked. The real values are
 * in the bubble above; this is the proof they never left.
 */
function sentBlock(sent: SentRecord): HTMLElement {
  const who = CHAT_PROVIDER_LABEL[sent.provider];
  const local = sent.provider === "ollama";
  const count = sent.masked.length;
  const head = h(
    "div",
    { class: "chat-sent-head" },
    h("span", { text: local ? `What ${who} received (on this PC)` : `What ${who} received` }),
    h("span", {
      class: `chat-sent-count${count ? " hit" : ""}`,
      text: count ? `${count} masked` : "nothing to mask",
      title: count ? sent.masked.join(", ") : "",
    }),
  );
  const el = h("div", { class: "chat-sent" }, head, h("div", { class: "chat-sent-text" }, withPlaceholders(sent.text)));
  if (sent.file) {
    const more = sent.file.chars > sent.file.preview.length ? "\n…" : "";
    el.append(h(
      "details",
      { class: "chat-sent-file" },
      h("summary", { text: `${sent.file.name} · ${sent.file.kind}, masked` }),
      h("pre", {}, withPlaceholders(sent.file.preview + more)),
    ));
  }
  return el;
}

function typingDots(): HTMLElement {
  return h(
    "div",
    { class: "chat-row" },
    h("div", { class: "typing" }, h("i"), h("i"), h("i")),
  );
}

/** The coloured chip showing what the question is about (a dropped file). */
function contextChip(label: string): HTMLElement {
  const chip = h("div", { class: "chip" }, h("i", { class: "chip-dot" }), h("span", { text: label }));
  requestAnimationFrame(() => chip.classList.add("settled"));
  return chip;
}

/**
 * Who answers, and where the words go: "Claude · claude-opus-5 · masked" (the cloud
 * gets masked text) or "Ollama · gemma3:4b · on this PC". Kept in step with Settings.
 * A click opens the switcher.
 */
function chatHead(onClick: () => void): { el: HTMLElement; sync(): void } {
  const dot = h("i", { class: "chat-head-dot" });
  const who = h("b");
  const model = h("span", { class: "chat-head-model" });
  const where = h("span");
  const caret = h("i", { class: "chat-head-caret" }, svg(ICONS.chevronDown, 9));
  const el = h("button", { class: "chat-head", type: "button", onclick: onClick }, dot, who, model, where, caret);
  let painted = "";
  return {
    el,
    sync() {
      const provider = State.settings.chatProvider ?? "anthropic";
      const m = chatModel(State.settings, provider);
      const key = `${provider}|${m}`;
      if (key === painted) return;
      painted = key;
      const local = provider === "ollama";
      el.classList.toggle("local", local);
      who.textContent = CHAT_PROVIDER_LABEL[provider];
      model.textContent = m;
      where.textContent = local ? "on this PC" : "masked";
      el.title = (local
        ? "Answered by Ollama on this computer. Nothing leaves your PC."
        : `Secrets and personal data are masked before anything is sent to ${CHAT_PROVIDER_LABEL[provider]}.`)
        + " Click to switch.";
    },
  };
}

/** Short names for Zuko's Claude models; any other model shows its id. */
const MODEL_LABEL: Record<string, string> = {
  "claude-opus-5": "Opus 5",
  "claude-sonnet-5": "Sonnet 5",
  "claude-haiku-4-5": "Haiku 4.5",
};

/** OpenAI offers dozens of models to a key; the switcher shows these, in this order. */
const OPENAI_FIRST = ["gpt-5-mini", "gpt-5", "gpt-4.1-mini", "gpt-4.1", "gpt-4o-mini", "gpt-4o"];
const MAX_OPTIONS = 6;

/** The models offered for one provider: a short list, always including the one in use. */
function shortList(provider: ChatProvider, models: string[], current: string | null): string[] {
  let list = models;
  if (provider === "anthropic") {
    // Biggest first, the way people think of them, not alphabetical.
    const order = Object.keys(MODEL_LABEL);
    list = [...models].sort((a, b) => (order.indexOf(a) + 1 || 99) - (order.indexOf(b) + 1 || 99));
  }
  if (provider === "openai") {
    const first = OPENAI_FIRST.filter((m) => models.includes(m));
    list = first.length ? first : models;
  }
  list = list.slice(0, MAX_OPTIONS);
  if (current && models.includes(current) && !list.includes(current)) list = [current, ...list.slice(0, MAX_OPTIONS - 1)];
  return list;
}

/**
 * Switch provider and model from the chat itself, with the keys already saved in
 * Settings → Chat. Each provider's list comes from Rust (`chat_models`): Claude's is
 * Zuko's own, OpenAI's what the key can use, Ollama's what is installed. A provider
 * that cannot answer (no key, Ollama not running) says why instead of offering models.
 * Changing provider starts a new conversation, like it does from Settings: what was
 * said to one provider is never replayed to another. The dropped file stays attached.
 */
function modelSwitcher(onResize: () => void): { el: HTMLElement; toggle(): void; close(): void } {
  const el = h("div", { class: "chat-switch", "data-nodrag": "" });
  el.hidden = true;
  const lists = new Map<ChatProvider, ChatModels | "loading" | string>();

  function pick(provider: ChatProvider, model: string) {
    const switched = provider !== State.settings.chatProvider;
    State.settings.chatProvider = provider;
    State.settings[CHAT_MODEL_FIELD[provider]] = model;
    void Bridge.saveSettings(State.settings);
    if (switched && State.chatHistory.length) {
      State.chatHistory = [];
      void Bridge.chatReset();
    }
    Sound.play("blip");
    close();
    State.notify();
  }

  function render() {
    clear(el);
    const current = State.settings.chatProvider ?? "anthropic";
    for (const provider of CHAT_PROVIDERS) {
      const using = chatModel(State.settings, provider);
      const row = h("div", { class: "chat-switch-row" },
        h("span", { class: "chat-switch-name", text: CHAT_PROVIDER_LABEL[provider] }),
        h("span", { class: "chat-switch-where", text: provider === "ollama" ? "on this PC" : "masked" }));
      const opts = h("div", { class: "chat-switch-opts" });
      const state = lists.get(provider);
      if (state === undefined || state === "loading") {
        opts.append(h("span", { class: "chat-switch-note", text: "Loading…" }));
      } else if (typeof state === "string" || state.error || !state.models.length) {
        const why = typeof state === "string" ? state : state.error ?? "No models found.";
        opts.append(
          h("span", { class: "chat-switch-note", text: why }),
          h("button", { class: "chat-switch-link", type: "button", text: "Settings", onclick: () => void Bridge.openSettingsWindow() }),
        );
      } else {
        for (const m of shortList(provider, state.models, using)) {
          const on = provider === current && m === using;
          opts.append(h("button", {
            class: `chat-switch-opt${on ? " on" : ""}`,
            type: "button",
            title: m,
            text: MODEL_LABEL[m] ?? m,
            onclick: () => pick(provider, m),
          }));
        }
      }
      row.append(opts);
      el.append(row);
    }
    onResize();
  }

  function load() {
    for (const provider of CHAT_PROVIDERS) {
      if (!lists.has(provider)) lists.set(provider, "loading");
      Bridge.chatModels(provider)
        .then((r) => lists.set(provider, r))
        .catch((err) => lists.set(provider, String(err).replace(/^Error:\s*/, "")))
        .finally(() => {
          if (!el.hidden) render();
        });
    }
  }

  function close() {
    if (el.hidden) return;
    el.hidden = true;
    onResize();
  }

  return {
    el,
    toggle() {
      if (!el.hidden) return close();
      el.hidden = false;
      render();
      load();
    },
    close,
  };
}

export function buildPrompt(onHeightChange: () => void): ViewHost {
  const switcher = modelSwitcher(onHeightChange);
  const head = chatHead(() => switcher.toggle());
  const chipRow = h("div", { class: "chip-row" });
  const log = h("div", { class: "chat-log" });
  const input = h("input", {
    type: "text",
    class: "chat-input",
    placeholder: "Ask me anything…",
    spellcheck: "false",
  }) as HTMLInputElement;
  const send = h("button", { class: "send-btn", title: "Send" }, svg(ICONS.arrowUp, 11));
  const bar = h("div", { class: "chat-bar" }, input, send);

  const el = h(
    "div",
    { class: "view" },
    h("div", { class: "card wash chat-card" }, h("div", { class: "chat-body" }, head.el, switcher.el, chipRow, log, bar)),
  );
  (el.querySelector(".card") as HTMLElement).style.setProperty("--wash", "rgba(99,102,241,0.5)");

  let sending = false;
  let renderedCount = -1;

  async function submit() {
    const query = input.value.trim();
    if (!query || sending) return;
    switcher.close();
    input.value = "";
    sending = true;
    Sound.play("send");

    const mine: ChatMessage = { id: nextId++, role: "user", content: query };
    const provider = State.settings.chatProvider ?? "anthropic";
    State.chatHistory.push(mine);
    State.stateOverride = "thinking";
    State.notify();
    onHeightChange();

    const file = State.droppedFile;
    const context: ChatContext | null =
      State.chatHistory.length === 1 && file ? { kind: "file", name: file.name, path: file.path } : null;

    try {
      const reply = await Bridge.chatSend(query, context);
      mine.sent = {
        provider,
        text: reply.sent,
        file: reply.sentFile,
        masked: reply.masked.map((m) => m.label || m.key),
      };
      State.chatHistory.push({ id: nextId++, role: "assistant", content: reply.text });
      State.stateOverride = null;
      Sound.play("finish");
    } catch (err) {
      State.stateOverride = null;
      State.noteMessage = String(err).replace(/^Error:\s*/, "");
      State.view = "note";
      Sound.play("error");
    } finally {
      sending = false;
      State.notify();
      onHeightChange();
      input.focus();
    }
  }

  send.addEventListener("click", () => void submit());
  input.addEventListener("keydown", (e) => {
    if ((e as KeyboardEvent).key === "Enter") {
      e.preventDefault();
      void submit();
    }
    e.stopPropagation(); // Escape closes the island, not the chat
  });

  return {
    el,
    sync() {
      const file = State.droppedFile;
      const wantChip = file?.name ?? "";
      if (chipRow.dataset.label !== wantChip) {
        chipRow.dataset.label = wantChip;
        clear(chipRow);
        if (wantChip) chipRow.append(contextChip(wantChip));
      }

      const thinking = State.stateOverride === "thinking";
      const count = State.chatHistory.length + (thinking ? 0.5 : 0);
      if (count !== renderedCount) {
        renderedCount = count;
        clear(log);
        for (const m of State.chatHistory) log.append(bubble(m));
        if (thinking) log.append(typingDots());
        // An answer is read from its question down: the last message sent (with what the
        // model received under it) at the top. While waiting, the dots at the bottom.
        const turns = log.querySelectorAll<HTMLElement>(".chat-turn, .chat-row.user");
        const last = turns[turns.length - 1];
        log.scrollTop = !thinking && last ? last.offsetTop - log.offsetTop - 2 : log.scrollHeight;
      }

      head.sync();
      const label = CHAT_PROVIDER_LABEL[State.settings.chatProvider ?? "anthropic"];
      input.placeholder = State.chatHistory.length === 0 ? `Ask ${label} anything…` : `Continue with ${label}…`;
      input.disabled = sending;
    },
    focus() {
      input.focus();
      input.select();
    },
  };
}
