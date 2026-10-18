#!/bin/bash
# Builds MediaForge for macOS (Apple silicon and Intel in one app) and puts it in a disk image.
#
#   scripts/package-macos.sh [--internal | --release] [--identifier ID] [--output FOLDER] [--repackage]
#
# --repackage skips fetching and compiling, and re-signs and re-images the app already in the output folder.
# --internal (the default) makes an unsigned testing build, signed "ad hoc": it runs on the Mac that built it
#   and on Macs whose owner approves it, and is labelled as not for distribution in its name, its window and
#   a read-me inside it.
# --release requires the app to have been signed with a Developer ID by scripts/sign-and-notarize-macos.sh
#   first, and refuses otherwise. See docs/DISTRIBUTION.md.
#
# Results in <output> (default dist/macos): MediaForge.app, MediaForge.dmg and MediaForge.dmg.sha256.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
MODE="internal"
REPACKAGE=0
IDENTIFIER=""
OUTPUT="$ROOT/dist/macos"
while [ $# -gt 0 ]; do
    case "$1" in
        --internal) MODE="internal"; shift ;;
        --release) MODE="release"; shift ;;
        --identifier) IDENTIFIER="$2"; shift 2 ;;
        --output) OUTPUT="$2"; shift 2 ;;
        --repackage) REPACKAGE=1; shift ;;
        *) echo "Unknown option: $1" >&2; exit 2 ;;
    esac
done

cd "$ROOT"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/mediaforge-package.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT

if [ "$MODE" = "internal" ] && [ "$REPACKAGE" = 0 ]; then
    echo "==> Third-party tools"
    node scripts/fetch-tools.mjs macos --download-only
    [ -x tools/macos/ffmpeg-build/ffmpeg ] || scripts/build-ffmpeg-macos.sh
    node scripts/fetch-tools.mjs macos

    for target in aarch64-apple-darwin x86_64-apple-darwin; do
        rustup target list --installed | grep -qx "$target" || rustup target add "$target"
    done

    echo "==> Building MediaForge (universal)"
    configs=(--config packaging/bundle.macos.json)
    [ -n "$IDENTIFIER" ] && configs+=(--config "{\"identifier\":\"$IDENTIFIER\"}")
    npx tauri build --target universal-apple-darwin --bundles app "${configs[@]}" > "$WORK/build.log" 2>&1 || { tail -30 "$WORK/build.log" >&2; exit 1; }
    BUILT="src-tauri/target/universal-apple-darwin/release/bundle/macos/MediaForge.app"

    mkdir -p "$OUTPUT"
    rm -rf "$OUTPUT/MediaForge.app"
    ditto "$BUILT" "$OUTPUT/MediaForge.app"
fi

APP="$OUTPUT/MediaForge.app"
[ -d "$APP" ] || { echo "No app at $APP." >&2; exit 1; }
MACOS="$APP/Contents/MacOS"

if [ "$MODE" = "internal" ]; then
    echo "==> Signing (ad hoc)"
    # Nothing inside the app may be writable by other users; MediaForge refuses to run tools that are.
    chmod -R u+rwX,go+rX,go-w "$APP"
    # Our own FFmpeg build gets the same local signature as the app. Deno keeps Deno Land's signature and
    # yt-dlp keeps the signatures it was published with.
    codesign --force --sign - --options runtime "$MACOS/ffmpeg" "$MACOS/ffprobe"
    codesign --force --sign - --options runtime "$APP"
else
    signature="$(codesign -dv --verbose=2 "$APP" 2>&1 || true)"
    case "$signature" in
        *"Authority=Developer ID Application"*) ;;
        *) echo "Refusing to make a release image: the app is not signed with a Developer ID." >&2; exit 1 ;;
    esac
fi
codesign --verify --deep --strict "$APP"

echo "==> Checking the assembled app"
plist() { /usr/libexec/PlistBuddy -c "Print :$1" "$APP/Contents/Info.plist"; }
VERSION="$(plist CFBundleShortVersionString)"
BUILD="$(plist CFBundleVersion)"
echo "    identifier  $(plist CFBundleIdentifier)"
echo "    version     $VERSION (build $BUILD)"
echo "    minimum OS  macOS $(plist LSMinimumSystemVersion)"
echo "    app         $(lipo -archs "$MACOS/MediaForge")"
for tool in ffmpeg ffprobe deno; do
    echo "    $tool$(printf '%*s' $((11 - ${#tool})) '')$(lipo -archs "$MACOS/$tool")"
done
echo "    yt-dlp      $(lipo -archs "$APP/Contents/Resources/tools/yt-dlp/yt-dlp")"
configuration="$("$MACOS/ffmpeg" -hide_banner -version)"
case "$configuration" in
    *enable-gpl*|*enable-nonfree*) echo "The bundled FFmpeg is not an LGPL build." >&2; exit 1 ;;
esac
[ -f "$APP/Contents/Resources/ThirdPartyNotices/THIRD-PARTY-NOTICES.txt" ] || { echo "Licence notices are missing." >&2; exit 1; }
echo "    size        $(du -sh "$APP" | cut -f1)"

echo "==> Disk image"
# dmgbuild lives in a private Python environment under tools/; nothing is installed system-wide. Set
# MEDIAFORGE_DMGBUILD to use an existing copy instead.
VENV="$ROOT/tools/dmgbuild-venv"
DMGBUILD="${MEDIAFORGE_DMGBUILD:-$VENV/bin/dmgbuild}"
if [ ! -x "$DMGBUILD" ]; then
    PYTHON=""
    for candidate in /opt/homebrew/opt/python@3.14/bin/python3.14 /opt/homebrew/opt/python@3.13/bin/python3.13 \
            /opt/homebrew/opt/python@3.12/bin/python3.12 /opt/homebrew/bin/python3 /usr/local/bin/python3 /usr/bin/python3 python3; do
        if command -v "$candidate" > /dev/null && "$candidate" -c 'import sys; sys.exit(0 if sys.version_info >= (3, 10) else 1)' 2>/dev/null; then
            PYTHON="$candidate"; break
        fi
    done
    [ -n "$PYTHON" ] || { echo "dmgbuild needs Python 3.10 or later, which was not found." >&2; exit 1; }
    "$PYTHON" -m venv "$VENV"
    "$VENV/bin/pip" install --quiet --disable-pip-version-check --require-hashes -r packaging/dmgbuild-requirements.txt
    DMGBUILD="$VENV/bin/dmgbuild"
fi

if [ "$MODE" = "internal" ]; then
    VOLUME="MediaForge $VERSION Internal"
    swift scripts/make_dmg_background.swift "$WORK/background.tiff" --internal > /dev/null
    README="$WORK/Read Me First.txt"
    sed -e "s/@VERSION@/$VERSION/g" -e "s/@BUILD@/$BUILD/g" "packaging/Read Me First (internal).txt" > "$README"
    defines=(-D internal=1 -D "readme=$README")
else
    VOLUME="MediaForge $VERSION"
    swift scripts/make_dmg_background.swift "$WORK/background.tiff" > /dev/null
    defines=(-D internal=0)
fi
DMG="$OUTPUT/MediaForge.dmg"
rm -f "$DMG" "$DMG.sha256"
"$DMGBUILD" -s packaging/dmg-settings.py \
    -D "app=$APP" -D "background=$WORK/background.tiff" -D "volume_icon=$APP/Contents/Resources/icon.icns" \
    "${defines[@]}" "$VOLUME" "$DMG" > "$WORK/dmgbuild.log" 2>&1 || { tail -20 "$WORK/dmgbuild.log" >&2; exit 1; }
hdiutil verify -quiet "$DMG"
( cd "$OUTPUT" && shasum -a 256 MediaForge.dmg > MediaForge.dmg.sha256 )
echo "    size        $(du -h "$DMG" | cut -f1)"
echo "    sha256      $(cut -d' ' -f1 "$DMG.sha256")"
echo "Created: ${DMG#$ROOT/}"
