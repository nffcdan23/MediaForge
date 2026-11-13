// The Metadata screen: a list of files on the left, the selected files' tags and details on the right.
// Works like a tagger such as Mp3tag: select one file or many, change fields, then Save.

import { ArrowDown, ArrowUp, FileDown, FilePen, FileText, Film, Folder, FolderPlus, Image as ImageIcon, ImageOff, ImagePlus, Music, Search, Star, Tags, Trash, X } from "lucide-react";
import { useEffect, useMemo, useRef, useState, type KeyboardEvent as ReactKeyboardEvent, type ReactNode } from "react";
import { api } from "../api/commands";
import type { FileKind, TagField, TagFile } from "../api/types";
import { EmptyState, ProgressBar, Segmented, Spinner, Toggle } from "../components/Controls";
import { tidy, useContextMenu } from "../components/Menu";
import { bytes } from "../lib/format";
import { computerName, isMac, revealLabel } from "../lib/platform";
import { fileNamePresets, splitName, tagsFromFileName, tokenize } from "../lib/tagPatterns";
import { reveal, showError, updateSettings } from "../state/actions";
import {
  applyFileNamePattern,
  applyNamePattern,
  chooseCover,
  chooseFiles,
  effective,
  fromLocalInput,
  isDirty,
  metadata,
  moveSelection,
  namesFromPattern,
  removeCover,
  removeSelected,
  revert,
  select,
  selectAll,
  selectedFiles,
  setDate,
  setField,
  setName,
  setReadOnly,
  setSort,
  toLocalInput,
  visibleFiles,
  type KindFilter,
  type SortKey,
} from "../state/metadata";
import { Store, useApp, useSettings, useStore } from "../state/store";

/** Which pattern dialog is open. */
export const metadataView = new Store<{ dialog: "fromName" | "toName" | null }>({ dialog: null });

export const fieldLabels: Record<TagField, string> = {
  title: "Title",
  artist: "Artist",
  album: "Album",
  albumArtist: "Album artist",
  track: "Track",
  trackTotal: "Total tracks",
  disc: "Disc",
  discTotal: "Total discs",
  year: "Year",
  genre: "Genre",
  composer: "Composer",
  comment: "Comment",
  description: "Description",
  copyright: "Copyright",
  authors: "Authors",
  subject: "Subject",
  keywords: "Tags",
  category: "Category",
  rating: "Rating",
  dateTaken: "Date taken",
};

const hints: Partial<Record<TagField, string>> = {
  keywords: "Put ; between tags",
  authors: "Put ; between names",
};

const multiline = new Set<TagField>(["comment", "description"]);

const genres = [
  "Acoustic", "Alternative", "Ambient", "Blues", "Classical", "Country", "Dance", "Drum & Bass", "Electronic", "Folk", "Funk", "Gospel",
  "Hip-Hop", "House", "Indie", "Jazz", "Latin", "Metal", "Pop", "Punk", "R&B", "Reggae", "Rock", "Singer-Songwriter", "Soul", "Soundtrack",
  "Spoken Word", "Synthwave", "Techno", "Trance", "World",
];

function KindIcon({ kind, size = 14 }: { kind: FileKind; size?: number }) {
  if (kind === "audio") return <Music size={size} />;
  if (kind === "video") return <Film size={size} />;
  if (kind === "image") return <ImageIcon size={size} />;
  return <FileText size={size} />;
}

// MARK: - Screen

export function MetadataScreen() {
  const count = useStore(metadata, (s) => s.files.length);
  const loading = useStore(metadata, (s) => s.loading);
  const dialog = useStore(metadataView, (s) => s.dialog);
  if (count === 0) return <Welcome loading={loading} />;
  return (
    <div className="metadata-screen">
      <FileList />
      <Editor />
      {dialog && <PatternDialog mode={dialog} onClose={() => metadataView.set({ dialog: null })} />}
    </div>
  );
}

function Welcome({ loading }: { loading: boolean }) {
  const targeted = useApp((s) => s.dropActive);
  return (
    <div className="page-content">
      <header className="page-header">
        <h1>Edit metadata</h1>
        <p>Change the tags, titles, artwork and dates stored in files on your {computerName()}, one at a time or hundreds at once.</p>
      </header>
      <div className={`drop-zone${targeted ? " targeted" : ""}`}>
        {targeted ? <FileDown size={38} strokeWidth={1.4} color="url(#brand-gradient)" /> : <Tags size={38} strokeWidth={1.4} color="url(#brand-gradient)" />}
        <div className="texts">
          <h3>{targeted ? "Drop to add" : "Drop files or folders here"}</h3>
          <p>Any kind of file. Music and video get their tags and artwork; photos, PDFs and Office files their details; everything gets its name and dates.</p>
        </div>
        <div className="buttons">
          <button type="button" className="button large" disabled={loading} onClick={() => void chooseFiles(false)}>
            <FolderPlus size={15} />
            Choose Files…
          </button>
          <button type="button" className="button large" disabled={loading} onClick={() => void chooseFiles(true)}>
            <Folder size={15} />
            Choose Folder…
          </button>
        </div>
        {loading && <Spinner />}
      </div>
      <div className="metadata-tips card">
        <Tip title="For Spotify and other players">MP3s are saved with ID3v2.3 tags and front-cover artwork, which Spotify's local files, Windows and car stereos read.</Tip>
        <Tip title="Many files at once">Select several files to give them all the same album, artist or artwork. Fields that differ say “Multiple values” and are left alone unless you type in them.</Tip>
        <Tip title="Nothing changes until you save">Edited files are marked in the list. Save writes them all ({isMac() ? "⌘S" : "Ctrl+S"}); Revert forgets the changes.</Tip>
      </div>
    </div>
  );
}

function Tip({ title, children }: { title: string; children: ReactNode }) {
  return (
    <div className="tip">
      <strong>{title}</strong>
      <p>{children}</p>
    </div>
  );
}

// MARK: - List

const columns: { key: SortKey; label: string; className: string }[] = [
  { key: "name", label: "Name", className: "col-name" },
  { key: "title", label: "Title", className: "col-title" },
  { key: "artist", label: "Artist / Author", className: "col-artist" },
  { key: "album", label: "Album", className: "col-album" },
  { key: "track", label: "#", className: "col-track" },
  { key: "year", label: "Year", className: "col-year" },
  { key: "format", label: "Type", className: "col-format" },
];

function FileList() {
  const state = useStore(metadata, (s) => s);
  const files = useMemo(() => visibleFiles(state), [state.files, state.filter, state.query, state.sort, state.edits]);
  const chosen = useMemo(() => new Set(state.selected), [state.selected]);
  const unsaved = useMemo(() => state.files.filter((f) => isDirty(state.edits[f.path])).length, [state.files, state.edits]);
  const list = useRef<HTMLDivElement>(null);

  // Keep the row reached with the arrow keys in view.
  useEffect(() => {
    if (!state.focus) return;
    list.current?.querySelector(`[data-path="${CSS.escape(state.focus)}"]`)?.scrollIntoView({ block: "nearest" });
  }, [state.focus]);

  const onKeyDown = (event: ReactKeyboardEvent) => {
    const command = isMac() ? event.metaKey : event.ctrlKey;
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      moveSelection(event.key === "ArrowDown" ? 1 : -1, event.shiftKey);
    } else if (command && event.key.toLowerCase() === "a") {
      event.preventDefault();
      selectAll();
    } else if (event.key === "Delete" || (isMac() && event.key === "Backspace" && command)) {
      event.preventDefault();
      removeSelected();
    }
  };

  return (
    <section className="metadata-list card">
      <div className="metadata-list-header">
        <Segmented<KindFilter>
          capsule
          label="Show"
          value={state.filter}
          onChange={(filter) => metadata.set({ filter })}
          options={[
            { value: "all", label: "All" },
            { value: "audio", label: "Audio" },
            { value: "video", label: "Video" },
            { value: "image", label: "Images" },
            { value: "document", label: "Documents" },
            { value: "other", label: "Other" },
          ]}
        />
        <span className="toolbar-spacer" />
        <label className="search-field glass compact">
          <Search size={13} className="secondary" />
          <input type="search" placeholder="Filter" value={state.query} spellCheck={false} onChange={(e) => metadata.set({ query: e.target.value })} />
        </label>
      </div>
      <div className="metadata-table scroll" ref={list} tabIndex={0} onKeyDown={onKeyDown} role="grid" aria-label="Files" aria-multiselectable="true">
        <div className="metadata-row heading" role="row">
          <span className="col-dot" />
          {columns.map((column) => (
            <button key={column.key} type="button" className={column.className} role="columnheader" onClick={() => setSort(column.key)}>
              {column.label}
              {state.sort?.key === column.key && (state.sort.ascending ? <ArrowUp size={11} /> : <ArrowDown size={11} />)}
            </button>
          ))}
        </div>
        {files.map((file) => (
          <FileRow key={file.path} file={file} selected={chosen.has(file.path)} dirty={isDirty(state.edits[file.path])} />
        ))}
        {files.length === 0 && <div className="metadata-none">No files match.</div>}
      </div>
      <ListFooter shown={files.length} unsaved={unsaved} />
    </section>
  );
}

function ListFooter({ shown, unsaved }: { shown: number; unsaved: number }) {
  const total = useStore(metadata, (s) => s.files.length);
  const selected = useStore(metadata, (s) => s.selected.length);
  const saving = useStore(metadata, (s) => s.saving);
  const status = useStore(metadata, (s) => s.status);
  const loading = useStore(metadata, (s) => s.loading);
  const parts = [shown === total ? `${total} ${total === 1 ? "file" : "files"}` : `${shown} of ${total} files`];
  if (selected > 0) parts.push(`${selected} selected`);
  if (unsaved > 0) parts.push(`${unsaved} unsaved`);
  return (
    <footer className="metadata-list-footer">
      {saving ? (
        <>
          <span>
            Saving {Math.min(saving.done + 1, saving.total)} of {saving.total}…
          </span>
          <ProgressBar fraction={saving.total > 0 ? saving.done / saving.total : null} height={5} />
        </>
      ) : (
        <>
          <span>{parts.join(" · ")}</span>
          {loading && <Spinner />}
          {status && <span className="saved">{status}</span>}
        </>
      )}
    </footer>
  );
}

function FileRow({ file, selected, dirty }: { file: TagFile; selected: boolean; dirty: boolean }) {
  const edits = useStore(metadata, (s) => s.edits[file.path]);
  const name = edits?.name !== undefined ? edits.name + splitName(file.name).extension : file.name;
  const value = (field: TagField) => edits?.fields[field] ?? file.fields[field] ?? "";
  const edited = (field: TagField) => edits?.fields[field] !== undefined;
  const year = value("year") || value("dateTaken").slice(0, 4);
  const { onContextMenu, menu } = useContextMenu(() =>
    tidy([
      { label: revealLabel(), icon: <Search size={14} />, onSelect: () => void reveal(file.path) },
      dirty && { label: "Revert Changes", icon: <X size={14} />, onSelect: () => revert(metadata.get().selected.includes(file.path) ? undefined : [file.path]) },
      "separator",
      { label: "Remove from List", icon: <Trash size={14} />, onSelect: removeSelected },
    ]),
  );
  return (
    <div
      className={`metadata-row${selected ? " selected" : ""}${dirty ? " dirty" : ""}`}
      data-path={file.path}
      role="row"
      aria-selected={selected}
      onMouseDown={(event) => {
        if (event.button !== 0) return;
        const command = isMac() ? event.metaKey : event.ctrlKey;
        select(file.path, event.shiftKey ? "range" : command ? "toggle" : "only");
      }}
      onContextMenu={(event) => {
        if (!selected) select(file.path, "only");
        onContextMenu(event);
      }}
      title={file.path}
    >
      <span className="col-dot">{dirty ? <span className="dot" title="Not saved yet" /> : <KindIcon kind={file.kind} size={13} />}</span>
      <span className={`col-name${edits?.name !== undefined ? " edited" : ""}`}>{name}</span>
      <span className={`col-title${edited("title") ? " edited" : ""}`}>{value("title")}</span>
      <span className={`col-artist${edited("artist") || edited("authors") ? " edited" : ""}`}>{value("artist") || value("authors")}</span>
      <span className={`col-album${edited("album") ? " edited" : ""}`}>{value("album")}</span>
      <span className={`col-track${edited("track") ? " edited" : ""}`}>{value("track")}</span>
      <span className={`col-year${edited("year") || edited("dateTaken") ? " edited" : ""}`}>{year}</span>
      <span className="col-format">{file.format}</span>
      {menu}
    </div>
  );
}

// MARK: - Editor

/** The value every selected file shares, or `mixed` when they differ. */
function shared(files: TagFile[], read: (file: TagFile) => string): { value: string; mixed: boolean } {
  const values = new Set(files.map(read));
  return values.size === 1 ? { value: [...values][0], mixed: false } : { value: "", mixed: true };
}

function Editor() {
  const state = useStore(metadata, (s) => s);
  const files = useMemo(() => selectedFiles(state), [state.files, state.selected, state.filter, state.query, state.sort, state.edits]);
  if (files.length === 0) {
    return (
      <aside className="metadata-editor card empty">
        <EmptyState icon={<FilePen size={36} strokeWidth={1.4} />} title="No file selected" message="Select a file to see and change its details. Select several to change them together." />
      </aside>
    );
  }
  // Fields every selected file can hold, in the first file's order.
  const common = files[0].editable.filter((field) => files.every((f) => f.editable.includes(field)));
  const single = files.length === 1 ? files[0] : null;
  const canHoldCover = files.every((f) => f.cover);
  const notes = [...new Set(files.map((f) => f.note).filter((n): n is string => !!n))];
  return (
    <aside className="metadata-editor card">
      <div className="editor-scroll scroll">
        <EditorHeader files={files} />
        {notes.map((note) => (
          <p key={note} className="notice">
            {note}
          </p>
        ))}
        {canHoldCover && <CoverSection files={files} />}
        {common.length > 0 ? (
          <section className="editor-section">
            <h3>{files.every((f) => f.kind === "audio" || f.kind === "video") ? "Tags" : "Details"}</h3>
            <Fields files={files} fields={common} />
          </section>
        ) : (
          files.length > 1 &&
          files.some((f) => f.editable.length > 0) && <p className="notice">These files have no fields in common. Select files of one kind to edit their tags together.</p>
        )}
        <FileSection files={files} single={single} />
        {single && <DetailsSection file={single} />}
      </div>
      <EditorFooter files={files} />
    </aside>
  );
}

function EditorHeader({ files }: { files: TagFile[] }) {
  if (files.length === 1) {
    const file = files[0];
    return (
      <div className="editor-header">
        <span className="kind-badge">
          <KindIcon kind={file.kind} size={16} />
        </span>
        <div>
          <strong title={file.name}>{file.name}</strong>
          <span>
            {file.format} · {bytes(file.size)}
          </span>
        </div>
      </div>
    );
  }
  const kinds = new Set(files.map((f) => f.kind));
  const kind = kinds.size === 1 ? [...kinds][0] : null;
  return (
    <div className="editor-header">
      <span className="kind-badge">{kind ? <KindIcon kind={kind} size={16} /> : <Tags size={16} />}</span>
      <div>
        <strong>{files.length} files selected</strong>
        <span>Changes apply to all of them</span>
      </div>
    </div>
  );
}

function Fields({ files, fields }: { files: TagFile[]; fields: TagField[] }) {
  const edits = useStore(metadata, (s) => s.edits);
  const rows: ReactNode[] = [];
  for (const field of fields) {
    if (field === "trackTotal" || field === "discTotal") continue;
    const current = shared(files, (f) => effective(f, field, edits));
    if (field === "track" || field === "disc") {
      const totalField = field === "track" ? "trackTotal" : "discTotal";
      const total = fields.includes(totalField) ? shared(files, (f) => effective(f, totalField, edits)) : null;
      rows.push(
        <div key={field} className="field">
          <span className="field-label">{fieldLabels[field]}</span>
          <div className="number-pair">
            <TextInput label={fieldLabels[field]} value={current.value} mixed={current.mixed} numeric onChange={(v) => setField(field, v)} />
            {total && (
              <>
                <span className="of">of</span>
                <TextInput label={fieldLabels[totalField]} value={total.value} mixed={total.mixed} numeric onChange={(v) => setField(totalField, v)} />
              </>
            )}
          </div>
        </div>,
      );
      continue;
    }
    rows.push(
      <div key={field} className={`field${multiline.has(field) ? " wide" : ""}`}>
        <span className="field-label">{fieldLabels[field]}</span>
        {field === "rating" ? (
          <Rating value={current.mixed ? null : Number(current.value || 0)} onChange={(stars) => setField("rating", stars === 0 ? "" : String(stars))} />
        ) : field === "dateTaken" ? (
          <DateInput label={fieldLabels[field]} value={current.value} mixed={current.mixed} onChange={(v) => setField("dateTaken", v)} />
        ) : (
          <TextInput
            label={fieldLabels[field]}
            value={current.value}
            mixed={current.mixed}
            multiline={multiline.has(field)}
            list={field === "genre" ? "metadata-genres" : undefined}
            hint={hints[field]}
            onChange={(v) => setField(field, v)}
          />
        )}
      </div>,
    );
  }
  return (
    <div className="fields">
      {rows}
      <datalist id="metadata-genres">
        {genres.map((g) => (
          <option key={g} value={g} />
        ))}
      </datalist>
    </div>
  );
}

function TextInput({
  label,
  value,
  mixed,
  onChange,
  multiline = false,
  numeric = false,
  list,
  hint,
}: {
  label: string;
  value: string;
  mixed: boolean;
  onChange: (value: string) => void;
  multiline?: boolean;
  numeric?: boolean;
  list?: string;
  hint?: string;
}) {
  const placeholder = mixed ? (numeric ? "Mixed" : "Multiple values") : (hint ?? "");
  const common = {
    "aria-label": label,
    value,
    placeholder,
    spellCheck: false,
    className: mixed ? "mixed" : "",
  };
  return (
    <span className={`text-input${numeric ? " numeric" : ""}`}>
      {multiline ? (
        <textarea {...common} rows={3} onChange={(e) => onChange(e.target.value)} />
      ) : (
        <input
          {...common}
          type="text"
          inputMode={numeric ? "numeric" : undefined}
          list={list}
          onChange={(e) => onChange(numeric ? e.target.value.replace(/[^0-9]/g, "") : e.target.value)}
        />
      )}
      {(value || mixed) && !numeric && (
        <button type="button" className="clear" title={mixed ? "Clear in every selected file" : "Clear"} aria-label={`Clear ${label}`} onClick={() => onChange("")}>
          <X size={11} strokeWidth={2.6} />
        </button>
      )}
    </span>
  );
}

function DateInput({ label, value, mixed, onChange }: { label: string; value: string; mixed: boolean; onChange: (value: string) => void }) {
  // An empty date box would otherwise show today's date in grey (WebKit) or "dd/mm/yyyy" (WebView2).
  const blank = mixed || !value;
  return (
    <span className="text-input">
      <input type="datetime-local" step={1} aria-label={label} className={blank ? "blank" : ""} value={value} onChange={(e) => onChange(e.target.value)} />
      {blank && <span className="mixed-label">{mixed ? "Multiple values" : "Not set"}</span>}
    </span>
  );
}

function Rating({ value, onChange }: { value: number | null; onChange: (stars: number) => void }) {
  return (
    <span className="rating" role="radiogroup" aria-label="Rating">
      {[1, 2, 3, 4, 5].map((star) => (
        <button
          key={star}
          type="button"
          role="radio"
          aria-checked={value === star}
          aria-label={`${star} star${star === 1 ? "" : "s"}`}
          className={value != null && star <= value ? "on" : ""}
          onClick={() => onChange(value === star ? 0 : star)}
        >
          <Star size={16} />
        </button>
      ))}
      {value == null && <span className="mixed-label">Multiple values</span>}
    </span>
  );
}

// MARK: - Artwork

const coverCache = new Map<string, Promise<string | null>>();

function useCover(file: TagFile | null): string | null {
  const [image, setImage] = useState<string | null>(null);
  const key = file ? `${file.path}|${file.revision}` : "";
  useEffect(() => {
    let live = true;
    setImage(null);
    if (!file) return;
    let pending = coverCache.get(key);
    if (!pending) {
      pending = api.metadataCover(file.path).catch(() => null);
      coverCache.set(key, pending);
    }
    void pending.then((url) => {
      if (live) setImage(url);
    });
    return () => {
      live = false;
    };
  }, [key]);
  return image;
}

function CoverSection({ files }: { files: TagFile[] }) {
  const edits = useStore(metadata, (s) => s.edits);
  const pending = files.map((f) => edits[f.path]?.cover);
  const pendingKinds = new Set(pending.map((p) => (p ? (p.type === "set" ? `set:${p.id}` : "remove") : "none")));
  const allPending = pending.every((p) => p) && pendingKinds.size === 1 ? pending[0] : null;
  const signatures = new Set(files.map((f) => (f.cover?.present ? f.cover.signature : null)));
  const sameStored = pendingKinds.size === 1 && pendingKinds.has("none") && signatures.size === 1;
  const stored = useCover(sameStored && files[0].cover?.present ? files[0] : null);
  const single = files.length === 1 ? files[0] : null;

  let picture: ReactNode;
  let caption: string;
  if (allPending?.type === "set") {
    picture = allPending.preview ? <img src={allPending.preview} alt="" /> : <ImagePlus size={30} />;
    caption = `New artwork${allPending.summary ? ` · ${allPending.summary}` : ""}`;
  } else if (allPending?.type === "remove") {
    picture = <ImageOff size={30} />;
    caption = "Artwork will be removed";
  } else if (sameStored && files[0].cover?.present) {
    picture = stored ? <img src={stored} alt="" /> : <Spinner />;
    caption = files[0].cover.summary ?? "Artwork";
  } else if (sameStored) {
    picture = <ImageOff size={30} />;
    caption = "No artwork";
  } else {
    picture = <Tags size={30} />;
    caption = "Different artwork";
  }
  const anyPresent = files.some((f) => f.cover?.present) || allPending?.type === "set";
  return (
    <section className="editor-section cover-section">
      <div className={`cover${allPending ? " edited" : ""}`}>{picture}</div>
      <div className="cover-side">
        <h3>Artwork</h3>
        <span className="secondary">{caption}</span>
        <div className="cover-buttons">
          <button type="button" className="button" onClick={() => void chooseCover()}>
            <ImagePlus size={13} />
            Choose…
          </button>
          <button type="button" className="button" disabled={!anyPresent || allPending?.type === "remove"} onClick={removeCover}>
            Remove
          </button>
          {single?.cover?.present && !allPending && (
            <button type="button" className="button" title="Save the artwork as a picture" onClick={() => void api.metadataExportCover(single.path).catch(showError)}>
              Save As…
            </button>
          )}
        </div>
      </div>
    </section>
  );
}

// MARK: - File

function FileSection({ files, single }: { files: TagFile[]; single: TagFile | null }) {
  const edits = useStore(metadata, (s) => s.edits);
  const date = (which: "modified" | "created") => shared(files, (f) => toLocalInput(edits[f.path]?.[which] ?? f[which]));
  const modified = date("modified");
  const created = date("created");
  const readOnly = shared(files, (f) => String(edits[f.path]?.readOnly ?? f.readOnly));
  const canSetCreated = files.every((f) => f.created);
  return (
    <section className="editor-section">
      <h3>File</h3>
      <div className="fields">
        {single && (
          <div className="field">
            <span className="field-label">Name</span>
            <span className="text-input with-suffix">
              <input
                type="text"
                aria-label="Name"
                spellCheck={false}
                value={edits[single.path]?.name ?? splitName(single.name).stem}
                onChange={(e) => setName(single.path, e.target.value)}
              />
              <span className="suffix">{splitName(single.name).extension}</span>
            </span>
          </div>
        )}
        <div className="field">
          <span className="field-label">Modified</span>
          <DateInput label="Modified" value={modified.value} mixed={modified.mixed} onChange={(v) => setDate("modified", fromLocalInput(v))} />
        </div>
        {canSetCreated && (
          <div className="field">
            <span className="field-label">Created</span>
            <DateInput label="Created" value={created.value} mixed={created.mixed} onChange={(v) => setDate("created", fromLocalInput(v))} />
          </div>
        )}
        <div className="field inline">
          <span className="field-label">Read-only</span>
          <Toggle label="Read-only" on={readOnly.value === "true"} onChange={(on) => setReadOnly(on)} />
          {readOnly.mixed && <span className="mixed-label">Some files</span>}
        </div>
      </div>
    </section>
  );
}

function DetailsSection({ file }: { file: TagFile }) {
  const rows = [...file.details, ...(file.storage ? [{ label: "Stored as", value: file.storage }] : [])];
  return (
    <section className="editor-section">
      <h3>About this file</h3>
      <dl className="details">
        {rows.map((row) => (
          <div key={row.label}>
            <dt>{row.label}</dt>
            <dd className="selectable">{row.value}</dd>
          </div>
        ))}
        <div>
          <dt>Location</dt>
          <dd>
            <button type="button" className="button link" title={file.path} onClick={() => void reveal(file.path)}>
              {revealLabel()}
            </button>
          </dd>
        </div>
      </dl>
    </section>
  );
}

function EditorFooter({ files }: { files: TagFile[] }) {
  const settings = useSettings();
  const edits = useStore(metadata, (s) => s.edits);
  const dirty = files.some((f) => isDirty(edits[f.path]));
  return (
    <footer className="editor-footer">
      <label className="keep-dates">
        <Toggle label="Keep modified dates when saving tags" on={settings.keepFileDates} onChange={(on) => void updateSettings({ keepFileDates: on })} />
        <span>Keep modified dates</span>
      </label>
      <button type="button" className="button" disabled={!dirty} onClick={() => revert()}>
        Revert
      </button>
    </footer>
  );
}

// MARK: - Patterns

function PatternDialog({ mode, onClose }: { mode: "fromName" | "toName"; onClose: () => void }) {
  const [pattern, setPattern] = useState(fileNamePresets[0]);
  const files = selectedFiles(metadata.get());
  const sample = files.slice(0, 6);
  const error = (() => {
    try {
      tokenize(pattern);
      return null;
    } catch (e) {
      return (e as Error).message;
    }
  })();
  const fromName = mode === "fromName";
  const names = useMemo(() => (fromName || error ? null : namesFromPattern(pattern)), [fromName, error, pattern]);
  const clashes = names ? [...names.values()].filter((n) => n.clash).length : 0;
  const apply = () => {
    if (error) return;
    const done = fromName ? applyFileNamePattern(pattern) : applyNamePattern(pattern);
    onClose();
    if (done < files.length) {
      const missed = files.length - done;
      const why = fromName ? "didn't fit the pattern" : "would have had the same name as another file, or no name at all,";
      metadata.set({ status: `${missed} ${missed === 1 ? "file" : "files"} ${why} and ${missed === 1 ? "was" : "were"} left alone.` });
    }
  };
  const preview = (file: TagFile): ReactNode => {
    if (error) return "";
    if (fromName) {
      const stem = metadata.get().edits[file.path]?.name ?? splitName(file.name).stem;
      const found = tagsFromFileName(stem, pattern);
      if (!found) return "Doesn't fit the pattern";
      return Object.entries(found)
        .map(([field, value]) => `${fieldLabels[field as TagField]}: ${value}`)
        .join(" · ");
    }
    const name = names?.get(file.path);
    if (!name?.stem) return "No tags to name it with";
    return (
      <>
        {name.stem + splitName(file.name).extension}
        {name.clash && <span className="clash"> · same as another file</span>}
      </>
    );
  };
  return (
    <div className="overlay" role="dialog" aria-modal="true" aria-labelledby="pattern-title" onClick={onClose}>
      <div className="pattern-dialog" onClick={(e) => e.stopPropagation()}>
        <h2 id="pattern-title">{fromName ? "Tags from file names" : "Rename from tags"}</h2>
        <p className="secondary">
          {fromName
            ? "Fill in tags from each selected file's name. Use placeholders for the parts that are tags."
            : "Rename each selected file from its tags. Characters file names can't contain are replaced."}
        </p>
        <div className="pattern-row">
          <span className="text-input">
            <input type="text" aria-label="Pattern" spellCheck={false} value={pattern} autoFocus onChange={(e) => setPattern(e.target.value)} onKeyDown={(e) => e.key === "Enter" && apply()} />
          </span>
          <select aria-label="Common patterns" value="" onChange={(e) => e.target.value && setPattern(e.target.value)}>
            <option value="">Common patterns</option>
            {fileNamePresets.map((p) => (
              <option key={p} value={p}>
                {p}
              </option>
            ))}
          </select>
        </div>
        <p className="placeholders secondary">
          %title% %artist% %album% %albumartist% %track% %tracktotal% %disc% %year% %genre% %composer%{fromName ? " %dummy% (ignored)" : ""}
        </p>
        {error && <p className="pattern-error">{error}</p>}
        {clashes > 0 && (
          <p className="pattern-error">
            {clashes} files would end up with the same name, so they'll be left alone. Add %track% or %title% to the pattern to tell them apart.
          </p>
        )}
        <div className="pattern-preview">
          {sample.map((file) => (
            <div key={file.path}>
              <span className="from" title={file.name}>
                {file.name}
              </span>
              <span className="arrow">→</span>
              <span className="to">{preview(file)}</span>
            </div>
          ))}
          {files.length > sample.length && <div className="more secondary">and {files.length - sample.length} more</div>}
        </div>
        <div className="buttons">
          <button type="button" className="button" onClick={onClose}>
            Cancel
          </button>
          <button type="button" className="button prominent" disabled={!!error} onClick={apply}>
            {fromName ? "Fill In Tags" : "Rename"}
          </button>
        </div>
        <p className="secondary small">Nothing is written until you save.</p>
      </div>
    </div>
  );
}
