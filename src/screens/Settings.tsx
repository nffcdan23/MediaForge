// The Settings screen (SettingsView.swift): a grouped form.

import { CircleCheck, CircleEllipsis, Copy, FolderOpen, RotateCw, SquareArrowOutUpRight, TriangleAlert } from "lucide-react";
import type { ReactNode } from "react";
import { api } from "../api/commands";
import type { ToolStatus } from "../api/types";
import { Popup, Segmented, Toggle } from "../components/Controls";
import { tidy, useContextMenu } from "../components/Menu";
import {
  appearanceName,
  appearances,
  audioFormats,
  bitDepthName,
  bitDepths,
  bitrateName,
  bitrates,
  compatibilities,
  compatibilityLabel,
  compatibilityName,
  formatDetail,
  formatName,
  imageFormatDetail,
  imageFormatName,
  imageFormats,
  imageQualities,
  imageQualityName,
  qualityName,
  rateLimitName,
  rateLimits,
  sampleRateName,
  sampleRates,
  sizeLimitName,
  sizeLimits,
  videoQualities,
} from "../lib/options";
import { computerName, fileManagerName, isMac } from "../lib/platform";
import { chooseDestination, copy, openDestination, reveal, showError, updateSettings } from "../state/actions";
import { app, useApp, useSettings } from "../state/store";

function Section({ title, footer, children }: { title: string; footer?: ReactNode; children: ReactNode }) {
  return (
    <section className="settings-section">
      <h2>{title}</h2>
      <div className="settings-group">{children}</div>
      {footer && <p className="settings-footer">{footer}</p>}
    </section>
  );
}

function Row({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="settings-row">
      <span className="label">{label}</span>
      <span className="value">{children}</span>
    </div>
  );
}

export function SettingsScreen() {
  const settings = useSettings();
  const tools = useApp((s) => s.tools);
  const hasNotices = useApp((s) => s.hasNotices);
  const version = useApp((s) => (s.build ? `${s.version} (${s.build})` : s.version));
  const allBundled = tools?.allBundled ?? false;

  return (
    <div className="settings">
      <Section title="General" footer="Files are written to a private temporary folder and only moved here once complete. Existing files are never overwritten.">
        <div className="settings-row stacked">
          <span className="label">Save files to</span>
          <div className="destination">
            <FolderOpen size={15} className="folder" />
            <span className="path" title={settings.destinationPath}>
              &lrm;{settings.destinationPath}&lrm;
            </span>
            <span style={{ flex: 1 }} />
            <button type="button" className="button" onClick={() => void chooseDestination()}>
              Choose…
            </button>
            <button type="button" className="button" title={`Open in ${fileManagerName()}`} aria-label={`Open in ${fileManagerName()}`} onClick={() => void openDestination()}>
              <SquareArrowOutUpRight size={13} />
            </button>
          </div>
        </div>
        <Row label="Simultaneous jobs">
          <Popup
            borderless
            label="Simultaneous jobs"
            value={settings.maxConcurrentJobs}
            options={[1, 2, 3, 4, 5, 6].map((n) => ({ value: n, label: String(n) }))}
            onChange={(maxConcurrentJobs) => void updateSettings({ maxConcurrentJobs })}
          />
        </Row>
        <Row label="Notify when a job finishes">
          <Toggle label="Notify when a job finishes" on={settings.notificationsEnabled} onChange={(notificationsEnabled) => void updateSettings({ notificationsEnabled })} />
        </Row>
        <Row label="Tag files with title, creator and source link">
          <Toggle label="Tag files" on={settings.embedMetadata} onChange={(embedMetadata) => void updateSettings({ embedMetadata })} />
        </Row>
      </Section>

      <Section title="Audio" footer="Downloads always fetch the best audio the source offers. When it's already in the format you chose, the stream is copied without re-encoding.">
        <Row label="Format">
          <Popup
            borderless
            label="Format"
            value={settings.audioFormat}
            options={audioFormats.map((f) => ({ value: f, label: `${formatName(f)} — ${formatDetail(f)}` }))}
            onChange={(audioFormat) => void updateSettings({ audioFormat })}
          />
        </Row>
        <Row label="Bitrate (MP3, AAC, M4A)">
          <Popup borderless label="Bitrate" value={settings.audioBitrate} options={bitrates.map((b) => ({ value: b, label: bitrateName(b) }))} onChange={(audioBitrate) => void updateSettings({ audioBitrate })} />
        </Row>
        <Row label="Bit depth (WAV, FLAC, AIFF)">
          <Popup borderless label="Bit depth" value={settings.bitDepth} options={bitDepths.map((d) => ({ value: d, label: bitDepthName(d) }))} onChange={(bitDepth) => void updateSettings({ bitDepth })} />
        </Row>
        <Row label="Sample rate">
          <Popup borderless label="Sample rate" value={settings.sampleRate} options={sampleRates.map((r) => ({ value: r, label: sampleRateName(r) }))} onChange={(sampleRate) => void updateSettings({ sampleRate })} />
        </Row>
      </Section>

      <Section
        title="Video"
        footer={`Video is saved as MP4. Above 1080p, many sources only offer VP9 or AV1, which ${isMac() ? "QuickTime" : "Windows' Media Player"} and most editing apps can't open. MediaForge converts those with your ${computerName()}'s built-in video encoder, which takes about as long as the video runs. Video already in H.264 or HEVC is never re-encoded.`}
      >
        <Row label="Preferred quality">
          <Popup borderless label="Preferred quality" value={settings.videoQuality} options={videoQualities.map((q) => ({ value: q, label: qualityName(q) }))} onChange={(videoQuality) => void updateSettings({ videoQuality })} />
        </Row>
        <Row label={compatibilityLabel()}>
          <Popup
            borderless
            label={compatibilityLabel()}
            value={settings.videoCompatibility}
            options={compatibilities.map((c) => ({ value: c, label: compatibilityName(c) }))}
            onChange={(videoCompatibility) => void updateSettings({ videoCompatibility })}
          />
        </Row>
      </Section>

      <Section
        title="Images"
        footer="Used for picture links and for WebP, PNG, JPEG, GIF, BMP and TIFF files you convert. JPG has no transparency, so transparent areas become white. A picture already in the format you chose is kept exactly as it is, and the first frame of an animation is used."
      >
        <Row label="Format">
          <Popup
            borderless
            label="Image format"
            value={settings.imageFormat}
            options={imageFormats.map((f) => ({ value: f, label: `${imageFormatName(f)} — ${imageFormatDetail(f)}` }))}
            onChange={(imageFormat) => void updateSettings({ imageFormat })}
          />
        </Row>
        <Row label="JPG quality">
          <Popup
            borderless
            label="JPG quality"
            value={settings.imageQuality}
            options={imageQualities.map((q) => ({ value: q, label: `${imageQualityName(q)} (${q}%)` }))}
            onChange={(imageQuality) => void updateSettings({ imageQuality })}
          />
        </Row>
      </Section>

      <Section title="Limits">
        <Row label="Download speed limit">
          <Popup borderless label="Download speed limit" value={settings.rateLimit} options={rateLimits.map((r) => ({ value: r, label: rateLimitName(r) }))} onChange={(rateLimit) => void updateSettings({ rateLimit })} />
        </Row>
        <Row label="Largest file to download">
          <Popup borderless label="Largest file to download" value={settings.sizeLimit} options={sizeLimits.map((s) => ({ value: s, label: sizeLimitName(s) }))} onChange={(sizeLimit) => void updateSettings({ sizeLimit })} />
        </Row>
      </Section>

      <Section title="Appearance">
        <Row label="Theme">
          <Segmented label="Theme" value={settings.appearance} options={appearances.map((a) => ({ value: a, label: appearanceName(a) }))} onChange={(appearance) => void updateSettings({ appearance })} />
        </Row>
      </Section>

      <Section
        title="Tools"
        footer={
          allBundled
            ? "These tools are included with MediaForge. You can point MediaForge at your own copy of any of them instead."
            : "MediaForge looks for these tools inside itself and in standard install locations only, and never downloads or installs software itself."
        }
      >
        {tools?.statuses.map((status) => <ToolRow key={status.tool} status={status} />)}
        {!allBundled && (
          <Row label="Installed something new?">
            <button type="button" className="button" onClick={() => void api.refreshTools().then((t) => app.set({ tools: t })).catch(showError)}>
              <RotateCw size={12} />
              Check Again
            </button>
          </Row>
        )}
      </Section>

      <Section title="Privacy & About">
        <Row label="Tracking, telemetry and ads">None</Row>
        <Row label="DRM, sign-in and region bypass">Never</Row>
        <Row label="Responsible use">
          <button type="button" className="button" onClick={() => app.set({ showingTerms: true })}>
            Review…
          </button>
        </Row>
        {hasNotices && (
          <Row label="Third-party software">
            <button type="button" className="button" onClick={() => void api.openThirdPartyNotices().catch(showError)}>
              View Licences…
            </button>
          </Row>
        )}
        <Row label="Version">{version}</Row>
      </Section>
    </div>
  );
}

/** Status, version and optional custom location for one helper tool. */
function ToolRow({ status }: { status: ToolStatus }) {
  const { onContextMenu, menu } = useContextMenu(() =>
    tidy([
      { label: "Choose Location…", onSelect: () => void api.chooseToolPath(status.tool).then((s) => s && app.set({ settings: s })).catch(showError) },
      status.custom && { label: "Find Automatically", onSelect: () => void api.resetToolPath(status.tool).then((s) => app.set({ settings: s })).catch(showError) },
      !!status.path && "separator",
      !!status.path && { label: `Show in ${fileManagerName()}`, onSelect: () => void reveal(status.path) },
    ]),
  );
  return (
    <div className="tool-row">
      <div className="head">
        {status.path ? <CircleCheck size={20} className="ok" /> : <TriangleAlert size={20} className="bad" />}
        <div style={{ flex: 1, minWidth: 0 }}>
          <div>
            <span className="name">{status.name}</span>
            {status.version && <span className="version">{status.version}</span>}
          </div>
          <div className="purpose">{status.purpose}</div>
        </div>
        <button type="button" className="button plain" title="Tool location options" aria-label="Tool location options" onClick={onContextMenu}>
          <CircleEllipsis size={16} />
        </button>
        {menu}
      </div>
      {status.path ? (
        status.bundled ? (
          <div className="where">Included with MediaForge</div>
        ) : (
          <div className="where mono selectable">
            {status.path}
            {status.custom ? "  (custom)" : ""}
          </div>
        )
      ) : (
        <div className="where hint">
          <span>{status.error === "Not installed" ? "Not found. Reinstall MediaForge, or install it yourself:" : status.error}</span>
          <span className="mono selectable">{status.installHint}</span>
          <button type="button" className="button plain" title="Copy command" onClick={() => void copy(status.installHint)}>
            <Copy size={12} />
          </button>
        </div>
      )}
    </div>
  );
}
