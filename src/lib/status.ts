// One-line descriptions of jobs, as JobRow.swift wrote them.

import type { Job, JobState, Resolution, VideoCompatibility, DownloadMode } from "../api/types";
import { bytes, conversionSpeed, duration, percent, remaining, speed } from "./format";
import { outputBadge, outputSummary } from "./options";

export const isActive = (state: JobState) =>
  state.type === "preparing" || state.type === "downloading" || state.type === "processing" || state.type === "converting" || state.type === "finalizing";
export const isFinished = (state: JobState) => state.type === "completed" || state.type === "failed" || state.type === "cancelled";
export const isPending = (state: JobState) => state.type === "queued" || isActive(state);

/** "Downloading · 42% · 12 MB of 29 MB · 3.2 MB/s · 12s left" and the like. */
export function jobStatusText(job: Job): string {
  const p = job.progress;
  switch (job.state.type) {
    case "queued":
      return "Waiting to start";
    case "preparing":
      return job.kind === "download" ? "Contacting source…" : "Preparing…";
    case "downloading": {
      const parts: string[] = [];
      if (p.fraction != null) parts.push(percent(p.fraction));
      if (p.downloadedBytes != null) {
        parts.push(p.totalBytes ? `${bytes(p.downloadedBytes)} of ${bytes(p.totalBytes)}` : bytes(p.downloadedBytes));
      }
      if (p.bytesPerSecond != null && p.bytesPerSecond > 0) parts.push(speed(p.bytesPerSecond));
      if (p.secondsRemaining != null) parts.push(remaining(p.secondsRemaining));
      return parts.length === 0 ? "Downloading…" : `Downloading · ${parts.filter(Boolean).join(" · ")}`;
    }
    case "processing":
      return `${job.state.stage}…`;
    case "converting": {
      const parts = [`Converting to ${outputBadge(job.output)}`];
      if (p.fraction != null) parts.push(percent(p.fraction));
      const multiple = conversionSpeed(p.conversionSpeed);
      if (multiple) parts.push(multiple);
      if (p.secondsRemaining != null && p.secondsRemaining >= 1) parts.push(remaining(p.secondsRemaining));
      return parts.join(" · ");
    }
    case "finalizing":
      return "Saving…";
    case "completed": {
      const parts = [outputSummary(job.output)];
      if (job.outputSize != null) parts.push(bytes(job.outputSize));
      if (job.duration != null) parts.push(duration(job.duration));
      return parts.join(" · ");
    }
    case "failed":
      return job.state.error.message;
    case "cancelled":
      return "Cancelled";
  }
}

/** Whether the preview card should say the download will be converted after it arrives. */
export function expectsConversionNote(
  mode: DownloadMode,
  compatibility: VideoCompatibility,
  resolution: Pick<Resolution, "conversionExpected"> | null,
  quality: number,
): boolean {
  if (mode !== "video" || compatibility === "original" || !resolution) return false;
  return resolution.conversionExpected[String(quality)] === true;
}
