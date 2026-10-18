// The window: sidebar, toolbar, the current screen, the player, and anything shown over them (RootView.swift).

import { getCurrentWebview } from "@tauri-apps/api/webview";
import { CircleArrowDown, Folder, PanelLeft, Plus, RefreshCw, Search, Settings as SettingsIcon, Trash } from "lucide-react";
import { useEffect, useRef, useState, type ReactNode } from "react";
import { api } from "./api/commands";
import type { Section } from "./api/types";
import { Segmented, ToolbarButton } from "./components/Controls";
import { BrandGradientDefs, HistoryIcon } from "./components/Icons";
import { AlertDialog, ConfirmationDialog, LogPopover, TermsSheet, VideoPreview } from "./components/Overlays";
import { PlayerBar } from "./components/PlayerBar";
import { isPending } from "./lib/status";
import { chooseFilesToConvert, confirmClearHistory, navigate, openDestination } from "./state/actions";
import { usePlayer } from "./state/player";
import { app, useApp } from "./state/store";
import { ConversionsScreen } from "./screens/Conversions";
import { DownloadsScreen } from "./screens/Downloads";
import { historyView, HistoryScreen, type HistoryFilter } from "./screens/History";
import { SettingsScreen } from "./screens/Settings";
import { useStore } from "./state/store";

const titles: Record<Section, string> = {
  downloads: "Downloads",
  conversions: "Conversions",
  history: "History",
  settings: "Settings",
};

export function App() {
  const ready = useApp((s) => s.ready);
  const platform = useApp((s) => s.platform);
  const section = useApp((s) => s.section);
  const collapsed = useApp((s) => s.sidebarCollapsed);
  const showingTerms = useApp((s) => s.showingTerms);
  const dropActive = useApp((s) => s.dropActive);
  const hasPlayer = usePlayer((p) => p.item !== null);
  const [scrolled, setScrolled] = useState(false);
  const page = useRef<HTMLDivElement>(null);

  // The window is created hidden and shown once the interface has rendered, so it never flashes white.
  // (Not requestAnimationFrame: a hidden web view doesn't run animation frames.)
  useEffect(() => {
    if (ready) window.setTimeout(() => void api.showMainWindow(), 30);
  }, [ready]);

  useEffect(() => {
    page.current?.scrollTo({ top: 0 });
    setScrolled(false);
  }, [section]);

  // Files dragged over the window: highlight while hovering; the backend adds them on drop.
  useEffect(() => {
    const unlisten = getCurrentWebview().onDragDropEvent((event) => {
      const type = event.payload.type;
      if (type === "enter" || type === "over") {
        if (!app.get().dropActive) app.set({ dropActive: true });
      } else {
        app.set({ dropActive: false });
      }
    });
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, []);

  if (!ready) return <div className={`window platform-${platform}`} />;

  return (
    <div className={`window platform-${platform}${collapsed ? " sidebar-collapsed" : ""}`}>
      <BrandGradientDefs />
      <Sidebar />
      <main className="detail">
        <Toolbar section={section} scrolled={scrolled} />
        <div ref={page} className={`page scroll${hasPlayer ? " with-player" : ""}`} onScroll={(e) => setScrolled(e.currentTarget.scrollTop > 4)}>
          {section === "settings" ? (
            <SettingsScreen />
          ) : (
            <div className="page-content">
              {section === "downloads" && <DownloadsScreen />}
              {section === "conversions" && <ConversionsScreen />}
              {section === "history" && <HistoryScreen />}
            </div>
          )}
        </div>
        <PlayerBar />
        {dropActive && section !== "conversions" && (
          <div className="drop-overlay">
            <div className="glass">Drop files to convert them</div>
          </div>
        )}
      </main>
      {showingTerms && <TermsSheet />}
      <VideoPreview />
      <LogPopover />
      <ConfirmationDialog />
      <AlertDialog />
    </div>
  );
}

function Sidebar() {
  const jobs = useApp((s) => s.jobs);
  const pendingDownloads = jobs.filter((j) => j.kind === "download" && isPending(j.state)).length;
  const pendingConversions = jobs.filter((j) => j.kind === "conversion" && isPending(j.state)).length;
  return (
    <nav className="sidebar" aria-label="Sections">
      <div className="sidebar-top drag-region" data-tauri-drag-region>
        <SidebarToggle />
      </div>
      <div className="sidebar-list">
        <div className="sidebar-section-title">Library</div>
        <SidebarRow section="downloads" icon={<CircleArrowDown size={16} strokeWidth={1.9} />} badge={pendingDownloads} />
        <SidebarRow section="conversions" icon={<RefreshCw size={15} strokeWidth={1.9} />} badge={pendingConversions} />
        <SidebarRow section="history" icon={<HistoryIcon size={16} strokeWidth={1.9} />} badge={0} />
        <div className="sidebar-section-title">App</div>
        <SidebarRow section="settings" icon={<SettingsIcon size={16} strokeWidth={1.9} />} badge={0} />
      </div>
      <ToolStatusFooter />
    </nav>
  );
}

function SidebarToggle() {
  return (
    <button
      type="button"
      className="sidebar-toggle"
      title="Show or hide the sidebar"
      aria-label="Show or hide the sidebar"
      onClick={() => app.set((s) => ({ sidebarCollapsed: !s.sidebarCollapsed }))}
    >
      <PanelLeft size={17} strokeWidth={1.7} />
    </button>
  );
}

function SidebarRow({ section, icon, badge }: { section: Section; icon: ReactNode; badge: number }) {
  const selected = useApp((s) => s.section === section);
  return (
    <button type="button" className={`sidebar-row${selected ? " selected" : ""}`} aria-current={selected ? "page" : undefined} onClick={() => navigate(section)}>
      {icon}
      {titles[section]}
      {badge > 0 && <span className="badge">{badge}</span>}
    </button>
  );
}

/** A one-line health check for the helper tools, pinned to the bottom of the sidebar. */
function ToolStatusFooter() {
  const tools = useApp((s) => s.tools);
  const missing = tools?.statuses.filter((s) => !s.path) ?? [];
  const detail = (() => {
    if (!tools) return "Checking tools…";
    if (missing.length > 0) return `${missing.map((m) => m.name).join(", ")} missing`;
    const versions = tools.statuses.filter((s) => s.tool !== "ffprobe" && s.version).map((s) => `${s.name} ${s.version}`);
    return versions.length > 0 ? versions.join(" · ") : "Checking tools…";
  })();
  return (
    <button type="button" className="tool-footer" title="Open tool settings" onClick={() => navigate("settings")}>
      <span className={`dot ${missing.length === 0 ? "ready" : "missing"}`} />
      <span style={{ minWidth: 0 }}>
        <strong>{missing.length === 0 ? "Ready" : "Setup needed"}</strong>
        <span>{detail}</span>
      </span>
    </button>
  );
}

function Toolbar({ section, scrolled }: { section: Section; scrolled: boolean }) {
  return (
    <header className={`toolbar drag-region${scrolled ? " scrolled" : ""}`} data-tauri-drag-region>
      <SidebarToggle />
      <span className="toolbar-title" data-tauri-drag-region>
        {titles[section]}
      </span>
      <span className="toolbar-spacer" data-tauri-drag-region />
      {section === "downloads" && (
        <ToolbarButton label="Open the destination folder" onClick={() => void openDestination()}>
          <Folder size={17} strokeWidth={1.8} />
        </ToolbarButton>
      )}
      {section === "conversions" && (
        <div className="toolbar-group">
          <button type="button" title="Choose files to convert" aria-label="Add files" onClick={() => void chooseFilesToConvert(false)}>
            <Plus size={18} strokeWidth={1.8} />
          </button>
          <button type="button" title="Open the destination folder" aria-label="Open folder" onClick={() => void openDestination()}>
            <Folder size={17} strokeWidth={1.8} />
          </button>
        </div>
      )}
      {section === "history" && <HistoryToolbar />}
    </header>
  );
}

function HistoryToolbar() {
  const { query, filter } = useStore(historyView, (s) => s);
  const empty = useApp((s) => s.history.length === 0);
  return (
    <>
      <Segmented<HistoryFilter>
        capsule
        label="Filter history"
        value={filter}
        onChange={(value) => historyView.set({ filter: value })}
        options={[
          { value: "all", label: "All" },
          { value: "audio", label: "Audio" },
          { value: "video", label: "Video" },
          { value: "failed", label: "Failed" },
        ]}
      />
      <ToolbarButton label="Clear history" disabled={empty} onClick={confirmClearHistory}>
        <Trash size={16} strokeWidth={1.8} />
      </ToolbarButton>
      <label className="search-field glass">
        <Search size={14} className="secondary" />
        <input type="search" placeholder="Search history" value={query} spellCheck={false} onChange={(e) => historyView.set({ query: e.target.value })} />
      </label>
    </>
  );
}
