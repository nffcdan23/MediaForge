# MediaForge for macOS and Windows

MediaForge downloads and converts media you're authorised to use: paste a link, pick a format, get an
edit-ready file. Built for video-production work such as pulling licensed music into a project. It also turns
pictures (WebP, PNG, JPEG, GIF, BMP, TIFF), downloaded or already on the computer, into JPG or PNG, and edits
the tags and details stored in files of any kind, as a tagger such as Mp3tag does.

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
- A link straight to a picture (a `.webp` file, say) shows the picture instead: choose JPG or PNG, then
  Download.
- Paste several links, one per line, to queue them all. A playlist or feed link offers every item.

**Conversions.**
- Drop audio, video or image files or folders onto the window, or use Choose Files… (⌘O / Ctrl+O). Audio
  and video use the audio settings; pictures use the image settings.
- On a Mac, right-click a file (including a `.webp` picture) in Finder and choose Open With ▸ MediaForge.
  On Windows, use Open with ▸ MediaForge in File Explorer.
- Originals are never changed.

**Metadata.**
- Add files or folders of any kind with the + buttons, or drop them on the window while Metadata is showing.
- Select one file or many (Ctrl/⌘-click, Shift-click, Ctrl/⌘+A, arrow keys) and change fields in the panel on
  the right. Where the selected files differ, a field says "Multiple values" and is left alone unless you
  type in it.
- What can be changed:
  - audio (MP3, M4A, FLAC, WAV, AIFF, Ogg, Opus and more): title, artist, album, album artist, track and disc,
    year, genre, composer, comment and artwork;
  - MP4 and MOV video: title, artist, album, year, genre, comment, description, copyright and artwork;
  - MKV, WebM, AVI and WMV: their tags, through FFmpeg;
  - photos (JPEG, PNG, WebP, TIFF, HEIC): title, subject, rating, tags, comments, authors, copyright and date
    taken, the fields File Explorer shows;
  - PDF, Word, Excel and PowerPoint files: title, author, subject and keywords (and category and comments
    for Office files);
  - every file: its name, modified and created dates, and Read-only.
- Tools (the wand): tags from file names (`%artist% - %title%`), rename from tags, number tracks in list order,
  clear all fields.
- Nothing is written until Save (⌘S / Ctrl+S). MP3s are saved with ID3v2.3 tags and front-cover artwork,
  which Spotify's local files, Windows and car stereos read. Each file is written to a hidden copy beside it
  that then replaces it, so an interrupted save can't damage it.

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
| JPG | Maximum, high, medium or low quality; transparent areas become white |
| PNG | Lossless, keeps transparency |

**Pictures.** MediaForge reads a picture by what's inside it, not its name (browsers sometimes save a JPEG
as `.webp`), turns phone photos the right way up and keeps their colour profile. An animation becomes its
first frame. A picture that's already in the chosen format is kept exactly as it is. This needs neither
yt-dlp nor FFmpeg.

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
  src/media/             yt-dlp and FFmpeg drivers, metadata, video conversion rules, images and image links
  src/process/           Subprocess runner (process groups / Job Objects), sanitised environment
  src/store/             Settings and history (and the one-time import from the Swift app)
  src/support/           Link validation, file-name sanitising, formatting
  src/tags/              Metadata screen: audio and video tags, photo EXIF and XMP, PDF and Office properties,
                         names and dates (safe save: hidden copy, then swap)
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
frontend-dist/           Generated: the built interface, bundled into the app
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
