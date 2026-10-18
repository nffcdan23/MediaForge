// The Downloads screen (DownloadsView.swift).

import {
  AudioWaveform,
  CircleArrowDown,
  ClipboardPaste,
  Globe,
  Info,
  Layers,
  Link as LinkIcon,
  ListVideo,
  Lock,
  Radio,
  RefreshCw,
  RotateCw,
  TriangleAlert,
  WifiOff,
  Wrench,
} from "lucide-react";
import { useEffect, useRef } from "react";
import { api } from "../api/commands";
import type { ErrorPayload, Resolution } from "../api/types";
import { FormatBadge, OptionField, PageHeader, Popup, ProgressBar, Segmented, Spinner, Thumbnail } from "../components/Controls";
import { ClearFill, PlayFill } from "../components/Icons";
import { AudioOptionsControls, QueueSection, ToolsBanner } from "../components/Jobs";
import { duration as formatDuration, fileName } from "../lib/format";
import { compatibilityCodec, outputSummary, qualityName } from "../lib/options";
import { computerName, shortcut } from "../lib/platform";
import { navigate, pasteLink, showError, updateSettings } from "../state/actions";
import {
  conversionNoteApplies,
  downloads,
  effectiveMode,
  effectiveQuality,
  offersVideo,
  outputFor,
  qualityOptions,
  useDownloads,
  type Detection,
} from "../state/downloads";
import { useApp, useSettings } from "../state/store";

export function DownloadsScreen() {
  const detection = useDownloads((d) => d.detection);
  const settings = useSettings();
  const hasDetection = detection.type !== "idle" && detection.type !== "invalid";
  return (
    <>
      <PageHeader title="Download media" subtitle="Paste a link to save its audio or video in the format you need." />
      <ToolsBanner />
      <UrlInputCard />
      <DetectionView detection={detection} />
      <QueueSection
        kind="download"
        showsEmptyState={!hasDetection}
        emptyTitle="No downloads yet"
        emptyMessage={`Finished files are saved to ${fileName(settings.destinationPath)} and listed in History.`}
        emptyIcon={<CircleArrowDown size={40} strokeWidth={1.4} />}
      />
    </>
  );
}

/** The prominent link field at the top of the Downloads screen. */
function UrlInputCard() {
  const text = useDownloads((d) => d.text);
  const detection = useDownloads((d) => d.detection);
  const field = useRef<HTMLTextAreaElement>(null);

  useEffect(() => {
    field.current?.focus();
  }, []);

  // Grows to fit up to four lines, as the SwiftUI field did.
  useEffect(() => {
    const el = field.current;
    if (!el) return;
    el.style.height = "0px";
    el.style.height = `${Math.min(el.scrollHeight, 88)}px`;
  }, [text]);

  return (
    <div>
      <div className="url-input glass">
        <LinkIcon size={19} strokeWidth={2.4} color="url(#brand-gradient)" />
        <textarea
          ref={field}
          rows={1}
          value={text}
          placeholder="Paste a link — or several, one per line"
          aria-label="Media link"
          spellCheck={false}
          autoCorrect="off"
          autoCapitalize="off"
          onChange={(event) => downloads.setText(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === "Enter" && !event.shiftKey) {
              event.preventDefault();
              downloads.submit();
            }
          }}
        />
        {detection.type === "resolving" && <Spinner />}
        {text && (
          <button
            type="button"
            className="clear"
            title="Clear"
            aria-label="Clear"
            onClick={() => {
              downloads.clear();
              field.current?.focus();
            }}
          >
            <ClearFill />
          </button>
        )}
        <button type="button" className="button" title={`Paste a link from the clipboard (${shortcut("v", true)})`} onClick={() => void pasteLink()}>
          <ClipboardPaste size={14} />
          Paste
        </button>
      </div>
      {detection.type === "invalid" ? (
        <div className="footnote warning">
          <TriangleAlert size={12} strokeWidth={2.4} />
          {detection.message}
        </div>
      ) : (
        <div className="footnote">Only download media you own, have licensed, or that the source permits you to download. DRM and sign-in walls are never bypassed.</div>
      )}
    </div>
  );
}

function DetectionView({ detection }: { detection: Detection }) {
  switch (detection.type) {
    case "idle":
    case "invalid":
      return null;
    case "resolving":
      return <ResolvingCard key="resolving" name={detection.url.displayName} />;
    case "media":
      return <MediaPreviewCard key={detection.url.url} resolution={detection.resolution} />;
    case "playlist":
      return <PlaylistCard key={detection.url.url} resolution={detection.resolution} />;
    case "batch":
      return <BatchCard urls={detection.urls.map((u) => u.displayName)} rejected={detection.rejected} truncated={detection.truncated} />;
    case "failed":
      return <ErrorCard error={detection.error} />;
  }
}

function ResolvingCard({ name }: { name: string }) {
  return (
    <div className="resolving-card card large detection">
      <div className="placeholder">
        <Spinner large />
      </div>
      <div>
        <h3>Looking up media…</h3>
        <div className="secondary" style={{ fontSize: 12, wordBreak: "break-all" }}>
          {name}
        </div>
        <div style={{ width: 260, marginTop: 14 }}>
          <ProgressBar fraction={null} />
        </div>
      </div>
    </div>
  );
}

/** Large artwork, details and output options for a single resolved link. */
function MediaPreviewCard({ resolution }: { resolution: Resolution }) {
  const settings = useSettings();
  const thumbnail = resolution.thumbnailUrl ? { kind: "remote" as const, url: resolution.thumbnailUrl } : null;
  return (
    <div className="media-card card large detection">
      {resolution.thumbnailUrl && <WashBackground url={resolution.thumbnailUrl} />}
      <Thumbnail
        className="artwork"
        source={thumbnail}
        icon={resolution.hasVideo ? <PlayBadge /> : <AudioWaveform size={44} color="url(#brand-gradient)" />}
        duration={resolution.duration}
      />
      <div className="info">
        <div className="badges">
          {resolution.sourceName && <FormatBadge text={resolution.sourceName.toUpperCase()} />}
          {!resolution.hasVideo ? (
            <FormatBadge text="AUDIO ONLY" />
          ) : resolution.maxVideoHeight != null ? (
            <FormatBadge text={`UP TO ${resolution.maxVideoHeight}P`} />
          ) : null}
        </div>
        <h2 className="selectable" title={resolution.title}>
          {resolution.title}
        </h2>
        {resolution.uploader && <div className="uploader">{resolution.uploader}</div>}
        <div className="grow" />
        <OutputOptions resolution={resolution} />
        {conversionNoteApplies(settings, resolution) && (
          <div className="notice">
            <RefreshCw size={12} />
            <span>
              At this quality the source isn't in a format {computerName() === "Mac" ? "Macs" : "most players and editing apps"} can play, so MediaForge
              will convert it to {compatibilityCodec(settings.videoCompatibility)} after downloading. That takes about as long as the video runs.
            </span>
          </div>
        )}
        <DownloadButton title="Download" count={1} resolution={resolution} />
      </div>
    </div>
  );
}

function PlayBadge() {
  return (
    <span style={{ width: 52, height: 40, borderRadius: 7, background: "var(--brand-diagonal)", display: "grid", placeItems: "center", color: "#1b1a22" }}>
      <PlayFill size={20} />
    </span>
  );
}

/** A soft wash of the artwork behind the card, the way Music and TV tint their detail views. */
function WashBackground({ url }: { url: string }) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    let live = true;
    void api.thumbnail({ kind: "remote", url }).then((data) => {
      if (live && data && ref.current) ref.current.style.backgroundImage = `url("${data}")`;
    });
    return () => {
      live = false;
    };
  }, [url]);
  return <div className="wash" ref={ref} />;
}

function PlaylistCard({ resolution }: { resolution: Resolution }) {
  const entries = resolution.entries;
  return (
    <div className="list-card card large detection">
      <div className="list-card-header">
        <ListVideo size={26} color="url(#brand-gradient)" />
        <div style={{ minWidth: 0 }}>
          <h3>{resolution.title}</h3>
          <p>
            {entries.length} item{entries.length === 1 ? "" : "s"} in this collection
          </p>
        </div>
      </div>
      <div className="playlist-rows">
        {entries.slice(0, 6).map((entry, index) => (
          <div className="playlist-row" key={entry.id}>
            <span className="index">{index + 1}</span>
            <span className="name">{entry.title}</span>
            {entry.duration != null && <span className="time">{formatDuration(entry.duration)}</span>}
          </div>
        ))}
        {entries.length > 6 && <div className="more">and {entries.length - 6} more</div>}
      </div>
      <OutputOptions resolution={null} />
      <DownloadButton title="Download All" count={entries.length} resolution={null} />
    </div>
  );
}

function BatchCard({ urls, rejected, truncated }: { urls: string[]; rejected: number; truncated: boolean }) {
  return (
    <div className="list-card card large detection">
      <div className="list-card-header">
        <Layers size={26} color="url(#brand-gradient)" />
        <div>
          <h3>{urls.length} links ready</h3>
          <p>Each link is looked up as its download starts.</p>
        </div>
      </div>
      <div className="batch-links">
        {urls.slice(0, 5).map((url) => (
          <div key={url}>
            <LinkIcon size={12} />
            {url}
          </div>
        ))}
        {urls.length > 5 && <div className="tertiary">and {urls.length - 5} more</div>}
      </div>
      {rejected > 0 && (
        <div className="notice warning">
          <TriangleAlert size={12} />
          {rejected} item{rejected === 1 ? " isn't a valid link and" : "s aren't valid links and"} will be skipped.
        </div>
      )}
      {truncated && (
        <div className="notice">
          <Info size={12} />
          Only the first 200 links are used.
        </div>
      )}
      <OutputOptions resolution={null} />
      <DownloadButton title="Download All" count={urls.length} resolution={null} />
    </div>
  );
}

function errorIcon(error: ErrorPayload) {
  switch (error.code) {
    case "drmProtected":
    case "authenticationRequired":
    case "accessDenied":
      return <Lock size={28} />;
    case "geoRestricted":
      return <Globe size={28} />;
    case "network":
    case "timedOut":
      return <WifiOff size={28} />;
    case "toolMissing":
      return <Wrench size={28} />;
    case "liveStream":
      return <Radio size={28} />;
    default:
      return <TriangleAlert size={28} />;
  }
}

function ErrorCard({ error }: { error: ErrorPayload }) {
  return (
    <div className="error-card card large detection">
      {errorIcon(error)}
      <div>
        <h3>{error.title}</h3>
        <p className="selectable">{error.message}</p>
        <div className="actions">
          {error.retryable && (
            <button type="button" className="button" onClick={() => downloads.retry()}>
              <RotateCw size={13} />
              Try Again
            </button>
          )}
          {error.code === "toolMissing" && (
            <button type="button" className="button" onClick={() => navigate("settings")}>
              Open Settings
            </button>
          )}
          <button type="button" className="button" onClick={() => downloads.clear()}>
            Clear
          </button>
        </div>
      </div>
    </div>
  );
}

/** Audio/Video switch plus the pickers that apply to the chosen mode. */
function OutputOptions({ resolution }: { resolution: Resolution | null }) {
  const settings = useSettings();
  const mode = effectiveMode(settings, resolution);
  return (
    <div className="output-options">
      {offersVideo(resolution) && (
        <Segmented
          label="Output"
          value={settings.downloadMode}
          onChange={(downloadMode) => void updateSettings({ downloadMode })}
          options={[
            { value: "audio", label: "Audio" },
            { value: "video", label: "Video" },
          ]}
        />
      )}
      {mode === "audio" ? (
        <AudioOptionsControls />
      ) : (
        <div className="option-row">
          <OptionField title="Format">
            <span className="static">MP4</span>
          </OptionField>
          <OptionField title="Quality">
            <Popup
              label="Quality"
              value={effectiveQuality(settings, resolution)}
              options={qualityOptions(resolution).map((q) => ({ value: q, label: qualityName(q) }))}
              onChange={(videoQuality) => void updateSettings({ videoQuality })}
            />
          </OptionField>
        </div>
      )}
    </div>
  );
}

function DownloadButton({ title, count, resolution }: { title: string; count: number; resolution: Resolution | null }) {
  const settings = useSettings();
  const tools = useApp((s) => s.tools);
  const canDownload = tools?.canDownload ?? false;
  const pending = useDownloads((d) => d.detection.type);
  const ready = pending === "media" || pending === "playlist" || pending === "batch";
  return (
    <div className="download-button-row">
      <button
        type="button"
        className="button large prominent"
        disabled={!canDownload || !ready}
        onClick={() => void downloads.enqueue().catch(showError)}
      >
        <CircleArrowDown size={16} strokeWidth={2.4} />
        {count > 1 ? `${title} (${count})` : title}
      </button>
      <div className="summary">
        <strong>{outputSummary(outputFor(settings, resolution))}</strong>
        <span>{canDownload ? `Saves to ${fileName(settings.destinationPath)}` : "Install the missing tools to continue"}</span>
      </div>
    </div>
  );
}
