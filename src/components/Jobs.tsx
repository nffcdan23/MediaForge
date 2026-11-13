// Job rows and the queue list shared by Downloads and Conversions (JobRow.swift), plus the audio and image
// pickers and the missing-tools banner.

import { AudioWaveform, Copy, Eye, Film, Image as ImageIcon, Link, RotateCw, Search, TextAlignStart, Trash, TriangleAlert, Wrench, X } from "lucide-react";
import { useMemo, type ReactNode } from "react";
import { api } from "../api/commands";
import type { Job, JobKind, OutputSpec } from "../api/types";
import {
  audioFormats,
  bitDepthName,
  bitDepths,
  bitrateName,
  bitrates,
  formatName,
  imageFormatName,
  imageFormats,
  imageQualities,
  imageQualityName,
  isLossy,
  outputBadge,
  outputSummary,
  sampleRateName,
  sampleRates,
} from "../lib/options";
import { previewLabel, revealLabel } from "../lib/platform";
import { isFinished, isPending, jobStatusText } from "../lib/status";
import { copy, preview, reveal, showError, updateSettings } from "../state/actions";
import { usePlayer } from "../state/player";
import { app, useApp, useSettings } from "../state/store";
import { EmptyState, FormatBadge, IconButton, OptionField, Popup, ProgressBar, SectionHeader, Thumbnail } from "./Controls";
import { PauseFill, PlayFill } from "./Icons";
import { tidy, useContextMenu } from "./Menu";

function thumbnailFor(job: Job) {
  if (job.thumbnailUrl) return { kind: "remote" as const, url: job.thumbnailUrl };
  if (job.source.type === "file") return { kind: "file" as const, path: job.source.path };
  return job.outputPath ? { kind: "file" as const, path: job.outputPath } : null;
}

/** The placeholder artwork for a kind of output. */
export function OutputIcon({ output, size }: { output: OutputSpec; size: number }) {
  if (output.type === "audio") return <AudioWaveform size={size} color="url(#brand-gradient)" />;
  if (output.type === "image") return <ImageIcon size={size} color="url(#brand-gradient)" />;
  return <Film size={size} color="url(#brand-gradient)" />;
}

/** A card for one download or conversion, with live progress and the actions that make sense for its state. */
export function JobRow({ job }: { job: Job }) {
  const isAudio = job.output.type === "audio";
  const playing = usePlayer((p) => p.item?.path === job.outputPath && p.isPlaying && !!job.outputPath);
  const pending = isPending(job.state);
  const failed = job.state.type === "failed" ? job.state.error : null;
  const canRetry = job.state.type === "cancelled" || (failed?.retryable ?? false);

  const doPreview = () => preview(job.outputPath, job.title, job.detail ?? outputSummary(job.output), job.thumbnailUrl, job.output.type);
  const showLog = (x: number, y: number) => app.set({ log: { jobId: job.id, anchor: { x, y } } });

  const { onContextMenu, menu } = useContextMenu(() =>
    tidy([
      job.state.type === "completed" && { label: isAudio ? "Preview" : previewLabel(), icon: <Eye size={14} />, onSelect: doPreview },
      job.state.type === "completed" && { label: revealLabel(), icon: <Search size={14} />, onSelect: () => void reveal(job.outputPath) },
      job.state.type === "completed" && "separator",
      pending && { label: "Cancel", icon: <X size={14} />, onSelect: () => void api.cancelJob(job.id) },
      canRetry || job.state.type === "failed" ? { label: "Try Again", icon: <RotateCw size={14} />, onSelect: () => void api.retryJob(job.id) } : null,
      job.source.type === "remote" && { label: "Copy Link", icon: <Link size={14} />, onSelect: () => void copy((job.source as { url: string }).url) },
      job.hasLog && { label: "Show Details", icon: <TextAlignStart size={14} />, onSelect: () => showLog(window.innerWidth / 2, 160) },
      isFinished(job.state) && "separator",
      isFinished(job.state) && { label: "Remove from List", icon: <Trash size={14} />, onSelect: () => void api.removeJob(job.id) },
    ]),
  );

  return (
    <div className="job-row card" onContextMenu={onContextMenu}>
      <Thumbnail
        source={thumbnailFor(job)}
        icon={<OutputIcon output={job.output} size={22} />}
        duration={job.duration}
        completed={job.state.type === "completed"}
      />
      <div className="body">
        <div className="title-line">
          <span className="title" title={job.title}>
            {job.title}
          </span>
          <FormatBadge text={outputBadge(job.output)} />
        </div>
        {failed && (
          <div className="error-title">
            <TriangleAlert size={12} strokeWidth={2.4} />
            {failed.title}
          </div>
        )}
        <div className={`status${failed ? " multiline selectable" : ""}`}>{jobStatusText(job)}</div>
        {pending && <ProgressBar fraction={job.state.type === "queued" ? 0 : job.progress.fraction} />}
      </div>
      <div className="row-actions">
        {pending && (
          <IconButton label="Cancel" onClick={() => void api.cancelJob(job.id)}>
            <X size={13} strokeWidth={2.6} />
          </IconButton>
        )}
        {job.state.type === "completed" && (
          <>
            {isAudio ? (
              <IconButton label={playing ? "Pause preview" : "Preview"} onClick={doPreview} tinted>
                {playing ? <PauseFill size={13} /> : <PlayFill size={13} />}
              </IconButton>
            ) : (
              <IconButton label={previewLabel()} onClick={doPreview}>
                <Eye size={14} strokeWidth={2.2} />
              </IconButton>
            )}
            <IconButton label={revealLabel()} onClick={() => void reveal(job.outputPath)}>
              <Search size={13} strokeWidth={2.6} />
            </IconButton>
          </>
        )}
        {(job.state.type === "failed" || job.state.type === "cancelled") && (
          <>
            {canRetry && (
              <IconButton label="Try again" onClick={() => void api.retryJob(job.id)}>
                <RotateCw size={13} strokeWidth={2.6} />
              </IconButton>
            )}
            <IconButton label="Remove" onClick={() => void api.removeJob(job.id)}>
              <Trash size={13} strokeWidth={2.4} />
            </IconButton>
          </>
        )}
      </div>
      {menu}
    </div>
  );
}

/** The list of jobs on the Downloads and Conversions screens. */
export function QueueSection({
  kind,
  showsEmptyState,
  emptyTitle,
  emptyMessage,
  emptyIcon,
}: {
  kind: JobKind;
  showsEmptyState: boolean;
  emptyTitle: string;
  emptyMessage: string;
  emptyIcon: ReactNode;
}) {
  const allJobs = useApp((s) => s.jobs);
  const jobs = useMemo(() => allJobs.filter((j) => j.kind === kind).reverse(), [allJobs, kind]);
  const pending = jobs.filter((j) => isPending(j.state)).length;
  const anyFinished = jobs.some((j) => isFinished(j.state));

  if (jobs.length === 0) {
    return showsEmptyState ? <EmptyState icon={emptyIcon} title={emptyTitle} message={emptyMessage} /> : null;
  }
  return (
    <section className="queue">
      <SectionHeader title={pending > 0 ? "In Progress" : "Recent"} count={pending > 0 ? pending : jobs.length}>
        {pending > 1 && (
          <button type="button" className="button link" onClick={() => void api.cancelAll(kind)}>
            Cancel All
          </button>
        )}
        {anyFinished && (
          <button type="button" className="button link" onClick={() => void api.clearFinished(kind)}>
            Clear Finished
          </button>
        )}
      </SectionHeader>
      <div className="queue-list">
        {jobs.map((job) => (
          <JobRow key={job.id} job={job} />
        ))}
      </div>
    </section>
  );
}

/** Format, quality and sample-rate menus bound straight to the saved preferences. */
export function AudioOptionsControls() {
  const settings = useSettings();
  return (
    <div className="option-row">
      <OptionField title="Format">
        <Popup
          label="Format"
          value={settings.audioFormat}
          options={audioFormats.map((f) => ({ value: f, label: formatName(f) }))}
          onChange={(audioFormat) => void updateSettings({ audioFormat })}
        />
      </OptionField>
      {isLossy(settings.audioFormat) ? (
        <OptionField title="Bitrate">
          <Popup
            label="Bitrate"
            value={settings.audioBitrate}
            options={bitrates.map((b) => ({ value: b, label: bitrateName(b) }))}
            onChange={(audioBitrate) => void updateSettings({ audioBitrate })}
          />
        </OptionField>
      ) : (
        <OptionField title="Bit Depth">
          <Popup
            label="Bit depth"
            value={settings.bitDepth}
            options={bitDepths.map((d) => ({ value: d, label: bitDepthName(d) }))}
            onChange={(bitDepth) => void updateSettings({ bitDepth })}
          />
        </OptionField>
      )}
      <OptionField title="Sample Rate">
        <Popup
          label="Sample rate"
          value={settings.sampleRate}
          options={sampleRates.map((r) => ({ value: r, label: sampleRateName(r) }))}
          onChange={(sampleRate) => void updateSettings({ sampleRate })}
        />
      </OptionField>
    </div>
  );
}

/** Format and JPG quality menus for images, bound to the saved preferences. */
export function ImageOptionsControls() {
  const settings = useSettings();
  return (
    <div className="option-row">
      <OptionField title="Format">
        <Popup
          label="Image format"
          value={settings.imageFormat}
          options={imageFormats.map((f) => ({ value: f, label: imageFormatName(f) }))}
          onChange={(imageFormat) => void updateSettings({ imageFormat })}
        />
      </OptionField>
      {settings.imageFormat === "jpg" ? (
        <OptionField title="Quality">
          <Popup
            label="JPG quality"
            value={settings.imageQuality}
            options={imageQualities.map((q) => ({ value: q, label: imageQualityName(q) }))}
            onChange={(imageQuality) => void updateSettings({ imageQuality })}
          />
        </OptionField>
      ) : (
        <OptionField title="Quality">
          <span className="static">Lossless</span>
        </OptionField>
      )}
    </div>
  );
}

/** Shown when a required helper tool is missing. MediaForge points to the fix; it never installs anything itself. */
export function ToolsBanner({ only }: { only?: (tool: string) => boolean }) {
  const tools = useApp((s) => s.tools);
  if (!tools) return null;
  const missing = tools.statuses.filter((s) => !s.path && (!only || only(s.tool)));
  if (missing.length === 0) return null;
  const names = missing.map((s) => s.name);
  const list = names.length === 1 ? names[0] : `${names.slice(0, -1).join(", ")} and ${names[names.length - 1]}`;
  const hints = Array.from(new Set(missing.map((s) => s.installHint)));
  return (
    <div className="tools-banner card">
      <Wrench size={22} />
      <div className="text">
        <h3>
          {list} {missing.length === 1 ? "isn't available" : "aren't available"}
        </h3>
        <p>
          MediaForge includes these tools; this copy can't find them. Reinstall MediaForge, or choose your own copy in Settings ▸ Tools. MediaForge never
          downloads or installs software itself.
        </p>
        {hints.map((hint) => (
          <div className="command-line" key={hint}>
            <span className="mono selectable">{hint}</span>
            <button type="button" className="button plain" title="Copy command" onClick={() => void copy(hint)}>
              <Copy size={12} />
            </button>
          </div>
        ))}
      </div>
      <div className="buttons">
        <button type="button" className="button" onClick={() => void api.refreshTools().then((t) => app.set({ tools: t })).catch(showError)}>
          Check Again
        </button>
        <button type="button" className="button" onClick={() => app.set({ section: "settings" })}>
          Settings
        </button>
      </div>
    </div>
  );
}
