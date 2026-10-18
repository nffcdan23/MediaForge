// Building blocks shared by every screen (Theme.swift's views and the SwiftUI controls they used).

import { ChevronsUpDown } from "lucide-react";
import { useEffect, useState, type CSSProperties, type ReactNode } from "react";
import { api } from "../api/commands";
import type { ThumbnailRequest } from "../api/types";
import { duration as formatDuration, percent } from "../lib/format";

export function PageHeader({ title, subtitle }: { title: string; subtitle: string }) {
  return (
    <header className="page-header">
      <h1>{title}</h1>
      <p>{subtitle}</p>
    </header>
  );
}

export function FormatBadge({ text, prominent = false }: { text: string; prominent?: boolean }) {
  return <span className={`format-badge${prominent ? " prominent" : ""}`}>{text}</span>;
}

/** A small circular glass button with a tooltip, used for row actions. */
export function IconButton({ label, onClick, children, tinted = false }: { label: string; onClick: () => void; children: ReactNode; tinted?: boolean }) {
  return (
    <button
      type="button"
      className={`icon-button${tinted ? " tinted" : ""}`}
      title={label}
      aria-label={label}
      onClick={(event) => {
        event.stopPropagation();
        onClick();
      }}
    >
      {children}
    </button>
  );
}

export function ToolbarButton({ label, onClick, children, disabled = false }: { label: string; onClick: () => void; children: ReactNode; disabled?: boolean }) {
  return (
    <button type="button" className="toolbar-button" title={label} aria-label={label} onClick={onClick} disabled={disabled}>
      {children}
    </button>
  );
}

/** A determinate or indeterminate progress bar in the brand gradient. */
export function ProgressBar({ fraction, height = 6 }: { fraction: number | null; height?: number }) {
  const clamped = fraction == null ? null : Math.min(Math.max(fraction, 0), 1);
  return (
    <div
      className={`progress${clamped == null ? " indeterminate" : ""}`}
      style={{ height }}
      role="progressbar"
      aria-label="Progress"
      aria-valuetext={clamped == null ? "In progress" : percent(clamped)}
    >
      <div className="fill" style={clamped == null ? undefined : { width: `${clamped * 100}%` }} />
    </div>
  );
}

const thumbnailCache = new Map<string, Promise<string | null>>();

function loadThumbnail(source: ThumbnailRequest): Promise<string | null> {
  const key = source.kind === "remote" ? `r:${source.url}` : `f:${source.path}`;
  let pending = thumbnailCache.get(key);
  if (!pending) {
    pending = api.thumbnail(source).catch(() => null);
    thumbnailCache.set(key, pending);
    // Failures aren't kept, so a file that appears later still gets its artwork.
    void pending.then((result) => {
      if (!result) thumbnailCache.delete(key);
    });
  }
  return pending;
}

/** Artwork for a job or history item, with a branded placeholder while loading or when there is none. */
export function Thumbnail({
  source,
  icon,
  className = "",
  style,
  duration,
  completed = false,
}: {
  source: ThumbnailRequest | null;
  icon: ReactNode;
  className?: string;
  style?: CSSProperties;
  duration?: number | null;
  completed?: boolean;
}) {
  const [image, setImage] = useState<string | null>(null);
  const key = source ? (source.kind === "remote" ? source.url : source.path) : "";
  useEffect(() => {
    let live = true;
    setImage(null);
    if (source) {
      void loadThumbnail(source).then((url) => {
        if (live) setImage(url);
      });
    }
    return () => {
      live = false;
    };
  }, [key]);
  return (
    <div className={`thumbnail ${className}`} style={style}>
      {!image && icon}
      {image && <img src={image} alt="" draggable={false} />}
      {duration != null && <span className="duration-badge">{formatDuration(duration)}</span>}
      {completed && <CheckBadgeSmall />}
    </div>
  );
}

function CheckBadgeSmall() {
  return (
    <svg width={22} height={22} viewBox="0 0 24 24" aria-hidden="true" className="done-check">
      <circle cx="12" cy="12" r="10.5" fill="#30d158" />
      <path d="M7.4 12.4l3.1 3.1 6.1-6.6" fill="none" stroke="#fff" strokeWidth="2.4" strokeLinecap="round" strokeLinejoin="round" />
    </svg>
  );
}

/** A pop-up button: the system's own menu on each platform, styled to match. */
export function Popup<T extends string | number>({
  value,
  options,
  label,
  onChange,
  borderless = false,
  width,
}: {
  value: T;
  options: { value: T; label: string }[];
  label: string;
  onChange: (value: T) => void;
  borderless?: boolean;
  width?: number;
}) {
  const index = Math.max(
    0,
    options.findIndex((o) => o.value === value),
  );
  return (
    <span className={`popup${borderless ? " borderless" : ""}`}>
      <select
        aria-label={label}
        value={String(index)}
        style={width ? { width } : undefined}
        onChange={(event) => onChange(options[Number(event.target.value)].value)}
      >
        {options.map((option, i) => (
          <option key={String(option.value)} value={String(i)}>
            {option.label}
          </option>
        ))}
      </select>
      <ChevronsUpDown size={12} strokeWidth={2.4} />
    </span>
  );
}

export function Segmented<T extends string>({
  value,
  options,
  onChange,
  capsule = false,
  label,
}: {
  value: T;
  options: { value: T; label: ReactNode }[];
  onChange: (value: T) => void;
  capsule?: boolean;
  label: string;
}) {
  return (
    <div className={`segmented${capsule ? " capsule" : ""}`} role="radiogroup" aria-label={label}>
      {options.map((option) => (
        <button
          key={option.value}
          type="button"
          role="radio"
          aria-checked={option.value === value}
          className={option.value === value ? "selected" : ""}
          onClick={() => onChange(option.value)}
        >
          {option.label}
        </button>
      ))}
    </div>
  );
}

export function Toggle({ on, onChange, label }: { on: boolean; onChange: (on: boolean) => void; label: string }) {
  return <button type="button" role="switch" aria-checked={on} aria-label={label} className={`toggle${on ? " on" : ""}`} onClick={() => onChange(!on)} />;
}

/** A small caption above a menu picker. */
export function OptionField({ title, children }: { title: string; children: ReactNode }) {
  return (
    <label className="option-field">
      <span>{title}</span>
      {children}
    </label>
  );
}

export function SectionHeader({ title, count, children }: { title: string; count?: number; children?: ReactNode }) {
  return (
    <div className="section-header">
      <h2>{title}</h2>
      {count != null && count > 0 && <span className="count">{count}</span>}
      <div className="actions">{children}</div>
    </div>
  );
}

export function EmptyState({ icon, title, message }: { icon: ReactNode; title: string; message: string }) {
  return (
    <div className="empty-state">
      {icon}
      <h3>{title}</h3>
      <p>{message}</p>
    </div>
  );
}

export function Spinner({ large = false }: { large?: boolean }) {
  return <span className={`spinner${large ? " large" : ""}`} role="progressbar" aria-label="Loading" />;
}
