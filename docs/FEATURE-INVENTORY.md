# MediaForge feature inventory (audit of the macOS app)

Audit of the native macOS app in the parent folder (`MediaForge.xcodeproj`, `MediaForgeCore/`, `MediaForge/`),
done on 18 October 2026 before the cross-platform migration began. The Swift app is the source of truth for
what the Tauri version must do. Its build at the time of the audit is 1.0.0 (build 3).

## 1. Languages, frameworks and architecture

| Aspect | Swift app |
| --- | --- |
| Language | Swift 6, strict concurrency (`@MainActor`, `Sendable`) |
| UI | SwiftUI with macOS 26 Liquid Glass (`.glassEffect`, `.buttonStyle(.glass)`), `NavigationSplitView` |
| State | Observation framework (`@Observable`) |
| Minimum OS | macOS 26.0, universal (Apple silicon and Intel) |
| Layers | `MediaForgeCore` (engine, no UI imports) is shared by the app and the test bundle; `MediaForge` holds the views, menus and app delegate |
| Tests | 154 Swift Testing tests, including integration tests against the real tools and a loopback HTTP server |

Engine layout:

- **Models**: output options, jobs, history entries, errors, media metadata.
- **Support**: URL validation, filename sanitising, display formatting.
- **Processes**: subprocess runner, tool discovery.
- **Services**: yt-dlp driver, FFmpeg driver, notifications, audio preview, thumbnails.
- **Engine**: job pipeline, job queue, URL-field state machine, output files, local-file import.
- **Stores**: settings (`UserDefaults`), history (JSON).
- **Licensing**: inactive interfaces.

## 2. External executables

| Tool | Version | How it is obtained | Role |
| --- | --- | --- | --- |
| yt-dlp | 2026.08.19 | Official `yt-dlp_macos.zip`, a PyInstaller folder with its own Python; pinned SHA-256 | Metadata lookup and downloading |
| Deno | 2.9.7 | Official Developer-ID-signed builds, joined into a universal file | JavaScript runtime that yt-dlp needs for some sites (YouTube) |
| FFmpeg, FFprobe | 9.0.2 | Compiled from source as an LGPL build with LAME 4.0, VideoToolbox and AudioToolbox | Conversion, probing, video re-encoding |

Search order for each tool: a path the user chose in Settings, then the copy inside the app, then Homebrew,
MacPorts and system folders. `$PATH` is never consulted. A tool is rejected if its path is relative, it isn't a
regular executable file, or other users can write to it.

## 3. Features

### Downloads screen

- **Link field.** Multi-line field ("Paste a link — or several, one per line"), Paste button, Clear button, a
  spinner while resolving, and a footnote about responsible use that becomes a warning for invalid input.
- **Validation before any network access.**
  - Only `http` and `https` with a real host. Bare `example.com/x` is upgraded to `https://`.
  - Rejects credentials in the URL, bad ports, control, bidi and zero-width characters, more than 2,048
    bytes, and option-like strings.
  - Pasting several links splits on whitespace, de-duplicates and caps at 200. A count of rejected
    tokens is shown.
- **Debounced lookup** (450 ms) with yt-dlp `--dump-single-json --flat-playlist`. Edits that leave the link
  unchanged don't trigger a new lookup; Return looks up immediately, or queues if a result is showing.
- **Detection states**:
  - resolving (placeholder card with an indeterminate bar);
  - single media (preview card);
  - playlist or feed (list card with the first 6 entries and "Download All (n)");
  - batch of links (list of the first 5 and "Download All (n)");
  - error card with an icon chosen per error, Try Again if the error is retryable, Open Settings if a tool
    is missing, and Clear.
- **Preview card.**
  - 340×191 artwork with a duration badge and a blurred tint of the artwork behind the card.
  - Source badge (e.g. YOUTUBE, or DIRECT LINK for the generic extractor), "AUDIO ONLY" or "UP TO nP".
  - Title, which can be selected, and uploader.
- **Output options.**
  - Audio/Video segmented control, shown only when the source has video.
  - Audio: format, bitrate (lossy formats) or bit depth (lossless formats), and sample rate.
  - Video: MP4 with a quality picker limited to heights the source actually offers; the saved preference
    falls back to "Best" when the source can't meet it.
  - A conversion note when the chosen quality only exists in VP9 or AV1.
- **Download button** with a summary ("MP3 · 320 kbps", "Saves to MediaForge"). It is disabled when tools are
  missing.
- **Queue section.** "In Progress"/"Recent" header with a count, Cancel All (more than one pending), Clear
  Finished, and an empty state.
- **Drag and drop.** Dropped web links go into the link field; dropped files go to Conversions.
- **Toolbar.** Open Folder.

### Conversions screen

- Drop zone for files and folders, with a targeted state; Choose Files…; Add Files toolbar button (⌘O).
- "Convert to" card: format detail, the same audio pickers, and a "Saves to" folder button.
- Folders are walked recursively (hidden files and packages skipped, natural sort, at most 500 files).
- 39 recognised audio and video extensions; anything else is ignored with an alert if nothing remains.
- Video files have their audio extracted. Originals are never modified.
- Files opened with **Open With → MediaForge**, or dropped on the Dock icon, are queued for conversion.

### Jobs (shared by both screens)

- **States**:
  - queued;
  - preparing ("Contacting source…" or "Preparing…");
  - downloading, with %, bytes, speed and time remaining;
  - processing (merging, remuxing, writing metadata);
  - converting, with %, speed (e.g. 42×) and time remaining;
  - finalizing ("Saving…");
  - completed;
  - failed;
  - cancelled.
- **Multi-part downloads** (separate video and audio) are combined into one progress figure, weighted by
  size, which never moves backwards.
- **Rows.**
  - 116×65 thumbnail with a duration badge and a green tick when complete.
  - Title, format badge, a one-line status, and a gradient progress bar (indeterminate shimmer when the
    amount is unknown).
- **Actions.**
  - Cancel.
  - Preview (audio) or Quick Look (video).
  - Show in Finder.
  - Try again (cancelled jobs, or failed jobs whose error is retryable).
  - Remove.
- **Context menu.** Copy Link, Show Details (the last 60 log lines in a popover with Copy), Remove from List.
- **Concurrency** of 1–6 jobs (default 3), taken from Settings and applied live.
- **Retry** runs the job from scratch with a new attempt number.
- **Cancellation** stops the whole tool process tree and removes partial files.
- **Dock badge** shows the number of pending jobs.

### Pipeline

- Each job runs in a private temporary folder (`$TMPDIR/MediaForge/Jobs/<pid>/<job>-<attempt>`, mode 700).
  Only the finished file is moved to the destination; the folder is removed however the job ends. Folders
  left by crashed runs are cleaned at launch, but never those of another running copy.
- **Destination checks.** The folder is created if missing; the job fails if it isn't a folder, isn't
  writable, or has less than 256 MB free.
- **Output names.** The remote title is sanitised. Name clashes become "Name 2.mp3", etc. The name is claimed
  atomically with up to 5 retries, and an existing file is never overwritten.
- **Audio from a link.** yt-dlp fetches `bestaudio/best`, then FFmpeg converts it with progress. The stream is
  copied instead of re-encoded when it is already in the target codec, at the same sample rate, and at or
  below the chosen bitrate. Title, artist and source link are written as tags (optional; never for ADTS AAC).
- **Video from a link.**
  - Format selector `bv*+ba/b[vcodec!=?none]`, plus an AV1-avoiding variant when conversion is on.
  - Sort order: resolution cap, fps, H.264, AAC. Merged or remuxed into MP4, with metadata embedded.
  - The result is probed and rejected if it has no video.
  - VP9/AV1 is re-encoded to H.264 or HEVC with VideoToolbox (bitrate rules by resolution and frame rate;
    audio copied when MP4-safe). Configurable: H.264 (default), HEVC, or keep the original.
- **Formats**:
  - MP3 (LAME, ID3v2.3);
  - WAV and AIFF (16/24-bit PCM);
  - AAC (ADTS);
  - M4A (AAC, faststart);
  - FLAC (level 8, 16/24-bit);
  - sample rate kept, 44.1 kHz or 48 kHz;
  - bitrates 128/192/256/320.
- **Limits.** Optional download speed limit (1–25 MB/s) and maximum file size (500 MB–10 GB). Overall and
  stall timeouts, output caps, socket timeout 30 s, 5 retries.
- **Live streams** are refused (`!is_live` filter and metadata check). Sources that are entirely DRM are refused.

### History

- Completed and failed jobs are saved as JSON (at most 1,000 entries, newest first). A corrupt file is moved
  aside rather than overwritten.
- Search across title, creator, link, path and format. Filters: All / Audio / Video / Failed.
- Grouped by day: Today, Yesterday, otherwise the full date.
- **Rows.**
  - 92×52 thumbnail, title, badge and failure label.
  - Subtitle with format, size and duration; "File moved or deleted" when the file has gone.
  - Time and origin (host, or source file name).
- **Actions.**
  - Preview or Quick Look, and Show in Finder (when the file still exists).
  - Download Again / Convert Again with the same output settings.
  - Copy Link, Copy File Path, Remove from History.
  - Clear History, with a confirmation that files are kept.

### Audio preview player

- Floating glass bar along the bottom with:
  - artwork, title and subtitle;
  - back 10 s, play/pause and forward 10 s;
  - elapsed time and total duration;
  - a waveform scrubber (180 peaks, gradient for the played part, click or drag to seek);
  - Show in Finder and close.
- Previewable formats: mp3, wav, aac, m4a, flac, aiff, aif, caf. Errors for a missing or undecodable file.
- Menu: Play/Pause Preview (⇧⌘P).

### Settings

- **General.** Destination folder (Choose…, Show), simultaneous jobs 1–6, notifications on/off, tag files on/off.
- **Audio.** Default format (with description), bitrate, bit depth and sample rate.
- **Video.** Preferred quality; what to do with formats Macs can't play.
- **Limits.** Download speed limit; largest file to download.
- **Appearance.** Dark (default), Light or Match System.
- **Tools.**
  - Status, version and purpose for yt-dlp, FFmpeg and FFprobe.
  - "Included with MediaForge" or the path in use.
  - Choose Location… / Find Automatically / Show in Finder.
  - Install hints when a tool is missing; Check Again.
- **Privacy & About.** No tracking; no bypass; Responsible Use (Review…); third-party licences; version.
- **Persistence.** Stored in `UserDefaults` with lenient parsing and clamping. Settings saved by builds that
  used the old placeholder identifier are adopted once. The selected sidebar section is remembered.

### App shell

- Sidebar with Library (Downloads, Conversions, History, each with a pending-count badge) and App (Settings).
- A footer showing tool health: green "Ready" with versions, or orange "Setup needed".
- Ambient mesh-gradient background (dark and light palettes). Translucent cards (16 pt radius, 1 px hairline)
  with the brand gradient: ember #FF9938 to flame #FF5461.
- **First-launch Welcome sheet** ("Download only what you're allowed to", "Restrictions are respected",
  "Private by design") with Quit or I Understand. It can be reopened from Help ▸ Responsible Use.
- **Menus.**
  - File: Import Files… ⌘O, Paste Link ⇧⌘V, Open Destination Folder ⇧⌘D.
  - Settings… ⌘,.
  - View: sections ⌘1–⌘4.
  - Queue: Cancel All, Clear Finished, Play/Pause Preview.
  - Help: Responsible Use…, Check Tools.
- **Closing and quitting.**
  - Closing the window keeps the app (and its queue) running.
  - Quitting with jobs pending asks for confirmation, then cancels and waits.
  - SIGTERM stops the helper processes before exit.
- **Notifications.** Native notifications on completion and failure, with a "Show in Finder" action.
  Permission is requested the first time a job is queued.
- **Document types.** The app registers as a viewer for audio and video files (Open With).

## 4. Security properties

- Tools are launched by absolute path with an argument vector, never through a shell. The link always follows
  `--`.
- **Child processes.**
  - Minimal environment: a fixed PATH, HOME, TMPDIR, UTF-8 locale and proxy/CA variables only.
  - Own process group, at utility priority, with stdin from /dev/null.
  - Tree-kill on cancel, timeout or quit: SIGTERM, then SIGKILL after 2.5 s.
- **yt-dlp** runs with `--ignore-config --no-plugin-dirs --no-cookies --no-cookies-from-browser --xff never`.
  TLS verification stays on, and remote titles never choose a path.
- **FFmpeg** runs with `-protocol_whitelist file,crypto,data` and `file:` prefixes. It never overwrites (`-n`)
  and ignores stdin.
- **Thumbnails** come from a private, ephemeral HTTP session: no cookies, at most 8 MB, http(s) only, cached
  by SHA-256 of the URL.
- **Finished downloads** are quarantined (`LSFileQuarantineEnabled`). Hardened runtime; not sandboxed.
- **No telemetry, accounts or advertising.** The only network traffic is to the sources the user pastes and
  their artwork.

## 5. Packaging

- `Scripts/fetch-tools.sh`: pinned downloads checked against SHA-256.
- `build-ffmpeg.sh`: LGPL, universal, minimum macOS 11.
- `package-app.sh`: assembles the app, adds notices, signs ad hoc, and checks the build is LGPL.
- `make-dmg.sh`: dmgbuild, with a banner and a read-me for internal builds.
- `sign-and-notarize.sh`: inside-out Developer ID signing and notarisation; production only, never run.
- Licence notices: FFmpeg LGPL, LAME LGPL, yt-dlp package GPLv3+, Deno MIT.

## 6. Known limitations and bugs found during the audit

1. **macOS 26 only.** Liquid Glass APIs set the minimum, so friends on older Macs can't run it.
2. **Unsigned and not notarised.** Other Macs refuse it; this needs a Developer ID (see the distribution
   docs).
3. **Notifications.** "Show in Finder" in a notification needs the app to be running; that's acceptable.
4. **Quick Look** is macOS-only (it previews video).
5. **Missing-tool messages** tell the user to install with Homebrew. That's wrong advice for the packaged app
   (tools are bundled) and has no Windows equivalent.
6. **Process trees after a hard kill.** If MediaForge is force-killed (SIGKILL), helper processes in their own
   process groups survive until they finish. SIGTERM is handled.
7. **AV1 downloads** can't be converted by the bundled macOS FFmpeg (no AV1 decoder), so the format selector
   avoids AV1 when conversion is on.
8. **Choose Files…** can't select folders and files together; folders work by drag and drop.

## 7. What the migration must preserve

Every feature in section 3 and every security property in section 4. Where something is macOS-specific
(Quick Look, Dock badge, Finder reveal, notification actions, Open With, AVFoundation playback and waveforms,
VideoToolbox), the Tauri version needs a working equivalent on each platform. The migration report records
how each one was handled.
