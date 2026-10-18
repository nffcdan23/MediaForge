# MediaForge for macOS and Windows

MediaForge downloads and converts media you're authorised to use: paste a link, pick a format, get an
edit-ready file. Built for video-production work such as pulling licensed music into a project.

This is the cross-platform version of MediaForge. It is built with [Tauri 2](https://tauri.app) (a Rust
engine with a React interface) and runs on:

- **macOS**: Apple silicon and Intel, in one app;
- **Windows**: Windows 11 and Windows 10, 64-bit.

It replaces the macOS-only Swift app in the parent folder, which is kept unchanged for reference.

MediaForge drives two open-source tools, [yt-dlp](https://github.com/yt-dlp/yt-dlp) and
[FFmpeg](https://ffmpeg.org), which are included in the installers. It has no accounts, tracking,
telemetry or advertising.

| | |
| --- | --- |
| Install it | [docs/INSTALLING.md](docs/INSTALLING.md) |
| Build the installers | [docs/BUILDING.md](docs/BUILDING.md) |
| How it is put together | [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) |
| Signing, notarisation, updates, releases | [docs/DISTRIBUTION.md](docs/DISTRIBUTION.md) |
| Licensing and accounts, for later | [docs/COMMERCIAL-ARCHITECTURE.md](docs/COMMERCIAL-ARCHITECTURE.md) |
| What changed from the Swift app | [docs/MIGRATION-REPORT.md](docs/MIGRATION-REPORT.md) |
| What has been tested, and how | [docs/VERIFICATION-REPORT.md](docs/VERIFICATION-REPORT.md) |
| The audit the migration started from | [docs/FEATURE-INVENTORY.md](docs/FEATURE-INVENTORY.md) |

## Using it

**Downloads.**
- Paste a link (⇧⌘V on a Mac, Ctrl+Shift+V on Windows). MediaForge looks it up and shows the title,
  artwork, duration and what's available.
- Choose Audio or Video, set the format and quality, then Download.
- Paste several links, one per line, to queue them all. A playlist or feed link offers every item.

**Conversions.**
- Drop audio or video files or folders onto the window, or use Choose Files… (⌘O / Ctrl+O).
- On a Mac, right-click a file in Finder and choose Open With ▸ MediaForge. On Windows, use Open with ▸
  MediaForge in File Explorer.
- Originals are never changed.

**History.**
- Everything finished, with search and filters.
- Preview audio in the built-in player, preview video, show the file in Finder or File Explorer, or run the
  same job again.

**Settings.** Destination folder, default formats, simultaneous jobs, speed and size limits,
notifications, appearance, and the status of the helper tools.

| Output | Options |
| --- | --- |
| MP3, AAC, M4A | 128, 192, 256 or 320 kbps |
| WAV, FLAC, AIFF | 16- or 24-bit |
| All audio | Source sample rate, 44.1 kHz or 48 kHz |
| MP4 video | Best available, or capped at 4K (2160p) / 1440p / 1080p / 720p / 480p / 360p |

**4K.** Above 1080p, large sites usually offer only VP9 or AV1 video, which most players and editing apps
can't open. MediaForge converts those downloads to H.264 (or HEVC) with the computer's own video encoder:
- VideoToolbox on a Mac;
- NVIDIA, Intel or AMD hardware on a Windows PC, or Windows' built-in encoder when there's none.

Only download media you own, have licensed, or that the source permits you to download. MediaForge does not
bypass DRM, sign-in requirements, paywalls or regional restrictions, and passes yt-dlp no cookies or
credentials.

## Project layout

```
src/                     React + TypeScript interface (Vite)
  api/                   Typed bindings to the Rust commands and events
  state/                 Stores: app data, link field, preview player, shortcuts
  components/, screens/  The interface, recreated from the SwiftUI views
  styles/                Palette and layout
src-tauri/               Rust
  src/engine/            Job queue, download/convert pipeline, output files
  src/media/             yt-dlp and FFmpeg drivers, metadata, video conversion rules
  src/process/           Subprocess runner (process groups / Job Objects), sanitised environment
  src/store/             Settings and history (and the one-time import from the Swift app)
  src/support/           Link validation, file-name sanitising, formatting
  src/licensing.rs       Licence interfaces (inactive: nothing is gated)
  src/app/               Tauri: commands, events, menus, window, self-test, debug hooks
  tests/engine.rs        End-to-end tests with the real tools
  tauri*.conf.json       App identity, window, security policy, bundle settings
  capabilities/          Exactly what the window may call
packaging/               Tool manifest (pinned checksums), bundle configs, notices, DMG layout
scripts/                 Fetch tools, build FFmpeg (macOS), package macOS/Windows, sign, verify
.github/workflows/       CI (tests on Windows and macOS) and installer builds
docs/                    Everything above
tools/, dist/            Generated: downloaded tools and packaged output (not in version control)
```

## Developing

You need Node.js 20+ and Rust (via [rustup](https://rustup.rs)). On Windows you also need the Visual Studio
C++ build tools, as for any Tauri app.

```bash
npm install
npm run tauri dev
```

This opens MediaForge with live reloading. Development builds use yt-dlp and FFmpeg installed on the
computer (for example `brew install yt-dlp ffmpeg`, or `winget install yt-dlp.yt-dlp Gyan.FFmpeg`).
Packaged builds carry their own.

Tests:

```bash
npm run typecheck && npm test          # interface
cd src-tauri && cargo test             # engine: unit tests, then end-to-end tests with the real tools
```

End-to-end tests skip themselves when a tool isn't installed. `MEDIAFORGE_TEST_TOOLS` can name the folders to
use. Development builds can be driven by environment variables (`src-tauri/src/app/debug.rs`). Release builds
include a `--self-test <report.json>` mode that checks an installed copy end to end
(`src-tauri/src/app/self_test.rs`).

## Building the installers

```bash
scripts/package-macos.sh                      # macOS: dist/macos/MediaForge.dmg (universal)
pwsh scripts/package-windows.ps1              # Windows: dist\windows\MediaForge-Setup.exe
```

Or run **Actions ▸ Build installers** on GitHub, which builds both and tests the Windows installer on a real
Windows machine. See [docs/BUILDING.md](docs/BUILDING.md).
