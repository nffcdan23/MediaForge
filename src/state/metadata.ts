// The Metadata screen's state: the files added to it, which are selected, and changes not yet saved.
// Nothing is written until Save; the backend then writes each file and sends back how it really is.

import { api, asError } from "../api/commands";
import type { CoverChoice, FileKind, MetadataEdit, TagField, TagFile } from "../api/types";
import { fileNameFromTags, splitName, tagsFromFileName } from "../lib/tagPatterns";
import { showError } from "./actions";
import { app, Store } from "./store";

export type KindFilter = "all" | FileKind;
export type SortKey = "name" | "title" | "artist" | "album" | "track" | "year" | "format";

export type PendingCover = { type: "set"; id: string; preview: string | null; summary: string | null } | { type: "remove" };

export interface PendingEdit {
  fields: Partial<Record<TagField, string>>;
  cover?: PendingCover;
  /** New name without the extension. */
  name?: string;
  modified?: string;
  created?: string;
  readOnly?: boolean;
}

export interface MetadataState {
  files: TagFile[];
  selected: string[];
  /** Where a Shift-click run starts. */
  anchor: string | null;
  /** The file most recently clicked or reached with the arrow keys. */
  focus: string | null;
  edits: Record<string, PendingEdit>;
  filter: KindFilter;
  query: string;
  sort: { key: SortKey; ascending: boolean } | null;
  loading: boolean;
  saving: { done: number; total: number } | null;
  status: string | null;
}

export const metadata = new Store<MetadataState>({
  files: [],
  selected: [],
  anchor: null,
  focus: null,
  edits: {},
  filter: "all",
  query: "",
  sort: null,
  loading: false,
  saving: null,
  status: null,
});

// MARK: - Reading

export function effective(file: TagFile, field: TagField, edits = metadata.get().edits): string {
  return edits[file.path]?.fields[field] ?? file.fields[field] ?? "";
}

export function isDirty(edit: PendingEdit | undefined): boolean {
  if (!edit) return false;
  return Object.keys(edit.fields).length > 0 || edit.cover !== undefined || edit.name !== undefined || edit.modified !== undefined || edit.created !== undefined || edit.readOnly !== undefined;
}

export function dirtyPaths(state = metadata.get()): string[] {
  return state.files.filter((f) => isDirty(state.edits[f.path])).map((f) => f.path);
}

function sortValue(file: TagFile, key: SortKey, edits: Record<string, PendingEdit>): string | number {
  switch (key) {
    case "name":
      return (edits[file.path]?.name ?? splitName(file.name).stem).toLowerCase();
    case "format":
      return file.format.toLowerCase();
    case "track":
      return Number(effective(file, "track", edits)) || Number.MAX_SAFE_INTEGER;
    case "artist":
      return (effective(file, "artist", edits) || effective(file, "authors", edits)).toLowerCase();
    case "year":
      return effective(file, "year", edits) || effective(file, "dateTaken", edits).slice(0, 4);
    default:
      return effective(file, key, edits).toLowerCase();
  }
}

/** The files as the list shows them: filtered, searched and sorted. */
export function visibleFiles(state: MetadataState): TagFile[] {
  const needle = state.query.trim().toLowerCase();
  let files = state.files.filter((f) => state.filter === "all" || f.kind === state.filter);
  if (needle) {
    files = files.filter((f) => [f.name, ...Object.values(f.fields), ...Object.values(state.edits[f.path]?.fields ?? {})].some((v) => v?.toLowerCase().includes(needle)));
  }
  if (state.sort) {
    const { key, ascending } = state.sort;
    files = [...files].sort((a, b) => {
      const x = sortValue(a, key, state.edits);
      const y = sortValue(b, key, state.edits);
      const order = typeof x === "number" && typeof y === "number" ? x - y : String(x).localeCompare(String(y), undefined, { numeric: true });
      return ascending ? order : -order;
    });
  }
  return files;
}

export function selectedFiles(state: MetadataState): TagFile[] {
  const chosen = new Set(state.selected);
  return visibleFiles(state).filter((f) => chosen.has(f.path));
}

// MARK: - The list

function addFiles(added: TagFile[]) {
  if (added.length === 0) return;
  metadata.set((s) => {
    const byPath = new Map(s.files.map((f) => [f.path, f]));
    const fresh: TagFile[] = [];
    for (const file of added) {
      if (!byPath.has(file.path)) fresh.push(file);
      byPath.set(file.path, file);
    }
    const files = [...s.files.map((f) => byPath.get(f.path)!), ...fresh];
    // What was just added is what the user wants to edit next.
    const selected = added.map((f) => f.path);
    return { files, selected, anchor: selected[0] ?? null, focus: selected[0] ?? null, filter: "all", query: "", status: null };
  });
}

export async function chooseFiles(folders: boolean) {
  metadata.set({ loading: true });
  try {
    addFiles(await api.metadataChooseFiles(folders));
  } catch (error) {
    showError(error);
  } finally {
    metadata.set({ loading: false });
  }
}

/** Files dropped on the window while this screen shows (read by the backend). */
export function filesDropped(files: TagFile[]) {
  addFiles(files);
}

function confirmDiscard(paths: string[], title: string, confirmLabel: string, action: () => void) {
  const state = metadata.get();
  const unsaved = paths.filter((p) => isDirty(state.edits[p])).length;
  if (unsaved === 0) return action();
  app.set({
    confirmation: {
      title,
      message: `${unsaved === 1 ? "One file has" : `${unsaved} files have`} changes that haven't been saved. They'll be lost.`,
      confirmLabel,
      destructive: true,
      onConfirm: action,
    },
  });
}

export function removeSelected() {
  const paths = metadata.get().selected;
  if (paths.length === 0) return;
  confirmDiscard(paths, "Remove from the list?", "Remove", () => {
    const doomed = new Set(paths);
    metadata.set((s) => {
      const edits = { ...s.edits };
      paths.forEach((p) => delete edits[p]);
      return { files: s.files.filter((f) => !doomed.has(f.path)), selected: [], anchor: null, focus: null, edits };
    });
    void api.metadataForget(paths);
  });
}

export function clearList() {
  const paths = metadata.get().files.map((f) => f.path);
  confirmDiscard(paths, "Clear the list?", "Clear", () => {
    metadata.set({ files: [], selected: [], anchor: null, focus: null, edits: {}, status: null });
    void api.metadataForget(paths);
  });
}

/** Click, Ctrl/⌘-click (add or take away) and Shift-click (a run of files). */
export function select(path: string, mode: "only" | "toggle" | "range") {
  metadata.set((s) => {
    if (mode === "toggle") {
      const selected = s.selected.includes(path) ? s.selected.filter((p) => p !== path) : [...s.selected, path];
      return { selected, anchor: path, focus: path };
    }
    if (mode === "range" && s.anchor) {
      const order = visibleFiles(s).map((f) => f.path);
      const [a, b] = [order.indexOf(s.anchor), order.indexOf(path)].sort((x, y) => x - y);
      if (a >= 0 && b >= 0) return { selected: order.slice(a, b + 1), focus: path };
    }
    return { selected: [path], anchor: path, focus: path };
  });
}

export function selectAll() {
  metadata.set((s) => ({ selected: visibleFiles(s).map((f) => f.path) }));
}

/** Arrow keys move the selection up and down the list. */
export function moveSelection(step: 1 | -1, extend: boolean) {
  const state = metadata.get();
  const order = visibleFiles(state).map((f) => f.path);
  if (order.length === 0) return;
  const index = state.focus ? order.indexOf(state.focus) : -1;
  const next = order[Math.min(Math.max(index + step, 0), order.length - 1)];
  select(next, extend && state.anchor ? "range" : "only");
}

export function setSort(key: SortKey) {
  metadata.set((s) => ({ sort: s.sort?.key === key ? (s.sort.ascending ? { key, ascending: false } : null) : { key, ascending: true } }));
}

// MARK: - Editing

function updateEdits(paths: string[], change: (edit: PendingEdit, file: TagFile) => PendingEdit) {
  metadata.set((s) => {
    const edits = { ...s.edits };
    for (const path of paths) {
      const file = s.files.find((f) => f.path === path);
      if (!file) continue;
      const next = change({ ...(edits[path] ?? { fields: {} }), fields: { ...(edits[path]?.fields ?? {}) } }, file);
      if (isDirty(next)) edits[path] = next;
      else delete edits[path];
    }
    return { edits, status: null };
  });
}

function setFieldOn(edit: PendingEdit, file: TagFile, field: TagField, value: string): PendingEdit {
  if (!file.editable.includes(field)) return edit;
  if ((file.fields[field] ?? "") === value) delete edit.fields[field];
  else edit.fields[field] = value;
  return edit;
}

/** Sets a field in every selected file that can hold it. */
export function setField(field: TagField, value: string) {
  updateEdits(metadata.get().selected, (edit, file) => setFieldOn(edit, file, field, value));
}

export function setFieldsFor(path: string, fields: Partial<Record<TagField, string>>) {
  updateEdits([path], (edit, file) => {
    for (const [field, value] of Object.entries(fields) as [TagField, string][]) setFieldOn(edit, file, field, value);
    return edit;
  });
}

export function setName(path: string, stem: string) {
  updateEdits([path], (edit, file) => {
    if (stem === splitName(file.name).stem) delete edit.name;
    else edit.name = stem;
    return edit;
  });
}

export function setDate(which: "modified" | "created", iso: string | null) {
  updateEdits(metadata.get().selected, (edit, file) => {
    const original = file[which];
    if (!iso || (original && new Date(original).getTime() === new Date(iso).getTime())) delete edit[which];
    else edit[which] = iso;
    return edit;
  });
}

export function setReadOnly(readOnly: boolean) {
  updateEdits(metadata.get().selected, (edit, file) => {
    if (file.readOnly === readOnly) delete edit.readOnly;
    else edit.readOnly = readOnly;
    return edit;
  });
}

export async function chooseCover() {
  try {
    const choice: CoverChoice | null = await api.metadataChooseCover();
    if (!choice) return;
    updateEdits(metadata.get().selected, (edit, file) => {
      if (file.cover) edit.cover = { type: "set", ...choice };
      return edit;
    });
  } catch (error) {
    showError(error);
  }
}

export function removeCover() {
  updateEdits(metadata.get().selected, (edit, file) => {
    if (!file.cover) return edit;
    if (file.cover.present) edit.cover = { type: "remove" };
    else delete edit.cover;
    return edit;
  });
}

/** Empties every field (and the artwork) of the selected files, as "Remove tags" does in other taggers. */
export function clearAllFields() {
  updateEdits(metadata.get().selected, (edit, file) => {
    for (const field of file.editable) setFieldOn(edit, file, field, "");
    if (file.cover?.present) edit.cover = { type: "remove" };
    return edit;
  });
}

/** Numbers the selected files 1, 2, 3… in list order, with the total. */
export function numberTracks() {
  const files = selectedFiles(metadata.get()).filter((f) => f.editable.includes("track"));
  const total = String(files.length);
  files.forEach((file, index) => setFieldsFor(file.path, { track: String(index + 1), ...(file.editable.includes("trackTotal") ? { trackTotal: total } : {}) }));
}

/** Fills tags from file names. Returns how many names fitted the pattern. */
export function applyFileNamePattern(pattern: string): number {
  const files = selectedFiles(metadata.get());
  let matched = 0;
  for (const file of files) {
    const stem = metadata.get().edits[file.path]?.name ?? splitName(file.name).stem;
    const found = tagsFromFileName(stem, pattern);
    if (!found) continue;
    const usable = Object.fromEntries(Object.entries(found).filter(([field]) => file.editable.includes(field as TagField)));
    if (Object.keys(usable).length === 0) continue;
    matched += 1;
    setFieldsFor(file.path, usable);
  }
  return matched;
}

/** The fields a file will have once saved, for naming it. */
export function effectiveFields(file: TagFile, edits = metadata.get().edits): Partial<Record<TagField, string>> {
  return { ...file.fields, ...(edits[file.path]?.fields ?? {}) };
}

/** The folder a file is in, and its new name in lower case, for spotting two files given the same name. */
function nameKey(file: TagFile, stem: string): string {
  const folder = file.path.slice(0, Math.max(file.path.lastIndexOf("/"), file.path.lastIndexOf("\\")));
  return `${folder}|${(stem + splitName(file.name).extension).toLowerCase()}`;
}

/** The names a pattern gives the selected files. Names two files would share are flagged as clashes. */
export function namesFromPattern(pattern: string): Map<string, { stem: string; clash: boolean }> {
  const files = selectedFiles(metadata.get());
  const stems = files.map((file) => ({ file, stem: fileNameFromTags(effectiveFields(file), pattern) }));
  const counts = new Map<string, number>();
  for (const { file, stem } of stems) if (stem) counts.set(nameKey(file, stem), (counts.get(nameKey(file, stem)) ?? 0) + 1);
  return new Map(stems.map(({ file, stem }) => [file.path, { stem, clash: !!stem && (counts.get(nameKey(file, stem)) ?? 0) > 1 }]));
}

/** Renames the selected files from their tags, leaving alone any that would share a name. Returns how many got one. */
export function applyNamePattern(pattern: string): number {
  let named = 0;
  for (const [path, { stem, clash }] of namesFromPattern(pattern)) {
    if (!stem || clash) continue;
    named += 1;
    setName(path, stem);
  }
  return named;
}

export function revert(paths = metadata.get().selected) {
  metadata.set((s) => {
    const edits = { ...s.edits };
    paths.forEach((p) => delete edits[p]);
    return { edits, status: null };
  });
}

// MARK: - Saving

function toEdit(path: string, edit: PendingEdit): MetadataEdit {
  return {
    path,
    fields: edit.fields,
    cover: edit.cover ? (edit.cover.type === "set" ? { type: "set", id: edit.cover.id } : { type: "remove" }) : undefined,
    name: edit.name,
    modified: edit.modified,
    created: edit.created,
    readOnly: edit.readOnly,
  };
}

export async function save() {
  const state = metadata.get();
  if (state.saving) return;
  const paths = dirtyPaths(state);
  if (paths.length === 0) return;
  // Release any field that has focus, so a value being typed is included.
  (document.activeElement as HTMLElement | null)?.blur?.();
  const latest = metadata.get();
  const edits = paths.map((p) => toEdit(p, latest.edits[p]));
  metadata.set({ saving: { done: 0, total: edits.length }, status: null });
  try {
    const outcomes = await api.metadataSave(edits);
    const failures: string[] = [];
    metadata.set((s) => {
      let files = s.files;
      let selected = s.selected;
      let anchor = s.anchor;
      let focus = s.focus;
      const pending = { ...s.edits };
      for (const outcome of outcomes) {
        const newPath = outcome.file?.path ?? outcome.path;
        if (outcome.file) {
          files = files.map((f) => (f.path === outcome.path ? outcome.file! : f));
        }
        if (outcome.error) {
          failures.push(`${outcome.path.split(/[\\/]/).pop()}: ${outcome.error.message}`);
          continue;
        }
        delete pending[outcome.path];
        if (newPath !== outcome.path) {
          selected = selected.map((p) => (p === outcome.path ? newPath : p));
          if (anchor === outcome.path) anchor = newPath;
          if (focus === outcome.path) focus = newPath;
        }
      }
      const saved = outcomes.length - failures.length;
      const status = saved > 0 ? `Saved ${saved === 1 ? "1 file" : `${saved} files`}.` : null;
      return { files, selected, anchor, focus, edits: pending, status };
    });
    if (failures.length > 0) {
      const shown = failures.slice(0, 6).join("\n");
      const more = failures.length > 6 ? `\n…and ${failures.length - 6} more.` : "";
      app.set({ alert: { title: failures.length === 1 ? "A file couldn't be saved" : `${failures.length} files couldn't be saved`, message: shown + more } });
    }
  } catch (error) {
    app.set({ alert: { title: "Couldn't save", message: asError(error).message } });
  } finally {
    metadata.set({ saving: null });
  }
}

export function saveProgress(progress: { done: number; total: number }) {
  if (metadata.get().saving) metadata.set({ saving: progress });
}

/** Reads the selected files again, dropping their unsaved changes. */
export async function reloadSelected() {
  const paths = metadata.get().selected;
  try {
    const fresh = await api.metadataReload(paths);
    const byPath = new Map(fresh.map((f) => [f.path, f]));
    metadata.set((s) => ({ files: s.files.map((f) => byPath.get(f.path) ?? f) }));
    revert(paths);
  } catch (error) {
    showError(error);
  }
}

// MARK: - Dates for <input type="datetime-local">

export function toLocalInput(iso: string | null | undefined): string {
  if (!iso) return "";
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return "";
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}T${pad(date.getHours())}:${pad(date.getMinutes())}:${pad(date.getSeconds())}`;
}

export function fromLocalInput(value: string): string | null {
  if (!value) return null;
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? null : date.toISOString();
}
