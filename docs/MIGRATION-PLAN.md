# Migration plan

Written 18 October 2026, after the audit in [FEATURE-INVENTORY.md](FEATURE-INVENTORY.md). This plan is based
on the Swift code as it stands, not on assumptions about it.

## Principles

1. **The Swift engine is ported, not reinvented.** Every rule in `MediaForgeCore` has been tuned and tested, so
   it moves to Rust function by function:
   - validation;
   - sanitising;
   - yt-dlp and FFmpeg arguments;
   - progress parsing;
   - error classification;
   - stream-copy rules;
   - video bitrate rules;
   - queue behaviour.

   The Swift tests are ported alongside.
2. **The UI is recreated, not redesigned.** It keeps the same screens, hierarchy, copy, sizes, colours,
   gradient, cards, badges, progress bar, player bar and Welcome sheet. Swift's point sizes map to CSS
   pixels 1:1.
3. **Rust owns behaviour; React owns presentation.** Jobs, history, settings and tools live in the Rust
   backend. The frontend sends typed commands and receives typed events. The webview can't run processes,
   read arbitrary files or reach the network.
4. **No mocks in the product.** Every button calls real code. Tests use real tools where the Swift tests did.

## Architecture

```
React + TypeScript (Vite)                     Rust (Tauri 2)
  screens/   Downloads, Conversions,            commands.rs   typed command handlers
             History, Settings, Welcome          engine/       queue, pipeline, jobs (tokio)
  components/ sidebar, cards, job row,           media/        yt-dlp, FFmpeg, ffprobe, metadata, video rules
             player bar, pickers                 process/      runner: process groups (Unix), Job Objects (Windows)
  state/     store fed by events  ── invoke ──▶  tools.rs      bundled → user choice → well-known folders
  api/       typed bindings       ◀── events ──  store/        settings.json, history.json
                                                 support/      URL validation, filenames, formatting
                                                 media_preview, thumbnails, waveform
                                                 licensing/    inactive interfaces (as in Swift)
```

## Platform equivalents

| macOS-only in Swift | Tauri, macOS | Tauri, Windows |
| --- | --- | --- |
| AVAudioPlayer preview | `<audio>` via Tauri's asset protocol, one file at a time | Same; formats WebView2 can't play (AIFF) get a temporary WAV made with FFmpeg |
| AVFoundation waveform | FFmpeg decodes to low-rate mono PCM; Rust computes 180 peaks | Same |
| Quick Look (video) | In-app video preview window, plus Open in default app | Same |
| QuickLook thumbnails / embedded art | FFmpeg extracts a frame, or the cover art, into a cached JPEG | Same |
| Finder reveal | `reveal_item_in_dir` (opener plugin) | Same (Explorer, with the file selected) |
| NSOpenPanel | Native dialogs (dialog plugin, called from Rust) | Same |
| UserDefaults | JSON in app-data folder; settings migrate once from the Swift app | JSON in `%LOCALAPPDATA%` |
| Dock badge | `set_badge_count` | Taskbar progress via `set_progress_bar` (also on macOS) |
| Native menus | Native menu bar with the same shortcuts | Same shortcuts handled in the window (no menu bar) |
| Process groups + SIGTERM | `process_group(0)`, kill(-pgid) | Job Object with kill-on-close; `CREATE_NO_WINDOW` |
| VideoToolbox | h264/hevc_videotoolbox | First working of NVENC, Quick Sync, AMF, Media Foundation |
| Notifications | notification plugin | Same (toast) |
| Open With / Dock drop | `RunEvent::Opened` file URLs | File paths passed on the command line (single-instance) |
| Liquid Glass | CSS backdrop blur over the ambient gradient; native traffic lights over a transparent title bar | Same CSS; native dark title bar |

## Packaging

- **macOS.** Build a universal Tauri app. Bundle the same pinned tools as the Swift build:
  - FFmpeg/FFprobe and Deno as Tauri sidecars (`externalBin`);
  - yt-dlp's folder as a resource.

  Sign ad hoc for internal builds. Make the DMG with dmgbuild (no Finder scripting). Port the Developer ID
  signing script; it is used only once credentials exist.
- **Windows.** Build `x86_64-pc-windows-msvc` with a static CRT, packaged with NSIS as `MediaForge-Setup.exe`:
  - per-user install with Start Menu entry and uninstaller;
  - WebView2 bootstrapper embedded;
  - pinned tools (yt-dlp folder, FFmpeg LGPL shared build, Deno) as resources and sidecars.
- **CI** (GitHub Actions, private repository):
  - lint and test on macOS and Windows;
  - build the DMG and the installer;
  - on Windows, install silently, run the installed app's self-test, take a screenshot, then uninstall.
  - Artifacts only, no releases.

## Order of work

1. Project skeleton, Tauri config, capabilities.
2. Rust engine and unit tests.
3. Frontend.
4. Tools and packaging.
5. Local macOS build and end-to-end checks with real tools.
6. GitHub CI for Windows.
7. Documentation and reports.
