# Migration report: Swift app → Tauri app

**In short.** Everything the macOS app did, the new app does, on macOS and on Windows. It looks the same: the
screens, wording, colours and layout were recreated from the SwiftUI views and checked against the Swift
app's screenshots.

Where a macOS-only feature had no Windows equivalent (Quick Look, the Dock badge, VideoToolbox), the new app
does the same job another way on both platforms, listed below. One small thing is lost: links can no longer be
dragged in from a web browser; pasting does the same. The new app also adds a few things the Swift app didn't
have.

The Swift app (`MediaForge.xcodeproj` in the parent folder) is unchanged. The new app lives in
`MediaForge-Tauri/`, in its own private Git repository.

## Technology

| | Swift app (1.0.0, build 3) | Tauri app (1.1.0, build 4) |
| --- | --- | --- |
| Platforms | macOS 26+ (Apple silicon, Intel) | macOS 12+ (Apple silicon, Intel, one app); Windows 10/11 x64 |
| Interface | SwiftUI, Liquid Glass | React 19 + TypeScript in the system web view; glass recreated in CSS |
| Engine | Swift (`MediaForgeCore`) | Rust (`src-tauri/src`), ported function by function |
| Tests | 154 Swift tests | 107 Rust unit tests + 15 end-to-end Rust tests + 7 interface tests + an installed-app self-test (details in [VERIFICATION-REPORT.md](VERIFICATION-REPORT.md)) |
| Installer | DMG | DMG (universal); `MediaForge-Setup.exe` (NSIS) |
| Build | Xcode + scripts | npm + Cargo + scripts; GitHub Actions for both platforms |

The new app's minimum macOS is 12, not 26: the Swift app needed macOS 26 only for its Liquid Glass controls.
macOS 12–25 is configured but untested (no older Mac or runner was available).

## Feature by feature

✔ = same behaviour · ≈ same job done differently · ✚ = new

| Feature (from the [inventory](FEATURE-INVENTORY.md)) | Status | Notes |
| --- | --- | --- |
| Link field: paste, several links, validation before any network access, 200-link cap | ✔ | Validation ported with its tests; runs in Rust |
| Debounced lookup, preview card, playlist and batch cards, error card with retry | ✔ | Same states, wording and layout |
| Source badge, "UP TO …P", audio-only, quality limited to what the source offers | ✔ | |
| Conversion note for VP9/AV1-only qualities | ✔ | Wording adapted on Windows ("most players and editing apps") |
| Audio: MP3, WAV, AAC, M4A, FLAC, AIFF; 128–320 kbps; 16/24-bit; sample rates | ✔ | Same FFmpeg arguments; stream copy when it can't lose quality |
| Audio extraction from video, tags (title, artist, source link) | ✔ | |
| Video: MP4 at a chosen quality, best audio, H.264/AAC preferred | ✔ | Same yt-dlp selectors and sort |
| 4K: VP9/AV1 converted to H.264 or HEVC, or kept | ≈ | VideoToolbox on Mac; NVENC, Quick Sync, AMF or Windows' own Media Foundation encoder on Windows, chosen by a test encode and retried with Media Foundation if a GPU encoder fails |
| Progress: percent, speed, time left, conversion speed; multi-part weighting | ✔ | Same parser and aggregator |
| Queue: concurrency 1–6, cancel, cancel all, retry, remove, clear finished | ✔ | |
| Whole-process-tree cancellation | ✔/✚ | Process groups on Mac; Job Objects on Windows, which also clean up if MediaForge itself crashes |
| Private work folders, destination checks, never overwrite, "Name 2.mp3" | ✔ | Name claimed atomically; falls back to copying across disks |
| Downloads carry the "downloaded from the internet" mark | ✔/✚ | Quarantine attribute on Mac; Mark of the Web on Windows |
| Conversions: drop files or folders, Choose Files…, 39 file types | ✔ | Plus **Choose Folder…** (✚), since Windows' file dialog can't pick both at once |
| Open With ▸ MediaForge | ✔ | Finder on Mac; Explorer on Windows, registered without becoming the default for anything |
| History: search, filters, by day, preview, show, again, copy link/path, remove, clear | ✔ | |
| Audio preview player with waveform scrubber | ≈ | System web audio instead of AVAudioPlayer; waveform computed with FFmpeg instead of AVFoundation; formats WebView2 can't decode (AIFF) are played from a temporary WAV |
| Quick Look for video | ≈ | An in-app video preview, plus Open (default player) and Show in Finder/File Explorer, on both platforms |
| Thumbnails: source artwork, video frames, embedded cover art | ≈ | Remote art fetched by Rust without cookies (as before); frames and cover art extracted with FFmpeg instead of Quick Look |
| Show in Finder | ✔ | And Show in File Explorer on Windows (file selected) |
| Settings, persisted, with lenient reading and clamping | ✔ | JSON file instead of UserDefaults |
| Settings and history carried over | ✚ | First launch on a Mac imports the Swift app's settings and history (it doesn't change them) |
| Tools: status, versions, choose location, find automatically, show | ✔ | Install hints per platform (Homebrew / WinGet) |
| Welcome sheet (responsible use), Review… | ✔ | |
| Notifications on finish/failure with "Show in Finder" | ✔ | "Show in File Explorer" on Windows |
| Dock badge with pending count | ≈ | Dock badge on Mac; taskbar-button progress on Windows |
| Menus and shortcuts (⌘O, ⇧⌘V, ⇧⌘D, ⌘1–4, ⌘,, ⇧⌘P, Cancel All, Clear Finished, Responsible Use, Check Tools) | ✔ | Native menu bar on Mac; the same shortcuts with Ctrl on Windows. Plus Import Folder… (⇧⌘O) |
| Closing the window keeps the queue running (Mac); confirm before quitting with work running | ✔ | On Windows, closing the window quits after the same confirmation, as Windows users expect |
| SIGTERM stops helpers | ✔ | |
| Dark (default), light, match system | ✔ | Windows' title bar follows the choice |
| Sidebar badges, tool status footer | ✔ | Sidebar can be collapsed, as with NavigationSplitView |
| Drag a **link** from a browser into the window | ✘ | Not migrated: Tauri's drop handling delivers only files, and turning it off would lose file paths for conversions, which matter more. Pasting (⇧⌘V / Ctrl+Shift+V) does the same |
| Licensing interfaces (inactive) | ✔ | Same five interfaces as Rust traits; nothing gated |
| Self-test of an installed copy | ✚ | `--self-test report.json`: real conversions, downloads, video, cancellation, all offline |
| Debug automation for development builds | ✔ | Same idea (environment variables), compiled out of release builds |

## Security, compared

All of the Swift app's protections are kept:
- argument vectors, never a shell;
- links after `--`;
- yt-dlp without config, plugins, cookies or forged headers;
- FFmpeg limited to local files;
- sanitised environment;
- timeouts, stall detection and output caps;
- process-tree cleanup;
- remote titles never choosing paths;
- validated tool locations;
- TLS left on;
- no telemetry.

The web interface adds an attack surface the Swift app didn't have, so:

- The window can call only MediaForge's own 35 commands, each listed as a permission (`capabilities/main.json`).
  No plugin JavaScript API is enabled: no file system, shell, HTTP or clipboard from the web page.
- Commands re-validate everything. Files to open, reveal, preview or thumbnail must be ones the app produced or
  the user added.
- A strict Content Security Policy: scripts only from the app; images and media only from the app, inline data
  and the asset protocol.
- The asset protocol starts with an empty scope and is opened one file at a time, for preview.
- Remote titles and messages are rendered as text, never HTML.

## Known limitations

1. **Not code-signed** on either platform yet. That needs C2 Software's Apple and Windows signing identities
   ([DISTRIBUTION.md](DISTRIBUTION.md)).
2. **Link drag-and-drop** from browsers (above).
3. **macOS 12–25 untested.** The build targets macOS 12, but only macOS 26 and 27 were available.
4. **Windows 10 untested.** GitHub's Windows runners are Windows Server 2025. A Windows 11 PC is the
   remaining real-world check.
5. **AV1 on Mac.** The bundled macOS FFmpeg can't decode AV1, as before, so when converting MediaForge prefers
   VP9 downloads at the same resolution. The Windows FFmpeg can decode AV1.
6. **Windows video conversion speed** depends on the PC. NVIDIA, Intel and AMD encoders are fast. Windows'
   own encoder, used when none of those works, is slower for 4K.
7. **Automatic updates** are designed but not built ([DISTRIBUTION.md](DISTRIBUTION.md#updates)).
