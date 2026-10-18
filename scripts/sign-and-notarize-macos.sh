#!/bin/bash
# PRODUCTION SIGNING. Not part of an internal build.
#
# Takes an assembled MediaForge.app (from scripts/package-macos.sh), signs every program inside it with
# C2 Software's Developer ID, wraps it in the customer disk image, and - only when told to - sends that image
# to Apple for notarisation.
#
#   DEVELOPER_ID="Developer ID Application: <Company> (<TEAM ID>)" scripts/sign-and-notarize-macos.sh
#   DEVELOPER_ID="..." scripts/sign-and-notarize-macos.sh --notarize --keychain-profile <NAME>
#   scripts/sign-and-notarize-macos.sh --rehearse
#
# Without --notarize nothing leaves this Mac. --notarize uploads the disk image to Apple, so it should be
# run only on a build that has been approved for release.
#
# --rehearse runs the same signing steps with a local "ad hoc" signature and then checks the helper tools
# still work under the hardened runtime. It needs no credentials and produces nothing distributable; it
# exists so the procedure can be exercised before C2 Software has a certificate.
#
# This script never creates, guesses or works around credentials. If the certificate or the notary profile
# is missing it stops and says so. See docs/DISTRIBUTION.md.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SOURCE_APP="$ROOT/dist/macos/MediaForge.app"
OUTPUT="$ROOT/dist/macos-release"
NOTARIZE=0
REHEARSE=0
PROFILE=""

while [ $# -gt 0 ]; do
    case "$1" in
        --app) SOURCE_APP="$2"; shift 2 ;;
        --output) OUTPUT="$2"; shift 2 ;;
        --notarize) NOTARIZE=1; shift ;;
        --keychain-profile) PROFILE="$2"; shift 2 ;;
        --rehearse) REHEARSE=1; shift ;;
        *) echo "Unknown option: $1" >&2; exit 2 ;;
    esac
done

die() { echo "error: $*" >&2; exit 1; }

[ -d "$SOURCE_APP" ] || die "No app at $SOURCE_APP. Run scripts/package-macos.sh first."

if [ "$REHEARSE" = 1 ]; then
    [ "$NOTARIZE" = 0 ] || die "A rehearsal cannot be notarised."
    IDENTITY="-"
    OUTPUT="$ROOT/dist/macos-rehearsal"
    TIMESTAMP=(--timestamp=none)
    echo "==> REHEARSAL: ad hoc signature, nothing distributable will be produced"
else
    IDENTITY="${DEVELOPER_ID:-}"
    [ -n "$IDENTITY" ] || die "DEVELOPER_ID is not set. It must name C2 Software's 'Developer ID Application' certificate, for example:
       DEVELOPER_ID=\"Developer ID Application: C2 Software Ltd (ABCDE12345)\"
       No such certificate exists on this Mac yet. docs/DISTRIBUTION.md explains how to obtain one."
    case "$IDENTITY" in
        "Developer ID Application:"*) ;;
        *) die "DEVELOPER_ID must be a 'Developer ID Application' certificate. Other certificate types are not accepted by Gatekeeper for apps distributed outside the App Store." ;;
    esac
    security find-identity -v -p codesigning | grep -F "\"$IDENTITY\"" > /dev/null \
        || die "No valid signing identity named \"$IDENTITY\" is in this Mac's keychain.
       Run 'security find-identity -v -p codesigning' to see what is installed."
    if [ "$NOTARIZE" = 1 ]; then
        [ -n "$PROFILE" ] || die "--notarize needs --keychain-profile <NAME>: the profile created with 'xcrun notarytool store-credentials'."
        xcrun notarytool history --keychain-profile "$PROFILE" > /dev/null 2>&1 \
            || die "The notary profile \"$PROFILE\" is missing or its credentials were rejected by Apple."
    fi
    # Apple's timestamp is required for notarisation; signing therefore needs an internet connection.
    TIMESTAMP=(--timestamp)
fi

APP="$OUTPUT/MediaForge.app"
mkdir -p "$OUTPUT"
rm -rf "$APP"
ditto "$SOURCE_APP" "$APP"       # the internal build is left untouched

# Tauri places single-file helpers ("sidecars") beside the app's own program, and folders as resources.
HELPERS="$APP/Contents/MacOS"
YTDLP="$APP/Contents/Resources/tools/yt-dlp"
ENTITLEMENTS="$ROOT/packaging/entitlements"

WORK="$(mktemp -d "${TMPDIR:-/tmp}/mediaforge-sign.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT

sign() { codesign --force --options runtime "${TIMESTAMP[@]}" --sign "$IDENTITY" "$@" 2> "$WORK/codesign.err" || { cat "$WORK/codesign.err" >&2; return 1; }; }

is_program() {
    case "$(file -b "$1")" in *Mach-O*) return 0 ;; *) return 1 ;; esac
}

# Signs one library inside the yt-dlp package. The yt-dlp project's zip stores its Python framework without
# the usual symbolic links, and codesign refuses to address a file inside a folder it can't classify
# ("bundle format is ambiguous"). The signature lives inside the file itself, so signing a copy elsewhere
# and putting it back is equivalent.
sign_library() {
    local file="$1"
    sign "$file" 2> /dev/null && return 0
    grep -q 'ambiguous' "$WORK/codesign.err" || { cat "$WORK/codesign.err" >&2; return 1; }
    local copy="$WORK/$(basename "$file")"
    cp -p "$file" "$copy" && sign "$copy" && mv -f "$copy" "$file"
}

echo "==> Signing from the inside out"
# 1. yt-dlp's libraries, then its program. Everything it loads must carry the same signature as it does.
count=0
while IFS= read -r -d '' file; do
    if is_program "$file"; then
        [ "$file" = "$YTDLP/yt-dlp" ] && continue
        sign_library "$file" || die "could not sign ${file#$APP/}"
        count=$((count + 1))
    fi
done < <(find "$YTDLP" -type f -print0)
if [ "$REHEARSE" = 1 ]; then
    # See the comment in the entitlements file: needed only because a rehearsal has no team identity.
    sign --entitlements "$ENTITLEMENTS/rehearsal-only.entitlements" "$YTDLP/yt-dlp"
else
    sign "$YTDLP/yt-dlp"
fi
echo "    yt-dlp: program and $count libraries"

# 2. FFmpeg and FFprobe need no special permissions.
sign "$HELPERS/ffmpeg" "$HELPERS/ffprobe"
echo "    ffmpeg, ffprobe"

# 3. Deno's JavaScript engine generates code at run time, which the hardened runtime forbids unless the
#    signature says otherwise. Re-signing replaces Deno Land's signature, so the permission is restated.
sign --entitlements "$ENTITLEMENTS/deno.entitlements" "$HELPERS/deno"
echo "    deno (with JIT entitlements)"

# 4. MediaForge's own program, then the app itself, last, sealing everything above.
sign "$HELPERS/MediaForge"
sign "$APP"
echo "    MediaForge.app"

echo "==> Verifying"
codesign --verify --deep --strict "$APP" || die "the signed app failed verification"
unsigned=0
while IFS= read -r -d '' file; do
    if is_program "$file"; then
        # Every architecture inside a universal file must carry the hardened-runtime flag.
        for arch in $(lipo -archs "$file" 2> /dev/null); do
            details="$(codesign -dv --arch "$arch" "$file" 2>&1 || true)"
            case "$details" in
                *"(runtime)"*|*",runtime)"*) ;;
                *) echo "    not hardened ($arch): ${file#$APP/}" >&2; unsigned=$((unsigned + 1)) ;;
            esac
        done
    fi
done < <(find "$APP" -type f -print0)
[ "$unsigned" = 0 ] || die "$unsigned program(s) are not signed with the hardened runtime; Apple would reject the submission."
echo "    every program inside is signed with the hardened runtime"

echo "==> Checking the helper tools still run"
( cd "${TMPDIR:-/tmp}" \
    && "$HELPERS/ffmpeg" -hide_banner -version > /dev/null \
    && "$HELPERS/ffprobe" -hide_banner -version > /dev/null \
    && "$HELPERS/deno" eval 'console.log([1, 2, 3].map(n => n * 2).join(","))' > /dev/null \
    && "$YTDLP/yt-dlp" --version > /dev/null ) || die "a helper tool no longer runs after signing. Its entitlements need adjusting before release."
echo "    ffmpeg, ffprobe, deno and yt-dlp all start"
# yt-dlp must also be able to find and start the other two from inside the signed bundle.
seen="$(cd "${TMPDIR:-/tmp}" && env -i HOME="$HOME" PATH="$HELPERS:/usr/bin:/bin" "$YTDLP/yt-dlp" --ignore-config --ffmpeg-location "$HELPERS" -v 2>&1 || true)"
case "$seen" in *"JS runtimes: deno"*) ;; *) die "yt-dlp cannot start the bundled Deno after signing." ;; esac
case "$seen" in *"exe versions: ffmpeg"*) ;; *) die "yt-dlp cannot start the bundled FFmpeg after signing." ;; esac
echo "    yt-dlp can start the bundled deno and ffmpeg"

if [ "$REHEARSE" = 1 ]; then
    echo "Rehearsal complete: ${APP#$ROOT/} (ad hoc; do not distribute)."
    exit 0
fi

echo "==> Building the customer disk image"
"$ROOT/scripts/package-macos.sh" --release --output "$OUTPUT"
DMG="$OUTPUT/MediaForge.dmg"
codesign --force --timestamp --sign "$IDENTITY" "$DMG"
( cd "$OUTPUT" && shasum -a 256 "$(basename "$DMG")" > "$(basename "$DMG").sha256" )

if [ "$NOTARIZE" = 0 ]; then
    echo "Signed but NOT notarised: ${DMG#$ROOT/}"
    echo "Gatekeeper will still block this on other Macs. Re-run with --notarize --keychain-profile <NAME> once the build is approved for release."
    exit 0
fi

echo "==> Sending to Apple for notarisation (this uploads the disk image)"
xcrun notarytool submit "$DMG" --keychain-profile "$PROFILE" --wait | tee "$OUTPUT/notarization.log"
grep -q 'status: Accepted' "$OUTPUT/notarization.log" \
    || die "Apple did not accept the submission. Fetch the reasons with: xcrun notarytool log <submission id> --keychain-profile $PROFILE"

echo "==> Attaching Apple's ticket"
xcrun stapler staple "$DMG"
xcrun stapler validate "$DMG"
( cd "$OUTPUT" && shasum -a 256 "$(basename "$DMG")" > "$(basename "$DMG").sha256" )

echo "==> Asking Gatekeeper"
spctl --assess --type open --context context:primary-signature -vv "$DMG"
echo "Ready for distribution: ${DMG#$ROOT/}"
