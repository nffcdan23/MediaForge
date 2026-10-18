// Words and keys that differ between macOS and Windows.

import type { Platform } from "../api/types";

let current: Platform = navigator.userAgent.includes("Windows") ? "windows" : navigator.userAgent.includes("Mac") ? "macos" : "linux";

export function setPlatform(platform: Platform) {
  current = platform;
}

export const platform = () => current;
export const isMac = () => current === "macos";

/** "Mac" or "PC", for sentences such as "on this Mac". */
export const computerName = () => (isMac() ? "Mac" : "PC");

export const fileManagerName = () => (isMac() ? "Finder" : current === "windows" ? "File Explorer" : "Files");

export const revealLabel = () => `Show in ${fileManagerName()}`;

/** A shortcut for display: "⇧⌘V" on a Mac, "Ctrl+Shift+V" elsewhere. */
export function shortcut(key: string, shift = false): string {
  if (isMac()) return `${shift ? "⇧" : ""}⌘${key.toUpperCase()}`;
  return `Ctrl+${shift ? "Shift+" : ""}${key.toUpperCase()}`;
}

export const previewLabel = () => (isMac() ? "Quick Look" : "Preview");
