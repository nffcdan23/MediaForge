// File names to tags and back, with patterns such as "%artist% - %title%", as taggers like Mp3tag use them.

import type { TagField } from "../api/types";

/** Placeholders a pattern may use. `%dummy%` matches text that is thrown away. */
export const placeholders: Record<string, TagField | null> = {
  title: "title",
  artist: "artist",
  album: "album",
  albumartist: "albumArtist",
  track: "track",
  tracktotal: "trackTotal",
  disc: "disc",
  year: "year",
  genre: "genre",
  composer: "composer",
  comment: "comment",
  dummy: null,
};

export const fileNamePresets = ["%artist% - %title%", "%track% - %title%", "%track% %artist% - %title%", "%artist% - %album% - %track% - %title%", "%title%"];

type Token = { literal: string } | { field: TagField | null };

/** Splits a pattern into literal text and placeholders. Throws with a readable message for unknown ones. */
export function tokenize(pattern: string): Token[] {
  const tokens: Token[] = [];
  const re = /%([a-z]+)%/gi;
  let last = 0;
  for (const match of pattern.matchAll(re)) {
    const index = match.index ?? 0;
    if (index > last) tokens.push({ literal: pattern.slice(last, index) });
    const name = match[1].toLowerCase();
    if (!(name in placeholders)) throw new Error(`“%${match[1]}%” isn't a placeholder MediaForge knows.`);
    tokens.push({ field: placeholders[name] });
    last = index + match[0].length;
  }
  if (last < pattern.length) tokens.push({ literal: pattern.slice(last) });
  return tokens;
}

const escapeRegExp = (text: string) => text.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

/** Track and disc numbers as plain numbers: "03" is 3, "3/12" is 3 of 12. Text that isn't a number is dropped. */
function normaliseNumber(value: string): { number?: string; total?: string } {
  const match = value.trim().match(/^0*(\d+)(?:\s*\/\s*0*(\d+))?$/);
  if (!match || match[1] === "") return {};
  return { number: String(Number(match[1])), total: match[2] ? String(Number(match[2])) : undefined };
}

/**
 * Reads tags out of a file name (without its extension). Returns null when the name doesn't fit the pattern.
 */
export function tagsFromFileName(stem: string, pattern: string): Partial<Record<TagField, string>> | null {
  const tokens = tokenize(pattern);
  if (!tokens.some((t) => "field" in t)) return null;
  const fields: (TagField | null)[] = [];
  const source = tokens
    .map((token) => {
      if ("literal" in token) return escapeRegExp(token.literal);
      fields.push(token.field);
      return "(.+?)";
    })
    .join("");
  const match = stem.match(new RegExp(`^${source}$`, "s"));
  if (!match) return null;
  const result: Partial<Record<TagField, string>> = {};
  fields.forEach((field, index) => {
    const value = match[index + 1].trim();
    if (!field || !value) return;
    if (field === "track" || field === "disc") {
      const { number, total } = normaliseNumber(value);
      if (number) result[field] = number;
      if (total) result[field === "track" ? "trackTotal" : "discTotal"] = total;
    } else if (field === "trackTotal") {
      const { number } = normaliseNumber(value);
      if (number) result.trackTotal = number;
    } else {
      result[field] = value;
    }
  });
  return result;
}

/** Characters no file name may contain, as the backend replaces them. */
export function cleanFileName(name: string): string {
  const replaced = name
    .replace(/[/\\|]/g, "-")
    .replace(/:/g, " -")
    .replace(/[?*]/g, "")
    .replace(/"/g, "'")
    .replace(/</g, "(")
    .replace(/>/g, ")")
    // eslint-disable-next-line no-control-regex
    .replace(/[\u0000-\u001f\u007f]/g, " ");
  return replaced
    .replace(/\s+/g, " ")
    .replace(/^[\s.\-_,]+|[\s.\-_,]+$/g, "")
    .trim();
}

/** A file name (without extension) made from tags. Track numbers get a leading zero so files sort in order. */
export function fileNameFromTags(fields: Partial<Record<TagField, string>>, pattern: string): string {
  const tokens = tokenize(pattern);
  const text = tokens
    .map((token) => {
      if ("literal" in token) return token.literal;
      if (!token.field) return "";
      const value = (fields[token.field] ?? "").trim();
      if ((token.field === "track" || token.field === "disc") && /^\d+$/.test(value)) {
        const total = Number(fields[token.field === "track" ? "trackTotal" : "discTotal"] ?? 0);
        return value.padStart(token.field === "track" ? (total >= 100 ? 3 : 2) : 1, "0");
      }
      return value;
    })
    .join("");
  return cleanFileName(text);
}

/** The file name without its extension, and the extension with its dot. */
export function splitName(name: string): { stem: string; extension: string } {
  const dot = name.lastIndexOf(".");
  if (dot <= 0) return { stem: name, extension: "" };
  return { stem: name.slice(0, dot), extension: name.slice(dot) };
}
