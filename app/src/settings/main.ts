// Settings window — the place where anything that writes to disk is confirmed.
// One scrolling page with a sticky section nav: Protection, Policy, Vault,
// Activity, Documents, Local AI, Browser, Chat and General.

import "./settings.css";
import { Bridge, IS_MOCK, onEvent, type ProtectionStatus } from "../core/bridge";
import { DEFAULT_SETTINGS, type Settings } from "../core/state";
import { h, clear, copyText } from "../views/dom";
import { protectionSection } from "./protection";
import { policySection } from "./policy";
import { vaultSection } from "./vault";
import { activitySection } from "./activity";
import { documentsSection } from "./documents";
import { localAiSection } from "./localai";
import { chatSection } from "./chat";
import { hint, section, setDot, statusDot, toggle } from "./ui";

let settings: Settings = { ...DEFAULT_SETTINGS };
let version = "";

const root = document.getElementById("settings-root")!;

async function save() {
  await Bridge.saveSettings(settings);
}

// ── Browser extension ─────────────────────────────────────────────────────────

function browserSection(status: ProtectionStatus | null): HTMLElement {
  const { el, head } = section("browser", "Browser extension");
  const dot = statusDot(status?.extensionConnected ? "ok" : "off");
  head.prepend(dot);
  const line = h("span", { class: "hint" });
  const paint = (p: ProtectionStatus | null) => {
    const on = !!p?.extensionConnected;
    setDot(dot, on ? "ok" : "off");
    line.textContent = on
      ? "Connected — the extension checked in during the last minute."
      : "Not connected. Install it once, keep Zuko running, and it connects by itself.";
  };
  paint(status);
  void onEvent("protection-changed", paint);

  const copy = (text: string) => {
    const b = h("button", { class: "small", text: "Copy" });
    b.addEventListener("click", async () => {
      b.textContent = (await copyText(text)) ? "Copied" : "Copy failed";
      window.setTimeout(() => (b.textContent = "Copy"), 1400);
    });
    return b;
  };

  el.append(
    line,
    hint("Masks your prompts on ChatGPT, claude.ai and DeepSeek before they are sent, restores the answers on screen, and sanitizes uploaded files."),
    h("ol", { class: "steps" },
      h("li", {}, "Open ", h("code", { text: "chrome://extensions" }), " (or ", h("code", { text: "edge://extensions" }), ") ", copy("chrome://extensions")),
      h("li", { text: "Turn on Developer mode (top right)." }),
      h("li", {}, "Click ", h("b", { text: "Load unpacked" }), " and pick the ", h("code", { text: "extension/dist" }), " folder of the Zuko repository."),
      h("li", { text: "Pin Zuko to the toolbar. Its badge turns teal once it reaches this app." }),
    ),
  );
  return el;
}

// ── Footer ────────────────────────────────────────────────────────────────────

/** Everywhere Zuko sends anything. OpenAI is only named when it answers the chat. */
function privacyLine(): string {
  return settings.chatProvider === "openai"
    ? "No telemetry. Zuko only talks to the Claude API through your own login or key, to the OpenAI API with your key for the island chat (masked text only), and, if you use it, to Ollama on this computer."
    : "No telemetry. Zuko only talks to the Claude API, through your own login or key (and, if you use it, to Ollama on this computer).";
}

// ── General ───────────────────────────────────────────────────────────────────

function generalSection(): HTMLElement {
  const { el } = section("general", "General");
  const volume = h("input", {
    type: "range", min: "0", max: "0.2", step: "0.005",
    value: String(settings.soundVolume),
  }) as HTMLInputElement;
  volume.addEventListener("input", () => {
    settings.soundVolume = Number(volume.value);
    void save();
  });

  const autoClose = h("input", {
    type: "number", min: "5", max: "120", step: "1",
    value: String(Math.round(settings.autoCloseInterval)),
    style: "width:72px",
  }) as HTMLInputElement;
  autoClose.addEventListener("change", () => {
    settings.autoCloseInterval = Math.max(5, Math.min(120, Number(autoClose.value) || 15));
    autoClose.value = String(settings.autoCloseInterval);
    void save();
  });

  const screen = h("select", {}) as HTMLSelectElement;
  screen.append(
    h("option", { value: "primary", text: "Main display" }),
    h("option", { value: "cursor", text: "Display under the cursor" }),
  );
  screen.value = settings.screen;
  screen.addEventListener("change", () => {
    settings.screen = screen.value as Settings["screen"];
    void save();
  });

  el.append(
    h("div", { class: "row" },
      h("label", { text: "Sound" }),
      toggle(settings.soundEnabled, (v) => { settings.soundEnabled = v; void save(); }, "Sound"),
      volume,
    ),
    h("div", { class: "row" },
      h("label", { text: "Auto-close" }),
      autoClose,
      h("span", { class: "hint", text: "seconds after you leave the island" }),
    ),
    h("div", { class: "row" },
      h("label", { text: "Island lives on" }),
      screen,
    ),
    h("div", { class: "row" },
      h("label", { text: "Launch at startup" }),
      toggle(settings.autostart, (v) => { settings.autostart = v; void save(); }, "Launch at startup"),
    ),
  );
  return el;
}

// ── Nav ───────────────────────────────────────────────────────────────────────

const SECTIONS: [id: string, label: string][] = [
  ["protection", "Protection"],
  ["policy", "Policy"],
  ["vault", "Vault"],
  ["activity", "Activity"],
  ["documents", "Documents"],
  ["localai", "Local AI"],
  ["browser", "Browser"],
  ["chat", "Chat"],
  ["general", "General"],
];

function nav(): HTMLElement {
  const links = SECTIONS.map(([id, label]) =>
    h("a", {
      href: `#${id}`, "data-id": id, text: label,
      onclick: (e: Event) => {
        e.preventDefault();
        document.getElementById(id)?.scrollIntoView({ behavior: "smooth", block: "start" });
      },
    }),
  );
  const el = h("nav", { class: "nav" }, ...links);
  // Highlight the section under the nav as the page scrolls.
  const mark = () => {
    let current = SECTIONS[0][0];
    for (const [id] of SECTIONS) {
      const s = document.getElementById(id);
      if (s && s.getBoundingClientRect().top < 120) current = id;
    }
    links.forEach((a) => a.classList.toggle("on", a.dataset.id === current));
  };
  window.addEventListener("scroll", mark, { passive: true });
  window.setTimeout(mark, 0);
  return el;
}

// ── Boot ──────────────────────────────────────────────────────────────────────

async function main() {
  const boot = await Bridge.boot();
  if (boot) {
    settings = { ...settings, ...boot.settings };
    version = boot.version;
  }
  const [status, policy, vault, activity, hasAnthropicKey, hasOpenAiKey] = await Promise.all([
    Bridge.protectionStatus(),
    Bridge.policyGet(),
    Bridge.vaultList(),
    Bridge.activityRecent(200),
    Bridge.secretPresent("anthropic-api-key"),
    Bridge.secretPresent("openai-api-key"),
  ]);

  const footer = hint(privacyLine());
  const chat = chatSection(
    {
      get: () => settings,
      update: (patch) => {
        settings = { ...settings, ...patch };
        void save();
      },
    },
    { anthropic: hasAnthropicKey ?? false, openai: hasOpenAiKey ?? false },
    () => (footer.textContent = privacyLine()),
  );

  const sections = [
    protectionSection(status),
    policySection(policy),
    vaultSection(vault),
    activitySection(activity, status?.auditPath ?? ""),
    documentsSection(),
    localAiSection(policy),
    browserSection(status),
    chat,
    generalSection(),
  ];
  // Dev only: `?mock=1&only=vault,activity` renders just those (screenshots).
  const only = IS_MOCK ? new URLSearchParams(window.location.search).get("only")?.split(",") : null;

  clear(root);
  root.append(
    h("header", { class: "top" },
      h("h1", {}, h("span", { text: "Zuko" }), h("span", { class: "version", text: version ? `v${version}` : "" })),
      nav()),
    ...sections.filter((s) => !only || only.includes(s.id)),
    footer,
  );

  void onEvent("settings-changed", (s) => {
    settings = { ...settings, ...s };
    footer.textContent = privacyLine();
  });

  if (window.location.hash) {
    document.getElementById(window.location.hash.slice(1))?.scrollIntoView({ block: "start" });
  }
}

void main();
