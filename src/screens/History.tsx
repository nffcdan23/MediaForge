// The History screen (HistoryView.swift). Its search field and filter live in the toolbar (see App.tsx).

import { CircleArrowDown, Copy, Eye, Link, RefreshCw, RotateCw, Search, Trash, TriangleAlert } from "lucide-react";
import { useMemo } from "react";
import { api } from "../api/commands";
import type { HistoryItem } from "../api/types";
import { EmptyState, FormatBadge, IconButton, PageHeader, Thumbnail } from "../components/Controls";
import { HistoryIcon, PauseFill, PlayFill } from "../components/Icons";
import { OutputIcon } from "../components/Jobs";
import { tidy, useContextMenu } from "../components/Menu";
import { bytes, dayTitle, duration, fileName, time } from "../lib/format";
import { outputBadge, outputSummary } from "../lib/options";
import { computerName, previewLabel, revealLabel } from "../lib/platform";
import { copy, preview, repeatHistory, reveal, showError } from "../state/actions";
import { usePlayer } from "../state/player";
import { Store, useApp, useStore } from "../state/store";

export type HistoryFilter = "all" | "audio" | "video" | "image" | "failed";

export const historyView = new Store<{ query: string; filter: HistoryFilter }>({ query: "", filter: "all" });

function matches(filter: HistoryFilter, entry: HistoryItem) {
  switch (filter) {
    case "all":
      return true;
    case "audio":
      return entry.output.type === "audio" && entry.outcome === "completed";
    case "video":
    case "image":
      return entry.output.type === filter && entry.outcome === "completed";
    case "failed":
      return entry.outcome === "failed";
  }
}

export function HistoryScreen() {
  const history = useApp((s) => s.history);
  const { query, filter } = useStore(historyView, (s) => s);
  const groups = useMemo(() => {
    const needle = query.trim().toLowerCase();
    const filtered = history.filter((entry) => {
      if (!matches(filter, entry)) return false;
      if (!needle) return true;
      return [entry.title, entry.detail, entry.sourceUrl, entry.sourceFilePath, outputSummary(entry.output)]
        .filter((v): v is string => !!v)
        .some((v) => v.toLowerCase().includes(needle));
    });
    const byDay = new Map<string, { title: string; entries: HistoryItem[] }>();
    for (const entry of filtered) {
      const date = new Date(entry.date);
      const key = `${date.getFullYear()}-${date.getMonth()}-${date.getDate()}`;
      if (!byDay.has(key)) byDay.set(key, { title: dayTitle(date), entries: [] });
      byDay.get(key)!.entries.push(entry);
    }
    return [...byDay.entries()].map(([key, group]) => ({ key, ...group }));
  }, [history, query, filter]);

  return (
    <>
      <PageHeader title="History" subtitle={`Everything MediaForge has downloaded or converted on this ${computerName()}.`} />
      {history.length === 0 ? (
        <EmptyState icon={<HistoryIcon size={40} strokeWidth={1.4} />} title="No history yet" message="Finished downloads and conversions appear here." />
      ) : groups.length === 0 ? (
        <EmptyState icon={<Search size={40} strokeWidth={1.4} />} title={query ? `No Results for “${query}”` : "No Results"} message="Check the spelling or try a new search." />
      ) : (
        <div className="history-list">
          {groups.map((group) => (
            <div key={group.key} className="history-list">
              <div className="history-day">{group.title}</div>
              {group.entries.map((entry) => (
                <HistoryRow key={entry.id} entry={entry} />
              ))}
            </div>
          ))}
        </div>
      )}
    </>
  );
}

function origin(entry: HistoryItem): string | null {
  if (entry.sourceUrl) {
    try {
      const host = new URL(entry.sourceUrl).hostname;
      return host.startsWith("www.") ? host.slice(4) : host;
    } catch {
      return null;
    }
  }
  return entry.sourceFilePath ? fileName(entry.sourceFilePath) : null;
}

function HistoryRow({ entry }: { entry: HistoryItem }) {
  const isAudio = entry.output.type === "audio";
  const playable = entry.outcome === "completed" && entry.fileExists;
  const playing = usePlayer((p) => !!entry.outputPath && p.item?.path === entry.outputPath && p.isPlaying);
  const source = entry.thumbnailUrl
    ? { kind: "remote" as const, url: entry.thumbnailUrl }
    : entry.outputPath && entry.fileExists
      ? { kind: "file" as const, path: entry.outputPath }
      : null;

  const subtitle = (() => {
    if (entry.outcome === "failed") return entry.errorMessage ?? "The job didn't complete.";
    const parts = [outputSummary(entry.output)];
    if (entry.fileSize != null) parts.push(bytes(entry.fileSize));
    if (entry.duration != null) parts.push(duration(entry.duration));
    if (!entry.fileExists) parts.push("File moved or deleted");
    return parts.join(" · ");
  })();

  const doPreview = () => preview(entry.outputPath, entry.title, entry.detail ?? outputSummary(entry.output), entry.thumbnailUrl ?? null, entry.output.type);
  const again = entry.kind === "download" ? "Download Again" : "Convert Again";
  const { onContextMenu, menu } = useContextMenu(() =>
    tidy([
      playable && { label: isAudio ? "Preview" : previewLabel(), icon: <Eye size={14} />, onSelect: doPreview },
      playable && { label: revealLabel(), icon: <Search size={14} />, onSelect: () => void reveal(entry.outputPath) },
      playable && "separator",
      { label: again, icon: <RotateCw size={14} />, onSelect: () => void repeatHistory(entry.id) },
      entry.sourceUrl ? { label: "Copy Link", icon: <Link size={14} />, onSelect: () => void copy(entry.sourceUrl!) } : null,
      entry.outputPath ? { label: "Copy File Path", icon: <Copy size={14} />, onSelect: () => void copy(entry.outputPath!) } : null,
      "separator",
      { label: "Remove from History", icon: <Trash size={14} />, destructive: true, onSelect: () => void api.removeHistory([entry.id]).catch(showError) },
    ]),
  );
  const from = origin(entry);

  return (
    <div className="history-row card" onContextMenu={onContextMenu}>
      <Thumbnail
        className={entry.fileExists || entry.outcome === "failed" ? "" : "faded"}
        source={source}
        icon={<OutputIcon output={entry.output} size={18} />}
      />
      <div className="body">
        <div className="title-line">
          <span className="title" title={entry.title}>
            {entry.title}
          </span>
          <FormatBadge text={outputBadge(entry.output)} />
          {entry.outcome === "failed" && (
            <span className="failure">
              <TriangleAlert size={11} strokeWidth={2.4} />
              {entry.errorTitle ?? "Failed"}
            </span>
          )}
        </div>
        <div className={`subtitle${entry.outcome === "failed" ? " multiline" : ""}`}>{subtitle}</div>
        <div className="meta">
          {entry.kind === "download" ? <CircleArrowDown size={11} /> : <RefreshCw size={11} />}
          <span>{time(new Date(entry.date))}</span>
          {from && (
            <>
              <span>·</span>
              <span>{from}</span>
            </>
          )}
        </div>
      </div>
      <div className="row-actions">
        {playable ? (
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
            <IconButton label={revealLabel()} onClick={() => void reveal(entry.outputPath)}>
              <Search size={13} strokeWidth={2.6} />
            </IconButton>
          </>
        ) : (
          <IconButton label={entry.kind === "download" ? "Download again" : "Convert again"} onClick={() => void repeatHistory(entry.id)}>
            <RotateCw size={13} strokeWidth={2.6} />
          </IconButton>
        )}
      </div>
      {menu}
    </div>
  );
}
