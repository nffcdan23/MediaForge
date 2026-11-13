// Sheets, alerts, the video and picture preview and the log popover.

import { convertFileSrc } from "@tauri-apps/api/core";
import { BadgeCheck, ShieldCheck, Search, SquareArrowOutUpRight, X } from "lucide-react";
import { RaisedHand } from "./Icons";
import { useEffect, useState, type ReactNode } from "react";
import { api } from "../api/commands";
import { computerName, isMac, revealLabel } from "../lib/platform";
import { copy, reveal, showError } from "../state/actions";
import { app, useApp } from "../state/store";
import appIcon from "../assets/app-icon.png";

/** First-launch statement of what MediaForge is for and what it will not do (TermsSheet in RootView.swift). */
export function TermsSheet() {
  const acknowledged = useApp((s) => s.settings?.hasAcknowledgedTerms ?? false);
  const close = () => {
    void api
      .acknowledgeTerms()
      .then((settings) => app.set({ settings, showingTerms: false }))
      .catch(showError);
  };
  return (
    <div className="overlay" role="dialog" aria-modal="true" aria-labelledby="terms-title">
      <div className="sheet">
        <img className="app-icon" src={appIcon} alt="" draggable={false} />
        <div className="heading">
          <h1 id="terms-title">Welcome to MediaForge</h1>
          <p>A few things to know before you start.</p>
        </div>
        <div className="terms-points">
          <Point icon={<BadgeCheck size={26} color="#fff" fill="#30d158" strokeWidth={2} />} title="Download only what you're allowed to">
            Use MediaForge for media you own, have licensed, or that the source and rights holder permit you to download. You're responsible for how you use it.
          </Point>
          <Point icon={<ShieldCheck size={26} color="#fff" fill="#0a84ff" strokeWidth={2} />} title="Restrictions are respected">
            MediaForge does not bypass DRM, sign-in requirements, paywalls or regional restrictions. When a source says no, you'll get a clear message instead.
          </Point>
          <Point icon={<RaisedHand />} title="Private by design">
            No accounts, tracking, telemetry or advertising. The only network requests are to the sources you paste. History and settings stay on this {computerName()}.
          </Point>
        </div>
        <div className="buttons">
          {!acknowledged && (
            <button type="button" className="button large" onClick={() => void api.quit()}>
              Quit
            </button>
          )}
          <button type="button" className="button large prominent" style={{ minWidth: 158 }} onClick={close} autoFocus>
            {acknowledged ? "Done" : "I Understand"}
          </button>
        </div>
      </div>
    </div>
  );
}

function Point({ icon, title, children }: { icon: ReactNode; title: string; children: ReactNode }) {
  return (
    <div className="terms-point">
      <span className="symbol">{icon}</span>
      <div>
        <strong>{title}</strong>
        <p>{children}</p>
      </div>
    </div>
  );
}

export function AlertDialog() {
  const alert = useApp((s) => s.alert);
  if (!alert) return null;
  return (
    <div className="overlay" role="alertdialog" aria-modal="true" aria-labelledby="alert-title" onClick={() => app.set({ alert: null })}>
      <div className="alert" onClick={(e) => e.stopPropagation()}>
        <img src={appIcon} alt="" draggable={false} />
        <h2 id="alert-title">{alert.title}</h2>
        {alert.message && <p className="selectable">{alert.message}</p>}
        <div className="buttons">
          <button type="button" className="button prominent" autoFocus onClick={() => app.set({ alert: null })}>
            OK
          </button>
        </div>
      </div>
    </div>
  );
}

export function ConfirmationDialog() {
  const confirmation = useApp((s) => s.confirmation);
  if (!confirmation) return null;
  const close = () => app.set({ confirmation: null });
  return (
    <div className="overlay" role="alertdialog" aria-modal="true" aria-labelledby="confirm-title" onClick={close}>
      <div className="alert" onClick={(e) => e.stopPropagation()}>
        <img src={appIcon} alt="" draggable={false} />
        <h2 id="confirm-title">{confirmation.title}</h2>
        <p>{confirmation.message}</p>
        <div className="buttons">
          <button type="button" className="button" onClick={close}>
            Cancel
          </button>
          <button
            type="button"
            className="button prominent"
            style={confirmation.destructive ? { background: "#ff453a" } : undefined}
            autoFocus
            onClick={() => {
              close();
              confirmation.onConfirm();
            }}
          >
            {confirmation.confirmLabel}
          </button>
        </div>
      </div>
    </div>
  );
}

/** Shows a finished video or picture inside MediaForge: Quick Look's role on the Mac, and the same on Windows. */
export function FilePreview() {
  const preview = useApp((s) => s.filePreview);
  const [source, setSource] = useState<string | null>(null);
  const [failed, setFailed] = useState(false);

  useEffect(() => {
    setSource(null);
    setFailed(false);
    if (!preview) return;
    let live = true;
    void api
      .previewSource(preview.path, false)
      .then((s) => live && setSource(convertFileSrc(s.path)))
      .catch((error) => {
        app.set({ filePreview: null });
        showError(error);
      });
    return () => {
      live = false;
    };
  }, [preview]);

  if (!preview) return null;
  const close = () => app.set({ filePreview: null });
  const isImage = preview.kind === "image";
  return (
    <div className="overlay" role="dialog" aria-modal="true" aria-label={preview.title} onClick={close}>
      <div className={`video-preview${isImage ? " image" : ""}`} onClick={(e) => e.stopPropagation()}>
        {isImage ? (
          <div className="stage">
            {source && !failed && <img src={source} alt={preview.title} draggable={false} onError={() => setFailed(true)} />}
            {failed && <PreviewFailure what="picture" opener="photo viewer" />}
          </div>
        ) : (
          <>
            {source && !failed && <video src={source} controls autoPlay playsInline muted={preview.muted} onError={() => setFailed(true)} />}
            {failed && <PreviewFailure what="video" opener="video player" />}
          </>
        )}
        <footer>
          <span className="title">{preview.title}</span>
          <button type="button" className="button" onClick={() => void api.openPath(preview.path).catch(showError)}>
            <SquareArrowOutUpRight size={13} />
            Open
          </button>
          <button type="button" className="button" onClick={() => void reveal(preview.path)}>
            <Search size={13} />
            {revealLabel()}
          </button>
          <button type="button" className="button" aria-label="Close" title="Close" onClick={close}>
            <X size={13} />
          </button>
        </footer>
      </div>
    </div>
  );
}

function PreviewFailure({ what, opener }: { what: string; opener: string }) {
  return (
    <div style={{ padding: 40, color: "#fff", textAlign: "center" }}>
      This {what} can't be shown inside MediaForge{isMac() ? "" : " on this PC"}. Open it in your {opener} instead.
    </div>
  );
}

/** The last lines the tools wrote for a job ("Show Details"). */
export function LogPopover() {
  const log = useApp((s) => s.log);
  const [lines, setLines] = useState<string[]>([]);
  useEffect(() => {
    setLines([]);
    if (!log) return;
    void api.jobLog(log.jobId).then(setLines).catch(() => setLines([]));
  }, [log]);
  if (!log) return null;
  const left = Math.max(12, Math.min(log.anchor.x - 260, window.innerWidth - 532));
  const top = Math.max(12, Math.min(log.anchor.y, window.innerHeight - 292));
  return (
    <>
      <div className="overlay" style={{ background: "transparent" }} onClick={() => app.set({ log: null })} />
      <div className="popover" style={{ left, top }} role="dialog" aria-label="Details">
        <header>
          Details
          <button type="button" className="button" onClick={() => void copy(lines.join("\n"))}>
            Copy
          </button>
        </header>
        <pre className="scroll selectable">{lines.join("\n")}</pre>
      </div>
    </>
  );
}
