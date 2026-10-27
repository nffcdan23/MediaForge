# Architecture

MediaForge is a Tauri 2 app. A Rust process owns everything that touches the computer: running tools, files,
the network, settings and history. A React interface runs in the platform's web view:
- WKWebView on macOS;
- WebView2 on Windows.

The interface can only ask the Rust side to do things through a fixed list of typed commands. It hears back
through events.

```
┌──────────────────────── Web view (React + TypeScript) ─────────────────────────┐
│ screens/      Downloads · Conversions · History · Settings · Welcome           │
│ components/   sidebar, cards, job rows, player bar, pickers, overlays          │
│ state/        app store · link-field state machine · preview player · keys     │
│ api/          commands.ts (invoke) · events.ts (listen) · types.ts             │
└──────────────────────────────┬─────────────────────────▲───────────────────────┘
                     commands  │                         │ events
┌──────────────────────────────▼─────────────────────────┴───────────────────────┐
│ app/          commands.rs · state.rs (AppState, queue observer) · menu.rs      │
│               mod.rs (start-up, window, quit, drag and drop, Open With)        │
│               self_test.rs · debug.rs (development only) · local_server.rs     │
├──────────────────────────────────────────────────────────────────────────────────┤
│ engine/       queue.rs (scheduling, cancel, retry) · pipeline.rs · job.rs      │
│               output_files.rs (work folders, destination, no-clobber moves)    │
│ media/        ytdlp.rs · ffmpeg.rs · video.rs (encoders) · metadata.rs         │
│               image.rs (JPG/PNG, in-process) · image_link.rs (picture links)   │
│ process/      runner (timeouts, output caps, tree kill) · env.rs · lines.rs    │
│ store/        settings.json · history.json · legacy.rs (import from Swift app) │
│ support/      url_validator.rs · filename.rs · format.rs                       │
│ tools.rs      locating yt-dlp, FFmpeg, FFprobe (bundled → chosen → installed)  │
│ artwork.rs    thumbnails, video frames, cover art, picture previews, waveforms │
│ licensing.rs  licence/entitlement/activation interfaces (inactive)             │
└──────────────────────────────┬─────────────────────────────────────────────────┘
                               │ argument vectors, never a shell
                     yt-dlp · ffmpeg · ffprobe · deno   (bundled per platform)
```

The engine (everything outside `app/`) has no dependency on the window. The integration tests in
`src-tauri/tests/engine.rs` drive it directly, as the Swift tests drove `MediaForgeCore`.

## Where the Swift code went

| Swift (`MediaForgeCore`) | Rust (`src-tauri/src`) |
| --- | --- |
| `Support/URLValidator`, `FilenameSanitizer`, `Formatters` | `support/url_validator.rs`, `filename.rs`, `format.rs` (+ `src/lib/format.ts`) |
| `Processes/ProcessRunner` | `process/mod.rs`, `tree.rs`, `env.rs`, `lines.rs` |
| `Processes/ToolLocator` | `tools.rs` |
| `Services/YTDLPService` | `media/ytdlp.rs` |
| `Services/FFmpegService` (incl. `VideoConversion`) | `media/ffmpeg.rs`, `media/video.rs` |
| `Models/*` | `models.rs`, `error.rs`, `engine/job.rs`, `media/metadata.rs` |
| `Engine/JobQueue`, `JobPipeline`, `OutputFiles`, `MediaFiles` | `engine/*` |
| `Engine/DownloadsViewModel` | `src/state/downloads.ts`; validation and lookups in Rust |
| `Stores/AppSettings`, `HistoryStore` | `store/settings.rs`, `store/history.rs`, `store/legacy.rs` |
| `Services/Notifier` | `app/state.rs` (notification plugin) |
| `Services/AudioPreviewPlayer`, `Thumbnails` | `src/state/player.ts`, `artwork.rs` |
| `Licensing/*` | `licensing.rs` |
| `MediaForge/Views/*` | `src/screens/*`, `src/components/*` |

## Commands and events

All commands are declared in `src-tauri/build.rs`. Tauri turns each one into a permission, and
`capabilities/main.json` grants exactly those, plus event listening and window dragging, to the main window.
No plugin's JavaScript API is enabled: the file dialog, opener, notification and clipboard plugins are used only
from Rust.

Every command validates its input instead of trusting the web view:
- links go through `url_validator`;
- tool paths through `validate_executable`;
- files the interface asks to open, reveal or play must be outputs the queue or History produced
  (`AppState::is_known_output`);
- thumbnails of local files are limited to media the user added;
- remote artwork is fetched only for addresses that came from yt-dlp, a picture link that was looked up, a
  job or History.

| Event | Payload |
| --- | --- |
| `jobs:changed` | One job's snapshot. Progress is throttled to 8 per second per job. |
| `jobs:removed` | Job IDs |
| `history:changed` | The full history list (at most 1,000) |
| `settings:changed` | Settings |
| `tools:changed` | Tool statuses and versions |
| `app:navigate`, `app:menu`, `app:alert`, `app:debug` | Window-level requests |

## Processes

`process::run` starts a tool by absolute path with an argument vector. It sanitises the environment (fixed
`PATH` and locale; proxy and CA variables passed on; nothing else inherited) and enforces:
- an overall timeout;
- a stall timeout;
- an output cap;
- cancellation.

| | macOS | Windows |
| --- | --- | --- |
| Whole-tree stop | Child leads its own process group; SIGTERM, then SIGKILL after 2.5 s, to the group | Child runs in a Job Object with kill-on-close; `TerminateJobObject`, and closing the job kills stragglers even if MediaForge crashes |
| Priority | `nice 5` | `BELOW_NORMAL_PRIORITY_CLASS` |
| Console windows | n/a | `CREATE_NO_WINDOW`, so no window flashes up |
| Quit with work running | Confirmation dialog, then cancel and wait; SIGTERM handled the same way | Same dialog when the window closes |

yt-dlp always runs with:
- `--ignore-config --no-plugin-dirs --no-cookies --no-cookies-from-browser --xff never`;
- a fixed output template inside a private work folder;
- the link after `--`.

FFmpeg always runs with:
- `-protocol_whitelist file,crypto,data`;
- `file:` paths;
- `-n` (never overwrite);
- `-nostdin`.

## Pictures

Images don't go through yt-dlp or FFmpeg.

- **Converting** (`media/image.rs`) uses the `image` crate, whose decoders are memory-safe Rust, inside
  MediaForge on a worker thread.
  - Files are identified from their first bytes, not their extension.
  - Decoding is capped at 32,768 pixels a side, 120 megapixels and 1 GB of memory.
  - EXIF orientation is applied. The ICC profile is copied when it matches the output's colour space.
  - For JPG, alpha is composited onto white.
  - A file already in the target format is copied unchanged.
- **Picture links** (`media/image_link.rs`). yt-dlp's generic extractor reports any file that isn't a web
  page as a "direct video", so MediaForge checks for pictures itself:
  - Links whose address ends in an image extension are checked before yt-dlp is consulted.
  - Links yt-dlp describes as a generic direct link are checked after it.
  - The check reads only the start of the file.
  - Downloads use a fresh `reqwest` client for each request: no cookies, `MediaForge/<version>` as the user
    agent, certificates verified and five redirects at most.
  - They honour the size and speed limits in Settings, with a 200 MB ceiling and 30-second stall and
    30-minute overall timeouts.
  - The content is checked as it arrives, so a web page or video is abandoned after its first bytes.
  - In a batch of links queued as audio or video, a picture link becomes a picture in the image format from
    Settings, whether the address gives it away or yt-dlp hands back a file that turns out to be one.

## Tools in the installers

| | macOS (`Contents/…`) | Windows (install folder) |
| --- | --- | --- |
| yt-dlp | `Resources/tools/yt-dlp/` (official `yt-dlp_macos.zip`, universal) | `tools\yt-dlp\` (official `yt-dlp_win.zip`) |
| FFmpeg, FFprobe | `MacOS/ffmpeg`, `MacOS/ffprobe`, Tauri sidecars (our LGPL universal build) | `tools\ffmpeg\` (BtbN LGPL shared build: programs plus DLLs) |
| Deno | `MacOS/deno`, sidecar (Deno Land's builds joined) | `deno.exe`, sidecar |
| Licences | `Resources/ThirdPartyNotices/` | `ThirdPartyNotices\` |

Tauri's sidecar mechanism (`externalBin`) is used for the single-file programs. yt-dlp's folder, and FFmpeg's
programs with their DLLs on Windows, are bundled as resources, since a sidecar can only be one file. All are
pinned by SHA-256 in `packaging/tools.lock`. The app looks for each tool in this order:
1. a location the user chose;
2. the bundled copy;
3. well-known install folders (Homebrew and MacPorts; WinGet, Scoop and Chocolatey).

`PATH` is never consulted.

## Data on disk

| | macOS | Windows |
| --- | --- | --- |
| Settings, history | `~/Library/Application Support/uk.co.c-2.MediaForge/` | `%LOCALAPPDATA%\uk.co.c-2.MediaForge\` |
| Thumbnail cache, preview copies | `~/Library/Caches/uk.co.c-2.MediaForge/` | `%LOCALAPPDATA%\uk.co.c-2.MediaForge\` (cache) |
| Work in progress | `$TMPDIR/MediaForge/Jobs/<process id>/` (mode 700) | `%TEMP%\MediaForge\Jobs\<process id>\` |
| Finished files | `~/Downloads/MediaForge` by default | `Downloads\MediaForge` by default |

The first time it runs on a Mac with the Swift app, MediaForge copies that app's settings and history.
Nothing is changed in the old app's files.

## Platform differences

The interface is one codebase. Where the platforms differ:

- **Title bar.** macOS draws its traffic lights over the sidebar, as the Swift app did. Windows has its native
  title bar, kept dark or light to match the app.
- **Shortcuts.** macOS has a menu bar. Windows has the same shortcuts with Ctrl, handled by the window.
- **Wording.** "Finder", "Mac" and "kB" become "File Explorer", "PC" and "KB" on Windows.
- **Video encoder.** VideoToolbox on a Mac. On Windows the first that works of NVENC, Quick Sync, AMF and
  Media Foundation, found with a short test encode and remembered.
- **Activity.** A Dock badge with the pending count on a Mac; taskbar-button progress on Windows.
