// What the window does in response to the user, and how it keeps in step with the backend.

import { api, asError } from "../api/commands";
import { on } from "../api/events";
import type { Appearance, ErrorPayload, OutputSpec, Section, Settings } from "../api/types";
import { useKiloLabel } from "../lib/format";
import { setPlatform } from "../lib/platform";
import { downloads } from "./downloads";
import { player } from "./player";
import { app, removeJobs, upsertJob } from "./store";

const systemDark = window.matchMedia("(prefers-color-scheme: dark)");

export function applyTheme(appearance: Appearance) {
  const dark = appearance === "dark" || (appearance === "system" && systemDark.matches);
  document.documentElement.dataset.theme = dark ? "dark" : "light";
}

systemDark.addEventListener("change", () => {
  const settings = app.get().settings;
  if (settings?.appearance === "system") applyTheme("system");
});

export function showError(error: unknown) {
  const payload: ErrorPayload = asError(error);
  if (payload.code === "cancelled") return;
  app.set({ alert: { title: payload.title, message: payload.message } });
}

export function navigate(section: Section) {
  if (app.get().section === section) return;
  app.set({ section, log: null });
  void api.setSection(section);
}

export async function updateSettings(change: Partial<Settings>) {
  // Shown straight away; the backend's reply (validated and clamped) then replaces it.
  app.set((s) => (s.settings ? { settings: { ...s.settings, ...change } } : {}));
  try {
    const settings = await api.updateSettings(change);
    app.set({ settings });
    applyTheme(settings.appearance);
  } catch (error) {
    showError(error);
  }
}

/** Puts the clipboard's text into the link field (⇧⌘V / Ctrl+Shift+V). */
export async function pasteLink() {
  navigate("downloads");
  const text = await api.readClipboard().catch(() => null);
  if (!text) return;
  downloads.setText(text);
}

export async function chooseFilesToConvert(folders = false) {
  navigate("conversions");
  try {
    await api.chooseFilesToConvert(folders);
  } catch (error) {
    showError(error);
  }
}

export async function chooseDestination() {
  try {
    const settings = await api.chooseDestination();
    if (settings) app.set({ settings });
  } catch (error) {
    showError(error);
  }
}

export const openDestination = () => api.openDestination().catch(showError);
export const reveal = (path: string | null | undefined) => (path ? api.revealPath(path).catch(showError) : undefined);
export const copy = (text: string) => api.copyText(text).catch(showError);

/** Preview a finished file: audio plays in the bar along the bottom, video and pictures in a preview window. */
export function preview(path: string | null | undefined, title: string, subtitle: string | null, artwork: string | null, kind: OutputSpec["type"]) {
  if (!path) return;
  if (kind === "audio") {
    player.toggle({ path, title, subtitle, artwork });
  } else {
    app.set({ filePreview: { path, title, kind } });
  }
}

export async function repeatHistory(id: string) {
  try {
    const section = await api.repeatHistory(id);
    navigate(section);
  } catch (error) {
    showError(error);
  }
}

export function confirmClearHistory() {
  app.set({
    confirmation: {
      title: "Clear all history?",
      message: "This removes the list only. Your downloaded and converted files stay where they are.",
      confirmLabel: "Clear History",
      destructive: true,
      onConfirm: () => void api.clearHistory().catch(showError),
    },
  });
}

/** Loads everything from the backend and starts listening for changes. */
export async function connect() {
  await Promise.all([
    on("jobs:changed", upsertJob),
    on("jobs:removed", removeJobs),
    on("history:changed", (history) => app.set({ history })),
    on("settings:changed", (settings) => {
      app.set({ settings });
      applyTheme(settings.appearance);
    }),
    on("tools:changed", (tools) => app.set({ tools })),
    on("app:navigate", (section) => app.set({ section, log: null })),
    on("app:alert", (alert) => app.set({ alert })),
    on("app:menu", handleMenu),
    on("app:debug", (debug) => {
      if (debug.action === "setUrl" && debug.value != null) {
        navigate("downloads");
        downloads.setText(debug.value, { enqueueWhenReady: debug.enqueue === true });
      } else if (debug.action === "scrollToEnd") {
        document.querySelectorAll(".page").forEach((page) => page.scrollTo({ top: page.scrollHeight }));
      } else if (debug.action === "previewMuted") {
        // Development check of the player without sound coming out of the speakers.
        const job = [...app.get().jobs].reverse().find((j) => j.state.type === "completed" && j.output.type === "audio");
        if (job?.outputPath) {
          player.setMuted(true);
          player.toggle({ path: job.outputPath, title: job.title, subtitle: job.detail, artwork: job.thumbnailUrl });
        }
      } else if (debug.action === "videoPreview" || debug.action === "imagePreview") {
        const kind = debug.action === "videoPreview" ? "video" : "image";
        const job = [...app.get().jobs].reverse().find((j) => j.state.type === "completed" && j.output.type === kind);
        if (job?.outputPath) app.set({ filePreview: { path: job.outputPath, title: job.title, kind, muted: true } });
      } else if (debug.action === "terms") {
        app.set({ showingTerms: true });
      } else if (debug.action.startsWith("log:")) {
        const index = Number(debug.action.slice(4));
        const job = [...app.get().jobs].reverse()[index];
        if (job) app.set({ log: { jobId: job.id, anchor: { x: window.innerWidth / 2, y: 200 } } });
      }
    }),
  ]);

  const boot = await api.bootstrap();
  setPlatform(boot.platform);
  useKiloLabel(boot.platform === "macos" ? "kB" : "KB");
  applyTheme(boot.settings.appearance);
  document.documentElement.dataset.platform = boot.platform;
  app.set({
    ready: true,
    platform: boot.platform,
    version: boot.version,
    build: boot.build,
    isDebug: boot.isDebug,
    settings: boot.settings,
    defaultDestination: boot.defaultDestination,
    jobs: boot.jobs,
    history: boot.history,
    tools: boot.tools,
    hasNotices: boot.hasThirdPartyNotices,
    section: boot.pendingSection ?? boot.settings.selectedSection,
    showingTerms: !boot.settings.hasAcknowledgedTerms,
  });
}

function handleMenu(action: string) {
  switch (action) {
    case "import":
      void chooseFilesToConvert(false);
      break;
    case "import-folder":
      void chooseFilesToConvert(true);
      break;
    case "paste":
      void pasteLink();
      break;
    case "toggle-preview":
      player.togglePlayback();
      break;
    case "terms":
      app.set({ showingTerms: true });
      break;
  }
}

export { handleMenu };
