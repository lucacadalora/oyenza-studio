#!/usr/bin/env bash
# The Downloader add-on's ffmpeg.exe: FFmpeg's own release, LGPL only, with
# just what yt-dlp needs to join and fix downloads by copying streams (never
# re-encoding). addons/downloader/FFMPEG.md says what's in it, why, and the
# licence. Builds the Windows exe (mingw-w64, static) and the same build for
# Linux, which runs here so yt-dlp can be tried with --ffmpeg-location.
#
#   tools/build-ffmpeg-min.sh [out-dir]          (default dist/ffmpeg)
#
# Makes <out>/windows/ffmpeg.exe, <out>/linux/ffmpeg and <out>/SHA256SUMS, and
# keeps the source tarball next to them (it ships with the exe). Needs gcc,
# make, curl and x86_64-w64-mingw32-gcc (apt install gcc-mingw-w64-x86-64-win32);
# no nasm, pkg-config or libraries. Same compilers, same bytes: the build is
# in-tree (no paths in the exe) and the exe's header holds a fixed time.
#   FFMPEG_TARBALL=<file>    use this ffmpeg-9.0.2.tar.xz instead of downloading it
#   TARGETS="windows"        build only these ("linux windows")
#   JOBS=6                   make -j (all cores)
set -euo pipefail

version=9.0.2
# https://ffmpeg.org/releases/ffmpeg-9.0.2.tar.xz, its .asc checked against
# FFmpeg's release key FCF986EA15E6E293A5644F10B4322F04D67658D8 (29 Oct 2026).
sha256=8c3850283eb25fa026482078a04051e0be17347b09ef81a0849bec15a96e002e

# The time the linker and strip write into the exe's header: 9.0.2's release
# (its signature's), not the time of the build.
export SOURCE_DATE_EPOCH=1789699562

out="$(mkdir -p "${1:-dist/ffmpeg}" && cd "${1:-dist/ffmpeg}" && pwd)"
targets="${TARGETS:-linux windows}"
jobs="${JOBS:-$(nproc)}"
tarball="$out/ffmpeg-$version.tar.xz"

flags=(
  # LGPL 2.1 or later: no --enable-gpl, no --enable-nonfree (checked below).
  # Nothing by default; every part the exe has is named here.
  --disable-everything
  # No outside libraries (zlib, iconv, SChannel...) and no hardware decoding
  # (DXVA2, D3D11, CUDA...): the exe needs only Windows' own DLLs.
  --disable-autodetect
  # yt-dlp downloads; ffmpeg only reads and writes the files it's given.
  --disable-network --enable-protocol=file
  # ffmpeg alone: yt-dlp works without ffprobe (FFMPEG.md).
  --disable-ffplay --disable-ffprobe
  # ffmpeg needs avutil, avcodec, avformat and avfilter. Scaling, resampling
  # and capture devices are for re-encoding and recording.
  --disable-avdevice --disable-swscale --disable-swresample
  # Copying streams does no signal processing: no assembly (so no nasm),
  # size over speed, and link-time optimization drops what's never called.
  --disable-asm --disable-runtime-cpudetect --enable-small --enable-lto
  --disable-pixelutils --disable-faan --disable-iamf --disable-doc --disable-debug
  # What yt-dlp hands it: mov reads MP4 and M4A (DASH and HLS fMP4 too),
  # matroska WebM and MKV, mpegts the .ts segments of an HLS download, aac
  # the raw AAC of HLS "packed audio".
  --enable-demuxer=mov,matroska,mpegts,aac
  # What it writes: mp4 (.mp4 and -f mp4), ipod (.m4a), mov (.mov), matroska
  # (.mkv) and webm (.webm), the formats yt-dlp merges into.
  --enable-muxer=mp4,ipod,mov,matroska,webm
  # MPEG-TS carries H.264, HEVC and AAC as a stream of bytes: these cut it
  # into frames and read the picture size. Opus and VP9 time WebM's packets.
  # (The muxers add the AC-3 parser themselves.)
  --enable-parser=h264,hevc,aac,opus,vp9
  # Two decoders, only to look at a few frames when a file opens; what ffmpeg
  # copies is never decoded. MPEG-TS doesn't say an AAC track's sample rate or
  # channels, and the MP4 muxer won't start without them (aac_fixed is 32 KB
  # smaller than aac). VP9's box in MP4 needs its pixel format, which neither
  # WebM nor MP4 states: without the decoder, VP9 merged into MP4 (yt-dlp's
  # pick for VP9-in-MP4 plus M4A) comes out unplayable and ffmpeg still says
  # it worked. The VP9 decoder is about 560 KB of the exe.
  --enable-decoder=aac_fixed,vp9
  # aac_adtstoasc: MPEG-TS AAC into MP4's form (yt-dlp asks for it). setts:
  # yt-dlp's timestamp fix without re-encoding. vp9_superframe: the MP4 and
  # Matroska muxers insert it for VP9 (they also add pgs_frame_merge).
  --enable-bsf=aac_adtstoasc,setts,vp9_superframe
  # No encoders: yt-dlp only ever copies (-c copy).
  --extra-cflags="-ffunction-sections -fdata-sections"
)

if [ -n "${FFMPEG_TARBALL:-}" ]; then
  [ "$FFMPEG_TARBALL" -ef "$tarball" ] || cp "$FFMPEG_TARBALL" "$tarball"
elif ! echo "$sha256  $tarball" | sha256sum -c --status 2>/dev/null; then
  curl -fsSL --retry 3 -o "$tarball" "https://ffmpeg.org/releases/ffmpeg-$version.tar.xz"
fi
echo "$sha256  $tarball" | sha256sum -c --quiet

for target in $targets; do
  case "$target" in
    linux)
      exe=ffmpeg
      tflags=(--extra-ldflags="-Wl,--gc-sections -static") ;;
    windows)
      exe=ffmpeg.exe
      tflags=(--enable-cross-compile --target-os=mingw32 --arch=x86_64 --cross-prefix=x86_64-w64-mingw32-
              --extra-ldflags="-Wl,--gc-sections -static") ;;
    *) echo "unknown target $target (linux or windows)" >&2; exit 1 ;;
  esac
  src="$out/build/$target"
  rm -rf "$src" && mkdir -p "$src"
  tar -xJf "$tarball" -C "$src" --strip-components=1
  echo "== $target: configure"
  ( cd "$src" && ./configure "${flags[@]}" "${tflags[@]}" > configure.log 2>&1 ) || { tail -20 "$src/configure.log"; exit 1; }
  grep -q '^License: LGPL version 2.1 or later$' "$src/configure.log" || { echo "configure didn't end up LGPL" >&2; exit 1; }
  echo "== $target: make -j$jobs"
  make -C "$src" -j"$jobs" > "$src/make.log" 2>&1 || { tail -30 "$src/make.log"; exit 1; }
  mkdir -p "$out/$target"
  cp "$src/$exe" "$out/$target/$exe"
done

# The Linux build runs here: make sure configure kept every part yt-dlp uses.
if [[ " $targets " == *" linux "* ]]; then
  for want in "-bsfs aac_adtstoasc" "-bsfs setts" "-demuxers mov,mp4" "-demuxers matroska,webm" "-demuxers mpegts" \
              "-muxers mp4" "-muxers ipod" "-muxers matroska" "-muxers webm" "-decoders aac_fixed" "-decoders vp9"; do
    list="$("$out/linux/ffmpeg" -hide_banner ${want% *} 2>/dev/null)"
    grep -qw -- "${want#* }" <<< "$list" || { echo "the build has no ${want#* } (${want% *})" >&2; exit 1; }
  done
fi
rm -rf "$out/build"

( cd "$out" && for target in $targets; do sha256sum "$target"/ffmpeg*; done ) > "$out/SHA256SUMS"
{
  echo "### Downloader ffmpeg, FFmpeg $version"
  echo
  echo "| File | Bytes | SHA-256 |"
  echo "|---|---|---|"
  while read -r sum file; do echo "| $file | $(stat -c %s "$out/$file") | \`$sum\` |"; done < "$out/SHA256SUMS"
} | tee -a "${GITHUB_STEP_SUMMARY:-/dev/null}"
