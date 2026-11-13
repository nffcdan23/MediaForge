// The choices offered in pickers, with their labels (from OutputOptions.swift and AppSettings.swift).

import type {
  Appearance,
  AudioBitrate,
  AudioFormat,
  AudioOptions,
  BitDepth,
  ImageFormat,
  ImageOptions,
  ImageQuality,
  OutputSpec,
  RateLimit,
  SampleRate,
  SizeLimit,
  VideoCompatibility,
  VideoQuality,
} from "../api/types";
import { computerName } from "./platform";

export const audioFormats: AudioFormat[] = ["mp3", "wav", "aac", "m4a", "flac", "aiff"];
export const bitrates: AudioBitrate[] = [128, 192, 256, 320];
export const sampleRates: SampleRate[] = [0, 44100, 48000];
export const bitDepths: BitDepth[] = [16, 24];
export const videoQualities: VideoQuality[] = [0, 2160, 1440, 1080, 720, 480, 360];
export const compatibilities: VideoCompatibility[] = ["h264", "hevc", "original"];
export const rateLimits: RateLimit[] = [0, 1, 2, 5, 10, 25];
export const sizeLimits: SizeLimit[] = [0, 500, 1000, 2000, 5000, 10000];
export const appearances: Appearance[] = ["dark", "light", "system"];
export const imageFormats: ImageFormat[] = ["jpg", "png"];
export const imageQualities: ImageQuality[] = [95, 90, 80, 70];

export const formatName = (format: AudioFormat) => format.toUpperCase();

export function isLossy(format: AudioFormat) {
  return format === "mp3" || format === "aac" || format === "m4a";
}

export function formatDetail(format: AudioFormat): string {
  switch (format) {
    case "mp3":
      return "Universal compatibility";
    case "wav":
      return "Uncompressed, edit-ready";
    case "aac":
      return "Raw AAC stream";
    case "m4a":
      return "AAC in an MPEG-4 container";
    case "flac":
      return "Lossless, compressed";
    case "aiff":
      return "Uncompressed, Apple native";
  }
}

export const imageFormatName = (format: ImageFormat) => format.toUpperCase();

export function imageFormatDetail(format: ImageFormat): string {
  return format === "jpg" ? "Opens anywhere; transparent areas become white" : "Lossless, keeps transparency";
}

export function imageQualityName(quality: ImageQuality): string {
  return quality === 95 ? "Maximum" : quality === 90 ? "High" : quality === 80 ? "Medium" : "Low";
}

export function imageSummary(options: ImageOptions): string {
  return options.format === "jpg" ? `JPG · ${imageQualityName(options.quality)} quality` : "PNG · Lossless";
}

export const bitrateName = (bitrate: AudioBitrate) => `${bitrate} kbps`;
export const bitDepthName = (depth: BitDepth) => `${depth}-bit`;

export function sampleRateName(rate: SampleRate): string {
  return rate === 0 ? "Same as source" : rate === 44100 ? "44.1 kHz" : "48 kHz";
}

export function qualityName(quality: VideoQuality): string {
  switch (quality) {
    case 0:
      return "Best available";
    case 2160:
      return "4K (2160p)";
    default:
      return `${quality}p`;
  }
}

export const qualityShortName = (quality: VideoQuality) => (quality === 0 ? "Best" : `${quality}p`);

export function compatibilityName(value: VideoCompatibility): string {
  switch (value) {
    case "h264":
      return "Convert to H.264 (most compatible)";
    case "hevc":
      return "Convert to HEVC (smaller files)";
    case "original":
      return "Keep the original format";
  }
}

export function compatibilityCodec(value: VideoCompatibility): string {
  return value === "h264" ? "H.264" : value === "hevc" ? "HEVC" : "the original format";
}

export const rateLimitName = (limit: RateLimit) => (limit === 0 ? "Unlimited" : `${limit} MB/s`);

export function sizeLimitName(limit: SizeLimit): string {
  if (limit === 0) return "No limit";
  if (limit === 500) return "500 MB";
  return `${limit / 1000} GB`;
}

export function appearanceName(value: Appearance): string {
  return value === "dark" ? "Dark" : value === "light" ? "Light" : "Match System";
}

export function audioSummary(options: AudioOptions): string {
  const parts = [formatName(options.format), isLossy(options.format) ? bitrateName(options.bitrate) : bitDepthName(options.bitDepth)];
  if (options.sampleRate !== 0) parts.push(sampleRateName(options.sampleRate));
  return parts.join(" · ");
}

export function outputSummary(output: OutputSpec): string {
  switch (output.type) {
    case "audio":
      return audioSummary(output);
    case "video":
      return `MP4 · ${qualityShortName(output.quality)}`;
    case "image":
      return imageSummary(output);
  }
}

export function outputBadge(output: OutputSpec): string {
  switch (output.type) {
    case "audio":
      return formatName(output.format);
    case "video":
      return "MP4";
    case "image":
      return imageFormatName(output.format);
  }
}

export function videoQualityOptions(maxHeight: number | null | undefined): VideoQuality[] {
  if (maxHeight == null) return [0];
  return [0, ...videoQualities.filter((q) => q !== 0 && q <= maxHeight)];
}

export function compatibilityLabel(): string {
  return `Formats ${computerName() === "Mac" ? "Macs" : "editing apps"} can't play`;
}
