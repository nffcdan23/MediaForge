// In-app playback of finished audio files (AudioPreviewPlayer.swift), with the platform's web audio
// engine. Files reach it one at a time through Tauri's asset protocol; formats the engine can't decode
// are played from a temporary WAV the backend makes.

import { convertFileSrc } from "@tauri-apps/api/core";
import { api, asError } from "../api/commands";
import { Store, useStore } from "./store";

export interface PlayerItem {
  path: string;
  title: string;
  subtitle: string | null;
  artwork: string | null;
}

interface PlayerState {
  item: PlayerItem | null;
  isPlaying: boolean;
  duration: number;
  currentTime: number;
  waveform: number[];
  error: string | null;
}

const mimeTypes: Record<string, string> = {
  mp3: "audio/mpeg",
  wav: "audio/wav",
  aac: "audio/aac",
  m4a: "audio/mp4",
  flac: "audio/flac",
  aiff: "audio/aiff",
  aif: "audio/aiff",
  caf: "audio/x-caf",
};

export const previewableExtensions = new Set(Object.keys(mimeTypes));

export function canPreview(path: string | null | undefined): boolean {
  if (!path) return false;
  const ext = path.split(".").pop()?.toLowerCase() ?? "";
  return previewableExtensions.has(ext);
}

class Player {
  readonly store = new Store<PlayerState>({ item: null, isPlaying: false, duration: 0, currentTime: 0, waveform: [], error: null });
  private audio = new Audio();
  private frame = 0;
  private transcoded = false;
  private loadToken = 0;
  /** Called with a message when a file can't be played at all. */
  onError: (message: string) => void = () => {};

  constructor() {
    this.audio.preload = "auto";
    this.audio.addEventListener("loadedmetadata", () => {
      if (Number.isFinite(this.audio.duration)) this.store.set({ duration: this.audio.duration });
    });
    this.audio.addEventListener("ended", () => {
      this.stopTicking();
      this.store.set({ isPlaying: false, currentTime: this.store.get().duration });
    });
    this.audio.addEventListener("error", () => void this.recover());
  }

  setMuted(muted: boolean) {
    this.audio.muted = muted;
  }

  isCurrent(path: string | null | undefined) {
    return !!path && this.store.get().item?.path === path;
  }

  /** Play/pause for the given file, switching tracks if something else is loaded. */
  toggle(item: PlayerItem) {
    if (this.isCurrent(item.path) && this.audio.src) {
      this.togglePlayback();
    } else {
      void this.play(item);
    }
  }

  togglePlayback() {
    if (!this.store.get().item) return;
    if (this.store.get().isPlaying) this.pause();
    else this.resume();
  }

  async play(item: PlayerItem) {
    this.stop();
    const token = ++this.loadToken;
    this.transcoded = false;
    this.store.set({ item, error: null });
    try {
      const ext = item.path.split(".").pop()?.toLowerCase() ?? "";
      const decodable = this.audio.canPlayType(mimeTypes[ext] ?? "") !== "";
      const source = await api.previewSource(item.path, !decodable);
      if (token !== this.loadToken) return;
      this.transcoded = source.transcoded;
      this.audio.src = convertFileSrc(source.path);
      this.resume();
      void api.waveform(item.path).then((peaks) => {
        if (token === this.loadToken) this.store.set({ waveform: peaks });
      });
    } catch (error) {
      if (token !== this.loadToken) return;
      this.fail(asError(error).message || "This file can't be previewed.");
    }
  }

  /** The web engine refused the file: try once more from a WAV copy, then give up. */
  private async recover() {
    const item = this.store.get().item;
    if (!item || !this.audio.src) return;
    if (this.transcoded) {
      this.fail("This file can't be previewed.");
      return;
    }
    const token = this.loadToken;
    try {
      const source = await api.previewSource(item.path, true);
      if (token !== this.loadToken) return;
      this.transcoded = true;
      this.audio.src = convertFileSrc(source.path);
      this.resume();
    } catch (error) {
      if (token === this.loadToken) this.fail(asError(error).message || "This file can't be previewed.");
    }
  }

  private fail(message: string) {
    this.stop();
    this.onError(message);
  }

  resume() {
    const { currentTime, duration } = this.store.get();
    if (duration > 0 && currentTime >= duration - 0.05) this.audio.currentTime = 0;
    this.audio
      .play()
      .then(() => {
        this.store.set({ isPlaying: true });
        this.startTicking();
      })
      .catch(() => {
        /* An "error" event follows for undecodable files and is handled there. */
      });
  }

  pause() {
    this.audio.pause();
    this.stopTicking();
    this.store.set({ isPlaying: false, currentTime: this.audio.currentTime });
  }

  stop() {
    this.loadToken += 1;
    this.audio.pause();
    this.audio.removeAttribute("src");
    this.audio.load();
    this.stopTicking();
    this.store.set({ item: null, isPlaying: false, duration: 0, currentTime: 0, waveform: [], error: null });
  }

  seek(fraction: number) {
    const { duration } = this.store.get();
    if (duration <= 0) return;
    const target = Math.min(Math.max(fraction, 0), 1) * duration;
    this.audio.currentTime = Math.min(target, Math.max(duration - 0.01, 0));
    this.store.set({ currentTime: this.audio.currentTime });
  }

  skip(seconds: number) {
    const { duration, currentTime } = this.store.get();
    if (duration > 0) this.seek((currentTime + seconds) / duration);
  }

  private startTicking() {
    this.stopTicking();
    const tick = () => {
      this.store.set({ currentTime: this.audio.currentTime });
      this.frame = requestAnimationFrame(tick);
    };
    this.frame = requestAnimationFrame(tick);
  }

  private stopTicking() {
    cancelAnimationFrame(this.frame);
  }
}

export const player = new Player();
export const usePlayer = <S,>(selector: (state: PlayerState) => S) => useStore(player.store, selector);
