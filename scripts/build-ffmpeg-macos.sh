#!/bin/bash
# Builds the FFmpeg and FFprobe that ship inside MediaForge for macOS, from the pinned sources in tools/downloads.
#
# The result is an LGPL build: no --enable-gpl, no --enable-nonfree, no GPL libraries. It is universal
# (Apple silicon + Intel), statically linked apart from macOS system libraries, and contains only the
# encoders MediaForge uses. Decoders and demuxers are left complete so any file a customer drops can be read.
#
# Usage: scripts/build-ffmpeg-macos.sh        (run `node scripts/fetch-tools.mjs macos` first; it stops at the
#        point where it needs this build, after downloading the sources)
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DOWNLOADS="$ROOT/tools/downloads"
OUT="$ROOT/tools/macos/ffmpeg-build"
# Both projects' build systems reject paths containing spaces, so compile in a temporary folder.
WORK="$(mktemp -d "${TMPDIR:-/tmp}/mediaforge-ffmpeg.XXXXXX")"
case "$WORK" in *[[:space:]]*) echo "Temporary folder path contains spaces: $WORK" >&2; exit 1 ;; esac
CURRENT_LOG=""
trap 'status=$?; if [ $status -ne 0 ] && [ -n "$CURRENT_LOG" ]; then echo "--- failed; end of $CURRENT_LOG" >&2; tail -25 "$CURRENT_LOG" >&2; fi; rm -rf "$WORK"; exit $status' EXIT

FFMPEG_VERSION="9.0.2"
LAME_VERSION="4.0"
# Lower than the app's own minimum on purpose, so the tools don't hold back a future decision to support
# older systems.
MIN_MACOS="11.0"
JOBS="$(sysctl -n hw.ncpu)"

# Encoders MediaForge needs: its six audio outputs, the few yt-dlp uses for metadata and subtitles, and
# macOS's own H.264 and HEVC encoders (VideoToolbox) for making 4K downloads playable. Those two are part of
# macOS, not of FFmpeg: no video-encoder code is compiled in, and the build stays LGPL.
ENCODERS="aac,aac_at,alac,flac,libmp3lame,pcm_s16le,pcm_s24le,pcm_s16be,pcm_s24be,pcm_s32le,pcm_f32le,mjpeg,png,srt,subrip,webvtt,ass,mov_text,h264_videotoolbox,hevc_videotoolbox"

for archive in "ffmpeg-$FFMPEG_VERSION.tar.xz" "lame-$LAME_VERSION.tar.gz"; do
    [ -f "$DOWNLOADS/$archive" ] || { echo "Missing $archive. Run `node scripts/fetch-tools.mjs macos` first." >&2; exit 1; }
done

rm -rf "$OUT"
mkdir -p "$WORK/src" "$WORK/bin" "$OUT"

# LAME's configure demands a pkg-config program. This stand-in answers "nothing optional is installed",
# which is what we want anyway: the build must not pick up libraries that happen to be on this Mac.
cat > "$WORK/bin/pkg-config" <<'STUB'
#!/bin/sh
case "$1" in
    --version) echo "0.29.2"; exit 0 ;;
    --atleast-pkgconfig-version) exit 0 ;;
esac
exit 1
STUB
chmod 755 "$WORK/bin/pkg-config"
export PATH="$WORK/bin:/usr/bin:/bin:/usr/sbin:/sbin"
export PKG_CONFIG="$WORK/bin/pkg-config"
unset PKG_CONFIG_PATH PKG_CONFIG_LIBDIR CPATH LIBRARY_PATH CFLAGS LDFLAGS CPPFLAGS
tar -xf "$DOWNLOADS/ffmpeg-$FFMPEG_VERSION.tar.xz" -C "$WORK/src"
tar -xf "$DOWNLOADS/lame-$LAME_VERSION.tar.gz" -C "$WORK/src"
FFMPEG_SRC="$WORK/src/ffmpeg-$FFMPEG_VERSION"
LAME_SRC="$WORK/src/lame-$LAME_VERSION"

build_arch() {
    local arch="$1" triple="$2"
    local prefix="$WORK/$arch/prefix"
    local flags="-arch $arch -mmacosx-version-min=$MIN_MACOS"

    echo "=== LAME $LAME_VERSION ($arch)"
    mkdir -p "$WORK/$arch/lame" && cd "$WORK/$arch/lame"
    CURRENT_LOG="$PWD/configure.log"
    "$LAME_SRC/configure" --prefix="$prefix" --host="$triple" \
        --disable-shared --enable-static --disable-frontend --disable-decoder --enable-nasm=no \
        CC="clang" CFLAGS="-O2 $flags" LDFLAGS="$flags" > "${CURRENT_LOG:=$PWD/configure.log}" 2>&1
    CURRENT_LOG="$PWD/make.log"; make -j"$JOBS" > "$CURRENT_LOG" 2>&1
    CURRENT_LOG="$PWD/install.log"; make install > "$CURRENT_LOG" 2>&1

    echo "=== FFmpeg $FFMPEG_VERSION ($arch)"
    mkdir -p "$WORK/$arch/ffmpeg" && cd "$WORK/$arch/ffmpeg"
    CURRENT_LOG="$PWD/configure.log"
    local extra=()
    # No assembler for x86 is installed (nasm), so the Intel build uses the portable C code paths.
    [ "$arch" = "x86_64" ] && extra+=(--disable-x86asm)
    "$FFMPEG_SRC/configure" \
        --prefix="$prefix" \
        --enable-cross-compile --target-os=darwin --arch="$arch" --cc="clang -arch $arch" \
        --extra-cflags="-mmacosx-version-min=$MIN_MACOS -I$prefix/include" \
        --extra-ldflags="-mmacosx-version-min=$MIN_MACOS -L$prefix/lib" \
        --disable-shared --enable-static \
        --disable-debug --disable-doc --disable-ffplay --disable-avdevice \
        --disable-autodetect --pkg-config="$WORK/bin/pkg-config" \
        --enable-zlib --enable-bzlib --enable-securetransport --enable-audiotoolbox --enable-videotoolbox \
        --enable-libmp3lame \
        --disable-encoders --enable-encoder="$ENCODERS" \
        ${extra[@]+"${extra[@]}"} > "$CURRENT_LOG" 2>&1
    CURRENT_LOG="$PWD/make.log"; make -j"$JOBS" > "$CURRENT_LOG" 2>&1
    CURRENT_LOG="$PWD/install.log"; make install > "$CURRENT_LOG" 2>&1
    CURRENT_LOG=""
}

build_arch arm64 aarch64-apple-darwin
build_arch x86_64 x86_64-apple-darwin

echo "=== Combining"
for tool in ffmpeg ffprobe; do
    lipo -create "$WORK/arm64/prefix/bin/$tool" "$WORK/x86_64/prefix/bin/$tool" -output "$OUT/$tool"
    chmod 755 "$OUT/$tool"
done

# Everything a recipient is entitled to under the LGPL: the licence texts and exactly how this was built.
mkdir -p "$OUT/licenses"
cp "$FFMPEG_SRC/COPYING.LGPLv2.1" "$OUT/licenses/FFmpeg-COPYING.LGPLv2.1.txt"
cp "$FFMPEG_SRC/LICENSE.md" "$OUT/licenses/FFmpeg-LICENSE.md"
cp "$LAME_SRC/COPYING" "$OUT/licenses/LAME-COPYING.txt"
{
    echo "FFmpeg $FFMPEG_VERSION and LAME $LAME_VERSION, built by scripts/build-ffmpeg-macos.sh"
    echo "Sources: https://ffmpeg.org/releases/ffmpeg-$FFMPEG_VERSION.tar.xz"
    echo "         https://downloads.sourceforge.net/project/lame/lame/$LAME_VERSION/lame-$LAME_VERSION.tar.gz"
    echo
    for arch in arm64 x86_64; do
        echo "[$arch] $(grep -m1 '^# ' "$WORK/$arch/ffmpeg/ffbuild/config.mak" 2>/dev/null || true)"
        sed -n 's/^#define FFMPEG_CONFIGURATION "\(.*\)"$/\1/p' "$WORK/$arch/ffmpeg/config.h" | sed "s#$WORK#<build>#g"
        echo
    done
} > "$OUT/licenses/FFmpeg-BUILD-CONFIGURATION.txt"

echo "Built: ${OUT#$ROOT/}"
lipo -archs "$OUT/ffmpeg"
