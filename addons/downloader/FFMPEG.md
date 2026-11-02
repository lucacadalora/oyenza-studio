# FFmpeg in the Downloader add-on

yt-dlp fetches a video's picture and sound as separate files; `ffmpeg.exe`
joins them into one file and fixes what HLS and DASH downloads leave behind.
It only copies streams, never re-encodes, and runs only while a download
finishes. It is FFmpeg itself, unmodified, built small by
`tools/build-ffmpeg-min.sh`.

| | |
|---|---|
| Version | FFmpeg 9.0.2 "Lei" (released 18 Sep 2026) |
| `ffmpeg.exe` | 2,298,880 bytes, SHA-256 `8a42638bc2c9bed20670a3e89ccb7117345a941e245d8151aa60066cac0b3993` |
| Runs on | Windows x86-64; static, needs only Windows' own DLLs (KERNEL32, msvcrt, SHELL32, bcrypt) |
| Licence | LGPL version 2.1 or later |

The script makes the same bytes every time with Ubuntu 24.04's compilers
(mingw-w64 GCC 13.2.0-6ubuntu1+26.1, binutils 2.41.90.20240122): the build is
in-tree, so no paths end up in the exe, and the header's time is 9.0.2's
release. It also builds the same `ffmpeg` for Linux (3,526,728 bytes, SHA-256
`20387ff1c97fbf210bb4850a8b4a453bb2dbb21c3a783c07ae470c1c0b7f5d8d`) to try
yt-dlp with; that one never ships.

Configure line, as `ffmpeg.exe -version` prints it:

```
--disable-everything --disable-autodetect --disable-network --enable-protocol=file
--disable-ffplay --disable-ffprobe --disable-avdevice --disable-swscale --disable-swresample
--disable-asm --disable-runtime-cpudetect --enable-small --enable-lto --disable-pixelutils
--disable-faan --disable-iamf --disable-doc --disable-debug
--enable-demuxer='mov,matroska,mpegts,aac' --enable-muxer='mp4,ipod,mov,matroska,webm'
--enable-parser='h264,hevc,aac,opus,vp9' --enable-decoder='aac_fixed,vp9'
--enable-bsf='aac_adtstoasc,setts,vp9_superframe' --extra-cflags='-ffunction-sections -fdata-sections'
--enable-cross-compile --target-os=mingw32 --arch=x86_64 --cross-prefix=x86_64-w64-mingw32-
--extra-ldflags='-Wl,--gc-sections -static'
```

## Licence

configure ends with "License: LGPL version 2.1 or later", and the script
refuses a build that doesn't: no `--enable-gpl`, `--enable-nonfree` or
`--enable-version3`, and no outside libraries. FFmpeg is Copyright (c)
2000-2026 the FFmpeg developers. Its notice:

> FFmpeg is free software; you can redistribute it and/or modify it under the
> terms of the GNU Lesser General Public License as published by the Free
> Software Foundation; either version 2.1 of the License, or (at your option)
> any later version.
>
> FFmpeg is distributed in the hope that it will be useful, but WITHOUT ANY
> WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS
> FOR A PARTICULAR PURPOSE. See the GNU Lesser General Public License for more
> details.
>
> You should have received a copy of the GNU Lesser General Public License
> along with FFmpeg; if not, write to the Free Software Foundation, Inc., 51
> Franklin Street, Fifth Floor, Boston, MA 02110-1301 USA

Wherever `ffmpeg.exe` is published, these go with it: this file, the licence
text (`COPYING.LGPLv2.1` from the tarball: 26,517 bytes, SHA-256
`246041b6ecf9bc32d718a62c57877c78b5eb397b6467e74ed7ae2626ab189c30`) and the
source below, from the same place (the add-on's pre-release), as the LGPL
asks. poni.exe starts `ffmpeg.exe` as a separate program and links none of
it, so poni.exe's own licence doesn't change, and anyone can put their own
build in its place.

## Source

- https://ffmpeg.org/releases/ffmpeg-9.0.2.tar.xz, 12,040,788 bytes, SHA-256
  `8c3850283eb25fa026482078a04051e0be17347b09ef81a0849bec15a96e002e`.
- Its signature, `ffmpeg-9.0.2.tar.xz.asc`, is good for FFmpeg's release key
  `FCF986EA15E6E293A5644F10B4322F04D67658D8` (https://ffmpeg.org/ffmpeg-devel.asc),
  checked with gpg on 29 Oct 2026. (The `.pq.asc` post-quantum signatures need
  `sq` 1.4 or later and weren't checked.)
- No patches: the tarball builds as it is.

## Rebuilding

    tools/build-ffmpeg-min.sh [out-dir]          (default dist/ffmpeg)

It downloads the tarball (or takes `FFMPEG_TARBALL=<file>`), checks the
SHA-256, builds Windows and Linux in about two minutes on six cores, checks
that every part yt-dlp uses is in, and prints sizes and SHA-256s
(`<out>/SHA256SUMS`). It needs gcc, make, curl and
`x86_64-w64-mingw32-gcc` (`apt install gcc-mingw-w64-x86-64-win32`); no nasm.
The `ci` workflow's `downloader-ffmpeg` job runs it on ubuntu-24.04 and uploads
the artifact `downloader-ffmpeg` (`ffmpeg.exe`, `SHA256SUMS`, the tarball). It
builds again only when the script changes.

## On a Mac

The Mac's Downloader runs the same build as `ffmpeg`: the same version, the
same configure flags, for Apple silicon and Intel in one file, macOS 14 and
later. The script's `darwin` target makes it on a Mac with Xcode's clang
(each half its own build, the half that isn't the Mac's own cross-compiled),
joins the halves with `lipo` and signs the file ad-hoc, since a Mac with
Apple silicon runs only signed code. mac.yml's `downloader-tools` job runs it
on macOS 15 and uploads the artifact `downloader-tools-mac`; the file is on
the `addon-downloader-mac-1` pre-release with this file, the licence and the
same source.

| | |
|---|---|
| `ffmpeg` (Mac) | 4,307,968 bytes, SHA-256 `fb47d9aa82cf1e61aa49b54d1989fffef98571d1cec3d0f9a5133e0b63468724` |
| Runs on | macOS 14 and later, Apple silicon and Intel; links only `/usr/lib/libSystem.B.dylib` |

What the Mac changes in the configure line, for each half (`arm64`, and
`x86_64` with `--enable-cross-compile` on Apple silicon):

```
--target-os=darwin --arch=arm64 --cc='clang -arch arm64' --extra-cflags=-mmacosx-version-min=14.0
--extra-ldflags='-mmacosx-version-min=14.0 -Wl,-dead_strip -Wl,-dead_strip_dylibs'
```

macOS has no static libSystem, so there is no `-static`; `-dead_strip` is
ld64's `--gc-sections`. configure always finds CoreFoundation, CoreMedia and
CoreVideo and hands them to libavutil, which uses them only for VideoToolbox
(off here), so `-dead_strip_dylibs` drops them: without it the file would
load three frameworks it never calls. A different Xcode may give different
bytes; the pinned file is the one on the release.

## What yt-dlp asks of it

From yt-dlp 2026.10.26 (`yt_dlp/postprocessor/ffmpeg.py`, `YoutubeDL.py`):

- **Finding it:** `ffmpeg -bsfs`. The banner gives the version (9.0.2), and
  `setts` in the list lets yt-dlp fix timestamps by copying. It never reads
  `-version` or `-h` (both work).
- **Joining:** `-c copy -map 0:v:0 -map 1:a:0`, plus `-bsf:a aac_adtstoasc`
  for AAC that came over HLS, into MP4, WebM or MKV, whichever fits the
  codecs (or `--merge-output-format`).
- **Fixing:** HLS MPEG-TS to MP4 (`-f mp4 -bsf:a aac_adtstoasc`), DASH M4A to
  MP4, a plain copy for duplicate MOOV boxes and durations, timestamps with
  `setts`, and `-aspect` for stretched videos. Every path is `file:...`.
- **Codec check without ffprobe:** `ffmpeg -i <file>`, read from its output.

What each part is for is in the script, next to its flag. Checked on
29 Oct 2026: yt-dlp's own commands on test files (H.264, HEVC, AV1, 8- and
10-bit VP9, AAC, Opus; MP4, WebM, MKV, MPEG-TS, fragmented and raw AAC) all
decode cleanly, and the copied streams are bit-identical to their sources;
yt-dlp joined a video.blender.org video's picture and sound into MP4 and MKV,
fixed its HLS download and turned an MPEG-TS download into MP4; the Windows
exe did the same under Wine 9.0, file names in Japanese included. Joining
70 seconds of 480p took 40 ms and 4 MB of memory.

## What it can't do

- **No ffprobe.** yt-dlp warns "Unable to extract metadata: ffprobe not found"
  once per HLS download and then remuxes every HLS MP4 or M4A, whether it
  needed it or not; the file comes out a normal MP4. An ffprobe.exe of the
  same build would add 2.1 MB.
- **No network.** When yt-dlp hands the whole download to ffmpeg (live
  streams, `--download-sections`, `--downloader ffmpeg`, formats an extractor
  marks `m3u8` instead of `m3u8_native`), ffmpeg can't open the URL and the
  download fails. yt-dlp's own downloaders do everything else.
- **No encoders.** Anything that converts fails: `-x`/`--audio-format`,
  `--recode-video`, `--embed-thumbnail`, subtitles into MP4 (`mov_text`),
  `--embed-metadata` (its `-write_id3v1` belongs to the MP3 muxer) and
  `--embed-chapters`. The add-on doesn't pass them.
- **The VP9 decoder is about 560 KB of the exe**, there only to read VP9's
  pixel format when it goes into MP4. Without it, VP9 into MP4 (yt-dlp's pick
  for VP9-in-MP4 plus M4A, or any `--merge-output-format mp4`) comes out
  unplayable while ffmpeg reports success. If the app always passes
  `--merge-output-format mp4/webm/mkv`, which sends VP9 to WebM or MKV, it
  can go.
