# Installing MediaForge

## Windows 11 and Windows 10 (64-bit)

1. Download `MediaForge-Setup.exe` and double-click it.
2. Windows may show **"Windows protected your PC"**, because this installer isn't code-signed yet. Click
   **More info**, check the publisher and file name, then click **Run anyway**. Do this only for a
   `MediaForge-Setup.exe` you received from C2 Software. (Antivirus software may also want to scan it; let it.)
3. Click through the installer. MediaForge installs for you alone, so Windows doesn't ask for an
   administrator password. It goes in `%LOCALAPPDATA%\MediaForge` and appears in the Start menu and on the desktop. If the PC lacks
   Microsoft Edge WebView2 (it is part of Windows 11 and of up-to-date Windows 10), the installer adds it, which
   needs an internet connection.
4. Open MediaForge from the Start menu. The first time, it explains what it is for. Click **I Understand**.

Everything MediaForge needs is included: no Python, FFmpeg or other downloads.

**Uninstalling.** Settings ▸ Apps ▸ Installed apps ▸ MediaForge ▸ Uninstall. The uninstaller offers to remove
MediaForge's settings and history too. Files you downloaded or converted are never touched.

**Company PCs.** If your organisation manages the PC, check with IT first. Their policies may block unsigned
installers, and MediaForge must not be used to get round them.

## macOS 12 or later (Apple silicon or Intel)

1. Open `MediaForge.dmg` and drag **MediaForge** onto **Applications**.
2. Open MediaForge from Applications.

**Internal test builds** (the disk image says "Internal" and has a "Read Me First" file) are not yet signed
with an Apple Developer ID or notarised. On a Mac other than the one that built them, macOS will say it
"could not verify MediaForge is free of malware" and won't open it. The owner of the Mac can approve this one
app:

1. Try to open MediaForge, then click **Done** in the warning.
2. Open **System Settings ▸ Privacy & Security**, scroll down, and click **Open Anyway** next to MediaForge.
3. Confirm with your password.

Don't turn off Gatekeeper or run Terminal commands to strip the warning. On a company-managed Mac, ask IT
first. The signed and notarised release will open like any other app ([DISTRIBUTION.md](DISTRIBUTION.md)).

**Coming from the earlier MediaForge (version 1.0)?** The first time it opens, this version copies that app's
settings and history. The earlier app keeps its own. Replace the old app in Applications when asked.

**Uninstalling.** Drag MediaForge from Applications to the Bin. Its settings and history are in
`~/Library/Application Support/uk.co.c-2.MediaForge`.

## What MediaForge needs

- About 400 MB of disk space (the app plus its bundled yt-dlp, FFmpeg and Deno).
- An internet connection for downloads. Conversions work offline.
- Nothing else: no accounts, no sign-in, no background services.
