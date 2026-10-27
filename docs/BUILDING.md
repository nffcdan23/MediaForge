# Building MediaForge

There are two ways to build the installers:

- **GitHub Actions** (recommended): it builds both, and tests the Windows installer on a real Windows
  machine.
- **By hand on each platform**: each installer is built on its own platform. A Mac makes the DMG; a Windows PC
  makes the Windows installer.

## With GitHub Actions

The repository is private on GitHub (`nffcdan23/MediaForge`). In the repository's **Actions** tab:

- **CI** runs on every push:
  - TypeScript checks, interface tests and a production build of the interface;
  - Rust formatting, lint and the full test suite on Windows, with the real Windows tools.

  The macOS test job runs on request (Run workflow) and on pull requests, because macOS runners use ten
  times as many Actions minutes.
- **Build installers** runs on request, or when a tag such as `v1.1.0` is pushed. It produces:

  | Artifact | Contents |
  | --- | --- |
  | `MediaForge-Windows` | `MediaForge-Setup.exe`, its SHA-256, and `verification/` (installation log, self-test report, screenshots) |
  | `MediaForge-macOS` | `MediaForge.dmg` (universal), its SHA-256, and the Apple silicon self-test report |
  | `MediaForge-macOS-Intel-verification` | The same disk image installed and self-tested on an Intel Mac |

  On Windows the job does what a person would: it installs the app silently, checks the Start Menu entry and
  "Open with" registration, and runs the installed app's self-test. It also takes screenshots, uninstalls, and
  checks that nothing is left behind.

Download artifacts from the run's page, or with
`gh run download <run-id> --repo nffcdan23/MediaForge`. Artifacts are private to the repository and expire
after 90 days. Nothing is published as a release.

## By hand

### Prerequisites

| | macOS | Windows |
| --- | --- | --- |
| Node.js | 20 or later | 20 or later |
| Rust | `rustup` with `aarch64-apple-darwin` and `x86_64-apple-darwin` | `rustup`, default `x86_64-pc-windows-msvc` |
| Platform tools | Xcode Command Line Tools; Python 3.10+ for the disk image | Visual Studio 2022 Build Tools ("Desktop development with C++") |

### macOS: `MediaForge.dmg`

```bash
npm ci
scripts/package-macos.sh
```

This:
1. downloads the pinned tools and checks each checksum (`node scripts/fetch-tools.mjs macos`);
2. compiles the LGPL FFmpeg once (`scripts/build-ffmpeg-macos.sh`, about 10 minutes, cached in
   `tools/macos/ffmpeg-build`);
3. builds MediaForge for Apple silicon and Intel and joins them;
4. signs it ad hoc, checks the bundle and the FFmpeg licence;
5. makes `dist/macos/MediaForge.dmg`. The disk image is labelled as an internal build in its volume name, its
   window and a "Read Me First" file.

`--repackage` redoes only the signing and the disk image. The DMG is not signed with a Developer ID, so it opens
only on Macs whose owner approves it (see [INSTALLING.md](INSTALLING.md)).
[DISTRIBUTION.md](DISTRIBUTION.md) covers the signed build.

Universal is the right choice for one download: the helper tools are already universal. It costs about 30 MB
over an Apple-silicon-only build, and nobody has to pick the right file.

### Windows: `MediaForge-Setup.exe`

```powershell
npm ci
node scripts/fetch-tools.mjs windows
pwsh scripts/package-windows.ps1
pwsh scripts/verify-windows-install.ps1 -Installer dist\windows\MediaForge-Setup.exe -Results dist\windows\verification   # optional
```

`package-windows.ps1` builds the app and the NSIS installer. Tauri downloads NSIS and its WebView2 bootstrapper
the first time.

The installer:
- installs per user into `%LOCALAPPDATA%\MediaForge`, so no administrator prompt;
- adds Start Menu and desktop shortcuts and an uninstaller in Apps & features;
- registers MediaForge under "Open with" for audio and video files, without becoming the default for any;
- installs Microsoft WebView2 if the PC lacks it.

The Visual C++ runtime is linked into `MediaForge.exe` (`src-tauri/.cargo/config.toml`), so no Visual C++
Redistributable is needed. `package-windows.ps1` fails the build if the program still refers to `VCRUNTIME140`,
`VCRUNTIME140_1` or `MSVCP140`. The program does use the Universal C Runtime (`api-ms-win-crt-*`), which is part
of Windows 10 and 11.

Building the Windows installer on a Mac is possible with `cargo-xwin`. It isn't set up here: it needs LLVM and
Microsoft's Windows SDK, which means accepting Microsoft's licence. It also can't test anything, which the
GitHub route does.

## Versions

| What | Where |
| --- | --- |
| Version shown to people (1.2.0) | `version` in `src-tauri/tauri.conf.json`, `src-tauri/Cargo.toml` and `package.json` (keep them equal) |
| Build number (5) | `bundle.macOS.bundleVersion` in `src-tauri/tauri.conf.json`. Raise it for every build given to anyone; macOS and a future updater use it to tell builds apart |
| Identifier | `uk.co.c-2.MediaForge`, shared with the Swift app so it replaces it. Never change it after release: settings, notification permissions and Windows' uninstall entry depend on it |

## Updating a bundled tool

yt-dlp needs updating far more often than MediaForge: sites change, and an old yt-dlp stops working with them.

1. Change the line in `packaging/tools.lock`: version, URL and the **publisher's** checksum.
2. Rebuild, and run **Build installers**. The Windows self-test and the macOS integration tests exercise the
   new version.

The Windows FFmpeg comes from a month-end BtbN build. BtbN keeps those, unlike its daily builds.
