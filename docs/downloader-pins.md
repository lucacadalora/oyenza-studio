# Downloader pins

The Downloader add-on gets three programs when it is switched on: on Windows yt-dlp.exe and qjs.exe from their own GitHub releases, unchanged, and ffmpeg.exe from the add-on's own release; on a Mac yt-dlp (yt-dlp's `yt-dlp_macos`, unchanged) from its release, and qjs and ffmpeg, our own builds, from the Mac add-on's release. The app keeps a file only if its size and SHA-256 are the ones built into it (`DOWNLOADER_WINDOWS` and `DOWNLOADER_MAC` in `crates/poni-core/src/addons.rs`), so a changed or broken file is refused. It also means a newer yt-dlp reaches people only with a new release of Oyenza Studio.

## yt-dlp, every week

yt-dlp ships fixes most weeks: sites keep changing, YouTube most of all, and an older yt-dlp soon stops working on them. So its pin moves with it.

- **Every Monday**, and by hand (Actions › Downloader pins › Run workflow), `.github/workflows/ytdlp-pin.yml` runs `tools/pin_ytdlp.py` on main.
- **The script** reads yt-dlp's latest release from the GitHub API. It takes the size of `yt-dlp.exe` and `yt-dlp_macos` from there, and each one's SHA-256 twice: the API's `digest` and the file's line in the release's `SHA2-256SUMS`. If the two disagree, it stops and changes nothing. Otherwise it rewrites the size, sha256 and url of yt-dlp.exe in `DOWNLOADER_WINDOWS` and of yt-dlp_macos in `DOWNLOADER_MAC`, both to the same release, and nothing else. It never moves the pin back to an older version.
- **When the pin moves,** the change goes to the `downloader/ytdlp-pin` branch, and the pull request `Downloader: yt-dlp <version>` opens, or the open one is updated. The branch belongs to the workflow: it is rewritten from main when a newer yt-dlp is out, so commits added there don't last. Closing the pull request skips that version; the next yt-dlp opens a new one.
- **Run ci on the branch before merging.** GitHub starts no workflow for a push made with a workflow's own token, so the pull request comes without checks: Actions › ci › Run workflow on `downloader/ytdlp-pin`, or `gh workflow run ci.yml --ref downloader/ytdlp-pin`. Read yt-dlp's release notes too: if they ask for a newer QuickJS, pin it as well (below), in its own pull request.
- **Once, in the repository's settings:** Settings › Actions › General › Workflow permissions › **Allow GitHub Actions to create and approve pull requests**. Without it GitHub refuses the pull request, and the run fails with a link to open it by hand.
- Forks don't run it.

Once merged, the next release carries the new yt-dlp. A copy with the Downloader on checks its files at start, finds that yt-dlp no longer matches, and downloads the new one, once (23.6 MB on Windows and 37.5 MB on a Mac for 2026.10.26); the other files stay. The add-on's `tag` doesn't change, since yt-dlp comes from its own release.

## QuickJS-ng, by hand

qjs.exe, which solves YouTube's JavaScript for yt-dlp, changes rarely. Pin it by hand when yt-dlp asks for a newer one or a QuickJS-ng release fixes something the Downloader needs. Its releases carry no checksum file, so the script downloads `qjs-windows-x86_64.exe` (about 2 MB) and hashes it as the second source. The Mac's qjs is our own build of the same version: move `QJS_COMMIT` in mac.yml's `downloader-tools` job to the new release's commit, then put the job's `qjs-mac` on a new `addon-downloader-mac-N` pre-release (saved as `qjs`, with the job's ffmpeg and a NOTICE.txt naming the version) and pin it in `DOWNLOADER_MAC`.

```sh
python3 tools/pin_ytdlp.py --check            # is there a newer yt-dlp? changes nothing
python3 tools/pin_ytdlp.py                    # pin it in addons.rs
python3 tools/pin_ytdlp.py --quickjs --check  # the same for QuickJS-ng
python3 tools/pin_ytdlp.py --quickjs
```

`GH_TOKEN` raises the GitHub API's rate limit (`GH_TOKEN=$(gh auth token) python3 tools/pin_ytdlp.py --check`). ffmpeg.exe and the Mac's ffmpeg are our own builds, pinned by hand when they go on their add-on's release.
