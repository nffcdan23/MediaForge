// Events the backend sends (src-tauri/src/app/state.rs, `events`).

import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { Alert, HistoryItem, Job, Section, Settings, TagFile, ToolsReport } from "./types";

export interface DebugAction {
  action: string;
  value?: string;
  enqueue?: boolean;
}

interface EventMap {
  "jobs:changed": Job;
  "jobs:removed": string[];
  "history:changed": HistoryItem[];
  "settings:changed": Settings;
  "tools:changed": ToolsReport;
  "app:navigate": Section;
  "app:menu": string;
  "app:alert": Alert;
  "app:debug": DebugAction;
  "metadata:added": TagFile[];
  "metadata:progress": { done: number; total: number };
}

export function on<K extends keyof EventMap>(name: K, handler: (payload: EventMap[K]) => void): Promise<UnlistenFn> {
  return listen<EventMap[K]>(name, (event) => handler(event.payload));
}
