// State behind the link field: validates what's typed, looks it up, and turns it into jobs.
// The same state machine as DownloadsViewModel.swift; validation and lookups happen in the backend.

import { api, asError } from "../api/commands";
import type { DownloadItem, ErrorPayload, LinkSummary, OutputSpec, Resolution, Settings, VideoQuality } from "../api/types";
import { videoQualities } from "../lib/options";
import { expectsConversionNote } from "../lib/status";
import { Store, useStore } from "./store";

export type Detection =
  | { type: "idle" }
  | { type: "invalid"; message: string }
  | { type: "resolving"; url: LinkSummary }
  | { type: "media"; url: LinkSummary; resolution: Resolution }
  | { type: "playlist"; url: LinkSummary; resolution: Resolution }
  | { type: "batch"; urls: LinkSummary[]; rejected: number; truncated: boolean }
  | { type: "failed"; url: LinkSummary; error: ErrorPayload };

interface DownloadsState {
  text: string;
  detection: Detection;
}

/** How long to wait after the last keystroke before contacting the source. */
const DEBOUNCE_MS = 450;

class DownloadsController {
  readonly store = new Store<DownloadsState>({ text: "", detection: { type: "idle" } });
  private timer: number | undefined;
  private generation = 0;
  private enqueueWhenReady = false;
  /** Supplies the current settings, so the output follows the saved preferences. */
  settings: () => Settings | null = () => null;

  get detection() {
    return this.store.get().detection;
  }

  setText(text: string, options: { enqueueWhenReady?: boolean } = {}) {
    this.enqueueWhenReady = options.enqueueWhenReady ?? false;
    this.store.set({ text: text.slice(0, 100_000) });
    void this.evaluate(false);
  }

  /** Return key: look up now if still waiting, otherwise queue what's on screen. */
  submit() {
    if (this.pendingJobCount() > 0) {
      void this.enqueue();
    } else {
      void this.evaluate(true);
    }
  }

  retry() {
    if (this.detection.type === "failed") this.store.set({ detection: { type: "idle" } });
    void this.evaluate(true);
  }

  clear() {
    window.clearTimeout(this.timer);
    this.generation += 1;
    void api.cancelResolve();
    this.store.set({ text: "", detection: { type: "idle" } });
  }

  private async evaluate(immediately: boolean) {
    const text = this.store.get().text.trim();
    const generation = ++this.generation;
    window.clearTimeout(this.timer);
    if (!text) {
      void api.cancelResolve();
      this.store.set({ detection: { type: "idle" } });
      return;
    }
    const inspection = await api.inspectLinks(text);
    if (generation !== this.generation) return;

    const first = inspection.urls[0];
    if (!first) {
      void api.cancelResolve();
      this.store.set({ detection: { type: "invalid", message: inspection.invalidMessage ?? "That doesn't look like a valid link." } });
      return;
    }
    if (inspection.urls.length > 1) {
      void api.cancelResolve();
      this.store.set({ detection: { type: "batch", urls: inspection.urls, rejected: inspection.rejected, truncated: inspection.truncated } });
      this.maybeEnqueue();
      return;
    }

    // Don't look up again for edits that leave the link itself unchanged.
    const current = this.detection;
    if ((current.type === "media" || current.type === "playlist" || current.type === "failed") && current.url.url === first.url) return;
    if (current.type === "resolving" && current.url.url === first.url && !immediately) return;

    this.store.set({ detection: { type: "resolving", url: first } });
    if (!immediately) {
      await new Promise<void>((resolve) => {
        this.timer = window.setTimeout(resolve, DEBOUNCE_MS);
      });
      if (generation !== this.generation) return;
    }
    try {
      const resolution = await api.resolveLink(first.url);
      if (generation !== this.generation) return;
      this.store.set({ detection: { type: resolution.isPlaylist ? "playlist" : "media", url: first, resolution } });
      this.maybeEnqueue();
    } catch (raw) {
      const error = asError(raw);
      if (error.code === "cancelled" || generation !== this.generation) return;
      this.store.set({ detection: { type: "failed", url: first, error } });
    }
  }

  private maybeEnqueue() {
    if (this.enqueueWhenReady) {
      this.enqueueWhenReady = false;
      void this.enqueue();
    }
  }

  resolution(): Resolution | null {
    const d = this.detection;
    return d.type === "media" ? d.resolution : null;
  }

  pendingJobCount(): number {
    const d = this.detection;
    if (d.type === "media") return 1;
    if (d.type === "playlist") return d.resolution.entries.length;
    if (d.type === "batch") return d.urls.length;
    return 0;
  }

  /** Creates jobs for whatever is detected, then clears the field. */
  async enqueue(): Promise<number> {
    const settings = this.settings();
    if (!settings) return 0;
    const d = this.detection;
    const output = outputFor(settings, this.resolution());
    let items: DownloadItem[] = [];
    if (d.type === "media") {
      const r = d.resolution;
      items = [{ url: r.url, title: r.title, detail: r.uploader, duration: r.duration, thumbnailUrl: r.thumbnailUrl }];
    } else if (d.type === "playlist") {
      items = d.resolution.entries.map((e) => ({ url: e.url, title: e.title, duration: e.duration }));
    } else if (d.type === "batch") {
      items = d.urls.map((u) => ({ url: u.url, title: u.displayName }));
    }
    if (items.length === 0) return 0;
    const count = await api.enqueueDownloads(items, output);
    this.clear();
    return count;
  }
}

export const downloads = new DownloadsController();

export const useDownloads = <S,>(selector: (state: DownloadsState) => S) => useStore(downloads.store, selector);

// ── Derived choices, as in DownloadsViewModel.swift ──────────────────────

/** Video is only offered when the source actually has some. */
export const offersVideo = (resolution: Resolution | null) => resolution?.hasVideo ?? true;

export function effectiveMode(settings: Settings, resolution: Resolution | null) {
  return offersVideo(resolution) ? settings.downloadMode : "audio";
}

export function qualityOptions(resolution: Resolution | null): VideoQuality[] {
  return resolution ? resolution.qualityOptions : videoQualities;
}

/** The saved preference, or "Best" when this source doesn't go that high. */
export function effectiveQuality(settings: Settings, resolution: Resolution | null): VideoQuality {
  return qualityOptions(resolution).includes(settings.videoQuality) ? settings.videoQuality : 0;
}

export function outputFor(settings: Settings, resolution: Resolution | null): OutputSpec {
  if (effectiveMode(settings, resolution) === "video") return { type: "video", quality: effectiveQuality(settings, resolution) };
  return { type: "audio", format: settings.audioFormat, bitrate: settings.audioBitrate, sampleRate: settings.sampleRate, bitDepth: settings.bitDepth };
}

export function conversionNoteApplies(settings: Settings, resolution: Resolution | null): boolean {
  return expectsConversionNote(effectiveMode(settings, resolution), settings.videoCompatibility, resolution, effectiveQuality(settings, resolution));
}
