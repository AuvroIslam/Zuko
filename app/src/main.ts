// Entry point: boot the bridge, wire the island, start the greeting.

import "./style.css";
import { Bridge, IS_TAURI, mockModule, onEvent } from "./core/bridge";
import { Sound } from "./core/sound";
import { State } from "./core/state";
import { Island } from "./island/island";
import { registerHookHandlers, registerZukoHandlers, seedZukoState } from "./island/hooks";

async function main() {
  const root = document.getElementById("root");
  if (!root) return;

  void Sound.preload();

  const island = new Island(root);

  const boot = await Bridge.boot();
  if (boot) {
    State.settings = { ...State.settings, ...boot.settings };
  }
  island.applySettings();
  State.loadSurfaces();
  if (boot && !boot.cursorPoll) island.followPageCursor();

  await onEvent("cursor", ({ x, y }) => island.onCursor(x, y));

  /** Pause has to reach Rust too, or the relay keeps waiting on the island. */
  const setPaused = (on: boolean) => {
    if (State.paused === on) return;
    State.paused = on;
    void Bridge.setPaused(on);
  };

  await onEvent("tray", (what) => {
    switch (what) {
      case "settings":
        setPaused(false);
        island.alert("settings");
        break;
      case "open":
        setPaused(false);
        island.alert(State.defaultView());
        break;
      case "activity":
        setPaused(false);
        island.alert("activity");
        break;
      case "pause":
        setPaused(!State.paused);
        if (State.paused) island.fsm.forceHidden();
        else island.reveal();
        break;
    }
  });

  await onEvent("screen-changed", () => void Bridge.reposition());

  // The settings window came forward: the island folds back to compact so it never
  // covers its title bar, unless an approval card is waiting for an answer.
  await onEvent("settings-focused", () => island.yieldToSettings());

  // The settings window writes preferences; apply them here without a restart.
  await onEvent("settings-changed", (s) => {
    // A conversation belongs to one chat provider (Rust drops it on the next turn):
    // clear the bubbles too, so the screen never suggests otherwise.
    if (s.chatProvider && s.chatProvider !== State.settings.chatProvider && State.chatHistory.length) {
      State.chatHistory = [];
      void Bridge.chatReset();
    }
    State.settings = { ...State.settings, ...s };
    island.applySettings();
  });

  registerHookHandlers(island);
  registerZukoHandlers(island);
  await seedZukoState();

  // Dev only (`npx vite`, `?mock=1&scene=…`): jump straight to a scene.
  const mock = mockModule();
  if (mock) {
    const scene = new URLSearchParams(window.location.search).get("scene");
    if (scene) {
      (await mock).playScene(scene, island);
      return;
    }
  }

  island.launch();

  // In a plain browser there is no wake strip behind the cursor: make the whole
  // page wake the island so the visuals can be checked with `npm run dev`.
  if (!IS_TAURI) {
    document.addEventListener("click", () => Sound.resume(), { once: true });
  }
}

void main();
