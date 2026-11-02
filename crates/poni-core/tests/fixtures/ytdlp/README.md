# yt-dlp output, recorded

Real output of yt-dlp 2026.10.26 for the Downloader's parser tests. Recorded on 28 Oct 2026
between 19:40 and 20:00 UTC on Linux x86_64 with the official `yt-dlp_linux` build (Python
3.14.10 inside, SHA-256 `767ba9dc…6795`), system FFmpeg 6.1.1 and `LANG=en_US.UTF-8`. yt-dlp
ran with stdin closed and stdout and stderr on two separate pipes, the way the island runs it.

Each case has four files:

- `<case>.cmd`: the exact command, quoted for a POSIX shell. `<dir>` is the download folder and
  `<ffmpeg>` is `/usr/bin/ffmpeg`.
- `<case>.stdout` and `<case>.stderr`: what yt-dlp wrote, byte for byte, with two edits only: the
  download folder's absolute path reads `<dir>`, and long runs of progress lines keep their
  first and last 30 lines (table at the end).
- `<case>.exit`: the exit code.

No media files. `.gitattributes` marks the folder `-text`, so the bytes are the same on every
system.

## Arguments

These are the arguments the fixtures were recorded with (`peertube_best.cmd`). They differ from
the first draft in five ways, each explained under "What changed and why".

```rust
const YTDLP_ARGS: &[&str] = &[
    "--ignore-config",
    "--no-plugin-dirs",
    "--color", "never",
    "--newline",
    "--encoding", "utf-8",
    "--no-simulate",
    "--no-quiet",
    "--progress",
    "--progress-template",
    r#"download:{"t":"dl","id":"%(info.id)s","st":"%(progress.status)s","done":%(progress.downloaded_bytes|0)s,"total":%(progress.total_bytes|0)s,"est":%(progress.total_bytes_estimate|0)s,"speed":%(progress.speed|0)s,"eta":%(progress.eta|0)s,"frag":%(progress.fragment_index|0)s,"frags":%(progress.fragment_count|0)s}"#,
    "--progress-template",
    r#"postprocess:{"t":"pp","id":"%(info.id)s","pp":"%(progress.postprocessor)s","st":"%(progress.status)s"}"#,
    "--print",
    r#"before_dl:{"t":"info","id":%(id|null)j,"title":%(title|null)j,"site":%(extractor_key|null)j,"thumb":%(thumbnail|null)j,"duration":%(duration|null)j,"uploader":%(uploader|null)j}"#,
    "--print",
    r#"after_move:{"t":"done","path":%(filepath|null)j}"#,
];
// Then, per job:
//   "--ffmpeg-location", <ffmpeg.exe>,
//   "--no-js-runtimes", "--js-runtimes", "quickjs:<qjs.exe>",
//   audio only: "-x", "--audio-format", "m4a", "-S", "+res",
//   "-o", "<dir>/%(title).180B [%(id)s].%(ext)s",
//   "--", <url>
```

- `--` before the URL: without it, pasted text that starts with `-` is read as an option.
  `--exec=…` was taken as an option in a test, and `--config-locations=<file>` would load a
  whole file of options. With `--`, it is a URL that fails (`error_not_a_url`). The other
  recordings put the URL last without `--`; it changes nothing in the output.
- The two JavaScript-runtime flags were not in the recordings (there is no qjs here). Passing
  them with a qjs path that doesn't exist printed nothing extra on PeerTube.
- `-S +res` for audio: see surprise 7. It was used in `hls_ts_audio_m4a` only.

The island's own arguments are `poni_core::ytdlp::args`, whose tests read these files. Its JSON
lines are these, with `|null` ids and two `info` fields more (`site` from `%(extractor)j`,
and `domain`). It also passes `--no-plugin-dirs`, `--cache-dir`, `--progress-delta 0.5`,
`--no-playlist --playlist-items 1` and `--merge-output-format mp4/webm/mkv`. It leaves out
`-x` and `--audio-format`: our FFmpeg only copies, so the sound comes as the site sends it.

### What changed and why

1. `--no-quiet`: `--print` turns quiet mode on, which removes yt-dlp's own lines from stdout
   (`original_flags_quiet` has only JSON lines) and moves the `postprocess:` lines to stderr.
   `--no-quiet` brings them back. Warnings and errors print either way.
2. `--no-simulate`: `--print` also turns on simulate, but not for the `before_dl`/`after_move`
   stages, so the file downloaded without it. Kept as a guard.
3. `--color never` instead of `--no-colors`: `--no-colors` is a hidden alias for
   `--color no_color`, which still writes cursor codes. With `--newline` both wrote plain
   lines; without `--newline`, `--no-colors` wrote `\r\x1b[K` before each progress line
   (`nonewline_nocolors`) and `--color never` a plain `\r`.
4. `|null` in the `j` fields: a missing field prints `NA`, not `null`, so the first draft wrote
   `"thumb":NA,"duration":NA,"uploader":NA` for a generic source (`original_template_na`).
   Apart from the `\r`-joined lines in `nonewline_nocolors`, it's the only JSON line in these
   fixtures that doesn't parse.
5. `"st"` in progress lines and the `postprocess:` template: the `finished` line can't be told
   apart by numbers alone (surprise 3), and the `{"t":"pp"}` lines say when merging or
   converting starts and ends.

## Which stream gets what

stdout, one line each, ending in `\n`:

- `[<site>] …` such as `[PeerTube] <id>: Downloading video JSON`, `[PeerTube:Playlist] …`,
  `[archive.org] …`, `[generic] …`, and `[info] <id>: Downloading 1 format(s): <ids>`
- `[download] Destination: <path>`, `[download] Resuming download at byte <n>`,
  `[download] <path> has already been downloaded`, `[download] Downloading playlist: <title>`,
  `[download] Downloading item <i> of <n>`, `[download] Finished downloading playlist: <title>`
- `[hlsnative] Downloading m3u8 manifest`, `[hlsnative] Total fragments: <n>`,
  `[dashsegments] Total fragments: <n>`
- `[Merger] Merging formats into "<path>"`, `[FixupM3u8] Fixing MPEG-TS in MP4 container of
  "<path>"`, `[FixupM4a] Correcting container of "<path>"`, `[FixupDuplicateMoov] Fixing
  duplicate MOOV atoms of "<path>"` (these four quote the path), `[ExtractAudio] Destination:
  <path>`, `[ExtractAudio] Not converting audio <path>; file is already in target format m4a`
- `Deleting original file <path> (pass -k to keep)`, with no `[…]` tag
- all four JSON lines: `{"t":"info"}`, `{"t":"dl"}`, `{"t":"pp"}`, `{"t":"done"}`
- `--version` and `-J` print their one line here too.

stderr:

- `WARNING: …` and `ERROR: …`
- an empty line before `ERROR: Interrupted by user`
- option errors: an empty line, `Usage: yt-dlp_linux [OPTIONS] URL [URL...]`, an empty line,
  `yt-dlp_linux: error: …` (the program name follows the file name)

In quiet mode (without `--no-quiet`) the `postprocess:` lines go to stderr instead; the
`download:` and `--print` lines stay on stdout.

The two pipes are separate, so the order between them is only roughly known. In these runs
PeerTube's warning came right after `Downloading description JSON`, `Falling back on generic
information extractor` came between `Downloading webpage` and `Extracting information`, and
each `ERROR` came after the last stdout line of the step that failed. In a playlist, a failed
item's `ERROR` comes before the playlist's last lines.

## Line endings and encoding

- Every line ends in `\n` alone. No fixture has a `\r` byte or an escape code, except
  `nonewline_nocolors` (6 `\r`, 5 `ESC`), recorded without `--newline` to show what happens.
  The `\r\n` in `peertube_json.stdout` is JSON text (backslash, `r`), not bytes.
- UTF-8. Non-ASCII shows as raw UTF-8 in the `[…]` and `Deleting` lines and as `\uXXXX` in
  the JSON lines (yt-dlp's `j` writes ASCII). File names swap some characters for fullwidth
  ones: the title `OVERGROWN: a feature …` is saved as `OVERGROWN： a feature …` (U+FF1A,
  written `\uff1a` in JSON), and `…Eevee?` as `…Eevee？` (U+FF1F). The `info`
  line keeps the real title.

## The JSON lines

- `dl`: `st` is `downloading` or `finished`. `done`, `total`, `frag` and `frags` are integers;
  `est`, `speed` and `eta` are integers or floats.
- `pp`: `pp` was one of `Merger`, `ExtractAudio`, `FixupM3u8`, `FixupM4a`, `FixupDuplicateMoov`,
  `MoveFiles` and, once per playlist, `Concat` (with the playlist's id); `st` is `started` or
  `finished`. `MoveFiles` runs for every download.
- `info`: `duration` is an integer, a float (`596.46`) or `null`; `thumb` and `uploader` can be
  `null`.
- `done`: the final file, after merging or converting.

## Surprises

1. Blender's PeerTube never merges. Its best format (`6`) is one HLS stream with both video and
   audio. Forcing `-f 480p-1+Audio_only` quietly downloaded only `480p-1`: that site gives no
   codecs, so yt-dlp counts both files as video and audio and drops the second (seen, not kept
   as a fixture). The merge fixture (`dash_merge`) therefore comes from Akamai's public DASH
   test stream of Big Buck Bunny, which has separate video and audio like YouTube.
2. A merge downloads two files one after the other, each with its own progress run from 0 and
   the same `id`. `[download] Destination:` names the part (`….fbbb_30fps_320x180_200k.mp4`,
   `….fbbb_a64k.m4a`). Then `[Merger] Merging formats into "<final>"` and one `Deleting
   original file` per part.
3. "Already downloaded" (`peertube_already_downloaded`) prints `[download] <path> has already
   been downloaded` and a `finished` line with `"done":0,"total":23256496`: 0% if you divide.
4. HLS and DASH: `total` stays 0 for the whole run, so use `est`, which swings early (25 KB, then
   26 MB, settling at 23 MB). `frag` can end one past `frags` (19 of 18). `eta` is a float and
   stops changing near the end. The `finished` line has the real size in `done` and `total`
   and sets `frag` and `frags` to 0.
5. Stop (SIGINT): exit 1, `\nERROR: Interrupted by user\n` on stderr, no `done` line. The
   `.part` file stays (HLS also leaves `.part-Frag3.part` and a `.ytdl`). Running the same
   command again resumes: `[download] Resuming download at byte 37747712` comes before
   `[download] Destination:`, `done` carries on from that byte, and the `finished` speed is the
   whole size over this run's time, so it is too high. An HLS resume prints nothing special.
6. A resumed HLS file isn't the same file. The resumed PeerTube download is 1,393 bytes larger
   than a clean one: yt-dlp wrote the init segment (`ftyp` and `moov`) again where it resumed.
   FFmpeg decodes it fully (1,670 frames, like the clean one), but strict MP4 readers, and our
   own muxer later, may not. Restarting an HLS download from scratch (deleting `.part`,
   `.part-Frag*` and `.ytdl`) avoids it.
7. Audio (`-x --audio-format m4a`) on that PeerTube downloads the 23 MB 1080p video to keep 2.8 MB
   of audio, because yt-dlp can't tell its `Audio_only` file is audio. `-f bestaudio` fails
   there (`error_format_unavailable`). `-S +res` makes it take the 2.8 MB file (checked with
   `--print`). On Mux's stream, which has no audio-only file, `-x` alone picks the 6.2 Mbit/s
   1080p variant (about 490 MB); with `-S +res` it took the 240p one (22 MB).
8. Warnings that are not failures: every Blender PeerTube run prints `WARNING: [PeerTube] <id>:
   Unable to download webpage: HTTP Error 400: Bad Request`, and generic pages print
   `WARNING: [generic] Falling back on generic information extractor` (also before `ERROR:
   Unsupported URL`).
9. Exit codes: 0 when done, also for "already downloaded"; 1 for any `ERROR`, including Stop and
   a playlist that prints `Finished downloading playlist` after a failed item; 2 for a bad
   option.
10. Playlists: `info` and `done` print once per item, item numbers count only the chosen items
    ("item 1 of 2"), and a `Concat` step runs at the end. Item 5 of Blender's "shorts" playlist
    is private; yt-dlp turns it into `https://video.blender.org/w/None` and fails with `ERROR:
    No suitable extractor (PeerTube) found for URL https://video.blender.org/w/None`.
11. Missing pages: a PeerTube video id that doesn't exist gives `ERROR: [PeerTube] <id>: Unable
    to download webpage: HTTP Error 404: Not Found (caused by <HTTPError 404: Not Found>)`, as a
    deleted video would. A missing page on the same site comes back as PeerTube's app page with
    status 200, so it's `ERROR: Unsupported URL: …`.
12. DNS: `ERROR: [generic] clip: Unable to download webpage: HTTPSConnection(host=…, port=443):
    Failed to resolve '…' ([Errno -2] Name or service not known) (caused by …)`. Windows has
    other errno text, so match on `Failed to resolve`.
13. archive.org format ids are positions after sorting: the same ogv file is `0` in `-J` and
    `2` with `-S +size`.
14. `yt-dlp_linux` runs as two processes (a PyInstaller launcher and the real one), and FFmpeg
    as a third while merging or converting. Stop has to end all of them. The Windows exe is
    built the same way (not checked here).
15. Progress lines come once per network read: 224 lines in 10 s for the 23 MB HLS film, 2,405 in
    274 s for the slow DASH stream, 26 for a 47 MB file at 12 MB/s. `--progress-delta 0.5` would
    thin them; it wasn't used here.

## Cases

| Case | Source | What it shows | Exit |
|---|---|---|---|
| `version` | none | `--version` | 0 |
| `peertube_json` | video.blender.org | `-J` | 0 |
| `peertube_best` | video.blender.org | default format: HLS, 18 fragments | 0 |
| `peertube_already_downloaded` | video.blender.org | the same command again | 0 |
| `peertube_audio_m4a` | video.blender.org | `-x --audio-format m4a` over HLS | 0 |
| `hls_ts_audio_m4a` | test-streams.mux.dev | HLS in MPEG-TS, `-x`, `[FixupM3u8]` | 0 |
| `dash_audio_only_m4a` | dash.akamaized.net | audio-only DASH, `-x`, "Not converting" | 0 |
| `dash_merge` | dash.akamaized.net | `-f "wv*+wa"`: video and audio parts, `[Merger]` | 0 |
| `archive_interrupted` | archive.org | smallest file (`-S +size`), SIGINT 3 s into the download | 1 |
| `archive_resumed` | archive.org | the same command again | 0 |
| `peertube_hls_interrupted` | video.blender.org | SIGINT 2 s into an HLS download | 1 |
| `peertube_hls_resumed` | video.blender.org | the same command again | 0 |
| `peertube_playlist` | video.blender.org | `--playlist-items 1-2` | 0 |
| `peertube_playlist_private_item` | video.blender.org | `--playlist-items 5`, a private video | 1 |
| `generic_direct_file` | video.blender.org | a direct `.mp4` link | 0 |
| `error_unsupported_url` | example.com | `ERROR: Unsupported URL` | 1 |
| `error_peertube_404` | video.blender.org | a video id that doesn't exist | 1 |
| `error_page_404` | video.blender.org | a page that doesn't exist | 1 |
| `error_dns` | `.invalid` host | the name doesn't resolve; nothing fetched | 1 |
| `error_format_unavailable` | video.blender.org | `-f bestaudio` | 1 |
| `error_bad_option` | none | an unknown option | 2 |
| `error_not_a_url` | none | `-- --version`: pasted text that looks like an option | 1 |
| `original_flags_quiet` | video.blender.org | the first draft's flags: quiet, JSON lines only | 0 |
| `original_template_na` | dash.akamaized.net | the first draft's flags: `NA` in JSON | 0 |
| `nonewline_nocolors` | video.blender.org | no `--newline`, `--no-colors`: `\r\x1b[K` | 0 |

The last three cases used yt-dlp's hidden `--test` option, which downloads only the first
10,241 bytes or the first fragment, so their `finished` lines show those sizes. `--test` also
hides the generic warning.

SIGINT went to yt-dlp's process id (the launcher, which passes it on), 3 s after the first
progress line for archive.org and 2 s for HLS.

## Shortened files

Runs of `{"t":"dl"…}` lines longer than 60 keep their first 30 and last 30 lines. Nothing marks
the cut; `done` jumps there.

| File | Lines recorded | Lines kept | Cut before line |
|---|---|---|---|
| `peertube_best.stdout` | 224 | 74 | 40 (150 removed) |
| `peertube_audio_m4a.stdout` | 228 | 78 | 40 (150) |
| `hls_ts_audio_m4a.stdout` | 655 | 79 | 40 (576) |
| `dash_audio_only_m4a.stdout` | 1,132 | 79 | 38 (1,053) |
| `dash_merge.stdout` | 2,405 | 137 | 38 (1,215, video) and 100 (1,053, audio) |
| `peertube_hls_resumed.stdout` | 211 | 74 | 40 (137) |
| `peertube_playlist.stdout` | 235 | 158 | 46 (42, item 1) and 121 (35, item 2) |

## Not recorded

- SoundCloud: soundcloud.com/robots.txt (updated 21 Oct 2026) closes every track page to
  Claude-User, so nothing was fetched from SoundCloud. The audio cases above come from open
  sources instead.
- YouTube: youtube.com/robots.txt closes `/youtubei/`, which yt-dlp calls for every video, and
  YouTube refuses this server anyway. Record it on a home connection. Its refusal text comes
  from YouTube (`Sign in to confirm you’re not a bot`, with a curly apostrophe), so match on
  `not a bot`.
- Windows. Expect backslashes in paths (`\\` inside the JSON), more characters swapped in file
  names, other DNS error text and the exe's own name in option errors. The JSON lines are
  ASCII, so the console code page can't change them; the `[…]` lines rely on
  `--encoding utf-8`.

## Sources

All robots.txt files were read before the first request: video.blender.org allows everything,
archive.org closes only `/control/` and `/report/`, and test-streams.mux.dev, dash.akamaized.net
and example.com have none. The films are Blender Studio's (PeerTube lists the OVERGROWN clip's
license as Unknown) and Big Buck Bunny (CC BY 3.0, Blender Foundation). `HOME` and
`XDG_CACHE_HOME` pointed at an empty folder, so no user settings or cache were involved.
