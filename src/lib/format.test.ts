import { describe, expect, it } from "vitest";
import { bytes, conversionSpeed, dayTitle, duration, fileName, percent, remaining, speed } from "./format";
import { expectsConversionNote, jobStatusText } from "./status";
import type { Job } from "../api/types";

describe("formatting", () => {
  it("formats durations", () => {
    expect(duration(0)).toBe("0:00");
    expect(duration(187)).toBe("3:07");
    expect(duration(3765)).toBe("1:02:45");
    expect(duration(Number.NaN)).toBe("–:––");
  });

  it("formats time remaining", () => {
    expect(remaining(0.2)).toBe("1s left");
    expect(remaining(42)).toBe("42s left");
    expect(remaining(240)).toBe("4 min left");
    expect(remaining(3900)).toBe("1 hr 5 min left");
    expect(remaining(Number.POSITIVE_INFINITY)).toBe("");
  });

  it("formats percentages, speeds and sizes", () => {
    expect(percent(0.426)).toBe("42%");
    expect(percent(7)).toBe("100%");
    expect(percent(-1)).toBe("0%");
    expect(speed(0)).toBe("—");
    expect(speed(2_500_000)).toBe("2.5 MB/s");
    expect(conversionSpeed(42.4)).toBe("42×");
    expect(conversionSpeed(3.14)).toBe("3.1×");
    expect(bytes(203_400)).toBe("203 kB");
    expect(bytes(4_400_000)).toBe("4.4 MB");
    expect(bytes(1_000_000)).toBe("1 MB");
    expect(bytes(216_049_000)).toBe("216 MB");
  });

  it("groups history by day", () => {
    const now = new Date(2026, 9, 18, 15, 0);
    expect(dayTitle(new Date(2026, 9, 18, 1, 0), now)).toBe("Today");
    expect(dayTitle(new Date(2026, 9, 17, 23, 0), now)).toBe("Yesterday");
    expect(dayTitle(new Date(2026, 9, 10), now)).toContain("2026");
  });

  it("takes file names from either kind of path", () => {
    expect(fileName("/Users/a/Music/Theme.mp3")).toBe("Theme.mp3");
    expect(fileName("C:\\Users\\a\\Music\\Theme.mp3")).toBe("Theme.mp3");
  });
});

function job(partial: Partial<Job>): Job {
  return {
    id: "1",
    kind: "download",
    source: { type: "remote", url: "https://example.com", displayName: "example.com" },
    output: { type: "audio", format: "mp3", bitrate: 320, sampleRate: 0, bitDepth: 16 },
    title: "Clip",
    detail: null,
    duration: null,
    thumbnailUrl: null,
    state: { type: "queued" },
    progress: { fraction: null, bytesPerSecond: null, secondsRemaining: null, downloadedBytes: null, totalBytes: null, conversionSpeed: null },
    outputPath: null,
    outputSize: null,
    createdAt: new Date().toISOString(),
    finishedAt: null,
    attempt: 1,
    hasLog: false,
    ...partial,
  };
}

describe("job status lines", () => {
  it("describes each state the way the Mac app did", () => {
    expect(jobStatusText(job({}))).toBe("Waiting to start");
    expect(jobStatusText(job({ state: { type: "preparing" } }))).toBe("Contacting source…");
    expect(jobStatusText(job({ kind: "conversion", state: { type: "preparing" } }))).toBe("Preparing…");
    expect(
      jobStatusText(
        job({
          state: { type: "downloading" },
          progress: { fraction: 0.23, downloadedBytes: 1_000_000, totalBytes: 4_400_000, bytesPerSecond: 985_000, secondsRemaining: 3, conversionSpeed: null },
        }),
      ),
    ).toBe("Downloading · 23% · 1 MB of 4.4 MB · 985 kB/s · 3s left");
    expect(jobStatusText(job({ state: { type: "processing", stage: "Merging video and audio" } }))).toBe("Merging video and audio…");
    expect(
      jobStatusText(job({ state: { type: "converting" }, progress: { fraction: 0.5, conversionSpeed: 42, secondsRemaining: 12, bytesPerSecond: null, downloadedBytes: null, totalBytes: null } })),
    ).toBe("Converting to MP3 · 50% · 42× · 12s left");
    expect(jobStatusText(job({ state: { type: "completed" }, outputSize: 216_049_000, duration: 5400 }))).toBe("MP3 · 320 kbps · 216 MB · 1:30:00");
    expect(jobStatusText(job({ state: { type: "cancelled" } }))).toBe("Cancelled");
  });

  it("only warns about conversion when it will happen", () => {
    const resolution = { conversionExpected: { "0": true, "2160": true, "1080": false } };
    expect(expectsConversionNote("video", "h264", resolution, 2160)).toBe(true);
    expect(expectsConversionNote("video", "h264", resolution, 1080)).toBe(false);
    expect(expectsConversionNote("video", "original", resolution, 2160)).toBe(false);
    expect(expectsConversionNote("audio", "h264", resolution, 2160)).toBe(false);
  });
});
