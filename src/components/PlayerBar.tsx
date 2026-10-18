// The floating preview player along the bottom of the window (PlayerBar.swift).

import { AudioWaveform, Search, X } from "lucide-react";
import { useEffect, useRef } from "react";
import { duration as formatDuration, fileName, percent } from "../lib/format";
import { revealLabel } from "../lib/platform";
import { reveal } from "../state/actions";
import { player, usePlayer } from "../state/player";
import { Thumbnail } from "./Controls";
import { Back10, Forward10, PauseFill, PlayFill } from "./Icons";

export function PlayerBar() {
  const item = usePlayer((p) => p.item);
  const isPlaying = usePlayer((p) => p.isPlaying);
  const current = usePlayer((p) => p.currentTime);
  const total = usePlayer((p) => p.duration);
  const waveform = usePlayer((p) => p.waveform);
  if (!item) return null;
  const progress = total > 0 ? Math.min(Math.max(current / total, 0), 1) : 0;
  const artwork = item.artwork ? { kind: "remote" as const, url: item.artwork } : { kind: "file" as const, path: item.path };

  return (
    <div className="player-wrap">
      <div className="player glass" role="region" aria-label="Preview player">
        <Thumbnail source={artwork} icon={<AudioWaveform size={16} color="url(#brand-gradient)" />} />
        <div className="texts">
          <strong title={item.title}>{item.title}</strong>
          <span>{item.subtitle ?? fileName(item.path)}</span>
        </div>
        <div className="transport">
          <button type="button" title="Back 10 seconds" aria-label="Back 10 seconds" onClick={() => player.skip(-10)}>
            <Back10 size={19} />
          </button>
          <button type="button" className="play" title={isPlaying ? "Pause" : "Play"} aria-label={isPlaying ? "Pause" : "Play"} onClick={() => player.togglePlayback()}>
            {isPlaying ? <PauseFill size={22} /> : <PlayFill size={22} />}
          </button>
          <button type="button" title="Forward 10 seconds" aria-label="Forward 10 seconds" onClick={() => player.skip(10)}>
            <Forward10 size={19} />
          </button>
        </div>
        <span className="time left">{formatDuration(current)}</span>
        <WaveformScrubber levels={waveform} progress={progress} onSeek={(f) => player.seek(f)} />
        <span className="time">{formatDuration(total)}</span>
        <div className="extras">
          <button type="button" title={revealLabel()} aria-label={revealLabel()} onClick={() => void reveal(item.path)}>
            <Search size={14} strokeWidth={2.6} />
          </button>
          <button type="button" title="Close player" aria-label="Close player" onClick={() => player.stop()}>
            <X size={14} strokeWidth={2.6} />
          </button>
        </div>
      </div>
    </div>
  );
}

/** A waveform you can click or drag to seek. Even bars stand in until analysis finishes. */
function WaveformScrubber({ levels, progress, onSeek }: { levels: number[]; progress: number; onSeek: (fraction: number) => void }) {
  const canvas = useRef<HTMLCanvasElement>(null);

  useEffect(() => {
    const element = canvas.current;
    if (!element) return;
    const draw = () => {
      const scale = window.devicePixelRatio || 1;
      const width = element.clientWidth;
      const height = element.clientHeight;
      if (element.width !== Math.round(width * scale)) element.width = Math.round(width * scale);
      if (element.height !== Math.round(height * scale)) element.height = Math.round(height * scale);
      const context = element.getContext("2d");
      if (!context) return;
      context.setTransform(scale, 0, 0, scale, 0, 0);
      context.clearRect(0, 0, width, height);
      const pitch = 3;
      const bar = 2;
      const count = Math.max(1, Math.floor(width / pitch));
      const gradient = context.createLinearGradient(0, 0, width, 0);
      gradient.addColorStop(0, "#ff9938");
      gradient.addColorStop(1, "#ff5461");
      const rest = getComputedStyle(element).getPropertyValue("--text").trim() || "rgba(255,255,255,0.9)";
      for (let index = 0; index < count; index += 1) {
        const position = index / count;
        const level = levels.length === 0 ? 0.14 : Math.max(0.06, Math.min(1, levels[Math.min(levels.length - 1, Math.floor(position * levels.length))]));
        const barHeight = Math.max(2, level * height);
        context.globalAlpha = position < progress ? 1 : 0.22;
        context.fillStyle = position < progress ? gradient : rest;
        context.beginPath();
        context.roundRect(index * pitch, (height - barHeight) / 2, bar, barHeight, 1);
        context.fill();
      }
      context.globalAlpha = 1;
    };
    draw();
    const observer = new ResizeObserver(draw);
    observer.observe(element);
    return () => observer.disconnect();
  }, [levels, progress]);

  const seekTo = (clientX: number) => {
    const rect = canvas.current?.getBoundingClientRect();
    if (rect && rect.width > 0) onSeek((clientX - rect.left) / rect.width);
  };

  return (
    <canvas
      ref={canvas}
      className="waveform"
      role="slider"
      aria-label="Playback position"
      aria-valuetext={percent(progress)}
      tabIndex={0}
      onKeyDown={(event) => {
        if (event.key === "ArrowRight") onSeek(Math.min(progress + 0.05, 1));
        if (event.key === "ArrowLeft") onSeek(Math.max(progress - 0.05, 0));
      }}
      onPointerDown={(event) => {
        event.currentTarget.setPointerCapture(event.pointerId);
        seekTo(event.clientX);
      }}
      onPointerMove={(event) => {
        if (event.buttons === 1) seekTo(event.clientX);
      }}
    />
  );
}
