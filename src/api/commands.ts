// Typed wrappers around the backend's commands (src-tauri/src/app/commands.rs). A rejected promise carries
// an ErrorPayload.

import { invoke } from "@tauri-apps/api/core";
import type {
  Bootstrap,
  DownloadItem,
  ErrorPayload,
  HistoryItem,
  JobKind,
  LinkInspection,
  OutputSpec,
  PreviewSource,
  Resolution,
  Section,
  Settings,
  ThumbnailRequest,
  ToolId,
  ToolsReport,
} from "./types";

export function isErrorPayload(value: unknown): value is ErrorPayload {
  return typeof value === "object" && value !== null && "title" in value && "message" in value;
}

/** Turns anything a command rejected with into an ErrorPayload. */
export function asError(value: unknown): ErrorPayload {
  if (isErrorPayload(value)) return value;
  return { code: "unknown", title: "Something went wrong", message: String(value ?? ""), retryable: true };
}

export const api = {
  bootstrap: () => invoke<Bootstrap>("bootstrap"),
  updateSettings: (change: Partial<Settings>) => invoke<Settings>("update_settings", { change }),
  chooseDestination: () => invoke<Settings | null>("choose_destination"),
  openDestination: () => invoke<void>("open_destination"),
  chooseToolPath: (tool: ToolId) => invoke<Settings | null>("choose_tool_path", { tool }),
  resetToolPath: (tool: ToolId) => invoke<Settings>("reset_tool_path", { tool }),
  refreshTools: () => invoke<ToolsReport>("refresh_tools"),
  acknowledgeTerms: () => invoke<Settings>("acknowledge_terms"),
  setSection: (section: Section) => invoke<void>("set_section", { section }),
  quit: () => invoke<void>("quit_app"),

  inspectLinks: (text: string) => invoke<LinkInspection>("inspect_links", { text }),
  resolveLink: (url: string) => invoke<Resolution>("resolve_link", { url }),
  cancelResolve: () => invoke<void>("cancel_resolve"),
  enqueueDownloads: (items: DownloadItem[], output: OutputSpec) => invoke<number>("enqueue_downloads", { items, output }),

  importFiles: (paths: string[]) => invoke<number>("import_files", { paths }),
  chooseFilesToConvert: (folders: boolean) => invoke<number>("choose_files_to_convert", { folders }),

  cancelJob: (id: string) => invoke<void>("cancel_job", { id }),
  retryJob: (id: string) => invoke<void>("retry_job", { id }),
  removeJob: (id: string) => invoke<void>("remove_job", { id }),
  cancelAll: (kind: JobKind | null) => invoke<void>("cancel_all", { kind }),
  clearFinished: (kind: JobKind | null) => invoke<void>("clear_finished", { kind }),
  jobLog: (id: string) => invoke<string[]>("job_log", { id }),

  revealPath: (path: string) => invoke<void>("reveal_path", { path }),
  openPath: (path: string) => invoke<void>("open_path", { path }),
  copyText: (text: string) => invoke<void>("copy_text", { text }),
  readClipboard: () => invoke<string | null>("read_clipboard"),
  openThirdPartyNotices: () => invoke<void>("open_third_party_notices"),

  removeHistory: (ids: string[]) => invoke<void>("remove_history", { ids }),
  clearHistory: () => invoke<void>("clear_history"),
  repeatHistory: (id: string) => invoke<Section>("repeat_history", { id }),

  thumbnail: (source: ThumbnailRequest) => invoke<string | null>("thumbnail", { source }),
  waveform: (path: string) => invoke<number[]>("waveform", { path }),
  previewSource: (path: string, transcode: boolean) => invoke<PreviewSource>("preview_source", { path, transcode }),
  showMainWindow: () => invoke<void>("show_main_window"),
};

export type HistoryList = HistoryItem[];
