# Distributing MediaForge

What has to happen before MediaForge can be given to customers through the C2 Software website, and how
updates would reach them. None of the steps that involve Apple, Microsoft, certificate authorities or
uploading anything have been taken. They need management approval and C2 Software's own credentials.

## Where things stand (18 October 2026)

| | macOS | Windows |
| --- | --- | --- |
| Installer | `MediaForge.dmg`, universal | `MediaForge-Setup.exe`, x64 |
| Signature | Ad hoc only (identifies nobody) | None |
| What people see | "Apple could not verify…", blocked until the owner clicks Open Anyway | "Windows protected your PC" (SmartScreen); More info ▸ Run anyway |
| Fine for | The Mac that built it; testers willing to approve it | Your own PC; testers willing to click through |
| Not fine for | Customers | Customers |

## macOS: Developer ID and notarisation

Outside the Mac App Store, macOS opens apps without warnings only when they are:
1. signed with a **Developer ID Application** certificate;
2. **notarised** (automatically checked) by Apple.

Steps, once approved:

1. **An Apple Developer Program membership for the business that will publish MediaForge.** The identity
   people see in macOS is this account's legal name. Xcode on the build Mac is signed in to a paid team,
   "STARSCALE LTD". Using it would show STARSCALE LTD as the developer of a C2-branded app. Whether to use it,
   enrol C2 Software, or use another account is a management decision.
2. **The account holder creates a "Developer ID Application" certificate**: Xcode ▸ Settings ▸ Accounts ▸
   Manage Certificates ▸ + ▸ Developer ID Application. The "Apple Development" certificates on this Mac can't
   be used for this.
3. **Store notarisation credentials once**, with an app-specific password or an App Store Connect API key:
   `xcrun notarytool store-credentials MediaForgeNotary --apple-id … --team-id …`.
4. **Build and sign:**
   ```bash
   scripts/package-macos.sh
   DEVELOPER_ID="Developer ID Application: <Company> (<TEAM ID>)" scripts/sign-and-notarize-macos.sh
   ```
   That signs every program inside the app from the inside out, with the hardened runtime:
   - yt-dlp's 106 libraries and program;
   - FFmpeg and FFprobe;
   - Deno, with JIT entitlements;
   - MediaForge itself.

   It then checks every architecture, confirms the tools still start, and makes the customer disk image.
5. **Notarise.** This uploads the disk image to Apple, so only do it for a build approved for release:
   ```bash
   DEVELOPER_ID="…" scripts/sign-and-notarize-macos.sh --notarize --keychain-profile MediaForgeNotary
   ```
   It waits for Apple's verdict, attaches the ticket ("staples" it) and asks Gatekeeper to confirm.

`scripts/sign-and-notarize-macos.sh --rehearse` runs the same signing with a local signature and no
credentials. It passed on 18 October 2026 (106 yt-dlp libraries signed; all four tools start under the
hardened runtime; yt-dlp finds the bundled Deno and FFmpeg).

To sign in GitHub Actions instead, import the certificate into a temporary keychain from encrypted repository
secrets. Don't do this until the account question is settled.

## Windows: code signing

Unsigned installers trigger SmartScreen's warning until the file builds a reputation. Options, roughly in
order of cost:

1. **Azure Trusted Signing** (Microsoft's signing service). It is the usual choice for a small company. It
   needs an Azure subscription and identity validation of the company. Tauri supports it through
   `bundle.windows.signCommand`.
2. **An OV or EV code-signing certificate** from a certificate authority. Since 2023 these come on a hardware
   token or a cloud HSM. EV no longer skips SmartScreen's reputation building.

Either way, the signing command goes into `tauri.conf.json` (`bundle.windows.signCommand`), and the GitHub
workflow gets the credentials as secrets. Both the app and the installer are signed. Signing also helps with
antivirus products such as Huntress, which treat unsigned PyInstaller programs (like yt-dlp) with suspicion.

## Updates

Planned, not built. The design:

- **Mechanism: Tauri's updater plugin** (`tauri-plugin-updater`). The app downloads a small JSON manifest from
  C2 Software's website, for example `https://updates.c-2.co.uk/mediaforge/{{target}}/{{current_version}}`.
  If a newer version exists, it downloads the update and **checks its signature** against a public key built
  into the app, then installs it:
  - Windows: runs the new NSIS installer quietly;
  - macOS: replaces the app bundle.
- **Keys.** An update-signing key pair, separate from Apple's and Microsoft's, made with `tauri signer
  generate`. The private key lives only in the release pipeline's secrets; losing it means users must
  reinstall by hand.
- **Build changes.**
  - Set `bundle.createUpdaterArtifacts: true`. This produces `.tar.gz` (macOS) and `.nsis.zip` (Windows)
    update packages with `.sig` files.
  - Add the plugin with its public key and endpoint to `tauri.conf.json`.
  - Add a "Check for Updates…" item to the menu and to Settings.
- **Policy.**
  - Check at most once a day.
  - Never download without asking.
  - Never install while jobs are running.
  - Release notes shown first.
  - Updates and licensing are independent: an expired licence never blocks a security update.
- **yt-dlp** goes stale faster than the app. Updating the whole app is the safest way to deliver a new yt-dlp,
  because it stays inside the signed bundle. A separate yt-dlp updater would mean downloading and running an
  unsigned program, which the security rules rule out.

## Releases

1. Raise `bundle.macOS.bundleVersion` (and the version, if it changes) and commit.
2. Push a tag, for example `git tag v1.1.0 && git push --tags`. **Build installers** produces both installers
   and their checks.
3. Sign and notarise the Mac build, and sign the Windows build (above).
4. Publish on the C2 Software website, with SHA-256 checksums. With the updater in place, publish the update
   manifest as well.

## Licences of the bundled tools

| Component | Licence | What C2 Software must do |
| --- | --- | --- |
| FFmpeg (macOS, our build) | LGPL 2.1+ | Include licence and notices (done). Offer the source of the exact version (links in the notices; supply on request) |
| FFmpeg (Windows, BtbN LGPL build) | LGPL 2.1+ | As above. The libraries are separate DLLs, so users can replace them, as the LGPL expects |
| yt-dlp packaged program | GPL 3+ as a whole (contains GPL components) | Include licence and notices (done). Provide or offer the source (links to the exact release). MediaForge runs it as a separate program and doesn't link it, so MediaForge itself is not affected |
| Deno | MIT | Include licence (done) |
| Tauri, React, Lucide, Rust crates | MIT / Apache-2.0 / ISC | Notice in the third-party file (done) |

The notices are in the app (Settings ▸ Privacy & About ▸ View Licences…). They have been written carefully,
but they are not legal advice. A licensing review before commercial release is advisable.
