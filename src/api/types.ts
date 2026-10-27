// TypeScript mirrors of the Rust types the backend sends and accepts. Field names follow the serde
// definitions in src-tauri/src (models.rs, engine/job.rs, store/*.rs, app/commands.rs).

export type Platform = "macos" | "windows" | "linux";

export type AudioFormat = "mp3" | "wav" | "aac" | "m4a" | "flac" | "aiff";
export type AudioBitrate = 128 | 192 | 256 | 320;
export type SampleRate = 0 | 44100 | 48000;
export type BitDepth = 16 | 24;
export type VideoQuality = 0 | 2160 | 1440 | 1080 | 720 | 480 | 360;
export type VideoCompatibility = "h264" | "hevc" | "original";
export type ImageFormat = "jpg" | "png";
/** JPEG quality, 1–100. */
export type ImageQuality = 95 | 90 | 80 | 70;
export type DownloadMode = "audio" | "video";
export type Appearance = "dark" | "light" | "system";
export type Section = "downloads" | "conversions" | "history" | "settings";
export type RateLimit = 0 | 1 | 2 | 5 | 10 | 25;
export type SizeLimit = 0 | 500 | 1000 | 2000 | 5000 | 10000;
export type ToolId = "yt-dlp" | "ffmpeg" | "ffprobe";

export interface AudioOptions {
  format: AudioFormat;
  bitrate: AudioBitrate;
  sampleRate: SampleRate;
  bitDepth: BitDepth;
}

export interface ImageOptions {
  format: ImageFormat;
  quality: ImageQuality;
}

export type OutputSpec = ({ type: "audio" } & AudioOptions) | { type: "video"; quality: VideoQuality } | ({ type: "image" } & ImageOptions);

export interface Settings {
  destinationPath: string;
  downloadMode: DownloadMode;
  audioFormat: AudioFormat;
  audioBitrate: AudioBitrate;
  sampleRate: SampleRate;
  bitDepth: BitDepth;
  videoQuality: VideoQuality;
  videoCompatibility: VideoCompatibility;
  imageFormat: ImageFormat;
  imageQuality: ImageQuality;
  maxConcurrentJobs: number;
  notificationsEnabled: boolean;
  embedMetadata: boolean;
  rateLimit: RateLimit;
  sizeLimit: SizeLimit;
  appearance: Appearance;
  hasAcknowledgedTerms: boolean;
  toolPaths: Record<string, string>;
  selectedSection: Section;
}

export interface ErrorPayload {
  code: string;
  title: string;
  message: string;
  retryable: boolean;
}

export type JobKind = "download" | "conversion";

export type JobState =
  | { type: "queued" }
  | { type: "preparing" }
  | { type: "downloading" }
  | { type: "processing"; stage: string }
  | { type: "converting" }
  | { type: "finalizing" }
  | { type: "completed" }
  | { type: "failed"; error: ErrorPayload }
  | { type: "cancelled" };

export interface JobProgress {
  fraction: number | null;
  bytesPerSecond: number | null;
  secondsRemaining: number | null;
  downloadedBytes: number | null;
  totalBytes: number | null;
  conversionSpeed: number | null;
}

export type JobSource = { type: "remote"; url: string; displayName: string } | { type: "file"; path: string; name: string };

export interface Job {
  id: string;
  kind: JobKind;
  source: JobSource;
  output: OutputSpec;
  title: string;
  detail: string | null;
  duration: number | null;
  thumbnailUrl: string | null;
  state: JobState;
  progress: JobProgress;
  outputPath: string | null;
  outputSize: number | null;
  createdAt: string;
  finishedAt: string | null;
  attempt: number;
  hasLog: boolean;
}

export interface HistoryItem {
  id: string;
  date: string;
  kind: JobKind;
  title: string;
  detail?: string;
  sourceUrl?: string;
  sourceFilePath?: string;
  output: OutputSpec;
  outputPath?: string;
  fileSize?: number;
  duration?: number;
  thumbnailUrl?: string;
  outcome: "completed" | "failed";
  errorTitle?: string;
  errorMessage?: string;
  fileExists: boolean;
}

export interface ToolStatus {
  tool: ToolId;
  name: string;
  purpose: string;
  path: string | null;
  version: string | null;
  bundled: boolean;
  custom: boolean;
  error: string | null;
  installHint: string;
}

export interface ToolsReport {
  statuses: ToolStatus[];
  missing: ToolId[];
  canDownload: boolean;
  canConvert: boolean;
  allBundled: boolean;
}

export interface Bootstrap {
  platform: Platform;
  version: string;
  build: string;
  isDebug: boolean;
  settings: Settings;
  defaultDestination: string;
  destination: string;
  jobs: Job[];
  history: HistoryItem[];
  tools: ToolsReport;
  hasThirdPartyNotices: boolean;
  pendingSection: Section | null;
}

export interface LinkSummary {
  url: string;
  displayName: string;
}

export interface LinkInspection {
  urls: LinkSummary[];
  rejected: number;
  truncated: boolean;
  invalidMessage: string | null;
}

export interface PlaylistEntry {
  id: string;
  title: string;
  url: string;
  duration: number | null;
}

export interface Resolution {
  isPlaylist: boolean;
  url: string;
  title: string;
  uploader: string | null;
  duration: number | null;
  thumbnailUrl: string | null;
  sourceName: string | null;
  hasVideo: boolean;
  maxVideoHeight: number | null;
  qualityOptions: VideoQuality[];
  conversionExpected: Record<string, boolean>;
  entries: PlaylistEntry[];
  /** Set when the link is a picture rather than audio or video. */
  image: ImageDetails | null;
}

export interface ImageDetails {
  /** "WebP", "PNG" and so on. */
  format: string;
  width: number | null;
  height: number | null;
  size: number | null;
}

export interface DownloadItem {
  url: string;
  title?: string | null;
  detail?: string | null;
  duration?: number | null;
  thumbnailUrl?: string | null;
}

export type ThumbnailRequest = { kind: "remote"; url: string } | { kind: "file"; path: string };

export interface PreviewSource {
  path: string;
  transcoded: boolean;
}

export interface Alert {
  title: string;
  message: string;
}
