// The Conversions screen (ConversionsView.swift).

import { FileDown, Folder, FolderPlus, Import, RefreshCw } from "lucide-react";
import { PageHeader } from "../components/Controls";
import { AudioOptionsControls, QueueSection, ToolsBanner } from "../components/Jobs";
import { fileName } from "../lib/format";
import { formatDetail } from "../lib/options";
import { computerName } from "../lib/platform";
import { chooseFilesToConvert, openDestination } from "../state/actions";
import { useApp, useSettings } from "../state/store";

export function ConversionsScreen() {
  const settings = useSettings();
  const canConvert = useApp((s) => s.tools?.canConvert ?? false);
  const targeted = useApp((s) => s.dropActive);
  return (
    <>
      <PageHeader title="Convert files" subtitle={`Turn audio and video already on your ${computerName()} into the format your project needs.`} />
      {!canConvert && <ToolsBanner only={(tool) => tool !== "yt-dlp"} />}

      <div className={`drop-zone${targeted ? " targeted" : ""}`}>
        {targeted ? <FileDown size={38} strokeWidth={1.4} color="url(#brand-gradient)" /> : <Import size={38} strokeWidth={1.4} color="url(#brand-gradient)" />}
        <div className="texts">
          <h3>{targeted ? "Drop to add" : "Drop audio or video files here"}</h3>
          <p>Folders work too. MP3, WAV, FLAC, M4A, MP4, MOV, MKV and more.</p>
        </div>
        <div className="buttons">
          <button type="button" className="button large" disabled={!canConvert} onClick={() => void chooseFilesToConvert(false)}>
            <FolderPlus size={15} />
            Choose Files…
          </button>
          <button type="button" className="button large" disabled={!canConvert} onClick={() => void chooseFilesToConvert(true)}>
            <Folder size={15} />
            Choose Folder…
          </button>
        </div>
      </div>

      <div className="output-card card">
        <div className="label">
          <strong>Convert to</strong>
          <span>{formatDetail(settings.audioFormat)}</span>
        </div>
        <AudioOptionsControls />
        <div className="saves-to">
          <span>Saves to</span>
          <button type="button" className="button link" title={settings.destinationPath} onClick={() => void openDestination()}>
            <Folder size={13} />
            {fileName(settings.destinationPath)}
          </button>
        </div>
      </div>

      <QueueSection
        kind="conversion"
        showsEmptyState
        emptyTitle="Nothing converted yet"
        emptyMessage="Add files above. Originals are never modified or replaced."
        emptyIcon={<RefreshCw size={40} strokeWidth={1.4} />}
      />
    </>
  );
}
