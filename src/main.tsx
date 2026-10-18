import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import { connect, showError } from "./state/actions";
import { downloads } from "./state/downloads";
import { player } from "./state/player";
import { installShortcuts } from "./state/shortcuts";
import { app } from "./state/store";
import "./styles/theme.css";
import "./styles/app.css";

downloads.settings = () => app.get().settings;
player.onError = (message) => app.set({ alert: { title: "Can't Preview", message } });
installShortcuts();

// The webview's own context menu (Reload, Inspect…) has no place in a desktop app.
window.addEventListener("contextmenu", (event) => {
  const target = event.target as HTMLElement | null;
  if (!target?.closest("input, textarea, .selectable")) event.preventDefault();
});

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);

connect().catch(showError);
