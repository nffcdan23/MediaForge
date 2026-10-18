// A small external store: the backend's state (jobs, history, settings, tools) plus what the window is showing.
// Components read slices with `useApp`, which re-renders only when the selected slice changes.

import { useSyncExternalStore } from "react";
import type { Alert, HistoryItem, Job, Platform, Section, Settings, ToolsReport } from "../api/types";

export interface Confirmation {
  title: string;
  message: string;
  confirmLabel: string;
  destructive?: boolean;
  onConfirm: () => void;
}

export interface AppData {
  ready: boolean;
  platform: Platform;
  version: string;
  build: string;
  isDebug: boolean;
  settings: Settings | null;
  defaultDestination: string;
  jobs: Job[];
  history: HistoryItem[];
  tools: ToolsReport | null;
  hasNotices: boolean;
  section: Section;
  showingTerms: boolean;
  alert: Alert | null;
  confirmation: Confirmation | null;
  videoPreview: { path: string; title: string; muted?: boolean } | null;
  log: { jobId: string; anchor: { x: number; y: number } } | null;
  sidebarCollapsed: boolean;
  dropActive: boolean;
}

export class Store<T> {
  private listeners = new Set<() => void>();
  constructor(private state: T) {}

  get = () => this.state;

  set(change: Partial<T> | ((state: T) => Partial<T>)) {
    const patch = typeof change === "function" ? change(this.state) : change;
    this.state = { ...this.state, ...patch };
    this.listeners.forEach((listener) => listener());
  }

  subscribe = (listener: () => void) => {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  };
}

export function useStore<T, S>(store: Store<T>, selector: (state: T) => S): S {
  return useSyncExternalStore(store.subscribe, () => selector(store.get()));
}

export const app = new Store<AppData>({
  ready: false,
  platform: "macos",
  version: "",
  build: "",
  isDebug: false,
  settings: null,
  defaultDestination: "",
  jobs: [],
  history: [],
  tools: null,
  hasNotices: false,
  section: "downloads",
  showingTerms: false,
  alert: null,
  confirmation: null,
  videoPreview: null,
  log: null,
  sidebarCollapsed: false,
  dropActive: false,
});

export const useApp = <S,>(selector: (state: AppData) => S) => useStore(app, selector);

/** Settings, which exist once the app has loaded. */
export const useSettings = () => useApp((s) => s.settings) as Settings;

export function upsertJob(job: Job) {
  app.set((s) => {
    const index = s.jobs.findIndex((j) => j.id === job.id);
    if (index === -1) return { jobs: [...s.jobs, job] };
    const jobs = s.jobs.slice();
    jobs[index] = job;
    return { jobs };
  });
}

export function removeJobs(ids: string[]) {
  const doomed = new Set(ids);
  app.set((s) => ({ jobs: s.jobs.filter((j) => !doomed.has(j.id)) }));
}
