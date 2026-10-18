# Verification report

What was built, compiled and tested, and what remains unverified. Written 18 October 2026.

**Rule followed throughout:** nothing is described as working unless it was seen working. "Configured" means
set up but not exercised.

## Where the installers are

| Installer | Location | Built on |
| --- | --- | --- |
| `MediaForge.dmg` (universal, internal test build) | `MediaForge-Tauri/dist/macos/MediaForge.dmg` (144 MB; SHA-256 `d7dada58…` in `.sha256` beside it) | This Mac |
| `MediaForge.dmg` (same, from CI) | GitHub ▸ nffcdan23/MediaForge ▸ Actions ▸ Build installers ▸ artifact `MediaForge-macOS` | GitHub macOS 26 runner |
| `MediaForge-Setup.exe` | `MediaForge-Tauri/dist/windows/MediaForge-Setup.exe` (95 MB; SHA-256 in `.sha256` beside it), downloaded from the CI artifact `MediaForge-Windows`, with its installation log and screenshots in `dist/windows/verification` | GitHub Windows Server 2025 runner |

The Windows installer was built, installed, self-tested and uninstalled on GitHub's Windows machine. The copy
in `dist/windows` is that tested artifact; its SHA-256 is beside it.

## What was implemented

- **Rust engine:** a port of `MediaForgeCore`; the Rust side is about 10,000 lines including tests.
  - link validation and file-name sanitising;
  - subprocess runner with process-group (macOS) and Job Object (Windows) tree control;
  - tool discovery;
  - yt-dlp and FFmpeg drivers, progress parsers and error classifier;
  - video re-encoding rules with per-platform encoder selection;
  - job queue and pipeline, output files, settings, history, the Swift-app import, artwork and waveforms;
  - inactive licensing.
- **Tauri shell:** 35 typed commands, events, the macOS menu, window behaviour, drag and drop, Open With,
  notifications with a reveal action, Dock badge / taskbar progress, SIGTERM handling, a release-build
  self-test and development-only automation.
- **React interface:** the Downloads, Conversions, History, Settings and Welcome screens, the preview
  player, video preview, menus and dialogs, recreated from the SwiftUI views.
- **Packaging:**
  - pinned, checksum-verified tool fetching for both platforms;
  - the macOS LGPL FFmpeg build;
  - universal macOS app and DMG;
  - Windows NSIS installer with installer artwork and Open With registration;
  - the Developer ID signing and notarisation script (not run);
  - GitHub Actions for CI and installers.
- **Documentation:** in `docs/`.

## What was compiled

| Target | Result |
| --- | --- |
| Interface: TypeScript 7 strict type check, Vite production build | ✔ This Mac and GitHub (Ubuntu) |
| Rust, debug and release, `aarch64-apple-darwin` | ✔ This Mac, no warnings (rustc 1.99.1) |
| Rust, release, `x86_64-apple-darwin`, joined into a universal app | ✔ This Mac (and GitHub macOS) |
| Rust, `x86_64-pc-windows-msvc`, including all Windows-only code (Job Objects, Win32 calls) | ✔ GitHub Windows: `cargo clippy` and tests compiled |
| `cargo fmt --check`, `cargo clippy --all-targets` | ✔ No warnings |

## What was tested

### Automated tests

| Suite | macOS (this Mac) | Windows (GitHub) |
| --- | --- | --- |
| Rust unit tests (107): validation, sanitising, arguments, parsing, error mapping, settings, history, Swift-app import, licensing, process runner | ✔ 107/107 | ✔ 98/98 (the Unix-only process tests are replaced by Windows ones: exit codes, timeout, Job Object tree cancellation) |
| Rust end-to-end tests (15) with real FFmpeg and yt-dlp: every audio format, numbered names, concurrency limit, awkward names, missing/corrupt/silent files, retry after fix, missing tool, unusable destination, cancel while waiting, link lookup, downloads to MP3 and FLAC with tags, VP9 → H.264 video, audio-only source asked for video, 404 and unreachable server, size limit, cancel mid-download | ✔ 15/15 (Homebrew FFmpeg 9.0.2, yt-dlp 2026.08.19) | ✔ 15/15, none skipped, with the bundled Windows FFmpeg (BtbN 9.0.2 LGPL) and yt-dlp; the VP9 → H.264 test used Windows' encoder |
| Interface unit tests (7): formatting, job status lines, conversion note | ✔ 7/7 | ✔ (Ubuntu) |

### The packaged macOS app (`dist/macos/MediaForge.app`, release build, bundled tools only)

`MediaForge --self-test` on this Mac (macOS 27.0.1, Apple silicon) passed every check:
- the window and interface loaded;
- all three tools started from inside the app;
- Deno is bundled;
- conversion to MP3, WAV 24-bit, AAC, M4A, FLAC and AIFF, each confirmed with ffprobe;
- a 180-peak waveform;
- link lookup and an audio download through the bundled yt-dlp;
- cancellation mid-download, leaving nothing behind;
- licensing inactive with every feature available.

The video check was skipped: the bundled LGPL FFmpeg has no test-pattern generator to make a VP9 clip. Video
conversion was tested instead in the end-to-end tests and the interface run below.

The same checks passed on GitHub with the CI-built disk image:
- on an Apple silicon Mac (macOS 26);
- **on an Intel Mac** (`macos-26-intel`), after installing from the disk image. That proves the Intel half of
  the universal app and of every bundled tool, which can't be run on this Mac.

Also checked on this Mac:
- every program inside the app is universal (arm64 + x86_64);
- the bundled FFmpeg's configuration has no GPL or non-free components;
- `codesign --verify --deep --strict` passes;
- the DMG verifies (`hdiutil verify`);
- the signing rehearsal (`sign-and-notarize-macos.sh --rehearse`) signed all 106 yt-dlp libraries and every
  tool with the hardened runtime, and all tools still ran afterwards.

### The interface, driven on this Mac

A development build was run behind other windows with scratch folders, so no real settings or files were
used. It was photographed at each step and compared with the Swift app's screenshots:

| Check | Result |
| --- | --- |
| First launch shows the Welcome sheet | ✔ |
| Downloads, Conversions, History, Settings match the Swift layouts (sidebar, toolbar, cards, pickers, badges, footer) | ✔ |
| Real link (Wikimedia Commons, which permits downloading): lookup, source badge, "UP TO 2250P", conversion note | ✔ |
| Real download (Commons `Example.ogg`) → MP3 320 kbps, tagged with title, artist and source link, quarantine mark set | ✔ |
| 4K VP9 download from a local server → 3840×2160 H.264 + AAC MP4 via VideoToolbox; finished row shows a video frame | ✔ |
| Three conversions (video and WAVs) with live progress ("70% · 138×"), then the finished rows | ✔ |
| Several links → batch card, invalid line flagged | ✔ |
| Missing file → "Media unavailable" error card with Try Again | ✔ |
| History grouped by day with metadata; filter and search controls | ✔ |
| Audio preview through the asset protocol: playback advances, waveform drawn, row shows pause | ✔ (muted) |
| Video preview window: plays, with Open / Show in Finder | ✔ (timer advanced; the window capture can't show video frames) |
| Light appearance | ✔ |

### Windows

`scripts/verify-windows-install.ps1` ran on GitHub's Windows Server 2025 machine against
`MediaForge-Setup.exe`, doing what a person would. Full log:
`dist/windows/verification/verification.txt`.

| Check | Result |
| --- | --- |
| Silent install for the current user (no administrator rights) | ✔ installed to `%LOCALAPPDATA%\MediaForge`, version 1.1.0, publisher C2 Software |
| Files: MediaForge.exe, deno.exe, tools\yt-dlp, tools\ffmpeg (with DLLs), licence notices | ✔ |
| Start Menu and desktop shortcuts | ✔ |
| "Open with" registered for 17 audio/video types, not as default | ✔ |
| Installed app's self-test: window loaded (WebView2) | ✔ |
| Bundled yt-dlp 2026.08.19, FFmpeg n9.0.2, FFprobe start; Deno bundled | ✔ |
| Conversion to MP3, WAV 24-bit, AAC, M4A, FLAC, AIFF (each checked with ffprobe) | ✔ |
| Waveform, link lookup, audio download through yt-dlp | ✔ |
| VP9 download converted to H.264, with **Windows' Media Foundation encoder** (no GPU on the runner) | ✔ |
| Video thumbnail extraction | ✔ |
| Cancellation mid-download, nothing left behind | ✔ |
| Licensing inactive, nothing withheld | ✔ |
| First normal launch: dark title bar, sidebar, Welcome sheet ("on this PC") | ✔ screenshot `first-launch.png` |
| Silent uninstall removes program files, Start Menu and desktop shortcuts, Apps & features entry, "Open with" registration | ✔ |

## What remains unverified

1. **Windows 11 or Windows 10 on a real PC.** GitHub's Windows runners are Windows Server 2025, the same
   Windows family. Worth checking on your gaming PC:
   - the installer's look and SmartScreen prompt;
   - drag and drop;
   - notifications;
   - an NVIDIA, Intel or AMD video encoder for 4K conversion (the runner has no graphics card).
2. **Interactive use**, rather than automated checks, on both platforms:
   - clicking through the native file dialogs;
   - menu items;
   - quit confirmation with work running;
   - Open With from Finder or Explorer;
   - notification buttons.

   Each of these calls the platform's own API through Tauri and is wired to code that was tested, but nobody
   clicked them in this session.
3. **macOS 12–25.** The app is configured for macOS 12 and later. Only macOS 26 (GitHub) and 27 (this Mac)
   were run.
4. **The Intel slice** on this Mac (Rosetta is gone). Covered instead by GitHub's Intel Mac runner (above), which installed the disk image and passed the
   self-test.
5. **Signed builds.** Developer ID signing, notarisation and Windows code signing need credentials that
   don't exist yet.
6. **The one-time import of the Swift app's settings and history** was tested with synthetic files written in
   the Swift app's exact formats, not with your real ones, to leave them untouched.
7. **Linux** is not a target. The code builds on Unix generally, but no Linux packaging was set up or tested.

## Housekeeping on this Mac

- Debug and test copies were unregistered from Launch Services. Scratch output is in the session's scratchpad
  folder, not your Downloads.
- **Disk:** about 12 GB free. The build folder `MediaForge-Tauri/src-tauri/target` holds several GB and can be
  deleted at any time (`cargo clean`); it is rebuilt when needed.
