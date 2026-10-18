// Keyboard shortcuts. On macOS the menu bar owns them (src-tauri/src/app/menu.rs); Windows has no menu bar, so
// the window handles the same commands with Ctrl in place of ⌘.

import { api } from "../api/commands";
import type { Section } from "../api/types";
import { isMac } from "../lib/platform";
import { chooseFilesToConvert, navigate, openDestination, pasteLink } from "./actions";
import { player } from "./player";
import { app } from "./store";

const sections: Section[] = ["downloads", "conversions", "history", "settings"];

export function installShortcuts() {
  window.addEventListener("keydown", (event) => {
    if (event.key === "Escape") {
      const s = app.get();
      if (s.log) app.set({ log: null });
      else if (s.videoPreview) app.set({ videoPreview: null });
      else if (s.alert) app.set({ alert: null });
      else if (s.confirmation) app.set({ confirmation: null });
      return;
    }
    if (isMac() || !event.ctrlKey || event.altKey || event.metaKey) return;
    const key = event.key.toLowerCase();
    const handled = (() => {
      if (event.shiftKey) {
        if (key === "v") return void pasteLink(), true;
        if (key === "d") return void openDestination(), true;
        if (key === "o") return void chooseFilesToConvert(true), true;
        if (key === "p") return player.togglePlayback(), true;
        return false;
      }
      if (key === "o") return void chooseFilesToConvert(false), true;
      if (key === ",") return navigate("settings"), true;
      const index = ["1", "2", "3", "4"].indexOf(key);
      if (index >= 0) return navigate(sections[index]), true;
      if (key === "q") return void api.quit(), true;
      return false;
    })();
    if (handled) event.preventDefault();
  });
}
